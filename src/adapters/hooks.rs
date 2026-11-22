//! Official hook payloads of Claude Code and Codex CLI.
//!
//! A hook payload is built by the client and delivered inside the worker; it is
//! a provider declaration at a client hook point, never an independent
//! observation. Worker paths (transcripts) are deliberately not retained.
use super::{
    AdapterContext, Mapping, bounded_message, bounded_value, claude::DELEGATION_TOOLS,
    claude::delegation_fields, native_event, text,
};
use serde_json::{Map, Value, json};

/// Codex collab tools that delegate work to another thread.
const CODEX_DELEGATION_TOOLS: &[&str] = &[
    "spawn_agent",
    "send_input",
    "resume_agent",
    "wait_agent",
    "close_agent",
    "send_message",
    "followup_task",
    "interrupt_agent",
];

/// Events defined by either client that ai-guard deliberately does not project.
const KNOWN_UNPROJECTED: &[&str] = &[
    "PermissionRequest",
    "PreModelRequest",
    "PostModelRequest",
    "PostCompact",
    "Interrupt",
    "Setup",
    "UserPromptExpansion",
    "PostToolBatch",
    "TaskCreated",
    "TaskCompleted",
    "TeammateIdle",
    "PreModelSwitch",
    "PostModelSwitch",
    "Elicitation",
    "ElicitationResult",
    "InstructionsLoaded",
    "ConfigChange",
    "CwdChanged",
    "DirectoryAdded",
    "FileChanged",
    "WorktreeCreate",
    "WorktreeRemove",
    "MessageDisplay",
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Client {
    ClaudeCode,
    Codex,
}

impl Client {
    pub(super) fn parse(value: &str) -> Option<Self> {
        match value {
            "claude-code" | "claude" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }
    fn source(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
        }
    }
}

fn metadata(payload: &Value, event: &str) -> Value {
    let mut data = Map::new();
    data.insert("provider_hook_event".into(), json!(event));
    for (input, output) in [
        ("session_id", "provider_session_id"),
        ("agent_id", "provider_agent_id"),
        ("agent_type", "agent_type"),
        ("turn_id", "provider_turn_id"),
        ("prompt_id", "provider_prompt_id"),
        ("cwd", "declared_cwd"),
        ("permission_mode", "permission_mode"),
        ("model", "model"),
    ] {
        if let Some(value) = text(payload, input) {
            data.insert(output.into(), json!(value));
        }
    }
    Value::Object(data)
}

pub(super) fn record(client: Client, payload: &Value, context: &AdapterContext) -> Mapping {
    let emit = |kind: &str, data: Value| native_event(client.source(), kind, data, context);
    let Some(event) = text(payload, "hook_event_name") else {
        return Mapping::Unrecognized;
    };
    let base = metadata(payload, &event);
    let with = |extra: Value| {
        let mut data = base.clone();
        if let (Some(target), Some(extra)) = (data.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        data
    };
    let message = |role: &str, body: &str, extra: Value| {
        let (body, truncated) = bounded_message(body);
        let mut data = with(extra);
        data["role"] = json!(role);
        data["text"] = json!(body);
        data["text_truncated"] = json!(truncated);
        emit("message.declared", data)
    };
    match event.as_str() {
        "SessionStart" => {
            let mut events = vec![emit(
                "session.declared",
                with(json!({"state":"started","session_source":text(payload,"source")})),
            )];
            // The hook session id names the root conversation: for Claude it is
            // the stream-json init session id; for Codex, an offline probe of
            // 0.170.0 showed it equal to exec --json `thread.started.thread_id`.
            if payload.get("agent_id").is_none()
                && let Some(session) = text(payload, "session_id")
            {
                let provider = match client {
                    Client::ClaudeCode => "claude-code",
                    Client::Codex => "codex",
                };
                events.push(emit(
                    "agent.declared",
                    json!({"provider_agent_id":session,"provider_parent_agent_id":null,"agent_role":"root","provider":provider,"model":text(payload,"model"),"state":"started"}),
                ));
            }
            Mapping::Events(events)
        }
        "SessionEnd" => Mapping::Events(vec![emit(
            "session.declared",
            with(json!({"state":"ended","reason":text(payload,"reason")})),
        )]),
        "UserPromptSubmit" => match payload.get("prompt").and_then(Value::as_str) {
            Some(prompt) => Mapping::Events(vec![message("user", prompt, json!({}))]),
            None => Mapping::Unrecognized,
        },
        "PreToolUse" => {
            let (Some(name), Some(id)) = (text(payload, "tool_name"), text(payload, "tool_use_id"))
            else {
                return Mapping::Unrecognized;
            };
            let input = payload.get("tool_input").cloned().unwrap_or(Value::Null);
            let mut data = with(json!({"provider_tool_use_id":id,"tool_name":name}));
            if !input.is_null() {
                data["input"] = bounded_value(&input);
            }
            if let Some(command) = text(&input, "command") {
                data["command_text"] = json!(command);
            }
            if let Some(server) = text(payload, "mcp_server") {
                data["mcp_server"] = json!(server);
            }
            let mut events = vec![emit("tool.requested", data)];
            let delegates = match client {
                Client::ClaudeCode => DELEGATION_TOOLS.contains(&name.as_str()),
                Client::Codex => CODEX_DELEGATION_TOOLS.contains(&name.as_str()),
            };
            if delegates {
                let mut data = with(json!({"provider_tool_use_id":id}));
                delegation_fields(&mut data, &name, &input);
                events.push(emit("delegation.declared", data));
            }
            Mapping::Events(events)
        }
        "PostToolUse" | "PostToolUseFailure" | "PermissionDenied" => {
            let Some(id) = text(payload, "tool_use_id") else {
                return Mapping::Unrecognized;
            };
            let mut data =
                with(json!({"provider_tool_use_id":id,"tool_name":text(payload,"tool_name")}));
            if let Some(duration) = payload.get("duration_ms").and_then(Value::as_u64) {
                data["duration_ms"] = json!(duration);
            }
            let mut result = Map::new();
            match event.as_str() {
                "PostToolUse" => {
                    if let Some(response) = payload.get("tool_response") {
                        result.insert("content".into(), bounded_value(response));
                    }
                }
                "PostToolUseFailure" => {
                    result.insert("is_error".into(), json!(true));
                    if let Some(error) = text(payload, "error") {
                        result.insert("error".into(), json!(error));
                    }
                    if let Some(interrupt) = payload.get("is_interrupt").and_then(Value::as_bool) {
                        result.insert("is_interrupt".into(), json!(interrupt));
                    }
                }
                _ => {
                    result.insert("is_error".into(), json!(true));
                    result.insert("reported_status".into(), json!("permission_denied"));
                    if let Some(reason) = text(payload, "reason") {
                        result.insert("error".into(), json!(reason));
                    }
                }
            }
            let mut normalized = emit("tool.result", data);
            normalized["result"] = Value::Object(result);
            Mapping::Events(vec![normalized])
        }
        "SubagentStart" | "SubagentStop" => {
            let Some(agent) = text(payload, "agent_id") else {
                return Mapping::Unrecognized;
            };
            let started = event == "SubagentStart";
            // No parent field: these hooks name the child, not its parent.
            let declared = json!({
                "provider_agent_id":agent,
                "agent_type":text(payload,"agent_type"),
                "agent_role":"subagent",
                "provider":match client { Client::ClaudeCode => "claude-code", Client::Codex => "codex" },
                "state":if started {"started"} else {"completed"},
                "provider_hook_event":event,
                "provider_session_id":text(payload,"session_id"),
            });
            let mut events = vec![emit("agent.declared", declared)];
            if !started
                && let Some(last) = payload
                    .get("last_assistant_message")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
            {
                events.push(message("assistant", last, json!({"turn_final":true})));
            }
            Mapping::Events(events)
        }
        "Stop" => match payload
            .get("last_assistant_message")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
        {
            Some(last) => {
                Mapping::Events(vec![message("assistant", last, json!({"turn_final":true}))])
            }
            None => Mapping::Events(vec![emit(
                "session.declared",
                with(json!({"state":"turn.stopped"})),
            )]),
        },
        "StopFailure" => Mapping::Events(vec![emit(
            "session.declared",
            with(json!({"state":"turn.failed","error":text(payload,"error")})),
        )]),
        "Notification" => Mapping::Events(vec![emit(
            "session.declared",
            with(
                json!({"state":"notification","notification_type":text(payload,"notification_type"),"message":payload.get("message").and_then(Value::as_str).map(|m| bounded_message(m).0)}),
            ),
        )]),
        "PreCompact" => Mapping::Events(vec![emit(
            "session.declared",
            with(json!({"state":"compacting","trigger":text(payload,"trigger")})),
        )]),
        name if KNOWN_UNPROJECTED.contains(&name) => Mapping::Ignored,
        _ => Mapping::Unrecognized,
    }
}
