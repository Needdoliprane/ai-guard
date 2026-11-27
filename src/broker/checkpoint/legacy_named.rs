//! Private named admission facts retained in the broker checkpoint.
use super::*;
use crate::control::session_control::{
    LegacyConfirmedPlanV1, LegacyNamedAdmissionReceiptV1, LegacyNamedSupervisedAdmissionV1,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyNamedAdmissionCheckpointV1 {
    version: u32,
    plan: LegacyConfirmedPlanV1,
    request: LegacyNamedSupervisedAdmissionV1,
    receipt: Option<LegacyNamedAdmissionReceiptV1>,
}

impl LegacyNamedAdmissionCheckpointV1 {
    #[allow(
        dead_code,
        reason = "the private named transport will retain the pending request"
    )]
    pub(crate) fn new(
        plan: LegacyConfirmedPlanV1,
        request: LegacyNamedSupervisedAdmissionV1,
    ) -> JournalResult<Self> {
        let value = Self {
            version: 1,
            plan,
            request,
            receipt: None,
        };
        let namespace = uuid(&value.plan.control_namespace_uuid)?;
        value.validate_shape(namespace)?;
        Ok(value)
    }

    /// Retain the authenticated Store reply at the existing pending operation.
    #[allow(
        dead_code,
        reason = "the private named transport will supply the Store acknowledgement"
    )]
    pub(crate) fn acknowledge(
        mut self,
        receipt: LegacyNamedAdmissionReceiptV1,
    ) -> JournalResult<Self> {
        ensure(self.receipt.is_none())?;
        self.receipt = Some(receipt);
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
        self.receipt.as_ref().map(|receipt| &receipt.admission)
    }

    pub(super) fn validate_shape(&self, namespace: Uuid) -> JournalResult<()> {
        if self.version != 1 {
            return Err(JournalError::UnsupportedVersion);
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
        ensure(canonical(self)?.len() <= MAX_OPERATION_BYTES)
    }

    pub(super) fn validate_transition(&self, previous: Option<&Self>) -> JournalResult<()> {
        match previous {
            None => ensure(self.receipt.is_none()),
            Some(old) => {
                ensure(self.plan == old.plan && equal(&self.request, &old.request)?)?;
                ensure(old.receipt.is_none() || equal(&self.receipt, &old.receipt)?)
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
