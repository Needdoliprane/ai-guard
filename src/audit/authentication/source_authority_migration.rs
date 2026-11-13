//! Offline4→5 and guarded startup. A pending receipt is never a committed
//! migration fact; the caller keeps the union's writers until each known commit.
use super::durable::{DurableAuthenticationAudit, LiveState};
use super::source_authority::{
    self as authority, AuthenticationSourceFailure, SourceAnchor, SourceIncarnation,
};
use super::*;
use crate::{
    audit::storage::{StoreStorage, verify_connection_main},
    control_fs::Directory,
    human_authority::{
        HumanSourceKindV1, HumanSourcesWriteEpoch, OwnedSourceToken,
        maintenance::HumanMaintenanceSourceToken,
    },
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior, params};
use std::time::{Duration, Instant};

fn replay(
    audit: &DurableAuthenticationAudit,
    db: &Connection,
    floor: Option<&AuthenticationReceiptV1>,
) -> Result<format::Replay> {
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    match version {
        format::TENANT_CAPACITY_VERSION => format::replay(
            db,
            audit.installation_id,
            &audit.policy,
            &audit.redactor,
            floor,
        ),
        format::HUMAN_AUTHORITY_VERSION => format::replay_review(
            db,
            audit.installation_id,
            &audit.policy,
            &audit.redactor,
            floor,
        ),
        _ => Err(AuthenticationAuditFailureV1::Format.into()),
    }
}
fn physical(
    backing: &StoreStorage,
    db: &Connection,
    installation: Uuid,
    token: &HumanMaintenanceSourceToken,
) -> Result<()> {
    token.check()?;
    verify_connection_main(backing, db)?;
    let d = token.descriptor();
    let (path, identity) = backing.identity_key();
    authority::exact(
        d.kind == HumanSourceKindV1::AuthenticationLedger
            && d.tenant_id.is_none()
            && d.required_format == 5
            && d.installation_id == installation
            && path == d.storage_binding.path()
            && identity.physical_identity()
                == (
                    d.storage_binding.source_device,
                    d.storage_binding.source_inode,
                ),
    )?;
    token.check().map_err(Into::into)
}
fn match_maintenance(checked: &format::Replay, token: &HumanMaintenanceSourceToken) -> Result<()> {
    let d = token.descriptor();
    authority::exact(checked.verified.namespace_id == d.durable_namespace_id)?;
    if let Some(snapshot) = &checked.authority {
        authority::exact(
            snapshot.anchor.migration_id == token.migration_id()
                && snapshot.anchor.source_id == d.source_id
                && snapshot.anchor.installation_id == d.installation_id
                && snapshot.anchor.durable_namespace_id == d.durable_namespace_id,
        )?;
    }
    Ok(())
}
/// The backup lives outside the source parent so initialization guards never
/// recurse, and the only retained source inode cannot become its own backup.
fn distinct_backup_parent(audit: &DurableAuthenticationAudit, path: &Path) -> Result<Directory> {
    let parent = Directory::open_existing(path.parent().context("backup parent required")?)?;
    let source_parent = Directory::open_existing(
        audit
            .backing
            .path()
            .parent()
            .context("source parent required")?,
    )?;
    authority::exact(parent.physical_identity()? != source_parent.physical_identity()?)?;
    Ok(parent)
}
fn distinct_backup_storage(
    audit: &DurableAuthenticationAudit,
    backing: &StoreStorage,
) -> Result<()> {
    distinct_backup_parent(audit, backing.path())?;
    authority::exact(backing.identity_key().1 != audit.backing.identity_key().1)
}
fn writer(db: &Connection) -> Result<()> {
    authority::exact(db.transaction_state(Some("main"))? == rusqlite::TransactionState::Write)
}
fn modern_connection(backing: &StoreStorage, readonly: bool, busy: Duration) -> Result<Connection> {
    let db = Connection::open_with_flags(
        backing.path(),
        (if readonly {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        }) | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    db.busy_timeout(busy)?;
    verify_connection_main(backing, &db)?;
    db.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON;")?;
    Ok(db)
}
fn quick(db: &Connection) -> Result<()> {
    let checked: String = db.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    ensure!(checked == "ok", AuthenticationAuditFailureV1::Integrity);
    Ok(())
}
/// Token validation precedes the very first mutable SQLite opener. No incarnation
/// is required by this probe: startup must first write it under a WriteEpoch.
fn open_review(
    path: &Path,
    installation: Uuid,
    policy: AuthenticationAuditPolicyV1,
    redactor: Redactor,
    check: impl Fn(&StoreStorage, &Connection) -> Result<()>,
    modern_only: bool,
) -> Result<DurableAuthenticationAudit> {
    format::validate_policy(installation, &policy, &redactor)?;
    let (backing, _initialization) = StoreStorage::open_existing(path)?;
    let mut probe = modern_connection(&backing, true, Duration::ZERO)?;
    check(&backing, &probe)?;
    let probe_tx = probe.transaction_with_behavior(TransactionBehavior::Deferred)?;
    check(&backing, &probe_tx)?;
    let preflight = if modern_only {
        format::replay_review(&probe_tx, installation, &policy, &redactor, None)?
    } else {
        let v: i64 = probe_tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        match v {
            format::TENANT_CAPACITY_VERSION => {
                format::replay(&probe_tx, installation, &policy, &redactor, None)?
            }
            format::HUMAN_AUTHORITY_VERSION => {
                format::replay_review(&probe_tx, installation, &policy, &redactor, None)?
            }
            _ => return Err(AuthenticationAuditFailureV1::Format.into()),
        }
    };
    quick(&probe_tx)?;
    check(&backing, &probe_tx)?;
    probe_tx.commit()?;
    check(&backing, &probe)?;
    probe.close().map_err(|(_, e)| e)?;
    let db = modern_connection(&backing, false, Duration::from_millis(100))?;
    check(&backing, &db)?;
    let recheck_tx = Transaction::new_unchecked(&db, TransactionBehavior::Deferred)?;
    check(&backing, &recheck_tx)?;
    let checked = if preflight.version == format::HUMAN_AUTHORITY_VERSION {
        format::replay_review(
            &recheck_tx,
            installation,
            &policy,
            &redactor,
            Some(&preflight.verified.head),
        )?
    } else {
        format::replay(
            &recheck_tx,
            installation,
            &policy,
            &redactor,
            Some(&preflight.verified.head),
        )?
    };
    check(&backing, &recheck_tx)?;
    recheck_tx.commit()?;
    check(&backing, &db)?;
    // Existing supported files are WAL already. This is verified only after the
    // read-only preflight and another exact physical check, without adopting a file.
    let mode: String = db.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
    ensure!(mode == "wal", AuthenticationAuditFailureV1::Format);
    durable::durability(&db)?;
    check(&backing, &db)?;
    Ok(DurableAuthenticationAudit::from_review_connection(
        backing,
        db,
        installation,
        policy,
        redactor,
        checked.verified,
    ))
}

pub(crate) struct AuthenticationReviewMaintenance {
    audit: DurableAuthenticationAudit,
    token: HumanMaintenanceSourceToken,
}
pub(crate) struct AuthenticationMaintenanceWriter<'a, 'db> {
    audit: &'a DurableAuthenticationAudit,
    token: &'a HumanMaintenanceSourceToken,
    db: &'db Connection,
    tx: Option<Transaction<'db>>,
    floor: &'db mut AuthenticationReceiptV1,
    failed: bool,
}
#[derive(Clone)]
pub(crate) enum AuthenticationSourceMigrationState {
    Ordinary4 { prefix: AuthenticationReceiptV1 },
    Converted5 { anchor: AuthenticationReceiptV1 },
}
impl AuthenticationSourceMigrationState {
    pub(crate) fn anchor(&self) -> Option<AuthenticationReceiptV1> {
        match self {
            Self::Converted5 { anchor } => Some(anchor.clone()),
            Self::Ordinary4 { .. } => None,
        }
    }
}
struct VerifiedBackup {
    source: (std::path::PathBuf, crate::control_fs::PrivateEntryIdentity),
    expected: AuthenticationVerificationV1,
    plan: AuthenticationCapacityPlanV1,
    backing: StoreStorage,
}
/// The backup proof borrows the actual retained maintenance token. Neither JSON
/// nor a claimed receipt can create this value.
pub(crate) struct VerifiedAuthenticationSourceBackup<'t> {
    token: &'t HumanMaintenanceSourceToken,
    backup: VerifiedBackup,
}
impl VerifiedAuthenticationSourceBackup<'_> {
    pub(crate) fn migration_id(&self) -> Uuid {
        self.token.migration_id()
    }
    pub(crate) fn backup_binding(
        &self,
    ) -> (std::path::PathBuf, crate::control_fs::PrivateEntryIdentity) {
        self.backup.backing.identity_key()
    }
    pub(crate) fn prefix(&self) -> &AuthenticationReceiptV1 {
        &self.backup.expected.head
    }
    pub(crate) fn verification(&self) -> &AuthenticationVerificationV1 {
        &self.backup.expected
    }
}
impl VerifiedBackup {
    fn verify(&self, audit: &DurableAuthenticationAudit) -> Result<()> {
        authority::exact(self.source == audit.backing.identity_key())?;
        distinct_backup_storage(audit, &self.backing)?;
        let db = modern_connection(&self.backing, true, Duration::ZERO)?;
        let got = format::replay(
            &db,
            audit.installation_id,
            &audit.policy,
            &audit.redactor,
            Some(&self.expected.head),
        )?;
        authority::exact(
            got.version == format::TENANT_CAPACITY_VERSION
                && got.verified == self.expected
                && got.budget.plan.as_ref() == Some(&self.plan),
        )?;
        quick(&db)?;
        verify_connection_main(&self.backing, &db)?;
        db.close().map_err(|(_, e)| e)?;
        self.backing.verify_names()
    }
}
impl AuthenticationReviewMaintenance {
    pub(crate) fn with_review_maintenance_writer<T>(
        &self,
        token: &HumanMaintenanceSourceToken,
        work: impl FnOnce(&mut AuthenticationMaintenanceWriter<'_, '_>) -> Result<T>,
    ) -> Result<T> {
        token.check()?;
        ensure!(
            self.token.same_guard_and_source(token),
            AuthenticationSourceFailure::Required
        );
        let mut state = self
            .audit
            .state
            .try_lock()
            .map_err(|_| AuthenticationSourceFailure::Busy)?;
        let LiveState { db, floor } = &mut *state;
        physical(&self.audit.backing, db, self.audit.installation_id, token)?;
        db.busy_timeout(Duration::ZERO)?;
        let tx = Transaction::new_unchecked(db, TransactionBehavior::Immediate)?;
        physical(&self.audit.backing, &tx, self.audit.installation_id, token)?;
        let checked = replay(&self.audit, &tx, Some(floor))?;
        match_maintenance(&checked, token)?;
        *floor = checked.verified.head;
        let mut writer = AuthenticationMaintenanceWriter {
            audit: &self.audit,
            token,
            db,
            tx: Some(tx),
            floor,
            failed: false,
        };
        work(&mut writer)
    }
}
impl AuthenticationMaintenanceWriter<'_, '_> {
    fn connection(&self) -> Result<&Connection> {
        let tx = self
            .tx
            .as_ref()
            .ok_or(AuthenticationSourceFailure::Required)?;
        physical(
            &self.audit.backing,
            tx,
            self.audit.installation_id,
            self.token,
        )?;
        writer(tx)?;
        Ok(tx)
    }
    fn checked(&self) -> Result<format::Replay> {
        let got = replay(self.audit, self.connection()?, Some(self.floor))?;
        match_maintenance(&got, self.token)?;
        Ok(got)
    }
    pub(crate) fn inspect(&self) -> Result<AuthenticationSourceMigrationState> {
        let got = self.checked()?;
        Ok(match got.authority {
            Some(s) => AuthenticationSourceMigrationState::Converted5 {
                anchor: s.anchor_receipt,
            },
            None => AuthenticationSourceMigrationState::Ordinary4 {
                prefix: got.verified.head,
            },
        })
    }
    pub(crate) fn verification(&self) -> Result<AuthenticationVerificationV1> {
        Ok(self.checked()?.verified)
    }
    pub(crate) fn capacity_plan(&self) -> Result<Option<AuthenticationCapacityPlanV1>> {
        Ok(self.checked()?.budget.plan)
    }
    pub(crate) fn durable_wall_floor(&self) -> Result<chrono::DateTime<chrono::Utc>> {
        Ok(
            chrono::DateTime::parse_from_rfc3339(&self.checked()?.wall_floor)?
                .with_timezone(&chrono::Utc),
        )
    }
    pub(crate) fn prepare_backup<'t>(
        &self,
        token: &'t HumanMaintenanceSourceToken,
        path: &Path,
    ) -> Result<VerifiedAuthenticationSourceBackup<'t>> {
        ensure!(
            self.token.same_guard_and_source(token),
            AuthenticationSourceFailure::Required
        );
        let expected = self.checked()?;
        ensure!(
            expected.version == format::TENANT_CAPACITY_VERSION,
            AuthenticationAuditFailureV1::Format
        );
        let path = crate::control_fs::absolute_path(path)?;
        let parent = distinct_backup_parent(self.audit, &path)?;
        let (backing, initialization) = StoreStorage::create_new(&path)?;
        distinct_backup_storage(self.audit, &backing)?;
        parent.durable_bootstrap()?;
        let mut destination = modern_connection(&backing, false, Duration::ZERO)?;
        durable::durability(&destination)?;
        let reader = modern_connection(&self.audit.backing, true, Duration::ZERO)?;
        let prefix = format::replay(
            &reader,
            self.audit.installation_id,
            &self.audit.policy,
            &self.audit.redactor,
            Some(&expected.verified.head),
        )?;
        authority::exact(
            prefix.verified == expected.verified && prefix.budget.plan == expected.budget.plan,
        )?;
        {
            let copy = rusqlite::backup::Backup::new(&reader, &mut destination)?;
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                self.connection()?;
                token.check()?;
                backing.verify_names()?;
                match copy.step(256)? {
                    rusqlite::backup::StepResult::Done => break,
                    rusqlite::backup::StepResult::More => {
                        ensure!(Instant::now() < deadline, AuthenticationSourceFailure::Busy)
                    }
                    _ => return Err(AuthenticationSourceFailure::Busy.into()),
                }
            }
        }
        let copied = format::replay(
            &destination,
            self.audit.installation_id,
            &self.audit.policy,
            &self.audit.redactor,
            Some(&expected.verified.head),
        )?;
        authority::exact(
            copied.verified == expected.verified && copied.budget.plan == expected.budget.plan,
        )?;
        quick(&destination)?;
        verify_connection_main(&backing, &destination)?;
        verify_connection_main(&self.audit.backing, &reader)?;
        let final_source = format::replay(
            &reader,
            self.audit.installation_id,
            &self.audit.policy,
            &self.audit.redactor,
            Some(&expected.verified.head),
        )?;
        authority::exact(final_source.verified == expected.verified)?;
        reader.close().map_err(|(_, e)| e)?;
        destination.close().map_err(|(_, e)| e)?;
        backing.sync_closed(&initialization)?;
        let backup = VerifiedBackup {
            source: self.audit.backing.identity_key(),
            expected: expected.verified,
            plan: expected
                .budget
                .plan
                .ok_or(AuthenticationAuditFailureV1::Format)?,
            backing,
        };
        backup.verify(self.audit)?;
        self.connection()?;
        token.check()?;
        Ok(VerifiedAuthenticationSourceBackup { token, backup })
    }
    pub(crate) fn resume_backup<'t>(
        &self,
        token: &'t HumanMaintenanceSourceToken,
        path: &Path,
    ) -> Result<VerifiedAuthenticationSourceBackup<'t>> {
        ensure!(
            self.token.same_guard_and_source(token),
            AuthenticationSourceFailure::Required
        );
        let checked = self.checked()?;
        let path = crate::control_fs::absolute_path(path)?;
        distinct_backup_parent(self.audit, &path)?;
        let (backing, _initialization) = StoreStorage::open_existing(&path)?;
        distinct_backup_storage(self.audit, &backing)?;
        let db = modern_connection(&backing, true, Duration::ZERO)?;
        let restored = format::replay(
            &db,
            self.audit.installation_id,
            &self.audit.policy,
            &self.audit.redactor,
            None,
        )?;
        ensure!(
            restored.version == format::TENANT_CAPACITY_VERSION,
            AuthenticationAuditFailureV1::Format
        );
        let prefix = checked.authority.as_ref().map_or_else(
            || checked.verified.head.clone(),
            |s| AuthenticationReceiptV1 {
                namespace_id: s.anchor.durable_namespace_id,
                sequence: s.anchor.previous_sequence,
                hash: s.anchor.previous_hash.clone(),
            },
        );
        authority::exact(
            restored.verified.head == prefix && restored.budget.plan == checked.budget.plan,
        )?;
        quick(&db)?;
        verify_connection_main(&backing, &db)?;
        db.close().map_err(|(_, e)| e)?;
        let backup = VerifiedBackup {
            source: self.audit.backing.identity_key(),
            expected: restored.verified,
            plan: restored
                .budget
                .plan
                .ok_or(AuthenticationAuditFailureV1::Format)?,
            backing,
        };
        backup.verify(self.audit)?;
        self.connection()?;
        token.check()?;
        Ok(VerifiedAuthenticationSourceBackup { token, backup })
    }
    pub(crate) fn install_format5(
        &mut self,
        backup: &VerifiedAuthenticationSourceBackup<'_>,
    ) -> Result<AuthenticationReceiptV1> {
        let result = self.install(backup);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn install(
        &self,
        backup: &VerifiedAuthenticationSourceBackup<'_>,
    ) -> Result<AuthenticationReceiptV1> {
        ensure!(
            self.token.same_guard_and_source(backup.token),
            AuthenticationSourceFailure::Required
        );
        backup.token.check()?;
        backup.backup.verify(self.audit)?;
        let got = self.checked()?;
        authority::exact(got.budget.plan.as_ref() == Some(&backup.backup.plan))?;
        if let Some(snapshot) = got.authority {
            authority::exact(
                snapshot.anchor.previous_sequence == backup.backup.expected.head.sequence
                    && snapshot.anchor.previous_hash == backup.backup.expected.head.hash,
            )?;
            return Ok(snapshot.anchor_receipt);
        }
        authority::exact(got.verified == backup.backup.expected)?;
        let d = self.token.descriptor();
        let entry = format::Entry::SourceAnchor {
            anchor: SourceAnchor {
                version: 1,
                protocol: authority::PROTOCOL.into(),
                from_format: 4,
                format: 5,
                migration_id: self.token.migration_id(),
                installation_id: d.installation_id,
                source_id: d.source_id,
                durable_namespace_id: d.durable_namespace_id,
                kind: d.kind,
                previous_sequence: got.verified.head.sequence,
                previous_hash: got.verified.head.hash.clone(),
            },
        };
        append_authority(self.audit, self.connection()?, got, entry, true)
    }
    pub(crate) fn commit(&mut self) -> Result<()> {
        ensure!(!self.failed, AuthenticationSourceFailure::Required);
        let checked = self.checked()?;
        self.token.check()?;
        self.tx
            .take()
            .ok_or(AuthenticationSourceFailure::Required)?
            .commit()?;
        // The COMMIT result is known. Nothing fallible may precede this floor.
        *self.floor = checked.verified.head;
        physical(
            &self.audit.backing,
            self.db,
            self.audit.installation_id,
            self.token,
        )?;
        let committed = replay(self.audit, self.db, Some(self.floor))?;
        match_maintenance(&committed, self.token)?;
        self.token.check().map_err(Into::into)
    }
}

fn append_authority(
    audit: &DurableAuthenticationAudit,
    db: &Connection,
    got: format::Replay,
    logical: format::Entry,
    installing: bool,
) -> Result<AuthenticationReceiptV1> {
    writer(db)?;
    if let format::Entry::SourceIncarnation { incarnation } = &logical {
        ensure!(
            !got.incarnation_operations
                .contains(&incarnation.operation_id),
            AuthenticationAuditFailureV1::Conflict
        );
    }
    let entry = format::Entry::Charged {
        charge: budget::CapacityOwner::Unscoped,
        entry: Box::new(logical.clone()),
    };
    let payload = format::checked_json(&entry, &audit.redactor)?;
    got.budget
        .validate_authority(Some(&budget::CapacityOwner::Unscoped), payload.len() as u64)?;
    let records = got
        .verified
        .records
        .checked_add(1)
        .ok_or(AuthenticationAuditFailureV1::Capacity)?;
    let bytes = got
        .verified
        .serialized_bytes
        .checked_add(payload.len() as u64)
        .ok_or(AuthenticationAuditFailureV1::Capacity)?;
    got.check_terminal_capacity(&entry, records, bytes, &audit.policy)?;
    let received = authority::clock(&got.wall_floor)?;
    let hash = format::digest(records, &received, &got.verified.head.hash, &payload);
    let receipt = AuthenticationReceiptV1 {
        namespace_id: got.verified.namespace_id,
        sequence: records,
        hash: hash.clone(),
    };
    let mut projection = got.authority;
    authority::Snapshot::apply(
        &mut projection,
        &logical,
        &receipt,
        &got.verified.head.hash,
        audit.installation_id,
    )?;
    // Capacity/time and all marker shape checks precede any conversion DDL.
    if installing {
        db.execute_batch(authority::SCHEMA.1)?;
    }
    authority::exact(db.execute("INSERT INTO authentication_entries(seq,received_at,prev_hash,hash,payload) VALUES(?1,?2,?3,?4,?5)", params![records as i64, received, got.verified.head.hash, hash, payload])? == 1)?;
    authority::exact(db.execute("UPDATE authentication_quota SET records=?1,bytes=?2 WHERE singleton=1 AND records=?3 AND bytes=?4", params![records as i64, bytes as i64, got.verified.records as i64, got.verified.serialized_bytes as i64])? == 1)?;
    projection
        .ok_or(AuthenticationSourceFailure::Mismatch)?
        .write(db, &received, &audit.redactor)?;
    if installing {
        db.pragma_update(None, "user_version", format::HUMAN_AUTHORITY_VERSION)?;
    }
    let verified = format::replay_review(
        db,
        audit.installation_id,
        &audit.policy,
        &audit.redactor,
        Some(&got.verified.head),
    )?;
    authority::exact(verified.verified.head == receipt)?;
    Ok(receipt)
}

pub(crate) struct AuthenticationReviewStartup {
    audit: DurableAuthenticationAudit,
    source: OwnedSourceToken,
}
impl AuthenticationReviewStartup {
    pub(crate) fn durable_wall_floor(&self) -> Result<chrono::DateTime<chrono::Utc>> {
        Ok(self.floor_and_capacity_plan()?.0)
    }
    pub(crate) fn require_capacity_plan(
        &self,
        expected: &AuthenticationCapacityPlanV1,
    ) -> Result<()> {
        expected.validate()?;
        authority::exact(self.floor_and_capacity_plan()?.1.as_ref() == Some(expected))
    }
    fn floor_and_capacity_plan(
        &self,
    ) -> Result<(
        chrono::DateTime<chrono::Utc>,
        Option<AuthenticationCapacityPlanV1>,
    )> {
        let state = self
            .audit
            .state
            .try_lock()
            .map_err(|_| AuthenticationSourceFailure::Busy)?;
        authority::physical(&self.audit.backing, &state.db, &self.source)?;
        let got = format::replay_review(
            &state.db,
            self.audit.installation_id,
            &self.audit.policy,
            &self.audit.redactor,
            Some(&state.floor),
        )?;
        got.authority
            .as_ref()
            .ok_or(AuthenticationSourceFailure::Required)?
            .match_anchor(&self.source)?;
        Ok((
            chrono::DateTime::parse_from_rfc3339(&got.wall_floor)?.with_timezone(&chrono::Utc),
            got.budget.plan,
        ))
    }
    /// Verified predecessor revision for startup CAS; never an execution capability.
    /// This deliberately accepts a prior owner incarnation only as history.
    pub(crate) fn current_incarnation_revision(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
    ) -> Result<u64> {
        epoch.validate_source(source)?;
        ensure!(
            self.source.same_owner_and_source(source),
            AuthenticationSourceFailure::Required
        );
        let mut state = self
            .audit
            .state
            .try_lock()
            .map_err(|_| AuthenticationSourceFailure::Busy)?;
        let LiveState { db, floor } = &mut *state;
        authority::physical(&self.audit.backing, db, source)?;
        epoch.validate_source(source)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Deferred)?;
        authority::physical(&self.audit.backing, &tx, source)?;
        epoch.validate_source(source)?;
        let got = format::replay_review(
            &tx,
            self.audit.installation_id,
            &self.audit.policy,
            &self.audit.redactor,
            Some(floor),
        )?;
        let snapshot = got
            .authority
            .as_ref()
            .ok_or(AuthenticationSourceFailure::Required)?;
        snapshot.match_anchor(source)?;
        let revision = snapshot
            .incarnation
            .as_ref()
            .map_or(0, |(incarnation, _)| incarnation.revision);
        epoch.validate_source(source)?;
        authority::physical(&self.audit.backing, &tx, source)?;
        tx.commit()?;
        // Retain a known verified head even if a later check refuses publication.
        if got.verified.head.sequence > floor.sequence {
            *floor = got.verified.head;
        }
        epoch.validate_source(source)?;
        authority::physical(&self.audit.backing, db, source)?;
        epoch.validate_source(source)?;
        Ok(revision)
    }
    pub(crate) fn append_incarnation(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
        operation_id: Uuid,
        expected_revision: u64,
    ) -> Result<AuthenticationReceiptV1> {
        epoch.validate_source(source)?;
        ensure!(
            self.source.same_owner_and_source(source),
            AuthenticationSourceFailure::Required
        );
        let mut state = self
            .audit
            .state
            .try_lock()
            .map_err(|_| AuthenticationSourceFailure::Busy)?;
        let LiveState { db, floor } = &mut *state;
        authority::physical(&self.audit.backing, db, source)?;
        epoch.validate_source(source)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        epoch.validate_source(source)?;
        authority::physical(&self.audit.backing, &tx, source)?;
        let got = format::replay_review(
            &tx,
            self.audit.installation_id,
            &self.audit.policy,
            &self.audit.redactor,
            Some(floor),
        )?;
        *floor = got.verified.head.clone();
        let snapshot = got
            .authority
            .as_ref()
            .ok_or(AuthenticationSourceFailure::Required)?;
        snapshot.match_anchor(source)?;
        authority::clock(&got.wall_floor)?;
        let c = source.closure();
        let receipt = if let Some((old, receipt)) = snapshot
            .incarnation
            .as_ref()
            .filter(|(old, _)| old.operation_id == operation_id)
        {
            snapshot.match_current(source)?;
            authority::exact(expected_revision.checked_add(1) == Some(old.revision))?;
            receipt.clone()
        } else {
            let revision = snapshot
                .incarnation
                .as_ref()
                .map_or(0, |(old, _)| old.revision);
            authority::exact(revision == expected_revision)?;
            let logical = format::Entry::SourceIncarnation {
                incarnation: SourceIncarnation {
                    version: 1,
                    protocol: authority::PROTOCOL.into(),
                    operation_id,
                    anchor: snapshot.anchor_receipt.clone(),
                    closure_id: c.closure_id,
                    closure_revision: c.closure_revision,
                    closure_digest: c.closure_digest.clone(),
                    control_config_public_digest: c.configuration.public_config_digest.clone(),
                    owner_incarnation: source.incarnation(),
                    revision: revision
                        .checked_add(1)
                        .ok_or(AuthenticationSourceFailure::Mismatch)?,
                    previous_incarnation: snapshot.incarnation.as_ref().map(|(_, r)| r.clone()),
                    previous_sequence: got.verified.head.sequence,
                    previous_hash: got.verified.head.hash.clone(),
                },
            };
            epoch.validate_source(source)?;
            append_authority(&self.audit, &tx, got, logical, false)?
        };
        epoch.validate_source(source)?;
        authority::physical(&self.audit.backing, &tx, source)?;
        tx.commit()?;
        // Even an expired epoch after commit cannot erase this known receipt.
        if receipt.sequence > floor.sequence {
            *floor = receipt.clone();
        }
        epoch.validate_source(source)?;
        authority::physical(&self.audit.backing, db, source)?;
        let checked = format::replay_review(
            db,
            self.audit.installation_id,
            &self.audit.policy,
            &self.audit.redactor,
            Some(floor),
        )?;
        checked
            .authority
            .as_ref()
            .ok_or(AuthenticationSourceFailure::Required)?
            .match_current(source)?;
        authority::clock(&checked.wall_floor)?;
        epoch.validate_source(source)?;
        Ok(receipt)
    }
    pub(crate) fn retain(
        mut self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
    ) -> Result<AuthenticationAudit> {
        epoch.validate_source(source)?;
        ensure!(
            self.source.same_owner_and_source(source),
            AuthenticationSourceFailure::Required
        );
        {
            let mut state = self
                .audit
                .state
                .try_lock()
                .map_err(|_| AuthenticationSourceFailure::Busy)?;
            let LiveState { db, floor } = &mut *state;
            let tx = db.transaction_with_behavior(TransactionBehavior::Deferred)?;
            authority::physical(&self.audit.backing, &tx, source)?;
            let got = format::replay_review(
                &tx,
                self.audit.installation_id,
                &self.audit.policy,
                &self.audit.redactor,
                Some(floor),
            )?;
            got.authority
                .as_ref()
                .ok_or(AuthenticationSourceFailure::Required)?
                .match_current(source)?;
            authority::clock(&got.wall_floor)?;
            epoch.validate_source(source)?;
            tx.commit()?;
            *floor = got.verified.head;
            authority::physical(&self.audit.backing, db, source)?;
            epoch.validate_source(source)?;
        }
        self.audit.review_source = Some(source.clone());
        Ok(AuthenticationAudit { inner: self.audit })
    }
}
impl AuthenticationAudit {
    pub(crate) fn open_review_maintenance(
        path: &Path,
        installation_id: Uuid,
        policy: AuthenticationAuditPolicyV1,
        redactor: Redactor,
        token: &HumanMaintenanceSourceToken,
    ) -> Result<AuthenticationReviewMaintenance> {
        token.check()?;
        let audit = open_review(
            path,
            installation_id,
            policy,
            redactor,
            |backing, db| physical(backing, db, installation_id, token),
            false,
        )?;
        {
            let state = audit
                .state
                .try_lock()
                .map_err(|_| AuthenticationSourceFailure::Busy)?;
            let checked = replay(&audit, &state.db, Some(&state.floor))?;
            match_maintenance(&checked, token)?;
        }
        token.check()?;
        Ok(AuthenticationReviewMaintenance {
            audit,
            token: token.clone(),
        })
    }
    pub(crate) fn open_review_startup(
        path: &Path,
        installation_id: Uuid,
        policy: AuthenticationAuditPolicyV1,
        redactor: Redactor,
        source: &OwnedSourceToken,
    ) -> Result<AuthenticationReviewStartup> {
        source.revalidate_authority()?;
        let audit = open_review(
            path,
            installation_id,
            policy,
            redactor,
            |backing, db| authority::physical(backing, db, source),
            true,
        )?;
        {
            let state = audit
                .state
                .try_lock()
                .map_err(|_| AuthenticationSourceFailure::Busy)?;
            let checked = format::replay_review(
                &state.db,
                installation_id,
                &audit.policy,
                &audit.redactor,
                Some(&state.floor),
            )?;
            authority::exact(
                checked.verified.namespace_id == source.descriptor().durable_namespace_id,
            )?;
            checked
                .authority
                .as_ref()
                .ok_or(AuthenticationSourceFailure::Required)?
                .match_anchor(source)?;
        }
        source.revalidate_authority()?;
        Ok(AuthenticationReviewStartup {
            audit,
            source: source.clone(),
        })
    }
}

#[cfg(all(test, unix))]
mod tests;
