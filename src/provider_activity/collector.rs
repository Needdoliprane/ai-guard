//! Live collector: normalize, mask, stamp and spool provider activity.
//!
//! Provider records stay `declared`. The collector adds only facts it observes
//! itself: receipt order and time, channel state, budgets and losses. A record
//! is durable once `ActivitySpool::append` returns; a gated hook is
//! acknowledged only after that point.
use super::spool::{ActivitySpool, SpooledEvent};
use crate::{
    adapters::{AdapterContext, RecordDisposition, normalize_hook_record, normalize_stdout_record},
    model::Event,
    redaction::Redactor,
};
use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Default per-session budgets (coordinator decision, 21 Nov 2026): sized so a
/// default tenant audit (100,000 events, 256 MiB) holds at least 50 sessions.
pub const DEFAULT_MAX_EVENTS: u64 = 1_000;
pub const DEFAULT_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// Highest configurable per-session budget.
pub const MAX_SESSION_EVENTS: u64 = 5_000;
pub const MAX_SESSION_BYTES: u64 = 8 * 1024 * 1024;
/// Undelivered backlog above which the gateway path counts as unavailable.
pub const DEFAULT_MAX_BACKLOG_EVENTS: u64 = 200;
pub const DEFAULT_MAX_BACKLOG_BYTES: u64 = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ActivityLimits {
    pub max_events: u64,
    pub max_bytes: u64,
    pub max_backlog_events: u64,
    pub max_backlog_bytes: u64,
}

impl Default for ActivityLimits {
    fn default() -> Self {
        Self {
            max_events: DEFAULT_MAX_EVENTS,
            max_bytes: DEFAULT_MAX_BYTES,
            max_backlog_events: DEFAULT_MAX_BACKLOG_EVENTS,
            max_backlog_bytes: DEFAULT_MAX_BACKLOG_BYTES,
        }
    }
}

impl ActivityLimits {
    /// Operator configuration may lower or raise the budgets within hard bounds.
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            (1..=MAX_SESSION_EVENTS).contains(&self.max_events)
                && (64 * 1024..=MAX_SESSION_BYTES).contains(&self.max_bytes)
                && (1..=self.max_events).contains(&self.max_backlog_events)
                && (64 * 1024..=self.max_bytes).contains(&self.max_backlog_bytes),
            "provider activity budgets are outside their supported bounds"
        );
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActivityClient {
    ClaudeCode,
    Codex,
}

impl ActivityClient {
    fn adapter(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityChannel {
    Stdout,
    Hook,
}

/// Trusted identity of the supervised execution; never read from the worker.
#[derive(Clone, Debug)]
pub struct ActivityContext {
    pub session: Uuid,
    pub session_id: Option<String>,
    pub project_id: Option<String>,
    pub sandbox_id: Option<String>,
    pub agent_id: Option<String>,
    pub correlation_id: Option<String>,
    pub trace_id: Option<String>,
    pub worker_id: Option<String>,
    pub attempt_id: Option<String>,
    pub client: ActivityClient,
    pub interactive: bool,
    /// Version requested for the image (control-side configuration).
    pub client_version_label: Option<String>,
}

/// Outcome returned to a channel owner for one record or hook submission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Durably spooled (possibly with zero projected events).
    Recorded,
    /// Not spooled: budget exhausted or record refused; counted as a loss.
    Dropped,
    /// The audit path is unavailable; the caller stalls (stdout) or refuses (hook).
    Unavailable,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ChannelCounts {
    pub records: u64,
    pub mapped: u64,
    pub ignored: u64,
    pub unrecognized: u64,
    pub oversized: u64,
    pub invalid: u64,
    pub events: u64,
    pub dropped: u64,
    pub refused: u64,
    pub declared_losses: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ActivitySummary {
    pub version: u32,
    pub client: ActivityClient,
    pub interactive: bool,
    pub client_version_label: Option<String>,
    pub stdout: Option<ChannelCounts>,
    pub hook: ChannelCounts,
    pub hook_channel: &'static str,
    pub spooled_events: u64,
    pub spooled_bytes: u64,
    pub delivered_events: u64,
    pub budget: ActivityLimits,
    pub budget_exhausted: bool,
    pub audit_unavailable: bool,
    pub stalled: bool,
    pub complete: bool,
}

struct State {
    spool: ActivitySpool,
    stdout: ChannelCounts,
    hook: ChannelCounts,
    hook_channel: &'static str,
    budget_exhausted: bool,
    stalled: bool,
    record_seq: [u64; 2],
}

/// Shared by the stdout pump, the hook channel and the forwarder.
#[derive(Clone)]
pub struct ActivityCollector {
    state: Arc<Mutex<State>>,
    context: Arc<ActivityContext>,
    adapter: Arc<AdapterContext>,
    redactor: Redactor,
    limits: ActivityLimits,
}

impl std::fmt::Debug for ActivityCollector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActivityCollector")
            .field("session", &self.context.session)
            .finish_non_exhaustive()
    }
}

impl ActivityCollector {
    pub fn new(
        spool: ActivitySpool,
        context: ActivityContext,
        redactor: Redactor,
        limits: ActivityLimits,
    ) -> Result<Self> {
        limits.validate()?;
        let adapter = AdapterContext {
            session_id: context.session_id.clone().unwrap_or_default(),
            project_id: context.project_id.clone().unwrap_or_default(),
            sandbox_id: context.sandbox_id.clone().unwrap_or_default(),
            agent_id: context.agent_id.clone().unwrap_or_default(),
            correlation_id: context.correlation_id.clone().unwrap_or_default(),
            trace_id: context.trace_id.clone().unwrap_or_default(),
        };
        let budget_exhausted =
            spool.len() >= limits.max_events || spool.bytes() >= limits.max_bytes;
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                spool,
                stdout: ChannelCounts::default(),
                hook: ChannelCounts::default(),
                hook_channel: "not_attached",
                budget_exhausted,
                stalled: false,
                record_seq: [0, 0],
            })),
            context: Arc::new(context),
            adapter: Arc::new(adapter),
            redactor,
            limits,
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>> {
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("provider activity collector is poisoned"))
    }

    /// One complete stdout line (batch mode only), without its delimiter.
    pub fn stdout_line(&self, line: &[u8]) -> Admission {
        let normalized = match std::str::from_utf8(line) {
            Ok(text) => normalize_stdout_record(
                self.context.client.adapter(),
                text,
                &self.adapter,
                &self.redactor,
            ),
            Err(_) => crate::adapters::NormalizedRecord {
                events: Vec::new(),
                disposition: RecordDisposition::Invalid,
            },
        };
        self.admit(
            ActivityChannel::Stdout,
            normalized.disposition,
            normalized.events,
        )
    }

    /// One hook payload forwarded by the worker helper.
    pub fn hook(&self, payload: &str, truncated: bool) -> Admission {
        let normalized = normalize_hook_record(
            self.context.client.adapter(),
            payload,
            &self.adapter,
            &self.redactor,
        );
        let mut events = normalized.events;
        if truncated {
            for event in &mut events {
                event["data"]["hook_input_truncated"] = json!(true);
            }
        }
        let admission = self.admit(ActivityChannel::Hook, normalized.disposition, events);
        // A hook is never retried: an outage refusal is counted here once.
        if admission == Admission::Unavailable
            && let Ok(mut state) = self.lock()
        {
            state.hook.refused += 1;
        }
        admission
    }

    /// Losses the worker helper reports. They are worker declarations.
    pub fn declared_hook_loss(&self, count: u64, reason: &str) -> Admission {
        let Ok(mut state) = self.lock() else {
            return Admission::Unavailable;
        };
        state.hook.declared_losses = state.hook.declared_losses.saturating_add(count);
        let event = self.control_event(
            "activity.loss",
            "declared",
            json!({"channel":"hook","count":count,"reason":crate::redaction::clip(reason, 64),"reporter":"worker_activity_helper"}),
        );
        self.append_locked(&mut state, vec![event])
    }

    /// An observed change of channel state, spooled like provider events.
    pub fn channel_state(
        &self,
        channel: ActivityChannel,
        state_name: &'static str,
        reason: Option<&str>,
    ) -> Admission {
        let Ok(mut state) = self.lock() else {
            return Admission::Unavailable;
        };
        if channel == ActivityChannel::Hook {
            state.hook_channel = state_name;
        }
        let event = self.control_event(
            "activity.channel",
            "observed",
            json!({"channel":channel,"state":state_name,"reason":reason.map(|r| crate::redaction::clip(r, 200))}),
        );
        self.append_locked(&mut state, vec![event])
    }

    /// True while the spool failed or the undelivered backlog is over budget.
    pub fn audit_unavailable(&self) -> bool {
        self.lock()
            .map_or(true, |state| self.unavailable_locked(&state))
    }

    fn unavailable_locked(&self, state: &State) -> bool {
        let (events, bytes) = state.spool.backlog();
        state.spool.failed()
            || events >= self.limits.max_backlog_events
            || bytes >= self.limits.max_backlog_bytes
    }

    /// Record that a channel owner is stalling because of an outage.
    pub fn set_stalled(&self, stalled: bool) {
        if let Ok(mut state) = self.lock() {
            state.stalled |= stalled;
        }
    }

    fn counts(state: &mut State, channel: ActivityChannel) -> &mut ChannelCounts {
        match channel {
            ActivityChannel::Stdout => &mut state.stdout,
            ActivityChannel::Hook => &mut state.hook,
        }
    }

    /// A stdout record that could not be framed (over-long or unterminated).
    /// It is counted and spooled as one observed loss marker.
    pub fn stdout_unframed(&self, reason: &'static str) -> Admission {
        let Ok(mut state) = self.lock() else {
            return Admission::Unavailable;
        };
        let counts = Self::counts(&mut state, ActivityChannel::Stdout);
        counts.records += 1;
        if reason == "oversized_record" {
            counts.oversized += 1;
        } else {
            counts.invalid += 1;
        }
        let marker = self.control_event(
            "activity.loss",
            "observed",
            json!({"channel":"stdout","count":1,"reason":reason}),
        );
        self.append_locked(&mut state, vec![marker])
    }

    /// Bytes the stdout tap stopped forwarding after its stall bound: one
    /// observed loss for the rest of the stream.
    pub fn stdout_overflow(&self, bytes: u64) -> Admission {
        let Ok(mut state) = self.lock() else {
            return Admission::Unavailable;
        };
        Self::counts(&mut state, ActivityChannel::Stdout).dropped += 1;
        let marker = self.control_event(
            "activity.loss",
            "observed",
            json!({"channel":"stdout","bytes":bytes,"reason":"stdout_stall_limit_exceeded"}),
        );
        self.append_locked(&mut state, vec![marker])
    }

    /// Admission order: an outage refuses before counting, so a stalled
    /// caller can retry the same record; then budget, then the durable append.
    fn admit(
        &self,
        channel: ActivityChannel,
        disposition: RecordDisposition,
        events: Vec<Value>,
    ) -> Admission {
        let Ok(mut state) = self.lock() else {
            return Admission::Unavailable;
        };
        if self.unavailable_locked(&state) {
            return Admission::Unavailable;
        }
        let index = channel as usize;
        state.record_seq[index] += 1;
        let record_seq = state.record_seq[index];
        let counts = Self::counts(&mut state, channel);
        counts.records += 1;
        match disposition {
            RecordDisposition::Mapped => counts.mapped += 1,
            RecordDisposition::Ignored => counts.ignored += 1,
            RecordDisposition::Unrecognized => counts.unrecognized += 1,
            RecordDisposition::Oversized => counts.oversized += 1,
            RecordDisposition::Invalid => counts.invalid += 1,
        }
        if state.budget_exhausted {
            Self::counts(&mut state, channel).dropped += 1;
            return Admission::Dropped;
        }
        let projected = events.len() as u64;
        let mut stamped: Vec<Event> = Vec::with_capacity(events.len());
        for value in events {
            match self.stamp(value, channel, record_seq) {
                Ok(event) => stamped.push(event),
                Err(_) => {
                    Self::counts(&mut state, channel).dropped += 1;
                    return Admission::Dropped;
                }
            }
        }
        let size: u64 = stamped
            .iter()
            .map(|e| serde_json::to_string(e).map_or(0, |s| s.len() as u64))
            .sum();
        if state.spool.len() + projected > self.limits.max_events
            || state.spool.bytes() + size > self.limits.max_bytes
        {
            state.budget_exhausted = true;
            Self::counts(&mut state, channel).dropped += 1;
            // One explicit, observed marker; it may exceed the budget by one.
            let marker = self.control_event(
                "activity.loss",
                "observed",
                json!({"channel":channel,"count":1,"reason":"session_budget_exhausted","max_events":self.limits.max_events,"max_bytes":self.limits.max_bytes}),
            );
            let _ = self.append_locked(&mut state, vec![marker]);
            return Admission::Dropped;
        }
        if stamped.is_empty() {
            return Admission::Recorded;
        }
        match self.append_locked(&mut state, stamped) {
            Admission::Recorded => {
                Self::counts(&mut state, channel).events += projected;
                Admission::Recorded
            }
            other => {
                Self::counts(&mut state, channel).refused += 1;
                other
            }
        }
    }

    fn append_locked(&self, state: &mut State, events: Vec<Event>) -> Admission {
        match state.spool.append(events) {
            Ok(_) => Admission::Recorded,
            Err(_) => Admission::Unavailable,
        }
    }

    fn activity_meta(&self, channel: Option<ActivityChannel>, record_seq: Option<u64>) -> Value {
        json!({
            "version":1,
            "channel":channel,
            "record_seq":record_seq,
            "client":self.context.client,
            "interactive":self.context.interactive,
            "client_version_label":self.context.client_version_label,
            "worker_id":self.context.worker_id,
            "attempt_id":self.context.attempt_id,
            "received_at":Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        })
    }

    fn stamp(&self, mut value: Value, channel: ActivityChannel, record_seq: u64) -> Result<Event> {
        value["data"]["activity"] = self.activity_meta(Some(channel), Some(record_seq));
        let mut event: Event = serde_json::from_value(value).context("normalized event shape")?;
        self.contextualize(&mut event);
        Ok(event)
    }

    fn control_event(&self, kind: &str, provenance: &str, mut data: Value) -> Event {
        data["activity"] = self.activity_meta(None, None);
        let mut event = Event::new(kind);
        event.source = "runner".into();
        event.provenance = provenance.into();
        event.coverage = "observed".into();
        event.data = data;
        self.contextualize(&mut event);
        event
    }

    /// Trusted context, deterministic id, receipt time and a final masking.
    fn contextualize(&self, event: &mut Event) {
        event.session_id = self.context.session_id.clone();
        event.project_id = self.context.project_id.clone();
        event.sandbox_id = self.context.sandbox_id.clone();
        event.agent_id = self.context.agent_id.clone();
        event.parent_agent_id = None;
        event.correlation_id = self.context.correlation_id.clone();
        event.trace_id = self.context.trace_id.clone();
        if let Some(at) = event.data["activity"]["received_at"].as_str() {
            event.timestamp = at.to_owned();
        }
        event.data = self.redactor.value(&event.data);
        event.result = event.result.as_ref().map(|r| self.redactor.value(r));
    }

    /// Next undelivered events for the forwarder. Ids were derived from the
    /// spool position at append, so a re-delivery is an idempotent replay.
    pub fn pending(&self, max_events: usize, max_bytes: usize) -> Vec<SpooledEvent> {
        self.lock()
            .map(|state| state.spool.pending(max_events, max_bytes))
            .unwrap_or_default()
    }

    pub fn mark_delivered(&self, through: u64) -> Result<()> {
        self.lock()?.spool.mark_delivered(through)
    }

    pub fn summary(&self) -> Result<ActivitySummary> {
        let state = self.lock()?;
        let audit_unavailable = self.unavailable_locked(&state);
        let delivered = state.spool.delivered_through();
        let complete = !state.budget_exhausted
            && !audit_unavailable
            && delivered == state.spool.len()
            && state.hook.refused == 0
            && state.hook.dropped == 0
            && state.hook.declared_losses == 0
            && state.stdout.dropped == 0
            && state.stdout.refused == 0
            && state.stdout.oversized == 0
            && state.stdout.invalid == 0;
        Ok(ActivitySummary {
            version: 1,
            client: self.context.client,
            interactive: self.context.interactive,
            client_version_label: self.context.client_version_label.clone(),
            stdout: (!self.context.interactive).then(|| state.stdout.clone()),
            hook: state.hook.clone(),
            hook_channel: state.hook_channel,
            spooled_events: state.spool.len(),
            spooled_bytes: state.spool.bytes(),
            delivered_events: delivered,
            budget: self.limits,
            budget_exhausted: state.budget_exhausted,
            audit_unavailable,
            stalled: state.stalled,
            complete,
        })
    }

    /// Spool the observed coverage summary; it is the last collector event.
    pub fn finish(&self) -> Result<ActivitySummary> {
        let summary = self.summary()?;
        let mut state = self.lock()?;
        let event = self.control_event(
            "activity.coverage",
            "observed",
            serde_json::to_value(&summary)?,
        );
        // The summary is always spooled, even past the budget: a coverage
        // statement must not be dropped for lack of room.
        state
            .spool
            .append(vec![event])
            .context("provider activity coverage cannot be spooled")?;
        Ok(summary)
    }
}
