//! Claude Code `--output-format stream-json` records (Agent SDK SDKMessage).
//!
//! Only complete messages are projected: `stream_event` deltas are ignored so a
//! secret split across fragments is never reconstructed or partially masked.
use super::{
    AdapterContext, MAX_EVENTS_PER_LINE, Mapping, bounded_message, bounded_value, native_event,
    text, usage,
};
use serde_json::{Map, Value, json};

const PROVIDER: &str = "claude";
/// Tool names that start a sub-agent run. `Task` is the historical name.
pub(super) const DELEGATION_TOOLS: &[&str] = &["Agent", "Task"];

fn metadata(record: &Value) -> Value {
    let mut data = Map::new();
    for (input, output) in [
        ("session_id", "provider_session_id"),
        ("parent_tool_use_id", "provider_parent_tool_use_id"),
        ("agent_id", "provider_agent_id"),
        ("uuid", "provider_uuid"),
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
    // A message the client synthesizes from an API failure, not model output
    // (recorded from Claude Code 2.1.329 with the network unavailable).
    if record.get("is_api_error_message").and_then(Value::as_bool) == Some(true) {
        data.insert("api_error_message".into(), json!(true));
        if let Some(error) = text(record, "api_error") {
            data.insert("api_error".into(), json!(error));
        }
    }
    Value::Object(data)
}

pub(super) fn record(record: &Value, context: &AdapterContext) -> Mapping {
    let emit = |kind: &str, data: Value| native_event(PROVIDER, kind, data, context);
    match record.get("type").and_then(Value::as_str) {
        Some("system") => match record.get("subtype").and_then(Value::as_str) {
            Some("init") => {
                let Some(session) = text(record, "session_id") else {
                    return Mapping::Unrecognized;
                };
                let model = text(record, "model");
                Mapping::Events(vec![
                    emit(
                        "session.declared",
                        json!({"state":"started","provider_session_id":session,"model":model,"declared_cwd":text(record,"cwd"),"declared_client_version":text(record,"claude_code_version"),"permission_mode":text(record,"permissionMode")}),
                    ),
                    emit(
                        "agent.declared",
                        json!({"provider_agent_id":session,"provider_parent_agent_id":null,"agent_role":"root","provider":"claude-code","model":model,"state":"started"}),
                    ),
                ])
            }
            Some("compact_boundary") => {
                let mut data = metadata(record);
                data["state"] = json!("compacted");
                if let Some(meta) = record.get("compact_metadata") {
                    data["trigger"] = json!(text(meta, "trigger"));
                    data["pre_tokens"] = json!(meta.get("pre_tokens").and_then(Value::as_u64));
                }
                Mapping::Events(vec![emit("session.declared", data)])
            }
            Some("api_retry") => {
                let mut data = metadata(record);
                data["state"] = json!("api_retry");
                for key in ["attempt", "max_retries", "retry_delay_ms", "error_status"] {
                    if let Some(number) = record.get(key).and_then(Value::as_u64) {
                        data[key] = json!(number);
                    }
                }
                Mapping::Events(vec![emit("session.declared", data)])
            }
            Some(_) => Mapping::Ignored,
            None => Mapping::Unrecognized,
        },
        Some(role @ ("assistant" | "user")) => blocks(record, role, context),
        Some("result") => {
            let Some(subtype) = text(record, "subtype") else {
                return Mapping::Unrecognized;
            };
            let mut data = metadata(record);
            data["state"] = json!("result");
            data["reported_status"] = json!(subtype);
            if let Some(error) = record.get("is_error").and_then(Value::as_bool) {
                data["is_error"] = json!(error);
            }
            for key in ["duration_ms", "num_turns"] {
                if let Some(number) = record.get(key).and_then(Value::as_u64) {
                    data[key] = json!(number);
                }
            }
            let mut events = vec![emit("session.declared", data)];
            if let Some(counts) = record.get("usage").and_then(usage) {
                let mut data = metadata(record);
                data["usage"] = counts;
                data["scope"] = json!("provider-result");
                events.push(emit("usage.reported", data));
            }
            Mapping::Events(events)
        }
        Some("stream_event" | "prompt_suggestion" | "rate_limit_event") => Mapping::Ignored,
        _ => Mapping::Unrecognized,
    }
}

fn blocks(record: &Value, role: &str, context: &AdapterContext) -> Mapping {
    let emit = |kind: &str, data: Value| native_event(PROVIDER, kind, data, context);
    let Some(content) = record.get("message").and_then(|m| m.get("content")) else {
        return Mapping::Unrecognized;
    };
    // A user message may carry its text directly instead of content blocks.
    if let Some(body) = content.as_str() {
        if body.is_empty() {
            return Mapping::Ignored;
        }
        let mut data = metadata(record);
        let (body, truncated) = bounded_message(body);
        data["role"] = json!(role);
        data["text"] = json!(body);
        data["text_truncated"] = json!(truncated);
        return Mapping::Events(vec![emit("message.declared", data)]);
    }
    let Some(blocks) = content.as_array() else {
        return Mapping::Unrecognized;
    };
    let mut events = Vec::new();
    for block in blocks {
        if events.len() >= MAX_EVENTS_PER_LINE {
            break;
        }
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let Some(body) = block
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                else {
                    continue;
                };
                let mut data = metadata(record);
                let (body, truncated) = bounded_message(body);
                data["role"] = json!(role);
                data["text"] = json!(body);
                data["text_truncated"] = json!(truncated);
                events.push(emit("message.declared", data));
            }
            Some("thinking") if role == "assistant" => {
                let Some(body) = block.get("thinking").and_then(Value::as_str) else {
                    continue;
                };
                let mut data = metadata(record);
                let (body, truncated) = bounded_message(body);
                data["text"] = json!(body);
                data["text_truncated"] = json!(truncated);
                events.push(emit("reasoning.declared", data));
            }
            Some("redacted_thinking") if role == "assistant" => {
                let mut data = metadata(record);
                data["redacted"] = json!(true);
                events.push(emit("reasoning.declared", data));
            }
            Some("tool_use") if role == "assistant" => {
                let (Some(id), Some(name), Some(input)) = (
                    text(block, "id"),
                    text(block, "name"),
                    block.get("input").filter(|v| v.is_object()),
                ) else {
                    continue;
                };
                let mut data = metadata(record);
                data["provider_tool_use_id"] = json!(id);
                data["tool_name"] = json!(name);
                data["input"] = bounded_value(input);
                if let Some(command) = text(input, "command") {
                    data["command_text"] = json!(command);
                }
                events.push(emit("tool.requested", data));
                if DELEGATION_TOOLS.contains(&name.as_str()) && events.len() < MAX_EVENTS_PER_LINE {
                    let mut data = metadata(record);
                    data["provider_tool_use_id"] = json!(id);
                    delegation_fields(&mut data, &name, input);
                    events.push(emit("delegation.declared", data));
                }
            }
            Some("tool_result") if role == "user" => {
                let Some(id) = text(block, "tool_use_id") else {
                    continue;
                };
                let mut data = metadata(record);
                data["provider_tool_use_id"] = json!(id);
                let mut normalized = emit("tool.result", data);
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
    if events.is_empty() {
        Mapping::Ignored
    } else {
        Mapping::Events(events)
    }
}

/// Shared by stream and hook records: declared delegation request fields.
pub(super) fn delegation_fields(data: &mut Value, tool: &str, input: &Value) {
    data["delegation_tool"] = json!(tool);
    if let Some(agent_type) = text(input, "subagent_type") {
        data["agent_type"] = json!(agent_type);
    }
    if let Some(description) = text(input, "description") {
        data["description"] = json!(description);
    }
    if let Some(prompt) = input.get("prompt").and_then(Value::as_str) {
        let (prompt, truncated) = bounded_message(prompt);
        data["prompt"] = json!(prompt);
        data["prompt_truncated"] = json!(truncated);
    }
}
