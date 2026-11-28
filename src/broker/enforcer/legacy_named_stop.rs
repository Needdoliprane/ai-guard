//! Compare verified journal STOP facts with one live named Import target.
use super::{
    CancelGeneration, EnforcerError, EnforcerResult, NamedImportPrepareV1,
    ProspectiveRegistrationV1, RegistrationV1, hex_hash,
};
use crate::{
    broker::{
        journal::{
            AcceptedJobV1, JournalScopeV1, RevisionRefV1, canonical,
            recovery::named_import_stop_owned_hash,
        },
        lifecycle::{LocalCommandRequestV1, LocalCommandV1},
        writer::RecordedLegacyNamedImportVerificationArm,
    },
    control::{
        ArmReceiptV1, ArmSubjectV1, HelperPermissionV1, HelperPurposeV1, content_hash,
        session_control::{
            LegacyConfirmedPlanV1, LegacyNamedImportArmRequestV1,
            LegacyNamedImportVerificationArmRequestV2, canonical_uuid,
        },
    },
};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// The immutable, journal-bound part of the first named reservation.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::broker) struct NamedImportArmBirthV1 {
    scope: JournalScopeV1,
    operation_id: Uuid,
    binding_ref: Uuid,
    session_id: Uuid,
    arm_pending_head: RevisionRefV1,
}

impl NamedImportArmBirthV1 {
    pub(in crate::broker) fn from_plan(
        scope: &JournalScopeV1,
        plan: &LegacyConfirmedPlanV1,
        arm_pending_head: &RevisionRefV1,
    ) -> EnforcerResult<Self> {
        plan.validate().map_err(|_| EnforcerError::InvalidInput)?;
        if scope.profile_id() != plan.broker_profile_id
            || scope.control_namespace_uuid().to_string() != plan.control_namespace_uuid
        {
            return Err(EnforcerError::Conflict);
        }
        Ok(Self {
            scope: scope.clone(),
            operation_id: plan.operation_id,
            binding_ref: plan.binding_ref,
            session_id: plan.session_id,
            arm_pending_head: arm_pending_head.clone(),
        })
    }

    pub(super) fn matches(&self, other: &Self) -> EnforcerResult<bool> {
        let invalid = |_| EnforcerError::InvalidInput;
        Ok(canonical(self).map_err(invalid)? == canonical(other).map_err(invalid)?)
    }
}

/// Confirmed journal birth and immutable prospective target of the named
/// verification helper. Native custody and its deadline check are separate.
#[allow(
    dead_code,
    reason = "the verification prospective reservation is composed separately"
)]
pub(in crate::broker) struct NamedImportVerificationArmBirthV2 {
    recorded: Box<RecordedLegacyNamedImportVerificationArm>,
    target: ProspectiveRegistrationV1,
    prospective_hash: String,
}

#[allow(
    dead_code,
    reason = "the verification prospective reservation is composed separately"
)]
impl NamedImportVerificationArmBirthV2 {
    pub(in crate::broker) fn from_recorded(
        recorded: Box<RecordedLegacyNamedImportVerificationArm>,
        target: ProspectiveRegistrationV1,
    ) -> EnforcerResult<Self> {
        let invalid = |_| EnforcerError::InvalidInput;
        let job = recorded.job();
        let scope = job.scope();
        let plan = job
            .intent()
            .legacy_named_plan()
            .ok_or(EnforcerError::Conflict)?;
        let checkpoint = job.broker_checkpoint_v3().ok_or(EnforcerError::Conflict)?;
        let named = checkpoint
            .broker()
            .supervision()
            .legacy_named_admission()
            .ok_or(EnforcerError::Conflict)?;
        let request = named
            .import_verification_arm_request()
            .ok_or(EnforcerError::Conflict)?;
        let facts = named
            .import_verification_arm_facts()
            .map_err(|_| EnforcerError::Conflict)?
            .ok_or(EnforcerError::Conflict)?;
        facts
            .validate_request(request, target.authority())
            .map_err(|_| EnforcerError::Conflict)?;
        let prospective_hash = target.digest()?;
        if job.head().revision() == 0
            || job.operation_id() != plan.operation_id
            || facts.plan() != plan
            || request.arm.intent_hash != job.intent_hash()
            || scope.profile_id() != plan.broker_profile_id
            || scope.control_namespace_uuid().to_string() != request.namespace
            || target.tenant_id() != scope.tenant_id()
            || target.control_origin_hash()
                != hex::encode(Sha256::digest(scope.gateway_origin().as_bytes()))
            || target.owned().handle.session_id() != plan.session_id
            || canonical(target.arm()).map_err(invalid)?
                != canonical(&request.arm).map_err(invalid)?
            || canonical(target.owned()).map_err(invalid)?
                != canonical(facts.owned()).map_err(invalid)?
            || checkpoint
                .commands()
                .iter()
                .any(|command| matches!(command.request(), LocalCommandRequestV1::CancelRequested))
        {
            return Err(EnforcerError::Conflict);
        }
        let (cancel, generation) = recorded.generation();
        cancel.with_current_generation(generation, || ())?;
        Ok(Self {
            recorded,
            target,
            prospective_hash,
        })
    }

    pub(in crate::broker) fn job(&self) -> &AcceptedJobV1 {
        self.recorded.job()
    }

    pub(in crate::broker) fn generation(&self) -> (&CancelGeneration, u64) {
        self.recorded.generation()
    }

    pub(in crate::broker) fn target(&self) -> &ProspectiveRegistrationV1 {
        &self.target
    }

    pub(in crate::broker) fn prospective_hash(&self) -> &str {
        &self.prospective_hash
    }

    /// Project the exact wire facts from the still-owned Writer birth.
    pub(super) fn prospective_facts(
        &self,
    ) -> EnforcerResult<NamedImportVerificationProspectiveFactsV2> {
        let job = self.job();
        let named = job
            .broker_checkpoint_v3()
            .and_then(|checkpoint| checkpoint.broker().supervision().legacy_named_admission())
            .ok_or(EnforcerError::Conflict)?;
        let request = named
            .import_verification_arm_request()
            .ok_or(EnforcerError::Conflict)?;
        let facts = named
            .import_verification_arm_facts()
            .map_err(|_| EnforcerError::Conflict)?
            .ok_or(EnforcerError::Conflict)?;
        let data = NamedImportVerificationProspectiveFactsV2 {
            version: 2,
            scope: job.scope().clone(),
            operation_id: job.operation_id(),
            arm_pending_head: job.head().clone(),
            session_id: facts.plan().session_id,
            helper_id: facts.owned().handle.operation_id(),
            request: request.clone(),
            create_facts_hash: facts.create_facts_hash().to_owned(),
            owned_snapshot_hash: named_import_stop_owned_hash(facts.owned())
                .map_err(|_| EnforcerError::InvalidInput)?,
            runtime_binding_hash: facts.runtime_binding_hash().to_owned(),
            prospective_hash: self.prospective_hash.clone(),
        };
        data.validate_target(job.scope(), &self.target)?;
        Ok(data)
    }

    #[cfg(test)]
    pub(super) fn into_prospective_facts(
        self,
    ) -> EnforcerResult<(
        NamedImportVerificationProspectiveFactsV2,
        ProspectiveRegistrationV1,
        (CancelGeneration, u64),
    )> {
        let data = self.prospective_facts()?;
        let (cancel, generation) = self.recorded.generation();
        Ok((data, self.target, (cancel.clone(), generation)))
    }
}

/// Private data for checking one verification target. The authenticated
/// sender or a retained latch establishes the journal provenance of the head
/// and of the Create facts digest.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NamedImportVerificationProspectiveFactsV2 {
    version: u32,
    scope: JournalScopeV1,
    #[serde(deserialize_with = "deserialize_non_nil_uuid")]
    operation_id: Uuid,
    arm_pending_head: RevisionRefV1,
    #[serde(deserialize_with = "deserialize_non_nil_uuid")]
    session_id: Uuid,
    #[serde(deserialize_with = "deserialize_non_nil_uuid")]
    helper_id: Uuid,
    request: LegacyNamedImportVerificationArmRequestV2,
    create_facts_hash: String,
    owned_snapshot_hash: String,
    runtime_binding_hash: String,
    prospective_hash: String,
}

impl NamedImportVerificationProspectiveFactsV2 {
    pub(super) fn import_predecessor(&self) -> EnforcerResult<(&str, &ArmReceiptV1)> {
        let ArmSubjectV1::Helper {
            purpose: HelperPurposeV1::ImportVerification,
            permission,
            ..
        } = &self.request.arm.subject
        else {
            return Err(EnforcerError::InvalidInput);
        };
        let HelperPermissionV1::ImportVerification {
            import_helper_id,
            import_arm,
            ..
        } = permission.as_ref()
        else {
            return Err(EnforcerError::InvalidInput);
        };
        Ok((import_helper_id, import_arm))
    }

    /// Compare the decoded data with the exact target. The caller supplies
    /// the expected scope from authenticated or locally retained context.
    pub(super) fn validate_target(
        &self,
        expected_scope: &JournalScopeV1,
        target: &ProspectiveRegistrationV1,
    ) -> EnforcerResult<()> {
        if self.version != 2
            || self.operation_id.is_nil()
            || self.session_id.is_nil()
            || self.helper_id.is_nil()
            || self.arm_pending_head.revision() == 0
            || !hex_hash(self.arm_pending_head.hash())
            || !hex_hash(&self.create_facts_hash)
        {
            return Err(EnforcerError::InvalidInput);
        }
        self.request
            .validate()
            .map_err(|_| EnforcerError::InvalidInput)?;
        let prospective_hash = target.digest()?;
        let owned = target.owned();
        let owned_snapshot_hash =
            named_import_stop_owned_hash(owned).map_err(|_| EnforcerError::InvalidInput)?;
        let runtime_binding_hash = content_hash(
            "ai-guard:legacy-named-import-runtime-binding:v1",
            target.authority(),
        )
        .map_err(|_| EnforcerError::InvalidInput)?;
        let process = &owned.observation.process;
        let invalid = |_| EnforcerError::InvalidInput;
        if canonical(&self.scope).map_err(invalid)? != canonical(expected_scope).map_err(invalid)?
            || self.request.namespace != self.scope.control_namespace_uuid().to_string()
            || self.operation_id == self.helper_id
            || self.request.arm.operation_id == self.operation_id.to_string()
            || self.request.arm.operation_id == self.helper_id.to_string()
            || target.tenant_id() != self.scope.tenant_id()
            || target.control_origin_hash()
                != hex::encode(Sha256::digest(self.scope.gateway_origin().as_bytes()))
            || owned.handle.session_id() != self.session_id
            || owned.handle.operation_id() != self.helper_id
            || canonical(&self.request.arm).map_err(invalid)?
                != canonical(target.arm()).map_err(invalid)?
            || self.prospective_hash != prospective_hash
            || self.owned_snapshot_hash != owned_snapshot_hash
            || self.runtime_binding_hash != runtime_binding_hash
            || process.running != Some(false)
            || !matches!(
                process.status.as_deref(),
                Some("created" | "configured" | "initialized")
            )
            || process.started_at.is_some()
            || process.finished_at.is_some()
            || !matches!(process.exit_code, None | Some(0))
            || process.oom_killed == Some(true)
        {
            return Err(EnforcerError::Conflict);
        }
        Ok(())
    }
}

/// Context fixed at the first named Reserve from its confirmed Writer birth.
/// This value is private to the living guardian and cannot be decoded.
pub(super) struct NamedImportStopAnchorV1 {
    birth: NamedImportArmBirthV1,
    prospective_hash: String,
    arm_input_hash: String,
    owned_snapshot_hash: String,
    runtime_binding_hash: String,
}

/// Receiver for the field-for-field projection from the verified STOP reader.
/// Decoding carries no authority or proof of journal ancestry by itself.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NamedImportStopFactsV1 {
    version: u32,
    scope: JournalScopeV1,
    #[serde(deserialize_with = "deserialize_non_nil_uuid")]
    operation_id: Uuid,
    #[serde(deserialize_with = "deserialize_non_nil_uuid")]
    binding_ref: Uuid,
    arm_pending_head: RevisionRefV1,
    #[serde(deserialize_with = "deserialize_non_nil_uuid")]
    command_id: Uuid,
    command_input_hash: String,
    stop_head: RevisionRefV1,
    arm_id: String,
    arm_input_hash: String,
    owned_snapshot_hash: String,
    runtime_binding_hash: String,
}

impl NamedImportStopFactsV1 {
    pub(super) fn arm_id(&self) -> &str {
        &self.arm_id
    }
}

impl NamedImportStopAnchorV1 {
    pub(super) fn new(
        birth: NamedImportArmBirthV1,
        target: &ProspectiveRegistrationV1,
    ) -> EnforcerResult<Self> {
        let invalid = |_| EnforcerError::InvalidInput;
        if birth.operation_id.is_nil()
            || birth.binding_ref.is_nil()
            || birth.session_id.is_nil()
            || birth.arm_pending_head.revision() == 0
        {
            return Err(EnforcerError::InvalidInput);
        }
        let prospective_hash = target.digest()?;
        let arm = target.arm();
        let attempt = canonical_uuid(&arm.attempt_id).map_err(invalid)?;
        let ArmSubjectV1::Helper {
            purpose: HelperPurposeV1::Import,
            permission,
            ..
        } = &arm.subject
        else {
            return Err(EnforcerError::InvalidInput);
        };
        let HelperPermissionV1::Admission { receipt } = permission.as_ref() else {
            return Err(EnforcerError::InvalidInput);
        };
        if target.tenant_id() != birth.scope.tenant_id()
            || target.control_origin_hash()
                != hex::encode(Sha256::digest(birth.scope.gateway_origin().as_bytes()))
            || target.owned().handle.session_id() != birth.session_id
            || attempt.is_nil()
            || arm.operation_id
                != LegacyNamedImportArmRequestV1::operation_id(birth.binding_ref, attempt)
            || arm.operation_id == receipt.operation_id
            || receipt.operation_id != birth.operation_id.to_string()
            || receipt.worker_id.as_deref() != Some(arm.worker_id.as_str())
            || !receipt.revision.is_some_and(|revision| {
                revision > 0 && revision.checked_add(1) == Some(arm.expected_revision)
            })
            || receipt.audit_seq <= 0
            || !hex_hash(&receipt.audit_hash)
            || !hex_hash(&receipt.input_hash)
        {
            return Err(EnforcerError::Conflict);
        }
        Ok(Self {
            birth,
            prospective_hash,
            arm_input_hash: arm.input_hash().map_err(invalid)?,
            owned_snapshot_hash: named_import_stop_owned_hash(target.owned())
                .map_err(|_| EnforcerError::InvalidInput)?,
            runtime_binding_hash: content_hash(
                "ai-guard:legacy-named-import-runtime-binding:v1",
                target.authority(),
            )
            .map_err(invalid)?,
        })
    }

    pub(super) fn matches_birth(&self, other: &Self) -> EnforcerResult<bool> {
        self.birth.matches(&other.birth)
    }

    pub(super) fn prospective_hash(&self) -> &str {
        &self.prospective_hash
    }

    pub(super) fn require_claim(
        &self,
        request: &NamedImportPrepareV1,
        registration: &RegistrationV1,
    ) -> EnforcerResult<()> {
        request
            .claim
            .validate_for_arm(
                self.birth.binding_ref,
                &self.birth.scope.control_namespace_uuid().to_string(),
                &registration.arm,
                &registration.receipt,
            )
            .and_then(|()| request.claim.validate_receipt(&request.receipt))
            .map_err(|_| EnforcerError::Conflict)
    }

    /// Check the verification target against the retained original Import.
    /// Its new journal head and Create/Terminal facts come from the Writer.
    pub(super) fn require_verification_target(
        &self,
        facts: &NamedImportVerificationProspectiveFactsV2,
        target: &ProspectiveRegistrationV1,
        import: &RegistrationV1,
    ) -> EnforcerResult<()> {
        facts.validate_target(&self.birth.scope, target)?;
        let (import_helper_id, import_arm) = facts.import_predecessor()?;
        let invalid = |_| EnforcerError::InvalidInput;
        if facts.operation_id != self.birth.operation_id
            || facts.request.binding_ref != self.birth.binding_ref
            || facts.session_id != self.birth.session_id
            || facts.arm_pending_head.revision() <= self.birth.arm_pending_head.revision()
            || facts.arm_pending_head.hash() == self.birth.arm_pending_head.hash()
            || import.arm().input_hash().map_err(invalid)? != self.arm_input_hash
            || canonical(import_arm).map_err(|_| EnforcerError::InvalidInput)?
                != canonical(import.receipt()).map_err(|_| EnforcerError::InvalidInput)?
            || import_helper_id != import.owned.handle.operation_id().to_string()
            || target.arm().intent_hash != import.arm().intent_hash
            || target.owned().handle.id() == import.owned.handle.id()
            || facts.runtime_binding_hash != self.runtime_binding_hash
        {
            return Err(EnforcerError::Conflict);
        }
        Ok(())
    }

    pub(super) fn require_facts(
        &self,
        facts: &NamedImportStopFactsV1,
        target: &ProspectiveRegistrationV1,
    ) -> EnforcerResult<()> {
        if facts.version != 1 {
            return Err(EnforcerError::InvalidInput);
        }
        let invalid = |_| EnforcerError::InvalidInput;
        if facts.operation_id != self.birth.operation_id
            || facts.binding_ref != self.birth.binding_ref
            || facts.arm_pending_head != self.birth.arm_pending_head
            || facts.stop_head.revision() <= self.birth.arm_pending_head.revision()
            || facts.stop_head.hash() == self.birth.arm_pending_head.hash()
            || facts.arm_id != target.arm_id()
            || facts.arm_input_hash != self.arm_input_hash
            || facts.owned_snapshot_hash != self.owned_snapshot_hash
            || facts.runtime_binding_hash != self.runtime_binding_hash
            || canonical(&facts.scope).map_err(invalid)?
                != canonical(&self.birth.scope).map_err(invalid)?
            || target.digest()? != self.prospective_hash
        {
            return Err(EnforcerError::Conflict);
        }
        let command = LocalCommandV1::new(
            self.birth.operation_id,
            &target.arm().intent_hash,
            facts.command_id,
            LocalCommandRequestV1::CancelRequested,
        )
        .map_err(invalid)?;
        if command.input_hash() != facts.command_input_hash {
            return Err(EnforcerError::Conflict);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn fixture_stop_facts(
        &self,
        target: &ProspectiveRegistrationV1,
    ) -> NamedImportStopFactsV1 {
        let command_id = Uuid::new_v4();
        let cancel = LocalCommandV1::new(
            self.birth.operation_id,
            &target.arm().intent_hash,
            command_id,
            LocalCommandRequestV1::CancelRequested,
        )
        .unwrap();
        NamedImportStopFactsV1 {
            version: 1,
            scope: self.birth.scope.clone(),
            operation_id: self.birth.operation_id,
            binding_ref: self.birth.binding_ref,
            arm_pending_head: self.birth.arm_pending_head.clone(),
            command_id,
            command_input_hash: cancel.input_hash().to_owned(),
            stop_head: RevisionRefV1::new(
                self.birth.arm_pending_head.revision() + 1,
                &"f".repeat(64),
            )
            .unwrap(),
            arm_id: target.arm_id().to_owned(),
            arm_input_hash: self.arm_input_hash.clone(),
            owned_snapshot_hash: self.owned_snapshot_hash.clone(),
            runtime_binding_hash: self.runtime_binding_hash.clone(),
        }
    }
}

fn deserialize_non_nil_uuid<'de, D: Deserializer<'de>>(decoder: D) -> Result<Uuid, D::Error> {
    let raw = String::deserialize(decoder)?;
    canonical_uuid(&raw)
        .ok()
        .filter(|id| !id.is_nil())
        .ok_or_else(|| D::Error::custom("invalid private named STOP identifier"))
}
