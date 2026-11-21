//! Apply audited identity mutations without an SQL backend.
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::identity::{
    HumanAccountV1, HumanCapabilityV1, IdentityChangeV1, IdentityFailureV1, IdentityMutationV1,
    LegacyControlRoleV1, LocalCredentialV1, MembershipV1, PrincipalRefV1, ResourceGrantV1,
};

#[derive(Default)]
pub(crate) struct Projection {
    pub(super) accounts: BTreeMap<String, (HumanAccountV1, i64)>,
    pub(super) credentials: BTreeMap<String, (LocalCredentialV1, i64)>,
    pub(super) memberships: BTreeMap<String, (MembershipV1, i64)>,
    pub(super) resources: BTreeMap<String, (ResourceGrantV1, i64)>,
    pub(super) operations: BTreeMap<String, (String, i64)>,
    pub(super) revision: u64,
    /// After the human rights marker, a grant that binds a resource for the
    /// first time only adds rights: it keeps the tenant authorization revision,
    /// so it revokes no live context. Every other change still advances it.
    pub(super) additive_resource_grants: bool,
}

pub(super) fn invalid(ok: bool) -> anyhow::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(IdentityFailureV1::InvalidRequest.into())
    }
}
pub(super) fn conflict(ok: bool) -> anyhow::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(IdentityFailureV1::Conflict.into())
    }
}
fn revision(previous: Option<u64>, expected: u64, next: u64) -> anyhow::Result<()> {
    conflict(previous.unwrap_or(0) == expected)?;
    invalid(expected.checked_add(1) == Some(next))
}

impl Projection {
    fn account_exists(&self, principal: &PrincipalRefV1) -> anyhow::Result<()> {
        invalid(self.accounts.contains_key(&principal.key()?))
    }
    fn member_exists(&self, principal: &PrincipalRefV1, organization: &str) -> bool {
        self.memberships.values().any(|(m, _)| {
            &m.principal == principal && m.active && m.organization_scope == organization
        })
    }
    pub(crate) fn apply(
        &mut self,
        mutation: &IdentityMutationV1,
        tenant: &str,
        seq: i64,
    ) -> anyhow::Result<()> {
        mutation.validate()?;
        invalid(!self.operations.contains_key(&mutation.operation_id))?;
        match &mutation.actor {
            PrincipalRefV1::LegacyControl {
                role: LegacyControlRoleV1::Runner,
                ..
            } => {}
            principal if principal.is_human() => {
                let actor = self
                    .accounts
                    .get(&principal.key()?)
                    .ok_or(IdentityFailureV1::PermissionDenied)?;
                invalid(actor.0.active)?;
                let (capability, organization) = match &mutation.change {
                    IdentityChangeV1::ResourceGrant { grant, .. } => {
                        (HumanCapabilityV1::ManageGrants, &grant.organization_scope)
                    }
                    IdentityChangeV1::Membership { membership, .. } => (
                        HumanCapabilityV1::ManageMemberships,
                        &membership.organization_scope,
                    ),
                    // Accounts and credentials are provisioned by the trusted
                    // operator in this schema foundation, not a tenant role.
                    _ => return Err(IdentityFailureV1::PermissionDenied.into()),
                };
                if !self.memberships.values().any(|(m, _)| {
                    m.active
                        && &m.principal == principal
                        && m.organization_scope == *organization
                        && m.capabilities.contains(&capability)
                }) {
                    return Err(IdentityFailureV1::PermissionDenied.into());
                }
            }
            _ => return Err(IdentityFailureV1::PermissionDenied.into()),
        }
        let mut keeps_revision = false;
        match &mutation.change {
            IdentityChangeV1::Account {
                expected_revision,
                account,
            } => {
                let key = account.principal.key()?;
                revision(
                    self.accounts.get(&key).map(|a| a.0.revision),
                    *expected_revision,
                    account.revision,
                )?;
                self.accounts.insert(key, (account.clone(), seq));
            }
            IdentityChangeV1::Credential {
                expected_revision,
                binding: credential,
            } => {
                self.account_exists(&credential.principal)?;
                let old = self.credentials.get(&credential.credential_id);
                revision(
                    old.map(|c| c.0.revision),
                    *expected_revision,
                    credential.revision,
                )?;
                invalid(old.is_none_or(|c| c.0.principal == credential.principal))?;
                conflict(!self.credentials.values().any(|(c, _)| {
                    c.sha256_digest == credential.sha256_digest
                        && c.credential_id != credential.credential_id
                }))?;
                self.credentials
                    .insert(credential.credential_id.clone(), (credential.clone(), seq));
            }
            IdentityChangeV1::Membership {
                expected_revision,
                membership,
            } => {
                invalid(membership.tenant_id == tenant)?;
                self.account_exists(&membership.principal)?;
                let old = self.memberships.get(&membership.membership_id);
                revision(
                    old.map(|m| m.0.revision),
                    *expected_revision,
                    membership.revision,
                )?;
                invalid(old.is_none_or(|m| {
                    m.0.principal == membership.principal
                        && m.0.organization_scope == membership.organization_scope
                }))?;
                // Organization administrators may retain or remove an existing
                // control-only grant, but only trusted control may introduce or
                // reactivate it. Identity and organization are immutable above.
                if mutation.actor.is_human()
                    && membership
                        .capabilities
                        .iter()
                        .filter(|capability| capability.is_control_only())
                        .any(|capability| {
                            old.is_none_or(|(previous, _)| {
                                !previous.capabilities.contains(capability)
                                    || (!previous.active && membership.active)
                            })
                        })
                {
                    return Err(IdentityFailureV1::PermissionDenied.into());
                }
                conflict(!self.memberships.values().any(|(m, _)| {
                    m.principal == membership.principal
                        && m.organization_scope == membership.organization_scope
                        && m.membership_id != membership.membership_id
                }))?;
                self.memberships
                    .insert(membership.membership_id.clone(), (membership.clone(), seq));
            }
            IdentityChangeV1::ResourceGrant {
                expected_revision,
                grant,
            } => {
                let key = grant.resource.key()?;
                // Only the operator may establish a resource's first binding.
                // A tombstone remains an assigned resource, retaining its org.
                if mutation.actor.is_human()
                    && self.resources.get(&key).is_none_or(|previous| {
                        previous.0.organization_scope != grant.organization_scope
                    })
                {
                    return Err(IdentityFailureV1::PermissionDenied.into());
                }
                invalid(grant.tenant_id == tenant)?;
                self.account_exists(&grant.owner)?;
                invalid(self.member_exists(&grant.owner, &grant.organization_scope))?;
                for share in &grant.shares {
                    self.account_exists(&share.principal)?;
                    invalid(self.member_exists(&share.principal, &grant.organization_scope))?;
                }
                invalid(
                    self.resources
                        .get(&key)
                        .is_none_or(|g| g.0.organization_scope == grant.organization_scope),
                )?;
                revision(
                    self.resources.get(&key).map(|g| g.0.revision),
                    *expected_revision,
                    grant.revision,
                )?;
                keeps_revision =
                    self.additive_resource_grants && !self.resources.contains_key(&key);
                self.resources.insert(key, (grant.clone(), seq));
            }
        }
        if !keeps_revision {
            self.revision = self
                .revision
                .checked_add(1)
                .ok_or(IdentityFailureV1::InvalidRequest)?;
        }
        i64::try_from(self.revision)
            .context("identity authorization revision exceeds SQLite integer range")?;
        self.operations
            .insert(mutation.operation_id.clone(), (mutation.input_hash()?, seq));
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Genesis {
    pub(crate) version: u32,
    pub(crate) control_namespace_uuid: String,
    pub(crate) previous_seq: i64,
    pub(crate) previous_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub(crate) version: u32,
    pub(crate) authorization_revision: u64,
    pub(crate) input_hash: String,
    pub(crate) mutation: IdentityMutationV1,
}

impl Genesis {
    pub(crate) fn matches(&self, namespace: &str, seq: i64, previous_hash: &str) -> bool {
        self.version == 4
            && self.control_namespace_uuid == namespace
            && seq.checked_sub(1) == Some(self.previous_seq)
            && self.previous_hash == previous_hash
    }
}

impl Record {
    pub(crate) fn decode_replay(value: &Value) -> anyhow::Result<Self> {
        let record: Self = serde_json::from_value(value.clone())?;
        invalid(record.version == 1 && record.input_hash == record.mutation.input_hash()?)?;
        Ok(record)
    }

    pub(crate) fn verify_revision(&self, projection: &Projection) -> anyhow::Result<()> {
        invalid(self.authorization_revision == projection.revision)
    }
}

const TABLES: [&str; 7] = [
    "identity_meta",
    "identity_accounts",
    "identity_credentials",
    "identity_memberships",
    "identity_resources",
    "identity_shares",
    "identity_operations",
];

pub(crate) enum ProjectionCell {
    Integer(i64),
    Text(String),
}

pub(crate) type ProjectionRows = BTreeMap<&'static str, Vec<Vec<ProjectionCell>>>;

impl Projection {
    #[cfg(feature = "mysql-storage")]
    pub(crate) fn has_local_credential_binding(&self, digest: &[u8; 32]) -> bool {
        let expected = hex::encode(digest);
        self.credentials
            .values()
            .any(|(credential, _)| credential.sha256_digest == expected)
    }

    pub(crate) fn rows(
        &self,
        genesis_seq: i64,
        genesis_hash: &str,
    ) -> anyhow::Result<ProjectionRows> {
        let text = |s: String| ProjectionCell::Text(s);
        let int = |i: i64| ProjectionCell::Integer(i);
        let mut result: BTreeMap<_, Vec<_>> = TABLES
            .into_iter()
            .map(|table| (table, Vec::new()))
            .collect();
        result
            .get_mut("identity_meta")
            .context("missing identity table")?
            .push(vec![
                int(1),
                int(i64::try_from(self.revision)?),
                int(genesis_seq),
                text(genesis_hash.to_owned()),
            ]);
        for (key, (account, seq)) in &self.accounts {
            result
                .get_mut("identity_accounts")
                .context("missing identity table")?
                .push(vec![
                    text(key.clone()),
                    text(serde_json::to_string(account)?),
                    int(*seq),
                ]);
        }
        for (id, (c, seq)) in &self.credentials {
            result
                .get_mut("identity_credentials")
                .context("missing identity table")?
                .push(vec![
                    text(id.clone()),
                    text(c.principal.key()?),
                    text(c.sha256_digest.clone()),
                    int(i64::from(c.active)),
                    text(serde_json::to_string(c)?),
                    int(*seq),
                ]);
        }
        for (id, (m, seq)) in &self.memberships {
            result
                .get_mut("identity_memberships")
                .context("missing identity table")?
                .push(vec![
                    text(id.clone()),
                    text(m.principal.key()?),
                    text(m.organization_scope.clone()),
                    int(i64::from(m.active)),
                    text(serde_json::to_string(m)?),
                    int(*seq),
                ]);
        }
        for (key, (g, seq)) in &self.resources {
            let kind = serde_json::to_value(g.resource.kind)?
                .as_str()
                .context("invalid resource kind")?
                .to_owned();
            result
                .get_mut("identity_resources")
                .context("missing identity table")?
                .push(vec![
                    text(key.clone()),
                    text(kind),
                    text(g.resource.resource_id.clone()),
                    text(g.owner.key()?),
                    int(i64::from(g.active)),
                    text(serde_json::to_string(g)?),
                    int(*seq),
                ]);
            let mut shares = g
                .shares
                .iter()
                .map(|s| Ok((s.principal.key()?, serde_json::to_string(&s.permissions)?)))
                .collect::<anyhow::Result<Vec<_>>>()?;
            shares.sort();
            for (principal, permissions) in shares {
                result
                    .get_mut("identity_shares")
                    .context("missing identity table")?
                    .push(vec![text(key.clone()), text(principal), text(permissions)]);
            }
        }
        for (id, (hash, seq)) in &self.operations {
            result
                .get_mut("identity_operations")
                .context("missing identity table")?
                .push(vec![text(id.clone()), text(hash.clone()), int(*seq)]);
        }
        Ok(result)
    }
}
