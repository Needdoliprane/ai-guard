//! Operator CLI behavior over durable private files and real gateway storage.
//! No model, IdP, engine or external infrastructure belongs to this target.
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    broker_registry::{
        BrokerAuthorityV1, BrokerGrantMutationV1, BrokerRuntimeKindV1, BrokerStateV1,
    },
    control::*,
    model::{CommandSpec, DecisionRequest},
    policy::{self, Policy},
    redaction::Redactor,
    runtime::capture_stream,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::TcpListener,
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use uuid::Uuid;

const NAMESPACE: &str = "11111111-1111-4111-8111-111111111111";
const BROKER: &str = "22222222-2222-4222-8222-222222222222";
const INCARNATION: &str = "33333333-3333-4333-8333-333333333333";
const RUNNER: &str = "public-workload-cli-runner-fixture-0000000000000000";
const VIEWER: &str = "public-workload-cli-viewer-fixture-0000000000000000";
const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
static GATEWAY_TEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Output {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command.env_clear();
    command
}

async fn run(command: Command, directory: &Path) -> Output {
    run_input(command, directory, None).await
}

async fn run_input(mut command: Command, directory: &Path, input: Option<Vec<u8>>) -> Output {
    let stdin = if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    };
    let mut child = command
        .current_dir(directory)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let stdin = child.stdin.take();
    let observed = tokio::time::timeout(Duration::from_secs(25), async {
        tokio::join!(
            child.wait(),
            capture_stream(stdout, 32_768),
            capture_stream(stderr, 8192),
            async move {
                if let (Some(mut stdin), Some(input)) = (stdin, input) {
                    let _ = stdin.write_all(&input).await;
                    let _ = stdin.shutdown().await;
                }
            }
        )
    })
    .await;
    let (status, stdout, stderr, ()) = match observed {
        Ok(observed) => observed,
        Err(_) => {
            let _ = child.start_kill();
            assert!(
                tokio::time::timeout(Duration::from_secs(2), child.wait())
                    .await
                    .is_ok_and(|result| result.is_ok()),
                "reap this fixture's timed-out CLI child"
            );
            panic!("workload CLI exceeded its fixture deadline");
        }
    };
    let stdout = stdout.unwrap();
    let stderr = stderr.unwrap();
    assert!(!stdout.truncated && !stderr.truncated, "bounded CLI output");
    Output {
        status: status.unwrap(),
        stdout: stdout.bytes,
        stderr: stderr.bytes,
    }
}

fn private_write(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn no_secret(output: &Output, secret: &str) {
    assert!(!secret.is_empty());
    assert!(
        !output
            .stdout
            .windows(secret.len())
            .chain(output.stderr.windows(secret.len()))
            .any(|bytes| bytes == secret.as_bytes()),
        "a private fixture credential entered CLI output"
    );
}

fn success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "workload CLI operation must succeed; exit={:?}",
        output.status.code()
    );
    no_secret(output, RUNNER);
    no_secret(output, VIEWER);
    serde_json::from_slice(&output.stdout).unwrap()
}

fn issue_manifest() -> Value {
    json!({
        "version": 1,
        "action": "issue",
        "expected_revision": 0,
        "subject": {"kind":"broker", "broker_id":BROKER, "incarnation_id":INCARNATION},
        "expires_at":"2100-01-01T00:00:00.000Z"
    })
}

fn prepare_command(input: &Path, control: &Path, output: &Path) -> Command {
    let mut command = command();
    command
        .args(["workload", "prepare", "--input"])
        .arg(input)
        .arg("--control-file")
        .arg(control)
        .arg("--output-dir")
        .arg(output);
    command
}

fn operation_command(action: &str, operation: &Path, control: Option<&Path>) -> Command {
    let mut command = command();
    command
        .args(["workload", action, "--operation-dir"])
        .arg(operation);
    if let Some(control) = control {
        command.arg("--control-file").arg(control);
    }
    command
}

fn migrated_store(path: &Path, tenant: &str) -> Arc<Store> {
    let store = Store::open_for_tenant(path, tenant, Redactor::new(vec![]), 10_000).unwrap();
    store
        .migrate_control_v3(&path.with_extension("before3"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("before4"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("before5"))
        .unwrap();
    store
        .migrate_workload_v6(&path.with_extension("before6"))
        .unwrap();
    Arc::new(store)
}

struct Gateway {
    store: Arc<Store>,
    neighbor: Arc<Store>,
    url: String,
    namespace: String,
    requests: Arc<AtomicUsize>,
    posts: Arc<AtomicUsize>,
    preparation_missing: Arc<AtomicBool>,
    drop_next_post_response: Arc<AtomicBool>,
    protocol_corruption: Arc<AtomicUsize>,
    receipt_corruption: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Gateway {
    async fn new(root: &Path, operation: PathBuf) -> Self {
        let store = migrated_store(&root.join("tenant-a.sqlite"), "tenant-a");
        let neighbor = migrated_store(&root.join("tenant-b.sqlite"), "tenant-b");
        let namespace = store
            .supervision_state()
            .unwrap()
            .control_namespace_uuid
            .unwrap();
        assert!(
            store.supervision_state().unwrap().active,
            "the migrated fixture retains its initial lease"
        );
        let state = GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: store.clone(),
                    runner_token: RUNNER.into(),
                    viewer_token: VIEWER.into(),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: neighbor.clone(),
                    runner_token: "public-workload-cli-neighbor-runner-0000000000000000".into(),
                    viewer_token: "public-workload-cli-neighbor-viewer-0000000000000000".into(),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![RUNNER.into(), VIEWER.into()]),
        )
        .unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let posts = Arc::new(AtomicUsize::new(0));
        let preparation_missing = Arc::new(AtomicBool::new(false));
        let drop_next_post_response = Arc::new(AtomicBool::new(false));
        let protocol_corruption = Arc::new(AtomicUsize::new(0));
        let receipt_corruption = Arc::new(AtomicUsize::new(0));
        let request_counter = requests.clone();
        let post_counter = posts.clone();
        let missing = preparation_missing.clone();
        let drop_post_response = drop_next_post_response.clone();
        let protocol_mode = protocol_corruption.clone();
        let receipt_mode = receipt_corruption.clone();
        let app = gateway_router(state).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                request_counter.fetch_add(1, Ordering::SeqCst);
                if request.method() == axum::http::Method::POST {
                    post_counter.fetch_add(1, Ordering::SeqCst);
                }
                if !operation_has_required_private_material(&operation) {
                    missing.store(true, Ordering::SeqCst);
                }
                let post = request.method() == axum::http::Method::POST;
                let protocol = request.uri().path() == "/api/v1/control/protocol";
                let lose_response = drop_post_response.clone();
                let protocol_mode = protocol_mode.clone();
                let receipt_mode = receipt_mode.clone();
                async move {
                    let response = next.run(request).await;
                    if post && lose_response.swap(false, Ordering::SeqCst) {
                        let (parts, _) = response.into_parts();
                        return axum::response::Response::from_parts(
                            parts,
                            axum::body::Body::from_stream(futures_util::stream::once(async {
                                Err::<axum::body::Bytes, _>(std::io::Error::new(
                                    std::io::ErrorKind::UnexpectedEof,
                                    "owned workload fixture dropped a committed response",
                                ))
                            })),
                        );
                    }
                    let mode = if protocol {
                        protocol_mode.load(Ordering::SeqCst)
                    } else if post {
                        receipt_mode.swap(0, Ordering::SeqCst)
                    } else {
                        0
                    };
                    corrupt_transport_response(response, protocol, mode).await
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            store,
            neighbor,
            url,
            namespace,
            requests,
            posts,
            preparation_missing,
            drop_next_post_response,
            protocol_corruption,
            receipt_corruption,
            task,
        }
    }

    fn control_file(&self, root: &Path, name: &str, token: &str, tenant: &str) -> PathBuf {
        let runner = root.join(format!("{name}.token"));
        private_write(&runner, format!("{token}\n").as_bytes());
        let control = root.join(format!("{name}.json"));
        private_write(
            &control,
            &serde_json::to_vec(&json!({
                "version":1,
                "gateway":self.url,
                "expected_tenant_id":tenant,
                "control_namespace_uuid":self.namespace,
                "runner_token_file":runner,
                "ca_bundle_file":null
            }))
            .unwrap(),
        );
        control
    }

    async fn enable_tls(&mut self, chain: &Path, key: &Path) {
        let state = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: "tenant-a".into(),
                store: self.store.clone(),
                runner_token: RUNNER.into(),
                viewer_token: VIEWER.into(),
            }],
            Policy::default(),
            None,
            Redactor::new(vec![RUNNER.into(), VIEWER.into()]),
        )
        .unwrap();
        let requests = self.requests.clone();
        let posts = self.posts.clone();
        let app = gateway_router(state).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                requests.fetch_add(1, Ordering::SeqCst);
                if request.method() == axum::http::Method::POST {
                    posts.fetch_add(1, Ordering::SeqCst);
                }
                async move { next.run(request).await }
            },
        ));
        // Tests run only in the isolated network-none OS. Binding all addresses
        // lets 127.0.0.2 exercise a real certificate name mismatch locally.
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let identity = agentsandbox::tls::ServerIdentity::from_files(chain, key).unwrap();
        self.task.abort();
        self.task = tokio::spawn(async move {
            agentsandbox::tls::serve(listener, app, identity, std::future::pending::<()>())
                .await
                .unwrap();
        });
        self.url = format!("https://127.0.0.1:{port}/");
    }

    async fn reopen(self, root: &Path, operation: PathBuf) -> Self {
        self.reopen_with_capacity(root, operation, 10_000).await
    }

    async fn reopen_with_capacity(
        mut self,
        root: &Path,
        operation: PathBuf,
        max_events: u64,
    ) -> Self {
        let address = self
            .url
            .strip_prefix("http://")
            .unwrap()
            .trim_end_matches('/')
            .to_owned();
        let url = self.url.clone();
        let namespace = self.namespace.clone();
        let requests = self.requests.clone();
        let posts = self.posts.clone();
        let preparation_missing = self.preparation_missing.clone();
        let drop_next_post_response = self.drop_next_post_response.clone();
        let protocol_corruption = self.protocol_corruption.clone();
        let receipt_corruption = self.receipt_corruption.clone();
        let old_a = Arc::downgrade(&self.store);
        let old_b = Arc::downgrade(&self.neighbor);
        self.task.abort();
        let _ = (&mut self.task).await;
        drop(self);
        tokio::time::timeout(Duration::from_secs(2), async {
            while old_a.strong_count() != 0 || old_b.strong_count() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all old gateway and SQLite owners must close before reopen");
        let store = Arc::new(
            Store::open_existing_for_tenant(
                &root.join("tenant-a.sqlite"),
                "tenant-a",
                Redactor::new(vec![]),
                max_events,
            )
            .unwrap(),
        );
        let neighbor = Arc::new(
            Store::open_existing_for_tenant(
                &root.join("tenant-b.sqlite"),
                "tenant-b",
                Redactor::new(vec![]),
                10_000,
            )
            .unwrap(),
        );
        assert_eq!(
            store
                .supervision_state()
                .unwrap()
                .control_namespace_uuid
                .as_deref(),
            Some(namespace.as_str())
        );
        let state = GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: store.clone(),
                    runner_token: RUNNER.into(),
                    viewer_token: VIEWER.into(),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: neighbor.clone(),
                    runner_token: "public-workload-cli-neighbor-runner-0000000000000000".into(),
                    viewer_token: "public-workload-cli-neighbor-viewer-0000000000000000".into(),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![RUNNER.into(), VIEWER.into()]),
        )
        .unwrap();
        let request_counter = requests.clone();
        let post_counter = posts.clone();
        let missing = preparation_missing.clone();
        let app = gateway_router(state).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                request_counter.fetch_add(1, Ordering::SeqCst);
                if request.method() == axum::http::Method::POST {
                    post_counter.fetch_add(1, Ordering::SeqCst);
                }
                if !operation_has_required_private_material(&operation) {
                    missing.store(true, Ordering::SeqCst);
                }
                async move { next.run(request).await }
            },
        ));
        // Keep the original origin: recovery is never edited to follow a new port.
        let listener = tokio::net::TcpListener::bind(&address).await.unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            store,
            neighbor,
            url,
            namespace,
            requests,
            posts,
            preparation_missing,
            drop_next_post_response,
            protocol_corruption,
            receipt_corruption,
            task,
        }
    }
}

async fn corrupt_transport_response(
    response: axum::response::Response,
    protocol: bool,
    mode: usize,
) -> axum::response::Response {
    if mode == 0 {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, 16_384).await.unwrap();
    let mut value: Value = serde_json::from_slice(&bytes).unwrap();
    if protocol {
        match mode {
            1 => value["control_namespace_uuid"] = json!("44444444-4444-4444-8444-444444444444"),
            2 => value["role"] = json!("viewer"),
            3 => value["schema_version"] = json!(5),
            _ => unreachable!("owned fixture mode"),
        }
    } else if value.get("broker").is_some() {
        match mode {
            7 => value["broker"]["granted_at"] = json!("2200-01-01T00:00:00.000Z"),
            8 => {
                let received_at = chrono::Utc::now();
                let incarnation_id = value["broker"]["incarnation_id"].clone();
                value["broker"]["heartbeat_revision"] = json!(1);
                value["broker"]["last_heartbeat"] = json!({
                    "incarnation_id": incarnation_id,
                    "sequence": 1,
                    "received_at": received_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    "lease_expires_at": (received_at + chrono::Duration::seconds(60))
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    "heartbeat_provenance": "declared",
                    "reception_provenance": "observed"
                });
                value["receipt"]["heartbeat_revision"] = json!(1);
            }
            _ => unreachable!("owned broker-result fixture mode"),
        }
    } else {
        match mode {
            1 => value["credential_id"] = json!("44444444-4444-4444-8444-444444444444"),
            2 => value["revision"] = json!(999),
            3 => value["unknown"] = json!(true),
            4 => value["replayed"] = json!(true),
            5 => value["padding"] = json!("public-padding".repeat(2000)),
            6 => {
                parts.status = axum::http::StatusCode::SERVICE_UNAVAILABLE;
                value = json!({"error":{"message":RUNNER}});
            }
            _ => unreachable!("owned fixture mode"),
        }
    }
    parts.headers.remove(axum::http::header::CONTENT_LENGTH);
    axum::response::Response::from_parts(
        parts,
        axum::body::Body::from(serde_json::to_vec(&value).unwrap()),
    )
}

#[tokio::test]
async fn apply_records_one_broker_binding_and_resume_returns_its_historical_receipt() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let operation = root.join("operation");
    let gateway = Gateway::new(root, operation.clone()).await;
    let control = gateway.control_file(root, "control", RUNNER, "tenant-a");
    let input = root.join("input.json");
    private_write(&input, &serde_json::to_vec(&issue_manifest()).unwrap());
    success(&run(prepare_command(&input, &control, &operation), root).await);
    assert_eq!(gateway.requests.load(Ordering::SeqCst), 0);
    let neighbor_head = gateway.neighbor.head().unwrap();
    let before = gateway.store.head().unwrap();
    let first = run(operation_command("apply", &operation, Some(&control)), root).await;
    let report = success(&first);
    assert_eq!(report["status"], "operation_recorded");
    assert_eq!(report["current_state"], "not_observed");
    assert_eq!(report["consumer_delivery"], "not_observed");
    assert_eq!(gateway.posts.load(Ordering::SeqCst), 1);
    assert!(!gateway.preparation_missing.load(Ordering::SeqCst));
    assert_eq!(gateway.neighbor.head().unwrap(), neighbor_head);
    assert_eq!(gateway.store.head().unwrap(), before + 1);
    let key = fs::read_to_string(operation.join("credential")).unwrap();
    let key = key.trim_end_matches('\n');
    no_secret(&first, key);
    let events = gateway
        .store
        .events(&EventQuery {
            after: Some(before),
            limit: Some(10),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.kind, "control.workload.issue");
    assert_eq!(
        events[0].event.data["change"]["input"]["sha256_digest"],
        hex::encode(Sha256::digest(key.as_bytes()))
    );
    assert!(!serde_json::to_string(&events).unwrap().contains(key));
    assert!(operation.join("receipt").join("receipt.json").is_file());
    let replay = success(
        &run(
            operation_command("resume", &operation, Some(&control)),
            root,
        )
        .await,
    );
    assert_eq!(replay["receipt"], report["receipt"]);
    assert_eq!(gateway.store.head().unwrap(), before + 1);
    let observed_before = gateway.requests.load(Ordering::SeqCst);
    let local = success(&run(operation_command("status", &operation, None), root).await);
    assert_eq!(local["status"], "operation_recorded");
    assert_eq!(local["current_state"], "not_observed");
    assert_eq!(gateway.requests.load(Ordering::SeqCst), observed_before);
}

#[tokio::test]
async fn a_lost_committed_response_resumes_the_same_key_operation_and_audit_receipt() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let operation = root.join("operation");
    let gateway = Gateway::new(root, operation.clone()).await;
    let control = gateway.control_file(root, "control", RUNNER, "tenant-a");
    let input = root.join("input.json");
    private_write(&input, &serde_json::to_vec(&issue_manifest()).unwrap());
    success(&run(prepare_command(&input, &control, &operation), root).await);
    let key_before = fs::read(operation.join("credential")).unwrap();
    let request_before = fs::read(operation.join("recovery.json")).unwrap();
    let head = gateway.store.head().unwrap();
    gateway
        .drop_next_post_response
        .store(true, Ordering::SeqCst);
    let lost = run(operation_command("apply", &operation, Some(&control)), root).await;
    assert!(
        !lost.status.success(),
        "an interrupted response is unresolved"
    );
    assert!(lost.stdout.is_empty(), "no false successful JSON");
    no_secret(&lost, RUNNER);
    no_secret(
        &lost,
        std::str::from_utf8(&key_before)
            .unwrap()
            .trim_end_matches('\n'),
    );
    assert_eq!(
        gateway.store.head().unwrap(),
        head + 1,
        "the backend committed before loss"
    );
    assert_eq!(
        gateway.posts.load(Ordering::SeqCst),
        1,
        "no automatic HTTP retry"
    );
    assert!(!operation.join("receipt").exists());
    assert!(
        fs::read(operation.join("credential")).unwrap() == key_before,
        "retained private key changed"
    );
    assert_eq!(
        fs::read(operation.join("recovery.json")).unwrap(),
        request_before
    );
    let gateway = gateway.reopen(root, operation.clone()).await;
    assert_eq!(
        gateway.store.head().unwrap(),
        head + 1,
        "the same durable mutation survives control reopen"
    );
    let resumed = success(
        &run(
            operation_command("resume", &operation, Some(&control)),
            root,
        )
        .await,
    );
    assert_eq!(resumed["status"], "operation_recorded");
    assert_eq!(resumed["current_state"], "not_observed");
    assert_eq!(gateway.posts.load(Ordering::SeqCst), 2);
    assert_eq!(
        gateway.store.head().unwrap(),
        head + 1,
        "exact replay adds no second mutation"
    );
    assert!(
        fs::read(operation.join("credential")).unwrap() == key_before,
        "exact resume must preserve its private key"
    );
    assert_eq!(
        fs::read(operation.join("recovery.json")).unwrap(),
        request_before
    );
    let stored = gateway
        .store
        .events(&EventQuery {
            after: Some(head),
            limit: Some(10),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(resumed["receipt"]["audit_seq"], stored[0].seq);
    assert_eq!(resumed["receipt"]["audit_hash"], stored[0].hash);
}

#[tokio::test]
async fn apply_accepts_a_new_explicit_runner_file_but_never_another_tenant() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let operation = root.join("operation");
    let gateway = Gateway::new(root, operation.clone()).await;
    let old_runner = "public-workload-cli-retired-runner-0000000000000000";
    let prepared_control = gateway.control_file(root, "retired-control", old_runner, "tenant-a");
    let current_control = gateway.control_file(root, "current-control", RUNNER, "tenant-a");
    let wrong_control = gateway.control_file(
        root,
        "wrong-tenant-control",
        "public-workload-cli-neighbor-runner-0000000000000000",
        "tenant-a",
    );
    let input = root.join("input.json");
    private_write(&input, &serde_json::to_vec(&issue_manifest()).unwrap());
    let prepared = run(prepare_command(&input, &prepared_control, &operation), root).await;
    success(&prepared);
    no_secret(&prepared, old_runner);
    let a_head = gateway.store.head().unwrap();
    let b_head = gateway.neighbor.head().unwrap();
    let wrong = run(
        operation_command("apply", &operation, Some(&wrong_control)),
        root,
    )
    .await;
    assert!(!wrong.status.success());
    assert!(wrong.stdout.is_empty());
    assert_eq!(
        gateway.posts.load(Ordering::SeqCst),
        0,
        "wrong scope must stop before POST"
    );
    assert_eq!(gateway.store.head().unwrap(), a_head);
    assert_eq!(gateway.neighbor.head().unwrap(), b_head);
    let applied = run(
        operation_command("resume", &operation, Some(&current_control)),
        root,
    )
    .await;
    let receipt = success(&applied);
    no_secret(&applied, old_runner);
    assert_eq!(receipt["status"], "operation_recorded");
    assert_eq!(gateway.posts.load(Ordering::SeqCst), 1);
    assert_eq!(gateway.store.head().unwrap(), a_head + 1);
    assert_eq!(gateway.neighbor.head().unwrap(), b_head);
}

#[tokio::test]
async fn unknown_manifest_fields_and_reused_destinations_are_refused_without_network() {
    let directory = support::private_tempdir();
    let root = directory.path();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let control = root.join("control.json");
    let runner = root.join("runner");
    private_write(&runner, RUNNER.as_bytes());
    private_write(
        &control,
        &serde_json::to_vec(&json!({
            "version":1,
            "gateway":format!("http://{}/", listener.local_addr().unwrap()),
            "expected_tenant_id":"tenant-a",
            "control_namespace_uuid":NAMESPACE,
            "runner_token_file":runner,
            "ca_bundle_file":null
        }))
        .unwrap(),
    );
    for (index, field) in ["tenant_id", "sha256_digest", "actor", "raw_credential"]
        .iter()
        .enumerate()
    {
        let mut input = issue_manifest();
        input[*field] = json!("public-field-that-must-be-refused");
        let source = root.join(format!("invalid-{index}.json"));
        private_write(&source, &serde_json::to_vec(&input).unwrap());
        let destination = root.join(format!("refused-{index}"));
        let output = run(prepare_command(&source, &control, &destination), root).await;
        assert_eq!(
            output.status.code(),
            Some(1),
            "closed manifest field must refuse"
        );
        assert!(output.stdout.is_empty());
        no_secret(&output, RUNNER);
        assert!(
            !destination.exists(),
            "invalid input must not create preparation"
        );
    }
    let input = root.join("valid.json");
    private_write(&input, &serde_json::to_vec(&issue_manifest()).unwrap());
    let destination = root.join("existing");
    fs::create_dir(&destination).unwrap();
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o700)).unwrap();
    let marker = destination.join("keep");
    private_write(&marker, b"public-existing-fixture-file");
    let refused = run(prepare_command(&input, &control, &destination), root).await;
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(fs::read(marker).unwrap(), b"public-existing-fixture-file");
    assert_eq!(fs::read_dir(destination).unwrap().count(), 1);
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[tokio::test]
async fn prepare_accepts_public_stdin_and_bounds_it_without_opening_http() {
    let directory = support::private_tempdir();
    let root = directory.path();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let runner = root.join("runner");
    let control = root.join("control.json");
    private_write(&runner, RUNNER.as_bytes());
    private_write(
        &control,
        &serde_json::to_vec(&json!({
            "version":1,
            "gateway":format!("http://{}/", listener.local_addr().unwrap()),
            "expected_tenant_id":"tenant-a",
            "control_namespace_uuid":NAMESPACE,
            "runner_token_file":runner,
            "ca_bundle_file":null
        }))
        .unwrap(),
    );
    let input = serde_json::to_vec(&issue_manifest()).unwrap();
    let operation = root.join("stdin-operation");
    let prepared = run_input(
        prepare_command(Path::new("-"), &control, &operation),
        root,
        Some(input),
    )
    .await;
    assert_eq!(success(&prepared)["status"], "prepared");
    assert!(operation.join("credential").is_file());
    let rejected_destination = root.join("oversized");
    let rejected = run_input(
        prepare_command(Path::new("-"), &control, &rejected_destination),
        root,
        Some(vec![b' '; 16_385]),
    )
    .await;
    assert_eq!(rejected.status.code(), Some(1));
    assert!(rejected.stdout.is_empty());
    no_secret(&rejected, RUNNER);
    assert!(!rejected_destination.exists());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

async fn prepare_named(root: &Path, name: &str, value: &Value, control: &Path) -> PathBuf {
    let input = root.join(format!("{name}.json"));
    let operation = root.join(name);
    private_write(&input, &serde_json::to_vec(value).unwrap());
    success(&run(prepare_command(&input, control, &operation), root).await);
    operation
}

fn operation_has_required_private_material(operation: &Path) -> bool {
    let recovery = match fs::read(operation.join("recovery.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    {
        Some(recovery) => recovery,
        None => return false,
    };
    match recovery["action"].as_str() {
        Some("issue" | "rotate") => operation.join("credential").is_file(),
        Some("revoke" | "broker_grant" | "broker_revoke") => true,
        _ => false,
    }
}

fn broker_grant_manifest(
    credential_id: &str,
    incarnation_id: &str,
    expected_revision: Option<u64>,
    not_after: &str,
) -> Value {
    json!({
        "version": 1,
        "action": "broker_grant",
        "broker_id": BROKER,
        "expected_revision": expected_revision,
        "credential_id": credential_id,
        "incarnation_id": incarnation_id,
        "authorities": [
            {"authority_id":"local-prod","runtime_kind":"docker"},
            {"authority_id":"remote-prod","runtime_kind":"podman"}
        ],
        "lease_seconds": 60,
        "not_after": not_after
    })
}

fn broker_revoke_manifest(expected_revision: u64) -> Value {
    json!({
        "version": 1,
        "action": "broker_revoke",
        "broker_id": BROKER,
        "expected_revision": expected_revision
    })
}

async fn issue_broker_binding(
    root: &Path,
    control: &Path,
    name: &str,
    incarnation_id: &str,
) -> (String, String) {
    let manifest = json!({
        "version": 1,
        "action": "issue",
        "expected_revision": 0,
        "subject": {"kind":"broker", "broker_id":BROKER, "incarnation_id":incarnation_id},
        "expires_at":"2100-01-01T00:00:00.000Z"
    });
    let operation = prepare_named(root, &format!("{name}-binding"), &manifest, control).await;
    let applied = run(operation_command("apply", &operation, Some(control)), root).await;
    let report = success(&applied);
    let key = fs::read_to_string(operation.join("credential")).unwrap();
    let key = key.trim_end_matches('\n').to_owned();
    no_secret(&applied, &key);
    (report["credential_id"].as_str().unwrap().to_owned(), key)
}

async fn prepare_broker_grant(
    root: &Path,
    control: &Path,
    name: &str,
    credential_id: &str,
    incarnation_id: &str,
    expected_revision: Option<u64>,
    not_after: &str,
) -> PathBuf {
    prepare_named(
        root,
        name,
        &broker_grant_manifest(credential_id, incarnation_id, expected_revision, not_after),
        control,
    )
    .await
}

async fn heartbeat_broker(gateway: &Gateway, key: &str, incarnation_id: &str) -> Value {
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("{}api/v1/brokers/heartbeat", gateway.url))
        .bearer_auth(key)
        .json(&json!({
            "version": 1,
            "operation_id": Uuid::new_v4().to_string(),
            "expected_heartbeat_revision": 0,
            "incarnation_id": incarnation_id,
            "sequence": 1
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let result: Value = response.json().await.unwrap();
    assert!(
        !serde_json::to_string(&result).unwrap().contains(key),
        "broker metadata response must not disclose its workload credential"
    );
    result
}

async fn broker_self(gateway: &Gateway, key: &str) -> reqwest::StatusCode {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{}api/v1/brokers/self", gateway.url))
        .bearer_auth(key)
        .send()
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn explicit_rotation_and_revocation_change_real_broker_authentication_only() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let first_directory = root.join("initial");
    let gateway = Gateway::new(root, first_directory.clone()).await;
    let control = gateway.control_file(root, "control", RUNNER, "tenant-a");
    let first_directory = prepare_named(root, "initial", &issue_manifest(), &control).await;
    let initial = success(
        &run(
            operation_command("apply", &first_directory, Some(&control)),
            root,
        )
        .await,
    );
    let credential_id = initial["credential_id"].as_str().unwrap();
    gateway
        .store
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: BROKER.into(),
            expected_revision: None,
            credential_id: credential_id.into(),
            incarnation_id: INCARNATION.into(),
            authorities: vec![BrokerAuthorityV1 {
                authority_id: "owned-workload-cli-fixture".into(),
                runtime_kind: BrokerRuntimeKindV1::Docker,
            }],
            lease_seconds: 300,
            not_after: (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
        })
        .unwrap();
    let old_key = fs::read_to_string(first_directory.join("credential")).unwrap();
    let old_key = old_key.trim_end_matches('\n');
    assert_eq!(
        broker_self(&gateway, old_key).await,
        reqwest::StatusCode::OK
    );
    let rotation = prepare_named(
        root,
        "rotation",
        &json!({
            "version":1, "action":"rotate", "credential_id":credential_id, "expected_revision":1,
            "subject":issue_manifest()["subject"], "expires_at":"2100-01-01T00:00:00.000Z"
        }),
        &control,
    )
    .await;
    let rotated_output = run(operation_command("apply", &rotation, Some(&control)), root).await;
    let rotated = success(&rotated_output);
    assert_eq!(rotated["credential_id"], credential_id);
    assert_eq!(rotated["receipt"]["revision"], 2);
    let new_key = fs::read_to_string(rotation.join("credential")).unwrap();
    let new_key = new_key.trim_end_matches('\n');
    assert!(
        new_key != old_key,
        "rotation must generate a different private key"
    );
    no_secret(&rotated_output, old_key);
    no_secret(&rotated_output, new_key);
    assert_eq!(
        broker_self(&gateway, old_key).await,
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        broker_self(&gateway, new_key).await,
        reqwest::StatusCode::OK
    );
    let revocation = prepare_named(
        root,
        "revocation",
        &json!({
            "version":1, "action":"revoke", "credential_id":credential_id, "expected_revision":2
        }),
        &control,
    )
    .await;
    assert!(!revocation.join("credential").exists());
    let revoked = success(
        &run(
            operation_command("apply", &revocation, Some(&control)),
            root,
        )
        .await,
    );
    assert_eq!(revoked["receipt"]["revision"], 3);
    assert_eq!(revoked["current_state"], "not_observed");
    assert_eq!(
        broker_self(&gateway, new_key).await,
        reqwest::StatusCode::UNAUTHORIZED
    );
    let head = gateway.store.head().unwrap();
    let replay = success(
        &run(
            operation_command("resume", &revocation, Some(&control)),
            root,
        )
        .await,
    );
    assert_eq!(replay["receipt"], revoked["receipt"]);
    assert_eq!(gateway.store.head().unwrap(), head);
    let historical = success(&run(operation_command("status", &first_directory, None), root).await);
    assert_eq!(historical["receipt"]["revision"], 1);
    assert_eq!(historical["current_state"], "not_observed");
    assert!(
        fs::read_to_string(first_directory.join("credential"))
            .unwrap()
            .trim_end_matches('\n')
            == old_key,
        "the historical private key file must be preserved"
    );
}

fn rewrite_ca_control(source: &Path, destination: &Path, ca: Option<&Path>, gateway: Option<&str>) {
    let mut value: Value = serde_json::from_slice(&fs::read(source).unwrap()).unwrap();
    value["ca_bundle_file"] = json!(ca);
    if let Some(gateway) = gateway {
        value["gateway"] = json!(gateway);
    }
    private_write(destination, &serde_json::to_vec(&value).unwrap());
}

#[tokio::test]
async fn explicit_certificate_only_ca_enables_real_tls_without_weakening_chain_or_name() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let pki = root.join("pki");
    let mut generate = Command::new(env!("CARGO_BIN_EXE_ai-guard-demo-pki"));
    generate
        .env_clear()
        .arg("--out")
        .arg(&pki)
        .args(["--dns", "workload-fixture.invalid"]);
    let generated = run(generate, root).await;
    assert!(
        generated.status.success(),
        "owned Rust TLS fixture generation"
    );
    let operation = root.join("tls-operation");
    let mut gateway = Gateway::new(root, operation.clone()).await;
    gateway
        .enable_tls(&pki.join("server-chain.pem"), &pki.join("server.key"))
        .await;
    let untrusted = gateway.control_file(root, "untrusted", RUNNER, "tenant-a");
    let input = root.join("input.json");
    private_write(&input, &serde_json::to_vec(&issue_manifest()).unwrap());
    success(&run(prepare_command(&input, &untrusted, &operation), root).await);
    let refused = run(
        operation_command("apply", &operation, Some(&untrusted)),
        root,
    )
    .await;
    assert_eq!(refused.status.code(), Some(1), "unknown CA must refuse");
    no_secret(&refused, RUNNER);
    assert_eq!(
        gateway.requests.load(Ordering::SeqCst),
        0,
        "no HTTP before valid TLS"
    );
    let ca = root.join("trust.pem");
    private_write(&ca, &fs::read(pki.join("root-ca.pem")).unwrap());
    let trusted = root.join("trusted.json");
    rewrite_ca_control(&untrusted, &trusted, Some(&ca), None);
    let recorded = success(
        &run(
            operation_command("resume", &operation, Some(&trusted)),
            root,
        )
        .await,
    );
    assert_eq!(recorded["status"], "operation_recorded");
    assert_eq!(gateway.posts.load(Ordering::SeqCst), 1);

    // Positive control on the exact 127.0.0.2 listener used below: the DNS
    // name was explicitly supplied to the owned Rust PKI fixture as a SAN.
    // Only this test client resolves that name; production keeps its IP origin.
    let port = reqwest::Url::parse(&gateway.url).unwrap().port().unwrap();
    let fixture_tls = reqwest::Client::builder()
        .https_only(true)
        .tls_built_in_root_certs(false)
        .add_root_certificate(reqwest::Certificate::from_pem(&fs::read(&ca).unwrap()).unwrap())
        .resolve(
            "workload-fixture.invalid",
            std::net::SocketAddr::from(([127, 0, 0, 2], port)),
        )
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(5))
        .pool_max_idle_per_host(0)
        .build()
        .unwrap();
    let healthy_alias = fixture_tls
        .get(format!("https://workload-fixture.invalid:{port}/healthz"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        healthy_alias.status(),
        reqwest::StatusCode::OK,
        "the exact loopback alias is reachable with the certificate's valid DNS SAN"
    );
    drop(healthy_alias);
    let wrong_name_url = gateway.url.replace("127.0.0.1", "127.0.0.2");
    let wrong_name = root.join("wrong-name.json");
    rewrite_ca_control(&untrusted, &wrong_name, Some(&ca), Some(&wrong_name_url));
    let wrong_operation = root.join("wrong-name-operation");
    success(&run(prepare_command(&input, &wrong_name, &wrong_operation), root).await);
    let before = gateway.requests.load(Ordering::SeqCst);
    let refused = run(
        operation_command("apply", &wrong_operation, Some(&wrong_name)),
        root,
    )
    .await;
    assert_eq!(refused.status.code(), Some(1), "wrong SAN must refuse");
    assert_eq!(gateway.requests.load(Ordering::SeqCst), before);
    no_secret(&refused, RUNNER);

    for (name, bytes) in [
        ("private-key", fs::read(pki.join("server.key")).unwrap()),
        ("nine-certificates", fs::read(&ca).unwrap().repeat(9)),
        ("oversized", vec![b' '; 65_537]),
    ] {
        let invalid_ca = root.join(format!("{name}.pem"));
        private_write(&invalid_ca, &bytes);
        let invalid_control = root.join(format!("{name}-control.json"));
        rewrite_ca_control(&untrusted, &invalid_control, Some(&invalid_ca), None);
        let invalid_destination = root.join(format!("{name}-operation"));
        let refused = run(
            prepare_command(&input, &invalid_control, &invalid_destination),
            root,
        )
        .await;
        assert_eq!(
            refused.status.code(),
            Some(1),
            "CA input is certificate-only and bounded"
        );
        assert!(refused.stdout.is_empty());
        assert!(!invalid_destination.exists());
        no_secret(&refused, RUNNER);
    }
}

#[tokio::test]
async fn prepare_refuses_json_escaped_runner_in_metadata_before_persistence() {
    let directory = support::private_tempdir();
    let root = directory.path();
    let escaped_runner = "public-runner-\"quoted\\backslash-0000000000000000";
    let private_parent = root.join(escaped_runner);
    fs::create_dir(&private_parent).unwrap();
    fs::set_permissions(&private_parent, fs::Permissions::from_mode(0o700)).unwrap();
    let runner = root.join("runner");
    private_write(&runner, escaped_runner.as_bytes());
    let control = root.join("control.json");
    private_write(
        &control,
        &serde_json::to_vec(&json!({
            "version":1, "gateway":"http://127.0.0.1:1/", "expected_tenant_id":"tenant-a",
            "control_namespace_uuid":NAMESPACE, "runner_token_file":runner, "ca_bundle_file":null
        }))
        .unwrap(),
    );
    let input = root.join("input.json");
    private_write(&input, &serde_json::to_vec(&issue_manifest()).unwrap());
    let operation = private_parent.join("operation");
    let output = run(prepare_command(&input, &control, &operation), root).await;
    assert_eq!(
        output.status.code(),
        Some(1),
        "JSON escaping must not bypass the literal credential guard"
    );
    assert!(
        output.stdout.is_empty(),
        "a protected path must not enter JSON output"
    );
    no_secret(&output, escaped_runner);
    assert!(
        !operation.join("recovery.json").exists(),
        "protected metadata must not enter a retained recovery record"
    );
}

#[tokio::test]
async fn status_reports_prepared_before_any_http() {
    let directory = support::private_tempdir();
    let root = directory.path();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let runner = root.join("runner");
    private_write(&runner, RUNNER.as_bytes());
    let control = root.join("control.json");
    private_write(
        &control,
        &serde_json::to_vec(&json!({
            "version":1, "gateway":format!("http://{}/", listener.local_addr().unwrap()),
            "expected_tenant_id":"tenant-a", "control_namespace_uuid":NAMESPACE,
            "runner_token_file":runner, "ca_bundle_file":null
        }))
        .unwrap(),
    );
    let input = root.join("input.json");
    private_write(&input, &serde_json::to_vec(&issue_manifest()).unwrap());
    let operation = root.join("operation");
    success(&run(prepare_command(&input, &control, &operation), root).await);
    let status = success(&run(operation_command("status", &operation, None), root).await);
    assert_eq!(
        status["status"], "prepared",
        "no HTTP was possible without a durable scope observation"
    );
    assert_eq!(status["current_state"], "not_observed");
    assert!(status["receipt"].is_null());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[tokio::test]
async fn locked_modified_and_linked_private_preparations_stop_before_http() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let operation = root.join("operation");
    let gateway = Gateway::new(root, operation.clone()).await;
    let control = gateway.control_file(root, "control", RUNNER, "tenant-a");
    prepare_named(root, "operation", &issue_manifest(), &control).await;
    let lock = OpenOptions::new()
        .read(true)
        .open(operation.join("recovery.json"))
        .unwrap();
    // SAFETY: this fixture owns the immutable private recovery inode and FD.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let locked = run(operation_command("apply", &operation, Some(&control)), root).await;
    assert_eq!(locked.status.code(), Some(1));
    assert_eq!(gateway.requests.load(Ordering::SeqCst), 0);
    drop(lock);
    let key_path = operation.join("credential");
    let original = fs::read(&key_path).unwrap();
    fs::write(&key_path, format!("aigw1_{}\n", "0".repeat(64))).unwrap();
    let modified = run(operation_command("apply", &operation, Some(&control)), root).await;
    assert_eq!(modified.status.code(), Some(1));
    assert_eq!(gateway.requests.load(Ordering::SeqCst), 0);
    fs::write(&key_path, &original).unwrap();
    let saved = root.join("owned-original-key");
    fs::rename(&key_path, &saved).unwrap();
    symlink(&saved, &key_path).unwrap();
    let linked = run(operation_command("apply", &operation, Some(&control)), root).await;
    assert_eq!(linked.status.code(), Some(1));
    assert_eq!(gateway.requests.load(Ordering::SeqCst), 0);
    fs::remove_file(&key_path).unwrap(); // Remove only this fixture's new symlink.
    fs::rename(&saved, &key_path).unwrap();
    fs::hard_link(&key_path, &saved).unwrap();
    let hardlinked = run(operation_command("apply", &operation, Some(&control)), root).await;
    assert_eq!(hardlinked.status.code(), Some(1));
    assert_eq!(gateway.requests.load(Ordering::SeqCst), 0);
    for output in [&locked, &modified, &linked, &hardlinked] {
        assert!(output.stdout.is_empty());
        no_secret(output, RUNNER);
        no_secret(
            output,
            std::str::from_utf8(&original)
                .unwrap()
                .trim_end_matches('\n'),
        );
    }
}

#[tokio::test]
async fn fresh_namespace_viewer_role_and_old_schema_are_refused_before_post() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let operation = root.join("operation");
    let gateway = Gateway::new(root, operation.clone()).await;
    let control = gateway.control_file(root, "control", RUNNER, "tenant-a");
    prepare_named(root, "operation", &issue_manifest(), &control).await;
    let a_head = gateway.store.head().unwrap();
    let b_head = gateway.neighbor.head().unwrap();
    for mode in 1..=3 {
        gateway.protocol_corruption.store(mode, Ordering::SeqCst);
        let output = run(operation_command("apply", &operation, Some(&control)), root).await;
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        no_secret(&output, RUNNER);
        assert_eq!(
            gateway.posts.load(Ordering::SeqCst),
            0,
            "fresh incompatible scope stops before any mutation"
        );
        assert_eq!(gateway.store.head().unwrap(), a_head);
        assert_eq!(gateway.neighbor.head().unwrap(), b_head);
        assert!(!operation.join("scope").exists());
    }
    gateway.protocol_corruption.store(0, Ordering::SeqCst);
    let resumed = success(
        &run(
            operation_command("resume", &operation, Some(&control)),
            root,
        )
        .await,
    );
    assert_eq!(resumed["status"], "operation_recorded");
    assert_eq!(gateway.posts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn corrupted_committed_responses_never_publish_receipts_or_retry_automatically() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let gateway = Gateway::new(root, root.join("case-1")).await;
    let control = gateway.control_file(root, "control", RUNNER, "tenant-a");
    for mode in 1..=6 {
        let operation =
            prepare_named(root, &format!("case-{mode}"), &issue_manifest(), &control).await;
        let head = gateway.store.head().unwrap();
        let posts = gateway.posts.load(Ordering::SeqCst);
        gateway.receipt_corruption.store(mode, Ordering::SeqCst);
        let corrupted = run(operation_command("apply", &operation, Some(&control)), root).await;
        assert_eq!(
            corrupted.status.code(),
            Some(1),
            "invalid response must leave outcome unresolved"
        );
        assert!(corrupted.stdout.is_empty());
        no_secret(&corrupted, RUNNER);
        assert!(!operation.join("receipt").exists());
        assert_eq!(
            gateway.posts.load(Ordering::SeqCst),
            posts + 1,
            "never retry automatically"
        );
        assert_eq!(
            gateway.store.head().unwrap(),
            head + 1,
            "real gateway committed exactly once"
        );
        let recovered = success(
            &run(
                operation_command("resume", &operation, Some(&control)),
                root,
            )
            .await,
        );
        assert_eq!(recovered["status"], "operation_recorded");
        assert_eq!(gateway.posts.load(Ordering::SeqCst), posts + 2);
        assert_eq!(
            gateway.store.head().unwrap(),
            head + 1,
            "exact replay adds no second mutation"
        );
    }
}

#[tokio::test]
async fn worker_binding_cli_authenticates_only_its_authorized_software_attempt() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let operation = root.join("worker-a");
    let gateway = Gateway::new(root, operation.clone()).await;
    // Real typed Store gates and SQLite projections; runtime observations in
    // these helpers are declared software fixtures, never container evidence.
    let a = running_worker(&gateway.store);
    let b = running_worker(&gateway.neighbor);
    let control = gateway.control_file(root, "control", RUNNER, "tenant-a");
    let manifest = |worker: &WorkerV1| {
        json!({
            "version":1, "action":"issue", "expected_revision":0,
            "subject":{"kind":"worker","worker_id":worker.worker_id,"attempt_id":worker.attempt.attempt_id,"generation":1},
            "expires_at":"2100-01-01T00:00:00.000Z"
        })
    };
    prepare_named(root, "worker-a", &manifest(&a), &control).await;
    let applied = run(operation_command("apply", &operation, Some(&control)), root).await;
    assert_eq!(success(&applied)["status"], "operation_recorded");
    let key = fs::read_to_string(operation.join("credential")).unwrap();
    let key = key.trim_end_matches('\n');
    no_secret(&applied, key);
    let inbox = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{}api/v1/relay/messages", gateway.url))
        .bearer_auth(key)
        .send()
        .await
        .unwrap();
    assert_eq!(
        inbox.status(),
        reqwest::StatusCode::OK,
        "the issued worker key authenticates its own inbox"
    );
    let inbox: Value = inbox.json().await.unwrap();
    assert!(inbox["items"].as_array().unwrap().is_empty());
    let a_head = gateway.store.head().unwrap();
    let b_head = gateway.neighbor.head().unwrap();
    let foreign = prepare_named(root, "foreign-worker", &manifest(&b), &control).await;
    let refused = run(operation_command("apply", &foreign, Some(&control)), root).await;
    assert_eq!(
        refused.status.code(),
        Some(1),
        "another tenant's attempt is not issuer authority"
    );
    assert!(refused.stdout.is_empty());
    assert!(!foreign.join("receipt").exists());
    no_secret(&refused, key);
    no_secret(&refused, RUNNER);
    assert_eq!(gateway.store.head().unwrap(), a_head);
    assert_eq!(gateway.neighbor.head().unwrap(), b_head);
}

#[tokio::test]
async fn replay_after_real_binding_expiry_retains_the_same_historical_receipt() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let operation = root.join("expiring");
    let gateway = Gateway::new(root, operation.clone()).await;
    let control = gateway.control_file(root, "control", RUNNER, "tenant-a");
    // The existing CLI subprocess guard is 25s per call. Ninety seconds leaves
    // ample room for both prepare and apply; elapsed time is not a performance
    // assertion. Expiry itself is the functional condition observed below.
    let expires = chrono::Utc::now() + chrono::Duration::seconds(90);
    let expiry = expires.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut manifest = issue_manifest();
    manifest["expires_at"] = json!(expiry);
    prepare_named(root, "expiring", &manifest, &control).await;
    let first = success(&run(operation_command("apply", &operation, Some(&control)), root).await);
    let credential_id = first["credential_id"].as_str().unwrap();
    gateway
        .store
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: BROKER.into(),
            expected_revision: None,
            credential_id: credential_id.into(),
            incarnation_id: INCARNATION.into(),
            authorities: vec![BrokerAuthorityV1 {
                authority_id: "owned-expiry-software-fixture".into(),
                runtime_kind: BrokerRuntimeKindV1::Docker,
            }],
            lease_seconds: 300,
            not_after: (chrono::Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
        })
        .unwrap();
    let key = fs::read(operation.join("credential")).unwrap();
    let request = fs::read(operation.join("recovery.json")).unwrap();
    let key_text = std::str::from_utf8(&key).unwrap().trim_end_matches('\n');
    assert_eq!(
        broker_self(&gateway, key_text).await,
        reqwest::StatusCode::OK,
        "the real binding must authenticate before expiry"
    );
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let remaining = expires.signed_duration_since(chrono::Utc::now());
            match remaining.to_std() {
                Ok(remaining) if !remaining.is_zero() => {
                    tokio::time::sleep(remaining.min(Duration::from_secs(2))).await
                }
                _ => break,
            }
        }
    })
    .await
    .expect("the test's real UTC expiry condition must eventually be reached");
    assert_eq!(
        broker_self(&gateway, key_text).await,
        reqwest::StatusCode::UNAUTHORIZED,
        "the unchanged private binding must be refused after its actual expiry"
    );
    let head = gateway.store.head().unwrap();
    let posts = gateway.posts.load(Ordering::SeqCst);
    let replayed_output = run(
        operation_command("resume", &operation, Some(&control)),
        root,
    )
    .await;
    no_secret(&replayed_output, key_text);
    let replayed = success(&replayed_output);
    assert_eq!(replayed["status"], "operation_recorded");
    assert_eq!(replayed["current_state"], "not_observed");
    assert_eq!(replayed["receipt"], first["receipt"]);
    assert_eq!(gateway.posts.load(Ordering::SeqCst), posts + 1);
    assert_eq!(
        gateway.store.head().unwrap(),
        head,
        "historical replay adds no event after expiry"
    );
    assert!(
        fs::read(operation.join("credential")).unwrap() == key,
        "replay must preserve its expired private key"
    );
    assert!(
        fs::read(operation.join("recovery.json")).unwrap() == request,
        "replay must preserve its exact request and expiry"
    );
    let retained: Value = serde_json::from_slice(&request).unwrap();
    assert_eq!(retained["request"]["expires_at"], expiry);
}

#[tokio::test]
async fn cas_conflict_and_real_audit_capacity_refusal_do_not_retry_or_publish_receipts() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let original = root.join("original");
    let gateway = Gateway::new(root, original.clone()).await;
    let control = gateway.control_file(root, "control", RUNNER, "tenant-a");
    prepare_named(root, "original", &issue_manifest(), &control).await;
    let first = success(&run(operation_command("apply", &original, Some(&control)), root).await);
    let conflicting = prepare_named(root, "conflicting", &json!({
        "version":1,"action":"rotate","credential_id":first["credential_id"],"expected_revision":9,
        "subject":issue_manifest()["subject"],"expires_at":"2100-01-01T00:00:00.000Z"
    }), &control).await;
    let before = gateway.store.head().unwrap();
    let neighbor_before = gateway.neighbor.head().unwrap();
    let posts = gateway.posts.load(Ordering::SeqCst);
    let refused = run(
        operation_command("apply", &conflicting, Some(&control)),
        root,
    )
    .await;
    assert_eq!(refused.status.code(), Some(1));
    assert!(refused.stdout.is_empty());
    assert!(
        std::str::from_utf8(&refused.stderr)
            .unwrap()
            .contains("conflicts with durable state"),
        "the real server CAS conflict must retain its closed error category"
    );
    no_secret(&refused, RUNNER);
    assert!(!conflicting.join("receipt").exists());
    assert_eq!(
        gateway.posts.load(Ordering::SeqCst),
        posts + 1,
        "CAS refusal must not retry"
    );
    assert_eq!(gateway.store.head().unwrap(), before);
    assert_eq!(gateway.neighbor.head().unwrap(), neighbor_before);

    let capacity = prepare_named(root, "capacity", &issue_manifest(), &control).await;
    let integrity = gateway.store.integrity().unwrap();
    assert!(integrity.valid);
    assert_eq!(integrity.event_count, u64::try_from(before).unwrap());
    // This closes every owner and reopens the same SQLite files with their
    // actual event count as the admission limit. No raw SQL, deletion, disk
    // filling, forced SQLite error or synthetic business response is involved.
    let gateway = gateway
        .reopen_with_capacity(root, original, integrity.event_count)
        .await;
    let protocol = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{}api/v1/control/protocol", gateway.url))
        .bearer_auth(RUNNER)
        .send()
        .await
        .unwrap();
    assert_eq!(
        protocol.status(),
        reqwest::StatusCode::OK,
        "identity remains readable at full audit capacity"
    );
    let protocol: Value = protocol.json().await.unwrap();
    assert_eq!(protocol["role"], "runner");
    assert_eq!(protocol["tenant_id"], "tenant-a");
    let posts = gateway.posts.load(Ordering::SeqCst);
    let refused = run(operation_command("apply", &capacity, Some(&control)), root).await;
    assert_eq!(refused.status.code(), Some(1));
    assert!(refused.stdout.is_empty());
    assert!(
        std::str::from_utf8(&refused.stderr)
            .unwrap()
            .contains("mandatory storage is unavailable"),
        "physical event admission exhaustion has the gateway's opaque unavailable category"
    );
    no_secret(&refused, RUNNER);
    assert!(!capacity.join("receipt").exists());
    assert_eq!(
        gateway.posts.load(Ordering::SeqCst),
        posts + 1,
        "audit refusal must not retry"
    );
    assert_eq!(
        gateway.store.head().unwrap(),
        before,
        "unrecordable binding must not mutate audit"
    );
    assert_eq!(gateway.neighbor.head().unwrap(), neighbor_before);
    let after = gateway.store.integrity().unwrap();
    assert!(
        after.valid,
        "the real failed transaction must leave projections and audit consistent"
    );
    assert_eq!(after.event_count, integrity.event_count);
    assert_eq!(after.head_hash, integrity.head_hash);
}

#[tokio::test]
async fn prepare_persists_a_private_broker_key_and_exact_request_without_http() {
    let directory = support::private_tempdir();
    let root = directory.path();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let gateway = format!("http://{}/", listener.local_addr().unwrap());
    let runner = root.join("runner");
    private_write(&runner, format!("{RUNNER}\n").as_bytes());
    let control = root.join("control.json");
    private_write(
        &control,
        &serde_json::to_vec(&json!({
            "version": 1,
            "gateway": gateway,
            "expected_tenant_id": "tenant-a",
            "control_namespace_uuid": NAMESPACE,
            "runner_token_file": runner,
            "ca_bundle_file": null
        }))
        .unwrap(),
    );
    let manifest = root.join("manifest.json");
    private_write(
        &manifest,
        &serde_json::to_vec(&json!({
            "version": 1,
            "action": "issue",
            "expected_revision": 0,
            "subject": {
                "kind": "broker", "broker_id": BROKER, "incarnation_id": INCARNATION
            },
            "expires_at": "2100-01-01T00:00:00.000Z"
        }))
        .unwrap(),
    );
    let operation = root.join("operation");
    let mut prepare = command();
    prepare
        .args(["workload", "prepare", "--input"])
        .arg(&manifest)
        .arg("--control-file")
        .arg(&control)
        .arg("--output-dir")
        .arg(&operation);
    let output = run(prepare, root).await;
    no_secret(&output, RUNNER);
    assert!(
        output.status.success(),
        "workload prepare must expose the durable local provisioning route; exit={:?}",
        output.status.code()
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["status"], "prepared");
    assert_eq!(report["current_state"], "not_observed");
    assert_eq!(
        fs::metadata(&operation).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for name in ["credential", "recovery.json"] {
        assert_eq!(
            fs::metadata(operation.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "durable private preparation permissions"
        );
    }
    let key = fs::read_to_string(operation.join("credential")).unwrap();
    let key = key.strip_suffix('\n').unwrap_or(&key);
    assert_eq!(key.len(), 70);
    assert!(key.starts_with("aigw1_"));
    assert!(
        key[6..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert!(key != RUNNER, "workload and runner keys remain separate");
    no_secret(&output, key);
    let recovery = fs::read(operation.join("recovery.json")).unwrap();
    assert!(
        !recovery
            .windows(key.len())
            .any(|bytes| bytes == key.as_bytes())
    );
    let recovery: Value = serde_json::from_slice(&recovery).unwrap();
    assert_eq!(recovery["expected_tenant_id"], "tenant-a");
    assert_eq!(recovery["control_namespace_uuid"], NAMESPACE);
    assert_eq!(recovery["request"]["expected_revision"], 0);
    assert_eq!(recovery["request"]["subject"]["broker_id"], BROKER);
    assert_eq!(
        recovery["request"]["subject"]["incarnation_id"],
        INCARNATION
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "prepare must not authenticate or connect to its configured gateway"
    );
}

#[tokio::test]
async fn prepare_persists_a_broker_grant_without_http_or_credential_generation() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let operation = root.join("broker-grant");
    let gateway = Gateway::new(root, operation.clone()).await;
    let control = gateway.control_file(root, "control", RUNNER, "tenant-a");
    let before = gateway.store.head().unwrap();
    let neighbor_before = gateway.neighbor.head().unwrap();
    let input = root.join("broker-grant.json");
    private_write(
        &input,
        &serde_json::to_vec(&json!({
            "version": 1,
            "action": "broker_grant",
            "broker_id": BROKER,
            "expected_revision": null,
            "credential_id": "44444444-4444-4444-8444-444444444444",
            "incarnation_id": INCARNATION,
            "authorities": [{"authority_id":"local-prod","runtime_kind":"docker"}],
            "lease_seconds": 60,
            "not_after": "2100-01-01T00:00:00.000Z"
        }))
        .unwrap(),
    );

    let prepared = run(prepare_command(&input, &control, &operation), root).await;
    let report = success(&prepared);
    assert_eq!(report["status"], "prepared");
    assert_eq!(report["action"], "broker_grant");
    assert_eq!(
        report["credential_id"],
        "44444444-4444-4444-8444-444444444444"
    );
    let operation_id = report["operation_id"]
        .as_str()
        .expect("prepared grant reports its retained operation ID");
    let parsed_operation_id = Uuid::parse_str(operation_id).unwrap();
    assert!(!parsed_operation_id.is_nil());
    assert_eq!(parsed_operation_id.to_string(), operation_id);

    assert!(!operation.join("credential").exists());
    assert_eq!(fs::read_dir(&operation).unwrap().count(), 1);
    assert_eq!(gateway.requests.load(Ordering::SeqCst), 0);
    assert_eq!(gateway.store.head().unwrap(), before);
    assert_eq!(gateway.neighbor.head().unwrap(), neighbor_before);

    let recovery: Value =
        serde_json::from_slice(&fs::read(operation.join("recovery.json")).unwrap()).unwrap();
    let request = recovery["request"].clone();
    let prepared_request: BrokerGrantMutationV1 = serde_json::from_value(request.clone()).unwrap();
    prepared_request.validate().unwrap();
    assert_eq!(prepared_request.operation_id, operation_id);
    assert_eq!(prepared_request.broker_id, BROKER);
    assert_eq!(prepared_request.expected_revision, None);
    assert_eq!(
        prepared_request.credential_id,
        "44444444-4444-4444-8444-444444444444"
    );
    assert_eq!(prepared_request.incarnation_id, INCARNATION);
    assert_eq!(prepared_request.lease_seconds, 60);
    assert_eq!(prepared_request.not_after, "2100-01-01T00:00:00.000Z");
    assert_eq!(prepared_request.authorities.len(), 1);
    assert_eq!(prepared_request.authorities[0].authority_id, "local-prod");
    assert_eq!(
        prepared_request.authorities[0].runtime_kind,
        BrokerRuntimeKindV1::Docker
    );
    assert_eq!(
        recovery["request_sha256"],
        hex::encode(Sha256::digest(
            serde_json::to_vec(&prepared_request).unwrap()
        ))
    );

    let status = success(&run(operation_command("status", &operation, None), root).await);
    assert_eq!(status["status"], "prepared");
    assert_eq!(status["operation_id"], operation_id);
    assert_eq!(gateway.requests.load(Ordering::SeqCst), 0);
    assert_eq!(gateway.store.head().unwrap(), before);
    assert_eq!(gateway.neighbor.head().unwrap(), neighbor_before);
}

#[tokio::test]
async fn broker_grant_rejects_inconsistent_initial_receipts_without_publishing_them() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let mut accepted_invalid_receipts = Vec::new();

    for (mode, label) in [
        (7, "grant_time_after_expiry"),
        (8, "initial_grant_with_heartbeat_history"),
    ] {
        let directory = support::private_tempdir();
        let root = directory.path();
        let operation = root.join("broker-grant");
        let gateway = Gateway::new(root, operation.clone()).await;
        let control = gateway.control_file(root, "control", RUNNER, "tenant-a");
        let (credential_id, broker_key) =
            issue_broker_binding(root, &control, "initial", INCARNATION).await;
        let grant = prepare_named(
            root,
            "broker-grant",
            &broker_grant_manifest(
                &credential_id,
                INCARNATION,
                None,
                "2100-01-01T00:00:00.000Z",
            ),
            &control,
        )
        .await;
        assert!(!grant.join("credential").exists());
        let head_before_grant = gateway.store.head().unwrap();
        let neighbor_before = gateway.neighbor.head().unwrap();
        let posts_before_grant = gateway.posts.load(Ordering::SeqCst);
        gateway.preparation_missing.store(false, Ordering::SeqCst);
        gateway.receipt_corruption.store(mode, Ordering::SeqCst);

        let attempted = run(operation_command("apply", &grant, Some(&control)), root).await;
        no_secret(&attempted, RUNNER);
        no_secret(&attempted, &broker_key);
        let receipt_published = grant.join("receipt").exists();
        if attempted.status.success() || receipt_published {
            accepted_invalid_receipts.push(label);
        }
        assert_eq!(gateway.posts.load(Ordering::SeqCst), posts_before_grant + 1);
        assert_eq!(gateway.store.head().unwrap(), head_before_grant + 1);
        assert_eq!(gateway.neighbor.head().unwrap(), neighbor_before);
        assert!(!gateway.preparation_missing.load(Ordering::SeqCst));

        let events = gateway
            .store
            .events(&EventQuery {
                after: Some(head_before_grant),
                limit: Some(2),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(events.len(), 1, "the real gateway committed the grant once");
        assert_eq!(events[0].event.kind, "control.broker_registry.grant");

        if !attempted.status.success() && !receipt_published {
            let recovered_output =
                run(operation_command("resume", &grant, Some(&control)), root).await;
            no_secret(&recovered_output, &broker_key);
            let recovered = success(&recovered_output);
            assert_eq!(recovered["status"], "operation_recorded");
            assert!(recovered["receipt"]["audit_seq"].is_number());
            assert_eq!(gateway.store.head().unwrap(), head_before_grant + 1);
            assert_eq!(gateway.posts.load(Ordering::SeqCst), posts_before_grant + 2);
        }
    }

    assert!(
        accepted_invalid_receipts.is_empty(),
        "broker grant accepted inconsistent initial response categories: {accepted_invalid_receipts:?}"
    );
}

#[tokio::test]
async fn broker_operation_preparation_requires_closed_explicit_cas_fields() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let gateway = Gateway::new(root, root.join("unused-operation")).await;
    let control = gateway.control_file(root, "control", RUNNER, "tenant-a");
    let head = gateway.store.head().unwrap();
    let neighbor_head = gateway.neighbor.head().unwrap();

    let mut missing_grant_revision = broker_grant_manifest(
        "44444444-4444-4444-8444-444444444444",
        INCARNATION,
        None,
        "2100-01-01T00:00:00.000Z",
    );
    missing_grant_revision
        .as_object_mut()
        .unwrap()
        .remove("expected_revision");
    let zero_grant_revision = broker_grant_manifest(
        "44444444-4444-4444-8444-444444444444",
        INCARNATION,
        Some(0),
        "2100-01-01T00:00:00.000Z",
    );
    let mut unknown_grant_field = broker_grant_manifest(
        "44444444-4444-4444-8444-444444444444",
        INCARNATION,
        None,
        "2100-01-01T00:00:00.000Z",
    );
    unknown_grant_field["tenant_id"] = json!("tenant-b");
    let mut missing_revoke_revision = broker_revoke_manifest(1);
    missing_revoke_revision
        .as_object_mut()
        .unwrap()
        .remove("expected_revision");
    let zero_revoke_revision = broker_revoke_manifest(0);

    for (name, value) in [
        ("missing-grant-cas", missing_grant_revision),
        ("zero-grant-cas", zero_grant_revision),
        ("unknown-grant-field", unknown_grant_field),
        ("missing-revoke-cas", missing_revoke_revision),
        ("zero-revoke-cas", zero_revoke_revision),
    ] {
        let input = root.join(format!("{name}.json"));
        private_write(&input, &serde_json::to_vec(&value).unwrap());
        let operation = root.join(name);
        let refused = run(prepare_command(&input, &control, &operation), root).await;
        assert_eq!(refused.status.code(), Some(1), "{name} must be refused");
        assert!(refused.stdout.is_empty());
        no_secret(&refused, RUNNER);
        assert!(
            !operation.exists(),
            "{name} must not create preparation state"
        );
    }

    assert_eq!(gateway.requests.load(Ordering::SeqCst), 0);
    assert_eq!(gateway.store.head().unwrap(), head);
    assert_eq!(gateway.neighbor.head().unwrap(), neighbor_head);
}

#[tokio::test]
async fn broker_grant_wrong_authenticated_scope_stops_before_post() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let operation = root.join("broker-grant");
    let gateway = Gateway::new(root, operation.clone()).await;
    let control = gateway.control_file(root, "control", RUNNER, "tenant-a");
    let (credential_id, broker_key) =
        issue_broker_binding(root, &control, "initial", INCARNATION).await;
    let grant = prepare_broker_grant(
        root,
        &control,
        "broker-grant",
        &credential_id,
        INCARNATION,
        None,
        "2100-01-01T00:00:00.000Z",
    )
    .await;
    let wrong_control = gateway.control_file(
        root,
        "neighbor-control",
        "public-workload-cli-neighbor-runner-0000000000000000",
        "tenant-a",
    );
    let head = gateway.store.head().unwrap();
    let neighbor_head = gateway.neighbor.head().unwrap();
    let posts = gateway.posts.load(Ordering::SeqCst);
    gateway.preparation_missing.store(false, Ordering::SeqCst);

    let refused = run(
        operation_command("apply", &grant, Some(&wrong_control)),
        root,
    )
    .await;
    assert_eq!(refused.status.code(), Some(1));
    assert!(refused.stdout.is_empty());
    no_secret(&refused, RUNNER);
    no_secret(&refused, &broker_key);
    assert_eq!(gateway.posts.load(Ordering::SeqCst), posts);
    assert_eq!(gateway.store.head().unwrap(), head);
    assert_eq!(gateway.neighbor.head().unwrap(), neighbor_head);
    assert!(!grant.join("receipt").exists());

    let applied = success(&run(operation_command("apply", &grant, Some(&control)), root).await);
    assert_eq!(applied["status"], "operation_recorded");
    assert_eq!(applied["receipt"]["broker_id"], BROKER);
    assert_eq!(applied["receipt"]["grant_revision"], 1);
    assert_eq!(gateway.posts.load(Ordering::SeqCst), posts + 1);
    assert_eq!(gateway.store.head().unwrap(), head + 1);
    assert_eq!(gateway.neighbor.head().unwrap(), neighbor_head);
    assert_eq!(
        broker_self(&gateway, &broker_key).await,
        reqwest::StatusCode::OK
    );
}

#[tokio::test]
async fn broker_grant_and_revoke_cas_divergence_do_not_mutate_the_registry() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let first_grant = root.join("initial-grant");
    let gateway = Gateway::new(root, first_grant.clone()).await;
    let control = gateway.control_file(root, "control", RUNNER, "tenant-a");
    let (first_credential, first_key) =
        issue_broker_binding(root, &control, "initial", INCARNATION).await;
    let grant = prepare_broker_grant(
        root,
        &control,
        "initial-grant",
        &first_credential,
        INCARNATION,
        None,
        "2100-01-01T00:00:00.000Z",
    )
    .await;
    success(&run(operation_command("apply", &grant, Some(&control)), root).await);
    assert_eq!(
        broker_self(&gateway, &first_key).await,
        reqwest::StatusCode::OK
    );

    let next_incarnation = "55555555-5555-4555-8555-555555555555";
    let (next_credential, next_key) =
        issue_broker_binding(root, &control, "replacement", next_incarnation).await;
    let head = gateway.store.head().unwrap();
    let neighbor_head = gateway.neighbor.head().unwrap();
    let posts = gateway.posts.load(Ordering::SeqCst);

    let stale_grant = prepare_broker_grant(
        root,
        &control,
        "stale-grant",
        &next_credential,
        next_incarnation,
        Some(2),
        "2100-01-01T00:00:00.000Z",
    )
    .await;
    assert!(!stale_grant.join("credential").exists());
    let stale_grant_result = run(
        operation_command("apply", &stale_grant, Some(&control)),
        root,
    )
    .await;
    assert_eq!(stale_grant_result.status.code(), Some(1));
    assert!(stale_grant_result.stdout.is_empty());
    no_secret(&stale_grant_result, RUNNER);
    no_secret(&stale_grant_result, &next_key);
    assert!(!stale_grant.join("receipt").exists());
    assert_eq!(gateway.store.head().unwrap(), head);
    assert_eq!(gateway.neighbor.head().unwrap(), neighbor_head);
    assert_eq!(gateway.posts.load(Ordering::SeqCst), posts + 1);

    let stale_revoke =
        prepare_named(root, "stale-revoke", &broker_revoke_manifest(2), &control).await;
    assert!(!stale_revoke.join("credential").exists());
    let stale_revoke_result = run(
        operation_command("apply", &stale_revoke, Some(&control)),
        root,
    )
    .await;
    assert_eq!(stale_revoke_result.status.code(), Some(1));
    assert!(stale_revoke_result.stdout.is_empty());
    no_secret(&stale_revoke_result, RUNNER);
    no_secret(&stale_revoke_result, &next_key);
    assert!(!stale_revoke.join("receipt").exists());
    assert_eq!(gateway.store.head().unwrap(), head);
    assert_eq!(gateway.neighbor.head().unwrap(), neighbor_head);
    assert_eq!(gateway.posts.load(Ordering::SeqCst), posts + 2);

    let current = gateway.store.broker(BROKER).unwrap().unwrap();
    assert_eq!(current.broker.grant_revision, 1);
    assert_eq!(current.broker.credential_id, first_credential);
    assert!(!current.broker.revoked);
    assert_eq!(
        broker_self(&gateway, &first_key).await,
        reqwest::StatusCode::OK
    );
    assert_eq!(
        broker_self(&gateway, &next_key).await,
        reqwest::StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn broker_grant_lost_ack_resumes_the_same_receipt_after_expiry() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let operation = root.join("broker-grant");
    let gateway = Gateway::new(root, operation.clone()).await;
    let control = gateway.control_file(root, "control", RUNNER, "tenant-a");
    let (credential_id, broker_key) =
        issue_broker_binding(root, &control, "initial", INCARNATION).await;
    let not_after = (chrono::Utc::now() + chrono::Duration::seconds(90))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let grant = prepare_broker_grant(
        root,
        &control,
        "broker-grant",
        &credential_id,
        INCARNATION,
        None,
        &not_after,
    )
    .await;
    assert!(!grant.join("credential").exists());
    gateway.preparation_missing.store(false, Ordering::SeqCst);
    let request: Value =
        serde_json::from_slice(&fs::read(grant.join("recovery.json")).unwrap()).unwrap();
    let operation_id = request["request"]["operation_id"].as_str().unwrap();
    let head_before = gateway.store.head().unwrap();
    let neighbor_before = gateway.neighbor.head().unwrap();
    let posts_before = gateway.posts.load(Ordering::SeqCst);
    gateway
        .drop_next_post_response
        .store(true, Ordering::SeqCst);

    let lost = run(operation_command("apply", &grant, Some(&control)), root).await;
    assert_eq!(lost.status.code(), Some(1));
    assert!(lost.stdout.is_empty());
    no_secret(&lost, RUNNER);
    no_secret(&lost, &broker_key);
    assert!(!grant.join("receipt").exists());
    assert_eq!(gateway.posts.load(Ordering::SeqCst), posts_before + 1);
    assert_eq!(gateway.store.head().unwrap(), head_before + 1);
    assert_eq!(gateway.neighbor.head().unwrap(), neighbor_before);

    tokio::time::timeout(Duration::from_secs(105), async {
        loop {
            let broker = gateway.store.broker(BROKER).unwrap().unwrap();
            if broker.state == BrokerStateV1::Expired {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("the real grant reaches its bounded expiry");
    assert_eq!(
        broker_self(&gateway, &broker_key).await,
        reqwest::StatusCode::FORBIDDEN
    );

    let resumed_output = run(operation_command("resume", &grant, Some(&control)), root).await;
    no_secret(&resumed_output, &broker_key);
    let resumed = success(&resumed_output);
    assert_eq!(resumed["status"], "operation_recorded");
    assert_eq!(resumed["current_state"], "not_observed");
    assert_eq!(resumed["receipt"]["operation_id"], operation_id);
    assert_eq!(resumed["receipt"]["broker_id"], BROKER);
    assert_eq!(resumed["receipt"]["grant_revision"], 1);
    assert_eq!(resumed["receipt"]["heartbeat_revision"], 0);
    assert_eq!(gateway.store.head().unwrap(), head_before + 1);
    assert_eq!(gateway.neighbor.head().unwrap(), neighbor_before);
    assert_eq!(gateway.posts.load(Ordering::SeqCst), posts_before + 2);

    let events = gateway
        .store
        .events(&EventQuery {
            after: Some(head_before),
            limit: Some(2),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.kind, "control.broker_registry.grant");
    assert_eq!(resumed["receipt"]["audit_seq"], events[0].seq);
    assert_eq!(resumed["receipt"]["audit_hash"], events[0].hash);
    assert_eq!(
        resumed["receipt"]["input_hash"],
        events[0].event.data["input_hash"]
    );
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains(&broker_key)
    );

    let requests_before_status = gateway.requests.load(Ordering::SeqCst);
    let status_output = run(operation_command("status", &grant, None), root).await;
    no_secret(&status_output, &broker_key);
    let status = success(&status_output);
    assert_eq!(status["receipt"], resumed["receipt"]);
    assert_eq!(status["current_state"], "not_observed");
    assert_eq!(
        gateway.requests.load(Ordering::SeqCst),
        requests_before_status
    );
}

#[tokio::test]
async fn broker_revoke_stops_broker_authentication_and_preserves_historical_receipts() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let grant_operation = root.join("grant");
    let gateway = Gateway::new(root, grant_operation.clone()).await;
    let control = gateway.control_file(root, "control", RUNNER, "tenant-a");
    let (credential_id, broker_key) =
        issue_broker_binding(root, &control, "initial", INCARNATION).await;
    let grant = prepare_broker_grant(
        root,
        &control,
        "grant",
        &credential_id,
        INCARNATION,
        None,
        "2100-01-01T00:00:00.000Z",
    )
    .await;
    let grant_output = run(operation_command("apply", &grant, Some(&control)), root).await;
    no_secret(&grant_output, &broker_key);
    let grant_report = success(&grant_output);
    assert_eq!(grant_report["receipt"]["grant_revision"], 1);
    assert_eq!(
        broker_self(&gateway, &broker_key).await,
        reqwest::StatusCode::OK
    );

    let revoke = prepare_named(root, "revoke", &broker_revoke_manifest(1), &control).await;
    assert!(!revoke.join("credential").exists());
    let before_revoke = gateway.store.head().unwrap();
    let neighbor_before = gateway.neighbor.head().unwrap();
    let posts_before = gateway.posts.load(Ordering::SeqCst);
    let revoked_output = run(operation_command("apply", &revoke, Some(&control)), root).await;
    no_secret(&revoked_output, &broker_key);
    let revoked = success(&revoked_output);
    assert_eq!(revoked["credential_id"], Value::Null);
    assert_eq!(revoked["broker_id"], BROKER);
    assert_eq!(revoked["receipt"]["grant_revision"], 2);
    assert_eq!(revoked["current_state"], "not_observed");
    assert_eq!(gateway.store.head().unwrap(), before_revoke + 1);
    assert_eq!(gateway.neighbor.head().unwrap(), neighbor_before);
    assert_eq!(gateway.posts.load(Ordering::SeqCst), posts_before + 1);

    let current = gateway.store.broker(BROKER).unwrap().unwrap();
    assert!(current.broker.revoked);
    assert_eq!(current.broker.grant_revision, 2);
    assert_eq!(current.state, BrokerStateV1::Revoked);
    assert_eq!(
        broker_self(&gateway, &broker_key).await,
        reqwest::StatusCode::FORBIDDEN
    );

    let replay_output = run(operation_command("resume", &revoke, Some(&control)), root).await;
    no_secret(&replay_output, &broker_key);
    let replay = success(&replay_output);
    assert_eq!(replay["receipt"], revoked["receipt"]);
    assert_eq!(gateway.store.head().unwrap(), before_revoke + 1);
    assert_eq!(gateway.posts.load(Ordering::SeqCst), posts_before + 2);
    assert_eq!(
        broker_self(&gateway, &broker_key).await,
        reqwest::StatusCode::FORBIDDEN
    );

    let requests_before_status = gateway.requests.load(Ordering::SeqCst);
    let historical_output = run(operation_command("status", &grant, None), root).await;
    no_secret(&historical_output, &broker_key);
    let historical = success(&historical_output);
    assert_eq!(historical["receipt"], grant_report["receipt"]);
    assert_eq!(historical["current_state"], "not_observed");
    assert_eq!(
        gateway.requests.load(Ordering::SeqCst),
        requests_before_status
    );
}

#[tokio::test]
async fn broker_regrant_preserves_heartbeat_history_and_normalizes_expiry_precision() {
    let _serial = GATEWAY_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let root = directory.path();
    let grant_operation = root.join("first-grant");
    let gateway = Gateway::new(root, grant_operation.clone()).await;
    let control = gateway.control_file(root, "control", RUNNER, "tenant-a");
    let (first_credential, first_key) =
        issue_broker_binding(root, &control, "initial", INCARNATION).await;
    let initial_grant = prepare_broker_grant(
        root,
        &control,
        "first-grant",
        &first_credential,
        INCARNATION,
        None,
        "2100-01-01T00:00:00.000Z",
    )
    .await;
    let initial_output = run(
        operation_command("apply", &initial_grant, Some(&control)),
        root,
    )
    .await;
    no_secret(&initial_output, &first_key);
    success(&initial_output);
    let heartbeat = heartbeat_broker(&gateway, &first_key, INCARNATION).await;
    assert_eq!(heartbeat["replayed"], false);
    let before_regrant = gateway.store.broker(BROKER).unwrap().unwrap();
    assert_eq!(before_regrant.broker.heartbeat_revision, 1);
    let old_heartbeat = before_regrant.broker.last_heartbeat.clone().unwrap();
    assert_eq!(old_heartbeat.incarnation_id, INCARNATION);

    let next_incarnation = "55555555-5555-4555-8555-555555555555";
    let (next_credential, next_key) =
        issue_broker_binding(root, &control, "replacement", next_incarnation).await;
    let regrant = prepare_broker_grant(
        root,
        &control,
        "regrant",
        &next_credential,
        next_incarnation,
        Some(1),
        "2100-01-01T00:00:00.123456Z",
    )
    .await;
    assert!(!regrant.join("credential").exists());
    let regrant_output = run(operation_command("apply", &regrant, Some(&control)), root).await;
    no_secret(&regrant_output, &next_key);
    let report = success(&regrant_output);
    assert_eq!(report["receipt"]["grant_revision"], 2);
    assert_eq!(report["receipt"]["heartbeat_revision"], 1);
    assert_eq!(
        broker_self(&gateway, &next_key).await,
        reqwest::StatusCode::OK
    );
    assert_eq!(
        broker_self(&gateway, &first_key).await,
        reqwest::StatusCode::UNAUTHORIZED
    );

    let current = gateway.store.broker(BROKER).unwrap().unwrap();
    assert_eq!(current.broker.grant_revision, 2);
    assert_eq!(current.broker.credential_id, next_credential);
    assert_eq!(current.broker.incarnation_id, next_incarnation);
    assert_eq!(current.broker.heartbeat_revision, 1);
    assert_eq!(current.broker.last_heartbeat, Some(old_heartbeat.clone()));
    assert_eq!(current.state, BrokerStateV1::Unseen);
    assert_eq!(current.broker.not_after, "2100-01-01T00:00:00.123Z");
    assert_eq!(current.broker.authorities.len(), 2);
}

fn decision_admission(store: &Store) -> AdmissionV1 {
    let session_id = Uuid::new_v4().to_string();
    let request = DecisionRequest {
        request_id: Uuid::new_v4().to_string(),
        profile: "strict".into(),
        command: CommandSpec {
            argv: vec!["/usr/bin/id".into()],
            cwd: "/workspace".into(),
        },
        project_id: None,
        sandbox_id: None,
        agent_id: None,
        session_id: Some(session_id.clone()),
        correlation_id: None,
        trace_id: None,
        human_approved: false,
        approval_hash: None,
        execution_mode: None,
        provider_resolution: None,
    };
    let policy = Policy::default();
    let stored = store
        .decision(
            &request,
            Some((policy::evaluate(&policy, &request), json!(null))),
        )
        .unwrap()
        .unwrap();
    AdmissionV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        receipt: DecisionReceiptV1 {
            version: 1,
            request_id: request.request_id.clone(),
            request_hash: policy::request_hash(&request),
            phase: DecisionPhaseV1::Initial,
            audit_seq: stored.seq,
            audit_hash: stored.hash,
        },
        launch: LaunchSpecV1 {
            version: 1,
            session_id,
            command: request.command.clone(),
            profile: request.profile.clone(),
            runtime_kind: RuntimeKindV1::Docker,
            authority_id: Uuid::new_v4().to_string(),
            image_ref: "worker:fixture".into(),
            image_id: format!("sha256:{HASH_A}"),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1_073_741_824,
            pids_limit: 256,
            timeout_seconds: 600,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: RestartPolicyV1::No,
            workspace_manifest_digest: HASH_B.into(),
            workspace_import_policy_version: 1,
            provider: "generic".into(),
            capture_output: false,
            credential_binding_id: None,
            credential_names: vec![],
            provider_resolution: None,
        },
        decision_request: request,
    }
}

fn supervised_request(admission: AdmissionV1) -> SupervisedAdmissionV1 {
    let compact = Uuid::new_v4().simple().to_string();
    SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: format!("{compact}{compact}"),
        admission,
    }
}

fn admit_supervised(store: &Store, request: &SupervisedAdmissionV1) -> AdmissionResultV1 {
    let policy = Policy::default();
    store
        .admit_supervised(
            request,
            &policy,
            policy::evaluate(&policy, &request.admission.decision_request),
            json!(null),
            &QuotaPolicyV1::default(),
        )
        .unwrap()
}

fn mutation(worker: &WorkerV1, transition: WorkerTransitionV1) -> WorkerMutationV1 {
    WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition,
    }
}

fn inspection(worker: &WorkerV1) -> RuntimeInspectionV1 {
    RuntimeInspectionV1 {
        authority_id: worker.launch.authority_id.clone(),
        runtime_kind: worker.launch.runtime_kind,
        container_id: "d".repeat(64),
        stable_name: worker.attempt.stable_name.clone(),
        volume_name: worker.attempt.volume_name.clone(),
        image_id: worker.launch.image_id.clone(),
        spec_hash: worker.spec_hash.clone(),
        cpu_millis: worker.launch.cpu_millis,
        memory_bytes: worker.launch.memory_bytes,
        pids_limit: worker.launch.pids_limit,
        network: worker.launch.network,
        security_preset: worker.launch.security_preset,
        restart_policy: worker.launch.restart_policy,
    }
}

fn host_clock(ns: u128) -> HostClockSampleV1 {
    HostClockSampleV1 {
        version: 1,
        method: HostClockMethodV1::DarwinMachContinuousV1,
        boot_id: "11111111-1111-4111-8111-111111111111".into(),
        continuous_ns_decimal: ns.to_string(),
        observed_utc: Some("2026-10-30T12:00:00Z".into()),
    }
}

fn worker_arm(worker: &WorkerV1, intent_hash: &str) -> ArmV1 {
    let inspection = worker.attempt.created_inspection.clone().unwrap();
    let start = 100_000_000_000_u128;
    let timeout = worker.launch.timeout_seconds;
    ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        intent_hash: intent_hash.into(),
        subject: ArmSubjectV1::Worker,
        inspection,
        authority_binding_hash: HASH_A.into(),
        configuration_fingerprint_version: 2,
        configuration_fingerprint: HASH_B.into(),
        clock: host_clock(start),
        timeout_seconds: timeout,
        deadline_ns_decimal: (start + u128::from(timeout) * 1_000_000_000).to_string(),
    }
}

fn helper_arm(
    worker: &WorkerV1,
    intent_hash: &str,
    helper_id: Uuid,
    purpose: HelperPurposeV1,
    permission: HelperPermissionV1,
) -> ArmV1 {
    worker
        .launch
        .validate()
        .expect("parent launch must be valid");
    assert_eq!(worker.spec_hash, worker.launch.spec_hash().unwrap());
    let policy = helper_execution_policy_v1(worker, purpose, helper_id).unwrap();
    let start = 100_000_000_000_u128;
    let timeout = 120_u32;
    ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        intent_hash: intent_hash.into(),
        subject: ArmSubjectV1::Helper {
            helper_id: helper_id.to_string(),
            purpose,
            policy_version: 1,
            permission: Box::new(permission),
        },
        inspection: RuntimeInspectionV1 {
            authority_id: policy.authority_id.clone(),
            runtime_kind: policy.runtime_kind,
            container_id: format!("{:064x}", helper_id.as_u128()),
            stable_name: policy.stable_name.clone(),
            volume_name: policy.volume_name.clone(),
            image_id: policy.image_id.clone(),
            spec_hash: policy.spec_hash().unwrap(),
            cpu_millis: policy.cpu_millis,
            memory_bytes: policy.memory_bytes,
            pids_limit: policy.pids_limit,
            network: policy.network,
            security_preset: policy.security_preset,
            restart_policy: policy.restart_policy,
        },
        authority_binding_hash: HASH_A.into(),
        configuration_fingerprint_version: 2,
        configuration_fingerprint: HASH_C.into(),
        clock: host_clock(start),
        timeout_seconds: timeout,
        deadline_ns_decimal: (start + u128::from(timeout) * 1_000_000_000).to_string(),
    }
}

fn helper_claim(_arm: &ArmV1, receipt: &ArmReceiptV1, helper_id: Uuid) -> HelperStartClaimV1 {
    HelperStartClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: helper_id.to_string(),
        emitter_id: Uuid::new_v4().to_string(),
        arm_receipt: receipt.clone(),
        clock_check: host_clock(100_000_000_001),
    }
}

fn helper_observation(
    arm: &ArmV1,
    receipt: &ArmReceiptV1,
    helper_id: Uuid,
    running: Option<bool>,
    exit_code: i64,
    verification: Option<agentsandbox::control::ImportVerificationReportV1>,
) -> HelperObservationV1 {
    HelperObservationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: helper_id.to_string(),
        arm_receipt: receipt.clone(),
        observation: RuntimeObservationV1 {
            inspection: arm.inspection.clone(),
            observed_at: "2026-10-30T12:00:02Z".into(),
            status: if running == Some(true) {
                RuntimeStatusV1::Running
            } else {
                RuntimeStatusV1::Exited
            },
            running,
            started_at: Some("2026-10-30T12:00:00Z".into()),
            finished_at: Some("2026-10-30T12:00:01Z".into()),
            exit_code: Some(exit_code),
            oom_killed: Some(false),
            restart_count: Some(0),
        },
        verification,
    }
}

fn parent_observation(
    worker: &WorkerV1,
    status: RuntimeStatusV1,
    running: bool,
    finished_at: Option<&str>,
    exit_code: Option<i64>,
) -> RuntimeObservationV1 {
    RuntimeObservationV1 {
        inspection: worker.attempt.created_inspection.clone().unwrap(),
        observed_at: "2026-10-30T12:00:02Z".into(),
        status,
        running: Some(running),
        started_at: Some("2026-10-30T12:00:00Z".into()),
        finished_at: finished_at.map(str::to_owned),
        exit_code,
        oom_killed: Some(false),
        restart_count: Some(0),
    }
}

fn running_worker(store: &Store) -> WorkerV1 {
    let request = supervised_request(decision_admission(store));
    let admitted = admit_supervised(store, &request);
    let mut worker = admitted.worker.unwrap();
    worker = store
        .mutate_worker(&mutation(&worker, WorkerTransitionV1::Prepare))
        .unwrap()
        .worker;

    // The real typed import and independent verification gates remain active in this fixture.
    let import_id = Uuid::new_v4();
    let import_arm = helper_arm(
        &worker,
        &request.intent_hash,
        import_id,
        HelperPurposeV1::Import,
        HelperPermissionV1::Admission {
            receipt: admitted.receipt,
        },
    );
    let imported = store.arm_execution(&import_arm).unwrap();
    store
        .claim_helper_start(&helper_claim(&import_arm, &imported.receipt, import_id))
        .unwrap();
    let import_terminal = store
        .observe_helper(&helper_observation(
            &import_arm,
            &imported.receipt,
            import_id,
            Some(false),
            0,
            None,
        ))
        .unwrap();

    let verify_id = Uuid::new_v4();
    let verify_arm = helper_arm(
        &worker,
        &request.intent_hash,
        verify_id,
        HelperPurposeV1::ImportVerification,
        HelperPermissionV1::ImportVerification {
            import_helper_id: import_id.to_string(),
            import_arm: imported.receipt,
            import_terminal,
            manifest_digest: worker.launch.workspace_manifest_digest.clone(),
        },
    );
    let verified = store.arm_execution(&verify_arm).unwrap();
    store
        .claim_helper_start(&helper_claim(&verify_arm, &verified.receipt, verify_id))
        .unwrap();
    store
        .observe_helper(&helper_observation(
            &verify_arm,
            &verified.receipt,
            verify_id,
            Some(false),
            0,
            Some(ImportVerificationReportV1 {
                import_helper_id: import_id.to_string(),
                expected_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
                observed_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
            }),
        ))
        .unwrap();

    worker = store
        .mutate_worker(&mutation(&worker, WorkerTransitionV1::CreateIssued))
        .unwrap()
        .worker;
    let mut observed = inspection(&worker);
    observed.container_id = format!("{:064x}", Uuid::new_v4().as_u128());
    worker = store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::Created {
                inspection: observed,
            },
        ))
        .unwrap()
        .worker;
    let arm = worker_arm(&worker, &request.intent_hash);
    let armed = store.arm_execution(&arm).unwrap();
    worker = store
        .claim_armed_start(&ArmedStartClaimV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            expected_revision: worker.revision,
            emitter_id: Uuid::new_v4().to_string(),
            arm_receipt: armed.receipt,
            clock_check: host_clock(100_000_000_001),
        })
        .unwrap()
        .worker;
    let observation = parent_observation(&worker, RuntimeStatusV1::Running, true, None, None);
    let worker = store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::Observe { observation },
        ))
        .unwrap()
        .worker;
    assert_eq!(worker.state, WorkerStateV1::Running);
    worker
}
