//! Closed V1 channel values, with explicit send-only envelope V2 opt-in.
//! The V2 exchange carries the same V1 business request and authority.
use super::{ChannelError, FrameLimit};
use crate::relay::{
    MAX_INBOX_BYTES, RelayAckV1, RelayInboxV1, RelayReceiptV1, RelaySendRefusalV1, RelaySendV1,
    validation,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, value::RawValue};

pub const MAX_REQUEST_FRAME_BYTES: usize = 278_528;
pub const MAX_RESPONSE_FRAME_BYTES: usize = 2_113_536;
pub const MAX_CONTROL_FRAME_BYTES: usize = 16_384;
pub const MAX_CORRELATIONS: usize = 1024;
// Bound JSON container/field overhead separately from encoded bytes. All V1
// closed DTOs (including 32 inbox items) fit well below this ceiling. Business
// body strings do not consume structural tokens inside the frame.
const MAX_JSON_STRUCTURE_TOKENS: usize = 16_384;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelLimitsV1 {
    pub request_frame_bytes: u32,
    pub response_frame_bytes: u32,
    pub control_frame_bytes: u32,
    pub correlations: u32,
}
impl Default for ChannelLimitsV1 {
    fn default() -> Self {
        Self {
            request_frame_bytes: MAX_REQUEST_FRAME_BYTES as u32,
            response_frame_bytes: MAX_RESPONSE_FRAME_BYTES as u32,
            control_frame_bytes: MAX_CONTROL_FRAME_BYTES as u32,
            correlations: MAX_CORRELATIONS as u32,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandshakeV1 {
    pub version: u32,
    pub channel_id: String,
    pub epoch: u64,
    pub limits: ChannelLimitsV1,
    pub helper_version: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationV1 {
    Send,
    Inbox,
    Ack,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxRequestV1 {
    pub after: Option<i64>,
    pub limit: u32,
}

// Request/response types deliberately omit Debug: business bodies are private.
#[derive(Clone)]
pub enum RequestPayloadV1 {
    Send(RelaySendV1),
    Inbox(InboxRequestV1),
    Ack(RelayAckV1),
}
impl RequestPayloadV1 {
    pub fn operation(&self) -> OperationV1 {
        match self {
            Self::Send(_) => OperationV1::Send,
            Self::Inbox(_) => OperationV1::Inbox,
            Self::Ack(_) => OperationV1::Ack,
        }
    }

    fn value(&self) -> Result<Value, ChannelError> {
        match self {
            Self::Send(value) => to_value(value),
            Self::Inbox(value) => to_value(value),
            Self::Ack(value) => to_value(value),
        }
    }
}

#[derive(Clone)]
pub struct RequestV1 {
    pub version: u32,
    pub channel_id: String,
    pub epoch: u64,
    pub correlation_id: String,
    pub payload: RequestPayloadV1,
}
impl RequestV1 {
    pub fn operation(&self) -> OperationV1 {
        self.payload.operation()
    }

    pub(crate) fn validate(&self) -> Result<(), ChannelError> {
        exchange_header(self.version, &self.channel_id, self.epoch, self.operation())?;
        correlation(&self.correlation_id)?;
        let result = match &self.payload {
            RequestPayloadV1::Send(input) => {
                validation::validate_send(input, None)?;
                validation::request_bytes(input, None)
            }
            RequestPayloadV1::Inbox(input) => {
                validation::validate_inbox_request(input.after, input.limit)?;
                validation::request_bytes(input, None)
            }
            RequestPayloadV1::Ack(input) => {
                validation::validate_ack(input)?;
                validation::request_bytes(input, None)
            }
        };
        result.map(|_| ()).map_err(|_| ChannelError::BusinessSize)
    }
}

#[derive(Clone)]
pub enum ReplyV1 {
    Send(RelayReceiptV1),
    Inbox(RelayInboxV1),
    Ack(RelayReceiptV1),
}
impl ReplyV1 {
    fn operation(&self) -> OperationV1 {
        match self {
            Self::Send(_) => OperationV1::Send,
            Self::Inbox(_) => OperationV1::Inbox,
            Self::Ack(_) => OperationV1::Ack,
        }
    }

    fn value(&self) -> Result<Value, ChannelError> {
        match self {
            Self::Send(value) | Self::Ack(value) => to_value(value),
            Self::Inbox(value) => to_value(value),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCodeV1 {
    InvalidRequest,
    Authentication,
    Forbidden,
    NotFound,
    Conflict,
    Capacity,
    Unavailable,
    Transport,
    Protocol,
}
impl ErrorCodeV1 {
    /// Only the current attempt is described; false is never proof of rollback.
    pub fn outcome_unknown(self) -> bool {
        matches!(self, Self::Unavailable | Self::Transport | Self::Protocol)
    }
}

#[derive(Clone)]
pub enum ResponseResultV1 {
    Ok(ReplyV1),
    Error(ErrorCodeV1),
    /// Only an explicitly opted-in V2 send may carry this capacity detail.
    SendRefusal(RelaySendRefusalV1),
}

#[derive(Clone)]
pub struct ResponseV1 {
    pub version: u32,
    pub channel_id: String,
    pub epoch: u64,
    pub correlation_id: String,
    pub operation: OperationV1,
    pub result: ResponseResultV1,
}
impl ResponseV1 {
    pub(super) fn validate(&self) -> Result<(), ChannelError> {
        exchange_header(self.version, &self.channel_id, self.epoch, self.operation)?;
        correlation(&self.correlation_id)?;
        if let ResponseResultV1::SendRefusal(refusal) = &self.result
            && (self.version != 2 || self.operation != OperationV1::Send || !refusal.valid())
        {
            return Err(ChannelError::Protocol);
        }
        if let ResponseResultV1::Ok(reply) = &self.result {
            if reply.operation() != self.operation {
                return Err(ChannelError::Correlation);
            }
            match reply {
                ReplyV1::Send(receipt) | ReplyV1::Ack(receipt) => {
                    if !validation::uuid(&receipt.operation_id)
                        || !validation::uuid(&receipt.resource_id)
                    {
                        return Err(ChannelError::Protocol);
                    }
                    validation::validate_receipt(
                        receipt,
                        &receipt.operation_id,
                        &receipt.resource_id,
                    )?;
                }
                ReplyV1::Inbox(inbox) => {
                    if inbox.version != 1 || inbox.items.len() > 32 {
                        return Err(ChannelError::Protocol);
                    }
                }
            }
            bounded_value(&reply.value()?, business_response_limit(self.operation))?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseReasonV1 {
    Shutdown,
    Revoked,
    Expired,
    AuditUnavailable,
    IdentityChanged,
    Capacity,
    Protocol,
    Timeout,
    Transport,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseV1 {
    pub version: u32,
    pub channel_id: String,
    pub epoch: u64,
    pub reason: CloseReasonV1,
}

#[derive(Clone)]
pub enum FrameV1 {
    Hello(HandshakeV1),
    Ready(HandshakeV1),
    Request(RequestV1),
    Response(ResponseV1),
    Close(CloseV1),
}
impl FrameV1 {
    pub(crate) fn maximum(&self) -> usize {
        match self {
            Self::Request(_) => MAX_REQUEST_FRAME_BYTES,
            Self::Response(ResponseV1 {
                result: ResponseResultV1::Ok(_),
                ..
            }) => MAX_RESPONSE_FRAME_BYTES,
            _ => MAX_CONTROL_FRAME_BYTES,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), ChannelError> {
        match self {
            Self::Hello(value) | Self::Ready(value) => value.validate(),
            Self::Request(value) => value.validate(),
            Self::Response(value) => value.validate(),
            Self::Close(value) => header(value.version, &value.channel_id, value.epoch),
        }
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, ChannelError> {
        self.validate()?;
        let mut object = Map::new();
        let (version, channel, epoch, kind) = match self {
            Self::Hello(value) => (value.version, &value.channel_id, value.epoch, "hello"),
            Self::Ready(value) => (value.version, &value.channel_id, value.epoch, "ready"),
            Self::Request(value) => (value.version, &value.channel_id, value.epoch, "request"),
            Self::Response(value) => (value.version, &value.channel_id, value.epoch, "response"),
            Self::Close(value) => (value.version, &value.channel_id, value.epoch, "close"),
        };
        put(&mut object, "version", &version)?;
        put(&mut object, "type", &kind)?;
        put(&mut object, "channel_id", channel)?;
        put(&mut object, "epoch", &epoch)?;
        match self {
            Self::Hello(value) | Self::Ready(value) => {
                put(&mut object, "limits", &value.limits)?;
                put(&mut object, "helper_version", &value.helper_version)?;
            }
            Self::Request(value) => {
                put(&mut object, "correlation_id", &value.correlation_id)?;
                put(&mut object, "operation", &value.operation())?;
                object.insert("payload".into(), value.payload.value()?);
            }
            Self::Response(value) => {
                put(&mut object, "correlation_id", &value.correlation_id)?;
                put(&mut object, "operation", &value.operation)?;
                match &value.result {
                    ResponseResultV1::Ok(reply) => {
                        put(&mut object, "result", "ok")?;
                        object.insert("payload".into(), reply.value()?);
                    }
                    ResponseResultV1::Error(code) => {
                        put(&mut object, "result", "error")?;
                        put(&mut object, "error", code)?;
                    }
                    ResponseResultV1::SendRefusal(refusal) => {
                        put(&mut object, "result", "error")?;
                        put(&mut object, "error", &ErrorCodeV1::Capacity)?;
                        put(&mut object, "refusal", refusal)?;
                    }
                }
            }
            Self::Close(value) => put(&mut object, "reason", &value.reason)?,
        }
        let bytes = serde_json::to_vec(&object).map_err(|_| ChannelError::Protocol)?;
        if bytes.len() > self.maximum() {
            return Err(ChannelError::FrameSize);
        }
        Ok(bytes)
    }

    pub(crate) fn decode(bytes: &[u8], limit: FrameLimit) -> Result<Self, ChannelError> {
        if std::str::from_utf8(bytes).is_err() {
            return Err(ChannelError::Protocol);
        }
        bounded_json_structure(bytes)?;
        let Value::Object(mut object) =
            crate::json::decode_unique(bytes, 64).map_err(|_| ChannelError::Protocol)?
        else {
            return Err(ChannelError::Protocol);
        };
        let version = take(&mut object, "version")?;
        let channel_id = take(&mut object, "channel_id")?;
        let epoch = take(&mut object, "epoch")?;
        let kind: String = take(&mut object, "type")?;
        let frame = match kind.as_str() {
            "hello" | "ready" => {
                let value = HandshakeV1 {
                    version,
                    channel_id,
                    epoch,
                    limits: take(&mut object, "limits")?,
                    helper_version: take(&mut object, "helper_version")?,
                };
                if kind == "hello" {
                    Self::Hello(value)
                } else {
                    Self::Ready(value)
                }
            }
            "request" => {
                let correlation_id = take(&mut object, "correlation_id")?;
                let operation: OperationV1 = take(&mut object, "operation")?;
                payload_bound(bytes, validation::MAX_REQUEST_BYTES)?;
                let payload = remove(&mut object, "payload")?;
                let payload = match operation {
                    OperationV1::Send => RequestPayloadV1::Send(exact(payload)?),
                    OperationV1::Inbox => RequestPayloadV1::Inbox(exact(payload)?),
                    OperationV1::Ack => RequestPayloadV1::Ack(exact(payload)?),
                };
                Self::Request(RequestV1 {
                    version,
                    channel_id,
                    epoch,
                    correlation_id,
                    payload,
                })
            }
            "response" => {
                let correlation_id = take(&mut object, "correlation_id")?;
                let operation: OperationV1 = take(&mut object, "operation")?;
                let result: String = take(&mut object, "result")?;
                let result = match result.as_str() {
                    "ok" => {
                        payload_bound(bytes, business_response_limit(operation))?;
                        let payload = remove(&mut object, "payload")?;
                        ResponseResultV1::Ok(match operation {
                            OperationV1::Send => ReplyV1::Send(exact(payload)?),
                            OperationV1::Inbox => ReplyV1::Inbox(exact(payload)?),
                            OperationV1::Ack => ReplyV1::Ack(exact(payload)?),
                        })
                    }
                    "error" => {
                        let code: ErrorCodeV1 = take(&mut object, "error")?;
                        if let Some(detail) = object.remove("refusal") {
                            if version != 2
                                || operation != OperationV1::Send
                                || code != ErrorCodeV1::Capacity
                            {
                                return Err(ChannelError::Protocol);
                            }
                            ResponseResultV1::SendRefusal(exact(detail)?)
                        } else {
                            ResponseResultV1::Error(code)
                        }
                    }
                    _ => return Err(ChannelError::Protocol),
                };
                Self::Response(ResponseV1 {
                    version,
                    channel_id,
                    epoch,
                    correlation_id,
                    operation,
                    result,
                })
            }
            "close" => Self::Close(CloseV1 {
                version,
                channel_id,
                epoch,
                reason: take(&mut object, "reason")?,
            }),
            _ => return Err(ChannelError::Protocol),
        };
        if !object.is_empty() || !limit.accepts(&frame) {
            return Err(ChannelError::Protocol);
        }
        if bytes.len() > frame.maximum() {
            return Err(ChannelError::FrameSize);
        }
        frame.validate()?;
        Ok(frame)
    }
}

impl HandshakeV1 {
    pub(crate) fn validate(&self) -> Result<(), ChannelError> {
        header(self.version, &self.channel_id, self.epoch)?;
        // V1 has fixed resolved limits. A future negotiated profile needs a
        // separately versioned contract, never an unverified raised bound.
        if self.helper_version != 1 || self.limits != ChannelLimitsV1::default() {
            return Err(ChannelError::Protocol);
        }
        Ok(())
    }
}

pub(crate) fn header(version: u32, channel: &str, epoch: u64) -> Result<(), ChannelError> {
    if version != 1 || epoch == 0 || !canonical_non_nil_uuid(channel) {
        Err(ChannelError::Protocol)
    } else {
        Ok(())
    }
}

// Deliberately separate from channel/discovery headers: V2 is not a handshake,
// a new identity, or permission to send V2 inbox/ACK payloads.
fn exchange_header(
    version: u32,
    channel: &str,
    epoch: u64,
    operation: OperationV1,
) -> Result<(), ChannelError> {
    if version == 1 || version == 2 && operation == OperationV1::Send {
        header(1, channel, epoch)
    } else {
        Err(ChannelError::Protocol)
    }
}

fn correlation(value: &str) -> Result<(), ChannelError> {
    if canonical_non_nil_uuid(value) {
        Ok(())
    } else {
        Err(ChannelError::Correlation)
    }
}

fn canonical_non_nil_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}

fn business_response_limit(operation: OperationV1) -> usize {
    match operation {
        OperationV1::Inbox => MAX_INBOX_BYTES,
        _ => validation::MAX_RECEIPT_BYTES,
    }
}

fn bounded_value(value: &Value, maximum: usize) -> Result<(), ChannelError> {
    if serde_json::to_vec(value)
        .map_err(|_| ChannelError::Protocol)?
        .len()
        > maximum
    {
        Err(ChannelError::BusinessSize)
    } else {
        Ok(())
    }
}

fn payload_bound(bytes: &[u8], maximum: usize) -> Result<(), ChannelError> {
    #[derive(Deserialize)]
    struct Payload<'a> {
        #[serde(borrow)]
        payload: &'a RawValue,
    }
    let raw: Payload<'_> = serde_json::from_slice(bytes).map_err(|_| ChannelError::Protocol)?;
    if raw.payload.get().len() > maximum {
        Err(ChannelError::BusinessSize)
    } else {
        Ok(())
    }
}

fn bounded_json_structure(bytes: &[u8]) -> Result<(), ChannelError> {
    let mut quoted = false;
    let mut escaped = false;
    let mut tokens = 0_usize;
    for byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                quoted = false;
            }
        } else if *byte == b'"' {
            quoted = true;
        } else if matches!(*byte, b'{' | b'[' | b',' | b':') {
            tokens += 1;
            if tokens > MAX_JSON_STRUCTURE_TOKENS {
                return Err(ChannelError::Capacity);
            }
        }
    }
    Ok(())
}

fn remove(object: &mut Map<String, Value>, key: &str) -> Result<Value, ChannelError> {
    object.remove(key).ok_or(ChannelError::Protocol)
}

fn take<T: DeserializeOwned>(
    object: &mut Map<String, Value>,
    key: &str,
) -> Result<T, ChannelError> {
    serde_json::from_value(remove(object, key)?).map_err(|_| ChannelError::Protocol)
}

fn exact<T: DeserializeOwned + Serialize>(value: Value) -> Result<T, ChannelError> {
    let decoded: T = serde_json::from_value(value.clone()).map_err(|_| ChannelError::Protocol)?;
    if to_value(&decoded)? != value {
        return Err(ChannelError::Protocol);
    }
    Ok(decoded)
}

fn to_value<T: Serialize + ?Sized>(value: &T) -> Result<Value, ChannelError> {
    serde_json::to_value(value).map_err(|_| ChannelError::Protocol)
}

fn put<T: Serialize + ?Sized>(
    object: &mut Map<String, Value>,
    key: &str,
    value: &T,
) -> Result<(), ChannelError> {
    object.insert(key.into(), to_value(value)?);
    Ok(())
}
