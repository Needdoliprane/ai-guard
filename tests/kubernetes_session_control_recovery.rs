//! Effective HTTP extraction boundary against real, fresh tenant Stores.
//! No cluster, runtime, provider, or real credential is involved.
#[allow(dead_code)]
#[path = "support/relay_admission_fixture.rs"]
mod relay_admission_fixture;
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    control::{kubernetes_session::KubernetesSessionControlRecoveryPolicyV2, *},
    model::{CommandSpec, DecisionRequest},
    policy::{self, Policy},
    redaction::Redactor,
    relay::RelayEndpointV1,
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use axum::{
    Router,
    body::{Body, Bytes},
    http::{Request, StatusCode, header},
};
use futures_util::stream;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    convert::Infallible,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::Poll,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};
use tower::ServiceExt;
use uuid::Uuid;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const RUNNER_A: &str = "fixture-control-recovery-tenant-a-runner-20261108";
const VIEWER_A: &str = "fixture-control-recovery-tenant-a-viewer-20261108";
const RUNNER_B: &str = "fixture-control-recovery-tenant-b-runner-20261108";
const VIEWER_B: &str = "fixture-control-recovery-tenant-b-viewer-20261108";

const ID: &str = "11111111-1111-4111-8111-111111111111";
const PATH: &str =
    "/api/v1/kubernetes/session-reservations/11111111-1111-4111-8111-111111111111/control-recovery";

fn fixture() -> (tempfile::TempDir, GatewayState, Vec<Arc<Store>>) {
    let directory = support::private_tempdir();
    let mut stores = Vec::new();
    let mut tenants = Vec::new();
    for (tenant, runner, viewer) in [
        ("tenant-a", RUNNER_A, VIEWER_A),
        ("tenant-b", RUNNER_B, VIEWER_B),
    ] {
        let store = Arc::new(
            Store::open_for_tenant(
                &directory.path().join(format!("{tenant}.sqlite")),
                tenant,
                Redactor::new(vec![]),
                1000,
            )
            .unwrap(),
        );
        tenants.push(TenantGateway {
            tenant_id: tenant.into(),
            store: Arc::clone(&store),
            runner_token: runner.into(),
            viewer_token: viewer.into(),
        });
        stores.push(store);
    }
    let state =
        GatewayState::for_tenants(tenants, Policy::default(), None, Redactor::new(vec![])).unwrap();
    (directory, state, stores)
}

fn counted_body(polls: Arc<AtomicUsize>) -> Body {
    let mut sent = false;
    Body::from_stream(stream::poll_fn(move |_| {
        polls.fetch_add(1, Ordering::SeqCst);
        if sent {
            Poll::Ready(None)
        } else {
            sent = true;
            Poll::Ready(Some(Ok::<Bytes, Infallible>(Bytes::from_static(b"{"))))
        }
    }))
}

#[tokio::test]
async fn invalid_control_credentials_and_viewers_are_rejected_before_path_and_body() {
    // A body/Path extractor placed before the identity would break these cases.
    let (_directory, state, stores) = fixture();
    let router = gateway_router(state);
    let heads: Vec<_> = stores.iter().map(|store| store.head().unwrap()).collect();
    for path in [
        PATH,
        "/api/v1/kubernetes/session-reservations/not-a-uuid/control-recovery",
    ] {
        for (tokens, expected) in [
            (vec![], StatusCode::UNAUTHORIZED),
            (vec!["unknown".into()], StatusCode::UNAUTHORIZED),
            (vec![VIEWER_A.to_owned()], StatusCode::FORBIDDEN),
            (vec![VIEWER_B.to_owned()], StatusCode::FORBIDDEN),
            (vec!["x".repeat(4097)], StatusCode::UNAUTHORIZED),
            (
                vec![RUNNER_A.to_owned(), RUNNER_B.to_owned()],
                StatusCode::UNAUTHORIZED,
            ),
        ] {
            let polls = Arc::new(AtomicUsize::new(0));
            let mut request = Request::builder()
                .method("POST")
                .uri(path)
                .header(header::CONTENT_TYPE, "application/json")
                .body(counted_body(Arc::clone(&polls)))
                .unwrap();
            for token in tokens {
                request.headers_mut().append(
                    header::AUTHORIZATION,
                    format!("Bearer {token}").parse().unwrap(),
                );
            }
            assert_eq!(
                router.clone().oneshot(request).await.unwrap().status(),
                expected
            );
            assert_eq!(polls.load(Ordering::SeqCst), 0);
        }
    }
    assert_eq!(
        stores
            .iter()
            .map(|store| store.head().unwrap())
            .collect::<Vec<_>>(),
        heads
    );
}

#[tokio::test]
async fn real_human_worker_and_broker_credentials_cannot_become_a_runner_before_body() {
    use agentsandbox::{broker_registry::*, identity::*};
    let directory = support::private_tempdir();
    let store = relay_admission_fixture::fresh(directory.path(), "tenant-a", 10_000);
    let worker = relay_admission_fixture::running_now(&store);
    let worker_token = "public-control-recovery-worker-fixture-000000000000000000";
    relay_admission_fixture::bind(&store, &worker, worker_token);
    let broker_token = "public-control-recovery-broker-fixture-000000000000000000";
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
            sha256_digest: hex::encode(Sha256::digest(broker_token.as_bytes())),
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
                authority_id: "public-software-authority".into(),
                runtime_kind: BrokerRuntimeKindV1::Docker,
            }],
            lease_seconds: 120,
            not_after: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    let human_token = generate_local_credential().unwrap();
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: Uuid::new_v4().to_string(),
        account_id: Uuid::new_v4().to_string(),
    };
    let changes = [
        IdentityChangeV1::Account {
            expected_revision: 0,
            account: HumanAccountV1 {
                version: 1,
                principal: principal.clone(),
                display_name: "Public fixture".into(),
                revision: 1,
                active: true,
            },
        },
        IdentityChangeV1::Credential {
            expected_revision: 0,
            binding: LocalCredentialV1 {
                version: 1,
                credential_id: Uuid::new_v4().to_string(),
                principal: principal.clone(),
                sha256_digest: hex::encode(Sha256::digest(human_token.as_bytes())),
                revision: 1,
                active: true,
                expires_at: None,
            },
        },
        IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: MembershipV1 {
                version: 1,
                membership_id: Uuid::new_v4().to_string(),
                principal,
                tenant_id: "tenant-a".into(),
                organization_scope: "fixture".into(),
                revision: 1,
                active: true,
                capabilities: std::collections::BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
            },
        },
    ];
    store
        .provision_local_human(&LocalHumanProvisionV1 {
            version: 1,
            mutations: changes.map(|change| IdentityMutationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                actor: PrincipalRefV1::LegacyControl {
                    credential_id: "public-fixture-control".into(),
                    role: LegacyControlRoleV1::Runner,
                },
                change,
            }),
        })
        .unwrap();
    let store = Arc::new(store);
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "tenant-a".into(),
            store: Arc::clone(&store),
            runner_token: RUNNER_A.into(),
            viewer_token: VIEWER_A.into(),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let router = gateway_router(state);
    for (path, token) in [
        ("/api/v1/human/identity", human_token.as_str()),
        ("/api/v1/relay/messages", worker_token),
        ("/api/v1/brokers/identity", broker_token),
    ] {
        let positive = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            positive.status(),
            StatusCode::OK,
            "the foreign identity fixture must authenticate"
        );
    }
    let head = store.head().unwrap();
    for token in [human_token.as_str(), worker_token, broker_token] {
        let polls = Arc::new(AtomicUsize::new(0));
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/kubernetes/session-reservations/not-a-uuid/control-recovery")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(counted_body(Arc::clone(&polls)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(polls.load(Ordering::SeqCst), 0);
    }
    assert_eq!(store.head().unwrap(), head);
}

async fn post(router: &Router, path: &str, input: Value) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {RUNNER_A}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&input).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 131_072)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn request_contract_is_closed_and_default_policy_denies_without_mutating_audit() {
    let (_directory, state, stores) = fixture();
    let router = gateway_router(state);
    let heads: Vec<_> = stores.iter().map(|store| store.head().unwrap()).collect();
    let observe =
        json!({"version":2,"action":"observe_reserved","reservation_input_hash":"c".repeat(64)});
    assert_eq!(
        post(&router, PATH, observe.clone()).await.0,
        StatusCode::FORBIDDEN
    );
    for field in ["tenant_id", "policy", "role", "broker_id", "charge"] {
        let mut extra = observe.clone();
        extra[field] = json!("tenant-b");
        assert_eq!(post(&router, PATH, extra).await.0, StatusCode::BAD_REQUEST);
    }
    let mut wrong_version = observe.clone();
    wrong_version["version"] = json!(1);
    assert_eq!(
        post(&router, PATH, wrong_version).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut unknown_action = observe.clone();
    unknown_action["action"] = json!("reserve");
    assert_eq!(
        post(&router, PATH, unknown_action).await.0,
        StatusCode::BAD_REQUEST
    );
    let close = json!({"version":2,"action":"close_before_effect","close":{
        "version":2,"operation_id":"33333333-3333-4333-8333-333333333333",
        "reservation_id":"22222222-2222-4222-8222-222222222222","reservation_input_hash":"c".repeat(64),
        "expected_revision":1,"reason":"operator_cancelled"
    }});
    assert_eq!(post(&router, PATH, close).await.0, StatusCode::BAD_REQUEST);
    let alias = format!("/api/kubernetes/session-reservations/{ID}/control-recovery");
    assert_eq!(
        post(&router, &alias, observe).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        stores
            .iter()
            .map(|store| store.head().unwrap())
            .collect::<Vec<_>>(),
        heads
    );
}

#[tokio::test]
async fn duplicate_fields_and_oversized_requests_are_client_errors_after_real_runner_auth() {
    let (_directory, state, _stores) = fixture();
    let router = gateway_router(state);
    for body in [
        r#"{"version":2,"version":2,"action":"observe_reserved","reservation_input_hash":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"}"#.to_owned(),
        " ".repeat(80 * 1024 + 1),
    ] {
        let response = router.clone().oneshot(Request::builder().method("POST").uri(PATH)
            .header(header::AUTHORIZATION, format!("Bearer {RUNNER_A}"))
            .header(header::CONTENT_TYPE, "application/json").body(Body::from(body)).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn real_tcp_rejects_before_the_peer_sends_its_declared_body() {
    let (_directory, state, _stores) = fixture();
    let retirement = Arc::new(state.control_recovery_retirement());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    let owner = Arc::clone(&retirement);
    let server = tokio::spawn(async move {
        let result = axum::serve(listener, gateway_router(state))
            .with_graceful_shutdown(async move {
                let _ = stopped.await;
                owner.retire();
            })
            .await;
        result.unwrap();
    });
    for authorization in [
        String::new(),
        format!("Authorization: Bearer {VIEWER_A}\r\n"),
    ] {
        let mut socket = TcpStream::connect(address).await.unwrap();
        socket.write_all(format!("POST {PATH} HTTP/1.1\r\nHost: 127.0.0.1\r\n{authorization}Content-Type: application/json\r\nContent-Length: 1024\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut reader = BufReader::new(socket);
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
            .await
            .expect("authorization waited for the body")
            .unwrap();
        let expected = if authorization.is_empty() {
            "401"
        } else {
            "403"
        };
        assert!(
            line.starts_with(&format!("HTTP/1.1 {expected} ")),
            "{line:?}"
        );
    }
    shutdown.send(()).unwrap();
    server.await.unwrap();
    assert!(retirement.drained());
}

#[test]
fn control_policy_builder_rejects_bad_tenant_mapping() {
    let (_directory, state, _stores) = fixture();
    let policy = KubernetesSessionControlRecoveryPolicyV2 {
        version: 2,
        decision_ttl_seconds: 60,
        allowed_broker_ids: vec![],
    };
    assert!(
        state
            .with_kubernetes_session_control_recovery_policies(vec![(
                "tenant-missing".into(),
                policy
            )])
            .is_err()
    );
}
