use super::*;
use crate::{
    control::{
        AdmissionV1, MutationReceiptV1, MutationResultV1, WorkerAttemptV1, WorkerMutationV1,
    },
    model::Event,
    runtime::ContainerObservation,
};
use serde_json::json;

#[test]
fn result_requires_acknowledged_exit_scan_and_cleanup() {
    let session = Uuid::new_v4();
    let worker_id = Uuid::new_v4().to_string();
    let attempt_id = Uuid::new_v4().to_string();
    let container_id = "a".repeat(64);
    let manifest_digest = "b".repeat(64);
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/broker_intent-managed-checkpoint.json"
    ))
    .unwrap();
    let admission = AdmissionV1::deserialize(&fixture["admission"]).unwrap();
    let mut launch = admission.launch;
    launch.session_id = session.to_string();
    launch.capture_output = true;
    let raw: ContainerObservation = serde_json::from_value(json!({
        "id":container_id, "name":"fixture-container", "image_id":launch.image_id,
        "restart_count":0,
        "process":{"status":"exited", "running":false, "exit_code":17, "oom_killed":false,
            "started_at":"2026-11-18T00:00:00Z", "finished_at":"2026-11-18T00:00:01Z"},
        "allocation":{"cpu":{"quota":launch.cpu_millis, "period":1000},
            "memory_bytes":launch.memory_bytes, "pids_limit":launch.pids_limit},
        "configuration_fingerprint":"c".repeat(64),
        "configuration_fingerprint_version":launch.security_preset.configuration_fingerprint_version(),
        "observed_at":"2026-11-18T00:00:02Z", "diagnostics":[]
    })).unwrap();
    let mut worker = WorkerV1 {
        worker_id: worker_id.clone(),
        admission_operation_id: admission.operation_id,
        request_id: admission.decision_request.request_id,
        receipt: admission.receipt,
        launch,
        spec_hash: "d".repeat(64),
        policy_hash: "e".repeat(64),
        state: WorkerStateV1::Terminal,
        quota_held: false,
        revision: 7,
        attempt: WorkerAttemptV1 {
            attempt_id: attempt_id.clone(),
            stable_name: raw.name.clone(),
            volume_name: "fixture-volume".into(),
            container_id: Some(container_id.clone()),
            created_inspection: None,
            create_operation_id: Some(Uuid::new_v4().to_string()),
            start_operation_id: Some(Uuid::new_v4().to_string()),
            start_emitter_id: Some(Uuid::new_v4().to_string()),
        },
        observed_started_at: Some("2026-11-18T00:00:00Z".into()),
        allocation_inconsistent: false,
        last_observation: None,
        unknown_reason: None,
        cleanup_removed: Some(true),
        cleanup_error: None,
    };
    let mut central = broker_observation_data(&worker, &raw).unwrap();
    central.observed_at = "2026-11-18T00:00:03Z".into();
    worker.attempt.created_inspection = Some(central.inspection.clone());
    worker.last_observation = Some(central);

    let cleanup = WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker_id.clone(),
        attempt_id: attempt_id.clone(),
        expected_revision: 6,
        transition: WorkerTransitionV1::CleanupResult {
            removed: true,
            error: None,
        },
    };
    let operations = [SupervisionOperationV1::WorkerMutation {
        response: Some(MutationResultV1 {
            receipt: MutationReceiptV1 {
                operation_id: cleanup.operation_id.clone(),
                worker_id: Some(worker_id.clone()),
                revision: Some(7),
                input_hash: crate::control::content_hash("ai-guard:worker-mutation:v1", &cleanup)
                    .unwrap(),
                audit_seq: 42,
                audit_hash: "f".repeat(64),
            },
            worker: worker.clone(),
            disposition: None,
        }),
        request: cleanup,
    }];
    let event = |kind, data| {
        let mut event = Event::new(kind);
        event.session_id = Some(session.to_string());
        event.data = data;
        OutboxEventV1::new(session, &"1".repeat(64), event).unwrap()
    };
    let result = event(
        "broker.batch.result",
        json!({
            "version":1, "worker_id":worker_id, "attempt_id":attempt_id, "observation":raw,
            "transport":{"exit_code":0, "duration_ms_decimal":"1000",
                "stdout":{"text":"fixture capture must stay private", "text_clipped":false, "truncated":false, "total_bytes":32},
                "stderr":{"text":"", "text_clipped":false, "truncated":false, "total_bytes":0}}
        }),
    );
    let scan = event(
        "broker.batch.scan",
        json!({
            "version":1, "worker_id":worker_id, "changed_files":2, "manifest_digest":manifest_digest
        }),
    );
    let result_acked = result.acknowledged(41).unwrap();
    let scan_acked = scan.acknowledged(43).unwrap();
    assert!(terminal(session, &worker, &operations, &[result, scan_acked.clone()]).is_none());
    assert!(terminal(session, &worker, &operations, &[result_acked.clone(), scan]).is_none());
    let events = [result_acked, scan_acked];
    assert!(terminal(session, &worker, &[], &events).is_none());

    let completed = terminal(session, &worker, &operations, &events).unwrap();
    assert_eq!(completed.exit_code, 17);
    assert_eq!(completed.changed_files, 2);
    assert_eq!(completed.result_audit_seq, 41);
    assert_eq!(completed.scan_audit_seq, 43);
    assert_eq!(completed.cleanup_audit_seq, 42);
    assert!(completed.output_captured);
    assert!(
        !serde_json::to_string(&completed)
            .unwrap()
            .contains("fixture capture")
    );
    worker.quota_held = true;
    assert!(terminal(session, &worker, &operations, &events).is_none());
}

#[tokio::test]
async fn result_waits_during_pending_preclaim_retirement_without_claiming_cancellation()
-> anyhow::Result<()> {
    use crate::broker::{
        enforcer::retirement::test_support::OriginalUnusedFixture,
        journal::retirement_live_fixture::{abandonment_event_data, live_fixture},
        writer::{BeginUnusedRetirement, WriterClient},
    };
    use std::time::Duration;

    let live = live_fixture();
    let operation = live.intent.operation_id();
    let (enforcer, client, slot) = OriginalUnusedFixture::register(
        live.journal.scope().installation_id(),
        live.registration.clone(),
    )
    .await?;
    let observed = Box::pin(async {
        let writer = WriterClient::start(live.journal.clone(), 8, Duration::from_secs(15))?;
        let binding = slot.full3_registration_binding()?;
        let arm = binding.arm_id();
        writer
            .bind_full3_registration(operation, live.head.clone(), binding)
            .await?;
        let command = Uuid::new_v4();
        let mut signal = abandonment_event_data(&live.intent, command, arm, client.epoch(), 0);
        signal.source = "ai-guard-broker".into();
        signal.coverage = "native".into();
        writer.enqueue_event(operation, signal.clone()).await?;
        let acknowledged = live.store.append(signal)?;
        writer
            .acknowledge_event(operation, command, acknowledged.seq)
            .await?;
        let unused = slot.revoke_unused(command).await?;
        let head = writer
            .load(operation)
            .await?
            .expect("accepted job")
            .head()
            .clone();
        let _pending = writer
            .begin_unused_retirement(BeginUnusedRetirement::new(operation, head, unused)?)
            .await?;
        let job = writer.load(operation).await?.expect("accepted job");
        assert!(job.broker_checkpoint_v5().is_some_and(|checkpoint| {
            checkpoint.retirements().len() == 1 && checkpoint.retirements()[0].finalized().is_none()
        }));
        let working = project(
            operation,
            Some(JournalStatusV1::Accepted(job)),
            (true, None),
        );
        assert!(matches!(working.outcome, JobResultOutcomeV1::Pending));
        let stopped = project(operation, writer.status(operation).await?, (false, None));
        assert!(matches!(
            stopped.outcome,
            JobResultOutcomeV1::Ambiguous {
                reason: BrokerErrorCodeV1::NeedsReconciliation
            }
        ));
        assert!(!working.cancel_requested && !stopped.cancel_requested);
        Ok(())
    })
    .await;
    enforcer.finish().await;
    observed
}
