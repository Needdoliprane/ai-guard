//! Live collector: normalize, mask, stamp and spool provider activity.
//!
//! Provider records stay `declared`. The collector adds only facts it observes
//! itself: receipt order and time, channel state, budgets and losses. A record
//! is durable once `ActivitySpool::append` returns; a gated hook is
//! acknowledged only after that point.
use super::{
    hooks::HookPumpOutcome,
    spool::{ActivitySpool, SpooledEvent},
    stdout::StdoutLossReason,
};
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
/// Up to one exhaustion marker and one terminal coverage record may exceed
/// the ordinary session budget. Each has its own fixed encoded size bound.
const MAX_RESERVED_RECORD_BYTES: u64 = 16 * 1024;
const BUDGET_EXHAUSTION_REASON: &str = "session_budget_exhausted";

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum HookCoverage {
    Unattached,
    Attached,
    Closed,
    Drained,
    Incomplete,
}

impl HookCoverage {
    fn observe(self, name: &str, reason: Option<&str>, admission: Admission) -> Self {
        match (self, name, reason, admission) {
            (Self::Unattached, "attached", None, Admission::Recorded) => Self::Attached,
            (Self::Attached, "closed", None, Admission::Recorded) => Self::Closed,
            _ => Self::Incomplete,
        }
    }
}

struct State {
    spool: ActivitySpool,
    stdout: ChannelCounts,
    hook: ChannelCounts,
    hook_channel: &'static str,
    hook_coverage: HookCoverage,
    stdout_failed: bool,
    /// The stdout of this session is collected live (batch with a tap).
    stdout_live: bool,
    budget_exhausted: bool,
    stalled: bool,
    record_seq: [u64; 2],
    coverage: CoverageFinalization,
}

// Bounded collector state value; keep value semantics.
#[allow(clippy::large_enum_variant)]
enum CoverageFinalization {
    Open,
    Live(ActivitySummary),
    Retained,
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
        let mut budget_exhausted =
            spool.len() > limits.max_events || spool.bytes() > limits.max_bytes;
        let mut coverage = CoverageFinalization::Open;
        let mut stdout = ChannelCounts::default();
        for entry in spool.events() {
            let event = &entry.event;
            if event.kind == "activity.loss"
                && event.source == "runner"
                && event.provenance == "observed"
                && event.data["reason"] == BUDGET_EXHAUSTION_REASON
            {
                budget_exhausted = true;
            }
            if event.kind == "activity.coverage" {
                coverage = CoverageFinalization::Retained;
            }
            if event.source == "codex-adapter"
                && event.provenance == "declared"
                && event.data["provider_reported_loss"] == true
            {
                stdout.declared_losses += 1;
            }
        }
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                spool,
                stdout,
                hook: ChannelCounts::default(),
                hook_channel: "not_attached",
                hook_coverage: HookCoverage::Unattached,
                stdout_failed: false,
                stdout_live: false,
                budget_exhausted,
                stalled: false,
                record_seq: [0, 0],
                coverage,
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
        self.hook_record(payload, truncated).0
    }

    /// Same as `hook`, also returning how the payload was understood. A gated
    /// hook is acknowledged `recorded` only when it was mapped to at least one
    /// durable event: an unreadable gated payload leaves no audit record.
    pub fn hook_record(&self, payload: &str, truncated: bool) -> (Admission, RecordDisposition) {
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
        let disposition = normalized.disposition;
        let admission = self.admit(ActivityChannel::Hook, disposition, events);
        (admission, disposition)
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
        self.append_locked(&mut state, ActivityChannel::Hook, vec![event])
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
        match channel {
            ActivityChannel::Hook => state.hook_channel = state_name,
            ActivityChannel::Stdout if state_name == "opened" => state.stdout_live = true,
            ActivityChannel::Stdout
                if matches!(state_name, "failed" | "closure_unknown")
                    || (state_name == "closed" && reason.is_some()) =>
            {
                state.stdout_failed = true;
            }
            ActivityChannel::Stdout => {}
        }
        let event = self.control_event(
            "activity.channel",
            "observed",
            json!({"channel":channel,"state":state_name,"reason":reason.map(|r| crate::redaction::clip(r, 200))}),
        );
        let admission = self.append_locked(&mut state, channel, vec![event]);
        if channel == ActivityChannel::Hook {
            state.hook_coverage = state.hook_coverage.observe(state_name, reason, admission);
        }
        admission
    }

    /// Called by the session owner with the final outcome of the joined hook task.
    pub(crate) fn confirm_hook_drain(&self, outcome: &HookPumpOutcome) -> Result<()> {
        let mut state = self.lock()?;
        // The pump counted the same submitted hooks, plus refusals that never
        // reached the collector or left no durable event. Do not add the totals.
        state.hook.refused = state
            .hook
            .refused
            .max(outcome.refused)
            .max(outcome.unrecorded_gates);
        if state.hook.refused > 0
            || outcome.transport_error
            || outcome.sequence_gaps > 0
            || outcome.declared_losses > 0
        {
            state.hook_coverage = HookCoverage::Incomplete;
        } else if state.hook_coverage == HookCoverage::Closed {
            state.hook_coverage = HookCoverage::Drained;
        }
        Ok(())
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
        self.append_locked(&mut state, ActivityChannel::Stdout, vec![marker])
    }

    /// Bytes the stdout tap stopped forwarding after its stall bound: one
    /// observed loss for the rest of the stream.
    pub fn stdout_overflow(&self, bytes: u64) -> Admission {
        self.record_stdout_loss(bytes, StdoutLossReason::StallDeadline)
    }

    pub(super) fn record_stdout_loss(&self, bytes: u64, reason: StdoutLossReason) -> Admission {
        let Ok(mut state) = self.lock() else {
            return Admission::Unavailable;
        };
        Self::counts(&mut state, ActivityChannel::Stdout).dropped += 1;
        let reason = match reason {
            StdoutLossReason::StallDeadline => "stdout_stall_limit_exceeded",
            StdoutLossReason::NotForwarded => "stdout_not_forwarded",
        };
        let mut marker = self.control_event(
            "activity.loss",
            "observed",
            json!({"channel":"stdout","bytes":bytes}),
        );
        marker.data["reason"] = json!(reason);
        self.append_locked(&mut state, ActivityChannel::Stdout, vec![marker])
    }

    /// An outage refuses before counting a stdout record so the pump can retry.
    /// A hook is refused once. Available records proceed to budget and append.
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
            if channel == ActivityChannel::Hook {
                state.hook.refused += 1;
            }
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
        if channel == ActivityChannel::Stdout
            && events
                .iter()
                .any(|event| event["data"]["provider_reported_loss"] == true)
        {
            counts.declared_losses = counts.declared_losses.saturating_add(1);
        }
        if state.budget_exhausted || !matches!(state.coverage, CoverageFinalization::Open) {
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
        if stamped.is_empty() {
            return Admission::Recorded;
        }
        match self.append_locked(&mut state, channel, stamped) {
            Admission::Recorded => {
                Self::counts(&mut state, channel).events += projected;
                Admission::Recorded
            }
            Admission::Dropped => {
                Self::counts(&mut state, channel).dropped += 1;
                Admission::Dropped
            }
            other => {
                Self::counts(&mut state, channel).refused += 1;
                other
            }
        }
    }

    fn append_locked(
        &self,
        state: &mut State,
        channel: ActivityChannel,
        events: Vec<Event>,
    ) -> Admission {
        if state.budget_exhausted || !matches!(state.coverage, CoverageFinalization::Open) {
            return Admission::Dropped;
        }
        match state
            .spool
            .append_bounded(events, self.limits.max_events, self.limits.max_bytes)
        {
            Ok(Some(_)) => return Admission::Recorded,
            Err(_) => return Admission::Unavailable,
            Ok(None) => {}
        }
        state.budget_exhausted = true;
        let mut marker = self.control_event(
            "activity.loss",
            "observed",
            json!({"channel":channel,"count":1,"reason":BUDGET_EXHAUSTION_REASON,"max_events":self.limits.max_events,"max_bytes":self.limits.max_bytes}),
        );
        // This fixed code also identifies the reserved marker after reopening.
        marker.data["reason"] = json!(BUDGET_EXHAUSTION_REASON);
        match Self::append_reserved(state, marker) {
            Ok(()) => Admission::Dropped,
            Err(_) => Admission::Unavailable,
        }
    }

    fn append_reserved(state: &mut State, event: Event) -> Result<()> {
        let max_events = state.spool.len().saturating_add(1);
        let max_bytes = state
            .spool
            .bytes()
            .saturating_add(MAX_RESERVED_RECORD_BYTES);
        state
            .spool
            .append_bounded(vec![event], max_events, max_bytes)?
            .context("reserved provider activity record exceeds its bound")?;
        Ok(())
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
        Ok(self.summary_locked(&state))
    }

    fn summary_locked(&self, state: &State) -> ActivitySummary {
        let audit_unavailable = self.unavailable_locked(state);
        let delivered = state.spool.delivered_through();
        let complete = state.hook_coverage == HookCoverage::Drained
            && !state.stdout_failed
            && !state.budget_exhausted
            && !audit_unavailable
            && delivered == state.spool.len()
            && state.hook.refused == 0
            && state.hook.dropped == 0
            && state.hook.declared_losses == 0
            && state.hook.oversized == 0
            && state.hook.invalid == 0
            && state.stdout.dropped == 0
            && state.stdout.refused == 0
            && state.stdout.declared_losses == 0
            && state.stdout.oversized == 0
            && state.stdout.invalid == 0;
        ActivitySummary {
            version: 1,
            client: self.context.client,
            interactive: self.context.interactive,
            client_version_label: self.context.client_version_label.clone(),
            stdout: (state.stdout_live
                || state.stdout.records > 0
                || state.stdout.declared_losses > 0)
                .then(|| state.stdout.clone()),
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
        }
    }

    /// Give a retained terminal spool a coverage event when collection ended
    /// without a final summary. An existing event keeps its bytes and identity.
    pub(crate) fn ensure_replay_coverage(&self) -> Result<()> {
        let mut state = self.lock()?;
        if !matches!(state.coverage, CoverageFinalization::Open) {
            return Ok(());
        }
        let mut event = self.control_event(
            "activity.coverage",
            "observed",
            json!({
                "version": 1,
                "complete": false,
                "reason": "activity_finalization_unconfirmed",
            }),
        );
        event.coverage = "partial".into();
        Self::append_reserved(&mut state, event)
            .context("unconfirmed provider activity coverage cannot be spooled")?;
        state.coverage = CoverageFinalization::Retained;
        Ok(())
    }

    /// Spool the observed coverage summary; it is the last collector event.
    pub fn finish(&self) -> Result<ActivitySummary> {
        let mut state = self.lock()?;
        match &state.coverage {
            CoverageFinalization::Open => {}
            CoverageFinalization::Live(summary) => return Ok(summary.clone()),
            CoverageFinalization::Retained => {
                anyhow::bail!("retained provider activity already has a coverage record")
            }
        }
        let summary = self.summary_locked(&state);
        let event = self.control_event(
            "activity.coverage",
            "observed",
            serde_json::to_value(&summary)?,
        );
        Self::append_reserved(&mut state, event)
            .context("provider activity coverage cannot be spooled")?;
        state.coverage = CoverageFinalization::Live(summary.clone());
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_fs::Directory;
    use std::os::unix::fs::PermissionsExt;

    fn fixture(interactive: bool) -> (tempfile::TempDir, ActivityCollector) {
        fixture_with_limits(interactive, ActivityLimits::default())
    }

    fn fixture_with_limits(
        interactive: bool,
        limits: ActivityLimits,
    ) -> (tempfile::TempDir, ActivityCollector) {
        client_fixture(ActivityClient::ClaudeCode, interactive, limits)
    }

    fn client_fixture(
        client: ActivityClient,
        interactive: bool,
        limits: ActivityLimits,
    ) -> (tempfile::TempDir, ActivityCollector) {
        let temp = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let directory = Directory::open_existing(temp.path()).unwrap();
        let session = Uuid::new_v4();
        let collector = ActivityCollector::new(
            ActivitySpool::create(&directory, session).unwrap(),
            ActivityContext {
                session,
                session_id: Some(session.to_string()),
                project_id: None,
                sandbox_id: None,
                agent_id: None,
                correlation_id: None,
                trace_id: None,
                worker_id: None,
                attempt_id: None,
                client,
                interactive,
                client_version_label: None,
            },
            Redactor::new(vec![]),
            limits,
        )
        .unwrap();
        (temp, collector)
    }

    fn deliver_pending(collector: &ActivityCollector) {
        collector
            .mark_delivered(collector.summary().unwrap().spooled_events)
            .unwrap();
    }

    #[test]
    fn hook_coverage_requires_attachment_and_confirmed_drain_in_batch_and_tty() {
        for interactive in [false, true] {
            let (_temp, collector) = fixture(interactive);
            assert!(!collector.summary().unwrap().complete);
            assert_eq!(
                collector.channel_state(ActivityChannel::Hook, "attached", None),
                Admission::Recorded
            );
            deliver_pending(&collector);
            assert!(!collector.summary().unwrap().complete);
            assert_eq!(
                collector.channel_state(ActivityChannel::Hook, "closed", None),
                Admission::Recorded
            );
            deliver_pending(&collector);
            assert!(!collector.summary().unwrap().complete);

            collector
                .confirm_hook_drain(&HookPumpOutcome::default())
                .unwrap();
            let summary = collector.finish().unwrap();
            assert!(summary.complete);
            assert_eq!(summary.hook.records, 0);
            let pending = collector.pending(1, usize::MAX);
            assert_eq!(pending[0].event.kind, "activity.coverage");
            assert_eq!(pending[0].event.data["complete"], true);
        }
    }

    #[test]
    fn channel_faults_keep_coverage_partial_after_a_later_close() {
        for (channel, fault, reason) in [
            (ActivityChannel::Hook, "sequence_gap", None),
            (ActivityChannel::Hook, "closure_unknown", None),
            (ActivityChannel::Hook, "closed", Some("hook_drain_deadline")),
            (ActivityChannel::Stdout, "failed", None),
        ] {
            let (_temp, collector) = fixture(false);
            collector.channel_state(ActivityChannel::Hook, "attached", None);
            collector.channel_state(channel, fault, reason);
            collector.channel_state(ActivityChannel::Hook, "closed", None);
            collector
                .confirm_hook_drain(&HookPumpOutcome::default())
                .unwrap();
            deliver_pending(&collector);

            let summary = collector.finish().unwrap();
            assert!(!summary.complete, "reported complete after {fault}");
            assert_eq!(summary.hook.dropped, 0);
            assert_eq!(
                collector.pending(1, usize::MAX)[0].event.data["complete"],
                false
            );
        }
    }

    #[test]
    fn invalid_and_oversized_hook_records_leave_coverage_partial() {
        let oversized = " ".repeat(crate::adapters::MAX_LINE_BYTES + 1);
        for (payload, expected, invalid, oversized) in [
            ("not JSON", RecordDisposition::Invalid, 1, 0),
            (oversized.as_str(), RecordDisposition::Oversized, 0, 1),
        ] {
            let (_temp, collector) = fixture(true);
            collector.channel_state(ActivityChannel::Hook, "attached", None);
            assert_eq!(
                collector.hook_record(payload, false),
                (Admission::Recorded, expected)
            );
            collector.channel_state(ActivityChannel::Hook, "closed", None);
            collector
                .confirm_hook_drain(&HookPumpOutcome::default())
                .unwrap();
            deliver_pending(&collector);

            let summary = collector.finish().unwrap();
            assert_eq!(
                (summary.hook.invalid, summary.hook.oversized),
                (invalid, oversized)
            );
            assert_eq!(summary.hook.events, 0);
            assert!(!summary.complete, "reported complete after {expected:?}");
        }
    }

    #[test]
    fn undurable_hook_refusals_survive_a_clean_drain_and_are_counted_once() {
        for backlogged in [false, true] {
            let (_temp, collector) = fixture_with_limits(
                false,
                ActivityLimits {
                    max_backlog_events: if backlogged {
                        1
                    } else {
                        DEFAULT_MAX_BACKLOG_EVENTS
                    },
                    ..ActivityLimits::default()
                },
            );
            collector.channel_state(ActivityChannel::Hook, "attached", None);
            let (admission, disposition) =
                collector.hook_record(r#"{"hook_event_name":"PreToolUse"}"#, false);
            assert_eq!(disposition, RecordDisposition::Unrecognized);
            assert_eq!(
                admission,
                if backlogged {
                    Admission::Unavailable
                } else {
                    Admission::Recorded
                }
            );
            deliver_pending(&collector);
            collector.channel_state(ActivityChannel::Hook, "closed", None);
            deliver_pending(&collector);

            collector
                .confirm_hook_drain(&HookPumpOutcome {
                    hooks: 1,
                    refused: 1,
                    unrecorded_gates: u64::from(!backlogged),
                    ..HookPumpOutcome::default()
                })
                .unwrap();
            let summary = collector.finish().unwrap();
            assert!(!summary.complete);
            assert!(!summary.audit_unavailable);
            assert_eq!(summary.hook.refused, 1);
            assert_eq!(summary.hook.events, 0);
            assert_eq!(summary.hook.unrecognized, u64::from(!backlogged));
        }
    }

    #[test]
    fn stdout_counts_recognized_messages_separately_from_unstructured_text() {
        let (_temp, collector) = fixture(false);
        collector.channel_state(ActivityChannel::Stdout, "opened", None);
        collector.channel_state(ActivityChannel::Hook, "attached", None);
        assert_eq!(
            collector.stdout_line(br#"{"type":"system","subtype":"init","session_id":"fixture"}"#),
            Admission::Recorded
        );
        assert_eq!(
            collector.stdout_line(br#"{"type":"stream_event"}"#),
            Admission::Recorded
        );
        collector.channel_state(ActivityChannel::Hook, "closed", None);
        collector
            .confirm_hook_drain(&HookPumpOutcome::default())
            .unwrap();
        deliver_pending(&collector);
        let summary = collector.summary().unwrap();
        assert!(summary.complete);
        let stdout = summary.stdout.unwrap();
        assert_eq!((stdout.mapped, stdout.ignored, stdout.invalid), (1, 1, 0));

        assert_eq!(
            collector.stdout_line(b"unstructured text"),
            Admission::Recorded
        );
        deliver_pending(&collector);
        let summary = collector.finish().unwrap();
        assert!(!summary.complete);
        let stdout = summary.stdout.unwrap();
        assert_eq!((stdout.mapped, stdout.ignored, stdout.invalid), (1, 1, 1));
    }

    #[test]
    fn a_provider_reported_stdout_loss_keeps_an_otherwise_complete_session_partial() {
        let (_temp, collector) =
            client_fixture(ActivityClient::Codex, false, ActivityLimits::default());
        collector.channel_state(ActivityChannel::Stdout, "opened", None);
        collector.channel_state(ActivityChannel::Hook, "attached", None);
        collector.channel_state(ActivityChannel::Hook, "closed", None);
        collector
            .confirm_hook_drain(&HookPumpOutcome::default())
            .unwrap();
        deliver_pending(&collector);
        assert!(collector.summary().unwrap().complete);

        assert_eq!(
            collector.stdout_line(br#"{"type":"item.completed","item":{"id":"item","type":"error","message":"in-process app-server event stream lagged; dropped 3 events"}}"#),
            Admission::Recorded,
        );
        let pending = collector.pending(1, usize::MAX);
        assert_eq!(pending[0].event.provenance, "declared");
        assert_eq!(pending[0].event.data["provider_reported_loss"], true);
        deliver_pending(&collector);

        let summary = collector.finish().unwrap();
        assert!(!summary.complete);
        let stdout = summary.stdout.unwrap();
        assert_eq!(
            (stdout.declared_losses, stdout.dropped, stdout.invalid),
            (1, 0, 0)
        );
        assert_eq!(
            collector.pending(1, usize::MAX)[0].event.data["complete"],
            false
        );
    }

    #[tokio::test]
    async fn stdout_loss_reports_the_first_abandoned_chunk_without_inventing_a_deadline() {
        use super::super::stdout::{TAP_CHUNKS, pump, stdout_tap_with_stall};
        use std::time::Duration;

        enum End {
            Deadline,
            Transport,
            Abandoned,
        }

        for (end, reason) in [
            (End::Deadline, "stdout_stall_limit_exceeded"),
            (End::Transport, "stdout_not_forwarded"),
            (End::Abandoned, "stdout_not_forwarded"),
        ] {
            let (_temp, collector) = fixture(false);
            collector.channel_state(ActivityChannel::Stdout, "opened", None);
            let stall = if matches!(end, End::Deadline) {
                Duration::from_millis(10)
            } else {
                Duration::from_secs(30)
            };
            let (tap, chunks) = stdout_tap_with_stall(stall);
            let release = tap.stall_release();
            for _ in 0..TAP_CHUNKS {
                tap.send(b"").await;
            }
            let mut send = Box::pin(tap.send(b"pending"));
            tokio::select! {
                biased;
                () = &mut send => panic!("the full tap must apply backpressure"),
                () = std::future::ready(()) => {}
            }
            match end {
                End::Deadline => send.await,
                End::Transport => {
                    release.send_replace(true);
                    send.await;
                }
                End::Abandoned => drop(send),
            }
            tap.send(b"tail").await;
            drop(tap);

            assert_eq!(pump(collector.clone(), chunks).await.overflow_bytes, 11);
            let losses: Vec<_> = collector
                .pending(8, usize::MAX)
                .into_iter()
                .filter(|item| item.event.kind == "activity.loss")
                .collect();
            assert_eq!(losses.len(), 1);
            assert_eq!(losses[0].event.provenance, "observed");
            assert_eq!(losses[0].event.data["reason"], reason);
            assert_eq!(losses[0].event.data["bytes"], 11);
            assert_eq!(collector.summary().unwrap().stdout.unwrap().dropped, 1);
        }
    }
}
