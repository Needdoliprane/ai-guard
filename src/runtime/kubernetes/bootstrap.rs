//! V2 waiting-bootstrap preamble, using the relay's four-byte BE envelope.
//! Attach identity is NOT established by an echo. There is no Release/Start,
//! credential handoff or command execution in this stage of the protocol.
//!
//! Stage S2b gives the worker side a caller: `session_worker` reaches
//! `bootstrap_waiting_for_attach` from a public library entry. The expectation
//! below is still fulfilled by `bootstrap_waiting`, which no item outside
//! tests references: it is dead whether or not rustc counts the callees of
//! expected or allowed items as live. Calling it from non-test code would
//! leave the expectation unfulfilled and fail `clippy -D warnings`.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "bootstrap_waiting keeps no caller outside tests after stage S2b; the worker waiting mode uses bootstrap_waiting_for_attach"
    )
)]
use super::session::SessionBootstrapBindingV2;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Mutex, OwnedSemaphorePermit},
    task::JoinHandle,
    time::{Instant, timeout_at},
};
use uuid::Uuid;

pub(crate) const MAX_FRAME_BYTES: usize = 16 * 1024;
pub(crate) const RESERVED_BYTES: u32 = (2 * MAX_FRAME_BYTES) as u32;
/// Ceiling of one handshake window. The attach owner derives its bootstrap
/// window from this constant instead of copying the number.
pub(crate) const MAX_HANDSHAKE_TIME: Duration = Duration::from_secs(5);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BootstrapError {
    Capacity,
    FrameSize,
    PartialFrame,
    Protocol,
    Binding,
    Io,
    Deadline,
    Unavailable,
}
impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Capacity => "Kubernetes bootstrap capacity unavailable",
            Self::FrameSize => "Kubernetes bootstrap frame exceeds its bound",
            Self::PartialFrame => "Kubernetes bootstrap frame is incomplete",
            Self::Protocol => "Kubernetes bootstrap frame is incompatible",
            Self::Binding => "Kubernetes bootstrap metadata or exchange differs",
            Self::Io => "Kubernetes bootstrap IO failed",
            Self::Deadline => "Kubernetes bootstrap handshake deadline elapsed",
            Self::Unavailable => "Kubernetes bootstrap owner requires reconciliation",
        })
    }
}
impl std::error::Error for BootstrapError {}
type Result<T> = std::result::Result<T, BootstrapError>;

pub(crate) struct BootstrapCapacity {
    _bytes: OwnedSemaphorePermit,
}
impl BootstrapCapacity {
    pub(crate) fn new(bytes: OwnedSemaphorePermit) -> Result<Self> {
        if bytes.num_permits() < RESERVED_BYTES as usize {
            return Err(BootstrapError::Capacity);
        }
        Ok(Self { _bytes: bytes })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BootstrapProvenanceV2 {
    Declared,
}
#[derive(Clone)]
pub(crate) struct BootstrapObservationV2 {
    _exchange_id: Uuid,
}
impl BootstrapObservationV2 {
    pub(crate) fn provenance(&self) -> BootstrapProvenanceV2 {
        BootstrapProvenanceV2::Declared
    }
}
impl std::fmt::Debug for BootstrapObservationV2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BootstrapObservationV2(declared; no release or identity proof)")
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireBinding {
    version: u32,
    installation_id: Uuid,
    control_namespace_uuid: Uuid,
    broker_id: Uuid,
    incarnation_id: Uuid,
    session_id: Uuid,
    attempt_id: Uuid,
    reservation_id: String,
    reservation_input_hash: String,
    authority_fingerprint: String,
    profile_hash: String,
    preparation_sha256: String,
}
impl From<&SessionBootstrapBindingV2> for WireBinding {
    fn from(b: &SessionBootstrapBindingV2) -> Self {
        Self {
            version: b.version,
            installation_id: b.installation_id,
            control_namespace_uuid: b.control_namespace_uuid,
            broker_id: b.broker_id,
            incarnation_id: b.incarnation_id,
            session_id: b.session_id,
            attempt_id: b.attempt_id,
            reservation_id: b.reservation_id.clone(),
            reservation_input_hash: b.reservation_input_hash.clone(),
            authority_fingerprint: b.authority_fingerprint.clone(),
            profile_hash: b.profile_hash.clone(),
            preparation_sha256: b.preparation_sha256.clone(),
        }
    }
}
impl WireBinding {
    fn validate(&self) -> Result<()> {
        if self.version != 2
            || [
                self.installation_id,
                self.control_namespace_uuid,
                self.broker_id,
                self.incarnation_id,
                self.session_id,
                self.attempt_id,
            ]
            .iter()
            .any(Uuid::is_nil)
            || !Uuid::parse_str(&self.reservation_id)
                .is_ok_and(|id| !id.is_nil() && id.to_string() == self.reservation_id)
            || [
                &self.reservation_input_hash,
                &self.authority_fingerprint,
                &self.profile_hash,
                &self.preparation_sha256,
            ]
            .iter()
            .any(|s| !super::valid_digest(s))
        {
            return Err(BootstrapError::Protocol);
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Frame {
    Hello {
        version: u32,
        exchange_id: Uuid,
        binding: WireBinding,
    },
    Ready {
        version: u32,
        exchange_id: Uuid,
        binding: WireBinding,
    },
}
impl Frame {
    fn validate(&self) -> Result<()> {
        let (version, exchange, binding) = match self {
            Self::Hello {
                version,
                exchange_id,
                binding,
            }
            | Self::Ready {
                version,
                exchange_id,
                binding,
            } => (*version, exchange_id, binding),
        };
        if version != 2 || exchange.is_nil() {
            return Err(BootstrapError::Protocol);
        }
        binding.validate()
    }
}
fn encode(frame: &Frame) -> Result<Vec<u8>> {
    frame.validate()?;
    let bytes = serde_json::to_vec(frame).map_err(|_| BootstrapError::Protocol)?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        return Err(BootstrapError::FrameSize);
    }
    Ok(bytes)
}
fn decode(bytes: &[u8]) -> Result<Frame> {
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        return Err(BootstrapError::FrameSize);
    }
    let value = crate::json::decode_unique(bytes, 16).map_err(|_| BootstrapError::Protocol)?;
    let frame: Frame =
        serde_json::from_value(value.clone()).map_err(|_| BootstrapError::Protocol)?;
    frame.validate()?;
    // Reject noncanonical UUID spellings rather than normalizing peer identity.
    if serde_json::to_value(&frame).map_err(|_| BootstrapError::Protocol)? != value {
        return Err(BootstrapError::Protocol);
    }
    Ok(frame)
}
/// Length declared by one 4-byte big-endian envelope prefix. Refuses 0 and any
/// value above `MAX_FRAME_BYTES` with `FrameSize`. Allocates nothing. Every
/// reader judges a declared length here and nowhere else; `encode` and
/// `decode` keep their own checks on buffers they already hold.
pub(crate) fn declared_frame_length(prefix: [u8; 4]) -> Result<usize> {
    match usize::try_from(u32::from_be_bytes(prefix)) {
        Ok(length) if (1..=MAX_FRAME_BYTES).contains(&length) => Ok(length),
        _ => Err(BootstrapError::FrameSize),
    }
}
/// `read_frame` never told the end of a stream from another read failure.
fn incomplete_frame(_: std::io::Error) -> BootstrapError {
    BootstrapError::PartialFrame
}
/// The waiting-for-attach reader separates a stream that ends inside a frame
/// from any other read failure.
fn truncated_or_io(error: std::io::Error) -> BootstrapError {
    if error.kind() == std::io::ErrorKind::UnexpectedEof {
        BootstrapError::PartialFrame
    } else {
        BootstrapError::Io
    }
}
/// Prefix step, then the body step.
async fn read_frame<R: AsyncRead + Unpin>(input: &mut R) -> Result<Frame> {
    let mut prefix = [0u8; 4];
    input
        .read_exact(&mut prefix)
        .await
        .map_err(incomplete_frame)?;
    read_frame_body(input, prefix, incomplete_frame).await
}
/// Body step of one envelope whose prefix was already read. The declared
/// length is judged before the exact reservation, and the body is read only
/// after both. `failure` is how this reader reports a failed body read.
async fn read_frame_body<R: AsyncRead + Unpin>(
    input: &mut R,
    prefix: [u8; 4],
    failure: fn(std::io::Error) -> BootstrapError,
) -> Result<Frame> {
    let length = declared_frame_length(prefix)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| BootstrapError::Capacity)?;
    bytes.resize(length, 0);
    input.read_exact(&mut bytes).await.map_err(failure)?;
    decode(&bytes)
}
async fn write_frame<W: AsyncWrite + Unpin>(output: &mut W, frame: &Frame) -> Result<()> {
    let bytes = encode(frame)?;
    output
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await
        .map_err(|_| BootstrapError::Io)?;
    output
        .write_all(&bytes)
        .await
        .map_err(|_| BootstrapError::Io)?;
    output.flush().await.map_err(|_| BootstrapError::Io)
}
fn check_deadline(until: Instant) -> Result<()> {
    let now = Instant::now();
    if until <= now || until.duration_since(now) > MAX_HANDSHAKE_TIME {
        Err(BootstrapError::Deadline)
    } else {
        Ok(())
    }
}

/// Worker-side waiting mode. It only echoes the closed metadata preamble. A
/// valid Hello has no authority and cannot release or execute a business command.
pub(crate) async fn bootstrap_waiting<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut input: R,
    mut output: W,
    _capacity: BootstrapCapacity,
    until: Instant,
) -> Result<()> {
    check_deadline(until)?;
    timeout_at(until, async {
        match read_frame(&mut input).await? {
            Frame::Hello {
                version,
                exchange_id,
                binding,
            } => {
                write_frame(
                    &mut output,
                    &Frame::Ready {
                        version,
                        exchange_id,
                        binding,
                    },
                )
                .await
            }
            _ => Err(BootstrapError::Protocol),
        }
    })
    .await
    .map_err(|_| BootstrapError::Deadline)??;
    // Timeout polls its inner future first. Ready IO may finish in one poll
    // after the absolute deadline, so successful completion needs this fence.
    check_deadline(until)
}

/// How a waiting-for-attach exchange ended without an error. Neither value is
/// a release, a start or a session result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaitingForAttachV2 {
    /// The input ended before its first byte: no peer wrote anything.
    EofBeforeHello,
    /// One Hello was answered by one flushed Ready.
    ReadyWritten,
}

/// Worker-side waiting mode behind a Pod attach. Like `bootstrap_waiting`, it
/// only echoes the closed metadata preamble and executes nothing.
///
/// The wait for the first input byte has no local deadline: the peer attaches
/// at a time this process cannot know, and that wait is bounded by the Pod
/// lifetime and its `activeDeadlineSeconds`, not by a timer invented here. The
/// first byte arms `handshake` for the rest of the Hello frame and for the
/// Ready write, with the same post-completion fence as `bootstrap_waiting`.
///
/// `handshake` can shorten the window, never extend it: zero, or more than
/// `MAX_HANDSHAKE_TIME`, is refused with `Deadline` before any read. The
/// worker passes `MAX_HANDSHAKE_TIME`.
///
/// An input that ends before its first byte is `Ok(EofBeforeHello)`, one that
/// ends inside a frame is `PartialFrame`, and any other read failure is `Io`.
/// Nothing is read after the Hello frame: the caller keeps `input` and decides
/// what the bytes after Ready mean.
pub(crate) async fn bootstrap_waiting_for_attach<R, W>(
    input: &mut R,
    output: &mut W,
    capacity: &BootstrapCapacity,
    handshake: Duration,
) -> Result<WaitingForAttachV2>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    Ok(
        match bootstrap_waiting_for_attach_hello(input, output, capacity, handshake).await? {
            WaitingForAttachHelloV2::EofBeforeHello => WaitingForAttachV2::EofBeforeHello,
            WaitingForAttachHelloV2::ReadyWritten(_) => WaitingForAttachV2::ReadyWritten,
        },
    )
}

/// Non-secret metadata of one accepted Hello, as the peer declared it. The
/// protocol v3 supervisor compares the Handoff profile with `profile_hash`
/// and the profile binding with the identifiers. Nothing here authenticates
/// the peer, the broker or the Pod.
#[derive(Clone)]
pub(crate) struct AcceptedHelloV2 {
    binding: WireBinding,
}
impl AcceptedHelloV2 {
    pub(crate) fn profile_hash(&self) -> &str {
        &self.binding.profile_hash
    }
    pub(crate) fn authority_fingerprint(&self) -> &str {
        &self.binding.authority_fingerprint
    }
    /// installation, control namespace, broker, incarnation, session, attempt.
    pub(crate) fn ids(&self) -> [Uuid; 6] {
        [
            self.binding.installation_id,
            self.binding.control_namespace_uuid,
            self.binding.broker_id,
            self.binding.incarnation_id,
            self.binding.session_id,
            self.binding.attempt_id,
        ]
    }
}
impl std::fmt::Debug for AcceptedHelloV2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AcceptedHelloV2(declared metadata; no identity proof)")
    }
}

/// `WaitingForAttachV2` with the accepted Hello kept for the caller.
#[derive(Clone, Debug)]
pub(crate) enum WaitingForAttachHelloV2 {
    EofBeforeHello,
    ReadyWritten(AcceptedHelloV2),
}

/// `bootstrap_waiting_for_attach`, returning the accepted Hello metadata.
/// Same reads, writes, window and fence; nothing is read after the Hello.
pub(crate) async fn bootstrap_waiting_for_attach_hello<R, W>(
    input: &mut R,
    output: &mut W,
    _capacity: &BootstrapCapacity,
    handshake: Duration,
) -> Result<WaitingForAttachHelloV2>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    if handshake.is_zero() || handshake > MAX_HANDSHAKE_TIME {
        return Err(BootstrapError::Deadline);
    }
    let mut prefix = [0u8; 4];
    match input.read(&mut prefix[..1]).await {
        Ok(0) => return Ok(WaitingForAttachHelloV2::EofBeforeHello),
        Ok(_) => {}
        Err(_) => return Err(BootstrapError::Io),
    }
    let until = Instant::now() + handshake;
    let accepted = timeout_at(until, async {
        input
            .read_exact(&mut prefix[1..])
            .await
            .map_err(truncated_or_io)?;
        match read_frame_body(input, prefix, truncated_or_io).await? {
            Frame::Hello {
                version,
                exchange_id,
                binding,
            } => {
                let accepted = AcceptedHelloV2 {
                    binding: binding.clone(),
                };
                write_frame(
                    output,
                    &Frame::Ready {
                        version,
                        exchange_id,
                        binding,
                    },
                )
                .await?;
                Ok(accepted)
            }
            Frame::Ready { .. } => Err(BootstrapError::Protocol),
        }
    })
    .await
    .map_err(|_| BootstrapError::Deadline)??;
    // Same fence as `bootstrap_waiting`: the Ready IO may finish in one poll
    // after `until`.
    check_deadline(until)?;
    Ok(WaitingForAttachHelloV2::ReadyWritten(accepted))
}

struct State {
    pending: Option<JoinHandle<Result<BootstrapObservationV2>>>,
    result: Option<Result<BootstrapObservationV2>>,
    capacity: Option<BootstrapCapacity>,
}
struct Inner {
    state: Mutex<State>,
}
/// Retains the actual IO task and reservation when a caller stops awaiting it.
/// Root must retain this owner until join; Drop is never a successful join.
#[derive(Clone)]
pub(crate) struct BootstrapIoOwner(Arc<Inner>);
impl BootstrapIoOwner {
    pub(crate) fn start<R, W>(
        binding: SessionBootstrapBindingV2,
        mut input: R,
        mut output: W,
        capacity: BootstrapCapacity,
        until: Instant,
    ) -> Result<Self>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        check_deadline(until)?;
        let binding = WireBinding::from(&binding);
        binding.validate()?;
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| BootstrapError::Unavailable)?;
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                pending: None,
                result: None,
                capacity: Some(capacity),
            }),
        });
        let retained = inner.clone();
        let pending = runtime.spawn(async move {
            let exchange_id = Uuid::new_v4();
            let result = timeout_at(until, async {
                write_frame(
                    &mut output,
                    &Frame::Hello {
                        version: 2,
                        exchange_id,
                        binding: binding.clone(),
                    },
                )
                .await?;
                match read_frame(&mut input).await? {
                    Frame::Ready {
                        version: 2,
                        exchange_id: reply,
                        binding: echoed,
                    } if reply == exchange_id && echoed == binding => Ok(BootstrapObservationV2 {
                        _exchange_id: exchange_id,
                    }),
                    _ => Err(BootstrapError::Binding),
                }
            })
            .await
            .unwrap_or(Err(BootstrapError::Deadline));
            drop(input);
            drop(output);
            let result = result.and_then(|observed| {
                check_deadline(until)?;
                Ok(observed)
            });
            drop(retained);
            result
        });
        inner
            .state
            .try_lock()
            .map_err(|_| BootstrapError::Unavailable)?
            .pending = Some(pending);
        Ok(Self(inner))
    }
    pub(crate) async fn join(&self) -> Result<BootstrapObservationV2> {
        let mut state = self.0.state.lock().await;
        if let Some(result) = &state.result {
            return result.clone();
        }
        let result = state
            .pending
            .as_mut()
            .ok_or(BootstrapError::Unavailable)?
            .await
            .unwrap_or(Err(BootstrapError::Unavailable));
        state.pending.take();
        state.capacity.take();
        state.result = Some(result.clone());
        result
    }
}

#[cfg(test)]
#[path = "bootstrap_tests.rs"]
mod tests;
