//! Service dispatch → authenticated signal/audit before claim. This composed
//! test uses real SQLite/Journal/Writer and a real native Full3 original, with
//! the explicit SyntheticReady fixture. It does not exercise arm_and_claim,
//! an in-flight Register race, a business engine, or the complete runtime path.
#[path = "preclaim_shutdown_tests.rs"]
mod shutdown_tests;

use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    broker::{
        enforcer::{EnforcedSlot, retirement::test_support::OriginalUnusedFixture},
        intent::ExecutionIntentV1,
        journal::retirement_live_fixture::{
            INVOCATION_DATA_PROGRAM, LiveFixture, live_fixture_with_gateway,
        },
        service::preclaim::{AbandonBeforeClaimV1, PreclaimStateV1},
        writer::BeginUnusedRetirement,
    },
    control::QuotaPolicyV1,
    runner::AuditClient,
};
use anyhow::Context;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::{Mutex, atomic::AtomicUsize},
};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

// Public fixture credentials, never environment/keychain/provider credentials.
const RUNNER: &str = "runner-preclaim-live-aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const VIEWER: &str = "viewer-preclaim-live-bbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SIGNAL: &str = "broker.abandon-before-claim.requested";

#[derive(Default)]
struct Delivery {
    fail_first_signal: AtomicBool,
    control_event_posts: AtomicUsize,
    signal_posts: AtomicUsize,
    injected_503: AtomicUsize,
    payloads: Mutex<Vec<Vec<u8>>>,
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
                    if request.method() != axum::http::Method::POST
                        || request.uri().path() != "/control/api/v1/control/events"
                    {
                        return next.run(request).await;
                    }
                    delivery.control_event_posts.fetch_add(1, Ordering::SeqCst);
                    let (parts, body) = request.into_parts();
                    let bytes = match axum::body::to_bytes(body, 128 * 1024).await {
                        Ok(bytes) => bytes,
                        Err(_) => {
                            return axum::response::IntoResponse::into_response(
                                axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                            );
                        }
                    };
                    let signal = serde_json::from_slice::<Event>(&bytes)
                        .ok()
                        .is_some_and(|event| event.kind == SIGNAL);
                    if signal {
                        delivery.signal_posts.fetch_add(1, Ordering::SeqCst);
                        if let Ok(mut payloads) = delivery.payloads.lock()
                            && payloads.len() < 3
                        {
                            payloads.push(bytes.to_vec());
                        }
                        if delivery.fail_first_signal.swap(false, Ordering::SeqCst) {
                            delivery.injected_503.fetch_add(1, Ordering::SeqCst);
                            // Owned, bounded transport fault before SQLite commit.
                            // Every successful retry reaches the real gateway router.
                            return axum::response::IntoResponse::into_response(
                                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                            );
                        }
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
                    return Err(anyhow!("owned preclaim gateway did not drain"));
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

fn private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}
fn registry(
    root: &Path,
    scope: &JournalScopeV1,
    intent: &ExecutionIntentV1,
    filesystem_roots: &Path,
) -> Result<Arc<Registry>> {
    let config = root.join("config");
    for name in ["sources", "exports"] {
        fs::create_dir(filesystem_roots.join(name))?;
        fs::set_permissions(
            filesystem_roots.join(name),
            fs::Permissions::from_mode(0o700),
        )?;
    }
    private_file(&config.join("runner.token"), RUNNER.as_bytes())?;
    let options = intent.initial_options().run_options();
    let path = config.join("registry.json");
    private_file(
        &path,
        &serde_json::to_vec(&json!({
            "version":1,"installation_id":scope.installation_id(),"root":root,"max_enforcer_slots":8,
            "profiles":[{"id":scope.profile_id(),"gateway":scope.gateway_origin(),"tenant_id":scope.tenant_id(),
                "control_namespace_uuid":scope.control_namespace_uuid(),"config_dir":config,
                "runner_token_file":config.join("runner.token"),"runtime":"docker","runtime_program":INVOCATION_DATA_PROGRAM,
                "runtime_endpoint":options.runtime_endpoint,"image":options.image,"policy_profile":options.profile,
                "network":options.network,"allow_rootful_runtime":false,"max_timeout_seconds":120,
                "allow_capture_output":false,"managed_clients":false,"manual_credential_file":options.secret_env_file,
                "source_roots":[filesystem_roots.join("sources")],"export_roots":[filesystem_roots.join("exports")]}]
        }))?,
    )?;
    Ok(Arc::new(Registry::load(&path)?))
}
fn envelope(profile: &str, credential: &str, signal: &AbandonBeforeClaimV1) -> Result<EnvelopeV1> {
    // Exercise the production closed request codec as well as dispatch auth.
    Ok(serde_json::from_slice(&serde_json::to_vec(&EnvelopeV1 {
        version: 1,
        profile_id: profile.into(),
        credential: credential.into(),
        request: BrokerCommandV1::AbandonBeforeClaim {
            request: signal.clone(),
        },
    })?)?)
}
fn rows(store: &Store, kind: &str) -> Result<Vec<crate::model::StoredEvent>> {
    store.events(&EventQuery {
        kind: Some(kind.into()),
        limit: Some(16),
        ..EventQuery::default()
    })
}

struct ProducerFixture {
    _root: tempfile::TempDir,
    _store_root: tempfile::TempDir,
    store: Arc<Store>,
    journal: Journal,
    intent: ExecutionIntentV1,
    head: crate::broker::journal::RevisionRefV1,
    registration: RegistrationV1,
}
impl From<LiveFixture> for ProducerFixture {
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
            _root,
            _store_root,
            store: Arc::new(store),
            journal,
            intent,
            head,
            registration,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_preclaim_signal_503_retains_original_and_exact_retry_reaches_real_q_f() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let live = ProducerFixture::from(live_fixture_with_gateway(&origin, true));
    let scope = live.journal.scope().clone();
    let store = Arc::clone(&live.store);
    let delivery = Arc::new(Delivery::default());
    delivery.fail_first_signal.store(true, Ordering::SeqCst);
    let gateway = OwnedGateway::start(
        listener,
        Arc::clone(&store),
        scope.tenant_id(),
        Arc::clone(&delivery),
    )
    .await
    .unwrap();
    let (native, full, slot) =
        OriginalUnusedFixture::register(scope.installation_id(), live.registration.clone())
            .await
            .unwrap();
    let observed = run_producer(&live, &full, &slot, &delivery, &origin).await;
    native.finish().await;
    let drained = gateway.finish().await;
    if observed.is_err() || drained.is_err() {
        eprintln!(
            "private failed preclaim journal retained: {}",
            live._root.keep().display()
        );
        eprintln!(
            "private failed preclaim store retained: {}",
            live._store_root.keep().display()
        );
    }
    observed.unwrap();
    drained.unwrap();
}

async fn run_producer(
    live: &ProducerFixture,
    full: &EnforcerFullClientV3,
    slot: &EnforcedSlot,
    delivery: &Delivery,
    origin: &str,
) -> Result<()> {
    let scope = live.journal.scope().clone();
    let store = live.store.as_ref();
    let writer = WriterClient::start(live.journal.clone(), 16, Duration::from_secs(10))
        .context("preclaim.writer.start")?;
    writer
        .bind_full3_registration(
            live.intent.operation_id(),
            live.head.clone(),
            slot.full3_registration_binding()?,
        )
        .await
        .context("preclaim.writer.bind")?;
    let root = live._root.path().canonicalize()?;
    let registry = registry(
        &root,
        &scope,
        &live.intent,
        &live._store_root.path().canonicalize()?,
    )?;
    let material = Arc::new(Material::load(&registry)?);
    let jobs = Arc::new(ProfileJobs::new());
    jobs.writer
        .set(ProfileWriterCapabilities::from_full(writer.clone()))
        .map_err(|_| anyhow!("preclaim writer factory repeated"))?;
    let local = jobs.job(live.intent.operation_id())?;
    let arm = slot.full3_registration_binding()?.arm_id();
    // Real Register is complete. These are the production coordinator
    // transitions before claim; there is deliberately no Register-race claim.
    local
        .preclaim
        .announce_registration(live.intent.operation_id(), arm, full.epoch(), 0)?;
    local
        .preclaim
        .mark_original_unclaimed(arm, full.epoch(), 0)?;
    let service = Arc::new(Service {
        full_owners: FullOwners::new(registry.data.max_enforcer_slots as usize)?,
        cold_owners: retained_audit::ColdOwners::new(registry.data.max_enforcer_slots as usize)?,
        registry,
        root: Directory::open_existing(&root)?,
        material,
        profiles: BTreeMap::from([(scope.profile_id().to_owned(), Arc::clone(&jobs))]),
        enforcer: Some(ServiceEnforcer::Native(ExecutionEnforcerV1::Full3(
            full.clone(),
        ))),
        stop_enforcer: None,
        blocking: Arc::new(Semaphore::new(2)),
        preparation: Arc::new(Semaphore::new(1)),
        execution: Arc::new(Semaphore::new(1)),
        stopping: Arc::new(AtomicBool::new(false)),
        fenced: Arc::new(AtomicBool::new(false)),
    });
    let signal = AbandonBeforeClaimV1 {
        version: 1,
        operation_id: live.intent.operation_id(),
        arm_id: arm,
        command_id: Uuid::new_v4(),
        expected_epoch: full.epoch(),
        generation: 0,
    };
    let worker_id = live.registration.arm().worker_id.clone();
    let worker_before = canonical(
        &store
            .worker(&worker_id)?
            .ok_or_else(|| anyhow!("preclaim central worker absent"))?,
    )?;
    let quota_before = canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?;
    let usage_before = canonical(&store.worker_usage()?)?;
    for (credential, expected) in [
        ("", BrokerErrorCodeV1::Unauthorized),
        (VIEWER, BrokerErrorCodeV1::Forbidden),
    ] {
        ensure!(
            matches!(service.dispatch(envelope(scope.profile_id(), credential, &signal)?).await, Err(code) if code == expected),
            "preclaim authorization did not refuse unavailable runner authority"
        );
        ensure!(
            local.preclaim.arm_view(arm).state == PreclaimStateV1::OriginalUnclaimed
                && rows(store, SIGNAL)?.is_empty(),
            "refused credential mutated preclaim or audit"
        );
    }
    let control_posts_before = delivery.control_event_posts.load(Ordering::SeqCst);
    let signal_posts_before = delivery.signal_posts.load(Ordering::SeqCst);
    let injected_503_before = delivery.injected_503.load(Ordering::SeqCst);
    let first_signal = service
        .dispatch(envelope(scope.profile_id(), RUNNER, &signal)?)
        .await;
    let first_signal_status = match &first_signal {
        Ok(_) => "Ok".to_owned(),
        Err(code) => format!("Err({code:?})"),
    };
    let control_posts = delivery
        .control_event_posts
        .load(Ordering::SeqCst)
        .saturating_sub(control_posts_before);
    let signal_posts = delivery
        .signal_posts
        .load(Ordering::SeqCst)
        .saturating_sub(signal_posts_before);
    let injected_503 = delivery
        .injected_503
        .load(Ordering::SeqCst)
        .saturating_sub(injected_503_before);
    ensure!(
        matches!(first_signal, Err(BrokerErrorCodeV1::Unavailable)),
        "phase=preclaim_first_signal_dispatch status={first_signal_status} control_route_posts={control_posts} preclaim_kind_posts={signal_posts} injected_503={injected_503}"
    );
    let view = local.preclaim.arm_view(arm);
    ensure!(
        view.state == PreclaimStateV1::AbandonPendingAudit
            && !view.audit_confirmed
            && view.command_id == Some(signal.command_id),
        "failed audit dropped the reserved inhibit branch"
    );
    ensure!(
        matches!(
            local.preclaim.choose_claim(arm, full.epoch(), 0),
            Err(BrokerErrorCodeV1::Conflict)
        ),
        "failed signal audit reopened claim"
    );
    ensure!(
        full.has_local_slot(&arm.to_string())?
            && full.status().await?.slot_count() == 1
            && rows(store, SIGNAL)?.is_empty(),
        "audit outage lost the original owner or invented a durable audit"
    );
    let failed = writer
        .load(signal.operation_id)
        .await?
        .ok_or_else(|| anyhow!("preclaim actual job absent"))?;
    ensure!(
        failed
            .broker_checkpoint_v5()
            .is_some_and(|cp| cp.retirements().is_empty()),
        "signal audit failure invented a retirement witness/Pending"
    );
    let retained_signal = failed
        .broker_checkpoint_v4()
        .and_then(|cp| {
            cp.events()
                .iter()
                .find(|event| event.event_id() == signal.command_id)
        })
        .ok_or_else(|| anyhow!("failed signal absent from real outbox"))?;
    let signal_payload = canonical(retained_signal.event())?;
    let signal_input_hash = retained_signal.input_hash().to_owned();
    ensure!(
        retained_signal.audit_seq().is_none(),
        "failed signal was prematurely ACKed"
    );
    drop(failed);
    let reply = service
        .dispatch(envelope(scope.profile_id(), RUNNER, &signal)?)
        .await
        .context("preclaim.dispatch.exact_retry")?;
    ensure!(
        reply.run.is_none(),
        "signal unexpectedly scheduled business execution"
    );
    ensure!(
        matches!(&reply.reply, BrokerReplyV1::AbandonRequested { operation_id, arm_id, command_id, input_hash, .. }
            if *operation_id == signal.operation_id && *arm_id == arm && *command_id == signal.command_id && input_hash == &signal_input_hash),
        "signal retry changed its request identity/hash"
    );
    let reply_bytes = canonical(&reply.reply)?;
    let stable = service
        .dispatch(envelope(scope.profile_id(), RUNNER, &signal)?)
        .await?;
    ensure!(
        stable.run.is_none() && canonical(&stable.reply)? == reply_bytes,
        "exact signal replay changed its durable ACK/revision"
    );
    let payloads = delivery
        .payloads
        .lock()
        .map_err(|_| anyhow!("preclaim delivery observation poisoned"))?
        .clone();
    ensure!(
        delivery.signal_posts.load(Ordering::SeqCst) == 3
            && payloads.len() == 3
            && payloads.windows(2).all(|pair| pair[0] == pair[1])
            && canonical(&serde_json::from_slice::<Event>(&payloads[0])?)? == signal_payload,
        "signal retry regenerated its timestamp/IDs/payload"
    );
    let audited = rows(store, SIGNAL)?;
    ensure!(
        audited.len() == 1 && canonical(&audited[0].event)? == signal_payload,
        "signal retry duplicated or changed its actual SQLite event"
    );
    let current = writer
        .load(signal.operation_id)
        .await?
        .ok_or_else(|| anyhow!("preclaim retry job absent"))?;
    let signal_entry = current
        .broker_checkpoint_v4()
        .and_then(|cp| {
            cp.events()
                .iter()
                .find(|event| event.event_id() == signal.command_id)
        })
        .ok_or_else(|| anyhow!("ACKed signal absent"))?;
    ensure!(
        signal_entry.audit_seq() == Some(audited[0].seq)
            && signal_entry.input_hash() == signal_input_hash,
        "signal durable ACK differs from actual SQLite sequence/hash"
    );
    let expected = current.head().clone();
    drop(current);
    let selected = local
        .preclaim
        .signal_for_arm(arm, full.epoch(), 0)?
        .ok_or_else(|| anyhow!("retained signal owner absent"))?;
    let persisted_signal: Event = serde_json::from_slice(&payloads[0])?;
    // Dispatch adds the confirmed job context to a clone of this retained
    // base event. Its timestamp, ID and closed request data stay original.
    let confirmed_request =
        tokio::time::timeout(Duration::from_secs(2), selected.wait_audit_confirmed())
            .await
            .context("preclaim.selected_signal.audit_wait")??;
    ensure!(
        confirmed_request == signal
            && selected.event().timestamp == persisted_signal.timestamp
            && selected.event().id == persisted_signal.id
            && selected.event().data == persisted_signal.data,
        "confirmed signal owner changed request"
    );
    ensure!(
        matches!(
            local.preclaim.choose_claim(arm, full.epoch(), 0),
            Err(BrokerErrorCodeV1::Conflict)
        ),
        "durably audited abandon reopened claim"
    );
    complete_real_retirement(&writer, full, slot, store, origin, &signal, expected).await?;
    ensure!(
        worker_before
            == canonical(
                &store
                    .worker(&worker_id)?
                    .ok_or_else(|| anyhow!("preclaim worker disappeared"))?
            )?
            && quota_before == canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?
            && usage_before == canonical(&store.worker_usage()?)?,
        "preclaim retirement invented terminal/quota/usage changes"
    );
    Ok(())
}

// The actual wrappers are produced below their real service signal. This is a
// scoped producer composition, not a test-only imitation of arm_and_claim.
async fn complete_real_retirement(
    writer: &WriterClient,
    full: &EnforcerFullClientV3,
    slot: &EnforcedSlot,
    store: &Store,
    origin: &str,
    signal: &AbandonBeforeClaimV1,
    expected: crate::broker::journal::RevisionRefV1,
) -> Result<()> {
    let unused = slot
        .revoke_unused(signal.command_id)
        .await
        .context("preclaim.native.revoke_unused")?;
    let pending = writer
        .begin_unused_retirement(BeginUnusedRetirement::new(
            signal.operation_id,
            expected,
            unused,
        )?)
        .await
        .context("preclaim.writer.begin")?;
    ensure!(
        *pending.identity().retirement_id() == signal.command_id
            && pending.identity().arm_receipt().arm_id == signal.arm_id.to_string(),
        "Pending no longer targets the original service signal"
    );
    let identity = pending.identity().clone();
    let intention_id = pending.intention_event_id();
    let final_id = pending.final_event_id();
    let client = AuditClient::new(origin, RUNNER)?;
    let seq = client
        .post_control_event(serde_json::to_value(pending.intention_event())?)
        .await?;
    writer
        .acknowledge_event(signal.operation_id, intention_id, seq)
        .await?;
    let authorized = writer
        .authorize_unused_retirement(pending)
        .await
        .context("preclaim.writer.authorize")?;
    let verified = full
        .retire_unused(authorized)
        .await
        .context("preclaim.native.retire_join_q")?;
    let q_hash = verified.receipt().digest()?;
    writer
        .stops()
        .finalize_retirement(verified)
        .await
        .context("preclaim.writer.finalize")?;
    let replay = full
        .query_retirement(&identity)
        .await?
        .ok_or_else(|| anyhow!("real Q replay absent"))?;
    ensure!(
        replay.receipt().digest()? == q_hash,
        "authenticated Q replay changed its original receipt"
    );
    full.retire_local_slot(&replay)?;
    ensure!(
        !full.has_local_slot(&signal.arm_id.to_string())?,
        "Final did not release exact local slot"
    );
    let events = writer
        .stops()
        .pending_retirement_events(signal.operation_id)
        .await?;
    ensure!(
        events.len() == 1 && events[0].event_id() == final_id,
        "service signal produced multiple final events"
    );
    let final_seq = client
        .post_control_event(serde_json::to_value(events[0].event())?)
        .await?;
    writer
        .stops()
        .acknowledge_retirement_events(
            signal.operation_id,
            vec![(final_id, events[0].input_hash().to_owned(), final_seq)],
        )
        .await?;
    let loaded = writer
        .load(signal.operation_id)
        .await?
        .ok_or_else(|| anyhow!("actual Final absent"))?;
    let cp = loaded
        .broker_checkpoint_v5()
        .ok_or_else(|| anyhow!("actual Final lost V5"))?;
    let finalized = cp
        .retirements()
        .first()
        .and_then(|entry| entry.finalized())
        .ok_or_else(|| anyhow!("verified Q did not produce confirmed Final"))?;
    ensure!(
        cp.retirements().len() == 1 && finalized.receipt().digest()? == q_hash,
        "actual service signal Q/Final mismatch"
    );
    ensure!(
        cp.outbox().events().iter().any(|event| {
            event.event_id() == final_id
                && event.audit_seq() == Some(final_seq)
                && event.input_hash() == events[0].input_hash()
        }),
        "final audit ACK was not durably retained with its exact payload hash"
    );
    ensure!(
        loaded
            .broker_checkpoint_v2()
            .is_some_and(
                |checkpoint| !checkpoint.supervision().operations().iter().any(
                    |operation| matches!(
                        operation,
                        crate::broker::checkpoint::SupervisionOperationV1::WorkerClaim { .. }
                            | crate::broker::checkpoint::SupervisionOperationV1::HelperClaim { .. }
                    )
                )
            ),
        "preclaim signal composition emitted a durable start claim"
    );
    let intention_rows = rows(store, "broker.retirement.pending")?;
    let final_rows = rows(store, "broker.retirement.final")?;
    ensure!(
        intention_rows.len() == 1
            && intention_rows[0].event.id == intention_id.to_string()
            && final_rows.len() == 1
            && final_rows[0].event.id == final_id.to_string(),
        "retirement audit uniqueness differs from SQLite"
    );
    Ok(())
}
