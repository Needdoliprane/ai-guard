//! Session protocol v3 (G11 lot K3, requirement R4): the closed framing that
//! follows the unchanged v2 Hello/Ready on the single `pods/attach` stdio
//! stream of one session Pod (maintainer decisions of 21 November 2026: no
//! exec, no sidecar, `stdinOnce`, no re-attach; provider network traffic is
//! carried inside the attach as opaque egress bytes and terminated in the
//! broker).
//!
//! The framing is transport-neutral: it runs on any byte stream. The broker
//! runs it over the attach relay of `session_attach`, the worker over its
//! process standard streams (`session_supervisor`), and the tests over pipes.
//!
//! # Frame
//!
//! `u32` big-endian length `L`, then one channel byte, then `L - 1` payload
//! bytes. The length counts the channel byte, so `L >= 1`. The declared length
//! is judged before any allocation, then the channel byte against its own
//! bound, and the payload buffer is reserved exactly (`try_reserve_exact`).
//!
//! | Channel | Byte | Direction | Payload bound |
//! |---|---|---|---|
//! | CONTROL | 0 | both | closed JSON, 1..=16 KiB; the single Handoff 1..=256 KiB |
//! | STDIN | 1 | broker to worker | 1..=64 KiB |
//! | STDOUT | 2 | worker to broker | 1..=64 KiB |
//! | STDERR | 3 | worker to broker | 1..=64 KiB |
//! | EGRESS | 4 | both | 1..=64 KiB, opaque bytes of the G05 framing, never parsed here |
//! | ARCHIVE | 5 | broker to worker (session), worker to broker (export) | 1..=64 KiB |
//!
//! An empty data payload, an unknown channel, a channel used in the wrong
//! direction or phase, an unknown or duplicate JSON member, a non-canonical
//! control encoding and any frame out of order is a protocol refusal. The
//! Handoff bound is larger because it carries the canonical profile (at most
//! 64 KiB), the provider credential payload (at most 64 KiB) and the business
//! invocation (at most 32 KiB of arguments); the receiver admits it only as
//! the first frame of a session, so at most one frame of that size is ever
//! held. These are configured software bounds, not measured capacities.
//!
//! # Order
//!
//! Session mode (`kubernetes-session-v3`), after Ready:
//!
//! 1. broker: CONTROL `handoff`, then exactly `archive_bytes` of ARCHIVE;
//! 2. worker: CONTROL `imported` (or `refused`, then exit);
//! 3. broker: CONTROL `start`;
//! 4. worker: CONTROL `started` (or `refused`);
//! 5. broker: STDIN, EGRESS, at most one `stdin_close`, at most one `stop`;
//!    worker: STDOUT, STDERR, EGRESS;
//! 6. worker: CONTROL `exited`, then the stream ends.
//!
//! `stop` is also accepted after the Handoff and before `start`: the worker
//! then answers `refused` with the reason `stopped`. Export mode
//! (`kubernetes-workspace-export-v3`): the broker sends `start` (or `stop`),
//! the worker streams ARCHIVE then CONTROL `archive_end` (or `refused`).
//! Nothing is sent after Ready until the broker speaks, in both modes, so the
//! attach bracket of the broker stays the same as for the preamble.
//!
//! # Provenance
//!
//! Every worker message is **declared** by the worker process. `exited` is
//! not the observed container terminal state, `imported` is not an observed
//! workspace and `archive_end` is checked by the broker against the bytes it
//! received, nothing more. Only the broker's own Kubernetes observations are
//! observed facts.
//!
//! # Secrets
//!
//! The Handoff carries the provider credential payload. Its type has no
//! `Debug` content and no accessor that formats it; this module overwrites the
//! buffers it owns (the decoded value, the frame buffer, the encoded frame)
//! before dropping them. Copies made by the JSON decoder's scratch space, by
//! the provider preparation and by the child process environment are not
//! overwritten: this is best-effort hygiene, not a memory protection.
//! Nothing in this module logs or echoes a payload.
#![cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "protocol v3 has no production broker caller before the pilot composes the effect permit (G11)"
    )
)]
use super::{session_profile::KubernetesSessionProfileV2, valid_digest};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fmt,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream},
    sync::{mpsc, watch},
    task::JoinHandle,
};

pub(crate) const PROTOCOL_VERSION: u32 = 3;
/// Length prefix and channel byte.
pub(crate) const FRAME_HEADER_BYTES: usize = 5;
pub(crate) const MAX_CONTROL_BYTES: usize = 16 * 1024;
pub(crate) const MAX_HANDOFF_BYTES: usize = 256 * 1024;
pub(crate) const MAX_DATA_BYTES: usize = 64 * 1024;
/// Same bound as `session_profile`'s canonical profile.
pub(crate) const MAX_PROFILE_BYTES: usize = 64 * 1024;
/// Same bound as `provider::bootstrap::prepare_execution`.
pub(crate) const MAX_BOOTSTRAP_PAYLOAD_BYTES: usize = 64 * 1024;
const MAX_JSON_DEPTH: usize = 8;
/// Frames queued towards the writer of a broker channel.
const OUTGOING_FRAMES: usize = 4;
/// Events queued towards the caller of a broker channel.
const EVENT_FRAMES: usize = 16;
/// In-memory pipe between the channel and the G05 egress owner.
pub(crate) const EGRESS_PIPE_BYTES: usize = 2 * MAX_DATA_BYTES;
/// Bound of the writer's drain on `close`.
const CLOSE_TIME: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionProtocolError {
    /// A declared length or payload outside its bound.
    FrameSize,
    /// The stream ended inside a frame.
    PartialFrame,
    /// Unknown channel, invalid or non-canonical JSON, frame out of order.
    Protocol,
    /// A declared result differs from what this side committed or received.
    Mismatch,
    /// A buffer could not be reserved.
    Capacity,
    Io,
    Deadline,
    /// The channel ended, or this side already ended it.
    Closed,
}
impl fmt::Display for SessionProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::FrameSize => "session protocol frame exceeds its bound",
            Self::PartialFrame => "session protocol frame is incomplete",
            Self::Protocol => "session protocol frame is refused",
            Self::Mismatch => "session protocol declared result differs from its commitment",
            Self::Capacity => "session protocol buffer is unavailable",
            Self::Io => "session protocol IO failed",
            Self::Deadline => "session protocol deadline elapsed",
            Self::Closed => "session protocol channel is closed",
        })
    }
}
impl std::error::Error for SessionProtocolError {}
type Result<T> = std::result::Result<T, SessionProtocolError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChannelV3 {
    Control,
    Stdin,
    Stdout,
    Stderr,
    Egress,
    Archive,
}
impl ChannelV3 {
    pub(crate) fn byte(self) -> u8 {
        match self {
            Self::Control => 0,
            Self::Stdin => 1,
            Self::Stdout => 2,
            Self::Stderr => 3,
            Self::Egress => 4,
            Self::Archive => 5,
        }
    }
    fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            0 => Self::Control,
            1 => Self::Stdin,
            2 => Self::Stdout,
            3 => Self::Stderr,
            4 => Self::Egress,
            5 => Self::Archive,
            _ => return None,
        })
    }
}

/// Which v3 mode the worker runs; fixed by the Pod argv, never by a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionModeV3 {
    Session,
    Export,
}

/// Overwrites a byte buffer before it is dropped.
pub(crate) fn scrub_bytes(bytes: &mut [u8]) {
    for byte in bytes.iter_mut() {
        // SAFETY: a valid, aligned, exclusively borrowed byte.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}
fn scrub_string(text: &mut String) {
    // SAFETY: zero bytes keep the string valid UTF-8.
    scrub_bytes(unsafe { text.as_mut_vec() });
}
/// Overwrites every string of a decoded JSON value before it is dropped.
fn scrub_value(value: &mut Value) {
    match value {
        Value::String(text) => scrub_string(text),
        Value::Array(values) => values.iter_mut().for_each(scrub_value),
        Value::Object(fields) => fields.values_mut().for_each(scrub_value),
        _ => {}
    }
}

/// Secret text. No `Debug` content, no `Display`, overwritten on drop.
#[derive(Default, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct SecretTextV3(String);
impl SecretTextV3 {
    pub(crate) fn new(text: &str) -> Self {
        Self(text.to_owned())
    }
    /// The secret, for the one consumer that needs it. Never format it.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
    fn len(&self) -> usize {
        self.0.len()
    }
}
impl Drop for SecretTextV3 {
    fn drop(&mut self) {
        scrub_string(&mut self.0);
    }
}
impl fmt::Debug for SecretTextV3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretTextV3(<private>)")
    }
}

/// One decoded frame. No `Debug` of the payload.
pub(crate) struct FrameV3 {
    pub(crate) channel: ChannelV3,
    pub(crate) payload: Vec<u8>,
}
impl FrameV3 {
    /// Overwrites the payload; used for control frames, which may carry the
    /// Handoff.
    pub(crate) fn scrub(mut self) {
        scrub_bytes(&mut self.payload);
    }
}
impl fmt::Debug for FrameV3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameV3")
            .field("channel", &self.channel)
            .field("bytes", &self.payload.len())
            .finish()
    }
}

fn payload_bound(channel: ChannelV3, control_limit: usize) -> usize {
    match channel {
        ChannelV3::Control => control_limit.min(MAX_HANDOFF_BYTES),
        _ => MAX_DATA_BYTES,
    }
}

/// Encodes one frame; refuses an empty payload or one above its bound.
pub(crate) fn encode_frame(channel: ChannelV3, payload: &[u8]) -> Result<Vec<u8>> {
    encode_bounded(channel, payload, MAX_CONTROL_BYTES)
}
fn encode_bounded(channel: ChannelV3, payload: &[u8], control_limit: usize) -> Result<Vec<u8>> {
    if payload.is_empty() || payload.len() > payload_bound(channel, control_limit) {
        return Err(SessionProtocolError::FrameSize);
    }
    let length = u32::try_from(payload.len() + 1).map_err(|_| SessionProtocolError::FrameSize)?;
    let mut frame = Vec::new();
    frame
        .try_reserve_exact(FRAME_HEADER_BYTES + payload.len())
        .map_err(|_| SessionProtocolError::Capacity)?;
    frame.extend_from_slice(&length.to_be_bytes());
    frame.push(channel.byte());
    frame.extend_from_slice(payload);
    Ok(frame)
}

/// Cancellation-safe frame reader: the bytes of a frame read so far stay in
/// the reader when a `read` future is dropped, and the next call continues.
/// After an error the reader refuses every later call.
#[derive(Default)]
pub(crate) struct FrameReaderV3 {
    header: [u8; FRAME_HEADER_BYTES],
    header_used: usize,
    channel: Option<ChannelV3>,
    body: Vec<u8>,
    body_used: usize,
    failed: bool,
}
impl FrameReaderV3 {
    pub(crate) fn new() -> Self {
        Self::default()
    }
    /// True between two frames: no byte of a next frame was read.
    pub(crate) fn at_boundary(&self) -> bool {
        self.header_used == 0
    }

    /// The next frame. `Ok(None)` is a clean end of the stream between two
    /// frames; an end inside a frame is `PartialFrame`. `control_limit` is
    /// the CONTROL payload bound admitted for this frame.
    pub(crate) async fn read<R: AsyncRead + Unpin>(
        &mut self,
        input: &mut R,
        control_limit: usize,
    ) -> Result<Option<FrameV3>> {
        if self.failed {
            return Err(SessionProtocolError::Closed);
        }
        let result = self.read_inner(input, control_limit).await;
        if result.is_err() {
            self.failed = true;
            scrub_bytes(&mut self.body);
            self.body = Vec::new();
        }
        result
    }

    async fn read_inner<R: AsyncRead + Unpin>(
        &mut self,
        input: &mut R,
        control_limit: usize,
    ) -> Result<Option<FrameV3>> {
        while self.header_used < 4 {
            let count = input
                .read(&mut self.header[self.header_used..4])
                .await
                .map_err(|_| SessionProtocolError::Io)?;
            if count == 0 {
                return if self.header_used == 0 {
                    Ok(None)
                } else {
                    Err(SessionProtocolError::PartialFrame)
                };
            }
            self.header_used += count;
        }
        let declared = u32::from_be_bytes([
            self.header[0],
            self.header[1],
            self.header[2],
            self.header[3],
        ]) as usize;
        // The largest admissible frame, before the channel is known.
        if declared < 2 || declared > MAX_HANDOFF_BYTES + 1 {
            return Err(SessionProtocolError::FrameSize);
        }
        if self.header_used < FRAME_HEADER_BYTES {
            let count = input
                .read(&mut self.header[4..FRAME_HEADER_BYTES])
                .await
                .map_err(|_| SessionProtocolError::Io)?;
            if count == 0 {
                return Err(SessionProtocolError::PartialFrame);
            }
            self.header_used += count;
        }
        let channel = match self.channel {
            Some(channel) => channel,
            None => {
                let channel =
                    ChannelV3::from_byte(self.header[4]).ok_or(SessionProtocolError::Protocol)?;
                let length = declared - 1;
                if length > payload_bound(channel, control_limit) {
                    return Err(SessionProtocolError::FrameSize);
                }
                self.body = Vec::new();
                self.body
                    .try_reserve_exact(length)
                    .map_err(|_| SessionProtocolError::Capacity)?;
                self.body.resize(length, 0);
                self.body_used = 0;
                self.channel = Some(channel);
                channel
            }
        };
        while self.body_used < self.body.len() {
            let count = input
                .read(&mut self.body[self.body_used..])
                .await
                .map_err(|_| SessionProtocolError::Io)?;
            if count == 0 {
                return Err(SessionProtocolError::PartialFrame);
            }
            self.body_used += count;
        }
        self.header_used = 0;
        self.channel = None;
        self.body_used = 0;
        Ok(Some(FrameV3 {
            channel,
            payload: std::mem::take(&mut self.body),
        }))
    }
}

// ---- control messages -------------------------------------------------------

/// Closed reason of a broker Stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StopReasonV3 {
    Cancelled,
    Deadline,
    Revoked,
}

/// Closed reason of a worker refusal. No peer or local text is carried.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RefusalV3 {
    /// A frame refused by the order or the codec.
    Protocol,
    /// A handoff, import or start window elapsed.
    Deadline,
    /// The profile is invalid, its hash differs from the Hello, or its
    /// binding differs from the Hello identifiers.
    ProfileMismatch,
    /// The business invocation differs from the profile commitment.
    InvocationMismatch,
    /// The credential payload does not match the invocation, or the private
    /// provider home is unusable.
    CredentialInvalid,
    /// The resolved transport is interactive; no PTY exists in this slice.
    InteractiveUnsupported,
    /// The archive size or digest differs from the Handoff.
    ArchiveMismatch,
    /// The archive holds a link, a special file, a traversal, a conflict or
    /// exceeds the workspace limits.
    WorkspaceInvalid,
    /// The archive manifest differs from the profile commitment.
    WorkspaceMismatch,
    /// The workspace directory is not empty before the import.
    WorkspaceNotEmpty,
    /// Writing the workspace failed locally.
    ImportFailed,
    /// The provider could not be prepared or started.
    ProviderUnavailable,
    /// The egress helper could not be started.
    EgressUnavailable,
    /// The workspace holds an entry that is not a regular file or a directory.
    ExportUnsupportedEntry,
    /// The workspace exceeds the archive limits.
    ExportLimit,
    /// Reading the workspace failed locally.
    ExportFailed,
    /// A broker Stop arrived before Start.
    Stopped,
}

/// The single Handoff of a session. Built by the broker, decoded by the
/// worker. The credential payload is secret; the business invocation is
/// private (it holds the prompt) and is never formatted either.
pub(crate) struct HandoffV3 {
    profile: String,
    bootstrap: SecretTextV3,
    program: String,
    args: Vec<String>,
    archive_bytes: u64,
    archive_sha256: String,
    /// Broker side only: the import result committed by the profile, never
    /// sent.
    commitment: Option<ImportedV3>,
}
impl fmt::Debug for HandoffV3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HandoffV3(<private>)")
    }
}
impl Drop for HandoffV3 {
    fn drop(&mut self) {
        for argument in &mut self.args {
            scrub_string(argument);
        }
    }
}
impl HandoffV3 {
    /// Broker side. `profile` supplies its canonical bytes; the worker
    /// re-hashes them against its Hello. `bootstrap` is the provider
    /// credential JSON that the Docker path passes through
    /// `AI_GUARD_PROVIDER_BOOTSTRAP`.
    pub(crate) fn new(
        profile: &KubernetesSessionProfileV2,
        bootstrap: &str,
        program: &str,
        args: &[String],
        archive_bytes: u64,
        archive_sha256: &str,
    ) -> Result<Self> {
        let canonical = profile
            .canonical_bytes()
            .map_err(|_| SessionProtocolError::Protocol)?;
        let (manifest_sha256, files, bytes) = profile.retained_workspace_commitment();
        let commitment = Some(ImportedV3 {
            manifest_sha256: manifest_sha256.to_owned(),
            files,
            bytes,
        });
        let profile = String::from_utf8(canonical).map_err(|_| SessionProtocolError::Protocol)?;
        let handoff = Self {
            profile,
            bootstrap: SecretTextV3::new(bootstrap),
            program: program.to_owned(),
            args: args.to_vec(),
            archive_bytes,
            archive_sha256: archive_sha256.to_owned(),
            commitment,
        };
        handoff.validate()?;
        Ok(handoff)
    }
    fn validate(&self) -> Result<()> {
        let limits = crate::workspace::Limits::default();
        if self.profile.is_empty()
            || self.profile.len() > MAX_PROFILE_BYTES
            || self.bootstrap.len() == 0
            || self.bootstrap.len() > MAX_BOOTSTRAP_PAYLOAD_BYTES
            || self.program != crate::provider::BOOTSTRAP_PROGRAM
            || self.args.len() > 256
            || self.archive_bytes > limits.max_archive_bytes() as u64
            || !valid_digest(&self.archive_sha256)
        {
            return Err(SessionProtocolError::Protocol);
        }
        Ok(())
    }
    pub(crate) fn profile_bytes(&self) -> &[u8] {
        self.profile.as_bytes()
    }
    pub(crate) fn bootstrap(&self) -> &SecretTextV3 {
        &self.bootstrap
    }
    pub(crate) fn program(&self) -> &str {
        &self.program
    }
    pub(crate) fn args(&self) -> &[String] {
        &self.args
    }
    pub(crate) fn archive_bytes(&self) -> u64 {
        self.archive_bytes
    }
    pub(crate) fn archive_sha256(&self) -> &str {
        &self.archive_sha256
    }
}

/// Declared import result: the commitment the worker verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImportedV3 {
    pub(crate) manifest_sha256: String,
    pub(crate) files: u64,
    pub(crate) bytes: u64,
}
/// Declared provider end: exactly one of `code` and `signal`. Not the
/// observed container terminal state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ExitedV3 {
    pub(crate) code: Option<i32>,
    pub(crate) signal: Option<i32>,
}
/// Declared end of an export stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArchiveEndV3 {
    pub(crate) archive_bytes: u64,
    pub(crate) archive_sha256: String,
}

#[derive(Debug)]
pub(crate) enum BrokerControlV3 {
    Handoff(Box<HandoffV3>),
    Start,
    StdinClose,
    Stop(StopReasonV3),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkerControlV3 {
    Imported(ImportedV3),
    Started,
    Exited(ExitedV3),
    ArchiveEnd(ArchiveEndV3),
    Refused(RefusalV3),
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum BrokerWire {
    Handoff {
        version: u32,
        profile: String,
        bootstrap: SecretTextV3,
        program: String,
        args: Vec<String>,
        archive_bytes: u64,
        archive_sha256: String,
    },
    Start {
        version: u32,
    },
    StdinClose {
        version: u32,
    },
    Stop {
        version: u32,
        reason: StopReasonV3,
    },
}
#[derive(Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum WorkerWire {
    Imported {
        version: u32,
        manifest_sha256: String,
        files: u64,
        bytes: u64,
    },
    Started {
        version: u32,
    },
    Exited {
        version: u32,
        code: Option<i32>,
        signal: Option<i32>,
    },
    ArchiveEnd {
        version: u32,
        archive_bytes: u64,
        archive_sha256: String,
    },
    Refused {
        version: u32,
        reason: RefusalV3,
    },
}

fn valid_exit(code: Option<i32>, signal: Option<i32>) -> bool {
    match (code, signal) {
        (Some(code), None) => (0..=255).contains(&code),
        (None, Some(signal)) => (1..=64).contains(&signal),
        _ => false,
    }
}

impl WorkerControlV3 {
    fn wire(&self) -> WorkerWire {
        let version = PROTOCOL_VERSION;
        match self {
            Self::Imported(imported) => WorkerWire::Imported {
                version,
                manifest_sha256: imported.manifest_sha256.clone(),
                files: imported.files,
                bytes: imported.bytes,
            },
            Self::Started => WorkerWire::Started { version },
            Self::Exited(exited) => WorkerWire::Exited {
                version,
                code: exited.code,
                signal: exited.signal,
            },
            Self::ArchiveEnd(end) => WorkerWire::ArchiveEnd {
                version,
                archive_bytes: end.archive_bytes,
                archive_sha256: end.archive_sha256.clone(),
            },
            Self::Refused(reason) => WorkerWire::Refused {
                version,
                reason: *reason,
            },
        }
    }
    fn validate(&self) -> Result<()> {
        let valid = match self {
            Self::Imported(imported) => valid_digest(&imported.manifest_sha256),
            Self::Exited(exited) => valid_exit(exited.code, exited.signal),
            Self::ArchiveEnd(end) => valid_digest(&end.archive_sha256),
            Self::Started | Self::Refused(_) => true,
        };
        if valid {
            Ok(())
        } else {
            Err(SessionProtocolError::Protocol)
        }
    }
    /// One CONTROL frame.
    pub(crate) fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let json = serde_json::to_vec(&self.wire()).map_err(|_| SessionProtocolError::Protocol)?;
        encode_frame(ChannelV3::Control, &json)
    }
    /// Closed decoding of one CONTROL payload. Duplicate members, unknown
    /// members, another version and any non-canonical spelling are refused.
    pub(crate) fn decode(payload: &[u8]) -> Result<Self> {
        if payload.is_empty() || payload.len() > MAX_CONTROL_BYTES {
            return Err(SessionProtocolError::FrameSize);
        }
        let value = crate::json::decode_unique(payload, MAX_JSON_DEPTH)
            .map_err(|_| SessionProtocolError::Protocol)?;
        let wire: WorkerWire =
            serde_json::from_value(value.clone()).map_err(|_| SessionProtocolError::Protocol)?;
        if serde_json::to_value(&wire).map_err(|_| SessionProtocolError::Protocol)? != value {
            return Err(SessionProtocolError::Protocol);
        }
        let (version, message) = match wire {
            WorkerWire::Imported {
                version,
                manifest_sha256,
                files,
                bytes,
            } => (
                version,
                Self::Imported(ImportedV3 {
                    manifest_sha256,
                    files,
                    bytes,
                }),
            ),
            WorkerWire::Started { version } => (version, Self::Started),
            WorkerWire::Exited {
                version,
                code,
                signal,
            } => (version, Self::Exited(ExitedV3 { code, signal })),
            WorkerWire::ArchiveEnd {
                version,
                archive_bytes,
                archive_sha256,
            } => (
                version,
                Self::ArchiveEnd(ArchiveEndV3 {
                    archive_bytes,
                    archive_sha256,
                }),
            ),
            WorkerWire::Refused { version, reason } => (version, Self::Refused(reason)),
        };
        if version != PROTOCOL_VERSION {
            return Err(SessionProtocolError::Protocol);
        }
        message.validate()?;
        Ok(message)
    }
}

impl BrokerControlV3 {
    /// One CONTROL frame. The encoded bytes of a Handoff hold the secret:
    /// callers scrub them after writing (`scrub_bytes`).
    pub(crate) fn encode(&self) -> Result<Vec<u8>> {
        let version = PROTOCOL_VERSION;
        let (mut json, limit) = match self {
            Self::Handoff(handoff) => {
                handoff.validate()?;
                // Borrowed fields: no second copy of the secret is built.
                #[derive(Serialize)]
                #[serde(tag = "kind", rename = "handoff")]
                struct HandoffRef<'a> {
                    version: u32,
                    profile: &'a str,
                    bootstrap: &'a str,
                    program: &'a str,
                    args: &'a [String],
                    archive_bytes: u64,
                    archive_sha256: &'a str,
                }
                let json = serde_json::to_vec(&HandoffRef {
                    version,
                    profile: &handoff.profile,
                    bootstrap: handoff.bootstrap.expose(),
                    program: &handoff.program,
                    args: &handoff.args,
                    archive_bytes: handoff.archive_bytes,
                    archive_sha256: &handoff.archive_sha256,
                })
                .map_err(|_| SessionProtocolError::Protocol)?;
                (json, MAX_HANDOFF_BYTES)
            }
            Self::Start => (
                serde_json::to_vec(&BrokerWire::Start { version })
                    .map_err(|_| SessionProtocolError::Protocol)?,
                MAX_CONTROL_BYTES,
            ),
            Self::StdinClose => (
                serde_json::to_vec(&BrokerWire::StdinClose { version })
                    .map_err(|_| SessionProtocolError::Protocol)?,
                MAX_CONTROL_BYTES,
            ),
            Self::Stop(reason) => (
                serde_json::to_vec(&BrokerWire::Stop {
                    version,
                    reason: *reason,
                })
                .map_err(|_| SessionProtocolError::Protocol)?,
                MAX_CONTROL_BYTES,
            ),
        };
        let frame = encode_bounded(ChannelV3::Control, &json, limit);
        scrub_bytes(&mut json);
        frame
    }

    /// Closed decoding of one CONTROL payload. A Handoff is admitted only up
    /// to `MAX_HANDOFF_BYTES`, every other message up to `MAX_CONTROL_BYTES`.
    /// The decoded value is overwritten before it is dropped; the caller
    /// scrubs the frame buffer.
    pub(crate) fn decode(payload: &[u8]) -> Result<Self> {
        if payload.is_empty() || payload.len() > MAX_HANDOFF_BYTES {
            return Err(SessionProtocolError::FrameSize);
        }
        let mut value = crate::json::decode_unique(payload, MAX_JSON_DEPTH)
            .map_err(|_| SessionProtocolError::Protocol)?;
        let is_handoff = value.get("kind").and_then(Value::as_str) == Some("handoff");
        if !is_handoff && payload.len() > MAX_CONTROL_BYTES {
            scrub_value(&mut value);
            return Err(SessionProtocolError::FrameSize);
        }
        if is_handoff {
            // No round-trip copy of the secret: the strings are moved into
            // the typed value, which validates every field itself.
            let wire: BrokerWire =
                serde_json::from_value(value).map_err(|_| SessionProtocolError::Protocol)?;
            return match wire {
                BrokerWire::Handoff {
                    version: PROTOCOL_VERSION,
                    profile,
                    bootstrap,
                    program,
                    args,
                    archive_bytes,
                    archive_sha256,
                } => {
                    let handoff = HandoffV3 {
                        profile,
                        bootstrap,
                        program,
                        args,
                        archive_bytes,
                        archive_sha256,
                        commitment: None,
                    };
                    handoff.validate()?;
                    Ok(Self::Handoff(Box::new(handoff)))
                }
                _ => Err(SessionProtocolError::Protocol),
            };
        }
        let wire: BrokerWire =
            serde_json::from_value(value.clone()).map_err(|_| SessionProtocolError::Protocol)?;
        if serde_json::to_value(&wire).map_err(|_| SessionProtocolError::Protocol)? != value {
            return Err(SessionProtocolError::Protocol);
        }
        match wire {
            BrokerWire::Start {
                version: PROTOCOL_VERSION,
            } => Ok(Self::Start),
            BrokerWire::StdinClose {
                version: PROTOCOL_VERSION,
            } => Ok(Self::StdinClose),
            BrokerWire::Stop {
                version: PROTOCOL_VERSION,
                reason,
            } => Ok(Self::Stop(reason)),
            _ => Err(SessionProtocolError::Protocol),
        }
    }
}

// ---- order ------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerPhase {
    AwaitHandoff,
    Archive { remaining: u64 },
    Importing,
    AwaitStart,
    Running { stdin_open: bool },
    Exporting,
    Stopped,
}

/// What a worker accepts next from its broker. Strict: every frame that the
/// phase does not name is a protocol refusal, and the phase does not move.
#[derive(Clone, Debug)]
pub(crate) struct WorkerInboundOrderV3 {
    mode: SessionModeV3,
    phase: WorkerPhase,
}
impl WorkerInboundOrderV3 {
    pub(crate) fn new(mode: SessionModeV3) -> Self {
        Self {
            mode,
            phase: match mode {
                SessionModeV3::Session => WorkerPhase::AwaitHandoff,
                SessionModeV3::Export => WorkerPhase::AwaitStart,
            },
        }
    }
    /// CONTROL payload bound admitted for the next frame.
    pub(crate) fn control_limit(&self) -> usize {
        if self.phase == WorkerPhase::AwaitHandoff {
            MAX_HANDOFF_BYTES
        } else {
            MAX_CONTROL_BYTES
        }
    }
    /// All archive bytes of the Handoff were received.
    pub(crate) fn archive_complete(&self) -> bool {
        self.phase == WorkerPhase::Importing
    }
    /// Admits one data frame of `bytes` on `channel`.
    pub(crate) fn admit_data(&mut self, channel: ChannelV3, bytes: usize) -> Result<()> {
        let bytes = bytes as u64;
        self.phase = match (self.phase, channel) {
            (WorkerPhase::Archive { remaining }, ChannelV3::Archive) if bytes <= remaining => {
                if remaining == bytes {
                    WorkerPhase::Importing
                } else {
                    WorkerPhase::Archive {
                        remaining: remaining - bytes,
                    }
                }
            }
            (WorkerPhase::Running { stdin_open: true }, ChannelV3::Stdin)
            | (WorkerPhase::Running { .. }, ChannelV3::Egress) => self.phase,
            _ => return Err(SessionProtocolError::Protocol),
        };
        Ok(())
    }
    /// Admits one control message.
    pub(crate) fn admit_control(&mut self, message: &BrokerControlV3) -> Result<()> {
        self.phase = match (self.mode, self.phase, message) {
            (SessionModeV3::Session, WorkerPhase::AwaitHandoff, BrokerControlV3::Handoff(h)) => {
                if h.archive_bytes == 0 {
                    WorkerPhase::Importing
                } else {
                    WorkerPhase::Archive {
                        remaining: h.archive_bytes,
                    }
                }
            }
            (
                _,
                WorkerPhase::Archive { .. }
                | WorkerPhase::Importing
                | WorkerPhase::AwaitStart
                | WorkerPhase::Running { .. }
                | WorkerPhase::Exporting,
                BrokerControlV3::Stop(_),
            ) => WorkerPhase::Stopped,
            (SessionModeV3::Session, WorkerPhase::AwaitStart, BrokerControlV3::Start) => {
                WorkerPhase::Running { stdin_open: true }
            }
            (SessionModeV3::Export, WorkerPhase::AwaitStart, BrokerControlV3::Start) => {
                WorkerPhase::Exporting
            }
            (_, WorkerPhase::Running { stdin_open: true }, BrokerControlV3::StdinClose) => {
                WorkerPhase::Running { stdin_open: false }
            }
            _ => return Err(SessionProtocolError::Protocol),
        };
        Ok(())
    }
    /// The worker sent Imported.
    pub(crate) fn imported(&mut self) -> Result<()> {
        if self.phase != WorkerPhase::Importing {
            return Err(SessionProtocolError::Protocol);
        }
        self.phase = WorkerPhase::AwaitStart;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum BrokerPhase {
    Fresh,
    /// Handoff sent; archive bytes remain to send.
    Archive {
        remaining: u64,
    },
    /// Whole archive sent; Imported or Refused expected.
    AwaitImported,
    /// Imported received; Start or Stop may be sent.
    Imported,
    /// Start sent (session); Started or Refused expected.
    AwaitStarted,
    Running,
    /// Export Start sent; ARCHIVE then ArchiveEnd or Refused expected.
    Exporting,
    /// Stop sent; the worker's end is expected.
    Stopping {
        started: bool,
    },
    Ended,
}

/// Order of one broker channel: what may be sent and what is accepted.
#[derive(Debug)]
pub(crate) struct BrokerOrderV3 {
    mode: SessionModeV3,
    phase: BrokerPhase,
    stdin_open: bool,
    /// Committed import result, from the profile of the Handoff.
    expected_import: Option<ImportedV3>,
    archive_sha256: Option<String>,
    archive_hash: Sha256,
    received_archive: u64,
}
impl BrokerOrderV3 {
    pub(crate) fn new(mode: SessionModeV3) -> Self {
        Self {
            mode,
            phase: BrokerPhase::Fresh,
            stdin_open: true,
            expected_import: None,
            archive_sha256: None,
            archive_hash: Sha256::new(),
            received_archive: 0,
        }
    }
    fn refuse<T>() -> Result<T> {
        Err(SessionProtocolError::Protocol)
    }

    /// Checks and records one outgoing control message. A Handoff must be
    /// the broker-built one, which retains its committed import result.
    pub(crate) fn send_control(&mut self, message: &BrokerControlV3) -> Result<()> {
        if self.phase == BrokerPhase::Ended {
            return Err(SessionProtocolError::Closed);
        }
        let next = match (self.mode, &self.phase, message) {
            (SessionModeV3::Session, BrokerPhase::Fresh, BrokerControlV3::Handoff(handoff)) => {
                self.expected_import = Some(
                    handoff
                        .commitment
                        .clone()
                        .ok_or(SessionProtocolError::Protocol)?,
                );
                self.archive_sha256 = Some(handoff.archive_sha256.clone());
                self.archive_hash = Sha256::new();
                if handoff.archive_bytes == 0 {
                    self.check_sent_archive()?;
                    BrokerPhase::AwaitImported
                } else {
                    BrokerPhase::Archive {
                        remaining: handoff.archive_bytes,
                    }
                }
            }
            (SessionModeV3::Session, BrokerPhase::Imported, BrokerControlV3::Start) => {
                BrokerPhase::AwaitStarted
            }
            (SessionModeV3::Export, BrokerPhase::Fresh, BrokerControlV3::Start) => {
                BrokerPhase::Exporting
            }
            (_, BrokerPhase::AwaitStarted | BrokerPhase::Running, BrokerControlV3::StdinClose)
                if self.stdin_open =>
            {
                self.stdin_open = false;
                self.phase.clone()
            }
            (
                _,
                BrokerPhase::Archive { .. } | BrokerPhase::AwaitImported | BrokerPhase::Imported,
                BrokerControlV3::Stop(_),
            ) => BrokerPhase::Stopping { started: false },
            (
                _,
                BrokerPhase::AwaitStarted | BrokerPhase::Running | BrokerPhase::Exporting,
                BrokerControlV3::Stop(_),
            ) => BrokerPhase::Stopping { started: true },
            _ => return Self::refuse(),
        };
        self.phase = next;
        Ok(())
    }

    fn check_sent_archive(&mut self) -> Result<()> {
        let digest = hex::encode(std::mem::take(&mut self.archive_hash).finalize());
        if self.archive_sha256.as_deref() != Some(digest.as_str()) {
            return Err(SessionProtocolError::Mismatch);
        }
        Ok(())
    }

    /// Checks and records one outgoing data frame.
    pub(crate) fn send_data(&mut self, channel: ChannelV3, payload: &[u8]) -> Result<()> {
        if self.phase == BrokerPhase::Ended {
            return Err(SessionProtocolError::Closed);
        }
        let bytes = payload.len() as u64;
        match (&self.phase, channel) {
            (BrokerPhase::Archive { remaining }, ChannelV3::Archive) if bytes <= *remaining => {
                let remaining = remaining - bytes;
                self.archive_hash.update(payload);
                if remaining == 0 {
                    self.check_sent_archive()?;
                    self.phase = BrokerPhase::AwaitImported;
                } else {
                    self.phase = BrokerPhase::Archive { remaining };
                }
                Ok(())
            }
            (BrokerPhase::AwaitStarted | BrokerPhase::Running, ChannelV3::Stdin)
                if self.stdin_open =>
            {
                Ok(())
            }
            (BrokerPhase::AwaitStarted | BrokerPhase::Running, ChannelV3::Egress) => Ok(()),
            _ => Self::refuse(),
        }
    }

    /// True while broker EGRESS bytes may be sent.
    pub(crate) fn egress_open(&self) -> bool {
        matches!(self.phase, BrokerPhase::AwaitStarted | BrokerPhase::Running)
    }

    /// Checks one incoming data frame.
    pub(crate) fn receive_data(&mut self, channel: ChannelV3, payload: &[u8]) -> Result<()> {
        let running = matches!(
            self.phase,
            BrokerPhase::Running | BrokerPhase::Stopping { started: true }
        );
        match channel {
            ChannelV3::Stdout | ChannelV3::Stderr | ChannelV3::Egress if running => Ok(()),
            ChannelV3::Archive
                if matches!(
                    self.phase,
                    BrokerPhase::Exporting | BrokerPhase::Stopping { started: true }
                ) && self.mode == SessionModeV3::Export =>
            {
                let total = self
                    .received_archive
                    .checked_add(payload.len() as u64)
                    .filter(|total| {
                        *total <= crate::workspace::Limits::default().max_archive_bytes() as u64
                    })
                    .ok_or(SessionProtocolError::FrameSize)?;
                self.received_archive = total;
                self.archive_hash.update(payload);
                Ok(())
            }
            _ => Self::refuse(),
        }
    }

    /// Checks one incoming control message.
    pub(crate) fn receive_control(&mut self, message: &WorkerControlV3) -> Result<()> {
        let next = match (self.mode, &self.phase, message) {
            // A refusal ends the worker at any point before `started`,
            // including its own windows (Handoff, start) and a Stop.
            (
                _,
                BrokerPhase::Fresh
                | BrokerPhase::Archive { .. }
                | BrokerPhase::AwaitImported
                | BrokerPhase::Imported
                | BrokerPhase::AwaitStarted
                | BrokerPhase::Exporting
                | BrokerPhase::Stopping { .. },
                WorkerControlV3::Refused(_),
            ) => BrokerPhase::Ended,
            (
                SessionModeV3::Session,
                BrokerPhase::AwaitImported,
                WorkerControlV3::Imported(got),
            ) => {
                if self.expected_import.as_ref() != Some(got) {
                    return Err(SessionProtocolError::Mismatch);
                }
                BrokerPhase::Imported
            }
            (SessionModeV3::Session, BrokerPhase::AwaitStarted, WorkerControlV3::Started) => {
                BrokerPhase::Running
            }
            (
                SessionModeV3::Session,
                BrokerPhase::Running | BrokerPhase::Stopping { started: true },
                WorkerControlV3::Exited(_),
            ) => BrokerPhase::Ended,
            // A Stop sent while Start was in flight: the worker may still
            // answer Started before its Exited.
            (
                SessionModeV3::Session,
                BrokerPhase::Stopping { started: true },
                WorkerControlV3::Started,
            ) => self.phase.clone(),
            (
                SessionModeV3::Export,
                BrokerPhase::Exporting | BrokerPhase::Stopping { started: true },
                WorkerControlV3::ArchiveEnd(end),
            ) => {
                let digest = hex::encode(std::mem::take(&mut self.archive_hash).finalize());
                if end.archive_bytes != self.received_archive || end.archive_sha256 != digest {
                    return Err(SessionProtocolError::Mismatch);
                }
                BrokerPhase::Ended
            }
            _ => return Self::refuse(),
        };
        self.phase = next;
        Ok(())
    }
    /// The worker's end was received: a clean end of stream is expected.
    pub(crate) fn ended(&self) -> bool {
        self.phase == BrokerPhase::Ended
    }
}

// ---- broker channel ---------------------------------------------------------

/// What the broker receives. Payload bytes have no `Debug`.
pub(crate) enum WorkerEventV3 {
    Control(WorkerControlV3),
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Archive(Vec<u8>),
}
impl fmt::Debug for WorkerEventV3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Control(message) => f.debug_tuple("Control").field(message).finish(),
            Self::Stdout(bytes) => write!(f, "Stdout({} bytes)", bytes.len()),
            Self::Stderr(bytes) => write!(f, "Stderr({} bytes)", bytes.len()),
            Self::Archive(bytes) => write!(f, "Archive({} bytes)", bytes.len()),
        }
    }
}

struct Outgoing {
    frame: Vec<u8>,
    secret: bool,
}
impl Drop for Outgoing {
    fn drop(&mut self) {
        if self.secret {
            scrub_bytes(&mut self.frame);
        }
    }
}

struct Shared {
    order: Mutex<BrokerOrderV3>,
}
impl Shared {
    fn order(&self) -> MutexGuard<'_, BrokerOrderV3> {
        self.order.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Broker side of one protocol v3 stream, over any byte stream (in
/// production: the attach relay of `session_attach`).
///
/// Three tasks own the IO: a writer of the outgoing frames, a reader that
/// checks every incoming frame against the order and routes it, and an
/// egress pump that frames what the G05 owner writes on the egress pipe once
/// Start was sent. Dropping the channel (both halves) aborts the three tasks
/// and drops the stream. Every queue is bounded: a caller that does not
/// consume its events stops the reader, and the stream applies backpressure
/// to the worker. Sends and event consumption must therefore be driven
/// concurrently by the caller.
pub(crate) struct SessionChannelV3 {
    sender: SessionSenderV3,
    events: SessionEventsV3,
    egress: Option<DuplexStream>,
}
/// Sending half. One owner: its order checks and its enqueues are serial.
pub(crate) struct SessionSenderV3 {
    shared: Arc<Shared>,
    send: Arc<tokio::sync::Mutex<Option<mpsc::Sender<Outgoing>>>>,
    gate: watch::Sender<bool>,
    pump: JoinHandle<()>,
    writer: Option<JoinHandle<()>>,
    _tasks: Arc<TaskSet>,
}
impl Drop for SessionSenderV3 {
    /// Dropped without `close`: the pump and the writer are aborted, and
    /// frames still queued are not written.
    fn drop(&mut self) {
        self.pump.abort();
        if let Some(writer) = self.writer.take() {
            writer.abort();
        }
    }
}
/// Receiving half.
pub(crate) struct SessionEventsV3 {
    events: mpsc::Receiver<Result<Option<WorkerEventV3>>>,
    done: bool,
    _tasks: Arc<TaskSet>,
}
struct TaskSet(Vec<JoinHandle<()>>);
impl Drop for TaskSet {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

impl SessionChannelV3 {
    /// Starts the broker side on `io`. Needs a Tokio runtime.
    pub(crate) fn broker<S>(io: S, mode: SessionModeV3) -> Result<Self>
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| SessionProtocolError::Closed)?;
        let (read, write) = tokio::io::split(io);
        let shared = Arc::new(Shared {
            order: Mutex::new(BrokerOrderV3::new(mode)),
        });
        let (outgoing, outgoing_receiver) = mpsc::channel::<Outgoing>(OUTGOING_FRAMES);
        let (event_sender, events) = mpsc::channel(EVENT_FRAMES);
        let (egress_owner, egress_channel) = tokio::io::duplex(EGRESS_PIPE_BYTES);
        let (egress_read, egress_write) = tokio::io::split(egress_channel);
        let (gate, gate_receiver) = watch::channel(false);
        let send = Arc::new(tokio::sync::Mutex::new(Some(outgoing)));
        let writer = runtime.spawn(write_frames(write, outgoing_receiver));
        let reader = runtime.spawn(read_frames(
            read,
            shared.clone(),
            event_sender,
            egress_write,
        ));
        let pump = runtime.spawn(pump_egress(
            egress_read,
            shared.clone(),
            send.clone(),
            gate_receiver,
        ));
        let tasks = Arc::new(TaskSet(vec![reader]));
        Ok(Self {
            sender: SessionSenderV3 {
                shared,
                send,
                gate,
                pump,
                writer: Some(writer),
                _tasks: tasks.clone(),
            },
            events: SessionEventsV3 {
                events,
                done: false,
                _tasks: tasks,
            },
            egress: Some(egress_owner),
        })
    }
    /// The byte stream that G05's egress owner runs on. Taken once.
    pub(crate) fn take_egress(&mut self) -> Option<DuplexStream> {
        self.egress.take()
    }
    /// The sending half, the receiving half and the egress stream if it was
    /// not taken.
    pub(crate) fn split(self) -> (SessionSenderV3, SessionEventsV3, Option<DuplexStream>) {
        (self.sender, self.events, self.egress)
    }
}

impl SessionSenderV3 {
    async fn enqueue(&mut self, frame: Vec<u8>, secret: bool) -> Result<()> {
        let outgoing = Outgoing { frame, secret };
        let guard = self.send.lock().await;
        let sender = guard.as_ref().ok_or(SessionProtocolError::Closed)?;
        sender
            .send(outgoing)
            .await
            .map_err(|_| SessionProtocolError::Closed)
    }

    /// Sends one control message after checking it against the order. The
    /// Handoff must come from `HandoffV3::new`, which retains the import
    /// result committed by its profile.
    pub(crate) async fn send_control(&mut self, message: BrokerControlV3) -> Result<()> {
        let secret = matches!(message, BrokerControlV3::Handoff(_));
        let outgoing = Outgoing {
            frame: message.encode()?,
            secret,
        };
        // The send lock serializes this check and enqueue with the pump, so
        // that no egress frame can follow a Stop.
        let guard = self.send.lock().await;
        let checked = self.shared.order().send_control(&message);
        drop(message);
        checked?;
        let open = self.shared.order().egress_open();
        self.gate.send_replace(open);
        let sender = guard.as_ref().ok_or(SessionProtocolError::Closed)?;
        sender
            .send(outgoing)
            .await
            .map_err(|_| SessionProtocolError::Closed)
    }

    /// Sends archive bytes (session mode), in frames of at most 64 KiB.
    pub(crate) async fn send_archive(&mut self, bytes: &[u8]) -> Result<()> {
        for chunk in bytes.chunks(MAX_DATA_BYTES) {
            self.shared.order().send_data(ChannelV3::Archive, chunk)?;
            let frame = encode_frame(ChannelV3::Archive, chunk)?;
            self.enqueue(frame, false).await?;
        }
        Ok(())
    }
    /// Sends provider stdin bytes, in frames of at most 64 KiB.
    pub(crate) async fn send_stdin(&mut self, bytes: &[u8]) -> Result<()> {
        for chunk in bytes.chunks(MAX_DATA_BYTES) {
            self.shared.order().send_data(ChannelV3::Stdin, chunk)?;
            let frame = encode_frame(ChannelV3::Stdin, chunk)?;
            self.enqueue(frame, false).await?;
        }
        Ok(())
    }
    /// Ends the stream towards the worker once the queued frames are
    /// written (within `CLOSE_TIME`): no frame is accepted afterwards.
    pub(crate) async fn close(&mut self) {
        self.gate.send_replace(false);
        self.pump.abort();
        self.send.lock().await.take();
        if let Some(mut writer) = self.writer.take()
            && tokio::time::timeout(CLOSE_TIME, &mut writer).await.is_err()
        {
            writer.abort();
        }
    }
}

impl SessionEventsV3 {
    /// The next event. `Ok(None)`: the worker ended the stream cleanly after
    /// its end message. Any error ends the channel.
    pub(crate) async fn next(&mut self) -> Result<Option<WorkerEventV3>> {
        if self.done {
            return Err(SessionProtocolError::Closed);
        }
        match self.events.recv().await {
            Some(Ok(Some(event))) => Ok(Some(event)),
            Some(Ok(None)) => {
                self.done = true;
                Ok(None)
            }
            Some(Err(error)) => {
                self.done = true;
                Err(error)
            }
            None => {
                self.done = true;
                Err(SessionProtocolError::Closed)
            }
        }
    }
}

async fn write_frames<W: AsyncWrite>(
    write: tokio::io::WriteHalf<W>,
    mut outgoing: mpsc::Receiver<Outgoing>,
) {
    let mut write = write;
    while let Some(item) = outgoing.recv().await {
        if write.write_all(&item.frame).await.is_err() || write.flush().await.is_err() {
            break;
        }
        drop(item);
    }
    let _ = write.shutdown().await;
}

async fn read_frames<R: AsyncRead>(
    read: tokio::io::ReadHalf<R>,
    shared: Arc<Shared>,
    events: mpsc::Sender<Result<Option<WorkerEventV3>>>,
    mut egress: tokio::io::WriteHalf<DuplexStream>,
) {
    let mut read = read;
    let mut reader = FrameReaderV3::new();
    let mut egress_open = true;
    loop {
        let frame = match reader.read(&mut read, MAX_CONTROL_BYTES).await {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                let end = if shared.order().ended() {
                    Ok(None)
                } else {
                    Err(SessionProtocolError::Closed)
                };
                let _ = events.send(end).await;
                return;
            }
            Err(error) => {
                let _ = events.send(Err(error)).await;
                return;
            }
        };
        let routed = route(&shared, frame);
        match routed {
            Ok(Routed::Event(event)) => {
                if events.send(Ok(Some(event))).await.is_err() {
                    return;
                }
            }
            Ok(Routed::Egress(bytes)) => {
                // The G05 owner went away: its bytes are dropped, the session
                // continues to its own end.
                if egress_open && egress.write_all(&bytes).await.is_err() {
                    egress_open = false;
                }
            }
            Err(error) => {
                let _ = events.send(Err(error)).await;
                return;
            }
        }
    }
}

enum Routed {
    Event(WorkerEventV3),
    Egress(Vec<u8>),
}
fn route(shared: &Shared, frame: FrameV3) -> Result<Routed> {
    let FrameV3 { channel, payload } = frame;
    if channel == ChannelV3::Control {
        let message = WorkerControlV3::decode(&payload)?;
        shared.order().receive_control(&message)?;
        return Ok(Routed::Event(WorkerEventV3::Control(message)));
    }
    shared.order().receive_data(channel, &payload)?;
    Ok(match channel {
        ChannelV3::Stdout => Routed::Event(WorkerEventV3::Stdout(payload)),
        ChannelV3::Stderr => Routed::Event(WorkerEventV3::Stderr(payload)),
        ChannelV3::Archive => Routed::Event(WorkerEventV3::Archive(payload)),
        ChannelV3::Egress => Routed::Egress(payload),
        ChannelV3::Control | ChannelV3::Stdin => return Err(SessionProtocolError::Protocol),
    })
}

async fn pump_egress(
    mut egress: tokio::io::ReadHalf<DuplexStream>,
    shared: Arc<Shared>,
    send: Arc<tokio::sync::Mutex<Option<mpsc::Sender<Outgoing>>>>,
    mut gate: watch::Receiver<bool>,
) {
    if gate.wait_for(|open| *open).await.is_err() {
        return;
    }
    let mut chunk = vec![0_u8; MAX_DATA_BYTES];
    loop {
        let count = match egress.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(count) => count,
        };
        let bytes = &chunk[..count];
        let guard = send.lock().await;
        // Checked under the send lock: no egress frame follows a Stop.
        if shared.order().send_data(ChannelV3::Egress, bytes).is_err() {
            return;
        }
        let Ok(frame) = encode_frame(ChannelV3::Egress, bytes) else {
            return;
        };
        let Some(sender) = guard.as_ref() else {
            return;
        };
        if sender
            .send(Outgoing {
                frame,
                secret: false,
            })
            .await
            .is_err()
        {
            return;
        }
    }
}

#[cfg(test)]
#[path = "session_protocol_tests.rs"]
pub(crate) mod tests;
