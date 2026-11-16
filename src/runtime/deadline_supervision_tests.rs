//! Deterministic scheduling regressions for the helper called by execute_created.
//!
//! These units model a pending observation/callback, not a runtime, child,
//! authority, stop effect or durable acknowledgement. They perform no native IO.
//! The baseline is expected to fail the final pollability assertion only after
//! the owned kernel and observation have actually completed. NOT_RUN here.
use super::deadline_supervision::{Event, next_event};
use std::{
    cell::Cell,
    future::{Future, pending, poll_fn, ready},
    pin::Pin,
    task::Poll,
};
use tokio::sync::oneshot;

#[derive(Default)]
struct Probe {
    entered: Cell<bool>,
    completed: Cell<bool>,
    guard_dropped: Cell<bool>,
    released: Cell<bool>,
}

struct ActiveProbe<'a>(&'a Probe);
impl Drop for ActiveProbe<'_> {
    fn drop(&mut self) {
        self.0.guard_dropped.set(true);
    }
}

async fn pending_unit(probe: &Probe, release: oneshot::Receiver<()>) {
    let _active = ActiveProbe(probe);
    probe.entered.set(true);
    probe.released.set(release.await.is_ok());
    probe.completed.set(true);
}

// Return one native Future::poll result immediately, including Poll::Pending.
// No wall-clock timeout, simulated clock or scheduler sleep is used.
async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
}

#[derive(Clone, Copy, Debug)]
enum Trigger {
    Deadline,
    Stop,
}

#[derive(Debug)]
struct Outcome {
    initial_pending: bool,
    entered_before_trigger: bool,
    retained_before_trigger: bool,
    trigger_sent: bool,
    selected_before_release: bool,
    retained_until_release: bool,
    release_sent: bool,
    completed: bool,
    guard_dropped_after_completion: bool,
    release_observed: bool,
}

async fn exercise<O: Future<Output = ()>>(
    observation: O,
    probe: &Probe,
    release: oneshot::Sender<()>,
    trigger: Trigger,
) -> Outcome {
    let (deadline, mut deadline_ready) = oneshot::channel::<()>();
    let (stop, mut stop_ready) = oneshot::channel::<()>();
    let mut observation = Box::pin(observation);
    // The trigger receivers are owned outside their branch futures. Selecting
    // tick in the baseline must not destroy the controlled deadline/stop input.
    let mut kernel = Box::pin(next_event(
        pending::<()>(),
        async {
            let _ = (&mut deadline_ready).await;
        },
        async {
            let _ = (&mut stop_ready).await;
        },
        ready(()),
        observation.as_mut(),
    ));
    let initial = poll_once(kernel.as_mut()).await;
    let initial_pending = matches!(&initial, Poll::Pending);
    let entered_before_trigger = probe.entered.get();
    let retained_before_trigger = !probe.completed.get() && !probe.guard_dropped.get();
    let trigger_sent = match trigger {
        Trigger::Deadline => deadline.send(()).is_ok(),
        Trigger::Stop => stop.send(()).is_ok(),
    };
    // Never poll a kernel that already returned Ready, even on an unexpected
    // early result. Record the oracle before releasing the pending unit.
    let second = if initial_pending {
        Some(poll_once(kernel.as_mut()).await)
    } else {
        None
    };
    let selected_before_release = match (&trigger, &second) {
        (Trigger::Deadline, Some(Poll::Ready(Event::Deadline))) => true,
        (Trigger::Stop, Some(Poll::Ready(Event::Interrupted(())))) => true,
        _ => false,
    };
    let retained_until_release = !probe.completed.get() && !probe.guard_dropped.get();
    let mut resolved = match initial {
        Poll::Ready(event) => Some(event),
        Poll::Pending => None,
    };
    if let Some(Poll::Ready(event)) = second {
        resolved = Some(event);
    }
    let release_sent = release.send(()).is_ok();
    // Cleanup precedes EVERY assertion: baseline Pending resumes into Observed;
    // a corrected helper may already have selected the independent control.
    let _end = match resolved {
        Some(event) => event,
        None => kernel.as_mut().await,
    };
    drop(kernel);
    // Drop the borrowing kernel before resuming its separately owned unit. A
    // completed unit is never polled again, including the baseline Observed case.
    if !probe.completed.get() {
        observation.as_mut().await;
    }
    drop(observation);
    Outcome {
        initial_pending,
        entered_before_trigger,
        retained_before_trigger,
        trigger_sent,
        selected_before_release,
        retained_until_release,
        release_sent,
        completed: probe.completed.get(),
        guard_dropped_after_completion: probe.guard_dropped.get(),
        release_observed: probe.released.get(),
    }
}

fn assert_collected(outcome: &Outcome) {
    assert!(
        outcome.initial_pending,
        "initial kernel poll was not pending: {outcome:?}"
    );
    assert!(
        outcome.entered_before_trigger,
        "unit was not engaged: {outcome:?}"
    );
    assert!(
        outcome.retained_before_trigger,
        "unit was not retained: {outcome:?}"
    );
    assert!(
        outcome.trigger_sent,
        "controlled input was lost: {outcome:?}"
    );
    assert!(
        outcome.retained_until_release,
        "unit ended before release: {outcome:?}"
    );
    assert!(
        outcome.release_sent && outcome.release_observed,
        "release was not collected: {outcome:?}"
    );
    assert!(
        outcome.completed && outcome.guard_dropped_after_completion,
        "owned unit did not complete before assertions: {outcome:?}"
    );
}

#[tokio::test]
async fn deadline_remains_pollable_while_observation_pending() {
    let probe = Probe::default();
    let (release, released) = oneshot::channel();
    let outcome = exercise(
        pending_unit(&probe, released),
        &probe,
        release,
        Trigger::Deadline,
    )
    .await;
    assert_collected(&outcome);
    assert!(
        outcome.selected_before_release,
        "deadline was not selected while the observation remained pending: {outcome:?}"
    );
}

#[tokio::test]
async fn stop_remains_pollable_while_callback_pending() {
    let probe = Probe::default();
    let inspection_returned = Cell::new(false);
    let callback_returned = Cell::new(false);
    let (release, released) = oneshot::channel();
    // Borrowed Cell state deliberately permits neither Send nor 'static. It
    // models the managed wrapper's local callback only, never a native proof.
    let observation_and_callback = async {
        inspection_returned.set(true);
        let callback = || pending_unit(&probe, released);
        callback().await;
        callback_returned.set(true);
    };
    let outcome = exercise(observation_and_callback, &probe, release, Trigger::Stop).await;
    assert_collected(&outcome);
    assert!(
        inspection_returned.get() && callback_returned.get(),
        "callback chain did not finish before assertions"
    );
    assert!(
        outcome.selected_before_release,
        "stop was not selected while the callback remained pending: {outcome:?}"
    );
}
