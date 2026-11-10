//! Offline verification of checkpoint records against a trust anchor.
//!
//! Trust comes only from the keys pinned in the anchor. A record names the key
//! it expects by fingerprint; the public key itself is always taken from the
//! anchor, never from the verified material. Rollback, truncation and
//! divergence are detected against the reference accepted in the anchor:
//! without a reference, the report says that a rollback to an earlier signed
//! state cannot be detected.
//!
//! No verdict covers a record that was not authenticated. A record after the
//! reference is verified by signature. A record at or below the reference is
//! not verified by signature again: it must be the record that the accepted
//! reference chains to through `previous_statement_sha256`, and a complete
//! record that no chain attaches to the reference is a refusal.
//!
//! A compared database must hold every trusted signed position: the ones the
//! anchor retains or references, and the one of every checkpoint verified
//! after the reference. Comparing the last position alone would let a holder
//! of the signing key rewrite the history under the reference and sign a new
//! head that merely recalls it.
//!
//! Every checkpoint also names the control genesis of the database it was
//! signed over. That genesis never changes: a checkpoint that names another
//! one than the trusted checkpoint before it is refused, and a compared
//! database must have the genesis named by the most recent trusted
//! checkpoint. The anchor carries no genesis; the one signed at or below the
//! reference is read from the reference chain when the set holds it. When no
//! record at hand names it, what remains checkable is that a genesis never
//! stands after a signed position.

use super::{
    VERIFICATION_FORMAT,
    anchor::{APPROVAL_OPERATOR_RECOVERY, AnchorReference, LoadedAnchor, TrustAnchor, load_anchor},
    bundle::read_bundle,
    evidence::read_audit_evidence,
    files,
    format::{Position, SignedGenesis, SignedRecord, Statement, ZERO_HASH, parse_record_file_name},
    keys,
    log::{ChainBreak, CheckpointLog, RecordSet, forked, refuse_renamed_records},
    report::{
        AnchorIdentity, CONTROL_GENESIS_ACTION, Code, DatabaseComparison, ErrorEntry, Failure,
        HeadSummary, ReferenceSummary, RollbackDetection, Scope, Verdict, VerificationReport,
        Warning,
    },
};
use std::path::Path;

/// Where the records to verify come from.
#[derive(Debug, Clone, Copy)]
pub enum RecordSource<'a> {
    /// The private checkpoint directory of the control.
    Directory(&'a Path),
    /// A public proof bundle produced by `export`.
    Bundle(&'a Path),
}

pub struct VerifyRequest<'a> {
    pub anchor: &'a Path,
    pub source: RecordSource<'a>,
    /// Audit database to compare with every trusted signed position.
    pub data: Option<&'a Path>,
    /// Refuse when the anchor carries no accepted reference yet.
    pub require_reference: bool,
    /// New file receiving the report, refused or not.
    pub report_out: Option<&'a Path>,
}

/// A signed audit position that a compared database must hold.
#[derive(Debug, Clone)]
pub(crate) struct TrustedPosition {
    /// Log index of the record that signed the position.
    pub log_index: u64,
    pub position: Position,
    /// Where the trust comes from: `anchor_retained`, `anchor_reference` or
    /// `checkpoint_record`.
    pub source: &'static str,
}

/// What the records establish, before any comparison with a database.
#[derive(Default)]
pub(crate) struct ChainOutcome {
    pub errors: Vec<ErrorEntry>,
    pub entries_checked: u64,
    pub signatures_verified: u64,
    /// Highest trusted log index: verified now or accepted by the reference.
    pub verified_through: Option<u64>,
    /// Statement hash of the record at `verified_through`.
    pub verified_sha256: Option<String>,
    pub head: Option<HeadSummary>,
    /// Last trusted signed audit position and the log index that signed it.
    pub position: Option<Position>,
    pub position_log_index: Option<u64>,
    /// Every trusted signed position, oldest first: the ones retained and
    /// referenced by the anchor, then the one of each checkpoint verified
    /// now. Bounded by the anchor and record bounds.
    pub positions: Vec<TrustedPosition>,
    /// Control genesis named by the most recent trusted checkpoint: the last
    /// one verified now, or else the most recent one at or below the anchor
    /// reference when the set holds it on the reference chain.
    pub genesis: Option<SignedGenesis>,
    /// The first key rotation found after the trusted position. It links
    /// correctly to the verified records, but its successor is not trusted
    /// until the anchor holder approves it.
    pub pending_rotation: Option<SignedRecord>,
}

fn scope_matches(anchor: &TrustAnchor, record: &SignedRecord) -> Result<(), Failure> {
    let statement = record.statement();
    for (field, expected, actual) in [
        (
            "tenant_id",
            anchor.tenant_id.as_str(),
            statement.tenant_id(),
        ),
        (
            "namespace_uuid",
            anchor.namespace_uuid.as_str(),
            statement.namespace_uuid(),
        ),
    ] {
        if expected != actual {
            return Err(Failure::new(
                Code::ScopeMismatch,
                "a checkpoint record belongs to another tenant or control namespace than the trust anchor",
            )
            .with("field", field)
            .with("between", vec!["trust_anchor", "checkpoint_record"])
            .with("expected", expected)
            .with("actual", actual)
            .with("log_index", record.log_index()));
        }
    }
    Ok(())
}

/// Verify the exact set of signatures required by the record kind, each with
/// the public key pinned in the anchor under the named fingerprint.
fn signatures_verify(anchor: &TrustAnchor, record: &SignedRecord) -> Result<u64, Failure> {
    let required = record.statement().required_signers();
    let signatures = record.signatures();
    let named: Vec<&str> = signatures
        .iter()
        .map(|signature| signature.key_id.as_str())
        .collect();
    if named != required {
        return Err(Failure::new(
            Code::SignatureInvalid,
            "the record does not carry exactly the signatures required by its statement",
        )
        .with("log_index", record.log_index())
        .with("required_signers", required)
        .with("actual_signers", named));
    }
    for signature in signatures {
        let key = anchor.key(&signature.key_id).ok_or_else(|| {
            Failure::new(
                Code::UntrustedKey,
                "the record is signed by a key that the trust anchor does not pin",
            )
            .with("log_index", record.log_index())
            .with("key_id", signature.key_id.clone())
            .with("record_kind", record.statement().kind())
        })?;
        if !key.valid_at(record.log_index()) {
            return Err(Failure::new(
                Code::KeyNotValidAtIndex,
                "the record is signed by a pinned key outside the log positions it may sign",
            )
            .with("log_index", record.log_index())
            .with("key_id", signature.key_id.clone())
            .with("valid_from_log_index", key.valid_from_log_index)
            .with("valid_through_log_index", key.valid_through_log_index));
        }
        let public_key = key.public_key_bytes().ok_or_else(|| {
            Failure::new(Code::AnchorInvalid, "a pinned public key is unreadable")
        })?;
        if !keys::verify_signature(&public_key, record.statement_bytes(), &signature.signature) {
            return Err(Failure::new(
                Code::SignatureInvalid,
                "a signature does not verify over the exact statement bytes with the pinned key",
            )
            .with("log_index", record.log_index())
            .with("key_id", signature.key_id.clone()));
        }
    }
    Ok(signatures.len() as u64)
}

pub(crate) fn head_summary(record: &SignedRecord) -> HeadSummary {
    let statement = record.statement();
    let position = statement.signed_position();
    HeadSummary {
        log_index: record.log_index(),
        kind: statement.kind().into(),
        statement_sha256: record.statement_sha256().into(),
        event_count: position.event_count,
        head_hash: position.head_hash,
        signing_key_id: statement.next_signer().into(),
        created_at_declared: statement.created_at().into(),
    }
}

/// The single complete record at `log_index`, or the refusal that explains
/// why there is none.
fn single_record(set: &RecordSet, log_index: u64) -> Result<&SignedRecord, Failure> {
    match set.at(log_index) {
        [record] => Ok(record),
        [] => Err(Failure::new(
            Code::LogDiscontinuous,
            "no complete checkpoint record is available at an expected log index",
        )
        .with("missing_log_index", log_index)),
        records => Err(forked(log_index, records)),
    }
}

/// Checks that do not depend on the kind of the record: scope, link to the
/// previous record and continuity of the signed audit position.
fn continues(
    anchor: &TrustAnchor,
    record: &SignedRecord,
    previous_sha256: &str,
    position: Option<&Position>,
) -> Result<(), Failure> {
    scope_matches(anchor, record)?;
    let statement = record.statement();
    if statement.previous_statement_sha256() != previous_sha256 {
        return Err(Failure::new(
            Code::LogDiscontinuous,
            "a checkpoint record does not link to the record that precedes it",
        )
        .with("log_index", record.log_index())
        .with("expected_previous_statement_sha256", previous_sha256)
        .with(
            "actual_previous_statement_sha256",
            statement.previous_statement_sha256(),
        ));
    }
    let expected = position.cloned().unwrap_or_else(Position::empty);
    let recalled = statement.previous_position();
    if recalled != expected {
        return Err(Failure::new(
            Code::CheckpointRegression,
            "a record does not recall the last signed audit position",
        )
        .with("log_index", record.log_index())
        .with("expected_previous_event_count", expected.event_count)
        .with("actual_previous_event_count", recalled.event_count));
    }
    Ok(())
}

fn control_genesis_mismatch(message: &str, between: [&'static str; 2]) -> Failure {
    Failure::new(Code::ScopeMismatch, message)
        .with("field", "control_genesis")
        .with("between", between.to_vec())
        .action(CONTROL_GENESIS_ACTION)
}

/// A checkpoint names the control genesis of the trusted checkpoint before
/// it. When the set holds no earlier checkpoint, the earlier genesis is
/// unknown: what remains is that it was signed at the last trusted position,
/// so that a genesis whose sequence exceeds that position was written later.
fn same_control_genesis(outcome: &ChainOutcome, record: &SignedRecord) -> Result<(), Failure> {
    let Some(named) = SignedGenesis::of(record) else {
        return Ok(());
    };
    let refusal = |message: &str, earlier: &'static str| {
        control_genesis_mismatch(message, ["checkpoint_record", earlier])
            .with("log_index", named.log_index)
            .with("control_genesis_seq", named.sequence)
            .with("control_genesis_hash", named.hash.clone())
    };
    match (&outcome.genesis, &outcome.position) {
        (Some(earlier), _) if !earlier.is(named.sequence, &named.hash) => Err(refusal(
            "a checkpoint names another control genesis than the trusted checkpoint before it",
            "earlier_checkpoint_record",
        )
        .with("earlier_log_index", earlier.log_index)
        .with("earlier_control_genesis_seq", earlier.sequence)
        .with("earlier_control_genesis_hash", earlier.hash.clone())),
        (None, Some(position)) if named.sequence > position.event_count => Err(refusal(
            "a checkpoint names a control genesis that stands after the last trusted signed position",
            "earlier_signed_position",
        )
        .with("earlier_signed_event_count", position.event_count)),
        _ => Ok(()),
    }
}

fn accept(outcome: &mut ChainOutcome, record: &SignedRecord, signatures: u64) {
    outcome.entries_checked += 1;
    outcome.signatures_verified += signatures;
    outcome.verified_through = Some(record.log_index());
    outcome.verified_sha256 = Some(record.statement_sha256().to_owned());
    if let Some(genesis) = SignedGenesis::of(record) {
        let position = record.statement().signed_position();
        outcome.positions.push(TrustedPosition {
            log_index: record.log_index(),
            position: position.clone(),
            source: "checkpoint_record",
        });
        outcome.position = Some(position);
        outcome.position_log_index = Some(record.log_index());
        outcome.genesis = Some(genesis);
    }
    outcome.head = Some(head_summary(record));
}

/// The refusal of a set whose complete records at or below the anchor
/// reference are not the chain that the accepted record commits to.
fn below_reference(reference: &AnchorReference, chain_break: ChainBreak<'_>) -> Failure {
    match chain_break {
        ChainBreak::Forked { log_index, records } => forked(log_index, records),
        ChainBreak::AnotherRecord {
            log_index,
            expected_statement_sha256,
            actual_statement_sha256,
        } => Failure::new(
            Code::LogDivergedFromReference,
            if log_index == reference.log_index {
                "the record at the anchor reference position is not the accepted record"
            } else {
                "a record below the anchor reference is not the record that the accepted record chains to"
            },
        )
        .with("log_index", log_index)
        .with("reference_log_index", reference.log_index)
        .with(
            "reference_statement_sha256",
            reference.statement_sha256.clone(),
        )
        .with("expected_statement_sha256", expected_statement_sha256)
        .with("actual_statement_sha256", actual_statement_sha256),
        ChainBreak::Unattached {
            missing_log_index,
            unattached_log_index,
        } => Failure::new(
            Code::LogDiscontinuous,
            "complete records stand below a log index that holds none, at or below the anchor reference: no chain attaches them to the accepted record",
        )
        .with("missing_log_index", missing_log_index)
        .with("unattached_log_index", unattached_log_index)
        .with("reference_log_index", reference.log_index),
    }
}

/// Authenticate the records at and below the anchor reference by the chain
/// that the reference commits to, then walk the records from the reference
/// (or from log index 1) to the head of the set. The walk stops at the first
/// refusal.
pub(crate) fn check_records(anchor: &TrustAnchor, set: &RecordSet) -> ChainOutcome {
    let mut outcome = ChainOutcome::default();
    if let Err(failure) = walk(anchor, set, &mut outcome) {
        outcome.errors.push(failure.error);
    }
    outcome
}

fn walk(anchor: &TrustAnchor, set: &RecordSet, outcome: &mut ChainOutcome) -> Result<(), Failure> {
    let head_index = set.head_index();
    let (first, mut previous_sha256) = match &anchor.reference {
        Some(reference) => {
            outcome.verified_through = Some(reference.log_index);
            outcome.verified_sha256 = Some(reference.statement_sha256.clone());
            for (pinned, source) in reference
                .retained_checkpoints
                .iter()
                .map(|retained| (retained, "anchor_retained"))
                .chain([(&reference.checkpoint, "anchor_reference")])
            {
                outcome.positions.push(TrustedPosition {
                    log_index: pinned.log_index,
                    position: Position {
                        event_count: pinned.event_count,
                        head_hash: pinned.head_hash.clone(),
                    },
                    source,
                });
            }
            outcome.position = Some(Position {
                event_count: reference.checkpoint.event_count,
                head_hash: reference.checkpoint.head_hash.clone(),
            });
            outcome.position_log_index = Some(reference.checkpoint.log_index);
            if head_index.is_none_or(|head| head < reference.log_index) {
                return Err(Failure::new(
                    Code::LogRolledBack,
                    "the checkpoint log ends before the record accepted by the trust anchor",
                )
                .with("reference_log_index", reference.log_index)
                .with("log_head_index", head_index));
            }
            // A continuation directory or a partial export may start after
            // the reference. Whatever the set holds at or below it must be
            // the chain that the accepted record commits to.
            let lowest = set
                .chain_below(reference.log_index, &reference.statement_sha256)
                .map_err(|chain_break| below_reference(reference, chain_break))?;
            if let [record] = set.at(reference.log_index) {
                outcome.head = Some(head_summary(record));
            }
            // The anchor carries no control genesis. The chain that the
            // reference commits to names it, when the set holds a checkpoint
            // of that chain.
            outcome.genesis =
                lowest.and_then(|lowest| set.last_signed_genesis(lowest, reference.log_index));
            (reference.log_index + 1, reference.statement_sha256.clone())
        }
        None => (1, ZERO_HASH.to_owned()),
    };
    let Some(head_index) = head_index.filter(|head| *head >= first) else {
        // Nothing beyond the reference; without a reference the first record
        // must exist.
        return match anchor.reference {
            Some(_) => Ok(()),
            None => single_record(set, first).map(|_| ()),
        };
    };
    for log_index in first..=head_index {
        let record = single_record(set, log_index)?;
        continues(anchor, record, &previous_sha256, outcome.position.as_ref())?;
        if let Statement::KeyRotation(statement) = record.statement() {
            // Approving a rotation pins it as the anchor reference, so a
            // rotation met beyond the reference is never approved yet. Its
            // endorsement proves nothing to a verifier: a compromised retiring
            // key could introduce any successor.
            outcome.pending_rotation = Some(record.clone());
            return Err(Failure::new(
                Code::UntrustedKey,
                "a key rotation is waiting for an explicit approval by the anchor holder",
            )
            .with("log_index", log_index)
            .with("key_id", statement.successor_key_id.clone())
            .with("record_kind", record.statement().kind()));
        }
        let signatures = signatures_verify(anchor, record)?;
        same_control_genesis(outcome, record)?;
        accept(outcome, record, signatures);
        previous_sha256 = record.statement_sha256().to_owned();
    }
    Ok(())
}

/// Compare the audit database with every trusted signed position, oldest
/// first, and stop at the first one it does not hold. Refusals are appended to
/// `errors`; only an unreadable database is an `Err`.
pub(crate) fn compare_database(
    anchor: &TrustAnchor,
    chain: &ChainOutcome,
    data: &Path,
    errors: &mut Vec<ErrorEntry>,
) -> Result<DatabaseComparison, Failure> {
    let mut wanted: Vec<u64> = chain
        .positions
        .iter()
        .map(|trusted| trusted.position.event_count)
        .collect();
    wanted.sort_unstable();
    wanted.dedup();
    let evidence = match read_audit_evidence(data, &anchor.tenant_id, &wanted) {
        Ok(evidence) => evidence,
        Err(failure) if failure.is_refusal() => {
            errors.push(failure.error);
            return Ok(DatabaseComparison::not_compared());
        }
        Err(failure) => return Err(failure),
    };
    let mut comparison = DatabaseComparison {
        compared: true,
        event_count: Some(evidence.event_count),
        head_hash: Some(evidence.head_hash.clone()),
        audit_schema_version: Some(evidence.audit_schema_version),
        positions_compared: None,
        extends_checkpoint: None,
        events_after_checkpoint: None,
    };
    if evidence.namespace_uuid != anchor.namespace_uuid {
        errors.push(
            Failure::new(
                Code::ScopeMismatch,
                "the audit database belongs to another control namespace than the trust anchor",
            )
            .with("field", "namespace_uuid")
            .with("between", vec!["trust_anchor", "database"])
            .with("expected", anchor.namespace_uuid.clone())
            .with("actual", evidence.namespace_uuid.clone())
            .error,
        );
        return Ok(comparison);
    }
    // With every trusted checkpoint naming one genesis, the most recent one
    // stands for all of them.
    if let Some(signed) = &chain.genesis
        && !signed.is(evidence.control_genesis_seq, &evidence.control_genesis_hash)
    {
        errors.push(
            control_genesis_mismatch(
                "the audit database has another control genesis than the one named by the most recent trusted checkpoint",
                ["checkpoint_record", "database"],
            )
            .with("signed_at_log_index", signed.log_index)
            .with("signed_control_genesis_seq", signed.sequence)
            .with("signed_control_genesis_hash", signed.hash.clone())
            .with("database_control_genesis_seq", evidence.control_genesis_seq)
            .with(
                "database_control_genesis_hash",
                evidence.control_genesis_hash.clone(),
            )
            .error,
        );
        return Ok(comparison);
    }
    // Whether or not a record at hand names the signed genesis, it was signed
    // at every trusted position: it cannot stand after the oldest of them.
    if let Some(oldest) = chain.positions.first()
        && evidence.control_genesis_seq > oldest.position.event_count
    {
        errors.push(
            control_genesis_mismatch(
                "the control genesis of the audit database stands after a trusted signed position",
                ["trusted_signed_position", "database"],
            )
            .with("signed_event_count", oldest.position.event_count)
            .with("signed_at_log_index", oldest.log_index)
            .with("position_source", oldest.source)
            .with("database_control_genesis_seq", evidence.control_genesis_seq)
            .with(
                "database_control_genesis_hash",
                evidence.control_genesis_hash.clone(),
            )
            .error,
        );
        return Ok(comparison);
    }
    let mut compared = 0_u64;
    for trusted in &chain.positions {
        compared += 1;
        comparison.positions_compared = Some(compared);
        let position = &trusted.position;
        if position.event_count > evidence.event_count {
            comparison.extends_checkpoint = Some(false);
            errors.push(
                Failure::new(
                    Code::DatabaseTruncated,
                    "the audit database holds fewer events than a trusted signed position",
                )
                .with("signed_event_count", position.event_count)
                .with("database_event_count", evidence.event_count)
                .with("signed_at_log_index", trusted.log_index)
                .with("position_source", trusted.source)
                .error,
            );
            return Ok(comparison);
        }
        let actual = evidence.hashes.get(&position.event_count);
        if actual != Some(&position.head_hash) {
            comparison.extends_checkpoint = Some(false);
            errors.push(
                Failure::new(
                    Code::DatabaseDiverged,
                    "the audit database does not hold the signed chain hash at a trusted signed position",
                )
                .with("event_count", position.event_count)
                .with("signed_head_hash", position.head_hash.clone())
                .with("database_hash_at_position", actual.cloned())
                .with("signed_at_log_index", trusted.log_index)
                .with("position_source", trusted.source)
                .error,
            );
            return Ok(comparison);
        }
    }
    let Some(last) = chain.positions.last() else {
        return Ok(comparison);
    };
    comparison.extends_checkpoint = Some(true);
    comparison.events_after_checkpoint = Some(evidence.event_count - last.position.event_count);
    Ok(comparison)
}

fn limits(anchor: &TrustAnchor, database_compared: bool) -> Vec<String> {
    let mut limits = vec![
        "A signature attests that the pinned key signed this chain head at this log position; it does not attest that an event is true or that a runtime effect happened.".to_owned(),
        "Events after the last signed position are not covered by any signature until the next checkpoint.".to_owned(),
        "created_at is declared by the signer and is not a trusted timestamp.".to_owned(),
        "Nothing here resists an administrator who controls the audit database, the signing key and the trust anchor at the same time.".to_owned(),
    ];
    limits.push(match &anchor.reference {
        Some(_) => "Rollback detection holds at and below the anchor reference only: between the reference and the head, the log and database can be returned to an earlier signed state without detection, and can be rewritten by whoever holds the signing key. The history at and below the reference is protected only when a database is compared: it must hold the referenced position, every position the anchor retains and every signed position verified after the reference.".to_owned(),
        None => "The trust anchor carries no accepted reference: a rollback of the log and database to an earlier signed state cannot be detected.".to_owned(),
    });
    if anchor.reference.is_some() {
        limits.push(
            "Records at and below the anchor reference are not verified by signature again: each one present was matched, by statement hash, to the chain that the accepted reference record commits to."
                .to_owned(),
        );
    }
    let retained = anchor
        .reference
        .as_ref()
        .map_or(0, |reference| reference.retained_checkpoints.len());
    if retained > 0 {
        limits.push(format!(
            "The trust anchor retains {retained} earlier position(s) that its reference moved past without a database comparison: that the reference extends them is attested by the signer only, until a database is compared."
        ));
    }
    if !database_compared {
        limits.push(
            "The audit database was not compared: this verdict covers the checkpoint records only and says nothing about the audit history itself."
                .to_owned(),
        );
    }
    limits
}

fn same_device_warning(anchor: &LoadedAnchor, others: &[&Path]) -> Option<Warning> {
    let device = files::device_of(&anchor.path)?;
    others
        .iter()
        .any(|path| files::device_of(path) == Some(device))
        .then(|| Warning {
            code: "same_filesystem_device",
            message: "The trust anchor is on the same filesystem device as the audit database or checkpoint directory read by this command. This is expected when an auditor verifies copies on a separate machine; on the control host it is a weak hint that the anchor is not held independently.".to_owned(),
        })
}

/// What a command that read every record says about the record files that
/// are not complete records: that the directory holds some, and that one
/// stands above the last complete record. `verify` and `anchor advance` carry
/// these warnings in the verification report, which lists the files in
/// `incomplete_records`; `anchor approve-rotation` carries them in its
/// outcome, which has no such list.
pub(crate) fn incomplete_record_warnings(set: &RecordSet) -> Vec<Warning> {
    let mut warnings = Vec::new();
    if !set.incomplete.is_empty() {
        warnings.push(Warning {
            code: "incomplete_records_present",
            message: "The checkpoint directory holds record files that are not complete records. They were ignored for the chain and are never removed by this tool. The reports of verify and diagnose list them in incomplete_records.".to_owned(),
        });
    }
    let log_head = set.head_index();
    if set.incomplete.iter().any(|record| {
        parse_record_file_name(&record.name)
            .is_some_and(|(log_index, _)| log_head.is_none_or(|head| log_index > head))
    }) {
        warnings.push(Warning {
            code: "incomplete_record_above_head",
            message: "A record file above the last complete record is not a complete record. An interrupted write leaves this, and so does truncating a signed record: the log is trusted no further than its last complete record (verified_through_log_index in the report of verify gives the exact position), and only the anchor reference detects a rollback. Run create to resume signing.".to_owned(),
        });
    }
    warnings
}

/// Load the records named by a source.
pub(crate) fn load_records(source: RecordSource<'_>) -> Result<RecordSet, Failure> {
    match source {
        RecordSource::Directory(directory) => CheckpointLog::open_reader(directory)?.load(),
        RecordSource::Bundle(bundle) => read_bundle(bundle),
    }
}

/// Everything a verification established, before it is rendered as a report.
pub(crate) struct Verification {
    pub loaded: LoadedAnchor,
    pub chain: ChainOutcome,
    pub report: VerificationReport,
}

pub(crate) fn run(request: &VerifyRequest<'_>) -> Result<Verification, Failure> {
    let loaded = load_anchor(request.anchor)?;
    let anchor = &loaded.anchor;
    let set = load_records(request.source)?;
    let mut chain = check_records(anchor, &set);
    let mut errors = std::mem::take(&mut chain.errors);
    if let Err(failure) = refuse_renamed_records(&set.incomplete) {
        errors.push(failure.error);
    }
    let database = match request.data {
        Some(data) => match compare_database(anchor, &chain, data, &mut errors) {
            Ok(database) => database,
            // A refusal that is already established is not lost because the
            // database could not be read: it is reported, with the reading
            // failure, and the database is marked as not compared.
            Err(failure) if !errors.is_empty() => {
                errors.push(failure.error);
                DatabaseComparison::not_compared()
            }
            Err(failure) => return Err(failure),
        },
        None => DatabaseComparison::not_compared(),
    };
    if request.require_reference && anchor.reference.is_none() {
        errors.push(
            Failure::new(
                Code::NoIndependentReference,
                "the trust anchor carries no accepted reference, and one was required",
            )
            .error,
        );
    }
    let mut observed: Vec<&Path> = Vec::new();
    if let RecordSource::Directory(directory) = request.source {
        observed.push(directory);
    }
    observed.extend(request.data);
    let mut warnings: Vec<Warning> = same_device_warning(&loaded, &observed)
        .into_iter()
        .collect();
    warnings.extend(incomplete_record_warnings(&set));
    if !set.unexpected.is_empty() {
        warnings.push(Warning {
            code: "unexpected_entries_present",
            message: "The checkpoint directory holds entries that are not checkpoint records. They were ignored and are listed in unexpected_entries.".to_owned(),
        });
    }
    if anchor
        .keys
        .iter()
        .any(|key| key.approval == APPROVAL_OPERATOR_RECOVERY)
    {
        warnings.push(Warning {
            code: "unendorsed_key_approval",
            message: "The trust anchor pins a key that was approved as an operator recovery, without an endorsement by the key it replaced.".to_owned(),
        });
    }
    let verdict = if errors.is_empty() {
        Verdict::Verified
    } else {
        Verdict::Refused
    };
    let summary = match (verdict, chain.verified_through) {
        (Verdict::Verified, Some(index)) => format!(
            "{} record(s) verified now; the log is trusted through log index {index} with keys pinned by the trust anchor.",
            chain.entries_checked
        ),
        (Verdict::Verified, None) => "No record was verified.".to_owned(),
        (Verdict::Refused, _) => format!(
            "Refused: {}.",
            errors
                .iter()
                .map(|error| error.code)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    let report = VerificationReport {
        format: VERIFICATION_FORMAT,
        verdict,
        summary,
        scope: Scope {
            tenant_id: anchor.tenant_id.clone(),
            namespace_uuid: anchor.namespace_uuid.clone(),
        },
        anchor: AnchorIdentity {
            revision: anchor.revision,
            sha256: loaded.sha256.clone(),
        },
        reference: anchor.reference.as_ref().map(|reference| ReferenceSummary {
            log_index: reference.log_index,
            statement_sha256: reference.statement_sha256.clone(),
        }),
        rollback_detection: match anchor.reference {
            Some(_) => RollbackDetection::AnchorReference,
            None => RollbackDetection::UnavailableNoReference,
        },
        entries_checked: chain.entries_checked,
        signatures_verified: chain.signatures_verified,
        verified_through_log_index: chain.verified_through,
        head: chain.head.clone(),
        limits: limits(anchor, database.compared),
        database,
        incomplete_records: set.incomplete,
        unexpected_entries: set.unexpected,
        warnings,
        errors,
    };
    Ok(Verification {
        loaded,
        chain,
        report,
    })
}

/// Verify checkpoint records, and optionally an audit database, against a
/// trust anchor. A refusal is a report with `verdict: refused`; an `Err` means
/// that no trust decision could be reached.
pub fn verify(request: &VerifyRequest<'_>) -> Result<VerificationReport, Failure> {
    if let Some(out) = request.report_out {
        files::require_new_output(out)?;
    }
    let mut report = run(request)?.report;
    if let Some(out) = request.report_out {
        let written = serde_json::to_vec(&report)
            .map_err(|_| {
                Failure::new(
                    Code::IoFailure,
                    "the verification report cannot be serialized",
                )
            })
            .and_then(|mut bytes| {
                bytes.push(b'\n');
                files::write_new(out, &bytes, crate::control_fs::MAX_PRIVATE_FILE_BYTES)
            });
        if let Err(failure) = written {
            // The destination was checked before the verification; a failure
            // here is a late one. A refusal is still reported as a refusal.
            // A verified report that could not be written is an error: the
            // caller asked for a file and has none.
            if report.is_verified() {
                return Err(failure);
            }
            report.errors.push(failure.error);
        }
    }
    Ok(report)
}
