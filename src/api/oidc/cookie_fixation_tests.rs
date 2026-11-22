//! Unit/registry regressions for OIDC browser correlation cookie rotation and
//! per-transaction `correlation_token` delivery verification (V1 / RC-P4-R02).
use super::*;
use crate::api::oidc::gateway::admission_tests::controlled;
use crate::api::read_execution_tests::{identity_change, local_identity_state};
use crate::audit::authentication::{
    ApprovedAuthenticationClientV1, AuthenticationAuditPolicyV1, AuthenticationCapacityPlanV1,
    AuthenticationCapacityV1, AuthenticationTenantCapacityV1,
};
use crate::identity::*;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:18100";

struct FixationFixture {
    _files: tempfile::TempDir,
    issuer: controlled::ControlledIssuer,
    state: GatewayState,
    router: Router,
    audit_path: std::path::PathBuf,
}

async fn fixation_fixture_with_pkce_ttl(pkce_ttl_seconds: u64) -> FixationFixture {
    let (files, state, _) = local_identity_state();
    let issuer = controlled::ControlledIssuer::new().await;
    let principal = PrincipalRefV1::HumanOidc {
        issuer: issuer.issuer.clone(),
        subject: "fixation-victim".into(),
    };
    state.tenants[0]
        .store
        .sqlite()
        .unwrap()
        .mutate_identity(&identity_change(IdentityChangeV1::Account {
            expected_revision: 0,
            account: HumanAccountV1 {
                version: 1,
                principal: principal.clone(),
                display_name: "Fixation Victim".into(),
                revision: 1,
                active: true,
            },
        }))
        .unwrap();
    state.tenants[0]
        .store
        .sqlite()
        .unwrap()
        .mutate_identity(&identity_change(IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: MembershipV1 {
                version: 1,
                membership_id: Uuid::new_v4().to_string(),
                principal,
                tenant_id: state.tenants[0].tenant_id.clone(),
                organization_scope: "organization-a".into(),
                revision: 1,
                active: true,
                capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
            },
        }))
        .unwrap();
    let audit_path = files.path().join("authentication.sqlite");
    let plan = AuthenticationCapacityPlanV1 {
        version: 1,
        unscoped: AuthenticationCapacityV1 {
            max_records: 32,
            max_serialized_bytes: 256 * 1024,
        },
        tenants: [("tenant-a", 483), ("tenant-b", 483)]
            .map(|(tenant_id, max_records)| AuthenticationTenantCapacityV1 {
                tenant_id: tenant_id.into(),
                capacity: AuthenticationCapacityV1 {
                    max_records,
                    max_serialized_bytes: 1536 * 1024,
                },
            })
            .to_vec(),
    };
    let audit = Arc::new(
        AuthenticationAudit::init_new_v4(
            &audit_path,
            Uuid::new_v4(),
            AuthenticationAuditPolicyV1 {
                max_records: 1000,
                max_serialized_bytes: 4 * 1024 * 1024,
                approved_clients: ["web-public", "cli-public"]
                    .map(|client_id| ApprovedAuthenticationClientV1 {
                        issuer: issuer.issuer.clone(),
                        client_id: client_id.into(),
                    })
                    .to_vec(),
            },
            plan,
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let mut provider = issuer.config();
    provider.limits.pkce_ttl_seconds = pkce_ttl_seconds;
    let oidc = OidcGateway::new(
        OidcGatewayConfigV1 {
            version: 1,
            provider,
            provider_label: "Controlled issuer".into(),
            web_origin: ORIGIN.into(),
            mapping: OidcClaimsMappingV1 {
                revision: "fixation-1".into(),
                default_capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                rules: vec![],
            },
        },
        audit,
        None,
    )
    .unwrap();
    let state = state.with_oidc(oidc).unwrap();
    let router = gateway_router(state.clone());
    FixationFixture {
        _files: files,
        issuer,
        state,
        router,
        audit_path,
    }
}

async fn fixation_fixture() -> FixationFixture {
    fixation_fixture_with_pkce_ttl(300).await
}

fn cookie_value(header_value: &HeaderValue) -> &str {
    header_value
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("aiguard_oidc=")
        .unwrap()
}

#[tokio::test]
async fn status_never_emits_cookie_and_validates_cookie_syntax() {
    let f = fixation_fixture().await;
    let empty = f
        .router
        .clone()
        .oneshot(
            Request::get("/auth/oidc/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(empty.status(), StatusCode::OK);
    assert!(empty.headers().get(header::SET_COOKIE).is_none());

    let planted = format!("aiguard_oidc={}abcd", "c0ffee".repeat(10));
    let with_planted = f
        .router
        .clone()
        .oneshot(
            Request::get("/auth/oidc/status")
                .header(header::COOKIE, &planted)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(with_planted.status(), StatusCode::OK);
    assert!(with_planted.headers().get(header::SET_COOKIE).is_none());

    for bad_cookie in [
        "aiguard_oidc=not-hex".to_owned(),
        format!(
            "aiguard_oidc={}; aiguard_oidc={}",
            "a".repeat(64),
            "b".repeat(64)
        ),
        format!("aiguard_oidc={}", "a".repeat(4097)),
    ] {
        let bad = f
            .router
            .clone()
            .oneshot(
                Request::get("/auth/oidc/status")
                    .header(header::COOKIE, bad_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
        assert!(bad.headers().get(header::SET_COOKIE).is_none());
    }
}

#[tokio::test]
async fn start_rotates_planted_cookie_and_tokens_mismatch_does_not_evict_delivery() {
    let f = fixation_fixture().await;
    let oidc = f.state.oidc.as_ref().unwrap();
    let planted_raw = format!("{}abcd", "c0ffee".repeat(10));
    let planted_header = format!("aiguard_oidc={planted_raw}");

    let start_res = f
        .router
        .clone()
        .oneshot(
            Request::post("/auth/oidc/start")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ORIGIN, ORIGIN)
                .header("sec-fetch-site", "same-origin")
                .header("sec-fetch-mode", "cors")
                .header(header::COOKIE, &planted_header)
                .body(Body::from(json!({"version":1}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(start_res.status(), StatusCode::OK);
    let set_cookie = start_res.headers()[header::SET_COOKIE].clone();
    assert!(valid_response_cookie(&set_cookie, false));
    let fresh_raw = cookie_value(&set_cookie).to_owned();
    assert_ne!(fresh_raw, planted_raw);

    let start_body: Value =
        serde_json::from_slice(&to_bytes(start_res.into_body(), 65_536).await.unwrap()).unwrap();
    let correlation_token = start_body["correlation_token"].as_str().unwrap().to_owned();
    assert!(hex_digest(&correlation_token));
    assert_ne!(correlation_token, fresh_raw);
    assert_ne!(correlation_token, planted_raw);

    let fresh_key: [u8; 32] = Sha256::digest(fresh_raw.as_bytes()).into();
    let planted_key: [u8; 32] = Sha256::digest(planted_raw.as_bytes()).into();
    let expected_token_digest: [u8; 32] = Sha256::digest(correlation_token.as_bytes()).into();
    {
        let registry = oidc.registry.lock().unwrap();
        assert!(!registry.transactions.contains_key(&planted_key));
        let tx = &registry.transactions[&fresh_key];
        assert_eq!(tx.expected_correlation_token, expected_token_digest);
    }

    let url = reqwest::Url::parse(start_body["authorization_url"].as_str().unwrap()).unwrap();
    let params = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    let victim_access =
        f.issuer
            .login_response(&params["nonce"], "fixation-victim", "fixation-login");

    // Old planted cookie cannot advance callback.
    let stale_cb = f
        .router
        .clone()
        .oneshot(
            Request::get(format!(
                "/auth/oidc/callback?code=controlled-code&state={}",
                params["state"]
            ))
            .header(header::COOKIE, &planted_header)
            .header("sec-fetch-site", "cross-site")
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale_cb.status(), StatusCode::CONFLICT);

    let fresh_cookie_header = format!("aiguard_oidc={fresh_raw}");
    let cb_res = f
        .router
        .clone()
        .oneshot(
            Request::get(format!(
                "/auth/oidc/callback?code=controlled-code&state={}",
                params["state"]
            ))
            .header(header::COOKIE, &fresh_cookie_header)
            .header("sec-fetch-site", "cross-site")
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cb_res.status(), StatusCode::SEE_OTHER);
    assert_eq!(cb_res.headers()[header::LOCATION], "/auth/oidc/complete");

    {
        let registry = oidc.registry.lock().unwrap();
        assert!(!registry.deliveries.contains_key(&planted_key));
        let delivery = &registry.deliveries[&fresh_key];
        assert_eq!(delivery.expected_correlation_token, expected_token_digest);
    }

    // Attacker holding fresh_raw but guessing wrong correlation_token gets 401
    // WITHOUT removing the pending delivery.
    let wrong_token = "f".repeat(64);
    assert_ne!(wrong_token, correlation_token);
    let attacker_guess = f
        .router
        .clone()
        .oneshot(
            Request::post("/auth/oidc/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ORIGIN, ORIGIN)
                .header(header::COOKIE, &fresh_cookie_header)
                .body(Body::from(
                    json!({"version":1,"correlation_token":wrong_token}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(attacker_guess.status(), StatusCode::UNAUTHORIZED);
    assert!(attacker_guess.headers().get(header::SET_COOKIE).is_none());
    {
        let registry = oidc.registry.lock().unwrap();
        assert!(
            registry.deliveries.contains_key(&fresh_key),
            "wrong correlation_token must not evict the legitimate pending delivery"
        );
    }

    // Legitimate browser redeems one-shot with rotated cookie + correlation_token.
    let victim_res = f
        .router
        .clone()
        .oneshot(
            Request::post("/auth/oidc/tokens")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ORIGIN, ORIGIN)
                .header("sec-fetch-site", "same-origin")
                .header("sec-fetch-mode", "cors")
                .header(header::COOKIE, &fresh_cookie_header)
                .body(Body::from(
                    json!({"version":1,"correlation_token":correlation_token}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(victim_res.status(), StatusCode::OK);
    let clear_cookie = &victim_res.headers()[header::SET_COOKIE];
    assert!(valid_response_cookie(clear_cookie, false));
    assert_eq!(cookie_value(clear_cookie), "");
    let delivery: Value =
        serde_json::from_slice(&to_bytes(victim_res.into_body(), 65_536).await.unwrap()).unwrap();
    assert_eq!(delivery["access_token"], victim_access);
    assert!(oidc.registry.lock().unwrap().deliveries.is_empty());

    // Audit ledger must never store fresh_raw, planted_raw, or correlation_token.
    let db = rusqlite::Connection::open(&f.audit_path).unwrap();
    let mut stmt = db
        .prepare("SELECT payload FROM authentication_entries")
        .unwrap();
    let payloads: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(!payloads.is_empty());
    for payload in payloads {
        assert!(!payload.contains(&fresh_raw));
        assert!(!payload.contains(&planted_raw));
        assert!(!payload.contains(&correlation_token));
    }
}

#[tokio::test]
async fn reap_evicts_expired_browser_states_carrying_correlation_token_digests() {
    let f = fixation_fixture_with_pkce_ttl(1).await;
    let oidc = f.state.oidc.as_ref().unwrap();
    let tx_key: [u8; 32] = [1; 32];
    let ex_key: [u8; 32] = [2; 32];
    let del_key: [u8; 32] = [3; 32];
    let expired_tx = Instant::now()
        .checked_sub(Duration::from_secs(
            oidc.config.provider.limits.pkce_ttl_seconds + 1,
        ))
        .unwrap();
    let expired_exchange = Instant::now().checked_sub(Duration::from_secs(36)).unwrap();
    let expired_delivery = Instant::now().checked_sub(Duration::from_secs(31)).unwrap();
    {
        let mut registry = oidc.registry.lock().unwrap();
        registry.transactions.insert(
            tx_key,
            BrowserTransaction {
                pending: None,
                expected_state: [0; 32],
                expected_correlation_token: [9; 32],
                transaction_id: Uuid::new_v4(),
                operation_id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
                created: expired_tx,
            },
        );
        registry.exchanging.insert(
            ex_key,
            ExchangeMarker {
                transaction_id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
                expected_correlation_token: [8; 32],
                created: expired_exchange,
            },
        );
        registry.deliveries.insert(
            del_key,
            PendingDelivery {
                result: Delivery::Error("login_cancelled"),
                expected_correlation_token: [7; 32],
                created: expired_delivery,
            },
        );
        gateway::reap(oidc, &mut registry).unwrap();
        assert!(!registry.transactions.contains_key(&tx_key));
        assert!(!registry.exchanging.contains_key(&ex_key));
        assert!(!registry.deliveries.contains_key(&del_key));
    }
}
