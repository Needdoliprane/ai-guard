//! Live framing of the batch client stdout into complete JSONL records.
//!
//! The runtime keeps its own bounded terminal capture; this tap receives the
//! same byte chunks through a bounded channel. While the audit path is
//! unavailable the pump stops consuming the channel, so the capture task and
//! then the client block on their writes (stall, coordinator decision D2).
//! The tap waits no longer than `TAP_STALL_LIMIT` or the runtime's signal that
//! the attach ended or an armed stop was received. After the first lost chunk,
//! it counts the remaining bytes without forwarding a discontinuous stream.
//! Gated hooks keep refusing actions for as long as the outage lasts.
use super::collector::{ActivityChannel, ActivityCollector, Admission};
use crate::adapters::MAX_LINE_BYTES;
use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, watch};

/// Chunks in flight between the capture task and the pump.
pub const TAP_CHUNKS: usize = 64;
/// Longest time one chunk may wait for a stalled pump.
pub const TAP_STALL_LIMIT: Duration = Duration::from_secs(30);
const STALL_POLL: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug)]
pub(super) enum StdoutLossReason {
    StallDeadline,
    NotForwarded,
}

#[derive(Debug, Default)]
struct TapState {
    first_loss: OnceLock<StdoutLossReason>,
    lost_bytes: AtomicU64,
}

struct PendingChunk<'a> {
    state: &'a TapState,
    bytes: u64,
    loss_reason: StdoutLossReason,
}

impl Drop for PendingChunk<'_> {
    fn drop(&mut self) {
        // The stop path may cancel the capture task before this send is polled
        // again. Count its observed bytes before dropping the channel sender.
        if self.bytes != 0 {
            // Later bytes are discarded because the stream already has a
            // gap. Preserve the reason for the first lost chunk.
            self.state.first_loss.get_or_init(|| self.loss_reason);
            self.state
                .lost_bytes
                .fetch_add(self.bytes, Ordering::AcqRel);
        }
    }
}

/// The capture side of the tap. Sending awaits while the pump is stalled, up
/// to the live stall bound or the end of the runtime transport.
#[derive(Clone, Debug)]
pub struct StdoutTap {
    sender: mpsc::Sender<Vec<u8>>,
    state: Arc<TapState>,
    stop_stalling: watch::Sender<bool>,
    stall_limit: Duration,
}

impl StdoutTap {
    /// Local notification owned by the runtime. It does not keep the chunk
    /// channel open or represent proof of workload termination.
    pub(crate) fn stall_release(&self) -> watch::Sender<bool> {
        self.stop_stalling.clone()
    }

    /// Forward one chunk. A closed pump or an overflowed tap never blocks or
    /// fails the terminal capture; the bytes are counted as lost instead.
    pub async fn send(&self, chunk: &[u8]) {
        let mut pending = PendingChunk {
            state: &self.state,
            bytes: chunk.len() as u64,
            loss_reason: StdoutLossReason::NotForwarded,
        };
        if self.state.first_loss.get().is_some() {
            return;
        }
        let mut stop = self.stop_stalling.subscribe();
        tokio::select! {
            // Continue delivering available tail chunks during capture drain.
            biased;
            result = self.sender.reserve() => {
                if let Ok(permit) = result {
                    permit.send(chunk.to_vec());
                    pending.bytes = 0;
                }
            }
            // A notified end wins over a timer that is also ready on this poll.
            _ = stop.wait_for(|ended| *ended) => {}
            () = tokio::time::sleep(self.stall_limit) => {
                pending.loss_reason = StdoutLossReason::StallDeadline;
            }
        }
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
    let (stop_stalling, _) = watch::channel(false);
    let state = Arc::new(TapState::default());
    (
        StdoutTap {
            sender,
            state: state.clone(),
            stop_stalling,
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
    /// Bytes the tap could not forward, including a send cancelled during drain.
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
        let reason = state
            .first_loss
            .get()
            .copied()
            .unwrap_or(StdoutLossReason::NotForwarded);
        let _ = collector.record_stdout_loss(outcome.overflow_bytes, reason);
    }
    if framer.unterminated() {
        outcome.unterminated_final_record = true;
        let _ = collector.stdout_unframed("unterminated_final_record");
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::capture_stream_tapped;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn transport_end_releases_stdout_backpressure_and_preserves_bounded_capture() {
        let (tap, mut chunks) = stdout_tap();
        let release = tap.stall_release();
        for _ in 0..TAP_CHUNKS {
            tap.send(b"").await;
        }
        let (mut writer, reader) = tokio::io::duplex(64);
        let payload = b"provider output\n";
        writer.write_all(payload).await.unwrap();
        writer.shutdown().await.unwrap();
        let capture = capture_stream_tapped(reader, 8, Some(tap));
        tokio::pin!(capture);
        tokio::select! {
            biased;
            _ = &mut capture => panic!("the live tap must apply backpressure"),
            () = std::future::ready(()) => {}
        }

        release.send_replace(true);
        let capture = tokio::time::timeout(Duration::from_secs(1), capture)
            .await
            .expect("terminal capture must drain without the optional tap")
            .unwrap();
        assert_eq!(capture.bytes, b"provider");
        assert_eq!(capture.total_bytes, payload.len() as u64);
        assert!(capture.truncated);
        assert_eq!(
            chunks.state.lost_bytes.load(Ordering::Acquire),
            payload.len() as u64
        );
        for _ in 0..TAP_CHUNKS {
            assert_eq!(chunks.chunks.recv().await, Some(Vec::new()));
        }
        assert_eq!(chunks.chunks.recv().await, None);
    }

    #[tokio::test]
    async fn cancelled_stdout_send_is_accounted_before_the_pump_sees_eof() {
        let (tap, mut chunks) = stdout_tap();
        let release = tap.stall_release();
        for _ in 0..TAP_CHUNKS {
            tap.send(b"").await;
        }
        let mut send = Box::pin(tap.send(b"unforwarded"));
        tokio::select! {
            biased;
            () = &mut send => panic!("the full tap must wait"),
            () = std::future::ready(()) => {}
        }

        release.send_replace(true);
        drop(send);
        drop(tap);
        for _ in 0..TAP_CHUNKS {
            assert_eq!(chunks.chunks.recv().await, Some(Vec::new()));
        }
        assert_eq!(chunks.chunks.recv().await, None);
        assert_eq!(chunks.state.lost_bytes.load(Ordering::Acquire), 11);
    }
}
