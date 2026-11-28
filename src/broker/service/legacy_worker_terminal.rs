//! Recover the terminal observation of an already retained Legacy target.
use super::*;
use crate::{
    broker::{
        journal::{
            JournalScopeV1, RevisionRefV1,
            recovery::legacy_worker_terminal::{
                ConfirmedLegacyWorkerTerminalObservationV1, LegacyWorkerTerminalEvidenceV1,
                LegacyWorkerTerminalRecoveryStateV1, ReservedLegacyWorkerTerminalObservationV1,
                RetainedLegacyWorkerTerminalObservationV1,
            },
        },
        writer::StopWriterClient,
    },
    control::{RuntimeStatusV1, WorkerMutationV1, WorkerStateV1, WorkerTransitionV1, WorkerV1},
    runner::ColdAuditBudget,
    runtime::{ContainerObservation, InvocationBindingV1, ManagedRuntime, Runtime, RuntimeKind},
};
use BrokerErrorCodeV1 as Code;

/// A private snapshot of the target at the matching Core read. The current
/// Worker matched the retained Worker in full. Before any later observation or
/// publication, the consumer must recheck scope, head, invocation and the
/// Legacy recovery owner on its restricted path.
pub(super) struct ValidatedLegacyWorkerTerminal {
    evidence: LegacyWorkerTerminalEvidenceV1,
}

/// Private observation of the exact retained target. A later publisher must
/// check the current head and authorization again at its own durable boundary.
pub(super) struct ObservedLegacyWorkerTerminal {
    validated: ValidatedLegacyWorkerTerminal,
    observation: ContainerObservation,
}

impl ObservedLegacyWorkerTerminal {
    pub(super) fn evidence(&self) -> &LegacyWorkerTerminalEvidenceV1 {
        self.validated.evidence()
    }

    pub(super) fn observation(&self) -> &ContainerObservation {
        &self.observation
    }

    /// Derive the observation mutation from the retained claim. The claim ID
    /// keeps the central operation stable across multiple STOP requests for
    /// this attempt. A publisher must retain the first exact body before send.
    pub(super) fn terminal_mutation(&self) -> Result<WorkerMutationV1, Code> {
        let evidence = self.evidence();
        let worker = evidence.worker();
        let operation_id = evidence
            .terminal_observation_operation_id()
            .map_err(|_| Code::Conflict)?;
        let observation = crate::runner::broker_worker_observation(worker, self.observation())
            .map_err(|_| Code::NeedsReconciliation)?;
        let mutation = WorkerMutationV1 {
            version: 1,
            operation_id,
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            expected_revision: worker.revision,
            transition: WorkerTransitionV1::Observe { observation },
        };
        let projected = crate::audit::expected_worker_transition(worker, &mutation)
            .map_err(|_| Code::Conflict)?;
        if projected.state != WorkerStateV1::Terminal || projected.quota_held {
            return Err(Code::NeedsReconciliation);
        }
        Ok(mutation)
    }
}

impl ValidatedLegacyWorkerTerminal {
    pub(super) fn evidence(&self) -> &LegacyWorkerTerminalEvidenceV1 {
        &self.evidence
    }

    /// Check a later runtime observation against the exact retained Created
    /// snapshot and Worker claim. This is a pure validator: it returns no
    /// runtime handle, mutation, or permission to create or start anything.
    pub(super) fn validate_terminal_observation(
        &self,
        observed: &ContainerObservation,
    ) -> Result<(), Code> {
        let (authority, created) = self.evidence.target();
        let worker = self.evidence.worker();
        let created_id = created.handle.id();
        if created.handle.authority_fingerprint() != authority.fingerprint()
            || worker.admission_operation_id != self.evidence.operation_id().to_string()
            || worker.attempt.container_id.as_deref() != Some(created_id)
            || worker
                .attempt
                .created_inspection
                .as_ref()
                .is_none_or(|inspection| inspection.container_id != created_id)
        {
            return Err(Code::Conflict);
        }

        let terminal_status = observed.process.status.as_deref() == Some("exited")
            || authority.kind == RuntimeKind::Podman
                && observed.process.status.as_deref() == Some("stopped");
        if !terminal_status
            || observed.process.running != Some(false)
            || observed.process.exit_code.is_none()
            || observed.process.started_at.is_none()
            || observed.process.finished_at.is_none()
        {
            return Err(Code::NeedsReconciliation);
        }

        let claimed_start = worker
            .observed_started_at
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .filter(|value| value.timestamp() > 0)
            .ok_or(Code::NeedsReconciliation)?;
        let claim_observation = worker
            .last_observation
            .as_ref()
            .filter(|observation| {
                observation.status == RuntimeStatusV1::Running && observation.running == Some(true)
            })
            .and_then(|observation| observation.started_at.as_deref())
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .filter(|value| value.timestamp() > 0)
            .ok_or(Code::NeedsReconciliation)?;
        if claim_observation != claimed_start {
            return Err(Code::Conflict);
        }
        if observed.process.started_at != Some(claimed_start) {
            return Err(Code::Conflict);
        }

        observed
            .validate_terminal(authority.kind, &created.observation)
            .map_err(|_| Code::Conflict)?;
        let finished_at = observed
            .process
            .finished_at
            .ok_or(Code::NeedsReconciliation)?;
        if observed.observed_at < finished_at.with_timezone(&chrono::Utc) {
            return Err(Code::Conflict);
        }
        Ok(())
    }
}

impl Service {
    /// Reconcile an exact operation on the installed STOP writer. Only a
    /// qualified fresh state can enter the guarded observation path.
    #[allow(
        dead_code,
        reason = "the Legacy terminal recovery caller is composed separately"
    )]
    pub(super) async fn reconcile_legacy_worker_terminal(
        &self,
        scope: &JournalScopeV1,
        operation_id: Uuid,
        ancestor: &RevisionRefV1,
    ) -> Result<ConfirmedLegacyWorkerTerminalObservationV1, Code> {
        use LegacyWorkerTerminalRecoveryStateV1::{Confirmed, Fresh, Pending};
        use ReservedLegacyWorkerTerminalObservationV1::{Appended, Existing};

        self.require_legacy_terminal_validation_fence()?;
        if operation_id.is_nil() {
            return Err(Code::InvalidInput);
        }
        let profile = self
            .registry
            .profile_for_scope(scope)
            .map_err(|_| Code::Forbidden)?
            .clone();
        let jobs = self.profile_jobs(scope.profile_id())?;
        let writer = jobs
            .writer
            .get()
            .filter(|capabilities| capabilities.is_stop_only())
            .ok_or(Code::NeedsReconciliation)?
            .stops();
        let state = tokio::time::timeout_at(ColdAuditBudget::start().deadline(), async {
            self.validate_registry().await?;
            writer
                .legacy_worker_terminal_recovery_state(operation_id, scope, ancestor)
                .await
                .map_err(jobs::journal_error)
        })
        .await
        .map_err(|_| Code::Unavailable)??;

        let pending_head = match state {
            Confirmed(confirmed) => return Ok(*confirmed),
            Pending(pending) => pending.head().clone(),
            Fresh(proof) => {
                let observed = self.observe_legacy_worker_terminal(&writer, &proof).await?;
                let request = observed.terminal_mutation()?;
                self.require_legacy_terminal_validation_fence()?;
                let reserved = writer
                    .reserve_legacy_worker_terminal_observation(
                        observed.validated.evidence,
                        request,
                    )
                    .await
                    .map_err(|_| Code::NeedsReconciliation)?;
                let (Appended(pending) | Existing(pending)) = reserved;
                pending.head().clone()
            }
        };
        self.publish_legacy_worker_terminal_observation(
            &writer,
            profile,
            operation_id,
            &pending_head,
        )
        .await
    }

    /// Read a terminal observation on the already installed Legacy recovery
    /// profile. Latch reads corroborate the exact durable STOP at each end of
    /// the observation. Each latch read independently authenticates its epoch.
    async fn observe_legacy_worker_terminal(
        &self,
        writer: &StopWriterClient,
        selected: &LegacyWorkerTerminalEvidenceV1,
    ) -> Result<ObservedLegacyWorkerTerminal, Code> {
        self.require_legacy_terminal_validation_fence()?;
        let operation_id = selected.operation_id();
        let command_id = selected.cancel_receipt().command_id();
        check_ids(operation_id, command_id)?;
        let guardian = self
            .stop_enforcer
            .as_ref()
            .ok_or(Code::NeedsReconciliation)?;
        let _capacity = self.execution.try_acquire().map_err(|_| Code::Busy)?;
        let budget = ColdAuditBudget::start();
        tokio::time::timeout_at(budget.deadline(), async {
            let expected = observation_target_facts(selected)?;
            let read = || async {
                let proof = writer
                    .legacy_worker_terminal_evidence(operation_id, command_id)
                    .await
                    .map_err(jobs::journal_error)?;
                if observation_target_facts(&proof)? != expected {
                    return Err(Code::Conflict);
                }
                Ok(proof)
            };
            let proof = read().await?;
            let pinned = self.check_legacy_terminal_target(&proof, &budget).await?;
            guardian
                .confirm_legacy_terminal_stop_latch(&proof)
                .await
                .map_err(|_| Code::NeedsReconciliation)?;
            read().await?;
            self.require_legacy_terminal_validation_fence()?;

            let validated = ValidatedLegacyWorkerTerminal { evidence: proof };
            let (authority, owned) = validated.evidence().target();
            let runtime = ManagedRuntime::reopen_with_runtime(authority.clone(), pinned)
                .await
                .map_err(|_| Code::NeedsReconciliation)?;
            read().await?;
            self.require_legacy_terminal_validation_fence()?;
            let observation = runtime
                .resume_terminal(&owned.handle)
                .await
                .map_err(|_| Code::NeedsReconciliation)?;
            validated.validate_terminal_observation(&observation)?;

            self.check_legacy_terminal_target(validated.evidence(), &budget)
                .await?;
            guardian
                .confirm_legacy_terminal_stop_latch(validated.evidence())
                .await
                .map_err(|_| Code::NeedsReconciliation)?;
            read().await?;
            self.require_legacy_terminal_validation_fence()?;
            Ok(ObservedLegacyWorkerTerminal {
                validated,
                observation,
            })
        })
        .await
        .map_err(|_| Code::Unavailable)?
    }

    pub(super) async fn validate_legacy_worker_terminal(
        &self,
        proof: LegacyWorkerTerminalEvidenceV1,
    ) -> Result<ValidatedLegacyWorkerTerminal, Code> {
        let budget = ColdAuditBudget::start();
        self.check_legacy_terminal_target(&proof, &budget).await?;
        Ok(ValidatedLegacyWorkerTerminal { evidence: proof })
    }

    /// Publish the already retained terminal observation under closed
    /// admission. A confirmed local receipt answers recovery immediately.
    /// An uncertain exchange leaves the same request available for replay.
    async fn publish_legacy_worker_terminal_observation(
        &self,
        writer: &StopWriterClient,
        profile: ControlProfileV1,
        operation_id: Uuid,
        ancestor: &RevisionRefV1,
    ) -> Result<ConfirmedLegacyWorkerTerminalObservationV1, Code> {
        self.require_legacy_terminal_validation_fence()?;
        let scope = writer.scope();
        let _capacity = self.execution.try_acquire().map_err(|_| Code::Busy)?;
        let budget = ColdAuditBudget::start();

        tokio::time::timeout_at(budget.deadline(), async {
            self.validate_registry().await?;
            let read_ack = || async {
                writer
                    .confirmed_legacy_worker_terminal_observation(operation_id, scope, ancestor)
                    .await
                    .map_err(jobs::journal_error)
            };
            if let Some(confirmed) = read_ack().await? {
                return Ok(confirmed);
            }
            let pending = writer
                .legacy_worker_terminal_observation(operation_id)
                .await
                .map_err(jobs::journal_error)?
                .ok_or(Code::NeedsReconciliation)?;

            let material = Arc::clone(&self.material);
            let retained_profile = profile.clone();
            let credential = self
                .filesystem(move || {
                    Ok(material
                        .checked_reconciler(&retained_profile)
                        .map_err(|_| Code::Unavailable))
                })
                .await??;
            let expected_scope = canonical(scope).map_err(jobs::journal_error)?;
            let authenticate = || async {
                let verified = VerifiedProfile::authenticate(
                    &self.registry,
                    scope.profile_id(),
                    &credential,
                    false,
                )
                .await?;
                if canonical(&verified.scope).map_err(jobs::journal_error)? != expected_scope {
                    return Err(Code::Forbidden);
                }
                Ok(verified)
            };
            let verified = authenticate().await?;
            let current = verified
                .client
                .cold_worker(&pending.request().worker_id, &budget)
                .await
                .map_err(|_| Code::Unavailable)?;
            self.validate_registry().await?;
            authenticate().await?;

            if let Some(confirmed) = read_ack().await? {
                return Ok(confirmed);
            }
            // Re-read the original STOP and exact pending head on the retained
            // actor immediately before dispatch. The writer lease stays held
            // across the exchange and its journal mutex is free during I/O.
            let proof = writer
                .legacy_worker_terminal_evidence(operation_id, pending.stop().command_id())
                .await
                .map_err(jobs::journal_error)?;
            legacy_terminal_publication_state(&proof, &pending, &current)?;
            require_archived_invocation(&profile, &proof)?;
            self.require_legacy_terminal_validation_fence()?;
            if budget.remaining().is_zero() {
                return Err(Code::Unavailable);
            }
            // Both eligible states send the exact durable body and require
            // the operation's receipt from the Core.
            let response = verified
                .client
                .post_legacy_terminal_observation(pending.request())
                .await
                .map_err(|_| Code::NeedsReconciliation)?;
            pending
                .validate_response(&response)
                .map_err(|_| Code::NeedsReconciliation)?;
            // Settle an exact response under this writer even if shutdown was
            // requested after dispatch. The writer revalidates scope and lease.
            writer
                .confirm_legacy_worker_terminal_observation(&pending, response)
                .await
                .map_err(|_| Code::NeedsReconciliation)
        })
        .await
        .map_err(|_| Code::NeedsReconciliation)?
    }

    async fn check_legacy_terminal_target(
        &self,
        proof: &LegacyWorkerTerminalEvidenceV1,
        budget: &ColdAuditBudget,
    ) -> Result<Runtime, Code> {
        self.require_legacy_terminal_validation_fence()?;
        let scope = proof.scope();
        let profile = self
            .registry
            .profile_for_scope(scope)
            .map_err(|_| Code::Forbidden)?
            .clone();
        let invocation = require_archived_invocation(&profile, proof)?.clone();
        let expected_scope = canonical(scope).map_err(jobs::journal_error)?;

        tokio::time::timeout_at(budget.deadline(), self.validate_registry())
            .await
            .map_err(|_| Code::Unavailable)??;
        let material = Arc::clone(&self.material);
        let retained_profile = profile.clone();
        let (credential, pinned) = tokio::time::timeout_at(
            budget.deadline(),
            self.filesystem(move || {
                let credential = material
                    .checked_reconciler(&retained_profile)
                    .map_err(|_| Code::Unavailable)?;
                // Pin the exact archived executable/directory, environment and
                // policy. This performs filesystem validation only.
                let pinned = Runtime::from_invocation(retained_profile.runtime, invocation)
                    .map_err(|_| Code::Conflict)?;
                Ok((credential, pinned))
            }),
        )
        .await
        .map_err(|_| Code::Unavailable)??;

        let authenticate = || async {
            let verified = tokio::time::timeout_at(
                budget.deadline(),
                VerifiedProfile::authenticate(&self.registry, &profile.id, &credential, false),
            )
            .await
            .map_err(|_| Code::Unavailable)??;
            if canonical(&verified.scope).map_err(jobs::journal_error)? != expected_scope {
                return Err(Code::Forbidden);
            }
            Ok(verified)
        };
        let verified = authenticate().await?;
        let worker = verified
            .client
            .cold_worker(&proof.worker().worker_id, budget)
            .await
            .map_err(|_| Code::Unavailable)?;
        // The Worker read permits ReadIdentity. Recheck that the same
        // credential still resolves to the Legacy Runner of this scope.
        authenticate().await?;
        self.require_legacy_terminal_validation_fence()?;
        require_current_worker(proof.worker(), &worker)?;
        if budget.remaining().is_zero() {
            return Err(Code::Unavailable);
        }
        Ok(pinned)
    }

    fn require_legacy_terminal_validation_fence(&self) -> Result<(), Code> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(Code::Unavailable);
        }
        if !self.recovery_only() && !self.fenced.load(Ordering::Acquire) {
            return Err(Code::NeedsReconciliation);
        }
        Ok(())
    }
}

fn observation_target_facts(proof: &LegacyWorkerTerminalEvidenceV1) -> Result<Vec<u8>, Code> {
    let stop = proof.cancel_receipt();
    canonical(&(
        proof.scope(),
        proof.head(),
        stop.command_id(),
        stop.input_hash(),
        stop.head(),
        proof.arm(),
        proof.claim(),
        proof.target(),
        proof.worker(),
    ))
    .map_err(jobs::journal_error)
}

fn require_archived_invocation<'a>(
    profile: &ControlProfileV1,
    proof: &'a LegacyWorkerTerminalEvidenceV1,
) -> Result<&'a InvocationBindingV1, Code> {
    let (authority, _) = proof.target();
    let invocation = authority
        .invocation
        .as_ref()
        .ok_or(Code::NeedsReconciliation)?;
    if profile.runtime != authority.kind
        || profile.runtime_endpoint != authority.endpoint
        || invocation.kind != profile.runtime
        || invocation.approved_entry != profile.runtime_program
    {
        return Err(Code::Conflict);
    }
    Ok(invocation)
}

fn require_current_worker(retained: &WorkerV1, current: &WorkerV1) -> Result<(), Code> {
    if !matches!(
        current.state,
        WorkerStateV1::Running | WorkerStateV1::Stopping
    ) || !current.quota_held
        || current.allocation_inconsistent
        || current.unknown_reason.is_some()
        || current.last_observation.as_ref().is_none_or(|observation| {
            observation.status != RuntimeStatusV1::Running || observation.running != Some(true)
        })
    {
        return Err(Code::NeedsReconciliation);
    }
    // Include the exact revision, immutable launch/decision, worker/attempt,
    // CID, claim operation and emitter, and the observed start and quota.
    if canonical(retained).map_err(jobs::journal_error)?
        != canonical(current).map_err(jobs::journal_error)?
    {
        return Err(Code::Conflict);
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum LegacyTerminalPublicationStateV1 {
    ExactParent,
    PossibleReplay,
}

/// Classify a Worker already read from the authenticated Core in this scope.
/// A matching terminal snapshot is a candidate for exact replay. The receipt
/// must still bind the Core operation and outcome to the retained request.
pub(super) fn legacy_terminal_publication_state(
    proof: &LegacyWorkerTerminalEvidenceV1,
    pending: &RetainedLegacyWorkerTerminalObservationV1,
    current: &WorkerV1,
) -> Result<LegacyTerminalPublicationStateV1, Code> {
    let terminal = pending
        .expected_worker_for_publication(proof)
        .map_err(jobs::journal_error)?;
    if current.state == WorkerStateV1::Terminal {
        if canonical(current).map_err(jobs::journal_error)?
            != canonical(terminal).map_err(jobs::journal_error)?
        {
            return Err(Code::Conflict);
        }
        return Ok(LegacyTerminalPublicationStateV1::PossibleReplay);
    }
    require_current_worker(proof.worker(), current)?;
    Ok(LegacyTerminalPublicationStateV1::ExactParent)
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
#[path = "legacy_worker_terminal_tests.rs"]
mod tests;
