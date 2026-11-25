use super::*;
use crate::{
    broker::{checkpoint::SupervisionCheckpointV1, lifecycle::LocalCommandV1},
    control::{
        MutationReceiptV1, MutationResultV1, WorkerMutationV1, WorkerV1,
        egress_decision::EgressChannelOpeningRequestV1,
    },
    egress::ids::IdV1,
    runner::{
        broker_observation_data,
        egress_release::{EgressReleaseTargetV1, RetainedEgressOpeningV1},
    },
    runtime::{OwnedContainerSnapshot, RuntimeAuthority},
};
use serde_json::json;

fn running_checkpoint() -> (BrokerCheckpointV3, EgressReleaseTargetV1, MutationResultV1) {
    let mut managed: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/broker_intent-managed-checkpoint.json"
    ))
    .unwrap();
    let authority: RuntimeAuthority = serde_json::from_value(json!({
        "authority_id": Uuid::from_u128(5),
        "kind": "docker",
        "endpoint": "unix:///synthetic-engine.sock",
        "engine_id": "synthetic-engine",
        "podman": null,
    }))
    .unwrap();
    let container_id = "a".repeat(64);
    let image_id = format!("sha256:{}", "a".repeat(64));
    let started = "2026-11-25T10:15:30Z";
    let owned: OwnedContainerSnapshot = serde_json::from_value(json!({
        "handle": {
            "authority_fingerprint": authority.fingerprint(),
            "session": managed["admission"]["launch"]["session_id"],
            "operation": managed["admission"]["operation_id"],
            "id": container_id, "name": "worker-fixture",
            "configuration_fingerprint_version": 2,
        },
        "observation": {
            "id": container_id, "name": "worker-fixture", "image_id": image_id,
            "restart_count": 0, "process": {"status":"created", "running":false},
            "allocation": {"cpu":{"quota":2,"period":1}, "memory_bytes":1073741824, "pids_limit":256},
            "configuration_fingerprint": "b".repeat(64),
            "configuration_fingerprint_version": 2, "observed_at": started, "diagnostics": []
        },
    }))
    .unwrap();
    managed["admission"]["launch"]["profile"] = json!("observe");
    managed["admission"]["launch"]["authority_id"] = json!(authority.authority_id);
    managed["admission"]["launch"]["security_preset"] = json!("fixed_sandbox_v2");
    managed["admission"]["launch"]["egress"] =
        json!({"version":1,"profile":"broker_proxy_v1","proxy_port":3128});
    let mut worker: WorkerV1 = serde_json::from_value(json!({
        "worker_id": Uuid::from_u128(6),
        "admission_operation_id": managed["admission"]["operation_id"],
        "request_id": managed["admission"]["receipt"]["request_id"],
        "receipt": managed["admission"]["receipt"],
        "launch": managed["admission"]["launch"],
        "spec_hash": "c".repeat(64), "policy_hash": "d".repeat(64),
        "state": "running", "quota_held": true, "revision": 6,
        "allocation_inconsistent": false, "observed_started_at": started,
        "attempt": {"attempt_id": Uuid::from_u128(7), "stable_name":"worker-fixture",
            "volume_name":"worker-fixture-work", "container_id":container_id},
    }))
    .unwrap();
    worker.spec_hash = worker.launch.spec_hash().unwrap();
    let mut observed = owned.observation.clone();
    observed.process.status = Some("running".into());
    observed.process.running = Some(true);
    observed.process.started_at = Some(started.parse().unwrap());
    let observation = broker_observation_data(&worker, &observed).unwrap();
    worker.last_observation = Some(observation.clone());
    let target = EgressReleaseTargetV1::from_observation(
        "tenant-a", 3, &worker, &authority, &owned, &observed,
    )
    .unwrap();
    let request = WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::from_u128(8).to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: 5,
        transition: WorkerTransitionV1::Observe { observation },
    };
    let running = MutationResultV1 {
        receipt: MutationReceiptV1 {
            operation_id: request.operation_id.clone(),
            worker_id: Some(worker.worker_id.clone()),
            revision: Some(6),
            input_hash: crate::control::content_hash("ai-guard:worker-mutation:v1", &request)
                .unwrap(),
            audit_seq: 7,
            audit_hash: "e".repeat(64),
        },
        worker,
        disposition: None,
    };
    managed["binding"] = json!(authority);
    managed["owned"] = json!(owned);
    managed["worker"] = json!(running.worker);
    managed["pending"] = Value::Null;
    let runner = serde_json::from_value(json!({
        "version": 1, "current_request": managed["admission"]["decision_request"], "managed": managed,
    }))
    .unwrap();
    let supervision = SupervisionCheckpointV1::new(
        Uuid::from_u128(9),
        vec![],
        vec![SupervisionOperationV1::WorkerMutation {
            request,
            response: Some(running.clone()),
        }],
    )
    .unwrap();
    let broker = BrokerCheckpointV2::new(runner, supervision).unwrap();
    (
        BrokerCheckpointV3::new(broker, vec![]).unwrap(),
        target,
        running,
    )
}

#[test]
fn opening_key_roundtrips_and_evolves_only_after_its_exact_running_ack() {
    let (running_head, target, running) = running_checkpoint();
    let opening = EgressChannelOpeningRequestV1 {
        version: 1,
        observation_id: IdV1::random(),
        binding: target.binding(IdV1::random(), 1).unwrap(),
    };
    let retained =
        RetainedEgressOpeningV1::from_running(target.clone(), opening.clone(), &running).unwrap();
    let old_bytes = running_head.canonical_bytes().unwrap();
    let old_wire: Value = serde_json::from_slice(&old_bytes).unwrap();
    assert!(old_wire["broker"]["runner"].get("egress_opening").is_none());
    assert_eq!(
        BrokerCheckpointV3::from_bytes(&old_bytes)
            .unwrap()
            .canonical_bytes()
            .unwrap(),
        old_bytes
    );
    let mut open_wire = old_wire.clone();
    open_wire["broker"]["runner"]["egress_opening"] = json!(retained);
    let decode = |wire: &Value| BrokerCheckpointV3::from_bytes(&serde_json::to_vec(wire).unwrap());
    let restored = decode(&open_wire).unwrap();
    assert_eq!(
        restored
            .broker()
            .runner()
            .broker_context()
            .egress_opening
            .unwrap()
            .request(),
        &opening
    );
    let verify = |previous: BrokerCheckpointV3, next: BrokerCheckpointV3| {
        check_transition(
            &LoadedCheckpoint::V3(previous),
            &LoadedCheckpoint::V3(next),
            false,
            None,
        )
    };
    assert_eq!(verify(running_head.clone(), restored.clone()), Ok(()));
    assert_eq!(
        verify(restored.clone(), running_head.clone()),
        Err(JournalError::Conflict)
    );

    let mut replacement = open_wire.clone();
    replacement["broker"]["runner"]["egress_opening"]["request"]["observation_id"] =
        json!(IdV1::random());
    assert_eq!(
        verify(restored.clone(), decode(&replacement).unwrap()),
        Err(JournalError::Conflict)
    );
    let mut stale = open_wire.clone();
    stale["broker"]["runner"]["egress_opening"]["worker_revision"] = json!(5);
    assert_eq!(
        verify(running_head.clone(), decode(&stale).unwrap()),
        Err(JournalError::Conflict)
    );
    for (field, wrong) in [("epoch", json!(2)), ("attempt_id", json!(IdV1::random()))] {
        let mut wrong_binding = open_wire.clone();
        wrong_binding["broker"]["runner"]["egress_opening"]["request"]["binding"][field] = wrong;
        assert!(decode(&wrong_binding).is_err());
    }
    let mut pending = old_wire.clone();
    pending["broker"]["supervision"]["operations"][0]["response"] = Value::Null;
    let pending_head = decode(&pending).unwrap();
    pending["broker"]["runner"]["egress_opening"] = json!(retained);
    assert_eq!(
        verify(pending_head.clone(), restored.clone()),
        Err(JournalError::Conflict)
    );
    assert_eq!(
        verify(pending_head, decode(&pending).unwrap()),
        Err(JournalError::Conflict)
    );

    let cancel = LocalCommandV1::new(
        Uuid::parse_str(&running.worker.admission_operation_id).unwrap(),
        &"f".repeat(64),
        Uuid::new_v4(),
        LocalCommandRequestV1::CancelRequested,
    )
    .unwrap();
    let mut stopped = open_wire.clone();
    stopped["commands"] = json!([cancel]);
    assert_eq!(
        verify(running_head.clone(), decode(&stopped).unwrap()),
        Err(JournalError::Conflict)
    );
    assert_eq!(verify(restored.clone(), decode(&stopped).unwrap()), Ok(()));

    let mut released = open_wire.clone();
    released["broker"]["runner"]["egress_release"] = json!({
        "version": 1, "target": target, "worker_revision": 6, "running_audit_seq": 7,
        "opening": {"version": 1, "binding": opening.binding,
            "observation_id": opening.observation_id, "disposition": "fresh",
            "receipt": {"audit_seq": 8, "audit_hash": "f".repeat(64)}},
    });
    let release_head = decode(&released).unwrap();
    let expected: crate::runner::egress_release::EgressReleaseIntentV1 =
        serde_json::from_value(released["broker"]["runner"]["egress_release"].clone()).unwrap();
    let verify_intent = |checkpoint| {
        super::super::egress_release_fence::require_release_intent(
            &LoadedCheckpoint::V3(checkpoint),
            &expected,
        )
    };
    assert_eq!(verify_intent(release_head.clone()), Ok(()));
    assert_eq!(verify_intent(restored.clone()), Err(JournalError::Conflict));

    let mut different_intent = released.clone();
    different_intent["broker"]["runner"]["egress_release"]["opening"]["receipt"]["audit_hash"] =
        json!("a".repeat(64));
    assert_eq!(
        verify_intent(decode(&different_intent).unwrap()),
        Err(JournalError::Conflict)
    );
    let mut release_without_key = released.clone();
    release_without_key["broker"]["runner"]
        .as_object_mut()
        .unwrap()
        .remove("egress_opening");
    assert!(decode(&release_without_key).is_err());
    assert_eq!(
        verify(running_head, release_head.clone()),
        Err(JournalError::Conflict)
    );
    assert_eq!(verify(restored.clone(), release_head.clone()), Ok(()));
    assert_eq!(
        verify(release_head, restored.clone()),
        Err(JournalError::Conflict)
    );
    let mut release_after_stop = released.clone();
    release_after_stop["commands"] = stopped["commands"].clone();
    assert_eq!(
        verify_intent(decode(&release_after_stop).unwrap()),
        Err(JournalError::Conflict)
    );
    assert_eq!(
        verify(
            decode(&stopped).unwrap(),
            decode(&release_after_stop).unwrap()
        ),
        Err(JournalError::Conflict)
    );
    released["broker"]["runner"]["egress_release"]["opening"]["observation_id"] =
        json!(IdV1::random());
    assert!(decode(&released).is_err());
}
