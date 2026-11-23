//! Administration read of the organization's memberships (`read_memberships`)
//! and the delegated membership change (`manage_memberships`), each in one
//! checked transaction of the tenant store.
//!
//! The read lists accounts, memberships and local credential metadata of the
//! reader's organization only; it never returns a credential digest. The
//! change is an ordinary identity mutation with the authenticated reader as
//! actor, so the projection's delegation rules apply: no self-change, no
//! capability the actor does not hold, no control-only capability introduced.
use super::*;
use crate::identity::*;

/// Immutable response anchors supplied only by the selected server context.
pub(crate) struct HumanMembershipFrameV1 {
    context_id: String,
    authorization_version: String,
}
impl HumanMembershipFrameV1 {
    pub(crate) fn new(context_id: &str, authorization_version: &str) -> anyhow::Result<Self> {
        for value in [context_id, authorization_version] {
            anyhow::ensure!(
                value.len() == 64
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "invalid human membership frame anchor"
            );
        }
        Ok(Self {
            context_id: context_id.into(),
            authorization_version: authorization_version.into(),
        })
    }
}

pub(crate) struct HumanMembershipStorePageV1 {
    pub page: HumanMembershipPageV1,
    pub after_membership_id: Option<String>,
}

impl Store {
    pub(crate) fn human_memberships(
        &self,
        scope: &AuthorizedScopeV1,
        frame: &HumanMembershipFrameV1,
        after: Option<&str>,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanMembershipStorePageV1> {
        anyhow::ensure!(
            (1..=HUMAN_MAX_PAGE_ITEMS).contains(&limit),
            "invalid human membership page"
        );
        if let Some(after) = after {
            crate::control::uuid(after)?;
        }
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let page = self.human_memberships_checked(&tx, scope, frame, after, limit, transient)?;
        self.commit_checked(tx)?;
        Ok(page)
    }
    /// The same read on an owned 14/15 source, inside its scoped transaction.
    pub(crate) fn modern_human_memberships(
        &self,
        access: &HumanSourceAccess<'_>,
        scope: &AuthorizedScopeV1,
        frame: &HumanMembershipFrameV1,
        after: Option<&str>,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanMembershipStorePageV1> {
        self.with_modern_read(access, |tx| {
            self.human_memberships_checked(tx, scope, frame, after, limit, transient)
        })
    }
    fn human_memberships_checked(
        &self,
        tx: &CheckedTransaction<'_>,
        scope: &AuthorizedScopeV1,
        frame: &HumanMembershipFrameV1,
        after: Option<&str>,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanMembershipStorePageV1> {
        anyhow::ensure!(
            (1..=HUMAN_MAX_PAGE_ITEMS).contains(&limit),
            "invalid human membership page"
        );
        if let Some(after) = after {
            crate::control::uuid(after)?;
        }
        identity::require_human_read(tx)?;
        let current = identity::revalidate_scope_checked(tx, scope, &self.tenant_id)?;
        if current.mode() != ContextModeV1::Administration
            || !current.has_capability(HumanCapabilityV1::ReadMemberships)
        {
            return Err(IdentityFailureV1::PermissionDenied.into());
        }
        if !super::human_rights::installed(tx)? {
            return Err(IdentityFailureV1::MigrationRequired.into());
        }
        let redactor = self.redactor.union(transient);
        let evaluated_at = crate::broker_registry::format_time(chrono::Utc::now());
        let reader = current.principal().key()?;
        let mut items = Vec::new();
        let mut position = after.map(str::to_owned);
        let mut has_more = false;
        let mut bytes = 2usize;
        {
            let mut statement = tx.prepare(
                "SELECT m.membership_id,m.principal_key,m.payload_json,a.payload_json FROM identity_memberships m JOIN identity_accounts a ON a.principal_key=m.principal_key WHERE m.organization_scope=?1 AND m.membership_id>?2 ORDER BY m.membership_id LIMIT ?3",
            )?;
            let rows = statement
                .query_map(
                    params![
                        current.organization_scope(),
                        after.unwrap_or(""),
                        i64::try_from(limit + 1)?
                    ],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                        ))
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (membership_id, principal_key, membership, account) in rows {
                if items.len() == limit {
                    has_more = true;
                    break;
                }
                let membership: MembershipV1 = serde_json::from_str(&membership)?;
                let account: HumanAccountV1 = serde_json::from_str(&account)?;
                anyhow::ensure!(
                    membership.membership_id == membership_id
                        && membership.principal == account.principal
                        && membership.tenant_id == self.tenant_id,
                    "membership projection differs from its account"
                );
                let (credentials, credentials_complete) = local_credentials(tx, &principal_key)?;
                let item = HumanMembershipItemV1 {
                    membership_id: membership.membership_id.clone(),
                    principal: membership.principal.clone(),
                    display_name: account.display_name,
                    account_active: account.active,
                    organization_scope: membership.organization_scope,
                    capabilities: membership.capabilities.into_iter().collect(),
                    revision: membership.revision.to_string(),
                    active: membership.active,
                    credentials,
                    credentials_complete,
                    own: principal_key == reader,
                };
                item.require_unredacted_metadata(&redactor)?;
                let size = serde_json::to_vec(&item)?
                    .len()
                    .checked_add(1)
                    .context("membership page size overflow")?;
                if bytes
                    .checked_add(size)
                    .is_none_or(|total| total > HUMAN_MAX_PAGE_BYTES - HUMAN_PAGE_ENVELOPE_RESERVE)
                {
                    anyhow::ensure!(!items.is_empty(), "membership exceeds page byte limit");
                    has_more = true;
                    break;
                }
                bytes += size;
                position = Some(item.membership_id.clone());
                items.push(item);
            }
        }
        let read_receipt = self.record_human_read_checked(
            tx,
            current.principal(),
            Some(&current),
            "memberships",
            items.len(),
            transient,
        )?;
        let page = HumanMembershipPageV1 {
            version: 1,
            context_id: frame.context_id.clone(),
            authorization_version: frame.authorization_version.clone(),
            source: "tenant_identity_memberships".into(),
            consistency: "snapshot_per_page".into(),
            evaluated_at,
            items,
            has_more,
            next_cursor: has_more.then(|| "0".repeat(HUMAN_CURSOR_HEX_BYTES)),
            read_receipt,
        };
        anyhow::ensure!(
            serde_json::to_vec(&page)?.len() <= HUMAN_MAX_PAGE_BYTES,
            "membership page exceeds its byte bound"
        );
        Ok(HumanMembershipStorePageV1 {
            page,
            after_membership_id: position,
        })
    }
}

fn local_credentials(
    tx: &Connection,
    principal_key: &str,
) -> anyhow::Result<(Vec<HumanMemberCredentialV1>, bool)> {
    let mut statement = tx.prepare(
        "SELECT payload_json FROM identity_credentials WHERE principal_key=?1 ORDER BY credential_id LIMIT ?2",
    )?;
    let rows = statement
        .query_map(
            params![principal_key, i64::try_from(MEMBER_CREDENTIALS_MAX + 1)?],
            |r| r.get::<_, String>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let complete = rows.len() <= MEMBER_CREDENTIALS_MAX;
    let credentials = rows
        .into_iter()
        .take(MEMBER_CREDENTIALS_MAX)
        .map(|row| {
            let credential: LocalCredentialV1 = serde_json::from_str(&row)?;
            Ok(HumanMemberCredentialV1 {
                credential_id: credential.credential_id,
                active: credential.active,
                expires_at: credential.expires_at,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok((credentials, complete))
}

impl Store {
    #[cfg(test)]
    pub(crate) fn test_fail_after_membership_commit(
        &self,
        operation_id: String,
    ) -> Arc<AtomicBool> {
        // The existing storage fault seam fires only once a separate SQLite
        // reader sees the actual committed operation.
        let observer = Connection::open_with_flags(
            self.backing.path(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .unwrap();
        observer.busy_timeout(std::time::Duration::ZERO).unwrap();
        let injected = Arc::new(AtomicBool::new(false));
        let reached = injected.clone();
        self.backing.test_hook.set(move || {
            if !reached.load(Ordering::Acquire)
                && observer.query_row(
                    "SELECT EXISTS(SELECT 1 FROM identity_operations WHERE operation_id=?1)",
                    [&operation_id],
                    |row| row.get::<_, bool>(0),
                )?
            {
                reached.store(true, Ordering::Release);
                return Err(std::io::Error::from_raw_os_error(libc::EMFILE).into());
            }
            Ok(())
        });
        injected
    }

    /// Delegated change of one existing membership of the reader's
    /// organization (`memberships_manage`), on a legacy store.
    pub(crate) fn human_change_membership(
        &self,
        scope: &AuthorizedScopeV1,
        membership_id: &str,
        change: &HumanMembershipChangeV1,
    ) -> anyhow::Result<IdentityReceiptV1> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let mutation = self.prepare_human_membership_change(&tx, scope, membership_id, change)?;
        self.mutate_identity_checked(tx, &mutation)
    }
    /// The same change on an owned 14/15 source, under its write epoch. The
    /// epoch is never held across a second wait for the store connection.
    pub(crate) fn modern_human_change_membership(
        &self,
        epoch: &crate::human_authority::HumanSourcesWriteEpoch,
        source: &crate::human_authority::OwnedSourceToken,
        scope: &AuthorizedScopeV1,
        membership_id: &str,
        change: &HumanMembershipChangeV1,
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
            self.prepare_human_membership_change(tx, scope, membership_id, change)
        })
    }
    /// Principal, tenant and organization come from the stored membership.
    /// Besides the projection's delegation rules, a capability the change adds
    /// must be held by the reader's effective context, so an OIDC ceiling
    /// bounds what it can delegate.
    fn prepare_human_membership_change(
        &self,
        tx: &CheckedTransaction<'_>,
        scope: &AuthorizedScopeV1,
        membership_id: &str,
        change: &HumanMembershipChangeV1,
    ) -> anyhow::Result<IdentityMutationV1> {
        crate::control::uuid(membership_id).map_err(|_| IdentityFailureV1::InvalidRequest)?;
        let expected_revision: u64 = change
            .expected_revision
            .parse()
            .map_err(|_| IdentityFailureV1::InvalidRequest)?;
        if !super::human_rights::installed(tx)? {
            return Err(IdentityFailureV1::MigrationRequired.into());
        }
        let current = identity::revalidate_scope_checked(tx, scope, &self.tenant_id)?;
        if current.mode() != ContextModeV1::Administration
            || !current.has_capability(HumanCapabilityV1::ManageMemberships)
            || !current.has_capability(HumanCapabilityV1::ReadMemberships)
        {
            return Err(IdentityFailureV1::PermissionDenied.into());
        }
        let stored: Option<String> = tx
            .query_row(
                "SELECT payload_json FROM identity_memberships WHERE membership_id=?1",
                [membership_id],
                |r| r.get(0),
            )
            .optional()?;
        let stored: MembershipV1 =
            serde_json::from_str(&stored.ok_or(IdentityFailureV1::NotFound)?)?;
        if stored.organization_scope != current.organization_scope()
            || stored.tenant_id != self.tenant_id
        {
            // Another organization's membership is not disclosed.
            return Err(IdentityFailureV1::NotFound.into());
        }
        for capability in &change.capabilities {
            let retained =
                stored.capabilities.contains(capability) && (stored.active || !change.active);
            if !retained && !current.has_capability(*capability) {
                return Err(IdentityFailureV1::PermissionDenied.into());
            }
        }
        let mutation = IdentityMutationV1 {
            version: 1,
            operation_id: change.operation_id.clone(),
            actor: current.principal().clone(),
            change: IdentityChangeV1::Membership {
                expected_revision,
                membership: MembershipV1 {
                    revision: expected_revision
                        .checked_add(1)
                        .ok_or(IdentityFailureV1::InvalidRequest)?,
                    active: change.active,
                    capabilities: change.capabilities.iter().copied().collect(),
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
