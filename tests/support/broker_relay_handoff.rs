//! Real service/Core/helper composition, software only. The runtime CLI is an
//! explicitly selected std-only witness; its observations are synthetic. No
//! engine, provider, project command or privileged host action is executed.
use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    broker::{
        local_ipc::{EndpointRecordV1, read_frame, write_frame},
        service::{self, RequestedExecutionV1, SubmitV1, encode_submit, protocol::*},
    },
    control::{WorkerMutationV1, WorkerStateV1, WorkerTransitionV1},
    control_fs::Directory,
    operations::initialize_config,
    policy::Policy,
    redaction::Redactor,
    relay::{RelayEndpointV1, RelayGrantV1, RelayReceiptV1, channel::helper},
};
use anyhow::{Context, Result, bail, ensure};
use axum::{
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, UnixListener, UnixStream},
    process::{Child, Command},
    sync::{Mutex, oneshot},
    task::{JoinHandle, JoinSet},
    time::{sleep, timeout},
};
use uuid::Uuid;

pub const BIN: &str = env!("CARGO_BIN_EXE_agentsandbox");
pub const A_RUNNER: &str = "broker-handoff-a-runner-public-fixture-0123456789";
pub const B_RUNNER: &str = "broker-handoff-b-runner-public-fixture-0123456789";
const A_VIEWER: &str = "broker-handoff-a-viewer-public-fixture-0123456789";
const B_VIEWER: &str = "broker-handoff-b-viewer-public-fixture-0123456789";
const IMAGE: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

pub struct Faults {
    pub lose_claim: AtomicBool,
    pub lost: AtomicUsize,
    pub claims: AtomicUsize,
    pub sends: AtomicUsize,
    pub acks: AtomicUsize,
    // The admission test leaves the real supervised reservation route open.
    // Later worker mutations are refused before the gateway can commit them.
    before_import: Option<std::sync::Mutex<Vec<WorkerMutationV1>>>,
}
async fn fault(State(f): State<Arc<Faults>>, request: Request, next: Next) -> Response {
    if request.method() == Method::POST {
        let path = request.uri().path();
        let worker_path = path.strip_prefix("/control").unwrap_or(path);
        if let Some(prepare) = &f.before_import
            && worker_path.starts_with("/api/v1/workers/")
            && worker_path != "/api/v1/workers/supervised-reservations"
        {
            if worker_path.ends_with("/transitions")
                && let Ok(bytes) = to_bytes(request.into_body(), 128 * 1024).await
                && let Ok(mutation) = serde_json::from_slice::<WorkerMutationV1>(&bytes)
                && matches!(&mutation.transition, WorkerTransitionV1::Prepare)
                && let Ok(mut seen) = prepare.lock()
                && seen.len() < 2
            {
                seen.push(mutation);
            }
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        if matches!(
            path,
            "/api/v1/relay/messages" | "/control/api/v1/relay/messages"
        ) {
            f.sends.fetch_add(1, Ordering::SeqCst);
        } else if (path.starts_with("/api/v1/relay/messages/")
            || path.starts_with("/control/api/v1/relay/messages/"))
            && path.ends_with("/ack")
        {
            f.acks.fetch_add(1, Ordering::SeqCst);
        }
    }
    let claim = request.method() == Method::POST
        && request
            .uri()
            .path()
            .starts_with("/control/api/v1/relay/channels/")
        && request.uri().path().ends_with("/claims");
    if claim {
        f.claims.fetch_add(1, Ordering::SeqCst);
    }
    let response = next.run(request).await;
    if claim
        && response.status() == StatusCode::CREATED
        && f.lose_claim.swap(false, Ordering::SeqCst)
    {
        f.lost.fetch_add(1, Ordering::SeqCst);
        let (parts, _) = response.into_parts();
        return Response::from_parts(
            parts,
            Body::from_stream(futures_util::stream::once(async {
                Err::<Vec<u8>, io::Error>(io::Error::from(io::ErrorKind::ConnectionReset))
            })),
        );
    }
    response
}
fn private_dir(parent: &Path, name: &str) -> Result<PathBuf> {
    let path = parent.join(name);
    std::fs::create_dir(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    Ok(path)
}
fn private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn diagnostic(path: &Path) -> Result<Stdio> {
    Ok(Stdio::from(
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)?,
    ))
}

fn fresh(root: &Path, tenant: &str) -> Result<Arc<Store>> {
    let store = Store::create_supervised_for_tenant(
        &root.join(format!("{tenant}.sqlite")),
        tenant,
        Redactor::new(vec![
            A_RUNNER.into(),
            B_RUNNER.into(),
            A_VIEWER.into(),
            B_VIEWER.into(),
        ]),
        10000,
        &root.join(format!("{tenant}-before3.sqlite")),
    )?;
    store.migrate_identity_v4(&root.join(format!("{tenant}-before4.sqlite")))?;
    store.migrate_human_access_v5(&root.join(format!("{tenant}-before5.sqlite")))?;
    store.migrate_workload_v6(&root.join(format!("{tenant}-before6.sqlite")))?;
    Ok(Arc::new(store))
}
async fn start_fixture_enforcer(root: &Path, broker_root: &Path) -> Result<Child> {
    let enforcer_root = private_dir(broker_root, "enforcer")?;
    // The relay fixture retains its existing guardian configuration.
    private_file(&enforcer_root.join("auth"), &[0x51; 32])?;
    let mut enforcer = Command::new(BIN)
        .args(["__broker-enforcer", "--root"])
        .arg(&enforcer_root)
        .args(["--max-slots", "32"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(diagnostic(&root.join("enforcer.stderr"))?)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", root)
        .current_dir(root)
        .kill_on_drop(true)
        .spawn()?;
    timeout(Duration::from_secs(10), async {
        loop {
            if let Some(status) = enforcer.try_wait()? {
                bail!("phase=enforcer_announcement_wait child_status={status}");
            }
            if std::fs::read_dir(enforcer_root.join("announcements"))
                .is_ok_and(|mut entries| entries.next().is_some())
            {
                return Ok::<_, anyhow::Error>(());
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .context("phase=enforcer_announcement_wait timed out")??;
    Ok(enforcer)
}
#[path = "broker_relay_runtime.rs"]
mod runtime;
use runtime::{Engine, EngineFailure, engine_connection};
pub use runtime::{
    RelayAttempt, contradictory_create_flags_are_refused_before_fixture_mutation, exchange,
    relay_attempt, worker_terminal,
};

pub struct Fixture {
    pub root: PathBuf,
    pub a: Arc<Store>,
    pub b: Arc<Store>,
    pub origin: String,
    pub faults: Arc<Faults>,
    pub broker_root: PathBuf,
    pub operations: Vec<PathBuf>,
    broker_tmp: Directory,
    registry: PathBuf,
    runner_a: PathBuf,
    broker: Option<Child>,
    enforcer: Option<Child>,
    engine: Arc<Mutex<Engine>>,
    engine_stop: Option<oneshot::Sender<()>>,
    engine_task: JoinHandle<()>,
    server_stop: Option<oneshot::Sender<()>>,
    server_task: JoinHandle<io::Result<()>>,
}
impl Fixture {
    pub async fn new(worker_timeout: u32) -> Result<Self> {
        Self::open(worker_timeout, false).await
    }
    pub async fn new_codex_admission(worker_timeout: u32) -> Result<Self> {
        Self::open(worker_timeout, true).await
    }
    async fn open(worker_timeout: u32, admission_only: bool) -> Result<Self> {
        let base = std::fs::canonicalize(std::env::temp_dir())?;
        let root = tempfile::Builder::new()
            .prefix("rh-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(base)?
            .keep();
        let a = fresh(root.as_path(), "tenant-a")?;
        let b = fresh(root.as_path(), "tenant-b")?;
        let faults = Arc::new(Faults {
            lose_claim: AtomicBool::new(false),
            lost: AtomicUsize::new(0),
            claims: AtomicUsize::new(0),
            sends: AtomicUsize::new(0),
            acks: AtomicUsize::new(0),
            before_import: admission_only.then(std::sync::Mutex::default),
        });
        let app = gateway_router(GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: a.clone(),
                    runner_token: A_RUNNER.into(),
                    viewer_token: A_VIEWER.into(),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: b.clone(),
                    runner_token: B_RUNNER.into(),
                    viewer_token: B_VIEWER.into(),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )?)
        .layer(middleware::from_fn_with_state(faults.clone(), fault));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let (server_stop, stopped) = oneshot::channel();
        let server_task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        });
        let engine = Arc::new(Mutex::new(Engine {
            next_id: 1,
            containers: BTreeMap::new(),
            volumes: BTreeMap::new(),
            calls: vec![],
            execs: BTreeMap::new(),
            errors: BTreeMap::new(),
            root: root.as_path().to_owned(),
            import_gate: None,
            import_counts: Default::default(),
        }));
        let socket = root.as_path().join("r");
        let listener = UnixListener::bind(&socket)?;
        let (engine_stop, mut stopped) = oneshot::channel();
        let shared = engine.clone();
        let engine_task = tokio::spawn(async move {
            let mut owners = JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = &mut stopped => break,
                    result = owners.join_next(), if !owners.is_empty() => {
                        if let Some(Err(_)) = result {
                            shared.lock().await.note_failure(EngineFailure::OwnerJoin);
                        }
                    }
                    accepted = listener.accept(), if owners.len() < 64 => {
                        match accepted {
                            Ok((stream, _)) => {
                                let state = shared.clone();
                                owners.spawn(async move {
                                    if let Err(error) = engine_connection(stream, state.clone()).await {
                                        let category = error.downcast_ref::<EngineFailure>()
                                            .copied()
                                            .unwrap_or(EngineFailure::RuntimeCommand);
                                        state.lock().await.note_failure(category);
                                    }
                                });
                            }
                            Err(_) => {
                                shared.lock().await.note_failure(EngineFailure::ListenerAccept);
                                break;
                            }
                        }
                    }
                }
            }
            owners.abort_all();
            while owners.join_next().await.is_some() {}
        });
        let source = root.as_path().join("runtime.rs");
        private_file(
            &source,
            include_bytes!("../runtime_fixtures/relay_stdio.rs"),
        )?;
        let program = root.as_path().join("runtime");
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let compile = timeout(
            Duration::from_secs(30),
            Command::new(rustc)
                .args(["--edition=2024", "-o"])
                .arg(&program)
                .arg(&source)
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        ensure!(
            compile.status.success(),
            "compile std-only fixture witness failed"
        );
        private_file(
            &program.with_extension("fixture"),
            socket.as_os_str().as_encoded_bytes(),
        )?;
        let operator = private_dir(root.as_path(), "operator")?;
        let runner_a = operator.join("a.runner");
        let runner_b = operator.join("b.runner");
        private_file(&runner_a, A_RUNNER.as_bytes())?;
        private_file(&runner_b, B_RUNNER.as_bytes())?;
        let sources = private_dir(root.as_path(), "sources")?;
        let exports = private_dir(root.as_path(), "exports")?;
        // The broker clears its environment. Keep its private temporary files
        // outside the imported source even when the QA root filesystem is RO.
        let broker_tmp = Directory::open_existing(&private_dir(root.as_path(), "broker-tmp")?)?;
        let configs = private_dir(root.as_path(), "configs")?;
        let ops = private_dir(root.as_path(), "ops")?;
        let operations = vec![ops.join("a1"), ops.join("a2")];
        let broker_root = private_dir(root.as_path(), "b")?;
        let mut profiles = Vec::new();
        for (id, tenant, store, token) in [
            ("a", "tenant-a", &a, &runner_a),
            ("b", "tenant-b", &b, &runner_b),
        ] {
            let config = private_dir(&configs, id)?;
            initialize_config(&config)?;
            let mut profile = json!({"id":id,"gateway":origin,"tenant_id":tenant,"control_namespace_uuid":store.supervision_state()?.control_namespace_uuid,"config_dir":config,"runner_token_file":token,"runtime":"docker","runtime_program":program,"runtime_endpoint":"unix:///software-relay-fixture.sock","image":IMAGE,"policy_profile":"strict","network":"none","allow_rootful_runtime":false,"max_timeout_seconds":worker_timeout,"allow_capture_output":false,"managed_clients":false,"providers_file":null,"manual_credential_file":null,"source_roots":[sources],"export_roots":[exports],"prearmed_start_profile":{"version":1,"mode":"docker_minimal"}});
            if id == "a" {
                if admission_only {
                    let key = operator.join("synthetic-codex.env");
                    private_file(
                        &key,
                        b"OPENAI_API_KEY=synthetic-broker-admission-test-value\n",
                    )?;
                    let providers = config.join("providers.json");
                    private_file(
                        &providers,
                        &serde_json::to_vec(&json!({
                            "version": 1,
                            "profiles": [{
                                "id": "codex-api", "revision": 1, "client": "codex",
                                "auth_mode": "api", "model": "gpt-6-luna",
                                "credential": { "kind": "api_env_file", "path": key }
                            }]
                        }))?,
                    )?;
                    profile["managed_clients"] = json!(true);
                    profile["providers_file"] = json!(providers);
                    profile["policy_profile"] = json!("observe");
                    profile["network"] = json!("bridge");
                } else {
                    profile["relay"] = json!({"version":1,"workload_operations":operations});
                }
            }
            profiles.push(profile);
        }
        let registry = operator.join("broker.json");
        private_file(
            &registry,
            &serde_json::to_vec(
                &json!({"version":2,"installation_id":Uuid::new_v4(),"root":broker_root,"max_enforcer_slots":32,"profiles":profiles,"additional_control_secret_files":[]}),
            )?,
        )?;
        let enforcer = if admission_only {
            // The actual broker must create and attest the new role itself.
            None
        } else {
            Some(start_fixture_enforcer(root.as_path(), &broker_root).await?)
        };
        let mut fixture = Self {
            root,
            a,
            b,
            origin,
            faults,
            broker_root,
            operations,
            broker_tmp,
            registry,
            runner_a,
            broker: None,
            enforcer,
            engine,
            engine_stop: Some(engine_stop),
            engine_task,
            server_stop: Some(server_stop),
            server_task,
        };
        if let Err(error) = fixture.start_broker().await {
            if admission_only {
                let cleanup = fixture.close().await;
                return Err(error.context(if cleanup.is_ok() {
                    "admission fixture broker did not start"
                } else {
                    "admission fixture broker did not start and cleanup is unconfirmed"
                }));
            }
            return Err(error);
        }
        Ok(fixture)
    }
    pub fn runner_file(&self) -> &Path {
        &self.runner_a
    }
    pub async fn wait_before_import(&self) -> Result<WorkerMutationV1> {
        let boundary = self
            .faults
            .before_import
            .as_ref()
            .context("admission-only fixture required")?;
        timeout(Duration::from_secs(25), async {
            loop {
                if let Some(prepare) = boundary
                    .lock()
                    .map_err(|_| anyhow::anyhow!("admission boundary poisoned"))?
                    .first()
                    .cloned()
                {
                    return Ok::<_, anyhow::Error>(prepare);
                }
                sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .context("broker did not attempt Prepare after admission")?
    }
    async fn assert_admission_engine_untouched(&self) -> Result<()> {
        let engine = self.engine.lock().await;
        ensure!(
            engine.containers.is_empty()
                && engine.volumes.is_empty()
                && engine.execs.is_empty()
                && !engine.calls.iter().flatten().any(|part| {
                    matches!(
                        part.as_str(),
                        "create" | "start" | "exec" | "run" | "kill" | "stop" | "rm"
                    )
                }),
            "admission fixture reached a runtime mutation"
        );
        Ok(())
    }
    pub async fn start_broker(&mut self) -> Result<()> {
        ensure!(self.broker.is_none(), "broker already owned");
        self.broker_tmp.revalidate()?;
        self.broker = Some(
            Command::new(BIN)
                .args(["broker", "serve", "--config"])
                .arg(&self.registry)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(diagnostic(&self.root.join("broker.stderr"))?)
                .env_clear()
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("HOME", self.root.as_path())
                .env("TMPDIR", self.broker_tmp.path())
                .current_dir(self.root.as_path())
                .kill_on_drop(true)
                .spawn()?,
        );
        timeout(Duration::from_secs(10), async {
            loop {
                ensure!(
                    self.broker.as_mut().unwrap().try_wait()?.is_none(),
                    "fixture broker exited before IPC ready"
                );
                if matches!(
                    service::request(
                        &self.broker_root,
                        "a",
                        A_RUNNER,
                        BrokerCommandV1::List {
                            after: None,
                            limit: 1,
                        },
                    )
                    .await,
                    Ok(BrokerReplyV1::Page { .. })
                ) {
                    return Ok::<_, anyhow::Error>(());
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await??;
        Ok(())
    }
    pub async fn restart_broker(&mut self) -> Result<()> {
        if let Some(mut child) = self.broker.take() {
            child.kill().await?;
            child.wait().await?;
        }
        self.start_broker().await
    }
    pub async fn submit(&self, profile: &str, seconds: u32) -> Result<Uuid> {
        let operation_id = Uuid::new_v4();
        let submit = SubmitV1 {
            version: 1,
            operation_id,
            project: self.root.as_path().join("sources"),
            execution: RequestedExecutionV1::Command {
                argv: vec!["/usr/bin/id".into()],
            },
            timeout_seconds: Some(seconds),
            capture_output: false,
        };
        let reply = service::request(
            &self.broker_root,
            profile,
            if profile == "a" { A_RUNNER } else { B_RUNNER },
            BrokerCommandV1::Submit {
                payload: encode_submit(&submit)?,
            },
        )
        .await?;
        ensure!(
            matches!(reply, BrokerReplyV1::Accepted { .. }),
            "real broker did not accept fixture submit"
        );
        Ok(operation_id)
    }
    pub async fn status(&self, profile: &str, job: Uuid) -> Result<RelayStatusV1> {
        match service::request(
            &self.broker_root,
            profile,
            if profile == "a" { A_RUNNER } else { B_RUNNER },
            BrokerCommandV1::RelayStatus { operation_id: job },
        )
        .await?
        {
            BrokerReplyV1::RelayStatus { status } => Ok(status),
            _ => bail!("relay status refused"),
        }
    }
    pub async fn wait_endpoint(&self, job: Uuid) -> Result<RelayEndpointV1> {
        timeout(Duration::from_secs(25), async {
            loop {
                if let Ok(status) = self.status("a", job).await {
                    if status.phase == Some(RelayAttachmentPhaseV1::AwaitingIssuedWorkload) {
                        if let Some(endpoint) = status.endpoint {
                            return Ok(endpoint);
                        }
                    }
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await?
    }
    pub async fn issue(&self, index: usize, endpoint: &RelayEndpointV1) -> Result<String> {
        let manifest = self.root.as_path().join(format!("issue-{index}.json"));
        private_file(
            &manifest,
            &serde_json::to_vec(
                &json!({"action":"issue","version":1,"expected_revision":0,"subject":{"kind":"worker","worker_id":endpoint.worker_id,"attempt_id":endpoint.attempt_id,"generation":1},"expires_at":(chrono::Utc::now()+chrono::Duration::minutes(5)).to_rfc3339()}),
            )?,
        )?;
        let control = self.root.as_path().join(format!("control-{index}.json"));
        private_file(
            &control,
            &serde_json::to_vec(
                &json!({"version":1,"gateway":self.origin,"expected_tenant_id":"tenant-a","control_namespace_uuid":self.a.supervision_state()?.control_namespace_uuid,"runner_token_file":self.runner_a,"ca_bundle_file":null}),
            )?,
        )?;
        for (action, args) in [
            (
                "prepare",
                vec![
                    "--input".to_owned(),
                    manifest.display().to_string(),
                    "--control-file".into(),
                    control.display().to_string(),
                    "--output-dir".into(),
                    self.operations[index].display().to_string(),
                ],
            ),
            (
                "apply",
                vec![
                    "--operation-dir".into(),
                    self.operations[index].display().to_string(),
                    "--control-file".into(),
                    control.display().to_string(),
                ],
            ),
        ] {
            let output = timeout(
                Duration::from_secs(15),
                Command::new(BIN)
                    .args(["workload", action])
                    .args(args)
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                    .env("HOME", self.root.as_path())
                    .current_dir(self.root.as_path())
                    .kill_on_drop(true)
                    .output(),
            )
            .await??;
            ensure!(output.status.success(), "real workload {action} failed");
        }
        let recovery: Value = serde_json::from_slice(&std::fs::read(
            self.operations[index].join("recovery.json"),
        )?)?;
        Ok(recovery["request"]["credential_id"]
            .as_str()
            .context("issued credential ID")?
            .into())
    }
    pub async fn ready(&self, job: Uuid) -> Result<RelayStatusV1> {
        timeout(Duration::from_secs(15), async {
            loop {
                let status = self.status("a", job).await?;
                if status.core_state
                    == Some(agentsandbox::relay::admission::RelayChannelStateV1::Ready)
                {
                    return Ok(status);
                }
                ensure!(
                    status.phase != Some(RelayAttachmentPhaseV1::Closed),
                    "relay closed before Ready"
                );
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await?
    }
    pub async fn exec_count(&self) -> usize {
        self.engine.lock().await.execs.values().sum()
    }
    pub fn committed_claims(&self) -> Result<usize> {
        let db = self.readonly_a()?;
        let mut query = db.prepare("SELECT payload FROM events WHERE kind='relay_channel.v1'")?;
        let mut rows = query.query([])?;
        let mut count = 0;
        while let Some(row) = rows.next()? {
            let payload: String = row.get(0)?;
            let event: Value = serde_json::from_str(&payload)?;
            count += usize::from(event["data"]["input"]["action"] == "claim");
        }
        Ok(count)
    }
    fn readonly_a(&self) -> Result<rusqlite::Connection> {
        let db = rusqlite::Connection::open_with_flags(
            self.root.join("tenant-a.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        db.busy_timeout(Duration::from_secs(2))?;
        Ok(db)
    }
    pub fn relay_effects(&self) -> Result<(Vec<(String, bool)>, Vec<String>)> {
        let db = self.readonly_a()?;
        let mut query =
            db.prepare("SELECT message_id,acknowledged FROM relay_messages ORDER BY message_id")?;
        let messages = query
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut query =
            db.prepare("SELECT operation_id FROM relay_operations ORDER BY operation_id")?;
        let operations = query
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((messages, operations))
    }
    pub fn assert_no_outbox_dispatch(&self, operation: &str) -> Result<()> {
        let directory = self
            .tenant_directory()?
            .join("relay_v1")
            .join(format!("op-{operation}"));
        ensure!(
            !directory.join("dispatch.json").exists() && !directory.join("receipt.json").exists(),
            "revoked request acquired an outbox dispatch/receipt"
        );
        Ok(())
    }
    pub fn tenant_directory(&self) -> Result<PathBuf> {
        let mut entries = std::fs::read_dir(self.broker_root.join("broker/relay"))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<Vec<_>>>()?;
        ensure!(
            entries.len() == 1,
            "two endpoints must share exactly one tenant IO namespace"
        );
        Ok(entries.remove(0))
    }
    pub fn durable_operation(&self, operation: &str) -> Result<Value> {
        let directory = self
            .tenant_directory()?
            .join("relay_v1")
            .join(format!("op-{operation}"));
        for name in ["request.json", "dispatch.json", "receipt.json"] {
            ensure!(
                directory.join(name).is_file(),
                "durable operation is missing {name}"
            );
        }
        Ok(serde_json::from_slice(&std::fs::read(
            directory.join("request.json"),
        )?)?)
    }
    pub async fn wait_worker(&self, profile: &str) -> Result<RelayEndpointV1> {
        let store = if profile == "a" { &self.a } else { &self.b };
        timeout(Duration::from_secs(25), async {
            loop {
                let workers = store.workers()?;
                ensure!(
                    workers.len() <= 1,
                    "this observer requires one worker in the selected fixture tenant"
                );
                if let Some(worker) = workers
                    .into_iter()
                    .find(|worker| worker.state == WorkerStateV1::Running)
                {
                    return Ok(RelayEndpointV1 {
                        worker_id: worker.worker_id,
                        attempt_id: worker.attempt.attempt_id,
                        generation: 1,
                    });
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await?
    }
    pub async fn closed(&self, job: Uuid) -> Result<()> {
        timeout(Duration::from_secs(15), async {
            loop {
                let status = self.status("a", job).await?;
                if status.core_state
                    == Some(agentsandbox::relay::admission::RelayChannelStateV1::Closed)
                {
                    ensure!(
                        status.local_joined == Some(true),
                        "Core Closed without a joined local owner"
                    );
                    return Ok::<_, anyhow::Error>(());
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await??;
        Ok(())
    }
    pub async fn revoke(&self, credential: &str) -> Result<()> {
        let input = agentsandbox::workload::WorkloadBindingRevocationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential.into(),
            expected_revision: 1,
        };
        let receipt: agentsandbox::workload::WorkloadBindingReceiptV1 = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()?
            .post(format!(
                "{}/api/v1/workload-bindings/{credential}/revoke",
                self.origin
            ))
            .bearer_auth(A_RUNNER)
            .json(&input)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        ensure!(receipt.revision == 2, "workload revocation revision");
        Ok(())
    }
    pub async fn helper_connection(
        &self,
        endpoint: &RelayEndpointV1,
    ) -> Result<(UnixStream, String, u64)> {
        let worker = self.a.worker(&endpoint.worker_id)?.context("worker")?;
        let cid = &worker
            .attempt
            .created_inspection
            .as_ref()
            .context("created inspection")?
            .container_id;
        let root = self
            .engine
            .lock()
            .await
            .containers
            .get(cid)
            .context("synthetic helper CID")?
            .helper_root
            .clone();
        timeout(Duration::from_secs(5), async {
            loop {
                for entry in std::fs::read_dir(root.path())? {
                    let entry = entry?;
                    if entry.file_type()?.is_dir() {
                        if let Ok(discovered) =
                            helper::discover(&root, &entry.file_name().to_string_lossy())
                        {
                            let stream = discovered.connect_checked(&root).await?;
                            return Ok::<_, anyhow::Error>((
                                stream,
                                discovered.channel_id().into(),
                                discovered.epoch(),
                            ));
                        }
                    }
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await?
    }
    pub fn latest_fact(&self, job: Uuid) -> Result<Value> {
        let relay = self.broker_root.join("broker/relay");
        for tenant in std::fs::read_dir(relay)? {
            let directory = tenant?.path().join("relay_channels_v1");
            let mut names = std::fs::read_dir(directory)?
                .map(|e| e.map(|e| e.path()))
                .collect::<std::io::Result<Vec<_>>>()?;
            names.sort();
            for path in names.into_iter().rev() {
                if path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("fact-"))
                {
                    let value: Value = serde_json::from_slice(&std::fs::read(path)?)?;
                    if value["fact"]["job_id"] == job.to_string() {
                        return Ok(value["fact"].clone());
                    }
                }
            }
        }
        bail!("no channel fact for job")
    }
    pub async fn cancel(&self, profile: &str, job: Uuid) -> Result<()> {
        let reply = service::request(
            &self.broker_root,
            profile,
            if profile == "a" { A_RUNNER } else { B_RUNNER },
            BrokerCommandV1::Cancel {
                operation_id: job,
                command_id: Uuid::new_v4(),
            },
        )
        .await?;
        ensure!(
            matches!(reply, BrokerReplyV1::CommandAcknowledged { .. }),
            "cancel not durably acknowledged"
        );
        Ok(())
    }
    pub async fn close(mut self) -> Result<()> {
        for child in [&mut self.broker, &mut self.enforcer] {
            if let Some(mut owned) = child.take() {
                if owned.try_wait()?.is_none() {
                    owned.start_kill()?;
                    timeout(Duration::from_secs(5), owned.wait()).await??;
                }
            }
        }
        let guardian = if self.faults.before_import.is_some() {
            timeout(
                Duration::from_secs(15),
                close_admission_guardian(&self.broker_root),
            )
            .await
            .context("admission guardian cleanup was not confirmed")
            .and_then(|result| result)
        } else {
            Ok(())
        };
        if let Some(stop) = self.engine_stop.take() {
            let _ = stop.send(());
        }
        timeout(Duration::from_secs(5), &mut self.engine_task).await??;
        if let Some(stop) = self.server_stop.take() {
            let _ = stop.send(());
        }
        timeout(Duration::from_secs(5), &mut self.server_task).await???;
        let untouched = if self.faults.before_import.is_some() {
            self.assert_admission_engine_untouched().await
        } else {
            Ok(())
        };
        untouched?;
        guardian?;
        ensure!(
            self.engine.lock().await.errors.is_empty(),
            "synthetic runtime fixture errors"
        );
        Ok(())
    }
}
async fn close_admission_guardian(broker_root: &Path) -> Result<()> {
    let role = broker_root.join("enforcer");
    match std::fs::symlink_metadata(&role) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let role = Directory::open_existing(&role)?;
    let announcements = Directory::open_existing(&role.path().join("announcements"))?;
    ensure!(
        std::fs::read_dir(announcements.path())?.count() == 1,
        "fresh admission guardian did not publish exactly one endpoint"
    );
    let publication = announcements.read_private_file("e-0001.json", 4096)?;
    let record: Value = serde_json::from_slice(publication.bytes())?;
    ensure!(
        record["version"] == 1 && record["sequence"] == 1,
        "invalid guardian publication"
    );
    let endpoint: EndpointRecordV1 = serde_json::from_value(record["endpoint"].clone())?;
    let endpoints = Directory::open_existing(&role.path().join("endpoints"))?;
    let capability = role.read_private_file("auth", 32)?;
    ensure!(
        capability.bytes().len() == 32,
        "invalid private fixture capability size"
    );

    // This one request uses only the already published endpoint. It cannot
    // start or reconnect to a replacement process. The guardian enforces idle.
    let wait = Duration::from_secs(3);
    let mut stream = endpoint.connect_checked(&endpoints).await?;
    let challenge: Value = read_frame(&mut stream, 1024, wait).await?;
    let epoch = serde_json::from_value::<Uuid>(challenge["epoch"].clone())?;
    let nonce = serde_json::from_value::<Uuid>(challenge["nonce"].clone())?;
    ensure!(
        epoch == endpoint.epoch() && !nonce.is_nil(),
        "guardian epoch differs from its publication"
    );
    capability.revalidate()?;
    publication.revalidate()?;
    let mut proof = Sha256::new();
    proof.update(b"ai-guard:enforcer-auth:v1\0");
    proof.update(capability.bytes());
    proof.update(epoch.as_bytes());
    proof.update(nonce.as_bytes());
    write_frame(
        &mut stream,
        &json!({"proof": format!("{:x}", proof.finalize())}),
        1024,
        wait,
    )
    .await?;
    write_frame(
        &mut stream,
        &json!({"kind": "shutdown_if_idle"}),
        1024,
        wait,
    )
    .await?;
    let reply: Value = read_frame(&mut stream, 1024, wait).await?;
    ensure!(
        reply == json!({"kind": "acknowledged"}),
        "idle guardian shutdown refused"
    );
    drop(stream);

    let socket = endpoints.path().join(endpoint.directory_name()).join("s");
    timeout(Duration::from_secs(5), async {
        loop {
            match UnixStream::connect(&socket).await {
                Ok(stream) => drop(stream),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                    ) =>
                {
                    endpoints.revalidate()?;
                    publication.revalidate()?;
                    ensure!(
                        std::fs::read_dir(announcements.path())?.count() == 1,
                        "guardian was replaced during shutdown"
                    );
                    return Ok::<_, anyhow::Error>(());
                }
                Err(error) => return Err(error.into()),
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("guardian acknowledged shutdown but its IPC remained reachable")?
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(child) = self.broker.as_mut() {
            let _ = child.start_kill();
        }
        if let Some(child) = self.enforcer.as_mut() {
            let _ = child.start_kill();
        }
        if let Some(stop) = self.engine_stop.take() {
            let _ = stop.send(());
        }
        if let Some(stop) = self.server_stop.take() {
            let _ = stop.send(());
        }
    }
}

pub async fn grant(
    f: &Fixture,
    source: RelayEndpointV1,
    destination: RelayEndpointV1,
) -> Result<RelayReceiptV1> {
    let input = RelayGrantV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: Uuid::new_v4().to_string(),
        source,
        destination,
        channel: "software-fixture".into(),
        expires_at: (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
    };
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?
        .post(format!("{}/api/v1/relay/grants", f.origin))
        .bearer_auth(A_RUNNER)
        .json(&input)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}
