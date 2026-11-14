//! Owned broker binding writers and status reads for the proposed
//! `broker_supervision_owned_v1` profile.
//!
//! Writers run under a real write epoch of the retained tenant source and an
//! `IdentityAuthority` transaction; `require_identity_authority` precedes any
//! replay or idempotence. They persist digests only and reuse the existing
//! binding projection, history and audit body. A credential secret never
//! enters this module. The existing `Business` scoped writers are neither
//! used nor presented as sufficient. Every committed binding change advances
//! the owned human discovery generation (`workload_binding_history`), so a
//! running owned gateway requires a rebuild; offline maintenance with the
//! control plane stopped is the recommended caller (decision D1).
//!
//! The same maintenance also verifies an operator-supplied backup of this
//! tenant Store before its first mutation: a private regular file outside the
//! source directory, without sidecar, with the operator's SHA-256, opened
//! read-only and immutable, whose format, tenant, namespace, schema and chain
//! integrity are checked; its head is then compared with this Store's own
//! chain in a checked transaction. A backup is evidence of a restore point,
//! never a qualified restore procedure or a permission.
use super::*;
use crate::{
    audit::broker_registry::owned_supervision::OwnedRetainedSourceFacts,
    broker_registry::BrokerViewV1,
    control_fs::{Directory, PrivateEntryIdentity},
    human_authority::{HumanAuthorityFailure, HumanSourcesWriteEpoch, OwnedSourceToken},
};
use anyhow::Context;
use rusqlite::{OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

/// Maximum binding revisions returned by one history page.
const MAX_HISTORY_PAGE: u32 = 100;
/// SQLite companions that must be absent next to a backup main file.
const BACKUP_SIDECARS: [&str; 3] = ["-wal", "-journal", "-shm"];
const BACKUP_READ_CHUNK: usize = 64 * 1024;

/// Closed step of an owned broker binding issuance. The step is selected by
/// the trusted maintenance plan, never by the stored state, and is checked
/// against the current binding only for a new (non-replayed) operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnedBrokerBindingStepV1 {
    /// First binding of a fresh credential named by the current grant.
    Issue,
    /// Same credential and subject with a new digest: a new bearer replaces
    /// the current, unexpired one and the binding revision advances. Expiry
    /// is never extended in place; a later grant is a new enrollment.
    Rotate,
}

/// Verified head of an operator-supplied backup of this tenant Store. Only
/// `Store::owned_inspect_backup` builds it; it proves no restore procedure.
pub(crate) struct OwnedBackupInspectionV1 {
    head_seq: i64,
    head_hash: String,
}
impl OwnedBackupInspectionV1 {
    pub(crate) fn head_seq(&self) -> i64 {
        self.head_seq
    }
    pub(crate) fn head_hash(&self) -> &str {
        &self.head_hash
    }
}

/// One checked observation of this Store's audit chain: its head, the
/// retained owner-incarnation receipt (the scoped transaction has verified
/// that it belongs to this owner) and the hash of the event at the requested
/// sequence, `None` when no such event exists.
pub(crate) struct OwnedChainPositionV1 {
    pub(crate) head_seq: i64,
    pub(crate) head_hash: String,
    pub(crate) incarnation: Option<(i64, String)>,
    pub(crate) hash_at: Option<String>,
}

/// Streaming SHA-256 of a private backup file admitted by metadata first: the
/// descriptor keeps that identity, a single link, its size and its times
/// while it is read, and the name still binds the same inode afterwards.
fn backup_digest(
    parent: &Directory,
    name: &OsStr,
    identity: PrivateEntryIdentity,
) -> anyhow::Result<String> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(parent.path().join(name))?;
    let before = file.metadata()?;
    anyhow::ensure!(
        before.is_file()
            && before.nlink() == 1
            && (before.dev(), before.ino()) == identity.physical_identity(),
        "backup identity changed before reading"
    );
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; BACKUP_READ_CHUNK];
    loop {
        let count = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        digest.update(&buffer[..count]);
    }
    let after = file.metadata()?;
    anyhow::ensure!(
        after.len() == before.len()
            && after.mtime() == before.mtime()
            && after.mtime_nsec() == before.mtime_nsec()
            && after.ctime() == before.ctime()
            && after.ctime_nsec() == before.ctime_nsec(),
        "backup changed while it was read"
    );
    anyhow::ensure!(
        parent.private_entry(name)? == Some(identity),
        "backup name changed while it was read"
    );
    Ok(hex::encode(digest.finalize()))
}

/// Read-only, immutable SQLite URI of an absolute UTF-8 path: no lock, no
/// journal, no shared memory and no write on the backup or its directory.
fn immutable_uri(path: &Path) -> anyhow::Result<String> {
    let text = path.to_str().context("backup path must be UTF-8")?;
    let mut uri = String::from("file:");
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri.push_str("?immutable=1");
    Ok(uri)
}

/// One retained binding revision, without its digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OwnedBrokerBindingRevisionV1 {
    pub(crate) credential_id: String,
    pub(crate) revision: u64,
    pub(crate) broker_id: String,
    pub(crate) incarnation_id: String,
    pub(crate) expires_at: String,
    pub(crate) revoked: bool,
    pub(crate) updated_at: String,
    pub(crate) audit_seq: i64,
}

/// Newest-first page of one credential's history. `has_more` means older
/// revisions exist; an empty page is not evidence that none ever existed
/// before `before_revision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OwnedBrokerBindingHistoryV1 {
    pub(crate) items: Vec<OwnedBrokerBindingRevisionV1>,
    pub(crate) has_more: bool,
}

/// Registry grant and current bindings of one broker's current incarnation.
/// `broker: None` is a complete durable absence in this tenant Store.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct OwnedBrokerIdentityStatusV1 {
    pub(crate) evaluated_at: String,
    pub(crate) broker: Option<BrokerViewV1>,
    pub(crate) bindings: Vec<OwnedBrokerBindingRevisionV1>,
}

fn binding_revision(
    binding: WorkloadBinding,
    audit_seq: i64,
) -> anyhow::Result<OwnedBrokerBindingRevisionV1> {
    let WorkloadSubjectV1::Broker {
        broker_id,
        incarnation_id,
    } = binding.subject
    else {
        return Err(WorkloadFailureV1::InvalidRequest.into());
    };
    Ok(OwnedBrokerBindingRevisionV1 {
        credential_id: binding.credential_id,
        revision: binding.revision,
        broker_id,
        incarnation_id,
        expires_at: binding.expires_at,
        revoked: binding.revoked,
        updated_at: binding.updated_at,
        audit_seq,
    })
}

/// The binding follows a committed, unrevoked and unexpired grant naming the
/// same credential and incarnation.
fn require_grant(
    db: &Connection,
    credential_id: &str,
    broker_id: &str,
    incarnation_id: &str,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    let broker = load_broker(db, broker_id)?
        .ok_or(WorkloadFailureV1::Conflict)?;
    workload_conflict(
        !broker.revoked
            && broker.credential_id == credential_id
            && broker.incarnation_id == incarnation_id
            && now < parse_time(&broker.not_after)?,
    )
}

/// Preconditions of a new issuance step. The shared projection still applies
/// its revision, subject, digest-ownership and subject-count rules after it.
fn issue_precondition(
    db: &Connection,
    step: OwnedBrokerBindingStepV1,
    input: &WorkloadBindingIssueV1,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    let WorkloadSubjectV1::Broker {
        broker_id,
        incarnation_id,
    } = &input.subject
    else {
        return Err(WorkloadFailureV1::InvalidRequest.into());
    };
    require_grant(db, &input.credential_id, broker_id, incarnation_id, now)?;
    let current = load_binding(db, &input.credential_id)?;
    match (step, current) {
        (OwnedBrokerBindingStepV1::Issue, None) => workload_conflict(input.expected_revision == 0),
        (OwnedBrokerBindingStepV1::Issue, Some(_)) | (OwnedBrokerBindingStepV1::Rotate, None) => {
            Err(WorkloadFailureV1::Conflict.into())
        }
        (OwnedBrokerBindingStepV1::Rotate, Some(current)) => workload_conflict(
            !current.revoked
                && current.subject == input.subject
                && current.revision == input.expected_revision
                && current.sha256_digest != input.sha256_digest
                && now < parse_time(&current.expires_at)?,
        ),
    }
}

/// A revocation targets an unrevoked binding of the planned broker at its
/// exact revision: a credential of another broker or a worker is refused even
/// when its operation was planned.
fn revoke_precondition(
    db: &Connection,
    broker_id: &str,
    input: &WorkloadBindingRevocationV1,
) -> anyhow::Result<()> {
    let current = load_binding(db, &input.credential_id)?
        .ok_or(WorkloadFailureV1::Conflict)?;
    let planned_broker = matches!(
        &current.subject,
        WorkloadSubjectV1::Broker { broker_id: subject, .. } if subject == broker_id
    );
    workload_conflict(
        planned_broker && !current.revoked && current.revision == input.expected_revision,
    )
}

impl Store {
    /// The shared scoped workload authentication body for a presented bearer
    /// (syntax, format guard, binding, expiry, grant, incarnation and control
    /// time floor), inside the caller's owned checked transaction and with
    /// its control clock, so that an online owned job authenticates,
    /// revalidates and works in one transaction. The caller restricts the
    /// class to a broker and alone commits.
    pub(in crate::audit) fn owned_authenticate_workload_checked(
        &self,
        tx: &CheckedTransaction<'_>,
        clock: &AuthorizedControlTimeV1,
        token: &str,
    ) -> anyhow::Result<Option<AuthenticatedWorkload>> {
        workload_authorized(
            (32..=512).contains(&token.len()) && !token.chars().any(char::is_whitespace),
        )?;
        require_checked_schema(tx)?;
        let digest = hex::encode(Sha256::digest(token.as_bytes()));
        self.authenticate_workload_digest_checked(tx, &digest, clock.at())
    }

    /// Owned broker binding issuance, renewal or rotation (digest only).
    pub(crate) fn owned_broker_binding_issue(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
        step: OwnedBrokerBindingStepV1,
        input: &WorkloadBindingIssueV1,
    ) -> anyhow::Result<WorkloadBindingReceiptV1> {
        input.validate()?;
        workload_valid(matches!(input.subject, WorkloadSubjectV1::Broker { .. }))?;
        self.owned_binding_write(
            epoch,
            source,
            Change::Issue {
                input: input.clone(),
            },
            |db, now| issue_precondition(db, step, input, now),
        )
    }

    /// Owned broker binding revocation of a binding of `broker_id`, the broker
    /// named by the trusted maintenance plan. History and digests are
    /// retained, so a revoked digest stays a collision and is never reissued.
    pub(crate) fn owned_broker_binding_revoke(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
        broker_id: &str,
        input: &WorkloadBindingRevocationV1,
    ) -> anyhow::Result<WorkloadBindingReceiptV1> {
        input.validate()?;
        crate::broker_registry::registry_uuid(broker_id)?;
        self.owned_binding_write(
            epoch,
            source,
            Change::Revoke {
                input: input.clone(),
            },
            |db, _| revoke_precondition(db, broker_id, input),
        )
    }

    fn owned_binding_write(
        &self,
        epoch: &HumanSourcesWriteEpoch,
        source: &OwnedSourceToken,
        change: Change,
        precondition: impl FnOnce(&CheckedTransaction<'_>, DateTime<Utc>) -> anyhow::Result<()>,
    ) -> anyhow::Result<WorkloadBindingReceiptV1> {
        let facts = OwnedRetainedSourceFacts::from_token(source)?;
        change.validate()?;
        let value = serde_json::to_value(&change)?;
        workload_valid(self.redactor.value(&value) == value)?;
        let access = HumanSourceAccess::write(epoch, source)?;
        // Never wait on the control mutex while retaining a write epoch.
        let mut db = self
            .db
            .try_lock()
            .map_err(|_| HumanAuthorityFailure::StorageUnavailable)?;
        let mut tx =
            self.begin_checked_scoped(&mut db, &access, StoreOperationClass::IdentityAuthority)?;
        // Class and epoch are proven before the replay/idempotence lookup.
        tx.require_identity_authority()?;
        self.owned_source(&tx, &facts)?;
        require_checked_schema(&tx)?;
        let clock = capture_control_time(&tx)?;
        // An exact replay returns its original receipt from the shared body;
        // divergent content for the same operation is a conflict there.
        if replay(&tx, &change)?.is_none() {
            precondition(&tx, clock.at())?;
        }
        let receipt = self.persist_workload_checked(&mut tx, &clock, change)?;
        self.commit_authorized(tx, &clock)?;
        Ok(receipt)
    }

    /// Newest-first history of one broker credential, digests excluded.
    pub(crate) fn owned_broker_binding_history(
        &self,
        access: &HumanSourceAccess<'_>,
        facts: &OwnedRetainedSourceFacts,
        credential_id: &str,
        before_revision: Option<u64>,
        limit: u32,
    ) -> anyhow::Result<OwnedBrokerBindingHistoryV1> {
        crate::broker_registry::registry_uuid(credential_id)?;
        workload_valid((1..=MAX_HISTORY_PAGE).contains(&limit))?;
        let before = match before_revision {
            Some(revision) => {
                workload_valid(revision > 0)?;
                i64::try_from(revision)?
            }
            None => i64::MAX,
        };
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?;
        self.owned_source(&tx, facts)?;
        require_checked_schema(&tx)?;
        let rows = {
            let mut statement = tx.prepare("SELECT payload_json,event_seq FROM workload_binding_history WHERE credential_id=?1 AND revision<?2 ORDER BY revision DESC LIMIT ?3")?;
            statement
                .query_map(
                    params![credential_id, before, i64::from(limit) + 1],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let has_more = rows.len() > usize::try_from(limit)?;
        let items = rows
            .into_iter()
            .take(usize::try_from(limit)?)
            .map(|(json, seq)| {
                let binding: WorkloadBinding = serde_json::from_str(&json)?;
                workload_valid(binding.credential_id == credential_id)?;
                binding_revision(binding, seq)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        self.commit_checked(tx)?;
        Ok(OwnedBrokerBindingHistoryV1 { items, has_more })
    }

    /// Registry grant and current bindings of the broker's current
    /// incarnation, evaluated at the control time of this transaction.
    pub(crate) fn owned_broker_identity_status(
        &self,
        access: &HumanSourceAccess<'_>,
        facts: &OwnedRetainedSourceFacts,
        broker_id: &str,
    ) -> anyhow::Result<OwnedBrokerIdentityStatusV1> {
        crate::broker_registry::registry_uuid(broker_id)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?;
        self.owned_source(&tx, facts)?;
        require_checked_schema(&tx)?;
        let clock = capture_control_time(&tx)?;
        let evaluated_at = format_time(clock.at());
        let broker = load_broker(&tx, broker_id)?;
        let bindings = match &broker {
            None => Vec::new(),
            Some(broker) => {
                let subject = WorkloadSubjectV1::Broker {
                    broker_id: broker.broker_id.clone(),
                    incarnation_id: broker.incarnation_id.clone(),
                };
                let mut statement = tx.prepare("SELECT payload_json,event_seq FROM workload_bindings WHERE subject_key=?1 ORDER BY credential_id LIMIT 65")?;
                let rows = statement
                    .query_map([subject.key()?], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                // The projection admits at most 64 bindings per subject.
                workload_valid(rows.len() <= 64)?;
                rows.into_iter()
                    .map(|(json, seq)| {
                        let binding: WorkloadBinding = serde_json::from_str(&json)?;
                        workload_valid(binding.subject == subject)?;
                        binding_revision(binding, seq)
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?
            }
        };
        let broker = broker
            .map(|broker| broker.view_at(&evaluated_at))
            .transpose()?;
        self.commit_authorized(tx, &clock)?;
        Ok(OwnedBrokerIdentityStatusV1 {
            evaluated_at,
            broker,
            bindings,
        })
    }

    /// Verify an operator-supplied backup of this tenant Store without taking
    /// this Store's lock or any epoch (the caller holds none): private regular
    /// single-link file in a directory other than the source's, no SQLite
    /// sidecar, the operator's SHA-256 before and after, then a read-only
    /// immutable connection that checks format, schema, tenant binding and
    /// quota, namespace, chain integrity and `quick_check`. The returned head
    /// is compared with the live chain by `owned_chain_position` separately.
    pub(crate) fn owned_inspect_backup(
        &self,
        facts: &OwnedRetainedSourceFacts,
        path: &Path,
        expected_sha256: &str,
    ) -> anyhow::Result<OwnedBackupInspectionV1> {
        crate::control::hash(expected_sha256)?;
        workload_valid(facts.tenant_id() == self.tenant_id)?;
        let path = crate::control_fs::absolute_path(path)?;
        let name = path.file_name().context("backup file name required")?;
        let parent = Directory::open_existing(path.parent().context("backup parent required")?)?;
        let identity = parent.private_entry(name)?.context("backup file absent")?;
        let (source_path, source_identity) = self.backing.identity_key();
        let source_parent =
            Directory::open_existing(source_path.parent().context("source parent required")?)?;
        // Never open a descriptor on a retained SQLite inode: closing it could
        // release this process's POSIX locks. The source directory, where the
        // SQLite companion names live, is refused as a backup location too.
        anyhow::ensure!(
            identity != source_identity
                && parent.physical_identity()? != source_parent.physical_identity()?,
            "backup aliases its source"
        );
        for suffix in BACKUP_SIDECARS {
            let mut sidecar = name.to_owned();
            sidecar.push(suffix);
            anyhow::ensure!(
                parent.private_entry(&sidecar)?.is_none(),
                "backup sidecar present"
            );
        }
        anyhow::ensure!(
            backup_digest(&parent, name, identity)? == expected_sha256,
            "backup digest differs"
        );
        let db = Connection::open_with_flags(
            immutable_uri(&path)?,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        db.busy_timeout(std::time::Duration::ZERO)?;
        let format: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        workload_valid(u32::try_from(format).ok() == Some(facts.format().number()))?;
        validate_schema(&db, false)?;
        Store::validate_metadata(&db, &self.tenant_id)?;
        let namespace: String = db.query_row(
            "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        workload_valid(
            uuid::Uuid::parse_str(&namespace).is_ok_and(|id| id == facts.namespace_id()),
        )?;
        workload_valid(Store::check_integrity(&db)?.valid)?;
        let quick: String = db.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        workload_valid(quick == "ok")?;
        let (head_seq, head_hash) = audit_head(&db)?;
        db.close().map_err(|(_, error)| error)?;
        anyhow::ensure!(
            backup_digest(&parent, name, identity)? == expected_sha256,
            "backup changed during verification"
        );
        workload_valid(head_seq > 0)?;
        Ok(OwnedBackupInspectionV1 {
            head_seq,
            head_hash,
        })
    }

    /// Chain observation of this retained source in one checked transaction:
    /// head, current owner-incarnation receipt and the hash at `seq`. It lets
    /// the maintenance prove that a backup is exactly the state before this
    /// session's incarnation record, that it is still a prefix on resume, and
    /// that a locally recorded receipt is still durable (not lost by a
    /// restore) before its operation is replayed.
    pub(crate) fn owned_chain_position(
        &self,
        access: &HumanSourceAccess<'_>,
        facts: &OwnedRetainedSourceFacts,
        seq: i64,
    ) -> anyhow::Result<OwnedChainPositionV1> {
        workload_valid(seq > 0)?;
        let mut db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("audit lock poisoned"))?;
        let tx = self.begin_checked_scoped(&mut db, access, StoreOperationClass::Business)?;
        self.owned_source(&tx, facts)?;
        let (head_seq, head_hash) = audit_head(&tx)?;
        let incarnation = crate::audit::review_authority::replay(&tx)?
            .incarnation_receipt()
            .map(|receipt| (receipt.seq, receipt.hash.clone()));
        let hash_at = tx
            .query_row("SELECT hash FROM events WHERE seq=?1", [seq], |row| {
                row.get::<_, String>(0)
            })
            .optional()?;
        self.commit_checked(tx)?;
        Ok(OwnedChainPositionV1 {
            head_seq,
            head_hash,
            incarnation,
            hash_at,
        })
    }
}
