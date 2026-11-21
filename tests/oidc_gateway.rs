//! OIDC gateway and dashboard HTTP boundaries. No IdP or runtime.
mod support;
use agentsandbox::{
    api::{GatewayState, dashboard_router_with_oidc, gateway_router},
    audit::Store,
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

fn fixture() -> (tempfile::TempDir, axum::Router) {
    let files = support::private_tempdir();
    let store = Arc::new(
        Store::open_for_tenant(
            &files.path().join("tenant.sqlite"),
            "tenant-a",
            Redactor::new(vec![]),
            1000,
        )
        .unwrap(),
    );
    let state = GatewayState::new(
        store,
        "r".repeat(48),
        "v".repeat(48),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    );
    (files, gateway_router(state))
}

fn dashboard_fixture(oidc_web_origin: Option<&str>) -> (tempfile::TempDir, axum::Router) {
    let files = support::private_tempdir();
    std::fs::write(
        files.path().join("index.html"),
        "<!doctype html><title>OIDC redirect completion</title>",
    )
    .unwrap();
    let router = dashboard_router_with_oidc(
        "http://127.0.0.1:8787",
        None,
        files.path().to_path_buf(),
        oidc_web_origin.map(str::to_owned),
    )
    .unwrap();
    (files, router)
}

#[tokio::test]
async fn dashboard_oidc_completion_accepts_cross_site_redirect_navigation() {
    let (_files, router) = dashboard_fixture(Some("http://127.0.0.1:4000"));
    // The IdP redirect chain keeps cross-site metadata on the final public page.
    let response = router
        .oneshot(
            Request::get("/auth/oidc/complete")
                .header("sec-fetch-site", "cross-site")
                .header("sec-fetch-mode", "navigate")
                .header("sec-fetch-dest", "document")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the public completion page must remain reachable after the IdP redirect"
    );
    assert_eq!(response.headers()["content-type"], "text/html");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-frame-options"], "DENY");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    assert!(
        response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
    assert!(response.headers().get("set-cookie").is_none());
    assert_eq!(
        to_bytes(response.into_body(), 4096).await.unwrap().as_ref(),
        b"<!doctype html><title>OIDC redirect completion</title>"
    );
}

#[tokio::test]
async fn dashboard_oidc_completion_exception_does_not_admit_cross_site_actions() {
    let (_files, router) = dashboard_fixture(Some("http://127.0.0.1:4000"));
    for (method, path, mode, destination) in [
        ("POST", "/auth/oidc/complete", "navigate", "document"),
        ("POST", "/auth/oidc/tokens", "cors", "empty"),
        ("GET", "/api/v1/human/identity", "navigate", "document"),
        ("GET", "/auth/oidc/complete", "cors", "empty"),
        ("GET", "/auth/oidc/complete", "no-cors", "image"),
        (
            "GET",
            "/auth/oidc/complete?unexpected=value",
            "navigate",
            "document",
        ),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("sec-fetch-site", "cross-site")
                    .header("sec-fetch-mode", mode)
                    .header("sec-fetch-dest", destination)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {path}");
    }
}

#[tokio::test]
async fn dashboard_oidc_completion_keeps_cross_site_guard_when_sso_is_disabled() {
    let (_files, router) = dashboard_fixture(None);
    let response = router
        .oneshot(
            Request::get("/auth/oidc/complete")
                .header("sec-fetch-site", "cross-site")
                .header("sec-fetch-mode", "navigate")
                .header("sec-fetch-dest", "document")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn oidc_is_explicitly_disabled_without_provider_configuration() {
    let (_files, router) = fixture();
    let response = router
        .oneshot(
            Request::get("/auth/oidc/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the public status must distinguish disabled OIDC from a missing route"
    );
    assert!(response.headers().get("set-cookie").is_none());
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(value, json!({"version":1,"enabled":false}));
}

#[tokio::test]
async fn disabled_oidc_never_accepts_login_or_cli_admission() {
    let (_files, router) = fixture();
    for route in ["start", "tokens", "refresh", "admit", "rotate"] {
        let response = router
            .clone()
            .oneshot(
                Request::post(format!("/auth/oidc/{route}"))
                    .header("content-type", "application/json")
                    .body(Body::from("{\"version\":1}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{route} must fail closed while OIDC is disabled"
        );
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(value["error"]["code"], "oidc_not_configured");
    }
}

#[tokio::test]
async fn cross_site_exception_is_limited_to_get_callback() {
    let (_files, router) = fixture();
    for (method, path) in [
        ("POST", "/auth/oidc/start"),
        ("GET", "/api/v1/human/identity"),
        ("POST", "/auth/oidc/callback"),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("sec-fetch-site", "cross-site")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {path}");
    }
    let callback = router
        .oneshot(
            Request::get("/auth/oidc/callback?state=untrusted&code=untrusted")
                .header("sec-fetch-site", "cross-site")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        callback.status(),
        StatusCode::FORBIDDEN,
        "disabled OIDC preserves the existing cross-site guard"
    );
}
