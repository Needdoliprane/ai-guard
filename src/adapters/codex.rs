//! Codex CLI `exec --json` records (codex-rs/exec/src/exec_events.rs).
//!
//! Only the primary thread is emitted by Codex: child-agent commands are absent
//! from this stream and reach ai-guard through hooks, when configured.
use super::{
    AdapterContext, MAX_COLLECTION_ITEMS, Mapping, bounded_message, bounded_text, bounded_value,
    native_event, text, usage,
};
use serde_json::{Map, Value, json};

const PROVIDER: &str = "codex";
/// Codex reports a lagged in-process stream through an error item.
const LAG_PREFIX: &str = "in-process app-server event stream lagged";

pub(super) fn record(record: &Value, context: &AdapterContext) -> Mapping {
    let emit = |kind: &str, data: Value| native_event(PROVIDER, kind, data, context);
    match record.get("type").and_then(Value::as_str) {
        Some("thread.started") => {
            let Some(id) = text(record, "thread_id") else {
                return Mapping::Unrecognized;
            };
            Mapping::Events(vec![
                emit(
                    "session.declared",
                    json!({"state":"started","provider_session_id":id}),
                ),
                emit(
                    "agent.declared",
                    json!({"provider_agent_id":id,"provider_parent_agent_id":null,"agent_role":"root","provider":"codex","state":"started"}),
                ),
            ])
        }
        Some("turn.started") => Mapping::Events(vec![emit(
            "session.declared",
            json!({"state":"turn.started"}),
        )]),
        Some("turn.completed") => {
            let mut events = vec![emit("session.declared", json!({"state":"turn.completed"}))];
            if let Some(counts) = record.get("usage").and_then(usage) {
                events.push(emit(
                    "usage.reported",
                    json!({"usage":counts,"scope":"provider-turn"}),
                ));
            }
            Mapping::Events(events)
        }
        Some("turn.failed") => Mapping::Events(vec![emit(
            "session.declared",
            json!({"state":"turn.failed","error":record.get("error").and_then(|v|text(v,"message"))}),
        )]),
        Some("error") => match text(record, "message") {
            Some(message) => Mapping::Events(vec![emit("session.declared", error_data(&message))]),
            None => Mapping::Unrecognized,
        },
        Some(kind @ ("item.started" | "item.updated" | "item.completed")) => {
            item(record, kind, context)
        }
        _ => Mapping::Unrecognized,
    }
}

fn error_data(message: &str) -> Value {
    let mut data = json!({"state":"error","error":message});
    if message.starts_with(LAG_PREFIX) {
        data["provider_reported_loss"] = json!(true);
    }
    data
}

fn item(record: &Value, phase: &str, context: &AdapterContext) -> Mapping {
    let emit = |kind: &str, data: Value| native_event(PROVIDER, kind, data, context);
    let Some(item) = record.get("item").filter(|v| v.is_object()) else {
        return Mapping::Unrecognized;
    };
    let Some(id) = text(item, "id") else {
        return Mapping::Unrecognized;
    };
    let completed = phase == "item.completed";
    let started = phase == "item.started";
    let status = item.get("status").and_then(Value::as_str);
    let Some(kind) = item.get("type").and_then(Value::as_str) else {
        return Mapping::Unrecognized;
    };
    // Codex documents item.updated for to-do lists only.
    if phase == "item.updated" && kind != "todo_list" {
        return Mapping::Ignored;
    }
    match kind {
        "agent_message" | "reasoning" => {
            if !completed {
                return Mapping::Ignored;
            }
            let Some(body) = item.get("text").and_then(Value::as_str) else {
                return Mapping::Unrecognized;
            };
            let (body, truncated) = bounded_message(body);
            let (event_kind, mut data) = if kind == "agent_message" {
                ("message.declared", json!({"role":"assistant"}))
            } else {
                ("reasoning.declared", json!({}))
            };
            data["provider_item_id"] = json!(id);
            data["text"] = json!(body);
            data["text_truncated"] = json!(truncated);
            Mapping::Events(vec![emit(event_kind, data)])
        }
        "command_execution" => {
            let Some(command) = text(item, "command") else {
                return Mapping::Unrecognized;
            };
            if !matches!(
                status,
                Some("in_progress" | "completed" | "failed" | "declined")
            ) {
                return Mapping::Unrecognized;
            }
            if !started && !completed {
                return Mapping::Ignored;
            }
            let mut normalized = emit(
                if completed {
                    "tool.result"
                } else {
                    "tool.requested"
                },
                json!({
                    "provider_item_id":id, "tool_name":"command_execution", "command_text":command, "reported_status":status
                }),
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
            Mapping::Events(vec![normalized])
        }
        "file_change" => {
            if !completed {
                return Mapping::Ignored;
            }
            let Some(changes) = item.get("changes").and_then(Value::as_array) else {
                return Mapping::Unrecognized;
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
                return Mapping::Unrecognized;
            }
            Mapping::Events(vec![emit(
                "file.declared",
                json!({"provider_item_id":id,"changes":selected,"reported_status":text(item,"status")}),
            )])
        }
        "mcp_tool_call" => {
            let (Some(server), Some(tool)) = (text(item, "server"), text(item, "tool")) else {
                return Mapping::Unrecognized;
            };
            if !started && !completed {
                return Mapping::Ignored;
            }
            let mut data = json!({"provider_item_id":id,"tool_name":"mcp_tool_call","mcp_server":server,"mcp_tool":tool,"reported_status":status});
            if let Some(arguments) = item.get("arguments").filter(|v| !v.is_null()) {
                data["input"] = bounded_value(arguments);
            }
            let mut normalized = emit(
                if completed {
                    "tool.result"
                } else {
                    "tool.requested"
                },
                data,
            );
            if completed {
                let mut result = Map::new();
                if let Some(output) = item.get("result").filter(|v| v.is_object()) {
                    result.insert("content".into(), bounded_value(output));
                }
                if let Some(error) = item.get("error").and_then(|v| text(v, "message")) {
                    result.insert("is_error".into(), json!(true));
                    result.insert("error".into(), json!(error));
                }
                normalized["result"] = Value::Object(result);
            }
            Mapping::Events(vec![normalized])
        }
        "web_search" => {
            let Some(query) = text(item, "query") else {
                return Mapping::Unrecognized;
            };
            if !started && !completed {
                return Mapping::Ignored;
            }
            Mapping::Events(vec![emit(
                if completed {
                    "tool.result"
                } else {
                    "tool.requested"
                },
                json!({"provider_item_id":id,"tool_name":"web_search","query":query}),
            )])
        }
        "todo_list" => {
            let Some(items) = item.get("items").and_then(Value::as_array) else {
                return Mapping::Unrecognized;
            };
            let items: Vec<_> = items
                .iter()
                .filter_map(|entry| {
                    let step = text(entry, "text")?;
                    let done = entry.get("completed")?.as_bool()?;
                    Some(json!({"text":step,"completed":done}))
                })
                .take(MAX_COLLECTION_ITEMS)
                .collect();
            Mapping::Events(vec![emit(
                "plan.declared",
                json!({"provider_item_id":id,"items":items,"reported_phase":phase}),
            )])
        }
        "collab_tool_call" => collab(item, &id, status, completed, context),
        "error" => match text(item, "message") {
            Some(message) => {
                let mut data = error_data(&message);
                data["provider_item_id"] = json!(id);
                Mapping::Events(vec![emit("session.declared", data)])
            }
            None => Mapping::Unrecognized,
        },
        _ => Mapping::Unrecognized,
    }
}

fn collab(
    item: &Value,
    id: &str,
    status: Option<&str>,
    completed: bool,
    context: &AdapterContext,
) -> Mapping {
    let emit = |kind: &str, data: Value| native_event(PROVIDER, kind, data, context);
    let (Some(tool), Some(sender)) = (text(item, "tool"), text(item, "sender_thread_id")) else {
        return Mapping::Unrecognized;
    };
    let Some(receivers) = item.get("receiver_thread_ids").and_then(Value::as_array) else {
        return Mapping::Unrecognized;
    };
    let receivers: Vec<String> = receivers
        .iter()
        .filter_map(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 256)
        .map(str::to_owned)
        .take(MAX_COLLECTION_ITEMS)
        .collect();
    let mut data = json!({
        "provider_item_id":id,
        "delegation_tool":tool,
        "provider_sender_agent_id":sender,
        "provider_receiver_agent_ids":receivers,
        "reported_status":status,
    });
    if let Some(prompt) = item.get("prompt").and_then(Value::as_str) {
        let (prompt, truncated) = bounded_message(prompt);
        data["prompt"] = json!(prompt);
        data["prompt_truncated"] = json!(truncated);
    }
    let mut events = vec![emit("delegation.declared", data)];
    if completed {
        let states = item.get("agents_states").and_then(Value::as_object);
        for receiver in &receivers {
            let reported = states
                .and_then(|states| states.get(receiver))
                .and_then(|state| state.get("status"))
                .and_then(Value::as_str);
            let mut agent = json!({
                "provider_agent_id":receiver,
                "agent_role":"subagent",
                "provider":"codex",
            });
            // Spawning names the parent; other collab tools only report state.
            if tool == "spawn_agent" {
                agent["provider_parent_agent_id"] = json!(sender);
            }
            if let Some(reported) = reported {
                agent["reported_agent_status"] = json!(reported);
                if let Some(state) = agent_state(reported) {
                    agent["state"] = json!(state);
                }
            }
            events.push(emit("agent.declared", agent));
        }
    }
    Mapping::Events(events)
}

fn agent_state(reported: &str) -> Option<&'static str> {
    Some(match reported {
        "pending_init" | "running" => "started",
        "completed" => "completed",
        "errored" => "failed",
        "interrupted" | "shutdown" => "stopped",
        _ => return None,
    })
}
