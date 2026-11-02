//! Gateway HTTP boundaries without a configured provider. No IdP or runtime.
mod support;
use agentsandbox::{
    api::{GatewayState, gateway_router},
    audit::Store,
    policy::Policy,
    redaction::Redactor,
};
use axum::{body::{Body, to_bytes}, http::{Request, StatusCode}};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

fn fixture() -> (tempfile::TempDir, axum::Router) {
    let files = support::private_tempdir();
    let store = Arc::new(Store::open_for_tenant(
        &files.path().join("tenant.sqlite"), "tenant-a", Redactor::new(vec![]), 1000,
    ).unwrap());
    let state = GatewayState::new(
        store, "r".repeat(48), "v".repeat(48), Policy::default(), None,
        Redactor::new(vec![]),
    );
    (files, gateway_router(state))
}

#[tokio::test]
async fn oidc_is_explicitly_disabled_without_provider_configuration() {
    let (_files, router) = fixture();
    let response = router.oneshot(Request::get("/auth/oidc/status").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "the public status must distinguish disabled OIDC from a missing route");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    let value: Value = serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(value, json!({"version":1,"enabled":false}));
}

#[tokio::test]
async fn disabled_oidc_never_accepts_login_or_cli_admission() {
    let (_files, router) = fixture();
    for route in ["start", "tokens", "refresh", "admit", "rotate"] {
        let response = router.clone().oneshot(Request::post(format!("/auth/oidc/{route}"))
            .header("content-type", "application/json")
            .body(Body::from("{\"version\":1}")).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{route} must fail closed while OIDC is disabled");
        let value: Value = serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(value["error"]["code"], "oidc_not_configured");
    }
}

#[tokio::test]
async fn cross_site_exception_is_limited_to_get_callback() {
    let (_files, router) = fixture();
    for (method, path) in [("POST", "/auth/oidc/start"), ("GET", "/api/v1/human/identity"), ("POST", "/auth/oidc/callback")] {
        let response = router.clone().oneshot(Request::builder().method(method).uri(path)
            .header("sec-fetch-site", "cross-site")
            .body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {path}");
    }
    let callback = router.oneshot(Request::get("/auth/oidc/callback?state=untrusted&code=untrusted")
        .header("sec-fetch-site", "cross-site").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(callback.status(), StatusCode::FORBIDDEN, "disabled OIDC preserves the existing cross-site guard");
}
