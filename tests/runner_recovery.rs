//! Recovery exercises the production CLI and SQLite gateway with a Rust engine
//! fixture. These process faults do not qualify an actual Docker/Podman engine.
#![cfg(unix)]
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    control::{QuotaPolicyV1, WorkerMutationV1, WorkerStateV1, WorkerTransitionV1},
    operations::initialize_config,
    policy::Policy,
    redaction::Redactor,
    runner::read_token,
    runtime::RuntimeAuthority,
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
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

// This bounds the test's observation of the entire CLI action, which performs
// several serial HTTP, durable SQLite and fixture-process operations. It does
// not alter the CLI's request, runtime or worker deadlines. Functional recovery
// assertions below must still succeed; expiration remains a test failure.
const CLI_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(120);

fn fixture_binary(temporary: &Path) -> PathBuf {
    let source = temporary.join("engine.rs");
    fs::write(&source, include_str!("runtime_fixtures/runner_terminal.rs")).unwrap();
    let bin = temporary.join("bin");
    fs::create_dir(&bin).unwrap();
    let compiled = std::process::Command::new(
        std::env::var_os("RUSTC")
            .map(PathBuf::from)
            .unwrap_or_else(|| "rustc".into()),
    )
    .args(["--edition=2024", "--crate-name", "recovery_engine_fixture"])
    .arg(source)
    .arg("-o")
    .arg(bin.join("docker"))
    .output()
    .unwrap();
    assert!(
        compiled.status.success(),
        "Rust fixture compilation failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    bin
}

struct Scenario {
    case: &'static str,
    root: PathBuf,
    config: PathBuf,
    project: PathBuf,
    bin: PathBuf,
    url: String,
    store: Arc<Store>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Scenario {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Scenario {
    async fn new(parent: &Path, bin: &Path, case: &'static str) -> Self {
        assert!(
            case.len() <= 64
                && case
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
            "fixture diagnostic labels must be bounded identifiers"
        );
        let root = parent.join(case);
        std::os::unix::fs::DirBuilderExt::mode(&mut fs::DirBuilder::new(), 0o700)
            .create(&root)
            .unwrap();
        let config = root.join("config");
        initialize_config(&config).unwrap();
        let project = root.join("project");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("original.txt"), "immutable original\n").unwrap();
        let control = root.join("control");
        fs::create_dir(&control).unwrap();
        let program = control.join("docker");
        fs::copy(bin.join("docker"), &program).unwrap();
        let metadata = program.with_extension("fixture");
        fs::create_dir(&metadata).unwrap();
        fs::write(metadata.join("root"), root.to_str().unwrap()).unwrap();
        fs::write(metadata.join("case"), case).unwrap();
        let runner = read_token(&config.join("secrets/runner")).unwrap();
        let viewer = read_token(&config.join("secrets/viewer")).unwrap();
        let other_runner = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let other_viewer = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        fs::write(root.join("other_runner"), &other_runner).unwrap();
        let redactor = Redactor::new(vec![
            runner.clone(),
            viewer.clone(),
            other_runner.clone(),
            other_viewer.clone(),
        ]);
        let store =
            Arc::new(Store::open(&root.join("audit.sqlite"), redactor.clone(), 1000).unwrap());
        let other_store = Arc::new(
            Store::open_for_tenant(&root.join("other.sqlite"), "other", redactor.clone(), 1000)
                .unwrap(),
        );
        let gateway = GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "local".into(),
                    store: store.clone(),
                    runner_token: runner,
                    viewer_token: viewer,
                },
                TenantGateway {
                    tenant_id: "other".into(),
                    store: other_store,
                    runner_token: other_runner,
                    viewer_token: other_viewer,
                },
            ],
            Policy::default(),
            None,
            redactor,
        )
        .unwrap();
        let faulted = Arc::new(AtomicBool::new(false));
        let event_root = root.clone();
        let app = gateway_router(gateway).layer(middleware::from_fn(move |request: Request, next: Next| {
            let root = event_root.clone();
            let faulted = faulted.clone();
            async move {
                if request.method() != Method::POST { return next.run(request).await; }
                let transitions = request.uri().path().ends_with("/transitions");
                let (parts, body) = request.into_parts();
                let bytes = to_bytes(body, 128 * 1024).await.unwrap();
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                if root.join("reject_invocation_audit").exists() && value["kind"] == "runtime.invocation.bound" {
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
                let mutation = transitions.then(|| serde_json::from_value::<WorkerMutationV1>(value.clone()).ok()).flatten();
                let lose_ack = mutation.as_ref().is_some_and(|mutation| match &mutation.transition {
                    WorkerTransitionV1::Created { .. } => case == "created-control-response-lost",
                    WorkerTransitionV1::StartClaim { .. } => case == "claim-response-lost",
                    _ => false,
                });
                if case == "import-completion-response-lost" && value["kind"] == "workspace.import.completed" && !faulted.swap(true, Ordering::SeqCst) {
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
                let response = next.run(Request::from_parts(parts, Body::from(bytes))).await;
                if response.status().is_success() && mutation.as_ref().is_some_and(|mutation| matches!(&mutation.transition, WorkerTransitionV1::Observe { observation } if observation.running == Some(false) && observation.started_at.is_some() && observation.finished_at.is_some())) {
                    fs::write(root.join("terminal_ack"), "durable terminal").unwrap();
                }
                if lose_ack && response.status().is_success() && !faulted.swap(true, Ordering::SeqCst) {
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
                response
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            case,
            root,
            config,
            project,
            bin: control,
            url,
            store,
            server,
        }
    }
    fn command(&self, action: &str) -> tokio::process::Command {
        self.command_at(action, &self.url)
    }
    fn command_at(&self, action: &str, origin: &str) -> tokio::process::Command {
        self.command_at_program(action, origin, &self.bin.join("docker"))
    }
    fn command_at_program(
        &self,
        action: &str,
        origin: &str,
        program: &Path,
    ) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        command
            .arg(action)
            .arg("--config-dir")
            .arg(&self.config)
            .args(["--gateway", origin])
            .kill_on_drop(true);
        if action != "worker-status" {
            command.arg("--runtime-program").arg(program);
        }
        if action == "run" {
            let credential = self.root.join("provider.env");
            if credential.exists() {
                command
                    .args(["--network", "bridge", "--secret-env-file"])
                    .arg(credential);
            }
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
            if action == "worker-resume" {
                command.args(["--wait-seconds", "0"]);
            }
        }
        command
    }
    async fn invoke(&self, action: &str) -> std::process::Output {
        // Only source-owned fixture labels and bounded scalar observations go
        // to diagnostics, never paths, argv, credentials or raw fixture calls.
        let action_label = match action {
            "run" => "run",
            "worker-resume" => "worker-resume",
            "worker-status" => "worker-status",
            "worker-stop" => "worker-stop",
            _ => "other",
        };
        let started = std::time::Instant::now();
        eprintln!(
            "recovery_cli case={} action={action_label} phase=started limit_ms={}",
            self.case,
            CLI_OBSERVATION_TIMEOUT.as_millis()
        );
        match tokio::time::timeout(CLI_OBSERVATION_TIMEOUT, self.command(action).output()).await {
            Ok(Ok(output)) => {
                eprintln!(
                    "recovery_cli case={} action={action_label} phase=finished elapsed_ms={} success={}",
                    self.case,
                    started.elapsed().as_millis(),
                    output.status.success()
                );
                output
            }
            Ok(Err(error)) => panic!(
                "recovery_cli case={} action={action_label} phase=process_error elapsed_ms={} kind={:?}",
                self.case,
                started.elapsed().as_millis(),
                error.kind()
            ),
            Err(_) => panic!(
                "CLI recovery observation deadline: case={} action={action_label} elapsed_ms={} limit_ms={}",
                self.case,
                started.elapsed().as_millis(),
                CLI_OBSERVATION_TIMEOUT.as_millis()
            ),
        }
    }
    fn calls(&self) -> String {
        fs::read_to_string(self.root.join("calls")).unwrap()
    }
    fn count(&self, verb: &str, purpose: &str) -> usize {
        self.calls()
            .lines()
            .filter(|line| {
                line.starts_with(&format!("{verb} ")) && line.ends_with(&format!(" {purpose}"))
            })
            .count()
    }
    fn invariant(&self) {
        assert!(self.store.integrity().unwrap().valid);
        assert_eq!(
            fs::read_to_string(self.project.join("original.txt")).unwrap(),
            "immutable original\n"
        );
    }
    fn checkpoint_path(&self) -> PathBuf {
        self.config.join("sessions").join(format!(
            "{}.json",
            self.store.workers().unwrap()[0].launch.session_id
        ))
    }
    fn legacy_invocation(&self) {
        let path = self.checkpoint_path();
        let mut plan: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        plan["managed"]["binding"]
            .as_object_mut()
            .unwrap()
            .remove("invocation");
        fs::write(path, serde_json::to_vec(&plan).unwrap()).unwrap();
    }
    fn legacy_workspace_exclusion(&self) {
        let path = self.checkpoint_path();
        let mut plan: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        plan["managed"]
            .as_object_mut()
            .unwrap()
            .remove("workspace_control_exclusion_version");
        fs::write(path, serde_json::to_vec(&plan).unwrap()).unwrap();
    }
}

#[tokio::test]
async fn legacy_prestart_workspace_without_exclusion_proof_refuses_without_runtime_or_new_claim() {
    let temporary = support::private_tempdir();
    let bin = fixture_binary(temporary.path());
    let scenario = Scenario::new(temporary.path(), &bin, "created-control-response-lost").await;
    assert!(!scenario.invoke("run").await.status.success());
    scenario.legacy_workspace_exclusion();
    let before = scenario.calls();
    let worker = scenario.store.workers().unwrap()[0].clone();
    let plan: Value =
        serde_json::from_slice(&fs::read(scenario.checkpoint_path()).unwrap()).unwrap();
    let archive = scenario
        .config
        .join("sessions")
        .join(plan["managed"]["archive_name"].as_str().unwrap());
    let archive_bytes = fs::read(&archive).unwrap();
    let rejected = scenario.invoke("worker-resume").await;
    assert!(
        !rejected.status.success(),
        "legacy workspace was started without control exclusion proof"
    );
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("control directory exclusion"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(
        scenario.calls(),
        before,
        "legacy pre-start recovery touched the engine"
    );
    let current = &scenario.store.workers().unwrap()[0];
    assert_eq!(current.worker_id, worker.worker_id);
    assert_eq!(current.attempt, worker.attempt);
    assert!(current.quota_held);
    assert!(current.attempt.start_operation_id.is_none());
    assert!(
        fs::read(&archive).unwrap() == archive_bytes,
        "legacy immutable evidence was replaced"
    );
    assert!(scenario.invoke("worker-status").await.status.success());
    scenario.invariant();
}

#[tokio::test]
async fn custom_control_directory_is_physically_excluded_from_manifest_and_worker_archive() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = support::private_tempdir();
    let bin = fixture_binary(temporary.path());
    for case_alias in [false, true] {
        let parent = temporary
            .path()
            .join(if case_alias { "case-alias" } else { "normal" });
        fs::create_dir(&parent).unwrap();
        let mut scenario = Scenario::new(&parent, &bin, "acknowledged").await;
        let control = scenario.project.join("control");
        fs::rename(&scenario.config, &control).unwrap();
        scenario.config = if case_alias {
            let alias = scenario.project.join("CONTROL");
            if !alias.exists() {
                eprintln!("NOT_APPLICABLE: case-sensitive filesystem alias variant");
                continue;
            }
            alias
        } else {
            control.clone()
        };
        let directory = agentsandbox::control_fs::Directory::open_existing(
            &agentsandbox::control_fs::absolute_path(&scenario.config).unwrap(),
        )
        .unwrap();
        let authorities = directory.child("authorities").unwrap();
        fs::write(
            authorities.path().join("fixture-seed.json"),
            b"{\"private_control_state\":true}\n",
        )
        .unwrap();
        fs::set_permissions(
            authorities.path().join("fixture-seed.json"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let provider = scenario.root.join("provider.env");
        fs::write(
            &provider,
            b"OPENAI_API_KEY=fixture-private-provider-value\n",
        )
        .unwrap();
        fs::set_permissions(&provider, fs::Permissions::from_mode(0o600)).unwrap();
        let output = scenario.invoke("run").await;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let plan: Value =
            serde_json::from_slice(&fs::read(scenario.checkpoint_path()).unwrap()).unwrap();
        assert!(
            plan["managed"]["credential_digest"].is_string(),
            "the credential snapshot was not prepared before import"
        );
        let manifest = plan["managed"]["before"].as_object().unwrap();
        let manifest_has_control = manifest
            .keys()
            .any(|path| Path::new(path).starts_with("control"));
        let mut archive =
            tar::Archive::new(fs::File::open(scenario.root.join("workspace_archive")).unwrap());
        let archive_has_control = archive
            .entries()
            .unwrap()
            .any(|entry| entry.unwrap().path().unwrap().starts_with("control"));
        assert!(
            !manifest_has_control && !archive_has_control,
            "custom configuration or its prepared snapshot entered the worker copy (alias={case_alias})"
        );
        assert_eq!(manifest.len(), 1);
        assert!(manifest.contains_key("original.txt"));
        assert!(
            control.join("authorities/fixture-seed.json").exists(),
            "source control state was changed during import"
        );
        scenario.invariant();
    }
}

#[tokio::test]
async fn resumed_provider_credentials_remain_bound_after_original_moves_and_reject_changed_bytes() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = support::private_tempdir();
    let bin = fixture_binary(temporary.path());
    let scenario = Scenario::new(temporary.path(), &bin, "import-completion-response-lost").await;
    let source = scenario.root.join("provider.env");
    let original = b"OPENAI_API_KEY=fixture-private-provider-value\n";
    fs::write(&source, original).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
    let initial = scenario.invoke("run").await;
    assert!(!initial.status.success());
    assert_eq!(
        scenario.count("start", "import"),
        1,
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );
    let moved = scenario.root.join("retained-original");
    fs::rename(&scenario.project, &moved).unwrap();
    let worker = scenario.store.workers().unwrap()[0].clone();
    fs::write(&source, b"OPENAI_API_KEY=fixture-rotated-provider-value\n").unwrap();
    let before = scenario.calls();
    let rejected = scenario.invoke("worker-resume").await;
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("credential source changed"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(scenario.count("start", "run"), 0);
    assert_eq!(scenario.count("create", "run"), 0);
    assert_eq!(
        scenario.calls(),
        before,
        "changed credentials emitted a recovery helper or runtime call"
    );
    assert!(scenario.store.workers().unwrap()[0].quota_held);
    fs::write(&source, original).unwrap();
    let resumed = scenario.invoke("worker-resume").await;
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let recovered = &scenario.store.workers().unwrap()[0];
    assert_eq!(worker.worker_id, recovered.worker_id);
    assert_eq!(worker.attempt.attempt_id, recovered.attempt.attempt_id);
    assert_eq!(scenario.count("start", "run"), 1);
    assert_eq!(scenario.count("create", "run"), 1);
    assert!(!recovered.quota_held);
    assert!(
        fs::read_dir(scenario.config.join("credential-snapshots"))
            .unwrap()
            .next()
            .is_none()
    );
    fs::rename(moved, &scenario.project).unwrap();
    scenario.invariant();
}

#[tokio::test]
async fn pending_program_change_cannot_fall_back_to_the_old_valid_program_after_audit_failure() {
    let temporary = support::private_tempdir();
    let bin = fixture_binary(temporary.path());
    let scenario = Scenario::new(temporary.path(), &bin, "created-control-response-lost").await;
    assert!(!scenario.invoke("run").await.status.success());
    let original = scenario.store.workers().unwrap()[0].clone();
    let alternate = scenario.bin.join("alternate");
    fs::copy(scenario.bin.join("docker"), &alternate).unwrap();
    let metadata = alternate.with_extension("fixture");
    fs::create_dir(&metadata).unwrap();
    fs::write(metadata.join("root"), scenario.root.to_str().unwrap()).unwrap();
    fs::write(metadata.join("case"), "created-control-response-lost").unwrap();
    fs::write(scenario.root.join("reject_invocation_audit"), "reject").unwrap();
    let rejected = scenario
        .command_at_program("worker-resume", &scenario.url, &alternate)
        .arg("--adopt-runtime-program")
        .output()
        .await
        .unwrap();
    assert!(!rejected.status.success());
    let before = scenario.calls();
    for adopt in [false, true] {
        let mut command = scenario.command("worker-resume");
        if adopt {
            command.arg("--adopt-runtime-program");
        }
        let rejected = command.output().await.unwrap();
        assert!(!rejected.status.success());
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("adoption is pending"));
    }
    assert_eq!(
        scenario.calls(),
        before,
        "old valid program bypassed the pending audit"
    );
    assert_eq!(scenario.count("start", "run"), 0);
    fs::remove_file(scenario.root.join("reject_invocation_audit")).unwrap();
    let resumed = scenario
        .command_at_program("worker-resume", &scenario.url, &alternate)
        .arg("--adopt-runtime-program")
        .output()
        .await
        .unwrap();
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let worker = &scenario.store.workers().unwrap()[0];
    assert_eq!(worker.worker_id, original.worker_id);
    assert_eq!(worker.attempt.container_id, original.attempt.container_id);
    assert_eq!(scenario.count("start", "run"), 1);
    scenario.invariant();
}

#[tokio::test]
async fn replaced_approved_program_is_rejected_before_any_recovery_engine_call() {
    use std::io::Write;
    let temporary = support::private_tempdir();
    let bin = fixture_binary(temporary.path());
    let scenario = Scenario::new(temporary.path(), &bin, "create-response-lost").await;
    assert!(!scenario.invoke("run").await.status.success());
    let calls = scenario.calls();
    fs::OpenOptions::new()
        .append(true)
        .open(scenario.bin.join("docker"))
        .unwrap()
        .write_all(b"changed program identity")
        .unwrap();
    let resumed = scenario.invoke("worker-resume").await;
    assert!(!resumed.status.success());
    assert!(
        String::from_utf8_lossy(&resumed.stderr)
            .contains("runtime executable identity or content changed"),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(scenario.calls(), calls);
    assert_eq!(scenario.count("start", "run"), 0);
    assert!(scenario.invoke("worker-status").await.status.success());
    assert!(scenario.store.workers().unwrap()[0].quota_held);
    scenario.invariant();
}

#[tokio::test]
async fn legacy_invocation_requires_explicit_adoption_and_durable_audit_before_start() {
    let temporary = support::private_tempdir();
    let bin = fixture_binary(temporary.path());
    let scenario = Scenario::new(temporary.path(), &bin, "created-control-response-lost").await;
    assert!(!scenario.invoke("run").await.status.success());
    scenario.legacy_invocation();
    verify_explicit_invocation_adoption(&scenario).await;
}

#[tokio::test]
async fn old_policy_and_replaced_client_require_audited_explicit_adoption_without_restarting() {
    let temporary = support::private_tempdir();
    let bin = fixture_binary(temporary.path());
    for mode in ["policy1", "replaced-client"] {
        let parent = temporary.path().join(mode);
        fs::create_dir(&parent).unwrap();
        let scenario = Scenario::new(&parent, &bin, "created-control-response-lost").await;
        assert!(!scenario.invoke("run").await.status.success());
        if mode == "policy1" {
            let mut plan: Value =
                serde_json::from_slice(&fs::read(scenario.checkpoint_path()).unwrap()).unwrap();
            let invocation = &mut plan["managed"]["binding"]["invocation"];
            invocation["policy_version"] = 1.into();
            invocation["cwd"] = "/".into();
            invocation
                .as_object_mut()
                .unwrap()
                .remove("executable_directory");
            fs::write(
                scenario.checkpoint_path(),
                serde_json::to_vec(&plan).unwrap(),
            )
            .unwrap();
        } else {
            let replacement = scenario.bin.join("new-client");
            fs::copy(bin.join("docker"), &replacement).unwrap();
            fs::rename(replacement, scenario.bin.join("docker")).unwrap();
        }
        verify_explicit_invocation_adoption(&scenario).await;
    }
}

async fn verify_explicit_invocation_adoption(scenario: &Scenario) {
    let previous: Value =
        serde_json::from_slice(&fs::read(scenario.checkpoint_path()).unwrap()).unwrap();
    let previous_binding: agentsandbox::runtime::RuntimeAuthority =
        serde_json::from_value(previous["managed"]["binding"].clone()).unwrap();
    let previous_fingerprint = previous_binding.invocation_fingerprint();
    let worker = scenario.store.workers().unwrap()[0].clone();
    let original_container = worker.attempt.container_id.clone();
    let before = scenario.calls();
    assert!(scenario.invoke("worker-status").await.status.success());
    let rejected = scenario.invoke("worker-resume").await;
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("--adopt-runtime-program"));
    assert_eq!(scenario.calls(), before);
    fs::write(scenario.root.join("reject_invocation_audit"), "reject").unwrap();
    let rejected = scenario
        .command("worker-resume")
        .arg("--adopt-runtime-program")
        .output()
        .await
        .unwrap();
    assert!(!rejected.status.success());
    assert_eq!(scenario.count("start", "run"), 0);
    let pending: Value =
        serde_json::from_slice(&fs::read(scenario.checkpoint_path()).unwrap()).unwrap();
    assert_eq!(
        pending["managed"]["binding"],
        previous["managed"]["binding"]
    );
    let pending_id = pending["managed"]["invocation_event"]["id"].clone();
    assert!(pending_id.is_string());
    let rejected = scenario
        .command("worker-resume")
        .arg("--adopt-runtime-program")
        .output()
        .await
        .unwrap();
    assert!(!rejected.status.success());
    let retry: Value =
        serde_json::from_slice(&fs::read(scenario.checkpoint_path()).unwrap()).unwrap();
    assert_eq!(retry["managed"]["invocation_event"]["id"], pending_id);
    assert_eq!(scenario.count("start", "run"), 0);
    fs::remove_file(scenario.root.join("reject_invocation_audit")).unwrap();
    let resumed = scenario
        .command("worker-resume")
        .arg("--adopt-runtime-program")
        .output()
        .await
        .unwrap();
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let recovered = &scenario.store.workers().unwrap()[0];
    assert_eq!(recovered.worker_id, worker.worker_id);
    assert_eq!(recovered.attempt.attempt_id, worker.attempt.attempt_id);
    assert_eq!(recovered.attempt.container_id, original_container);
    assert_eq!(scenario.count("start", "run"), 1);
    let plan: Value =
        serde_json::from_slice(&fs::read(scenario.checkpoint_path()).unwrap()).unwrap();
    assert!(plan["managed"]["binding"]["invocation"].is_object());
    assert!(plan["managed"].get("invocation_event").is_none());
    let events = scenario
        .store
        .events(&EventQuery {
            kind: Some("runtime.invocation.bound".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        serde_json::to_value(&events[0].event.id).unwrap(),
        pending_id
    );
    let adopted: agentsandbox::runtime::RuntimeAuthority =
        serde_json::from_value(plan["managed"]["binding"].clone()).unwrap();
    assert_eq!(
        events[0].event.data["previous_invocation_fingerprint"],
        serde_json::to_value(&previous_fingerprint).unwrap()
    );
    assert_eq!(
        events[0].event.data["new_invocation_fingerprint"],
        serde_json::to_value(adopted.invocation_fingerprint()).unwrap()
    );
    let history = scenario.config.join("invocation-history").join(format!(
        "{}-{}.json",
        previous_binding.authority_id,
        previous_fingerprint.as_deref().unwrap_or("unbound")
    ));
    let preserved: agentsandbox::runtime::RuntimeAuthority =
        serde_json::from_slice(&fs::read(history).unwrap()).unwrap();
    assert_eq!(preserved, previous_binding);
    let exported = scenario
        .command("export-workspace")
        .arg("--output")
        .arg(scenario.root.join("adopted-export"))
        .arg("--adopt-runtime-program")
        .output()
        .await
        .unwrap();
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );
    assert_eq!(
        scenario
            .store
            .events(&EventQuery {
                kind: Some("runtime.invocation.bound".into()),
                ..Default::default()
            })
            .unwrap()
            .len(),
        1,
        "same valid invocation was adopted twice"
    );
    assert_eq!(scenario.count("start", "run"), 1);
    scenario.invariant();
}

#[tokio::test]
async fn recovery_with_another_explicit_program_refuses_without_a_second_start() {
    let temporary = support::private_tempdir();
    let bin = fixture_binary(temporary.path());
    let scenario = Scenario::new(temporary.path(), &bin, "create-response-lost").await;
    assert!(!scenario.invoke("run").await.status.success());
    let alternate = scenario.bin.join("alternate");
    fs::copy(scenario.bin.join("docker"), &alternate).unwrap();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    let output = command
        .arg("worker-resume")
        .arg("--config-dir")
        .arg(&scenario.config)
        .args([
            "--gateway",
            &scenario.url,
            "--session",
            &scenario.store.workers().unwrap()[0].launch.session_id,
            "--runtime-program",
        ])
        .arg(alternate)
        .args(["--wait-seconds", "0"])
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(scenario.count("start", "run"), 0);
    let recovered = scenario
        .command("worker-resume")
        .env("PATH", scenario.project.as_os_str())
        .env("DOCKER_HOST", "tcp://untrusted.invalid:2375")
        .output()
        .await
        .unwrap();
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert_eq!(scenario.count("start", "run"), 1);
    scenario.invariant();
}

#[tokio::test]
async fn legacy_terminal_status_stop_and_resume_work_and_export_requires_adoption() {
    let temporary = support::private_tempdir();
    let bin = fixture_binary(temporary.path());
    let scenario = Scenario::new(temporary.path(), &bin, "acknowledged").await;
    let run = scenario.invoke("run").await;
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    scenario.legacy_invocation();
    let before = scenario.calls();
    for action in ["worker-status", "worker-stop", "worker-resume"] {
        assert!(scenario.invoke(action).await.status.success(), "{action}");
    }
    assert_eq!(scenario.calls(), before);
    let output = scenario.root.join("exported");
    let rejected = scenario
        .command("export-workspace")
        .arg("--output")
        .arg(&output)
        .output()
        .await
        .unwrap();
    assert!(!rejected.status.success());
    assert!(!output.exists());
    assert_eq!(scenario.calls(), before);
    let exported = scenario
        .command("export-workspace")
        .arg("--output")
        .arg(&output)
        .arg("--adopt-runtime-program")
        .output()
        .await
        .unwrap();
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );
    assert_eq!(
        fs::read_to_string(output.join("original.txt")).unwrap(),
        "immutable original\n"
    );
    assert_eq!(scenario.count("start", "run"), 1);
    assert_eq!(scenario.count("start", "export"), 2); // One change scan and one explicit export.
    scenario.invariant();
}

#[tokio::test]
async fn recovered_create_and_import_keep_the_same_instance_and_first_start() {
    let temporary = support::private_tempdir();
    let bin = fixture_binary(temporary.path());
    let mut failures = Vec::new();
    for case in [
        "create-response-lost",
        "created-control-response-lost",
        "import-completion-response-lost",
    ] {
        let scenario = Scenario::new(temporary.path(), &bin, case).await;
        let initial = scenario.invoke("run").await;
        assert!(
            !initial.status.success(),
            "{case}: first fault was not triggered"
        );
        assert_eq!(
            scenario
                .store
                .worker_quota(&QuotaPolicyV1::default())
                .unwrap()
                .held_workers,
            1
        );
        assert!(scenario.store.worker_usage().unwrap().is_empty());
        let resumed = scenario.invoke("worker-resume").await;
        let workers = scenario.store.workers().unwrap();
        if !resumed.status.success()
            || workers[0].state != WorkerStateV1::Terminal
            || workers[0].quota_held
            || scenario.store.worker_usage().unwrap().len() != 1
            || scenario.count("create", "run") != 1
            || scenario.count("start", "run") != 1
            || scenario.count("create", "import") != 1
            || scenario.count("start", "import") != 1
        {
            failures.push(format!("{case}: recovery did not finish the same attempt once; state={:?}, run_create={}, run_start={}, import_create={}, import_start={}, stderr={}", workers[0].state, scenario.count("create", "run"), scenario.count("start", "run"), scenario.count("create", "import"), scenario.count("start", "import"), String::from_utf8_lossy(&resumed.stderr)));
        }
        let _ = scenario.invoke("worker-resume").await;
        assert!(
            scenario.count("start", "run") <= 1,
            "{case}: replay started twice"
        );
        assert!(
            scenario.store.worker_usage().unwrap().len() <= 1,
            "{case}: duplicated allocation"
        );
        scenario.invariant();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn recovered_first_start_reports_a_failed_command_as_cli_failure() {
    let temporary = support::private_tempdir();
    let bin = fixture_binary(temporary.path());
    let scenario = Scenario::new(temporary.path(), &bin, "created-control-response-lost").await;
    fs::write(scenario.root.join("run_exit_code"), "7").unwrap();
    assert!(!scenario.invoke("run").await.status.success());
    assert_eq!(scenario.count("start", "run"), 0);
    let resumed = scenario.invoke("worker-resume").await;
    let summary: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(
        summary["exit_code"], 7,
        "observed result must remain available"
    );
    assert!(
        !resumed.status.success(),
        "a failed recovered command must not exit zero"
    );
    assert_eq!(scenario.count("create", "run"), 1);
    assert_eq!(scenario.count("start", "run"), 1);
    let worker = &scenario.store.workers().unwrap()[0];
    assert_eq!(worker.state, WorkerStateV1::Terminal);
    assert!(!worker.quota_held);
    assert_eq!(scenario.store.worker_usage().unwrap().len(), 1);
    let again = scenario.invoke("worker-resume").await;
    assert!(
        !again.status.success(),
        "a saved failed command must not exit zero on replay"
    );
    let saved: Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(saved["result"]["exit_code"], 7);
    assert_eq!(scenario.count("start", "run"), 1);
    scenario.invariant();
}

#[tokio::test]
async fn lost_start_claim_ack_keeps_unknown_quota_and_never_starts() {
    let temporary = support::private_tempdir();
    let bin = fixture_binary(temporary.path());
    let scenario = Scenario::new(temporary.path(), &bin, "claim-response-lost").await;
    let initial = scenario.invoke("run").await;
    assert!(!initial.status.success());
    assert_eq!(
        scenario.count("start", "import"),
        1,
        "fault not reached: {}; calls={}",
        String::from_utf8_lossy(&initial.stderr),
        scenario.calls()
    );
    for _ in 0..2 {
        let resumed = scenario.invoke("worker-resume").await;
        assert!(
            !resumed.status.success(),
            "an unresolved consumed claim must return a nonzero CLI status"
        );
        let value: Value = serde_json::from_slice(&resumed.stdout).unwrap();
        assert!(
            value["message"]
                .as_str()
                .unwrap()
                .contains("No terminal proof")
        );
    }
    let worker = &scenario.store.workers().unwrap()[0];
    assert_eq!(worker.state, WorkerStateV1::InDoubt);
    assert!(worker.attempt.start_operation_id.is_some() && worker.quota_held);
    assert_eq!(scenario.count("create", "run"), 1);
    assert_eq!(scenario.count("start", "run"), 0);
    assert!(scenario.store.worker_usage().unwrap().is_empty());
    scenario.invariant();
}

#[tokio::test]
async fn broker_crash_recovery_observes_then_stops_the_same_running_instance() {
    let temporary = support::private_tempdir();
    let bin = fixture_binary(temporary.path());
    let scenario = Scenario::new(temporary.path(), &bin, "broker-crash-running").await;
    let mut broker = scenario
        .command("run")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let (original, container_id) = loop {
        let workers = scenario.store.workers().unwrap();
        if let [worker] = workers.as_slice()
            && worker.state == WorkerStateV1::Running
            && let Some(id) = worker.attempt.container_id.as_deref()
            && fs::read_to_string(scenario.root.join("containers").join(id).join("state"))
                .is_ok_and(|state| state == "running")
        {
            break (worker.clone(), id.to_owned());
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the exact worker never reached observed and durable Running"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(
        fs::read_to_string(scenario.root.join("run_id")).unwrap(),
        container_id
    );
    broker.kill().await.unwrap();
    let _ = broker.wait().await.unwrap();
    scenario.legacy_workspace_exclusion();
    let relocated = scenario.root.join("relocated-original");
    fs::rename(&scenario.project, &relocated).unwrap();

    let assert_unreconciled = || {
        let workers = scenario.store.workers().unwrap();
        let [worker] = workers.as_slice() else {
            panic!("expected the single original worker");
        };
        assert_eq!(worker.worker_id, original.worker_id);
        assert_eq!(worker.attempt, original.attempt);
        assert_ne!(worker.state, WorkerStateV1::Terminal);
        assert!(worker.quota_held);
        assert!(scenario.store.worker_usage().unwrap().is_empty());
        assert!(!scenario.root.join("terminal_ack").exists());
    };
    let assert_one_stop = || {
        let expected = format!("kill {container_id}");
        let calls = scenario.calls();
        let stops: Vec<_> = calls
            .lines()
            .filter(|line| line.starts_with("kill "))
            .collect();
        assert_eq!(stops, [expected.as_str()]);
    };
    let before_resume = scenario.store.head().unwrap();
    let resumed = scenario.invoke("worker-resume").await;
    assert!(resumed.status.success(), "local recovery failed");
    let value: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(value["outputs"], "unknown_after_broker_interruption");
    assert_eq!(value["worker"]["worker_id"], original.worker_id);
    assert_eq!(
        value["worker"]["attempt"]["attempt_id"],
        original.attempt.attempt_id
    );
    assert_eq!(
        value["worker"]["launch"]["authority_id"],
        original.launch.authority_id
    );
    assert_eq!(value["runtime_observation"]["id"], container_id);
    assert_eq!(value["runtime_observation"]["process"]["running"], false);
    assert!(value["result"].is_null());
    assert_eq!(
        fs::read_to_string(
            scenario
                .root
                .join("containers")
                .join(&container_id)
                .join("state")
        )
        .unwrap(),
        "exited"
    );
    assert_one_stop();
    assert_unreconciled();
    assert_eq!(scenario.store.head().unwrap(), before_resume);

    // This legacy adoption restores the same invocation used by the first CLI.
    let checkpoint_path = scenario.checkpoint_path();
    let original_plan: Value =
        serde_json::from_slice(&fs::read(&checkpoint_path).unwrap()).unwrap();
    let original_binding: RuntimeAuthority =
        serde_json::from_value(original_plan["managed"]["binding"].clone()).unwrap();
    let expected_fingerprint = original_binding
        .invocation_fingerprint()
        .expect("original CLI did not persist its invocation");
    scenario.legacy_invocation();
    let before = scenario.calls();
    let rejected = scenario.invoke("worker-stop").await;
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("--adopt-runtime-program"));
    assert_eq!(
        scenario.calls(),
        before,
        "unbound stop emitted an engine effect"
    );
    let resume_with_adoption = || async {
        tokio::time::timeout(
            CLI_OBSERVATION_TIMEOUT,
            scenario
                .command("worker-resume")
                .arg("--adopt-runtime-program")
                .output(),
        )
        .await
        .unwrap()
        .unwrap()
    };
    let audit_refusal = scenario.root.join("reject_invocation_audit");
    fs::write(&audit_refusal, "reject").unwrap();
    let before_rejected_audit = scenario.store.head().unwrap();
    let rejected = resume_with_adoption().await;
    assert!(
        !rejected.status.success(),
        "adoption succeeded without a confirmed gateway audit"
    );
    let pending: Value = serde_json::from_slice(&fs::read(&checkpoint_path).unwrap()).unwrap();
    let pending_adoption = &pending["managed"]["invocation_event"];
    let pending_id = pending_adoption["id"].as_str().unwrap();
    assert!(
        pending_adoption["data"]["invocation_fingerprint"].as_str()
            == Some(expected_fingerprint.as_str()),
        "pending adoption did not bind the original CLI invocation"
    );
    assert_unreconciled();
    assert_eq!(scenario.store.head().unwrap(), before_rejected_audit);

    // This batch was interrupted before it could retain a command result.
    // Core25 requires this exact partial-coverage refusal even after recovery.
    const CAPTURE_UNAVAILABLE: &str =
        "legacy declared capture is unavailable; delivery counts remain unknown";
    let capture_refusal =
        format!("Error: postprocessing recovery incomplete: {CAPTURE_UNAVAILABLE}\n");
    let assert_capture_refusal = |output: &std::process::Output, phase: &str| {
        let category = if output.stderr.as_slice() == capture_refusal.as_bytes() {
            "declared_capture_unavailable"
        } else if output.stderr.as_slice()
            == b"Error: postprocessing recovery requires durable cleanup\n"
        {
            "cleanup_unconfirmed"
        } else if output.status.success() {
            "unexpected_success"
        } else {
            "unexpected_refusal"
        };
        eprintln!(
            "g09_recovery_cli phase={phase} category={category} exit_code={:?} stdout_empty={}",
            output.status.code(),
            output.stdout.is_empty()
        );
        assert!(
            output.status.code() == Some(1)
                && output.stdout.is_empty()
                && category == "declared_capture_unavailable",
            "unexpected authenticated terminal reconciliation outcome at {phase}"
        );
    };
    fs::remove_file(audit_refusal).unwrap();
    let reconciled = resume_with_adoption().await;
    assert_capture_refusal(&reconciled, "initial");

    let workers = scenario.store.workers().unwrap();
    let [worker] = workers.as_slice() else {
        panic!("expected the single reconciled worker");
    };
    assert!(
        worker.worker_id == original.worker_id
            && worker.attempt == original.attempt
            && worker.launch.authority_id == original.launch.authority_id,
        "recovery changed the original worker, attempt or authority"
    );
    assert_eq!(worker.state, WorkerStateV1::Terminal);
    assert!(!worker.quota_held);
    assert_eq!(worker.cleanup_removed, Some(true));
    assert!(scenario.root.join("terminal_ack").is_file());
    assert!(
        !scenario
            .root
            .join("containers")
            .join(&container_id)
            .exists()
    );
    assert_eq!(scenario.count("rm", "run"), 1);
    let observation = worker
        .last_observation
        .as_ref()
        .expect("missing terminal observation");
    assert!(
        observation.inspection.container_id == container_id
            && observation.running == Some(false)
            && observation.started_at.is_some()
            && observation.finished_at.is_some(),
        "durable observation does not attest termination of the original instance"
    );

    let allocations = scenario.store.worker_usage().unwrap();
    let [allocation] = allocations.as_slice() else {
        panic!("expected exactly one terminal allocation");
    };
    assert!(
        allocation.source.container_id == container_id
            && allocation.worker_id == original.worker_id
            && allocation.attempt_id == original.attempt.attempt_id
            && !allocation.billable,
        "terminal usage changed identity or billability"
    );
    let terminal_receipt = scenario
        .store
        .worker_operation(&allocation.terminal_observation_id)
        .unwrap()
        .expect("missing terminal usage receipt");
    assert!(
        terminal_receipt.worker_id.as_deref() == Some(original.worker_id.as_str())
            && terminal_receipt.audit_seq > 0
            && terminal_receipt.audit_hash.len() == 64,
        "terminal usage does not have a durable receipt for the original worker"
    );

    let single_event = |kind: &str| {
        let events = scenario
            .store
            .events(&EventQuery {
                kind: Some(kind.into()),
                limit: Some(2),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(events.len(), 1, "{kind} must be audited exactly once");
        let event = events.into_iter().next().unwrap();
        assert!(
            event.event.session_id.as_deref() == Some(original.launch.session_id.as_str()),
            "{kind} must be attributed to the original session"
        );
        event
    };
    let adoption = single_event("runtime.invocation.bound");
    let adoption_data = &adoption.event.data;
    assert!(
        adoption.event.id == pending_id
            && adoption_data["invocation_fingerprint"].as_str()
                == Some(expected_fingerprint.as_str())
            && adoption_data["new_invocation_fingerprint"].as_str()
                == Some(expected_fingerprint.as_str())
            && adoption_data
                .get("previous_invocation_fingerprint")
                .is_some_and(Value::is_null)
            && adoption_data["legacy_adoption"].as_bool() == Some(true),
        "durable adoption does not match the original invocation and pending event"
    );
    let scan_requested = single_event("workspace.scan.requested");
    let scan_created = single_event("workspace.scan.created");
    let scan_started = single_event("workspace.scan.started");
    let scan_completed = single_event("workspace.scan.completed");
    let diff_completed = single_event("workspace.diff.completed");
    let coverage = single_event("observer.coverage");
    assert!(
        adoption.seq < terminal_receipt.audit_seq
            && terminal_receipt.audit_seq < scan_requested.seq
            && scan_requested.seq < scan_created.seq
            && scan_created.seq < scan_started.seq
            && scan_started.seq < scan_completed.seq
            && scan_completed.seq < diff_completed.seq
            && diff_completed.seq < coverage.seq,
        "recovery audit observations are out of order"
    );
    assert!(
        diff_completed.event.data["changed_files"].as_u64() == Some(0)
            && diff_completed.event.data["observation_phase"].as_str()
                == Some("post_completion_recovery"),
        "trusted diff does not describe the unchanged recovered volume"
    );
    let export_id = scan_created.event.data["container_id"]
        .as_str()
        .filter(|id| id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .expect("scan did not audit a valid export helper");
    assert!(
        scan_created.event.data["operation_id"]
            .as_str()
            .and_then(|operation| uuid::Uuid::parse_str(operation).ok())
            .is_some_and(|operation| !operation.is_nil()),
        "scan did not audit a non-nil operation UUID"
    );
    assert!(
        export_id != container_id.as_str(),
        "scan reused the original worker"
    );
    assert!(!scenario.root.join("containers").join(export_id).exists());
    let calls_after_recovery = scenario.calls();
    for action in ["create", "start", "rm"] {
        assert_eq!(scenario.count(action, "export"), 1);
        let expected = format!("{action} {export_id} export");
        assert!(
            calls_after_recovery
                .lines()
                .any(|line| line == expected.as_str()),
            "audited export helper did not perform its expected {action}"
        );
    }

    let read_checkpoint = || {
        let plan: Value = serde_json::from_slice(&fs::read(&checkpoint_path).unwrap())
            .expect("invalid private recovery checkpoint");
        let adopted_binding: RuntimeAuthority =
            serde_json::from_value(plan["managed"]["binding"].clone())
                .expect("invalid adopted runtime binding");
        assert!(
            adopted_binding == original_binding
                && plan["managed"].get("invocation_event").is_none(),
            "committed checkpoint must restore the audited invocation and clear the pending adoption"
        );
        assert!(
            plan["managed"].get("result").is_some_and(Value::is_null)
                && plan["managed"]
                    .get("output_event")
                    .is_some_and(Value::is_null)
                && plan["managed"].get("pending").is_some_and(Value::is_null)
                && plan["managed"]["worker"]["cleanup_removed"].as_bool() == Some(true),
            "checkpoint must acknowledge cleanup and preserve the unknown command result"
        );
        plan
    };
    let checkpoint = read_checkpoint();
    let head_after_recovery = scenario.store.head().unwrap();
    let read_status = || async {
        let output = scenario.invoke("worker-status").await;
        assert!(
            output.status.success(),
            "worker-status could not read the reconciled session"
        );
        serde_json::from_slice::<Value>(&output.stdout).expect("worker-status did not return JSON")
    };
    let status = read_status().await;
    assert!(
        status["session_id"].as_str() == Some(original.launch.session_id.as_str())
            && status["tenant_id"].as_str() == Some("local")
            && status["gateway_origin"] == checkpoint["managed"]["gateway_origin"]
            && status["worker"] == serde_json::to_value(worker).unwrap()
            && status["registry_observation_scope"].as_str() == Some("last_durably_recorded")
            && status
                .get("runtime_observation")
                .is_some_and(Value::is_null)
            && status.get("result").is_some_and(Value::is_null)
            && status["outputs"].as_str() == Some("unknown_after_broker_interruption"),
        "worker-status must report authenticated terminal registry facts, no live inspection and unknown output"
    );
    let postprocessing = &status["postprocessing"];
    let declared = &postprocessing["declared"];
    assert!(
        declared["state"].as_str() == Some("unavailable_after_interruption")
            && declared["error"].as_str() == Some(CAPTURE_UNAVAILABLE),
        "worker-status must retain the reason for partial declared coverage"
    );
    for field in [
        "normalized_events",
        "acknowledged_events",
        "dropped_events",
        "unknown_delivery_events",
        "not_attempted_events",
    ] {
        assert!(
            declared.get(field).is_some_and(Value::is_null),
            "unavailable capture count must remain unknown for {field}"
        );
    }
    let diff = &postprocessing["diff"];
    assert!(
        diff["completed_acknowledged"].as_bool() == Some(true)
            && diff["changed_files"].as_u64() == Some(0)
            && diff["completed"]["id"].as_str() == Some(diff_completed.event.id.as_str())
            && postprocessing.get("diff_error").is_some_and(Value::is_null),
        "trusted diff in worker-status must match the acknowledged audit event"
    );
    let retained_coverage = &postprocessing["coverage"];
    assert!(
        postprocessing["coverage_acknowledged"].as_bool() == Some(true)
            && postprocessing
                .get("coverage_error")
                .is_some_and(Value::is_null)
            && retained_coverage["id"].as_str() == Some(coverage.event.id.as_str())
            && retained_coverage["coverage"].as_str() == Some("partial")
            && coverage.event.data == retained_coverage["data"]
            && retained_coverage["data"]["trusted_diff"].as_str() == Some("complete")
            && retained_coverage["data"]["postprocessing_complete"].as_bool() == Some(false)
            && retained_coverage["data"]["declared_collection"] == *declared,
        "worker-status must show acknowledged partial coverage for the reconciled session"
    );

    let again = scenario.invoke("worker-resume").await;
    assert_capture_refusal(&again, "replay");
    read_checkpoint();
    assert!(
        read_status().await == status,
        "replay changed the observed worker-status"
    );
    assert!(
        scenario.store.worker_usage().unwrap() == allocations,
        "replay changed terminal usage"
    );
    assert!(
        scenario
            .store
            .worker_operation(&allocation.terminal_observation_id)
            .unwrap()
            == Some(terminal_receipt),
        "replay changed the terminal receipt"
    );
    assert!(
        scenario.store.head().unwrap() == head_after_recovery,
        "status or replay changed the durable audit head"
    );
    assert!(
        scenario.calls() == calls_after_recovery,
        "status or replay invoked the runtime after cleanup"
    );
    assert_eq!(scenario.count("create", "run"), 1);
    assert_eq!(scenario.count("start", "run"), 1);
    assert_one_stop();
    fs::rename(relocated, &scenario.project).unwrap();
    scenario.invariant();
}

#[tokio::test]
async fn live_owner_blocks_resume_and_processes_a_durable_stop_intent() {
    let temporary = support::private_tempdir();
    let bin = fixture_binary(temporary.path());
    let scenario = Scenario::new(temporary.path(), &bin, "broker-crash-running").await;
    let mut broker = scenario
        .command("run")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !scenario
        .store
        .workers()
        .unwrap()
        .first()
        .is_some_and(|worker| worker.state == WorkerStateV1::Running)
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "broker did not record running"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let resumed = scenario.invoke("worker-resume").await;
    assert!(!resumed.status.success());
    assert!(String::from_utf8_lossy(&resumed.stderr).contains("another broker holds this attempt"));
    assert_eq!(scenario.count("start", "run"), 1);
    let stopped = scenario.invoke("worker-stop").await;
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    let status = tokio::time::timeout(Duration::from_secs(10), broker.wait())
        .await
        .expect("live owner ignored durable stop")
        .unwrap();
    assert!(status.success());
    let worker = &scenario.store.workers().unwrap()[0];
    assert_eq!(worker.state, WorkerStateV1::Terminal);
    assert!(!worker.quota_held);
    assert_eq!(scenario.count("create", "run"), 1);
    assert_eq!(scenario.count("start", "run"), 1);
    assert_eq!(scenario.store.worker_usage().unwrap().len(), 1);
    scenario.invariant();
}

#[tokio::test]
async fn recovery_refuses_another_origin_tenant_binding_manifest_or_archive_before_runtime_effects()
{
    let temporary = support::private_tempdir();
    let bin = fixture_binary(temporary.path());
    let scenario = Scenario::new(temporary.path(), &bin, "import-completion-response-lost").await;
    let initial = scenario.invoke("run").await;
    assert!(!initial.status.success());
    assert_eq!(
        scenario.count("start", "import"),
        1,
        "fault not reached: {}; calls={}",
        String::from_utf8_lossy(&initial.stderr),
        scenario.calls()
    );
    let worker = &scenario.store.workers().unwrap()[0];
    let checkpoint_path = scenario
        .config
        .join("sessions")
        .join(format!("{}.json", worker.launch.session_id));
    let original = fs::read(&checkpoint_path).unwrap();
    let initial_calls = scenario.calls();
    let changed_origin = scenario
        .command_at("worker-resume", "http://127.0.0.1:1")
        .output()
        .await
        .unwrap();
    assert!(!changed_origin.status.success());
    assert!(String::from_utf8_lossy(&changed_origin.stderr).contains("origin"));
    let runner_path = scenario.config.join("secrets/runner");
    let runner = fs::read(&runner_path).unwrap();
    fs::write(
        &runner_path,
        fs::read(scenario.root.join("other_runner")).unwrap(),
    )
    .unwrap();
    let changed_tenant = scenario.invoke("worker-resume").await;
    fs::write(&runner_path, runner).unwrap();
    assert!(!changed_tenant.status.success());
    assert!(
        String::from_utf8_lossy(&changed_tenant.stderr).contains("authenticated tenant differs")
    );
    for pointer in [
        "/managed/binding/authority_id",
        "/managed/admission/launch/workspace_manifest_digest",
    ] {
        let mut altered: Value = serde_json::from_slice(&original).unwrap();
        *altered.pointer_mut(pointer).unwrap() =
            Value::String(if pointer.ends_with("authority_id") {
                uuid::Uuid::new_v4().to_string()
            } else {
                "f".repeat(64)
            });
        fs::write(&checkpoint_path, serde_json::to_vec(&altered).unwrap()).unwrap();
        let rejected = scenario.invoke("worker-resume").await;
        fs::write(&checkpoint_path, &original).unwrap();
        assert!(!rejected.status.success(), "altered {pointer} was accepted");
    }
    assert_eq!(
        scenario.calls(),
        initial_calls,
        "rejected identity/binding touched the engine"
    );
    let checkpoint: Value = serde_json::from_slice(&original).unwrap();
    let archive = scenario
        .config
        .join("sessions")
        .join(checkpoint["managed"]["archive_name"].as_str().unwrap());
    let archive_bytes = fs::read(&archive).unwrap();
    fs::write(&archive, b"substituted archive").unwrap();
    let rejected = scenario.invoke("worker-resume").await;
    fs::write(&archive, archive_bytes).unwrap();
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("immutable workspace snapshot changed")
    );
    assert_eq!(scenario.count("start", "import"), 1);
    assert_eq!(scenario.count("create", "run"), 0);
    assert!(scenario.store.worker_usage().unwrap().is_empty());
    assert!(scenario.store.workers().unwrap()[0].quota_held);
    scenario.invariant();
}

#[tokio::test]
async fn preparation_without_any_helper_emission_can_resume_from_unknown() {
    let temporary = support::private_tempdir();
    let bin = fixture_binary(temporary.path());
    let scenario = Scenario::new(temporary.path(), &bin, "volume-response-lost").await;
    let initial = scenario.invoke("run").await;
    assert!(!initial.status.success());
    assert_eq!(scenario.count("create", "import"), 0);
    let worker = scenario.store.workers().unwrap().remove(0);
    assert!(
        worker.attempt.create_operation_id.is_none()
            && worker.attempt.container_id.is_none()
            && worker.attempt.start_operation_id.is_none()
    );
    scenario
        .store
        .mutate_worker(&WorkerMutationV1 {
            version: 1,
            operation_id: uuid::Uuid::new_v4().to_string(),
            worker_id: worker.worker_id,
            attempt_id: worker.attempt.attempt_id,
            expected_revision: worker.revision,
            transition: WorkerTransitionV1::Unknown {
                reason: "broker interrupted before any helper emission".into(),
            },
        })
        .unwrap();
    let resumed = scenario.invoke("worker-resume").await;
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let worker = scenario.store.workers().unwrap().remove(0);
    assert_eq!(worker.state, WorkerStateV1::Terminal);
    assert!(!worker.quota_held);
    assert_eq!(scenario.count("create", "import"), 1);
    assert_eq!(scenario.count("start", "import"), 1);
    assert_eq!(scenario.count("create", "run"), 1);
    assert_eq!(scenario.count("start", "run"), 1);
    assert_eq!(scenario.store.worker_usage().unwrap().len(), 1);
    scenario.invariant();
}
