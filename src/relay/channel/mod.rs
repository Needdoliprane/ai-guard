//! Software-only private relay framing and exchange validation.
//!
//! This module neither opens a worker channel nor authenticates a worker. The
//! future transport owner must retain the authenticated endpoint, deadlines,
//! memory reservation and runtime authority outside these untrusted values.
//! A validated request is not a dispatch permit or evidence of a durable audit.
mod codec;
// No production admission factory exists until the retained runtime permit,
// RoleLease and mandatory audit reservation are connected and reviewed.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "private control owner awaits concrete runtime admission"
    )
)]
mod control;
pub mod helper;
mod protocol;

pub use codec::{FrameLimit, FrameReader, FrameWriter};
pub use protocol::{
    ChannelLimitsV1, CloseReasonV1, CloseV1, ErrorCodeV1, FrameV1, HandshakeV1, InboxRequestV1,
    MAX_CONTROL_FRAME_BYTES, MAX_CORRELATIONS, MAX_REQUEST_FRAME_BYTES, MAX_RESPONSE_FRAME_BYTES,
    OperationV1, ReplyV1, RequestPayloadV1, RequestV1, ResponseResultV1, ResponseV1,
};

use super::{RelayEndpointV1, client::RelayClientError, validation};
use std::{collections::HashSet, io::Read};

/// Diagnostics never retain source bytes, bodies, URLs or underlying IO errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelError {
    Closed,
    Io,
    PartialFrame,
    FrameSize,
    BusinessSize,
    Protocol,
    Binding,
    Correlation,
    Busy,
    Capacity,
}
impl std::fmt::Display for ChannelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Closed => "relay channel is closed",
            Self::Io => "relay channel IO failed",
            Self::PartialFrame => "relay frame is incomplete",
            Self::FrameSize => "relay frame length is outside its bound",
            Self::BusinessSize => "relay business payload exceeds its own bound",
            Self::Protocol => "relay frame is incompatible",
            Self::Binding => "relay channel binding does not match",
            Self::Correlation => "relay exchange correlation does not match",
            Self::Busy => "relay channel already has a pending exchange",
            Self::Capacity => "relay channel correlation capacity is exhausted",
        })
    }
}
impl std::error::Error for ChannelError {}
impl From<RelayClientError> for ChannelError {
    fn from(_: RelayClientError) -> Self {
        Self::Protocol
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitingReady,
    Open,
    Closed,
}

struct Pending {
    version: u32,
    correlation: String,
    operation: OperationV1,
    payload: PendingPayload,
}
enum PendingPayload {
    Send(String),
    Inbox(InboxRequestV1),
    Ack { operation: String, message: String },
}

/// Local anti-replay state for one epoch, with one in-flight exchange.
///
/// The endpoint comes from trusted control, never a frame. Closing preserves
/// pending state: a lost response never proves the remote mutation rolled back.
/// This in-memory ledger is not the durable mutation outbox.
pub struct ChannelState {
    hello: HandshakeV1,
    endpoint: RelayEndpointV1,
    phase: Phase,
    seen: HashSet<String>,
    pending: Option<Pending>,
}
impl ChannelState {
    pub fn new(hello: HandshakeV1, endpoint: RelayEndpointV1) -> Result<Self, ChannelError> {
        hello.validate()?;
        endpoint.validate().map_err(|_| ChannelError::Binding)?;
        Ok(Self {
            hello,
            endpoint,
            phase: Phase::AwaitingReady,
            seen: HashSet::new(),
            pending: None,
        })
    }

    pub fn accept_ready(&mut self, ready: &HandshakeV1) -> Result<(), ChannelError> {
        let result = if self.phase == Phase::Closed {
            Err(ChannelError::Closed)
        } else if self.phase != Phase::AwaitingReady || ready != &self.hello {
            Err(ChannelError::Binding)
        } else {
            self.phase = Phase::Open;
            Ok(())
        };
        self.close_on_error(result)
    }

    /// Check capacity before reading any request body. The runtime owner must
    /// also avoid reading while its output or byte reservation is unavailable.
    /// A second read during a pending exchange closes admission without a
    /// correlation or a response for the unread request.
    pub fn read_request<R: Read>(
        &mut self,
        reader: &mut FrameReader<R>,
    ) -> Result<Option<RequestV1>, ChannelError> {
        if let Err(error) = self.request_capacity() {
            self.close();
            reader.close();
            return Err(error);
        }
        let frame = match reader.read(FrameLimit::Request) {
            Ok(Some(FrameV1::Request(value))) => value,
            Ok(None) => {
                self.close();
                return Ok(None);
            }
            Ok(Some(_)) => {
                self.close();
                reader.close();
                return Err(ChannelError::Protocol);
            }
            Err(error) => {
                self.close();
                return Err(error);
            }
        };
        if let Err(error) = self.admit_request(&frame) {
            reader.close();
            return Err(error);
        }
        Ok(Some(frame))
    }

    /// Validate an already decoded request without authorizing any IO/effect.
    pub fn admit_request(&mut self, request: &RequestV1) -> Result<(), ChannelError> {
        let result = self.admit_inner(request);
        self.close_on_error(result)
    }

    fn admit_inner(&mut self, request: &RequestV1) -> Result<(), ChannelError> {
        self.request_capacity()?;
        request.validate()?;
        self.binding(&request.channel_id, request.epoch)?;
        if self.seen.contains(&request.correlation_id) {
            return Err(ChannelError::Correlation);
        }
        let payload = match &request.payload {
            RequestPayloadV1::Send(value) => PendingPayload::Send(value.operation_id.clone()),
            RequestPayloadV1::Inbox(value) => PendingPayload::Inbox(value.clone()),
            RequestPayloadV1::Ack(value) => PendingPayload::Ack {
                operation: value.operation_id.clone(),
                message: value.message_id.clone(),
            },
        };
        self.seen.insert(request.correlation_id.clone());
        self.pending = Some(Pending {
            version: request.version,
            correlation: request.correlation_id.clone(),
            operation: request.operation(),
            payload,
        });
        Ok(())
    }

    /// Validate before forwarding a response to the local application. A valid
    /// receipt confirms Store acceptance only, never recipient execution.
    pub fn accept_response(&mut self, response: &ResponseV1) -> Result<(), ChannelError> {
        let result = self.response_inner(response);
        self.close_on_error(result)
    }

    fn response_inner(&mut self, response: &ResponseV1) -> Result<(), ChannelError> {
        if self.phase != Phase::Open {
            return Err(ChannelError::Closed);
        }
        response.validate()?;
        self.binding(&response.channel_id, response.epoch)?;
        let pending = self.pending.as_ref().ok_or(ChannelError::Correlation)?;
        if response.version != pending.version
            || response.correlation_id != pending.correlation
            || response.operation != pending.operation
        {
            return Err(ChannelError::Correlation);
        }
        if let ResponseResultV1::Ok(reply) = &response.result {
            match (&pending.payload, reply) {
                (PendingPayload::Send(operation), ReplyV1::Send(receipt)) => {
                    validation::validate_receipt(receipt, operation, operation)?;
                }
                (PendingPayload::Ack { operation, message }, ReplyV1::Ack(receipt)) => {
                    validation::validate_receipt(receipt, operation, message)?;
                }
                (PendingPayload::Inbox(input), ReplyV1::Inbox(inbox)) => {
                    validation::validate_inbox(inbox, &self.endpoint, input.after, input.limit)?;
                }
                _ => return Err(ChannelError::Correlation),
            }
        }
        self.pending = None;
        Ok(())
    }

    fn request_capacity(&self) -> Result<(), ChannelError> {
        if self.phase != Phase::Open {
            Err(ChannelError::Closed)
        } else if self.pending.is_some() {
            Err(ChannelError::Busy)
        } else if self.seen.len() >= MAX_CORRELATIONS {
            Err(ChannelError::Capacity)
        } else {
            Ok(())
        }
    }

    fn binding(&self, channel: &str, epoch: u64) -> Result<(), ChannelError> {
        if channel != self.hello.channel_id || epoch != self.hello.epoch {
            Err(ChannelError::Binding)
        } else {
            Ok(())
        }
    }

    pub fn accept_close(&mut self, close: &CloseV1) -> Result<(), ChannelError> {
        let result = if self.phase == Phase::Closed {
            Err(ChannelError::Closed)
        } else {
            protocol::header(close.version, &close.channel_id, close.epoch)
                .and_then(|()| self.binding(&close.channel_id, close.epoch))
        };
        self.close();
        result
    }

    fn close_on_error(&mut self, result: Result<(), ChannelError>) -> Result<(), ChannelError> {
        if result.is_err() {
            self.close();
        }
        result
    }

    pub fn close(&mut self) {
        self.phase = Phase::Closed;
    }

    pub fn is_closed(&self) -> bool {
        self.phase == Phase::Closed
    }

    pub fn has_pending_exchange(&self) -> bool {
        self.pending.is_some()
    }
}

#[cfg(test)]
mod tests;
