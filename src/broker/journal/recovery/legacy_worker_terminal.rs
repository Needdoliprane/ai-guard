//! Retained Legacy worker facts and the first durable terminal request.
use super::*;
use crate::{
    broker::checkpoint::SupervisionCheckpointV1,
    control::{
        ArmedStartClaimV1, MutationReceiptV1, MutationResultV1, RuntimeStatusV1, WorkerStateV1,
        content_hash, hash,
    },
    runtime::RuntimeKind,
};
use std::sync::Weak;

/// Historical data from one verified journal prefix. The STOP receipt records
/// a request. The Arm, claim and Created snapshot qualify the target, while a
/// fresh observation and current control identity still need separate proof.
/// The complete authority comes from the confirmed head. The Legacy Arm
/// fingerprint binds the engine and excludes the invocation. A consumer
/// must reopen the authority with the exact configured invocation.
/// Any later publication must recheck the prefix under the same writer.
pub(in crate::broker) struct LegacyWorkerTerminalEvidenceV1 {
    owner: Weak<Inner>,
    scope: JournalScopeV1,
    head: RevisionRefV1,
    cancel_receipt: LocalCommandReceiptV1,
    arm: ArmV1,
    arm_receipt: ArmReceiptV1,
    claim: ArmedStartClaimV1,
    claim_receipt: MutationReceiptV1,
    claim_index: usize,
    authority: RuntimeAuthority,
    owned: OwnedContainerSnapshot,
    worker: WorkerV1,
}
opaque!(LegacyWorkerTerminalEvidenceV1);

/// Exact journal data. An append or cold read grants no authority to send it.
#[derive(Clone)]
#[allow(
    dead_code,
    reason = "the Legacy terminal publisher is composed separately"
)]
pub(in crate::broker) struct RetainedLegacyWorkerTerminalObservationV1 {
    owner: Weak<Inner>,
    operation: Uuid,
    head: RevisionRefV1,
    stop: LocalCommandReceiptV1,
    request: WorkerMutationV1,
    expected_worker: WorkerV1,
    parent_audit_seq: i64,
}

#[allow(
    dead_code,
    reason = "the Legacy terminal publisher is composed separately"
)]
pub(in crate::broker) struct ConfirmedLegacyWorkerTerminalObservationV1 {
    head: RevisionRefV1,
    response: MutationResultV1,
}

#[allow(
    dead_code,
    reason = "the Legacy terminal publisher is composed separately"
)]
impl ConfirmedLegacyWorkerTerminalObservationV1 {
    pub(in crate::broker) fn head(&self) -> &RevisionRefV1 {
        &self.head
    }

    pub(in crate::broker) fn response(&self) -> &MutationResultV1 {
        &self.response
    }
}

struct RecordedLegacyWorkerTerminalObservation<'a> {
    index: usize,
    parent: &'a WorkerV1,
    pending: RetainedLegacyWorkerTerminalObservationV1,
    response: Option<&'a MutationResultV1>,
}

#[allow(
    dead_code,
    reason = "the Legacy terminal publisher is composed separately"
)]
pub(in crate::broker) enum ReservedLegacyWorkerTerminalObservationV1 {
    Appended(RetainedLegacyWorkerTerminalObservationV1),
    Existing(RetainedLegacyWorkerTerminalObservationV1),
}

opaque!(
    RetainedLegacyWorkerTerminalObservationV1,
    ConfirmedLegacyWorkerTerminalObservationV1,
    ReservedLegacyWorkerTerminalObservationV1
);

#[allow(
    dead_code,
    reason = "the Legacy terminal publisher is composed separately"
)]
impl RetainedLegacyWorkerTerminalObservationV1 {
    pub(in crate::broker) fn head(&self) -> &RevisionRefV1 {
        &self.head
    }

    pub(in crate::broker) fn stop(&self) -> &LocalCommandReceiptV1 {
        &self.stop
    }

    pub(in crate::broker) fn request(&self) -> &WorkerMutationV1 {
        &self.request
    }

    /// Borrow the terminal postimage only for evidence read at the same
    /// journal head, under the same retained owner and original STOP.
    pub(in crate::broker) fn expected_worker_for_publication(
        &self,
        evidence: &LegacyWorkerTerminalEvidenceV1,
    ) -> JournalResult<&WorkerV1> {
        let _owner = self.owner.upgrade().ok_or(JournalError::Interrupted)?;
        let stop = evidence.cancel_receipt();
        let worker = evidence.worker();
        if !Weak::ptr_eq(&self.owner, &evidence.owner)
            || self.operation != evidence.operation_id()
            || self.head() != evidence.head()
            || self.stop.command_id() != stop.command_id()
            || self.stop.head() != stop.head()
            || self.stop.input_hash() != stop.input_hash()
            || self.request.operation_id != evidence.terminal_observation_operation_id()?
            || self.request.worker_id != worker.worker_id
            || self.request.attempt_id != worker.attempt.attempt_id
            || self.request.expected_revision != worker.revision
        {
            return Err(JournalError::Conflict);
        }
        Ok(&self.expected_worker)
    }

    /// Match a response from the authenticated Core to the first durable body
    /// and the exact parent revision. The audit pair identifies an event for a
    /// separate audit check. This method only validates the response.
    pub(in crate::broker) fn validate_response(
        &self,
        response: &MutationResultV1,
    ) -> JournalResult<()> {
        let receipt = &response.receipt;
        let expected_hash = content_hash("ai-guard:worker-mutation:v1", self.request())
            .map_err(|_| JournalError::Conflict)?;
        if receipt.operation_id != self.request.operation_id
            || receipt.input_hash != expected_hash
            || receipt.worker_id.as_deref() != Some(self.request.worker_id.as_str())
            || receipt.revision != Some(self.expected_worker.revision)
            || response.disposition.is_some()
            || receipt.audit_seq <= self.parent_audit_seq
            || hash(&receipt.audit_hash).is_err()
            || canonical(&response.worker)? != canonical(&self.expected_worker)?
        {
            return Err(JournalError::Conflict);
        }
        Ok(())
    }
}

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

    pub(in crate::broker) fn terminal_observation_operation_id(&self) -> JournalResult<String> {
        let claim_id =
            crate::control::session_control::canonical_uuid(&self.claim_receipt.operation_id)
                .map_err(|_| JournalError::Conflict)?;
        Ok(crate::model::bound_event_id(
            b"ai-guard:legacy-worker-terminal-observation:v1",
            claim_id,
        ))
    }

    fn target_facts(&self) -> JournalResult<Vec<u8>> {
        canonical(&(
            self.scope(),
            self.cancel_receipt().command_id(),
            self.cancel_receipt().input_hash(),
            self.cancel_receipt().head(),
            self.arm(),
            self.claim(),
            self.target(),
            self.worker(),
        ))
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
        let proof = project(&self.core, &job, command_id)?;
        reader.check_shared()?;
        Ok(proof)
    }

    pub(in crate::broker) fn legacy_worker_terminal_observation(
        &self,
        operation_id: Uuid,
    ) -> JournalResult<Option<RetainedLegacyWorkerTerminalObservationV1>> {
        let mut writer = self.core.writer.lock().map_err(|_| JournalError::Corrupt)?;
        let (reader, job, incomplete) = self
            .read_locked(operation_id, &mut writer)?
            .ok_or(JournalError::Interrupted)?;
        if incomplete {
            return Err(JournalError::Interrupted);
        }
        let command = *job
            .cancel_command_ids()
            .last()
            .ok_or(JournalError::Interrupted)?;
        let current = project(&self.core, &job, command)?;
        let retained = retained_observation(&job, &current)?;
        reader.check_shared()?;
        Ok(retained)
    }

    /// Read a durable ACK under the current lease from an exact journal
    /// ancestor. None identifies a verified request still awaiting its ACK.
    pub(in crate::broker) fn confirmed_legacy_worker_terminal_observation(
        &self,
        operation_id: Uuid,
        expected_scope: &JournalScopeV1,
        ancestor: &RevisionRefV1,
    ) -> JournalResult<Option<ConfirmedLegacyWorkerTerminalObservationV1>> {
        if canonical(expected_scope)? != canonical(self.scope())? {
            return Err(JournalError::Conflict);
        }
        let mut writer = self.core.writer.lock().map_err(|_| JournalError::Corrupt)?;
        let (reader, job, incomplete) = self
            .read_locked(operation_id, &mut writer)?
            .ok_or(JournalError::Interrupted)?;
        if incomplete {
            return Err(JournalError::Interrupted);
        }
        if !job.chain.contains(ancestor) {
            return Err(JournalError::Conflict);
        }
        let command = *job
            .cancel_command_ids()
            .last()
            .ok_or(JournalError::Interrupted)?;
        let proof = project(&self.core, &job, command)?;
        let recorded = recorded_observation(&job, &proof)?.ok_or(JournalError::Interrupted)?;
        reader.check_shared()?;
        Ok(recorded
            .response
            .map(|response| ConfirmedLegacyWorkerTerminalObservationV1 {
                head: job.head().clone(),
                response: response.clone(),
            }))
    }

    pub(in crate::broker) fn reserve_legacy_worker_terminal_observation(
        &self,
        evidence: &LegacyWorkerTerminalEvidenceV1,
        request: WorkerMutationV1,
        on_publication_attempt: impl FnOnce(),
    ) -> JournalResult<ReservedLegacyWorkerTerminalObservationV1> {
        use ReservedLegacyWorkerTerminalObservationV1::{Appended, Existing};

        if !Weak::ptr_eq(&evidence.owner, &Arc::downgrade(&self.core)) {
            return Err(JournalError::Conflict);
        }
        let operation = evidence.operation_id();
        let command = evidence.cancel_receipt().command_id();
        let mut writer = self.core.writer.lock().map_err(|_| JournalError::Corrupt)?;
        let (reader, job, incomplete) = self
            .read_locked(operation, &mut writer)?
            .ok_or(JournalError::Interrupted)?;
        if incomplete {
            return Err(JournalError::Interrupted);
        }

        let current = project(&self.core, &job, command)?;
        if !job.chain.contains(evidence.head())
            || evidence.target_facts()? != current.target_facts()?
        {
            return Err(JournalError::Conflict);
        }
        if let Some(retained) = retained_observation(&job, &current)? {
            require_exact_observation(&retained, command, &request)?;
            reader.check_shared()?;
            return Ok(Existing(retained));
        }
        if job.head() != evidence.head() {
            return Err(JournalError::Conflict);
        }
        if writer.cold_audit.upgrade().is_some() {
            return Err(JournalError::Busy);
        }
        validate_terminal_request(&current, current.worker(), &request)?;
        let metadata = job.metadata.as_ref().ok_or(JournalError::Corrupt)?;
        let lifecycle = metadata
            .checkpoint
            .lifecycle()
            .ok_or(JournalError::Interrupted)?;
        let old = lifecycle.broker();
        let supervision = old.supervision();
        terminal_parent_audit_seq(
            supervision.operations(),
            current.worker(),
            current.claim().1.audit_seq,
        )?;
        let mut operations = supervision.operations().to_vec();
        let stop = last_stop_before_observation(&job, current.claim_index, operations.len())?;
        if stop.command_id() != command {
            return Err(JournalError::Conflict);
        }
        // A running broker can have retained a random-id Observe (or another
        // worker transition) before losing its Store reply. It still owns
        // reconciliation of that request. The new ID cannot take its place.
        for prior in &operations {
            if let SupervisionOperationV1::WorkerMutation {
                request: prior,
                response,
            } = prior
                && prior.worker_id == request.worker_id
                && prior.attempt_id == request.attempt_id
            {
                let response = response.as_ref().ok_or(JournalError::Interrupted)?;
                if response.worker.revision > request.expected_revision {
                    return Err(JournalError::Conflict);
                }
            }
        }
        operations.push(SupervisionOperationV1::WorkerMutation {
            request: request.clone(),
            response: None,
        });
        let next = terminal_checkpoint(&metadata.checkpoint, old.runner().clone(), operations)?;
        let (reader, confirmed) = self.publish_terminal_checkpoint(
            reader,
            &job,
            &mut writer,
            next,
            on_publication_attempt,
        )?;
        let current = project(&self.core, &confirmed, command)?;
        let retained = retained_observation(&confirmed, &current)?.ok_or(JournalError::Corrupt)?;
        require_exact_observation(&retained, command, &request)?;
        reader.check_shared()?;
        #[cfg(test)]
        stop_before_ack()?;
        Ok(Appended(retained))
    }

    pub(in crate::broker) fn confirm_legacy_worker_terminal_observation(
        &self,
        pending: &RetainedLegacyWorkerTerminalObservationV1,
        response: MutationResultV1,
        on_publication_attempt: impl FnOnce(),
    ) -> JournalResult<ConfirmedLegacyWorkerTerminalObservationV1> {
        if !Weak::ptr_eq(&pending.owner, &Arc::downgrade(&self.core)) {
            return Err(JournalError::Conflict);
        }
        let operation = pending.operation;
        let command = pending.stop().command_id();
        let mut writer = self.core.writer.lock().map_err(|_| JournalError::Corrupt)?;
        let (reader, job, incomplete) = self
            .read_locked(operation, &mut writer)?
            .ok_or(JournalError::Interrupted)?;
        if incomplete {
            return Err(JournalError::Interrupted);
        }
        if !job.chain.contains(pending.head()) {
            return Err(JournalError::Conflict);
        }

        let proof = project(&self.core, &job, command)?;
        let recorded = recorded_observation(&job, &proof)?.ok_or(JournalError::Conflict)?;
        require_exact_observation(&recorded.pending, command, pending.request())?;
        if recorded.pending.stop().head() != pending.stop().head()
            || recorded.pending.stop().input_hash() != pending.stop().input_hash()
            || recorded.pending.parent_audit_seq != pending.parent_audit_seq
            || canonical(&recorded.pending.expected_worker)? != canonical(&pending.expected_worker)?
        {
            return Err(JournalError::Conflict);
        }
        recorded.pending.validate_response(&response)?;
        if let Some(saved) = recorded.response {
            if canonical(saved)? != canonical(&response)? {
                return Err(JournalError::Conflict);
            }
            reader.check_shared()?;
            return Ok(ConfirmedLegacyWorkerTerminalObservationV1 {
                head: job.head().clone(),
                response: saved.clone(),
            });
        }
        if writer.cold_audit.upgrade().is_some() {
            return Err(JournalError::Busy);
        }

        let metadata = job.metadata.as_ref().ok_or(JournalError::Corrupt)?;
        let broker = metadata
            .checkpoint
            .supervised()
            .ok_or(JournalError::Corrupt)?;
        let runner = broker
            .runner()
            .with_confirmed_terminal_worker(recorded.parent, &response.worker)?;
        let mut operations = broker.supervision().operations().to_vec();
        let Some(SupervisionOperationV1::WorkerMutation {
            response: stored, ..
        }) = operations.get_mut(recorded.index)
        else {
            return Err(JournalError::Corrupt);
        };
        *stored = Some(response.clone());
        let next = terminal_checkpoint(&metadata.checkpoint, runner, operations)?;
        let (reader, confirmed) = self.publish_terminal_checkpoint(
            reader,
            &job,
            &mut writer,
            next,
            on_publication_attempt,
        )?;
        let proof = project(&self.core, &confirmed, command)?;
        let recorded = recorded_observation(&confirmed, &proof)?.ok_or(JournalError::Corrupt)?;
        let saved = recorded.response.ok_or(JournalError::Corrupt)?;
        if canonical(saved)? != canonical(&response)? {
            return Err(JournalError::Corrupt);
        }
        reader.check_shared()?;
        #[cfg(test)]
        stop_before_ack()?;
        Ok(ConfirmedLegacyWorkerTerminalObservationV1 {
            head: confirmed.head().clone(),
            response: saved.clone(),
        })
    }

    fn publish_terminal_checkpoint(
        &self,
        reader: StopRecoveryReaderV1,
        job: &RecoveredJobV1,
        writer: &mut Writer,
        next: LoadedCheckpoint,
        on_publication_attempt: impl FnOnce(),
    ) -> JournalResult<(StopRecoveryReaderV1, RecoveredJobV1)> {
        let operation = job.operation_id();
        let metadata = job.metadata.as_ref().ok_or(JournalError::Corrupt)?;
        confirmed::context_checkpoint_with_launch(
            &self.core.scope,
            &self.core.control,
            &next,
            &metadata.intent,
            &metadata.binding,
            metadata.launch_binding.as_ref(),
        )?;
        confirmed::check_transition(
            &metadata.checkpoint,
            &next,
            metadata.legacy_managed_seen,
            metadata.last_cancel_operations,
        )?;
        let intent_hash = metadata
            .intent
            .intent_hash()
            .map_err(|_| JournalError::Corrupt)?;
        let (bytes, head) =
            encode_checkpoint(&self.core, operation, &intent_hash, job.head(), &next)?;
        let usage = stop_budget_usage(&self.core, operation)?;
        reader.check_shared()?;
        publish_revision_observing(
            &self.core,
            operation,
            &intent_hash,
            job.head(),
            &head,
            &bytes,
            PublicationResources {
                usage,
                budget: retirement::PublicationBudget::NewAllocation,
                writer,
            },
            on_publication_attempt,
        )?;
        writer.floors.insert(operation, head.clone());
        writer.observed_jobs.insert(operation);
        let (reader, confirmed, incomplete) = self
            .read_locked(operation, writer)?
            .ok_or(JournalError::Corrupt)?;
        if incomplete || confirmed.head() != &head {
            return Err(JournalError::Interrupted);
        }
        Ok((reader, confirmed))
    }
}

fn terminal_checkpoint(
    old: &LoadedCheckpoint,
    runner: crate::runner::CheckpointPayloadV1,
    operations: Vec<SupervisionOperationV1>,
) -> JournalResult<LoadedCheckpoint> {
    let lifecycle = old.lifecycle().ok_or(JournalError::Interrupted)?;
    let supervision = lifecycle.broker().supervision();
    let broker = BrokerCheckpointV2::new(
        runner,
        SupervisionCheckpointV1::new(
            supervision.control_namespace_uuid(),
            supervision.helpers().to_vec(),
            operations,
        )?,
    )?;
    let lifecycle = BrokerCheckpointV3::new(broker, lifecycle.commands().to_vec())?;
    Ok(match old.outbox() {
        Some(old) => {
            LoadedCheckpoint::V4(BrokerCheckpointV4::new(lifecycle, old.events().to_vec())?)
        }
        None => LoadedCheckpoint::V3(lifecycle),
    })
}

fn require_exact_observation(
    retained: &RetainedLegacyWorkerTerminalObservationV1,
    command: Uuid,
    request: &WorkerMutationV1,
) -> JournalResult<()> {
    if retained.stop.command_id() != command || canonical(&retained.request)? != canonical(request)?
    {
        return Err(JournalError::Conflict);
    }
    Ok(())
}

fn retained_observation(
    job: &RecoveredJobV1,
    proof: &LegacyWorkerTerminalEvidenceV1,
) -> JournalResult<Option<RetainedLegacyWorkerTerminalObservationV1>> {
    let Some(recorded) = recorded_observation(job, proof)? else {
        return Ok(None);
    };
    if recorded.response.is_some() {
        return Err(JournalError::Conflict);
    }
    Ok(Some(recorded.pending))
}

fn recorded_observation<'a>(
    job: &'a RecoveredJobV1,
    proof: &LegacyWorkerTerminalEvidenceV1,
) -> JournalResult<Option<RecordedLegacyWorkerTerminalObservation<'a>>> {
    let metadata = job.metadata.as_ref().ok_or(JournalError::Corrupt)?;
    let supervision = metadata
        .checkpoint
        .supervised()
        .ok_or(JournalError::Interrupted)?
        .supervision();
    let id = proof.terminal_observation_operation_id()?;
    let operations = supervision.operations();
    for (index, operation) in operations.iter().enumerate() {
        if let SupervisionOperationV1::WorkerMutation { request, response } = operation
            && request.operation_id == id
        {
            // The head's projection advances on ACK. The exact parent
            // remains in the verified supervision prefix.
            let parent = terminal_parent_response(
                &operations[..index],
                &request.worker_id,
                &request.attempt_id,
                request.expected_revision,
                proof.claim().1.audit_seq,
            )?;
            let expected_worker = validate_terminal_request(proof, &parent.worker, request)?;
            let stop = last_stop_before_observation(job, proof.claim_index, index)?;
            let pending = RetainedLegacyWorkerTerminalObservationV1 {
                owner: proof.owner.clone(),
                operation: job.operation_id(),
                head: job.head().clone(),
                stop: stop.clone(),
                request: request.clone(),
                expected_worker,
                parent_audit_seq: parent.receipt.audit_seq,
            };
            let response = response.as_ref();
            let current = proof.worker();
            if let Some(saved) = response {
                pending.validate_response(saved)?;
                if current.revision < saved.worker.revision
                    || (current.revision == saved.worker.revision
                        && canonical(current)? != canonical(&saved.worker)?)
                {
                    return Err(JournalError::Conflict);
                }
            } else if canonical(current)? != canonical(&parent.worker)? {
                return Err(JournalError::Conflict);
            }
            return Ok(Some(RecordedLegacyWorkerTerminalObservation {
                index,
                parent: &parent.worker,
                pending,
                response,
            }));
        }
    }
    Ok(None)
}

fn terminal_parent_audit_seq(
    prefix: &[SupervisionOperationV1],
    parent: &WorkerV1,
    claim_audit_seq: i64,
) -> JournalResult<i64> {
    let matching = terminal_parent_response(
        prefix,
        &parent.worker_id,
        &parent.attempt.attempt_id,
        parent.revision,
        claim_audit_seq,
    )?;
    if canonical(&matching.worker)? != canonical(parent)? {
        return Err(JournalError::Conflict);
    }
    Ok(matching.receipt.audit_seq)
}

fn terminal_parent_response<'a>(
    prefix: &'a [SupervisionOperationV1],
    worker_id: &str,
    attempt_id: &str,
    revision: u64,
    claim_audit_seq: i64,
) -> JournalResult<&'a MutationResultV1> {
    let mut replies = prefix.iter().filter_map(|operation| {
        let SupervisionOperationV1::WorkerMutation {
            response: Some(response),
            ..
        } = operation
        else {
            return None;
        };
        let worker = &response.worker;
        (worker.worker_id == worker_id
            && worker.attempt.attempt_id == attempt_id
            && worker.revision == revision)
            .then_some(response)
    });
    let matching = replies.next().ok_or(JournalError::Conflict)?;
    if replies.next().is_some() || matching.receipt.audit_seq <= claim_audit_seq {
        return Err(JournalError::Conflict);
    }
    Ok(matching)
}

fn last_stop_before_observation(
    job: &RecoveredJobV1,
    claim_index: usize,
    observation_index: usize,
) -> JournalResult<&LocalCommandReceiptV1> {
    // A later Cancel's boundary includes the already recorded observation.
    // Choose the last eligible command in the verified append-only prefix
    // as it stood before this request was born, even on a later head.
    let command = job
        .cancel_command_ids()
        .iter()
        .rfind(|id| {
            job.cancel_boundaries
                .get(id)
                .is_some_and(|boundary| claim_index < *boundary && *boundary <= observation_index)
        })
        .ok_or(JournalError::Interrupted)?;
    job.cancel_receipt(*command).ok_or(JournalError::Corrupt)
}

fn validate_terminal_request(
    proof: &LegacyWorkerTerminalEvidenceV1,
    worker: &WorkerV1,
    request: &WorkerMutationV1,
) -> JournalResult<WorkerV1> {
    let WorkerTransitionV1::Observe { observation } = &request.transition else {
        return Err(JournalError::Conflict);
    };
    if request.operation_id != proof.terminal_observation_operation_id()?
        || observation.status != RuntimeStatusV1::Exited
        || !matches!(
            worker.state,
            WorkerStateV1::Running | WorkerStateV1::Stopping
        )
        || !worker.quota_held
        || worker.allocation_inconsistent
        || worker.unknown_reason.is_some()
        || worker.last_observation.as_ref().is_none_or(|last| {
            last.status != RuntimeStatusV1::Running || last.running != Some(true)
        })
    {
        return Err(JournalError::Conflict);
    }
    let observed = chrono::DateTime::parse_from_rfc3339(&observation.observed_at)
        .map_err(|_| JournalError::Conflict)?;
    let finished = chrono::DateTime::parse_from_rfc3339(
        observation
            .finished_at
            .as_deref()
            .ok_or(JournalError::Conflict)?,
    )
    .map_err(|_| JournalError::Conflict)?;
    let terminal = crate::audit::expected_worker_transition(worker, request)
        .map_err(|_| JournalError::Conflict)?;
    if terminal.state != WorkerStateV1::Terminal || terminal.quota_held || observed < finished {
        return Err(JournalError::Conflict);
    }
    Ok(terminal)
}

fn project(
    owner: &Arc<Inner>,
    job: &RecoveredJobV1,
    command_id: Uuid,
) -> JournalResult<LegacyWorkerTerminalEvidenceV1> {
    let scope = &owner.scope;
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
        owner: Arc::downgrade(owner),
        scope: scope.clone(),
        head: job.head().clone(),
        cancel_receipt: cancel_receipt.clone(),
        arm: arm.clone(),
        arm_receipt: arm_receipt.clone(),
        claim: claim.clone(),
        claim_receipt: acknowledged.receipt.clone(),
        claim_index,
        authority: authority.clone(),
        owned: owned.clone(),
        worker: worker.clone(),
    })
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
#[path = "legacy_worker_terminal_tests.rs"]
mod tests;
