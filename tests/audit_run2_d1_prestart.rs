//! D1: real legacy CLI + gateway/SQLite; only the engine is a Rust fixture.
//! The canary must reach its oracle before the fault test earns a RED.
#![cfg(unix)]
mod support;

use agentsandbox::{
    api::{GatewayState, gateway_router},
    audit::{EventQuery, Store},
    control::{WorkerMutationV1, WorkerStateV1, WorkerTransitionV1},
    operations::initialize_config,
    policy::Policy,
    redaction::Redactor,
    runner::read_token,
};
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{Method, StatusCode},
    middleware::{self, Next},
    response::IntoResponse,
};
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    task::JoinHandle,
};

const SETUP: Duration = Duration::from_secs(30);
const ORACLE: Duration = Duration::from_secs(10);
const DRAIN: Duration = Duration::from_secs(10);
const CAPTURE: usize = 64 * 1024;

struct Output {
    bytes: Vec<u8>,
    truncated: bool,
}
async fn capture(mut pipe: impl AsyncRead + Unpin) -> std::io::Result<Output> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut buffer = [0u8; 4096];
    loop {
        let count = pipe.read(&mut buffer).await?;
        if count == 0 {
            return Ok(Output { bytes, truncated });
        }
        let keep = count.min(CAPTURE.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..keep]);
        truncated |= keep != count;
    }
}
struct OwnedCli {
    child: Child,
    stdout: JoinHandle<std::io::Result<Output>>,
    stderr: JoinHandle<std::io::Result<Output>>,
}
struct Collected {
    status: ExitStatus,
    stdout: Output,
    stderr: Output,
    forced: bool,
}
impl OwnedCli {
    fn spawn(mut command: Command) -> Self {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdout = tokio::spawn(capture(child.stdout.take().unwrap()));
        let stderr = tokio::spawn(capture(child.stderr.take().unwrap()));
        Self {
            child,
            stdout,
            stderr,
        }
    }
    async fn wait(&mut self, duration: Duration) -> Option<ExitStatus> {
        tokio::time::timeout(duration, self.child.wait())
            .await
            .ok()
            .and_then(Result::ok)
    }
    async fn finish(mut self) -> Collected {
        let (status, forced) = match self.wait(DRAIN).await {
            Some(status) => (status, false),
            None => {
                self.child.start_kill().unwrap();
                (
                    tokio::time::timeout(DRAIN, self.child.wait())
                        .await
                        .unwrap()
                        .unwrap(),
                    true,
                )
            }
        };
        // The child and synthetic engine are closed before either reader may
        // be cancelled. A reader timeout is a harness failure, never RED D1.
        let stdout = tokio::time::timeout(DRAIN, &mut self.stdout).await;
        let stderr = tokio::time::timeout(DRAIN, &mut self.stderr).await;
        if stdout.is_err() {
            self.stdout.abort();
            let _ = self.stdout.await;
        }
        if stderr.is_err() {
            self.stderr.abort();
            let _ = self.stderr.await;
        }
        Collected {
            status,
            forced,
            stdout: stdout
                .expect("setup: CLI stdout did not drain")
                .unwrap()
                .unwrap(),
            stderr: stderr
                .expect("setup: CLI stderr did not drain")
                .unwrap()
                .unwrap(),
        }
    }
}
fn fixture(root: &Path) -> PathBuf {
    let bin = root.join("fixture-bin");
    fs::DirBuilder::new().mode(0o700).create(&bin).unwrap();
    let source = bin.join("engine.rs");
    fs::write(&source, include_str!("runtime_fixtures/runner_terminal.rs")).unwrap();
    let compiled =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--edition=2024", "--crate-name", "d1_engine_fixture"])
            .arg(&source)
            .arg("-o")
            .arg(bin.join("docker"))
            .output()
            .unwrap();
    assert!(
        compiled.status.success(),
        "setup: Rust fixture compilation failed"
    );
    let metadata = bin.join("docker.fixture");
    fs::DirBuilder::new().mode(0o700).create(&metadata).unwrap();
    fs::write(metadata.join("root"), root.to_str().unwrap()).unwrap();
    fs::write(metadata.join("case"), "broker-crash-running").unwrap();
    bin.join("docker")
}
fn calls(root: &Path) -> String {
    fs::read_to_string(root.join("calls")).unwrap_or_default()
}
fn inspect_count(root: &Path, id: &str) -> usize {
    calls(root)
        .lines()
        .filter(|line| *line == format!("inspect {id}"))
        .count()
}
fn command(config: &Path, origin: &str, program: &Path, action: &str) -> Command {
    let mut result = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    result.env_clear();
    // Root admission supplies only source-free public/private-scratch values.
    for key in ["PATH", "HOME", "TMPDIR", "LANG"] {
        if let Some(value) = std::env::var_os(key) {
            result.env(key, value);
        }
    }
    result
        .arg(action)
        .arg("--config-dir")
        .arg(config)
        .args(["--gateway", origin])
        .arg("--runtime-program")
        .arg(program);
    result
}
fn force_fixture_exit(root: &Path) {
    if let Ok(entries) = fs::read_dir(root.join("containers")) {
        for entry in entries.flatten() {
            if entry.path().join("state").is_file() {
                fs::write(entry.path().join("state"), "exited").unwrap();
            }
        }
    }
}

#[tokio::test]
async fn canary_cli_owner_enforces_durable_stop_without_prestart_fault() {
    scenario(false).await;
}
#[tokio::test]
async fn prestart_read_failure_refuses_cli_start_and_retains_audited_owner() {
    scenario(true).await;
}

async fn scenario(inject: bool) {
    // Break caught: the monitor returns Err before start while join! lets the
    // production CLI continue issuing the first business start unmonitored.
    let temporary = support::private_tempdir();
    let root = temporary.path().canonicalize().unwrap();
    let config = root.join("config");
    initialize_config(&config).unwrap();
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("original.txt"), b"immutable original\n").unwrap();
    let program = fixture(&root);
    let runner = read_token(&config.join("secrets/runner")).unwrap();
    let viewer = read_token(&config.join("secrets/viewer")).unwrap();
    let redactor = Redactor::new(vec![runner.clone(), viewer.clone()]);
    let store = Arc::new(Store::open(&root.join("audit.sqlite"), redactor.clone(), 1000).unwrap());
    let gateway = GatewayState::new(
        store.clone(),
        runner.clone(),
        viewer.clone(),
        Policy::default(),
        None,
        redactor,
    );
    let submitted = Arc::new(AtomicBool::new(false));
    let created_held = Arc::new(AtomicBool::new(false));
    let failed_read = Arc::new(AtomicBool::new(false));
    let monitor_inspected = Arc::new(AtomicBool::new(false));
    let teardown = Arc::new(AtomicBool::new(false));
    let app = gateway_router(gateway).layer(middleware::from_fn({
        let (submitted, created_held, failed_read, monitor_inspected, teardown, store, root) = (
            submitted.clone(),
            created_held.clone(),
            failed_read.clone(),
            monitor_inspected.clone(),
            teardown.clone(),
            store.clone(),
            root.clone(),
        );
        move |request: Request, next: Next| {
            let (submitted, created_held, failed_read, monitor_inspected, teardown, store, root) = (
                submitted.clone(),
                created_held.clone(),
                failed_read.clone(),
                monitor_inspected.clone(),
                teardown.clone(),
                store.clone(),
                root.clone(),
            );
            async move {
                if inject
                    && request.method() == Method::GET
                    && request.uri().path().starts_with("/control/api/v1/workers/")
                    && submitted.load(Ordering::SeqCst)
                    && created_held.load(Ordering::SeqCst)
                    && !failed_read.swap(true, Ordering::SeqCst)
                {
                    // The real Created HTTP request is held before Store. Its
                    // on_created caller has already retained the actual CID.
                    assert!(
                        store
                            .workers()
                            .unwrap()
                            .iter()
                            .all(|worker| worker.state != WorkerStateV1::Running
                                && worker.observed_started_at.is_none()),
                        "setup: fault was no longer prestart"
                    );
                    return StatusCode::TOO_MANY_REQUESTS.into_response();
                }
                if request.method() != Method::POST {
                    return next.run(request).await;
                }
                let (parts, body) = request.into_parts();
                let bytes = to_bytes(body, 256 * 1024).await.unwrap();
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                let is_submitted = value["kind"] == "runtime.submitted";
                let is_created = serde_json::from_value::<WorkerMutationV1>(value)
                    .is_ok_and(|m| matches!(m.transition, WorkerTransitionV1::Created { .. }));
                if inject && is_created {
                    let id = fs::read_to_string(root.join("run_id")).unwrap();
                    let before = inspect_count(&root, &id);
                    created_held.store(true, Ordering::SeqCst);
                    // No arbitrary settling sleep: while Created is held the
                    // only run-CID inspector is monitor::stop_owned after Err.
                    // The fixture logs this real native inspection, not a bool
                    // supplied to the production launch or permission path.
                    loop {
                        if teardown.load(Ordering::SeqCst) {
                            return StatusCode::SERVICE_UNAVAILABLE.into_response();
                        }
                        if failed_read.load(Ordering::SeqCst) && inspect_count(&root, &id) > before
                        {
                            monitor_inspected.store(true, Ordering::SeqCst);
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }
                let response = next
                    .run(Request::from_parts(parts, Body::from(bytes)))
                    .await;
                if is_submitted && response.status().is_success() {
                    submitted.store(true, Ordering::SeqCst);
                }
                if response.status().is_success()
                    && store
                        .workers()
                        .unwrap()
                        .iter()
                        .any(|worker| worker.state == WorkerStateV1::Terminal)
                {
                    fs::write(root.join("terminal_ack"), b"durable terminal").unwrap();
                }
                response
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    let mut server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    let mut run = command(&config, &origin, &program, "run");
    run.args([
        "--profile",
        "observe",
        "--image",
        "worker:fixture",
        "--timeout",
        "120",
        "--runtime-endpoint",
        &format!("unix://{}", root.join("fixture.sock").display()),
    ])
    .arg("--project")
    .arg(&project)
    .args(["--", "/usr/bin/id"]);
    let mut owner = OwnedCli::spawn(run);
    let setup_deadline = tokio::time::Instant::now() + SETUP;
    let setup_ok = loop {
        let workers = store.workers().unwrap();
        let ready = if inject {
            monitor_inspected.load(Ordering::SeqCst)
        } else {
            workers.first().is_some_and(|worker| {
                worker.state == WorkerStateV1::Running
                    && config
                        .join("worker-control-monitor")
                        .join(format!("{}.json", worker.launch.session_id))
                        .is_file()
            }) && !store
                .events(&EventQuery {
                    kind: Some("command.started".into()),
                    ..Default::default()
                })
                .unwrap()
                .is_empty()
        };
        if ready {
            break true;
        }
        if owner.child.try_wait().unwrap().is_some()
            || tokio::time::Instant::now() >= setup_deadline
        {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let mut stop_ack = true;
    if setup_ok && !inject {
        let worker = store.workers().unwrap().pop().unwrap();
        let mut stop = command(&config, &origin, &program, "worker-stop");
        stop.args(["--session", &worker.launch.session_id]);
        let result = OwnedCli::spawn(stop).finish().await;
        stop_ack = result.status.success()
            && !result.forced
            && !result.stdout.truncated
            && !result.stderr.truncated;
    }
    let owner_status = if setup_ok && stop_ack {
        owner.wait(ORACLE).await
    } else {
        None
    };
    // All oracle observations precede any synthetic teardown action.
    let before_teardown_calls = calls(&root);
    let starts = before_teardown_calls
        .lines()
        .filter(|line| line.starts_with("start ") && line.ends_with(" run"))
        .count();
    let id = fs::read_to_string(root.join("run_id")).ok();
    let exact_kill = id.as_ref().is_some_and(|id| {
        before_teardown_calls
            .lines()
            .any(|line| line == format!("kill {id}"))
    });
    let process_state = id
        .as_ref()
        .and_then(|id| fs::read_to_string(root.join("containers").join(id).join("state")).ok());
    let workers = store.workers().unwrap();
    let audit_valid = store.integrity().unwrap().valid;
    let started_events = store
        .events(&EventQuery {
            kind: Some("command.started".into()),
            ..Default::default()
        })
        .unwrap()
        .len();
    let checkpoint = workers.first().map(|worker| {
        config
            .join("sessions")
            .join(format!("{}.json", worker.launch.session_id))
    });
    let retained: Option<Value> = checkpoint
        .as_ref()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let retained_exact_id = id.as_deref().is_some_and(|id| {
        retained
            .as_ref()
            .and_then(|plan| plan.pointer("/managed/owned/handle/id"))
            .and_then(Value::as_str)
            == Some(id)
    });
    let tolerance_epoch = workers.first().is_some_and(|worker| {
        config
            .join("worker-control-monitor")
            .join(format!("{}.json", worker.launch.session_id))
            .exists()
    });
    eprintln!(
        "D1_ORACLE fault={inject} setup={setup_ok} monitor_inspected={} owner_exited={} starts={starts} exact_kill={exact_kill} audit_valid={audit_valid}",
        monitor_inspected.load(Ordering::SeqCst),
        owner_status.is_some()
    );
    teardown.store(true, Ordering::SeqCst);
    force_fixture_exit(&root);
    let collected = owner.finish().await;
    let _ = shutdown.send(());
    let server_closed = match tokio::time::timeout(DRAIN, &mut server).await {
        Ok(result) => result.is_ok_and(|result| result.is_ok()),
        Err(_) => {
            server.abort();
            let _ = server.await;
            false
        }
    };
    assert!(
        server_closed && !collected.forced,
        "setup: fixture/CLI closure incomplete"
    );
    assert!(
        !collected.stdout.truncated && !collected.stderr.truncated,
        "setup: bounded CLI capture incomplete"
    );
    assert!(
        !collected
            .stdout
            .bytes
            .windows(runner.len())
            .any(|bytes| bytes == runner.as_bytes())
            && !collected
                .stderr
                .bytes
                .windows(runner.len())
                .any(|bytes| bytes == runner.as_bytes())
            && !collected
                .stdout
                .bytes
                .windows(viewer.len())
                .any(|bytes| bytes == viewer.as_bytes())
            && !collected
                .stderr
                .bytes
                .windows(viewer.len())
                .any(|bytes| bytes == viewer.as_bytes()),
        "setup: CLI output exposed a synthetic control credential"
    );
    assert!(
        setup_ok && stop_ack,
        "setup: D1 discriminating precondition not reached"
    );
    assert!(audit_valid, "setup: real audit chain invalid");
    assert_eq!(
        fs::read(project.join("original.txt")).unwrap(),
        b"immutable original\n"
    );
    let worker = workers.first().expect("setup: no real worker record");
    if inject {
        assert!(
            failed_read.load(Ordering::SeqCst) && monitor_inspected.load(Ordering::SeqCst),
            "setup: actual monitor error/stop path not reached"
        );
        assert_eq!(
            starts, 0,
            "D1_ORACLE: prestart failure issued a business start"
        );
        assert!(
            owner_status.is_some_and(|status| !status.success()),
            "D1_ORACLE: refused launch owner did not return an error"
        );
        assert_eq!(
            started_events, 0,
            "D1_ORACLE: command.started was acknowledged after failure"
        );
        assert_eq!(
            worker.state,
            WorkerStateV1::InDoubt,
            "D1_ORACLE: refusal was not made explicit in durable control"
        );
        assert!(
            worker
                .unknown_reason
                .as_ref()
                .is_some_and(|reason| reason.contains("429")),
            "D1_ORACLE: original control-read cause was not retained in audit"
        );
        assert!(
            worker.quota_held && retained_exact_id,
            "D1_ORACLE: prestart refusal lost its hold or exact owned instance"
        );
        assert!(
            worker.attempt.start_operation_id.is_none(),
            "D1_ORACLE: failed supervision still obtained a first-start claim"
        );
        assert!(
            !tolerance_epoch,
            "D1_ORACLE: prestart failure entered post-ACK tolerance"
        );
        assert_ne!(process_state.as_deref(), Some("running"));
    } else {
        assert!(
            owner_status.is_some_and(|status| status.success()),
            "D1_CANARY: actual CLI did not complete durable stop"
        );
        assert_eq!(starts, 1);
        assert_eq!(started_events, 1);
        assert!(exact_kill);
        assert_eq!(worker.state, WorkerStateV1::Terminal);
        assert!(!worker.quota_held);
        assert_eq!(worker.cleanup_removed, Some(true));
        assert_eq!(store.worker_usage().unwrap().len(), 1);
    }
}

// The two preceding c60 scenarios and their oracles remain byte-exact.
// This additional case catches revocation after a real StartClaim, in the
// last awaited authority check, before the engine start is admitted.
#[tokio::test]
async fn prestart_failure_during_final_authority_check_refuses_first_start() {
    after_claim_authority_failure().await;
}

fn fixture_with_late_info_gate(root: &Path) -> PathBuf {
    let bin = root.join("fixture-bin");
    fs::DirBuilder::new().mode(0o700).create(&bin).unwrap();
    let original = include_str!("runtime_fixtures/runner_terminal.rs");
    let needle = "        \"info\" => {\n";
    assert_eq!(
        original.matches(needle).count(),
        1,
        "setup: fixture info entry no longer has one closed hook location"
    );
    let hook = r#"        "info" => {
            // Synthetic test barrier only. One actual info process waits;
            // subsequent info calls needed by monitor STOP remain usable.
            if root.join("d1_late_info_arm").is_file() {
                let mut claim = fs::OpenOptions::new();
                claim.write(true).create_new(true);
                std::os::unix::fs::OpenOptionsExt::mode(&mut claim, 0o600);
                match claim.open(root.join("d1_late_info_claimed")) {
                    Ok(_owned_claim) => {
                        fs::write(root.join("d1_late_info_entered"), b"waiting").unwrap();
                        while !root.join("d1_late_info_release").is_file() {
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => panic!("synthetic info barrier unavailable: {:?}", error.kind()),
                }
            }
"#;
    let source = bin.join("engine.rs");
    fs::write(&source, original.replacen(needle, hook, 1)).unwrap();
    let compiled =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args([
                "--edition=2024",
                "--crate-name",
                "d1_late_authority_fixture",
            ])
            .arg(&source)
            .arg("-o")
            .arg(bin.join("docker"))
            .output()
            .unwrap();
    assert!(
        compiled.status.success(),
        "setup: late-authority Rust fixture compilation failed"
    );
    let metadata = bin.join("docker.fixture");
    fs::DirBuilder::new().mode(0o700).create(&metadata).unwrap();
    fs::write(metadata.join("root"), root.to_str().unwrap()).unwrap();
    fs::write(metadata.join("case"), "broker-crash-running").unwrap();
    bin.join("docker")
}

async fn after_claim_authority_failure() {
    let temporary = support::private_tempdir();
    let root = temporary.path().canonicalize().unwrap();
    let config = root.join("config");
    initialize_config(&config).unwrap();
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("original.txt"), b"immutable original\n").unwrap();
    let program = fixture_with_late_info_gate(&root);
    let runner = read_token(&config.join("secrets/runner")).unwrap();
    let viewer = read_token(&config.join("secrets/viewer")).unwrap();
    let redactor = Redactor::new(vec![runner.clone(), viewer.clone()]);
    let store = Arc::new(Store::open(&root.join("audit.sqlite"), redactor.clone(), 1000).unwrap());
    let gateway = GatewayState::new(
        store.clone(),
        runner.clone(),
        viewer.clone(),
        Policy::default(),
        None,
        redactor,
    );
    let failed_read = Arc::new(AtomicBool::new(false));
    let fault_ready = Arc::new(AtomicBool::new(false));
    let inspect_floor = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let known_claim = Arc::new(std::sync::Mutex::new(None::<String>));
    let app = gateway_router(gateway).layer(middleware::from_fn({
        let (failed_read, fault_ready, inspect_floor, known_claim, store, root) = (
            failed_read.clone(),
            fault_ready.clone(),
            inspect_floor.clone(),
            known_claim.clone(),
            store.clone(),
            root.clone(),
        );
        move |request: Request, next: Next| {
            let (failed_read, fault_ready, inspect_floor, known_claim, store, root) = (
                failed_read.clone(),
                fault_ready.clone(),
                inspect_floor.clone(),
                known_claim.clone(),
                store.clone(),
                root.clone(),
            );
            async move {
                if request.method() == Method::GET
                    && request.uri().path().starts_with("/control/api/v1/workers/")
                    && root.join("d1_late_info_entered").is_file()
                    && !failed_read.swap(true, Ordering::SeqCst)
                {
                    let workers = store.workers().unwrap();
                    let worker = workers
                        .first()
                        .expect("setup: late authority gate has no worker");
                    assert_eq!(
                        worker.state,
                        WorkerStateV1::StartIssued,
                        "setup: fault is not between the real claim and first start"
                    );
                    assert!(
                        worker.observed_started_at.is_none(),
                        "setup: late-authority fault followed an observed start"
                    );
                    assert_eq!(
                        worker.attempt.start_operation_id.as_deref(),
                        known_claim.lock().unwrap().as_deref(),
                        "setup: late-authority gate is not bound to its canonical claim"
                    );
                    assert!(worker.attempt.start_operation_id.is_some());
                    let id = fs::read_to_string(root.join("run_id")).unwrap();
                    inspect_floor.store(inspect_count(&root, &id), Ordering::SeqCst);
                    fault_ready.store(true, Ordering::SeqCst);
                    return StatusCode::TOO_MANY_REQUESTS.into_response();
                }
                if request.method() != Method::POST {
                    return next.run(request).await;
                }
                let (parts, body) = request.into_parts();
                let bytes = to_bytes(body, 256 * 1024).await.unwrap();
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                let mutation = serde_json::from_value::<WorkerMutationV1>(value).ok();
                let claim = mutation.as_ref().filter(|mutation| {
                    matches!(&mutation.transition, WorkerTransitionV1::StartClaim { .. })
                });
                let response = next
                    .run(Request::from_parts(parts, Body::from(bytes)))
                    .await;
                if response.status().is_success() {
                    if let Some(claim) = claim {
                        let workers = store.workers().unwrap();
                        let worker = workers
                            .first()
                            .expect("setup: acknowledged claim has no worker");
                        assert_eq!(
                            worker.attempt.start_operation_id.as_deref(),
                            Some(claim.operation_id.as_str()),
                            "setup: HTTP response did not acknowledge this exact canonical claim"
                        );
                        *known_claim.lock().unwrap() = Some(claim.operation_id.clone());
                        fs::write(root.join("d1_late_info_arm"), b"canonical claim known").unwrap();
                    }
                    if store
                        .workers()
                        .unwrap()
                        .iter()
                        .any(|worker| worker.state == WorkerStateV1::Terminal)
                    {
                        fs::write(root.join("terminal_ack"), b"durable terminal").unwrap();
                    }
                }
                response
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    let mut server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    let mut run = command(&config, &origin, &program, "run");
    run.args([
        "--profile",
        "observe",
        "--image",
        "worker:fixture",
        "--timeout",
        "120",
        "--runtime-endpoint",
        &format!("unix://{}", root.join("fixture.sock").display()),
    ])
    .arg("--project")
    .arg(&project)
    .args(["--", "/usr/bin/id"]);
    let mut owner = OwnedCli::spawn(run);
    let setup_deadline = tokio::time::Instant::now() + SETUP;
    let setup_ok = loop {
        let inspected = fault_ready.load(Ordering::SeqCst)
            && fs::read_to_string(root.join("run_id"))
                .ok()
                .is_some_and(|id| inspect_count(&root, &id) > inspect_floor.load(Ordering::SeqCst));
        if inspected {
            // The launch's info call is still held, so this new exact-CID
            // inspection is from the monitor's post-error stop path. Release
            // that actual awaited command only after selecting the error.
            fs::write(root.join("d1_late_info_release"), b"monitor inspected").unwrap();
            break true;
        }
        if owner.child.try_wait().unwrap().is_some()
            || tokio::time::Instant::now() >= setup_deadline
        {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let owner_status = if setup_ok {
        owner.wait(ORACLE).await
    } else {
        None
    };
    let before_teardown_calls = calls(&root);
    let starts = before_teardown_calls
        .lines()
        .filter(|line| line.starts_with("start ") && line.ends_with(" run"))
        .count();
    let id = fs::read_to_string(root.join("run_id")).ok();
    let process_state = id
        .as_ref()
        .and_then(|id| fs::read_to_string(root.join("containers").join(id).join("state")).ok());
    let workers = store.workers().unwrap();
    let audit_valid = store.integrity().unwrap().valid;
    let started_events = store
        .events(&EventQuery {
            kind: Some("command.started".into()),
            ..Default::default()
        })
        .unwrap()
        .len();
    let expected_claim = known_claim.lock().unwrap().clone();
    let retained: Option<Value> = workers
        .first()
        .and_then(|worker| {
            fs::read(
                config
                    .join("sessions")
                    .join(format!("{}.json", worker.launch.session_id)),
            )
            .ok()
        })
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let retained_exact_id = id.as_deref().is_some_and(|id| {
        retained
            .as_ref()
            .and_then(|plan| plan.pointer("/managed/owned/handle/id"))
            .and_then(Value::as_str)
            == Some(id)
    });
    let tolerance_epoch = workers.first().is_some_and(|worker| {
        config
            .join("worker-control-monitor")
            .join(format!("{}.json", worker.launch.session_id))
            .exists()
    });
    eprintln!(
        "D1_LATE_ORACLE setup={setup_ok} fault={} owner_exited={} starts={starts} audit_valid={audit_valid}",
        failed_read.load(Ordering::SeqCst),
        owner_status.is_some()
    );
    // Nothing below changes an oracle observation. This releases only the
    // synthetic command/worker and joins the same real CLI/capture owners.
    fs::write(root.join("d1_late_info_release"), b"teardown").unwrap();
    force_fixture_exit(&root);
    let collected = owner.finish().await;
    let _ = shutdown.send(());
    let server_closed = match tokio::time::timeout(DRAIN, &mut server).await {
        Ok(result) => result.is_ok_and(|result| result.is_ok()),
        Err(_) => {
            server.abort();
            let _ = server.await;
            false
        }
    };
    assert!(
        server_closed && !collected.forced,
        "setup: late-authority fixture/CLI closure incomplete"
    );
    assert!(
        !collected.stdout.truncated && !collected.stderr.truncated,
        "setup: late-authority bounded CLI capture incomplete"
    );
    assert!(
        !collected
            .stdout
            .bytes
            .windows(runner.len())
            .any(|bytes| bytes == runner.as_bytes())
            && !collected
                .stderr
                .bytes
                .windows(runner.len())
                .any(|bytes| bytes == runner.as_bytes())
            && !collected
                .stdout
                .bytes
                .windows(viewer.len())
                .any(|bytes| bytes == viewer.as_bytes())
            && !collected
                .stderr
                .bytes
                .windows(viewer.len())
                .any(|bytes| bytes == viewer.as_bytes()),
        "setup: late-authority CLI exposed a synthetic control credential"
    );
    assert!(
        setup_ok && fault_ready.load(Ordering::SeqCst),
        "setup: actual post-claim authority wait and monitor error were not reached"
    );
    assert!(
        audit_valid,
        "setup: late-authority real audit chain invalid"
    );
    assert_eq!(
        fs::read(project.join("original.txt")).unwrap(),
        b"immutable original\n"
    );
    let worker = workers
        .first()
        .expect("setup: late-authority worker record missing");
    assert_eq!(
        starts, 0,
        "D1_LATE_ORACLE: revocation during final authority wait still issued start"
    );
    assert!(
        owner_status.is_some_and(|status| !status.success()),
        "D1_LATE_ORACLE: refused owner did not return an error"
    );
    assert_eq!(
        started_events, 0,
        "D1_LATE_ORACLE: started audit appeared after prestart revocation"
    );
    assert_eq!(
        worker.state,
        WorkerStateV1::InDoubt,
        "D1_LATE_ORACLE: post-claim refusal was not durably explicit"
    );
    assert!(
        worker
            .unknown_reason
            .as_ref()
            .is_some_and(|reason| reason.contains("429")),
        "D1_LATE_ORACLE: original monitor cause was lost"
    );
    assert!(
        worker.quota_held && retained_exact_id,
        "D1_LATE_ORACLE: claimed refusal lost hold or exact owned instance"
    );
    assert!(expected_claim.is_some());
    assert_eq!(
        worker.attempt.start_operation_id, expected_claim,
        "D1_LATE_ORACLE: committed claim was erased or replaced after refusal"
    );
    assert!(
        !tolerance_epoch,
        "D1_LATE_ORACLE: prestart failure acquired post-ACK tolerance"
    );
    assert_ne!(process_state.as_deref(), Some("running"));
}

// AfterEffect contrast: a selected control failure before the second ACK
// must remain fatal without erasing genuinely completed terminal evidence.
#[tokio::test]
async fn control_failure_after_start_before_started_ack_preserves_terminal_known() {
    after_effect_started_ack_failure().await;
}

async fn after_effect_started_ack_failure() {
    let temporary = support::private_tempdir();
    let root = temporary.path().canonicalize().unwrap();
    let config = root.join("config");
    initialize_config(&config).unwrap();
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("original.txt"), b"immutable original\n").unwrap();
    let program = fixture(&root);
    let runner = read_token(&config.join("secrets/runner")).unwrap();
    let viewer = read_token(&config.join("secrets/viewer")).unwrap();
    let redactor = Redactor::new(vec![runner.clone(), viewer.clone()]);
    let store = Arc::new(Store::open(&root.join("audit.sqlite"), redactor.clone(), 1000).unwrap());
    let gateway = GatewayState::new(
        store.clone(),
        runner.clone(),
        viewer.clone(),
        Policy::default(),
        None,
        redactor,
    );
    let started_held = Arc::new(AtomicBool::new(false));
    let failed_read = Arc::new(AtomicBool::new(false));
    let native_stop_seen = Arc::new(AtomicBool::new(false));
    let callback_committed = Arc::new(AtomicBool::new(false));
    let teardown = Arc::new(AtomicBool::new(false));
    let app = gateway_router(gateway).layer(middleware::from_fn({
        let (
            started_held,
            failed_read,
            native_stop_seen,
            callback_committed,
            teardown,
            store,
            root,
        ) = (
            started_held.clone(),
            failed_read.clone(),
            native_stop_seen.clone(),
            callback_committed.clone(),
            teardown.clone(),
            store.clone(),
            root.clone(),
        );
        move |request: Request, next: Next| {
            let (
                started_held,
                failed_read,
                native_stop_seen,
                callback_committed,
                teardown,
                store,
                root,
            ) = (
                started_held.clone(),
                failed_read.clone(),
                native_stop_seen.clone(),
                callback_committed.clone(),
                teardown.clone(),
                store.clone(),
                root.clone(),
            );
            async move {
                if request.method() == Method::GET
                    && request.uri().path().starts_with("/control/api/v1/workers/")
                    && started_held.load(Ordering::SeqCst)
                    && !failed_read.swap(true, Ordering::SeqCst)
                {
                    let workers = store.workers().unwrap();
                    let worker = workers
                        .first()
                        .expect("setup: held Started ACK has no worker");
                    assert_eq!(
                        worker.state,
                        WorkerStateV1::Running,
                        "setup: AfterEffect fault has no acknowledged Running state"
                    );
                    assert!(
                        worker.observed_started_at.is_some()
                            && worker.attempt.start_operation_id.is_some(),
                        "setup: AfterEffect fault did not follow the actual first start"
                    );
                    assert!(
                        store
                            .events(&EventQuery {
                                kind: Some("command.started".into()),
                                ..Default::default()
                            })
                            .unwrap()
                            .is_empty(),
                        "setup: the second Started acknowledgement was not held"
                    );
                    return StatusCode::TOO_MANY_REQUESTS.into_response();
                }
                if request.method() != Method::POST {
                    return next.run(request).await;
                }
                let (parts, body) = request.into_parts();
                let bytes = to_bytes(body, 256 * 1024).await.unwrap();
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                let is_started = value["kind"] == "command.started";
                if is_started {
                    let id = fs::read_to_string(root.join("run_id")).unwrap();
                    assert_eq!(
                        calls(&root)
                            .lines()
                            .filter(|line| line.starts_with("start ") && line.ends_with(" run"))
                            .count(),
                        1,
                        "setup: held Started ACK did not follow exactly one real start"
                    );
                    assert_eq!(
                        fs::read_to_string(root.join("containers").join(&id).join("state"))
                            .unwrap(),
                        "running",
                        "setup: Started callback was not in flight on its running instance"
                    );
                    started_held.store(true, Ordering::SeqCst);
                    loop {
                        if teardown.load(Ordering::SeqCst) {
                            return StatusCode::SERVICE_UNAVAILABLE.into_response();
                        }
                        let killed = calls(&root)
                            .lines()
                            .any(|line| line == format!("kill {id}"));
                        let exited =
                            fs::read_to_string(root.join("containers").join(&id).join("state"))
                                .is_ok_and(|state| state == "exited");
                        if failed_read.load(Ordering::SeqCst) && killed && exited {
                            native_stop_seen.store(true, Ordering::SeqCst);
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    // Release the SAME mandatory audit request after real
                    // native kill. It is never replaced by a successful bool.
                }
                let response = next
                    .run(Request::from_parts(parts, Body::from(bytes)))
                    .await;
                if is_started && response.status().is_success() {
                    callback_committed.store(true, Ordering::SeqCst);
                }
                if response.status().is_success()
                    && store
                        .workers()
                        .unwrap()
                        .iter()
                        .any(|worker| worker.state == WorkerStateV1::Terminal)
                {
                    fs::write(root.join("terminal_ack"), b"durable terminal").unwrap();
                }
                response
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    let mut server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    let mut run = command(&config, &origin, &program, "run");
    run.args([
        "--profile",
        "observe",
        "--image",
        "worker:fixture",
        "--timeout",
        "120",
        "--runtime-endpoint",
        &format!("unix://{}", root.join("fixture.sock").display()),
    ])
    .arg("--project")
    .arg(&project)
    .args(["--", "/usr/bin/id"]);
    let mut owner = OwnedCli::spawn(run);
    let setup_deadline = tokio::time::Instant::now() + SETUP;
    let setup_ok = loop {
        if native_stop_seen.load(Ordering::SeqCst) {
            break true;
        }
        if owner.child.try_wait().unwrap().is_some()
            || tokio::time::Instant::now() >= setup_deadline
        {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let owner_status = if setup_ok {
        owner.wait(ORACLE).await
    } else {
        None
    };
    // Oracle facts are captured before any synthetic teardown action.
    let before_teardown_calls = calls(&root);
    let starts = before_teardown_calls
        .lines()
        .filter(|line| line.starts_with("start ") && line.ends_with(" run"))
        .count();
    let id = fs::read_to_string(root.join("run_id")).ok();
    let exact_kill = id.as_ref().is_some_and(|id| {
        before_teardown_calls
            .lines()
            .any(|line| line == format!("kill {id}"))
    });
    let workers = store.workers().unwrap();
    let audit_valid = store.integrity().unwrap().valid;
    let started_events = store
        .events(&EventQuery {
            kind: Some("command.started".into()),
            ..Default::default()
        })
        .unwrap();
    let terminal_commands = store
        .events(&EventQuery {
            kind: Some("command.completed".into()),
            ..Default::default()
        })
        .unwrap()
        .len()
        + store
            .events(&EventQuery {
                kind: Some("command.failed".into()),
                ..Default::default()
            })
            .unwrap()
            .len();
    let control_events = store
        .events(&EventQuery {
            kind: Some("control.worker.transition".into()),
            ..Default::default()
        })
        .unwrap();
    let usages = store.worker_usage().unwrap();
    let checkpoint: Option<Value> = workers
        .first()
        .and_then(|worker| {
            fs::read(
                config
                    .join("sessions")
                    .join(format!("{}.json", worker.launch.session_id)),
            )
            .ok()
        })
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let tolerance_epoch = workers.first().is_some_and(|worker| {
        config
            .join("worker-control-monitor")
            .join(format!("{}.json", worker.launch.session_id))
            .exists()
    });
    eprintln!(
        "D1_AFTER_ORACLE setup={setup_ok} fault={} native_stop={} callback_committed={} owner_exited={} starts={starts} exact_kill={exact_kill} audit_valid={audit_valid}",
        failed_read.load(Ordering::SeqCst),
        native_stop_seen.load(Ordering::SeqCst),
        callback_committed.load(Ordering::SeqCst),
        owner_status.is_some()
    );
    teardown.store(true, Ordering::SeqCst);
    force_fixture_exit(&root);
    let collected = owner.finish().await;
    let _ = shutdown.send(());
    let server_closed = match tokio::time::timeout(DRAIN, &mut server).await {
        Ok(result) => result.is_ok_and(|result| result.is_ok()),
        Err(_) => {
            server.abort();
            let _ = server.await;
            false
        }
    };
    assert!(
        server_closed && !collected.forced,
        "setup: AfterEffect fixture/CLI closure incomplete"
    );
    assert!(
        !collected.stdout.truncated && !collected.stderr.truncated,
        "setup: AfterEffect bounded CLI capture incomplete"
    );
    assert!(
        !collected
            .stdout
            .bytes
            .windows(runner.len())
            .any(|bytes| bytes == runner.as_bytes())
            && !collected
                .stderr
                .bytes
                .windows(runner.len())
                .any(|bytes| bytes == runner.as_bytes())
            && !collected
                .stdout
                .bytes
                .windows(viewer.len())
                .any(|bytes| bytes == viewer.as_bytes())
            && !collected
                .stderr
                .bytes
                .windows(viewer.len())
                .any(|bytes| bytes == viewer.as_bytes()),
        "setup: AfterEffect CLI exposed a synthetic control credential"
    );
    assert!(
        setup_ok
            && started_held.load(Ordering::SeqCst)
            && failed_read.load(Ordering::SeqCst)
            && native_stop_seen.load(Ordering::SeqCst),
        "setup: real running/held second ACK/fault/native stop sequence was not reached"
    );
    assert!(audit_valid, "setup: AfterEffect real audit chain invalid");
    assert_eq!(
        fs::read(project.join("original.txt")).unwrap(),
        b"immutable original\n"
    );
    let worker = workers
        .first()
        .expect("setup: AfterEffect worker record missing");
    assert!(
        owner_status.is_some_and(|status| !status.success()),
        "D1_AFTER_ORACLE: supervision failure was not fatal to the CLI owner"
    );
    assert_eq!(
        starts, 1,
        "D1_AFTER_ORACLE: business start was missing or reissued"
    );
    assert!(
        exact_kill,
        "D1_AFTER_ORACLE: exact native stop did not occur"
    );
    assert!(
        callback_committed.load(Ordering::SeqCst),
        "D1_AFTER_ORACLE: original mandatory Started audit request was discarded"
    );
    assert_eq!(
        started_events.len(),
        1,
        "D1_AFTER_ORACLE: mandatory Started audit was missing or duplicated"
    );
    assert_eq!(
        terminal_commands, 1,
        "D1_AFTER_ORACLE: actual command result was not durably finalized once"
    );
    assert_eq!(
        worker.state,
        WorkerStateV1::Terminal,
        "D1_AFTER_ORACLE: genuinely known terminal was rewritten as uncertain"
    );
    assert!(
        !worker.quota_held,
        "D1_AFTER_ORACLE: known terminal allocation was left held"
    );
    assert_eq!(
        worker.cleanup_removed,
        Some(true),
        "D1_AFTER_ORACLE: known terminal cleanup was not durably acknowledged"
    );
    assert_eq!(
        usages.len(),
        1,
        "D1_AFTER_ORACLE: known terminal usage was lost or duplicated"
    );
    let own_control = control_events
        .iter()
        .filter(|event| {
            event
                .event
                .data
                .pointer("/worker/worker_id")
                .and_then(Value::as_str)
                == Some(worker.worker_id.as_str())
        })
        .collect::<Vec<_>>();
    let first_terminal = own_control
        .iter()
        .filter(|event| {
            event
                .event
                .data
                .pointer("/worker/state")
                .and_then(Value::as_str)
                == Some("terminal")
        })
        .min_by_key(|event| event.seq)
        .expect("D1_AFTER_ORACLE: no canonical terminal audit record");
    assert!(
        started_events[0].seq < first_terminal.seq,
        "D1_AFTER_ORACLE: late Started audit followed a canonical terminal"
    );
    assert!(
        own_control
            .iter()
            .filter(|event| event.seq >= first_terminal.seq)
            .all(|event| event
                .event
                .data
                .pointer("/worker/state")
                .and_then(Value::as_str)
                == Some("terminal")),
        "D1_AFTER_ORACLE: terminal Known was subsequently rewritten"
    );
    let summary = checkpoint
        .as_ref()
        .and_then(|plan| plan.pointer("/managed/result"))
        .expect("D1_AFTER_ORACLE: completed runtime summary was not retained");
    assert_eq!(
        summary.get("session_id").and_then(Value::as_str),
        Some(worker.launch.session_id.as_str())
    );
    assert_eq!(summary.get("exit_code").and_then(Value::as_i64), Some(0));
    assert_eq!(
        summary.get("timed_out").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        summary.pointer("/cleanup/state").and_then(Value::as_str),
        Some("removed")
    );
    assert!(
        summary
            .get("postprocessing_error")
            .and_then(Value::as_str)
            .is_some_and(|reason| reason.contains("429")),
        "D1_AFTER_ORACLE: supervision cause was not retained beside known result"
    );
    assert!(
        !tolerance_epoch,
        "D1_AFTER_ORACLE: fault before the second ACK acquired tolerance"
    );
}
