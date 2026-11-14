//! Real bounded duplex IO and retained permits. Peer assertions are declared.
use super::*;
use crate::control_fs::Directory;
use crate::runtime::kubernetes::session::{SessionJournal, SessionJournalProtection};
use tokio::sync::Semaphore;

#[path = "bootstrap_deadline_tests.rs"]
mod deadline_tests;

fn binding() -> SessionBootstrapBindingV2 {
    let (f, intent, confirmed) = crate::runtime::kubernetes::session::tests::fixture();
    let root = Directory::open_existing(f._directory.path())
        .unwrap()
        .persistent_child("bootstrap", true)
        .unwrap()
        .unwrap();
    let journal = SessionJournal::prepare_or_resume(
        root,
        SessionJournalProtection::new(Vec::new(), Vec::new()).unwrap(),
        &intent,
        &confirmed,
    )
    .unwrap();
    journal.bootstrap_binding().unwrap()
}
async fn capacity(pool: &std::sync::Arc<Semaphore>) -> BootstrapCapacity {
    BootstrapCapacity::new(
        pool.clone()
            .acquire_many_owned(RESERVED_BYTES)
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn real_handshake_is_declared_only_and_capacity_remains_until_explicit_join() {
    let binding = binding();
    let pool = std::sync::Arc::new(Semaphore::new(RESERVED_BYTES as usize));
    let worker_pool = std::sync::Arc::new(Semaphore::new(RESERVED_BYTES as usize));
    let (control, worker) = tokio::io::duplex(4096);
    let (input, output) = tokio::io::split(control);
    let (worker_input, worker_output) = tokio::io::split(worker);
    let until = Instant::now() + Duration::from_secs(2);
    let owner =
        BootstrapIoOwner::start(binding, input, output, capacity(&pool).await, until).unwrap();
    bootstrap_waiting(
        worker_input,
        worker_output,
        capacity(&worker_pool).await,
        until,
    )
    .await
    .unwrap();
    assert_eq!(pool.available_permits(), 0);
    let observed = owner.join().await.unwrap();
    assert_eq!(observed.provenance(), BootstrapProvenanceV2::Declared);
    assert_eq!(pool.available_permits(), RESERVED_BYTES as usize);
    assert_eq!(
        owner.join().await.unwrap().provenance(),
        BootstrapProvenanceV2::Declared
    );
}

#[tokio::test]
async fn cancelled_waiter_does_not_drop_the_owned_exchange_or_its_capacity() {
    let binding = binding();
    let pool = std::sync::Arc::new(Semaphore::new(RESERVED_BYTES as usize));
    let worker_pool = std::sync::Arc::new(Semaphore::new(RESERVED_BYTES as usize));
    let (control, worker) = tokio::io::duplex(4096);
    let (input, output) = tokio::io::split(control);
    let until = Instant::now() + Duration::from_secs(2);
    let owner =
        BootstrapIoOwner::start(binding, input, output, capacity(&pool).await, until).unwrap();
    let waiting_owner = owner.clone();
    let (pending_tx, pending_rx) = tokio::sync::oneshot::channel();
    let waiter = tokio::spawn(async move {
        let joining = waiting_owner.join();
        tokio::pin!(joining);
        let mut pending_tx = Some(pending_tx);
        std::future::poll_fn(|cx| {
            let result = std::future::Future::poll(joining.as_mut(), cx);
            // Acknowledge only after polling the real join to Pending. The peer
            // cannot answer until the parent has cancelled this waiter.
            if result.is_pending()
                && let Some(tx) = pending_tx.take()
            {
                let _ = tx.send(());
            }
            result
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), pending_rx)
        .await
        .unwrap()
        .unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert_eq!(pool.available_permits(), 0);
    let (input, output) = tokio::io::split(worker);
    bootstrap_waiting(input, output, capacity(&worker_pool).await, until)
        .await
        .unwrap();
    assert_eq!(pool.available_permits(), 0);
    assert_eq!(
        owner.join().await.unwrap().provenance(),
        BootstrapProvenanceV2::Declared
    );
    assert_eq!(pool.available_permits(), RESERVED_BYTES as usize);
}

#[tokio::test]
async fn oversized_prefix_refuses_without_waiting_for_payload_and_releases_only_on_join() {
    let binding = binding();
    let pool = std::sync::Arc::new(Semaphore::new(RESERVED_BYTES as usize));
    let (control, mut peer) = tokio::io::duplex(4096);
    let (input, output) = tokio::io::split(control);
    let owner = BootstrapIoOwner::start(
        binding,
        input,
        output,
        capacity(&pool).await,
        Instant::now() + Duration::from_secs(2),
    )
    .unwrap();
    let _hello = read_frame(&mut peer).await.unwrap();
    peer.write_all(&((MAX_FRAME_BYTES + 1) as u32).to_be_bytes())
        .await
        .unwrap();
    assert!(matches!(owner.join().await, Err(BootstrapError::FrameSize)));
    assert_eq!(pool.available_permits(), RESERVED_BYTES as usize);
}

#[test]
fn closed_wire_refuses_release_start_duplicates_and_insufficient_capacity() {
    let binding = WireBinding::from(&binding());
    let hello = Frame::Hello {
        version: 2,
        exchange_id: Uuid::new_v4(),
        binding,
    };
    let encoded = encode(&hello).unwrap();
    assert!(decode(&encoded).is_ok());
    assert!(decode(br#"{"kind":"start","version":2,"argv":["/bin/sh"]}"#).is_err());
    assert!(decode(br#"{"kind":"release","version":2}"#).is_err());
    assert!(decode(br#"{"kind":"hello","version":2,"version":2}"#).is_err());
    assert!(
        BootstrapCapacity::new(
            std::sync::Arc::new(Semaphore::new(1))
                .try_acquire_owned()
                .unwrap()
        )
        .is_err()
    );
    assert!(!format!("{:?}", BootstrapError::Protocol).contains("argv"));
}

#[test]
fn starting_without_a_runtime_refuses_without_panicking_or_retaining_capacity() {
    let binding = binding();
    let pool = std::sync::Arc::new(Semaphore::new(RESERVED_BYTES as usize));
    let capacity =
        BootstrapCapacity::new(pool.clone().try_acquire_many_owned(RESERVED_BYTES).unwrap())
            .unwrap();
    assert!(matches!(
        BootstrapIoOwner::start(
            binding,
            tokio::io::empty(),
            tokio::io::sink(),
            capacity,
            Instant::now() + Duration::from_secs(2),
        ),
        Err(BootstrapError::Unavailable)
    ));
    assert_eq!(pool.available_permits(), RESERVED_BYTES as usize);
}

#[tokio::test]
async fn an_echo_from_a_different_attempt_is_refused_without_release_or_execution() {
    let pool = std::sync::Arc::new(Semaphore::new(RESERVED_BYTES as usize));
    let (control, mut peer) = tokio::io::duplex(4096);
    let (input, output) = tokio::io::split(control);
    let owner = BootstrapIoOwner::start(
        binding(),
        input,
        output,
        capacity(&pool).await,
        Instant::now() + Duration::from_secs(2),
    )
    .unwrap();
    let Frame::Hello {
        version,
        exchange_id,
        mut binding,
    } = read_frame(&mut peer).await.unwrap()
    else {
        panic!("the real controller must write Hello first");
    };
    binding.attempt_id = Uuid::new_v4();
    write_frame(
        &mut peer,
        &Frame::Ready {
            version,
            exchange_id,
            binding,
        },
    )
    .await
    .unwrap();
    assert!(matches!(owner.join().await, Err(BootstrapError::Binding)));
    assert_eq!(pool.available_permits(), RESERVED_BYTES as usize);
}

/// Watchdog of the test below for an await that must complete. It elapses
/// only when the code under test is broken: a deadline that no longer fires
/// then fails the test instead of hanging it. It is no timing assertion.
async fn must_complete<F: std::future::Future>(what: &str, pending: F) -> F::Output {
    match tokio::time::timeout(Duration::from_secs(5), pending).await {
        Ok(output) => output,
        Err(_) => panic!("{what} did not complete"),
    }
}

/// Real time against scaled windows: Tokio's paused clock needs the
/// `test-util` feature, which this crate does not enable. Each timing
/// assertion is a lower bound or a pending state, so a slow host cannot fail
/// it; only the late Hello must be answered within its one-second window.
/// The literal 5 s ceiling is pinned as a constant, not waited for. Every
/// await that depends on the worker goes through `must_complete`.
#[tokio::test]
async fn waiting_for_attach_accepts_a_late_hello_and_still_bounds_the_handshake() {
    assert_eq!(MAX_HANDSHAKE_TIME, Duration::from_secs(5));
    let wire = WireBinding::from(&binding());
    let pool = std::sync::Arc::new(Semaphore::new(RESERVED_BYTES as usize));

    // Silence longer than the whole window, then a Hello: no deadline ran
    // before the first byte, so the Hello is still answered.
    let window = Duration::from_secs(1);
    let (worker, mut peer) = tokio::io::duplex(4096);
    let reserved = capacity(&pool).await;
    let waiting = tokio::spawn(async move {
        let (mut input, mut output) = tokio::io::split(worker);
        bootstrap_waiting_for_attach(&mut input, &mut output, &reserved, window).await
    });
    tokio::time::sleep(window + Duration::from_millis(200)).await;
    assert!(!waiting.is_finished());
    let exchange_id = Uuid::new_v4();
    write_frame(
        &mut peer,
        &Frame::Hello {
            version: 2,
            exchange_id,
            binding: wire.clone(),
        },
    )
    .await
    .unwrap();
    let Frame::Ready {
        version: 2,
        exchange_id: echoed,
        binding: echoed_binding,
    } = must_complete("the Ready frame", read_frame(&mut peer))
        .await
        .unwrap()
    else {
        panic!("the waiting worker must answer Hello with Ready");
    };
    assert!(echoed == exchange_id && echoed_binding == wire);
    assert_eq!(
        must_complete("the answered worker", waiting).await.unwrap(),
        Ok(WaitingForAttachV2::ReadyWritten)
    );
    let mut rest = Vec::new();
    must_complete("the end of the worker output", peer.read_to_end(&mut rest))
        .await
        .unwrap();
    assert!(rest.is_empty(), "exactly one Ready frame is written");
    assert_eq!(pool.available_permits(), RESERVED_BYTES as usize);

    // A first byte followed by a stall: the window runs from that byte, not
    // from the start of the wait, and no Ready byte is written.
    let window = Duration::from_millis(200);
    let (worker, mut peer) = tokio::io::duplex(4096);
    let reserved = capacity(&pool).await;
    let waiting = tokio::spawn(async move {
        let (mut input, mut output) = tokio::io::split(worker);
        bootstrap_waiting_for_attach(&mut input, &mut output, &reserved, window).await
    });
    tokio::time::sleep(window + Duration::from_millis(100)).await;
    assert!(!waiting.is_finished());
    let first_byte = Instant::now();
    peer.write_all(&[0]).await.unwrap();
    assert_eq!(
        must_complete("the stalled worker", waiting).await.unwrap(),
        Err(BootstrapError::Deadline)
    );
    assert!(first_byte.elapsed() >= window);
    let mut written = Vec::new();
    must_complete(
        "the end of the worker output",
        peer.read_to_end(&mut written),
    )
    .await
    .unwrap();
    assert!(written.is_empty(), "no Ready byte follows a stalled Hello");

    // The window is refused before any read when it is empty or above the
    // ceiling: an input already at its end would otherwise be reported as such.
    let reserved = capacity(&pool).await;
    for refused in [
        Duration::ZERO,
        MAX_HANDSHAKE_TIME + Duration::from_millis(1),
    ] {
        assert_eq!(
            bootstrap_waiting_for_attach(
                &mut tokio::io::empty(),
                &mut tokio::io::sink(),
                &reserved,
                refused,
            )
            .await,
            Err(BootstrapError::Deadline)
        );
    }
    assert_eq!(
        bootstrap_waiting_for_attach(
            &mut tokio::io::empty(),
            &mut tokio::io::sink(),
            &reserved,
            MAX_HANDSHAKE_TIME,
        )
        .await,
        Ok(WaitingForAttachV2::EofBeforeHello)
    );
}

#[test]
fn declared_frame_length_refuses_zero_and_oversize_before_any_allocation() {
    let declared = |length: u32| declared_frame_length(length.to_be_bytes());
    for refused in [0, MAX_FRAME_BYTES as u32 + 1, u32::MAX] {
        assert_eq!(declared(refused), Err(BootstrapError::FrameSize));
    }
    assert_eq!(declared(1), Ok(1));
    assert_eq!(declared(MAX_FRAME_BYTES as u32), Ok(MAX_FRAME_BYTES));
    // The prefix is big-endian: 0x0000_4000 is the bound itself, and the same
    // bytes read the other way round would be 4 MiB.
    assert_eq!(declared_frame_length([0, 0, 0x40, 0]), Ok(MAX_FRAME_BYTES));
    assert_eq!(
        declared_frame_length([0, 0x40, 0, 0]),
        Err(BootstrapError::FrameSize)
    );
}
