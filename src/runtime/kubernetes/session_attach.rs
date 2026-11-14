//! Preamble-only attach of one exact V2 session instance (stage S2b).
//!
//! The attach carries one bootstrap Hello and its Ready echo, and nothing
//! else (Root decision Q1). Two observations of the same instance bracket
//! that exchange. The bracket is neither an atomic precondition nor an
//! identity proven by the echo: it rests on the stated assumptions of the
//! design (a UID is never reused, the API server and the kubelet are honest,
//! an established stream stays bound to its container). No argv, workspace,
//! credential or secret travels before or after the preamble. Every attempt
//! that held a stream ends its stdin: by the v5 stdin close while the stream
//! still takes a frame, and by dropping the connection after a well-formed
//! close frame of the peer, an ended transport or a cut frame. A close frame
//! of the peer that the codec library refuses as malformed (a one-byte body,
//! a reason that is not UTF-8) leaves the stream able to take frames, so the
//! stdin close is written after it. With `stdinOnce` every attempt whose
//! upgrade was initiated is final for its Pod: there is no second connection
//! and no re-attach. An attempt refused before the initiation did not touch
//! the Pod. The observed end of the stream is not a terminal proof of the
//! Pod. The effect guard stays sealed without a production implementor, so
//! no product binary reaches this code.
//!
//! The stream side speaks the `v5.channel.k8s.io` subprotocol only, on the
//! stream that the retained HTTPS client upgraded and that this module
//! validated. WebSocket framing is done by a maintained library behind the
//! closed adapter of the `codec` module, with every bound set from the
//! constants below; that module states its memory and logging limits. From
//! the server this module accepts binary messages on the stdout, stderr and
//! status channels, ping, pong and close, and nothing else: no text message,
//! no other channel, no extension, no other subprotocol and no SPDY or v4
//! fallback. It is not a general WebSocket client and must not be used as
//! one.
//!
//! From the initiation of the upgrade request on, one owner task per attempt
//! holds the initiated request, then its upgraded IO, with the attach
//! capacity and the bootstrap owner, until the effective end; it then
//! publishes one record. Cancelling or dropping a waiter neither stops that
//! task nor returns the capacity, and it makes no end. Before the initiation
//! the owner is the caller's future (see `AttachAttemptV2`). The Hello and
//! Ready frames stay the bootstrap envelope carried by `BootstrapIoOwner`;
//! this module adds no second codec for them.
//!
//! `allow`, not `expect`: `attach_exact_instance` has no caller outside
//! tests, so an `expect(dead_code)` would be fulfilled if rustc reports an
//! uncalled restricted method, which is the reading this module relies on.
//! That reading was not verified with a compiler here, and an unfulfilled
//! expectation fails `clippy -D warnings`, so the module follows
//! `session_transport.rs` and stays correct under both readings.
#![cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "V2 preamble-only attach without a production caller or guard (Root decision Q1; DESIGN section 3.6)"
    )
)]
use super::{
    KubernetesError, KubernetesRuntime,
    bootstrap::{
        self, BootstrapCapacity, BootstrapError, BootstrapIoOwner, BootstrapObservationV2,
    },
    session::SessionBootstrapBindingV2,
    session_effect::SessionEffectGuardV2,
    session_instance::{
        KubernetesSessionRuntimeError, SessionContainerStateV2, SessionInstanceV2,
        SessionObservationStateV2, SessionObservedV2, parse_session_observation,
    },
    session_pod::session_pod_name,
    valid_dns_label,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, SecondsFormat, Utc};
use reqwest::header::{
    CONNECTION, HeaderMap, HeaderName, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_EXTENSIONS,
    SEC_WEBSOCKET_PROTOCOL, UPGRADE,
};
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};
use serde_json::{Value, json};
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf},
    runtime::Handle,
    sync::{OwnedSemaphorePermit, Semaphore, oneshot, watch},
    task::JoinHandle,
    time::{Instant, timeout_at},
};

#[path = "session_attach_codec.rs"]
mod codec;
use codec::{AttachCodec, ServerItem, WriteFailure};

type Outcome<T> = Result<T, KubernetesSessionRuntimeError>;
const REFUSED: KubernetesSessionRuntimeError =
    KubernetesSessionRuntimeError::EffectRefusedBeforeSend;
const UNPROVEN: KubernetesSessionRuntimeError =
    KubernetesSessionRuntimeError::AttachIdentityUnproven;
const PROTOCOL: KubernetesSessionRuntimeError = KubernetesSessionRuntimeError::AttachProtocol;
const DEADLINE: KubernetesSessionRuntimeError = KubernetesSessionRuntimeError::AttachDeadline;

/// The only subprotocol offered and accepted (remotecommand v5).
pub(crate) const ATTACH_SUBPROTOCOL: &str = "v5.channel.k8s.io";
/// RFC 6455 section 1.3.
const WEBSOCKET_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

// The bounds below are software ceilings chosen for a preamble. None of them
// is a measured capacity.
/// One reassembled server message, channel byte included, and the payload of
/// one server frame.
const MAX_WS_MESSAGE_BYTES: usize = 64 * 1024;
/// Stderr bytes counted over one attempt before it is refused.
const MAX_STDERR_BYTES: u64 = 16 * 1024;
/// One channel 3 `v1.Status` document.
const MAX_STATUS_BYTES: usize = 4 * 1024;
/// Upper bound of the HTTP upgrade, in addition to the client's own timeout.
pub(in crate::runtime::kubernetes) const ATTACH_HANDSHAKE: Duration = Duration::from_secs(5);
/// Upper bound of the caller deadline of one bracket.
const ATTACH_BRACKET_MAX: Duration = Duration::from_secs(15);
/// Concurrent preamble attempts of one runtime. Each lasts at most
/// `ATTACH_BRACKET_MAX` plus its bounded close, so this bounds a rate of
/// attempts. It is not a number of live sessions.
pub(in crate::runtime::kubernetes) const MAX_CONCURRENT_ATTACH: usize = 2;
/// Bound of each write of the final close, after any deadline.
const CLOSE_WRITE_LIMIT: Duration = Duration::from_secs(1);
/// Period at which a wait of the bracket reads the caller's cancellation
/// flag. The flag is an `AtomicBool` and wakes nobody, so it is polled.
const CANCEL_POLL: Duration = Duration::from_millis(50);
/// Pause before identity two is asked again after `Busy`. `Busy` is the
/// refusal of the local request slots: no request was sent.
const IDENTITY_TWO_RETRY_PAUSE: Duration = Duration::from_millis(25);

/// Two bytes, then at most eight bytes of extended length; a server frame
/// carries no masking key.
const MAX_SERVER_HEADER_BYTES: usize = 10;
/// A client frame header: the server form and the four-byte masking key.
const MAX_CLIENT_HEADER_BYTES: usize = MAX_SERVER_HEADER_BYTES + 4;
/// Payload bound of one control frame (RFC 6455 section 5.5).
const MAX_CONTROL_BYTES: usize = 125;
/// The most that one read of the codec asks of the transport, and the size
/// at which its read buffer starts. It is not the size of that buffer, which
/// is also the frame buffer and is grown to the declared length of each
/// admitted frame (see the memory note of the `codec` module).
const READ_CHUNK_BYTES: usize = 4096;
const STDIN_CHUNK_BYTES: usize = 4096;
/// One whole bootstrap envelope: neither side of the pipe ever waits for the
/// other inside the single frame that each direction carries.
const PIPE_BYTES: usize = bootstrap::MAX_FRAME_BYTES + 4;

const CHANNEL_STDIN: u8 = 0;
const CHANNEL_STDOUT: u8 = 1;
const CHANNEL_STDERR: u8 = 2;
const CHANNEL_STATUS: u8 = 3;
/// v5 close signal: one binary message `[255, stream id]`.
const CHANNEL_CLOSE: u8 = 255;
const CLOSE_NORMAL: u16 = 1000;
const CLOSE_PROTOCOL: u16 = 1002;
const CLOSE_TOO_BIG: u16 = 1009;
const NO_BYTES: &[u8] = &[];
/// Channel byte that opens every stdin message.
const STDIN_PREFIX: &[u8] = &[CHANNEL_STDIN];
/// The v5 close of stdin, sent once at the end of every owned stream that
/// still takes a frame.
const STDIN_CLOSE: &[u8] = &[CHANNEL_CLOSE, CHANNEL_STDIN];

/// The codec hands each frame to the transport as soon as it is queued and
/// batches nothing.
const WRITE_BUFFER_BYTES: usize = 0;
/// Write queue of the codec: one client frame, at most one stdin chunk with
/// its channel byte. Every write is flushed, or ends the stream, before the
/// next one, so the queue never holds two frames.
const MAX_WRITE_QUEUE_BYTES: usize =
    MAX_CLIENT_HEADER_BYTES + STDIN_PREFIX.len() + STDIN_CHUNK_BYTES;
// The codec library panics on a queue bound that does not exceed its write
// buffer.
const _: () = assert!(MAX_WRITE_QUEUE_BYTES > WRITE_BUFFER_BYTES);
// A pong or a close reply queued by the library fits the queue.
const _: () = assert!(MAX_CLIENT_HEADER_BYTES + MAX_CONTROL_BYTES <= MAX_WRITE_QUEUE_BYTES);

/// Fixed attach route of one Pod. The caller supplies no path, query, header
/// or container name.
pub(in crate::runtime::kubernetes) fn attach_path(namespace: &str, pod_name: &str) -> String {
    format!(
        "/api/v1/namespaces/{namespace}/pods/{pod_name}/attach?container=worker&stdin=true&stdout=true&stderr=true&tty=false"
    )
}

/// Fresh `Sec-WebSocket-Key`: 16 random bytes in base64.
pub(in crate::runtime::kubernetes) fn websocket_key() -> Outcome<String> {
    let mut nonce = [0_u8; 16];
    SystemRandom::new().fill(&mut nonce).map_err(|_| REFUSED)?;
    Ok(STANDARD.encode(nonce))
}

/// `Sec-WebSocket-Accept` expected for `key` (RFC 6455 section 4.2.2). SHA-1
/// is the handshake checksum of the protocol, never a security property here.
pub(in crate::runtime::kubernetes) fn websocket_accept(key: &str) -> String {
    let mut context = digest::Context::new(&digest::SHA1_FOR_LEGACY_USE_ONLY);
    context.update(key.as_bytes());
    context.update(WEBSOCKET_GUID);
    STANDARD.encode(context.finish())
}

/// The value of a header that is present exactly once.
fn single_value(headers: &HeaderMap, name: HeaderName) -> Option<&str> {
    let mut values = headers.get_all(name).iter();
    match (values.next(), values.next()) {
        (Some(value), None) => value.to_str().ok(),
        _ => None,
    }
}

/// Closed acceptance of the upgrade answer, without any socket.
///
/// Accepted only: status exactly 101, one `Upgrade` equal to `websocket`, a
/// `Connection` header containing the token `upgrade`, the exact accept value
/// of `key`, exactly one `Sec-WebSocket-Protocol` value equal to the offered
/// one, and no `Sec-WebSocket-Extensions`. Anything else is refused.
pub(in crate::runtime::kubernetes) fn validate_attach_upgrade(
    status: u16,
    headers: &HeaderMap,
    key: &str,
) -> Outcome<()> {
    let upgrade =
        single_value(headers, UPGRADE).is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    let connection = headers.get_all(CONNECTION).iter().any(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        })
    });
    let expected = websocket_accept(key);
    let accept = single_value(headers, SEC_WEBSOCKET_ACCEPT) == Some(expected.as_str());
    let protocol = single_value(headers, SEC_WEBSOCKET_PROTOCOL) == Some(ATTACH_SUBPROTOCOL);
    if status != 101
        || !upgrade
        || !connection
        || !accept
        || !protocol
        || headers.contains_key(SEC_WEBSOCKET_EXTENSIONS)
    {
        return Err(KubernetesSessionRuntimeError::AttachHandshakeRefused);
    }
    Ok(())
}

/// Why a server item, message or channel was refused. No byte is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Violation {
    Protocol,
    TooBig,
}

/// Stdout is the single bootstrap envelope of one Ready and nothing else: R4
/// is not delivered, so any byte beyond that envelope is refused.
struct StdoutEnvelope {
    prefix: [u8; 4],
    seen: usize,
    total: Option<usize>,
}
impl StdoutEnvelope {
    fn new() -> Self {
        Self {
            prefix: [0; 4],
            seen: 0,
            total: None,
        }
    }

    /// Admits `payload` only if it stays inside the one declared envelope.
    /// A refused payload changes nothing, so no byte of it is delivered.
    fn admit(&mut self, payload: &[u8]) -> Result<(), Violation> {
        let mut prefix = self.prefix;
        let mut seen = self.seen;
        let mut total = self.total;
        let mut rest = payload;
        if total.is_none() {
            let take = prefix.len().saturating_sub(seen).min(rest.len());
            let (head, tail) = rest.split_at(take);
            prefix
                .get_mut(seen..seen + take)
                .ok_or(Violation::Protocol)?
                .copy_from_slice(head);
            seen += take;
            rest = tail;
            if seen == prefix.len() {
                // The single definition of the envelope bound.
                let declared =
                    bootstrap::declared_frame_length(prefix).map_err(|_| Violation::Protocol)?;
                total = Some(prefix.len() + declared);
            }
        }
        if let Some(total) = total {
            if rest.len() > total.saturating_sub(seen) {
                return Err(Violation::Protocol);
            }
            seen += rest.len();
        }
        *self = Self {
            prefix,
            seen,
            total,
        };
        Ok(())
    }
}

/// How one attach stream ended. A declared fact of the stream, never a
/// terminal proof of the Pod or of its container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::runtime::kubernetes) enum AttachEndV2 {
    /// Both observations matched; stdin and the stream were then closed.
    ClosedByUsAfterIdentity,
    /// Closed by this side without a proven bracket, also when the peer sent
    /// a close frame that this client refuses. The stdin close and the close
    /// frame are not written after a cut frame (`AttachDeadline` from a
    /// pending write), nor after a well-formed close frame of the peer,
    /// where the reply of the codec library is the single frame written.
    /// After a close frame that the library refuses as malformed (a
    /// one-byte body, a reason that is not UTF-8) the stream still takes
    /// frames: the stdin close and a close frame 1002 are written. With the
    /// cause `OwnerStopped` or `LocalFailure` the stream may have been
    /// dropped without either.
    ClosedByUsWithoutIdentity,
    /// End of stream, transport failure, or a close frame of the peer with
    /// no code or with the code 1000 or 1001. No stdin close is written
    /// after any of them.
    PeerClosed,
    /// Channel 3 reported `Success` before the bracket was proven.
    StatusSuccess,
    /// Channel 3 reported `Failure`; its reason is kept as a closed value.
    StatusFailure {
        reason: Option<AttachStatusReasonV2>,
    },
}

/// Closed projection of the `reason` of a channel 3 status. The peer's text
/// is compared and dropped: no byte of it is kept, so none can reach a
/// `Debug` output or a log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::runtime::kubernetes) enum AttachStatusReasonV2 {
    /// Exactly `NonZeroExitCode`.
    NonZeroExitCode,
    /// Exactly `InternalError`.
    InternalError,
    /// Any other non-empty text.
    Other,
}
impl AttachStatusReasonV2 {
    /// `None` for an empty reason, which says nothing.
    fn project(reason: &str) -> Option<Self> {
        match reason {
            "" => None,
            "NonZeroExitCode" => Some(Self::NonZeroExitCode),
            "InternalError" => Some(Self::InternalError),
            _ => Some(Self::Other),
        }
    }
}

/// Channel 3 `v1.Status` reduced to a closed projection: `status` and a
/// closed `reason`. An absent, null or empty reason is `None`, and a reason
/// that is not a string refuses the document. The message, details and code
/// of the document are never kept.
fn project_status(payload: &[u8]) -> Result<AttachEndV2, Violation> {
    if payload.len() > MAX_STATUS_BYTES {
        return Err(Violation::TooBig);
    }
    let document = crate::json::decode_unique(payload, 16).map_err(|_| Violation::Protocol)?;
    let fields = document.as_object().ok_or(Violation::Protocol)?;
    let reason = match fields.get("reason") {
        None | Some(Value::Null) => None,
        Some(Value::String(reason)) => AttachStatusReasonV2::project(reason),
        Some(_) => return Err(Violation::Protocol),
    };
    match fields.get("status").and_then(Value::as_str) {
        Some("Success") => Ok(AttachEndV2::StatusSuccess),
        Some("Failure") => Ok(AttachEndV2::StatusFailure { reason }),
        _ => Err(Violation::Protocol),
    }
}

/// Detail kept beside the closed runtime error of an attempt that ended
/// without identity. It carries no body, URL, path or token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::runtime::kubernetes) enum AttachCauseV2 {
    /// The bootstrap exchange failed. Its echo would not have been an
    /// identity anyway.
    Bootstrap(BootstrapError),
    /// The second observation failed or was refused by the exact-instance
    /// parser.
    SecondObservation(KubernetesSessionRuntimeError),
    /// The second observation is valid and differs from the first.
    InstanceChanged,
    /// The caller's cancellation was raised after the request was initiated
    /// and seen while the bracket waited for the Ready or for identity two.
    /// Beside `AttachDeadline` or an error of the upgrade, it says that the
    /// cancellation had been asked for when that end was observed.
    Cancelled,
    /// The caller stopped driving the bracket: no verdict can arrive.
    CallerGone,
    /// The owner task was dropped before its own end: its runtime stopped (a
    /// panic inside the task would be recorded the same way). If it held the
    /// stream, the stream was dropped with it, without the stdin close and
    /// the close frame, and the byte counts of that record read 0 because the
    /// stopped owner could not report them. If it still held the initiated
    /// request, that request was dropped and no stream was ever held.
    OwnerStopped,
    /// A failure on this side that the peer did not cause: a message could
    /// not be reserved or queued, no runtime could own the attempt, or the
    /// owner task could not be started. Never recorded as a close by the
    /// peer.
    LocalFailure,
}

/// Identity one of the bracket: the facts of one observation of the exact
/// instance with a Running container. The spec digest, `restartCount == 0`
/// and the anchored authority are enforced by `parse_session_observation` on
/// both observations and are not copied here. Declared API facts, never an
/// ownership.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::runtime::kubernetes) struct AttachIdentityV2 {
    pod_uid: String,
    node_name: String,
    container_id: String,
    image_id: String,
    started_at: DateTime<Utc>,
}
impl AttachIdentityV2 {
    /// `Some` only for a scheduled instance whose container is Running with
    /// a container id, a start time and an image id.
    pub(in crate::runtime::kubernetes) fn from_observation(
        observed: &SessionObservedV2,
    ) -> Option<Self> {
        match observed.state() {
            SessionObservationStateV2::Scheduled {
                node_name,
                container:
                    SessionContainerStateV2::Running {
                        container_id,
                        image_id: Some(image_id),
                        started_at,
                    },
            } => Some(Self {
                pod_uid: observed.pod_uid().into(),
                node_name: node_name.clone(),
                container_id: container_id.clone(),
                image_id: image_id.clone(),
                started_at: *started_at,
            }),
            _ => None,
        }
    }
    pub(in crate::runtime::kubernetes) fn pod_uid(&self) -> &str {
        &self.pod_uid
    }
    pub(in crate::runtime::kubernetes) fn node_name(&self) -> &str {
        &self.node_name
    }
    pub(in crate::runtime::kubernetes) fn container_id(&self) -> &str {
        &self.container_id
    }
    pub(in crate::runtime::kubernetes) fn image_id(&self) -> &str {
        &self.image_id
    }
    pub(in crate::runtime::kubernetes) fn started_at(&self) -> DateTime<Utc> {
        self.started_at
    }
    /// Canonical JSON (sorted keys, no whitespace) bound by the attach
    /// request digest in the place of a body.
    pub(in crate::runtime::kubernetes) fn canonical_bytes(&self) -> Outcome<Vec<u8>> {
        crate::control::kubernetes_session::validation::canonical(&json!({
            "version": 1,
            "pod_uid": self.pod_uid,
            "node_name": self.node_name,
            "container_id": self.container_id,
            "image_id": self.image_id,
            "started_at": self.started_at.to_rfc3339_opts(SecondsFormat::Nanos, true),
        }))
        .map_err(|_| REFUSED)
    }
}

/// Success of one bracket: the preamble travelled and both observations
/// matched. The preamble stays declared metadata, and nothing here is a
/// start, a permit, a settlement or a quota release.
#[derive(Clone, Debug)]
pub(in crate::runtime::kubernetes) struct AttachPreambleOutcomeV2 {
    identity: AttachIdentityV2,
    preamble: BootstrapObservationV2,
    end: AttachEndV2,
}
impl AttachPreambleOutcomeV2 {
    pub(in crate::runtime::kubernetes) fn identity(&self) -> &AttachIdentityV2 {
        &self.identity
    }
    pub(in crate::runtime::kubernetes) fn preamble(&self) -> &BootstrapObservationV2 {
        &self.preamble
    }
    pub(in crate::runtime::kubernetes) fn end(&self) -> &AttachEndV2 {
        &self.end
    }
}

/// Recorded facts of one ended attempt, successful or not. Counts only: no
/// stdout, stderr or status byte is kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::runtime::kubernetes) struct AttachReportV2 {
    request_initiated: bool,
    end: Option<AttachEndV2>,
    stdout_bytes: u64,
    stderr_bytes: u64,
    cause: Option<AttachCauseV2>,
}
impl AttachReportV2 {
    /// True once the consumed guard admitted the request: an attach may have
    /// reached the API server, whatever was answered.
    pub(in crate::runtime::kubernetes) fn request_initiated(&self) -> bool {
        self.request_initiated
    }
    /// `None`: no upgraded stream was ever held by this attempt, also when
    /// its owner was stopped while the upgrade was pending. `Some` as soon as
    /// one was held, also when its owner was stopped with its runtime
    /// (`OwnerStopped`) or could not be started (`LocalFailure`).
    pub(in crate::runtime::kubernetes) fn end(&self) -> Option<&AttachEndV2> {
        self.end.as_ref()
    }
    /// Bytes of the single envelope handed to the bootstrap reader. Not
    /// reported, and read as 0, when the cause is `OwnerStopped`.
    pub(in crate::runtime::kubernetes) fn stdout_bytes(&self) -> u64 {
        self.stdout_bytes
    }
    /// Stderr bytes counted, then discarded. Not reported, and read as 0,
    /// when the cause is `OwnerStopped`.
    pub(in crate::runtime::kubernetes) fn stderr_bytes(&self) -> u64 {
        self.stderr_bytes
    }
    pub(in crate::runtime::kubernetes) fn cause(&self) -> Option<AttachCauseV2> {
        self.cause
    }
}

/// One slot of the attach capacity of a runtime. Dropping it returns the
/// slot, which only the attempt owner does, at its effective end.
pub(in crate::runtime::kubernetes) struct AttachCapacity {
    permit: OwnedSemaphorePermit,
}
impl AttachCapacity {
    pub(in crate::runtime::kubernetes) fn new(permit: OwnedSemaphorePermit) -> Self {
        Self { permit }
    }
    fn is_from(&self, pool: &Arc<Semaphore>) -> bool {
        Arc::ptr_eq(self.permit.semaphore(), pool)
    }
}

#[derive(Clone)]
struct AttachRecordV2 {
    result: Outcome<AttachPreambleOutcomeV2>,
    report: AttachReportV2,
}

/// What one ended attempt publishes; the owner adds the initiation mark.
struct AttachFactsV2 {
    result: Outcome<AttachPreambleOutcomeV2>,
    end: Option<AttachEndV2>,
    stdout_bytes: u64,
    stderr_bytes: u64,
    cause: Option<AttachCauseV2>,
}
impl AttachFactsV2 {
    fn without_stream(error: KubernetesSessionRuntimeError, cause: Option<AttachCauseV2>) -> Self {
        Self {
            result: Err(error),
            end: None,
            stdout_bytes: 0,
            stderr_bytes: 0,
            cause,
        }
    }

    /// An upgraded stream was held and is dropped without the ordinary end
    /// of its owner. Never an identity; the counts were not reported.
    fn stream_dropped(cause: AttachCauseV2) -> Self {
        Self {
            result: Err(UNPROVEN),
            end: Some(AttachEndV2::ClosedByUsWithoutIdentity),
            stdout_bytes: 0,
            stderr_bytes: 0,
            cause: Some(cause),
        }
    }

    /// The opening ended without a stream. With the caller gone (its verdict
    /// sender was dropped) the record is the lost caller. Otherwise it is
    /// the error of the opening, beside the cause of an unproven verdict
    /// that the caller had already given: what the caller asked for and the
    /// end that was observed stay separate. The receiver is not polled after
    /// this call.
    fn opening_ended(
        error: KubernetesSessionRuntimeError,
        verdict: &mut oneshot::Receiver<BracketVerdictV2>,
    ) -> Self {
        match verdict.try_recv() {
            Err(oneshot::error::TryRecvError::Closed) => {
                Self::without_stream(UNPROVEN, Some(AttachCauseV2::CallerGone))
            }
            Ok(BracketVerdictV2::Unproven(cause)) => Self::without_stream(error, Some(cause)),
            Ok(BracketVerdictV2::Proven) | Err(oneshot::error::TryRecvError::Empty) => {
                Self::without_stream(error, None)
            }
        }
    }
}

enum AttachPhase {
    /// Capacity reserved; `attach_exact_instance` was never entered.
    Idle(AttachCapacity),
    /// Entered once; no request was initiated yet.
    Claimed(AttachCapacity),
    /// The owner task holds the initiated request, then its stream, and the
    /// capacity. Its handle is kept here once the task is spawned, unless
    /// the task already ended.
    Owned(Option<JoinHandle<()>>),
    Ended,
}

struct AttemptInner {
    /// Idle, then Claimed, then Owned or Ended; never backwards.
    phase: Mutex<AttachPhase>,
    record: watch::Sender<Option<AttachRecordV2>>,
    /// Set synchronously when the consumed guard admitted the request.
    initiated: AtomicBool,
}
impl AttemptInner {
    fn phase(&self) -> MutexGuard<'_, AttachPhase> {
        self.phase.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Publishes the single record of the attempt; a later call changes
    /// nothing. Whatever the phase still held (the capacity of a claim whose
    /// request was never initiated) is returned before the record becomes
    /// visible.
    fn publish(&self, facts: AttachFactsV2) {
        let previous = std::mem::replace(&mut *self.phase(), AttachPhase::Ended);
        drop(previous);
        let record = AttachRecordV2 {
            result: facts.result,
            report: AttachReportV2 {
                request_initiated: self.initiated.load(Ordering::Acquire),
                end: facts.end,
                stdout_bytes: facts.stdout_bytes,
                stderr_bytes: facts.stderr_bytes,
                cause: facts.cause,
            },
        };
        self.record.send_if_modified(|current| {
            if current.is_some() {
                return false;
            }
            *current = Some(record);
            true
        });
    }
}

/// Caller-retained owner of one attach attempt (`Clone`, shared state).
///
/// The attempt is single-use and has two owners in turn.
///
/// Until the upgrade request is initiated, the owner is the caller's future
/// (the one returned by `attach_exact_instance`). Dropping that future then
/// abandons an attempt that sent nothing: it is recorded at that moment as
/// `AttachIdentityUnproven` with the cause `CallerGone`, `request_initiated`
/// is false, and the capacity is returned at that moment. A refusal before
/// the initiation returns the capacity in the same way, when it is recorded.
///
/// From the initiation on, the owner is one task. It holds the initiated
/// request, then its upgraded IO, with the attach capacity and the bootstrap
/// owner, until the effective end; the capacity is returned exactly once, by
/// that task, after the request and the IO are dropped and before the record
/// becomes visible. From then on, cancelling a `join`, dropping a clone or
/// dropping the future that drives the bracket neither stops the task nor
/// returns the capacity, and it makes no end: a pending upgrade ends at its
/// answer or at its handshake bound, a stream at its deadline. When the
/// upgrade ends without a stream, the connection of that request is left to
/// the HTTP client. For a request dropped while its answer is awaited, the
/// client's connection task closes the connection once nobody waits for the
/// answer. For an upgrade that was answered and refused, the answer is
/// dropped and the connection is closed because the client keeps no idle
/// connection: `pool_max_idle_per_host(0)`, set where `Transport::new`
/// builds the client in `transport.rs`. Both closings are behaviours of the
/// HTTP client that this module assumes and does not enforce. If the caller
/// is gone when the upgrade ends without a stream, the record is
/// `AttachIdentityUnproven` with the cause `CallerGone`. The server may have
/// accepted the request all the same, so the Pod's single attach must be
/// treated as spent. Only the stop of its runtime drops that task earlier,
/// which is recorded with the cause `OwnerStopped`.
///
/// Drop is never a successful join.
#[derive(Clone)]
pub(in crate::runtime::kubernetes) struct AttachAttemptV2(Arc<AttemptInner>);
impl AttachAttemptV2 {
    pub(in crate::runtime::kubernetes) fn new(capacity: AttachCapacity) -> Self {
        let (record, _) = watch::channel(None);
        Self(Arc::new(AttemptInner {
            phase: Mutex::new(AttachPhase::Idle(capacity)),
            record,
            initiated: AtomicBool::new(false),
        }))
    }

    /// The one recorded outcome, once the attempt has effectively ended.
    ///
    /// An attempt that was never entered has no outcome to wait for: the
    /// answer is then an immediate `AttachIdentityUnproven`, and `report`
    /// stays `None`. No path returns success without a recorded success.
    pub(in crate::runtime::kubernetes) async fn join(&self) -> Outcome<AttachPreambleOutcomeV2> {
        if self.is_idle() {
            return Err(UNPROVEN);
        }
        let mut record = self.0.record.subscribe();
        let recorded = match record.wait_for(|current| current.is_some()).await {
            Ok(current) => (*current).clone(),
            Err(_) => None,
        };
        recorded.map_or(Err(UNPROVEN), |record| record.result)
    }

    /// Facts of the ended attempt; `None` until its record is published.
    pub(in crate::runtime::kubernetes) fn report(&self) -> Option<AttachReportV2> {
        self.0
            .record
            .borrow()
            .as_ref()
            .map(|record| record.report.clone())
    }

    fn is_idle(&self) -> bool {
        matches!(*self.0.phase(), AttachPhase::Idle(_))
    }

    /// True only for a claimed attempt whose slot comes from `pool`: the
    /// transport refuses a capacity that is not its own.
    pub(in crate::runtime::kubernetes) fn holds_capacity_of(&self, pool: &Arc<Semaphore>) -> bool {
        matches!(&*self.0.phase(), AttachPhase::Claimed(capacity) if capacity.is_from(pool))
    }

    /// Called by the transport between the guard and the initiation, with no
    /// await around it.
    pub(in crate::runtime::kubernetes) fn admit_initiation(&self) {
        self.0.initiated.store(true, Ordering::Release);
    }

    /// Single use: only an idle attempt can be claimed, and nothing is sent
    /// for an attempt that was already entered.
    fn claim(&self) -> Outcome<AttachClaimV2> {
        let mut phase = self.0.phase();
        match std::mem::replace(&mut *phase, AttachPhase::Ended) {
            AttachPhase::Idle(capacity) => {
                *phase = AttachPhase::Claimed(capacity);
                Ok(AttachClaimV2 {
                    attempt: self.clone(),
                    armed: true,
                })
            }
            other => {
                *phase = other;
                Err(REFUSED)
            }
        }
    }
}
impl fmt::Debug for AttachAttemptV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let phase = match &*self.0.phase() {
            AttachPhase::Idle(_) => "idle",
            AttachPhase::Claimed(_) => "claimed",
            AttachPhase::Owned(Some(task)) if task.is_finished() => "owner task finished",
            AttachPhase::Owned(_) => "owned by its task",
            AttachPhase::Ended => "ended",
        };
        f.debug_struct("AttachAttemptV2")
            .field("phase", &phase)
            .field("recorded", &self.0.record.borrow().is_some())
            .finish()
    }
}

/// The entered state of one attempt before its upgrade request is initiated:
/// the caller's future is the owner. If that future is dropped here, the
/// attempt is recorded as unproven with the cause `CallerGone` and its
/// capacity is returned, instead of leaving a later join without an answer.
/// No request was sent. Once the transport has initiated the request, the
/// claim is handed to the owner task with no await in between, so this drop
/// never abandons an initiated request.
struct AttachClaimV2 {
    attempt: AttachAttemptV2,
    armed: bool,
}
impl Drop for AttachClaimV2 {
    fn drop(&mut self) {
        if self.armed {
            self.attempt.0.publish(AttachFactsV2::without_stream(
                UNPROVEN,
                Some(AttachCauseV2::CallerGone),
            ));
        }
    }
}
impl AttachClaimV2 {
    /// Records a refusal that left no stream, and returns its error.
    fn refuse(
        mut self,
        error: KubernetesSessionRuntimeError,
        cause: Option<AttachCauseV2>,
    ) -> KubernetesSessionRuntimeError {
        self.armed = false;
        self.attempt
            .0
            .publish(AttachFactsV2::without_stream(error, cause));
        error
    }

    /// Records that the upgraded stream was received and dropped unused,
    /// because its owner task could not be started. The caller drops the
    /// stream before this call.
    fn fail_to_start(mut self) -> KubernetesSessionRuntimeError {
        self.armed = false;
        self.attempt
            .0
            .publish(AttachFactsV2::stream_dropped(AttachCauseV2::LocalFailure));
        UNPROVEN
    }

    /// Starts the one owner task of the attempt on a stream that is already
    /// upgraded. It is the same owner as on the production path, with an
    /// opening that is already resolved.
    fn start<S>(
        self,
        io: S,
        identity: AttachIdentityV2,
        binding: SessionBootstrapBindingV2,
        bootstrap: BootstrapCapacity,
        until: Instant,
    ) -> Outcome<AttachBracketV2>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let Ok(runtime) = Handle::try_current() else {
            drop(io);
            return Err(self.fail_to_start());
        };
        let inputs = StreamInputs {
            identity,
            binding,
            bootstrap,
            until,
        };
        self.start_opening(&runtime, std::future::ready(Ok(io)), inputs)
    }

    /// Hands the opening (the initiated upgrade request, or a stream that is
    /// already upgraded), the capacity and the bootstrap exchange to the one
    /// owner task of the attempt.
    fn start_opening<O, S>(
        mut self,
        runtime: &Handle,
        opening: O,
        inputs: StreamInputs,
    ) -> Outcome<AttachBracketV2>
    where
        O: Future<Output = Outcome<S>> + Send + 'static,
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let inner = self.attempt.0.clone();
        // The phase lock is never held while the task is spawned: a task that
        // a stopping runtime drops at once records its end through this lock.
        let claimed = {
            let mut phase = inner.phase();
            match std::mem::replace(&mut *phase, AttachPhase::Owned(None)) {
                AttachPhase::Claimed(capacity) => Some(capacity),
                other => {
                    *phase = other;
                    None
                }
            }
        };
        let Some(capacity) = claimed else {
            // Not reachable while this claim is armed: its phase is `Claimed`.
            // The opening is dropped unowned, and this attempt held no stream.
            drop(opening);
            return Err(self.refuse(UNPROVEN, Some(AttachCauseV2::LocalFailure)));
        };
        let (ready_sender, ready) = oneshot::channel();
        let (verdict, verdict_receiver) = oneshot::channel();
        let ownership = AttemptOwnership {
            attempt: inner.clone(),
            capacity: Some(capacity),
            stream_held: false,
        };
        // The task is given one value, with the ownership as its last field
        // (see `owner_task`).
        let owned = (opening, inputs, ready_sender, verdict_receiver, ownership);
        let task = runtime.spawn(owner_task(owned));
        {
            let mut phase = inner.phase();
            // A task that already ended has recorded; its handle is dropped.
            if matches!(*phase, AttachPhase::Owned(None)) {
                *phase = AttachPhase::Owned(Some(task));
            }
        }
        self.armed = false;
        Ok(AttachBracketV2 { ready, verdict })
    }
}

/// What the owner task holds, as one value: the opening first, the
/// ownership, which records, last.
type OwnedAttempt<O> = (
    O,
    StreamInputs,
    oneshot::Sender<()>,
    oneshot::Receiver<BracketVerdictV2>,
    AttemptOwnership,
);

/// The owner task of one attempt: it owns the attempt to its effective end,
/// then settles it.
///
/// It takes what it holds as one value, by value. A task that is dropped
/// before its first poll (its runtime was already stopping) therefore drops
/// that one value, and the fields of a tuple are dropped in their order: the
/// opening before the ownership, so the request or the stream is dropped
/// before the slot is returned and the record published. Once the task runs,
/// the ownership is a local declared before the future that holds the
/// request or the stream, so that future is dropped first there as well.
async fn owner_task<O, S>(owned: OwnedAttempt<O>)
where
    O: Future<Output = Outcome<S>>,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (opening, inputs, ready, verdict, mut ownership) = owned;
    let facts = own_attempt(opening, inputs, ready, verdict, &mut ownership.stream_held).await;
    ownership.settle(facts);
}

/// Body of the owner task before its record. It awaits the opening to its
/// end, whoever still waits for the attempt, then runs the stream that the
/// opening gave. `stream_held` is raised as soon as a stream is held, for the
/// record of an owner that is dropped later. Every IO object is dropped
/// before it returns.
async fn own_attempt<O, S>(
    opening: O,
    inputs: StreamInputs,
    ready: oneshot::Sender<()>,
    mut verdict: oneshot::Receiver<BracketVerdictV2>,
    stream_held: &mut bool,
) -> AttachFactsV2
where
    O: Future<Output = Outcome<S>>,
    S: AsyncRead + AsyncWrite + Unpin,
{
    // Its own statement: the initiated request ends here, and nothing of it
    // outlives this line except the stream it gave.
    let opened = opening.await;
    match opened {
        Ok(io) => {
            *stream_held = true;
            run_stream(io, inputs, ready, verdict).await
        }
        Err(error) => {
            // Nothing of the attempt outlives its record.
            drop((inputs, ready));
            AttachFactsV2::opening_ended(error, &mut verdict)
        }
    }
}

/// Held by the owner task. It publishes one record and returns the capacity
/// even when the task is dropped by the stop of its runtime.
struct AttemptOwnership {
    attempt: Arc<AttemptInner>,
    capacity: Option<AttachCapacity>,
    /// The opening gave its stream to this task.
    stream_held: bool,
}
impl AttemptOwnership {
    /// The capacity goes back first: a waiter that sees the record also sees
    /// the slot returned. The initiated request and the stream were dropped
    /// before this call.
    fn settle(&mut self, facts: AttachFactsV2) {
        if let Some(capacity) = self.capacity.take() {
            drop(capacity);
            self.attempt.publish(facts);
        }
    }
}
impl Drop for AttemptOwnership {
    /// Still holding the capacity here means that the task was dropped
    /// before its own end: its runtime stopped (a panic inside the task
    /// would end here too). That is recorded as such, never as a refused
    /// handshake, and the record says whether a stream was held.
    fn drop(&mut self) {
        let facts = if self.stream_held {
            AttachFactsV2::stream_dropped(AttachCauseV2::OwnerStopped)
        } else {
            AttachFactsV2::without_stream(UNPROVEN, Some(AttachCauseV2::OwnerStopped))
        };
        self.settle(facts);
    }
}

/// Caller side of one owned attempt.
struct AttachBracketV2 {
    /// Resolves when the Ready echo was accepted. An error means that the
    /// attempt ended, or will end, without it.
    ready: oneshot::Receiver<()>,
    /// The bracket verdict. Losing this sender means "identity unproven".
    verdict: oneshot::Sender<BracketVerdictV2>,
}

enum BracketVerdictV2 {
    /// The second observation equals the first.
    Proven,
    Unproven(AttachCauseV2),
}

enum StreamEnding {
    Proven,
    Refused {
        error: KubernetesSessionRuntimeError,
        cause: Option<AttachCauseV2>,
        close_code: u16,
    },
    Peer(AttachEndV2),
}
impl StreamEnding {
    fn refused(error: KubernetesSessionRuntimeError, cause: Option<AttachCauseV2>) -> Self {
        Self::Refused {
            error,
            cause,
            close_code: CLOSE_NORMAL,
        }
    }
    fn violation(violation: Violation) -> Self {
        let close_code = match violation {
            Violation::Protocol => CLOSE_PROTOCOL,
            Violation::TooBig => CLOSE_TOO_BIG,
        };
        Self::Refused {
            error: PROTOCOL,
            cause: None,
            close_code,
        }
    }
    fn bootstrap(error: BootstrapError) -> Self {
        let (attach_error, close_code) = match error {
            BootstrapError::Deadline => (DEADLINE, CLOSE_NORMAL),
            BootstrapError::Protocol | BootstrapError::FrameSize | BootstrapError::PartialFrame => {
                (PROTOCOL, CLOSE_PROTOCOL)
            }
            BootstrapError::Binding
            | BootstrapError::Io
            | BootstrapError::Capacity
            | BootstrapError::Unavailable => (UNPROVEN, CLOSE_NORMAL),
        };
        Self::Refused {
            error: attach_error,
            cause: Some(AttachCauseV2::Bootstrap(error)),
            close_code,
        }
    }
}

struct StreamInputs {
    identity: AttachIdentityV2,
    binding: SessionBootstrapBindingV2,
    bootstrap: BootstrapCapacity,
    until: Instant,
}

/// The stream and its counters; lives only inside the owner task.
struct OwnedStream<S> {
    codec: AttachCodec<S>,
    envelope: StdoutEnvelope,
    /// Attach side of the bootstrap pipe: the Hello comes out of `stdin`,
    /// the Ready goes into `stdout`.
    stdin: ReadHalf<DuplexStream>,
    stdout: Option<WriteHalf<DuplexStream>>,
    stdout_bytes: u64,
    stderr_bytes: u64,
    preamble: Option<BootstrapObservationV2>,
    bootstrap_joined: bool,
    caller_present: bool,
    until: Instant,
}
impl<S: AsyncRead + AsyncWrite + Unpin> OwnedStream<S> {
    /// Takes the codec of the upgraded stream and the attach side of the
    /// bootstrap pipe.
    fn new(
        codec: AttachCodec<S>,
        stdin: ReadHalf<DuplexStream>,
        stdout: WriteHalf<DuplexStream>,
        until: Instant,
    ) -> Self {
        Self {
            codec,
            envelope: StdoutEnvelope::new(),
            stdin,
            stdout: Some(stdout),
            stdout_bytes: 0,
            stderr_bytes: 0,
            preamble: None,
            bootstrap_joined: false,
            caller_present: true,
            until,
        }
    }

    /// Runs until one ending is known. Every branch future is cancel safe:
    /// a read that loses the race has consumed nothing, the codec keeps its
    /// own state, and `BootstrapIoOwner::join` keeps its task.
    ///
    /// The order is deliberate. The deadline is polled first. Server items
    /// come before the verdict, so a success verdict is accepted only when
    /// no server item was ready at that poll.
    ///
    /// A write (one stdin chunk, the pong of a ping) is awaited inside its
    /// step, up to the deadline, and is not raced with the verdict: a frame
    /// is written whole or it ends the stream (see `write_ending`).
    async fn drive(
        &mut self,
        owner: &BootstrapIoOwner,
        ready: oneshot::Sender<()>,
        verdict: &mut oneshot::Receiver<BracketVerdictV2>,
    ) -> StreamEnding {
        let mut ready = Some(ready);
        let mut stdin_chunk = [0_u8; STDIN_CHUNK_BYTES];
        let mut stdin_open = true;
        loop {
            tokio::select! {
                biased;
                () = tokio::time::sleep_until(self.until) => return self.deadline_ending(),
                item = self.codec.read() => {
                    if let Err(ending) = self.server_item(item, verdict).await {
                        return ending;
                    }
                },
                joined = owner.join(), if !self.bootstrap_joined => {
                    self.bootstrap_joined = true;
                    match joined {
                        Ok(observed) => {
                            self.preamble = Some(observed);
                            if let Some(ready) = ready.take() {
                                // A caller that is gone cannot be told.
                                let _ = ready.send(());
                            }
                        }
                        Err(error) => return StreamEnding::bootstrap(error),
                    }
                },
                read = self.stdin.read(&mut stdin_chunk), if stdin_open => match read {
                    // The bootstrap owner dropped its halves. That is not the
                    // v5 stdin close: stdin stays open until the bracket ends.
                    Ok(0) | Err(_) => stdin_open = false,
                    Ok(count) => {
                        let bytes = stdin_chunk.get(..count).unwrap_or(NO_BYTES);
                        if let Err(ending) = self.send_stdin(bytes, verdict).await {
                            return ending;
                        }
                    }
                },
                answer = &mut *verdict, if self.caller_present => match answer {
                    Ok(BracketVerdictV2::Proven) => return self.proven_ending(),
                    Ok(BracketVerdictV2::Unproven(cause)) => {
                        return StreamEnding::refused(UNPROVEN, Some(cause));
                    }
                    // The stream is kept until its deadline; it can no longer
                    // end with an identity.
                    Err(_) => self.caller_present = false,
                },
            }
        }
    }

    /// The owner reached the deadline between two of its steps, with no
    /// write pending. Before the Ready the bracket deadline is a handshake
    /// deadline; after it, the bracket did not complete and the identity is
    /// unproven. A write that is still pending at the deadline is another
    /// ending (see `write_ending`).
    fn deadline_ending(&self) -> StreamEnding {
        let cause = (!self.caller_present).then_some(AttachCauseV2::CallerGone);
        if self.preamble.is_some() {
            StreamEnding::refused(UNPROVEN, cause)
        } else {
            StreamEnding::refused(DEADLINE, cause)
        }
    }

    /// A timer may fire late: a verdict that arrives at or after the
    /// deadline, or without an accepted Ready, is never a success.
    fn proven_ending(&self) -> StreamEnding {
        if self.preamble.is_some() && Instant::now() < self.until {
            StreamEnding::Proven
        } else {
            StreamEnding::refused(UNPROVEN, None)
        }
    }

    /// One stdin chunk as one whole binary message, flushed inside the
    /// deadline.
    async fn send_stdin(
        &mut self,
        bytes: &[u8],
        verdict: &mut oneshot::Receiver<BracketVerdictV2>,
    ) -> Result<(), StreamEnding> {
        let Some(message) = stdin_message(bytes) else {
            // Nothing was written and the peer did nothing: closed by us.
            return Err(StreamEnding::refused(
                UNPROVEN,
                Some(AttachCauseV2::LocalFailure),
            ));
        };
        let sent = self.codec.send(message, self.until).await;
        sent.map_err(|failure| write_ending(failure, verdict))
    }

    /// One item of the server. A ping is answered here: the codec library
    /// queued its pong, and the flush is awaited inside the deadline, so
    /// that a pong the peer does not absorb is seen as a pending write.
    async fn server_item(
        &mut self,
        item: ServerItem,
        verdict: &mut oneshot::Receiver<BracketVerdictV2>,
    ) -> Result<(), StreamEnding> {
        match item {
            ServerItem::Message(message) => self.channel_message(message.bytes()).await,
            ServerItem::Ping => {
                let flushed = self.codec.flush(self.until).await;
                flushed.map_err(|failure| write_ending(failure, verdict))
            }
            // An unsolicited pong needs no answer.
            ServerItem::Pong => Ok(()),
            ServerItem::Closed | ServerItem::Gone => {
                Err(StreamEnding::Peer(AttachEndV2::PeerClosed))
            }
            ServerItem::Refused(violation) => Err(StreamEnding::violation(violation)),
        }
    }

    /// The first byte of a message is its channel. Only stdout, stderr and
    /// the status channel are accepted from the server.
    async fn channel_message(&mut self, message: &[u8]) -> Result<(), StreamEnding> {
        let Some((channel, payload)) = message.split_first() else {
            return Err(StreamEnding::violation(Violation::Protocol));
        };
        match *channel {
            CHANNEL_STDOUT => self.stdout_payload(payload).await,
            CHANNEL_STDERR => {
                // Counted, then dropped with the message buffer; never copied.
                let total = u64::try_from(payload.len())
                    .ok()
                    .and_then(|length| self.stderr_bytes.checked_add(length))
                    .filter(|total| *total <= MAX_STDERR_BYTES)
                    .ok_or_else(|| StreamEnding::violation(Violation::TooBig))?;
                self.stderr_bytes = total;
                Ok(())
            }
            CHANNEL_STATUS => Err(match project_status(payload) {
                Ok(end) => StreamEnding::Peer(end),
                Err(violation) => StreamEnding::violation(violation),
            }),
            // Channel 0 (stdin), 4 (resize), 255 (close) and any other value
            // are never sent by a server to this client.
            _ => Err(StreamEnding::violation(Violation::Protocol)),
        }
    }

    async fn stdout_payload(&mut self, payload: &[u8]) -> Result<(), StreamEnding> {
        self.envelope
            .admit(payload)
            .map_err(StreamEnding::violation)?;
        if payload.is_empty() {
            return Ok(());
        }
        let Some(stdout) = self.stdout.as_mut() else {
            return Ok(());
        };
        // The pipe holds one whole envelope, so this write does not wait for
        // the reader. A reader that already ended is reported by its join.
        match timeout_at(self.until, stdout.write_all(payload)).await {
            Ok(Ok(())) => {
                self.stdout_bytes = self
                    .stdout_bytes
                    .saturating_add(u64::try_from(payload.len()).unwrap_or(u64::MAX));
                Ok(())
            }
            Ok(Err(_)) => {
                self.stdout = None;
                Ok(())
            }
            Err(_) => Err(self.deadline_ending()),
        }
    }

    /// Bounded close of the stream, after which every IO object of the
    /// stream is dropped. The codec first writes what its state still allows
    /// (see `AttachCodec::close`), then the attach side of the bootstrap
    /// pipe goes. Returns the stdout and stderr counts and whether the
    /// bootstrap owner was already joined.
    async fn close(self, code: u16) -> (u64, u64, bool) {
        let Self {
            codec,
            stdin,
            stdout,
            stdout_bytes,
            stderr_bytes,
            bootstrap_joined,
            ..
        } = self;
        codec.close(code).await;
        drop((stdin, stdout));
        (stdout_bytes, stderr_bytes, bootstrap_joined)
    }
}

/// One stdin chunk behind its channel byte, in a buffer reserved exactly.
/// `None` when the reservation fails; nothing was written then.
fn stdin_message(bytes: &[u8]) -> Option<Vec<u8>> {
    let length = bytes.len().checked_add(STDIN_PREFIX.len())?;
    let mut message = Vec::new();
    message.try_reserve_exact(length).ok()?;
    message.extend_from_slice(STDIN_PREFIX);
    message.extend_from_slice(bytes);
    Some(message)
}

/// Ending of a stream write that did not complete.
///
/// A write that is still pending at the deadline is an observed deadline,
/// before and after the Ready: the record is `AttachDeadline`. Its frame may
/// be cut, so the codec writes nothing more. What the caller had already
/// stated is read from the verdict channel without waiting and kept as the
/// cause beside that deadline: the cause of an unproven verdict, or
/// `CallerGone` when its sender was dropped. A proven verdict is never a
/// success here. The receiver is not polled after this call.
fn write_ending(
    failure: WriteFailure,
    verdict: &mut oneshot::Receiver<BracketVerdictV2>,
) -> StreamEnding {
    match failure {
        WriteFailure::Deadline => {
            let cause = match verdict.try_recv() {
                Ok(BracketVerdictV2::Unproven(cause)) => Some(cause),
                Ok(BracketVerdictV2::Proven) | Err(oneshot::error::TryRecvError::Empty) => None,
                Err(oneshot::error::TryRecvError::Closed) => Some(AttachCauseV2::CallerGone),
            };
            StreamEnding::refused(DEADLINE, cause)
        }
        // The transport failed under the write: an end by the peer.
        WriteFailure::Broken => StreamEnding::Peer(AttachEndV2::PeerClosed),
        // Nothing was written and the peer did nothing: closed by us.
        WriteFailure::Local => StreamEnding::refused(UNPROVEN, Some(AttachCauseV2::LocalFailure)),
    }
}

/// Body of the owner task once a stream is held: the bootstrap exchange over
/// the stream, the wait for the bracket verdict, the bounded close, then the
/// join of the bootstrap owner. Every IO object is dropped before it returns.
async fn run_stream<S>(
    io: S,
    inputs: StreamInputs,
    ready: oneshot::Sender<()>,
    mut verdict: oneshot::Receiver<BracketVerdictV2>,
) -> AttachFactsV2
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let StreamInputs {
        identity,
        binding,
        bootstrap,
        until,
    } = inputs;
    // Wrapping the stream reads and writes nothing; the bound is there
    // because the owner awaits nothing without one.
    let Ok(codec) = timeout_at(until, AttachCodec::new(io)).await else {
        // The stream was dropped with the unfinished wrapping, unused.
        return AttachFactsV2 {
            result: Err(DEADLINE),
            end: Some(AttachEndV2::ClosedByUsWithoutIdentity),
            stdout_bytes: 0,
            stderr_bytes: 0,
            cause: None,
        };
    };
    let (bootstrap_side, attach_side) = tokio::io::duplex(PIPE_BYTES);
    let (bootstrap_input, bootstrap_output) = tokio::io::split(bootstrap_side);
    let (stdin, stdout) = tokio::io::split(attach_side);
    let mut stream = OwnedStream::new(codec, stdin, stdout, until);
    // The bootstrap window never exceeds the caller deadline.
    let window = until.min(Instant::now() + bootstrap::MAX_HANDSHAKE_TIME);
    let owner = BootstrapIoOwner::start(
        binding,
        bootstrap_input,
        bootstrap_output,
        bootstrap,
        window,
    );
    let ending = match &owner {
        Ok(owner) => stream.drive(owner, ready, &mut verdict).await,
        Err(error) => StreamEnding::bootstrap(*error),
    };
    let (close_code, result, end, cause) = match (ending, stream.preamble.clone()) {
        (StreamEnding::Proven, Some(preamble)) => (
            CLOSE_NORMAL,
            Ok((identity, preamble)),
            AttachEndV2::ClosedByUsAfterIdentity,
            None,
        ),
        (StreamEnding::Proven, None) => (
            CLOSE_NORMAL,
            Err(UNPROVEN),
            AttachEndV2::ClosedByUsWithoutIdentity,
            None,
        ),
        (
            StreamEnding::Refused {
                error,
                cause,
                close_code,
            },
            _,
        ) => (
            close_code,
            Err(error),
            AttachEndV2::ClosedByUsWithoutIdentity,
            cause,
        ),
        (StreamEnding::Peer(end), _) => (CLOSE_NORMAL, Err(UNPROVEN), end, None),
    };
    // The bounded close ends stdin in every case, exactly once: by the v5
    // stdin close while the stream still takes a frame, and by dropping the
    // connection otherwise. Nothing is read afterwards.
    let (stdout_bytes, stderr_bytes, joined) = stream.close(close_code).await;
    // With both ends of its pipe closed the bootstrap task ends at once, and
    // at its own window at the latest: it is observed before the record.
    if let Ok(owner) = &owner
        && !joined
    {
        let _ = owner.join().await;
    }
    AttachFactsV2 {
        result: result.map(|(identity, preamble)| AttachPreambleOutcomeV2 {
            identity,
            preamble,
            end: end.clone(),
        }),
        end: Some(end),
        stdout_bytes,
        stderr_bytes,
        cause,
    }
}

/// One fixed attach request. The caller names only the Pod of a retained
/// instance; the transport builds the method, path, query and headers.
pub(in crate::runtime::kubernetes) struct SessionAttachRequestV2<'a> {
    pub(in crate::runtime::kubernetes) pod_name: &'a str,
    /// Canonical identity-one bytes, bound by the request digest.
    pub(in crate::runtime::kubernetes) identity_one: &'a [u8],
    /// Holder of the attach capacity and of the initiation mark.
    pub(in crate::runtime::kubernetes) attempt: &'a AttachAttemptV2,
    pub(in crate::runtime::kubernetes) until: Instant,
}

/// The initiated upgrade of one attach request. The transport polled it
/// once, synchronously after the consumed guard, and did nothing else to it:
/// the owner task of the attempt awaits it to its end. It resolves to the
/// upgraded IO of that request only for the exact `101` of the v5
/// subprotocol. Every other answer, a transport failure and the elapsed
/// handshake bound resolve to their closed error with no stream held, and
/// nothing is retried.
pub(in crate::runtime::kubernetes) type AttachOpeningV2 =
    Pin<Box<dyn Future<Output = Outcome<reqwest::Upgraded>> + Send>>;

/// Resolves once `cancelled` is raised. The flag is read at once, then every
/// `CANCEL_POLL`: an `AtomicBool` wakes nobody. Raced with a wait of the
/// bracket, it lets that wait end within one period of the cancellation.
async fn raised(cancelled: &AtomicBool) {
    while !cancelled.load(Ordering::Acquire) {
        tokio::time::sleep(CANCEL_POLL).await;
    }
}

impl KubernetesRuntime {
    /// Reserves one slot of this runtime's attach capacity, or
    /// `AttachCapacity`. The slot is separate from the request slots.
    pub(in crate::runtime::kubernetes) fn reserve_attach_capacity(
        &self,
    ) -> Outcome<AttachCapacity> {
        self.transport.reserve_attach()
    }

    #[cfg(test)]
    pub(in crate::runtime::kubernetes) fn attach_slots_available(&self) -> usize {
        self.transport.attach_slots_available()
    }

    /// Takes every request slot that is free now, so that a test can hold
    /// the local contention that answers `Busy`.
    #[cfg(test)]
    pub(in crate::runtime::kubernetes) fn hold_request_slots(
        &self,
    ) -> Vec<tokio::sync::SemaphorePermit<'_>> {
        self.transport.hold_request_slots()
    }

    /// Bracketed preamble-only attach of the exact instance.
    ///
    /// Requests, in order: anchors, Pod (identity one), attach, Pod
    /// (identity two), anchors. Only the Hello is written before identity
    /// two, and the stdin of an owned stream is ended in every case, by its
    /// v5 close or with the connection. The recorded outcome is also read
    /// through `attempt.join()`; it is `Ok` only when the Ready echo was
    /// accepted and both observations are equal. No divergence, error,
    /// cancellation or lost caller ends `Ok`.
    ///
    /// Ownership. Until the upgrade request is initiated, this future is the
    /// owner of the attempt: dropping it then records
    /// `AttachIdentityUnproven` with the cause `CallerGone`, returns the
    /// attach slot at that moment, and nothing was sent. The transport
    /// initiates the request with one synchronous poll, right after the
    /// consumed guard, and the initiated request goes to the owner task with
    /// no await in between. From then on the owner task ends the attempt
    /// with or without this future: dropping this future makes no end and
    /// returns no slot. A pending upgrade ends at its answer or at its
    /// handshake bound, a stream at its deadline, and the record stays
    /// readable through `join`.
    ///
    /// Cancellation. `cancelled` is read before any request, and by the
    /// transport immediately before the guard. After the initiation it is
    /// read every `CANCEL_POLL` while this future waits for the Ready and
    /// for identity two: a raised flag sends the verdict `Cancelled` at
    /// once. The owner task acts on a verdict between two of its steps: it
    /// then closes the stream and records the cause `Cancelled`. The upgrade
    /// itself is not interrupted: it is bounded by `ATTACH_HANDSHAKE`, and a
    /// verdict given meanwhile is acted on as soon as the stream exists; if
    /// the upgrade ends without a stream, its own error is recorded with the
    /// cause `Cancelled`. A write that the peer does not absorb is not
    /// interrupted either: it lasts until the deadline, the stream is then
    /// unusable and is dropped without another frame, and the record is
    /// `AttachDeadline` with the cause `Cancelled`. A flag that this future
    /// saw raised never yields `Ok`.
    ///
    /// Identity two is asked again after `Kubernetes(Busy)`, which is the
    /// refusal of the local request slots, and only inside `until`.
    ///
    /// `until` must be in the future and at most `ATTACH_BRACKET_MAX` away.
    /// An attempt is consumed by this call, whatever it returns.
    #[allow(
        clippy::too_many_arguments,
        reason = "one parameter per independent input of the bracket, as in the reviewed design"
    )]
    pub(in crate::runtime::kubernetes) async fn attach_exact_instance<G: SessionEffectGuardV2>(
        &self,
        instance: &SessionInstanceV2,
        binding: SessionBootstrapBindingV2,
        guard: G,
        attempt: &AttachAttemptV2,
        bootstrap: BootstrapCapacity,
        until: Instant,
        cancelled: &AtomicBool,
    ) -> Outcome<()> {
        let claim = attempt.claim()?;
        let now = Instant::now();
        if until <= now || until.duration_since(now) > ATTACH_BRACKET_MAX {
            return Err(claim.refuse(DEADLINE, None));
        }
        // Local refusals before any request: another authority, a binding
        // that is not the preparation of this instance, or a cancellation.
        let target = instance.target();
        let name = target.pod_name();
        if target.authority_fingerprint() != self.binding.fingerprint()
            || !valid_dns_label(name)
            || binding.authority_fingerprint != target.authority_fingerprint()
            || binding.preparation_sha256 != target.preparation_sha256()
            || session_pod_name(binding.attempt_id) != name
            || cancelled.load(Ordering::Acquire)
        {
            return Err(claim.refuse(REFUSED, None));
        }
        // The owner task needs a runtime. It is taken before any request, so
        // that nothing can fail between the initiation and its owner.
        let Ok(runtime) = Handle::try_current() else {
            return Err(claim.refuse(REFUSED, Some(AttachCauseV2::LocalFailure)));
        };
        let first = match timeout_at(until, self.observe_session_instance(instance)).await {
            Ok(Ok(observed)) => observed,
            Ok(Err(error)) => return Err(claim.refuse(error, None)),
            Err(_) => return Err(claim.refuse(DEADLINE, None)),
        };
        let Some(identity) = AttachIdentityV2::from_observation(&first) else {
            return Err(claim.refuse(REFUSED, None));
        };
        let identity_one = match identity.canonical_bytes() {
            Ok(bytes) => bytes,
            Err(error) => return Err(claim.refuse(error, None)),
        };
        let request = SessionAttachRequestV2 {
            pod_name: name,
            identity_one: &identity_one,
            attempt,
            until,
        };
        // Not awaited: the transport returns once its one synchronous poll
        // has initiated the request.
        let opening =
            match self
                .transport
                .send_session_attach(&self.binding, request, guard, cancelled)
            {
                Ok(opening) => opening,
                Err(error) => return Err(claim.refuse(error, None)),
            };
        // No await since the initiation, and the claim is armed, so the owner
        // task takes the initiated request here. From here it ends the
        // attempt with or without this future. At most the Hello is written.
        let inputs = StreamInputs {
            identity: identity.clone(),
            binding,
            bootstrap,
            until,
        };
        let AttachBracketV2 { ready, verdict } = claim.start_opening(&runtime, opening, inputs)?;
        if let Some(answer) = self
            .bracket_verdict(instance, attempt, &identity, ready, until, cancelled)
            .await
        {
            // A lost receiver means that the owner task already ended on its
            // own recorded cause.
            let _ = verdict.send(answer);
        }
        attempt.join().await.map(|_| ())
    }

    /// Caller side of the bracket once the owner task holds the attempt:
    /// waits for the accepted Ready, reads identity two and compares it with
    /// identity one. The wait for the Ready covers the upgrade itself.
    ///
    /// `None`: the owner task already ended on its own recorded cause (an
    /// upgrade without a stream, no Ready, a stray byte, a peer close, its
    /// deadline), so no verdict is owed and the caller reads the record at
    /// once instead of waiting for identity two. Both waits also end as soon
    /// as `cancelled` is seen raised, with the verdict `Cancelled`.
    async fn bracket_verdict(
        &self,
        instance: &SessionInstanceV2,
        attempt: &AttachAttemptV2,
        identity: &AttachIdentityV2,
        ready: oneshot::Receiver<()>,
        until: Instant,
        cancelled: &AtomicBool,
    ) -> Option<BracketVerdictV2> {
        tokio::select! {
            biased;
            () = raised(cancelled) => {
                return Some(BracketVerdictV2::Unproven(AttachCauseV2::Cancelled));
            }
            accepted = ready => {
                if accepted.is_err() {
                    return None;
                }
            }
        }
        let observed = tokio::select! {
            biased;
            () = raised(cancelled) => {
                return Some(BracketVerdictV2::Unproven(AttachCauseV2::Cancelled));
            }
            _ = attempt.join() => return None,
            observed = timeout_at(until, self.observe_identity_two(instance)) => observed,
        };
        Some(match observed {
            Err(_) => BracketVerdictV2::Unproven(AttachCauseV2::SecondObservation(
                KubernetesSessionRuntimeError::Kubernetes(KubernetesError::Timeout),
            )),
            Ok(Err(error)) => BracketVerdictV2::Unproven(AttachCauseV2::SecondObservation(error)),
            Ok(Ok(second))
                if AttachIdentityV2::from_observation(&second).as_ref() != Some(identity) =>
            {
                BracketVerdictV2::Unproven(AttachCauseV2::InstanceChanged)
            }
            // The poll period may not have elapsed since the flag was raised.
            Ok(Ok(_)) if cancelled.load(Ordering::Acquire) => {
                BracketVerdictV2::Unproven(AttachCauseV2::Cancelled)
            }
            Ok(Ok(_)) => BracketVerdictV2::Proven,
        })
    }

    /// Identity two, asked again while the local request slots are all
    /// taken. `Busy` is the refusal of those slots: the refused request was
    /// not sent, and the round is read again from its Pod GET. Every request
    /// of a round is a read. Every other error is final. The caller bounds
    /// the whole wait by its deadline, so this loop extends nothing.
    async fn observe_identity_two(
        &self,
        instance: &SessionInstanceV2,
    ) -> Outcome<SessionObservedV2> {
        loop {
            match self.observe_attached_instance(instance).await {
                Err(KubernetesSessionRuntimeError::Kubernetes(KubernetesError::Busy)) => {
                    tokio::time::sleep(IDENTITY_TWO_RETRY_PAUSE).await;
                }
                observed => return observed,
            }
        }
    }

    /// Identity two: the Pod first, then the anchors, so that the bracket
    /// reads anchors, Pod, attach, Pod, anchors. Detection after the
    /// exchange, not prevention.
    async fn observe_attached_instance(
        &self,
        instance: &SessionInstanceV2,
    ) -> Outcome<SessionObservedV2> {
        let namespace = self.binding.namespace();
        let name = instance.target().pod_name();
        let pod: Value = self
            .transport
            .get_unique(&format!("/api/v1/namespaces/{namespace}/pods/{name}"))
            .await
            .map_err(KubernetesSessionRuntimeError::Kubernetes)?;
        let observed = parse_session_observation(&self.binding, instance, pod)?;
        self.verify_active_authority()
            .await
            .map_err(KubernetesSessionRuntimeError::Kubernetes)?;
        Ok(observed)
    }
}

#[cfg(test)]
#[path = "session_attach_tests.rs"]
mod tests;
