use super::*;

fn normalized_request(request: &DecisionRequest, intent: &ExecutionIntentV1) -> JournalResult<()> {
    let expected = crate::policy::request_hash(intent.request_data());
    ensure(if request.human_approved {
        request.approval_hash.as_deref() == Some(expected.as_str())
    } else {
        request.approval_hash.is_none()
    })?;
    let mut initial = request.clone();
    initial.human_approved = false;
    initial.approval_hash = None;
    ensure(equal(&initial, intent.request_data())?)
}
pub(super) fn bound_receipt(
    value: &MutationReceiptV1,
    operation: &str,
    input: &str,
    worker: Option<&str>,
    revision: Option<u64>,
) -> JournalResult<()> {
    receipt(value)?;
    ensure(
        value.operation_id == operation
            && value.input_hash == input
            && value.worker_id.as_deref() == worker
            && value.revision == revision,
    )
}
pub(super) fn bound_worker(
    worker: &WorkerV1,
    admission: &SupervisedAdmissionV1,
    namespace: Uuid,
) -> JournalResult<()> {
    let id = uuid(&worker.worker_id)?;
    uuid(&worker.attempt.attempt_id)?;
    ensure(
        worker.revision > 0
            && worker.admission_operation_id == admission.admission.operation_id
            && worker.request_id == admission.admission.decision_request.request_id,
    )?;
    ensure(
        equal(&worker.receipt, &admission.admission.receipt)?
            && equal(&worker.launch, &admission.admission.launch)?,
    )?;
    ensure(worker.spec_hash == checked(worker.launch.spec_hash())?)?;
    ensure(
        worker.attempt.stable_name == format!("aig-{}-{}", namespace.simple(), id.simple())
            && worker.attempt.volume_name == format!("{}-work", worker.attempt.stable_name),
    )?;
    for operation in [
        &worker.attempt.create_operation_id,
        &worker.attempt.start_operation_id,
        &worker.attempt.start_emitter_id,
    ]
    .into_iter()
    .flatten()
    {
        uuid(operation)?;
    }
    if let Some(cid) = &worker.attempt.container_id {
        hash(cid)?;
    }
    Ok(())
}
fn find_arm<'a>(
    previous: &'a [SupervisionOperationV1],
    given: &ArmReceiptV1,
) -> JournalResult<&'a ArmV1> {
    for operation in previous {
        if let SupervisionOperationV1::Arm {
            request,
            response: Some(response),
            ..
        } = operation
            && equal(&response.receipt, given)?
        {
            return Ok(request);
        }
    }
    Err(JournalError::Conflict)
}
fn known_worker(previous: &[SupervisionOperationV1], worker: &WorkerV1) -> JournalResult<()> {
    for operation in previous {
        let saved = match operation {
            SupervisionOperationV1::Admission {
                response: Some(response),
                ..
            } => response.worker.as_ref(),
            SupervisionOperationV1::WorkerMutation {
                response: Some(response),
                ..
            }
            | SupervisionOperationV1::WorkerClaim {
                response: Some(response),
                ..
            } => Some(&response.worker),
            _ => None,
        };
        if let Some(saved) = saved
            && equal(saved, worker)?
        {
            return Ok(());
        }
    }
    Err(JournalError::Conflict)
}
fn matching_worker<'a>(
    previous: &'a [SupervisionOperationV1],
    worker_id: &str,
    attempt: &str,
    revision: u64,
) -> JournalResult<&'a WorkerV1> {
    for operation in previous {
        let worker = match operation {
            SupervisionOperationV1::Admission {
                response: Some(response),
                ..
            } => response.worker.as_ref(),
            SupervisionOperationV1::WorkerMutation {
                response: Some(response),
                ..
            }
            | SupervisionOperationV1::WorkerClaim {
                response: Some(response),
                ..
            } => Some(&response.worker),
            _ => None,
        };
        if let Some(worker) = worker
            && worker.worker_id == worker_id
            && worker.attempt.attempt_id == attempt
            && worker.revision == revision
        {
            return Ok(worker);
        }
    }
    Err(JournalError::Conflict)
}
fn has_helper_claim(
    previous: &[SupervisionOperationV1],
    helper: &str,
    arm: &ArmReceiptV1,
) -> JournalResult<()> {
    for operation in previous {
        if let SupervisionOperationV1::HelperClaim {
            request,
            response: Some(_),
            ..
        } = operation
            && request.helper_id == helper
            && equal(&request.arm_receipt, arm)?
        {
            return Ok(());
        }
    }
    Err(JournalError::Conflict)
}
fn terminal(observation: &RuntimeObservationV1) -> bool {
    let start = observation
        .started_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
    let end = observation
        .finished_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
    observation.running == Some(false)
        && matches!(
            observation.status,
            RuntimeStatusV1::Exited | RuntimeStatusV1::Dead
        )
        && observation.exit_code.is_some()
        && observation.restart_count == Some(0)
        && start
            .zip(end)
            .is_some_and(|(start, end)| start.timestamp() > 0 && end >= start)
}
fn verified_import(
    previous: &[SupervisionOperationV1],
    worker: &str,
    attempt: &str,
    manifest: &str,
) -> JournalResult<()> {
    for operation in previous {
        if let SupervisionOperationV1::HelperObservation {
            request,
            response: Some(_),
        } = operation
            && let Some(report) = &request.verification
            && terminal(&request.observation)
            && request.observation.exit_code == Some(0)
            && report.expected_manifest_digest == manifest
            && report.observed_manifest_digest == manifest
        {
            let arm = find_arm(previous, &request.arm_receipt)?;
            if arm.worker_id == worker
                && arm.attempt_id == attempt
                && matches!(
                    &arm.subject,
                    ArmSubjectV1::Helper {
                        purpose: HelperPurposeV1::ImportVerification,
                        ..
                    }
                )
            {
                return Ok(());
            }
        }
    }
    Err(JournalError::Conflict)
}
fn permission(
    previous: &[SupervisionOperationV1],
    permission: &HelperPermissionV1,
    purpose: HelperPurposeV1,
    worker: &WorkerV1,
) -> JournalResult<()> {
    match permission {
        HelperPermissionV1::Admission { receipt } => {
            ensure(matches!(
                purpose,
                HelperPurposeV1::Import | HelperPurposeV1::FinalScan
            ))?;
            for operation in previous {
                if let SupervisionOperationV1::Admission {
                    response: Some(response),
                    ..
                } = operation
                    && response.accepted
                    && equal(&response.receipt, receipt)?
                    && receipt.worker_id.as_deref() == Some(worker.worker_id.as_str())
                {
                    return Ok(());
                }
            }
        }
        HelperPermissionV1::ImportVerification {
            import_helper_id,
            import_arm,
            import_terminal,
            manifest_digest,
        } => {
            ensure(
                purpose == HelperPurposeV1::ImportVerification
                    && manifest_digest == &worker.launch.workspace_manifest_digest,
            )?;
            let arm = find_arm(previous, import_arm)?;
            ensure(
                matches!(&arm.subject, ArmSubjectV1::Helper { helper_id, purpose: HelperPurposeV1::Import, .. } if helper_id == import_helper_id),
            )?;
            ensure(
                arm.worker_id == worker.worker_id && arm.attempt_id == worker.attempt.attempt_id,
            )?;
            for operation in previous {
                if let SupervisionOperationV1::HelperObservation {
                    request,
                    response: Some(response),
                } = operation
                    && request.helper_id == *import_helper_id
                    && equal(response, import_terminal)?
                    && equal(&request.arm_receipt, import_arm)?
                    && terminal(&request.observation)
                    && request.observation.exit_code == Some(0)
                {
                    return Ok(());
                }
            }
        }
        HelperPermissionV1::Export {
            receipt,
            destination_ref,
            destination_intent_hash,
        } => {
            ensure(purpose == HelperPurposeV1::Export)?;
            for operation in previous {
                if let SupervisionOperationV1::Export {
                    request,
                    response: Some(response),
                } = operation
                    && equal(response, receipt)?
                    && request.worker_id == worker.worker_id
                    && request.attempt_id == worker.attempt.attempt_id
                    && request.destination_ref == *destination_ref
                    && request.destination_intent_hash == *destination_intent_hash
                {
                    return Ok(());
                }
            }
        }
    }
    Err(JournalError::Conflict)
}
fn owned_context(
    owned: &OwnedContainerSnapshot,
    inspection: &RuntimeInspectionV1,
    arm: &ArmV1,
    authority: &crate::runtime::RuntimeAuthority,
    operation: Uuid,
    session: Uuid,
) -> JournalResult<()> {
    hash(owned.handle.id())?;
    let observation = &owned.observation;
    ensure(
        owned.handle.operation_id() == operation
            && owned.handle.session_id() == session
            && owned.handle.authority_fingerprint() == authority.fingerprint()
            && arm.authority_binding_hash == authority.fingerprint()
            && authority.authority_id.to_string() == inspection.authority_id
            && owned.handle.id() == inspection.container_id
            && owned.handle.name() == inspection.stable_name
            && observation.id.as_deref() == Some(owned.handle.id())
            && observation.name == inspection.stable_name
            && observation.image_id.as_deref() == Some(inspection.image_id.as_str())
            && observation.configuration_fingerprint.as_deref()
                == Some(arm.configuration_fingerprint.as_str())
            && observation.configuration_fingerprint_version.unwrap_or(1)
                == arm.configuration_fingerprint_version
            && owned.handle.fingerprint_version() == arm.configuration_fingerprint_version,
    )?;
    checked(observation.validate_identity_allocation(observation))?;
    let cpu = observation
        .allocation
        .cpu
        .as_ref()
        .ok_or(JournalError::Conflict)?;
    ensure(
        cpu.period > 0
            && u128::from(cpu.quota) * 1000
                == u128::from(inspection.cpu_millis) * u128::from(cpu.period)
            && observation.allocation.memory_bytes == Some(inspection.memory_bytes)
            && observation
                .allocation
                .pids_limit
                .is_some_and(|v| u32::try_from(v).ok() == Some(inspection.pids_limit)),
    )
}
fn admission_context(
    request: &SupervisedAdmissionV1,
    intent: &ExecutionIntentV1,
    binding: &BindingManifestV1,
) -> JournalResult<()> {
    checked(request.validate())?;
    normalized_request(&request.admission.decision_request, intent)?;
    let options = intent.initial_options().run_options();
    let launch = &request.admission.launch;
    let kind = match options.runtime {
        crate::runtime::RuntimeKind::Docker => RuntimeKindV1::Docker,
        crate::runtime::RuntimeKind::Podman => RuntimeKindV1::Podman,
    };
    let network = match options.network {
        crate::runtime::NetworkMode::None => NetworkV1::None,
        crate::runtime::NetworkMode::Bridge => NetworkV1::Bridge,
    };
    ensure(
        request.intent_hash == intent.intent_hash().map_err(|_| JournalError::Conflict)?
            && request.admission.operation_id == intent.operation_id().to_string()
            && launch.session_id == intent.session().to_string()
            && launch.command.argv == options.argv
            && launch.command.cwd == "/workspace"
            && launch.profile == options.profile
            && launch.runtime_kind == kind
            && launch.network == network
            && launch.egress.as_ref() == intent.egress()
            && u64::from(launch.timeout_seconds) == options.timeout
            && launch.image_ref == options.image
            && launch.provider == options.provider
            && launch.capture_output == options.capture_output
            && launch.security_preset.is_interactive() == options.interactive
            && launch.credential_binding_id == binding.binding_id().map(|id| id.to_string())
            && launch.credential_names == binding.names(),
    )
}

fn require_admission_decision(
    previous: &[SupervisionOperationV1],
    request: &SupervisedAdmissionV1,
) -> JournalResult<()> {
    let mut found = false;
    for operation in previous {
        if let SupervisionOperationV1::Decision {
            request: decision_request,
            response: Some(answer),
        } = operation
            && equal(decision_request, &request.admission.decision_request)?
            && equal(&answer.receipt, &request.admission.receipt)?
            && answer.decision.outcome == Outcome::Allow
        {
            found = true;
        }
    }
    ensure(found)
}

impl BrokerCheckpointV2 {
    pub(crate) fn validate_context(
        &self,
        intent: &ExecutionIntentV1,
        scope: &JournalScopeV1,
        binding: &BindingManifestV1,
    ) -> JournalResult<()> {
        self.validate_shape()?;
        let namespace = scope.control_namespace_uuid();
        ensure(self.supervision.control_namespace_uuid == namespace)?;
        let context = self.runner.broker_context();
        ensure(!context.legacy_pending && !context.legacy_import)?;
        let named_intent = intent.legacy_named_plan();
        if let Some(plan) = named_intent {
            ensure(
                self.supervision.helpers.is_empty()
                    && self.supervision.operations.iter().all(|operation| {
                        matches!(operation, SupervisionOperationV1::Decision { .. })
                    })
                    && self
                        .supervision
                        .legacy_named_admission()
                        .is_none_or(|named| named.plan() == plan),
            )?;
        }
        if !self.supervision.helpers.is_empty()
            || context.owned.is_some()
            || self
                .supervision
                .operations
                .iter()
                .any(|op| matches!(op, SupervisionOperationV1::Arm { .. }))
        {
            ensure(context.binding.is_some())?;
        }
        let mut requested: Option<&SupervisedAdmissionV1> = None;
        let mut admitted: Option<&SupervisedAdmissionV1> = None;
        let mut acknowledged_revisions = HashSet::new();
        for (index, operation) in self.supervision.operations.iter().enumerate() {
            let previous = &self.supervision.operations[..index];
            match operation {
                SupervisionOperationV1::Decision { request, response } => {
                    normalized_request(request, intent)?;
                    if request.human_approved {
                        ensure(context.request.human_approved)?;
                    }
                    if let Some(response) = response {
                        ensure(
                            response.receipt.request_id == request.request_id
                                && response.receipt.request_hash
                                    == crate::policy::request_hash(request)
                                && response.receipt.phase
                                    == if request.human_approved {
                                        DecisionPhaseV1::Approved
                                    } else {
                                        DecisionPhaseV1::Initial
                                    },
                        )?;
                    }
                }
                SupervisionOperationV1::Admission { request, response } => {
                    admission_context(request, intent, binding)?;
                    ensure(equal(&request.admission.decision_request, context.request)?)?;
                    requested = Some(request);
                    require_admission_decision(previous, request)?;
                    if let Some(response) = response {
                        ensure(
                            response.accepted == response.refusal.is_none()
                                && response.accepted == response.worker.is_some(),
                        )?;
                        bound_receipt(
                            &response.receipt,
                            &request.admission.operation_id,
                            &checked(request.input_hash())?,
                            response.worker.as_ref().map(|w| w.worker_id.as_str()),
                            response.worker.as_ref().map(|w| w.revision),
                        )?;
                        if let Some(worker) = &response.worker {
                            bound_worker(worker, request, namespace)?;
                            ensure(
                                worker.revision == 1
                                    && worker.state == WorkerStateV1::Reserved
                                    && worker.quota_held,
                            )?;
                            admitted = Some(request);
                        }
                    }
                }
                SupervisionOperationV1::WorkerMutation { request, response } => {
                    let admission = admitted.ok_or(JournalError::Conflict)?;
                    if matches!(request.transition, WorkerTransitionV1::CreateIssued) {
                        verified_import(
                            previous,
                            &request.worker_id,
                            &request.attempt_id,
                            &admission.admission.launch.workspace_manifest_digest,
                        )?;
                    }
                    let parent = matching_worker(
                        previous,
                        &request.worker_id,
                        &request.attempt_id,
                        request.expected_revision,
                    )?;
                    let expected =
                        checked(crate::audit::expected_worker_transition(parent, request))?;
                    if let Some(response) = response {
                        ensure(equal(&expected, &response.worker)?)?;
                        bound_worker(&response.worker, admission, namespace)?;
                        ensure(
                            response.worker.worker_id == request.worker_id
                                && response.worker.attempt.attempt_id == request.attempt_id
                                && request.expected_revision.checked_add(1)
                                    == Some(response.worker.revision)
                                && response.disposition.is_none(),
                        )?;
                        bound_receipt(
                            &response.receipt,
                            &request.operation_id,
                            &checked(content_hash("ai-guard:worker-mutation:v1", request))?,
                            Some(&request.worker_id),
                            Some(response.worker.revision),
                        )?;
                    }
                }
                SupervisionOperationV1::Arm {
                    parent,
                    owned,
                    request,
                    response,
                } => {
                    let admission = admitted.ok_or(JournalError::Conflict)?;
                    bound_worker(parent, admission, namespace)?;
                    known_worker(previous, parent)?;
                    checked(request.validate_for_parent(parent))?;
                    ensure(request.intent_hash == admission.intent_hash)?;
                    let expected_operation = match &request.subject {
                        ArmSubjectV1::Worker => intent.operation_id(),
                        ArmSubjectV1::Helper {
                            helper_id,
                            purpose,
                            permission: given,
                            ..
                        } => {
                            let helper = self
                                .supervision
                                .helpers
                                .iter()
                                .find(|helper| helper.policy.helper_id == *helper_id)
                                .ok_or(JournalError::Conflict)?;
                            ensure(
                                equal(&helper.owned, &Some(owned.clone()))?
                                    && equal(&helper.permission, given.as_ref())?,
                            )?;
                            checked(helper.policy.validate_inspection(&request.inspection))?;
                            permission(previous, given, *purpose, parent)?;
                            uuid(helper_id)?
                        }
                    };
                    owned_context(
                        owned,
                        &request.inspection,
                        request,
                        context.binding.ok_or(JournalError::Conflict)?,
                        expected_operation,
                        intent.session(),
                    )?;
                    if let Some(response) = response {
                        ensure(response.receipt.control_namespace_uuid == namespace.to_string())?;
                    }
                }
                SupervisionOperationV1::WorkerClaim { request, response } => {
                    let admission = admitted.ok_or(JournalError::Conflict)?;
                    verified_import(
                        previous,
                        &request.worker_id,
                        &request.attempt_id,
                        &admission.admission.launch.workspace_manifest_digest,
                    )?;
                    let arm = find_arm(previous, &request.arm_receipt)?;
                    checked(request.validate_for_arm(arm))?;
                    let parent = matching_worker(
                        previous,
                        &request.worker_id,
                        &request.attempt_id,
                        request.expected_revision,
                    )?;
                    let mutation = WorkerMutationV1 {
                        version: 1,
                        operation_id: request.operation_id.clone(),
                        worker_id: request.worker_id.clone(),
                        attempt_id: request.attempt_id.clone(),
                        expected_revision: request.expected_revision,
                        transition: WorkerTransitionV1::StartClaim {
                            emitter_id: request.emitter_id.clone(),
                        },
                    };
                    let expected =
                        checked(crate::audit::expected_worker_transition(parent, &mutation))?;
                    if let Some(response) = response {
                        ensure(equal(&expected, &response.worker)?)?;
                        bound_worker(&response.worker, admission, namespace)?;
                        ensure(
                            response.worker.worker_id == request.worker_id
                                && response.worker.attempt.attempt_id == request.attempt_id
                                && request.expected_revision.checked_add(1)
                                    == Some(response.worker.revision)
                                && response.disposition.is_some()
                                && response.worker.attempt.start_operation_id.as_deref()
                                    == Some(request.operation_id.as_str())
                                && response.worker.attempt.start_emitter_id.as_deref()
                                    == Some(request.emitter_id.as_str()),
                        )?;
                        bound_receipt(
                            &response.receipt,
                            &request.operation_id,
                            &checked(request.input_hash())?,
                            Some(&request.worker_id),
                            Some(response.worker.revision),
                        )?;
                    }
                }
                SupervisionOperationV1::HelperClaim { request, response } => {
                    let arm = find_arm(previous, &request.arm_receipt)?;
                    checked(request.validate_for_arm(arm))?;
                    if let Some(response) = response {
                        bound_receipt(
                            &response.receipt,
                            &request.operation_id,
                            &checked(request.input_hash())?,
                            Some(&arm.worker_id),
                            None,
                        )?;
                    }
                }
                SupervisionOperationV1::HelperObservation { request, response } => {
                    let arm = find_arm(previous, &request.arm_receipt)?;
                    has_helper_claim(previous, &request.helper_id, &request.arm_receipt)?;
                    ensure(
                        matches!(&arm.subject, ArmSubjectV1::Helper { helper_id, .. } if helper_id == &request.helper_id)
                            && equal(&request.observation.inspection, &arm.inspection)?,
                    )?;
                    for operation in previous {
                        if let SupervisionOperationV1::HelperObservation {
                            request: old,
                            response: Some(_),
                        } = operation
                            && old.helper_id == request.helper_id
                            && terminal(&old.observation)
                        {
                            let mut expected = old.observation.clone();
                            expected.observed_at = request.observation.observed_at.clone();
                            ensure(
                                equal(&expected, &request.observation)?
                                    && (old.verification.is_none()
                                        || equal(&old.verification, &request.verification)?),
                            )?;
                        }
                    }
                    if let Some(report) = &request.verification {
                        let ArmSubjectV1::Helper {
                            purpose: HelperPurposeV1::ImportVerification,
                            permission: given,
                            ..
                        } = &arm.subject
                        else {
                            return Err(JournalError::Conflict);
                        };
                        let HelperPermissionV1::ImportVerification {
                            import_helper_id,
                            manifest_digest,
                            ..
                        } = given.as_ref()
                        else {
                            return Err(JournalError::Conflict);
                        };
                        ensure(
                            report.import_helper_id == *import_helper_id
                                && report.expected_manifest_digest == *manifest_digest
                                && terminal(&request.observation),
                        )?;
                    }
                    if let Some(response) = response {
                        bound_receipt(
                            response,
                            &request.operation_id,
                            &checked(request.input_hash())?,
                            Some(&arm.worker_id),
                            None,
                        )?;
                    }
                }
                SupervisionOperationV1::HelperCleanup { request, response } => {
                    let arm = find_arm(previous, &request.arm_receipt)?;
                    has_helper_claim(previous, &request.helper_id, &request.arm_receipt)?;
                    ensure(
                        matches!(&arm.subject, ArmSubjectV1::Helper { helper_id, .. } if helper_id == &request.helper_id),
                    )?;
                    let mut found = false;
                    for operation in previous {
                        if let SupervisionOperationV1::HelperCleanup {
                            request: old,
                            response: Some(_),
                        } = operation
                            && old.helper_id == request.helper_id
                            && old.removed
                        {
                            return Err(JournalError::Conflict);
                        }
                        if let SupervisionOperationV1::HelperObservation {
                            request: observed,
                            response: Some(_),
                            ..
                        } = operation
                            && observed.helper_id == request.helper_id
                            && equal(&observed.arm_receipt, &request.arm_receipt)?
                            && terminal(&observed.observation)
                        {
                            found = true;
                        }
                    }
                    ensure(found && (request.removed || request.error.is_some()))?;
                    if let Some(response) = response {
                        bound_receipt(
                            response,
                            &request.operation_id,
                            &checked(request.input_hash())?,
                            Some(&arm.worker_id),
                            None,
                        )?;
                    }
                }
                SupervisionOperationV1::Export { request, response } => {
                    let admission = admitted.ok_or(JournalError::Conflict)?;
                    let mut found = false;
                    for operation in previous {
                        if let SupervisionOperationV1::WorkerMutation {
                            response: Some(response),
                            ..
                        } = operation
                        {
                            let worker = &response.worker;
                            if worker.worker_id == request.worker_id
                                && worker.attempt.attempt_id == request.attempt_id
                                && worker.state == WorkerStateV1::Terminal
                                && !worker.quota_held
                                && !worker.allocation_inconsistent
                            {
                                found = true;
                            }
                        }
                    }
                    ensure(
                        found
                            && admission.admission.operation_id
                                == intent.operation_id().to_string(),
                    )?;
                    if let Some(response) = response {
                        bound_receipt(
                            response,
                            &request.operation_id,
                            &checked(request.input_hash())?,
                            Some(&request.worker_id),
                            None,
                        )?;
                    }
                }
            }
            let acknowledged = match operation {
                SupervisionOperationV1::Admission {
                    response: Some(result),
                    ..
                } => result.worker.as_ref(),
                SupervisionOperationV1::WorkerMutation {
                    response: Some(result),
                    ..
                }
                | SupervisionOperationV1::WorkerClaim {
                    response: Some(result),
                    ..
                } => Some(&result.worker),
                _ => None,
            };
            if let Some(worker) = acknowledged {
                ensure(acknowledged_revisions.insert((
                    worker.worker_id.as_str(),
                    worker.attempt.attempt_id.as_str(),
                    worker.revision,
                )))?;
            }
        }
        if let Some(named) = self.supervision.legacy_named_admission() {
            let request = &named.request().admission;
            ensure(
                named.archive_digest().is_some() == named_intent.is_some()
                    && named.plan().broker_profile_id == scope.profile_id()
                    && equal(&request.admission.decision_request, context.request)?
                    && context.owned.is_none(),
            )?;
            admission_context(request, intent, binding)?;
            require_admission_decision(&self.supervision.operations, request)?;
            if let Some(saved) = context.admission {
                ensure(equal(saved, &request.admission)?)?;
            }
            if let Some(worker) = context.worker {
                let admitted = named
                    .admission_result()
                    .and_then(|result| result.worker.as_ref())
                    .ok_or(JournalError::Conflict)?;
                ensure(equal(worker, admitted)?)?;
            }
            return Ok(());
        }
        if let Some(saved) = context.admission {
            let admission = requested.ok_or(JournalError::Conflict)?;
            ensure(equal(saved, &admission.admission)?)?;
        }
        if let Some(worker) = context.worker {
            bound_worker(worker, admitted.ok_or(JournalError::Conflict)?, namespace)?;
            known_worker(&self.supervision.operations, worker)?;
        }
        for helper in &self.supervision.helpers {
            let admission = admitted.ok_or(JournalError::Conflict)?;
            let parent = self
                .supervision
                .operations
                .iter()
                .find_map(|op| {
                    if let SupervisionOperationV1::Admission {
                        response: Some(result),
                        ..
                    } = op
                    {
                        result.worker.as_ref()
                    } else {
                        None
                    }
                })
                .ok_or(JournalError::Conflict)?;
            let expected = checked(helper_execution_policy_v1(
                parent,
                helper.policy.purpose,
                uuid(&helper.policy.helper_id)?,
            ))?;
            ensure(
                equal(&expected, &helper.policy)?
                    && helper.policy.parent_worker_id == parent.worker_id
                    && admission.admission.operation_id == parent.admission_operation_id,
            )?;
            permission(
                &self.supervision.operations,
                &helper.permission,
                helper.policy.purpose,
                parent,
            )?;
            if let Some(owned) = &helper.owned {
                ensure(
                    owned.handle.session_id() == intent.session()
                        && owned.handle.operation_id().to_string() == helper.policy.helper_id
                        && owned.handle.name() == helper.policy.stable_name
                        && owned.handle.authority_fingerprint()
                            == context.binding.ok_or(JournalError::Conflict)?.fingerprint()
                        && owned.observation.id.as_deref() == Some(owned.handle.id())
                        && owned.observation.name == helper.policy.stable_name,
                )?;
                hash(owned.handle.id())?;
                ensure(
                    owned.observation.image_id.as_deref() == Some(helper.policy.image_id.as_str())
                        && owned.handle.fingerprint_version()
                            == helper
                                .policy
                                .security_preset
                                .configuration_fingerprint_version()
                        && owned
                            .observation
                            .configuration_fingerprint_version
                            .unwrap_or(1)
                            == owned.handle.fingerprint_version()
                        && owned.observation.allocation.memory_bytes
                            == Some(helper.policy.memory_bytes)
                        && owned.observation.allocation.pids_limit
                            == Some(u64::from(helper.policy.pids_limit)),
                )?;
                hash(
                    owned
                        .observation
                        .configuration_fingerprint
                        .as_deref()
                        .ok_or(JournalError::Conflict)?,
                )?;
                let cpu = owned
                    .observation
                    .allocation
                    .cpu
                    .as_ref()
                    .ok_or(JournalError::Conflict)?;
                ensure(
                    cpu.period > 0
                        && u128::from(cpu.quota) * 1000
                            == u128::from(helper.policy.cpu_millis) * u128::from(cpu.period),
                )?;
                checked(
                    owned
                        .observation
                        .validate_identity_allocation(&owned.observation),
                )?;
            }
        }
        Ok(())
    }
}
