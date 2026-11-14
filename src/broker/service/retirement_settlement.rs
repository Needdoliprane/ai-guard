//! Same-process convergence of an actual Submitted keeper through Q and Final.
//! Only a genuine Q or the writer's retained input can produce the same Final;
//! RetireUnused is never retransmitted. The caller owns the real Job gate.
use super::{BrokerErrorCodeV1, EnforcerFullClientV3, StopWriterClient, jobs};
use crate::broker::{
    journal::{CheckpointReceiptV1, StopJobSnapshotV1, canonical},
    retirement::RetirementDigestV1,
    service::preclaim::SubmittedRetirementSettlement,
    writer::{FinalRetirementRecovery, RetirementFailure},
};
use std::sync::Arc;
use uuid::Uuid;

struct ConfirmedPending {
    request: Vec<u8>,
    receipt: Option<RetirementDigestV1>,
}

fn confirmed_pending(
    continuation: &SubmittedRetirementSettlement,
    snapshot: &StopJobSnapshotV1,
) -> Result<ConfirmedPending, BrokerErrorCodeV1> {
    let owner = continuation.owner();
    if snapshot.incomplete()
        || !owner.matches_recovery_origin(snapshot)
        || snapshot.operation_id() != *owner.identity().job_operation_id()
        || canonical(snapshot.scope()).map_err(jobs::journal_error)?
            != canonical(owner.identity().scope()).map_err(jobs::journal_error)?
    {
        return Err(BrokerErrorCodeV1::NeedsReconciliation);
    }
    let mut matches = snapshot.retirements().iter().filter(|entry| {
        entry.request().identity().retirement_id() == owner.identity().retirement_id()
    });
    let entry = matches
        .next()
        .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
    if matches.next().is_some() {
        return Err(BrokerErrorCodeV1::NeedsReconciliation);
    }
    let pending = entry.request();
    if pending.identity().digest().map_err(jobs::journal_error)?
        != owner.identity().digest().map_err(jobs::journal_error)?
        || pending.evidence().digest().map_err(jobs::journal_error)?
            != owner.evidence().digest().map_err(jobs::journal_error)?
        || canonical(
            pending
                .audit_ack()
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?,
        )
        .map_err(jobs::journal_error)?
            != canonical(owner.audit_ack()).map_err(jobs::journal_error)?
    {
        return Err(BrokerErrorCodeV1::NeedsReconciliation);
    }
    Ok(ConfirmedPending {
        request: canonical(pending).map_err(jobs::journal_error)?,
        receipt: entry
            .finalized()
            .map(|finalized| finalized.receipt().digest().map_err(jobs::journal_error))
            .transpose()?,
    })
}

fn observe_final_registry(
    writer: &StopWriterClient,
    continuation: &SubmittedRetirementSettlement,
    snapshot: &StopJobSnapshotV1,
) -> Result<(), BrokerErrorCodeV1> {
    let identity = continuation.owner().identity();
    let arm = Uuid::parse_str(&identity.arm().operation_id)
        .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
    match writer.recover_final_retirement_state(
        *identity.job_operation_id(),
        arm,
        *identity.retirement_id(),
    ) {
        // Absent is not evidence. The caller has independently required the
        // original-core F, and still needs Q plus actual local removal.
        Ok(FinalRetirementRecovery::Absent) => Ok(()),
        Ok(FinalRetirementRecovery::Completed(receipt)) => {
            if receipt.operation_id() != snapshot.operation_id()
                || snapshot.head().revision() < receipt.head().revision()
                || (snapshot.head().revision() == receipt.head().revision()
                    && snapshot.head() != receipt.head())
            {
                return Err(BrokerErrorCodeV1::NeedsReconciliation);
            }
            Ok(())
        }
        // InFlight, Refused and Unknown still own their original input. Do not
        // retry, manufacture a replacement wrapper, or fall through to Final.
        Ok(FinalRetirementRecovery::InFlight) | Err(_) => {
            Err(BrokerErrorCodeV1::NeedsReconciliation)
        }
    }
}

fn same_canonical_owner(
    guard: &jobs::RetirementQueryGuard,
    continuation: &SubmittedRetirementSettlement,
) -> Result<(), BrokerErrorCodeV1> {
    let preclaim = &guard.job().preclaim;
    let identity = continuation.owner().identity();
    let selected = preclaim
        .submitted_retirement_for_settlement(identity)?
        .or(preclaim.retained_final_for_settlement(identity)?);
    if !selected.is_some_and(|selected| Arc::ptr_eq(selected.owner(), continuation.owner())) {
        return Err(BrokerErrorCodeV1::NeedsReconciliation);
    }
    Ok(())
}

async fn reload_pending(
    writer: &StopWriterClient,
    continuation: &SubmittedRetirementSettlement,
    before: &StopJobSnapshotV1,
    proof: &ConfirmedPending,
) -> Result<(StopJobSnapshotV1, ConfirmedPending), BrokerErrorCodeV1> {
    let current = writer
        .recovery_status(before.operation_id())
        .await
        .map_err(jobs::journal_error)?
        .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
    let after = confirmed_pending(continuation, &current)?;
    if current.head().revision() < before.head().revision()
        || (current.head().revision() == before.head().revision() && current.head() != before.head())
        || proof.request != after.request
        || (proof.receipt.is_some() && proof.receipt != after.receipt)
    {
        return Err(BrokerErrorCodeV1::NeedsReconciliation);
    }
    Ok((current, after))
}

fn confirms_writer_receipt(
    snapshot: &StopJobSnapshotV1,
    proof: &ConfirmedPending,
    receipt: &CheckpointReceiptV1,
) -> Result<(), BrokerErrorCodeV1> {
    if proof.receipt.is_none()
        || receipt.operation_id() != snapshot.operation_id()
        || snapshot.head().revision() < receipt.head().revision()
        || (snapshot.head().revision() == receipt.head().revision()
            && snapshot.head() != receipt.head())
    {
        return Err(BrokerErrorCodeV1::NeedsReconciliation);
    }
    Ok(())
}

async fn recover_final(
    guard: &jobs::RetirementQueryGuard,
    writer: &StopWriterClient,
    continuation: &SubmittedRetirementSettlement,
) -> Result<Option<CheckpointReceiptV1>, BrokerErrorCodeV1> {
    let identity = continuation.owner().identity();
    let arm = Uuid::parse_str(&identity.arm().operation_id)
        .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
    match writer.recover_final_retirement_state(
        *identity.job_operation_id(),
        arm,
        *identity.retirement_id(),
    ) {
        Ok(FinalRetirementRecovery::Absent) => Ok(None),
        Ok(FinalRetirementRecovery::Completed(receipt)) => Ok(Some(receipt)),
        Ok(FinalRetirementRecovery::InFlight) => Err(BrokerErrorCodeV1::NeedsReconciliation),
        Err(RetirementFailure::Refused { continuation: retained, .. }) => {
            // This ticket retains the same writer input; no Q is queried to
            // replace it, and dropping the waiter cannot cancel that input.
            match writer.retry_final_retirement(retained).await {
                Ok(receipt) => Ok(Some(receipt)),
                Err(RetirementFailure::NotQueued { input, error }) => {
                    // Keep even an unexpected future NotQueued return. Current
                    // retry ports retain that input inside the same registry.
                    let token = guard
                        .job()
                        .preclaim
                        .reserve_final_input(guard, continuation, input)?;
                    drop(token);
                    Err(jobs::journal_error(error))
                }
                Err(failure) => Err(jobs::journal_error(failure.error())),
            }
        }
        // A caller timeout is not necessarily State::Unknown. Only a later
        // actual registry recovery can establish Completed/Refused/InFlight.
        Err(failure) => Err(jobs::journal_error(failure.error())),
    }
}

pub(super) async fn settle_confirmed_final(
    guard: &jobs::RetirementQueryGuard,
    writer: &StopWriterClient,
    full: &EnforcerFullClientV3,
    initial: &StopJobSnapshotV1,
    retirement: Uuid,
) -> Result<(), BrokerErrorCodeV1> {
    let operation = guard.operation_id();
    if initial.operation_id() != operation {
        return Err(BrokerErrorCodeV1::NeedsReconciliation);
    }
    let mut entries = initial
        .retirements()
        .iter()
        .filter(|entry| *entry.request().identity().retirement_id() == retirement);
    let entry = entries
        .next()
        .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
    if entries.next().is_some() {
        return Err(BrokerErrorCodeV1::NeedsReconciliation);
    }
    let local = guard.job();
    let normal = local
        .preclaim
        .submitted_retirement_for_settlement(entry.request().identity())?;
    let retained_input = normal.is_none();
    let continuation = match normal {
        Some(continuation) => continuation,
        None => local
            .preclaim
            .retained_final_for_settlement(entry.request().identity())?
            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?,
    };
    let before = confirmed_pending(&continuation, initial)?;
    // Absence cannot prove a previous removal, and this tranche never queries
    // on behalf of such an out-of-scope keeper. Presence is only an early
    // refusal check; the actual matching-slot removal is still required below.
    if !full
        .has_local_slot(&continuation.owner().identity().arm().operation_id)
        .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
    {
        return Err(BrokerErrorCodeV1::NeedsReconciliation);
    }
    // Preserve the already-durable F path, including its refusal of a plain
    // retained observation. Only the separate private handoff lane can retry.
    if before.receipt.is_some() && !retained_input {
        return cleanup_confirmed_final(guard, writer, full, initial, &continuation).await;
    }
    let recovered = recover_final(guard, writer, &continuation).await?;
    // Completed recovery consumes its registry entry. Absent therefore never
    // proves that no F exists: always reload the original physical Journal.
    let (mut current, mut proof) =
        reload_pending(writer, &continuation, initial, &before).await?;
    same_canonical_owner(guard, &continuation)?;
    if let Some(receipt) = recovered.as_ref() {
        confirms_writer_receipt(&current, &proof, receipt)?;
    }
    let mut input = match local.preclaim.take_retained_final_input(guard, &continuation)? {
        Some(input) => input,
        None if proof.receipt.is_some() => {
            return cleanup_confirmed_final(guard, writer, full, &current, &continuation).await;
        }
        None => {
            let verified = full
                .query_retirement_retained(continuation.owner())
                .await
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
            let reloaded = reload_pending(writer, &continuation, &current, &proof).await?;
            current = reloaded.0;
            proof = reloaded.1;
            same_canonical_owner(guard, &continuation)?;
            if proof.receipt.is_some() {
                // This is only a fresh read wrapper, not a retained/admitted
                // Final input. A late original F needs cleanup, not admission.
                drop(verified);
                return cleanup_confirmed_final(guard, writer, full, &current, &continuation).await;
            }
            local.preclaim.reserve_final_input(guard, &continuation, verified)?
        }
    };
    let expected = input
        .input()?
        .receipt()
        .digest()
        .map_err(jobs::journal_error)?;
    if proof.receipt.as_ref().is_some_and(|receipt| receipt != &expected) {
        return Err(BrokerErrorCodeV1::NeedsReconciliation);
    }
    // No await separates taking the actual input from the writer's existing
    // synchronous enqueue-before-await. On cancellation its registry owns it.
    let receipt = match writer.finalize_retirement(input.take_input()?).await {
        Ok(receipt) => receipt,
        Err(RetirementFailure::NotQueued { input: returned, error }) => {
            input.retain_returned(returned)?;
            return Err(jobs::journal_error(error));
        }
        // Refused/Unknown keep the actual input in the writer registry. The
        // empty local token releases only its Borrowed marker on Drop.
        Err(failure) => return Err(jobs::journal_error(failure.error())),
    };
    drop(input);
    let (current, after) = reload_pending(writer, &continuation, &current, &proof).await?;
    confirms_writer_receipt(&current, &after, &receipt)?;
    if after.receipt.as_ref() != Some(&expected) {
        return Err(BrokerErrorCodeV1::NeedsReconciliation);
    }
    cleanup_confirmed_final(guard, writer, full, &current, &continuation).await
}

async fn cleanup_confirmed_final(
    guard: &jobs::RetirementQueryGuard,
    writer: &StopWriterClient,
    full: &EnforcerFullClientV3,
    initial: &StopJobSnapshotV1,
    continuation: &SubmittedRetirementSettlement,
) -> Result<(), BrokerErrorCodeV1> {
    let before = confirmed_pending(continuation, initial)?;
    let before_receipt = before
        .receipt
        .as_ref()
        .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
    observe_final_registry(writer, continuation, initial)?;
    let mut verified = full
        .query_retirement_retained(continuation.owner())
        .await
        .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
        .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
    let current = writer
        .recovery_status(guard.operation_id())
        .await
        .map_err(jobs::journal_error)?
        .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
    let after = confirmed_pending(continuation, &current)?;
    if current.head().revision() < initial.head().revision()
        || (current.head().revision() == initial.head().revision()
            && current.head() != initial.head())
        || before.request != after.request
        || after.receipt.as_ref() != Some(before_receipt)
        || Some(verified.receipt().digest().map_err(jobs::journal_error)?) != after.receipt
    {
        return Err(BrokerErrorCodeV1::NeedsReconciliation);
    }
    observe_final_registry(writer, continuation, &current)?;
    // No await follows the canonical-holder recheck. Only the existing client
    // can seal actual present-slot removal; no sealed Q is stored in its owner.
    guard.job()
        .preclaim
        .finish_retirement_settlement(continuation, &mut verified, |owner, receipt| {
            full.retire_retained_local_after_final(owner, receipt, &current)
        })
}
