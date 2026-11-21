//! Verify operator-prepared workload bindings within the audit snapshot.
use anyhow::{Context, Result, anyhow, ensure};
use chrono::{DateTime, Utc};
use mysql_async::{Row, Transaction, Value, prelude::Queryable};

use crate::{
    audit::workload::projection::{
        Change, Genesis, Projection, ProjectionCell, Record, canonical_control_time, worker_matches,
    },
    broker_registry::{format_time, parse_time},
    model::{Event, StoredEvent, event_kind_in_namespace},
    relay::RelayEndpointV1,
    workload::{
        WorkloadBinding, WorkloadBindingIssueV1, WorkloadBindingReceiptV1,
        WorkloadBindingRevocationV1, WorkloadDigestOwnerV1, WorkloadFailureV1, workload_authorized,
        workload_conflict,
    },
};

use super::{
    audit::{decode_event, decode_row},
    driver_error,
    worker::snapshot_before_verified,
};

const GENESIS: &str = "control.workload.genesis";

const QUERIES: [(&str, &str); 6] = [
    (
        "workload_meta",
        "SELECT singleton,genesis_seq,genesis_hash FROM ai_guard_workload_meta ORDER BY singleton",
    ),
    (
        "workload_bindings",
        "SELECT credential_id,revision,sha256_digest,subject_key,payload_json,event_seq FROM ai_guard_workload_bindings ORDER BY credential_id",
    ),
    (
        "workload_binding_history",
        "SELECT credential_id,revision,payload_json,event_seq FROM ai_guard_workload_binding_history ORDER BY credential_id,revision",
    ),
    (
        "workload_digests",
        "SELECT sha256_digest,credential_id FROM ai_guard_workload_digests ORDER BY sha256_digest",
    ),
    (
        "workload_operations",
        "SELECT operation_id,input_hash,event_seq,credential_id,revision FROM ai_guard_workload_operations ORDER BY operation_id",
    ),
    (
        "workload_worker_endpoints",
        "SELECT worker_id,attempt_id,generation,event_seq FROM ai_guard_workload_worker_endpoints ORDER BY worker_id",
    ),
];

pub(super) fn is_event(kind: &str) -> bool {
    event_kind_in_namespace(kind, "control.workload")
}

async fn schema_counts(tx: &mut Transaction<'_>) -> Result<(u64, u64)> {
    let row = tx
        .query_first::<Row, _>(
            "SELECT COUNT(*), COUNT(CASE WHEN TABLE_TYPE='BASE TABLE' AND ENGINE='InnoDB' THEN 1 END)
             FROM information_schema.TABLES WHERE BINARY TABLE_SCHEMA=BINARY DATABASE()
             AND BINARY TABLE_NAME IN ('ai_guard_workload_meta','ai_guard_workload_bindings',
              'ai_guard_workload_binding_history','ai_guard_workload_digests',
              'ai_guard_workload_operations','ai_guard_workload_worker_endpoints')",
        )
        .await
        .map_err(driver_error)?
        .context("MySQL workload schema inventory unavailable")?;
    decode_row(row)
}

pub(super) struct Replay {
    genesis_seq: i64,
    genesis_hash: String,
    seen_genesis: bool,
    records: Vec<(i64, Record)>,
}

/// Binding history is available only from the validated projection.
pub(super) struct Verified {
    projection: Projection,
    time_floor: DateTime<Utc>,
}

impl Verified {
    pub(super) fn has_historical_digest(&self, digest: &[u8; 32]) -> bool {
        self.projection.has_historical_digest(digest)
    }

    pub(super) fn historical_digest_owner(
        &self,
        digest: &[u8; 32],
    ) -> Result<Option<WorkloadDigestOwnerV1>> {
        self.projection.historical_digest_owner(digest)
    }

    pub(super) fn capture_control_time(&self) -> Result<ControlClock> {
        let clock = ControlClock {
            captured: Utc::now(),
            floor: self.time_floor,
        };
        clock.check(clock.captured)?;
        Ok(clock)
    }

    pub(super) async fn replay(
        &self,
        tx: &mut Transaction<'_>,
        change: &Change,
    ) -> Result<Option<StoredEvent>> {
        let Some(seq) = self.projection.replay_sequence(change)? else {
            return Ok(None);
        };
        let row = tx
            .exec_first::<Row, _, _>(
                "SELECT seq,event_id,received_at,prev_hash,hash,payload FROM ai_guard_audit_event WHERE seq=?",
                (seq,),
            )
            .await
            .map_err(driver_error)?
            .context("MySQL workload operation event is missing")?;
        let stored = decode_event(row)?.stored;
        workload_conflict(stored.event.kind == change.kind())?;
        Ok(Some(stored))
    }

    pub(super) fn plan_revocation(
        mut self,
        clock: &ControlClock,
        input: &WorkloadBindingRevocationV1,
        sequence: i64,
    ) -> Result<(Event, Revocation)> {
        let change = Change::Revoke {
            input: input.clone(),
        };
        let (event, binding, input_hash) = self.plan_binding(clock, change, sequence)?;
        Ok((
            event,
            Revocation {
                input: input.clone(),
                input_hash,
                binding,
                sequence,
            },
        ))
    }

    pub(super) fn plan_issue(
        mut self,
        clock: &ControlClock,
        input: &WorkloadBindingIssueV1,
        sequence: i64,
    ) -> Result<(Event, Issuance)> {
        let change = Change::Issue {
            input: input.clone(),
        };
        let (new_digest, new_endpoint) = self
            .projection
            .issue_index_insertions(&input.sha256_digest, change.worker_issue_endpoint());
        let (event, binding, input_hash) = self.plan_binding(clock, change, sequence)?;
        Ok((
            event,
            Issuance {
                operation_id: input.operation_id.clone(),
                expected_revision: input.expected_revision,
                input_hash,
                binding,
                sequence,
                new_digest,
                new_endpoint,
            },
        ))
    }

    fn plan_binding(
        &mut self,
        clock: &ControlClock,
        change: Change,
        sequence: i64,
    ) -> Result<(Event, WorkloadBinding, String)> {
        let record = Record {
            version: 1,
            input_hash: change.input_hash()?,
            received_at: format_time(clock.captured),
            change,
        };
        let binding = self.projection.apply(&record, sequence)?;
        let mut event = Event::new(record.change.kind());
        event.timestamp.clone_from(&record.received_at);
        event.data = serde_json::to_value(&record)?;
        Ok((event, binding, record.input_hash))
    }
}

pub(super) async fn validate_issue_worker(
    tx: &mut Transaction<'_>,
    change: &Change,
    sequence: i64,
) -> Result<()> {
    if let Some(endpoint) = change.worker_issue_endpoint() {
        let worker = snapshot_before_verified(tx, &endpoint.worker_id, sequence).await?;
        workload_authorized(
            worker
                .as_ref()
                .is_some_and(|worker| worker_matches(worker, &endpoint)),
        )?;
    }
    Ok(())
}

/// Wall-clock authority established under the locked, verified audit head.
pub(super) struct ControlClock {
    captured: DateTime<Utc>,
    floor: DateTime<Utc>,
}

impl ControlClock {
    fn check(&self, raw: DateTime<Utc>) -> Result<DateTime<Utc>> {
        let at = canonical_control_time(raw)?;
        ensure!(
            raw >= self.captured && at >= self.floor,
            WorkloadFailureV1::ClockRegression
        );
        Ok(at)
    }

    pub(super) fn append_time(&mut self) -> Result<DateTime<Utc>> {
        let at = self.check(Utc::now())?;
        self.floor = at;
        Ok(at)
    }

    pub(super) fn check_commit_time(&self) -> Result<()> {
        self.check(Utc::now()).map(|_| ())
    }
}

pub(super) struct Revocation {
    input: WorkloadBindingRevocationV1,
    input_hash: String,
    binding: WorkloadBinding,
    sequence: i64,
}

impl Revocation {
    pub(super) async fn persist(&self, tx: &mut Transaction<'_>, seq: i64) -> Result<()> {
        ensure!(
            seq == self.sequence,
            "MySQL workload revocation sequence changed"
        );
        let revision = i64::try_from(self.binding.revision)?;
        let previous_revision = i64::try_from(self.input.expected_revision)?;
        let payload = serde_json::to_vec(&self.binding)?;
        tx.exec_drop(
            "UPDATE ai_guard_workload_bindings SET revision=?,payload_json=?,event_seq=?
             WHERE credential_id=? AND revision=?",
            (
                revision,
                &payload,
                seq,
                &self.input.credential_id,
                previous_revision,
            ),
        )
        .await
        .map_err(driver_error)?;
        workload_conflict(tx.affected_rows() == 1)?;
        persist_history(tx, &self.binding.credential_id, revision, &payload, seq).await?;
        persist_operation(
            tx,
            &self.input.operation_id,
            &self.input_hash,
            &self.binding.credential_id,
            revision,
            seq,
        )
        .await?;
        Ok(())
    }
}

pub(super) struct Issuance {
    operation_id: String,
    expected_revision: u64,
    input_hash: String,
    binding: WorkloadBinding,
    sequence: i64,
    new_digest: bool,
    new_endpoint: Option<RelayEndpointV1>,
}

impl Issuance {
    pub(super) async fn persist(&self, tx: &mut Transaction<'_>, seq: i64) -> Result<()> {
        ensure!(
            seq == self.sequence,
            "MySQL workload issue sequence changed"
        );
        let binding = &self.binding;
        let revision = i64::try_from(binding.revision)?;
        let previous_revision = i64::try_from(self.expected_revision)?;
        let payload = serde_json::to_vec(binding)?;
        if previous_revision == 0 {
            tx.exec_drop(
                "INSERT INTO ai_guard_workload_bindings
                 (credential_id,revision,sha256_digest,subject_key,payload_json,event_seq)
                 VALUES (?,?,?,?,?,?)",
                (
                    &binding.credential_id,
                    revision,
                    &binding.sha256_digest,
                    binding.subject.key()?,
                    &payload,
                    seq,
                ),
            )
            .await
            .map_err(driver_error)?;
        } else {
            tx.exec_drop(
                "UPDATE ai_guard_workload_bindings SET revision=?,sha256_digest=?,payload_json=?,event_seq=?
                 WHERE credential_id=? AND revision=?",
                (revision, &binding.sha256_digest, &payload, seq, &binding.credential_id, previous_revision),
            )
            .await
            .map_err(driver_error)?;
        }
        workload_conflict(tx.affected_rows() == 1)?;
        persist_history(tx, &binding.credential_id, revision, &payload, seq).await?;
        if self.new_digest {
            tx.exec_drop(
                "INSERT INTO ai_guard_workload_digests (sha256_digest,credential_id) VALUES (?,?)",
                (&binding.sha256_digest, &binding.credential_id),
            )
            .await
            .map_err(driver_error)?;
        }
        if let Some(endpoint) = &self.new_endpoint {
            tx.exec_drop(
                "INSERT INTO ai_guard_workload_worker_endpoints
                 (worker_id,attempt_id,generation,event_seq) VALUES (?,?,?,?)",
                (
                    &endpoint.worker_id,
                    &endpoint.attempt_id,
                    i64::try_from(endpoint.generation)?,
                    seq,
                ),
            )
            .await
            .map_err(driver_error)?;
        }
        persist_operation(
            tx,
            &self.operation_id,
            &self.input_hash,
            &binding.credential_id,
            revision,
            seq,
        )
        .await?;
        Ok(())
    }
}

async fn persist_history(
    tx: &mut Transaction<'_>,
    credential_id: &str,
    revision: i64,
    payload: &[u8],
    seq: i64,
) -> Result<()> {
    tx.exec_drop(
        "INSERT INTO ai_guard_workload_binding_history
         (credential_id,revision,payload_json,event_seq) VALUES (?,?,?,?)",
        (credential_id, revision, payload, seq),
    )
    .await
    .map_err(driver_error)?;
    Ok(())
}

async fn persist_operation(
    tx: &mut Transaction<'_>,
    operation_id: &str,
    input_hash: &str,
    credential_id: &str,
    revision: i64,
    seq: i64,
) -> Result<()> {
    tx.exec_drop(
        "INSERT INTO ai_guard_workload_operations
         (operation_id,input_hash,event_seq,credential_id,revision) VALUES (?,?,?,?,?)",
        (operation_id, input_hash, seq, credential_id, revision),
    )
    .await
    .map_err(driver_error)?;
    Ok(())
}

pub(super) fn binding_receipt(
    stored: &StoredEvent,
    replayed: bool,
) -> Result<WorkloadBindingReceiptV1> {
    let record: Record = serde_json::from_value(stored.event.data.clone())?;
    let (credential_id, previous_revision) = match record.change {
        Change::Issue { input } => (input.credential_id, input.expected_revision),
        Change::Revoke { input } => (input.credential_id, input.expected_revision),
    };
    let revision = crate::broker_registry::next_revision(previous_revision)?;
    Ok(crate::audit::workload::receipt(
        stored,
        credential_id,
        revision,
        replayed,
    ))
}

impl Replay {
    pub(super) async fn open(tx: &mut Transaction<'_>) -> Result<Option<Self>> {
        let counts = schema_counts(tx).await?;
        if counts.0 == 0 {
            return Ok(None);
        }
        let expected = QUERIES.len() as u64;
        ensure!(
            counts == (expected, expected),
            "MySQL workload requires all prepared InnoDB tables"
        );
        // Hold the projection tables' metadata locks for the verified snapshot.
        for (_, query) in QUERIES {
            tx.query_drop(format!("{query} LIMIT 0"))
                .await
                .map_err(driver_error)?;
        }
        ensure!(
            schema_counts(tx).await? == counts,
            "MySQL workload schema changed during verification"
        );
        let mut markers: Vec<Row> = tx
            .query("SELECT singleton,genesis_seq,genesis_hash FROM ai_guard_workload_meta LIMIT 2")
            .await
            .map_err(driver_error)?;
        ensure!(
            markers.len() == 1,
            "MySQL workload requires one prepared capability marker"
        );
        let row = markers
            .pop()
            .context("MySQL workload capability marker is missing")?;
        let (singleton, genesis_seq, genesis_hash): (i64, i64, String) = decode_row(row)?;
        ensure!(
            singleton == 1 && genesis_seq > 1 && crate::control::hash(&genesis_hash).is_ok(),
            "invalid MySQL workload capability marker"
        );
        Ok(Some(Self {
            genesis_seq,
            genesis_hash,
            seen_genesis: false,
            records: Vec::new(),
        }))
    }

    pub(super) fn observe(
        &mut self,
        stored: &StoredEvent,
        tenant: &str,
        namespace: &str,
        prior_max_received_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        let event = &stored.event;
        ensure!(
            event.source == "gateway"
                && event.provenance == "observed"
                && event.coverage == "observed",
            "invalid MySQL workload event attribution"
        );
        if event.kind == GENESIS {
            let genesis: Genesis = serde_json::from_value(event.data.clone())
                .map_err(|_| anyhow!("invalid MySQL workload genesis payload"))?;
            ensure!(
                !self.seen_genesis
                    && stored.seq == self.genesis_seq
                    && stored.hash == self.genesis_hash
                    && genesis.matches_prefix(tenant, namespace, stored.seq, &stored.prev_hash),
                "MySQL workload genesis does not match the prepared capability"
            );
            self.seen_genesis = true;
            return Ok(());
        }

        ensure!(
            self.seen_genesis,
            "MySQL workload mutation precedes its capability"
        );
        let record: Record = serde_json::from_value(event.data.clone())
            .map_err(|_| anyhow!("invalid MySQL workload mutation payload"))?;
        ensure!(
            event.kind == record.change.kind(),
            WorkloadFailureV1::InvalidRequest
        );
        let control_time = parse_time(&record.received_at)?;
        ensure!(
            control_time <= parse_time(&stored.received_at)?,
            WorkloadFailureV1::InvalidRequest
        );
        let floor = prior_max_received_at.context("MySQL workload audit time prefix is missing")?;
        ensure!(
            canonical_control_time(control_time)? >= floor,
            WorkloadFailureV1::ClockRegression
        );
        self.records.push((stored.seq, record));
        Ok(())
    }

    /// The audit stream is drained and the worker projections have been
    /// verified in this transaction before querying historical worker state.
    pub(super) async fn verify(
        self,
        tx: &mut Transaction<'_>,
        time_floor: DateTime<Utc>,
    ) -> Result<Verified> {
        ensure!(self.seen_genesis, "MySQL workload genesis is missing");
        let mut projection = Projection::default();
        for (sequence, record) in self.records {
            validate_issue_worker(tx, &record.change, sequence).await?;
            projection.apply(&record, sequence)?;
        }

        let mut rows = projection.rows(self.genesis_seq, &self.genesis_hash)?;
        for (table, query) in QUERIES {
            let expected = rows.remove(table).context("missing workload projection")?;
            // Use the binary protocol for typed integers and exact bytes.
            let mut records = tx.exec_iter(query, ()).await.map_err(driver_error)?;
            for expected_row in expected {
                let actual = records.next().await.map_err(driver_error)?;
                ensure!(
                    actual.is_some_and(|row| row_matches(row, expected_row)),
                    "MySQL workload projection differs from audit: {table}"
                );
            }
            ensure!(
                records.next().await.map_err(driver_error)?.is_none(),
                "MySQL workload projection contains unexpected rows: {table}"
            );
            records.drop_result().await.map_err(driver_error)?;
        }
        Ok(Verified {
            projection,
            time_floor,
        })
    }
}

fn row_matches(row: Row, expected: Vec<ProjectionCell>) -> bool {
    row.len() == expected.len()
        && expected.into_iter().enumerate().all(|(index, expected)| {
            match (row.as_ref(index), expected) {
                (Some(Value::Int(actual)), ProjectionCell::Integer(expected)) => {
                    *actual == expected
                }
                (Some(Value::UInt(actual)), ProjectionCell::Integer(expected)) => {
                    i64::try_from(*actual).ok() == Some(expected)
                }
                (Some(Value::Bytes(actual)), ProjectionCell::Text(expected)) => {
                    actual.as_slice() == expected.as_bytes()
                }
                _ => false,
            }
        })
}
