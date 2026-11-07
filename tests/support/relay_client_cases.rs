//! Real HTTP/SQLite client tests; worker observations below remain synthetic.
use super::*;
use agentsandbox::relay::client::{RelayClientError, WorkerRelayClient};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Server {
    origin: String,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    async fn start(app: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { origin, task }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn relay_client_uses_bound_workers_and_preserves_send_and_ack_replays() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 10_000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 10_000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let b1 = running_worker(&b);
    let token1 = "1".repeat(48);
    let token2 = "2".repeat(48);
    let foreign = "3".repeat(48);
    bind(&a, &a1, &token1);
    bind(&a, &a2, &token2);
    bind(&b, &b1, &foreign);
    let grant = directed(&a1, &a2, 300);
    a.relay_grant(&grant).unwrap();
    assert!(a.relay_grant(&directed(&a1, &b1, 300)).is_err());
    let server = Server::start(router(a.clone(), b.clone())).await;
    let source = WorkerRelayClient::new(&server.origin, &token1, endpoint(&a1)).unwrap();
    let destination = WorkerRelayClient::new(&server.origin, &token2, endpoint(&a2)).unwrap();
    let other = WorkerRelayClient::new(&server.origin, &foreign, endpoint(&b1)).unwrap();
    let payload = send(&grant, "client business body");
    assert_eq!(
        other.send(&payload).await.unwrap_err(),
        RelayClientError::Forbidden
    );
    let first = source.send(&payload).await.unwrap();
    assert_eq!(first.resource_id, payload.operation_id);
    assert!(!first.replayed);
    let head = a.head().unwrap();
    let replay = source.send(&payload).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(first.audit_seq, replay.audit_seq);
    assert_eq!(a.head().unwrap(), head);
    let mut changed = payload.clone();
    changed.body = "divergent body".into();
    assert_eq!(
        source.send(&changed).await.unwrap_err(),
        RelayClientError::Conflict
    );
    assert!(other.inbox(None, 32).await.unwrap().items.is_empty());
    let inbox = destination.inbox(None, 32).await.unwrap();
    assert_eq!(inbox.items.len(), 1);
    assert_eq!(inbox.items[0].body, payload.body);
    let ack = RelayAckV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        message_id: first.resource_id,
    };
    assert!(!destination.ack(&ack).await.unwrap().replayed);
    let head = a.head().unwrap();
    assert!(destination.ack(&ack).await.unwrap().replayed);
    assert_eq!(a.head().unwrap(), head);
    a.relay_revoke(&RelayRevokeV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: grant.grant_id,
    })
    .unwrap();
    assert_eq!(
        source.send(&payload).await.unwrap_err(),
        RelayClientError::Forbidden
    );
    assert_eq!(
        destination.ack(&ack).await.unwrap_err(),
        RelayClientError::Forbidden
    );
    for store in [&a, &b] {
        let audit = serde_json::to_string(
            &store
                .events(&EventQuery {
                    order: Some("asc".into()),
                    limit: Some(1000),
                    ..Default::default()
                })
                .unwrap(),
        )
        .unwrap();
        for private in [&token1, &token2, &foreign, &payload.body] {
            assert!(!audit.contains(private));
        }
        assert!(store.integrity().unwrap().valid);
    }
}

#[tokio::test]
async fn relay_client_lost_response_is_unknown_and_explicit_retry_keeps_the_operation() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 2000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 2000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let token = "1".repeat(48);
    bind(&a, &a1, &token);
    bind(&a, &a2, &"2".repeat(48));
    let grant = directed(&a1, &a2, 300);
    a.relay_grant(&grant).unwrap();
    let first_response = Arc::new(AtomicBool::new(true));
    let calls = Arc::new(AtomicUsize::new(0));
    let app = router(a.clone(), b).layer(axum::middleware::from_fn({
        let calls = calls.clone();
        move |request: Request<Body>, next: axum::middleware::Next| {
            let first_response = first_response.clone();
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                // Run the real auth and durable mutation, then destroy only delivery evidence.
                let mut response = next.run(request).await;
                if first_response.swap(false, Ordering::SeqCst) {
                    *response.body_mut() = Body::from("{}");
                    response.headers_mut().remove("content-length");
                }
                response
            }
        }
    }));
    let server = Server::start(app).await;
    let client = WorkerRelayClient::new(&server.origin, &token, endpoint(&a1)).unwrap();
    let payload = send(&grant, "durable despite lost response");
    let error = client.send(&payload).await.unwrap_err();
    assert!(error.outcome_unknown());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "client retried automatically"
    );
    let head = a.head().unwrap();
    let receipt = client.send(&payload).await.unwrap();
    assert!(receipt.replayed);
    assert_eq!(receipt.operation_id, payload.operation_id);
    assert_eq!(receipt.resource_id, payload.operation_id);
    assert_eq!(a.head().unwrap(), head);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(a.integrity().unwrap().valid);
}

#[tokio::test]
async fn relay_client_rejects_unbounded_input_and_runner_proofs_without_leaking_secrets() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 2000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 2000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let token = "1".repeat(48);
    bind(&a, &a1, &token);
    bind(&a, &a2, &"2".repeat(48));
    let grant = directed(&a1, &a2, 300);
    a.relay_grant(&grant).unwrap();
    let server = Server::start(router(a.clone(), b)).await;
    let client = WorkerRelayClient::new(&server.origin, &token, endpoint(&a1)).unwrap();
    let before = a.head().unwrap();
    for body in ["x".repeat(MAX_MESSAGE_BYTES + 1), token.clone()] {
        let error = client.send(&send(&grant, &body)).await.unwrap_err();
        assert_eq!(error, RelayClientError::InvalidRequest);
        assert!(!format!("{error:?}: {error}").contains(&token));
    }
    assert!(matches!(
        client.inbox(Some(-1), 32).await,
        Err(RelayClientError::InvalidRequest)
    ));
    assert!(matches!(
        client.inbox(None, 33).await,
        Err(RelayClientError::InvalidRequest)
    ));
    assert_eq!(
        a.head().unwrap(),
        before,
        "invalid local request reached the gateway"
    );
    let runner = WorkerRelayClient::new(&server.origin, &"a".repeat(48), endpoint(&a1)).unwrap();
    assert_eq!(
        runner
            .send(&send(&grant, "not a worker proof"))
            .await
            .unwrap_err(),
        RelayClientError::Forbidden
    );
    assert_eq!(a.head().unwrap(), before);
    for origin in [
        "http://example.com",
        "http://localhost:8080",
        "http://127.0.0.1/control",
        "http://name:secret@127.0.0.1",
        "http://127.0.0.1?secret=value",
    ] {
        assert!(matches!(
            WorkerRelayClient::new(origin, &token, endpoint(&a1)),
            Err(RelayClientError::InvalidConfiguration)
        ));
    }
    assert!(matches!(
        WorkerRelayClient::new(
            &format!("https://{token}.example.com"),
            &token,
            endpoint(&a1)
        ),
        Err(RelayClientError::InvalidConfiguration)
    ));
}

#[tokio::test]
async fn relay_client_rejects_corrupted_responses_and_never_follows_a_redirect() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 2000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 2000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let token = "1".repeat(48);
    bind(&a, &a1, &token);
    bind(&a, &a2, &"2".repeat(48));
    let grant = directed(&a1, &a2, 300);
    a.relay_grant(&grant).unwrap();
    let followed = Arc::new(AtomicUsize::new(0));
    let trap = Server::start(Router::new().fallback({
        let followed = followed.clone();
        move || {
            let followed = followed.clone();
            async move {
                followed.fetch_add(1, Ordering::SeqCst);
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }))
    .await;
    let mode = Arc::new(AtomicUsize::new(0));
    let app = router(a.clone(), b).layer(axum::middleware::from_fn({
        let mode = mode.clone();
        let redirect = trap.origin.clone();
        let token = token.clone();
        move |request: Request<Body>, next: axum::middleware::Next| {
            let mode = mode.clone();
            let redirect = redirect.clone();
            let token = token.clone();
            async move {
                let response = next.run(request).await;
                let (mut parts, body) = response.into_parts();
                parts.headers.remove("content-length");
                let body = match mode.load(Ordering::SeqCst) {
                    0 => {
                        parts.status = StatusCode::TEMPORARY_REDIRECT;
                        parts.headers.insert("location", redirect.parse().unwrap());
                        Body::from("{}")
                    }
                    1 => {
                        let bytes = body.collect().await.unwrap().to_bytes();
                        let mut value: Value = serde_json::from_slice(&bytes).unwrap();
                        value["operation_id"] = json!(Uuid::new_v4().to_string());
                        Body::from(serde_json::to_vec(&value).unwrap())
                    }
                    2 => Body::from("x".repeat(16_385)),
                    3 => Body::from(serde_json::to_vec(&json!({"private": token})).unwrap()),
                    4 => Body::from("x".repeat(MAX_INBOX_BYTES + 1)),
                    5 => {
                        let bytes = body.collect().await.unwrap().to_bytes();
                        let mut value: Value = serde_json::from_slice(&bytes).unwrap();
                        value["items"][0]["body"] = json!("modified after durable read");
                        Body::from(serde_json::to_vec(&value).unwrap())
                    }
                    _ => body,
                };
                axum::response::Response::from_parts(parts, body)
            }
        }
    }));
    let server = Server::start(app).await;
    let source = WorkerRelayClient::new(&server.origin, &token, endpoint(&a1)).unwrap();
    let destination =
        WorkerRelayClient::new(&server.origin, &"2".repeat(48), endpoint(&a2)).unwrap();
    let payload = send(&grant, "immutable relay body");
    for variant in 0..4 {
        mode.store(variant, Ordering::SeqCst);
        let error = source.send(&payload).await.unwrap_err();
        assert_eq!(error, RelayClientError::Protocol);
        assert!(error.outcome_unknown());
        assert!(!format!("{error:?}: {error}").contains(&token));
    }
    assert_eq!(
        followed.load(Ordering::SeqCst),
        0,
        "credential followed a redirect"
    );
    for variant in 4..6 {
        mode.store(variant, Ordering::SeqCst);
        assert!(matches!(
            destination.inbox(None, 32).await,
            Err(RelayClientError::Protocol)
        ));
    }
    mode.store(6, Ordering::SeqCst);
    assert!(source.send(&payload).await.unwrap().replayed);
    assert_eq!(
        destination.inbox(None, 32).await.unwrap().items[0].body,
        payload.body
    );
    assert!(a.integrity().unwrap().valid);
}

#[derive(Clone, Copy, Debug)]
enum ProtectedIdentifier {
    SendOperation,
    SendGrant,
    AckOperation,
    AckMessage,
}

async fn assert_uuid_proof_identifier_is_rejected_locally(field: ProtectedIdentifier) {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 2000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 2000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    // A canonical UUID is a valid 36-byte workload proof, independently bound
    // through the real issuer. Its shape must not exempt it from secret checks.
    let proof = Uuid::new_v4().to_string();
    let other_proof = "2".repeat(48);
    bind(&a, &a1, &proof);
    bind(&a, &a2, &other_proof);
    let forward = directed(&a1, &a2, 300);
    let reverse = directed(&a2, &a1, 300);
    a.relay_grant(&forward).unwrap();
    a.relay_grant(&reverse).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let app = router(a.clone(), b).layer(axum::middleware::from_fn({
        let calls = calls.clone();
        move |request: Request<Body>, next: axum::middleware::Next| {
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                next.run(request).await
            }
        }
    }));
    let server = Server::start(app).await;
    let client = WorkerRelayClient::new(&server.origin, &proof, endpoint(&a1)).unwrap();
    let other = WorkerRelayClient::new(&server.origin, &other_proof, endpoint(&a2)).unwrap();
    let incoming = other
        .send(&send(&reverse, "real message for recipient ACK"))
        .await
        .unwrap();
    // Exercise the UUID proof through a successful real authenticated read
    // before the refusal oracle; neither an unknown token nor mocked auth helps.
    assert_eq!(client.inbox(None, 32).await.unwrap().items.len(), 1);
    calls.store(0, Ordering::SeqCst);
    let before = a.head().unwrap();
    let mut outgoing = send(&forward, "protected identifiers are not message content");
    let mut ack = RelayAckV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        message_id: incoming.resource_id,
    };
    let result = match field {
        ProtectedIdentifier::SendOperation => {
            outgoing.operation_id = proof;
            client.send(&outgoing).await
        }
        ProtectedIdentifier::SendGrant => {
            outgoing.grant_id = proof;
            client.send(&outgoing).await
        }
        ProtectedIdentifier::AckOperation => {
            ack.operation_id = proof;
            client.ack(&ack).await
        }
        ProtectedIdentifier::AckMessage => {
            ack.message_id = proof;
            client.ack(&ack).await
        }
    };
    let local_rejection = matches!(result, Err(RelayClientError::InvalidRequest));
    let http_calls = calls.load(Ordering::SeqCst);
    let changed = a.head().unwrap() != before;
    // No response/ID/proof is formatted, including on the expected RED path.
    assert!(
        local_rejection && http_calls == 0 && !changed,
        "protected {field:?}: local_rejection={local_rejection}, http_calls={http_calls}, durable_state_changed={changed}"
    );
    assert!(a.integrity().unwrap().valid);
}

#[tokio::test]
async fn relay_client_uuid_proof_send_operation_is_rejected_before_transport() {
    assert_uuid_proof_identifier_is_rejected_locally(ProtectedIdentifier::SendOperation).await;
}

#[tokio::test]
async fn relay_client_uuid_proof_send_grant_is_rejected_before_transport() {
    assert_uuid_proof_identifier_is_rejected_locally(ProtectedIdentifier::SendGrant).await;
}

#[tokio::test]
async fn relay_client_uuid_proof_ack_operation_is_rejected_before_transport() {
    assert_uuid_proof_identifier_is_rejected_locally(ProtectedIdentifier::AckOperation).await;
}

#[tokio::test]
async fn relay_client_uuid_proof_ack_message_is_rejected_before_url_construction() {
    assert_uuid_proof_identifier_is_rejected_locally(ProtectedIdentifier::AckMessage).await;
}

async fn assert_success_wire_fault_is_rejected(mode: usize) {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 2000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 2000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let source_proof = "1".repeat(48);
    let destination_proof = "2".repeat(48);
    bind(&a, &a1, &source_proof);
    bind(&a, &a2, &destination_proof);
    let grant = directed(&a1, &a2, 300);
    a.relay_grant(&grant).unwrap();
    let fault = Arc::new(AtomicUsize::new(0));
    let app = router(a.clone(), b).layer(axum::middleware::from_fn({
        let fault = fault.clone();
        move |request: Request<Body>, next: axum::middleware::Next| {
            let fault = fault.clone();
            async move {
                let response = next.run(request).await;
                let mode = fault.load(Ordering::SeqCst);
                if mode == 0 {
                    return response;
                }
                assert!(
                    response.status().is_success(),
                    "fault must follow real successful authentication and Store operation"
                );
                let (mut parts, body) = response.into_parts();
                parts.headers.remove("content-length");
                let bytes = body.collect().await.unwrap().to_bytes();
                let body = match mode {
                    // The final known key remains valid. Value-first decoding
                    // silently discards the earlier incompatible duplicate.
                    1 | 2 => String::from_utf8(bytes.to_vec())
                        .unwrap()
                        .replacen('{', "{\"version\":2,", 1)
                        .into_bytes(),
                    3 => String::from_utf8(bytes.to_vec())
                        .unwrap()
                        .replacen("\"generation\":1", "\"generation\":2,\"generation\":1", 1)
                        .into_bytes(),
                    4 => {
                        let mut value: Value = serde_json::from_slice(&bytes).unwrap();
                        assert!(
                            value
                                .as_object_mut()
                                .unwrap()
                                .remove("next_after")
                                .is_some()
                        );
                        serde_json::to_vec(&value).unwrap()
                    }
                    _ => unreachable!("test fault is closed"),
                };
                assert!(
                    body.as_slice() != bytes.as_ref(),
                    "fault did not change the real wire response"
                );
                axum::response::Response::from_parts(parts, Body::from(body))
            }
        }
    }));
    let server = Server::start(app).await;
    let source = WorkerRelayClient::new(&server.origin, &source_proof, endpoint(&a1)).unwrap();
    let destination =
        WorkerRelayClient::new(&server.origin, &destination_proof, endpoint(&a2)).unwrap();
    source
        .send(&send(&grant, "real queued message before response fault"))
        .await
        .unwrap();
    fault.store(mode, Ordering::SeqCst);
    if mode == 1 {
        let result = source
            .send(&send(
                &grant,
                "durable mutation with duplicate response key",
            ))
            .await;
        assert!(
            matches!(result, Err(RelayClientError::Protocol)),
            "duplicate mutation response key was accepted"
        );
        assert!(result.unwrap_err().outcome_unknown());
    } else {
        assert!(
            matches!(
                destination.inbox(None, 32).await,
                Err(RelayClientError::Protocol)
            ),
            "ambiguous or incomplete inbox wire response was accepted"
        );
    }
    assert!(a.integrity().unwrap().valid);
}

#[tokio::test]
async fn relay_client_wire_duplicate_mutation_receipt_is_protocol_unknown() {
    assert_success_wire_fault_is_rejected(1).await;
}

#[tokio::test]
async fn relay_client_wire_duplicate_inbox_field_is_protocol_error() {
    assert_success_wire_fault_is_rejected(2).await;
}

#[tokio::test]
async fn relay_client_wire_duplicate_nested_endpoint_field_is_protocol_error() {
    assert_success_wire_fault_is_rejected(3).await;
}

#[tokio::test]
async fn relay_client_wire_missing_inbox_next_after_is_protocol_error() {
    assert_success_wire_fault_is_rejected(4).await;
}

#[tokio::test]
async fn relay_client_lost_ack_response_is_unknown_and_exact_retry_preserves_the_head() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 2000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 2000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let source_proof = "1".repeat(48);
    let destination_proof = "2".repeat(48);
    bind(&a, &a1, &source_proof);
    bind(&a, &a2, &destination_proof);
    let grant = directed(&a1, &a2, 300);
    a.relay_grant(&grant).unwrap();
    let lose_ack = Arc::new(AtomicBool::new(true));
    let ack_posts = Arc::new(AtomicUsize::new(0));
    let app = router(a.clone(), b).layer(axum::middleware::from_fn({
        let ack_posts = ack_posts.clone();
        move |request: Request<Body>, next: axum::middleware::Next| {
            let lose_ack = lose_ack.clone();
            let ack_posts = ack_posts.clone();
            async move {
                let is_ack = request.method() == axum::http::Method::POST
                    && request.uri().path().ends_with("/ack");
                if is_ack {
                    ack_posts.fetch_add(1, Ordering::SeqCst);
                }
                let mut response = next.run(request).await;
                if is_ack && lose_ack.swap(false, Ordering::SeqCst) {
                    assert_eq!(
                        response.status(),
                        StatusCode::OK,
                        "corruption must follow a real durable recipient ACK"
                    );
                    response.headers_mut().remove("content-length");
                    *response.body_mut() = Body::from("{}");
                }
                response
            }
        }
    }));
    let server = Server::start(app).await;
    let source = WorkerRelayClient::new(&server.origin, &source_proof, endpoint(&a1)).unwrap();
    let destination =
        WorkerRelayClient::new(&server.origin, &destination_proof, endpoint(&a2)).unwrap();
    let message = source
        .send(&send(
            &grant,
            "message explicitly acknowledged before lost response",
        ))
        .await
        .unwrap();
    let ack = RelayAckV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        message_id: message.resource_id,
    };
    let before = a.head().unwrap();
    let error = destination.ack(&ack).await.unwrap_err();
    assert_eq!(error, RelayClientError::Protocol);
    assert!(error.outcome_unknown());
    assert_eq!(
        ack_posts.load(Ordering::SeqCst),
        1,
        "client automatically retried the ACK"
    );
    let committed_head = a.head().unwrap();
    assert_ne!(
        committed_head, before,
        "real ACK was not committed before response corruption"
    );
    let receipt = destination.ack(&ack).await.unwrap();
    assert!(receipt.replayed);
    assert_eq!(receipt.operation_id, ack.operation_id);
    assert_eq!(receipt.resource_id, ack.message_id);
    assert_eq!(a.head().unwrap(), committed_head);
    assert_eq!(ack_posts.load(Ordering::SeqCst), 2);
    assert!(a.integrity().unwrap().valid);
}
