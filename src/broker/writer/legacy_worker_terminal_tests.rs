use super::*;
use crate::{
    broker::journal::canonical, control::ArmSubjectV1,
    runner::broker::running_seed_for_cold_terminal_test,
};
use anyhow::Context;

#[tokio::test]
async fn stops_writer_reads_the_exact_legacy_worker_after_durable_cancel() -> anyhow::Result<()> {
    let seed = running_seed_for_cold_terminal_test().await;
    let root = Directory::open_existing(&seed.journal_root)?;
    let report = StopRecoveryReaderV1::open_existing(
        &root,
        seed.scope().clone(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new())?,
    )?
    .context("retained journal lease missing")?
    .scan();
    let capabilities =
        ProfileWriterCapabilities::from_recovery_report(&report, true, 1, Duration::from_secs(10))?;
    assert!(capabilities.is_stop_only());
    let stops = capabilities.stops();
    drop(report);

    let operation = seed.operation_id;
    let command = Uuid::new_v4();
    let before = stops
        .recovery_status(operation)
        .await?
        .context("Running Legacy job missing")?;
    let retained = before
        .stops()
        .iter()
        .find(|target| matches!(&target.arm().subject, ArmSubjectV1::Worker))
        .context("retained worker Arm missing")?;
    let store_head = seed.store().head()?;
    let core_reads = seed.worker_get_count();
    let core_mutations = seed.post_count();
    let engine_calls = std::fs::read(seed.root().join("calls"))?;
    assert!(matches!(
        stops
            .legacy_worker_terminal_evidence(operation, command)
            .await,
        Err(JournalError::Interrupted)
    ));

    let confirmed = stops
        .append_cancel(CancelInputV1::new(operation, command)?)
        .await?;
    assert_eq!(
        confirmed.receipt().head().revision(),
        before.head().revision() + 1
    );
    let proof = stops
        .legacy_worker_terminal_evidence(operation, command)
        .await?;
    assert_eq!(proof.operation_id(), operation);
    assert_eq!(proof.head(), confirmed.receipt().head());
    assert_eq!(proof.cancel_receipt().command_id(), command);
    assert_eq!(
        proof.cancel_receipt().input_hash(),
        confirmed.receipt().input_hash()
    );
    assert_eq!(proof.cancel_receipt().head(), confirmed.receipt().head());
    assert!(canonical(proof.scope())? == canonical(seed.scope())?);
    assert_eq!(proof.worker().worker_id, seed.worker_id);
    let (arm, receipt) = proof.arm();
    assert!(canonical(arm)? == canonical(retained.arm())?);
    assert!(canonical(receipt)? == canonical(retained.receipt())?);
    let (claim, claim_receipt) = proof.claim();
    assert!(canonical(&claim.arm_receipt)? == canonical(receipt)?);
    assert!(claim_receipt.audit_seq > receipt.audit_seq);
    let (authority, owned) = proof.target();
    assert!(canonical(authority)? == canonical(retained.authority())?);
    assert!(canonical(owned)? == canonical(retained.owned())?);

    assert!(matches!(
        stops
            .legacy_worker_terminal_evidence(Uuid::new_v4(), command)
            .await,
        Err(JournalError::Interrupted)
    ));
    let after = stops
        .recovery_status(operation)
        .await?
        .context("Legacy job missing after the proof read")?;
    assert_eq!(after.head(), confirmed.receipt().head());
    assert_eq!(
        after.cancel_receipts().len(),
        before.cancel_receipts().len() + 1
    );
    assert_eq!(seed.store().head()?, store_head);
    assert_eq!(seed.worker_get_count(), core_reads);
    assert_eq!(seed.post_count(), core_mutations);
    assert!(std::fs::read(seed.root().join("calls"))? == engine_calls);
    Ok(())
}
