use super::*;
use crate::control::*;

#[path = "supervision.rs"]
pub(super) mod supervision;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlRecord {
    version: u32,
    operation_id: String,
    action: String,
    input_hash: String,
    previous_revision: Option<u64>,
    worker: Option<WorkerV1>,
    snapshot_hash: Option<String>,
    observation: Option<RuntimeObservationV1>,
    allocation: Option<AllocationV1>,
    refusal: Option<AdmissionRefusalV1>,
    admission_policy: Option<Value>,
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
    anyhow::ensure!(version == 2 || version == 3, "unsupported control schema");
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
                replay_worker_transition(previous, worker, &record)?;
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
    Ok(())
}

fn require_control(db: &Connection) -> anyhow::Result<()> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version == 2 || version == 3 {
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
fn receipt(stored: &StoredEvent, record: &ControlRecord) -> MutationReceiptV1 {
    MutationReceiptV1 {
        operation_id: record.operation_id.clone(),
        worker_id: record.worker.as_ref().map(|w| w.worker_id.clone()),
        revision: record.worker.as_ref().map(|w| w.revision),
        input_hash: record.input_hash.clone(),
        audit_seq: stored.seq,
        audit_hash: stored.hash.clone(),
    }
}
fn admission_result(stored: &StoredEvent, record: &ControlRecord) -> AdmissionResultV1 {
    AdmissionResultV1 {
        replayed: false,
        accepted: record.worker.is_some(),
        refusal: record.refusal,
        worker: record.worker.clone(),
        receipt: receipt(stored, record),
    }
}
fn quota_usage(db: &Connection, policy: &QuotaPolicyV1) -> anyhow::Result<QuotaUsageV1> {
    let (count,cpu,ram):(i64,i64,i64)=db.query_row("SELECT count(*),COALESCE(SUM(cpu_millis),0),COALESCE(SUM(memory_bytes),0) FROM workers WHERE quota_held=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    Ok(QuotaUsageV1 {
        policy: policy.clone(),
        held_workers: u32::try_from(count)?,
        held_cpu_millis: u64::try_from(cpu)?,
        held_memory_bytes: u64::try_from(ram)?,
    })
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
        admission.validate()?;
        invalid(admission.version == 1 && admission.receipt.version == 1)?;
        uuid(&admission.operation_id)?;
        uuid(&admission.decision_request.request_id)?;
        admission.launch.validate()?;
        quota.validate()?;
        let request = &admission.decision_request;
        invalid(
            admission.receipt.request_id == request.request_id
                && admission.receipt.request_hash == policy::request_hash(request)
                && admission.launch.command == request.command
                && admission.launch.profile == request.profile
                && request.session_id.as_deref() == Some(admission.launch.session_id.as_str())
                && admission.receipt.phase
                    == if request.human_approved {
                        DecisionPhaseV1::Approved
                    } else {
                        DecisionPhaseV1::Initial
                    },
        )?;
        hash(&admission.receipt.request_hash)?;
        hash(&admission.receipt.audit_hash)?;
        invalid(admission.receipt.audit_seq > 0)?;
        let value = serde_json::to_value(admission)?;
        invalid(
            self.redactor.value(&value) == value && serde_json::to_vec(&value)?.len() <= 65536,
        )?;
        let input_hash = admission.input_hash()?;
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
        let input_hash = admission.input_hash()?;
        let stored = Self::lookup_decision(tx, request, &admission.receipt.request_hash)?
            .ok_or(ControlFailure::InvalidRequest)?;
        invalid(
            stored.seq == admission.receipt.audit_seq
                && stored.hash == admission.receipt.audit_hash
                && stored
                    .event
                    .decision
                    .as_ref()
                    .is_some_and(|decision| decision.outcome == Outcome::Allow),
        )?;
        let existing: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM workers WHERE request_id=?1 OR session_id=?2)",
            params![request.request_id, admission.launch.session_id],
            |r| r.get(0),
        )?;
        conflict(!existing)?;
        let current = policy::evaluate(policy, request);
        let usage = quota_usage(tx, quota)?;
        let refusal =
            if current.outcome != Outcome::Allow || fresh_decision.outcome != Outcome::Allow {
                Some(AdmissionRefusalV1::Policy)
            } else if usage.held_workers >= quota.max_active_workers
                || usage.held_cpu_millis + u64::from(admission.launch.cpu_millis)
                    > u64::from(quota.max_cpu_millis)
                || usage.held_memory_bytes + admission.launch.memory_bytes > quota.max_memory_bytes
                || admission.launch.timeout_seconds > quota.max_timeout_seconds
            {
                Some(AdmissionRefusalV1::Quota)
            } else {
                None
            };
        let policy_hash = content_hash("ai-guard:admission-policy:v1", &(policy, quota))?;
        let worker = if refusal.is_none() {
            let namespace: String = tx.query_row(
                "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
                [],
                |r| r.get(0),
            )?;
            let worker_id = uuid::Uuid::new_v4().to_string();
            let attempt_id = uuid::Uuid::new_v4().to_string();
            let physical = format!(
                "aig-{}-{}",
                namespace.replace('-', ""),
                worker_id.replace('-', "")
            );
            Some(WorkerV1 {
                worker_id,
                admission_operation_id: admission.operation_id.clone(),
                request_id: request.request_id.clone(),
                receipt: admission.receipt.clone(),
                launch: admission.launch.clone(),
                spec_hash: admission.launch.spec_hash()?,
                policy_hash: policy_hash.clone(),
                state: WorkerStateV1::Reserved,
                quota_held: true,
                revision: 1,
                attempt: WorkerAttemptV1 {
                    attempt_id,
                    stable_name: physical.clone(),
                    volume_name: format!("{physical}-work"),
                    container_id: None,
                    created_inspection: None,
                    create_operation_id: None,
                    start_operation_id: None,
                    start_emitter_id: None,
                },
                observed_started_at: None,
                allocation_inconsistent: false,
                last_observation: None,
                unknown_reason: None,
                cleanup_removed: None,
                cleanup_error: None,
            })
        } else {
            None
        };
        let record = ControlRecord {
            version: 1,
            operation_id: admission.operation_id.clone(),
            action: "admit".into(),
            input_hash,
            previous_revision: None,
            snapshot_hash: worker
                .as_ref()
                .map(|w| content_hash("ai-guard:worker-snapshot:v1", w))
                .transpose()?,
            worker,
            observation: None,
            allocation: None,
            refusal,
            admission_policy: Some(
                json!({"policy_hash":policy_hash,"deterministic":current,"fresh":fresh_decision,"jev":jev,"quota":quota}),
            ),
        };
        Ok(record)
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
    fn migrate_control_version(
        &self,
        backup_path: &Path,
        from_version: u32,
        to_version: u32,
    ) -> anyhow::Result<MigrationReportV1> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let preflight = self.begin_checked(&mut db)?;
        let version: i64 = preflight.pragma_query_value(None, "user_version", |r| r.get(0))?;
        invalid(version == i64::from(from_version))?;
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
        let activate_supervision = to_version == 3 && tx.lease.untouched_fresh_origin(&tx)?;
        match to_version {
            2 => initialize(&tx, self.max_events, &self.redactor)?,
            3 => self.initialize_supervision(&tx)?,
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
            if to_version == 2 {
                "SELECT genesis_seq,genesis_hash FROM control_meta WHERE singleton=1"
            } else {
                "SELECT genesis_seq,genesis_hash FROM supervision_meta WHERE singleton=1"
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
        invalid(mutation.version == 1 && mutation.expected_revision > 0)?;
        uuid(&mutation.operation_id)?;
        uuid(&mutation.worker_id)?;
        uuid(&mutation.attempt_id)?;
        let value = serde_json::to_value(mutation)?;
        invalid(
            self.redactor.value(&value) == value && serde_json::to_vec(&value)?.len() <= 65536,
        )?;
        let input_hash = content_hash("ai-guard:worker-mutation:v1", mutation)?;
        let action = match &mutation.transition {
            WorkerTransitionV1::Prepare => "prepare",
            WorkerTransitionV1::CreateIssued => "create_issued",
            WorkerTransitionV1::Created { .. } => "created",
            WorkerTransitionV1::StartClaim { .. } => "start_claim",
            WorkerTransitionV1::Observe { .. } => "observe",
            WorkerTransitionV1::Unknown { .. } => "unknown",
            WorkerTransitionV1::Stop => "stop",
            WorkerTransitionV1::CancelReserved => "cancel_reserved",
            WorkerTransitionV1::CleanupResult { .. } => "cleanup_result",
        };
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        require_control(&tx)?;
        if let Some((stored, record)) = load_operation(&tx, &mutation.operation_id)? {
            conflict(record.input_hash == input_hash && record.action == action)?;
            let worker = record.worker.clone().ok_or(ControlFailure::Conflict)?;
            let result = MutationResultV1 {
                worker,
                receipt: receipt(&stored, &record),
                disposition: if action == "start_claim" {
                    Some(ClaimDispositionV1::AlreadyConsumed)
                } else {
                    None
                },
            };
            self.commit_checked(tx)?;
            return Ok(result);
        }
        let mut worker = load_worker(&tx, &mutation.worker_id)?.ok_or(ControlFailure::NotFound)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version == 3 {
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
        conflict(
            worker.attempt.attempt_id == mutation.attempt_id
                && worker.revision == mutation.expected_revision,
        )?;
        let (observation, allocation) = apply_worker_transition(&mut worker, mutation)?;
        let record = ControlRecord {
            version: 1,
            operation_id: mutation.operation_id.clone(),
            action: action.into(),
            input_hash,
            previous_revision: Some(mutation.expected_revision),
            snapshot_hash: Some(content_hash("ai-guard:worker-snapshot:v1", &worker)?),
            worker: Some(worker.clone()),
            observation,
            allocation,
            refusal: None,
            admission_policy: None,
        };
        let stored = self.persist_control_record(&tx, &record, false)?;
        let result = MutationResultV1 {
            worker,
            receipt: receipt(&stored, &record),
            disposition: if action == "start_claim" {
                Some(ClaimDispositionV1::EmitOnce)
            } else {
                None
            },
        };
        self.commit_checked(tx)?;
        Ok(result)
    }
}

/// Derive checkpoint facts from an acknowledged parent without storage or IO.
/// The returned clone is data, never a start or mutation permission.
pub(crate) fn expected_worker_transition(
    parent: &WorkerV1,
    mutation: &WorkerMutationV1,
) -> anyhow::Result<WorkerV1> {
    invalid(mutation.version == 1 && mutation.expected_revision > 0)?;
    uuid(&mutation.operation_id)?;
    uuid(&mutation.worker_id)?;
    uuid(&mutation.attempt_id)?;
    invalid(serde_json::to_vec(mutation)?.len() <= 65536)?;
    conflict(
        mutation.worker_id == parent.worker_id
            && mutation.attempt_id == parent.attempt.attempt_id
            && mutation.expected_revision == parent.revision,
    )?;
    let mut expected = parent.clone();
    apply_worker_transition(&mut expected, mutation)?;
    Ok(expected)
}

/// Deterministic state change shared by live mutations and journal replay.
/// This function performs no database or runtime operation.
fn apply_worker_transition(
    worker: &mut WorkerV1,
    mutation: &WorkerMutationV1,
) -> anyhow::Result<(Option<RuntimeObservationV1>, Option<AllocationV1>)> {
    let mut observation = None;
    let mut allocation = None;
    match &mutation.transition {
        WorkerTransitionV1::Prepare => {
            prepare_worker(worker)?;
        }
        WorkerTransitionV1::CreateIssued => {
            conflict(
                worker.state == WorkerStateV1::Preparing
                    && worker.attempt.create_operation_id.is_none(),
            )?;
            worker.attempt.create_operation_id = Some(mutation.operation_id.clone());
        }
        WorkerTransitionV1::Created { inspection } => {
            record_created_worker(worker, inspection)?;
        }
        WorkerTransitionV1::StartClaim { emitter_id } => {
            uuid(emitter_id)?;
            conflict(
                worker.state == WorkerStateV1::Created
                    && worker.quota_held
                    && worker.attempt.container_id.is_some()
                    && worker.attempt.start_operation_id.is_none(),
            )?;
            worker.attempt.start_operation_id = Some(mutation.operation_id.clone());
            worker.attempt.start_emitter_id = Some(emitter_id.clone());
            worker.state = WorkerStateV1::StartIssued;
        }
        WorkerTransitionV1::Observe {
            observation: incoming,
        } => {
            conflict(
                worker.quota_held
                    && worker.attempt.container_id.is_some()
                    && ![
                        WorkerStateV1::Reserved,
                        WorkerStateV1::Preparing,
                        WorkerStateV1::Terminal,
                        WorkerStateV1::Cancelled,
                    ]
                    .contains(&worker.state),
            )?;
            validate_inspection(&incoming.inspection)?;
            chrono::DateTime::parse_from_rfc3339(&incoming.observed_at)
                .map_err(|_| ControlFailure::InvalidRequest)?;
            for date in [&incoming.started_at, &incoming.finished_at]
                .into_iter()
                .flatten()
            {
                bounded(date, 64)?;
            }
            // Later inspection cannot erase an observed restart or an
            // identity/configuration divergence during this allocation.
            if !inspection_matches(worker, &incoming.inspection, true)
                || incoming.restart_count.is_some_and(|count| count > 0)
                || worker
                    .observed_started_at
                    .as_ref()
                    .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
                    .zip(
                        incoming
                            .started_at
                            .as_ref()
                            .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok()),
                    )
                    .is_some_and(|(first, current)| first != current)
            {
                worker.allocation_inconsistent = true;
            }
            if worker.observed_started_at.is_none()
                && !worker.allocation_inconsistent
                && inspection_matches(worker, &incoming.inspection, true)
                && incoming.restart_count == Some(0)
                && worker.attempt.start_operation_id.is_some()
                && ((incoming.status == RuntimeStatusV1::Running && incoming.running == Some(true))
                    || ([RuntimeStatusV1::Exited, RuntimeStatusV1::Dead]
                        .contains(&incoming.status)
                        && incoming.running == Some(false)))
                && let Some(started) = incoming
                    .started_at
                    .as_ref()
                    .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
                    .filter(|date| date.timestamp() > 0)
            {
                // A credible historical start remains evidence even when
                // finish time or exit code is not yet observable.
                worker.observed_started_at = Some(
                    started
                        .with_timezone(&chrono::Utc)
                        .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
                );
            }
            let diagnosis = classify_observation(worker, incoming);
            match diagnosis {
                ObservationClass::Running => {
                    worker.state = if worker.state == WorkerStateV1::Stopping {
                        WorkerStateV1::Stopping
                    } else {
                        WorkerStateV1::Running
                    };
                    worker.unknown_reason = None;
                    if worker.observed_started_at.is_none() {
                        worker.observed_started_at = incoming.started_at.clone();
                    }
                }
                ObservationClass::Terminal(ref started, ref finished, ns) => {
                    let cpu = ns
                        .checked_mul(u128::from(worker.launch.cpu_millis))
                        .context("allocation CPU overflow")?;
                    let ram = ns
                        .checked_mul(u128::from(worker.launch.memory_bytes))
                        .context("allocation memory overflow")?;
                    allocation = Some(AllocationV1 {
                        attempt_id: worker.attempt.attempt_id.clone(),
                        worker_id: worker.worker_id.clone(),
                        terminal_observation_id: mutation.operation_id.clone(),
                        started_at: started.clone(),
                        finished_at: finished.clone(),
                        duration_ns: ns.to_string(),
                        cpu_millicore_nanoseconds: cpu.to_string(),
                        memory_byte_nanoseconds: ram.to_string(),
                        method_version: 1,
                        billable: false,
                        source: allocation_source(incoming),
                    });
                    worker.state = WorkerStateV1::Terminal;
                    worker.quota_held = false;
                    worker.unknown_reason = None;
                    if worker.observed_started_at.is_none() {
                        worker.observed_started_at = Some(started.clone());
                    }
                }
                ObservationClass::Unknown(reason) => {
                    worker.state = WorkerStateV1::InDoubt;
                    worker.unknown_reason = Some(reason.into());
                }
            }
            worker.last_observation = Some(incoming.clone());
            observation = Some(incoming.clone());
        }
        WorkerTransitionV1::Unknown { reason } => {
            bounded(reason, 1024)?;
            conflict(worker.quota_held)?;
            worker.state = WorkerStateV1::InDoubt;
            worker.unknown_reason = Some(reason.clone());
        }
        WorkerTransitionV1::Stop => {
            conflict(worker.quota_held && worker.attempt.container_id.is_some())?;
            worker.state = WorkerStateV1::Stopping;
        }
        WorkerTransitionV1::CancelReserved => {
            conflict(
                worker.state == WorkerStateV1::Reserved
                    && worker.attempt.create_operation_id.is_none()
                    && worker.attempt.start_operation_id.is_none()
                    && worker.quota_held,
            )?;
            worker.state = WorkerStateV1::Cancelled;
            worker.quota_held = false;
        }
        WorkerTransitionV1::CleanupResult { removed, error } => {
            conflict(worker.state == WorkerStateV1::Terminal && !worker.quota_held)?;
            invalid((*removed && error.is_none()) || (!*removed && error.is_some()))?;
            if let Some(error) = error {
                bounded(error, 1024)?;
            }
            worker.cleanup_removed = Some(*removed);
            worker.cleanup_error = error.clone();
        }
    }
    worker.revision = worker
        .revision
        .checked_add(1)
        .context("worker revision exhausted")?;
    Ok((observation, allocation))
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
    let event = &source.event;
    let approved = event.data["human_approved"]
        .as_bool()
        .context("admission receipt phase is missing")?;
    anyhow::ensure!(
        source.seq < admission_seq
            && source.hash == worker.receipt.audit_hash
            && event.kind == "security.decision"
            && event.source == "gateway"
            && event.provenance == "observed"
            && event
                .decision
                .as_ref()
                .is_some_and(|decision| decision.outcome == Outcome::Allow)
            && event.data["request_hash"].as_str() == Some(worker.receipt.request_hash.as_str()),
        "admission receipt does not reference an earlier allow"
    );
    let request = DecisionRequest {
        request_id: event.data["request_id"]
            .as_str()
            .context("admission receipt request is missing")?
            .into(),
        profile: event.data["profile"]
            .as_str()
            .context("admission receipt profile is missing")?
            .into(),
        command: event
            .command
            .clone()
            .context("admission command is missing")?,
        project_id: event.project_id.clone(),
        sandbox_id: event.sandbox_id.clone(),
        agent_id: event.agent_id.clone(),
        session_id: event.session_id.clone(),
        correlation_id: event.correlation_id.clone(),
        trace_id: event.trace_id.clone(),
        human_approved: approved,
        approval_hash: approved.then(|| worker.receipt.request_hash.clone()),
        execution_mode: Store::decision_execution_mode(&event.data)?,
        provider_resolution: Store::decision_provider_resolution(&event.data)?,
    };
    let linked = Store::lookup_decision(db, &request, &worker.receipt.request_hash)?
        .context("admission receipt decision is missing")?;
    let admission = AdmissionV1 {
        version: 1,
        operation_id: record.operation_id.clone(),
        decision_request: request,
        receipt: worker.receipt.clone(),
        launch: worker.launch.clone(),
    };
    admission.validate()?;
    anyhow::ensure!(
        linked.seq == source.seq
            && linked.hash == source.hash
            && admission.input_hash()? == record.input_hash,
        "admission input differs from its linked decision or journal"
    );
    Ok(())
}

fn replay_worker_transition(
    previous: &WorkerV1,
    worker: &WorkerV1,
    record: &ControlRecord,
) -> anyhow::Result<()> {
    // Version 1 stores each mutation's full input across its identity, action,
    // snapshot and observation. Rebuild that input without trusting its result.
    // Admission inputs have a different contract and are validated separately.
    let transition = match record.action.as_str() {
        "prepare" => WorkerTransitionV1::Prepare,
        "create_issued" => WorkerTransitionV1::CreateIssued,
        "created" => WorkerTransitionV1::Created {
            inspection: worker
                .attempt
                .created_inspection
                .clone()
                .context("created transition has no inspection")?,
        },
        "start_claim" => WorkerTransitionV1::StartClaim {
            emitter_id: worker
                .attempt
                .start_emitter_id
                .clone()
                .context("start claim has no emitter")?,
        },
        "observe" => WorkerTransitionV1::Observe {
            observation: record
                .observation
                .clone()
                .context("observe transition has no observation")?,
        },
        "unknown" => WorkerTransitionV1::Unknown {
            reason: worker
                .unknown_reason
                .clone()
                .context("unknown transition has no reason")?,
        },
        "stop" => WorkerTransitionV1::Stop,
        "cancel_reserved" => WorkerTransitionV1::CancelReserved,
        "cleanup_result" => WorkerTransitionV1::CleanupResult {
            removed: worker
                .cleanup_removed
                .context("cleanup result is missing")?,
            error: worker.cleanup_error.clone(),
        },
        _ => anyhow::bail!("unsupported worker mutation action"),
    };
    let mutation = WorkerMutationV1 {
        version: 1,
        operation_id: record.operation_id.clone(),
        worker_id: previous.worker_id.clone(),
        attempt_id: previous.attempt.attempt_id.clone(),
        expected_revision: previous.revision,
        transition,
    };
    anyhow::ensure!(
        content_hash("ai-guard:worker-mutation:v1", &mutation)? == record.input_hash
            && record.refusal.is_none()
            && record.admission_policy.is_none(),
        "worker mutation input or metadata differs from journal"
    );
    let mut expected = previous.clone();
    let (observation, allocation) = apply_worker_transition(&mut expected, &mutation)?;
    anyhow::ensure!(
        serde_json::to_value(expected)? == serde_json::to_value(worker)?
            && observation == record.observation
            && allocation == record.allocation,
        "worker mutation result differs from journal"
    );
    Ok(())
}

fn prepare_worker(worker: &mut WorkerV1) -> anyhow::Result<()> {
    conflict(
        [WorkerStateV1::Reserved, WorkerStateV1::InDoubt].contains(&worker.state)
            && worker.quota_held
            && worker.attempt.create_operation_id.is_none()
            && worker.attempt.container_id.is_none()
            && worker.attempt.created_inspection.is_none()
            && worker.attempt.start_operation_id.is_none()
            && !worker.allocation_inconsistent,
    )?;
    worker.state = WorkerStateV1::Preparing;
    worker.unknown_reason = None;
    Ok(())
}

fn record_created_worker(
    worker: &mut WorkerV1,
    inspection: &RuntimeInspectionV1,
) -> anyhow::Result<()> {
    conflict(
        [WorkerStateV1::Preparing, WorkerStateV1::InDoubt].contains(&worker.state)
            && worker.quota_held
            && worker.attempt.create_operation_id.is_some()
            && worker.attempt.container_id.is_none()
            && worker.attempt.created_inspection.is_none()
            && worker.attempt.start_operation_id.is_none()
            && !worker.allocation_inconsistent,
    )?;
    validate_inspection(inspection)?;
    invalid(inspection_matches(worker, inspection, false))?;
    worker.attempt.container_id = Some(inspection.container_id.clone());
    worker.attempt.created_inspection = Some(inspection.clone());
    worker.state = WorkerStateV1::Created;
    worker.unknown_reason = None;
    Ok(())
}

fn validate_inspection(inspection: &RuntimeInspectionV1) -> anyhow::Result<()> {
    bounded(&inspection.authority_id, 256)?;
    bounded(&inspection.stable_name, 128)?;
    bounded(&inspection.volume_name, 128)?;
    hash(&inspection.container_id)?;
    hash(&inspection.spec_hash)?;
    hash(
        inspection
            .image_id
            .strip_prefix("sha256:")
            .ok_or(ControlFailure::InvalidRequest)?,
    )?;
    Ok(())
}
fn inspection_matches(worker: &WorkerV1, i: &RuntimeInspectionV1, known_cid: bool) -> bool {
    i.authority_id == worker.launch.authority_id
        && i.runtime_kind == worker.launch.runtime_kind
        && i.stable_name == worker.attempt.stable_name
        && i.volume_name == worker.attempt.volume_name
        && i.image_id == worker.launch.image_id
        && i.spec_hash == worker.spec_hash
        && i.cpu_millis == worker.launch.cpu_millis
        && i.memory_bytes == worker.launch.memory_bytes
        && i.pids_limit == worker.launch.pids_limit
        && i.network == worker.launch.network
        && i.security_preset == worker.launch.security_preset
        && i.restart_policy == worker.launch.restart_policy
        && (!known_cid || worker.attempt.container_id.as_deref() == Some(i.container_id.as_str()))
}
enum ObservationClass {
    Running,
    Terminal(String, String, u128),
    Unknown(&'static str),
}
fn allocation_source(observation: &RuntimeObservationV1) -> AllocationSourceV1 {
    AllocationSourceV1 {
        authority_id: observation.inspection.authority_id.clone(),
        runtime_kind: observation.inspection.runtime_kind,
        container_id: observation.inspection.container_id.clone(),
        spec_hash: observation.inspection.spec_hash.clone(),
        provenance: AllocationProvenanceV1::Observed,
    }
}
fn classify_observation(worker: &WorkerV1, observation: &RuntimeObservationV1) -> ObservationClass {
    if worker.allocation_inconsistent {
        return ObservationClass::Unknown(
            "allocation history is inconsistent; explicit reconciliation required",
        );
    }
    if !inspection_matches(worker, &observation.inspection, true) {
        return ObservationClass::Unknown("runtime identity or inspected configuration differs");
    }
    if worker.attempt.start_operation_id.is_none() {
        return ObservationClass::Unknown("runtime start was not claimed by control");
    }
    if observation.restart_count != Some(0) {
        return ObservationClass::Unknown("runtime restart count is missing or nonzero");
    }
    let Some(started) = observation
        .started_at
        .as_ref()
        .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
    else {
        return ObservationClass::Unknown("runtime start time is missing or invalid");
    };
    if started.timestamp() <= 0 {
        return ObservationClass::Unknown("runtime start time is not an observed execution time");
    }
    if let Some(previous) = worker
        .observed_started_at
        .as_ref()
        .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
        && previous.timestamp() > 0
        && previous != started
    {
        return ObservationClass::Unknown("runtime start time changed");
    }
    if observation.status == RuntimeStatusV1::Running && observation.running == Some(true) {
        return ObservationClass::Running;
    }
    if ![RuntimeStatusV1::Exited, RuntimeStatusV1::Dead].contains(&observation.status)
        || observation.running != Some(false)
        || observation.exit_code.is_none()
    {
        return ObservationClass::Unknown("runtime terminal state or exit code is incomplete");
    }
    let Some(finished) = observation
        .finished_at
        .as_ref()
        .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
    else {
        return ObservationClass::Unknown("runtime finish time is missing or invalid");
    };
    if finished < started {
        return ObservationClass::Unknown("runtime finish precedes start");
    }
    let Some(duration) = (finished - started)
        .num_nanoseconds()
        .and_then(|ns| u128::try_from(ns).ok())
    else {
        return ObservationClass::Unknown("runtime interval cannot be represented exactly");
    };
    ObservationClass::Terminal(
        started
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        finished
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        duration,
    )
}
