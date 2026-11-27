//! Private preparation and admission through the Writer and the named identity.
use super::super::profile::LegacyNamedSource;
use super::*;
use crate::{
    broker::presence::LegacyNamedAdmissionOutcome,
    control::{
        MutationReceiptV1, WorkerV1,
        session_control::{
            LegacyConfirmedLaunchSpecV1, LegacyConfirmedPlanV1, LegacyNamedAdmissionReceiptV1,
            LegacyNamedInitialDecisionV1,
        },
    },
    model::Outcome,
    runner::broker::{
        LegacyNamedWorkspaceArchiveV1, PreparedSubmission, broker_helper_spec,
        create_import_volume, reopen_legacy_named_archive, reopen_legacy_named_runtime,
        retain_legacy_named_archive,
    },
    runtime::{ManagedRuntime, OwnedContainerSnapshot, Runtime},
};
use std::fs::File;

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
    request: AuthorizedRequest,
    accepted: AcceptedJobV1,
    material: LegacyConfirmedLaunchSpecV1,
    archive: (LegacyNamedWorkspaceArchiveV1, Directory),
    owner: FullOwnerSlot,
    job: Arc<Job>,
    running: RunningJob,
    capacity: OwnedSemaphorePermit,
}

#[allow(
    dead_code,
    reason = "the private named service caller is a separate G04 integration"
)]
pub(super) enum LegacyNamedLaunchAdmission {
    Admitted(Box<AdmittedLegacyNamedLaunch>),
    Refused(Box<LegacyNamedAdmissionReceiptV1>),
    Replayed(Box<LegacyNamedAdmissionReceiptV1>),
}

/// Live reservation and exact reopened archive. The caller retains these
/// guards when crossing the separate Writer and STOP gates for creation.
#[allow(
    dead_code,
    reason = "the private named service caller is a separate G04 integration"
)]
pub(super) struct AdmittedLegacyNamedLaunch {
    pub(super) request: AuthorizedRequest,
    pub(super) accepted: AcceptedJobV1,
    pub(super) material: LegacyConfirmedLaunchSpecV1,
    pub(super) reservation: MutationReceiptV1,
    pub(super) worker: WorkerV1,
    pub(super) archive: File,
    pub(super) archive_digest: String,
    config: Directory,
    pub(super) owner: FullOwnerSlot,
    pub(super) job: Arc<Job>,
    pub(super) running: RunningJob,
    pub(super) capacity: OwnedSemaphorePermit,
}

impl AdmittedLegacyNamedLaunch {
    #[allow(
        dead_code,
        reason = "the private named service caller is a separate G04 integration"
    )]
    pub(super) fn config(&self) -> &Directory {
        &self.config
    }
}

/// The exact Created instance and its live admission remain together until
/// the separate Arm and start-claim phase.
#[allow(
    dead_code,
    reason = "the named import Arm caller is composed separately"
)]
pub(super) struct CreatedLegacyNamedImport {
    pub(super) admission: Box<AdmittedLegacyNamedLaunch>,
    pub(super) runtime: ManagedRuntime,
    pub(super) owned: OwnedContainerSnapshot,
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
        drop(preparation);
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
                .map_err(named_effect_error)?;
            let accepted = writer
                .load(operation)
                .await
                .map_err(jobs::journal_error)?
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
            require_named_intent(&accepted, &material.plan, &intent_hash)?;
            if decision.decision.outcome != Outcome::Allow {
                return Ok((accepted, decision, None));
            }
            let preparation = Arc::clone(&self.preparation)
                .acquire_owned()
                .await
                .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
            self.require_starts().await?;
            let cancel = job.cancel.clone();
            let control_material = Arc::clone(&self.material);
            let launch = material.clone();
            let archive = self
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
                material,
                archive,
                owner,
                job,
                running,
                capacity,
            },
        )))
    }

    #[allow(
        dead_code,
        reason = "the private named service caller is a separate G04 integration"
    )]
    pub(super) async fn admit_prepared_legacy_named_launch(
        &self,
        prepared: Box<PreparedLegacyNamedLaunch>,
    ) -> Result<LegacyNamedLaunchAdmission, BrokerErrorCodeV1> {
        let PreparedLegacyNamedLaunch {
            request,
            accepted,
            material,
            archive,
            owner,
            job,
            running,
            capacity,
        } = *prepared;
        let generation = running.generation();
        let result = async {
            require_generation(&job.cancel, generation)?;
            self.require_starts().await?;
            self.validate_control().await?;
            let control = request
                .jobs
                .named_control
                .get()
                .ok_or(BrokerErrorCodeV1::IncompatibleControl)?;
            let archive_digest = archive
                .0
                .digest_for_launch(&material)
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
                .to_owned();
            let config = archive.1.clone();
            require_generation(&job.cancel, generation)?;
            let writer = &request.writer;
            let outcome = control
                .admit_legacy_named(writer, &material, archive, (&job.cancel, generation))
                .await
                .map_err(named_effect_error)?;
            let receipt = match outcome {
                LegacyNamedAdmissionOutcome::Refused(receipt) => {
                    return Ok(LegacyNamedLaunchAdmission::Refused(Box::new(receipt)));
                }
                LegacyNamedAdmissionOutcome::Accepted(receipt) if receipt.admission.replayed => {
                    return Ok(LegacyNamedLaunchAdmission::Replayed(Box::new(receipt)));
                }
                LegacyNamedAdmissionOutcome::Accepted(receipt) => receipt,
            };

            // An in-flight POST can finish after STOP. The transport settles
            // its receipt, and the service gives that generation no handoff.
            require_generation(&job.cancel, generation)?;
            let retained = writer
                .load(accepted.operation_id())
                .await
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
            require_named_intent(&retained, &material.plan, accepted.intent_hash())?;
            require_live_admission(&retained, &archive_digest, &receipt)?;
            let worker = receipt
                .admission
                .worker
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
            let reservation = receipt.admission.receipt;
            let cancel = job.cancel.clone();
            let archive_config = config.clone();
            let (accepted, archive) = self
                .filesystem(move || {
                    require_generation(&cancel, generation)?;
                    let archive = reopen_legacy_named_archive(&archive_config, &retained)?;
                    require_generation(&cancel, generation)?;
                    Ok((retained, archive))
                })
                .await?;
            self.validate_control().await?;
            self.require_starts().await?;
            require_generation(&job.cancel, generation)?;
            Ok(LegacyNamedLaunchAdmission::Admitted(Box::new(
                AdmittedLegacyNamedLaunch {
                    request,
                    accepted,
                    material,
                    reservation,
                    worker,
                    archive,
                    archive_digest,
                    config,
                    owner,
                    job: Arc::clone(&job),
                    running,
                    capacity,
                },
            )))
        }
        .await;
        job.result(result.as_ref().err().copied());
        result
    }

    #[allow(
        dead_code,
        reason = "the named import Arm caller is composed separately"
    )]
    pub(super) async fn create_admitted_legacy_named_import(
        &self,
        admitted: Box<AdmittedLegacyNamedLaunch>,
    ) -> Result<CreatedLegacyNamedImport, BrokerErrorCodeV1> {
        let job = Arc::clone(&admitted.job);
        let generation = admitted.running.generation();
        let result = async {
            require_generation(&job.cancel, generation)?;
            self.require_starts().await?;
            self.validate_control().await?;
            let config = admitted.config().clone();
            let profile = &admitted.request.profile;
            let kind = profile.runtime;
            let program = profile.runtime_program.clone();
            let approved = self
                .filesystem(move || {
                    config.revalidate()?;
                    Runtime::with_program(kind, &program)
                })
                .await?;
            let options = admitted.accepted.intent().initial_options().run_options();
            let runtime = reopen_legacy_named_runtime(
                &admitted.material,
                &options,
                admitted.config(),
                &approved,
            )
            .await
            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
            self.require_starts().await?;
            self.validate_control().await?;
            require_generation(&job.cancel, generation)?;

            let writer = &admitted.request.writer;
            let plan = &admitted.material.plan;
            let digest = &admitted.archive_digest;
            let creation = writer
                .record_legacy_named_import_creation(
                    plan,
                    digest,
                    Uuid::new_v4(),
                    runtime.binding(),
                )
                .await
                .map_err(jobs::journal_error)?
                .into_job();
            let confirmed = creation
                .broker_checkpoint_v2()
                .and_then(|checkpoint| checkpoint.supervision().legacy_named_admission())
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
            let preparation = confirmed
                .import_creation()
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
            let worker = confirmed
                .admission_result()
                .and_then(|result| result.worker.as_ref())
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
            let spec = broker_helper_spec(preparation.policy())
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;

            // The intent is durable before either command. Either effect can
            // win before STOP, so a failed response keeps reconciliation open.
            create_import_volume(&runtime, worker, &job.cancel, generation)
                .await
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
            let owned = runtime
                .create_owned_import_for_generation(&spec, &job.cancel, generation)
                .await
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
            writer
                .acknowledge_legacy_named_import_creation(
                    plan,
                    digest,
                    runtime.binding(),
                    owned.clone(),
                )
                .await
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
            require_generation(&job.cancel, generation)?;
            Ok(CreatedLegacyNamedImport {
                admission: admitted,
                runtime,
                owned,
            })
        }
        .await;
        job.result(result.as_ref().err().copied());
        result
    }

    /// Settle a lost creation result from the original Writer intent. The
    /// inspection can retain evidence even after STOP and grants no handoff.
    #[allow(
        dead_code,
        reason = "the named import recovery caller is composed separately"
    )]
    pub(super) async fn reconcile_legacy_named_import_creation(
        &self,
        request: &AuthorizedRequest,
        operation: Uuid,
    ) -> Result<(), BrokerErrorCodeV1> {
        self.validate_control().await?;
        let retained = request
            .writer
            .load(operation)
            .await
            .map_err(jobs::journal_error)?
            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
        let material = retained
            .intent()
            .legacy_named_launch()
            .ok_or(BrokerErrorCodeV1::Conflict)?;
        let named = retained
            .broker_checkpoint_v2()
            .and_then(|checkpoint| checkpoint.supervision().legacy_named_admission())
            .filter(|named| named.plan() == &material.plan)
            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
        let digest = named
            .archive_digest()
            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
        let preparation = named
            .import_creation()
            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
        if preparation.owned().is_some() {
            return Ok(());
        }

        let profile = &request.profile;
        let options = retained.intent().initial_options().run_options();
        if options.runtime_endpoint.as_deref() != Some(profile.runtime_endpoint.as_str()) {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let config_path = profile.config_dir.clone();
        let kind = profile.runtime;
        let program = profile.runtime_program.clone();
        let (config, approved) = self
            .filesystem(move || {
                Ok((
                    Directory::open_existing(&config_path)?,
                    Runtime::with_program(kind, &program)?,
                ))
            })
            .await?;
        let runtime = reopen_legacy_named_runtime(material, &options, &config, &approved)
            .await
            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
        named
            .require_import_creation_binding(runtime.binding())
            .map_err(jobs::journal_error)?;
        let spec = broker_helper_spec(preparation.policy())
            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
        let owned = runtime
            .lookup_never_started_import(&spec)
            .await
            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
        request
            .writer
            .acknowledge_legacy_named_import_creation(
                &material.plan,
                digest,
                runtime.binding(),
                owned,
            )
            .await
            .map_err(jobs::journal_error)
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

fn require_live_admission(
    retained: &AcceptedJobV1,
    archive_digest: &str,
    receipt: &LegacyNamedAdmissionReceiptV1,
) -> Result<(), BrokerErrorCodeV1> {
    let checkpoint = retained
        .broker_checkpoint_v3()
        .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
    if checkpoint
        .commands()
        .iter()
        .any(|command| matches!(command.request(), LocalCommandRequestV1::CancelRequested))
    {
        return Err(BrokerErrorCodeV1::Interrupted);
    }
    let named = checkpoint
        .broker()
        .supervision()
        .legacy_named_admission()
        .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
    let saved = named
        .receipt()
        .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
    if named.archive_digest() != Some(archive_digest)
        || named.import_creation().is_some()
        || named.import_arm().is_some()
        || canonical(saved).map_err(jobs::journal_error)?
            != canonical(receipt).map_err(jobs::journal_error)?
    {
        return Err(BrokerErrorCodeV1::NeedsReconciliation);
    }
    Ok(())
}

fn require_generation(cancel: &CancelGeneration, generation: u64) -> Result<(), BrokerErrorCodeV1> {
    cancel
        .with_current_generation(generation, || ())
        .map_err(|_| BrokerErrorCodeV1::Interrupted)
}

fn named_effect_error(error: PresenceError) -> BrokerErrorCodeV1 {
    match error {
        PresenceError::Stopped => BrokerErrorCodeV1::Interrupted,
        _ => BrokerErrorCodeV1::NeedsReconciliation,
    }
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
