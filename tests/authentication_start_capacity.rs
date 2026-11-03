//! Inverted regressions from audit_verify_auth_ledger_exhaustion_start_path.
//! Real gateway/SQLite with an owned loopback issuer; no real IdP, worker or
//! provider inference. Unverified login activity must not consume the durable
//! authentication ledger that gates verified human sessions.
// Other OIDC targets use the shared issuer's additional fault controls.
#[allow(dead_code)]
#[path = "support/oidc_fixture.rs"]
mod controlled;
mod support;

use agentsandbox::{
    api::{GatewayState, OidcClaimsMappingV1, OidcGateway, OidcGatewayConfigV1, gateway_router},
    audit::{
        Store,
        authentication::{
            ApprovedAuthenticationClientV1, AuthenticationAudit, AuthenticationAuditPolicyV1,
        },
    },
    identity::{
        HumanAccountV1, HumanCapabilityV1, IdentityChangeV1, IdentityMutationV1,
        LegacyControlRoleV1, PrincipalRefV1,
    },
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use controlled::ControlledIssuer;
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:18100";

async fn build(
    max_records: u64,
) -> (
    tempfile::TempDir,
    ControlledIssuer,
    Arc<AuthenticationAudit>,
    Router,
    Arc<Store>,
) {
    let files = support::private_tempdir();
    let issuer = ControlledIssuer::new().await;
    let path = files.path().join("tenant.sqlite");
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap();
    store
        .migrate_control_v3(&path.with_extension("before-v3"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("before-v4"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("before-v5"))
        .unwrap();
    let store = Arc::new(store);
    let audit = Arc::new(
        AuthenticationAudit::init_new(
            &files.path().join("authentication.sqlite"),
            Uuid::new_v4(),
            AuthenticationAuditPolicyV1 {
                max_records,
                max_serialized_bytes: 8 * 1024 * 1024,
                approved_clients: ["web-public", "cli-public"]
                    .map(|client_id| ApprovedAuthenticationClientV1 {
                        issuer: issuer.issuer.clone(),
                        client_id: client_id.into(),
                    })
                    .to_vec(),
            },
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let config = OidcGatewayConfigV1 {
        version: 1,
        provider: issuer.config(),
        provider_label: "Controlled issuer".into(),
        web_origin: ORIGIN.into(),
        mapping: OidcClaimsMappingV1 {
            revision: "mapping-1".into(),
            default_capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
            rules: vec![],
        },
    };
    let router = gateway_router(
        GatewayState::new(
            store.clone(),
            "r".repeat(48),
            "v".repeat(48),
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .with_oidc(OidcGateway::new(config, audit.clone(), None).unwrap())
        .unwrap(),
    );
    (files, issuer, audit, router, store)
}

async fn send(router: &Router, request: Request<Body>) -> Response {
    router.clone().oneshot(request).await.unwrap()
}

async fn json_response(response: Response) -> (StatusCode, Value) {
    let status = response.status();
    let body = to_bytes(response.into_body(), 35_000).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

async fn seed_cookie(router: &Router) -> String {
    let response = send(
        router,
        Request::get("/auth/oidc/status")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

fn web_post(path: &str, cookie: &str) -> Request<Body> {
    Request::post(path)
        .header("content-type", "application/json")
        .header("cookie", cookie)
        .header("origin", ORIGIN)
        .header("sec-fetch-site", "same-origin")
        .header("sec-fetch-mode", "cors")
        .body(Body::from(json!({"version": 1}).to_string()))
        .unwrap()
}

fn native_request(action: Value) -> Request<Body> {
    Request::post("/auth/oidc/native-intent")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"version": 1, "action": action}).to_string(),
        ))
        .unwrap()
}

fn assert_no_token_exchange(issuer: &ControlledIssuer) {
    assert!(
        issuer
            .requests()
            .iter()
            .all(|(path, _)| !path.ends_with("/token")),
        "the scenario must not establish an identity proof with a token exchange"
    );
}

#[tokio::test]
async fn unverified_web_start_does_not_consume_durable_authentication_audit() {
    let (_files, issuer, audit, router, _store) = build(1_000).await;
    let before = audit.verify().unwrap().records;
    let cookie = seed_cookie(&router).await;
    let response = send(&router, web_post("/auth/oidc/start", &cookie)).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "HARNESS INCONCLUSIVE: controlled-issuer preparation did not complete"
    );
    assert_no_token_exchange(&issuer);
    assert_eq!(
        audit.verify().unwrap().records,
        before,
        "a status-seeded cookie is not an identity proof or a durable audit reservation"
    );
}

#[tokio::test]
async fn anonymous_web_starts_cannot_fill_the_durable_authentication_audit() {
    let (_files, issuer, audit, router, _store) = build(12).await;
    let before = audit.verify().unwrap().records;
    let mut accepted = 0;
    for _ in 0..24 {
        let cookie = seed_cookie(&router).await;
        let (status, _) =
            json_response(send(&router, web_post("/auth/oidc/start", &cookie)).await).await;
        let records = audit.verify().unwrap().records;
        if status == StatusCode::SERVICE_UNAVAILABLE && records != 12 {
            panic!(
                "HARNESS INCONCLUSIVE: controlled issuer/start unavailable before ledger capacity; records={records}"
            );
        }
        assert!(
            matches!(status, StatusCode::OK | StatusCode::TOO_MANY_REQUESTS),
            "anonymous starts must be bounded in memory without exhausting the durable ledger: status={status}, records={records}"
        );
        accepted += usize::from(status == StatusCode::OK);
    }
    assert!(
        accepted > 0,
        "HARNESS INCONCLUSIVE: no anonymous start reached the controlled issuer"
    );
    assert_no_token_exchange(&issuer);
    assert_eq!(audit.verify().unwrap().records, before);
}

#[tokio::test]
async fn unverified_native_intent_and_cancellation_do_not_consume_durable_audit() {
    let (_files, issuer, audit, router, _store) = build(12).await;
    let before = audit.verify().unwrap().records;
    let (status, intent) =
        json_response(send(&router, native_request(json!({"kind": "login"}))).await).await;
    assert_eq!(status, StatusCode::OK);
    let intent_id = intent["intent_id"].as_str().unwrap();
    Uuid::parse_str(intent_id).unwrap();
    let finish = json!({"kind": "finish", "intent_id": intent_id, "outcome": "cancelled"});
    assert_eq!(
        send(&router, native_request(finish.clone())).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        send(&router, native_request(finish)).await.status(),
        StatusCode::CONFLICT,
        "an anonymous cancellation still consumes its in-memory intent only once"
    );
    assert_no_token_exchange(&issuer);
    assert_eq!(
        audit.verify().unwrap().records,
        before,
        "a declared cancellation without verified proof must not append durable authentication records"
    );
}

#[tokio::test]
async fn unverified_native_proof_rejection_does_not_consume_durable_audit() {
    let (_files, issuer, audit, router, _store) = build(12).await;
    let before = audit.verify().unwrap().records;
    let (status, intent) =
        json_response(send(&router, native_request(json!({"kind": "login"}))).await).await;
    assert_eq!(status, StatusCode::OK);
    let request = Request::post("/auth/oidc/admit")
        .header("content-type", "application/json")
        .header(
            "authorization",
            "Bearer invalid-fixture-jwt-not-issued-by-controlled-issuer",
        )
        .body(Body::from(
            json!({"version": 1, "intent_id": intent["intent_id"]}).to_string(),
        ))
        .unwrap();
    let (status, _) = json_response(send(&router, request).await).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_no_token_exchange(&issuer);
    assert_eq!(
        audit.verify().unwrap().records,
        before,
        "a rejected bearer without verified identity must not append durable authentication records"
    );
}

#[tokio::test]
async fn unverified_web_callback_rejection_does_not_append_to_the_ledger() {
    let (_files, issuer, audit, router, _store) = build(12).await;
    let before = audit.verify().unwrap().records;
    let cookie = seed_cookie(&router).await;
    let (status, start) =
        json_response(send(&router, web_post("/auth/oidc/start", &cookie)).await).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "HARNESS INCONCLUSIVE: controlled-issuer preparation did not complete"
    );
    let authorization = reqwest::Url::parse(start["authorization_url"].as_str().unwrap()).unwrap();
    let state = authorization
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    let mut callback = reqwest::Url::parse("http://127.0.0.1/auth/oidc/callback").unwrap();
    callback
        .query_pairs_mut()
        .append_pair("state", &state)
        .append_pair("error", "access_denied");
    let request = Request::get(format!("{}?{}", callback.path(), callback.query().unwrap()))
        .header("cookie", &cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&router, request).await.status(), StatusCode::SEE_OTHER);
    let (status, failure) =
        json_response(send(&router, web_post("/auth/oidc/tokens", &cookie)).await).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(failure["error"]["code"], "login_cancelled");
    assert_no_token_exchange(&issuer);
    assert_eq!(
        audit.verify().unwrap().records,
        before,
        "an unverified provider-error query must not spend durable authentication audit capacity"
    );
}

#[tokio::test]
async fn verified_unapproved_identity_keeps_its_durable_denial_audit() {
    let (_files, issuer, audit, router, _store) = build(12).await;
    let before = audit.verify().unwrap().records;
    let token = issuer.token(
        "cli-public",
        "unapproved-subject",
        None,
        &[],
        "verified-denial",
    );
    let (status, intent) =
        json_response(send(&router, native_request(json!({"kind": "login"}))).await).await;
    assert_eq!(status, StatusCode::OK);
    let request = Request::post("/auth/oidc/admit")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(
            json!({"version": 1, "intent_id": intent["intent_id"]}).to_string(),
        ))
        .unwrap();
    let (status, failure) = json_response(send(&router, request).await).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(failure["error"]["code"], "permission_denied");
    assert_eq!(
        audit.verify().unwrap().records,
        before + 2,
        "verified proof still requires a durable intention and denial before a session can be admitted"
    );
}

#[tokio::test]
async fn verified_proof_cannot_bypass_missing_durable_audit_capacity() {
    // Genesis already occupies the only durable record. Creating a volatile
    // nonce remains possible, but a verified admission must stop before even
    // evaluating the otherwise unapproved membership.
    let (_files, issuer, audit, router, store) = build(1).await;
    // An active account makes human_profile append a tenant read event even
    // without a membership. This detects audit refusal ordered too late.
    store
        .mutate_identity(&IdentityMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            actor: PrincipalRefV1::LegacyControl {
                credential_id: "fixture-operator".into(),
                role: LegacyControlRoleV1::Runner,
            },
            change: IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: PrincipalRefV1::HumanOidc {
                        issuer: issuer.issuer.clone(),
                        subject: "unapproved-subject".into(),
                    },
                    display_name: "OIDC account without membership".into(),
                    revision: 1,
                    active: true,
                },
            },
        })
        .unwrap();
    let tenant_before = store.integrity().unwrap();
    assert!(tenant_before.valid);
    assert!(tenant_before.event_count > 0);
    let before = audit.verify().unwrap().records;
    let token = issuer.token(
        "cli-public",
        "unapproved-subject",
        None,
        &[],
        "verified-full-ledger",
    );
    let (status, intent) =
        json_response(send(&router, native_request(json!({"kind": "login"}))).await).await;
    assert_eq!(status, StatusCode::OK);
    let request = Request::post("/auth/oidc/admit")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(
            json!({"version": 1, "intent_id": intent["intent_id"]}).to_string(),
        ))
        .unwrap();
    let (status, failure) = json_response(send(&router, request).await).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(failure["error"]["code"], "audit_unavailable");
    assert_eq!(audit.verify().unwrap().records, before);
    let tenant_after = store.integrity().unwrap();
    assert!(tenant_after.valid);
    assert_eq!(
        tenant_after.event_count, tenant_before.event_count,
        "durable authentication audit must refuse before the active account's tenant profile read is audited"
    );
    assert!(
        issuer
            .requests()
            .iter()
            .any(|(path, _)| path.ends_with("/keys")),
        "the valid signed proof must reach provider verification before admission audit is refused"
    );
}
