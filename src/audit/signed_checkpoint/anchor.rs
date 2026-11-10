//! The trust anchor: the only source of trusted keys and of the accepted
//! reference, held outside the controlled storage.
//!
//! The anchor is public material. Its value comes from who holds it: when the
//! account that can rewrite the audit database and the signing key can also
//! rewrite the anchor, this mechanism proves nothing. Every change is a new
//! revision written to a new file and linked to the previous revision by its
//! SHA-256; no revision is ever overwritten.

use super::{
    ANCHOR_FORMAT, MAX_ANCHOR_BYTES, MAX_ANCHOR_KEYS, MAX_RETAINED_CHECKPOINTS,
    SIGNATURE_ALGORITHM, files,
    format::{
        ENDORSEMENT_RETIRING_KEY, SignedRecord, Statement, decode_base64url, is_canonical_time,
        is_canonical_uuid, is_sha256_hex, now, sha256_hex,
    },
    keys,
    log::refuse_renamed_records,
    report::{Code, Failure, FailureClass, VerificationReport, Warning},
    verify::{self, RecordSource, VerifyRequest},
};
use crate::tenancy::validate_tenant_id;
use serde::{Deserialize, Deserializer, Serialize};
use std::path::{Path, PathBuf};

pub(crate) const APPROVAL_INITIAL: &str = "initial";
pub(crate) const APPROVAL_ENDORSED_ROTATION: &str = "endorsed_rotation";
pub(crate) const APPROVAL_OPERATOR_RECOVERY: &str = "operator_recovery";

/// A nullable member that must still be present in the document.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// One explicitly approved public key and the log positions it may sign.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorKey {
    pub key_id: String,
    pub algorithm: String,
    pub public_key: String,
    /// First log index this key may sign.
    pub valid_from_log_index: u64,
    /// Last log index this key may sign a checkpoint at; `null` for the current
    /// key. A retired key stops one position before its rotation record.
    #[serde(deserialize_with = "present")]
    pub valid_through_log_index: Option<u64>,
    /// `initial`, `endorsed_rotation` or `operator_recovery`.
    pub approval: String,
}

/// The last signed audit position at or below the reference record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorCheckpoint {
    pub log_index: u64,
    pub event_count: u64,
    pub head_hash: String,
}

/// The last record accepted by the anchor holder. Any log that does not hold
/// exactly this record at this position is refused, and a compared database
/// must hold `checkpoint` and every retained position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorReference {
    pub log_index: u64,
    pub statement_sha256: String,
    pub checkpoint: AnchorCheckpoint,
    /// Signed positions pinned by earlier revisions, oldest first, that the
    /// reference moved past without a database comparison: that `checkpoint`
    /// extends them is attested by the signer only. They stay here, and every
    /// verification with a database compares each of them, until an advance
    /// made with a database confirms the continuity. Advancing without a
    /// database therefore never drops the protection of a pinned position.
    pub retained_checkpoints: Vec<AnchorCheckpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustAnchor {
    pub format: String,
    pub revision: u64,
    #[serde(deserialize_with = "present")]
    pub previous_anchor_sha256: Option<String>,
    pub tenant_id: String,
    pub namespace_uuid: String,
    pub keys: Vec<AnchorKey>,
    #[serde(deserialize_with = "present")]
    pub reference: Option<AnchorReference>,
    /// Declared by the writer of this revision; not a trusted timestamp.
    pub updated_at: String,
}

fn invalid(message: &str) -> Failure {
    Failure::new(Code::AnchorInvalid, message)
}

fn require(condition: bool, message: &str) -> Result<(), Failure> {
    if condition {
        Ok(())
    } else {
        Err(invalid(message))
    }
}

impl AnchorKey {
    pub(crate) fn public_key_bytes(&self) -> Option<[u8; 32]> {
        decode_base64url(&self.public_key, 32).and_then(|bytes| bytes.try_into().ok())
    }

    /// Whether this key may sign a checkpoint at `log_index`.
    pub(crate) fn valid_at(&self, log_index: u64) -> bool {
        self.valid_from_log_index <= log_index
            && self
                .valid_through_log_index
                .is_none_or(|through| log_index <= through)
    }
}

impl TrustAnchor {
    pub(crate) fn validate(&self) -> Result<(), Failure> {
        require(
            self.format == ANCHOR_FORMAT,
            "trust anchor format is not supported",
        )?;
        require(
            self.revision >= 1 && self.revision <= i64::MAX as u64,
            "trust anchor revision is out of range",
        )?;
        require(
            match &self.previous_anchor_sha256 {
                None => self.revision == 1,
                Some(previous) => self.revision > 1 && is_sha256_hex(previous),
            },
            "previous_anchor_sha256 is inconsistent with the revision",
        )?;
        require(
            validate_tenant_id(&self.tenant_id).is_ok() && is_canonical_uuid(&self.namespace_uuid),
            "trust anchor scope is not a tenant identifier and a canonical namespace UUID",
        )?;
        require(
            !self.keys.is_empty() && self.keys.len() <= MAX_ANCHOR_KEYS,
            "trust anchor must pin between 1 and the maximum number of keys",
        )?;
        for (position, key) in self.keys.iter().enumerate() {
            require(
                key.algorithm == SIGNATURE_ALGORITHM
                    && is_sha256_hex(&key.key_id)
                    && key
                        .public_key_bytes()
                        .is_some_and(|public| sha256_hex(&public) == key.key_id),
                "a pinned key does not match its fingerprint or algorithm",
            )?;
            require(
                key.valid_from_log_index >= 1
                    && key
                        .valid_through_log_index
                        .is_none_or(|through| through <= i64::MAX as u64),
                "a pinned key has an invalid validity window",
            )?;
            require(
                matches!(
                    key.approval.as_str(),
                    APPROVAL_INITIAL | APPROVAL_ENDORSED_ROTATION | APPROVAL_OPERATOR_RECOVERY
                ),
                "a pinned key has an unknown approval",
            )?;
            require(
                !self.keys[..position]
                    .iter()
                    .any(|other| other.key_id == key.key_id),
                "a key is pinned twice",
            )?;
        }
        require(
            self.keys
                .iter()
                .filter(|key| key.valid_through_log_index.is_none())
                .count()
                == 1
                && self
                    .keys
                    .last()
                    .is_some_and(|key| key.valid_through_log_index.is_none()),
            "exactly the last pinned key must be the current key",
        )?;
        if let Some(reference) = &self.reference {
            require(
                reference.log_index >= 1
                    && reference.log_index <= i64::MAX as u64
                    && is_sha256_hex(&reference.statement_sha256)
                    && (1..=reference.log_index).contains(&reference.checkpoint.log_index)
                    && (1..=i64::MAX as u64).contains(&reference.checkpoint.event_count)
                    && is_sha256_hex(&reference.checkpoint.head_hash),
                "trust anchor reference is invalid",
            )?;
            require(
                reference.retained_checkpoints.len() <= MAX_RETAINED_CHECKPOINTS,
                "trust anchor retains more earlier positions than its bound",
            )?;
            // Oldest first: each retained position precedes the next one in
            // the log and never signs more events than it.
            let mut later = &reference.checkpoint;
            for retained in reference.retained_checkpoints.iter().rev() {
                require(
                    retained.log_index >= 1
                        && retained.log_index < later.log_index
                        && (1..=later.event_count).contains(&retained.event_count)
                        && is_sha256_hex(&retained.head_hash)
                        && (retained.event_count != later.event_count
                            || retained.head_hash == later.head_hash),
                    "a retained earlier position is invalid or out of order",
                )?;
                later = retained;
            }
        }
        require(
            is_canonical_time(&self.updated_at),
            "updated_at is not a canonical UTC time with milliseconds",
        )
    }

    pub(crate) fn key(&self, key_id: &str) -> Option<&AnchorKey> {
        self.keys.iter().find(|key| key.key_id == key_id)
    }

    /// The key that signs new records.
    pub(crate) fn current_key(&self) -> Option<&AnchorKey> {
        self.keys
            .iter()
            .find(|key| key.valid_through_log_index.is_none())
    }

    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>, Failure> {
        self.validate()?;
        let mut bytes =
            serde_json::to_vec(self).map_err(|_| invalid("trust anchor cannot be serialized"))?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_ANCHOR_BYTES {
            return Err(invalid("trust anchor exceeds its size bound"));
        }
        Ok(bytes)
    }
}

/// A trust anchor read from a file, with the SHA-256 of the exact file bytes.
#[derive(Debug, Clone)]
pub(crate) struct LoadedAnchor {
    pub anchor: TrustAnchor,
    pub sha256: String,
    pub path: PathBuf,
}

pub(crate) fn load_anchor(path: &Path) -> Result<LoadedAnchor, Failure> {
    let absolute = files::absolute(path)?;
    let bytes = files::read_input(
        &absolute,
        MAX_ANCHOR_BYTES,
        Code::AnchorUnavailable,
        "anchor",
    )?;
    crate::json::decode_unique(&bytes, 8).map_err(|error| {
        invalid("trust anchor is not strict JSON").with("reason", error.to_string())
    })?;
    let anchor: TrustAnchor = serde_json::from_slice(&bytes).map_err(|error| {
        invalid("trust anchor does not match its contract").with("reason", error.to_string())
    })?;
    anchor.validate()?;
    Ok(LoadedAnchor {
        anchor,
        sha256: sha256_hex(&bytes),
        path: absolute,
    })
}

/// Result of a command that wrote a new trust anchor revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnchorOutcome {
    pub status: &'static str,
    pub anchor_file: String,
    pub revision: u64,
    pub sha256: String,
    pub previous_anchor_sha256: Option<String>,
    pub tenant_id: String,
    pub namespace_uuid: String,
    pub keys: Vec<AnchorKey>,
    pub reference: Option<AnchorReference>,
    pub warnings: Vec<Warning>,
}

pub(crate) fn write_anchor(
    anchor: TrustAnchor,
    out: &Path,
    status: &'static str,
    warnings: Vec<Warning>,
) -> Result<AnchorOutcome, Failure> {
    let bytes = anchor.to_bytes()?;
    let path = files::write_new(out, &bytes, MAX_ANCHOR_BYTES)?;
    Ok(AnchorOutcome {
        status,
        anchor_file: path.display().to_string(),
        revision: anchor.revision,
        sha256: sha256_hex(&bytes),
        previous_anchor_sha256: anchor.previous_anchor_sha256,
        tenant_id: anchor.tenant_id,
        namespace_uuid: anchor.namespace_uuid,
        keys: anchor.keys,
        reference: anchor.reference,
        warnings,
    })
}

pub struct AnchorInitRequest<'a> {
    /// Public key document handed over by the operator.
    pub public_key: &'a Path,
    /// Fingerprint obtained through a separate channel.
    pub expect_key_id: &'a str,
    pub tenant_id: &'a str,
    pub namespace_uuid: &'a str,
    /// New file, outside the storage controlled by the signer.
    pub out: &'a Path,
}

pub(crate) fn expected_key_id(value: &str) -> Result<(), Failure> {
    if is_sha256_hex(value) {
        Ok(())
    } else {
        Err(Failure::new(
            Code::InvalidArgument,
            "the expected key fingerprint is not 64 lowercase hexadecimal characters",
        ))
    }
}

/// Approve one key explicitly for one tenant and namespace. The key is
/// accepted only when its fingerprint equals the one supplied by the caller.
pub fn init_anchor(request: &AnchorInitRequest<'_>) -> Result<AnchorOutcome, Failure> {
    expected_key_id(request.expect_key_id)?;
    if validate_tenant_id(request.tenant_id).is_err() || !is_canonical_uuid(request.namespace_uuid)
    {
        return Err(Failure::new(
            Code::InvalidArgument,
            "the scope is not a tenant identifier and a canonical lowercase namespace UUID",
        ));
    }
    let document = keys::read_public_key(request.public_key)?;
    if document.key_id != request.expect_key_id {
        return Err(Failure::new(
            Code::KeyIdMismatch,
            "the public key file does not carry the expected fingerprint",
        )
        .with("between", vec!["expect_key_id", "public_key_file"])
        .with("expected", request.expect_key_id)
        .with("actual", document.key_id));
    }
    let anchor = TrustAnchor {
        format: ANCHOR_FORMAT.into(),
        revision: 1,
        previous_anchor_sha256: None,
        tenant_id: request.tenant_id.into(),
        namespace_uuid: request.namespace_uuid.into(),
        keys: vec![AnchorKey {
            key_id: document.key_id,
            algorithm: document.algorithm,
            public_key: document.public_key,
            valid_from_log_index: 1,
            valid_through_log_index: None,
            approval: APPROVAL_INITIAL.into(),
        }],
        reference: None,
        updated_at: now(),
    };
    write_anchor(anchor, request.out, "initialized", Vec::new())
}

/// The earlier positions that the next revision must keep comparing with a
/// database. A comparison that held every trusted position confirms the
/// continuity up to the new reference and leaves nothing to retain.
fn retained_after(
    current: Option<&AnchorReference>,
    position_log_index: u64,
    database_compared: bool,
) -> Result<Vec<AnchorCheckpoint>, Failure> {
    let Some(reference) = current else {
        return Ok(Vec::new());
    };
    if database_compared {
        return Ok(Vec::new());
    }
    let mut retained = reference.retained_checkpoints.clone();
    if reference.checkpoint.log_index != position_log_index {
        retained.push(reference.checkpoint.clone());
    }
    if retained.len() > MAX_RETAINED_CHECKPOINTS {
        return Err(Failure::new(
            Code::DatabaseComparisonRequired,
            "the trust anchor already retains the maximum number of positions recorded without a database comparison",
        )
        .with("retained_checkpoints", reference.retained_checkpoints.len())
        .with("limit", MAX_RETAINED_CHECKPOINTS));
    }
    Ok(retained)
}

fn advanced_without_database() -> Warning {
    Warning {
        code: "reference_advanced_without_database",
        message: "The reference was recorded without comparing an audit database: that the new signed position extends the positions pinned so far is attested by the signer only. The earlier positions are retained in this revision and compared whenever a database is given; advance with --data to confirm the continuity.".to_owned(),
    }
}

pub struct AnchorAdvanceRequest<'a> {
    pub anchor: &'a Path,
    pub source: RecordSource<'a>,
    /// Audit database to compare before accepting the new reference. Without
    /// it, the positions pinned so far are retained in the new revision.
    pub data: Option<&'a Path>,
    /// New file for the next revision. The current revision is kept.
    pub out: &'a Path,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdvanceStatus {
    /// A new revision carries the verified head as its reference.
    Advanced,
    /// The reference already is the verified head, and the compared database
    /// held every position that the anchor retained: a new revision carries
    /// the same reference and retains nothing.
    Confirmed,
    /// The reference already is the verified head and nothing was left to
    /// confirm; no file was written.
    Unchanged,
    /// Verification refused; no file was written.
    Refused,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdvanceOutcome {
    pub status: AdvanceStatus,
    pub anchor: Option<AnchorOutcome>,
    pub verification: VerificationReport,
}

/// Verify, then record the verified head as the accepted reference in a new
/// anchor revision. The reference never moves backwards: a log that ends
/// before the current reference is a refused verification.
///
/// Records alone prove that the pinned key signed the new head, not that the
/// new head extends the audit history pinned so far. With a database, every
/// trusted position was compared and the new revision carries the new
/// position only. Without one, the positions pinned so far are retained in the
/// new revision, which says so in a warning.
///
/// When no record is new, a database that held every retained position is
/// still recorded: the continuity that earlier advances took on the word of
/// the signer is confirmed, and a new revision keeps the same reference and
/// retains nothing. The chain hash of the reference position commits to every
/// event before it, so nothing that the retained positions protected is lost.
pub fn advance_anchor(request: &AnchorAdvanceRequest<'_>) -> Result<AdvanceOutcome, Failure> {
    files::require_new_output(request.out)?;
    let verification = verify::run(&VerifyRequest {
        anchor: request.anchor,
        source: request.source,
        data: request.data,
        require_reference: false,
        report_out: None,
    })?;
    let refused = |verification: verify::Verification| AdvanceOutcome {
        status: AdvanceStatus::Refused,
        anchor: None,
        verification: verification.report,
    };
    if !verification.report.is_verified() {
        return Ok(refused(verification));
    }
    let chain = &verification.chain;
    let (Some(log_index), Some(statement_sha256), Some(position), Some(position_log_index)) = (
        chain.verified_through,
        chain.verified_sha256.clone(),
        chain.position.clone(),
        chain.position_log_index,
    ) else {
        return Ok(refused(verification));
    };
    let current = &verification.loaded.anchor;
    let database_compared = verification.report.database.compared
        && verification.report.database.extends_checkpoint == Some(true);
    let status = match &current.reference {
        Some(reference) if reference.log_index == log_index => {
            // No record is new. Only a database that held the positions this
            // revision retains is worth another revision.
            if !database_compared || reference.retained_checkpoints.is_empty() {
                return Ok(AdvanceOutcome {
                    status: AdvanceStatus::Unchanged,
                    anchor: None,
                    verification: verification.report,
                });
            }
            AdvanceStatus::Confirmed
        }
        _ => AdvanceStatus::Advanced,
    };
    let retained_checkpoints = retained_after(
        current.reference.as_ref(),
        position_log_index,
        database_compared,
    )?;
    let mut warnings = verification.report.warnings.clone();
    if !database_compared {
        warnings.push(advanced_without_database());
    }
    let next = TrustAnchor {
        revision: current.revision + 1,
        previous_anchor_sha256: Some(verification.loaded.sha256.clone()),
        reference: Some(AnchorReference {
            log_index,
            statement_sha256,
            checkpoint: AnchorCheckpoint {
                log_index: position_log_index,
                event_count: position.event_count,
                head_hash: position.head_hash,
            },
            retained_checkpoints,
        }),
        updated_at: now(),
        ..current.clone()
    };
    let written_as = match status {
        AdvanceStatus::Confirmed => "confirmed",
        _ => "advanced",
    };
    let written = write_anchor(next, request.out, written_as, warnings)?;
    Ok(AdvanceOutcome {
        status,
        anchor: Some(written),
        verification: verification.report,
    })
}

pub struct RotationApprovalRequest<'a> {
    pub anchor: &'a Path,
    pub source: RecordSource<'a>,
    /// Audit database to compare with every trusted position up to the
    /// rotation before the reference moves. Without it, the positions pinned
    /// so far are retained in the new revision.
    pub data: Option<&'a Path>,
    /// Public key document of the successor, handed over by the operator.
    pub public_key: &'a Path,
    /// Successor fingerprint obtained through a separate channel.
    pub expect_key_id: &'a str,
    /// Accept a rotation that the retiring key did not endorse.
    pub unendorsed_recovery: bool,
    /// New file for the next revision. The current revision is kept.
    pub out: &'a Path,
}

fn signature_of<'a>(record: &'a SignedRecord, key_id: &str) -> Option<&'a str> {
    record
        .signatures()
        .iter()
        .find(|signature| signature.key_id == key_id)
        .map(|signature| signature.signature.as_str())
}

/// Approve the first key rotation recorded after the anchor reference.
///
/// The successor is trusted only after three fingerprints agree (the expected
/// one, the public key file and the rotation record), after the successor
/// proved possession of its key, and after the retiring key pinned in the
/// anchor endorsed the hand-over — or after an explicit recovery decision.
/// The new revision pins the rotation record itself as its reference: no other
/// record can stand at that log position any more, and the retired key cannot
/// sign anything at or after it. As with `advance_anchor`, the positions
/// pinned so far are retained unless a database was compared, and the outcome
/// carries the warnings of a verification about record files that are not
/// complete records.
///
/// The approval examines the records up to the rotation: the ones at and
/// below the current reference must be the chain it commits to, and the ones
/// between the reference and the rotation are verified. It does not examine
/// what stands after the rotation, and says so in a warning: those records
/// are verified with the new revision.
pub fn approve_rotation(request: &RotationApprovalRequest<'_>) -> Result<AnchorOutcome, Failure> {
    files::require_new_output(request.out)?;
    expected_key_id(request.expect_key_id)?;
    let loaded = load_anchor(request.anchor)?;
    let anchor = &loaded.anchor;
    let set = verify::load_records(request.source)?;
    // A directory in which a record file holds another complete record than
    // the one it is named after was altered. Nothing is approved from it,
    // exactly as `verify` and `advance_anchor` refuse it.
    refuse_renamed_records(&set.incomplete)?;
    let mut chain = verify::check_records(anchor, &set);
    let Some(rotation) = chain.pending_rotation.take() else {
        // The walk stopped before any rotation, or found none.
        return Err(match chain.errors.into_iter().next() {
            Some(error) => Failure {
                class: FailureClass::Refusal,
                error,
            },
            None => Failure::new(
                Code::NoPendingRotation,
                "no key rotation record follows the anchor reference",
            ),
        });
    };
    let Statement::KeyRotation(statement) = rotation.statement() else {
        return Err(invalid("a pending rotation is not a rotation record"));
    };
    let log_index = rotation.log_index();

    // The retiring key is the current key of the anchor.
    let Some(current) = anchor.current_key() else {
        return Err(invalid("trust anchor has no current key"));
    };
    if statement.retiring_key_id != current.key_id {
        return Err(Failure::new(
            Code::KeyIdMismatch,
            "the rotation does not retire the current key of the trust anchor",
        )
        .with(
            "between",
            vec!["trust_anchor_current_key", "rotation_record"],
        )
        .with("expected", current.key_id.clone())
        .with("actual", statement.retiring_key_id.clone())
        .with("log_index", log_index));
    }

    // Three fingerprints: expected, public key file, rotation record.
    let document = keys::read_public_key(request.public_key)?;
    for (between, actual) in [
        ("public_key_file", &document.key_id),
        ("rotation_record", &statement.successor_key_id),
    ] {
        if actual != request.expect_key_id {
            return Err(Failure::new(
                Code::KeyIdMismatch,
                "the successor fingerprint differs from the expected one",
            )
            .with("between", vec!["expect_key_id", between])
            .with("expected", request.expect_key_id)
            .with("actual", actual.clone())
            .with("log_index", log_index));
        }
    }
    if anchor.key(&document.key_id).is_some() {
        return Err(Failure::new(
            Code::KeyIdMismatch,
            "the successor key is already pinned by the trust anchor",
        )
        .with("key_id", document.key_id));
    }

    // Exactly the signatures required by the statement.
    let required = rotation.statement().required_signers();
    let named: Vec<&str> = rotation
        .signatures()
        .iter()
        .map(|signature| signature.key_id.as_str())
        .collect();
    if named != required {
        return Err(Failure::new(
            Code::SignatureInvalid,
            "the rotation record does not carry exactly the signatures required by its statement",
        )
        .with("log_index", log_index)
        .with("required_signers", required)
        .with("actual_signers", named));
    }

    // Endorsement by the retiring key, verified with the pinned public key.
    let endorsed = statement.endorsement == ENDORSEMENT_RETIRING_KEY;
    let approval = if endorsed {
        if request.unendorsed_recovery {
            return Err(Failure::new(
                Code::InvalidArgument,
                "this rotation is endorsed by the retiring key; --unendorsed-recovery does not apply",
            ));
        }
        let verified = current
            .public_key_bytes()
            .zip(signature_of(&rotation, &current.key_id))
            .is_some_and(|(public_key, signature)| {
                keys::verify_signature(&public_key, rotation.statement_bytes(), signature)
            });
        if !verified {
            return Err(Failure::new(
                Code::RotationNotEndorsed,
                "the endorsement of the retiring key does not verify with the key pinned in the trust anchor",
            )
            .with("log_index", log_index)
            .with("retiring_key_id", current.key_id.clone()));
        }
        APPROVAL_ENDORSED_ROTATION
    } else {
        if !request.unendorsed_recovery {
            return Err(Failure::new(
                Code::RotationNotEndorsed,
                "the retiring key did not endorse this rotation",
            )
            .with("log_index", log_index)
            .with("retiring_key_id", current.key_id.clone()));
        }
        APPROVAL_OPERATOR_RECOVERY
    };

    // Proof of possession by the successor, verified with the supplied key.
    let successor_public_key = document.public_key_bytes()?;
    let possessed = signature_of(&rotation, &document.key_id).is_some_and(|signature| {
        keys::verify_signature(&successor_public_key, rotation.statement_bytes(), signature)
    });
    if !possessed {
        return Err(Failure::new(
            Code::SignatureInvalid,
            "the successor signature does not verify with the supplied public key",
        )
        .with("log_index", log_index)
        .with("key_id", document.key_id));
    }

    let (Some(position), Some(position_log_index)) =
        (chain.position.clone(), chain.position_log_index)
    else {
        return Err(invalid(
            "a rotation was reached without a signed audit position",
        ));
    };
    // The checkpoints between the reference and the rotation were signed by
    // the retiring key; a database, when given, must hold all of them.
    let database_compared = match request.data {
        Some(data) => {
            let mut refusals = Vec::new();
            let comparison = verify::compare_database(anchor, &chain, data, &mut refusals)?;
            if let Some(error) = refusals.into_iter().next() {
                return Err(Failure {
                    class: FailureClass::Refusal,
                    error,
                });
            }
            comparison.extends_checkpoint == Some(true)
        }
        None => false,
    };
    let position_moved = anchor
        .reference
        .as_ref()
        .is_none_or(|reference| reference.checkpoint.log_index != position_log_index);
    let retained_checkpoints = retained_after(
        anchor.reference.as_ref(),
        position_log_index,
        database_compared,
    )?;
    if anchor.keys.len() >= MAX_ANCHOR_KEYS {
        return Err(Failure::new(
            Code::AnchorKeyLimit,
            "the trust anchor already pins the maximum number of keys",
        )
        .with("limit", MAX_ANCHOR_KEYS));
    }
    let mut keys = anchor.keys.clone();
    for key in &mut keys {
        if key.key_id == current.key_id {
            // The retired key may not sign a checkpoint at the rotation index
            // or after it.
            key.valid_through_log_index = Some(log_index - 1);
        }
    }
    keys.push(AnchorKey {
        key_id: document.key_id,
        algorithm: document.algorithm,
        public_key: document.public_key,
        valid_from_log_index: log_index,
        valid_through_log_index: None,
        approval: approval.into(),
    });
    let next = TrustAnchor {
        revision: anchor.revision + 1,
        previous_anchor_sha256: Some(loaded.sha256.clone()),
        keys,
        reference: Some(AnchorReference {
            log_index,
            statement_sha256: rotation.statement_sha256().to_owned(),
            checkpoint: AnchorCheckpoint {
                log_index: position_log_index,
                event_count: position.event_count,
                head_hash: position.head_hash,
            },
            retained_checkpoints,
        }),
        updated_at: now(),
        ..anchor.clone()
    };
    // The approval read every record, as `verify` does, and says the same
    // about the record files that are not complete records.
    let mut warnings = verify::incomplete_record_warnings(&set);
    if set.head_index().is_some_and(|head| head > log_index) {
        warnings.push(Warning {
            code: "records_after_rotation_not_examined",
            message: "Records stand after the approved rotation. This approval examined the records up to the rotation only: verify the later ones with the new revision (verify, then anchor advance).".to_owned(),
        });
    }
    if !endorsed {
        warnings.push(Warning {
            code: "unendorsed_key_approval",
            message: "The successor key was approved without an endorsement by the retiring key. Records signed by the retiring key between the previous anchor reference and this rotation are only as trustworthy as that key was: if it was compromised, treat them as untrusted.".to_owned(),
        });
    }
    if position_moved && !database_compared {
        warnings.push(advanced_without_database());
    }
    write_anchor(next, request.out, "rotation_approved", warnings)
}
