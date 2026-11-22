//! The real membership helper with a verified provider proof and real Stores.
//! This isolates profile admission from the earlier collision scan. It does
//! not replace the end-to-end intention/admission/rotation/cleanup canaries.
use super::*;
use crate as test_product;
use crate::api::read_execution_tests::{identity_change, local_identity_state};
use crate::audit::{
    authentication::{
        ApprovedAuthenticationClientV1, AuthenticationAuditPolicyV1, AuthenticationCapacityPlanV1,
        AuthenticationCapacityV1, AuthenticationTenantCapacityV1,
    },
    read_test_observer::{Call, observe},
};
use crate::identity::*;

// The shared issuer's other fault controls are exercised by integration targets.
#[allow(dead_code)]
#[path = "../../../tests/support/oidc_fixture.rs"]
pub(in crate::api::oidc) mod controlled;

#[path = "admission_capacity_tests.rs"]
mod capacity_tests;

fn capacity_plan(
    unscoped_records: u64,
    a_records: u64,
    b_records: u64,
) -> AuthenticationCapacityPlanV1 {
    AuthenticationCapacityPlanV1 {
        version: 1,
        unscoped: AuthenticationCapacityV1 {
            max_records: unscoped_records,
            max_serialized_bytes: 256 * 1024,
        },
        tenants: [("tenant-a", a_records), ("tenant-b", b_records)]
            .map(|(tenant_id, max_records)| AuthenticationTenantCapacityV1 {
                tenant_id: tenant_id.into(),
                capacity: AuthenticationCapacityV1 {
                    max_records,
                    max_serialized_bytes: 1536 * 1024,
                },
            })
            .to_vec(),
    }
}

async fn verified_memberships(
    state: &GatewayState,
    access: &Arc<VerifiedAccess>,
) -> ApiResult<BTreeSet<HumanCapabilityV1>> {
    let access = Arc::clone(access);
    state
        .blocking_identity(move |state| {
            let oidc = state.oidc.as_ref().unwrap();
            membership(&state, oidc, &access)
        })
        .await
}

#[tokio::test]
async fn verified_oidc_memberships_do_not_open_saturated_profile_stores() {
    let (directory, state, _local_token) = local_identity_state();
    let issuer = controlled::ControlledIssuer::new().await;
    let principal = PrincipalRefV1::HumanOidc {
        issuer: issuer.issuer.clone(),
        subject: "profile-reader".into(),
    };
    for tenant in state.tenants.iter() {
        tenant
            .store
            .sqlite()
            .unwrap()
            .mutate_identity(&identity_change(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "OIDC profile reader".into(),
                    revision: 1,
                    active: true,
                },
            }))
            .unwrap();
        tenant
            .store
            .sqlite()
            .unwrap()
            .mutate_identity(&identity_change(IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: MembershipV1 {
                    version: 1,
                    membership_id: Uuid::new_v4().to_string(),
                    principal: principal.clone(),
                    tenant_id: tenant.tenant_id.clone(),
                    organization_scope: "organization-ab".into(),
                    revision: 1,
                    active: true,
                    capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                },
            }))
            .unwrap();
    }
    let audit = Arc::new(
        AuthenticationAudit::init_new_v4(
            &directory.path().join("authentication.sqlite"),
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
            capacity_plan(32, 483, 483), // Genesis + plan + all parts = G1000.
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let oidc = OidcGateway::new(
        OidcGatewayConfigV1 {
            version: 1,
            provider: issuer.config(),
            provider_label: "Controlled issuer".into(),
            web_origin: "http://127.0.0.1:18100".into(),
            mapping: OidcClaimsMappingV1 {
                revision: "profile-admission-1".into(),
                default_capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                rules: vec![],
            },
        },
        audit,
        None,
    )
    .unwrap();
    let state = state.with_oidc(oidc).unwrap();
    let oidc = state.oidc.as_ref().unwrap();
    let raw = issuer.token(
        "cli-public",
        "profile-reader",
        None,
        &[],
        "profile-admission",
    );
    let access = Arc::new(oidc.provider.verify_access(&raw).await.unwrap());
    assert_eq!(access.subject(), "profile-reader");
    // Satisfy the helper's durable-intention precondition after real signature
    // verification. The integration canaries verify install's ordering itself.
    let candidates = membership_capacity_candidates(&state, oidc, &access, &principal);
    assert!(candidates.failure.is_none());
    assert_eq!(
        candidates.tenant_ids,
        vec!["tenant-a".to_owned(), "tenant-b".to_owned()]
    );
    oidc.audit
        .begin_verified_initial_scoped(
            record(
                oidc,
                Action::SessionAdmission,
                Uuid::new_v4(),
                "cli-public",
                None,
                None,
                Uuid::new_v4(),
            ),
            principal.clone(),
            Fingerprint::access_token(&raw),
            AuthenticationTenantCandidatesV1::from_verified_memberships(
                principal,
                candidates.tenant_ids,
            )
            .unwrap(),
        )
        .unwrap();
    let observations_a = observe(state.tenants[0].store.sqlite().unwrap());
    let observations_b = observe(state.tenants[1].store.sqlite().unwrap());
    let expected = BTreeSet::from([HumanCapabilityV1::ReadMetadata]);
    assert_eq!(
        verified_memberships(&state, &access).await.unwrap(),
        expected
    );
    let baseline_a = observations_a
        .try_iter()
        .filter(|sample| sample.call == Call::BeginChecked)
        .count();
    let baseline_b = observations_b
        .try_iter()
        .filter(|sample| sample.call == Call::BeginChecked)
        .count();
    assert!(baseline_a > 0 && baseline_b > 0);

    let _slots_b: Vec<_> = (0..4)
        .map(|_| {
            state.tenants[1]
                .io
                .admit(store_io::Lane::Read, false)
                .unwrap()
        })
        .collect();
    let actual = verified_memberships(&state, &access).await;
    let transactions_b = observations_b
        .try_iter()
        .filter(|sample| sample.call == Call::BeginChecked)
        .count();
    assert_eq!(
        transactions_b, 0,
        "OIDC membership bypassed the saturated profile B admission"
    );
    assert_eq!(
        actual.unwrap(),
        expected,
        "the verified healthy A membership remains usable"
    );
    assert!(
        observations_a
            .try_iter()
            .filter(|sample| sample.call == Call::BeginChecked)
            .count()
            > 0
    );

    let _slots_a: Vec<_> = (0..4)
        .map(|_| {
            state.tenants[0]
                .io
                .admit(store_io::Lane::Read, false)
                .unwrap()
        })
        .collect();
    let refused = verified_memberships(&state, &access).await.unwrap_err().0;
    assert!(matches!(
        refused,
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
    ));
    assert_eq!(
        observations_a
            .try_iter()
            .filter(|sample| sample.call == Call::BeginChecked)
            .count(),
        0
    );
    assert_eq!(
        observations_b
            .try_iter()
            .filter(|sample| sample.call == Call::BeginChecked)
            .count(),
        0
    );
}

#[tokio::test]
async fn exhausted_unscoped_budget_preserves_healthy_b_when_first_tenant_read_is_saturated() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    async fn attempt(router: &Router, token: &str) -> (StatusCode, Value) {
        let request = Request::post("/auth/oidc/native-intent")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"version":1,"action":{"kind":"login"}}).to_string(),
            ))
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "HARNESS INCONCLUSIVE: native intent failed"
        );
        let intent: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 35_000).await.unwrap()).unwrap();
        let request = Request::post("/auth/oidc/admit")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(
                json!({"version":1,"intent_id":intent["intent_id"]}).to_string(),
            ))
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body =
            serde_json::from_slice(&to_bytes(response.into_body(), 35_000).await.unwrap()).unwrap();
        (status, body)
    }

    let (directory, state, _local_token) = local_identity_state();
    let issuer = controlled::ControlledIssuer::new().await;
    let audit = Arc::new(
        AuthenticationAudit::init_new_v4(
            &directory.path().join("authentication.sqlite"),
            Uuid::new_v4(),
            AuthenticationAuditPolicyV1 {
                max_records: 13,
                max_serialized_bytes: 4 * 1024 * 1024,
                approved_clients: ["web-public", "cli-public"]
                    .map(|client_id| ApprovedAuthenticationClientV1 {
                        issuer: issuer.issuer.clone(),
                        client_id: client_id.into(),
                    })
                    .to_vec(),
            },
            capacity_plan(2, 2, 7), // G13 = genesis/plan2 + unscoped2 + A2 + B7.
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let state = state
        .with_oidc(
            OidcGateway::new(
                OidcGatewayConfigV1 {
                    version: 1,
                    provider: issuer.config(),
                    provider_label: "Controlled issuer".into(),
                    web_origin: "http://127.0.0.1:18100".into(),
                    mapping: OidcClaimsMappingV1 {
                        revision: "membership-recovery-b".into(),
                        default_capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                        rules: vec![],
                    },
                },
                audit.clone(),
                None,
            )
            .unwrap(),
        )
        .unwrap();
    let router = gateway_router(state.clone());
    assert_eq!(audit.format_version().unwrap(), 4);
    assert_eq!(audit.verify().unwrap().records, 2);

    let denied = issuer.token("cli-public", "exhausted-b", None, &[], "before-provision");
    let (status, body) = attempt(&router, &denied).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "permission_denied");
    assert_eq!(audit.verify().unwrap().records, 4);
    let exhausted = audit.verify().unwrap();
    let spent = issuer.token(
        "cli-public",
        "exhausted-b",
        None,
        &[],
        "spent-before-provision",
    );
    let (status, body) = attempt(&router, &spent).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body["error"]["code"], "audit_unavailable",
        "HARNESS INCONCLUSIVE: the unscoped durable part was not exhausted"
    );
    assert_eq!(audit.verify().unwrap(), exhausted);

    assert_eq!(state.tenants[0].tenant_id, "tenant-a");
    assert_eq!(state.tenants[1].tenant_id, "tenant-b");
    for subject in ["fresh-b", "exhausted-b"] {
        let principal = PrincipalRefV1::HumanOidc {
            issuer: issuer.issuer.clone(),
            subject: subject.into(),
        };
        assert!(
            state.tenants[0]
                .store
                .sqlite()
                .unwrap()
                .human_account(&principal)
                .unwrap()
                .is_none()
        );
        for change in [
            IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Synthetic B reader".into(),
                    revision: 1,
                    active: true,
                },
            },
            IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: MembershipV1 {
                    version: 1,
                    membership_id: Uuid::new_v4().to_string(),
                    principal,
                    tenant_id: "tenant-b".into(),
                    organization_scope: "organization-b".into(),
                    revision: 1,
                    active: true,
                    capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                },
            },
        ] {
            state.tenants[1]
                .store
                .sqlite()
                .unwrap()
                .mutate_identity(&identity_change(change))
                .unwrap();
        }
    }

    // These RAII permits do not expire. A's admission remains unavailable
    // throughout both HTTP oracles, independently of scheduling or latency.
    let held_a: Vec<_> = (0..4)
        .map(|_| {
            state.tenants[0]
                .io
                .admit(store_io::Lane::Read, false)
                .unwrap()
        })
        .collect();
    let refused = || {
        state.tenants[0]
            .io
            .admit(store_io::Lane::Read, false)
            .err()
            .unwrap()
            .0
    };
    assert_eq!(held_a.len(), 4);
    assert_eq!(refused(), StatusCode::TOO_MANY_REQUESTS);
    let fresh = issuer.token("cli-public", "fresh-b", None, &[], "healthy-b-control");
    let (fresh_status, fresh_body) = attempt(&router, &fresh).await;
    assert_eq!(
        fresh_status,
        StatusCode::CREATED,
        "HARNESS INCONCLUSIVE: fresh B membership was not admitted; error={}",
        fresh_body["error"]["code"]
    );
    assert_eq!(audit.verify().unwrap().records, 6);
    assert_eq!(refused(), StatusCode::TOO_MANY_REQUESTS);
    let recovery = issuer.token("cli-public", "exhausted-b", None, &[], "healthy-b-recovery");
    let (status, body) = attempt(&router, &recovery).await;
    assert_eq!(refused(), StatusCode::TOO_MANY_REQUESTS);
    println!(
        "MEMBERSHIP_CAPACITY_B held_a={} fresh_status={} recovery_status={} recovery_error={}",
        held_a.len(),
        fresh_status.as_u16(),
        status.as_u16(),
        body["error"]["code"]
    );
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a saturated neighboring A and exhausted unscoped part must preserve the current authorized B membership; error={}",
        body["error"]["code"]
    );
    assert_eq!(audit.verify().unwrap().records, 8);
}
