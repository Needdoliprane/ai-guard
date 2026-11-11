//! Local trusted composition of held-CREATE retention and central confirmation.
//! No production activation or authority constructor. Store and journal calls
//! remain synchronous: a future service caller must own bounded control-worker
//! admission; this module does not isolate blocking I/O from Tokio workers.
use super::{
    journal::{AdmissionJournal, CandidateRefusal},
    permission::{HeldCreateClaimOutcome, PendingHeldCreateClaim},
};
use crate::{
    audit::{KubernetesCreateAuthorization, Store},
    broker_registry::BrokerIdentityV1,
    control::kubernetes_admission::{
        KubernetesCreateClaimV1, KubernetesCreateFailureV1, KubernetesCreateOutcomeV1,
        KubernetesCreateReceiptV1, KubernetesCreateResultV1, KubernetesCreateUnknownReasonV1,
    },
    runtime::kubernetes::{KubernetesError, KubernetesPodHandle, KubernetesRuntime},
};
use std::{
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CoordinatorError {
    InvalidContext,
    ControlRejected,
    ControlUnavailable,
    JournalUnavailable,
}
impl fmt::Display for CoordinatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidContext => {
                "held CREATE coordinator context differs from the retained request"
            }
            Self::ControlRejected => {
                "held CREATE control rejected the retained operation; reconciliation is required"
            }
            Self::ControlUnavailable => {
                "held CREATE control confirmation is unavailable; retained state must be inspected"
            }
            Self::JournalUnavailable => {
                "held CREATE journal durability is unavailable; reconciliation is required"
            }
        })
    }
}
impl std::error::Error for CoordinatorError {}

pub(crate) struct HeldCreateCompletion {
    pub(crate) receipt: KubernetesCreateReceiptV1,
    pub(crate) outcome: KubernetesCreateOutcomeV1,
    pub(crate) handle: Option<KubernetesPodHandle>,
}
impl fmt::Debug for HeldCreateCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HeldCreateCompletion(<retained descriptive result>)")
    }
}

pub(crate) struct HeldCreateCoordinator<'a> {
    store: &'a Store,
    identity: &'a BrokerIdentityV1,
    authorization: &'a KubernetesCreateAuthorization,
    #[cfg(test)]
    lose_result_ack: AtomicBool,
}
impl<'a> HeldCreateCoordinator<'a> {
    /// Hold existing control capabilities; this constructs no authority.
    pub(crate) fn new(
        store: &'a Store,
        identity: &'a BrokerIdentityV1,
        authorization: &'a KubernetesCreateAuthorization,
    ) -> Self {
        Self {
            store,
            identity,
            authorization,
            #[cfg(test)]
            lose_result_ack: AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    pub(crate) fn lose_result_ack_once_for_test(&self) {
        self.lose_result_ack
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub(crate) async fn execute(
        &self,
        runtime: &KubernetesRuntime,
        pending: PendingHeldCreateClaim,
        claim: &KubernetesCreateClaimV1,
        journal: &mut AdmissionJournal,
        cancelled: &AtomicBool,
    ) -> Result<HeldCreateCompletion, CoordinatorError> {
        // Check the supplied invocation even when a retained candidate takes
        // priority. A divergent Pending must not silently resume another intent.
        if pending.intent() != journal.intent()
            || runtime.binding() != &journal.intent().authority
            || claim != journal.claim_request()
        {
            return Err(CoordinatorError::InvalidContext);
        }
        self.validate_context(journal)?;
        if journal.candidate().is_some() {
            return self.resume_result(journal);
        }

        // A failed Pending claim is not an observation and cannot manufacture a
        // candidate. The claim method owns its exact Store/durability checks.
        let (response, outcome) = match pending
            .claim(
                self.store,
                self.identity,
                self.authorization,
                claim,
                journal,
            )
            .map_err(pending_error)?
        {
            HeldCreateClaimOutcome::Ready {
                response,
                permission,
            } => {
                let outcome = match runtime.create_held(permission, journal, cancelled).await {
                    Ok(outcome) => outcome,
                    Err(error) => pre_send_unknown(error, cancelled),
                };
                (response, outcome)
            }
            HeldCreateClaimOutcome::ObserveOnly { response } => (
                response,
                KubernetesCreateOutcomeV1::OwnershipUnknown {
                    reason: KubernetesCreateUnknownReasonV1::RecoveredAfterClaim,
                    http_status: None,
                },
            ),
        };
        let candidate = KubernetesCreateResultV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            create_id: journal.intent().create_id.clone(),
            intent_hash: response.snapshot.intent_hash,
            claim_operation_id: claim.operation_id.clone(),
            expected_revision: response.snapshot.revision,
            expected_create_epoch: response.snapshot.create_epoch,
            outcome,
        };
        // No await from the runtime outcome through this synchronous durable
        // retention. Process death before it still leaves possible emission,
        // never a promise that this candidate reached stable storage.
        journal
            .retain_candidate(&candidate, None)
            .map_err(|_| CoordinatorError::JournalUnavailable)?;
        self.resume_result(journal)
    }

    pub(crate) fn resume_result(
        &self,
        journal: &mut AdmissionJournal,
    ) -> Result<HeldCreateCompletion, CoordinatorError> {
        self.validate_context(journal)?;
        let candidate = journal
            .candidate()
            .ok_or(CoordinatorError::InvalidContext)?
            .clone();
        // The Store revalidates current identity, grant and authorization even
        // for historical replay. This neither claims nor reactivates control.
        let confirmed = match self.store.record_kubernetes_create_result(
            self.identity,
            self.authorization,
            &candidate,
        ) {
            Ok(confirmed) => confirmed,
            Err(error) => {
                let refusal = match error.downcast_ref::<KubernetesCreateFailureV1>() {
                    Some(KubernetesCreateFailureV1::StaleEpoch) => CandidateRefusal::StaleEpoch,
                    Some(KubernetesCreateFailureV1::StaleRevision) => {
                        CandidateRefusal::StaleRevision
                    }
                    Some(KubernetesCreateFailureV1::Unavailable) | None => {
                        CandidateRefusal::Unavailable
                    }
                    Some(_) => CandidateRefusal::ControlRejected,
                };
                return retain_result_refusal(journal, &candidate, refusal, control_error(error));
            }
        };
        #[cfg(test)]
        if self.lose_result_ack.swap(false, Ordering::AcqRel) {
            // Inject only loss of the confirmation after a real durable commit.
            // The exact candidate stays retained; no SQL or receipt is forged.
            drop(confirmed);
            return retain_result_refusal(
                journal,
                &candidate,
                CandidateRefusal::Unavailable,
                CoordinatorError::ControlUnavailable,
            );
        }
        let receipt = confirmed.receipt().clone();
        let handle = journal
            .acknowledge_result(confirmed)
            .map_err(|_| CoordinatorError::JournalUnavailable)?;
        Ok(HeldCreateCompletion {
            receipt,
            outcome: candidate.outcome,
            handle,
        })
    }

    fn validate_context(&self, journal: &AdmissionJournal) -> Result<(), CoordinatorError> {
        if self.store.tenant_id() != self.identity.tenant_id()
            || journal.tenant_id() != self.store.tenant_id()
            || journal.intent().broker_id != self.identity.broker_id()
        {
            return Err(CoordinatorError::InvalidContext);
        }
        journal
            .intent()
            .validate()
            .map_err(|_| CoordinatorError::InvalidContext)?;
        let prepared =
            super::prepare(journal.intent()).map_err(|_| CoordinatorError::InvalidContext)?;
        if prepared.target() != journal.target() {
            return Err(CoordinatorError::InvalidContext);
        }
        let control = self.store.supervision_state().map_err(control_error)?;
        if !matches!(control.schema_version, 8 | 9 | 10 | 11 | 12 | 13)
            || control.control_namespace_uuid.as_deref()
                != Some(journal.intent().control_namespace_uuid.as_str())
        {
            return Err(CoordinatorError::InvalidContext);
        }
        journal
            .revalidate_retained()
            .map_err(|_| CoordinatorError::JournalUnavailable)
    }
}

fn retain_result_refusal(
    journal: &mut AdmissionJournal,
    candidate: &KubernetesCreateResultV1,
    observed: CandidateRefusal,
    error: CoordinatorError,
) -> Result<HeldCreateCompletion, CoordinatorError> {
    // Retain the first historical diagnosis, even if this attempt reports a
    // different refusal. The journal still rejects changed candidate bytes/CAS.
    let first = journal.candidate_refusal().unwrap_or(observed);
    journal
        .retain_candidate(candidate, Some(first))
        .map_err(|_| CoordinatorError::JournalUnavailable)?;
    Err(error)
}

pub(super) fn pending_error(error: KubernetesError) -> CoordinatorError {
    match error {
        KubernetesError::HeldCreateJournalUnavailable => CoordinatorError::JournalUnavailable,
        KubernetesError::HeldCreateControlUnavailable => CoordinatorError::ControlUnavailable,
        _ => CoordinatorError::ControlRejected,
    }
}
fn control_error(error: anyhow::Error) -> CoordinatorError {
    match error.downcast_ref::<KubernetesCreateFailureV1>() {
        Some(KubernetesCreateFailureV1::Unavailable) | None => CoordinatorError::ControlUnavailable,
        Some(_) => CoordinatorError::ControlRejected,
    }
}
pub(super) fn pre_send_unknown(
    error: KubernetesError,
    cancelled: &AtomicBool,
) -> KubernetesCreateOutcomeV1 {
    let reason = if cancelled.load(Ordering::Acquire) {
        KubernetesCreateUnknownReasonV1::CancelledAfterClaim
    } else if matches!(
        error,
        KubernetesError::InvalidCredentials
            | KubernetesError::InvalidAuthority
            | KubernetesError::AuthorityChanged
            | KubernetesError::NamespaceNotActive
    ) {
        KubernetesCreateUnknownReasonV1::AuthorityChanged
    } else {
        KubernetesCreateUnknownReasonV1::TransportUnknown
    };
    KubernetesCreateOutcomeV1::OwnershipUnknown {
        reason,
        http_status: None,
    }
}
