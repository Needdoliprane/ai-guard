mod support;
use agentsandbox::{
    audit::{EventQuery, Store},
    control::SupervisionProtocolV1,
    identity::*,
    model::Event,
    redaction::Redactor,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path, sync::Arc};
use uuid::Uuid;

fn v4(path: &Path, capacity: u64) -> Store {
    let store = Store::create_supervised_for_tenant(
        path,
        "tenant-a",
        Redactor::new(vec![]),
        capacity,
        &path.with_extension("before-v3"),
    )
    .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("before-v4"))
        .unwrap();
    store
}
fn version(path: &Path) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap()
}

// The audited-read vocabulary is a real capability migration: exact prior
// payloads and live/inactive supervision witnesses must retain their meaning.
#[test]
fn human_access_migration_preserves_v4_prefix_namespace_and_authority_without_reactivation() {
    for reopen_first in [false, true] {
        let files = support::private_tempdir();
        let path = files.path().join("audit.sqlite");
        let backup = files.path().join("before-v5.sqlite");
        let mut store = v4(&path, 1000);
        store.append(Event::new("legacy.retained")).unwrap();
        if reopen_first {
            drop(store);
            store = Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000)
                .unwrap();
        }
        let state = store.supervision_state().unwrap();
        assert_eq!(state.active, !reopen_first);
        let before = store
            .events(&EventQuery {
                order: Some("asc".into()),
                limit: Some(1000),
                ..Default::default()
            })
            .unwrap();
        let revision = store.identity_authorization_revision().unwrap();
        let report = store.migrate_human_access_v5(&backup).unwrap();
        assert_eq!((report.from_version, report.to_version), (4, 5));
        assert_eq!(version(&path), 5);
        assert_eq!(store.identity_authorization_revision().unwrap(), revision);
        let after = store
            .events(&EventQuery {
                order: Some("asc".into()),
                limit: Some(1000),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            serde_json::to_value(&after[..before.len()]).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
        let marker = after.last().unwrap();
        assert_eq!(marker.event.kind, "control.identity.human_access.genesis");
        assert_eq!(marker.event.data["protocol"], "local_human_access_v1");
        assert_eq!(report.genesis_hash, marker.hash);
        let migrated = store.supervision_state().unwrap();
        assert_eq!(migrated.schema_version, 5);
        assert_eq!(migrated.active, state.active);
        assert_eq!(
            migrated.control_namespace_uuid,
            state.control_namespace_uuid
        );
        assert_eq!(
            migrated.protocols,
            [SupervisionProtocolV1::BrokerSupervisionV1]
        );
        assert!(store.integrity().unwrap().valid);
        let backed =
            Store::open_existing_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 1000)
                .unwrap();
        assert_eq!(backed.supervision_state().unwrap().schema_version, 4);
        assert_eq!(backed.head().unwrap(), report.genesis_seq - 1);
        assert!(backed.integrity().unwrap().valid);
        drop(store);
        let reopened =
            Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000)
                .unwrap();
        assert!(!reopened.supervision_state().unwrap().active);
        assert_eq!(
            reopened.supervision_state().unwrap().control_namespace_uuid,
            state.control_namespace_uuid
        );
        assert!(reopened.integrity().unwrap().valid);
    }
}

// Failed audit capacity and a reused backup cannot leave a hidden capability
// activation or consume the old namespace's authority.
#[test]
fn human_access_migration_audit_failure_keeps_exact_v4_and_verified_backup() {
    let files = support::private_tempdir();
    let path = files.path().join("audit.sqlite");
    let backup = files.path().join("before-v5.sqlite");
    let store = v4(&path, 3);
    let head = store.head().unwrap();
    assert!(store.migrate_human_access_v5(&backup).is_err());
    assert_eq!(version(&path), 4);
    assert_eq!(store.head().unwrap(), head);
    assert!(store.supervision_state().unwrap().active);
    assert!(
        store
            .events(&EventQuery {
                kind: Some("control.identity.human_access.genesis".into()),
                ..Default::default()
            })
            .unwrap()
            .is_empty()
    );
    let bytes = std::fs::read(&backup).unwrap();
    assert!(store.migrate_human_access_v5(&backup).is_err());
    assert_eq!(std::fs::read(&backup).unwrap(), bytes);
    assert!(store.integrity().unwrap().valid);
    assert!(
        Store::open_existing_for_tenant(&backup, "tenant-a", Redactor::new(vec![]), 1000)
            .unwrap()
            .integrity()
            .unwrap()
            .valid
    );
}

// A pragma alone cannot advertise the new vocabulary, and the new vocabulary
// may not be downgraded to published4 or accepted as an unknown future schema.
#[test]
fn forced_v5_missing_marker_and_marker_under_v4_are_refused_before_repair() {
    for (migrate, target) in [(false, 5), (true, 4), (true, 6)] {
        let files = support::private_tempdir();
        let path = files.path().join("audit.sqlite");
        let store = v4(&path, 1000);
        if migrate {
            store
                .migrate_human_access_v5(&files.path().join("before-v5.sqlite"))
                .unwrap();
        }
        drop(store);
        let db = rusqlite::Connection::open(&path).unwrap();
        db.pragma_update(None, "user_version", target).unwrap();
        drop(db);
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000)
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

fn provision(token: &str) -> LocalHumanProvisionV1 {
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: Uuid::new_v4().to_string(),
        account_id: Uuid::new_v4().to_string(),
    };
    let actor = PrincipalRefV1::LegacyControl {
        credential_id: "fixture-offline-control".into(),
        role: LegacyControlRoleV1::Runner,
    };
    let input = |change| IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: actor.clone(),
        change,
    };
    LocalHumanProvisionV1 {
        version: 1,
        mutations: [
            input(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Local reader".into(),
                    revision: 1,
                    active: true,
                },
            }),
            input(IdentityChangeV1::Credential {
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
            input(IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: MembershipV1 {
                    version: 1,
                    membership_id: Uuid::new_v4().to_string(),
                    principal,
                    tenant_id: "tenant-a".into(),
                    organization_scope: "organization-a".into(),
                    revision: 1,
                    active: true,
                    capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                },
            }),
        ],
    }
}

// Provisioning remains4-compatible; audited human GET requires the explicit5
// capability and may never append access events while still on4.
#[tokio::test]
async fn human_get_requires_explicit_v5_while_technical_v4_and_prepared_identity_remain_valid() {
    use agentsandbox::{
        api::{GatewayState, gateway_router},
        policy::Policy,
    };
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    let files = support::private_tempdir();
    let path = files.path().join("audit.sqlite");
    let store = Arc::new(v4(&path, 1000));
    let token = generate_local_credential().unwrap();
    let prepared = provision(&token);
    store.provision_local_human(&prepared).unwrap();
    let head = store.head().unwrap();
    let router = gateway_router(GatewayState::new(
        Arc::clone(&store),
        "fixture-technical-runner".repeat(3),
        "fixture-technical-viewer".repeat(3),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    ));
    let request = || {
        Request::builder()
            .uri("/api/v1/human/identity")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    };
    let response = router.clone().oneshot(request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "migration_required");
    assert_eq!(store.head().unwrap(), head);
    assert!(
        store
            .events(&EventQuery {
                kind: Some("control.identity.access".into()),
                ..Default::default()
            })
            .unwrap()
            .is_empty()
    );
    drop(router);
    let receipts = store.provision_local_human(&prepared).unwrap();
    store
        .migrate_human_access_v5(&files.path().join("before-v5.sqlite"))
        .unwrap();
    assert_eq!(store.provision_local_human(&prepared).unwrap(), receipts);
    let router = gateway_router(GatewayState::new(
        Arc::clone(&store),
        "fixture-technical-runner".repeat(3),
        "fixture-technical-viewer".repeat(3),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    ));
    assert_eq!(
        router.oneshot(request()).await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(
        store
            .events(&EventQuery {
                kind: Some("control.identity.access".into()),
                ..Default::default()
            })
            .unwrap()
            .len(),
        1
    );
    assert!(store.integrity().unwrap().valid);
}
