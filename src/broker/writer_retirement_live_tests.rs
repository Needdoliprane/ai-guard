//! Real sync_channel pressure with genuine live retirement owners. The shared
//! fixture uses SQLite/Journal and private native Unix IPC; SyntheticReady is
//! explicit and qualifies neither an engine nor the whole service/CLI route.
use super::*;
use crate::{
    audit::EventQuery,
    broker::{
        enforcer::{
            EnforcedSlot, EnforcerFullClientV3, retirement::test_support::OriginalUnusedFixture,
        },
        journal::{
            canonical,
            retirement_live_fixture::{LiveFixture, live_fixture},
        },
    },
    control::QuotaPolicyV1,
};
use anyhow::{Context, anyhow, ensure};

fn occupy_real_queue(
    writer: &WriterClient,
    operation_id: Uuid,
) -> anyhow::Result<oneshot::Receiver<JournalResult<Option<JournalStatusV1>>>> {
    let (reply, receive) = oneshot::channel();
    // This is an ordinary productive command, not a synthetic backend/result.
    writer
        .0
        .sender
        .try_send(Command::Status {
            operation_id,
            reply,
        })
        .map_err(|_| anyhow!("test could not fill the real capacity-one queue"))?;
    Ok(receive)
}

fn begin_ids(input: &BeginUnusedRetirement) -> (Uuid, Uuid, Uuid, Uuid) {
    (
        input.operation_id(),
        input.retirement_id(),
        input.intention_event_id(),
        input.final_event_id(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_queue_busy_returns_same_original_and_pending_then_authentic_q_f_commit_once() {
    let live = live_fixture();
    let (native, full, slot) = OriginalUnusedFixture::register(
        live.journal.scope().installation_id(),
        live.registration.clone(),
    )
    .await
    .unwrap();
    let result = run_pressure(&live, &full, &slot).await;
    native.finish().await;
    if result.is_err() {
        eprintln!(
            "private failed queue journal retained: {}",
            live._root.keep().display()
        );
        eprintln!(
            "private failed queue store retained: {}",
            live._store_root.keep().display()
        );
    }
    result.unwrap();
}

async fn run_pressure(
    live: &LiveFixture,
    full: &EnforcerFullClientV3,
    slot: &EnforcedSlot,
) -> anyhow::Result<()> {
    let writer = WriterClient::start(live.journal.clone(), 1, Duration::from_secs(10))
        .context("queue fixture writer.start")?;
    writer
        .bind_full3_registration(
            live.intent.operation_id(),
            live.head.clone(),
            slot.full3_registration_binding()?,
        )
        .await
        .context("queue fixture bind Full3")?;
    let retirement_id = Uuid::new_v4();
    let arm_id = slot.full3_registration_binding()?.arm_id();
    let mut signal = Event::new("broker.abandon-before-claim.requested");
    signal.id = retirement_id.to_string();
    signal.source = "ai-guard-broker".into();
    signal.coverage = "native".into();
    let request = live.intent.request_data();
    signal.session_id = request.session_id.clone();
    signal.project_id = request.project_id.clone();
    signal.sandbox_id = request.sandbox_id.clone();
    signal.correlation_id = request.correlation_id.clone();
    signal.trace_id = request.trace_id.clone();
    signal.data = serde_json::json!({"version":1,"operation_id":live.intent.operation_id(),"arm_id":arm_id,
            "expected_epoch":full.epoch(),"generation":0,"command_id":retirement_id});
    writer
        .enqueue_event(live.intent.operation_id(), signal.clone())
        .await
        .context("queue fixture signal enqueue")?;
    let signal_ack = live.store.append(signal)?;
    writer
        .acknowledge_event(live.intent.operation_id(), retirement_id, signal_ack.seq)
        .await
        .context("queue fixture signal ACK")?;
    let current = writer
        .load(live.intent.operation_id())
        .await?
        .ok_or_else(|| anyhow!("actual Arm job missing"))?;
    let expected = current.head().clone();
    let worker_id = live.registration.arm().worker_id.clone();
    let worker_before = canonical(
        &live
            .store
            .worker(&worker_id)?
            .ok_or_else(|| anyhow!("actual central worker missing"))?,
    )?;
    let quota_before = canonical(&live.store.worker_quota(&QuotaPolicyV1::default())?)?;
    let usage_before = canonical(&live.store.worker_usage()?)?;
    drop(current);
    // The sole revocation in this test. Neither Busy retry creates a new
    // witness, nonce, command or Begin identity.
    let unused = slot.revoke_unused(retirement_id).await?;
    let actor_nonce = unused.actor_nonce();
    let revocation_nonce = unused.revocation_nonce();
    let mut input =
        BeginUnusedRetirement::new(live.intent.operation_id(), expected.clone(), unused)?;
    let ids = begin_ids(&input);
    ensure!(
        ids.0 == live.intent.operation_id()
            && ids.1 == retirement_id
            && ids.2 != ids.3
            && ids.2 != retirement_id
            && ids.3 != retirement_id,
        "Begin did not allocate distinct fixed intention/final identities"
    );
    let pause = writer
        .pause_for_test()
        .await
        .context("queue fixture pause before Begin")?;
    ensure!(pause.is_paused(), "real writer did not acknowledge pause");
    let barrier = occupy_real_queue(&writer, live.intent.operation_id())?;
    for _ in 0..2 {
        input = match writer.begin_unused_retirement(input).await {
            Err(CompletionFailure::NotQueued {
                error: JournalError::Busy,
                input,
            }) => input,
            Err(failure) => {
                return Err(anyhow!(
                    "Begin pressure outcome differs: {:?}",
                    failure.error()
                ));
            }
            Ok(_) => {
                return Err(anyhow!(
                    "Begin entered a queue known full under its real actor pause"
                ));
            }
        };
        ensure!(
            pause.is_paused() && begin_ids(&input) == ids,
            "Busy changed original owner identities or test pause expired"
        );
    }
    let native_status = full.status().await?;
    ensure!(
        native_status.slot_count() == 1,
        "writer pressure must not release native retirement capacity"
    );
    let before_begin = live
        .journal
        .load(live.intent.operation_id())?
        .ok_or_else(|| anyhow!("actual job disappeared under queue pressure"))?;
    ensure!(
        before_begin.head() == &expected
            && before_begin
                .broker_checkpoint_v5()
                .is_some_and(|cp| cp.retirements().is_empty()),
        "NotQueued Begin changed durable state"
    );
    ensure!(pause.is_paused(), "pause elapsed before Drop release");
    drop(pause); // RAII releases the same actor without holding a journal lock.
    ensure!(
        writer.0.receive(barrier).await?.is_some(),
        "ordinary queued status did not resume"
    );
    let mut pending = writer
        .begin_unused_retirement(input)
        .await
        .context("queue fixture resubmit exact Begin")?;
    ensure!(
        pending.intention_event_id() == ids.2
            && pending.final_event_id() == ids.3
            && *pending.identity().retirement_id() == ids.1,
        "resubmitted Begin regenerated IDs"
    );
    ensure!(
        *pending.evidence().original_actor_nonce() == actor_nonce
            && *pending.evidence().prepare_revocation_nonce() == revocation_nonce,
        "resubmitted Begin changed the genuine original nonces"
    );
    let identity = pending.identity().clone();
    let evidence_hash = pending.evidence().digest()?;
    let intention_payload = canonical(pending.intention_event())?;
    let intention_input_hash = OutboxEventV1::new(
        live.intent.operation_id(),
        &live.intent.intent_hash()?,
        pending.intention_event().clone(),
    )?
    .input_hash()
    .to_owned();
    let pending_head = pending.head().clone();
    let intention_ack = live.store.append(pending.intention_event().clone())?;
    writer
        .acknowledge_event(
            live.intent.operation_id(),
            pending.intention_event_id(),
            intention_ack.seq,
        )
        .await
        .context("queue fixture intention ACK")?;
    let pause = writer
        .pause_for_test()
        .await
        .context("queue fixture pause before Authorize")?;
    let barrier = occupy_real_queue(&writer, live.intent.operation_id())?;
    for _ in 0..2 {
        pending = match writer.authorize_unused_retirement(pending).await {
            Err(CompletionFailure::NotQueued {
                error: JournalError::Busy,
                input,
            }) => input,
            Err(failure) => {
                return Err(anyhow!(
                    "Authorize pressure outcome differs: {:?}",
                    failure.error()
                ));
            }
            Ok(_) => {
                return Err(anyhow!(
                    "Authorize entered a queue known full under its real actor pause"
                ));
            }
        };
        ensure!(
            pause.is_paused()
                && pending.intention_event_id() == ids.2
                && pending.final_event_id() == ids.3
                && pending.head() == &pending_head
                && pending.evidence().digest()? == evidence_hash
                && canonical(pending.intention_event())? == intention_payload,
            "NotQueued Authorize changed its live Pending, nonce commitment or event"
        );
    }
    drop(pause);
    ensure!(
        writer.0.receive(barrier).await?.is_some(),
        "second ordinary queued status did not resume"
    );
    let authorized = writer
        .authorize_unused_retirement(pending)
        .await
        .context("queue fixture resubmit exact Pending")?;
    let verified = full
        .retire_unused(authorized)
        .await
        .context("queue fixture genuine Closing/T/join/Q")?;
    ensure!(
        *verified.receipt().retirement_id() == retirement_id
            && verified.accepted().evidence().digest()? == evidence_hash
            && *verified
                .receipt()
                .finalization()
                .joined()
                .original_actor_nonce()
                == actor_nonce
            && *verified.accepted().audit_ack().event_id() == ids.2
            && verified.accepted().audit_ack().input_hash().as_str() == intention_input_hash
            && *verified.accepted().audit_ack().audit_seq() == intention_ack.seq,
        "guardian accepted another identity/evidence/audit ACK"
    );
    let q_hash = verified.receipt().digest()?;
    let final_receipt = writer
        .stops()
        .finalize_retirement(verified)
        .await
        .context("queue fixture Final")?;
    let replay = full
        .query_retirement(&identity)
        .await?
        .ok_or_else(|| anyhow!("real Q replay missing"))?;
    ensure!(
        replay.receipt().digest()? == q_hash,
        "Q replay changed receipt"
    );
    full.retire_local_slot(&replay)?;
    let final_replay = writer.stops().finalize_retirement(replay).await?;
    ensure!(
        final_receipt.head() == final_replay.head(),
        "exact Q replay wrote a second Final"
    );
    let events = writer
        .stops()
        .pending_retirement_events(live.intent.operation_id())
        .await?;
    ensure!(
        events.len() == 1 && events[0].event_id() == ids.3,
        "queue retries created duplicate final events"
    );
    let final_input_hash = events[0].input_hash().to_owned();
    let final_payload = canonical(events[0].event())?;
    let actual_ack = live.store.append(events[0].event().clone())?;
    let final_acks = writer
        .stops()
        .acknowledge_retirement_events(
            live.intent.operation_id(),
            vec![(ids.3, final_input_hash.clone(), actual_ack.seq)],
        )
        .await?;
    ensure!(
        final_acks.len() == 1
            && final_acks[0].event_id() == ids.3
            && final_acks[0].input_hash() == final_input_hash
            && final_acks[0].audit_seq() == Some(actual_ack.seq),
        "Final ACK did not preserve its exact event/hash/SQLite sequence"
    );
    ensure!(
        writer
            .stops()
            .pending_retirement_events(live.intent.operation_id())
            .await?
            .is_empty(),
        "durable Final ACK was not retained"
    );
    let loaded = writer
        .load(live.intent.operation_id())
        .await?
        .ok_or_else(|| anyhow!("confirmed Final missing"))?;
    let cp = loaded
        .broker_checkpoint_v5()
        .ok_or_else(|| anyhow!("Final lost V5"))?;
    let final_entry = cp
        .retirements()
        .first()
        .and_then(|entry| entry.finalized())
        .ok_or_else(|| anyhow!("real Final missing"))?;
    ensure!(
        cp.retirements().len() == 1
            && final_entry.receipt().digest()? == q_hash
            && final_entry.final_input_hash().as_str() == final_input_hash,
        "Final changed or duplicated original receipt/hash"
    );
    for (kind, id, payload) in [
        ("broker.retirement.pending", ids.2, intention_payload),
        ("broker.retirement.final", ids.3, final_payload),
    ] {
        let records = live.store.events(&EventQuery {
            kind: Some(kind.into()),
            limit: Some(16),
            ..EventQuery::default()
        })?;
        ensure!(
            records.len() == 1
                && records[0].event.id == id.to_string()
                && canonical(&records[0].event)? == payload,
            "queue retry duplicated an audited retirement event"
        );
    }
    ensure!(
        worker_before
            == canonical(
                &live
                    .store
                    .worker(&worker_id)?
                    .ok_or_else(|| anyhow!("central worker disappeared"))?
            )?
            && quota_before == canonical(&live.store.worker_quota(&QuotaPolicyV1::default())?)?
            && usage_before == canonical(&live.store.worker_usage()?)?,
        "retirement queue pressure synthesized terminal/quota/usage changes"
    );
    Ok(())
}
