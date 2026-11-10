//! Production of signed records from an existing audit database.
//!
//! The database is read on one consistent read-only snapshot and is never
//! rewritten. The first checkpoint of an existing history is a baseline: it
//! protects nothing that happened before it, and starting a log is an
//! explicit request.
//!
//! A checkpoint is never signed over a database that is shorter than, or
//! different from, the position signed by the head of the log *as found on
//! disk*. That is a guard against accidents, not a trust decision: nothing
//! here consults the trust anchor. Whoever can write both the database and the
//! checkpoint directory can remove the last records, rewrite what they
//! covered, and obtain a new signature from the next scheduled run without
//! ever reading the key. Only a verification against an anchor reference
//! bounds that.
//!
//! A record is appended only to a log whose complete records are the chain
//! that its head commits to: under the writer lock, `create` and `rotate-key`
//! read every record of the directory and refuse a fork, a record that the
//! head does not chain to, and a record below a log index that holds none.
//! This too reads the directory as found: it tells nothing about a head that
//! was replaced together with everything below it.
//!
//! A checkpoint is never signed either over a database whose control genesis
//! is not the one that the log already signed: that genesis is written once
//! and names the origin of the control projections. The most recent
//! checkpoint at or below the head names it; when only a rotation is at hand
//! (`--previous-record` after a rotation), what remains checkable is that the
//! genesis of the database does not stand after the last signed position.

use super::{
    CHAIN_ALGORITHM, MAX_LOG_RECORDS, MAX_RECORD_BYTES, ROTATION_RESERVE_RECORDS, STATEMENT_TYPE,
    STATEMENT_VERSION,
    evidence::{AuditEvidence, read_audit_evidence},
    files,
    format::{
        CheckpointStatement, ENDORSEMENT_NONE, ENDORSEMENT_RETIRING_KEY, KIND_CHECKPOINT,
        KIND_KEY_ROTATION, Position, RotationStatement, SignedGenesis, SignedRecord, Statement,
        ZERO_HASH, now,
    },
    keys::{self, SigningKey},
    log::{CheckpointLog, IncompleteRecord, LogHead, database_identity, refuse_renamed_records},
    report::{CONTROL_GENESIS_ACTION, Code, Failure},
};
use crate::control_fs::PrivateEntryIdentity;
use serde::Serialize;
use std::path::{Path, PathBuf};

pub struct CreateRequest<'a> {
    pub data: &'a Path,
    pub tenant_id: &'a str,
    pub key_dir: &'a Path,
    pub checkpoint_dir: &'a Path,
    /// Start a checkpoint log. Required, and only accepted, while the
    /// directory holds no complete record: the first record of an existing
    /// history is a baseline, and a mistyped directory must not start a second
    /// log silently while the real one stops growing.
    pub new_log: bool,
    /// Last record of a previous, full checkpoint directory. Starts the new
    /// directory like `new_log`, linked to that record.
    pub previous_record: Option<&'a Path>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CreateStatus {
    /// A new record was signed and made durable.
    Created,
    /// The audit head is the position already signed by the last record; no
    /// file was written.
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateOutcome {
    pub status: CreateStatus,
    /// True for log index 1: events before this checkpoint receive no
    /// retroactive protection.
    pub baseline: bool,
    pub tenant_id: String,
    pub namespace_uuid: String,
    pub log_index: u64,
    pub statement_sha256: String,
    pub event_count: u64,
    pub head_hash: String,
    pub signing_key_id: String,
    pub record: String,
    /// Incomplete record files found at or above the head of the log. They
    /// are kept and ignored.
    pub incomplete_records: Vec<IncompleteRecord>,
}

/// Where the next record links to.
struct Link {
    log_index: u64,
    previous_statement_sha256: String,
    previous: Position,
}

impl Link {
    fn after(record: &SignedRecord) -> Self {
        Self {
            log_index: record.log_index() + 1,
            previous_statement_sha256: record.statement_sha256().to_owned(),
            previous: record.statement().signed_position(),
        }
    }
}

/// Refuse directories that would mix records or keys with SQLite files.
pub(crate) fn separate_from_database(
    database_directory: &Path,
    directory: &Path,
    argument: &str,
) -> Result<(), Failure> {
    let directory = files::absolute(directory)?;
    if directory == database_directory {
        return Err(Failure::new(
            Code::InvalidArgument,
            "checkpoint records and keys must not share the directory of the audit database",
        )
        .with("argument", argument));
    }
    Ok(())
}

/// Refuse a key directory that is, or lies inside, the checkpoint directory.
/// The checkpoint directory is evidence that is copied, exported and backed
/// up; the private key must never travel with it.
pub(crate) fn key_outside_records(
    key_directory: &Path,
    argument: &str,
    checkpoint_directory: &Path,
) -> Result<(), Failure> {
    let key_directory = files::absolute(key_directory)?;
    let checkpoint_directory = files::absolute(checkpoint_directory)?;
    if key_directory.starts_with(&checkpoint_directory) {
        return Err(Failure::new(
            Code::InvalidArgument,
            "a signing key must not be kept in the checkpoint directory or under it",
        )
        .with("argument", argument));
    }
    Ok(())
}

/// The record that the current key may extend: a checkpoint it signed, or a
/// rotation that names it as successor.
pub(crate) fn require_extendable(record: &SignedRecord, key: &SigningKey) -> Result<(), Failure> {
    let expected = record.statement().next_signer();
    if expected != key.key_id() {
        return Err(Failure::new(
            Code::KeyIdMismatch,
            "the signing key is not the key expected by the last record of the log",
        )
        .with("between", vec!["checkpoint_log_head", "key_dir"])
        .with("expected", expected)
        .with("actual", key.key_id())
        .action(
            "Use the key that signed the last record, or record a rotation to the new key with rotate-key.",
        ));
    }
    let verified = record
        .signatures()
        .iter()
        .find(|signature| signature.key_id == key.key_id())
        .is_some_and(|signature| {
            keys::verify_signature(
                key.public_key(),
                record.statement_bytes(),
                &signature.signature,
            )
        });
    if !verified {
        return Err(Failure::new(
            Code::SignatureInvalid,
            "the last record of the log does not verify with the signing key",
        )
        .with("log_index", record.log_index()));
    }
    Ok(())
}

fn scope_matches(record: &SignedRecord, evidence: &AuditEvidence) -> Result<(), Failure> {
    let statement = record.statement();
    for (field, logged, actual) in [
        (
            "tenant_id",
            statement.tenant_id(),
            evidence.tenant_id.as_str(),
        ),
        (
            "namespace_uuid",
            statement.namespace_uuid(),
            evidence.namespace_uuid.as_str(),
        ),
    ] {
        if logged != actual {
            return Err(Failure::new(
                Code::ScopeMismatch,
                "the checkpoint log belongs to another tenant or control namespace than the audit database",
            )
            .with("field", field)
            .with("between", vec!["checkpoint_log", "database"])
            .with("expected", logged)
            .with("actual", actual));
        }
    }
    Ok(())
}

/// The audit database must still hold the last signed position.
fn extends_signed_position(previous: &Position, evidence: &AuditEvidence) -> Result<(), Failure> {
    if previous.event_count == 0 {
        return Ok(());
    }
    if previous.event_count > evidence.event_count {
        return Err(Failure::new(
            Code::DatabaseTruncated,
            "the audit database holds fewer events than the last signed position; nothing was signed",
        )
        .with("signed_event_count", previous.event_count)
        .with("database_event_count", evidence.event_count));
    }
    let actual = evidence.hashes.get(&previous.event_count);
    if actual != Some(&previous.head_hash) {
        return Err(Failure::new(
            Code::DatabaseDiverged,
            "the audit database does not hold the signed chain hash at the last signed position; nothing was signed",
        )
        .with("event_count", previous.event_count)
        .with("signed_head_hash", previous.head_hash.clone())
        .with("database_hash_at_position", actual.cloned()));
    }
    Ok(())
}

/// The audit database must still have the control genesis that the log
/// signed: the one named by the most recent checkpoint at or below the record
/// that the new one extends, and in every case one that does not stand after
/// the last signed position.
fn same_control_genesis(
    signed: Option<&SignedGenesis>,
    previous: &Position,
    evidence: &AuditEvidence,
) -> Result<(), Failure> {
    let refusal = |message: &str| {
        Failure::new(Code::ScopeMismatch, message)
            .with("field", "control_genesis")
            .with("between", vec!["checkpoint_log", "database"])
            .with("database_control_genesis_seq", evidence.control_genesis_seq)
            .with(
                "database_control_genesis_hash",
                evidence.control_genesis_hash.clone(),
            )
            .action(CONTROL_GENESIS_ACTION)
    };
    if let Some(signed) = signed
        && !signed.is(evidence.control_genesis_seq, &evidence.control_genesis_hash)
    {
        return Err(refusal(
            "the audit database has another control genesis than the one named by the last checkpoint of the log; nothing was signed",
        )
        .with("signed_at_log_index", signed.log_index)
        .with("signed_control_genesis_seq", signed.sequence)
        .with("signed_control_genesis_hash", signed.hash.clone()));
    }
    if previous.event_count >= 1 && evidence.control_genesis_seq > previous.event_count {
        return Err(refusal(
            "the control genesis of the audit database stands after the last signed position; nothing was signed",
        )
        .with("signed_event_count", previous.event_count));
    }
    Ok(())
}

/// Read the last record of a previous checkpoint directory.
///
/// The file must carry the name of the record it holds, as every record file
/// of a checkpoint directory does. A file named after one record that holds
/// another one was altered or replaced, exactly as in a directory, and
/// continuing after it would silently drop the records in between; a name
/// that announces no record cannot be checked against anything. Both are
/// refused here, before anything is created.
fn read_previous_record(path: &Path) -> Result<SignedRecord, Failure> {
    let bytes = files::read_input(
        path,
        MAX_RECORD_BYTES,
        Code::InputUnavailable,
        "previous_record",
    )?;
    let record =
        SignedRecord::parse(&bytes).map_err(|failure| failure.with("input", "previous_record"))?;
    let expected = record.file_name();
    let actual = path.file_name().map(|name| name.to_string_lossy());
    if actual.as_deref() != Some(expected.as_str()) {
        return Err(Failure::new(
            Code::RecordNameMismatch,
            "the previous record file is not named after the record it holds; nothing was created",
        )
        .with("input", "previous_record")
        .with("count", 1)
        .with(
            "records",
            actual
                .map(|name| vec![name.into_owned()])
                .unwrap_or_default(),
        )
        .with("expected_name", expected)
        .action(
            "Pass the last record of the previous checkpoint directory under its own file name, as that directory holds it. If the file in that directory holds another record than the one its name announces, it was altered or replaced: preserve the directory, compare it with an independent copy or export, and investigate.",
        ));
    }
    Ok(record)
}

/// The record a new checkpoint extends, and where it was read from.
struct Predecessor {
    record: SignedRecord,
    path: PathBuf,
    /// Control genesis named by the most recent checkpoint at or below the
    /// record, when a record at hand names one.
    signed_genesis: Option<SignedGenesis>,
}

fn link_to(predecessor: Option<&Predecessor>, key: &SigningKey) -> Result<Link, Failure> {
    match predecessor {
        Some(predecessor) => {
            require_extendable(&predecessor.record, key)?;
            Ok(Link::after(&predecessor.record))
        }
        None => Ok(Link {
            log_index: 1,
            previous_statement_sha256: ZERO_HASH.into(),
            previous: Position::empty(),
        }),
    }
}

/// Read the audit evidence for the next record, and refuse a database that
/// belongs to another scope than the log, no longer holds the position its
/// head signed, or has another control genesis than the one the log signed.
fn evidence_extending(
    request: &CreateRequest<'_>,
    predecessor: Option<&Predecessor>,
    link: &Link,
) -> Result<AuditEvidence, Failure> {
    let evidence = read_audit_evidence(
        request.data,
        request.tenant_id,
        &[link.previous.event_count],
    )?;
    if let Some(predecessor) = predecessor {
        scope_matches(&predecessor.record, &evidence)?;
    }
    extends_signed_position(&link.previous, &evidence)?;
    if let Some(predecessor) = predecessor {
        same_control_genesis(
            predecessor.signed_genesis.as_ref(),
            &link.previous,
            &evidence,
        )?;
    }
    Ok(evidence)
}

/// The outcome when the audit head is the position that the predecessor
/// already signed with this key: nothing is written.
fn unchanged(
    predecessor: Option<&Predecessor>,
    evidence: &AuditEvidence,
    key: &SigningKey,
    incomplete: &[IncompleteRecord],
) -> Option<CreateOutcome> {
    let predecessor = predecessor?;
    let Statement::Checkpoint(statement) = predecessor.record.statement() else {
        return None;
    };
    (statement.signing_key_id == key.key_id()
        && statement.event_count == evidence.event_count
        && statement.head_hash == evidence.head_hash)
        .then(|| CreateOutcome {
            status: CreateStatus::Unchanged,
            baseline: predecessor.record.log_index() == 1,
            tenant_id: evidence.tenant_id.clone(),
            namespace_uuid: evidence.namespace_uuid.clone(),
            log_index: predecessor.record.log_index(),
            statement_sha256: predecessor.record.statement_sha256().to_owned(),
            event_count: evidence.event_count,
            head_hash: evidence.head_hash.clone(),
            signing_key_id: key.key_id().to_owned(),
            record: predecessor.path.display().to_string(),
            incomplete_records: incomplete.to_vec(),
        })
}

/// What the head scan of an open log found beside its head record.
struct Scanned {
    incomplete: Vec<IncompleteRecord>,
    record_files: usize,
}

/// Refuse a directory that holds the maximum number of record files.
fn require_room(record_files: usize, predecessor: Option<&Predecessor>) -> Result<(), Failure> {
    if record_files < MAX_LOG_RECORDS as usize {
        return Ok(());
    }
    let failure = Failure::new(
        Code::CheckpointLogFull,
        "the checkpoint directory holds the maximum number of record files",
    )
    .with("record_files", record_files)
    .with("limit", MAX_LOG_RECORDS);
    Err(match predecessor {
        Some(predecessor) => failure.with("last_record", predecessor.path.display().to_string()),
        None => failure,
    })
}

/// Sign the evidence as the next record of the log and make it durable.
fn sign_next(
    log: &CheckpointLog,
    scanned: Scanned,
    predecessor: Option<&Predecessor>,
    link: Link,
    evidence: AuditEvidence,
    key: &SigningKey,
) -> Result<CreateOutcome, Failure> {
    require_room(scanned.record_files, predecessor)?;
    let log_index = link.log_index;
    let statement = Statement::Checkpoint(CheckpointStatement {
        statement_type: STATEMENT_TYPE.into(),
        version: STATEMENT_VERSION,
        kind: KIND_CHECKPOINT.into(),
        tenant_id: evidence.tenant_id.clone(),
        namespace_uuid: evidence.namespace_uuid.clone(),
        log_index,
        previous_statement_sha256: link.previous_statement_sha256,
        created_at: now(),
        signing_key_id: key.key_id().to_owned(),
        audit_schema_version: evidence.audit_schema_version,
        chain_algorithm: CHAIN_ALGORITHM.into(),
        event_count: evidence.event_count,
        head_hash: evidence.head_hash.clone(),
        control_genesis_seq: evidence.control_genesis_seq,
        control_genesis_hash: evidence.control_genesis_hash.clone(),
        previous_event_count: link.previous.event_count,
        previous_head_hash: link.previous.head_hash,
    });
    let signature = key.sign(&statement.canonical_bytes()?);
    let record = SignedRecord::seal(statement, vec![signature])?;
    require_extendable(&record, key)?;
    let path = log.append(&record)?;
    Ok(CreateOutcome {
        status: CreateStatus::Created,
        baseline: log_index == 1,
        tenant_id: evidence.tenant_id,
        namespace_uuid: evidence.namespace_uuid,
        log_index,
        statement_sha256: record.statement_sha256().to_owned(),
        event_count: evidence.event_count,
        head_hash: evidence.head_hash,
        signing_key_id: key.key_id().to_owned(),
        record: path.display().to_string(),
        incomplete_records: scanned.incomplete,
    })
}

const START_HINT: &str = "To start a checkpoint log in this directory, pass --new-log, or --previous-record to continue a full directory.";

/// Refuse a directory in which a first record may not be written: one in
/// which a record file was altered, one that already holds a complete record
/// and one that has no room left for a record file.
fn startable(head: &LogHead, predecessor: Option<&Predecessor>) -> Result<(), Failure> {
    refuse_renamed_records(&head.incomplete)?;
    if head.record.is_some() {
        return Err(Failure::new(
            Code::InvalidArgument,
            "--new-log and --previous-record are only accepted while the checkpoint directory holds no complete record",
        ));
    }
    require_room(head.record_files, predecessor)
}

/// Write the first record of a checkpoint directory. Everything that can
/// refuse is read before the directory is created and, when it already
/// exists, before the writer lock is taken in it: a refused start leaves
/// nothing behind, not even the lock file. Only the write itself can still
/// fail under the lock.
fn start_log(
    request: &CreateRequest<'_>,
    key: &SigningKey,
    database: PrivateEntryIdentity,
) -> Result<CreateOutcome, Failure> {
    let predecessor = match request.previous_record {
        Some(path) => {
            let record = read_previous_record(path)?;
            Some(Predecessor {
                // Only this record of the previous directory is read: a
                // rotation names no control genesis.
                signed_genesis: SignedGenesis::of(&record),
                record,
                path: files::absolute(path)?,
            })
        }
        None => None,
    };
    let link = link_to(predecessor.as_ref(), key)?;
    let evidence = evidence_extending(request, predecessor.as_ref(), &link)?;
    if let Some(mut outcome) = unchanged(predecessor.as_ref(), &evidence, key, &[]) {
        if let Some(head) = CheckpointLog::starting_head_if_present(
            request.checkpoint_dir,
            vec![database],
            |head| startable(head, predecessor.as_ref()),
        )? {
            outcome.incomplete_records = head.incomplete;
        }
        return Ok(outcome);
    }
    let (log, head) =
        CheckpointLog::open_starting_writer(request.checkpoint_dir, vec![database], |head| {
            startable(head, predecessor.as_ref())
        })?;
    let scanned = Scanned {
        incomplete: head.incomplete,
        record_files: head.record_files,
    };
    sign_next(&log, scanned, predecessor.as_ref(), link, evidence, key)
}

/// Extend an existing log. A missing or empty directory is an error here, so
/// that a mistyped path never starts a second log, and that error creates
/// nothing in the directory.
fn extend_log(
    request: &CreateRequest<'_>,
    key: &SigningKey,
    database: PrivateEntryIdentity,
) -> Result<CreateOutcome, Failure> {
    let directory = files::absolute(request.checkpoint_dir)?;
    if matches!(
        std::fs::symlink_metadata(&directory),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    ) {
        return Err(Failure::new(
            Code::CheckpointLogUnavailable,
            "the checkpoint directory does not exist; nothing was created",
        )
        .with("checkpoint_dir", directory.display().to_string())
        .action(format!("Check the --checkpoint-dir path. {START_HINT}")));
    }
    let (log, head) =
        CheckpointLog::open_existing_writer(request.checkpoint_dir, vec![database], || {
            Failure::new(
                Code::CheckpointLogEmpty,
                "the checkpoint directory holds no complete record",
            )
            .action(START_HINT)
        })?;
    let predecessor = Predecessor {
        path: log.path().join(head.record.file_name()),
        record: head.record,
        signed_genesis: head.signed_genesis,
    };
    let link = link_to(Some(&predecessor), key)?;
    let evidence = evidence_extending(request, Some(&predecessor), &link)?;
    if let Some(outcome) = unchanged(Some(&predecessor), &evidence, key, &head.incomplete) {
        return Ok(outcome);
    }
    let scanned = Scanned {
        incomplete: head.incomplete,
        record_files: head.record_files,
    };
    sign_next(&log, scanned, Some(&predecessor), link, evidence, key)
}

/// Sign the current head of the audit chain and append the record to the log.
pub fn create_checkpoint(request: &CreateRequest<'_>) -> Result<CreateOutcome, Failure> {
    let (database_directory, database) = database_identity(request.data)?;
    separate_from_database(
        &database_directory,
        request.checkpoint_dir,
        "checkpoint_dir",
    )?;
    separate_from_database(&database_directory, request.key_dir, "key_dir")?;
    key_outside_records(request.key_dir, "key_dir", request.checkpoint_dir)?;
    let key = keys::load_signing_key(request.key_dir)?;
    if request.new_log || request.previous_record.is_some() {
        start_log(request, &key, database)
    } else {
        extend_log(request, &key, database)
    }
}

/// Which key hands the log over to its successor.
#[derive(Debug, Clone, Copy)]
pub enum RetiringKey<'a> {
    /// The retiring key is available and endorses its successor.
    Endorsing { key_dir: &'a Path },
    /// The retiring key is lost or compromised. The rotation carries no
    /// endorsement and needs an explicit recovery approval by the anchor
    /// holder.
    UnendorsedRecovery { retiring_key_id: &'a str },
}

pub struct RotateRequest<'a> {
    pub checkpoint_dir: &'a Path,
    /// Directory of the successor key, already created with `key init`.
    pub new_key_dir: &'a Path,
    pub retiring: RetiringKey<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RotateStatus {
    /// A rotation record was signed and made durable.
    Rotated,
    /// The last record of the log already is this rotation; nothing was
    /// written.
    AlreadyRotated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RotateOutcome {
    pub status: RotateStatus,
    pub log_index: u64,
    pub statement_sha256: String,
    pub retiring_key_id: String,
    pub successor_key_id: String,
    /// `retiring_key` or `none`.
    pub endorsement: String,
    pub record: String,
    pub incomplete_records: Vec<IncompleteRecord>,
}

/// Append a key rotation record. It changes nothing for a verifier until the
/// anchor holder approves the successor with `anchor approve-rotation`.
pub fn rotate_key(request: &RotateRequest<'_>) -> Result<RotateOutcome, Failure> {
    key_outside_records(request.new_key_dir, "new_key_dir", request.checkpoint_dir)?;
    let successor = keys::load_signing_key(request.new_key_dir)?;
    let (retiring, retiring_key_id) = match request.retiring {
        RetiringKey::Endorsing { key_dir } => {
            key_outside_records(key_dir, "key_dir", request.checkpoint_dir)?;
            let key = keys::load_signing_key(key_dir)?;
            let key_id = key.key_id().to_owned();
            (Some(key), key_id)
        }
        RetiringKey::UnendorsedRecovery { retiring_key_id } => {
            super::anchor::expected_key_id(retiring_key_id)?;
            (None, retiring_key_id.to_owned())
        }
    };
    if retiring_key_id == successor.key_id() {
        return Err(Failure::new(
            Code::InvalidArgument,
            "the successor key must differ from the retiring key",
        ));
    }
    let (log, head) =
        CheckpointLog::open_existing_writer(request.checkpoint_dir, Vec::new(), || {
            Failure::new(
                Code::CheckpointLogEmpty,
                "a key rotation needs a checkpoint log with at least one record",
            )
        })?;
    let record = head.record;
    let endorsement = if retiring.is_some() {
        ENDORSEMENT_RETIRING_KEY
    } else {
        ENDORSEMENT_NONE
    };
    // An interrupted command that already made its record durable resumes as
    // a no-op.
    if let Statement::KeyRotation(statement) = record.statement()
        && statement.successor_key_id == successor.key_id()
        && statement.retiring_key_id == retiring_key_id
        && statement.endorsement == endorsement
    {
        require_extendable(&record, &successor)?;
        return Ok(RotateOutcome {
            status: RotateStatus::AlreadyRotated,
            log_index: record.log_index(),
            statement_sha256: record.statement_sha256().to_owned(),
            retiring_key_id,
            successor_key_id: successor.key_id().to_owned(),
            endorsement: endorsement.to_owned(),
            record: log.path().join(record.file_name()).display().to_string(),
            incomplete_records: head.incomplete,
        });
    }
    // The last record must expect the retiring key as its next signer.
    match &retiring {
        Some(key) => require_extendable(&record, key)?,
        None => {
            let expected = record.statement().next_signer();
            if expected != retiring_key_id {
                return Err(Failure::new(
                    Code::KeyIdMismatch,
                    "the retiring key is not the key expected by the last record of the log",
                )
                .with("between", vec!["checkpoint_log_head", "retiring_key_id"])
                .with("expected", expected)
                .with("actual", retiring_key_id));
            }
        }
    }
    if head.record_files >= (MAX_LOG_RECORDS + ROTATION_RESERVE_RECORDS) as usize {
        return Err(Failure::new(
            Code::CheckpointLogFull,
            "the checkpoint directory cannot hold another rotation record",
        )
        .with("record_files", head.record_files));
    }
    let previous = record.statement().signed_position();
    let statement = Statement::KeyRotation(RotationStatement {
        statement_type: STATEMENT_TYPE.into(),
        version: STATEMENT_VERSION,
        kind: KIND_KEY_ROTATION.into(),
        tenant_id: record.statement().tenant_id().to_owned(),
        namespace_uuid: record.statement().namespace_uuid().to_owned(),
        log_index: record.log_index() + 1,
        previous_statement_sha256: record.statement_sha256().to_owned(),
        created_at: now(),
        retiring_key_id: retiring_key_id.clone(),
        successor_key_id: successor.key_id().to_owned(),
        endorsement: endorsement.to_owned(),
        previous_event_count: previous.event_count,
        previous_head_hash: previous.head_hash,
    });
    let bytes = statement.canonical_bytes()?;
    let mut signatures = Vec::with_capacity(2);
    if let Some(key) = &retiring {
        signatures.push(key.sign(&bytes));
    }
    signatures.push(successor.sign(&bytes));
    let rotation = SignedRecord::seal(statement, signatures)?;
    require_extendable(&rotation, &successor)?;
    let path = log.append(&rotation)?;
    Ok(RotateOutcome {
        status: RotateStatus::Rotated,
        log_index: rotation.log_index(),
        statement_sha256: rotation.statement_sha256().to_owned(),
        retiring_key_id,
        successor_key_id: successor.key_id().to_owned(),
        endorsement: endorsement.to_owned(),
        record: path.display().to_string(),
        incomplete_records: head.incomplete,
    })
}
