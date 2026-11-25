use agentsandbox::{
    adapters::{AdapterContext, parse_line},
    declared_agents::DeclaredAgentProjection,
    model::Event,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn declared(session: &str, id: &str, parent: Value, state: Option<&str>) -> Event {
    let mut event = Event::new("agent.declared");
    event.source = "generic-adapter".into();
    event.provenance = "declared".into();
    event.session_id = Some(session.into());
    event.sandbox_id = Some("sandbox".into());
    event.agent_id = Some("runner-agent".into());
    event.project_id = Some("project".into());
    event.data = json!({"provider_agent_id":id,"provider_parent_agent_id":parent});
    if let Some(state) = state {
        event.data["state"] = json!(state);
    }
    event
}

fn project(events: &[Event]) -> Vec<Value> {
    let mut projection = DeclaredAgentProjection::default();
    for event in events {
        projection.observe(event);
    }
    projection.finish()
}

fn node<'a>(items: &'a [Value], id: &str) -> &'a Value {
    items
        .iter()
        .find(|item| item["provider_agent_id"] == id)
        .expect("declared agent should exist")
}

#[test]
fn explicit_children_resolve_after_parent_and_remain_declared() {
    let root = declared("session", "main", Value::Null, Some("started"));
    let child = declared("session", "worker-tests", json!("main"), Some("started"));
    let items = project(&[child, root]);
    assert_eq!(items.len(), 2);
    assert_eq!(
        node(&items, "worker-tests")["parent_id"],
        node(&items, "main")["id"]
    );
    assert_eq!(
        node(&items, "worker-tests")["context_agent_id"],
        "runner-agent"
    );
    assert!(
        items
            .iter()
            .all(|item| item["provenance"] == "declared" && item["compatibility"] == "observed")
    );
    assert_eq!(
        node(&items, "worker-tests")["field_provenance"]["status"],
        "declared"
    );
    assert_eq!(node(&items, "worker-tests")["status"], "running");
}

#[test]
fn parents_cannot_resolve_across_sessions_or_context_agents() {
    let root = declared("session-one", "main", Value::Null, Some("started"));
    let child = declared("session-two", "child", json!("main"), Some("started"));
    let mut other = declared("session-one", "other-child", json!("main"), Some("started"));
    other.agent_id = Some("different-runner-agent".into());
    let items = project(&[root, child, other]);
    assert_eq!(items.len(), 3);
    for id in ["child", "other-child"] {
        assert!(node(&items, id)["parent_id"].is_null());
        assert_eq!(node(&items, id)["parent_unresolved"], "main");
        assert!(node(&items, id)["hierarchy_warning"].is_string());
    }
}

#[test]
fn generic_parser_aliases_form_a_usable_tree_without_identity_replacement() {
    let context = AdapterContext {
        session_id: "session".into(),
        project_id: "project".into(),
        sandbox_id: "sandbox".into(),
        agent_id: "runner-agent".into(),
        correlation_id: "correlation".into(),
        trace_id: "trace".into(),
    };
    let lines = [
        json!({"type":"agentsandbox.telemetry","version":1,"kind":"agent.declared","agent_id":"main","data":{"state":"started"}}),
        json!({"type":"agentsandbox.telemetry","version":1,"kind":"agent.declared","agent_id":"child","parent_agent_id":"main","data":{"state":"completed"}}),
    ];
    let mut events = Vec::new();
    for line in lines {
        let values = parse_line("generic", &line.to_string(), &context);
        for value in values {
            let event: Event = serde_json::from_value(value).unwrap();
            assert_eq!(event.agent_id.as_deref(), Some("runner-agent"));
            assert!(event.parent_agent_id.is_none());
            events.push(event);
        }
    }
    let items = project(&events);
    assert_eq!(items.len(), 2);
    assert_eq!(
        node(&items, "child")["parent_id"],
        node(&items, "main")["id"]
    );
    assert_eq!(node(&items, "child")["status"], "completed");
}

#[test]
fn spoofed_provenance_coverage_or_output_ids_are_never_accepted() {
    let mut claim = declared("session", "child", Value::Null, Some("started"));
    claim.data["id"] = json!("runner-agent");
    claim.data["provenance"] = json!("observed");
    claim.data["compatibility"] = json!("native");
    claim.data["parent_id"] = json!("runner-agent");
    let mut wrong_provenance =
        declared("session", "not-a-declaration", Value::Null, Some("started"));
    wrong_provenance.provenance = "observed".into();
    let mut wrong_kind = declared(
        "session",
        "not-an-agent-event",
        Value::Null,
        Some("started"),
    );
    wrong_kind.kind = "tool.result".into();
    let items = project(&[claim, wrong_provenance, wrong_kind]);
    assert_eq!(items.len(), 1);
    assert_ne!(items[0]["id"], "runner-agent");
    assert!(items[0]["id"].as_str().unwrap().starts_with("declared-"));
    assert_eq!(items[0]["provenance"], "declared");
    assert_eq!(items[0]["compatibility"], "observed");
    assert!(items[0]["parent_id"].is_null());
}

#[test]
fn cycle_and_self_parent_links_are_removed_and_reported() {
    let items = project(&[
        declared("session", "a", json!("b"), Some("started")),
        declared("session", "b", json!("a"), Some("started")),
        declared("session", "self", json!("self"), Some("started")),
    ]);
    assert_eq!(items.len(), 3);
    for item in items {
        assert!(item["parent_id"].is_null());
        assert!(item["hierarchy_warning"].is_string());
        assert!(item["parent_unresolved"].is_null());
    }
}

#[test]
fn missing_parent_never_creates_a_phantom_agent() {
    let items = project(&[declared("session", "child", json!("unseen-parent"), None)]);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["parent_unresolved"], "unseen-parent");
    assert!(items[0]["parent_id"].is_null());
    assert_eq!(items[0]["status"], "unknown");
    assert!(items[0]["started_at"].is_null());
}

#[test]
fn stable_ids_do_not_depend_on_observation_order_or_metadata() {
    let root = declared("session", "main", Value::Null, Some("started"));
    let child = declared("session", "café-子", json!("main"), Some("started"));
    let first = project(&[root.clone(), child.clone()]);
    let mut changed = child;
    changed.data["name"] = json!("Updated name");
    let second = project(&[changed, root]);
    assert_eq!(
        node(&first, "café-子")["id"],
        node(&second, "café-子")["id"]
    );
    assert_eq!(
        node(&first, "café-子")["parent_id"],
        node(&second, "café-子")["parent_id"]
    );
}

#[test]
fn only_explicit_known_states_set_status_and_repeated_records_update() {
    let started = declared("session", "worker", Value::Null, Some("started"));
    let completed = declared("session", "worker", Value::Null, Some("completed"));
    let unknown = declared("session", "future-state", Value::Null, Some("unrecognized"));
    let items = project(&[started.clone(), completed, unknown]);
    assert_eq!(items.len(), 2);
    assert_eq!(node(&items, "worker")["status"], "completed");
    assert_eq!(node(&items, "worker")["started_at"], started.timestamp);
    assert_eq!(node(&items, "future-state")["status"], "unknown");
    assert!(node(&items, "future-state")["started_at"].is_null());
}

#[test]
fn invalid_or_conflicting_identifiers_do_not_get_normalized_into_valid_ids() {
    let mut too_long = declared("session", &"a".repeat(257), Value::Null, None);
    too_long.data["name"] = json!("Must not appear");
    let control = declared("session", "bad\nidentifier", Value::Null, None);
    let mut conflict = declared("session", "canonical", Value::Null, None);
    conflict.data["declared_agent_id"] = json!("different-alias");
    let mut missing_context = declared("session", "no-context", Value::Null, None);
    missing_context.session_id = None;
    assert!(project(&[too_long, control, conflict, missing_context]).is_empty());
}

#[test]
fn partial_updates_preserve_parent_but_explicit_null_and_conflict_clear_it() {
    let root = declared("session", "main", Value::Null, Some("started"));
    let child = declared("session", "child", json!("main"), Some("started"));
    let mut update = declared("session", "child", Value::Null, Some("failed"));
    update
        .data
        .as_object_mut()
        .unwrap()
        .remove("provider_parent_agent_id");
    let items = project(&[root.clone(), child.clone(), update.clone()]);
    assert_eq!(
        node(&items, "child")["parent_id"],
        node(&items, "main")["id"]
    );
    assert_eq!(node(&items, "child")["status"], "error");

    update.data["provider_parent_agent_id"] = Value::Null;
    let items = project(&[root.clone(), child.clone(), update.clone()]);
    assert!(node(&items, "child")["parent_id"].is_null());
    assert!(node(&items, "child")["hierarchy_warning"].is_null());

    update.data["provider_parent_agent_id"] = json!("main");
    update.data["declared_parent_agent_id"] = json!("different-parent");
    let items = project(&[root, child, update]);
    assert!(node(&items, "child")["parent_id"].is_null());
    assert!(node(&items, "child")["hierarchy_warning"].is_string());
    assert!(node(&items, "child")["parent_unresolved"].is_null());
}

#[test]
fn namespaces_include_sandbox_and_source_and_do_not_guess_a_context_parent() {
    let root = declared("session", "main", Value::Null, Some("started"));
    let mut other_sandbox = declared("session", "other-sandbox", json!("main"), None);
    other_sandbox.sandbox_id = Some("sandbox-two".into());
    let mut other_source = declared("session", "other-source", json!("main"), None);
    other_source.source = "another-adapter".into();
    let wrapper_parent = declared("session", "wrapper-child", json!("runner-agent"), None);
    let items = project(&[root, other_sandbox, other_source, wrapper_parent]);
    for (id, missing) in [
        ("other-sandbox", "main"),
        ("other-source", "main"),
        ("wrapper-child", "runner-agent"),
    ] {
        assert!(node(&items, id)["parent_id"].is_null());
        assert_eq!(node(&items, id)["parent_unresolved"], missing);
    }
}

#[test]
fn removing_cycle_edges_preserves_noncyclic_descendants() {
    let items = project(&[
        declared("session", "a", json!("b"), Some("started")),
        declared("session", "b", json!("a"), Some("started")),
        declared("session", "tail", json!("a"), Some("stopped")),
    ]);
    assert_eq!(node(&items, "tail")["parent_id"], node(&items, "a")["id"]);
    assert_eq!(node(&items, "tail")["status"], "stopped");
    assert!(node(&items, "tail")["hierarchy_warning"].is_null());
    assert!(node(&items, "a")["parent_id"].is_null());
    assert!(node(&items, "b")["parent_id"].is_null());
}

#[test]
fn legacy_claude_checkpoint_replay_preserves_canonical_bytes() {
    let mut parent = declared("session", "parent", Value::Null, Some("started"));
    parent.source = "claude-adapter".into();
    parent.coverage = "native".into();
    parent.timestamp = "2026-11-25T00:00:00.000Z".into();
    let mut call = parent.clone();
    call.kind = "delegation.declared".into();
    call.data = json!({
        "delegation_tool": "Agent",
        "provider_tool_use_id": "toolu_call",
        "provider_agent_id": "parent",
    });
    let mut activity = parent.clone();
    activity.kind = "message.declared".into();
    activity.data = json!({
        "provider_agent_id": "child",
        "provider_parent_tool_use_id": "toolu_call",
    });
    let mut child = parent.clone();
    child.data = json!({"provider_agent_id": "child"});

    let mut projection = DeclaredAgentProjection::default();
    for event in [parent, call, activity, child] {
        projection.observe(&event);
    }
    let checkpoint = serde_json::to_vec(&projection).unwrap();
    // The retention verifier replays archived events and hashes the snapshot
    // containing these bytes. V1 replay must preserve this exact encoding.
    assert_eq!(
        hex::encode(Sha256::digest(&checkpoint)),
        "95ea6f5e3561409572990d005ea55b62facbd196ce46e147a51bcd2c1c2a5e88"
    );
    let resumed: DeclaredAgentProjection = serde_json::from_slice(&checkpoint).unwrap();
    assert_eq!(serde_json::to_vec(&resumed).unwrap(), checkpoint);
}
