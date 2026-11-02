//! Real local identity, Store and Axum behavior; no provider or container runtime.
mod support;
use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    identity::*,
    model::{CommandSpec, Decision, Event, Outcome},
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tower::ServiceExt;
use uuid::Uuid;

fn principal(id: &str) -> PrincipalRefV1 {
    PrincipalRefV1::HumanLocal {
        installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
        account_id: id.into(),
    }
}
fn operator() -> PrincipalRefV1 {
    PrincipalRefV1::LegacyControl {
        credential_id: "fixture-control-operator".into(),
        role: LegacyControlRoleV1::Runner,
    }
}
fn change(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: operator(),
        change,
    }
}
fn prepared(principal: PrincipalRefV1, tenant: &str, token: &str) -> LocalHumanProvisionV1 {
    LocalHumanProvisionV1 {
        version: 1,
        mutations: [
            change(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Named local reader".into(),
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
                    tenant_id: tenant.into(),
                    organization_scope: "organization-a".into(),
                    revision: 1,
                    active: true,
                    capabilities: BTreeSet::from([
                        HumanCapabilityV1::ReadMetadata,
                        HumanCapabilityV1::ReadContent,
                        HumanCapabilityV1::Export,
                    ]),
                },
            }),
        ],
    }
}
fn fresh(path: &std::path::Path, tenant: &str, capacity: u64) -> Arc<Store> {
    let store = Store::open_for_tenant(path, tenant, Redactor::new(vec![]), capacity).unwrap();
    store
        .migrate_control_v3(&path.with_extension("before-v3"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("before-v4"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("before-v5"))
        .unwrap();
    Arc::new(store)
}
struct Fixture {
    _files: tempfile::TempDir,
    a: Arc<Store>,
    b: Arc<Store>,
    router: Router,
    a1: String,
    a2: String,
    b1: String,
    runner_a: String,
    user_a1: PrincipalRefV1,
    user_a2: PrincipalRefV1,
    provision_a1: LocalHumanProvisionV1,
    event_a: String,
    event_b: String,
}
impl Fixture {
    fn new() -> Self {
        let files = support::private_tempdir();
        let a = fresh(&files.path().join("a.sqlite"), "tenant-a", 10000);
        let b = fresh(&files.path().join("b.sqlite"), "tenant-b", 10000);
        let a1 = generate_local_credential().unwrap();
        let a2 = generate_local_credential().unwrap();
        let b1 = generate_local_credential().unwrap();
        let user_a1 = principal("871be6f3-b1a5-4eb9-9c72-989cedb05c43");
        let user_a2 = principal("f7e62c9f-54bb-4e99-99b9-99902da33cd6");
        let user_b1 = principal("290a1a3b-8538-49ef-a830-25df6fa19030");
        let provision_a1 = prepared(user_a1.clone(), "tenant-a", &a1);
        a.provision_local_human(&provision_a1).unwrap();
        a.provision_local_human(&prepared(user_a2.clone(), "tenant-a", &a2))
            .unwrap();
        b.provision_local_human(&prepared(user_b1.clone(), "tenant-b", &b1))
            .unwrap();
        let mut event = Event::new("fixture.authorized");
        event.session_id = Some("session-a1".into());
        event.source = "https://private.example.test/free diagnostic".into();
        event.command = Some(CommandSpec {
            argv: vec!["/usr/bin/id".into()],
            cwd: "/workspace".into(),
        });
        event.decision = Some(Decision::new(
            Outcome::Allow,
            "fixture-rule",
            "private free-text decision reason",
        ));
        event.data = json!({"note":a1,"label":"authorized-a1"});
        let event_a = a.append(event).unwrap().event.id;
        for (store, user, session) in [
            (&a, &user_a1, "session-a1"),
            (&a, &user_a2, "session-a2"),
            (&b, &user_b1, "session-b1"),
        ] {
            if session != "session-a1" {
                let mut event = Event::new("fixture.authorized");
                event.session_id = Some(session.into());
                event.data = json!({"label":session});
                store.append(event).unwrap();
            }
            let shares = if session == "session-a1" {
                vec![ResourceShareV1 {
                    principal: user_a2.clone(),
                    permissions: BTreeSet::from([ResourcePermissionV1::Metadata]),
                }]
            } else {
                vec![]
            };
            store
                .mutate_identity(&change(IdentityChangeV1::ResourceGrant {
                    expected_revision: 0,
                    grant: ResourceGrantV1 {
                        version: 1,
                        tenant_id: store.tenant_id().into(),
                        organization_scope: "organization-a".into(),
                        resource: ResourceRefV1 {
                            kind: ResourceKindV1::Session,
                            resource_id: session.into(),
                        },
                        owner: user.clone(),
                        revision: 1,
                        active: true,
                        shares,
                    },
                }))
                .unwrap();
        }
        let mut event = Event::new("fixture.authorized");
        event.session_id = Some("session-a1".into());
        event.data = json!({"label":"authorized-a1-second"});
        a.append(event).unwrap();
        let event_b = b
            .events(&EventQuery {
                kind: Some("fixture.authorized".into()),
                ..Default::default()
            })
            .unwrap()[0]
            .event
            .id
            .clone();
        // A valid human-shaped technical key makes the later collision oracle
        // depend on class separation, rather than the human format guard.
        let runner_a = generate_local_credential().unwrap();
        let state = GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: Arc::clone(&a),
                    runner_token: runner_a.clone(),
                    viewer_token: "fixture-control-viewer-a".repeat(3),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: Arc::clone(&b),
                    runner_token: "fixture-control-runner-b".repeat(3),
                    viewer_token: "fixture-control-viewer-b".repeat(3),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap();
        Self {
            _files: files,
            a,
            b,
            router: gateway_router(state),
            a1,
            a2,
            b1,
            runner_a,
            user_a1,
            user_a2,
            provision_a1,
            event_a,
            event_b,
        }
    }
}
async fn get(
    router: &Router,
    token: &str,
    path: &str,
    context: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .uri(path)
        .header("authorization", format!("Bearer {token}"));
    if let Some(context) = context {
        request = request.header("x-aiguard-context", context);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap();
    (status, value)
}
async fn context(fixture: &Fixture, token: &str) -> String {
    let (status, identity) = get(&fixture.router, token, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    identity["contexts"][0]["id"].as_str().unwrap().to_owned()
}
fn page_path(path: &str, cursor: &str) -> String {
    let mut url = reqwest::Url::parse(&format!("http://127.0.0.1{path}")).unwrap();
    url.query_pairs_mut().append_pair("cursor", cursor);
    format!("{}?{}", url.path(), url.query().unwrap())
}

// Local human credentials authenticate their named account, never a technical role.
#[tokio::test]
async fn local_identity_is_named_and_legacy_classes_remain_separate() {
    let fixture = Fixture::new();
    let (status, identity) =
        get(&fixture.router, &fixture.a1, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        identity["principal"],
        serde_json::to_value(&fixture.user_a1).unwrap()
    );
    assert_eq!(identity["authentication"]["method"], "local_credential");
    assert_eq!(identity["contexts"][0]["tenant_id"], "tenant-a");
    let version = identity["contexts"][0]["authorization_version"]
        .as_str()
        .unwrap();
    assert!(
        version.len() == 64
            && version
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert!(
        identity["contexts"][0]
            .get("authorization_revision")
            .is_none()
    );
    assert!(
        identity["contexts"][0]["id"]
            .as_str()
            .is_some_and(|id| id.len() == 64 && !id.contains('.'))
    );
    let serialized = serde_json::to_string(&identity).unwrap();
    assert!(!serialized.contains(&fixture.a1));
    assert!(!serialized.contains("sha256_digest"));
    assert!(identity.get("role").is_none());
    assert_eq!(
        get(&fixture.router, &fixture.a1, "/api/v1/identity", None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(
            &fixture.router,
            &fixture.runner_a,
            "/api/v1/human/identity",
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, legacy) = get(&fixture.router, &fixture.runner_a, "/api/v1/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(legacy["role"], "runner");
}

// ACL filtering precedes LIMIT and has_more, including interleaved private events.
#[tokio::test]
async fn human_pages_filter_before_limit_and_cursors_cannot_change_identity_or_selection() {
    let fixture = Fixture::new();
    let context_a = context(&fixture, &fixture.a1).await;
    let context_b = context(&fixture, &fixture.b1).await;
    let (status, first) = get(
        &fixture.router,
        &fixture.a1,
        "/api/v1/human/events?limit=1",
        Some(&context_a),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["items"][0]["metadata"]["id"], fixture.event_a);
    assert_eq!(first["has_more"], true);
    let cursor = first["next_cursor"].as_str().unwrap();
    assert!(cursor.len() <= 2048);
    let path = page_path("/api/v1/human/events?limit=1", cursor);
    let (status, second) = get(&fixture.router, &fixture.a1, &path, Some(&context_a)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        second["items"][0]["metadata"]["resource"]["resource_id"],
        "session-a1"
    );
    assert_eq!(second["has_more"], false);
    assert!(second["next_cursor"].is_null());
    assert_eq!(
        get(&fixture.router, &fixture.b1, &path, Some(&context_b))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(
            &fixture.router,
            &fixture.a1,
            &page_path("/api/v1/human/export?limit=1", cursor),
            Some(&context_a)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(get(&fixture.router,&fixture.a1,"/api/v1/human/events?limit=1&cursor=0000000000000000000000000000000000000000000000000000000000000000",Some(&context_a)).await.0,StatusCode::BAD_REQUEST);
    assert_eq!(
        get(
            &fixture.router,
            &fixture.a1,
            "/api/v1/human/events?tenant_id=tenant-b",
            Some(&context_a)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status, resources) = get(
        &fixture.router,
        &fixture.a1,
        "/api/v1/human/resources?limit=1",
        Some(&context_a),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resources["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        resources["items"][0]["resource"]["resource_id"],
        "session-a1"
    );
    assert_eq!(resources["has_more"], false);
    for hidden in [
        "seq",
        "audit_seq",
        "head_hash",
        "sha256_digest",
        "authorization_revision",
    ] {
        assert!(
            !serde_json::to_string(&first)
                .unwrap()
                .contains(&format!("\"{hidden}\""))
        );
    }
}

// Long but valid owner principals must not overflow a complete resource page.
// Pagination has to retain every authorized resource exactly once at cutoff.
#[tokio::test]
async fn resource_page_full_envelope_is_bounded_without_skipping_long_owner_rows() {
    let fixture = Fixture::new();
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: "session-a1".into(),
    };
    let mut old = fixture
        .a
        .identity_resource_grant(&resource)
        .unwrap()
        .unwrap();
    old.active = false;
    old.revision = 2;
    fixture
        .a
        .mutate_identity(&change(IdentityChangeV1::ResourceGrant {
            expected_revision: 1,
            grant: old,
        }))
        .unwrap();
    let owner = PrincipalRefV1::HumanOidc {
        issuer: format!("https://oidc.example.test/{}", "x".repeat(2000)),
        subject: "s".repeat(512),
    };
    fixture
        .a
        .mutate_identity(&change(IdentityChangeV1::Account {
            expected_revision: 0,
            account: HumanAccountV1 {
                version: 1,
                principal: owner.clone(),
                display_name: "Owner fixture".into(),
                revision: 1,
                active: true,
            },
        }))
        .unwrap();
    fixture
        .a
        .mutate_identity(&change(IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: MembershipV1 {
                version: 1,
                membership_id: Uuid::new_v4().to_string(),
                principal: owner.clone(),
                tenant_id: "tenant-a".into(),
                organization_scope: "organization-a".into(),
                revision: 1,
                active: true,
                capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
            },
        }))
        .unwrap();
    for number in 0..100 {
        let id = format!("bulk-session-{number:03}");
        let mut event = Event::new("fixture.bulk");
        event.session_id = Some(id.clone());
        fixture.a.append(event).unwrap();
        fixture
            .a
            .mutate_identity(&change(IdentityChangeV1::ResourceGrant {
                expected_revision: 0,
                grant: ResourceGrantV1 {
                    version: 1,
                    tenant_id: "tenant-a".into(),
                    organization_scope: "organization-a".into(),
                    resource: ResourceRefV1 {
                        kind: ResourceKindV1::Session,
                        resource_id: id,
                    },
                    owner: owner.clone(),
                    revision: 1,
                    active: true,
                    shares: vec![ResourceShareV1 {
                        principal: fixture.user_a1.clone(),
                        permissions: BTreeSet::from([ResourcePermissionV1::Metadata]),
                    }],
                },
            }))
            .unwrap();
    }
    let selected = context(&fixture, &fixture.a1).await;
    let mut path = "/api/v1/human/resources?limit=100".to_owned();
    let mut seen = BTreeSet::new();
    let mut pages = 0;
    loop {
        let request = Request::builder()
            .uri(&path)
            .header("authorization", format!("Bearer {}", fixture.a1))
            .header("x-aiguard-context", &selected)
            .body(Body::empty())
            .unwrap();
        let response = fixture.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 512 * 1024).await.unwrap();
        assert!(
            bytes.len() <= HUMAN_MAX_PAGE_BYTES,
            "complete resource envelope exceeds its byte bound"
        );
        let page: Value = serde_json::from_slice(&bytes).unwrap();
        let items = page["items"].as_array().unwrap();
        assert!(!items.is_empty());
        for item in items {
            assert!(
                seen.insert(item["resource"]["resource_id"].as_str().unwrap().to_owned()),
                "duplicate resource across byte-limited pages"
            );
        }
        let request_id = page["read_receipt"]["request_id"].as_str().unwrap();
        let audit = fixture
            .a
            .events(&EventQuery {
                kind: Some("control.identity.access".into()),
                limit: Some(1000),
                ..Default::default()
            })
            .unwrap();
        assert!(
            audit
                .iter()
                .any(|e| e.event.data["request_id"] == request_id
                    && e.event.data["result_count"].as_u64()
                        == Some(u64::try_from(items.len()).unwrap()))
        );
        pages += 1;
        assert!(pages <= 4);
        match page["next_cursor"].as_str() {
            Some(cursor) => path = page_path("/api/v1/human/resources?limit=100", cursor),
            None => break,
        }
    }
    assert!(pages > 1, "fixture must exercise the byte cutoff");
    assert_eq!(seen.len(), 100);
    for number in 0..100 {
        assert!(seen.contains(&format!("bulk-session-{number:03}")));
    }
}

// Metadata excludes bodies/free text while scoped content and export need their own rights.
#[tokio::test]
async fn metadata_content_export_and_uniform_detail_denials_use_distinct_permissions() {
    let fixture = Fixture::new();
    let context_a = context(&fixture, &fixture.a1).await;
    let context_a2 = context(&fixture, &fixture.a2).await;
    let (status, metadata) = get(
        &fixture.router,
        &fixture.a2,
        "/api/v1/human/events?resource_kind=session&resource_id=session-a1",
        Some(&context_a2),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(metadata["items"][0]["content"].is_null());
    assert_eq!(metadata["items"][0]["metadata"]["source"], "unknown");
    let encoded = serde_json::to_string(&metadata).unwrap();
    assert!(!encoded.contains("private free-text"));
    assert!(!encoded.contains("/usr/bin/id"));
    assert!(!encoded.contains("private.example.test"));
    assert!(!encoded.contains("control.identity."));
    assert_eq!(
        get(
            &fixture.router,
            &fixture.a2,
            "/api/v1/human/events?view=content&resource_kind=session&resource_id=session-a1",
            Some(&context_a2)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(
            &fixture.router,
            &fixture.a2,
            "/api/v1/human/export?resource_kind=session&resource_id=session-a1",
            Some(&context_a2)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let detail = format!("/api/v1/human/events/{}?view=content", fixture.event_a);
    assert_eq!(
        get(&fixture.router, &fixture.a2, &detail, Some(&context_a2))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (status, content) = get(&fixture.router, &fixture.a1, &detail, Some(&context_a)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        content["items"][0]["content"]["command"]["argv"][0],
        "/usr/bin/id"
    );
    assert!(
        !serde_json::to_string(&content)
            .unwrap()
            .contains(&fixture.a1)
    );
    let foreign = format!("/api/v1/human/events/{}", fixture.event_b);
    assert_eq!(
        get(&fixture.router, &fixture.a1, &foreign, Some(&context_a))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (status, export) = get(
        &fixture.router,
        &fixture.a1,
        "/api/v1/human/export?view=content&limit=1",
        Some(&context_a),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(export["selection_complete"], false);
    let request_id = export["read_receipt"]["request_id"].as_str().unwrap();
    let audit = fixture
        .a
        .events(&EventQuery {
            kind: Some("control.identity.access".into()),
            limit: Some(1000),
            ..Default::default()
        })
        .unwrap();
    assert!(
        audit
            .iter()
            .any(|event| event.event.data["request_id"] == request_id
                && event.event.data["principal"]
                    == serde_json::to_value(&fixture.user_a1).unwrap())
    );
    assert!(!serde_json::to_string(&audit).unwrap().contains(&fixture.a1));
}

// Revoking a share or account invalidates server scopes; expired/revoked keys cannot authenticate.
#[tokio::test]
async fn revocation_expiry_and_account_withdrawal_are_checked_on_every_read() {
    let fixture = Fixture::new();
    let context_a2 = context(&fixture, &fixture.a2).await;
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: "session-a1".into(),
    };
    let mut grant = fixture
        .a
        .identity_resource_grant(&resource)
        .unwrap()
        .unwrap();
    grant.shares.clear();
    grant.revision = 2;
    fixture
        .a
        .mutate_identity(&change(IdentityChangeV1::ResourceGrant {
            expected_revision: 1,
            grant,
        }))
        .unwrap();
    let (status, failure) = get(
        &fixture.router,
        &fixture.a2,
        "/api/v1/human/events",
        Some(&context_a2),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(failure["error"]["code"], "context_revoked");
    let fresh = context(&fixture, &fixture.a2).await;
    let (_, page) = get(
        &fixture.router,
        &fixture.a2,
        "/api/v1/human/resources",
        Some(&fresh),
    )
    .await;
    assert!(
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["resource"]["resource_id"] != "session-a1")
    );
    let IdentityChangeV1::Credential { binding, .. } = &fixture.provision_a1.mutations[1].change
    else {
        unreachable!()
    };
    let mut expired = binding.clone();
    expired.revision = 2;
    expired.expires_at = Some("2000-01-01T00:00:00Z".into());
    fixture
        .a
        .mutate_identity(&change(IdentityChangeV1::Credential {
            expected_revision: 1,
            binding: expired,
        }))
        .unwrap();
    let (status, failure) = get(&fixture.router, &fixture.a1, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(failure["error"]["code"], "credential_expired");
    let mut account = fixture.a.human_account(&fixture.user_a2).unwrap().unwrap();
    account.active = false;
    account.revision = 2;
    fixture
        .a
        .mutate_identity(&change(IdentityChangeV1::Account {
            expected_revision: 1,
            account,
        }))
        .unwrap();
    assert_eq!(
        get(&fixture.router, &fixture.a2, "/api/v1/human/identity", None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

// Duplicate human bindings and human/control collisions must fail closed in both classes.
#[tokio::test]
async fn credential_collisions_between_tenants_and_classes_never_select_a_tenant() {
    let fixture = Fixture::new();
    fixture
        .b
        .provision_local_human(&prepared(fixture.user_a1.clone(), "tenant-b", &fixture.a1))
        .unwrap();
    assert_eq!(
        get(&fixture.router, &fixture.a1, "/api/v1/human/identity", None)
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let other = principal("12536929-30d8-4d53-8833-aa2912cc88d3");
    fixture
        .b
        .provision_local_human(&prepared(other, "tenant-b", &fixture.runner_a))
        .unwrap();
    assert_eq!(
        get(
            &fixture.router,
            &fixture.runner_a,
            "/api/v1/human/identity",
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&fixture.router, &fixture.runner_a, "/api/v1/identity", None)
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

// Administration exposes only explicitly permitted metadata; selecting its
// context cannot grant private content and never changes the personal view.
#[tokio::test]
async fn administration_is_an_explicit_metadata_scope_without_content_escalation() {
    let fixture = Fixture::new();
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: "session-a1".into(),
    };
    let mut grant = fixture
        .a
        .identity_resource_grant(&resource)
        .unwrap()
        .unwrap();
    grant.shares.clear();
    grant.revision = 2;
    fixture
        .a
        .mutate_identity(&change(IdentityChangeV1::ResourceGrant {
            expected_revision: 1,
            grant,
        }))
        .unwrap();
    let mut member = fixture
        .a
        .identity_memberships(&fixture.user_a2)
        .unwrap()
        .remove(0);
    member
        .capabilities
        .insert(HumanCapabilityV1::ReadTenantMetadata);
    member.revision = 2;
    fixture
        .a
        .mutate_identity(&change(IdentityChangeV1::Membership {
            expected_revision: 1,
            membership: member,
        }))
        .unwrap();
    let (status, identity) =
        get(&fixture.router, &fixture.a2, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    let contexts = identity["contexts"].as_array().unwrap();
    let personal = contexts.iter().find(|c| c["mode"] == "personal").unwrap();
    let administration = contexts
        .iter()
        .find(|c| c["mode"] == "administration")
        .unwrap();
    assert!(
        !personal["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "read_tenant_metadata")
    );
    let (_, personal_page) = get(
        &fixture.router,
        &fixture.a2,
        "/api/v1/human/resources",
        Some(personal["id"].as_str().unwrap()),
    )
    .await;
    assert_eq!(personal_page["items"].as_array().unwrap().len(), 1);
    let (status, admin_page) = get(
        &fixture.router,
        &fixture.a2,
        "/api/v1/human/resources",
        Some(administration["id"].as_str().unwrap()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(admin_page["items"].as_array().unwrap().len(), 2);
    let private = admin_page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["resource"]["resource_id"] == "session-a1")
        .unwrap();
    assert_eq!(private["permissions"]["metadata"], true);
    assert_eq!(private["permissions"]["content"], false);
    assert_eq!(private["permissions"]["export"], false);
    let path = format!("/api/v1/human/events/{}?view=content", fixture.event_a);
    assert_eq!(
        get(
            &fixture.router,
            &fixture.a2,
            &path,
            Some(administration["id"].as_str().unwrap())
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

// Stateless cursor branches preserve previous pages beyond the context quota.
#[tokio::test]
async fn stateless_pagination_exceeds_sixty_four_pages_and_keeps_previous_cursors() {
    let fixture = Fixture::new();
    for _ in 0..70 {
        let mut event = Event::new("fixture.authorized");
        event.session_id = Some("session-a1".into());
        fixture.a.append(event).unwrap();
    }
    let selected = context(&fixture, &fixture.a1).await;
    let mut path = "/api/v1/human/events?limit=1".to_owned();
    let mut first_cursor = None;
    let mut seen = BTreeSet::new();
    let mut pages = 0;
    loop {
        let (status, page) = get(&fixture.router, &fixture.a1, &path, Some(&selected)).await;
        assert_eq!(status, StatusCode::OK);
        let items = page["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert!(seen.insert(items[0]["metadata"]["id"].as_str().unwrap().to_owned()));
        pages += 1;
        assert!(pages <= 100);
        match page["next_cursor"].as_str() {
            Some(cursor) => {
                first_cursor.get_or_insert_with(|| cursor.to_owned());
                path = page_path("/api/v1/human/events?limit=1", cursor);
            }
            None => break,
        }
    }
    assert_eq!(pages, 72);
    assert_eq!(seen.len(), 72);
    let first_path = page_path(
        "/api/v1/human/events?limit=1",
        first_cursor.as_deref().unwrap(),
    );
    let (status, again) = get(&fixture.router, &fixture.a1, &first_path, Some(&selected)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["items"].as_array().unwrap().len(), 1);
}

// A lost response and a later child must leave the original encrypted snapshot
// usable. Fresh seals and read receipts do not claim receipt/download.
#[tokio::test]
async fn lost_page_response_preserves_snapshot_and_previous_cursor_until_expiry() {
    let fixture = Fixture::new();
    let mut extra = Event::new("fixture.authorized");
    extra.session_id = Some("session-a1".into());
    fixture.a.append(extra).unwrap();
    let selected = context(&fixture, &fixture.a1).await;
    let (_, first) = get(
        &fixture.router,
        &fixture.a1,
        "/api/v1/human/events?limit=1",
        Some(&selected),
    )
    .await;
    let parent = first["next_cursor"].as_str().unwrap();
    let parent_path = page_path("/api/v1/human/events?limit=1", parent);
    let (status, lost) = get(&fixture.router, &fixture.a1, &parent_path, Some(&selected)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, replayed) = get(&fixture.router, &fixture.a1, &parent_path, Some(&selected)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed["items"], lost["items"]);
    assert_ne!(replayed["next_cursor"], lost["next_cursor"]);
    assert_ne!(
        replayed["read_receipt"]["request_id"],
        lost["read_receipt"]["request_id"]
    );
    let child = replayed["next_cursor"].as_str().unwrap();
    let child_path = page_path("/api/v1/human/events?limit=1", child);
    let (status, last) = get(&fixture.router, &fixture.a1, &child_path, Some(&selected)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(last["has_more"], false);
    let (status, previous) = get(&fixture.router, &fixture.a1, &parent_path, Some(&selected)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(previous["items"], lost["items"]);
}

fn sse_record(bytes: &[u8]) -> (String, Value) {
    let text = std::str::from_utf8(bytes).unwrap();
    let id = text
        .lines()
        .find_map(|line| line.strip_prefix("id: "))
        .unwrap()
        .to_owned();
    let data = text
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap();
    (id, serde_json::from_str(data).unwrap())
}

// Reconnecting from the last complete id retains the authorized snapshot and
// resumes without a jump; a fresh nonce/receipt makes the new token distinct.
#[tokio::test]
async fn human_stream_reconnect_replays_lost_batch_from_last_complete_id() {
    let fixture = Fixture::new();
    let selected = context(&fixture, &fixture.a1).await;
    let request = |path: &str| {
        Request::builder()
            .uri(path)
            .header("authorization", format!("Bearer {}", fixture.a1))
            .header("x-aiguard-context", &selected)
            .body(Body::empty())
            .unwrap()
    };
    let response = fixture
        .router
        .clone()
        .oneshot(request("/api/v1/human/stream?limit=1"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(2), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (last_complete, _) = sse_record(&first);
    let lost = tokio::time::timeout(Duration::from_secs(2), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (lost_id, lost_page) = sse_record(&lost);
    drop(body);
    let path = page_path("/api/v1/human/stream?limit=1", &last_complete);
    let response = fixture
        .router
        .clone()
        .oneshot(request(&path))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut resumed = response.into_body().into_data_stream();
    let replayed = tokio::time::timeout(Duration::from_secs(2), resumed.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (replayed_id, replayed_page) = sse_record(&replayed);
    assert_ne!(replayed_id, lost_id);
    assert_eq!(replayed_page["items"], lost_page["items"]);
    assert!(
        !serde_json::to_string(&replayed_page)
            .unwrap()
            .contains(&fixture.a1)
    );
}

// A full mandatory audit rolls back the entire activation, not only its last row.
#[test]
fn local_provisioning_is_atomic_and_exact_replay_never_overwrites_an_account() {
    let files = support::private_tempdir();
    let path = files.path().join("atomic.sqlite");
    let store = fresh(&path, "tenant-a", 5);
    let token = generate_local_credential().unwrap();
    let principal = principal("871be6f3-b1a5-4eb9-9c72-989cedb05c43");
    let provision = prepared(principal.clone(), "tenant-a", &token);
    let head = store.head().unwrap();
    assert!(store.provision_local_human(&provision).is_err());
    assert_eq!(store.head().unwrap(), head);
    assert_eq!(store.identity_authorization_revision().unwrap(), 0);
    assert!(store.human_account(&principal).unwrap().is_none());
    assert!(store.identity_memberships(&principal).unwrap().is_empty());
    assert!(store.integrity().unwrap().valid);
    drop(store);
    let store =
        Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![token]), 1000).unwrap();
    let receipts = store.provision_local_human(&provision).unwrap();
    assert_eq!(receipts.len(), 3);
    let head = store.head().unwrap();
    assert_eq!(store.provision_local_human(&provision).unwrap(), receipts);
    assert_eq!(store.head().unwrap(), head);
    let mut conflict = provision.clone();
    if let IdentityChangeV1::Account { account, .. } = &mut conflict.mutations[0].change {
        account.display_name = "Unrelated replacement".into();
    }
    assert!(store.provision_local_human(&conflict).is_err());
    assert_eq!(store.head().unwrap(), head);
    assert!(store.integrity().unwrap().valid);
}

// Maintenance opening must not recreate missing or initialize empty data, and
// may never turn a newly opened4 database back into execution authority.
#[test]
fn maintenance_open_requires_existing_versioned_data_without_creation_or_adoption() {
    use std::os::unix::fs::OpenOptionsExt;
    let files = support::private_tempdir();
    let missing = files.path().join("missing.sqlite");
    assert!(
        Store::open_existing_for_tenant(&missing, "tenant-a", Redactor::new(vec![]), 1000).is_err()
    );
    assert!(!missing.exists());
    let empty = files.path().join("empty.sqlite");
    drop(
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&empty)
            .unwrap(),
    );
    assert!(
        Store::open_existing_for_tenant(&empty, "tenant-a", Redactor::new(vec![]), 1000).is_err()
    );
    assert!(std::fs::read(&empty).unwrap().is_empty());
    let path = files.path().join("existing.sqlite");
    drop(fresh(&path, "tenant-a", 1000));
    let opened =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert_eq!(opened.supervision_state().unwrap().schema_version, 5);
    assert!(!opened.supervision_state().unwrap().active);
    assert!(opened.integrity().unwrap().valid);
}

// A successful authentication cannot return identity data if its mandatory
// read audit cannot become durable.
#[tokio::test]
async fn unavailable_mandatory_read_audit_returns_error_instead_of_empty_identity() {
    let fixture = Fixture::new();
    let path = fixture._files.path().join("a.sqlite");
    let limit = u64::try_from(fixture.a.head().unwrap()).unwrap();
    let full = Arc::new(
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), limit).unwrap(),
    );
    let router = gateway_router(GatewayState::new(
        full,
        "other-control-runner".repeat(3),
        "other-control-viewer".repeat(3),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    ));
    let (status, error) = get(&router, &fixture.a1, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["error"]["code"], "audit_unavailable");
    assert!(error.get("contexts").is_none());
}

// The stream must terminate after a real durable credential withdrawal, not continue silently.
#[tokio::test]
async fn human_stream_stops_on_durable_credential_revocation() {
    let fixture = Fixture::new();
    let selected = context(&fixture, &fixture.a1).await;
    let request = Request::builder()
        .uri("/api/v1/human/stream?limit=1")
        .header("authorization", format!("Bearer {}", fixture.a1))
        .header("x-aiguard-context", &selected)
        .body(Body::empty())
        .unwrap();
    let response = fixture.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(2), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&first).contains("human_audit"));
    let IdentityChangeV1::Credential { binding, .. } = &fixture.provision_a1.mutations[1].change
    else {
        unreachable!()
    };
    let mut revoked = binding.clone();
    revoked.active = false;
    revoked.revision = 2;
    fixture
        .a
        .mutate_identity(&change(IdentityChangeV1::Credential {
            expected_revision: 1,
            binding: revoked,
        }))
        .unwrap();
    let terminal = tokio::time::timeout(Duration::from_secs(2), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let terminal = String::from_utf8_lossy(&terminal);
    assert!(terminal.contains("identity_revoked"));
    assert!(!terminal.contains(&fixture.a1));
    assert!(
        tokio::time::timeout(Duration::from_secs(2), body.next())
            .await
            .unwrap()
            .is_none()
    );
}

// The real HTTP presentation proxy must preserve human bearer/context guards
// and keep technical runner rejection on its historical viewer surface.
#[tokio::test]
async fn dashboard_proxy_forwards_human_context_without_viewer_fallback() {
    use agentsandbox::api::dashboard_router;
    struct Server(tokio::task::JoinHandle<()>);
    impl Drop for Server {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let fixture = Fixture::new();
    let selected = context(&fixture, &fixture.a1).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let gateway = fixture.router.clone();
    let _server = Server(tokio::spawn(async move {
        axum::serve(listener, gateway).await.unwrap();
    }));
    let front = fixture._files.path().join("front");
    std::fs::create_dir(&front).unwrap();
    std::fs::write(front.join("index.html"), "<title>test frontend</title>").unwrap();
    let proxy = dashboard_router(
        &format!("http://{address}"),
        "fixture-control-viewer-a".repeat(3),
        front,
    )
    .unwrap();
    let (status, identity) = get(&proxy, &fixture.a1, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        identity["principal"],
        serde_json::to_value(&fixture.user_a1).unwrap()
    );
    let (status, resources) = get(
        &proxy,
        &fixture.a1,
        "/api/v1/human/resources",
        Some(&selected),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resources["context_id"], selected);
    assert_eq!(
        get(&proxy, &fixture.a1, "/api/v1/human/resources", None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(
            &proxy,
            "fixture-control-viewer-a".repeat(3).as_str(),
            "/api/v1/human/resources",
            Some(&selected)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert!(
        !get(&proxy, &fixture.runner_a, "/api/v1/identity", None)
            .await
            .0
            .is_success()
    );
}

// Reacquisition after repeated authorization changes must never exhaust an
// eviction-free registry with contexts already proved revoked by Store.
#[tokio::test]
async fn authoritative_revalidation_after_more_than_sixty_four_changes_replaces_revoked_contexts() {
    let fixture = Fixture::new();
    let mut old_context = context(&fixture, &fixture.a1).await;
    let mut member = fixture
        .a
        .identity_memberships(&fixture.user_a1)
        .unwrap()
        .remove(0);
    let mut ids = BTreeSet::from([old_context.clone()]);
    let mut versions = BTreeSet::new();
    for _ in 0..70 {
        let previous_revision = member.revision;
        member.revision += 1;
        if member
            .capabilities
            .contains(&HumanCapabilityV1::ReadContent)
        {
            member.capabilities.remove(&HumanCapabilityV1::ReadContent);
        } else {
            member.capabilities.insert(HumanCapabilityV1::ReadContent);
        }
        fixture
            .a
            .mutate_identity(&change(IdentityChangeV1::Membership {
                expected_revision: previous_revision,
                membership: member.clone(),
            }))
            .unwrap();
        let (status, identity) =
            get(&fixture.router, &fixture.a1, "/api/v1/human/identity", None).await;
        assert_eq!(status, StatusCode::OK);
        let next = identity["contexts"][0]["id"].as_str().unwrap().to_owned();
        let version = identity["contexts"][0]["authorization_version"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(ids.insert(next.clone()));
        assert!(versions.insert(version));
        assert_eq!(
            get(
                &fixture.router,
                &fixture.a1,
                "/api/v1/human/resources",
                Some(&old_context)
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            get(
                &fixture.router,
                &fixture.a1,
                "/api/v1/human/resources",
                Some(&next)
            )
            .await
            .0,
            StatusCode::OK
        );
        old_context = next;
    }
    let (_, unchanged) = get(&fixture.router, &fixture.a1, "/api/v1/human/identity", None).await;
    assert_eq!(unchanged["contexts"][0]["id"], old_context);
}

// A newly initialized gateway cannot accept another registry's contexts or
// cursors, even when the durable human account and database are unchanged.
#[tokio::test]
async fn new_gateway_registry_rejects_old_contexts_and_cross_context_cursor_tags() {
    let fixture = Fixture::new();
    let old_context = context(&fixture, &fixture.a1).await;
    let (_, page) = get(
        &fixture.router,
        &fixture.a1,
        "/api/v1/human/events?limit=1",
        Some(&old_context),
    )
    .await;
    let cursor = page["next_cursor"].as_str().unwrap();
    let restarted = gateway_router(GatewayState::new(
        Arc::clone(&fixture.a),
        fixture.runner_a.clone(),
        "new-viewer-control-fixture".repeat(3),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    ));
    assert_eq!(
        get(
            &restarted,
            &fixture.a1,
            "/api/v1/human/resources",
            Some(&old_context)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, identity) = get(&restarted, &fixture.a1, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    let new_context = identity["contexts"][0]["id"].as_str().unwrap();
    assert_ne!(new_context, old_context);
    let (status, error) = get(
        &restarted,
        &fixture.a1,
        &page_path("/api/v1/human/events?limit=1", cursor),
        Some(new_context),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["error"]["code"], "invalid_cursor");
}

// More than64 SSE batches must retain their authenticated snapshot and resume
// from the last complete frame, without treating a retry as a new authority.
#[tokio::test]
async fn human_stream_exceeds_sixty_four_batches_and_resumes_without_skipping_new_events() {
    async fn next_complete_frame<S>(body: &mut S, pending: &mut Vec<u8>) -> (String, Value)
    where
        S: futures_util::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin,
    {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(end) = pending.windows(2).position(|bytes| bytes == b"\n\n") {
                    let frame = pending.drain(..end + 2).collect::<Vec<_>>();
                    let text = std::str::from_utf8(&frame).unwrap();
                    if text.starts_with(':') {
                        continue;
                    }
                    assert!(
                        text.lines().any(|line| line == "event: human_audit"),
                        "SSE terminated before the expected authorized batch"
                    );
                    // Do not expose an ID to the caller until the complete
                    // frame and its JSON have both been parsed successfully.
                    return sse_record(&frame);
                }
                let bytes = body
                    .next()
                    .await
                    .expect("SSE ended before the expected authorized batch")
                    .expect("SSE body failed before the expected authorized batch");
                pending.extend_from_slice(&bytes);
                assert!(
                    pending.len() <= HUMAN_MAX_PAGE_BYTES * 2,
                    "SSE frame buffering exceeded its test bound"
                );
            }
        })
        .await
        .expect("SSE made no progress toward the expected authorized batch")
    }

    tokio::time::timeout(Duration::from_secs(30), async {
        let fixture = Fixture::new();
        // The fixture already has two visible A1 events. Seventy distinct
        // events require seventy limit1 business batches in this context.
        for _ in 0..68 {
            let mut event = Event::new("fixture.authorized");
            event.session_id = Some("session-a1".into());
            fixture.a.append(event).unwrap();
        }
        let expected = fixture
            .a
            .events(&EventQuery {
                kind: Some("fixture.authorized".into()),
                order: Some("asc".into()),
                limit: Some(1000),
                ..Default::default()
            })
            .unwrap()
            .into_iter()
            .filter(|stored| stored.event.session_id.as_deref() == Some("session-a1"))
            .map(|stored| stored.event.id)
            .collect::<Vec<_>>();
        assert_eq!(expected.len(), 70);
        let (status, identity) =
            get(&fixture.router, &fixture.a1, "/api/v1/human/identity", None).await;
        assert_eq!(status, StatusCode::OK);
        let selected = identity["contexts"][0]["id"].as_str().unwrap();
        let authorization_version = identity["contexts"][0]["authorization_version"]
            .as_str()
            .unwrap();
        let request = |path: &str| {
            Request::builder()
                .uri(path)
                .header("authorization", format!("Bearer {}", fixture.a1))
                .header("x-aiguard-context", selected)
                .body(Body::empty())
                .unwrap()
        };
        let inspect_page = |resume: &str, page: &Value| {
            assert_eq!(resume.len(), 1082);
            assert!(
                resume
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            );
            assert_eq!(page["context_id"].as_str(), Some(selected));
            assert_eq!(
                page["authorization_version"].as_str(),
                Some(authorization_version)
            );
            assert_eq!(page["version"], 1);
            assert_eq!(page["coverage"], "authorized_selection");
            assert!(page["next_cursor"].is_null());
            assert_eq!(page["items"].as_array().unwrap().len(), 1);
            assert!(page.get("authorization_revision").is_none());
            (
                page["items"][0]["metadata"]["id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                (
                    page["read_receipt"]["request_id"]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                    page["read_receipt"]["audit_event_id"]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                ),
            )
        };
        let response = fixture
            .router
            .clone()
            .oneshot(request("/api/v1/human/stream?limit=1"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body().into_data_stream();
        let mut pending = Vec::new();
        let mut last_complete = String::new();
        let mut seen = BTreeSet::new();
        let mut receipts = BTreeSet::new();
        for expected_id in &expected[..65] {
            let (resume, page) = next_complete_frame(&mut body, &mut pending).await;
            let (event_id, receipt) = inspect_page(&resume, &page);
            assert_eq!(&event_id, expected_id);
            assert!(seen.insert(event_id));
            assert!(receipts.insert(receipt));
            assert_eq!(page["has_more"], true);
            last_complete = resume;
        }
        // Observe a complete66th frame but deliberately retain the65th
        // checkpoint to represent a response lost before application commit.
        let (lost_resume, lost_page) = next_complete_frame(&mut body, &mut pending).await;
        let (lost_event, lost_receipt) = inspect_page(&lost_resume, &lost_page);
        assert_eq!(lost_event, expected[65]);
        assert!(seen.insert(lost_event));
        assert!(receipts.insert(lost_receipt));
        drop(body);
        let mut event = Event::new("fixture.authorized");
        event.session_id = Some("session-a1".into());
        let during_drain = fixture.a.append(event).unwrap().event.id;
        let path = page_path("/api/v1/human/stream?limit=1", &last_complete);
        let response = fixture
            .router
            .clone()
            .oneshot(request(&path))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body().into_data_stream();
        pending.clear();
        for (offset, expected_id) in expected[65..].iter().enumerate() {
            let (resume, page) = next_complete_frame(&mut body, &mut pending).await;
            let (event_id, receipt) = inspect_page(&resume, &page);
            assert_eq!(&event_id, expected_id);
            // The lost66th batch is explicitly allowed to appear twice. All
            // other IDs must be new; duplicate acceptance cannot hide a gap.
            assert_eq!(seen.insert(event_id), offset != 0);
            assert!(receipts.insert(receipt));
            if offset == 0 {
                assert_eq!(page["items"], lost_page["items"]);
                assert_ne!(resume, lost_resume);
            }
            assert_eq!(page["has_more"], offset != 4);
            last_complete = resume;
        }
        assert_eq!(seen, expected.iter().cloned().collect::<BTreeSet<_>>());
        drop(body);
        let mut event = Event::new("fixture.authorized");
        event.session_id = Some("session-a1".into());
        let between_drains = fixture.a.append(event).unwrap().event.id;
        // The previous authenticated snapshot is drained. The last complete
        // ID must now open the newer snapshot, including both appended events.
        let path = page_path("/api/v1/human/stream?limit=1", &last_complete);
        let response = fixture
            .router
            .clone()
            .oneshot(request(&path))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body().into_data_stream();
        pending.clear();
        for (offset, expected_id) in [during_drain, between_drains].iter().enumerate() {
            let (resume, page) = next_complete_frame(&mut body, &mut pending).await;
            let (event_id, receipt) = inspect_page(&resume, &page);
            assert_eq!(&event_id, expected_id);
            assert!(seen.insert(event_id));
            assert!(receipts.insert(receipt));
            assert_eq!(page["has_more"], offset == 0);
        }
        drop(body);
        assert_eq!(seen.len(), 72);
        assert_eq!(receipts.len(), 73);
        let audited = fixture
            .a
            .events(&EventQuery {
                kind: Some("control.identity.access".into()),
                limit: Some(1000),
                ..Default::default()
            })
            .unwrap()
            .into_iter()
            .filter(|stored| stored.event.data["action"] == "stream")
            .map(|stored| {
                assert_eq!(stored.event.data["result_count"], 1);
                assert_eq!(
                    stored.event.data["principal"],
                    serde_json::to_value(&fixture.user_a1).unwrap()
                );
                (
                    stored.event.data["request_id"].as_str().unwrap().to_owned(),
                    stored.event.id,
                )
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(audited, receipts);
    })
    .await
    .expect("SSE traversal exceeded its total progression budget");
}

#[test]
fn capability_ceiling_survives_store_revalidation_and_cannot_be_widened() {
    let f = Fixture::new();
    let IdentityChangeV1::Membership { membership, .. } = &f.provision_a1.mutations[2].change
    else {
        panic!("fixture membership");
    };
    let scope =
        f.a.authorize_human_context(
            &f.user_a1,
            &membership.membership_id,
            membership.revision,
            ContextModeV1::Personal,
        )
        .unwrap();
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: "session-a1".into(),
    };
    assert!(
        f.a.authorized_resource_grant(&scope, &resource, ResourcePermissionV1::Content,)
            .unwrap()
            .is_some(),
        "positive control: owner has content rights"
    );
    let restricted = scope
        .restrict_capabilities(BTreeSet::from([HumanCapabilityV1::ReadMetadata]))
        .unwrap();
    assert!(restricted.has_capability(HumanCapabilityV1::ReadMetadata));
    assert!(!restricted.has_capability(HumanCapabilityV1::ReadContent));
    assert!(
        f.a.authorized_resource_grant(&restricted, &resource, ResourcePermissionV1::Metadata,)
            .unwrap()
            .is_some()
    );
    for permission in [ResourcePermissionV1::Content, ResourcePermissionV1::Export] {
        assert!(
            f.a.authorized_resource_grant(&restricted, &resource, permission,)
                .unwrap()
                .is_none(),
            "transaction must retain the ceiling"
        );
    }
    let restricted_again = restricted
        .restrict_capabilities(BTreeSet::from([
            HumanCapabilityV1::ReadMetadata,
            HumanCapabilityV1::ReadContent,
            HumanCapabilityV1::Export,
            HumanCapabilityV1::ReadTenantMetadata,
        ]))
        .unwrap();
    assert!(!restricted_again.has_capability(HumanCapabilityV1::ReadContent));
    assert!(!restricted_again.has_capability(HumanCapabilityV1::ReadTenantMetadata));
    assert!(
        f.a.authorized_resource_grant(&restricted_again, &resource, ResourcePermissionV1::Content,)
            .unwrap()
            .is_none(),
        "second restriction cannot remove the first"
    );
}

#[test]
fn capability_ceiling_refuses_a_context_with_no_remaining_capability() {
    let f = Fixture::new();
    let IdentityChangeV1::Membership { membership, .. } = &f.provision_a1.mutations[2].change
    else {
        panic!("fixture membership");
    };
    let scope =
        f.a.authorize_human_context(
            &f.user_a1,
            &membership.membership_id,
            membership.revision,
            ContextModeV1::Personal,
        )
        .unwrap();
    assert!(scope.has_capability(HumanCapabilityV1::ReadMetadata));
    assert_eq!(
        scope.restrict_capabilities(BTreeSet::new()).unwrap_err(),
        IdentityFailureV1::PermissionDenied,
    );
}

#[test]
fn capability_ceiling_refuses_administration_when_mapping_removes_it() {
    let f = Fixture::new();
    let IdentityChangeV1::Membership { membership, .. } = &f.provision_a1.mutations[2].change
    else {
        panic!("fixture membership");
    };
    let mut administrative = membership.clone();
    administrative.revision += 1;
    administrative
        .capabilities
        .insert(HumanCapabilityV1::ReadTenantMetadata);
    f.a.mutate_identity(&change(IdentityChangeV1::Membership {
        expected_revision: membership.revision,
        membership: administrative.clone(),
    }))
    .unwrap();
    let scope =
        f.a.authorize_human_context(
            &f.user_a1,
            &administrative.membership_id,
            administrative.revision,
            ContextModeV1::Administration,
        )
        .unwrap();
    assert!(scope.has_capability(HumanCapabilityV1::ReadTenantMetadata));
    assert_eq!(
        scope
            .restrict_capabilities(BTreeSet::from([HumanCapabilityV1::ReadMetadata,]))
            .unwrap_err(),
        IdentityFailureV1::PermissionDenied,
        "admin context cannot survive removal of its admin ceiling",
    );
}

#[test]
fn capability_ceiling_is_required_for_oidc_before_transactional_resource_reads() {
    let f = Fixture::new();
    let oidc = PrincipalRefV1::HumanOidc {
        issuer: "https://identity.example.test/realm".into(),
        subject: "fixture-verified-subject".into(),
    };
    f.a.mutate_identity(&change(IdentityChangeV1::Account {
        expected_revision: 0,
        account: HumanAccountV1 {
            version: 1,
            principal: oidc.clone(),
            display_name: "OIDC fixture reader".into(),
            revision: 1,
            active: true,
        },
    }))
    .unwrap();
    let membership = MembershipV1 {
        version: 1,
        membership_id: Uuid::new_v4().to_string(),
        principal: oidc.clone(),
        tenant_id: "tenant-a".into(),
        organization_scope: "organization-a".into(),
        revision: 1,
        active: true,
        capabilities: BTreeSet::from([
            HumanCapabilityV1::ReadMetadata,
            HumanCapabilityV1::ReadContent,
            HumanCapabilityV1::Export,
        ]),
    };
    f.a.mutate_identity(&change(IdentityChangeV1::Membership {
        expected_revision: 0,
        membership: membership.clone(),
    }))
    .unwrap();
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: "session-a1".into(),
    };
    let mut grant = f.a.identity_resource_grant(&resource).unwrap().unwrap();
    let expected_revision = grant.revision;
    grant.revision += 1;
    grant.shares.push(ResourceShareV1 {
        principal: oidc.clone(),
        permissions: BTreeSet::from([
            ResourcePermissionV1::Metadata,
            ResourcePermissionV1::Content,
            ResourcePermissionV1::Export,
        ]),
    });
    f.a.mutate_identity(&change(IdentityChangeV1::ResourceGrant {
        expected_revision,
        grant,
    }))
    .unwrap();
    let scope =
        f.a.authorize_human_context(
            &oidc,
            &membership.membership_id,
            membership.revision,
            ContextModeV1::Personal,
        )
        .unwrap();
    assert!(scope.has_capability(HumanCapabilityV1::ReadContent));
    let error =
        f.a.authorized_resource_grant(&scope, &resource, ResourcePermissionV1::Metadata)
            .unwrap_err();
    assert_eq!(
        error.downcast_ref::<IdentityFailureV1>(),
        Some(&IdentityFailureV1::PermissionDenied),
        "OIDC requires an explicit ceiling even when a durable share exists"
    );
    let restricted = scope
        .restrict_capabilities(BTreeSet::from([HumanCapabilityV1::ReadMetadata]))
        .unwrap();
    assert!(
        f.a.authorized_resource_grant(&restricted, &resource, ResourcePermissionV1::Metadata)
            .unwrap()
            .is_some(),
        "positive control: the explicit ceiling permits the existing share"
    );
    assert!(
        f.a.authorized_resource_grant(&restricted, &resource, ResourcePermissionV1::Content)
            .unwrap()
            .is_none()
    );
}
