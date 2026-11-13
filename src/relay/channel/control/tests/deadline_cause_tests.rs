//! The actual serve select and Close writer use a real WorkerRelayDeadline.
//! Admission/runtime observations remain the existing component fixture; no
//! Source/DispatchAuthority, HTTP response or execution result is fabricated.
use super::*;
use crate::{broker::relay_owner::WorkerRelayDeadline, control::HostClockMethodV1};

async fn close_after_deadline_change(
    change: &str,
    expected_error: ControlError,
    expected_reason: CloseReasonV1,
) {
    let (_temporary, root) = private_root();
    // This existing origin is only inert client/scope metadata. The test never
    // dispatches an HTTP request and creates no pretend Core endpoint.
    let context = FixtureContext::new("http://127.0.0.1:9", PROOF, endpoint());
    let outbox = Outbox::create(&root, &context.scope, &context.secrets).unwrap();
    let io = context.io(outbox, Arc::new(RoleLease::acquire(&root).unwrap()));
    let mut owner = context.owner(io.clone(), 1);
    let (deadline, sample, cancel, observations) = WorkerRelayDeadline::deadline_cause_fixture();
    let (control, mut peer) = tokio::io::duplex(16 * 1024);
    let (input, output) = tokio::io::split(control);
    let (_shutdown_owner, shutdown) = oneshot::channel();
    let serving = owner.serve_with_deadline(
        input,
        output,
        shutdown,
        ControlDeadline::OriginalWorker(deadline),
    );
    tokio::pin!(serving);
    assert!(futures_util::poll!(&mut serving).is_pending());
    assert!(
        observations.load(Ordering::SeqCst) > 0,
        "the real watcher must have entered before the change"
    );
    match change {
        "boot" => sample.lock().unwrap().boot_id = uuid::Uuid::from_u128(77).to_string(),
        "method" => sample.lock().unwrap().method = HostClockMethodV1::DarwinMachContinuousV1,
        "regression" => sample.lock().unwrap().continuous_ns_decimal = "990000009".into(),
        "cancel" => {
            cancel.invalidate();
        }
        "expired" => sample.lock().unwrap().continuous_ns_decimal = "1000000010".into(),
        _ => unreachable!(),
    }
    let close = async {
        loop {
            match read_frame(&mut peer, FrameLimit::Control).await.unwrap() {
                Some(FrameV1::Hello(_)) => {}
                Some(FrameV1::Close(close)) => break close,
                _ => panic!("the real control writer did not deliver Close"),
            }
        }
    };
    let (result, close) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(serving, close)
    })
    .await
    .expect("component waiting oracle did not finish");
    assert_eq!(
        (result, close.reason),
        (Err(expected_error), expected_reason)
    );
    assert_eq!(close.version, 1);
    if change != "expired" {
        // Clock/cancel cases never moved to or beyond the original deadline.
        assert_ne!(sample.lock().unwrap().continuous_ns_decimal, "1000000010");
    }
    io.join_pending().await.unwrap();
}

#[tokio::test]
async fn watcher_keeps_clock_cause_and_writes_general_v1_shutdown_after_waiting() {
    for change in ["boot", "method", "regression"] {
        close_after_deadline_change(
            change,
            ControlError::AuthorityClockUnavailable,
            CloseReasonV1::Shutdown,
        )
        .await;
    }
}

#[tokio::test]
async fn watcher_keeps_cancellation_cause_and_writes_v1_revoked_after_waiting() {
    close_after_deadline_change(
        "cancel",
        ControlError::AuthorityCancelled,
        CloseReasonV1::Revoked,
    )
    .await;
}

#[tokio::test]
async fn watcher_keeps_original_expiration_and_writes_v1_expired_after_waiting() {
    close_after_deadline_change("expired", ControlError::Timeout, CloseReasonV1::Expired).await;
}
