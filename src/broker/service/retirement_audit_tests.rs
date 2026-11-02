//! Real service retirement pump → authenticated local gateway → SQLite →
//! metadata-only Stops ACK. The native original uses the explicit SyntheticReady
//! test runtime seam; this is not engine/provider/service-release qualification.
use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    broker::{
        enforcer::retirement::test_support::OriginalUnusedFixture,
        intent::ExecutionIntentV1,
        journal::{
            BindingInputs,
            retirement_live_fixture::{INVOCATION_DATA_PROGRAM, live_fixture_with_gateway},
        },
        writer::BeginUnusedRetirement,
    },
    control::QuotaPolicyV1,
    runner::{AuditClient, CheckpointPayloadV1},
};
use anyhow::Context;
use std::cell::Cell;
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::{Mutex, atomic::AtomicUsize},
};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

// Public synthetic test credentials; no environment/keychain/provider material.
const RUNNER: &str = "runner-retirement-audit-aaaaaaaaaaaaaaaaaaaaaaaa";
const VIEWER: &str = "viewer-retirement-audit-bbbbbbbbbbbbbbbbbbbbbbb";

/// Closed labels/durations only. Context preserves the original typed error;
/// no event body, credential, command, raw HTTP response or nonce is logged.
struct PhaseTrace(Cell<(&'static str, std::time::Instant)>);
impl PhaseTrace {
    fn new() -> Self {
        Self(Cell::new(("test.start", std::time::Instant::now())))
    }
    fn enter(&self, label: &'static str) {
        let (previous, started) = self.0.replace((label, std::time::Instant::now()));
        eprintln!(
            "d5-retirement-audit completed={previous} elapsed_ms={} next={label}",
            started.elapsed().as_millis()
        );
    }
    fn failure(&self, error: anyhow::Error) -> anyhow::Error {
        let (phase, started) = self.0.get();
        let kind = closed_error_kind(&error);
        error.context(format!(
            "d5-retirement-audit phase={phase} elapsed_ms={} error_type={kind}",
            started.elapsed().as_millis()
        ))
    }
}
fn closed_error_kind(error: &anyhow::Error) -> String {
    if let Some(code) = error.downcast_ref::<crate::broker::journal::JournalError>() {
        return format!("JournalError::{code:?}");
    }
    if let Some(code) = error.downcast_ref::<crate::broker::enforcer::EnforcerError>() {
        return format!("EnforcerError::{code:?}");
    }
    if let Some(code) = error.downcast_ref::<BrokerErrorCodeV1>() {
        return format!("BrokerErrorCodeV1::{code:?}");
    }
    macro_rules! completion {
        ($input:ty,$name:literal) => {
            if let Some(failure) = error.downcast_ref::<CompletionFailure<$input>>() {
                let state = match failure {
                    CompletionFailure::NotQueued { .. } => "NotQueued",
                    CompletionFailure::Unknown { .. } => "Unknown",
                };
                return format!("{}::{state}({:?})", $name, failure.error());
            }
        };
    }
    completion!(BeginUnusedRetirement, "BeginCompletion");
    completion!(
        crate::broker::writer::PendingUnusedRetirement,
        "AuthorizeCompletion"
    );
    if let Some(code) = error.downcast_ref::<std::io::Error>() {
        return format!("IoError::{:?}", code.kind());
    }
    "OtherPreservedError".into()
}

#[derive(Default)]
struct GatewayObservations {
    runner_identity_requests: AtomicUsize,
    runner_final_posts: AtomicUsize,
    lose_next_final_ack: AtomicBool,
    final_payloads: Mutex<Vec<Vec<u8>>>,
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
        observed: Arc<GatewayObservations>,
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
                let observed = Arc::clone(&observed);
                async move {
                    let runner = request
                        .headers()
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|header| header.to_str().ok())
                        .is_some_and(|header| header == format!("Bearer {RUNNER}"));
                    if runner && request.uri().path() == "/api/v1/control/protocol" {
                        observed
                            .runner_identity_requests
                            .fetch_add(1, Ordering::SeqCst);
                    }
                    if request.method() != axum::http::Method::POST
                        || request.uri().path() != "/api/events"
                    {
                        return next.run(request).await;
                    }
                    let (parts, body) = request.into_parts();
                    let bytes = match axum::body::to_bytes(body, 128 * 1024).await {
                        Ok(bytes) => bytes,
                        Err(_) => {
                            return axum::response::IntoResponse::into_response(
                                axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                            );
                        }
                    };
                    let final_event = runner
                        && serde_json::from_slice::<Value>(&bytes)
                            .ok()
                            .is_some_and(|event| {
                                event.get("kind").and_then(Value::as_str)
                                    == Some("broker.retirement.final")
                            });
                    if final_event {
                        observed.runner_final_posts.fetch_add(1, Ordering::SeqCst);
                        // Two bounded actual request bodies, never auth headers.
                        if let Ok(mut payloads) = observed.final_payloads.lock()
                            && payloads.len() < 3
                        {
                            payloads.push(bytes.to_vec());
                        }
                    }
                    let request =
                        axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
                    let response = next.run(request).await;
                    if final_event
                        && response.status().is_success()
                        && observed.lose_next_final_ack.swap(false, Ordering::SeqCst)
                    {
                        // The real gateway/Store has already durably committed.
                        // Only this owned test response is made unavailable.
                        axum::response::IntoResponse::into_response(
                            axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        )
                    } else {
                        response
                    }
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
                    return Err(anyhow!("owned test gateway did not drain"));
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

fn configured_registry(
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
            "version":1,"installation_id":scope.installation_id(),"root":root,
            "max_enforcer_slots":8,"profiles":[{
                "id":scope.profile_id(),"gateway":scope.gateway_origin(),"tenant_id":scope.tenant_id(),
                "control_namespace_uuid":scope.control_namespace_uuid(),"config_dir":config,
                "runner_token_file":config.join("runner.token"),"runtime":"docker",
                "runtime_program":INVOCATION_DATA_PROGRAM,"runtime_endpoint":options.runtime_endpoint,
                "image":options.image,"policy_profile":options.profile,"network":options.network,
                "allow_rootful_runtime":false,"max_timeout_seconds":120,"allow_capture_output":false,
                "managed_clients":false,"manual_credential_file":options.secret_env_file,
                "source_roots":[filesystem_roots.join("sources")],"export_roots":[filesystem_roots.join("exports")]
            }]
        }))?,
    )?;
    Ok(Arc::new(Registry::load(&path)?))
}

fn sibling_intent(intent: &ExecutionIntentV1) -> Result<ExecutionIntentV1> {
    let mut value = serde_json::to_value(intent)?;
    value["operation_id"] = json!(Uuid::new_v4());
    value["initial_options"]["secret_env_file"] = Value::Null;
    Ok(ExecutionIntentV1::from_bytes(&canonical(&value)?)?)
}

fn final_rows(store: &Store) -> Result<Vec<crate::model::StoredEvent>> {
    store.events(&EventQuery {
        kind: Some("broker.retirement.final".into()),
        limit: Some(16),
        ..EventQuery::default()
    })
}

#[derive(Clone, Copy)]
enum CompletionScenario {
    FinalBeforeFactory,
    PendingWithHeldFull3,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fenced_stops_service_pump_replays_exact_final_after_lost_http_ack_without_business_blob() {
    composed_retirement_audit(CompletionScenario::FinalBeforeFactory).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fenced_stops_service_pump_completes_real_q_without_restart_and_replays_exact_audit() {
    composed_retirement_audit(CompletionScenario::PendingWithHeldFull3).await;
}

async fn composed_retirement_audit(scenario: CompletionScenario) {
    let phases = PhaseTrace::new();
    phases.enter("fixture.tcp_listener");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    phases.enter("fixture.journal_sqlite_graph");
    let crate::broker::journal::retirement_live_fixture::LiveFixture {
        _root,
        _store_root,
        store,
        journal,
        intent,
        head,
        registration,
    } = live_fixture_with_gateway(&origin, true);
    let scope = journal.scope().clone();
    let store = Arc::new(store);
    let observed = Arc::new(GatewayObservations::default());
    observed.lose_next_final_ack.store(true, Ordering::SeqCst);
    phases.enter("fixture.http_gateway");
    let gateway = OwnedGateway::start(
        listener,
        Arc::clone(&store),
        scope.tenant_id(),
        Arc::clone(&observed),
    )
    .await
    .unwrap();
    phases.enter("fixture.native_original_register");
    let (native, full, slot) =
        OriginalUnusedFixture::register(scope.installation_id(), registration)
            .await
            .unwrap();
    let result: Result<()> = async {
        phases.enter("writer.start");
        let writer = WriterClient::start(journal.clone(), 16, Duration::from_secs(10))?;
        phases.enter("writer.bind_full3");
        writer
            .bind_full3_registration(
                intent.operation_id(),
                head,
                slot.full3_registration_binding()?,
            )
            .await?;
        let client = AuditClient::new(&origin, RUNNER)?;
        let command = Uuid::new_v4();
        let arm = slot.full3_registration_binding()?.arm_id();
        let mut signal = Event::new("broker.abandon-before-claim.requested");
        signal.id = command.to_string();
        signal.source = "ai-guard-broker".into();
        signal.coverage = "native".into();
        let request = intent.request_data();
        signal.session_id = request.session_id.clone();
        signal.project_id = request.project_id.clone();
        signal.sandbox_id = request.sandbox_id.clone();
        signal.correlation_id = request.correlation_id.clone();
        signal.trace_id = request.trace_id.clone();
        signal.data = json!({"version":1,"operation_id":intent.operation_id(),"arm_id":arm,
            "expected_epoch":full.epoch(),"generation":0,"command_id":command});
        phases.enter("signal.outbox_enqueue");
        writer
            .enqueue_event(intent.operation_id(), signal.clone())
            .await?;
        phases.enter("signal.http_audit");
        let sequence = client.post_event(serde_json::to_value(signal)?).await?;
        phases.enter("signal.writer_ack");
        writer
            .acknowledge_event(intent.operation_id(), command, sequence)
            .await?;
        phases.enter("native.revoke_unused");
        let unused = slot.revoke_unused(command).await?;
        phases.enter("writer.load_confirmed_head");
        let current = writer
            .load(intent.operation_id())
            .await?
            .ok_or_else(|| anyhow!("missing actual Arm job"))?;
        let current_head = current.head().clone();
        let manifest = current.binding_manifest();
        let file_name = manifest
            .file_name()
            .ok_or_else(|| anyhow!("manual fixture has no actual credential blob"))?
            .to_owned();
        let namespace_hash =
            crate::broker::journal::digest("ai-guard:broker-journal-scope:v1", &canonical(&scope)?);
        let journal_root = _root
            .path()
            .canonicalize()?
            .join("journals")
            .join(scope.profile_id());
        let job_path = journal_root
            .join("broker-journal-v1")
            .join(namespace_hash)
            .join("jobs")
            .join(intent.operation_id().to_string());
        let business_blob = job_path.join("bindings").join(file_name);
        anyhow::ensure!(
            business_blob.is_file(),
            "manual binding was not physically created"
        );
        drop(current);
        phases.enter("writer.begin_retirement");
        let pending = writer
            .begin_unused_retirement(BeginUnusedRetirement::new(
                intent.operation_id(),
                current_head,
                unused,
            )?)
            .await?;
        let worker_id = pending.identity().arm().worker_id.clone();
        let retirement_identity = pending.identity().clone();
        let expected_final_id = pending.final_event_id();
        let viewer_probe_event = pending.intention_event().clone();
        phases.enter("intention.http_audit");
        let sequence = client
            .post_event(serde_json::to_value(pending.intention_event())?)
            .await?;
        phases.enter("intention.writer_ack");
        writer
            .acknowledge_event(
                intent.operation_id(),
                pending.intention_event_id(),
                sequence,
            )
            .await?;
        phases.enter("writer.authorize_retirement");
        let authorized = writer.authorize_unused_retirement(pending).await?;
        phases.enter("native.retire_and_join_q");
        let verified = full.retire_unused(authorized).await?;
        let q_digest = verified.receipt().digest()?;

        phases.enter("sibling.reserve_and_accept");
        let other = sibling_intent(&intent)?;
        let ReserveResult::New(ticket) =
            journal.reserve(other.operation_id(), b"actual sibling B")?
        else {
            return Err(anyhow!("sibling is not fresh"));
        };
        drop(journal.accept(
            ticket,
            &other,
            BindingInputs::None,
            &CheckpointPayloadV1::initial(&other)?,
        )?);
        let other_job = job_path
            .parent()
            .ok_or_else(|| anyhow!("missing job parent"))?
            .join(other.operation_id().to_string());
        phases.enter("sibling.corrupt_current");
        fs::OpenOptions::new()
            .append(true)
            .open(other_job.join("current.json"))?
            .write_all(b"corrupt-owned-test-sibling-B")?;
        let preserved_blob = _root.path().join("preserved-manual-credential-blob.bin");
        phases.enter("credential.preserve_owned_blob");
        fs::rename(&business_blob, &preserved_blob)?;
        anyhow::ensure!(
            journal.load(intent.operation_id()).is_err(),
            "ordinary business load unexpectedly accepts absent credential blob"
        );
        phases.enter("control.snapshot_before");
        let worker_before = canonical(
            &store
                .worker(&worker_id)?
                .ok_or_else(|| anyhow!("missing central worker"))?,
        )?;
        let quota_before = canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?;
        let usage_before = canonical(&store.worker_usage()?)?;
        phases.enter("writer.release_before_recovery");
        drop(writer);
        drop(journal);

        let directory = Directory::open_existing(&journal_root)?;
        phases.enter("recovery.acquire_and_scan");
        let mut report = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match StopRecoveryReaderV1::open_existing(
                    &directory,
                    scope.clone(),
                    &FsExclusions::new(),
                    JournalLimitsV1::new(256 * 1024 * 1024, 256, 256, 512 * 1024 * 1024, 256, 128)?,
                    ControlSecrets::new(Vec::new())?,
                ) {
                    Ok(Some(reader)) => return Ok::<_, anyhow::Error>(reader.scan()),
                    Err(crate::broker::journal::JournalError::Busy) => {
                        tokio::time::sleep(Duration::from_millis(5)).await
                    }
                    _ => return Err(anyhow!("retained recovery reader unavailable")),
                }
            }
        })
        .await??;
        anyhow::ensure!(
            report.incomplete(),
            "missing blob and corrupt sibling must remain explicit"
        );
        phases.enter("recovery.final_or_pending");
        match scenario {
            CompletionScenario::FinalBeforeFactory => {
                report.finalize_retirement(verified)?;
                let replay = full
                    .query_retirement(&retirement_identity)
                    .await?
                    .ok_or_else(|| anyhow!("authenticated Q replay unavailable after F"))?;
                anyhow::ensure!(
                    replay.receipt().digest()? == q_digest,
                    "Q replay changed after F"
                );
                full.retire_local_slot(&replay)?;
                let finalized = report
                    .jobs()
                    .iter()
                    .find(|job| job.operation_id() == intent.operation_id())
                    .ok_or_else(|| anyhow!("known job disappeared"))?;
                let final_entry = finalized
                    .retirements()
                    .first()
                    .and_then(|entry| entry.finalized())
                    .ok_or_else(|| anyhow!("real verified Q did not produce F"))?;
                anyhow::ensure!(
                    final_entry.receipt().digest()? == q_digest && finalized.stops().is_empty(),
                    "F changed Q or restored a retired Arm"
                );
            }
            CompletionScenario::PendingWithHeldFull3 => {
                // Model the lost Retire reply by discarding the genuine wrapper.
                // Only the Service's retained authenticated Full3 can query Q;
                // this report is deliberately handed off with P, not F.
                drop(verified);
                let known = report
                    .jobs()
                    .iter()
                    .find(|job| job.operation_id() == intent.operation_id())
                    .ok_or_else(|| anyhow!("known Pending disappeared"))?;
                anyhow::ensure!(
                    known.retirements().len() == 1
                        && known.retirements()[0].finalized().is_none()
                        && known.retirements()[0].request().audit_ack().is_some(),
                    "fixture must retain exact audited P without F"
                );
            }
        }
        let final_id = expected_final_id;
        phases.enter("recovery.single_stops_factory");
        let capabilities = ProfileWriterCapabilities::from_recovery_report(
            &report,
            true,
            16,
            Duration::from_secs(10),
        )?;
        anyhow::ensure!(
            capabilities.is_stop_only() && capabilities.full().is_none(),
            "fenced writer unexpectedly has business admission"
        );
        let stops = capabilities.stops();
        phases.enter("stops.snapshot_before_pump");
        let before_pump = stops
            .recovery_status(intent.operation_id())
            .await?
            .ok_or_else(|| anyhow!("missing pre-pump metadata"))?;
        let before_pump_head = before_pump.head().clone();
        let initial_events = stops
            .pending_retirement_events(intent.operation_id())
            .await?;
        let preexisting_final = match scenario {
            CompletionScenario::FinalBeforeFactory => {
                anyhow::ensure!(
                    initial_events.len() == 1 && initial_events[0].event_id() == final_id,
                    "Final event was not fixed at F"
                );
                Some((
                    initial_events[0].input_hash().to_owned(),
                    canonical(initial_events[0].event())?,
                ))
            }
            CompletionScenario::PendingWithHeldFull3 => {
                anyhow::ensure!(
                    initial_events.is_empty()
                        && before_pump.retirements().len() == 1
                        && before_pump.retirements()[0].finalized().is_none(),
                    "service must start from P/Q, without a prefilled F/event"
                );
                None
            }
        };
        phases.enter("service.registry_and_material");
        let registry = configured_registry(
            &_root.path().canonicalize()?,
            &scope,
            &intent,
            &_store_root.path().canonicalize()?,
        )?;
        let profile = registry.profile(scope.profile_id())?.clone();
        let material = Arc::new(Material::load(&registry)?);
        let jobs = Arc::new(ProfileJobs::new());
        jobs.writer
            .set(capabilities)
            .map_err(|_| anyhow!("writer factory unexpectedly repeated"))?;
        jobs.job(intent.operation_id())?;
        jobs.job(other.operation_id())?;
        phases.enter("service.create_fenced_mode");
        let service = Arc::new(Service {
            registry,
            root: Directory::open_existing(&_root.path().canonicalize()?)?,
            material,
            profiles: BTreeMap::from([(scope.profile_id().to_owned(), Arc::clone(&jobs))]),
            enforcer: match scenario {
                CompletionScenario::FinalBeforeFactory => None,
                CompletionScenario::PendingWithHeldFull3 => Some(ServiceEnforcer::Native(
                    ExecutionEnforcerV1::Full3(full.clone()),
                )),
            },
            stop_enforcer: match scenario {
                CompletionScenario::FinalBeforeFactory => Some(native.stop_client().await?),
                CompletionScenario::PendingWithHeldFull3 => None,
            },
            blocking: Arc::new(Semaphore::new(2)),
            preparation: Arc::new(Semaphore::new(1)),
            execution: Arc::new(Semaphore::new(1)),
            stopping: Arc::new(AtomicBool::new(false)),
            fenced: AtomicBool::new(true),
        });
        anyhow::ensure!(
            service.require_starts().await.is_err(),
            "fixture is not actually fenced"
        );
        match scenario {
            CompletionScenario::FinalBeforeFactory => anyhow::ensure!(
                service.recovery_only()
                    && service.enforcer.is_none()
                    && service.full_enforcer().is_err(),
                "audit-only variant must remain literal RecoveryOnly without Full"
            ),
            CompletionScenario::PendingWithHeldFull3 => anyhow::ensure!(
                !service.recovery_only()
                    && service
                        .full_enforcer()?
                        .full3()
                        .is_some_and(|client| client.epoch() == full.epoch()),
                "live Q variant must use only the held Full3 epoch"
            ),
        }
        phases.enter("http.viewer_refusal");
        let forbidden = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()?
            .post(format!("{origin}/api/events"))
            .bearer_auth(VIEWER)
            .json(&viewer_probe_event)
            .send()
            .await?;
        anyhow::ensure!(
            forbidden.status() == reqwest::StatusCode::FORBIDDEN,
            "viewer must not deliver a runner event"
        );

        anyhow::ensure!(
            final_rows(&store)?.is_empty(),
            "final event was prefilled before the service pump"
        );
        if matches!(scenario, CompletionScenario::PendingWithHeldFull3) {
            anyhow::ensure!(
                full.has_local_slot(&arm.to_string())?,
                "original local slot was released before confirmed F"
            );
            // Per-instance seam: the first real authenticated QueryQ is
            // returned. The second reaches MAC/receipt verification, then its
            // genuine wrapper is dropped and Unavailable is returned once.
            full.lose_query_reply_after_successes_for_test(1)?;
        }
        // Call the actual productive Service method. First real HTTP commit
        // loses its reply; no Writer ACK may be fabricated from that failure.
        phases.enter("service.first_pump");
        Arc::clone(&service)
            .reconcile_retirement_audit_profile(profile.clone())
            .await?;
        if matches!(scenario, CompletionScenario::PendingWithHeldFull3) {
            anyhow::ensure!(
                full.has_local_slot(&arm.to_string())?,
                "lost post-F authenticated Q reply falsely released the local slot"
            );
        }
        phases.enter("stops.observe_first_pump");
        let after_lost = stops
            .pending_retirement_events(intent.operation_id())
            .await?;
        anyhow::ensure!(
            after_lost.len() == 1 && after_lost[0].event_id() == final_id,
            "real pump did not leave one exact final event pending after lost ACK"
        );
        let fixed_payload = canonical(after_lost[0].event())?;
        let final_input_hash = after_lost[0].input_hash().to_owned();
        if let Some((expected_hash, expected_payload)) = &preexisting_final {
            anyhow::ensure!(
                *expected_hash == final_input_hash && *expected_payload == fixed_payload,
                "lost ACK changed the existing final event"
            );
        }
        let after_final = stops
            .recovery_status(intent.operation_id())
            .await?
            .ok_or_else(|| anyhow!("pump omitted confirmed F"))?;
        let final_head = after_final.head().clone();
        let produced = after_final
            .retirements()
            .first()
            .and_then(|entry| entry.finalized())
            .ok_or_else(|| anyhow!("service did not complete genuine Q into F"))?;
        anyhow::ensure!(
            produced.receipt().digest()? == q_digest
                && produced.final_input_hash().as_str() == final_input_hash
                && after_final.stops().is_empty(),
            "pump F changed Q or manufactured a new active STOP actor"
        );
        let expected_revision = before_pump_head.revision()
            + u64::from(matches!(scenario, CompletionScenario::PendingWithHeldFull3));
        anyhow::ensure!(
            final_head.revision() == expected_revision,
            "service published more than the single promised F"
        );
        let first_rows = final_rows(&store)?;
        anyhow::ensure!(
            first_rows.len() == 1 && first_rows[0].event.id == final_id.to_string(),
            "gateway did not actually commit the first Final delivery"
        );
        let initial_sequence = first_rows[0].seq;
        anyhow::ensure!(
            canonical(&first_rows[0].event)? == fixed_payload,
            "gateway stored a different event"
        );
        phases.enter("service.second_pump");
        Arc::clone(&service)
            .reconcile_retirement_audit_profile(profile)
            .await?;
        if matches!(scenario, CompletionScenario::PendingWithHeldFull3) {
            anyhow::ensure!(
                !full.has_local_slot(&arm.to_string())?,
                "Final plus fresh authenticated Q did not finish local-slot removal"
            );
        }
        anyhow::ensure!(
            stops
                .pending_retirement_events(intent.operation_id())
                .await?
                .is_empty(),
            "real late HTTP ACK did not persist through Stops"
        );
        phases.enter("stops.observe_second_pump");
        let delivered = final_rows(&store)?;
        anyhow::ensure!(
            delivered.len() == 1 && delivered[0].seq == initial_sequence,
            "replay created a second logical event/sequence"
        );
        let snapshot = stops
            .recovery_status(intent.operation_id())
            .await?
            .ok_or_else(|| anyhow!("missing confirmed audit snapshot"))?;
        anyhow::ensure!(
            snapshot.incomplete() && snapshot.head().revision() == final_head.revision() + 1,
            "ACK must be durable while partial coverage stays explicit"
        );
        let final_after_ack = snapshot
            .retirements()
            .first()
            .and_then(|entry| entry.finalized())
            .ok_or_else(|| anyhow!("Final disappeared after late ACK"))?;
        anyhow::ensure!(
            final_after_ack.final_input_hash().as_str() == final_input_hash
                && final_after_ack.receipt().digest()? == q_digest,
            "late ACK mutated F input hash or Q receipt"
        );
        // Reopening the retained frame again would be a second actor factory.
        anyhow::ensure!(
            matches!(
                ProfileWriterCapabilities::from_recovery_report(
                    &report,
                    true,
                    16,
                    Duration::from_secs(10)
                ),
                Err(crate::broker::journal::JournalError::Busy)
            ),
            "pump replaced the single claimed factory"
        );
        let ack_head = snapshot.head().clone();
        phases.enter("service.third_pump");
        service.reconcile_retirement_audit().await;
        phases.enter("stops.observe_third_pump");
        let stable = stops
            .recovery_status(intent.operation_id())
            .await?
            .ok_or_else(|| anyhow!("confirmed Final disappeared"))?;
        anyhow::ensure!(
            stable.head() == &ack_head && service.require_starts().await.is_err(),
            "audit replay changed F again or lifted starts fence"
        );
        anyhow::ensure!(
            observed.runner_final_posts.load(Ordering::SeqCst) == 2
                && observed.runner_identity_requests.load(Ordering::SeqCst) >= 2,
            "pump did not use real runner HTTP identity/event paths"
        );
        let payloads = observed
            .final_payloads
            .lock()
            .map_err(|_| anyhow!("test observation unavailable"))?;
        anyhow::ensure!(
            payloads.len() == 2 && payloads[0] == payloads[1],
            "retry regenerated Final timestamp/content/identity"
        );
        drop(payloads);
        anyhow::ensure!(
            !business_blob.exists() && preserved_blob.is_file(),
            "pump depended on restoring the business credential blob"
        );
        phases.enter("control.verify_unchanged");
        anyhow::ensure!(
            worker_before
                == canonical(
                    &store
                        .worker(&worker_id)?
                        .ok_or_else(|| anyhow!("central worker disappeared"))?
                )?
                && quota_before == canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?
                && usage_before == canonical(&store.worker_usage()?)?,
            "audit completion changed central worker/quota/usage"
        );
        Ok(())
    }
    .await;
    let result = result.map_err(|error| phases.failure(error));
    phases.enter("cleanup.native_owned_tasks");
    native.finish().await;
    phases.enter("cleanup.http_owned_task");
    let gateway_result = gateway
        .finish()
        .await
        .context("d5-retirement-audit phase=cleanup.http_owned_task");
    if result.is_err() || gateway_result.is_err() {
        // A timed-out queued write can still be running. Preserve its real
        // files instead of removing the pathname during its later barriers.
        eprintln!(
            "private failed retirement journal retained: {}",
            _root.keep().display()
        );
        eprintln!(
            "private failed retirement store retained: {}",
            _store_root.keep().display()
        );
    }
    // Preserve the primary phase/type even if the owned HTTP cleanup also fails.
    result.unwrap();
    gateway_result.unwrap();
}
