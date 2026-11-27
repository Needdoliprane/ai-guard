//! Retained Legacy worker facts for a future observation-only reconciliation.
use super::*;
use crate::{
    control::{ArmedStartClaimV1, MutationReceiptV1, WorkerStateV1},
    runtime::RuntimeKind,
};

/// Historical data from one verified journal prefix. The STOP receipt records
/// a request. The Arm, claim and Created snapshot qualify the target, while a
/// fresh observation and current control identity still need separate proof.
/// The complete authority comes from the confirmed head. The Legacy Arm
/// fingerprint binds the engine and excludes the invocation. A consumer
/// must reopen the authority with the exact configured invocation.
/// Any later publication must recheck the prefix under the same writer.
pub(in crate::broker) struct LegacyWorkerTerminalEvidenceV1 {
    scope: JournalScopeV1,
    head: RevisionRefV1,
    cancel_receipt: LocalCommandReceiptV1,
    arm: ArmV1,
    arm_receipt: ArmReceiptV1,
    claim: ArmedStartClaimV1,
    claim_receipt: MutationReceiptV1,
    authority: RuntimeAuthority,
    owned: OwnedContainerSnapshot,
    worker: WorkerV1,
}
opaque!(LegacyWorkerTerminalEvidenceV1);

impl LegacyWorkerTerminalEvidenceV1 {
    pub(in crate::broker) fn operation_id(&self) -> Uuid {
        self.owned.handle.operation_id()
    }

    pub(in crate::broker) fn scope(&self) -> &JournalScopeV1 {
        &self.scope
    }

    pub(in crate::broker) fn head(&self) -> &RevisionRefV1 {
        &self.head
    }

    pub(in crate::broker) fn cancel_receipt(&self) -> &LocalCommandReceiptV1 {
        &self.cancel_receipt
    }

    pub(in crate::broker) fn arm(&self) -> (&ArmV1, &ArmReceiptV1) {
        (&self.arm, &self.arm_receipt)
    }

    pub(in crate::broker) fn claim(&self) -> (&ArmedStartClaimV1, &MutationReceiptV1) {
        (&self.claim, &self.claim_receipt)
    }

    pub(in crate::broker) fn target(&self) -> (&RuntimeAuthority, &OwnedContainerSnapshot) {
        (&self.authority, &self.owned)
    }

    pub(in crate::broker) fn worker(&self) -> &WorkerV1 {
        &self.worker
    }
}

impl StopJournal {
    /// Read the original claim under the retained lease and writer lock using
    /// the STOP reader. This reads metadata and the verified chain only.
    pub(in crate::broker) fn legacy_worker_terminal_evidence(
        &self,
        operation_id: Uuid,
        command_id: Uuid,
    ) -> JournalResult<LegacyWorkerTerminalEvidenceV1> {
        if command_id.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        let mut writer = self.core.writer.lock().map_err(|_| JournalError::Corrupt)?;
        let (reader, job, incomplete) = self
            .read_locked(operation_id, &mut writer)?
            .ok_or(JournalError::Interrupted)?;
        if incomplete {
            return Err(JournalError::Interrupted);
        }
        let proof = project(&reader.scope, &job, command_id)?;
        reader.check_shared()?;
        Ok(proof)
    }
}

fn project(
    scope: &JournalScopeV1,
    job: &RecoveredJobV1,
    command_id: Uuid,
) -> JournalResult<LegacyWorkerTerminalEvidenceV1> {
    let boundary = *job
        .cancel_boundaries
        .get(&command_id)
        .ok_or(JournalError::Interrupted)?;
    let cancel_receipt = job
        .cancel_receipt(command_id)
        .ok_or(JournalError::Corrupt)?;
    if cancel_receipt.command_id() != command_id
        || !job.chain.iter().any(|head| head == cancel_receipt.head())
    {
        return Err(JournalError::Corrupt);
    }

    let metadata = job.metadata.as_ref().ok_or(JournalError::Corrupt)?;
    let checkpoint = &metadata.checkpoint;
    let supervised = checkpoint.supervised().ok_or(JournalError::Interrupted)?;
    let supervision = supervised.supervision();
    let context = supervised.runner().broker_context();
    if checkpoint.retirement().is_some()
        || metadata.intent.legacy_named_plan().is_some()
        || supervision.legacy_named_admission().is_some()
    {
        return Err(JournalError::UnsupportedVersion);
    }
    // The reader checks the configured gateway with the journal's URL
    // comparison. The namespace and tenant identify the selected worker.
    if supervision.control_namespace_uuid() != scope.control_namespace_uuid()
        || context.tenant_id != Some(scope.tenant_id())
    {
        return Err(JournalError::Conflict);
    }

    let mut targets = job
        .stops()
        .iter()
        .filter(|stop| matches!(&stop.arm().subject, ArmSubjectV1::Worker));
    let target = targets.next().ok_or(JournalError::Interrupted)?;
    let mut claims = supervision.operations().iter().enumerate().filter_map(
        |(index, operation)| match operation {
            SupervisionOperationV1::WorkerClaim { request, response } => {
                Some((index, request, response.as_ref()))
            }
            _ => None,
        },
    );
    let (claim_index, claim, acknowledged) = claims.next().ok_or(JournalError::Interrupted)?;
    if targets.next().is_some()
        || claims.next().is_some()
        || claim_index <= target.birth_index()
        || claim_index >= boundary
    {
        return Err(JournalError::Conflict);
    }
    let acknowledged = acknowledged.ok_or(JournalError::Interrupted)?;

    let worker = context.worker.ok_or(JournalError::Interrupted)?;
    let owned = context.owned.ok_or(JournalError::Interrupted)?;
    let authority = context.binding.ok_or(JournalError::Interrupted)?;
    if authority.authority_id.is_nil()
        || authority.invocation.is_none()
        || authority.endpoint.is_empty()
        || matches!(
            (authority.kind, authority.engine_id.as_deref()),
            (RuntimeKind::Docker, None | Some("")) | (RuntimeKind::Podman, Some(_))
        )
    {
        return Err(JournalError::Interrupted);
    }
    let arm = target.arm();
    let arm_receipt = target.receipt();
    if authority != target.authority()
        || authority.fingerprint() != arm.authority_binding_hash
        || canonical(owned)? != canonical(target.owned())?
        || owned.handle.operation_id() != job.operation_id()
        || owned.handle.id() != arm.inspection.container_id
        || worker.admission_operation_id != job.operation_id().to_string()
        || worker.worker_id != arm.worker_id
        || worker.attempt.attempt_id != arm.attempt_id
        || worker.attempt.container_id.as_deref() != Some(owned.handle.id())
        || worker.attempt.created_inspection.as_ref() != Some(&arm.inspection)
        || worker.attempt != acknowledged.worker.attempt
        || worker.revision < acknowledged.worker.revision
        || canonical(&claim.arm_receipt)? != canonical(arm_receipt)?
        || acknowledged.receipt.audit_seq <= arm_receipt.audit_seq
    {
        return Err(JournalError::Conflict);
    }

    // The reader has already validated the entire chain, including claim
    // request/ACK content and the transition to the retained Worker.
    let known_allocation = matches!(
        (worker.state, worker.quota_held),
        (WorkerStateV1::Running | WorkerStateV1::Stopping, true) | (WorkerStateV1::Terminal, false)
    );
    let known_start = worker
        .observed_started_at
        .as_deref()
        .and_then(|start| chrono::DateTime::parse_from_rfc3339(start).ok())
        .is_some_and(|start| start.timestamp() > 0);
    if !known_allocation || worker.allocation_inconsistent || !known_start {
        return Err(JournalError::Interrupted);
    }

    Ok(LegacyWorkerTerminalEvidenceV1 {
        scope: scope.clone(),
        head: job.head().clone(),
        cancel_receipt: cancel_receipt.clone(),
        arm: arm.clone(),
        arm_receipt: arm_receipt.clone(),
        claim: claim.clone(),
        claim_receipt: acknowledged.receipt.clone(),
        authority: authority.clone(),
        owned: owned.clone(),
        worker: worker.clone(),
    })
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
#[path = "legacy_worker_terminal_tests.rs"]
mod tests;
