mod support;

use agentsandbox::{audit::Store, model::Event, redaction::Redactor};
use serde_json::json;

#[test]
fn freeform_command_secrets_are_redacted_too() {
    let r = Redactor::new(vec![]);
    for text in [
        "tool --password fixture-private-password --token fixture-private-token",
        "OPENAI_API_KEY=fixture-private-key tool",
        "export CUSTOM_SECRET='fixture-secret'",
        "curl --api-key \"fixture quoted key\"",
    ] {
        let out = r.text(text);
        assert!(!out.contains("fixture"), "leak in {out}");
    }
}

#[test]
fn declared_agent_data_cannot_promote_observation_coverage() {
    let d = support::private_tempdir();
    let store = Store::open(&d.path().join("audit.sqlite"), Redactor::new(vec![]), 100).unwrap();
    let mut start = Event::new("agent.started");
    start.agent_id = Some("test-agent".into());
    start.data = json!({"provider":"generic","compatibility":"observed"});
    store.append(start).unwrap();
    let mut claim = Event::new("agent.declared");
    claim.agent_id = Some("test-agent".into());
    claim.provenance = "declared".into();
    claim.data = json!({"compatibility":"native","model":"reported-model","status":"completed"});
    store.append(claim).unwrap();
    let agents = store.entities(false).unwrap();
    assert_eq!(agents[0]["compatibility"], "observed");
    assert_eq!(agents[0]["status"], "running");
    assert_eq!(agents[0]["field_provenance"]["model"], "declared");
}

#[test]
fn detected_corruption_latches_audit_closed() {
    let d = support::private_tempdir();
    let path = d.path().join("audit.sqlite");
    let store = Store::open(&path, Redactor::new(vec![]), 100).unwrap();
    store.append(Event::new("command.requested")).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE events SET hash='tampered'", []).unwrap();
    assert!(!store.integrity().unwrap().valid);
    assert!(store.append(Event::new("security.decision")).is_err());
}

#[test]
fn pending_counter_tracks_latest_decision_for_same_request() {
    let d = support::private_tempdir();
    let store = Store::open(&d.path().join("audit.sqlite"), Redactor::new(vec![]), 100).unwrap();
    let mut review = Event::new("security.decision");
    review.data = json!({"request_id":"r1"});
    review.decision = Some(agentsandbox::model::Decision::new(
        agentsandbox::model::Outcome::Review,
        "review",
        "review",
    ));
    store.append(review).unwrap();
    let mut allow = Event::new("security.decision");
    allow.data = json!({"request_id":"r1"});
    allow.decision = Some(agentsandbox::model::Decision::new(
        agentsandbox::model::Outcome::Allow,
        "approved",
        "approved",
    ));
    store.append(allow).unwrap();
    assert_eq!(store.overview().unwrap()["counts"]["pending"], 0);
}

#[test]
fn declared_hierarchy_is_projected_without_inflating_observed_counts() {
    let d = support::private_tempdir();
    let store = Store::open(&d.path().join("audit.sqlite"), Redactor::new(vec![]), 100).unwrap();
    let mut started = Event::new("agent.started");
    started.agent_id = Some("context-agent".into());
    started.session_id = Some("session-a".into());
    started.sandbox_id = Some("sandbox-a".into());
    store.append(started).unwrap();
    for (id, parent) in [
        ("main", None),
        ("tests", Some("main")),
        ("audit", Some("main")),
    ] {
        let mut e = Event::new("agent.declared");
        e.agent_id = Some("context-agent".into());
        e.session_id = Some("session-a".into());
        e.sandbox_id = Some("sandbox-a".into());
        e.provenance = "declared".into();
        e.source = "generic-adapter".into();
        e.data =
            json!({"provider_agent_id":id,"provider_parent_agent_id":parent,"state":"started"});
        store.append(e).unwrap();
    }
    let agents = store.entities(false).unwrap();
    assert_eq!(agents.len(), 4);
    assert_eq!(
        agents
            .iter()
            .filter(|e| e["provenance"] == "declared" && !e["parent_id"].is_null())
            .count(),
        2
    );
    let overview = store.overview().unwrap();
    assert_eq!(overview["counts"]["agents"], 1);
    assert_eq!(overview["counts"]["declared_agents"], 3);
    assert_eq!(overview["counts"]["declared_subagents"], 2);
}
