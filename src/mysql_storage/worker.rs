use super::{audit::decode_event_columns, decision, driver_error};
use crate::{
    audit::{
        decision_codec, supervision_admission,
        worker_admission::{self, ControlRecord},
        worker_transition,
    },
    control::{
        AdmissionV1, AllocationV1, ControlFailure, MutationReceiptV1, QuotaPolicyV1, QuotaUsageV1,
        SupervisedAdmissionV1, WorkerStateV1, WorkerV1, conflict, content_hash, hash, uuid,
    },
    model::{Decision, Event, StoredEvent},
    policy::Policy,
    redaction::Redactor,
};
use anyhow::{Context, Result, anyhow, ensure};
use mysql_async::{Row, Transaction, from_row_opt, prelude::Queryable};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const KIND: &str = "control.worker.transition";
const MUTATION_EVENT_COLUMNS: &str = "SELECT m.operation_id,m.worker_id,m.action,m.input_hash,
    e.seq,e.event_id,e.received_at,e.prev_hash,e.hash,e.payload
    FROM ai_guard_worker_mutation m JOIN ai_guard_audit_event e ON e.seq=m.event_seq";
const WORKER_SNAPSHOT: &str = "SELECT IF(OCTET_LENGTH(snapshot)<=131072,snapshot,NULL)
    FROM ai_guard_worker";

const WORKER_COLUMNS: &str = "SELECT worker_id,operation_id,request_id,session_id,state,quota_held,
    cpu_millis,memory_bytes,revision,IF(OCTET_LENGTH(snapshot)<=131072,snapshot,NULL),snapshot_hash,event_seq
    FROM ai_guard_worker ORDER BY worker_id";
const ATTEMPT_COLUMNS: &str = "SELECT attempt_id,worker_id,authority_id,stable_name,volume_name,
    IF(OCTET_LENGTH(snapshot)<=131072,snapshot,NULL)
    FROM ai_guard_worker_attempt ORDER BY attempt_id";
const OBSERVATION_COLUMNS: &str = "SELECT observation_id,worker_id,attempt_id,input_hash,
    IF(OCTET_LENGTH(snapshot)<=131072,snapshot,NULL),event_seq
    FROM ai_guard_worker_observation ORDER BY observation_id";
const ALLOCATION_COLUMNS: &str = "SELECT attempt_id,worker_id,terminal_observation_id,
    IF(OCTET_LENGTH(snapshot)<=131072,snapshot,NULL),event_seq
    FROM ai_guard_worker_allocation";
const SUPERVISION_COLUMNS: &str = "SELECT worker_id,intent_hash,
    IF(OCTET_LENGTH(payload_json)<=131072,payload_json,NULL),event_seq
    FROM ai_guard_worker_supervision ORDER BY worker_id";
type WorkerRow = (
    String,
    String,
    String,
    String,
    String,
    u8,
    u32,
    u64,
    u64,
    Vec<u8>,
    String,
    i64,
);
type AttemptRow = (String, String, String, String, String, Vec<u8>);
type ObservationRow = (String, String, String, String, Vec<u8>, i64);
type AllocationRow = (String, String, String, Vec<u8>, i64);
type SupervisionRow = (String, String, Vec<u8>, i64);

pub(super) type DecisionHistory = BTreeMap<String, Vec<StoredEvent>>;

pub(super) struct Proposal<'a> {
    pub(super) policy: &'a Policy,
    pub(super) fresh: Decision,
    pub(super) jev: Value,
    pub(super) quota: &'a QuotaPolicyV1,
}

pub(super) fn event(record: &ControlRecord) -> Result<Event> {
    let mut event = Event::new(KIND);
    event.data = serde_json::to_value(record)?;
    event.session_id = record
        .worker
        .as_ref()
        .map(|worker| worker.launch.session_id.clone());
    Ok(event)
}

pub(super) fn require_prepared_event(
    prepared: &Event,
    record: &ControlRecord,
    redactor: &Redactor,
) -> Result<()> {
    let mut expected = serde_json::to_value(record)?;
    if let Some(explanation) = expected.get_mut("admission_policy") {
        *explanation = redactor.value(explanation);
    }
    ensure!(prepared.data == expected, ControlFailure::InvalidRequest);
    check_event(prepared)?;
    Ok(())
}

pub(super) fn check_event(event: &Event) -> Result<ControlRecord> {
    let record: ControlRecord = serde_json::from_value(event.data.clone())
        .map_err(|_| anyhow!("invalid MySQL worker control record"))?;
    check_record(event, &record, KIND)?;
    Ok(record)
}

fn check_record(event: &Event, record: &ControlRecord, kind: &str) -> Result<()> {
    uuid(&record.operation_id)?;
    hash(&record.input_hash)?;
    ensure!(
        event.kind == kind
            && event.source == "gateway"
            && event.provenance == "observed"
            && event.coverage == "observed"
            && event.session_id.as_ref() == record.worker.as_ref().map(|w| &w.launch.session_id)
            && record.version == 1
            && record.worker.is_some() == record.snapshot_hash.is_some(),
        "invalid MySQL worker control record"
    );
    if record.action == "admit" {
        ensure!(
            record.previous_revision.is_none()
                && record.observation.is_none()
                && record.allocation.is_none()
                && record.admission_policy.is_some()
                && record.worker.is_some() != record.refusal.is_some(),
            "invalid MySQL worker admission record"
        );
    } else {
        ensure!(
            record
                .previous_revision
                .is_some_and(|revision| revision > 0)
                && record.worker.is_some()
                && record.refusal.is_none()
                && record.admission_policy.is_none(),
            "invalid MySQL worker transition record"
        );
    }
    Ok(())
}

struct DecodedMutation {
    stored: StoredEvent,
    record: ControlRecord,
    action: String,
    input_hash: String,
    supervision: Option<SupervisionRow>,
    supervised_receipt: Option<MutationReceiptV1>,
}

fn decode_mutation(row: Row) -> Result<DecodedMutation> {
    type MutationColumns = (
        String,
        Option<String>,
        String,
        String,
        i64,
        String,
        String,
        String,
        String,
        String,
    );
    let (operation, worker, action, input_hash, seq, id, received, prev, hash, payload):
        MutationColumns = from_row_opt(row).map_err(|_| anyhow!("unexpected MySQL worker mutation row"))?;
    let stored = decode_event_columns((seq, id, received, prev, hash, payload))?.stored;
    let (record, supervision, supervised_receipt) =
        if stored.event.kind == supervision_admission::KIND {
            let envelope = supervision_admission::decode(&stored)?;
            let (request, record) = envelope.admission()?;
            check_record(&stored.event, record, supervision_admission::KIND)?;
            let receipt = supervision_admission::receipt_for(&stored, &envelope)?;
            ensure!(
                operation == receipt.operation_id
                    && worker == receipt.worker_id
                    && action == supervision_admission::ACTION
                    && input_hash == receipt.input_hash,
                "MySQL supervised admission mutation differs from its audit event"
            );
            (
                record.clone(),
                record
                    .worker
                    .as_ref()
                    .map(|worker| supervision_row(request, worker, stored.seq))
                    .transpose()?,
                Some(receipt),
            )
        } else {
            let record = check_event(&stored.event)?;
            ensure!(
                operation == record.operation_id
                    && worker.as_ref() == record.worker.as_ref().map(|worker| &worker.worker_id)
                    && action == record.action
                    && input_hash == record.input_hash,
                "MySQL worker mutation differs from its audit event"
            );
            (record, None, None)
        };
    Ok(DecodedMutation {
        stored,
        record,
        action,
        input_hash,
        supervision,
        supervised_receipt,
    })
}

async fn find_operation(
    tx: &mut Transaction<'_>,
    operation: &str,
) -> Result<Option<DecodedMutation>> {
    let row = tx
        .exec_first::<Row, _, _>(
            format!("{MUTATION_EVENT_COLUMNS} WHERE m.operation_id=?"),
            (operation,),
        )
        .await
        .map_err(driver_error)?;
    row.map(decode_mutation).transpose()
}

pub(super) async fn lookup(
    tx: &mut Transaction<'_>,
    operation: &str,
    action: &str,
    input_hash: &str,
) -> Result<Option<StoredEvent>> {
    find_operation(tx, operation)
        .await?
        .map(|decoded| {
            conflict(decoded.action == action && decoded.input_hash == input_hash)?;
            Ok(decoded.stored)
        })
        .transpose()
}

/// Read the receipt from the same snapshot as the verified worker projections.
pub(super) async fn operation_receipt(
    tx: &mut Transaction<'_>,
    operation: &str,
) -> Result<Option<MutationReceiptV1>> {
    Ok(find_operation(tx, operation).await?.map(|decoded| {
        decoded
            .supervised_receipt
            .unwrap_or_else(|| worker_admission::receipt(&decoded.stored, &decoded.record))
    }))
}

/// Read the last audited worker snapshot strictly before `before_seq`.
/// The caller must have verified the tenant binding, audit chain and worker
/// projections in this same repeatable-read transaction.
pub(super) async fn snapshot_before_verified(
    tx: &mut Transaction<'_>,
    worker_id: &str,
    before_seq: i64,
) -> Result<Option<WorkerV1>> {
    uuid(worker_id)?;
    ensure!(before_seq > 0, ControlFailure::InvalidRequest);
    let row = tx
        .exec_first::<Row, _, _>(
            format!(
                "{MUTATION_EVENT_COLUMNS} WHERE m.worker_id=? AND m.event_seq<?
                 ORDER BY m.event_seq DESC LIMIT 1"
            ),
            (worker_id, before_seq),
        )
        .await
        .map_err(driver_error)?;
    let Some(row) = row else {
        return Ok(None);
    };

    let decoded = decode_mutation(row)?;
    let worker = decoded
        .record
        .worker
        .context("MySQL historical worker mutation has no snapshot")?;
    ensure!(
        decoded.stored.seq > 0 && decoded.stored.seq < before_seq && worker.worker_id == worker_id,
        "MySQL historical worker snapshot differs from the requested prefix"
    );
    Ok(Some(worker))
}

pub(super) async fn load(tx: &mut Transaction<'_>, worker_id: &str) -> Result<WorkerV1> {
    find(tx, worker_id)
        .await?
        .ok_or_else(|| ControlFailure::NotFound.into())
}

pub(super) async fn find(tx: &mut Transaction<'_>, worker_id: &str) -> Result<Option<WorkerV1>> {
    let row = tx
        .exec_first::<Row, _, _>(format!("{WORKER_SNAPSHOT} WHERE worker_id=?"), (worker_id,))
        .await
        .map_err(driver_error)?;
    row.map(decode_snapshot).transpose()
}

/// The caller validates the limit and any nonempty cursor before opening the Store.
pub(super) async fn page(
    tx: &mut Transaction<'_>,
    cursor: &str,
    limit: u32,
) -> Result<Vec<WorkerV1>> {
    // worker_id is a fixed-width binary column containing canonical UUID text.
    let rows: Vec<Row> = tx
        .exec(
            format!(
                "{WORKER_SNAPSHOT} WHERE worker_id>CAST(? AS BINARY) ORDER BY worker_id LIMIT ?"
            ),
            (cursor, limit),
        )
        .await
        .map_err(driver_error)?;
    rows.into_iter().map(decode_snapshot).collect()
}

fn decode_snapshot(row: Row) -> Result<WorkerV1> {
    let (snapshot,): (Vec<u8>,) =
        from_row_opt(row).map_err(|_| anyhow!("unexpected MySQL worker snapshot"))?;
    serde_json::from_slice(&snapshot).map_err(|_| anyhow!("invalid MySQL worker snapshot"))
}

/// Read bounded allocation snapshots after verifying projections in the same transaction.
pub(super) async fn usage_page(
    tx: &mut Transaction<'_>,
    cursor: &str,
    limit: u32,
) -> Result<Vec<AllocationV1>> {
    let rows: Vec<Row> = tx
        .exec(
            format!(
                "{ALLOCATION_COLUMNS} WHERE attempt_id>CAST(? AS BINARY) ORDER BY attempt_id LIMIT ?"
            ),
            (cursor, limit),
        )
        .await
        .map_err(driver_error)?;
    rows.into_iter()
        .map(|row| {
            let (_, _, _, snapshot, _): AllocationRow =
                from_row_opt(row).map_err(|_| anyhow!("unexpected MySQL worker allocation row"))?;
            serde_json::from_slice(&snapshot)
                .map_err(|_| anyhow!("invalid MySQL worker allocation snapshot"))
        })
        .collect()
}

pub(super) async fn usage(
    tx: &mut Transaction<'_>,
    policy: &QuotaPolicyV1,
) -> Result<QuotaUsageV1> {
    let (count, cpu, memory) = tx
        .query_first::<(u32, u64, u64), _>(
            "SELECT COUNT(*),COALESCE(SUM(cpu_millis),0),COALESCE(SUM(memory_bytes),0)
             FROM ai_guard_worker WHERE quota_held=1",
        )
        .await
        .map_err(driver_error)?
        .context("MySQL worker quota is unavailable")?;
    Ok(QuotaUsageV1 {
        policy: policy.clone(),
        held_workers: count,
        held_cpu_millis: cpu,
        held_memory_bytes: memory,
    })
}

pub(super) async fn prepare(
    tx: &mut Transaction<'_>,
    admission: &AdmissionV1,
    proposal: Proposal<'_>,
    namespace: &str,
) -> Result<ControlRecord> {
    let stored = decision::lookup(
        tx,
        &admission.decision_request,
        &admission.receipt.request_hash,
    )
    .await?
    .ok_or(ControlFailure::InvalidRequest)?;
    worker_admission::require_receipt(&admission.receipt, &stored)?;
    let duplicate = tx
        .exec_first::<u8, _, _>(
            "SELECT EXISTS(SELECT 1 FROM ai_guard_worker WHERE request_id=? OR session_id=?)",
            (
                &admission.decision_request.request_id,
                &admission.launch.session_id,
            ),
        )
        .await
        .map_err(driver_error)?;
    conflict(duplicate == Some(0))?;
    let usage = usage(tx, proposal.quota).await?;
    worker_admission::build_record(
        admission,
        proposal.policy,
        proposal.fresh,
        proposal.jev,
        &usage,
        || {
            Ok((
                namespace.into(),
                ::uuid::Uuid::new_v4().to_string(),
                ::uuid::Uuid::new_v4().to_string(),
            ))
        },
    )
}

pub(super) async fn persist(
    tx: &mut Transaction<'_>,
    record: &ControlRecord,
    seq: i64,
) -> Result<()> {
    persist_worker(tx, record, seq).await?;
    persist_mutation(
        tx,
        &record.operation_id,
        record
            .worker
            .as_ref()
            .map(|worker| worker.worker_id.as_str()),
        &record.action,
        &record.input_hash,
        seq,
    )
    .await
}

pub(super) async fn persist_supervised(
    tx: &mut Transaction<'_>,
    envelope: &supervision_admission::Record,
    stored: &StoredEvent,
) -> Result<()> {
    let (request, record) = envelope.admission()?;
    let receipt = supervision_admission::receipt_for(stored, envelope)?;
    if record.worker.is_some() {
        let exists = tx
            .exec_first::<u8, _, _>(
                "SELECT EXISTS(SELECT 1 FROM ai_guard_worker_supervision WHERE intent_hash=?)",
                (&request.intent_hash,),
            )
            .await
            .map_err(driver_error)?;
        conflict(exists == Some(0))?;
    }
    persist_worker(tx, record, stored.seq).await?;
    if let Some(worker) = &record.worker {
        tx.exec_drop(
            "INSERT INTO ai_guard_worker_supervision (worker_id,intent_hash,payload_json,event_seq)
             VALUES (?,?,?,?)",
            supervision_row(request, worker, stored.seq)?,
        )
        .await
        .map_err(driver_error)?;
    }
    persist_mutation(
        tx,
        &receipt.operation_id,
        receipt.worker_id.as_deref(),
        supervision_admission::ACTION,
        &receipt.input_hash,
        stored.seq,
    )
    .await
}

async fn persist_worker(tx: &mut Transaction<'_>, record: &ControlRecord, seq: i64) -> Result<()> {
    if let Some(worker) = &record.worker {
        if let Some(revision) = record.previous_revision {
            tx.exec_drop(
                "UPDATE ai_guard_worker SET state=?,quota_held=?,revision=?,snapshot=?,snapshot_hash=?,event_seq=?
                 WHERE worker_id=? AND revision=?",
                (state_name(worker.state)?, u8::from(worker.quota_held), worker.revision,
                 serde_json::to_vec(worker)?, &record.snapshot_hash, seq, &worker.worker_id, revision),
            ).await.map_err(driver_error)?;
            conflict(tx.affected_rows() == 1)?;
            tx.exec_drop(
                "UPDATE ai_guard_worker_attempt SET snapshot=? WHERE attempt_id=? AND worker_id=?",
                (
                    serde_json::to_vec(&worker.attempt)?,
                    &worker.attempt.attempt_id,
                    &worker.worker_id,
                ),
            )
            .await
            .map_err(driver_error)?;
        } else {
            tx.exec_drop(
                "INSERT INTO ai_guard_worker
                 (worker_id,operation_id,request_id,session_id,state,quota_held,cpu_millis,memory_bytes,revision,snapshot,snapshot_hash,event_seq)
                 VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                worker_row(worker, seq)?,
            ).await.map_err(driver_error)?;
            tx.exec_drop(
                "INSERT INTO ai_guard_worker_attempt (attempt_id,worker_id,authority_id,stable_name,volume_name,snapshot) VALUES (?,?,?,?,?,?)",
                attempt_row(worker)?,
            ).await.map_err(driver_error)?;
        }
        if let Some(observation) = observation_row(record, worker, seq)? {
            tx.exec_drop(
                "INSERT INTO ai_guard_worker_observation
                 (observation_id,worker_id,attempt_id,input_hash,snapshot,event_seq) VALUES (?,?,?,?,?,?)",
                observation,
            ).await.map_err(driver_error)?;
        }
        if let Some(allocation) = &record.allocation {
            tx.exec_drop(
                "INSERT INTO ai_guard_worker_allocation
                 (attempt_id,worker_id,terminal_observation_id,snapshot,event_seq) VALUES (?,?,?,?,?)",
                allocation_row(allocation, seq)?,
            ).await.map_err(driver_error)?;
        }
    }
    Ok(())
}

async fn persist_mutation(
    tx: &mut Transaction<'_>,
    operation_id: &str,
    worker_id: Option<&str>,
    action: &str,
    input_hash: &str,
    seq: i64,
) -> Result<()> {
    tx.exec_drop(
        "INSERT INTO ai_guard_worker_mutation (operation_id,worker_id,action,input_hash,event_seq) VALUES (?,?,?,?,?)",
        (operation_id, worker_id, action, input_hash, seq),
    ).await.map_err(driver_error)?;
    Ok(())
}

fn state_name(state: WorkerStateV1) -> Result<String> {
    serde_json::from_value(serde_json::to_value(state)?).context("invalid MySQL worker state")
}

fn worker_row(worker: &WorkerV1, seq: i64) -> Result<WorkerRow> {
    Ok((
        worker.worker_id.clone(),
        worker.admission_operation_id.clone(),
        worker.request_id.clone(),
        worker.launch.session_id.clone(),
        state_name(worker.state)?,
        u8::from(worker.quota_held),
        worker.launch.cpu_millis,
        worker.launch.memory_bytes,
        worker.revision,
        serde_json::to_vec(worker)?,
        content_hash("ai-guard:worker-snapshot:v1", worker)?,
        seq,
    ))
}

fn observation_row(
    record: &ControlRecord,
    worker: &WorkerV1,
    seq: i64,
) -> Result<Option<ObservationRow>> {
    record
        .observation
        .as_ref()
        .map(|observation| {
            Ok((
                record.operation_id.clone(),
                worker.worker_id.clone(),
                worker.attempt.attempt_id.clone(),
                record.input_hash.clone(),
                serde_json::to_vec(observation)?,
                seq,
            ))
        })
        .transpose()
}

fn allocation_row(allocation: &AllocationV1, seq: i64) -> Result<AllocationRow> {
    Ok((
        allocation.attempt_id.clone(),
        allocation.worker_id.clone(),
        allocation.terminal_observation_id.clone(),
        serde_json::to_vec(allocation)?,
        seq,
    ))
}

fn attempt_row(worker: &WorkerV1) -> Result<AttemptRow> {
    Ok((
        worker.attempt.attempt_id.clone(),
        worker.worker_id.clone(),
        worker.launch.authority_id.clone(),
        worker.attempt.stable_name.clone(),
        worker.attempt.volume_name.clone(),
        serde_json::to_vec(&worker.attempt)?,
    ))
}

fn supervision_row(
    request: &SupervisedAdmissionV1,
    worker: &WorkerV1,
    seq: i64,
) -> Result<SupervisionRow> {
    let binding = supervision_admission::Binding {
        worker_id: worker.worker_id.clone(),
        intent_hash: request.intent_hash.clone(),
        input: supervision_admission::Input::Admission(Box::new(request.clone())),
    };
    let payload = serde_json::to_vec(&binding)?;
    Ok((binding.worker_id, binding.intent_hash, payload, seq))
}

pub(super) async fn require_empty(tx: &mut Transaction<'_>) -> Result<()> {
    require_counts(tx, 0, 0, 0, 0, 0).await
}

async fn require_counts(
    tx: &mut Transaction<'_>,
    workers: u64,
    mutations: u64,
    observations: u64,
    allocations: u64,
    supervision: u64,
) -> Result<()> {
    let count = tx.query_first::<(u64,u64,u64,u64,u64,u64), _>(
        "SELECT (SELECT COUNT(*) FROM ai_guard_worker), (SELECT COUNT(*) FROM ai_guard_worker_attempt),
         (SELECT COUNT(*) FROM ai_guard_worker_mutation), (SELECT COUNT(*) FROM ai_guard_worker_observation),
         (SELECT COUNT(*) FROM ai_guard_worker_allocation), (SELECT COUNT(*) FROM ai_guard_worker_supervision)",
    ).await.map_err(driver_error)?;
    ensure!(
        count
            == Some((
                workers,
                workers,
                mutations,
                observations,
                allocations,
                supervision
            )),
        "MySQL worker projection count mismatch"
    );
    Ok(())
}

pub(super) async fn verify_projections(
    tx: &mut Transaction<'_>,
    namespace: &str,
    expected_events: u64,
    history: &DecisionHistory,
) -> Result<()> {
    let mut workers: BTreeMap<String, (i64, WorkerV1)> = BTreeMap::new();
    let mut observations = BTreeMap::new();
    let mut allocations = BTreeMap::new();
    let mut supervision_rows = BTreeMap::new();
    let mut intent_hashes = BTreeSet::new();
    let mut usage = QuotaUsageV1 {
        policy: QuotaPolicyV1::default(),
        held_workers: 0,
        held_cpu_millis: 0,
        held_memory_bytes: 0,
    };
    let mut mutations = 0;
    let mut previous_seq = 0;
    let mut result = tx
        .query_iter(format!("{MUTATION_EVENT_COLUMNS} ORDER BY e.seq"))
        .await
        .map_err(driver_error)?;
    while let Some(row) = result.next().await.map_err(driver_error)? {
        let DecodedMutation {
            stored,
            record,
            supervision,
            ..
        } = decode_mutation(row)?;
        ensure!(
            stored.seq > previous_seq,
            "MySQL worker projections have duplicate audit sequences"
        );
        previous_seq = stored.seq;
        mutations += 1;
        let Some(worker) = record.worker.as_ref() else {
            quota_from_record(&record)?;
            continue;
        };
        if let Some((_, previous)) = workers.get(&worker.worker_id) {
            worker_transition::replay_record(previous, &record)?;
            let snapshot_hash = content_hash("ai-guard:worker-snapshot:v1", worker)?;
            ensure!(
                record.snapshot_hash.as_deref() == Some(snapshot_hash.as_str()),
                "invalid MySQL worker transition snapshot"
            );
            if previous.quota_held && !worker.quota_held {
                usage.held_workers -= 1;
                usage.held_cpu_millis -= u64::from(worker.launch.cpu_millis);
                usage.held_memory_bytes -= worker.launch.memory_bytes;
            }
            if let Some(row) = observation_row(&record, worker, stored.seq)? {
                ensure!(
                    observations
                        .insert(record.operation_id.clone(), row)
                        .is_none(),
                    "MySQL worker observation reuses an operation"
                );
            }
            if let Some(allocation) = &record.allocation {
                ensure!(
                    allocations
                        .insert(
                            allocation.attempt_id.clone(),
                            allocation_row(allocation, stored.seq)?
                        )
                        .is_none(),
                    "MySQL worker attempt has multiple allocations"
                );
            }
        } else {
            ensure!(
                record.action == "admit",
                "MySQL worker transition has no admission"
            );
            verify_worker(stored.seq, worker, &record, namespace, history)?;
            usage.policy = quota_from_record(&record)?;
            ensure!(
                worker_admission::fits_quota(&worker.launch, &usage),
                "MySQL worker admission exceeds historical tenant quota"
            );
            if let Some(row) = supervision {
                ensure!(
                    intent_hashes.insert(row.1.clone())
                        && supervision_rows.insert(row.0.clone(), row).is_none(),
                    "MySQL supervised admission reuses an intent or worker"
                );
            }
            usage.held_workers += 1;
            usage.held_cpu_millis += u64::from(worker.launch.cpu_millis);
            usage.held_memory_bytes += worker.launch.memory_bytes;
        }
        workers.insert(worker.worker_id.clone(), (stored.seq, worker.clone()));
    }
    result.drop_result().await.map_err(driver_error)?;
    ensure!(
        mutations == expected_events,
        "MySQL worker audit event lacks its projection"
    );
    require_counts(
        tx,
        workers.len() as u64,
        mutations,
        observations.len() as u64,
        allocations.len() as u64,
        supervision_rows.len() as u64,
    )
    .await?;
    if workers.is_empty() {
        return Ok(());
    }
    let attempts: BTreeMap<_, _> = workers
        .values()
        .map(|(_, worker)| (&worker.attempt.attempt_id, worker))
        .collect();
    ensure!(
        attempts.len() == workers.len(),
        "MySQL worker record reuses an attempt identity"
    );
    ensure!(
        projections_match(
            tx,
            WORKER_COLUMNS,
            workers
                .values()
                .map(|(seq, worker)| worker_row(worker, *seq))
        )
        .await?,
        "MySQL worker snapshot differs from its audit event"
    );
    ensure!(
        projections_match(tx, ATTEMPT_COLUMNS, attempts.into_values().map(attempt_row)).await?,
        "MySQL worker attempt differs from its audit event"
    );
    if !observations.is_empty() {
        ensure!(
            projections_match(tx, OBSERVATION_COLUMNS, observations.into_values().map(Ok)).await?,
            "MySQL worker observation differs from its audit event"
        );
    }
    if !allocations.is_empty() {
        ensure!(
            projections_match(
                tx,
                &format!("{ALLOCATION_COLUMNS} ORDER BY attempt_id"),
                allocations.into_values().map(Ok)
            )
            .await?,
            "MySQL worker allocation differs from its audit event"
        );
    }
    if !supervision_rows.is_empty() {
        ensure!(
            projections_match(
                tx,
                SUPERVISION_COLUMNS,
                supervision_rows.into_values().map(Ok)
            )
            .await?,
            "MySQL worker supervision binding differs from its audit event"
        );
    }
    Ok(())
}

async fn projections_match<T: mysql_async::prelude::FromRow + PartialEq>(
    tx: &mut Transaction<'_>,
    query: &str,
    expected: impl Iterator<Item = Result<T>>,
) -> Result<bool> {
    let mut result = tx.query_iter(query).await.map_err(driver_error)?;
    for expected in expected {
        let Some(row) = result.next().await.map_err(driver_error)? else {
            return Ok(false);
        };
        let actual: T =
            from_row_opt(row).map_err(|_| anyhow!("unexpected MySQL worker projection row"))?;
        if actual != expected? {
            return Ok(false);
        }
    }
    let complete = result.next().await.map_err(driver_error)?.is_none();
    result.drop_result().await.map_err(driver_error)?;
    Ok(complete)
}

fn quota_from_record(record: &ControlRecord) -> Result<QuotaPolicyV1> {
    let evidence = record
        .admission_policy
        .as_ref()
        .context("missing MySQL worker admission policy")?;
    let value = evidence
        .get("quota")
        .context("missing MySQL worker quota policy")?;
    let quota: QuotaPolicyV1 = serde_json::from_value(value.clone())
        .map_err(|_| anyhow!("invalid MySQL worker quota policy"))?;
    quota.validate()?;
    Ok(quota)
}

fn verify_worker(
    seq: i64,
    worker: &WorkerV1,
    record: &ControlRecord,
    namespace: &str,
    history: &DecisionHistory,
) -> Result<()> {
    worker.launch.validate()?;
    uuid(&worker.worker_id)?;
    uuid(&worker.attempt.attempt_id)?;
    hash(&worker.policy_hash)?;
    let name = format!(
        "aig-{}-{}",
        namespace.replace('-', ""),
        worker.worker_id.replace('-', "")
    );
    let snapshot_hash = content_hash("ai-guard:worker-snapshot:v1", worker)?;
    ensure!(
        worker.admission_operation_id == record.operation_id
            && worker.request_id == worker.receipt.request_id
            && worker.spec_hash == worker.launch.spec_hash()?
            && record.snapshot_hash.as_deref() == Some(snapshot_hash.as_str())
            && record
                .admission_policy
                .as_ref()
                .and_then(|p| p.get("policy_hash"))
                .and_then(Value::as_str)
                == Some(worker.policy_hash.as_str())
            && worker.state == WorkerStateV1::Reserved
            && worker.quota_held
            && worker.revision == 1
            && worker.attempt.stable_name == name
            && worker.attempt.volume_name == format!("{name}-work")
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
            && worker.cleanup_error.is_none(),
        "invalid MySQL reserved worker snapshot"
    );
    let decisions = history
        .get(&worker.request_id)
        .context("MySQL worker decision history is missing")?;
    let source = decisions
        .iter()
        .find(|decision| decision.seq == worker.receipt.audit_seq)
        .context("MySQL worker decision source is missing")?;
    let admission = worker_admission::historical_admission(worker, record, source, seq)?;
    let linked = decision_codec::find_receipt(
        decisions
            .iter()
            .filter(|decision| decision.seq < seq)
            .cloned(),
        &admission.decision_request,
        &worker.receipt.request_hash,
    )?
    .context("MySQL worker decision is missing")?;
    ensure!(
        linked.seq == source.seq && linked.hash == source.hash,
        "MySQL worker decision does not match its source"
    );
    Ok(())
}
