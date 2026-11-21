//! Closed egress payloads and bounded, software-only channel state.
//!
//! These payloads use the existing relay channel framing through an explicitly
//! versioned integration port. There is no reader, writer, socket, resolver,
//! task, trusted identity or durable admission in this module. The transport
//! owner must retain the authenticated binding, epoch and mandatory audit
//! outside these worker-controlled values. Accepted is a wire transition,
//! never a factory for a broker connect capability.

use super::Authority;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

pub const MAX_DATA_BYTES: usize = 16_384;
pub const MAX_EGRESS_FRAME_BYTES: usize = 24_576;
pub const MAX_EGRESS_CONTROL_BYTES: usize = 2_048;
const MAX_AUTHORITY_BYTES: usize = 512;
const MAX_ENCODED_DATA_BYTES: usize = MAX_DATA_BYTES.div_ceil(3) * 4;
const MAX_FLOWS: usize = 16;
const MAX_PENDING: usize = 4;
const MAX_CREDIT_BYTES: usize = 262_144;
const MAX_BUFFER_BYTES: usize = 1_048_576;
const HANDSHAKE_MS: u64 = 5_000;
const OPEN_MS: u64 = 10_000;
const IDLE_MS: u64 = 60_000;
const OPEN_TOKENS: u64 = 8_000;
const OPEN_COST: u64 = 1_000;
const REFILL_PER_MS: u64 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EgressProtocolError {
    Closed,
    Capability,
    Protocol,
    Capacity,
    Size,
    Credit,
    State,
    Timeout,
    Clock,
}
impl std::fmt::Display for EgressProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Closed => "egress channel is closed",
            Self::Capability => "egress capability is absent or incompatible",
            Self::Protocol => "egress payload is incompatible",
            Self::Capacity => "egress channel capacity is exhausted",
            Self::Size => "egress payload exceeds its bound",
            Self::Credit => "egress direction has insufficient credit",
            Self::State => "egress flow is not in the required state",
            Self::Timeout => "egress channel deadline expired",
            Self::Clock => "egress monotone clock moved backwards",
        })
    }
}
impl std::error::Error for EgressProtocolError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressLimitsV1 {
    pub data_bytes: u32,
    pub frame_bytes: u32,
    pub control_frame_bytes: u32,
    pub live_flows: u32,
    pub pending_flows: u32,
    pub credit_bytes_per_direction: u32,
    pub buffer_bytes: u32,
    pub opens_per_second: u32,
    pub open_burst: u32,
    pub handshake_ms: u32,
    pub open_ms: u32,
    pub idle_ms: u32,
}
impl Default for EgressLimitsV1 {
    fn default() -> Self {
        Self {
            data_bytes: MAX_DATA_BYTES as u32,
            frame_bytes: MAX_EGRESS_FRAME_BYTES as u32,
            control_frame_bytes: MAX_EGRESS_CONTROL_BYTES as u32,
            live_flows: MAX_FLOWS as u32,
            pending_flows: MAX_PENDING as u32,
            credit_bytes_per_direction: MAX_CREDIT_BYTES as u32,
            buffer_bytes: MAX_BUFFER_BYTES as u32,
            opens_per_second: 4,
            open_burst: 8,
            handshake_ms: HANDSHAKE_MS as u32,
            open_ms: OPEN_MS as u32,
            idle_ms: IDLE_MS as u32,
        }
    }
}

/// This declaration is negotiated within the common, versioned channel. It
/// cannot establish workload identity or enable egress on a historical frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressCapabilityV1 {
    pub version: u32,
    pub limits: EgressLimitsV1,
}
impl Default for EgressCapabilityV1 {
    fn default() -> Self {
        Self {
            version: 1,
            limits: EgressLimitsV1::default(),
        }
    }
}
impl EgressCapabilityV1 {
    pub fn validate(&self) -> Result<(), EgressProtocolError> {
        if self.version != 1 || self.limits != EgressLimitsV1::default() {
            Err(EgressProtocolError::Capability)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressReasonV1 {
    Cancelled,
    Denied,
    Review,
    Capacity,
    Protocol,
    Timeout,
    Revoked,
    Expired,
    AuditUnavailable,
    /// The path is unavailable and this wire cannot establish a narrower cause.
    Unavailable,
    /// The broker observed upstream read EOF; this is not business success.
    EndOfStream,
    Transport,
}

/// No Debug/owned Deserialize: neither opaque content nor requested authority
/// belongs in diagnostics, and inbound strings are bounded before ownership.
#[derive(Clone, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum EgressMessageV1 {
    Open {
        flow_id: u64,
        authority: String,
    },
    Accepted {
        flow_id: u64,
    },
    Rejected {
        flow_id: u64,
        reason: EgressReasonV1,
    },
    Data {
        flow_id: u64,
        data_b64: String,
    },
    Credit {
        flow_id: u64,
        bytes: u32,
    },
    HalfClose {
        flow_id: u64,
    },
    Reset {
        flow_id: u64,
        reason: EgressReasonV1,
    },
    Closed {
        flow_id: u64,
        reason: EgressReasonV1,
    },
}

// No internally tagged Deserialize: Serde's Content buffering would allocate
// unknown/escaped values before variant-specific bounds. The fixed visitor
// below retains only borrowed scalar RawValue references.
enum BorrowedMessage<'a> {
    Open {
        flow_id: u64,
        authority: &'a str,
    },
    Accepted {
        flow_id: u64,
    },
    Rejected {
        flow_id: u64,
        reason: EgressReasonV1,
    },
    Data {
        flow_id: u64,
        data_b64: &'a str,
    },
    Credit {
        flow_id: u64,
        bytes: u32,
    },
    HalfClose {
        flow_id: u64,
    },
    Reset {
        flow_id: u64,
        reason: EgressReasonV1,
    },
    Closed {
        flow_id: u64,
        reason: EgressReasonV1,
    },
}

const FIELD_OPERATION: usize = 0;
const FIELD_FLOW: usize = 1;
const FIELD_AUTHORITY: usize = 2;
const FIELD_DATA: usize = 3;
const FIELD_CREDIT: usize = 4;
const FIELD_REASON: usize = 5;
const COMMON_FIELDS: u8 = (1 << FIELD_OPERATION) | (1 << FIELD_FLOW);

struct RawShape<'a> {
    fields: [Option<&'a RawValue>; 6],
    seen: u8,
    invalid: bool,
}
impl<'de> Deserialize<'de> for RawShape<'de> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ShapeVisitor;
        impl<'de> Visitor<'de> for ShapeVisitor {
            type Value = RawShape<'de>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a flat egress message object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut shape = RawShape {
                    fields: [None; 6],
                    seen: 0,
                    invalid: false,
                };
                while let Some(key) = map.next_key::<&'de str>()? {
                    // The raw guard forbids escaped keys/nested values before
                    // this standard parser runs. Scalar RawValue parsing from
                    // a slice borrows even unknown escaped string values.
                    let raw = map.next_value::<&'de RawValue>()?;
                    let index = match key {
                        "operation" => FIELD_OPERATION,
                        "flow_id" => FIELD_FLOW,
                        "authority" => FIELD_AUTHORITY,
                        "data_b64" => FIELD_DATA,
                        "bytes" => FIELD_CREDIT,
                        "reason" => FIELD_REASON,
                        _ => {
                            shape.invalid = true;
                            continue;
                        }
                    };
                    if shape.fields[index].is_some() {
                        shape.invalid = true;
                    } else {
                        shape.fields[index] = Some(raw);
                    }
                    shape.seen |= 1_u8 << index;
                }
                Ok(shape)
            }
        }
        deserializer.deserialize_map(ShapeVisitor)
    }
}
impl RawShape<'_> {
    fn require(&self, fields: u8) -> Result<(), EgressProtocolError> {
        if self.invalid || self.seen != fields {
            Err(EgressProtocolError::Protocol)
        } else {
            Ok(())
        }
    }
}
impl<'a> BorrowedMessage<'a> {
    fn decode(bytes: &'a [u8]) -> Result<Self, EgressProtocolError> {
        let shape: RawShape<'a> =
            serde_json::from_slice(bytes).map_err(|_| EgressProtocolError::Protocol)?;
        let operation = shape.fields[FIELD_OPERATION].map(RawValue::get);
        // Only canonical Data is eligible for the larger wire budget. Check
        // this before unknown/duplicate fields or escaped values are decoded,
        // regardless of the operation field's order in the object.
        if operation != Some("\"data\"") && bytes.len() > MAX_EGRESS_CONTROL_BYTES {
            return Err(EgressProtocolError::Size);
        }
        let flow_id = raw_integer(shape.fields[FIELD_FLOW], 20)?;
        if flow_id == 0 {
            return Err(EgressProtocolError::Protocol);
        }
        match operation {
            Some("\"open\"") => {
                shape.require(COMMON_FIELDS | (1 << FIELD_AUTHORITY))?;
                let authority = raw_string(shape.fields[FIELD_AUTHORITY], MAX_AUTHORITY_BYTES)?;
                validate_authority(authority)?;
                Ok(Self::Open { flow_id, authority })
            }
            Some("\"accepted\"") => {
                shape.require(COMMON_FIELDS)?;
                Ok(Self::Accepted { flow_id })
            }
            Some("\"data\"") => {
                shape.require(COMMON_FIELDS | (1 << FIELD_DATA))?;
                let data_b64 = raw_string(shape.fields[FIELD_DATA], MAX_ENCODED_DATA_BYTES)?;
                decoded_length(data_b64)?;
                Ok(Self::Data { flow_id, data_b64 })
            }
            Some("\"credit\"") => {
                shape.require(COMMON_FIELDS | (1 << FIELD_CREDIT))?;
                let bytes = u32::try_from(raw_integer(shape.fields[FIELD_CREDIT], 10)?)
                    .map_err(|_| EgressProtocolError::Protocol)?;
                Ok(Self::Credit { flow_id, bytes })
            }
            Some("\"half_close\"") => {
                shape.require(COMMON_FIELDS)?;
                Ok(Self::HalfClose { flow_id })
            }
            Some("\"rejected\"" | "\"reset\"" | "\"closed\"") => {
                shape.require(COMMON_FIELDS | (1 << FIELD_REASON))?;
                let reason = match raw_string(shape.fields[FIELD_REASON], 17)? {
                    "cancelled" => EgressReasonV1::Cancelled,
                    "denied" => EgressReasonV1::Denied,
                    "review" => EgressReasonV1::Review,
                    "capacity" => EgressReasonV1::Capacity,
                    "protocol" => EgressReasonV1::Protocol,
                    "timeout" => EgressReasonV1::Timeout,
                    "revoked" => EgressReasonV1::Revoked,
                    "expired" => EgressReasonV1::Expired,
                    "audit_unavailable" => EgressReasonV1::AuditUnavailable,
                    "unavailable" => EgressReasonV1::Unavailable,
                    "end_of_stream" => EgressReasonV1::EndOfStream,
                    "transport" => EgressReasonV1::Transport,
                    _ => return Err(EgressProtocolError::Protocol),
                };
                match operation {
                    Some("\"rejected\"") => Ok(Self::Rejected { flow_id, reason }),
                    Some("\"reset\"") => Ok(Self::Reset { flow_id, reason }),
                    _ => Ok(Self::Closed { flow_id, reason }),
                }
            }
            _ => Err(EgressProtocolError::Protocol),
        }
    }
}

fn raw_string(raw: Option<&RawValue>, maximum: usize) -> Result<&str, EgressProtocolError> {
    let raw = raw.ok_or(EgressProtocolError::Protocol)?.get();
    if raw.len() > maximum + 2 {
        return Err(EgressProtocolError::Size);
    }
    let value = raw
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .ok_or(EgressProtocolError::Protocol)?;
    if !value.is_ascii() || value.as_bytes().contains(&b'\\') {
        return Err(EgressProtocolError::Protocol);
    }
    Ok(value)
}

fn raw_integer(raw: Option<&RawValue>, digits: usize) -> Result<u64, EgressProtocolError> {
    let raw = raw.ok_or(EgressProtocolError::Protocol)?.get();
    if raw.is_empty() || raw.len() > digits || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(EgressProtocolError::Protocol);
    }
    raw.parse().map_err(|_| EgressProtocolError::Protocol)
}

impl EgressMessageV1 {
    pub fn flow_id(&self) -> u64 {
        match self {
            Self::Open { flow_id, .. }
            | Self::Accepted { flow_id }
            | Self::Rejected { flow_id, .. }
            | Self::Data { flow_id, .. }
            | Self::Credit { flow_id, .. }
            | Self::HalfClose { flow_id }
            | Self::Reset { flow_id, .. }
            | Self::Closed { flow_id, .. } => *flow_id,
        }
    }

    pub fn validate(&self) -> Result<(), EgressProtocolError> {
        if self.flow_id() == 0 {
            return Err(EgressProtocolError::Protocol);
        }
        match self {
            Self::Open { authority, .. } => validate_authority(authority),
            Self::Data { data_b64, .. } => decoded_length(data_b64).map(|_| ()),
            Self::Credit { bytes, .. } if *bytes == 0 || *bytes as usize > MAX_CREDIT_BYTES => {
                Err(EgressProtocolError::Credit)
            }
            Self::Rejected { reason: EgressReasonV1::EndOfStream, .. }
            | Self::Reset { reason: EgressReasonV1::EndOfStream, .. } => Err(EgressProtocolError::Protocol),
            _ => Ok(()),
        }
    }

    /// Payload codec only; the common framing owner bounds the length prefix
    /// before reading/allocating these bytes and reserves its scratch budget.
    pub fn decode_payload(bytes: &[u8]) -> Result<Self, EgressProtocolError> {
        if bytes.is_empty() || bytes.len() > MAX_EGRESS_FRAME_BYTES {
            return Err(EgressProtocolError::Size);
        }
        bounded_structure(bytes)?;
        // Fixed borrowed preflight preserves the closed unique-JSON contract
        // without producing a second owned Value/Content tree.
        let borrowed = BorrowedMessage::decode(bytes)?;
        let message = match borrowed {
            BorrowedMessage::Open { flow_id, authority } => Self::Open {
                flow_id,
                authority: copy_string(authority)?,
            },
            BorrowedMessage::Accepted { flow_id } => Self::Accepted { flow_id },
            BorrowedMessage::Rejected { flow_id, reason } => Self::Rejected { flow_id, reason },
            BorrowedMessage::Data { flow_id, data_b64 } => Self::Data {
                flow_id,
                data_b64: copy_string(data_b64)?,
            },
            BorrowedMessage::Credit { flow_id, bytes } => Self::Credit { flow_id, bytes },
            BorrowedMessage::HalfClose { flow_id } => Self::HalfClose { flow_id },
            BorrowedMessage::Reset { flow_id, reason } => Self::Reset { flow_id, reason },
            BorrowedMessage::Closed { flow_id, reason } => Self::Closed { flow_id, reason },
        };
        message.validate()?;
        Ok(message)
    }

    pub fn encode_payload(&self) -> Result<Vec<u8>, EgressProtocolError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| EgressProtocolError::Protocol)?;
        let maximum = if matches!(self, Self::Data { .. }) {
            MAX_EGRESS_FRAME_BYTES
        } else {
            MAX_EGRESS_CONTROL_BYTES
        };
        if bytes.len() > maximum {
            Err(EgressProtocolError::Size)
        } else {
            Ok(bytes)
        }
    }

    pub fn data(flow_id: u64, bytes: &[u8]) -> Result<Self, EgressProtocolError> {
        if flow_id == 0 {
            return Err(EgressProtocolError::Protocol);
        }
        if bytes.is_empty() || bytes.len() > MAX_DATA_BYTES {
            return Err(EgressProtocolError::Size);
        }
        let mut data_b64 = String::new();
        data_b64
            .try_reserve_exact(bytes.len().div_ceil(3) * 4)
            .map_err(|_| EgressProtocolError::Capacity)?;
        STANDARD.encode_string(bytes, &mut data_b64);
        Ok(Self::Data { flow_id, data_b64 })
    }

    pub fn decoded_data(&self) -> Result<Option<Vec<u8>>, EgressProtocolError> {
        self.validate()?;
        let Self::Data { data_b64, .. } = self else {
            return Ok(None);
        };
        let length = decoded_length(data_b64)?;
        // Capacity is based on the already bounded encoded form. The engine's
        // conservative estimate includes up to two padding bytes.
        let capacity = (data_b64.len() / 4) * 3;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(capacity)
            .map_err(|_| EgressProtocolError::Capacity)?;
        bytes.resize(capacity, 0);
        let decoded = STANDARD
            .decode_slice(data_b64, &mut bytes)
            .map_err(|_| EgressProtocolError::Protocol)?;
        if decoded != length {
            return Err(EgressProtocolError::Protocol);
        }
        bytes.truncate(decoded);
        Ok(Some(bytes))
    }
}

fn validate_authority(raw: &str) -> Result<(), EgressProtocolError> {
    if raw.is_empty() || raw.len() > MAX_AUTHORITY_BYTES {
        return Err(EgressProtocolError::Size);
    }
    Authority::parse_connect(raw)
        .map(|_| ())
        .map_err(|_| EgressProtocolError::Protocol)
}

fn copy_string(value: &str) -> Result<String, EgressProtocolError> {
    let mut owned = String::new();
    owned
        .try_reserve_exact(value.len())
        .map_err(|_| EgressProtocolError::Capacity)?;
    owned.push_str(value);
    Ok(owned)
}

fn decoded_length(encoded: &str) -> Result<usize, EgressProtocolError> {
    if encoded.is_empty() || encoded.len() > MAX_ENCODED_DATA_BYTES {
        return Err(EgressProtocolError::Size);
    }
    // The existing standard decoder validates alphabet, padding and unused
    // trailing bits. Fixed scratch precedes all decoded-payload allocation.
    let mut scratch = [0_u8; MAX_DATA_BYTES + 2];
    let length = STANDARD
        .decode_slice(encoded, &mut scratch)
        .map_err(|_| EgressProtocolError::Protocol)?;
    if length == 0 || length > MAX_DATA_BYTES {
        Err(EgressProtocolError::Size)
    } else {
        Ok(length)
    }
}

fn bounded_structure(bytes: &[u8]) -> Result<(), EgressProtocolError> {
    if bytes
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        != Some(b'{')
    {
        return Err(EgressProtocolError::Protocol);
    }
    let mut quoted = false;
    let mut escaped = false;
    let mut key_string = false;
    let mut expecting_key = false;
    let mut tokens = 0_u32;
    let mut depth = 0_u32;
    for byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                // Reject escaped keys even when malformed/unterminated before
                // serde_json's key parser could decode them into scratch.
                if key_string {
                    return Err(if bytes.len() > MAX_EGRESS_CONTROL_BYTES {
                        EgressProtocolError::Size
                    } else {
                        EgressProtocolError::Protocol
                    });
                }
                escaped = true;
            } else if *byte == b'"' {
                quoted = false;
            }
        } else if *byte == b'"' {
            quoted = true;
            key_string = expecting_key;
        } else {
            if matches!(*byte, b'{' | b'[' | b',' | b':') {
                tokens += 1;
                if tokens > 64 {
                    return Err(EgressProtocolError::Size);
                }
            }
            if matches!(*byte, b'{' | b'[') {
                depth += 1;
                // Every message field is scalar. Reject nested containers
                // before RawValue's ignore-value stack could grow a Vec.
                if depth > 1 {
                    return Err(EgressProtocolError::Size);
                }
                expecting_key = true;
            } else if matches!(*byte, b'}' | b']') {
                depth = depth.checked_sub(1).ok_or(EgressProtocolError::Protocol)?;
            } else if *byte == b',' {
                expecting_key = true;
            } else if *byte == b':' {
                expecting_key = false;
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectionV1 {
    WorkerToBroker,
    BrokerToWorker,
}
impl DirectionV1 {
    fn index(self) -> usize {
        match self {
            Self::WorkerToBroker => 0,
            Self::BrokerToWorker => 1,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitingCapability,
    Open,
    Closed,
}
struct Flow {
    id: u64,
    pending: bool,
    opened_at: u64,
    last_active: u64,
    credit: [usize; 2],
    buffered: [usize; 2],
    half_closed: [bool; 2],
}

/// Fixed-size local ledger for one externally owned epoch. It reserves payload
/// bytes but stores none. The owner must not queue a payload until admission
/// succeeds, and must release its reservation only after forwarding/drop.
/// Every error invalidates the channel and discards all reservations. The owner
/// must then close its actual FDs; clearing this state is no observation of IO.
pub struct EgressChannelState {
    phase: Phase,
    created_at: u64,
    last_now: u64,
    last_flow_id: u64,
    token_at: u64,
    tokens: u64,
    flows: [Option<Flow>; MAX_FLOWS],
    credit: [usize; 2],
    buffered: [usize; 2],
}
impl EgressChannelState {
    pub fn new(now_ms: u64) -> Self {
        Self {
            phase: Phase::AwaitingCapability,
            created_at: now_ms,
            last_now: now_ms,
            last_flow_id: 0,
            token_at: now_ms,
            tokens: OPEN_TOKENS,
            flows: std::array::from_fn(|_| None),
            credit: [0; 2],
            buffered: [0; 2],
        }
    }

    pub fn accept_capability(
        &mut self,
        capability: Option<&EgressCapabilityV1>,
        now_ms: u64,
    ) -> Result<(), EgressProtocolError> {
        let result = (|| {
            self.check_deadlines(now_ms)?;
            if self.phase != Phase::AwaitingCapability {
                return Err(EgressProtocolError::State);
            }
            capability
                .ok_or(EgressProtocolError::Capability)?
                .validate()?;
            self.phase = Phase::Open;
            Ok(())
        })();
        self.finish(result)
    }

    pub fn on_worker(
        &mut self,
        message: &EgressMessageV1,
        now_ms: u64,
    ) -> Result<(), EgressProtocolError> {
        self.on_message(message, DirectionV1::WorkerToBroker, now_ms)
    }

    pub fn on_broker(
        &mut self,
        message: &EgressMessageV1,
        now_ms: u64,
    ) -> Result<(), EgressProtocolError> {
        self.on_message(message, DirectionV1::BrokerToWorker, now_ms)
    }

    fn on_message(
        &mut self,
        message: &EgressMessageV1,
        sender: DirectionV1,
        now_ms: u64,
    ) -> Result<(), EgressProtocolError> {
        let result = (|| {
            self.check_deadlines(now_ms)?;
            if self.phase != Phase::Open {
                return Err(EgressProtocolError::Capability);
            }
            message.validate()?;
            let worker = sender == DirectionV1::WorkerToBroker;
            match message {
                EgressMessageV1::Open { flow_id, .. } if worker => self.open(*flow_id, now_ms),
                EgressMessageV1::Accepted { flow_id } if !worker => {
                    let index = self.flow_index(*flow_id)?;
                    let flow = self.flows[index]
                        .as_mut()
                        .ok_or(EgressProtocolError::State)?;
                    if !flow.pending {
                        return Err(EgressProtocolError::State);
                    }
                    flow.pending = false;
                    flow.last_active = now_ms;
                    Ok(())
                }
                EgressMessageV1::Rejected { flow_id, .. } if !worker => {
                    let index = self.flow_index(*flow_id)?;
                    if !self.flows[index].as_ref().is_some_and(|flow| flow.pending) {
                        return Err(EgressProtocolError::State);
                    }
                    self.remove_flow(index)
                }
                EgressMessageV1::Data { flow_id, data_b64 } => {
                    self.admit_data(*flow_id, sender.index(), decoded_length(data_b64)?, now_ms)
                }
                EgressMessageV1::Credit { flow_id, bytes } => {
                    self.admit_credit(*flow_id, 1 - sender.index(), *bytes as usize, now_ms)
                }
                EgressMessageV1::HalfClose { flow_id } => {
                    let index = self.flow_index(*flow_id)?;
                    let flow = self.flows[index]
                        .as_mut()
                        .ok_or(EgressProtocolError::State)?;
                    if flow.pending || flow.half_closed[sender.index()] {
                        return Err(EgressProtocolError::State);
                    }
                    // No future Data can consume this sender's unused window.
                    // Release only that credit; queued bytes stay reserved and
                    // remain drainable until the actual owner forwards them.
                    self.credit[sender.index()] -= flow.credit[sender.index()];
                    flow.credit[sender.index()] = 0;
                    flow.half_closed[sender.index()] = true;
                    flow.last_active = now_ms;
                    Ok(())
                }
                EgressMessageV1::Reset { flow_id, reason }
                    if !worker || *reason == EgressReasonV1::Cancelled =>
                {
                    let index = self.flow_index(*flow_id)?;
                    self.remove_flow(index)
                }
                EgressMessageV1::Closed { flow_id, .. } if !worker => {
                    let index = self.flow_index(*flow_id)?;
                    let flow = self.flows[index]
                        .as_ref()
                        .ok_or(EgressProtocolError::State)?;
                    if flow.pending || flow.buffered != [0; 2] {
                        return Err(EgressProtocolError::State);
                    }
                    self.remove_flow(index)
                }
                _ => Err(EgressProtocolError::Protocol),
            }
        })();
        self.finish(result)
    }

    fn open(&mut self, flow_id: u64, now_ms: u64) -> Result<(), EgressProtocolError> {
        if flow_id <= self.last_flow_id {
            return Err(EgressProtocolError::Protocol);
        }
        if self.flow_count() >= MAX_FLOWS
            || self
                .flows
                .iter()
                .flatten()
                .filter(|flow| flow.pending)
                .count()
                >= MAX_PENDING
        {
            return Err(EgressProtocolError::Capacity);
        }
        self.tokens = self
            .tokens
            .saturating_add((now_ms - self.token_at).saturating_mul(REFILL_PER_MS))
            .min(OPEN_TOKENS);
        self.token_at = now_ms;
        if self.tokens < OPEN_COST {
            return Err(EgressProtocolError::Capacity);
        }
        let index = self
            .flows
            .iter()
            .position(Option::is_none)
            .ok_or(EgressProtocolError::Capacity)?;
        self.tokens -= OPEN_COST;
        self.last_flow_id = flow_id;
        self.flows[index] = Some(Flow {
            id: flow_id,
            pending: true,
            opened_at: now_ms,
            last_active: now_ms,
            credit: [0; 2],
            buffered: [0; 2],
            half_closed: [false; 2],
        });
        Ok(())
    }

    fn admit_data(
        &mut self,
        id: u64,
        direction: usize,
        bytes: usize,
        now_ms: u64,
    ) -> Result<(), EgressProtocolError> {
        let index = self.flow_index(id)?;
        let flow = self.flows[index]
            .as_mut()
            .ok_or(EgressProtocolError::State)?;
        if flow.pending || flow.half_closed[direction] {
            return Err(EgressProtocolError::State);
        }
        if bytes > flow.credit[direction] {
            return Err(EgressProtocolError::Credit);
        }
        let total = self.buffered[0]
            .checked_add(self.buffered[1])
            .and_then(|total| total.checked_add(bytes))
            .ok_or(EgressProtocolError::Capacity)?;
        if total > MAX_BUFFER_BYTES {
            return Err(EgressProtocolError::Capacity);
        }
        flow.credit[direction] -= bytes;
        flow.buffered[direction] += bytes;
        flow.last_active = now_ms;
        self.credit[direction] -= bytes;
        self.buffered[direction] += bytes;
        Ok(())
    }

    fn admit_credit(
        &mut self,
        id: u64,
        direction: usize,
        bytes: usize,
        now_ms: u64,
    ) -> Result<(), EgressProtocolError> {
        let index = self.flow_index(id)?;
        let flow = self.flows[index]
            .as_mut()
            .ok_or(EgressProtocolError::State)?;
        if flow.pending || flow.half_closed[direction] {
            return Err(EgressProtocolError::State);
        }
        let outstanding = self.credit[direction]
            .checked_add(self.buffered[direction])
            .and_then(|value| value.checked_add(bytes))
            .ok_or(EgressProtocolError::Capacity)?;
        if outstanding > MAX_CREDIT_BYTES {
            return Err(EgressProtocolError::Credit);
        }
        flow.credit[direction] += bytes;
        flow.last_active = now_ms;
        self.credit[direction] += bytes;
        Ok(())
    }

    /// Trusted owner's forwarding completion only; not callable by a wire
    /// message. It releases queue memory, never grants additional send credit.
    /// HalfClose does not prevent draining bytes admitted before that marker.
    pub fn release_buffer(
        &mut self,
        flow_id: u64,
        direction: DirectionV1,
        bytes: usize,
    ) -> Result<(), EgressProtocolError> {
        let result = (|| {
            if self.phase != Phase::Open {
                return Err(EgressProtocolError::Closed);
            }
            let index = self.flow_index(flow_id)?;
            let flow = self.flows[index]
                .as_mut()
                .ok_or(EgressProtocolError::State)?;
            let direction = direction.index();
            if bytes == 0 || bytes > flow.buffered[direction] {
                return Err(EgressProtocolError::State);
            }
            flow.buffered[direction] -= bytes;
            self.buffered[direction] -= bytes;
            Ok(())
        })();
        self.finish(result)
    }

    pub fn check_deadlines(&mut self, now_ms: u64) -> Result<(), EgressProtocolError> {
        let result = (|| {
            if self.phase == Phase::Closed {
                return Err(EgressProtocolError::Closed);
            }
            if now_ms < self.last_now {
                return Err(EgressProtocolError::Clock);
            }
            self.last_now = now_ms;
            if self.phase == Phase::AwaitingCapability && now_ms - self.created_at >= HANDSHAKE_MS {
                return Err(EgressProtocolError::Timeout);
            }
            for flow in self.flows.iter().flatten() {
                let expired = if flow.pending {
                    now_ms - flow.opened_at >= OPEN_MS
                } else {
                    now_ms - flow.last_active >= IDLE_MS
                };
                if expired {
                    return Err(EgressProtocolError::Timeout);
                }
            }
            Ok(())
        })();
        self.finish(result)
    }

    fn flow_index(&self, id: u64) -> Result<usize, EgressProtocolError> {
        self.flows
            .iter()
            .position(|entry| entry.as_ref().is_some_and(|flow| flow.id == id))
            .ok_or(EgressProtocolError::State)
    }

    fn remove_flow(&mut self, index: usize) -> Result<(), EgressProtocolError> {
        let flow = self.flows[index].take().ok_or(EgressProtocolError::State)?;
        for direction in 0..2 {
            self.credit[direction] -= flow.credit[direction];
            self.buffered[direction] -= flow.buffered[direction];
        }
        Ok(())
    }

    fn finish<T>(
        &mut self,
        result: Result<T, EgressProtocolError>,
    ) -> Result<T, EgressProtocolError> {
        if result.is_err() {
            self.close();
        }
        result
    }
    pub fn close(&mut self) {
        self.phase = Phase::Closed;
        for flow in &mut self.flows {
            *flow = None;
        }
        self.credit = [0; 2];
        self.buffered = [0; 2];
    }
    pub fn is_closed(&self) -> bool {
        self.phase == Phase::Closed
    }
    pub fn flow_count(&self) -> usize {
        self.flows.iter().flatten().count()
    }
    pub fn buffered_bytes(&self) -> usize {
        self.buffered[0] + self.buffered[1]
    }
}
