//! Semantic format maintenance; no new tables, activation or emission authority.
use super::*;
use crate::broker_registry::{format_time, parse_time};

pub(super) const POLICY_GENESIS_KIND: &str = "control.kubernetes.create.policy.genesis";
const POLICY_PROTOCOL: &str = "kubernetes_held_create_policy_v1";
pub(super) const RECOVERY_GENESIS_KIND: &str = "control.kubernetes.create.recovery.genesis";
const RECOVERY_PROTOCOL: &str = "kubernetes_create_recovery_v3";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryGenesis {
    version: u32,
    protocol: String,
    from_schema: u32,
    tenant_id: String,
    control_namespace_uuid: String,
    create_genesis_seq: i64,
    create_genesis_hash: String,
    policy_genesis_seq: i64,
    policy_genesis_hash: String,
    previous_seq: i64,
    previous_hash: String,
}

/// Isolated descriptive validation for global scanners without a DB handle.
pub(super) fn recovery_event(event: &Event) -> anyhow::Result<()> {
    let marker: RecoveryGenesis = serde_json::from_value(event.data.clone())?;
    crate::tenancy::validate_tenant_id(&marker.tenant_id)?;
    crate::control::uuid(&marker.control_namespace_uuid)?;
    crate::control::uuid(&event.id)?;
    anyhow::ensure!(
        !uuid::Uuid::parse_str(&marker.control_namespace_uuid)?.is_nil()
            && !uuid::Uuid::parse_str(&event.id)?.is_nil(),
        "nil held Kubernetes recovery marker identity"
    );
    for digest in [
        &marker.create_genesis_hash,
        &marker.policy_genesis_hash,
        &marker.previous_hash,
    ] {
        crate::control::hash(digest)?;
    }
    anyhow::ensure!(
        marker.version == 13
            && marker.from_schema == 12
            && marker.protocol == RECOVERY_PROTOCOL
            && marker.create_genesis_seq > 0
            && marker.policy_genesis_seq > marker.create_genesis_seq
            && marker.previous_seq >= marker.policy_genesis_seq
            && event.kind == RECOVERY_GENESIS_KIND
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
        "invalid held Kubernetes recovery format marker"
    );
    Ok(())
}

fn unique_marker(db: &Connection, kind: &str) -> anyhow::Result<StoredEvent> {
    let mut query = db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind=?1 ORDER BY seq LIMIT 2")?;
    let entries = query
        .query_map([kind], row_event)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    anyhow::ensure!(entries.len() == 1, "held Kubernetes marker must be unique");
    entries
        .into_iter()
        .next()
        .ok_or_else(|| KubernetesCreateFailureV1::Unavailable.into())
}

pub(super) fn validate_recovery_marker(db: &Connection) -> anyhow::Result<()> {
    let schema: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let count: i64 = db.query_row(
        "SELECT COUNT(*) FROM events WHERE kind=?1",
        [RECOVERY_GENESIS_KIND],
        |row| row.get(0),
    )?;
    if schema < 13 {
        anyhow::ensure!(
            count == 0,
            "held Kubernetes recovery marker requires schema13"
        );
        return Ok(());
    }
    anyhow::ensure!(
        matches!(schema, 13 | 14 | 15) && count == 1,
        "invalid schema13 recovery marker count"
    );
    let stored = unique_marker(db, RECOVERY_GENESIS_KIND)?;
    recovery_event(&stored.event)?;
    let marker: RecoveryGenesis = serde_json::from_value(stored.event.data.clone())?;
    let create = unique_marker(db, GENESIS_KIND)?;
    let policy = unique_marker(db, POLICY_GENESIS_KIND)?;
    let checkpoint = unique_marker(db, "control.broker_checkpoints.genesis")?;
    super::super::broker_checkpoint_migration::marker(&checkpoint.event)?;
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
    validate_marker(&policy, &tenant, &namespace, create.seq, &create.hash)?;
    let previous_hash: String = db.query_row(
        "SELECT hash FROM events WHERE seq=?1",
        [marker.previous_seq],
        |row| row.get(0),
    )?;
    anyhow::ensure!(
        marker.tenant_id == tenant
            && marker.control_namespace_uuid == namespace
            && marker.create_genesis_seq == create.seq
            && marker.create_genesis_hash == create.hash
            && marker.policy_genesis_seq == policy.seq
            && marker.policy_genesis_hash == policy.hash
            && checkpoint.seq > policy.seq
            && checkpoint.seq < stored.seq
            && marker.previous_seq.checked_add(1) == Some(stored.seq)
            && marker.previous_hash == stored.prev_hash
            && marker.previous_hash == previous_hash
            && parse_time(&stored.event.timestamp)? <= parse_time(&stored.received_at)?,
        "held Kubernetes recovery marker differs from its durable prefix"
    );
    Ok(())
}

pub(super) fn initialize_recovery(store: &Store, tx: &Transaction<'_>) -> anyhow::Result<()> {
    let schema: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    anyhow::ensure!(
        schema == 12,
        "held Kubernetes recovery migration requires exact schema12"
    );
    validate_recovery_marker(tx)?;
    let create = unique_marker(tx, GENESIS_KIND)?;
    let policy = unique_marker(tx, POLICY_GENESIS_KIND)?;
    let (previous_seq, previous_hash) = audit_head(tx)?;
    let namespace: String = tx.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let mut event = Event::new(RECOVERY_GENESIS_KIND);
    event.data = serde_json::to_value(RecoveryGenesis {
        version: 13,
        protocol: RECOVERY_PROTOCOL.into(),
        from_schema: 12,
        tenant_id: store.tenant_id.clone(),
        control_namespace_uuid: namespace,
        create_genesis_seq: create.seq,
        create_genesis_hash: create.hash,
        policy_genesis_seq: policy.seq,
        policy_genesis_hash: policy.hash,
        previous_seq,
        previous_hash,
    })?;
    recovery_event(&event)?;
    let original = serde_json::to_value(&event)?;
    let (event, payload) = store.prepare_event(event)?;
    anyhow::ensure!(
        serde_json::to_value(&event)? == original,
        "held Kubernetes recovery marker changed during preparation"
    );
    // Ordinary backed-up maintenance, not a protected identity/broker reserve.
    store.append_in_transaction(tx, event, payload)?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PolicyGenesis {
    version: u32,
    protocol: String,
    tenant_id: String,
    control_namespace_uuid: String,
    create_genesis_seq: i64,
    create_genesis_hash: String,
    previous_seq: i64,
    previous_hash: String,
}

pub(super) fn policy_event(event: &Event) -> anyhow::Result<PolicyGenesis> {
    let marker: PolicyGenesis = serde_json::from_value(event.data.clone())?;
    crate::tenancy::validate_tenant_id(&marker.tenant_id)?;
    crate::control::uuid(&marker.control_namespace_uuid)?;
    crate::control::uuid(&event.id)?;
    crate::control::hash(&marker.create_genesis_hash)?;
    crate::control::hash(&marker.previous_hash)?;
    anyhow::ensure!(
        marker.version == 1
            && marker.protocol == POLICY_PROTOCOL
            && marker.create_genesis_seq > 0
            && marker.previous_seq >= marker.create_genesis_seq
            && event.kind == POLICY_GENESIS_KIND
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
        "invalid held Kubernetes policy format marker"
    );
    Ok(marker)
}

pub(super) fn validate_marker(
    stored: &StoredEvent,
    tenant: &str,
    namespace: &str,
    create_genesis_seq: i64,
    create_genesis_hash: &str,
) -> anyhow::Result<()> {
    let marker = policy_event(&stored.event)?;
    anyhow::ensure!(
        marker.tenant_id == tenant
            && marker.control_namespace_uuid == namespace
            && marker.create_genesis_seq == create_genesis_seq
            && marker.create_genesis_hash == create_genesis_hash
            && marker.previous_seq.checked_add(1) == Some(stored.seq)
            && marker.previous_hash == stored.prev_hash
            && parse_time(&stored.event.timestamp)? <= parse_time(&stored.received_at)?,
        "held Kubernetes policy marker differs from its durable prefix"
    );
    Ok(())
}

pub(super) fn initialize(store: &Store, tx: &Transaction<'_>) -> anyhow::Result<()> {
    let (create_genesis_seq, create_genesis_hash): (i64, String) = tx.query_row(
        "SELECT seq,hash FROM events WHERE kind=?1",
        [GENESIS_KIND],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
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
    let mut event = Event::new(POLICY_GENESIS_KIND);
    event.data = serde_json::to_value(PolicyGenesis {
        version: 1,
        protocol: POLICY_PROTOCOL.into(),
        tenant_id: store.tenant_id.clone(),
        control_namespace_uuid: namespace,
        create_genesis_seq,
        create_genesis_hash,
        previous_seq,
        previous_hash,
    })?;
    policy_event(&event)?;
    let original = serde_json::to_value(&event)?;
    let (event, payload) = store.prepare_event(event)?;
    anyhow::ensure!(
        serde_json::to_value(&event)? == original,
        "held Kubernetes policy marker changed during preparation"
    );
    // Maintenance is charged to ordinary capacity and the existing time floor.
    store.append_in_transaction(tx, event, payload)?;
    Ok(())
}
