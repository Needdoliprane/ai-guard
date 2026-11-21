//! Verify an operator-prepared human identity projection in the audit snapshot.
use anyhow::{Context, Result, ensure};
use mysql_async::{Row, Transaction, Value, prelude::Queryable};
use std::collections::{BTreeMap, BTreeSet};

use crate::{
    audit::identity::{
        projection::{Genesis, Projection, ProjectionCell, Record},
        require_replay_capabilities,
    },
    identity::{IdentityChangeV1, ResourceKindV1, ResourceRefV1},
    model::{StoredEvent, event_kind_in_namespace},
};

use super::{audit::decode_row, driver_error};

const GENESIS: &str = "control.identity.genesis";
const MUTATION: &str = "control.identity.mutation";
const CREDENTIALS_TABLE: &str = "identity_credentials";
const CREDENTIAL_DIGEST_COLUMN: usize = 2;

const QUERIES: [(&str, &str); 7] = [
    (
        "identity_meta",
        "SELECT singleton,authorization_revision,genesis_seq,genesis_hash FROM ai_guard_identity_meta ORDER BY singleton",
    ),
    (
        "identity_accounts",
        "SELECT principal_key,payload_json,event_seq FROM ai_guard_identity_accounts ORDER BY principal_key",
    ),
    (
        CREDENTIALS_TABLE,
        "SELECT credential_id,principal_key,digest,active,payload_json,event_seq FROM ai_guard_identity_credentials ORDER BY credential_id",
    ),
    (
        "identity_memberships",
        "SELECT membership_id,principal_key,organization_scope,active,payload_json,event_seq FROM ai_guard_identity_memberships ORDER BY membership_id",
    ),
    (
        "identity_resources",
        "SELECT resource_key,resource_kind,resource_id,owner_key,active,payload_json,event_seq FROM ai_guard_identity_resources ORDER BY resource_key",
    ),
    (
        "identity_shares",
        "SELECT resource_key,principal_key,permissions_json FROM ai_guard_identity_shares ORDER BY resource_key,principal_key",
    ),
    (
        "identity_operations",
        "SELECT operation_id,input_hash,event_seq FROM ai_guard_identity_operations ORDER BY operation_id",
    ),
];

pub(super) fn is_event(kind: &str) -> bool {
    event_kind_in_namespace(kind, "control.identity")
}

async fn schema_counts(tx: &mut Transaction<'_>) -> Result<(u64, u64)> {
    let row = tx.query_first::<Row, _>(
        "SELECT COUNT(*), COUNT(CASE WHEN TABLE_TYPE='BASE TABLE' AND ENGINE='InnoDB' THEN 1 END)
         FROM information_schema.TABLES WHERE BINARY TABLE_SCHEMA=BINARY DATABASE()
         AND BINARY TABLE_NAME IN ('ai_guard_identity_meta','ai_guard_identity_accounts',
          'ai_guard_identity_credentials','ai_guard_identity_memberships','ai_guard_identity_resources',
          'ai_guard_identity_shares','ai_guard_identity_operations')",
    )
    .await
    .map_err(driver_error)?
    .context("MySQL identity schema inventory unavailable")?;
    decode_row(row)
}

pub(super) struct Replay {
    genesis_seq: i64,
    genesis_hash: String,
    seen_genesis: bool,
    projection: Projection,
    resource_references: Vec<(i64, ResourceRefV1)>,
}

pub(super) struct Verified {
    projection: Projection,
    known_digests: BTreeSet<[u8; 32]>,
}

impl Verified {
    pub(super) fn has_local_credential_binding(&self, digest: &[u8; 32]) -> bool {
        self.projection.has_local_credential_binding(digest)
    }

    pub(super) fn knows_candidate(&self, digest: &[u8; 32]) -> bool {
        self.known_digests.contains(digest)
    }
}

impl Replay {
    pub(super) async fn open(tx: &mut Transaction<'_>) -> Result<Option<Self>> {
        let counts = schema_counts(tx).await?;
        if counts.0 == 0 {
            return Ok(None);
        }
        let expected_count = QUERIES.len() as u64;
        ensure!(
            counts == (expected_count, expected_count),
            "MySQL identity requires all prepared InnoDB tables"
        );
        // Acquire the tables' metadata locks and recheck their engines in the
        // same transaction before trusting any projection data.
        for (_, query) in QUERIES {
            tx.query_drop(format!("{query} LIMIT 0"))
                .await
                .map_err(driver_error)?;
        }
        ensure!(
            schema_counts(tx).await? == counts,
            "MySQL identity schema changed during verification"
        );
        let mut markers: Vec<Row> = tx
            .query("SELECT singleton,authorization_revision,genesis_seq,genesis_hash FROM ai_guard_identity_meta LIMIT 2")
            .await
            .map_err(driver_error)?;
        ensure!(
            markers.len() == 1,
            "MySQL identity requires one prepared capability marker"
        );
        let row = markers
            .pop()
            .context("MySQL identity capability marker is missing")?;
        let (singleton, revision, genesis_seq, genesis_hash): (i64, i64, i64, String) =
            decode_row(row)?;
        ensure!(
            singleton == 1
                && revision >= 0
                && genesis_seq > 1
                && genesis_hash.len() == 64
                && genesis_hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid MySQL identity capability marker"
        );
        Ok(Some(Self {
            genesis_seq,
            genesis_hash,
            seen_genesis: false,
            projection: Projection::default(),
            resource_references: Vec::new(),
        }))
    }

    pub(super) fn observe(
        &mut self,
        stored: &StoredEvent,
        tenant: &str,
        namespace: &str,
    ) -> Result<()> {
        let event = &stored.event;
        ensure!(
            event.source == "gateway"
                && event.provenance == "observed"
                && event.coverage == "observed",
            "invalid MySQL identity event attribution"
        );
        match event.kind.as_str() {
            GENESIS => {
                let genesis: Genesis = serde_json::from_value(event.data.clone())
                    .map_err(|_| anyhow::anyhow!("invalid MySQL identity genesis payload"))?;
                ensure!(
                    !self.seen_genesis
                        && stored.seq == self.genesis_seq
                        && stored.hash == self.genesis_hash
                        && genesis.matches(namespace, stored.seq, &stored.prev_hash),
                    "MySQL identity genesis does not match the prepared capability"
                );
                self.seen_genesis = true;
            }
            MUTATION => {
                ensure!(
                    self.seen_genesis,
                    "MySQL identity mutation precedes its capability"
                );
                let record = Record::decode_replay(&event.data)
                    .map_err(|_| anyhow::anyhow!("invalid MySQL identity mutation payload"))?;
                require_replay_capabilities(&record.mutation, 4, false, false)?;
                self.projection
                    .apply(&record.mutation, tenant, stored.seq)?;
                record.verify_revision(&self.projection)?;
                if let IdentityChangeV1::ResourceGrant { grant, .. } = record.mutation.change {
                    self.resource_references.push((stored.seq, grant.resource));
                }
            }
            _ => anyhow::bail!("unsupported MySQL identity event kind"),
        }
        Ok(())
    }

    pub(super) async fn verify(
        self,
        tx: &mut Transaction<'_>,
        candidates: &BTreeMap<String, [u8; 32]>,
    ) -> Result<Verified> {
        ensure!(self.seen_genesis, "MySQL identity genesis is missing");
        for (sequence, resource) in &self.resource_references {
            ensure!(
                resource_known(tx, *sequence, resource).await?,
                "MySQL identity grant refers to an unknown prior resource"
            );
        }
        let mut projection = self.projection.rows(self.genesis_seq, &self.genesis_hash)?;
        let mut known_digests = BTreeSet::new();
        for (table, query) in QUERIES {
            let expected = projection
                .remove(table)
                .context("missing identity projection")?;
            // The binary protocol preserves the distinction between SQL
            // integers and byte strings in the projection rows.
            let mut records = tx.exec_iter(query, ()).await.map_err(driver_error)?;
            for expected_row in expected {
                if table == CREDENTIALS_TABLE && !candidates.is_empty() {
                    // This field follows the column order in QUERIES. Collect
                    // only configured digests from the canonical comparison.
                    let Some(ProjectionCell::Text(digest)) =
                        expected_row.get(CREDENTIAL_DIGEST_COLUMN)
                    else {
                        anyhow::bail!("invalid MySQL identity credential digest column");
                    };
                    if let Some(candidate) = candidates.get(digest) {
                        known_digests.insert(*candidate);
                    }
                }
                let actual = records
                    .next()
                    .await
                    .map_err(driver_error)?
                    .context("MySQL identity projection rows are missing")?;
                ensure!(
                    row_matches(actual, expected_row),
                    "MySQL identity projection differs from audit: {table}"
                );
            }
            ensure!(
                records.next().await.map_err(driver_error)?.is_none(),
                "MySQL identity projection contains unexpected rows: {table}"
            );
            records.drop_result().await.map_err(driver_error)?;
        }
        Ok(Verified {
            projection: self.projection,
            known_digests,
        })
    }
}

fn row_matches(row: Row, expected: Vec<ProjectionCell>) -> bool {
    let actual = row.unwrap();
    actual.len() == expected.len()
        && actual
            .into_iter()
            .zip(expected)
            .all(|(actual, expected)| match (actual, expected) {
                (Value::Int(actual), ProjectionCell::Integer(expected)) => actual == expected,
                (Value::UInt(actual), ProjectionCell::Integer(expected)) => {
                    i64::try_from(actual).ok() == Some(expected)
                }
                (Value::Bytes(actual), ProjectionCell::Text(expected)) => {
                    actual == expected.into_bytes()
                }
                _ => false,
            })
}

async fn resource_known(
    tx: &mut Transaction<'_>,
    sequence: i64,
    resource: &ResourceRefV1,
) -> Result<bool> {
    let path = match resource.kind {
        ResourceKindV1::Session => "$.session_id",
        ResourceKindV1::Agent => "$.agent_id",
        ResourceKindV1::Sandbox => "$.sandbox_id",
        ResourceKindV1::Project => "$.project_id",
    };
    let row = tx
        .exec_first::<Row, _, _>(
            "SELECT EXISTS(SELECT 1 FROM ai_guard_audit_event WHERE seq < ?
             AND BINARY JSON_UNQUOTE(JSON_EXTRACT(CAST(payload AS CHAR CHARACTER SET utf8mb4), '$.kind'))
               NOT LIKE 'control.identity.%'
             AND JSON_TYPE(JSON_EXTRACT(CAST(payload AS CHAR CHARACTER SET utf8mb4), ?))='STRING'
             AND BINARY JSON_UNQUOTE(JSON_EXTRACT(CAST(payload AS CHAR CHARACTER SET utf8mb4), ?))=BINARY ?)",
            (sequence, path, path, &resource.resource_id),
        )
        .await
        .map_err(driver_error)?
        .context("MySQL identity resource check is unavailable")?;
    let found: u8 = decode_row(row)?;
    Ok(found == 1)
}
