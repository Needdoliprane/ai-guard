//! Delegated resource sharing of the administration views (`grants_manage`):
//! the read of one resource's grant and the change of its shares.
//!
//! The owner, tenant and organization of a grant never come from a request.
//! Only an operator establishes a resource's first binding; the delegated
//! change replaces the share list of an existing grant.
use super::*;
use serde::{Deserialize, Serialize};

/// One principal's share of a resource, with the account's display name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanGrantShareV1 {
    pub principal: PrincipalRefV1,
    pub display_name: String,
    pub permissions: Vec<ResourcePermissionV1>,
}

/// The grant of one resource in the context's organization.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanGrantV1 {
    pub version: u32,
    pub context_id: String,
    pub authorization_version: String,
    pub source: String,
    pub evaluated_at: String,
    pub resource: ResourceRefV1,
    pub owner: PrincipalRefV1,
    pub owner_display_name: String,
    pub revision: String,
    pub active: bool,
    pub shares: Vec<HumanGrantShareV1>,
    pub read_receipt: HumanReadReceiptV1,
}

/// Replacement of the share list of one existing grant. An empty permission
/// set is not a share: remove the principal from the list instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanGrantChangeV1 {
    pub operation_id: String,
    pub resource: ResourceRefV1,
    pub expected_revision: String,
    pub shares: Vec<ResourceShareV1>,
}

/// Durable outcome of a delegated share change, decimal strings throughout.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanGrantChangeReceiptV1 {
    pub version: u32,
    pub operation_id: String,
    pub resource: ResourceRefV1,
    pub grant_revision: String,
    pub authorization_revision: String,
    pub audit_seq: String,
}

/// At most this many shares in one grant change.
pub const GRANT_SHARES_MAX: usize = 64;
/// Bound of a delegated share change body.
pub const GRANT_CHANGE_BODY_MAX_BYTES: usize = 32_768;
