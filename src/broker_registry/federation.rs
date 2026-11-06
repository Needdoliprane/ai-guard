//! Descriptive associations, separate from the strict WorkerV1 wire contract.
use super::{BrokerRuntimeKindV1, BrokerStateV1};
use crate::control::WorkerV1;
use serde::{Deserialize, Serialize};

pub const MAX_FEDERATION_PAGE_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityBrokerStatusV1 {
    RegistryUnavailable,
    AuthorityUnknown,
    Unregistered,
    Registered,
}

/// Current registry metadata, without credentials or host paths. The current
/// incarnation is not evidence of the incarnation at a worker's launch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBrokerV1 {
    pub broker_id: String,
    pub current_incarnation_id: String,
    pub state: BrokerStateV1,
    pub grant_revision: String,
    pub heartbeat_revision: String,
    pub last_received_at: Option<String>,
    pub lease_expires_at: Option<String>,
}

/// Historical ownership of an authority label, which can be registered after
/// the worker fact. This never proves which broker executed or inspected it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityBrokerAssociationV1 {
    pub status: AuthorityBrokerStatusV1,
    pub broker: Option<FederationBrokerV1>,
    pub runtime_kind: Option<BrokerRuntimeKindV1>,
    pub runtime_matches: Option<bool>,
    pub in_current_grant: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationWorkerV1 {
    pub worker: WorkerV1,
    pub launch_broker: AuthorityBrokerAssociationV1,
    pub observation_broker: AuthorityBrokerAssociationV1,
}

/// One coherent checked transaction per page; successive keyset pages are live
/// reads and cannot imply a frozen tenant snapshot or a complete inventory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationWorkerPageV1 {
    pub version: u32,
    pub source: String,
    pub consistency: String,
    pub evaluated_at: String,
    pub runtime_inventory: String,
    /// Structural schema absence only; I/O or integrity failures are errors.
    pub registry_status: String,
    pub items: Vec<FederationWorkerV1>,
    pub next_cursor: Option<String>,
}
