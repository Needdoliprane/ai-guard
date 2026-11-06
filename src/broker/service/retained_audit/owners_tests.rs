use super::{BrokerErrorCodeV1, ColdDrainError, ColdOwnerError, ColdOwners, RetainedAuditSuccess};
use std::{
    future::{Future, poll_fn},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::Poll,
};
use tokio::sync::Notify;

struct NotifyOnDrop(Arc<Notify>);

impl Drop for NotifyOnDrop {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

async fn poll_once<F: Future>(future: &mut Pin<Box<F>>) -> Poll<F::Output> {
    poll_fn(|context| Poll::Ready(future.as_mut().poll(context))).await
}

#[tokio::test(flavor = "current_thread")]
async fn cold_owners_enforce_exact_capacity_and_reap_finished_slots_before_reuse() {
    assert!(matches!(
        ColdOwners::new(0),
        Err(ColdOwnerError::InvalidCapacity)
    ));
    assert!(matches!(
        ColdOwners::new(1025),
        Err(ColdOwnerError::InvalidCapacity)
    ));

    let owners = ColdOwners::new(2).expect("capacity two is valid");
    let first = owners.try_reserve().expect("first slot");
    let second = owners.try_reserve().expect("second slot");
    assert!(matches!(owners.try_reserve(), Err(ColdOwnerError::Busy)));

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let finished = Arc::new(Notify::new());
    let task_started = Arc::clone(&started);
    let task_release = Arc::clone(&release);
    let task_finished = Arc::clone(&finished);
    first
        .start(async move {
            task_started.notify_one();
            task_release.notified().await;
            let _finished = NotifyOnDrop(task_finished);
            Ok(RetainedAuditSuccess::Complete)
        })
        .expect("reserved slot starts once");
    started.notified().await;
    assert!(matches!(owners.try_reserve(), Err(ColdOwnerError::Busy)));

    release.notify_one();
    finished.notified().await;
    // try_reserve performs the actual join/reap before reusing a completed slot.
    let third = owners
        .try_reserve()
        .expect("completed owner was reaped and its slot reused");
    assert!(matches!(owners.try_reserve(), Err(ColdOwnerError::Busy)));

    drop(second);
    drop(third);
    owners
        .close()
        .expect("close after reservations are dropped");
    owners.drain().await.expect("no live owner remains");
    assert!(matches!(owners.try_reserve(), Err(ColdOwnerError::Closed)));
}

#[tokio::test(flavor = "current_thread")]
async fn cold_owners_close_atomically_and_drain_ignores_reserved_late_starts() {
    let owners = ColdOwners::new(2).expect("capacity two is valid");
    let running = owners.try_reserve().expect("running slot");
    let reserved = owners.try_reserve().expect("late reserved slot");
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let task_started = Arc::clone(&started);
    let task_release = Arc::clone(&release);
    running
        .start(async move {
            task_started.notify_one();
            task_release.notified().await;
            Ok(RetainedAuditSuccess::PartialConfirmed)
        })
        .expect("pre-close owner starts");
    started.notified().await;

    owners.close().expect("close registration");
    assert!(matches!(owners.try_reserve(), Err(ColdOwnerError::Closed)));

    let mut drain = Box::pin(owners.drain());
    assert!(matches!(poll_once(&mut drain).await, Poll::Pending));
    release.notify_one();
    drain
        .await
        .expect("drain joins the owner accepted before close");

    let polls = Arc::new(AtomicUsize::new(0));
    let task_polls = Arc::clone(&polls);
    assert!(matches!(
        reserved.start(async move {
            task_polls.fetch_add(1, Ordering::SeqCst);
            Ok(RetainedAuditSuccess::Complete)
        }),
        Err(ColdOwnerError::Closed)
    ));
    assert_eq!(
        polls.load(Ordering::SeqCst),
        0,
        "late future was never polled"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cold_owners_retain_failure_after_reap_and_cancelled_drain_joins_every_owner() {
    let owners = ColdOwners::new(2).expect("capacity two is valid");
    let failed = owners.try_reserve().expect("failure slot");
    let held = owners.try_reserve().expect("held slot");

    let failure_finished = Arc::new(Notify::new());
    let task_failure_finished = Arc::clone(&failure_finished);
    failed
        .start(async move {
            let _completed = NotifyOnDrop(task_failure_finished);
            Err(BrokerErrorCodeV1::Unavailable)
        })
        .expect("failure owner starts");

    let held_started = Arc::new(Notify::new());
    let held_release = Arc::new(Notify::new());
    let held_completed = Arc::new(AtomicBool::new(false));
    let task_held_started = Arc::clone(&held_started);
    let task_held_release = Arc::clone(&held_release);
    let task_held_completed = Arc::clone(&held_completed);
    held.start(async move {
        task_held_started.notify_one();
        task_held_release.notified().await;
        task_held_completed.store(true, Ordering::SeqCst);
        Ok(RetainedAuditSuccess::PartialConfirmed)
    })
    .expect("second owner starts");
    held_started.notified().await;

    // The first task's future has completed. Reuse causes its JoinHandle to be
    // reaped, and the failure must survive that slot reuse.
    failure_finished.notified().await;
    let reused = owners
        .try_reserve()
        .expect("finished failed owner is reaped before slot reuse");
    drop(reused);
    owners.close().expect("close after both starts");

    let mut abandoned_drain = Box::pin(owners.drain());
    assert!(matches!(
        poll_once(&mut abandoned_drain).await,
        Poll::Pending
    ));
    drop(abandoned_drain);

    let mut replacement_drain = Box::pin(owners.drain());
    assert!(matches!(
        poll_once(&mut replacement_drain).await,
        Poll::Pending
    ));
    held_release.notify_one();
    assert!(matches!(
        replacement_drain.await,
        Err(ColdDrainError::OwnerFailed)
    ));
    assert!(held_completed.load(Ordering::SeqCst));
    assert!(matches!(
        owners.drain().await,
        Err(ColdDrainError::OwnerFailed)
    ));
}
