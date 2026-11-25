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
/// binding in `prepared`; the supervised broker admission lot advances it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionConversationStatusV1 {
    /// Binding retained at launch; no worker has been admitted yet.
    Prepared,
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
