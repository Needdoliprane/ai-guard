//! Delegated resource sharing (`grants_manage`): the read of one resource's
//! grant and the replacement of its shares, each in one checked transaction.
//!
//! Both need `manage_grants` and `read_memberships` in an administration
//! context: the share list names members of the organization, which only a
//! membership reader may see. The change is an ordinary identity mutation
//! with the authenticated administrator as actor, so the projection's rules
//! apply: no owner change, no share to oneself, no permission the actor does
//! not hold, no first binding of a resource.
use super::*;
use crate::identity::*;

/// Immutable response anchors supplied only by the selected server context.
pub(crate) struct HumanGrantFrameV1 {
    context_id: String,
    authorization_version: String,
}
impl HumanGrantFrameV1 {
    pub(crate) fn new(context_id: &str, authorization_version: &str) -> anyhow::Result<Self> {
        for value in [context_id, authorization_version] {
            anyhow::ensure!(
                value.len() == 64
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "invalid human grant frame anchor"
            );
        }
        Ok(Self {
            context_id: context_id.into(),
            authorization_version: authorization_version.into(),
        })
    }
}

/// The administration context may manage shares of this organization.
fn require_manager(
    tx: &CheckedTransaction<'_>,
    scope: &AuthorizedScopeV1,
    tenant: &str,
) -> anyhow::Result<AuthorizedScopeV1> {
    if !super::human_rights::installed(tx)? {
        return Err(IdentityFailureV1::MigrationRequired.into());
    }
    let current = identity::revalidate_scope_checked(tx, scope, tenant)?;
    if current.mode() != ContextModeV1::Administration
        || !current.has_capability(HumanCapabilityV1::ManageGrants)
        || !current.has_capability(HumanCapabilityV1::ReadMemberships)
    {
        return Err(IdentityFailureV1::PermissionDenied.into());
    }
    Ok(current)
}

/// The stored grant of the resource, if it belongs to this organization.
/// Another organization's grant is not disclosed.
fn stored_grant(
    tx: &Connection,
    resource: &ResourceRefV1,
    tenant: &str,
    organization: &str,
) -> anyhow::Result<ResourceGrantV1> {
    resource
        .validate()
        .map_err(|_| IdentityFailureV1::InvalidRequest)?;
    let key = resource
        .key()
        .map_err(|_| IdentityFailureV1::InvalidRequest)?;
    let stored: Option<String> = tx
        .query_row(
            "SELECT payload_json FROM identity_resources WHERE resource_key=?1",
            [key],
            |r| r.get(0),
        )
        .optional()?;
    let grant: ResourceGrantV1 = serde_json::from_str(&stored.ok_or(IdentityFailureV1::NotFound)?)?;
    if grant.organization_scope != organization || grant.tenant_id != tenant {
        return Err(IdentityFailureV1::NotFound.into());
    }
    Ok(grant)
}

fn display_name(tx: &Connection, principal: &PrincipalRefV1) -> anyhow::Result<String> {
    let account: String = tx.query_row(
        "SELECT payload_json FROM identity_accounts WHERE principal_key=?1",
        [principal.key()?],
        |r| r.get(0),
    )?;
    let account: HumanAccountV1 = serde_json::from_str(&account)?;
    anyhow::ensure!(
        &account.principal == principal,
        "grant principal differs from its account"
    );
    Ok(account.display_name)
}

/// The capability a share permission requires of the delegating context.
fn permission_held(scope: &AuthorizedScopeV1, permission: ResourcePermissionV1) -> bool {
    use HumanCapabilityV1 as C;
    match permission {
        ResourcePermissionV1::Metadata => {
            scope.has_capability(C::ReadMetadata) || scope.has_capability(C::ReadTenantMetadata)
        }
        ResourcePermissionV1::Content => scope.has_capability(C::ReadContent),
        ResourcePermissionV1::Export => scope.has_capability(C::Export),
    }
}

impl Store {
    pub(crate) fn human_grant(
        &self,
        scope: &AuthorizedScopeV1,
        frame: &HumanGrantFrameV1,
        resource: &ResourceRefV1,
        transient: &Redactor,
    ) -> anyhow::Result<HumanGrantV1> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let grant = self.human_grant_checked(&tx, scope, frame, resource, transient)?;
        self.commit_checked(tx)?;
        Ok(grant)
    }
    /// The same read on an owned 14/15 source, inside its scoped transaction.
    pub(crate) fn modern_human_grant(
        &self,
        access: &HumanSourceAccess<'_>,
        scope: &AuthorizedScopeV1,
        frame: &HumanGrantFrameV1,
        resource: &ResourceRefV1,
        transient: &Redactor,
    ) -> anyhow::Result<HumanGrantV1> {
        self.with_modern_read(access, |tx| {
            self.human_grant_checked(tx, scope, frame, resource, transient)
        })
    }
    fn human_grant_checked(
        &self,
        tx: &CheckedTransaction<'_>,
        scope: &AuthorizedScopeV1,
        frame: &HumanGrantFrameV1,
        resource: &ResourceRefV1,
        transient: &Redactor,
    ) -> anyhow::Result<HumanGrantV1> {
        identity::require_human_read(tx)?;
        let current = require_manager(tx, scope, &self.tenant_id)?;
        let grant = stored_grant(tx, resource, &self.tenant_id, current.organization_scope())?;
        let redactor = self.redactor.union(transient);
        let evaluated_at = crate::broker_registry::format_time(chrono::Utc::now());
        let mut shares = Vec::with_capacity(grant.shares.len());
        for share in &grant.shares {
            shares.push(HumanGrantShareV1 {
                principal: share.principal.clone(),
                display_name: display_name(tx, &share.principal)?,
                permissions: share.permissions.iter().copied().collect(),
            });
        }
        let owner_display_name = display_name(tx, &grant.owner)?;
        let read_receipt = self.record_human_read_checked(
            tx,
            current.principal(),
            Some(&current),
            "grants",
            shares.len(),
            transient,
        )?;
        let body = HumanGrantV1 {
            version: 1,
            context_id: frame.context_id.clone(),
            authorization_version: frame.authorization_version.clone(),
            source: "tenant_identity_resources".into(),
            evaluated_at,
            resource: grant.resource,
            owner: grant.owner,
            owner_display_name,
            revision: grant.revision.to_string(),
            active: grant.active,
            shares,
            read_receipt,
        };
        let value = serde_json::to_value(&body)?;
        anyhow::ensure!(
            redactor.value(&value) == value,
            "grant metadata requires redaction"
        );
        anyhow::ensure!(
            serde_json::to_vec(&body)?.len() <= HUMAN_MAX_PAGE_BYTES,
            "grant exceeds its byte bound"
        );
        Ok(body)
    }

    /// Delegated replacement of one existing grant's shares, legacy store.
    pub(crate) fn human_change_grant(
        &self,
        scope: &AuthorizedScopeV1,
        change: &HumanGrantChangeV1,
    ) -> anyhow::Result<IdentityReceiptV1> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let mutation = self.prepare_human_grant_change(&tx, scope, change)?;
        self.mutate_identity_checked(tx, &mutation)
    }
    /// The same change on an owned 14/15 source, under its write epoch.
    pub(crate) fn modern_human_change_grant(
        &self,
        epoch: &crate::human_authority::HumanSourcesWriteEpoch,
        source: &crate::human_authority::OwnedSourceToken,
        scope: &AuthorizedScopeV1,
        change: &HumanGrantChangeV1,
        expected: HumanDiscoveryGeneration,
    ) -> anyhow::Result<(IdentityReceiptV1, Option<HumanDiscoveryGeneration>)> {
        let access = HumanSourceAccess::write(epoch, source)?;
        let mut db = self
            .db
            .try_lock()
            .map_err(|_| crate::human_authority::HumanAuthorityFailure::StorageUnavailable)?;
        let tx =
            self.begin_checked_scoped(&mut db, &access, StoreOperationClass::IdentityAuthority)?;
        self.mutate_delegation_with_discovery(tx, expected, |tx| {
            self.prepare_human_grant_change(tx, scope, change)
        })
    }
    /// Owner, tenant, organization and state come from the stored grant.
    /// Besides the projection's delegation rules, a permission the change adds
    /// must be held by the reader's effective context (OIDC ceiling included).
    fn prepare_human_grant_change(
        &self,
        tx: &CheckedTransaction<'_>,
        scope: &AuthorizedScopeV1,
        change: &HumanGrantChangeV1,
    ) -> anyhow::Result<IdentityMutationV1> {
        let expected_revision: u64 = change
            .expected_revision
            .parse()
            .map_err(|_| IdentityFailureV1::InvalidRequest)?;
        if change.shares.len() > GRANT_SHARES_MAX
            || change
                .shares
                .iter()
                .any(|share| share.permissions.is_empty())
        {
            return Err(IdentityFailureV1::InvalidRequest.into());
        }
        let current = require_manager(tx, scope, &self.tenant_id)?;
        let stored = stored_grant(
            tx,
            &change.resource,
            &self.tenant_id,
            current.organization_scope(),
        )?;
        for share in &change.shares {
            let before = stored
                .shares
                .iter()
                .find(|previous| previous.principal == share.principal)
                .filter(|_| stored.active);
            for permission in &share.permissions {
                let retained =
                    before.is_some_and(|previous| previous.permissions.contains(permission));
                if !retained && !permission_held(&current, *permission) {
                    return Err(IdentityFailureV1::PermissionDenied.into());
                }
            }
        }
        let mutation = IdentityMutationV1 {
            version: 1,
            operation_id: change.operation_id.clone(),
            actor: current.principal().clone(),
            change: IdentityChangeV1::ResourceGrant {
                expected_revision,
                grant: ResourceGrantV1 {
                    revision: expected_revision
                        .checked_add(1)
                        .ok_or(IdentityFailureV1::InvalidRequest)?,
                    shares: change.shares.clone(),
                    ..stored
                },
            },
        };
        mutation
            .validate()
            .map_err(|_| IdentityFailureV1::InvalidRequest)?;
        Ok(mutation)
    }
}
