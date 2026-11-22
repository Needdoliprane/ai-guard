use super::*;
use axum::http::{Method, Request};
use std::{
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicUsize, Ordering},
    task::Poll,
};
use tower::ServiceExt;

fn fixture() -> (tempfile::TempDir, GatewayState) {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let mut tenants = Vec::new();
    for id in ["a", "b"] {
        let path = directory.path().join(format!("{id}.sqlite"));
        let store = Store::create_supervised_for_tenant(
            &path,
            id,
            Redactor::new(vec![]),
            10_000,
            &directory.path().join(format!("{id}-before3")),
        )
        .unwrap();
        store
            .migrate_identity_v4(&directory.path().join(format!("{id}-before4")))
            .unwrap();
        store
            .migrate_human_access_v5(&directory.path().join(format!("{id}-before5")))
            .unwrap();
        store
            .migrate_workload_v6(&directory.path().join(format!("{id}-before6")))
            .unwrap();
        store
            .migrate_human_brokers_v7(&directory.path().join(format!("{id}-before7")))
            .unwrap();
        store
            .migrate_kubernetes_create_v8(&directory.path().join(format!("{id}-before8")))
            .unwrap();
        store
            .migrate_kubernetes_policy_v9(&directory.path().join(format!("{id}-before9")))
            .unwrap();
        store
            .migrate_human_workers_v10(&directory.path().join(format!("{id}-before10")))
            .unwrap();
        tenants.push(TenantGateway {
            tenant_id: id.into(),
            store: Arc::new(store),
            runner_token: format!("runner-{id}-{}", "r".repeat(48)),
            viewer_token: format!("viewer-{id}-{}", "v".repeat(48)),
        });
    }
    (
        directory,
        GatewayState::for_tenants(tenants, Policy::default(), None, Redactor::new(vec![])).unwrap(),
    )
}

fn counted_body(polls: Arc<AtomicUsize>) -> Body {
    Body::from_stream(futures_util::stream::poll_fn(move |_| {
        polls.fetch_add(1, Ordering::SeqCst);
        Poll::<Option<Result<axum::body::Bytes, std::io::Error>>>::Pending
    }))
}

async fn request(router: Router, path: &str, token: Option<&str>, body: Body) -> Response {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    router.oneshot(request.body(body).unwrap()).await.unwrap()
}

#[tokio::test]
async fn anonymous_and_viewer_refuse_before_body_even_when_runner_capacity_is_full() {
    let (_directory, state) = fixture();
    let app = super::router(state.clone());
    let capacity = &state.tenants[0].control_capacity;
    let _full = capacity
        .runner
        .clone()
        .acquire_many_owned(RUNNER_OPERATIONS as u32)
        .await
        .unwrap();
    let before = state.tenants[0].store.head().unwrap();
    let viewer = format!("viewer-a-{}", "v".repeat(48));
    for (token, status) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(viewer.as_str()), StatusCode::FORBIDDEN),
    ] {
        let polls = Arc::new(AtomicUsize::new(0));
        let response = request(
            app.clone(),
            "/api/v1/workers/not-a-uuid/transitions",
            token,
            counted_body(polls.clone()),
        )
        .await;
        assert_eq!(response.status(), status);
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        assert_eq!(capacity.runner.available_permits(), 0);
        assert_eq!(capacity.presence.available_permits(), BROKER_PRESENCE);
    }
    assert_eq!(state.tenants[0].store.head().unwrap(), before);
}

#[tokio::test]
async fn cloned_listeners_share_per_tenant_capacity_and_cancel_releases_operation() {
    let (_directory, state) = fixture();
    let direct = gateway_control_router(state.clone());
    let nested = gateway_router(state.clone());
    let a = state.tenants[0].control_capacity.clone();
    let _full = a
        .runner
        .clone()
        .acquire_many_owned(RUNNER_OPERATIONS as u32)
        .await
        .unwrap();
    for (app, prefix) in [(direct.clone(), ""), (nested, "/control")] {
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("{prefix}/api/v1/control/protocol"))
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer runner-a-{}", "r".repeat(48)),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }
    let response = direct
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/control/protocol")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer runner-b-{}", "r".repeat(48)),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(a.presence.available_permits(), BROKER_PRESENCE);
    drop(_full);
    let (polled, admitted) = tokio::sync::oneshot::channel();
    let mut polled = Some(polled);
    let pending = Body::from_stream(futures_util::stream::poll_fn(move |_| {
        if let Some(polled) = polled.take() {
            let _ = polled.send(());
        }
        Poll::<Option<Result<axum::body::Bytes, std::io::Error>>>::Pending
    }));
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async move {
        request(
            direct,
            "/api/v1/decide",
            Some(&format!("runner-a-{}", "r".repeat(48))),
            pending,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(30), admitted)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a.runner.available_permits(), RUNNER_OPERATIONS - 1);
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    assert_eq!(a.runner.available_permits(), RUNNER_OPERATIONS);
}

#[tokio::test]
async fn broker_presence_is_separate_and_revocation_is_rechecked_after_upload() {
    use crate::{
        broker_registry::*,
        workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
    };
    use uuid::Uuid;
    let (_directory, state) = fixture();
    let store = &state.tenants[0].store;
    let token = "fixture-presence-broker-proof".repeat(3);
    let broker_id = Uuid::new_v4().to_string();
    let credential_id = Uuid::new_v4().to_string();
    let incarnation_id = Uuid::new_v4().to_string();
    store
        .sqlite()
        .unwrap()
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential_id.clone(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: broker_id.clone(),
                incarnation_id: incarnation_id.clone(),
            },
            sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
            expires_at: "2100-01-01T00:00:00Z".into(),
        })
        .unwrap();
    store
        .sqlite()
        .unwrap()
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: broker_id.clone(),
            expected_revision: None,
            credential_id,
            incarnation_id: incarnation_id.clone(),
            authorities: vec![BrokerAuthorityV1 {
                authority_id: "fixture-local-authority".into(),
                runtime_kind: BrokerRuntimeKindV1::Docker,
            }],
            lease_seconds: 120,
            not_after: "2100-01-01T00:00:00Z".into(),
        })
        .unwrap();
    let app = gateway_control_router(state.clone());
    let capacity = state.tenants[0].control_capacity.clone();
    let _runner_full = capacity
        .runner
        .clone()
        .acquire_many_owned(RUNNER_OPERATIONS as u32)
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/brokers/identity")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let polls = Arc::new(AtomicUsize::new(0));
    let response = request(
        app.clone(),
        "/api/v1/decide",
        Some(&token),
        counted_body(polls.clone()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    let (polled, admission) = tokio::sync::oneshot::channel();
    let (finish, finished) = tokio::sync::oneshot::channel::<Vec<u8>>();
    let stream = async_stream::stream! {
        let _ = polled.send(());
        if let Ok(bytes) = finished.await { yield Ok::<_, std::io::Error>(bytes); }
    };
    let mut tasks = tokio::task::JoinSet::new();
    let proof = token.clone();
    tasks.spawn(async move {
        request(
            app,
            "/api/v1/brokers/heartbeat",
            Some(&proof),
            Body::from_stream(stream),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(30), admission)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(capacity.presence.available_permits(), BROKER_PRESENCE - 1);
    store
        .sqlite()
        .unwrap()
        .revoke_broker(&BrokerRevocationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id,
            expected_revision: 1,
        })
        .unwrap();
    let head = store.head().unwrap();
    finish
        .send(
            serde_json::to_vec(&BrokerHeartbeatV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                expected_heartbeat_revision: 0,
                incarnation_id,
                sequence: 1,
            })
            .unwrap(),
        )
        .unwrap();
    let response = tasks.join_next().await.unwrap().unwrap();
    assert!(matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    assert_eq!(store.head().unwrap(), head);
    assert_eq!(capacity.presence.available_permits(), BROKER_PRESENCE);
    assert_eq!(capacity.runner.available_permits(), 0);
}

#[tokio::test]
async fn failed_or_oversized_bodies_do_not_dispatch_and_release_capacity() {
    let (_directory, state) = fixture();
    let app = gateway_control_router(state.clone());
    let capacity = state.tenants[0].control_capacity.clone();
    let token = format!("runner-a-{}", "r".repeat(48));
    let head = state.tenants[0].store.head().unwrap();
    for (body, length, expected) in [
        (
            Body::from(vec![b'x'; MAX_BODY + 1]),
            None,
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (Body::from("{}"), Some("4"), StatusCode::BAD_REQUEST),
        (
            Body::from_stream(futures_util::stream::once(async {
                Err::<axum::body::Bytes, _>(std::io::Error::other("inert incomplete fixture"))
            })),
            None,
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/decide")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(length) = length {
            builder = builder.header(header::CONTENT_LENGTH, length);
        }
        let response = app
            .clone()
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(capacity.runner.available_permits(), RUNNER_OPERATIONS);
        assert_eq!(state.tenants[0].store.head().unwrap(), head);
    }
    for ambiguous in [false, true] {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/decide")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_LENGTH, "2");
        builder = if ambiguous {
            builder.header(header::TRANSFER_ENCODING, "chunked")
        } else {
            builder.header(header::CONTENT_LENGTH, "3")
        };
        let response = app
            .clone()
            .oneshot(builder.body(Body::from("{}")).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(capacity.runner.available_permits(), RUNNER_OPERATIONS);
    }
    let response = tokio::time::timeout(
        Duration::from_secs(30),
        request(
            app,
            "/api/v1/decide",
            Some(&token),
            counted_body(Arc::new(AtomicUsize::new(0))),
        ),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    assert_eq!(capacity.runner.available_permits(), RUNNER_OPERATIONS);
    assert_eq!(state.tenants[0].store.head().unwrap(), head);
}
