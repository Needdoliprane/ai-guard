//! One supervised session's live activity, as owned by the execution owner
//! (broker executor or managed runner): stdout tap, worker hook channel,
//! forwarder and final coverage. Errors of an optional channel are recorded
//! as coverage facts; they never start or stop the worker by themselves.
use super::{
    collector::{
        ActivityChannel, ActivityClient, ActivityCollector, ActivityContext, ActivityLimits,
    },
    forwarder::{ActivitySink, ForwardOutcome, forward},
    hooks::{HookAck, HookFrame, HookPumpOutcome, HookTransport, pump_hooks},
    spool::ActivitySpool,
    stdout::{PumpOutcome, StdoutTap, pump, stdout_tap},
};
use crate::{
    activity_channel::wire::{ClientV1, HelperFrameV1, StatusV1},
    control_fs::Directory,
    redaction::Redactor,
    runtime::{ActivityExecTransport, ContainerSpec, ManagedRuntime, OwnedContainerSnapshot},
};
use anyhow::Result;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::watch,
    task::JoinHandle,
    time::{Instant, timeout_at},
};

/// Bounded waits after the transport ended. They delay only finalization.
const PUMP_DRAIN: Duration = Duration::from_secs(30);
const HOOK_DRAIN: Duration = Duration::from_secs(30);
const FORWARD_DRAIN: Duration = Duration::from_secs(30);
const ABORT_JOIN_RESERVE: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DrainError {
    Cancelled,
    Panicked,
    Deadline,
    Unconfirmed,
}

struct SessionTask<T>(JoinHandle<T>);

impl<T> SessionTask<T> {
    fn new(task: JoinHandle<T>) -> Self {
        Self(task)
    }

    async fn drain(mut self, budget: Duration, abort_reserve: Duration) -> Result<T, DrainError> {
        let started = Instant::now();
        let deadline = started + budget;
        let graceful = started + budget.saturating_sub(abort_reserve);
        let (joined, abort_requested) = match timeout_at(graceful, &mut self.0).await {
            Ok(joined) => (joined, false),
            Err(_) => {
                self.0.abort();
                let joined = timeout_at(deadline, &mut self.0)
                    .await
                    .map_err(|_| DrainError::Unconfirmed)?;
                (joined, true)
            }
        };
        if Instant::now() >= deadline {
            return Err(DrainError::Unconfirmed);
        }
        match joined {
            Ok(value) => Ok(value),
            Err(error) if error.is_panic() => Err(DrainError::Panicked),
            Err(_) if abort_requested => Err(DrainError::Deadline),
            Err(_) => Err(DrainError::Cancelled),
        }
    }
}

impl<T> Drop for SessionTask<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
#[path = "session_close_tests.rs"]
mod close_tests;

#[derive(Default)]
enum HookTask {
    #[default]
    Idle,
    Attaching,
    Unavailable,
    Active(SessionTask<HookPumpOutcome>),
    Closed,
}

type HookSlot = Arc<Mutex<HookTask>>;

pub(crate) struct LiveSession {
    collector: ActivityCollector,
    reopened_spool: bool,
    tap: Option<StdoutTap>,
    pump: Option<SessionTask<PumpOutcome>>,
    forwarder: Option<SessionTask<ForwardOutcome>>,
    stop: watch::Sender<bool>,
    hook_task: HookSlot,
    hook_stop: watch::Sender<bool>,
}

/// Clonable handle used from a runtime `started` callback.
#[derive(Clone)]
pub(crate) struct HookAttacher {
    collector: ActivityCollector,
    slot: HookSlot,
    stop: watch::Receiver<bool>,
    client: ClientV1,
}

pub(crate) fn report_terminal_unconfirmed() {
    use std::io::Write;

    let _ = writeln!(
        std::io::stderr(),
        "provider_activity phase=terminal state=unconfirmed"
    );
}

impl LiveSession {
    /// Open (or reopen after an interruption) the session spool in the
    /// private `root` and start its forwarder; with `live_stdout`, for a
    /// batch session whose owner taps the client stdout, also its pump. The
    /// activity is durable before any start is emitted.
    pub(crate) fn open(
        root: &Directory,
        context: ActivityContext,
        redactor: Redactor,
        client: impl ActivitySink + 'static,
        limits: ActivityLimits,
        live_stdout: bool,
    ) -> Result<Self> {
        let live_stdout = live_stdout && !context.interactive;
        let (spool, recovery) = ActivitySpool::open_or_create(root, context.session)?;
        let collector = ActivityCollector::new(spool, context, redactor, limits)?;
        let (stop, stopped) = watch::channel(false);
        let forwarder = {
            let collector = collector.clone();
            SessionTask::new(tokio::spawn(async move {
                forward(collector, &client, stopped, FORWARD_DRAIN).await
            }))
        };
        let (tap, pump) = if !live_stdout {
            (None, None)
        } else {
            let (tap, chunks) = stdout_tap();
            let task = SessionTask::new(tokio::spawn(pump(collector.clone(), chunks)));
            let _ = collector.channel_state(ActivityChannel::Stdout, "opened", None);
            (Some(tap), Some(task))
        };
        let (hook_stop, _) = watch::channel(false);
        Ok(Self {
            collector,
            reopened_spool: recovery.is_some(),
            tap,
            pump,
            forwarder: Some(forwarder),
            stop,
            hook_task: Arc::new(Mutex::new(HookTask::Idle)),
            hook_stop,
        })
    }

    /// The stdout tap for the start control; taken at most once.
    pub(crate) fn take_tap(&mut self) -> Option<StdoutTap> {
        self.tap.take()
    }

    pub(crate) fn begin_producer_drain(&mut self) {
        self.tap = None;
        self.hook_stop.send_replace(true);
    }

    /// Replay persisted terminal activity with its original coverage facts.
    /// A missing final summary leaves collection coverage unconfirmed.
    pub(crate) async fn replay_retained(
        root: &Directory,
        context: ActivityContext,
        redactor: Redactor,
        client: &impl ActivitySink,
        limits: ActivityLimits,
    ) -> Result<()> {
        let (spool, _) = ActivitySpool::reopen(root, context.session)?;
        let collector = ActivityCollector::new(spool, context, redactor, limits)?;
        collector.ensure_replay_coverage()?;
        let (_, stop) = watch::channel(true);
        let delivery = forward(collector, client, stop, FORWARD_DRAIN).await;
        anyhow::ensure!(
            delivery.undelivered == Some(0),
            "provider activity replay is not confirmed"
        );
        Ok(())
    }

    pub(crate) fn hook_attacher(&self, client: ActivityClient) -> HookAttacher {
        HookAttacher {
            collector: self.collector.clone(),
            slot: self.hook_task.clone(),
            stop: self.hook_stop.subscribe(),
            client: match client {
                ActivityClient::ClaudeCode => ClientV1::ClaudeCode,
                ActivityClient::Codex => ClientV1::Codex,
            },
        }
    }

    /// Settle the activity after execution and its required audit. The session
    /// owner reports incomplete collection separately from the execution result.
    pub(crate) async fn finish_terminal(self, terminal_confirmed: bool) {
        let finished = self.finish(terminal_confirmed).await;
        if !terminal_confirmed || finished.is_err() {
            report_terminal_unconfirmed();
        }
    }

    async fn finish(mut self, terminal_confirmed: bool) -> Result<()> {
        // An unused tap (no start emitted) must not keep the pump open.
        self.begin_producer_drain();
        let (hook, poisoned) = self.close_hook_slot();
        let unattached = matches!(&hook, HookTask::Idle);
        let pump = self.pump.take();
        let (hook, stdout) = tokio::join!(
            async {
                match hook {
                    HookTask::Active(task) => {
                        task.drain(HOOK_DRAIN, ABORT_JOIN_RESERVE).await.map(Some)
                    }
                    HookTask::Attaching | HookTask::Closed => Err(DrainError::Unconfirmed),
                    HookTask::Unavailable | HookTask::Idle => Ok(None),
                }
            },
            async {
                match pump {
                    Some(task) => task.drain(PUMP_DRAIN, ABORT_JOIN_RESERVE).await.map(Some),
                    None => Ok(None),
                }
            }
        );
        let coverage = (|| {
            // A pump's blocking append can outlive cancellation of its async
            // task. Keep its spool open for recovery when the producer's
            // normal completion is unknown.
            let hook = hook.map_err(|error| {
                anyhow::anyhow!("provider activity hook closure is unknown ({error:?})")
            })?;
            let stdout = stdout.map_err(|error| {
                anyhow::anyhow!("provider activity stdout closure is unknown ({error:?})")
            })?;
            anyhow::ensure!(!poisoned, "provider activity hook ownership is unknown");
            if !terminal_confirmed {
                // The same session may still need a fresh activity producer
                // on recovery. Leave its verified prefix open until terminal.
                return Ok(());
            }
            if self.reopened_spool {
                // Producer counters cover only this run. The retained prefix
                // needs its original summary or an explicit coverage gap.
                return self.collector.ensure_replay_coverage();
            }
            if unattached {
                let _ = self.collector.channel_state(
                    ActivityChannel::Hook,
                    "not_attached",
                    Some("no worker hook channel was attached in this run"),
                );
            }
            if let Some(outcome) = hook.as_ref() {
                self.collector.confirm_hook_drain(outcome)?;
            }
            if stdout.is_some() {
                let _ = self
                    .collector
                    .channel_state(ActivityChannel::Stdout, "closed", None);
            }
            self.collector.finish()?;
            Ok(())
        })();
        self.stop.send_replace(true);
        let delivery = match self.forwarder.take() {
            Some(task) => task
                .drain(FORWARD_DRAIN, ABORT_JOIN_RESERVE)
                .await
                .map(Some),
            None => Ok(None),
        };
        coverage?;
        let delivery = delivery.map_err(|error| {
            anyhow::anyhow!("provider activity forwarding is unknown ({error:?})")
        })?;
        if let Some(outcome) = delivery {
            anyhow::ensure!(
                outcome.undelivered == Some(0),
                "provider activity delivery is unconfirmed"
            );
        }
        Ok(())
    }

    fn close_hook_slot(&self) -> (HookTask, bool) {
        match self.hook_task.lock() {
            Ok(mut slot) => (std::mem::replace(&mut *slot, HookTask::Closed), false),
            Err(error) => (
                std::mem::replace(&mut *error.into_inner(), HookTask::Closed),
                true,
            ),
        }
    }
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        self.begin_producer_drain();
        self.stop.send_replace(true);
        // Abort a remaining pump even when a HookAttacher holds a slot clone.
        drop(self.close_hook_slot());
    }
}

impl HookAttacher {
    /// Open, handshake and pump the worker hook channel of a running worker.
    /// Never fails the caller: an unavailable channel is recorded, and the
    /// worker's gated hooks then refuse their actions on their own.
    pub(crate) async fn attach(
        &self,
        runtime: &ManagedRuntime,
        snapshot: OwnedContainerSnapshot,
        spec: &ContainerSpec,
    ) {
        {
            let Ok(mut slot) = self.slot.lock() else {
                return;
            };
            if !matches!(&*slot, HookTask::Idle) || *self.stop.borrow() {
                return;
            }
            *slot = HookTask::Attaching;
        }
        let mut transport = match runtime.open_activity_channel(snapshot, spec).await {
            Ok(transport) => transport,
            Err(error) => {
                let _ = self.collector.channel_state(
                    ActivityChannel::Hook,
                    "unavailable",
                    Some(&error.to_string()),
                );
                self.mark_unavailable();
                return;
            }
        };
        if let Err(error) = transport.handshake().await {
            let _ = self.collector.channel_state(
                ActivityChannel::Hook,
                "unavailable",
                Some(&error.to_string()),
            );
            let _ = transport.close_and_join().await;
            self.mark_unavailable();
            return;
        }
        if let Ok(mut slot) = self.slot.lock()
            && matches!(&*slot, HookTask::Attaching)
            && !*self.stop.borrow()
        {
            let exec = ExecHooks {
                transport,
                client: self.client,
                stop: self.stop.clone(),
                draining: false,
                closed: false,
            };
            *slot = HookTask::Active(SessionTask::new(tokio::spawn(pump_hooks(
                self.collector.clone(),
                exec,
            ))));
            return;
        }
        let _ = transport.close_and_join().await;
    }

    fn mark_unavailable(&self) {
        if let Ok(mut slot) = self.slot.lock()
            && matches!(&*slot, HookTask::Attaching)
        {
            *slot = HookTask::Unavailable;
        }
    }
}

/// Adapter from the runtime exec transport to the hook pump seam.
struct ExecHooks {
    transport: ActivityExecTransport,
    client: ClientV1,
    stop: watch::Receiver<bool>,
    draining: bool,
    closed: bool,
}

impl ExecHooks {
    fn frame(&self, frame: HelperFrameV1) -> Option<HookFrame> {
        match frame {
            HelperFrameV1::Hook(hook) => {
                let submit = hook.submit;
                // A frame claiming another client is unreadable for this
                // session: a gate is then refused instead of trusted.
                let payload = if submit.client == self.client {
                    submit.payload
                } else {
                    String::new()
                };
                Some(HookFrame::Hook {
                    seq: hook.seq,
                    event: submit.event,
                    payload,
                    truncated: submit.stdin_truncated || submit.payload_invalid_utf8,
                })
            }
            HelperFrameV1::Loss(loss) => Some(HookFrame::Loss {
                seq: loss.seq,
                count: loss.count,
                reason: serde_json::to_value(loss.reason)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "unknown".into()),
            }),
            HelperFrameV1::Hello(_) => None,
        }
    }
}

impl HookTransport for ExecHooks {
    async fn next(&mut self) -> anyhow::Result<Option<HookFrame>> {
        loop {
            if self.closed {
                return Ok(None);
            }
            let next = if self.draining {
                self.transport.next_frame().await
            } else {
                tokio::select! {
                    frame = self.transport.next_frame() => frame,
                    changed = self.stop.changed() => {
                        if changed.is_err() || *self.stop.borrow() {
                            // EOF to the helper: it refuses waiting hooks and
                            // reports its final losses before ending.
                            self.transport.close_input();
                            self.draining = true;
                        }
                        continue;
                    }
                }
            };
            match next {
                Ok(Some(frame)) => match self.frame(frame) {
                    Some(frame) => return Ok(Some(frame)),
                    None => anyhow::bail!("unexpected helper hello after the handshake"),
                },
                Ok(None) => {
                    self.closed = true;
                    let closure = self.transport.close_and_join().await;
                    anyhow::ensure!(closure.local_joined, "activity channel closure is unknown");
                    anyhow::ensure!(
                        !closure.cli_kill_requested && closure.cli_exit_code == Some(0),
                        "activity channel CLI exited abnormally"
                    );
                    return Ok(None);
                }
                Err(error) => {
                    self.closed = true;
                    let _ = self.transport.close_and_join().await;
                    anyhow::bail!("activity channel failed: {error}");
                }
            }
        }
    }

    async fn ack(&mut self, seq: u64, ack: HookAck) -> anyhow::Result<()> {
        if self.draining {
            // The helper already refuses what remains; no ack is accepted.
            return Ok(());
        }
        let status = match ack {
            HookAck::Recorded => StatusV1::Recorded,
            HookAck::Refused => StatusV1::Refused,
        };
        self.transport
            .acknowledge(seq, status)
            .await
            .map_err(|error| anyhow::anyhow!("activity acknowledgement failed: {error}"))
    }
}
