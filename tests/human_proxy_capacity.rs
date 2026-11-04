//! Real SQLite/gateway HTTP/dashboard regression for a durable read refusal.
//! The additional hostile HTTP fixtures test the proxy boundary only; they do
//! not stand in for the real Store budget oracle or qualify an identity service.
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, dashboard_router, gateway_router},
    audit::Store,
    identity::*,
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tower::ServiceExt;
use uuid::Uuid;

struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn serve(router: Router) -> (Server, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = Server(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    }));
    (server, format!("http://{address}"))
}

fn proxy(upstream: &str, root: &Path) -> Router {
    let front = root.join(Uuid::new_v4().to_string());
    std::fs::create_dir(&front).unwrap();
    std::fs::write(front.join("index.html"), "<title>test frontend</title>").unwrap();
    dashboard_router(upstream, "proxy-fixture-viewer".repeat(3), front).unwrap()
}

async fn proxy_get(router: &Router, token: &str, context: &str) -> (StatusCode, HeaderMap, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/human/resources")
                .header("authorization", format!("Bearer {token}"))
                .header("x-aiguard-context", context)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap())
}

fn change(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "proxy-fixture-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    }
}

fn provision(store: &Store, token: &str) {
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
        account_id: Uuid::new_v4().to_string(),
    };
    store
        .provision_local_human(&LocalHumanProvisionV1 {
            version: 1,
            mutations: [
                change(IdentityChangeV1::Account {
                    expected_revision: 0,
                    account: HumanAccountV1 {
                        version: 1,
                        principal: principal.clone(),
                        display_name: "Proxy budget regression reader".into(),
                        revision: 1,
                        active: true,
                    },
                }),
                change(IdentityChangeV1::Credential {
                    expected_revision: 0,
                    binding: LocalCredentialV1 {
                        version: 1,
                        credential_id: Uuid::new_v4().to_string(),
                        principal: principal.clone(),
                        sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                        revision: 1,
                        active: true,
                        expires_at: None,
                    },
                }),
                change(IdentityChangeV1::Membership {
                    expected_revision: 0,
                    membership: MembershipV1 {
                        version: 1,
                        membership_id: Uuid::new_v4().to_string(),
                        principal,
                        tenant_id: store.tenant_id().into(),
                        organization_scope: "proxy-fixture-organization".into(),
                        revision: 1,
                        active: true,
                        capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                    },
                }),
            ],
        })
        .unwrap();
}

// Replacing the durable gateway refusal with a temporary proxy error must fail
// this test. Provisioning, migration and exhaustion all use production APIs.
#[tokio::test]
async fn dashboard_preserves_real_cumulative_read_budget_refusal() {
    let files = support::private_tempdir();
    let path = files.path().join("tenant.sqlite");
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 64).unwrap();
    store
        .migrate_control_v3(&path.with_extension("before-v3"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("before-v4"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("before-v5"))
        .unwrap();
    let token = generate_local_credential().unwrap();
    provision(&store, &token);
    let store = Arc::new(store);
    let gateway = gateway_router(
        GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: "tenant-a".into(),
                store: Arc::clone(&store),
                runner_token: "proxy-fixture-runner".repeat(3),
                viewer_token: "proxy-fixture-viewer".repeat(3),
            }],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let (_server, upstream) = serve(gateway).await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let identity_url = format!("{upstream}/api/v1/human/identity");
    let baseline = client
        .get(&identity_url)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(baseline.status(), StatusCode::OK, "real identity baseline");
    let identity: Value = baseline.json().await.unwrap();
    let context = identity["contexts"][0]["id"].as_str().unwrap();
    let dashboard = proxy(&upstream, files.path());
    let baseline = proxy_get(&dashboard, &token, context).await;
    assert_eq!(baseline.0, StatusCode::OK, "real proxy/context baseline");
    assert_eq!(baseline.2["context_id"], context);

    let mut refusal = None;
    for _ in 0..16 {
        let response = client
            .get(&identity_url)
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        if response.status() != StatusCode::OK {
            refusal = Some(response);
            break;
        }
    }
    let refusal = refusal.expect("bounded reads must exhaust the small principal budget");
    assert_eq!(refusal.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(refusal.headers().get(header::RETRY_AFTER).is_none());
    let refusal: Value = refusal.json().await.unwrap();
    assert_eq!(refusal["error"]["code"], "audit_read_budget_exhausted");
    assert_eq!(refusal["error"]["retry_action"], "contact_operator");
    let head = store.head().unwrap();
    assert!(
        head < 64,
        "protected budget refusal precedes physical exhaustion"
    );

    let (status, headers, body) = proxy_get(&dashboard, &token, context).await;
    println!(
        "real_gateway=429 audit_read_budget_exhausted/contact_operator proxy={status} proxy_code={} proxy_action={} audit_head={head}",
        body["error"]["code"], body["error"]["retry_action"]
    );
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "the proxy must preserve the durable stop"
    );
    assert_eq!(body["error"]["code"], "audit_read_budget_exhausted");
    assert_eq!(body["error"]["retry_action"], "contact_operator");
    assert!(headers.get(header::RETRY_AFTER).is_none());
    assert!(body.get("contexts").is_none());
    assert_eq!(
        store.head().unwrap(),
        head,
        "a refused proxy read creates no receipt"
    );
}

fn assert_local_failure(headers: &HeaderMap, body: &Value, code: &str, action: &str) {
    assert_eq!(body["error"]["code"], code);
    assert_eq!(body["error"]["retry_action"], action);
    assert_eq!(body.as_object().unwrap().len(), 1);
    assert_eq!(body["error"].as_object().unwrap().len(), 3);
    assert!(!body.to_string().contains("PRIVATE_ECHO"));
    assert!(headers.get(header::RETRY_AFTER).is_none());
    assert!(headers.get(header::SET_COOKIE).is_none());
    assert!(headers.get("x-private-echo").is_none());
}

// Synthetic hostile/error fixtures are necessary here: production Store does
// not emit invalid JSON, secret echoes, mismatched tuples or oversized errors.
#[tokio::test]
async fn dashboard_preflight_accepts_only_closed_budget_tuple_without_echoes() {
    let files = support::private_tempdir();
    let known = json!({"error":{"code":"audit_read_budget_exhausted","message":"PRIVATE_ECHO","retry_action":"contact_operator"}});
    let mut unknown_field = known.clone();
    unknown_field["error"]["private"] = json!("PRIVATE_ECHO");
    let cases = [
        ("known tuple with untrusted message", StatusCode::TOO_MANY_REQUESTS, known.to_string(), true),
        ("status mismatch", StatusCode::SERVICE_UNAVAILABLE, known.to_string(), false),
        ("invalid JSON", StatusCode::TOO_MANY_REQUESTS, "PRIVATE_ECHO".into(), false),
        ("unknown field", StatusCode::TOO_MANY_REQUESTS, unknown_field.to_string(), false),
        ("unknown code", StatusCode::TOO_MANY_REQUESTS, json!({"error":{"code":"PRIVATE_ECHO","message":"PRIVATE_ECHO","retry_action":"contact_operator"}}).to_string(), false),
        ("action mismatch", StatusCode::TOO_MANY_REQUESTS, json!({"error":{"code":"audit_read_budget_exhausted","message":"PRIVATE_ECHO","retry_action":"retry"}}).to_string(), false),
        ("missing action", StatusCode::TOO_MANY_REQUESTS, json!({"error":{"code":"audit_read_budget_exhausted","message":"PRIVATE_ECHO"}}).to_string(), false),
        ("duplicate code", StatusCode::TOO_MANY_REQUESTS, r#"{"error":{"code":"PRIVATE_ECHO","code":"audit_read_budget_exhausted","message":"PRIVATE_ECHO","retry_action":"contact_operator"}}"#.into(), false),
        ("oversized valid JSON", StatusCode::TOO_MANY_REQUESTS, json!({"error":{"code":"audit_read_budget_exhausted","message":"PRIVATE_ECHO".repeat(1024),"retry_action":"contact_operator"}}).to_string(), false),
        ("401 keeps existing authentication handling", StatusCode::UNAUTHORIZED, known.to_string(), false),
    ];
    for (name, status, payload, recognized) in cases {
        let forwarded = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&forwarded);
        let upstream = Router::new()
            .route(
                "/api/v1/human/identity",
                get(move || {
                    let payload = payload.clone();
                    async move {
                        (
                            status,
                            [
                                ("content-type", "application/json"),
                                ("retry-after", "1"),
                                ("set-cookie", "PRIVATE_ECHO"),
                                ("x-private-echo", "PRIVATE_ECHO"),
                            ],
                            payload,
                        )
                            .into_response()
                    }
                }),
            )
            .fallback(get(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                async { StatusCode::INTERNAL_SERVER_ERROR }
            }));
        let (_server, upstream) = serve(upstream).await;
        let (result, headers, body) = proxy_get(
            &proxy(&upstream, files.path()),
            "PRIVATE_ECHO",
            "PRIVATE_ECHO",
        )
        .await;
        let (wanted_status, code, action) = if recognized {
            (
                StatusCode::TOO_MANY_REQUESTS,
                "audit_read_budget_exhausted",
                "contact_operator",
            )
        } else if status == StatusCode::UNAUTHORIZED {
            (
                StatusCode::UNAUTHORIZED,
                "credential_invalid",
                "authenticate",
            )
        } else {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "identity_unavailable",
                "retry",
            )
        };
        assert_eq!(result, wanted_status, "fixture: {name}");
        assert_local_failure(&headers, &body, code, action);
        assert_eq!(
            forwarded.load(Ordering::SeqCst),
            0,
            "failed preflight must block forwarding: {name}"
        );
    }
}

// A stalled body is a synthetic transport failure. The real request's existing
// three-second total deadline must also bound reading the error response.
#[tokio::test]
async fn dashboard_preflight_error_body_keeps_existing_request_deadline() {
    let files = support::private_tempdir();
    let upstream = Router::new().route(
        "/api/v1/human/identity",
        get(|| async {
            (
                StatusCode::TOO_MANY_REQUESTS,
                Body::from_stream(futures_util::stream::pending::<
                    Result<Vec<u8>, std::io::Error>,
                >()),
            )
        }),
    );
    let (_server, upstream) = serve(upstream).await;
    let dashboard = proxy(&upstream, files.path());
    let (status, headers, body) = tokio::time::timeout(
        Duration::from_secs(8),
        proxy_get(&dashboard, "PRIVATE_ECHO", "PRIVATE_ECHO"),
    )
    .await
    .expect("preflight must not wait indefinitely for an error body");
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_local_failure(&headers, &body, "identity_unavailable", "retry");
}
