//! Held-CREATE over authenticated control HTTP with owned filesystem admission.
use super::{
    coordinator::{CoordinatorError, HeldCreateCompletion, pending_error, pre_send_unknown},
    journal::{AdmissionJournal, CandidateRefusal},
    journal_io::JournalIo,
    permission::{HeldCreateClaimOutcome, PendingHeldCreateClaim},
};
use crate::{
    broker::held_create_client::{
        HeldCreateClaimExchange, HeldCreateClientError, HeldCreateControlClient,
    },
    control::kubernetes_admission::{
        KubernetesCreateClaimResponseV1, KubernetesCreateOutcomeV1, KubernetesCreateResultV1,
        KubernetesCreateUnknownReasonV1,
    },
    runtime::kubernetes::KubernetesRuntime,
};
use std::sync::{Arc, atomic::AtomicBool};

pub(crate) struct RemoteHeldCreateCoordinator {
    client: Arc<HeldCreateControlClient>,
}
enum ClaimReply {
    Exchange(Box<HeldCreateClaimExchange>),
    Observed(Box<KubernetesCreateClaimResponseV1>),
}

impl RemoteHeldCreateCoordinator {
    pub(crate) fn new(client: Arc<HeldCreateControlClient>) -> Self {
        Self { client }
    }

    pub(crate) async fn execute(
        &self,
        runtime: &KubernetesRuntime,
        pending: PendingHeldCreateClaim,
        owned: JournalIo,
        cancelled: &AtomicBool,
    ) -> Result<HeldCreateCompletion, CoordinatorError> {
        if pending.intent() != owned.journal().intent()
            || runtime.binding() != &owned.journal().intent().authority
        {
            return Err(CoordinatorError::InvalidContext);
        }
        if owned.journal().candidate().is_some() {
            return self.resume_result(owned).await;
        }
        let client = Arc::clone(&self.client);
        let (owned, prepared) = owned
            .dispatch(move |journal| {
                let prepared: Result<_, CoordinatorError> = (|| {
                    validate_context(&client, journal)?;
                    let claim = journal.claim_request().clone();
                    let attempt = if journal.observation_only() {
                        None
                    } else {
                        Some(pending.remote_attempt(journal).map_err(pending_error)?)
                    };
                    Ok((pending, claim, attempt))
                })();
                Ok(prepared)
            })
            .await
            .map_err(|_| CoordinatorError::JournalUnavailable)?
            .map_err(|_| CoordinatorError::JournalUnavailable)?;
        let (pending, claim, attempt) = prepared?;
        let reply = match attempt {
            Some(attempt) => ClaimReply::Exchange(Box::new(
                self.client
                    .claim_once(attempt)
                    .await
                    .map_err(client_error)?,
            )),
            None => ClaimReply::Observed(Box::new(
                self.client
                    .observe_claim(&claim.create_id, &claim.operation_id)
                    .await
                    .map_err(client_error)?,
            )),
        };
        let (owned, accepted) = owned
            .dispatch(move |journal| {
                Ok(match reply {
                    ClaimReply::Exchange(reply) => pending.accept_remote_exchange(*reply, journal),
                    ClaimReply::Observed(reply) => pending.accept_remote_history(*reply, journal),
                })
            })
            .await
            .map_err(|_| CoordinatorError::JournalUnavailable)?
            .map_err(|_| CoordinatorError::JournalUnavailable)?;
        let (response, outcome) = match accepted.map_err(pending_error)? {
            HeldCreateClaimOutcome::Ready {
                response,
                permission,
            } => {
                let outcome = runtime
                    .create_held(permission, owned.journal(), cancelled)
                    .await
                    .unwrap_or_else(|error| pre_send_unknown(error, cancelled));
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
            operation_id: uuid::Uuid::new_v4().to_string(),
            create_id: claim.create_id,
            intent_hash: response.snapshot.intent_hash,
            claim_operation_id: claim.operation_id,
            expected_revision: response.snapshot.revision,
            expected_create_epoch: response.snapshot.create_epoch,
            outcome,
        };
        // No await/acquisition from Outcome through dispatch. The closure owns
        // journal + already-held capacity even if its waiter disappears.
        let retention = owned.dispatch(move |journal| journal.retain_candidate(&candidate, None));
        let (owned, _) = retention
            .await
            .map_err(|_| CoordinatorError::JournalUnavailable)?
            .map_err(|_| CoordinatorError::JournalUnavailable)?;
        self.resume_result(owned).await
    }

    /// Replay the exact retained result, or report an unrecoverable response
    /// after a historical claim. Reopening never requires activation and never
    /// grants permission to emit, inspect/adopt a Pod by name, or adjust CAS.
    pub(crate) async fn resume_result(
        &self,
        owned: JournalIo,
    ) -> Result<HeldCreateCompletion, CoordinatorError> {
        let client = Arc::clone(&self.client);
        let (owned, retained) = owned
            .dispatch(move |journal| {
                Ok(validate_context(&client, journal).and_then(|()| {
                    if journal.candidate().is_none() && !journal.observation_only() {
                        return Err(CoordinatorError::InvalidContext);
                    }
                    Ok((
                        journal.candidate().cloned(),
                        journal.claim_request().clone(),
                    ))
                }))
            })
            .await
            .map_err(|_| CoordinatorError::JournalUnavailable)?
            .map_err(|_| CoordinatorError::JournalUnavailable)?;
        let (candidate, claim) = retained?;
        let (owned, candidate) = if let Some(candidate) = candidate {
            (owned, candidate)
        } else {
            // Prepared-only recovery cannot know whether its Claim committed.
            // Only this read is allowed: a 404 must never become a new claim.
            let history = self
                .client
                .observe_claim(&claim.create_id, &claim.operation_id)
                .await
                .map_err(client_error)?;
            if history.snapshot.intent != *owned.journal().intent()
                || history.snapshot.target != *owned.journal().target()
                || history.snapshot.claim.as_ref() != Some(&claim)
            {
                return Err(CoordinatorError::InvalidContext);
            }
            let retention = owned.dispatch(move |journal| {
                let tenant_id = journal.tenant_id().to_owned();
                journal.retain_observed_claim(&tenant_id, &history)?;
                let candidate = KubernetesCreateResultV1 {
                    version: 1,
                    operation_id: uuid::Uuid::new_v4().to_string(),
                    create_id: claim.create_id,
                    intent_hash: history.snapshot.intent_hash,
                    claim_operation_id: claim.operation_id,
                    expected_revision: history.snapshot.revision,
                    expected_create_epoch: history.snapshot.create_epoch,
                    outcome: KubernetesCreateOutcomeV1::OwnershipUnknown {
                        reason: KubernetesCreateUnknownReasonV1::RecoveredAfterClaim,
                        http_status: None,
                    },
                };
                journal.retain_candidate(&candidate, None)?;
                Ok(candidate)
            });
            retention
                .await
                .map_err(|_| CoordinatorError::JournalUnavailable)?
                .map_err(|_| CoordinatorError::JournalUnavailable)?
        };
        match self.client.result(&candidate).await {
            Ok(confirmed) => {
                let receipt = confirmed.receipt().clone();
                let outcome = candidate.outcome;
                // Capacity also precedes Result HTTP: dispatch ACK without
                // another await/acquisition after receiving the response.
                let ack =
                    owned.dispatch(move |journal| journal.acknowledge_remote_result(confirmed));
                let (_, handle) = ack
                    .await
                    .map_err(|_| CoordinatorError::JournalUnavailable)?
                    .map_err(|_| CoordinatorError::JournalUnavailable)?;
                Ok(HeldCreateCompletion {
                    receipt,
                    outcome,
                    handle,
                })
            }
            Err(error) => {
                let observed = match error {
                    HeldCreateClientError::StaleEpoch => CandidateRefusal::StaleEpoch,
                    HeldCreateClientError::StaleRevision => CandidateRefusal::StaleRevision,
                    HeldCreateClientError::Transport
                    | HeldCreateClientError::Protocol
                    | HeldCreateClientError::Unavailable
                    | HeldCreateClientError::Capacity
                    | HeldCreateClientError::Clock
                    | HeldCreateClientError::UnsafeFilesystem
                    | HeldCreateClientError::InvalidRequest
                    | HeldCreateClientError::InvalidContext
                    | HeldCreateClientError::InvalidConfiguration => CandidateRefusal::Unavailable,
                    _ => CandidateRefusal::ControlRejected,
                };
                let refusal = owned.dispatch(move |journal| {
                    let first = journal.candidate_refusal().unwrap_or(observed);
                    journal.retain_candidate(&candidate, Some(first))
                });
                let _ = refusal
                    .await
                    .map_err(|_| CoordinatorError::JournalUnavailable)?
                    .map_err(|_| CoordinatorError::JournalUnavailable)?;
                Err(client_error(error))
            }
        }
    }
}

fn validate_context(
    client: &HeldCreateControlClient,
    journal: &AdmissionJournal,
) -> Result<(), CoordinatorError> {
    let scope = client.expected_scope();
    let intent = journal.intent();
    if scope.tenant_id != journal.tenant_id()
        || scope.control_namespace_uuid != intent.control_namespace_uuid
        || scope.broker_id != intent.broker_id
        || scope.incarnation_id != intent.incarnation_id
    {
        return Err(CoordinatorError::InvalidContext);
    }
    let prepared = super::prepare(intent).map_err(|_| CoordinatorError::InvalidContext)?;
    if prepared.target() != journal.target() {
        return Err(CoordinatorError::InvalidContext);
    }
    journal
        .revalidate_retained()
        .map_err(|_| CoordinatorError::JournalUnavailable)
}

fn client_error(error: HeldCreateClientError) -> CoordinatorError {
    match error {
        HeldCreateClientError::Transport
        | HeldCreateClientError::Protocol
        | HeldCreateClientError::Unavailable
        | HeldCreateClientError::Capacity
        | HeldCreateClientError::Clock
        | HeldCreateClientError::UnsafeFilesystem => CoordinatorError::ControlUnavailable,
        HeldCreateClientError::InvalidConfiguration
        | HeldCreateClientError::InvalidContext
        | HeldCreateClientError::InvalidRequest => CoordinatorError::InvalidContext,
        _ => CoordinatorError::ControlRejected,
    }
}
