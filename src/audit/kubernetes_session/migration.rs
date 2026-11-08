use super::*;

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
    let marker: Genesis = serde_json::from_value(event.data.clone())?;
    crate::tenancy::validate_tenant_id(&marker.tenant_id)?;
    uuid(&marker.control_namespace_uuid)?;
    uuid(&event.id)?;
    hash(&marker.previous_hash)?;
    require(
        marker.version == 2
            && marker.protocol == "kubernetes_session_reservation_v2"
            && marker.from_schema == 10
            && marker.previous_seq > 0
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
    )
}
pub(super) fn genesis(db: &Connection, tenant: &str) -> anyhow::Result<(i64, String)> {
    let mut query = db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind=?1 ORDER BY seq LIMIT 2")?;
    let rows = query
        .query_map([GENESIS_KIND], row_event)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    conflict(rows.len() == 1)?;
    let stored = &rows[0];
    marker(&stored.event)?;
    let marker: Genesis = serde_json::from_value(stored.event.data.clone())?;
    let namespace: String = db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    conflict(
        marker.tenant_id == tenant
            && marker.control_namespace_uuid == namespace
            && marker.previous_seq.checked_add(1) == Some(stored.seq)
            && marker.previous_hash == stored.prev_hash
            && parse_time(&stored.event.timestamp)? <= parse_time(&stored.received_at)?,
    )?;
    let meta: (i64, String) = db.query_row("SELECT genesis_seq,genesis_hash FROM kubernetes_session_reservation_meta WHERE singleton=1", [],
        |row| Ok((row.get(0)?, row.get(1)?)))?;
    conflict(meta == (stored.seq, stored.hash.clone()))?;
    let worker_marker: i64 = db.query_row(
        "SELECT seq FROM events WHERE kind='control.identity.workers_access.genesis'",
        [],
        |row| row.get(0),
    )?;
    conflict(worker_marker < stored.seq)?;
    Ok(meta)
}
pub(super) fn initialize(store: &Store, tx: &Transaction<'_>) -> anyhow::Result<()> {
    let schema: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    require(schema == 10)?;
    for (_, _, sql) in SCHEMA {
        tx.execute_batch(sql)?;
    }
    let (previous_seq, previous_hash) = audit_head(tx)?;
    let namespace = tx.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let mut event = Event::new(GENESIS_KIND);
    event.data = serde_json::to_value(Genesis {
        version: 2,
        protocol: "kubernetes_session_reservation_v2".into(),
        tenant_id: store.tenant_id.clone(),
        control_namespace_uuid: namespace,
        from_schema: 10,
        previous_seq,
        previous_hash,
    })?;
    marker(&event)?;
    let original = serde_json::to_value(&event)?;
    let (event, payload) = store.prepare_event(event)?;
    permission(serde_json::to_value(&event)? == original)?;
    // Only this exact maintenance marker uses the existing control reserve.
    let stored = store.append_in_transaction(tx, event, payload)?;
    tx.execute("INSERT INTO kubernetes_session_reservation_meta(singleton,genesis_seq,genesis_hash) VALUES(1,?1,?2)", params![stored.seq, stored.hash])?;
    Ok(())
}
