//! D1: the real CLI/Gateway/SQLite finalizer after a late Managed inspect fault.
//! Only the native engine is the existing Rust fixture. No provider or daemon.
#![cfg(unix)]
mod support;

use agentsandbox::{
    api::{GatewayState, gateway_router},
    audit::{EventQuery, Store},
    control::{
        QuotaPolicyV1, RuntimeStatusV1, WorkerMutationV1, WorkerStateV1, WorkerTransitionV1,
    },
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
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
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
const OUT: &str = "d1 genuine transport stdout\n";
const ERR: &str = "d1 genuine transport stderr\n";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fallback {
    Canary,
    Terminal,
    Created,
    Unknown,
}
impl Fallback {
    fn name(self) -> &'static str {
        match self {
            Self::Canary => "canary",
            Self::Terminal => "terminal",
            Self::Created => "created",
            Self::Unknown => "unknown",
        }
    }
    fn finalizes(self) -> bool {
        matches!(self, Self::Canary | Self::Terminal)
    }
}

struct FailureEvidence(Option<tempfile::TempDir>);
impl FailureEvidence {
    fn new() -> Self {
        Self(Some(support::private_tempdir()))
    }
    fn path(&self) -> &Path {
        self.0.as_ref().unwrap().path()
    }
}
impl Drop for FailureEvidence {
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Some(directory) = self.0.take()
        {
            // Private synthetic credentials/SQLite/calls survive a raw FAIL.
            // The path is diagnostic; no credential or database body is logged.
            eprintln!(
                "D1_LATE_FINALIZER retained private fixture: {}",
                directory.keep().display()
            );
        }
    }
}

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

async fn fixture(root: &Path, fallback: Fallback) -> PathBuf {
    let bin = root.join("fixture-bin");
    fs::DirBuilder::new().mode(0o700).create(&bin).unwrap();
    let original = include_str!("runtime_fixtures/runner_terminal.rs");
    // Keep the original create/inspect/ownership/configuration protocol. The
    // first Started callback is forced to observe Running, so execute_created
    // owes exactly one final native inspect after its callback and attach end.
    // Managed's next inspect is the fault boundary; its fallback is another
    // real native invocation, never a test-supplied terminal return value.
    let start_seam = "            if purpose == \"import\" {\n";
    assert_eq!(
        original.matches(start_seam).count(),
        1,
        "fixture start seam changed"
    );
    let source = original.replace(
        start_seam,
        r#"            if purpose == "run" {
                println!("d1 genuine transport stdout");
                eprintln!("d1 genuine transport stderr");
                fs::write(root.join("attach_ended"), b"native attach ending").unwrap();
            }
            if purpose == "import" {
"#,
    );
    let inspect_seam = "            log(&root, &format!(\"inspect {id}\"));\n";
    assert_eq!(
        source.matches(inspect_seam).count(),
        1,
        "fixture inspect seam changed"
    );
    let source = source.replace(inspect_seam, r#"            log(&root, &format!("inspect {id}"));
            if fs::read_to_string(instance.join("purpose")).unwrap() == "run"
                && root.join("started_ack").is_file()
            {
                if fs::OpenOptions::new().write(true).create_new(true)
                    .open(root.join("transport_terminal_inspect")).is_ok()
                {
                    assert_eq!(state, "exited", "transport must see a genuine terminal");
                    log(&root, &format!("transport_terminal {id}"));
                } else if fs::OpenOptions::new().write(true).create_new(true)
                    .open(root.join("managed_inspect")).is_ok()
                {
                    let fallback = fs::read_to_string(root.join("fallback")).unwrap();
                    if fallback != "canary" {
                        fs::write(root.join("fault_consumed"), b"malformed Managed inspection").unwrap();
                        if matches!(fallback.as_str(), "created" | "unknown") {
                            set_state(&instance, &fallback);
                        }
                        log(&root, &format!("managed_inspect_fault {id}"));
                        println!("malformed native inspection");
                        return;
                    }
                } else if root.join("fault_consumed").is_file()
                    && !root.join("fallback_inspect").exists()
                {
                    fs::write(root.join("fallback_inspect"), state.as_bytes()).unwrap();
                    log(&root, &format!("fallback_inspect {id} {state}"));
                }
            }
"#);
    let source_path = bin.join("engine.rs");
    fs::write(&source_path, source).unwrap();
    let program = bin.join("docker");
    let output = tokio::time::timeout(
        SETUP,
        Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--edition=2024", "--crate-name", "d1_late_engine_fixture"])
            .arg(&source_path)
            .arg("-o")
            .arg(&program)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("setup: fixture compiler exceeded its budget")
    .unwrap();
    assert!(
        output.status.success(),
        "setup: Rust fixture compilation failed"
    );
    let metadata = bin.join("docker.fixture");
    fs::DirBuilder::new().mode(0o700).create(&metadata).unwrap();
    fs::write(metadata.join("root"), root.to_str().unwrap()).unwrap();
    fs::write(metadata.join("case"), "broker-crash-running").unwrap();
    fs::write(root.join("fallback"), fallback.name()).unwrap();
    program
}
fn calls(root: &Path) -> String {
    fs::read_to_string(root.join("calls")).unwrap_or_default()
}
fn count(log: &str, line: &str) -> usize {
    log.lines().filter(|actual| *actual == line).count()
}
fn events(store: &Store, kind: &str) -> Vec<agentsandbox::model::StoredEvent> {
    store
        .events(&EventQuery {
            kind: Some(kind.into()),
            ..Default::default()
        })
        .unwrap()
}
fn checkpoint(config: &Path, session: &str) -> Value {
    serde_json::from_slice(
        &fs::read(config.join("sessions").join(format!("{session}.json"))).unwrap(),
    )
    .unwrap()
}
fn lock_available(config: &Path, attempt: &str) -> bool {
    // A separate open-file description in the test process probes the actual
    // CLI flock. This never creates/replaces the lock or changes its inode.
    let path = config.join("worker-locks").join(format!("{attempt}.lock"));
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .unwrap();
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        assert_eq!(unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) }, 0);
        true
    } else {
        assert_eq!(
            std::io::Error::last_os_error().kind(),
            std::io::ErrorKind::WouldBlock
        );
        false
    }
}

#[tokio::test]
async fn canary_completed_transport_reaches_real_store_finalizer_once() {
    scenario(Fallback::Canary).await;
}
#[tokio::test]
async fn late_managed_inspect_failure_finalizes_same_native_terminal_once() {
    scenario(Fallback::Terminal).await;
}
#[tokio::test]
async fn late_managed_inspect_failure_with_created_fallback_keeps_quota_and_instance() {
    scenario(Fallback::Created).await;
}
#[tokio::test]
async fn late_managed_inspect_failure_with_unknown_fallback_keeps_quota_and_instance() {
    scenario(Fallback::Unknown).await;
}

async fn scenario(fallback: Fallback) {
    // Removing take_completed_legacy_failure from launch_worker, bypassing its
    // same-owner qualification, finalizing twice, releasing quota before the
    // real terminal ACK or promoting Created/Unknown must break these oracles.
    let temporary = FailureEvidence::new();
    let root = temporary.path().canonicalize().unwrap();
    let config = root.join("config");
    initialize_config(&config).unwrap();
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("original.txt"), b"immutable original\n").unwrap();
    let program = fixture(&root, fallback).await;
    let runner = read_token(&config.join("secrets/runner")).unwrap();
    let viewer = read_token(&config.join("secrets/viewer")).unwrap();
    let redactor = Redactor::new(vec![runner.clone(), viewer.clone()]);
    let database = root.join("audit.sqlite");
    let store = Arc::new(Store::open(&database, redactor.clone(), 1000).unwrap());
    let gateway = GatewayState::new(
        store.clone(),
        runner.clone(),
        viewer.clone(),
        Policy::default(),
        None,
        redactor.clone(),
    );
    let started = Arc::new(AtomicBool::new(false));
    let held_terminal = Arc::new(AtomicBool::new(false));
    let terminal_requests = Arc::new(AtomicUsize::new(0));
    let release_terminal = Arc::new(AtomicBool::new(false));
    let teardown = Arc::new(AtomicBool::new(false));
    let app = gateway_router(gateway).layer(middleware::from_fn({
        let (root, store, started, held_terminal, terminal_requests, release_terminal, teardown) = (
            root.clone(),
            store.clone(),
            started.clone(),
            held_terminal.clone(),
            terminal_requests.clone(),
            release_terminal.clone(),
            teardown.clone(),
        );
        move |request: Request, next: Next| {
            let (
                root,
                store,
                started,
                held_terminal,
                terminal_requests,
                release_terminal,
                teardown,
            ) = (
                root.clone(),
                store.clone(),
                started.clone(),
                held_terminal.clone(),
                terminal_requests.clone(),
                release_terminal.clone(),
                teardown.clone(),
            );
            async move {
                if request.method() != Method::POST {
                    return next.run(request).await;
                }
                let (parts, body) = request.into_parts();
                let bytes = to_bytes(body, 256 * 1024).await.unwrap();
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                let is_started = value["kind"] == "command.started";
                let terminal = serde_json::from_value::<WorkerMutationV1>(value)
                    .ok()
                    .is_some_and(|mutation| {
                        matches!(mutation.transition,
                        WorkerTransitionV1::Observe { observation }
                            if observation.status == RuntimeStatusV1::Exited
                                && observation.running == Some(false)
                                && observation.finished_at.is_some())
                    });
                if is_started {
                    assert!(
                        !started.swap(true, Ordering::SeqCst),
                        "setup: duplicate Started callback"
                    );
                    let workers = store.workers().unwrap();
                    let worker = workers.first().expect("setup: Started has no real worker");
                    let id = worker.attempt.container_id.as_ref().unwrap();
                    assert_eq!(worker.state, WorkerStateV1::Running);
                    assert!(worker.quota_held && worker.attempt.start_operation_id.is_some());
                    assert_eq!(
                        fs::read_to_string(root.join("containers").join(id).join("state")).unwrap(),
                        "running"
                    );
                    assert!(events(&store, "command.started").is_empty());
                    // A fixture-native natural exit, no kill/STOP. Keep the
                    // SAME audit callback in flight until the attach has ended.
                    fs::write(root.join("containers").join(id).join("state"), "exited").unwrap();
                    while !root.join("attach_ended").is_file() {
                        if teardown.load(Ordering::SeqCst) {
                            return StatusCode::SERVICE_UNAVAILABLE.into_response();
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }
                if terminal {
                    terminal_requests.fetch_add(1, Ordering::SeqCst);
                    held_terminal.store(true, Ordering::SeqCst);
                    while !release_terminal.load(Ordering::SeqCst) {
                        if teardown.load(Ordering::SeqCst) {
                            return StatusCode::SERVICE_UNAVAILABLE.into_response();
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }
                let response = next
                    .run(Request::from_parts(parts, Body::from(bytes)))
                    .await;
                if is_started && response.status().is_success() {
                    fs::write(root.join("started_ack"), b"real Started commit").unwrap();
                }
                if terminal && response.status().is_success() {
                    assert!(store.workers().unwrap().iter().any(|worker| worker.state
                        == WorkerStateV1::Terminal
                        && !worker.quota_held));
                    fs::write(root.join("terminal_ack"), b"real terminal commit").unwrap();
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
    let mut run = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    run.env_clear();
    for key in ["PATH", "HOME", "TMPDIR", "LANG"] {
        if let Some(value) = std::env::var_os(key) {
            run.env(key, value);
        }
    }
    run.arg("run")
        .arg("--config-dir")
        .arg(&config)
        .args(["--gateway", &origin])
        .arg("--runtime-program")
        .arg(&program)
        .args([
            "--profile",
            "observe",
            "--image",
            "worker:fixture",
            "--timeout",
            "120",
            "--capture-output",
            "--runtime-endpoint",
        ])
        .arg(format!("unix://{}", root.join("fixture.sock").display()))
        .arg("--project")
        .arg(&project)
        .args(["--", "/usr/bin/id"]);
    let mut owner = OwnedCli::spawn(run);
    let deadline = tokio::time::Instant::now() + SETUP;
    let setup_ok = loop {
        if held_terminal.load(Ordering::SeqCst)
            || (!fallback.finalizes() && owner.child.try_wait().unwrap().is_some())
        {
            break true;
        }
        if owner.child.try_wait().unwrap().is_some() || tokio::time::Instant::now() >= deadline {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let before = store.workers().unwrap();
    let before_quota = store.worker_quota(&QuotaPolicyV1::default()).unwrap();
    let held_log = calls(&root);
    let held_instance = before.first().is_some_and(|worker| {
        worker
            .attempt
            .container_id
            .as_ref()
            .is_some_and(|id| root.join("containers").join(id).is_dir())
    });
    let held_usage = store.worker_usage().unwrap();
    let held_commands =
        events(&store, "command.completed").len() + events(&store, "command.failed").len();
    let lock_identity = before.first().map(|worker| {
        let path = config
            .join("worker-locks")
            .join(format!("{}.lock", worker.attempt.attempt_id));
        let metadata = fs::symlink_metadata(path).unwrap();
        (metadata.dev(), metadata.ino())
    });
    let lock_held = before
        .first()
        .is_some_and(|worker| !lock_available(&config, &worker.attempt.attempt_id));
    release_terminal.store(true, Ordering::SeqCst);
    let owner_status = if setup_ok {
        owner.wait(ORACLE).await
    } else {
        None
    };
    // All product oracles are captured before any synthetic teardown action.
    let log = calls(&root);
    let workers = store.workers().unwrap();
    let quota = store.worker_quota(&QuotaPolicyV1::default()).unwrap();
    let usage = store.worker_usage().unwrap();
    let completed = events(&store, "command.completed");
    let failed = events(&store, "command.failed");
    let started_events = events(&store, "command.started");
    let control = events(&store, "control.worker.transition");
    let audit_valid = store.integrity().unwrap().valid;
    let saved = workers
        .first()
        .map(|worker| checkpoint(&config, &worker.launch.session_id));
    let retained = workers.first().is_some_and(|worker| {
        worker
            .attempt
            .container_id
            .as_ref()
            .is_some_and(|id| root.join("containers").join(id).is_dir())
    });
    let fault_consumed = root.join("fault_consumed").is_file();
    let fallback_observed = fs::read_to_string(root.join("fallback_inspect")).ok();
    eprintln!(
        "D1_LATE_FINALIZER case={} setup={setup_ok} fault={fault_consumed} held_terminal={} requests={} owner_exited={} audit_valid={audit_valid}",
        fallback.name(),
        held_terminal.load(Ordering::SeqCst),
        terminal_requests.load(Ordering::SeqCst),
        owner_status.is_some()
    );
    teardown.store(true, Ordering::SeqCst);
    if let Ok(id) = fs::read_to_string(root.join("run_id")) {
        let state = root.join("containers").join(id).join("state");
        // Only a still-running synthetic attach needs teardown. Preserve an
        // unresolved Created/Unknown backing state in the failure evidence.
        if owner_status.is_none()
            && fs::read_to_string(&state).is_ok_and(|state| state == "running")
        {
            fs::write(state, "exited").unwrap();
        }
    }
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
    drop(store);
    let reopened = Store::open(&database, redactor, 1000).unwrap();
    let reopened_valid = reopened.integrity().unwrap().valid;
    let reopened_workers = reopened.workers().unwrap();
    let reopened_usage = reopened.worker_usage().unwrap();
    let reopened_quota = reopened.worker_quota(&QuotaPolicyV1::default()).unwrap();
    assert!(
        server_closed && !collected.forced,
        "setup: fixture/CLI closure incomplete"
    );
    assert!(
        !collected.stdout.truncated && !collected.stderr.truncated,
        "setup: CLI capture incomplete"
    );
    for token in [&runner, &viewer] {
        assert!(
            !collected
                .stdout
                .bytes
                .windows(token.len())
                .any(|bytes| bytes == token.as_bytes())
                && !collected
                    .stderr
                    .bytes
                    .windows(token.len())
                    .any(|bytes| bytes == token.as_bytes()),
            "setup: CLI exposed a synthetic control credential"
        );
    }
    assert!(
        setup_ok
            && started.load(Ordering::SeqCst)
            && root.join("started_ack").is_file()
            && root.join("attach_ended").is_file()
            && root.join("transport_terminal_inspect").is_file()
            && root.join("managed_inspect").is_file(),
        "setup: exact real transport/Managed sequence missing"
    );
    assert_eq!(
        fault_consumed,
        fallback != Fallback::Canary,
        "setup: wrong fault boundary"
    );
    assert!(
        audit_valid && reopened_valid,
        "D1_LATE_FINALIZER: durable audit invalid"
    );
    assert_eq!(
        fs::read(project.join("original.txt")).unwrap(),
        b"immutable original\n"
    );
    assert_eq!(workers.len(), 1);
    let worker = &workers[0];
    let id = worker.attempt.container_id.as_ref().unwrap();
    assert_eq!(
        count(&log, &format!("start {id} run")),
        1,
        "D1_LATE_FINALIZER: start reissued"
    );
    assert_eq!(
        count(&log, &format!("kill {id}")),
        0,
        "D1_LATE_FINALIZER: natural terminal issued STOP"
    );
    assert_eq!(
        started_events.len(),
        1,
        "D1_LATE_FINALIZER: Started audit lost/duplicated"
    );
    assert!(
        owner_status.is_some(),
        "D1_LATE_FINALIZER: CLI did not return without forced teardown"
    );
    assert_eq!(
        collected.status.success(),
        fallback == Fallback::Canary,
        "D1_LATE_FINALIZER: original late observation error ceased to be fatal"
    );
    assert_eq!(reopened_workers.len(), 1);
    assert_eq!(reopened_workers[0].attempt, worker.attempt);
    assert_eq!(reopened_workers[0].state, worker.state);
    assert_eq!(reopened_workers[0].quota_held, worker.quota_held);
    assert_eq!(reopened_usage, usage);
    assert_eq!(reopened_quota.held_workers, quota.held_workers);
    assert!(lock_available(&config, &worker.attempt.attempt_id));
    let metadata = fs::symlink_metadata(
        config
            .join("worker-locks")
            .join(format!("{}.lock", worker.attempt.attempt_id)),
    )
    .unwrap();
    assert_eq!(
        lock_identity,
        Some((metadata.dev(), metadata.ino())),
        "D1_LATE_FINALIZER: attempt lock replaced"
    );
    let saved = saved.unwrap();
    assert_eq!(
        saved
            .pointer("/managed/owned/handle/id")
            .and_then(Value::as_str),
        Some(id.as_str()),
        "D1_LATE_FINALIZER: exact retained ownership evidence lost"
    );
    if fallback != Fallback::Canary {
        let transport = log
            .lines()
            .position(|line| line == format!("transport_terminal {id}"))
            .unwrap();
        let fault = log
            .lines()
            .position(|line| line == format!("managed_inspect_fault {id}"))
            .unwrap();
        let fallback_read = log
            .lines()
            .position(|line| line.starts_with(&format!("fallback_inspect {id} ")))
            .unwrap();
        assert!(
            transport < fault && fault < fallback_read,
            "setup: native fault/fallback order changed"
        );
        assert_eq!(count(&log, &format!("managed_inspect_fault {id}")), 1);
    }
    if fallback.finalizes() {
        assert!(
            held_terminal.load(Ordering::SeqCst) && lock_held && held_instance,
            "D1_LATE_FINALIZER: owner/instance released before terminal ACK"
        );
        assert_eq!(before[0].state, WorkerStateV1::Running);
        assert!(before[0].quota_held && held_usage.is_empty());
        assert_eq!(before_quota.held_workers, 1);
        assert_eq!(
            held_commands, 0,
            "D1_LATE_FINALIZER: command finalized before terminal ACK"
        );
        assert_eq!(
            count(&held_log, &format!("rm {id} run")),
            0,
            "D1_LATE_FINALIZER: cleanup before terminal ACK"
        );
        assert_eq!(
            terminal_requests.load(Ordering::SeqCst),
            1,
            "D1_LATE_FINALIZER: terminal finalizer repeated"
        );
        assert_eq!(
            completed.len(),
            1,
            "D1_LATE_FINALIZER: real command result lost/duplicated"
        );
        assert!(failed.is_empty());
        assert_eq!(worker.state, WorkerStateV1::Terminal);
        assert!(!worker.quota_held && worker.cleanup_removed == Some(true) && !retained);
        assert_eq!(quota.held_workers, 0);
        assert_eq!(quota.held_cpu_millis, 0);
        assert_eq!(quota.held_memory_bytes, 0);
        assert_eq!(count(&log, &format!("rm {id} run")), 1);
        assert_eq!(
            usage.len(),
            1,
            "D1_LATE_FINALIZER: real allocation lost/duplicated"
        );
        assert_eq!(usage[0].worker_id, worker.worker_id);
        assert_eq!(usage[0].attempt_id, worker.attempt.attempt_id);
        assert_eq!(usage[0].source.container_id, *id);
        assert_eq!(usage[0].duration_ns, "1000000000");
        assert!(!usage[0].billable);
        let terminal_events = control
            .iter()
            .filter(|event| {
                event
                    .event
                    .data
                    .pointer("/worker/state")
                    .and_then(Value::as_str)
                    == Some("terminal")
            })
            .collect::<Vec<_>>();
        let first = terminal_events
            .iter()
            .min_by_key(|event| event.seq)
            .expect("D1_LATE_FINALIZER: no durable terminal transition");
        assert_eq!(
            control
                .iter()
                .filter(|event| event
                    .event
                    .data
                    .get("allocation")
                    .is_some_and(|value| !value.is_null()))
                .count(),
            1,
            "D1_LATE_FINALIZER: terminal allocation commit repeated"
        );
        assert_eq!(
            first.event.data.get("operation_id").and_then(Value::as_str),
            Some(usage[0].terminal_observation_id.as_str()),
            "D1_LATE_FINALIZER: allocation not tied to exact terminal commit"
        );
        assert!(started_events[0].seq < first.seq && first.seq < completed[0].seq);
        assert!(
            control
                .iter()
                .filter(|event| event.seq >= first.seq)
                .all(|event| event
                    .event
                    .data
                    .pointer("/worker/state")
                    .and_then(Value::as_str)
                    == Some("terminal")),
            "D1_LATE_FINALIZER: terminal Known rewritten as uncertain"
        );
        assert_eq!(
            completed[0]
                .event
                .result
                .as_ref()
                .and_then(|result| result.get("stdout"))
                .and_then(Value::as_str),
            Some(OUT)
        );
        assert_eq!(
            completed[0]
                .event
                .result
                .as_ref()
                .and_then(|result| result.get("stderr"))
                .and_then(Value::as_str),
            Some(ERR)
        );
        assert_eq!(
            completed[0]
                .event
                .result
                .as_ref()
                .and_then(|result| result.get("exit_code"))
                .and_then(Value::as_i64),
            Some(0)
        );
        let result = completed[0].event.result.as_ref().unwrap();
        assert_eq!(
            result
                .get("runtime_attach_exit_code")
                .and_then(Value::as_i64),
            Some(0)
        );
        assert_eq!(
            result.get("stdout_bytes").and_then(Value::as_u64),
            Some(OUT.len() as u64)
        );
        assert_eq!(
            result.get("stderr_bytes").and_then(Value::as_u64),
            Some(ERR.len() as u64)
        );
        for field in ["timed_out", "stdout_truncated", "stderr_truncated"] {
            assert_eq!(result.get(field).and_then(Value::as_bool), Some(false));
        }
        let summary = saved.pointer("/managed/result").unwrap();
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
        if fallback == Fallback::Terminal {
            assert_eq!(
                fallback_observed.as_deref(),
                Some("exited"),
                "setup: real terminal fallback not observed"
            );
            assert!(
                summary
                    .get("postprocessing_error")
                    .and_then(Value::as_str)
                    .is_some_and(
                        |reason| reason.contains("runtime returned malformed inspection facts")
                    ),
                "D1_LATE_FINALIZER: original late cause lost beside terminal result"
            );
        }
    } else {
        assert_eq!(
            fallback_observed.as_deref(),
            Some(fallback.name()),
            "setup: requested native fallback not observed"
        );
        assert!(!held_terminal.load(Ordering::SeqCst));
        assert_eq!(terminal_requests.load(Ordering::SeqCst), 0);
        assert!(completed.is_empty() && failed.is_empty() && usage.is_empty());
        assert_eq!(worker.state, WorkerStateV1::InDoubt);
        assert!(
            worker.unknown_reason.as_deref().is_some_and(
                |reason| reason.contains("runtime returned malformed inspection facts")
            ),
            "D1_LATE_FINALIZER: incomplete fallback lost original failure cause"
        );
        assert!(worker.quota_held && worker.cleanup_removed.is_none() && retained);
        assert_eq!(quota.held_workers, 1);
        assert_eq!(quota.held_cpu_millis, u64::from(worker.launch.cpu_millis));
        assert_eq!(quota.held_memory_bytes, worker.launch.memory_bytes);
        assert_eq!(count(&log, &format!("rm {id} run")), 0);
        assert!(
            saved.pointer("/managed/result").is_none_or(Value::is_null),
            "D1_LATE_FINALIZER: incomplete fallback promoted to completed summary"
        );
        assert!(
            control.iter().all(|event| event
                .event
                .data
                .pointer("/worker/state")
                .and_then(Value::as_str)
                != Some("terminal")),
            "D1_LATE_FINALIZER: incomplete fallback produced durable terminal"
        );
    }
}
