//! Publication races use the actual Router, checked Store and independent SQLite handle.
use super::*;
use crate::api::read_execution_tests::{human_get, identity_change, local_identity_state};
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
async fn either_federation_capability_revoked_after_both_receipts_prevents_publication() {
    for withdrawn in [
        HumanCapabilityV1::ReadWorkerRegistry,
        HumanCapabilityV1::ReadBrokerRegistry,
    ] {
        let (files, state, token) = local_identity_state();
        let store = Arc::clone(&state.tenants[1].store);
        for (version, suffix) in [
            (6, "six"),
            (7, "seven"),
            (8, "eight"),
            (9, "nine"),
            (10, "ten"),
        ] {
            let backup = files
                .path()
                .join(format!("federation-before-{suffix}.sqlite"));
            match version {
                6 => store.migrate_workload_v6(&backup).unwrap(),
                7 => store.migrate_human_brokers_v7(&backup).unwrap(),
                8 => store.migrate_kubernetes_create_v8(&backup).unwrap(),
                9 => store.migrate_kubernetes_policy_v9(&backup).unwrap(),
                10 => store.migrate_human_workers_v10(&backup).unwrap(),
                _ => unreachable!(),
            };
        }
        let principal = PrincipalRefV1::HumanLocal {
            installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
            account_id: "290a1a3b-8538-49ef-a830-25df6fa19030".into(),
        };
        let mut member = store.identity_memberships(&principal).unwrap().remove(0);
        member.revision += 1;
        member.capabilities.extend([
            HumanCapabilityV1::ReadWorkerRegistry,
            HumanCapabilityV1::ReadBrokerRegistry,
        ]);
        store
            .mutate_identity(&identity_change(IdentityChangeV1::Membership {
                expected_revision: member.revision - 1,
                membership: member.clone(),
            }))
            .unwrap();
        let app = gateway_router(state.clone());
        let (status, identity) = human_get(&app, &token, "/api/v1/human/identity", None).await;
        assert_eq!(status, StatusCode::OK);
        let context = identity["contexts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["tenant_id"] == "tenant-b" && entry["mode"] == "administration")
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
        let request = Request::get("/api/v1/human/federation/workers")
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
            before + 2,
            "pause follows two actual durable reads"
        );
        let independent = Store::open_existing_for_tenant(
            &files.path().join("tenant-b.sqlite"),
            "tenant-b",
            Redactor::new(vec![]),
            1000,
        )
        .unwrap();
        member.revision += 1;
        member.capabilities.remove(&withdrawn);
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
        assert!(error.get("workers").is_none() && error.get("brokers").is_none());
        assert_eq!(independent.head().unwrap(), before + 3);
        assert!(independent.integrity().unwrap().valid);
    }
}

#[test]
fn dashboard_allows_only_the_exact_additive_human_federation_path() {
    assert!(crate::api::human_dashboard_path(
        "/api/human/federation/workers"
    ));
    for path in [
        "/api/human/federation",
        "/api/human/federation/workers/other",
        "/api/human/federation/brokers",
    ] {
        assert!(!crate::api::human_dashboard_path(path));
    }
}
