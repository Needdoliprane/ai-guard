//! Human reads use the actual router, private SQLite files and local credentials.
mod support;
use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    broker_registry::*,
    identity::*,
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    process::{Command, Stdio},
    sync::Arc,
};
use uuid::Uuid;

fn id() -> String {
    Uuid::new_v4().to_string()
}
fn operator() -> PrincipalRefV1 {
    PrincipalRefV1::LegacyControl {
        credential_id: "trusted-fixture-control".into(),
        role: LegacyControlRoleV1::Runner,
    }
}
fn change(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: id(),
        actor: operator(),
        change,
    }
}
fn cap(value: &str) -> HumanCapabilityV1 {
    serde_json::from_value(json!(value)).unwrap()
}
fn migrate7(path: &std::path::Path) {
    let result = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["migrate-control", "--data"])
        .arg(path)
        .args(["--tenant-id", "tenant-a", "--backup"])
        .arg(path.with_extension("v6.bak"))
        .args([
            "--target-version",
            "7",
            "--confirm-control-stopped",
            "--max-events",
            "10000",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "migration7: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}
fn store(path: &std::path::Path, tenant: &str) -> Store {
    store_at(path, tenant, 6)
}
fn store_at(path: &std::path::Path, tenant: &str, version: u32) -> Store {
    let store = Store::open_for_tenant(path, tenant, Redactor::new(vec![]), 10000).unwrap();
    store
        .migrate_control_v3(&path.with_extension("v2.bak"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("v3.bak"))
        .unwrap();
    if version >= 5 {
        store
            .migrate_human_access_v5(&path.with_extension("v4.bak"))
            .unwrap();
    }
    if version >= 6 {
        store
            .migrate_workload_v6(&path.with_extension("v5.bak"))
            .unwrap();
    }
    store
}
fn provision(
    store: &Store,
    token: &str,
    caps: &[HumanCapabilityV1],
) -> (PrincipalRefV1, MembershipV1) {
    let (prepared, principal, membership) = preparation(store, token, caps);
    store.provision_local_human(&prepared).unwrap();
    (principal, membership)
}
fn preparation(
    store: &Store,
    token: &str,
    caps: &[HumanCapabilityV1],
) -> (LocalHumanProvisionV1, PrincipalRefV1, MembershipV1) {
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
        account_id: id(),
    };
    let membership = MembershipV1 {
        version: 1,
        membership_id: id(),
        principal: principal.clone(),
        tenant_id: store.tenant_id().into(),
        organization_scope: "organization-a".into(),
        revision: 1,
        active: true,
        capabilities: caps.iter().copied().collect(),
    };
    let prepared = LocalHumanProvisionV1 {
        version: 1,
        mutations: [
            change(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Named broker reader".into(),
                    revision: 1,
                    active: true,
                },
            }),
            change(IdentityChangeV1::Credential {
                expected_revision: 0,
                binding: LocalCredentialV1 {
                    version: 1,
                    credential_id: id(),
                    principal: principal.clone(),
                    sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                    revision: 1,
                    active: true,
                    expires_at: None,
                },
            }),
            change(IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: membership.clone(),
            }),
        ],
    };
    (prepared, principal, membership)
}
fn grant(store: &Store, authority: &str) -> BrokerGrantMutationV1 {
    let grant = BrokerGrantMutationV1 {
        version: 1,
        operation_id: id(),
        broker_id: id(),
        expected_revision: None,
        credential_id: id(),
        incarnation_id: id(),
        authorities: vec![BrokerAuthorityV1 {
            authority_id: authority.into(),
            runtime_kind: BrokerRuntimeKindV1::Docker,
        }],
        lease_seconds: 30,
        not_after: "2100-01-01T00:00:00.000Z".into(),
    };
    store.mutate_broker_grant(&grant).unwrap();
    grant
}
fn router(a: &Arc<Store>, b: &Arc<Store>) -> Router {
    gateway_router(
        GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: Arc::clone(a),
                    runner_token: "fixture-control-runner-a".repeat(3),
                    viewer_token: "fixture-control-viewer-a".repeat(3),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: Arc::clone(b),
                    runner_token: "fixture-control-runner-b".repeat(3),
                    viewer_token: "fixture-control-viewer-b".repeat(3),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap(),
    )
}
async fn get(
    router: &Router,
    token: &str,
    path: &str,
    context: Option<&str>,
) -> (StatusCode, Value) {
    use tower::ServiceExt;
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
    let bytes = to_bytes(response.into_body(), 300000).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
fn context(identity: &Value, mode: &str) -> String {
    identity["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["mode"] == mode)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .into()
}

#[test]
fn schema7_only_capability_provisioning_is_rejected_atomically_on4_5_6() {
    for version in [4, 5, 6] {
        let files = support::private_tempdir();
        let path = files.path().join("a.sqlite");
        let a = store_at(&path, "tenant-a", version);
        let token = generate_local_credential().unwrap();
        let (prepared, principal, _) = preparation(&a, &token, &[cap("read_broker_registry")]);
        let before = a.integrity().unwrap();
        let error = a
            .provision_local_human(&prepared)
            .expect_err("schema7-only provision was accepted on historical schema");
        assert_eq!(
            error.downcast_ref::<IdentityFailureV1>(),
            Some(&IdentityFailureV1::MigrationRequired)
        );
        assert_eq!(
            a.mutate_identity(&prepared.mutations[2])
                .unwrap_err()
                .downcast_ref::<IdentityFailureV1>(),
            Some(&IdentityFailureV1::MigrationRequired)
        );
        assert_eq!(a.integrity().unwrap().head_hash, before.head_hash);
        assert!(a.human_account(&principal).unwrap().is_none());
        assert!(a.identity_memberships(&principal).unwrap().is_empty());
        let db = rusqlite::Connection::open(&path).unwrap();
        let (revision, operations): (i64, i64) = db.query_row("SELECT authorization_revision,(SELECT COUNT(*) FROM identity_operations) FROM identity_meta", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!((revision, operations), (0, 0));
        drop(db);
        drop(a);
        let reopened =
            Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10000)
                .unwrap();
        assert!(reopened.integrity().unwrap().valid);
        assert_eq!(reopened.integrity().unwrap().head_hash, before.head_hash);
    }
}

#[tokio::test]
async fn full_broker_response_frame_is_mask_checked_before_read_commit() {
    for canary in ["tenant_broker_registry", "runtime_inventory"] {
        let files = support::private_tempdir();
        let path = files.path().join("a.sqlite");
        drop(store(&path, "tenant-a"));
        migrate7(&path);
        let a = Arc::new(
            Store::open_existing_for_tenant(
                &path,
                "tenant-a",
                Redactor::new(vec![canary.into()]),
                10000,
            )
            .unwrap(),
        );
        let b = Arc::new(store(&files.path().join("b.sqlite"), "tenant-b"));
        let token = generate_local_credential().unwrap();
        provision(&a, &token, &[cap("read_broker_registry")]);
        let app = router(&a, &b);
        let (status, identity) = get(&app, &token, "/api/v1/human/identity", None).await;
        assert_eq!(status, StatusCode::OK);
        let selected = context(&identity, "administration");
        let before = a.integrity().unwrap();
        let (status, body) = get(&app, &token, "/api/v1/human/brokers", Some(&selected)).await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "frame canary was emitted"
        );
        assert_eq!(body["error"]["code"], "audit_unavailable");
        assert!(!body.to_string().contains(canary));
        assert_eq!(a.integrity().unwrap().head_hash, before.head_hash);
        assert!(
            a.events(&EventQuery {
                kind: Some("control.identity.brokers.access".into()),
                ..Default::default()
            })
            .unwrap()
            .is_empty()
        );
    }
}

#[tokio::test]
async fn broker_authority_redaction_is_explicit_and_required_ids_fail_closed() {
    for required_id in [false, true] {
        let files = support::private_tempdir();
        let path = files.path().join("a.sqlite");
        drop(store(&path, "tenant-a"));
        migrate7(&path);
        let clean =
            Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10000)
                .unwrap();
        let token = generate_local_credential().unwrap();
        provision(&clean, &token, &[cap("read_broker_registry")]);
        let broker = grant(&clean, "authority-frame-canary");
        let canary = if required_id {
            broker.broker_id.as_str()
        } else {
            "authority-frame-canary"
        };
        drop(clean);
        let a = Arc::new(
            Store::open_existing_for_tenant(
                &path,
                "tenant-a",
                Redactor::new(vec![canary.into()]),
                10000,
            )
            .unwrap(),
        );
        let b = Arc::new(store(&files.path().join("b.sqlite"), "tenant-b"));
        let app = router(&a, &b);
        let (status, identity) = get(&app, &token, "/api/v1/human/identity", None).await;
        assert_eq!(status, StatusCode::OK);
        let before = a.head().unwrap();
        let (status, body) = get(
            &app,
            &token,
            "/api/v1/human/brokers",
            Some(&context(&identity, "administration")),
        )
        .await;
        assert!(!body.to_string().contains(canary));
        if required_id {
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(body["error"]["code"], "audit_unavailable");
            assert_eq!(a.head().unwrap(), before);
        } else {
            assert_eq!(status, StatusCode::OK);
            assert!(body["items"][0]["authorities"][0]["authority_id"].is_null());
            assert_eq!(
                body["items"][0]["authorities"][0]["authority_id_redacted"],
                true
            );
            assert_eq!(a.head().unwrap(), before + 1);
        }
    }
}

#[tokio::test]
async fn schema6_human_broker_read_requires_explicit_migration() {
    let files = support::private_tempdir();
    let a = Arc::new(store(&files.path().join("a.sqlite"), "tenant-a"));
    let b = Arc::new(store(&files.path().join("b.sqlite"), "tenant-b"));
    let token = generate_local_credential().unwrap();
    provision(&a, &token, &[HumanCapabilityV1::ReadTenantMetadata]);
    let app = router(&a, &b);
    let (status, identity) = get(&app, &token, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    let head = a.head().unwrap();
    let (status, error) = get(
        &app,
        &token,
        "/api/v1/human/brokers",
        Some(&context(&identity, "administration")),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["error"]["code"], "migration_required");
    assert_eq!(a.head().unwrap(), head);
}

#[tokio::test]
async fn explicit_broker_only_capability_reads_own_tenant_and_closed_projection() {
    let files = support::private_tempdir();
    let path = files.path().join("a.sqlite");
    drop(store(&path, "tenant-a"));
    migrate7(&path);
    let a = Arc::new(
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10000).unwrap(),
    );
    let b = Arc::new(store(&files.path().join("b.sqlite"), "tenant-b"));
    let token = generate_local_credential().unwrap();
    let (principal, _) = provision(&a, &token, &[cap("read_broker_registry")]);
    let first = grant(&a, "authority-a:docker");
    let second = grant(&a, "authority-a:podman");
    let foreign = grant(&b, "authority-b:docker");
    let app = router(&a, &b);
    let (status, identity) = get(&app, &token, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    let selected = context(&identity, "administration");
    let (status, page) = get(
        &app,
        &token,
        "/api/v1/human/brokers?limit=1",
        Some(&selected),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["source"], "tenant_broker_registry");
    assert_eq!(page["consistency"], "live_keyset");
    assert_eq!(page["runtime_inventory"], "unknown");
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["has_more"], true);
    let cursor = page["next_cursor"].as_str().unwrap();
    assert_eq!(cursor.len(), 1082);
    let (status, next) = get(
        &app,
        &token,
        &format!("/api/v1/human/brokers?limit=1&cursor={cursor}"),
        Some(&selected),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(next["has_more"], false);
    assert!(next["next_cursor"].is_null());
    assert_ne!(page["items"][0]["broker_id"], next["items"][0]["broker_id"]);
    assert_ne!(
        page["read_receipt"]["request_id"],
        next["read_receipt"]["request_id"]
    );
    let wire = format!("{page}{next}");
    for forbidden in [
        &first.credential_id,
        &second.credential_id,
        &foreign.broker_id,
        "credential_id",
        "input_hash",
        "runtime_inventory_complete",
    ] {
        assert!(!wire.contains(forbidden));
    }
    assert_eq!(page["items"][0]["grant_revision"], "1");
    for path in [
        "/api/v1/human/resources",
        "/api/v1/human/events",
        "/api/v1/human/export",
    ] {
        assert_eq!(
            get(&app, &token, path, Some(&selected)).await.0,
            StatusCode::FORBIDDEN,
            "unexpected permission response from {path}"
        );
    }
    // The existing SSE protocol opens HTTP200 and carries its first read
    // refusal in an error frame, then terminates. JSON routes above use403.
    use tower::ServiceExt;
    let before_stream = a.head().unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/human/stream")
                .header("authorization", format!("Bearer {token}"))
                .header("x-aiguard-context", &selected)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        to_bytes(response.into_body(), 2048),
    )
    .await
    .expect("denied stream did not terminate")
    .unwrap();
    let stream = std::str::from_utf8(&bytes).unwrap();
    let frames: Vec<_> = stream
        .split("\n\n")
        .filter(|frame| !frame.is_empty())
        .collect();
    assert_eq!(frames.len(), 1);
    assert!(frames[0].starts_with("event: error\n"));
    let error: Value =
        serde_json::from_str(frames[0].strip_prefix("event: error\ndata: ").unwrap()).unwrap();
    assert_eq!(error["error"]["code"], "permission_denied");
    assert_eq!(error["error"]["retry_action"], "review_permissions");
    assert_eq!(error.as_object().unwrap().len(), 1);
    assert_eq!(error["error"].as_object().unwrap().len(), 3);
    for forbidden in [
        &token,
        &first.broker_id,
        &second.broker_id,
        &foreign.broker_id,
        "event: page",
        "event: audit",
        "read_receipt",
        "items",
        "content",
    ] {
        assert!(!stream.contains(forbidden));
    }
    assert_eq!(
        a.head().unwrap(),
        before_stream,
        "denied stream must not commit a read receipt"
    );
    assert_eq!(
        get(
            &app,
            &token,
            "/api/v1/human/brokers",
            Some(&context(&identity, "personal"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    for path in [
        "/api/v1/human/brokers?tenant=tenant-b",
        "/api/v1/human/brokers?limit=0",
        "/api/v1/human/brokers?limit=1&limit=2",
        "/api/v1/human/brokers?view=metadata",
    ] {
        assert_eq!(
            get(&app, &token, path, Some(&selected)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let events = a
        .events(&EventQuery {
            kind: Some("control.identity.brokers.access".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].event.data["principal"],
        serde_json::to_value(principal).unwrap()
    );
    assert!(a.integrity().unwrap().valid);
}

#[test]
fn broker_capability_cannot_be_persisted_before7_or_delegated_by_org_admin() {
    let files = support::private_tempdir();
    let path = files.path().join("a.sqlite");
    let a = store(&path, "tenant-a");
    let token = generate_local_credential().unwrap();
    let (admin, mut membership) = provision(&a, &token, &[HumanCapabilityV1::ManageMemberships]);
    let mut mutation = change(IdentityChangeV1::Membership {
        expected_revision: 1,
        membership: {
            membership.revision = 2;
            membership.capabilities.insert(cap("read_broker_registry"));
            membership.clone()
        },
    });
    assert_eq!(
        a.mutate_identity(&mutation)
            .unwrap_err()
            .downcast_ref::<IdentityFailureV1>(),
        Some(&IdentityFailureV1::MigrationRequired)
    );
    drop(a);
    migrate7(&path);
    let a =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10000).unwrap();
    mutation.actor = admin.clone();
    assert!(a.mutate_identity(&mutation).is_err());
    mutation.actor = operator();
    a.mutate_identity(&mutation).unwrap();
    membership.revision = 3;
    membership.active = false;
    let mut retain = change(IdentityChangeV1::Membership {
        expected_revision: 2,
        membership: membership.clone(),
    });
    retain.actor = admin.clone();
    a.mutate_identity(&retain).unwrap();
    // The disabled admin cannot mutate itself; trusted control re-enables its old org permission separately.
    membership.revision = 4;
    membership.active = true;
    let mut reactivate = change(IdentityChangeV1::Membership {
        expected_revision: 3,
        membership,
    });
    reactivate.actor = admin;
    assert!(a.mutate_identity(&reactivate).is_err());
    assert!(a.integrity().unwrap().valid);
}

#[test]
fn organization_admin_can_retain_or_remove_but_never_introduce_or_reactivate_broker_grants() {
    let files = support::private_tempdir();
    let path = files.path().join("a.sqlite");
    let a = store(&path, "tenant-a");
    a.migrate_human_brokers_v7(&path.with_extension("v6.bak"))
        .unwrap();
    let (admin, mut admin_member) = provision(
        &a,
        &generate_local_credential().unwrap(),
        &[HumanCapabilityV1::ManageMemberships],
    );
    let (_, mut target) = provision(
        &a,
        &generate_local_credential().unwrap(),
        &[cap("read_broker_registry")],
    );
    let apply = |membership: MembershipV1, actor: PrincipalRefV1| {
        let expected_revision = membership.revision - 1;
        let mut input = change(IdentityChangeV1::Membership {
            expected_revision,
            membership,
        });
        input.actor = actor;
        a.mutate_identity(&input)
    };
    admin_member.revision = 2;
    admin_member
        .capabilities
        .insert(cap("read_broker_registry"));
    let before = a.head().unwrap();
    assert!(apply(admin_member, admin.clone()).is_err());
    assert_eq!(a.head().unwrap(), before);
    // Same active membership retains the existing grant, then becomes dormant.
    for (revision, active) in [(2, true), (3, false), (4, false)] {
        target.revision = revision;
        target.active = active;
        apply(target.clone(), admin.clone()).unwrap();
    }
    target.revision = 5;
    target.active = true;
    let before = a.head().unwrap();
    assert!(apply(target.clone(), admin.clone()).is_err());
    assert_eq!(a.head().unwrap(), before);
    target.active = false;
    target.capabilities.clear();
    apply(target.clone(), admin.clone()).unwrap();
    target.revision = 6;
    target.capabilities.insert(cap("read_broker_registry"));
    assert!(apply(target.clone(), admin.clone()).is_err());
    let mut copied = target.clone();
    copied.membership_id = id();
    copied.revision = 1;
    assert!(apply(copied, admin.clone()).is_err());
    target.active = true;
    apply(target.clone(), operator()).unwrap();
    for mutate_principal in [false, true] {
        let mut changed = target.clone();
        changed.revision = 7;
        if mutate_principal {
            changed.principal = admin.clone();
        } else {
            changed.organization_scope = "other-organization".into();
        }
        let actor = if mutate_principal {
            admin.clone()
        } else {
            operator()
        };
        let error = apply(changed, actor).unwrap_err();
        // Immutable identity checks precede the tenant-wide retention rule.
        assert_eq!(
            error.downcast_ref::<IdentityFailureV1>(),
            Some(&IdentityFailureV1::InvalidRequest)
        );
    }
    drop(a);
    let reopened =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10000).unwrap();
    assert!(reopened.integrity().unwrap().valid);
    assert!(
        reopened.identity_memberships(&target.principal).unwrap()[0]
            .capabilities
            .contains(&cap("read_broker_registry"))
    );
}

#[tokio::test]
async fn historical_admin_capabilities_and_technical_credentials_never_read_brokers() {
    let files = support::private_tempdir();
    let path = files.path().join("a.sqlite");
    let a = store(&path, "tenant-a");
    a.migrate_human_brokers_v7(&path.with_extension("v6.bak"))
        .unwrap();
    let a = Arc::new(a);
    let b = Arc::new(store(&files.path().join("b.sqlite"), "tenant-b"));
    let tokens: Vec<_> = [
        HumanCapabilityV1::ReadTenantMetadata,
        HumanCapabilityV1::ManageMemberships,
    ]
    .into_iter()
    .map(|capability| {
        let token = generate_local_credential().unwrap();
        provision(&a, &token, &[capability]);
        token
    })
    .collect();
    let app = router(&a, &b);
    for token in &tokens {
        let (status, identity) = get(&app, token, "/api/v1/human/identity", None).await;
        assert_eq!(status, StatusCode::OK);
        let before = a.head().unwrap();
        let (status, body) = get(
            &app,
            token,
            "/api/v1/human/brokers",
            Some(&context(&identity, "administration")),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], "permission_denied");
        assert_eq!(a.head().unwrap(), before);
    }
    let before = a.head().unwrap();
    for token in [
        "fixture-control-runner-a".repeat(3),
        "fixture-control-viewer-a".repeat(3),
        "unrecognized-worker-broker-credential".repeat(2),
    ] {
        assert_eq!(
            get(&app, &token, "/api/v1/human/brokers", None).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(a.head().unwrap(), before);
}

#[tokio::test]
async fn historical_tenant_metadata_keeps_its_organization_filter_on_schema7() {
    let files = support::private_tempdir();
    let path = files.path().join("a.sqlite");
    let a = store(&path, "tenant-a");
    a.migrate_human_brokers_v7(&path.with_extension("v6.bak"))
        .unwrap();
    let token = generate_local_credential().unwrap();
    let (owner_a, _) = provision(&a, &token, &[HumanCapabilityV1::ReadTenantMetadata]);
    let (mut prepared, owner_b, mut member_b) = preparation(
        &a,
        &generate_local_credential().unwrap(),
        &[HumanCapabilityV1::ReadMetadata],
    );
    member_b.organization_scope = "organization-b".into();
    prepared.mutations[2].change = IdentityChangeV1::Membership {
        expected_revision: 0,
        membership: member_b,
    };
    a.provision_local_human(&prepared).unwrap();
    for (principal, organization, session) in [
        (owner_a, "organization-a", "organization-a-session"),
        (owner_b, "organization-b", "organization-b-private-session"),
    ] {
        let mut event = agentsandbox::model::Event::new("fixture.organization");
        event.session_id = Some(session.into());
        a.append(event).unwrap();
        a.mutate_identity(&change(IdentityChangeV1::ResourceGrant {
            expected_revision: 0,
            grant: ResourceGrantV1 {
                version: 1,
                tenant_id: "tenant-a".into(),
                organization_scope: organization.into(),
                resource: ResourceRefV1 {
                    kind: ResourceKindV1::Session,
                    resource_id: session.into(),
                },
                owner: principal,
                revision: 1,
                active: true,
                shares: vec![],
            },
        }))
        .unwrap();
    }
    let a = Arc::new(a);
    let b = Arc::new(store(&files.path().join("b.sqlite"), "tenant-b"));
    let app = router(&a, &b);
    let (status, identity) = get(&app, &token, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    let selected = context(&identity, "administration");
    for path in ["/api/v1/human/resources", "/api/v1/human/events"] {
        let (status, page) = get(&app, &token, path, Some(&selected)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        let wire = page.to_string();
        assert!(wire.contains("organization-a-session"));
        assert!(!wire.contains("organization-b-private-session"));
    }
    let before = a.head().unwrap();
    assert_eq!(
        get(&app, &token, "/api/v1/human/brokers", Some(&selected))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(a.head().unwrap(), before);
}

#[tokio::test]
async fn broker_contexts_refuse_other_tenants_proofs_and_ambiguous_headers_without_receipts() {
    use tower::ServiceExt;
    let files = support::private_tempdir();
    let a_path = files.path().join("a.sqlite");
    let b_path = files.path().join("b.sqlite");
    let a = store(&a_path, "tenant-a");
    a.migrate_human_brokers_v7(&a_path.with_extension("v6.bak"))
        .unwrap();
    let b = store(&b_path, "tenant-b");
    b.migrate_human_brokers_v7(&b_path.with_extension("v6.bak"))
        .unwrap();
    let token_a = generate_local_credential().unwrap();
    let token_b = generate_local_credential().unwrap();
    provision(&a, &token_a, &[cap("read_broker_registry")]);
    provision(&b, &token_b, &[cap("read_broker_registry")]);
    let grant_a = grant(&a, "tenant-a-authority");
    let grant_b = grant(&b, "tenant-b-authority");
    let a = Arc::new(a);
    let b = Arc::new(b);
    let app = router(&a, &b);
    let (_, identity_a) = get(&app, &token_a, "/api/v1/human/identity", None).await;
    let (_, identity_b) = get(&app, &token_b, "/api/v1/human/identity", None).await;
    let context_a = context(&identity_a, "administration");
    let context_b = context(&identity_b, "administration");
    let (status, page) = get(&app, &token_b, "/api/v1/human/brokers", Some(&context_b)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"][0]["broker_id"], grant_b.broker_id);
    assert!(!page.to_string().contains(&grant_a.broker_id));
    let heads = (a.head().unwrap(), b.head().unwrap());
    for (token, context) in [(&token_a, &context_b), (&token_b, &context_a)] {
        let (status, body) = get(&app, token, "/api/v1/human/brokers", Some(context)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.get("items").is_none());
    }
    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/human/brokers")
                .header("authorization", format!("Bearer {token_a}"))
                .header("x-aiguard-context", &context_a)
                .header("x-aiguard-context", &context_b)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/human/brokers")
                .header("authorization", format!("Bearer {token_a}"))
                .header("authorization", format!("Bearer {token_b}"))
                .header("x-aiguard-context", &context_a)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!((a.head().unwrap(), b.head().unwrap()), heads);
}

#[tokio::test]
async fn broker_live_keyset_pages_are_ordered_complete_byte_bounded_and_domain_bound() {
    let files = support::private_tempdir();
    let path = files.path().join("a.sqlite");
    let a = store(&path, "tenant-a");
    a.migrate_human_brokers_v7(&path.with_extension("v6.bak"))
        .unwrap();
    let token = generate_local_credential().unwrap();
    provision(
        &a,
        &token,
        &[cap("read_broker_registry"), HumanCapabilityV1::ReadMetadata],
    );
    let mut expected = Vec::new();
    for index in 0..105 {
        let input = BrokerGrantMutationV1 {
            version: 1,
            operation_id: id(),
            broker_id: id(),
            expected_revision: None,
            credential_id: id(),
            incarnation_id: id(),
            authorities: (0..16)
                .map(|authority| BrokerAuthorityV1 {
                    authority_id: format!("authority-{index}-{authority}:{}", "x".repeat(220)),
                    runtime_kind: BrokerRuntimeKindV1::Docker,
                })
                .collect(),
            lease_seconds: 30,
            not_after: "2100-01-01T00:00:00.000Z".into(),
        };
        a.mutate_broker_grant(&input).unwrap();
        expected.push(input.broker_id);
    }
    expected.sort();
    let a = Arc::new(a);
    let b = Arc::new(store(&files.path().join("b.sqlite"), "tenant-b"));
    let app = router(&a, &b);
    let (status, identity) = get(&app, &token, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    let selected = context(&identity, "administration");
    let mut seen = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let path = cursor
            .as_ref()
            .map_or("/api/v1/human/brokers?limit=100".into(), |cursor| {
                format!("/api/v1/human/brokers?limit=100&cursor={cursor}")
            });
        let (status, page) = get(&app, &token, &path, Some(&selected)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(serde_json::to_vec(&page).unwrap().len() <= HUMAN_MAX_PAGE_BYTES);
        assert!(!page["items"].as_array().unwrap().is_empty());
        seen.extend(
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["broker_id"].as_str().unwrap().to_owned()),
        );
        pages += 1;
        if pages == 1 {
            assert!(
                page["items"].as_array().unwrap().len() < 100,
                "byte boundary must constrain this real page"
            );
            let token_cursor = page["next_cursor"].as_str().unwrap();
            let before = a.head().unwrap();
            for endpoint in ["resources", "events", "export", "stream"] {
                assert_eq!(
                    get(
                        &app,
                        &token,
                        &format!("/api/v1/human/{endpoint}?limit=100&cursor={token_cursor}"),
                        Some(&selected)
                    )
                    .await
                    .0,
                    StatusCode::BAD_REQUEST
                );
            }
            for bad in [
                format!("{token_cursor}0"),
                format!("1{}", &token_cursor[1..]),
                token_cursor.to_ascii_uppercase(),
            ] {
                assert_eq!(
                    get(
                        &app,
                        &token,
                        &format!("/api/v1/human/brokers?limit=100&cursor={bad}"),
                        Some(&selected)
                    )
                    .await
                    .0,
                    StatusCode::BAD_REQUEST
                );
            }
            assert_eq!(
                get(
                    &app,
                    &token,
                    &format!("/api/v1/human/brokers?limit=99&cursor={token_cursor}"),
                    Some(&selected)
                )
                .await
                .0,
                StatusCode::BAD_REQUEST
            );
            assert_eq!(
                a.head().unwrap(),
                before,
                "invalid cursor must not create a read receipt"
            );
            let retry_path = format!("/api/v1/human/brokers?limit=100&cursor={token_cursor}");
            let first = get(&app, &token, &retry_path, Some(&selected)).await;
            let retry = get(&app, &token, &retry_path, Some(&selected)).await;
            assert_eq!((first.0, retry.0), (StatusCode::OK, StatusCode::OK));
            assert_eq!(first.1["items"], retry.1["items"]);
            assert_ne!(
                first.1["read_receipt"]["request_id"],
                retry.1["read_receipt"]["request_id"]
            );
        }
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            assert_eq!(page["has_more"], false);
            break;
        }
        assert!(pages < 10);
    }
    assert!(pages > 2);
    assert_eq!(seen, expected);
    assert!(a.integrity().unwrap().valid);
}

#[tokio::test]
async fn mixed_human_reads_keep_cumulative_event_and_byte_budgets_across_reopen_and_proxy() {
    use agentsandbox::api::dashboard_router;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;
    struct Server(tokio::task::JoinHandle<()>);
    impl Drop for Server {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    for byte_bound in [false, true] {
        let files = support::private_tempdir();
        let path = files.path().join("a.sqlite");
        let a = store(&path, "tenant-a");
        a.migrate_human_brokers_v7(&path.with_extension("v6.bak"))
            .unwrap();
        let mut policy = a.capacity_policy().clone();
        policy.human_principal.events = if byte_bound { 1000 } else { 3 };
        let a = Arc::new(a.with_capacity_policy(policy.clone()).unwrap());
        let b = Arc::new(store(&files.path().join("b.sqlite"), "tenant-b"));
        let token = generate_local_credential().unwrap();
        let second = generate_local_credential().unwrap();
        let (principal, _) = provision(
            &a,
            &token,
            &[cap("read_broker_registry"), HumanCapabilityV1::ReadMetadata],
        );
        provision(&a, &second, &[cap("read_broker_registry")]);
        let app = router(&a, &b);
        let (status, identity) = get(&app, &token, "/api/v1/human/identity", None).await;
        assert_eq!(status, StatusCode::OK);
        let selected = context(&identity, "administration");
        assert_eq!(
            get(&app, &token, "/api/v1/human/resources", Some(&selected))
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            get(&app, &token, "/api/v1/human/brokers", Some(&selected))
                .await
                .0,
            StatusCode::OK
        );
        let usage: usize = a
            .events(&EventQuery {
                limit: Some(1000),
                ..Default::default()
            })
            .unwrap()
            .iter()
            .filter(|stored| {
                ["control.identity.access", "control.identity.brokers.access"]
                    .contains(&stored.event.kind.as_str())
                    && stored.event.data["principal"] == serde_json::to_value(&principal).unwrap()
            })
            .map(|stored| serde_json::to_vec(&stored.event).unwrap().len())
            .sum();
        assert!(usage > 0);
        if byte_bound {
            policy.human_principal.bytes = u64::try_from(usage).unwrap();
        }
        let replacement = generate_local_credential().unwrap();
        a.mutate_identity(&change(IdentityChangeV1::Credential {
            expected_revision: 0,
            binding: LocalCredentialV1 {
                version: 1,
                credential_id: id(),
                principal: principal.clone(),
                sha256_digest: hex::encode(Sha256::digest(replacement.as_bytes())),
                revision: 1,
                active: true,
                expires_at: None,
            },
        }))
        .unwrap();
        drop(app);
        drop(a);
        let a = Arc::new(
            Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10000)
                .unwrap()
                .with_capacity_policy(policy)
                .unwrap(),
        );
        let app = router(&a, &b);
        let before = a.head().unwrap();
        for credential in [&token, &replacement] {
            let (status, error) = get(&app, credential, "/api/v1/human/identity", None).await;
            assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(error["error"]["code"], "audit_read_budget_exhausted");
            assert_eq!(error["error"]["retry_action"], "contact_operator");
        }
        assert_eq!(
            a.head().unwrap(),
            before,
            "reopen or a new credential must not reset the principal budget"
        );
        let (status, other) = get(&app, &second, "/api/v1/human/identity", None).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "another principal keeps its independent share"
        );
        let broker = grant(&a, "quota-independent-authority");
        let workload_token = "bounded-workload-broker-token".repeat(3);
        a.issue_workload_binding(&agentsandbox::workload::WorkloadBindingIssueV1 {
            version: 1,
            operation_id: id(),
            credential_id: broker.credential_id.clone(),
            expected_revision: 0,
            subject: agentsandbox::workload::WorkloadSubjectV1::Broker {
                broker_id: broker.broker_id.clone(),
                incarnation_id: broker.incarnation_id.clone(),
            },
            sha256_digest: hex::encode(Sha256::digest(workload_token.as_bytes())),
            expires_at: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
        let before_workload_refusal = a.head().unwrap();
        assert_eq!(
            get(
                &app,
                &workload_token,
                "/api/v1/human/brokers",
                Some(&context(&other, "administration"))
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(a.head().unwrap(), before_workload_refusal);
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/v1/brokers/heartbeat")
                    .header("authorization", format!("Bearer {workload_token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&BrokerHeartbeatV1 {
                            version: 1,
                            operation_id: id(),
                            expected_heartbeat_revision: 0,
                            incarnation_id: broker.incarnation_id.clone(),
                            sequence: 1,
                        })
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "workload admission retains its separate audit budget"
        );
        let (status, page) = get(
            &app,
            &second,
            "/api/v1/human/brokers",
            Some(&context(&other, "administration")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["items"][0]["state"], "current");
        assert_eq!(page["items"][0]["last_heartbeat"]["sequence"], "1");
        assert_eq!(
            page["items"][0]["last_heartbeat"]["heartbeat_provenance"],
            "declared"
        );
        assert_eq!(
            page["items"][0]["last_heartbeat"]["reception_provenance"],
            "observed"
        );
        let requests = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&requests);
        let gateway = app.layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let counted = Arc::clone(&counted);
                async move {
                    if request.uri().path() == "/api/v1/human/brokers" {
                        counted.fetch_add(1, Ordering::SeqCst);
                    }
                    next.run(request).await
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = format!("http://{}", listener.local_addr().unwrap());
        let _server = Server(tokio::spawn(async move {
            axum::serve(listener, gateway).await.unwrap();
        }));
        let front = files.path().join("front");
        std::fs::create_dir(&front).unwrap();
        std::fs::write(front.join("index.html"), "<title>test</title>").unwrap();
        let dashboard =
            dashboard_router(&upstream, "dashboard-budget-viewer".repeat(3), front).unwrap();
        let before = a.head().unwrap();
        let (status, body) = get(
            &dashboard,
            &replacement,
            "/api/v1/human/brokers",
            Some(&"0".repeat(64)),
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["error"]["code"], "audit_read_budget_exhausted");
        assert_eq!(body["error"]["retry_action"], "contact_operator");
        assert!(!body.to_string().contains(&replacement));
        assert_eq!(
            requests.load(Ordering::SeqCst),
            0,
            "durable identity preflight refusal must never reach broker handler"
        );
        assert_eq!(a.head().unwrap(), before);
        let mut account = a.human_account(&principal).unwrap().unwrap();
        account.revision += 1;
        account.active = false;
        a.mutate_identity(&change(IdentityChangeV1::Account {
            expected_revision: account.revision - 1,
            account,
        }))
        .unwrap();
        assert!(
            a.integrity().unwrap().valid,
            "identity reserve remains available after human reads are exhausted"
        );
    }
}
