//! Signed checkpoints of a tenant audit chain and their offline verification.
//!
//! The audit chain of `Store` is an unkeyed SHA-256 chain verified from the
//! database itself: whoever can rewrite the SQLite file can rewrite a coherent
//! chain. A checkpoint is an Ed25519 signature over a versioned, canonical
//! statement that binds the tenant, the real control namespace, a log
//! position, the chain head and the previous checkpoint. Records are immutable
//! files kept outside the audited database.
//!
//! Trust never comes from the verified material. Only keys pinned in a trust
//! anchor, held outside the controlled storage, are accepted, and rollback is
//! detected only against the reference recorded in that anchor: a compared
//! database must hold the referenced position and every signed position
//! verified after it. Nothing here
//! resists an administrator who controls the database, the signing key and the
//! trust anchor at the same time. This is unrelated to broker business
//! checkpoints and to the OIDC authentication ledger.

mod anchor;
mod bundle;
pub mod cli;
mod create;
mod diagnose;
mod evidence;
mod files;
mod format;
mod keys;
mod log;
mod report;
#[cfg(test)]
mod tests;
mod verify;

pub use anchor::{
    AdvanceOutcome, AdvanceStatus, AnchorAdvanceRequest, AnchorCheckpoint, AnchorInitRequest,
    AnchorKey, AnchorOutcome, AnchorReference, RotationApprovalRequest, TrustAnchor,
    advance_anchor, approve_rotation, init_anchor,
};
pub use bundle::{ExportOutcome, ExportRequest, export_bundle};
pub use create::{
    CreateOutcome, CreateRequest, CreateStatus, RetiringKey, RotateOutcome, RotateRequest,
    RotateStatus, create_checkpoint, rotate_key,
};
pub use diagnose::{
    AnchorDiagnostic, DatabaseDiagnostic, DiagnoseRequest, DiagnosticReport, Finding,
    KeyDiagnostic, LogDiagnostic, diagnose,
};
pub use evidence::AuditEvidence;
pub use format::{
    CheckpointStatement, RecordSignature, RotationStatement, SignedRecord, Statement,
};
pub use keys::{KeyOutcome, PublicKeyDocument, init_key, show_key};
pub use log::IncompleteRecord;
pub use report::{
    AnchorIdentity, DatabaseComparison, ErrorEntry, Failure, FailureClass, HeadSummary,
    ReferenceSummary, RollbackDetection, Scope, Verdict, VerificationReport, Warning,
};
pub use verify::{RecordSource, VerifyRequest, verify};

/// `type` of every signed statement.
pub const STATEMENT_TYPE: &str = "ai-guard.audit.checkpoint";
/// Version of the signed statement contract.
pub const STATEMENT_VERSION: u32 = 1;
/// Envelope stored as one immutable record file.
pub const RECORD_FORMAT: &str = "ai-guard.audit.signed-checkpoint.v1";
/// Public half of a signing key. It is an input to an explicit approval, never
/// a root of trust by itself.
pub const PUBLIC_KEY_FORMAT: &str = "ai-guard.audit.signing-public-key.v1";
/// Trust anchor held outside the controlled storage.
pub const ANCHOR_FORMAT: &str = "ai-guard.audit.trust-anchor.v1";
/// Public proof bundle. It carries no key.
pub const PROOF_FORMAT: &str = "ai-guard.audit.checkpoint-proof.v1";
/// Verification report.
pub const VERIFICATION_FORMAT: &str = "ai-guard.audit.checkpoint-verification.v1";
/// Error document of every command other than a completed verification.
pub const ERROR_FORMAT: &str = "ai-guard.audit.checkpoint-error.v1";
/// Result document of a successful command.
pub const COMMAND_FORMAT: &str = "ai-guard.audit.checkpoint-command.v1";
/// Diagnostic report. It never carries a trust decision.
pub const DIAGNOSTIC_FORMAT: &str = "ai-guard.audit.checkpoint-diagnostic.v1";
/// Domain separation prefix of the signed message.
pub const SIGNATURE_DOMAIN: &[u8] = b"ai-guard:audit-signed-checkpoint:v1\0";
/// Name of the unkeyed chain construction whose head is signed.
pub const CHAIN_ALGORITHM: &str = "agentsandbox:audit:v1";
/// Only signature algorithm of version 1.
pub const SIGNATURE_ALGORITHM: &str = "ed25519";
/// Largest accepted record file, in bytes.
pub const MAX_RECORD_BYTES: u64 = 2048;
/// Largest number of record files in one checkpoint directory. Beyond it the
/// log continues in a new directory linked to the last record.
pub const MAX_LOG_RECORDS: u32 = 8192;
/// Record files beyond `MAX_LOG_RECORDS` that only rotation records may use,
/// so that a full directory never prevents retiring a lost or compromised key.
pub const ROTATION_RESERVE_RECORDS: u32 = 16;
/// Largest number of entries in one proof bundle: every record that a full
/// checkpoint directory can hold. `export` never writes more, so that it never
/// writes a bundle that `verify` refuses to read.
pub const MAX_BUNDLE_ENTRIES: u32 = MAX_LOG_RECORDS + ROTATION_RESERVE_RECORDS;
/// Largest number of keys pinned by one trust anchor.
pub const MAX_ANCHOR_KEYS: usize = 512;
/// Largest number of earlier signed positions a trust anchor retains while its
/// reference advances without a database comparison. Beyond it, the reference
/// only advances with a database.
pub const MAX_RETAINED_CHECKPOINTS: usize = 64;
/// Largest accepted trust anchor file, in bytes.
pub const MAX_ANCHOR_BYTES: u64 = 256 * 1024;
/// Largest accepted public key document, in bytes.
pub const MAX_PUBLIC_KEY_BYTES: u64 = 1024;
/// Largest accepted proof bundle, in bytes.
pub const MAX_BUNDLE_BYTES: u64 = 20 * 1024 * 1024;
