use super::{format, *};
use crate::{
    audit::storage::{InitializationGuard, StoreStorage},
    control_fs::Directory,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

struct LiveState {
    db: Connection,
    floor: AuthenticationReceiptV1,
}

pub(super) struct DurableAuthenticationAudit {
    backing: StoreStorage,
    installation_id: Uuid,
    namespace_id: Uuid,
    policy: AuthenticationAuditPolicyV1,
    redactor: Redactor,
    state: Mutex<LiveState>,
    unhealthy: AtomicBool,
}

fn connection(backing: &StoreStorage, read_only: bool) -> Result<Connection> {
    let db = Connection::open_with_flags(
        backing.path(),
        (if read_only {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        }) | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    backing.verify(&db)?;
    db.busy_timeout(Duration::from_secs(5))?;
    db.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON;")?;
    Ok(db)
}

fn durability(db: &Connection) -> Result<()> {
    db.execute_batch(
        "PRAGMA synchronous=FULL; PRAGMA fullfsync=ON; PRAGMA checkpoint_fullfsync=ON;",
    )?;
    let synchronous: i64 = db.pragma_query_value(None, "synchronous", |row| row.get(0))?;
    ensure!(
        synchronous == 2,
        "authentication audit requires full SQLite durability"
    );
    Ok(())
}

fn durable_parent(backing: &StoreStorage) -> Result<()> {
    Directory::open_existing(
        backing
            .path()
            .parent()
            .context("authentication audit parent is missing")?,
    )?
    .durable_bootstrap()
}

impl DurableAuthenticationAudit {
    pub fn init_new(
        path: &Path,
        installation_id: Uuid,
        policy: AuthenticationAuditPolicyV1,
        redactor: Redactor,
    ) -> Result<Self> {
        format::validate_policy(installation_id, &policy, &redactor)?;
        let (backing, guard) = StoreStorage::create_new(path)?;
        durable_parent(&backing)?;
        let namespace_id = Uuid::new_v4();
        let mut db = connection(&backing, false)?;
        durability(&db)?;
        // All metadata, genesis, counters and version headers commit together.
        // Failure retains the partial new file; no later open repairs it.
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        backing.verify(&tx)?;
        format::initialize(&tx, installation_id, namespace_id, &policy, &redactor)?;
        format::replay(&tx, installation_id, &policy, &redactor, None)?;
        tx.commit()?;
        backing.verify(&db)?;
        db.close().map_err(|(_, error)| error)?;
        backing.sync_closed(&guard)?;
        Self::open_prepared(backing, guard, installation_id, policy, redactor)
    }

    pub fn open_existing(
        path: &Path,
        installation_id: Uuid,
        policy: AuthenticationAuditPolicyV1,
        redactor: Redactor,
    ) -> Result<Self> {
        format::validate_policy(installation_id, &policy, &redactor)?;
        let (backing, guard) = StoreStorage::open_existing(path)?;
        Self::open_prepared(backing, guard, installation_id, policy, redactor)
    }

    fn open_prepared(
        backing: StoreStorage,
        _guard: InitializationGuard,
        installation_id: Uuid,
        policy: AuthenticationAuditPolicyV1,
        redactor: Redactor,
    ) -> Result<Self> {
        // A read-only VFS probe cannot recover a foreign hot journal into its
        // main file. Its SQLite descriptor is managed by SQLite's own lock
        // bookkeeping; no raw database FD is opened/closed here.
        let mut probe = connection(&backing, true)?;
        // Inspect the complete closed format and replay BEFORE changing WAL.
        // Business, empty, partial, unknown or damaged files are never adopted.
        let preflight = {
            let tx = probe.transaction_with_behavior(TransactionBehavior::Deferred)?;
            backing.verify(&tx)?;
            let replay = format::replay(&tx, installation_id, &policy, &redactor, None)?;
            let quick: String = tx.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
            ensure!(quick == "ok", AuthenticationAuditFailureV1::Integrity);
            backing.verify(&tx)?;
            tx.commit()?;
            replay.verified
        };
        probe.close().map_err(|(_, error)| error)?;
        let mut db = connection(&backing, false)?;
        // Recheck after the read-only probe and before any journal-mode write.
        {
            let tx = db.transaction_with_behavior(TransactionBehavior::Deferred)?;
            backing.verify(&tx)?;
            format::replay(
                &tx,
                installation_id,
                &policy,
                &redactor,
                Some(&preflight.head),
            )?;
            backing.verify(&tx)?;
            tx.commit()?;
        }
        let mode: String =
            db.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
        ensure!(mode == "wal", "authentication audit WAL mode unavailable");
        durability(&db)?;
        db.execute_batch("PRAGMA wal_autocheckpoint=256; PRAGMA journal_size_limit=16777216;")?;
        // A concurrent legitimate opener may append, but cannot replace this
        // validated prefix/namespace between inspection and the live witness.
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        backing.verify(&tx)?;
        let checked = format::replay(
            &tx,
            installation_id,
            &policy,
            &redactor,
            Some(&preflight.head),
        )?
        .verified;
        tx.commit()?;
        backing.verify(&db)?;
        Ok(Self {
            backing,
            installation_id,
            namespace_id: checked.namespace_id,
            policy,
            redactor,
            state: Mutex::new(LiveState {
                db,
                floor: checked.head,
            }),
            unhealthy: AtomicBool::new(false),
        })
    }

    fn healthy(&self) -> Result<()> {
        ensure!(
            !self.unhealthy.load(Ordering::Acquire),
            AuthenticationAuditFailureV1::Integrity
        );
        Ok(())
    }

    fn inspect(&self, db: &Connection, floor: &AuthenticationReceiptV1) -> Result<format::Replay> {
        let result = (|| {
            self.healthy()?;
            self.backing.verify(db)?;
            let replay = format::replay(
                db,
                self.installation_id,
                &self.policy,
                &self.redactor,
                Some(floor),
            )?;
            ensure!(
                replay.verified.namespace_id == self.namespace_id,
                AuthenticationAuditFailureV1::Integrity
            );
            self.backing.verify(db)?;
            Ok(replay)
        })();
        if result.is_err() {
            self.unhealthy.store(true, Ordering::Release);
        }
        result
    }

    fn verify_backing(&self, db: &Connection) -> Result<()> {
        let result = self.backing.verify(db);
        if result.is_err() {
            self.unhealthy.store(true, Ordering::Release);
        }
        result
    }

    pub fn append_typed(&self, record: AuthenticationRecordV1) -> Result<AuthenticationReceiptV1> {
        Ok(self
            .append_entry(format::Entry::Authentication { record })?
            .receipt)
    }

    pub fn mutate(&self, mutation: format::SessionMutation) -> Result<AuthenticationMutationV1> {
        self.append_entry(format::Entry::Session {
            mutation: Box::new(mutation),
        })
    }

    pub fn begin_verified_initial(
        &self,
        record: AuthenticationRecordV1,
        principal: PrincipalRefV1,
        access_fingerprint: AuthenticationTokenFingerprintV1,
        membership_recovery: bool,
    ) -> Result<AuthenticationMutationV1> {
        self.append_entry(format::Entry::VerifiedInitial {
            record,
            principal,
            access_fingerprint,
            membership_recovery,
        })
    }

    fn append_entry(&self, entry: format::Entry) -> Result<AuthenticationMutationV1> {
        self.healthy()?;
        // Never persist or alter an identity containing a redaction collision.
        let payload = format::checked_json(&entry, &self.redactor)?;
        let record = entry
            .record()
            .ok_or(AuthenticationAuditFailureV1::InvalidInput)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("authentication audit lock poisoned"))?;
        let LiveState { db, floor } = &mut *state;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let replay = self.inspect(&tx, floor)?;
        // This prefix was fully validated before any write in our transaction.
        // Remember it even when a later CAS/phase/capacity check refuses work.
        // A newly inserted, uncommitted receipt is never used as this witness.
        *floor = replay.verified.head.clone();
        if let Some(receipt) = replay.existing(record, &payload)? {
            if let format::Entry::Session { mutation } = &entry {
                replay.validate_mutation_replay(mutation, &receipt, &self.policy)?;
            }
            self.verify_backing(&tx)?;
            tx.commit()?;
            self.verify_backing(db)?;
            return Ok(AuthenticationMutationV1 {
                receipt,
                inserted: false,
            });
        }
        format::validate_record(record, &self.policy)?;
        replay.validate_transition(record)?;
        if matches!(&entry, format::Entry::Authentication { .. }) {
            replay.validate_generic_lane(record)?;
        }
        if let format::Entry::VerifiedInitial {
            principal,
            access_fingerprint,
            membership_recovery,
            ..
        } = &entry
        {
            replay.validate_initial(
                record,
                principal,
                *access_fingerprint,
                *membership_recovery,
                &payload,
                &self.policy,
            )?;
        }
        if let format::Entry::Session { mutation } = &entry {
            replay.validate_mutation(mutation, &self.policy)?;
            // Replay validates the timeless binding shape. A newly committed
            // admission/rotation must also still be unexpired at this action.
            if let format::SessionMutation::Admit { binding, .. }
            | format::SessionMutation::Rotate { binding, .. } = mutation.as_ref()
            {
                ensure!(
                    binding.expires_at_unix_seconds > chrono::Utc::now().timestamp(),
                    AuthenticationAuditFailureV1::InvalidInput
                );
            }
        }
        let records = replay
            .verified
            .records
            .checked_add(1)
            .ok_or(AuthenticationAuditFailureV1::Capacity)?;
        let bytes = replay
            .verified
            .serialized_bytes
            .checked_add(payload.len() as u64)
            .ok_or(AuthenticationAuditFailureV1::Capacity)?;
        // This same Immediate transaction owns both the Pending transition and
        // the terminal allowance derived from it. No other corrected writer
        // can spend its record/bytes between provider admission and completion.
        replay.check_terminal_capacity(&entry, records, bytes, &self.policy)?;
        let received = format::timestamp();
        let hash = format::digest(records, &received, &replay.verified.head.hash, &payload);
        let changed = tx.execute("INSERT INTO authentication_entries(seq,received_at,prev_hash,hash,payload,operation_id,phase) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![records as i64, received, replay.verified.head.hash, hash, payload, record.operation_id.to_string(), format::phase_key(record.phase).0])?;
        ensure!(changed == 1, AuthenticationAuditFailureV1::Integrity);
        let changed = tx.execute("UPDATE authentication_quota SET records=?1,bytes=?2 WHERE singleton=1 AND records=?3 AND bytes=?4", params![records as i64, bytes as i64, replay.verified.records as i64, replay.verified.serialized_bytes as i64])?;
        ensure!(changed == 1, AuthenticationAuditFailureV1::Integrity);
        let receipt = AuthenticationReceiptV1 {
            namespace_id: self.namespace_id,
            sequence: records,
            hash,
        };
        // Verify what SQLite actually stored before committing/acknowledging.
        let appended = self.inspect(&tx, floor)?;
        ensure!(
            appended.verified.head == receipt,
            AuthenticationAuditFailureV1::Integrity
        );
        tx.commit()?;
        self.verify_backing(db)?;
        *floor = receipt.clone();
        Ok(AuthenticationMutationV1 {
            receipt,
            inserted: true,
        })
    }

    fn read_projection<T>(&self, read: impl FnOnce(&format::Replay) -> Result<T>) -> Result<T> {
        self.healthy()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("authentication audit lock poisoned"))?;
        let LiveState { db, floor } = &mut *state;
        let tx = db.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let replay = self.inspect(&tx, floor)?;
        *floor = replay.verified.head.clone();
        let result = read(&replay)?;
        self.verify_backing(&tx)?;
        tx.commit()?;
        self.verify_backing(db)?;
        Ok(result)
    }

    pub fn verify(&self) -> Result<AuthenticationVerificationV1> {
        self.read_projection(|replay| Ok(replay.verified.clone()))
    }

    pub fn format_version(&self) -> Result<u32> {
        self.read_projection(|replay| {
            Ok(match replay.version {
                format::LEGACY_VERSION => 1,
                format::VERIFIED_INITIAL_VERSION => 2,
                format::VERSION => 3,
                _ => return Err(AuthenticationAuditFailureV1::Format.into()),
            })
        })
    }

    pub fn session_current(
        &self,
        session_id: Uuid,
    ) -> Result<Option<AuthenticationSessionCurrentV1>> {
        self.read_projection(|replay| {
            ensure!(
                !session_id.is_nil(),
                AuthenticationAuditFailureV1::InvalidInput
            );
            Ok(replay.session_current(session_id))
        })
    }

    pub fn fingerprint_seen(&self, fingerprint: &AuthenticationTokenFingerprintV1) -> Result<bool> {
        self.read_projection(|replay| Ok(replay.fingerprint_seen(fingerprint)))
    }

    pub fn backup_new(&self, path: &Path) -> Result<AuthenticationVerificationV1> {
        self.healthy()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("authentication audit lock poisoned"))?;
        let LiveState { db, floor } = &mut *state;
        // Keep one consistent source read snapshot through verification and
        // Backup, including when another legitimate handle appends in WAL.
        let source = db.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let expected = self.inspect(&source, floor)?.verified;
        *floor = expected.head.clone();
        let restored = self.backup_snapshot(&source, path, &expected)?;
        self.backing.verify(&source)?;
        source.commit()?;
        self.verify_backing(db)?;
        Ok(restored)
    }

    fn backup_snapshot(
        &self,
        source: &Connection,
        path: &Path,
        expected: &AuthenticationVerificationV1,
    ) -> Result<AuthenticationVerificationV1> {
        let (backing, guard) = StoreStorage::create_new(path)?;
        durable_parent(&backing)?;
        ensure!(
            backing.identity_key().1 != self.backing.identity_key().1,
            AuthenticationAuditFailureV1::Format
        );
        let mut destination = connection(&backing, false)?;
        durability(&destination)?;
        {
            let backup = rusqlite::backup::Backup::new(source, &mut destination)?;
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                backing.verify_names()?;
                self.backing.verify(source)?;
                if backup.step(256)? == rusqlite::backup::StepResult::Done {
                    break;
                }
                ensure!(
                    Instant::now() < deadline,
                    "authentication audit backup deadline exceeded"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        backing.verify(&destination)?;
        let restored = format::replay(
            &destination,
            self.installation_id,
            &self.policy,
            &self.redactor,
            Some(&expected.head),
        )?
        .verified;
        ensure!(
            restored == *expected,
            AuthenticationAuditFailureV1::Integrity
        );
        let quick: String = destination.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        ensure!(quick == "ok", AuthenticationAuditFailureV1::Integrity);
        backing.verify(&destination)?;
        destination.close().map_err(|(_, error)| error)?;
        // This is a new, now closed inode protected by the retained guard.
        backing.sync_closed(&guard)?;
        Ok(restored)
    }

    pub fn migrate_verified_initial_v2(
        &self,
        backup: &Path,
    ) -> Result<AuthenticationVerificationV1> {
        self.migrate_format(
            backup,
            format::LEGACY_VERSION,
            format::VERIFIED_INITIAL_VERSION,
        )
    }

    pub fn migrate_native_logout_v3(&self, backup: &Path) -> Result<AuthenticationVerificationV1> {
        self.migrate_format(backup, format::VERIFIED_INITIAL_VERSION, format::VERSION)
    }

    fn migrate_format(
        &self,
        backup: &Path,
        source_version: i64,
        destination_version: i64,
    ) -> Result<AuthenticationVerificationV1> {
        self.healthy()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("authentication audit lock poisoned"))?;
        let LiveState { db, floor } = &mut *state;
        // No writer can advance the head between this backup and the format
        // transition. Migration consumes no ledger entry or quota capacity.
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let replay = self.inspect(&tx, floor)?;
        *floor = replay.verified.head.clone();
        ensure!(
            replay.version == source_version,
            AuthenticationAuditFailureV1::Format
        );
        let expected = replay.verified;
        // SQLite Backup cannot read through its own write transaction. Keep
        // the IMMEDIATE writer fence, but copy the same committed WAL snapshot
        // through a separate read-only SQLite-managed connection.
        let mut probe = connection(&self.backing, true)?;
        {
            let source = probe.transaction_with_behavior(TransactionBehavior::Deferred)?;
            ensure!(
                self.inspect(&source, floor)?.verified == expected,
                AuthenticationAuditFailureV1::Integrity
            );
            self.backup_snapshot(&source, backup, &expected)?;
            source.commit()?;
        }
        probe.close().map_err(|(_, error)| error)?;
        tx.pragma_update(None, "user_version", destination_version)?;
        ensure!(
            self.inspect(&tx, floor)?.verified == expected,
            AuthenticationAuditFailureV1::Integrity
        );
        tx.commit()?;
        self.verify_backing(db)?;
        Ok(expected)
    }

    pub fn storage_identity(&self) -> Result<AuthenticationStorageIdentityV1> {
        self.healthy()?;
        self.backing.verify_names()?;
        Ok(AuthenticationStorageIdentityV1(
            self.backing.identity_key().1,
        ))
    }

    pub fn ensure_distinct_from_business(&self, store: &crate::audit::Store) -> Result<()> {
        self.healthy()?;
        self.backing.verify_names()?;
        store.backing.verify_names()?;
        ensure!(
            self.backing.identity_key().1 != store.backing.identity_key().1,
            AuthenticationAuditFailureV1::Format
        );
        Ok(())
    }
}
