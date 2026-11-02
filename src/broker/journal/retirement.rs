//! Journal-owned Full3 mutations and cumulative durable future reservations.
use super::*;
use crate::{
    broker::{
        enforcer::retirement::UnusedPrepareRevoked,
        retirement::{
            ClosingAcceptanceV1, LedgerHeadV1, RetiredReceiptV1, RetiredTombstoneV1,
            RetirementDigestV1, RetirementEvidenceV1, RetirementIdentityV1, RetirementReasonV1,
        },
        retirement_checkpoint::{
            BrokerCheckpointV5, FinalRetirementV1, Full3RegistrationBindingV1, PendingRetirementV1,
            RetirementEntryV1, acknowledged_arm, ensure_unclaimed,
        },
    },
    model::Event,
};

fn require(valid: bool) -> JournalResult<()> {
    if valid {
        Ok(())
    } else {
        Err(JournalError::Conflict)
    }
}

pub(crate) fn event(intent: &ExecutionIntentV1, id: Uuid, kind: &str, data: Value) -> Event {
    let mut event = Event::new(kind);
    let request = intent.request_data();
    event.id = id.to_string();
    event.source = "ai-guard-broker".into();
    event.coverage = "native".into();
    event.session_id = request.session_id.clone();
    event.project_id = request.project_id.clone();
    event.sandbox_id = request.sandbox_id.clone();
    event.correlation_id = request.correlation_id.clone();
    event.trace_id = request.trace_id.clone();
    event.data = data;
    event
}

impl Journal {
    #[cfg(test)]
    pub(super) fn advance_retirement(
        &self,
        operation: Uuid,
        expected: RevisionRefV1,
        next: &BrokerCheckpointV5,
    ) -> JournalResult<CheckpointReceiptV1> {
        self.advance_checkpoint(operation, expected, LoadedCheckpoint::V5(next.clone()))
    }
    pub(crate) fn bind_full3_registration(
        &self,
        operation: Uuid,
        expected: RevisionRefV1,
        binding: Full3RegistrationBindingV1,
    ) -> JournalResult<CheckpointReceiptV1> {
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        let job = self
            .load_locked(operation, &mut writer)?
            .ok_or(JournalError::InvalidInput)?;
        let outbox = full_outbox(&job)?;
        let (arm, _) = acknowledged_arm(&outbox, &binding.arm_id().to_string())?;
        let mut bindings = job
            .broker_checkpoint_v5()
            .map_or_else(Vec::new, |value| value.bindings().to_vec());
        if let Some(existing) = bindings
            .iter()
            .find(|value| value.arm_id() == binding.arm_id())
        {
            require(canonical(existing)? == canonical(&binding)?)?;
            // An exact binding remains replayable after claim. New binding
            // publication still requires the confirmed current head above.
            return Ok(CheckpointReceiptV1 {
                operation_id: operation,
                head: job.head().clone(),
            });
        }
        require(job.head() == &expected)?;
        ensure_unclaimed(&outbox, arm)?;
        bindings.push(binding);
        let retirements = job
            .broker_checkpoint_v5()
            .map_or_else(Vec::new, |value| value.retirements().to_vec());
        let next = BrokerCheckpointV5::new(outbox, bindings, retirements)?;
        self.advance_loaded_checkpoint(
            operation,
            expected,
            LoadedCheckpoint::V5(next),
            job,
            &mut writer,
        )
    }
    pub(crate) fn begin_unused_retirement(
        &self,
        operation: Uuid,
        expected: RevisionRefV1,
        unused: &UnusedPrepareRevoked,
        intention_id: Uuid,
        final_id: Uuid,
    ) -> JournalResult<(PendingRetirementV1, Event, CheckpointReceiptV1)> {
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        let job = self
            .load_locked(operation, &mut writer)?
            .ok_or(JournalError::InvalidInput)?;
        require(job.head() == &expected)?;
        let current = job.broker_checkpoint_v5().ok_or(JournalError::Conflict)?;
        let registration = unused.registration();
        require(
            registration.tenant_id() == job.scope().tenant_id()
                && registration.control_origin_hash()
                    == sha(job.scope().gateway_origin().as_bytes()),
        )?;
        let binding = current
            .bindings()
            .iter()
            .find(|value| value.arm_id().to_string() == registration.arm().operation_id)
            .ok_or(JournalError::Conflict)?;
        require(
            binding.original_epoch() == unused.epoch()
                && binding.ledger().matches(unused.ledger())?
                && binding.registration_hash().as_str()
                    == registration.digest().map_err(|_| JournalError::Conflict)?,
        )?;
        let (arm, receipt) = acknowledged_arm(current.outbox(), &registration.arm().operation_id)?;
        require(
            canonical(arm)? == canonical(registration.arm())?
                && canonical(receipt)? == canonical(registration.receipt())?,
        )?;
        ensure_unclaimed(current.outbox(), arm)?;
        let identity = RetirementIdentityV1::new(
            job.scope().clone(),
            operation,
            RetirementDigestV1::parse(job.intent_hash())?,
            arm.clone(),
            receipt.clone(),
            binding.registration_hash().clone(),
            unused.epoch(),
            unused.generation(),
            unused.retirement_id(),
            RetirementReasonV1::UnusedPrepareRevoked,
        )?;
        let evidence = RetirementEvidenceV1::new(
            identity.digest()?,
            unused.actor_nonce(),
            unused.revocation_nonce(),
        )?;
        let intention = event(
            job.intent(),
            intention_id,
            "broker.retirement.pending",
            serde_json::json!({
            "identity_hash":identity.digest()?,"evidence_hash":evidence.digest()? }),
        );
        let intention_entry = super::super::outbox::OutboxEventV1::new(
            operation,
            job.intent_hash(),
            intention.clone(),
        )?;
        // Bound the complete future closed Final record and its outbox event,
        // from the actual immutable metadata, before publishing any Pending.
        let event_bound = (canonical(&intention_entry)?.len() as u64)
            .checked_add(1024)
            .ok_or(JournalError::LimitExceeded)?;
        let provisional = PendingRetirementV1::new(
            identity.clone(),
            evidence.clone(),
            intention_id,
            RetirementDigestV1::parse(intention_entry.input_hash())?,
            final_id,
            CHECKPOINT,
            event_bound,
        )?;
        let final_bound = (canonical(&provisional)?.len() as u64)
            .checked_add(4096)
            .ok_or(JournalError::LimitExceeded)?;
        let pending = PendingRetirementV1::new(
            identity,
            evidence,
            intention_id,
            RetirementDigestV1::parse(intention_entry.input_hash())?,
            final_id,
            final_bound,
            event_bound,
        )?;
        let mut events = current.outbox().events().to_vec();
        events.push(intention_entry);
        let outbox = BrokerCheckpointV4::new(current.outbox().lifecycle().clone(), events)?;
        let mut entries = current.retirements().to_vec();
        entries.push(RetirementEntryV1::pending(pending.clone()));
        let next = BrokerCheckpointV5::new(outbox, current.bindings().to_vec(), entries)?;
        let receipt = self.advance_loaded_checkpoint(
            operation,
            expected,
            LoadedCheckpoint::V5(next),
            job,
            &mut writer,
        )?;
        Ok((pending, intention, receipt))
    }
}
fn full_outbox(job: &AcceptedJobV1) -> JournalResult<BrokerCheckpointV4> {
    if let Some(value) = job.broker_checkpoint_v4() {
        return Ok(value.clone());
    }
    let lifecycle = match job.broker_checkpoint_v3() {
        Some(value) => value.clone(),
        None => BrokerCheckpointV3::new(
            job.broker_checkpoint_v2()
                .cloned()
                .ok_or(JournalError::Conflict)?,
            Vec::new(),
        )?,
    };
    BrokerCheckpointV4::new(lifecycle, Vec::new())
}

pub(super) fn finalized_checkpoint(
    current: &BrokerCheckpointV5,
    intent: &ExecutionIntentV1,
    identity: &RetirementIdentityV1,
    receipt: RetiredReceiptV1,
    head: LedgerHeadV1,
    accepted: ClosingAcceptanceV1,
) -> JournalResult<BrokerCheckpointV5> {
    let index = current
        .retirements()
        .iter()
        .position(|entry| entry.request().identity().retirement_id() == identity.retirement_id())
        .ok_or(JournalError::Conflict)?;
    let entry = &current.retirements()[index];
    let pending = entry.request();
    let ack = crate::broker::retirement_checkpoint::audit_ack(current.outbox(), pending)?;
    let binding = current
        .bindings()
        .iter()
        .find(|binding| binding.arm_id().to_string() == identity.arm().operation_id)
        .ok_or(JournalError::Conflict)?;
    // C and the separate authenticated head are checked here while the opaque
    // Verified owner is consumed. Neither is added to the frozen Final wire.
    require(
        canonical(pending.identity())? == canonical(identity)?
            && canonical(accepted.identity())? == canonical(identity)?
            && canonical(accepted.evidence())? == canonical(pending.evidence())?
            && canonical(accepted.audit_ack())? == canonical(&ack)?
            && head.ledger().matches(binding.ledger())?
            && receipt.ledger().matches(binding.ledger())?
            && *head.revision() >= 3
            && receipt.finalization().closing_hash() == &accepted.digest()?
            && receipt.finalization().tombstone_hash()
                == &RetiredTombstoneV1::new(
                    binding.ledger().clone(),
                    identity.digest()?,
                    receipt.finalization().closing_hash().clone(),
                )?
                .digest()?,
    )?;
    if let Some(existing) = entry.finalized() {
        require(canonical(existing.receipt())? == canonical(&receipt)?)?;
        return Ok(current.clone());
    }
    let final_event = event(
        intent,
        pending.final_event_id(),
        "broker.retirement.final",
        serde_json::json!({
        "identity_hash":identity.digest()?,"receipt_hash":receipt.digest()? }),
    );
    let outbox_event = crate::broker::outbox::OutboxEventV1::new(
        *identity.job_operation_id(),
        identity.intent_hash().as_str(),
        final_event,
    )?;
    require(canonical(&outbox_event)?.len() as u64 <= pending.reserved_final_event_bytes())?;
    let finalized = FinalRetirementV1::new(
        pending.clone(),
        receipt,
        RetirementDigestV1::parse(outbox_event.input_hash())?,
    )?;
    let mut entries = current.retirements().to_vec();
    entries[index] = RetirementEntryV1::Final(Box::new(finalized));
    let mut events = current.outbox().events().to_vec();
    events.push(outbox_event);
    BrokerCheckpointV5::new(
        BrokerCheckpointV4::new(current.outbox().lifecycle().clone(), events)?,
        current.bindings().to_vec(),
        entries,
    )
}

#[derive(Default)]
struct Future {
    bytes: u64,
    entries: u32,
}
fn future(
    checkpoint: &BrokerCheckpointV5,
    checkpoint_bytes: u64,
    head: &RevisionRefV1,
    limits: &JournalLimitsV1,
) -> JournalResult<Future> {
    let mut revisions = 0_u64;
    let mut growth = 0_u64;
    let mut future_events = 0_usize;
    let mut future_outbox_bytes = 0_usize;
    for entry in checkpoint.retirements() {
        let pending = entry.request();
        let intention = checkpoint
            .outbox()
            .events()
            .iter()
            .find(|event| event.event_id() == pending.intention_event_id())
            .ok_or(JournalError::Corrupt)?;
        if entry.finalized().is_none() {
            future_events += 1;
            let event_growth = usize::try_from(pending.reserved_final_event_bytes())
                .map_err(|_| JournalError::LimitExceeded)?;
            future_outbox_bytes = future_outbox_bytes
                .checked_add(event_growth)
                .ok_or(JournalError::LimitExceeded)?;
            // Stored amounts bound the complete closed Final entry and event.
            // The current Pending bytes are already charged in this revision.
            let entry_growth = pending
                .reserved_final_checkpoint_bytes()
                .checked_sub(canonical(pending)?.len() as u64)
                .ok_or(JournalError::LimitExceeded)?;
            growth = growth
                .checked_add(entry_growth)
                .and_then(|bytes| bytes.checked_add(event_growth as u64))
                .ok_or(JournalError::LimitExceeded)?;
            revisions = revisions
                .checked_add(if intention.audit_seq().is_some() {
                    2
                } else {
                    3
                })
                .ok_or(JournalError::LimitExceeded)?;
        } else {
            let event = checkpoint
                .outbox()
                .events()
                .iter()
                .find(|event| event.event_id() == pending.final_event_id())
                .ok_or(JournalError::Corrupt)?;
            if event.audit_seq().is_none() {
                revisions += 1;
            }
        }
    }
    if revisions == 0 {
        return Ok(Future::default());
    }
    require(
        checkpoint
            .outbox()
            .events()
            .len()
            .checked_add(future_events)
            .is_some_and(|count| count <= 512),
    )?;
    // V4 uses a 4 MiB outbox budget. Reserve the actual bounded metadata
    // context and closed final payload, including future ACK decimal growth.
    if canonical(&checkpoint.outbox().events())?
        .len()
        .checked_add(future_outbox_bytes)
        .and_then(|bytes| {
            bytes.checked_add(40 * checkpoint.outbox().events().len() + 40 * future_events)
        })
        .is_none_or(|bytes| bytes > 4 * 1024 * 1024)
    {
        return Err(JournalError::LimitExceeded);
    }
    // Lifecycle permits at most 64 commands. Reserve every still-available
    // Cancel command, including its complete checkpoint copy and CAS pointer;
    // future ordinary mutations cannot spend that existing cancellation budget.
    let remaining_cancel = 64_usize
        .checked_sub(checkpoint.outbox().lifecycle().commands().len())
        .ok_or(JournalError::LimitExceeded)? as u64;
    revisions = revisions
        .checked_add(remaining_cancel)
        .ok_or(JournalError::LimitExceeded)?;
    let max_revision_bytes = checkpoint_bytes
        .checked_add(growth)
        .and_then(|bytes| {
            bytes.checked_add(
                1024 * remaining_cancel + 40 * checkpoint.outbox().events().len() as u64,
            )
        })
        .ok_or(JournalError::LimitExceeded)?;
    if max_revision_bytes > CHECKPOINT
        || head
            .revision
            .checked_add(revisions)
            .is_none_or(|last| last >= u64::from(limits.revisions))
    {
        return Err(JournalError::LimitExceeded);
    }
    let bytes = max_revision_bytes
        .checked_add(SMALL)
        .and_then(|bytes| bytes.checked_mul(revisions))
        .ok_or(JournalError::LimitExceeded)?;
    let entries = u32::try_from(
        revisions
            .checked_mul(2)
            .ok_or(JournalError::LimitExceeded)?,
    )
    .map_err(|_| JournalError::LimitExceeded)?;
    Ok(Future { bytes, entries })
}

/// Only these reviewed producers may spend a pre-existing promise while
/// another job's future metadata is unavailable. No caller-supplied bool.
pub(super) enum PublicationBudget {
    NewAllocation,
    Cancel,
    RetirementCompletion(PromisedCompletion),
}
pub(super) struct PromisedCompletion {
    operation_id: Uuid,
}
impl PublicationBudget {
    pub(super) fn retirement_final(
        operation: Uuid,
        current: &BrokerCheckpointV5,
        next: &BrokerCheckpointV5,
        verified: &crate::broker::enforcer::retirement::VerifiedRetirementReceipt,
    ) -> JournalResult<Self> {
        let retirement_id = *verified.identity().retirement_id();
        require(*verified.identity().job_operation_id() == operation)?;
        let index = current
            .retirements()
            .iter()
            .position(|entry| *entry.request().identity().retirement_id() == retirement_id)
            .ok_or(JournalError::Conflict)?;
        require(
            canonical(current.outbox().lifecycle())? == canonical(next.outbox().lifecycle())?
                && canonical(&current.bindings())? == canonical(&next.bindings())?
                && current.retirements().len() == next.retirements().len()
                && current.retirements()[index].request().audit_ack().is_some()
                && next.retirements()[index].finalized().is_some(),
        )?;
        for (position, (before, after)) in current
            .retirements()
            .iter()
            .zip(next.retirements())
            .enumerate()
        {
            if position != index {
                require(canonical(before)? == canonical(after)?)?;
            } else {
                require(canonical(before.request())? == canonical(after.request())?)?;
            }
        }
        let appended = usize::from(current.retirements()[index].finalized().is_none());
        require(next.outbox().events().len() == current.outbox().events().len() + appended)?;
        for (before, after) in current.outbox().events().iter().zip(next.outbox().events()) {
            require(canonical(before)? == canonical(after)?)?;
        }
        if appended == 1 {
            require(next.outbox().events().last().is_some_and(|event| {
                event.event_id() == current.retirements()[index].request().final_event_id()
            }))?;
        }
        next.validate_transition(Some(current))?;
        Ok(Self::RetirementCompletion(PromisedCompletion {
            operation_id: operation,
        }))
    }
    pub(super) fn retirement_ack(
        operation: Uuid,
        current: &BrokerCheckpointV5,
        next: &BrokerCheckpointV5,
        ids: &HashSet<Uuid>,
    ) -> JournalResult<Self> {
        require(
            canonical(current.outbox().lifecycle())? == canonical(next.outbox().lifecycle())?
                && canonical(&current.bindings())? == canonical(&next.bindings())?
                && current.outbox().events().len() == next.outbox().events().len()
                && current.retirements().len() == next.retirements().len(),
        )?;
        for (before, after) in current.outbox().events().iter().zip(next.outbox().events()) {
            if !ids.contains(&before.event_id()) {
                require(canonical(before)? == canonical(after)?)?;
            } else {
                require(current.retirements().iter().any(|entry| {
                    entry.request().intention_event_id() == before.event_id()
                        || (entry.finalized().is_some()
                            && entry.request().final_event_id() == before.event_id())
                }))?;
            }
        }
        next.validate_transition(Some(current))?;
        Ok(Self::RetirementCompletion(PromisedCompletion {
            operation_id: operation,
        }))
    }
    fn permits_unknown_others(&self, operation: Uuid) -> bool {
        match self {
            Self::NewAllocation => false,
            Self::Cancel => true,
            Self::RetirementCompletion(proof) => proof.operation_id == operation,
        }
    }
}

pub(super) fn reserve_future(
    core: &Inner,
    selected: Uuid,
    replacement: Option<(&RevisionRefV1, &[u8])>,
    usage: &Usage,
    publication: &PublicationBudget,
    floors: &HashMap<Uuid, RevisionRefV1>,
) -> JournalResult<()> {
    let independent_completion = publication.permits_unknown_others(selected);
    let note_unknown = || {
        core.budget_incomplete
            .store(true, std::sync::atomic::Ordering::SeqCst)
    };
    let mut projected = Usage {
        bytes: usage.bytes,
        entries: usage.entries,
        jobs: usage.jobs,
        job_bytes: usage.job_bytes,
        job_entries: usage.job_entries,
    };
    let mut visited = HashSet::new();
    for entry in fscheck(core.jobs.persistent_entries(core.limits.scope_entries))? {
        if !entry.directory {
            if independent_completion {
                note_unknown();
                continue;
            }
            return Err(JournalError::UnsafeFilesystem);
        }
        let operation = match Uuid::parse_str(&entry.name) {
            Ok(operation) if !operation.is_nil() && operation.to_string() == entry.name => {
                operation
            }
            _ if independent_completion => {
                note_unknown();
                continue;
            }
            _ => return Err(JournalError::Corrupt),
        };
        visited.insert(operation);
        let (record_bytes, current_head) = if operation == selected && replacement.is_some() {
            let (head, bytes) = replacement.ok_or(JournalError::Corrupt)?;
            (bytes.to_vec(), head.clone())
        } else {
            let record = (|| -> JournalResult<Option<(Vec<u8>, RevisionRefV1)>> {
                let directory = fscheck(core.jobs.persistent_child(&entry.name, false))?
                    .ok_or(JournalError::Corrupt)?;
                let Some(pointer) =
                    fscheck(directory.persistent_read("current.json", SMALL, &core.exclusions.0))?
                else {
                    require(!floors.contains_key(&operation))?;
                    let entries = fscheck(directory.persistent_entries(core.limits.job_entries))?;
                    require(entries.iter().all(|entry| {
                        !entry.directory
                            && matches!(entry.name.as_str(), "reservation.json" | "rejected.json")
                    }))?;
                    return Ok(None);
                };
                let current: Current = parse(pointer.bytes())?;
                require(
                    current.version == 1
                        && current.operation_id == operation
                        && canonical(&current)? == pointer.bytes()
                        && floors
                            .get(&operation)
                            .is_none_or(|floor| floor == &current.head),
                )?;
                let record = fscheck(directory.persistent_read(
                    &revision_name(current.head.revision),
                    CHECKPOINT,
                    &core.exclusions.0,
                ))?
                .ok_or(JournalError::Corrupt)?;
                let raw: Value = parse(record.bytes())?;
                let domain = match raw.get("version").and_then(Value::as_u64) {
                    Some(1) => "ai-guard:broker-checkpoint:v1",
                    Some(2) => "ai-guard:broker-checkpoint:v2",
                    Some(3) => "ai-guard:broker-checkpoint:v3",
                    Some(4) => "ai-guard:broker-checkpoint:v4",
                    Some(5) => "ai-guard:broker-checkpoint:v5",
                    _ => return Err(JournalError::UnsupportedVersion),
                };
                require(
                    raw.get("operation_id").and_then(Value::as_str) == Some(entry.name.as_str())
                        && raw.get("intent_hash").and_then(Value::as_str)
                            == Some(current.intent_hash.as_str())
                        && raw.get("revision").and_then(Value::as_u64)
                            == Some(current.head.revision)
                        && canonical(&raw)? == record.bytes()
                        && digest(domain, record.bytes()) == current.head.hash,
                )?;
                Ok(Some((record.bytes().to_vec(), current.head)))
            })();
            match record {
                Ok(Some(record)) => record,
                Ok(None) => continue,
                Err(_) if independent_completion && operation != selected => {
                    note_unknown();
                    continue;
                }
                Err(error) => return Err(error),
            }
        };
        // A malformed other job never disables a known independent Cancel.
        // The ordinary metadata inventory already charged its real bytes.
        // Unknown future promises forbid new Pending/other allocations.
        if independent_completion && operation != selected {
            let checked = (|| -> JournalResult<()> {
                let raw: Value = parse(&record_bytes)?;
                if raw.get("version").and_then(Value::as_u64) == Some(5) {
                    let _: RetirementCheckpoint = parse(&record_bytes)?;
                }
                Ok(())
            })();
            if checked.is_err() {
                note_unknown();
                continue;
            }
        }
        let raw: Value = parse(&record_bytes)?;
        match raw.get("version").and_then(Value::as_u64) {
            Some(1..=4) => continue,
            Some(5) => {}
            _ if independent_completion && operation != selected => {
                note_unknown();
                continue;
            }
            _ => return Err(JournalError::UnsupportedVersion),
        }
        let projected_future = (|| -> JournalResult<Future> {
            let checkpoint: RetirementCheckpoint = parse(&record_bytes)?;
            require(
                checkpoint.operation_id == operation
                    && checkpoint.revision == current_head.revision
                    && digest("ai-guard:broker-checkpoint:v5", &record_bytes) == current_head.hash,
            )?;
            future(
                &checkpoint.payload,
                record_bytes.len() as u64,
                &current_head,
                &core.limits,
            )
        })();
        let future = match projected_future {
            Ok(future) => future,
            Err(_) if independent_completion && operation != selected => {
                note_unknown();
                continue;
            }
            Err(error) => return Err(error),
        };
        // Revisions is deliberately computed per job above; scope has bytes
        // and entries rather than a shared revision cap.
        projected.add(
            future.bytes,
            future.entries,
            operation == selected,
            &core.limits,
        )?;
    }
    if floors.keys().any(|operation| !visited.contains(operation)) {
        if independent_completion {
            note_unknown();
        } else {
            return Err(JournalError::Conflict);
        }
    }
    Ok(())
}
