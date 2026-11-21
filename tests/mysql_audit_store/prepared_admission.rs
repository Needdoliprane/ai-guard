use agentsandbox::{
    audit::EventQuery,
    control::{
        AdmissionRefusalV1, QuotaPolicyV1, SecurityPresetV1, SupervisedAdmissionV1,
        SupervisionProtocolV1, WorkerAttemptV1, WorkerStateV1, WorkerV1, content_hash,
    },
    model::{Event, Outcome},
    mysql_storage::audit::MysqlAuditStore,
    policy::{self, Policy},
};
use anyhow::Context;
use mysql_async::{Conn, TxOpts, prelude::Queryable};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Serialize)]
struct AdmissionInput<'a> {
    action: &'static str,
    request: &'a SupervisedAdmissionV1,
}

#[derive(Serialize)]
struct IntentBinding<'a> {
    worker_id: &'a str,
    intent_hash: &'a str,
    input: &'a AdmissionInput<'a>,
}

/// Prepare two admissions in the maintenance fixture after real Store
/// decisions. The execution connection reads and extends that same journal.
pub(super) async fn exercise(
    store: &MysqlAuditStore,
    mut maintenance: Conn,
    namespace: &str,
) -> anyhow::Result<()> {
    exercise_admission(store, &mut maintenance, namespace, false).await?;
    exercise_admission(store, &mut maintenance, namespace, true).await
}

async fn exercise_admission(
    store: &MysqlAuditStore,
    maintenance: &mut Conn,
    namespace: &str,
    admitted: bool,
) -> anyhow::Result<()> {
    let policy = Policy::default();
    let (mut admission, decision) = super::worker::admission(store, &policy).await?;
    anyhow::ensure!(
        decision
            .event
            .decision
            .as_ref()
            .is_some_and(|decision| decision.outcome == Outcome::Allow)
    );
    admission.launch.authority_id = Uuid::new_v4().to_string();
    admission.launch.security_preset = SecurityPresetV1::FixedSandboxV2;
    let supervised = SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: content_hash("qa:prepared-worker-intent", &admission.operation_id)?,
        admission,
    };
    let outer_hash = supervised.input_hash()?;
    let operation_id = &supervised.admission.operation_id;
    let quota = if admitted {
        QuotaPolicyV1::default()
    } else {
        QuotaPolicyV1 {
            max_active_workers: 0,
            ..Default::default()
        }
    };
    let policy_hash = content_hash("ai-guard:admission-policy:v1", &(&policy, &quota))?;
    let deterministic = policy::evaluate(&policy, &supervised.admission.decision_request);

    let worker = if admitted {
        Some(reserved_worker(&supervised, namespace, &policy_hash)?)
    } else {
        None
    };
    let snapshot_hash = worker
        .as_ref()
        .map(|worker| content_hash("ai-guard:worker-snapshot:v1", worker))
        .transpose()?;
    let input = AdmissionInput {
        action: "admission",
        request: &supervised,
    };
    let mut event = Event::new("control.supervision.transition");
    event.session_id = worker
        .as_ref()
        .map(|worker| worker.launch.session_id.clone());
    event.data = json!({
        "version": 2,
        "input": input,
        "worker_record": {
            "version": 1,
            "operation_id": operation_id,
            "action": "admit",
            "input_hash": supervised.admission.input_hash()?,
            "previous_revision": null,
            "worker": worker,
            "snapshot_hash": snapshot_hash,
            "observation": null,
            "allocation": null,
            "refusal": (!admitted).then_some(AdmissionRefusalV1::Quota),
            "admission_policy": {
                "policy_hash": policy_hash, "deterministic": deterministic,
                "fresh": deterministic, "jev": null, "quota": quota
            }
        }
    });
    event.validate()?;
    let payload = serde_json::to_string(&event)?;
    let projection = worker
        .as_ref()
        .map(|worker| {
            serde_json::to_vec(&IntentBinding {
                worker_id: &worker.worker_id,
                intent_hash: &supervised.intent_hash,
                input: &input,
            })
        })
        .transpose()?;

    let mut tx = maintenance.start_transaction(TxOpts::new()).await?;
    let (seq, prev_hash, old_bytes): (i64, String, u64) = tx
        .query_first(
            "SELECT head_seq,head_hash,payload_bytes FROM ai_guard_audit_state WHERE singleton=1 FOR UPDATE",
        )
        .await?
        .context("audit head missing while preparing admission")?;
    let next_seq = seq.checked_add(1).context("audit sequence overflow")?;
    let received_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let hash = event_hash(next_seq, &received_at, &prev_hash, &payload);
    tx.exec_drop(
        "INSERT INTO ai_guard_audit_event (seq,event_id,received_at,prev_hash,hash,payload) VALUES (?,?,?,?,?,?)",
        (next_seq, &event.id, &received_at, &prev_hash, &hash, &payload),
    )
    .await?;
    if let (Some(worker), Some(snapshot_hash), Some(binding)) =
        (&worker, &snapshot_hash, &projection)
    {
        tx.exec_drop(
            "INSERT INTO ai_guard_worker (worker_id,operation_id,request_id,session_id,state,\
             quota_held,cpu_millis,memory_bytes,revision,snapshot,snapshot_hash,event_seq) \
             VALUES (?,?,?,?,'reserved',1,?,?,1,?,?,?)",
            (
                &worker.worker_id,
                operation_id,
                &worker.request_id,
                &worker.launch.session_id,
                worker.launch.cpu_millis,
                worker.launch.memory_bytes,
                serde_json::to_vec(worker)?,
                snapshot_hash,
                next_seq,
            ),
        )
        .await?;
        tx.exec_drop(
            "INSERT INTO ai_guard_worker_attempt (attempt_id,worker_id,authority_id,stable_name,volume_name,snapshot) \
             VALUES (?,?,?,?,?,?)",
            (&worker.attempt.attempt_id, &worker.worker_id, &worker.launch.authority_id,
             &worker.attempt.stable_name, &worker.attempt.volume_name, serde_json::to_vec(&worker.attempt)?),
        ).await?;
        tx.exec_drop(
            "INSERT INTO ai_guard_worker_supervision (worker_id,intent_hash,payload_json,event_seq) VALUES (?,?,?,?)",
            (&worker.worker_id, &supervised.intent_hash, binding, next_seq),
        )
        .await?;
    }
    tx.exec_drop(
        "INSERT INTO ai_guard_worker_mutation (operation_id,worker_id,action,input_hash,event_seq) VALUES (?,?,?,?,?)",
        (operation_id, worker.as_ref().map(|worker| &worker.worker_id), "supervised_admit", &outer_hash, next_seq),
    )
    .await?;
    tx.exec_drop(
        "UPDATE ai_guard_audit_state SET head_seq=?,head_hash=?,payload_bytes=? WHERE singleton=1",
        (next_seq, &hash, old_bytes + payload.len() as u64),
    )
    .await?;
    tx.commit().await?;

    let verified = store.verify().await?;
    anyhow::ensure!(verified.seq == next_seq && verified.hash == hash);
    let usage = store.worker_quota(&QuotaPolicyV1::default()).await?;
    let expected_usage = worker.as_ref().map_or((0, 0, 0), |worker| {
        (
            1,
            u64::from(worker.launch.cpu_millis),
            worker.launch.memory_bytes,
        )
    });
    anyhow::ensure!(
        (
            usage.held_workers,
            usage.held_cpu_millis,
            usage.held_memory_bytes
        ) == expected_usage
    );
    let events = store
        .events(&EventQuery {
            order: Some("asc".into()),
            limit: Some(64),
            ..Default::default()
        })
        .await?;
    let stored = events
        .iter()
        .find(|event| event.seq == next_seq)
        .context("prepared supervised admission is absent from audit history")?;
    anyhow::ensure!(stored.hash == hash && stored.event.data == event.data);

    let appended = store
        .append(Event::new("qa.prepared_admission.append"))
        .await?;
    anyhow::ensure!(appended.seq == next_seq + 1 && appended.prev_hash == hash);
    let preserved: Option<Vec<u8>> = maintenance
        .exec_first(
            "SELECT payload FROM ai_guard_audit_event WHERE seq=?",
            (next_seq,),
        )
        .await?;
    anyhow::ensure!(preserved.as_deref() == Some(payload.as_bytes()));

    if let (Some(worker), Some(binding)) = (&worker, &projection) {
        maintenance
            .exec_drop(
                "UPDATE ai_guard_worker_supervision SET payload_json=? WHERE worker_id=?",
                (b"{}".as_slice(), &worker.worker_id),
            )
            .await?;
        anyhow::ensure!(maintenance.affected_rows() == 1);
        let checked = store.verify().await;
        let rejected = store
            .append(Event::new("qa.prepared_admission.refused"))
            .await;
        maintenance
            .exec_drop(
                "UPDATE ai_guard_worker_supervision SET payload_json=? WHERE worker_id=?",
                (binding, &worker.worker_id),
            )
            .await?;
        anyhow::ensure!(maintenance.affected_rows() == 1);
        anyhow::ensure!(checked.is_err() && rejected.is_err());
    }
    anyhow::ensure!(store.verify().await?.hash == appended.hash);
    let final_usage = store.worker_quota(&QuotaPolicyV1::default()).await?;
    anyhow::ensure!(
        (
            final_usage.held_workers,
            final_usage.held_cpu_millis,
            final_usage.held_memory_bytes
        ) == expected_usage
    );
    Ok(())
}

fn reserved_worker(
    supervised: &SupervisedAdmissionV1,
    namespace: &str,
    policy_hash: &str,
) -> anyhow::Result<WorkerV1> {
    let admission = &supervised.admission;
    let id = Uuid::new_v4();
    let stable_name = format!(
        "aig-{}-{}",
        Uuid::parse_str(namespace)?.simple(),
        id.simple()
    );
    Ok(WorkerV1 {
        worker_id: id.to_string(),
        admission_operation_id: admission.operation_id.clone(),
        request_id: admission.decision_request.request_id.clone(),
        receipt: admission.receipt.clone(),
        spec_hash: admission.launch.spec_hash()?,
        launch: admission.launch.clone(),
        policy_hash: policy_hash.into(),
        state: WorkerStateV1::Reserved,
        quota_held: true,
        revision: 1,
        attempt: WorkerAttemptV1 {
            attempt_id: Uuid::new_v4().to_string(),
            volume_name: format!("{stable_name}-work"),
            stable_name,
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
}

fn event_hash(seq: i64, received: &str, previous: &str, payload: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"agentsandbox:audit:v1\0");
    digest.update(seq.to_be_bytes());
    for part in [received, previous, payload] {
        digest.update(part.as_bytes());
        digest.update([0]);
    }
    hex::encode(digest.finalize())
}
