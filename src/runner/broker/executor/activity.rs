//! Live provider activity for one supervised batch execution.
//!
//! The execution owner opens one durable spool per session in the broker's
//! private `provider-activity` directory, taps the client stdout, forwards the
//! spool to the gateway while the worker runs, and spools an observed coverage
//! summary once the transport has ended. Provider records stay declarations.
use super::Execution;
use crate::provider_activity::{
    ActivityChannel, ActivityClient, ActivityCollector, ActivityContext, ActivityLimits,
    ActivitySpool, ActivitySummary, ForwardOutcome, PumpOutcome, StdoutTap, forward, pump,
    stdout_tap,
};
use anyhow::{Context, Result};
use std::time::Duration;
use tokio::{sync::watch, task::JoinHandle};

/// Bounded waits after the transport ended. They delay only finalization.
const PUMP_DRAIN: Duration = Duration::from_secs(30);
const FORWARD_DRAIN: Duration = Duration::from_secs(30);

pub(super) struct LiveActivity {
    collector: ActivityCollector,
    tap: Option<StdoutTap>,
    pump: Option<JoinHandle<PumpOutcome>>,
    forwarder: Option<JoinHandle<ForwardOutcome>>,
    stop: watch::Sender<bool>,
}

/// Map the plan's provider option; other providers have no live channel.
fn client(provider: &str) -> Option<ActivityClient> {
    match provider {
        "codex" => Some(ActivityClient::Codex),
        "claude" | "claude-code" => Some(ActivityClient::ClaudeCode),
        _ => None,
    }
}

impl Execution<'_> {
    /// Open (or, after an interruption, reopen) this session's live activity.
    /// `None` only when the plan has no supported provider client.
    pub(super) fn open_activity(&self, draft: &super::Draft) -> Result<Option<LiveActivity>> {
        let options = draft.run.options();
        let Some(client) = client(&options.provider) else {
            return Ok(None);
        };
        let root = self
            .executor
            .activity
            .as_ref()
            .context("provider activity directory is not configured")?;
        let session = draft.run.session();
        let (spool, _recovery) = ActivitySpool::open_or_create(root, session)?;
        let request = &draft.run.plan.request;
        let worker = draft.worker().ok();
        let context = ActivityContext {
            session,
            session_id: request.session_id.clone(),
            project_id: request.project_id.clone(),
            sandbox_id: request.sandbox_id.clone(),
            agent_id: request.agent_id.clone(),
            correlation_id: request.correlation_id.clone(),
            trace_id: request.trace_id.clone(),
            worker_id: worker.map(|w| w.worker_id.clone()),
            attempt_id: worker.map(|w| w.attempt.attempt_id.clone()),
            client,
            interactive: options.interactive,
            client_version_label: None,
        };
        let collector = ActivityCollector::new(
            spool,
            context,
            self.client.redactor.clone(),
            ActivityLimits::default(),
        )?;
        let (stop, stopped) = watch::channel(false);
        let forwarder = {
            let collector = collector.clone();
            let client = self.client.clone();
            tokio::spawn(async move { forward(collector, &client, stopped, FORWARD_DRAIN).await })
        };
        let (tap, pump) = if options.interactive {
            (None, None)
        } else {
            let (tap, chunks) = stdout_tap();
            let task = tokio::spawn(pump(collector.clone(), chunks));
            (Some(tap), Some(task))
        };
        if !options.interactive {
            let _ = collector.channel_state(ActivityChannel::Stdout, "opened", None);
        }
        Ok(Some(LiveActivity {
            collector,
            tap,
            pump,
            forwarder: Some(forwarder),
            stop,
        }))
    }
}

impl LiveActivity {
    /// The stdout tap for the start control; taken at most once.
    pub(super) fn take_tap(&mut self) -> Option<StdoutTap> {
        self.tap.take()
    }

    /// Finish after the transport ended: drain the stdout pump, spool the
    /// observed coverage summary and drain the forwarder, each bounded. Events
    /// still undelivered stay in the spool for a later recovery run.
    pub(super) async fn finish(mut self) -> Result<ActivitySummary> {
        // An unused tap (no start emitted) must not keep the pump open.
        self.tap = None;
        if let Some(task) = self.pump.take() {
            match tokio::time::timeout(PUMP_DRAIN, task).await {
                Ok(_) => {
                    let _ = self
                        .collector
                        .channel_state(ActivityChannel::Stdout, "closed", None);
                }
                Err(_) => {
                    let _ = self.collector.channel_state(
                        ActivityChannel::Stdout,
                        "closed",
                        Some("pump_drain_deadline"),
                    );
                }
            }
        }
        let summary = self.collector.finish()?;
        let _ = self.stop.send(true);
        if let Some(task) = self.forwarder.take() {
            let _ = task.await;
        }
        Ok(summary)
    }
}
