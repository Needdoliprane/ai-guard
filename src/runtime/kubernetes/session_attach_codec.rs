//! WebSocket layer of the preamble-only attach stream (stage S2b).
//!
//! Framing is done by `tokio-tungstenite` on the stream that the retained
//! HTTPS client already upgraded and that `validate_attach_upgrade` accepted:
//! `from_raw_socket` with the client role. The library performs no handshake,
//! connection, TLS or redirect here, and those features are not compiled in.
//! It is a codec: it is neither an identity nor a permit.
//!
//! This module is the only place that names the library. Every item and
//! every error of the library becomes a closed value, and no library error
//! is formatted or kept. The adapter's own values keep no peer byte beyond
//! the one message being handled. The library keeps some until the codec is
//! dropped: the reason of a close frame of the peer, inside the reply that
//! it queued, and the bytes that it read ahead, inside its read buffer.
//! Every bound of the library is set from the constants of the attach
//! module, and none is left to a library default.
//!
//! A write is whole or final. A `send` or `flush` that is still pending at
//! its limit may have left a cut frame on the transport: the adapter then
//! refuses every later frame, and `close` only takes the transport back,
//! shuts it down and drops it. Neither the rest of that frame nor any other
//! frame is written after the cut.
//!
//! Memory. The configured ceilings bound contents, not allocations: the
//! payload of one frame, the aggregate of one fragmented message and the
//! bytes waiting in the write queue. The read buffer of the library is its
//! frame buffer. It starts at `READ_CHUNK_BYTES`, which is also the most
//! that one read asks of the transport, and it is grown to the declared
//! length of a frame once that length was admitted. The buffers of the
//! library grow geometrically, so the allocation behind the frame buffer can
//! reach about twice the frame ceiling, and the one behind the aggregate
//! about twice the message ceiling. A frame and the aggregate can be held at
//! the same time, because the aggregate is judged once the fragment that
//! crosses its ceiling has been read. A delivered message is no further
//! allocation: a single-frame message is a slice of the frame buffer, and a
//! reassembled one is the aggregate itself. Every one of these allocations
//! is infallible: a failed one aborts the process. Nothing here is a
//! measured heap, a sum that bounds one, or a recovery from an exhausted
//! one.
//!
//! Panics. The library draws the mask of every client frame from `rand`,
//! which panics if its generator cannot be seeded or reseeded, and its
//! decoder of fragmented text holds unwraps that rest on its own invariants.
//! A panic inside the library unwinds in the owner task of the attempt: the
//! stream is dropped, and the attempt is recorded with the cause
//! `OwnerStopped` and its slot returned. The writer that this layer replaced
//! reported a failed random source as a typed `LocalFailure`; that case no
//! longer exists.
//!
//! Framing checks. A declared size above the bound and a reserved opcode are
//! refused from the frame header. The other refusals of the library
//! (reserved bits, a masked frame, a fragmented or oversized control frame,
//! a continuation without a start) are made once the frame's payload has
//! been read, within the same size bound.
//!
//! Logging. The library emits `log` records at debug and trace level that
//! include frames and their payload. This crate installs no `log` logger; a
//! logger installed elsewhere at those levels would receive peer bytes.
use super::{
    CLOSE_WRITE_LIMIT, MAX_WRITE_QUEUE_BYTES, MAX_WS_MESSAGE_BYTES, NO_BYTES, READ_CHUNK_BYTES,
    STDIN_CLOSE, Violation, WRITE_BUFFER_BYTES,
};
use futures_util::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    time::{Instant, error::Elapsed, timeout, timeout_at},
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        error::{CapacityError, Error, ProtocolError},
        protocol::{
            CloseFrame, Message, Role, WebSocketConfig,
            frame::{Utf8Bytes, coding::CloseCode},
        },
    },
};

/// Every bound of the library, from the constants of the attach module.
/// The library panics on a queue bound that does not exceed its write
/// buffer; a constant assertion of the attach module holds that.
fn config() -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(READ_CHUNK_BYTES)
        .write_buffer_size(WRITE_BUFFER_BYTES)
        .max_write_buffer_size(MAX_WRITE_QUEUE_BYTES)
        .max_message_size(Some(MAX_WS_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_WS_MESSAGE_BYTES))
        .accept_unmasked_frames(false)
}

/// A close frame of the peer is a peer end only without a body, or with the
/// code of a normal closure (1000) or of an endpoint going away (1001).
/// Every other code is refused. That includes 1002, which the library
/// substitutes for a reserved code (1005, 1006, 1015 and the unassigned
/// ranges) while hiding the code that was received.
fn accepted_close(frame: Option<&CloseFrame>) -> bool {
    frame.is_none_or(|frame| matches!(frame.code, CloseCode::Normal | CloseCode::Away))
}

/// What the stream still accepts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WireState {
    /// Frames can be read and written.
    Open,
    /// The peer sent a well-formed close frame, whatever its code. The
    /// library refuses any data frame from then on, and the reply that it
    /// queued is the only thing left to write. A close frame that the
    /// library refuses as malformed (a one-byte body, a reason that is not
    /// UTF-8) is an error of the reader instead, and leaves the state `Open`.
    PeerClosing,
    /// A cut frame, or a failed or ended transport: nothing more is read or
    /// written.
    Unusable,
}

/// Why a write did not complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WriteFailure {
    /// Still pending at its limit. Its frame may be cut: the stream is
    /// unusable from now on.
    Deadline,
    /// The transport or the library refused it: the stream is unusable.
    Broken,
    /// Refused here before any byte of it was queued. Nothing was written
    /// and the stream is still on a frame boundary.
    Local,
}

/// One item read from the server. No `Debug`: a message carries payload
/// bytes.
pub(super) enum ServerItem {
    /// One reassembled binary message, possibly empty.
    Message(ServerMessage),
    /// A ping. The library queued its pong, which `flush` writes.
    Ping,
    /// An unsolicited pong.
    Pong,
    /// A close frame that is accepted as a peer end (see `accepted_close`).
    Closed,
    /// The transport ended without a close frame, or failed.
    Gone,
    /// Anything else, as the closed reason of its refusal.
    Refused(Violation),
}

/// One reassembled binary message of the server. No `Debug`.
pub(super) struct ServerMessage(Message);
impl ServerMessage {
    /// The channel byte, then its payload. Empty when the peer sent a
    /// message without any byte.
    pub(super) fn bytes(&self) -> &[u8] {
        match &self.0 {
            Message::Binary(payload) => &payload[..],
            _ => NO_BYTES,
        }
    }
}

/// The attach stream behind the codec library.
pub(super) struct AttachCodec<S> {
    ws: WebSocketStream<S>,
    wire: WireState,
}
impl<S: AsyncRead + AsyncWrite + Unpin> AttachCodec<S> {
    /// Wraps the upgraded stream. No handshake is performed and no byte is
    /// read or written.
    pub(super) async fn new(io: S) -> Self {
        Self {
            ws: WebSocketStream::from_raw_socket(io, Role::Client, Some(config())).await,
            wire: WireState::Open,
        }
    }

    /// The next item of the server. Its single await is the `next` of the
    /// library, which loses nothing when it is dropped: this is cancel safe.
    /// Nothing is read once the stream closed, failed or was cut.
    pub(super) async fn read(&mut self) -> ServerItem {
        if self.wire != WireState::Open {
            return ServerItem::Gone;
        }
        match self.ws.next().await {
            Some(Ok(message)) => self.item(message),
            Some(Err(error)) => self.refused_read(&error),
            None => {
                self.wire = WireState::Unusable;
                ServerItem::Gone
            }
        }
    }

    fn item(&mut self, message: Message) -> ServerItem {
        match message {
            binary @ Message::Binary(_) => ServerItem::Message(ServerMessage(binary)),
            Message::Ping(_) => ServerItem::Ping,
            Message::Pong(_) => ServerItem::Pong,
            Message::Close(frame) => {
                self.wire = WireState::PeerClosing;
                if accepted_close(frame.as_ref()) {
                    ServerItem::Closed
                } else {
                    ServerItem::Refused(Violation::Protocol)
                }
            }
            // A text message, and the raw frame variant that a read never
            // returns.
            _ => ServerItem::Refused(Violation::Protocol),
        }
    }

    /// Closed reading of an error of the reader. The error is neither
    /// formatted nor kept. A refusal leaves the write side usable, so that
    /// the stdin close and the close frame can still be written; an ended or
    /// failed transport does not. A malformed close frame of the peer is
    /// such a refusal: the library fails on it before it changes state.
    fn refused_read(&mut self, error: &Error) -> ServerItem {
        match error {
            Error::Capacity(CapacityError::MessageTooLong { .. }) => {
                ServerItem::Refused(Violation::TooBig)
            }
            Error::Io(_)
            | Error::ConnectionClosed
            | Error::AlreadyClosed
            | Error::Protocol(ProtocolError::ResetWithoutClosingHandshake) => {
                self.wire = WireState::Unusable;
                ServerItem::Gone
            }
            // Every other refusal of the reader, whatever it is.
            _ => ServerItem::Refused(Violation::Protocol),
        }
    }

    /// One whole masked binary message, written and flushed within `until`.
    pub(super) async fn send(
        &mut self,
        message: Vec<u8>,
        until: Instant,
    ) -> Result<(), WriteFailure> {
        self.write(Message::binary(message), until).await
    }

    /// Writes within `until` what the library queued: the pong of a ping, or
    /// its reply to a close frame of the peer.
    pub(super) async fn flush(&mut self, until: Instant) -> Result<(), WriteFailure> {
        if self.wire == WireState::Unusable {
            return Err(WriteFailure::Broken);
        }
        let outcome = timeout_at(until, SinkExt::<Message>::flush(&mut self.ws)).await;
        self.written(outcome)
    }

    async fn write(&mut self, message: Message, until: Instant) -> Result<(), WriteFailure> {
        if self.wire != WireState::Open {
            return Err(WriteFailure::Broken);
        }
        let outcome = timeout_at(until, self.ws.send(message)).await;
        self.written(outcome)
    }

    fn written(&mut self, outcome: Result<Result<(), Error>, Elapsed>) -> Result<(), WriteFailure> {
        match outcome {
            Ok(Ok(())) => Ok(()),
            // Refused against the queue bound before any byte of it was
            // queued.
            Ok(Err(Error::WriteBufferFull(_))) => Err(WriteFailure::Local),
            Ok(Err(_)) => {
                self.wire = WireState::Unusable;
                Err(WriteFailure::Broken)
            }
            // Still pending: the frame may be cut on the transport.
            Err(_) => {
                self.wire = WireState::Unusable;
                Err(WriteFailure::Deadline)
            }
        }
    }

    /// The bounded end of the stream. It consumes the adapter: when it
    /// returns, the transport is dropped.
    ///
    /// On a usable stream it writes the v5 close of stdin, then the close
    /// frame with `code`, each whole and each within `CLOSE_WRITE_LIMIT`.
    /// A stream whose peer sent a close frame that the library refused as
    /// malformed is still such a stream: both frames are written, and the
    /// owner passes the code 1002 for it. After a well-formed close frame of
    /// the peer it writes only the reply that the library queued. After a
    /// cut frame or on a failed transport it writes no frame. In every case
    /// the transport is then taken back from the library, which drops its
    /// queue unwritten, and is shut down within one more limit.
    pub(super) async fn close(mut self, code: u16) {
        match self.wire {
            WireState::Open => {
                let limit = Instant::now() + CLOSE_WRITE_LIMIT;
                let _ = self.write(Message::binary(STDIN_CLOSE), limit).await;
                // Not behind a stdin close that was cut.
                if self.wire == WireState::Open {
                    let frame = CloseFrame {
                        code: CloseCode::from(code),
                        reason: Utf8Bytes::default(),
                    };
                    let limit = Instant::now() + CLOSE_WRITE_LIMIT;
                    let _ = self.write(Message::Close(Some(frame)), limit).await;
                }
            }
            WireState::PeerClosing => {
                let _ = self.flush(Instant::now() + CLOSE_WRITE_LIMIT).await;
            }
            WireState::Unusable => {}
        }
        let mut io = self.ws.into_inner();
        let _ = timeout(CLOSE_WRITE_LIMIT, io.shutdown()).await;
    }
}
