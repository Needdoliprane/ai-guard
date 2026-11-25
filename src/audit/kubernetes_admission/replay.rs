//! Projection verification and historical decisions; no repair or capabilities.
use super::*;
use crate::broker_registry::{format_time, parse_time};
use crate::control::QuotaUsageV1;
use authorization::permission;
use model::{Change, Record, conflict};
use rusqlite::types::Value as SqlValue;

const SELECT_ROW: &str = "SELECT create_id,session_id,intent_hash,state,create_epoch,revision,quota_held,cpu_millis,memory_bytes,snapshot_json,snapshot_hash,last_event_seq FROM kubernetes_creates";
fn sql_row(snapshot: &KubernetesCreateSnapshotV1, seq: i64) -> anyhow::Result<Vec<SqlValue>> {
    let state = serde_json::to_value(snapshot.state)?
        .as_str()
        .ok_or(KubernetesCreateFailureV1::InvalidRequest)?
        .to_owned();
    let text = |value: String| SqlValue::Text(value);
    Ok(vec![
        text(snapshot.intent.create_id.clone()),
        text(snapshot.intent.session_id.clone()),
        text(snapshot.intent_hash.clone()),
        text(state),
        SqlValue::Integer(i64::try_from(snapshot.create_epoch)?),
        SqlValue::Integer(i64::try_from(snapshot.revision)?),
        SqlValue::Integer(i64::from(snapshot.quota_held)),
        SqlValue::Integer(i64::from(snapshot.intent.resources.cpu_millis)),
        SqlValue::Integer(i64::try_from(memory_bytes(&snapshot.intent)?)?),
        text(serde_json::to_string(snapshot)?),
        text(snapshot.snapshot_hash()?),
        SqlValue::Integer(seq),
    ])
}
fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Vec<SqlValue>> {
    (0..12).map(|column| row.get(column)).collect()
}
pub(super) fn load_snapshot(
    db: &Connection,
    create_id: &str,
) -> anyhow::Result<Option<KubernetesCreateSnapshotV1>> {
    db.query_row(
        "SELECT snapshot_json FROM kubernetes_creates WHERE create_id=?1",
        [create_id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|json| serde_json::from_str(&json).map_err(Into::into))
    .transpose()
}
pub(super) fn memory_bytes(intent: &KubernetesCreateIntentV1) -> anyhow::Result<u64> {
    u64::from(intent.resources.memory_mib)
        .checked_mul(1_048_576)
        .ok_or_else(|| KubernetesCreateFailureV1::QuotaExceeded.into())
}
pub(super) fn write_snapshot(
    db: &Connection,
    snapshot: &KubernetesCreateSnapshotV1,
    seq: i64,
    previous: Option<u64>,
) -> anyhow::Result<()> {
    let expected = sql_row(snapshot, seq)?;
    let (sql, values) = if let Some(previous) = previous {
        let mut values = expected.clone();
        values.push(SqlValue::Integer(i64::try_from(previous)?));
        (
            "UPDATE kubernetes_creates SET session_id=?2,intent_hash=?3,state=?4,create_epoch=?5,revision=?6,quota_held=?7,cpu_millis=?8,memory_bytes=?9,snapshot_json=?10,snapshot_hash=?11,last_event_seq=?12 WHERE create_id=?1 AND revision=?13",
            values,
        )
    } else {
        (
            "INSERT INTO kubernetes_creates(create_id,session_id,intent_hash,state,create_epoch,revision,quota_held,cpu_millis,memory_bytes,snapshot_json,snapshot_hash,last_event_seq) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            expected.clone(),
        )
    };
    conflict(db.execute(sql, rusqlite::params_from_iter(values))? == 1)?;
    let actual = db.query_row(
        &format!("{SELECT_ROW} WHERE create_id=?1"),
        [&snapshot.intent.create_id],
        read_row,
    )?;
    conflict(actual == expected)
}

pub(super) fn record(stored: &StoredEvent) -> anyhow::Result<Record> {
    let record: Record = serde_json::from_value(stored.event.data.clone())?;
    record.change.validate()?;
    record.intent.validate()?;
    let is_result = matches!(record.change, Change::Result { .. });
    conflict(
        matches!(record.version, 1..=3)
            && record.version == *record.authorization.version()
            && (record.version == 3)
                == matches!(record.change, Change::RecoverBeforeEmission { .. })
            && stored.event.id == record.change.operation_id()
            && stored.event.kind == record.change.kind()
            && record.input_hash == record.change.input_hash()?
            && record.intent.create_id == record.change.create_id()
            && stored.event.timestamp == record.received_at
            && stored.event.source == "gateway"
            && stored.event.provenance == if is_result { "declared" } else { "observed" }
            && stored.event.coverage == if is_result { "partial" } else { "observed" }
            && format_time(parse_time(&record.received_at)?) == record.received_at
            && parse_time(&record.received_at)? <= parse_time(&stored.received_at)?,
    )?;
    if record.version == 3 {
        conflict(
            stored.event.session_id.is_none()
                && stored.event.project_id.is_none()
                && stored.event.sandbox_id.is_none()
                && stored.event.agent_id.is_none()
                && stored.event.parent_agent_id.is_none()
                && stored.event.correlation_id.is_none()
                && stored.event.trace_id.is_none()
                && stored.event.command.is_none()
                && stored.event.decision.is_none()
                && stored.event.result.is_none(),
        )?;
    }
    record
        .authorization
        .validate_access(&record.change, mutations::HeldChangeAccess::Mutate)?;
    Ok(record)
}

fn policy_marker(db: &Connection) -> anyhow::Result<Option<i64>> {
    Ok(db
        .query_row(
            "SELECT seq FROM events WHERE kind=?1",
            [migration::POLICY_GENESIS_KIND],
            |row| row.get(0),
        )
        .optional()?)
}

pub(super) fn validate_v2_origin(
    db: &Connection,
    evidence: &authorization::AuthorizationEvidence,
    record_seq: Option<i64>,
) -> anyhow::Result<()> {
    let marker = policy_marker(db)?.ok_or(KubernetesCreateFailureV1::PermissionDenied)?;
    let origin = *evidence.origin_seq();
    permission(origin >= marker && record_seq.is_none_or(|before| origin < before))?;
    let actual: String = db.query_row("SELECT hash FROM events WHERE seq=?1", [origin], |row| {
        row.get(0)
    })?;
    permission(evidence.origin_hash() == &actual)
}

pub(super) fn reservation(
    db: &Connection,
    intent: &KubernetesCreateIntentV1,
    tenant: &str,
) -> anyhow::Result<(StoredEvent, Record)> {
    let stored = db
        .query_row(
            "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE id=?1",
            [&intent.create_id],
            row_event,
        )
        .optional()?
        .ok_or(KubernetesCreateFailureV1::NotFound)?;
    let record = record(&stored)?;
    permission(
        matches!(&record.change, Change::Reserve { intent: original } if original == intent)
            && record.intent == *intent
            && record.previous_revision == 0,
    )?;
    // A Reserve's own evidence is Admission (or historical V1), so this check
    // cannot recursively follow a Reconcile reference or another result.
    validate_historical_identity(db, &stored, &record, tenant)?;
    Ok((stored, record))
}

pub(super) fn validate_current_basis(
    db: &Connection,
    evidence: &authorization::AuthorizationEvidence,
    intent: &KubernetesCreateIntentV1,
    tenant: &str,
) -> anyhow::Result<()> {
    permission(*evidence.version() != 3)?;
    let Some(basis) = evidence.basis() else {
        return Ok(());
    };
    validate_v2_origin(db, evidence, None)?;
    if let authorization::AuthorizationBasisV2::Reconcile {
        reservation_seq,
        reservation_hash,
        admission_decision_hash,
    } = basis
    {
        let (stored, original) = reservation(db, intent, tenant)?;
        permission(
            stored.seq == *reservation_seq
                && stored.seq <= *evidence.origin_seq()
                && stored.hash == *reservation_hash
                && original.authorization.decision_hash() == admission_decision_hash,
        )?;
    }
    Ok(())
}

pub(super) fn validate_historical_identity(
    db: &Connection,
    stored: &StoredEvent,
    record: &Record,
    tenant: &str,
) -> anyhow::Result<()> {
    if let authorization::AuthorizationEvidence::V3(evidence) = &record.authorization {
        return super::recovery::validate_historical_identity(db, stored, record, tenant, evidence);
    }
    let identity = record.identity.identity()?;
    let at = parse_time(&record.received_at)?;
    permission(identity.tenant_id() == tenant && record.authorization.tenant_id() == tenant)?;
    match (record.version, policy_marker(db)?) {
        (1, Some(marker)) => permission(stored.seq < marker)?,
        (1, None) => {}
        (2, Some(marker)) => permission(stored.seq > marker)?,
        _ => return Err(KubernetesCreateFailureV1::PermissionDenied.into()),
    }
    workload::validate_broker_identity_at(db, &identity, at, stored.seq)?;
    record
        .authorization
        .validate(&identity, &record.intent, at)?;
    let origin = *record.authorization.origin_seq();
    let before_origin = origin
        .checked_add(1)
        .ok_or(KubernetesCreateFailureV1::InvalidRequest)?;
    permission(origin > 0 && origin < stored.seq)?;
    let origin_hash: String =
        db.query_row("SELECT hash FROM events WHERE seq=?1", [origin], |row| {
            row.get(0)
        })?;
    let genesis: i64 = db.query_row(
        "SELECT seq FROM events WHERE kind=?1",
        [GENESIS_KIND],
        |row| row.get(0),
    )?;
    permission(&origin_hash == record.authorization.origin_hash() && origin >= genesis)?;
    if record.version == 2 {
        validate_v2_origin(db, &record.authorization, Some(stored.seq))?;
        validate_current_basis(db, &record.authorization, &record.intent, tenant)?;
    }
    let issued_at = parse_time(record.authorization.issued_at())?;
    workload::validate_broker_identity_at(db, &identity, issued_at, before_origin)?;
    // The capability was issued against one exact durable prefix. Reusing it
    // after other events must not compare issuance with their later time floor.
    for before in [before_origin, stored.seq] {
        let binding = workload::prefix_binding(db, identity.credential_id(), before)?
            .ok_or(KubernetesCreateFailureV1::PermissionDenied)?;
        let grant = broker_registry::broker_at(db, identity.broker_id(), before)?
            .ok_or(KubernetesCreateFailureV1::PermissionDenied)?;
        permission(
            parse_time(record.authorization.expires_at())? <= parse_time(&binding.expires_at)?
                && parse_time(record.authorization.expires_at())? <= parse_time(&grant.not_after)?
                && parse_time(&binding.updated_at)? <= issued_at
                && parse_time(&grant.granted_at)? <= issued_at,
        )?;
    }
    let broker = broker_registry::broker_at(db, identity.broker_id(), stored.seq)?
        .ok_or(KubernetesCreateFailureV1::PermissionDenied)?;
    let namespace: String = db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    permission(
        record.intent.control_namespace_uuid == namespace
            && record.intent.broker_id == identity.broker_id()
            && record.intent.incarnation_id == broker.incarnation_id
            && broker.authorities.iter().any(|authority| {
                authority.runtime_kind == crate::broker_registry::BrokerRuntimeKindV1::Kubernetes
                    && authority.authority_id == record.intent.authority.authority_id().to_string()
            }),
    )
}

pub(super) fn ensure_quota(
    usage: &QuotaUsageV1,
    intent: &KubernetesCreateIntentV1,
    reserve: bool,
) -> anyhow::Result<()> {
    let quota = &usage.policy;
    quota.validate()?;
    let count = usage.held_workers.checked_add(u32::from(reserve));
    let cpu = usage.held_cpu_millis.checked_add(if reserve {
        u64::from(intent.resources.cpu_millis)
    } else {
        0
    });
    let memory =
        usage
            .held_memory_bytes
            .checked_add(if reserve { memory_bytes(intent)? } else { 0 });
    if count.is_some_and(|v| v <= quota.max_active_workers)
        && cpu.is_some_and(|v| v <= u64::from(quota.max_cpu_millis))
        && memory.is_some_and(|v| v <= quota.max_memory_bytes)
        && intent.resources.deadline_seconds <= quota.max_timeout_seconds
    {
        Ok(())
    } else {
        Err(KubernetesCreateFailureV1::QuotaExceeded.into())
    }
}

/// Read only the already typed historical post-states. This helper deliberately
/// does not call a quota validator, so mixed V1/V2 replay cannot recurse.
pub(super) fn held_usage_before(db: &Connection, before: i64) -> anyhow::Result<(u64, u64, u64)> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version < 8 {
        return Ok((0, 0, 0));
    }
    let mut creates = BTreeMap::new();
    let mut query = db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind GLOB 'control.kubernetes.create.*' AND kind NOT IN ('control.kubernetes.create.genesis','control.kubernetes.create.policy.genesis','control.kubernetes.create.recovery.genesis') AND seq<?1 ORDER BY seq")?;
    for item in query.query_map([before], row_event)? {
        let record = record(&item?)?;
        creates.insert(record.intent.create_id.clone(), record.snapshot);
    }
    let (mut count, mut cpu, mut memory) = (0u64, 0u64, 0u64);
    for snapshot in creates.values() {
        if snapshot.quota_held {
            count = count
                .checked_add(1)
                .ok_or(KubernetesCreateFailureV1::QuotaExceeded)?;
            cpu = cpu
                .checked_add(u64::from(snapshot.intent.resources.cpu_millis))
                .ok_or(KubernetesCreateFailureV1::QuotaExceeded)?;
            memory = memory
                .checked_add(memory_bytes(&snapshot.intent)?)
                .ok_or(KubernetesCreateFailureV1::QuotaExceeded)?;
        }
    }
    Ok((count, cpu, memory))
}

pub(super) fn validate(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let mut statement = db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind GLOB 'control.kubernetes.create.*' ORDER BY seq")?;
    let events = statement
        .query_map([], row_event)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if version < 8 {
        conflict(events.is_empty())?;
        return Ok(());
    }
    conflict([8, 9, 10, 11, 12, 13, 14, 15].contains(&version))?;
    migration::validate_recovery_marker(db)?;
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
    let mut create_genesis: Option<(i64, String)> = None;
    let mut policy_seen = false;
    let mut recovery_seen = false;
    let mut creates = BTreeMap::<String, (KubernetesCreateSnapshotV1, i64)>::new();
    for stored in events {
        if stored.event.kind == GENESIS_KIND {
            conflict(create_genesis.is_none() && creates.is_empty())?;
            let genesis: Genesis = serde_json::from_value(stored.event.data.clone())?;
            conflict(
                genesis.version == 1
                    && genesis.tenant_id == tenant
                    && genesis.control_namespace_uuid == namespace
                    && genesis.previous_seq.checked_add(1) == Some(stored.seq)
                    && genesis.previous_hash == stored.prev_hash
                    && stored.event.source == "gateway"
                    && stored.event.provenance == "observed",
            )?;
            create_genesis = Some((stored.seq, stored.hash.clone()));
            continue;
        }
        if stored.event.kind == migration::POLICY_GENESIS_KIND {
            conflict([9, 10, 11, 12, 13, 14, 15].contains(&version) && !policy_seen)?;
            let (genesis_seq, genesis_hash) = create_genesis
                .as_ref()
                .ok_or(KubernetesCreateFailureV1::Conflict)?;
            migration::validate_marker(&stored, &tenant, &namespace, *genesis_seq, genesis_hash)?;
            policy_seen = true;
            continue;
        }
        if stored.event.kind == migration::RECOVERY_GENESIS_KIND {
            conflict(matches!(version, 13..=15) && policy_seen && !recovery_seen)?;
            migration::recovery_event(&stored.event)?;
            recovery_seen = true;
            continue;
        }
        conflict(create_genesis.is_some())?;
        let record = record(&stored)?;
        if record.version == 3 {
            conflict(recovery_seen && policy_seen)?;
        } else {
            conflict(record.version == if policy_seen { 2 } else { 1 })?;
        }
        validate_historical_identity(db, &stored, &record, &tenant)?;
        let previous = creates
            .get(record.change.create_id())
            .map(|(snapshot, _)| snapshot);
        conflict(record.previous_revision == previous.map_or(0, |snapshot| snapshot.revision))?;
        if matches!(record.change, Change::Reserve { .. }) {
            conflict(
                !creates
                    .values()
                    .any(|(snapshot, _)| snapshot.intent.session_id == record.intent.session_id),
            )?;
            let mut workers = db.prepare("SELECT worker_id FROM workers WHERE session_id=?1")?;
            for worker in
                workers.query_map([&record.intent.session_id], |row| row.get::<_, String>(0))?
            {
                conflict(control::worker_at(db, &worker?, stored.seq)?.is_none())?;
            }
        }
        if record.change.requires_active() {
            let usage = control::quota_usage_before(db, stored.seq, record.authorization.quota())?;
            ensure_quota(
                &usage,
                &record.intent,
                matches!(record.change, Change::Reserve { .. }),
            )?;
        }
        let window = if matches!(record.change, Change::Claim { .. }) {
            let broker = broker_registry::broker_at(db, &record.intent.broker_id, stored.seq)?
                .ok_or(KubernetesCreateFailureV1::PermissionDenied)?;
            Some(model::emission_window(
                parse_time(&record.received_at)?,
                record.authorization.expires_at(),
                &broker.not_after,
            )?)
        } else {
            None
        };
        let expected = model::transition(&record.intent, previous, &record.change, window)?;
        conflict(expected == record.snapshot)?;
        creates.insert(record.intent.create_id.clone(), (expected, stored.seq));
    }
    conflict(
        create_genesis.is_some()
            && policy_seen == ([9, 10, 11, 12, 13, 14, 15].contains(&version))
            && recovery_seen == matches!(version, 13..=15),
    )?;
    let mut statement = db.prepare(&format!("{SELECT_ROW} ORDER BY create_id"))?;
    let actual = statement
        .query_map([], read_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let expected = creates
        .values()
        .map(|(snapshot, seq)| sql_row(snapshot, *seq))
        .collect::<anyhow::Result<Vec<_>>>()?;
    conflict(actual == expected)?;
    let duplicate: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM workers JOIN kubernetes_creates ON workers.session_id=kubernetes_creates.session_id)", [], |row| row.get(0))?;
    conflict(!duplicate)
}
