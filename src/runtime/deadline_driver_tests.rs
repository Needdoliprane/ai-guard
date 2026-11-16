//! Real Supervisor + local callback + owned software Child IO.
//!
//! No engine, Store, audit grant or native-stop proof. NativeStop below is an
//! explicit negative UNIT result. The required Instant is an inert far-future
//! parameter; observations use manual polls/causal barriers, never elapsed time.
//! Source only / NOT_RUN here. Root caller and A06 Store coverage remain open.
use super::{
    batch_io::OwnedBatchIo,
    deadline_supervision::{Cause, CauseLatch, NativeStop, Supervisor, checked_blocking},
};
use std::{
    cell::Cell,
    future::{Future, poll_fn},
    pin::Pin,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    task::Poll,
    time::Duration,
};
use tokio::{sync::oneshot, time::Instant};

async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
}

struct CallbackGuard<'a>(&'a Cell<bool>);
impl Drop for CallbackGuard<'_> {
    fn drop(&mut self) {
        self.0.set(true);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn deadline_progresses_stop_during_same_callback_and_preserves_late_causes() {
    let child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .env_clear()
        .env("AI_GUARD_DEADLINE_DRIVER_FIXTURE17", "park-after-stdin")
        .args([
            "--exact",
            "runtime::deadline_driver_tests::driver_child_fixture17",
            "--ignored",
            "--nocapture",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let io = OwnedBatchIo::own(child, None, 4096);
    let cause = CauseLatch::default();
    let trigger = cause.clone();
    let stop_calls = Cell::new(0);
    let stop = async {
        stop_calls.set(stop_calls.get() + 1);
        NativeStop {
            observed_stopped: false,
            evidence: None,
            errors: vec![anyhow::anyhow!("UNIT_NOT_NATIVE_STOP")],
            moved_outer_fallback: false,
        }
    };
    let mut supervisor =
        Supervisor::new(io, Instant::now() + Duration::from_secs(3600), cause, stop);
    let callback_calls = Cell::new(0);
    let callback_done = Cell::new(false);
    let callback_dropped = Cell::new(false);
    let (release, released) = oneshot::channel::<()>();
    // Borrowed Cell state deliberately makes this callback local, non-Send and
    // non-static. It is one owned future, never recreated after a control event.
    let mut callback = Box::pin(async {
        let _guard = CallbackGuard(&callback_dropped);
        callback_calls.set(callback_calls.get() + 1);
        let _ = released.await;
        callback_done.set(true);
        Err::<(), _>(anyhow::anyhow!("UNIT_LATE_CALLBACK_ERROR"))
    });
    let mut driving = Box::pin(supervisor.drive(callback.as_mut()));
    let first = poll_once(driving.as_mut()).await;
    let first_pending = matches!(&first, Poll::Pending);
    let engaged_before_deadline =
        callback_calls.get() == 1 && !callback_done.get() && !callback_dropped.get();
    trigger.latch(Cause::Deadline);
    let second = if first_pending {
        Some(poll_once(driving.as_mut()).await)
    } else {
        None
    };
    let stop_progressed_before_release = stop_calls.get() == 1;
    let same_callback_retained =
        callback_calls.get() == 1 && !callback_done.get() && !callback_dropped.get();
    let release_sent = release.send(()).is_ok();
    // Drain before EVERY assertion, and do not repoll a completed drive future.
    let callback_result = match first {
        Poll::Ready(result) => result,
        Poll::Pending => match second {
            Some(Poll::Ready(result)) => result,
            _ => driving.as_mut().await,
        },
    };
    drop(driving);
    drop(callback);
    if let Err(error) = callback_result {
        supervisor.operation_failed(error);
    }
    supervisor.finish_stop().await;
    supervisor.wait_attach().await;
    supervisor.finish_stop().await;
    let collected = supervisor.io.collect().await;
    // All local callback, stop and actual Child/capture work completed above.
    let collected = collected.unwrap();
    assert!(first_pending && engaged_before_deadline && same_callback_retained);
    assert!(release_sent && callback_done.get() && callback_dropped.get());
    assert_eq!(callback_calls.get(), 1);
    assert_eq!(stop_calls.get(), 1);
    assert!(
        stop_progressed_before_release,
        "stop did not progress while the original callback was pending"
    );
    assert_eq!(supervisor.cause.get(), Some(Cause::Deadline));
    assert!(
        supervisor
            .errors
            .iter()
            .any(|error| error.to_string().contains("UNIT_LATE_CALLBACK_ERROR"))
    );
    let stopped = supervisor.stop_result.as_ref().unwrap();
    assert!(!stopped.observed_stopped);
    assert!(
        stopped
            .errors
            .iter()
            .any(|error| error.to_string().contains("UNIT_NOT_NATIVE_STOP"))
    );
    assert!(collected.status.is_some());
    assert!(collected.stdout_eof && collected.stderr_eof);
    // This status is only the fixture CLI Child, never a container terminal.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(collected.status.unwrap().signal(), Some(libc::SIGKILL));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn checked_blocking_keeps_control_pollable_and_allows_second_real_pool_check() {
    let (release, blocked) = mpsc::channel::<()>();
    let (entered, observed_entry) = oneshot::channel::<()>();
    let finished = Arc::new(AtomicBool::new(false));
    let worker_finished = finished.clone();
    let mut first = Box::pin(checked_blocking(move || {
        let _ = entered.send(());
        blocked
            .recv()
            .map_err(|_| anyhow::anyhow!("UNIT_CHECK_RELEASE_LOST"))?;
        worker_finished.store(true, Ordering::Release);
        Ok(17usize)
    }));
    let initial = poll_once(first.as_mut()).await;
    let first_pending = matches!(&initial, Poll::Pending);
    let entered = observed_entry.await;
    let (control, control_ready) = oneshot::channel::<()>();
    let control_sent = control.send(()).is_ok();
    let mut control_ready = Box::pin(control_ready);
    let control_result = poll_once(control_ready.as_mut()).await;
    // This real pool work must finish while check1 is still held on std mpsc.
    // No slot-global-one substitute or success from a scheduling flag is used.
    let second = checked_blocking(|| Ok(23usize)).await;
    let first_still_retained = !finished.load(Ordering::Acquire);
    let released = release.send(()).is_ok();
    let first_result = match initial {
        Poll::Ready(result) => result,
        Poll::Pending => first.as_mut().await,
    };
    drop(first);
    // Both blocking closures have returned and their actual joins are observed.
    assert!(first_pending && entered.is_ok());
    assert!(control_sent && matches!(control_result, Poll::Ready(Ok(()))));
    assert!(first_still_retained && released);
    assert_eq!(second.unwrap(), 23);
    assert_eq!(first_result.unwrap(), 17);
    assert!(finished.load(Ordering::Acquire));
}

#[test]
#[ignore = "software Child fixture; selected explicitly by the owning driver test"]
fn driver_child_fixture17() {
    if std::env::var("AI_GUARD_DEADLINE_DRIVER_FIXTURE17")
        .ok()
        .as_deref()
        != Some("park-after-stdin")
    {
        return;
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    std::io::stdin().take(1024).read_to_end(&mut bytes).unwrap();
    // OwnedBatchIo closes an unused stdin. Remain a real live software Child
    // after EOF so only its CLI transport kill ends this fixture, not an engine.
    loop {
        std::thread::park();
    }
}
