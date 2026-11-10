//! Structured errors and reports. Every failure names a stable code, a message,
//! a machine-readable detail object and the action expected from the operator.

use super::log::IncompleteRecord;
use serde::Serialize;
use serde_json::{Map, Value};

/// Exit-status class of a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// The command could not reach a trust decision (exit status 1).
    Operational,
    /// An integrity, trust or scope refusal (exit status 2).
    Refusal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Code {
    InvalidArgument,
    OutputExists,
    OutputUnavailable,
    KeyDirectoryExists,
    KeyUnavailable,
    KeyInvalid,
    CheckpointLogUnavailable,
    CheckpointLogBusy,
    CheckpointLogFull,
    CheckpointLogEmpty,
    CheckpointRecordExists,
    AnchorUnavailable,
    AnchorInvalid,
    AnchorKeyLimit,
    DatabaseComparisonRequired,
    InputUnavailable,
    AuditUnavailable,
    AuditSchemaUnsupported,
    NoPendingRotation,
    IoFailure,
    AuditChainInvalid,
    AuditSchemaUnversioned,
    AuditNamespaceUnavailable,
    ScopeMismatch,
    RecordMalformed,
    RecordNotCanonical,
    RecordNameMismatch,
    BundleMalformed,
    SignatureInvalid,
    UntrustedKey,
    KeyNotValidAtIndex,
    KeyIdMismatch,
    LogDiscontinuous,
    LogForked,
    LogRolledBack,
    LogDivergedFromReference,
    CheckpointRegression,
    DatabaseTruncated,
    DatabaseDiverged,
    RotationNotEndorsed,
    NoIndependentReference,
}

impl Code {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::OutputExists => "output_exists",
            Self::OutputUnavailable => "output_unavailable",
            Self::KeyDirectoryExists => "key_directory_exists",
            Self::KeyUnavailable => "key_unavailable",
            Self::KeyInvalid => "key_invalid",
            Self::CheckpointLogUnavailable => "checkpoint_log_unavailable",
            Self::CheckpointLogBusy => "checkpoint_log_busy",
            Self::CheckpointLogFull => "checkpoint_log_full",
            Self::CheckpointLogEmpty => "checkpoint_log_empty",
            Self::CheckpointRecordExists => "checkpoint_record_exists",
            Self::AnchorUnavailable => "anchor_unavailable",
            Self::AnchorInvalid => "anchor_invalid",
            Self::AnchorKeyLimit => "anchor_key_limit",
            Self::DatabaseComparisonRequired => "database_comparison_required",
            Self::InputUnavailable => "input_unavailable",
            Self::AuditUnavailable => "audit_unavailable",
            Self::AuditSchemaUnsupported => "audit_schema_unsupported",
            Self::NoPendingRotation => "no_pending_rotation",
            Self::IoFailure => "io_failure",
            Self::AuditChainInvalid => "audit_chain_invalid",
            Self::AuditSchemaUnversioned => "audit_schema_unversioned",
            Self::AuditNamespaceUnavailable => "audit_namespace_unavailable",
            Self::ScopeMismatch => "scope_mismatch",
            Self::RecordMalformed => "record_malformed",
            Self::RecordNotCanonical => "record_not_canonical",
            Self::RecordNameMismatch => "record_name_mismatch",
            Self::BundleMalformed => "bundle_malformed",
            Self::SignatureInvalid => "signature_invalid",
            Self::UntrustedKey => "untrusted_key",
            Self::KeyNotValidAtIndex => "key_not_valid_at_index",
            Self::KeyIdMismatch => "key_id_mismatch",
            Self::LogDiscontinuous => "log_discontinuous",
            Self::LogForked => "log_forked",
            Self::LogRolledBack => "log_rolled_back",
            Self::LogDivergedFromReference => "log_diverged_from_reference",
            Self::CheckpointRegression => "checkpoint_regression",
            Self::DatabaseTruncated => "database_truncated",
            Self::DatabaseDiverged => "database_diverged",
            Self::RotationNotEndorsed => "rotation_not_endorsed",
            Self::NoIndependentReference => "no_independent_reference",
        }
    }

    pub(crate) fn class(self) -> FailureClass {
        match self {
            Self::InvalidArgument
            | Self::OutputExists
            | Self::OutputUnavailable
            | Self::KeyDirectoryExists
            | Self::KeyUnavailable
            | Self::KeyInvalid
            | Self::CheckpointLogUnavailable
            | Self::CheckpointLogBusy
            | Self::CheckpointLogFull
            | Self::CheckpointLogEmpty
            | Self::CheckpointRecordExists
            | Self::AnchorUnavailable
            | Self::AnchorInvalid
            | Self::AnchorKeyLimit
            | Self::DatabaseComparisonRequired
            | Self::InputUnavailable
            | Self::AuditUnavailable
            | Self::AuditSchemaUnsupported
            | Self::NoPendingRotation
            | Self::IoFailure => FailureClass::Operational,
            Self::AuditChainInvalid
            | Self::AuditSchemaUnversioned
            | Self::AuditNamespaceUnavailable
            | Self::ScopeMismatch
            | Self::RecordMalformed
            | Self::RecordNotCanonical
            | Self::RecordNameMismatch
            | Self::BundleMalformed
            | Self::SignatureInvalid
            | Self::UntrustedKey
            | Self::KeyNotValidAtIndex
            | Self::KeyIdMismatch
            | Self::LogDiscontinuous
            | Self::LogForked
            | Self::LogRolledBack
            | Self::LogDivergedFromReference
            | Self::CheckpointRegression
            | Self::DatabaseTruncated
            | Self::DatabaseDiverged
            | Self::RotationNotEndorsed
            | Self::NoIndependentReference => FailureClass::Refusal,
        }
    }

    fn action(self) -> &'static str {
        match self {
            Self::InvalidArgument => "Correct the command arguments; nothing was written.",
            Self::OutputExists => {
                "Choose a new output path. Existing files are never overwritten or removed."
            }
            Self::OutputUnavailable => {
                "Write to a directory on a supported local filesystem whose ancestors belong to root or the current account and are not writable by group or others."
            }
            Self::KeyDirectoryExists => {
                "Choose a new key directory. An existing directory is never reused, even after an interrupted initialization."
            }
            Self::KeyUnavailable => {
                "Use a private key directory (mode 0700) holding signing-key.pk8 (mode 0600), both owned by the current account on a supported local filesystem."
            }
            Self::KeyInvalid => {
                "The key file is not an Ed25519 PKCS#8 v2 document written by this tool. Restore the intended key or record a rotation; the file is left untouched."
            }
            Self::CheckpointLogUnavailable => {
                "Use a private checkpoint directory (mode 0700, files 0600, owned by the current account) that holds only checkpoint records. Unexpected entries are never removed by this tool."
            }
            Self::CheckpointLogBusy => {
                "Another writer holds the checkpoint log. Wait for it to finish, then retry."
            }
            Self::CheckpointLogFull => {
                "Continue in a new checkpoint directory with create --previous-record pointing to the last record of this one."
            }
            Self::CheckpointLogEmpty => {
                "Create a first checkpoint before recording a key rotation."
            }
            Self::CheckpointRecordExists => {
                "Retry: a new attempt carries a new declared time and therefore a new record name. The existing file is preserved."
            }
            Self::AnchorUnavailable => {
                "Copy the trust anchor to a supported local filesystem, as a regular single-link file owned by root or the current account and not writable by group or others, under ancestors with the same ownership rule."
            }
            Self::AnchorInvalid => {
                "Restore a trust anchor revision written by this tool. An edited or truncated anchor is never repaired."
            }
            Self::AnchorKeyLimit => {
                "This trust anchor cannot pin another key. Keep it, and have the anchor holder decide on a new log and a new anchor."
            }
            Self::DatabaseComparisonRequired => {
                "Advance the reference with --data and a consistent copy of the audit database: this trust anchor cannot retain another position recorded without a database comparison."
            }
            Self::InputUnavailable => {
                "Copy the input to a supported local filesystem, as a regular single-link file owned by root or the current account and not writable by group or others."
            }
            Self::AuditUnavailable => {
                "Point --data to an existing audit database (mode 0600) in a private directory (mode 0700) owned by the current account, with an ASCII file name, and retry if the database was busy."
            }
            Self::AuditSchemaUnsupported => {
                "The audit schema version of the database is newer than this build knows. Use a build of this tool that supports it. The database is not modified."
            }
            Self::NoPendingRotation => {
                "There is no key rotation record after the anchor reference. Record one with rotate-key, or use anchor advance."
            }
            Self::IoFailure => {
                "Check free space and permissions, then retry with new output paths. Partial files are kept for diagnosis."
            }
            Self::AuditChainInvalid => {
                "Treat the audit database as altered or damaged: preserve it and every checkpoint record, and investigate before any restore. No checkpoint was signed."
            }
            Self::AuditSchemaUnversioned => {
                "If a checkpoint was ever signed for this database, treat this as a truncation: preserve the file and every checkpoint record, and investigate before any restore. A legacy database must first be opened once by the gateway, which records its schema version, then migrated with migrate-control --target-version 2. The database was not modified."
            }
            Self::RecordNameMismatch => {
                "A record file was altered or replaced after it was written; an interrupted write cannot produce this. Preserve the checkpoint directory, compare it with an independent copy or export, and investigate. This tool never removes the file."
            }
            Self::AuditNamespaceUnavailable => {
                "Run the explicit maintenance migrate-control --target-version 2 with the control stopped, then create the first checkpoint. The database was not modified."
            }
            Self::ScopeMismatch => {
                "Use the database, checkpoint log and trust anchor of the same tenant and control namespace. A recreated database is a new namespace and needs a new log and a new anchor."
            }
            Self::RecordMalformed | Self::RecordNotCanonical | Self::BundleMalformed => {
                "Do not trust this material. Obtain the records again from the checkpoint directory or a fresh export."
            }
            Self::SignatureInvalid => {
                "Do not trust this record. Preserve the checkpoint log and compare it with an independent copy or export."
            }
            Self::UntrustedKey => {
                "Only keys pinned in the trust anchor are accepted. For a key rotation, check the successor fingerprint through a separate channel, then run anchor approve-rotation."
            }
            Self::KeyNotValidAtIndex => {
                "The signing key was retired before this log position. Treat the record as untrusted and investigate the use of the retired key."
            }
            Self::KeyIdMismatch => {
                "Compare the fingerprints: the expected key, the key file and the record must name the same key."
            }
            Self::LogDiscontinuous => {
                "A checkpoint record is missing or does not link to its predecessor. Restore the complete checkpoint log from an independent copy."
            }
            Self::LogForked => {
                "Two complete records claim the same log position. Preserve both and investigate which writer produced them."
            }
            Self::LogRolledBack => {
                "The checkpoint log ends before the record accepted by the trust anchor. Treat it as a rollback: restore the missing records, or have the anchor holder decide on a documented break with a new log and a new anchor."
            }
            Self::LogDivergedFromReference => {
                "The record at the anchor reference position is not the accepted one. Treat the checkpoint log as replaced and investigate."
            }
            Self::CheckpointRegression => {
                "A checkpoint does not extend the previous signed position. Treat the checkpoint log as inconsistent and investigate."
            }
            Self::DatabaseTruncated => {
                "The audit database is shorter than a trusted signed position (detail.position_source and detail.signed_at_log_index name it). Treat it as a rollback or truncation: preserve it, and restore only by an explicit, documented decision."
            }
            Self::DatabaseDiverged => {
                "The audit database differs from a trusted signed position (detail.position_source and detail.signed_at_log_index name it). Treat its history as rewritten: preserve the database and the checkpoint records, and investigate."
            }
            Self::RotationNotEndorsed => {
                "The retiring key did not endorse this rotation. Approve it only as an explicit recovery with --unendorsed-recovery after an out-of-band decision by the anchor holder."
            }
            Self::NoIndependentReference => {
                "Record a reference with anchor advance after a verification you accept; without it a rollback to an earlier signed state cannot be detected."
            }
        }
    }
}

/// Action of a `scope_mismatch` refusal whose field is `control_genesis`.
pub(crate) const CONTROL_GENESIS_ACTION: &str = "The control genesis of an audit database is written once and never changes. Treat the database or the checkpoint record that names another one as altered: preserve the database and every checkpoint record, and investigate. A recreated database is a new namespace and needs a new log and a new anchor.";

/// One structured error of an error document or a verification report.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ErrorEntry {
    pub code: &'static str,
    pub message: String,
    pub detail: Map<String, Value>,
    pub action: String,
}

/// A failed command. It never carries key material or audit event bodies.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub class: FailureClass,
    pub error: ErrorEntry,
}

impl Failure {
    pub(crate) fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            class: code.class(),
            error: ErrorEntry {
                code: code.as_str(),
                message: message.into(),
                detail: Map::new(),
                action: code.action().into(),
            },
        }
    }

    pub(crate) fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.error.detail.insert(key.into(), value.into());
        self
    }

    pub(crate) fn action(mut self, action: impl Into<String>) -> Self {
        self.error.action = action.into();
        self
    }

    /// Attach the cause reported by a storage or parsing layer. Those layers
    /// describe policies and paths, never file contents.
    pub(crate) fn cause(self, error: &anyhow::Error) -> Self {
        self.with("reason", format!("{error:#}"))
    }

    pub fn code(&self) -> &'static str {
        self.error.code
    }

    pub fn is_refusal(&self) -> bool {
        self.class == FailureClass::Refusal
    }

    pub fn exit_code(&self) -> u8 {
        match self.class {
            FailureClass::Operational => 1,
            FailureClass::Refusal => 2,
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.error.code, self.error.message)
    }
}

impl std::error::Error for Failure {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Verified,
    Refused,
}

/// Whether a rollback of the log and database can be detected at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackDetection {
    /// The anchor carries an accepted reference: a log that ends before it is
    /// refused, and a compared database must hold the referenced position.
    /// Without a compared database, only the log is protected.
    AnchorReference,
    /// The anchor carries no reference yet: a rollback to an earlier signed
    /// state is not detectable.
    UnavailableNoReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Scope {
    pub tenant_id: String,
    pub namespace_uuid: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnchorIdentity {
    pub revision: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferenceSummary {
    pub log_index: u64,
    pub statement_sha256: String,
}

/// The last record whose signatures and continuity were verified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HeadSummary {
    pub log_index: u64,
    pub kind: String,
    pub statement_sha256: String,
    pub event_count: u64,
    pub head_hash: String,
    pub signing_key_id: String,
    /// Declared by the signer. This is not a trusted timestamp.
    pub created_at_declared: String,
}

/// Comparison of the audit database with the trusted signed positions: the
/// ones retained and referenced by the anchor and the one of every checkpoint
/// verified now. Every field other than `compared` is unknown when no
/// comparison was made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatabaseComparison {
    pub compared: bool,
    pub event_count: Option<u64>,
    pub head_hash: Option<String>,
    pub audit_schema_version: Option<u32>,
    /// Trusted signed positions examined, oldest first, up to and including
    /// the first one the database does not hold.
    pub positions_compared: Option<u64>,
    /// True when the database holds every trusted signed position.
    pub extends_checkpoint: Option<bool>,
    /// Events after the last trusted signed position: not covered by any
    /// signature until the next checkpoint.
    pub events_after_checkpoint: Option<u64>,
}

impl DatabaseComparison {
    pub(crate) fn not_compared() -> Self {
        Self {
            compared: false,
            event_count: None,
            head_hash: None,
            audit_schema_version: None,
            positions_compared: None,
            extends_checkpoint: None,
            events_after_checkpoint: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Warning {
    pub code: &'static str,
    pub message: String,
}

/// Result of an offline verification against a trust anchor.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VerificationReport {
    pub format: &'static str,
    pub verdict: Verdict,
    pub summary: String,
    pub scope: Scope,
    pub anchor: AnchorIdentity,
    pub reference: Option<ReferenceSummary>,
    pub rollback_detection: RollbackDetection,
    pub entries_checked: u64,
    pub signatures_verified: u64,
    /// Highest log position that is trusted: verified now, or accepted earlier
    /// through the anchor reference. Unknown when neither exists.
    pub verified_through_log_index: Option<u64>,
    pub head: Option<HeadSummary>,
    pub database: DatabaseComparison,
    pub incomplete_records: Vec<IncompleteRecord>,
    pub unexpected_entries: Vec<String>,
    pub warnings: Vec<Warning>,
    pub limits: Vec<String>,
    pub errors: Vec<ErrorEntry>,
}

impl VerificationReport {
    pub fn is_verified(&self) -> bool {
        self.verdict == Verdict::Verified
    }

    /// Codes of the reported errors, in order.
    pub fn error_codes(&self) -> Vec<&'static str> {
        self.errors.iter().map(|error| error.code).collect()
    }
}
