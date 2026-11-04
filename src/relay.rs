//! Directed worker messaging contracts. A message is tenant business data, not audit content.
use serde::{Deserialize, Serialize};

pub const MAX_MESSAGE_BYTES: usize = 65_536;
pub const MAX_RETAINED_BODY_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_RETAINED_MESSAGES: u64 = 1024;
pub const MAX_MESSAGES_PER_MINUTE: u64 = 60;
pub const MAX_GRANT_SECONDS: i64 = 86_400;
pub const MAX_MESSAGE_TTL_SECONDS: u32 = 3600;
pub const MAX_INBOX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayFailure {
    InvalidRequest,
    Forbidden,
    NotFound,
    Conflict,
    RateLimited,
    Capacity,
    Expired,
}
impl std::fmt::Display for RelayFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidRequest => "invalid relay request",
            Self::Forbidden => "worker relay exchange is not authorized",
            Self::NotFound => "relay resource not found in authorized scope",
            Self::Conflict => "relay operation identity belongs to different content",
            Self::RateLimited => "relay grant rate limit reached",
            Self::Capacity => "relay retained message capacity reached; prune expired messages",
            Self::Expired => "relay resource has expired",
        })
    }
}
impl std::error::Error for RelayFailure {}

/// No deserializer or public constructor: only verified control authentication creates this.
#[derive(Clone)]
pub struct WorkerRelayIdentity {
    pub(crate) tenant_id: String,
    pub(crate) credential_id: String,
    pub(crate) worker_id: String,
    pub(crate) attempt_id: String,
    pub(crate) generation: u64,
    pub(crate) binding_revision: u64,
}
impl WorkerRelayIdentity {
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }
    pub fn attempt_id(&self) -> &str {
        &self.attempt_id
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn credential_id(&self) -> &str {
        &self.credential_id
    }
    pub fn binding_revision(&self) -> u64 {
        self.binding_revision
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayEndpointV1 {
    pub worker_id: String,
    pub attempt_id: String,
    pub generation: u64,
}
impl RelayEndpointV1 {
    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }
    pub fn attempt_id(&self) -> &str {
        &self.attempt_id
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        crate::control::uuid(&self.worker_id)?;
        crate::control::uuid(&self.attempt_id)?;
        // No central generation transition exists in protocol v1. Credential
        // rotation changes the binding revision while this endpoint stays fixed.
        valid(self.generation == 1)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayGrantV1 {
    pub version: u32,
    pub operation_id: String,
    pub grant_id: String,
    pub source: RelayEndpointV1,
    pub destination: RelayEndpointV1,
    pub channel: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayRevokeV1 {
    pub version: u32,
    pub operation_id: String,
    pub grant_id: String,
}

/// Deliberately has no tenant/source/worker field: the authenticated capability supplies it.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelaySendV1 {
    pub version: u32,
    pub operation_id: String,
    pub grant_id: String,
    pub ttl_seconds: u32,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// An explicit recipient acknowledgement, not proof of execution or a resulting effect.
pub struct RelayAckV1 {
    pub version: u32,
    pub operation_id: String,
    pub message_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayPruneV1 {
    pub version: u32,
    pub operation_id: String,
    pub message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayReceiptV1 {
    pub version: u32,
    pub operation_id: String,
    pub resource_id: String,
    pub audit_seq: i64,
    pub audit_hash: String,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayMessageMetadataV1 {
    pub version: u32,
    pub message_id: String,
    pub grant_id: String,
    pub source: RelayEndpointV1,
    pub destination: RelayEndpointV1,
    pub channel: String,
    pub created_at: String,
    pub expires_at: String,
    pub body_bytes: u64,
    pub body_sha256: String,
    pub acknowledged: bool,
    pub body_pruned: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayMessageV1 {
    pub sequence: i64,
    pub metadata: RelayMessageMetadataV1,
    pub body: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayInboxV1 {
    pub version: u32,
    pub items: Vec<RelayMessageV1>,
    pub next_after: Option<i64>,
}

pub(crate) fn valid(condition: bool) -> anyhow::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(RelayFailure::InvalidRequest.into())
    }
}
pub(crate) fn channel_valid(channel: &str) -> bool {
    !channel.is_empty()
        && channel.len() <= 64
        && channel
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}
