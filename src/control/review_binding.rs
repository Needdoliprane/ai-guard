//! Closed descriptive runner configuration. None of these DTOs authenticates
//! a runner, proves source ownership, or authorizes a human act or launch.
use crate::identity::PrincipalRefV1;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewBindingFailureV1 {
    InvalidRequest,
    Conflict,
    PermissionDenied,
    MigrationRequired,
    NotFound,
    Unavailable,
    Integrity,
}
impl fmt::Display for ReviewBindingFailureV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidRequest => "invalid Review runner binding",
            Self::Conflict => "Review runner binding conflicts with durable content or state",
            Self::PermissionDenied => {
                "Review runner binding authority is absent or outside its scope"
            }
            Self::MigrationRequired => {
                "Review runner binding requires the guarded modern tenant format"
            }
            Self::NotFound => "Review runner binding or operation was not found",
            Self::Unavailable => "Review runner binding is unavailable",
            Self::Integrity => "Review runner binding audit or projection integrity failed",
        })
    }
}
impl std::error::Error for ReviewBindingFailureV1 {}

pub(crate) fn require(valid: bool) -> anyhow::Result<()> {
    if valid {
        Ok(())
    } else {
        Err(ReviewBindingFailureV1::InvalidRequest.into())
    }
}
pub(crate) fn conflict(valid: bool) -> anyhow::Result<()> {
    if valid {
        Ok(())
    } else {
        Err(ReviewBindingFailureV1::Conflict.into())
    }
}
pub(crate) fn permission(valid: bool) -> anyhow::Result<()> {
    if valid {
        Ok(())
    } else {
        Err(ReviewBindingFailureV1::PermissionDenied.into())
    }
}
fn uuid(value: &str) -> anyhow::Result<()> {
    require(uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value))
}
pub(crate) fn revision(value: u64) -> anyhow::Result<()> {
    require(value > 0 && value <= i64::MAX as u64)
}
fn digest(value: &str) -> anyhow::Result<()> {
    require(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
    )
}
fn symbol(value: &str, maximum: usize) -> anyhow::Result<()> {
    require(
        !value.is_empty()
            && value.len() <= maximum
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            && value != "."
            && value != "..",
    )
}
fn tenant(value: &str) -> anyhow::Result<()> {
    crate::tenancy::validate_tenant_id(value)
        .map_err(|_| ReviewBindingFailureV1::InvalidRequest.into())
}
fn public_text(value: &str, maximum: usize) -> anyhow::Result<()> {
    require(
        !value.trim().is_empty()
            && value.trim() == value
            && value.len() <= maximum
            && !value.chars().any(char::is_control),
    )
}

// Validate on every serde path, including nested event records. The explicit
// from_json path also rejects duplicate JSON keys and enforces the byte bound.
macro_rules! validated_dto {
    ($name:ident, $wire:ident, $maximum:expr, { $($(#[$meta:meta])* $field:ident : $ty:ty),+ $(,)? }) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize)]
        pub struct $name { $($(#[$meta])* pub $field:$ty),+ }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct $wire { $($(#[$meta])* $field:$ty),+ }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D:serde::Deserializer<'de>>(deserializer:D)->Result<Self,D::Error> {
                let wire=$wire::deserialize(deserializer)?;
                let value=Self { $($field:wire.$field),+ };
                value.validate().map_err(serde::de::Error::custom)?;
                Ok(value)
            }
        }
        impl $name {
            pub fn from_json(bytes:&[u8])->anyhow::Result<Self> {
                require(bytes.len() <= $maximum)?;
                let value=crate::json::decode_unique(bytes,32).map_err(|_|ReviewBindingFailureV1::InvalidRequest)?;
                serde_json::from_value(value).map_err(|_|ReviewBindingFailureV1::InvalidRequest.into())
            }
        }
    }
}

validated_dto!(RunnerReviewBindingV1, RunnerReviewBindingWire, 32*1024, {
    version:u32, binding_id:String, revision:u64, active:bool,
    credential_id:String, credential_generation:u64,
    tenant_id:String, control_namespace_uuid:String,
    broker_id:String, broker_profile_id:String,
    /// Identity of the project/workspace source. Never an authority SQLite source ID.
    source_id:String, project_id:String, source_policy_digest:String,
    routing_policy_id:String, routing_policy_revision:u64, routing_policy_digest:String,
    session_binding_policy_id:String, session_binding_policy_revision:u64,
    session_binding_policy_digest:String,
    closure_id:String, closure_revision:u64, closure_digest:String,
    control_config_public_digest:String
});
impl RunnerReviewBindingV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(self.version == 1)?;
        for value in [
            &self.binding_id,
            &self.credential_id,
            &self.control_namespace_uuid,
            &self.broker_id,
            &self.routing_policy_id,
            &self.session_binding_policy_id,
            &self.closure_id,
        ] {
            uuid(value)?;
        }
        for value in [
            self.revision,
            self.credential_generation,
            self.routing_policy_revision,
            self.session_binding_policy_revision,
            self.closure_revision,
        ] {
            revision(value)?;
        }
        tenant(&self.tenant_id)?;
        for value in [&self.broker_profile_id, &self.source_id, &self.project_id] {
            symbol(value, 256)?;
        }
        for value in [
            &self.source_policy_digest,
            &self.routing_policy_digest,
            &self.session_binding_policy_digest,
            &self.closure_digest,
            &self.control_config_public_digest,
        ] {
            digest(value)?;
        }
        require(serde_json::to_vec(self)?.len() <= 32 * 1024)
    }
    pub fn digest(&self) -> anyhow::Result<String> {
        self.validate()?;
        super::content_hash("ai-guard:runner-review-binding:v1", self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewSessionPermissionV1 {
    Content,
    ApproveLaunch,
}

validated_dto!(ReviewSessionShareV1, ReviewSessionShareWire, 4096, {
    principal:PrincipalRefV1, permissions:Vec<ReviewSessionPermissionV1>
});
impl ReviewSessionShareV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        self.principal
            .validate()
            .map_err(|_| ReviewBindingFailureV1::InvalidRequest)?;
        require(
            self.principal.is_human()
                && !self.permissions.is_empty()
                && self.permissions.len() <= 2
                && self.permissions.windows(2).all(|pair| pair[0] < pair[1]),
        )?;
        // Approval never conveys blind access without Content.
        require(
            !self
                .permissions
                .contains(&ReviewSessionPermissionV1::ApproveLaunch)
                || self
                    .permissions
                    .contains(&ReviewSessionPermissionV1::Content),
        )
    }
}

validated_dto!(ReviewSessionBindingPolicyV1, ReviewSessionBindingPolicyWire, 48*1024, {
    version:u32, policy_id:String, revision:u64, active:bool,
    tenant_id:String, control_namespace_uuid:String, organization_scope:String,
    project_id:String, owner:PrincipalRefV1,
    /// Explicit descriptive ACL; ownership alone never names an approver.
    owner_permissions:Vec<ReviewSessionPermissionV1>, shares:Vec<ReviewSessionShareV1>
});
impl ReviewSessionBindingPolicyV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(self.version == 1)?;
        uuid(&self.policy_id)?;
        uuid(&self.control_namespace_uuid)?;
        revision(self.revision)?;
        tenant(&self.tenant_id)?;
        symbol(&self.project_id, 256)?;
        public_text(&self.organization_scope, 128)?;
        self.owner
            .validate()
            .map_err(|_| ReviewBindingFailureV1::InvalidRequest)?;
        require(self.owner.is_human() && self.shares.len() <= 64)?;
        require(
            !self.owner_permissions.is_empty()
                && self.owner_permissions.len() <= 2
                && self
                    .owner_permissions
                    .windows(2)
                    .all(|pair| pair[0] < pair[1])
                && self
                    .owner_permissions
                    .contains(&ReviewSessionPermissionV1::Content),
        )?;
        let mut principals = std::collections::BTreeSet::new();
        for share in &self.shares {
            share.validate()?;
            require(share.principal != self.owner && principals.insert(&share.principal))?;
        }
        require(serde_json::to_vec(self)?.len() <= 48 * 1024)
    }
    pub fn digest(&self) -> anyhow::Result<String> {
        self.validate()?;
        super::content_hash("ai-guard:review-session-binding-policy:v1", self)
    }
    pub(crate) fn validate_binding(&self, binding: &RunnerReviewBindingV1) -> anyhow::Result<()> {
        self.validate()?;
        binding.validate()?;
        require(
            self.tenant_id == binding.tenant_id
                && self.control_namespace_uuid == binding.control_namespace_uuid
                && self.project_id == binding.project_id
                && self.policy_id == binding.session_binding_policy_id
                && self.revision == binding.session_binding_policy_revision
                && self.digest()? == binding.session_binding_policy_digest,
        )
    }
}

validated_dto!(ProvisionRunnerReviewBindingV1, ProvisionRunnerReviewBindingWire, 56*1024, {
    version:u32, operation_id:String, operator_delegation_id:String,
    expected_revision:u64, expected_policy_revision:u64,
    binding:RunnerReviewBindingV1, policy:ReviewSessionBindingPolicyV1
});
impl ProvisionRunnerReviewBindingV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(self.version == 1 && self.binding.active && self.policy.active)?;
        uuid(&self.operation_id)?;
        uuid(&self.operator_delegation_id)?;
        require(self.expected_revision.checked_add(1) == Some(self.binding.revision))?;
        require(self.expected_policy_revision.checked_add(1) == Some(self.policy.revision))?;
        self.policy.validate_binding(&self.binding)?;
        require(serde_json::to_vec(self)?.len() <= 56 * 1024)
    }
    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        super::content_hash("ai-guard:provision-runner-review-binding:v1", self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewBindingRevocationReasonV1 {
    OperatorRevoked,
    CredentialRotated,
    ConfigurationChanged,
}

validated_dto!(RevokeRunnerReviewBindingV1, RevokeRunnerReviewBindingWire, 4096, {
    version:u32, operation_id:String, operator_delegation_id:String,
    binding_id:String, expected_revision:u64, expected_binding_digest:String,
    reason:ReviewBindingRevocationReasonV1
});
impl RevokeRunnerReviewBindingV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(self.version == 1)?;
        uuid(&self.operation_id)?;
        uuid(&self.operator_delegation_id)?;
        uuid(&self.binding_id)?;
        revision(self.expected_revision)?;
        digest(&self.expected_binding_digest)?;
        require(self.expected_revision < i64::MAX as u64)
    }
    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        super::content_hash("ai-guard:revoke-runner-review-binding:v1", self)
    }
}

validated_dto!(ReviewBindingSnapshotV1, ReviewBindingSnapshotWire, 56*1024, {
    version:u32, binding:RunnerReviewBindingV1, policy:ReviewSessionBindingPolicyV1,
    binding_digest:String, policy_digest:String
});
impl ReviewBindingSnapshotV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(self.version == 1)?;
        self.policy.validate_binding(&self.binding)?;
        require(!self.binding.active || self.policy.active)?;
        require(
            self.binding_digest == self.binding.digest()?
                && self.policy_digest == self.policy.digest()?,
        )
    }
    pub fn snapshot_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        super::content_hash("ai-guard:runner-review-binding-snapshot:v1", self)
    }
}

validated_dto!(ReviewBindingReceiptV1, ReviewBindingReceiptWire, 4096, {
    version:u32,operation_id:String,input_hash:String,binding_id:String,
    binding_revision:u64,audit_seq:i64,audit_hash:String
});
impl ReviewBindingReceiptV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(self.version == 1 && self.audit_seq > 0)?;
        uuid(&self.operation_id)?;
        uuid(&self.binding_id)?;
        revision(self.binding_revision)?;
        digest(&self.input_hash)?;
        digest(&self.audit_hash)
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct ReviewBindingMutationResultV1 {
    pub version: u32,
    pub receipt: ReviewBindingReceiptV1,
    pub snapshot: ReviewBindingSnapshotV1,
    pub replayed: bool,
}

#[cfg(test)]
pub(crate) mod tests;
