//! Private batch helpers operations.
use super::journal::{same, without_response};
use super::*;

impl Execution<'_> {
    pub(super) async fn arm_and_claim(
        &self,
        runtime: &ManagedRuntime,
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
        let pending = if let Some(previous) = previous_arm {
            without_response(&previous)
        } else {
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
                authority_binding_hash: runtime.binding().fingerprint(),
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
        let (arm, _) = self.rpc(pending).await?;
        let Operation::Arm {
            request,
            response: Some(response),
            ..
        } = arm
        else {
            return Err(anyhow!("arm acknowledgement is missing"));
        };
        response.receipt.validate_for_arm(&request)?;
        let slot = self
            .executor
            .enforcer
            .register(
                RegistrationV1::new(
                    hex::encode(Sha256::digest(
                        self.context.scope().gateway_origin().as_bytes(),
                    )),
                    self.context.scope().tenant_id().into(),
                    request.clone(),
                    response.receipt.clone(),
                    runtime.binding().clone(),
                    owned,
                )?,
                self.context.cancel_generation().clone(),
            )
            .await?;
        let (_, current) = self.load().await?;
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
        self.generation_current()?;
        // This is mandatory immediately before the HTTP claim, in addition to
        // the enforcer's consumed guard immediately before the engine spawn.
        slot.check_ready_for_claim(self.generation)?;
        let operation_id = Uuid::new_v4().to_string();
        let emitter_id = Uuid::new_v4().to_string();
        let check = self.executor.clock.sample()?;
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
        let (acknowledged, fresh) = self.rpc_checked(claim, Some(&slot)).await?;
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
        self.generation_current()?;
        let permission = FreshStartPermission {
            arm_id: request.operation_id.clone(),
            worker_id: request.worker_id.clone(),
            attempt_id: request.attempt_id.clone(),
            control_namespace: response.receipt.control_namespace_uuid,
            subject_key: request.subject_key()?,
            emitter_id,
            claim_operation_id: operation_id,
            generation: self.generation,
        };
        Ok(PrearmedStartDirective::EmitOnce(
            slot.prepare_start_diagnosed(permission, self.generation)
                .await?,
        ))
    }

    pub(super) async fn helper(
        &self,
        runtime: &ManagedRuntime,
        purpose: HelperPurposeV1,
        permission: HelperPermissionV1,
        request_id: Option<Uuid>,
    ) -> Result<HelperCompletion> {
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
            return Ok(completion);
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
            self.executor.enforcer.restore(RegistrationV1::new(
                hex::encode(Sha256::digest(self.context.scope().gateway_origin().as_bytes())),
                self.context.scope().tenant_id().into(), arm, receipt.clone(),
                runtime.binding().clone(), owned.clone(),
            )?, self.context.cancel_generation().clone()).await?;
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
            return Ok(HelperCompletion { policy, arm: receipt, receipt: terminal_receipt, bytes: None, manifest: None });
        }
        let input = if purpose == HelperPurposeV1::Import {
            let checkpoint = draft
                .run
                .plan
                .managed
                .as_ref()
                .context("workspace preparation is missing")?
                .clone();
            Some(tokio::task::spawn_blocking(move || managed::broker_archive(&checkpoint)).await??)
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
                    .await
            }
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
        let manifest: Result<Option<workspace::Manifest>> = if retained.result().stdout.truncated {
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
                Some(ImportVerificationReportV1 {
                    import_helper_id: import_helper_id.clone(),
                    expected_manifest_digest: manifest_digest.clone(),
                    observed_manifest_digest: content_hash(
                        "ai-guard:workspace-manifest:v1",
                        manifest,
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
        Ok(HelperCompletion {
            policy,
            arm,
            receipt,
            bytes: Some(bytes),
            manifest,
        })
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
