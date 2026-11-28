//! Live provider activity normalization at the public adapter seam.
//!
//! Fixtures are synthetic records written from the official schemas listed in
//! tests/adapters_fixtures/SOURCES.json. They are not recordings of an
//! authenticated session and qualify no client version. No runtime, network,
//! provider or credential is used.

use agentsandbox::{
    adapters::{
        AdapterContext, NormalizedRecord, RecordDisposition, normalize_hook_record,
        normalize_stdout_record,
    },
    model::Event,
    redaction::Redactor,
};
use serde_json::{Value, json};

fn context() -> AdapterContext {
    AdapterContext {
        session_id: "trusted-session".into(),
        project_id: "trusted-project".into(),
        sandbox_id: "trusted-sandbox".into(),
        agent_id: "trusted-agent".into(),
        correlation_id: "trusted-correlation".into(),
        trace_id: "trusted-trace".into(),
    }
}

fn stdout(provider: &str, fixture: &str) -> Vec<NormalizedRecord> {
    fixture
        .lines()
        .map(|line| normalize_stdout_record(provider, line, &context(), &Redactor::new(vec![])))
        .collect()
}

fn hooks(client: &str, fixture: &str) -> Vec<NormalizedRecord> {
    fixture
        .lines()
        .map(|line| normalize_hook_record(client, line, &context(), &Redactor::new(vec![])))
        .collect()
}

fn kinds(records: &[NormalizedRecord]) -> Vec<Vec<String>> {
    records
        .iter()
        .map(|record| {
            record
                .events
                .iter()
                .map(|event| event["kind"].as_str().unwrap().to_owned())
                .collect()
        })
        .collect()
}

fn flat(records: &[NormalizedRecord]) -> Vec<Value> {
    records
        .iter()
        .flat_map(|record| record.events.clone())
        .collect()
}

fn find<'a>(events: &'a [Value], kind: &str, field: &str, value: &str) -> &'a Value {
    events
        .iter()
        .find(|event| event["kind"] == kind && event["data"][field] == value)
        .unwrap_or_else(|| panic!("missing {kind} with {field}={value}"))
}

const CODEX_EXEC: &str = include_str!("adapters_fixtures/codex.exec.jsonl");
const CODEX_HOOKS: &str = include_str!("adapters_fixtures/codex.hooks.jsonl");
const CLAUDE_STREAM: &str = include_str!("adapters_fixtures/claude-code.stream.jsonl");
const CLAUDE_HOOKS: &str = include_str!("adapters_fixtures/claude-code.hooks.jsonl");
const ROOT_THREAD: &str = "019a0000-0000-7000-8000-00000000a001";
const CHILD_THREAD: &str = "019a0000-0000-7000-8000-00000000a002";
const CLAUDE_SESSION: &str = "c0de0000-0000-4000-8000-0000000000c1";
const CLAUDE_CHILD: &str = "a1b2c3d4e5f60718";

#[test]
fn codex_exec_json_maps_every_official_item_type_while_the_session_runs() {
    let records = stdout("codex", CODEX_EXEC);
    let expected: Vec<Vec<&str>> = vec![
        vec!["session.declared", "agent.declared"],
        vec!["session.declared"],
        vec!["reasoning.declared"],
        vec!["tool.requested"],
        vec!["tool.result"],
        vec!["tool.requested"],
        vec!["tool.result"],
        vec!["plan.declared"],
        vec!["plan.declared"],
        vec!["delegation.declared"],
        vec!["delegation.declared", "agent.declared"],
        vec!["file.declared"],
        vec!["tool.requested"],
        vec!["message.declared"],
        vec!["session.declared"],
        vec!["session.declared", "usage.reported"],
    ];
    assert_eq!(kinds(&records), expected);
    assert!(
        records
            .iter()
            .all(|record| record.disposition == RecordDisposition::Mapped)
    );
    let events = flat(&records);
    let root = find(&events, "agent.declared", "provider_agent_id", ROOT_THREAD);
    assert_eq!(root["data"]["agent_role"], "root");
    assert!(root["data"]["provider_parent_agent_id"].is_null());
    let reasoning = &events[3];
    assert_eq!(
        reasoning["data"]["text"],
        "Inspecting the test suite before running it."
    );
    let mcp = find(&events, "tool.requested", "tool_name", "mcp_tool_call");
    assert_eq!(mcp["data"]["mcp_server"], "docs");
    assert_eq!(mcp["data"]["mcp_tool"], "search");
    assert_eq!(mcp["data"]["input"], json!({"q":"hooks"}));
    let plan = events
        .iter()
        .filter(|event| event["kind"] == "plan.declared")
        .last()
        .unwrap();
    assert_eq!(
        plan["data"]["items"],
        json!([{"text":"Run tests","completed":true},{"text":"Fix failure","completed":false}])
    );
    let search = find(&events, "tool.requested", "tool_name", "web_search");
    assert_eq!(search["data"]["query"], "rust tokio timeout");
    let message = find(&events, "message.declared", "role", "assistant");
    assert_eq!(message["data"]["text"], "Tests pass after the fix.");
    let usage = events
        .iter()
        .find(|event| event["kind"] == "usage.reported")
        .unwrap();
    assert_eq!(usage["data"]["usage"]["reasoning_output_tokens"], 64);
}

#[test]
fn codex_spawn_declares_the_child_thread_under_its_sender() {
    let events = flat(&stdout("codex", CODEX_EXEC));
    let delegations: Vec<_> = events
        .iter()
        .filter(|event| event["kind"] == "delegation.declared")
        .collect();
    assert_eq!(delegations.len(), 2);
    let completed = delegations[1];
    assert_eq!(completed["data"]["delegation_tool"], "spawn_agent");
    assert_eq!(completed["data"]["provider_sender_agent_id"], ROOT_THREAD);
    assert_eq!(
        completed["data"]["provider_receiver_agent_ids"],
        json!([CHILD_THREAD])
    );
    assert_eq!(completed["data"]["prompt"], "Review the failing test");
    let child = find(&events, "agent.declared", "provider_agent_id", CHILD_THREAD);
    assert_eq!(child["data"]["provider_parent_agent_id"], ROOT_THREAD);
    assert_eq!(child["data"]["agent_role"], "subagent");
    assert_eq!(child["data"]["state"], "started");
    assert_eq!(child["data"]["reported_agent_status"], "running");
}

#[test]
fn codex_lag_warning_is_kept_as_a_provider_declared_loss() {
    let events = flat(&stdout("codex", CODEX_EXEC));
    let lag = events
        .iter()
        .find(|event| event["data"]["state"] == "error")
        .unwrap();
    assert_eq!(lag["data"]["provider_reported_loss"], true);
    assert_eq!(lag["provenance"], "declared");
}

#[test]
fn claude_stream_maps_messages_reasoning_tools_and_delegation() {
    let records = stdout("claude-code", CLAUDE_STREAM);
    let expected: Vec<Vec<&str>> = vec![
        vec!["session.declared", "agent.declared"],
        vec!["reasoning.declared", "message.declared", "tool.requested"],
        vec!["tool.result"],
        vec!["tool.requested", "delegation.declared"],
        vec!["message.declared"],
        vec!["tool.requested"],
        vec!["tool.result"],
        vec!["message.declared"],
        vec!["tool.result"],
        vec!["session.declared"],
        vec![],
        vec!["session.declared", "usage.reported"],
    ];
    assert_eq!(kinds(&records), expected);
    assert_eq!(records[10].disposition, RecordDisposition::Ignored);
    let events = flat(&records);
    let root = find(
        &events,
        "agent.declared",
        "provider_agent_id",
        CLAUDE_SESSION,
    );
    assert_eq!(root["data"]["agent_role"], "root");
    assert_eq!(root["data"]["model"], "claude-haiku-5-5");
    let init = &events[0];
    assert_eq!(init["data"]["declared_client_version"], "2.1.329");
    let reasoning = &events[2];
    assert_eq!(reasoning["data"]["text"], "I should list the files first.");
    assert!(reasoning["data"].get("signature").is_none());
    let delegation = events
        .iter()
        .find(|event| event["kind"] == "delegation.declared")
        .unwrap();
    assert_eq!(delegation["data"]["provider_tool_use_id"], "toolu_02");
    assert_eq!(delegation["data"]["delegation_tool"], "Agent");
    assert_eq!(delegation["data"]["agent_type"], "Explore");
    assert_eq!(delegation["data"]["description"], "Find tests");
    assert_eq!(delegation["data"]["prompt"], "Locate the unit tests");
    let child_read = find(&events, "tool.requested", "tool_name", "Read");
    assert_eq!(
        child_read["data"]["provider_parent_tool_use_id"],
        "toolu_02"
    );
    assert_eq!(child_read["data"]["provider_agent_id"], CLAUDE_CHILD);
    let child_message = events
        .iter()
        .find(|event| {
            event["kind"] == "message.declared" && event["data"]["text"] == "Tests live in tests/."
        })
        .unwrap();
    assert_eq!(child_message["data"]["role"], "assistant");
    assert_eq!(child_message["data"]["provider_agent_id"], CLAUDE_CHILD);
    let child_prompt = find(&events, "message.declared", "role", "user");
    assert_eq!(child_prompt["data"]["text"], "Locate the unit tests");
    assert_eq!(
        child_prompt["data"]["provider_parent_tool_use_id"],
        "toolu_02"
    );
}

#[test]
fn claude_hooks_map_tools_prompts_and_subagents_without_inventing_a_parent() {
    let records = hooks("claude-code", CLAUDE_HOOKS);
    let expected: Vec<Vec<&str>> = vec![
        vec!["session.declared", "agent.declared"],
        vec!["message.declared"],
        vec!["tool.requested"],
        vec!["tool.result"],
        vec!["agent.declared"],
        vec!["tool.requested"],
        vec!["tool.result"],
        vec!["tool.result"],
        vec!["agent.declared", "message.declared"],
        vec!["session.declared"],
        vec!["message.declared"],
        vec!["session.declared"],
    ];
    assert_eq!(kinds(&records), expected);
    let events = flat(&records);
    let prompt = find(&events, "message.declared", "role", "user");
    assert_eq!(prompt["data"]["text"], "Fix the failing test");
    assert_eq!(prompt["data"]["provider_hook_event"], "UserPromptSubmit");
    let bash = find(
        &events,
        "tool.requested",
        "provider_tool_use_id",
        "toolu_01",
    );
    assert_eq!(bash["data"]["command_text"], "ls");
    assert_eq!(bash["data"]["provider_hook_event"], "PreToolUse");
    let result = find(&events, "tool.result", "provider_tool_use_id", "toolu_01");
    assert_eq!(result["data"]["duration_ms"], 35);
    assert_eq!(result["result"]["content"]["stdout"], "README.md\nsrc");
    let failure = find(&events, "tool.result", "provider_tool_use_id", "toolu_04");
    assert_eq!(failure["result"]["is_error"], true);
    assert_eq!(failure["result"]["error"], "Exit code 1");
    assert_eq!(failure["provenance"], "declared");
    assert_eq!(failure["coverage"], "native");
    assert_eq!(failure["source"], "claude-adapter");
    assert!(failure["result"].get("content").is_none());
    let denied = find(&events, "tool.result", "provider_tool_use_id", "toolu_05");
    assert_eq!(denied["data"]["provider_hook_event"], "PermissionDenied");
    assert_eq!(denied["result"]["is_error"], true);
    assert_eq!(denied["result"]["reported_status"], "permission_denied");
    assert_eq!(denied["result"]["error"], "User denied the request");
    assert_eq!(denied["provenance"], "declared");
    assert_eq!(denied["coverage"], "native");
    assert_eq!(denied["source"], "claude-adapter");
    assert!(denied["result"].get("content").is_none());
    for tool_id in ["toolu_04", "toolu_05"] {
        let outcomes: Vec<_> = events
            .iter()
            .filter(|event| event["data"]["provider_tool_use_id"] == tool_id)
            .collect();
        assert_eq!(outcomes.len(), 1, "{tool_id} has only its reported outcome");
        assert_eq!(outcomes[0]["kind"], "tool.result");
        assert_eq!(outcomes[0]["result"]["is_error"], true);
    }
    let started = events
        .iter()
        .find(|event| {
            event["kind"] == "agent.declared"
                && event["data"]["state"] == "started"
                && event["data"]["provider_agent_id"] == CLAUDE_CHILD
        })
        .unwrap();
    assert_eq!(started["data"]["agent_type"], "Explore");
    assert!(
        started["data"].get("provider_parent_agent_id").is_none(),
        "a hook that names no parent must not clear or invent one"
    );
    let child_tool = find(
        &events,
        "tool.requested",
        "provider_tool_use_id",
        "toolu_03",
    );
    assert_eq!(child_tool["data"]["provider_agent_id"], CLAUDE_CHILD);
    let stopped = events
        .iter()
        .find(|event| event["kind"] == "agent.declared" && event["data"]["state"] == "completed")
        .unwrap();
    assert_eq!(stopped["data"]["provider_agent_id"], CLAUDE_CHILD);
    let child_result = find(&events, "message.declared", "text", "Tests live in tests/.");
    assert_eq!(child_result["data"]["provider_agent_id"], CLAUDE_CHILD);
    assert_eq!(child_result["data"]["role"], "assistant");
    assert_eq!(child_result["data"]["provider_hook_event"], "SubagentStop");
    let final_message = events
        .iter()
        .filter(|event| event["kind"] == "message.declared")
        .last()
        .unwrap();
    assert_eq!(final_message["data"]["text"], "Done.");
    assert_eq!(final_message["data"]["turn_final"], true);
    assert!(
        events
            .iter()
            .all(|event| !event.to_string().contains("transcript")),
        "worker transcript paths are not persisted"
    );
}

#[test]
fn codex_hooks_map_child_activity_absent_from_exec_json() {
    let records = hooks("codex", CODEX_HOOKS);
    let expected: Vec<Vec<&str>> = vec![
        vec!["session.declared", "agent.declared"],
        vec!["message.declared"],
        vec!["tool.requested"],
        vec!["tool.result"],
        vec!["agent.declared"],
        vec!["tool.requested"],
        vec!["agent.declared", "message.declared"],
        vec!["message.declared"],
        vec!["session.declared"],
    ];
    assert_eq!(kinds(&records), expected);
    let events = flat(&records);
    let child = find(&events, "tool.requested", "provider_tool_use_id", "call_2");
    assert_eq!(child["data"]["provider_agent_id"], CHILD_THREAD);
    assert_eq!(child["data"]["provider_turn_id"], "turn-2");
    assert_eq!(child["data"]["command_text"], "git diff");
    assert_eq!(child["source"], "codex-adapter");
    // Offline probe of Codex 0.170.0: the hook session id equals the root
    // thread id of exec --json, so SessionStart declares the same root agent.
    let root = find(&events, "agent.declared", "provider_agent_id", ROOT_THREAD);
    assert_eq!(root["data"]["agent_role"], "root");
}

#[test]
fn delegation_tools_seen_by_hooks_are_declared_delegations() {
    let claude = normalize_hook_record(
        "claude-code",
        &json!({"session_id":"s","hook_event_name":"PreToolUse","tool_name":"Agent","tool_use_id":"toolu_9","tool_input":{"subagent_type":"Explore","description":"d","prompt":"p"}}).to_string(),
        &context(),
        &Redactor::new(vec![]),
    );
    let kinds: Vec<_> = claude.events.iter().map(|e| e["kind"].clone()).collect();
    assert_eq!(
        kinds,
        [json!("tool.requested"), json!("delegation.declared")]
    );
    let codex = normalize_hook_record(
        "codex",
        &json!({"session_id":"s","turn_id":"t","hook_event_name":"PreToolUse","tool_name":"spawn_agent","tool_use_id":"call_9","tool_input":{"message":"review"}}).to_string(),
        &context(),
        &Redactor::new(vec![]),
    );
    let kinds: Vec<_> = codex.events.iter().map(|e| e["kind"].clone()).collect();
    assert_eq!(
        kinds,
        [json!("tool.requested"), json!("delegation.declared")]
    );
}

#[test]
fn every_provider_event_is_declared_native_and_keeps_the_trusted_context() {
    let mut events = flat(&stdout("codex", CODEX_EXEC));
    events.extend(flat(&stdout("claude-code", CLAUDE_STREAM)));
    events.extend(flat(&hooks("claude-code", CLAUDE_HOOKS)));
    events.extend(flat(&hooks("codex", CODEX_HOOKS)));
    // 19 Codex exec + 16 Claude stream + 14 Claude hook + 11 Codex hook events.
    assert_eq!(events.len(), 60);
    for value in events {
        assert_eq!(value["provenance"], "declared", "{value}");
        assert_eq!(value["coverage"], "native", "{value}");
        assert!(
            ["codex-adapter", "claude-adapter"].contains(&value["source"].as_str().unwrap()),
            "{value}"
        );
        assert_eq!(value["session_id"], "trusted-session");
        assert_eq!(value["agent_id"], "trusted-agent");
        assert!(value["parent_agent_id"].is_null());
        let event: Event = serde_json::from_value(value).unwrap();
        event.validate().unwrap();
    }
}

#[test]
fn hook_payload_secrets_are_masked_before_projection() {
    let secret = "a-generated-canary-for-hook-input-77";
    let record = normalize_hook_record(
        "claude-code",
        &json!({"session_id":"s","hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"toolu_1","tool_input":{"command":format!("curl -H 'X: {secret}' https://example.invalid")}}).to_string(),
        &context(),
        &Redactor::new(vec![secret.into()]),
    );
    assert_eq!(record.events.len(), 1);
    let text = record.events[0].to_string();
    assert!(!text.contains(secret));
    assert!(text.contains("[REDACTED]"));
}

#[test]
fn dispositions_distinguish_invalid_oversized_unknown_and_ignored_records() {
    let redactor = Redactor::new(vec![]);
    let check = |provider: &str, line: &str| {
        normalize_stdout_record(provider, line, &context(), &redactor).disposition
    };
    assert_eq!(check("codex", "not json"), RecordDisposition::Invalid);
    assert_eq!(check("codex", "[]"), RecordDisposition::Invalid);
    assert_eq!(
        check("codex", r#"{"type":"future.event"}"#),
        RecordDisposition::Unrecognized
    );
    assert_eq!(
        check("claude-code", r#"{"type":"stream_event","event":{}}"#),
        RecordDisposition::Ignored
    );
    let big = json!({"type":"thread.started","thread_id":"x".repeat(1024 * 1024 + 1)}).to_string();
    assert_eq!(check("codex", &big), RecordDisposition::Oversized);
    let large_but_bounded = json!({"type":"item.completed","item":{"id":"i","type":"agent_message","text":"é".repeat(200 * 1024)}}).to_string();
    let record = normalize_stdout_record("codex", &large_but_bounded, &context(), &redactor);
    assert_eq!(record.disposition, RecordDisposition::Mapped);
    let text = record.events[0]["data"]["text"].as_str().unwrap();
    assert!(text.len() <= 8 * 1024);
    assert_eq!(record.events[0]["data"]["text_truncated"], true);
    let hook = |payload: &str| normalize_hook_record("codex", payload, &context(), &redactor);
    assert_eq!(hook("{").disposition, RecordDisposition::Invalid);
    assert_eq!(
        hook(r#"{"session_id":"s"}"#).disposition,
        RecordDisposition::Unrecognized
    );
    assert_eq!(
        hook(r#"{"session_id":"s","hook_event_name":"PreModelRequest"}"#).disposition,
        RecordDisposition::Ignored
    );
    assert_eq!(
        normalize_hook_record(
            "unknown-client",
            r#"{"hook_event_name":"Stop"}"#,
            &context(),
            &redactor
        )
        .disposition,
        RecordDisposition::Unrecognized
    );
}

/// Real client output recorded offline (no network, no credential, no model
/// call; see SOURCES.json). Every record must be understood: mapped, or a
/// known type deliberately ignored, never unrecognized or invalid.
#[test]
fn recorded_offline_client_output_is_fully_recognized() {
    for (client, hooks_channel, fixture) in [
        (
            "claude-code",
            false,
            include_str!("adapters_fixtures/recorded/claude-code-2.1.329.offline.stream.jsonl"),
        ),
        (
            "claude-code",
            true,
            include_str!("adapters_fixtures/recorded/claude-code-2.1.329.offline.hooks.jsonl"),
        ),
        (
            "codex",
            false,
            include_str!("adapters_fixtures/recorded/codex-0.170.0.offline.exec.jsonl"),
        ),
        (
            "codex",
            true,
            include_str!("adapters_fixtures/recorded/codex-0.170.0.offline.hooks.jsonl"),
        ),
    ] {
        let records = if hooks_channel {
            hooks(client, fixture)
        } else {
            stdout(client, fixture)
        };
        assert!(!records.is_empty());
        for (index, record) in records.iter().enumerate() {
            assert!(
                matches!(
                    record.disposition,
                    RecordDisposition::Mapped | RecordDisposition::Ignored
                ),
                "{client} hooks={hooks_channel} record {index}: {:?}",
                record.disposition
            );
        }
    }
    let claude = flat(&stdout(
        "claude-code",
        include_str!("adapters_fixtures/recorded/claude-code-2.1.329.offline.stream.jsonl"),
    ));
    let init = &claude[0];
    assert_eq!(init["data"]["declared_client_version"], "2.1.329");
    let failure = claude
        .iter()
        .find(|event| event["kind"] == "message.declared")
        .unwrap();
    assert_eq!(failure["data"]["api_error_message"], true);
    let codex_exec = flat(&stdout(
        "codex",
        include_str!("adapters_fixtures/recorded/codex-0.170.0.offline.exec.jsonl"),
    ));
    let codex_hooks = flat(&hooks(
        "codex",
        include_str!("adapters_fixtures/recorded/codex-0.170.0.offline.hooks.jsonl"),
    ));
    let thread = codex_exec[0]["data"]["provider_session_id"]
        .as_str()
        .unwrap();
    let hook_session = codex_hooks[0]["data"]["provider_session_id"]
        .as_str()
        .unwrap();
    assert_eq!(
        thread, hook_session,
        "recorded hook session id is the root thread id"
    );
}
