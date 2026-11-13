//! Software seams only: retained authority is a test fixture, never a runtime permit.
mod deadline_cause_tests;
mod refusal_wire_tests;
use super::*;
use crate::relay::channel::ChannelLimitsV1;
use crate::relay::{RelayAckV1, RelaySendV1};
use crate::{
    broker::{discovery::RoleLease, journal::JournalScopeV1},
    relay::client::RelayClientProfile,
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::net::UnixStream;

const PROOF: &str = "fixture-workload-proof-0000000000000000000000000000";

struct FixtureContext {
    scope: JournalScopeV1,
    binding: Arc<Binding>,
    client: Arc<WorkerRelayClient>,
    secrets: Arc<Secrets>,
    io_tenant: Arc<Semaphore>,
    io_global: Arc<Semaphore>,
    memory_tenant: Arc<Semaphore>,
    memory_global: Arc<Semaphore>,
}
impl FixtureContext {
    fn new(origin: &str, proof: &str, endpoint: RelayEndpointV1) -> Self {
        Self {
            scope: JournalScopeV1::new(
                uuid::Uuid::from_u128(1),
                "fixture-tenant",
                origin,
                uuid::Uuid::from_u128(2),
                "fixture",
            )
            .unwrap(),
            binding: Arc::new(Binding {
                endpoint: endpoint.clone(),
                credential_id: uuid::Uuid::new_v4().to_string(),
            }),
            client: Arc::new(
                WorkerRelayClient::with_profile(
                    origin,
                    proof,
                    endpoint,
                    RelayClientProfile::ControlIngress,
                )
                .unwrap(),
            ),
            secrets: Arc::new(Secrets(vec![proof.into()])),
            io_tenant: Arc::new(Semaphore::new(4)),
            io_global: Arc::new(Semaphore::new(16)),
            memory_tenant: Arc::new(Semaphore::new(32 * 1024 * 1024)),
            memory_global: Arc::new(Semaphore::new(128 * 1024 * 1024)),
        }
    }
    fn io(&self, outbox: Outbox, role: Arc<RoleLease>) -> Arc<IoOwner> {
        IoOwner::fixture(
            outbox,
            self.binding.clone(),
            self.client.clone(),
            self.secrets.clone(),
            role,
            self.io_tenant.clone(),
            self.io_global.clone(),
        )
    }
    fn owner(&self, io: Arc<IoOwner>, epoch: u64) -> ControlOwner {
        ControlOwner {
            hello: HandshakeV1 {
                version: 1,
                channel_id: uuid::Uuid::new_v4().to_string(),
                epoch,
                limits: ChannelLimitsV1::default(),
                helper_version: 1,
            },
            binding: self.binding.clone(),
            client: self.client.clone(),
            secrets: self.secrets.clone(),
            io,
            authority: None,
            bytes: ByteBudget {
                tenant: self.memory_tenant.clone(),
                global: self.memory_global.clone(),
            },
            expires: Instant::now() + Duration::from_secs(30),
            synthetic_admission: Arc::new(AtomicBool::new(true)),
        }
    }
}

fn private_root() -> (tempfile::TempDir, Directory) {
    let temporary = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let root = Directory::open_existing(temporary.path()).unwrap();
    (temporary, root)
}

fn endpoint() -> RelayEndpointV1 {
    RelayEndpointV1 {
        worker_id: uuid::Uuid::new_v4().to_string(),
        attempt_id: uuid::Uuid::new_v4().to_string(),
        generation: 1,
    }
}

fn send() -> RequestPayloadV1 {
    RequestPayloadV1::Send(RelaySendV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        grant_id: uuid::Uuid::new_v4().to_string(),
        ttl_seconds: 60,
        body: "private business payload".into(),
    })
}

#[test]
fn outbox_retains_unknown_exact_identity_and_rejects_divergence_after_reopen() {
    let (_temporary, root) = private_root();
    let context = FixtureContext::new("http://127.0.0.1:9", PROOF, endpoint());
    let input = send();
    let mut journal = Outbox::create(&root, &context.scope, &context.secrets).unwrap();
    journal
        .prepare(&context.binding, &input, &context.client, &context.secrets)
        .unwrap();
    journal
        .dispatched(&context.binding, &input, &context.secrets)
        .unwrap();
    drop(journal);
    let mut reopened = Outbox::open(&root, &context.scope, &context.secrets).unwrap();
    assert!(reopened.unresolved(&context.binding).unwrap());
    // A current refusal does not alter this durable dispatch marker.
    reopened
        .prepare(&context.binding, &input, &context.client, &context.secrets)
        .unwrap();
    let RequestPayloadV1::Send(mut divergent) = input.clone() else {
        unreachable!()
    };
    divergent.body = "different".into();
    assert_eq!(
        reopened.prepare(
            &context.binding,
            &RequestPayloadV1::Send(divergent),
            &context.client,
            &context.secrets
        ),
        Err(ControlError::Conflict)
    );
    let RequestPayloadV1::Send(prior) = input else {
        unreachable!()
    };
    let collision = RequestPayloadV1::Ack(RelayAckV1 {
        version: 1,
        operation_id: prior.operation_id,
        message_id: uuid::Uuid::new_v4().to_string(),
    });
    assert_eq!(
        reopened.prepare(
            &context.binding,
            &collision,
            &context.client,
            &context.secrets
        ),
        Err(ControlError::Conflict)
    );
}

#[test]
fn preflight_checks_final_target_and_body_before_creating_operation_records() {
    let (_temporary, root) = private_root();
    // This accepted proof spans the fixed /messages/ path and an ACK UUID.
    let message = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    let path_proof = "messages/aaaaaaaa-aaaa-4aaa-8aaa-a";
    let context = FixtureContext::new("http://127.0.0.1:9", path_proof, endpoint());
    let mut journal = Outbox::create(&root, &context.scope, &context.secrets).unwrap();
    let ack = RequestPayloadV1::Ack(RelayAckV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        message_id: message.into(),
    });
    assert_eq!(
        journal.prepare(&context.binding, &ack, &context.client, &context.secrets),
        Err(ControlError::InvalidRequest)
    );
    assert_eq!(journal.operation_count().unwrap(), 0);
    let context = FixtureContext::new(
        "http://127.0.0.1:9",
        PROOF,
        context.binding.endpoint.clone(),
    );
    let RequestPayloadV1::Send(mut leaking) = send() else {
        unreachable!()
    };
    leaking.body = format!("{{\"x\":\"{PROOF}\"}}");
    assert_eq!(
        journal.prepare(
            &context.binding,
            &RequestPayloadV1::Send(leaking),
            &context.client,
            &context.secrets
        ),
        Err(ControlError::InvalidRequest)
    );
    assert_eq!(journal.operation_count().unwrap(), 0);
}

#[test]
fn generated_metadata_and_operation_names_reject_known_secrets_before_writing() {
    let (_temporary, root) = private_root();
    let lock_proof = "{\"format\":\"relay_v1\",\"version\":1}";
    let context = FixtureContext::new("http://127.0.0.1:9", lock_proof, endpoint());
    assert!(matches!(
        Outbox::create(&root, &context.scope, &context.secrets),
        Err(ControlError::InvalidRequest)
    ));
    assert!(!root.path().join("relay_v1").exists());

    let context = FixtureContext::new("http://127.0.0.1:9", PROOF, endpoint());
    let mut outbox = Outbox::create(&root, &context.scope, &context.secrets).unwrap();
    let input = send();
    let RequestPayloadV1::Send(dto) = &input else {
        unreachable!()
    };
    let operation = root
        .path()
        .join("relay_v1")
        .join(format!("op-{}", dto.operation_id));
    let named_secret = Secrets(vec![PROOF.into(), format!("op-{}", dto.operation_id)]);
    assert_eq!(
        outbox.prepare(&context.binding, &input, &context.client, &named_secret),
        Err(ControlError::InvalidRequest)
    );
    assert!(!operation.exists());
    outbox
        .prepare(&context.binding, &input, &context.client, &context.secrets)
        .unwrap();
    let exact = std::fs::read(operation.join("request.json")).unwrap();
    let digest_proof = hex::encode(Sha256::digest(&exact));
    // Accepted proof format, constructed without a preimage search. The normal
    // CLI's aigw1_ prefix is separate; this exercises the component boundary.
    let client = WorkerRelayClient::with_profile(
        "http://127.0.0.1:9",
        &digest_proof,
        context.binding.endpoint.clone(),
        RelayClientProfile::ControlIngress,
    )
    .unwrap();
    let secrets = Secrets(vec![digest_proof]);
    outbox
        .prepare(&context.binding, &input, &client, &secrets)
        .unwrap();
    assert_eq!(
        outbox.dispatched(&context.binding, &input, &secrets),
        Err(ControlError::InvalidRequest)
    );
    assert!(!operation.join("dispatch.json").exists());
    assert!(outbox.unresolved(&context.binding).unwrap());
}

#[derive(Default)]
struct HttpFixture {
    calls: AtomicUsize,
    inputs: Mutex<Vec<Value>>,
    lose_first: AtomicBool,
    refuse: AtomicBool,
    typed_capacity: AtomicBool,
    hold: AtomicBool,
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
    before_effect: Mutex<Option<std::path::PathBuf>>,
    inbox: Mutex<Option<crate::relay::RelayInboxV1>>,
}
async fn post_send(
    State(fixture): State<Arc<HttpFixture>>,
    headers: HeaderMap,
    Json(input): Json<RelaySendV1>,
) -> Response {
    assert_eq!(
        headers.get("authorization").unwrap().to_str().unwrap(),
        format!("Bearer {PROOF}")
    );
    if let Some(root) = fixture.before_effect.lock().unwrap().as_ref() {
        let operation = root
            .join("relay_v1")
            .join(format!("op-{}", input.operation_id));
        assert!(operation.join("request.json").is_file());
        assert!(operation.join("dispatch.json").is_file());
        assert!(!operation.join("receipt.json").exists());
    }
    let previous = fixture.calls.fetch_add(1, Ordering::SeqCst);
    fixture
        .inputs
        .lock()
        .unwrap()
        .push(serde_json::to_value(&input).unwrap());
    fixture.started.notify_one();
    if fixture.hold.load(Ordering::SeqCst) {
        fixture.release.notified().await;
    }
    if fixture.refuse.load(Ordering::SeqCst) {
        return (StatusCode::FORBIDDEN, Json(json!({"error":"forbidden"}))).into_response();
    }
    if fixture.lose_first.swap(false, Ordering::SeqCst) {
        return (StatusCode::CREATED, Json(json!({}))).into_response();
    }
    if fixture.typed_capacity.load(Ordering::SeqCst) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error": {
                "code": "relay_sender_capacity_exhausted", "message": "remote-private-diagnostic",
                "scope": "sender", "dimension": "messages",
                "limit": {"messages": 1, "bytes": 65536},
                "retained": {"messages": 1, "bytes": 24},
                "requested_bytes": input.body.len(), "next_release_at": null,
                "retry_action": "contact_operator"
            }})),
        )
            .into_response();
    }
    let replayed = previous > 0;
    (
        if replayed {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        Json(receipt(&input.operation_id, &input.operation_id, replayed)),
    )
        .into_response()
}
async fn inbox(State(fixture): State<Arc<HttpFixture>>, headers: HeaderMap) -> Json<Value> {
    assert_eq!(
        headers.get("authorization").unwrap().to_str().unwrap(),
        format!("Bearer {PROOF}")
    );
    Json(match fixture.inbox.lock().unwrap().as_ref() {
        Some(inbox) => serde_json::to_value(inbox).unwrap(),
        None => json!({"version":1,"items":[],"next_after":null}),
    })
}
async fn post_ack(
    headers: HeaderMap,
    Json(input): Json<RelayAckV1>,
) -> Json<crate::relay::RelayReceiptV1> {
    assert_eq!(
        headers.get("authorization").unwrap().to_str().unwrap(),
        format!("Bearer {PROOF}")
    );
    Json(receipt(&input.operation_id, &input.message_id, false))
}
fn receipt(operation: &str, resource: &str, replayed: bool) -> crate::relay::RelayReceiptV1 {
    crate::relay::RelayReceiptV1 {
        version: 1,
        operation_id: operation.into(),
        resource_id: resource.into(),
        audit_seq: 7,
        audit_hash: "a".repeat(64),
        replayed,
    }
}
struct Server {
    origin: String,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    async fn start(fixture: Arc<HttpFixture>) -> Self {
        // This is a real loopback HTTP router; its declared receipts deliberately
        // do not claim gateway authorization, SQLite audit or runtime qualification.
        let router = Router::new()
            .route("/control/api/v1/relay/messages", post(post_send).get(inbox))
            .route(
                "/control/api/v1/relay/messages/{message}/ack",
                post(post_ack),
            )
            .with_state(fixture);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self {
            origin,
            stop: Some(stop),
            task,
        }
    }
    async fn finish(mut self) {
        let _ = self.stop.take().unwrap().send(());
        timeout_at(Instant::now() + Duration::from_secs(5), &mut self.task)
            .await
            .unwrap()
            .unwrap();
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start_owner(
    mut owner: ControlOwner,
) -> (
    UnixStream,
    HandshakeV1,
    oneshot::Sender<CloseReasonV1>,
    tokio::task::JoinHandle<(ControlOwner, Result<(), ControlError>)>,
) {
    let (control, mut peer) = UnixStream::pair().unwrap();
    let (input, output) = control.into_split();
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        let result = owner.serve(input, output, stopped).await;
        (owner, result)
    });
    let Some(FrameV1::Hello(hello)) = read_frame(&mut peer, FrameLimit::Control).await.unwrap()
    else {
        panic!("missing hello")
    };
    checked_write(
        &mut peer,
        &FrameV1::Ready(hello.clone()),
        &Secrets(vec![PROOF.into()]),
    )
    .await
    .unwrap();
    (peer, hello, stop, task)
}
async fn request(
    peer: &mut UnixStream,
    hello: &HandshakeV1,
    payload: RequestPayloadV1,
) -> ResponseV1 {
    let frame = FrameV1::Request(RequestV1 {
        version: 1,
        channel_id: hello.channel_id.clone(),
        epoch: hello.epoch,
        correlation_id: uuid::Uuid::new_v4().to_string(),
        payload,
    });
    checked_write(peer, &frame, &Secrets(vec![PROOF.into()]))
        .await
        .unwrap();
    let Some(FrameV1::Response(response)) = read_frame(peer, FrameLimit::Response).await.unwrap()
    else {
        panic!("missing response")
    };
    response
}
async fn stop_owner(
    mut peer: UnixStream,
    stop: oneshot::Sender<CloseReasonV1>,
    mut task: tokio::task::JoinHandle<(ControlOwner, Result<(), ControlError>)>,
) {
    stop.send(CloseReasonV1::Shutdown).unwrap();
    assert!(matches!(
        read_frame(&mut peer, FrameLimit::Response).await.unwrap(),
        Some(FrameV1::Close(_))
    ));
    let (owner, result) = timeout_at(Instant::now() + Duration::from_secs(5), &mut task)
        .await
        .unwrap()
        .unwrap();
    result.unwrap();
    owner.io.join_pending().await.unwrap();
}

#[tokio::test]
async fn idle_uds_channels_do_not_reserve_body_capacity_before_a_complete_prefix() {
    let (_temporary, root) = private_root();
    let context = FixtureContext::new("http://127.0.0.1:9", PROOF, endpoint());
    let io = context.io(
        Outbox::create(&root, &context.scope, &context.secrets).unwrap(),
        Arc::new(RoleLease::acquire(&root).unwrap()),
    );
    let (mut first, _hello, first_stop, first_task) =
        start_owner(context.owner(io.clone(), 1)).await;
    let (second, _hello, second_stop, second_task) = start_owner(context.owner(io, 1)).await;
    // Both Hello writes follow completed journal checks. An idle read may
    // retain four prefix bytes, not a body allocation/reservation.
    first.write_all(&[0]).await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(context.memory_tenant.available_permits(), 32 * 1024 * 1024);
    assert_eq!(context.memory_global.available_permits(), 128 * 1024 * 1024);
    stop_owner(first, first_stop, first_task).await;
    stop_owner(second, second_stop, second_task).await;
}

#[tokio::test]
async fn control_uds_http_dispatch_persists_before_send_and_before_local_success() {
    let fixture = Arc::new(HttpFixture::default());
    let server = Server::start(fixture.clone()).await;
    let (_temporary, root) = private_root();
    *fixture.before_effect.lock().unwrap() = Some(root.path().into());
    let context = FixtureContext::new(&server.origin, PROOF, endpoint());
    let outbox = Outbox::create(&root, &context.scope, &context.secrets).unwrap();
    let role = Arc::new(RoleLease::acquire(&root).unwrap());
    let io = context.io(outbox, role);
    let (mut peer, hello, stop, task) = start_owner(context.owner(io.clone(), 1)).await;
    let payload = send();
    let RequestPayloadV1::Send(input) = &payload else {
        unreachable!()
    };
    let response = request(&mut peer, &hello, payload.clone()).await;
    assert!(matches!(
        response.result,
        ResponseResultV1::Ok(ReplyV1::Send(_))
    ));
    assert!(
        root.path()
            .join("relay_v1")
            .join(format!("op-{}", input.operation_id))
            .join("receipt.json")
            .is_file()
    );
    let response = request(
        &mut peer,
        &hello,
        RequestPayloadV1::Inbox(super::super::InboxRequestV1 {
            after: None,
            limit: 32,
        }),
    )
    .await;
    assert!(matches!(
        response.result,
        ResponseResultV1::Ok(ReplyV1::Inbox(_))
    ));
    let response = request(
        &mut peer,
        &hello,
        RequestPayloadV1::Ack(RelayAckV1 {
            version: 1,
            operation_id: uuid::Uuid::new_v4().to_string(),
            message_id: input.operation_id.clone(),
        }),
    )
    .await;
    assert!(matches!(
        response.result,
        ResponseResultV1::Ok(ReplyV1::Ack(_))
    ));
    stop_owner(peer, stop, task).await;
    drop(io);
    let reopened = Outbox::open(&root, &context.scope, &context.secrets).unwrap();
    assert!(!reopened.unresolved(&context.binding).unwrap());
    for entry in std::fs::read_dir(root.path().join("relay_v1"))
        .unwrap()
        .flatten()
    {
        if entry.path().is_file() {
            assert!(
                !std::fs::read(entry.path())
                    .unwrap()
                    .windows(PROOF.len())
                    .any(|bytes| bytes == PROOF.as_bytes())
            );
        }
    }
    server.finish().await;
}

#[tokio::test]
async fn dense_inbox_output_backpressure_retains_the_same_logical_reservation() {
    let fixture = Arc::new(HttpFixture::default());
    let server = Server::start(fixture.clone()).await;
    let (_temporary, root) = private_root();
    let mut context = FixtureContext::new(&server.origin, PROOF, endpoint());
    context.memory_tenant = Arc::new(Semaphore::new(EXCHANGE_BYTES as usize));
    context.memory_global = Arc::new(Semaphore::new(EXCHANGE_BYTES as usize));
    let source = endpoint();
    let body = "x".repeat(60_000);
    *fixture.inbox.lock().unwrap() = Some(crate::relay::RelayInboxV1 {
        version: 1,
        next_after: Some(32),
        items: (1..=32)
            .map(|sequence| crate::relay::RelayMessageV1 {
                sequence,
                metadata: crate::relay::RelayMessageMetadataV1 {
                    version: 1,
                    message_id: uuid::Uuid::new_v4().to_string(),
                    grant_id: uuid::Uuid::new_v4().to_string(),
                    source: source.clone(),
                    destination: context.binding.endpoint.clone(),
                    channel: "fixture".into(),
                    created_at: "2026-11-07T00:00:00Z".into(),
                    expires_at: "2026-11-07T00:01:00Z".into(),
                    body_bytes: body.len() as u64,
                    body_sha256: hex::encode(Sha256::digest(body.as_bytes())),
                    acknowledged: false,
                    body_pruned: false,
                },
                body: body.clone(),
            })
            .collect(),
    });
    let io = context.io(
        Outbox::create(&root, &context.scope, &context.secrets).unwrap(),
        Arc::new(RoleLease::acquire(&root).unwrap()),
    );
    let mut owner = context.owner(io.clone(), 1);
    let (control, mut peer) = UnixStream::pair().unwrap();
    let small = 4096_i32;
    // SAFETY: this test owns the local socket; one initialized integer is passed
    // with its exact length. No user process or external descriptor is touched.
    assert_eq!(
        unsafe {
            libc::setsockopt(
                control.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&small as *const i32).cast(),
                std::mem::size_of_val(&small) as libc::socklen_t,
            )
        },
        0
    );
    let mut actual = 0_i32;
    let mut length = std::mem::size_of_val(&actual) as libc::socklen_t;
    // SAFETY: retained socket and writable buffers of the announced size.
    assert_eq!(
        unsafe {
            libc::getsockopt(
                control.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&mut actual as *mut i32).cast(),
                &mut length,
            )
        },
        0
    );
    assert!((1..=65_536).contains(&actual));
    let (input, output) = control.into_split();
    let (stop, stopped) = oneshot::channel();
    let mut task = tokio::spawn(async move {
        let result = owner.serve(input, output, stopped).await;
        (owner, result)
    });
    let Some(FrameV1::Hello(hello)) = read_frame(&mut peer, FrameLimit::Control).await.unwrap()
    else {
        panic!("missing hello")
    };
    checked_write(&mut peer, &FrameV1::Ready(hello.clone()), &context.secrets)
        .await
        .unwrap();
    let request = FrameV1::Request(RequestV1 {
        version: 1,
        channel_id: hello.channel_id,
        epoch: hello.epoch,
        correlation_id: uuid::Uuid::new_v4().to_string(),
        payload: RequestPayloadV1::Inbox(super::super::InboxRequestV1 {
            after: None,
            limit: 32,
        }),
    });
    checked_write(&mut peer, &request, &context.secrets)
        .await
        .unwrap();
    let mut prefix = [0_u8; 4];
    peer.read_exact(&mut prefix).await.unwrap();
    assert!(u32::from_be_bytes(prefix) > 512 * 1024);
    // Only the prefix was drained; the real socket cannot hold this page, so
    // this observes the output write, not another idle request reservation.
    assert_eq!(context.memory_tenant.available_permits(), 0);
    assert_eq!(context.memory_global.available_permits(), 0);
    stop.send(CloseReasonV1::Shutdown).unwrap();
    drop(peer);
    let (owner, result) = timeout_at(Instant::now() + FRAME_TIME, &mut task)
        .await
        .unwrap()
        .unwrap();
    result.unwrap();
    owner.io.join_pending().await.unwrap();
    assert_eq!(
        context.memory_tenant.available_permits(),
        EXCHANGE_BYTES as usize
    );
    assert_eq!(
        context.memory_global.available_permits(),
        EXCHANGE_BYTES as usize
    );
    drop(owner);
    drop(io);
    server.finish().await;
}

#[tokio::test]
async fn explicit_replay_after_reopen_preserves_unknown_when_current_attempt_is_refused() {
    let fixture = Arc::new(HttpFixture::default());
    fixture.lose_first.store(true, Ordering::SeqCst);
    let server = Server::start(fixture.clone()).await;
    let (_temporary, root) = private_root();
    let context = FixtureContext::new(&server.origin, PROOF, endpoint());
    let payload = send();
    let role = Arc::new(RoleLease::acquire(&root).unwrap());
    let outbox = Outbox::create(&root, &context.scope, &context.secrets).unwrap();
    let io = context.io(outbox, role.clone());
    let (mut peer, hello, stop, task) = start_owner(context.owner(io.clone(), 1)).await;
    assert!(matches!(
        request(&mut peer, &hello, payload.clone()).await.result,
        ResponseResultV1::Error(ErrorCodeV1::Protocol)
    ));
    stop_owner(peer, stop, task).await;
    drop(io);
    let outbox = Outbox::open(&root, &context.scope, &context.secrets).unwrap();
    assert!(outbox.unresolved(&context.binding).unwrap());
    fixture.refuse.store(true, Ordering::SeqCst);
    let io = context.io(outbox, role);
    let (mut peer, hello, stop, task) = start_owner(context.owner(io.clone(), 2)).await;
    assert!(matches!(
        request(&mut peer, &hello, payload.clone()).await.result,
        ResponseResultV1::Error(ErrorCodeV1::Forbidden)
    ));
    // A different operation remains refused by the local unresolved reservation.
    assert!(matches!(
        request(&mut peer, &hello, send()).await.result,
        ResponseResultV1::Error(ErrorCodeV1::Capacity)
    ));
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 2);
    fixture.refuse.store(false, Ordering::SeqCst);
    assert!(matches!(
        request(&mut peer, &hello, payload.clone()).await.result,
        ResponseResultV1::Ok(ReplyV1::Send(_))
    ));
    stop_owner(peer, stop, task).await;
    drop(io);
    let reopened = Outbox::open(&root, &context.scope, &context.secrets).unwrap();
    assert!(!reopened.unresolved(&context.binding).unwrap());
    let inputs = fixture.inputs.lock().unwrap();
    assert_eq!(inputs.len(), 3);
    assert!(inputs.iter().all(|input| input == &inputs[0]));
    drop(inputs);
    server.finish().await;
}

#[tokio::test]
async fn second_prefix_during_http_closes_without_reading_or_dispatching_second_body() {
    let fixture = Arc::new(HttpFixture::default());
    fixture.hold.store(true, Ordering::SeqCst);
    let server = Server::start(fixture.clone()).await;
    let (_temporary, root) = private_root();
    let context = FixtureContext::new(&server.origin, PROOF, endpoint());
    let outbox = Outbox::create(&root, &context.scope, &context.secrets).unwrap();
    let io = context.io(outbox, Arc::new(RoleLease::acquire(&root).unwrap()));
    let (mut peer, hello, _stop, mut task) = start_owner(context.owner(io.clone(), 1)).await;
    let first = FrameV1::Request(RequestV1 {
        version: 1,
        channel_id: hello.channel_id,
        epoch: hello.epoch,
        correlation_id: uuid::Uuid::new_v4().to_string(),
        payload: send(),
    });
    checked_write(&mut peer, &first, &context.secrets)
        .await
        .unwrap();
    fixture.started.notified().await;
    peer.write_all(&[0]).await.unwrap();
    assert!(matches!(
        read_frame(&mut peer, FrameLimit::Response).await.unwrap(),
        Some(FrameV1::Close(_))
    ));
    let (owner, result) = timeout_at(Instant::now() + Duration::from_secs(5), &mut task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result, Err(ControlError::ExtraFrame));
    drop(owner);
    drop(io);
    assert!(
        Outbox::open(&root, &context.scope, &context.secrets)
            .unwrap()
            .unresolved(&context.binding)
            .unwrap()
    );
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    fixture.release.notify_one();
    server.finish().await;
}

#[tokio::test]
async fn eof_during_http_closes_the_owner_and_preserves_the_dispatched_mutation() {
    let fixture = Arc::new(HttpFixture::default());
    fixture.hold.store(true, Ordering::SeqCst);
    let server = Server::start(fixture.clone()).await;
    let (_temporary, root) = private_root();
    let context = FixtureContext::new(&server.origin, PROOF, endpoint());
    let io = context.io(
        Outbox::create(&root, &context.scope, &context.secrets).unwrap(),
        Arc::new(RoleLease::acquire(&root).unwrap()),
    );
    let (mut peer, hello, _stop, mut task) = start_owner(context.owner(io.clone(), 1)).await;
    let first = FrameV1::Request(RequestV1 {
        version: 1,
        channel_id: hello.channel_id,
        epoch: hello.epoch,
        correlation_id: uuid::Uuid::new_v4().to_string(),
        payload: send(),
    });
    checked_write(&mut peer, &first, &context.secrets)
        .await
        .unwrap();
    fixture.started.notified().await;
    peer.shutdown().await.unwrap();
    assert!(matches!(
        read_frame(&mut peer, FrameLimit::Response).await.unwrap(),
        Some(FrameV1::Close(_))
    ));
    let (owner, result) = timeout_at(Instant::now() + FRAME_TIME, &mut task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result, Err(ControlError::PeerClosed));
    drop(owner);
    drop(io);
    assert!(
        Outbox::open(&root, &context.scope, &context.secrets)
            .unwrap()
            .unresolved(&context.binding)
            .unwrap()
    );
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    fixture.release.notify_one();
    server.finish().await;
}

#[tokio::test]
async fn unavailable_runtime_admission_closes_before_handshake_or_http() {
    let fixture = Arc::new(HttpFixture::default());
    let server = Server::start(fixture.clone()).await;
    let (_temporary, root) = private_root();
    let context = FixtureContext::new(&server.origin, PROOF, endpoint());
    let io = context.io(
        Outbox::create(&root, &context.scope, &context.secrets).unwrap(),
        Arc::new(RoleLease::acquire(&root).unwrap()),
    );
    let mut owner = context.owner(io, 1);
    owner.synthetic_admission.store(false, Ordering::SeqCst);
    let (control, mut peer) = UnixStream::pair().unwrap();
    let (input, output) = control.into_split();
    let (_stop, stopped) = oneshot::channel();
    let mut task = tokio::spawn(async move { owner.serve(input, output, stopped).await });
    let Some(FrameV1::Close(close)) = read_frame(&mut peer, FrameLimit::Control).await.unwrap()
    else {
        panic!("missing refusal")
    };
    assert_eq!(close.reason, CloseReasonV1::IdentityChanged);
    assert_eq!(
        timeout_at(Instant::now() + FRAME_TIME, &mut task)
            .await
            .unwrap()
            .unwrap(),
        Err(ControlError::AuthorityUnavailable)
    );
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    server.finish().await;
}

#[tokio::test]
async fn cancelled_serve_retains_role_slots_memory_and_handle_until_explicit_join() {
    let (_temporary, root) = private_root();
    let mut context = FixtureContext::new("http://127.0.0.1:9", PROOF, endpoint());
    context.memory_tenant = Arc::new(Semaphore::new(EXCHANGE_BYTES as usize));
    context.memory_global = Arc::new(Semaphore::new(EXCHANGE_BYTES as usize));
    let outbox = Outbox::create(&root, &context.scope, &context.secrets).unwrap();
    let role = Arc::new(RoleLease::acquire(&root).unwrap());
    // Two distinct shared counters, each of one slot; no owner creates these.
    let tenant_slots = Arc::new(Semaphore::new(1));
    let global_slots = Arc::new(Semaphore::new(1));
    let io = IoOwner::fixture(
        outbox,
        context.binding.clone(),
        context.client.clone(),
        context.secrets.clone(),
        role,
        tenant_slots.clone(),
        global_slots.clone(),
    );
    let (started, ready) = oneshot::channel();
    let (release, finish) = std::sync::mpsc::channel();
    io.pause_next(started, finish);
    let mut owner = context.owner(io.clone(), 1);
    let memory = ByteBudget {
        tenant: context.memory_tenant.clone(),
        global: context.memory_global.clone(),
    };
    let (control, _peer) = UnixStream::pair().unwrap();
    let (input, output) = control.into_split();
    let (_stop, stopped) = oneshot::channel();
    let waiter = tokio::spawn(async move { owner.serve(input, output, stopped).await });
    ready.await.unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert_eq!(tenant_slots.available_permits(), 0);
    assert_eq!(global_slots.available_permits(), 0);
    assert!(matches!(memory.reserve(), Err(ControlError::Capacity)));
    assert!(RoleLease::acquire(&root).is_err());
    assert!(Outbox::open(&root, &context.scope, &context.secrets).is_err());
    // The pending handle closes admission even if another caller has memory.
    let extra = ByteBudget {
        tenant: Arc::new(Semaphore::new(EXCHANGE_BYTES as usize)),
        global: Arc::new(Semaphore::new(EXCHANGE_BYTES as usize)),
    };
    assert_eq!(
        io.run(IoCommand::Check, Arc::new(extra.reserve().unwrap()))
            .await,
        Err(ControlError::NeedsReconciliation)
    );
    release.send(()).unwrap();
    io.join_pending().await.unwrap();
    assert_eq!(tenant_slots.available_permits(), 1);
    assert_eq!(global_slots.available_permits(), 1);
    drop(memory.reserve().unwrap());
    // Even after the blocking segment finishes, its retained owner still owns
    // the role and journal lock. Explicit join precedes dropping that owner.
    assert!(RoleLease::acquire(&root).is_err());
    drop(io);
    assert!(RoleLease::acquire(&root).is_ok());
    assert!(Outbox::open(&root, &context.scope, &context.secrets).is_ok());
}

#[tokio::test]
async fn wrong_epoch_and_partial_or_oversized_frames_never_reach_http() {
    let (mut input, mut writer) = UnixStream::pair().unwrap();
    writer
        .write_all(&((MAX_REQUEST_FRAME_BYTES + 1) as u32).to_be_bytes())
        .await
        .unwrap();
    assert!(matches!(
        read_frame(&mut input, FrameLimit::Request).await,
        Err(ControlError::Protocol)
    ));
    let (mut input, mut writer) = UnixStream::pair().unwrap();
    writer.write_all(&[0, 0]).await.unwrap();
    drop(writer);
    assert!(matches!(
        read_frame(&mut input, FrameLimit::Request).await,
        Err(ControlError::Protocol)
    ));
    let fixture = Arc::new(HttpFixture::default());
    let server = Server::start(fixture.clone()).await;
    let (_temporary, root) = private_root();
    let context = FixtureContext::new(&server.origin, PROOF, endpoint());
    let io = context.io(
        Outbox::create(&root, &context.scope, &context.secrets).unwrap(),
        Arc::new(RoleLease::acquire(&root).unwrap()),
    );
    let (mut peer, hello, _stop, mut task) = start_owner(context.owner(io, 2)).await;
    let stale = FrameV1::Request(RequestV1 {
        version: 1,
        channel_id: hello.channel_id,
        epoch: 1,
        correlation_id: uuid::Uuid::new_v4().to_string(),
        payload: send(),
    });
    checked_write(&mut peer, &stale, &context.secrets)
        .await
        .unwrap();
    assert!(matches!(
        read_frame(&mut peer, FrameLimit::Response).await.unwrap(),
        Some(FrameV1::Close(_))
    ));
    let (_, result) = timeout_at(Instant::now() + Duration::from_secs(5), &mut task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result, Err(ControlError::Protocol));
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    server.finish().await;
}

#[test]
fn corrupt_or_partial_namespace_is_preserved_and_never_repaired() {
    let (_temporary, root) = private_root();
    let context = FixtureContext::new("http://127.0.0.1:9", PROOF, endpoint());
    let mut outbox = Outbox::create(&root, &context.scope, &context.secrets).unwrap();
    let payload = send();
    outbox
        .prepare(
            &context.binding,
            &payload,
            &context.client,
            &context.secrets,
        )
        .unwrap();
    drop(outbox);
    let directory = root.path().join("relay_v1");
    let original = std::fs::read(directory.join("scope.json")).unwrap();
    std::fs::write(
        directory.join("scope.json"),
        b"{\"format\":\"relay_v1\",\"version\":999}",
    )
    .unwrap();
    let malformed = std::fs::read(directory.join("scope.json")).unwrap();
    assert!(Outbox::open(&root, &context.scope, &context.secrets).is_err());
    assert_eq!(
        std::fs::read(directory.join("scope.json")).unwrap(),
        malformed
    );
    assert_ne!(original, malformed);
    let (_temporary, root) = private_root();
    let outbox = Outbox::create(&root, &context.scope, &context.secrets).unwrap();
    drop(outbox);
    let directory = root.persistent_child("relay_v1", false).unwrap().unwrap();
    let partial = format!("op-{}", uuid::Uuid::new_v4());
    directory.exclusive_child(&partial).unwrap();
    assert!(Outbox::open(&root, &context.scope, &context.secrets).is_err());
    assert!(directory.path().join(partial).is_dir());
}
