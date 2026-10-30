use agentsandbox::{
    model::{CommandSpec, DecisionRequest},
    policy::request_hash,
    runner::{AuditClient, Authorization},
};
use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn interactive_cli_refuses_pipes_before_project_import_or_admission() {
    let temporary = tempfile::tempdir().unwrap();
    let project = temporary.path().join("does-not-exist-project");
    let config = temporary.path().join("does-not-exist-config");
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .arg("run")
        .arg("--interactive")
        .arg("--project")
        .arg(&project)
        .arg("--config-dir")
        .arg(&config)
        .args(["--", "/usr/bin/id"])
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(
        diagnostic.contains("--interactive requires terminal stdin, stdout and stderr"),
        "{diagnostic}"
    );
    assert!(!project.exists() && !config.exists());
}

#[tokio::test]
async fn interactive_capture_is_refused_by_programmatic_api_before_filesystem_or_network() {
    use agentsandbox::{
        runner::RunOptions,
        runtime::{NetworkMode, RuntimeKind},
    };
    let temporary = tempfile::tempdir().unwrap();
    let options = RunOptions {
        project: temporary.path().join("missing-project"),
        config_dir: temporary.path().join("missing-config"),
        gateway: "http://127.0.0.1:1".into(),
        runtime: RuntimeKind::Docker,
        runtime_program: None,
        runtime_endpoint: None,
        image: "worker:fixture".into(),
        profile: "strict".into(),
        network: NetworkMode::None,
        allow_rootful_runtime: false,
        timeout: 5,
        provider: "generic".into(),
        approve_hash: None,
        capture_output: true,
        interactive: true,
        secret_env_file: None,
        client: None,
        auth_mode: None,
        client_profile: None,
        providers_file: None,
        prompt: None,
        model: None,
        argv: vec!["/usr/bin/id".into()],
    };
    let mut legacy = options.clone();
    legacy.interactive = false;
    assert!(
        serde_json::to_value(&legacy)
            .unwrap()
            .get("interactive")
            .is_none()
    );
    let mut legacy_json = serde_json::to_value(&legacy).unwrap();
    assert!(
        !serde_json::from_value::<RunOptions>(legacy_json.clone())
            .unwrap()
            .interactive
    );
    legacy_json["interactive"] = json!(true);
    assert!(
        serde_json::from_value::<RunOptions>(legacy_json)
            .unwrap()
            .interactive
    );
    let error = agentsandbox::runner::run(options).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("--interactive cannot be combined with --capture-output")
    );
    assert!(!temporary.path().join("missing-project").exists());
    assert!(!temporary.path().join("missing-config").exists());
}

fn request() -> DecisionRequest {
    DecisionRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        profile: "review".into(),
        command: CommandSpec {
            argv: vec!["cargo".into(), "test".into()],
            cwd: "/workspace".into(),
        },
        project_id: None,
        sandbox_id: None,
        agent_id: None,
        session_id: None,
        correlation_id: None,
        trace_id: None,
        human_approved: false,
        approval_hash: None,
        execution_mode: None,
        provider_resolution: None,
    }
}

async fn fixture(
    outcome: &str,
) -> (
    String,
    Arc<Mutex<Vec<DecisionRequest>>>,
    tokio::task::JoinHandle<()>,
) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let fixed = outcome.to_string();
    let app=Router::new().route("/api/decide",post(move |State(calls):State<Arc<Mutex<Vec<DecisionRequest>>>>,Json(req):Json<DecisionRequest>| {
        let fixed=fixed.clone();async move {
            calls.lock().unwrap().push(req.clone());
            let outcome=if fixed=="review"&&req.human_approved {"allow"}else{fixed.as_str()};
            Json(json!({"decision":{"outcome":outcome,"engine":"deterministic","rule":"fixture","reason":"fixture"},"request_hash":request_hash(&req),"audit_seq":1,"jev":null}))
        }
    })).with_state(calls.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), calls, server)
}

#[tokio::test]
async fn denied_command_cannot_be_promoted_by_human_approval() {
    let (url, calls, server) = fixture("deny").await;
    let req = request();
    let client = AuditClient::new(&url, "test-runner-token-32-characters-long").unwrap();
    assert!(
        client
            .authorize(&req, Some(&request_hash(&req)))
            .await
            .is_err()
    );
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert!(!calls.lock().unwrap()[0].human_approved);
    server.abort();
}

#[tokio::test]
async fn review_requires_the_exact_request_hash_and_resubmits_unchanged_identity() {
    let (url, calls, server) = fixture("review").await;
    let req = request();
    let client = AuditClient::new(&url, "test-runner-token-32-characters-long").unwrap();
    assert!(matches!(
        client.authorize(&req, None).await.unwrap(),
        Authorization::Review { .. }
    ));
    assert!(client.authorize(&req, Some(&"0".repeat(64))).await.is_err());
    assert_eq!(calls.lock().unwrap().len(), 2);
    assert!(matches!(
        client
            .authorize(&req, Some(&request_hash(&req)))
            .await
            .unwrap(),
        Authorization::Allowed(_)
    ));
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    assert!(calls[3].human_approved);
    assert_eq!(
        calls[3].approval_hash.as_deref(),
        Some(request_hash(&req).as_str())
    );
    assert_eq!(calls[3].request_id, req.request_id);
    assert_eq!(calls[3].command, req.command);
    server.abort();
}

#[tokio::test]
async fn unavailable_or_unacknowledged_audit_never_yields_allow() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let client = AuditClient::new(
        &format!("http://{address}"),
        "test-runner-token-32-characters-long",
    )
    .unwrap();
    assert!(client.authorize(&request(), None).await.is_err());
    let app = Router::new().route(
        "/api/decide",
        post(|Json(req): Json<DecisionRequest>| async move {
            Json::<Value>(json!({"decision":{"outcome":"allow"},"request_hash":request_hash(&req)}))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = AuditClient::new(
        &format!("http://{address}"),
        "test-runner-token-32-characters-long",
    )
    .unwrap();
    assert!(client.authorize(&request(), None).await.is_err());
    server.abort();
}

#[test]
fn bearer_token_is_never_sent_over_non_loopback_plain_http() {
    assert!(AuditClient::new("http://example.com:8787", "runner-token").is_err());
    assert!(AuditClient::new("http://user:password@127.0.0.1:8787", "runner-token").is_err());
}

#[tokio::test]
async fn known_runner_credential_in_command_is_refused_before_transmission() {
    let (url, calls, server) = fixture("allow").await;
    let token = "private-runner-credential-never-transmitted";
    let client = AuditClient::new(&url, token).unwrap();
    let mut request = request();
    request.command.argv.push(token.into());
    assert!(
        client.authorize(&request, None).await.is_err(),
        "a command whose identity would change during masking must be refused"
    );
    assert!(
        calls.lock().unwrap().is_empty(),
        "known secret reached the gateway request boundary"
    );
    server.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn provider_secrets_in_identity_and_control_secrets_in_injection_never_reach_http() {
    use agentsandbox::{
        operations::initialize_config,
        runner::{RunOptions, read_token},
        runtime::{NetworkMode, RuntimeKind},
    };
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicUsize, Ordering},
    };
    for control_injection in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config");
        initialize_config(&config).unwrap();
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        let value = if control_injection {
            read_token(&config.join("secrets/runner")).unwrap()
        } else {
            "provider-private-fixture-value".into()
        };
        let source = temp.path().join("provider.env");
        fs::write(&source, format!("OPENAI_API_KEY={value}\n")).unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let app = Router::new().fallback(axum::routing::any(move || {
            let observed = observed.clone();
            async move {
                observed.fetch_add(1, Ordering::SeqCst);
                axum::http::StatusCode::SERVICE_UNAVAILABLE
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let argv = if control_injection {
            vec!["/usr/bin/id".into()]
        } else {
            vec!["/usr/bin/echo".into(), value]
        };
        let result = agentsandbox::runner::run(RunOptions {
            project,
            config_dir: config,
            gateway: url,
            runtime: RuntimeKind::Docker,
            runtime_program: None,
            runtime_endpoint: Some("unix:///fixture.sock".into()),
            image: "worker:fixture".into(),
            profile: "observe".into(),
            network: NetworkMode::Bridge,
            allow_rootful_runtime: false,
            timeout: 5,
            provider: "generic".into(),
            approve_hash: None,
            capture_output: false,
            interactive: false,
            secret_env_file: Some(source),
            client: None,
            auth_mode: None,
            client_profile: None,
            providers_file: None,
            prompt: None,
            model: None,
            argv,
        })
        .await;
        assert!(result.is_err());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "sensitive command identity or control credential injection reached HTTP"
        );
        server.abort();
    }
}
