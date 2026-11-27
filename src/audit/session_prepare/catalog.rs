//! Known predecessors of the complete Store14/15 session-prepare projection.
//! Their shared genesis is kept intact. Only in-place identity maintenance can
//! verify these catalogs, back them up, and append the extension receipt.
use super::*;

pub(in crate::audit) const UPGRADED_KIND: &str = "control.session_prepare.catalog_upgraded";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::audit) enum HistoricalCatalog {
    /// Installer at 9ba8b9b6.
    PrepareOnly,
    /// Installer at e3c9f82f.
    WithLaunches,
}

/// Select candidates only. The caller still matches the entire catalog,
/// including the exact DDL and every automatic index, and replays the audit.
pub(in crate::audit) fn historical_shape(
    mut has_table: impl FnMut(&str) -> bool,
) -> Option<HistoricalCatalog> {
    if session_message::SCHEMA
        .iter()
        .any(|(_, table, _)| has_table(table))
    {
        return None;
    }
    Some(if has_table(LAUNCHES_TABLE) {
        HistoricalCatalog::WithLaunches
    } else {
        HistoricalCatalog::PrepareOnly
    })
}

pub(in crate::audit) fn includes_prepare_table(
    historical: Option<HistoricalCatalog>,
    table: &str,
) -> bool {
    historical != Some(HistoricalCatalog::PrepareOnly) || table != LAUNCHES_TABLE
}

/// Control events are structural and remain live even after retention. A
/// missing projection with later effects in the chain is a loss of data.
pub(super) fn validate_historical_audit(
    db: &Connection,
    historical: HistoricalCatalog,
    before: Option<i64>,
) -> anyhow::Result<()> {
    let incompatible: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM events WHERE (?1 IS NULL OR seq < ?1) AND \
            (kind GLOB 'control.session_message.*' OR kind=?2 OR (?3 AND kind=?4)))",
        params![
            before,
            UPGRADED_KIND,
            historical == HistoricalCatalog::PrepareOnly,
            LAUNCHED_KIND,
        ],
        |row| row.get(0),
    )?;
    anyhow::ensure!(
        !incompatible,
        "session audit requires projections absent from its catalog"
    );
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Upgrade {
    version: u32,
    previous_catalog: HistoricalCatalog,
    prepare_genesis_seq: i64,
    prepare_genesis_hash: String,
    previous_seq: i64,
    previous_hash: String,
}

pub(in crate::audit) fn upgraded_from(
    db: &Connection,
) -> anyhow::Result<Option<HistoricalCatalog>> {
    let mut query = db.prepare(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind=?1 ORDER BY seq LIMIT 2",
    )?;
    let mut records = query.query_map([UPGRADED_KIND], row_event)?;
    let Some(stored) = records.next().transpose()? else {
        return Ok(None);
    };
    anyhow::ensure!(
        records.next().is_none(),
        "duplicate session catalog upgrade"
    );
    let original = genesis(db)?.context("session prepare genesis required by catalog upgrade")?;
    let upgrade: Upgrade = serde_json::from_value(stored.event.data)?;
    anyhow::ensure!(
        upgrade.version == 1
            && upgrade.prepare_genesis_seq == original.seq
            && upgrade.prepare_genesis_hash == original.hash
            && original.seq < stored.seq
            && upgrade.previous_seq == stored.seq - 1
            && upgrade.previous_hash == stored.prev_hash
            && stored.event.source == "gateway"
            && stored.event.provenance == "observed"
            && stored.event.coverage == "native"
            && stored.event.session_id.is_none(),
        "invalid session catalog upgrade"
    );
    validate_historical_audit(db, upgrade.previous_catalog, Some(stored.seq))?;
    Ok(Some(upgrade.previous_catalog))
}

pub(in crate::audit) fn upgrade(tx: &Transaction<'_>) -> anyhow::Result<Event> {
    let historical = validate_in_place_identity_schema(tx)?
        .context("session catalog upgrade requires a historical catalog")?;
    let original = genesis(tx)?.context("session prepare genesis required by catalog upgrade")?;
    let (previous_seq, previous_hash) = audit_head(tx)?;
    let mut event = Event::new(UPGRADED_KIND);
    event.coverage = "native".into();
    event.data = serde_json::to_value(Upgrade {
        version: 1,
        previous_catalog: historical,
        prepare_genesis_seq: original.seq,
        prepare_genesis_hash: original.hash,
        previous_seq,
        previous_hash,
    })?;
    if historical == HistoricalCatalog::PrepareOnly {
        tx.execute_batch(LAUNCHES_SCHEMA)?;
    }
    session_message::install_schema(tx)?;
    if historical == HistoricalCatalog::WithLaunches {
        project_historical_conversations(tx, original.seq)?;
    }
    Ok(event)
}

/// Every historical launch was committed with a prepared binding in its
/// audit event and receipt. Recover that projection with its original audit
/// reference. Delivery events are absent, so no messages can be recovered.
fn project_historical_conversations(tx: &Transaction<'_>, genesis_seq: i64) -> anyhow::Result<()> {
    let tenant: String = tx.query_row(
        "SELECT tenant_id FROM tenant_binding WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let namespace: String = tx.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let mut query = tx.prepare(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind=?1 ORDER BY seq",
    )?;
    let mut projected = 0_i64;
    for stored in query.query_map([LAUNCHED_KIND], row_event)? {
        let stored = stored?;
        validate_replay_event(tx, &stored)?;
        let launched: Launched = serde_json::from_value(stored.event.data)?;
        let receipt_json: String = tx.query_row(
            "SELECT receipt_json FROM session_launches WHERE operation_id=?1 AND input_hash=?2 \
             AND session_id=?3 AND preparation_operation_id=?4 AND event_seq=?5 AND event_hash=?6",
            params![
                launched.operation_id,
                launched.input_hash,
                launched.session_id,
                launched.preparation_operation_id,
                stored.seq,
                stored.hash,
            ],
            |row| row.get(0),
        )?;
        let receipt: SessionLaunchReceiptV1 = serde_json::from_str(&receipt_json)?;
        let binding = &launched.conversation;
        anyhow::ensure!(
            stored.seq > genesis_seq
                && receipt.version == 1
                && receipt.operation_id.to_string() == launched.operation_id
                && receipt.launch_id == stored.event.id
                && receipt.session_id == launched.session_id
                && receipt.preparation_operation_id.to_string()
                    == launched.preparation_operation_id
                && receipt.input_hash == launched.input_hash
                && receipt.audit_seq == stored.seq
                && receipt.audit_hash == stored.hash
                && !receipt.replayed
                && receipt.conversation == *binding
                && binding.tenant_id == tenant
                && binding.source_id == namespace
                && binding.launch_target_id.to_string() == launched.launch_target_id
                && binding.target_revision == launched.target_revision
                && receipt.launch_target_id == binding.launch_target_id
                && receipt.target_revision == binding.target_revision
                && binding.input_epoch == 0
                && binding.broker_id.is_none()
                && binding.broker_incarnation.is_none()
                && binding.worker_id.is_none()
                && binding.attempt_id.is_none()
                && binding.runtime_instance.is_none()
                && binding.provider.is_none()
                && binding.protocol.is_none(),
            "historical session launch does not match its audit record"
        );
        tx.execute(
            "INSERT INTO session_conversations(session_id,binding_json,event_seq,event_hash) VALUES (?1,?2,?3,?4)",
            params![
                binding.session_id,
                serde_json::to_string(binding)?,
                stored.seq,
                stored.hash,
            ],
        )?;
        projected += 1;
    }
    let launches: i64 = tx.query_row("SELECT COUNT(*) FROM session_launches", [], |row| {
        row.get(0)
    })?;
    anyhow::ensure!(
        projected == launches,
        "historical session launch catalog is incomplete"
    );
    Ok(())
}
