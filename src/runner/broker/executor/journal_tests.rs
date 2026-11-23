//! Real Journal/Writer and gateway contracts. The native enforcer remains empty;
//! no engine program, registration, start or stop is exercised by the tests in
//! this file. The child recovery module declares its synthetic engine fixture.
#[path = "cancelled_recovery_tests.rs"]
mod cancelled_recovery_tests;
#[path = "export_retirement_tests.rs"]
mod export_retirement_tests;
use super::super::diagnostics::BrokerStep;
use super::*;
use crate::{
    api::{GatewayState, gateway_router},
    audit::{EventQuery, Store},
    broker::{
        clock::NativeHostClock,
        discovery::{RoleLease, load_endpoint},
        enforcer::{EnforcerClient, run_internal_legacy_fresh},
        intent::ExecutionIntentV1,
        journal::{
            BindingInputs, ControlSecrets, FsExclusions, Journal, JournalLimitsV1, ReserveResult,
            RevisionRefV1,
        },
        local_ipc::IpcError,
        outbox::{BrokerCheckpointV4, OutboxEventV1},
        service::protocol::BrokerErrorCodeV1,
    },
    control_fs::{Directory, PrivateFile},
    model::Event,
    policy::Policy,
    redaction::Redactor,
    runtime::{Runtime, RuntimeKind},
};
pub(crate) use cancelled_recovery_tests::{
    ColdRecoverySeed, terminal_pending_seed_for_cold_audit_test,
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    fs,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Mutex as StdMutex,
        atomic::{AtomicBool, AtomicUsize},
    },
};
use tokio::sync::oneshot;

const TENANT: &str = "tenant-committed-draft-test";
const RUNNER: &str = "runner-committed-draft-aaaaaaaaaaaaaaaaaaaaaaaa";
const VIEWER: &str = "viewer-committed-draft-bbbbbbbbbbbbbbbbbbbbbbbb";
#[cfg(target_os = "macos")]
const PRIVATE_TMP: &str = "/private/tmp";

thread_local! {
    static MOVE_BINDING_ONCE: RefCell<Option<(PathBuf, PathBuf)>> = const { RefCell::new(None) };
}

pub(super) fn before_commit() -> Result<()> {
    if let Some((source, preserved)) = MOVE_BINDING_ONCE.with(|slot| slot.borrow_mut().take()) {
        fs::rename(source, preserved)?;
    }
    Ok(())
}

struct Fixture {
    store: Arc<Store>,
    journal: Option<Journal>,
    writer: Option<WriterClient>,
    scope: JournalScopeV1,
    client: AuditClient,
    config: Directory,
    requests: Arc<AtomicUsize>,
    drop_next_worker_transition_response: Arc<AtomicBool>,
    audit_response_gate: Arc<StdMutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>>,
    gateway: tokio::task::JoinHandle<()>,
    root: tempfile::TempDir,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.gateway.abort();
        MOVE_BINDING_ONCE.with(|slot| {
            slot.borrow_mut().take();
        });
    }
}
impl Fixture {
    /// Pause one successful event response after its real gateway handler has
    /// committed the event to SQLite, but before the response reaches the
    /// client. This is a test-only ownership barrier, not a fake receipt.
    pub(super) fn pause_next_audit_response_for_test(
        &self,
    ) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let mut gate = self.audit_response_gate.lock().unwrap();
        assert!(gate.is_none(), "only one audit response gate may be armed");
        *gate = Some((entered_tx, release_rx));
        (entered_rx, release_tx)
    }

    async fn new() -> Self {
        Self::new_with_writer_timeout(Duration::from_secs(10)).await
    }

    async fn new_with_writer_timeout(writer_timeout: Duration) -> Self {
        Self::new_with_layout(writer_timeout, false, None).await
    }

    /// Use the broker service's real `root/journals/<profile>` layout for a
    /// cold-recovery test. All other executor fixtures retain their original
    /// directory layout.
    pub(super) async fn new_cold_recovery_with_writer_timeout(writer_timeout: Duration) -> Self {
        Self::new_with_layout(writer_timeout, true, None).await
    }

    pub(super) async fn new_cold_recovery_with_scope(
        writer_timeout: Duration,
        installation_id: Uuid,
        profile_id: String,
    ) -> Self {
        Self::new_with_layout(writer_timeout, true, Some((installation_id, profile_id))).await
    }

    async fn new_with_layout(
        writer_timeout: Duration,
        service_layout: bool,
        scope_override: Option<(Uuid, String)>,
    ) -> Self {
        #[cfg(target_os = "linux")]
        let temporary_root = std::env::temp_dir();
        #[cfg(target_os = "macos")]
        let temporary_root = PathBuf::from(PRIVATE_TMP);
        let root = tempfile::Builder::new()
            .prefix("cd-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(temporary_root)
            .unwrap();
        let base = root.path().canonicalize().unwrap();
        let store = Arc::new(
            Store::create_supervised_for_tenant(
                &base.join("audit.sqlite"),
                TENANT,
                Redactor::new(vec![]),
                1000,
                &base.join("before-v3.sqlite"),
            )
            .unwrap(),
        );
        let namespace = Uuid::parse_str(
            store
                .supervision_state()
                .unwrap()
                .control_namespace_uuid
                .as_deref()
                .unwrap(),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&requests);
        let drop_next_worker_transition_response = Arc::new(AtomicBool::new(false));
        let observed_fault = Arc::clone(&drop_next_worker_transition_response);
        let audit_response_gate: Arc<
            StdMutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
        > = Arc::new(StdMutex::new(None));
        let observed_audit_response_gate = Arc::clone(&audit_response_gate);
        let app = gateway_router(GatewayState::new(
            Arc::clone(&store),
            RUNNER.into(),
            VIEWER.into(),
            Policy::default(),
            None,
            Redactor::new(vec![]),
        ))
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let observed = Arc::clone(&observed);
                let observed_fault = Arc::clone(&observed_fault);
                let observed_audit_response_gate = Arc::clone(&observed_audit_response_gate);
                async move {
                    let decision = request.uri().path() == "/control/api/decide";
                    let worker_transition = request.method() == axum::http::Method::POST
                        && request.uri().path().starts_with("/control/api/v1/workers/")
                        && request.uri().path().ends_with("/transitions");
                    let audit_event = request.method() == axum::http::Method::POST
                        && matches!(request.uri().path(), "/api/events" | "/api/v1/events");
                    if decision {
                        observed.fetch_add(1, Ordering::SeqCst);
                        eprintln!("cancelled_recovery_fixture phase=decision_http_enter");
                    }
                    if worker_transition {
                        eprintln!("cancelled_recovery_fixture phase=worker_transition_http_enter");
                    }
                    let response = next.run(request).await;
                    if decision {
                        eprintln!("cancelled_recovery_fixture phase=decision_http_complete");
                    }
                    if worker_transition {
                        eprintln!(
                            "cancelled_recovery_fixture phase=worker_transition_http_complete"
                        );
                    }
                    if worker_transition
                        && response.status().is_success()
                        && observed_fault.swap(false, Ordering::SeqCst)
                    {
                        eprintln!(
                            "cancelled_recovery_fixture phase=worker_transition_response_cut"
                        );
                        // The real handler has returned a successful response
                        // after committing. Fail its body stream so the client
                        // cannot receive the durable transition acknowledgement.
                        let (mut parts, _) = response.into_parts();
                        parts.headers.remove(axum::http::header::CONTENT_LENGTH);
                        let body = axum::body::Body::from_stream(futures_util::stream::once(
                            std::future::ready(Err::<axum::body::Bytes, std::io::Error>(
                                std::io::Error::new(
                                    std::io::ErrorKind::ConnectionReset,
                                    "test fixture drops the committed transition acknowledgement",
                                ),
                            )),
                        ));
                        return axum::response::Response::from_parts(parts, body);
                    }
                    if audit_event && response.status().is_success() {
                        let gate = observed_audit_response_gate.lock().unwrap().take();
                        if let Some((entered, release)) = gate {
                            let _ = entered.send(());
                            let _ = release.await;
                        }
                    }
                    response
                }
            },
        ));
        let gateway = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        for name in ["journal", "config", "journals"] {
            fs::create_dir(base.join(name)).unwrap();
            fs::set_permissions(base.join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = Directory::open_existing(&base.join("config")).unwrap();
        let (installation, profile) =
            scope_override.unwrap_or_else(|| (Uuid::new_v4(), "committed-test".to_owned()));
        let scope =
            JournalScopeV1::new(installation, TENANT, &origin, namespace, &profile).unwrap();
        let journal_path = if service_layout {
            base.join("journals").join(scope.profile_id())
        } else {
            base.join("journal")
        };
        if service_layout {
            fs::create_dir(&journal_path).unwrap();
            fs::set_permissions(&journal_path, fs::Permissions::from_mode(0o700)).unwrap();
            let reconciler_token = base.join("config/reconciler.token");
            fs::write(&reconciler_token, RUNNER).unwrap();
            fs::set_permissions(&reconciler_token, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let journal = Journal::open(
            &Directory::open_existing(&journal_path).unwrap(),
            scope.clone(),
            &FsExclusions::new(),
            Default::default(),
            ControlSecrets::new(vec![]).unwrap(),
        )
        .unwrap();
        let writer = WriterClient::start(journal.clone(), 8, writer_timeout).unwrap();
        Self {
            store,
            journal: Some(journal),
            writer: Some(writer),
            scope,
            client: AuditClient::new(&origin, RUNNER).unwrap(),
            config,
            requests,
            drop_next_worker_transition_response,
            audit_response_gate,
            gateway,
            root,
        }
    }

    fn journal(&self) -> &Journal {
        self.journal.as_ref().expect("fixture journal is open")
    }

    fn writer(&self) -> &WriterClient {
        self.writer.as_ref().expect("fixture writer is open")
    }

    /// Drop every full journal owner while retaining the real gateway and
    /// private fixture directory for a cold StopOnly reopen.
    fn close_journal_owners_for_cold_test(&mut self) {
        drop(self.writer.take());
        drop(self.journal.take());
    }

    fn accept(&self, operation: Uuid, manual: bool) -> AcceptedJobV1 {
        let mut plan: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/broker_intent-plan-v3.json"
        )))
        .unwrap();
        plan["version"] = json!(1);
        plan["operation_id"] = json!(operation);
        plan["options"]["config_dir"] = json!(self.config.path());
        plan["options"]["gateway"] = json!(self.scope.gateway_origin());
        plan["options"]["profile"] = json!(if manual { "observe" } else { "strict" });
        plan["options"]["network"] = json!(if manual { "bridge" } else { "none" });
        plan["options"]["provider"] = json!("generic");
        plan["options"]["argv"] = json!(["/usr/bin/id"]);
        for field in [
            "client",
            "auth_mode",
            "client_profile",
            "providers_file",
            "prompt",
            "model",
        ] {
            plan["options"].as_object_mut().unwrap().remove(field);
        }
        plan["request"]["request_id"] = json!(operation);
        plan["request"]["profile"] = plan["options"]["profile"].clone();
        plan["request"]["agent_id"] = json!("generic-22222222-2222-4222-8222-222222222222");
        plan["request"]["command"]["argv"] = json!(["/usr/bin/id"]);
        plan["request"]
            .as_object_mut()
            .unwrap()
            .remove("provider_resolution");
        plan["provider_input"] = Value::Null;
        plan["provider_credential"] = Value::Null;
        let source = manual.then(|| {
            let path = self.root.path().join(format!("manual-{operation}.env"));
            fs::write(&path, b"FIXTURE_KEY=synthetic-committed-draft-canary\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            plan["options"]["secret_env_file"] = json!(path);
            PrivateFile::open(&path, 64 * 1024).unwrap()
        });
        let intent =
            ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap();
        let initial = CheckpointPayloadV1::initial(&intent).unwrap();
        let ReserveResult::New(ticket) = self
            .journal()
            .reserve(operation, br#"{"committed_fixture":true}"#)
            .unwrap()
        else {
            panic!("fresh fixture");
        };
        let binding = source
            .as_ref()
            .map_or(BindingInputs::None, |source| BindingInputs::Manual {
                source,
            });
        self.journal()
            .accept(ticket, &intent, binding, &initial)
            .unwrap()
    }
}

struct NativePeer {
    client: EnforcerClient,
    child: NativeChild,
}

struct NativeChild(Child);
impl Drop for NativeChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
const FRESH_PEER_PARENT: &str = "AI_GUARD_TEST_RECOVERY_FRESH_PEER_PARENT";
const FRESH_PEER_NONCE: &str = "AI_GUARD_TEST_RECOVERY_FRESH_PEER_NONCE";

#[test]
#[ignore = "only launched by the cancelled recovery fixture"]
fn native_fresh_recovery_peer_child() {
    let path = PathBuf::from(std::env::var_os(FRESH_PEER_PARENT).unwrap());
    let parent = Directory::open_existing(&path).unwrap();
    let nonce = Uuid::parse_str(&std::env::var(FRESH_PEER_NONCE).unwrap()).unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_internal_legacy_fresh(&parent, "enforcer", 8, 0, nonce))
        .unwrap();
}

impl NativePeer {
    async fn empty(fixture: &Fixture) -> Self {
        Self::start(fixture, None).await
    }

    async fn for_admission(fixture: &Fixture) -> Self {
        Self::start(fixture, Some(Uuid::new_v4())).await
    }

    async fn start(fixture: &Fixture, fresh_nonce: Option<Uuid>) -> Self {
        let path = fixture.root.path().canonicalize().unwrap();
        let parent = Directory::open_existing(&path).unwrap();
        let executable = std::env::current_exe().unwrap();
        let stderr = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(fixture.root.path().join("enforcer.stderr"))
            .unwrap();
        let mut command = Command::new(&executable);
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr));
        if let Some(nonce) = fresh_nonce {
            // The owned child creates the missing role with the genuine
            // factory. Reopening an existing role leaves it cold-fenced.
            assert!(
                parent
                    .persistent_child("enforcer", false)
                    .unwrap()
                    .is_none()
            );
            command
                .args([
                    "--exact",
                    "runner::broker::executor::journal::tests::native_fresh_recovery_peer_child",
                    "--ignored",
                ])
                .env(FRESH_PEER_PARENT, parent.path())
                .env(FRESH_PEER_NONCE, nonce.to_string());
        } else {
            // Seed an empty Legacy V1 role and release its lease before recovery.
            let lease =
                RoleLease::create_new(parent.create_enforcer_root("enforcer").unwrap()).unwrap();
            let root = lease.root();
            root.persistent_create("auth", &[61; 32])
                .unwrap()
                .persistent_sync()
                .unwrap();
            root.sync().unwrap();
            command
                .args([
                    "--exact",
                    "broker::enforcer::client::retained_cancel_tests::native_cancel_server_child",
                    "--ignored",
                ])
                .env("AI_GUARD_TEST_RETAINED_CANCEL_ROOT", root.path());
        }
        let mut child = NativeChild(command.spawn().unwrap());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut owned = Self {
            client: loop {
                if let Some(root) = parent.persistent_child("enforcer", false).unwrap() {
                    match load_endpoint(&root) {
                        Ok(Some(_)) => {
                            break EnforcerClient::connect_or_spawn(&root, &executable, 8)
                                .await
                                .unwrap();
                        }
                        // The owned fixture can still be writing its first announcement.
                        Ok(None) | Err(IpcError::UnsafeFilesystem) => {}
                        Err(error) => panic!("native recovery fixture discovery failed: {error:?}"),
                    }
                }
                assert!(
                    child.0.try_wait().unwrap().is_none(),
                    "native fixture child exited before IPC was ready"
                );
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "native fixture child did not publish IPC"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            },
            child,
        };
        assert!(owned.child.0.try_wait().unwrap().is_none());
        assert_eq!(owned.client.status().await.unwrap().slot_count(), 0);
        if let Some(nonce) = fresh_nonce {
            let capabilities =
                serde_json::to_value(owned.client.legacy_capabilities().await.unwrap()).unwrap();
            assert!(
                capabilities.get("fresh_launch_nonce") == Some(&json!(nonce)),
                "native fixture child nonce mismatch"
            );
            owned.client.require_admission_ready().await.unwrap();
        }
        owned
    }
    async fn close(mut self) {
        self.client.shutdown_if_idle().await.unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while self.child.0.try_wait().unwrap().is_none() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "empty native child did not release its lease"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

#[tokio::test]
async fn committed_draft_conflict_preserves_cancel_and_acknowledged_outbox() {
    let fixture = Fixture::new().await;
    let operation = Uuid::new_v4();
    let accepted = fixture.accept(operation, false);
    let cancel_id = Uuid::new_v4();
    let failure_id = Uuid::new_v4();
    let injected = Cell::new(false);
    let attempts = Cell::new(0);
    let committed = commit_patch(fixture.writer(), operation, &fixture.scope, |draft| {
        attempts.set(attempts.get() + 1);
        if !injected.replace(true) {
            let cancel = LocalCommandV1::new(
                operation,
                accepted.intent_hash(),
                cancel_id,
                LocalCommandRequestV1::CancelRequested,
            )?;
            let mut concurrent = Draft::from_job(&accepted)?;
            concurrent.commands.push(cancel);
            let lifecycle = concurrent.checkpoint(fixture.scope.control_namespace_uuid())?;
            let mut event = Event::new("agent.progress");
            let request = &draft.run.plan.request;
            event.session_id = request.session_id.clone();
            event.project_id = request.project_id.clone();
            event.sandbox_id = request.sandbox_id.clone();
            event.correlation_id = request.correlation_id.clone();
            event.trace_id = request.trace_id.clone();
            let pending = OutboxEventV1::new(operation, accepted.intent_hash(), event)?;
            let born = BrokerCheckpointV4::new(lifecycle.clone(), vec![pending.clone()])?;
            let birth =
                fixture
                    .journal()
                    .advance_outbox(operation, accepted.head().clone(), &born)?;
            let acked = BrokerCheckpointV4::new(lifecycle, vec![pending.acknowledged(73)?])?;
            fixture
                .journal()
                .advance_outbox(operation, birth.head().clone(), &acked)?;
        }
        draft.commands.push(LocalCommandV1::new(
            operation,
            accepted.intent_hash(),
            failure_id,
            LocalCommandRequestV1::FailureObserved {
                phase: LocalFailurePhaseV1::Worker,
                code: LocalFailureCodeV1::Unavailable,
            },
        )?);
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(
        attempts.get(),
        2,
        "the obsolete CAS must reload before committing"
    );
    let current = fixture.writer().load(operation).await.unwrap().unwrap();
    assert_eq!(committed.head.hash(), current.head().hash());
    assert_eq!(committed.head.revision(), current.head().revision());
    assert!(
        committed
            .require_context(Uuid::new_v4(), &fixture.scope)
            .is_err()
    );
    let foreign = JournalScopeV1::new(
        Uuid::new_v4(),
        "tenant-foreign-test",
        "http://127.0.0.1:8787",
        Uuid::new_v4(),
        "foreign",
    )
    .unwrap();
    assert!(committed.require_context(operation, &foreign).is_err());
    committed
        .require_context(operation, &fixture.scope)
        .unwrap();
    assert!(committed.draft.cancelled());
    assert_eq!(committed.draft.commands.len(), 2);
    assert_eq!(
        current.broker_checkpoint_v4().unwrap().events()[0].audit_seq(),
        Some(73)
    );
    assert_eq!(
        serde_json::to_vec(
            &committed
                .draft
                .checkpoint(fixture.scope.control_namespace_uuid())
                .unwrap()
        )
        .unwrap(),
        serde_json::to_vec(current.broker_checkpoint_v3().unwrap()).unwrap()
    );
}

#[tokio::test]
async fn committed_rpc_replay_is_not_fresh_and_binding_loss_blocks_http() {
    let fixture = Fixture::new().await;
    let peer = NativePeer::empty(&fixture).await;
    let runtime =
        Runtime::with_program(RuntimeKind::Docker, &std::env::current_exe().unwrap()).unwrap();
    let context = AuthenticatedBatchContext::for_executor_test(
        fixture.client.clone(),
        fixture.scope.clone(),
        fixture.config.clone(),
        runtime,
    )
    .await
    .unwrap();
    let executor = BatchExecutor::new(
        fixture.writer().clone(),
        peer.client.clone(),
        Arc::new(NativeHostClock::new().unwrap()),
    );
    let operation = Uuid::new_v4();
    let accepted = fixture.accept(operation, true);
    let pending = Operation::Decision {
        request: accepted.checkpoint().0.current_request.clone(),
        response: None,
    };
    let execution = Execution::open(&executor, operation, &context)
        .await
        .unwrap();
    let (_, fresh, committed) = execution
        .rpc_checked_at_head(pending.clone(), None)
        .await
        .unwrap();
    assert!(fresh);
    let (_, replay_fresh, replay) = execution.rpc_checked_at_head(pending, None).await.unwrap();
    assert!(
        !replay_fresh,
        "stored ACK data never grants a fresh emission"
    );
    assert_eq!(committed.head.hash(), replay.head.hash());
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 1);
    drop(execution);

    let second = Uuid::new_v4();
    let accepted = fixture.accept(second, true);
    let pending = Operation::Decision {
        request: accepted.checkpoint().0.current_request.clone(),
        response: None,
    };
    let execution = Execution::open(&executor, second, &context).await.unwrap();
    let pointer_path = accepted
        .binding_source()
        .unwrap()
        .path()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("current.json");
    let preserved = fixture.root.path().join("preserved-binding.bin");
    MOVE_BINDING_ONCE.with(|slot| {
        assert!(
            slot.borrow_mut()
                .replace((
                    accepted.binding_source().unwrap().path().to_owned(),
                    preserved.clone()
                ))
                .is_none()
        );
    });
    let error = execution
        .rpc_checked_at_head(pending, None)
        .await
        .expect_err("strict writer binding validation must refuse this pending mutation");
    assert!(error.downcast_ref::<JournalError>().is_some());
    assert!(MOVE_BINDING_ONCE.with(|slot| slot.borrow().is_none()));
    assert!(preserved.is_file());
    assert_eq!(
        fixture.requests.load(Ordering::SeqCst),
        1,
        "strict precommit binding failure must precede HTTP"
    );
    assert!(fixture.writer().load(second).await.is_err());
    let pointer: Value = serde_json::from_slice(&fs::read(pointer_path).unwrap()).unwrap();
    assert_eq!(
        pointer["head"]["revision"],
        json!(accepted.head().revision())
    );
    assert_eq!(peer.client.status().await.unwrap().slot_count(), 0);
    drop(execution);
    drop(executor);
    peer.close().await;
}

#[tokio::test]
async fn unknown_failure_is_durable_public_and_opaque_without_runtime_authority() {
    let fixture = Fixture::new().await;
    let peer = NativePeer::empty(&fixture).await;
    let runtime =
        Runtime::with_program(RuntimeKind::Docker, &std::env::current_exe().unwrap()).unwrap();
    let context = AuthenticatedBatchContext::for_executor_test(
        fixture.client.clone(),
        fixture.scope.clone(),
        fixture.config.clone(),
        runtime,
    )
    .await
    .unwrap();
    let executor = BatchExecutor::new(
        fixture.writer().clone(),
        peer.client.clone(),
        Arc::new(NativeHostClock::new().unwrap()),
    );
    let operation = Uuid::new_v4();
    fixture.accept(operation, false);
    let execution = Execution::open(&executor, operation, &context)
        .await
        .unwrap();
    let private = "synthetic-unknown-batch-failure-private-canary";
    let boundaries = [
        (None, "failure_observed"),
        (
            Some(BrokerStep::TerminalObservationAck),
            "terminal_observation_ack",
        ),
        (Some(BrokerStep::ResultAudit), "result_audit"),
        (
            Some(BrokerStep::CleanupConfirmation),
            "cleanup_confirmation",
        ),
        (Some(BrokerStep::CleanupAck), "cleanup_ack"),
    ];
    for (boundary, _) in boundaries {
        let error = anyhow!("{private}");
        let error = match boundary {
            Some(boundary) => error.context(boundary),
            None => error,
        };
        execution.failure(LocalFailurePhaseV1::Worker, &error).await;
    }
    let current = fixture.writer().load(operation).await.unwrap().unwrap();
    let public: Value = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
        .get(format!("{}/api/v1/events", fixture.scope.gateway_origin()))
        .bearer_auth(VIEWER)
        .query(&[("kind", "broker.batch.failure"), ("order", "asc")])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let slots = peer.client.status().await.unwrap().slot_count();
    drop(execution);
    drop(executor);
    peer.close().await;

    assert_eq!(
        slots, 0,
        "failure reporting must leave the native peer empty"
    );
    let draft = Draft::from_job(&current).unwrap();
    assert_eq!(draft.commands.len(), boundaries.len());
    assert!(draft.commands.iter().all(|command| matches!(
        command.request(),
        LocalCommandRequestV1::FailureObserved {
            phase: LocalFailurePhaseV1::Worker,
            code: LocalFailureCodeV1::Unavailable,
        }
    )));
    assert!(draft.operations.is_empty());
    assert!(draft.helpers.is_empty());
    assert!(draft.run.plan.managed.is_none());
    let checkpoint = current
        .broker_checkpoint_v4()
        .expect("every observed batch failure must have a durable public outbox event");
    let items = public["items"].as_array().unwrap();
    assert_eq!(checkpoint.events().len(), boundaries.len());
    assert_eq!(items.len(), boundaries.len());
    for (((_, label), pending), item) in boundaries.into_iter().zip(checkpoint.events()).zip(items)
    {
        let event = pending.event();
        assert_eq!(event.kind, "broker.batch.failure");
        assert_eq!(event.source, "runner");
        assert_eq!(event.provenance, "observed");
        assert_eq!(event.coverage, "observed");
        assert!(event.command.is_none());
        assert!(event.decision.is_none());
        assert!(event.result.is_none());
        assert_eq!(
            event.data,
            json!({
                "version": 1,
                "phase": "worker",
                "runtime_effect": "unknown",
                "enforcer": null,
                "runtime": null,
                "broker_step": label,
            })
        );
        assert_eq!(event.id, pending.event_id().to_string());
        let stored: crate::model::StoredEvent = serde_json::from_value(item.clone()).unwrap();
        assert!(stored.seq > 0);
        assert_eq!(pending.audit_seq(), Some(stored.seq));
        assert_eq!(
            serde_json::to_value(&stored.event).unwrap(),
            serde_json::to_value(event).unwrap()
        );
    }
    assert!(!serde_json::to_string(checkpoint).unwrap().contains(private));
    assert!(!serde_json::to_string(&public).unwrap().contains(private));
}

const EXPORT_POLICY: &str = "workspace.export.policy";
const EXPORT_RESULT: &str = "broker.batch.export";

struct ExportPublicationFixture {
    source: PathBuf,
    destination: PathBuf,
    control: Directory,
    context: AuthenticatedBatchContext,
    enforcer: EnforcerClient,
    operation: Uuid,
    request: Uuid,
    profile: Value,
    initial_head: RevisionRefV1,
    archive: Vec<u8>,
    manifest_digest: String,
}

impl ExportPublicationFixture {
    async fn new(fixture: &Fixture) -> Result<Self> {
        let root = fixture.root.path().canonicalize()?;
        for name in ["source", "worker-copy", "exports", "export-control"] {
            let path = root.join(name);
            fs::create_dir(&path)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        let source = root.join("source");
        let worker_copy = root.join("worker-copy");
        fs::write(source.join("note.txt"), b"original\n")?;
        fs::write(worker_copy.join("note.txt"), b"exported\n")?;
        fs::set_permissions(
            worker_copy.join("note.txt"),
            fs::Permissions::from_mode(0o644),
        )?;
        let prepared = workspace::prepare_workspace(&worker_copy, Limits::default())?;
        let manifest_digest = content_hash("ai-guard:workspace-manifest:v1", &prepared.manifest)?;
        let archive = fs::read(prepared.archive.path())?;
        let exports = root.join("exports");
        let destination = exports.join("new-result");
        let control = Directory::open_existing(&root.join("export-control"))?;
        let operation = Uuid::new_v4();
        let request = Uuid::new_v4();
        let scope = &fixture.scope;
        let profile = json!({
            "id":scope.profile_id(), "gateway":scope.gateway_origin(),
            "tenant_id":scope.tenant_id(), "control_namespace_uuid":scope.control_namespace_uuid(),
            "config_dir":fixture.config.path(), "runner_token_file":root.join("unused.token"),
            "runtime":"docker", "runtime_program":std::env::current_exe()?, "runtime_endpoint":"unix:///unused-export-fixture.sock",
            "image":"agentsandbox-worker:local", "policy_profile":"strict", "network":"none",
            "allow_rootful_runtime":false, "max_timeout_seconds":60, "allow_capture_output":false,
            "managed_clients":false, "source_roots":[source], "export_roots":[exports]
        });
        let enforcer = EnforcerClient::unavailable_for_test(&control)?;
        let runtime = Runtime::unavailable_for_test(RuntimeKind::Docker, &control)?;
        assert!(
            runtime.command().is_err(),
            "export fixture must have no engine"
        );
        let context = AuthenticatedBatchContext::for_executor_test(
            fixture.client.clone(),
            scope.clone(),
            fixture.config.clone(),
            runtime,
        )
        .await?;
        let initial_head = fixture.accept(operation, false).head().clone();
        Ok(Self {
            source,
            destination,
            control,
            context,
            enforcer,
            operation,
            request,
            profile,
            initial_head,
            archive,
            manifest_digest,
        })
    }

    fn target(&self, path: &Path) -> Result<ValidatedExportTarget, BrokerErrorCodeV1> {
        ValidatedExportTarget::for_executor_test(
            &self.control,
            self.profile.clone(),
            self.context.scope(),
            self.operation,
            self.request,
            path,
        )
    }

    fn executor(&self, writer: &WriterClient) -> Result<BatchExecutor> {
        Ok(BatchExecutor::new(
            writer.clone(),
            self.enforcer.clone(),
            Arc::new(NativeHostClock::new()?),
        ))
    }

    fn assert_published_files(&self) -> Result<u64> {
        assert_eq!(fs::read(self.source.join("note.txt"))?, b"original\n");
        assert_eq!(fs::read(self.destination.join("note.txt"))?, b"exported\n");
        Ok(fs::metadata(&self.destination)?.ino())
    }
}

fn export_events(store: &Store, kind: &str) -> Result<Vec<crate::model::StoredEvent>> {
    store.events(&EventQuery {
        kind: Some(kind.into()),
        ..EventQuery::default()
    })
}

async fn reopen_export_writer(fixture: &Fixture, limits: JournalLimitsV1) -> Result<WriterClient> {
    let journal_root =
        Directory::open_existing(&fixture.root.path().canonicalize()?.join("journal"))?;
    let secrets = ControlSecrets::new(vec![])?;
    let journal = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match Journal::open(
                &journal_root,
                fixture.scope.clone(),
                &FsExclusions::new(),
                limits.clone(),
                secrets.clone(),
            ) {
                Err(JournalError::Busy) => tokio::time::sleep(Duration::from_millis(10)).await,
                result => break result,
            }
        }
    })
    .await
    .context("previous journal owner did not drain")??;
    Ok(WriterClient::start(journal, 8, Duration::from_secs(10))?)
}

#[tokio::test]
async fn published_export_replays_when_the_policy_audit_response_is_lost() -> Result<()> {
    use crate::broker::enforcer::EnforcerError;

    let mut fixture = Fixture::new().await;
    let export = ExportPublicationFixture::new(&fixture).await?;
    let executor = export.executor(fixture.writer())?;
    let execution = Execution::open(&executor, export.operation, &export.context).await?;
    let (entered, release) = fixture.pause_next_audit_response_for_test();
    {
        // Execute the production phase after the engine helper supplied its
        // archive. The gateway pauses its first response after real commit.
        let publishing =
            execution.publish_export(export.target(&export.destination)?, export.archive.clone());
        tokio::pin!(publishing);
        tokio::time::timeout(Duration::from_secs(15), async {
            tokio::select! {
                reply = &mut publishing => {
                    reply?;
                    anyhow::bail!("export completed before the audit response boundary");
                }
                committed = entered => committed.context("audit response barrier ended early"),
            }
        })
        .await
        .context("the policy audit did not reach its committed response boundary")??;
        // Drop the wait before releasing the already-committed response.
    }
    let _ = release.send(());
    let published_inode = export.assert_published_files()?;
    let initial_policy = export_events(&fixture.store, EXPORT_POLICY)?;
    assert_eq!(initial_policy.len(), 1);
    assert!(export_events(&fixture.store, EXPORT_RESULT)?.is_empty());
    let pending_job = fixture
        .writer()
        .load(export.operation)
        .await?
        .context("accepted job missing")?;
    let pending = pending_job
        .broker_checkpoint_v4()
        .context("export outbox missing")?
        .events();
    assert_eq!(
        pending
            .iter()
            .map(|entry| entry.event().kind.as_str())
            .collect::<Vec<_>>(),
        [EXPORT_POLICY, EXPORT_RESULT],
        "published export must retain both audit facts before the first HTTP response"
    );
    assert!(pending.iter().all(|entry| entry.audit_seq().is_none()));
    let retained_ids: Vec<_> = pending.iter().map(|entry| entry.event_id()).collect();
    assert_ne!(retained_ids[0], retained_ids[1]);
    assert!(retained_ids.iter().all(|id| *id != export.request));
    for id in &retained_ids {
        assert_eq!(
            pending_job
                .outbox_receipt(*id, None)
                .context("export fact has no pending receipt")?
                .head(),
            pending_job.head()
        );
    }

    drop(execution);
    drop(executor);
    fixture.close_journal_owners_for_cold_test();
    let writer = reopen_export_writer(&fixture, Default::default()).await?;
    let executor = export.executor(&writer)?;
    let resumed = Execution::open(&executor, export.operation, &export.context).await?;
    let outcome = Box::pin(resumed.export(export.target(&export.destination)?)).await?;
    let ControlFlow::Continue(receipt) = outcome else {
        anyhow::bail!("published export did not return its retained receipt");
    };
    assert_eq!(receipt.request_id, export.request);
    assert_eq!(receipt.manifest_digest, export.manifest_digest);
    assert_eq!(receipt.files, 1);
    assert_eq!(export.assert_published_files()?, published_inode);
    let divergent = export.destination.with_file_name("other-result");
    assert!(matches!(
        export.target(&divergent),
        Err(BrokerErrorCodeV1::Conflict)
    ));
    assert!(!divergent.exists());
    assert_eq!(
        fs::read_dir(export.destination.parent().unwrap())?.count(),
        1
    );

    let policy = export_events(&fixture.store, EXPORT_POLICY)?;
    let completion = export_events(&fixture.store, EXPORT_RESULT)?;
    assert_eq!(policy.len(), 1);
    assert_eq!(completion.len(), 1);
    assert_eq!(policy[0].seq, initial_policy[0].seq);
    assert_eq!(policy[0].event.id, initial_policy[0].event.id);
    assert_eq!(policy[0].hash, initial_policy[0].hash);
    assert_eq!(policy[0].event.data["omitted_entries"], 0);
    let current = writer
        .load(export.operation)
        .await?
        .context("reopened job missing")?;
    let confirmed = current
        .broker_checkpoint_v4()
        .context("reopened outbox missing")?
        .events();
    assert_eq!(confirmed.len(), 2);
    for (index, row) in [&policy[0], &completion[0]].into_iter().enumerate() {
        assert_eq!(Uuid::parse_str(&row.event.id)?, retained_ids[index]);
        assert_eq!(confirmed[index].event_id(), retained_ids[index]);
        assert_eq!(confirmed[index].audit_seq(), Some(row.seq));
    }
    // The positive result has no live guardian to contact, even during replay.
    assert!(matches!(
        export.enforcer.status().await,
        Err(EnforcerError::Unhealthy)
    ));
    Ok(())
}

#[tokio::test]
async fn published_export_at_the_journal_limit_is_preserved_without_a_receipt() -> Result<()> {
    let mut fixture = Fixture::new().await;
    let export = ExportPublicationFixture::new(&fixture).await?;
    let last_revision = u32::try_from(export.initial_head.revision())?;
    let limits = JournalLimitsV1::new(
        256 * 1024 * 1024,
        512,
        last_revision + 1,
        1024 * 1024 * 1024,
        65536,
        256,
    )?;
    fixture.close_journal_owners_for_cold_test();
    let published_inode = {
        let writer = reopen_export_writer(&fixture, limits).await?;
        let executor = export.executor(&writer)?;
        let execution = Execution::open(&executor, export.operation, &export.context).await?;
        // Run the actual post-helper publication with a readable head and no
        // room for another revision. This fails after exclusive rename/fsync.
        let error = execution
            .publish_export(export.target(&export.destination)?, export.archive.clone())
            .await
            .expect_err("the writer limit must prevent a confirmed export receipt");
        assert!(matches!(
            error.downcast_ref(),
            Some(JournalError::LimitExceeded)
        ));
        export.assert_published_files()?
    };

    let reopened = reopen_export_writer(&fixture, Default::default()).await?;
    let executor = export.executor(&reopened)?;
    let execution = Execution::open(&executor, export.operation, &export.context).await?;
    // Supply the archive again at the same production boundary, with the
    // writer limit removed, to exercise the refusal of an existing leaf.
    let retry = execution
        .publish_export(export.target(&export.destination)?, export.archive.clone())
        .await
        .expect_err("a new writer must not adopt or replace a published directory");
    assert_eq!(
        retry.root_cause().to_string(),
        "export destination already exists; nothing overwritten"
    );
    assert_eq!(export.assert_published_files()?, published_inode);
    let divergent = export.destination.with_file_name("other-result");
    assert!(matches!(
        export.target(&divergent),
        Err(BrokerErrorCodeV1::Conflict)
    ));
    assert!(!divergent.exists());
    assert_eq!(
        fs::read_dir(export.destination.parent().unwrap())?.count(),
        1
    );
    let job = reopened
        .load(export.operation)
        .await?
        .context("reopened job missing")?;
    assert_eq!(job.head(), &export.initial_head);
    assert!(job.broker_checkpoint_v4().is_none());
    assert!(export_events(&fixture.store, EXPORT_POLICY)?.is_empty());
    assert!(export_events(&fixture.store, EXPORT_RESULT)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn decision_ack_publication_and_generation_gate_are_distinct() -> Result<()> {
    let fixture = Fixture::new().await;
    for lose_ack in [true, false] {
        boxed_execution_phase(|| check_decision_ack_return(&fixture, lose_ack)).await?;
    }
    Ok(())
}

async fn check_decision_ack_return(fixture: &Fixture, lose_ack: bool) -> Result<()> {
    let runtime = Runtime::unavailable_for_test(RuntimeKind::Docker, &fixture.config)?;
    let context = AuthenticatedBatchContext::for_executor_test(
        fixture.client.clone(),
        fixture.scope.clone(),
        fixture.config.clone(),
        runtime,
    )
    .await?;
    let writer = fixture.writer();
    let executor = BatchExecutor::new(
        writer.clone(),
        EnforcerClient::unavailable_for_test(&fixture.config)?,
        Arc::new(NativeHostClock::new()?),
    );
    let operation = Uuid::new_v4();
    let accepted = fixture.accept(operation, false);
    let request = accepted.checkpoint().0.current_request.clone();
    let pending = Operation::Decision {
        request: request.clone(),
        response: None,
    };
    let execution = Execution::open(&executor, operation, &context).await?;
    ensure!(
        execution.generation_current().is_ok(),
        "fixture generation must be current"
    );
    let posts_before = fixture.requests.load(Ordering::SeqCst);
    let audit_before = fixture.store.head()?;

    // Place the existing Writer barrier on the ACK, never on the pending write.
    execution
        .patch(|draft| {
            ensure!(
                draft.operations.is_empty() && draft.run.plan.managed.is_none(),
                "fixture must precede any decision or admission"
            );
            draft.operations.push(pending.clone());
            Ok(())
        })
        .await?;
    let mut barrier = writer.advance_reply_barrier_for_test(lose_ack).await?;
    let mut rpc = boxed_execution_phase(|| execution.rpc_checked_at_head(pending, None));
    tokio::select! {
        _ = &mut rpc => return Err(anyhow!("Decision returned before its Writer reply barrier")),
        reached = tokio::time::timeout(Duration::from_secs(5), barrier.entered()) => {
            reached.context("Decision ACK Writer barrier not reached")?;
        }
    }
    // Use the strict journal while the Writer reply is still held. The barrier
    // alone can also be reached after a failed journal call.
    let durable = fixture
        .journal()
        .load(operation)?
        .context("decision checkpoint missing")?;
    let checkpoint = Draft::from_job(&durable)?;
    let [
        Operation::Decision {
            request: stored_request,
            response: Some(durable_answer),
        },
    ] = checkpoint.operations.as_slice()
    else {
        return Err(anyhow!(
            "decision ACK must already be durable before the Writer reply"
        ));
    };
    let receipt = durable_answer.receipt();
    let events = fixture.store.events(&EventQuery {
        kind: Some("security.decision".into()),
        limit: Some(4),
        ..Default::default()
    })?;
    ensure!(
        durable.head().revision() == accepted.head().revision() + 2
            && checkpoint.run.plan.managed.is_none()
            && same(stored_request, &request)?
            && durable_answer.decision().outcome == Outcome::Allow
            && receipt.request_id == request.request_id
            && receipt.request_hash == crate::policy::request_hash(&request)
            && receipt.audit_seq == audit_before + 1
            && events
                .iter()
                .any(|event| event.seq == receipt.audit_seq && event.hash == receipt.audit_hash)
            && fixture.requests.load(Ordering::SeqCst) == posts_before + 1,
        "journal ACK must match the one durable Allow decision while its reply is held"
    );
    let expected_answer = durable_answer.clone();
    drop(checkpoint);
    drop(barrier);
    let returned = rpc.await;
    ensure!(
        execution.phase() == LocalFailurePhaseV1::Decision
            && execution.generation_current().is_ok(),
        "decision exchange must leave a current generation in Decision"
    );
    if lose_ack {
        ensure!(
            matches!(returned, Err(ref error) if matches!(error.downcast_ref::<JournalError>(), Some(JournalError::Io))),
            "the durable decision must not be returned after loss of the Writer ACK"
        );
    } else {
        let (completed, fresh, returned_head) = returned?;
        let Operation::Decision {
            response: Some(answer),
            ..
        } = completed
        else {
            return Err(anyhow!("successful Writer ACK did not return a decision"));
        };
        ensure!(
            !fresh && returned_head.head == *durable.head() && same(&answer, &expected_answer)?,
            "the returned decision must be the exact durable acknowledgement"
        );

        // Current generation crosses into the real preparation method. This
        // test enforcer refuses before the method can touch workspace or runtime.
        let valid = boxed_execution_phase(|| execution.prepare_active(&accepted, answer.clone()))
            .await
            .err()
            .context("fixture enforcer permitted workspace preparation")?;
        ensure!(
            execution.phase() == LocalFailurePhaseV1::WorkspacePreparation
                && matches!(
                    valid.downcast_ref::<crate::broker::enforcer::EnforcerError>(),
                    Some(crate::broker::enforcer::EnforcerError::Unhealthy)
                ),
            "current generation must enter WorkspacePreparation before the enforcer refusal"
        );

        // A separate execution starts again at Decision on this same retained job.
        let stale = Execution::open(&executor, operation, &context).await?;
        ensure!(
            stale.generation_current().is_ok(),
            "new execution must start current"
        );
        context.cancel_generation().invalidate();
        let refused = boxed_execution_phase(|| stale.prepare_active(&accepted, answer))
            .await
            .err()
            .context("stale generation advanced to workspace preparation")?;
        ensure!(
            stale.phase() == LocalFailurePhaseV1::Decision
                && stale.generation_current().is_err()
                && refused.to_string() == "execution generation was cancelled",
            "generation rejection must precede the WorkspacePreparation marker"
        );
    }
    ensure!(
        fixture.requests.load(Ordering::SeqCst) == posts_before + 1,
        "Decision endpoint must observe exactly one request through the end of this variant"
    );
    ensure!(
        fixture.store.head()? == audit_before + 1
            && fixture.store.workers()?.is_empty()
            && fixture
                .journal()
                .load(operation)?
                .context("retained decision missing")?
                .head()
                == durable.head(),
        "the canary must leave only the one decision and its two journal revisions"
    );
    Ok(())
}
