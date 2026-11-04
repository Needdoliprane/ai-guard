//! Workload credentials are independent of human sessions and runner keys.
//! Only digests are persisted; a binding never changes its workload subject.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkloadFailureV1 {
    InvalidRequest,
    Conflict,
    Unauthorized,
    MigrationRequired,
    ClockRegression,
}
impl std::fmt::Display for WorkloadFailureV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidRequest => "invalid workload binding request",
            Self::Conflict => "workload binding conflicts with durable content or identity",
            Self::Unauthorized => "workload credential is unavailable, expired, revoked or outside its authenticated scope",
            Self::MigrationRequired => "explicit workload schema migration is required",
            Self::ClockRegression => "control UTC clock regressed; reconcile the durable audit time floor before retrying",
        })
    }
}
impl std::error::Error for WorkloadFailureV1 {}
pub(crate) fn workload_valid(value: bool) -> anyhow::Result<()> {
    if value {
        Ok(())
    } else {
        Err(WorkloadFailureV1::InvalidRequest.into())
    }
}
pub(crate) fn workload_conflict(value: bool) -> anyhow::Result<()> {
    if value {
        Ok(())
    } else {
        Err(WorkloadFailureV1::Conflict.into())
    }
}
pub(crate) fn workload_authorized(value: bool) -> anyhow::Result<()> {
    if value {
        Ok(())
    } else {
        Err(WorkloadFailureV1::Unauthorized.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkloadSubjectV1 {
    Broker {
        broker_id: String,
        incarnation_id: String,
    },
    Worker {
        worker_id: String,
        attempt_id: String,
        generation: u64,
    },
}
impl WorkloadSubjectV1 {
    pub(crate) fn key(&self) -> anyhow::Result<String> {
        crate::control::content_hash("ai-guard:workload-subject:v1", self)
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Broker {
                broker_id,
                incarnation_id,
            } => {
                crate::broker_registry::registry_uuid(broker_id)?;
                crate::broker_registry::registry_uuid(incarnation_id)
            }
            Self::Worker {
                worker_id,
                attempt_id,
                generation,
            } => {
                crate::broker_registry::registry_uuid(worker_id)?;
                crate::broker_registry::registry_uuid(attempt_id)?;
                // v1 has no central generation transition protocol. Credential
                // rotation cannot invent a new attempt or execution generation.
                workload_valid(*generation == 1)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadBindingIssueV1 {
    pub version: u32,
    pub operation_id: String,
    pub credential_id: String,
    pub expected_revision: u64,
    pub subject: WorkloadSubjectV1,
    pub sha256_digest: String,
    pub expires_at: String,
}
impl WorkloadBindingIssueV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        workload_valid(self.version == 1 && self.expected_revision < i64::MAX as u64)?;
        crate::broker_registry::registry_uuid(&self.operation_id)?;
        crate::broker_registry::registry_uuid(&self.credential_id)?;
        self.subject.validate()?;
        crate::control::hash(&self.sha256_digest)?;
        crate::broker_registry::parse_time(&self.expires_at)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadBindingRevocationV1 {
    pub version: u32,
    pub operation_id: String,
    pub credential_id: String,
    pub expected_revision: u64,
}
impl WorkloadBindingRevocationV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        workload_valid(
            self.version == 1
                && self.expected_revision > 0
                && self.expected_revision < i64::MAX as u64,
        )?;
        crate::broker_registry::registry_uuid(&self.operation_id)?;
        crate::broker_registry::registry_uuid(&self.credential_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadBindingReceiptV1 {
    pub version: u32,
    pub credential_id: String,
    pub revision: u64,
    pub audit_event_id: String,
    pub audit_seq: i64,
    pub audit_hash: String,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkloadBinding {
    pub version: u32,
    pub credential_id: String,
    pub revision: u64,
    pub subject: WorkloadSubjectV1,
    pub sha256_digest: String,
    pub expires_at: String,
    pub revoked: bool,
    pub updated_at: String,
}

pub(crate) enum AuthenticatedWorkload {
    Broker(crate::broker_registry::BrokerIdentityV1),
    Worker(crate::relay::WorkerRelayIdentity),
}

/// Historical ownership for collision checks, never an authentication proof.
/// No secret or digest is returned to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkloadDigestOwnerV1 {
    pub credential_id: String,
    pub subject: WorkloadSubjectV1,
}

#[cfg(test)]
#[path = "workload/tests.rs"]
mod tests;
