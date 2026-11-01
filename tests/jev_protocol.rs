//! Local HTTP protocol fixtures and real API/audit. No model qualification.
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    jev::{JevBackend, RuleCatalog, SelectionSource},
    model::DecisionRequest,
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    response::Response,
    routing::post,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::{sync::oneshot, task::JoinHandle, time::Duration};
use tower::ServiceExt;

struct Server {
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
impl Server {
    async fn finish(mut self) -> Result<(), &'static str> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let task = self.task.as_mut().ok_or("fixture_task_missing")?;
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .map_err(|_| "fixture_shutdown_budget")?
            .map_err(|_| "fixture_join_failed")?
            .map_err(|_| "fixture_server_failed")?;
        self.task.take();
        Ok(())
    }
}
#[derive(Default)]
struct Capture {
    requests: Vec<Value>,
    authorization: Vec<bool>,
}
async fn server(replies: Vec<(StatusCode, Vec<u8>)>) -> (String, Arc<Mutex<Capture>>, Server) {
    let captures = Arc::new(Mutex::new(Capture::default()));
    let captured = Arc::clone(&captures);
    let replies = Arc::new(replies);
    let app = Router::new().route(
        "/fixture",
        post(move |request: Request<Body>| {
            let captured = Arc::clone(&captured);
            let replies = Arc::clone(&replies);
            async move {
                let authorization = request.headers().contains_key("authorization");
                let bytes = axum::body::to_bytes(request.into_body(), 65_536).await;
                let body = bytes
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .unwrap_or(Value::Null);
                let mut capture = captured.lock().unwrap_or_else(|poison| poison.into_inner());
                let index = capture.requests.len();
                capture.requests.push(body);
                capture.authorization.push(authorization);
                let (status, bytes) = replies
                    .get(index)
                    .cloned()
                    .unwrap_or((StatusCode::INTERNAL_SERVER_ERROR, Vec::new()));
                let mut response = Response::new(Body::from(bytes));
                *response.status_mut() = status;
                response
                    .headers_mut()
                    .insert("content-type", "application/json".parse().unwrap());
                response
                    .headers_mut()
                    .insert("location", "/fixture".parse().unwrap());
                response
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    (
        format!("http://{address}/fixture"),
        captures,
        Server {
            stop: Some(stop),
            task: Some(task),
        },
    )
}
fn request(argv: &[&str]) -> DecisionRequest {
    serde_json::from_value(json!({"request_id":uuid::Uuid::new_v4(),"profile":"strict","command":{"argv":argv,"cwd":"/workspace"}})).unwrap()
}
async fn decide(app: Router, request: &DecisionRequest) -> Result<Value, &'static str> {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/decide")
                .header("authorization", format!("Bearer {}", "r".repeat(48)))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(request).map_err(|_| "request_json")?,
                ))
                .map_err(|_| "request_build")?,
        )
        .await
        .map_err(|_| "api_service")?;
    if response.status() != StatusCode::OK {
        return Err("api_status");
    }
    let bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|_| "api_body")?
        .to_bytes();
    serde_json::from_slice(&bytes).map_err(|_| "api_json")
}

#[tokio::test]
async fn rust_rules_keep_unknown_visible_and_cannot_expand_the_mandatory_policy() {
    let directory = support::private_tempdir();
    let store = Arc::new(
        Store::open(
            &directory.path().join("audit.sqlite"),
            Redactor::new(vec![]),
            100,
        )
        .unwrap(),
    );
    let app = gateway_router(GatewayState::new_with_backend(
        Arc::clone(&store),
        "r".repeat(48),
        "v".repeat(48),
        Policy::default(),
        Some(JevBackend::rust_rules(
            RuleCatalog::default(),
            SelectionSource::Programmatic,
        )),
        Redactor::new(vec![]),
    ));
    let allowed = request(&["/usr/bin/id"]);
    let unknown = request(&["/bin/echo", "sandbox-ok"]);
    let veto = request(&["/usr/bin/true"]);
    let allow = decide(app.clone(), &allowed).await.unwrap();
    let review = decide(app.clone(), &unknown).await.unwrap();
    let denied = decide(app, &veto).await.unwrap();
    assert_eq!(allow["decision"]["outcome"], "allow");
    assert_eq!(allow["jev"]["backend"], "rust_rules");
    assert_eq!(review["decision"]["outcome"], "deny");
    assert_eq!(review["jev"]["outcome"], "review");
    assert!(review["jev"]["confidence"].is_null() && review["jev"]["usage"].is_null());
    assert_eq!(denied["decision"]["rule"], "strict_default_deny");
    assert_eq!(denied["jev"]["status"], "skipped_local_policy");
    let stored = store.decision(&unknown, None).unwrap().unwrap();
    assert_eq!(stored.event.data["jev"], review["jev"]);
    assert!(
        review["jev"]["config_sha256"]
            .as_str()
            .is_some_and(|digest| digest.len() == 64)
    );
}

fn local_reply(outcome: &str, reason: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"model":"fixture-reported-local","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":json!({"outcome":outcome,"reason":reason}).to_string()}}],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}})).unwrap()
}
#[tokio::test]
async fn local_advisory_verdicts_are_audited_masked_and_replay_never_calls_again() {
    let runner = "r".repeat(48);
    let stored_only = "StoredOnlyCanaryGHIJKLMNOPQRSTUV";
    let (url, captures, server) = server(
        ["allow", "deny", "review", "unknown"]
            .into_iter()
            .map(|outcome| {
                (
                    StatusCode::OK,
                    local_reply(outcome, &format!("fixture {runner}")),
                )
            })
            .collect(),
    )
    .await;
    let directory = support::private_tempdir();
    let store = Arc::new(
        Store::open(
            &directory.path().join("audit.sqlite"),
            Redactor::new(vec![stored_only.into()]),
            100,
        )
        .unwrap(),
    );
    let observed = async {
        let backend = JevBackend::local_openai(
            &url,
            None,
            "fixture-requested-local".into(),
            true,
            SelectionSource::Programmatic,
        )
        .map_err(|_| "backend")?;
        let app = gateway_router(GatewayState::new_with_backend(
            Arc::clone(&store),
            runner.clone(),
            "v".repeat(48),
            Policy::default(),
            Some(backend),
            Redactor::new(vec![]),
        ));
        let mut results = Vec::new();
        for _ in 0..4 {
            let mut request = request(&["/usr/bin/id"]);
            request.project_id = Some(stored_only.into());
            let result = decide(app.clone(), &request).await?;
            let replay = decide(app.clone(), &request).await?;
            let stored = store
                .decision(&request, None)
                .map_err(|_| "audit")?
                .ok_or("audit_missing")?;
            results.push((result, replay, stored));
        }
        Ok::<_, &'static str>(results)
    }
    .await;
    let cleanup = server.finish().await;
    assert!(cleanup.is_ok(), "owned fixture cleanup");
    let observed = observed.expect("collect real local API/audit observations");
    for ((result, replay, stored), provider) in
        observed.iter().zip(["allow", "deny", "review", "unknown"])
    {
        assert_eq!(result["decision"]["outcome"], "deny");
        assert_eq!(result["jev"]["provider_outcome"], provider);
        assert_eq!(
            result["jev"]["outcome"],
            if provider == "deny" { "deny" } else { "review" }
        );
        assert_eq!(result["jev"]["backend"], "local_openai");
        assert!(result["jev"]["confidence"].is_null());
        assert_eq!(result["jev"]["requested_model"], "fixture-requested-local");
        assert_eq!(result["jev"]["model"], "fixture-reported-local");
        assert_eq!(result["jev"]["usage"]["total_tokens"], 10);
        assert!(!result.to_string().contains(&runner));
        assert_eq!(result, replay);
        assert_eq!(stored.event.data["jev"], result["jev"]);
    }
    let captures = captures.lock().unwrap_or_else(|poison| poison.into_inner());
    assert_eq!(captures.requests.len(), 4);
    assert!(captures.authorization.iter().all(|present| !present));
    assert!(
        captures
            .requests
            .iter()
            .all(|body| body["model"] == "fixture-requested-local"
                && body["stream"] == false
                && !body.to_string().contains(stored_only)
                && body.to_string().contains("[REDACTED]"))
    );
}

#[test]
fn local_credentials_are_normalized_and_never_reuse_any_control_role() {
    let runner = "r".repeat(48);
    let viewer = "v".repeat(48);
    for credential in [&runner, &viewer] {
        let backend = JevBackend::local_openai(
            "http://127.0.0.1:1/fixture",
            Some(format!(" {credential} ")),
            "fixture".into(),
            true,
            SelectionSource::Programmatic,
        )
        .unwrap();
        assert!(
            backend
                .validate_control_credentials([runner.as_str(), viewer.as_str()])
                .is_err()
        );
    }
    assert!(
        JevBackend::local_openai(
            "http://127.0.0.1:1/fixture",
            Some("   ".into()),
            "fixture".into(),
            true,
            SelectionSource::Programmatic
        )
        .is_err()
    );
    for short in ["a", "ab", "abc"] {
        assert!(
            JevBackend::local_openai(
                "http://127.0.0.1:1/fixture",
                Some(short.into()),
                "fixture".into(),
                true,
                SelectionSource::Programmatic
            )
            .is_err()
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn exact_catalogue_is_private_retained_and_replacement_does_not_reload_it() {
    use std::os::unix::fs::PermissionsExt;
    let directory = support::private_tempdir();
    let path = directory.path().join("catalogue.json");
    std::fs::write(
        &path,
        br#"{"version":1,"allow_argv":[["/bin/echo","sandbox-ok"]],"deny_argv":[]}"#,
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let catalogue = RuleCatalog::from_private_file(&path).unwrap();
    let store = Arc::new(
        Store::open(
            &directory.path().join("audit.sqlite"),
            Redactor::new(vec![]),
            100,
        )
        .unwrap(),
    );
    let app = gateway_router(GatewayState::new_with_backend(
        store,
        "r".repeat(48),
        "v".repeat(48),
        Policy::default(),
        Some(JevBackend::rust_rules(
            catalogue,
            SelectionSource::Programmatic,
        )),
        Redactor::new(vec![]),
    ));
    let first = decide(app.clone(), &request(&["/bin/echo", "sandbox-ok"]))
        .await
        .unwrap();
    assert_eq!(first["decision"]["outcome"], "allow");
    assert_eq!(first["jev"]["configuration"]["source"], "private_file");
    std::fs::write(
        &path,
        br#"{"version":1,"allow_argv":[["/usr/bin/id"]],"deny_argv":[]}"#,
    )
    .unwrap();
    let replaced = decide(app, &request(&["/bin/echo", "sandbox-ok"]))
        .await
        .unwrap();
    assert_eq!(replaced["decision"]["outcome"], "deny");
    assert_eq!(replaced["jev"]["error"], "invalid_catalog");
}

#[tokio::test]
async fn rust_rules_recognize_only_a_validated_managed_bootstrap_structure() {
    let directory = support::private_tempdir();
    let store = Arc::new(
        Store::open(
            &directory.path().join("audit.sqlite"),
            Redactor::new(vec![]),
            100,
        )
        .unwrap(),
    );
    let app = gateway_router(GatewayState::new_with_backend(
        store,
        "r".repeat(48),
        "v".repeat(48),
        Policy::default(),
        Some(JevBackend::rust_rules(
            RuleCatalog::default(),
            SelectionSource::Programmatic,
        )),
        Redactor::new(vec![]),
    ));
    let value = json!({"request_id":uuid::Uuid::new_v4(),"profile":"observe","command":{"cwd":"/workspace","argv":["/usr/local/bin/ai-guard-provider","exec","--client","codex","--auth-mode","api","--transport","batch","--preset-version","1","--model","fixture-model","--prompt","fixture task"]},
        "provider_resolution":{"version":1,"client":"codex","auth_mode":"api","transport":"batch","profile_id":"fixture-api","profile_revision":1,"model":"fixture-model","credential_binding_id":uuid::Uuid::new_v4(),"credential_names":["AI_GUARD_PROVIDER_BOOTSTRAP"],"invocation_preset_version":1,"provenance":{"client":"cli","auth_mode":"cli","profile":"profile","model":"profile"}}});
    let valid: DecisionRequest = serde_json::from_value(value.clone()).unwrap();
    let positive = decide(app.clone(), &valid).await.unwrap();
    assert_eq!(positive["decision"]["outcome"], "allow");
    assert_eq!(
        positive["jev"]["reason"],
        "validated_managed_bootstrap_structure_only"
    );
    assert_eq!(
        positive["jev"]["coverage"],
        "managed_bootstrap_structure_only_no_transitive_mediation"
    );
    assert!(positive["jev"]["confidence"].is_null() && positive["jev"]["usage"].is_null());
    for mutation in 0..5 {
        let mut changed = value.clone();
        changed["request_id"] = json!(uuid::Uuid::new_v4());
        match mutation {
            0 => changed["provider_resolution"]["model"] = json!("different-model"),
            1 => changed["command"]["argv"][11] = json!("different-model"),
            2 => changed["profile"] = json!("strict"),
            3 => changed["execution_mode"] = json!("interactive_tty_v1"),
            _ => {
                changed
                    .as_object_mut()
                    .unwrap()
                    .remove("provider_resolution");
            }
        }
        let invalid: DecisionRequest = serde_json::from_value(changed).unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/decide")
                    .header("authorization", format!("Bearer {}", "r".repeat(48)))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&invalid).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn gateway_fails_closed_if_a_masked_sibling_cannot_be_attached() {
    let directory = support::private_tempdir();
    let path = directory.path().join("audit.sqlite");
    let displaced = directory.path().join("held-audit.sqlite");
    let store = Arc::new(Store::open(&path, Redactor::new(vec![]), 100).unwrap());
    let before = store.head().unwrap();
    std::fs::rename(&path, &displaced).unwrap();
    let single = GatewayState::new_with_backend(
        Arc::clone(&store),
        "r".repeat(48),
        "v".repeat(48),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    );
    let multiple = GatewayState::for_tenants_with_backend(
        vec![TenantGateway {
            tenant_id: store.tenant_id().into(),
            store: Arc::clone(&store),
            runner_token: "r".repeat(48),
            viewer_token: "v".repeat(48),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    );
    let restored = std::fs::rename(&displaced, &path);
    if restored.is_err() {
        let _ = directory.keep();
    }
    assert!(
        restored.is_ok(),
        "restore the owned fixture backing before any oracle"
    );
    let failure = multiple
        .err()
        .expect("multi-tenant assembly must refuse a missing live backing");
    assert_eq!(
        failure.to_string(),
        "gateway audit redaction attachment failed"
    );
    let app = gateway_router(single);
    let health = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::SERVICE_UNAVAILABLE);
    let health_bytes = axum::body::to_bytes(health.into_body(), 4096)
        .await
        .unwrap();
    let health_value: Value = serde_json::from_slice(&health_bytes).unwrap();
    assert_eq!(health_value["status"], "unavailable");
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/events")
                .header("authorization", format!("Bearer {}", "r".repeat(48)))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&agentsandbox::model::Event::new("fixture.refused"))
                        .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(store.head().unwrap(), before);
}
