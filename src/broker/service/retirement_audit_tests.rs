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

#[path = "retirement_recovery_tests.rs"]
mod retirement_recovery_tests;

// Public synthetic test credentials; no environment/keychain/provider material.
const RUNNER: &str = "runner-retirement-audit-aaaaaaaaaaaaaaaaaaaaaaaa";
const VIEWER: &str = "viewer-retirement-audit-bbbbbbbbbbbbbbbbbbbbbbb";

async fn audit_abandonment(
    writer: &WriterClient,
    client: &AuditClient,
    intent: &ExecutionIntentV1,
    retirement: Uuid,
    arm: Uuid,
    epoch: Uuid,
) -> Result<()> {
    let event = crate::broker::journal::retirement_live_fixture::abandonment_event_data(
        intent, retirement, arm, epoch, 0,
    );
    writer
        .enqueue_event(intent.operation_id(), event.clone())
        .await
        .context("fixture signal enqueue")?;
    let sequence = client
        .post_control_event(serde_json::to_value(&event)?)
        .await
        .context("fixture signal actual HTTP ACK")?;
    writer
        .acknowledge_event(intent.operation_id(), retirement, sequence)
        .await
        .context("fixture signal durable writer ACK")?;
    Ok(())
}

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
            if let Some(failure) =
                error.downcast_ref::<crate::broker::writer::RetirementFailure<$input>>()
            {
                let state = match failure {
                    crate::broker::writer::RetirementFailure::NotQueued { .. } => "NotQueued",
                    crate::broker::writer::RetirementFailure::Refused { .. } => "RefusedCold",
                    crate::broker::writer::RetirementFailure::Unknown { .. } => "Unknown",
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
    completion!(
        crate::broker::enforcer::retirement::VerifiedRetirementReceipt,
        "FinalCompletion"
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
                    if runner && request.uri().path() == "/control/api/v1/control/protocol" {
                        observed
                            .runner_identity_requests
                            .fetch_add(1, Ordering::SeqCst);
                    }
                    if request.method() != axum::http::Method::POST
                        || request.uri().path() != "/control/api/v1/control/events"
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
        let sequence = client
            .post_control_event(serde_json::to_value(signal)?)
            .await?;
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
            .post_control_event(serde_json::to_value(pending.intention_event())?)
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
            full_owners: FullOwners::new(registry.data.max_enforcer_slots as usize)?,
            cold_owners: retained_audit::ColdOwners::new(
                registry.data.max_enforcer_slots as usize,
            )?,
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
            fenced: Arc::new(AtomicBool::new(true)),
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
            .post(format!("{origin}/control/api/v1/control/events"))
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closed_retirement_registry_consumes_retained_final_without_re_admitting_a_stale_snapshot()
{
    retirement_reconcile_race(true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_flight_final_after_pending_snapshot_is_not_treated_as_an_absent_owner() {
    retirement_reconcile_race(false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closed_retirement_registry_via_real_stops_cold_actor_keeps_the_same_final_oracle() {
    retirement_reconcile_race(true, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_flight_final_via_real_stops_actor_is_not_treated_as_absent() {
    retirement_reconcile_race(false, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_final_consumed_before_lookup_does_not_re_admit_from_pending_snapshot() {
    retirement_reconcile_race_with_consumed_final(false, true, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn job_guarded_producer_after_pump_q_does_not_leave_a_second_job_retirement_owner() {
    retirement_reconcile_with_guarded_producer(
        false,
        true,
        true,
        true,
        true,
        FullOwnerBoundary::None,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_full_owner_guard_keeps_pump_q_out_while_final_audit_progresses() {
    retirement_reconcile_with_guarded_producer(
        false,
        true,
        false,
        false,
        false,
        FullOwnerBoundary::FinalBeforePump,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_full_owner_can_publish_its_first_final_during_real_shutdown() {
    retirement_reconcile_with_guarded_producer(
        false,
        true,
        false,
        false,
        false,
        FullOwnerBoundary::FirstFinalDuringShutdown,
    )
    .await;
}

#[derive(Clone, Copy)]
enum FullOwnerBoundary {
    None,
    FinalBeforePump,
    FirstFinalDuringShutdown,
}

async fn retirement_reconcile_race(registry_closed: bool, actor_consumer: bool) {
    retirement_reconcile_race_with_consumed_final(registry_closed, actor_consumer, false).await;
}

async fn retirement_reconcile_race_with_consumed_final(
    registry_closed: bool,
    actor_consumer: bool,
    consume_final_before_lookup: bool,
) {
    retirement_reconcile_race_at_boundary(
        registry_closed,
        actor_consumer,
        consume_final_before_lookup,
        false,
    )
    .await;
}

async fn retirement_reconcile_race_at_boundary(
    registry_closed: bool,
    actor_consumer: bool,
    consume_final_before_resume: bool,
    after_q: bool,
) {
    retirement_reconcile_with_guarded_producer(
        registry_closed,
        actor_consumer,
        consume_final_before_resume,
        after_q,
        false,
        FullOwnerBoundary::None,
    )
    .await;
}

async fn retirement_reconcile_with_guarded_producer(
    registry_closed: bool,
    actor_consumer: bool,
    consume_final_before_resume: bool,
    after_q: bool,
    guarded_controller: bool,
    full_owner_boundary: FullOwnerBoundary,
) {
    use crate::broker::journal::recovery::cold_audit;
    let phases = PhaseTrace::new();
    phases.enter("closed.fixture");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let crate::broker::journal::retirement_live_fixture::LiveFixture {
        _root,
        _store_root,
        store,
        journal,
        intent,
        head,
        registration,
    } = live_fixture_with_gateway(&origin, true);
    let store = Arc::new(store);
    let scope = journal.scope().clone();
    let gateway = OwnedGateway::start(
        listener,
        Arc::clone(&store),
        scope.tenant_id(),
        Arc::new(GatewayObservations::default()),
    )
    .await
    .unwrap();
    let (native, full, slot) =
        OriginalUnusedFixture::register(scope.installation_id(), registration)
            .await
            .unwrap();
    // Retain the actual transfer/lease even if the awaiting operation fails.
    // It outlives native/gateway cleanup and is never flattened into an error.
    let mut handoff_guard: Option<crate::broker::writer::StopsHandoffForTest> = None;
    let mut producer_input: Option<WriterClient> = None;
    let mut verified_guard: Option<crate::broker::enforcer::retirement::VerifiedRetirementReceipt> =
        None;
    let outcome: Result<(bool, bool, bool, bool, Arc<Service>)> = async {
        phases.enter("closed.writer.start");
        let operation = intent.operation_id();
        producer_input = Some(WriterClient::start(
            journal.clone(),
            16,
            Duration::from_secs(10),
        )?);
        let writer = producer_input
            .as_ref()
            .ok_or_else(|| anyhow!("actual producer client missing"))?;
        let binding = slot.full3_registration_binding()?;
        let arm = binding.arm_id();
        phases.enter("closed.writer.bind");
        writer
            .bind_full3_registration(operation, head, binding)
            .await?;
        let retirement = Uuid::new_v4();
        let client = AuditClient::new(&origin, RUNNER)?;
        phases.enter("closed.signal.enqueue_http_ack");
        audit_abandonment(&writer, &client, &intent, retirement, arm, full.epoch()).await?;
        phases.enter("closed.writer.load");
        let current = writer
            .load(operation)
            .await?
            .ok_or_else(|| anyhow!("actual accepted job missing"))?;
        phases.enter("closed.native.revoke");
        let unused = slot.revoke_unused(retirement).await?;
        phases.enter("closed.writer.begin");
        let pending = writer
            .begin_unused_retirement(BeginUnusedRetirement::new(
                operation,
                current.head().clone(),
                unused,
            )?)
            .await?;
        phases.enter("closed.pending.http");
        let sequence = client
            .post_control_event(serde_json::to_value(pending.intention_event())?)
            .await?;
        phases.enter("closed.pending.ack");
        writer
            .acknowledge_event(operation, pending.intention_event_id(), sequence)
            .await?;
        phases.enter("closed.writer.authorize");
        let authorized = writer.authorize_unused_retirement(pending).await?;
        phases.enter("closed.native.retire_join_q");
        verified_guard = Some(full.retire_unused(authorized).await?);
        phases.enter("closed.pair.enqueue");
        let mut pair = Vec::new();
        for kind in ["observer.coverage", "broker.batch.result"] {
            let event = crate::broker::journal::retirement::event(
                &intent,
                Uuid::new_v4(),
                kind,
                json!({"fixture":"metadata scope only"}),
            );
            writer.enqueue_event(operation, event.clone()).await?;
            pair.push(event);
        }
        let (capabilities, direct_journal) = if actor_consumer {
            phases.enter("actor.full_writer.handoff_prepare");
            handoff_guard = Some(WriterClient::prepare_stops_handoff_for_test(
                &mut producer_input,
            )?);
            anyhow::ensure!(
                producer_input.is_none(),
                "the original Full client was not consumed"
            );
            // The handoff already retains the exact original core/lease.
            // No raw Full journal is used or kept for a mutation after this.
            drop(journal);
            phases.enter("actor.full_writer.join_before_stops");
            let capabilities = handoff_guard
                .as_ref()
                .ok_or_else(|| anyhow!("retained handoff missing"))?
                .joined_stops()
                .await?;
            anyhow::ensure!(
                capabilities.is_stop_only()
                    && capabilities.full().is_none()
                    && capabilities.cold_audit().is_some()
                    && canonical(capabilities.stops().scope())? == canonical(&scope)?,
                "handoff did not admit the real Stops-only consumer"
            );
            phases.enter("actor.stops.started_after_real_join");
            (capabilities, None)
        } else {
            let direct = crate::broker::journal::StopJournal::from_full(&journal);
            (
                ProfileWriterCapabilities::from_full(
                    producer_input
                        .take()
                        .ok_or_else(|| anyhow!("original Full client missing"))?,
                ),
                Some(direct),
            )
        };
        let stops = capabilities.stops();
        if registry_closed {
            phases.enter("closed.cold.prepare");
            let mut direct_prepared = None;
            let mut reserved = None;
            let cold = capabilities.cold_audit();
            if actor_consumer {
                reserved = Some(
                    cold.as_ref()
                        .ok_or_else(|| anyhow!("real cold facade missing"))?
                        .prepare(operation)
                        .await?,
                );
            } else {
                direct_prepared = Some(cold_audit::prepare(
                    direct_journal
                        .as_ref()
                        .ok_or_else(|| anyhow!("direct fixture lease missing"))?,
                    operation,
                )?);
            }
            phases.enter("closed.final.refused");
            match stops
                .finalize_retirement(
                    verified_guard
                        .take()
                        .ok_or_else(|| anyhow!("actual Verified missing"))?,
                )
                .await
            {
                Err(crate::broker::writer::RetirementFailure::Refused {
                    error: crate::broker::journal::JournalError::Busy,
                    continuation,
                }) => drop(continuation),
                other => return Err(anyhow!("actual Cold-refused Final not reached: {other:?}")),
            }
            phases.enter("closed.registry.close");
            stops.close_retirement_owners()?;
            let budget = crate::runner::ColdAuditBudget::start();
            let mut receipts = Vec::new();
            phases.enter("closed.cold.http");
            for event in &pair {
                receipts.push(client.post_cold_audit_event(event, &budget).await?);
            }
            phases.enter("closed.cold.complete");
            if actor_consumer {
                let reserved = reserved
                    .take()
                    .ok_or_else(|| anyhow!("retained reservation missing"))?;
                let before_head = reserved.prepared().head().clone();
                phases.enter("actor.cold.submit.before");
                let waiter = reserved.submit(receipts)?;
                phases.enter("actor.cold.submit.after");
                phases.enter("actor.cold.confirmed.before");
                let confirmed = waiter.confirmed().await?;
                phases.enter("actor.cold.confirmed.after");
                let expected: BTreeSet<_> = pair
                    .iter()
                    .map(|event| Uuid::parse_str(&event.id))
                    .collect::<Result<_, _>>()?;
                let actual: BTreeSet<_> = confirmed.event_ids().iter().copied().collect();
                anyhow::ensure!(
                    actual == expected
                        && actual.len() == 2
                        && confirmed.operation_id() == operation
                        && confirmed.head().revision() == before_head.revision() + 1,
                    "actual actor did not confirm the same exact Cold ACK batch/head"
                );
                let metadata = stops
                    .recovery_status(operation)
                    .await?
                    .ok_or_else(|| anyhow!("actual post-ACK snapshot missing"))?;
                anyhow::ensure!(
                    metadata.head() == confirmed.head(),
                    "actor confirmation does not match durable current metadata"
                );
                // None here means no quarantine only. The ACK proof above is
                // the actual confirmed return plus durable exact current head.
                anyhow::ensure!(
                    cold.as_ref()
                        .unwrap()
                        .recover_quarantined(operation)
                        .await?
                        .is_none(),
                    "successful actor ACK unexpectedly left an ambiguous lot"
                );
            } else {
                let prepared = direct_prepared
                    .take()
                    .ok_or_else(|| anyhow!("direct preparation missing"))?;
                let journal = direct_journal
                    .as_ref()
                    .ok_or_else(|| anyhow!("direct lease missing"))?;
                cold_audit::completion::complete(journal, &prepared, &receipts)?;
                cold_audit::completion::confirm(journal, &prepared, &receipts)?;
                drop(prepared);
            }
        }
        phases.enter("closed.snapshot.precondition");
        let before = stops
            .recovery_status(operation)
            .await?
            .ok_or_else(|| anyhow!("actual Pending missing"))?;
        anyhow::ensure!(
            before.retirements().len() == 1 && before.retirements()[0].finalized().is_none(),
            "baseline must still be P/Q before the pump"
        );
        phases.enter("closed.service.configuration");
        let registry = configured_registry(
            &_root.path().canonicalize()?,
            &scope,
            &intent,
            &_store_root.path().canonicalize()?,
        )?;
        let profile = registry.profile(scope.profile_id())?.clone();
        let material = Arc::new(Material::load(&registry)?);
        let jobs = Arc::new(ProfileJobs::new());
        let test_writer = capabilities.full();
        jobs.writer
            .set(capabilities)
            .map_err(|_| anyhow!("single writer factory"))?;
        let job = jobs.job(operation)?;
        let service = Arc::new(Service {
            full_owners: FullOwners::new(registry.data.max_enforcer_slots as usize)?,
            cold_owners: retained_audit::ColdOwners::new(
                registry.data.max_enforcer_slots as usize,
            )?,
            registry,
            root: Directory::open_existing(&_root.path().canonicalize()?)?,
            material,
            profiles: BTreeMap::from([(scope.profile_id().to_owned(), Arc::clone(&jobs))]),
            enforcer: Some(ServiceEnforcer::Native(ExecutionEnforcerV1::Full3(
                full.clone(),
            ))),
            stop_enforcer: None,
            blocking: Arc::new(Semaphore::new(2)),
            preparation: Arc::new(Semaphore::new(1)),
            execution: Arc::new(Semaphore::new(1)),
            stopping: Arc::new(AtomicBool::new(!matches!(
                full_owner_boundary,
                FullOwnerBoundary::FirstFinalDuringShutdown
            ))),
            fenced: Arc::new(AtomicBool::new(true)),
        });
        // This is a real current credential/protocol check, not an assumed
        // profile readiness. Starts remain closed throughout the observation.
        phases.enter("closed.control.auth_protocol");
        VerifiedProfile::authenticate(&service.registry, &profile.id, RUNNER, false).await?;
        anyhow::ensure!(
            service.require_starts().await.is_err(),
            "no new admission during shutdown"
        );
        let mut consumed_final = None;
        phases.enter("closed.service.real_pump");
        if matches!(full_owner_boundary, FullOwnerBoundary::FirstFinalDuringShutdown) {
            full_owner_first_final_during_shutdown(
                &service, &job, &stops, &full, operation, arm, &mut verified_guard,
            ).await?;
        } else if matches!(full_owner_boundary, FullOwnerBoundary::FinalBeforePump) {
            full_owner_held_while_pump_audits(
                &service,
                profile,
                &jobs,
                &job,
                &store,
                &stops,
                &full,
                operation,
                arm,
                &mut verified_guard,
            )
            .await?;
        } else if registry_closed {
            Arc::clone(&service)
                .reconcile_retirement_audit_profile(profile)
                .await?;
        } else {
            let (entered, resume, classified) = if after_q {
                jobs.gate_retirement_after_q_for_test()
            } else {
                jobs.gate_retirement_snapshot_for_test()
            };
            let pump = Arc::clone(&service).reconcile_retirement_audit_profile(profile.clone());
            let control = async {
                tokio::time::timeout(Duration::from_secs(30), entered)
                    .await
                    .context(if after_q {
                        "actual authenticated Q before Final admission barrier"
                    } else {
                        "actual Pending snapshot barrier"
                    })??;
                let mut receive = None;
                anyhow::ensure!(
                    Arc::ptr_eq(
                        &job,
                        &jobs.known(operation).ok_or_else(|| anyhow!("known Job missing"))?
                    ),
                    "controller and pump must share the original Job Arc"
                );
                let (controller_running, controller_busy) = if guarded_controller {
                    match job.enter() {
                        Ok(running) => {
                            phases.enter("actor.controller.real_running_job_entered");
                            (Some(running), false)
                        }
                        Err(BrokerErrorCodeV1::Busy) => {
                            phases.enter("actor.controller.real_running_job_busy");
                            (None, true)
                        }
                        Err(code) => return Err(anyhow!("controller Job entry failed: {code:?}")),
                    }
                } else {
                    (None, false)
                };
                let paused = if controller_busy {
                    // The future pump owns the real same-Job metadata permit.
                    // The controller must perform no Final/close operation.
                    let unchanged = stops
                        .recovery_status(operation)
                        .await?
                        .ok_or_else(|| anyhow!("Busy controller metadata missing"))?;
                    anyhow::ensure!(
                        verified_guard.is_some()
                            && unchanged.head() == before.head()
                            && unchanged.retirements().len() == 1
                            && unchanged.retirements()[0].finalized().is_none()
                            && !stops.has_retained_retirement()
                            && full.has_local_slot(&arm.to_string())?,
                        "Busy controller must preserve its original input and all facts"
                    );
                    None
                } else if consume_final_before_resume {
                    phases.enter(if after_q {
                        "actor.final.consume.after_pump_q"
                    } else {
                        "actor.final.consume.before_lookup"
                    });
                    let before = stops
                        .recovery_status(operation)
                        .await?
                        .ok_or_else(|| anyhow!("actual pre-Final snapshot missing"))?;
                    if after_q {
                        phases.enter("actor.pump.real_q_held_before_final_admission");
                        anyhow::ensure!(
                            before.retirements().iter().any(|entry| {
                                *entry.request().identity().retirement_id() == retirement
                                    && entry.finalized().is_none()
                            }) && !stops.has_retained_retirement(),
                            "the after-Q arrangement must still have genuine Pending and no Final owner"
                        );
                    }
                    let before = before.head().clone();
                    // This is the same production successful caller path as
                    // the executor: await Final, consume its retained outcome,
                    // then pause before the separate Q/local-slot removal.
                    let receipt = stops
                        .finalize_retirement(
                            verified_guard
                                .take()
                                .ok_or_else(|| anyhow!("original Verified missing"))?,
                        )
                        .await?;
                    let durable = stops
                        .recovery_status(operation)
                        .await?
                        .ok_or_else(|| anyhow!("actual committed Final snapshot missing"))?;
                    let final_entry = durable
                        .retirements()
                        .iter()
                        .find(|entry| *entry.request().identity().retirement_id() == retirement)
                        .and_then(|entry| entry.finalized())
                        .ok_or_else(|| anyhow!("actual durable Final entry missing"))?;
                    anyhow::ensure!(
                        receipt.operation_id() == operation
                            && durable.head() == receipt.head()
                            && receipt.head().revision() == before.revision() + 1
                            && matches!(
                                stops.recover_final_retirement_state(operation, arm, retirement)?,
                                crate::broker::writer::FinalRetirementRecovery::Absent
                            )
                            && !stops.has_retained_retirement()
                            && full.has_local_slot(&arm.to_string())?,
                        "Final was not genuinely durable and consumed before lookup"
                    );
                    consumed_final = Some((receipt.head().clone(), canonical(final_entry)?));
                    // Exercise an already closed retirement registry after the
                    // Final was consumed; lookup must not force a second Final.
                    stops.close_retirement_owners()?;
                    phases.enter("actor.final.consumed_registry_absent_closed");
                    None
                } else {
                    let paused = if actor_consumer {
                        stops.pause_for_test().await?
                    } else {
                        test_writer
                            .as_ref()
                            .ok_or_else(|| {
                                anyhow!("in-flight fixture requires its original Full writer")
                            })?
                            .pause_for_test()
                            .await?
                    };
                    anyhow::ensure!(paused.is_paused(), "actual writer did not pause");
                    receive = Some(
                        stops.enqueue_final_for_test(
                            verified_guard
                                .take()
                                .ok_or_else(|| anyhow!("original Verified missing"))?,
                        )?,
                    );
                    Some(paused)
                };
                // The original Final is either really queued or already
                // committed/consumed. Neither setup invents a second input.
                let _ = resume.send(());
                tokio::time::timeout(Duration::from_secs(30), classified)
                    .await
                    .context("actual recovery classification barrier")??;
                if let Some(paused) = paused.as_ref() {
                    anyhow::ensure!(
                        paused.is_paused(),
                        "the actor pause expired before recovery classification completed"
                    );
                }
                drop(paused);
                drop(receive);
                drop(controller_running);
                Ok::<(), anyhow::Error>(())
            };
            let (pumped, controlled) = tokio::join!(pump, control);
            controlled?;
            pumped?;
            Arc::clone(&service)
                .reconcile_retirement_audit_profile(profile)
                .await?;
        }
        phases.enter("closed.snapshot.after_pump");
        let after = stops
            .recovery_status(operation)
            .await?
            .ok_or_else(|| anyhow!("post-pump metadata missing"))?;
        if let Some((head, final_bytes)) = consumed_final {
            let final_entry = after
                .retirements()
                .iter()
                .find(|entry| *entry.request().identity().retirement_id() == retirement)
                .and_then(|entry| entry.finalized())
                .ok_or_else(|| anyhow!("consumed Final disappeared during reconciliation"))?;
            anyhow::ensure!(
                after.head().revision() >= head.revision()
                    && (after.head().revision() != head.revision() || after.head() == &head)
                    && canonical(final_entry)? == final_bytes,
                "reconciliation changed the already consumed Final"
            );
        }
        let finalized = after
            .retirements()
            .first()
            .is_some_and(|entry| entry.finalized().is_some());
        let slot_retained = full.has_local_slot(&arm.to_string())?;
        let job_retained = job.preclaim.has_retained_retirement();
        let registry_retained = stops.has_retained_retirement();
        Ok((
            finalized,
            slot_retained,
            job_retained,
            registry_retained,
            service,
        ))
    }
    .await
    .map_err(|error| phases.failure(error));
    native.finish().await;
    let gateway_result = gateway.finish().await;
    let expected_failure = match &outcome {
        Ok((finalized, slot, job, registry, _)) => {
            (*finalized, *slot, *job, *registry) != (true, false, false, false)
        }
        Err(_) => true,
    };
    if expected_failure || gateway_result.is_err() {
        eprintln!(
            "private failed actor-consumer journal retained: {}",
            _root.keep().display()
        );
        eprintln!(
            "private failed actor-consumer store retained: {}",
            _store_root.keep().display()
        );
    }
    let (finalized, slot_retained, job_retained, registry_retained, _service_guard) =
        outcome.unwrap();
    gateway_result.unwrap();
    // Keep the actual Service/backend owner until after real fixture cleanup.
    // Its intentional test teardown is not claimed as a successful drain.
    assert_eq!(
        (finalized, slot_retained, job_retained, registry_retained),
        (true, false, false, false),
        "confirmed Final must not re-admit a new wrapper from the pre-retry Pending snapshot"
    );
}

// The accepted Full future already owns the genuine Q before shutdown. Its
// first Final must finish before shutdown closes the D5 writer owners. No
// business/start capability is obtained from the Stops-only fixture backend.
async fn full_owner_first_final_during_shutdown(
    service: &Arc<Service>,
    job: &Arc<Job>,
    stops: &StopWriterClient,
    full: &EnforcerFullClientV3,
    operation: Uuid,
    arm: Uuid,
    original: &mut Option<crate::broker::enforcer::retirement::VerifiedRetirementReceipt>,
) -> Result<()> {
    let before = stops
        .recovery_status(operation)
        .await?
        .ok_or_else(|| anyhow!("pre-shutdown Pending missing"))?;
    anyhow::ensure!(
        before.retirements().len() == 1
            && before.retirements()[0].finalized().is_none()
            && !stops.has_retained_retirement(),
        "first Final must not be admitted before the shutdown boundary"
    );
    let verified = original
        .take()
        .ok_or_else(|| anyhow!("original Verified missing"))?;
    let identity = verified.identity().clone();
    let expected_q = verified.receipt().digest()?;
    anyhow::ensure!(
        !service.stopping.load(Ordering::Acquire),
        "Full producer must be accepted before shutdown starts"
    );
    let slot = service.full_owners.try_reserve()?;
    // A token reserved before close also cannot launch a new producer later.
    let late_slot = service.full_owners.try_reserve()?;
    let running = job.enter()?;
    let producer_stops = stops.clone();
    let producer_full = full.clone();
    let (entered, observed) = oneshot::channel();
    let (release, wait) = oneshot::channel();
    let (returned_input, observe_input) = oneshot::channel();
    slot.start(async move {
        let result = async {
            let _ = entered.send(());
            wait.await.map_err(|_| BrokerErrorCodeV1::Unavailable)?;
            producer_stops
                .finalize_retirement(verified)
                .await
                .map_err(|failure| jobs::journal_error(failure.error()))?;
            // This is the existing authenticated replay of Q solely for local
            // removal after F, never another retire_unused or new guardian Q.
            let replay = producer_full
                .query_retirement(&identity)
                .await
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
            if replay.receipt().digest().map_err(jobs::journal_error)? != expected_q {
                return Err(BrokerErrorCodeV1::NeedsReconciliation);
            }
            producer_full
                .retire_local_slot(&replay)
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
            // Retain this same authenticated replay for the post-drain
            // admission check, without another Q lookup or new capability.
            let _ = returned_input.send(replay);
            Ok(())
        }
        .await;
        drop(running);
        result
    })?;
    tokio::time::timeout(Duration::from_secs(30), observed).await??;
    let reconciler = tokio::spawn(std::future::pending());
    let (presence, _) = watch::channel(false);
    let mut presence_tasks = JoinSet::new();
    let mut requests = JoinSet::new();
    let shutdown = service.shutdown(reconciler, &presence, &mut presence_tasks, &mut requests);
    let continue_accepted = async {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if service.stopping.load(Ordering::Acquire)
                    && service.full_owners.try_reserve().is_err()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;
        let late_ran = Arc::new(AtomicBool::new(false));
        let late_observed = Arc::clone(&late_ran);
        let late_start = late_slot.start(async move {
            late_observed.store(true, Ordering::Release);
            Ok(())
        });
        let admissions_closed = service.full_owners.try_reserve().is_err()
            && service.cold_owners.try_reserve().is_err()
            && late_start.is_err()
            && !late_ran.load(Ordering::Acquire)
            && service.require_starts().await.is_err();
        let _ = release.send(());
        anyhow::ensure!(
            admissions_closed,
            "shutdown admitted a new Full/Cold producer"
        );
        Ok::<_, anyhow::Error>(())
    };
    let (drained, continued) = tokio::join!(shutdown, continue_accepted);
    continued?;
    // Gather the real post-drain state before reporting the RED oracle.
    let after = stops
        .recovery_status(operation)
        .await?
        .ok_or_else(|| anyhow!("post-shutdown metadata missing"))?;
    let finished = after.retirements()[0].finalized().is_some();
    let local_retained = full.has_local_slot(&arm.to_string())?;
    anyhow::ensure!(
        drained.is_ok() && finished && !local_retained && !stops.has_retained_retirement(),
        "accepted Full first Final was stranded by shutdown: drained={} final={} local_retained={}",
        drained.is_ok(),
        finished,
        local_retained,
    );
    let replay = observe_input.await?;
    match stops.finalize_retirement(replay).await {
        Err(crate::broker::writer::RetirementFailure::NotQueued { error, input })
            if error == crate::broker::journal::JournalError::Io =>
        {
            anyhow::ensure!(
                canonical(input.identity())?
                    == canonical(before.retirements()[0].request().identity())?,
                "post-shutdown refusal changed the original verified input"
            );
        }
        _ => {
            return Err(anyhow!(
                "shutdown left D5 admission open after its Full drain"
            ));
        }
    }
    anyhow::ensure!(
        after.head().revision() == before.head().revision() + 1,
        "shutdown republished Final or changed the existing Pending head"
    );
    let final_events = stops.pending_retirement_events(operation).await?;
    anyhow::ensure!(
        final_events.len() == 1
            && final_events[0].event().kind == "broker.retirement.final"
            && final_events[0].event_id() == before.retirements()[0].request().final_event_id()
            && after.stops().is_empty(),
        "shutdown lost the fixed pending Final audit or recreated a STOP actor"
    );
    Ok(())
}

// This is the real Full owner registry + real RunningJob + native Q/Final,
// not a BatchExecutor/runtime test. DeferredRun's capture is a separate canary.
async fn full_owner_held_while_pump_audits(
    service: &Arc<Service>,
    profile: ControlProfileV1,
    jobs: &Arc<ProfileJobs>,
    job: &Arc<Job>,
    store: &Store,
    stops: &StopWriterClient,
    full: &EnforcerFullClientV3,
    operation: Uuid,
    arm: Uuid,
    original: &mut Option<crate::broker::enforcer::retirement::VerifiedRetirementReceipt>,
) -> Result<()> {
    anyhow::ensure!(
        final_rows(store)?.is_empty(),
        "fresh fixture has no Final audit"
    );
    let slot = service.full_owners.try_reserve()?;
    let running = job.enter()?;
    let verified = original
        .take()
        .ok_or_else(|| anyhow!("original Verified missing"))?;
    let identity = verified.identity().clone();
    let expected_q = verified.receipt().digest()?;
    let producer_stops = stops.clone();
    let producer_full = full.clone();
    let (entered, observed) = oneshot::channel();
    let (release, wait) = oneshot::channel();
    slot.start(async move {
        let result: std::result::Result<(), BrokerErrorCodeV1> = async {
            producer_stops
                .finalize_retirement(verified)
                .await
                .map_err(|failure| jobs::journal_error(failure.error()))?;
            // The actual registered Full future still owns RunningJob while
            // its completed Final awaits the following Q/local-slot cleanup.
            let _ = entered.send(());
            wait.await.map_err(|_| BrokerErrorCodeV1::Unavailable)?;
            let replay = producer_full
                .query_retirement(&identity)
                .await
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
                .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
            if replay.receipt().digest().map_err(jobs::journal_error)? != expected_q {
                return Err(BrokerErrorCodeV1::NeedsReconciliation);
            }
            producer_full
                .retire_local_slot(&replay)
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
            Ok(())
        }
        .await;
        drop(running);
        result
    })?;

    // Both release senders outlive the fallible evidence collection. Any
    // failed precondition releases the owners before the test reports it.
    let mut owner_release = Some(release);
    let mut q_release = None;
    let result: Result<(bool, bool, bool, bool)> = async {
        tokio::time::timeout(Duration::from_secs(30), observed)
            .await
            .context("real Full owner completed its actual Final")??;
        anyhow::ensure!(
            full.has_local_slot(&arm.to_string())?
                && matches!(job.enter(), Err(BrokerErrorCodeV1::Busy))
                && !stops.has_retained_retirement(),
            "registered Full owner must hold the real same-Job guard and slot"
        );
        let pending = stops.pending_retirement_events(operation).await?;
        anyhow::ensure!(
            pending.len() == 1 && pending[0].event().kind == "broker.retirement.final",
            "actual Final audit must be pending before the pump"
        );
        let final_id = pending[0].event_id().to_string();
        let final_bytes = canonical(pending[0].event())?;
        let (mut q_entered, resume, _classified) = jobs.gate_retirement_before_q_for_test();
        q_release = Some(resume);
        let pump = Arc::clone(service).reconcile_retirement_audit_profile(profile);
        tokio::pin!(pump);
        let q_attempted = tokio::time::timeout(Duration::from_secs(30), async {
            let attempted = tokio::select! {
                entered = &mut q_entered => {
                    entered.context("actual pre-Q hook")?;
                    if let Some(resume) = q_release.take() {
                        let _ = resume.send(());
                    }
                    true
                },
                completed = &mut pump => {
                    completed?;
                    false
                },
            };
            if attempted {
                pump.await?;
            }
            Ok::<bool, anyhow::Error>(attempted)
        })
        .await
        .context("real pump Q-skip/audit watchdog")??;
        let slot_held = full.has_local_slot(&arm.to_string())?;
        let guard_held = matches!(job.enter(), Err(BrokerErrorCodeV1::Busy));
        let rows = final_rows(store)?;
        let audited = rows.len() == 1
            && rows[0].seq > 0
            && rows[0].event.id == final_id
            && canonical(&rows[0].event)? == final_bytes
            && stops.pending_retirement_events(operation).await?.is_empty();
        Ok((q_attempted, slot_held, guard_held, audited))
    }
    .await;
    if let Some(resume) = q_release.take() {
        let _ = resume.send(());
    }
    if let Some(release) = owner_release.take() {
        let _ = release.send(());
    }
    let drained = service.full_owners.drain().await;
    let evidence = result?;
    drained?;
    // Assert after all Full work has joined. Baseline may have attempted Q
    // and removed the slot early, but it must still receive real cleanup.
    anyhow::ensure!(
        evidence == (false, true, true, true),
        "Full-held pump evidence (Q attempted, slot held, guard held, audited): {evidence:?}"
    );
    Ok(())
}

#[derive(Clone, Copy)]
enum StartupExercise {
    Retain,
    Confirm,
    Service,
    RejectRollback,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_actual_report_and_verified_survive_lost_fs_notification_and_postcommit_error() {
    for (after_send, fail_before_ack) in [(false, false), (true, false), (false, true)] {
        startup_actual_report_case(after_send, fail_before_ack, StartupExercise::Retain).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_same_owner_confirms_exact_final_after_postcommit_ack_error() {
    startup_actual_report_case(false, true, StartupExercise::Confirm).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_service_pump_resumes_same_owner_after_persistent_io_and_during_close() {
    startup_actual_report_case(false, true, StartupExercise::Service).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_same_owner_rejects_rollback_without_republishing_final_or_reopening_after_repair()
{
    startup_actual_report_case(false, true, StartupExercise::RejectRollback).await;
}

async fn startup_actual_report_case(
    after_send: bool,
    fail_before_ack: bool,
    exercise: StartupExercise,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
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
    let gateway = OwnedGateway::start(
        listener,
        Arc::clone(&store),
        scope.tenant_id(),
        Arc::new(GatewayObservations::default()),
    )
    .await
    .unwrap();
    let (native, full, slot) =
        OriginalUnusedFixture::register(scope.installation_id(), registration)
            .await
            .unwrap();
    let outcome: Result<()> = async {
        let operation = intent.operation_id();
        let writer = WriterClient::start(journal.clone(), 16, Duration::from_secs(10))?;
        writer
            .bind_full3_registration(operation, head, slot.full3_registration_binding()?)
            .await?;
        let retirement = Uuid::new_v4();
        let client = AuditClient::new(&origin, RUNNER)?;
        audit_abandonment(
            &writer,
            &client,
            &intent,
            retirement,
            slot.full3_registration_binding()?.arm_id(),
            full.epoch(),
        )
        .await?;
        let expected = writer.load(operation).await?.unwrap().head().clone();
        let pending = writer
            .begin_unused_retirement(BeginUnusedRetirement::new(
                operation,
                expected,
                slot.revoke_unused(retirement).await?,
            )?)
            .await?;
        let seq = client
            .post_control_event(serde_json::to_value(pending.intention_event())?)
            .await?;
        writer
            .acknowledge_event(operation, pending.intention_event_id(), seq)
            .await?;
        let verified = full
            .retire_unused(writer.authorize_unused_retirement(pending).await?)
            .await?;
        let q_hash = verified.receipt().digest()?.as_str().to_owned();
        drop(writer);
        drop(journal);
        let journal_path = _root
            .path()
            .canonicalize()?
            .join("journals")
            .join(scope.profile_id());
        let journal_root = Directory::open_existing(&journal_path)?;
        let report = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match StopRecoveryReaderV1::open_existing(
                    &journal_root,
                    scope.clone(),
                    &FsExclusions::new(),
                    JournalLimitsV1::new(256 * 1024 * 1024, 256, 256, 512 * 1024 * 1024, 256, 128)?,
                    ControlSecrets::new(Vec::new())?,
                ) {
                    Ok(Some(reader)) => break Ok::<_, anyhow::Error>(reader.scan()),
                    Err(crate::broker::journal::JournalError::Busy) => {
                        tokio::task::yield_now().await
                    }
                    _ => break Err(anyhow!("actual startup reader not available")),
                }
            }
        })
        .await??;
        let before = report
            .jobs()
            .iter()
            .find(|job| job.operation_id() == operation)
            .ok_or_else(|| anyhow!("retained Pending job missing"))?
            .head()
            .clone();
        let startup_jobs = Arc::new(ProfileJobs::new());
        let retained = &startup_jobs.retirement_startup;
        let owner = retained.install(report)?;
        let identity = owner
            .next_query()?
            .ok_or_else(|| anyhow!("real Pending identity missing"))?;
        anyhow::ensure!(
            canonical(&identity)? == canonical(verified.identity())?,
            "startup Q scope mismatch"
        );
        owner
            .retain_verified(verified)
            .map_err(|_| anyhow!("startup did not retain the real verified receipt"))?;
        if matches!(exercise, StartupExercise::Service) {
            return Box::pin(startup_service_reconcile_case(
                &_root.path().canonicalize()?,
                &_store_root.path().canonicalize()?,
                &scope,
                &intent,
                &store,
                &full,
                startup_jobs,
                owner,
                before.revision(),
                &q_hash,
            )).await;
        }
        let (notification, waiter) = oneshot::channel();
        let (entered, observed) = oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let (finished, done) = oneshot::channel();
        let dispatched = Arc::clone(&owner);
        let task = tokio::task::spawn_blocking(move || {
            if fail_before_ack {
                crate::broker::journal::fail_next_cold_ack_for_test();
            }
            let result = dispatched.process();
            let sent = if after_send {
                let sent = notification.send(()).is_ok();
                let _ = entered.send(Some(sent));
                let _ = resume.recv_timeout(Duration::from_secs(30));
                sent
            } else {
                let _ = entered.send(None);
                let _ = resume.recv_timeout(Duration::from_secs(30));
                notification.send(()).is_ok()
            };
            let _ = finished.send((result, sent));
        });
        let boundary = tokio::time::timeout(Duration::from_secs(30), observed).await??;
        anyhow::ensure!(
            boundary == if after_send { Some(true) } else { None },
            "actual startup notification boundary missing"
        );
        drop(waiter);
        drop(task); // Losing the FS waiter is not aborting or joining IO.
        let _ = release.send(());
        let (processed, sent) = tokio::time::timeout(Duration::from_secs(30), done).await??;
        processed?;
        anyhow::ensure!(
            sent == after_send
                && retained.has_retained()
                && owner.retained_receipt_for_test().as_deref() == Some(q_hash.as_str()),
            "lost notification abandoned or changed Report/Verified"
        );
        let namespace_hash =
            crate::broker::journal::digest("ai-guard:broker-journal-scope:v1", &canonical(&scope)?);
        let current: Value = serde_json::from_slice(&fs::read(
            journal_path
                .join("broker-journal-v1")
                .join(&namespace_hash)
                .join("jobs")
                .join(operation.to_string())
                .join("current.json"),
        )?)?;
        anyhow::ensure!(
            current["head"]["revision"].as_u64() == Some(before.revision() + 1),
            "the failure test did not follow an actual Final publication"
        );
        if fail_before_ack {
            anyhow::ensure!(
                owner.confirm_step().is_err()
                    && owner.completed_report().is_err()
                    && owner.next_query().is_err(),
                "ambiguous mutation opened another Q/factory or normal drain"
            );
            anyhow::ensure!(
                owner.retained_receipt_for_test().as_deref() == Some(q_hash.as_str()),
                "postcommit error dropped original Verified"
            );
            if matches!(exercise, StartupExercise::RejectRollback) {
                let current_path = journal_path
                    .join("broker-journal-v1")
                    .join(&namespace_hash)
                    .join("jobs")
                    .join(operation.to_string())
                    .join("current.json");
                let committed = fs::read(&current_path)?;
                let mut rolled_back = current.clone();
                rolled_back["head"] = serde_json::to_value(&before)?;
                let rolled_back = serde_json::to_vec(&rolled_back)?;
                fs::write(&current_path, &rolled_back)?;
                let processed = owner.process();
                let closed = owner.confirm_step().is_err()
                    && owner.completed_report().is_err()
                    && owner.next_query().is_err();
                let untouched = fs::read(&current_path);
                // Restore the exact committed evidence before reporting any
                // failed assertion; the owner must still remain fail-closed.
                fs::write(&current_path, committed)?;
                anyhow::ensure!(
                    processed.is_ok() && closed && untouched? == rolled_back
                        && owner.process().is_err()
                        && owner.retained_receipt_for_test().as_deref() == Some(q_hash.as_str()),
                    "rollback was republished, lost its owner or reopened after repair"
                );
            }
            if matches!(exercise, StartupExercise::Confirm) {
                // Keep the exact owner, report, lease and Verified. A second
                // Q or a writer factory would abandon the ambiguity under
                // test instead of confirming its already committed Final.
                owner.process().context(
                    "same retained startup owner must reconcile committed Final after its ACK error",
                )?;
                owner.confirm_step().context(
                    "same startup owner must confirm the exact committed Final after its ACK error",
                )?;
                let report = owner.completed_report()?;
                let final_entry = report
                    .jobs()
                    .iter()
                    .find(|job| job.operation_id() == operation)
                    .and_then(|job| job.retirements().iter().find_map(|entry| entry.finalized()))
                    .ok_or_else(|| anyhow!("same-owner report did not adopt committed Final"))?;
                anyhow::ensure!(
                    final_entry.receipt().digest()?.as_str() == q_hash.as_str()
                        && canonical(final_entry.pending().identity())? == canonical(&identity)?
                        && retained.has_retained()
                        && !report.retirement_factory_consumed()
                        && retained.release_after_factory(&owner).is_err(),
                    "reconciliation changed the receipt/identity or consumed the factory"
                );
                let after: Value = serde_json::from_slice(&fs::read(
                    journal_path
                        .join("broker-journal-v1")
                        .join(&namespace_hash)
                        .join("jobs")
                        .join(operation.to_string())
                        .join("current.json"),
                )?)?;
                anyhow::ensure!(
                    after == current,
                    "confirming committed Final published another revision"
                );
            }
        } else {
            owner.confirm_step()?;
            let report = owner.completed_report()?;
            anyhow::ensure!(
                report
                    .jobs()
                    .iter()
                    .any(|job| job.operation_id() == operation
                        && job
                            .retirements()
                            .iter()
                            .any(|entry| entry.finalized().is_some())),
                "confirmed startup did not retain actual F before factory"
            );
            let capabilities = ProfileWriterCapabilities::from_recovery_report(
                &report,
                true,
                16,
                Duration::from_secs(10),
            )?;
            anyhow::ensure!(
                capabilities.is_stop_only()
                    && canonical(capabilities.stops().scope())? == canonical(&scope)?,
                "same report factory changed scope or minted Full"
            );
            retained.release_after_factory(&owner)?;
            anyhow::ensure!(
                !retained.has_retained(),
                "successful factory handoff did not release the slot"
            );
            anyhow::ensure!(
                ProfileWriterCapabilities::from_recovery_report(
                    &report,
                    true,
                    16,
                    Duration::from_secs(10)
                )
                .is_err(),
                "startup produced a second factory"
            );
        }
        Ok(())
    }
    .await;
    native.finish().await;
    gateway.finish().await.unwrap();
    outcome.unwrap();
}

async fn startup_service_reconcile_case(
    root: &Path,
    filesystem_roots: &Path,
    scope: &JournalScopeV1,
    intent: &ExecutionIntentV1,
    store: &Store,
    full: &EnforcerFullClientV3,
    jobs: Arc<ProfileJobs>,
    owner: Arc<super::super::retirement_startup::StartupOwner>,
    pending_revision: u64,
    q_hash: &str,
) -> Result<()> {
    let operation = intent.operation_id();
    let identity = owner.retained_receipt_for_test();
    anyhow::ensure!(
        identity.as_deref() == Some(q_hash),
        "real original Q missing"
    );
    jobs.job(operation)?;
    let registry = configured_registry(root, scope, intent, filesystem_roots)?;
    drop(registry);
    // An independent, genuinely empty journal profile must still finish its
    // handoff while the first profile cannot confirm its exact committed F.
    let registry_path = root.join("config/registry.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&registry_path)?)?;
    let mut neighbor = config["profiles"][0].clone();
    neighbor["id"] = json!("startup-neighbor");
    let namespace = scope.control_namespace_uuid();
    neighbor["control_namespace_uuid"] = json!(namespace);
    config["profiles"].as_array_mut().unwrap().push(neighbor);
    private_file(&registry_path, &serde_json::to_vec(&config)?)?;
    let registry = Arc::new(Registry::load(&registry_path)?);
    let profile = registry.profile(scope.profile_id())?.clone();
    let neighbor_scope = JournalScopeV1::new(
        scope.installation_id(),
        scope.tenant_id(),
        scope.gateway_origin(),
        namespace,
        "startup-neighbor",
    )?;
    let neighbor_path = root.join("journals/startup-neighbor");
    fs::create_dir(&neighbor_path)?;
    fs::set_permissions(&neighbor_path, fs::Permissions::from_mode(0o700))?;
    let neighbor_directory = Directory::open_existing(&neighbor_path)?;
    drop(Journal::open(
        &neighbor_directory,
        neighbor_scope.clone(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new())?,
    )?);
    let neighbor_report = StopRecoveryReaderV1::open_existing(
        &neighbor_directory,
        neighbor_scope,
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new())?,
    )?
    .ok_or_else(|| anyhow!("real neighbor reader missing"))?
    .scan();
    anyhow::ensure!(
        !neighbor_report.incomplete() && neighbor_report.jobs().is_empty(),
        "neighbor fixture must be a complete empty actual journal"
    );
    let neighbor_jobs = Arc::new(ProfileJobs::new());
    let neighbor_owner = neighbor_jobs.retirement_startup.install(neighbor_report)?;
    let material = Arc::new(Material::load(&registry)?);
    let service = Arc::new(Service {
        full_owners: FullOwners::new(registry.data.max_enforcer_slots as usize)?,
        cold_owners: retained_audit::ColdOwners::new(registry.data.max_enforcer_slots as usize)?,
        registry,
        root: Directory::open_existing(root)?,
        material,
        profiles: BTreeMap::from([
            (scope.profile_id().to_owned(), Arc::clone(&jobs)),
            ("startup-neighbor".into(), Arc::clone(&neighbor_jobs)),
        ]),
        enforcer: Some(ServiceEnforcer::Native(ExecutionEnforcerV1::Full3(
            full.clone(),
        ))),
        stop_enforcer: None,
        blocking: Arc::new(Semaphore::new(2)),
        preparation: Arc::new(Semaphore::new(1)),
        execution: Arc::new(Semaphore::new(1)),
        stopping: Arc::new(AtomicBool::new(false)),
        fenced: Arc::new(AtomicBool::new(true)),
    });
    let quota = canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?;
    let usage = canonical(&store.worker_usage()?)?;
    let scope_hash =
        crate::broker::journal::digest("ai-guard:broker-journal-scope:v1", &canonical(scope)?);
    let current_path = root
        .join("journals")
        .join(scope.profile_id())
        .join("broker-journal-v1")
        .join(scope_hash)
        .join("jobs")
        .join(operation.to_string())
        .join("current.json");
    owner.fail_ack_and_confirmation_for_test();
    // Any compensating Q would consume this failure and cannot provide the
    // original receipt needed for local-slot removal in the successful round.
    full.lose_query_reply_after_successes_for_test(0)?;
    anyhow::ensure!(
        Arc::clone(&service)
            .reconcile_retirement_audit_profile(profile.clone())
            .await
            .is_err(),
        "actual service must retain the first postcommit ACK error"
    );
    let committed_bytes = fs::read(&current_path)?;
    let committed: Value = serde_json::from_slice(&committed_bytes)?;
    anyhow::ensure!(
        committed["head"]["revision"].as_u64() == Some(pending_revision + 1)
            && jobs.writer.get().is_none()
            && service.has_retained_retirement()
            && Arc::ptr_eq(&jobs.retirement_startup.retained()?.unwrap(), &owner)
            && owner.retained_receipt_for_test().as_deref() == Some(q_hash),
        "actual postcommit failure changed owner/receipt or admitted a writer"
    );
    // This is the same metadata pump used by RetainedServiceShutdown after
    // normal close. No new Full task, business command or factory is admitted.
    service.stopping.store(true, Ordering::Release);
    service.full_owners.close()?;
    service.cold_owners.close()?;
    service.reconcile_retirement_audit().await;
    anyhow::ensure!(
        jobs.writer.get().is_none()
            && Arc::ptr_eq(&jobs.retirement_startup.retained()?.unwrap(), &owner)
            && owner.retained_receipt_for_test().as_deref() == Some(q_hash)
            && fs::read(&current_path)? == committed_bytes
            && neighbor_jobs
                .writer
                .get()
                .is_some_and(|cap| cap.is_stop_only())
            && !neighbor_jobs.retirement_startup.has_retained()
            && neighbor_owner
                .completed_report()?
                .retirement_factory_consumed(),
        "persistent IO lost the first owner, rewrote F or starved the other profile"
    );
    service.reconcile_retirement_audit().await;
    let cap = jobs
        .writer
        .get()
        .ok_or_else(|| anyhow!("normal pump did not hand off the retained owner"))?;
    anyhow::ensure!(
        cap.is_stop_only()
            && cap.full().is_none()
            && !jobs.retirement_startup.has_retained()
            && !service.has_retained_retirement()
            && service.require_starts().await.is_err(),
        "reconciliation did not finish exactly one restricted handoff"
    );
    let report = owner.completed_report()?;
    anyhow::ensure!(
        report.retirement_factory_consumed()
            && ProfileWriterCapabilities::from_recovery_report(
                &report,
                true,
                16,
                Duration::from_secs(10)
            )
            .is_err(),
        "reconciliation permitted a repeated factory"
    );
    let snapshot = cap
        .stops()
        .recovery_status(operation)
        .await?
        .ok_or_else(|| anyhow!("handed-off F missing"))?;
    let entry = snapshot
        .retirements()
        .first()
        .and_then(|entry| entry.finalized())
        .ok_or_else(|| anyhow!("handed-off exact Final missing"))?;
    anyhow::ensure!(
        entry.receipt().digest()?.as_str() == q_hash
            && !full.has_local_slot(&entry.pending().identity().arm().operation_id)?
            && cap
                .stops()
                .pending_retirement_events(operation)
                .await?
                .is_empty()
            && final_rows(store)?.len() == 1
            && canonical(&store.worker_quota(&QuotaPolicyV1::default())?)? == quota
            && canonical(&store.worker_usage()?)? == usage,
        "same-owner convergence changed Q/control facts or lost Final audit"
    );
    service.full_owners.drain().await?;
    service.cold_owners.drain().await?;
    Ok(())
}
