//! Wire projection of confirmed STOP metadata; contains no business payload.
use super::protocol::{JournalModeV1, RecoveryDataSourceV1, RecoveryRuntimeEvidenceV1};
use crate::broker::journal::{
    JournalError,
    recovery::{StopRecoveryPhaseV1, inventory::StopInventoryPageV1},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryInventoryCoverageV1 {
    AcceptedStopMetadata,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryInventoryPhaseV1 {
    Inventory,
    SharedScope,
    Job,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryInventoryIssueCodeV1 {
    InvalidInput,
    UnsupportedVersion,
    TooLarge,
    LimitExceeded,
    Busy,
    Conflict,
    Interrupted,
    Corrupt,
    UnsafeFilesystem,
    ControlSecret,
    Io,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryInventoryEntryV1 {
    pub operation_id: Uuid,
    pub head_revision: u64,
    pub head_hash: String,
    pub incomplete: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryInventoryIssueV1 {
    pub operation_id: Option<Uuid>,
    pub phase: RecoveryInventoryPhaseV1,
    pub code: RecoveryInventoryIssueCodeV1,
}

/// Each head is independently confirmed. No runtime probe, transactional
/// global snapshot, total activity count or action permission is implied.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryInventoryV1 {
    pub version: u32,
    pub tenant_id: String,
    pub control_namespace_uuid: Uuid,
    pub profile_id: String,
    pub journal_mode: JournalModeV1,
    pub source: RecoveryDataSourceV1,
    pub coverage: RecoveryInventoryCoverageV1,
    pub runtime_evidence: RecoveryRuntimeEvidenceV1,
    pub incomplete: bool,
    pub reconciliation_required: bool,
    pub entries: Vec<RecoveryInventoryEntryV1>,
    pub issues: Vec<RecoveryInventoryIssueV1>,
    pub issues_truncated: bool,
    pub next_after: Option<Uuid>,
}

pub(super) fn project(page: &StopInventoryPageV1, mode: JournalModeV1) -> RecoveryInventoryV1 {
    RecoveryInventoryV1 {
        version: 1,
        tenant_id: page.scope().tenant_id().into(),
        control_namespace_uuid: page.scope().control_namespace_uuid(),
        profile_id: page.scope().profile_id().into(),
        journal_mode: mode,
        source: RecoveryDataSourceV1::ConfirmedJournal,
        coverage: RecoveryInventoryCoverageV1::AcceptedStopMetadata,
        runtime_evidence: RecoveryRuntimeEvidenceV1::NotQueried,
        incomplete: page.incomplete(),
        reconciliation_required: mode == JournalModeV1::StopOnly || page.incomplete(),
        entries: page
            .entries()
            .iter()
            .map(|entry| RecoveryInventoryEntryV1 {
                operation_id: entry.operation_id(),
                head_revision: entry.head().revision(),
                head_hash: entry.head().hash().into(),
                incomplete: entry.incomplete(),
            })
            .collect(),
        issues: page
            .issues()
            .iter()
            .map(|issue| RecoveryInventoryIssueV1 {
                operation_id: issue.operation_id(),
                phase: match issue.phase() {
                    StopRecoveryPhaseV1::Inventory => RecoveryInventoryPhaseV1::Inventory,
                    StopRecoveryPhaseV1::SharedScope => RecoveryInventoryPhaseV1::SharedScope,
                    StopRecoveryPhaseV1::Job => RecoveryInventoryPhaseV1::Job,
                },
                code: match issue.error() {
                    JournalError::InvalidInput => RecoveryInventoryIssueCodeV1::InvalidInput,
                    JournalError::UnsupportedVersion => {
                        RecoveryInventoryIssueCodeV1::UnsupportedVersion
                    }
                    JournalError::TooLarge => RecoveryInventoryIssueCodeV1::TooLarge,
                    JournalError::LimitExceeded => RecoveryInventoryIssueCodeV1::LimitExceeded,
                    JournalError::Busy => RecoveryInventoryIssueCodeV1::Busy,
                    JournalError::Conflict => RecoveryInventoryIssueCodeV1::Conflict,
                    JournalError::Interrupted => RecoveryInventoryIssueCodeV1::Interrupted,
                    JournalError::Corrupt => RecoveryInventoryIssueCodeV1::Corrupt,
                    JournalError::UnsafeFilesystem => {
                        RecoveryInventoryIssueCodeV1::UnsafeFilesystem
                    }
                    JournalError::ControlSecret => RecoveryInventoryIssueCodeV1::ControlSecret,
                    JournalError::Io => RecoveryInventoryIssueCodeV1::Io,
                },
            })
            .collect(),
        issues_truncated: page.issues_truncated(),
        next_after: page.next_after(),
    }
}

#[cfg(test)]
#[path = "recovery_inventory_protocol_tests.rs"]
mod tests;
