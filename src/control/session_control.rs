//! Session control contracts shared by the catalogue and launch.

use super::{ControlFailure, bounded, content_hash, invalid, uuid};
use crate::identity::PrincipalRefV1;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The exact opaque target selected by the caller. The server resolves the
/// source and execution settings from its current target policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionLaunchTargetSelectionV1 {
    launch_target_id: Uuid,
    expected_target_revision: u64,
}

impl SessionLaunchTargetSelectionV1 {
    pub fn new(launch_target_id: Uuid, expected_target_revision: u64) -> anyhow::Result<Self> {
        invalid(
            !launch_target_id.is_nil() && (1..=i64::MAX as u64).contains(&expected_target_revision),
        )?;
        Ok(Self {
            launch_target_id,
            expected_target_revision,
        })
    }

    /// Decode the canonical UUID and decimal revision used by the catalogue.
    pub fn parse(launch_target_id: &str, expected_target_revision: &str) -> anyhow::Result<Self> {
        let id = Uuid::parse_str(launch_target_id).map_err(|_| ControlFailure::InvalidRequest)?;
        invalid(
            id.hyphenated().to_string() == launch_target_id && expected_target_revision.len() <= 19,
        )?;
        let revision = expected_target_revision
            .parse::<u64>()
            .map_err(|_| ControlFailure::InvalidRequest)?;
        invalid(revision.to_string() == expected_target_revision)?;
        Self::new(id, revision)
    }

    pub fn launch_target_id(self) -> Uuid {
        self.launch_target_id
    }

    pub fn expected_target_revision(self) -> u64 {
        self.expected_target_revision
    }
}

/// The exact retained intent of one nominative launch preparation. It names
/// the opaque target and its expected server revision, the operation identity
/// and the preparation deadline; it carries no host path, argv, credential or
/// free provider choice. The canonical fingerprint of this record is the
/// preparation's `input_hash`: a replay must present the identical intent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLaunchPrepareRequestV1 {
    pub version: u32,
    pub operation_id: Uuid,
    pub launch_target_id: Uuid,
    pub expected_target_revision: u64,
    /// RFC3339 deadline after which the retained preparation may not launch.
    pub expires_at: String,
}

impl SessionLaunchPrepareRequestV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(
            self.version == 1
                && !self.operation_id.is_nil()
                && !self.launch_target_id.is_nil()
                && (1..=i64::MAX as u64).contains(&self.expected_target_revision),
        )?;
        bounded(&self.expires_at, 40)?;
        chrono::DateTime::parse_from_rfc3339(&self.expires_at)
            .map_err(|_| ControlFailure::InvalidRequest)?;
        Ok(())
    }

    pub fn selection(&self) -> anyhow::Result<SessionLaunchTargetSelectionV1> {
        SessionLaunchTargetSelectionV1::new(self.launch_target_id, self.expected_target_revision)
    }

    /// Canonical fingerprint of the exact intent, stable across replays.
    pub fn input_hash(&self) -> anyhow::Result<String> {
        content_hash("ai-guard:session-prepare:v1", self)
    }
}

/// The durable receipt of one retained preparation. It links the operation,
/// the authenticated initiator and the target revision to the mandatory audit
/// event. It confers no grant, reservation or start claim by itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPrepareReceiptV1 {
    pub version: u32,
    pub operation_id: Uuid,
    /// The preparation's own audit event identifier (never a session id).
    pub preparation_id: String,
    pub launch_target_id: Uuid,
    pub target_revision: u64,
    pub input_hash: String,
    pub expires_at: String,
    pub audit_seq: i64,
    pub audit_hash: String,
    /// True when this call returned an already committed preparation.
    pub replayed: bool,
}

/// The exact launch commit of one retained preparation. It names the launch
/// operation (the idempotency key), the opaque target and its expected server
/// revision, and the preparation it consumes by reference and exact fingerprint.
/// It carries no host path, argv, image, credential or free provider choice: the
/// server resolves execution from its current target policy, never from browser
/// JSON. The canonical fingerprint of this record is the launch's `input_hash`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLaunchRequestV1 {
    pub version: u32,
    pub operation_id: Uuid,
    pub launch_target_id: Uuid,
    pub expected_target_revision: u64,
    /// The retained preparation this launch consumes.
    pub preparation_operation_id: Uuid,
    /// The exact `input_hash` of the retained preparation intent. A launch must
    /// present the identical fingerprint the preparation retained.
    pub preparation_input_hash: String,
}

impl SessionLaunchRequestV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(
            self.version == 1
                && !self.operation_id.is_nil()
                && !self.launch_target_id.is_nil()
                && !self.preparation_operation_id.is_nil()
                && (1..=i64::MAX as u64).contains(&self.expected_target_revision),
        )?;
        bounded(&self.preparation_input_hash, 128)?;
        Ok(())
    }

    pub fn selection(&self) -> anyhow::Result<SessionLaunchTargetSelectionV1> {
        SessionLaunchTargetSelectionV1::new(self.launch_target_id, self.expected_target_revision)
    }

    /// Canonical fingerprint of the exact launch intent, stable across replays.
    pub fn input_hash(&self) -> anyhow::Result<String> {
        content_hash("ai-guard:session-launch:v1", self)
    }
}

/// The retained conversation binding created during launch. It anchors the
/// future SendSessionMessage channel to the exact tenant/source/session and the
/// target revision. The broker-side facts (broker/incarnation, worker/attempt,
/// runtime instance, provider/protocol) stay unknown until the supervised broker
/// admission resolves them; an unknown field is never fabricated. Restart,
/// attempt or owner changes must retarget explicitly, never silently.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConversationBindingV1 {
    pub version: u32,
    pub tenant_id: String,
    /// Durable control namespace of the owned source the session lives on.
    pub source_id: String,
    pub session_id: String,
    pub launch_target_id: Uuid,
    pub target_revision: u64,
    pub broker_id: Option<String>,
    pub broker_incarnation: Option<String>,
    pub worker_id: Option<String>,
    pub attempt_id: Option<String>,
    pub runtime_instance: Option<String>,
    /// Input epoch of the conversation; zero at launch, advanced by the broker.
    pub input_epoch: u64,
    pub provider: Option<String>,
    pub protocol: Option<String>,
    /// RFC3339 deadline inherited from the consumed preparation.
    pub expires_at: String,
    pub status: SessionConversationStatusV1,
}

/// Closed lifecycle of a retained conversation binding. Launch retains the
/// binding in `prepared`; the trusted control binds the resolved channel facts
/// (`active`) and STOP closes the right to emit (`closed`). A closed binding
/// never reopens silently: a restart binds a new epoch explicitly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionConversationStatusV1 {
    /// Binding retained at launch; no worker has been admitted yet.
    Prepared,
    /// The trusted control resolved the exact broker/worker/runtime channel.
    Active,
    /// STOP (or an explicit close) removed the right to emit. Priority over
    /// any queued message; a message never reopens it.
    Closed,
}

/// Conversation destination visible to the authorized session owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConversationViewV1 {
    pub version: u32,
    pub tenant_id: String,
    pub source_id: String,
    pub session_id: String,
    pub input_epoch: u64,
    pub provider: Option<String>,
    pub protocol: Option<String>,
    pub expires_at: String,
    pub status: SessionConversationStatusV1,
}

impl SessionConversationBindingV1 {
    /// Canonical fingerprint of the exact retained binding. A message pins the
    /// fingerprint it was enqueued against; the emission point refuses any
    /// divergence instead of silently retargeting the channel.
    pub fn fingerprint(&self) -> anyhow::Result<String> {
        content_hash("ai-guard:session-conversation:v1", self)
    }
}

/// The exact resolved channel facts the trusted control binds after the
/// supervised admission. Every field is mandatory: an unknown fact is never
/// fabricated, and a partial channel is not bindable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConversationChannelV1 {
    pub broker_id: String,
    pub broker_incarnation: String,
    pub worker_id: String,
    pub attempt_id: String,
    pub runtime_instance: String,
    pub provider: String,
    pub protocol: String,
}

impl SessionConversationChannelV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        for value in [
            &self.broker_id,
            &self.broker_incarnation,
            &self.worker_id,
            &self.attempt_id,
            &self.runtime_instance,
            &self.provider,
            &self.protocol,
        ] {
            bounded(value, 128)?;
            invalid(!value.is_empty())?;
        }
        Ok(())
    }
}

/// Trusted-control binding of the resolved conversation channel. Offline and
/// epoch-checked: the expected input epoch must equal the retained one, and
/// the binding advances the epoch explicitly. No HTTP route binds a channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindSessionConversationV1 {
    pub version: u32,
    pub operation_id: Uuid,
    pub session_id: String,
    pub expected_input_epoch: u64,
    pub channel: SessionConversationChannelV1,
}

impl BindSessionConversationV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(self.version == 1 && !self.operation_id.is_nil())?;
        canonical_uuid(&self.session_id)?;
        self.channel.validate()
    }

    pub fn input_hash(&self) -> anyhow::Result<String> {
        content_hash("ai-guard:session-conversation-bind:v1", self)
    }
}

/// Trusted-control close of the conversation (STOP priority). It closes the
/// right to emit before the worker stop follows the broker's guard; it never
/// restarts, retargets or reopens anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloseSessionConversationV1 {
    pub version: u32,
    pub operation_id: Uuid,
    pub session_id: String,
}

impl CloseSessionConversationV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(self.version == 1 && !self.operation_id.is_nil())?;
        canonical_uuid(&self.session_id)?;
        Ok(())
    }

    pub fn input_hash(&self) -> anyhow::Result<String> {
        content_hash("ai-guard:session-conversation-close:v1", self)
    }
}

/// Upper bound of one short instruction body, in bytes. The body is protected
/// tenant business data: it is persisted in the tenant store, never duplicated
/// into the audit chain, URLs, argv, logs or errors.
pub const SESSION_MESSAGE_MAX_CONTENT_BYTES: usize = 8_192;

/// The exact intent of one SendSessionMessage operation. It names the session,
/// the input epoch the caller pins its binding to, and the short instruction.
/// The canonical fingerprint covers the content; only the fingerprint and the
/// content hash reach the audit, never the body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionMessageRequestV1 {
    pub version: u32,
    pub operation_id: Uuid,
    pub session_id: Uuid,
    /// The retained binding epoch the caller read. A restart advances the
    /// epoch, so a stale pin is refused instead of silently retargeted.
    pub expected_input_epoch: u64,
    pub content: String,
}

impl SessionMessageRequestV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(self.version == 1 && !self.operation_id.is_nil() && !self.session_id.is_nil())?;
        // A short plain-text instruction: no NUL or C0 control bytes that a
        // terminal or a framed protocol could reinterpret. Newline and tab
        // are the only admitted control characters, so the generic `bounded`
        // helper (which rejects every control character) does not apply.
        invalid(
            !self.content.is_empty()
                && self.content.len() <= SESSION_MESSAGE_MAX_CONTENT_BYTES
                && self
                    .content
                    .chars()
                    .all(|c| !c.is_control() || c == '\n' || c == '\t'),
        )?;
        Ok(())
    }

    /// Canonical fingerprint of the exact intent (including the content),
    /// stable across replays. The audit retains this hash, not the body.
    pub fn input_hash(&self) -> anyhow::Result<String> {
        content_hash("ai-guard:session-message:v1", self)
    }

    /// Content-only fingerprint used to correlate the exact sent text.
    pub fn content_hash(&self) -> anyhow::Result<String> {
        content_hash("ai-guard:session-message-content:v1", &self.content)
    }
}

/// Truthful delivery status of one retained message. `queued` means durably
/// recorded; `transmitted` means a complete frame was written to the exact
/// owned channel; `acknowledged` requires an exact correlated official
/// protocol response; `unknown` means an emission was issued but no
/// reconcilable observation closed it — it is never silently resent. None of
/// these states means the instruction was executed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMessageStatusV1 {
    Queued,
    Transmitted,
    Acknowledged,
    Unknown,
}

/// The exact correlated protocol acknowledgement of one transmitted message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionMessageAckV1 {
    /// The JSON-RPC request identifier of the exact emitted frame.
    pub rpc_id: u64,
    /// The provider turn identifier returned by the typed response.
    pub turn_id: String,
}

/// Trusted-control emission issue of one queued message. It names the message
/// operation and the exact binding fingerprint the emitter read; the store
/// revalidates that fingerprint against the retained binding at the effect
/// point. Issuing is the durable latch before the first frame byte: after it,
/// delivery is unknown until a complete write is observed, and no automatic
/// resend is ever permitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueSessionMessageEmissionV1 {
    pub version: u32,
    pub operation_id: Uuid,
    pub expected_binding_fingerprint: String,
}

impl IssueSessionMessageEmissionV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(self.version == 1 && !self.operation_id.is_nil())?;
        bounded(&self.expected_binding_fingerprint, 128)?;
        Ok(())
    }
}

/// The durable receipt of one SendSessionMessage operation. It carries
/// metadata and receipts only — never the instruction body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMessageReceiptV1 {
    pub version: u32,
    pub operation_id: Uuid,
    /// The enqueue's own audit event identifier.
    pub message_id: String,
    pub session_id: Uuid,
    pub input_hash: String,
    pub content_hash: String,
    /// The binding fingerprint pinned at enqueue; the emission point
    /// revalidates it against the current retained binding.
    pub binding_fingerprint: String,
    pub input_epoch: u64,
    pub status: SessionMessageStatusV1,
    pub ack: Option<SessionMessageAckV1>,
    pub audit_seq: i64,
    pub audit_hash: String,
    /// True when this call returned an already retained result.
    pub replayed: bool,
}

/// The private handoff of one issued emission to the trusted conversation
/// adapter. It carries the exact retained body to the trusted control only —
/// never to the audit chain, the frontend, argv, URLs or logs. The adapter
/// writes it as one complete framed turn on the bound channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMessageEmissionV1 {
    pub version: u32,
    pub operation_id: Uuid,
    pub session_id: String,
    pub input_epoch: u64,
    pub binding_fingerprint: String,
    pub content_hash: String,
    pub content: String,
}

/// The durable receipt of one committed launch. It links the launch operation,
/// the consumed preparation, the nominative session grant and the mandatory
/// audit events to the retained conversation binding. Accepted launch, start
/// issued and runtime-observed running remain separate facts: this receipt
/// proves the durable launch authority and grant, not a running worker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLaunchReceiptV1 {
    pub version: u32,
    pub operation_id: Uuid,
    /// The launch's own audit event identifier (never the preparation id).
    pub launch_id: String,
    pub session_id: String,
    pub launch_target_id: Uuid,
    pub target_revision: u64,
    pub preparation_operation_id: Uuid,
    pub preparation_id: String,
    pub input_hash: String,
    pub grant_revision: u64,
    pub grant_audit_seq: i64,
    pub grant_audit_hash: String,
    pub audit_seq: i64,
    pub audit_hash: String,
    pub conversation: SessionConversationBindingV1,
    /// True when this call returned an already committed launch.
    pub replayed: bool,
}

/// One operator-provisioned opaque launch target of the owned source catalog.
/// The server resolves authorization from this record; the browser only names
/// the target and its expected revision. Launchers are sorted canonical humans.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLaunchTargetRecordV1 {
    pub version: u32,
    pub launch_target_id: Uuid,
    pub revision: u64,
    pub tenant_id: String,
    pub organization_scope: String,
    pub active: bool,
    pub launchers: Vec<PrincipalRefV1>,
}

impl SessionLaunchTargetRecordV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(
            self.version == 1
                && !self.launch_target_id.is_nil()
                && (1..=i64::MAX as u64).contains(&self.revision),
        )?;
        bounded(&self.tenant_id, 128)?;
        bounded(&self.organization_scope, 128)?;
        invalid(
            !self.launchers.is_empty()
                && self.launchers.len() <= 64
                && self.launchers.windows(2).all(|pair| pair[0] < pair[1])
                && self
                    .launchers
                    .iter()
                    .all(|principal| principal.is_human() && principal.validate().is_ok()),
        )?;
        Ok(())
    }

    pub fn input_hash(&self) -> anyhow::Result<String> {
        content_hash("ai-guard:session-launch-target:v1", self)
    }
}

/// Offline provisioning of one launch target on the owned source. The caller
/// attests a stopped control plane; no HTTP route writes the catalog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvisionSessionLaunchTargetV1 {
    pub version: u32,
    pub operation_id: Uuid,
    pub expected_revision: u64,
    pub target: SessionLaunchTargetRecordV1,
}

impl ProvisionSessionLaunchTargetV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        invalid(self.version == 1 && !self.operation_id.is_nil())?;
        self.target.validate()?;
        invalid(
            self.target.revision
                == self
                    .expected_revision
                    .checked_add(1)
                    .ok_or(ControlFailure::InvalidRequest)?,
        )?;
        Ok(())
    }

    pub fn input_hash(&self) -> anyhow::Result<String> {
        content_hash("ai-guard:session-launch-target-provision:v1", self)
    }
}

/// Validate a canonical UUID string used by the catalog and receipts.
pub(crate) fn canonical_uuid(value: &str) -> anyhow::Result<Uuid> {
    uuid(value)?;
    Uuid::parse_str(value).map_err(|_| ControlFailure::InvalidRequest.into())
}

#[cfg(test)]
#[path = "session_control_tests.rs"]
mod tests;
