//! Newest-first page of the events a human context may read.
//!
//! Authorization is the one of `human_events`: the same scope revalidation,
//! view check, filtered-resource check and ACL predicates, applied in SQL
//! before the limit. Only the order and the bounds differ. The read records
//! one `events` receipt in the same transaction before anything is returned.
use super::*;

impl Store {
    /// `upper` is the inclusive audit position to read down from: `None` for
    /// the first page, bounded by the audit head of this transaction, or the
    /// position just below the previous page for an older one.
    ///
    /// In the returned page, `upper_event` is that snapshot bound and
    /// `after_event` the position of the oldest item (0 when empty). Items
    /// are ordered from the most recent to the oldest; `has_more` reports
    /// older authorized events below `after_event`.
    pub(crate) fn human_events_recent(
        &self,
        scope: &AuthorizedScopeV1,
        filter: &HumanEventFilterV1,
        upper: Option<i64>,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanStorePageV1<HumanEventItemV1>> {
        self.human_events_recent_impl(None, scope, filter, upper, limit, transient)
    }
    pub(crate) fn human_events_recent_scoped(
        &self,
        access: &HumanSourceAccess<'_>,
        scope: &AuthorizedScopeV1,
        filter: &HumanEventFilterV1,
        upper: Option<i64>,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanStorePageV1<HumanEventItemV1>> {
        self.human_events_recent_impl(Some(access), scope, filter, upper, limit, transient)
    }
    fn human_events_recent_impl(
        &self,
        access: Option<&HumanSourceAccess<'_>>,
        scope: &AuthorizedScopeV1,
        filter: &HumanEventFilterV1,
        upper: Option<i64>,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanStorePageV1<HumanEventItemV1>> {
        anyhow::ensure!(
            (1..=HUMAN_MAX_PAGE_ITEMS).contains(&limit)
                && upper.is_none_or(|bound| bound >= 0)
                && filter.event_id.is_none()
                && !filter.export
                && !filter.stream,
            "invalid recent human event page"
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
        identity::require_human_read(&tx)?;
        let current = identity::revalidate_scope_checked(&tx, scope, &self.tenant_id)?;
        require_view(&current, filter)?;
        if let Some(resource) = &filter.resource {
            let grant: ResourceGrantV1 = identity::json_row(
                &tx,
                "SELECT payload_json FROM identity_resources WHERE resource_key=?1",
                &resource.key()?,
            )?
            .filter(|grant: &ResourceGrantV1| {
                current.permits_resource(grant, ResourcePermissionV1::Metadata)
            })
            .ok_or(IdentityFailureV1::NotFound)?;
            if filter.view == HumanReadViewV1::Content
                && !current.permits_resource(&grant, ResourcePermissionV1::Content)
            {
                return Err(IdentityFailureV1::PermissionDenied.into());
            }
        }
        let head = audit_head(&tx)?.0;
        let upper = upper.unwrap_or(head);
        anyhow::ensure!(upper <= head, "invalid recent human snapshot bound");
        let mut predicates = vec![allowed(&current, ResourcePermissionV1::Metadata)];
        if filter.view == HumanReadViewV1::Content {
            predicates.push(allowed(&current, ResourcePermissionV1::Content));
        }
        // Same selection as `human_events`, read downwards. The differential
        // test in tests/human_recent_events.rs keeps both in agreement.
        let sql = format!(
            "SELECT e.seq,e.payload,e.received_at,r.resource_kind,r.resource_id FROM events e JOIN identity_resources r ON r.resource_kind=CASE WHEN json_extract(e.payload,'$.session_id') IS NOT NULL THEN 'session' WHEN e.sandbox_id IS NOT NULL THEN 'sandbox' WHEN e.agent_id IS NOT NULL THEN 'agent' ELSE 'project' END AND r.resource_id=COALESCE(json_extract(e.payload,'$.session_id'),e.sandbox_id,e.agent_id,e.project_id) WHERE e.seq<=:upper AND e.kind NOT GLOB 'control.identity.*' AND r.active=1 AND json_extract(r.payload_json,'$.organization_scope')=:organization AND (:resource_kind IS NULL OR r.resource_kind=:resource_kind) AND (:resource_id IS NULL OR r.resource_id=:resource_id) AND {} ORDER BY e.seq DESC LIMIT :limit",
            predicates.join(" AND ")
        );
        let key = current.principal().key()?;
        let kind = filter
            .resource
            .as_ref()
            .map(|resource| serde_json::to_value(resource.kind))
            .transpose()?
            .and_then(|value| value.as_str().map(str::to_owned));
        let resource_id = filter
            .resource
            .as_ref()
            .map(|resource| resource.resource_id.as_str());
        let mut items = Vec::new();
        let mut oldest = 0i64;
        let mut bytes = 0usize;
        let mut has_more = false;
        {
            let mut statement = tx.prepare(&sql)?;
            let rows = statement.query_map(
                named_params! {
                    ":upper": upper,
                    ":organization": current.organization_scope(),
                    ":principal": key,
                    ":resource_kind": kind,
                    ":resource_id": resource_id,
                    ":limit": i64::try_from(limit + 1)?,
                },
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )?;
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
                        decision: event
                            .decision
                            .as_ref()
                            .map(|decision| decision.outcome.clone()),
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
                oldest = seq;
            }
        }
        let receipt = self.record_human_read_checked(
            &tx,
            current.principal(),
            Some(&current),
            "events",
            items.len(),
            transient,
        )?;
        self.commit_checked(tx)?;
        Ok(HumanStorePageV1 {
            items,
            after_resource: String::new(),
            after_event: oldest,
            upper_event: upper,
            has_more,
            receipt,
        })
    }
}
