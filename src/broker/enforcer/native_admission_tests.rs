//! D7 native admission regressions. The IPC fixture's Ready observation is
//! explicitly synthetic; the lease, retained OS thread, runtime construction
//! and admission/Original/Prepare gates exercised here are the real producers.
use super::*;

#[tokio::test]
async fn recovery_fence_retains_register_waiting_for_native_ready() {
    let mut fixture = OriginalIpcFixture::start().await;
    let registration = registration(fixture.root.path());
    let arm_id = registration.arm.operation_id.clone();
    let digest = registration.digest().unwrap();
    let control = fixture.install(&registration, false);
    control.ready_allowed.store(false, Ordering::SeqCst);
    let reservation = fixture
        .full
        .reserve_prospective(prospective_registration(&registration), 0, false)
        .await
        .unwrap();
    let original_exchange_nonce = fixture
        .full
        .begin_prospective_arm_exchange(&reservation, 0)
        .unwrap();
    let owner = Arc::clone(&fixture.full);
    let pending = tokio::spawn(async move {
        owner
            .bind_prospective(reservation, registration, 0, false, original_exchange_nonce)
            .await
    });

    let expires = Instant::now() + Duration::from_secs(3);
    let actor = loop {
        if let Ok(actor) = fixture.full.actor(&arm_id, &digest)
            && actor.runtime_installed.load(Ordering::SeqCst)
        {
            break actor;
        }
        assert!(Instant::now() < expires, "native runtime was not installed");
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert_eq!(
        actor.state.lock().unwrap().status.phase,
        SlotPhaseV1::Initializing
    );
    fixture
        .full
        .native_stop_installed(&arm_id, &digest)
        .unwrap();
    assert!(
        !pending.is_finished(),
        "the withheld Ready must block Original"
    );

    assert_eq!(fixture.full.admitted.begin_recovery().unwrap(), 1);
    fixture.full.reserve_recovery_stops().unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), pending)
            .await
            .unwrap()
            .unwrap(),
        Err(EnforcerError::NotReady)
    ));
    assert_eq!(fixture.full.admitted.lock().unwrap().len(), 1);
    let retained = fixture.full.actor(&arm_id, &digest).unwrap();
    assert!(
        Arc::ptr_eq(&actor, &retained),
        "fence must retain the exact native owner"
    );
    assert!(retained.state.lock().unwrap().gate.stop_reserved);
    assert!(matches!(
        fixture.full.prepare(&arm_id, &digest, 0),
        Err(EnforcerError::NotReady)
    ));
    fixture
        .full
        .native_stop_installed(&arm_id, &digest)
        .unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn recovery_fence_refuses_late_prepare_but_keeps_exact_cancel() {
    let mut fixture = OriginalIpcFixture::start().await;
    let registration = registration(fixture.root.path());
    let arm_id = registration.arm.operation_id.clone();
    let digest = registration.digest().unwrap();
    fixture.install(&registration, false);
    let reservation = fixture
        .full
        .reserve_prospective(prospective_registration(&registration), 0, false)
        .await
        .unwrap();
    let original_exchange_nonce = fixture
        .full
        .begin_prospective_arm_exchange(&reservation, 0)
        .unwrap();
    let (_, replayed, _, _, original_nonce) = fixture
        .full
        .bind_prospective(reservation, registration, 0, false, original_exchange_nonce)
        .await
        .unwrap();
    assert!(!replayed);
    assert!(original_nonce.is_some());
    fixture
        .full
        .native_stop_installed(&arm_id, &digest)
        .unwrap();

    fixture.full.admitted.begin_recovery().unwrap();
    fixture.full.reserve_recovery_stops().unwrap();
    assert!(matches!(
        fixture.full.prepare(&arm_id, &digest, 0),
        Err(EnforcerError::NotReady)
    ));
    fixture.full.cancel(&arm_id, &digest, 1).unwrap();
    let status = fixture.full.slot_snapshot(&arm_id, &digest).unwrap();
    assert_eq!(status.generation, 1);
    assert_eq!(status.phase, SlotPhaseV1::Stopping);
    assert_eq!(fixture.full.admitted.lock().unwrap().len(), 1);
    fixture.full.complete().await.unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn full_constructed_after_recovery_scan_cannot_reopen_shared_admission() {
    let fixture = OriginalIpcFixture::start().await;
    let admitted = Arc::clone(&fixture.full.admitted);
    assert_eq!(admitted.begin_recovery().unwrap(), 1);
    let late = FullState::new(
        Arc::clone(&fixture.full.ledger),
        fixture.full.epoch,
        8,
        fixture.full.clock.clone(),
        Arc::clone(&admitted),
    )
    .unwrap();
    let registration = registration(fixture.root.path());
    assert!(matches!(
        late.reserve_prospective(prospective_registration(&registration), 0, false)
            .await,
        Err(EnforcerError::NotReady)
    ));
    assert!(matches!(
        late.register(registration, 0, false).await,
        Err(EnforcerError::NotReady)
    ));
    assert!(admitted.lock().unwrap().is_empty());
    assert!(late.actors.lock().unwrap().is_empty());
    assert_eq!(admitted.begin_recovery().unwrap(), 1);
    assert!(fixture.full.admitted.fenced());
    late.finish_idle().await.unwrap();
    fixture.finish().await;
}

#[test]
fn recovery_intent_refuses_original_before_waiting_for_its_commit_gate() {
    let admitted = NativeAdmissionRegistry::fresh();
    let guard = admitted.lock().unwrap();
    let owner = Arc::clone(&admitted);
    let pending = std::thread::spawn(move || owner.begin_recovery());
    let expires = Instant::now() + Duration::from_secs(3);
    while !admitted.fenced() {
        assert!(
            Instant::now() < expires,
            "native fence intent did not publish"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        admitted.recovery_generation(),
        None,
        "intent is not yet a committed fence"
    );
    assert!(matches!(
        admitted.check_original(),
        Err(EnforcerError::NotReady)
    ));
    assert!(matches!(
        admitted.close_fresh_idle(),
        Err(EnforcerError::NotIdle)
    ));
    drop(guard);
    assert_eq!(pending.join().unwrap().unwrap(), 1);
    assert_eq!(admitted.recovery_generation(), Some(1));
}
