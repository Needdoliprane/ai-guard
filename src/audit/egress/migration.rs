//! Explicit stopped-consumer14→15 operation. No normal opener auto-migrates.
use super::*;
use crate::human_authority::{HumanSourceKindV1, maintenance::HumanMaintenanceSourceToken};

fn physical(
    store: &Store,
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
) -> anyhow::Result<()> {
    token.check()?;
    storage::verify_connection_main(&store.backing, tx)?;
    ensure(tx.transaction_state(Some("main"))? == rusqlite::TransactionState::Write)?;
    let d = token.descriptor();
    let (path, identity) = store.backing.identity_key();
    ensure(
        d.kind == HumanSourceKindV1::TenantStore
            && d.required_format == 15
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
    ensure(db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? == 14)?;
    validate_schema(db, false)?;
    Store::validate_metadata(db, store.tenant_id())?;
    control::validate(db)?;
    let integrity = Store::check_integrity(db)?;
    ensure(integrity.valid && audit_head(db)? == *prefix)?;
    Ok(integrity.event_count)
}

/// Proof borrows a real maintenance token and retains the verified backup's
/// own filesystem identity. A path/receipt/serialized manifest cannot mint it.
pub(crate) struct VerifiedEgressSourceBackup<'a> {
    token: &'a HumanMaintenanceSourceToken,
    source_binding: (std::path::PathBuf, crate::control_fs::PrivateEntryIdentity),
    source_head: (i64, String),
    event_count: u64,
    backing: StoreStorage,
}
impl VerifiedEgressSourceBackup<'_> {
    fn verify(&self, store: &Store) -> anyhow::Result<()> {
        self.token.check()?;
        ensure(self.source_binding == store.backing.identity_key())?;
        let db = Connection::open_with_flags(
            self.backing.path(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        db.busy_timeout(std::time::Duration::ZERO)?;
        storage::verify_connection_main(&self.backing, &db)?;
        ensure(verify14(store, &db, &self.source_head)? == self.event_count)?;
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
    physical(store, tx, token)?;
    let prefix = audit_head(tx)?;
    let count = verify14(store, tx, &prefix)?;
    let path = crate::control_fs::absolute_path(path)?;
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
    ensure(parent.physical_identity()? != source_parent.physical_identity()?)?;
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
    ensure(verify14(store, &reader, &prefix)? == count)?;
    {
        let backup = rusqlite::backup::Backup::new(&reader, &mut destination)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            physical(store, tx, token)?;
            backing.verify_names()?;
            match backup.step(256)? {
                rusqlite::backup::StepResult::Done => break,
                rusqlite::backup::StepResult::More => ensure(std::time::Instant::now() < deadline)?,
                _ => return Err(EgressStoreFailure::Permission.into()),
            }
        }
    }
    ensure(verify14(store, &reader, &prefix)? == count)?;
    storage::verify_connection_main(&store.backing, &reader)?;
    reader.close().map_err(|(_, e)| e)?;
    ensure(verify14(store, &destination, &prefix)? == count)?;
    storage::verify_connection_main(&backing, &destination)?;
    let quick: String = destination.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    ensure(quick == "ok")?;
    destination.close().map_err(|(_, e)| e)?;
    backing.sync_closed(&initialization)?;
    let proof = VerifiedEgressSourceBackup {
        token,
        source_binding: store.backing.identity_key(),
        source_head: prefix,
        event_count: count,
        backing,
    };
    proof.verify(store)?;
    physical(store, tx, token)?;
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

pub(in crate::audit) fn resume_backup<'a>(
    store: &Store,
    tx: &Transaction<'_>,
    token: &'a HumanMaintenanceSourceToken,
    path: &Path,
) -> anyhow::Result<VerifiedEgressSourceBackup<'a>> {
    physical(store, tx, token)?;
    validate_maintenance_identity(tx, token)?;
    let format: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let prefix = if format == 14 {
        audit_head(tx)?
    } else {
        let p = replay(tx)?;
        let g = &p.genesis.as_ref().ok_or(EgressStoreFailure::Integrity)?.0;
        (g.previous_seq, g.previous_hash.clone())
    };
    let (backing, _guard) = StoreStorage::open_existing(path)?;
    let proof = VerifiedEgressSourceBackup {
        token,
        source_binding: store.backing.identity_key(),
        source_head: prefix.clone(),
        event_count: u64::try_from(prefix.0)?,
        backing,
    };
    proof.verify(store)?;
    physical(store, tx, token)?;
    Ok(proof)
}

pub(in crate::audit) fn install_format15(
    store: &Store,
    tx: &Transaction<'_>,
    token: &HumanMaintenanceSourceToken,
    backup: &VerifiedEgressSourceBackup<'_>,
    limits: &EgressStoreLimitsV1,
) -> anyhow::Result<review_authority::AuthorityReceiptV1> {
    physical(store, tx, token)?;
    ensure(token.same_guard_and_source(backup.token))?;
    backup.verify(store)?;
    limits.validate()?;
    let format: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if format == 15 {
        validate_maintenance_identity(tx, token)?;
        let p = replay(tx)?;
        let (g, seq, hash) = p.genesis.as_ref().ok_or(EgressStoreFailure::Integrity)?;
        cas(g.limits == *limits && (g.previous_seq, g.previous_hash.clone()) == backup.source_head)?;
        physical(store, tx, token)?;
        return Ok(review_authority::AuthorityReceiptV1 {
            seq: *seq,
            hash: hash.clone(),
        });
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
