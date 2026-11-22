//! Bounded, read-only normalization of untrusted provider JSONL and hooks.
//!
//! Every Claude Code and Codex record is a provider declaration (`declared`)
//! collected through an official channel (`native` coverage). Context identity
//! comes from the trusted caller, never from provider output. Nothing here
//! executes provider content or enforces a decision.

use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use uuid::Uuid;

mod claude;
mod codex;
mod hooks;

/// Limits apply before parsing and during projection; no line is executed.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
pub const MAX_EVENTS_PER_CAPTURE: usize = 500;
const MAX_EVENTS_PER_LINE: usize = 32;
const MAX_TEXT_BYTES: usize = 4096;
/// Message, reasoning and prompt text, measured on its JSON-escaped form.
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024;
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
    "reasoning_output_tokens",
];
/// Data fields bounded by `bounded_message` before the generic projection.
const MESSAGE_FIELDS: &[&str] = &["text", "prompt"];

#[derive(Debug, Clone)]
pub struct AdapterContext {
    pub session_id: String,
    pub project_id: String,
    pub sandbox_id: String,
    pub agent_id: String,
    pub correlation_id: String,
    pub trace_id: String,
}

/// Outcome class of one untrusted record, for loss and coverage accounting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordDisposition {
    /// At least one event was produced.
    Mapped,
    /// A known record type that is deliberately not projected.
    Ignored,
    /// A JSON object whose type or required fields are not recognized.
    Unrecognized,
    /// Larger than `MAX_LINE_BYTES`; not parsed.
    Oversized,
    /// Not UTF-8 JSON, or not a JSON object.
    Invalid,
}

pub struct NormalizedRecord {
    pub events: Vec<Value>,
    pub disposition: RecordDisposition,
}

/// Result of one provider mapping before the disposition is finalized.
enum Mapping {
    Events(Vec<Value>),
    Ignored,
    Unrecognized,
}

/// Parse one complete JSONL record without executing any provider content.
///
/// Unsupported and incomplete records return an empty vector. This is not an
/// enforcement hook.
pub fn parse_line(provider: &str, line: &str, context: &AdapterContext) -> Vec<Value> {
    normalize(provider, line, context, None).events
}

/// Mask decoded JSON values before bounded projections can split a secret.
pub fn parse_redacted_line(
    provider: &str,
    line: &str,
    context: &AdapterContext,
    redactor: &crate::redaction::Redactor,
) -> Vec<Value> {
    normalize(provider, line, context, Some(redactor)).events
}

/// Normalize one stdout record (Claude stream-json, Codex exec --json or the
/// generic contract), masked before projection, with its disposition.
pub fn normalize_stdout_record(
    provider: &str,
    line: &str,
    context: &AdapterContext,
    redactor: &crate::redaction::Redactor,
) -> NormalizedRecord {
    normalize(provider, line, context, Some(redactor))
}

/// Normalize one official hook payload of `claude-code` or `codex`, masked
/// before projection. Payloads up to `MAX_LINE_BYTES` are accepted; larger
/// fields are bounded by the projection rather than dropping the record.
pub fn normalize_hook_record(
    client: &str,
    payload: &str,
    context: &AdapterContext,
    redactor: &crate::redaction::Redactor,
) -> NormalizedRecord {
    let Some(client) = hooks::Client::parse(client) else {
        return finish(Mapping::Unrecognized);
    };
    match decode(payload, Some(redactor)) {
        Ok(record) => finish(hooks::record(client, &record, context)),
        Err(disposition) => NormalizedRecord {
            events: Vec::new(),
            disposition,
        },
    }
}

fn decode(
    input: &str,
    redactor: Option<&crate::redaction::Redactor>,
) -> Result<Value, RecordDisposition> {
    if input.len() > MAX_LINE_BYTES {
        return Err(RecordDisposition::Oversized);
    }
    let record = serde_json::from_str::<Value>(input).map_err(|_| RecordDisposition::Invalid)?;
    if !record.is_object() {
        return Err(RecordDisposition::Invalid);
    }
    Ok(match redactor {
        Some(redactor) => redactor.value(&record),
        None => record,
    })
}

fn finish(mapping: Mapping) -> NormalizedRecord {
    match mapping {
        Mapping::Events(events) if !events.is_empty() => NormalizedRecord {
            events,
            disposition: RecordDisposition::Mapped,
        },
        Mapping::Events(_) | Mapping::Ignored => NormalizedRecord {
            events: Vec::new(),
            disposition: RecordDisposition::Ignored,
        },
        Mapping::Unrecognized => NormalizedRecord {
            events: Vec::new(),
            disposition: RecordDisposition::Unrecognized,
        },
    }
}

fn normalize(
    provider: &str,
    line: &str,
    context: &AdapterContext,
    redactor: Option<&crate::redaction::Redactor>,
) -> NormalizedRecord {
    let record = match decode(line, redactor) {
        Ok(record) => record,
        Err(disposition) => {
            return NormalizedRecord {
                events: Vec::new(),
                disposition,
            };
        }
    };
    finish(match provider {
        "codex" => codex::record(&record, context),
        "claude" | "claude-code" => claude::record(&record, context),
        "generic" => {
            let events = generic(&record, context);
            if events.is_empty() {
                Mapping::Unrecognized
            } else {
                Mapping::Events(events)
            }
        }
        _ => Mapping::Unrecognized,
    })
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

/// Bound visible text on its JSON-escaped size and report truncation.
fn bounded_message(input: &str) -> (String, bool) {
    let mut used = 2;
    for (index, ch) in input.char_indices() {
        let escaped = match ch {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
            ch if (ch as u32) < 0x20 => 6,
            ch => ch.len_utf8(),
        };
        if used + escaped > MAX_MESSAGE_BYTES {
            return (input[..index].to_owned(), true);
        }
        used += escaped;
    }
    (input.to_owned(), false)
}

fn bounded_value(value: &Value) -> Value {
    bounded_value_within(value, MAX_DATA_BYTES)
}

fn bounded_value_within(value: &Value, limit: usize) -> Value {
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
    let mut budget = limit;
    let mut result = project(value, 0, &mut budget);
    if &result != value
        && let Value::Object(fields) = &mut result
    {
        fields.insert("adapter_payload_truncated".into(), json!(true));
    }
    // JSON escaping can expand control characters to six bytes. Check the
    // serialized form as well as the projection's allocation budget.
    if result.to_string().len() > limit {
        return json!({"adapter_payload_truncated":true});
    }
    result
}

fn envelope(
    provider: &str,
    kind: &str,
    coverage: &str,
    data: Value,
    context: &AdapterContext,
) -> Value {
    json!({
        "id": Uuid::new_v4().to_string(),
        "timestamp": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        "kind": kind,
        "source": format!("{provider}-adapter"),
        "provenance": "declared",
        "coverage": coverage,
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
        "data": data,
    })
}

/// Generic contract events: integration level `observed`, as before.
fn event(provider: &str, kind: &str, data: Value, context: &AdapterContext) -> Value {
    envelope(provider, kind, "observed", bounded_value(&data), context)
}

/// Official Claude Code / Codex channel events: `native` coverage. Message
/// fields were bounded by `bounded_message`; the rest shares the data budget.
fn native_event(provider: &str, kind: &str, data: Value, context: &AdapterContext) -> Value {
    let mut data = data;
    let mut kept = Map::new();
    if let Some(fields) = data.as_object_mut() {
        for key in MESSAGE_FIELDS {
            if let Some(value) = fields.remove(*key).filter(Value::is_string) {
                kept.insert((*key).to_owned(), value);
            }
        }
    }
    let reserved: usize = kept
        .iter()
        .map(|(key, value)| key.len() + value.to_string().len() + 4)
        .sum();
    let mut data = bounded_value_within(&data, MAX_DATA_BYTES.saturating_sub(reserved).max(256));
    if let Some(fields) = data.as_object_mut() {
        fields.extend(kept);
    }
    envelope(provider, kind, "native", data, context)
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
                "agent.declared",
                "tool.requested",
                "tool.result",
                "file.declared",
                "session.declared",
                "usage.reported"
            ]
        );
        assert_eq!(events[3]["result"]["exit_code"], 0);
        assert_eq!(events[4]["data"]["changes"][0]["path"], "src/main.rs");
        assert_eq!(events[6]["data"]["usage"]["input_tokens"], 100);
        assert!(
            events
                .iter()
                .all(|e| e["provenance"] == "declared" && e["coverage"] == "native")
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
                "agent.declared",
                "tool.requested",
                "tool.result",
                "session.declared",
                "usage.reported"
            ]
        );
        assert_eq!(events[2]["data"]["tool_name"], "Bash");
        assert_eq!(events[3]["data"]["provider_tool_use_id"], "toolu-test");
        assert_eq!(events[5]["data"]["usage"]["output_tokens"], 42);
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
    fn lines_over_1_mib_are_rejected_by_utf8_byte_size() {
        let line = json!({"type":"thread.started","thread_id":"☕".repeat(350_000)}).to_string();
        assert!(line.len() > 1024 * 1024);
        assert!(parse_line("codex", &line, &context()).is_empty());
        let bounded = json!({"type":"thread.started","thread_id":"☕".repeat(23000)}).to_string();
        assert!(bounded.len() > 64 * 1024);
        assert_eq!(parse_line("codex", &bounded, &context()).len(), 2);
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
        // The visible text is declared; its partial usage is never counted.
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["kind"], "message.declared");
        assert!(events.iter().all(|e| e["kind"] != "usage.reported"));
    }

    #[test]
    fn each_normalized_record_gets_a_fresh_uuid_and_utc_reception_time() {
        let a = parse("codex", json!({"type":"thread.started","thread_id":"t"}));
        let b = parse("codex", json!({"type":"thread.started","thread_id":"t"}));
        // The session declaration and its root agent declaration.
        assert_eq!(a.len(), 2);
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
