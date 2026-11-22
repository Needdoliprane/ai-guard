//! Explicit stopped-consumer14→15 operation. No normal opener auto-migrates.
//! The same verified source backup also serves the in-place maintenance of an
//! adopted source (identity markers), which never changes its format.
use super::*;
use crate::human_authority::{
    HumanSourceKindV1, SourceStorageBindingV1, maintenance::HumanMaintenanceSourceToken,
};
use std::path::PathBuf;

/// Which stopped-consumer maintenance a verified source backup belongs to.
/// A proof never crosses steps: egress keeps its exact Store14 source, and
/// the in-place step keeps the adopted source's own format14 or15.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceBackupStepV1 {
    Egress15,
    InPlace,
}

fn physical(
    store: &Store,
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
) -> anyhow::Result<()> {
    physical_for(store, tx, token, SourceBackupStepV1::Egress15)
}

/// The held writer, the exact physical source and the token's step. An
/// in-place token is admitted only by an in-place plan (`in_place_anchor`).
pub(in crate::audit) fn physical_for(
    store: &Store,
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
    step: SourceBackupStepV1,
) -> anyhow::Result<()> {
    token.check()?;
    storage::verify_connection_main(&store.backing, tx)?;
    ensure(tx.transaction_state(Some("main"))? == rusqlite::TransactionState::Write)?;
    let d = token.descriptor();
    let format = match step {
        SourceBackupStepV1::Egress15 => d.required_format == 15,
        SourceBackupStepV1::InPlace => {
            matches!(d.required_format, 14 | 15) && token.in_place_anchor()?.is_some()
        }
    };
    let (path, identity) = store.backing.identity_key();
    ensure(
        d.kind == HumanSourceKindV1::TenantStore
            && format
            && d.tenant_id.as_deref() == Some(store.tenant_id())
            && d.durable_namespace_id == review_authority::namespace(tx)?
            && d.storage_binding.path() == path
            && identity.physical_identity()
                == (
                    d.storage_binding.source_device,
                    d.storage_binding.source_inode,
                ),
    )?;
    token.check().map_err(Into::into)
}
fn verify14(store: &Store, db: &Connection, prefix: &(i64, String)) -> anyhow::Result<u64> {
    verify_format(store, db, prefix, 14)
}
fn verify_format(
    store: &Store,
    db: &Connection,
    prefix: &(i64, String),
    format: i64,
) -> anyhow::Result<u64> {
    ensure(db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? == format)?;
    validate_schema(db, false)?;
    Store::validate_metadata(db, store.tenant_id())?;
    control::validate(db)?;
    let integrity = Store::check_integrity(db)?;
    ensure(integrity.valid && audit_head(db)? == *prefix)?;
    Ok(integrity.event_count)
}

fn backup_path(
    token: &HumanMaintenanceSourceToken,
    path: &Path,
    step: SourceBackupStepV1,
) -> anyhow::Result<PathBuf> {
    let path = crate::control_fs::absolute_path(path)?;
    let parent = crate::control_fs::Directory::open_existing(
        path.parent().context("backup parent required")?,
    )?;
    ensure(parent.physical_identity()? != token.descriptor().storage_binding.parent_identity())
        .context(match step {
            SourceBackupStepV1::Egress15 => "egress backup parent must differ from source parent",
            SourceBackupStepV1::InPlace => {
                "identity marker backup parent must differ from source parent"
            }
        })?;
    Ok(path)
}

/// Store format proved by a backup of this step for this token.
fn step_format(token: &HumanMaintenanceSourceToken, step: SourceBackupStepV1) -> i64 {
    match step {
        SourceBackupStepV1::Egress15 => 14,
        SourceBackupStepV1::InPlace => i64::from(token.descriptor().required_format),
    }
}

/// Proof borrows a real maintenance token and retains the verified backup's
/// own filesystem identity. A path/receipt/serialized manifest cannot mint it.
pub(crate) struct VerifiedEgressSourceBackup<'a> {
    token: &'a HumanMaintenanceSourceToken,
    step: SourceBackupStepV1,
    source_binding: (std::path::PathBuf, crate::control_fs::PrivateEntryIdentity),
    source_head: (i64, String),
    event_count: u64,
    backing: StoreStorage,
}
/// The same proof for any source step; the name is kept for egress callers.
pub(crate) type VerifiedSourceBackup<'a> = VerifiedEgressSourceBackup<'a>;
impl VerifiedEgressSourceBackup<'_> {
    /// Exact committed prefix the backup proves. Not an authority by itself.
    pub(in crate::audit) fn source_head(&self) -> &(i64, String) {
        &self.source_head
    }

    /// Describe the existing proof for the journal without opening a raw file.
    pub(crate) fn journal_observation(
        &self,
        token: &HumanMaintenanceSourceToken,
    ) -> anyhow::Result<(
        SourceStorageBindingV1,
        review_authority::AuthorityReceiptV1,
        u64,
    )> {
        token.check()?;
        ensure(token.same_guard_and_source(self.token))?;
        let binding = self.backing.retained_binding()?;
        token.check()?;
        Ok((
            binding,
            review_authority::AuthorityReceiptV1 {
                seq: self.source_head.0,
                hash: self.source_head.1.clone(),
            },
            self.event_count,
        ))
    }

    fn verify(&self, store: &Store) -> anyhow::Result<()> {
        self.token.check()?;
        ensure(self.source_binding == store.backing.identity_key())?;
        let db = Connection::open_with_flags(
            self.backing.path(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        db.busy_timeout(std::time::Duration::ZERO)?;
        storage::verify_connection_main(&self.backing, &db)?;
        ensure(
            verify_format(
                store,
                &db,
                &self.source_head,
                step_format(self.token, self.step),
            )? == self.event_count,
        )?;
        let quick: String = db.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        ensure(quick == "ok")?;
        storage::verify_connection_main(&self.backing, &db)?;
        db.close().map_err(|(_, e)| e)?;
        self.backing.verify_names()?;
        self.token.check().map_err(Into::into)
    }
}

pub(in crate::audit) fn prepare_backup<'a>(
    store: &Store,
    tx: &Transaction<'_>,
    token: &'a HumanMaintenanceSourceToken,
    path: &Path,
) -> anyhow::Result<VerifiedEgressSourceBackup<'a>> {
    prepare_backup_for(store, tx, token, path, SourceBackupStepV1::Egress15)
}

/// Exclusive new SQLite backup of the exact held source prefix for `step`.
pub(in crate::audit) fn prepare_backup_for<'a>(
    store: &Store,
    tx: &Transaction<'_>,
    token: &'a HumanMaintenanceSourceToken,
    path: &Path,
    step: SourceBackupStepV1,
) -> anyhow::Result<VerifiedEgressSourceBackup<'a>> {
    physical_for(store, tx, token, step)?;
    let format = step_format(token, step);
    let prefix = audit_head(tx)?;
    let count = verify_format(store, tx, &prefix, format)?;
    let path = backup_path(token, path, step)?;
    let (backing, initialization) = StoreStorage::create_new(&path)?;
    let mut destination = Connection::open_with_flags(
        backing.path(),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    destination.busy_timeout(std::time::Duration::ZERO)?;
    destination.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA synchronous=FULL;")?;
    storage::verify_connection_main(&backing, &destination)?;
    // Separate SQLite reader sees the exact committed prefix, while source's
    // retained Immediate writer prevents every normal/maintenance mutation.
    let reader = Connection::open_with_flags(
        store.backing.path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    reader.busy_timeout(std::time::Duration::ZERO)?;
    storage::verify_connection_main(&store.backing, &reader)?;
    ensure(verify_format(store, &reader, &prefix, format)? == count)?;
    {
        let backup = rusqlite::backup::Backup::new(&reader, &mut destination)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            physical_for(store, tx, token, step)?;
            backing.verify_names()?;
            match backup.step(256)? {
                rusqlite::backup::StepResult::Done => break,
                rusqlite::backup::StepResult::More => ensure(std::time::Instant::now() < deadline)?,
                _ => return Err(EgressStoreFailure::Permission.into()),
            }
        }
    }
    ensure(verify_format(store, &reader, &prefix, format)? == count)?;
    storage::verify_connection_main(&store.backing, &reader)?;
    reader.close().map_err(|(_, e)| e)?;
    ensure(verify_format(store, &destination, &prefix, format)? == count)?;
    storage::verify_connection_main(&backing, &destination)?;
    let quick: String = destination.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    ensure(quick == "ok")?;
    destination.close().map_err(|(_, e)| e)?;
    backing.sync_closed(&initialization)?;
    let proof = VerifiedEgressSourceBackup {
        token,
        step,
        source_binding: store.backing.identity_key(),
        source_head: prefix,
        event_count: count,
        backing,
    };
    proof.verify(store)?;
    physical_for(store, tx, token, step)?;
    Ok(proof)
}

pub(in crate::audit) fn validate_maintenance_identity(
    db: &Connection,
    token: &HumanMaintenanceSourceToken,
) -> anyhow::Result<()> {
    token.check()?;
    let format: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    ensure(token.descriptor().required_format == 15 && matches!(format, 14 | 15))?;
    if format == 15 {
        let p = replay(db)?;
        let g = &p.genesis.as_ref().ok_or(EgressStoreFailure::Integrity)?.0;
        ensure(
            g.migration_id == token.migration_id() && g.source_id == token.descriptor().source_id,
        )?;
    }
    token.check().map_err(Into::into)
}

pub(in crate::audit) fn maintenance_prefix(
    store: &Store,
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
    before_install: bool,
) -> anyhow::Result<review_authority::AuthorityReceiptV1> {
    physical(store, tx, token)?;
    validate_maintenance_identity(tx, token)?;
    let format: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let (seq, hash) = if format == 14 {
        audit_head(tx)?
    } else {
        ensure(!before_install)?;
        let p = replay(tx)?;
        let g = &p.genesis.as_ref().ok_or(EgressStoreFailure::Integrity)?.0;
        (g.previous_seq, g.previous_hash.clone())
    };
    Ok(review_authority::AuthorityReceiptV1 { seq, hash })
}

pub(in crate::audit) fn resume_backup<'a>(
    store: &Store,
    tx: &Transaction<'_>,
    token: &'a HumanMaintenanceSourceToken,
    path: &Path,
) -> anyhow::Result<VerifiedEgressSourceBackup<'a>> {
    let prefix = maintenance_prefix(store, tx, token, false)?;
    resume_backup_for(store, tx, token, path, SourceBackupStepV1::Egress15, prefix)
}

/// Re-check an existing backup of `step` against the prefix the caller
/// derived from durable replay. It never creates or repairs a backup.
pub(in crate::audit) fn resume_backup_for<'a>(
    store: &Store,
    tx: &Transaction<'_>,
    token: &'a HumanMaintenanceSourceToken,
    path: &Path,
    step: SourceBackupStepV1,
    prefix: review_authority::AuthorityReceiptV1,
) -> anyhow::Result<VerifiedEgressSourceBackup<'a>> {
    physical_for(store, tx, token, step)?;
    let path = backup_path(token, path, step)?;
    let (backing, _guard) = StoreStorage::open_existing(&path)?;
    let proof = VerifiedEgressSourceBackup {
        token,
        step,
        source_binding: store.backing.identity_key(),
        source_head: (prefix.seq, prefix.hash),
        event_count: u64::try_from(prefix.seq)?,
        backing,
    };
    proof.verify(store)?;
    physical_for(store, tx, token, step)?;
    Ok(proof)
}

/// Inputs of an in-place install or confirmation: the held in-place writer,
/// a proof of that step from the same guard, re-verified through SQLite.
pub(in crate::audit) fn verify_in_place_backup(
    store: &Store,
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
    backup: &VerifiedEgressSourceBackup<'_>,
) -> anyhow::Result<()> {
    physical_for(store, tx, token, SourceBackupStepV1::InPlace)?;
    ensure(
        backup.step == SourceBackupStepV1::InPlace && token.same_guard_and_source(backup.token),
    )?;
    backup.verify(store)
}

fn validate_install_inputs(
    store: &Store,
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
    backup: &VerifiedEgressSourceBackup<'_>,
    limits: &EgressStoreLimitsV1,
) -> anyhow::Result<()> {
    physical(store, tx, token)?;
    ensure(
        backup.step == SourceBackupStepV1::Egress15 && token.same_guard_and_source(backup.token),
    )?;
    backup.verify(store)?;
    limits.validate()
}

fn read_installed_receipt(
    store: &Store,
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
    backup: &VerifiedEgressSourceBackup<'_>,
    limits: &EgressStoreLimitsV1,
) -> anyhow::Result<review_authority::AuthorityReceiptV1> {
    validate_maintenance_identity(tx, token)?;
    let p = replay(tx)?;
    let (genesis, seq, hash) = p.genesis.as_ref().ok_or(EgressStoreFailure::Integrity)?;
    cas(genesis.limits == *limits
        && (genesis.previous_seq, &genesis.previous_hash)
            == (backup.source_head.0, &backup.source_head.1))?;
    physical(store, tx, token)?;
    Ok(review_authority::AuthorityReceiptV1 {
        seq: *seq,
        hash: hash.clone(),
    })
}

pub(in crate::audit) fn confirm_format15(
    store: &Store,
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
    backup: &VerifiedEgressSourceBackup<'_>,
    limits: &EgressStoreLimitsV1,
) -> anyhow::Result<review_authority::AuthorityReceiptV1> {
    validate_install_inputs(store, tx, token, backup, limits)?;
    ensure(tx.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? == 15)?;
    read_installed_receipt(store, tx, token, backup, limits)
}

pub(in crate::audit) fn install_format15(
    store: &Store,
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
    backup: &VerifiedEgressSourceBackup<'_>,
    limits: &EgressStoreLimitsV1,
) -> anyhow::Result<review_authority::AuthorityReceiptV1> {
    validate_install_inputs(store, tx, token, backup, limits)?;
    let format: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if format == 15 {
        return read_installed_receipt(store, tx, token, backup, limits);
    }
    ensure(format == 14 && audit_head(tx)? == backup.source_head)?;
    verify14(store, tx, &backup.source_head)?;
    let source = review_authority::replay(tx)?;
    let d = token.descriptor();
    ensure(
        source.anchor.installation_id == d.installation_id
            && source.anchor.source_id == d.source_id
            && source.anchor.tenant_id == store.tenant_id()
            && source.anchor.durable_namespace_id == d.durable_namespace_id,
    )?;
    let g = Genesis {
        version: 1,
        from_format: 14,
        format: 15,
        migration_id: token.migration_id(),
        tenant_id: store.tenant_id().into(),
        source_id: d.source_id,
        namespace: d.durable_namespace_id,
        source_anchor: source.anchor_receipt,
        previous_seq: backup.source_head.0,
        previous_hash: backup.source_head.1.clone(),
        limits: limits.clone(),
    };
    let clock = workload::capture_control_time(tx)?;
    let mut event = Event::new(GENESIS_KIND);
    event.id = token.migration_id().to_string();
    event.timestamp = crate::broker_registry::format_time(clock.at());
    event.coverage = "native".into();
    event.data = serde_json::to_value(&g)?;
    marker(&event)?;
    let original = serde_json::to_value(&event)?;
    let (event, payload) = store.prepare_event(event)?;
    ensure(serde_json::to_value(&event)? == original && payload.len() <= EVENT_BYTES)?;
    for (_, _, sql) in SCHEMA {
        tx.execute_batch(sql)?;
    }
    tx.pragma_update(None, "user_version", 15)?;
    let stored = store.append_authorized_in_transaction(tx, event, payload, &clock)?;
    ensure(tx.execute("INSERT INTO egress_policy_meta(singleton,version,genesis_seq,genesis_hash,limits_json) VALUES(1,1,?1,?2,?3)",params![stored.seq,stored.hash,serde_json::to_string(limits)?])?==1)?;
    validate_schema(tx, false)?;
    control::validate(tx)?;
    ensure(Store::check_integrity(tx)?.valid)?;
    physical(store, tx, token)?;
    workload::validate_fresh_control_time(tx, &clock)?;
    Ok(review_authority::AuthorityReceiptV1 {
        seq: stored.seq,
        hash: stored.hash,
    })
}
