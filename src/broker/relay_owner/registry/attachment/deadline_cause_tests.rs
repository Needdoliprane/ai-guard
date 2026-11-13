//! Real waiting component with declared Arm/clock data. No Source, broker
//! authentication, dispatch authority, endpoint or runtime effect is created.
use super::*;
use crate::{
    broker::clock::{ClockError, ClockResult},
    control::{HostClockMethodV1, HostClockSampleV1},
};
use std::sync::atomic::AtomicUsize;

struct WaitingClock {
    sample: Arc<Mutex<HostClockSampleV1>>,
    observations: Arc<AtomicUsize>,
}
impl HostClock for WaitingClock {
    fn sample(&self) -> ClockResult<HostClockSampleV1> {
        self.observations.fetch_add(1, Ordering::SeqCst);
        self.sample
            .lock()
            .map(|sample| sample.clone())
            .map_err(|_| ClockError::InvalidReading)
    }
}

impl WorkerRelayDeadline {
    // Reuse the existing relay-owner Arm fixture, with only its observation
    // positioned 10ms before the same immutable absolute deadline. This test
    // helper issues no Source/DispatchAuthority or execution capability.
    pub(crate) fn deadline_cause_fixture() -> (
        Arc<Self>,
        Arc<Mutex<HostClockSampleV1>>,
        CancelGeneration,
        Arc<AtomicUsize>,
    ) {
        let arm = crate::broker::relay_owner::tests::arm();
        let mut current = arm.clock.clone();
        current.continuous_ns_decimal = "990000010".into();
        let sample = Arc::new(Mutex::new(current));
        let observations = Arc::new(AtomicUsize::new(0));
        let cancel = CancelGeneration::new();
        let deadline = Arc::new(Self {
            last_ns: Mutex::new(10),
            arm,
            clock: Arc::new(WaitingClock {
                sample: sample.clone(),
                observations: observations.clone(),
            }),
            generation: cancel.current(),
            cancel: cancel.clone(),
        });
        (deadline, sample, cancel, observations)
    }
}

async fn finish_wait(
    deadline: &WorkerRelayDeadline,
    stop: &mut watch::Receiver<bool>,
    outcome: Result<()>,
) -> Result<()> {
    // A delayed scheduler may make the unchanged 250ms retry cadence ready
    // alongside the deadline waiter. Permit that wake, then sample the same
    // original deadline again; no timeout or authority is renewed.
    match outcome {
        Ok(()) => wait_issued_binding_retry(deadline, stop).await,
        error => error,
    }
}

#[tokio::test]
async fn late_issue_wait_keeps_clock_failures_after_the_select_is_pending() {
    // Replacing the actual wait result with Expired must fail these cases.
    for change in ["boot", "method", "regression"] {
        let (deadline, sample, _cancel, observations) =
            WorkerRelayDeadline::deadline_cause_fixture();
        let (_stop_owner, mut stop) = watch::channel(false);
        let outcome = {
            let waiting = wait_issued_binding_retry(&deadline, &mut stop);
            tokio::pin!(waiting);
            assert!(futures_util::poll!(&mut waiting).is_pending());
            assert!(
                observations.load(Ordering::SeqCst) > 0,
                "the real deadline waiter must have entered"
            );
            let mut sample = sample.lock().unwrap();
            match change {
                "boot" => sample.boot_id = Uuid::from_u128(77).to_string(),
                "method" => sample.method = HostClockMethodV1::DarwinMachContinuousV1,
                "regression" => sample.continuous_ns_decimal = "990000009".into(),
                _ => unreachable!(),
            }
            drop(sample);
            waiting.await
        };
        assert_eq!(
            finish_wait(&deadline, &mut stop, outcome).await,
            Err(RelayOwnerError::Clock)
        );
        assert_eq!(deadline.arm.deadline_ns_decimal, "1000000010");
    }
}

#[tokio::test]
async fn late_issue_wait_keeps_generation_cancellation_without_expiring_the_arm() {
    let (deadline, sample, cancel, observations) = WorkerRelayDeadline::deadline_cause_fixture();
    let (_stop_owner, mut stop) = watch::channel(false);
    let outcome = {
        let waiting = wait_issued_binding_retry(&deadline, &mut stop);
        tokio::pin!(waiting);
        assert!(futures_util::poll!(&mut waiting).is_pending());
        assert!(observations.load(Ordering::SeqCst) > 0);
        cancel.invalidate();
        waiting.await
    };
    assert_eq!(
        finish_wait(&deadline, &mut stop, outcome).await,
        Err(RelayOwnerError::Cancelled)
    );
    assert_eq!(sample.lock().unwrap().continuous_ns_decimal, "990000010");
    assert_eq!(deadline.arm.deadline_ns_decimal, "1000000010");
}

#[tokio::test]
async fn late_issue_wait_reports_expiration_only_when_the_original_clock_reaches_the_arm() {
    let (deadline, sample, _cancel, observations) = WorkerRelayDeadline::deadline_cause_fixture();
    let (_stop_owner, mut stop) = watch::channel(false);
    let outcome = {
        let waiting = wait_issued_binding_retry(&deadline, &mut stop);
        tokio::pin!(waiting);
        assert!(futures_util::poll!(&mut waiting).is_pending());
        assert!(observations.load(Ordering::SeqCst) > 0);
        sample.lock().unwrap().continuous_ns_decimal = "1000000010".into();
        waiting.await
    };
    assert_eq!(
        finish_wait(&deadline, &mut stop, outcome).await,
        Err(RelayOwnerError::Expired)
    );
    assert_eq!(deadline.arm.deadline_ns_decimal, "1000000010");
}
