//! Private preparation through the Writer and the named broker identity.
use super::super::profile::LegacyNamedSource;
use super::*;
use crate::{
    control::session_control::{
        LegacyConfirmedLaunchSpecV1, LegacyConfirmedPlanV1, LegacyNamedInitialDecisionV1,
    },
    model::Outcome,
    runner::broker::{
        LegacyNamedWorkspaceArchiveV1, PreparedSubmission, retain_legacy_named_archive,
    },
};

#[allow(
    dead_code,
    reason = "the private named service caller is a separate G04 integration"
)]
pub(super) enum LegacyNamedLaunchPreparation {
    Replayed(AcceptedJobV1),
    Rejected {
        receipt: RejectionReceiptV1,
        replayed: bool,
    },
    Held {
        accepted: AcceptedJobV1,
        decision: Box<LegacyNamedInitialDecisionV1>,
    },
    Ready(Box<PreparedLegacyNamedLaunch>),
}

/// Only this live result retains the original job generation and owner slot.
/// A replay has no archive or capability to start the admission workflow.
#[allow(
    dead_code,
    reason = "the private named service caller is a separate G04 integration"
)]
pub(super) struct PreparedLegacyNamedLaunch {
    pub(super) request: AuthorizedRequest,
    pub(super) accepted: AcceptedJobV1,
    pub(super) decision: LegacyNamedInitialDecisionV1,
    pub(super) material: LegacyConfirmedLaunchSpecV1,
    pub(super) archive: (LegacyNamedWorkspaceArchiveV1, Directory),
    pub(super) owner: FullOwnerSlot,
    pub(super) job: Arc<Job>,
    pub(super) running: RunningJob,
    pub(super) capacity: OwnedSemaphorePermit,
}

impl Service {
    #[allow(
        dead_code,
        reason = "the private named service caller is a separate G04 integration"
    )]
    pub(super) async fn prepare_legacy_named_launch(
        &self,
        request: AuthorizedRequest,
        operation: Uuid,
        binding_ref: Uuid,
        credential: &str,
    ) -> Result<LegacyNamedLaunchPreparation, BrokerErrorCodeV1> {
        check_ids(operation, binding_ref)?;
        let writer = &request.writer;
        let scope = &request.verified.scope;
        if request.profile.id != scope.profile_id()
            || canonical(writer.scope()).map_err(jobs::journal_error)?
                != canonical(scope).map_err(jobs::journal_error)?
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let control = request
            .jobs
            .named_control
            .get()
            .ok_or(BrokerErrorCodeV1::IncompatibleControl)?;
        let generation = request
            .jobs
            .known(operation)
            .map_or(0, |job| job.cancel.current());
        let preparation = Arc::clone(&self.preparation)
            .acquire_owned()
            .await
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        self.validate_control().await?;
        let material = control
            .read_legacy_confirmed_launch_spec(scope.profile_id(), binding_ref)
            .await
            .map_err(plan_error)?;
        if material.plan.operation_id != operation || material.plan.binding_ref != binding_ref {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        self.material.screen(
            &serde_json::to_value(&material).map_err(|_| BrokerErrorCodeV1::InvalidInput)?,
            credential,
        )?;

        let job = request.jobs.job(operation)?;
        let owner = if writer
            .status(operation)
            .await
            .map_err(jobs::journal_error)?
            .is_none()
        {
            Some(self.reserve_full_owner()?)
        } else {
            None
        };
        let ticket = match writer
            .reserve_legacy_named(operation, binding_ref)
            .await
            .map_err(jobs::journal_error)?
        {
            ReserveResult::Accepted(retained) => {
                let (prepared, _) = self.named_preflight(scope, &material).await?;
                require_named_intent(&retained, &material.plan, &submission_hash(&prepared)?)?;
                return Ok(LegacyNamedLaunchPreparation::Replayed(retained));
            }
            ReserveResult::Rejected(receipt) => {
                return Ok(LegacyNamedLaunchPreparation::Rejected {
                    receipt,
                    replayed: true,
                });
            }
            ReserveResult::InProgress => return Err(BrokerErrorCodeV1::Busy),
            ReserveResult::Interrupted => return Err(BrokerErrorCodeV1::Interrupted),
            ReserveResult::Absent => return Err(BrokerErrorCodeV1::IncompatibleControl),
            ReserveResult::New(ticket) => ticket,
        };
        let Some(owner) = owner else {
            job.result(Some(BrokerErrorCodeV1::NeedsReconciliation));
            return Err(BrokerErrorCodeV1::NeedsReconciliation);
        };
        let preflight = async {
            let running = job.enter()?;
            if running.generation() != generation {
                return Err(BrokerErrorCodeV1::Interrupted);
            }
            self.require_starts().await?;
            let (submission, source) = self.named_preflight(scope, &material).await?;
            let intent_hash = submission_hash(&submission)?;
            require_generation(&job.cancel, generation)?;
            Ok::<_, BrokerErrorCodeV1>((running, submission, source, intent_hash))
        }
        .await;
        let (running, submission, source, intent_hash) = match preflight {
            Ok(prepared) => prepared,
            Err(error) => {
                job.result(Some(error));
                let code = if job.cancel.current() != generation {
                    RejectionCodeV1::CancelledBeforeAcceptance
                } else {
                    RejectionCodeV1::PreparationFailed
                };
                let receipt = complete_rejection(writer, operation, ticket, &job, code).await?;
                return Ok(LegacyNamedLaunchPreparation::Rejected {
                    receipt,
                    replayed: false,
                });
            }
        };
        let config = submission.config().clone();
        let accepted =
            match complete_acceptance(writer, ticket, submission, &job, generation).await? {
                Acceptance::Accepted(accepted) => accepted,
                Acceptance::Rejected(receipt) => {
                    return Ok(LegacyNamedLaunchPreparation::Rejected {
                        receipt,
                        replayed: false,
                    });
                }
            };

        let initialized = async {
            require_named_intent(&accepted, &material.plan, &intent_hash)?;
            require_generation(&job.cancel, generation)?;
            let decision = control
                .legacy_named_initial_decision(writer, &material, (&job.cancel, generation))
                .await
                .map_err(|error| match error {
                    PresenceError::Stopped => BrokerErrorCodeV1::Interrupted,
                    _ => BrokerErrorCodeV1::NeedsReconciliation,
                })?;
            let accepted = writer
                .load(operation)
                .await
                .map_err(jobs::journal_error)?
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
            require_named_intent(&accepted, &material.plan, &intent_hash)?;
            if decision.decision.outcome != Outcome::Allow {
                return Ok((accepted, decision, None));
            }
            self.require_starts().await?;
            let cancel = job.cancel.clone();
            let control_material = Arc::clone(&self.material);
            let launch = material.clone();
            let (archive, config) = self
                .filesystem(move || {
                    require_generation(&cancel, generation)?;
                    control_material.revalidate()?;
                    let workspace = source.collect(&control_material.parents)?;
                    control_material.revalidate()?;
                    require_generation(&cancel, generation)?;
                    let archive =
                        retain_legacy_named_archive(&config, operation, &launch, workspace)
                            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
                    Ok((archive, config))
                })
                .await?;
            drop(preparation);
            let capacity = Arc::clone(&self.execution)
                .acquire_owned()
                .await
                .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
            self.require_starts().await?;
            self.validate_control().await?;
            require_generation(&job.cancel, generation)?;
            Ok((accepted, decision, Some((archive, capacity))))
        }
        .await;
        job.result(initialized.as_ref().err().copied());
        let (accepted, decision, ready) = initialized?;
        let Some((archive, capacity)) = ready else {
            return Ok(LegacyNamedLaunchPreparation::Held {
                accepted,
                decision: Box::new(decision),
            });
        };
        Ok(LegacyNamedLaunchPreparation::Ready(Box::new(
            PreparedLegacyNamedLaunch {
                request,
                accepted,
                decision,
                material,
                archive,
                owner,
                job,
                running,
                capacity,
            },
        )))
    }

    async fn named_preflight(
        &self,
        scope: &JournalScopeV1,
        material: &LegacyConfirmedLaunchSpecV1,
    ) -> Result<(PreparedSubmission, LegacyNamedSource), BrokerErrorCodeV1> {
        let registry = Arc::clone(&self.registry);
        let root = self.root.clone();
        let scope = scope.clone();
        let material = material.clone();
        self.filesystem(move || {
            Ok(registry.prepare_legacy_named_submission(&scope, &material, &root)?)
        })
        .await
    }
}

fn submission_hash(submission: &PreparedSubmission) -> Result<String, BrokerErrorCodeV1> {
    submission
        .intent()
        .intent_hash()
        .map_err(|_| BrokerErrorCodeV1::InvalidInput)
}

fn require_named_intent(
    retained: &AcceptedJobV1,
    plan: &LegacyConfirmedPlanV1,
    intent_hash: &str,
) -> Result<(), BrokerErrorCodeV1> {
    if retained.intent().legacy_named_plan() == Some(plan) && retained.intent_hash() == intent_hash
    {
        Ok(())
    } else {
        Err(BrokerErrorCodeV1::Conflict)
    }
}

fn require_generation(cancel: &CancelGeneration, generation: u64) -> Result<(), BrokerErrorCodeV1> {
    cancel
        .with_current_generation(generation, || ())
        .map_err(|_| BrokerErrorCodeV1::Interrupted)
}

fn plan_error(error: PresenceError) -> BrokerErrorCodeV1 {
    match error {
        PresenceError::Authentication => BrokerErrorCodeV1::Unauthorized,
        PresenceError::Forbidden => BrokerErrorCodeV1::Forbidden,
        PresenceError::Conflict => BrokerErrorCodeV1::Conflict,
        PresenceError::Stopped => BrokerErrorCodeV1::Interrupted,
        PresenceError::Capacity => BrokerErrorCodeV1::LimitExceeded,
        PresenceError::Transport | PresenceError::Unavailable => BrokerErrorCodeV1::Unavailable,
        _ => BrokerErrorCodeV1::NeedsReconciliation,
    }
}
