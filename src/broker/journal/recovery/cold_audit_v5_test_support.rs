//! Assertions over real metadata borrowed by the native retirement fixture.
//! This helper creates sizing copies only; it mints no authority or receipt.
use super::*;

pub(in crate::broker) fn assert_real_v5_bound(
    stops: &StopJournal,
    operation: Uuid,
    selected: bool,
) -> JournalResult<()> {
    let mut writer = stops
        .core
        .writer
        .lock()
        .map_err(|_| JournalError::Corrupt)?;
    let (_, recovered, incomplete) = stops
        .read_locked(operation, &mut writer)?
        .ok_or(JournalError::Corrupt)?;
    assert!(!incomplete);
    let metadata = recovered.metadata.as_ref().ok_or(JournalError::Corrupt)?;
    let checkpoint = metadata
        .checkpoint
        .retirement()
        .ok_or(JournalError::Conflict)?;
    assert!(
        !checkpoint.bindings().is_empty(),
        "measure actual nonempty registrations"
    );
    let before = canonical(checkpoint)?;
    let bound = estimate_job(
        operation,
        &recovered.head,
        metadata,
        selected,
        &stops.core.limits,
    )?;
    let events = checkpoint
        .outbox()
        .events()
        .iter()
        .map(|entry| {
            if selected && entry.audit_seq().is_none() {
                entry.acknowledged(i64::MAX)
            } else {
                Ok(entry.clone())
            }
        })
        .collect::<JournalResult<Vec<_>>>()?;
    let maximum = checkpoint.with_outbox(BrokerCheckpointV4::new(
        checkpoint.outbox().lifecycle().clone(),
        events,
    )?)?;
    let expected = RevisionRefV1::new(u64::MAX - 1, recovered.head.hash())?;
    let (encoded, head) = encode_checkpoint(
        &stops.core,
        operation,
        &metadata
            .intent
            .intent_hash()
            .map_err(|_| JournalError::Corrupt)?,
        &expected,
        &LoadedCheckpoint::V5(maximum),
    )?;
    assert_eq!(head.revision(), u64::MAX);
    assert_eq!(
        encoded.len() as u64,
        bound.checkpoint_bytes,
        "the borrowed V5 estimate must equal the real complete maximum-width envelope"
    );
    assert_eq!(
        before,
        canonical(checkpoint)?,
        "counting never mutates retained V5 data"
    );
    assert!(bound.cancel_delta_bytes <= 1024);
    if checkpoint
        .retirements()
        .iter()
        .any(|entry| entry.finalized().is_none())
    {
        assert!(
            bound.retirement_bytes >= bound.cancel_bytes,
            "D5 combined promise must include remaining Cancels as well as Final"
        );
        assert!(bound.retirement_revisions >= bound.remaining_cancels);
    }
    if selected {
        let physical = inventory_usage(&stops.core)?;
        assert_eq!(
            physical.jobs.len(),
            1,
            "boundary fixture is one fully accounted real job"
        );
        let actual = physical.jobs.get(&operation).ok_or(JournalError::Corrupt)?;
        let future = bound
            .remaining_cancels
            .max(bound.retirement_revisions)
            .checked_add(1)
            .ok_or(JournalError::LimitExceeded)?;
        let bytes = bound
            .cancel_bytes
            .max(bound.retirement_bytes)
            .checked_add(bound.completion_bytes)
            .ok_or(JournalError::LimitExceeded)?;
        let extra_entries =
            u32::try_from(future.checked_mul(2).ok_or(JournalError::LimitExceeded)?)
                .map_err(|_| JournalError::LimitExceeded)?;
        let mut exact = stops.core.limits.clone();
        exact.job_bytes = actual
            .bytes
            .checked_add(bytes)
            .ok_or(JournalError::LimitExceeded)?;
        exact.job_entries = actual
            .entries
            .checked_add(extra_entries)
            .ok_or(JournalError::LimitExceeded)?;
        exact.revisions = u32::try_from(
            recovered
                .head
                .revision()
                .checked_add(future)
                .and_then(|n| n.checked_add(1))
                .ok_or(JournalError::LimitExceeded)?,
        )
        .map_err(|_| JournalError::LimitExceeded)?;
        exact.scope_bytes = physical
            .bytes
            .checked_add(bytes)
            .ok_or(JournalError::LimitExceeded)?;
        exact.scope_entries = physical
            .entries
            .checked_add(extra_entries)
            .ok_or(JournalError::LimitExceeded)?;
        assert!(
            check_capacity(&exact, &physical, std::slice::from_ref(&bound)).is_ok(),
            "combined real Pending/Cancel/Cold upper bound fits at its exact boundary before HTTP"
        );
        exact.job_bytes -= 1;
        assert_eq!(
            check_capacity(&exact, &physical, std::slice::from_ref(&bound)),
            Err(JournalError::LimitExceeded),
            "one byte less must refuse before a publication can be attempted"
        );
    }
    Ok(())
}
