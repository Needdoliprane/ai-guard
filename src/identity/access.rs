//! Wire projections for authenticated human readers, separate from control keys.
use super::*;
use crate::model::{CommandSpec, Decision};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanReadViewV1 {
    Metadata,
    Content,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanContextV1 {
    pub id: String,
    pub tenant_id: String,
    pub organization_scope: String,
    pub mode: ContextModeV1,
    pub capabilities: Vec<HumanCapabilityV1>,
    pub membership_revision: String,
    pub authorization_version: String,
    pub expires_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanAuthenticationV1 {
    pub method: String,
    pub credential_id: String,
    pub expires_at: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanIdentityV1 {
    pub version: u32,
    pub principal: PrincipalRefV1,
    pub display_name: String,
    pub authentication: HumanAuthenticationV1,
    pub contexts: Vec<HumanContextV1>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanResourcePermissionsV1 {
    pub metadata: bool,
    pub content: bool,
    pub export: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanResourceItemV1 {
    pub resource: ResourceRefV1,
    pub owner: PrincipalRefV1,
    pub revision: String,
    pub permissions: HumanResourcePermissionsV1,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanEventMetadataV1 {
    pub id: String,
    pub timestamp: String,
    pub received_at: String,
    pub kind: String,
    pub source: String,
    pub provenance: String,
    pub coverage: String,
    pub resource: ResourceRefV1,
    pub decision: Option<crate::model::Outcome>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanEventContentV1 {
    pub command: Option<CommandSpec>,
    pub decision: Option<Decision>,
    pub result: Option<serde_json::Value>,
    pub data: serde_json::Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanEventItemV1 {
    pub metadata: HumanEventMetadataV1,
    pub content: Option<HumanEventContentV1>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanReadReceiptV1 {
    pub request_id: String,
    pub audit_event_id: String,
    pub received_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanPageV1<T> {
    pub version: u32,
    pub context_id: String,
    pub authorization_version: String,
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub coverage: String,
    pub read_receipt: HumanReadReceiptV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<HumanReadViewV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_complete: Option<bool>,
}

/// Typed CLI preparation; contains digests and operation identities, never a raw secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalHumanProvisionV1 {
    pub version: u32,
    pub mutations: [IdentityMutationV1; 3],
}
impl LocalHumanProvisionV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.version == 1, "unsupported local provisioning version");
        for input in &self.mutations {
            input.validate()?;
        }
        let [account_change, binding_change, membership_change] = &self.mutations;
        let (
            IdentityChangeV1::Account {
                expected_revision: 0,
                account,
            },
            IdentityChangeV1::Credential {
                expected_revision: 0,
                binding,
            },
            IdentityChangeV1::Membership {
                expected_revision: 0,
                membership,
            },
        ) = (
            &account_change.change,
            &binding_change.change,
            &membership_change.change,
        )
        else {
            anyhow::bail!("local provisioning requires three fresh typed mutations");
        };
        anyhow::ensure!(
            matches!(account.principal, PrincipalRefV1::HumanLocal { .. })
                && account.principal == binding.principal
                && account.principal == membership.principal
                && account.revision == 1
                && binding.revision == 1
                && membership.revision == 1
                && account.active
                && binding.active
                && membership.active
                && self
                    .mutations
                    .iter()
                    .all(|m| m.actor == account_change.actor)
                && matches!(
                    account_change.actor,
                    PrincipalRefV1::LegacyControl {
                        role: LegacyControlRoleV1::Runner,
                        ..
                    }
                )
                && account_change.operation_id != binding_change.operation_id
                && account_change.operation_id != membership_change.operation_id
                && binding_change.operation_id != membership_change.operation_id,
            "inconsistent local provisioning identities"
        );
        Ok(())
    }
}

pub(crate) struct VerifiedLocalCredentialV1 {
    pub principal: PrincipalRefV1,
    pub credential_id: String,
    pub revision: u64,
    pub expires_at: Option<String>,
}

pub const HUMAN_MAX_PAGE_ITEMS: usize = 100;
pub const HUMAN_MAX_PAGE_BYTES: usize = 262_144;
pub(crate) const HUMAN_PAGE_ENVELOPE_RESERVE: usize = 4096;
