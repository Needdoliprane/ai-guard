//! Real Router/SQLite stream admission; retained Bodies are never transport mocks.
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    identity::*,
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tower::ServiceExt;
use uuid::Uuid;

fn change(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "stream-capacity-fixture".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    }
}

fn fresh(path: &std::path::Path, tenant: &str) -> Arc<Store> {
    let store = Store::open_for_tenant(path, tenant, Redactor::new(vec![]), 10000).unwrap();
    store
        .migrate_control_v3(&path.with_extension("v3"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("v4"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("v5"))
        .unwrap();
    Arc::new(store)
}

fn credential(principal: &PrincipalRefV1, token: &str) -> LocalCredentialV1 {
    LocalCredentialV1 {
        version: 1,
        credential_id: Uuid::new_v4().to_string(),
        principal: principal.clone(),
        sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
        revision: 1,
        active: true,
        expires_at: None,
    }
}

struct Fixture {
    _files: tempfile::TempDir,
    store: Arc<Store>,
    router: Router,
    tokens: Vec<String>,
    alternate: String,
    other_tenant: String,
    first_credential: LocalCredentialV1,
    viewer: String,
}

impl Fixture {
    fn new() -> Self {
        let files = support::private_tempdir();
        let store = fresh(&files.path().join("a.sqlite"), "tenant-a");
        let other = fresh(&files.path().join("b.sqlite"), "tenant-b");
        let mut tokens = Vec::new();
        let alternate = generate_local_credential().unwrap();
        let other_tenant = generate_local_credential().unwrap();
        let mut first_credential = None;
        for index in 0..7 {
            let principal = PrincipalRefV1::HumanLocal {
                installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
                account_id: format!("00000000-0000-4000-8000-{index:012}"),
            };
            let token = generate_local_credential().unwrap();
            let binding = credential(&principal, &token);
            for (target, token_binding) in std::iter::once((&store, binding.clone()))
                .chain((index == 0).then(|| (&other, credential(&principal, &other_tenant))))
            {
                target
                    .provision_local_human(&LocalHumanProvisionV1 {
                        version: 1,
                        mutations: [
                            change(IdentityChangeV1::Account {
                                expected_revision: 0,
                                account: HumanAccountV1 {
                                    version: 1,
                                    principal: principal.clone(),
                                    display_name: "Stream capacity reader".into(),
                                    revision: 1,
                                    active: true,
                                },
                            }),
                            change(IdentityChangeV1::Credential {
                                expected_revision: 0,
                                binding: token_binding,
                            }),
                            change(IdentityChangeV1::Membership {
                                expected_revision: 0,
                                membership: MembershipV1 {
                                    version: 1,
                                    membership_id: Uuid::new_v4().to_string(),
                                    principal: principal.clone(),
                                    tenant_id: target.tenant_id().into(),
                                    organization_scope: "stream-capacity-organization".into(),
                                    revision: 1,
                                    active: true,
                                    capabilities: BTreeSet::from([
                                        HumanCapabilityV1::ReadMetadata,
                                        HumanCapabilityV1::ReadTenantMetadata,
                                    ]),
                                },
                            }),
                        ],
                    })
                    .unwrap();
            }
            if index == 0 {
                store
                    .mutate_identity(&change(IdentityChangeV1::Credential {
                        expected_revision: 0,
                        binding: credential(&principal, &alternate),
                    }))
                    .unwrap();
                first_credential = Some(binding);
            }
            tokens.push(token);
        }
        let viewer = "fixture-stream-capacity-viewer-a".repeat(3);
        let router = gateway_router(
            GatewayState::for_tenants(
                vec![
                    TenantGateway {
                        tenant_id: "tenant-a".into(),
                        store: Arc::clone(&store),
                        runner_token: "fixture-stream-capacity-runner-a".repeat(3),
                        viewer_token: viewer.clone(),
                    },
                    TenantGateway {
                        tenant_id: "tenant-b".into(),
                        store: other,
                        runner_token: "fixture-stream-capacity-runner-b".repeat(3),
                        viewer_token: "fixture-stream-capacity-viewer-b".repeat(3),
                    },
                ],
                Policy::default(),
                None,
                Redactor::new(vec![]),
            )
            .unwrap(),
        );
        Self {
            _files: files,
            store,
            router,
            tokens,
            alternate,
            other_tenant,
            first_credential: first_credential.unwrap(),
            viewer,
        }
    }
}

async fn request(router: &Router, token: &str, path: &str, context: Option<&str>) -> Response {
    let mut request = Request::builder()
        .uri(path)
        .header("authorization", format!("Bearer {token}"));
    if let Some(context) = context {
        request = request.header("x-aiguard-context", context);
    }
    router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[derive(Deserialize)]
struct Context {
    id: String,
    tenant_id: String,
    mode: String,
}

async fn contexts(fixture: &Fixture, token: &str, account: usize) -> Vec<Context> {
    let response = request(&fixture.router, token, "/api/v1/human/identity", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 512 * 1024).await.unwrap()).unwrap();
    assert_eq!(value["principal"]["kind"], "human_local");
    assert_eq!(
        value["principal"]["installation_id"],
        "909d9572-f1a2-4a53-ad8b-a741030a2b18"
    );
    assert_eq!(
        value["principal"]["account_id"],
        format!("00000000-0000-4000-8000-{account:012}")
    );
    value["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|context| serde_json::from_value(context.clone()).unwrap())
        .collect()
}

fn selected_context<'a>(contexts: &'a [Context], tenant: &str, mode: &str) -> &'a str {
    let matching: Vec<_> = contexts
        .iter()
        .filter(|context| context.tenant_id == tenant && context.mode == mode)
        .collect();
    assert_eq!(matching.len(), 1, "one explicit {tenant}/{mode} context");
    &matching[0].id
}

async fn stream(fixture: &Fixture, token: &str, context: &str) -> Response {
    request(
        &fixture.router,
        token,
        "/api/v1/human/stream?limit=1",
        Some(context),
    )
    .await
}

async fn retained(fixture: &Fixture, token: &str, context: &str, count: usize) -> Vec<Body> {
    let mut bodies = Vec::new();
    for _ in 0..count {
        let response = stream(fixture, token, context).await;
        assert_eq!(response.status(), StatusCode::OK);
        bodies.push(response.into_body());
    }
    bodies
}

// Missing principal admission lets one human occupy another human's capacity.
#[tokio::test]
async fn unpolled_bodies_share_principal_capacity_across_credentials_and_contexts() {
    let fixture = Fixture::new();
    let selected = contexts(&fixture, &fixture.tokens[0], 0).await;
    assert_eq!(selected.len(), 4);
    let personal_a = selected_context(&selected, "tenant-a", "personal");
    let administration_a = selected_context(&selected, "tenant-a", "administration");
    selected_context(&selected, "tenant-b", "personal");
    selected_context(&selected, "tenant-b", "administration");
    let alternate = contexts(&fixture, &fixture.alternate, 0).await;
    let alternate_a = selected_context(&alternate, "tenant-a", "personal");
    let a2 = contexts(&fixture, &fixture.tokens[1], 1).await;
    let a2 = selected_context(&a2, "tenant-a", "personal");
    let b1 = contexts(&fixture, &fixture.other_tenant, 0).await;
    let b1 = selected_context(&b1, "tenant-b", "personal");
    let mut held = retained(&fixture, &fixture.tokens[0], personal_a, 4).await;
    for (token, context) in [
        (&fixture.tokens[0], personal_a),
        (&fixture.tokens[0], administration_a),
        (&fixture.alternate, alternate_a),
    ] {
        let response = stream(&fixture, token, context).await;
        assert_eq!(
            response.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "a fifth live Body for the same tenant/principal must be refused before any poll"
        );
    }
    let other_human = retained(&fixture, &fixture.tokens[1], a2, 1).await;
    let other_tenant = retained(&fixture, &fixture.other_tenant, b1, 1).await;
    drop(held.pop());
    let replacement = retained(&fixture, &fixture.alternate, alternate_a, 1).await;
    assert_eq!(
        stream(&fixture, &fixture.tokens[0], personal_a)
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    drop((held, replacement, other_human, other_tenant));
    let _all_returned = retained(&fixture, &fixture.tokens[0], personal_a, 4).await;
}

// Emitting a terminal error must not return slots while the Body is still live.
#[tokio::test]
async fn retained_terminal_body_keeps_capacity_until_drop() {
    let fixture = Fixture::new();
    let selected = contexts(&fixture, &fixture.tokens[0], 0).await;
    let selected = selected_context(&selected, "tenant-a", "personal");
    let mut held = retained(&fixture, &fixture.tokens[0], selected, 4).await;
    let mut revoked = fixture.first_credential.clone();
    revoked.active = false;
    revoked.revision = 2;
    fixture
        .store
        .mutate_identity(&change(IdentityChangeV1::Credential {
            expected_revision: 1,
            binding: revoked,
        }))
        .unwrap();
    // The identity mutation changes authorization_revision. Obtain a current
    // context so this probe reaches stream admission rather than a stale-context refusal.
    let alternate = contexts(&fixture, &fixture.alternate, 0).await;
    let alternate = selected_context(&alternate, "tenant-a", "personal");
    let mut terminal_body = held.pop().unwrap().into_data_stream();
    let frame = tokio::time::timeout(Duration::from_secs(2), terminal_body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&frame).contains("identity_revoked"));
    assert!(!String::from_utf8_lossy(&frame).contains("human_audit"));
    assert_eq!(
        stream(&fixture, &fixture.alternate, alternate)
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(2), terminal_body.next())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        stream(&fixture, &fixture.alternate, alternate)
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS,
        "even EOF must not release capacity before the retained Body is dropped"
    );
    drop(terminal_body);
    let _replacement = retained(&fixture, &fixture.alternate, alternate, 1).await;
}

// A separate human sub-pool preserves eight of the shared tenant slots for control readers.
#[tokio::test]
async fn human_pool_preserves_eight_technical_stream_slots_and_rolls_back_failed_admission() {
    let fixture = Fixture::new();
    let mut selections = Vec::new();
    for (account, token) in fixture.tokens.iter().enumerate() {
        let available = contexts(&fixture, token, account).await;
        selections.push(selected_context(&available, "tenant-a", "personal").to_owned());
    }
    let mut humans = Vec::new();
    for (token, selected) in fixture.tokens.iter().zip(&selections).take(6) {
        humans.extend(retained(&fixture, token, selected, 4).await);
    }
    assert_eq!(
        stream(&fixture, &fixture.tokens[6], &selections[6])
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    let mut technical = Vec::new();
    for _ in 0..8 {
        let response = request(&fixture.router, &fixture.viewer, "/api/v1/stream", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        technical.push(response.into_body());
    }
    assert_eq!(
        request(&fixture.router, &fixture.viewer, "/api/v1/stream", None)
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(humans.pop());
    // 23 humans plus nine technical streams fill total32 while human24
    // still has room. Failure here must roll back both earlier acquisitions.
    let ninth = request(&fixture.router, &fixture.viewer, "/api/v1/stream", None).await;
    assert_eq!(ninth.status(), StatusCode::OK);
    technical.push(ninth.into_body());
    assert_eq!(
        stream(&fixture, &fixture.tokens[6], &selections[6])
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(technical.pop());
    let replacement = retained(&fixture, &fixture.tokens[6], &selections[6], 1).await;
    drop((humans, technical, replacement));
    // Fill human24 again, including all four slots of the previously refused
    // principal. Any principal/human/total permit leak must prevent this fill.
    let mut returned = Vec::new();
    for (token, selected) in fixture.tokens.iter().zip(&selections).skip(1) {
        returned.extend(retained(&fixture, token, selected, 4).await);
    }
    assert_eq!(returned.len(), 24);
    let mut returned_technical = Vec::new();
    for _ in 0..8 {
        let response = request(&fixture.router, &fixture.viewer, "/api/v1/stream", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        returned_technical.push(response.into_body());
    }
}
