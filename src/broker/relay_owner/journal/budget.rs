//! Logical record reservation and FULL-envelope upper bounds before cleanup.
//! The synthetic shapes below are size estimates only, never persisted proofs.
use super::*;
use crate::relay::admission::*;

fn fits(job: Uuid, change: ChannelChangeV2) -> Result<()> {
    let bytes = super::super::super::journal::canonical(&RecordV2 {
        version: 2,
        sequence: MAX_FACTS as u64,
        previous_sha256: Some("f".repeat(64)),
        job_id: job,
        observed_at: "x".repeat(64),
        change,
    })
    .map_err(|_| RelayOwnerError::InvalidMaterial)?;
    if bytes.len() > MAX_RECORD {
        return Err(RelayOwnerError::Capacity);
    }
    Ok(())
}

pub(super) fn before_cleanup(
    history: &ChannelHistoryV2,
    job: Uuid,
    request: &RelayChannelTransitionV1,
) -> Result<()> {
    let unresolved = history
        .operations
        .iter()
        .filter(|op| op.resolution.is_none())
        .count();
    let cleanups = history
        .operations
        .iter()
        .filter(|op| op.purpose == ChannelOperationPurposeV2::Retirement)
        .count();
    if cleanups > 3 {
        return Err(RelayOwnerError::Capacity);
    }
    // Every unresolved identity may need one proof; preserve finalization and
    // both records for each remaining monotone cleanup attempt, plus at most
    // three additional snapshot-only advances. Begin already includes Unknown.
    let required = unresolved + 1 + (3 - cleanups) * 2 + 3;
    if history.terminal_records.saturating_add(required) > TERMINAL
        || history.ordinary_records + history.terminal_records + required > PER_JOB
    {
        return Err(RelayOwnerError::Capacity);
    }
    let admission = history
        .fact
        .admission
        .as_ref()
        .ok_or(RelayOwnerError::Scope)?;
    let mut channel = history.fact.core.clone().ok_or(RelayOwnerError::Scope)?;
    // Longest state spelling and numeric encodings bound every possible
    // successful/replayed result at the same immutable channel identity.
    channel.state = RelayChannelStateV1::KnownRefusedBeforeSpawn;
    channel.revision = i64::MAX as u64 - 1;
    let receipt = RelayChannelReceiptV1 {
        version: 1,
        operation_id: request.operation_id.clone(),
        channel_id: request.channel_id.clone(),
        epoch: request.epoch,
        input_hash: request.input_hash().map_err(|_| RelayOwnerError::Scope)?,
        audit_seq: i64::MAX,
        audit_hash: "f".repeat(64),
    };
    let mut admission_receipt = match history.admission_proof.as_ref().map(|proof| &proof.outcome) {
        Some(RelayChannelHistoryOutcomeV1::Historical {
            admission_receipt, ..
        }) => admission_receipt.clone(),
        _ => return Err(RelayOwnerError::Scope),
    };
    admission_receipt.audit_seq = i64::MAX;
    admission_receipt.audit_hash = "f".repeat(64);
    fits(
        job,
        ChannelChangeV2::Mutation {
            operation_id: request.operation_id.clone(),
            response: RelayChannelResultV1 {
                version: 1,
                replayed: false,
                channel: channel.clone(),
                receipt: receipt.clone(),
            },
        },
    )?;
    fits(
        job,
        ChannelChangeV2::History {
            pending_operation_id: Some(request.operation_id.clone()),
            response: RelayChannelReconciliationV1 {
                version: 1,
                tenant_id: channel.tenant_id.clone(),
                control_namespace_uuid: admission.control_namespace_uuid.clone(),
                admission_input_hash: admission.input_hash().map_err(|_| RelayOwnerError::Scope)?,
                audit_seq: i64::MAX,
                audit_hash: "f".repeat(64),
                outcome: RelayChannelHistoryOutcomeV1::Historical {
                    channel: channel.clone(),
                    admission_receipt,
                    pending: Some(RelayChannelPendingHistoryV1 {
                        pending: RelayChannelPendingV1::Transition(request.clone()),
                        resolution: RelayChannelPendingResolutionV1::Recorded { receipt },
                    }),
                },
            },
        },
    )?;
    let mut fact = history.fact.clone();
    fact.core = Some(channel);
    fact.phase = AttachmentPhase::Closed;
    fact.error = None;
    fact.local_joined = true;
    fact.observed_at = "x".repeat(64);
    fits(job, ChannelChangeV2::Finalize { fact })
}
