use super::*;
use crate::{
    model::Event,
    provider_activity::spool::{event_id, spool_name},
};
use std::os::unix::fs::PermissionsExt;
use tokio::sync::oneshot;

struct SignalDrop(Option<oneshot::Sender<()>>);

impl Drop for SignalDrop {
    fn drop(&mut self) {
        if let Some(signal) = self.0.take() {
            let _ = signal.send(());
        }
    }
}

#[tokio::test]
async fn timed_out_producer_is_cancelled_and_joined_before_drain_returns() {
    let (started, running) = oneshot::channel();
    let (dropped, mut stopped) = oneshot::channel();
    let task = SessionTask::new(tokio::spawn(async move {
        let _drop = SignalDrop(Some(dropped));
        let _ = started.send(());
        std::future::pending::<()>().await;
    }));
    running.await.unwrap();

    let result = task
        .drain(Duration::from_millis(60), Duration::from_millis(30))
        .await;

    assert_eq!(result, Err(DrainError::Deadline));
    assert_eq!(
        stopped.try_recv(),
        Ok(()),
        "drain must join the cancelled producer"
    );
}

#[tokio::test]
async fn failed_joins_do_not_become_clean_drains() {
    let budget = Duration::from_secs(1);
    let reserve = Duration::from_millis(100);
    let panicked = SessionTask::new(tokio::spawn(async { panic!("synthetic task failure") }));
    assert_eq!(
        panicked.drain(budget, reserve).await,
        Err(DrainError::Panicked)
    );

    let cancelled = tokio::spawn(std::future::pending::<()>());
    cancelled.abort();
    assert_eq!(
        SessionTask::new(cancelled).drain(budget, reserve).await,
        Err(DrainError::Cancelled)
    );
    assert_eq!(
        SessionTask::new(tokio::spawn(async { 7 }))
            .drain(budget, reserve)
            .await,
        Ok(7)
    );
}

#[tokio::test]
async fn cancelling_the_drain_requests_cancellation_of_its_task() {
    let (dropped, stopped) = oneshot::channel();
    let guard = SignalDrop(Some(dropped));
    let task = SessionTask::new(tokio::spawn(async move {
        let _drop = guard;
        std::future::pending::<()>().await;
    }));
    let mut drain = Box::pin(task.drain(Duration::from_secs(10), Duration::from_secs(1)));
    use std::{future::Future, task::Poll};
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(drain.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    drop(drain);
    timeout_at(Instant::now() + Duration::from_secs(1), stopped)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn an_unjoined_task_leaves_closure_unconfirmed() {
    let (started, running) = oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let task = SessionTask::new(tokio::task::spawn_blocking(move || {
        let _ = started.send(());
        let _ = blocked.recv();
    }));
    running.await.unwrap();

    let result = task
        .drain(Duration::from_millis(60), Duration::from_millis(30))
        .await;
    release.send(()).unwrap();

    assert_eq!(result, Err(DrainError::Unconfirmed));
}

#[derive(Default)]
struct RecordedActivity(Mutex<Vec<Event>>);

impl ActivitySink for RecordedActivity {
    async fn deliver(&self, events: &[Event]) -> Result<()> {
        self.0.lock().unwrap().extend_from_slice(events);
        Ok(())
    }
}

fn retained_fixture() -> (tempfile::TempDir, Directory, ActivityContext) {
    let temporary = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let canonical = crate::control_fs::absolute_path(temporary.path()).unwrap();
    let root = Directory::open_existing(&canonical).unwrap();
    let session = uuid::Uuid::new_v4();
    let context = ActivityContext {
        session,
        session_id: Some(session.to_string()),
        project_id: None,
        sandbox_id: None,
        agent_id: None,
        correlation_id: None,
        trace_id: None,
        worker_id: None,
        attempt_id: None,
        client: ActivityClient::ClaudeCode,
        interactive: false,
        client_version_label: None,
    };
    (temporary, root, context)
}

#[tokio::test]
async fn an_interrupted_session_leaves_coverage_open_for_terminal_replay() {
    let (_temporary, root, context) = retained_fixture();
    let session_id = context.session;
    let mut spool = ActivitySpool::create(&root, session_id).unwrap();
    spool.append(vec![Event::new("activity.channel")]).unwrap();
    let collector = ActivityCollector::new(
        spool,
        context.clone(),
        Redactor::new(Vec::new()),
        ActivityLimits::default(),
    )
    .unwrap();
    let (stop, _) = watch::channel(false);
    let (hook_stop, _) = watch::channel(false);
    let session = LiveSession {
        collector,
        tap: None,
        pump: None,
        forwarder: None,
        stop,
        hook_task: Arc::new(Mutex::new(HookTask::Idle)),
        hook_stop,
    };
    session.finish(false).await.unwrap();

    let (spool, _) = ActivitySpool::reopen(&root, session_id).unwrap();
    assert_eq!(spool.len(), 1, "interruption must keep the prefix open");
    drop(spool);

    let sink = RecordedActivity::default();
    LiveSession::replay_retained(
        &root,
        context,
        Redactor::new(Vec::new()),
        &sink,
        ActivityLimits::default(),
    )
    .await
    .unwrap();
    let events = sink.0.lock().unwrap();
    let [_, event] = events.as_slice() else {
        panic!("terminal replay must record one coverage fact after the prefix");
    };
    assert_eq!(event.kind, "activity.coverage");
    assert_eq!(event.provenance, "observed");
    assert_eq!(event.coverage, "partial");
    assert_eq!(event.data["complete"], false);
    assert_eq!(event.data["reason"], "activity_finalization_unconfirmed");
    for field in ["stdout", "hook", "spooled_events", "delivered_events"] {
        assert!(event.data.get(field).is_none(), "{field} is unknown");
    }
}

#[tokio::test]
async fn replay_retains_coverage_event_and_only_advances_its_delivery_cursor() {
    let (temporary, root, context) = retained_fixture();
    let session = context.session;
    let mut spool = ActivitySpool::create(&root, session).unwrap();
    let mut coverage = Event::new("activity.coverage");
    coverage.data = serde_json::json!({"complete": false, "reason": "closure_unknown"});
    spool.append(vec![coverage]).unwrap();
    let expected = serde_json::to_vec(&spool.pending(1, usize::MAX)[0].event).unwrap();
    drop(spool);
    let path = temporary.path().join(spool_name(session));
    let before = std::fs::read(&path).unwrap();

    let destination = RecordedActivity::default();
    for _ in 0..2 {
        LiveSession::replay_retained(
            &root,
            context.clone(),
            Redactor::new(Vec::new()),
            &destination,
            ActivityLimits::default(),
        )
        .await
        .unwrap();
    }
    let recorded = destination.0.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(serde_json::to_vec(&recorded[0]).unwrap(), expected);
    assert!(std::fs::read(path).unwrap().starts_with(&before));
    let (spool, _) = ActivitySpool::reopen(&root, session).unwrap();
    assert_eq!(spool.len(), 1);
    assert_eq!(spool.delivered_through(), 1);
}

#[tokio::test]
async fn replay_records_one_unknown_coverage_after_recovering_the_retained_chain() {
    use std::io::Write;

    let (temporary, root, context) = retained_fixture();
    let session = context.session;
    let path = temporary.path().join(spool_name(session));
    let mut spool = ActivitySpool::create(&root, session).unwrap();
    spool.append(vec![Event::new("activity.channel")]).unwrap();
    drop(spool);
    let prefix = std::fs::read(&path).unwrap();
    let mut writer = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writer.write_all(b"{\"interrupted\":").unwrap();
    writer.sync_data().unwrap();
    drop(writer);

    let destination = RecordedActivity::default();
    LiveSession::replay_retained(
        &root,
        context.clone(),
        Redactor::new(Vec::new()),
        &destination,
        ActivityLimits::default(),
    )
    .await
    .unwrap();
    let first_replay = std::fs::read(&path).unwrap();
    LiveSession::replay_retained(
        &root,
        context,
        Redactor::new(Vec::new()),
        &destination,
        ActivityLimits::default(),
    )
    .await
    .unwrap();

    let recorded = destination.0.lock().unwrap();
    assert_eq!(
        recorded.len(),
        2,
        "replay must retain exactly one unknown coverage"
    );
    let coverage = &recorded[1];
    coverage.validate().unwrap();
    assert_eq!(coverage.id, event_id(session, 2));
    assert_eq!(coverage.kind, "activity.coverage");
    assert_eq!(coverage.source, "runner");
    assert_eq!(coverage.provenance, "observed");
    assert_eq!(coverage.coverage, "partial");
    assert_eq!(coverage.data["complete"], false);
    assert_eq!(coverage.data["reason"], "activity_finalization_unconfirmed");
    for count in [
        "stdout",
        "hook",
        "spooled_events",
        "spooled_bytes",
        "delivered_events",
    ] {
        assert!(coverage.data.get(count).is_none(), "{count} is unknown");
    }
    assert!(first_replay.starts_with(&prefix));
    assert_eq!(std::fs::read(&path).unwrap(), first_replay);
    let (spool, recovery) = ActivitySpool::reopen(&root, session).unwrap();
    assert_eq!(
        (
            recovery.events,
            recovery.delivered_through,
            recovery.torn_tail_bytes
        ),
        (2, 2, 0)
    );
    assert_eq!(
        serde_json::to_vec(&spool.events()[1].event).unwrap(),
        serde_json::to_vec(coverage).unwrap()
    );
}

#[tokio::test]
async fn replay_refuses_absent_busy_and_invalid_spools_without_inventing_coverage() {
    use std::io::{ErrorKind, Write};

    let (temporary, root, context) = retained_fixture();
    let path = temporary.path().join(spool_name(context.session));
    let destination = RecordedActivity::default();
    let replay = || {
        LiveSession::replay_retained(
            &root,
            context.clone(),
            Redactor::new(Vec::new()),
            &destination,
            ActivityLimits::default(),
        )
    };

    assert!(replay().await.is_err());
    assert!(!path.exists());

    let mut spool = ActivitySpool::create(&root, context.session).unwrap();
    spool.append(vec![Event::new("activity.channel")]).unwrap();
    let before = std::fs::read(&path).unwrap();
    let busy = replay().await.unwrap_err();
    assert_eq!(
        busy.downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind),
        Some(ErrorKind::WouldBlock)
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    drop(spool);

    let mut writer = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writer.write_all(b"{}\n").unwrap();
    writer.sync_data().unwrap();
    drop(writer);
    let corrupt = std::fs::read(&path).unwrap();
    assert!(replay().await.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), corrupt);
    assert!(destination.0.lock().unwrap().is_empty());
}
