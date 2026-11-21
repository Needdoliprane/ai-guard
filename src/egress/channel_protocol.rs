//! Broker-owned egress pipe framing (`broker_proxy_v1`). These values are untrusted data;
//! echoing the challenge proves neither workload identity nor network authority.
//! This framing is never selected from a historical relay token or frame.

use super::ids::{CHANNEL_CONTRACT, DigestV1, IdV1};
use super::protocol::{EgressCapabilityV1, EgressMessageV1, MAX_EGRESS_FRAME_BYTES};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    time::{Instant, timeout_at},
};

pub const MAX_HANDSHAKE_BYTES: usize = 4_096;
pub const HANDSHAKE_TIME: Duration = Duration::from_secs(5);
pub const FRAME_TIME: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommonEgressChannelError {
    Protocol,
    Size,
    Capacity,
    PartialFrame,
    Closed,
    Io,
    Timeout,
}
impl std::fmt::Display for CommonEgressChannelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Protocol => "common egress frame is incompatible",
            Self::Size => "common egress frame exceeds its bound",
            Self::Capacity => "common egress frame capacity is unavailable",
            Self::PartialFrame => "common egress frame is incomplete",
            Self::Closed => "common egress channel is closed",
            Self::Io => "common egress channel IO failed",
            Self::Timeout => "common egress channel deadline expired",
        })
    }
}
impl std::error::Error for CommonEgressChannelError {}

/// HELLO from the retained broker pipe, then an exact READY echo on that pipe.
/// The direction is defined by the explicit mode and caller state, not a field.
/// The trusted owner must independently inspect its retained runtime instance
/// before/after the echo and durably admit Attached before admitting any Open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommonEgressHandshakeV1 {
    pub version: u32,
    pub protocol: String,
    pub claim_id: IdV1,
    pub binding_id: IdV1,
    pub transport_epoch: u64,
    pub context_digest: DigestV1,
    pub challenge: IdV1,
    pub helper_version: u32,
    pub capability: EgressCapabilityV1,
}
impl CommonEgressHandshakeV1 {
    pub fn validate(&self) -> Result<(), CommonEgressChannelError> {
        if self.version != 1
            || self.protocol != CHANNEL_CONTRACT
            || self.helper_version != 1
            || self.transport_epoch == 0
            || self.transport_epoch > i64::MAX as u64
        {
            return Err(CommonEgressChannelError::Protocol);
        }
        self.capability
            .validate()
            .map_err(|_| CommonEgressChannelError::Protocol)
    }
}

/// The caller supplies one absolute deadline retained across HELLO and READY.
/// No successful fragment, parse or echo grants a new handshake time budget.
pub async fn read_handshake<R: AsyncRead + Unpin>(
    input: &mut R,
    deadline: Instant,
) -> Result<CommonEgressHandshakeV1, CommonEgressChannelError> {
    let mut prefix = [0_u8; 4];
    read_exact_before(input, &mut prefix, deadline).await?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_HANDSHAKE_BYTES {
        return Err(CommonEgressChannelError::Size);
    }
    // A fixed scratch buffer also bounds malformed/unknown JSON before Serde.
    let mut bytes = [0_u8; MAX_HANDSHAKE_BYTES];
    read_exact_before(input, &mut bytes[..length], deadline).await?;
    let hello: CommonEgressHandshakeV1 =
        serde_json::from_slice(&bytes[..length]).map_err(|_| CommonEgressChannelError::Protocol)?;
    hello.validate()?;
    if Instant::now() >= deadline {
        return Err(CommonEgressChannelError::Timeout);
    }
    Ok(hello)
}

/// Retain this future until completion, or close the channel after abandoning
/// it. Cancellation can leave a partial prefix/body on the underlying pipe.
pub async fn write_handshake<W: AsyncWrite + Unpin>(
    output: &mut W,
    handshake: &CommonEgressHandshakeV1,
    deadline: Instant,
) -> Result<(), CommonEgressChannelError> {
    handshake.validate()?;
    let bytes = serde_json::to_vec(handshake).map_err(|_| CommonEgressChannelError::Protocol)?;
    if bytes.is_empty() || bytes.len() > MAX_HANDSHAKE_BYTES {
        return Err(CommonEgressChannelError::Size);
    }
    write_payload(output, &bytes, deadline).await
}

/// Cancellation-safe reader: completed fragment offsets and the original
/// deadline remain owned across select branches. Size precedes body allocation.
pub struct EgressFrameReader {
    prefix: [u8; 4],
    prefix_used: usize,
    body: Vec<u8>,
    body_used: usize,
    deadline: Option<Instant>,
    closed: bool,
}
impl Default for EgressFrameReader {
    fn default() -> Self {
        Self::new()
    }
}
impl EgressFrameReader {
    pub fn new() -> Self {
        Self {
            prefix: [0; 4],
            prefix_used: 0,
            body: Vec::new(),
            body_used: 0,
            deadline: None,
            closed: false,
        }
    }

    pub async fn read<R: AsyncRead + Unpin>(
        &mut self,
        input: &mut R,
    ) -> Result<Option<EgressMessageV1>, CommonEgressChannelError> {
        if self.closed {
            return Err(CommonEgressChannelError::Closed);
        }
        let result = self.read_inner(input).await;
        if !matches!(result, Ok(Some(_))) {
            self.closed = true;
        }
        result
    }

    async fn read_inner<R: AsyncRead + Unpin>(
        &mut self,
        input: &mut R,
    ) -> Result<Option<EgressMessageV1>, CommonEgressChannelError> {
        while self.prefix_used < self.prefix.len() {
            let count =
                read_piece(input, &mut self.prefix[self.prefix_used..], self.deadline).await?;
            if count == 0 {
                return if self.prefix_used == 0 {
                    Ok(None)
                } else {
                    Err(CommonEgressChannelError::PartialFrame)
                };
            }
            self.deadline.get_or_insert(Instant::now() + FRAME_TIME);
            self.prefix_used += count;
        }
        if self.body.is_empty() {
            let length = u32::from_be_bytes(self.prefix) as usize;
            if length == 0 || length > MAX_EGRESS_FRAME_BYTES {
                return Err(CommonEgressChannelError::Size);
            }
            self.body
                .try_reserve_exact(length)
                .map_err(|_| CommonEgressChannelError::Capacity)?;
            self.body.resize(length, 0);
        }
        while self.body_used < self.body.len() {
            let count = read_piece(input, &mut self.body[self.body_used..], self.deadline).await?;
            if count == 0 {
                return Err(CommonEgressChannelError::PartialFrame);
            }
            self.body_used += count;
        }
        let message = EgressMessageV1::decode_payload(&self.body)
            .map_err(|_| CommonEgressChannelError::Protocol)?;
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(CommonEgressChannelError::Timeout);
        }
        *self = Self::new();
        Ok(Some(message))
    }
}

/// Retain this future until completion, or close the channel after abandoning
/// it. Cancellation can leave a partial prefix/body on the underlying pipe.
pub async fn write_message<W: AsyncWrite + Unpin>(
    output: &mut W,
    message: &EgressMessageV1,
    deadline: Instant,
) -> Result<(), CommonEgressChannelError> {
    let bytes = message
        .encode_payload()
        .map_err(|_| CommonEgressChannelError::Protocol)?;
    write_payload(output, &bytes, deadline).await
}

async fn write_payload<W: AsyncWrite + Unpin>(
    output: &mut W,
    bytes: &[u8],
    deadline: Instant,
) -> Result<(), CommonEgressChannelError> {
    if Instant::now() >= deadline {
        return Err(CommonEgressChannelError::Timeout);
    }
    let length = u32::try_from(bytes.len()).map_err(|_| CommonEgressChannelError::Size)?;
    timeout_at(deadline, async {
        output.write_all(&length.to_be_bytes()).await?;
        output.write_all(bytes).await?;
        output.flush().await
    })
    .await
    .map_err(|_| CommonEgressChannelError::Timeout)?
    .map_err(|_| CommonEgressChannelError::Io)?;
    if Instant::now() >= deadline {
        return Err(CommonEgressChannelError::Timeout);
    }
    Ok(())
}

async fn read_exact_before<R: AsyncRead + Unpin>(
    input: &mut R,
    bytes: &mut [u8],
    deadline: Instant,
) -> Result<(), CommonEgressChannelError> {
    let mut used = 0;
    while used < bytes.len() {
        let count = read_piece(input, &mut bytes[used..], Some(deadline)).await?;
        if count == 0 {
            return Err(CommonEgressChannelError::PartialFrame);
        }
        used += count;
    }
    Ok(())
}

async fn read_piece<R: AsyncRead + Unpin>(
    input: &mut R,
    bytes: &mut [u8],
    deadline: Option<Instant>,
) -> Result<usize, CommonEgressChannelError> {
    match deadline {
        Some(deadline) => {
            if Instant::now() >= deadline {
                return Err(CommonEgressChannelError::Timeout);
            }
            timeout_at(deadline, input.read(bytes))
                .await
                .map_err(|_| CommonEgressChannelError::Timeout)?
                .map_err(|_| CommonEgressChannelError::Io)
        }
        None => input
            .read(bytes)
            .await
            .map_err(|_| CommonEgressChannelError::Io),
    }
}

#[cfg(test)]
mod tests;
