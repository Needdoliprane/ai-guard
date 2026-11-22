//! Administration reads of the tenant quota and of the observed allocations
//! (`read_worker_registry`), each recorded under its own human read action.
//!
//! Reservations are durable control facts, not consumption. Allocations are
//! terminal observations of method 1 with `billable=false`: neither a price
//! nor an invoice. Decimal products stay strings; nothing is rounded.
use super::*;
use crate::{
    control::{AllocationV1, QuotaPolicyV1},
    identity::*,
};

/// Immutable response anchors supplied only by the selected server context.
pub(crate) struct HumanUsageFrameV1 {
    context_id: String,
    authorization_version: String,
}
impl HumanUsageFrameV1 {
    pub(crate) fn new(context_id: &str, authorization_version: &str) -> anyhow::Result<Self> {
        for value in [context_id, authorization_version] {
            anyhow::ensure!(
                value.len() == 64
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "invalid human usage frame anchor"
            );
        }
        Ok(Self {
            context_id: context_id.into(),
            authorization_version: authorization_version.into(),
        })
    }
}

pub(crate) struct HumanUsageStorePageV1 {
    pub page: HumanUsagePageV1,
    pub after_attempt_id: Option<String>,
}

impl Store {
    fn usage_scope(
        &self,
        tx: &CheckedTransaction<'_>,
        scope: &AuthorizedScopeV1,
    ) -> anyhow::Result<AuthorizedScopeV1> {
        identity::require_human_read(tx)?;
        let current = identity::revalidate_scope_checked(tx, scope, &self.tenant_id)?;
        if current.mode() != ContextModeV1::Administration
            || !current.has_capability(HumanCapabilityV1::ReadWorkerRegistry)
        {
            return Err(IdentityFailureV1::PermissionDenied.into());
        }
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if !(6..=13).contains(&version) || !super::human_rights::installed(tx)? {
            return Err(IdentityFailureV1::MigrationRequired.into());
        }
        Ok(current)
    }

    pub(crate) fn human_quota(
        &self,
        scope: &AuthorizedScopeV1,
        frame: &HumanUsageFrameV1,
        policy: &QuotaPolicyV1,
        transient: &Redactor,
    ) -> anyhow::Result<HumanQuotaV1> {
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let current = self.usage_scope(&tx, scope)?;
        let usage = super::control::quota_usage(&tx, policy)?;
        let read_receipt = self.record_human_read_checked(
            &tx,
            current.principal(),
            Some(&current),
            "quota",
            1,
            transient,
        )?;
        let body = HumanQuotaV1 {
            version: 1,
            context_id: frame.context_id.clone(),
            authorization_version: frame.authorization_version.clone(),
            evaluated_at: crate::broker_registry::format_time(chrono::Utc::now()),
            source: "tenant_worker_reservations".into(),
            quota: SummaryQuotaV1 {
                max_active_workers: usage.policy.max_active_workers.to_string(),
                max_cpu_millis: usage.policy.max_cpu_millis.to_string(),
                max_memory_bytes: usage.policy.max_memory_bytes.to_string(),
                held_workers: usage.held_workers.to_string(),
                held_cpu_millis: usage.held_cpu_millis.to_string(),
                held_memory_bytes: usage.held_memory_bytes.to_string(),
            },
            max_timeout_seconds: usage.policy.max_timeout_seconds.to_string(),
            read_receipt,
        };
        self.commit_checked(tx)?;
        Ok(body)
    }

    pub(crate) fn human_usage(
        &self,
        scope: &AuthorizedScopeV1,
        frame: &HumanUsageFrameV1,
        after: Option<&str>,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanUsageStorePageV1> {
        anyhow::ensure!(
            (1..=HUMAN_MAX_PAGE_ITEMS).contains(&limit),
            "invalid human usage page"
        );
        if let Some(after) = after {
            crate::control::uuid(after)?;
        }
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked(&mut db)?;
        let current = self.usage_scope(&tx, scope)?;
        let mut statement = tx.prepare(
            "SELECT attempt_id,payload_json FROM worker_allocations WHERE attempt_id>?1 ORDER BY attempt_id LIMIT ?2",
        )?;
        let rows = statement
            .query_map(
                params![after.unwrap_or(""), i64::try_from(limit + 1)?],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        let has_more = rows.len() > limit;
        let mut items = Vec::new();
        let mut position = after.map(str::to_owned);
        for (attempt_id, payload) in rows.into_iter().take(limit) {
            let allocation: AllocationV1 = serde_json::from_str(&payload)?;
            anyhow::ensure!(
                allocation.attempt_id == attempt_id
                    && allocation.method_version == 1
                    && !allocation.billable,
                "allocation projection differs from its key or method"
            );
            position = Some(attempt_id);
            items.push(allocation);
        }
        let read_receipt = self.record_human_read_checked(
            &tx,
            current.principal(),
            Some(&current),
            "usage",
            items.len(),
            transient,
        )?;
        let page = HumanUsagePageV1 {
            version: 1,
            context_id: frame.context_id.clone(),
            authorization_version: frame.authorization_version.clone(),
            evaluated_at: crate::broker_registry::format_time(chrono::Utc::now()),
            source: "tenant_terminal_allocations".into(),
            method_version: 1,
            billable: false,
            items,
            has_more,
            next_cursor: has_more.then(|| "0".repeat(HUMAN_CURSOR_HEX_BYTES)),
            read_receipt,
        };
        let value = serde_json::to_value(&page)?;
        anyhow::ensure!(
            self.redactor.union(transient).value(&value) == value,
            "human usage page requires redaction"
        );
        anyhow::ensure!(
            serde_json::to_vec(&page)?.len() <= HUMAN_MAX_PAGE_BYTES,
            "human usage page exceeds its byte bound"
        );
        self.commit_checked(tx)?;
        Ok(HumanUsageStorePageV1 {
            page,
            after_attempt_id: position,
        })
    }
}
