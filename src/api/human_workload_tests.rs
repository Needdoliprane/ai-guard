//! Class-collision regressions with real Stores and admitted proof lifecycles.
//! The controlled local issuer is software test evidence, not IdP qualification.
use super::*;
use crate as test_product;
use crate::api::read_execution_tests::{human_get, identity_change, local_identity_state};
use crate::workload::{WorkloadBindingIssueV1, WorkloadSubjectV1};
use axum::http::Request;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use tower::ServiceExt;
use uuid::Uuid;

#[allow(dead_code)]
#[path = "../../tests/support/oidc_fixture.rs"]
mod controlled;

fn migrate(state: &GatewayState, directory: &std::path::Path) {
    for tenant in state.tenants.iter() {
        tenant
            .store
            .migrate_workload_v6(&directory.join(format!("{}-before-v6.sqlite", tenant.tenant_id)))
            .unwrap();
    }
}
fn alias(state: &GatewayState, source: usize, digest: &[u8; 32]) {
    // Authorized provisioning deliberately creates conflicting metadata. A
    // digest alone is never used as a live broker/worker proof by this test.
    state.tenants[source]
        .store
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: Uuid::new_v4().to_string(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: Uuid::new_v4().to_string(),
                incarnation_id: Uuid::new_v4().to_string(),
            },
            sha256_digest: hex::encode(digest),
            expires_at: (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
        })
        .unwrap();
}
async fn extract(state: &GatewayState, token: &str) -> VerifiedHuman {
    let (mut parts, _) = Request::builder()
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
        .into_parts();
    VerifiedHuman::from_request_parts(&mut parts, state)
        .await
        .unwrap()
}
async fn require_reauth_failure_before_operation(state: &GatewayState, proof: &VerifiedHuman) {
    let called = Arc::new(AtomicBool::new(false));
    let observed = called.clone();
    let result = run_human(state, proof, None, move |_, _| {
        observed.store(true, Ordering::SeqCst);
        Ok(())
    })
    .await;
    let Err((status, Json(error))) = result else {
        panic!("retained human proof admitted content after workload collision")
    };
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["error"]["code"], "identity_unavailable");
    assert_eq!(error["error"]["retry_action"], "contact_operator");
    assert!(!called.load(Ordering::SeqCst));
}

#[tokio::test]
async fn extracted_local_human_proof_rechecks_workload_collision_before_read() {
    let (directory, state, token) = local_identity_state();
    migrate(&state, directory.path());
    let proof = extract(&state, &token).await;
    let checked = proof.clone();
    state
        .blocking_identity(move |state| reauthenticate(&state, &checked))
        .await
        .unwrap();
    alias(&state, 0, &proof.digest);
    require_reauth_failure_before_operation(&state, &proof).await;
}

async fn post(
    router: &Router,
    path: &str,
    value: Value,
    bearer: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::post(path).header("content-type", "application/json");
    if let Some(token) = bearer {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(value.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 262144)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn admitted_oidc_proof_rechecks_workload_collision_on_authentication_and_reauth() {
    use crate::audit::authentication::{
        ApprovedAuthenticationClientV1, AuthenticationAudit, AuthenticationAuditPolicyV1,
    };
    let (directory, state, _) = local_identity_state();
    migrate(&state, directory.path());
    let issuer = controlled::ControlledIssuer::new().await;
    let principal = PrincipalRefV1::HumanOidc {
        issuer: issuer.issuer.clone(),
        subject: "collision-reader".into(),
    };
    state.tenants[0]
        .store
        .mutate_identity(&identity_change(IdentityChangeV1::Account {
            expected_revision: 0,
            account: HumanAccountV1 {
                version: 1,
                principal: principal.clone(),
                display_name: "Controlled OIDC collision reader".into(),
                revision: 1,
                active: true,
            },
        }))
        .unwrap();
    state.tenants[0]
        .store
        .mutate_identity(&identity_change(IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: MembershipV1 {
                version: 1,
                membership_id: Uuid::new_v4().to_string(),
                principal,
                tenant_id: "tenant-a".into(),
                organization_scope: "organization-oidc".into(),
                revision: 1,
                active: true,
                capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
            },
        }))
        .unwrap();
    let audit = Arc::new(
        AuthenticationAudit::init_new(
            &directory.path().join("authentication-collision.sqlite"),
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
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let oidc = OidcGateway::new(
        OidcGatewayConfigV1 {
            version: 1,
            provider: issuer.config(),
            provider_label: "Controlled collision fixture".into(),
            web_origin: "http://127.0.0.1:18100".into(),
            mapping: OidcClaimsMappingV1 {
                revision: "collision-1".into(),
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
    let token = issuer.token(
        "cli-public",
        "collision-reader",
        None,
        &[],
        "human-workload-collision",
    );
    let (status, intent) = post(
        &router,
        "/auth/oidc/native-intent",
        json!({"version":1,"action":{"kind":"login"}}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        post(
            &router,
            "/auth/oidc/admit",
            json!({"version":1,"intent_id":intent["intent_id"]}),
            Some(&token)
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        human_get(&router, &token, "/api/v1/human/identity", None)
            .await
            .0,
        StatusCode::OK
    );
    let proof = extract(&state, &token).await;
    let checked = proof.clone();
    state
        .blocking_identity(move |state| reauthenticate(&state, &checked))
        .await
        .unwrap();
    alias(&state, 1, &proof.digest);
    let result = human_get(&router, &token, "/api/v1/human/identity", None).await;
    assert_eq!(result.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(result.1["error"]["code"], "identity_unavailable");
    assert_eq!(result.1["error"]["retry_action"], "contact_operator");
    assert!(result.1.get("principal").is_none() && result.1.get("contexts").is_none());
    require_reauth_failure_before_operation(&state, &proof).await;
}
