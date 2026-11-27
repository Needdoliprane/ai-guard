//! Real service/Core/helper composition, software only. The runtime CLI is an
//! explicitly selected std-only witness; its observations are synthetic. No
//! engine, provider, project command or privileged host action is executed.
use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    broker::{
        lifecycle::LocalFailurePhaseV1,
        service::{self, RequestedExecutionV1, SubmitV1, encode_submit, protocol::*},
    },
    control::{WorkerMutationV1, WorkerStateV1, WorkerTransitionV1, WorkerV1},
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
#[path = "owned_legacy_guardian.rs"]
mod owned_legacy_guardian;
use owned_legacy_guardian::{OwnedLegacyGuardian, require_fresh_role, require_socket_closed};
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "owned_darwin.rs"]
mod owned_darwin;

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
    legacy_guardian: Option<OwnedLegacyGuardian>,
    legacy_start_attempted: bool,
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
            legacy_guardian: None,
            legacy_start_attempted: false,
            engine,
            engine_stop: Some(engine_stop),
            engine_task,
            server_stop: Some(server_stop),
            server_task,
        };
        if let Err(error) = fixture.start_broker().await {
            return Err(match fixture.close().await {
                Ok(()) => error.context("fixture broker did not start"),
                Err(cleanup) => error.context(format!(
                    "fixture broker did not start and cleanup also failed: {cleanup:#}"
                )),
            });
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
    async fn bind_legacy_guardian(&mut self) -> Result<()> {
        let broker = self.broker.as_mut().context("fixture broker owner")?;
        self.legacy_guardian =
            Some(OwnedLegacyGuardian::capture(&self.broker_root, broker, Path::new(BIN)).await?);
        Ok(())
    }
    pub async fn start_broker(&mut self) -> Result<()> {
        ensure!(self.broker.is_none(), "broker already owned");
        self.broker_tmp.revalidate()?;
        let first = !self.legacy_start_attempted;
        if first {
            require_fresh_role(&self.broker_root)?;
        } else {
            self.legacy_guardian
                .as_ref()
                .context("original fixture Legacy guardian custody unavailable")?
                .require_live()
                .await?;
        }
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
                .kill_on_drop(false)
                .spawn()?,
        );
        self.legacy_start_attempted = true;
        timeout(Duration::from_secs(10), async {
            loop {
                if let Some(status) = self
                    .broker
                    .as_mut()
                    .context("fixture broker owner")?
                    .try_wait()?
                {
                    bail!("phase=broker_ipc_preflight child_status={status}");
                }
                if first
                    && self.legacy_guardian.is_none()
                    && self.bind_legacy_guardian().await.is_err()
                {
                    sleep(Duration::from_millis(20)).await;
                    continue;
                }
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
                    let guardian = self
                        .legacy_guardian
                        .as_ref()
                        .context("original fixture Legacy guardian custody unavailable")?;
                    if first {
                        guardian.require_fresh().await?;
                    } else {
                        guardian.require_live().await?;
                    }
                    return Ok::<_, anyhow::Error>(());
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await??;
        Ok(())
    }
    pub async fn restart_broker(&mut self) -> Result<()> {
        self.legacy_guardian
            .as_ref()
            .context("original fixture Legacy guardian custody unavailable")?
            .require_live()
            .await?;
        if let Some(child) = self.broker.as_mut() {
            child.kill().await?;
            child.wait().await?;
            self.broker = None;
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
    pub fn report_terminal_wait_evidence(&self, worker: Option<&WorkerV1>) {
        let (stop_received, terminal) = worker
            .and_then(|worker| self.terminal_runtime_witness(worker))
            .unwrap_or(("Unknown", "Unknown"));
        println!(
            "phase=handoff_worker_terminal view=runtime_witness stop_received={stop_received} terminal={terminal}"
        );
        match worker.map(|worker| self.terminal_batch_failure(worker)) {
            Some(Ok(Some(failure))) => println!(
                "phase=handoff_worker_terminal view=batch_failure category=observed failure_phase={:?} broker_step={} runtime_stage={} enforcer_stage={}",
                failure.phase, failure.broker_step, failure.runtime_stage, failure.enforcer_stage,
            ),
            Some(Ok(None)) => {
                println!("phase=handoff_worker_terminal view=batch_failure category=not_observed");
            }
            _ => println!("phase=handoff_worker_terminal view=batch_failure category=Unknown"),
        }
    }
    fn terminal_runtime_witness(&self, worker: &WorkerV1) -> Option<(&'static str, &'static str)> {
        if self.engine_task.is_finished() {
            return None;
        }
        let inspection = worker.attempt.created_inspection.as_ref()?;
        let id = inspection.container_id.as_str();
        let name = worker.attempt.stable_name.as_str();
        if worker.attempt.container_id.as_deref() != Some(id) || inspection.stable_name != name {
            return None;
        }
        let engine = self.engine.try_lock().ok()?;
        let container = engine.containers.get(id)?;
        let facts = &container.facts;
        let labels = &facts["Config"]["Labels"];
        if facts["Id"].as_str() != Some(id)
            || facts["Name"]
                .as_str()
                .and_then(|name| name.strip_prefix('/'))
                != Some(name)
            || labels["agentsandbox.purpose"] != "run"
            || labels["agentsandbox.session"].as_str() != Some(worker.launch.session_id.as_str())
            || labels["agentsandbox.operation"].as_str()
                != Some(worker.admission_operation_id.as_str())
        {
            return None;
        }
        let mut stop_received = if engine.errors.is_empty() {
            "false"
        } else {
            "Unknown"
        };
        for call in &engine.calls {
            let target = call.last().map(String::as_str);
            if target != Some(id) && target != Some(name) {
                continue;
            }
            let command = match call.as_slice() {
                [option, _, command, ..] if option == "--host" => Some(command.as_str()),
                [command, ..] => Some(command.as_str()),
                [] => None,
            };
            if matches!(command, Some("kill" | "stop")) && target == Some(id) {
                stop_received = "true";
                break;
            }
            if call
                .iter()
                .any(|arg| matches!(arg.as_str(), "kill" | "stop"))
            {
                stop_received = "Unknown";
            }
        }
        let state = &facts["State"];
        let terminal = match (
            state["Status"].as_str(),
            state["Running"].as_bool(),
            *container.done.borrow(),
            facts["RestartCount"].as_u64(),
        ) {
            (Some("exited"), Some(false), true, Some(0)) => "true",
            (Some("running"), Some(true), false, Some(0)) => "false",
            _ => "Unknown",
        };
        Some((stop_received, terminal))
    }
    fn terminal_batch_failure(&self, worker: &WorkerV1) -> Result<Option<BatchFailureWitness>> {
        use rusqlite::OptionalExtension;

        let db = self.readonly_a()?;
        db.busy_timeout(Duration::ZERO)?;
        // Worker session is unique in this Store and tied to the verified admission.
        let payload = db
            .query_row(
                "SELECT json_extract(payload,'$.data') FROM events \
                 WHERE kind='broker.batch.failure' AND json_extract(payload,'$.source')='runner' \
                 AND json_extract(payload,'$.provenance')='observed' \
                 AND json_extract(payload,'$.session_id')=?1 ORDER BY seq DESC LIMIT 1",
                [worker.launch.session_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(payload) = payload else {
            return Ok(None);
        };
        let data: Value = serde_json::from_str(&payload)?;
        ensure!(
            data["version"].as_u64() == Some(1) && data["runtime_effect"] == "unknown",
            "unrecognized batch failure diagnostic"
        );
        Ok(Some(BatchFailureWitness {
            phase: serde_json::from_value(data["phase"].clone())?,
            broker_step: closed_category(
                &data["broker_step"],
                &[
                    "terminal_observation_ack",
                    "result_audit",
                    "cleanup_confirmation",
                    "cleanup_ack",
                    "failure_observed",
                ],
            ),
            runtime_stage: closed_category(
                &data["runtime"]["stage"],
                &[
                    "observe_only",
                    "authority_before_spawn",
                    "invocation_before_spawn",
                    "stop_before_spawn",
                    "guard_before_spawn",
                    "spawn",
                    "started_observation",
                ],
            ),
            enforcer_stage: closed_category(
                &data["enforcer"]["stage"],
                &["register", "before_claim", "prepare", "before_spawn"],
            ),
        }))
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
                    let record: Value = serde_json::from_slice(&std::fs::read(path)?)?;
                    if let Some(fact) = channel_fact_for_job(&record, job)? {
                        return Ok(fact.clone());
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
        if self.legacy_start_attempted && self.legacy_guardian.is_none() {
            // The original child may have announced just as readiness failed.
            let _ = self.bind_legacy_guardian().await;
        }
        if let Some(stop) = self.engine_stop.take() {
            let _ = stop.send(());
        }
        let engine_joined = matches!(
            timeout(Duration::from_secs(5), &mut self.engine_task).await,
            Ok(Ok(()))
        );
        if !engine_joined {
            self.engine_task.abort();
            let _ = timeout(Duration::from_secs(5), &mut self.engine_task).await;
        }
        let engine_closed =
            engine_joined && require_socket_closed(&self.root.join("r")).await.is_ok();
        let processes = async {
            if !self.legacy_start_attempted {
                return Ok::<_, anyhow::Error>(());
            }
            ensure!(engine_closed, "fixture runtime closure unconfirmed");
            let guardian = match self.legacy_guardian.as_ref() {
                Some(owner) => owner.require_cleanup_ready().await.map(|()| owner),
                None => Err(anyhow::anyhow!(
                    "original fixture Legacy guardian custody unavailable"
                )),
            };
            // Reap the owned broker even if guardian custody is unconfirmed.
            // A live broker can restart its guardian on EOF.
            if let Some(broker) = self.broker.as_mut() {
                if broker.try_wait()?.is_none() {
                    broker.start_kill()?;
                }
                timeout(Duration::from_secs(5), broker.wait()).await??;
            }
            self.broker = None;
            let owner = guardian.context("fixture Legacy guardian pre-stop proof unavailable")?;
            let category = timeout(Duration::from_secs(15), owner.close())
                .await
                .context("fixture Legacy guardian closure timeout")??;
            println!("phase=fixture_legacy_guardian_close category={category}");
            self.legacy_guardian = None;
            self.legacy_start_attempted = false;
            Ok(())
        }
        .await;
        if let Some(stop) = self.server_stop.take() {
            let _ = stop.send(());
        }
        let server = timeout(Duration::from_secs(5), &mut self.server_task).await;
        if server.is_err() {
            self.server_task.abort();
            let _ = timeout(Duration::from_secs(5), &mut self.server_task).await;
        }
        let untouched = if self.faults.before_import.is_some() {
            self.assert_admission_engine_untouched().await
        } else {
            Ok(())
        };
        untouched?;
        processes?;
        server???;
        ensure!(engine_closed, "fixture runtime closure unconfirmed");
        ensure!(
            self.engine.lock().await.errors.is_empty(),
            "synthetic runtime fixture errors"
        );
        Ok(())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.legacy_start_attempted {
            println!(
                "phase=fixture_drop category=legacy_reconciliation_required private_state_preserved=true guardian_attested={} broker_join_unconfirmed={} guardian_announcement={}",
                self.legacy_guardian.is_some(),
                self.broker.is_some(),
                self.broker_root
                    .join("enforcer/announcements/e-0001.json")
                    .is_file()
            );
        }
        if let Some(stop) = self.engine_stop.take() {
            let _ = stop.send(());
        }
        if let Some(stop) = self.server_stop.take() {
            let _ = stop.send(());
        }
    }
}

fn channel_fact_for_job(record: &Value, job: Uuid) -> Result<Option<&Value>> {
    let (fact, record_job) = match record["version"].as_u64() {
        Some(1) => (&record["fact"], None),
        Some(2) => {
            let record_job: Uuid = serde_json::from_value(record["job_id"].clone())
                .context("invalid V2 channel record job identity")?;
            ensure!(
                !record_job.is_nil(),
                "V2 channel record job identity is nil"
            );
            let change = &record["change"];
            match change["kind"].as_str() {
                Some("fact" | "begin_retirement" | "finalize") => {
                    (&change["fact"], Some(record_job))
                }
                Some("history" | "retirement_intent" | "mutation") => {
                    ensure!(change.get("fact").is_none(), "unexpected V2 channel fact");
                    return Ok(None);
                }
                _ => bail!("unknown V2 channel change"),
            }
        }
        _ => bail!("unknown channel record version"),
    };
    let fact_job: Uuid = serde_json::from_value(fact["job_id"].clone())
        .context("invalid channel fact job identity")?;
    ensure!(
        fact["version"].as_u64() == Some(1) && !fact_job.is_nil(),
        "invalid channel fact version or identity"
    );
    ensure!(
        record_job.is_none_or(|record_job| record_job == fact_job),
        "channel fact job identity differs from V2 record"
    );
    Ok((fact_job == job).then_some(fact))
}

struct BatchFailureWitness {
    phase: LocalFailurePhaseV1,
    broker_step: &'static str,
    runtime_stage: &'static str,
    enforcer_stage: &'static str,
}

fn closed_category(value: &Value, allowed: &[&'static str]) -> &'static str {
    let observed = value.as_str();
    allowed
        .iter()
        .copied()
        .find(|&name| observed == Some(name))
        .unwrap_or("Unknown")
}

#[test]
fn relay_fact_selection_preserves_v1_and_checks_v2_job_binding() -> Result<()> {
    let job = Uuid::from_u128(1);
    let other = Uuid::from_u128(2);
    let fact = json!({ "version": 1, "job_id": job });
    let mut legacy = json!({ "version": 1, "fact": &fact });
    assert_eq!(channel_fact_for_job(&legacy, job)?, Some(&fact));
    assert_eq!(channel_fact_for_job(&legacy, other)?, None);
    legacy["version"] = json!(3);
    assert!(channel_fact_for_job(&legacy, job).is_err());

    for kind in ["fact", "begin_retirement", "finalize"] {
        let mut record = json!({
            "version": 2,
            "job_id": job,
            "change": { "kind": kind, "fact": &fact },
        });
        assert_eq!(channel_fact_for_job(&record, job)?, Some(&fact));
        assert_eq!(channel_fact_for_job(&record, other)?, None);
        record["job_id"] = json!(other);
        assert!(channel_fact_for_job(&record, other).is_err());
    }

    let mut history = json!({ "version": 2, "job_id": job, "change": { "kind": "history" } });
    assert_eq!(channel_fact_for_job(&history, job)?, None);
    history["change"]["kind"] = json!("unknown");
    assert!(channel_fact_for_job(&history, job).is_err());
    Ok(())
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
