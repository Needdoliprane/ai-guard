//! V2 waiting-bootstrap preamble, using the relay's four-byte BE envelope.
//! Attach identity is NOT established by an echo. There is no Release/Start,
//! credential handoff or command execution in this stage of the protocol.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "V2 waiting-bootstrap preamble without a production caller until stage S2b (Root decisions Q1 and Q6)"
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
const MAX_HANDSHAKE_TIME: Duration = Duration::from_secs(5);
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
async fn read_frame<R: AsyncRead + Unpin>(input: &mut R) -> Result<Frame> {
    let mut prefix = [0u8; 4];
    input
        .read_exact(&mut prefix)
        .await
        .map_err(|_| BootstrapError::PartialFrame)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(BootstrapError::FrameSize);
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| BootstrapError::Capacity)?;
    bytes.resize(length, 0);
    input
        .read_exact(&mut bytes)
        .await
        .map_err(|_| BootstrapError::PartialFrame)?;
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
