//! Private reopening capabilities for maintenance and owner startup. Neither
//! wrapper publishes a business Store before its actual incarnation is retained.
use super::review_authority::{self as authority, AuthorityReceiptV1, ReviewAuthorityFailure};
use super::*;
use crate::human_authority::{
    HumanSourceKindV1, HumanSourcesWriteEpoch, OwnedSourceToken,
    maintenance::HumanMaintenanceSourceToken,
};

fn require(value: bool) -> anyhow::Result<()> {
    if value {
        Ok(())
    } else {
        Err(ReviewAuthorityFailure::Mismatch.into())
    }
}

enum OpeningAuthority<'a> {
    Maintenance(&'a HumanMaintenanceSourceToken),
    Startup(&'a OwnedSourceToken),
}
impl OpeningAuthority<'_> {
    fn check(&self) -> anyhow::Result<()> {
        match self {
            Self::Maintenance(token) => token.check().map_err(Into::into),
            Self::Startup(source) => source.revalidate_authority().map_err(Into::into),
        }
    }

    fn physical(
        &self,
        backing: &StoreStorage,
        db: &Connection,
        tenant: &str,
    ) -> anyhow::Result<()> {
        self.check()?;
        storage::verify_connection_main(backing, db)?;
        let (path, identity) = backing.identity_key();
        let (kind, required_format, expected_tenant, binding) = match self {
            Self::Maintenance(token) => {
                let d = token.descriptor();
                (
                    d.kind,
                    d.required_format,
                    d.tenant_id.as_deref(),
                    &d.storage_binding,
                )
            }
            Self::Startup(source) => {
                let d = source.descriptor();
                (
                    d.kind,
                    d.required_format,
                    d.tenant_id.as_deref(),
                    &d.storage_binding,
                )
            }
        };
        // A foreign argument is an admission refusal, never proof that the
        // correctly retained owner's durable source has become corrupt.
        if kind != HumanSourceKindV1::TenantStore
            || !matches!(required_format, 14 | 15)
            || expected_tenant != Some(tenant)
            || path != binding.path()
            || identity.physical_identity() != (binding.source_device, binding.source_inode)
        {
            return Err(ReviewAuthorityFailure::Required.into());
        }
        self.check()
    }

    fn verify(&self, backing: &StoreStorage, db: &Connection, tenant: &str) -> anyhow::Result<()> {
        self.physical(backing, db, tenant)?;
        let format: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        require(match self {
            Self::Maintenance(token) => match token.descriptor().required_format {
                14 => matches!(format, 13 | 14),
                15 => matches!(format, 14 | 15),
                _ => false,
            },
            Self::Startup(source) => {
                matches!(format, 14 | 15)
                    && u32::try_from(format).ok() == Some(source.descriptor().required_format)
            }
        })?;
        validate_schema(db, false)?;
        Store::validate_metadata(db, tenant)?;
        control::validate(db)?;
        require(Store::check_integrity(db)?.valid)?;
        let namespace = authority::namespace(db)?;
        match self {
            Self::Maintenance(token) => {
                let d = token.descriptor();
                require(namespace == d.durable_namespace_id)?;
                if matches!(format, 14 | 15) {
                    let snapshot = authority::replay(db)?;
                    require(
                        (d.required_format == 15
                            || snapshot.anchor.migration_id == token.migration_id())
                            && snapshot.anchor.installation_id == d.installation_id
                            && snapshot.anchor.source_id == d.source_id
                            && snapshot.anchor.kind == d.kind
                            && snapshot.anchor.tenant_id == tenant
                            && snapshot.anchor.durable_namespace_id == d.durable_namespace_id,
                    )?;
                    if d.required_format == 15 {
                        egress::validate_maintenance_identity(db, token)?;
                    }
                }
            }
            Self::Startup(source) => {
                let d = source.descriptor();
                let snapshot = authority::replay(db)?;
                require(
                    namespace == d.durable_namespace_id
                        && snapshot.anchor.installation_id == d.installation_id
                        && snapshot.anchor.source_id == source.source_id()
                        && snapshot.anchor.kind == d.kind
                        && snapshot.anchor.tenant_id == tenant
                        && snapshot.anchor.durable_namespace_id == d.durable_namespace_id
                        && snapshot.anchor_receipt.seq == d.authority_marker_receipt.seq
                        && snapshot.anchor_receipt.hash == d.authority_marker_receipt.hash,
                )?;
                // An existing incarnation may belong to a previous process.
                // Only retain(), under WRITE, requires this owner's incarnation.
            }
        }
        self.physical(backing, db, tenant)
    }
}

fn open_review(
    path: &Path,
    tenant: &str,
    redactor: Redactor,
    max_events: u64,
    authority: OpeningAuthority<'_>,
) -> anyhow::Result<Store> {
    validate_tenant_id(tenant)?;
    anyhow::ensure!(max_events <= i64::MAX as u64, "invalid audit capacity");
    authority.check()?;
    let (backing, _initialization) = StoreStorage::open_existing(path)?;
    // Inspect the exact real source read-only before the first SQLite open
    // capable of changing journal state. There is no creation/adoption path.
    let probe = Connection::open_with_flags(
        backing.path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    probe.busy_timeout(std::time::Duration::ZERO)?;
    probe.execute_batch("PRAGMA trusted_schema=OFF;")?;
    {
        let tx = Transaction::new_unchecked(&probe, rusqlite::TransactionBehavior::Deferred)?;
        authority.verify(&backing, &tx, tenant)?;
        tx.commit()?;
    }
    authority.physical(&backing, &probe, tenant)?;
    probe.close().map_err(|(_, error)| error)?;
    authority.check()?;
    let mut db = Connection::open_with_flags(
        backing.path(),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    db.busy_timeout(std::time::Duration::ZERO)?;
    db.execute_batch("PRAGMA trusted_schema=OFF;")?;
    authority.verify(&backing, &db, tenant)?;
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;
        PRAGMA wal_autocheckpoint=256; PRAGMA journal_size_limit=16777216;")?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    authority.verify(&backing, &tx, tenant)?;
    let validated_data_version = tx.pragma_query_value(None, "data_version", |row| row.get(0))?;
    let initial_head = audit_head(&tx)?;
    let namespace = Some(authority::namespace(&tx)?.to_string());
    let mut capacity = capacity::Cache::default();
    let prefix = capacity.prepare(&tx)?;
    let positive_bindings = metadata::PositiveBindings::load(&tx)?;
    authority.check()?;
    tx.commit()?;
    authority.verify(&backing, &db, tenant)?;
    capacity.publish(prefix);
    Ok(Store::from_prepared_connection(
        backing,
        db,
        tenant,
        redactor,
        max_events,
        PreparedStoreState {
            initial_valid: true,
            fresh_origin: false,
            validated_data_version,
            initial_head,
            namespace,
            capacity,
            positive_bindings,
        },
    ))
}

pub(crate) struct ReviewStoreMaintenance {
    store: Store,
    token: HumanMaintenanceSourceToken,
}

/// The transaction cannot escape this borrowed writer. Root nests these
/// callbacks across the entire union before explicitly requesting any backup.
pub(crate) struct ReviewStoreMaintenanceWriter<'a, 'db> {
    store: &'a Store,
    token: &'a HumanMaintenanceSourceToken,
    db: &'db Connection,
    tx: Option<Transaction<'db>>,
    lease: MutexGuard<'a, SupervisionLease>,
    failed: bool,
}

impl ReviewStoreMaintenance {
    pub(crate) fn with_review_maintenance_writer<T>(
        &self,
        token: &HumanMaintenanceSourceToken,
        work: impl FnOnce(&mut ReviewStoreMaintenanceWriter<'_, '_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        token.check()?;
        if !self.token.same_guard_and_source(token) {
            return Err(ReviewAuthorityFailure::Required.into());
        }
        let db = self
            .store
            .db
            .try_lock()
            .map_err(|_| review_access::ReviewAccessFailure::Busy)?;
        let lease = self
            .store
            .supervision_lease
            .state
            .try_lock()
            .map_err(|_| review_access::ReviewAccessFailure::Busy)?;
        let authority = OpeningAuthority::Maintenance(token);
        authority.physical(&self.store.backing, &db, self.store.tenant_id())?;
        db.busy_timeout(std::time::Duration::ZERO)?;
        let tx = Transaction::new_unchecked(&db, rusqlite::TransactionBehavior::Immediate)?;
        authority.verify(&self.store.backing, &tx, self.store.tenant_id())?;
        lease.verify(&self.store.backing, &tx)?;
        let mut writer = ReviewStoreMaintenanceWriter {
            store: &self.store,
            token,
            db: &db,
            tx: Some(tx),
            lease,
            failed: false,
        };
        work(&mut writer)
    }
}

impl<'a, 'db> ReviewStoreMaintenanceWriter<'a, 'db> {
    pub(crate) fn prepare_egress_backup<'t>(
        &self,
        token: &'t HumanMaintenanceSourceToken,
        path: &Path,
    ) -> anyhow::Result<egress::VerifiedEgressSourceBackup<'t>> {
        self.same_token(token)?;
        egress::prepare_backup(self.store, self.transaction()?, token, path)
    }

    pub(crate) fn resume_egress_backup<'t>(
        &self,
        token: &'t HumanMaintenanceSourceToken,
        path: &Path,
    ) -> anyhow::Result<egress::VerifiedEgressSourceBackup<'t>> {
        self.same_token(token)?;
        egress::resume_backup(self.store, self.transaction()?, token, path)
    }

    /// Pending until explicit commit and source reopen/replay. A real backup
    /// proof and stopped-consumer maintenance token are mandatory arguments.
    pub(crate) fn install_egress_format15(
        &mut self,
        token: &HumanMaintenanceSourceToken,
        backup: &egress::VerifiedEgressSourceBackup<'_>,
        limits: &egress::EgressStoreLimitsV1,
    ) -> anyhow::Result<AuthorityReceiptV1> {
        self.same_token(token)?;
        let result =
            egress::install_format15(self.store, self.transaction()?, token, backup, limits);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn transaction(&self) -> anyhow::Result<&Transaction<'db>> {
        if self.failed {
            return Err(ReviewAuthorityFailure::Required.into());
        }
        let tx = self.tx.as_ref().ok_or(ReviewAuthorityFailure::Required)?;
        require(tx.transaction_state(Some("main"))? == rusqlite::TransactionState::Write)?;
        OpeningAuthority::Maintenance(self.token).verify(
            &self.store.backing,
            tx,
            self.store.tenant_id(),
        )?;
        self.lease.verify(&self.store.backing, tx)?;
        Ok(tx)
    }

    fn same_token(&self, token: &HumanMaintenanceSourceToken) -> anyhow::Result<()> {
        token.check()?;
        if !self.token.same_guard_and_source(token) {
            return Err(ReviewAuthorityFailure::Required.into());
        }
        Ok(())
    }

    pub(crate) fn inspect(&self) -> anyhow::Result<ReviewSourceMigrationState> {
        self.store
            .inspect_review_source_migration(self.transaction()?, self.token)
    }

    pub(crate) fn durable_wall_floor(&self) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
        durable_wall_floor(self.transaction()?)
    }

    pub(crate) fn prepare_backup<'t>(
        &self,
        token: &'t HumanMaintenanceSourceToken,
        path: &Path,
    ) -> anyhow::Result<VerifiedReviewSourceBackup<'t>> {
        self.same_token(token)?;
        self.store
            .prepare_review_source_backup(self.transaction()?, token, path)
    }

    pub(crate) fn resume_backup<'t>(
        &self,
        token: &'t HumanMaintenanceSourceToken,
        path: &Path,
    ) -> anyhow::Result<VerifiedReviewSourceBackup<'t>> {
        self.same_token(token)?;
        self.store
            .resume_review_source_backup(self.transaction()?, token, path)
    }

    /// The receipt is pending until commit() and a later source re-open/check.
    pub(crate) fn install_format14(
        &mut self,
        token: &HumanMaintenanceSourceToken,
        backup: &VerifiedReviewSourceBackup<'_>,
    ) -> anyhow::Result<AuthorityReceiptV1> {
        self.same_token(token)?;
        let result = self.store.install_review_source_format14_in_transaction(
            self.transaction()?,
            token,
            backup,
        );
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    pub(crate) fn commit(&mut self) -> anyhow::Result<()> {
        let tx = self.transaction()?;
        let head = audit_head(tx)?;
        let mut capacity = self
            .store
            .supervision_lease
            .capacity
            .try_lock()
            .map_err(|_| review_access::ReviewAccessFailure::Busy)?;
        let prefix = capacity.prepare(tx)?;
        self.token.check()?;
        self.tx
            .take()
            .ok_or(ReviewAuthorityFailure::Required)?
            .commit()?;
        // COMMIT is known. Preserve this witness before any fallible owner,
        // physical or replay check; no later failure claims rollback.
        self.lease.floor = head;
        capacity.publish(prefix);
        OpeningAuthority::Maintenance(self.token).verify(
            &self.store.backing,
            self.db,
            self.store.tenant_id(),
        )?;
        self.lease.verify(&self.store.backing, self.db)?;
        self.token.check().map_err(Into::into)
    }
}

pub(crate) struct ReviewStoreStartup {
    store: Store,
    source: OwnedSourceToken,
}

fn durable_wall_floor(db: &Connection) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
    let floor: String = db.query_row(
        "SELECT max_received_at FROM audit_time_floors ORDER BY event_seq DESC LIMIT 1",
        [],
        |row| row.get(0),
    )?;
    Ok(chrono::DateTime::parse_from_rfc3339(&floor)?.with_timezone(&chrono::Utc))
}

fn require_current_incarnation(db: &Connection, source: &OwnedSourceToken) -> anyhow::Result<()> {
    authority::replay(db)?.require_current_incarnation_for_startup(source)
}

impl ReviewStoreStartup {
    fn same_source(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
    ) -> anyhow::Result<()> {
        epoch.validate_source(source)?;
        if !self.source.same_owner_and_source(source) {
            return Err(ReviewAuthorityFailure::Required.into());
        }
        Ok(())
    }

    pub(crate) fn durable_wall_floor(&self) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
        let db = self
            .store
            .db
            .try_lock()
            .map_err(|_| review_access::ReviewAccessFailure::Busy)?;
        let mut lease = self
            .store
            .supervision_lease
            .state
            .try_lock()
            .map_err(|_| review_access::ReviewAccessFailure::Busy)?;
        let tx = Transaction::new_unchecked(&db, rusqlite::TransactionBehavior::Deferred)?;
        OpeningAuthority::Startup(&self.source).verify(
            &self.store.backing,
            &tx,
            self.store.tenant_id(),
        )?;
        lease.verify(&self.store.backing, &tx)?;
        let floor = durable_wall_floor(&tx)?;
        let head = audit_head(&tx)?;
        self.source.revalidate_authority()?;
        tx.commit()?;
        // The read COMMIT is known. Retain its verified snapshot before any
        // postcommit refusal, without replacing a newer existing witness.
        if head.0 > lease.floor.0 {
            lease.floor = head;
        }
        self.source.revalidate_authority()?;
        Ok(floor)
    }

    /// Descriptive CAS input from the actual anchor-checked source. This does
    /// not install its old incarnation or confer any mutation authority.
    pub(crate) fn current_incarnation_revision(
        &self,
        epoch: &HumanSourcesWriteEpoch,
    ) -> anyhow::Result<u64> {
        epoch.validate_source(&self.source)?;
        let db = self
            .store
            .db
            .try_lock()
            .map_err(|_| review_access::ReviewAccessFailure::Busy)?;
        let mut lease = self
            .store
            .supervision_lease
            .state
            .try_lock()
            .map_err(|_| review_access::ReviewAccessFailure::Busy)?;
        epoch.validate_source(&self.source)?;
        let tx = Transaction::new_unchecked(&db, rusqlite::TransactionBehavior::Deferred)?;
        epoch.validate_source(&self.source)?;
        OpeningAuthority::Startup(&self.source).verify(
            &self.store.backing,
            &tx,
            self.store.tenant_id(),
        )?;
        lease.verify(&self.store.backing, &tx)?;
        let revision = authority::replay(&tx)?.incarnation_revision();
        let head = audit_head(&tx)?;
        epoch.validate_source(&self.source)?;
        tx.commit()?;
        // Preserve the head of that same verified read snapshot even when the
        // epoch's next check refuses to publish the descriptive result.
        if head.0 > lease.floor.0 {
            lease.floor = head;
        }
        epoch.validate_source(&self.source)?;
        Ok(revision)
    }

    pub(crate) fn append_incarnation(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
        operation_id: uuid::Uuid,
        expected_revision: u64,
    ) -> anyhow::Result<AuthorityReceiptV1> {
        #[cfg(test)]
        {
            self.append_incarnation_inner(epoch, source, operation_id, expected_revision, || {})
        }
        #[cfg(not(test))]
        {
            self.append_incarnation_inner(epoch, source, operation_id, expected_revision)
        }
    }

    // The private callback is used only by the child regression after the real
    // COMMIT/floor publication. It cannot intercept or replace any SQLite work.
    fn append_incarnation_inner(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
        operation_id: uuid::Uuid,
        expected_revision: u64,
        #[cfg(test)] after_commit: impl FnOnce(),
    ) -> anyhow::Result<AuthorityReceiptV1> {
        self.same_source(epoch, source)?;
        let db = self
            .store
            .db
            .try_lock()
            .map_err(|_| review_access::ReviewAccessFailure::Busy)?;
        let mut lease = self
            .store
            .supervision_lease
            .state
            .try_lock()
            .map_err(|_| review_access::ReviewAccessFailure::Busy)?;
        db.busy_timeout(std::time::Duration::ZERO)?;
        self.same_source(epoch, source)?;
        let tx = Transaction::new_unchecked(&db, rusqlite::TransactionBehavior::Immediate)?;
        OpeningAuthority::Startup(source).verify(
            &self.store.backing,
            &tx,
            self.store.tenant_id(),
        )?;
        lease.verify(&self.store.backing, &tx)?;
        let receipt = authority::append_incarnation_in_transaction(
            &self.store,
            &tx,
            epoch,
            source,
            operation_id,
            expected_revision,
        )?;
        OpeningAuthority::Startup(source).verify(
            &self.store.backing,
            &tx,
            self.store.tenant_id(),
        )?;
        let head = audit_head(&tx)?;
        // append_incarnation uses the actual Store append/capacity APIs, so
        // acquire their cache only after that append has released it.
        let mut capacity = self
            .store
            .supervision_lease
            .capacity
            .try_lock()
            .map_err(|_| review_access::ReviewAccessFailure::Busy)?;
        let prefix = capacity.prepare(&tx)?;
        self.same_source(epoch, source)?;
        tx.commit()?;
        lease.floor = head;
        capacity.publish(prefix);
        #[cfg(test)]
        after_commit();
        self.same_source(epoch, source)?;
        // A fresh, real read transaction checks this owner's incarnation after
        // the known commit. No synthetic Transaction/DTO installs authority.
        let tx = Transaction::new_unchecked(&db, rusqlite::TransactionBehavior::Deferred)?;
        OpeningAuthority::Startup(source).verify(
            &self.store.backing,
            &tx,
            self.store.tenant_id(),
        )?;
        lease.verify(&self.store.backing, &tx)?;
        require_current_incarnation(&tx, source)?;
        authority::RetainedReviewAuthority::retain_in_transaction(&self.store, &tx, epoch, source)?;
        workload::capture_control_time(&tx)?;
        self.same_source(epoch, source)?;
        tx.commit()?;
        self.same_source(epoch, source)?;
        Ok(receipt)
    }

    /// Root keeps this result private until every source in the same actual
    /// closure has been retained. This is not Ready, a route or a launch grant.
    pub(crate) fn retain(
        mut self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
    ) -> anyhow::Result<Store> {
        self.same_source(epoch, source)?;
        let retained = {
            let db = self
                .store
                .db
                .try_lock()
                .map_err(|_| review_access::ReviewAccessFailure::Busy)?;
            let mut lease = self
                .store
                .supervision_lease
                .state
                .try_lock()
                .map_err(|_| review_access::ReviewAccessFailure::Busy)?;
            let tx = Transaction::new_unchecked(&db, rusqlite::TransactionBehavior::Deferred)?;
            OpeningAuthority::Startup(source).verify(
                &self.store.backing,
                &tx,
                self.store.tenant_id(),
            )?;
            lease.verify(&self.store.backing, &tx)?;
            require_current_incarnation(&tx, source)?;
            let retained = authority::RetainedReviewAuthority::retain_in_transaction(
                &self.store,
                &tx,
                epoch,
                source,
            )?;
            workload::capture_control_time(&tx)?;
            let head = audit_head(&tx)?;
            self.same_source(epoch, source)?;
            tx.commit()?;
            lease.floor = head;
            self.same_source(epoch, source)?;
            OpeningAuthority::Startup(source).physical(
                &self.store.backing,
                &db,
                self.store.tenant_id(),
            )?;
            retained
        };
        self.store.review_authority = Some(retained);
        self.same_source(epoch, source)?;
        Ok(self.store)
    }
}

impl Store {
    pub(crate) fn open_review_maintenance_for_tenant(
        path: &Path,
        tenant_id: &str,
        redactor: Redactor,
        max_events: u64,
        token: &HumanMaintenanceSourceToken,
    ) -> anyhow::Result<ReviewStoreMaintenance> {
        let store = open_review(
            path,
            tenant_id,
            redactor,
            max_events,
            OpeningAuthority::Maintenance(token),
        )?;
        token.check()?;
        Ok(ReviewStoreMaintenance {
            store,
            token: token.clone(),
        })
    }

    pub(crate) fn open_review_startup_for_tenant(
        path: &Path,
        tenant_id: &str,
        redactor: Redactor,
        max_events: u64,
        source: &OwnedSourceToken,
    ) -> anyhow::Result<ReviewStoreStartup> {
        let store = open_review(
            path,
            tenant_id,
            redactor,
            max_events,
            OpeningAuthority::Startup(source),
        )?;
        source.revalidate_authority()?;
        Ok(ReviewStoreStartup {
            store,
            source: source.clone(),
        })
    }
}

#[cfg(test)]
mod tests;
