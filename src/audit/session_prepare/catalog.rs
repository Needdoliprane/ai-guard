//! Known predecessors of the complete Store14/15 session-prepare projection.
//! Their shared genesis is kept intact. Only in-place identity maintenance can
//! verify these catalogs, back them up, and append the extension receipt.
use super::*;
use crate::control::session_control::canonical_uuid;
use std::collections::BTreeMap;

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

fn projection_count(db: &Connection, table: &str) -> anyhow::Result<i64> {
    Ok(
        db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })?,
    )
}

/// Check both directions of the correspondence while the historical catalog
/// is still active. Stop after the expected row count plus one event, and
/// retain at most one revision per projected target.
pub(super) fn validate_historical_projections(
    db: &Connection,
    historical: HistoricalCatalog,
    genesis_seq: i64,
) -> anyhow::Result<()> {
    let preparations = projection_count(db, PREPARATIONS_TABLE)?;
    let operations = projection_count(db, TARGET_OPERATIONS_TABLE)?;
    let target_count = usize::try_from(projection_count(db, TARGETS_TABLE)?)?;
    let launches = match historical {
        HistoricalCatalog::PrepareOnly => 0,
        HistoricalCatalog::WithLaunches => {
            let (rows, preparations): (i64, i64) = db.query_row(
                "SELECT COUNT(*),COUNT(DISTINCT preparation_operation_id) FROM session_launches",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            anyhow::ensure!(
                rows == preparations,
                "historical session preparation was reused"
            );
            rows
        }
    };
    let limit = [preparations, operations, launches]
        .into_iter()
        .try_fold(1_i64, i64::checked_add)
        .context("historical session projection count exceeds the audit range")?;
    let (head, _) = audit_head(db)?;
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
    let mut query = db.prepare(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events
         WHERE kind IN (?1,?2,?3) AND seq<=?4 ORDER BY seq LIMIT ?5",
    )?;
    let mut targets = BTreeMap::<Uuid, (SessionLaunchTargetRecordV1, i64)>::new();
    let (mut preparations_seen, mut operations_seen, mut launches_seen) = (0_i64, 0_i64, 0_i64);
    for stored in query.query_map(
        params![
            PREPARED_KIND,
            TARGET_PROVISION_KIND,
            LAUNCHED_KIND,
            head,
            limit
        ],
        row_event,
    )? {
        let stored = stored?;
        anyhow::ensure!(
            stored.seq > genesis_seq,
            "session projection predates its genesis"
        );
        validate_replay_event(db, &stored)?;
        match stored.event.kind.as_str() {
            TARGET_PROVISION_KIND => {
                let command: ProvisionSessionLaunchTargetV1 =
                    serde_json::from_value(stored.event.data)?;
                let operation = target_operation(db, &command.operation_id.to_string())?
                    .context("historical session target operation is missing")?;
                let previous = targets
                    .get(&command.target.launch_target_id)
                    .map_or(0, |(target, _)| target.revision);
                anyhow::ensure!(
                    stored.event.id == provision_event_id(command.operation_id)
                        && operation.event_seq == stored.seq
                        && operation.event_hash == stored.hash
                        && operation.input_hash == command.input_hash()?
                        && previous == command.expected_revision,
                    "historical session target operation differs from its audit event"
                );
                targets.insert(
                    command.target.launch_target_id,
                    (command.target, stored.seq),
                );
                anyhow::ensure!(
                    targets.len() <= target_count,
                    "historical session target is missing"
                );
                operations_seen += 1;
            }
            PREPARED_KIND => {
                let prepared: Prepared = serde_json::from_value(stored.event.data.clone())?;
                let request = validate_historical_preparation(db, &stored, &prepared)?;
                let (target, _) = targets
                    .get(&request.launch_target_id)
                    .context("historical session preparation has no audited target")?;
                validate_historical_selection(
                    target,
                    &tenant,
                    &prepared.organization_scope,
                    prepared.target_revision,
                    &prepared.principal_key,
                )?;
                preparations_seen += 1;
            }
            LAUNCHED_KIND if historical == HistoricalCatalog::WithLaunches => {
                let launched: Launched = serde_json::from_value(stored.event.data.clone())?;
                let (target, _) = targets
                    .get(&canonical_uuid(&launched.launch_target_id)?)
                    .context("historical session launch has no audited target")?;
                validate_historical_selection(
                    target,
                    &tenant,
                    &launched.organization_scope,
                    launched.target_revision,
                    &launched.principal_key,
                )?;
                validate_historical_launch(db, &stored, &launched, &tenant, &namespace)?;
                launches_seen += 1;
            }
            _ => anyhow::bail!("unexpected historical session projection event"),
        }
    }
    anyhow::ensure!(
        (
            preparations_seen,
            operations_seen,
            launches_seen,
            targets.len()
        ) == (preparations, operations, launches, target_count),
        "historical session projections differ from their audit event counts"
    );
    for (target, seq) in targets.values() {
        let payload: String = db
            .query_row(
                "SELECT payload_json FROM session_launch_targets
                 WHERE launch_target_id=?1 AND revision=?2 AND tenant_id=?3
                 AND organization_scope=?4 AND active=?5 AND event_seq=?6",
                params![
                    target.launch_target_id.to_string(),
                    i64::try_from(target.revision)?,
                    target.tenant_id,
                    target.organization_scope,
                    target.active,
                    seq,
                ],
                |row| row.get(0),
            )
            .optional()?
            .context("historical session target differs from its latest audit event")?;
        anyhow::ensure!(
            serde_json::from_str::<SessionLaunchTargetRecordV1>(&payload)? == *target,
            "historical session target payload differs from its latest audit event"
        );
    }
    Ok(())
}

fn validate_historical_selection(
    target: &SessionLaunchTargetRecordV1,
    tenant: &str,
    organization: &str,
    revision: u64,
    principal_key: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        target.active
            && target.tenant_id == tenant
            && target.organization_scope == organization
            && target.revision == revision
            && target
                .launchers
                .iter()
                .any(|principal| principal.key().is_ok_and(|key| key == principal_key)),
        "historical session target was not applicable at the audited operation"
    );
    Ok(())
}

fn validate_historical_preparation(
    db: &Connection,
    stored: &StoredEvent,
    prepared: &Prepared,
) -> anyhow::Result<SessionLaunchPrepareRequestV1> {
    let request = SessionLaunchPrepareRequestV1 {
        version: prepared.version,
        operation_id: canonical_uuid(&prepared.operation_id)?,
        launch_target_id: canonical_uuid(&prepared.launch_target_id)?,
        expected_target_revision: prepared.target_revision,
        expires_at: prepared.expires_at.clone(),
    };
    request.validate()?;
    let row = preparation_row(db, &prepared.operation_id)?
        .context("historical session preparation projection is missing")?;
    anyhow::ensure!(
        stored.event.id == preparation_event_id(request.operation_id)
            && prepared.input_hash == request.input_hash()?
            && row.input_hash == prepared.input_hash
            && row.principal_key == prepared.principal_key
            && row.membership_id == prepared.membership_id
            && row.organization_scope == prepared.organization_scope
            && row.launch_target_id == prepared.launch_target_id
            && u64::try_from(row.target_revision)? == prepared.target_revision
            && row.expires_at == prepared.expires_at
            && row.event_seq == stored.seq
            && row.event_hash == stored.hash,
        "historical session preparation differs from its audit event"
    );
    Ok(request)
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
        project_historical_conversations(tx, previous_seq)?;
    }
    Ok(event)
}

fn validate_historical_launch(
    db: &Connection,
    stored: &StoredEvent,
    launched: &Launched,
    tenant: &str,
    namespace: &str,
) -> anyhow::Result<()> {
    let request = SessionLaunchRequestV1 {
        version: launched.version,
        operation_id: canonical_uuid(&launched.operation_id)?,
        launch_target_id: canonical_uuid(&launched.launch_target_id)?,
        expected_target_revision: launched.target_revision,
        preparation_operation_id: canonical_uuid(&launched.preparation_operation_id)?,
        preparation_input_hash: launched.preparation_input_hash.clone(),
    };
    request.validate()?;
    let preparation = preparation_row(db, &launched.preparation_operation_id)?
        .context("historical session launch preparation is missing")?;
    let preparation_event = stored_event(db, preparation.event_seq)?;
    let receipt_json: String = db
        .query_row(
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
        )
        .optional()?
        .context("historical session launch projection differs from its audit event")?;
    let receipt: SessionLaunchReceiptV1 = serde_json::from_str(&receipt_json)?;
    let binding = &launched.conversation;
    anyhow::ensure!(
        stored.event.id == bound_event_id(LAUNCH_EVENT_DOMAIN, request.operation_id)
            && launched.session_id == bound_event_id(LAUNCH_SESSION_DOMAIN, request.operation_id)
            && launched.grant_operation_id
                == bound_event_id(LAUNCH_GRANT_DOMAIN, request.operation_id)
            && launched.input_hash == request.input_hash()?
            && preparation.event_seq < stored.seq
            && preparation.event_hash == preparation_event.hash
            && preparation_event.event.kind == PREPARED_KIND
            && preparation.input_hash == launched.preparation_input_hash
            && preparation.principal_key == launched.principal_key
            && preparation.membership_id == launched.membership_id
            && preparation.organization_scope == launched.organization_scope
            && preparation.launch_target_id == launched.launch_target_id
            && u64::try_from(preparation.target_revision)? == launched.target_revision
            && receipt.version == 1
            && receipt.operation_id == request.operation_id
            && receipt.launch_id == stored.event.id
            && receipt.session_id == launched.session_id
            && receipt.preparation_operation_id == request.preparation_operation_id
            && receipt.preparation_id == preparation_event.event.id
            && receipt.input_hash == launched.input_hash
            && receipt.audit_seq == stored.seq
            && receipt.audit_hash == stored.hash
            && !receipt.replayed
            && receipt.conversation == *binding
            && binding.tenant_id == tenant
            && binding.source_id == namespace
            && binding.launch_target_id == request.launch_target_id
            && binding.target_revision == launched.target_revision
            && binding.expires_at == preparation.expires_at
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
    validate_historical_launch_grant(db, stored, launched, &receipt, tenant)
}

fn validate_historical_launch_grant(
    db: &Connection,
    launch: &StoredEvent,
    launched: &Launched,
    receipt: &SessionLaunchReceiptV1,
    tenant: &str,
) -> anyhow::Result<()> {
    let stored = stored_event(db, receipt.grant_audit_seq)?;
    anyhow::ensure!(
        launch.seq.checked_add(1) == Some(stored.seq)
            && stored.prev_hash == launch.hash
            && stored.hash == receipt.grant_audit_hash
            && stored.event.kind == "control.identity.mutation"
            && stored.event.source == "gateway"
            && stored.event.provenance == "observed"
            && stored.event.coverage == "observed",
        "historical session launch grant does not match its audit receipt"
    );
    let mutation = identity::projection::Record::decode_replay(&stored.event.data)?.mutation;
    mutation.validate()?;
    let IdentityChangeV1::ResourceGrant {
        expected_revision,
        grant,
    } = mutation.change
    else {
        anyhow::bail!("historical session launch requires an audited resource grant");
    };
    anyhow::ensure!(
        expected_revision == 0
            && mutation.operation_id == launched.grant_operation_id
            && mutation.actor
                == (PrincipalRefV1::LegacyControl {
                    credential_id: LAUNCH_GRANT_ACTOR.into(),
                    role: LegacyControlRoleV1::Runner,
                })
            && grant.revision == 1
            && grant.revision == receipt.grant_revision
            && grant.resource.kind == ResourceKindV1::Session
            && grant.resource.resource_id == launched.session_id
            && grant.tenant_id == tenant
            && grant.organization_scope == launched.organization_scope
            && grant.owner.key()? == launched.principal_key
            && grant.active
            && grant.shares.is_empty(),
        "historical session launch grant differs from the audited initial grant"
    );
    Ok(())
}

/// The held transaction has verified these rows before adding any table.
/// Copy the original prepared bindings with their original audit references.
fn project_historical_conversations(tx: &Transaction<'_>, through_seq: i64) -> anyhow::Result<()> {
    let mut query = tx.prepare(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events
         WHERE kind=?1 AND seq<=?2 ORDER BY seq",
    )?;
    for stored in query.query_map(params![LAUNCHED_KIND, through_seq], row_event)? {
        let stored = stored?;
        let launched: Launched = serde_json::from_value(stored.event.data)?;
        let binding = &launched.conversation;
        tx.execute(
            "INSERT INTO session_conversations(session_id,binding_json,event_seq,event_hash) VALUES (?1,?2,?3,?4)",
            params![
                binding.session_id,
                serde_json::to_string(binding)?,
                stored.seq,
                stored.hash,
            ],
        )?;
    }
    Ok(())
}
