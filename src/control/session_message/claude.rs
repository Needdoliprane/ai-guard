//! Claude Code `stream-json` correlation on one already owned stdio channel.
//!
//! The caller owns the channel incarnation and the durable emission issue.
//! Use a client qualified for `--replay-user-messages`, `client_composed` and
//! explicit result attribution on that same channel. The archived 2.1.140
//! help establishes the CLI flag only. See [Claude CLI reference], [SDK wire
//! types] and `docs/engineering/evidence/2026-11-25/provider-conversation.fr.md`.
//!
//! [Claude CLI reference]: https://code.claude.com/docs/en/cli-reference#cli-flags
//! [SDK wire types]: https://code.claude.com/docs/en/agent-sdk/typescript#message-types

use super::{MAX_FRAME_BYTES, TurnRefusalV1};
use crate::control::{
    ControlFailure, invalid,
    session_control::{SESSION_MESSAGE_MAX_CONTENT_BYTES, canonical_uuid},
};
use serde_json::{Value, json};
use uuid::Uuid;

const MAX_JSON_DEPTH: usize = 64;

/// A fact reported by the provider on the exact channel. The replay and turn
/// result are separate facts and convey no independent observation of effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaudeObservationV1 {
    ExactReplay {
        message_uuid: Uuid,
    },
    Result {
        message_uuid: Uuid,
        result_uuid: Uuid,
        outcome: ClaudeTurnOutcomeV1,
    },
}

/// Terminal status declared by the correlated result frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaudeTurnOutcomeV1 {
    Success,
    ExecutionError,
    MaxTurns,
    MaxBudgetUsd,
    MaxStructuredOutputRetries,
}

struct PendingTurn {
    message_uuid: Uuid,
    // Retain private text only until the exact replay has been compared.
    unacknowledged_content: Option<String>,
}

enum ChannelState {
    Idle,
    InFlight(PendingTurn),
    DivergentSession,
}

/// In-memory codec for one private channel incarnation. Keep it across read
/// timeouts and discard it when retiring that channel.
pub struct ClaudeStreamJsonAdapterV1 {
    session_id: String,
    state: ChannelState,
}

impl ClaudeStreamJsonAdapterV1 {
    /// Use the session UUID supplied to this exact child's `--session-id`.
    pub fn new(session_id: Uuid) -> anyhow::Result<Self> {
        invalid(!session_id.is_nil())?;
        Ok(Self {
            session_id: session_id.to_string(),
            state: ChannelState::Idle,
        })
    }

    /// Frame one newly issued durable operation, using its UUID as the user
    /// message UUID. The private frame excludes the delimiter that the owned
    /// stdio writer adds. A possible emission occupies the turn until an exact
    /// replay and explicitly attributed result arrive on this same channel.
    pub fn issue_turn(&mut self, message_uuid: Uuid, text: &str) -> anyhow::Result<String> {
        match &self.state {
            ChannelState::Idle => {}
            ChannelState::InFlight(_) => return Err(TurnRefusalV1::TurnInFlight.into()),
            ChannelState::DivergentSession => return Err(ControlFailure::InvalidRequest.into()),
        }
        invalid(
            !message_uuid.is_nil()
                && !text.is_empty()
                && text.len() <= SESSION_MESSAGE_MAX_CONTENT_BYTES
                && text
                    .chars()
                    .all(|c| !c.is_control() || c == '\n' || c == '\t'),
        )?;
        let frame = json!({
            "type": "user",
            "message": {"role": "user", "content": text},
            "parent_tool_use_id": null,
            "session_id": self.session_id,
            "uuid": message_uuid,
            "client_composed": true,
            "priority": "later",
        })
        .to_string();
        invalid(frame.len() < MAX_FRAME_BYTES)?;
        self.state = ChannelState::InFlight(PendingTurn {
            message_uuid,
            unacknowledged_content: Some(text.to_owned()),
        });
        Ok(frame)
    }

    /// Reconcile one complete JSONL line read from the same owned channel.
    /// Unrelated, duplicate or unattributed frames emit no fact. A result
    /// before the exact replay cannot close the turn. The result's own UUID
    /// identifies that frame, and only `user_message_uuid` links it to input.
    pub fn reconcile_line(&mut self, line: &str) -> anyhow::Result<Option<ClaudeObservationV1>> {
        invalid(!matches!(&self.state, ChannelState::DivergentSession))?;
        let frame = decode_line(line)?;
        let same_session = frame["session_id"].as_str() == Some(self.session_id.as_str());
        if frame["type"] == "system" && frame["subtype"] == "init" && !same_session {
            self.state = ChannelState::DivergentSession;
            return Err(ControlFailure::InvalidRequest.into());
        }
        if !same_session || !regular_origin(&frame) {
            return Ok(None);
        }
        let ChannelState::InFlight(pending) = &mut self.state else {
            return Ok(None);
        };
        let message_uuid = pending.message_uuid;
        if let Some(text) = pending.unacknowledged_content.as_deref() {
            if exact_replay(&frame, message_uuid, text) {
                pending.unacknowledged_content = None;
                return Ok(Some(ClaudeObservationV1::ExactReplay { message_uuid }));
            }
            return Ok(None);
        }
        let Some((result_uuid, outcome)) = attributed_result(&frame, message_uuid) else {
            return Ok(None);
        };
        self.state = ChannelState::Idle;
        Ok(Some(ClaudeObservationV1::Result {
            message_uuid,
            result_uuid,
            outcome,
        }))
    }
}

fn decode_line(line: &str) -> anyhow::Result<Value> {
    invalid(line.len() <= MAX_FRAME_BYTES)?;
    let payload = line
        .strip_suffix('\n')
        .ok_or(ControlFailure::InvalidRequest)?;
    let payload = payload.strip_suffix('\r').unwrap_or(payload);
    invalid(!payload.contains(['\r', '\n']))?;
    let frame = crate::json::decode_unique(payload.as_bytes(), MAX_JSON_DEPTH)
        .map_err(|_| ControlFailure::InvalidRequest)?;
    invalid(frame.is_object())?;
    Ok(frame)
}

fn wire_uuid(value: &Value) -> Option<Uuid> {
    canonical_uuid(value.as_str()?)
        .ok()
        .filter(|id| !id.is_nil())
}

fn regular_origin(frame: &Value) -> bool {
    frame
        .get("origin")
        .is_none_or(|origin| origin["kind"] == "human")
}

fn exact_replay(frame: &Value, message_uuid: Uuid, text: &str) -> bool {
    frame["type"] == "user"
        && frame["isReplay"].as_bool() == Some(true)
        && wire_uuid(&frame["uuid"]) == Some(message_uuid)
        && frame.get("parent_tool_use_id").is_some_and(Value::is_null)
        && frame
            .get("isSynthetic")
            .is_none_or(|value| value.as_bool() == Some(false))
        && frame
            .get("client_composed")
            .is_none_or(|value| value.as_bool() == Some(true))
        && frame["message"]["role"] == "user"
        && match &frame["message"]["content"] {
            Value::String(value) => value == text,
            Value::Array(blocks) => match blocks.as_slice() {
                [block] => block["type"] == "text" && block["text"].as_str() == Some(text),
                _ => false,
            },
            _ => false,
        }
}

fn attributed_result(frame: &Value, message_uuid: Uuid) -> Option<(Uuid, ClaudeTurnOutcomeV1)> {
    if frame["type"] != "result"
        || !frame["parent_tool_use_id"].is_null()
        || frame
            .get("isSynthetic")
            .is_some_and(|value| value.as_bool() != Some(false))
        || wire_uuid(&frame["user_message_uuid"]) != Some(message_uuid)
    {
        return None;
    }
    if let Some(messages) = frame.get("user_message_uuids") {
        let [message] = messages.as_array()?.as_slice() else {
            return None;
        };
        if wire_uuid(message) != Some(message_uuid) {
            return None;
        }
    }
    let result_uuid = wire_uuid(&frame["uuid"])?;
    if result_uuid == message_uuid || frame.get("local_command").is_some() {
        return None;
    }
    let outcome = match (frame["subtype"].as_str()?, frame["is_error"].as_bool()?) {
        ("success", false) => ClaudeTurnOutcomeV1::Success,
        ("error_during_execution", true) => ClaudeTurnOutcomeV1::ExecutionError,
        ("error_max_turns", true) => ClaudeTurnOutcomeV1::MaxTurns,
        ("error_max_budget_usd", true) => ClaudeTurnOutcomeV1::MaxBudgetUsd,
        ("error_max_structured_output_retries", true) => {
            ClaudeTurnOutcomeV1::MaxStructuredOutputRetries
        }
        _ => return None,
    };
    Some((result_uuid, outcome))
}

#[cfg(test)]
#[path = "claude_tests.rs"]
mod tests;
