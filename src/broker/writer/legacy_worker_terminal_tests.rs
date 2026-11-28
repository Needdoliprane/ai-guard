use super::*;
use crate::{
    broker::journal::canonical,
    control::{
        ArmSubjectV1, MutationReceiptV1, MutationResultV1, QuotaPolicyV1, RuntimeStatusV1,
        WorkerMutationV1, WorkerStateV1, WorkerTransitionV1, content_hash,
    },
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

#[tokio::test]
async fn stops_writer_retains_the_first_terminal_observation_across_later_stops()
-> anyhow::Result<()> {
    use ReservedLegacyWorkerTerminalObservationV1::{Appended, Existing};

    let seed = running_seed_for_cold_terminal_test().await;
    let root = Directory::open_existing(&seed.journal_root)?;
    let operation = seed.operation_id;
    let open_fixture_journal = || {
        Journal::open(
            &root,
            seed.scope().clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(Vec::new())?,
        )
    };
    // Seed a normal retained outbox entry before the Stops actor takes the
    // lease. The entire tested reservation and replay use the restricted actor.
    let outbox_before = {
        let journal = open_fixture_journal()?;
        let job = journal.load(operation)?.context("fixture job missing")?;
        let request = job.intent().request_data();
        let mut event = Event::new("observer.coverage");
        event.session_id = request.session_id.clone();
        event.project_id = request.project_id.clone();
        event.sandbox_id = request.sandbox_id.clone();
        event.correlation_id = request.correlation_id.clone();
        event.trace_id = request.trace_id.clone();
        let mut events = job
            .broker_checkpoint_v4()
            .map_or_else(Vec::new, |value| value.events().to_vec());
        events.push(OutboxEventV1::new(operation, job.intent_hash(), event)?);
        let lifecycle = job.broker_checkpoint_v3().context("fixture STOP missing")?;
        let next = BrokerCheckpointV4::new(lifecycle.clone(), events)?;
        journal.advance_outbox(operation, job.head().clone(), &next)?;
        canonical(&next.events())?
    };
    let open_stops = || -> anyhow::Result<StopWriterClient> {
        let report = StopRecoveryReaderV1::open_existing(
            &root,
            seed.scope().clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(Vec::new())?,
        )?
        .context("retained journal lease missing")?
        .scan();
        let caps = ProfileWriterCapabilities::from_recovery_report(
            &report,
            true,
            4,
            Duration::from_secs(10),
        )?;
        assert!(caps.is_stop_only());
        Ok(caps.stops())
    };
    let stops = open_stops()?;
    let command = Uuid::new_v4();
    let confirmed = stops
        .append_cancel(CancelInputV1::new(operation, command)?)
        .await?;
    let stop_head = confirmed.receipt().head().clone();
    let stop_hash = confirmed.receipt().input_hash().to_owned();
    drop(confirmed);

    let proof = stops
        .legacy_worker_terminal_evidence(operation, command)
        .await?;
    let worker = proof.worker();
    let mut terminal = worker
        .last_observation
        .clone()
        .context("Running observation missing")?;
    let start = chrono::DateTime::parse_from_rfc3339(
        terminal
            .started_at
            .as_deref()
            .context("Running start missing")?,
    )?;
    terminal.observed_at = (start + chrono::Duration::seconds(4)).to_rfc3339();
    terminal.finished_at = Some((start + chrono::Duration::seconds(3)).to_rfc3339());
    terminal.status = RuntimeStatusV1::Exited;
    terminal.running = Some(false);
    terminal.exit_code = Some(17);
    let expected = WorkerMutationV1 {
        version: 1,
        operation_id: proof.terminal_observation_operation_id()?,
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition: WorkerTransitionV1::Observe {
            observation: terminal,
        },
    };
    let expected_bytes = canonical(&expected)?;
    let store_head = seed.store().head()?;
    let quota = canonical(&seed.store().worker_quota(&QuotaPolicyV1::default())?)?;
    let core_reads = seed.worker_get_count();
    let core_mutations = seed.post_count();
    let calls_path = seed.root().join("calls");
    let engine_calls = std::fs::read(&calls_path)?;

    let older = stops
        .legacy_worker_terminal_evidence(operation, seed.cancel_id)
        .await?;
    assert!(matches!(
        stops
            .reserve_legacy_worker_terminal_observation(older, expected.clone())
            .await,
        Err(LifecycleWriteFailure::BeforePublication(
            JournalError::Conflict
        ))
    ));
    assert!(
        stops
            .legacy_worker_terminal_observation(operation)
            .await?
            .is_none()
    );

    let same_head = stops
        .legacy_worker_terminal_evidence(operation, command)
        .await?;
    let (one, two) = tokio::join!(
        stops.reserve_legacy_worker_terminal_observation(proof, expected.clone()),
        stops.reserve_legacy_worker_terminal_observation(same_head, expected.clone()),
    );
    let (appended, replay) = match (one, two) {
        (Ok(Appended(a)), Ok(Existing(b))) | (Ok(Existing(b)), Ok(Appended(a))) => (a, b),
        _ => anyhow::bail!("exact concurrent reservations must publish only once"),
    };
    assert_eq!(appended.head().revision(), stop_head.revision() + 1);
    assert_eq!(replay.head(), appended.head());
    assert_eq!(canonical(appended.request())?, expected_bytes);
    assert_eq!(canonical(replay.request())?, expected_bytes);
    assert_eq!(appended.stop().command_id(), command);
    assert_eq!(appended.stop().head(), &stop_head);
    assert_eq!(appended.stop().input_hash(), stop_hash);

    let later = Uuid::new_v4();
    let later_stop = stops
        .append_cancel(CancelInputV1::new(operation, later)?)
        .await?;
    let later_head = later_stop.receipt().head().clone();
    drop(later_stop);
    let original_stop = stops
        .legacy_worker_terminal_evidence(operation, command)
        .await?;
    let Ok(Existing(replayed)) = stops
        .reserve_legacy_worker_terminal_observation(original_stop, expected.clone())
        .await
    else {
        anyhow::bail!("a later STOP must keep the original pending request");
    };
    assert_eq!(replayed.stop().command_id(), command);
    assert_eq!(replayed.head(), &later_head);
    assert_eq!(canonical(replayed.request())?, expected_bytes);
    let still_exact = stops
        .legacy_worker_terminal_evidence(operation, command)
        .await?;
    let mut divergent = expected.clone();
    let WorkerTransitionV1::Observe { observation } = &mut divergent.transition else {
        unreachable!("fixture creates only an Observe request");
    };
    observation.exit_code = Some(18);
    assert!(matches!(
        stops
            .reserve_legacy_worker_terminal_observation(still_exact, divergent)
            .await,
        Err(LifecycleWriteFailure::BeforePublication(
            JournalError::Conflict
        ))
    ));

    let previous_lease = stops
        .legacy_worker_terminal_evidence(operation, command)
        .await?;
    close_stops_actor(stops).await?;
    let reopened = open_stops()?;
    assert!(matches!(
        reopened
            .reserve_legacy_worker_terminal_observation(previous_lease, expected)
            .await,
        Err(LifecycleWriteFailure::BeforePublication(
            JournalError::Conflict
        ))
    ));
    let pending = reopened
        .legacy_worker_terminal_observation(operation)
        .await?
        .context("terminal request was lost after reopening")?;
    assert_eq!(canonical(pending.request())?, expected_bytes);
    assert_eq!(pending.stop().command_id(), command);
    assert_eq!(pending.stop().head(), &stop_head);
    assert_eq!(pending.head(), &later_head);

    let parent = reopened
        .legacy_worker_terminal_evidence(operation, command)
        .await?;
    let worker = crate::audit::expected_worker_transition(parent.worker(), pending.request())?;
    let response = MutationResultV1 {
        receipt: MutationReceiptV1 {
            operation_id: pending.request().operation_id.clone(),
            worker_id: Some(worker.worker_id.clone()),
            revision: Some(worker.revision),
            input_hash: content_hash("ai-guard:worker-mutation:v1", pending.request())?,
            // Synthetic reply above the fixture's entire audit prefix. This
            // oracle exercises the journal adapter, with no Core exchange.
            audit_seq: i64::MAX - 1,
            audit_hash: "a".repeat(64),
        },
        worker,
        disposition: None,
    };
    let intercalated = Uuid::new_v4();
    let intercalated_stop = reopened
        .append_cancel(CancelInputV1::new(operation, intercalated)?)
        .await?;
    let intercalated_head = intercalated_stop.receipt().head().clone();
    let intercalated_hash = intercalated_stop.receipt().input_hash().to_owned();
    drop(intercalated_stop);
    assert!(matches!(
        reopened
            .confirm_legacy_worker_terminal_observation(&appended, response.clone())
            .await,
        Err(LifecycleWriteFailure::BeforePublication(
            JournalError::Conflict
        ))
    ));

    let mut divergent = response.clone();
    divergent.receipt.input_hash = "b".repeat(64);
    assert!(matches!(
        reopened
            .confirm_legacy_worker_terminal_observation(&pending, divergent)
            .await,
        Err(LifecycleWriteFailure::BeforePublication(
            JournalError::Conflict
        ))
    ));
    let acknowledged = reopened
        .confirm_legacy_worker_terminal_observation(&pending, response.clone())
        .await
        .map_err(LifecycleWriteFailure::journal_error)?;
    assert_eq!(
        acknowledged.head().revision(),
        intercalated_head.revision() + 1
    );
    assert_eq!(canonical(acknowledged.response())?, canonical(&response)?);
    let replayed = reopened
        .confirm_legacy_worker_terminal_observation(&pending, response.clone())
        .await
        .map_err(LifecycleWriteFailure::journal_error)?;
    assert_eq!(acknowledged.head(), replayed.head());
    assert_eq!(canonical(replayed.response())?, canonical(&response)?);
    assert!(matches!(
        reopened.legacy_worker_terminal_observation(operation).await,
        Err(JournalError::Conflict)
    ));
    // Both audit hashes are well-formed. A plausible but different reply
    // cannot replace the exact receipt already durable at this index.
    let mut divergent = response.clone();
    divergent.receipt.audit_hash = "c".repeat(64);
    pending.validate_response(&divergent)?;
    assert!(matches!(
        reopened
            .confirm_legacy_worker_terminal_observation(&pending, divergent)
            .await,
        Err(LifecycleWriteFailure::BeforePublication(
            JournalError::Conflict
        ))
    ));
    let observed = reopened
        .recovery_status(operation)
        .await?
        .context("acknowledged job missing")?;
    assert_eq!(observed.head(), acknowledged.head());
    let stop = observed
        .cancel_receipts()
        .iter()
        .find(|receipt| receipt.command_id() == intercalated)
        .context("intercalated STOP lost")?;
    assert_eq!(stop.head(), &intercalated_head);
    assert_eq!(stop.input_hash(), intercalated_hash);
    drop(observed);

    assert_eq!(seed.store().head()?, store_head);
    assert_eq!(
        canonical(&seed.store().worker_quota(&QuotaPolicyV1::default())?)?,
        quota
    );
    assert!(seed.store().worker_usage()?.is_empty());
    assert_eq!(seed.worker_get_count(), core_reads);
    assert_eq!(seed.post_count(), core_mutations);
    assert_eq!(std::fs::read(calls_path)?, engine_calls);
    close_stops_actor(reopened).await?;
    let journal = open_fixture_journal()?;
    let job = journal.load(operation)?.context("fixture job missing")?;
    let after = job.broker_checkpoint_v4().context("outbox was lost")?;
    assert_eq!(canonical(&after.events())?, outbox_before);
    let broker = after.lifecycle().broker();
    let worker = broker
        .runner()
        .broker_context()
        .worker
        .context("local Worker missing")?;
    assert_eq!(worker.state, WorkerStateV1::Terminal);
    assert!(!worker.quota_held);
    assert_eq!(canonical(worker)?, canonical(&response.worker)?);
    let mut replies = broker
        .supervision()
        .operations()
        .iter()
        .filter_map(|operation| match operation {
            crate::broker::checkpoint::SupervisionOperationV1::WorkerMutation {
                request,
                response: Some(receipt),
            } if request.operation_id == response.receipt.operation_id => Some((request, receipt)),
            _ => None,
        });
    let (stored_request, stored_response) = replies.next().context("durable ACK missing")?;
    assert!(replies.next().is_none());
    assert_eq!(canonical(stored_request)?, expected_bytes);
    assert_eq!(canonical(stored_response)?, canonical(&response)?);
    Ok(())
}

async fn close_stops_actor(stops: StopWriterClient) -> anyhow::Result<()> {
    let join = stops.0.retirements.take_writer_join_for_test()?;
    drop(stops);
    tokio::task::spawn_blocking(move || join.join().map_err(|_| JournalError::Io)).await??;
    Ok(())
}
