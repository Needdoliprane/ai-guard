//! Preamble-only attach: handshake, framing, channels, ownership and bracket.
//!
//! The WebSocket codec is the `tokio-tungstenite` library: this crate has no
//! frame parser of its own, so no test here names one. Framing cases are
//! literal wire bytes, RFC 6455 vectors and hand-built frames that do not
//! come from the code under test. They are written to a `tokio::io::duplex`
//! stream and read by the real owner task through the library reader, with
//! the real bootstrap owner behind it. The handshake acceptance, the stdout
//! envelope and the status projection are pure functions of this crate and
//! are also held to their vectors without IO. The bracket runs through the
//! real HTTPS client against one owned raw TLS loopback peer. The peer
//! answers, the Pod bodies and the fixture stream are declared fixtures: they
//! prove no API server or kubelet behaviour, no confinement, no cluster
//! permission and no capacity.
//!
//! Time is real. Tokio's paused clock needs its `test-util` feature, which
//! this crate does not enable, so every deadline here is a short caller
//! deadline. The 5 s and 15 s product constants are pinned by value and are
//! not reached by execution.
//!
//! The imports below name everything these tests take from other crates and
//! from the sibling modules, so that the test build does not depend on the
//! import list of the module under test.
use super::*;
use crate::{
    control_fs::PrivateFile,
    runtime::kubernetes::{
        KubernetesCredentials, KubernetesError, KubernetesRuntime,
        bootstrap::{
            BootstrapCapacity, BootstrapError, BootstrapProvenanceV2, MAX_HANDSHAKE_TIME,
            RESERVED_BYTES,
        },
        session::{SessionBootstrapBindingV2, tests::fixture_at},
        session_effect::{
            LoopbackEffectGuardV2, SessionCreateOutcomeV2, SessionEffectKindV2,
            effect_request_sha256,
        },
        session_instance::{KubernetesSessionRuntimeError, SessionInstanceV2},
        session_pod::tests::{
            POD_UID, Rendered, admitted, rendered_case, rendered_case_with, replace,
        },
        session_test_peer::{
            ClientFrame, PeerReply, PeerRequest, PeerScriptFuture, PeerStream, SessionTestPeer,
            read_client_frame, server_frame, server_message, websocket_accept_value,
        },
        session_worker::{
            EXIT_NO_HANDOFF, WorkerBootstrapEndV2, serve_waiting_bootstrap, waiting_exit_code,
        },
    },
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream},
    sync::Semaphore,
    time::Instant,
};

const TOKEN: &str = "public-session-attach-loopback-fixture-token-000000000000";
const CHANGED_NAMESPACE_UID: &str = "44444444-4444-4444-8444-444444444444";
const OTHER_POD_UID: &str = "77777777-7777-4777-8777-777777777777";
const NODE: &str = "node-a";
const CONTAINER_ID: &str = "containerd://0123456789abcdef";
const IMAGE_ID: &str = "sha256:fedcba9876543210";
const STARTED_AT: &str = "2026-11-12T08:01:00Z";
const CREATE: &str = "/api/v1/namespaces/tenant-a/pods?fieldValidation=Strict";
const ANCHORS: [&str; 2] = [
    "GET /api/v1/namespaces/kube-system",
    "GET /api/v1/namespaces/tenant-a",
];
const RFC_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const RFC_ACCEPT: &str = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";
const STDERR_MARKER: &[u8] = b"STDERR-MARKER-7f3a91";
const STATUS_MARKER: &str = "STATUS-MESSAGE-MARKER-5c1d02";
/// A status reason chosen by the peer: alphanumeric, so that only the closed
/// projection keeps it out of the diagnostics.
const REASON_MARKER: &str = "PeerChosenReason9b27e4";
const PING: &[u8] = b"between";
/// Caller deadline of a flow that ends by itself well before it, and of the
/// flows whose owner is expected to reach it after several loopback TLS
/// connections. Contract regressions below use their own bounded window.
const SOON: Duration = Duration::from_millis(1900);
/// Caller deadline of a flow that is expected to reach it. It leaves room
/// for the loopback TLS connections that precede the awaited state.
const SHORT: Duration = Duration::from_millis(1200);
/// A caller window for state-based contract regressions under a loaded host.
/// It shortens the 15 s bracket and 5 s handshake ceilings; changes neither.
const CONTRACT_WINDOW: Duration = Duration::from_secs(4);

/// Test watchdog: a wrong implementation fails here instead of hanging the
/// suite. It is not a product bound.
async fn watched<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("the step did not end within the test watchdog")
}

fn fixture_identity() -> AttachIdentityV2 {
    AttachIdentityV2 {
        pod_uid: POD_UID.into(),
        node_name: NODE.into(),
        container_id: CONTAINER_ID.into(),
        image_id: IMAGE_ID.into(),
        started_at: STARTED_AT.parse().unwrap(),
    }
}
fn bootstrap_pool() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(4 * RESERVED_BYTES as usize))
}
fn bootstrap_capacity(pool: &Arc<Semaphore>) -> BootstrapCapacity {
    BootstrapCapacity::new(pool.clone().try_acquire_many_owned(RESERVED_BYTES).unwrap()).unwrap()
}
fn attach_target(pod_name: &str) -> String {
    // Written out by hand: the fixture routes only this exact target.
    format!(
        "/api/v1/namespaces/tenant-a/pods/{pod_name}/attach?container=worker&stdin=true&stdout=true&stderr=true&tty=false"
    )
}

/// Released, scheduled on a node, one Running `worker` container.
fn running(held: &Value) -> Value {
    let mut pod = held.clone();
    assert!(
        pod["spec"]
            .as_object_mut()
            .unwrap()
            .remove("schedulingGates")
            .is_some()
    );
    pod["metadata"]["resourceVersion"] = json!("46");
    pod["spec"]["nodeName"] = json!(NODE);
    pod["status"] = json!({"phase":"Running","containerStatuses":[{"name":"worker",
        "restartCount":0,"containerID":CONTAINER_ID,"imageID":IMAGE_ID,
        "state":{"running":{"startedAt":STARTED_AT}}}]});
    pod
}
fn with_status(pod: &Value, status: Value) -> Value {
    let mut pod = pod.clone();
    pod["status"] = status;
    pod
}
fn terminated_status() -> Value {
    json!({"phase":"Failed","containerStatuses":[{"name":"worker","restartCount":0,
        "containerID":CONTAINER_ID,"imageID":IMAGE_ID,
        "state":{"terminated":{"exitCode":66,"reason":"Error","startedAt":STARTED_AT,
            "finishedAt":"2026-11-12T08:02:00Z"}}}]})
}

// ---- fixture stream -------------------------------------------------------

/// One step of the fixture stream after the complete Hello.
#[derive(Clone)]
enum Step {
    /// Server bytes computed from the Ready envelope of the received Hello.
    Reply(fn(&[u8]) -> Vec<u8>),
    /// Fixed server bytes.
    Raw(Vec<u8>),
    /// Waits until the peer has served the second Pod GET.
    AwaitSecondPodGet,
    /// Waits until the test raises this flag.
    AwaitFlag(Arc<AtomicBool>),
    /// Ends the stream without a close frame.
    Hangup,
}

/// What the fixture stream does. The default reads until the client ends the
/// stream and answers nothing.
#[derive(Clone, Default)]
struct Script {
    /// Server bytes written before anything is read.
    first: Vec<u8>,
    /// Steps run once the complete Hello was read. Adjacent writes leave in
    /// one write.
    after_hello: Vec<Step>,
    /// Server bytes written once the stdin close was read.
    after_stdin_close: Option<fn(&[u8]) -> Vec<u8>>,
    /// Writes one byte at a time, so that frames cross read boundaries.
    dribble: bool,
}
impl Script {
    fn ready(reply: fn(&[u8]) -> Vec<u8>) -> Self {
        Self {
            after_hello: vec![Step::Reply(reply)],
            ..Self::default()
        }
    }
    fn first(bytes: Vec<u8>) -> Self {
        Self {
            first: bytes,
            ..Self::default()
        }
    }
}

/// What the fixture stream received. Every frame it read carried the mask
/// bit: `read_client_frame` refuses any other frame.
#[derive(Clone, Default)]
struct Seen {
    /// Stdin payload bytes, in order.
    stdin: Vec<u8>,
    /// The Ready envelope computed for the Hello, sent or not.
    ready: Vec<u8>,
    stdin_closes: usize,
    stdin_bytes_after_close: usize,
    /// Codes of the close frames received, with or without a reason.
    closes: Vec<u16>,
    /// Close frames received without a body.
    empty_closes: usize,
    pongs: Vec<Vec<u8>>,
    /// Frames that are none of the above, or that are not final.
    unexpected: usize,
    /// Pod GETs served by the peer when the stdin close was read.
    pod_gets_at_stdin_close: Option<usize>,
    ended: bool,
}

enum Noted {
    HelloComplete(Vec<u8>),
    StdinClosed,
    Other,
}

fn envelope_complete(bytes: &[u8]) -> bool {
    bytes.len() >= 4
        && bytes.len() >= 4 + u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize
}

fn note(
    frame: &ClientFrame,
    seen: &Mutex<Seen>,
    world: Option<&Arc<Mutex<World>>>,
    answered: bool,
) -> Noted {
    let mut seen = seen.lock().unwrap();
    if !frame.is_final() {
        seen.unexpected += 1;
    }
    match (frame.opcode(), frame.payload.as_slice()) {
        (0x2, [0, bytes @ ..]) => {
            if seen.stdin_closes > 0 {
                seen.stdin_bytes_after_close += bytes.len();
            }
            seen.stdin.extend_from_slice(bytes);
            if !answered && envelope_complete(&seen.stdin) {
                Noted::HelloComplete(seen.stdin.clone())
            } else {
                Noted::Other
            }
        }
        (0x2, [255, 0]) => {
            seen.stdin_closes += 1;
            seen.pod_gets_at_stdin_close = world.map(|world| world.lock().unwrap().pod_gets);
            Noted::StdinClosed
        }
        (0x8, []) => {
            seen.empty_closes += 1;
            Noted::Other
        }
        // The reason, if any, follows the code and is not kept.
        (0x8, [high, low, ..]) => {
            seen.closes.push(u16::from_be_bytes([*high, *low]));
            Noted::Other
        }
        (0xA, payload) => {
            seen.pongs.push(payload.to_vec());
            Noted::Other
        }
        _ => {
            seen.unexpected += 1;
            Noted::Other
        }
    }
}

/// The Ready echo of one Hello envelope, as a declared fixture: the same
/// frame with its kind replaced. The body is padded with JSON whitespace so
/// that its length ends with 0xFF: the envelope then holds NUL and 0xFF bytes.
fn ready_envelope(hello: &[u8]) -> Vec<u8> {
    let mut frame: Value =
        serde_json::from_slice(&hello[4..]).expect("the client wrote a JSON Hello");
    assert_eq!(frame["kind"], "hello");
    frame["kind"] = json!("ready");
    let mut body = serde_json::to_vec(&frame).unwrap();
    while body.len() % 256 != 255 {
        body.push(b' ');
    }
    let mut envelope = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
    envelope.extend_from_slice(&body);
    envelope
}

/// The same Ready, padded further so that its length prefix holds no byte
/// above 0x7F: the whole envelope is then valid UTF-8, which is what a text
/// frame must carry to be handed over by a conforming reader.
fn text_safe_envelope(ready: &[u8]) -> Vec<u8> {
    let mut body = ready[4..].to_vec();
    while body.len() % 256 != 0x20 {
        body.push(b' ');
    }
    let mut envelope = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
    envelope.extend_from_slice(&body);
    assert!(
        std::str::from_utf8(&envelope).is_ok(),
        "the fixture Hello is not plain UTF-8"
    );
    envelope
}

/// The reply of the real worker waiting mode behind `worker`.
async fn worker_reply(worker: &mut DuplexStream, hello: &[u8]) -> Vec<u8> {
    worker.write_all(hello).await.unwrap();
    let mut prefix = [0_u8; 4];
    worker.read_exact(&mut prefix).await.unwrap();
    let mut envelope = prefix.to_vec();
    envelope.resize(4 + u32::from_be_bytes(prefix) as usize, 0);
    worker.read_exact(&mut envelope[4..]).await.unwrap();
    envelope
}

async fn send<S: AsyncWrite + Unpin>(stream: &mut S, bytes: &[u8], dribble: bool) {
    if dribble {
        for byte in bytes {
            let _ = stream.write_all(std::slice::from_ref(byte)).await;
            let _ = stream.flush().await;
            tokio::task::yield_now().await;
        }
    } else if !bytes.is_empty() {
        let _ = stream.write_all(bytes).await;
        let _ = stream.flush().await;
    }
}

async fn await_second_pod_get(world: Option<&Arc<Mutex<World>>>) {
    let Some(world) = world else {
        return;
    };
    for _ in 0..400 {
        if world.lock().unwrap().pod_gets >= 2 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn await_flag(flag: &AtomicBool) {
    for _ in 0..400 {
        if flag.load(Ordering::Acquire) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// False once the script hung up.
async fn run_steps<S: AsyncWrite + Unpin>(
    stream: &mut S,
    script: &Script,
    ready: &[u8],
    world: Option<&Arc<Mutex<World>>>,
) -> bool {
    let mut pending = Vec::new();
    for step in &script.after_hello {
        match step {
            Step::Reply(reply) => pending.extend(reply(ready)),
            Step::Raw(bytes) => pending.extend_from_slice(bytes),
            Step::AwaitSecondPodGet => {
                send(stream, &pending, script.dribble).await;
                pending.clear();
                await_second_pod_get(world).await;
            }
            Step::AwaitFlag(flag) => {
                send(stream, &pending, script.dribble).await;
                pending.clear();
                await_flag(flag).await;
            }
            Step::Hangup => {
                send(stream, &pending, script.dribble).await;
                return false;
            }
        }
    }
    send(stream, &pending, script.dribble).await;
    true
}

/// Server side of one attach stream. With `worker`, the Ready comes from the
/// real worker waiting mode behind that pipe, whose input is closed when the
/// client closes stdin.
async fn serve_stream<S>(
    mut stream: S,
    script: Script,
    seen: Arc<Mutex<Seen>>,
    world: Option<Arc<Mutex<World>>>,
    mut worker: Option<DuplexStream>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    send(&mut stream, &script.first, script.dribble).await;
    let mut answered = false;
    while let Ok(Some(frame)) = read_client_frame(&mut stream).await {
        match note(&frame, &seen, world.as_ref(), answered) {
            Noted::HelloComplete(hello) => {
                answered = true;
                let ready = match worker.as_mut() {
                    Some(worker) => worker_reply(worker, &hello).await,
                    None => ready_envelope(&hello),
                };
                seen.lock().unwrap().ready = ready.clone();
                if !run_steps(&mut stream, &script, &ready, world.as_ref()).await {
                    break;
                }
            }
            Noted::StdinClosed => {
                if let Some(worker) = worker.as_mut() {
                    let _ = worker.shutdown().await;
                }
                if let Some(reply) = script.after_stdin_close {
                    let ready = seen.lock().unwrap().ready.clone();
                    send(&mut stream, &reply(&ready), script.dribble).await;
                }
            }
            Noted::Other => {}
        }
    }
    seen.lock().unwrap().ended = true;
}

fn whole(ready: &[u8]) -> Vec<u8> {
    server_message(1, ready)
}
/// One message in three frames with a ping between two of them. The first
/// frame ends inside the envelope prefix.
fn fragments(ready: &[u8]) -> Vec<u8> {
    let (head, rest) = ready.split_at(1);
    let (middle, tail) = rest.split_at(rest.len() / 2);
    let mut first = vec![1];
    first.extend_from_slice(head);
    [
        server_frame(0x02, &first),
        server_frame(0x89, PING),
        server_frame(0x00, middle),
        server_frame(0x80, tail),
    ]
    .concat()
}
/// An empty stdout message, then the envelope over three messages.
fn several_messages(ready: &[u8]) -> Vec<u8> {
    [
        server_message(1, &[]),
        server_message(1, &ready[..3]),
        server_message(1, &ready[3..10]),
        server_message(1, &ready[10..]),
    ]
    .concat()
}
fn channel_message(channel: u8, payload: &[u8]) -> Vec<u8> {
    let mut message = vec![channel];
    message.extend_from_slice(payload);
    message
}

// ---- duplex harness -------------------------------------------------------

/// The real owner task over one in-memory stream.
struct Wire {
    attempt: AttachAttemptV2,
    bracket: AttachBracketV2,
    slots: Arc<Semaphore>,
}
fn open(
    binding: &SessionBootstrapBindingV2,
    within: Duration,
    pipe: usize,
) -> (Wire, DuplexStream) {
    let slots = Arc::new(Semaphore::new(1));
    let attempt = AttachAttemptV2::new(AttachCapacity::new(
        slots.clone().try_acquire_owned().unwrap(),
    ));
    let (client, server) = tokio::io::duplex(pipe);
    let bracket = attempt
        .claim()
        .unwrap()
        .start(
            client,
            fixture_identity(),
            binding.clone(),
            bootstrap_capacity(&bootstrap_pool()),
            Instant::now() + within,
        )
        .unwrap();
    (
        Wire {
            attempt,
            bracket,
            slots,
        },
        server,
    )
}

struct Concluded {
    result: Outcome<AttachPreambleOutcomeV2>,
    report: AttachReportV2,
    seen: Seen,
    ready_accepted: bool,
    attempt: AttachAttemptV2,
}

/// One whole attempt over a duplex stream against the fixture script. Like
/// the bracket caller, it waits for the accepted Ready and then gives
/// `verdict`; without a verdict the sender is kept until the attempt ended.
async fn play(
    binding: &SessionBootstrapBindingV2,
    script: Script,
    within: Duration,
    verdict: Option<BracketVerdictV2>,
) -> Concluded {
    let (wire, server) = open(binding, within, 64 * 1024);
    let Wire {
        attempt,
        bracket,
        slots,
    } = wire;
    // The owner task holds the slot from its start; nothing has run yet.
    assert_eq!(slots.available_permits(), 0);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let serving = tokio::spawn(serve_stream(server, script, seen.clone(), None, None));
    let AttachBracketV2 {
        ready,
        verdict: sender,
    } = bracket;
    let ready_accepted = watched(ready).await.is_ok();
    let sender = match verdict {
        Some(verdict) if ready_accepted => {
            assert!(sender.send(verdict).is_ok());
            None
        }
        _ => Some(sender),
    };
    let result = watched(attempt.join()).await;
    // The record is visible only with the slot returned.
    assert_eq!(slots.available_permits(), 1);
    drop(sender);
    watched(serving).await.unwrap();
    let seen = seen.lock().unwrap().clone();
    Concluded {
        result,
        report: attempt.report().expect("an ended attempt has a report"),
        seen,
        ready_accepted,
        attempt,
    }
}

/// A stream refusal by this side: `error`, closed by us, nothing delivered.
fn assert_refused_stream(concluded: &Concluded, error: KubernetesSessionRuntimeError, code: u16) {
    assert_eq!(concluded.result.as_ref().err(), Some(&error));
    assert_eq!(
        concluded.report.end(),
        Some(&AttachEndV2::ClosedByUsWithoutIdentity)
    );
    assert_eq!(concluded.report.stdout_bytes(), 0);
    assert_eq!(concluded.report.cause(), None);
    assert!(!concluded.ready_accepted);
    assert_eq!(concluded.seen.stdin_closes, 1);
    assert_eq!(concluded.seen.closes, [code]);
    assert_eq!(concluded.seen.unexpected, 0);
    assert!(concluded.seen.ended);
}

// ---- loopback harness -----------------------------------------------------

#[derive(Clone)]
enum PodAnswer {
    Json(u16, Value),
    /// The Pod, after which the tenant namespace answers with another UID.
    ThenNamespace(Value, &'static str),
    Never,
}
type AttachAnswer = Box<dyn FnOnce(&PeerRequest) -> PeerReply + Send>;

/// A refused upgrade without a body, written by a held script.
const FORBIDDEN: &str = "HTTP/1.1 403 Fixture\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

/// What the peer answers to an upgrade request that it first holds.
enum HeldAnswer {
    /// `403`, then the connection is closed.
    Forbidden,
    /// The exact `101`, then the default fixture stream.
    Switching,
}

/// One upgrade request that the peer reads at once and answers on demand.
struct HeldUpgrade {
    /// Resolves when the peer has read the whole upgrade request.
    received: tokio::sync::oneshot::Receiver<()>,
    /// Lets the peer answer.
    release: tokio::sync::oneshot::Sender<()>,
    /// What the fixture stream received after a `101`.
    seen: Arc<Mutex<Seen>>,
}

/// What the peer answers; shared by the responder, the scripts and the test.
struct World {
    namespace_uid: String,
    /// Answers of the successive Pod GETs; the last one repeats.
    pods: Vec<PodAnswer>,
    pod_gets: usize,
    /// The flag is raised when the Pod GET of this rank is served (1 is the
    /// first of the flow), and stays raised.
    raise_on_pod_get: Option<(usize, Arc<AtomicBool>)>,
    /// Answers of the successive attach requests; none left is a 404.
    attaches: VecDeque<AttachAnswer>,
    attach_requests: Vec<PeerRequest>,
}

fn namespace_reply(name: &str, uid: &str) -> PeerReply {
    PeerReply::json(
        200,
        &json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":name,"uid":uid},
            "status":{"phase":"Active"}}),
    )
}
fn not_found() -> PeerReply {
    PeerReply::json(404, &json!({"kind":"Status","code":404}))
}
fn lines(requests: &[PeerRequest]) -> Vec<String> {
    requests.iter().map(PeerRequest::line).collect()
}
fn exact_headers(request: &PeerRequest) -> Vec<(String, String)> {
    let key = request.header("sec-websocket-key").unwrap_or_default();
    vec![
        ("Upgrade".into(), "websocket".into()),
        ("Connection".into(), "Upgrade".into()),
        ("Sec-WebSocket-Accept".into(), websocket_accept_value(key)),
        ("Sec-WebSocket-Protocol".into(), "v5.channel.k8s.io".into()),
    ]
}
fn set_header(headers: &mut [(String, String)], name: &str, value: &str) {
    for (header, current) in headers.iter_mut() {
        if header.as_str() == name {
            *current = value.into();
        }
    }
}

fn responder(
    world: Arc<Mutex<World>>,
    cluster_uid: String,
    pod_path: String,
    attach_target: String,
) -> impl FnMut(&PeerRequest) -> PeerReply + Send + 'static {
    move |request: &PeerRequest| {
        let mut guard = world.lock().unwrap();
        let world = &mut *guard;
        match (request.method(), request.target()) {
            ("GET", "/api/v1/namespaces/kube-system") => {
                namespace_reply("kube-system", &cluster_uid)
            }
            ("GET", "/api/v1/namespaces/tenant-a") => {
                namespace_reply("tenant-a", &world.namespace_uid)
            }
            ("GET", target) if target == pod_path => {
                let index = world.pod_gets.min(world.pods.len().saturating_sub(1));
                world.pod_gets += 1;
                if let Some((rank, flag)) = &world.raise_on_pod_get
                    && world.pod_gets >= *rank
                {
                    flag.store(true, Ordering::Release);
                }
                match world.pods.get(index).cloned() {
                    Some(PodAnswer::Json(status, pod)) => PeerReply::json(status, &pod),
                    Some(PodAnswer::ThenNamespace(pod, uid)) => {
                        world.namespace_uid = uid.into();
                        PeerReply::json(200, &pod)
                    }
                    Some(PodAnswer::Never) => PeerReply::never_answer(),
                    None => not_found(),
                }
            }
            ("GET", target) if target == attach_target => {
                world.attach_requests.push(request.clone());
                match world.attaches.pop_front() {
                    Some(answer) => answer(request),
                    None => not_found(),
                }
            }
            ("POST", CREATE) => {
                let mut pod: Value = serde_json::from_slice(request.body()).unwrap();
                pod["metadata"]["uid"] = json!(POD_UID);
                pod["metadata"]["resourceVersion"] = json!("42");
                PeerReply::json(201, &pod)
            }
            _ => not_found(),
        }
    }
}

/// A real reservation, journal, rendered request and validated CREATE of the
/// peer, then the real runtime over the real HTTPS client.
struct Case {
    peer: SessionTestPeer,
    rendered: Rendered,
    runtime: KubernetesRuntime,
    instance: SessionInstanceV2,
    world: Arc<Mutex<World>>,
    /// The Running observation of the exact instance.
    running: Value,
    original_namespace_uid: String,
    bootstrap: Arc<Semaphore>,
}
impl Case {
    async fn new() -> Self {
        let peer = SessionTestPeer::start().await;
        let ca = PrivateFile::open(peer.ca(), 64 * 1024).unwrap();
        let trust = ca.digest().to_owned();
        let token = peer.ca().parent().unwrap().join("session-attach-token");
        std::fs::write(&token, TOKEN).unwrap();
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
        let rendered = rendered_case_with(fixture_at(peer.endpoint(), &trust));
        let authority = rendered.intent.authority().clone();
        assert_eq!(authority.namespace(), "tenant-a");
        let runtime = KubernetesRuntime::connect(
            authority.clone(),
            KubernetesCredentials::bearer(ca, PrivateFile::open(&token, 8192).unwrap()),
        )
        .unwrap();
        let running = running(&admitted(&rendered.request));
        let pod_name = rendered.request.target().pod_name().to_owned();
        let original_namespace_uid = authority.namespace_uid.clone();
        let world = Arc::new(Mutex::new(World {
            namespace_uid: original_namespace_uid.clone(),
            pods: vec![PodAnswer::Json(200, running.clone())],
            pod_gets: 0,
            raise_on_pod_get: None,
            attaches: VecDeque::new(),
            attach_requests: Vec::new(),
        }));
        peer.respond(responder(
            world.clone(),
            authority.cluster_namespace_uid.clone(),
            format!("/api/v1/namespaces/tenant-a/pods/{pod_name}"),
            attach_target(&pod_name),
        ));
        let guard = LoopbackEffectGuardV2::new(
            SessionEffectKindV2::CreateHeld,
            rendered.request.target().request_sha256(),
            peer.listener(),
        )
        .unwrap();
        let instance = match runtime
            .create_session_held(&rendered.request, guard, &AtomicBool::new(false))
            .await
            .unwrap()
        {
            SessionCreateOutcomeV2::CreatedHeld(instance) => *instance,
            other => panic!("a validated 201 must retain the instance: {other:?}"),
        };
        Self {
            peer,
            rendered,
            runtime,
            instance,
            world,
            running,
            original_namespace_uid,
            bootstrap: bootstrap_pool(),
        }
    }
    fn pod_name(&self) -> &str {
        self.rendered.request.target().pod_name()
    }
    fn pod_line(&self) -> String {
        format!("GET /api/v1/namespaces/tenant-a/pods/{}", self.pod_name())
    }
    fn attach_line(&self) -> String {
        format!("GET {}", attach_target(self.pod_name()))
    }
    /// Digest of the exact attach request for the fixture identity.
    fn attach_digest(&self) -> String {
        effect_request_sha256(
            SessionEffectKindV2::Attach,
            &self.rendered.intent.authority().fingerprint(),
            "GET",
            &attach_target(self.pod_name()),
            &fixture_identity().canonical_bytes().unwrap(),
        )
    }
    fn attach_guard(&self) -> LoopbackEffectGuardV2 {
        LoopbackEffectGuardV2::new(
            SessionEffectKindV2::Attach,
            &self.attach_digest(),
            self.peer.listener(),
        )
        .unwrap()
    }
    /// Pod answers of the next flow, with the original namespace UID.
    fn set_pods(&self, pods: Vec<PodAnswer>) {
        let mut world = self.world.lock().unwrap();
        world.pods = pods;
        world.pod_gets = 0;
        world.namespace_uid = self.original_namespace_uid.clone();
    }
    fn pod_gets(&self) -> usize {
        self.world.lock().unwrap().pod_gets
    }
    fn attach_count(&self) -> usize {
        self.world.lock().unwrap().attach_requests.len()
    }
    fn lines_since(&self, start: usize) -> Vec<String> {
        lines(&self.peer.requests()[start..])
    }
    /// The next attach request gets `answer`.
    fn expect_reply(&self, answer: impl FnOnce(&PeerRequest) -> PeerReply + Send + 'static) {
        self.world
            .lock()
            .unwrap()
            .attaches
            .push_back(Box::new(answer));
    }
    /// The next attach request gets the exact `101`, changed by `mutate`,
    /// then the fixture stream.
    fn expect_stream(
        &self,
        script: Script,
        worker: Option<DuplexStream>,
        mutate: fn(&mut Vec<(String, String)>),
    ) -> Arc<Mutex<Seen>> {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let (script_seen, world) = (seen.clone(), self.world.clone());
        self.expect_reply(move |request: &PeerRequest| {
            let mut headers = exact_headers(request);
            mutate(&mut headers);
            let borrowed: Vec<(&str, &str)> = headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            PeerReply::switching(&borrowed, move |stream: PeerStream| {
                serve_stream(stream, script, script_seen, Some(world), worker)
            })
        });
        seen
    }
    fn expect_attach(&self, script: Script) -> Arc<Mutex<Seen>> {
        self.expect_stream(script, None, |_| {})
    }
    /// The next attach request is read at once and answered only when the
    /// test releases it: by a `403`, or by the exact `101` followed by the
    /// default fixture stream, which reads and never answers.
    fn expect_held_upgrade(&self, answer: HeldAnswer) -> HeldUpgrade {
        let (request_seen, received) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let script_seen = seen.clone();
        self.expect_reply(move |request: &PeerRequest| {
            request_seen
                .send(())
                .expect("the test awaits the received request");
            let (head, upgraded) = match answer {
                HeldAnswer::Forbidden => (FORBIDDEN.to_owned(), false),
                HeldAnswer::Switching => {
                    let headers: Vec<String> = exact_headers(request)
                        .iter()
                        .map(|(name, value)| format!("{name}: {value}\r\n"))
                        .collect();
                    let head = format!(
                        "HTTP/1.1 101 Switching Protocols\r\n{}\r\n",
                        headers.concat()
                    );
                    (head, true)
                }
            };
            PeerReply::Handoff {
                head: Vec::new(),
                script: Box::new(move |mut stream: PeerStream| -> PeerScriptFuture {
                    Box::pin(async move {
                        // A test that ended has dropped its sender, which
                        // releases the answer as well: no script stays held.
                        let _ = released.await;
                        let written = stream.write_all(head.as_bytes()).await.is_ok()
                            && stream.flush().await.is_ok();
                        if written && upgraded {
                            let script = Script::default();
                            serve_stream(stream, script, script_seen, None, None).await;
                        } else {
                            let _ = stream.shutdown().await;
                        }
                    })
                }),
            }
        });
        HeldUpgrade {
            received,
            release,
            seen,
        }
    }
    /// Starts one attach on a fresh case, waits until the peer has read its
    /// upgrade request, then drops the caller's future. The request is
    /// initiated and unanswered, so nothing may have ended: these
    /// observations are synchronous and precede any scheduling of the owner.
    async fn lose_caller_before_the_answer(
        &self,
        attempt: &AttachAttemptV2,
        received: tokio::sync::oneshot::Receiver<()>,
        until: Instant,
    ) {
        let cancelled = AtomicBool::new(false);
        let mut attach = Box::pin(self.runtime.attach_exact_instance(
            &self.instance,
            self.rendered.binding.clone(),
            self.attach_guard(),
            attempt,
            bootstrap_capacity(&self.bootstrap),
            until,
            &cancelled,
        ));
        tokio::select! {
            result = &mut attach => panic!("the attach ended early: {result:?}"),
            received = watched(received) => {
                received.expect("the peer observed the real upgrade request");
            }
        }
        assert_eq!(self.attach_count(), 1);
        assert!(attempt.report().is_none());
        assert_eq!(
            self.runtime.attach_slots_available(),
            MAX_CONCURRENT_ATTACH - 1
        );
        drop(attach);
        assert!(attempt.report().is_none());
        assert_eq!(
            self.runtime.attach_slots_available(),
            MAX_CONCURRENT_ATTACH - 1
        );
    }
    fn new_attempt(&self) -> AttachAttemptV2 {
        AttachAttemptV2::new(self.runtime.reserve_attach_capacity().unwrap())
    }
    async fn attach_with(
        &self,
        attempt: &AttachAttemptV2,
        guard: LoopbackEffectGuardV2,
        cancelled: &AtomicBool,
        within: Duration,
    ) -> Outcome<()> {
        self.runtime
            .attach_exact_instance(
                &self.instance,
                self.rendered.binding.clone(),
                guard,
                attempt,
                bootstrap_capacity(&self.bootstrap),
                Instant::now() + within,
                cancelled,
            )
            .await
    }
    async fn attach(&self, attempt: &AttachAttemptV2, within: Duration) -> Outcome<()> {
        watched(self.attach_with(
            attempt,
            self.attach_guard(),
            &AtomicBool::new(false),
            within,
        ))
        .await
    }
    /// One attempt of a fresh owner; its result and its report.
    async fn attempt(&self, within: Duration) -> (Outcome<()>, AttachAttemptV2, AttachReportV2) {
        let attempt = self.new_attempt();
        let result = self.attach(&attempt, within).await;
        assert_eq!(watched(attempt.join()).await.map(|_| ()), result);
        let report = attempt.report().expect("an ended attempt has a report");
        (result, attempt, report)
    }
}

fn snapshot(seen: &Mutex<Seen>) -> Seen {
    seen.lock().unwrap().clone()
}
async fn until_true(condition: impl Fn() -> bool) {
    while !condition() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The stdin bytes are exactly one Hello envelope carrying `binding`.
fn assert_exactly_one_hello(stdin: &[u8], binding: &SessionBootstrapBindingV2) {
    assert!(envelope_complete(stdin));
    let declared = u32::from_be_bytes([stdin[0], stdin[1], stdin[2], stdin[3]]) as usize;
    assert_eq!(stdin.len(), 4 + declared, "nothing follows the Hello");
    let frame: Value = serde_json::from_slice(&stdin[4..]).unwrap();
    assert_eq!(frame["kind"], "hello");
    assert_eq!(frame["version"], 2);
    assert_eq!(frame["binding"], serde_json::to_value(binding).unwrap());
    let fields = frame.as_object().unwrap();
    assert_eq!(
        fields.len(),
        4,
        "kind, version, exchange_id and binding only"
    );
    assert!(fields.contains_key("exchange_id"));
}

// ---- pure -----------------------------------------------------------------

#[test]
fn websocket_accept_matches_the_rfc_6455_sample() {
    // RFC 6455 section 1.3: a vector that does not come from this code.
    assert_eq!(websocket_accept(RFC_KEY), RFC_ACCEPT);
    // The fixture's own composition is held to the same vector.
    assert_eq!(websocket_accept_value(RFC_KEY), RFC_ACCEPT);
    // A fresh key is 16 random bytes in base64.
    for key in [websocket_key().unwrap(), websocket_key().unwrap()] {
        assert_eq!(STANDARD.decode(key.as_bytes()).unwrap().len(), 16);
    }
    // The named bounds, pinned by value: the tests below scale their caller
    // deadline and never wait for these figures.
    let bounds = [
        ATTACH_HANDSHAKE,
        ATTACH_BRACKET_MAX,
        CLOSE_WRITE_LIMIT,
        MAX_HANDSHAKE_TIME,
    ];
    assert_eq!(bounds.map(|bound| bound.as_secs()), [5, 15, 1, 5]);
    let periods = [CANCEL_POLL, IDENTITY_TWO_RETRY_PAUSE];
    assert_eq!(periods.map(|period| period.as_millis()), [50, 25]);
    let sizes = [
        MAX_CONCURRENT_ATTACH,
        MAX_WS_MESSAGE_BYTES,
        MAX_STATUS_BYTES,
        PIPE_BYTES,
    ];
    assert_eq!(sizes, [2, 64 * 1024, 4 * 1024, 16 * 1024 + 4]);
    // The bounds handed to the codec library, pinned by value as well: the
    // read buffer, the stdin chunk, the write buffer and the write queue of
    // one client frame. None may be raised silently.
    let codec_bounds = [
        READ_CHUNK_BYTES,
        STDIN_CHUNK_BYTES,
        WRITE_BUFFER_BYTES,
        MAX_WRITE_QUEUE_BYTES,
    ];
    assert_eq!(codec_bounds, [4096, 4096, 0, 4111]);
    assert_eq!(MAX_STDERR_BYTES, 16 * 1024);
    assert_eq!(ATTACH_SUBPROTOCOL, "v5.channel.k8s.io");
}

#[test]
fn attach_request_digest_binds_identity_one_and_the_fixed_path() {
    let fingerprint = "a".repeat(64);
    let name = "ai-guard-session-0123456789abcdef0123456789abcdef";
    let path = format!(
        "/api/v1/namespaces/tenant-a/pods/{name}/attach?container=worker&stdin=true&stdout=true&stderr=true&tty=false"
    );
    assert_eq!(attach_path("tenant-a", name), path);
    let identity = fixture_identity();
    // Canonical identity one, written out by hand: sorted keys, no whitespace.
    let body = format!(
        "{{\"container_id\":\"{CONTAINER_ID}\",\"image_id\":\"{IMAGE_ID}\",\"node_name\":\"{NODE}\",\
         \"pod_uid\":\"{POD_UID}\",\"started_at\":\"2026-11-12T08:01:00.000000000Z\",\"version\":1}}"
    );
    assert_eq!(identity.canonical_bytes().unwrap(), body.as_bytes());
    // Recomputed here from the documented domain and separators.
    let mut hash = Sha256::new();
    hash.update(b"ai-guard:kubernetes-session-effect-request:v2\0attach\0");
    hash.update(fingerprint.as_bytes());
    hash.update(b"\0GET ");
    hash.update(path.as_bytes());
    hash.update(b"\0");
    hash.update(body.as_bytes());
    let expected = hex::encode(hash.finalize());
    let digest_of = |name: &str, identity: &AttachIdentityV2| {
        effect_request_sha256(
            SessionEffectKindV2::Attach,
            &fingerprint,
            "GET",
            &attach_path("tenant-a", name),
            &identity.canonical_bytes().unwrap(),
        )
    };
    assert_eq!(digest_of(name, &identity), expected);
    let changed = [
        AttachIdentityV2 {
            container_id: "containerd://ffffffffffffffff".into(),
            ..identity.clone()
        },
        AttachIdentityV2 {
            started_at: "2026-11-12T08:01:01Z".parse().unwrap(),
            ..identity.clone()
        },
        AttachIdentityV2 {
            image_id: "sha256:0000000000000000".into(),
            ..identity.clone()
        },
        AttachIdentityV2 {
            pod_uid: OTHER_POD_UID.into(),
            ..identity.clone()
        },
        AttachIdentityV2 {
            node_name: "node-b".into(),
            ..identity.clone()
        },
    ];
    let mut digests: Vec<String> = changed
        .iter()
        .map(|identity| digest_of(name, identity))
        .collect();
    digests.push(digest_of(
        "ai-guard-session-fedcba9876543210fedcba9876543210",
        &identity,
    ));
    digests.push(expected);
    let distinct: std::collections::BTreeSet<&String> = digests.iter().collect();
    assert_eq!(distinct.len(), digests.len());
}

#[test]
fn attach_error_variants_display_closed_text() {
    use KubernetesSessionRuntimeError as E;
    let attach = [
        (
            E::AttachHandshakeRefused,
            "Kubernetes session attach upgrade was refused or was not the exact v5 channel handshake",
        ),
        (
            E::AttachProtocol,
            "Kubernetes session attach stream violated the bounded v5 channel protocol",
        ),
        (
            E::AttachIdentityUnproven,
            "Kubernetes session attach ended without a proven exact instance; no identity is established",
        ),
        (
            E::AttachCapacity,
            "Kubernetes session attach capacity is unavailable",
        ),
        (
            E::AttachDeadline,
            "Kubernetes session attach deadline elapsed or exceeds its bound",
        ),
    ];
    for (error, text) in attach {
        assert_eq!(error.to_string(), text);
        // A unit variant: its Debug is its name and carries no data.
        assert!(
            format!("{error:?}")
                .bytes()
                .all(|byte| byte.is_ascii_alphabetic())
        );
    }
    // The existing variants keep their text.
    let existing = [
        (
            E::ProfileChanged,
            "Kubernetes session Pod differs from the admitted V2 profile",
        ),
        (
            E::InstanceReplaced,
            "Kubernetes session Pod name now designates another instance",
        ),
        (
            E::InstanceRestarted,
            "Kubernetes session container restarted; its identity is no longer exact",
        ),
        (
            E::WorkspaceClaimInvalid,
            "Kubernetes session workspace claim is not a bound ReadWriteOncePod claim for this attempt",
        ),
        (
            E::EffectRefusedBeforeSend,
            "Kubernetes session effect was refused before any request was sent",
        ),
        (
            E::InstanceDeleting,
            "Kubernetes session Pod deletion is requested; its gate is not released",
        ),
        (
            E::Kubernetes(KubernetesError::Busy),
            "Kubernetes HTTPS request capacity is occupied; retry inspection",
        ),
    ];
    for (error, text) in existing {
        assert_eq!(error.to_string(), text);
    }
}

fn header_map(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        headers.append(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    headers
}

// ---- framing and channels over duplex --------------------------------------

/// RFC 6455 section 5.7, as literal wire bytes: the unmasked and the masked
/// text "Hello", the same text in two fragments, the unmasked ping "Hello"
/// and its masked pong. None comes from this crate or from its library.
const RFC_UNMASKED_TEXT: [u8; 7] = [0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f];
const RFC_MASKED_TEXT: [u8; 11] = [
    0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
];
const RFC_FRAGMENTED_TEXT: [u8; 9] = [0x01, 0x03, 0x48, 0x65, 0x6c, 0x80, 0x02, 0x6c, 0x6f];
const RFC_UNMASKED_PING: [u8; 7] = [0x89, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f];
const RFC_MASKED_PONG: [u8; 11] = [
    0x8a, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
];
/// Headers of the 256-byte and of the 64 KiB binary messages of the same
/// section: the 16-bit and the 64-bit length forms.
const RFC_HEADER_256_BYTES: [u8; 4] = [0x82, 0x7E, 0x01, 0x00];
const RFC_HEADER_64_KIB: [u8; 10] = [0x82, 0x7F, 0, 0, 0, 0, 0, 0x01, 0, 0];

#[tokio::test]
async fn rfc_6455_reference_frames_decode_and_client_frames_are_masked() {
    let rendered = rendered_case();
    // The fixture reader is held to the masked pong of the RFC first: what
    // it reports about a client frame rests on that vector, not on the
    // client under test.
    let mut vector: &[u8] = &RFC_MASKED_PONG;
    let pong = read_client_frame(&mut vector).await.unwrap().unwrap();
    assert_eq!(pong.first, 0x8A);
    assert_eq!(pong.payload, b"Hello");
    assert!(vector.is_empty());

    // The unmasked ping of the RFC, read by the real owner through the real
    // reader. The pong that comes back is a final, masked frame with the
    // same payload, and the Ready that follows is still accepted.
    let script = Script {
        after_hello: vec![Step::Raw(RFC_UNMASKED_PING.to_vec()), Step::Reply(whole)],
        ..Script::default()
    };
    let concluded = play(
        &rendered.binding,
        script,
        SOON,
        Some(BracketVerdictV2::Proven),
    )
    .await;
    assert!(
        concluded.result.is_ok(),
        "{:?}",
        concluded.result.as_ref().err()
    );
    assert_eq!(concluded.seen.pongs, [b"Hello".to_vec()]);
    assert_eq!(concluded.seen.unexpected, 0);

    // The three text frames of the RFC. A server never sends text on this
    // subprotocol: the unmasked one and the fragmented one are complete,
    // valid messages that this client refuses itself, and the masked one is
    // refused as a masked server frame. Nothing is delivered.
    for text in [
        RFC_UNMASKED_TEXT.to_vec(),
        RFC_FRAGMENTED_TEXT.to_vec(),
        RFC_MASKED_TEXT.to_vec(),
    ] {
        let script = Script {
            after_hello: vec![Step::Raw(text)],
            ..Script::default()
        };
        let concluded = play(&rendered.binding, script, SOON, None).await;
        assert_refused_stream(&concluded, PROTOCOL, 1002);
    }

    // The 16-bit length form: the 256-byte message of the RFC, here one
    // stderr message of 255 bytes behind its channel byte. It is counted
    // whole, and the attempt goes on to its Ready.
    let mut medium = RFC_HEADER_256_BYTES.to_vec();
    medium.push(2);
    medium.extend_from_slice(&[b'e'; 255]);
    let script = Script {
        first: medium,
        ..Script::ready(whole)
    };
    let concluded = play(
        &rendered.binding,
        script,
        SOON,
        Some(BracketVerdictV2::Proven),
    )
    .await;
    assert!(
        concluded.result.is_ok(),
        "{:?}",
        concluded.result.as_ref().err()
    );
    assert_eq!(concluded.report.stderr_bytes(), 255);

    // The 64-bit length form: the 64 KiB message of the RFC is exactly the
    // message bound, so it is read whole. Its first byte, `H`, is no channel
    // of this client: the refusal is the channel's, closed with 1002. A
    // frame refused for its size is closed with 1009 (see the next test).
    assert_eq!(MAX_WS_MESSAGE_BYTES, 64 * 1024);
    let mut large = RFC_HEADER_64_KIB.to_vec();
    large.resize(large.len() + MAX_WS_MESSAGE_BYTES, b'H');
    let concluded = play(&rendered.binding, Script::first(large), SOON, None).await;
    assert_refused_stream(&concluded, PROTOCOL, 1002);

    // The first client frame, read from the wire without the fixture reader:
    // a final binary frame with the mask bit, in the 16-bit length form
    // because a Hello is longer than 125 bytes, then the four key bytes.
    // Unmasked by hand, it is the stdin channel byte and exactly one Hello.
    let (wire, mut server) = open(&rendered.binding, SOON, 64 * 1024);
    let mut head = [0_u8; 8];
    watched(server.read_exact(&mut head)).await.unwrap();
    assert_eq!(head[..2], [0x82, 0x80 | 126]);
    let length = usize::from(u16::from_be_bytes([head[2], head[3]]));
    assert!(length > 125, "the 16-bit form must be the minimal one");
    let mut payload = vec![0_u8; length];
    watched(server.read_exact(&mut payload)).await.unwrap();
    for (byte, mask) in payload.iter_mut().zip(head[4..].iter().cycle()) {
        *byte ^= mask;
    }
    assert_eq!(payload[0], 0);
    assert_exactly_one_hello(&payload[1..], &rendered.binding);
    // The peer goes away without a close frame: a transport end, recorded as
    // a close by the peer and never as an identity.
    drop(server);
    assert_eq!(watched(wire.attempt.join()).await.err(), Some(UNPROVEN));
    let report = wire.attempt.report().unwrap();
    assert_eq!(report.end(), Some(&AttachEndV2::PeerClosed));
    assert_eq!(wire.slots.available_permits(), 1);
    drop(wire.bracket);

    // The real owner task again: every frame it writes is read by a fixture
    // that refuses an unmasked frame, and a ping between two fragments is
    // answered by a pong with the same payload.
    let script = Script {
        dribble: true,
        ..Script::ready(fragments)
    };
    let concluded = play(
        &rendered.binding,
        script,
        SOON,
        Some(BracketVerdictV2::Proven),
    )
    .await;
    assert!(
        concluded.result.is_ok(),
        "{:?}",
        concluded.result.as_ref().err()
    );
    assert!(concluded.ready_accepted);
    assert_exactly_one_hello(&concluded.seen.stdin, &rendered.binding);
    assert_eq!(concluded.seen.pongs, [PING.to_vec()]);
    assert_eq!(concluded.seen.stdin_closes, 1);
    assert_eq!(concluded.seen.closes, [1000]);
    assert_eq!(concluded.seen.unexpected, 0);
    assert!(concluded.seen.ended);
}

#[tokio::test]
async fn text_oversized_or_masked_server_frames_close_without_partial_delivery() {
    // Each offending frame carries the Ready envelope on stdout wherever its
    // form allows it: a client that delivered it would accept the Ready.
    fn masked(ready: &[u8]) -> Vec<u8> {
        let message = channel_message(1, ready);
        let key = [0x11, 0x22, 0x33, 0x44];
        let mut frame = vec![0x82, 0x80 | 126];
        frame.extend_from_slice(&u16::try_from(message.len()).unwrap().to_be_bytes());
        frame.extend_from_slice(&key);
        frame.extend(
            message
                .iter()
                .zip(key.iter().cycle())
                .map(|(byte, mask)| byte ^ mask),
        );
        frame
    }
    // A text frame whose payload is valid UTF-8: a conforming reader hands
    // it over as a whole message, so only this client's own refusal of text
    // keeps the Ready it carries away from stdout.
    fn text_that_is_valid_utf8(ready: &[u8]) -> Vec<u8> {
        server_frame(0x81, &channel_message(1, &text_safe_envelope(ready)))
    }
    // 126 bytes in the 16-bit form: one byte above the control bound.
    fn ping_above_the_control_bound(_: &[u8]) -> Vec<u8> {
        let mut frame = vec![0x89, 126, 0, 126];
        frame.extend_from_slice(&[0x55; 126]);
        frame
    }
    fn new_message_inside_a_fragmented_one(ready: &[u8]) -> Vec<u8> {
        let (head, tail) = ready.split_at(ready.len() / 2);
        [
            server_frame(0x02, &channel_message(1, head)),
            server_frame(0x82, &channel_message(1, tail)),
        ]
        .concat()
    }
    fn fragments_above_the_bound(_: &[u8]) -> Vec<u8> {
        [
            server_frame(0x02, &channel_message(2, &vec![b'e'; 40_000])),
            server_frame(0x80, &vec![b'e'; 30_000]),
        ]
        .concat()
    }
    // 4 bytes announced on 16 bits, then the rest of the Ready in the
    // minimal form.
    fn short_length_on_16_bits(ready: &[u8]) -> Vec<u8> {
        let rest = server_message(1, &ready[3..]);
        [&[0x82, 126, 0, 4, 1][..], &ready[..3], &rest[..]].concat()
    }
    // The whole Ready, its 16-bit size announced on 64 bits.
    fn long_form_of_a_short_length(ready: &[u8]) -> Vec<u8> {
        let message = channel_message(1, ready);
        let mut frame = vec![0x82, 127];
        frame.extend_from_slice(&u64::try_from(message.len()).unwrap().to_be_bytes());
        frame.extend_from_slice(&message);
        frame
    }
    // Every frame below is complete on the wire. Apart from a reserved
    // opcode, which the codec library refuses from the header, these
    // offences are judged once the frame is whole: by the library
    // (tungstenite 0.30.0, `read_message_frame` in `protocol/mod.rs`) or, for
    // text that it delivers, by this client. The former parser of this crate
    // refused at the header. The next test holds two of them announced by a
    // header alone. The frame bound limits what is read before such a
    // refusal.
    let protocol: [fn(&[u8]) -> Vec<u8>; 9] = [
        // Text whose bytes are not UTF-8 (the envelope prefix holds 0xFF),
        // text that is valid UTF-8, RSV1 and a reserved opcode.
        |ready| server_frame(0x81, &channel_message(1, ready)),
        text_that_is_valid_utf8,
        |ready| server_frame(0xC2, &channel_message(1, ready)),
        |ready| server_frame(0x83, &channel_message(1, ready)),
        masked,
        // Control frames: above 125 bytes, and fragmented.
        ping_above_the_control_bound,
        |_| server_frame(0x09, b"ping"),
        // A continuation without a start.
        |ready| server_frame(0x80, &channel_message(1, ready)),
        // A new data frame inside a fragmented message: its first fragment
        // was never delivered either.
        new_message_inside_a_fragmented_one,
    ];
    let rendered = rendered_case();
    for offense in protocol {
        let script = Script {
            after_hello: vec![Step::Reply(offense)],
            ..Script::default()
        };
        let concluded = play(&rendered.binding, script, SOON, None).await;
        assert_refused_stream(&concluded, PROTOCOL, 1002);
        assert_exactly_one_hello(&concluded.seen.stdin, &rendered.binding);
    }
    // RFC 6455 section 5.2 asks for the minimal length form. The library
    // reader does not refuse a longer form, and this client no longer does:
    // the frame is read as the same frame in its minimal form, so the Ready
    // it carries is delivered once and accepted. The declared length is held
    // to the frame bound either way.
    let longer_forms: [fn(&[u8]) -> Vec<u8>; 2] =
        [short_length_on_16_bits, long_form_of_a_short_length];
    for shape in longer_forms {
        let concluded = play(
            &rendered.binding,
            Script::ready(shape),
            SOON,
            Some(BracketVerdictV2::Proven),
        )
        .await;
        assert!(
            concluded.result.is_ok(),
            "{:?}",
            concluded.result.as_ref().err()
        );
        assert!(concluded.ready_accepted);
        assert_eq!(
            concluded.report.stdout_bytes(),
            concluded.seen.ready.len() as u64
        );
        assert_eq!(concluded.seen.closes, [1000]);
    }
    // Two headers alone, without any body. The top bit of the 64-bit form is
    // read as a length far above the frame bound, and the second header
    // announces one byte more than the message bound: both are refused as
    // too big, from the header, before anything is reserved for them. The
    // fixture never sends a body and the attempt still ends, before its
    // deadline.
    for header in [
        vec![0x82, 127, 0x80, 0, 0, 0, 0, 0, 0, 0],
        vec![0x82, 127, 0, 0, 0, 0, 0, 1, 0, 1],
    ] {
        let started = Instant::now();
        let script = Script {
            after_hello: vec![Step::Raw(header)],
            ..Script::default()
        };
        let concluded = play(&rendered.binding, script, SOON, None).await;
        assert_refused_stream(&concluded, PROTOCOL, 1009);
        assert!(started.elapsed() < SOON);
    }
    // Fragments whose sum exceeds the message bound are refused when the
    // fragment that crosses it is complete: each of them is within the frame
    // bound, and none of their bytes is counted or delivered.
    let script = Script {
        after_hello: vec![Step::Reply(fragments_above_the_bound)],
        ..Script::default()
    };
    let concluded = play(&rendered.binding, script, SOON, None).await;
    assert_refused_stream(&concluded, PROTOCOL, 1009);
    assert_eq!(concluded.report.stderr_bytes(), 0);
}

#[tokio::test]
async fn frame_offences_announced_without_their_payload_end_at_the_deadline() {
    // A masked server frame and a control frame above 125 bytes, each
    // announced by its header alone. The former parser of this crate refused
    // both at that header. The codec library judges a frame once it is whole
    // (tungstenite 0.30.0): `read_message_frame` (`protocol/mod.rs`, lines
    // 610-663) first takes a frame from `FrameCodec::read_frame`
    // (`protocol/frame/mod.rs`, 161-231), whose loop ends only when the
    // declared payload is buffered, and tests the mask and the size of a
    // control frame afterwards. A masked header without its four key bytes
    // is not even a complete header (`FrameHeader::parse_internal`,
    // `protocol/frame/frame.rs`, 181-185). Both offences therefore wait for
    // bytes that never come, and the attempt ends at its own deadline: never
    // an identity, closed by this side with the stdin close and close 1000,
    // and nothing reaches the bootstrap reader.
    let rendered = rendered_case();
    let latest = SHORT + 3 * CLOSE_WRITE_LIMIT;
    let cases = [
        ("masked_frame", vec![0x82, 0x85]),
        ("control_frame_above_125_bytes", vec![0x89, 126, 0, 126]),
    ];
    for (case, header) in cases {
        let started = Instant::now();
        let script = Script {
            after_hello: vec![Step::Raw(header)],
            ..Script::default()
        };
        let concluded = play(&rendered.binding, script, SHORT, None).await;
        let elapsed = started.elapsed();
        assert_eq!(
            concluded.result.as_ref().err(),
            Some(&DEADLINE),
            "case {case}"
        );
        // Not before the caller deadline, and no later than that deadline
        // plus the bounded close.
        assert!(elapsed >= SHORT, "case {case}: ended before its deadline");
        assert!(elapsed <= latest, "case {case}: ended after {elapsed:?}");
        assert_eq!(
            concluded.report.end(),
            Some(&AttachEndV2::ClosedByUsWithoutIdentity)
        );
        assert_eq!(concluded.report.stdout_bytes(), 0);
        assert!(!concluded.ready_accepted);
        // The fixture sends the header of the case only once it has read the
        // whole Hello: without that Hello this ending would be plain silence.
        assert_exactly_one_hello(&concluded.seen.stdin, &rendered.binding);
        assert_eq!(concluded.seen.stdin_closes, 1);
        assert_eq!(concluded.seen.closes, [1000], "case {case}");
        assert!(concluded.seen.ended);
    }
}

#[tokio::test]
async fn unknown_or_forbidden_channel_and_empty_message_are_refused() {
    let offenses: [fn(&[u8]) -> Vec<u8>; 6] = [
        |ready| server_message(0, ready),
        |ready| server_message(4, ready),
        |ready| server_message(5, ready),
        |ready| server_message(254, ready),
        |ready| server_message(255, ready),
        // A binary message without a channel byte.
        |_| server_frame(0x82, &[]),
    ];
    let rendered = rendered_case();
    for offense in offenses {
        let script = Script {
            after_hello: vec![Step::Reply(offense)],
            ..Script::default()
        };
        let concluded = play(&rendered.binding, script, SOON, None).await;
        assert_refused_stream(&concluded, PROTOCOL, 1002);
    }
    // The v5 close signal and a status that is not a document are refused
    // from a server as well.
    for bytes in [server_message(255, &[0]), server_message(3, &[])] {
        let concluded = play(&rendered.binding, Script::first(bytes), SOON, None).await;
        assert_refused_stream(&concluded, PROTOCOL, 1002);
    }
}

#[tokio::test]
async fn fragmented_and_coalesced_stdout_reassembles_binary_payload_with_nul_bytes() {
    // The single-envelope rule, without any IO.
    let mut envelope = StdoutEnvelope::new();
    assert_eq!(envelope.admit(&[]), Ok(()));
    assert_eq!(envelope.admit(&[0, 0]), Ok(()));
    assert_eq!(envelope.admit(&[0, 3, 0xFF]), Ok(()));
    // One byte beyond the declared envelope refuses the whole payload.
    assert_eq!(envelope.admit(&[0, 0xFF, 1]), Err(Violation::Protocol));
    assert_eq!(envelope.admit(&[0, 0xFF]), Ok(()));
    assert_eq!((envelope.seen, envelope.total), (7, Some(7)));
    assert_eq!(envelope.admit(&[]), Ok(()));
    assert_eq!(envelope.admit(&[1]), Err(Violation::Protocol));
    // A declared length of zero or above the bootstrap bound is refused from
    // the prefix alone.
    assert_eq!(
        StdoutEnvelope::new().admit(&[0, 0, 0, 0]),
        Err(Violation::Protocol)
    );
    assert_eq!(
        StdoutEnvelope::new().admit(&[0, 0, 0x40, 1]),
        Err(Violation::Protocol)
    );

    // The real bootstrap owner accepts the Ready only if every byte reached
    // it in order: its envelope holds NUL and 0xFF bytes.
    let rendered = rendered_case();
    let shapes: [(fn(&[u8]) -> Vec<u8>, bool); 4] = [
        (fragments, false),
        (several_messages, false),
        (whole, false),
        (several_messages, true),
    ];
    for (shape, dribble) in shapes {
        let script = Script {
            dribble,
            ..Script::ready(shape)
        };
        let concluded = play(
            &rendered.binding,
            script,
            SOON,
            Some(BracketVerdictV2::Proven),
        )
        .await;
        let outcome = concluded.result.expect("the reassembled Ready is accepted");
        assert_eq!(
            outcome.preamble().provenance(),
            BootstrapProvenanceV2::Declared
        );
        assert_eq!(outcome.identity(), &fixture_identity());
        assert_eq!(outcome.end(), &AttachEndV2::ClosedByUsAfterIdentity);
        let ready = &concluded.seen.ready;
        assert!(ready.contains(&0x00) && ready.contains(&0xFF));
        assert_eq!(concluded.report.stdout_bytes(), ready.len() as u64);
        assert_eq!(
            concluded.report.end(),
            Some(&AttachEndV2::ClosedByUsAfterIdentity)
        );
        assert_eq!(concluded.report.cause(), None);
        assert!(!concluded.report.request_initiated());
    }
}

#[tokio::test]
async fn stderr_is_counted_then_discarded_and_bounded() {
    let rendered = rendered_case();
    // Counted, never mixed into the envelope and never printed.
    let script = Script {
        first: server_message(2, STDERR_MARKER),
        ..Script::ready(whole)
    };
    let concluded = play(
        &rendered.binding,
        script,
        SOON,
        Some(BracketVerdictV2::Proven),
    )
    .await;
    let outcome = concluded
        .result
        .clone()
        .expect("stderr does not reach stdout");
    assert_eq!(concluded.report.stderr_bytes(), STDERR_MARKER.len() as u64);
    assert_eq!(
        concluded.report.stdout_bytes(),
        concluded.seen.ready.len() as u64
    );
    let marker = std::str::from_utf8(STDERR_MARKER).unwrap();
    for printed in [
        format!("{outcome:?}"),
        format!("{:?}", concluded.report),
        format!("{:?}", concluded.attempt),
    ] {
        assert!(!printed.contains(marker), "{printed}");
    }
    // Exactly the bound is accepted; one byte beyond it ends the attempt.
    let full = vec![b'e'; MAX_STDERR_BYTES as usize];
    let concluded = play(
        &rendered.binding,
        Script::first([server_message(2, &full), server_message(2, &[])].concat()),
        SHORT,
        None,
    )
    .await;
    assert_eq!(concluded.result.err(), Some(DEADLINE));
    assert_eq!(concluded.report.stderr_bytes(), MAX_STDERR_BYTES);
    let concluded = play(
        &rendered.binding,
        Script::first([server_message(2, &full), server_message(2, b"x")].concat()),
        SOON,
        None,
    )
    .await;
    assert_refused_stream(&concluded, PROTOCOL, 1009);
    assert_eq!(concluded.report.stderr_bytes(), MAX_STDERR_BYTES);
}

#[tokio::test]
async fn status_channel_is_reduced_to_the_closed_projection() {
    // The projection itself.
    let failure = format!(
        "{{\"kind\":\"Status\",\"apiVersion\":\"v1\",\"metadata\":{{}},\"status\":\"Failure\",\
         \"message\":\"{STATUS_MARKER}\",\"reason\":\"NonZeroExitCode\",\
         \"details\":{{\"causes\":[{{\"reason\":\"ExitCode\",\"message\":\"66\"}}]}},\"code\":500}}"
    );
    assert_eq!(
        project_status(br#"{"kind":"Status","metadata":{},"status":"Success"}"#),
        Ok(AttachEndV2::StatusSuccess)
    );
    let failed = |reason: Option<AttachStatusReasonV2>| Ok(AttachEndV2::StatusFailure { reason });
    assert_eq!(
        project_status(failure.as_bytes()),
        failed(Some(AttachStatusReasonV2::NonZeroExitCode))
    );
    assert_eq!(
        project_status(br#"{"status":"Failure","reason":"InternalError"}"#),
        failed(Some(AttachStatusReasonV2::InternalError))
    );
    // An absent, null or empty reason says nothing.
    for silent in [
        br#"{"status":"Failure"}"#.as_slice(),
        br#"{"status":"Failure","reason":null}"#.as_slice(),
        br#"{"status":"Failure","reason":""}"#.as_slice(),
    ] {
        assert_eq!(project_status(silent), failed(None));
    }
    // Any other text is `Other`, whatever its length, case or characters,
    // and no byte of it is kept.
    let long_reason = format!(
        "{{\"status\":\"Failure\",\"reason\":\"{}\"}}",
        "R".repeat(65)
    );
    let chosen = format!("{{\"status\":\"Failure\",\"reason\":\"{REASON_MARKER}\"}}");
    for other in [
        br#"{"status":"Failure","reason":"not a reason!"}"#.as_slice(),
        br#"{"status":"Failure","reason":"nonzeroexitcode"}"#.as_slice(),
        long_reason.as_bytes(),
        chosen.as_bytes(),
    ] {
        let projected = project_status(other);
        assert_eq!(projected, failed(Some(AttachStatusReasonV2::Other)));
        assert!(!format!("{projected:?}").contains(REASON_MARKER));
    }
    let oversized = format!("{{\"status\":\"Success\"{}}}", " ".repeat(MAX_STATUS_BYTES));
    assert_eq!(project_status(oversized.as_bytes()), Err(Violation::TooBig));
    // A reason that is not a string refuses the document.
    for refused in [
        b"not json".as_slice(),
        br#"{"status":"Success","status":"Success"}"#.as_slice(),
        br#"{"status":"Unknown"}"#.as_slice(),
        br#"{"reason":"NonZeroExitCode"}"#.as_slice(),
        br#"["Success"]"#.as_slice(),
        br#"{"status":"Failure","reason":7}"#.as_slice(),
        br#"{"status":"Failure","reason":["NonZeroExitCode"]}"#.as_slice(),
        br#"{"status":"Success","reason":true}"#.as_slice(),
    ] {
        assert_eq!(project_status(refused), Err(Violation::Protocol));
    }

    // Through the stream: a status ends the attempt without identity, and
    // only the closed reason is kept.
    let rendered = rendered_case();
    let success = server_message(3, br#"{"kind":"Status","metadata":{},"status":"Success"}"#);
    let concluded = play(&rendered.binding, Script::first(success), SOON, None).await;
    assert_eq!(concluded.result.err(), Some(UNPROVEN));
    assert_eq!(concluded.report.end(), Some(&AttachEndV2::StatusSuccess));
    assert_eq!(concluded.seen.stdin_closes, 1);
    let concluded = play(
        &rendered.binding,
        Script::first(server_message(3, failure.as_bytes())),
        SOON,
        None,
    )
    .await;
    assert_eq!(concluded.result.as_ref().err(), Some(&UNPROVEN));
    assert_eq!(
        concluded.report.end(),
        Some(&AttachEndV2::StatusFailure {
            reason: Some(AttachStatusReasonV2::NonZeroExitCode)
        })
    );
    for printed in [
        format!("{:?}", concluded.result),
        format!("{:?}", concluded.report),
        format!("{:?}", concluded.attempt),
        UNPROVEN.to_string(),
    ] {
        assert!(!printed.contains(STATUS_MARKER), "{printed}");
    }
    // A reason chosen by the peer is recorded as `Other` and never printed.
    let concluded = play(
        &rendered.binding,
        Script::first(server_message(3, chosen.as_bytes())),
        SOON,
        None,
    )
    .await;
    assert_eq!(concluded.result.as_ref().err(), Some(&UNPROVEN));
    assert_eq!(
        concluded.report.end(),
        Some(&AttachEndV2::StatusFailure {
            reason: Some(AttachStatusReasonV2::Other)
        })
    );
    for printed in [
        format!("{:?}", concluded.result),
        format!("{:?}", concluded.report),
        format!("{:?}", concluded.attempt),
    ] {
        assert!(!printed.contains(REASON_MARKER), "{printed}");
    }
    for (document, code) in [
        (oversized.as_bytes(), 1009),
        (b"not json".as_slice(), 1002),
        (
            br#"{"status":"Success","status":"Success"}"#.as_slice(),
            1002,
        ),
        (br#"{"status":"Unknown"}"#.as_slice(), 1002),
    ] {
        let script = Script::first(server_message(3, document));
        let concluded = play(&rendered.binding, script, SOON, None).await;
        assert_refused_stream(&concluded, PROTOCOL, code);
    }
}

#[tokio::test]
async fn backpressure_and_half_close_keep_bounds() {
    let rendered = rendered_case();
    // A peer that never reads: the Hello write waits in a 64-byte pipe and
    // the attempt ends at its deadline, with nothing buffered beyond it.
    let started = Instant::now();
    let (wire, mut unread) = open(&rendered.binding, SHORT, 64);
    assert_eq!(watched(wire.attempt.join()).await.err(), Some(DEADLINE));
    assert!(started.elapsed() >= SHORT);
    assert!(started.elapsed() < SHORT + Duration::from_secs(2));
    let report = wire.attempt.report().unwrap();
    assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsWithoutIdentity));
    assert_eq!(wire.slots.available_permits(), 1);
    // The cut frame is not followed by any other frame: only the bytes that
    // fitted the pipe are there, then the end of the stream.
    let mut buffered = Vec::new();
    watched(unread.read_to_end(&mut buffered)).await.unwrap();
    assert!(buffered.len() <= 64, "{}", buffered.len());
    drop(wire.bracket);

    // The peer ends the stream without a close frame before the Ready.
    let script = Script {
        after_hello: vec![Step::Hangup],
        ..Script::default()
    };
    let concluded = play(&rendered.binding, script, SOON, None).await;
    assert_eq!(concluded.result.err(), Some(UNPROVEN));
    assert_eq!(concluded.report.end(), Some(&AttachEndV2::PeerClosed));
    assert!(!concluded.ready_accepted);

    // A close frame from the peer is the same end.
    let script = Script {
        after_hello: vec![Step::Raw(server_frame(0x88, &1000_u16.to_be_bytes()))],
        ..Script::default()
    };
    let concluded = play(&rendered.binding, script, SOON, None).await;
    assert_eq!(concluded.result.err(), Some(UNPROVEN));
    assert_eq!(concluded.report.end(), Some(&AttachEndV2::PeerClosed));

    // The stdin close is sent exactly once, and a Ready that arrives after
    // it is not delivered.
    let script = Script {
        after_stdin_close: Some(whole),
        ..Script::default()
    };
    let concluded = play(&rendered.binding, script, SHORT, None).await;
    assert_eq!(concluded.result.err(), Some(DEADLINE));
    assert_eq!(
        concluded.report.end(),
        Some(&AttachEndV2::ClosedByUsWithoutIdentity)
    );
    assert_eq!(concluded.seen.stdin_closes, 1);
    assert_eq!(concluded.seen.stdin_bytes_after_close, 0);
    assert_eq!(concluded.seen.closes, [1000]);
    assert_eq!(concluded.report.stdout_bytes(), 0);
    assert!(!concluded.ready_accepted);

    // An unproven verdict of the caller closes at once, with its cause.
    let concluded = play(
        &rendered.binding,
        Script::ready(whole),
        SOON,
        Some(BracketVerdictV2::Unproven(AttachCauseV2::InstanceChanged)),
    )
    .await;
    assert_eq!(concluded.result.err(), Some(UNPROVEN));
    assert_eq!(
        concluded.report.cause(),
        Some(AttachCauseV2::InstanceChanged)
    );
    assert_eq!(
        concluded.report.end(),
        Some(&AttachEndV2::ClosedByUsWithoutIdentity)
    );
    assert_eq!(concluded.seen.stdin_closes, 1);
}

// ---- bracket over the loopback peer -----------------------------------------

#[tokio::test]
async fn websocket_handshake_requires_exact_accept_and_v5_subprotocol() {
    // The closed acceptance, without a socket.
    let exact = [
        ("upgrade", "websocket"),
        ("connection", "Upgrade"),
        ("sec-websocket-accept", RFC_ACCEPT),
        ("sec-websocket-protocol", "v5.channel.k8s.io"),
    ];
    let with = |extra: (&'static str, &'static str)| {
        let mut headers = exact.to_vec();
        headers.push(extra);
        headers
    };
    let without = |name: &str| -> Vec<(&'static str, &'static str)> {
        exact
            .iter()
            .copied()
            .filter(|(header, _)| *header != name)
            .collect()
    };
    let replaced = |name: &str, value: &'static str| -> Vec<(&'static str, &'static str)> {
        exact
            .iter()
            .map(|(header, current)| (*header, if *header == name { value } else { *current }))
            .collect()
    };
    let refused = Err(KubernetesSessionRuntimeError::AttachHandshakeRefused);
    assert_eq!(
        validate_attach_upgrade(101, &header_map(&exact), RFC_KEY),
        Ok(())
    );
    let tolerated = [
        ("upgrade", "WebSocket"),
        ("connection", "keep-alive, upgrade"),
        exact[2],
        exact[3],
    ];
    assert_eq!(
        validate_attach_upgrade(101, &header_map(&tolerated), RFC_KEY),
        Ok(())
    );
    for status in [100, 200, 204, 302, 400, 403, 426, 500] {
        assert_eq!(
            validate_attach_upgrade(status, &header_map(&exact), RFC_KEY),
            refused,
            "{status}"
        );
    }
    for headers in [
        without("upgrade"),
        replaced("upgrade", "h2c"),
        with(("upgrade", "websocket")),
        without("connection"),
        replaced("connection", "keep-alive"),
        without("sec-websocket-accept"),
        replaced("sec-websocket-accept", "s3pPLMBiTxaQ9kYGzzhZRbK+xOO="),
        with(("sec-websocket-accept", RFC_ACCEPT)),
        without("sec-websocket-protocol"),
        replaced("sec-websocket-protocol", "v4.channel.k8s.io"),
        replaced("sec-websocket-protocol", "V5.CHANNEL.K8S.IO"),
        replaced(
            "sec-websocket-protocol",
            "v5.channel.k8s.io, v4.channel.k8s.io",
        ),
        with(("sec-websocket-protocol", "v5.channel.k8s.io")),
        with(("sec-websocket-extensions", "permessage-deflate")),
        with(("sec-websocket-extensions", "")),
    ] {
        assert_eq!(
            validate_attach_upgrade(101, &header_map(&headers), RFC_KEY),
            refused,
            "{headers:?}"
        );
    }
    // The accept value belongs to the key of this request.
    assert_eq!(
        validate_attach_upgrade(101, &header_map(&exact), "AAAAAAAAAAAAAAAAAAAAAA=="),
        refused
    );

    // The real request, through the real client.
    let case = Case::new().await;
    let exact_lines = [
        ANCHORS[0].to_owned(),
        ANCHORS[1].to_owned(),
        case.pod_line(),
        case.attach_line(),
    ];
    let start = case.peer.requests().len();
    let seen = case.expect_attach(Script::ready(whole));
    let (result, _, report) = case.attempt(SOON).await;
    assert_eq!(result, Ok(()));
    assert!(report.request_initiated());
    case.peer.finish_scripts().await;
    assert_eq!(snapshot(&seen).closes, [1000]);
    let attach = case.world.lock().unwrap().attach_requests[0].clone();
    assert_eq!(attach.method(), "GET");
    assert_eq!(attach.target(), attach_target(case.pod_name()));
    assert!(attach.body().is_empty());
    assert_eq!(attach.header("connection"), Some("Upgrade"));
    assert_eq!(attach.header("upgrade"), Some("websocket"));
    assert_eq!(attach.header("sec-websocket-version"), Some("13"));
    assert_eq!(
        attach.header("sec-websocket-protocol"),
        Some("v5.channel.k8s.io")
    );
    assert_eq!(attach.header("sec-websocket-extensions"), None);
    let key = attach.header("sec-websocket-key").unwrap();
    assert_eq!(STANDARD.decode(key).unwrap().len(), 16);
    let bearer = format!("Bearer {TOKEN}");
    assert_eq!(attach.header("authorization"), Some(bearer.as_str()));
    let mut accepted = exact_lines.to_vec();
    accepted.extend([case.pod_line(), ANCHORS[0].into(), ANCHORS[1].into()]);
    assert_eq!(case.lines_since(start), accepted);

    // Every other answer is refused with zero frames sent, and no request
    // follows it: no redirect is followed and nothing is retried.
    for status in [200_u16, 400, 403, 500] {
        let start = case.peer.requests().len();
        case.expect_reply(move |_: &PeerRequest| {
            PeerReply::json(status, &json!({"kind":"Status","code":status}))
        });
        let (result, _, report) = case.attempt(SOON).await;
        assert_eq!(result, refused, "{status}");
        assert!(report.request_initiated());
        assert_eq!(report.end(), None);
        assert_eq!(case.lines_since(start), exact_lines, "{status}");
    }
    let start = case.peer.requests().len();
    case.expect_reply(|_: &PeerRequest| {
        PeerReply::head(
            302,
            &[(
                "location",
                "/api/v1/namespaces/tenant-a/pods/elsewhere/attach",
            )],
        )
    });
    let (result, _, report) = case.attempt(SOON).await;
    assert_eq!(result, refused);
    assert_eq!(report.end(), None);
    assert_eq!(case.lines_since(start), exact_lines);
    let mutations: [fn(&mut Vec<(String, String)>); 8] = [
        |headers| {
            set_header(
                headers,
                "Sec-WebSocket-Accept",
                "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=",
            )
        },
        |headers| headers.retain(|(name, _)| name != "Upgrade"),
        |headers| set_header(headers, "Connection", "keep-alive"),
        |headers| set_header(headers, "Sec-WebSocket-Protocol", "v4.channel.k8s.io"),
        |headers| headers.retain(|(name, _)| name != "Sec-WebSocket-Protocol"),
        |headers| headers.push(("Sec-WebSocket-Protocol".into(), "v5.channel.k8s.io".into())),
        |headers| headers.push(("Sec-WebSocket-Protocol".into(), "v4.channel.k8s.io".into())),
        |headers| {
            headers.push((
                "Sec-WebSocket-Extensions".into(),
                "permessage-deflate".into(),
            ))
        },
    ];
    for (index, mutate) in mutations.into_iter().enumerate() {
        let start = case.peer.requests().len();
        let seen = case.expect_stream(Script::default(), None, mutate);
        let (result, _, report) = case.attempt(SOON).await;
        assert_eq!(result, refused, "mutation {index}");
        assert!(report.request_initiated());
        assert_eq!(report.end(), None);
        assert_eq!(case.lines_since(start), exact_lines, "mutation {index}");
        // The fixture stream saw the connection end without one frame.
        case.peer.finish_scripts().await;
        let seen = snapshot(&seen);
        assert!(seen.ended, "mutation {index}");
        assert!(seen.stdin.is_empty() && seen.closes.is_empty());
        assert_eq!((seen.stdin_closes, seen.unexpected), (0, 0));
    }
    // A lost answer is the transport failure of an initiated request. It is
    // reported as such and never retried.
    let start = case.peer.requests().len();
    case.expect_reply(|_: &PeerRequest| PeerReply::CloseWithoutResponse);
    let (result, _, report) = case.attempt(SOON).await;
    assert_eq!(
        result,
        Err(KubernetesSessionRuntimeError::Kubernetes(
            KubernetesError::TransportUnavailable
        ))
    );
    assert!(report.request_initiated());
    assert_eq!(report.end(), None);
    assert_eq!(case.lines_since(start), exact_lines);
    // An upgrade that is never answered ends at the caller deadline.
    case.expect_reply(|_: &PeerRequest| PeerReply::never_answer());
    let (result, _, report) = case.attempt(SHORT).await;
    assert_eq!(result, Err(DEADLINE));
    assert!(report.request_initiated());
    assert_eq!(report.end(), None);
    assert_eq!(case.attach_count(), 16);
}

#[tokio::test]
async fn refused_guard_cancellation_or_changed_anchor_opens_no_attach_connection() {
    let case = Case::new().await;
    let refused = Err(REFUSED);
    let digest = case.attach_digest();
    let not_cancelled = AtomicBool::new(false);
    let observed = [
        ANCHORS[0].to_owned(),
        ANCHORS[1].to_owned(),
        case.pod_line(),
    ];
    // A guard for another kind, another digest or another listener refuses
    // after identity one and before any attach request.
    let foreign_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    for guard in [
        LoopbackEffectGuardV2::new(
            SessionEffectKindV2::DeleteExact,
            &digest,
            case.peer.listener(),
        )
        .unwrap(),
        LoopbackEffectGuardV2::new(
            SessionEffectKindV2::Attach,
            &"e".repeat(64),
            case.peer.listener(),
        )
        .unwrap(),
        LoopbackEffectGuardV2::new(SessionEffectKindV2::Attach, &digest, &foreign_listener)
            .unwrap(),
    ] {
        let start = case.peer.requests().len();
        case.set_pods(vec![PodAnswer::Json(200, case.running.clone())]);
        let attempt = case.new_attempt();
        assert_eq!(
            case.attach_with(&attempt, guard, &not_cancelled, SOON)
                .await,
            refused
        );
        assert_eq!(case.lines_since(start), observed);
        let report = attempt.report().unwrap();
        assert!(!report.request_initiated());
        assert_eq!(report.end(), None);
        assert_eq!(watched(attempt.join()).await.err(), Some(REFUSED));
    }
    // A cancellation raised before the call sends nothing at all.
    let start = case.peer.requests().len();
    let attempt = case.new_attempt();
    assert_eq!(
        case.attach_with(&attempt, case.attach_guard(), &AtomicBool::new(true), SOON)
            .await,
        refused
    );
    assert!(case.lines_since(start).is_empty());
    // A cancellation raised while identity one is read is refused by the
    // transport itself: this guard ignores the flag.
    let raised = Arc::new(AtomicBool::new(false));
    case.world.lock().unwrap().raise_on_pod_get = Some((1, raised.clone()));
    let ignoring = LoopbackEffectGuardV2::ignoring_cancellation(
        SessionEffectKindV2::Attach,
        &digest,
        case.peer.listener(),
    )
    .unwrap();
    let start = case.peer.requests().len();
    let attempt = case.new_attempt();
    assert_eq!(
        case.attach_with(&attempt, ignoring, &raised, SOON).await,
        refused
    );
    assert!(raised.load(Ordering::Acquire));
    assert_eq!(case.lines_since(start), observed);
    assert!(!attempt.report().unwrap().request_initiated());
    case.world.lock().unwrap().raise_on_pod_get = None;
    // A replaced namespace refuses before the Pod is even read.
    case.world.lock().unwrap().namespace_uid = CHANGED_NAMESPACE_UID.into();
    let start = case.peer.requests().len();
    let attempt = case.new_attempt();
    assert_eq!(
        case.attach(&attempt, SOON).await,
        Err(KubernetesSessionRuntimeError::Kubernetes(
            KubernetesError::AuthorityChanged
        ))
    );
    assert_eq!(case.lines_since(start), ANCHORS);
    // No attach request ever reached the peer, and every connection carried
    // a parsed, recorded request.
    assert_eq!(case.attach_count(), 0);
    assert_eq!(case.peer.accepted(), case.peer.requests().len());
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
}

#[tokio::test]
async fn preamble_runs_over_a_real_attach_stream_against_the_real_worker_mode() {
    let case = Case::new().await;
    let (worker_end, bridge_end) = tokio::io::duplex(64 * 1024);
    let (worker_input, worker_output) = tokio::io::split(worker_end);
    let worker_pool = bootstrap_pool();
    let worker = serve_waiting_bootstrap(
        worker_input,
        worker_output,
        bootstrap_capacity(&worker_pool),
    );
    let script = Script {
        first: server_message(2, STDERR_MARKER),
        ..Script::ready(whole)
    };
    let seen = case.expect_stream(script, Some(bridge_end), |_| {});
    let attempt = case.new_attempt();
    let start = case.peer.requests().len();
    let (attached, worker_end) =
        watched(async { tokio::join!(case.attach(&attempt, SOON), worker) }).await;
    assert_eq!(attached, Ok(()));
    let outcome = watched(attempt.join()).await.unwrap();
    assert_eq!(
        outcome.preamble().provenance(),
        BootstrapProvenanceV2::Declared
    );
    assert_eq!(outcome.end(), &AttachEndV2::ClosedByUsAfterIdentity);
    // The identity fields of the two equal observations.
    let identity = outcome.identity();
    assert_eq!(identity.pod_uid(), POD_UID);
    assert_eq!(identity.node_name(), NODE);
    assert_eq!(identity.container_id(), CONTAINER_ID);
    assert_eq!(identity.image_id(), IMAGE_ID);
    assert_eq!(
        identity.started_at(),
        STARTED_AT.parse::<DateTime<Utc>>().unwrap()
    );
    // The real worker mode answered, then saw its input end when stdin was
    // closed: no handoff, exit 66, never a success of a session.
    assert_eq!(worker_end, Ok(WorkerBootstrapEndV2::ReadyThenEof));
    assert_eq!(waiting_exit_code(&worker_end), EXIT_NO_HANDOFF);
    assert_eq!(EXIT_NO_HANDOFF, 66);
    // Anchors, Pod, attach, Pod, anchors.
    assert_eq!(
        case.lines_since(start),
        [
            ANCHORS[0].to_owned(),
            ANCHORS[1].to_owned(),
            case.pod_line(),
            case.attach_line(),
            case.pod_line(),
            ANCHORS[0].to_owned(),
            ANCHORS[1].to_owned(),
        ]
    );
    case.peer.finish_scripts().await;
    let seen = snapshot(&seen);
    assert_exactly_one_hello(&seen.stdin, &case.rendered.binding);
    assert_eq!((seen.stdin_closes, seen.unexpected), (1, 0));
    assert_eq!(seen.closes, [1000]);
    // Stderr was counted and never reached the bootstrap reader: the reader
    // received exactly the worker's Ready envelope.
    let report = attempt.report().unwrap();
    assert_eq!(report.stderr_bytes(), STDERR_MARKER.len() as u64);
    assert_eq!(report.stdout_bytes(), seen.ready.len() as u64);
    assert!(report.request_initiated());
    assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsAfterIdentity));
    assert_eq!(report.cause(), None);
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
}

#[tokio::test]
async fn dropped_bootstrap_halves_do_not_close_stdin_before_identity_two() {
    let case = Case::new().await;
    let seen = case.expect_attach(Script::ready(whole));
    let (result, _, _) = case.attempt(SOON).await;
    assert_eq!(result, Ok(()));
    case.peer.finish_scripts().await;
    let seen = snapshot(&seen);
    // The bootstrap owner dropped its halves when the Ready arrived, before
    // the second Pod GET. Stdin was closed only after that GET was served.
    assert_eq!(seen.stdin_closes, 1);
    assert_eq!(seen.pod_gets_at_stdin_close, Some(2));
    assert_eq!(case.pod_gets(), 2);
}

#[tokio::test]
async fn replaced_pod_between_inspections_refuses_after_only_the_hello() {
    let case = Case::new().await;
    case.set_pods(vec![
        PodAnswer::Json(200, case.running.clone()),
        PodAnswer::Json(
            200,
            replace(&case.running, "/metadata/uid", json!(OTHER_POD_UID)),
        ),
    ]);
    let seen = case.expect_attach(Script::ready(whole));
    let (result, _, report) = case.attempt(SOON).await;
    assert_eq!(result, Err(UNPROVEN));
    assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsWithoutIdentity));
    assert_eq!(
        report.cause(),
        Some(AttachCauseV2::SecondObservation(
            KubernetesSessionRuntimeError::InstanceReplaced
        ))
    );
    case.peer.finish_scripts().await;
    let seen = snapshot(&seen);
    // Exactly the Hello, then the stdin close and the close frame.
    assert_exactly_one_hello(&seen.stdin, &case.rendered.binding);
    assert_eq!(seen.stdin_closes, 1);
    assert_eq!(seen.stdin_bytes_after_close, 0);
    assert_eq!(seen.closes, [1000]);
    assert_eq!(seen.unexpected, 0);
    assert!(seen.pongs.is_empty());
    assert_eq!(case.attach_count(), 1);
}

#[tokio::test]
async fn new_container_id_or_restart_between_inspections_refuses_attach_identity() {
    let case = Case::new().await;
    let running = &case.running;
    let container = "/status/containerStatuses/0";
    let changed = Some(AttachCauseV2::InstanceChanged);
    let second = |error| Some(AttachCauseV2::SecondObservation(error));
    let api = |error| second(KubernetesSessionRuntimeError::Kubernetes(error));
    let cases = [
        (
            PodAnswer::Json(
                200,
                replace(
                    running,
                    &format!("{container}/containerID"),
                    json!("containerd://ffffffffffffffff"),
                ),
            ),
            changed,
        ),
        (
            PodAnswer::Json(
                200,
                replace(
                    running,
                    &format!("{container}/state/running/startedAt"),
                    json!("2026-11-12T08:03:00Z"),
                ),
            ),
            changed,
        ),
        (
            PodAnswer::Json(
                200,
                replace(
                    running,
                    &format!("{container}/imageID"),
                    json!("sha256:0000000000000000"),
                ),
            ),
            changed,
        ),
        (
            PodAnswer::Json(
                200,
                replace(running, &format!("{container}/restartCount"), json!(1)),
            ),
            second(KubernetesSessionRuntimeError::InstanceRestarted),
        ),
        (
            PodAnswer::Json(200, replace(running, "/spec/hostNetwork", json!(true))),
            second(KubernetesSessionRuntimeError::ProfileChanged),
        ),
        (
            PodAnswer::Json(
                200,
                replace(
                    running,
                    "/metadata/deletionTimestamp",
                    json!("2026-11-12T08:05:00Z"),
                ),
            ),
            changed,
        ),
        (
            PodAnswer::Json(200, with_status(running, terminated_status())),
            changed,
        ),
        (
            PodAnswer::ThenNamespace(running.clone(), CHANGED_NAMESPACE_UID),
            api(KubernetesError::AuthorityChanged),
        ),
        (
            PodAnswer::Json(500, json!({"kind":"Status","code":500})),
            api(KubernetesError::ApiRejected(500)),
        ),
        (
            PodAnswer::Json(404, json!({"kind":"Status","code":404})),
            api(KubernetesError::ApiRejected(404)),
        ),
    ];
    for (index, (answer, cause)) in cases.into_iter().enumerate() {
        case.set_pods(vec![PodAnswer::Json(200, running.clone()), answer]);
        let seen = case.expect_attach(Script::ready(whole));
        let (result, _, report) = case.attempt(SOON).await;
        assert_eq!(result, Err(UNPROVEN), "case {index}");
        assert_eq!(report.cause(), cause, "case {index}");
        assert_eq!(
            report.end(),
            Some(&AttachEndV2::ClosedByUsWithoutIdentity),
            "case {index}"
        );
        case.peer.finish_scripts().await;
        assert_eq!(snapshot(&seen).stdin_closes, 1, "case {index}");
        assert_eq!(case.attach_count(), index + 1);
    }
    // No answer to the second Pod GET: unproven at the caller deadline,
    // whichever of the caller and the owner task notices it first.
    case.set_pods(vec![
        PodAnswer::Json(200, running.clone()),
        PodAnswer::Never,
    ]);
    let seen = case.expect_attach(Script::ready(whole));
    let (result, _, report) = case.attempt(SOON).await;
    assert_eq!(result, Err(UNPROVEN));
    assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsWithoutIdentity));
    assert!(matches!(
        report.cause(),
        None | Some(AttachCauseV2::SecondObservation(
            KubernetesSessionRuntimeError::Kubernetes(KubernetesError::Timeout)
        ))
    ));
    case.peer.finish_scripts().await;
    assert_eq!(snapshot(&seen).stdin_closes, 1);
}

#[tokio::test]
async fn first_observation_must_be_a_running_exact_instance_before_any_connection() {
    let case = Case::new().await;
    let held = admitted(&case.rendered.request);
    let mut released = held.clone();
    assert!(
        released["spec"]
            .as_object_mut()
            .unwrap()
            .remove("schedulingGates")
            .is_some()
    );
    let waiting = json!({"phase":"Pending","containerStatuses":[{"name":"worker",
        "restartCount":0,"containerID":"","imageID":"",
        "state":{"waiting":{"reason":"ContainerCreating"}}}]});
    let pods = [
        held,
        released,
        with_status(&case.running, waiting),
        with_status(&case.running, terminated_status()),
        replace(
            &case.running,
            "/metadata/deletionTimestamp",
            json!("2026-11-12T08:05:00Z"),
        ),
        replace(
            &case.running,
            "/status/containerStatuses/0/imageID",
            json!(""),
        ),
    ];
    for (index, pod) in pods.into_iter().enumerate() {
        let start = case.peer.requests().len();
        case.set_pods(vec![PodAnswer::Json(200, pod)]);
        let (result, _, report) = case.attempt(SOON).await;
        assert_eq!(result, Err(REFUSED), "case {index}");
        assert!(!report.request_initiated(), "case {index}");
        assert_eq!(report.end(), None);
        assert_eq!(
            case.lines_since(start),
            [
                ANCHORS[0].to_owned(),
                ANCHORS[1].to_owned(),
                case.pod_line()
            ],
            "case {index}"
        );
    }
    // A replaced or missing Pod keeps the parser's own closed error.
    for (answer, error) in [
        (
            PodAnswer::Json(
                200,
                replace(&case.running, "/metadata/uid", json!(OTHER_POD_UID)),
            ),
            KubernetesSessionRuntimeError::InstanceReplaced,
        ),
        (
            PodAnswer::Json(404, json!({"kind":"Status","code":404})),
            KubernetesSessionRuntimeError::Kubernetes(KubernetesError::ApiRejected(404)),
        ),
    ] {
        case.set_pods(vec![answer]);
        let (result, _, report) = case.attempt(SOON).await;
        assert_eq!(result, Err(error));
        assert!(!report.request_initiated());
    }
    assert_eq!(case.attach_count(), 0);
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
}

#[tokio::test]
async fn bytes_after_ready_are_refused_even_when_the_bracket_matches() {
    fn another_envelope(ready: &[u8]) -> Vec<u8> {
        [server_message(1, ready), server_message(1, ready)].concat()
    }
    fn one_stray_byte(ready: &[u8]) -> Vec<u8> {
        [server_message(1, ready), server_message(1, b"x")].concat()
    }
    let case = Case::new().await;
    // Both observations are equal; the extra stdout bytes arrive with the
    // Ready, in the same write.
    let extras: [fn(&[u8]) -> Vec<u8>; 2] = [another_envelope, one_stray_byte];
    for extra in extras {
        case.set_pods(vec![PodAnswer::Json(200, case.running.clone())]);
        let seen = case.expect_attach(Script::ready(extra));
        let (result, _, report) = case.attempt(SOON).await;
        assert_eq!(result, Err(PROTOCOL));
        assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsWithoutIdentity));
        case.peer.finish_scripts().await;
        let seen = snapshot(&seen);
        // Only the one envelope was handed to the bootstrap reader.
        assert_eq!(report.stdout_bytes(), seen.ready.len() as u64);
        assert_eq!(seen.closes, [1002]);
        assert_eq!(seen.stdin_closes, 1);
    }
    // The stray byte arrives after the Ready was accepted, while identity
    // two is being read: still a refusal, never a success. Identity two is
    // never answered here, and the caller does not wait for it: it returns
    // the owner's record as soon as the owner ended, before the deadline.
    case.set_pods(vec![
        PodAnswer::Json(200, case.running.clone()),
        PodAnswer::Never,
    ]);
    let script = Script {
        after_hello: vec![
            Step::Reply(whole),
            Step::AwaitSecondPodGet,
            Step::Raw(server_message(1, b"x")),
        ],
        ..Script::default()
    };
    let seen = case.expect_attach(script);
    let started = Instant::now();
    let (result, _, report) = case.attempt(SOON).await;
    assert!(
        started.elapsed() < SOON,
        "the caller waited for identity two although the owner had ended"
    );
    assert_eq!(result, Err(PROTOCOL));
    assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsWithoutIdentity));
    assert_eq!(report.cause(), None);
    case.peer.finish_scripts().await;
    assert_eq!(snapshot(&seen).closes, [1002]);
    assert_eq!(case.pod_gets(), 2);

    // A peer close while identity two is being read is returned at once too.
    case.set_pods(vec![
        PodAnswer::Json(200, case.running.clone()),
        PodAnswer::Never,
    ]);
    let script = Script {
        after_hello: vec![Step::Reply(whole), Step::AwaitSecondPodGet, Step::Hangup],
        ..Script::default()
    };
    case.expect_attach(script);
    let started = Instant::now();
    let (result, _, report) = case.attempt(SOON).await;
    assert!(started.elapsed() < SOON);
    assert_eq!(result, Err(UNPROVEN));
    assert_eq!(report.end(), Some(&AttachEndV2::PeerClosed));
    case.peer.finish_scripts().await;
}

#[tokio::test]
async fn expired_deadline_refuses_attach_before_http_initiation() {
    let case = Case::new().await;
    let attempt = case.new_attempt();
    let claim = attempt.claim().unwrap();
    let identity_one = fixture_identity().canonical_bytes().unwrap();
    let start = case.peer.requests().len();
    // Exercise the actual transport boundary after the caller's observation
    // would have completed. No scheduler race or elapsed-time guess is needed:
    // the absolute deadline is already reached when the transport is entered.
    let request = SessionAttachRequestV2 {
        pod_name: case.pod_name(),
        identity_one: &identity_one,
        attempt: &attempt,
        until: Instant::now(),
    };
    let opening = case.runtime.transport.send_session_attach(
        case.rendered.intent.authority(),
        request,
        case.attach_guard(),
        &AtomicBool::new(false),
    );
    let error = match opening {
        Err(error) => error,
        Ok(opening) => {
            drop(opening);
            panic!("an expired attach must be refused before initiation");
        }
    };
    assert_eq!(error, DEADLINE);
    assert!(!attempt.0.initiated.load(Ordering::Acquire));
    assert!(attempt.report().is_none());
    assert_eq!(claim.refuse(error, None), DEADLINE);
    assert_eq!(watched(attempt.join()).await.err(), Some(DEADLINE));
    let report = attempt.report().unwrap();
    assert!(!report.request_initiated());
    assert_eq!(report.end(), None);
    assert_eq!(report.cause(), None);
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
    tokio::task::yield_now().await;
    assert!(case.lines_since(start).is_empty());
    assert_eq!(case.attach_count(), 0);
}

#[tokio::test]
async fn silence_is_bounded_before_and_after_ready() {
    let case = Case::new().await;
    // Ready, then silence: the bracket completes and closes by itself,
    // without waiting for its deadline.
    let started = Instant::now();
    let seen = case.expect_attach(Script::ready(whole));
    let (result, attempt, report) = case.attempt(SOON).await;
    assert_eq!(result, Ok(()));
    assert!(started.elapsed() < SOON);
    assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsAfterIdentity));
    assert_eq!(
        watched(attempt.join()).await.unwrap().end(),
        &AttachEndV2::ClosedByUsAfterIdentity
    );
    case.peer.finish_scripts().await;
    assert_eq!(snapshot(&seen).closes, [1000]);
    // Silence before the Ready: the bootstrap window is the caller deadline
    // here, and the attempt ends there, closed by us without identity.
    case.set_pods(vec![PodAnswer::Json(200, case.running.clone())]);
    let started = Instant::now();
    let seen = case.expect_attach(Script::default());
    let (result, _, report) = case.attempt(SHORT).await;
    assert_eq!(result, Err(DEADLINE));
    assert!(started.elapsed() >= SHORT);
    assert!(started.elapsed() < SHORT + Duration::from_secs(2));
    assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsWithoutIdentity));
    assert!(matches!(
        report.cause(),
        None | Some(AttachCauseV2::Bootstrap(BootstrapError::Deadline))
    ));
    assert_eq!(report.stdout_bytes(), 0);
    case.peer.finish_scripts().await;
    let seen = snapshot(&seen);
    assert_exactly_one_hello(&seen.stdin, &case.rendered.binding);
    assert_eq!(seen.stdin_closes, 1);
    assert_eq!(seen.closes, [1000]);
    assert_eq!(
        case.pod_gets(),
        1,
        "identity two is not read without a Ready"
    );
}

#[tokio::test]
async fn cancelled_waiter_keeps_the_attempt_and_capacity_until_its_end() {
    let case = Case::new().await;
    let seen = [
        case.expect_attach(Script::default()),
        case.expect_attach(Script::default()),
    ];
    let attempts = [case.new_attempt(), case.new_attempt()];
    let cancelled = AtomicBool::new(false);
    let until = Instant::now() + SOON;
    let mut first = Box::pin(case.runtime.attach_exact_instance(
        &case.instance,
        case.rendered.binding.clone(),
        case.attach_guard(),
        &attempts[0],
        bootstrap_capacity(&case.bootstrap),
        until,
        &cancelled,
    ));
    let mut second = Box::pin(case.runtime.attach_exact_instance(
        &case.instance,
        case.rendered.binding.clone(),
        case.attach_guard(),
        &attempts[1],
        bootstrap_capacity(&case.bootstrap),
        until,
        &cancelled,
    ));
    // Both attempts stall before the Ready: each fixture stream read its
    // Hello and stays silent.
    let stalled = until_true(|| {
        seen.iter()
            .all(|seen| envelope_complete(&seen.lock().unwrap().stdin))
    });
    tokio::select! {
        result = &mut first => panic!("the first attach ended early: {result:?}"),
        result = &mut second => panic!("the second attach ended early: {result:?}"),
        () = watched(stalled) => {}
    }
    // Waiters park on the join, then are aborted.
    let waiters: Vec<_> = attempts
        .iter()
        .cloned()
        .map(|attempt| tokio::spawn(async move { attempt.join().await }))
        .collect();
    tokio::time::sleep(Duration::from_millis(20)).await;
    for waiter in waiters {
        assert!(!waiter.is_finished());
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
    }
    // The futures that drove the brackets are dropped as well.
    drop(first);
    drop(second);
    assert!(
        Instant::now() < until,
        "the fixture was too slow to observe the stalled attempts"
    );
    // The attempts, their streams and their capacity outlive every waiter.
    assert_eq!(
        case.runtime.reserve_attach_capacity().err(),
        Some(KubernetesSessionRuntimeError::AttachCapacity)
    );
    assert_eq!(case.runtime.attach_slots_available(), 0);
    for (attempt, seen) in attempts.iter().zip(&seen) {
        assert!(attempt.report().is_none());
        let seen = snapshot(seen);
        assert!(!seen.ended, "the peer connection is still open");
        assert_eq!(seen.stdin_closes, 0);
    }
    // At the deadline each owner closes its stream and records its end.
    for attempt in &attempts {
        assert_eq!(watched(attempt.join()).await.err(), Some(DEADLINE));
        let report = attempt.report().unwrap();
        assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsWithoutIdentity));
        assert!(matches!(
            report.cause(),
            Some(AttachCauseV2::CallerGone | AttachCauseV2::Bootstrap(BootstrapError::Deadline))
        ));
        // A later join returns the same record.
        assert_eq!(watched(attempt.join()).await.err(), Some(DEADLINE));
        assert_eq!(attempt.report(), Some(report));
    }
    assert!(Instant::now() >= until);
    case.peer.finish_scripts().await;
    for seen in &seen {
        let seen = snapshot(seen);
        assert_eq!(seen.stdin_closes, 1);
        assert_eq!(seen.closes, [1000]);
        assert!(seen.ended);
    }
    // The capacity is available only now.
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
    assert_eq!(case.attach_count(), 2);
}

#[tokio::test]
async fn effective_join_returns_one_recorded_outcome_and_releases_capacity_once() {
    let case = Case::new().await;
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
    // Identity two is never answered, so the bracket stays open after the
    // accepted Ready until the caller deadline.
    case.set_pods(vec![
        PodAnswer::Json(200, case.running.clone()),
        PodAnswer::Never,
    ]);
    case.expect_attach(Script::ready(whole));
    let attempt = case.new_attempt();
    assert_eq!(
        case.runtime.attach_slots_available(),
        MAX_CONCURRENT_ATTACH - 1
    );
    let cancelled = AtomicBool::new(false);
    let until = Instant::now() + SOON;
    let mut attach = Box::pin(case.runtime.attach_exact_instance(
        &case.instance,
        case.rendered.binding.clone(),
        case.attach_guard(),
        &attempt,
        bootstrap_capacity(&case.bootstrap),
        until,
        &cancelled,
    ));
    tokio::select! {
        result = &mut attach => panic!("the attach ended early: {result:?}"),
        () = watched(until_true(|| case.pod_gets() == 2)) => {}
    }
    // The stream is owned: the slot is held and nothing is recorded.
    assert!(Instant::now() < until, "the fixture was too slow");
    assert_eq!(
        case.runtime.attach_slots_available(),
        MAX_CONCURRENT_ATTACH - 1
    );
    assert!(attempt.report().is_none());
    let waiters: Vec<_> = (0..2)
        .map(|_| {
            let attempt = attempt.clone();
            tokio::spawn(async move { attempt.join().await })
        })
        .collect();
    // Every waiter and the driving future are dropped: no success follows.
    drop(attach);
    let mut joined = Vec::new();
    for waiter in waiters {
        joined.push(watched(waiter).await.unwrap().err());
    }
    joined.push(watched(attempt.join()).await.err());
    assert_eq!(joined, [Some(UNPROVEN); 3]);
    assert!(Instant::now() >= until);
    let report = attempt.report().unwrap();
    assert_eq!(report.cause(), Some(AttachCauseV2::CallerGone));
    assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsWithoutIdentity));
    // The slot came back once, with the record, and never above the maximum.
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);

    // A recorded success is returned to concurrent and later joins alike.
    case.set_pods(vec![PodAnswer::Json(200, case.running.clone())]);
    case.expect_attach(Script::ready(whole));
    let attempt = case.new_attempt();
    assert_eq!(case.attach(&attempt, SOON).await, Ok(()));
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
    let (one, two) = tokio::join!(attempt.join(), attempt.join());
    let later = watched(attempt.join()).await.unwrap();
    for outcome in [one.unwrap(), two.unwrap()] {
        assert_eq!(outcome.identity(), later.identity());
        assert_eq!(outcome.end(), later.end());
    }
    // Dropping the owner of an ended attempt returns nothing a second time.
    drop(attempt);
    let held = [
        case.runtime.reserve_attach_capacity().unwrap(),
        case.runtime.reserve_attach_capacity().unwrap(),
    ];
    assert_eq!(
        case.runtime.reserve_attach_capacity().err(),
        Some(KubernetesSessionRuntimeError::AttachCapacity)
    );
    drop(held);
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
    // An owner that was never entered has no outcome and still holds its
    // slot until it is dropped.
    let idle = case.new_attempt();
    assert_eq!(idle.join().await.err(), Some(UNPROVEN));
    assert!(idle.report().is_none());
    assert_eq!(
        case.runtime.attach_slots_available(),
        MAX_CONCURRENT_ATTACH - 1
    );
    drop(idle);
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
    case.peer.finish_scripts().await;
}

#[tokio::test]
async fn until_beyond_the_bracket_bound_or_elapsed_is_refused_before_any_request() {
    let case = Case::new().await;
    let start = case.peer.requests().len();
    let not_cancelled = AtomicBool::new(false);
    for until in [
        Instant::now(),
        Instant::now() + ATTACH_BRACKET_MAX + Duration::from_secs(1),
    ] {
        let attempt = case.new_attempt();
        let result = case
            .runtime
            .attach_exact_instance(
                &case.instance,
                case.rendered.binding.clone(),
                case.attach_guard(),
                &attempt,
                bootstrap_capacity(&case.bootstrap),
                until,
                &not_cancelled,
            )
            .await;
        assert_eq!(result, Err(DEADLINE));
        assert_eq!(watched(attempt.join()).await.err(), Some(DEADLINE));
        let report = attempt.report().unwrap();
        assert!(!report.request_initiated());
        assert_eq!(report.end(), None);
    }
    assert!(case.lines_since(start).is_empty());
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
}

#[tokio::test]
async fn no_second_attach_connection_is_ever_opened() {
    let case = Case::new().await;
    // A refused handshake.
    case.expect_reply(|_: &PeerRequest| PeerReply::json(403, &json!({"kind":"Status","code":403})));
    let (result, refused_attempt, _) = case.attempt(SOON).await;
    assert_eq!(
        result,
        Err(KubernetesSessionRuntimeError::AttachHandshakeRefused)
    );
    assert_eq!(case.attach_count(), 1);
    // A protocol error on the stream.
    case.set_pods(vec![PodAnswer::Json(200, case.running.clone())]);
    case.expect_attach(Script::first(server_frame(0x81, b"text")));
    let (result, _, _) = case.attempt(SOON).await;
    assert_eq!(result, Err(PROTOCOL));
    assert_eq!(case.attach_count(), 2);
    // An identity mismatch.
    case.set_pods(vec![
        PodAnswer::Json(200, case.running.clone()),
        PodAnswer::Json(
            200,
            replace(&case.running, "/metadata/uid", json!(OTHER_POD_UID)),
        ),
    ]);
    case.expect_attach(Script::ready(whole));
    let (result, mismatched_attempt, _) = case.attempt(SOON).await;
    assert_eq!(result, Err(UNPROVEN));
    assert_eq!(case.attach_count(), 3);
    // The same owners are refused before any request, and keep their record.
    case.set_pods(vec![PodAnswer::Json(200, case.running.clone())]);
    let start = case.peer.requests().len();
    for (attempt, recorded) in [
        (
            &refused_attempt,
            KubernetesSessionRuntimeError::AttachHandshakeRefused,
        ),
        (&mismatched_attempt, UNPROVEN),
    ] {
        assert_eq!(case.attach(attempt, SOON).await, Err(REFUSED));
        assert_eq!(watched(attempt.join()).await.err(), Some(recorded));
    }
    assert!(case.lines_since(start).is_empty());
    assert_eq!(case.attach_count(), 3);
    assert!(case.world.lock().unwrap().attaches.is_empty());
    case.peer.finish_scripts().await;
}

#[tokio::test]
async fn diagnostics_carry_no_body_token_or_url() {
    let case = Case::new().await;
    let binding = &case.rendered.binding;
    let mut printed = Vec::new();
    // A success with stderr on the way.
    let script = Script {
        first: server_message(2, STDERR_MARKER),
        ..Script::ready(whole)
    };
    case.expect_attach(script);
    let (result, attempt, report) = case.attempt(SOON).await;
    assert_eq!(result, Ok(()));
    let outcome = watched(attempt.join()).await.unwrap();
    printed.extend([
        format!("{outcome:?}"),
        format!("{report:?}"),
        format!("{attempt:?}"),
    ]);
    // A status failure before the Ready, whose reason and message are both
    // text chosen by the peer: only the closed projection is recorded.
    case.set_pods(vec![PodAnswer::Json(200, case.running.clone())]);
    let status = format!(
        "{{\"status\":\"Failure\",\"reason\":\"{REASON_MARKER}\",\"message\":\"{STATUS_MARKER}\"}}"
    );
    case.expect_attach(Script::first(server_message(3, status.as_bytes())));
    let (result, attempt, report) = case.attempt(SOON).await;
    assert_eq!(result, Err(UNPROVEN));
    assert_eq!(
        report.end(),
        Some(&AttachEndV2::StatusFailure {
            reason: Some(AttachStatusReasonV2::Other)
        })
    );
    printed.extend([
        format!("{result:?}"),
        format!("{report:?}"),
        format!("{attempt:?}"),
    ]);
    for reason in [
        AttachStatusReasonV2::NonZeroExitCode,
        AttachStatusReasonV2::InternalError,
        AttachStatusReasonV2::Other,
    ] {
        printed.push(format!("{reason:?}"));
    }
    for error in [
        KubernetesSessionRuntimeError::AttachHandshakeRefused,
        PROTOCOL,
        UNPROVEN,
        KubernetesSessionRuntimeError::AttachCapacity,
        DEADLINE,
    ] {
        printed.extend([error.to_string(), format!("{error:?}")]);
    }
    for cause in [
        AttachCauseV2::Bootstrap(BootstrapError::Binding),
        AttachCauseV2::SecondObservation(KubernetesSessionRuntimeError::InstanceReplaced),
        AttachCauseV2::InstanceChanged,
        AttachCauseV2::Cancelled,
        AttachCauseV2::CallerGone,
        AttachCauseV2::OwnerStopped,
        AttachCauseV2::LocalFailure,
    ] {
        printed.push(format!("{cause:?}"));
    }
    let endpoint = case.peer.endpoint().trim_start_matches("https://");
    let forbidden = [
        TOKEN.to_owned(),
        std::str::from_utf8(STDERR_MARKER).unwrap().to_owned(),
        STATUS_MARKER.to_owned(),
        REASON_MARKER.to_owned(),
        endpoint.to_owned(),
        "https://".to_owned(),
        "/api/v1/".to_owned(),
        // The Hello bytes: its keys and the values of its binding.
        "exchange_id".to_owned(),
        "hello".to_owned(),
        binding.session_id.to_string(),
        binding.attempt_id.to_string(),
        binding.reservation_id.clone(),
        binding.preparation_sha256.clone(),
    ];
    for text in &printed {
        for needle in &forbidden {
            assert!(!text.contains(needle.as_str()), "{needle} in {text}");
        }
    }
    case.peer.finish_scripts().await;
}

#[tokio::test]
async fn attach_capacity_does_not_consume_the_inspection_request_permits() {
    let case = Case::new().await;
    let held = [
        case.runtime.reserve_attach_capacity().unwrap(),
        case.runtime.reserve_attach_capacity().unwrap(),
    ];
    assert_eq!(case.runtime.attach_slots_available(), 0);
    assert_eq!(
        case.runtime.reserve_attach_capacity().err(),
        Some(KubernetesSessionRuntimeError::AttachCapacity)
    );
    // With every attach slot taken, an inspection is still served, not Busy.
    let observed = case
        .runtime
        .observe_session_instance(&case.instance)
        .await
        .unwrap();
    assert_eq!(
        AttachIdentityV2::from_observation(&observed),
        Some(fixture_identity())
    );
    drop(held);
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
    // A slot that does not come from this runtime is refused by the
    // transport before any attach request, and the runtime's own slots are
    // untouched.
    let foreign = Arc::new(Semaphore::new(1));
    let attempt = AttachAttemptV2::new(AttachCapacity::new(
        foreign.clone().try_acquire_owned().unwrap(),
    ));
    assert_eq!(case.attach(&attempt, SOON).await, Err(REFUSED));
    assert_eq!(case.attach_count(), 0);
    assert_eq!(foreign.available_permits(), 1);
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
}

// ---- review repairs ---------------------------------------------------------

#[tokio::test]
async fn cancellation_raised_at_identity_two_ends_unproven_with_one_stdin_close() {
    let case = Case::new().await;
    // Both observations are equal: without the cancellation this bracket
    // would be proven. The flag is raised when the second Pod GET is served.
    let raised = Arc::new(AtomicBool::new(false));
    case.world.lock().unwrap().raise_on_pod_get = Some((2, raised.clone()));
    let seen = case.expect_attach(Script::ready(whole));
    let attempt = case.new_attempt();
    let result = watched(case.attach_with(&attempt, case.attach_guard(), &raised, SOON)).await;
    assert!(raised.load(Ordering::Acquire));
    assert_eq!(result, Err(UNPROVEN));
    assert_eq!(watched(attempt.join()).await.err(), Some(UNPROVEN));
    let report = attempt.report().unwrap();
    assert!(report.request_initiated());
    assert_eq!(report.cause(), Some(AttachCauseV2::Cancelled));
    assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsWithoutIdentity));
    case.peer.finish_scripts().await;
    let seen = snapshot(&seen);
    assert_exactly_one_hello(&seen.stdin, &case.rendered.binding);
    assert_eq!(seen.stdin_closes, 1);
    assert_eq!(seen.stdin_bytes_after_close, 0);
    assert_eq!(seen.closes, [1000]);
    assert_eq!(case.pod_gets(), 2);
    assert_eq!(case.attach_count(), 1);
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
}

#[tokio::test]
async fn cancellation_while_the_worker_is_silent_closes_the_stream_promptly() {
    let case = Case::new().await;
    // The fixture stream reads the Hello and never answers.
    let seen = case.expect_attach(Script::default());
    let attempt = case.new_attempt();
    let cancelled = AtomicBool::new(false);
    let until = Instant::now() + SOON;
    let attach = case.runtime.attach_exact_instance(
        &case.instance,
        case.rendered.binding.clone(),
        case.attach_guard(),
        &attempt,
        bootstrap_capacity(&case.bootstrap),
        until,
        &cancelled,
    );
    // The flag is raised once the caller waits for a Ready that never comes.
    let raise = async {
        until_true(|| envelope_complete(&seen.lock().unwrap().stdin)).await;
        cancelled.store(true, Ordering::Release);
        Instant::now()
    };
    let (result, raised_at) = watched(async { tokio::join!(attach, raise) }).await;
    let closed_after = raised_at.elapsed();
    // Without the cancellation this attempt would end at `until`, as a
    // deadline. It ended well before, as a cancellation.
    assert!(
        Instant::now() < until,
        "the cancelled attempt lasted until its deadline"
    );
    assert!(
        closed_after < Duration::from_secs(1),
        "closed {closed_after:?} after the cancellation"
    );
    assert_eq!(result, Err(UNPROVEN));
    let report = attempt.report().unwrap();
    assert!(report.request_initiated());
    assert_eq!(report.cause(), Some(AttachCauseV2::Cancelled));
    assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsWithoutIdentity));
    assert_eq!(report.stdout_bytes(), 0);
    case.peer.finish_scripts().await;
    let seen = snapshot(&seen);
    assert_eq!(seen.stdin_closes, 1);
    assert_eq!(seen.closes, [1000]);
    assert_eq!(
        case.pod_gets(),
        1,
        "identity two is not read after a cancellation"
    );
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
}

#[tokio::test]
async fn identity_two_is_asked_again_while_the_local_request_slots_are_taken() {
    let case = Case::new().await;
    // The fixture stream holds its Ready until the test has taken every
    // request slot, so that identity two meets the local contention.
    let gate = Arc::new(AtomicBool::new(false));
    let script = Script {
        after_hello: vec![Step::AwaitFlag(gate.clone()), Step::Reply(whole)],
        ..Script::default()
    };
    let seen = case.expect_attach(script);
    let attempt = case.new_attempt();
    let start = case.peer.requests().len();
    let contend = async {
        until_true(|| envelope_complete(&seen.lock().unwrap().stdin)).await;
        // The caller waits for the Ready and holds no request slot.
        let held = case.runtime.hold_request_slots();
        assert_eq!(held.len(), 4);
        gate.store(true, Ordering::Release);
        // Several retry pauses pass while every slot is taken.
        tokio::time::sleep(Duration::from_millis(200)).await;
        // Identity two was neither sent nor given up.
        let while_held = (case.pod_gets(), attempt.report().is_none());
        drop(held);
        while_held
    };
    let (result, while_held) =
        watched(async { tokio::join!(case.attach(&attempt, SOON), contend) }).await;
    assert_eq!(while_held, (1, true));
    // Once a slot is free the same bracket is proven, inside its deadline.
    assert_eq!(result, Ok(()));
    assert_eq!(
        case.lines_since(start),
        [
            ANCHORS[0].to_owned(),
            ANCHORS[1].to_owned(),
            case.pod_line(),
            case.attach_line(),
            case.pod_line(),
            ANCHORS[0].to_owned(),
            ANCHORS[1].to_owned(),
        ]
    );
    let report = attempt.report().unwrap();
    assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsAfterIdentity));
    assert_eq!(report.cause(), None);
    case.peer.finish_scripts().await;
    assert_eq!(snapshot(&seen).stdin_closes, 1);
    assert_eq!(case.pod_gets(), 2);
}

#[tokio::test]
async fn initiated_upgrade_keeps_capacity_after_caller_loss_until_http_work_ends() {
    let case = Case::new().await;
    let (request_seen, received) = tokio::sync::oneshot::channel();
    case.expect_reply(move |_: &PeerRequest| {
        request_seen
            .send(())
            .expect("the test awaits the received request");
        PeerReply::never_answer()
    });
    let attempt = case.new_attempt();
    let cancelled = AtomicBool::new(false);
    let until = Instant::now() + CONTRACT_WINDOW;
    let mut attach = Box::pin(case.runtime.attach_exact_instance(
        &case.instance,
        case.rendered.binding.clone(),
        case.attach_guard(),
        &attempt,
        bootstrap_capacity(&case.bootstrap),
        until,
        &cancelled,
    ));
    // The upgrade request reached the peer, which never answers it.
    tokio::select! {
        result = &mut attach => panic!("the attach ended early: {result:?}"),
        received = watched(received) => {
            received.expect("the peer observed the real upgrade request");
        }
    }
    assert_eq!(case.attach_count(), 1);
    assert!(attempt.report().is_none());
    assert_eq!(
        case.runtime.attach_slots_available(),
        MAX_CONCURRENT_ATTACH - 1
    );
    println!("S2B_C16_SETUP_UPGRADE_REQUEST_RECEIVED_CAPACITY_HELD");
    drop(attach);
    // Losing the caller is not the effective end of the initiated HTTP work.
    // These synchronous observations precede any further scheduling of it.
    assert!(
        attempt.report().is_none(),
        "S2B_C16_TERMINAL_MUST_WAIT_FOR_HTTP_OWNER"
    );
    assert_eq!(
        case.runtime.attach_slots_available(),
        MAX_CONCURRENT_ATTACH - 1,
        "S2B_C16_CAPACITY_MUST_OUTLIVE_THE_CALLER"
    );
    assert_eq!(watched(attempt.join()).await.err(), Some(UNPROVEN));
    // Observe the actual peer task's end as well: no detached fixture IO.
    watched(case.peer.finish_scripts()).await;
    let report = attempt
        .report()
        .expect("the drained attempt has one terminal record");
    assert!(report.request_initiated());
    assert_eq!(report.end(), None);
    assert_eq!(report.cause(), Some(AttachCauseV2::CallerGone));
    assert_eq!((report.stdout_bytes(), report.stderr_bytes()), (0, 0));
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
    assert_eq!(watched(attempt.join()).await.err(), Some(UNPROVEN));
    assert_eq!(attempt.report(), Some(report));
    // The attempt is spent: its owner is refused before any request.
    let start = case.peer.requests().len();
    assert_eq!(case.attach(&attempt, SOON).await, Err(REFUSED));
    assert!(case.lines_since(start).is_empty());
    assert_eq!(case.attach_count(), 1);
}

#[tokio::test]
async fn post_ready_blocked_pong_write_records_a_typed_io_deadline() {
    let rendered = rendered_case();
    let slots = Arc::new(Semaphore::new(1));
    let attempt = AttachAttemptV2::new(AttachCapacity::new(
        slots.clone().try_acquire_owned().unwrap(),
    ));
    let (client, mut server) = tokio::io::duplex(64);
    let until = Instant::now() + CONTRACT_WINDOW;
    let AttachBracketV2 { ready, verdict } = attempt
        .claim()
        .unwrap()
        .start(
            client,
            fixture_identity(),
            rendered.binding.clone(),
            bootstrap_capacity(&bootstrap_pool()),
            until,
        )
        .unwrap();
    let hello = watched(read_client_frame(&mut server))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hello.opcode(), 0x2);
    let (channel, payload) = hello.payload.split_first().unwrap();
    assert_eq!(*channel, 0);
    assert_exactly_one_hello(payload, &rendered.binding);
    let echo = ready_envelope(payload);
    watched(server.write_all(&whole(&echo))).await.unwrap();
    watched(ready)
        .await
        .expect("the real bootstrap owner accepted Ready");
    watched(server.write_all(&server_frame(0x89, &[0x55; 125])))
        .await
        .unwrap();
    // A masked Pong needs 2 header + 4 mask + 125 payload bytes. Reading
    // only its six-byte prefix leaves 125 bytes for a 64-byte duplex: its
    // write/flush cannot complete while this test keeps the peer undrained.
    let mut prefix = [0_u8; 6];
    watched(server.read_exact(&mut prefix)).await.unwrap();
    assert_eq!(&prefix[..2], &[0x8A, 0xFD]);
    assert!(attempt.report().is_none());
    assert_eq!(slots.available_permits(), 0);
    println!("S2B_C25_SETUP_READY_PONG_PREFIX_RECEIVED_UNDRAINED");
    let result = watched(attempt.join()).await;
    assert_eq!(result.err(), Some(DEADLINE), "S2B_C25_IO_DEADLINE_REQUIRED");
    assert!(
        Instant::now() >= until,
        "a pending write cannot time out before its deadline"
    );
    let report = attempt.report().unwrap();
    assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsWithoutIdentity));
    assert_eq!(report.cause(), None);
    let mut tail = Vec::new();
    watched(server.read_to_end(&mut tail)).await.unwrap();
    assert!(tail.len() <= 64);
    // All remaining bytes belong to the one incomplete masked Pong. A
    // stdin-close/Close appended after the cut would corrupt this prefix.
    for (index, byte) in tail.iter().enumerate() {
        assert_eq!(
            *byte,
            0x55 ^ prefix[2 + index % 4],
            "no frame after a cut Pong"
        );
    }
    assert_eq!(slots.available_permits(), 1);
    assert_eq!(watched(attempt.join()).await.err(), Some(DEADLINE));
    assert_eq!(attempt.report(), Some(report));
    drop(verdict);
}

#[tokio::test]
async fn cancelled_post_ready_blocked_write_keeps_cancellation_with_its_io_deadline() {
    let rendered = rendered_case();
    let slots = Arc::new(Semaphore::new(1));
    let attempt = AttachAttemptV2::new(AttachCapacity::new(
        slots.clone().try_acquire_owned().unwrap(),
    ));
    let (client, mut server) = tokio::io::duplex(64);
    let until = Instant::now() + CONTRACT_WINDOW;
    let AttachBracketV2 { ready, verdict } = attempt
        .claim()
        .unwrap()
        .start(
            client,
            fixture_identity(),
            rendered.binding.clone(),
            bootstrap_capacity(&bootstrap_pool()),
            until,
        )
        .unwrap();
    let hello = watched(read_client_frame(&mut server))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hello.opcode(), 0x2);
    let (channel, payload) = hello.payload.split_first().unwrap();
    assert_eq!(*channel, 0);
    assert_exactly_one_hello(payload, &rendered.binding);
    let echo = ready_envelope(payload);
    watched(server.write_all(&whole(&echo))).await.unwrap();
    watched(ready)
        .await
        .expect("the real bootstrap owner accepted Ready");
    watched(server.write_all(&server_frame(0x89, &[0x55; 125])))
        .await
        .unwrap();
    let mut prefix = [0_u8; 6];
    watched(server.read_exact(&mut prefix)).await.unwrap();
    assert_eq!(&prefix[..2], &[0x8A, 0xFD]);
    assert!(attempt.report().is_none());
    assert_eq!(slots.available_permits(), 0);
    assert!(
        verdict
            .send(BracketVerdictV2::Unproven(AttachCauseV2::Cancelled))
            .is_ok()
    );
    println!("S2B_C25_CANCEL_SETUP_READY_PENDING_WRITE_VERDICT_SENT");
    let result = watched(attempt.join()).await;
    assert_eq!(
        result.err(),
        Some(DEADLINE),
        "S2B_C25_CANCEL_IO_DEADLINE_REQUIRED"
    );
    assert!(
        Instant::now() >= until,
        "a cancellation request is not an observed write deadline"
    );
    let report = attempt.report().unwrap();
    assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsWithoutIdentity));
    assert_eq!(report.cause(), Some(AttachCauseV2::Cancelled));
    let mut tail = Vec::new();
    watched(server.read_to_end(&mut tail)).await.unwrap();
    assert!(tail.len() <= 64);
    for (index, byte) in tail.iter().enumerate() {
        assert_eq!(
            *byte,
            0x55 ^ prefix[2 + index % 4],
            "no frame after a cut Pong"
        );
    }
    assert_eq!(slots.available_permits(), 1);
    assert_eq!(watched(attempt.join()).await.err(), Some(DEADLINE));
    assert_eq!(attempt.report(), Some(report));
}

#[tokio::test]
async fn invalid_close_payloads_are_refused_by_the_real_stream_reader() {
    let rendered = rendered_case();
    // Literal RFC-invalid wire bodies, independent of the decoder under test.
    let cases = [
        ("one_byte", vec![0x00]),
        ("reserved_1005", vec![0x03, 0xED]),
        ("reserved_1006", vec![0x03, 0xEE]),
        ("reserved_1015", vec![0x03, 0xF7]),
        ("invalid_utf8_reason", vec![0x03, 0xE8, 0xFF]),
    ];
    for (case, payload) in cases {
        let concluded = play(
            &rendered.binding,
            Script::first(server_frame(0x88, &payload)),
            CONTRACT_WINDOW,
            None,
        )
        .await;
        println!("S2B_CLOSE_SETUP_REAL_STREAM_ENDED case={case}");
        assert_eq!(
            concluded.result.as_ref().err(),
            Some(&PROTOCOL),
            "S2B_CLOSE_PROTOCOL_REQUIRED case={case}"
        );
        assert_eq!(
            concluded.report.end(),
            Some(&AttachEndV2::ClosedByUsWithoutIdentity)
        );
        assert_eq!(concluded.report.stdout_bytes(), 0);
        assert_eq!(concluded.report.cause(), None);
        assert!(!concluded.ready_accepted);
        assert!(concluded.seen.ended);
        assert_eq!(concluded.seen.stdin_bytes_after_close, 0);
        assert!(concluded.seen.stdin_closes <= 1);
        // A protocol failure may drop the transport rather than deliver a
        // close frame; when delivered, the only permitted close is 1002.
        assert!(concluded.seen.closes.len() <= 1);
        assert!(concluded.seen.closes.iter().all(|code| *code == 1002));
    }
}

#[test]
fn runtime_stop_while_a_stream_is_held_is_recorded_as_owner_stopped() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (wire, _server) = runtime.block_on(async {
        let rendered = rendered_case();
        let (wire, mut server) = open(&rendered.binding, SOON, 64 * 1024);
        // The owner task runs and writes its Hello: the stream is in use.
        let frame = watched(read_client_frame(&mut server))
            .await
            .unwrap()
            .expect("the owner wrote a frame");
        assert_eq!((frame.opcode(), frame.payload.first()), (0x2, Some(&0)));
        (wire, server)
    });
    assert!(wire.attempt.report().is_none());
    assert_eq!(wire.slots.available_permits(), 0);
    // The runtime stops while its task still owns the stream.
    drop(runtime);
    let report = wire
        .attempt
        .report()
        .expect("the stopped owner recorded its end");
    // A stream was held: the record differs from a refused handshake, whose
    // end is `None`.
    assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsWithoutIdentity));
    assert_eq!(report.cause(), Some(AttachCauseV2::OwnerStopped));
    assert_eq!(wire.slots.available_permits(), 1);
    // Another runtime reads the same record at once, and never a success.
    let other = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    assert_eq!(other.block_on(wire.attempt.join()).err(), Some(UNPROVEN));
}

#[test]
fn owner_that_cannot_be_started_is_a_local_failure_not_a_close_by_the_peer() {
    // No runtime is entered here, so the owner task cannot be started. The
    // stream is dropped unused: nothing was written and the peer did nothing.
    let rendered = rendered_case();
    let slots = Arc::new(Semaphore::new(1));
    let attempt = AttachAttemptV2::new(AttachCapacity::new(
        slots.clone().try_acquire_owned().unwrap(),
    ));
    let (client, mut server) = tokio::io::duplex(1024);
    let claim = attempt.claim().unwrap();
    let started = claim.start(
        client,
        fixture_identity(),
        rendered.binding.clone(),
        bootstrap_capacity(&bootstrap_pool()),
        Instant::now() + SOON,
    );
    assert_eq!(started.err(), Some(UNPROVEN));
    // The record exists at once, on its own cause: closed by this side, not
    // by the peer, and never a refused handshake, whose end is `None`.
    let report = attempt
        .report()
        .expect("the failed start is recorded at once");
    assert!(!report.request_initiated());
    assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsWithoutIdentity));
    assert_eq!(report.cause(), Some(AttachCauseV2::LocalFailure));
    assert_eq!((report.stdout_bytes(), report.stderr_bytes()), (0, 0));
    assert_eq!(slots.available_permits(), 1);
    // The peer reads the end of the stream and not one byte before it, and a
    // later join returns the same record.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (received, joined) = runtime.block_on(async {
        let mut received = Vec::new();
        watched(server.read_to_end(&mut received)).await.unwrap();
        (received, watched(attempt.join()).await)
    });
    assert!(received.is_empty());
    assert_eq!(joined.err(), Some(UNPROVEN));
    assert_eq!(attempt.report(), Some(report));
}

// ---- owner from the initiation and peer close ---------------------------------

#[tokio::test]
async fn caller_lost_before_a_refused_upgrade_answer_ends_when_that_answer_arrives() {
    let case = Case::new().await;
    let held = case.expect_held_upgrade(HeldAnswer::Forbidden);
    let attempt = case.new_attempt();
    let until = Instant::now() + CONTRACT_WINDOW;
    case.lose_caller_before_the_answer(&attempt, held.received, until)
        .await;
    // The refusal arrives now, and nobody waits for it.
    held.release
        .send(())
        .expect("the peer script still holds the request");
    assert_eq!(watched(attempt.join()).await.err(), Some(UNPROVEN));
    // The initiated HTTP work ended with its answer, not with the deadline:
    // the slot is not kept beyond the effective end.
    assert!(
        Instant::now() < until,
        "the fixture was too slow: the attempt may have ended at its deadline"
    );
    watched(case.peer.finish_scripts()).await;
    let report = attempt
        .report()
        .expect("the ended attempt has one terminal record");
    // The lost caller is the recorded cause, not the refused handshake that
    // nobody was there to read. No stream was ever held.
    assert!(report.request_initiated());
    assert_eq!(report.end(), None);
    assert_eq!(report.cause(), Some(AttachCauseV2::CallerGone));
    assert_eq!((report.stdout_bytes(), report.stderr_bytes()), (0, 0));
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
    assert_eq!(watched(attempt.join()).await.err(), Some(UNPROVEN));
    assert_eq!(attempt.report(), Some(report));
    assert_eq!(case.attach_count(), 1);
    // The attempt is spent: its owner is refused before any request.
    let start = case.peer.requests().len();
    assert_eq!(case.attach(&attempt, SOON).await, Err(REFUSED));
    assert!(case.lines_since(start).is_empty());
}

#[tokio::test]
async fn upgrade_answered_after_the_caller_was_lost_is_owned_until_its_stream_is_closed() {
    let case = Case::new().await;
    let held = case.expect_held_upgrade(HeldAnswer::Switching);
    let attempt = case.new_attempt();
    let until = Instant::now() + CONTRACT_WINDOW;
    case.lose_caller_before_the_answer(&attempt, held.received, until)
        .await;
    // The exact 101 arrives now: a stream exists that no caller waits for.
    held.release
        .send(())
        .expect("the peer script still holds the request");
    // The owner holds that stream like any other, to its deadline: nobody
    // is left to read identity two, so it can only end without identity.
    assert_eq!(watched(attempt.join()).await.err(), Some(DEADLINE));
    assert!(
        Instant::now() >= until,
        "a stream without a caller is kept until the deadline"
    );
    watched(case.peer.finish_scripts()).await;
    let report = attempt
        .report()
        .expect("the ended attempt has one terminal record");
    assert!(report.request_initiated());
    assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsWithoutIdentity));
    assert!(matches!(
        report.cause(),
        Some(AttachCauseV2::CallerGone | AttachCauseV2::Bootstrap(BootstrapError::Deadline))
    ));
    assert_eq!(report.stdout_bytes(), 0);
    // Only the Hello travelled, then the stdin close and the close frame.
    let seen = snapshot(&held.seen);
    assert_exactly_one_hello(&seen.stdin, &case.rendered.binding);
    assert_eq!(seen.stdin_closes, 1);
    assert_eq!(seen.stdin_bytes_after_close, 0);
    assert_eq!(seen.closes, [1000]);
    assert_eq!(seen.unexpected, 0);
    assert!(seen.ended);
    assert_eq!(
        case.pod_gets(),
        1,
        "identity two is not read without a caller"
    );
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
    assert_eq!(watched(attempt.join()).await.err(), Some(DEADLINE));
    assert_eq!(attempt.report(), Some(report));
    assert_eq!(case.attach_count(), 1);
}

#[test]
fn runtime_stop_while_the_upgrade_is_unanswered_is_owner_stopped_without_a_stream() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // The peer reads the upgrade request and is never released. The caller
    // is lost, then the runtime stops while the owner still awaits the answer.
    let (case, attempt, release) = runtime.block_on(async {
        let case = Case::new().await;
        let held = case.expect_held_upgrade(HeldAnswer::Forbidden);
        let attempt = case.new_attempt();
        let until = Instant::now() + CONTRACT_WINDOW;
        case.lose_caller_before_the_answer(&attempt, held.received, until)
            .await;
        (case, attempt, held.release)
    });
    assert!(attempt.report().is_none());
    drop(runtime);
    let report = attempt
        .report()
        .expect("the stopped owner recorded its end");
    // The request was initiated and no stream was ever held: the record
    // differs from a stream dropped with its runtime, whose end is `Some`.
    assert!(report.request_initiated());
    assert_eq!(report.end(), None);
    assert_eq!(report.cause(), Some(AttachCauseV2::OwnerStopped));
    assert_eq!((report.stdout_bytes(), report.stderr_bytes()), (0, 0));
    assert_eq!(case.runtime.attach_slots_available(), MAX_CONCURRENT_ATTACH);
    // Another runtime reads the same record at once, and never a success.
    let other = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    assert_eq!(other.block_on(attempt.join()).err(), Some(UNPROVEN));
    // The answer was never released.
    drop(release);
}

/// An opening that is never ready, in place of the initiated request. When
/// it is dropped, it notes what the attempt looked like at that instant: the
/// free permits of the attach pool, and whether a record already existed.
struct ProbeOpening {
    slots: Arc<Semaphore>,
    attempt: AttachAttemptV2,
    at_drop: Arc<Mutex<Vec<(usize, bool)>>>,
}
impl std::future::Future for ProbeOpening {
    type Output = Outcome<DuplexStream>;
    fn poll(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::task::Poll::Pending
    }
}
impl Drop for ProbeOpening {
    fn drop(&mut self) {
        let free = self.slots.available_permits();
        let recorded = self.attempt.report().is_some();
        self.at_drop.lock().unwrap().push((free, recorded));
    }
}

#[test]
fn owner_task_that_never_ran_drops_its_request_before_the_slot_and_the_record() {
    // The end of an owner task that never ran: its runtime stopped before
    // the task was spawned on the handle, so the task is dropped at once,
    // without one poll. This holds the order "request dropped, then slot
    // returned, then record" for that task: when the probe that stands for
    // the initiated request is dropped, the slot is still held and no record
    // exists; both follow before the call returns.
    let rendered = rendered_case();
    let slots = Arc::new(Semaphore::new(1));
    let attempt = AttachAttemptV2::new(AttachCapacity::new(
        slots.clone().try_acquire_owned().unwrap(),
    ));
    let claim = attempt.claim().unwrap();
    // The handle of a runtime that has already stopped.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let handle = runtime.handle().clone();
    drop(runtime);
    let at_drop = Arc::new(Mutex::new(Vec::new()));
    let opening = ProbeOpening {
        slots: slots.clone(),
        attempt: attempt.clone(),
        at_drop: at_drop.clone(),
    };
    let inputs = StreamInputs {
        identity: fixture_identity(),
        binding: rendered.binding.clone(),
        bootstrap: bootstrap_capacity(&bootstrap_pool()),
        until: Instant::now() + SOON,
    };
    assert_eq!(slots.available_permits(), 0);
    assert!(attempt.report().is_none());
    assert!(at_drop.lock().unwrap().is_empty());
    let bracket = claim.start_opening(&handle, opening, inputs);
    // The probe was dropped exactly once, inside that call, while the slot
    // was still held and before any record.
    assert_eq!(*at_drop.lock().unwrap(), [(0, false)]);
    // Then the slot came back and the end of the owner was recorded: no
    // stream was ever held, and nothing was counted.
    assert_eq!(slots.available_permits(), 1);
    let report = attempt
        .report()
        .expect("the dropped owner recorded its end");
    assert_eq!(report.end(), None);
    assert_eq!(report.cause(), Some(AttachCauseV2::OwnerStopped));
    assert_eq!((report.stdout_bytes(), report.stderr_bytes()), (0, 0));
    // A join answers at its first poll, with that record and never a
    // success.
    let other = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let first_poll = other.block_on(async {
        let joining = attempt.join();
        tokio::pin!(joining);
        std::future::poll_fn(|cx| {
            std::task::Poll::Ready(std::future::Future::poll(joining.as_mut(), cx))
        })
        .await
    });
    let std::task::Poll::Ready(joined) = first_poll else {
        panic!("the join of a recorded attempt waited");
    };
    assert_eq!(joined.err(), Some(UNPROVEN));
    assert_eq!(attempt.report(), Some(report));
    // The caller's half of the bracket was kept until here, so that nothing
    // above comes from its drop. Dropping it returns no second slot.
    drop(bracket);
    assert_eq!(slots.available_permits(), 1);
}

#[tokio::test]
async fn peer_close_is_a_peer_end_only_without_a_code_or_with_1000_or_1001() {
    fn close(body: &[u8]) -> Script {
        Script {
            after_hello: vec![Step::Raw(server_frame(0x88, body))],
            ..Script::default()
        }
    }
    let rendered = rendered_case();
    // Accepted as an end by the peer: no body, 1000 and 1001, with or
    // without a reason. Each row is the literal close body, then the codes
    // and the empty close frames that the fixture must read back.
    let mut with_reason = 1000_u16.to_be_bytes().to_vec();
    with_reason.extend_from_slice(REASON_MARKER.as_bytes());
    let accepted: [(Vec<u8>, Vec<u16>, usize); 4] = [
        (Vec::new(), Vec::new(), 1),
        (vec![0x03, 0xE8], vec![1000], 0),
        (vec![0x03, 0xE9], vec![1001], 0),
        (with_reason, vec![1000], 0),
    ];
    for (body, closes, empty_closes) in accepted {
        let concluded = play(&rendered.binding, close(&body), SOON, None).await;
        // An end by the peer is never an identity.
        assert_eq!(concluded.result.as_ref().err(), Some(&UNPROVEN));
        assert_eq!(concluded.report.end(), Some(&AttachEndV2::PeerClosed));
        assert_eq!(concluded.report.cause(), None);
        assert_eq!(concluded.report.stdout_bytes(), 0);
        assert!(!concluded.ready_accepted);
        // No data frame can follow a close by the peer: the v5 stdin close
        // is not written, and the only close frame is the reply, which
        // echoes the peer's own code.
        assert_exactly_one_hello(&concluded.seen.stdin, &rendered.binding);
        assert_eq!(concluded.seen.stdin_closes, 0);
        assert_eq!(concluded.seen.closes, closes);
        assert_eq!(concluded.seen.empty_closes, empty_closes);
        assert_eq!(concluded.seen.unexpected, 0);
        assert!(concluded.seen.ended);
        // The reason chosen by the peer is not kept.
        for printed in [
            format!("{:?}", concluded.result),
            format!("{:?}", concluded.report),
            format!("{:?}", concluded.attempt),
        ] {
            assert!(!printed.contains(REASON_MARKER), "{printed}");
        }
    }
    // Any other code is refused, although RFC 6455 allows it on the wire:
    // protocol error, unsupported data, message too big, server error, and
    // the registered and private ranges. The reply still echoes the code.
    for code in [1002_u16, 1003, 1009, 1011, 3000, 4000] {
        let script = close(&code.to_be_bytes());
        let concluded = play(&rendered.binding, script, SOON, None).await;
        assert_eq!(
            concluded.result.as_ref().err(),
            Some(&PROTOCOL),
            "close code {code}"
        );
        assert_eq!(
            concluded.report.end(),
            Some(&AttachEndV2::ClosedByUsWithoutIdentity)
        );
        assert_eq!(concluded.report.cause(), None);
        assert_eq!(concluded.report.stdout_bytes(), 0);
        assert!(!concluded.ready_accepted);
        assert_eq!(concluded.seen.stdin_closes, 0);
        assert_eq!(concluded.seen.closes, [code], "close code {code}");
        assert_eq!(concluded.seen.empty_closes, 0);
        assert_eq!(concluded.seen.unexpected, 0);
        assert!(concluded.seen.ended);
    }
}

#[tokio::test]
async fn cancelled_blocked_hello_write_keeps_cancellation_with_its_io_deadline() {
    let rendered = rendered_case();
    let started = Instant::now();
    let (wire, mut server) = open(&rendered.binding, CONTRACT_WINDOW, 64);
    let Wire {
        attempt,
        bracket,
        slots,
    } = wire;
    let AttachBracketV2 { ready, verdict } = bracket;
    // The peer reads the first four bytes of the Hello frame and no more:
    // a final masked binary frame in the 16-bit length form. Its key and its
    // payload cannot fit the 64-byte pipe, so its write stays pending.
    let mut head = [0_u8; 4];
    watched(server.read_exact(&mut head)).await.unwrap();
    assert_eq!(head[..2], [0x82, 0x80 | 126]);
    let length = usize::from(u16::from_be_bytes([head[2], head[3]]));
    assert!(4 + length > 64, "the Hello frame must not fit the pipe");
    assert!(attempt.report().is_none());
    assert_eq!(slots.available_permits(), 0);
    // The caller asks for a cancellation while that write is pending.
    assert!(
        verdict
            .send(BracketVerdictV2::Unproven(AttachCauseV2::Cancelled))
            .is_ok()
    );
    // The observed end is the deadline, not a prompt cancellation; the
    // cancellation that was requested is kept as its cause.
    assert_eq!(watched(attempt.join()).await.err(), Some(DEADLINE));
    assert!(
        started.elapsed() >= CONTRACT_WINDOW,
        "a cancellation request is not an observed write deadline"
    );
    let report = attempt.report().unwrap();
    assert_eq!(report.end(), Some(&AttachEndV2::ClosedByUsWithoutIdentity));
    assert_eq!(report.cause(), Some(AttachCauseV2::Cancelled));
    assert_eq!(report.stdout_bytes(), 0);
    assert!(watched(ready).await.is_err());
    // What the pipe still held is the rest of that one frame and nothing
    // else: its key, then the stdin channel byte and the start of the one
    // envelope that the frame announces. Then the stream ends.
    let mut tail = Vec::new();
    watched(server.read_to_end(&mut tail)).await.unwrap();
    assert!((10..=64).contains(&tail.len()), "{}", tail.len());
    let (key, masked) = tail.split_at(4);
    let payload: Vec<u8> = masked
        .iter()
        .zip(key.iter().cycle())
        .map(|(byte, mask)| byte ^ mask)
        .collect();
    assert_eq!(payload[0], 0);
    let declared = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
    assert_eq!(1 + 4 + declared as usize, length);
    assert_eq!(payload[5], b'{');
    assert_eq!(slots.available_permits(), 1);
    assert_eq!(watched(attempt.join()).await.err(), Some(DEADLINE));
    assert_eq!(attempt.report(), Some(report));
}

/// The next client frame, within the test watchdog.
async fn next_frame(server: &mut DuplexStream) -> Option<ClientFrame> {
    watched(read_client_frame(server)).await.unwrap()
}

#[tokio::test]
async fn frame_that_cannot_be_built_locally_is_closed_by_us_not_by_the_peer() {
    // The write queue of the codec holds one client frame. The largest
    // message this client builds, one whole stdin chunk behind its channel
    // byte, fits it and leaves in one masked frame.
    let (client, mut server) = tokio::io::duplex(64 * 1024);
    let mut adapter = codec::AttachCodec::new(client).await;
    let until = Instant::now() + SOON;
    let largest = vec![0_u8; STDIN_PREFIX.len() + STDIN_CHUNK_BYTES];
    assert!(adapter.send(largest, until).await.is_ok());
    let frame = next_frame(&mut server).await.unwrap();
    assert_eq!(frame.opcode(), 0x2);
    assert_eq!(frame.payload.len(), STDIN_PREFIX.len() + STDIN_CHUNK_BYTES);
    // A message above that bound cannot be queued: the library refuses it
    // before any byte of it is written.
    let oversized = vec![0_u8; MAX_WRITE_QUEUE_BYTES + 1];
    assert!(matches!(
        adapter.send(oversized.clone(), until).await,
        Err(codec::WriteFailure::Local)
    ));
    // The stream is still on a frame boundary: the next frame leaves whole,
    // and the bounded close sends the stdin close and the close frame.
    assert!(adapter.send(vec![0, b'o', b'k'], until).await.is_ok());
    let frame = next_frame(&mut server).await.unwrap();
    assert_eq!(frame.payload, [0, b'o', b'k']);
    adapter.close(CLOSE_NORMAL).await;
    let stdin_close = next_frame(&mut server).await.unwrap();
    assert_eq!(stdin_close.opcode(), 0x2);
    assert_eq!(stdin_close.payload, [255, 0]);
    let close = next_frame(&mut server).await.unwrap();
    assert_eq!(close.opcode(), 0x8);
    assert_eq!(close.payload, 1000_u16.to_be_bytes());
    assert!(next_frame(&mut server).await.is_none());

    // The owner ends on its own cause, not on a close by the peer, and its
    // bounded close still sends the stdin close and the close frame.
    let (client, mut server) = tokio::io::duplex(1024);
    let (_bootstrap_side, attach_side) = tokio::io::duplex(PIPE_BYTES);
    let (stdin, stdout) = tokio::io::split(attach_side);
    let adapter = codec::AttachCodec::new(client).await;
    let mut stream = OwnedStream::new(adapter, stdin, stdout, until);
    let (_caller, mut verdict) = tokio::sync::oneshot::channel();
    let ending = stream.send_stdin(&oversized, &mut verdict).await;
    assert!(matches!(
        ending,
        Err(StreamEnding::Refused {
            error: KubernetesSessionRuntimeError::AttachIdentityUnproven,
            cause: Some(AttachCauseV2::LocalFailure),
            close_code: CLOSE_NORMAL,
        })
    ));
    stream.close(CLOSE_NORMAL).await;
    let stdin_close = next_frame(&mut server).await.unwrap();
    assert_eq!(stdin_close.opcode(), 0x2);
    assert_eq!(stdin_close.payload, [255, 0]);
    let close = next_frame(&mut server).await.unwrap();
    assert_eq!(close.opcode(), 0x8);
    assert_eq!(close.payload, 1000_u16.to_be_bytes());
    assert!(next_frame(&mut server).await.is_none());
}
