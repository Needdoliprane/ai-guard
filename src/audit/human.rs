//! Authorized human snapshots. SQL applies resource ACLs before pagination.
use super::*;
use crate::identity::*;
use rusqlite::named_params;
mod recent;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HumanReadAuditV1 {
    pub version: u32,
    pub request_id: String,
    pub principal: PrincipalRefV1,
    pub membership_id: Option<String>,
    pub mode: Option<ContextModeV1>,
    pub membership_revision: Option<u64>,
    pub authorization_revision: u64,
    pub action: String,
    pub result_count: usize,
}

pub(crate) struct HumanStorePageV1<T> {
    pub items: Vec<T>,
    pub after_resource: String,
    pub after_event: i64,
    pub upper_event: i64,
    pub has_more: bool,
    pub receipt: HumanReadReceiptV1,
}
pub(crate) struct HumanEventPollV1 {
    pub page: Option<HumanStorePageV1<HumanEventItemV1>>,
    pub after_event: i64,
    pub archived_through_seq: Option<i64>,
}
pub(crate) struct HumanProfileSnapshotV1 {
    pub account: Option<HumanAccountV1>,
    pub memberships: Vec<MembershipV1>,
    pub authorization_revision: Option<u64>,
    /// The tenant store carries the validated human rights marker.
    pub human_rights_marker: bool,
}

#[derive(Clone)]
pub(crate) struct HumanEventFilterV1 {
    pub view: HumanReadViewV1,
    pub resource: Option<ResourceRefV1>,
    pub event_id: Option<String>,
    pub export: bool,
    pub stream: bool,
}

fn allowed(scope: &AuthorizedScopeV1, permission: ResourcePermissionV1) -> String {
    if permission == ResourcePermissionV1::Metadata
        && scope.mode() == ContextModeV1::Administration
        && scope.has_capability(HumanCapabilityV1::ReadTenantMetadata)
    {
        return "(:principal IS NOT NULL)".into();
    }
    let (capability, name) = match permission {
        ResourcePermissionV1::Metadata => (HumanCapabilityV1::ReadMetadata, "metadata"),
        ResourcePermissionV1::Content => (HumanCapabilityV1::ReadContent, "content"),
        ResourcePermissionV1::Export => (HumanCapabilityV1::Export, "export"),
    };
    if !scope.has_capability(capability) {
        return "(:principal IS NOT NULL AND 0)".into();
    }
    format!(
        "(r.owner_key=:principal OR EXISTS(SELECT 1 FROM identity_shares s,json_each(s.permissions_json) p WHERE s.resource_key=r.resource_key AND s.principal_key=:principal AND p.value='{name}'))"
    )
}

/// The authorized-resource predicate of a human read, for sibling Store reads
/// that count instead of listing. Bound parameter `:principal` is required.
pub(super) fn allowed_sql(scope: &AuthorizedScopeV1, permission: ResourcePermissionV1) -> String {
    allowed(scope, permission)
}

fn require_view(scope: &AuthorizedScopeV1, filter: &HumanEventFilterV1) -> anyhow::Result<()> {
    let metadata = scope.has_capability(HumanCapabilityV1::ReadMetadata)
        || (scope.mode() == ContextModeV1::Administration
            && scope.has_capability(HumanCapabilityV1::ReadTenantMetadata));
    if !metadata
        || (filter.view == HumanReadViewV1::Content
            && !scope.has_capability(HumanCapabilityV1::ReadContent))
        || (filter.export && !scope.has_capability(HumanCapabilityV1::Export))
    {
        return Err(IdentityFailureV1::PermissionDenied.into());
    }
    if let Some(resource) = &filter.resource {
        resource.validate()?;
    }
    Ok(())
}

pub(super) fn validate_resource_page(after: &str, limit: usize) -> anyhow::Result<()> {
    anyhow::ensure!(
        (1..=HUMAN_MAX_PAGE_ITEMS).contains(&limit) && after.len() <= 64,
        "invalid human resource page"
    );
    Ok(())
}

pub(super) fn validate_read(record: &HumanReadAuditV1) -> anyhow::Result<()> {
    anyhow::ensure!(
        record.version == 1
            && record.principal.is_human()
            && uuid::Uuid::parse_str(&record.request_id).is_ok()
            && ([
                "identity",
                "resources",
                "events",
                "event",
                "export",
                "stream"
            ]
            .contains(&record.action.as_str())
                || super::human_rights::read_action_requires_marker(&record.action))
            && record.result_count <= HUMAN_MAX_PAGE_ITEMS,
        "invalid human read audit"
    );
    record.principal.validate()?;
    anyhow::ensure!(
        i64::try_from(record.authorization_revision).is_ok(),
        "human read revision exceeds SQLite range"
    );
    if record.action == "identity" {
        anyhow::ensure!(
            record.membership_id.is_none()
                && record.mode.is_none()
                && record.membership_revision.is_none(),
            "unexpected identity read scope"
        );
    } else {
        anyhow::ensure!(
            record
                .membership_id
                .as_ref()
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
                && record.mode.is_some()
                && record
                    .membership_revision
                    .is_some_and(|r| r > 0 && i64::try_from(r).is_ok()),
            "missing human read scope"
        );
    }
    Ok(())
}

impl Store {
    pub(crate) fn try_local_credential_binding(
        &self,
        digest: &[u8; 32],
    ) -> MetadataLookup<LocalCredentialV1> {
        self.with_known_binding(
            self.try_read_metadata(|db| local_credential_binding(db, digest)),
            |bindings| bindings.local.get(&hex::encode(digest)).cloned(),
        )
    }
    /// Presence includes revoked/expired bindings so they cannot shadow another
    /// credential class. No bearer or digest is returned to HTTP readers.
    pub(crate) fn local_credential_binding(
        &self,
        digest: &[u8; 32],
    ) -> anyhow::Result<Option<LocalCredentialV1>> {
        self.local_credential_binding_impl(None, digest)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn local_credential_binding_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        digest: &[u8; 32],
    ) -> anyhow::Result<Option<LocalCredentialV1>> {
        self.local_credential_binding_impl(Some(access), digest)
    }
    fn local_credential_binding_impl(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        digest: &[u8; 32],
    ) -> anyhow::Result<Option<LocalCredentialV1>> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = match access {
            Some(access) => {
                self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?
            }
            None => self.begin_checked(&mut db)?,
        };
        let binding = if identity::identity_read_format(&tx)? {
            identity::json_row(
                &tx,
                "SELECT payload_json FROM identity_credentials WHERE digest=?1",
                &hex::encode(digest),
            )?
        } else {
            None
        };
        self.commit_checked(tx)?;
        Ok(binding)
    }
    pub(crate) fn authenticate_local_digest(
        &self,
        digest: &[u8; 32],
    ) -> anyhow::Result<Option<VerifiedLocalCredentialV1>> {
        self.authenticate_local_digest_impl(None, digest)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn authenticate_local_digest_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        digest: &[u8; 32],
    ) -> anyhow::Result<Option<VerifiedLocalCredentialV1>> {
        self.authenticate_local_digest_impl(Some(access), digest)
    }
    fn authenticate_local_digest_impl(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        digest: &[u8; 32],
    ) -> anyhow::Result<Option<VerifiedLocalCredentialV1>> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = match access {
            Some(access) => {
                self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?
            }
            None => self.begin_checked(&mut db)?,
        };
        let result = self.authenticate_local_digest_checked(&tx, digest)?;
        self.commit_checked(tx)?;
        Ok(result)
    }
    pub(super) fn authenticate_local_digest_checked(
        &self,
        tx: &CheckedTransaction<'_>,
        digest: &[u8; 32],
    ) -> anyhow::Result<Option<VerifiedLocalCredentialV1>> {
        if !identity::identity_read_format(tx)? {
            return Ok(None);
        }
        let binding: Option<LocalCredentialV1> = identity::json_row(
            tx,
            "SELECT payload_json FROM identity_credentials WHERE digest=?1",
            &hex::encode(digest),
        )?;
        let Some(binding) = binding else {
            return Ok(None);
        };
        identity::require_human_read(tx)?;
        binding.validate()?;
        if !binding.active {
            return Err(IdentityFailureV1::IdentityRevoked.into());
        }
        if let Some(expiry) = &binding.expires_at
            && chrono::DateTime::parse_from_rfc3339(expiry)?.with_timezone(&chrono::Utc)
                <= chrono::Utc::now()
        {
            return Err(IdentityFailureV1::CredentialExpired.into());
        }
        let account: HumanAccountV1 = identity::json_row(
            tx,
            "SELECT payload_json FROM identity_accounts WHERE principal_key=?1",
            &binding.principal.key()?,
        )?
        .ok_or(IdentityFailureV1::IdentityRevoked)?;
        if !account.active {
            return Err(IdentityFailureV1::IdentityRevoked.into());
        }
        let verified = VerifiedLocalCredentialV1 {
            principal: binding.principal,
            credential_id: binding.credential_id,
            revision: binding.revision,
            expires_at: binding.expires_at,
        };
        Ok(Some(verified))
    }
    pub(crate) fn human_profile(
        &self,
        principal: &PrincipalRefV1,
        transient: &Redactor,
    ) -> anyhow::Result<HumanProfileSnapshotV1> {
        self.human_profile_impl(None, principal, transient)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn human_profile_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        principal: &PrincipalRefV1,
        transient: &Redactor,
    ) -> anyhow::Result<HumanProfileSnapshotV1> {
        self.human_profile_impl(Some(access), principal, transient)
    }
    fn human_profile_impl(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        principal: &PrincipalRefV1,
        transient: &Redactor,
    ) -> anyhow::Result<HumanProfileSnapshotV1> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = match access {
            Some(access) => {
                self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?
            }
            None => self.begin_checked(&mut db)?,
        };
        let result = self.human_profile_checked(&tx, principal, transient)?;
        self.commit_checked(tx)?;
        Ok(result)
    }
    pub(super) fn human_profile_checked(
        &self,
        tx: &CheckedTransaction<'_>,
        principal: &PrincipalRefV1,
        transient: &Redactor,
    ) -> anyhow::Result<HumanProfileSnapshotV1> {
        if !identity::identity_read_format(tx)? {
            return Ok(HumanProfileSnapshotV1 {
                account: None,
                memberships: vec![],
                authorization_revision: None,
                human_rights_marker: false,
            });
        }
        let revision = identity::authorization_revision(tx)?;
        let human_rights_marker = super::human_rights::installed(tx)?;
        let key = principal.key()?;
        let account: Option<HumanAccountV1> = identity::json_row(
            tx,
            "SELECT payload_json FROM identity_accounts WHERE principal_key=?1",
            &key,
        )?;
        let Some(account) = account.filter(|account| account.active) else {
            return Ok(HumanProfileSnapshotV1 {
                account: None,
                memberships: vec![],
                authorization_revision: Some(revision),
                human_rights_marker,
            });
        };
        identity::require_human_read(tx)?;
        let members = {
            let mut stmt = tx.prepare("SELECT payload_json FROM identity_memberships WHERE principal_key=?1 AND active=1 ORDER BY membership_id LIMIT 257")?;
            stmt.query_map([&key], |r| r.get::<_, String>(0))?
                .map(|row| Ok(serde_json::from_str(&row?)?))
                .collect::<anyhow::Result<Vec<MembershipV1>>>()?
        };
        anyhow::ensure!(members.len() <= 256, "human membership capacity exceeded");
        self.record_human_read_checked(tx, principal, None, "identity", 1, transient)?;
        Ok(HumanProfileSnapshotV1 {
            account: Some(account),
            memberships: members,
            authorization_revision: Some(revision),
            human_rights_marker,
        })
    }
    pub(crate) fn human_resources(
        &self,
        scope: &AuthorizedScopeV1,
        after: &str,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanStorePageV1<HumanResourceItemV1>> {
        self.human_resources_impl(None, scope, after, limit, transient)
    }
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn human_resources_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        scope: &AuthorizedScopeV1,
        after: &str,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanStorePageV1<HumanResourceItemV1>> {
        self.human_resources_impl(Some(access), scope, after, limit, transient)
    }
    fn human_resources_impl(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        scope: &AuthorizedScopeV1,
        after: &str,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanStorePageV1<HumanResourceItemV1>> {
        validate_resource_page(after, limit)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = match access {
            Some(access) => {
                self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?
            }
            None => self.begin_checked(&mut db)?,
        };
        let result = self.human_resources_checked(&tx, scope, after, limit, transient)?;
        self.commit_checked(tx)?;
        Ok(result)
    }
    pub(super) fn human_resources_checked(
        &self,
        tx: &CheckedTransaction<'_>,
        scope: &AuthorizedScopeV1,
        after: &str,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanStorePageV1<HumanResourceItemV1>> {
        validate_resource_page(after, limit)?;
        identity::require_human_read(tx)?;
        let current = identity::revalidate_scope_checked(tx, scope, &self.tenant_id)?;
        require_view(
            &current,
            &HumanEventFilterV1 {
                view: HumanReadViewV1::Metadata,
                resource: None,
                event_id: None,
                export: false,
                stream: false,
            },
        )?;
        let sql = format!(
            "SELECT r.resource_key,r.payload_json FROM identity_resources r WHERE r.active=1 AND json_extract(r.payload_json,'$.organization_scope')=:organization AND r.resource_key>:after AND {} ORDER BY r.resource_key LIMIT :limit",
            allowed(&current, ResourcePermissionV1::Metadata)
        );
        let key = current.principal().key()?;
        let mut items = Vec::new();
        let mut position = after.to_owned();
        let mut has_more = false;
        let mut bytes = 2usize;
        {
            let mut statement = tx.prepare(&sql)?;
            let rows = statement.query_map(named_params!{":organization":current.organization_scope(),":principal":key,":after":after,":limit":i64::try_from(limit+1)?},|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?;
            for row in rows {
                let (next, payload) = row?;
                if items.len() == limit {
                    has_more = true;
                    break;
                }
                let grant: ResourceGrantV1 = serde_json::from_str(&payload)?;
                let item = mask(
                    transient,
                    &HumanResourceItemV1 {
                        resource: grant.resource.clone(),
                        owner: grant.owner.clone(),
                        revision: grant.revision.to_string(),
                        permissions: HumanResourcePermissionsV1 {
                            metadata: current
                                .permits_resource(&grant, ResourcePermissionV1::Metadata),
                            content: current
                                .permits_resource(&grant, ResourcePermissionV1::Content),
                            export: current.permits_resource(&grant, ResourcePermissionV1::Export),
                        },
                    },
                )?;
                let size = serde_json::to_vec(&item)?
                    .len()
                    .checked_add(1)
                    .ok_or(IdentityFailureV1::InvalidRequest)?;
                if bytes
                    .checked_add(size)
                    .is_none_or(|total| total > HUMAN_MAX_PAGE_BYTES - HUMAN_PAGE_ENVELOPE_RESERVE)
                {
                    anyhow::ensure!(
                        !items.is_empty(),
                        "authorized resource exceeds page byte limit"
                    );
                    has_more = true;
                    break;
                }
                bytes += size;
                items.push(item);
                position = next;
            }
        }
        let receipt = self.record_human_read_checked(
            tx,
            current.principal(),
            Some(&current),
            "resources",
            items.len(),
            transient,
        )?;
        Ok(HumanStorePageV1 {
            items,
            after_resource: position,
            after_event: 0,
            upper_event: 0,
            has_more,
            receipt,
        })
    }
    pub(crate) fn human_events(
        &self,
        scope: &AuthorizedScopeV1,
        filter: &HumanEventFilterV1,
        after: i64,
        upper: Option<i64>,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanEventPollV1> {
        self.human_events_impl(None, scope, filter, after, upper, limit, transient)
    }
    // Each argument is a distinct authorized selection input; keep the explicit signature.
    // Scoped authorization variant; production wiring pending.
    #[allow(dead_code, clippy::too_many_arguments)]
    pub(crate) fn human_events_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        scope: &AuthorizedScopeV1,
        filter: &HumanEventFilterV1,
        after: i64,
        upper: Option<i64>,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanEventPollV1> {
        self.human_events_impl(Some(access), scope, filter, after, upper, limit, transient)
    }
    // Each argument is a distinct authorized selection input; keep the explicit signature.
    #[allow(clippy::too_many_arguments)]
    fn human_events_impl(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        scope: &AuthorizedScopeV1,
        filter: &HumanEventFilterV1,
        after: i64,
        upper: Option<i64>,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanEventPollV1> {
        anyhow::ensure!(
            (1..=HUMAN_MAX_PAGE_ITEMS).contains(&limit) && after >= 0,
            "invalid human event page"
        );
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = match access {
            Some(access) => {
                self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?
            }
            None => self.begin_checked(&mut db)?,
        };
        let result =
            self.human_events_checked(&tx, scope, filter, after, upper, limit, transient)?;
        self.commit_checked(tx)?;
        Ok(result)
    }
    // Each argument is a distinct authorized selection input; keep the explicit signature.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn human_events_checked(
        &self,
        tx: &CheckedTransaction<'_>,
        scope: &AuthorizedScopeV1,
        filter: &HumanEventFilterV1,
        after: i64,
        upper: Option<i64>,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanEventPollV1> {
        anyhow::ensure!(
            (1..=HUMAN_MAX_PAGE_ITEMS).contains(&limit) && after >= 0,
            "invalid human event page"
        );
        identity::require_human_read(tx)?;
        let current = identity::revalidate_scope_checked(tx, scope, &self.tenant_id)?;
        require_view(&current, filter)?;
        if let Some(resource) = &filter.resource {
            let grant: ResourceGrantV1 = identity::json_row(
                tx,
                "SELECT payload_json FROM identity_resources WHERE resource_key=?1",
                &resource.key()?,
            )?
            .filter(|grant: &ResourceGrantV1| {
                current.permits_resource(grant, ResourcePermissionV1::Metadata)
            })
            .ok_or(IdentityFailureV1::NotFound)?;
            if (filter.view == HumanReadViewV1::Content
                && !current.permits_resource(&grant, ResourcePermissionV1::Content))
                || (filter.export
                    && !current.permits_resource(&grant, ResourcePermissionV1::Export))
            {
                return Err(IdentityFailureV1::PermissionDenied.into());
            }
        }
        let upper = upper.unwrap_or(audit_head(tx)?.0);
        anyhow::ensure!(upper >= after, "invalid human snapshot bound");
        let mut predicates = vec![allowed(&current, ResourcePermissionV1::Metadata)];
        if filter.view == HumanReadViewV1::Content {
            predicates.push(allowed(&current, ResourcePermissionV1::Content));
        }
        if filter.export {
            predicates.push(allowed(&current, ResourcePermissionV1::Export));
        }
        // Keep the exact ID lookup eligible for the unique event ID index.
        let event_id_predicate = if filter.event_id.is_some() {
            "e.id=:event_id"
        } else {
            ":event_id IS NULL"
        };
        let sql = format!(
            "SELECT e.seq,e.payload,e.received_at,r.resource_kind,r.resource_id FROM events e JOIN identity_resources r ON r.resource_kind=CASE WHEN json_extract(e.payload,'$.session_id') IS NOT NULL THEN 'session' WHEN e.sandbox_id IS NOT NULL THEN 'sandbox' WHEN e.agent_id IS NOT NULL THEN 'agent' ELSE 'project' END AND r.resource_id=COALESCE(json_extract(e.payload,'$.session_id'),e.sandbox_id,e.agent_id,e.project_id) WHERE e.seq>:after AND e.seq<=:upper AND e.kind NOT GLOB 'control.identity.*' AND r.active=1 AND json_extract(r.payload_json,'$.organization_scope')=:organization AND (:resource_kind IS NULL OR r.resource_kind=:resource_kind) AND (:resource_id IS NULL OR r.resource_id=:resource_id) AND {event_id_predicate} AND {} ORDER BY e.seq LIMIT :limit",
            predicates.join(" AND ")
        );
        let key = current.principal().key()?;
        let kind = filter
            .resource
            .as_ref()
            .map(|r| serde_json::to_value(r.kind))
            .transpose()?
            .and_then(|v| v.as_str().map(str::to_owned));
        let resource_id = filter.resource.as_ref().map(|r| r.resource_id.as_str());
        let mut items = Vec::new();
        let mut position = after;
        let mut bytes = 0usize;
        let mut has_more = false;
        {
            let mut statement = tx.prepare(&sql)?;
            let rows = statement.query_map(named_params!{":after":after,":upper":upper,":organization":current.organization_scope(),":principal":key,":resource_kind":kind,":resource_id":resource_id,":event_id":filter.event_id,":limit":i64::try_from(limit+1)?},|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?)))?;
            for row in rows {
                let (seq, payload, received_at, kind, resource_id) = row?;
                if items.len() == limit {
                    has_more = true;
                    break;
                }
                let event: Event = serde_json::from_str(&payload)?;
                let resource_kind: ResourceKindV1 = serde_json::from_value(json!(kind))?;
                let mut item = HumanEventItemV1 {
                    metadata: HumanEventMetadataV1 {
                        id: event.id,
                        timestamp: event.timestamp,
                        received_at,
                        kind: event.kind,
                        source: safe_source(&event.source),
                        provenance: event.provenance,
                        coverage: event.coverage,
                        resource: ResourceRefV1 {
                            kind: resource_kind,
                            resource_id,
                        },
                        decision: event.decision.as_ref().map(|d| d.outcome.clone()),
                    },
                    content: None,
                };
                if filter.view == HumanReadViewV1::Content {
                    item.content = Some(HumanEventContentV1 {
                        command: event.command,
                        decision: event.decision,
                        result: event.result,
                        data: event.data,
                    });
                }
                let item = mask(transient, &item)?;
                let size = serde_json::to_vec(&item)?.len();
                if bytes
                    .checked_add(size)
                    .is_none_or(|total| total > HUMAN_MAX_PAGE_BYTES - HUMAN_PAGE_ENVELOPE_RESERVE)
                {
                    anyhow::ensure!(
                        !items.is_empty(),
                        "authorized event exceeds page byte limit"
                    );
                    has_more = true;
                    break;
                }
                bytes += size;
                items.push(item);
                position = seq;
            }
        }
        if filter.stream && items.is_empty() {
            return Ok(HumanEventPollV1 {
                page: None,
                after_event: upper,
                archived_through_seq: None,
            });
        }
        let archived_through_seq = if filter.stream {
            None
        } else {
            let (last_seq, _) = super::retention::ledger::base(tx)?;
            (last_seq > 0).then_some(last_seq)
        };
        let action = if filter.stream {
            "stream"
        } else if filter.export {
            "export"
        } else if filter.event_id.is_some() {
            "event"
        } else {
            "events"
        };
        let receipt = self.record_human_read_checked(
            tx,
            current.principal(),
            Some(&current),
            action,
            items.len(),
            transient,
        )?;
        let after_event = if has_more { position } else { upper };
        Ok(HumanEventPollV1 {
            after_event,
            archived_through_seq,
            page: Some(HumanStorePageV1 {
                items,
                after_resource: String::new(),
                after_event,
                upper_event: upper,
                has_more,
                receipt,
            }),
        })
    }
    // Scoped read-audit variant; production wiring pending.
    #[allow(dead_code)]
    pub(super) fn record_human_read(
        &self,
        tx: &Transaction<'_>,
        principal: &PrincipalRefV1,
        scope: Option<&AuthorizedScopeV1>,
        action: &str,
        count: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanReadReceiptV1> {
        identity::require_human_access(tx)?;
        self.persist_human_read(tx, principal, scope, action, count, transient)
    }
    pub(super) fn record_human_read_checked(
        &self,
        tx: &CheckedTransaction<'_>,
        principal: &PrincipalRefV1,
        scope: Option<&AuthorizedScopeV1>,
        action: &str,
        count: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanReadReceiptV1> {
        identity::require_human_read(tx)?;
        self.persist_human_read(tx, principal, scope, action, count, transient)
    }
    fn persist_human_read(
        &self,
        tx: &Transaction<'_>,
        principal: &PrincipalRefV1,
        scope: Option<&AuthorizedScopeV1>,
        action: &str,
        count: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanReadReceiptV1> {
        // Format fence before any receipt: a newer read action never enters a
        // history that an older binary could still replay.
        if super::human_rights::read_action_requires_marker(action) {
            if !super::human_rights::installed(tx)? {
                return Err(IdentityFailureV1::MigrationRequired.into());
            }
            if !scope.is_some_and(|scope| super::human_rights::read_action_permitted(action, scope))
            {
                return Err(IdentityFailureV1::PermissionDenied.into());
            }
        }
        let request_id = uuid::Uuid::new_v4().to_string();
        let record = HumanReadAuditV1 {
            version: 1,
            request_id: request_id.clone(),
            principal: principal.clone(),
            membership_id: scope.map(|s| s.membership_id().to_owned()),
            mode: scope.map(AuthorizedScopeV1::mode),
            membership_revision: scope.map(AuthorizedScopeV1::membership_revision),
            authorization_revision: identity::authorization_revision(tx)?,
            action: action.into(),
            result_count: count,
        };
        validate_read(&record)?;
        let mut event = Event::new("control.identity.access");
        event.data = serde_json::to_value(&record)?;
        let protected = transient.value(&serde_json::to_value(&event)?);
        anyhow::ensure!(
            protected == serde_json::to_value(&event)?,
            "human read audit identity requires redaction"
        );
        let (event, payload) = self.prepare_event(event)?;
        let stored = self.append_human_in_transaction(tx, event, payload, principal)?;
        Ok(HumanReadReceiptV1 {
            request_id,
            audit_event_id: stored.event.id,
            received_at: stored.received_at,
        })
    }
}

fn local_credential_binding(
    db: &Connection,
    digest: &[u8; 32],
) -> anyhow::Result<Option<LocalCredentialV1>> {
    let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if [4, 5, 6, 7, 8, 9, 10, 11, 12, 13].contains(&version) {
        identity::json_row(
            db,
            "SELECT payload_json FROM identity_credentials WHERE digest=?1",
            &hex::encode(digest),
        )
    } else {
        Ok(None)
    }
}

fn mask<T: Serialize + serde::de::DeserializeOwned>(
    transient: &Redactor,
    value: &T,
) -> anyhow::Result<T> {
    Ok(serde_json::from_value(
        transient.value(&serde_json::to_value(value)?),
    )?)
}
fn safe_source(source: &str) -> String {
    if !source.is_empty()
        && source.len() <= 32
        && source.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'.')
        })
    {
        source.into()
    } else {
        "unknown".into()
    }
}
