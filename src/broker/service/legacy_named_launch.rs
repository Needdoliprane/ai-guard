//! Private preparation and admission through the Writer and the named identity.
use super::super::profile::LegacyNamedSource;
use super::*;
use crate::{
    broker::clock::HostClock,
    broker::enforcer::{
        ExecutionEnforcerV1, LegacyNamedProspectiveBind, LegacyNamedProspectiveBindAck,
    },
    broker::presence::LegacyNamedAdmissionOutcome,
    control::{
        ArmSubjectV1, ArmV1, HelperPermissionV1, HelperPurposeV1, MutationReceiptV1,
        WorkerMutationV1, WorkerStateV1, WorkerTransitionV1, WorkerV1,
        session_control::{
            LegacyConfirmedLaunchSpecV1, LegacyConfirmedPlanV1, LegacyNamedAdmissionReceiptV1,
            LegacyNamedImportArmRequestV1, LegacyNamedInitialDecisionV1,
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
    /// The exact Writer prefix that reserved helper creation. This value is
    /// available only after the matching Created observation was durably
    /// acknowledged below; it never grants a runtime or enforcer capability.
    writer_creation_prefix: crate::broker::checkpoint::LegacyNamedAdmissionCheckpoint,
}

/// Retains the live admission and the exact native acknowledgement for a
/// separate, durable start claim.
#[allow(dead_code, reason = "the named start claim is composed separately")]
pub(super) struct BoundLegacyNamedImport {
    pub(super) created: CreatedLegacyNamedImport,
    pub(super) acknowledgement: Box<LegacyNamedProspectiveBindAck>,
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
                writer_creation_prefix: confirmed.clone(),
            })
        }
        .await;
        job.result(result.as_ref().err().copied());
        result
    }

    /// Derive the private prospective STOP target for the already-created
    /// import helper. This is data only: no IPC, Core call, guardian action,
    /// claim, or start capability is produced here.
    #[allow(
        dead_code,
        reason = "the named Import Arm caller is composed separately"
    )]
    pub(super) fn prospective_legacy_named_import_target(
        &self,
        created: &CreatedLegacyNamedImport,
    ) -> Result<crate::broker::enforcer::ProspectiveRegistrationV1, BrokerErrorCodeV1> {
        let admitted = &created.admission;
        let generation = admitted.running.generation();
        require_generation(&admitted.job.cancel, generation)?;
        if self.stopping.load(Ordering::SeqCst) || self.fenced.load(Ordering::SeqCst) {
            return Err(BrokerErrorCodeV1::NeedsReconciliation);
        }

        let retained = &created.writer_creation_prefix;
        let plan = &admitted.material.plan;
        let scope = &admitted.request.verified.scope;
        if plan.binding_ref.is_nil()
            || plan.operation_id != admitted.accepted.operation_id()
            || canonical(admitted.accepted.scope()).map_err(jobs::journal_error)?
                != canonical(scope).map_err(jobs::journal_error)?
            || admitted.accepted.intent().legacy_named_plan() != Some(plan)
            || plan.broker_profile_id != scope.profile_id()
            || plan.control_namespace_uuid != scope.control_namespace_uuid().to_string()
            || retained.plan() != plan
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }

        let saved_receipt = retained
            .receipt()
            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
        let saved_worker = saved_receipt
            .admission
            .worker
            .as_ref()
            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
        if retained.archive_digest() != Some(admitted.archive_digest.as_str())
            || !saved_receipt.admission.accepted
            || saved_receipt.admission.refusal.is_some()
            || canonical(saved_worker).map_err(jobs::journal_error)?
                != canonical(&admitted.worker).map_err(jobs::journal_error)?
            || canonical(&saved_receipt.admission.receipt).map_err(jobs::journal_error)?
                != canonical(&admitted.reservation).map_err(jobs::journal_error)?
            || retained.import_arm().is_some()
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let preparation = retained
            .import_creation()
            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
        if preparation.owned().is_some() {
            return Err(BrokerErrorCodeV1::Conflict);
        }

        let parent = &admitted.worker;
        if parent.state != WorkerStateV1::Reserved
            || !parent.quota_held
            || admitted.reservation.worker_id.as_deref() != Some(parent.worker_id.as_str())
            || admitted.reservation.revision != Some(parent.revision)
            || parent.admission_operation_id != plan.operation_id.to_string()
            || parent.launch.session_id != plan.session_id.to_string()
            || parent.attempt.create_operation_id.is_some()
            || parent.attempt.start_operation_id.is_some()
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let policy = preparation.policy();
        if policy.purpose != HelperPurposeV1::Import
            || policy.parent_worker_id != parent.worker_id
            || policy.parent_attempt_id != parent.attempt.attempt_id
            || policy.parent_spec_hash != parent.spec_hash
            || policy.session_id != parent.launch.session_id
            || policy.helper_id != created.owned.handle.operation_id().to_string()
            || policy.timeout_seconds != 120
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }

        let admission_permission = HelperPermissionV1::Admission {
            receipt: admitted.reservation.clone(),
        };
        if canonical(preparation.permission()).map_err(jobs::journal_error)?
            != canonical(&admission_permission).map_err(jobs::journal_error)?
            || created.owned.handle.session_id() != plan.session_id
            || created.owned.handle.authority_fingerprint()
                != created.runtime.binding().fingerprint()
            || created
                .owned
                .observation
                .configuration_fingerprint
                .is_none()
            || created.owned.observation.restart_count != Some(0)
            || created.owned.observation.process.running != Some(false)
            || !matches!(
                created.owned.observation.process.status.as_deref(),
                Some("created" | "configured" | "initialized")
            )
            || created.owned.observation.process.started_at.is_some()
            || created.owned.observation.process.finished_at.is_some()
            || created.owned.observation.process.exit_code.is_some()
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        retained
            .require_import_creation_binding(created.runtime.binding())
            .map_err(jobs::journal_error)?;

        // Prepare is a pure projection from the retained Reserved worker. The
        // durable Arm will use the projected revision, never a guessed +1.
        let arm_id = LegacyNamedImportArmRequestV1::operation_id(
            plan.binding_ref,
            Uuid::parse_str(&parent.attempt.attempt_id).map_err(|_| BrokerErrorCodeV1::Conflict)?,
        );
        let prepare = WorkerMutationV1 {
            version: 1,
            operation_id: arm_id.clone(),
            worker_id: parent.worker_id.clone(),
            attempt_id: parent.attempt.attempt_id.clone(),
            expected_revision: parent.revision,
            transition: WorkerTransitionV1::Prepare,
        };
        let prepared = crate::audit::expected_worker_transition(parent, &prepare)
            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
        if prepared.state != WorkerStateV1::Preparing
            || prepared.revision
                != parent
                    .revision
                    .checked_add(1)
                    .ok_or(BrokerErrorCodeV1::Conflict)?
            || prepared.worker_id != parent.worker_id
            || prepared.attempt.attempt_id != parent.attempt.attempt_id
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }

        // Reuse the runner's pure normalization of runtime facts, then apply
        // the helper policy's exact spec identity just as the helper executor
        // does. `validate_inspection` rejects incomplete or divergent facts.
        let inspection =
            crate::runner::broker_helper_observation(parent, policy, &created.owned.observation)
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
                .inspection;
        policy
            .validate_inspection(&inspection)
            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;

        let native_clock = crate::broker::clock::NativeHostClock::new()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let clock = native_clock
            .sample()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let deadline = crate::broker::clock::checked_deadline(
            clock
                .continuous_ns()
                .map_err(|_| BrokerErrorCodeV1::Unavailable)?,
            policy.timeout_seconds,
        )
        .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let binding = created.runtime.binding();
        let arm = ArmV1 {
            version: 1,
            operation_id: arm_id,
            worker_id: parent.worker_id.clone(),
            attempt_id: parent.attempt.attempt_id.clone(),
            expected_revision: prepared.revision,
            intent_hash: admitted.accepted.intent_hash().to_owned(),
            subject: ArmSubjectV1::Helper {
                helper_id: policy.helper_id.clone(),
                purpose: HelperPurposeV1::Import,
                policy_version: policy.version,
                permission: Box::new(admission_permission),
            },
            inspection,
            authority_binding_hash: binding.fingerprint(),
            configuration_fingerprint_version: created
                .owned
                .observation
                .configuration_fingerprint_version
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?,
            configuration_fingerprint: created
                .owned
                .observation
                .configuration_fingerprint
                .clone()
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?,
            clock,
            timeout_seconds: policy.timeout_seconds,
            deadline_ns_decimal: deadline.to_string(),
        };
        arm.validate()
            .and_then(|_| arm.validate_for_parent(&prepared))
            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;

        let target = crate::broker::enforcer::ProspectiveRegistrationV1::new(
            hex::encode(Sha256::digest(scope.gateway_origin().as_bytes())),
            scope.tenant_id().to_owned(),
            arm,
            binding.clone(),
            created.owned.clone(),
        )
        .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
        require_generation(&admitted.job.cancel, generation)?;
        Ok(target)
    }

    /// Persist Arm and retain its exact native acknowledgement under the
    /// original job ownership. Only the separate claim stage can allow start.
    #[allow(dead_code, reason = "the named start claim is composed separately")]
    pub(super) async fn bind_created_legacy_named_import(
        &self,
        created: CreatedLegacyNamedImport,
    ) -> Result<BoundLegacyNamedImport, BrokerErrorCodeV1> {
        let admitted = &created.admission;
        let job = Arc::clone(&admitted.job);
        let generation = admitted.running.generation();
        let result = async {
            self.require_starts().await?;
            self.validate_control().await?;
            require_generation(&job.cancel, generation)?;
            let enforcer = self.full_enforcer()?;
            let ExecutionEnforcerV1::Legacy(client) = enforcer else {
                return Err(BrokerErrorCodeV1::IncompatibleControl);
            };
            let control = admitted
                .request
                .jobs
                .named_control
                .get()
                .ok_or(BrokerErrorCodeV1::IncompatibleControl)?;
            let writer = &admitted.request.writer;
            let target = self.prospective_legacy_named_import_target(&created)?;
            let recorded = control
                .record_legacy_named_import_arm(
                    enforcer,
                    writer,
                    &admitted.material.plan,
                    &admitted.archive_digest,
                    target,
                    (&job.cancel, generation),
                )
                .await
                .map_err(named_effect_error)?;
            let (arm, exchange) = control
                .arm_legacy_named_import(recorded)
                .await
                .map_err(named_effect_error)?;
            let retained = writer
                .load(admitted.accepted.operation_id())
                .await
                .map_err(jobs::journal_error)?
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
            self.require_starts().await?;
            self.validate_control().await?;
            require_generation(&job.cancel, generation)?;
            let bound = client
                .legacy_bind_named_prospective(&retained, &arm, exchange)
                .await
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
            require_generation(&job.cancel, generation)?;
            let LegacyNamedProspectiveBind::Bound(acknowledgement) = bound else {
                return Err(BrokerErrorCodeV1::NeedsReconciliation);
            };
            Ok(BoundLegacyNamedImport {
                created,
                acknowledgement,
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
