//! One supervised session's live activity, as owned by the execution owner
//! (broker executor or managed runner): stdout tap, worker hook channel,
//! forwarder and final coverage. Errors of an optional channel are recorded
//! as coverage facts; they never start or stop the worker by themselves.
use super::{
    collector::{
        ActivityChannel, ActivityClient, ActivityCollector, ActivityContext, ActivityLimits,
        ActivitySummary,
    },
    forwarder::{ForwardOutcome, forward},
    hooks::{HookAck, HookFrame, HookPumpOutcome, HookTransport, pump_hooks},
    spool::ActivitySpool,
    stdout::{PumpOutcome, StdoutTap, pump, stdout_tap},
};
use crate::{
    activity_channel::wire::{ClientV1, HelperFrameV1, StatusV1},
    control_fs::Directory,
    redaction::Redactor,
    runner::AuditClient,
    runtime::{ActivityExecTransport, ContainerSpec, ManagedRuntime, OwnedContainerSnapshot},
};
use anyhow::Result;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::watch, task::JoinHandle};

/// Bounded waits after the transport ended. They delay only finalization.
const PUMP_DRAIN: Duration = Duration::from_secs(30);
const HOOK_DRAIN: Duration = Duration::from_secs(30);
const FORWARD_DRAIN: Duration = Duration::from_secs(30);

type HookSlot = Arc<Mutex<Option<JoinHandle<HookPumpOutcome>>>>;

pub(crate) struct LiveSession {
    collector: ActivityCollector,
    tap: Option<StdoutTap>,
    pump: Option<JoinHandle<PumpOutcome>>,
    forwarder: Option<JoinHandle<ForwardOutcome>>,
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

impl LiveSession {
    /// Open (or reopen after an interruption) the session spool in the
    /// private `root` and start its forwarder; with `live_stdout`, for a
    /// batch session whose owner taps the client stdout, also its pump. The
    /// activity is durable before any start is emitted.
    pub(crate) fn open(
        root: &Directory,
        context: ActivityContext,
        redactor: Redactor,
        client: AuditClient,
        limits: ActivityLimits,
        live_stdout: bool,
    ) -> Result<Self> {
        let live_stdout = live_stdout && !context.interactive;
        let (spool, _recovery) = ActivitySpool::open_or_create(root, context.session)?;
        let collector = ActivityCollector::new(spool, context, redactor, limits)?;
        let (stop, stopped) = watch::channel(false);
        let forwarder = {
            let collector = collector.clone();
            tokio::spawn(async move { forward(collector, &client, stopped, FORWARD_DRAIN).await })
        };
        let (tap, pump) = if !live_stdout {
            (None, None)
        } else {
            let (tap, chunks) = stdout_tap();
            let task = tokio::spawn(pump(collector.clone(), chunks));
            let _ = collector.channel_state(ActivityChannel::Stdout, "opened", None);
            (Some(tap), Some(task))
        };
        let (hook_stop, _) = watch::channel(false);
        Ok(Self {
            collector,
            tap,
            pump,
            forwarder: Some(forwarder),
            stop,
            hook_task: Arc::new(Mutex::new(None)),
            hook_stop,
        })
    }

    /// The stdout tap for the start control; taken at most once.
    pub(crate) fn take_tap(&mut self) -> Option<StdoutTap> {
        self.tap.take()
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

    /// Finish after the transport ended: close the hook channel, drain the
    /// stdout pump, spool the observed coverage summary and drain the
    /// forwarder, each bounded. Undelivered events stay in the spool for a
    /// later recovery run.
    pub(crate) async fn finish(mut self) -> Result<ActivitySummary> {
        // An unused tap (no start emitted) must not keep the pump open.
        self.tap = None;
        let _ = self.hook_stop.send(true);
        let hook = self.hook_task.lock().ok().and_then(|mut slot| slot.take());
        match hook {
            Some(task) => {
                if tokio::time::timeout(HOOK_DRAIN, task).await.is_err() {
                    let _ = self.collector.channel_state(
                        ActivityChannel::Hook,
                        "closed",
                        Some("hook_drain_deadline"),
                    );
                }
            }
            None => {
                let _ = self.collector.channel_state(
                    ActivityChannel::Hook,
                    "not_attached",
                    Some("no worker hook channel was attached in this run"),
                );
            }
        }
        if let Some(task) = self.pump.take() {
            let reason = match tokio::time::timeout(PUMP_DRAIN, task).await {
                Ok(_) => None,
                Err(_) => Some("pump_drain_deadline"),
            };
            let _ = self
                .collector
                .channel_state(ActivityChannel::Stdout, "closed", reason);
        }
        let summary = self.collector.finish()?;
        let _ = self.stop.send(true);
        if let Some(task) = self.forwarder.take() {
            let _ = task.await;
        }
        Ok(summary)
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
        let mut transport = match runtime.open_activity_channel(snapshot, spec).await {
            Ok(transport) => transport,
            Err(error) => {
                let _ = self.collector.channel_state(
                    ActivityChannel::Hook,
                    "unavailable",
                    Some(&error.to_string()),
                );
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
            return;
        }
        let exec = ExecHooks {
            transport,
            client: self.client,
            stop: self.stop.clone(),
            draining: false,
            closed: false,
        };
        let task = tokio::spawn(pump_hooks(self.collector.clone(), exec));
        if let Ok(mut slot) = self.slot.lock() {
            *slot = Some(task);
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
                    payload,
                    truncated: submit.stdin_truncated || submit.payload_invalid_utf8,
                    gate: submit.gate,
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
                    let _ = self.transport.close_and_join().await;
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
