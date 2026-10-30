//! Bounded, read-only normalization of untrusted provider JSONL.

use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use uuid::Uuid;

/// Limits apply before parsing and during projection; no line is executed.
pub const MAX_LINE_BYTES: usize = 64 * 1024;
pub const MAX_EVENTS_PER_CAPTURE: usize = 500;
const MAX_EVENTS_PER_LINE: usize = 32;
const MAX_TEXT_BYTES: usize = 4096;
const MAX_DATA_BYTES: usize = 12 * 1024;
const MAX_DEPTH: usize = 8;
const MAX_COLLECTION_ITEMS: usize = 32;
const USAGE_FIELDS: &[&str] = &[
    "input_tokens",
    "output_tokens",
    "cached_input_tokens",
    "cache_write_input_tokens",
    "cache_read_input_tokens",
    "cache_creation_input_tokens",
];

#[derive(Debug, Clone)]
pub struct AdapterContext {
    pub session_id: String,
    pub project_id: String,
    pub sandbox_id: String,
    pub agent_id: String,
    pub correlation_id: String,
    pub trace_id: String,
}

/// Parse one complete JSONL record without executing any provider content.
///
/// All generated events are declared/observed. Context identity comes from the
/// trusted caller, never from provider output. Unsupported and incomplete
/// records return an empty vector. This is not an enforcement hook.
pub fn parse_line(provider: &str, line: &str, context: &AdapterContext) -> Vec<Value> {
    normalize_line(provider, line, context, None)
}

/// Mask decoded JSON values before bounded projections can split a secret.
pub fn parse_redacted_line(
    provider: &str,
    line: &str,
    context: &AdapterContext,
    redactor: &crate::redaction::Redactor,
) -> Vec<Value> {
    normalize_line(provider, line, context, Some(redactor))
}

pub struct NormalizedCapture {
    pub events: Vec<Value>,
    pub unterminated_final_line_omitted: bool,
}

/// Keep complete JSONL records without masking the JSON syntax itself. When a
/// capture is truncated, its final undelimited line cannot prove a complete
/// record, even if the retained prefix happens to parse as JSON.
pub fn parse_redacted_capture(
    provider: &str,
    bytes: &[u8],
    truncated: bool,
    context: &AdapterContext,
    redactor: &crate::redaction::Redactor,
) -> NormalizedCapture {
    let omitted = truncated && !bytes.is_empty() && !bytes.ends_with(b"\n");
    let end = if omitted {
        bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |end| end + 1)
    } else {
        bytes.len()
    };
    let events = String::from_utf8_lossy(&bytes[..end])
        .lines()
        .flat_map(|line| parse_redacted_line(provider, line, context, redactor))
        .take(MAX_EVENTS_PER_CAPTURE)
        .collect();
    NormalizedCapture {
        events,
        unterminated_final_line_omitted: omitted,
    }
}

fn normalize_line(
    provider: &str,
    line: &str,
    context: &AdapterContext,
    redactor: Option<&crate::redaction::Redactor>,
) -> Vec<Value> {
    if line.len() > MAX_LINE_BYTES {
        return Vec::new();
    }
    let Ok(record) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    if !record.is_object() {
        return Vec::new();
    }
    let record = match redactor {
        Some(redactor) => redactor.value(&record),
        None => record,
    };
    match provider {
        "codex" => codex(&record, context),
        "claude" | "claude-code" => claude(&record, context),
        "generic" => generic(&record, context),
        _ => Vec::new(),
    }
}

fn text(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(bounded_text)
}

fn bounded_text(input: &str) -> String {
    if input.len() <= MAX_TEXT_BYTES {
        return input.to_owned();
    }
    const SUFFIX: &str = "[truncated]";
    let mut end = MAX_TEXT_BYTES - SUFFIX.len();
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &input[..end], SUFFIX)
}

fn bounded_value(value: &Value) -> Value {
    fn project(value: &Value, depth: usize, budget: &mut usize) -> Value {
        if depth >= MAX_DEPTH || *budget < 4 {
            return Value::Null;
        }
        match value {
            Value::String(s) => {
                let mut result = bounded_text(s);
                if result.len() > *budget {
                    let mut end = *budget;
                    while !result.is_char_boundary(end) {
                        end -= 1;
                    }
                    result.truncate(end);
                }
                *budget = budget.saturating_sub(result.len() + 2);
                Value::String(result)
            }
            Value::Array(items) => {
                *budget = budget.saturating_sub(2);
                let mut result = Vec::new();
                for item in items.iter().take(MAX_COLLECTION_ITEMS) {
                    if *budget < 4 {
                        break;
                    }
                    result.push(project(item, depth + 1, budget));
                    *budget = budget.saturating_sub(1);
                }
                Value::Array(result)
            }
            Value::Object(fields) => {
                *budget = budget.saturating_sub(2);
                let mut result = Map::new();
                for (key, item) in fields.iter().take(MAX_COLLECTION_ITEMS) {
                    if key.len() > 128 || *budget < key.len() + 8 {
                        continue;
                    }
                    *budget = budget.saturating_sub(key.len() + 4);
                    result.insert(key.clone(), project(item, depth + 1, budget));
                }
                Value::Object(result)
            }
            _ => {
                *budget = budget.saturating_sub(value.to_string().len());
                value.clone()
            }
        }
    }
    let mut budget = MAX_DATA_BYTES;
    let mut result = project(value, 0, &mut budget);
    if &result != value
        && let Value::Object(fields) = &mut result
    {
        fields.insert("adapter_payload_truncated".into(), json!(true));
    }
    // JSON escaping can expand control characters to six bytes. Check the
    // serialized form as well as the projection's allocation budget.
    if result.to_string().len() > MAX_DATA_BYTES {
        return json!({"adapter_payload_truncated":true});
    }
    result
}

fn event(provider: &str, kind: &str, data: Value, context: &AdapterContext) -> Value {
    json!({
        "id": Uuid::new_v4().to_string(),
        "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        "kind": kind,
        "source": format!("{provider}-adapter"),
        "provenance": "declared",
        "coverage": "observed",
        "session_id": context.session_id,
        "project_id": context.project_id,
        "sandbox_id": context.sandbox_id,
        "agent_id": context.agent_id,
        "parent_agent_id": null,
        "correlation_id": context.correlation_id,
        "trace_id": context.trace_id,
        "command": null,
        "decision": null,
        "result": null,
        "data": bounded_value(&data),
    })
}

fn usage(value: &Value) -> Option<Value> {
    let mut result = Map::new();
    for key in USAGE_FIELDS {
        if let Some(number) = value.get(*key).and_then(Value::as_u64) {
            result.insert((*key).to_owned(), json!(number));
        }
    }
    (!result.is_empty()).then_some(Value::Object(result))
}

fn codex(record: &Value, context: &AdapterContext) -> Vec<Value> {
    match record.get("type").and_then(Value::as_str) {
        Some("thread.started") => {
            let Some(id) = text(record, "thread_id") else {
                return Vec::new();
            };
            vec![event(
                "codex",
                "session.declared",
                json!({"state":"started", "provider_session_id":id}),
                context,
            )]
        }
        Some("turn.started") => vec![event(
            "codex",
            "session.declared",
            json!({"state":"turn.started"}),
            context,
        )],
        Some("turn.completed") => {
            let mut events = vec![event(
                "codex",
                "session.declared",
                json!({"state":"turn.completed"}),
                context,
            )];
            if let Some(counts) = record.get("usage").and_then(usage) {
                events.push(event(
                    "codex",
                    "usage.reported",
                    json!({"usage":counts,"scope":"provider-turn"}),
                    context,
                ));
            }
            events
        }
        Some("turn.failed") => vec![event(
            "codex",
            "session.declared",
            json!({"state":"turn.failed","error":record.get("error").and_then(|v|text(v,"message"))}),
            context,
        )],
        Some("error") => {
            let Some(message) = text(record, "message") else {
                return Vec::new();
            };
            vec![event(
                "codex",
                "session.declared",
                json!({"state":"error","error":message}),
                context,
            )]
        }
        Some("item.started" | "item.completed") => codex_item(record, context),
        _ => Vec::new(),
    }
}

fn codex_item(record: &Value, context: &AdapterContext) -> Vec<Value> {
    let Some(item) = record.get("item").filter(|v| v.is_object()) else {
        return Vec::new();
    };
    let Some(id) = text(item, "id") else {
        return Vec::new();
    };
    let completed = record["type"] == "item.completed";
    match item.get("type").and_then(Value::as_str) {
        Some("command_execution") => {
            let Some(command) = text(item, "command") else {
                return Vec::new();
            };
            let status = item.get("status").and_then(Value::as_str);
            if !matches!(status, Some("in_progress" | "completed" | "failed")) {
                return Vec::new();
            }
            let mut normalized = event(
                "codex",
                if completed {
                    "tool.result"
                } else {
                    "tool.requested"
                },
                json!({
                    "provider_item_id":id, "tool_name":"command_execution", "command_text":command, "reported_status":status
                }),
                context,
            );
            if completed {
                let mut result = Map::new();
                if let Some(exit) = item.get("exit_code").and_then(Value::as_i64) {
                    result.insert("exit_code".into(), json!(exit));
                }
                if let Some(output) = item.get("aggregated_output").and_then(Value::as_str) {
                    result.insert("output".into(), json!(bounded_text(output)));
                }
                normalized["result"] = Value::Object(result);
            }
            vec![normalized]
        }
        Some("file_change") if completed => {
            let Some(changes) = item.get("changes").and_then(Value::as_array) else {
                return Vec::new();
            };
            let selected: Vec<_> = changes
                .iter()
                .filter_map(|change| {
                    let path = text(change, "path")?;
                    let kind = change.get("kind")?.as_str()?;
                    matches!(kind, "add" | "update" | "delete")
                        .then(|| json!({"path":path,"kind":kind}))
                })
                .take(MAX_COLLECTION_ITEMS)
                .collect();
            if selected.is_empty() {
                return Vec::new();
            }
            vec![event(
                "codex",
                "file.declared",
                json!({"provider_item_id":id,"changes":selected,"reported_status":text(item,"status")}),
                context,
            )]
        }
        _ => Vec::new(),
    }
}

fn claude_metadata(record: &Value) -> Value {
    let mut data = Map::new();
    for (input, output) in [
        ("session_id", "provider_session_id"),
        ("parent_tool_use_id", "provider_parent_tool_use_id"),
    ] {
        if let Some(value) = text(record, input) {
            data.insert(output.into(), json!(value));
        }
    }
    if let Some(message) = record.get("message")
        && let Some(id) = text(message, "id")
    {
        data.insert("provider_message_id".into(), json!(id));
    }
    Value::Object(data)
}

fn claude(record: &Value, context: &AdapterContext) -> Vec<Value> {
    match record.get("type").and_then(Value::as_str) {
        Some("system") if record["subtype"] == "init" => {
            let Some(session) = text(record, "session_id") else {
                return Vec::new();
            };
            vec![event(
                "claude",
                "session.declared",
                json!({"state":"started", "provider_session_id":session,"model":text(record,"model"),"declared_cwd":text(record,"cwd")}),
                context,
            )]
        }
        Some("assistant" | "user") => claude_blocks(record, context),
        Some("result") => {
            let Some(subtype) = text(record, "subtype") else {
                return Vec::new();
            };
            let mut data = claude_metadata(record);
            data["reported_status"] = json!(subtype);
            if let Some(error) = record.get("is_error").and_then(Value::as_bool) {
                data["is_error"] = json!(error);
            }
            for key in ["duration_ms", "num_turns"] {
                if let Some(number) = record.get(key).and_then(Value::as_u64) {
                    data[key] = json!(number);
                }
            }
            let mut events = vec![event("claude", "session.declared", data, context)];
            if let Some(counts) = record.get("usage").and_then(usage) {
                let mut data = claude_metadata(record);
                data["usage"] = counts;
                data["scope"] = json!("provider-result");
                events.push(event("claude", "usage.reported", data, context));
            }
            events
        }
        _ => Vec::new(),
    }
}

fn claude_blocks(record: &Value, context: &AdapterContext) -> Vec<Value> {
    let Some(blocks) = record
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for block in blocks {
        if events.len() == MAX_EVENTS_PER_LINE {
            break;
        }
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") if record["type"] == "assistant" => {
                let (Some(id), Some(name), Some(input)) = (
                    text(block, "id"),
                    text(block, "name"),
                    block.get("input").filter(|v| v.is_object()),
                ) else {
                    continue;
                };
                let mut data = claude_metadata(record);
                data["provider_tool_use_id"] = json!(id);
                data["tool_name"] = json!(name);
                data["input"] = bounded_value(input);
                if let Some(command) = text(input, "command") {
                    data["command_text"] = json!(command);
                }
                events.push(event("claude", "tool.requested", data, context));
            }
            Some("tool_result") if record["type"] == "user" => {
                let Some(id) = text(block, "tool_use_id") else {
                    continue;
                };
                let mut data = claude_metadata(record);
                data["provider_tool_use_id"] = json!(id);
                let mut normalized = event("claude", "tool.result", data, context);
                let mut result = Map::new();
                if let Some(output) = block.get("content") {
                    result.insert("content".into(), bounded_value(output));
                }
                if let Some(error) = block.get("is_error").and_then(Value::as_bool) {
                    result.insert("is_error".into(), json!(error));
                }
                normalized["result"] = Value::Object(result);
                events.push(normalized);
            }
            _ => {}
        }
    }
    events
}

fn explicit_command(value: &Value) -> Option<Value> {
    let argv = value.get("argv")?.as_array()?;
    if argv.is_empty() || argv.len() > 64 {
        return None;
    }
    let mut total = 0_usize;
    for argument in argv {
        let argument = argument.as_str()?;
        total = total.checked_add(argument.len())?;
        if argument.len() > MAX_TEXT_BYTES || total > 8192 {
            return None;
        }
    }
    let cwd = match value.get("cwd") {
        Some(Value::String(cwd)) if !cwd.is_empty() && cwd.len() <= MAX_TEXT_BYTES => json!(cwd),
        _ => return None,
    };
    Some(json!({"argv":argv,"cwd":cwd}))
}

fn generic(record: &Value, context: &AdapterContext) -> Vec<Value> {
    if record["type"] != "agentsandbox.telemetry" || record["version"].as_u64() != Some(1) {
        return Vec::new();
    }
    let Some(kind) = record.get("kind").and_then(Value::as_str) else {
        return Vec::new();
    };
    if !matches!(
        kind,
        "tool.requested"
            | "tool.result"
            | "file.declared"
            | "usage.reported"
            | "agent.declared"
            | "session.declared"
    ) {
        return Vec::new();
    }
    let mut data = match record.get("data") {
        Some(data) if data.is_object() => bounded_value(data),
        None => json!({}),
        _ => return Vec::new(),
    };
    if kind == "agent.declared" {
        for (input, output) in [
            ("agent_id", "declared_agent_id"),
            ("parent_agent_id", "declared_parent_agent_id"),
        ] {
            if let Some(id) = text(record, input) {
                data[output] = json!(id);
            }
        }
    }
    if let Some(timestamp) = text(record, "timestamp") {
        data["declared_timestamp"] = json!(timestamp);
    }
    if let Some(decision) = text(record, "decision") {
        data["declared_decision"] = json!(decision);
    }
    if kind == "usage.reported" {
        let Some(counts) = record
            .get("data")
            .and_then(|v| v.get("usage"))
            .and_then(usage)
        else {
            return Vec::new();
        };
        data["usage"] = counts;
    }
    let mut normalized = event("generic", kind, data, context);
    if let Some(command) = record.get("command") {
        let Some(command) = explicit_command(command) else {
            return Vec::new();
        };
        normalized["command"] = command;
    }
    if kind == "tool.result"
        && let Some(result) = record.get("result").filter(|v| v.is_object())
    {
        normalized["result"] = bounded_value(result);
    }
    vec![normalized]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

    fn parse(provider: &str, value: Value) -> Vec<Value> {
        parse_line(provider, &value.to_string(), &context())
    }

    #[test]
    fn codex_fixture_preserves_declared_lifecycle_and_usage() {
        let events: Vec<_> = include_str!("../tests/adapters_fixtures/codex.jsonl")
            .lines()
            .flat_map(|line| parse_line("codex", line, &context()))
            .collect();
        let kinds: Vec<_> = events.iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            [
                "session.declared",
                "tool.requested",
                "tool.result",
                "file.declared",
                "session.declared",
                "usage.reported"
            ]
        );
        assert_eq!(events[2]["result"]["exit_code"], 0);
        assert_eq!(events[3]["data"]["changes"][0]["path"], "src/main.rs");
        assert_eq!(events[5]["data"]["usage"]["input_tokens"], 100);
        assert!(
            events
                .iter()
                .all(|e| e["provenance"] == "declared" && e["coverage"] == "observed")
        );
    }

    #[test]
    fn command_text_is_not_split_into_an_executable_command() {
        let text = "printf '%s' 'café ☕' | cat; $(printf ignored)";
        let events = parse(
            "codex",
            json!({"type":"item.started","item":{"id":"item-1","type":"command_execution","command":text,"status":"in_progress"}}),
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["data"]["command_text"], text);
        assert!(events[0]["command"].is_null());
    }

    #[test]
    fn claude_fixture_is_not_promoted_to_independent_observation() {
        let events: Vec<_> = include_str!("../tests/adapters_fixtures/claude.jsonl")
            .lines()
            .flat_map(|line| parse_line("claude", line, &context()))
            .collect();
        let kinds: Vec<_> = events.iter().map(|e| e["kind"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            [
                "session.declared",
                "tool.requested",
                "tool.result",
                "session.declared",
                "usage.reported"
            ]
        );
        assert_eq!(events[1]["data"]["tool_name"], "Bash");
        assert_eq!(events[2]["data"]["provider_tool_use_id"], "toolu-test");
        assert_eq!(events[4]["data"]["usage"]["output_tokens"], 42);
        assert!(events.iter().all(|e| e["provenance"] == "declared"));
    }

    #[test]
    fn parent_tool_id_is_not_mistaken_for_a_parent_agent() {
        let events = parse(
            "claude",
            json!({"type":"assistant","session_id":"provider-session","parent_tool_use_id":"toolu-parent","message":{"content":[{"type":"tool_use","id":"toolu-child","name":"Read","input":{"file_path":"/workspace/a.rs"}}]}}),
        );
        assert_eq!(events.len(), 1);
        assert!(events[0]["parent_agent_id"].is_null());
        assert_eq!(events[0]["agent_id"], "trusted-agent");
        assert_eq!(
            events[0]["data"]["provider_parent_tool_use_id"],
            "toolu-parent"
        );
    }

    #[test]
    fn a_requested_write_is_not_a_confirmed_file_modification() {
        let events = parse(
            "claude",
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"Write","input":{"file_path":"/workspace/a.txt","content":"hello"}}]}}),
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["kind"], "tool.requested");
    }

    #[test]
    fn generic_telemetry_requires_the_explicit_contract() {
        let events: Vec<_> = include_str!("../tests/adapters_fixtures/generic.jsonl")
            .lines()
            .flat_map(|line| parse_line("generic", line, &context()))
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["command"]["argv"], json!(["printf", "café ☕"]));
        assert_eq!(events[0]["command"]["cwd"], "/workspace");
        assert_eq!(events[1]["kind"], "agent.declared");
        assert_eq!(events[1]["data"]["declared_agent_id"], "worker-test");
        assert!(events[1]["parent_agent_id"].is_null());
        assert_eq!(events[1]["data"]["declared_parent_agent_id"], "root-test");
    }

    #[test]
    fn untrusted_records_cannot_spoof_source_identity_or_provenance() {
        let events = parse(
            "generic",
            json!({"type":"agentsandbox.telemetry","version":1,"kind":"session.declared","id":"forged","timestamp":"2000-01-01T00:00:00Z","source":"runner","provenance":"observed","coverage":"native","session_id":"forged","project_id":"forged","sandbox_id":"forged","agent_id":"forged","parent_agent_id":"forged","correlation_id":"forged","trace_id":"forged","data":{"state":"running"}}),
        );
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event["source"], "generic-adapter");
        assert_eq!(event["provenance"], "declared");
        assert_eq!(event["coverage"], "observed");
        assert_eq!(event["session_id"], "trusted-session");
        assert_eq!(event["project_id"], "trusted-project");
        assert_eq!(event["sandbox_id"], "trusted-sandbox");
        assert_eq!(event["agent_id"], "trusted-agent");
        assert_eq!(event["correlation_id"], "trusted-correlation");
        assert_eq!(event["trace_id"], "trusted-trace");
        assert!(event["parent_agent_id"].is_null());
        assert_ne!(event["id"], "forged");
        assert_ne!(event["timestamp"], "2000-01-01T00:00:00Z");
    }

    #[test]
    fn malformed_unknown_and_partial_records_produce_no_fake_events() {
        for line in [
            "",
            "not json",
            "{",
            "null",
            "[]",
            "42",
            "{}",
            "{\"type\":\"unknown\"}",
            "{\"type\":\"item.started\",\"item\":{}}",
        ] {
            assert!(parse_line("codex", line, &context()).is_empty(), "{line}");
        }
        assert!(parse("claude", json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"partial_json":"{broken"}}})).is_empty());
        assert!(
            parse(
                "unknown-provider",
                json!({"type":"thread.started","thread_id":"x"})
            )
            .is_empty()
        );
    }

    #[test]
    fn generic_wrong_version_and_invalid_argv_are_rejected() {
        assert!(
            parse(
                "generic",
                json!({"type":"agentsandbox.telemetry","version":2,"kind":"tool.requested"})
            )
            .is_empty()
        );
        assert!(
            parse(
                "generic",
                json!({"type":"tool.requested","command":{"argv":["echo"]}})
            )
            .is_empty()
        );
        assert!(parse("generic", json!({"type":"agentsandbox.telemetry","version":1,"kind":"tool.requested","command":{"argv":["echo",42]}})).is_empty());
        assert!(parse("generic", json!({"type":"agentsandbox.telemetry","version":1,"kind":"tool.requested","command":{"argv":["echo"]}})).is_empty());
    }

    #[test]
    fn every_fixture_event_conforms_to_the_gateway_event_model() {
        for (provider, fixture) in [
            (
                "codex",
                include_str!("../tests/adapters_fixtures/codex.jsonl"),
            ),
            (
                "claude",
                include_str!("../tests/adapters_fixtures/claude.jsonl"),
            ),
            (
                "generic",
                include_str!("../tests/adapters_fixtures/generic.jsonl"),
            ),
        ] {
            for line in fixture.lines() {
                for value in parse_line(provider, line, &context()) {
                    let event: crate::model::Event = serde_json::from_value(value).unwrap();
                    event.validate().unwrap();
                }
            }
        }
    }

    #[test]
    fn lines_over_64_kib_are_rejected_by_utf8_byte_size() {
        let line = json!({"type":"thread.started","thread_id":"☕".repeat(23000)}).to_string();
        assert!(line.len() > 64 * 1024);
        assert!(parse_line("codex", &line, &context()).is_empty());
    }

    #[test]
    fn provider_text_is_bounded_without_breaking_utf8() {
        let events = parse(
            "codex",
            json!({"type":"item.completed","item":{"id":"item","type":"command_execution","command":"echo","status":"completed","exit_code":0,"aggregated_output":"☕".repeat(9000)}}),
        );
        assert_eq!(events.len(), 1);
        let output = events[0]["result"]["output"].as_str().unwrap();
        assert!(output.len() <= 4096);
        assert!(output.ends_with("[truncated]"));
    }

    #[test]
    fn escaped_control_characters_cannot_exceed_the_json_payload_budget() {
        let events = parse(
            "generic",
            json!({"type":"agentsandbox.telemetry","version":1,"kind":"session.declared","data":{"payload":"\u{0000}".repeat(5000)}}),
        );
        assert_eq!(events.len(), 1);
        assert!(events[0]["data"].to_string().len() <= MAX_DATA_BYTES);
        assert_eq!(events[0]["data"]["adapter_payload_truncated"], true);
    }

    #[test]
    fn usage_accepts_only_reported_nonnegative_integer_counts() {
        let events = parse(
            "codex",
            json!({"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":-1,"cached_input_tokens":"not-a-number","invented_total":400}}),
        );
        let usage = events
            .iter()
            .find(|e| e["kind"] == "usage.reported")
            .unwrap();
        assert_eq!(usage["data"]["usage"], json!({"input_tokens":10}));
        assert!(usage["data"].get("cost").is_none());
    }

    #[test]
    fn partial_assistant_usage_is_not_counted_as_final_usage() {
        let events = parse(
            "claude",
            json!({"type":"assistant","message":{"usage":{"input_tokens":10,"output_tokens":5},"content":[{"type":"text","text":"hello"}]}}),
        );
        assert!(events.is_empty());
    }

    #[test]
    fn each_normalized_record_gets_a_fresh_uuid_and_utc_reception_time() {
        let a = parse("codex", json!({"type":"thread.started","thread_id":"t"}));
        let b = parse("codex", json!({"type":"thread.started","thread_id":"t"}));
        assert_eq!(a.len(), 1);
        assert_ne!(a[0]["id"], b[0]["id"]);
        assert!(uuid::Uuid::parse_str(a[0]["id"].as_str().unwrap()).is_ok());
        let timestamp = a[0]["timestamp"].as_str().unwrap();
        assert!(chrono::DateTime::parse_from_rfc3339(timestamp).is_ok());
        assert!(timestamp.ends_with('Z'));
    }

    #[test]
    fn one_line_cannot_amplify_into_unbounded_events() {
        let blocks: Vec<_> = (0..64)
            .map(|i| json!({"type":"tool_use","id":format!("t-{i}"),"name":"Read","input":{}}))
            .collect();
        let events = parse(
            "claude",
            json!({"type":"assistant","message":{"content":blocks}}),
        );
        assert_eq!(events.len(), 32);
    }

    #[test]
    fn unknown_file_change_kinds_are_not_normalized_as_real_changes() {
        let events = parse(
            "codex",
            json!({"type":"item.completed","item":{"id":"f","type":"file_change","status":"failed","changes":[{"path":"a","kind":"add"},{"path":"b","kind":"imaginary"}]}}),
        );
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0]["data"]["changes"],
            json!([{"path":"a","kind":"add"}])
        );
        assert_eq!(events[0]["data"]["reported_status"], "failed");
    }
}
