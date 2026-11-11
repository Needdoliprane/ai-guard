//! Explicit logical schema12 gate for the new broker checkpoint record kind.
//! Existing SQL projections and historical records keep their exact formats.
use super::*;
use crate::broker_registry::{format_time, parse_time};
use crate::control::{hash, uuid};

pub(super) const GENESIS_KIND: &str = "control.broker_checkpoints.genesis";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Genesis {
    version: u32,
    protocol: String,
    tenant_id: String,
    control_namespace_uuid: String,
    from_schema: u32,
    previous_seq: i64,
    previous_hash: String,
}

pub(super) fn marker(event: &Event) -> anyhow::Result<()> {
    let value: Genesis = serde_json::from_value(event.data.clone())?;
    validate_tenant_id(&value.tenant_id)?;
    uuid(&value.control_namespace_uuid)?;
    uuid(&event.id)?;
    hash(&value.previous_hash)?;
    anyhow::ensure!(
        value.version == 12
            && value.protocol == "broker_checkpoints_v1"
            && value.from_schema == 11
            && value.previous_seq > 0
            && event.kind == GENESIS_KIND
            && event.source == "gateway"
            && event.provenance == "observed"
            && event.coverage == "observed"
            && event.session_id.is_none()
            && event.project_id.is_none()
            && event.sandbox_id.is_none()
            && event.agent_id.is_none()
            && event.parent_agent_id.is_none()
            && event.correlation_id.is_none()
            && event.trace_id.is_none()
            && event.command.is_none()
            && event.decision.is_none()
            && event.result.is_none()
            && format_time(parse_time(&event.timestamp)?) == event.timestamp,
        "invalid broker checkpoint migration marker"
    );
    Ok(())
}

pub(super) fn validate(db: &Connection) -> anyhow::Result<()> {
    let schema: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let mut statement = db.prepare(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind=?1 ORDER BY seq LIMIT 2",
    )?;
    let rows = statement
        .query_map([GENESIS_KIND], row_event)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if schema < 12 {
        anyhow::ensure!(
            rows.is_empty(),
            "broker checkpoint marker requires schema12"
        );
        return Ok(());
    }
    anyhow::ensure!(
        matches!(schema, 12 | 13) && rows.len() == 1,
        "invalid schema12 genesis count"
    );
    let stored = &rows[0];
    marker(&stored.event)?;
    let value: Genesis = serde_json::from_value(stored.event.data.clone())?;
    let tenant: String = db.query_row(
        "SELECT tenant_id FROM tenant_binding WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let namespace: String = db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let prior_genesis: i64 = db.query_row(
        "SELECT genesis_seq FROM kubernetes_session_reservation_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    anyhow::ensure!(
        value.tenant_id == tenant
            && value.control_namespace_uuid == namespace
            && value.previous_seq.checked_add(1) == Some(stored.seq)
            && value.previous_hash == stored.prev_hash
            && prior_genesis < stored.seq
            && parse_time(&stored.event.timestamp)? <= parse_time(&stored.received_at)?,
        "broker checkpoint migration marker differs from retained control history"
    );
    Ok(())
}

pub(super) fn initialize(store: &Store, tx: &Transaction<'_>) -> anyhow::Result<()> {
    let schema: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    anyhow::ensure!(
        schema == 11,
        "broker checkpoint migration requires exact schema11"
    );
    validate(tx)?;
    let (previous_seq, previous_hash) = audit_head(tx)?;
    let namespace = tx.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let mut event = Event::new(GENESIS_KIND);
    event.data = serde_json::to_value(Genesis {
        version: 12,
        protocol: "broker_checkpoints_v1".into(),
        tenant_id: store.tenant_id.clone(),
        control_namespace_uuid: namespace,
        from_schema: 11,
        previous_seq,
        previous_hash,
    })?;
    marker(&event)?;
    let original = serde_json::to_value(&event)?;
    let (event, payload) = store.prepare_event(event)?;
    anyhow::ensure!(
        serde_json::to_value(&event)? == original,
        "migration marker was redacted"
    );
    // The existing backed-up maintenance transaction owns this exact marker;
    // this does not create a broker workload or use a worker audit identity.
    store.append_in_transaction(tx, event, payload)?;
    Ok(())
}

impl Store {
    /// Backed-up11→12 maintenance. The new record kind stays unavailable until
    /// this exact transaction and marker commit; no worker is started or freed.
    pub fn migrate_broker_checkpoints_v12(
        &self,
        backup: &Path,
    ) -> anyhow::Result<crate::control::MigrationReportV1> {
        self.migrate_control_version(backup, 11, 12)
    }
}
