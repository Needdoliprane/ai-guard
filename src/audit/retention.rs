//! Audit retention: a verifiable archive of a closed prefix of a tenant hash
//! chain, written to a new private destination, then the selective reclaim
//! of that prefix's leaf events from the live store.
//!
//! The chain is never rewritten. Structural events (every reserved control
//! namespace, decisions, worker and usage facts) stay live because
//! projections reference them and the startup replays fold them. Only
//! ordinary events and the three human read audit kinds are reclaimable, and
//! the live store also keeps the boundary event of each segment and the
//! earliest event that names each session, agent, sandbox or project, so that
//! identity grants keep their exact prior-sighting check. Every reclaimed
//! event is accounted for by a `control.audit_retention.applied` event in the
//! chain itself and by its ledger row; the archive holds every event of its
//! range, structural ones included, so `verify` checks the chain end to end
//! from the archives and the live store.
//!
//! The capability is installed explicitly and offline after a verified new
//! backup, on store formats 13 to 15; no store format number changes. No tool
//! deletes an archive: losing one makes its prefix unverifiable, which
//! `verify` reports as `archive_missing`.
mod archive;
pub(crate) mod chain;
pub(crate) mod classify;
pub mod cli;
pub mod failure;
pub(crate) mod ledger;
pub mod policy;
pub(crate) mod snapshot;
mod store;

/// Signed positions that one segment may keep live; a denser log needs a
/// smaller segment.
pub(crate) const MAX_KEPT_POSITIONS: usize = 4096;

pub use archive::{
    ARCHIVE_FORMAT, ArchiveManifestV1, ArchivePurposeV1, ArchivedEventV1, ClassUsageV1,
    EVENTS_FILE, MANIFEST_FILE, ProducerV1, ReclaimSummaryV1,
};
pub use classify::ReclaimClassV1;
pub use failure::RetentionFailure;
pub use ledger::RetentionAppliedV1;
pub use policy::{AuditRetentionPolicyV1, MIN_AGE_FLOOR_SECONDS};
pub use store::{
    ArchiveSummaryV1, AuditUsageV1, PlannedRetentionV1, RetentionApplyRequest,
    RetentionInstallReportV1, RetentionLedgerV1, RetentionPlanV1, RetentionReceiptV1,
    RetentionSegmentSummaryV1, RetentionVerificationV1, VERIFICATION_FORMAT, verify_archive_chain,
};
