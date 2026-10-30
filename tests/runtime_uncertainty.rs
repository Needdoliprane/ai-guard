mod support;

use agentsandbox::{audit::Store, model::Event, redaction::Redactor};

#[test]
fn runtime_failure_does_not_present_a_missing_terminal_observation_as_known() {
    let temporary = support::private_tempdir();
    let store = Store::open(
        &temporary.path().join("audit.sqlite"),
        Redactor::new(vec![]),
        20,
    )
    .unwrap();
    let mut started = Event::new("command.started");
    started.sandbox_id = Some("sandbox".into());
    started.agent_id = Some("agent".into());
    store.append(started.clone()).unwrap();
    let mut uncertain = started.clone();
    uncertain.id = uuid::Uuid::new_v4().to_string();
    uncertain.kind = "runtime.execution.error".into();
    store.append(uncertain).unwrap();
    for sandbox in [true, false] {
        assert_eq!(store.entities(sandbox).unwrap()[0]["status"], "unknown");
    }
    let mut completed = started;
    completed.id = uuid::Uuid::new_v4().to_string();
    completed.kind = "command.completed".into();
    store.append(completed.clone()).unwrap();
    completed.id = uuid::Uuid::new_v4().to_string();
    completed.kind = "runtime.cleanup.failed".into();
    store.append(completed).unwrap();
    for sandbox in [true, false] {
        assert_eq!(store.entities(sandbox).unwrap()[0]["status"], "completed");
    }
}
