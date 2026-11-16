//! Real Store/router/HTTP client before-effect lifecycle. Synthetic broker
//! credentials and profiles only; no Kubernetes endpoint or provider is used.
use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{kubernetes_admission_state_tests::Fixture, kubernetes_session::tests as fixture},
    broker::held_create_client::{HeldCreateClientScope, sessions::SessionControlClientV2},
    control_fs::{Directory, PrivateFile},
    policy::Policy,
    redaction::Redactor,
    runtime::kubernetes::{session::SessionJournalProtection, session_lifecycle::remote},
};
use axum::{body::Body, http::Request};
use futures_util::stream;
use std::{
    convert::Infallible,
    fs,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::Poll,
};
use tower::ServiceExt;
use uuid::Uuid;

const TOKEN: &str = "public-held-create-test-broker-token-0000000000000000";
const RUNNER: &str = "public-r2-test-runner-00000000000000000000";
const VIEWER: &str = "public-r2-test-viewer-00000000000000000000";
const BASE: &str = "/api/v1/brokers/kubernetes/session-reservations";

struct Case {
    directory: tempfile::TempDir,
    store: Arc<Store>,
    intent: KubernetesSessionReservationIntentV2,
    router: Router,
}
fn case() -> Case {
    case_with_capacity(None)
}
fn case_with_capacity(capacity: Option<crate::audit::WorkloadAuditPolicyV1>) -> Case {
    let f = Fixture::new();
    fixture::prepare(&f);
    let intent = fixture::intent(&f, Uuid::new_v4());
    let policy = fixture::policy(&intent);
    let store = Arc::new(match capacity {
        Some(capacity) => f.store.with_workload_audit_policy(capacity).unwrap(),
        None => f.store,
    });
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "tenant-a".into(),
            store: store.clone(),
            runner_token: RUNNER.into(),
            viewer_token: VIEWER.into(),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap()
    .with_kubernetes_session_reservation_policies(vec![("tenant-a".into(), policy)])
    .unwrap();
    Case {
        directory: f._directory,
        store,
        intent,
        router: gateway_router(state),
    }
}

fn close(intent: &KubernetesSessionReservationIntentV2) -> KubernetesSessionReservationCloseV2 {
    KubernetesSessionReservationCloseV2 {
        version: 2,
        operation_id: Uuid::new_v4().to_string(),
        reservation_id: intent.reservation_id().into(),
        reservation_input_hash: intent.input_hash("tenant-a").unwrap(),
        expected_revision: 1,
        reason: KubernetesSessionReservationCloseReasonV2::PreparationAbandoned,
    }
}

#[tokio::test]
async fn saturated_audit_refuses_abandonment_without_releasing_quota() {
    use crate::audit::{AuditCapacityV1, WorkloadAuditPolicyV1};
    let case = case_with_capacity(Some(WorkloadAuditPolicyV1 {
        version: 1,
        tenant: AuditCapacityV1 {
            events: 3,
            bytes: 3_000_000,
        },
        principal: AuditCapacityV1 {
            events: 1,
            bytes: 1_000_000,
        },
    }));
    assert_eq!(post(&case, BASE, &case.intent).await.0, StatusCode::CREATED);
    let head = case.store.head().unwrap();
    let quota = case
        .store
        .worker_quota(&crate::control::QuotaPolicyV1::default())
        .unwrap();
    let request = close(&case.intent);
    let (status, _) = post(
        &case,
        &format!("{BASE}/{}/abandonments", case.intent.reservation_id()),
        &request,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(case.store.head().unwrap(), head);
    assert_eq!(
        case.store
            .worker_quota(&crate::control::QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        quota.held_workers
    );
    let (status, replay) = post(&case, BASE, &case.intent).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["replayed"], true);
    assert_eq!(case.store.head().unwrap(), head);
}

#[tokio::test]
async fn changed_reservation_and_wrong_broker_cannot_overwrite_the_original() {
    use crate::{
        broker_registry::{BrokerAuthorityV1, BrokerGrantMutationV1, BrokerRuntimeKindV1},
        workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
    };
    use sha2::{Digest, Sha256};
    let case = case();
    assert_eq!(post(&case, BASE, &case.intent).await.0, StatusCode::CREATED);
    let head = case.store.head().unwrap();
    let mut changed = serde_json::to_value(&case.intent).unwrap();
    changed["worker_id"] = json!(Uuid::new_v4().to_string());
    assert_eq!(post(&case, BASE, &changed).await.0, StatusCode::CONFLICT);
    assert_eq!(case.store.head().unwrap(), head);
    let stranger = "public-r2-stranger-broker-token-0000000000000000000000";
    let broker_id = Uuid::new_v4().to_string();
    let incarnation_id = Uuid::new_v4().to_string();
    let credential_id = Uuid::new_v4().to_string();
    case.store
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential_id.clone(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: broker_id.clone(),
                incarnation_id: incarnation_id.clone(),
            },
            sha256_digest: hex::encode(Sha256::digest(stranger.as_bytes())),
            expires_at: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    let mut grant = BrokerGrantMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        broker_id: broker_id.clone(),
        expected_revision: None,
        credential_id,
        incarnation_id,
        authorities: vec![BrokerAuthorityV1 {
            authority_id: case.intent.authority().authority_id().to_string(),
            runtime_kind: BrokerRuntimeKindV1::Kubernetes,
        }],
        lease_seconds: 120,
        not_after: "2100-01-01T00:00:00.000Z".into(),
    };
    // An authority already owned by the first broker cannot prepare a valid
    // second-broker fixture. Assert that rejection before granting its own
    // authority, so the HTTP check below reaches a genuinely authenticated peer.
    let head = case.store.head().unwrap();
    let error = case
        .store
        .mutate_broker_grant(&grant)
        .err()
        .expect("authority reassignment must conflict");
    assert_eq!(
        error.downcast_ref::<crate::broker_registry::BrokerRegistryFailureV1>(),
        Some(&crate::broker_registry::BrokerRegistryFailureV1::Conflict)
    );
    assert_eq!(case.store.head().unwrap(), head);
    assert!(case.store.broker(&broker_id).unwrap().is_none());
    assert_eq!(
        case.store
            .broker_for_authority(&grant.authorities[0].authority_id)
            .unwrap()
            .unwrap()
            .broker
            .broker_id,
        case.intent.ids().unwrap().broker_id.to_string()
    );
    grant.operation_id = Uuid::new_v4().to_string();
    grant.authorities[0].authority_id = Uuid::new_v4().to_string();
    assert_ne!(
        grant.authorities[0].authority_id,
        case.intent.authority().authority_id().to_string()
    );
    case.store.mutate_broker_grant(&grant).unwrap();
    let identity = match case
        .store
        .authenticate_workload_token(stranger)
        .unwrap()
        .unwrap()
    {
        crate::workload::AuthenticatedWorkload::Broker(identity) => identity,
        _ => panic!("stranger fixture must authenticate its real broker binding and grant"),
    };
    assert_eq!(identity.tenant_id(), "tenant-a");
    assert_eq!(identity.broker_id(), broker_id.as_str());
    let head = case.store.head().unwrap();
    let response = case
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(BASE)
                .header(header::AUTHORIZATION, format!("Bearer {stranger}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&case.intent).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(case.store.head().unwrap(), head);
    let (status, replay) = post(&case, BASE, &case.intent).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["replayed"], true);
    assert_eq!(case.store.head().unwrap(), head);
    assert_eq!(
        case.store
            .worker_quota(&crate::control::QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
}

#[tokio::test]
async fn authenticated_tenant_b_cannot_select_tenant_a_by_body_or_header() {
    use crate::{
        broker_registry::{BrokerAuthorityV1, BrokerGrantMutationV1, BrokerRuntimeKindV1},
        workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
    };
    use sha2::{Digest, Sha256};
    let case = case();
    assert_eq!(post(&case, BASE, &case.intent).await.0, StatusCode::CREATED);
    let path = case.directory.path().join("tenant-b.sqlite");
    let store = Store::open_for_tenant(&path, "tenant-b", Redactor::new(vec![]), 10_000).unwrap();
    store
        .migrate_control_v3(&path.with_extension("before3"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("before4"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("before5"))
        .unwrap();
    store
        .migrate_workload_v6(&path.with_extension("before6"))
        .unwrap();
    store
        .migrate_human_brokers_v7(&path.with_extension("before7"))
        .unwrap();
    store
        .migrate_kubernetes_create_v8(&path.with_extension("before8"))
        .unwrap();
    store
        .migrate_kubernetes_policy_v9(&path.with_extension("before9"))
        .unwrap();
    store
        .migrate_human_workers_v10(&path.with_extension("before10"))
        .unwrap();
    store
        .migrate_kubernetes_session_reservations_v11(&path.with_extension("before11"))
        .unwrap();
    let token = "public-r2-tenant-b-broker-token-0000000000000000000000";
    let broker_id = Uuid::new_v4().to_string();
    let incarnation_id = Uuid::new_v4().to_string();
    let credential_id = Uuid::new_v4().to_string();
    store
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
            expires_at: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    store
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id,
            expected_revision: None,
            credential_id,
            incarnation_id,
            authorities: vec![BrokerAuthorityV1 {
                authority_id: case.intent.authority().authority_id().to_string(),
                runtime_kind: BrokerRuntimeKindV1::Kubernetes,
            }],
            lease_seconds: 120,
            not_after: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    let store = Arc::new(store);
    let state = GatewayState::for_tenants(
        vec![
            TenantGateway {
                tenant_id: "tenant-a".into(),
                store: case.store.clone(),
                runner_token: RUNNER.into(),
                viewer_token: VIEWER.into(),
            },
            TenantGateway {
                tenant_id: "tenant-b".into(),
                store: store.clone(),
                runner_token: "public-r2-b-runner-0000000000000000000000".into(),
                viewer_token: "public-r2-b-viewer-0000000000000000000000".into(),
            },
        ],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap()
    .with_kubernetes_session_reservation_policies(vec![
        ("tenant-a".into(), fixture::policy(&case.intent)),
        ("tenant-b".into(), fixture::policy(&case.intent)),
    ])
    .unwrap();
    let heads = (case.store.head().unwrap(), store.head().unwrap());
    let response = gateway_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(BASE)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("x-tenant-id", "tenant-a")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&case.intent).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!((case.store.head().unwrap(), store.head().unwrap()), heads);
    assert_eq!(
        store
            .worker_quota(&crate::control::QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0
    );
}

#[tokio::test]
async fn successor_abandonment_uses_only_the_gateway_tenant_policy() {
    use crate::{
        broker_registry::{
            BrokerAuthorityV1, BrokerGrantMutationV1, BrokerRevocationV1, BrokerRuntimeKindV1,
        },
        workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
    };
    use sha2::{Digest, Sha256};
    let case = case();
    assert_eq!(post(&case, BASE, &case.intent).await.0, StatusCode::CREATED);
    let close = close(&case.intent);
    let old = match case
        .store
        .authenticate_workload_token(TOKEN)
        .unwrap()
        .unwrap()
    {
        crate::workload::AuthenticatedWorkload::Broker(identity) => identity,
        _ => panic!("broker fixture"),
    };
    case.store
        .revoke_broker(&BrokerRevocationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: old.broker_id().into(),
            expected_revision: old.grant_revision(),
        })
        .unwrap();
    let token = "public-r2-successor-broker-token-0000000000000000000000";
    let credential_id = Uuid::new_v4().to_string();
    let incarnation_id = Uuid::new_v4().to_string();
    case.store
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential_id.clone(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: old.broker_id().into(),
                incarnation_id: incarnation_id.clone(),
            },
            sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
            expires_at: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    case.store
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: old.broker_id().into(),
            expected_revision: Some(old.grant_revision() + 1),
            credential_id,
            incarnation_id,
            authorities: vec![BrokerAuthorityV1 {
                authority_id: Uuid::new_v4().to_string(),
                runtime_kind: BrokerRuntimeKindV1::Kubernetes,
            }],
            lease_seconds: 120,
            not_after: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    let path = format!("{BASE}/{}/abandonments", case.intent.reservation_id());
    let request = || {
        Request::builder()
            .method("POST")
            .uri(&path)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&close).unwrap()))
            .unwrap()
    };
    let head = case.store.head().unwrap();
    assert_eq!(
        case.router
            .clone()
            .oneshot(request())
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(case.store.head().unwrap(), head);
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "tenant-a".into(),
            store: case.store.clone(),
            runner_token: RUNNER.into(),
            viewer_token: VIEWER.into(),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap()
    .with_kubernetes_session_recovery_policies(vec![(
        "tenant-a".into(),
        KubernetesSessionRecoveryPolicyV2 {
            version: 2,
            decision_ttl_seconds: 60,
            allowed_broker_ids: vec![old.broker_id().into()],
        },
    )])
    .unwrap();
    let router = gateway_router(state);
    assert_eq!(
        router.clone().oneshot(request()).await.unwrap().status(),
        StatusCode::OK
    );
    let head = case.store.head().unwrap();
    assert_eq!(
        router.oneshot(request()).await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(case.store.head().unwrap(), head);
    assert_eq!(
        case.store
            .worker_quota(&crate::control::QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0
    );
}

#[derive(Clone)]
struct LostAcks(Arc<AtomicUsize>);
async fn lose_mutation_acks(
    State(state): State<LostAcks>,
    request: Request<Body>,
    next: axum::middleware::Next,
) -> Response {
    let mutation = request.method() == axum::http::Method::POST
        && (request.uri().path() == BASE || request.uri().path().ends_with("/abandonments"));
    let response = next.run(request).await;
    if mutation && response.status().is_success() {
        let index = state.0.fetch_add(1, Ordering::SeqCst);
        if index == 0 || index == 2 {
            // Real Store commits first. Only the transport ACK is cut; no
            // store fixture, guard or proof stands in for that transaction.
            return (
                response.status(),
                [(header::CONTENT_TYPE, "application/json")],
                "{",
            )
                .into_response();
        }
    }
    response
}

async fn client_for(case: &Case, gateway: String) -> SessionControlClientV2 {
    let token = case.directory.path().join("client-token");
    fs::write(&token, TOKEN).unwrap();
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    let ids = case.intent.ids().unwrap();
    let credential_id = match case
        .store
        .authenticate_workload_token(TOKEN)
        .unwrap()
        .unwrap()
    {
        crate::workload::AuthenticatedWorkload::Broker(identity) => {
            identity.credential_id().to_owned()
        }
        _ => panic!("broker fixture"),
    };
    SessionControlClientV2::new(
        HeldCreateClientScope {
            gateway,
            tenant_id: "tenant-a".into(),
            control_namespace_uuid: ids.control_namespace_uuid.to_string(),
            broker_id: ids.broker_id.to_string(),
            credential_id,
            incarnation_id: ids.incarnation_id.to_string(),
        },
        PrivateFile::open(&token, 4097).unwrap(),
        None,
        vec![],
    )
    .unwrap()
}

#[tokio::test]
async fn lost_real_reserve_and_close_acks_retry_only_the_same_durable_requests() {
    let case = case();
    let lost = Arc::new(AtomicUsize::new(0));
    let router = case
        .router
        .clone()
        .layer(axum::middleware::from_fn_with_state(
            LostAcks(lost.clone()),
            lose_mutation_acks,
        ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = client_for(&case, format!("http://{}", listener.local_addr().unwrap())).await;
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    assert!(matches!(
        client.reserve(&case.intent).await,
        Err(crate::broker::held_create_client::sessions::SessionControlErrorV2::Protocol)
    ));
    assert_eq!(
        lost.load(Ordering::SeqCst),
        1,
        "no automatic mutation retry"
    );
    let head = case.store.head().unwrap();
    let replay = client.reserve(&case.intent).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(case.store.head().unwrap(), head);
    let root = Directory::open_or_create(&case.directory.path().join("lost-ack-journal")).unwrap();
    let protection = || SessionJournalProtection::new(vec![], vec![TOKEN.into()]).unwrap();
    let confirmed = client.confirm(&case.intent).await.unwrap();
    let journal = crate::runtime::kubernetes::session::SessionJournal::prepare_or_resume(
        root.clone(),
        protection(),
        &case.intent,
        &confirmed,
    )
    .unwrap();
    let close = remote::retain_abandonment(&client, journal).await.unwrap();
    assert!(matches!(
        client.abandon(&case.intent, &close).await,
        Err(crate::broker::held_create_client::sessions::SessionControlErrorV2::Protocol)
    ));
    let head = case.store.head().unwrap();
    let replay = remote::resume_abandonment(&client, root, protection())
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.receipt.operation_id, close.operation_id);
    assert_eq!(case.store.head().unwrap(), head);
    assert_eq!(lost.load(Ordering::SeqCst), 4);
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}
async fn post(case: &Case, path: &str, value: &impl Serialize) -> (StatusCode, Value) {
    let response = case
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(value).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 128 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn broker_identity_precedes_path_and_body_for_session_lifecycle() {
    let case = case();
    let head = case.store.head().unwrap();
    for token in [None, Some("unknown"), Some(RUNNER), Some(VIEWER)] {
        let polls = Arc::new(AtomicUsize::new(0));
        let count = polls.clone();
        let body = Body::from_stream(stream::poll_fn(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            Poll::Ready(Some(Ok::<_, Infallible>(axum::body::Bytes::from_static(
                b"{",
            ))))
        }));
        let mut request = Request::builder()
            .method("POST")
            .uri(format!("{BASE}/invalid/confirmations"))
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let response = case
            .router
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if token.is_none() || token == Some("unknown") {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::FORBIDDEN
            }
        );
        assert_eq!(polls.load(Ordering::SeqCst), 0);
    }
    assert_eq!(case.store.head().unwrap(), head);
}

#[tokio::test]
async fn authenticated_incomplete_body_times_out_without_reserving_quota() {
    let case = case();
    let head = case.store.head().unwrap();
    let request = Request::builder()
        .method("POST")
        .uri(BASE)
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from_stream(stream::pending::<
            Result<axum::body::Bytes, Infallible>,
        >()))
        .unwrap();
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(7),
        case.router.clone().oneshot(request),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(case.store.head().unwrap(), head);
    assert_eq!(
        case.store
            .worker_quota(&crate::control::QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0
    );
}

#[tokio::test]
async fn owned_sources_require_the_unimplemented_native_broker_port_before_body() {
    let (_owned_files, state) = crate::api::owned_human_fixture_for_test();
    let polls = Arc::new(AtomicUsize::new(0));
    let count = polls.clone();
    let body = Body::from_stream(stream::poll_fn(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
        Poll::Ready(Some(Ok::<_, Infallible>(axum::body::Bytes::from_static(
            b"{",
        ))))
    }));
    let response = gateway_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(BASE)
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(polls.load(Ordering::SeqCst), 0);
    let bytes = axum::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        value["error"]["code"],
        "session_lifecycle_native17_unavailable"
    );
}

async fn change_confirmation_nonce(
    State(changed): State<Arc<AtomicUsize>>,
    request: Request<Body>,
    next: axum::middleware::Next,
) -> Response {
    let confirmation = request.uri().path().ends_with("/confirmations");
    let response = next.run(request).await;
    if confirmation
        && response.status() == StatusCode::OK
        && changed.fetch_add(1, Ordering::SeqCst) == 0
    {
        let bytes = axum::body::to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap();
        let mut value: Value = serde_json::from_slice(&bytes).unwrap();
        value["request_id"] = json!(Uuid::new_v4().to_string());
        return (StatusCode::OK, Json(value)).into_response();
    }
    response
}

#[tokio::test]
async fn changed_live_nonce_never_mints_a_local_preparation_from_a_dto() {
    let case = case();
    let changed = Arc::new(AtomicUsize::new(0));
    let router = case
        .router
        .clone()
        .layer(axum::middleware::from_fn_with_state(
            changed.clone(),
            change_confirmation_nonce,
        ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = client_for(&case, format!("http://{}", listener.local_addr().unwrap())).await;
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let root_path = case.directory.path().join("nonce-journal");
    let root = Directory::open_or_create(&root_path).unwrap();
    let protection = || SessionJournalProtection::new(vec![], vec![TOKEN.into()]).unwrap();
    assert!(matches!(
        remote::prepare(&client, root.clone(), protection(), case.intent.clone()).await,
        Err(remote::RemoteSessionErrorV2::Control(
            crate::broker::held_create_client::sessions::SessionControlErrorV2::Protocol
        ))
    ));
    assert_eq!(fs::read_dir(&root_path).unwrap().count(), 0);
    assert_eq!(
        case.store
            .worker_quota(&crate::control::QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
    let head = case.store.head().unwrap();
    let journal = remote::prepare(&client, root, protection(), case.intent.clone())
        .await
        .unwrap();
    assert_eq!(
        journal.phase().unwrap(),
        crate::runtime::kubernetes::session::SessionPhaseV2::Prepared
    );
    assert_eq!(changed.load(Ordering::SeqCst), 2);
    assert_eq!(case.store.head().unwrap(), head);
    drop(journal);
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn reservation_replay_confirmation_and_abandonment_use_real_store_cas() {
    let case = case();
    let (status, first) = post(&case, BASE, &case.intent).await;
    assert_eq!(status, StatusCode::CREATED);
    let head = case.store.head().unwrap();
    let (status, replay) = post(&case, BASE, &case.intent).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["receipt"], first["receipt"]);
    assert_eq!(case.store.head().unwrap(), head);
    let id = case.intent.reservation_id();
    let request = SessionConfirmationRequestV2 {
        version: 2,
        request_id: Uuid::new_v4().to_string(),
        reservation_input_hash: case.intent.input_hash("tenant-a").unwrap(),
    };
    let (status, confirmed) = post(&case, &format!("{BASE}/{id}/confirmations"), &request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(confirmed["request_id"], request.request_id);
    assert_eq!(
        confirmed["current"]["snapshot"]["state"]["effect_fence"]["state"],
        "never_issued"
    );
    assert_eq!(case.store.head().unwrap(), head);
    let close = KubernetesSessionReservationCloseV2 {
        version: 2,
        operation_id: Uuid::new_v4().to_string(),
        reservation_id: id.into(),
        reservation_input_hash: request.reservation_input_hash.clone(),
        expected_revision: 1,
        reason: KubernetesSessionReservationCloseReasonV2::PreparationAbandoned,
    };
    let path = format!("{BASE}/{id}/abandonments");
    let (status, closed) = post(&case, &path, &close).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(closed["snapshot"]["state"]["quota_held"], false);
    let closed_head = case.store.head().unwrap();
    let (_, replay) = post(&case, &path, &close).await;
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], closed["receipt"]);
    assert_eq!(case.store.head().unwrap(), closed_head);
    let (status, _) = post(&case, &format!("{BASE}/{id}/confirmations"), &request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn live_http_confirmation_prepares_reopens_and_resumes_retained_abandonment() {
    let case = case();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway = format!("http://{}", listener.local_addr().unwrap());
    let router = case.router.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let token = case.directory.path().join("r2-token");
    fs::write(&token, TOKEN).unwrap();
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    let ids = case.intent.ids().unwrap();
    let client = SessionControlClientV2::new(
        HeldCreateClientScope {
            gateway,
            tenant_id: "tenant-a".into(),
            control_namespace_uuid: ids.control_namespace_uuid.to_string(),
            broker_id: ids.broker_id.to_string(),
            credential_id: case
                .store
                .authenticate_workload_token(TOKEN)
                .unwrap()
                .and_then(|identity| match identity {
                    crate::workload::AuthenticatedWorkload::Broker(identity) => {
                        Some(identity.credential_id().into())
                    }
                    _ => None,
                })
                .unwrap(),
            incarnation_id: ids.incarnation_id.to_string(),
        },
        PrivateFile::open(&token, 4097).unwrap(),
        None,
        vec![],
    )
    .unwrap();
    let root = Directory::open_or_create(&case.directory.path().join("r2-journal")).unwrap();
    let protection = || SessionJournalProtection::new(vec![], vec![TOKEN.into()]).unwrap();
    let journal = remote::prepare(&client, root.clone(), protection(), case.intent.clone())
        .await
        .unwrap();
    let binding = journal.bootstrap_binding().unwrap();
    drop(journal);
    let journal = remote::prepare(&client, root.clone(), protection(), case.intent.clone())
        .await
        .unwrap();
    assert_eq!(journal.bootstrap_binding().unwrap(), binding);
    let close = remote::retain_abandonment(&client, journal).await.unwrap();
    // The exact retained close is the retry input; a lost response may not
    // replace its UUID or mint a new local preparation after central close.
    let first = client.abandon(&case.intent, &close).await.unwrap();
    let head = case.store.head().unwrap();
    let replay = remote::resume_abandonment(&client, root, protection())
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(first.receipt, replay.receipt);
    assert_eq!(case.store.head().unwrap(), head);
    assert_eq!(
        replay.snapshot.state.effect_fence,
        KubernetesSessionEffectFenceV2::NeverIssued { revision: 0 }
    );
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn operator_cli_prepares_and_abandons_only_the_before_effect_reservation() {
    #[derive(clap::Parser)]
    struct Cli {
        #[command(flatten)]
        args: crate::kubernetes_cli::KubernetesArgs,
    }
    let case = case();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway = format!("http://{}", listener.local_addr().unwrap());
    let router = case.router.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let ids = case.intent.ids().unwrap();
    let credential_id = match case
        .store
        .authenticate_workload_token(TOKEN)
        .unwrap()
        .unwrap()
    {
        crate::workload::AuthenticatedWorkload::Broker(identity) => {
            identity.credential_id().to_owned()
        }
        _ => panic!("broker fixture"),
    };
    let token = case.directory.path().join("cli-token");
    let control = case.directory.path().join("cli-control.json");
    let intent = case.directory.path().join("cli-intent.json");
    let root = case.directory.path().join("cli-journal");
    for (path, bytes) in [
        (&token, TOKEN.as_bytes().to_vec()),
        (&intent, serde_json::to_vec(&case.intent).unwrap()),
        (&control, serde_json::to_vec(&json!({"version":2,"gateway":gateway,"tenant_id":"tenant-a","control_namespace_uuid":ids.control_namespace_uuid.to_string(),"broker_id":ids.broker_id.to_string(),"incarnation_id":ids.incarnation_id.to_string(),"credential_id":credential_id,"broker_token_file":token})).unwrap()),
    ] { fs::write(path, bytes).unwrap(); fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap(); }
    for action in ["prepare", "status", "abandon", "abandon", "status"] {
        let mut argv = vec![
            "fixture".into(),
            "session".into(),
            action.into(),
            "--control-file".into(),
            control.as_os_str().to_owned(),
            "--journal-dir".into(),
            root.as_os_str().to_owned(),
        ];
        if action == "prepare" {
            argv.extend(["--intent-file".into(), intent.as_os_str().to_owned()]);
        }
        let cli = <Cli as clap::Parser>::try_parse_from(argv).unwrap();
        crate::kubernetes_cli::execute(cli.args).await.unwrap();
    }
    let view = crate::runtime::kubernetes::session::SessionJournal::inspect_retained(
        Directory::open_existing(&root).unwrap(),
        SessionJournalProtection::new(vec![], vec![TOKEN.into()]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        view.phase(),
        crate::runtime::kubernetes::session::SessionPhaseV2::Abandoned
    );
    assert!(view.abandonment().is_some());
    assert_eq!(
        case.store
            .worker_quota(&crate::control::QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0
    );
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn real_control_restart_reopens_reserved_journal_without_fresh_admission() {
    let case = case();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = client_for(&case, format!("http://{}", listener.local_addr().unwrap())).await;
    let router = case.router.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let root = Directory::open_or_create(&case.directory.path().join("restart-journal")).unwrap();
    let protection = || SessionJournalProtection::new(vec![], vec![TOKEN.into()]).unwrap();
    let journal = remote::prepare(&client, root.clone(), protection(), case.intent.clone())
        .await
        .unwrap();
    let binding = journal.bootstrap_binding().unwrap();
    drop(journal);
    let head = case.store.head().unwrap();
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    drop(client);
    let Case {
        directory,
        store,
        intent,
        router,
    } = case;
    drop(router);
    drop(store);
    let store = Arc::new(
        Store::open_existing_for_tenant(
            &directory.path().join("tenant.sqlite"),
            "tenant-a",
            Redactor::new(vec![]),
            10_000,
        )
        .unwrap(),
    );
    assert!(!store.supervision_state().unwrap().active);
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "tenant-a".into(),
            store: store.clone(),
            runner_token: RUNNER.into(),
            viewer_token: VIEWER.into(),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let case = Case {
        directory,
        store,
        intent,
        router: gateway_router(state),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = client_for(&case, format!("http://{}", listener.local_addr().unwrap())).await;
    let router = case.router.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let journal = remote::prepare(&client, root, protection(), case.intent.clone())
        .await
        .unwrap();
    assert_eq!(journal.bootstrap_binding().unwrap(), binding);
    assert_eq!(case.store.head().unwrap(), head);
    assert!(!case.store.supervision_state().unwrap().active);
    drop(journal);
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}
