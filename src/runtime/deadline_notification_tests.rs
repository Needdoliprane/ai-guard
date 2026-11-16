//! Notification ownership only. RuntimeState is ordinary unit DATA here; no
//! native observation, authority, audit admission or terminal proof is created.
use super::{
    RuntimeState,
    deadline_supervision::{Notification, NotificationState},
};
use std::{
    cell::Cell,
    future::{Future, poll_fn, ready},
    pin::Pin,
    task::Poll,
};
use tokio::sync::oneshot;

fn unit_state() -> RuntimeState {
    RuntimeState {
        status: "exited".into(),
        running: false,
        started_at: "2026-01-01T00:00:00Z".into(),
        finished_at: "2026-01-01T00:00:01Z".into(),
        exit_code: 0,
        oom_killed: false,
    }
}

#[tokio::test]
async fn failed_started_notification_cannot_be_reissued_by_final_reconciliation() {
    let notification = Notification::new();
    let calls = Cell::new(0);
    let mut callback = |_| {
        calls.set(calls.get() + 1);
        ready(Err(anyhow::anyhow!(
            "UNIT_POSSIBLY_COMMITTED_CALLBACK_ERROR"
        )))
    };
    let first = notification.notify(unit_state(), &mut callback).await;
    let state_after_first = notification.get();
    let second = notification.notify(unit_state(), &mut callback).await;
    assert!(first.is_err() && second.is_err());
    assert_eq!(state_after_first, NotificationState::Failed);
    assert_eq!(notification.get(), NotificationState::Failed);
    assert_eq!(calls.get(), 1);
}

async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
}

#[tokio::test]
async fn engaged_notification_keeps_the_same_local_callback_until_its_result() {
    let notification = Notification::new();
    let calls = Cell::new(0);
    let done = Cell::new(false);
    let (release, released) = oneshot::channel::<()>();
    let mut released = Some(released);
    let done_ref = &done;
    let calls_ref = &calls;
    let mut callback = move |_| {
        calls_ref.set(calls_ref.get() + 1);
        let receiver = released.take();
        let done = done_ref;
        async move {
            let receiver = receiver.ok_or_else(|| anyhow::anyhow!("UNIT_CALLBACK_REISSUED"))?;
            receiver
                .await
                .map_err(|_| anyhow::anyhow!("UNIT_RELEASE_LOST"))?;
            done.set(true);
            Ok(())
        }
    };
    let mut notifying = Box::pin(notification.notify(unit_state(), &mut callback));
    let first = poll_once(notifying.as_mut()).await;
    let was_pending = matches!(&first, Poll::Pending);
    let engaged = notification.get();
    let completed_too_early = done.get();
    let released = release.send(()).is_ok();
    let result = match first {
        Poll::Ready(result) => result,
        Poll::Pending => notifying.as_mut().await,
    };
    drop(notifying);
    // All futures finish before assertions; no cancelled effect is called done.
    assert!(was_pending && released && !completed_too_early);
    assert_eq!(engaged, NotificationState::Engaged);
    assert!(result.is_ok() && done.get());
    assert_eq!(notification.get(), NotificationState::Succeeded);
    assert_eq!(calls.get(), 1);
}
