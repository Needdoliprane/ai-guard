//! Direct production retirement consumer, after the real Gateway authenticates
//! its context and Execution::open validates it. These tests do not call
//! arm_and_claim or exercise its Register race/branch selection. Native
//! SyntheticReady substitutes only runtime observation; real Unix registration,
//! original ownership, C/T/join/Q,
//! Journal/Writer and SQLite remain in use. No engine is invoked or qualified.
use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    broker::{
        clock::NativeHostClock,
        enforcer::{EnforcerFullClientV3, retirement::test_support::OriginalUnusedFixture},
        journal::{
            Journal, canonical,
            retirement_live_fixture::{
                INVOCATION_DATA_PROGRAM, LiveFixture, live_fixture_with_gateway,
            },
        },
        service::preclaim::{AbandonBeforeClaimV1, PreclaimStateV1},
    },
    control_fs::Directory,
    model::{Event, StoredEvent},
    redaction::Redactor,
    runtime::{Runtime, RuntimeKind},
};
use std::{
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize},
    },
};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

// Public synthetic credentials, never read from an environment or keychain.
const RUNNER: &str = "runner-direct-retirement-aaaaaaaaaaaaaaaaaaaaaaaa";
const VIEWER: &str = "viewer-direct-retirement-bbbbbbbbbbbbbbbbbbbbbbbb";
const SIGNAL: &str = "broker.abandon-before-claim.requested";
const INTENTION: &str = "broker.retirement.pending";
const FINAL: &str = "broker.retirement.final";

#[derive(Default)]
struct Delivery {
    fail_intention: AtomicBool,
    business_posts: AtomicUsize,
    event_posts: AtomicUsize,
    event_payloads: Mutex<Vec<Vec<u8>>>,
}
struct OwnedGateway {
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}
impl OwnedGateway {
    async fn start(
        listener: TcpListener,
        store: Arc<Store>,
        tenant: &str,
        delivery: Arc<Delivery>,
    ) -> Result<Self> {
        let state = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: tenant.into(),
                store,
                runner_token: RUNNER.into(),
                viewer_token: VIEWER.into(),
            }],
            crate::policy::Policy::default(),
            None,
            Redactor::new(Vec::new()),
        )?;
        let app = gateway_router(state).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let delivery = Arc::clone(&delivery);
                async move {
                    if request.method() != axum::http::Method::POST {
                        return next.run(request).await;
                    }
                    if request.uri().path() != "/api/events" {
                        delivery.business_posts.fetch_add(1, Ordering::SeqCst);
                        return next.run(request).await;
                    }
                    delivery.event_posts.fetch_add(1, Ordering::SeqCst);
                    let (parts, body) = request.into_parts();
                    let bytes = match axum::body::to_bytes(body, 128 * 1024).await {
                        Ok(bytes) => bytes,
                        Err(_) => {
                            return axum::response::IntoResponse::into_response(
                                axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                            );
                        }
                    };
                    let kind = serde_json::from_slice::<Event>(&bytes)
                        .ok()
                        .map(|event| event.kind);
                    if matches!(kind.as_deref(), Some(SIGNAL | INTENTION | FINAL))
                        && let Ok(mut payloads) = delivery.event_payloads.lock()
                        && payloads.len() < 3
                    {
                        payloads.push(bytes.to_vec());
                    }
                    if kind.as_deref() == Some(INTENTION)
                        && delivery.fail_intention.swap(false, Ordering::SeqCst)
                    {
                        // Exactly one owned HTTP response fails before Store commit.
                        // The real consumer has already produced durable P by then.
                        return axum::response::IntoResponse::into_response(
                            axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        );
                    }
                    next.run(axum::extract::Request::from_parts(
                        parts,
                        axum::body::Body::from(bytes),
                    ))
                    .await
                }
            },
        ));
        let (stop, receive) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = receive.await;
                })
                .await
        });
        Ok(Self {
            stop: Some(stop),
            task: Some(task),
        })
    }
    async fn finish(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(mut task) = self.task.take() {
            match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
                Ok(result) => result??,
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    return Err(anyhow!("owned consumer gateway did not drain"));
                }
            }
        }
        Ok(())
    }
}
impl Drop for OwnedGateway {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

// Root ownership stays outside the fallible consumer future. An unexpected
// failure/unknown outcome preserves both private namespaces rather than racing
// a queued writer with TempDir deletion. The expected blocked-P case preserves
// them as well; its real native fixture preserves its unhealthy role anchors.
struct Fixture {
    roots: Option<(tempfile::TempDir, tempfile::TempDir)>,
    cleanup: bool,
    store: Arc<Store>,
    journal: Journal,
    intent: crate::broker::intent::ExecutionIntentV1,
    head: crate::broker::journal::RevisionRefV1,
    registration: RegistrationV1,
}
impl From<LiveFixture> for Fixture {
    fn from(live: LiveFixture) -> Self {
        let LiveFixture {
            _root,
            _store_root,
            store,
            journal,
            intent,
            head,
            registration,
        } = live;
        Self {
            roots: Some((_root, _store_root)),
            cleanup: false,
            store: Arc::new(store),
            journal,
            intent,
            head,
            registration,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.cleanup
            && let Some((journal, store)) = self.roots.take()
        {
            let _journal_root = journal.keep();
            let _store_root = store.keep();
            eprintln!(
                "d5 direct consumer: private fixture roots retained for a blocked or unknown outcome"
            );
        }
    }
}
#[derive(Clone, Copy)]
enum Scenario {
    Final,
    IntentionUnavailable,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_consumer_retires_original_after_durable_signal_without_engine_command() {
    composed_consumer(Scenario::Final).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_consumer_retains_live_pending_when_intention_audit_returns_503() {
    composed_consumer(Scenario::IntentionUnavailable).await;
}
async fn composed_consumer(scenario: Scenario) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut fixture = Fixture::from(live_fixture_with_gateway(&origin, true));
    let delivery = Arc::new(Delivery::default());
    delivery.fail_intention.store(
        matches!(scenario, Scenario::IntentionUnavailable),
        Ordering::SeqCst,
    );
    let gateway = OwnedGateway::start(
        listener,
        Arc::clone(&fixture.store),
        fixture.journal.scope().tenant_id(),
        Arc::clone(&delivery),
    )
    .await
    .unwrap();
    // Approve/pin/hash/verify a real OS file as invocation DATA only. No
    // ManagedRuntime is fabricated and no command of this program is executed.
    let runtime =
        Runtime::with_program(RuntimeKind::Docker, Path::new(INVOCATION_DATA_PROGRAM)).unwrap();
    let config =
        Directory::open_existing(&fixture.intent.initial_options().run_options().config_dir)
            .unwrap();
    let context = AuthenticatedBatchContext::for_executor_test(
        AuditClient::new(&origin, RUNNER).unwrap(),
        fixture.journal.scope().clone(),
        config,
        runtime,
    )
    .await
    .unwrap();
    let (native, full, slot) = OriginalUnusedFixture::register_with_generation(
        fixture.journal.scope().installation_id(),
        fixture.registration.clone(),
        context.cancel_generation().clone(),
    )
    .await
    .unwrap();
    let observed = run_consumer(&fixture, &context, &full, slot, &delivery, scenario).await;
    // Preserve the context and any retained Pending through native cleanup.
    native.finish().await;
    let drained = gateway.finish().await;
    fixture.cleanup = matches!(scenario, Scenario::Final) && observed.is_ok() && drained.is_ok();
    observed.unwrap();
    drained.unwrap();
}
fn rows(store: &Store, kind: &str) -> Result<Vec<StoredEvent>> {
    store.events(&EventQuery {
        kind: Some(kind.into()),
        limit: Some(16),
        ..EventQuery::default()
    })
}
async fn run_consumer(
    fixture: &Fixture,
    context: &AuthenticatedBatchContext,
    full: &EnforcerFullClientV3,
    slot: EnforcedSlot,
    delivery: &Delivery,
    scenario: Scenario,
) -> Result<()> {
    let operation = fixture.intent.operation_id();
    let arm = fixture.registration.arm();
    let arm_id = Uuid::parse_str(&arm.operation_id)?;
    let generation = context.expected_generation();
    let store = fixture.store.as_ref();
    let writer = WriterClient::start(fixture.journal.clone(), 16, Duration::from_secs(10))
        .context("consumer.writer.start")?;
    writer
        .bind_full3_registration(
            operation,
            fixture.head.clone(),
            slot.full3_registration_binding()?,
        )
        .await
        .context("consumer.bind.original")?;
    context
        .preclaim()
        .announce_registration(operation, arm_id, full.epoch(), generation)?;
    context
        .preclaim()
        .mark_original_unclaimed(arm_id, full.epoch(), generation)?;
    let request = AbandonBeforeClaimV1 {
        version: 1,
        operation_id: operation,
        arm_id,
        command_id: Uuid::new_v4(),
        expected_epoch: full.epoch(),
        generation,
    };
    let signal = context.preclaim().request(request.clone())?;
    let mut event = signal.event().clone();
    let data = fixture.intent.request_data();
    event.session_id = data.session_id.clone();
    event.project_id = data.project_id.clone();
    event.sandbox_id = data.sandbox_id.clone();
    event.agent_id = data.agent_id.clone();
    event.correlation_id = data.correlation_id.clone();
    event.trace_id = data.trace_id.clone();
    let signal_payload = canonical(&event)?;
    let pending_signal = writer
        .enqueue_event(operation, event.clone())
        .await
        .context("consumer.signal.enqueue")?;
    let signal_seq = context
        .client()
        .post_event(serde_json::to_value(&event)?)
        .await
        .context("consumer.signal.http")?;
    let signal_ack = writer
        .acknowledge_event(operation, request.command_id, signal_seq)
        .await
        .context("consumer.signal.writer_ack")?;
    ensure!(
        signal_ack.audit_seq() == Some(signal_seq)
            && signal_ack.input_hash() == pending_signal.input_hash(),
        "signal ACK triple diverged before consumer entry"
    );
    signal.confirm_audit()?;
    ensure!(
        matches!(
            context
                .preclaim()
                .choose_claim(arm_id, full.epoch(), generation),
            Err(crate::broker::service::protocol::BrokerErrorCodeV1::Conflict)
        ),
        "selected signal did not inhibit claim"
    );
    let worker_before = canonical(
        &store
            .worker(&arm.worker_id)?
            .ok_or_else(|| anyhow!("consumer worker missing"))?,
    )?;
    let quota_before = canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?;
    let usage_before = canonical(&store.worker_usage()?)?;
    let deadline_before = arm.deadline_ns_decimal.clone();
    let executor = BatchExecutor::new(
        writer.clone(),
        ExecutionEnforcerV1::Full3(full.clone()),
        Arc::new(NativeHostClock::new()?),
    );
    let execution = Execution::open(&executor, operation, context)
        .await
        .context("consumer.execution.open")?;
    // All revocation, Begin, audit, authorization, native retirement and Final
    // below run ONLY inside the production consumer. No test proof factory or
    // manual substitute for any of those calls exists in this module.
    let outcome = execution
        .retire_unused_before_claim(slot, arm, signal)
        .await;
    let loaded = writer
        .load(operation)
        .await
        .context("consumer.inspect.confirmed")?
        .ok_or_else(|| anyhow!("consumer job disappeared"))?;
    let checkpoint = loaded
        .broker_checkpoint_v5()
        .ok_or_else(|| anyhow!("consumer lost V5"))?;
    ensure!(
        checkpoint.retirements().len() == 1,
        "consumer created zero or multiple retirement requests"
    );
    let entry = &checkpoint.retirements()[0];
    let pending = entry.request();
    ensure!(
        *pending.identity().retirement_id() == request.command_id
            && pending.identity().arm().operation_id == arm.operation_id
            && *pending.identity().original_epoch() == full.epoch()
            && pending.identity().arm().deadline_ns_decimal == deadline_before,
        "consumer regenerated request/Arm/epoch or extended the original deadline"
    );
    let intention_id = pending.intention_event_id();
    let final_id = pending.final_event_id();
    ensure!(
        intention_id != final_id
            && intention_id != request.command_id
            && final_id != request.command_id,
        "consumer failed to fix distinct event IDs"
    );
    let view = context.preclaim().arm_view(arm_id);
    match scenario {
        Scenario::Final => {
            let PrearmedStartDirective::AbandonedBeforeClaim(retirement) =
                outcome.context("consumer.retire.success")?
            else {
                return Err(anyhow!("consumer returned a start/observe directive"));
            };
            let finalized = entry
                .finalized()
                .ok_or_else(|| anyhow!("consumer returned abandonment before confirmed F"))?;
            let birth = loaded
                .outbox_receipt(final_id, None)
                .ok_or_else(|| anyhow!("Final event birth receipt missing"))?;
            ensure!(
                retirement.operation_id() == operation
                    && retirement.arm_id() == arm_id
                    && retirement.final_revision() == birth.head(),
                "abandon result does not bind its exact durable F head"
            );
            ensure!(
                !full.has_local_slot(&arm.operation_id)?
                    && view.state == PreclaimStateV1::Retiring
                    && view.retirement_continuation_retained == Some(false),
                "successful consumer retained a live continuation or slot"
            );
            let verified = full
                .query_retirement(pending.identity())
                .await?
                .ok_or_else(|| anyhow!("authentic Q missing after consumer F"))?;
            ensure!(
                verified.receipt().digest()? == finalized.receipt().digest()?,
                "consumer F differs from authentic native Q"
            );
            let intentions = rows(store, INTENTION)?;
            let finals = rows(store, FINAL)?;
            ensure!(
                intentions.len() == 1
                    && intentions[0].event.id == intention_id.to_string()
                    && finals.len() == 1
                    && finals[0].event.id == final_id.to_string(),
                "consumer audit event IDs duplicated or changed"
            );
            let ack = pending
                .audit_ack()
                .ok_or_else(|| anyhow!("Final pending has no actual intention ACK"))?;
            ensure!(
                *ack.event_id() == intention_id
                    && *ack.audit_seq() == intentions[0].seq
                    && ack.input_hash() == pending.intent_input_hash(),
                "consumer did not persist actual intention ACK triple"
            );
            ensure!(
                checkpoint
                    .outbox()
                    .events()
                    .iter()
                    .any(|event| event.event_id() == final_id
                        && event.audit_seq() == Some(finals[0].seq)
                        && event.input_hash() == finalized.final_input_hash().as_str()),
                "consumer did not persist the real Final audit ACK"
            );
            ensure!(
                writer
                    .stops()
                    .pending_retirement_events(operation)
                    .await?
                    .is_empty(),
                "successful consumer left a final audit event unacknowledged"
            );
        }
        Scenario::IntentionUnavailable => {
            let Err(error) = outcome else {
                return Err(anyhow!(
                    "503 consumer unexpectedly returned a completion directive"
                ));
            };
            ensure!(
                !delivery.fail_intention.load(Ordering::SeqCst)
                    && error.chain().any(|cause| cause
                        .to_string()
                        .contains("gateway rejected the operation (HTTP 503")),
                "consumer failure did not preserve the actual intention HTTP503 cause"
            );
            ensure!(
                entry.finalized().is_none()
                    && pending.audit_ack().is_none()
                    && view.state == PreclaimStateV1::AbandonRequested
                    && view.audit_confirmed
                    && view.retirement_continuation_retained == Some(true),
                "503 consumer lost its genuine Pending owner or fabricated authorization/F"
            );
            ensure!(
                full.has_local_slot(&arm.operation_id)? && full.status().await?.slot_count() == 1,
                "503 consumer released its original local/native capacity"
            );
            ensure!(
                full.query_retirement(pending.identity()).await?.is_none(),
                "503 intention audit produced a native Q"
            );
            ensure!(
                rows(store, INTENTION)?.is_empty() && rows(store, FINAL)?.is_empty(),
                "503 before Store commit invented a durable retirement audit"
            );
            ensure!(
                checkpoint
                    .outbox()
                    .events()
                    .iter()
                    .any(|event| event.event_id() == intention_id && event.audit_seq().is_none()),
                "503 consumer dropped its fixed pending intention event"
            );
        }
    }
    let actual_signal = rows(store, SIGNAL)?;
    ensure!(
        actual_signal.len() == 1
            && actual_signal[0].seq == signal_seq
            && canonical(&actual_signal[0].event)? == signal_payload,
        "consumer changed the original audited preclaim signal"
    );
    let payloads = delivery
        .event_payloads
        .lock()
        .map_err(|_| anyhow!("consumer delivery observation poisoned"))?
        .clone();
    let expected_posts = if matches!(scenario, Scenario::Final) {
        3
    } else {
        2
    };
    ensure!(
        payloads.len() == expected_posts
            && delivery.event_posts.load(Ordering::SeqCst) == expected_posts
            && delivery.business_posts.load(Ordering::SeqCst) == 0,
        "consumer repeated audit delivery or emitted a business HTTP mutation"
    );
    for body in payloads {
        let event: Event = serde_json::from_slice(&body)?;
        let stored = checkpoint
            .outbox()
            .events()
            .iter()
            .find(|stored| stored.event_id().to_string() == event.id)
            .ok_or_else(|| anyhow!("consumer HTTP event absent from exact outbox"))?;
        ensure!(
            canonical(&event)? == canonical(stored.event())?,
            "consumer HTTP payload differs from its original durable event"
        );
    }
    ensure!(
        !loaded
            .broker_checkpoint_v2()
            .ok_or_else(|| anyhow!("consumer supervision missing"))?
            .supervision()
            .operations()
            .iter()
            .any(|operation| matches!(
                operation,
                Operation::WorkerClaim { .. } | Operation::HelperClaim { .. }
            )),
        "consumer emitted a durable start claim"
    );
    ensure!(
        worker_before
            == canonical(
                &store
                    .worker(&arm.worker_id)?
                    .ok_or_else(|| anyhow!("consumer worker disappeared"))?
            )?
            && quota_before == canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?
            && usage_before == canonical(&store.worker_usage()?)?,
        "consumer invented central terminal/quota/usage changes"
    );
    Ok(())
}
