//! Causal post-commit authorization regression using the actual Router/SQLite.
use super::*;
use crate::api::read_execution_tests::{identity_change, local_identity_state};
use axum::{body::to_bytes, http::Request};
use tower::ServiceExt;

struct ResumeOnDrop(Option<std::sync::mpsc::SyncSender<()>>);
impl Drop for ResumeOnDrop {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broker_membership_revoked_after_durable_read_is_rechecked_before_publication() {
    let (files, state, token) = local_identity_state();
    let store = Arc::clone(state.tenants[1].store.sqlite().unwrap());
    store
        .migrate_workload_v6(&files.path().join("broker-before-v6.sqlite"))
        .unwrap();
    store
        .migrate_human_brokers_v7(&files.path().join("broker-before-v7.sqlite"))
        .unwrap();
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
        account_id: "290a1a3b-8538-49ef-a830-25df6fa19030".into(),
    };
    let mut member = store.identity_memberships(&principal).unwrap().remove(0);
    member.revision += 1;
    member
        .capabilities
        .insert(HumanCapabilityV1::ReadBrokerRegistry);
    store
        .mutate_identity(&identity_change(IdentityChangeV1::Membership {
            expected_revision: member.revision - 1,
            membership: member.clone(),
        }))
        .unwrap();
    let broker_id = uuid::Uuid::new_v4().to_string();
    store
        .mutate_broker_grant(&crate::broker_registry::BrokerGrantMutationV1 {
            version: 1,
            operation_id: uuid::Uuid::new_v4().to_string(),
            broker_id: broker_id.clone(),
            expected_revision: None,
            credential_id: uuid::Uuid::new_v4().to_string(),
            incarnation_id: uuid::Uuid::new_v4().to_string(),
            authorities: vec![crate::broker_registry::BrokerAuthorityV1 {
                authority_id: "private-publication-race-authority".into(),
                runtime_kind: crate::broker_registry::BrokerRuntimeKindV1::Docker,
            }],
            lease_seconds: 30,
            not_after: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    let app = gateway_router(state.clone());
    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/human/identity")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let identity: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), HUMAN_MAX_PAGE_BYTES)
            .await
            .unwrap(),
    )
    .unwrap();
    let context = identity["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|context| context["tenant_id"] == "tenant-b" && context["mode"] == "administration")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (reached, receive) = std::sync::mpsc::sync_channel(1);
    let (resume, wait) = std::sync::mpsc::sync_channel(1);
    *state.human.broker_publication_pause.lock().unwrap() = Some(BrokerPublicationPause {
        reached,
        resume: wait,
    });
    let mut release = ResumeOnDrop(Some(resume));
    let before = store.head().unwrap();
    let request = Request::get("/api/v1/human/brokers")
        .header("authorization", format!("Bearer {token}"))
        .header("x-aiguard-context", context)
        .body(Body::empty())
        .unwrap();
    let pending = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    tokio::task::spawn_blocking(move || receive.recv_timeout(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store.head().unwrap(),
        before + 1,
        "pause follows the real durable read"
    );
    let independent = Store::open_existing_for_tenant(
        &files.path().join("tenant-b.sqlite"),
        "tenant-b",
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    member.revision += 1;
    member
        .capabilities
        .remove(&HumanCapabilityV1::ReadBrokerRegistry);
    independent
        .mutate_identity(&identity_change(IdentityChangeV1::Membership {
            expected_revision: member.revision - 1,
            membership: member,
        }))
        .unwrap();
    release.0.take().unwrap().send(()).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(3), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(error["error"]["code"], "context_revoked");
    assert!(error.get("items").is_none() && error.get("read_receipt").is_none());
    assert!(!error.to_string().contains(&token));
    assert!(!error.to_string().contains(&broker_id));
    assert!(
        !error
            .to_string()
            .contains("private-publication-race-authority")
    );
    assert_eq!(
        store.head().unwrap(),
        before + 2,
        "read and explicit revocation remain distinct durable events"
    );
    assert!(store.integrity().unwrap().valid);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_membership_revoked_after_durable_read_is_rechecked_before_publication() {
    let (files, state, token) = local_identity_state();
    let store = Arc::clone(state.tenants[1].store.sqlite().unwrap());
    store
        .migrate_workload_v6(&files.path().join("broker-before-v6.sqlite"))
        .unwrap();
    store
        .migrate_human_brokers_v7(&files.path().join("broker-before-v7.sqlite"))
        .unwrap();
    store
        .migrate_kubernetes_create_v8(&files.path().join("worker-before8"))
        .unwrap();
    store
        .migrate_kubernetes_policy_v9(&files.path().join("worker-before9"))
        .unwrap();
    store
        .migrate_human_workers_v10(&files.path().join("worker-before10"))
        .unwrap();
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
        account_id: "290a1a3b-8538-49ef-a830-25df6fa19030".into(),
    };
    let mut member = store.identity_memberships(&principal).unwrap().remove(0);
    member.revision += 1;
    member
        .capabilities
        .insert(HumanCapabilityV1::ReadWorkerRegistry);
    store
        .mutate_identity(&identity_change(IdentityChangeV1::Membership {
            expected_revision: member.revision - 1,
            membership: member.clone(),
        }))
        .unwrap();
    let broker_id = uuid::Uuid::new_v4().to_string();
    store
        .mutate_broker_grant(&crate::broker_registry::BrokerGrantMutationV1 {
            version: 1,
            operation_id: uuid::Uuid::new_v4().to_string(),
            broker_id: broker_id.clone(),
            expected_revision: None,
            credential_id: uuid::Uuid::new_v4().to_string(),
            incarnation_id: uuid::Uuid::new_v4().to_string(),
            authorities: vec![crate::broker_registry::BrokerAuthorityV1 {
                authority_id: "private-publication-race-authority".into(),
                runtime_kind: crate::broker_registry::BrokerRuntimeKindV1::Docker,
            }],
            lease_seconds: 30,
            not_after: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    let app = gateway_router(state.clone());
    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/human/identity")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let identity: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), HUMAN_MAX_PAGE_BYTES)
            .await
            .unwrap(),
    )
    .unwrap();
    let context = identity["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|context| context["tenant_id"] == "tenant-b" && context["mode"] == "administration")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (reached, receive) = std::sync::mpsc::sync_channel(1);
    let (resume, wait) = std::sync::mpsc::sync_channel(1);
    *state.human.broker_publication_pause.lock().unwrap() = Some(BrokerPublicationPause {
        reached,
        resume: wait,
    });
    let mut release = ResumeOnDrop(Some(resume));
    let before = store.head().unwrap();
    let request = Request::get("/api/v1/human/workers")
        .header("authorization", format!("Bearer {token}"))
        .header("x-aiguard-context", context)
        .body(Body::empty())
        .unwrap();
    let pending = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    tokio::task::spawn_blocking(move || receive.recv_timeout(Duration::from_secs(3)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        store.head().unwrap(),
        before + 1,
        "pause follows the real durable read"
    );
    let independent = Store::open_existing_for_tenant(
        &files.path().join("tenant-b.sqlite"),
        "tenant-b",
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    member.revision += 1;
    member
        .capabilities
        .remove(&HumanCapabilityV1::ReadWorkerRegistry);
    independent
        .mutate_identity(&identity_change(IdentityChangeV1::Membership {
            expected_revision: member.revision - 1,
            membership: member,
        }))
        .unwrap();
    release.0.take().unwrap().send(()).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(3), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(error["error"]["code"], "context_revoked");
    assert!(error.get("items").is_none() && error.get("read_receipt").is_none());
    assert!(!error.to_string().contains(&token));
    assert!(!error.to_string().contains(&broker_id));
    assert!(
        !error
            .to_string()
            .contains("private-publication-race-authority")
    );
    assert_eq!(
        store.head().unwrap(),
        before + 2,
        "read and explicit revocation remain distinct durable events"
    );
    assert!(store.integrity().unwrap().valid);
}
