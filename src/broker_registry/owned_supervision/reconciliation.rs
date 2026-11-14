//! Exact recovery after an offline binding rotation. These are observations,
//! never credentials or permission to replay under another identity.
use super::*;
use crate::workload::WorkloadBindingReceiptV1;

pub const MAX_OWNED_RECONCILIATION_REQUEST_BYTES: usize = 96 * 1024;
pub const MAX_OWNED_RECONCILIATION_RESULT_BYTES: usize = 128 * 1024;
pub const MAX_OWNED_SUPERSESSION_BYTES: usize = 192 * 1024;
const REQUEST_DOMAIN: &str = "ai-guard:broker-supervision-owned:v2:reconciliation";

/// Public receipt of the exact durable binding transition. The Store alone
/// produces it from checked history after authenticating the current bearer.
/// It carries no digest and cannot authorize a subsequent operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedBindingRotationFenceV2 {
    pub previous_revision: u64,
    pub receipt: WorkloadBindingReceiptV1,
    /// Actual current source-owner incarnation, committed after rotation.
    /// Its receipt links recovery to a real exclusive reopen, not r+1 alone.
    pub owner_incarnation: String,
    pub previous_owner_incarnation: String,
    pub previous_incarnation_seq: i64,
    pub previous_incarnation_hash: String,
    pub incarnation_seq: i64,
    pub incarnation_hash: String,
}
impl OwnedBindingRotationFenceV2 {
    pub fn validate_for(
        &self,
        original: &OwnedBrokerIdentityViewV2,
        observation: &OwnedBrokerIdentityViewV2,
    ) -> OwnedChecked<()> {
        original.validate()?;
        observation.validate()?;
        check_progress_identity(original, observation)
            .map_err(|_| OwnedSupervisionErrorCodeV2::IdentityMismatch)?;
        let old = original.subject.binding_revision;
        let receipt = &self.receipt;
        check(
            self.previous_revision == old
                && old.checked_add(1) == Some(observation.subject.binding_revision)
                && receipt.version == 1
                && receipt.credential_id == original.subject.credential_id
                && receipt.revision == observation.subject.binding_revision
                && receipt.audit_seq > 0
                && registry_uuid(&receipt.audit_event_id).is_ok()
                && is_hex64(&receipt.audit_hash)
                && registry_uuid(&self.owner_incarnation).is_ok()
                && registry_uuid(&self.previous_owner_incarnation).is_ok()
                && self.owner_incarnation != self.previous_owner_incarnation
                && self.previous_incarnation_seq > 0
                && self.previous_incarnation_seq < receipt.audit_seq
                && is_hex64(&self.previous_incarnation_hash)
                && self.incarnation_seq > receipt.audit_seq
                && is_hex64(&self.incarnation_hash),
            OwnedSupervisionErrorCodeV2::IdentityMismatch,
        )
    }
}

/// The original Pending plus its exact local predecessor. A sequence number
/// supplied without this locally retained history is never enough.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OwnedReconciliationRequestV2 {
    Heartbeat {
        pending: Box<OwnedHeartbeatPendingV2>,
        previous: Option<Box<OwnedBrokerHeartbeatResultV2>>,
    },
    Checkpoint {
        pending: Box<OwnedCheckpointPendingV2>,
        previous: Option<Box<OwnedBrokerCheckpointResultV2>>,
    },
}
impl OwnedReconciliationRequestV2 {
    pub fn operation(&self) -> OwnedBrokerOperationV2 {
        match self {
            Self::Heartbeat { .. } => OwnedBrokerOperationV2::HeartbeatReconcile,
            Self::Checkpoint { .. } => OwnedBrokerOperationV2::CheckpointReconcile,
        }
    }
    pub fn identity(&self) -> &OwnedBrokerIdentityViewV2 {
        match self {
            Self::Heartbeat { pending, .. } => &pending.identity,
            Self::Checkpoint { pending, .. } => &pending.identity,
        }
    }
    pub fn operation_id(&self) -> &str {
        match self {
            Self::Heartbeat { pending, .. } => &pending.heartbeat.operation_id,
            Self::Checkpoint { pending, .. } => &pending.checkpoint.operation_id,
        }
    }
    pub fn registry_input_hash(&self) -> &str {
        match self {
            Self::Heartbeat { pending, .. } => &pending.registry_input_hash,
            Self::Checkpoint { pending, .. } => &pending.registry_input_hash,
        }
    }
    pub fn validate(&self) -> OwnedChecked<()> {
        let conflict = OwnedSupervisionErrorCodeV2::OperationConflict;
        match self {
            Self::Heartbeat { pending, previous } => {
                pending.validate()?;
                match previous {
                    Some(last) => {
                        last.validate()?;
                        check_progress_identity(&last.identity, &pending.identity).map_err(|_| conflict)?;
                        check(last.result.broker == pending.prior.broker, conflict)?;
                    }
                    None => check(previous_heartbeat_sequence(&pending.prior.broker,
                        &pending.identity.subject.incarnation_id) == 0, conflict)?,
                }
            }
            Self::Checkpoint { pending, previous } => {
                pending.validate()?;
                match previous {
                    Some(last) => {
                        last.validate()?;
                        check_progress_identity(&last.identity, &pending.identity).map_err(|_| conflict)?;
                        check(last.result.checkpoint.incarnation_id == pending.checkpoint.incarnation_id
                            && last.result.checkpoint.sequence == pending.checkpoint.previous_sequence,
                            conflict)?;
                    }
                    None => check(pending.checkpoint.previous_sequence == 0, conflict)?,
                }
            }
        }
        bounded(self, MAX_OWNED_RECONCILIATION_REQUEST_BYTES, conflict)
    }
    pub fn commitment(&self) -> OwnedChecked<String> {
        self.validate()?;
        hash(REQUEST_DOMAIN, self)
    }
}

/// Current head from the same authenticated transaction as the global
/// operation lookup. Absence here is only for the current incarnation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OwnedReconciliationHeadV2 {
    Heartbeat { view: BrokerViewV1, receipt: Option<BrokerMutationReceiptV1> },
    Checkpoint { view: BrokerViewV1, current: Option<Box<BrokerCheckpointResultV1>> },
}

/// Historical operation identity is deliberately separate from the current
/// observation. No ordinary result has its identity rewritten to r+1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OwnedHistoricalResultV2 {
    Heartbeat { result: Box<OwnedBrokerHeartbeatResultV2> },
    Checkpoint { result: Box<OwnedBrokerCheckpointResultV2> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum OwnedReconciliationStateV2 {
    CommittedExact { historical: OwnedHistoricalResultV2 },
    /// Produced only after an exact global operation-ID lookup returned no
    /// row in the current healthy transaction, never from n-1 alone.
    NonCommitted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnedReconciliationResultV2 {
    pub profile: String,
    pub version: u32,
    pub request_commitment: String,
    pub observation: OwnedBrokerIdentityViewV2,
    pub fence: OwnedBindingRotationFenceV2,
    pub head: OwnedReconciliationHeadV2,
    pub state: OwnedReconciliationStateV2,
}
impl OwnedReconciliationResultV2 {
    pub fn validate_for(&self, request: &OwnedReconciliationRequestV2) -> OwnedChecked<()> {
        let conflict = OwnedSupervisionErrorCodeV2::OperationConflict;
        require_profile(&self.profile, self.version)?;
        request.validate()?;
        check(self.request_commitment == request.commitment()?, conflict)?;
        self.fence.validate_for(request.identity(), &self.observation)?;
        match (request, &self.head, &self.state) {
            (OwnedReconciliationRequestV2::Heartbeat { pending, previous },
             OwnedReconciliationHeadV2::Heartbeat { view, receipt }, state) => {
                OwnedBrokerSelfViewV2::new(self.observation.clone(), view.clone(), receipt.clone())?;
                match state {
                    OwnedReconciliationStateV2::CommittedExact {
                        historical: OwnedHistoricalResultV2::Heartbeat { result }
                    } => {
                        pending.validate_result(result).map_err(|_| conflict)?;
                        check(result.result.replayed
                            && result.result.receipt.audit_seq < self.fence.receipt.audit_seq
                            && view.broker == result.result.broker
                            && receipt.as_ref() == Some(&result.result.receipt), conflict)?;
                    }
                    OwnedReconciliationStateV2::NonCommitted => {
                        check(view.broker == pending.prior.broker, conflict)?;
                        if let Some(last) = previous {
                            check(receipt.as_ref() == Some(&last.result.receipt), conflict)?;
                        } else {
                            check(previous_heartbeat_sequence(&view.broker,
                                &self.observation.subject.incarnation_id) == 0, conflict)?;
                        }
                    }
                    _ => return Err(conflict),
                }
            }
            (OwnedReconciliationRequestV2::Checkpoint { pending, previous },
             OwnedReconciliationHeadV2::Checkpoint { view, current }, state) => {
                validate_broker_view(&self.observation.subject, view)?;
                if let Some(current) = current {
                    current.checkpoint.validate().map_err(|_| conflict)?;
                    validate_receipt(&current.receipt, &self.observation.subject)?;
                    check(current.checkpoint.incarnation_id == self.observation.subject.incarnation_id
                        && current.receipt.grant_revision == self.observation.subject.grant_revision
                        && current.receipt.operation_id == current.checkpoint.operation_id, conflict)?;
                }
                match state {
                    OwnedReconciliationStateV2::CommittedExact {
                        historical: OwnedHistoricalResultV2::Checkpoint { result }
                    } => {
                        pending.validate_result(result).map_err(|_| conflict)?;
                        check(result.result.replayed && result.result.receipt.audit_seq < self.fence.receipt.audit_seq
                            && current.as_ref().is_some_and(|head|
                            head.checkpoint == result.result.checkpoint
                                && head.receipt == result.result.receipt), conflict)?;
                    }
                    OwnedReconciliationStateV2::NonCommitted => match (previous, current) {
                        (None, None) => {}
                        (Some(last), Some(head)) => check(
                            last.result.checkpoint == head.checkpoint && last.result.receipt == head.receipt,
                            conflict)?,
                        _ => return Err(conflict),
                    },
                    _ => return Err(conflict),
                }
            }
            _ => return Err(conflict),
        }
        bounded(self, MAX_OWNED_RECONCILIATION_RESULT_BYTES, conflict)
    }
    pub fn non_committed_for(&self, request: &OwnedReconciliationRequestV2) -> OwnedChecked<()> {
        self.validate_for(request)?;
        check(matches!(self.state, OwnedReconciliationStateV2::NonCommitted),
            OwnedSupervisionErrorCodeV2::OperationConflict)
    }
}

#[cfg(test)]
mod tests;
