//! Controlled futures exercise only Full/signal ownership and joins. The real
//! IPC/HTTP/SQLite/RoleLease boundary is covered by preclaim_shutdown_tests.
use super::*;
use std::{
    future::{Future, poll_fn},
    pin::Pin,
    task::Poll,
};

struct BlockedOwner {
    entered: oneshot::Receiver<()>,
    release: Option<oneshot::Sender<()>>,
    completed: oneshot::Receiver<()>,
    completion_observed: Option<bool>,
}
impl BlockedOwner {
    fn start(slot: FullOwnerSlot) -> Result<Self, retained_audit::OwnerError> {
        let (started, entered) = oneshot::channel();
        let (release, blocked) = oneshot::channel();
        let (finished, completed) = oneshot::channel();
        slot.start(async move {
            let _ = started.send(());
            // The test owns this actual barrier. A dropped test owner releases
            // it too; no sleep or scheduler yield stands in for task entry.
            let _ = blocked.await;
            let _ = finished.send(());
            Ok(())
        })?;
        Ok(Self {
            entered,
            release: Some(release),
            completed,
            completion_observed: None,
        })
    }

    async fn entered(&mut self) -> bool {
        (&mut self.entered).await.is_ok()
    }

    fn still_blocked(&mut self) -> bool {
        match self.completed.try_recv() {
            Err(oneshot::error::TryRecvError::Empty) => true,
            result => {
                self.completion_observed = Some(result.is_ok());
                false
            }
        }
    }

    fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }

    async fn completed(&mut self) -> bool {
        match self.completion_observed {
            Some(completed) => completed,
            None => (&mut self.completed).await.is_ok(),
        }
    }
}
impl Drop for BlockedOwner {
    fn drop(&mut self) {
        self.release();
    }
}

async fn poll_once<F: Future>(future: &mut Pin<Box<F>>) -> Poll<F::Output> {
    poll_fn(|context| Poll::Ready(future.as_mut().poll(context))).await
}

#[tokio::test(flavor = "current_thread")]
async fn saturated_run_pool_still_admits_and_joins_its_independent_signal_owner() {
    let owners = FullOwners::new(1).unwrap();
    let mut run = BlockedOwner::start(owners.try_reserve().unwrap()).unwrap();
    let run_entered = run.entered().await;
    let run_saturated = matches!(owners.try_reserve(), Err(retained_audit::OwnerError::Busy));
    let mut signal = owners
        .try_reserve_signal()
        .and_then(BlockedOwner::start)
        .ok();
    let signal_entered = match signal.as_mut() {
        Some(signal) => signal.entered().await,
        None => false,
    };
    let signal_saturated = matches!(
        owners.try_reserve_signal(),
        Err(retained_audit::OwnerError::Busy)
    );
    let close = owners.close();
    let run_closed = matches!(
        owners.try_reserve(),
        Err(retained_audit::OwnerError::Closed)
    );
    let signal_closed = matches!(
        owners.try_reserve_signal(),
        Err(retained_audit::OwnerError::Closed)
    );
    let mut drain = Box::pin(owners.drain());
    let first = poll_once(&mut drain).await;
    let drain_pending = first.is_pending();
    let run_blocked = run.still_blocked();
    let signal_blocked = signal.as_mut().is_some_and(BlockedOwner::still_blocked);

    // Release and join every accepted future before inspecting any oracle.
    run.release();
    if let Some(signal) = signal.as_mut() {
        signal.release();
    }
    let aggregate = match first {
        Poll::Ready(result) => result,
        Poll::Pending => drain.as_mut().await,
    };
    drop(drain);
    // Separate joins also clean up an implementation that prematurely returned
    // from the aggregate drain, rather than relying on that result for cleanup.
    let run_join = owners.runs.drain().await;
    let signal_join = owners.signals.drain().await;
    let run_completed = run.completed().await;
    let signal_completed = match signal.as_mut() {
        Some(signal) => signal.completed().await,
        None => false,
    };

    assert!(run_entered && run_saturated && signal_entered && signal_saturated);
    assert!(close.is_ok() && run_closed && signal_closed);
    assert!(drain_pending && run_blocked && signal_blocked);
    assert!(aggregate.is_ok() && run_join.is_ok() && signal_join.is_ok());
    assert!(run_completed && signal_completed && !owners.has_failure());
}

async fn failed_run_still_joins_a_blocked_signal(panic_run: bool) {
    let owners = FullOwners::new(1).unwrap();
    let run = owners.try_reserve().unwrap();
    let signal = owners.try_reserve_signal().unwrap();
    let mut signal = BlockedOwner::start(signal).unwrap();
    let started_run = run.start(async move {
        if panic_run {
            panic!("controlled Full run panic before signal drain");
        }
        Err(BrokerErrorCodeV1::NeedsReconciliation)
    });
    let signal_entered = signal.entered().await;
    // This is a real join, not a completion flag: the first registry has
    // already returned the task's error/panic before the combined drain polls.
    let failed_run_join = owners.runs.drain().await;
    let close = owners.close();
    let mut drain = Box::pin(owners.drain());
    let first = poll_once(&mut drain).await;
    let pending_before_release = first.is_pending();
    let failure_before_release = owners.has_failure();
    let signal_blocked = signal.still_blocked();

    signal.release();
    let aggregate = match first {
        Poll::Ready(result) => result,
        Poll::Pending => drain.as_mut().await,
    };
    drop(drain);
    let run_join = owners.runs.drain().await;
    let signal_join = owners.signals.drain().await;
    let signal_completed = signal.completed().await;
    let failure_after_joins = owners.has_failure();
    let repeated = owners.drain().await;

    // Every barrier and handle above is released/joined even when the
    // aggregate incorrectly short-circuits on the first registry's failure.
    assert!(started_run.is_ok() && signal_entered && close.is_ok());
    assert_eq!(
        failed_run_join,
        Err(retained_audit::DrainError::OwnerFailed)
    );
    assert!(pending_before_release && signal_blocked);
    assert!(failure_before_release && failure_after_joins);
    assert_eq!(aggregate, Err(retained_audit::DrainError::OwnerFailed));
    assert_eq!(run_join, Err(retained_audit::DrainError::OwnerFailed));
    assert!(signal_join.is_ok() && signal_completed);
    assert_eq!(repeated, Err(retained_audit::DrainError::OwnerFailed));
    assert!(owners.has_failure());
}

#[tokio::test(flavor = "current_thread")]
async fn failed_run_drain_waits_for_blocked_signal_and_keeps_failure_after_both_joins() {
    failed_run_still_joins_a_blocked_signal(false).await;
}

#[tokio::test(flavor = "current_thread")]
async fn panicked_run_drain_waits_for_blocked_signal_and_keeps_failure_after_both_joins() {
    failed_run_still_joins_a_blocked_signal(true).await;
}
