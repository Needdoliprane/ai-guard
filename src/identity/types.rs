use crate::{control::content_hash, tenancy::validate_tenant_id};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyControlRoleV1 {
    Runner,
    Viewer,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PrincipalRefV1 {
    HumanLocal {
        installation_id: String,
        account_id: String,
    },
    HumanOidc {
        issuer: String,
        subject: String,
    },
    LegacyControl {
        credential_id: String,
        role: LegacyControlRoleV1,
    },
}

impl PrincipalRefV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::HumanLocal {
                installation_id,
                account_id,
            } => {
                uuid(installation_id)?;
                uuid(account_id)?;
            }
            Self::HumanOidc { issuer, subject } => {
                text(issuer, 2048)?;
                text(subject, 512)?;
                let url = reqwest::Url::parse(issuer)?;
                anyhow::ensure!(
                    matches!(url.scheme(), "https" | "http")
                        && url.host_str().is_some()
                        && url.username().is_empty()
                        && url.password().is_none()
                        && url.fragment().is_none()
                        && url.query().is_none(),
                    "invalid OIDC principal issuer"
                );
                // Exact issuer bytes remain the identity; HTTP acceptance here
                // is not approval of a provider or of the demo transport.
            }
            Self::LegacyControl { credential_id, .. } => text(credential_id, 128)?,
        }
        Ok(())
    }
    pub fn is_human(&self) -> bool {
        matches!(self, Self::HumanLocal { .. } | Self::HumanOidc { .. })
    }
    pub fn key(&self) -> anyhow::Result<String> {
        self.validate()?;
        content_hash("ai-guard:principal:v1", self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanCapabilityV1 {
    ReadMetadata,
    ReadContent,
    Export,
    ReadTenantMetadata,
    ManageMemberships,
    ManageGrants,
    ReadBrokerRegistry,
    ReadWorkerRegistry,
    /// Explicit personal capability. A registered session's separate approval
    /// ACL and current Content access are also required; ownership/admin alone
    /// never confers approval. Persistence requires the modern source format.
    ApproveLaunch,
}

impl HumanCapabilityV1 {
    pub(crate) fn is_administration(self) -> bool {
        matches!(
            self,
            Self::ReadTenantMetadata
                | Self::ManageMemberships
                | Self::ManageGrants
                | Self::ReadBrokerRegistry
                | Self::ReadWorkerRegistry
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextModeV1 {
    Personal,
    Administration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanAccountV1 {
    pub version: u32,
    pub principal: PrincipalRefV1,
    pub display_name: String,
    pub revision: u64,
    pub active: bool,
}
impl HumanAccountV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        version_revision(self.version, self.revision)?;
        self.principal.validate()?;
        anyhow::ensure!(
            self.principal.is_human(),
            "human account requires a human principal"
        );
        text(&self.display_name, 160)
    }
}

/// A digest of a separately generated high-entropy credential; never a password hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalCredentialV1 {
    pub version: u32,
    pub credential_id: String,
    pub principal: PrincipalRefV1,
    pub sha256_digest: String,
    pub revision: u64,
    pub active: bool,
    pub expires_at: Option<String>,
}
impl LocalCredentialV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        version_revision(self.version, self.revision)?;
        uuid(&self.credential_id)?;
        self.principal.validate()?;
        anyhow::ensure!(
            matches!(self.principal, PrincipalRefV1::HumanLocal { .. }),
            "local credential requires a local account"
        );
        anyhow::ensure!(
            self.sha256_digest.len() == 64
                && self
                    .sha256_digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid credential digest"
        );
        if let Some(expiry) = &self.expires_at {
            text(expiry, 64)?;
            chrono::DateTime::parse_from_rfc3339(expiry)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MembershipV1 {
    pub version: u32,
    pub membership_id: String,
    pub principal: PrincipalRefV1,
    pub tenant_id: String,
    pub organization_scope: String,
    pub revision: u64,
    pub active: bool,
    pub capabilities: BTreeSet<HumanCapabilityV1>,
}
impl MembershipV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        version_revision(self.version, self.revision)?;
        uuid(&self.membership_id)?;
        self.principal.validate()?;
        anyhow::ensure!(
            self.principal.is_human(),
            "human membership requires a human principal"
        );
        validate_tenant_id(&self.tenant_id)?;
        text(&self.organization_scope, 128)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKindV1 {
    Session,
    Agent,
    Sandbox,
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceRefV1 {
    pub kind: ResourceKindV1,
    pub resource_id: String,
}
impl ResourceRefV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        text(&self.resource_id, 256)
    }
    pub fn key(&self) -> anyhow::Result<String> {
        self.validate()?;
        content_hash("ai-guard:resource:v1", self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourcePermissionV1 {
    Metadata,
    Content,
    Export,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceShareV1 {
    pub principal: PrincipalRefV1,
    pub permissions: BTreeSet<ResourcePermissionV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceGrantV1 {
    pub version: u32,
    pub tenant_id: String,
    pub organization_scope: String,
    pub resource: ResourceRefV1,
    pub owner: PrincipalRefV1,
    pub revision: u64,
    pub active: bool,
    pub shares: Vec<ResourceShareV1>,
}
impl ResourceGrantV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        version_revision(self.version, self.revision)?;
        validate_tenant_id(&self.tenant_id)?;
        text(&self.organization_scope, 128)?;
        self.resource.validate()?;
        self.owner.validate()?;
        anyhow::ensure!(
            self.owner.is_human() && self.shares.len() <= 256,
            "invalid grant owner or share count"
        );
        let mut seen = BTreeSet::new();
        for share in &self.shares {
            share.principal.validate()?;
            anyhow::ensure!(
                share.principal.is_human()
                    && share.principal != self.owner
                    && !share.permissions.is_empty()
                    && seen.insert(&share.principal),
                "invalid or ambiguous resource share"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityChangeV1 {
    Account {
        expected_revision: u64,
        account: HumanAccountV1,
    },
    Credential {
        expected_revision: u64,
        binding: LocalCredentialV1,
    },
    Membership {
        expected_revision: u64,
        membership: MembershipV1,
    },
    ResourceGrant {
        expected_revision: u64,
        grant: ResourceGrantV1,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityMutationV1 {
    pub version: u32,
    pub operation_id: String,
    /// Effective authenticated control actor; never inferred from a display name.
    pub actor: PrincipalRefV1,
    pub change: IdentityChangeV1,
}
impl IdentityMutationV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.version == 1, "unsupported identity mutation version");
        uuid(&self.operation_id)?;
        self.actor.validate()?;
        match &self.change {
            IdentityChangeV1::Account { account, .. } => account.validate()?,
            IdentityChangeV1::Credential { binding, .. } => binding.validate()?,
            IdentityChangeV1::Membership { membership, .. } => membership.validate()?,
            IdentityChangeV1::ResourceGrant { grant, .. } => grant.validate()?,
        }
        anyhow::ensure!(
            serde_json::to_vec(self)?.len() <= 65_536,
            "identity mutation exceeds 64 KiB"
        );
        Ok(())
    }
    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        content_hash("ai-guard:identity-mutation:v1", self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityReceiptV1 {
    pub version: u32,
    pub operation_id: String,
    pub input_hash: String,
    pub authorization_revision: u64,
    pub audit_seq: i64,
    pub audit_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityFailureV1 {
    AuthenticationRequired,
    CredentialInvalid,
    CredentialExpired,
    IdentityRevoked,
    ContextRevoked,
    PermissionDenied,
    ProviderUnavailable,
    IdentityUnavailable,
    IncompatibleConfiguration,
    MigrationRequired,
    AuditUnavailable,
    Conflict,
    InvalidRequest,
    NotFound,
}
impl std::fmt::Display for IdentityFailureV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "identity operation refused: {self:?}")
    }
}
impl std::error::Error for IdentityFailureV1 {}

fn version_revision(version: u32, revision: u64) -> anyhow::Result<()> {
    anyhow::ensure!(
        version == 1 && revision > 0 && revision <= i64::MAX as u64,
        "invalid identity version or revision"
    );
    Ok(())
}
fn uuid(value: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value && !id.is_nil()),
        "invalid canonical identity UUID"
    );
    Ok(())
}
fn text(value: &str, max: usize) -> anyhow::Result<()> {
    anyhow::ensure!(
        !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control),
        "invalid identity text"
    );
    Ok(())
}
