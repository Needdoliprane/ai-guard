//! Wire of the administration memberships read and of the delegated
//! membership and share mutations of the 360° views.
use super::*;
use serde::{Deserialize, Serialize};

/// Credential metadata of a local account; never a digest or a secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanMemberCredentialV1 {
    pub credential_id: String,
    pub active: bool,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanMembershipItemV1 {
    pub membership_id: String,
    pub principal: PrincipalRefV1,
    pub display_name: String,
    pub account_active: bool,
    pub organization_scope: String,
    pub capabilities: Vec<HumanCapabilityV1>,
    pub revision: String,
    pub active: bool,
    /// Local credentials of the account, at most 16; OIDC accounts have none.
    pub credentials: Vec<HumanMemberCredentialV1>,
    pub credentials_complete: bool,
    /// The reader's own membership: the delegated route refuses to change it.
    pub own: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanMembershipPageV1 {
    pub version: u32,
    pub context_id: String,
    pub authorization_version: String,
    pub source: String,
    pub consistency: String,
    pub evaluated_at: String,
    pub items: Vec<HumanMembershipItemV1>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub read_receipt: HumanReadReceiptV1,
}

/// Delegated change of one existing membership of the reader's organization.
/// Principal, tenant and organization are taken from the stored membership,
/// never from this body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanMembershipChangeV1 {
    pub operation_id: String,
    pub expected_revision: String,
    pub capabilities: Vec<HumanCapabilityV1>,
    pub active: bool,
}

pub const MEMBER_CREDENTIALS_MAX: usize = 16;
