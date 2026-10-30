//! Real CLI + SQLite v2 gateway with targeted HTTP faults and a Rust engine
//! fixture. This proves broker ordering, never Docker/Podman confinement.
#![cfg(unix)]
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
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
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
fn log(root: &Path, line: &str) {
    writeln!(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("calls"))
            .unwrap(),
        "{line}"
    )
    .unwrap();
}
fn run_retained(root: &Path) -> bool {
    fs::read_to_string(root.join("run_id"))
        .is_ok_and(|id| root.join("containers").join(id).join("state").is_file())
}
fn terminal_control(value: &Value) -> Option<WorkerMutationV1> {
    let mutation = serde_json::from_value::<WorkerMutationV1>(value.clone()).ok()?;
    matches!(&mutation.transition,WorkerTransitionV1::Observe{observation} if [RuntimeStatusV1::Exited,RuntimeStatusV1::Dead].contains(&observation.status) && observation.running==Some(false)).then_some(mutation)
}
#[tokio::test]
async fn terminal_audit_precedes_cleanup_and_failures_keep_the_observed_result() {
    let temporary = support::private_tempdir();
    let source = temporary.path().join("engine.rs");
    fs::write(&source, include_str!("runtime_fixtures/runner_terminal.rs")).unwrap();
    let bin = temporary.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let compiled = std::process::Command::new(
        std::env::var_os("RUSTC")
            .map(PathBuf::from)
            .unwrap_or_else(|| "rustc".into()),
    )
    .args(["--edition=2024", "--crate-name", "terminal_audit_fixture"])
    .arg(source)
    .arg("-o")
    .arg(bin.join("docker"))
    .output()
    .unwrap();
    assert!(
        compiled.status.success(),
        "Rust engine fixture compilation failed"
    );
    let mut failures = Vec::<String>::new();
    for case in [
        "acknowledged",
        "terminal-rejected",
        "cleanup-failed",
        "cleanup-report-rejected",
        "cleanup-response-lost",
        "postprocessing-rejected",
        "runtime-error-secret",
        "runtime-error-capture-secret",
        "output-boundary-secret",
        "capture-boundary-secret",
        "adapter-boundary-secret",
    ] {
        let root = temporary.path().join(case);
        std::os::unix::fs::DirBuilderExt::mode(&mut fs::DirBuilder::new(), 0o700)
            .create(&root)
            .unwrap();
        let config = root.join("config");
        initialize_config(&config).unwrap();
        if matches!(
            case,
            "capture-boundary-secret" | "runtime-error-capture-secret"
        ) {
            // Long enough that replacing full occurrences brings the final
            // capture fragment under the separate 8192-byte display limit.
            fs::write(config.join("secrets/runner"), "Q".repeat(503)).unwrap();
        }
        let project = root.join("project");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("fixture.txt"), "unchanged original").unwrap();
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
        let redactor = Redactor::new(vec![runner.clone(), viewer.clone()]);
        let database = root.join("audit.sqlite");
        let store = Arc::new(Store::open(&database, redactor.clone(), 1000).unwrap());
        let gateway = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: "local".into(),
                store: store.clone(),
                runner_token: runner,
                viewer_token: viewer,
            }],
            Policy::default(),
            None,
            redactor,
        )
        .unwrap();
        let terminal_checks = Arc::new(Mutex::new(Vec::new()));
        let event_checks = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::new(Mutex::new(Vec::<WorkerMutationV1>::new()));
        let (event_root, checks, event_safety, submissions, read_store) = (
            root.clone(),
            terminal_checks.clone(),
            event_checks.clone(),
            captured.clone(),
            store.clone(),
        );
        let app = gateway_router(gateway).layer(middleware::from_fn(
            move |request: Request, next: Next| {
                let (root, checks, event_safety, submissions, store) = (
                    event_root.clone(),
                    checks.clone(),
                    event_safety.clone(),
                    submissions.clone(),
                    read_store.clone(),
                );
                async move {
                    if request.method() != Method::POST {
                        return next.run(request).await;
                    }
                    let control = request.uri().path().starts_with("/api/v1/workers/")
                        && request.uri().path().ends_with("/transitions");
                    let events = ["/api/events", "/api/v1/events"].contains(&request.uri().path());
                    if !control && !events {
                        return next.run(request).await;
                    }
                    let (parts, body) = request.into_parts();
                    let Ok(bytes) = to_bytes(body, 128 * 1024).await else {
                        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
                    };
                    let value = serde_json::from_slice::<Value>(&bytes).unwrap();
                    if control && let Some(mutation) = terminal_control(&value) {
                        let retained = run_retained(&root);
                        let fresh = {
                            let mut recorded = submissions.lock().unwrap();
                            let fresh = !recorded
                                .iter()
                                .any(|old| old.operation_id == mutation.operation_id);
                            if fresh {
                                recorded.push(mutation.clone());
                            }
                            fresh
                        };
                        if case == "terminal-rejected" {
                            if fresh {
                                checks.lock().unwrap().push(retained);
                            }
                            log(&root, "terminal_rejected");
                            return StatusCode::SERVICE_UNAVAILABLE.into_response();
                        }
                        let response = next
                            .run(Request::from_parts(parts, Body::from(bytes)))
                            .await;
                        if response.status().is_success() {
                            let durable =
                                store
                                    .worker(&mutation.worker_id)
                                    .unwrap()
                                    .is_some_and(|worker| {
                                        worker.state == WorkerStateV1::Terminal
                                            && !worker.quota_held
                                    })
                                    && store.worker_usage().unwrap().len() == 1;
                            if fresh {
                                checks.lock().unwrap().push(retained && durable);
                            }
                            fs::write(root.join("terminal_ack"), "durable SQLite terminal")
                                .unwrap();
                            log(&root, "terminal_ack");
                        }
                        return response;
                    }
                    let cleanup_failure = control
                        && serde_json::from_value::<WorkerMutationV1>(value.clone()).is_ok_and(
                            |mutation| {
                                matches!(
                                    mutation.transition,
                                    WorkerTransitionV1::CleanupResult {
                                        removed: false,
                                        error: Some(_)
                                    }
                                )
                            },
                        );
                    if case == "cleanup-report-rejected"
                        && (cleanup_failure || events && value["kind"] == "runtime.cleanup.failed")
                    {
                        log(&root, "cleanup_audit_rejected");
                        return StatusCode::SERVICE_UNAVAILABLE.into_response();
                    }
                    if events {
                        if case == "postprocessing-rejected" && value["kind"] == "observer.coverage"
                        {
                            return StatusCode::SERVICE_UNAVAILABLE.into_response();
                        }
                        if case == "adapter-boundary-secret" && value["kind"] == "tool.result" {
                            let secret =
                                fs::read_to_string(root.join("config/secrets/runner")).unwrap();
                            event_safety
                                .lock()
                                .unwrap()
                                .push(!value.to_string().contains(&secret.trim()[..12]));
                        }
                        if case == "output-boundary-secret"
                            && matches!(
                                value["kind"].as_str(),
                                Some("command.completed" | "command.failed")
                            )
                        {
                            let secret =
                                fs::read_to_string(root.join("config/secrets/runner")).unwrap();
                            event_safety.lock().unwrap().push(
                                !value["result"]["stdout"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .contains(&secret.trim()[..12]),
                            );
                        }
                        if case == "capture-boundary-secret"
                            && matches!(
                                value["kind"].as_str(),
                                Some("command.completed" | "command.failed")
                            )
                        {
                            let result = &value["result"];
                            let safe = ["stdout", "stderr"].into_iter().all(|field| {
                                result[field].as_str().is_some_and(|text| {
                                    text.starts_with("useful worker report\n")
                                        && !text.contains("QQQQQQQQ")
                                })
                            });
                            event_safety.lock().unwrap().push(
                                safe && result["stdout_truncated"] == true
                                    && result["stderr_truncated"] == true
                                    && result["stdout_bytes"]
                                        .as_u64()
                                        .is_some_and(|bytes| bytes > 256 * 1024)
                                    && result["stderr_bytes"]
                                        .as_u64()
                                        .is_some_and(|bytes| bytes > 256 * 1024),
                            );
                        }
                    }
                    next.run(Request::from_parts(parts, Body::from(bytes)))
                        .await
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let endpoint = format!("unix://{}", root.join("fixture.sock").display());
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(25),
            tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
                .args([
                    "run",
                    "--profile",
                    "observe",
                    "--gateway",
                    &url,
                    "--image",
                    "worker:fixture",
                    "--runtime-endpoint",
                    &endpoint,
                ])
                .args(
                    matches!(case, "output-boundary-secret" | "capture-boundary-secret")
                        .then_some("--capture-output"),
                )
                .arg("--config-dir")
                .arg(&config)
                .arg("--project")
                .arg(&project)
                .arg("--runtime-program")
                .arg(&program)
                .args(["--", "/usr/bin/id"])
                .kill_on_drop(true)
                .output(),
        )
        .await
        .expect("runner fixture exceeded its deadline")
        .unwrap();
        server.abort();
        let _ = server.await;
        let workers = store.workers().unwrap();
        if workers.len() != 1 {
            failures.push(format!(
                "{case}: a real durable worker reservation was not used"
            ));
        }
        let ledger = store.worker_usage().unwrap();
        let quota = store.worker_quota(&QuotaPolicyV1::default()).unwrap();
        let secret = fs::read_to_string(config.join("secrets/runner")).unwrap();
        if output
            .stderr
            .windows(secret.trim().len())
            .any(|bytes| bytes == secret.trim().as_bytes())
            || output
                .stdout
                .windows(secret.trim().len())
                .any(|bytes| bytes == secret.trim().as_bytes())
        {
            failures.push(format!(
                "{case}: a known secret escaped in the CLI response"
            ));
        }
        let events = store
            .events(&EventQuery {
                limit: Some(1000),
                ..Default::default()
            })
            .unwrap();
        if events.iter().any(|event| {
            serde_json::to_string(event)
                .unwrap()
                .contains(secret.trim())
        }) {
            failures.push(format!("{case}: a known secret escaped durable masking"));
        }
        if matches!(
            case,
            "runtime-error-secret" | "runtime-error-capture-secret"
        ) {
            if case == "runtime-error-capture-secret" {
                let diagnostic = String::from_utf8_lossy(&output.stderr);
                let reason = workers
                    .first()
                    .and_then(|worker| worker.unknown_reason.as_deref())
                    .unwrap_or_default();
                let audit = serde_json::to_string(&events).unwrap();
                if !diagnostic.contains("useful control report")
                    || !diagnostic.contains("stderr_truncated=true")
                    || diagnostic.contains("QQQQQQQQ")
                    || !reason.contains("useful control report")
                    || !reason.contains("stderr_truncated=true")
                    || reason.contains("QQQQQQQQ")
                    || !audit.contains("useful control report")
                    || audit.contains("QQQQQQQQ")
                {
                    failures.push(format!(
                        "runtime-error-capture-secret: useful_cli={}, flag_cli={}, safe_cli={}, useful_reason={}, flag_reason={}, safe_reason={}, useful_audit={}, safe_audit={}",
                        diagnostic.contains("useful control report"), diagnostic.contains("stderr_truncated=true"), !diagnostic.contains("QQQQQQQQ"),
                        reason.contains("useful control report"), reason.contains("stderr_truncated=true"), !reason.contains("QQQQQQQQ"),
                        audit.contains("useful control report"), !audit.contains("QQQQQQQQ"),
                    ));
                }
            }
            if output.status.success()
                || !captured.lock().unwrap().is_empty()
                || !ledger.is_empty()
                || quota.held_workers != 1
            {
                failures.push("runtime-error-secret: an unobserved terminal result or released quota was reported".into());
            }
            assert!(store.integrity().unwrap().valid);
            continue;
        }
        if *terminal_checks.lock().unwrap() != vec![true] || captured.lock().unwrap().len() != 1 {
            failures.push(format!("{case}: terminal control did not retain the instance or prove its durable allocation"));
        }
        if matches!(
            case,
            "adapter-boundary-secret" | "output-boundary-secret" | "capture-boundary-secret"
        ) && *event_checks.lock().unwrap() != vec![true]
        {
            failures.push(format!(
                "{case}: masking before the projection boundary was not proved"
            ));
        }
        let calls = fs::read_to_string(root.join("calls")).unwrap();
        let ack = calls.lines().position(|line| line == "terminal_ack");
        let removal = calls
            .lines()
            .position(|line| line.starts_with("rm ") && line.ends_with(" run"));
        if case == "terminal-rejected" {
            if output.status.success()
                || removal.is_some()
                || !run_retained(&root)
                || !ledger.is_empty()
                || quota.held_workers != 1
                || workers.first().is_none_or(|worker| {
                    worker.state != WorkerStateV1::InDoubt || !worker.quota_held
                })
            {
                failures.push("terminal-rejected: control failure allowed cleanup, allocation, terminal state or quota release".into());
            }
        } else {
            if !matches!((ack,removal),(Some(ack),Some(rm)) if ack<rm) {
                failures.push(format!(
                    "{case}: cleanup did not follow the real SQLite terminal acknowledgement"
                ));
            }
            if ledger.len() != 1
                || ledger[0].billable
                || quota.held_workers != 0
                || workers
                    .first()
                    .is_none_or(|worker| worker.state != WorkerStateV1::Terminal)
            {
                failures.push(format!(
                    "{case}: committed terminal allocation was lost or duplicated"
                ));
            }
            let summary = serde_json::from_slice::<Value>(&output.stdout).ok();
            let removed = matches!(
                case,
                "acknowledged"
                    | "postprocessing-rejected"
                    | "output-boundary-secret"
                    | "capture-boundary-secret"
                    | "adapter-boundary-secret"
            );
            if summary.as_ref().is_none_or(|value| {
                value["exit_code"] != 0
                    || value["cleanup"]["state"] != if removed { "removed" } else { "unconfirmed" }
            }) {
                failures.push(format!("{case}: observed exit/cleanup result was lost"));
            }
            let audit = match case {
                "cleanup-report-rejected" => "unacknowledged",
                "cleanup-failed" | "cleanup-response-lost" => "acknowledged",
                _ => "not_applicable",
            };
            if summary
                .as_ref()
                .is_none_or(|value| value["cleanup"]["error_audit"] != audit)
            {
                failures.push(format!(
                    "{case}: cleanup error acknowledgement was misrepresented"
                ));
            }
            if output.status.success()
                != matches!(
                    case,
                    "acknowledged"
                        | "output-boundary-secret"
                        | "capture-boundary-secret"
                        | "adapter-boundary-secret"
                )
            {
                failures.push(format!(
                    "{case}: CLI exit did not report cleanup/postprocessing status"
                ));
            }
            if case == "postprocessing-rejected"
                && summary
                    .as_ref()
                    .is_none_or(|value| !value["postprocessing_error"].is_string())
            {
                failures.push("postprocessing-rejected: optional observation failure was confused with the terminal result".into());
            }
        }
        let expected_retained = matches!(
            case,
            "terminal-rejected" | "cleanup-failed" | "cleanup-report-rejected"
        );
        if run_retained(&root) != expected_retained {
            failures.push(format!(
                "{case}: wrong retention of the owned worker instance"
            ));
        }
        if case == "cleanup-report-rejected"
            && calls
                .lines()
                .skip_while(|line| *line != "cleanup_audit_rejected")
                .skip(1)
                .any(|line| line.starts_with("create ") || line.starts_with("start "))
        {
            failures.push(
                "cleanup-report-rejected: a helper ran after the unacknowledged cleanup error"
                    .into(),
            );
        }
        assert!(store.integrity().unwrap().valid);
        assert_eq!(
            fs::read_to_string(project.join("fixture.txt")).unwrap(),
            "unchanged original"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
