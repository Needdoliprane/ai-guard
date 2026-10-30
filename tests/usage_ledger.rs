mod support;

use agentsandbox::{
    audit::Store,
    control::*,
    model::*,
    policy::{self, Policy},
    redaction::Redactor,
};
use serde_json::json;

fn ready(store: &Store) -> WorkerV1 {
    let session = uuid::Uuid::new_v4().to_string();
    let original:DecisionRequest=serde_json::from_value(json!({"request_id":uuid::Uuid::new_v4().to_string(),"profile":"strict","command":{"argv":["id"],"cwd":"/workspace"},"session_id":session,"project_id":null,"sandbox_id":null,"agent_id":null,"correlation_id":null,"trace_id":null})).unwrap();
    let event = store
        .decision(
            &original,
            Some((policy::evaluate(&Policy::default(), &original), json!(null))),
        )
        .unwrap()
        .unwrap();
    let admission:AdmissionV1=serde_json::from_value(json!({"version":1,"operation_id":uuid::Uuid::new_v4().to_string(),"decision_request":original,"receipt":{"version":1,"request_id":original.request_id,"request_hash":policy::request_hash(&original),"phase":"initial","audit_seq":event.seq,"audit_hash":event.hash},"launch":{"version":1,"session_id":session,"command":original.command,"profile":"strict","runtime_kind":"docker","authority_id":"daemon-test","image_ref":"worker:test","image_id":format!("sha256:{}","a".repeat(64)),"network":"none","cpu_millis":2000,"memory_bytes":1073741824_u64,"pids_limit":256,"timeout_seconds":60,"security_preset":"fixed_sandbox_v1","restart_policy":"no","workspace_manifest_digest":"b".repeat(64),"workspace_import_policy_version":1,"provider":"generic","capture_output":false,"credential_binding_id":null,"credential_names":[]}})).unwrap();
    let worker = store
        .admit_worker(
            &admission,
            &Policy::default(),
            policy::evaluate(&Policy::default(), &original),
            json!(null),
            &QuotaPolicyV1::default(),
        )
        .unwrap()
        .worker
        .unwrap();
    let worker = transition(store, &worker, WorkerTransitionV1::Prepare);
    let worker = transition(store, &worker, WorkerTransitionV1::CreateIssued);
    let inspect = inspect(&worker);
    let worker = transition(
        store,
        &worker,
        WorkerTransitionV1::Created {
            inspection: inspect,
        },
    );
    transition(
        store,
        &worker,
        WorkerTransitionV1::StartClaim {
            emitter_id: uuid::Uuid::new_v4().to_string(),
        },
    )
}
fn inspect(worker: &WorkerV1) -> RuntimeInspectionV1 {
    RuntimeInspectionV1 {
        authority_id: worker.launch.authority_id.clone(),
        runtime_kind: worker.launch.runtime_kind,
        container_id: "c".repeat(64),
        stable_name: worker.attempt.stable_name.clone(),
        volume_name: worker.attempt.volume_name.clone(),
        image_id: worker.launch.image_id.clone(),
        spec_hash: worker.spec_hash.clone(),
        cpu_millis: 2000,
        memory_bytes: 1073741824,
        pids_limit: 256,
        network: NetworkV1::None,
        security_preset: SecurityPresetV1::FixedSandboxV1,
        restart_policy: RestartPolicyV1::No,
    }
}
fn transition(store: &Store, worker: &WorkerV1, transition: WorkerTransitionV1) -> WorkerV1 {
    store
        .mutate_worker(&WorkerMutationV1 {
            version: 1,
            operation_id: uuid::Uuid::new_v4().to_string(),
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            expected_revision: worker.revision,
            transition,
        })
        .unwrap()
        .worker
}
#[test]
fn observed_zero_interval_and_missing_interval_have_distinct_results() {
    for complete in [true, false] {
        let directory = support::private_tempdir();
        let store = Store::open(&directory.path().join("db"), Redactor::new(vec![]), 1000).unwrap();
        let worker = ready(&store);
        let observation = RuntimeObservationV1 {
            inspection: inspect(&worker),
            observed_at: "2026-10-29T10:00:00Z".into(),
            status: RuntimeStatusV1::Exited,
            running: Some(false),
            started_at: Some("2026-10-29T10:00:00Z".into()),
            finished_at: complete.then(|| "2026-10-29T10:00:00Z".into()),
            exit_code: Some(0),
            oom_killed: Some(false),
            restart_count: Some(0),
        };
        let worker = transition(&store, &worker, WorkerTransitionV1::Observe { observation });
        let page = store.worker_usage_page(None, 100).unwrap();
        if complete {
            assert!(!worker.quota_held);
            assert_eq!(page.len(), 1);
            assert_eq!(page[0].duration_ns, "0");
            assert_eq!(page[0].cpu_millicore_nanoseconds, "0");
            assert_eq!(page[0].memory_byte_nanoseconds, "0");
            assert!(!page[0].billable);
            assert_eq!(
                serde_json::to_value(&page[0]).unwrap()["source"]["container_id"],
                "c".repeat(64),
                "allocation API retains runtime provenance without requiring an inferred source"
            );
            assert!(
                store
                    .worker_usage_page(Some(&page[0].attempt_id), 100)
                    .unwrap()
                    .is_empty()
            );
        } else {
            assert!(worker.quota_held);
            assert_eq!(worker.state, WorkerStateV1::InDoubt);
            assert!(page.is_empty());
        }
        assert!(store.worker_usage_page(None, 0).is_err());
        assert!(store.worker_usage_page(None, 101).is_err());
        assert!(store.integrity().unwrap().valid);
    }
}

#[test]
fn deleting_a_durable_observation_is_detected_after_restart() {
    let directory = support::private_tempdir();
    let path = directory.path().join("db");
    let store = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    let worker = ready(&store);
    let worker = transition(
        &store,
        &worker,
        WorkerTransitionV1::Observe {
            observation: RuntimeObservationV1 {
                inspection: inspect(&worker),
                observed_at: "2026-10-29T10:00:02Z".into(),
                status: RuntimeStatusV1::Running,
                running: Some(true),
                started_at: Some("2026-10-29T10:00:00Z".into()),
                finished_at: None,
                exit_code: None,
                oom_killed: Some(false),
                restart_count: Some(0),
            },
        },
    );
    assert_eq!(worker.state, WorkerStateV1::Running);
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("DELETE FROM worker_observations", []).unwrap();
    drop(db);
    let reopened = Store::open(&path, Redactor::new(vec![]), 1000).unwrap();
    assert!(!reopened.healthy());
    assert!(reopened.workers_page(None, 100).is_err());
}
