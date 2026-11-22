//! Live provider activity: durable spool, collector, stdout framing, forwarder
//! and the runner-only gateway batch route, on private temporary fixtures.
//! No runtime, network, provider or real credential is used; the gateway is
//! exercised in process through its router.
mod support;

use agentsandbox::{
    api::{GatewayState, gateway_router},
    audit::Store,
    control_fs::Directory,
    model::Event,
    policy::Policy,
    provider_activity::{
        ActivityChannel, ActivityClient, ActivityCollector, ActivityContext, ActivityLimits,
        ActivitySink, ActivitySpool, Admission, forward, pump, stdout_tap, stdout_tap_with_stall,
    },
    redaction::Redactor,
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tower::ServiceExt;
use uuid::Uuid;

const CLAUDE_STREAM: &str = include_str!("adapters_fixtures/claude-code.stream.jsonl");
const CLAUDE_HOOKS: &str = include_str!("adapters_fixtures/claude-code.hooks.jsonl");
const RUNNER: &str = "rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr";
const VIEWER: &str = "vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv";

fn directory() -> (tempfile::TempDir, Directory) {
    let temp = support::private_tempdir();
    let directory = Directory::open_existing(temp.path()).unwrap();
    (temp, directory)
}

fn context(session: Uuid, interactive: bool) -> ActivityContext {
    ActivityContext {
        session,
        session_id: Some(session.to_string()),
        project_id: Some("project-a".into()),
        sandbox_id: Some("sandbox-a".into()),
        agent_id: Some("runner-agent".into()),
        correlation_id: None,
        trace_id: None,
        worker_id: Some("worker-a".into()),
        attempt_id: Some("attempt-1".into()),
        client: ActivityClient::ClaudeCode,
        interactive,
        client_version_label: Some("2.1.329".into()),
    }
}

fn collector(directory: &Directory, session: Uuid, limits: ActivityLimits) -> ActivityCollector {
    let spool = ActivitySpool::create(directory, session).unwrap();
    ActivityCollector::new(
        spool,
        context(session, false),
        Redactor::new(vec![]),
        limits,
    )
    .unwrap()
}

fn spooled(collector: &ActivityCollector) -> Vec<Event> {
    collector
        .pending(10_000, usize::MAX)
        .into_iter()
        .map(|spooled| spooled.event)
        .collect()
}

#[test]
fn stdout_records_are_spooled_with_trusted_context_and_replay_stable_ids() {
    let (_temp, directory) = directory();
    let session = Uuid::new_v4();
    let collector = collector(&directory, session, ActivityLimits::default());
    for line in CLAUDE_STREAM.lines() {
        assert_eq!(collector.stdout_line(line.as_bytes()), Admission::Recorded);
    }
    let events = spooled(&collector);
    assert_eq!(events.len(), 16);
    for event in &events {
        event.validate().unwrap();
        assert_eq!(event.provenance, "declared");
        assert_eq!(event.coverage, "native");
        assert_eq!(
            event.session_id.as_deref(),
            Some(session.to_string().as_str())
        );
        assert_eq!(event.agent_id.as_deref(), Some("runner-agent"));
        assert_eq!(event.data["activity"]["channel"], "stdout");
        assert_eq!(event.data["activity"]["worker_id"], "worker-a");
        assert_eq!(event.data["activity"]["client_version_label"], "2.1.329");
        assert_eq!(
            event.timestamp,
            event.data["activity"]["received_at"].as_str().unwrap()
        );
    }
    let summary = collector.summary().unwrap();
    let stdout = summary.stdout.unwrap();
    assert_eq!((stdout.records, stdout.mapped, stdout.ignored), (12, 11, 1));
    drop(collector);
    let (reopened, recovery) = ActivitySpool::reopen(&directory, session).unwrap();
    assert_eq!(recovery.events, 16);
    assert_eq!(recovery.torn_tail_bytes, 0);
    let again = ActivityCollector::new(
        reopened,
        context(session, false),
        Redactor::new(vec![]),
        ActivityLimits::default(),
    )
    .unwrap();
    let ids: Vec<_> = spooled(&again).into_iter().map(|e| e.id).collect();
    assert_eq!(ids, events.into_iter().map(|e| e.id).collect::<Vec<_>>());
}

#[test]
fn reopen_cuts_a_torn_tail_and_refuses_a_tampered_record() {
    let (temp, directory) = directory();
    let session = Uuid::new_v4();
    let collector = collector(&directory, session, ActivityLimits::default());
    for line in CLAUDE_STREAM.lines().take(3) {
        collector.stdout_line(line.as_bytes());
    }
    drop(collector);
    let path: PathBuf = temp
        .path()
        .join(format!("provider-activity-{session}.spool"));
    let mut bytes = std::fs::read(&path).unwrap();
    let complete = bytes.len();
    bytes.extend_from_slice(b"{\"type\":\"event\",\"v\":1,\"seq\":");
    std::fs::write(&path, &bytes).unwrap();
    let (spool, recovery) = ActivitySpool::reopen(&directory, session).unwrap();
    assert_eq!(recovery.events, 6);
    assert!(recovery.torn_tail_bytes > 0);
    assert_eq!(std::fs::read(&path).unwrap().len(), complete);
    drop(spool);
    let text = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
    std::fs::write(
        &path,
        text.replacen("Listing the workspace.", "Listing the wurkspace.", 1),
    )
    .unwrap();
    let error = ActivitySpool::reopen(&directory, session).unwrap_err();
    assert!(format!("{error:#}").contains("hash mismatch"), "{error:#}");
}

#[test]
fn a_gated_hook_is_refused_while_the_undelivered_backlog_is_over_budget() {
    let (_temp, directory) = directory();
    let limits = ActivityLimits {
        max_events: 100,
        max_bytes: 1024 * 1024,
        max_backlog_events: 2,
        max_backlog_bytes: 64 * 1024,
    };
    let collector = collector(&directory, Uuid::new_v4(), limits);
    let hooks: Vec<&str> = CLAUDE_HOOKS.lines().collect();
    // SessionStart declares a session and its root agent: two events.
    assert_eq!(collector.hook(hooks[0], false), Admission::Recorded);
    assert!(collector.audit_unavailable());
    assert_eq!(collector.hook(hooks[2], false), Admission::Unavailable);
    assert_eq!(collector.summary().unwrap().hook.refused, 1);
    let delivered = collector.pending(10, usize::MAX).last().unwrap().seq;
    collector.mark_delivered(delivered).unwrap();
    assert!(!collector.audit_unavailable());
    assert_eq!(collector.hook(hooks[2], false), Admission::Recorded);
}

#[test]
fn an_exhausted_session_budget_drops_records_with_one_explicit_loss() {
    let (_temp, directory) = directory();
    let limits = ActivityLimits {
        // The backlog bound equals the budget so that only the budget is hit.
        max_events: 4,
        max_bytes: 1024 * 1024,
        max_backlog_events: 4,
        max_backlog_bytes: 64 * 1024,
    };
    let collector = collector(&directory, Uuid::new_v4(), limits);
    let lines: Vec<&str> = CLAUDE_STREAM.lines().collect();
    assert_eq!(
        collector.stdout_line(lines[0].as_bytes()),
        Admission::Recorded
    );
    assert_eq!(
        collector.stdout_line(lines[1].as_bytes()),
        Admission::Dropped
    );
    assert_eq!(
        collector.hook(CLAUDE_HOOKS.lines().nth(2).unwrap(), false),
        Admission::Dropped
    );
    let events = spooled(&collector);
    let losses: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "activity.loss")
        .collect();
    assert_eq!(losses.len(), 1);
    assert_eq!(losses[0].provenance, "observed");
    assert_eq!(losses[0].data["reason"], "session_budget_exhausted");
    let summary = collector.finish().unwrap();
    assert!(summary.budget_exhausted && !summary.complete);
    let coverage = spooled(&collector).pop().unwrap();
    assert_eq!(coverage.kind, "activity.coverage");
    assert_eq!(coverage.data["budget_exhausted"], true);
}

#[test]
fn worker_reported_hook_losses_stay_declared() {
    let (_temp, directory) = directory();
    let collector = collector(&directory, Uuid::new_v4(), ActivityLimits::default());
    assert_eq!(
        collector.declared_hook_loss(2, "client_unrecorded"),
        Admission::Recorded
    );
    assert_eq!(
        collector.channel_state(ActivityChannel::Hook, "attached", None),
        Admission::Recorded
    );
    let events = spooled(&collector);
    assert_eq!(events[0].kind, "activity.loss");
    assert_eq!(events[0].provenance, "declared");
    assert_eq!(events[1].kind, "activity.channel");
    assert_eq!(events[1].provenance, "observed");
    assert!(!collector.summary().unwrap().complete);
}

#[tokio::test]
async fn the_stdout_pump_frames_split_chunks_and_counts_unframed_records() {
    let (_temp, directory) = directory();
    let collector = collector(&directory, Uuid::new_v4(), ActivityLimits::default());
    let (tap, chunks) = stdout_tap();
    let task = tokio::spawn(pump(collector.clone(), chunks));
    let stream = CLAUDE_STREAM.as_bytes();
    for piece in stream.chunks(7) {
        tap.send(piece).await;
    }
    let oversized = format!("{}\n", "x".repeat(1024 * 1024 + 10));
    tap.send(oversized.as_bytes()).await;
    tap.send(b"{\"type\":\"result\"").await;
    drop(tap);
    let outcome = task.await.unwrap();
    assert_eq!(outcome.lines, 12);
    assert_eq!(outcome.oversized, 1);
    assert!(outcome.unterminated_final_record);
    let stdout = collector.summary().unwrap().stdout.unwrap();
    assert_eq!(
        (stdout.mapped, stdout.oversized, stdout.invalid),
        (11, 1, 1)
    );
}

#[tokio::test]
async fn a_stalled_audit_bounds_the_stdout_stall_and_records_the_overflow() {
    let (_temp, directory) = directory();
    let limits = ActivityLimits {
        max_events: 100,
        max_bytes: 1024 * 1024,
        max_backlog_events: 2,
        max_backlog_bytes: 64 * 1024,
    };
    let session = Uuid::new_v4();
    let collector = collector(&directory, session, limits);
    let (tap, chunks) = stdout_tap_with_stall(Duration::from_millis(200));
    let task = tokio::spawn(pump(collector.clone(), chunks));
    // The init record (two events) fills the undelivered backlog: the next
    // records stall the pump, then the bounded tap overflows instead of
    // blocking the capture forever.
    let started = tokio::time::Instant::now();
    for line in CLAUDE_STREAM.lines().cycle().take(400) {
        tap.send(format!("{line}\n").as_bytes()).await;
    }
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the capture side was not bounded"
    );
    drop(tap);
    // Recover the outage so the pump can finish and account the loss.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let seq = collector.pending(10, usize::MAX).last().unwrap().seq;
    collector.mark_delivered(seq).unwrap();
    let mut outcome = None;
    for _ in 0..2400 {
        if task.is_finished() {
            outcome = Some(task.await.unwrap());
            break;
        }
        let pending = collector.pending(1000, usize::MAX);
        if let Some(last) = pending.last() {
            collector.mark_delivered(last.seq).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let outcome = outcome.expect("pump finished after recovery");
    assert!(outcome.stalled);
    assert!(outcome.overflow_bytes > 0);
    assert!(collector.summary().unwrap().stalled);
    // Delivered and undelivered events alike, as a recovery run reads them.
    let (spool, _) = ActivitySpool::reopen(&directory, session).unwrap();
    let events: Vec<Event> = spool.events().iter().map(|e| e.event.clone()).collect();
    assert!(
        events
            .iter()
            .any(|e| e.kind == "activity.loss" && e.data["reason"] == "stdout_stall_limit_exceeded")
    );
    assert!(
        events
            .iter()
            .any(|e| e.kind == "activity.channel" && e.data["state"] == "stalled")
    );
}

#[derive(Default)]
struct MemorySink {
    failures: AtomicUsize,
    delivered: Mutex<Vec<Event>>,
}

impl ActivitySink for MemorySink {
    fn deliver(&self, events: &[Event]) -> impl Future<Output = anyhow::Result<()>> + Send {
        let fail = self
            .failures
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
        if !fail {
            self.delivered
                .lock()
                .unwrap()
                .extend(events.iter().cloned());
        }
        async move {
            if fail {
                anyhow::bail!("synthetic gateway outage")
            }
            Ok(())
        }
    }
}

#[tokio::test]
async fn the_forwarder_retries_without_loss_and_resumes_after_reopen() {
    let (_temp, directory) = directory();
    let session = Uuid::new_v4();
    let collector = collector(&directory, session, ActivityLimits::default());
    for line in CLAUDE_STREAM.lines() {
        collector.stdout_line(line.as_bytes());
    }
    let sink = MemorySink {
        failures: AtomicUsize::new(2),
        ..Default::default()
    };
    let (stop, stopped) = tokio::sync::watch::channel(true);
    let outcome = forward(collector.clone(), &sink, stopped, Duration::from_secs(30)).await;
    drop(stop);
    assert_eq!(outcome.failed_attempts, 2);
    assert_eq!(outcome.undelivered, 0);
    assert_eq!(outcome.delivered_through, 16);
    assert_eq!(sink.delivered.lock().unwrap().len(), 16);
    for line in CLAUDE_HOOKS.lines().take(2) {
        collector.hook(line, false);
    }
    drop(collector);
    let (spool, recovery) = ActivitySpool::reopen(&directory, session).unwrap();
    assert_eq!((recovery.events, recovery.delivered_through), (19, 16));
    let resumed = ActivityCollector::new(
        spool,
        context(session, false),
        Redactor::new(vec![]),
        ActivityLimits::default(),
    )
    .unwrap();
    assert_eq!(resumed.pending(100, usize::MAX).len(), 3);
}

fn gateway() -> (tempfile::TempDir, Arc<Store>, Router) {
    let directory = support::private_tempdir();
    let path = directory.path().join("tenant.sqlite");
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    store
        .migrate_control_v3(&path.with_extension("v3.backup"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("v4.backup"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("v5.backup"))
        .unwrap();
    store
        .migrate_workload_v6(&path.with_extension("v6.backup"))
        .unwrap();
    let store = Arc::new(store);
    let router = gateway_router(GatewayState::new(
        store.clone(),
        RUNNER.into(),
        VIEWER.into(),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    ));
    (directory, store, router)
}

async fn post_batch(router: &Router, token: &str, body: &Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/events/batch")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn ordinary(kind: &str) -> Event {
    let mut event = Event::new(kind);
    event.source = "claude-adapter".into();
    event.provenance = "declared".into();
    event.coverage = "native".into();
    event.session_id = Some("s".into());
    event
}

#[tokio::test]
async fn the_batch_route_is_atomic_idempotent_and_runner_only() {
    let (_temp, store, router) = gateway();
    // Migrations append their genesis control events first.
    let base = store.head().unwrap();
    let events: Vec<Event> = (0..3).map(|_| ordinary("message.declared")).collect();
    let body = json!({"events": events});
    assert_ne!(post_batch(&router, VIEWER, &body).await.0, StatusCode::OK);
    assert_eq!(store.head().unwrap(), base);
    let (status, first) = post_batch(&router, RUNNER, &body).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["stored"].as_array().unwrap().len(), 3);
    let (status, replay) = post_batch(&router, RUNNER, &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, first, "an identical replay returns the stored rows");
    assert_eq!(store.head().unwrap(), base + 3);
    let mut divergent = events.clone();
    divergent[1].data = json!({"text":"changed"});
    divergent.push(ordinary("message.declared"));
    let (status, _) = post_batch(&router, RUNNER, &json!({"events": divergent})).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        store.head().unwrap(),
        base + 3,
        "a conflicting batch appends nothing"
    );
    let mixed = vec![
        ordinary("message.declared"),
        ordinary("control.relay.forged"),
    ];
    let (status, _) = post_batch(&router, RUNNER, &json!({"events": mixed})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let too_many: Vec<Event> = (0..65).map(|_| ordinary("tool.requested")).collect();
    let (status, _) = post_batch(&router, RUNNER, &json!({"events": too_many})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(store.head().unwrap(), base + 3);
}

struct RouterSink(Router);

impl ActivitySink for RouterSink {
    fn deliver(&self, events: &[Event]) -> impl Future<Output = anyhow::Result<()>> + Send {
        let router = self.0.clone();
        let body = json!({"events": events});
        async move {
            let (status, _) = post_batch(&router, RUNNER, &body).await;
            anyhow::ensure!(status == StatusCode::OK, "batch refused: {status}");
            Ok(())
        }
    }
}

#[tokio::test]
async fn hook_activity_reaches_the_gateway_while_the_session_is_still_running() {
    let (_temp, store, router) = gateway();
    let base = store.head().unwrap();
    let (_spool_temp, directory) = directory();
    let collector = collector(&directory, Uuid::new_v4(), ActivityLimits::default());
    let sink = RouterSink(router.clone());
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let forwarder = {
        let collector = collector.clone();
        tokio::spawn(
            async move { forward(collector, &sink, stopped, Duration::from_secs(30)).await },
        )
    };
    assert_eq!(
        collector.hook(CLAUDE_HOOKS.lines().nth(2).unwrap(), false),
        Admission::Recorded
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while store.head().unwrap() == base {
        assert!(tokio::time::Instant::now() < deadline, "no live delivery");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // The session has not finished: the forwarder is still running.
    assert!(!forwarder.is_finished());
    let request = Request::builder()
        .uri("/api/events?limit=10")
        .header("Authorization", format!("Bearer {VIEWER}"))
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let text = body.to_string();
    assert!(text.contains("tool.requested"), "{text}");
    assert!(text.contains("toolu_01"), "{text}");
    stop.send(true).unwrap();
    let outcome = forwarder.await.unwrap();
    assert_eq!(outcome.undelivered, 0);
}

struct ScriptedHooks {
    frames: std::collections::VecDeque<agentsandbox::provider_activity::HookFrame>,
    acks: Arc<Mutex<Vec<(u64, agentsandbox::provider_activity::HookAck)>>>,
}

impl agentsandbox::provider_activity::HookTransport for ScriptedHooks {
    fn next(
        &mut self,
    ) -> impl Future<Output = anyhow::Result<Option<agentsandbox::provider_activity::HookFrame>>> + Send
    {
        let frame = self.frames.pop_front();
        async move { Ok(frame) }
    }
    fn ack(
        &mut self,
        seq: u64,
        ack: agentsandbox::provider_activity::HookAck,
    ) -> impl Future<Output = anyhow::Result<()>> + Send {
        self.acks.lock().unwrap().push((seq, ack));
        async { Ok(()) }
    }
}

#[tokio::test]
async fn the_hook_pump_acknowledges_only_durable_records_and_reports_anomalies() {
    use agentsandbox::provider_activity::{HookAck, HookFrame, pump_hooks};
    let (_temp, directory) = directory();
    let limits = ActivityLimits {
        max_events: 100,
        max_bytes: 1024 * 1024,
        max_backlog_events: 5,
        max_backlog_bytes: 64 * 1024,
    };
    let collector = collector(&directory, Uuid::new_v4(), limits);
    let hooks: Vec<&str> = CLAUDE_HOOKS.lines().collect();
    let acks = Arc::new(Mutex::new(Vec::new()));
    let transport = ScriptedHooks {
        frames: [
            HookFrame::Hook {
                seq: 1,
                payload: hooks[2].into(),
                truncated: false,
                gate: true,
            },
            // A gated payload the adapter cannot map leaves no audit record:
            // the action must be refused even though nothing failed.
            HookFrame::Hook {
                seq: 2,
                payload: r#"{"session_id":"s"}"#.into(),
                truncated: false,
                gate: true,
            },
            HookFrame::Loss {
                seq: 3,
                count: 1,
                reason: "client_unrecorded".into(),
            },
            // Seq 4 is missing: the helper's numbering is not trusted.
            HookFrame::Hook {
                seq: 5,
                payload: hooks[3].into(),
                truncated: true,
                gate: false,
            },
            // The backlog is now full: a gated hook is refused, not dropped silently.
            HookFrame::Hook {
                seq: 6,
                payload: hooks[2].into(),
                truncated: false,
                gate: true,
            },
        ]
        .into(),
        acks: acks.clone(),
    };
    let outcome = pump_hooks(collector.clone(), transport).await;
    assert_eq!(outcome.hooks, 4);
    assert_eq!((outcome.recorded, outcome.refused), (2, 2));
    assert_eq!(outcome.unrecorded_gates, 1);
    assert_eq!(outcome.declared_losses, 1);
    assert_eq!(outcome.sequence_gaps, 1);
    assert!(!outcome.transport_error);
    assert_eq!(
        *acks.lock().unwrap(),
        [
            (1, HookAck::Recorded),
            (2, HookAck::Refused),
            (5, HookAck::Recorded),
            (6, HookAck::Refused)
        ]
    );
    let events = spooled(&collector);
    let truncated = events
        .iter()
        .find(|e| e.kind == "tool.result")
        .expect("PostToolUse spooled");
    assert_eq!(truncated.data["hook_input_truncated"], true);
    assert_eq!(truncated.data["activity"]["channel"], "hook");
    assert!(
        events
            .iter()
            .any(|e| e.kind == "activity.channel" && e.data["state"] == "sequence_gap")
    );
    let summary = collector.summary().unwrap();
    assert_eq!(summary.hook_channel, "closed");
    assert_eq!(summary.hook.refused, 1);
    assert_eq!(summary.hook.unrecognized, 1);
    assert!(!summary.complete);
}
