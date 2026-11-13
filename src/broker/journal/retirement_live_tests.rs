//! Composed Journal/Writer/SQLite/private-Unix regressions. SyntheticReady is
//! the explicit test runtime seam; no engine, provider or business CLI runs.
use super::*;
use crate::broker::{
    enforcer::{
        EnforcedSlot, EnforcerFullClientV3, retirement::test_support::OriginalUnusedFixture,
    },
    writer::{
        BeginUnusedRetirement, CancelInputV1, PendingUnusedRetirement, ProfileWriterCapabilities,
        WriterClient,
    },
};
use std::{io::Write, time::Duration};

fn sibling(intent: &ExecutionIntentV1) -> ExecutionIntentV1 {
    let mut value = serde_json::to_value(intent).unwrap();
    value["operation_id"] = serde_json::json!(Uuid::new_v4());
    ExecutionIntentV1::from_bytes(&canonical(&value).unwrap()).unwrap()
}

async fn begin_a(
    journal: &Journal,
    store: &crate::audit::Store,
    intent: &ExecutionIntentV1,
    head: &RevisionRefV1,
    writer: &WriterClient,
    client: &EnforcerFullClientV3,
    slot: &EnforcedSlot,
) -> anyhow::Result<PendingUnusedRetirement> {
    writer
        .bind_full3_registration(
            intent.operation_id(),
            head.clone(),
            slot.full3_registration_binding()?,
        )
        .await?;
    let command = Uuid::new_v4();
    let arm = &slot.full3_registration_binding()?.arm_id().to_string();
    let mut signal = retirement::event(
        intent,
        command,
        "broker.abandon-before-claim.requested",
        serde_json::json!({
        "version":1,"operation_id":intent.operation_id(),"arm_id":arm,"expected_epoch":client.epoch(),"generation":0,"command_id":command}),
    );
    signal.source = "ai-guard-broker".into();
    signal.coverage = "native".into();
    writer
        .enqueue_event(intent.operation_id(), signal.clone())
        .await?;
    let signal_ack = store.append(signal)?;
    writer
        .acknowledge_event(intent.operation_id(), command, signal_ack.seq)
        .await?;
    let unused = slot.revoke_unused(command).await?;
    let current = writer
        .load(intent.operation_id())
        .await?
        .ok_or(JournalError::Corrupt)?;
    let pending = writer
        .begin_unused_retirement(BeginUnusedRetirement::new(
            intent.operation_id(),
            current.head().clone(),
            unused,
        )?)
        .await?;
    let ack = store.append(pending.intention_event().clone())?;
    writer
        .acknowledge_event(intent.operation_id(), pending.intention_event_id(), ack.seq)
        .await?;
    let confirmed = journal
        .load(intent.operation_id())?
        .ok_or(JournalError::Corrupt)?;
    anyhow::ensure!(
        confirmed.broker_checkpoint_v5().is_some(),
        "Pending was not confirmed"
    );
    Ok(pending)
}

#[tokio::test]
async fn promised_pending_survives_other_job_reserve_accept_and_reject_pressure() {
    let live = super::retirement_live_fixture::live_fixture();
    let actor_registration = live.registration.clone();
    let (fixture, client, slot) =
        OriginalUnusedFixture::register(live.journal.scope().installation_id(), actor_registration)
            .await
            .unwrap();
    let observed = async {
        let writer = WriterClient::start(live.journal.clone(), 32, Duration::from_secs(30))?;
        let reject_job = sibling(&live.intent);
        let accept_job = sibling(&live.intent);
        let ReserveResult::New(reject_ticket) = writer
            .reserve(
                reject_job.operation_id(),
                b"reserved rejection target".to_vec(),
            )
            .await?
        else {
            anyhow::bail!("fresh rejection target");
        };
        let ReserveResult::New(accept_ticket) = writer
            .reserve(
                accept_job.operation_id(),
                b"reserved acceptance target".to_vec(),
            )
            .await?
        else {
            anyhow::bail!("fresh acceptance target");
        };
        let pending = begin_a(
            &live.journal,
            &live.store,
            &live.intent,
            &live.head,
            &writer,
            &client,
            &slot,
        )
        .await?;
        let worker_id = pending.identity().arm().worker_id.clone();
        let before = canonical(
            &live
                .store
                .worker(&worker_id)?
                .ok_or(JournalError::Corrupt)?,
        )?;
        let mut tickets = Vec::new();
        let mut refused = None;
        for _ in 0..128 {
            let operation = Uuid::new_v4();
            match writer
                .reserve(operation, b"legitimate new submission".to_vec())
                .await
            {
                Ok(ReserveResult::New(ticket)) => tickets.push(ticket),
                Err(JournalError::LimitExceeded) => {
                    refused = Some(operation);
                    break;
                }
                _ => anyhow::bail!("unexpected reservation result under bounded pressure"),
            }
        }
        let refused = refused
            .ok_or_else(|| anyhow::anyhow!("scope pressure did not reach promised reservation"))?;
        anyhow::ensure!(
            !live
                .journal
                .0
                .jobs
                .path()
                .join(refused.to_string())
                .exists(),
            "refused reserve wrote a job"
        );
        // Reservations add two entries; use actual rejected submissions to
        // consume any remaining single-entry capacity, rather than padding FS.
        while let Some(ticket) = tickets.pop() {
            match writer
                .reject(
                    ticket,
                    RejectionPhaseV1::SubmissionPreparation,
                    RejectionCodeV1::PreparationFailed,
                )
                .await
            {
                Ok(_) => {}
                Err(error) if error.error() == JournalError::LimitExceeded => break,
                Err(error) => return Err(error.into()),
            }
        }
        let rejected = writer
            .reject(
                reject_ticket,
                RejectionPhaseV1::SubmissionPreparation,
                RejectionCodeV1::PreparationFailed,
            )
            .await;
        anyhow::ensure!(
            rejected
                .as_ref()
                .err()
                .is_some_and(|error| error.error() == JournalError::LimitExceeded),
            "reject consumed another job's promise"
        );
        anyhow::ensure!(
            !live
                .journal
                .0
                .jobs
                .path()
                .join(reject_job.operation_id().to_string())
                .join("rejected.json")
                .exists(),
            "refused reject wrote its record"
        );
        let accepted = live.journal.accept(
            accept_ticket,
            &accept_job,
            BindingInputs::None,
            &CheckpointPayloadV1::initial(&accept_job)?,
        );
        anyhow::ensure!(
            matches!(accepted, Err(JournalError::LimitExceeded)),
            "accept consumed another job's promise"
        );
        anyhow::ensure!(
            !live
                .journal
                .0
                .jobs
                .path()
                .join(accept_job.operation_id().to_string())
                .join("intent.json")
                .exists(),
            "refused accept wrote intent"
        );
        let authorized = writer.authorize_unused_retirement(pending).await?;
        let verified = client.retire_unused(authorized).await?;
        let receipt_hash = verified.receipt().digest()?;
        writer.stops().finalize_retirement(verified).await?;
        let loaded = writer
            .load(live.intent.operation_id())
            .await?
            .ok_or(JournalError::Corrupt)?;
        let entry = &loaded
            .broker_checkpoint_v5()
            .ok_or(JournalError::Corrupt)?
            .retirements()[0];
        anyhow::ensure!(
            entry.finalized().is_some_and(
                |finalized| finalized.receipt().digest().ok().as_ref() == Some(&receipt_hash)
            ),
            "promised Final did not commit"
        );
        anyhow::ensure!(
            before
                == canonical(
                    &live
                        .store
                        .worker(&worker_id)?
                        .ok_or(JournalError::Corrupt)?
                )?,
            "retirement changed central worker state"
        );
        anyhow::Ok(())
    }
    .await;
    fixture.finish().await;
    observed.unwrap();
}

#[tokio::test]
async fn q_finalizes_before_factory_with_same_owner_and_late_ack_survives_corrupt_b() {
    let super::retirement_live_fixture::LiveFixture {
        _root,
        _store_root,
        store,
        journal,
        intent,
        head,
        registration,
    } = super::retirement_live_fixture::live_fixture();
    let scope = journal.scope().clone();
    let (fixture, client, slot) =
        OriginalUnusedFixture::register(scope.installation_id(), registration)
            .await
            .unwrap();
    let observed = async {
        let writer = WriterClient::start(journal.clone(), 32, Duration::from_secs(30))?;
        let other = sibling(&intent);
        let ReserveResult::New(ticket) = journal.reserve(other.operation_id(), b"real sibling")?
        else {
            anyhow::bail!("fresh sibling");
        };
        journal.accept(
            ticket,
            &other,
            BindingInputs::None,
            &CheckpointPayloadV1::initial(&other)?,
        )?;
        let pending = begin_a(&journal, &store, &intent, &head, &writer, &client, &slot).await?;
        let identity = pending.identity().clone();
        let authorized = writer.authorize_unused_retirement(pending).await?;
        let verified = client.retire_unused(authorized).await?;
        let qhash = verified.receipt().digest()?;
        let cancel = Uuid::new_v4();
        let corrupted = journal
            .0
            .jobs
            .path()
            .join(other.operation_id().to_string())
            .join("current.json");
        std::fs::OpenOptions::new()
            .append(true)
            .open(corrupted)?
            .write_all(b"corrupt-B-current")?;
        writer
            .stops()
            .append_cancel(CancelInputV1::new(intent.operation_id(), cancel)?)
            .await?;
        // Old process owner is released; the startup report retains the next
        // exclusive lease continuously from its pre-factory F through handoff.
        drop(writer);
        let retained = Arc::clone(&journal.0);
        drop(journal);
        tokio::time::timeout(Duration::from_secs(5), async {
            while Arc::strong_count(&retained) > 1 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        drop(retained);
        let directory = Directory::open_existing(&_root.path().canonicalize()?)?;
        let mut report = recovery::StopRecoveryReaderV1::open_existing(
            &directory,
            scope.clone(),
            &FsExclusions::new(),
            JournalLimitsV1::new(256 * 1024 * 1024, 256, 256, 512 * 1024 * 1024, 256, 128)?,
            ControlSecrets::new(Vec::new())?,
        )?
        .ok_or(JournalError::Corrupt)?
        .scan();
        anyhow::ensure!(report.incomplete(), "corrupt B must remain incomplete");
        let initial = report
            .jobs()
            .iter()
            .find(|job| job.operation_id() == intent.operation_id())
            .ok_or(JournalError::Corrupt)?;
        anyhow::ensure!(
            initial.retirements().len() == 1 && initial.retirements()[0].finalized().is_none(),
            "startup omitted Pending when dropping bulky metadata"
        );
        let core_before = report.test_retained_core()?;
        report.finalize_retirement(verified)?;
        let finalized = report
            .jobs()
            .iter()
            .find(|job| job.operation_id() == intent.operation_id())
            .ok_or(JournalError::Corrupt)?;
        anyhow::ensure!(
            finalized.stops().is_empty() && finalized.retirements()[0].finalized().is_some(),
            "Final was not reloaded before Restore/factory"
        );
        anyhow::ensure!(
            finalized.retirements()[0]
                .finalized()
                .ok_or(JournalError::Corrupt)?
                .receipt()
                .digest()?
                == qhash,
            "Q receipt changed"
        );
        let capabilities = ProfileWriterCapabilities::from_recovery_report(
            &report,
            true,
            16,
            Duration::from_secs(30),
        )?;
        anyhow::ensure!(
            capabilities.is_stop_only(),
            "recovery unexpectedly gained admission"
        );
        anyhow::ensure!(
            Arc::ptr_eq(&core_before, &report.test_retained_core()?),
            "factory replaced retained core"
        );
        let replayed = client
            .query_retirement(&identity)
            .await?
            .ok_or(JournalError::Corrupt)?;
        anyhow::ensure!(
            matches!(
                report.finalize_retirement(replayed),
                Err(JournalError::Busy)
            ),
            "report Final port remained live after factory"
        );
        let stops = capabilities.stops();
        let events = stops
            .pending_retirement_events(intent.operation_id())
            .await?;
        anyhow::ensure!(
            events.len() == 1,
            "one exact final event must remain pending"
        );
        let mut acknowledgements = Vec::new();
        for event in events {
            let ack = store.append(event.event().clone())?;
            acknowledgements.push((event.event_id(), event.input_hash().to_owned(), ack.seq));
        }
        stops
            .acknowledge_retirement_events(intent.operation_id(), acknowledgements)
            .await?;
        anyhow::ensure!(
            stops
                .pending_retirement_events(intent.operation_id())
                .await?
                .is_empty(),
            "late final ACK did not persist under Stops"
        );
        let snapshot = stops
            .recovery_status(intent.operation_id())
            .await?
            .ok_or(JournalError::Corrupt)?;
        anyhow::ensure!(snapshot.incomplete(), "B unknown was presented as complete");
        anyhow::ensure!(
            snapshot
                .cancel_receipts()
                .iter()
                .any(|receipt| receipt.command_id() == cancel),
            "Final lost concurrent Cancel"
        );
        anyhow::Ok(())
    }
    .await;
    fixture.finish().await;
    observed.unwrap();
}
