//! One real offline13→14 source step; no closure-wide atomicity or activation.
//! Root retains the maintenance union, every writer transaction and its private
//! durable progress journal before backups/any conversion, then commits/reopens
//! each step explicitly. A partial commit is preserved, never downgraded here.
use super::review_authority::{
    self as authority, AuthorityReceiptV1, AuthorityRecordV1, ReviewAuthorityFailure,
    SourceAnchorV1,
};
use super::*;
use crate::human_authority::{HumanSourceKindV1, maintenance::HumanMaintenanceSourceToken};

fn require(value: bool) -> anyhow::Result<()> {
    if value {
        Ok(())
    } else {
        Err(ReviewAuthorityFailure::Mismatch.into())
    }
}
fn writer(tx: &Transaction<'_>) -> anyhow::Result<()> {
    require(tx.transaction_state(Some("main"))? == rusqlite::TransactionState::Write)
}
fn physical(
    store: &Store,
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
) -> anyhow::Result<()> {
    token.check()?;
    // Refuse a foreign connection before interpreting its writer/namespace
    // state as facts about this retained source.
    storage::verify_connection_main(&store.backing, tx)?;
    writer(tx)?;
    let descriptor = token.descriptor();
    let (path, identity) = store.backing.identity_key();
    require(
        descriptor.kind == HumanSourceKindV1::TenantStore
            && descriptor.required_format == 14
            && descriptor.tenant_id.as_deref() == Some(store.tenant_id())
            && descriptor.durable_namespace_id == authority::namespace(tx)?
            && descriptor.storage_binding.path() == path
            && identity.physical_identity()
                == (
                    descriptor.storage_binding.source_device,
                    descriptor.storage_binding.source_inode,
                ),
    )?;
    token.check()?;
    Ok(())
}
fn verify13(store: &Store, db: &Connection, head: &(i64, String)) -> anyhow::Result<u64> {
    let format: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    require(format == 13)?;
    validate_schema(db, false)?;
    Store::validate_metadata(db, store.tenant_id())?;
    control::validate(db)?;
    authority::validate(db)?;
    let checked = Store::check_integrity(db)?;
    require(checked.valid && audit_head(db)? == *head)?;
    Ok(checked.event_count)
}
fn verify14(store: &Store, db: &Connection) -> anyhow::Result<authority::AuthoritySnapshot> {
    let format: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    require(format == 14)?;
    validate_schema(db, false)?;
    Store::validate_metadata(db, store.tenant_id())?;
    control::validate(db)?;
    super::review_format14::validate(db)?;
    require(Store::check_integrity(db)?.valid)?;
    authority::replay(db)
}
struct VerifiedBackup {
    source_binding: (std::path::PathBuf, crate::control_fs::PrivateEntryIdentity),
    source_head: (i64, String),
    event_count: u64,
    backing: StoreStorage,
}
impl VerifiedBackup {
    fn verify_bytes(&self, store: &Store) -> anyhow::Result<()> {
        require(self.source_binding == store.backing.identity_key())?;
        let db = Connection::open_with_flags(
            self.backing.path(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        db.busy_timeout(std::time::Duration::ZERO)?;
        storage::verify_connection_main(&self.backing, &db)?;
        require(verify13(store, &db, &self.source_head)? == self.event_count)?;
        let quick: String = db.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        require(quick == "ok")?;
        storage::verify_connection_main(&self.backing, &db)?;
        db.close().map_err(|(_, e)| e)?;
        self.backing.verify_names()
    }
    fn revalidate(&self, store: &Store, tx: &Transaction<'_>) -> anyhow::Result<()> {
        writer(tx)?;
        storage::verify_connection_main(&store.backing, tx)?;
        require(audit_head(tx)? == self.source_head)?;
        self.verify_bytes(store)
    }
}
fn copy_verified_backup(
    store: &Store,
    tx: &Transaction<'_>,
    path: &Path,
) -> anyhow::Result<VerifiedBackup> {
    writer(tx)?;
    storage::verify_connection_main(&store.backing, tx)?;
    let prefix = audit_head(tx)?;
    let count = verify13(store, tx, &prefix)?;
    let path = crate::control_fs::absolute_path(path)?;
    // Root holds initialization locks for source parents. A separate fresh
    // backup parent avoids recursive acquisition of their fixed init lock.
    let parent = crate::control_fs::Directory::open_existing(
        path.parent().context("backup parent required")?,
    )?;
    let source_parent = crate::control_fs::Directory::open_existing(
        store
            .backing
            .path()
            .parent()
            .context("source parent required")?,
    )?;
    require(parent.physical_identity()? != source_parent.physical_identity()?)?;
    let (backing, initialization) = StoreStorage::create_new(&path)?;
    let mut destination = Connection::open_with_flags(
        backing.path(),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    destination.busy_timeout(std::time::Duration::ZERO)?;
    destination.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA synchronous=FULL;")?;
    storage::verify_connection_main(&backing, &destination)?;
    // A separate VFS reader sees the committed prefix while this source's
    // Immediate writer blocks all old writers. No raw main/SHM FD is opened.
    let reader = Connection::open_with_flags(
        store.backing.path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    reader.busy_timeout(std::time::Duration::ZERO)?;
    storage::verify_connection_main(&store.backing, &reader)?;
    require(verify13(store, &reader, &prefix)? == count)?;
    {
        let backup = rusqlite::backup::Backup::new(&reader, &mut destination)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            storage::verify_connection_main(&store.backing, tx)?;
            backing.verify_names()?;
            match backup.step(256)? {
                rusqlite::backup::StepResult::Done => break,
                rusqlite::backup::StepResult::More => {
                    require(std::time::Instant::now() < deadline)?
                }
                _ => return Err(ReviewAuthorityFailure::Unavailable.into()),
            }
        }
    }
    require(verify13(store, &reader, &prefix)? == count)?;
    storage::verify_connection_main(&store.backing, &reader)?;
    reader.close().map_err(|(_, e)| e)?;
    storage::verify_connection_main(&backing, &destination)?;
    require(verify13(store, &destination, &prefix)? == count)?;
    let quick: String = destination.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    require(quick == "ok")?;
    destination.close().map_err(|(_, e)| e)?;
    backing.sync_closed(&initialization)?;
    let verified = VerifiedBackup {
        source_binding: store.backing.identity_key(),
        source_head: prefix,
        event_count: count,
        backing,
    };
    verified.revalidate(store, tx)?;
    Ok(verified)
}

/// Runtime ownership is borrowed from the actual retained maintenance token.
/// This is a verified private backup, never deserializable progress or Ready.
pub(crate) struct VerifiedReviewSourceBackup<'a> {
    token: &'a HumanMaintenanceSourceToken,
    backup: VerifiedBackup,
}
impl VerifiedReviewSourceBackup<'_> {
    /// Descriptive journal fields. Reuse still requires the real token, writer
    /// and backup verification; these values cannot reconstruct this proof.
    pub(crate) fn migration_id(&self) -> uuid::Uuid {
        self.token.migration_id()
    }
    pub(crate) fn backup_binding(
        &self,
    ) -> (std::path::PathBuf, crate::control_fs::PrivateEntryIdentity) {
        self.backup.backing.identity_key()
    }
    pub(crate) fn prefix(&self) -> &(i64, String) {
        &self.backup.source_head
    }
    pub(crate) fn event_count(&self) -> u64 {
        self.backup.event_count
    }
}
impl std::fmt::Debug for VerifiedReviewSourceBackup<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VerifiedReviewSourceBackup(<private retained backup>)")
    }
}
#[derive(Clone, Debug, Serialize)]
pub(crate) enum ReviewSourceMigrationState {
    Ordinary13 {
        prefix_seq: i64,
        prefix_hash: String,
    },
    Converted14 {
        anchor: AuthorityReceiptV1,
    },
}
fn match_anchor(
    store: &Store,
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
) -> anyhow::Result<AuthorityReceiptV1> {
    physical(store, tx, token)?;
    let snapshot = verify14(store, tx)?;
    let expected = token.descriptor();
    require(
        snapshot.anchor.migration_id == token.migration_id()
            && snapshot.anchor.installation_id == expected.installation_id
            && snapshot.anchor.source_id == expected.source_id
            && snapshot.anchor.kind == expected.kind
            && snapshot.anchor.tenant_id == store.tenant_id()
            && snapshot.anchor.durable_namespace_id == expected.durable_namespace_id,
    )?;
    physical(store, tx, token)?;
    Ok(snapshot.anchor_receipt)
}
impl Store {
    pub(crate) fn prepare_review_source_backup<'a>(
        &self,
        tx: &Transaction<'_>,
        token: &'a HumanMaintenanceSourceToken,
        path: &Path,
    ) -> anyhow::Result<VerifiedReviewSourceBackup<'a>> {
        physical(self, tx, token)?;
        let backup = copy_verified_backup(self, tx, path)?;
        physical(self, tx, token)?;
        Ok(VerifiedReviewSourceBackup { token, backup })
    }
    /// Resume reads the actual backup into a retained proof; it never recreates
    /// or overwrites a missing/existing backup to make partial progress green.
    pub(crate) fn resume_review_source_backup<'a>(
        &self,
        tx: &Transaction<'_>,
        token: &'a HumanMaintenanceSourceToken,
        path: &Path,
    ) -> anyhow::Result<VerifiedReviewSourceBackup<'a>> {
        physical(self, tx, token)?;
        let format: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let prefix = match format {
            13 => {
                let prefix = audit_head(tx)?;
                verify13(self, tx, &prefix)?;
                prefix
            }
            14 => {
                match_anchor(self, tx, token)?;
                let snapshot = verify14(self, tx)?;
                (snapshot.anchor.previous_seq, snapshot.anchor.previous_hash)
            }
            _ => return Err(ReviewAuthorityFailure::Mismatch.into()),
        };
        let (backing, _guard) = StoreStorage::open_existing(path)?;
        let db = Connection::open_with_flags(
            backing.path(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        db.busy_timeout(std::time::Duration::ZERO)?;
        storage::verify_connection_main(&backing, &db)?;
        let count = verify13(self, &db, &prefix)?;
        db.close().map_err(|(_, e)| e)?;
        let backup = VerifiedBackup {
            source_binding: self.backing.identity_key(),
            source_head: prefix,
            event_count: count,
            backing,
        };
        backup.verify_bytes(self)?;
        physical(self, tx, token)?;
        Ok(VerifiedReviewSourceBackup { token, backup })
    }
    pub(crate) fn inspect_review_source_migration(
        &self,
        tx: &Transaction<'_>,
        token: &HumanMaintenanceSourceToken,
    ) -> anyhow::Result<ReviewSourceMigrationState> {
        physical(self, tx, token)?;
        let format: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let result = match format {
            13 => {
                let prefix = audit_head(tx)?;
                verify13(self, tx, &prefix)?;
                ReviewSourceMigrationState::Ordinary13 {
                    prefix_seq: prefix.0,
                    prefix_hash: prefix.1,
                }
            }
            14 => ReviewSourceMigrationState::Converted14 {
                anchor: match_anchor(self, tx, token)?,
            },
            _ => return Err(ReviewAuthorityFailure::Mismatch.into()),
        };
        physical(self, tx, token)?;
        Ok(result)
    }
    /// Appends only inside the caller's real writer transaction. The returned
    /// receipt is pending until Root commits, reopens and verifies this source.
    /// Root must retain writers for the entire union BEFORE any backup/DDL.
    pub(crate) fn install_review_source_format14_in_transaction(
        &self,
        tx: &Transaction<'_>,
        token: &HumanMaintenanceSourceToken,
        backup: &VerifiedReviewSourceBackup<'_>,
    ) -> anyhow::Result<AuthorityReceiptV1> {
        physical(self, tx, token)?;
        require(token.same_guard_and_source(backup.token))?;
        backup.backup.verify_bytes(self)?;
        let format: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if format == 14 {
            let receipt = match_anchor(self, tx, token)?;
            let snapshot = authority::replay(tx)?;
            require(
                (snapshot.anchor.previous_seq, snapshot.anchor.previous_hash)
                    == backup.backup.source_head,
            )?;
            physical(self, tx, token)?;
            return Ok(receipt);
        }
        require(format == 13)?;
        backup.backup.revalidate(self, tx)?;
        let descriptor = token.descriptor();
        let prefix = audit_head(tx)?;
        for (_, _, sql) in authority::SCHEMA
            .iter()
            .chain(super::review_binding::SCHEMA.iter())
        {
            tx.execute_batch(sql)?;
        }
        // This private14 profile is exactly13 plus authority and binding.
        // Any future additions require an explicit new closed profile before
        // activation, never arbitrary acceptance of extra schema objects.
        tx.pragma_update(None, "user_version", 14)?;
        let record = SourceAnchorV1 {
            version: 1,
            protocol: authority::SOURCE_PROTOCOL.into(),
            from_format: 13,
            format: 14,
            migration_id: token.migration_id(),
            installation_id: descriptor.installation_id,
            source_id: descriptor.source_id,
            kind: descriptor.kind,
            tenant_id: self.tenant_id.clone(),
            durable_namespace_id: descriptor.durable_namespace_id,
            previous_seq: prefix.0,
            previous_hash: prefix.1,
        };
        token.check()?;
        let receipt = authority::append_record(
            self,
            tx,
            token.migration_id(),
            AuthorityRecordV1::SourceAnchor(record),
            None,
        )?;
        verify14(self, tx)?;
        physical(self, tx, token)?;
        token.check()?;
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests;
