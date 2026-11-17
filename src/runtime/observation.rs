//! Optional process-local diagnostics. No authority, durable audit or engine proof.
//! Producers only try_send bounded, closed records; they never call consumer code.
use anyhow::{Result, ensure};
use serde::Serialize;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Submitted,
    ImageResolutionRequested,
    ImageResolutionReturned,
    ImageResolutionFailed,
    CreateRequested,
    Created,
    StartSpawnRequested,
    StartSpawnReturned,
    DeadlineArmed,
    DeadlineObservedByPoller,
    StopRequested,
    StopSpawnRequested,
    StopSpawnReturned,
    StopInvocationRequested,
    StopCommandExitedOk,
    StopCommandExitedError,
    StopCommandWaitFailed,
    StopInvocationReturnedOk,
    StopInvocationReturnedError,
    AttachExited,
    AttachWaitFailed,
    StdoutEof,
    StdoutReadFailed,
    StderrEof,
    StderrReadFailed,
    DrainCollected,
    DrainCollectionFailed,
    TerminalInspected,
    CleanupRequested,
    CleanupReturnedOk,
    CleanupReturnedError,
    OperationReturnedOk,
    OperationReturnedError,
}

/// All times share one process-local monotonic origin. A deadline is a plan;
/// DeadlineObservedByPoller is when the runtime poller actually handled it.
#[derive(Clone, Debug, Serialize)]
pub struct Event {
    pub trace_id: Uuid,
    pub operation: Uuid,
    /// Exact immutable 64-hex CID decoded into 32 bytes, never a name/payload.
    pub instance: Option<[u8; 32]>,
    pub elapsed_ns: u128,
    pub planned_deadline_ns: Option<u128>,
    pub phase: Phase,
}

#[derive(Clone, Debug)]
pub struct ObservationSink {
    origin: Instant,
    trace_id: Uuid,
    sender: mpsc::Sender<Event>,
    lost: Arc<AtomicBool>,
}

/// One consumer; retention/persistence happens outside the runtime control path.
#[derive(Debug)]
pub struct ObservationCollector {
    receiver: mpsc::Receiver<Event>,
    lost: Arc<AtomicBool>,
}
impl ObservationCollector {
    pub fn bounded(capacity: usize) -> Result<(ObservationSink, Self)> {
        ensure!(
            (1..=4096).contains(&capacity),
            "observation capacity must be 1..=4096"
        );
        let (sender, receiver) = mpsc::channel(capacity);
        let lost = Arc::new(AtomicBool::new(false));
        Ok((
            ObservationSink {
                origin: Instant::now(),
                trace_id: Uuid::new_v4(),
                sender,
                lost: lost.clone(),
            },
            Self { receiver, lost },
        ))
    }
    pub async fn recv(&mut self) -> Option<Event> {
        self.receiver.recv().await
    }
    pub fn try_recv(&mut self) -> Option<Event> {
        self.receiver.try_recv().ok()
    }
    /// Sticky: any dropped/malformed diagnostic invalidates completeness.
    pub fn lost(&self) -> bool {
        self.lost.load(Ordering::Acquire)
    }
}
impl ObservationSink {
    pub(super) fn operation(&self, operation: Uuid) -> Scope {
        if operation.is_nil() {
            self.lost.store(true, Ordering::Release);
        }
        Scope {
            sink: self.clone(),
            operation,
            instance: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Scope {
    sink: ObservationSink,
    operation: Uuid,
    instance: Option<[u8; 32]>,
}
impl Scope {
    pub fn instance(&self, target: &str) -> Self {
        let mut scope = self.clone();
        let mut id = [0; 32];
        // Reject aliases/names: diagnostics must never persist arbitrary strings.
        if target.len() == 64
            && target
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && hex::decode_to_slice(target, &mut id).is_ok()
        {
            scope.instance = Some(id);
        } else {
            scope.sink.lost.store(true, Ordering::Release);
            scope.instance = None;
        }
        scope
    }
    pub fn event(&self, phase: Phase) {
        self.at(phase, None);
    }
    pub fn at(&self, phase: Phase, deadline: Option<tokio::time::Instant>) {
        let planned_deadline_ns = deadline
            .and_then(|d| d.into_std().checked_duration_since(self.sink.origin))
            .map(|d| d.as_nanos());
        if deadline.is_some() && planned_deadline_ns.is_none() {
            self.sink.lost.store(true, Ordering::Release);
        }
        let event = Event {
            trace_id: self.sink.trace_id,
            operation: self.operation,
            instance: self.instance,
            elapsed_ns: self.sink.origin.elapsed().as_nanos(),
            planned_deadline_ns,
            phase,
        };
        if self.sink.sender.try_send(event).is_err() {
            self.sink.lost.store(true, Ordering::Release);
        }
    }
}
pub(super) fn record(scope: &Option<Scope>, phase: Phase) {
    if let Some(scope) = scope {
        scope.event(phase);
    }
}

#[cfg(test)]
#[path = "observation_tests.rs"]
mod tests;
