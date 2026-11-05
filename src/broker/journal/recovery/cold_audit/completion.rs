//! Exact metadata-only ACK merge on the prepared STOP journal and lease.
//!
//! The preparation is borrowed: any failure leaves its strong reservation
//! with the owner. After an attempted publication the future actor MUST retain
//! PreparedColdAudit and the exact receipts in strong quarantine before it
//! answers an error/Unknown. This primitive cannot enforce that actor lifetime.
//! It never stores PreparedColdAudit in Writer (which would cycle Arc<Inner>).
//! A sealed failed attempt stays closed until explicit recovery/recount; even
//! an error before the first filesystem write does not permit a blind retry.

use super::*;
use crate::{model::StoredEvent, runner::ExactStoredEventReceipt};

#[derive(Default)]
pub(super) struct CompletionProgress {
    attempt: Option<CompletionAttempt>,
}

struct CompletionAttempt {
    receipts_hash: String,
    target: RevisionRefV1,
}

/// Confirmed local metadata only. This grants no runtime, Full, HTTP or
/// authentication capability and cannot be constructed from a sequence alone.
pub(in crate::broker) struct ConfirmedColdAudit {
    operation_id: Uuid,
    head: RevisionRefV1,
    event_ids: Vec<Uuid>,
}

opaque!(CompletionProgress, CompletionAttempt, ConfirmedColdAudit);

impl ConfirmedColdAudit {
    pub(in crate::broker) fn operation_id(&self) -> Uuid {
        self.operation_id
    }

    pub(in crate::broker) fn head(&self) -> &RevisionRefV1 {
        &self.head
    }

    pub(in crate::broker) fn event_ids(&self) -> &[Uuid] {
        &self.event_ids
    }
}

struct BoundReceipt<'a> {
    pending: &'a OutboxEventV1,
    receipt: &'a ExactStoredEventReceipt,
}

pub(in crate::broker) fn complete(
    journal: &StopJournal,
    prepared: &PreparedColdAudit,
    receipts: &[ExactStoredEventReceipt],
) -> JournalResult<ConfirmedColdAudit> {
    if !Arc::ptr_eq(&journal.core, &prepared.owner) {
        return Err(JournalError::Conflict);
    }
    let mut writer = journal
        .core
        .writer
        .lock()
        .map_err(|_| JournalError::Corrupt)?;
    let active = writer.cold_audit.upgrade().ok_or(JournalError::Conflict)?;
    if !Arc::ptr_eq(&active, &prepared.reservation) || journal.scope_incomplete {
        return Err(JournalError::Conflict);
    }
    let reserved = &prepared.reservation;
    if reserved.operation_id != prepared.operation_id() || reserved.head != *prepared.head() {
        return Err(JournalError::Conflict);
    }
    let original = prepared
        .selected
        .metadata
        .as_ref()
        .ok_or(JournalError::Corrupt)?;
    if original.intent.operation_id() != reserved.operation_id
        || original
            .intent
            .intent_hash()
            .map_err(|_| JournalError::Corrupt)?
            != reserved.intent_hash
    {
        return Err(JournalError::Conflict);
    }
    let baseline = original
        .checkpoint
        .outbox()
        .ok_or(JournalError::UnsupportedVersion)?;
    baseline.validate_context(&original.intent)?;
    check_selected_pair(baseline)?;
    let bindings = bind_receipts(prepared, baseline, receipts)?;
    let receipts_hash = receipts_digest(&bindings)?;

    // Lock order is always core.writer -> completion. No inverse path or
    // network operation exists. Invalid receipt batches cannot seal a reserve.
    let mut progress = reserved
        .completion
        .lock()
        .map_err(|_| JournalError::Corrupt)?;
    if progress
        .attempt
        .as_ref()
        .is_some_and(|old| old.receipts_hash != receipts_hash)
    {
        return Err(JournalError::Conflict);
    }
    let (reader, current, incomplete) = journal
        .read_locked(reserved.operation_id, &mut writer)?
        .ok_or(JournalError::Conflict)?;
    if incomplete {
        return Err(JournalError::Interrupted);
    }
    validate_current(prepared, &current, &bindings)?;
    let metadata = current.metadata.as_ref().ok_or(JournalError::Corrupt)?;
    let outbox = metadata
        .checkpoint
        .outbox()
        .ok_or(JournalError::UnsupportedVersion)?;
    let all_acked = acknowledged(outbox, &bindings);

    if let Some(attempt) = &progress.attempt {
        // q=1 is already sealed. Only prove the same completed target (or
        // Cancel descendants); never call the publisher again on re-entry.
        if !all_acked || !descends(&current, &attempt.target)? {
            return Err(JournalError::Interrupted);
        }
        checked_scope(&journal.core, reserved, &mut writer)?;
        reader.check_shared()?;
        return confirmation(reserved.operation_id, current.head.clone(), &bindings);
    }
    if all_acked {
        checked_scope(&journal.core, reserved, &mut writer)?;
        reader.check_shared()?;
        progress.attempt = Some(CompletionAttempt {
            receipts_hash,
            target: current.head.clone(),
        });
        return confirmation(reserved.operation_id, current.head.clone(), &bindings);
    }

    let events = outbox
        .events()
        .iter()
        .map(|entry| {
            match bindings
                .iter()
                .find(|bound| bound.pending.event_id() == entry.event_id())
            {
                Some(bound) => entry.acknowledged(bound.receipt.seq()),
                None => Ok(entry.clone()),
            }
        })
        .collect::<JournalResult<Vec<_>>>()?;
    let next = LoadedCheckpoint::V4(BrokerCheckpointV4::new(outbox.lifecycle().clone(), events)?);
    confirmed::context_checkpoint_with_launch(
        &journal.core.scope,
        &journal.core.control,
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
    let (bytes, target) = encode_checkpoint(
        &journal.core,
        reserved.operation_id,
        &reserved.intent_hash,
        &current.head,
        &next,
    )?;
    let usage = publication_usage(&journal.core, reserved, &mut writer, &target, &bytes)?;
    reader.check_shared()?;
    // All validation/preflight is finished. Retain the exact lot and target
    // before the first possible publication effect, including ambiguous I/O.
    progress.attempt = Some(CompletionAttempt {
        receipts_hash,
        target: target.clone(),
    });
    publish_revision(
        &journal.core,
        reserved.operation_id,
        &reserved.intent_hash,
        &current.head,
        &target,
        &bytes,
        usage,
    )?;
    writer.floors.insert(reserved.operation_id, target.clone());
    writer.observed_jobs.insert(reserved.operation_id);
    let (reader, current, incomplete) = journal
        .read_locked(reserved.operation_id, &mut writer)?
        .ok_or(JournalError::Corrupt)?;
    if incomplete || current.head != target {
        return Err(JournalError::Interrupted);
    }
    validate_current(prepared, &current, &bindings)?;
    if !acknowledged(
        current
            .metadata
            .as_ref()
            .and_then(|m| m.checkpoint.outbox())
            .ok_or(JournalError::Corrupt)?,
        &bindings,
    ) {
        return Err(JournalError::Corrupt);
    }
    checked_scope(&journal.core, reserved, &mut writer)?;
    reader.check_shared()?;
    confirmation(reserved.operation_id, target, &bindings)
}

fn bind_receipts<'a>(
    prepared: &'a PreparedColdAudit,
    baseline: &BrokerCheckpointV4,
    receipts: &'a [ExactStoredEventReceipt],
) -> JournalResult<Vec<BoundReceipt<'a>>> {
    if receipts.is_empty() || receipts.len() > 2 || receipts.len() > prepared.pending_events.len() {
        return Err(JournalError::InvalidInput);
    }
    let pending = baseline
        .events()
        .iter()
        .filter(|entry| entry.audit_seq().is_none())
        .collect::<Vec<_>>();
    if pending.len() != prepared.pending_events.len() {
        return Err(JournalError::Conflict);
    }
    for (original, retained) in pending.iter().zip(&prepared.pending_events) {
        if canonical(original)? != canonical(retained)? {
            return Err(JournalError::Conflict);
        }
    }
    let mut seen = HashSet::new();
    // A known audit sequence identifies one Event. Neither sequence order nor
    // contiguity is required; unknown audit entries remain outside this map.
    let mut sequence_ids = HashMap::new();
    for entry in baseline.events() {
        if let Some(seq) = entry.audit_seq()
            && sequence_ids
                .insert(seq, entry.event_id())
                .is_some_and(|old| old != entry.event_id())
        {
            return Err(JournalError::Conflict);
        }
    }
    let mut bindings = Vec::new();
    for receipt in receipts {
        let stored = receipt.stored_event();
        let id = Uuid::parse_str(&stored.event.id).map_err(|_| JournalError::InvalidInput)?;
        if id.is_nil()
            || id.to_string() != stored.event.id
            || receipt.seq() <= 0
            || !seen.insert(id)
        {
            return Err(JournalError::InvalidInput);
        }
        if sequence_ids
            .insert(receipt.seq(), id)
            .is_some_and(|old| old != id)
        {
            return Err(JournalError::Conflict);
        }
        valid_hash(&stored.hash)?;
        valid_hash(&stored.prev_hash)?;
        if stored.received_at.len() > 64
            || chrono::DateTime::parse_from_rfc3339(&stored.received_at).is_err()
        {
            return Err(JournalError::InvalidInput);
        }
        let expected = prepared
            .pending_events
            .iter()
            .find(|entry| entry.event_id() == id)
            .ok_or(JournalError::Conflict)?;
        // Recompute the existing V1 outbox input hash over borrowed exact
        // Event data. No new event or transformed payload is constructed.
        #[derive(Serialize)]
        struct Input<'a> {
            version: u32,
            operation_id: Uuid,
            intent_hash: &'a str,
            event: &'a crate::model::Event,
        }
        let hash = digest(
            "ai-guard:broker-outbox-event:v1",
            &canonical(&Input {
                version: 1,
                operation_id: prepared.operation_id(),
                intent_hash: &prepared.reservation.intent_hash,
                event: &stored.event,
            })?,
        );
        if hash != expected.input_hash()
            || canonical(&stored.event)? != canonical(expected.event())?
        {
            return Err(JournalError::Conflict);
        }
        bindings.push(BoundReceipt {
            pending: expected,
            receipt,
        });
    }
    bindings.sort_unstable_by_key(|bound| bound.pending.event_id());
    Ok(bindings)
}

fn receipts_digest(bindings: &[BoundReceipt<'_>]) -> JournalResult<String> {
    let stored: Vec<&StoredEvent> = bindings
        .iter()
        .map(|bound| bound.receipt.stored_event())
        .collect();
    Ok(digest(
        "ai-guard:broker-cold-audit-completion:v1",
        &canonical(&stored)?,
    ))
}

fn descends(current: &RecoveredJobV1, head: &RevisionRefV1) -> JournalResult<bool> {
    let index = usize::try_from(head.revision).map_err(|_| JournalError::Corrupt)?;
    Ok(current.chain.get(index) == Some(head))
}

fn validate_current(
    prepared: &PreparedColdAudit,
    current: &RecoveredJobV1,
    bindings: &[BoundReceipt<'_>],
) -> JournalResult<()> {
    if current.operation_id != prepared.operation_id() || !descends(current, prepared.head())? {
        return Err(JournalError::Conflict);
    }
    let original = prepared
        .selected
        .metadata
        .as_ref()
        .ok_or(JournalError::Corrupt)?;
    let metadata = current.metadata.as_ref().ok_or(JournalError::Corrupt)?;
    if canonical(&original.intent)? != canonical(&metadata.intent)?
        || original.binding != metadata.binding
        || canonical(&original.launch_binding)? != canonical(&metadata.launch_binding)?
    {
        return Err(JournalError::Conflict);
    }
    let baseline = original
        .checkpoint
        .outbox()
        .ok_or(JournalError::UnsupportedVersion)?;
    let outbox = metadata
        .checkpoint
        .outbox()
        .ok_or(JournalError::UnsupportedVersion)?;
    outbox.validate_context(&metadata.intent)?;
    outbox
        .lifecycle()
        .validate_context(current.operation_id, &prepared.reservation.intent_hash)?;
    // Freeze all business/runner/supervision data. Only appended Cancel
    // commands and this exact receipt lot may differ from preparation.
    if canonical(baseline.lifecycle().broker())? != canonical(outbox.lifecycle().broker())? {
        return Err(JournalError::Conflict);
    }
    let old_commands = baseline.lifecycle().commands();
    let commands = outbox.lifecycle().commands();
    let prefix = commands
        .get(..old_commands.len())
        .ok_or(JournalError::Conflict)?;
    let suffix = commands
        .get(old_commands.len()..)
        .ok_or(JournalError::Conflict)?;
    if canonical(&prefix)? != canonical(&old_commands)?
        || suffix
            .iter()
            .any(|command| !matches!(command.request(), LocalCommandRequestV1::CancelRequested))
        || baseline.events().len() != outbox.events().len()
    {
        return Err(JournalError::Conflict);
    }
    for (old, entry) in baseline.events().iter().zip(outbox.events()) {
        if old.event_id() != entry.event_id()
            || old.input_hash() != entry.input_hash()
            || canonical(old.event())? != canonical(entry.event())?
        {
            return Err(JournalError::Conflict);
        }
        let seq = bindings
            .iter()
            .find(|bound| bound.pending.event_id() == old.event_id())
            .map(|bound| bound.receipt.seq());
        match (old.audit_seq(), entry.audit_seq(), seq) {
            (Some(old), Some(current), _) if old == current => {}
            (None, None, _) => {}
            (None, Some(current), Some(received)) if current == received => {}
            _ => return Err(JournalError::Conflict),
        }
    }
    Ok(())
}

fn acknowledged(outbox: &BrokerCheckpointV4, bindings: &[BoundReceipt<'_>]) -> bool {
    bindings.iter().all(|bound| {
        outbox.events().iter().any(|entry| {
            entry.event_id() == bound.pending.event_id()
                && entry.audit_seq() == Some(bound.receipt.seq())
        })
    })
}

fn publication_usage(
    core: &Inner,
    reserved: &ReservationState,
    writer: &mut Writer,
    target: &RevisionRefV1,
    bytes: &[u8],
) -> JournalResult<Usage> {
    let mut selected = reserved.jobs.iter().filter(|job| job.completion_bytes != 0);
    let bound = selected.next().ok_or(JournalError::Conflict)?;
    if selected.next().is_some()
        || bound.operation_id != reserved.operation_id
        || bound.head != reserved.head
    {
        return Err(JournalError::Conflict);
    }
    let physical = checked_scope(core, reserved, writer)?;
    let job = physical
        .jobs
        .get(&reserved.operation_id)
        .ok_or(JournalError::Conflict)?;
    let usage = Usage {
        bytes: physical.bytes,
        entries: physical.entries,
        jobs: u32::try_from(physical.jobs.len()).map_err(|_| JournalError::LimitExceeded)?,
        job_bytes: job.bytes,
        job_entries: job.entries,
    };
    let pointer = checked_core_record(
        core,
        &Current {
            version: 1,
            operation_id: reserved.operation_id,
            intent_hash: reserved.intent_hash.clone(),
            head: target.clone(),
        },
        SMALL,
    )?;
    let cost = add(
        u64::try_from(bytes.len()).map_err(|_| JournalError::LimitExceeded)?,
        u64::try_from(pointer.len()).map_err(|_| JournalError::LimitExceeded)?,
    )?;
    if cost > bound.completion_bytes || target.revision >= u64::from(core.limits.revisions) {
        return Err(JournalError::LimitExceeded);
    }
    // Check the actual cost before sealing q=1, without renewing the original
    // reserve or promising physical free space. Publisher applies these same
    // byte/entry limits again before its first create.
    let mut check = Usage {
        bytes: usage.bytes,
        entries: usage.entries,
        jobs: usage.jobs,
        job_bytes: usage.job_bytes,
        job_entries: usage.job_entries,
    };
    check.add(cost, 2, true, &core.limits)?;
    Ok(usage)
}

fn checked_scope(
    core: &Inner,
    reserved: &ReservationState,
    writer: &mut Writer,
) -> JournalResult<PhysicalUsage> {
    let mut physical = inventory_usage(core)?;
    if physical.jobs.len() != reserved.physical_job_ids.len()
        || physical
            .jobs
            .keys()
            .any(|id| !reserved.physical_job_ids.contains(id))
        || writer.observed_jobs != reserved.physical_job_ids
        || writer
            .floors
            .keys()
            .chain(writer.rejected_floors.keys())
            .any(|id| !reserved.physical_job_ids.contains(id))
    {
        return Err(JournalError::Conflict);
    }
    // IDs alone do not attest heads: an old, fully valid sibling prefix may
    // have been restored below a retained floor. Read every snapshot job with
    // the same strict checks as preparation, without N scope inventories or
    // keeping N payloads. Cost is bounded by the scope's metadata/revisions;
    // no latency or physical I/O guarantee follows from these logical caps.
    let reader = StopRecoveryReaderV1::from_core(core)?;
    let mut ids: Vec<_> = reserved.physical_job_ids.iter().copied().collect();
    ids.sort_unstable();
    for id in ids {
        drop(read_scope_job_locked(&reader, id, &mut physical, writer)?);
    }
    reader.check_shared()?;
    Ok(physical)
}

fn confirmation(
    operation_id: Uuid,
    head: RevisionRefV1,
    bindings: &[BoundReceipt<'_>],
) -> JournalResult<ConfirmedColdAudit> {
    #[cfg(test)]
    stop_before_ack()?;
    Ok(ConfirmedColdAudit {
        operation_id,
        head,
        event_ids: bindings
            .iter()
            .map(|bound| bound.pending.event_id())
            .collect(),
    })
}
