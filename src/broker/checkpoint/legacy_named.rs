//! Private named admission facts retained in the broker checkpoint.
use super::*;
use crate::{
    control::session_control::{
        LegacyConfirmedPlanV1, LegacyNamedAdmissionReceiptV1, LegacyNamedImportArmRequestV1,
        LegacyNamedImportClaimRequestV1, LegacyNamedImportObservationPhaseV1,
        LegacyNamedImportObservationRequestV1, LegacyNamedSupervisedAdmissionV1,
    },
    runtime::{RuntimeAuthority, RuntimeKind},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyNamedAdmissionCheckpoint {
    version: u32,
    plan: LegacyConfirmedPlanV1,
    request: LegacyNamedSupervisedAdmissionV1,
    receipt: Option<LegacyNamedAdmissionReceiptV1>,
    #[serde(
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    archive_digest: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    import_arm: Option<Box<LegacyNamedImportArmCheckpointV1>>,
    #[serde(
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    import_creation: Option<Box<LegacyNamedImportCreationCheckpoint>>,
    #[serde(
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    import_claim: Option<Box<LegacyNamedImportClaimCheckpointV1>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    import_observations: Vec<LegacyNamedImportObservationCheckpointV1>,
    #[serde(
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    import_verification: Option<Box<LegacyNamedImportVerificationPlanV1>>,
    #[serde(
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    import_verification_create: Option<Box<LegacyNamedImportVerificationCreateV1>>,
}

fn deserialize_present<'de, D, T>(decoder: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(decoder).map(Some)
}

/// The retained control request and receipt. Neither proves that any runtime
/// effect is known or gives permission to start the helper.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyNamedImportArmCheckpointV1 {
    version: u32,
    arm: ArmV1,
    receipt: Option<ArmReceiptV1>,
}

/// Created/Arm facts retained by the named checkpoint. This is evidence only:
/// it carries no RuntimeAuthority and cannot be used to start or inspect a
/// runtime.
pub(crate) struct LegacyNamedCreatedImportEvidenceV1 {
    owned: OwnedContainerSnapshot,
    arm: ArmV1,
    receipt: Option<ArmReceiptV1>,
    handle_fingerprint: String,
    runtime_binding_hash: String,
}

impl LegacyNamedCreatedImportEvidenceV1 {
    pub(crate) fn owned(&self) -> &OwnedContainerSnapshot {
        &self.owned
    }
    pub(crate) fn arm(&self) -> &ArmV1 {
        &self.arm
    }
    pub(crate) fn receipt(&self) -> Option<&ArmReceiptV1> {
        self.receipt.as_ref()
    }
    pub(crate) fn handle_fingerprint(&self) -> &str {
        &self.handle_fingerprint
    }
    pub(crate) fn runtime_binding_hash(&self) -> &str {
        &self.runtime_binding_hash
    }
}

impl LegacyNamedImportArmCheckpointV1 {
    pub(crate) fn receipt(&self) -> Option<&ArmReceiptV1> {
        self.receipt.as_ref()
    }
}

/// Retained Core request and receipt. Native binding and start permission
/// belong to the live owner and have no representation in this record.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyNamedImportClaimCheckpointV1 {
    version: u32,
    request: LegacyNamedImportClaimRequestV1,
    receipt: Option<MutationReceiptV1>,
}

impl LegacyNamedImportClaimCheckpointV1 {
    pub(crate) fn request(&self) -> &LegacyNamedImportClaimRequestV1 {
        &self.request
    }

    pub(crate) fn receipt(&self) -> Option<&MutationReceiptV1> {
        self.receipt.as_ref()
    }
}

/// One exact, append-only runtime fact for the already-claimed Import helper.
/// An ACK records the Store receipt; this checkpoint never grants start.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyNamedImportObservationCheckpointV1 {
    version: u32,
    request: LegacyNamedImportObservationRequestV1,
    receipt: Option<MutationReceiptV1>,
}

/// Durable intent for a distinct ImportVerification helper. It contains no
/// owned handle or runtime authority; the successful named Terminal receipt
/// and immutable helper policy are the only inputs to a later effect.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyNamedImportVerificationPlanV1 {
    version: u32,
    preparation: HelperPreparationV1,
    parent_revision: u64,
}

impl LegacyNamedImportVerificationPlanV1 {
    pub(crate) fn preparation(&self) -> &HelperPreparationV1 {
        &self.preparation
    }
    pub(crate) fn parent_revision(&self) -> u64 {
        self.parent_revision
    }
}

/// One durable Create attempt for the exact named ImportVerification plan.
/// Its presence consumes the only Create opportunity; it contains no runtime
/// handle and cannot be replayed into another effect.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyNamedImportVerificationCreateV1 {
    version: u32,
    plan_hash: String,
    spec_hash: String,
    terminal_receipt: MutationReceiptV1,
    authority_id: String,
    runtime_kind: RuntimeKindV1,
    authority: LegacyNamedImportAuthorityV1,
}

impl LegacyNamedImportObservationCheckpointV1 {
    pub(crate) fn request(&self) -> &LegacyNamedImportObservationRequestV1 {
        &self.request
    }

    pub(crate) fn receipt(&self) -> Option<&MutationReceiptV1> {
        self.receipt.as_ref()
    }
}

/// The first local create boundary for the exact import helper. Its presence
/// records that creation may be issued, before any runtime observation exists.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyNamedImportCreationCheckpoint {
    version: u32,
    preparation: HelperPreparationV1,
    #[serde(
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    authority: Option<LegacyNamedImportAuthorityV1>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyNamedImportAuthorityV1 {
    handle_fingerprint: String,
    binding_hash: String,
}

fn import_verification_spec_hash(policy: &HelperExecutionPolicyV1) -> JournalResult<String> {
    ensure(policy.purpose == HelperPurposeV1::ImportVerification)?;
    checked(content_hash(
        "ai-guard:legacy-named-import-verification-readonly-spec:v1",
        &(
            policy.spec_hash().map_err(|_| JournalError::Conflict)?,
            true,
        ),
    ))
}

impl LegacyNamedImportAuthorityV1 {
    fn for_policy(
        policy: &HelperExecutionPolicyV1,
        binding: &RuntimeAuthority,
    ) -> JournalResult<Self> {
        let kind = match binding.kind {
            RuntimeKind::Docker => RuntimeKindV1::Docker,
            RuntimeKind::Podman => RuntimeKindV1::Podman,
        };
        ensure(
            policy.authority_id == binding.authority_id.to_string()
                && policy.runtime_kind == kind
                && binding.invocation.is_some(),
        )?;
        Ok(Self {
            handle_fingerprint: binding.fingerprint(),
            binding_hash: checked(content_hash(
                "ai-guard:legacy-named-import-runtime-binding:v1",
                binding,
            ))?,
        })
    }
}

/// The exact request and archive pinned by a V2 pending checkpoint.
pub(crate) struct LegacyNamedAdmissionPendingV2<'a> {
    pub(crate) plan: &'a LegacyConfirmedPlanV1,
    pub(crate) request: &'a LegacyNamedSupervisedAdmissionV1,
    pub(crate) archive_digest: &'a str,
}

impl LegacyNamedAdmissionCheckpoint {
    #[cfg(test)]
    pub(crate) fn new(
        plan: LegacyConfirmedPlanV1,
        request: LegacyNamedSupervisedAdmissionV1,
    ) -> JournalResult<Self> {
        Self::pending(1, plan, request, None)
    }

    #[allow(
        dead_code,
        reason = "the private named transport will persist the validated archive digest"
    )]
    pub(crate) fn pending_with_archive(
        plan: LegacyConfirmedPlanV1,
        request: LegacyNamedSupervisedAdmissionV1,
        archive_digest: String,
    ) -> JournalResult<Self> {
        Self::pending(2, plan, request, Some(archive_digest))
    }

    fn pending(
        version: u32,
        plan: LegacyConfirmedPlanV1,
        request: LegacyNamedSupervisedAdmissionV1,
        archive_digest: Option<String>,
    ) -> JournalResult<Self> {
        let value = Self {
            version,
            plan,
            request,
            receipt: None,
            archive_digest,
            import_arm: None,
            import_creation: None,
            import_claim: None,
            import_observations: Vec::new(),
            import_verification: None,
            import_verification_create: None,
        };
        let namespace = uuid(&value.plan.control_namespace_uuid)?;
        value.validate_shape(namespace)?;
        Ok(value)
    }

    /// Retain the authenticated Store reply at the existing pending operation.
    /// Reconciliation can return the same receipt with different delivery metadata.
    #[allow(
        dead_code,
        reason = "the private named transport will supply the Store acknowledgement"
    )]
    pub(crate) fn acknowledge(
        mut self,
        mut receipt: LegacyNamedAdmissionReceiptV1,
    ) -> JournalResult<Self> {
        if let Some(previous) = &self.receipt {
            receipt.admission.replayed = previous.admission.replayed;
            ensure(equal(previous, &receipt)?)?;
        } else {
            self.receipt = Some(receipt);
        }
        let namespace = uuid(&self.plan.control_namespace_uuid)?;
        self.validate_shape(namespace)?;
        Ok(self)
    }

    pub(crate) fn plan(&self) -> &LegacyConfirmedPlanV1 {
        &self.plan
    }

    pub(crate) fn request(&self) -> &LegacyNamedSupervisedAdmissionV1 {
        &self.request
    }

    pub(crate) fn admission_result(&self) -> Option<&AdmissionResultV1> {
        self.receipt().map(|receipt| &receipt.admission)
    }

    pub(crate) fn receipt(&self) -> Option<&LegacyNamedAdmissionReceiptV1> {
        self.receipt.as_ref()
    }

    pub(crate) fn archive_digest(&self) -> Option<&str> {
        self.archive_digest.as_deref()
    }

    pub(crate) fn import_creation(&self) -> Option<&HelperPreparationV1> {
        self.import_creation
            .as_deref()
            .map(|saved| &saved.preparation)
    }

    pub(crate) fn require_import_creation_binding(
        &self,
        binding: &RuntimeAuthority,
    ) -> JournalResult<()> {
        let creation = self
            .import_creation
            .as_deref()
            .ok_or(JournalError::Conflict)?;
        let expected =
            LegacyNamedImportAuthorityV1::for_policy(creation.preparation.policy(), binding)?;
        ensure(creation.authority.as_ref() == Some(&expected))
    }

    #[allow(dead_code, reason = "the named create caller is composed separately")]
    pub(crate) fn with_import_creation(
        mut self,
        helper_id: Uuid,
        binding: &RuntimeAuthority,
    ) -> JournalResult<Self> {
        ensure(self.import_creation.is_none() && self.import_arm.is_none())?;
        let preparation = self.import_preparation(helper_id)?;
        self.import_creation = Some(Box::new(LegacyNamedImportCreationCheckpoint {
            version: 2,
            authority: Some(LegacyNamedImportAuthorityV1::for_policy(
                preparation.policy(),
                binding,
            )?),
            preparation,
        }));
        self.validate_shape(uuid(&self.plan.control_namespace_uuid)?)?;
        Ok(self)
    }

    #[allow(dead_code, reason = "the named create caller is composed separately")]
    pub(crate) fn with_import_owned(
        mut self,
        binding: &RuntimeAuthority,
        owned: OwnedContainerSnapshot,
    ) -> JournalResult<Self> {
        self.require_import_creation_binding(binding)?;
        let creation = self
            .import_creation
            .as_mut()
            .ok_or(JournalError::Conflict)?;
        let preparation = &creation.preparation;
        if let Some(previous) = preparation.owned() {
            // A new inspection captures its own wall time. Preserve the
            // first observation and reject every difference in its facts.
            let mut repeated = owned;
            repeated.observation.observed_at = previous.observation.observed_at;
            ensure(equal(previous, &repeated)?)?;
            return Ok(self);
        }
        ensure(self.import_arm.is_none() && owned.observation.process.finished_at.is_none())?;
        creation.preparation = HelperPreparationV1::new(
            preparation.policy().clone(),
            preparation.permission().clone(),
            Some(owned),
        )?;
        self.validate_shape(uuid(&self.plan.control_namespace_uuid)?)?;
        Ok(self)
    }

    fn import_preparation(&self, helper_id: Uuid) -> JournalResult<HelperPreparationV1> {
        let admission = self.admission_result().ok_or(JournalError::Conflict)?;
        let parent = admission.worker.as_ref().ok_or(JournalError::Conflict)?;
        let policy = checked(helper_execution_policy_v1(
            parent,
            HelperPurposeV1::Import,
            helper_id,
        ))?;
        HelperPreparationV1::new(
            policy,
            HelperPermissionV1::Admission {
                receipt: admission.receipt.clone(),
            },
            None,
        )
    }

    pub(crate) fn import_arm(&self) -> Option<&LegacyNamedImportArmCheckpointV1> {
        self.import_arm.as_deref()
    }

    /// Return a complete, already-validated Created/Arm fact set. Incomplete
    /// historical records deliberately stay unavailable to STOP projection.
    pub(crate) fn created_import_evidence(&self) -> Option<LegacyNamedCreatedImportEvidenceV1> {
        let creation = self.import_creation.as_deref()?;
        let authority = creation.authority.as_ref()?;
        let owned = creation.preparation.owned()?.clone();
        let saved_arm = self.import_arm.as_deref()?;
        if creation.version != 2
            || saved_arm.version != 1
            || owned.handle.id() != saved_arm.arm.inspection.container_id
            || owned.handle.authority_fingerprint() != saved_arm.arm.authority_binding_hash
        {
            return None;
        }
        Some(LegacyNamedCreatedImportEvidenceV1 {
            owned,
            arm: saved_arm.arm.clone(),
            receipt: saved_arm.receipt().cloned(),
            handle_fingerprint: authority.handle_fingerprint.clone(),
            runtime_binding_hash: authority.binding_hash.clone(),
        })
    }

    pub(crate) fn import_arm_request(&self) -> Option<LegacyNamedImportArmRequestV1> {
        self.import_arm()
            .map(|record| LegacyNamedImportArmRequestV1 {
                version: 1,
                binding_ref: self.plan.binding_ref,
                namespace: self.plan.control_namespace_uuid.clone(),
                arm: record.arm.clone(),
            })
    }

    pub(crate) fn with_import_arm(mut self, arm: ArmV1) -> JournalResult<Self> {
        ensure(self.import_arm.is_none())?;
        self.import_arm = Some(Box::new(LegacyNamedImportArmCheckpointV1 {
            version: 1,
            arm,
            receipt: None,
        }));
        self.validate_shape(uuid(&self.plan.control_namespace_uuid)?)?;
        Ok(self)
    }

    pub(crate) fn acknowledge_import_arm(mut self, result: ArmResultV1) -> JournalResult<Self> {
        let saved = self.import_arm.as_mut().ok_or(JournalError::Conflict)?;
        ensure(equal(&saved.arm, &result.arm)?)?;
        if let Some(previous) = saved.receipt() {
            ensure(equal(previous, &result.receipt)?)?;
        } else {
            saved.receipt = Some(result.receipt);
        }
        self.validate_shape(uuid(&self.plan.control_namespace_uuid)?)?;
        Ok(self)
    }

    pub(crate) fn import_claim(&self) -> Option<&LegacyNamedImportClaimCheckpointV1> {
        self.import_claim.as_deref()
    }

    #[allow(
        dead_code,
        reason = "named claim transport and native binding are composed separately"
    )]
    pub(crate) fn with_import_claim(
        mut self,
        request: LegacyNamedImportClaimRequestV1,
    ) -> JournalResult<Self> {
        ensure(self.import_claim.is_none())?;
        self.import_claim = Some(Box::new(LegacyNamedImportClaimCheckpointV1 {
            version: 1,
            request,
            receipt: None,
        }));
        self.validate_shape(uuid(&self.plan.control_namespace_uuid)?)?;
        Ok(self)
    }

    #[allow(dead_code, reason = "named claim transport is composed separately")]
    pub(crate) fn acknowledge_import_claim(
        mut self,
        receipt: MutationReceiptV1,
    ) -> JournalResult<Self> {
        let saved = self.import_claim.as_mut().ok_or(JournalError::Conflict)?;
        if let Some(previous) = saved.receipt() {
            ensure(equal(previous, &receipt)?)?;
        } else {
            saved.receipt = Some(receipt);
        }
        self.validate_shape(uuid(&self.plan.control_namespace_uuid)?)?;
        Ok(self)
    }

    pub(crate) fn import_observation(
        &self,
        phase: LegacyNamedImportObservationPhaseV1,
    ) -> Option<(
        &LegacyNamedImportObservationRequestV1,
        Option<&MutationReceiptV1>,
    )> {
        self.import_observations
            .iter()
            .find(|saved| saved.request.phase == phase)
            .map(|saved| (saved.request(), saved.receipt()))
    }

    pub(crate) fn import_verification_plan(&self) -> Option<&LegacyNamedImportVerificationPlanV1> {
        self.import_verification.as_deref()
    }

    pub(crate) fn import_verification_create(
        &self,
    ) -> Option<&LegacyNamedImportVerificationCreateV1> {
        self.import_verification_create.as_deref()
    }

    /// Add the one immutable verification-helper intent after the exact
    /// successful named Import Terminal receipt is retained. Replaying the
    /// same helper id is idempotent; a different id conflicts.
    pub(crate) fn with_import_verification_plan(mut self, helper_id: Uuid) -> JournalResult<Self> {
        let namespace = uuid(&self.plan.control_namespace_uuid)?;
        self.validate_shape(namespace)?;
        if let Some(saved) = self.import_verification_plan() {
            ensure(saved.preparation().policy().helper_id == helper_id.to_string())?;
            return Ok(self);
        }
        let (parent, permission) = self.import_verification_inputs()?;
        let policy = checked(helper_execution_policy_v1(
            &parent,
            HelperPurposeV1::ImportVerification,
            helper_id,
        ))?;
        let preparation = HelperPreparationV1::new(policy, permission, None)?;
        self.import_verification = Some(Box::new(LegacyNamedImportVerificationPlanV1 {
            version: 1,
            preparation,
            parent_revision: parent.revision,
        }));
        self.validate_shape(namespace)?;
        Ok(self)
    }

    /// Append a one-time Create intent for the read-only named verification
    /// runtime. The caller supplies the exact terminal receipt and admitted
    /// authority; replay can confirm this record but never reserve another
    /// Create.
    pub(crate) fn with_import_verification_create(
        mut self,
        terminal_receipt: MutationReceiptV1,
        binding: &RuntimeAuthority,
    ) -> JournalResult<Self> {
        let namespace = uuid(&self.plan.control_namespace_uuid)?;
        self.validate_shape(namespace)?;
        let expected = self.expected_import_verification_create(terminal_receipt, binding)?;
        if let Some(saved) = self.import_verification_create() {
            ensure(equal(saved, &expected)?)?;
            return Ok(self);
        }
        self.import_verification_create = Some(Box::new(expected));
        self.validate_shape(namespace)?;
        Ok(self)
    }

    fn expected_import_verification_create(
        &self,
        terminal_receipt: MutationReceiptV1,
        binding: &RuntimeAuthority,
    ) -> JournalResult<LegacyNamedImportVerificationCreateV1> {
        let plan = self
            .import_verification_plan()
            .ok_or(JournalError::Conflict)?;
        let terminal = match plan.preparation().permission() {
            HelperPermissionV1::ImportVerification {
                import_terminal, ..
            } => import_terminal,
            _ => return Err(JournalError::Conflict),
        };
        ensure(equal(terminal, &terminal_receipt)?)?;
        ensure(equal(
            &self.import_verification_permission()?,
            plan.preparation().permission(),
        )?)?;
        let policy = plan.preparation().policy();
        ensure(policy.purpose == HelperPurposeV1::ImportVerification)?;
        Ok(LegacyNamedImportVerificationCreateV1 {
            version: 1,
            plan_hash: checked(content_hash(
                "ai-guard:legacy-named-import-verification-plan:v1",
                plan,
            ))?,
            spec_hash: import_verification_spec_hash(policy)?,
            terminal_receipt,
            authority_id: policy.authority_id.clone(),
            runtime_kind: policy.runtime_kind,
            authority: LegacyNamedImportAuthorityV1::for_policy(policy, binding)?,
        })
    }

    fn import_verification_inputs(&self) -> JournalResult<(WorkerV1, HelperPermissionV1)> {
        let reserved = self
            .admission_result()
            .filter(|result| result.accepted)
            .and_then(|result| result.worker.as_ref())
            .ok_or(JournalError::Conflict)?;
        let import_arm = self
            .import_arm()
            .filter(|arm| arm.receipt().is_some())
            .ok_or(JournalError::Conflict)?;
        let prepare = WorkerMutationV1 {
            version: 1,
            operation_id: import_arm.arm.operation_id.clone(),
            worker_id: reserved.worker_id.clone(),
            attempt_id: reserved.attempt.attempt_id.clone(),
            expected_revision: reserved.revision,
            transition: WorkerTransitionV1::Prepare,
        };
        let parent = checked(crate::audit::expected_worker_transition(reserved, &prepare))?;
        checked(import_arm.arm.validate_for_parent(&parent))?;
        Ok((parent, self.import_verification_permission()?))
    }

    /// Build the evidence for a separate verification Arm from the exact
    /// acknowledged Import result. Store still checks the current parent.
    #[allow(dead_code, reason = "the separate verification Arm is composed next")]
    pub(crate) fn import_verification_permission(&self) -> JournalResult<HelperPermissionV1> {
        let (request, Some(receipt)) = self
            .import_observation(LegacyNamedImportObservationPhaseV1::Terminal)
            .ok_or(JournalError::Conflict)?
        else {
            return Err(JournalError::Conflict);
        };
        self.validate_import_observation_request(request)?;
        checked(request.validate_receipt(receipt))?;
        ensure(request.observation.observation.exit_code == Some(0))?;
        Ok(HelperPermissionV1::ImportVerification {
            import_helper_id: request.observation.helper_id.clone(),
            import_arm: request.observation.arm_receipt.clone(),
            import_terminal: receipt.clone(),
            manifest_digest: self
                .request
                .admission
                .admission
                .launch
                .workspace_manifest_digest
                .clone(),
        })
    }

    /// Persist the exact observation payload before it is submitted to Store.
    /// Reconciliation reuses this identity; it cannot manufacture an Arm or
    /// a launch permission.
    pub(crate) fn with_import_observation(
        mut self,
        request: LegacyNamedImportObservationRequestV1,
    ) -> JournalResult<Self> {
        self.validate_import_observation_request(&request)?;
        if let Some((saved, _)) = self.import_observation(request.phase) {
            ensure(equal(saved, &request)?)?;
            return Ok(self);
        }
        if request.phase == LegacyNamedImportObservationPhaseV1::Terminal
            && self
                .import_observation(LegacyNamedImportObservationPhaseV1::Started)
                .is_some_and(|(_, receipt)| receipt.is_none())
        {
            return Err(JournalError::Conflict);
        }
        self.import_observations
            .push(LegacyNamedImportObservationCheckpointV1 {
                version: 1,
                request,
                receipt: None,
            });
        self.validate_shape(uuid(&self.plan.control_namespace_uuid)?)?;
        Ok(self)
    }

    /// Retain the Store ACK against the previously persisted exact payload.
    /// A concurrent STOP is preserved by the Writer's lifecycle CAS retry.
    pub(crate) fn acknowledge_import_observation(
        mut self,
        request: &LegacyNamedImportObservationRequestV1,
        receipt: MutationReceiptV1,
    ) -> JournalResult<Self> {
        checked(request.validate_receipt(&receipt))?;
        let saved = self
            .import_observations
            .iter_mut()
            .find(|saved| saved.request.phase == request.phase)
            .ok_or(JournalError::Conflict)?;
        ensure(equal(saved.request(), request)?)?;
        if let Some(previous) = saved.receipt() {
            ensure(equal(previous, &receipt)?)?;
        } else {
            saved.receipt = Some(receipt);
        }
        self.validate_shape(uuid(&self.plan.control_namespace_uuid)?)?;
        Ok(self)
    }

    pub(crate) fn requires_strict_cas(&self) -> bool {
        self.pending_v2().is_some()
            || (self.import_creation().is_some() && self.import_arm().is_none())
            || self.import_arm().is_some_and(|arm| arm.receipt().is_none())
            || self
                .import_claim()
                .is_some_and(|claim| claim.receipt().is_none())
            || self
                .import_observations
                .iter()
                .any(|observation| observation.receipt().is_none())
            || self.import_verification_plan().is_some()
            || self.import_verification_create().is_some()
    }

    #[allow(
        dead_code,
        reason = "the broker orchestrator will dispatch the exact committed pending"
    )]
    pub(crate) fn pending_v2(&self) -> Option<LegacyNamedAdmissionPendingV2<'_>> {
        if self.version != 2 || self.receipt.is_some() {
            return None;
        }
        let namespace = uuid(&self.plan.control_namespace_uuid).ok()?;
        self.validate_shape(namespace).ok()?;
        Some(LegacyNamedAdmissionPendingV2 {
            plan: &self.plan,
            request: &self.request,
            archive_digest: self.archive_digest()?,
        })
    }

    pub(super) fn validate_shape(&self, namespace: Uuid) -> JournalResult<()> {
        match self.version {
            1 => ensure(
                self.archive_digest.is_none()
                    && self.import_arm.is_none()
                    && self.import_creation.is_none()
                    && self.import_claim.is_none()
                    && self.import_observations.is_empty()
                    && self.import_verification.is_none()
                    && self.import_verification_create.is_none(),
            )?,
            2 => hash(self.archive_digest().ok_or(JournalError::Conflict)?)?,
            _ => return Err(JournalError::UnsupportedVersion),
        }
        let plan = &self.plan;
        ensure(
            uuid(&plan.control_namespace_uuid)? == namespace
                && !plan.launch_target_id.is_nil()
                && plan.target_revision > 0
                && plan.expires_at.timestamp() > 0
                && self.request.admission.admission.receipt.phase == DecisionPhaseV1::Initial,
        )?;
        checked(self.request.require_plan(plan))?;
        if let Some(saved) = &self.receipt {
            validate_legacy_named_admission_receipt(namespace, &self.request, saved)?;
        }
        if let Some(creation) = &self.import_creation {
            let preparation = &creation.preparation;
            match creation.version {
                1 => ensure(creation.authority.is_none() && preparation.owned().is_none())?,
                2 => {
                    let authority = creation.authority.as_ref().ok_or(JournalError::Conflict)?;
                    hash(&authority.binding_hash)?;
                    hash(&authority.handle_fingerprint)?;
                    if let Some(owned) = preparation.owned() {
                        validation::helper_owned_context(
                            owned,
                            preparation.policy(),
                            plan.session_id,
                            &authority.handle_fingerprint,
                        )?;
                        let process = &owned.observation.process;
                        ensure(
                            process.running == Some(false)
                                && matches!(
                                    process.status.as_deref(),
                                    Some("created" | "configured" | "initialized")
                                )
                                && process.started_at.is_none(),
                        )?;
                    }
                }
                _ => return Err(JournalError::UnsupportedVersion),
            }
            let expected = self.import_preparation(uuid(&preparation.policy().helper_id)?)?;
            ensure(
                equal(preparation.policy(), expected.policy())?
                    && equal(preparation.permission(), expected.permission())?,
            )?;
        }
        if let Some(arm) = self.import_arm() {
            self.validate_import_arm(arm)?;
        }
        if let Some(claim) = self.import_claim() {
            self.validate_import_claim(claim)?;
        }
        self.validate_import_observations()?;
        if let Some(saved) = self.import_verification_plan() {
            ensure(saved.version == 1 && saved.preparation().owned().is_none())?;
            let helper_id = uuid(&saved.preparation().policy().helper_id)?;
            let (parent, permission) = self.import_verification_inputs()?;
            let expected = checked(helper_execution_policy_v1(
                &parent,
                HelperPurposeV1::ImportVerification,
                helper_id,
            ))?;
            ensure(
                saved.preparation().policy().purpose == HelperPurposeV1::ImportVerification
                    && saved.parent_revision() == parent.revision
                    && equal(saved.preparation().policy(), &expected)?
                    && equal(saved.preparation().permission(), &permission)?,
            )?;
            let import_helper = match permission {
                HelperPermissionV1::ImportVerification {
                    import_helper_id, ..
                } => uuid(&import_helper_id)?,
                _ => return Err(JournalError::Conflict),
            };
            ensure(helper_id != import_helper && helper_id != self.plan.operation_id)?;
        }
        if let Some(saved) = self.import_verification_create() {
            let plan = self
                .import_verification_plan()
                .ok_or(JournalError::Conflict)?;
            let policy = plan.preparation().policy();
            let terminal = match plan.preparation().permission() {
                HelperPermissionV1::ImportVerification {
                    import_terminal, ..
                } => import_terminal,
                _ => return Err(JournalError::Conflict),
            };
            ensure(
                saved.version == 1
                    && saved.terminal_receipt == *terminal
                    && saved.plan_hash
                        == checked(content_hash(
                            "ai-guard:legacy-named-import-verification-plan:v1",
                            plan,
                        ))?
                    && saved.spec_hash == import_verification_spec_hash(policy)?
                    && saved.authority_id == policy.authority_id
                    && saved.runtime_kind == policy.runtime_kind,
            )?;
            uuid(&saved.authority_id)?;
            hash(&saved.authority.handle_fingerprint)?;
            hash(&saved.authority.binding_hash)?;
        }
        ensure(canonical(self)?.len() <= MAX_OPERATION_BYTES)
    }

    fn validate_import_arm(&self, saved: &LegacyNamedImportArmCheckpointV1) -> JournalResult<()> {
        if saved.version != 1 {
            return Err(JournalError::UnsupportedVersion);
        }
        let admission = self.receipt().ok_or(JournalError::Conflict)?;
        let parent = admission
            .admission
            .worker
            .as_ref()
            .ok_or(JournalError::Conflict)?;
        let arm = &saved.arm;
        let operation_id = LegacyNamedImportArmRequestV1::operation_id(
            self.plan.binding_ref,
            uuid(&parent.attempt.attempt_id)?,
        );
        ensure(
            self.version == 2
                && arm.operation_id == operation_id
                && arm.operation_id != self.plan.operation_id.to_string()
                && arm.intent_hash == self.request.admission.intent_hash
                && matches!(
                    &arm.subject,
                    ArmSubjectV1::Helper { purpose: HelperPurposeV1::Import, helper_id, permission, .. }
                    if self.import_creation().is_none_or(|prep| &prep.policy().helper_id == helper_id)
                        && matches!(permission.as_ref(), HelperPermissionV1::Admission { receipt } if receipt == &admission.admission.receipt)
                ),
        )?;
        if let Some(creation) = &self.import_creation
            && creation.authority.is_some()
        {
            let owned = creation.preparation.owned().ok_or(JournalError::Conflict)?;
            ensure(
                owned.handle.id() == arm.inspection.container_id
                    && owned.handle.authority_fingerprint() == arm.authority_binding_hash
                    && owned.handle.fingerprint_version() == arm.configuration_fingerprint_version
                    && owned.observation.configuration_fingerprint.as_deref()
                        == Some(arm.configuration_fingerprint.as_str()),
            )?;
        }
        let prepare = WorkerMutationV1 {
            version: 1,
            operation_id: arm.operation_id.clone(),
            worker_id: parent.worker_id.clone(),
            attempt_id: parent.attempt.attempt_id.clone(),
            expected_revision: parent.revision,
            transition: WorkerTransitionV1::Prepare,
        };
        let prepared = checked(crate::audit::expected_worker_transition(parent, &prepare))?;
        checked(arm.validate_for_parent(&prepared))?;
        if let Some(receipt) = saved.receipt() {
            checked(receipt.validate_for_arm(arm))?;
            ensure(
                receipt.control_namespace_uuid == self.plan.control_namespace_uuid
                    && receipt.audit_seq > admission.audit_seq,
            )?;
        }
        Ok(())
    }

    fn validate_import_claim(
        &self,
        saved: &LegacyNamedImportClaimCheckpointV1,
    ) -> JournalResult<()> {
        if saved.version != 1 {
            return Err(JournalError::UnsupportedVersion);
        }
        ensure(
            self.import_creation()
                .and_then(HelperPreparationV1::owned)
                .is_some(),
        )?;
        let arm = self.import_arm().ok_or(JournalError::Conflict)?;
        let arm_receipt = arm.receipt().ok_or(JournalError::Conflict)?;
        let request = saved.request();
        checked(request.validate_for_arm(
            self.plan.binding_ref,
            &self.plan.control_namespace_uuid,
            &arm.arm,
            arm_receipt,
        ))?;
        ensure(
            request.claim.operation_id != self.plan.operation_id.to_string()
                && request.claim.operation_id != arm.arm.operation_id,
        )?;
        if let Some(acknowledgement) = saved.receipt() {
            receipt(acknowledgement)?;
            checked(request.validate_receipt(acknowledgement))?;
        }
        Ok(())
    }

    fn validate_import_observation_request(
        &self,
        request: &LegacyNamedImportObservationRequestV1,
    ) -> JournalResult<()> {
        checked(request.validate())?;
        let claim = self.import_claim().ok_or(JournalError::Conflict)?;
        let claim_receipt = claim.receipt().ok_or(JournalError::Conflict)?;
        let arm = self.import_arm().ok_or(JournalError::Conflict)?;
        let arm_receipt = arm.receipt().ok_or(JournalError::Conflict)?;
        let expected_operation = LegacyNamedImportObservationRequestV1::operation_id(
            self.plan.binding_ref,
            uuid(&claim.request.claim.operation_id)?,
            request.phase,
        );
        ensure(
            request.binding_ref == self.plan.binding_ref
                && request.namespace == self.plan.control_namespace_uuid
                && equal(&request.claim_receipt, claim_receipt)?
                && equal(&request.observation.arm_receipt, arm_receipt)?
                && request.observation.helper_id == claim.request.claim.helper_id
                && request.observation.operation_id == expected_operation
                && equal(
                    &request.observation.observation.inspection,
                    &arm.arm.inspection,
                )?,
        )?;
        Ok(())
    }

    fn validate_import_observations(&self) -> JournalResult<()> {
        ensure(self.import_observations.len() <= 2)?;
        for saved in &self.import_observations {
            ensure(saved.version == 1)?;
            self.validate_import_observation_request(&saved.request)?;
            if let Some(receipt) = saved.receipt() {
                checked(saved.request.validate_receipt(receipt))?;
            }
        }
        match self.import_observations.as_slice() {
            [] => Ok(()),
            [one] => match one.request.phase {
                LegacyNamedImportObservationPhaseV1::Started
                | LegacyNamedImportObservationPhaseV1::Terminal => Ok(()),
            },
            [started, terminal] => {
                ensure(
                    started.request.phase == LegacyNamedImportObservationPhaseV1::Started
                        && terminal.request.phase == LegacyNamedImportObservationPhaseV1::Terminal,
                )?;
                ensure(started.receipt().is_some())?;
                let started_fact = &started.request.observation.observation;
                let terminal_fact = &terminal.request.observation.observation;
                ensure(started_fact.started_at == terminal_fact.started_at)?;
                let started_seen = chrono::DateTime::parse_from_rfc3339(&started_fact.observed_at)
                    .map_err(|_| JournalError::Conflict)?;
                let terminal_seen =
                    chrono::DateTime::parse_from_rfc3339(&terminal_fact.observed_at)
                        .map_err(|_| JournalError::Conflict)?;
                ensure(started_seen <= terminal_seen)?;
                if let (Some(started_receipt), Some(terminal_receipt)) =
                    (started.receipt(), terminal.receipt())
                {
                    ensure(terminal_receipt.audit_seq > started_receipt.audit_seq)?;
                }
                Ok(())
            }
            _ => Err(JournalError::Conflict),
        }
    }

    pub(super) fn validate_transition(&self, previous: Option<&Self>) -> JournalResult<()> {
        match previous {
            None => ensure(
                self.receipt.is_none()
                    && self.import_arm.is_none()
                    && self.import_creation.is_none()
                    && self.import_claim.is_none()
                    && self.import_verification.is_none()
                    && self.import_verification_create.is_none(),
            ),
            Some(old) => {
                ensure(
                    self.version == old.version
                        && self.archive_digest == old.archive_digest
                        && self.plan == old.plan
                        && equal(&self.request, &old.request)?,
                )?;
                ensure(old.receipt.is_none() || equal(&self.receipt, &old.receipt)?)?;
                match (&old.import_creation, &self.import_creation) {
                    (None, None) => {}
                    (None, Some(new)) => ensure(
                        old.admission_result().is_some_and(|result| result.accepted)
                            && old.import_arm.is_none()
                            && self.import_arm.is_none()
                            && new.preparation.owned().is_none(),
                    )?,
                    (Some(previous), Some(current)) => {
                        ensure(
                            previous.version == current.version
                                && previous.authority == current.authority
                                && equal(
                                    previous.preparation.policy(),
                                    current.preparation.policy(),
                                )?
                                && equal(
                                    previous.preparation.permission(),
                                    current.preparation.permission(),
                                )?,
                        )?;
                        let old_owned = previous.preparation.owned();
                        let new_owned = current.preparation.owned();
                        if old_owned.is_some() {
                            ensure(equal(&old_owned, &new_owned)?)?;
                        } else if new_owned.is_some() {
                            ensure(old.import_arm.is_none() && self.import_arm.is_none())?;
                        }
                    }
                    (Some(_), None) => return Err(JournalError::Conflict),
                }
                match (old.import_claim(), self.import_claim()) {
                    (None, None) => {}
                    (None, Some(new)) => ensure(
                        old.import_arm().and_then(|arm| arm.receipt()).is_some()
                            && new.receipt().is_none(),
                    )?,
                    (Some(old), Some(new)) => ensure(
                        equal(old.request(), new.request())?
                            && (old.receipt().is_none() || equal(&old.receipt, &new.receipt)?),
                    )?,
                    (Some(_), None) => return Err(JournalError::Conflict),
                }
                ensure(self.import_observations.len() >= old.import_observations.len())?;
                ensure(self.import_observations.len() <= old.import_observations.len() + 1)?;
                if self.import_observations.len() > old.import_observations.len() {
                    ensure(
                        self.import_observations
                            .last()
                            .is_some_and(|new| new.receipt().is_none()),
                    )?;
                }
                for (previous, current) in old
                    .import_observations
                    .iter()
                    .zip(&self.import_observations)
                {
                    ensure(equal(previous.request(), current.request())?)?;
                    if let Some(receipt) = previous.receipt() {
                        let current_receipt = current.receipt().ok_or(JournalError::Conflict)?;
                        ensure(equal(receipt, current_receipt)?)?;
                    }
                }
                match (
                    old.import_verification_plan(),
                    self.import_verification_plan(),
                ) {
                    (None, None) => {}
                    (None, Some(current)) => {
                        // The named Terminal ACK must predate the verification
                        // intent; one atomic write cannot invent both facts.
                        let previous_permission = old.import_verification_permission()?;
                        ensure(
                            current.version == 1
                                && current.preparation().owned().is_none()
                                && equal(current.preparation().permission(), &previous_permission)?,
                        )?;
                    }
                    (Some(previous), Some(current)) => {
                        ensure(equal(previous, current)?)?;
                    }
                    (Some(_), None) => return Err(JournalError::Conflict),
                }
                match (
                    old.import_verification_create(),
                    self.import_verification_create(),
                ) {
                    (None, None) => Ok(()),
                    (None, Some(current)) => ensure(
                        old.import_verification_plan().is_some()
                            && current.version == 1
                            && self.import_verification_plan().is_some(),
                    ),
                    (Some(previous), Some(current)) => ensure(equal(previous, current)?),
                    (Some(_), None) => Err(JournalError::Conflict),
                }?;
                match (old.import_arm(), self.import_arm()) {
                    (None, None) => Ok(()),
                    (None, Some(new)) => ensure(
                        old.admission_result().is_some_and(|result| result.accepted)
                            && new.receipt().is_none(),
                    ),
                    (Some(old), Some(new)) => ensure(
                        equal(&old.arm, &new.arm)?
                            && (old.receipt().is_none() || equal(&old.receipt, &new.receipt)?),
                    ),
                    (Some(_), None) => Err(JournalError::Conflict),
                }
            }
        }
    }
}

/// The broker uses the same linked receipt checks at the HTTP boundary and
/// when reading an acknowledged checkpoint.
pub(crate) fn validate_legacy_named_admission_receipt(
    namespace: Uuid,
    request: &LegacyNamedSupervisedAdmissionV1,
    saved: &LegacyNamedAdmissionReceiptV1,
) -> JournalResult<()> {
    ensure(saved.version == 1)?;
    let result = &saved.admission;
    ensure(result.accepted == result.refusal.is_none())?;
    validation::bound_receipt(
        &result.receipt,
        &request.admission.admission.operation_id,
        &checked(request.admission.input_hash())?,
        result
            .worker
            .as_ref()
            .map(|worker| worker.worker_id.as_str()),
        result.worker.as_ref().map(|worker| worker.revision),
    )?;
    hash(&saved.audit_hash)?;
    match (result.accepted, &result.worker, &saved.grant) {
        (true, Some(worker), Some(grant)) => {
            validation::bound_worker(worker, &request.admission, namespace)?;
            ensure(
                worker.state == WorkerStateV1::Reserved
                    && worker.revision == 1
                    && worker.quota_held
                    && worker.attempt.container_id.is_none()
                    && worker.attempt.created_inspection.is_none()
                    && worker.attempt.create_operation_id.is_none()
                    && worker.attempt.start_emitter_id.is_none()
                    && worker.attempt.start_operation_id.is_none(),
            )?;
            uuid(&grant.operation_id)?;
            hash(&grant.input_hash)?;
            hash(&grant.audit_hash)?;
            ensure(
                grant.version == 1
                    && (1..=i64::MAX as u64).contains(&grant.authorization_revision)
                    && result.receipt.audit_seq.checked_add(1) == Some(grant.audit_seq)
                    && grant.audit_seq.checked_add(1) == Some(saved.audit_seq),
            )
        }
        (false, None, None) => {
            ensure(result.receipt.audit_seq.checked_add(1) == Some(saved.audit_seq))
        }
        _ => Err(JournalError::Conflict),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        broker::enforcer::{
            CancelGeneration, LegacyIpcFixture, NamedImportArmBirthV1, ProspectiveRegistrationV1,
            connect_named_legacy_fixture, deliver_named_import_stop_from_legacy_root,
        },
        broker::{
            clock::{HostClock, NativeHostClock, checked_deadline},
            journal::{
                ControlSecrets, FsExclusions, Journal, JournalLimitsV1, ReserveResult,
                recovery::StopRecoveryReaderV1,
            },
            lifecycle::BrokerCheckpointV3,
            writer::{CancelInputV1, NamedImportArmWriteFailure, WriterClient},
        },
        control::session_control::{LegacyConfirmedLaunchSpecV1, LegacyNamedInitialDecisionV1},
        control_fs::Directory,
        identity::IdentityReceiptV1,
        runner::{RunOptions, broker::PreparedSubmission},
    };
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::{os::unix::fs::PermissionsExt, time::Duration};

    fn import_observation_fixture(
        plan: &LegacyConfirmedPlanV1,
        claim: &LegacyNamedImportClaimRequestV1,
        claim_receipt: &MutationReceiptV1,
        arm: &ArmResultV1,
        phase: LegacyNamedImportObservationPhaseV1,
        started_at: &str,
        observed_at: &str,
        finished_at: Option<&str>,
    ) -> LegacyNamedImportObservationRequestV1 {
        let (status, running, exit_code) = match phase {
            LegacyNamedImportObservationPhaseV1::Started => {
                (RuntimeStatusV1::Running, Some(true), None)
            }
            LegacyNamedImportObservationPhaseV1::Terminal => {
                (RuntimeStatusV1::Exited, Some(false), Some(0))
            }
        };
        LegacyNamedImportObservationRequestV1 {
            version: 1,
            binding_ref: plan.binding_ref,
            namespace: plan.control_namespace_uuid.clone(),
            phase,
            claim_receipt: claim_receipt.clone(),
            observation: HelperObservationV1 {
                version: 1,
                operation_id: LegacyNamedImportObservationRequestV1::operation_id(
                    plan.binding_ref,
                    uuid(&claim.claim.operation_id).unwrap(),
                    phase,
                ),
                helper_id: claim.claim.helper_id.clone(),
                arm_receipt: arm.receipt.clone(),
                observation: RuntimeObservationV1 {
                    inspection: arm.arm.inspection.clone(),
                    observed_at: observed_at.to_owned(),
                    status,
                    running,
                    started_at: Some(started_at.to_owned()),
                    finished_at: finished_at.map(str::to_owned),
                    exit_code,
                    oom_killed: Some(false),
                    restart_count: Some(0),
                },
                verification: None,
            },
        }
    }

    async fn seed_named_import_writer_through_claim(
        writer: &WriterClient,
        plan: &LegacyConfirmedPlanV1,
        archive_digest: &str,
        helper_id: Uuid,
        authority: &RuntimeAuthority,
        owned: &OwnedContainerSnapshot,
        arm: &ArmV1,
        arm_result: &ArmResultV1,
        claim: &LegacyNamedImportClaimRequestV1,
        claim_receipt: &MutationReceiptV1,
    ) -> anyhow::Result<u64> {
        writer
            .record_legacy_named_import_creation(plan, archive_digest, helper_id, authority)
            .await?;
        let creation_revision = writer
            .load(plan.operation_id)
            .await?
            .ok_or(JournalError::Conflict)?
            .head()
            .revision();
        writer
            .acknowledge_legacy_named_import_creation(
                plan,
                archive_digest,
                authority,
                owned.clone(),
            )
            .await?;
        let recorded_arm = writer
            .record_legacy_named_import_arm(plan, archive_digest, arm.clone())
            .await?;
        writer
            .acknowledge_legacy_named_import_arm(
                plan.operation_id,
                recorded_arm.request(),
                arm_result.clone(),
            )
            .await?;
        writer
            .record_legacy_named_import_claim(plan, claim.clone())
            .await?;
        writer
            .acknowledge_legacy_named_import_claim(plan.operation_id, claim, claim_receipt.clone())
            .await?;
        Ok(creation_revision)
    }

    /// Reopen an existing named Writer journal and retry the exact retained
    /// observation. This is a cold Writer fixture only, not a Service or
    /// runtime reconciliation test.
    async fn reopen_and_replay_import_observation(
        root: &std::path::Path,
        journal_name: &str,
        scope: &JournalScopeV1,
        plan: &LegacyConfirmedPlanV1,
        request: &LegacyNamedImportObservationRequestV1,
        receipt: &MutationReceiptV1,
    ) -> anyhow::Result<crate::broker::journal::RevisionRefV1> {
        let journal_dir = Directory::open_existing(&root.join(journal_name))?;
        let journal = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match Journal::open(
                    &journal_dir,
                    scope.clone(),
                    &FsExclusions::new(),
                    JournalLimitsV1::default(),
                    ControlSecrets::new(vec![])?,
                ) {
                    Ok(journal) => break Ok(journal),
                    Err(JournalError::Busy) => tokio::task::yield_now().await,
                    Err(error) => break Err(error),
                }
            }
        })
        .await??;
        let writer = WriterClient::start(journal, 8, Duration::from_secs(5))?;
        let retained = writer
            .load_legacy_named_import_observation(plan.operation_id, request.phase)
            .await?
            .expect("cold Writer retains the exact observation");
        assert!(equal(retained.request(), request)?);
        assert_eq!(retained.receipt(), Some(receipt));
        let head_before_replay = writer
            .load(plan.operation_id)
            .await?
            .unwrap()
            .head()
            .clone();
        let replay = writer
            .record_legacy_named_import_observation(plan, request.clone())
            .await?;
        assert_eq!(replay.receipt(), Some(receipt));
        writer
            .acknowledge_legacy_named_import_observation(
                plan.operation_id,
                request,
                receipt.clone(),
            )
            .await?;
        let head_after_replay = writer
            .load(plan.operation_id)
            .await?
            .unwrap()
            .head()
            .clone();
        assert_eq!(head_after_replay, head_before_replay);
        drop(writer);
        Ok(head_after_replay)
    }

    #[tokio::test]
    async fn explicit_named_admission_requires_an_immutable_archive_digest() -> anyhow::Result<()> {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()?;
        let root = temporary.path().canonicalize()?;
        let config = Directory::open_existing(&root)?;
        let operation = Uuid::new_v4();
        let binding_ref = Uuid::new_v4();
        let session = Uuid::new_v4();
        let namespace = Uuid::new_v4();
        let launch: LaunchSpecV1 = serde_json::from_value(json!({
            "version": 1,
            "session_id": session,
            "command": {"argv": ["/usr/bin/id", "-u"], "cwd": "/workspace"},
            "profile": "strict",
            "runtime_kind": "docker",
            "authority_id": Uuid::new_v4(),
            "image_ref": "worker:fixture",
            "image_id": format!("sha256:{}", "b".repeat(64)),
            "network": "none",
            "cpu_millis": 2000,
            "memory_bytes": 1_073_741_824_u64,
            "pids_limit": 256,
            "timeout_seconds": 30,
            "security_preset": "fixed_sandbox_v2",
            "restart_policy": "no",
            "workspace_manifest_digest": "c".repeat(64),
            "workspace_import_policy_version": 1,
            "provider": "generic",
            "capture_output": false,
            "credential_binding_id": null,
            "credential_names": [],
        }))?;
        let plan = LegacyConfirmedPlanV1 {
            version: 1,
            binding_ref,
            control_namespace_uuid: namespace.to_string(),
            operation_id: operation,
            session_id: session,
            launch_target_id: Uuid::new_v4(),
            target_revision: 1,
            broker_profile_id: "fixture-profile".into(),
            launch_spec_hash: launch.spec_hash()?,
            expires_at: chrono::DateTime::from_timestamp(4_102_444_800, 0).unwrap(),
        };
        let material = LegacyConfirmedLaunchSpecV1 {
            version: 2,
            plan: plan.clone(),
            launch: launch.clone(),
            source_identity_hash: Some("a".repeat(64)),
        };
        let gateway = "http://127.0.0.1:8787";
        let options: RunOptions = serde_json::from_value(json!({
            "project": root,
            "config_dir": root,
            "gateway": gateway,
            "runtime": "docker",
            "image": launch.image_ref,
            "profile": launch.profile,
            "network": "none",
            "allow_rootful_runtime": false,
            "timeout": launch.timeout_seconds,
            "provider": launch.provider,
            "capture_output": false,
            "argv": launch.command.argv,
        }))?;
        let intent = ExecutionIntentV1::from_legacy_named_launch(options.clone(), &material)?;
        let scope = JournalScopeV1::new(
            Uuid::new_v4(),
            "fixture",
            gateway,
            namespace,
            &plan.broker_profile_id,
        )?;
        let binding: BindingManifestV1 = serde_json::from_value(json!({"kind": "none"}))?;
        let runner = CheckpointPayloadV1::initial(&intent)?;
        let decision_request = intent.request_data().clone();
        let receipt = DecisionReceiptV1 {
            version: 1,
            request_id: decision_request.request_id.clone(),
            request_hash: crate::policy::request_hash(&decision_request),
            phase: DecisionPhaseV1::Initial,
            audit_seq: 1,
            audit_hash: "d".repeat(64),
        };
        let answer = DecisionAnswerV1::new(
            Decision::new(Outcome::Allow, "fixture", "Exact policy decision."),
            receipt.clone(),
        )?;
        let initial_decision = LegacyNamedInitialDecisionV1 {
            version: 1,
            binding_ref,
            request: decision_request.clone(),
            decision: answer.decision().clone(),
            receipt: receipt.clone(),
            replayed: false,
        };
        let request = LegacyNamedSupervisedAdmissionV1 {
            version: 1,
            binding_ref,
            broker_profile_id: plan.broker_profile_id.clone(),
            admission: SupervisedAdmissionV1 {
                version: 1,
                protocol: SupervisionProtocolV1::BrokerSupervisionV1,
                intent_hash: intent.intent_hash()?,
                admission: AdmissionV1 {
                    version: 1,
                    operation_id: operation.to_string(),
                    decision_request: decision_request.clone(),
                    receipt,
                    launch,
                },
            },
        };
        let decided = SupervisionCheckpointV1::new(
            namespace,
            vec![],
            vec![SupervisionOperationV1::Decision {
                request: decision_request,
                response: Some(answer),
            }],
        )?;
        let before = BrokerCheckpointV2::new(runner.clone(), decided.clone())?;
        before.validate_context(&intent, &scope, &binding)?;
        let checkpoint = |named| {
            BrokerCheckpointV2::new(
                runner.clone(),
                decided.clone().with_legacy_named_admission(named)?,
            )
        };

        let historical = canonical(&json!({
            "version": 1, "plan": plan, "request": request, "receipt": null
        }))?;
        let legacy: LegacyNamedAdmissionCheckpoint = serde_json::from_slice(&historical)?;
        assert_eq!(canonical(&legacy)?, historical);
        assert!(legacy.pending_v2().is_none());
        let mut unexpected: serde_json::Value = serde_json::from_slice(&historical)?;
        unexpected["archive_digest"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<LegacyNamedAdmissionCheckpoint>(unexpected).is_err());
        assert!(matches!(
            checkpoint(legacy)?.validate_context(&intent, &scope, &binding),
            Err(JournalError::Conflict)
        ));

        let digest = "e".repeat(64);
        let pending = LegacyNamedAdmissionCheckpoint::pending_with_archive(
            plan.clone(),
            request.clone(),
            digest.clone(),
        )?;
        assert_eq!(
            canonical(&pending)?,
            canonical(&json!({
                "version": 2, "plan": plan, "request": request, "receipt": null,
                "archive_digest": digest
            }))?
        );
        let prepared = checkpoint(pending.clone())?;
        prepared.validate_transition(Some(&before))?;
        prepared.validate_context(&intent, &scope, &binding)?;
        let mut historical_material = material.clone();
        historical_material.version = 1;
        historical_material.source_identity_hash = None;
        let unattested =
            ExecutionIntentV1::from_legacy_named_launch(options.clone(), &historical_material)?;
        let mut unattested_request = request.clone();
        unattested_request.admission.intent_hash = unattested.intent_hash()?;
        let unattested_pending = LegacyNamedAdmissionCheckpoint::pending_with_archive(
            plan.clone(),
            unattested_request,
            digest.clone(),
        )?;
        assert!(matches!(
            checkpoint(unattested_pending)?.validate_context(&unattested, &scope, &binding),
            Err(JournalError::Conflict)
        ));
        let mut archive_free = serde_json::to_value(&prepared)?;
        archive_free["supervision"]["legacy_named_admission"]
            .as_object_mut()
            .unwrap()
            .remove("archive_digest");
        assert!(BrokerCheckpointV2::from_bytes(&serde_json::to_vec(&archive_free)?).is_err());
        let reopened = BrokerCheckpointV2::from_bytes(&prepared.canonical_bytes()?)?;
        let observed = reopened
            .supervision()
            .legacy_named_admission()
            .unwrap()
            .pending_v2()
            .unwrap();
        assert_eq!(observed.plan, &plan);
        assert_eq!(observed.request.input_hash()?, request.input_hash()?);
        assert_eq!(observed.archive_digest, digest);
        let swapped = checkpoint(LegacyNamedAdmissionCheckpoint::pending_with_archive(
            plan.clone(),
            request.clone(),
            "f".repeat(64),
        )?)?;
        assert!(matches!(
            swapped.validate_transition(Some(&reopened)),
            Err(JournalError::Conflict)
        ));

        let saved = LegacyNamedAdmissionReceiptV1 {
            version: 1,
            admission: AdmissionResultV1 {
                accepted: false,
                replayed: false,
                refusal: Some(AdmissionRefusalV1::Policy),
                worker: None,
                receipt: MutationReceiptV1 {
                    operation_id: operation.to_string(),
                    worker_id: None,
                    revision: None,
                    input_hash: request.admission.input_hash()?,
                    audit_seq: 2,
                    audit_hash: "1".repeat(64),
                },
            },
            grant: None,
            audit_seq: 3,
            audit_hash: "2".repeat(64),
        };
        let mut divergent = saved.clone();
        divergent.admission.receipt.input_hash = "0".repeat(64);
        assert!(pending.clone().acknowledge(divergent).is_err());
        let acknowledged = pending.clone().acknowledge(saved.clone())?;
        assert!(acknowledged.pending_v2().is_none());
        assert_eq!(acknowledged.archive_digest(), Some(digest.as_str()));
        let acknowledged = checkpoint(acknowledged)?;
        acknowledged.validate_transition(Some(&reopened))?;
        let reopened = BrokerCheckpointV2::from_bytes(&acknowledged.canonical_bytes()?)?;
        let named = reopened.supervision().legacy_named_admission().unwrap();
        assert!(equal(named.receipt().unwrap(), &saved)?);

        let start_writer = async |name: &str| -> anyhow::Result<WriterClient> {
            let journal_dir = Directory::open_or_create(&root.join(name))?;
            let journal = Journal::open(
                &journal_dir,
                scope.clone(),
                &FsExclusions::new(),
                JournalLimitsV1::default(),
                ControlSecrets::new(vec![])?,
            )?;
            let writer = WriterClient::start(journal, 8, Duration::from_secs(5))?;
            let ReserveResult::New(ticket) =
                writer.reserve_legacy_named(operation, binding_ref).await?
            else {
                panic!("fixture must have a fresh operation");
            };
            let submission = PreparedSubmission::from_legacy_named_launch(
                options.clone(),
                config.clone(),
                &material,
            )?;
            writer
                .accept_prepared(ticket, submission)
                .await
                .map_err(|failure| failure.error())?;
            writer.prepare_legacy_named_initial_decision(&plan).await?;
            writer
                .acknowledge_legacy_named_initial_decision(&plan, &initial_decision)
                .await?;
            Ok(writer)
        };
        for stop_first in [true, false] {
            let writer = start_writer(&format!("journal-{stop_first}")).await?;
            if stop_first {
                let stop = writer
                    .stops()
                    .append_cancel(CancelInputV1::new(operation, Uuid::new_v4())?)
                    .await
                    .map_err(|failure| failure.error())?;
                assert!(stop.named_reservation().is_none());
                let stopped = writer.load(operation).await?.unwrap();
                let next = BrokerCheckpointV3::new(
                    prepared.clone(),
                    stopped.broker_checkpoint_v3().unwrap().commands().to_vec(),
                )?;
                assert!(matches!(
                    writer
                        .advance_lifecycle(operation, stopped.head().clone(), next)
                        .await,
                    Err(JournalError::Conflict)
                ));
                assert!(matches!(
                    writer
                        .record_legacy_named_admission(&plan, digest.clone())
                        .await,
                    Err(JournalError::Interrupted)
                ));
                continue;
            }
            let before = writer.load(operation).await?.unwrap();
            let (one, two) = tokio::join!(
                writer.record_legacy_named_admission(&plan, digest.clone()),
                writer.record_legacy_named_admission(&plan, digest.clone()),
            );
            let recorded = match (one, two) {
                (Ok(recorded), Err(JournalError::Conflict))
                | (Err(JournalError::Conflict), Ok(recorded)) => recorded,
                _ => panic!("exact concurrency must yield one permission to emit"),
            };
            assert_eq!(recorded.into_request().input_hash()?, request.input_hash()?);
            let next = BrokerCheckpointV3::new(prepared.clone(), Vec::new())?;
            assert!(matches!(
                writer
                    .advance_lifecycle(operation, before.head().clone(), next)
                    .await,
                Err(JournalError::Conflict)
            ));
            let stop_id = Uuid::new_v4();
            let stop = writer
                .stops()
                .append_cancel(CancelInputV1::new(operation, stop_id)?)
                .await
                .map_err(|failure| failure.error())?;
            assert!(!stop.incomplete());
            assert!(stop.stops().is_empty());
            let reservation = stop.named_reservation().unwrap();
            assert!(reservation.admission().receipt().is_none());
            assert_eq!(
                reservation.admission().request().input_hash()?,
                request.input_hash()?
            );
            let mut divergent = saved.clone();
            divergent.admission.receipt.input_hash = "0".repeat(64);
            assert!(reservation.cancellation(divergent).is_err());
            assert!(reservation.cancellation(saved.clone())?.is_none());
            writer
                .acknowledge_legacy_named_admission(&plan, &request, saved.clone())
                .await?;
            let stopped = writer
                .stops()
                .cancel_stop_projection(operation, stop_id)
                .await?
                .unwrap();
            assert!(stopped.named_reservation().is_none());
            let retained = writer.load_legacy_named_admission(operation).await?;
            assert_eq!(retained.archive_digest(), Some(digest.as_str()));
            assert!(equal(retained.receipt().unwrap(), &saved)?);
        }

        let worker = Uuid::new_v4();
        let stable_name = format!("aig-{}-{}", namespace.simple(), worker.simple());
        let reserved = WorkerV1 {
            worker_id: worker.to_string(),
            admission_operation_id: operation.to_string(),
            request_id: request
                .admission
                .admission
                .decision_request
                .request_id
                .clone(),
            receipt: request.admission.admission.receipt.clone(),
            launch: request.admission.admission.launch.clone(),
            spec_hash: plan.launch_spec_hash.clone(),
            policy_hash: "3".repeat(64),
            state: WorkerStateV1::Reserved,
            quota_held: true,
            revision: 1,
            attempt: WorkerAttemptV1 {
                attempt_id: Uuid::new_v4().to_string(),
                volume_name: format!("{stable_name}-work"),
                stable_name,
                container_id: None,
                created_inspection: None,
                create_operation_id: None,
                start_operation_id: None,
                start_emitter_id: None,
            },
            observed_started_at: None,
            allocation_inconsistent: false,
            last_observation: None,
            unknown_reason: None,
            cleanup_removed: None,
            cleanup_error: None,
        };
        let admission = LegacyNamedAdmissionReceiptV1 {
            version: 1,
            admission: AdmissionResultV1 {
                accepted: true,
                replayed: false,
                refusal: None,
                worker: Some(reserved.clone()),
                receipt: MutationReceiptV1 {
                    worker_id: Some(worker.to_string()),
                    revision: Some(1),
                    ..saved.admission.receipt.clone()
                },
            },
            grant: Some(IdentityReceiptV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                input_hash: "4".repeat(64),
                authorization_revision: 1,
                audit_seq: 3,
                audit_hash: "5".repeat(64),
            }),
            audit_seq: 4,
            audit_hash: "6".repeat(64),
        };
        let helper_id = Uuid::new_v4();
        let helper = helper_execution_policy_v1(&reserved, HelperPurposeV1::Import, helper_id)?;
        // Private codec fixtures for a Created callback. No runtime is opened.
        let unavailable = root.join("unavailable-runtime");
        let authority: RuntimeAuthority = serde_json::from_value(json!({
            "authority_id": helper.authority_id,
            "kind": "docker",
            "endpoint": format!("unix://{}", root.join("unavailable.sock").display()),
            "engine_id": "import-fixture",
            "invocation": {
                "policy_version": 2, "kind": "docker",
                "approved_entry": unavailable, "canonical_program": unavailable,
                "file_identity": {
                    "device": 1, "inode": 2, "owner": 0, "group": 0, "mode": 493,
                    "length": 1, "sha256": "b".repeat(64)
                },
                "environment": {}, "cwd": root
            },
            "podman": null
        }))?;
        let clock = NativeHostClock::new()?.sample()?;
        let armed_ns = clock.continuous_ns()?;
        let arm = ArmV1 {
            version: 1,
            operation_id: LegacyNamedImportArmRequestV1::operation_id(
                plan.binding_ref,
                uuid(&reserved.attempt.attempt_id)?,
            ),
            worker_id: reserved.worker_id.clone(),
            attempt_id: reserved.attempt.attempt_id.clone(),
            expected_revision: 2,
            intent_hash: request.admission.intent_hash.clone(),
            subject: ArmSubjectV1::Helper {
                helper_id: helper_id.to_string(),
                purpose: HelperPurposeV1::Import,
                policy_version: helper.version,
                permission: Box::new(HelperPermissionV1::Admission {
                    receipt: admission.admission.receipt.clone(),
                }),
            },
            inspection: RuntimeInspectionV1 {
                authority_id: helper.authority_id.clone(),
                runtime_kind: helper.runtime_kind,
                container_id: "7".repeat(64),
                stable_name: helper.stable_name.clone(),
                volume_name: helper.volume_name.clone(),
                image_id: helper.image_id.clone(),
                spec_hash: helper.spec_hash()?,
                cpu_millis: helper.cpu_millis,
                memory_bytes: helper.memory_bytes,
                pids_limit: helper.pids_limit,
                network: helper.network,
                security_preset: helper.security_preset,
                restart_policy: helper.restart_policy,
            },
            authority_binding_hash: authority.fingerprint(),
            configuration_fingerprint_version: 2,
            configuration_fingerprint: "9".repeat(64),
            clock,
            timeout_seconds: 120,
            deadline_ns_decimal: checked_deadline(armed_ns, 120)?.to_string(),
        };
        let owned: OwnedContainerSnapshot = serde_json::from_value(json!({
            "handle": {
                "authority_fingerprint": arm.authority_binding_hash, "session": session,
                "operation": helper_id, "id": arm.inspection.container_id,
                "name": helper.stable_name, "configuration_fingerprint_version": 2
            },
            "observation": {
                "id": arm.inspection.container_id, "name": helper.stable_name,
                "image_id": helper.image_id, "restart_count": 0,
                "process": {
                    "status": "created", "running": false, "started_at": null,
                    "finished_at": null, "exit_code": null, "oom_killed": false
                },
                "allocation": {
                    "cpu": {"quota": 2, "period": 1}, "memory_bytes": helper.memory_bytes,
                    "pids_limit": helper.pids_limit
                },
                "configuration_fingerprint": arm.configuration_fingerprint,
                "configuration_fingerprint_version": 2,
                "observed_at": "2026-11-27T00:00:00Z", "diagnostics": []
            }
        }))?;
        let arm_result = ArmResultV1 {
            replayed: true,
            arm: arm.clone(),
            receipt: ArmReceiptV1 {
                version: 1,
                arm_id: arm.operation_id.clone(),
                worker_id: arm.worker_id.clone(),
                attempt_id: arm.attempt_id.clone(),
                input_hash: arm.input_hash()?,
                control_namespace_uuid: namespace.to_string(),
                audit_seq: 5,
                audit_hash: "a".repeat(64),
            },
        };
        let claim = LegacyNamedImportClaimRequestV1 {
            version: 1,
            binding_ref,
            namespace: namespace.to_string(),
            claim: HelperStartClaimV1 {
                version: 1,
                operation_id: LegacyNamedImportClaimRequestV1::operation_id(
                    binding_ref,
                    uuid(&arm_result.receipt.arm_id)?,
                ),
                helper_id: helper_id.to_string(),
                emitter_id: Uuid::new_v4().to_string(),
                arm_receipt: arm_result.receipt.clone(),
                clock_check: HostClockSampleV1 {
                    continuous_ns_decimal: (armed_ns + 1).to_string(),
                    ..arm.clock.clone()
                },
            },
        };
        let claim_receipt = MutationReceiptV1 {
            operation_id: claim.claim.operation_id.clone(),
            input_hash: claim.input_hash()?,
            worker_id: Some(arm.worker_id.clone()),
            revision: None,
            audit_seq: arm_result.receipt.audit_seq + 1,
            audit_hash: "c".repeat(64),
        };
        let accepted = pending.clone().acknowledge(admission.clone())?;
        let named_arm = accepted.clone().with_import_arm(arm.clone())?;
        assert!(matches!(
            checkpoint(named_arm.clone())?.validate_transition(Some(&prepared)),
            Err(JournalError::Conflict)
        ));
        let arm_without_latch = checkpoint(
            named_arm
                .clone()
                .acknowledge_import_arm(arm_result.clone())?,
        )?;
        let historical_bytes = arm_without_latch.canonical_bytes()?;
        let reopened = BrokerCheckpointV2::from_bytes(&historical_bytes)?;
        assert_eq!(reopened.canonical_bytes()?, historical_bytes);
        let retained = reopened.supervision().legacy_named_admission().unwrap();
        assert!(retained.import_creation().is_none());
        assert!(retained.import_claim().is_none());
        assert!(equal(
            retained.import_arm().unwrap().receipt().unwrap(),
            &arm_result.receipt
        )?);
        let latched = accepted
            .clone()
            .with_import_creation(helper_id, &authority)?;
        let mut historical_latch = latched.clone();
        let historical_creation = historical_latch.import_creation.as_mut().unwrap();
        historical_creation.version = 1;
        historical_creation.authority = None;
        let historical_bytes = checkpoint(historical_latch)?.canonical_bytes()?;
        let reopened = BrokerCheckpointV2::from_bytes(&historical_bytes)?;
        assert_eq!(reopened.canonical_bytes()?, historical_bytes);
        let historical_latch = reopened.supervision().legacy_named_admission().unwrap();
        assert!(
            historical_latch
                .import_creation()
                .unwrap()
                .owned()
                .is_none()
        );
        assert!(matches!(
            historical_latch
                .clone()
                .with_import_owned(&authority, owned.clone()),
            Err(JournalError::Conflict)
        ));
        let other_latch = accepted
            .clone()
            .with_import_creation(Uuid::new_v4(), &authority)?;
        assert!(matches!(
            other_latch
                .clone()
                .with_import_owned(&authority, owned.clone()),
            Err(JournalError::Conflict)
        ));

        let start_admitted_writer = async |name: &str| -> anyhow::Result<WriterClient> {
            let writer = start_writer(name).await?;
            let recorded = writer
                .record_legacy_named_admission(&plan, digest.clone())
                .await?;
            writer
                .acknowledge_legacy_named_admission(
                    &plan,
                    &recorded.into_request(),
                    admission.clone(),
                )
                .await?;
            Ok(writer)
        };
        let mut cold_stop_case = None;
        for (stop_first, record_claim) in [(true, false), (false, false), (false, true)] {
            let namespace = format!("import-{stop_first}-{record_claim}");
            let writer = start_admitted_writer(&namespace).await?;
            let admitted = writer.load(operation).await?.unwrap();
            assert!(
                writer
                    .load_legacy_named_import_arm(operation)
                    .await?
                    .is_none()
            );
            if stop_first {
                writer
                    .stops()
                    .append_cancel(CancelInputV1::new(operation, Uuid::new_v4())?)
                    .await
                    .map_err(|failure| failure.error())?;
                let stale = BrokerCheckpointV3::new(checkpoint(named_arm.clone())?, Vec::new())?;
                assert!(matches!(
                    writer
                        .advance_lifecycle(operation, admitted.head().clone(), stale)
                        .await,
                    Err(JournalError::Conflict)
                ));
                let stopped = writer.load(operation).await?.unwrap();
                let next = BrokerCheckpointV3::new(
                    checkpoint(named_arm.clone())?,
                    stopped.broker_checkpoint_v3().unwrap().commands().to_vec(),
                )?;
                assert!(matches!(
                    writer
                        .advance_lifecycle(operation, stopped.head().clone(), next)
                        .await,
                    Err(JournalError::Conflict)
                ));
                assert!(matches!(
                    writer
                        .record_legacy_named_import_arm(&plan, &digest, arm.clone())
                        .await,
                    Err(JournalError::Interrupted)
                ));
                let next = BrokerCheckpointV3::new(
                    checkpoint(latched.clone())?,
                    stopped.broker_checkpoint_v3().unwrap().commands().to_vec(),
                )?;
                assert!(matches!(
                    writer
                        .advance_lifecycle(operation, stopped.head().clone(), next)
                        .await,
                    Err(JournalError::Conflict)
                ));
                assert!(matches!(
                    writer
                        .record_legacy_named_import_creation(&plan, &digest, helper_id, &authority)
                        .await,
                    Err(JournalError::Interrupted)
                ));
                continue;
            }

            assert!(matches!(
                writer
                    .record_legacy_named_import_arm(&plan, &digest, arm.clone())
                    .await,
                Err(JournalError::Conflict)
            ));
            assert_eq!(
                writer.load(operation).await?.unwrap().head(),
                admitted.head()
            );
            let recorded_creation = writer
                .record_legacy_named_import_creation(&plan, &digest, helper_id, &authority)
                .await?;
            assert!(matches!(
                writer
                    .record_legacy_named_import_arm(&plan, &digest, arm.clone())
                    .await,
                Err(JournalError::Conflict)
            ));
            assert_eq!(
                writer.load(operation).await?.unwrap().head(),
                recorded_creation.job().head()
            );
            writer
                .acknowledge_legacy_named_import_creation(&plan, &digest, &authority, owned.clone())
                .await?;
            let before_arm = writer.load(operation).await?.unwrap();
            // Let the attempt's read complete, then fill the paused Writer
            // before polling the attempt far enough to enqueue its advance.
            let before_read = writer.pause_for_test().await?;
            let attempt =
                writer.record_legacy_named_import_arm_diagnosed(&plan, &digest, arm.clone());
            tokio::pin!(attempt);
            assert!(futures_util::poll!(&mut attempt).is_pending());
            let pause_after_read = writer.pause_for_test();
            tokio::pin!(pause_after_read);
            assert!(futures_util::poll!(&mut pause_after_read).is_pending());
            assert!(before_read.is_paused());
            drop(before_read);
            let after_read = pause_after_read.await?;
            let mut queued = (0..8)
                .map(|_| Box::pin(writer.status(operation)))
                .collect::<Vec<_>>();
            for read in &mut queued {
                assert!(futures_util::poll!(read.as_mut()).is_pending());
            }
            assert!(after_read.is_paused());
            assert!(matches!(
                attempt.await,
                Err(NamedImportArmWriteFailure::BeforePublication(
                    JournalError::Busy
                ))
            ));
            drop(after_read);
            for read in queued {
                assert!(read.await?.is_some());
            }
            assert_eq!(
                writer.load(operation).await?.unwrap().head(),
                before_arm.head()
            );
            assert!(
                writer
                    .load_legacy_named_import_arm(operation)
                    .await?
                    .is_none()
            );
            let mut different_container = arm.clone();
            different_container.inspection.container_id = "a".repeat(64);
            assert!(matches!(
                writer
                    .record_legacy_named_import_arm(&plan, &digest, different_container)
                    .await,
                Err(JournalError::Conflict)
            ));
            assert!(matches!(
                writer
                    .record_legacy_named_import_arm(&plan, &"0".repeat(64), arm.clone())
                    .await,
                Err(JournalError::Conflict)
            ));
            let mut different_attempt = arm.clone();
            different_attempt.attempt_id = Uuid::new_v4().to_string();
            assert!(matches!(
                writer
                    .record_legacy_named_import_arm(&plan, &digest, different_attempt)
                    .await,
                Err(JournalError::Conflict)
            ));
            let mut different_operation = arm.clone();
            different_operation.operation_id =
                LegacyNamedImportArmRequestV1::operation_id(Uuid::new_v4(), uuid(&arm.attempt_id)?);
            assert!(matches!(
                writer
                    .record_legacy_named_import_arm(&plan, &digest, different_operation)
                    .await,
                Err(JournalError::Conflict)
            ));
            assert_eq!(
                writer.load(operation).await?.unwrap().head(),
                before_arm.head()
            );
            let (first, second) = tokio::join!(
                writer.record_legacy_named_import_arm(&plan, &digest, arm.clone()),
                writer.record_legacy_named_import_arm(&plan, &digest, arm.clone()),
            );
            let recorded = match (first, second) {
                (Ok(recorded), Err(JournalError::Conflict))
                | (Err(JournalError::Conflict), Ok(recorded)) => recorded,
                _ => panic!("only one exact import Arm write may issue a ticket"),
            };
            let next = BrokerCheckpointV3::new(
                checkpoint(
                    latched
                        .clone()
                        .with_import_owned(&authority, owned.clone())?
                        .with_import_arm(arm.clone())?,
                )?,
                Vec::new(),
            )?;
            assert!(matches!(
                writer
                    .advance_lifecycle(operation, before_arm.head().clone(), next)
                    .await,
                Err(JournalError::Conflict)
            ));

            let pending_arm = writer.load(operation).await?.unwrap();
            assert_eq!(recorded.birth(), pending_arm.head());
            let submitted = recorded.request().clone();
            let mut stop_ipc = if record_claim {
                None
            } else {
                let fixture = LegacyIpcFixture::start(1, 0).await;
                let client = connect_named_legacy_fixture(&fixture).await?;
                let target = ProspectiveRegistrationV1::new(
                    hex::encode(Sha256::digest(scope.gateway_origin().as_bytes())),
                    scope.tenant_id().to_owned(),
                    submitted.arm.clone(),
                    authority.clone(),
                    owned.clone(),
                )?;
                let birth = NamedImportArmBirthV1::from_plan(&scope, &plan, recorded.birth())?;
                let generation = CancelGeneration::new();
                let expected = generation.current();
                let reserved = client
                    .legacy_reserve_named_prospective(target, &birth, &generation, expected)
                    .await
                    .map_err(|failure| {
                        anyhow::anyhow!("named IPC reservation failed: {failure:?}")
                    })?;
                Some((fixture, client, Some(reserved)))
            };
            if record_claim {
                assert!(matches!(
                    writer
                        .record_legacy_named_import_claim(&plan, claim.clone())
                        .await,
                    Err(JournalError::Conflict)
                ));
                assert_eq!(
                    writer.load(operation).await?.unwrap().head(),
                    pending_arm.head()
                );
                writer
                    .acknowledge_legacy_named_import_arm(operation, &submitted, arm_result.clone())
                    .await?;
                let mut different_attempt = claim.clone();
                different_attempt.claim.arm_receipt.attempt_id = Uuid::new_v4().to_string();
                assert!(matches!(
                    writer
                        .record_legacy_named_import_claim(&plan, different_attempt)
                        .await,
                    Err(JournalError::Conflict)
                ));
                let (first, second) = tokio::join!(
                    writer.record_legacy_named_import_claim(&plan, claim.clone()),
                    writer.record_legacy_named_import_claim(&plan, claim.clone()),
                );
                assert!(matches!(
                    (first, second),
                    (Ok(()), Err(JournalError::Conflict)) | (Err(JournalError::Conflict), Ok(()))
                ));
                let pending = writer.load_legacy_named_admission(operation).await?;
                let retained = pending.import_claim().unwrap();
                assert!(equal(retained.request(), &claim)?);
                assert!(retained.receipt().is_none());
                writer
                    .stops()
                    .append_cancel(CancelInputV1::new(operation, Uuid::new_v4())?)
                    .await
                    .map_err(|failure| failure.error())?;
                let stopped = writer.load(operation).await?.unwrap();
                let mut generic_receipt = claim_receipt.clone();
                generic_receipt.input_hash = claim.claim.input_hash()?;
                assert!(matches!(
                    writer
                        .acknowledge_legacy_named_import_claim(operation, &claim, generic_receipt)
                        .await,
                    Err(JournalError::Conflict)
                ));
                writer
                    .acknowledge_legacy_named_import_claim(operation, &claim, claim_receipt.clone())
                    .await?;
                let acknowledged = writer.load(operation).await?.unwrap();
                assert_eq!(
                    acknowledged.head().revision(),
                    stopped.head().revision() + 1
                );
                assert!(equal(
                    &acknowledged.broker_checkpoint_v3().unwrap().commands(),
                    &stopped.broker_checkpoint_v3().unwrap().commands(),
                )?);
                let retained = writer.load_legacy_named_admission(operation).await?;
                let retained = retained.import_claim().unwrap();
                assert!(equal(retained.request(), &claim)?);
                assert_eq!(retained.receipt(), Some(&claim_receipt));
                writer
                    .acknowledge_legacy_named_import_claim(operation, &claim, claim_receipt.clone())
                    .await?;
                let mut different_receipt = claim_receipt.clone();
                different_receipt.audit_hash = "d".repeat(64);
                assert!(matches!(
                    writer
                        .acknowledge_legacy_named_import_claim(operation, &claim, different_receipt)
                        .await,
                    Err(JournalError::Conflict)
                ));
                assert_eq!(
                    writer.load(operation).await?.unwrap().head(),
                    acknowledged.head()
                );
                continue;
            }
            let mut wrong = arm_result.clone();
            wrong.receipt.control_namespace_uuid = Uuid::new_v4().to_string();
            assert!(matches!(
                writer
                    .acknowledge_legacy_named_import_arm(operation, &submitted, wrong)
                    .await,
                Err(JournalError::Conflict)
            ));
            let mut different = arm_result.clone();
            different.receipt.audit_hash = "b".repeat(64);

            // Both ACKs read the pending Arm before STOP through the real FIFO.
            // Leave them unpolled until STOP has committed.
            let pause = writer.pause_for_test().await?;
            let acknowledgement = writer.acknowledge_legacy_named_import_arm(
                operation,
                &submitted,
                arm_result.clone(),
            );
            tokio::pin!(acknowledgement);
            assert!(futures_util::poll!(&mut acknowledgement).is_pending());
            let conflicting =
                writer.acknowledge_legacy_named_import_arm(operation, &submitted, different);
            tokio::pin!(conflicting);
            assert!(futures_util::poll!(&mut conflicting).is_pending());
            let stops = writer.stops();
            let stop_id = Uuid::new_v4();
            let cancellation = stops.append_cancel(CancelInputV1::new(operation, stop_id)?);
            tokio::pin!(cancellation);
            assert!(futures_util::poll!(&mut cancellation).is_pending());
            assert!(pause.is_paused());
            drop(pause);
            let stop = cancellation.await.map_err(|failure| failure.error())?;
            if let Some((_, client, _)) = &stop_ipc {
                client.deliver_named_import_stop(&stop).await?;
            }
            assert!(stop.incomplete());
            assert!(stop.named_reservation().is_none());
            let named_import = stop.named_import().expect("pending Arm predates STOP");
            assert_eq!(named_import.command_id(), stop_id);
            assert_eq!(named_import.arm_pending_head(), recorded.birth());
            assert!(
                named_import.arm_pending_head().revision() < named_import.stop_head().revision()
            );
            assert_eq!(named_import.admission().plan(), &plan);
            assert!(equal(named_import.evidence().owned(), &owned)?);
            assert!(equal(named_import.evidence().arm(), &submitted.arm)?);
            assert!(named_import.evidence().receipt().is_none());
            assert_eq!(
                named_import.evidence().handle_fingerprint(),
                authority.fingerprint()
            );
            assert_eq!(named_import.evidence().runtime_binding_hash().len(), 64);
            let stop_facts = serde_json::to_value(
                stop.named_import_stop_facts()?
                    .expect("verified STOP can be projected from the original lease"),
            )?;
            assert_eq!(stop_facts["arm_pending_head"], json!(recorded.birth()));
            assert_eq!(stop_facts["stop_head"], json!(stop.receipt().head()));
            assert_eq!(
                stop_facts["command_input_hash"],
                stop.receipt().input_hash()
            );
            assert!(stop.stops().is_empty());
            let snapshot = writer.stops().recovery_status(operation).await?.unwrap();
            assert!(snapshot.incomplete());
            assert_eq!(snapshot.pending_arms(), 1);
            let (retained, receipt) = writer
                .load_legacy_named_import_arm(operation)
                .await?
                .unwrap();
            assert!(equal(&retained, &submitted)?);
            assert!(receipt.is_none());
            if let Some((_, client, reserved)) = stop_ipc.as_mut() {
                let reserved = reserved.take().expect("one exact named reservation");
                let failure = match client
                    .legacy_begin_named_prospective_arm_exchange(reserved)
                    .await
                {
                    Err(failure) => failure,
                    Ok(_) => panic!("STOP latches the exact pending actor before Arm ACK"),
                };
                assert!(
                    matches!(failure, crate::broker::enforcer::LegacyNamedProspectiveFailure::Refused(refusal) if refusal.code() == crate::broker::enforcer::EnforcerError::NotReady)
                );
            }
            let stopped = writer.load(operation).await?.unwrap();
            assert_eq!(stopped.head().revision(), pending_arm.head().revision() + 1);
            acknowledgement.await?;
            assert!(matches!(conflicting.await, Err(JournalError::Conflict)));
            let replayed_stop = writer
                .stops()
                .cancel_stop_projection(operation, stop_id)
                .await?
                .unwrap();
            if let Some((_, client, _)) = &stop_ipc {
                client.deliver_named_import_stop(&replayed_stop).await?;
            }
            assert!(replayed_stop.incomplete());
            assert!(replayed_stop.named_reservation().is_none());
            let replayed_import = replayed_stop
                .named_import()
                .expect("Arm remains before STOP");
            assert_eq!(replayed_import.command_id(), stop_id);
            assert_eq!(
                replayed_import.arm_pending_head(),
                named_import.arm_pending_head()
            );
            assert_eq!(replayed_import.stop_head(), named_import.stop_head());
            assert_eq!(
                serde_json::to_value(replayed_stop.named_import_stop_facts()?.unwrap())?,
                stop_facts
            );
            let projected_receipt = replayed_import
                .evidence()
                .receipt()
                .expect("exact Arm receipt remains available after STOP");
            assert!(equal(projected_receipt, &arm_result.receipt)?);
            let snapshot = writer.stops().recovery_status(operation).await?.unwrap();
            assert!(snapshot.incomplete());
            assert_eq!(snapshot.pending_arms(), 0);
            let acknowledged = writer.load(operation).await?.unwrap();
            assert_eq!(
                acknowledged.head().revision(),
                stopped.head().revision() + 1
            );
            assert!(equal(
                &acknowledged.broker_checkpoint_v3().unwrap().commands(),
                &stopped.broker_checkpoint_v3().unwrap().commands(),
            )?);
            let (retained, receipt) = writer
                .load_legacy_named_import_arm(operation)
                .await?
                .unwrap();
            assert!(equal(&retained, &submitted)?);
            assert!(equal(&receipt.unwrap(), &arm_result.receipt)?);
            let mut replay = arm_result.clone();
            replay.replayed = false;
            writer
                .acknowledge_legacy_named_import_arm(operation, &retained, replay)
                .await?;
            assert_eq!(
                writer.load(operation).await?.unwrap().head(),
                acknowledged.head()
            );
            assert!(matches!(
                writer
                    .record_legacy_named_import_claim(&plan, claim.clone())
                    .await,
                Err(JournalError::Interrupted)
            ));
            let stopped_named = writer.load_legacy_named_admission(operation).await?;
            let next = BrokerCheckpointV3::new(
                checkpoint(stopped_named.with_import_claim(claim.clone())?)?,
                acknowledged
                    .broker_checkpoint_v3()
                    .unwrap()
                    .commands()
                    .to_vec(),
            )?;
            assert!(matches!(
                writer
                    .advance_lifecycle(operation, acknowledged.head().clone(), next)
                    .await,
                Err(JournalError::Conflict)
            ));
            if let Some((fixture, client, _)) = stop_ipc {
                drop(client);
                cold_stop_case = Some((namespace, fixture, stop_facts));
            }
        }

        // The original Writer and its projections have left scope. The broker
        // reacquires only the existing journal, while the native guardian stays.
        let (namespace, fixture, exact_facts) = cold_stop_case.expect("one live guardian fixture");
        let journal_dir = Directory::open_existing(&root.join(namespace))?;
        let report = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match StopRecoveryReaderV1::open_existing(
                    &journal_dir,
                    scope.clone(),
                    &FsExclusions::new(),
                    JournalLimitsV1::default(),
                    ControlSecrets::new(vec![])?,
                ) {
                    Ok(Some(reader)) => break Ok(reader.scan()),
                    Err(JournalError::Busy) => tokio::task::yield_now().await,
                    Ok(None) => break Err(JournalError::Conflict),
                    Err(error) => break Err(error),
                }
            }
        })
        .await??;
        assert_eq!(report.named_import_stop_jobs().collect::<Vec<_>>(), [0]);
        let recovered = report.named_import_stop_projection(0)?.unwrap();
        assert!(recovered.incomplete());
        assert_eq!(
            serde_json::to_value(recovered.named_import_stop_facts()?.unwrap())?,
            exact_facts
        );
        deliver_named_import_stop_from_legacy_root(fixture.root(), &recovered).await?;
        fixture.finish().await;

        // Persist exact Import observations independently of the native
        // guardian. Exercise write-before-send, Started -> Terminal, exact
        // replay, divergence refusal, and ACK CAS after a concurrent STOP.
        let observation_writer = start_admitted_writer("import-observation-race").await?;
        let creation_revision = seed_named_import_writer_through_claim(
            &observation_writer,
            &plan,
            &digest,
            helper_id,
            &authority,
            &owned,
            &arm,
            &arm_result,
            &claim,
            &claim_receipt,
        )
        .await?;
        assert!(
            observation_writer
                .load(operation)
                .await?
                .unwrap()
                .head()
                .revision()
                > creation_revision
        );

        let now = chrono::Utc::now();
        let started_at = (now - chrono::Duration::seconds(2)).to_rfc3339();
        let started_seen = (now - chrono::Duration::seconds(1)).to_rfc3339();
        let terminal_seen = (now + chrono::Duration::seconds(2)).to_rfc3339();
        let finished_at = (now + chrono::Duration::seconds(1)).to_rfc3339();
        let started_request = import_observation_fixture(
            &plan,
            &claim,
            &claim_receipt,
            &arm_result,
            LegacyNamedImportObservationPhaseV1::Started,
            &started_at,
            &started_seen,
            None,
        );
        let terminal_request = import_observation_fixture(
            &plan,
            &claim,
            &claim_receipt,
            &arm_result,
            LegacyNamedImportObservationPhaseV1::Terminal,
            &started_at,
            &terminal_seen,
            Some(&finished_at),
        );
        let observation_base = accepted
            .clone()
            .with_import_creation(helper_id, &authority)?
            .with_import_owned(&authority, owned.clone())?
            .with_import_arm(arm.clone())?
            .acknowledge_import_arm(arm_result.clone())?
            .with_import_claim(claim.clone())?
            .acknowledge_import_claim(claim_receipt.clone())?;
        let mut created_started = started_request.clone();
        created_started.observation.observation.status = RuntimeStatusV1::Created;
        created_started.observation.observation.running = Some(false);
        assert!(matches!(
            observation_base
                .clone()
                .with_import_observation(created_started),
            Err(JournalError::Conflict)
        ));
        let started_pending = observation_writer
            .record_legacy_named_import_observation(&plan, started_request.clone())
            .await?;
        assert!(started_pending.receipt().is_none());
        assert!(equal(started_pending.request(), &started_request)?);
        let head_with_started_pending = observation_writer
            .load(operation)
            .await?
            .unwrap()
            .head()
            .clone();
        assert!(matches!(
            observation_writer
                .record_legacy_named_import_observation(&plan, terminal_request.clone())
                .await,
            Err(JournalError::Conflict)
        ));
        assert_eq!(
            observation_writer.load(operation).await?.unwrap().head(),
            &head_with_started_pending
        );
        let head_before_divergence = observation_writer
            .load(operation)
            .await?
            .unwrap()
            .head()
            .clone();
        let mut divergent_started = started_request.clone();
        divergent_started.observation.observation.observed_at =
            (now + chrono::Duration::seconds(1)).to_rfc3339();
        assert!(matches!(
            observation_writer
                .record_legacy_named_import_observation(&plan, divergent_started)
                .await,
            Err(JournalError::Conflict)
        ));
        assert_eq!(
            observation_writer.load(operation).await?.unwrap().head(),
            &head_before_divergence
        );

        let started_receipt = MutationReceiptV1 {
            operation_id: started_request.observation.operation_id.clone(),
            worker_id: Some(arm.worker_id.clone()),
            revision: None,
            input_hash: started_request.input_hash()?,
            audit_seq: claim_receipt.audit_seq + 1,
            audit_hash: "d".repeat(64),
        };
        {
            let pause = observation_writer.pause_for_test().await?;
            let acknowledgement = observation_writer.acknowledge_legacy_named_import_observation(
                operation,
                &started_request,
                started_receipt.clone(),
            );
            tokio::pin!(acknowledgement);
            assert!(futures_util::poll!(&mut acknowledgement).is_pending());
            let stops = observation_writer.stops();
            let stop_id = Uuid::new_v4();
            let cancellation = stops.append_cancel(CancelInputV1::new(operation, stop_id)?);
            tokio::pin!(cancellation);
            assert!(futures_util::poll!(&mut cancellation).is_pending());
            assert!(pause.is_paused());
            drop(pause);
            let stopped = cancellation.await.map_err(|failure| failure.error())?;
            assert!(stopped.incomplete());
            let stopped_job = observation_writer.load(operation).await?.unwrap();
            acknowledgement.await?;
            let after_started_ack = observation_writer.load(operation).await?.unwrap();
            assert_eq!(
                after_started_ack
                    .broker_checkpoint_v2()
                    .unwrap()
                    .supervision()
                    .legacy_named_admission()
                    .unwrap()
                    .import_observation(LegacyNamedImportObservationPhaseV1::Started)
                    .unwrap()
                    .1,
                Some(&started_receipt)
            );
            assert!(equal(
                &after_started_ack.broker_checkpoint_v3().unwrap().commands(),
                &stopped_job.broker_checkpoint_v3().unwrap().commands()
            )?);
        }

        let terminal_first = observation_base
            .clone()
            .with_import_observation(terminal_request.clone())?;
        assert!(
            terminal_first
                .import_observation(LegacyNamedImportObservationPhaseV1::Terminal)
                .is_some()
        );
        assert!(matches!(
            terminal_first.import_verification_permission(),
            Err(JournalError::Conflict)
        ));
        assert!(matches!(
            terminal_first
                .clone()
                .with_import_verification_plan(Uuid::new_v4()),
            Err(JournalError::Conflict)
        ));
        assert!(matches!(
            terminal_first.with_import_observation(started_request.clone()),
            Err(JournalError::Conflict)
        ));
        let terminal_receipt = MutationReceiptV1 {
            operation_id: terminal_request.observation.operation_id.clone(),
            worker_id: Some(arm.worker_id.clone()),
            revision: None,
            input_hash: terminal_request.input_hash()?,
            audit_seq: claim_receipt.audit_seq + 2,
            audit_hash: "e".repeat(64),
        };
        let verified_prefix = observation_base
            .clone()
            .with_import_observation(terminal_request.clone())?
            .acknowledge_import_observation(&terminal_request, terminal_receipt.clone())?;
        let reopened: LegacyNamedAdmissionCheckpoint =
            serde_json::from_slice(&canonical(&verified_prefix)?)?;
        assert!(equal(
            &reopened.import_verification_permission()?,
            &HelperPermissionV1::ImportVerification {
                import_helper_id: terminal_request.observation.helper_id.clone(),
                import_arm: arm_result.receipt.clone(),
                import_terminal: terminal_receipt.clone(),
                manifest_digest: accepted
                    .request()
                    .admission
                    .admission
                    .launch
                    .workspace_manifest_digest
                    .clone(),
            }
        )?);
        let verification_helper_id = Uuid::new_v4();
        let verification_plan = verified_prefix
            .clone()
            .with_import_verification_plan(verification_helper_id)?;
        let retained_plan = verification_plan
            .import_verification_plan()
            .ok_or(JournalError::Conflict)?;
        assert_eq!(
            retained_plan.preparation().policy().purpose,
            HelperPurposeV1::ImportVerification
        );
        assert!(retained_plan.preparation().owned().is_none());
        assert_eq!(retained_plan.parent_revision(), arm.expected_revision);
        assert!(equal(
            retained_plan.preparation().permission(),
            &verified_prefix.import_verification_permission()?
        )?);
        let verification_bytes = canonical(&verification_plan)?;
        let reopened_plan: LegacyNamedAdmissionCheckpoint =
            serde_json::from_slice(&verification_bytes)?;
        assert_eq!(canonical(&reopened_plan)?, verification_bytes);
        assert!(equal(
            reopened_plan.import_verification_plan().unwrap(),
            retained_plan
        )?);
        assert!(equal(
            &verification_plan
                .clone()
                .with_import_verification_plan(verification_helper_id)?,
            &verification_plan
        )?);
        assert!(matches!(
            verification_plan
                .clone()
                .with_import_verification_plan(Uuid::new_v4()),
            Err(JournalError::Conflict)
        ));
        verification_plan.validate_transition(Some(&verified_prefix))?;
        assert!(matches!(
            verification_plan.validate_transition(Some(&observation_base)),
            Err(JournalError::Conflict)
        ));
        let mut failed = terminal_request.clone();
        failed.observation.observation.exit_code = Some(1);
        let failed_receipt = MutationReceiptV1 {
            input_hash: failed.input_hash()?,
            ..terminal_receipt.clone()
        };
        let failed_prefix = observation_base
            .clone()
            .with_import_observation(failed.clone())?
            .acknowledge_import_observation(&failed, failed_receipt)?;
        assert!(matches!(
            failed_prefix.import_verification_permission(),
            Err(JournalError::Conflict)
        ));
        assert!(matches!(
            failed_prefix.with_import_verification_plan(Uuid::new_v4()),
            Err(JournalError::Conflict)
        ));
        let terminal_pending = observation_writer
            .record_legacy_named_import_observation(&plan, terminal_request.clone())
            .await?;
        assert!(terminal_pending.receipt().is_none());
        assert!(equal(terminal_pending.request(), &terminal_request)?);
        observation_writer
            .acknowledge_legacy_named_import_observation(
                operation,
                &terminal_request,
                terminal_receipt.clone(),
            )
            .await?;
        let replay = observation_writer
            .record_legacy_named_import_observation(&plan, started_request.clone())
            .await?;
        assert_eq!(replay.receipt(), Some(&started_receipt));
        observation_writer
            .acknowledge_legacy_named_import_observation(
                operation,
                &terminal_request,
                terminal_receipt.clone(),
            )
            .await?;
        let mut different_ack = terminal_receipt.clone();
        different_ack.audit_hash = "f".repeat(64);
        assert!(matches!(
            observation_writer
                .acknowledge_legacy_named_import_observation(
                    operation,
                    &terminal_request,
                    different_ack,
                )
                .await,
            Err(JournalError::Conflict)
        ));
        let retained = observation_writer
            .load_legacy_named_import_observation(
                operation,
                LegacyNamedImportObservationPhaseV1::Terminal,
            )
            .await?
            .unwrap();
        assert!(equal(retained.request(), &terminal_request)?);
        assert_eq!(retained.receipt(), Some(&terminal_receipt));
        assert!(matches!(
            observation_writer
                .record_legacy_named_import_verification_plan(&plan, Uuid::new_v4())
                .await,
            Err(JournalError::Interrupted)
        ));
        let terminal_ack_head = observation_writer
            .load(operation)
            .await?
            .unwrap()
            .head()
            .clone();
        // Drop every live Writer handle, reacquire the same journal, and
        // reconcile the exact retained Terminal bytes after the process-local
        // owner is gone. This proves Writer recovery/replay only; the Service
        // cold route and native runtime inspection remain outside this oracle.
        drop(observation_writer);
        let cold_head = reopen_and_replay_import_observation(
            &root,
            "import-observation-race",
            &scope,
            &plan,
            &terminal_request,
            &terminal_receipt,
        )
        .await?;
        assert_eq!(cold_head, terminal_ack_head);

        // A second real Writer gives the verification intent a clean
        // post-Terminal, pre-STOP insertion point. The exact plan remains
        // replayable after STOP and cold reopen, while a new helper id cannot
        // replace it.
        let verification_writer = start_admitted_writer("import-verification-plan").await?;
        seed_named_import_writer_through_claim(
            &verification_writer,
            &plan,
            &digest,
            helper_id,
            &authority,
            &owned,
            &arm,
            &arm_result,
            &claim,
            &claim_receipt,
        )
        .await?;
        let terminal_pending = verification_writer
            .record_legacy_named_import_observation(&plan, terminal_request.clone())
            .await?;
        assert!(terminal_pending.receipt().is_none());
        verification_writer
            .acknowledge_legacy_named_import_observation(
                operation,
                &terminal_request,
                terminal_receipt.clone(),
            )
            .await?;
        let plan_base = verification_writer
            .load(operation)
            .await?
            .ok_or(JournalError::Conflict)?;
        let plan_base_lifecycle = plan_base
            .broker_checkpoint_v3()
            .ok_or(JournalError::Conflict)?;
        let plan_base_head = plan_base.head().clone();
        let pending_plan = plan_base
            .broker_checkpoint_v2()
            .and_then(|checkpoint| checkpoint.supervision().legacy_named_admission())
            .ok_or(JournalError::Conflict)?
            .clone()
            .with_import_verification_plan(verification_helper_id)?;
        let pending_supervision = plan_base_lifecycle
            .broker()
            .supervision()
            .clone()
            .with_legacy_named_admission(pending_plan)?;
        let pending_broker = BrokerCheckpointV2::new(
            plan_base_lifecycle.broker().runner().clone(),
            pending_supervision,
        )?;
        let exact_pending_lifecycle =
            BrokerCheckpointV3::new(pending_broker, plan_base_lifecycle.commands().to_vec())?;
        // Exercise concurrent callers without depending on which one the
        // executor polls first. A direct stale write below proves the exact
        // same V3 candidate cannot publish twice from this saved head.
        let (inserted, concurrent) = tokio::join!(
            verification_writer
                .record_legacy_named_import_verification_plan(&plan, verification_helper_id),
            verification_writer
                .record_legacy_named_import_verification_plan(&plan, verification_helper_id),
        );
        let inserted = inserted?;
        let concurrent = concurrent?;
        assert_ne!(inserted.inserted(), concurrent.inserted());
        let (inserted, concurrent) = if inserted.inserted() {
            (&inserted, &concurrent)
        } else {
            (&concurrent, &inserted)
        };
        assert!(inserted.inserted());
        assert!(!concurrent.inserted());
        assert!(equal(
            inserted.preparation().permission(),
            &verified_prefix.import_verification_permission()?
        )?);
        assert_eq!(inserted.parent_revision(), arm.expected_revision);
        let committed_lifecycle = verification_writer
            .load(operation)
            .await?
            .and_then(|job| job.broker_checkpoint_v3().cloned())
            .ok_or(JournalError::Conflict)?;
        assert_eq!(
            committed_lifecycle.canonical_bytes()?,
            exact_pending_lifecycle.canonical_bytes()?
        );
        assert!(matches!(
            verification_writer
                .advance_lifecycle(operation, plan_base_head, exact_pending_lifecycle.clone())
                .await,
            Err(JournalError::Conflict)
        ));
        let planned_head = verification_writer
            .load(operation)
            .await?
            .unwrap()
            .head()
            .clone();
        assert!(equal(concurrent.preparation(), inserted.preparation())?);
        assert_eq!(
            verification_writer.load(operation).await?.unwrap().head(),
            &planned_head
        );
        let create_base = verification_writer
            .load(operation)
            .await?
            .ok_or(JournalError::Conflict)?;
        let create_base_lifecycle = create_base
            .broker_checkpoint_v3()
            .ok_or(JournalError::Conflict)?;
        let create_base_head = create_base.head().clone();
        let named_create = create_base
            .broker_checkpoint_v2()
            .and_then(|checkpoint| checkpoint.supervision().legacy_named_admission())
            .ok_or(JournalError::Conflict)?
            .clone()
            .with_import_verification_create(terminal_receipt.clone(), &authority)?;
        let create_supervision = create_base_lifecycle
            .broker()
            .supervision()
            .clone()
            .with_legacy_named_admission(named_create)?;
        let create_broker = BrokerCheckpointV2::new(
            create_base_lifecycle.broker().runner().clone(),
            create_supervision,
        )?;
        let exact_pending_create =
            BrokerCheckpointV3::new(create_broker, create_base_lifecycle.commands().to_vec())?;
        let (create_one, create_two) = tokio::join!(
            verification_writer.record_legacy_named_import_verification_create(
                &plan,
                &terminal_receipt,
                &authority,
            ),
            verification_writer.record_legacy_named_import_verification_create(
                &plan,
                &terminal_receipt,
                &authority,
            ),
        );
        let create_one = create_one?;
        let create_two = create_two?;
        assert_ne!(create_one.is_some(), create_two.is_some());
        let create_token = create_one.or(create_two).ok_or(JournalError::Conflict)?;
        let committed_create_lifecycle = verification_writer
            .load(operation)
            .await?
            .and_then(|job| job.broker_checkpoint_v3().cloned())
            .ok_or(JournalError::Conflict)?;
        assert_eq!(
            committed_create_lifecycle.canonical_bytes()?,
            exact_pending_create.canonical_bytes()?
        );
        assert_eq!(create_token.operation_id(), operation);
        assert_eq!(
            create_token.confirmed_head(),
            committed_create_lifecycle.head()
        );
        assert_eq!(create_token.facts_hash().len(), 64);
        assert!(matches!(
            verification_writer
                .advance_lifecycle(operation, create_base_head, exact_pending_create)
                .await,
            Err(JournalError::Conflict)
        ));
        let create_head = verification_writer
            .load(operation)
            .await?
            .unwrap()
            .head()
            .clone();
        assert_eq!(
            verification_writer.load(operation).await?.unwrap().head(),
            &create_head
        );
        let verification_stops = verification_writer.stops();
        verification_stops
            .append_cancel(CancelInputV1::new(operation, Uuid::new_v4())?)
            .await
            .map_err(|failure| failure.error())?;
        assert!(matches!(
            verification_writer
                .record_legacy_named_import_verification_plan(&plan, Uuid::new_v4())
                .await,
            Err(JournalError::Conflict)
        ));
        let stopped_plan = verification_writer
            .load_legacy_named_import_verification_plan(operation)
            .await?
            .ok_or(JournalError::Conflict)?;
        assert!(equal(
            &stopped_plan,
            verification_plan.import_verification_plan().unwrap()
        )?);
        assert!(
            verification_writer
                .record_legacy_named_import_verification_create(
                    &plan,
                    &terminal_receipt,
                    &authority,
                )
                .await?
                .is_none()
        );
        let mut divergent_terminal = terminal_receipt.clone();
        divergent_terminal.audit_hash = "e".repeat(64);
        assert!(matches!(
            verification_writer
                .record_legacy_named_import_verification_create(
                    &plan,
                    &divergent_terminal,
                    &authority,
                )
                .await,
            Err(JournalError::Conflict)
        ));
        let mut divergent_binding = authority.clone();
        divergent_binding.endpoint.push_str("-different");
        assert!(matches!(
            verification_writer
                .record_legacy_named_import_verification_create(
                    &plan,
                    &terminal_receipt,
                    &divergent_binding,
                )
                .await,
            Err(JournalError::Conflict)
        ));
        let stopped_head = verification_writer
            .load(operation)
            .await?
            .unwrap()
            .head()
            .clone();
        drop(verification_stops);
        drop(verification_writer);
        let journal_dir = Directory::open_existing(&root.join("import-verification-plan"))?;
        let journal = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match Journal::open(
                    &journal_dir,
                    scope.clone(),
                    &FsExclusions::new(),
                    JournalLimitsV1::default(),
                    ControlSecrets::new(vec![])?,
                ) {
                    Ok(journal) => break Ok(journal),
                    Err(JournalError::Busy) => tokio::task::yield_now().await,
                    Err(error) => break Err(error),
                }
            }
        })
        .await??;
        let cold_verification_writer = WriterClient::start(journal, 8, Duration::from_secs(5))?;
        let cold_plan = cold_verification_writer
            .load_legacy_named_import_verification_plan(operation)
            .await?
            .ok_or(JournalError::Conflict)?;
        assert!(equal(
            &cold_plan,
            verification_plan.import_verification_plan().unwrap()
        )?);
        let cold_replay = cold_verification_writer
            .record_legacy_named_import_verification_plan(&plan, verification_helper_id)
            .await?;
        assert!(!cold_replay.inserted());
        assert_eq!(
            cold_verification_writer
                .load(operation)
                .await?
                .unwrap()
                .head(),
            &stopped_head
        );
        assert!(
            cold_verification_writer
                .record_legacy_named_import_verification_create(
                    &plan,
                    &terminal_receipt,
                    &authority,
                )
                .await?
                .is_none()
        );
        drop(cold_verification_writer);

        // The lifecycle validator must independently fence adding this new
        // stage after a durable STOP, even if a caller bypasses WriterClient's
        // open-lifecycle check and supplies an otherwise valid named plan.
        let stopped_plan_writer = start_admitted_writer("verification-stop-transition").await?;
        seed_named_import_writer_through_claim(
            &stopped_plan_writer,
            &plan,
            &digest,
            helper_id,
            &authority,
            &owned,
            &arm,
            &arm_result,
            &claim,
            &claim_receipt,
        )
        .await?;
        stopped_plan_writer
            .record_legacy_named_import_observation(&plan, terminal_request.clone())
            .await?;
        stopped_plan_writer
            .acknowledge_legacy_named_import_observation(
                operation,
                &terminal_request,
                terminal_receipt.clone(),
            )
            .await?;
        let stopped_plan = stopped_plan_writer
            .record_legacy_named_import_verification_plan(&plan, verification_helper_id)
            .await?;
        assert!(stopped_plan.inserted());
        let before_stop = stopped_plan_writer
            .load(operation)
            .await?
            .ok_or(JournalError::Conflict)?;
        let before_lifecycle = before_stop
            .broker_checkpoint_v3()
            .ok_or(JournalError::Conflict)?;
        stopped_plan_writer
            .stops()
            .append_cancel(CancelInputV1::new(operation, Uuid::new_v4())?)
            .await
            .map_err(|failure| failure.error())?;
        assert!(matches!(
            stopped_plan_writer
                .record_legacy_named_import_verification_create(
                    &plan,
                    &terminal_receipt,
                    &authority,
                )
                .await,
            Err(JournalError::Interrupted)
        ));
        let after_stop = stopped_plan_writer
            .load(operation)
            .await?
            .ok_or(JournalError::Conflict)?;
        let after_stop_lifecycle = after_stop
            .broker_checkpoint_v3()
            .ok_or(JournalError::Conflict)?;
        let stopped_named = after_stop
            .broker_checkpoint_v2()
            .and_then(|checkpoint| checkpoint.supervision().legacy_named_admission())
            .ok_or(JournalError::Conflict)?
            .clone()
            .with_import_verification_plan(verification_helper_id)?;
        let forged_supervision = after_stop_lifecycle
            .broker()
            .supervision()
            .clone()
            .with_legacy_named_admission(
                stopped_named
                    .with_import_verification_create(terminal_receipt.clone(), &authority)?,
            )?;
        let forged_broker = BrokerCheckpointV2::new(
            after_stop_lifecycle.broker().runner().clone(),
            forged_supervision,
        )?;
        let forged_after_stop =
            BrokerCheckpointV3::new(forged_broker, after_stop_lifecycle.commands().to_vec())?;
        assert!(
            forged_after_stop
                .validate_transition(
                    Some(before_lifecycle),
                    Some(before_lifecycle.broker()),
                    before_stop.checkpoint(),
                    None,
                )
                .is_err()
        );
        let stopped_head = after_stop.head().clone();
        assert!(matches!(
            stopped_plan_writer
                .advance_lifecycle(operation, stopped_head.clone(), forged_after_stop)
                .await,
            Err(JournalError::Conflict)
        ));
        assert_eq!(
            stopped_plan_writer.load(operation).await?.unwrap().head(),
            &stopped_head
        );
        drop(stopped_plan_writer);

        let writer = start_admitted_writer("import-creation").await?;
        let before_creation = writer.load(operation).await?.unwrap();
        assert!(matches!(
            writer
                .record_legacy_named_import_creation(&plan, &"0".repeat(64), helper_id, &authority)
                .await,
            Err(JournalError::Conflict)
        ));
        let (first, second) = tokio::join!(
            writer.record_legacy_named_import_creation(&plan, &digest, helper_id, &authority),
            writer.record_legacy_named_import_creation(&plan, &digest, helper_id, &authority),
        );
        let recorded = match (first, second) {
            (Ok(recorded), Err(JournalError::Conflict))
            | (Err(JournalError::Conflict), Ok(recorded)) => recorded,
            _ => panic!("only one exact import creation write may return a first ticket"),
        };
        let creation_head = recorded.job().head();
        assert_eq!(
            creation_head.revision(),
            before_creation.head().revision() + 1
        );
        assert_eq!(writer.load(operation).await?.unwrap().head(), creation_head);
        let retained = writer.load_legacy_named_admission(operation).await?;
        let preparation = retained.import_creation().unwrap();
        assert!(equal(preparation.policy(), &helper)?);
        assert!(matches!(
            preparation.permission(),
            HelperPermissionV1::Admission { receipt } if receipt == &admission.admission.receipt
        ));
        assert!(preparation.owned().is_none());
        assert!(retained.import_arm().is_none());
        assert!(matches!(
            writer
                .advance_lifecycle(
                    operation,
                    before_creation.head().clone(),
                    BrokerCheckpointV3::new(checkpoint(latched)?, Vec::new())?,
                )
                .await,
            Err(JournalError::Conflict)
        ));
        let replacement = BrokerCheckpointV3::new(checkpoint(other_latch)?, Vec::new())?;
        assert!(matches!(
            writer
                .advance_lifecycle(operation, creation_head.clone(), replacement)
                .await,
            Err(JournalError::Conflict)
        ));
        assert_eq!(writer.load(operation).await?.unwrap().head(), creation_head);
        let mut invocation_changed = serde_json::to_value(&authority)?;
        invocation_changed["invocation"]["cwd"] = json!(root.join("different-cwd"));
        let invocation_changed: RuntimeAuthority = serde_json::from_value(invocation_changed)?;
        assert_eq!(invocation_changed.fingerprint(), authority.fingerprint());
        assert!(matches!(
            writer
                .acknowledge_legacy_named_import_creation(
                    &plan,
                    &digest,
                    &invocation_changed,
                    owned.clone(),
                )
                .await,
            Err(JournalError::Conflict)
        ));
        let mut running = owned.clone();
        running.observation.process.running = Some(true);
        assert!(matches!(
            writer
                .acknowledge_legacy_named_import_creation(&plan, &digest, &authority, running)
                .await,
            Err(JournalError::Conflict)
        ));
        let mut finished = owned.clone();
        finished.observation.process.finished_at =
            Some(finished.observation.observed_at.fixed_offset());
        assert!(matches!(
            writer
                .acknowledge_legacy_named_import_creation(&plan, &digest, &authority, finished)
                .await,
            Err(JournalError::Conflict)
        ));
        assert_eq!(writer.load(operation).await?.unwrap().head(), creation_head);
        let mut different = serde_json::to_value(&owned)?;
        different["handle"]["id"] = json!("a".repeat(64));
        different["observation"]["id"] = json!("a".repeat(64));
        let different: OwnedContainerSnapshot = serde_json::from_value(different)?;

        // Queue both observation loads before STOP, then let STOP commit
        // before polling either acknowledgement again.
        let pause = writer.pause_for_test().await?;
        let acknowledgement = writer.acknowledge_legacy_named_import_creation(
            &plan,
            &digest,
            &authority,
            owned.clone(),
        );
        tokio::pin!(acknowledgement);
        assert!(futures_util::poll!(&mut acknowledgement).is_pending());
        let conflicting =
            writer.acknowledge_legacy_named_import_creation(&plan, &digest, &authority, different);
        tokio::pin!(conflicting);
        assert!(futures_util::poll!(&mut conflicting).is_pending());
        let stops = writer.stops();
        let cancellation = stops.append_cancel(CancelInputV1::new(operation, Uuid::new_v4())?);
        tokio::pin!(cancellation);
        assert!(futures_util::poll!(&mut cancellation).is_pending());
        assert!(pause.is_paused());
        drop(pause);
        let stop = cancellation.await.map_err(|failure| failure.error())?;
        assert!(stop.incomplete());
        assert!(stop.named_reservation().is_none());
        assert!(stop.named_import().is_none());
        let snapshot = stops.recovery_status(operation).await?.unwrap();
        assert!(snapshot.incomplete());
        assert_eq!(snapshot.pending_arms(), 0);
        let stopped_job = writer.load(operation).await?.unwrap();
        assert_eq!(stopped_job.head().revision(), creation_head.revision() + 1);
        acknowledgement.await?;
        assert!(matches!(conflicting.await, Err(JournalError::Conflict)));
        let owned_job = writer.load(operation).await?.unwrap();
        assert_eq!(
            owned_job.head().revision(),
            stopped_job.head().revision() + 1
        );
        assert!(equal(
            &owned_job.broker_checkpoint_v3().unwrap().commands(),
            &stopped_job.broker_checkpoint_v3().unwrap().commands(),
        )?);
        let retained = writer.load_legacy_named_admission(operation).await?;
        assert!(equal(
            retained.import_creation().unwrap().owned().unwrap(),
            &owned
        )?);
        assert!(retained.import_arm().is_none());
        let mut reinspected = owned.clone();
        reinspected.observation.observed_at += chrono::TimeDelta::seconds(1);
        writer
            .acknowledge_legacy_named_import_creation(&plan, &digest, &authority, reinspected)
            .await?;
        assert_eq!(
            writer.load(operation).await?.unwrap().head(),
            owned_job.head()
        );
        assert!(matches!(
            writer
                .record_legacy_named_import_arm(&plan, &digest, arm)
                .await,
            Err(JournalError::Interrupted)
        ));
        Ok(())
    }
}
