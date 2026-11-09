use super::*;
use authorization::{Basis, Evidence};
use model::{Change, Creator, Record};
use rusqlite::types::Value as Sql;

const SELECT_ROW: &str = "SELECT reservation_id,session_id,attempt_id,worker_id,generation,input_hash,session_hash,attempt_hash,profile_hash,state,revision,quota_held,effect_state,effect_revision,cpu_millis,memory_bytes,deadline_seconds,snapshot_json,snapshot_hash,last_event_seq FROM kubernetes_session_reservations";
fn row(
    snapshot: &KubernetesSessionReservationSnapshotV2,
    seq: i64,
    tenant: &str,
) -> anyhow::Result<Vec<Sql>> {
    snapshot.validate(tenant)?;
    let ids = snapshot.intent.ids()?;
    let state = match snapshot.state.state {
        KubernetesSessionReservationStateV2::Reserved => "reserved",
        KubernetesSessionReservationStateV2::ClosedBeforeEffect => "closed_before_effect",
    };
    let text = |value: String| Sql::Text(value);
    Ok(vec![
        text(snapshot.intent.reservation_id().into()),
        text(ids.session_id.to_string()),
        text(ids.attempt_id.to_string()),
        text(snapshot.intent.worker_id().into()),
        Sql::Integer(1),
        text(snapshot.input_hash.clone()),
        text(snapshot.session_hash.clone()),
        text(snapshot.attempt_hash.clone()),
        text(snapshot.profile_hash.clone()),
        text(state.into()),
        Sql::Integer(i64::try_from(snapshot.state.revision)?),
        Sql::Integer(i64::from(snapshot.state.quota_held)),
        text("never_issued".into()),
        Sql::Integer(0),
        Sql::Integer(i64::from(snapshot.intent.resources().cpu_millis)),
        Sql::Integer(i64::try_from(memory_bytes(&snapshot.intent)?)?),
        Sql::Integer(i64::from(snapshot.intent.resources().deadline_seconds)),
        text(serde_json::to_string(snapshot)?),
        text(snapshot.snapshot_hash(tenant)?),
        Sql::Integer(seq),
    ])
}
fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Vec<Sql>> {
    (0..20).map(|column| row.get(column)).collect()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotWire {
    version: u32,
    intent: Value,
    input_hash: String,
    session_hash: String,
    attempt_hash: String,
    profile_hash: String,
    state: KubernetesSessionReservationStateDataV2,
}
pub(super) fn load_snapshot(
    db: &Connection,
    id: &str,
    tenant: &str,
) -> anyhow::Result<Option<(KubernetesSessionReservationSnapshotV2, i64)>> {
    let row: Option<(String, i64, String)> = db.query_row("SELECT snapshot_json,last_event_seq,snapshot_hash FROM kubernetes_session_reservations WHERE reservation_id=?1", [id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
    row.map(|(json, seq, hash)| {
        let value =
            crate::control::kubernetes_session::validation::unique(json.as_bytes(), 96 * 1024)?;
        let wire: SnapshotWire = serde_json::from_value(value)?;
        let snapshot = KubernetesSessionReservationSnapshotV2 {
            version: wire.version,
            intent: KubernetesSessionReservationIntentV2::retained_from_value(wire.intent)?,
            input_hash: wire.input_hash,
            session_hash: wire.session_hash,
            attempt_hash: wire.attempt_hash,
            profile_hash: wire.profile_hash,
            state: wire.state,
        };
        conflict(
            snapshot.intent.reservation_id() == id
                && seq > 0
                && snapshot.snapshot_hash(tenant)? == hash,
        )?;
        Ok((snapshot, seq))
    })
    .transpose()
}
pub(super) fn write_snapshot(
    db: &Connection,
    snapshot: &KubernetesSessionReservationSnapshotV2,
    seq: i64,
    previous: Option<u64>,
    tenant: &str,
) -> anyhow::Result<()> {
    let expected = row(snapshot, seq, tenant)?;
    let mut values = expected.clone();
    let sql = if let Some(previous) = previous {
        values.push(Sql::Integer(i64::try_from(previous)?));
        "UPDATE kubernetes_session_reservations SET session_id=?2,attempt_id=?3,worker_id=?4,generation=?5,input_hash=?6,session_hash=?7,attempt_hash=?8,profile_hash=?9,state=?10,revision=?11,quota_held=?12,effect_state=?13,effect_revision=?14,cpu_millis=?15,memory_bytes=?16,deadline_seconds=?17,snapshot_json=?18,snapshot_hash=?19,last_event_seq=?20 WHERE reservation_id=?1 AND revision=?21 AND state='reserved' AND quota_held=1 AND effect_state='never_issued' AND effect_revision=0"
    } else {
        "INSERT INTO kubernetes_session_reservations(reservation_id,session_id,attempt_id,worker_id,generation,input_hash,session_hash,attempt_hash,profile_hash,state,revision,quota_held,effect_state,effect_revision,cpu_millis,memory_bytes,deadline_seconds,snapshot_json,snapshot_hash,last_event_seq) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)"
    };
    conflict(db.execute(sql, rusqlite::params_from_iter(values))? == 1)?;
    let actual = db.query_row(
        &format!("{SELECT_ROW} WHERE reservation_id=?1"),
        [snapshot.intent.reservation_id()],
        read_row,
    )?;
    conflict(actual == expected)
}
pub(super) fn operation(
    db: &Connection,
    id: &str,
) -> anyhow::Result<Option<(StoredEvent, Record)>> {
    let stored = db
        .query_row(
            "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE id=?1",
            [id],
            row_event,
        )
        .optional()?;
    stored
        .map(|stored| {
            conflict(matches!(
                stored.event.kind.as_str(),
                "control.kubernetes.session.reserved"
                    | "control.kubernetes.session.closed_before_effect"
            ))?;
            let record = model::record(&stored.event).map_err(|_| Failure::Unavailable)?;
            Ok((stored, record))
        })
        .transpose()
}
pub(super) fn reservation(
    db: &Connection,
    id: &str,
    tenant: &str,
) -> anyhow::Result<(StoredEvent, Record)> {
    let (stored, record) = operation(db, id)?.ok_or(Failure::NotFound)?;
    permission(
        matches!(record.change, Change::Reserve)
            && record.intent.reservation_id() == id
            && record.previous_revision == 0,
    )?;
    validate_historical(db, &stored, &record, tenant)?;
    Ok((stored, record))
}
pub(super) fn validate_origin(
    db: &Connection,
    evidence: &Evidence,
    before: Option<i64>,
) -> anyhow::Result<()> {
    let (marker, _) = migration::genesis(db, &evidence.actor.tenant_id)?;
    permission(
        evidence.origin_seq >= marker && before.is_none_or(|seq| evidence.origin_seq < seq),
    )?;
    let actual: String = db.query_row(
        "SELECT hash FROM events WHERE seq=?1",
        [evidence.origin_seq],
        |row| row.get(0),
    )?;
    permission(actual == evidence.origin_hash)
}
fn exact_reservation_reference(
    db: &Connection,
    record: &Record,
    tenant: &str,
) -> anyhow::Result<()> {
    let reference = match &record.authorization.broker()?.basis {
        Basis::Reserve { .. } => return Ok(()),
        Basis::Reconcile {
            reservation_seq,
            reservation_hash,
            reservation_decision_hash,
            ..
        } => {
            let (stored, original) = reservation(db, record.intent.reservation_id(), tenant)?;
            permission(
                stored.seq == *reservation_seq
                    && stored.hash == *reservation_hash
                    && original.intent.same(&record.intent)?
                    && original.authorization.broker()?.decision_hash == *reservation_decision_hash,
            )?;
            (stored, original)
        }
        Basis::RecoverBeforeEffect {
            creator_reference, ..
        } => {
            permission(creator_reference == &record.creator)?;
            reservation(db, record.intent.reservation_id(), tenant)?
        }
        Basis::RecoveryReceiptLookup { .. } => return Err(Failure::PermissionDenied.into()),
    };
    let Creator::Reservation {
        reservation_seq,
        reservation_hash,
        actor,
    } = &record.creator
    else {
        return Err(Failure::PermissionDenied.into());
    };
    permission(
        reference.0.seq == *reservation_seq
            && reference.0.hash == *reservation_hash
            && reference.1.actor.identity()? == *actor
            && reference.1.intent.same(&record.intent)?,
    )
}
pub(super) fn validate_historical(
    db: &Connection,
    stored: &StoredEvent,
    record: &Record,
    tenant: &str,
) -> anyhow::Result<()> {
    if record.authorization.control().is_some() {
        return control_recovery::validate_historical(db, stored, record, tenant);
    }
    let authorization = record.authorization.broker()?;
    let actor = record.actor.identity()?;
    let identity = actor.identity()?;
    let received = parse_time(&record.received_at)?;
    let issued = parse_time(&authorization.issued_at)?;
    permission(
        actor.tenant_id == tenant
            && authorization.actor == actor
            && parse_time(&stored.received_at)? >= received,
    )?;
    validate_origin(db, authorization, Some(stored.seq))?;
    workload::validate_broker_identity_at(db, &identity, received, stored.seq)?;
    let before_origin = authorization
        .origin_seq
        .checked_add(1)
        .ok_or(Failure::Unavailable)?;
    workload::validate_broker_identity_at(db, &identity, issued, before_origin)?;
    for before in [before_origin, stored.seq] {
        let binding = workload::prefix_binding(db, &actor.credential_id, before)?
            .ok_or(Failure::PermissionDenied)?;
        let broker = broker_registry::broker_at(db, &actor.broker_id, before)?
            .ok_or(Failure::PermissionDenied)?;
        permission(
            broker.incarnation_id == actor.incarnation_id
                && parse_time(&authorization.expires_at)? <= parse_time(&binding.expires_at)?
                && parse_time(&authorization.expires_at)? <= parse_time(&broker.not_after)?
                && parse_time(&binding.updated_at)? <= issued
                && parse_time(&broker.granted_at)? <= issued,
        )?;
    }
    let ids = record.intent.ids()?;
    let namespace: String = db.query_row(
        "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    permission(
        namespace == ids.control_namespace_uuid.to_string()
            && actor.broker_id == ids.broker_id.to_string(),
    )?;
    if matches!(
        authorization.basis,
        Basis::Reserve { .. } | Basis::Reconcile { .. }
    ) {
        let broker = broker_registry::broker_at(db, &actor.broker_id, stored.seq)?
            .ok_or(Failure::PermissionDenied)?;
        permission(
            actor.incarnation_id == ids.incarnation_id.to_string()
                && broker.authorities.iter().any(|authority| {
                    authority.runtime_kind == BrokerRuntimeKindV1::Kubernetes
                        && authority.authority_id
                            == record.intent.authority().authority_id().to_string()
                }),
        )?;
    }
    exact_reservation_reference(db, record, tenant)
}
pub(super) fn ensure_no_legacy_identity(
    db: &Connection,
    intent: &KubernetesSessionReservationIntentV2,
) -> anyhow::Result<()> {
    let ids = intent.ids()?;
    let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM workers WHERE worker_id=?1 OR session_id=?2) OR EXISTS(SELECT 1 FROM worker_attempts WHERE attempt_id=?3) OR EXISTS(SELECT 1 FROM kubernetes_creates WHERE session_id=?2)",
        params![intent.worker_id(), ids.session_id.to_string(), ids.attempt_id.to_string()], |row| row.get(0))?;
    conflict(!exists)
}
pub(super) fn ensure_quota(
    usage: &crate::control::QuotaUsageV1,
    intent: &KubernetesSessionReservationIntentV2,
    reserve: bool,
) -> anyhow::Result<()> {
    let resources = intent.resources();
    let quota = &usage.policy;
    quota.validate()?;
    if usage
        .held_workers
        .checked_add(u32::from(reserve))
        .is_some_and(|count| count <= quota.max_active_workers)
        && usage
            .held_cpu_millis
            .checked_add(if reserve {
                u64::from(resources.cpu_millis)
            } else {
                0
            })
            .is_some_and(|cpu| cpu <= u64::from(quota.max_cpu_millis))
        && usage
            .held_memory_bytes
            .checked_add(if reserve { memory_bytes(intent)? } else { 0 })
            .is_some_and(|memory| memory <= quota.max_memory_bytes)
        && resources.deadline_seconds <= quota.max_timeout_seconds
    {
        Ok(())
    } else {
        Err(Failure::QuotaExceeded.into())
    }
}

/// Pure prefix reconstruction, deliberately without recursive quota validation.
fn prefix(
    db: &Connection,
    before: i64,
) -> anyhow::Result<BTreeMap<String, (KubernetesSessionReservationSnapshotV2, i64)>> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version < 11 {
        return Ok(BTreeMap::new());
    }
    let tenant: String = db.query_row(
        "SELECT tenant_id FROM tenant_binding WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let (marker, _) = migration::genesis(db, &tenant)?;
    let mut states = BTreeMap::<String, (KubernetesSessionReservationSnapshotV2, i64)>::new();
    let mut statement = db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind GLOB 'control.kubernetes.session.*' AND seq<?1 ORDER BY seq")?;
    for item in statement.query_map([before], row_event)? {
        let stored = item?;
        if stored.event.kind == GENESIS_KIND {
            migration::marker(&stored.event)?;
            continue;
        }
        conflict(stored.seq > marker)?;
        let record = model::record(&stored.event)?;
        let previous = states.get(record.intent.reservation_id());
        conflict(
            record.previous_revision == previous.map_or(0, |(snapshot, _)| snapshot.state.revision),
        )?;
        if let Some((old, _)) = previous {
            conflict(old.intent.same(&record.intent)?)?;
        }
        let state = model::transition(
            &record.intent,
            previous.map(|(snapshot, _)| &snapshot.state),
            &record.change,
            &tenant,
        )?;
        conflict(state == record.post_state)?;
        states.insert(
            record.intent.reservation_id().into(),
            (model::snapshot(&record.intent, state, &tenant)?, stored.seq),
        );
    }
    Ok(states)
}
pub(super) fn held_usage_before(db: &Connection, before: i64) -> anyhow::Result<(u64, u64, u64)> {
    let mut count = 0u64;
    let mut cpu = 0u64;
    let mut memory = 0u64;
    for (snapshot, _) in prefix(db, before)?.values() {
        if snapshot.state.quota_held {
            count = count.checked_add(1).ok_or(Failure::QuotaExceeded)?;
            cpu = cpu
                .checked_add(u64::from(snapshot.intent.resources().cpu_millis))
                .ok_or(Failure::QuotaExceeded)?;
            memory = memory
                .checked_add(memory_bytes(&snapshot.intent)?)
                .ok_or(Failure::QuotaExceeded)?;
        }
    }
    Ok((count, cpu, memory))
}
pub(super) fn validate(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let present: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM events WHERE kind GLOB 'control.kubernetes.session.*')",
        [],
        |row| row.get(0),
    )?;
    if version < 11 {
        return conflict(!present);
    }
    require_schema(db)?;
    let tenant: String = db.query_row(
        "SELECT tenant_id FROM tenant_binding WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let (marker, _) = migration::genesis(db, &tenant)?;
    let mut statement = db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind GLOB 'control.kubernetes.session.*' AND kind<>?1 ORDER BY seq")?;
    for stored in statement.query_map([GENESIS_KIND], row_event)? {
        let stored = stored?;
        conflict(stored.seq > marker)?;
        let record = model::record(&stored.event)?;
        validate_historical(db, &stored, &record, &tenant)?;
        ensure_no_legacy_identity(db, &record.intent)?;
        if matches!(record.change, Change::Reserve) {
            ensure_quota(
                &control::quota_usage_before(
                    db,
                    stored.seq,
                    &record.authorization.broker()?.quota,
                )?,
                &record.intent,
                true,
            )?;
        }
    }
    let states = prefix(db, i64::MAX)?;
    let mut sessions = std::collections::BTreeSet::new();
    let mut attempts = std::collections::BTreeSet::new();
    let mut workers = std::collections::BTreeSet::new();
    for (snapshot, _) in states.values() {
        let ids = snapshot.intent.ids()?;
        conflict(
            sessions.insert(ids.session_id)
                && attempts.insert(ids.attempt_id)
                && workers.insert(snapshot.intent.worker_id().to_owned()),
        )?;
    }
    let mut query = db.prepare(&format!("{SELECT_ROW} ORDER BY reservation_id"))?;
    let actual = query
        .query_map([], read_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let expected = states
        .values()
        .map(|(snapshot, seq)| row(snapshot, *seq, &tenant))
        .collect::<anyhow::Result<Vec<_>>>()?;
    conflict(actual == expected)
}
