//! Wire of the per-context 360° summary (`GET /api/v1/human/summary`).
//!
//! One store snapshot transaction of one tenant, recorded under the human read
//! action `summary`. Every count is a decimal string computed by the control
//! over its own durable registries or the verified journal; nothing is
//! extrapolated from a page. A section the context may not read is
//! `not_granted`, never omitted and never zero. A runtime inventory is never
//! claimed: the control counts what it recorded, not what an engine runs.
use super::*;
use serde::{Deserialize, Serialize};

/// Closed summary windows; the server resolves `from` at evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SummaryWindowV1 {
    #[serde(rename = "24h")]
    Day,
    #[serde(rename = "7d")]
    Week,
    #[serde(rename = "30d")]
    Month,
    #[serde(rename = "all")]
    All,
}
impl SummaryWindowV1 {
    pub fn duration(self) -> Option<chrono::Duration> {
        match self {
            Self::Day => Some(chrono::Duration::hours(24)),
            Self::Week => Some(chrono::Duration::days(7)),
            Self::Month => Some(chrono::Duration::days(30)),
            Self::All => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryPeriodV1 {
    pub window: SummaryWindowV1,
    /// Inclusive lower bound on decision `received_at` and allocation `finished_at`.
    /// It is absent for `all`.
    pub from: Option<String>,
    /// The evaluation instant and inclusive upper bound for the same fields.
    pub to: String,
}

/// What a count covers. `complete_registry`: every row of the control's own
/// registry in this tenant. `authorized_selection`: every row the context is
/// authorized to see. `verified_journal`: every matching event of the verified
/// audit journal in the period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryCoverageV1 {
    CompleteRegistry,
    AuthorizedSelection,
    VerifiedJournal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryUnavailableV1 {
    /// The adopted source profile has no protocol for this section.
    ProfileUnavailable,
    /// The store format predates the registry this section reads.
    MigrationRequired,
    /// The section's bounded evaluation exceeded its row budget.
    CapacityExceeded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum SummarySectionV1<T> {
    Ready {
        coverage: SummaryCoverageV1,
        data: T,
    },
    NotGranted,
    Unavailable {
        code: SummaryUnavailableV1,
    },
}

/// Count keyed by a closed label; values are decimal strings.
pub type SummaryCountsV1 = std::collections::BTreeMap<String, String>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryResourcesV1 {
    /// session, agent, sandbox, project.
    pub by_kind: SummaryCountsV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryDecisionsV1 {
    /// allow, deny, review: gateway decisions recorded in the period.
    pub by_outcome: SummaryCountsV1,
    /// False when the journal row budget ended before the period's start.
    pub complete: bool,
    /// Oldest counted reception when incomplete; the counts cover only
    /// decisions received from this instant to the evaluation.
    pub oldest_counted_at: Option<String>,
}

/// One launch authority group of the worker registry, joined by exact
/// identifier to the broker registry. `broker` is null when the authority is
/// unknown to the registry; its workers are still counted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryWorkerGroupV1 {
    pub association: String,
    pub runtime_kind: String,
    pub broker: Option<SummaryBrokerRefV1>,
    pub by_state: SummaryCountsV1,
    pub quota_held: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryBrokerRefV1 {
    pub broker_id: String,
    pub state: String,
    pub last_received_at: Option<String>,
    pub lease_expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryWorkersV1 {
    pub by_state: SummaryCountsV1,
    pub quota_held: String,
    /// At most `SUMMARY_MAX_GROUPS`, ordered by broker then authority.
    pub groups: Vec<SummaryWorkerGroupV1>,
    pub groups_complete: bool,
    pub runtime_inventory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryBrokersV1 {
    /// unseen, current, stale, expired, revoked, clock_uncertain.
    pub by_state: SummaryCountsV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryQuotaV1 {
    pub max_active_workers: String,
    pub max_cpu_millis: String,
    pub max_memory_bytes: String,
    pub held_workers: String,
    pub held_cpu_millis: String,
    pub held_memory_bytes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryUsageV1 {
    /// Terminal allocations observed in the period (method 1).
    pub allocations: String,
    pub duration_ns: String,
    pub cpu_millicore_nanoseconds: String,
    pub memory_byte_nanoseconds: String,
    pub method_version: u32,
    pub billable: bool,
}

/// A point of attention derived only from durable control states.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryAttentionItemV1 {
    pub kind: String,
    pub subject_kind: String,
    pub subject_id: String,
    /// The recorded instant of the fact, when the source has one.
    pub at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryAttentionV1 {
    pub by_kind: SummaryCountsV1,
    /// At most `SUMMARY_MAX_ATTENTION`, newest first.
    pub items: Vec<SummaryAttentionItemV1>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummarySectionsV1 {
    pub resources: SummarySectionV1<SummaryResourcesV1>,
    pub decisions: SummarySectionV1<SummaryDecisionsV1>,
    pub workers: SummarySectionV1<SummaryWorkersV1>,
    pub brokers: SummarySectionV1<SummaryBrokersV1>,
    pub quota: SummarySectionV1<SummaryQuotaV1>,
    pub usage: SummarySectionV1<SummaryUsageV1>,
    pub attention: SummarySectionV1<SummaryAttentionV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanSummaryV1 {
    pub version: u32,
    pub context_id: String,
    pub authorization_version: String,
    pub mode: ContextModeV1,
    pub evaluated_at: String,
    pub period: SummaryPeriodV1,
    pub consistency: String,
    pub sections: SummarySectionsV1,
    pub read_receipt: HumanReadReceiptV1,
}

pub const SUMMARY_MAX_GROUPS: usize = 100;
pub const SUMMARY_MAX_ATTENTION: usize = 50;

/// `GET /api/v1/human/admin/quota`: the tenant policy and its durable reservations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanQuotaV1 {
    pub version: u32,
    pub context_id: String,
    pub authorization_version: String,
    pub evaluated_at: String,
    pub source: String,
    pub quota: SummaryQuotaV1,
    pub max_timeout_seconds: String,
    pub read_receipt: HumanReadReceiptV1,
}

/// `GET /api/v1/human/admin/usage`: terminal allocations, method 1, never billable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanUsagePageV1 {
    pub version: u32,
    pub context_id: String,
    pub authorization_version: String,
    pub evaluated_at: String,
    pub source: String,
    pub method_version: u32,
    pub billable: bool,
    pub items: Vec<crate::control::AllocationV1>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub read_receipt: HumanReadReceiptV1,
}
