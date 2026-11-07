//! Real HTTP/SQLite profile checks. Worker supervision records are synthetic;
//! no helper, container, provider or runtime transport is executed.
use super::*;
use agentsandbox::{
    api::gateway_control_router,
    ingress::ingress_router_with_control,
    relay::client::{RelayClientError, RelayClientProfile, WorkerRelayClient},
};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Server {
    origin: String,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    async fn start(app: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { origin, task }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn control_app(a: Arc<Store>, b: Arc<Store>) -> Router {
    gateway_control_router(
        GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: a,
                    runner_token: "a".repeat(48),
                    viewer_token: "v".repeat(48),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: b,
                    runner_token: "b".repeat(48),
                    viewer_token: "w".repeat(48),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap(),
    )
}

#[derive(Debug)]
struct ObservedRequest {
    method: axum::http::Method,
    target: String,
    proof: Option<u8>,
    json_headers: bool,
    body: Option<Value>,
}
type Requests = Arc<tokio::sync::Mutex<Vec<ObservedRequest>>>;

fn recording(app: Router, requests: Requests, source: &str, destination: &str) -> Router {
    let source = format!("Bearer {source}");
    let destination = format!("Bearer {destination}");
    app.layer(axum::middleware::from_fn(
        move |request: Request<Body>, next: axum::middleware::Next| {
            let source = source.clone();
            let destination = destination.clone();
            let requests = requests.clone();
            async move {
                let (parts, body) = request.into_parts();
                let bytes = axum::body::to_bytes(body, 262_144).await.unwrap();
                let proof = match parts.headers.get("authorization") {
                    Some(value) if value == source.as_str() => Some(1),
                    Some(value) if value == destination.as_str() => Some(2),
                    _ => None,
                };
                let json_headers = parts
                    .headers
                    .get("accept")
                    .is_some_and(|v| v == "application/json")
                    && parts
                        .headers
                        .get("cache-control")
                        .is_some_and(|v| v == "no-store")
                    && (bytes.is_empty()
                        || parts
                            .headers
                            .get("content-type")
                            .is_some_and(|v| v == "application/json"));
                requests.lock().await.push(ObservedRequest {
                    method: parts.method.clone(),
                    target: parts
                        .extensions
                        .get::<axum::extract::OriginalUri>()
                        .map_or_else(|| parts.uri.to_string(), |original| original.0.to_string()),
                    proof,
                    json_headers,
                    body: if bytes.is_empty() {
                        None
                    } else {
                        Some(serde_json::from_slice(&bytes).unwrap())
                    },
                });
                next.run(Request::from_parts(parts, Body::from(bytes)))
                    .await
            }
        },
    ))
}

#[tokio::test]
async fn relay_client_profiles_keep_fixed_paths_credentials_and_closed_wire_dtos() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 2000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 2000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let token1 = "1".repeat(48);
    let token2 = "2".repeat(48);
    bind(&a, &a1, &token1);
    bind(&a, &a2, &token2);
    let grant = directed(&a1, &a2, 300);
    a.relay_grant(&grant).unwrap();
    let requests = Requests::default();
    let root = Server::start(recording(
        router(a.clone(), b.clone()),
        requests.clone(),
        &token1,
        &token2,
    ))
    .await;
    let gateway = Server::start(control_app(a.clone(), b)).await;
    let public_calls = Arc::new(AtomicUsize::new(0));
    let count = public_calls.clone();
    let public = Server::start(Router::new().fallback(move || {
        let count = count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            StatusCode::IM_A_TEAPOT
        }
    }))
    .await;
    let ingress = Server::start(recording(
        ingress_router_with_control(&public.origin, &gateway.origin).unwrap(),
        requests.clone(),
        &token1,
        &token2,
    ))
    .await;
    for (profile, origin, path) in [
        (None, &root.origin, "/api/v1/relay/messages"),
        (
            Some(RelayClientProfile::RootOrigin),
            &root.origin,
            "/api/v1/relay/messages",
        ),
        (
            Some(RelayClientProfile::ControlIngress),
            &ingress.origin,
            "/control/api/v1/relay/messages",
        ),
    ] {
        let make = |token: &str, endpoint| match profile {
            None => WorkerRelayClient::new(origin, token, endpoint),
            Some(profile) => WorkerRelayClient::with_profile(origin, token, endpoint, profile),
        };
        let source = make(&token1, endpoint(&a1)).unwrap();
        let destination = make(&token2, endpoint(&a2)).unwrap();
        let payload = send(&grant, "profile round trip");
        let receipt = source.send(&payload).await.unwrap();
        assert_eq!(receipt.operation_id, payload.operation_id);
        assert!(!receipt.replayed);
        let inbox = destination.inbox(Some(0), 32).await.unwrap();
        assert_eq!(inbox.items.len(), 1);
        assert_eq!(inbox.items[0].body, "profile round trip");
        assert_eq!(inbox.items[0].metadata.source, endpoint(&a1));
        assert_eq!(inbox.items[0].metadata.destination, endpoint(&a2));
        let sequence = inbox.items[0].sequence;
        assert!(
            destination
                .inbox(Some(sequence), 1)
                .await
                .unwrap()
                .items
                .is_empty()
        );
        let ack = RelayAckV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            message_id: payload.operation_id.clone(),
        };
        let receipt = destination.ack(&ack).await.unwrap();
        assert_eq!(receipt.operation_id, ack.operation_id);
        assert_eq!(receipt.resource_id, ack.message_id);
        assert!(destination.inbox(None, 1).await.unwrap().items.is_empty());
        let captured = std::mem::take(&mut *requests.lock().await);
        assert_eq!(captured.len(), 5);
        for (index, (method, target, proof, body)) in [
            ("POST", path.to_owned(), 1, Some(json!({"version":1,"operation_id":payload.operation_id,"grant_id":payload.grant_id,"ttl_seconds":60,"body":"profile round trip"}))),
            ("GET", format!("{path}?limit=32&after=0"), 2, None),
            ("GET", format!("{path}?limit=1&after={sequence}"), 2, None),
            ("POST", format!("{path}/{}/ack", ack.message_id), 2, Some(json!({"version":1,"operation_id":ack.operation_id,"message_id":ack.message_id}))),
            ("GET", format!("{path}?limit=1"), 2, None),
        ].into_iter().enumerate() {
            assert_eq!(captured[index].method, method);
            assert_eq!(captured[index].target, target);
            assert_eq!(captured[index].proof, Some(proof));
            assert!(captured[index].json_headers);
            assert_eq!(captured[index].body, body);
        }
    }
    assert_eq!(public_calls.load(Ordering::SeqCst), 0);
    assert!(a.integrity().unwrap().valid);
}

#[test]
fn relay_client_profiles_refuse_caller_paths_and_origin_aliases() {
    let endpoint = RelayEndpointV1 {
        worker_id: Uuid::new_v4().to_string(),
        attempt_id: Uuid::new_v4().to_string(),
        generation: 1,
    };
    let proof = "1".repeat(48);
    for origin in [
        "http://127.0.0.1/control",
        "http://127.0.0.1/control/",
        "http://127.0.0.1/api/v1/relay/messages",
        "http://127.0.0.1/a/..",
        "http://127.0.0.1//",
        "http://127.0.0.1/%2fcontrol",
        "http://127.0.0.1\\control",
        "http://name:password@127.0.0.1",
        "http://127.0.0.1?limit=1",
        "http://127.0.0.1#control",
        "http://example.com",
        "http://localhost:8080",
    ] {
        assert!(matches!(
            WorkerRelayClient::new(origin, &proof, endpoint.clone()),
            Err(RelayClientError::InvalidConfiguration)
        ));
        for profile in [
            RelayClientProfile::RootOrigin,
            RelayClientProfile::ControlIngress,
        ] {
            assert!(matches!(
                WorkerRelayClient::with_profile(origin, &proof, endpoint.clone(), profile),
                Err(RelayClientError::InvalidConfiguration)
            ));
            assert!(matches!(
                WorkerRelayClient::with_profile_and_ca_file(
                    origin,
                    &proof,
                    endpoint.clone(),
                    profile,
                    Path::new("unused-public-ca.pem")
                ),
                Err(RelayClientError::InvalidConfiguration)
            ));
        }
    }
}

#[test]
fn relay_client_root_origin_refuses_empty_userinfo_before_transport() {
    let endpoint = RelayEndpointV1 {
        worker_id: Uuid::new_v4().to_string(),
        attempt_id: Uuid::new_v4().to_string(),
        generation: 1,
    };
    let rejected = ["http://@127.0.0.1", "http:@127.0.0.1", "http:/@127.0.0.1"].map(|origin| {
        matches!(
            WorkerRelayClient::new(origin, &"1".repeat(48), endpoint.clone()),
            Err(RelayClientError::InvalidConfiguration)
        )
    });
    assert_eq!(
        rejected, [true; 3],
        "raw userinfo aliases must all be refused"
    );
}

#[tokio::test]
async fn relay_client_control_profile_refuses_proof_in_ack_target_before_transport() {
    // This valid 32-byte dummy proof is absent from the input DTO and origin,
    // but occurs in the fixed prefix followed by this message UUID.
    let proof = "/control/api/v1/relay/messages/1";
    let ack = RelayAckV1 {
        version: 1,
        operation_id: "22222222-2222-4222-8222-222222222222".into(),
        message_id: "11111111-1111-4111-8111-111111111111".into(),
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let receipt = RelayReceiptV1 {
        version: 1,
        operation_id: ack.operation_id.clone(),
        resource_id: ack.message_id.clone(),
        audit_seq: 1,
        audit_hash: HASH_A.into(),
        replayed: false,
    };
    let server = Server::start(Router::new().fallback(move || {
        let count = count.clone();
        let receipt = receipt.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            (StatusCode::OK, axum::Json(receipt))
        }
    }))
    .await;
    let client = WorkerRelayClient::with_profile(
        &server.origin,
        proof,
        RelayEndpointV1 {
            worker_id: Uuid::new_v4().to_string(),
            attempt_id: Uuid::new_v4().to_string(),
            generation: 1,
        },
        RelayClientProfile::ControlIngress,
    )
    .unwrap();
    let error = client.ack(&ack).await.err();
    let local_refusal = error == Some(RelayClientError::InvalidRequest);
    let opaque = error.is_some_and(|error| !format!("{error:?}: {error}").contains(proof));
    let peer_requests = calls.load(Ordering::SeqCst);
    assert!(
        local_refusal && opaque && peer_requests == 0,
        "constructed target: local_refusal={local_refusal}, opaque={opaque}, peer_requests={peer_requests}"
    );
}

#[tokio::test]
async fn relay_client_control_profile_keeps_role_and_expected_endpoint_boundaries() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 2000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 2000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let b1 = running_worker(&b);
    let token1 = "1".repeat(48);
    let token2 = "2".repeat(48);
    let foreign = "3".repeat(48);
    bind(&a, &a1, &token1);
    bind(&a, &a2, &token2);
    bind(&b, &b1, &foreign);
    let human = "h".repeat(48);
    bind_local_human_metadata(&a, &human);
    let grant = directed(&a1, &a2, 300);
    a.relay_grant(&grant).unwrap();
    let server = Server::start(router(a.clone(), b.clone())).await;
    let make = |proof: &str, endpoint| {
        WorkerRelayClient::with_profile(
            &server.origin,
            proof,
            endpoint,
            RelayClientProfile::ControlIngress,
        )
        .unwrap()
    };
    let source = make(&token1, endpoint(&a1));
    let payload = send(&grant, "bound to the authenticated worker");
    source.send(&payload).await.unwrap();
    let ack = RelayAckV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        message_id: payload.operation_id.clone(),
    };
    let heads = (a.head().unwrap(), b.head().unwrap());
    for proof in [&"a".repeat(48), &"v".repeat(48), &human] {
        let wrong_role = make(proof, endpoint(&a2));
        assert_eq!(
            wrong_role.send(&payload).await.unwrap_err(),
            RelayClientError::Forbidden
        );
        assert!(matches!(
            wrong_role.inbox(None, 32).await,
            Err(RelayClientError::Forbidden)
        ));
        assert_eq!(
            wrong_role.ack(&ack).await.unwrap_err(),
            RelayClientError::Forbidden
        );
    }
    assert_eq!((a.head().unwrap(), b.head().unwrap()), heads);
    let other = make(&foreign, endpoint(&a2));
    assert_eq!(
        other.send(&payload).await.unwrap_err(),
        RelayClientError::Forbidden
    );
    assert!(other.inbox(None, 32).await.unwrap().items.is_empty());
    let wrong_expectation = make(&token2, endpoint(&b1));
    let error = wrong_expectation.inbox(None, 32).await.err().unwrap();
    assert_eq!(error, RelayClientError::Protocol);
    assert!(error.outcome_unknown());
    let destination = make(&token2, endpoint(&a2));
    assert_eq!(destination.inbox(None, 32).await.unwrap().items.len(), 1);
    assert!(a.integrity().unwrap().valid);
    assert!(b.integrity().unwrap().valid);
}

#[tokio::test]
async fn relay_client_control_profile_parses_near_two_mib_inbox_without_losing_pagination() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 2000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 2000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let token1 = "1".repeat(48);
    let token2 = "2".repeat(48);
    bind(&a, &a1, &token1);
    bind(&a, &a2, &token2);
    let grant = directed(&a1, &a2, 300);
    a.relay_grant(&grant).unwrap();
    let gateway = Server::start(control_app(a.clone(), b)).await;
    let public = Server::start(Router::new().fallback(|| async { StatusCode::IM_A_TEAPOT })).await;
    let ingress =
        Server::start(ingress_router_with_control(&public.origin, &gateway.origin).unwrap()).await;
    let source = WorkerRelayClient::with_profile(
        &ingress.origin,
        &token1,
        endpoint(&a1),
        RelayClientProfile::ControlIngress,
    )
    .unwrap();
    let destination = WorkerRelayClient::with_profile(
        &ingress.origin,
        &token2,
        endpoint(&a2),
        RelayClientProfile::ControlIngress,
    )
    .unwrap();
    let body = "\n".repeat(MAX_MESSAGE_BYTES);
    let mut sent = std::collections::BTreeSet::new();
    for _ in 0..20 {
        let payload = send(&grant, &body);
        let receipt = source.send(&payload).await.unwrap();
        assert!(sent.insert(receipt.resource_id));
    }
    let first = destination.inbox(Some(0), 32).await.unwrap();
    let bytes = serde_json::to_vec(&first).unwrap().len();
    assert!(bytes > 1_900_000 && bytes <= 2 * 1024 * 1024);
    assert!(!first.items.is_empty() && first.items.len() < 20);
    let after = first.next_after.unwrap();
    assert_eq!(
        Some(after),
        first.items.last().map(|message| message.sequence)
    );
    let second = destination.inbox(Some(after), 32).await.unwrap();
    assert!(second.next_after.is_none());
    let mut seen = std::collections::BTreeSet::new();
    for item in first.items.into_iter().chain(second.items) {
        assert_eq!(item.body, body);
        assert!(seen.insert(item.metadata.message_id));
    }
    assert_eq!(seen, sent);
    assert!(a.integrity().unwrap().valid);
}
