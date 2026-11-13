//! Same-process cleanup of an actual Submitted keeper after an exact durable F.
//! This path never submits/retries RetireUnused or Final, and never reconstructs
//! an owner from metadata. The caller owns the real Job gate for the whole round.
use super::{BrokerErrorCodeV1, EnforcerFullClientV3, StopWriterClient, jobs};
use crate::broker::{
    journal::{StopJobSnapshotV1, canonical},
    retirement::RetirementDigestV1,
    service::preclaim::SubmittedRetirementSettlement,
    writer::FinalRetirementRecovery,
};
use uuid::Uuid;

struct ConfirmedFinal {
    request: Vec<u8>,
    receipt: RetirementDigestV1,
}

fn confirmed_final(
    continuation: &SubmittedRetirementSettlement,
    snapshot: &StopJobSnapshotV1,
) -> Result<ConfirmedFinal, BrokerErrorCodeV1> {
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
    let finalized = entry
        .finalized()
        .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
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
    Ok(ConfirmedFinal {
        request: canonical(pending).map_err(jobs::journal_error)?,
        receipt: finalized.receipt().digest().map_err(jobs::journal_error)?,
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
    let continuation = local
        .preclaim
        .submitted_retirement_for_settlement(entry.request().identity())?
        .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
    let before = confirmed_final(&continuation, initial)?;
    // Absence cannot prove a previous removal, and this tranche never queries
    // on behalf of such an out-of-scope keeper. Presence is only an early
    // refusal check; the actual matching-slot removal is still required below.
    if !full
        .has_local_slot(&continuation.owner().identity().arm().operation_id)
        .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
    {
        return Err(BrokerErrorCodeV1::NeedsReconciliation);
    }
    observe_final_registry(writer, &continuation, initial)?;
    let mut verified = full
        .query_retirement_retained(continuation.owner())
        .await
        .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
        .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
    let current = writer
        .recovery_status(operation)
        .await
        .map_err(jobs::journal_error)?
        .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
    let after = confirmed_final(&continuation, &current)?;
    if current.head().revision() < initial.head().revision()
        || (current.head().revision() == initial.head().revision()
            && current.head() != initial.head())
        || before.request != after.request
        || before.receipt != after.receipt
        || verified.receipt().digest().map_err(jobs::journal_error)? != after.receipt
    {
        return Err(BrokerErrorCodeV1::NeedsReconciliation);
    }
    observe_final_registry(writer, &continuation, &current)?;
    // No await follows the canonical-holder recheck. Only the existing client
    // can seal actual present-slot removal; no sealed Q is stored in its owner.
    local
        .preclaim
        .finish_retirement_settlement(&continuation, &mut verified, |owner, receipt| {
            full.retire_retained_local_after_final(owner, receipt, &current)
        })
}
