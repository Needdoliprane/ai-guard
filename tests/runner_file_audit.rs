//! Lot I reproductions: real CLI, HTTP router and SQLite, synthetic engine only.
//! Initial residual assertions were preserved separately for RED evidence.
#![cfg(unix)]
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    control::{WorkerMutationV1, WorkerStateV1, WorkerTransitionV1},
    model::{Event, StoredEvent},
    operations::initialize_config,
    policy::Policy,
    redaction::Redactor,
    runner::{AuditClient, read_token},
};
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{Method, StatusCode},
    middleware::{self, Next},
    response::IntoResponse,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

const PROCESS_DEADLINE: Duration = Duration::from_secs(90);

fn fixture_binary(parent: &Path) -> PathBuf {
    // Reuse the established, standard-library-only engine. This addition only
    // supplies synthetic stdout and the bytes returned by the export helper.
    let original = include_str!("runtime_fixtures/runner_terminal.rs");
    let marker = r#"} else if case == "adapter-boundary-secret" {"#;
    assert!(original.contains(marker));
    let source = original.replace(
        marker,
        r#"} else if case == "file-audit" {
                let lines: usize = fs::read_to_string(root.join("telemetry_lines"))
                    .unwrap().trim().parse().unwrap();
                for index in 0..lines {
                    println!("{{\"type\":\"agentsandbox.telemetry\",\"version\":1,\"kind\":\"session.declared\",\"data\":{{\"index\":{index}}}}}");
                }
            } else if case == "adapter-boundary-secret" {"#,
    );
    let archive = r#"fs::read(root.join("workspace_archive")).unwrap_or_else(|_| vec![0; 1024])"#;
    assert!(source.contains(archive));
    let source = source.replace(archive, r#"fs::read(root.join("worker_archive")).unwrap()"#);
    let running = r#"if purpose == "run" && case == "broker-crash-running" {"#;
    assert!(source.contains(running));
    let source = source.replace(
        running,
        r#"if purpose == "run" && case == "file-audit"
                && root.join("exit_after_running_ack").exists()
            {
                set_state(&instance, "running");
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                while !root.join("running_ack").exists() {
                    if std::time::Instant::now() >= deadline { std::process::exit(125); }
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                set_state(&instance, "exited");
            } else if purpose == "run" && case == "broker-crash-running" {"#,
    );
    let inspection = "            let process = format!(";
    assert!(source.contains(inspection));
    let source = source.replace(inspection, r#"            if state == "exited"
                && fs::read_to_string(instance.join("purpose")).unwrap() == "run"
                && root.join("hold_terminal_inspection").exists()
            {
                fs::write(root.join("terminal_inspection_waiting"), "runtime exited before final inspection returned").unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
                while !root.join("release_terminal_inspection").exists() {
                    if std::time::Instant::now() >= deadline { std::process::exit(125); }
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
            let process = format!("#);
    let source_path = parent.join("engine.rs");
    let program = parent.join("engine");
    fs::write(&source_path, source).unwrap();
    let output = std::process::Command::new(
        std::env::var_os("RUSTC")
            .map(PathBuf::from)
            .unwrap_or_else(|| "rustc".into()),
    )
    .args([
        "--edition=2024",
        "--crate-name",
        "file_audit_engine_fixture",
    ])
    .arg(source_path)
    .arg("-o")
    .arg(&program)
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "fixture compilation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    program
}

fn worker_archive() -> Vec<u8> {
    let mut archive = tar::Builder::new(Vec::new());
    for (name, bytes) in [
        ("original.txt", &b"unchanged project input\n"[..]),
        ("planted.txt", &b"synthetic worker change\n"[..]),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        archive.append_data(&mut header, name, bytes).unwrap();
    }
    archive.into_inner().unwrap()
}

#[derive(Default)]
struct CaptureGate {
    reject_declared: AtomicBool,
    reject_once: AtomicBool,
    lose_file_ack: AtomicBool,
    reject_control: AtomicBool,
    hold_terminal: AtomicBool,
    terminal_waiting: AtomicBool,
    release_terminal: AtomicBool,
    fail_monitor: AtomicBool,
    monitor_refused: AtomicBool,
    reject_started_once: AtomicBool,
    finish_after_started_ack: AtomicBool,
    started_after_running: AtomicUsize,
    refused: AtomicUsize,
    event_ids: Mutex<Vec<String>>,
    file_requests: Mutex<Vec<Vec<u8>>>,
    started_requests: Mutex<Vec<(String, Vec<u8>)>>,
}

struct Scenario {
    root: PathBuf,
    config: PathBuf,
    project: PathBuf,
    program: PathBuf,
    url: String,
    runner: String,
    store: Arc<Store>,
    gate: Arc<CaptureGate>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Scenario {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Scenario {
    async fn new(parent: &Path, binary: &Path, name: &str, lines: usize) -> Self {
        let root = parent.join(name);
        std::os::unix::fs::DirBuilderExt::mode(&mut fs::DirBuilder::new(), 0o700)
            .create(&root)
            .unwrap();
        let config = root.join("config");
        initialize_config(&config).unwrap();
        let project = root.join("project");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("original.txt"), b"unchanged project input\n").unwrap();
        let control = root.join("control");
        fs::create_dir(&control).unwrap();
        let program = control.join("docker");
        fs::copy(binary, &program).unwrap();
        let metadata = program.with_extension("fixture");
        fs::create_dir(&metadata).unwrap();
        fs::write(metadata.join("root"), root.to_str().unwrap()).unwrap();
        fs::write(metadata.join("case"), "file-audit").unwrap();
        fs::write(root.join("telemetry_lines"), lines.to_string()).unwrap();
        fs::write(root.join("run_exit_code"), "0").unwrap();
        fs::write(root.join("worker_archive"), worker_archive()).unwrap();
        let runner = read_token(&config.join("secrets/runner")).unwrap();
        let viewer = read_token(&config.join("secrets/viewer")).unwrap();
        let redactor = Redactor::new(vec![runner.clone(), viewer.clone()]);
        let store =
            Arc::new(Store::open(&root.join("audit.sqlite"), redactor.clone(), 5000).unwrap());
        let gateway = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: "local".into(),
                store: store.clone(),
                runner_token: runner.clone(),
                viewer_token: viewer,
            }],
            Policy::default(),
            None,
            redactor,
        )
        .unwrap();
        let gate = Arc::new(CaptureGate::default());
        let shared_gate = gate.clone();
        let event_root = root.clone();
        let app = gateway_router(gateway).layer(middleware::from_fn(
            move |request: Request, next: Next| {
                let gate = shared_gate.clone();
                let root = event_root.clone();
                async move {
                    if request.method() == Method::GET
                        && request.uri().path().starts_with("/control/api/v1/workers/")
                        && root.join("running_ack").exists()
                        && gate.fail_monitor.load(Ordering::SeqCst)
                        && !gate.monitor_refused.swap(true, Ordering::SeqCst)
                    {
                        return StatusCode::SERVICE_UNAVAILABLE.into_response();
                    }
                    if request.method() != Method::POST {
                        return next.run(request).await;
                    }
                    let transition = request.uri().path().ends_with("/transitions");
                    let event_route = request.uri().path().ends_with("/events");
                    let (parts, body) = request.into_parts();
                    let bytes = to_bytes(body, 256 * 1024).await.unwrap();
                    let value: Value = serde_json::from_slice(&bytes).unwrap();
                    if parts.uri.path() == "/control/api/v1/control/events" && gate.reject_control.load(Ordering::SeqCst) {
                        return StatusCode::NOT_FOUND.into_response();
                    }
                    let file_event=event_route && value["kind"]=="file.created";
                    if file_event {gate.file_requests.lock().unwrap().push(bytes.to_vec());}
                    let started_event=event_route && value["kind"]=="command.started";
                    if started_event {
                        gate.started_requests.lock().unwrap().push((parts.uri.path().into(),bytes.to_vec()));
                        if root.join("running_ack").exists() {gate.started_after_running.fetch_add(1,Ordering::SeqCst);}
                        if gate.reject_started_once.swap(false,Ordering::SeqCst) {
                            gate.refused.fetch_add(1,Ordering::SeqCst);
                            return (StatusCode::TOO_MANY_REQUESTS,[("retry-after","1")],axum::Json(json!({"error":{"code":"write_rate_limited"}}))).into_response();
                        }
                    }
                    if event_route && value["kind"] == "retry.probe" {
                        gate.event_ids
                            .lock()
                            .unwrap()
                            .push(value["id"].as_str().unwrap().into());
                        if gate.reject_once.swap(false, Ordering::SeqCst) {
                            gate.refused.fetch_add(1, Ordering::SeqCst);
                            return (
                                StatusCode::TOO_MANY_REQUESTS,
                                [("retry-after", "1")],
                                axum::Json(json!({"error":"synthetic transient write window"})),
                            )
                                .into_response();
                        }
                    }
                    if event_route
                        && value["source"] == "generic-adapter"
                        && gate.reject_declared.load(Ordering::SeqCst)
                    {
                        gate.refused.fetch_add(1, Ordering::SeqCst);
                        return (
                            StatusCode::TOO_MANY_REQUESTS,
                            [("retry-after", "1")],
                            axum::Json(json!({"error":"synthetic held ingestion window"})),
                        )
                            .into_response();
                    }
                    let mutation = transition
                        .then(|| serde_json::from_value::<WorkerMutationV1>(value).ok())
                        .flatten();
                    if gate.hold_terminal.load(Ordering::SeqCst) && mutation.as_ref().is_some_and(|mutation|matches!(&mutation.transition,WorkerTransitionV1::Observe{observation} if observation.running==Some(false) && observation.finished_at.is_some())) {
                        gate.terminal_waiting.store(true,Ordering::SeqCst);
                        while !gate.release_terminal.load(Ordering::SeqCst) {tokio::time::sleep(Duration::from_millis(2)).await;}
                    }
                    let response = next
                        .run(Request::from_parts(parts, Body::from(bytes)))
                        .await;
                    if file_event && response.status().is_success() && gate.lose_file_ack.swap(false,Ordering::SeqCst) {
                        return StatusCode::SERVICE_UNAVAILABLE.into_response();
                    }
                    if response.status().is_success() && mutation.as_ref().is_some_and(|mutation| {
                        matches!(&mutation.transition, WorkerTransitionV1::Observe { observation }
                                if observation.running == Some(false)
                                    && observation.started_at.is_some()
                                    && observation.finished_at.is_some())
                    }) {
                        fs::write(root.join("terminal_ack"), "real durable terminal").unwrap();
                    }
                    if response.status().is_success() && mutation.as_ref().is_some_and(|mutation|matches!(&mutation.transition, WorkerTransitionV1::Observe{observation} if observation.running==Some(true))) {
                        fs::write(root.join("running_ack"),"real durable running observation").unwrap();
                    }
                    if started_event && response.status().is_success() && gate.finish_after_started_ack.load(Ordering::SeqCst) {
                        let id=fs::read_to_string(root.join("run_id")).unwrap();
                        fs::write(root.join("containers").join(id).join("state"),"exited").unwrap();
                    }
                    response
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            root,
            config,
            project,
            program,
            url,
            runner,
            store,
            gate,
            server,
        }
    }

    fn command(&self, action: &str) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        command
            .arg(action)
            .arg("--config-dir")
            .arg(&self.config)
            .args(["--gateway", &self.url])
            .arg("--runtime-program")
            .arg(&self.program)
            .kill_on_drop(true);
        if action == "run" {
            command
                .args([
                    "--profile",
                    "observe",
                    "--image",
                    "worker:fixture",
                    "--runtime-endpoint",
                    &format!("unix://{}", self.root.join("fixture.sock").display()),
                ])
                .arg("--project")
                .arg(&self.project)
                .args(["--", "/usr/bin/id"]);
        } else {
            let workers = self.store.workers().unwrap();
            command.args(["--session", &workers[0].launch.session_id]);
            command.args(["--wait-seconds", "0"]);
        }
        command
    }

    async fn run(&self, action: &str) -> std::process::Output {
        tokio::time::timeout(PROCESS_DEADLINE, self.command(action).output())
            .await
            .expect("bounded CLI observation expired")
            .unwrap()
    }

    fn events(&self) -> Vec<StoredEvent> {
        let mut items = Vec::new();
        let mut after = 0;
        loop {
            let page = self
                .store
                .events(&EventQuery {
                    after: Some(after),
                    limit: Some(200),
                    order: Some("asc".into()),
                    ..Default::default()
                })
                .unwrap();
            let Some(last) = page.last() else {
                return items;
            };
            after = last.seq;
            items.extend(page);
        }
    }

    fn checkpoint(&self) -> Value {
        let worker = self.store.workers().unwrap().remove(0);
        serde_json::from_slice(
            &fs::read(
                self.config
                    .join("sessions")
                    .join(format!("{}.json", worker.launch.session_id)),
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn journal_path(&self) -> PathBuf {
        let worker = self.store.workers().unwrap().remove(0);
        self.config.join("sessions").join(format!(
            "{}.postprocessing-v1.json",
            worker.launch.session_id
        ))
    }
}

fn assert_trusted_file_events(events: &[StoredEvent]) {
    let created = events
        .iter()
        .filter(|row| row.event.kind == "file.created" && row.event.data["path"] == "planted.txt")
        .collect::<Vec<_>>();
    assert_eq!(created.len(), 1, "one real observed change must be durable");
    assert_eq!(created[0].event.source, "runner");
    let completed = events
        .iter()
        .filter(|row| row.event.kind == "workspace.diff.completed")
        .collect::<Vec<_>>();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].event.data["changed_files"], 1);
    assert_eq!(completed[0].event.data["emitted_changes"], 1);
    assert_eq!(completed[0].event.data["truncated"], false);
    assert!(
        events
            .iter()
            .any(|row| row.event.kind == "observer.coverage")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_cli_control_records_observed_file_created_and_diff_completed() {
    let parent = support::private_tempdir();
    let binary = fixture_binary(parent.path());
    let scenario = Scenario::new(parent.path(), &binary, "control", 0).await;
    let output = scenario.run("run").await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_trusted_file_events(&scenario.events());
    assert_eq!(
        fs::read(scenario.project.join("original.txt")).unwrap(),
        b"unchanged project input\n"
    );
    assert!(!scenario.project.join("planted.txt").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declared_refusal_retains_trusted_diff_and_explicit_partial_coverage() {
    let parent = support::private_tempdir();
    let binary = fixture_binary(parent.path());
    let scenario = Scenario::new(parent.path(), &binary, "declared-refused", 500).await;
    scenario.gate.reject_declared.store(true, Ordering::SeqCst);
    let output = scenario.run("run").await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("429"));
    assert!(scenario.gate.refused.load(Ordering::SeqCst) > 0);
    let workers = scenario.store.workers().unwrap();
    assert_eq!(workers[0].state, WorkerStateV1::Terminal);
    assert!(!workers[0].quota_held);
    assert_eq!(workers[0].cleanup_removed, Some(true));
    let before = scenario.events();
    assert_trusted_file_events(&before);
    let coverage = before
        .iter()
        .find(|row| row.event.kind == "observer.coverage")
        .unwrap();
    assert_eq!(coverage.event.coverage, "partial");
    assert_eq!(coverage.event.data["postprocessing_complete"], false);
    assert_eq!(coverage.event.data["posted_events"], 0);
    assert_eq!(coverage.event.data["dropped_events"], 500);
    assert!(scenario.checkpoint()["managed"]["result"]["postprocessing_error"].is_string());
    // Remove the synthetic refusal before resume. Any continued absence is the
    // product's recovery behavior, not a second refusal from this harness.
    scenario.gate.reject_declared.store(false, Ordering::SeqCst);
    let resumed = scenario.run("worker-resume").await;
    assert!(
        !resumed.status.success(),
        "unavailable declared capture must remain explicit after resume"
    );
    let resumed_error = String::from_utf8_lossy(&resumed.stderr);
    assert!(
        resumed_error.contains("worker result recorded; postprocessing failed:")
            && resumed_error.contains("429"),
        "{resumed_error}"
    );
    let after = scenario.events();
    assert_eq!(
        after.len(),
        before.len(),
        "already acknowledged trusted facts must not be duplicated"
    );
    assert!(scenario.checkpoint()["managed"]["result"]["postprocessing_error"].is_string());
    let calls = fs::read_to_string(scenario.root.join("calls")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("start ") && line.ends_with(" run"))
            .count(),
        1
    );
    assert!(!calls.lines().any(|line| line.starts_with("volume rm ")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn untyped_429_is_not_retried() {
    let parent = support::private_tempdir();
    let binary = fixture_binary(parent.path());
    let scenario = Scenario::new(parent.path(), &binary, "retry", 0).await;
    scenario.gate.reject_once.store(true, Ordering::SeqCst);
    let client = AuditClient::new(&scenario.url, &scenario.runner).unwrap();
    let event = serde_json::to_value(Event::new("retry.probe")).unwrap();
    let identity = event["id"].as_str().unwrap().to_owned();
    let result = tokio::time::timeout(Duration::from_secs(10), client.post_event(event))
        .await
        .unwrap();
    assert!(
        result.is_err(),
        "residual first-refusal behavior no longer present"
    );
    assert_eq!(*scenario.gate.event_ids.lock().unwrap(), vec![identity]);
    assert!(
        !scenario
            .events()
            .iter()
            .any(|row| row.event.kind == "retry.probe")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_file_ack_replays_exact_retained_event_without_runtime_or_duplicate_change() {
    let parent = support::private_tempdir();
    let binary = fixture_binary(parent.path());
    let scenario = Scenario::new(parent.path(), &binary, "file-ack-lost", 0).await;
    scenario.gate.lose_file_ack.store(true, Ordering::SeqCst);
    let output = scenario.run("run").await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("503"));
    let before = scenario.events();
    let original = before
        .iter()
        .find(|row| row.event.kind == "file.created")
        .unwrap();
    assert!(
        !before
            .iter()
            .any(|row| row.event.kind == "workspace.diff.completed")
    );
    assert!(
        before
            .iter()
            .any(|row| row.event.kind == "observer.coverage"
                && row.event.data["postprocessing_complete"] == false)
    );
    let pending = before
        .iter()
        .find(|row| row.event.kind == "observer.coverage")
        .unwrap();
    assert_eq!(
        pending.event.data["file_delivery"],
        json!({"version":1,"phase":"files_pending","prepared_changes":1,"acknowledged_changes":0})
    );
    assert!(
        pending
            .event
            .data
            .get("trusted_diff_emitted_changes")
            .is_none()
    );
    let calls = fs::read_to_string(scenario.root.join("calls")).unwrap();
    let resumed = scenario.run("worker-resume").await;
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let after = scenario.events();
    assert_trusted_file_events(&after);
    let complete = after
        .iter()
        .rev()
        .find(|row| row.event.kind == "observer.coverage")
        .unwrap();
    assert_eq!(
        complete.event.data["file_delivery"],
        json!({"version":1,"phase":"complete","prepared_changes":1,"acknowledged_changes":1})
    );
    assert_eq!(
        after
            .iter()
            .find(|row| row.event.kind == "file.created")
            .unwrap()
            .seq,
        original.seq
    );
    let requests = scenario.gate.file_requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    assert_eq!(
        fs::read_to_string(scenario.root.join("calls")).unwrap(),
        calls,
        "a prepared delivery replay needs no runtime invocation"
    );
    assert!(
        scenario.checkpoint()["managed"]
            .get("postprocessing")
            .is_none(),
        "strict plans1/2/3 must retain their schema"
    );
    assert!(scenario.checkpoint()["managed"]["result"]["postprocessing_error"].is_null());
    let journal: Value =
        serde_json::from_slice(&fs::read(scenario.journal_path()).unwrap()).unwrap();
    assert_eq!(journal["version"], 1);
    assert_eq!(journal["payload"]["diff"]["acknowledged_files"], 1);
    assert_eq!(journal["payload"]["diff"]["completed_acknowledged"], true);
    assert_eq!(journal["payload"]["coverage_acknowledged"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_gateway_is_refused_by_first_control_fact_before_any_runtime_invocation() {
    let parent = support::private_tempdir();
    let binary = fixture_binary(parent.path());
    let scenario = Scenario::new(parent.path(), &binary, "old-gateway", 0).await;
    scenario.gate.reject_control.store(true, Ordering::SeqCst);
    let output = scenario.run("run").await;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("404"));
    assert!(scenario.store.workers().unwrap().is_empty());
    assert!(!scenario.root.join("calls").exists());
    assert!(
        !scenario
            .events()
            .iter()
            .any(|row| row.event.kind == "command.requested")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_journal_rejects_unknown_version_identity_checksum_and_oversize() {
    let parent = support::private_tempdir();
    let binary = fixture_binary(parent.path());
    let scenario = Scenario::new(parent.path(), &binary, "journal-refusals", 0).await;
    let output = scenario.run("run").await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let path = scenario.journal_path();
    let original = fs::read(&path).unwrap();
    let journal: Value = serde_json::from_slice(&original).unwrap();
    let events = scenario.events().len();
    let calls = fs::read_to_string(scenario.root.join("calls")).unwrap();
    let mut invalid = Vec::new();
    let mut value = journal.clone();
    value["version"] = json!(2);
    invalid.push(serde_json::to_vec(&value).unwrap());
    let mut value = journal.clone();
    value["tenant_id"] = json!("other-tenant");
    invalid.push(serde_json::to_vec(&value).unwrap());
    let mut value = journal.clone();
    value["unexpected_field"] = json!(true);
    invalid.push(serde_json::to_vec(&value).unwrap());
    let mut value = journal;
    value["payload"]["diff"]["acknowledged_files"] = json!(0);
    invalid.push(serde_json::to_vec(&value).unwrap());
    invalid.push(vec![b' '; 3 * 1024 * 1024 + 1]);
    for bytes in invalid {
        fs::write(&path, bytes).unwrap();
        let resumed = scenario.run("worker-resume").await;
        assert!(!resumed.status.success(), "invalid journal was accepted");
        assert_eq!(scenario.events().len(), events);
        assert_eq!(
            fs::read_to_string(scenario.root.join("calls")).unwrap(),
            calls
        );
    }
    fs::write(&path, original).unwrap();
    assert!(scenario.run("worker-resume").await.status.success());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn process_loss_before_terminal_ack_retains_pending_and_recovers_real_diff() {
    let parent = support::private_tempdir();
    let binary = fixture_binary(parent.path());
    let scenario = Scenario::new(parent.path(), &binary, "terminal-ack-cut", 1).await;
    // on_started must observe Running. Otherwise this fixture's immediate exit
    // can produce an earlier terminal Observe before finish_execution/Pending.
    fs::write(
        scenario.root.join("exit_after_running_ack"),
        "bounded fixture phase gate",
    )
    .unwrap();
    scenario.gate.hold_terminal.store(true, Ordering::SeqCst);
    let mut child = scenario
        .command("run")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = tokio::time::Instant::now() + PROCESS_DEADLINE;
    while !scenario.gate.terminal_waiting.load(Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "terminal cut was not reached"
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "CLI exited before the cut"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(
        scenario.root.join("running_ack").exists(),
        "terminal cut preceded the durable Running receipt"
    );
    assert!(
        scenario.journal_path().is_file(),
        "terminal cut preceded Pending despite the Running phase gate"
    );
    let journal: Value =
        serde_json::from_slice(&fs::read(scenario.journal_path()).unwrap()).unwrap();
    assert_eq!(journal["payload"]["declared"]["state"], "pending");
    assert_eq!(journal["payload"]["declared"]["normalized_events"], 1);
    assert!(journal["payload"]["diff"].is_null());
    let checkpoint = scenario.checkpoint();
    assert!(checkpoint["managed"]["result"].is_object());
    assert!(checkpoint["managed"]["output_event"].is_object());
    assert_eq!(
        journal["output_event_digest"],
        agentsandbox::control::content_hash(
            "ai-guard:postprocessing-output:v1",
            &checkpoint["managed"]["output_event"]
        )
        .unwrap()
    );
    assert!(
        !scenario
            .events()
            .iter()
            .any(|row| row.event.kind == "workspace.diff.completed")
    );
    child.kill().await.unwrap();
    scenario.gate.release_terminal.store(true, Ordering::SeqCst);
    let resumed = scenario.run("worker-resume").await;
    assert!(
        !resumed.status.success(),
        "lost declared capture must remain partial"
    );
    let after = scenario.events();
    assert_trusted_file_events(&after);
    let completed = after
        .iter()
        .find(|row| row.event.kind == "workspace.diff.completed")
        .unwrap();
    assert_eq!(
        completed.event.data["observation_phase"],
        "post_completion_recovery"
    );
    let journal: Value =
        serde_json::from_slice(&fs::read(scenario.journal_path()).unwrap()).unwrap();
    assert_eq!(
        journal["payload"]["declared"]["state"],
        "unavailable_after_interruption"
    );
    assert!(journal["payload"]["declared"]["acknowledged_events"].is_null());
    let calls = fs::read_to_string(scenario.root.join("calls")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("start ") && line.ends_with(" run"))
            .count(),
        1
    );
    assert!(!calls.lines().any(|line| line.starts_with("volume rm ")));
}

// Initial reviewer reproductions are preserved separately with their REDs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_escaped_path_keeps_trusted_file_events_and_diff_completion() {
    let parent = support::private_tempdir();
    let binary = fixture_binary(parent.path());
    let scenario = Scenario::new(parent.path(), &binary, "escaped-path-budget", 0).await;
    let mut components = vec!["\u{1}".repeat(128); 31];
    components.push("\u{1}".repeat(97));
    let path = components.join("/");
    assert_eq!(path.len(), 4096);
    agentsandbox::workspace::validate_relative_path(Path::new(&path)).unwrap();
    let mut probe = Event::new("payload.size.probe");
    probe.source = "size-proof".into();
    probe.data = json!({"path":path});
    probe.validate().unwrap();
    let bytes = serde_json::to_vec(&probe).unwrap();
    assert!(bytes.len() > 12 * 1024 && bytes.len() < 120 * 1024);
    // Positive HTTP/SQLite control: escaping does not exceed the existing
    // Event/transport limit. This is a size probe, not a claimed worker change.
    AuditClient::new(&scenario.url, &scenario.runner)
        .unwrap()
        .post_event(json!(probe))
        .await
        .unwrap();
    let mut archive = tar::Builder::new(Vec::new());
    for (name, body) in [
        ("original.txt", &b"unchanged project input\n"[..]),
        ("planted.txt", &b"synthetic worker change\n"[..]),
        (path.as_str(), &b"valid synthetic path\n"[..]),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        archive.append_data(&mut header, name, body).unwrap();
    }
    fs::write(
        scenario.root.join("worker_archive"),
        archive.into_inner().unwrap(),
    )
    .unwrap();
    let output = scenario.run("run").await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events = scenario.events();
    assert!(
        events
            .iter()
            .any(|row| row.event.kind == "workspace.scan.completed")
    );
    let files = events
        .iter()
        .filter(|row| row.event.kind == "file.created")
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 2);
    assert!(files.iter().any(|row| row.event.data["path"] == path));
    let completed = events
        .iter()
        .find(|row| row.event.kind == "workspace.diff.completed")
        .unwrap();
    assert_eq!(completed.event.data["changed_files"], 2);
    assert_eq!(completed.event.data["emitted_changes"], 2);
    assert_eq!(completed.event.data["omitted_changes"], 0);
    assert_eq!(completed.event.data["truncated"], false);
    assert!(fs::metadata(scenario.journal_path()).unwrap().len() <= 3 * 1024 * 1024);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn process_loss_before_pending_recovers_diff_and_reports_unknown_capture() {
    let parent = support::private_tempdir();
    let binary = fixture_binary(parent.path());
    let scenario = Scenario::new(parent.path(), &binary, "before-pending-cut", 1).await;
    fs::write(
        scenario.root.join("hold_terminal_inspection"),
        "bounded fixture cut",
    )
    .unwrap();
    let mut child = scenario
        .command("run")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = tokio::time::Instant::now() + PROCESS_DEADLINE;
    while !scenario.root.join("terminal_inspection_waiting").exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "pre-Pending inspection cut not reached"
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "CLI exited before pre-Pending cut"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let checkpoint = scenario.checkpoint();
    assert!(checkpoint["managed"]["result"].is_null());
    assert!(checkpoint["managed"]["output_event"].is_null());
    assert!(!scenario.journal_path().exists());
    let run_id = fs::read_to_string(scenario.root.join("run_id")).unwrap();
    assert_eq!(
        fs::read_to_string(scenario.root.join("containers").join(run_id).join("state")).unwrap(),
        "exited"
    );
    child.kill().await.unwrap();
    fs::write(
        scenario.root.join("release_terminal_inspection"),
        "release fixture child",
    )
    .unwrap();
    let resumed = scenario.run("worker-resume").await;
    assert!(
        !resumed.status.success(),
        "missing declared capture must remain an explicit recovery error: {}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let worker = scenario.store.workers().unwrap().remove(0);
    assert_eq!(worker.state, WorkerStateV1::Terminal);
    assert!(!worker.quota_held);
    assert_eq!(worker.cleanup_removed, Some(true));
    assert!(scenario.journal_path().exists());
    let events = scenario.events();
    assert_trusted_file_events(&events);
    let journal: Value =
        serde_json::from_slice(&fs::read(scenario.journal_path()).unwrap()).unwrap();
    assert_eq!(
        journal["payload"]["declared"]["state"],
        "unavailable_after_interruption"
    );
    for field in [
        "normalized_events",
        "acknowledged_events",
        "dropped_events",
        "unknown_delivery_events",
        "not_attempted_events",
    ] {
        assert!(journal["payload"]["declared"][field].is_null());
    }
    assert!(scenario.checkpoint()["managed"]["result"].is_null());
    assert!(scenario.checkpoint()["managed"]["output_event"].is_null());
    let calls = fs::read_to_string(scenario.root.join("calls")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("start ") && line.ends_with(" run"))
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_preserves_monitor_failure_owned_outside_the_journal() {
    let parent = support::private_tempdir();
    let binary = fixture_binary(parent.path());
    let scenario = Scenario::new(parent.path(), &binary, "external-monitor-diagnostic", 0).await;
    fs::write(
        scenario.program.with_extension("fixture").join("case"),
        "broker-crash-running",
    )
    .unwrap();
    scenario.gate.fail_monitor.store(true, Ordering::SeqCst);
    let output = scenario.run("run").await;
    assert!(!output.status.success());
    assert!(scenario.gate.monitor_refused.load(Ordering::SeqCst));
    let checkpoint = scenario.checkpoint();
    assert!(
        checkpoint["managed"]["result"]["postprocessing_error"]
            .as_str()
            .unwrap()
            .contains("execution observation failed: legacy control supervision failed:")
    );
    let journal: Value =
        serde_json::from_slice(&fs::read(scenario.journal_path()).unwrap()).unwrap();
    assert_eq!(
        journal["payload"]["coverage"]["data"]["postprocessing_complete"],
        true
    );
    let before = scenario.events().len();
    let calls = fs::read_to_string(scenario.root.join("calls")).unwrap();
    let resumed = scenario.run("worker-resume").await;
    assert!(
        !resumed.status.success(),
        "an external execution diagnostic must remain an error: {}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(
        scenario.checkpoint()["managed"]["result"]["postprocessing_error"],
        checkpoint["managed"]["result"]["postprocessing_error"]
    );
    assert_eq!(scenario.events().len(), before);
    assert_eq!(
        fs::read_to_string(scenario.root.join("calls")).unwrap(),
        calls
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_started_callback_uses_one_attempt_and_stops_its_owned_instance() {
    let parent = support::private_tempdir();
    let binary = fixture_binary(parent.path());
    let scenario = Scenario::new(parent.path(), &binary, "started-callback-retry", 0).await;
    fs::write(
        scenario.program.with_extension("fixture").join("case"),
        "broker-crash-running",
    )
    .unwrap();
    scenario
        .gate
        .reject_started_once
        .store(true, Ordering::SeqCst);
    scenario
        .gate
        .finish_after_started_ack
        .store(true, Ordering::SeqCst);
    let output = scenario.run("run").await;
    assert!(
        !output.status.success(),
        "the mandatory callback must refuse its first failed audit: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(scenario.root.join("running_ack").exists());
    assert_eq!(scenario.gate.refused.load(Ordering::SeqCst), 1);
    assert_eq!(
        scenario.gate.started_after_running.load(Ordering::SeqCst),
        1
    );
    let requests = scenario.gate.started_requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        1,
        "the mandatory callback retried its failed audit"
    );
    assert!(
        requests
            .iter()
            .all(|(path, _)| path == "/control/api/v1/control/events")
    );
    let events = scenario.events();
    let started = events
        .iter()
        .filter(|row| row.event.kind == "command.started")
        .collect::<Vec<_>>();
    assert!(started.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("429"));
    let calls = fs::read_to_string(scenario.root.join("calls")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("start ") && line.ends_with(" run"))
            .count(),
        1
    );
    assert!(
        calls
            .lines()
            .filter(|line| line.starts_with("kill "))
            .count()
            >= 1,
        "failed mandatory audit did not request the owned instance stop"
    );
    let worker = scenario.store.workers().unwrap().remove(0);
    assert!(worker.attempt.start_operation_id.is_some());
    assert!(
        worker.quota_held,
        "a consumed start requires terminal reconciliation before quota release"
    );
    assert_ne!(worker.cleanup_removed, Some(true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_prefix_respects_global_journal_bytes_and_reports_omissions() {
    let parent = support::private_tempdir();
    let binary = fixture_binary(parent.path());
    let scenario = Scenario::new(parent.path(), &binary, "file-byte-prefix", 0).await;
    let mut archive = tar::Builder::new(Vec::new());
    let mut append = |path: &str, bytes: &[u8]| {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        archive.append_data(&mut header, path, bytes).unwrap();
    };
    append("original.txt", b"unchanged project input\n");
    append("planted.txt", b"synthetic worker change\n");
    for index in 0..190 {
        let mut components = vec!["\u{1}".repeat(128); 31];
        components.push(format!("{}{:03}", "\u{1}".repeat(94), index));
        let path = components.join("/");
        assert_eq!(path.len(), 4096);
        append(&path, b"valid bounded file\n");
    }
    fs::write(
        scenario.root.join("worker_archive"),
        archive.into_inner().unwrap(),
    )
    .unwrap();
    // This byte-budget case deliberately retains 190 maximally escaped paths.
    // Give only this external test observation a larger bound; the CLI's
    // runtime and HTTP timeouts, fixture inputs and functional oracles stay put.
    let observations = Arc::new(Mutex::new(Vec::new()));
    let snapshots = observations.clone();
    let (root, config, store, gate) = (
        scenario.root.clone(),
        scenario.config.clone(),
        scenario.store.clone(),
        scenario.gate.clone(),
    );
    let (stop_diagnostics, mut stopped) = tokio::sync::oneshot::channel();
    let diagnostics = tokio::spawn(async move {
        let started = tokio::time::Instant::now();
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                biased;
                _ = &mut stopped => break,
                _ = interval.tick() => {}
            }
            let (root, config, store, gate) =
                (root.clone(), config.clone(), store.clone(), gate.clone());
            let elapsed = started.elapsed();
            // A SQLite/filesystem diagnostic must not block the 240s timer.
            let snapshot = tokio::task::spawn_blocking(move || {
                file_prefix_progress(&root, &config, &store, &gate, elapsed)
            })
            .await
            .unwrap_or_else(
                |_| json!({"elapsed_seconds":elapsed.as_secs(),"diagnostic":"unavailable"}),
            );
            snapshots.lock().unwrap().push(snapshot);
        }
    });
    let cli_started = tokio::time::Instant::now();
    let output =
        tokio::time::timeout(Duration::from_secs(240), scenario.command("run").output()).await;
    let cli_observed = cli_started.elapsed();
    let _ = stop_diagnostics.send(());
    diagnostics
        .await
        .expect("byte-prefix diagnostics did not stop cleanly");
    let diagnostic = json!({
        "cli_observed_milliseconds": cli_observed.as_millis(),
        "snapshots": *observations.lock().unwrap()
    });
    let output = match output {
        Ok(output) => {
            eprintln!("byte-prefix CLI observation: {diagnostic}");
            output.unwrap()
        }
        Err(_) => {
            panic!("bounded byte-prefix CLI observation expired: {diagnostic}");
        }
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events = scenario.events();
    let completed = events
        .iter()
        .find(|row| row.event.kind == "workspace.diff.completed")
        .unwrap();
    let emitted = completed.event.data["emitted_changes"].as_u64().unwrap();
    assert!(emitted > 0 && emitted < 191);
    assert_eq!(completed.event.data["changed_files"], 191);
    assert_eq!(completed.event.data["omitted_changes"], 191 - emitted);
    assert_eq!(completed.event.data["truncated"], true);
    assert_eq!(
        completed.event.data["omission_reasons"],
        json!(["journal_byte_budget"])
    );
    assert_eq!(
        events
            .iter()
            .filter(|row| row.event.kind == "file.created")
            .count() as u64,
        emitted
    );
    let coverage = events
        .iter()
        .find(|row| row.event.kind == "observer.coverage")
        .unwrap();
    assert_eq!(coverage.event.coverage, "partial");
    assert_eq!(coverage.event.data["trusted_diff_events_truncated"], true);
    assert_eq!(coverage.event.data["trusted_diff_changed_files"], 191);
    assert_eq!(
        coverage.event.data["file_delivery"],
        json!({"version":1,"phase":"complete","prepared_changes":emitted,"acknowledged_changes":emitted})
    );
    assert!(fs::metadata(scenario.journal_path()).unwrap().len() <= 3 * 1024 * 1024);
}

// Diagnostics for the single byte-budget parent above. Never print Event data,
// raw fixture arguments, IDs, paths, journal errors or child output on timeout.
fn file_prefix_progress(
    root: &Path,
    config: &Path,
    store: &Store,
    gate: &CaptureGate,
    elapsed: Duration,
) -> Value {
    use std::io::Read;
    #[derive(serde::Deserialize)]
    struct PostedIdentity {
        id: uuid::Uuid,
    }

    let events = store
        .events(&EventQuery {
            limit: Some(1000),
            order: Some("asc".into()),
            ..Default::default()
        })
        .ok();
    let mut kinds = std::collections::BTreeMap::<&str, usize>::new();
    for row in events.iter().flatten() {
        let kind = match row.event.kind.as_str() {
            "command.requested" => "command.requested",
            "command.started" => "command.started",
            "command.completed" => "command.completed",
            "command.failed" => "command.failed",
            "runtime.submitted" => "runtime.submitted",
            "workspace.import.requested" => "workspace.import.requested",
            "workspace.import.created" => "workspace.import.created",
            "workspace.import.started" => "workspace.import.started",
            "workspace.import.completed" => "workspace.import.completed",
            "workspace.scan.requested" => "workspace.scan.requested",
            "workspace.scan.created" => "workspace.scan.created",
            "workspace.scan.started" => "workspace.scan.started",
            "workspace.scan.completed" => "workspace.scan.completed",
            "workspace.diff.completed" => "workspace.diff.completed",
            "file.created" => "file.created",
            "observer.coverage" => "observer.coverage",
            _ => "other",
        };
        *kinds.entry(kind).or_default() += 1;
    }
    let workers = store.workers();
    let workers_readable = workers.is_ok();
    let worker = workers.ok().and_then(|workers| workers.into_iter().next());
    let journal_path = worker
        .as_ref()
        .and_then(|worker| uuid::Uuid::parse_str(&worker.launch.session_id).ok())
        .map(|session| {
            config
                .join("sessions")
                .join(format!("{session}.postprocessing-v1.json"))
        });
    let journal_size = journal_path
        .as_ref()
        .and_then(|path| fs::metadata(path).ok())
        .map(|metadata| metadata.len());
    let mut journal_state = if workers_readable {
        "no_admitted_worker"
    } else {
        "worker_query_unavailable"
    };
    let mut journal = None;
    if let Some(path) = &journal_path {
        match fs::File::open(path) {
            Ok(file) => {
                let mut bytes = Vec::new();
                if file
                    .take(3 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .is_err()
                {
                    journal_state = "unreadable";
                } else if bytes.len() > 3 * 1024 * 1024 {
                    journal_state = "oversize";
                } else {
                    journal = serde_json::from_slice::<Value>(&bytes).ok();
                    journal_state = if journal.is_some() {
                        "readable"
                    } else {
                        "invalid_json"
                    };
                }
            }
            Err(error) => {
                journal_state = if error.kind() == std::io::ErrorKind::NotFound {
                    "missing"
                } else {
                    "unreadable"
                };
            }
        }
    }
    let diff = journal
        .as_ref()
        .and_then(|value| value["payload"]["diff"].as_object());
    let prepared = diff
        .and_then(|value| value.get("files")?.as_array())
        .map(Vec::len);
    let acknowledged = diff.and_then(|value| value.get("acknowledged_files")?.as_u64());
    let summary_acknowledged =
        diff.and_then(|value| value.get("completed_acknowledged")?.as_bool());
    let phase = match (prepared, acknowledged, summary_acknowledged) {
        _ if !workers_readable
            || events.is_none()
            || matches!(journal_state, "unreadable" | "oversize" | "invalid_json") =>
        {
            "unknown"
        }
        (Some(prepared), Some(acknowledged), _) if acknowledged < prepared as u64 => {
            "file_delivery"
        }
        (Some(_), Some(_), Some(false)) => "diff_summary_delivery",
        (Some(_), Some(_), Some(true)) => "files_and_diff_acknowledged",
        _ if kinds.contains_key("workspace.scan.completed") => "preparing_file_batch",
        _ if kinds.contains_key("workspace.scan.requested") => "workspace_scan",
        _ if journal.is_some() => "pending_before_scan",
        _ => "execution_preparation_or_before_pending",
    };
    let (file_posts, distinct_file_ids, invalid_file_ids) = {
        let requests = gate.file_requests.lock().unwrap();
        let mut ids = std::collections::HashSet::new();
        let mut invalid = 0;
        for bytes in requests.iter() {
            if let Ok(event) = serde_json::from_slice::<PostedIdentity>(bytes) {
                ids.insert(event.id);
            } else {
                invalid += 1;
            }
        }
        (requests.len(), ids.len(), invalid)
    };
    let calls = fs::read_to_string(root.join("calls")).ok();
    let words = calls
        .as_deref()
        .and_then(|calls| calls.lines().last())
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .unwrap_or_default();
    let verb = match words.first().copied() {
        Some("info") => "info",
        Some("image") => "image",
        Some("volume") => "volume",
        Some("create") => "create",
        Some("start") => "start",
        Some("inspect") => "inspect",
        Some("kill") => "kill",
        Some("rm") => "rm",
        Some("ps") => "ps",
        _ => "unavailable_or_other",
    };
    let purpose = words
        .get(1)
        .filter(|id| id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .and_then(|id| fs::read_to_string(root.join("containers").join(id).join("purpose")).ok());
    let purpose = match purpose.as_deref().or_else(|| words.last().copied()) {
        Some("run") => "run",
        Some("import") => "import",
        Some("export") => "export",
        _ => "unavailable_or_other",
    };
    json!({
        "elapsed_seconds": elapsed.as_secs(), "inferred_phase": phase,
        "durable_events_readable": events.is_some(), "durable_events_sample_limit": 1000,
        "durable_events_sample_count": events.as_ref().map(Vec::len), "durable_kinds": kinds,
        "workers_readable": workers_readable,
        "worker_state": worker.as_ref().map(|worker| &worker.state),
        "journal_state": journal_state, "journal_bytes": journal_size,
        "prepared_files": prepared, "acknowledged_files": acknowledged,
        "diff_summary_acknowledged": summary_acknowledged,
        "coverage_acknowledged": journal.as_ref().and_then(|value| value["payload"]["coverage_acknowledged"].as_bool()),
        "journal_diff_error_present": journal.as_ref().map(|value| !value["payload"]["diff_error"].is_null()),
        "file_post_attempts": file_posts, "file_post_distinct_ids": distinct_file_ids,
        "file_post_invalid_ids": invalid_file_ids,
        "file_post_repeated_known_ids": file_posts - distinct_file_ids - invalid_file_ids,
        "fixture_call_count": calls.as_ref().map(|calls| calls.lines().count()),
        "last_fixture_command": {"verb":verb,"purpose":purpose}
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn summary_projection_recovers_injected_states_before_and_after_plan_write() {
    #[derive(serde::Deserialize)]
    struct RawJournal {
        payload: Box<serde_json::value::RawValue>,
        payload_digest: String,
    }
    let parent = support::private_tempdir();
    let binary = fixture_binary(parent.path());
    for before_plan_write in [false, true] {
        let scenario = Scenario::new(
            parent.path(),
            &binary,
            if before_plan_write {
                "projection-before-plan"
            } else {
                "projection-after-plan"
            },
            0,
        )
        .await;
        scenario.gate.lose_file_ack.store(true, Ordering::SeqCst);
        assert!(!scenario.run("run").await.status.success());
        let original = fs::read_to_string(scenario.journal_path()).unwrap();
        let journal: Value = serde_json::from_str(&original).unwrap();
        assert_eq!(
            journal["payload"]["summary_projection"]["acknowledged"],
            true
        );
        assert!(journal["payload"]["summary_projection"]["intended"].is_string());
        let raw: RawJournal = serde_json::from_str(&original).unwrap();
        let edited =
            raw.payload
                .get()
                .replacen("\"acknowledged\":true", "\"acknowledged\":false", 1);
        assert_ne!(edited, raw.payload.get());
        let pending = serde_json::value::RawValue::from_string(edited).unwrap();
        let digest =
            agentsandbox::control::content_hash("ai-guard:postprocessing-payload:v1", &pending)
                .unwrap();
        let pending_journal = original
            .replace(raw.payload.get(), pending.get())
            .replace(&raw.payload_digest, &digest);
        fs::write(scenario.journal_path(), pending_journal).unwrap();
        if before_plan_write {
            assert_eq!(
                journal["payload"]["summary_projection"]["previous_digest"],
                agentsandbox::control::content_hash(
                    "ai-guard:summary-error:v1",
                    &Option::<String>::None
                )
                .unwrap()
            );
            let mut checkpoint = scenario.checkpoint();
            checkpoint["managed"]["result"]["postprocessing_error"] = Value::Null;
            let worker = scenario.store.workers().unwrap().remove(0);
            fs::write(
                scenario
                    .config
                    .join("sessions")
                    .join(format!("{}.json", worker.launch.session_id)),
                serde_json::to_vec(&checkpoint).unwrap(),
            )
            .unwrap();
        }
        let calls = fs::read_to_string(scenario.root.join("calls")).unwrap();
        let resumed = scenario.run("worker-resume").await;
        assert!(
            resumed.status.success(),
            "{}",
            String::from_utf8_lossy(&resumed.stderr)
        );
        assert!(scenario.checkpoint()["managed"]["result"]["postprocessing_error"].is_null());
        assert_trusted_file_events(&scenario.events());
        assert_eq!(
            fs::read_to_string(scenario.root.join("calls")).unwrap(),
            calls
        );
        let journal: Value =
            serde_json::from_slice(&fs::read(scenario.journal_path()).unwrap()).unwrap();
        assert_eq!(
            journal["payload"]["summary_projection"]["acknowledged"],
            true
        );
        assert!(journal["payload"]["summary_projection"]["intended"].is_null());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_result_without_diff_recovers_missing_postprocessing() {
    let parent = support::private_tempdir();
    let binary = fixture_binary(parent.path());
    let scenario = Scenario::new(parent.path(), &binary, "legacy-result-without-diff", 1).await;
    fs::write(
        scenario.root.join("exit_after_running_ack"),
        "bounded fixture phase gate",
    )
    .unwrap();
    scenario.gate.hold_terminal.store(true, Ordering::SeqCst);
    let mut child = scenario
        .command("run")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = tokio::time::Instant::now() + PROCESS_DEADLINE;
    while !scenario.gate.terminal_waiting.load(Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "legacy checkpoint cut was not reached"
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "CLI exited before legacy checkpoint cut"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(scenario.root.join("running_ack").exists());
    assert!(scenario.journal_path().is_file());
    let checkpoint = scenario.checkpoint();
    assert!(checkpoint["managed"]["result"].is_object());
    assert!(checkpoint["managed"]["result"]["changed_files"].is_null());
    assert!(checkpoint["managed"]["result"]["postprocessing_error"].is_null());
    child.kill().await.unwrap();
    // Model a valid historical plan that predates the separate journal. Keep
    // the new-format record intact as fixture evidence; do not claim a physical
    // crash could make a synced journal disappear, and do not rewrite the plan.
    let preserved = scenario.root.join("retained-new-format-journal.json");
    fs::rename(scenario.journal_path(), &preserved).unwrap();
    scenario.gate.release_terminal.store(true, Ordering::SeqCst);
    let worker = scenario.store.workers().unwrap().remove(0);
    let mut status = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    status
        .arg("worker-status")
        .arg("--config-dir")
        .arg(&scenario.config)
        .args([
            "--gateway",
            &scenario.url,
            "--session",
            &worker.launch.session_id,
        ])
        .kill_on_drop(true);
    let output = tokio::time::timeout(PROCESS_DEADLINE, status.output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "historical checkpoint failed validation: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let validated: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(validated["result"], checkpoint["managed"]["result"]);
    assert!(validated["postprocessing"].is_null());
    assert!(!scenario.journal_path().exists());
    let resumed = scenario.run("worker-resume").await;
    assert!(
        !resumed.status.success(),
        "unknown legacy capture must remain an explicit recovery error: {}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let worker = scenario.store.workers().unwrap().remove(0);
    assert_eq!(worker.state, WorkerStateV1::Terminal);
    assert!(!worker.quota_held);
    assert_eq!(worker.cleanup_removed, Some(true));
    let recovered = scenario.checkpoint();
    assert_eq!(recovered["managed"]["result"]["changed_files"], 1);
    assert!(recovered["managed"]["result"]["postprocessing_error"].is_string());
    for field in [
        "exit_code",
        "timed_out",
        "output_captured",
        "transport_stop_reason",
    ] {
        assert_eq!(
            recovered["managed"]["result"][field],
            checkpoint["managed"]["result"][field]
        );
    }
    assert_eq!(
        recovered["managed"]["output_event"],
        checkpoint["managed"]["output_event"]
    );
    assert!(scenario.journal_path().exists());
    let events = scenario.events();
    assert_trusted_file_events(&events);
    let journal: Value =
        serde_json::from_slice(&fs::read(scenario.journal_path()).unwrap()).unwrap();
    assert_eq!(
        journal["payload"]["declared"]["state"],
        "unavailable_after_interruption"
    );
    for field in [
        "normalized_events",
        "acknowledged_events",
        "dropped_events",
        "unknown_delivery_events",
        "not_attempted_events",
    ] {
        assert!(journal["payload"]["declared"][field].is_null());
    }
    // The second resume exercises the already-cleaned terminal shortcut too.
    let calls = fs::read_to_string(scenario.root.join("calls")).unwrap();
    assert!(!scenario.run("worker-resume").await.status.success());
    assert_eq!(scenario.events().len(), events.len());
    assert_eq!(
        fs::read_to_string(scenario.root.join("calls")).unwrap(),
        calls
    );
    assert!(preserved.is_file());
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("start ") && line.ends_with(" run"))
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redacted_path_collision_keeps_distinct_changes_and_marks_ambiguous_display() {
    // These are inert synthetic names, never real credentials. The raw name is
    // only present in the fixture archive, not retained control/audit content.
    let synthetic_token_name = "sk-abcdefghijkl";
    let literal_mask_name = "[REDACTED]";
    assert_ne!(synthetic_token_name, literal_mask_name);
    for path in [synthetic_token_name, literal_mask_name] {
        agentsandbox::workspace::validate_relative_path(Path::new(path)).unwrap();
    }
    let redactor = Redactor::new(vec![]);
    assert_eq!(redactor.text(synthetic_token_name), literal_mask_name);
    assert_eq!(redactor.text(literal_mask_name), literal_mask_name);
    let parent = support::private_tempdir();
    let binary = fixture_binary(parent.path());
    for collision in [false, true] {
        let scenario = Scenario::new(
            parent.path(),
            &binary,
            if collision {
                "redacted-path-collision"
            } else {
                "redacted-path-control"
            },
            0,
        )
        .await;
        let mut names = vec!["original.txt", "planted.txt", synthetic_token_name];
        if collision {
            names.push(literal_mask_name);
        }
        let mut archive = tar::Builder::new(Vec::new());
        for path in names {
            let body = if path == "original.txt" {
                &b"unchanged project input\n"[..]
            } else {
                &b"inert worker fixture\n"[..]
            };
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            archive.append_data(&mut header, path, body).unwrap();
        }
        fs::write(
            scenario.root.join("worker_archive"),
            archive.into_inner().unwrap(),
        )
        .unwrap();
        let output = scenario.run("run").await;
        let events = scenario.events();
        assert!(
            events
                .iter()
                .any(|row| row.event.kind == "workspace.scan.completed")
        );
        let files = events
            .iter()
            .filter(|row| row.event.kind == "file.created")
            .collect::<Vec<_>>();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected = if collision { 3 } else { 2 };
        assert_eq!(files.len(), expected);
        let neighbor = files
            .iter()
            .find(|row| row.event.data["path"] == "planted.txt")
            .unwrap();
        assert_eq!(neighbor.event.data["path_display_ambiguous"], false);
        assert_eq!(neighbor.event.data["path_redacted"], false);
        let displayed = files
            .iter()
            .filter(|row| row.event.data["path"] == literal_mask_name)
            .collect::<Vec<_>>();
        assert_eq!(displayed.len(), if collision { 2 } else { 1 });
        assert!(
            displayed
                .iter()
                .all(|row| row.event.data["path_display_ambiguous"] == collision)
        );
        assert_eq!(
            displayed
                .iter()
                .filter(|row| row.event.data["path_redacted"] == true)
                .count(),
            1
        );
        let mut identities = std::collections::HashSet::new();
        for row in &files {
            assert_eq!(row.event.data["change_id"], row.event.id);
            assert_eq!(
                uuid::Uuid::parse_str(&row.event.id)
                    .unwrap()
                    .get_version_num(),
                4
            );
            assert!(identities.insert(row.event.id.clone()));
        }
        let completed = events
            .iter()
            .find(|row| row.event.kind == "workspace.diff.completed")
            .unwrap();
        assert_eq!(completed.event.data["changed_files"], expected);
        assert_eq!(completed.event.data["emitted_changes"], expected);
        assert_eq!(completed.event.data["omitted_changes"], 0);
        assert_eq!(completed.event.data["truncated"], false);
        assert!(
            !serde_json::to_string(&events)
                .unwrap()
                .contains(synthetic_token_name)
        );
        assert!(
            !fs::read_to_string(scenario.journal_path())
                .unwrap()
                .contains(synthetic_token_name)
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains(synthetic_token_name));
        assert_eq!(
            fs::read(scenario.project.join("original.txt")).unwrap(),
            b"unchanged project input\n"
        );
        assert!(!scenario.project.join(synthetic_token_name).exists());
    }
}
