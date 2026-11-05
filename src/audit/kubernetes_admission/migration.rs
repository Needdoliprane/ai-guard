//! Semantic format maintenance; no new tables, activation or emission authority.
use super::*;
use crate::broker_registry::{format_time, parse_time};

pub(super) const POLICY_GENESIS_KIND: &str = "control.kubernetes.create.policy.genesis";
const POLICY_PROTOCOL: &str = "kubernetes_held_create_policy_v1";

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
