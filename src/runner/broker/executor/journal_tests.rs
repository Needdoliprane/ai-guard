//! Real Journal/Writer and gateway contracts. The native enforcer remains empty;
//! no engine program, registration, start or stop is exercised by these tests.
use super::*;
use crate::{
    api::{GatewayState, gateway_router},
    audit::Store,
    broker::{
        clock::NativeHostClock,
        discovery::load_endpoint,
        intent::ExecutionIntentV1,
        journal::{BindingInputs, ControlSecrets, FsExclusions, Journal, ReserveResult},
        outbox::{BrokerCheckpointV4, OutboxEventV1},
    },
    control_fs::{Directory, PrivateFile},
    model::Event,
    policy::Policy,
    redaction::Redactor,
    runtime::{Runtime, RuntimeKind},
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::AtomicUsize,
};

const TENANT: &str = "tenant-committed-draft-test";
const RUNNER: &str = "runner-committed-draft-aaaaaaaaaaaaaaaaaaaaaaaa";
const VIEWER: &str = "viewer-committed-draft-bbbbbbbbbbbbbbbbbbbbbbbb";
#[cfg(target_os = "macos")]
const PRIVATE_TMP: &str = "/private/tmp";
#[cfg(target_os = "linux")]
const PRIVATE_TMP: &str = "/tmp";

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
    journal: Journal,
    writer: WriterClient,
    scope: JournalScopeV1,
    client: AuditClient,
    config: Directory,
    requests: Arc<AtomicUsize>,
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
    async fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("cd-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(PRIVATE_TMP)
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
        let app = gateway_router(GatewayState::new(
            store,
            RUNNER.into(),
            VIEWER.into(),
            Policy::default(),
            None,
            Redactor::new(vec![]),
        ))
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let observed = Arc::clone(&observed);
                async move {
                    if request.uri().path() == "/api/decide" {
                        observed.fetch_add(1, Ordering::SeqCst);
                    }
                    next.run(request).await
                }
            },
        ));
        let gateway = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        for name in ["journal", "config"] {
            fs::create_dir(base.join(name)).unwrap();
            fs::set_permissions(base.join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = Directory::open_existing(&base.join("config")).unwrap();
        let scope =
            JournalScopeV1::new(Uuid::new_v4(), TENANT, &origin, namespace, "committed-test")
                .unwrap();
        let journal = Journal::open(
            &Directory::open_existing(&base.join("journal")).unwrap(),
            scope.clone(),
            &FsExclusions::new(),
            Default::default(),
            ControlSecrets::new(vec![]).unwrap(),
        )
        .unwrap();
        let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(10)).unwrap();
        Self {
            journal,
            writer,
            scope,
            client: AuditClient::new(&origin, RUNNER).unwrap(),
            config,
            requests,
            gateway,
            root,
        }
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
            .journal
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
        self.journal
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
impl NativePeer {
    async fn empty(fixture: &Fixture) -> Self {
        let path = fixture.root.path().canonicalize().unwrap().join("enforcer");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let root = Directory::open_existing(&path).unwrap();
        root.persistent_create("auth", &[61; 32])
            .unwrap()
            .persistent_sync()
            .unwrap();
        root.sync().unwrap();
        let executable = std::env::current_exe().unwrap();
        let stderr = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(fixture.root.path().join("enforcer.stderr"))
            .unwrap();
        let mut child = NativeChild(
            Command::new(&executable)
                .args([
                    "--exact",
                    "broker::enforcer::client::retained_cancel_tests::native_cancel_server_child",
                    "--ignored",
                ])
                .env_clear()
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("AI_GUARD_TEST_RETAINED_CANCEL_ROOT", &path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::from(stderr))
                .spawn()
                .unwrap(),
        );
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut owned = Self {
            client: loop {
                if load_endpoint(&root).unwrap().is_some() {
                    break EnforcerClient::connect_or_spawn(&root, &executable, 8)
                        .await
                        .unwrap();
                }
                assert!(
                    child.0.try_wait().unwrap().is_none(),
                    "empty native child exited"
                );
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "empty native child did not publish IPC"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            },
            child,
        };
        assert!(owned.child.0.try_wait().unwrap().is_none());
        assert_eq!(owned.client.status().await.unwrap().slot_count(), 0);
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
    let committed = commit_patch(&fixture.writer, operation, &fixture.scope, |draft| {
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
                    .journal
                    .advance_outbox(operation, accepted.head().clone(), &born)?;
            let acked = BrokerCheckpointV4::new(lifecycle, vec![pending.acknowledged(73)?])?;
            fixture
                .journal
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
    let current = fixture.writer.load(operation).await.unwrap().unwrap();
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
        fixture.writer.clone(),
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
    assert!(fixture.writer.load(second).await.is_err());
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
