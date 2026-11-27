//! Compare verified journal STOP facts with one live named Import target.
use super::{
    EnforcerError, EnforcerResult, NamedImportPrepareV1, ProspectiveRegistrationV1, RegistrationV1,
    hex_hash,
};
use crate::{
    broker::{
        journal::{
            JournalScopeV1, RevisionRefV1, canonical, recovery::named_import_stop_owned_hash,
        },
        lifecycle::{LocalCommandRequestV1, LocalCommandV1},
    },
    control::{
        ArmSubjectV1, HelperPermissionV1, HelperPurposeV1, content_hash,
        session_control::{LegacyConfirmedPlanV1, LegacyNamedImportArmRequestV1, canonical_uuid},
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
