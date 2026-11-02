use super::*;
use std::collections::BTreeSet;

/// A snapshot, never durable authority. Store revalidates it with the resource
/// in one transaction; callers cannot authorize reads from this value alone.
#[derive(Debug, Clone)]
pub struct AuthorizedScopeV1 {
    principal: PrincipalRefV1,
    tenant_id: String,
    organization_scope: String,
    membership_id: String,
    membership_revision: u64,
    authorization_revision: u64,
    mode: ContextModeV1,
    capabilities: BTreeSet<HumanCapabilityV1>,
}

pub(crate) fn authorize_context(
    principal: &PrincipalRefV1,
    membership: &MembershipV1,
    mode: ContextModeV1,
    expected_revision: u64,
) -> Result<AuthorizedScopeV1, IdentityFailureV1> {
    membership
        .validate()
        .map_err(|_| IdentityFailureV1::IdentityUnavailable)?;
    if !membership.active
        || &membership.principal != principal
        || membership.revision != expected_revision
    {
        return Err(IdentityFailureV1::ContextRevoked);
    }
    if mode == ContextModeV1::Administration
        && !membership
            .capabilities
            .iter()
            .any(|c| c.is_administration())
    {
        return Err(IdentityFailureV1::PermissionDenied);
    }
    let capabilities = membership
        .capabilities
        .iter()
        .copied()
        .filter(|c| mode == ContextModeV1::Administration || !c.is_administration())
        .collect();
    Ok(AuthorizedScopeV1 {
        principal: principal.clone(),
        tenant_id: membership.tenant_id.clone(),
        organization_scope: membership.organization_scope.clone(),
        membership_id: membership.membership_id.clone(),
        membership_revision: membership.revision,
        authorization_revision: 0,
        mode,
        capabilities,
    })
}

impl AuthorizedScopeV1 {
    pub fn principal(&self) -> &PrincipalRefV1 {
        &self.principal
    }
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    pub fn organization_scope(&self) -> &str {
        &self.organization_scope
    }
    pub fn membership_id(&self) -> &str {
        &self.membership_id
    }
    pub fn membership_revision(&self) -> u64 {
        self.membership_revision
    }
    pub fn authorization_revision(&self) -> u64 {
        self.authorization_revision
    }
    pub(crate) fn at_authorization_revision(mut self, revision: u64) -> Self {
        self.authorization_revision = revision;
        self
    }
    pub fn mode(&self) -> ContextModeV1 {
        self.mode
    }
    pub fn has_capability(&self, capability: HumanCapabilityV1) -> bool {
        self.capabilities.contains(&capability)
    }
    /// Restrict an authorized snapshot, retaining earlier restrictions when
    /// Store revalidates it. This never grants a membership capability.
    pub fn restrict_capabilities(
        self,
        _ceiling: BTreeSet<HumanCapabilityV1>,
    ) -> Result<Self, IdentityFailureV1> {
        // Test-first fail-closed seam; no API route uses this scaffold.
        Err(IdentityFailureV1::IdentityUnavailable)
    }
    pub(crate) fn permits_resource(
        &self,
        grant: &ResourceGrantV1,
        permission: ResourcePermissionV1,
    ) -> bool {
        if !grant.active
            || grant.tenant_id != self.tenant_id
            || grant.organization_scope != self.organization_scope
            || grant.validate().is_err()
        {
            return false;
        }
        let required = match permission {
            ResourcePermissionV1::Metadata => HumanCapabilityV1::ReadMetadata,
            ResourcePermissionV1::Content => HumanCapabilityV1::ReadContent,
            ResourcePermissionV1::Export => HumanCapabilityV1::Export,
        };
        if permission == ResourcePermissionV1::Metadata
            && self.mode == ContextModeV1::Administration
            && self.has_capability(HumanCapabilityV1::ReadTenantMetadata)
        {
            return true;
        }
        self.has_capability(required)
            && (grant.owner == self.principal
                || grant.shares.iter().any(|share| {
                    share.principal == self.principal && share.permissions.contains(&permission)
                }))
    }
}
