//! Wire protocol v1 of the activity channel: one length-prefix codec and the
//! closed, typed frames of its three hops.
//!
//! Framing: a `u32` big-endian body length, then one UTF-8 JSON object. The
//! advertised length is checked against the direction's bound before any
//! allocation or body read. Any IO, framing, decoding or validation error
//! permanently poisons a reader: no later byte is scanned to resynchronize.
//! Unknown fields are refused on every frame.
//!
//! Hops and directions:
//! - hook client → helper: [`ClientFrameV1`] (`submit`), answered by one
//!   [`ReplyFrameV1`] (`reply`);
//! - helper → broker (helper stdout): [`HelperFrameV1`], `hello` first, then
//!   `hook` and `loss` frames whose `seq` is 1, 2, 3, … without gap;
//! - broker → helper (helper stdin): [`BrokerFrameV1`], `ready` once after
//!   `hello`, then one `ack` per `hook` seq. A `loss` frame is not acked.
//!
//! The `submit` object nested in a `hook` frame carries the submission fields
//! without the `type` key. A submission is provider-declared activity: its
//! payload is untrusted text and never an observation of an effect.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{fmt, marker::PhantomData, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    time::{Instant, timeout_at},
};

pub const PROTOCOL_VERSION: u32 = 1;
/// Stdin bytes a hook client keeps, and also the budget of their JSON string
/// encoding: a payload that escapes beyond it is cut and flagged truncated.
pub const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
/// Room for every non-payload field of a submission.
pub const ENVELOPE_BYTES: usize = 4 * 1024;
pub const MAX_SUBMIT_FRAME_BYTES: usize = MAX_PAYLOAD_BYTES + ENVELOPE_BYTES;
/// A helper `hook` frame wraps one submission with its own small envelope.
pub const MAX_HELPER_FRAME_BYTES: usize = MAX_SUBMIT_FRAME_BYTES + 1024;
/// `reply`, `ready` and `ack` frames.
pub const MAX_CONTROL_FRAME_BYTES: usize = 1024;
/// A started frame must complete within this absolute time …
const FRAME_TIME: Duration = Duration::from_secs(5);
/// … and make progress at least this often.
const IDLE_TIME: Duration = Duration::from_secs(2);

/// Closed diagnostics: no source byte, payload or OS error text is retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    Closed,
    Io,
    PartialFrame,
    FrameSize,
    Protocol,
    Timeout,
    Capacity,
}
impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Closed => "activity channel is closed",
            Self::Io => "activity channel IO failed",
            Self::PartialFrame => "activity frame is incomplete",
            Self::FrameSize => "activity frame length is outside its bound",
            Self::Protocol => "activity frame is incompatible",
            Self::Timeout => "activity frame deadline expired",
            Self::Capacity => "activity frame memory is unavailable",
        })
    }
}
impl std::error::Error for WireError {}

/// Provider clients whose official hooks call the activity client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClientV1 {
    #[serde(rename = "claude-code")]
    ClaudeCode,
    #[serde(rename = "codex")]
    Codex,
}
impl ClientV1 {
    pub const ALL: [Self; 2] = [Self::ClaudeCode, Self::Codex];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|client| client.as_str() == value)
    }

    /// Claude Code may call every allowlisted event; Codex only its subset.
    pub fn supports(self, event: EventV1) -> bool {
        match self {
            Self::ClaudeCode => true,
            Self::Codex => CODEX_EVENTS.contains(&event),
        }
    }
}

/// Fixed hook event allowlist. Names are the providers' official event names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventV1 {
    SessionStart,
    SessionEnd,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    SubagentStart,
    SubagentStop,
    Stop,
    Notification,
    PreCompact,
}
/// Codex CLI hook events configured by the managed requirements file.
pub const CODEX_EVENTS: [EventV1; 5] = [
    EventV1::SessionStart,
    EventV1::UserPromptSubmit,
    EventV1::PreToolUse,
    EventV1::PostToolUse,
    EventV1::Stop,
];
impl EventV1 {
    pub const ALL: [Self; 11] = [
        Self::SessionStart,
        Self::SessionEnd,
        Self::UserPromptSubmit,
        Self::PreToolUse,
        Self::PostToolUse,
        Self::PostToolUseFailure,
        Self::SubagentStart,
        Self::SubagentStop,
        Self::Stop,
        Self::Notification,
        Self::PreCompact,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::SubagentStart => "SubagentStart",
            Self::SubagentStop => "SubagentStop",
            Self::Stop => "Stop",
            Self::Notification => "Notification",
            Self::PreCompact => "PreCompact",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|event| event.as_str() == value)
    }

    /// A gate event blocks its provider action unless the broker recorded it.
    pub fn is_gate(self) -> bool {
        matches!(self, Self::PreToolUse | Self::UserPromptSubmit)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusV1 {
    Recorded,
    Refused,
}

/// Each reason is counted by the observer that saw it. Reasons are not
/// additive: a non-gate hook refused for capacity is counted once by the
/// helper (`helper_capacity`) and once by its own marker (`client_unrecorded`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LossReasonV1 {
    /// Loss markers left by hook clients whose submission was not recorded.
    ClientUnrecorded,
    /// Connections the helper refused at its pending-hook capacity.
    HelperCapacity,
    /// Client frames whose advertised length exceeded the submit bound.
    OversizedFrame,
    /// Client frames that were incomplete, late, undecodable or invalid.
    InvalidFrame,
    /// Connections whose peer UID differed from the helper's.
    PeerRefused,
}
impl LossReasonV1 {
    pub const ALL: [Self; 5] = [
        Self::ClientUnrecorded,
        Self::HelperCapacity,
        Self::OversizedFrame,
        Self::InvalidFrame,
        Self::PeerRefused,
    ];
}

/// One hook invocation as the provider handed it to the hook client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitV1 {
    pub version: u32,
    pub client: ClientV1,
    pub event: EventV1,
    /// Must equal `event.is_gate()`.
    pub gate: bool,
    /// Stdin bytes counted before EOF or the client's drain deadline. When the
    /// drain deadline expired the count is a lower bound and the payload is
    /// flagged truncated.
    pub stdin_total_bytes: u64,
    /// The payload is not the complete stdin.
    pub stdin_truncated: bool,
    /// The kept stdin bytes were not UTF-8; the payload is then empty.
    pub payload_invalid_utf8: bool,
    /// A UTF-8 prefix of the hook's stdin (normally its JSON input), unparsed.
    pub payload: String,
}
impl SubmitV1 {
    pub fn validate(&self) -> Result<(), WireError> {
        let length = self.payload.len() as u64;
        let complete = !self.stdin_truncated && !self.payload_invalid_utf8;
        if self.version != PROTOCOL_VERSION
            || !self.client.supports(self.event)
            || self.gate != self.event.is_gate()
            || self.payload.len() > MAX_PAYLOAD_BYTES
            || self.stdin_total_bytes < length
            || (self.payload_invalid_utf8
                && (!self.payload.is_empty() || self.stdin_total_bytes == 0))
            || (complete && self.stdin_total_bytes != length)
        {
            return Err(WireError::Protocol);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplyV1 {
    pub version: u32,
    pub status: StatusV1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelloV1 {
    pub version: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadyV1 {
    pub version: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookV1 {
    pub seq: u64,
    pub submit: SubmitV1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LossV1 {
    pub seq: u64,
    pub count: u64,
    pub reason: LossReasonV1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AckV1 {
    pub seq: u64,
    pub status: StatusV1,
}

/// Hook client → helper.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrameV1 {
    Submit(SubmitV1),
}
/// Helper → hook client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReplyFrameV1 {
    Reply(ReplyV1),
}
/// Helper → broker, on the helper's stdout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HelperFrameV1 {
    Hello(HelloV1),
    Hook(HookV1),
    Loss(LossV1),
}
/// Broker → helper, on the helper's stdin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrokerFrameV1 {
    Ready(ReadyV1),
    Ack(AckV1),
}

/// A frame of one direction: its encoded bound and its own field checks.
pub trait Frame: Serialize + DeserializeOwned {
    const MAX_BYTES: usize;
    fn validate(&self) -> Result<(), WireError>;
}
fn version(value: u32) -> Result<(), WireError> {
    if value == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(WireError::Protocol)
    }
}
fn positive(value: u64) -> Result<(), WireError> {
    if value == 0 {
        Err(WireError::Protocol)
    } else {
        Ok(())
    }
}
impl Frame for ClientFrameV1 {
    const MAX_BYTES: usize = MAX_SUBMIT_FRAME_BYTES;
    fn validate(&self) -> Result<(), WireError> {
        match self {
            Self::Submit(submit) => submit.validate(),
        }
    }
}
impl Frame for ReplyFrameV1 {
    const MAX_BYTES: usize = MAX_CONTROL_FRAME_BYTES;
    fn validate(&self) -> Result<(), WireError> {
        match self {
            Self::Reply(reply) => version(reply.version),
        }
    }
}
impl Frame for HelperFrameV1 {
    const MAX_BYTES: usize = MAX_HELPER_FRAME_BYTES;
    fn validate(&self) -> Result<(), WireError> {
        match self {
            Self::Hello(hello) => version(hello.version),
            Self::Hook(hook) => positive(hook.seq).and_then(|()| hook.submit.validate()),
            Self::Loss(loss) => positive(loss.seq).and_then(|()| positive(loss.count)),
        }
    }
}
impl Frame for BrokerFrameV1 {
    const MAX_BYTES: usize = MAX_CONTROL_FRAME_BYTES;
    fn validate(&self) -> Result<(), WireError> {
        match self {
            Self::Ready(ready) => version(ready.version),
            Self::Ack(ack) => positive(ack.seq),
        }
    }
}

struct BoundedBody {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}
impl std::io::Write for BoundedBody {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(std::io::Error::other("activity frame bound exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Validate, then serialize into at most `F::MAX_BYTES`; returns the body.
pub fn encode_body<F: Frame>(frame: &F) -> Result<Vec<u8>, WireError> {
    frame.validate()?;
    let mut body = BoundedBody {
        bytes: Vec::new(),
        limit: F::MAX_BYTES,
        exceeded: false,
    };
    body.bytes
        .try_reserve(F::MAX_BYTES.min(64 * 1024))
        .map_err(|_| WireError::Capacity)?;
    if serde_json::to_writer(&mut body, frame).is_err() {
        return Err(if body.exceeded {
            WireError::FrameSize
        } else {
            WireError::Protocol
        });
    }
    if body.bytes.is_empty() {
        return Err(WireError::Protocol);
    }
    Ok(body.bytes)
}

/// The length prefix followed by the encoded body.
pub fn encode_framed<F: Frame>(frame: &F) -> Result<Vec<u8>, WireError> {
    let body = encode_body(frame)?;
    let length = u32::try_from(body.len()).map_err(|_| WireError::FrameSize)?;
    let mut framed = Vec::new();
    framed
        .try_reserve_exact(body.len() + 4)
        .map_err(|_| WireError::Capacity)?;
    framed.extend_from_slice(&length.to_be_bytes());
    framed.extend_from_slice(&body);
    Ok(framed)
}

/// Decode one body of this direction: UTF-8, closed JSON shape, then fields.
pub fn decode_body<F: Frame>(body: &[u8]) -> Result<F, WireError> {
    if body.is_empty() || body.len() > F::MAX_BYTES {
        return Err(WireError::FrameSize);
    }
    let text = std::str::from_utf8(body).map_err(|_| WireError::Protocol)?;
    let frame: F = serde_json::from_str(text).map_err(|_| WireError::Protocol)?;
    frame.validate()?;
    Ok(frame)
}

/// Escaped length of one character inside a JSON string, as serde_json
/// writes it (two-byte escapes for quote, backslash and the five named
/// controls; `\u00XX` for other controls; other characters verbatim).
fn escaped_len(character: char) -> usize {
    match character {
        '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
        other if (other as u32) < 0x20 => 6,
        other => other.len_utf8(),
    }
}

/// The longest character-boundary prefix of `text` whose JSON string encoding
/// fits [`MAX_PAYLOAD_BYTES`], and whether it is shorter than `text`.
pub fn bounded_payload(text: &str) -> (&str, bool) {
    let mut used = 0usize;
    for (index, character) in text.char_indices() {
        let cost = escaped_len(character);
        if cost > MAX_PAYLOAD_BYTES - used {
            return (&text[..index], true);
        }
        used += cost;
    }
    (text, false)
}

/// Cancellation-safe reader of one direction. Every completed read advances the
/// retained prefix/body offsets before another await, so a select branch may
/// drop the future between frames or inside one. A started frame keeps its
/// original absolute and inactivity deadlines across such cancellations. The
/// first byte of a frame has no deadline here; owners bound idle peers.
pub struct FrameReader<F> {
    prefix: [u8; 4],
    prefix_used: usize,
    body: Vec<u8>,
    body_used: usize,
    absolute: Option<Instant>,
    idle: Option<Instant>,
    closed: bool,
    frame: PhantomData<fn() -> F>,
}
impl<F: Frame> Default for FrameReader<F> {
    fn default() -> Self {
        Self::new()
    }
}
impl<F: Frame> FrameReader<F> {
    pub fn new() -> Self {
        Self {
            prefix: [0; 4],
            prefix_used: 0,
            body: Vec::new(),
            body_used: 0,
            absolute: None,
            idle: None,
            closed: false,
            frame: PhantomData,
        }
    }

    /// `Ok(None)` is EOF before a frame. EOF and every error poison the reader.
    pub async fn read<R: AsyncRead + Unpin>(
        &mut self,
        input: &mut R,
    ) -> Result<Option<F>, WireError> {
        if self.closed {
            return Err(WireError::Closed);
        }
        let result = self.read_inner(input).await;
        if !matches!(&result, Ok(Some(_))) {
            self.close();
        }
        result
    }

    async fn read_inner<R: AsyncRead + Unpin>(
        &mut self,
        input: &mut R,
    ) -> Result<Option<F>, WireError> {
        while self.prefix_used < 4 {
            let deadline = self.deadline();
            let count = read_piece(input, &mut self.prefix[self.prefix_used..], deadline).await?;
            if count == 0 {
                return if self.prefix_used == 0 {
                    Ok(None)
                } else {
                    Err(WireError::PartialFrame)
                };
            }
            self.progress();
            self.prefix_used += count;
        }
        if self.body.is_empty() {
            let length = u32::from_be_bytes(self.prefix) as usize;
            // The advertised size is refused before any allocation or read.
            if length == 0 || length > F::MAX_BYTES {
                return Err(WireError::FrameSize);
            }
            self.body
                .try_reserve_exact(length)
                .map_err(|_| WireError::Capacity)?;
            self.body.resize(length, 0);
        }
        while self.body_used < self.body.len() {
            let deadline = self.deadline();
            let count = read_piece(input, &mut self.body[self.body_used..], deadline).await?;
            if count == 0 {
                return Err(WireError::PartialFrame);
            }
            self.progress();
            self.body_used += count;
        }
        let frame = decode_body::<F>(&self.body)?;
        *self = Self::new();
        Ok(Some(frame))
    }

    fn deadline(&self) -> Option<Instant> {
        self.absolute
            .zip(self.idle)
            .map(|(absolute, idle)| absolute.min(idle))
    }

    fn progress(&mut self) {
        let now = Instant::now();
        self.absolute.get_or_insert(now + FRAME_TIME);
        self.idle = Some(now + IDLE_TIME);
    }

    /// Refuse every later read, with or without a partial frame retained.
    pub fn close(&mut self) {
        self.closed = true;
        self.body = Vec::new();
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

async fn read_piece<R: AsyncRead + Unpin>(
    input: &mut R,
    bytes: &mut [u8],
    deadline: Option<Instant>,
) -> Result<usize, WireError> {
    match deadline {
        Some(deadline) => {
            if Instant::now() >= deadline {
                return Err(WireError::Timeout);
            }
            timeout_at(deadline, input.read(bytes))
                .await
                .map_err(|_| WireError::Timeout)?
                .map_err(|_| WireError::Io)
        }
        None => input.read(bytes).await.map_err(|_| WireError::Io),
    }
}

/// Writer that poisons itself after an error or a cancelled write: a frame
/// partially written to the transport can never be followed by another one.
#[derive(Debug, Default)]
pub struct FrameWriter {
    busy: bool,
    closed: bool,
}
impl FrameWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// One frame within an absolute 5 s deadline and 2 s per write progress.
    pub async fn write<F: Frame, W: AsyncWrite + Unpin>(
        &mut self,
        output: &mut W,
        frame: &F,
    ) -> Result<(), WireError> {
        if self.closed || self.busy {
            self.closed = true;
            return Err(WireError::Closed);
        }
        self.busy = true;
        let result = write_frame(output, frame).await;
        self.busy = false;
        if result.is_err() {
            self.closed = true;
        }
        result
    }

    pub fn close(&mut self) {
        self.closed = true;
    }

    pub fn is_closed(&self) -> bool {
        self.closed || self.busy
    }
}

/// Encode before the first byte is written, then write within an absolute 5 s
/// deadline and 2 s per progress step. Dropping this future mid-frame leaves
/// the transport unusable; [`FrameWriter`] records that.
pub async fn write_frame<F: Frame, W: AsyncWrite + Unpin>(
    output: &mut W,
    frame: &F,
) -> Result<(), WireError> {
    let bytes = encode_framed(frame)?;
    let absolute = Instant::now() + FRAME_TIME;
    let mut offset = 0;
    while offset < bytes.len() {
        let deadline = absolute.min(Instant::now() + IDLE_TIME);
        if Instant::now() >= deadline {
            return Err(WireError::Timeout);
        }
        let count = timeout_at(deadline, output.write(&bytes[offset..]))
            .await
            .map_err(|_| WireError::Timeout)?
            .map_err(|_| WireError::Io)?;
        if count == 0 {
            return Err(WireError::Io);
        }
        offset += count;
    }
    timeout_at(absolute.min(Instant::now() + IDLE_TIME), output.flush())
        .await
        .map_err(|_| WireError::Timeout)?
        .map_err(|_| WireError::Io)
}

/// Broker-side order check of the helper stream: exactly one `hello` first,
/// then `hook`/`loss` sequence numbers 1, 2, 3, … without gap or reuse. A gap
/// would hide a frame; any violation is a protocol error for the whole stream.
#[derive(Debug, Default)]
pub struct HelperSequence {
    hello: bool,
    last: u64,
}
impl HelperSequence {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn accept(&mut self, frame: &HelperFrameV1) -> Result<(), WireError> {
        match frame {
            HelperFrameV1::Hello(_) if !self.hello => {
                self.hello = true;
                Ok(())
            }
            HelperFrameV1::Hook(HookV1 { seq, .. }) | HelperFrameV1::Loss(LossV1 { seq, .. })
                if self.hello && Some(*seq) == self.last.checked_add(1) =>
            {
                self.last = *seq;
                Ok(())
            }
            _ => Err(WireError::Protocol),
        }
    }

    /// The last accepted sequence number (0 before the first hook or loss).
    pub fn last(&self) -> u64 {
        self.last
    }
}
