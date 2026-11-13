//! Own-source Business reads, with the complete checked transaction protocol.
//! No neighboring connection, metadata snapshot or recovery participates here.
use super::*;
use crate::identity::*;
mod recent;

/// Existing durable projection markers; coverage metadata, never identity.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct HumanDiscoveryGeneration {
    authorization_revision: u64,
    workload_history_event: Option<i64>,
}
fn discovery_generation(tx: &CheckedTransaction<'_>) -> anyhow::Result<HumanDiscoveryGeneration> {
    Ok(HumanDiscoveryGeneration {
        authorization_revision: identity::authorization_revision(tx)?,
        workload_history_event: tx.query_row(
            "SELECT MAX(event_seq) FROM workload_binding_history",
            [],
            |row| row.get(0),
        )?,
    })
}

impl Store {
    pub(crate) fn owned_redaction_covers(&self, redactor: &Redactor) -> bool {
        self.redactor.covers(redactor)
    }
    #[cfg(test)]
    pub(crate) fn modern_test_hold_mutex(
        &self,
        ready: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) {
        let _held = self.db.lock().unwrap();
        ready.send(()).unwrap();
        release.recv().unwrap();
    }
    #[cfg(test)]
    pub(crate) fn modern_test_abort_events_audit(
        &self,
    ) -> anyhow::Result<HumanEventsAuditAbortGuard<'_>> {
        let db = self.db.try_lock().map_err(|_| anyhow::anyhow!("fixture store busy"))?;
        db.execute_batch("CREATE TEMP TRIGGER fixture_owned_events_audit_abort BEFORE INSERT ON main.events WHEN NEW.kind='control.identity.access' AND json_extract(NEW.payload,'$.data.action')='events' BEGIN SELECT RAISE(ABORT,'synthetic owned events audit insertion failure'); END;")?;
        Ok(HumanEventsAuditAbortGuard { store: Some(self) })
    }
    #[cfg(test)]
    pub(crate) fn modern_test_human_usage(&self) -> Value {
        let capacity = self.supervision_lease.capacity.lock().unwrap();
        json!({"tenant":capacity.tenant,"principals":capacity.principals})
    }
    fn with_modern_read<T>(
        &self,
        access: &HumanSourceAccess<'_>,
        read: impl FnOnce(&CheckedTransaction<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        access.check(StoreOperationClass::Business)?;
        let mut db = self
            .db
            .try_lock()
            .map_err(|_| review_access::ReviewAccessFailure::Busy)?;
        let prior: i64 = db.pragma_query_value(None, "busy_timeout", |row| row.get(0))?;
        let prior = u64::try_from(prior).context("invalid negative SQLite busy timeout")?;
        db.busy_timeout(std::time::Duration::ZERO)?;
        let result = (|| {
            // Unlike begin_checked_scoped, this entrance does not reset zero
            // to100ms. Every source/lease/floor/schema/capacity check is shared.
            let tx = self.begin_checked_with_access(
                &mut db,
                Some(access),
                StoreOperationClass::Business,
            )?;
            let result = read(&tx)?;
            self.commit_checked(tx)?;
            Ok(result)
        })();
        // The exclusive connection guard survives rollback and restoration.
        // A restoration error refuses publication even after a known COMMIT.
        db.busy_timeout(std::time::Duration::from_millis(prior))?;
        let restored: i64 = db.pragma_query_value(None, "busy_timeout", |row| row.get(0))?;
        let restored = u64::try_from(restored).context("invalid negative SQLite busy timeout")?;
        anyhow::ensure!(restored == prior, "human source timeout restoration failed");
        result.map_err(|error:anyhow::Error| {
            let busy=error.chain().any(|cause|matches!(cause.downcast_ref::<rusqlite::Error>(),Some(rusqlite::Error::SqliteFailure(code,_)) if matches!(code.code,rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)));
            if busy {error.context(review_access::ReviewAccessFailure::Busy)}else{error}
        })
    }

    pub(crate) fn modern_authenticate_local_digest(
        &self,
        access: &HumanSourceAccess<'_>,
        digest: &[u8; 32],
    ) -> anyhow::Result<Option<VerifiedLocalCredentialV1>> {
        self.with_modern_read(access, |tx| {
            // Historical workload ownership is a class collision even after
            // revocation or rotation; the actual own source decides presence.
            let workload: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM workload_digests WHERE sha256_digest=?1)",
                [hex::encode(digest)],
                |row| row.get(0),
            )?;
            anyhow::ensure!(!workload, "human credential class is ambiguous");
            self.authenticate_local_digest_checked(tx, digest)
        })
    }

    pub(crate) fn modern_human_profile(
        &self,
        access: &HumanSourceAccess<'_>,
        principal: &PrincipalRefV1,
        transient: &Redactor,
    ) -> anyhow::Result<human::HumanProfileSnapshotV1> {
        self.with_modern_read(access, |tx| {
            self.human_profile_checked(tx, principal, transient)
        })
    }

    pub(crate) fn modern_authorize_human_context(
        &self,
        access: &HumanSourceAccess<'_>,
        principal: &PrincipalRefV1,
        membership_id: &str,
        revision: u64,
        mode: ContextModeV1,
    ) -> anyhow::Result<AuthorizedScopeV1> {
        let key = principal.key()?;
        self.with_modern_read(access, |tx| {
            self.authorize_human_context_checked(tx, principal, &key, membership_id, revision, mode)
        })
    }

    pub(crate) fn modern_human_resources(
        &self,
        access: &HumanSourceAccess<'_>,
        scope: &AuthorizedScopeV1,
        after: &str,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanStorePageV1<HumanResourceItemV1>> {
        human::validate_resource_page(after, limit)?;
        self.with_modern_read(access, |tx| {
            self.human_resources_checked(tx, scope, after, limit, transient)
        })
    }

    /// Adopted metadata list only. Other event traversals remain unadopted
    /// even when the existing legacy selector supports their filters.
    pub(crate) fn modern_human_events(
        &self,
        access: &HumanSourceAccess<'_>,
        scope: &AuthorizedScopeV1,
        filter: &HumanEventFilterV1,
        after: i64,
        upper: Option<i64>,
        limit: usize,
        transient: &Redactor,
    ) -> anyhow::Result<HumanEventPollV1> {
        anyhow::ensure!(
            filter.view == HumanReadViewV1::Metadata
                && filter.event_id.is_none()
                && !filter.export
                && !filter.stream
                && (1..=HUMAN_MAX_PAGE_ITEMS).contains(&limit)
                && after >= 0,
            IdentityFailureV1::InvalidRequest
        );
        self.with_modern_read(access, |tx| {
            self.human_events_checked(tx, scope, filter, after, upper, limit, transient)
        })
    }

    /// Streaming startup-only producer. The consumer charges each fixed-size
    /// candidate before accumulation. Historical positive ownership is kept.
    pub(crate) fn modern_visit_candidates(
        &self,
        access: &HumanSourceAccess<'_>,
        mut visit: impl FnMut([u8; 32], bool) -> anyhow::Result<()>,
    ) -> anyhow::Result<HumanDiscoveryGeneration> {
        self.with_modern_read(access, |tx| {
            let mut statement = tx.prepare("SELECT digest,0 FROM identity_credentials UNION ALL SELECT sha256_digest,1 FROM workload_digests ORDER BY 1,2 LIMIT 65537")?;
            let mut rows = statement.query([])?;
            let mut count = 0usize;
            while let Some(row) = rows.next()? {
                count = count.checked_add(1).ok_or_else(|| anyhow::anyhow!("human discovery capacity exceeded"))?;
                anyhow::ensure!(count <= 65536, "human discovery capacity exceeded");
                // Bound the SQL value before allocating a String/Vec.
                let value = row.get_ref(0)?.as_str()?;
                anyhow::ensure!(
                    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                    "invalid discovery digest"
                );
                let mut digest = [0u8; 32];
                hex::decode_to_slice(value, &mut digest)?;
                visit(digest, row.get::<_, bool>(1)?)?;
            }
            discovery_generation(tx)
        })
    }
    /// Coverage-generation observation only, never a principal or permission.
    pub(crate) fn modern_discovery_generation(
        &self,
        access: &HumanSourceAccess<'_>,
    ) -> anyhow::Result<HumanDiscoveryGeneration> {
        self.with_modern_read(access, discovery_generation)
    }
}

/// Temporary SQLite fixture state. Never present in a production build.
#[cfg(test)]
pub(crate) struct HumanEventsAuditAbortGuard<'a> {
    store: Option<&'a Store>,
}
#[cfg(test)]
impl HumanEventsAuditAbortGuard<'_> {
    pub(crate) fn remove(mut self) -> anyhow::Result<()> {
        let store = self.store.as_ref().ok_or_else(|| anyhow::anyhow!("fixture trigger removed"))?;
        store.db.try_lock().map_err(|_| anyhow::anyhow!("fixture store busy"))?
            .execute_batch("DROP TRIGGER temp.fixture_owned_events_audit_abort;")?;
        self.store = None;
        Ok(())
    }
}
#[cfg(test)]
impl Drop for HumanEventsAuditAbortGuard<'_> {
    fn drop(&mut self) {
        if let Some(store) = self.store.take() {
            // Unwind cleanup never blocks behind a failed asynchronous test.
            // In the normal path remove() above verifies exact removal.
            if let Ok(db) = store.db.try_lock() {
                let _ = db.execute_batch("DROP TRIGGER IF EXISTS temp.fixture_owned_events_audit_abort;");
            }
        }
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod journal_tests;
