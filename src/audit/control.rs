use super::worker_admission::{self, ControlRecord, admission_result, receipt};
use super::worker_transition::{
    self, ObservationClass, allocation_source, apply_worker_transition, classify_observation,
};
use super::*;
use crate::control::*;

#[path = "supervision.rs"]
pub(super) mod supervision;

#[cfg(test)]
#[path = "control_session_quota_tests.rs"]
mod kubernetes_session_quota_tests;

#[cfg(test)]
#[path = "control/projection14_tests.rs"]
mod projection14_tests;

pub(super) const SCHEMA: [(&str, &str, &str); 7] = [
    (
        "table",
        "control_meta",
        "CREATE TABLE control_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),namespace_uuid TEXT NOT NULL UNIQUE,genesis_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq),genesis_hash TEXT NOT NULL)",
    ),
    (
        "table",
        "workers",
        "CREATE TABLE workers(worker_id TEXT PRIMARY KEY,admission_operation_id TEXT NOT NULL UNIQUE,request_id TEXT NOT NULL UNIQUE,session_id TEXT NOT NULL UNIQUE,state TEXT NOT NULL,quota_held INTEGER NOT NULL CHECK(quota_held IN (0,1)),cpu_millis INTEGER NOT NULL,memory_bytes INTEGER NOT NULL,revision INTEGER NOT NULL,snapshot_json TEXT NOT NULL,snapshot_hash TEXT NOT NULL,last_event_seq INTEGER NOT NULL REFERENCES events(seq))",
    ),
    (
        "table",
        "worker_attempts",
        "CREATE TABLE worker_attempts(attempt_id TEXT PRIMARY KEY,worker_id TEXT NOT NULL UNIQUE REFERENCES workers(worker_id),authority_id TEXT NOT NULL,stable_name TEXT NOT NULL UNIQUE,volume_name TEXT NOT NULL UNIQUE,container_id TEXT,create_operation_id TEXT UNIQUE,start_operation_id TEXT UNIQUE,start_emitter_id TEXT,snapshot_json TEXT NOT NULL)",
    ),
    (
        "table",
        "worker_observations",
        "CREATE TABLE worker_observations(observation_id TEXT PRIMARY KEY,worker_id TEXT NOT NULL REFERENCES workers(worker_id),attempt_id TEXT NOT NULL REFERENCES worker_attempts(attempt_id),input_hash TEXT NOT NULL,payload_json TEXT NOT NULL,event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq))",
    ),
    (
        "table",
        "worker_allocations",
        "CREATE TABLE worker_allocations(attempt_id TEXT PRIMARY KEY REFERENCES worker_attempts(attempt_id),worker_id TEXT NOT NULL REFERENCES workers(worker_id),terminal_observation_id TEXT NOT NULL UNIQUE REFERENCES worker_observations(observation_id),payload_json TEXT NOT NULL,event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq))",
    ),
    (
        "table",
        "worker_mutations",
        "CREATE TABLE worker_mutations(operation_id TEXT PRIMARY KEY,worker_id TEXT REFERENCES workers(worker_id),action TEXT NOT NULL,input_hash TEXT NOT NULL,event_seq INTEGER NOT NULL UNIQUE REFERENCES events(seq))",
    ),
    (
        "index",
        "workers_held",
        "CREATE INDEX workers_held ON workers(quota_held)",
    ),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Genesis {
    version: u32,
    namespace_uuid: String,
    previous_seq: i64,
    previous_hash: String,
}
type TableRows = BTreeMap<String, Vec<rusqlite::types::Value>>;
const TABLE_QUERIES: [(&str, &str); 6] = [
    (
        "workers",
        "SELECT worker_id,admission_operation_id,request_id,session_id,state,quota_held,cpu_millis,memory_bytes,revision,snapshot_json,snapshot_hash,last_event_seq FROM workers ORDER BY worker_id",
    ),
    (
        "worker_attempts",
        "SELECT attempt_id,worker_id,authority_id,stable_name,volume_name,container_id,create_operation_id,start_operation_id,start_emitter_id,snapshot_json FROM worker_attempts ORDER BY attempt_id",
    ),
    (
        "worker_observations",
        "SELECT observation_id,worker_id,attempt_id,input_hash,payload_json,event_seq FROM worker_observations ORDER BY observation_id",
    ),
    (
        "worker_allocations",
        "SELECT attempt_id,worker_id,terminal_observation_id,payload_json,event_seq FROM worker_allocations ORDER BY attempt_id",
    ),
    (
        "worker_mutations",
        "SELECT operation_id,worker_id,action,input_hash,event_seq FROM worker_mutations ORDER BY operation_id",
    ),
    (
        "control_meta",
        "SELECT singleton,namespace_uuid,genesis_seq,genesis_hash FROM control_meta ORDER BY singleton",
    ),
];
fn sql_text(value: impl Into<String>) -> rusqlite::types::Value {
    rusqlite::types::Value::Text(value.into())
}
fn optional_text(value: &Option<String>) -> rusqlite::types::Value {
    value
        .as_ref()
        .map_or(rusqlite::types::Value::Null, |v| sql_text(v.clone()))
}
fn integer(value: u64) -> anyhow::Result<rusqlite::types::Value> {
    Ok(rusqlite::types::Value::Integer(i64::try_from(value)?))
}
fn state_name(state: WorkerStateV1) -> anyhow::Result<String> {
    Ok(serde_json::to_value(state)?
        .as_str()
        .context("invalid worker state")?
        .into())
}
fn worker_row(worker: &WorkerV1, seq: i64) -> anyhow::Result<Vec<rusqlite::types::Value>> {
    Ok(vec![
        sql_text(&worker.worker_id),
        sql_text(&worker.admission_operation_id),
        sql_text(&worker.request_id),
        sql_text(&worker.launch.session_id),
        sql_text(state_name(worker.state)?),
        integer(u64::from(worker.quota_held))?,
        integer(u64::from(worker.launch.cpu_millis))?,
        integer(worker.launch.memory_bytes)?,
        integer(worker.revision)?,
        sql_text(serde_json::to_string(worker)?),
        sql_text(content_hash("ai-guard:worker-snapshot:v1", worker)?),
        rusqlite::types::Value::Integer(seq),
    ])
}
fn attempt_row(worker: &WorkerV1) -> anyhow::Result<Vec<rusqlite::types::Value>> {
    let a = &worker.attempt;
    Ok(vec![
        sql_text(&a.attempt_id),
        sql_text(&worker.worker_id),
        sql_text(&worker.launch.authority_id),
        sql_text(&a.stable_name),
        sql_text(&a.volume_name),
        optional_text(&a.container_id),
        optional_text(&a.create_operation_id),
        optional_text(&a.start_operation_id),
        optional_text(&a.start_emitter_id),
        sql_text(serde_json::to_string(a)?),
    ])
}
fn observation_row(
    record: &ControlRecord,
    worker: &WorkerV1,
    seq: i64,
) -> anyhow::Result<Vec<rusqlite::types::Value>> {
    Ok(vec![
        sql_text(&record.operation_id),
        sql_text(&worker.worker_id),
        sql_text(&worker.attempt.attempt_id),
        sql_text(&record.input_hash),
        sql_text(serde_json::to_string(
            &record.observation.as_ref().context("missing observation")?,
        )?),
        rusqlite::types::Value::Integer(seq),
    ])
}
fn allocation_row(
    allocation: &AllocationV1,
    seq: i64,
) -> anyhow::Result<Vec<rusqlite::types::Value>> {
    Ok(vec![
        sql_text(&allocation.attempt_id),
        sql_text(&allocation.worker_id),
        sql_text(&allocation.terminal_observation_id),
        sql_text(serde_json::to_string(allocation)?),
        rusqlite::types::Value::Integer(seq),
    ])
}
fn mutation_row(record: &ControlRecord, seq: i64) -> Vec<rusqlite::types::Value> {
    vec![
        sql_text(&record.operation_id),
        record
            .worker
            .as_ref()
            .map_or(rusqlite::types::Value::Null, |worker| {
                sql_text(&worker.worker_id)
            }),
        sql_text(&record.action),
        sql_text(&record.input_hash),
        rusqlite::types::Value::Integer(seq),
    ]
}
fn create_tables(db: &Connection) -> anyhow::Result<()> {
    for (_, _, sql) in SCHEMA {
        db.execute_batch(sql)?;
    }
    Ok(())
}
pub(super) fn initialize(
    tx: &Transaction<'_>,
    max_events: u64,
    redactor: &Redactor,
) -> anyhow::Result<()> {
    create_tables(tx)?;
    let (last, prev): (i64, String) = tx
        .query_row(
            "SELECT seq,hash FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .unwrap_or((0, "0".repeat(64)));
    anyhow::ensure!(
        last < i64::try_from(max_events)?,
        "audit capacity exhausted before control genesis"
    );
    let mut event = Event::new("control.schema.genesis");
    let genesis = Genesis {
        version: 2,
        namespace_uuid: uuid::Uuid::new_v4().to_string(),
        previous_seq: last,
        previous_hash: prev.clone(),
    };
    event.data = serde_json::to_value(&genesis)?;
    invalid(redactor.value(&serde_json::to_value(&event)?) == serde_json::to_value(&event)?)?;
    let payload = serde_json::to_string(&event)?;
    let used: i64 = tx.query_row("SELECT bytes FROM quota WHERE id=1", [], |r| r.get(0))?;
    anyhow::ensure!(
        used + i64::try_from(payload.len())? <= 256 * 1024 * 1024,
        "audit byte capacity exhausted before control genesis"
    );
    let seq = last + 1;
    let received = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let hash = super::digest(seq, &received, &prev, &payload);
    tx.execute("INSERT INTO events(seq,id,received_at,prev_hash,hash,payload,kind) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![seq,event.id,received,prev,hash,payload,event.kind])?;
    tx.execute(
        "UPDATE quota SET bytes=bytes+?1 WHERE id=1",
        [i64::try_from(payload.len())?],
    )?;
    tx.execute("INSERT INTO control_meta(singleton,namespace_uuid,genesis_seq,genesis_hash) VALUES(1,?1,?2,?3)",params![genesis.namespace_uuid,seq,hash])?;
    Ok(())
}

/// Reconstruct every column and row from the authenticated audit, never repair it.
pub(super) fn validate(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version == 1 {
        return Ok(());
    }
    anyhow::ensure!(
        [2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version),
        "unsupported control schema"
    );
    if matches!(version, 13 | 14 | 15) {
        super::kubernetes_admission::validate_recovery_marker(db)?;
    }
    let review_tenant: Option<String> = if matches!(version, 14 | 15) {
        Some(db.query_row(
            "SELECT tenant_id FROM tenant_binding WHERE singleton=1",
            [],
            |row| row.get(0),
        )?)
    } else {
        None
    };
    let (namespace, genesis_seq, genesis_hash): (String, i64, String) = db.query_row(
        "SELECT namespace_uuid,genesis_seq,genesis_hash FROM control_meta WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    uuid(&namespace)?;
    let genesis_event: StoredEvent = db.query_row(
        "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE seq=?1",
        [genesis_seq],
        row_event,
    )?;
    let genesis: Genesis = serde_json::from_value(genesis_event.event.data.clone())?;
    anyhow::ensure!(
        genesis.version == 2
            && genesis.namespace_uuid == namespace
            && genesis.previous_seq == genesis_seq - 1
            && genesis.previous_hash == genesis_event.prev_hash
            && genesis_event.hash == genesis_hash
            && genesis_event.event.kind == "control.schema.genesis"
            && genesis_event.event.source == "gateway"
            && genesis_event.event.provenance == "observed",
        "invalid control genesis"
    );
    let mut expected: BTreeMap<&str, TableRows> = TABLE_QUERIES
        .iter()
        .map(|(name, _)| (*name, BTreeMap::new()))
        .collect();
    expected
        .get_mut("control_meta")
        .context("missing table")?
        .insert(
            "1".into(),
            vec![
                rusqlite::types::Value::Integer(1),
                sql_text(namespace.clone()),
                rusqlite::types::Value::Integer(genesis_seq),
                sql_text(genesis_hash),
            ],
        );
    let mut workers: BTreeMap<String, WorkerV1> = BTreeMap::new();
    let mut overlay = supervision::Replay::new(db, version, namespace.clone())?;
    let mut statement=db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE seq>?1 AND kind GLOB 'control.*' ORDER BY seq")?;
    let events = statement
        .query_map([genesis_seq], row_event)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for stored in events {
        if stored.event.kind.starts_with("control.egress_policy.") {
            anyhow::ensure!(version == 15, "egress policy requires exact schema15");
            super::egress::marker(&stored.event)?;
            continue;
        }
        // Dispatch only the exact modern families. Their complete typed
        // histories and inherited projections are checked below as well.
        if matches!(
            stored.event.kind.as_str(),
            super::review_authority::ANCHOR_KIND | super::review_authority::INCARNATION_KIND
        ) {
            anyhow::ensure!(
                matches!(version, 14 | 15),
                "review authority requires schema14"
            );
            super::review_authority::marker(&stored.event)?;
            continue;
        }
        if matches!(
            stored.event.kind.as_str(),
            "control.review_binding.provisioned" | "control.review_binding.revoked"
        ) {
            let tenant = review_tenant
                .as_deref()
                .context("review binding requires schema14")?;
            super::review_binding::marker(&stored.event, tenant)?;
            continue;
        }
        if stored.event.kind == super::broker_checkpoint_migration::GENESIS_KIND {
            anyhow::ensure!(
                matches!(version, 12 | 13 | 14 | 15),
                "broker checkpoint marker requires schema12 or schema13"
            );
            super::broker_checkpoint_migration::marker(&stored.event)?;
            continue;
        }
        if stored.event.kind == "control.kubernetes.create.recovery.genesis" {
            anyhow::ensure!(
                matches!(version, 13 | 14 | 15),
                "held-create recovery marker requires schema13"
            );
            super::kubernetes_admission::recovery_event(&stored.event)?;
            continue;
        }
        if stored.event.kind.starts_with("control.kubernetes.session.") {
            anyhow::ensure!(
                [11, 12, 13, 14, 15].contains(&version),
                "Kubernetes session event requires schema11"
            );
            continue;
        }
        if stored.event.kind.starts_with("control.kubernetes.create.") {
            anyhow::ensure!(
                [8, 9, 10, 11, 12, 13, 14, 15].contains(&version),
                "held Kubernetes event requires schema8 or schema9"
            );
            continue;
        }
        if stored.event.kind.starts_with("control.workload.")
            || stored.event.kind.starts_with("control.broker_registry.")
            || stored.event.kind.starts_with("control.relay.")
        {
            anyhow::ensure!(
                [6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version),
                "workload control event requires schema6"
            );
            continue;
        }
        if stored.event.kind.starts_with("control.identity.") {
            anyhow::ensure!(
                [4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version),
                "identity event requires schema4 or5"
            );
            continue;
        }
        if stored.event.kind == "control.supervision.genesis" {
            overlay.genesis(&stored)?;
            continue;
        }
        let (record, mutation) = if stored.event.kind == "control.worker.transition" {
            anyhow::ensure!(
                stored.event.source == "gateway" && stored.event.provenance == "observed",
                "unexpected control provenance"
            );
            let record: ControlRecord = serde_json::from_value(stored.event.data.clone())?;
            overlay.legacy_allowed(stored.seq, &record)?;
            let row = mutation_row(&record, stored.seq);
            (Some(record), row)
        } else {
            overlay.apply(db, &stored, &workers)?
        };
        let operation_id = mutation
            .first()
            .and_then(|value| {
                if let rusqlite::types::Value::Text(value) = value {
                    Some(value.clone())
                } else {
                    None
                }
            })
            .context("mutation identity missing")?;
        uuid(&operation_id)?;
        anyhow::ensure!(
            expected
                .get_mut("worker_mutations")
                .context("missing table")?
                .insert(operation_id, mutation)
                .is_none(),
            "duplicate control operation"
        );
        let Some(record) = record else { continue };
        uuid(&record.operation_id)?;
        hash(&record.input_hash)?;
        anyhow::ensure!(record.version == 1, "unsupported legacy worker record");
        if let Some(worker) = &record.worker {
            worker.launch.validate()?;
            uuid(&worker.worker_id)?;
            uuid(&worker.attempt.attempt_id)?;
            anyhow::ensure!(
                worker.spec_hash == worker.launch.spec_hash()?
                    && worker.attempt.stable_name
                        == format!(
                            "aig-{}-{}",
                            namespace.replace('-', ""),
                            worker.worker_id.replace('-', "")
                        )
                    && worker.attempt.volume_name == format!("{}-work", worker.attempt.stable_name)
                    && worker.quota_held
                        == ![WorkerStateV1::Terminal, WorkerStateV1::Cancelled]
                            .contains(&worker.state)
                    && record.snapshot_hash.as_ref()
                        == Some(&content_hash("ai-guard:worker-snapshot:v1", worker)?),
                "control snapshot hash mismatch"
            );
            let previous = workers.get(&worker.worker_id);
            anyhow::ensure!(
                match previous {
                    None =>
                        record.action == "admit"
                            && record.previous_revision.is_none()
                            && worker.revision == 1,
                    Some(previous) =>
                        record.previous_revision == Some(previous.revision)
                            && previous.revision.checked_add(1) == Some(worker.revision)
                            && previous.spec_hash == worker.spec_hash
                            && previous.attempt.attempt_id == worker.attempt.attempt_id
                            && previous.request_id == worker.request_id
                            && previous.admission_operation_id == worker.admission_operation_id
                            && previous.policy_hash == worker.policy_hash
                            && serde_json::to_value(&previous.receipt)?
                                == serde_json::to_value(&worker.receipt)?
                            && previous.attempt.stable_name == worker.attempt.stable_name
                            && previous.attempt.volume_name == worker.attempt.volume_name
                            && (!previous.allocation_inconsistent || worker.allocation_inconsistent)
                            && (previous.observed_started_at.is_none()
                                || previous.observed_started_at == worker.observed_started_at)
                            && (previous.attempt.container_id.is_none()
                                || previous.attempt.container_id == worker.attempt.container_id)
                            && (previous.attempt.created_inspection.is_none()
                                || previous.attempt.created_inspection
                                    == worker.attempt.created_inspection)
                            && (previous.attempt.create_operation_id.is_none()
                                || previous.attempt.create_operation_id
                                    == worker.attempt.create_operation_id)
                            && (previous.attempt.start_emitter_id.is_none()
                                || previous.attempt.start_emitter_id
                                    == worker.attempt.start_emitter_id)
                            && (previous.attempt.start_operation_id.is_none()
                                || previous.attempt.start_operation_id
                                    == worker.attempt.start_operation_id),
                },
                "invalid control revision or immutable identity"
            );
            if let Some(previous) = previous {
                worker_transition::replay_record(previous, &record)?;
            } else {
                anyhow::ensure!(
                    worker.state == WorkerStateV1::Reserved
                        && worker.quota_held
                        && worker.admission_operation_id == record.operation_id
                        && worker.receipt.version == 1
                        && worker.receipt.request_id == worker.request_id
                        && worker.receipt.audit_seq > 0
                        && worker.attempt.container_id.is_none()
                        && worker.attempt.created_inspection.is_none()
                        && worker.attempt.create_operation_id.is_none()
                        && worker.attempt.start_operation_id.is_none()
                        && worker.attempt.start_emitter_id.is_none()
                        && worker.observed_started_at.is_none()
                        && !worker.allocation_inconsistent
                        && worker.last_observation.is_none()
                        && worker.unknown_reason.is_none()
                        && worker.cleanup_removed.is_none()
                        && worker.cleanup_error.is_none()
                        && record.observation.is_none()
                        && record.allocation.is_none()
                        && record.refusal.is_none()
                        && record.admission_policy.is_some(),
                    "invalid initial worker admission"
                );
                uuid(&worker.request_id)?;
                hash(&worker.policy_hash)?;
                hash(&worker.receipt.request_hash)?;
                hash(&worker.receipt.audit_hash)?;
                validate_admission_input(db, worker, &record, stored.seq)?;
            }
            expected
                .get_mut("workers")
                .context("missing table")?
                .insert(worker.worker_id.clone(), worker_row(worker, stored.seq)?);
            expected
                .get_mut("worker_attempts")
                .context("missing table")?
                .insert(worker.attempt.attempt_id.clone(), attempt_row(worker)?);
            if record.observation.is_some() {
                expected
                    .get_mut("worker_observations")
                    .context("missing table")?
                    .insert(
                        record.operation_id.clone(),
                        observation_row(&record, worker, stored.seq)?,
                    );
            }
            if let Some(allocation) = &record.allocation {
                anyhow::ensure!(
                    worker.state == WorkerStateV1::Terminal
                        && !worker.quota_held
                        && !allocation.billable,
                    "invalid terminal allocation"
                );
                let observation = record
                    .observation
                    .as_ref()
                    .context("allocation observation missing")?;
                let ObservationClass::Terminal(started, finished, duration) =
                    classify_observation(worker, observation)
                else {
                    anyhow::bail!("allocation has no complete terminal observation");
                };
                anyhow::ensure!(
                    allocation.method_version == 1
                        && allocation.attempt_id == worker.attempt.attempt_id
                        && allocation.worker_id == worker.worker_id
                        && allocation.terminal_observation_id == record.operation_id
                        && allocation.source == allocation_source(observation)
                        && allocation.started_at == started
                        && allocation.finished_at == finished
                        && allocation.duration_ns == duration.to_string()
                        && allocation.cpu_millicore_nanoseconds
                            == duration
                                .checked_mul(u128::from(worker.launch.cpu_millis))
                                .context("allocation overflow")?
                                .to_string()
                        && allocation.memory_byte_nanoseconds
                            == duration
                                .checked_mul(u128::from(worker.launch.memory_bytes))
                                .context("allocation overflow")?
                                .to_string(),
                    "allocation differs from runtime interval"
                );
                let rows = expected
                    .get_mut("worker_allocations")
                    .context("missing table")?;
                anyhow::ensure!(
                    rows.insert(
                        allocation.attempt_id.clone(),
                        allocation_row(allocation, stored.seq)?
                    )
                    .is_none(),
                    "duplicate allocation"
                );
            }
            workers.insert(worker.worker_id.clone(), worker.clone());
        } else {
            anyhow::ensure!(
                record.action == "admit"
                    && record.refusal.is_some()
                    && record.previous_revision.is_none()
                    && record.snapshot_hash.is_none()
                    && record.observation.is_none()
                    && record.allocation.is_none()
                    && record.admission_policy.is_some(),
                "invalid refused admission"
            );
        }
    }
    overlay.finish(db)?;
    super::identity::validate(db)?;
    for (table, query) in TABLE_QUERIES {
        let mut statement = db.prepare(query)?;
        let count = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..count)
                    .map(|i| row.get::<_, rusqlite::types::Value>(i))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let expected_rows = expected
            .remove(table)
            .context("missing reconstruction")?
            .into_values()
            .collect::<Vec<_>>();
        anyhow::ensure!(
            rows == expected_rows,
            "control projection differs from audit: {table}"
        );
    }
    // These readers use indexed historical worker projections, so validate
    // every worker/mutation column above before accepting dependent evidence.
    super::workload::validate(db)?;
    super::broker_registry::validate(db)?;
    super::relay::validate(db)?;
    super::relay_admission::validate(db)?;
    super::kubernetes_admission::validate(db)?;
    super::kubernetes_session::validate(db)?;
    super::broker_checkpoint_migration::validate(db)?;
    if matches!(version, 14 | 15) {
        super::review_format14::validate(db)?;
    }
    super::egress::validate(db)?;
    Ok(())
}

pub(in crate::audit) fn worker_at(
    db: &Connection,
    worker_id: &str,
    before_seq: i64,
) -> anyhow::Result<Option<WorkerV1>> {
    uuid(worker_id)?;
    invalid(before_seq > 0)?;
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    // Descriptive replay also needs the inherited worker history in14. This
    // private projection grants no access through the legacy Store methods.
    invalid([6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version))?;
    worker_snapshot_before(db, worker_id, before_seq)
}

fn worker_snapshot_before(
    db: &Connection,
    worker_id: &str,
    before_seq: i64,
) -> anyhow::Result<Option<WorkerV1>> {
    // The partial compound index excludes helper/Arm/export records, which
    // cannot alter WorkerV1. This seeks one snapshot without scanning them.
    let query = format!(
        "SELECT e.seq,e.received_at,e.prev_hash,e.hash,e.payload FROM worker_mutations m JOIN events e ON e.seq=m.event_seq WHERE m.worker_id=?1 AND m.event_seq<?2 AND {WORKER_SNAPSHOT_PREDICATE} ORDER BY m.event_seq DESC LIMIT 1"
    );
    let stored = db
        .query_row(&query, params![worker_id, before_seq], row_event)
        .optional()?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    let worker = if stored.event.kind == "control.worker.transition" {
        serde_json::from_value::<ControlRecord>(stored.event.data)?.worker
    } else {
        supervision::worker_snapshot(&stored)?
    };
    let worker = worker.context("indexed worker mutation has no snapshot")?;
    anyhow::ensure!(
        worker.worker_id == worker_id,
        "historical worker snapshot identity differs"
    );
    Ok(Some(worker))
}

fn require_control(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if [2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13].contains(&version) {
        Ok(())
    } else {
        Err(ControlFailure::MigrationRequired.into())
    }
}
fn require_v2_only(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version == 2 {
        Ok(())
    } else {
        Err(ControlFailure::MigrationRequired.into())
    }
}
fn load_worker(db: &Connection, id: &str) -> anyhow::Result<Option<WorkerV1>> {
    let payload: Option<String> = db
        .query_row(
            "SELECT snapshot_json FROM workers WHERE worker_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    payload
        .map(|payload| serde_json::from_str(&payload).map_err(Into::into))
        .transpose()
}
fn load_operation(
    db: &Connection,
    id: &str,
) -> anyhow::Result<Option<(StoredEvent, ControlRecord)>> {
    let stored=db.query_row("SELECT e.seq,e.received_at,e.prev_hash,e.hash,e.payload FROM worker_mutations m JOIN events e ON e.seq=m.event_seq WHERE m.operation_id=?1",[id],row_event).optional()?;
    stored
        .map(|stored: StoredEvent| {
            conflict(stored.event.kind == "control.worker.transition")?;
            let record = serde_json::from_value(stored.event.data.clone())?;
            Ok((stored, record))
        })
        .transpose()
}
pub(super) fn quota_usage(db: &Connection, policy: &QuotaPolicyV1) -> anyhow::Result<QuotaUsageV1> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    // This reader is a projection, not a business-access gate. Its callers
    // retain their own checked transaction and exact legacy schema guards.
    invalid([2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version))?;
    let query = if [11, 12, 13, 14, 15].contains(&version) {
        "SELECT count(*),COALESCE(SUM(cpu_millis),0),COALESCE(SUM(memory_bytes),0) FROM (SELECT cpu_millis,memory_bytes FROM workers WHERE quota_held=1 UNION ALL SELECT cpu_millis,memory_bytes FROM kubernetes_creates WHERE quota_held=1 UNION ALL SELECT cpu_millis,memory_bytes FROM kubernetes_session_reservations WHERE quota_held=1)"
    } else if [8, 9, 10].contains(&version) {
        "SELECT count(*),COALESCE(SUM(cpu_millis),0),COALESCE(SUM(memory_bytes),0) FROM (SELECT cpu_millis,memory_bytes FROM workers WHERE quota_held=1 UNION ALL SELECT cpu_millis,memory_bytes FROM kubernetes_creates WHERE quota_held=1)"
    } else {
        "SELECT count(*),COALESCE(SUM(cpu_millis),0),COALESCE(SUM(memory_bytes),0) FROM workers WHERE quota_held=1"
    };
    let (count, cpu, ram): (i64, i64, i64) =
        db.query_row(query, [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(QuotaUsageV1 {
        policy: policy.clone(),
        held_workers: u32::try_from(count)?,
        held_cpu_millis: u64::try_from(cpu)?,
        held_memory_bytes: u64::try_from(ram)?,
    })
}

/// Pure historical projection: each source reconstructs retained states before
/// the operation. None calls its complete quota validator, so worker/CREATE/V2
/// validators may all use this helper without recursing into one another.
pub(super) fn quota_usage_before(
    db: &Connection,
    before_seq: i64,
    policy: &QuotaPolicyV1,
) -> anyhow::Result<QuotaUsageV1> {
    policy.validate()?;
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    invalid([2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15].contains(&version))?;
    let (head, _) = audit_head(db)?;
    invalid(before_seq > 0 && head.checked_add(1).is_some_and(|end| before_seq <= end))?;
    let mut usage = QuotaUsageV1 {
        policy: policy.clone(),
        held_workers: 0,
        held_cpu_millis: 0,
        held_memory_bytes: 0,
    };
    let mut add = |count: u64, cpu: u64, memory: u64| -> anyhow::Result<()> {
        usage.held_workers = usage
            .held_workers
            .checked_add(u32::try_from(count)?)
            .context("historical shared worker quota count overflow")?;
        usage.held_cpu_millis = usage
            .held_cpu_millis
            .checked_add(cpu)
            .context("historical shared worker quota CPU overflow")?;
        usage.held_memory_bytes = usage
            .held_memory_bytes
            .checked_add(memory)
            .context("historical shared worker quota memory overflow")?;
        Ok(())
    };
    let mut workers = db.prepare(
        "SELECT DISTINCT worker_id FROM worker_mutations WHERE worker_id IS NOT NULL AND event_seq<?1 ORDER BY worker_id",
    )?;
    for id in workers.query_map([before_seq], |row| row.get::<_, String>(0))? {
        let id = id?;
        uuid(&id)?;
        // The private reader also handles legacy schemas2–5. The existing
        // worker_at API retains its closed schema6..14 contract and index query.
        if let Some(worker) = worker_snapshot_before(db, &id, before_seq)? {
            worker.launch.validate()?;
            if worker.quota_held {
                add(
                    1,
                    u64::from(worker.launch.cpu_millis),
                    worker.launch.memory_bytes,
                )?;
            }
        }
    }
    let (count, cpu, memory) = super::kubernetes_admission::held_usage_before(db, before_seq)?;
    add(count, cpu, memory)?;
    // This source contributes zero before schema11/its genesis, independently
    // of a reservation's final closed projection.
    let (count, cpu, memory) = super::kubernetes_session::held_usage_before(db, before_seq)?;
    add(count, cpu, memory)?;
    Ok(usage)
}
fn execute_row(tx: &Connection, sql: &str, row: Vec<rusqlite::types::Value>) -> anyhow::Result<()> {
    anyhow::ensure!(
        tx.execute(sql, rusqlite::params_from_iter(row))? == 1,
        "control projection write failed"
    );
    Ok(())
}
fn verify_projection_row(
    db: &Connection,
    table: &str,
    expected: Vec<rusqlite::types::Value>,
) -> anyhow::Result<()> {
    let query = TABLE_QUERIES
        .iter()
        .find(|(name, _)| *name == table)
        .context("unknown control projection")?
        .1;
    let select = query
        .split_once(" ORDER BY ")
        .context("projection order missing")?
        .0;
    let key = match table {
        "workers" => "worker_id",
        "worker_attempts" | "worker_allocations" => "attempt_id",
        "worker_observations" => "observation_id",
        "worker_mutations" => "operation_id",
        _ => anyhow::bail!("unsupported control projection write"),
    };
    let actual = db.query_row(
        &format!("{select} WHERE {key}=?1"),
        [expected.first().context("empty projection row")?],
        |row| {
            (0..expected.len())
                .map(|column| row.get::<_, rusqlite::types::Value>(column))
                .collect::<rusqlite::Result<Vec<_>>>()
        },
    )?;
    anyhow::ensure!(
        actual == expected,
        "control projection write did not persist its exact row: {table}"
    );
    Ok(())
}
impl Store {
    /// Called only after fresh policy/Jev evaluation outside the database lock.
    /// The historical receipt is a link; deterministic policy is re-evaluated here.
    pub fn admit_worker(
        &self,
        admission: &AdmissionV1,
        policy: &policy::Policy,
        fresh_decision: Decision,
        jev: Value,
        quota: &QuotaPolicyV1,
    ) -> anyhow::Result<AdmissionResultV1> {
        let input_hash = worker_admission::validate_input(admission, quota, &self.redactor)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_control(&tx)?;
        if let Some((stored, old)) = load_operation(&tx, &admission.operation_id)? {
            conflict(old.input_hash == input_hash && old.action == "admit")?;
            self.commit_checked(tx)?;
            let mut result = admission_result(&stored, &old);
            result.replayed = true;
            return Ok(result);
        }
        require_v2_only(&tx)?;
        let record =
            self.build_admission_record(&tx, admission, policy, fresh_decision, jev, quota)?;
        let stored = self.persist_control_record(&tx, &record, true)?;
        self.commit_checked(tx)?;
        Ok(admission_result(&stored, &record))
    }
    fn build_admission_record(
        &self,
        tx: &Transaction<'_>,
        admission: &AdmissionV1,
        policy: &policy::Policy,
        fresh_decision: Decision,
        jev: Value,
        quota: &QuotaPolicyV1,
    ) -> anyhow::Result<ControlRecord> {
        let request = &admission.decision_request;
        let stored = Self::lookup_decision(tx, request, &admission.receipt.request_hash)?
            .ok_or(ControlFailure::InvalidRequest)?;
        worker_admission::require_receipt(&admission.receipt, &stored)?;
        let existing: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM workers WHERE request_id=?1 OR session_id=?2)",
            params![request.request_id, admission.launch.session_id],
            |r| r.get(0),
        )?;
        conflict(!existing)?;
        let schema: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if [8, 9, 10, 11, 12, 13].contains(&schema) {
            let reserved: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM kubernetes_creates WHERE session_id=?1)",
                [&admission.launch.session_id],
                |row| row.get(0),
            )?;
            conflict(!reserved)?;
        }
        if [11, 12, 13].contains(&schema) {
            let reserved: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM kubernetes_session_reservations WHERE session_id=?1)",
                [&admission.launch.session_id],
                |row| row.get(0),
            )?;
            conflict(!reserved)?;
        }
        let usage = quota_usage(tx, quota)?;
        worker_admission::build_record(admission, policy, fresh_decision, jev, &usage, || {
            let namespace: String = tx.query_row(
                "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
                [],
                |r| r.get(0),
            )?;
            let worker_id = uuid::Uuid::new_v4().to_string();
            let attempt_id = uuid::Uuid::new_v4().to_string();
            if [11, 12, 13].contains(&schema) {
                let duplicate: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM kubernetes_session_reservations WHERE worker_id=?1 OR attempt_id=?2)",
                    params![worker_id, attempt_id], |row| row.get(0))?;
                conflict(!duplicate)?;
            }
            Ok((namespace, worker_id, attempt_id))
        })
    }
    fn persist_control_record(
        &self,
        tx: &Transaction<'_>,
        record: &ControlRecord,
        admission: bool,
    ) -> anyhow::Result<StoredEvent> {
        let mut event = Event::new("control.worker.transition");
        event.data = serde_json::to_value(record)?;
        if let Some(worker) = &record.worker {
            event.session_id = Some(worker.launch.session_id.clone());
        }
        let (prepared, payload) = self.prepare_event(event.clone())?;
        // The masking policy must not silently change identities or snapshots.
        // Explanatory policy/Jev evidence is allowed to be masked separately.
        let mut expected = serde_json::to_value(record)?;
        if let Some(policy) = expected.get_mut("admission_policy") {
            *policy = self.redactor.value(policy);
        }
        invalid(prepared.data == expected)?;
        let stored = self.append_in_transaction(tx, prepared, payload)?;
        self.write_control_projection(
            tx,
            record,
            admission,
            &stored,
            mutation_row(record, stored.seq),
        )?;
        Ok(stored)
    }
    fn write_control_projection(
        &self,
        tx: &Transaction<'_>,
        record: &ControlRecord,
        admission: bool,
        stored: &StoredEvent,
        mutation: Vec<rusqlite::types::Value>,
    ) -> anyhow::Result<()> {
        if let Some(worker) = &record.worker {
            if admission {
                execute_row(
                    tx,
                    "INSERT INTO workers(worker_id,admission_operation_id,request_id,session_id,state,quota_held,cpu_millis,memory_bytes,revision,snapshot_json,snapshot_hash,last_event_seq) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                    worker_row(worker, stored.seq)?,
                )?;
                execute_row(
                    tx,
                    "INSERT INTO worker_attempts(attempt_id,worker_id,authority_id,stable_name,volume_name,container_id,create_operation_id,start_operation_id,start_emitter_id,snapshot_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    attempt_row(worker)?,
                )?;
            } else {
                let mut row = worker_row(worker, stored.seq)?;
                row.push(integer(
                    record
                        .previous_revision
                        .context("missing previous revision")?,
                )?);
                execute_row(
                    tx,
                    "UPDATE workers SET admission_operation_id=?2,request_id=?3,session_id=?4,state=?5,quota_held=?6,cpu_millis=?7,memory_bytes=?8,revision=?9,snapshot_json=?10,snapshot_hash=?11,last_event_seq=?12 WHERE worker_id=?1 AND revision=?13",
                    row,
                )?;
                execute_row(
                    tx,
                    "UPDATE worker_attempts SET worker_id=?2,authority_id=?3,stable_name=?4,volume_name=?5,container_id=?6,create_operation_id=?7,start_operation_id=?8,start_emitter_id=?9,snapshot_json=?10 WHERE attempt_id=?1",
                    attempt_row(worker)?,
                )?;
            }
            if record.observation.is_some() {
                execute_row(
                    tx,
                    "INSERT INTO worker_observations(observation_id,worker_id,attempt_id,input_hash,payload_json,event_seq) VALUES(?1,?2,?3,?4,?5,?6)",
                    observation_row(record, worker, stored.seq)?,
                )?;
            }
            if let Some(allocation) = &record.allocation {
                execute_row(
                    tx,
                    "INSERT INTO worker_allocations(attempt_id,worker_id,terminal_observation_id,payload_json,event_seq) VALUES(?1,?2,?3,?4,?5)",
                    allocation_row(allocation, stored.seq)?,
                )?;
            }
        }
        execute_row(
            tx,
            "INSERT INTO worker_mutations(operation_id,worker_id,action,input_hash,event_seq) VALUES(?1,?2,?3,?4,?5)",
            mutation.clone(),
        )?;
        let verified = (|| -> anyhow::Result<()> {
            if let Some(worker) = &record.worker {
                verify_projection_row(tx, "workers", worker_row(worker, stored.seq)?)?;
                verify_projection_row(tx, "worker_attempts", attempt_row(worker)?)?;
                if record.observation.is_some() {
                    verify_projection_row(
                        tx,
                        "worker_observations",
                        observation_row(record, worker, stored.seq)?,
                    )?;
                }
                if let Some(allocation) = &record.allocation {
                    verify_projection_row(
                        tx,
                        "worker_allocations",
                        allocation_row(allocation, stored.seq)?,
                    )?;
                }
            }
            verify_projection_row(tx, "worker_mutations", mutation.clone())?;
            Ok(())
        })();
        if let Err(error) = verified {
            self.unhealthy.store(true, Ordering::Release);
            return Err(error);
        }
        Ok(())
    }
    pub fn worker_operation(&self, id: &str) -> anyhow::Result<Option<MutationReceiptV1>> {
        uuid(id)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_control(&tx)?;
        let result = supervision::operation_receipt(&tx, id)?;
        self.commit_checked(tx)?;
        Ok(result)
    }
    pub fn worker(&self, id: &str) -> anyhow::Result<Option<WorkerV1>> {
        uuid(id)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_control(&tx)?;
        let result = load_worker(&tx, id)?;
        self.commit_checked(tx)?;
        Ok(result)
    }
    pub fn workers(&self) -> anyhow::Result<Vec<WorkerV1>> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_control(&tx)?;
        let result = {
            let mut stmt = tx.prepare("SELECT snapshot_json FROM workers ORDER BY worker_id")?;
            let payloads = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            payloads
                .iter()
                .map(|p| serde_json::from_str(p).map_err(Into::into))
                .collect::<anyhow::Result<Vec<_>>>()?
        };
        self.commit_checked(tx)?;
        Ok(result)
    }
    pub fn workers_page(&self, cursor: Option<&str>, limit: u32) -> anyhow::Result<Vec<WorkerV1>> {
        invalid((1..=100).contains(&limit))?;
        if let Some(cursor) = cursor {
            uuid(cursor)?;
        }
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_control(&tx)?;
        let result = {
            let mut stmt = tx.prepare(
                "SELECT snapshot_json FROM workers WHERE worker_id>?1 ORDER BY worker_id LIMIT ?2",
            )?;
            let payloads = stmt
                .query_map(params![cursor.unwrap_or(""), limit], |r| {
                    r.get::<_, String>(0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            payloads
                .iter()
                .map(|p| serde_json::from_str(p).map_err(Into::into))
                .collect::<anyhow::Result<Vec<_>>>()?
        };
        self.commit_checked(tx)?;
        Ok(result)
    }
    pub fn worker_quota(&self, policy: &QuotaPolicyV1) -> anyhow::Result<QuotaUsageV1> {
        policy.validate()?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_control(&tx)?;
        let result = quota_usage(&tx, policy)?;
        self.commit_checked(tx)?;
        Ok(result)
    }
    pub fn worker_usage(&self) -> anyhow::Result<Vec<AllocationV1>> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_control(&tx)?;
        let result = {
            let mut stmt =
                tx.prepare("SELECT payload_json FROM worker_allocations ORDER BY attempt_id")?;
            let payloads = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            payloads
                .iter()
                .map(|p| serde_json::from_str(p).map_err(Into::into))
                .collect::<anyhow::Result<Vec<_>>>()?
        };
        self.commit_checked(tx)?;
        Ok(result)
    }
    pub fn worker_usage_page(
        &self,
        cursor: Option<&str>,
        limit: u32,
    ) -> anyhow::Result<Vec<AllocationV1>> {
        invalid((1..=100).contains(&limit))?;
        if let Some(cursor) = cursor {
            uuid(cursor)?;
        }
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_control(&tx)?;
        let result = {
            let mut stmt=tx.prepare("SELECT payload_json FROM worker_allocations WHERE attempt_id>?1 ORDER BY attempt_id LIMIT ?2")?;
            let payloads = stmt
                .query_map(params![cursor.unwrap_or(""), limit], |r| {
                    r.get::<_, String>(0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            payloads
                .iter()
                .map(|p| serde_json::from_str(p).map_err(Into::into))
                .collect::<anyhow::Result<Vec<_>>>()?
        };
        self.commit_checked(tx)?;
        Ok(result)
    }
    /// Explicit maintenance only: caller must stop legacy brokers and gateway.
    /// The backup preserves v1; no restore or authority re-enablement is implied.
    pub fn migrate_control_v2(&self, backup_path: &Path) -> anyhow::Result<MigrationReportV1> {
        self.migrate_control_version(backup_path, 1, 2)
    }
    /// Explicit maintenance only; no runtime activation or restore fencing is implied.
    pub fn migrate_control_v3(&self, backup_path: &Path) -> anyhow::Result<MigrationReportV1> {
        self.migrate_control_version(backup_path, 2, 3)
    }
    /// Offline maintenance only; the verified schema5 backup is retained.
    /// Migration preserves an existing live origin but never activates one.
    pub fn migrate_workload_v6(&self, backup_path: &Path) -> anyhow::Result<MigrationReportV1> {
        self.migrate_control_version(backup_path, 5, 6)
    }
    /// Explicit offline maintenance; preserves schema6 and never activates supervision.
    pub fn migrate_human_brokers_v7(
        &self,
        backup_path: &Path,
    ) -> anyhow::Result<MigrationReportV1> {
        self.migrate_control_version(backup_path, 6, 7)
    }
    /// Explicit backed-up offline9→10 maintenance; never activates supervision.
    pub fn migrate_human_workers_v10(
        &self,
        backup_path: &Path,
    ) -> anyhow::Result<MigrationReportV1> {
        self.migrate_control_version(backup_path, 9, 10)
    }
    /// Explicit backed-up offline maintenance appending the human rights
    /// marker on a store that already carries the worker-access marker. The
    /// store format (`user_version`) is unchanged; never activates supervision.
    pub fn migrate_human_rights_v2(&self, backup_path: &Path) -> anyhow::Result<MigrationReportV1> {
        self.backed_up_maintenance(backup_path, Maintenance::HumanRights)
    }
    pub(super) fn migrate_control_version(
        &self,
        backup_path: &Path,
        from_version: u32,
        to_version: u32,
    ) -> anyhow::Result<MigrationReportV1> {
        // The recovery format has one explicit semantic edge. An internal
        // caller cannot skip the checkpoint marker and history of schema12.
        invalid(to_version != 13 || from_version == 12)?;
        self.backed_up_maintenance(
            backup_path,
            Maintenance::Version {
                from_version,
                to_version,
            },
        )
    }
    fn backed_up_maintenance(
        &self,
        backup_path: &Path,
        step: Maintenance,
    ) -> anyhow::Result<MigrationReportV1> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let preflight = self.begin_checked(&mut db)?;
        let version: i64 = preflight.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let (from_version, to_version) = match step {
            Maintenance::Version {
                from_version,
                to_version,
            } => {
                invalid(version == i64::from(from_version))?;
                (from_version, to_version)
            }
            Maintenance::HumanRights => {
                invalid(
                    super::human_rights::format_admits_marker(version)
                        && !super::human_rights::installed(&preflight)?,
                )?;
                let version = u32::try_from(version)?;
                (version, version)
            }
        };
        validate_schema(&preflight, false)?;
        Self::validate_metadata(&preflight, &self.tenant_id)?;
        let expected = Self::check_integrity(&preflight)?;
        anyhow::ensure!(expected.valid, "cannot migrate corrupt audit");
        self.commit_checked(preflight)?;
        let (backup_backing, initialization_guard) = storage::StoreStorage::create_new(backup_path)
            .context("create new migration backup destination")?;
        let target = backup_backing.path().to_path_buf();
        self.backing.verify(&db)?;
        let mut destination = Connection::open_with_flags(
            &target,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        backup_backing.verify(&destination)?;
        destination.busy_timeout(std::time::Duration::from_secs(1))?;
        destination.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA synchronous=FULL;")?;
        {
            let backup = rusqlite::backup::Backup::new(&db, &mut destination)?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                // Backup borrows destination mutably. Filesystem checks here
                // never open/close main or SHM descriptors held by SQLite;
                // HAS_MOVED runs on destination before and after this borrow.
                backup_backing.verify_names()?;
                self.backing.verify(&db)?;
                if backup.step(256)? == rusqlite::backup::StepResult::Done {
                    break;
                }
                anyhow::ensure!(
                    std::time::Instant::now() < deadline,
                    "migration backup deadline exceeded"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        backup_backing.verify(&destination)?;
        self.backing.verify(&db)?;
        validate_schema(&destination, false)?;
        Self::validate_metadata(&destination, &self.tenant_id)?;
        let backed = Self::check_integrity(&destination)?;
        anyhow::ensure!(
            backed.valid
                && backed.event_count == expected.event_count
                && backed.head_hash == expected.head_hash,
            "migration backup does not match preflight audit"
        );
        let quick: String = destination.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        anyhow::ensure!(quick == "ok", "migration backup SQLite integrity failure");
        backup_backing.verify(&destination)?;
        destination.close().map_err(|(_, error)| error)?;
        // The destination is fresh and its SQLite connection is now closed.
        // The initialization guard remains held through synchronization.
        backup_backing.sync_closed(&initialization_guard)?;
        // A separate connection may have appended while the backup was copied.
        // Recheck the exact backed head before any DDL; never migrate past it.
        let mut tx = self.begin_checked(&mut db)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        conflict(version == i64::from(from_version))?;
        validate_schema(&tx, false)?;
        let current = Self::check_integrity(&tx)?;
        conflict(
            current.valid
                && current.event_count == expected.event_count
                && current.head_hash == expected.head_hash,
        )?;
        if step == Maintenance::HumanRights {
            conflict(!super::human_rights::installed(&tx)?)?;
            self.initialize_human_rights(&tx)?;
            validate_schema(&tx, false)?;
            validate(&tx)?;
            Self::validate_metadata(&tx, &self.tenant_id)?;
            anyhow::ensure!(
                Self::check_integrity(&tx)?.valid,
                "post-maintenance audit integrity failure"
            );
            let (seq, hash): (i64, String) = tx.query_row(
                "SELECT seq,hash FROM events WHERE kind=?1",
                [super::human_rights::GENESIS_KIND],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            self.commit_checked(tx)?;
            return Ok(MigrationReportV1 {
                from_version,
                to_version,
                backup_path: target.to_string_lossy().into(),
                genesis_seq: seq,
                genesis_hash: hash,
            });
        }
        let activate_supervision = to_version == 3 && tx.lease.untouched_fresh_origin(&tx)?;
        match to_version {
            2 => initialize(&tx, self.max_events, &self.redactor)?,
            3 => self.initialize_supervision(&tx)?,
            4 => self.initialize_identity(&tx)?,
            5 => self.initialize_human_access(&tx)?,
            6 => {
                super::workload::initialize(self, &tx)?;
                super::broker_registry::initialize(self, &tx)?;
                super::relay::initialize(&tx)?;
                for (_, _, sql) in WORKLOAD_INDEX_SCHEMA {
                    tx.execute_batch(sql)?;
                }
            }
            7 => self.initialize_human_brokers(&tx)?,
            8 => super::kubernetes_admission::initialize(self, &tx)?,
            9 => super::kubernetes_admission::initialize_policy(self, &tx)?,
            10 => self.initialize_human_workers(&tx)?,
            11 => super::kubernetes_session::initialize(self, &tx)?,
            12 => super::broker_checkpoint_migration::initialize(self, &tx)?,
            13 => super::kubernetes_admission::initialize_recovery(self, &tx)?,
            _ => return Err(ControlFailure::InvalidRequest.into()),
        }
        tx.pragma_update(None, "user_version", to_version)?;
        validate_schema(&tx, false)?;
        validate(&tx)?;
        Self::validate_metadata(&tx, &self.tenant_id)?;
        anyhow::ensure!(
            Self::check_integrity(&tx)?.valid,
            "post-migration audit integrity failure"
        );
        let (seq, hash): (i64, String) = tx.query_row(
            match to_version {
                2 => "SELECT genesis_seq,genesis_hash FROM control_meta WHERE singleton=1",
                3 => "SELECT genesis_seq,genesis_hash FROM supervision_meta WHERE singleton=1",
                4 => "SELECT genesis_seq,genesis_hash FROM identity_meta WHERE singleton=1",
                5 => {
                    "SELECT seq,hash FROM events WHERE kind='control.identity.human_access.genesis'"
                }
                6 => "SELECT genesis_seq,genesis_hash FROM workload_meta WHERE singleton=1",
                7 => "SELECT seq,hash FROM events WHERE kind='control.identity.brokers_access.genesis'",
                8 => "SELECT seq,hash FROM events WHERE kind='control.kubernetes.create.genesis'",
                9 => "SELECT seq,hash FROM events WHERE kind='control.kubernetes.create.policy.genesis'",
                10 => "SELECT seq,hash FROM events WHERE kind='control.identity.workers_access.genesis'",
                11 => "SELECT genesis_seq,genesis_hash FROM kubernetes_session_reservation_meta WHERE singleton=1",
                12 => "SELECT seq,hash FROM events WHERE kind='control.broker_checkpoints.genesis'",
                13 => "SELECT seq,hash FROM events WHERE kind='control.kubernetes.create.recovery.genesis'",
                _ => return Err(ControlFailure::InvalidRequest.into()),
            },
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        tx.activate_supervision = activate_supervision;
        self.commit_checked(tx)?;
        Ok(MigrationReportV1 {
            from_version,
            to_version,
            backup_path: target.to_string_lossy().into(),
            genesis_seq: seq,
            genesis_hash: hash,
        })
    }
    pub fn mutate_worker(&self, mutation: &WorkerMutationV1) -> anyhow::Result<MutationResultV1> {
        let input_hash = worker_transition::validate_input(mutation, &self.redactor)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_control(&tx)?;
        if let Some((stored, record)) = load_operation(&tx, &mutation.operation_id)? {
            let result = worker_transition::replay(mutation, &input_hash, &record, &stored)?;
            self.commit_checked(tx)?;
            return Ok(result);
        }
        let worker = load_worker(&tx, &mutation.worker_id)?.ok_or(ControlFailure::NotFound)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13].contains(&version) {
            if !tx.lease.active
                && !matches!(
                    &mutation.transition,
                    WorkerTransitionV1::Observe { .. }
                        | WorkerTransitionV1::Unknown { .. }
                        | WorkerTransitionV1::Stop
                        | WorkerTransitionV1::CleanupResult { .. }
                )
            {
                return Err(ControlFailure::SupervisionInactive.into());
            }
            conflict(!matches!(
                &mutation.transition,
                WorkerTransitionV1::StartClaim { .. }
            ))?;
            if matches!(&mutation.transition, WorkerTransitionV1::CreateIssued) {
                supervision::can_create(&tx, &worker)?;
            }
        }
        let record = worker_transition::build_record(worker, mutation, input_hash)?;
        let stored = self.persist_control_record(&tx, &record, false)?;
        self.commit_checked(tx)?;
        worker_transition::committed_result(&stored, &record)
    }
}

fn validate_admission_input(
    db: &Connection,
    worker: &WorkerV1,
    record: &ControlRecord,
    admission_seq: i64,
) -> anyhow::Result<()> {
    let source = db
        .query_row(
            "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE seq=?1",
            [worker.receipt.audit_seq],
            row_event,
        )
        .optional()?
        .context("admission receipt event is missing")?;
    let admission = worker_admission::historical_admission(worker, record, &source, admission_seq)?;
    let linked = Store::lookup_decision(
        db,
        &admission.decision_request,
        &worker.receipt.request_hash,
    )?
    .context("admission receipt decision is missing")?;
    anyhow::ensure!(
        linked.seq == source.seq && linked.hash == source.hash,
        "admission input differs from its linked decision or journal"
    );
    let policy = record
        .admission_policy
        .as_ref()
        .and_then(|policy| policy.get("quota"))
        .context("historical worker admission quota policy is missing")?;
    let quota: QuotaPolicyV1 = serde_json::from_value(policy.clone())
        .context("historical worker admission quota policy is invalid")?;
    quota.validate()?;
    let usage = quota_usage_before(db, admission_seq, &quota)?;
    anyhow::ensure!(
        worker_admission::fits_quota(&worker.launch, &usage),
        "worker admission exceeds historical shared tenant quota"
    );
    Ok(())
}

/// One backed-up offline maintenance step. A version step changes the store
/// format; the human rights marker never does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Maintenance {
    Version { from_version: u32, to_version: u32 },
    HumanRights,
}
