mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    jev::JevClient,
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::json;
use std::{net::SocketAddr, path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
};
use tower::ServiceExt;

struct GatewayFixture {
    _directory: tempfile::TempDir,
    provider: std::net::TcpListener,
    command: Command,
    credentials: Vec<String>,
    databases: Vec<PathBuf>,
}

fn gateway_fixture(registry: bool, reused_role: Option<&str>) -> GatewayFixture {
    let mut fixture = gateway_control_fixture(registry, reused_role);
    add_typesafe_options(&mut fixture.command, fixture.provider.local_addr().unwrap());
    fixture
}

fn add_typesafe_options(command: &mut Command, address: SocketAddr) {
    command.args([
        "--jev-url",
        &format!("http://{address}/fixture-only"),
        "--jev-key-file",
        "jev",
        "--jev-model",
        "fixture",
    ]);
}

fn gateway_control_fixture(registry: bool, reused_role: Option<&str>) -> GatewayFixture {
    let directory = support::private_tempdir();
    let provider = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    provider.set_nonblocking(true).unwrap();
    let mut credentials = Vec::new();
    let mut tenants = Vec::new();
    let mut databases = Vec::new();
    for tenant in if registry {
        &['a', 'b'][..]
    } else {
        &['a'][..]
    } {
        for role in ["runner", "viewer"] {
            let value = format!("fixture-tenant-{tenant}-{role}-{}", "x".repeat(40));
            std::fs::write(directory.path().join(format!("{tenant}-{role}")), &value).unwrap();
            credentials.push(value);
        }
        databases.push(directory.path().join(format!("{tenant}.sqlite")));
        tenants.push(json!({
            "tenant_id": format!("tenant-{tenant}"),
            "data": format!("{tenant}.sqlite"),
            "runner_token_file": format!("{tenant}-runner"),
            "viewer_token_file": format!("{tenant}-viewer")
        }));
    }
    let key = if let Some(role) = reused_role {
        let tenant = if registry { 'b' } else { 'a' };
        std::fs::read_to_string(directory.path().join(format!("{tenant}-{role}"))).unwrap()
    } else {
        format!("fixture-jev-distinct-{}", "j".repeat(40))
    };
    std::fs::write(directory.path().join("jev"), &key).unwrap();
    credentials.push(key);
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .env_clear()
        .current_dir(directory.path())
        .args(["gateway", "--listen", "127.0.0.1:0", "--policy"])
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("policies/default.json"))
        .arg("--jev-allow-local-http")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if registry {
        std::fs::write(
            directory.path().join("tenants.json"),
            json!({"version": 1, "tenants": tenants}).to_string(),
        )
        .unwrap();
        command.args(["--tenants-file", "tenants.json"]);
    } else {
        command.args([
            "--data",
            "a.sqlite",
            "--runner-token-file",
            "a-runner",
            "--viewer-token-file",
            "a-viewer",
        ]);
    }
    GatewayFixture {
        _directory: directory,
        provider,
        command,
        credentials,
        databases,
    }
}

fn assert_no_credentials(text: &str, credentials: &[String]) {
    for credential in credentials {
        assert!(!text.contains(credential), "credential in gateway output");
    }
}

fn assert_no_provider_contact(provider: &std::net::TcpListener) {
    assert!(
        matches!(provider.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "startup must not contact the Jev provider"
    );
}

async fn assert_reuse_rejected(registry: bool, role: &str) {
    let mut fixture = gateway_fixture(registry, Some(role));
    let mut child = fixture.command.spawn().unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut diagnostic = String::new();
    tokio::time::timeout(Duration::from_secs(10), stderr.read_line(&mut diagnostic))
        .await
        .unwrap()
        .unwrap();
    let served = diagnostic.starts_with("Gateway ");
    if served {
        child.start_kill().unwrap();
    }
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    stderr.read_to_string(&mut diagnostic).await.unwrap();
    diagnostic.push_str(&String::from_utf8_lossy(&output.stdout));
    assert_no_credentials(&diagnostic, &fixture.credentials);
    assert_no_provider_contact(&fixture.provider);
    assert!(!served, "gateway served with Jev/control credential reuse");
    assert!(!output.status.success());
    assert!(diagnostic.contains("Jev") && diagnostic.contains("distinct"));
    assert!(
        fixture.databases.iter().all(|path| !path.exists()),
        "rejected credentials must not initialize tenant databases"
    );
}

#[tokio::test]
async fn standalone_gateway_refuses_jev_runner_reuse_before_serving() {
    assert_reuse_rejected(false, "runner").await;
}

#[tokio::test]
async fn standalone_gateway_refuses_jev_viewer_reuse_before_serving() {
    assert_reuse_rejected(false, "viewer").await;
}

#[tokio::test]
async fn registry_gateway_refuses_jev_runner_reuse_from_the_second_tenant() {
    assert_reuse_rejected(true, "runner").await;
}

#[tokio::test]
async fn registry_gateway_refuses_jev_viewer_reuse_from_the_second_tenant() {
    assert_reuse_rejected(true, "viewer").await;
}

#[tokio::test]
async fn distinct_jev_key_allows_gateway_startup_without_contacting_the_provider() {
    assert_gateway_starts_without_provider_contact(gateway_fixture(true, None)).await;
}

#[tokio::test]
async fn explicit_local_jev_rejects_typesafe_options_before_database_or_provider_contact() {
    let local_fixture = || {
        let mut fixture = gateway_control_fixture(true, None);
        let address = fixture.provider.local_addr().unwrap();
        fixture.command.args([
            "--jev-backend",
            "local-openai",
            "--jev-local-url",
            &format!("http://{address}/v1/chat/completions"),
            "--jev-local-model",
            "fixture-local-model",
        ]);
        fixture
    };
    let mut mixed = local_fixture();
    let typesafe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    typesafe.set_nonblocking(true).unwrap();
    add_typesafe_options(&mut mixed.command, typesafe.local_addr().unwrap());

    let output = tokio::time::timeout(Duration::from_secs(10), mixed.command.output())
        .await
        .expect("mixed configuration must exit before serving")
        .unwrap();
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert_no_credentials(&diagnostic, &mixed.credentials);
    assert_no_credentials(&String::from_utf8_lossy(&output.stdout), &mixed.credentials);
    assert!(!output.status.success());
    assert!(diagnostic.contains("Incomplete or mixed Jev backend configuration"));
    assert!(mixed.databases.iter().all(|path| !path.exists()));
    assert_no_provider_contact(&mixed.provider);
    assert_no_provider_contact(&typesafe);

    assert_gateway_starts_without_provider_contact(local_fixture()).await;
}

async fn assert_gateway_starts_without_provider_contact(mut fixture: GatewayFixture) {
    let mut child = fixture.command.spawn().unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut diagnostic = String::new();
    tokio::time::timeout(Duration::from_secs(10), stderr.read_line(&mut diagnostic))
        .await
        .unwrap()
        .unwrap();
    assert_no_credentials(&diagnostic, &fixture.credentials);
    assert!(diagnostic.starts_with("Gateway "));
    let address: SocketAddr = diagnostic
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    assert!(address.ip().is_loopback());
    let response = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
        .get(format!("http://{address}/healthz"))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert!(fixture.databases.iter().all(|path| path.is_file()));
    assert!(child.try_wait().unwrap().is_none());
    child.start_kill().unwrap();
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(!output.status.success());
    stderr.read_to_string(&mut diagnostic).await.unwrap();
    diagnostic.push_str(&String::from_utf8_lossy(&output.stdout));
    assert_no_credentials(&diagnostic, &fixture.credentials);
    assert_no_provider_contact(&fixture.provider);
}

fn library_jev(key: String) -> (std::net::TcpListener, JevClient) {
    let provider = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    provider.set_nonblocking(true).unwrap();
    let client = JevClient::new(
        &format!("http://{}/fixture-only", provider.local_addr().unwrap()),
        key,
        "fixture".into(),
        0.85,
        true,
    )
    .unwrap();
    (provider, client)
}

fn library_request(method: &str, path: &str, token: &str, body: String) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .body(Body::from(body))
        .unwrap()
}

async fn assert_library_local_reuse_refuses_authorization(role: &str, padded: bool) {
    let directory = support::private_tempdir();
    let redactor = Redactor::new(vec![]);
    let store = Arc::new(
        Store::open(
            &directory.path().join("audit.sqlite"),
            redactor.clone(),
            100,
        )
        .unwrap(),
    );
    let runner = "fixture-library-runner-11111111111111111111111111111111".to_string();
    let viewer = "fixture-library-viewer-22222222222222222222222222222222".to_string();
    let key = if role == "runner" { &runner } else { &viewer };
    let supplied = if padded {
        format!(" \t{key}\n")
    } else {
        key.clone()
    };
    let (provider, jev) = library_jev(supplied);
    let original_head = store.head().unwrap();
    let state = GatewayState::new(
        store.clone(),
        runner.clone(),
        viewer.clone(),
        Policy::default(),
        Some(jev),
        redactor,
    );
    let app = gateway_router(state);
    for token in [&runner, &viewer] {
        let response = app
            .clone()
            .oneshot(library_request(
                "GET",
                "/api/v1/identity",
                token,
                String::new(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_no_credentials(
            &String::from_utf8_lossy(&bytes),
            &[runner.clone(), viewer.clone()],
        );
    }
    let response = app
        .oneshot(library_request(
            "POST",
            "/api/v1/decide",
            &runner,
            json!({"request_id":uuid::Uuid::new_v4(),"profile":"strict","command":{"argv":["cargo","test"],"cwd":"/workspace"},"human_approved":false}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(store.head().unwrap(), original_head);
    assert_no_provider_contact(&provider);
}

#[tokio::test]
async fn library_gateway_refuses_runner_reuse_before_authorization() {
    assert_library_local_reuse_refuses_authorization("runner", false).await;
}

#[tokio::test]
async fn library_gateway_refuses_viewer_reuse_before_authorization() {
    assert_library_local_reuse_refuses_authorization("viewer", false).await;
}

#[tokio::test]
async fn library_gateway_refuses_padded_runner_key_reuse() {
    assert_library_local_reuse_refuses_authorization("runner", true).await;
}

#[tokio::test]
async fn library_gateway_refuses_padded_viewer_key_reuse() {
    assert_library_local_reuse_refuses_authorization("viewer", true).await;
}

#[tokio::test]
async fn library_distinct_jev_key_keeps_local_control_authorization() {
    let directory = support::private_tempdir();
    let redactor = Redactor::new(vec![]);
    let store = Arc::new(
        Store::open(
            &directory.path().join("audit.sqlite"),
            redactor.clone(),
            100,
        )
        .unwrap(),
    );
    let (provider, jev) = library_jev("fixture-distinct-jev-key-33333333333333333333333333".into());
    let state = GatewayState::new(
        store,
        "fixture-runner-key-11111111111111111111111111111111".into(),
        "fixture-viewer-key-22222222222222222222222222222222".into(),
        Policy::default(),
        Some(jev),
        redactor,
    );
    let response = gateway_router(state)
        .oneshot(library_request(
            "GET",
            "/api/v1/identity",
            "fixture-viewer-key-22222222222222222222222222222222",
            String::new(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let identity: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(identity["role"], "viewer");
    assert_no_provider_contact(&provider);
}

fn assert_library_registry_reuse_refused(role: &str, padded: bool) {
    let directory = support::private_tempdir();
    let redactor = Redactor::new(vec![]);
    let mut tenants = Vec::new();
    let mut credentials = Vec::new();
    for tenant in ['a', 'b'] {
        let runner = format!("fixture-library-{tenant}-runner-{}", "r".repeat(32));
        let viewer = format!("fixture-library-{tenant}-viewer-{}", "v".repeat(32));
        credentials.extend([runner.clone(), viewer.clone()]);
        let identity = format!("tenant-{tenant}");
        tenants.push(TenantGateway {
            tenant_id: identity.clone(),
            store: Arc::new(
                Store::open_for_tenant(
                    &directory.path().join(format!("{tenant}.sqlite")),
                    &identity,
                    redactor.clone(),
                    100,
                )
                .unwrap(),
            ),
            runner_token: runner,
            viewer_token: viewer,
        });
    }
    let key = if role == "runner" {
        &tenants[1].runner_token
    } else {
        &tenants[1].viewer_token
    };
    let supplied = if padded {
        format!(" \t{key}\n")
    } else {
        key.clone()
    };
    let (provider, jev) = library_jev(supplied);
    let result = GatewayState::for_tenants(tenants, Policy::default(), Some(jev), redactor);
    let diagnostic = match result {
        Ok(_) => panic!("library assembly accepted Jev/control credential reuse"),
        Err(error) => format!("{error:#}"),
    };
    assert!(diagnostic.contains("Jev") && diagnostic.contains("distinct"));
    assert_no_credentials(&diagnostic, &credentials);
    assert_no_provider_contact(&provider);
}

#[test]
fn library_registry_refuses_second_tenant_runner_key_reuse() {
    assert_library_registry_reuse_refused("runner", false);
}

#[test]
fn library_registry_refuses_second_tenant_viewer_key_reuse() {
    assert_library_registry_reuse_refused("viewer", false);
}

#[test]
fn library_registry_refuses_second_tenant_padded_runner_key_reuse() {
    assert_library_registry_reuse_refused("runner", true);
}

#[test]
fn library_registry_refuses_second_tenant_padded_viewer_key_reuse() {
    assert_library_registry_reuse_refused("viewer", true);
}
