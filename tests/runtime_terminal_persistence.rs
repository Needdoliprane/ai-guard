//! A real engine, real SQLite gateway, and an injected terminal-audit outage.
//! The newly created evidence, config, container and volume are retained.
use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    control::{
        QuotaPolicyV1, RuntimeStatusV1, WorkerMutationV1, WorkerStateV1, WorkerTransitionV1,
    },
    operations::initialize_config,
    policy::Policy,
    redaction::Redactor,
    runner::{RunOptions, read_token},
    runtime::{ManagedRuntime, NetworkMode, Runtime, RuntimeKind},
};
use anyhow::{Context, Result, ensure};
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
    fs::OpenOptions,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

#[tokio::test]
#[ignore = "requires an explicitly selected real engine and built worker image; retains its evidence"]
async fn rejected_terminal_audit_keeps_the_real_terminal_instance_inspectable() -> Result<()> {
    let kind = match std::env::var("AGENTSANDBOX_TEST_RUNTIME").as_deref() {
        Ok("docker") => RuntimeKind::Docker,
        Ok("podman") => RuntimeKind::Podman,
        _ => anyhow::bail!("set AGENTSANDBOX_TEST_RUNTIME=docker or podman explicitly"),
    };
    let image =
        std::env::var("AGENTSANDBOX_TEST_IMAGE").context("select the built worker image")?;
    let allow_rootful = std::env::var("AGENTSANDBOX_TEST_ALLOW_ROOTFUL").as_deref() == Ok("1");
    let endpoint = std::env::var("AGENTSANDBOX_TEST_RUNTIME_ENDPOINT")
        .context("set the explicit local runtime endpoint for this test")?;
    ensure!(
        endpoint.starts_with("unix:///") || kind == RuntimeKind::Podman && endpoint == "local",
        "runtime endpoint must be an explicit local Unix endpoint"
    );
    let runtime_program =
        std::env::var_os("AGENTSANDBOX_TEST_RUNTIME_PROGRAM").map(std::path::PathBuf::from);
    let runtime = match &runtime_program {
        Some(program) => Runtime::with_program(kind, program)?,
        None => Runtime::new(kind)?,
    };
    match kind {
        RuntimeKind::Docker => {
            ManagedRuntime::connect_docker_with_runtime(
                uuid::Uuid::new_v4(),
                &endpoint,
                runtime.clone(),
            )
            .await?
            .doctor(allow_rootful)
            .await?;
        }
        RuntimeKind::Podman => {
            runtime.diagnose_podman(&endpoint, allow_rootful).await?;
        }
    }
    let evidence = tempfile::Builder::new()
        .prefix("ai-guard-terminal-audit-")
        .permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .tempdir()?
        .keep();
    eprintln!("retained terminal-audit evidence: {}", evidence.display());
    let config = evidence.join("config");
    initialize_config(&config)?;
    let project = evidence.join("project");
    fs::create_dir(&project)?;
    fs::write(project.join("original.txt"), "original remains unchanged\n")?;
    let canary = format!("FAKE_TEST_CANARY_{}", uuid::Uuid::new_v4().simple());
    let credential_path = evidence.join("canary.env");
    let mut credential = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&credential_path)?;
    writeln!(credential, "TEST_CANARY={canary}")?;
    credential.sync_all()?;
    ensure!(
        fs::metadata(&credential_path)?.permissions().mode() & 0o777 == 0o600
            && !credential_path.starts_with(&project),
        "fake credential source must be private and outside the project"
    );
    let runner_token = read_token(&config.join("secrets/runner"))?;
    let viewer_token = read_token(&config.join("secrets/viewer"))?;
    let redactor = Redactor::new(vec![runner_token.clone(), viewer_token.clone()]);
    let database = evidence.join("audit.sqlite");
    let store = Arc::new(Store::open_for_tenant(
        &database,
        "local",
        redactor.clone(),
        1_000,
    )?);
    let gateway = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "local".into(),
            store: store.clone(),
            runner_token: runner_token.clone(),
            viewer_token: viewer_token.clone(),
        }],
        Policy::default(),
        None,
        redactor,
    )?;
    let rejected = Arc::new(Mutex::new(Vec::<WorkerMutationV1>::new()));
    let captured = rejected.clone();
    let app =
        gateway_router(gateway).layer(middleware::from_fn(move |request: Request, next: Next| {
            let captured = captured.clone();
            async move {
                if request.method() != Method::POST
                    || !request.uri().path().starts_with("/api/v1/workers/")
                    || !request.uri().path().ends_with("/transitions") {
                    return next.run(request).await;
                }
                let (parts, body) = request.into_parts();
                let Ok(bytes) = to_bytes(body, 128 * 1024).await else {
                    return StatusCode::PAYLOAD_TOO_LARGE.into_response();
                };
                if let Ok(mutation) = serde_json::from_slice::<WorkerMutationV1>(&bytes)
                    && matches!(&mutation.transition,WorkerTransitionV1::Observe {observation}
                        if [RuntimeStatusV1::Exited,RuntimeStatusV1::Dead].contains(&observation.status)
                            && observation.running==Some(false))
                {
                    // Refuse before the real gateway/Store sees the mutation.
                    // No fabricated receipt or compatibility-event fault.
                    captured.lock().unwrap().push(mutation);
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
                next.run(Request::from_parts(parts, Body::from(bytes)))
                    .await
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let outcome = tokio::time::timeout(
        Duration::from_secs(90),
        agentsandbox::runner::run(RunOptions {
            project: project.clone(),
            config_dir: config.clone(),
            gateway: url.clone(),
            runtime: kind,
            runtime_program,
            runtime_endpoint: Some(endpoint.clone()),
            image,
            profile: "observe".into(),
            network: NetworkMode::Bridge,
            allow_rootful_runtime: allow_rootful,
            timeout: 20,
            provider: "generic".into(),
            approve_hash: None,
            capture_output: true,
            interactive: false,
            secret_env_file: Some(credential_path),
            client: None,
            auth_mode: None,
            client_profile: None,
            providers_file: None,
            prompt: None,
            model: None,
            argv: vec![
                "/usr/bin/python3".into(),
                "-c".into(),
                "import os, time; time.sleep(0.75); print(os.environ['TEST_CANARY'], end='')"
                    .into(),
            ],
        }),
    )
    .await;
    let run_error = outcome
        .context("runner exceeded the qualification deadline")?
        .expect_err("rejected audit must fail the run");
    let diagnostic = format!("{run_error:#}");
    ensure!(
        !diagnostic.contains(&canary),
        "runner error exposed the fake credential value"
    );
    let rejection_count = rejected
        .lock()
        .map_err(|_| anyhow::anyhow!("qualification capture poisoned"))?
        .len();
    if rejection_count == 0 {
        // runner::run flattens RuntimeCommandFailure into bounded, redacted
        // text. Preserve closed observations from it without printing it.
        let stage = [
            (
                "immutable workspace snapshot is unavailable",
                "workspace_archive",
            ),
            (
                "immutable workspace snapshot changed; import refused",
                "workspace_archive",
            ),
            (
                "container creation failed with automatic image pulls disabled",
                "container_create",
            ),
            ("runtime create response is ambiguous", "container_create"),
            ("runtime inspection exceeds its bound", "container_inspect"),
            (
                "runtime inspection lacks required sandbox configuration facts",
                "container_inspect",
            ),
            (
                "runtime did not prove a fresh never-started instance",
                "container_inspect",
            ),
            (
                "runtime created allocation differs from the requested bounded limits",
                "container_inspect",
            ),
        ]
        .into_iter()
        .find_map(|(message, stage)| diagnostic.contains(message).then_some(stage))
        .unwrap_or("before_terminal_audit");
        let engine_failure_prefix = format!("{kind} command failed (");
        let engine_failure = diagnostic
            .split_once(engine_failure_prefix.as_str())
            .map(|(_, tail)| tail);
        let exit_code = engine_failure
            .and_then(|tail| tail.strip_prefix("exit status: "))
            .and_then(|tail| tail.split_once(')'))
            .and_then(|(code, _)| code.parse::<u8>().ok());
        let stderr_truncated = engine_failure
            .and_then(|tail| tail.split_once(", stderr_truncated="))
            .and_then(|(_, tail)| tail.split_once(']'))
            .and_then(|(value, _)| value.parse::<bool>().ok());
        let mentions_keep_id = engine_failure.is_some_and(|tail| tail.contains("keep-id"));
        anyhow::bail!(
            "run failed before injected terminal audit refusal: stage={stage} engine_failure_reported={} exit_code={exit_code:?} stderr_truncated={stderr_truncated:?} diagnostic_mentions_keep_id={mentions_keep_id}",
            engine_failure.is_some()
        );
    }
    let workers = store.workers()?;
    ensure!(
        workers.len() == 1,
        "expected one durable real worker reservation"
    );
    let worker = &workers[0];
    ensure!(
        worker.state == WorkerStateV1::InDoubt && worker.quota_held,
        "rejected terminal control must leave an unknown worker with retained capacity"
    );
    ensure!(
        store.worker_usage()?.is_empty(),
        "a rejected terminal control must not produce an allocation"
    );
    ensure!(
        store.worker_quota(&QuotaPolicyV1::default())?.held_workers == 1,
        "rejected terminal control released quota"
    );
    let container_id = worker
        .attempt
        .container_id
        .as_deref()
        .context("created worker has no immutable instance ID")?;
    ensure!(
        container_id.len() == 64 && container_id.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "worker instance ID is not a full immutable ID"
    );
    let cli_endpoint = match kind {
        RuntimeKind::Docker => vec!["--host".to_owned(), endpoint.clone()],
        RuntimeKind::Podman if endpoint == "local" => vec!["--remote=false".into()],
        RuntimeKind::Podman => vec!["--remote".into(), "--url".into(), endpoint.clone()],
    };
    let mut inspect_args = cli_endpoint.clone();
    inspect_args.extend([
        "inspect".into(),
        "--format".into(),
        "{{json .State}}\n{{.HostConfig.LogConfig.Type}}\n{{json .HostConfig.LogConfig.Config}}\n{{.Config.Tty}}\n{{.Config.OpenStdin}}\n{{.Config.StdinOnce}}".into(),
        container_id.into(),
    ]);
    let inspected = runtime.cli(&inspect_args, Duration::from_secs(10)).await?;
    ensure!(!inspected.truncated, "terminal inspect was truncated");
    let inspect_text = std::str::from_utf8(&inspected.bytes)?;
    let mut facts = inspect_text.lines();
    let state: Value = serde_json::from_str(
        facts
            .next()
            .context("runtime inspect omitted process state")?,
    )?;
    ensure!(
        facts.next() == Some("none"),
        "worker logging driver is not none"
    );
    let log_options: Value = serde_json::from_str(
        facts
            .next()
            .context("runtime inspect omitted logging options")?,
    )?;
    ensure!(
        log_options.is_null()
            || log_options
                .as_object()
                .is_some_and(|options| options.is_empty()),
        "worker logging driver has unexpected options"
    );
    ensure!(
        facts.next() == Some("false")
            && facts.next() == Some("false")
            && facts.next() == Some("false"),
        "worker process stream settings differ from the non-interactive contract"
    );
    ensure!(
        state["Running"] == false && state["ExitCode"] == 0,
        "the retained worker must prove its terminal result"
    );
    ensure!(
        state["Status"] == "exited" || kind == RuntimeKind::Podman && state["Status"] == "stopped",
        "worker is not terminal"
    );
    // Use the already approved and anchored runtime executable. Driver `none`
    // may make `logs` return a diagnostic on stderr; stdout must contain no
    // worker transcript and neither stream may expose the fake credential.
    let mut logs = runtime.command()?;
    logs.args(&cli_endpoint)
        .arg("logs")
        .arg(container_id)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let log_result = tokio::time::timeout(Duration::from_secs(10), logs.output())
        .await
        .context("runtime logs inspection exceeded its deadline")??;
    ensure!(
        log_result.stdout.is_empty(),
        "worker stdout transcript was retained by the none log driver"
    );
    ensure!(
        !log_result
            .stdout
            .windows(canary.len())
            .any(|window| window == canary.as_bytes())
            && !log_result
                .stderr
                .windows(canary.len())
                .any(|window| window == canary.as_bytes()),
        "runtime logs inspection exposed the fake credential value"
    );
    ensure!(
        log_result.status.success() || !log_result.stderr.is_empty(),
        "runtime logs returned an unexplained failure without a diagnostic"
    );
    let session_id = uuid::Uuid::parse_str(&worker.launch.session_id)?;
    let checkpoint_path = config.join("sessions").join(format!("{session_id}.json"));
    let checkpoint: Value = serde_json::from_slice(&fs::read(checkpoint_path)?)?;
    let output_event = &checkpoint["managed"]["output_event"];
    ensure!(
        output_event["result"]["stdout"].as_str() == Some("[REDACTED]"),
        "captured stdout was not masked before local persistence"
    );
    ensure!(
        !serde_json::to_string(&checkpoint)?.contains(&canary),
        "private checkpoint exposed the fake credential value"
    );
    let public_events = agentsandbox::runner::AuditClient::new(&url, &runner_token)?
        .get_json(&format!("api/events?sandbox_id={session_id}&limit=1000"))
        .await?;
    ensure!(
        !serde_json::to_string(&public_events)?.contains(&canary),
        "gateway event API exposed the fake credential value"
    );
    {
        let rejected = rejected
            .lock()
            .map_err(|_| anyhow::anyhow!("qualification capture poisoned"))?;
        ensure!(
            rejected.len() == 1,
            "expected exactly one rejected terminal control mutation"
        );
        let WorkerTransitionV1::Observe { observation } = &rejected[0].transition else {
            anyhow::bail!("captured mutation is not a runtime observation");
        };
        let same_instant = |captured: Option<&str>, runtime: &Value| -> Result<bool> {
            let captured = captured.context("captured observation has no runtime timestamp")?;
            let runtime = runtime
                .as_str()
                .context("runtime inspect has no timestamp")?;
            Ok(chrono::DateTime::parse_from_rfc3339(captured)?
                == chrono::DateTime::parse_from_rfc3339(runtime)?)
        };
        ensure!(
            rejected[0].worker_id == worker.worker_id
                && rejected[0].attempt_id == worker.attempt.attempt_id
                && observation.inspection.container_id == container_id
                && same_instant(observation.started_at.as_deref(), &state["StartedAt"])?
                && same_instant(observation.finished_at.as_deref(), &state["FinishedAt"])?
                && observation.exit_code == state["ExitCode"].as_i64(),
            "retained instance does not match the rejected observation"
        );
    }
    for kind in ["command.completed", "command.failed"] {
        ensure!(
            store
                .events(&EventQuery {
                    kind: Some(kind.into()),
                    ..Default::default()
                })?
                .is_empty(),
            "a compatibility terminal must not precede its rejected control terminal"
        );
    }
    ensure!(store.integrity()?.valid, "earlier audit must remain valid");
    server.abort();
    let _ = server.await;
    drop(store);
    let reopened = Store::open_for_tenant(&database, "local", Redactor::new(vec![]), 1_000)?;
    ensure!(
        reopened.integrity()?.valid,
        "audit must remain valid after reopening the database"
    );
    let recovered = reopened
        .worker(&worker.worker_id)?
        .context("worker disappeared after restart")?;
    ensure!(
        recovered.state == WorkerStateV1::InDoubt && recovered.quota_held,
        "restart lost retained unknown worker state"
    );
    ensure!(
        reopened.worker_usage()?.is_empty()
            && reopened
                .worker_quota(&QuotaPolicyV1::default())?
                .held_workers
                == 1,
        "restart fabricated allocation or released unknown quota"
    );
    ensure!(
        fs::read_to_string(project.join("original.txt"))? == "original remains unchanged\n",
        "original project changed"
    );
    eprintln!(
        "PASS terminal control HTTP503; retained instance {container_id}; session {session_id}; audit valid; allocation absent; unknown quota retained; capture masked; log stdout bytes {}; log stderr bytes {}; log command success {}",
        log_result.stdout.len(),
        log_result.stderr.len(),
        log_result.status.success()
    );
    Ok(())
}
