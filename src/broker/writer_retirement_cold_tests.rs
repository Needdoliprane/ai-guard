//! Real original native retirement plus metadata-only Cold composition.
//! The peer uses the fixture's actual SQLite Store. It proves receipt integrity,
//! not production HTTP identity, a runtime terminal observation or an engine.
use super::*;
use crate::{
    broker::{
        enforcer::{
            EnforcedSlot, EnforcerFullClientV3, retirement::test_support::OriginalUnusedFixture,
        },
        journal::{
            canonical,
            recovery::cold_audit,
            retirement_live_fixture::{LiveFixture, live_fixture_with_gateway},
        },
    },
    model::{Event, StoredEvent},
    runner::{AuditClient, ColdAuditBudget, ExactStoredEventReceipt},
};
use anyhow::{Context, Result, anyhow, ensure};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use tokio::{net::TcpListener, task::JoinHandle};

struct Peer {
    live: Arc<LiveFixture>,
    task: JoinHandle<()>,
}
impl Peer {
    async fn start() -> Result<Self> {
        Self::start_with_sibling_capacity(false).await
    }
    async fn start_with_sibling_capacity(sibling: bool) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let live = live_fixture_with_gateway(&origin, false);
        let live = Arc::new(if sibling {
            // The shared fixture has 256 scope entries for one job. This
            // scenario later keeps two jobs, each retaining all of its unused
            // Cancel capacity: 63*2 + (64+1)*2 = 256 future entries alone.
            // Bound this two-job scope by the existing per-job cap, plus the
            // three scope anchors (.writer.lock, scope.json and jobs). Keep
            // every per-job byte/entry/revision limit and the byte budget.
            let LiveFixture {
                _root,
                _store_root,
                store,
                journal,
                intent,
                head,
                registration,
            } = live;
            let scope = journal.scope().clone();
            drop(journal);
            // Setup only: no writer/native owner exists yet. Reopen the same
            // fixture namespace with its unchanged records and scope identity.
            let directory =
                crate::control_fs::Directory::open_existing(&_root.path().canonicalize()?)?;
            let journal = Journal::open(
                &directory,
                scope,
                &crate::broker::journal::FsExclusions::new(),
                crate::broker::journal::JournalLimitsV1::new(
                    256 * 1024 * 1024,
                    256,
                    256,
                    512 * 1024 * 1024,
                    3 + 2 * 256,
                    128,
                )?,
                crate::broker::journal::ControlSecrets::new(Vec::new())?,
            )?;
            LiveFixture {
                _root,
                _store_root,
                store,
                journal,
                intent,
                head,
                registration,
            }
        } else {
            live
        });
        let app = Router::new()
            .route("/api/v1/events", post(receive_event))
            .route("/api/events", post(receive_event))
            .with_state(Arc::clone(&live));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("fixture HTTP peer");
        });
        Ok(Self { live, task })
    }
    async fn receipts(&self, events: &[Event]) -> Result<Vec<ExactStoredEventReceipt>> {
        let client = AuditClient::new(
            self.live.journal.scope().gateway_origin(),
            "fixture-retirement-cold-token",
        )?;
        let budget = ColdAuditBudget::start();
        let mut receipts = Vec::new();
        for event in events {
            receipts.push(client.post_cold_audit_event(event, &budget).await?);
        }
        Ok(receipts)
    }
    async fn signal(
        &self,
        writer: &WriterClient,
        retirement: Uuid,
        arm: Uuid,
        epoch: Uuid,
    ) -> Result<OutboxReceiptV1> {
        let event = crate::broker::journal::retirement_live_fixture::abandonment_event_data(
            &self.live.intent,
            retirement,
            arm,
            epoch,
            0,
        );
        writer
            .enqueue_event(self.live.intent.operation_id(), event.clone())
            .await?;
        let client = AuditClient::new(
            self.live.journal.scope().gateway_origin(),
            "fixture-retirement-cold-token",
        )?;
        let sequence = client.post_event(serde_json::to_value(&event)?).await?;
        writer
            .acknowledge_event(self.live.intent.operation_id(), retirement, sequence)
            .await
            .map_err(Into::into)
    }
    async fn finish(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}
async fn receive_event(State(live): State<Arc<LiveFixture>>, Json(event): Json<Event>) -> Response {
    match live.store.append(event) {
        Ok(stored) => (StatusCode::OK, Json(stored)).into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

async fn seed_pair(
    writer: &WriterClient,
    intent: &super::super::intent::ExecutionIntentV1,
) -> Result<Vec<Event>> {
    let events = ["observer.coverage", "broker.batch.result"].map(|kind| {
        crate::broker::journal::retirement::event(
            intent,
            Uuid::new_v4(),
            kind,
            serde_json::json!({"fixture":"metadata composition only"}),
        )
    });
    for event in &events {
        writer
            .enqueue_event(intent.operation_id(), event.clone())
            .await?;
    }
    Ok(events.to_vec())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_ack_moves_head_but_same_retirement_inputs_resume_once_and_final_stays_compatible() {
    let peer = Peer::start_with_sibling_capacity(true).await.unwrap();
    let live = Arc::clone(&peer.live);
    let (native, full, slot) = OriginalUnusedFixture::register(
        live.journal.scope().installation_id(),
        live.registration.clone(),
    )
    .await
    .unwrap();
    let outcome = run_cold(&peer, &full, &slot).await;
    native.finish().await;
    peer.finish().await;
    outcome.unwrap();
}

async fn run_cold(peer: &Peer, full: &EnforcerFullClientV3, slot: &EnforcedSlot) -> Result<()> {
    let live = &peer.live;
    let operation = live.intent.operation_id();
    let writer = WriterClient::start(live.journal.clone(), 2, Duration::from_secs(10))?;
    let binding = slot.full3_registration_binding()?;
    let arm = binding.arm_id();
    writer
        .bind_full3_registration(operation, live.head.clone(), binding.clone())
        .await?;
    let pair = seed_pair(&writer, &live.intent).await?;
    let retirement = Uuid::new_v4();
    peer.signal(&writer, retirement, arm, full.epoch()).await?;
    let expected = writer
        .load(operation)
        .await?
        .context("real accepted job")?
        .head()
        .clone();
    let stops = StopJournal::from_full(&live.journal);
    let prepared = cold_audit::prepare(&stops, operation).context("cold.first_pair.prepare")?;
    cold_audit::v5_test_support::assert_real_v5_bound(&stops, operation, true)?;
    let begin = BeginUnusedRetirement::new(
        operation,
        expected.clone(),
        slot.revoke_unused(retirement).await?,
    )?;
    let intention_id = begin.intention_event_id();
    let final_id = begin.final_event_id();
    let continuation = match writer.begin_unused_retirement(begin).await {
        Err(RetirementFailure::Refused {
            error: JournalError::Busy,
            continuation,
        }) => continuation,
        other => {
            return Err(anyhow!(
                "expected explicit pre-mutation Cold refusal, got {other:?}"
            ));
        }
    };
    drop(continuation);
    ensure!(
        writer.load(operation).await?.unwrap().head() == &expected,
        "Cold refusal appended a Pending"
    );
    ensure!(
        matches!(
            writer
                .bind_full3_registration(operation, expected.clone(), binding)
                .await,
            Err(JournalError::Busy)
        ),
        "direct binding escaped Cold guard"
    );
    let first_receipt = peer.receipts(&pair[..1]).await?;
    let partial = cold_audit::completion::complete(&stops, &prepared, &first_receipt)?;
    ensure!(
        partial.head().revision() == expected.revision() + 1,
        "real Cold ACK did not advance the head"
    );
    ensure!(
        cold_audit::completion::confirm(&stops, &prepared, &first_receipt)?.head()
            == partial.head(),
        "confirmation changed head"
    );
    let after_ack = partial.head().clone();
    drop(prepared);
    let continuation = match writer.recover_begin_unused_retirement(operation, arm, retirement) {
        Err(RetirementFailure::Refused { continuation, .. }) => continuation,
        _ => {
            return Err(anyhow!(
                "abandoning the refusal ticket lost the original input"
            ));
        }
    };
    let pending = writer.retry_begin_unused_retirement(continuation).await?;
    ensure!(
        pending.intention_event_id() == intention_id
            && pending.final_event_id() == final_id
            && pending.head().revision() == after_ack.revision() + 1,
        "retry changed original identities or appended twice"
    );
    let pending_ack: StoredEvent = live.store.append(pending.intention_event().clone())?;
    writer
        .acknowledge_event(operation, intention_id, pending_ack.seq)
        .await?;
    let authorized = writer.authorize_unused_retirement(pending).await?;
    let verified = full.retire_unused(authorized).await?;
    let identity = verified.identity().clone();
    let q_hash = verified.receipt().digest()?;
    let prepared = cold_audit::prepare(&stops, operation).context("cold.pending_final.prepare")?;
    cold_audit::v5_test_support::assert_real_v5_bound(&stops, operation, true)?;
    ensure!(
        matches!(
            stops.finalize_retirement(&verified),
            Err(crate::broker::journal::retirement::RetirementMutationError::ColdReserved)
        ),
        "direct Final escaped Cold guard"
    );
    let continuation = match writer.stops().finalize_retirement(verified).await {
        Err(RetirementFailure::Refused {
            error: JournalError::Busy,
            continuation,
        }) => continuation,
        other => {
            return Err(anyhow!(
                "Final must retain its verified input under Cold, got {other:?}"
            ));
        }
    };
    drop(continuation);
    let cancel_id = Uuid::new_v4();
    let cancel = stops.append_cancel(operation, cancel_id)?;
    ensure!(
        cancel.receipt().command_id() == cancel_id,
        "real concurrent metadata Cancel is missing"
    );
    let second_receipt = peer.receipts(&pair[1..]).await?;
    let acknowledged = cold_audit::completion::complete(&stops, &prepared, &second_receipt)?;
    ensure!(
        cold_audit::completion::confirm(&stops, &prepared, &second_receipt)?.head()
            == acknowledged.head(),
        "second confirmation republishes"
    );
    let after_ack = acknowledged.head().clone();
    drop(prepared);
    let continuation = match writer
        .stops()
        .recover_final_retirement(operation, arm, retirement)
    {
        Err(RetirementFailure::Refused { continuation, .. }) => continuation,
        _ => return Err(anyhow!("Final continuation was lost with its ticket")),
    };
    let committed = writer.stops().retry_final_retirement(continuation).await?;
    ensure!(
        committed.head().revision() == after_ack.revision() + 1,
        "Final did not use the current compatible head exactly once"
    );
    let replay = full
        .query_retirement(&identity)
        .await?
        .context("real Q replay")?;
    ensure!(replay.receipt().digest()? == q_hash, "Q changed");
    full.retire_local_slot(&replay)?;
    ensure!(
        writer.stops().finalize_retirement(replay).await?.head() == committed.head(),
        "replay wrote a second Final"
    );
    let final_events = writer.stops().pending_retirement_events(operation).await?;
    ensure!(
        final_events.len() == 1 && final_events[0].event_id() == final_id,
        "Final event identity changed"
    );
    let receipt = live.store.append(final_events[0].event().clone())?;
    writer
        .stops()
        .acknowledge_retirement_events(
            operation,
            vec![(
                final_id,
                final_events[0].input_hash().to_owned(),
                receipt.seq,
            )],
        )
        .await?;
    let original = writer.load(operation).await?.unwrap();
    let final_cp = original
        .broker_checkpoint_v5()
        .context("durable V5 Final")?;
    ensure!(
        final_cp.retirements().len() == 1 && final_cp.retirements()[0].finalized().is_some(),
        "Final projection missing"
    );
    ensure!(
        final_cp
            .outbox()
            .lifecycle()
            .commands()
            .iter()
            .any(|command| command.command_id() == cancel_id),
        "Cold ACK or Final discarded the concurrent metadata Cancel"
    );
    let sections = (
        canonical(&final_cp.bindings())?,
        canonical(&final_cp.retirements())?,
    );
    cold_audit::v5_test_support::assert_real_v5_bound(&stops, operation, false)?;
    // Historical Final must not freeze the profile: create another genuine
    // accepted metadata job and prepare its pair in this same journal scope.
    let mut wire: serde_json::Value = serde_json::from_slice(&live.intent.canonical_bytes()?)?;
    let sibling = Uuid::new_v4();
    wire["operation_id"] = serde_json::json!(sibling);
    let intent = super::super::intent::ExecutionIntentV1::from_bytes(&serde_json::to_vec(&wire)?)?;
    let ReserveResult::New(ticket) = live.journal.reserve(sibling, b"cold sibling metadata")?
    else {
        return Err(anyhow!("fresh sibling required"));
    };
    live.journal.accept(
        ticket,
        &intent,
        super::super::journal::BindingInputs::None,
        &crate::runner::CheckpointPayloadV1::initial(&intent)?,
    )?;
    let sibling_pair = seed_pair(&writer, &intent).await?;
    let prepared = cold_audit::prepare(&stops, sibling).context("cold.two_job_scope.prepare")?;
    let receipts = peer.receipts(&sibling_pair).await?;
    cold_audit::completion::complete(&stops, &prepared, &receipts)?;
    drop(prepared);
    let after = writer.load(operation).await?.unwrap();
    let after_cp = after.broker_checkpoint_v5().context("Final remains V5")?;
    ensure!(
        after.head() == original.head()
            && sections
                == (
                    canonical(&after_cp.bindings())?,
                    canonical(&after_cp.retirements())?
                ),
        "Cold sibling changed stable Final or registrations"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_begin_and_final_survive_lost_receivers_before_and_after_successful_send() {
    for after_send in [false, true] {
        let peer = Peer::start().await.unwrap();
        let live = Arc::clone(&peer.live);
        let (native, full, slot) = OriginalUnusedFixture::register(
            live.journal.scope().installation_id(),
            live.registration.clone(),
        )
        .await
        .unwrap();
        let outcome = run_lost_receivers(&peer, &full, &slot, after_send).await;
        native.finish().await;
        peer.finish().await;
        outcome.unwrap();
    }
}

async fn run_lost_receivers(
    peer: &Peer,
    full: &EnforcerFullClientV3,
    slot: &EnforcedSlot,
    after_send: bool,
) -> Result<()> {
    use super::retirement_owners::{OwnerKey, OwnerPhase};
    let live = &peer.live;
    let operation = live.intent.operation_id();
    let writer = WriterClient::start(live.journal.clone(), 1, Duration::from_secs(10))?;
    let binding = slot.full3_registration_binding()?;
    let arm = binding.arm_id();
    writer
        .bind_full3_registration(operation, live.head.clone(), binding)
        .await?;
    let retirement = Uuid::new_v4();
    let bound = peer.signal(&writer, retirement, arm, full.epoch()).await?;
    let input = BeginUnusedRetirement::new(
        operation,
        bound.head().clone(),
        slot.revoke_unused(retirement).await?,
    )?;
    let intention = input.intention_event_id();
    let final_event = input.final_event_id();
    let (entered, resume) = writer
        .0
        .retirements
        .pause_reply_for_test(OwnerPhase::Begin, after_send);
    // This is the same synchronous admission used by WriterClient, with the
    // actual input installed before enqueue. Only the reply waiter is manual.
    let (_, receive) =
        writer
            .0
            .enqueue_retirement(input, |owner, reply| Command::BeginUnusedRetirement {
                owner,
                reply,
            })?;
    let sent = tokio::time::timeout(Duration::from_secs(30), entered)
        .await
        .context("Begin reply barrier watchdog")??;
    ensure!(
        sent == if after_send { Some(true) } else { None },
        "barrier did not follow the actual send boundary"
    );
    drop(receive);
    let _ = resume.send(());
    // An ordinary FIFO command succeeds while the independent retention slot
    // is still occupied by the genuine Pending. Queue release is not reuse.
    ensure!(
        writer.status(operation).await?.is_some(),
        "ordinary queue did not progress"
    );
    let capacity = writer.0.retirements.admit::<(), ()>(
        OwnerKey {
            operation: Uuid::new_v4(),
            arm: Uuid::new_v4(),
            retirement: Uuid::new_v4(),
            phase: OwnerPhase::Begin,
        },
        "data-only-capacity-marker".into(),
        (),
    );
    ensure!(
        matches!(
            capacity,
            Err(RetirementFailure::NotQueued {
                error: JournalError::Busy,
                ..
            })
        ),
        "dequeue freed the distinct retention slot"
    );
    let pending = writer
        .recover_begin_unused_retirement(operation, arm, retirement)?
        .context("actual retained Pending")?;
    ensure!(
        pending.intention_event_id() == intention
            && pending.final_event_id() == final_event
            && pending.head().revision() == bound.head().revision() + 1,
        "Pending changed at a lost reply"
    );
    ensure!(
        writer
            .recover_begin_unused_retirement(operation, arm, retirement)?
            .is_none(),
        "Pending was consumed twice"
    );
    let stored = live.store.append(pending.intention_event().clone())?;
    writer
        .acknowledge_event(operation, intention, stored.seq)
        .await?;
    let verified = full
        .retire_unused(writer.authorize_unused_retirement(pending).await?)
        .await?;
    let identity = verified.identity().clone();
    let q_hash = verified.receipt().digest()?;
    let before = writer.load(operation).await?.unwrap().head().clone();
    let (entered, resume) = writer
        .0
        .retirements
        .pause_reply_for_test(OwnerPhase::Final, after_send);
    let (_, receive) =
        writer
            .0
            .enqueue_retirement(verified, |owner, reply| Command::FinalizeRetirement {
                owner,
                reply,
            })?;
    let sent = tokio::time::timeout(Duration::from_secs(30), entered)
        .await
        .context("Final reply barrier watchdog")??;
    ensure!(
        sent == if after_send { Some(true) } else { None },
        "Final actual send boundary not reached"
    );
    drop(receive);
    let _ = resume.send(());
    ensure!(
        writer.status(operation).await?.is_some(),
        "queue did not pass retained Final"
    );
    let committed = writer
        .stops()
        .recover_final_retirement(operation, arm, retirement)?
        .context("actual retained Final receipt")?;
    ensure!(
        committed.head().revision() == before.revision() + 1,
        "Final was not committed once"
    );
    ensure!(
        writer
            .stops()
            .recover_final_retirement(operation, arm, retirement)?
            .is_none(),
        "Final was consumed twice"
    );
    let replay = full
        .query_retirement(&identity)
        .await?
        .context("actual Q replay")?;
    ensure!(replay.receipt().digest()? == q_hash, "reply loss changed Q");
    full.retire_local_slot(&replay)?;
    ensure!(
        writer.stops().finalize_retirement(replay).await?.head() == committed.head(),
        "lost reply caused a second Final publication"
    );
    let events = writer.stops().pending_retirement_events(operation).await?;
    ensure!(
        events.len() == 1 && events[0].event_id() == final_event,
        "lost reply changed the final audit identity"
    );
    ensure!(
        !writer.stops().has_retained_retirement(),
        "successful handoff left an owner behind"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_after_cold_ack_is_not_mistaken_for_an_ack_only_begin_retry() {
    let peer = Peer::start().await.unwrap();
    let live = Arc::clone(&peer.live);
    let (native, full, slot) = OriginalUnusedFixture::register(
        live.journal.scope().installation_id(),
        live.registration.clone(),
    )
    .await
    .unwrap();
    let outcome: Result<()> = async {
        let operation = live.intent.operation_id();
        let writer = WriterClient::start(live.journal.clone(), 1, Duration::from_secs(10))?;
        let binding = slot.full3_registration_binding()?;
        let arm = binding.arm_id();
        writer
            .bind_full3_registration(operation, live.head.clone(), binding)
            .await?;
        let pair = seed_pair(&writer, &live.intent).await?;
        let retirement = Uuid::new_v4();
        peer.signal(&writer, retirement, arm, full.epoch()).await?;
        let expected = writer.load(operation).await?.unwrap().head().clone();
        let stops = StopJournal::from_full(&live.journal);
        let prepared = cold_audit::prepare(&stops, operation)?;
        let input = BeginUnusedRetirement::new(
            operation,
            expected.clone(),
            slot.revoke_unused(retirement).await?,
        )?;
        let intention = input.intention_event_id();
        let ticket = match writer.begin_unused_retirement(input).await {
            Err(RetirementFailure::Refused { continuation, .. }) => continuation,
            _ => return Err(anyhow!("real Cold refusal not reached")),
        };
        let receipts = peer.receipts(&pair[..1]).await?;
        cold_audit::completion::complete(&stops, &prepared, &receipts)?;
        drop(prepared);
        let cancel_id = Uuid::new_v4();
        stops.append_cancel(operation, cancel_id)?;
        let before = writer.load(operation).await?.unwrap().head().clone();
        let outcome = writer.retry_begin_unused_retirement(ticket).await;
        ensure!(
            matches!(
                outcome,
                Err(RetirementFailure::Unknown {
                    error: JournalError::Conflict
                })
            ),
            "non-ACK ancestry was accepted or its original input was returned as never queued"
        );
        let after = writer.load(operation).await?.unwrap();
        ensure!(
            after.head() == &before
                && after
                    .broker_checkpoint_v5()
                    .unwrap()
                    .retirements()
                    .is_empty()
                && after
                    .broker_checkpoint_v4()
                    .unwrap()
                    .events()
                    .iter()
                    .all(|event| event.event_id() != intention),
            "rejected ancestry appended a Pending or changed the current head"
        );
        ensure!(
            matches!(
                writer.recover_begin_unused_retirement(operation, arm, retirement),
                Err(RetirementFailure::Unknown {
                    error: JournalError::Conflict
                })
            ),
            "failure lost its retained original input"
        );
        ensure!(
            writer.stops().has_retained_retirement(),
            "unknown original continuation was abandoned"
        );
        writer.stops().close_retirement_owners()?;
        ensure!(
            writer.stops().drain_retirement_owners().await == Err(JournalError::Interrupted),
            "unconsumed original was reported drained"
        );
        Ok(())
    }
    .await;
    native.finish().await;
    peer.finish().await;
    outcome.unwrap();
}
