mod support;

use agentsandbox::{
    audit::{DecisionFailure, Store},
    model::{Decision, DecisionRequest, ExecutionModeV1, Outcome},
    policy::request_hash,
    redaction::Redactor,
};
use serde_json::json;

fn request() -> DecisionRequest {
    serde_json::from_value(json!({
        "request_id": uuid::Uuid::new_v4().to_string(),
        "profile": "observe",
        "command": {"argv": ["/usr/bin/id"], "cwd": "/workspace"},
        "project_id": null, "sandbox_id": null, "agent_id": null,
        "session_id": null, "correlation_id": null, "trace_id": null,
        "human_approved": false, "approval_hash": null,
        "execution_mode": "interactive_tty_v1"
    }))
    .unwrap()
}

#[test]
fn interactive_decision_mode_is_durable_and_replayed_without_another_event() {
    let directory = support::private_tempdir();
    let path = directory.path().join("audit.sqlite");
    let request = request();
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 100).unwrap();
    let decision = store
        .decision(
            &request,
            Some((
                Decision::new(Outcome::Allow, "test", "test decision"),
                json!(null),
            )),
        )
        .unwrap()
        .unwrap();
    assert_eq!(decision.event.data["execution_mode"], "interactive_tty_v1");
    assert_eq!(decision.event.data["request_hash"], request_hash(&request));
    let count = store.integrity().unwrap().event_count;
    drop(store);

    let reopened = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 100).unwrap();
    let repeated = reopened.decision(&request, None).unwrap().unwrap();
    assert_eq!(repeated.seq, decision.seq);
    assert_eq!(repeated.hash, decision.hash);
    assert_eq!(repeated.event.data["execution_mode"], "interactive_tty_v1");
    assert_eq!(reopened.integrity().unwrap().event_count, count);

    let mut batch = request;
    batch.execution_mode = None;
    assert!(matches!(
        reopened.decision(&batch, None).unwrap_err().downcast_ref(),
        Some(DecisionFailure::Conflict)
    ));
    assert_eq!(reopened.integrity().unwrap().event_count, count);
}

#[test]
fn approval_for_batch_cannot_authorize_interactive_mode() {
    let directory = support::private_tempdir();
    let store = Store::open_for_tenant(
        &directory.path().join("audit.sqlite"),
        "tenant-a",
        Redactor::new(vec![]),
        100,
    )
    .unwrap();
    let mut batch = request();
    batch.execution_mode = None;
    let batch_hash = request_hash(&batch);
    batch.execution_mode = Some(ExecutionModeV1::InteractiveTtyV1);
    assert_ne!(request_hash(&batch), batch_hash);
    batch.human_approved = true;
    batch.approval_hash = Some(batch_hash);
    let count = store.integrity().unwrap().event_count;
    assert!(matches!(
        store.decision(&batch, None).unwrap_err().downcast_ref(),
        Some(DecisionFailure::ApprovalRequired)
    ));
    assert_eq!(store.integrity().unwrap().event_count, count);
}

#[test]
fn a_mode_that_would_be_masked_cannot_become_a_durable_decision() {
    let directory = support::private_tempdir();
    let store = Store::open_for_tenant(
        &directory.path().join("audit.sqlite"),
        "tenant-a",
        Redactor::new(vec!["interactive_tty_v1".into()]),
        100,
    )
    .unwrap();
    let count = store.integrity().unwrap().event_count;
    assert!(matches!(
        store.decision(&request(), None).unwrap_err().downcast_ref(),
        Some(DecisionFailure::InvalidRequest)
    ));
    assert_eq!(store.integrity().unwrap().event_count, count);
}
