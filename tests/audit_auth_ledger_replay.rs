//! Regression for audit run-1, lot M: one verified but unapproved proof must
//! not consume the shared authentication ledger repeatedly. Real router and
//! private SQLite, owned loopback issuer, synthetic identities only.
#[allow(dead_code)]
#[path = "support/oidc_fixture.rs"]
mod controlled;
use agentsandbox as test_product;
mod support;

use agentsandbox::{
    api::{GatewayState, OidcClaimsMappingV1, OidcGateway, OidcGatewayConfigV1, gateway_router},
    audit::{
        Store,
        authentication::{
            ApprovedAuthenticationClientV1, AuthenticationAudit, AuthenticationAuditPolicyV1,
            AuthenticationCapacityPlanV1, AuthenticationCapacityV1, AuthenticationTenantCapacityV1,
        },
    },
    identity::{
        HumanAccountV1, HumanCapabilityV1, IdentityChangeV1, IdentityMutationV1,
        LegacyControlRoleV1, MembershipV1, PrincipalRefV1,
    },
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use controlled::ControlledIssuer;
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};
use tower::ServiceExt;
use uuid::Uuid;

struct Fixture {
    files: tempfile::TempDir,
    issuer: ControlledIssuer,
    audit: Arc<AuthenticationAudit>,
    router: Router,
    store: Arc<Store>,
    installation: Uuid,
    policy: AuthenticationAuditPolicyV1,
    config: OidcGatewayConfigV1,
}

fn member(store: &Store, issuer: &str, subject: &str) {
    let member = PrincipalRefV1::HumanOidc {
        issuer: issuer.into(),
        subject: subject.into(),
    };
    for change in [
        IdentityChangeV1::Account {
            expected_revision: 0,
            account: HumanAccountV1 {
                version: 1,
                principal: member.clone(),
                display_name: "Synthetic approved reader".into(),
                revision: 1,
                active: true,
            },
        },
        IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: MembershipV1 {
                version: 1,
                membership_id: Uuid::new_v4().to_string(),
                principal: member,
                tenant_id: "tenant-a".into(),
                organization_scope: "organization-a".into(),
                revision: 1,
                active: true,
                capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
            },
        },
    ] {
        store
            .mutate_identity(&IdentityMutationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                actor: PrincipalRefV1::LegacyControl {
                    credential_id: "fixture-operator".into(),
                    role: LegacyControlRoleV1::Runner,
                },
                change,
            })
            .unwrap();
    }
}

fn try_router(
    store: Arc<Store>,
    audit: Arc<AuthenticationAudit>,
    config: OidcGatewayConfigV1,
) -> anyhow::Result<Router> {
    Ok(gateway_router(
        GatewayState::new(
            store,
            "r".repeat(48),
            "v".repeat(48),
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .with_oidc(OidcGateway::new(config, audit, None)?)?,
    ))
}

fn router(
    store: Arc<Store>,
    audit: Arc<AuthenticationAudit>,
    config: OidcGatewayConfigV1,
) -> Router {
    try_router(store, audit, config).unwrap()
}

async fn build(max_records: u64) -> Fixture {
    build_limits(max_records, 8 * 1024 * 1024).await
}

async fn build_limits(max_records: u64, max_serialized_bytes: u64) -> Fixture {
    build_with_plan(max_records, max_serialized_bytes, None).await
}

fn capacity(max_records: u64, max_serialized_bytes: u64) -> AuthenticationCapacityV1 {
    AuthenticationCapacityV1 {
        max_records,
        max_serialized_bytes,
    }
}

/// One closed logical plan for the single `tenant-a` gateway. The unscoped
/// partition funds verified-but-unapproved denials; the tenant partition funds
/// approved admissions. They never borrow from each other (V4 accounting).
fn tenant_a_plan(
    unscoped: AuthenticationCapacityV1,
    tenant_a: AuthenticationCapacityV1,
) -> AuthenticationCapacityPlanV1 {
    AuthenticationCapacityPlanV1 {
        version: 1,
        unscoped,
        tenants: vec![AuthenticationTenantCapacityV1 {
            tenant_id: "tenant-a".into(),
            capacity: tenant_a,
        }],
    }
}

/// A genuinely new V4 ledger (genesis + capacity plan = 2 records) with an
/// explicit closed plan. `policy_max_records` must fund
/// `2 + unscoped.max_records + tenant_a.max_records` (budget::install).
async fn build_v4(policy_max_records: u64, plan: AuthenticationCapacityPlanV1) -> Fixture {
    build_with_plan(policy_max_records, 8 * 1024 * 1024, Some(plan)).await
}

async fn build_with_plan(
    max_records: u64,
    max_serialized_bytes: u64,
    plan: Option<AuthenticationCapacityPlanV1>,
) -> Fixture {
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
    member(&store, &issuer.issuer, "approved-subject");
    let store = Arc::new(store);
    let installation = Uuid::new_v4();
    let policy = AuthenticationAuditPolicyV1 {
        max_records,
        max_serialized_bytes,
        approved_clients: ["web-public", "cli-public"]
            .map(|client_id| ApprovedAuthenticationClientV1 {
                issuer: issuer.issuer.clone(),
                client_id: client_id.into(),
            })
            .to_vec(),
    };
    let audit_path = files.path().join("authentication.sqlite");
    let audit = Arc::new(match plan {
        Some(plan) => AuthenticationAudit::init_new_v4(
            &audit_path,
            installation,
            policy.clone(),
            plan,
            Redactor::new(vec![]),
        )
        .unwrap(),
        None => AuthenticationAudit::init_new(
            &audit_path,
            installation,
            policy.clone(),
            Redactor::new(vec![]),
        )
        .unwrap(),
    });
    let config = OidcGatewayConfigV1 {
        version: 1,
        provider: issuer.config(),
        provider_label: "Controlled issuer".into(),
        web_origin: "http://127.0.0.1:18100".into(),
        mapping: OidcClaimsMappingV1 {
            revision: "mapping-1".into(),
            default_capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
            rules: vec![],
        },
    };
    let router = router(store.clone(), audit.clone(), config.clone());
    Fixture {
        files,
        issuer,
        audit,
        router,
        store,
        installation,
        policy,
        config,
    }
}

async fn post(
    router: &Router,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::post(path).header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 35_000).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

fn diagnostic_response(status: StatusCode, body: &Value) -> (u16, &'static str) {
    let code = match body["error"]["code"].as_str() {
        Some("permission_denied") => "permission_denied",
        Some("session_conflict") => "session_conflict",
        Some("capacity_exhausted") => "capacity_exhausted",
        Some("authentication_subject_capacity_exhausted") => {
            "authentication_subject_capacity_exhausted"
        }
        Some("identity_unavailable") => "identity_unavailable",
        Some("credential_invalid") => "credential_invalid",
        Some("provider_unavailable") => "provider_unavailable",
        Some("audit_unavailable") => "audit_unavailable",
        None => "no_error_code",
        Some(_) => "other_redacted",
    };
    (status.as_u16(), code)
}

fn diagnostic_provider_requests(issuer: &ControlledIssuer) -> (usize, usize, usize) {
    let requests = issuer.requests();
    (
        requests
            .iter()
            .filter(|(path, _)| path.ends_with("openid-configuration"))
            .count(),
        requests
            .iter()
            .filter(|(path, _)| path.ends_with("/keys"))
            .count(),
        requests
            .iter()
            .filter(|(path, _)| path.ends_with("/token"))
            .count(),
    )
}

/// The gateway paces anonymous native intents to `ANONYMOUS_STARTS` (8) per
/// one-second `START_WINDOW` (src/api/oidc/gateway.rs). A scenario that needs
/// more than eight anonymous intents must wait for a fresh window. This paces
/// the test client like a real caller; it never relaxes the product's
/// anti-abuse boundary, nor any ledger/subject capacity assertion.
async fn fresh_anonymous_start_window() {
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
}

async fn admit(router: &Router, token: &str) -> (StatusCode, Value) {
    // Each burst stays within the gateway's anonymous start window; scenarios
    // needing more than eight intents call fresh_anonymous_start_window().
    // An unexpected setup refusal is not an authentication-ledger RED.
    let (status, intent) = post(
        router,
        "/auth/oidc/native-intent",
        None,
        json!({"version": 1, "action": {"kind": "login"}}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "HARNESS INCONCLUSIVE: native intent failed; diagnostic={:?}",
        diagnostic_response(status, &intent)
    );
    assert!(intent["intent_id"].as_str().is_some());
    post(
        router,
        "/auth/oidc/admit",
        Some(token),
        json!({"version": 1, "intent_id": intent["intent_id"]}),
    )
    .await
}

#[tokio::test]
async fn refused_verified_proof_replay_preserves_shared_ledger_for_another_subject() {
    // V4: the unscoped partition funds exactly one verified denial (intention +
    // terminal = 2 records); the tenant-a partition funds the approved subject.
    // Replaying the refused proof must not spend either partition again, and an
    // independently approved subject must still admit. policy 8 = 2 genesis/plan
    // + 2 unscoped + 4 tenant-a (budget::install).
    let f = build_v4(8, tenant_a_plan(capacity(2, 70_000), capacity(4, 100_000))).await;
    let (issuer, audit, router) = (&f.issuer, &f.audit, &f.router);
    let before = audit.verify().unwrap().records;
    assert_eq!(before, 2, "V4 genesis + capacity plan");
    let token = issuer.token("cli-public", "never-provisioned", None, &[], "one-proof");
    let (first_status, first_body) = admit(router, &token).await;
    assert_eq!(first_status, StatusCode::FORBIDDEN);
    assert_eq!(first_body["error"]["code"], "permission_denied");
    let after_first = audit.verify().unwrap().records;
    assert_eq!(
        after_first,
        before + 2,
        "the first verified denial must remain durably audited"
    );

    let mut trace = Vec::new();
    for _ in 0..7 {
        let (status, body) = admit(router, &token).await;
        trace.push((
            status.as_u16(),
            body["error"]["code"].clone(),
            audit.verify().unwrap().records,
        ));
    }
    let after_replays = audit.verify().unwrap().records;
    let other = issuer.token(
        "cli-public",
        "approved-subject",
        None,
        &[],
        "independent-proof",
    );
    // 1 initial + 7 replays already consumed this window's 8 anonymous starts.
    fresh_anonymous_start_window().await;
    let (other_status, other_body) = admit(router, &other).await;
    println!(
        "AUTH_LEDGER_REPLAY before={before} first={after_first} after_replays={after_replays} other={} trace={trace:?}",
        other_status.as_u16()
    );
    assert_eq!(
        other_status,
        StatusCode::CREATED,
        "one refused proof must not deny an independently approved subject; error={}",
        other_body["error"]["code"]
    );
    assert_eq!(
        after_replays, after_first,
        "replaying one refused proof must use its existing durable denial without appending another operation"
    );
    assert!(
        trace.iter().all(|(status, code, _)| {
            (*status == 403 && code == "permission_denied")
                || (*status == 409 && code == "session_conflict")
        }),
        "replays must remain refused with an explicit reason: {trace:?}"
    );
    assert_eq!(audit.verify().unwrap().records, after_first + 2);
    let requests = issuer.requests();
    assert!(
        requests.iter().any(|(path, _)| path.ends_with("/keys")),
        "the signed proof must be verified by the real configured verifier"
    );
    assert!(
        requests.iter().all(|(path, _)| !path.ends_with("/token")),
        "replay needs no fresh IdP exchange"
    );
}

#[tokio::test]
async fn fresh_tokens_for_one_refused_subject_cannot_spend_the_shared_ledger() {
    // V4 replaces the V3 per-subject attempt budget with closed logical
    // partitions. The unscoped partition funds exactly one verified denial
    // (intention + terminal = 2 records); fresh proofs for the same refused
    // subject cannot spend it again, and the tenant-a partition stays available
    // for an independently approved subject. policy 8 = 2 genesis/plan + 2
    // unscoped + 4 tenant-a (budget::install).
    let f = build_v4(8, tenant_a_plan(capacity(2, 70_000), capacity(4, 100_000))).await;
    let before = f.audit.verify().unwrap().records;
    assert_eq!(before, 2, "V4 genesis + capacity plan");
    let first = f
        .issuer
        .token("cli-public", "unapproved", None, &[], "first");
    assert_eq!(admit(&f.router, &first).await.0, StatusCode::FORBIDDEN);
    let after_first = f.audit.verify().unwrap().records;
    let mut refusals = Vec::new();
    for index in 0..7 {
        let token = f.issuer.token(
            "cli-public",
            "unapproved",
            None,
            &[],
            &format!("fresh-{index}"),
        );
        let (status, body) = admit(&f.router, &token).await;
        refusals.push((status, body["error"]["code"].clone()));
    }
    let after = f.audit.verify().unwrap().records;
    let other = f
        .issuer
        .token("cli-public", "approved-subject", None, &[], "other");
    // 1 initial + 7 fresh proofs already consumed this window's 8 anonymous starts.
    fresh_anonymous_start_window().await;
    assert_eq!(
        admit(&f.router, &other).await.0,
        StatusCode::CREATED,
        "one verified subject with fresh tokens must leave room for another subject; records={after}"
    );
    assert_eq!(after_first, before + 2);
    assert_eq!(
        after, after_first,
        "fresh refused proofs must append no durable operation to the shared ledger"
    );
    // V4 durable partition exhaustion is fail-closed `audit_unavailable` (503),
    // not the V3 subject-budget 429. The isolation guarantee is unchanged: no
    // ledger growth above, and the approved subject still admits.
    assert!(
        refusals.iter().all(|(status, code)| {
            *status == StatusCode::SERVICE_UNAVAILABLE && code == "audit_unavailable"
        }),
        "fresh proofs must be refused without spending the shared ledger: {refusals:?}"
    );
}

#[tokio::test]
async fn concurrent_refused_proof_is_reserved_once() {
    // V4: two unscoped denials fit (first race + ready race = 4 records); the
    // provider-inflight refusals and the dedup conflicts append nothing.
    // policy 100 generously funds 2 genesis/plan + 8 unscoped + 8 tenant-a.
    let f = build_v4(
        100,
        tenant_a_plan(capacity(8, 150_000), capacity(8, 150_000)),
    )
    .await;
    let token = f
        .issuer
        .token("cli-public", "unapproved", None, &[], "concurrent");
    assert!(
        f.config.provider.limits.max_inflight >= 4,
        "HARNESS INCONCLUSIVE: four verification operations must fit before discovery coordination"
    );
    // Hold the first real discovery response before verification can finish.
    // discover() retains its update lock across this owned /keys response;
    // the other three calls must hit that non-waiting capacity boundary.
    let barrier = f.issuer.block_keys();
    let initial_router = f.router.clone();
    let initial_token = token.clone();
    let initial = tokio::spawn(async move { admit(&initial_router, &initial_token).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), barrier.entered())
        .await
        .expect("HARNESS INCONCLUSIVE: controlled key response was not reached");
    let (two, three, four) = tokio::join!(
        admit(&f.router, &token),
        admit(&f.router, &token),
        admit(&f.router, &token),
    );
    let records_while_discovering = f.audit.verify().unwrap().records;
    let requests_while_discovering = diagnostic_provider_requests(&f.issuer);
    barrier.release();
    f.issuer.clear_keys_barrier();
    let one = initial.await.unwrap();
    let statuses = [one.0, two.0, three.0, four.0];
    let diagnostics =
        [&one, &two, &three, &four].map(|(status, body)| diagnostic_response(*status, body));
    println!(
        "AUTH_LEDGER_CONCURRENT outcomes={diagnostics:?} records_while_discovering={records_while_discovering} requests_while_discovering={requests_while_discovering:?} records={} provider_requests_discovery_keys_token={:?}",
        f.audit.verify().unwrap().records,
        diagnostic_provider_requests(&f.issuer)
    );
    assert_eq!(
        records_while_discovering, 2,
        "V4 genesis + plan; no intention yet"
    );
    assert_eq!(requests_while_discovering, (1, 1, 0));
    assert_eq!(
        diagnostic_response(one.0, &one.1),
        (403, "permission_denied"),
        "HARNESS INCONCLUSIVE: the released controlled proof did not complete its first denial"
    );
    assert_eq!(
        &diagnostics[1..],
        &[(429, "capacity_exhausted"); 3],
        "the three refusals must occur while the first discovery owns the update lock"
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::FORBIDDEN)
            .count(),
        1,
        "only the first reservation may evaluate a new denial: {statuses:?}"
    );
    assert_eq!(
        f.audit.verify().unwrap().records,
        4,
        "V4 genesis + plan + one denial"
    );
    assert_eq!(diagnostic_provider_requests(&f.issuer), (1, 1, 0));
    // With all four requests complete, retry each consumed intent using a new
    // nonce and the same proof. Every replay must now conflict without another
    // ledger write or provider fetch, including earlier capacity refusals.
    for _ in 0..4 {
        let (status, body) = admit(&f.router, &token).await;
        assert_eq!(
            diagnostic_response(status, &body),
            (409, "session_conflict")
        );
        assert_eq!(f.audit.verify().unwrap().records, 4);
    }
    assert_eq!(diagnostic_provider_requests(&f.issuer), (1, 1, 0));

    // Exercise the original strict race after metadata/keys are ready. A new
    // proof from the same refused subject must reserve exactly one intention,
    // even when all four verifications can reach the durable ledger.
    // 4 concurrent + 4 replays already consumed this window's 8 anonymous starts.
    fresh_anonymous_start_window().await;
    let ready_token = f
        .issuer
        .token("cli-public", "unapproved", None, &[], "ready-concurrent");
    let before_ready = f.audit.verify().unwrap().records;
    let (one, two, three, four) = tokio::join!(
        admit(&f.router, &ready_token),
        admit(&f.router, &ready_token),
        admit(&f.router, &ready_token),
        admit(&f.router, &ready_token),
    );
    let diagnostics =
        [&one, &two, &three, &four].map(|(status, body)| diagnostic_response(*status, body));
    println!(
        "AUTH_LEDGER_CONCURRENT_READY outcomes={diagnostics:?} records={} provider_requests_discovery_keys_token={:?}",
        f.audit.verify().unwrap().records,
        diagnostic_provider_requests(&f.issuer)
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|outcome| **outcome == (403, "permission_denied"))
            .count(),
        1
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|outcome| **outcome == (409, "session_conflict"))
            .count(),
        3
    );
    assert_eq!(f.audit.verify().unwrap().records, before_ready + 2);
    assert_eq!(diagnostic_provider_requests(&f.issuer), (1, 1, 0));
}

#[tokio::test]
async fn refused_proof_deduplication_survives_a_new_gateway_and_audit_handle() {
    let f = build(100).await;
    let token = f
        .issuer
        .token("cli-public", "unapproved", None, &[], "before-reopen");
    assert_eq!(admit(&f.router, &token).await.0, StatusCode::FORBIDDEN);
    let before = f.audit.verify().unwrap();
    let reopened = Arc::new(
        AuthenticationAudit::open_existing(
            &f.files.path().join("authentication.sqlite"),
            f.installation,
            f.policy.clone(),
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let second = router(f.store.clone(), reopened.clone(), f.config.clone());
    assert_eq!(admit(&second, &token).await.0, StatusCode::CONFLICT);
    assert_eq!(reopened.verify().unwrap(), before);
}

#[tokio::test]
async fn consumed_admitted_proof_cannot_add_new_denial_records() {
    let f = build(100).await;
    let token = f
        .issuer
        .token("cli-public", "approved-subject", None, &[], "approved");
    assert_eq!(admit(&f.router, &token).await.0, StatusCode::CREATED);
    let before = f.audit.verify().unwrap();
    assert_eq!(admit(&f.router, &token).await.0, StatusCode::CONFLICT);
    assert_eq!(f.audit.verify().unwrap(), before);
}

#[tokio::test]
async fn a_new_proof_checks_membership_after_an_earlier_denial() {
    let f = build(13).await;
    let first = f
        .issuer
        .token("cli-public", "new-member", None, &[], "before-grant");
    assert_eq!(admit(&f.router, &first).await.0, StatusCode::FORBIDDEN);
    let spent = f
        .issuer
        .token("cli-public", "new-member", None, &[], "before-grant-spent");
    assert_eq!(
        admit(&f.router, &spent).await.0,
        StatusCode::TOO_MANY_REQUESTS,
        "the subject budget must really be exhausted before provisioning"
    );
    member(&f.store, &f.issuer.issuer, "new-member");
    let next = f
        .issuer
        .token("cli-public", "new-member", None, &[], "after-grant");
    assert_eq!(admit(&f.router, &next).await.0, StatusCode::CREATED);
    assert_eq!(
        admit(&f.router, &first).await.0,
        StatusCode::CONFLICT,
        "the consumed proof cannot acquire a new operation after a rights change"
    );
}

#[tokio::test]
async fn subject_budget_also_bounds_bytes_and_survives_reopen() {
    let f = build_limits(10_000, 8_000).await;
    let first = f
        .issuer
        .token("cli-public", "unapproved", None, &[], "bytes-first");
    assert_eq!(admit(&f.router, &first).await.0, StatusCode::FORBIDDEN);
    let before = f.audit.verify().unwrap();
    let reopened = Arc::new(
        AuthenticationAudit::open_existing(
            &f.files.path().join("authentication.sqlite"),
            f.installation,
            f.policy.clone(),
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let second = router(f.store.clone(), reopened.clone(), f.config.clone());
    let fresh = f
        .issuer
        .token("cli-public", "unapproved", None, &[], "bytes-fresh");
    let (status, body) = admit(&second, &fresh).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        body["error"]["code"],
        "authentication_subject_capacity_exhausted"
    );
    assert_eq!(body["error"]["retry_action"], "contact_operator");
    assert_eq!(reopened.verify().unwrap(), before);
    let other = f
        .issuer
        .token("cli-public", "approved-subject", None, &[], "bytes-other");
    assert_eq!(admit(&second, &other).await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn successful_admissions_release_the_pending_subject_budget() {
    let f = build(13).await;
    for salt in ["success-one", "success-two"] {
        let token = f
            .issuer
            .token("cli-public", "approved-subject", None, &[], salt);
        assert_eq!(admit(&f.router, &token).await.0, StatusCode::CREATED);
    }
    assert_eq!(f.audit.verify().unwrap().records, 5);
}

#[tokio::test]
async fn a_crashed_pending_reservation_keeps_proof_and_subject_budget() {
    use agentsandbox::audit::authentication::{
        AuthenticationActionV1, AuthenticationPhaseV1, AuthenticationRecordV1,
        AuthenticationTokenFingerprintV1,
    };
    let f = build(13).await;
    let token = f
        .issuer
        .token("cli-public", "pending-subject", None, &[], "pending");
    // Fixture the trusted verifier's durable boundary, then abandon it without
    // a result. The later router still verifies the synthetic signature itself.
    let record = AuthenticationRecordV1 {
        operation_id: Uuid::new_v4(),
        action: AuthenticationActionV1::SessionAdmission,
        phase: AuthenticationPhaseV1::Intention,
        issuer: f.issuer.issuer.clone(),
        client_id: "cli-public".into(),
        principal: None,
        transaction_id: None,
        session_id: Some(Uuid::new_v4()),
        causal: None,
    };
    let pending = f
        .audit
        .begin_verified_initial(
            record,
            PrincipalRefV1::HumanOidc {
                issuer: f.issuer.issuer.clone(),
                subject: "pending-subject".into(),
            },
            AuthenticationTokenFingerprintV1::access_token(&token),
        )
        .unwrap();
    assert!(pending.inserted);
    let before = f.audit.verify().unwrap();
    assert_eq!(before.records, 2);
    let reopened = Arc::new(
        AuthenticationAudit::open_existing(
            &f.files.path().join("authentication.sqlite"),
            f.installation,
            f.policy.clone(),
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let second = router(f.store.clone(), reopened.clone(), f.config.clone());
    assert_eq!(admit(&second, &token).await.0, StatusCode::CONFLICT);
    let next = f
        .issuer
        .token("cli-public", "pending-subject", None, &[], "after-crash");
    assert_eq!(admit(&second, &next).await.0, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(reopened.verify().unwrap(), before);
    let other = f.issuer.token(
        "cli-public",
        "approved-subject",
        None,
        &[],
        "after-crash-other",
    );
    assert_eq!(admit(&second, &other).await.0, StatusCode::CREATED);
}

async fn web_login(f: &Fixture, subject: &str) -> (StatusCode, Value) {
    let seeded = f
        .router
        .clone()
        .oneshot(
            Request::get("/auth/oidc/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(seeded.status(), StatusCode::OK);
    assert!(seeded.headers().get("set-cookie").is_none());
    let web_request = |path: &str, cookie: Option<&str>, body: Value| {
        let mut request = Request::post(path)
            .header("content-type", "application/json")
            .header("origin", &f.config.web_origin)
            .header("sec-fetch-site", "same-origin")
            .header("sec-fetch-mode", "cors");
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        request.body(Body::from(body.to_string())).unwrap()
    };
    let response = f
        .router
        .clone()
        .oneshot(web_request("/auth/oidc/start", None, json!({"version": 1})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let start: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 35_000).await.unwrap()).unwrap();
    let correlation_token = start["correlation_token"].as_str().unwrap().to_owned();
    let url = reqwest::Url::parse(start["authorization_url"].as_str().unwrap()).unwrap();
    let params = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    f.issuer
        .login_response(&params["nonce"], subject, "web-proof");
    let callback = Request::get(format!(
        "/auth/oidc/callback?code=controlled-code&state={}",
        params["state"]
    ))
    .header("cookie", &cookie)
    .header("sec-fetch-site", "cross-site")
    .body(Body::empty())
    .unwrap();
    assert_eq!(
        f.router.clone().oneshot(callback).await.unwrap().status(),
        StatusCode::SEE_OTHER
    );
    let response = f
        .router
        .clone()
        .oneshot(web_request(
            "/auth/oidc/tokens",
            Some(&cookie),
            json!({"version": 1, "correlation_token": correlation_token}),
        ))
        .await
        .unwrap();
    let status = response.status();
    (
        status,
        serde_json::from_slice(&to_bytes(response.into_body(), 35_000).await.unwrap()).unwrap(),
    )
}

#[tokio::test]
async fn cli_and_web_share_the_same_verified_subject_budget() {
    let f = build(13).await;
    let cli = f
        .issuer
        .token("cli-public", "unapproved", None, &[], "cli-proof");
    assert_eq!(admit(&f.router, &cli).await.0, StatusCode::FORBIDDEN);
    let before = f.audit.verify().unwrap();
    let (status, body) = web_login(&f, "unapproved").await;
    println!(
        "AUTH_LEDGER_WEB delivery={:?} records_before={} records_after={} provider_requests_discovery_keys_token={:?}",
        diagnostic_response(status, &body),
        before.records,
        f.audit.verify().unwrap().records,
        diagnostic_provider_requests(&f.issuer)
    );
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        body["error"]["code"],
        "authentication_subject_capacity_exhausted"
    );
    assert_eq!(body["error"]["retry_action"], "contact_operator");
    assert_eq!(f.audit.verify().unwrap(), before);
    assert_eq!(
        f.issuer
            .requests()
            .iter()
            .filter(|(path, _)| path.ends_with("/token"))
            .count(),
        1
    );
}

fn entries(path: &std::path::Path) -> Vec<(i64, String, String, String, String)> {
    let db = rusqlite::Connection::open(path).unwrap();
    let mut statement = db.prepare("SELECT seq,received_at,prev_hash,hash,payload FROM authentication_entries ORDER BY seq").unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn version(path: &std::path::Path) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

#[tokio::test]
async fn v1_requires_explicit_migration_and_preserves_its_entries_and_backup() {
    use agentsandbox::audit::authentication::{
        AuthenticationActionV1 as A, AuthenticationAuditFailureV1 as Failure,
        AuthenticationOutcomeV1 as O, AuthenticationPhaseV1 as P, AuthenticationReasonV1 as R,
        AuthenticationRecordV1,
    };
    let f = build(13).await;
    // These are exactly V1's existing Genesis/Authentication variants. No V2
    // entry is present when the legacy header is restored for this fixture.
    let record = AuthenticationRecordV1 {
        operation_id: Uuid::new_v4(),
        action: A::SessionAdmission,
        phase: P::Intention,
        issuer: f.issuer.issuer.clone(),
        client_id: "cli-public".into(),
        principal: None,
        transaction_id: None,
        session_id: Some(Uuid::new_v4()),
        causal: None,
    };
    let receipt = f.audit.append_typed(record.clone()).unwrap();
    f.audit
        .append_typed(AuthenticationRecordV1 {
            phase: P::Result {
                outcome: O::Rejected,
                reason: R::NoMembership,
            },
            principal: Some(PrincipalRefV1::HumanOidc {
                issuer: f.issuer.issuer.clone(),
                subject: "historically-denied".into(),
            }),
            causal: Some(receipt),
            ..record
        })
        .unwrap();
    let path = f.files.path().join("authentication.sqlite");
    rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 1_000_001)
        .unwrap();
    let before = entries(&path);
    let reopened = Arc::new(
        AuthenticationAudit::open_existing(
            &path,
            f.installation,
            f.policy.clone(),
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let old_head = reopened.verify().unwrap();
    assert_eq!(
        version(&path),
        1_000_001,
        "open must not migrate a V1 ledger implicitly"
    );
    let other = f.issuer.token(
        "cli-public",
        "approved-subject",
        None,
        &[],
        "migration-other",
    );
    // The current gateway fails closed at assembly on an unmigrated V1 ledger:
    // GatewayState::with_oidc requires format 3 or 4 (src/api.rs). No admission
    // can even be attempted, and the durable entries stay untouched.
    assert!(
        try_router(f.store.clone(), reopened.clone(), f.config.clone()).is_err(),
        "a V1 ledger must not assemble an OIDC gateway"
    );
    assert_eq!(entries(&path), before);

    let occupied = f.files.path().join("occupied-backup");
    std::fs::write(&occupied, b"keep-existing-backup").unwrap();
    assert!(reopened.migrate_verified_initial_v2(&occupied).is_err());
    assert_eq!(std::fs::read(&occupied).unwrap(), b"keep-existing-backup");
    assert_eq!(version(&path), 1_000_001);
    assert_eq!(entries(&path), before);
    let backup = f.files.path().join("before-verified-v2.sqlite");
    assert_eq!(
        reopened.migrate_verified_initial_v2(&backup).unwrap(),
        old_head
    );
    assert_eq!(version(&path), 1_000_002);
    assert_eq!(version(&backup), 1_000_001);
    assert_eq!(entries(&path), before);
    assert_eq!(entries(&backup), before);
    let saved = AuthenticationAudit::open_existing(
        &backup,
        f.installation,
        f.policy.clone(),
        Redactor::new(vec![]),
    )
    .unwrap();
    assert_eq!(saved.verify().unwrap(), old_head);
    let duplicate = f.files.path().join("must-not-be-created.sqlite");
    assert_eq!(
        reopened
            .migrate_verified_initial_v2(&duplicate)
            .unwrap_err()
            .downcast_ref::<Failure>(),
        Some(&Failure::Format)
    );
    assert!(!duplicate.exists());
    // V2 is still not a supported gateway format. The explicit chain continues
    // to V3 (migrate_native_logout_v3) before any admission; like V1->V2 it
    // consumes no ledger entry or quota and leaves the V1 backup above intact.
    let before_v3 = f.files.path().join("before-native-v3.sqlite");
    reopened.migrate_native_logout_v3(&before_v3).unwrap();
    assert_eq!(version(&path), 1_000_003);
    assert_eq!(version(&before_v3), 1_000_002);
    assert_eq!(entries(&path), before);
    let old_router = router(f.store.clone(), reopened.clone(), f.config.clone());
    // A historical verified denial already consumes that subject's budget.
    let denied = f.issuer.token(
        "cli-public",
        "historically-denied",
        None,
        &[],
        "historical-replay",
    );
    assert_eq!(
        admit(&old_router, &denied).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(reopened.verify().unwrap(), old_head);
    assert_eq!(admit(&old_router, &other).await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn a_full_ledger_cannot_turn_a_reserved_proof_into_an_admitted_session() {
    let f = build(2).await;
    let token = f
        .issuer
        .token("cli-public", "approved-subject", None, &[], "full-ledger");
    let (status, body) = admit(&f.router, &token).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "audit_unavailable");
    assert_eq!(f.audit.verify().unwrap().records, 2);
    // The committed intention survived the failed result; retrying cannot
    // create a live registry session or spend another durable record.
    assert_eq!(admit(&f.router, &token).await.0, StatusCode::CONFLICT);
    assert_eq!(f.audit.verify().unwrap().records, 2);
}

#[tokio::test]
async fn concurrent_fresh_proofs_share_one_subject_reservation_budget() {
    let f = build(13).await;
    let tokens = ["fresh-one", "fresh-two", "fresh-three", "fresh-four"]
        .map(|salt| f.issuer.token("cli-public", "unapproved", None, &[], salt));
    let (one, two, three, four) = tokio::join!(
        admit(&f.router, &tokens[0]),
        admit(&f.router, &tokens[1]),
        admit(&f.router, &tokens[2]),
        admit(&f.router, &tokens[3]),
    );
    let statuses = [one.0, two.0, three.0, four.0];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::FORBIDDEN)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::TOO_MANY_REQUESTS)
            .count(),
        3
    );
    assert_eq!(f.audit.verify().unwrap().records, 3);
}

#[tokio::test]
async fn v2_proof_entries_cannot_be_opened_under_a_downgraded_v1_header() {
    let f = build(100).await;
    let token = f
        .issuer
        .token("cli-public", "unapproved", None, &[], "downgrade");
    assert_eq!(admit(&f.router, &token).await.0, StatusCode::FORBIDDEN);
    let path = f.files.path().join("authentication.sqlite");
    let before = entries(&path);
    rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 1_000_001)
        .unwrap();
    assert!(
        AuthenticationAudit::open_existing(
            &path,
            f.installation,
            f.policy.clone(),
            Redactor::new(vec![])
        )
        .is_err()
    );
    assert_eq!(entries(&path), before);
}
