//! Closed refusal codes of audit retention. A refusal never leaves a partial
//! effect on the live store; the detail names what was kept (for example an
//! archive written before a refused commit).
use serde_json::{Map, Value};

#[derive(Debug, Clone)]
pub struct RetentionFailure {
    pub code: &'static str,
    pub message: String,
    pub detail: Map<String, Value>,
}

impl RetentionFailure {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: Map::new(),
        }
    }

    pub(crate) fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.detail.insert(key.to_owned(), value.into());
        self
    }
}

impl std::fmt::Display for RetentionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RetentionFailure {}

/// Codes of the operator contract (`docs/setup/audit-retention`).
pub mod code {
    pub const CAPABILITY_MISSING: &str = "retention_capability_missing";
    pub const CAPABILITY_PRESENT: &str = "retention_capability_present";
    pub const FORMAT_UNSUPPORTED: &str = "retention_requires_format";
    pub const POLICY_INVALID: &str = "policy_invalid";
    pub const PLAN_MISMATCH: &str = "plan_mismatch";
    pub const NOTHING_TO_RECLAIM: &str = "nothing_to_reclaim";
    pub const RANGE_INVALID: &str = "range_invalid";
    pub const AUDIT_CHAIN_INVALID: &str = "audit_chain_invalid";
    pub const DESTINATION_NOT_NEW: &str = "archive_destination_not_new";
    pub const DESTINATION_UNAVAILABLE: &str = "archive_destination_unavailable";
    pub const DESTINATION_RESERVED: &str = "archive_destination_reserved";
    pub const ARCHIVE_WRITE_FAILED: &str = "archive_write_failed";
    pub const ARCHIVE_UNVERIFIED: &str = "archive_unverified";
    pub const COMMIT_UNCONFIRMED: &str = "retention_commit_unconfirmed";
    pub const RECLAIM_REFERENCED_EVENT: &str = "reclaim_referenced_event";
    pub const SNAPSHOT_TOO_LARGE: &str = "snapshot_too_large";
    pub const ARCHIVE_MISSING: &str = "archive_missing";
    pub const ARCHIVE_UNAVAILABLE: &str = "archive_unavailable";
    pub const ARCHIVE_MANIFEST_INVALID: &str = "archive_manifest_invalid";
    pub const ARCHIVE_MANIFEST_MISMATCH: &str = "archive_manifest_mismatch";
    pub const ARCHIVE_DIGEST_MISMATCH: &str = "archive_digest_mismatch";
    pub const ARCHIVE_CHAIN_INVALID: &str = "archive_chain_invalid";
    pub const RETAINED_EVENT_DIVERGES: &str = "retained_event_diverges";
    pub const UNACCOUNTED_LOSS: &str = "unaccounted_loss";
    pub const DUPLICATE_EVENT_ID: &str = "duplicate_event_id";
    pub const SNAPSHOT_MISMATCH: &str = "snapshot_mismatch";
    pub const EXPORT_DIVERGES: &str = "export_diverges";
    pub const EVENT_PREDATES_ARCHIVE: &str = "event_predates_archive";
    pub const CHECKPOINT_LOG_UNAVAILABLE: &str = "checkpoint_log_unavailable";
}

/// The refusal carried by an error chain, if any.
pub fn refusal(error: &anyhow::Error) -> Option<&RetentionFailure> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<RetentionFailure>())
}
