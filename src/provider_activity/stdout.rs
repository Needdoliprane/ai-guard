//! Live framing of the batch client stdout into complete JSONL records.
//!
//! The runtime keeps its own bounded terminal capture; this tap receives the
//! same byte chunks through a bounded channel. While the audit path is
//! unavailable the pump stops consuming the channel, so the capture task and
//! then the client block on their writes (stall, coordinator decision D2).
//! The stall is bounded by `TAP_STALL_LIMIT`: after it the tap stops
//! forwarding and counts the remaining bytes as an explicit loss, so a stalled
//! audit can never prevent the runtime from draining a finished transport.
//! Gated hooks keep refusing actions for as long as the outage lasts.
use super::collector::{ActivityChannel, ActivityCollector, Admission};
use crate::adapters::MAX_LINE_BYTES;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc;

/// Chunks in flight between the capture task and the pump.
pub const TAP_CHUNKS: usize = 64;
/// Longest time one chunk may wait for a stalled pump.
pub const TAP_STALL_LIMIT: Duration = Duration::from_secs(30);
const STALL_POLL: Duration = Duration::from_millis(250);

#[derive(Debug, Default)]
struct TapState {
    overflowed: AtomicBool,
    lost_bytes: AtomicU64,
}

/// The capture side of the tap. Sending awaits while the pump is stalled, up
/// to `TAP_STALL_LIMIT`; after that the tap is permanently in overflow.
#[derive(Clone, Debug)]
pub struct StdoutTap {
    sender: mpsc::Sender<Vec<u8>>,
    state: Arc<TapState>,
    stall_limit: Duration,
}

impl StdoutTap {
    /// Forward one chunk. A closed pump or an overflowed tap never blocks or
    /// fails the terminal capture; the bytes are counted as lost instead.
    pub async fn send(&self, chunk: &[u8]) {
        if !self.state.overflowed.load(Ordering::Acquire) {
            match tokio::time::timeout(self.stall_limit, self.sender.send(chunk.to_vec())).await {
                Ok(Ok(())) => return,
                Ok(Err(_)) => return,
                Err(_) => self.state.overflowed.store(true, Ordering::Release),
            }
        }
        self.state
            .lost_bytes
            .fetch_add(chunk.len() as u64, Ordering::AcqRel);
    }
}

/// The pump side: chunks plus the tap's overflow accounting.
pub struct TapReceiver {
    chunks: mpsc::Receiver<Vec<u8>>,
    state: Arc<TapState>,
}

pub fn stdout_tap() -> (StdoutTap, TapReceiver) {
    stdout_tap_with_stall(TAP_STALL_LIMIT)
}

/// Same tap with an explicit stall bound (tests use a short one).
pub fn stdout_tap_with_stall(stall_limit: Duration) -> (StdoutTap, TapReceiver) {
    let (sender, chunks) = mpsc::channel(TAP_CHUNKS);
    let state = Arc::new(TapState::default());
    (
        StdoutTap {
            sender,
            state: state.clone(),
            stall_limit,
        },
        TapReceiver { chunks, state },
    )
}

/// Bounded splitter. An over-long record is discarded up to its delimiter and
/// reported once; bytes are never spliced across records.
#[derive(Default)]
pub struct LineFramer {
    buffer: Vec<u8>,
    discarding: bool,
}

pub enum Framed {
    Line(Vec<u8>),
    Oversized,
}

impl LineFramer {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<Framed> {
        let mut output = Vec::new();
        let mut rest = chunk;
        while !rest.is_empty() {
            match rest.iter().position(|byte| *byte == b'\n') {
                Some(end) => {
                    let (head, tail) = rest.split_at(end);
                    if self.discarding {
                        self.discarding = false;
                    } else if self.buffer.len() + head.len() > MAX_LINE_BYTES {
                        output.push(Framed::Oversized);
                    } else {
                        self.buffer.extend_from_slice(head);
                        output.push(Framed::Line(std::mem::take(&mut self.buffer)));
                    }
                    self.buffer.clear();
                    rest = &tail[1..];
                }
                None => {
                    if !self.discarding {
                        if self.buffer.len() + rest.len() > MAX_LINE_BYTES {
                            self.buffer.clear();
                            self.discarding = true;
                            output.push(Framed::Oversized);
                        } else {
                            self.buffer.extend_from_slice(rest);
                        }
                    }
                    rest = &[];
                }
            }
        }
        output
    }

    /// Bytes left without a delimiter at end of stream.
    pub fn unterminated(&self) -> bool {
        !self.buffer.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PumpOutcome {
    pub lines: u64,
    pub oversized: u64,
    pub unterminated_final_record: bool,
    pub stalled: bool,
    /// Bytes the tap could not forward after its stall bound.
    pub overflow_bytes: u64,
}

/// Consume the tap until the capture side closes it.
pub async fn pump(collector: ActivityCollector, tap: TapReceiver) -> PumpOutcome {
    let TapReceiver { mut chunks, state } = tap;
    let mut framer = LineFramer::default();
    let mut outcome = PumpOutcome::default();
    while let Some(chunk) = chunks.recv().await {
        for framed in framer.push(&chunk) {
            match framed {
                Framed::Line(line) => {
                    outcome.lines += 1;
                    loop {
                        let collector_ref = collector.clone();
                        let line_ref = line.clone();
                        let admission = tokio::task::spawn_blocking(move || {
                            collector_ref.stdout_line(&line_ref)
                        })
                        .await
                        .unwrap_or(Admission::Unavailable);
                        if admission != Admission::Unavailable {
                            break;
                        }
                        // Audit outage: stop consuming; retry the same record.
                        if !outcome.stalled {
                            outcome.stalled = true;
                            collector.set_stalled(true);
                            let _ = collector.channel_state(
                                ActivityChannel::Stdout,
                                "stalled",
                                Some("audit_unavailable"),
                            );
                        }
                        tokio::time::sleep(STALL_POLL).await;
                    }
                }
                Framed::Oversized => {
                    outcome.oversized += 1;
                    let _ = collector.stdout_unframed("oversized_record");
                }
            }
        }
    }
    outcome.overflow_bytes = state.lost_bytes.load(Ordering::Acquire);
    if outcome.overflow_bytes > 0 {
        let _ = collector.stdout_overflow(outcome.overflow_bytes);
    }
    if framer.unterminated() {
        outcome.unterminated_final_record = true;
        let _ = collector.stdout_unframed("unterminated_final_record");
    }
    outcome
}
