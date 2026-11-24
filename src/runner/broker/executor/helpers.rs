//! Private batch helpers operations.
use super::journal::{same, without_response};
use super::*;
use futures_util::{FutureExt, future::BoxFuture};

#[cfg(all(test, unix))]
#[path = "prospective_live_tests.rs"]
mod prospective_live_tests;

struct PreparedHelper {
    worker: WorkerV1,
    draft: Draft,
    preparation: HelperPreparationV1,
    policy: HelperExecutionPolicyV1,
}

enum HelperPreparationPhase {
    Completed(Box<HelperCompletion>),
    Ready(Box<PreparedHelper>),
}

impl Execution<'_> {
    /// A loaded Arm can only reconnect to the exact role assignment confirmed
    /// in V5. Its metadata never reconstructs an original registration owner.
    pub(super) async fn restore_assigned_slot(
        &self,
        registration: RegistrationV1,
    ) -> Result<EnforcedSlot> {
        let (job, _) = self.load().await?;
        let arm = Uuid::parse_str(registration.arm_id())?;
        let binding = job.broker_checkpoint_v5().and_then(|checkpoint| {
            checkpoint
                .bindings()
                .iter()
                .find(|binding| binding.arm_id() == arm)
        });
        if self.executor.enforcer.full3().is_none() {
            ensure!(
                binding.is_none(),
                "Full3 assigned Arm cannot enter a legacy guardian"
            );
            return Ok(self
                .executor
                .enforcer
                .restore(registration, self.context.cancel_generation().clone())
                .await?);
        }
        let binding = binding.context("loaded Arm has no confirmed Full3 assignment")?;
        Ok(self
            .executor
            .enforcer
            .restore_with_binding(
                registration,
                self.context.cancel_generation().clone(),
                binding,
            )
            .await?)
    }

    pub(super) async fn arm_and_claim(
        &self,
        runtime: &ManagedRuntime,
        owned: OwnedContainerSnapshot,
        helper: Option<(&HelperExecutionPolicyV1, &HelperPermissionV1)>,
    ) -> Result<PrearmedStartDirective> {
        self.arm_and_claim_bound(runtime.binding(), owned, helper)
            .await
    }

    // Keep the production entry bound to the real ManagedRuntime Created
    // callback. Unit fixtures can exercise only this control-plane boundary
    // with explicit synthetic Created metadata, without invoking a runtime.
    async fn arm_and_claim_bound(
        &self,
        binding: &crate::runtime::RuntimeAuthority,
        owned: OwnedContainerSnapshot,
        helper: Option<(&HelperExecutionPolicyV1, &HelperPermissionV1)>,
    ) -> Result<PrearmedStartDirective> {
        let (job, draft) = self.load().await?;
        let parent = draft.worker()?.clone();
        let subject = match helper {
            None => ArmSubjectV1::Worker,
            Some((policy, permission)) => ArmSubjectV1::Helper {
                helper_id: policy.helper_id.clone(),
                purpose: policy.purpose,
                policy_version: policy.version,
                permission: Box::new(permission.clone()),
            },
        };
        let previous_arm = draft
            .operations
            .iter()
            .find_map(|operation| match operation {
                Operation::Arm { request, .. }
                    if same(&request.subject, &subject).unwrap_or(false) =>
                {
                    Some(operation.clone())
                }
                _ => None,
            });
        let newly_requested_arm = previous_arm.is_none();
        let acknowledged_arm = matches!(
            previous_arm.as_ref(),
            Some(Operation::Arm {
                response: Some(_),
                ..
            })
        );
        let pending = if let Some(previous) = previous_arm {
            without_response(&previous)
        } else {
            self.executor.enforcer.require_new_prearm_identity().context(
                "fresh Arm refused: prior pending publication is unknown; controlled reconciliation is required",
            )?;
            self.generation_current()?;
            let clock = self.executor.clock.sample()?;
            let timeout_seconds = helper
                .map(|(policy, _)| policy.timeout_seconds)
                .unwrap_or(parent.launch.timeout_seconds);
            let inspection = match helper {
                Some((policy, _)) => {
                    managed::broker_helper_observation(&parent, policy, &owned.observation)?
                        .inspection
                }
                None => managed::inspection(&parent, &owned.observation)?,
            };
            let request = ArmV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                worker_id: parent.worker_id.clone(),
                attempt_id: parent.attempt.attempt_id.clone(),
                expected_revision: parent.revision,
                intent_hash: job.intent_hash().into(),
                subject,
                inspection,
                authority_binding_hash: binding.fingerprint(),
                configuration_fingerprint_version: owned
                    .observation
                    .configuration_fingerprint_version
                    .context("sandbox fingerprint version is unknown")?,
                configuration_fingerprint: owned
                    .observation
                    .configuration_fingerprint
                    .clone()
                    .context("sandbox fingerprint is unknown")?,
                deadline_ns_decimal: checked_deadline(clock.continuous_ns()?, timeout_seconds)?
                    .to_string(),
                clock,
                timeout_seconds,
            };
            Operation::Arm {
                parent,
                owned: owned.clone(),
                request,
                response: None,
            }
        };
        let arm_owned = match &pending {
            Operation::Arm { owned, .. } => owned.clone(),
            _ => return Err(anyhow!("prospective Arm target is missing")),
        };
        let mut prospective_exchange = None;
        let mut fresh_original = false;
        // The native owner must already retain the immutable target, runtime
        // and exclusive S capacity before the gateway can acknowledge an Arm.
        // The managed runtime invokes this path only after inspecting Created
        // and validating the exact configuration/allocation. Historical
        // pending DATA may reconnect to that same owner, never allocate one.
        if !acknowledged_arm {
            let enforcer = &self.executor.enforcer;
            let Operation::Arm { request, owned, .. } = &pending else {
                return Err(anyhow!("prospective Arm target is missing"));
            };
            let target = crate::broker::enforcer::ProspectiveRegistrationV1::new(
                hex::encode(Sha256::digest(
                    self.context.scope().gateway_origin().as_bytes(),
                )),
                self.context.scope().tenant_id().into(),
                request.clone(),
                binding.clone(),
                owned.clone(),
            )?;
            // Fix the identity/deadline durably before native allocation. An
            // unknown first publication fences new local identities; a later
            // empty-looking read cannot cause another Arm to be generated.
            let key = operation_key(&pending);
            let confirmed = if newly_requested_arm {
                self.patch(|draft| {
                    if let Some(existing) =
                        draft.operations.iter().find(|op| operation_key(op) == key)
                    {
                        ensure!(
                            same(&without_response(existing), &pending)?,
                            "pending Arm identity conflicts"
                        );
                    } else {
                        draft.operations.push(pending.clone());
                    }
                    Ok(())
                })
                .await
            } else {
                // Preserve the second strict identity check formerly done by
                // patch, without allocating an unchanged journal revision.
                async {
                    let (_, draft) = self.load().await?;
                    let existing = draft
                        .operations
                        .iter()
                        .find(|op| operation_key(op) == key)
                        .context("existing pending Arm disappeared")?;
                    ensure!(
                        same(&without_response(existing), &pending)?,
                        "pending Arm identity conflicts"
                    );
                    Ok(())
                }
                .await
            };
            if let Err(error) = confirmed {
                enforcer.fence_unknown_prearm_write()?;
                let context = if newly_requested_arm {
                    "pending Arm publication is unknown; no Arm RPC was issued"
                } else {
                    "existing pending Arm confirmation is unavailable; no Arm RPC was issued"
                };
                return Err(error.context(context));
            }
            enforcer
                .reserve_prospective(
                    target.clone(),
                    self.context.cancel_generation().clone(),
                    !newly_requested_arm,
                )
                .await
                .context(diagnostics::RegistrationStage::ReserveProspective)?;
            if let Err(error) = self.generation_current() {
                if let ExecutionEnforcerV1::Legacy(client) = enforcer {
                    if let Err(cancel) = client
                        .legacy_cancel_prospective(
                            &target,
                            self.context.cancel_generation().current(),
                        )
                        .await
                    {
                        return Err(error.context(cancel));
                    }
                }
                return Err(error);
            }
            let exchange = enforcer
                .begin_prospective_arm_exchange(&target, self.context.cancel_generation())
                .await
                .context(diagnostics::RegistrationStage::BeginExchange)?;
            if let Err(error) = self.generation_current() {
                if let ExecutionEnforcerV1::Legacy(client) = enforcer {
                    if let Err(cancel) = client
                        .legacy_cancel_prospective(
                            &target,
                            self.context.cancel_generation().current(),
                        )
                        .await
                    {
                        return Err(error.context(cancel));
                    }
                }
                return Err(error);
            }
            fresh_original = newly_requested_arm && exchange.permits_original();
            prospective_exchange = Some(if fresh_original {
                exchange
            } else {
                exchange.into_restrictive()
            });
        }
        let (arm, _, committed) = self
            .rpc_checked_at_head(pending, None)
            .await
            .context(diagnostics::BrokerStep::ArmExchange)?;
        let Operation::Arm {
            request,
            response: Some(response),
            ..
        } = arm
        else {
            return Err(anyhow!("arm acknowledgement is missing")
                .context(diagnostics::BrokerStep::ArmExchange));
        };
        response
            .receipt
            .validate_for_arm(&request)
            .context(diagnostics::BrokerStep::ArmExchange)?;
        // The exact acknowledged head is data, not a latest-state or start
        // capability. Reject any operation/profile mismatch before using it.
        let (_confirmed_arm_head, current) = committed
            .into_parts(self.operation, self.context.scope())
            .context(diagnostics::BrokerStep::ArmExchange)?;
        if let Some(full) = self.executor.enforcer.full3()
            && fresh_original
        {
            self.context.preclaim().announce_registration(
                self.operation,
                Uuid::parse_str(&request.operation_id)?,
                full.epoch(),
                self.generation,
            )?;
        }
        let registration = RegistrationV1::new(
            hex::encode(Sha256::digest(
                self.context.scope().gateway_origin().as_bytes(),
            )),
            self.context.scope().tenant_id().into(),
            request.clone(),
            response.receipt.clone(),
            binding.clone(),
            arm_owned,
        )
        .context(diagnostics::RegistrationStage::BuildRegistration)?;
        let (slot, stage) = if !acknowledged_arm {
            (
                self.executor
                    .enforcer
                    .bind_prospective(
                        registration,
                        self.context.cancel_generation().clone(),
                        prospective_exchange
                            .take()
                            .context("native Arm exchange phase is unavailable")?,
                    )
                    .await
                    .map_err(anyhow::Error::from),
                diagnostics::RegistrationStage::BindProspective,
            )
        } else if self.executor.enforcer.full3().is_some() {
            (
                self.restore_assigned_slot(registration).await,
                diagnostics::RegistrationStage::RestoreAssigned,
            )
        } else {
            (
                self.executor
                    .enforcer
                    .register(registration, self.context.cancel_generation().clone())
                    .await
                    .map_err(anyhow::Error::from),
                diagnostics::RegistrationStage::RegisterLegacy,
            )
        };
        let slot = slot
            .map_err(|code| -> anyhow::Error {
                let Some(code) = code
                    .downcast_ref::<crate::broker::enforcer::EnforcerError>()
                    .copied()
                else {
                    return code;
                };
                match crate::broker::enforcer::EnforcerStageFailure::registration_failure(
                    &request, code,
                ) {
                    Some(failure) => failure.into(),
                    None => code.into(),
                }
            })
            .context(stage)?;
        if !acknowledged_arm && !fresh_original {
            // A replayed pending Arm can obtain its exact receipt and STOP
            // binding, but cannot reconstruct the original fresh permission.
            return Ok(PrearmedStartDirective::ObserveOnly);
        }
        if self.executor.enforcer.full3().is_some() && fresh_original {
            let binding = slot.full3_registration_binding()?;
            let mut confirmed = false;
            for _ in 0..16 {
                // An authenticated abandon signal or Cancel can advance the
                // head while native Register is in flight. Keep this exact
                // original slot/binding; merge only through the writer CAS.
                let (latest, _) = self.load().await?;
                match self
                    .executor
                    .writer
                    .bind_full3_registration(self.operation, latest.head().clone(), binding.clone())
                    .await
                {
                    Ok(_) => {
                        confirmed = true;
                        break;
                    }
                    Err(JournalError::Conflict) => continue,
                    Err(error) => return Err(error.into()),
                }
            }
            ensure!(
                confirmed,
                "Full3 assignment publication requires reconciliation"
            );
        }
        // Later mutations still reload and use the writer's strict CAS.
        if current.operations.iter().any(|op| match op {
            Operation::WorkerClaim { request: claim, .. } => {
                claim.arm_receipt.arm_id == request.operation_id
            }
            Operation::HelperClaim { request: claim, .. } => {
                claim.arm_receipt.arm_id == request.operation_id
            }
            _ => false,
        }) {
            return Ok(PrearmedStartDirective::ObserveOnly);
        }
        self.generation_current()
            .context(diagnostics::BrokerStep::ReadinessBeforeClaim)?;
        // This is mandatory immediately before the HTTP claim, in addition to
        // the enforcer's consumed guard immediately before the engine spawn.
        slot.check_ready_for_claim_diagnosed(self.generation)
            .context(diagnostics::BrokerStep::ReadinessBeforeClaim)?;
        if let Some(full) = self.executor.enforcer.full3() {
            let arm_id = Uuid::parse_str(&request.operation_id)?;
            self.context.preclaim().mark_original_unclaimed(
                arm_id,
                full.epoch(),
                self.generation,
            )?;
            match self
                .context
                .preclaim()
                .choose_claim(arm_id, full.epoch(), self.generation)
            {
                Ok(reservation) => reservation.consume()?,
                Err(crate::broker::service::protocol::BrokerErrorCodeV1::Conflict) => {
                    let signal = self
                        .context
                        .preclaim()
                        .signal_for_arm(arm_id, full.epoch(), self.generation)?
                        .context("preclaim reservation is unavailable")?;
                    return self
                        .retire_unused_before_claim(slot, &request, signal)
                        .await;
                }
                Err(error) => return Err(error.into()),
            }
        }
        let operation_id = Uuid::new_v4().to_string();
        let emitter_id = Uuid::new_v4().to_string();
        let check = self
            .executor
            .clock
            .sample()
            .context(diagnostics::BrokerStep::ClockBeforeClaim)?;
        let claim = if let Some((policy, _)) = helper {
            Operation::HelperClaim {
                request: HelperStartClaimV1 {
                    version: 1,
                    operation_id: operation_id.clone(),
                    helper_id: policy.helper_id.clone(),
                    emitter_id: emitter_id.clone(),
                    arm_receipt: response.receipt.clone(),
                    clock_check: check,
                },
                response: None,
            }
        } else {
            Operation::WorkerClaim {
                request: ArmedStartClaimV1 {
                    version: 1,
                    operation_id: operation_id.clone(),
                    worker_id: request.worker_id.clone(),
                    attempt_id: request.attempt_id.clone(),
                    expected_revision: current.worker()?.revision,
                    emitter_id: emitter_id.clone(),
                    arm_receipt: response.receipt.clone(),
                    clock_check: check,
                },
                response: None,
            }
        };
        let reservation = slot.reserve_claim(self.generation)?;
        let claim_arm_id = match &claim {
            Operation::WorkerClaim { request, .. } => &request.arm_receipt.arm_id,
            Operation::HelperClaim { request, .. } => &request.arm_receipt.arm_id,
            _ => return Err(anyhow!("invalid preclaim operation")),
        };
        ensure!(
            claim_arm_id == &request.operation_id,
            "claim selected a different Arm"
        );
        slot.consume_claim_reservation(reservation, &request, self.generation)?;
        let (acknowledged, fresh) = self
            .rpc_checked(claim, Some(&slot))
            .await
            .context(diagnostics::BrokerStep::ClaimExchange)?;
        let emit = match acknowledged {
            Operation::HelperClaim {
                response: Some(response),
                ..
            } => response.disposition == ClaimDispositionV1::EmitOnce,
            Operation::WorkerClaim {
                response: Some(response),
                ..
            } => response.disposition == Some(ClaimDispositionV1::EmitOnce),
            _ => false,
        };
        if !fresh || !emit {
            return Ok(PrearmedStartDirective::ObserveOnly);
        }
        self.generation_current()
            .context(diagnostics::BrokerStep::GenerationBeforePrepare)?;
        let permission = FreshStartPermission {
            arm_id: request.operation_id.clone(),
            worker_id: request.worker_id.clone(),
            attempt_id: request.attempt_id.clone(),
            control_namespace: response.receipt.control_namespace_uuid,
            subject_key: request
                .subject_key()
                .context(diagnostics::BrokerStep::GenerationBeforePrepare)?,
            emitter_id,
            claim_operation_id: operation_id,
            generation: self.generation,
        };
        Ok(PrearmedStartDirective::EmitOnce(
            slot.prepare_start_diagnosed(permission, self.generation)
                .await
                .context(diagnostics::BrokerStep::Prepare)?,
        ))
    }

    pub(super) async fn helper(
        &self,
        runtime: &ManagedRuntime,
        purpose: HelperPurposeV1,
        permission: HelperPermissionV1,
        request_id: Option<Uuid>,
    ) -> Result<BatchStep<HelperCompletion>> {
        // A phase owns its future on the heap, but stays with this same caller.
        // Completed preparation/result polls must not remain on the native
        // stack while the prearmed transport polls its ownership/Arm callback.
        let prepared = match self
            .prepare_helper(runtime, purpose, permission, request_id)
            .await?
        {
            HelperPreparationPhase::Completed(completion) => {
                return Ok(ControlFlow::Continue(*completion));
            }
            HelperPreparationPhase::Ready(prepared) => prepared,
        };
        let retained = match self.execute_helper(runtime, &prepared).await? {
            crate::runtime::ManagedPrearmedOutcome::Executed(retained) => retained,
            crate::runtime::ManagedPrearmedOutcome::Abandoned { owned, retirement } => {
                return Ok(ControlFlow::Break(Box::new(AbandonedBatch {
                    owned,
                    retirement,
                })));
            }
        };
        self.complete_helper(runtime, prepared, retained).await
    }

    fn prepare_helper<'phase>(
        &'phase self,
        runtime: &'phase ManagedRuntime,
        purpose: HelperPurposeV1,
        permission: HelperPermissionV1,
        request_id: Option<Uuid>,
    ) -> BoxFuture<'phase, Result<HelperPreparationPhase>> {
        async move {
            self.stage(match purpose {
                HelperPurposeV1::Import => LocalFailurePhaseV1::Import,
                HelperPurposeV1::ImportVerification => LocalFailurePhaseV1::ImportVerification,
                HelperPurposeV1::FinalScan => LocalFailurePhaseV1::FinalScan,
                HelperPurposeV1::Export => LocalFailurePhaseV1::Export,
            });
            let (_, draft) = self.load().await?;
            let worker = draft.worker()?.clone();
            // Import has one immutable subject per attempt. Other helpers are
            // selected by their exact durable permission, never a caller argv.
            let mut preparation = None;
            for helper in &draft.helpers {
                if helper.policy().purpose != purpose
                    || !same(helper.permission(), &permission)?
                    || request_id.is_some_and(|id| helper.policy().helper_id != id.to_string())
                {
                    continue;
                }
                let verification_lost = purpose == HelperPurposeV1::ImportVerification
                    && draft.operations.iter().any(|op| matches!(op,
                        Operation::HelperObservation {request, response: Some(_)}
                        if request.helper_id == helper.policy().helper_id && request.verification.is_none()
                        && request.observation.finished_at.is_some()));
                if !verification_lost {
                    preparation = Some(helper.clone());
                    break;
                }
            }
            let preparation = match preparation {
                Some(preparation) => preparation,
                None => {
                    self.generation_current()?;
                    let policy = helper_execution_policy_v1(
                        &worker,
                        purpose,
                        request_id.unwrap_or_else(Uuid::new_v4),
                    )?;
                    let preparation = HelperPreparationV1::new(policy, permission, None)?;
                    self.patch(|draft| {
                        self.generation_current()?;
                        ensure!(
                            !draft
                                .helpers
                                .iter()
                                .any(|h| h.policy().helper_id == preparation.policy().helper_id),
                            "helper already exists"
                        );
                        draft.helpers.push(preparation.clone());
                        Ok(())
                    })
                    .await?;
                    preparation
                }
            };
            let policy = preparation.policy().clone();
            let (_, draft) = self.load().await?;
            if let Some(completion) = completed_helper(&draft, &policy)? {
                self.finish_helper_cleanup(runtime, &preparation, &completion.arm)
                    .await?;
                return Ok(HelperPreparationPhase::Completed(Box::new(completion)));
            }
            if let Some(owned) = preparation.owned()
                && draft.operations.iter().any(|op| matches!(op,
                    Operation::HelperClaim {request, response: Some(_)} if request.helper_id == policy.helper_id))
            {
                let (arm, receipt) = draft.operations.iter().find_map(|op| match op {
                    Operation::Arm {request, response: Some(response), ..}
                        if matches!(&request.subject, ArmSubjectV1::Helper {helper_id, ..} if helper_id == &policy.helper_id)
                        => Some((request.clone(), response.receipt.clone())),
                    _ => None,
                }).context("historical helper arm is missing")?;
                self.restore_assigned_slot(RegistrationV1::new(
                    hex::encode(Sha256::digest(self.context.scope().gateway_origin().as_bytes())),
                    self.context.scope().tenant_id().into(), arm, receipt.clone(),
                    runtime.binding().clone(), owned.clone(),
                )?).await?;
                let terminal = runtime.resume_terminal(&owned.handle).await?;
                let observation = managed::broker_helper_observation(&worker, &policy, &terminal)?;
                let request = HelperObservationV1 {version: 1, operation_id: Uuid::new_v4().to_string(),
                    helper_id: policy.helper_id.clone(), arm_receipt: receipt.clone(), observation, verification: None};
                let (ack, _) = self.rpc(Operation::HelperObservation { request, response: None }).await?;
                let Operation::HelperObservation {response: Some(terminal_receipt), request} = ack else {
                    return Err(anyhow!("helper terminal acknowledgement is missing"));
                };
                self.finish_helper_cleanup(runtime, &preparation, &receipt).await?;
                ensure!(request.observation.exit_code == Some(0), "historical helper failed");
                ensure!(purpose == HelperPurposeV1::Import,
                    "historical helper transport bytes are unknown; fresh verification or export is required");
                return Ok(HelperPreparationPhase::Completed(Box::new(HelperCompletion { policy, arm: receipt, receipt: terminal_receipt, bytes: None, manifest: None })));
            }
            Ok(HelperPreparationPhase::Ready(Box::new(PreparedHelper {
                worker,
                draft,
                preparation,
                policy,
            })))
        }
        .boxed()
    }

    fn execute_helper<'phase>(
        &'phase self,
        runtime: &'phase ManagedRuntime,
        prepared: &'phase PreparedHelper,
    ) -> BoxFuture<'phase, Result<crate::runtime::ManagedPrearmedOutcome>> {
        async move {
            let PreparedHelper {
                worker,
                draft,
                preparation,
                policy,
            } = prepared;
            let purpose = policy.purpose;
            let input = if purpose == HelperPurposeV1::Import {
                let checkpoint = draft
                    .run
                    .plan
                    .managed
                    .as_ref()
                    .context("workspace preparation is missing")?
                    .clone();
                Some(
                    tokio::task::spawn_blocking(move || managed::broker_archive(&checkpoint))
                        .await??,
                )
            } else {
                None
            };
            let spec = managed::broker_helper_spec(&policy)?;
            let recovered = match preparation.owned() {
                Some(owned) => Some(owned.clone()),
                None => runtime.lookup_created(&spec).await?,
            };
            let permission = preparation.permission().clone();
            let on_created = |owned: OwnedContainerSnapshot| {
                let policy = policy.clone();
                let permission = permission.clone();
                async move {
                    self.patch(|draft| {
                        let current = draft
                            .helpers
                            .iter_mut()
                            .find(|h| h.policy().helper_id == policy.helper_id)
                            .context("prepared helper disappeared")?;
                        if let Some(previous) = current.owned() {
                            ensure!(
                                same(&previous.handle, &owned.handle)?,
                                "owned helper changed"
                            );
                            owned
                                .observation
                                .validate_identity_allocation(&previous.observation)?;
                        } else {
                            *current = HelperPreparationV1::new(
                                policy.clone(),
                                permission.clone(),
                                Some(owned.clone()),
                            )?;
                        }
                        Ok(())
                    })
                    .await?;
                    let (_, draft) = self.load().await?;
                    let durable = draft
                        .helpers
                        .iter()
                        .find(|h| h.policy().helper_id == policy.helper_id)
                        .and_then(|h| h.owned())
                        .cloned()
                        .context("helper ownership was not persisted")?;
                    self.arm_and_claim(runtime, durable, Some((&policy, &permission)))
                        .boxed()
                        .await
                }
                .boxed()
            };
            let retained = if let Some(owned) = recovered.as_ref() {
                runtime
                    .start_owned_prearmed_retained(
                        &spec,
                        owned,
                        input,
                        Limits::default().max_archive_bytes(),
                        on_created,
                        |_observation| async { Ok(()) },
                    )
                    .await?
            } else {
                self.generation_current()?;
                runtime
                    .execute_prearmed_retained(
                        &spec,
                        input,
                        Limits::default().max_archive_bytes(),
                        on_created,
                        |_observation| async { Ok(()) },
                    )
                    .await?
            };
            Ok(retained)
        }
        .boxed()
    }

    fn complete_helper<'phase>(
        &'phase self,
        runtime: &'phase ManagedRuntime,
        prepared: Box<PreparedHelper>,
        retained: ManagedPrearmedExecution,
    ) -> BoxFuture<'phase, Result<BatchStep<HelperCompletion>>> {
        async move {
            let PreparedHelper {
                worker,
                draft,
                preparation,
                policy,
            } = *prepared;
            let purpose = policy.purpose;
            let observation =
                managed::broker_helper_observation(&worker, &policy, retained.terminal())?;
            ensure!(
                observation.exit_code.is_some()
                    && observation.running == Some(false)
                    && observation.started_at.is_some()
                    && observation.finished_at.is_some(),
                "helper did not prove terminal execution"
            );
            let succeeded = observation.exit_code == Some(0);
            let bytes = retained.result().stdout.bytes.clone();
            let manifest: Result<Option<workspace::Manifest>> =
                if retained.result().stdout.truncated {
                    Err(anyhow!("helper archive exceeded its bound"))
                } else if purpose != HelperPurposeV1::Import {
                    let bytes = bytes.clone();
                    tokio::task::spawn_blocking(move || {
                        workspace::scan_workspace_archive(bytes.as_slice(), Limits::default())
                    })
                    .await
                    .map_err(anyhow::Error::from)
                    .and_then(|result| result)
                    .map(Some)
                } else {
                    Ok(None)
                };
            let verification = if let HelperPermissionV1::ImportVerification {
                import_helper_id,
                manifest_digest,
                ..
            } = preparation.permission()
            {
                if let Ok(Some(manifest)) = &manifest {
                    let recorded = &draft
                        .run
                        .plan
                        .managed
                        .as_ref()
                        .context("source manifest is missing")?
                        .before;
                    let observed = workspace::manifest_at_recorded_coverage(recorded, manifest);
                    Some(ImportVerificationReportV1 {
                        import_helper_id: import_helper_id.clone(),
                        expected_manifest_digest: manifest_digest.clone(),
                        observed_manifest_digest: content_hash(
                            "ai-guard:workspace-manifest:v1",
                            &observed,
                        )?,
                    })
                } else {
                    None
                }
            } else {
                None
            };
            let verified = purpose != HelperPurposeV1::ImportVerification
                || verification.as_ref().is_some_and(|report| {
                    report.expected_manifest_digest == report.observed_manifest_digest
                });
            let arm = self.helper_arm(&policy.helper_id).await?;
            let request = HelperObservationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                helper_id: policy.helper_id.clone(),
                arm_receipt: arm.clone(),
                observation,
                verification,
            };
            let (ack, _) = self
                .rpc(Operation::HelperObservation {
                    request,
                    response: None,
                })
                .await?;
            let Operation::HelperObservation {
                response: Some(receipt),
                ..
            } = ack
            else {
                return Err(anyhow!("helper terminal acknowledgement is missing"));
            };
            let removed = runtime
                .cleanup_terminal(retained.handle(), retained.terminal())
                .await
                .is_ok();
            let cleanup = HelperCleanupV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                helper_id: policy.helper_id.clone(),
                arm_receipt: arm.clone(),
                removed,
                error: (!removed).then(|| "helper cleanup was not confirmed".into()),
            };
            self.rpc(Operation::HelperCleanup {
                request: cleanup,
                response: None,
            })
            .await?;
            ensure!(removed, "helper cleanup requires reconciliation");
            ensure!(succeeded, "helper reported a nonzero exit status");
            let manifest = manifest?;
            ensure!(
                verified,
                "import verification differs from its immutable source"
            );
            Ok(ControlFlow::Continue(HelperCompletion {
                policy,
                arm,
                receipt,
                bytes: Some(bytes),
                manifest,
            }))
        }
        .boxed()
    }

    pub(super) async fn helper_arm(&self, helper_id: &str) -> Result<ArmReceiptV1> {
        let (_, draft) = self.load().await?;
        draft.operations.iter().find_map(|op| match op {
            Operation::Arm { request, response: Some(response), .. }
                if matches!(&request.subject, ArmSubjectV1::Helper { helper_id: id, .. } if id == helper_id) => Some(response.receipt.clone()),
            _ => None,
        }).context("helper arm is missing")
    }

    pub(super) async fn finish_helper_cleanup(
        &self,
        runtime: &ManagedRuntime,
        helper: &HelperPreparationV1,
        arm: &ArmReceiptV1,
    ) -> Result<()> {
        let (_, draft) = self.load().await?;
        if draft.operations.iter().any(|op| {
            matches!(op, Operation::HelperCleanup {request, response: Some(_)}
            if request.helper_id == helper.policy().helper_id && request.removed)
        }) {
            return Ok(());
        }
        let owned = helper.owned().context("helper ownership is missing")?;
        let terminal = runtime.resume_terminal(&owned.handle).await?;
        let removed = runtime
            .cleanup_terminal(&owned.handle, &terminal)
            .await
            .is_ok();
        let request = HelperCleanupV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            helper_id: helper.policy().helper_id.clone(),
            arm_receipt: arm.clone(),
            removed,
            error: (!removed).then(|| "helper cleanup was not confirmed".into()),
        };
        self.rpc(Operation::HelperCleanup {
            request,
            response: None,
        })
        .await?;
        ensure!(removed, "helper cleanup requires reconciliation");
        Ok(())
    }
}

pub(super) struct HelperCompletion {
    pub(super) policy: HelperExecutionPolicyV1,
    pub(super) arm: ArmReceiptV1,
    pub(super) receipt: MutationReceiptV1,
    pub(super) bytes: Option<Vec<u8>>,
    pub(super) manifest: Option<workspace::Manifest>,
}
fn completed_helper(
    draft: &Draft,
    policy: &HelperExecutionPolicyV1,
) -> Result<Option<HelperCompletion>> {
    for operation in draft.operations.iter().rev() {
        if let Operation::HelperObservation {
            request,
            response: Some(receipt),
        } = operation
            && request.helper_id == policy.helper_id
            && request.observation.exit_code == Some(0)
            && request.observation.running == Some(false)
            && request.observation.started_at.is_some()
            && request.observation.finished_at.is_some()
        {
            if policy.purpose == HelperPurposeV1::ImportVerification {
                let report = request
                    .verification
                    .as_ref()
                    .context("verification report is missing")?;
                ensure!(
                    report.expected_manifest_digest == report.observed_manifest_digest,
                    "import verification did not match"
                );
            }
            return Ok(Some(HelperCompletion {
                policy: policy.clone(),
                arm: request.arm_receipt.clone(),
                receipt: receipt.clone(),
                bytes: None,
                manifest: None,
            }));
        }
    }
    Ok(None)
}
