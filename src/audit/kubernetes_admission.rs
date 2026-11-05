//! Tenant held-CREATE projection and its typed, audited migration boundary.
use super::*;
use crate::control::kubernetes_admission::*;
mod authorization;
pub(crate) use authorization::KubernetesCreateAuthorization;
mod migration;
mod model;
mod mutations;
mod replay;
pub(crate) use mutations::{ConfirmedKubernetesCreateResult, FreshKubernetesCreateClaimCommit};

fn require_schema(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    // V1 mutation capabilities stay on schema8 until the V2 producer is wired.
    // Historical replay independently accepts schema9's format marker.
    if version == 8 {
        Ok(())
    } else {
        Err(KubernetesCreateFailureV1::MigrationRequired.into())
    }
}

pub(super) const SCHEMA: [(&str, &str, &str); 2] = [
    (
        "table",
        "kubernetes_creates",
        "CREATE TABLE kubernetes_creates(create_id TEXT PRIMARY KEY NOT NULL,session_id TEXT NOT NULL UNIQUE,intent_hash TEXT NOT NULL UNIQUE,state TEXT NOT NULL CHECK(state IN ('reserved','emission_possible','created_held','ownership_unknown','cancelled_before_emission')),create_epoch INTEGER NOT NULL CHECK(create_epoch>=1),revision INTEGER NOT NULL CHECK(revision>=1),quota_held INTEGER NOT NULL CHECK(quota_held IN (0,1)),cpu_millis INTEGER NOT NULL CHECK(cpu_millis>0),memory_bytes INTEGER NOT NULL CHECK(memory_bytes>0),snapshot_json TEXT NOT NULL,snapshot_hash TEXT NOT NULL,last_event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq))",
    ),
    (
        "index",
        "kubernetes_creates_held",
        "CREATE INDEX kubernetes_creates_held ON kubernetes_creates(quota_held)",
    ),
];
pub(super) const AUTO_INDEX_COUNTS: [(&str, u32); 1] = [("kubernetes_creates", 4)];
const GENESIS_KIND: &str = "control.kubernetes.create.genesis";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Genesis {
    version: u32,
    tenant_id: String,
    control_namespace_uuid: String,
    previous_seq: i64,
    previous_hash: String,
}

pub(super) fn initialize(store: &Store, tx: &Transaction<'_>) -> anyhow::Result<()> {
    for (_, _, sql) in SCHEMA {
        tx.execute_batch(sql)?;
    }
    let (previous_seq, previous_hash): (i64, String) = tx.query_row(
        "SELECT seq,hash FROM events ORDER BY seq DESC LIMIT 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let namespace: String = tx.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let mut event = Event::new(GENESIS_KIND);
    event.data = serde_json::to_value(Genesis {
        version: 1,
        tenant_id: store.tenant_id.clone(),
        control_namespace_uuid: namespace,
        previous_seq,
        previous_hash,
    })?;
    let original = serde_json::to_value(&event)?;
    let (event, payload) = store.prepare_event(event)?;
    anyhow::ensure!(
        serde_json::to_value(&event)? == original,
        "held Kubernetes genesis changed during preparation"
    );
    // Offline schema maintenance, not a broker mutation or emission claim.
    // Like the earlier typed genesis migrations it consumes ordinary capacity.
    store.append_in_transaction(tx, event, payload)?;
    Ok(())
}

pub(super) fn validate(db: &Connection) -> anyhow::Result<()> {
    replay::validate(db).map_err(|_| KubernetesCreateFailureV1::Unavailable.into())
}

pub(super) fn initialize_policy(store: &Store, tx: &Transaction<'_>) -> anyhow::Result<()> {
    migration::initialize(store, tx)
}

#[cfg(test)]
pub(super) fn validate_unwrapped_for_test(db: &Connection) -> anyhow::Result<()> {
    replay::validate(db)
}

pub(super) fn workload_audit_identity(
    event: &Event,
) -> anyhow::Result<Option<capacity::WorkloadAuditPrincipalV1>> {
    if event.kind == GENESIS_KIND || !event.kind.starts_with("control.kubernetes.create.") {
        return Ok(None);
    }
    if event.kind == migration::POLICY_GENESIS_KIND {
        migration::policy_event(event)?;
        return Ok(None);
    }
    let record: model::Record = serde_json::from_value(event.data.clone())?;
    record.change.validate()?;
    let is_result = matches!(record.change, model::Change::Result { .. });
    model::conflict(
        record.version == 1
            && record.input_hash == record.change.input_hash()?
            && event.id == record.change.operation_id()
            && event.kind == record.change.kind()
            && event.source == "gateway"
            && event.provenance == if is_result { "declared" } else { "observed" }
            && event.coverage == if is_result { "partial" } else { "observed" },
    )?;
    record.identity.identity()?;
    Ok(Some(capacity::WorkloadAuditPrincipalV1::Broker {
        broker_id: record.identity.broker_id,
    }))
}

impl Store {
    /// Explicit maintenance with a new, verified schema7 backup. This never
    /// manufactures an origin, broker authorization or runtime emission permit.
    pub fn migrate_kubernetes_create_v8(
        &self,
        backup_path: &Path,
    ) -> anyhow::Result<crate::control::MigrationReportV1> {
        self.migrate_control_version(backup_path, 7, 8)
    }

    /// Preserve a verified schema8 backup and append only the format marker.
    /// The semantic migration grants neither activation nor a mutation permit.
    pub fn migrate_kubernetes_policy_v9(
        &self,
        backup_path: &Path,
    ) -> anyhow::Result<crate::control::MigrationReportV1> {
        self.migrate_control_version(backup_path, 8, 9)
    }
}
