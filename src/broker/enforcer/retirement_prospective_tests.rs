//! Native factory software regressions. Snapshot/receipt values and Ready are
//! explicitly synthetic. The real lease, S budget, retained JoinHandle,
//! runtime installation, binding and restrictive gates are exercised.
use super::*;

#[tokio::test]
async fn prospective_native_capacity_precedes_receipt_and_bind_reuses_owner() {
    let mut fixture = OriginalIpcFixture::start_with_capacity(Uuid::new_v4(), 1).await;
    let registration = registration(fixture.root.path());
    fixture.install(&registration, false);
    let prospective = prospective_registration(&registration);
    let arm_id = Uuid::parse_str(registration.arm_id()).unwrap();
    let reservation = fixture
        .full
        .reserve_prospective(prospective, 0, false)
        .await
        .unwrap();
    assert_eq!(fixture.full.admitted.lock().unwrap().len(), 1);
    assert!(fixture.full.actors.lock().unwrap().is_empty());
    assert!(matches!(
        fixture
            .full
            .admitted
            .lock()
            .unwrap()
            .get(registration.arm_id()),
        Some(RegistryOwner::Prospective)
    ));
    let pending = fixture
        .full
        .prospective
        .lock()
        .unwrap()
        .get(&arm_id)
        .cloned()
        .unwrap();
    assert!(pending.runtime_installed.load(Ordering::SeqCst));
    assert_eq!(pending.key.nonce, reservation.reservation_nonce);
    assert!(matches!(
        fixture.full.register(registration.clone(), 0, false).await,
        Err(EnforcerError::Conflict)
    ));
    let other = super::registration(fixture.root.path());
    assert!(matches!(
        fixture
            .full
            .reserve_prospective(prospective_registration(&other), 0, false)
            .await,
        Err(EnforcerError::Capacity)
    ));

    assert!(matches!(
        fixture
            .full
            .bind_prospective(reservation.clone(), registration.clone(), 0, false, None)
            .await,
        Err(EnforcerError::Unauthorized)
    ));
    let original_exchange_nonce = fixture
        .full
        .begin_prospective_arm_exchange(&reservation, 0)
        .unwrap();
    assert!(original_exchange_nonce.is_some());
    assert!(
        fixture
            .full
            .begin_prospective_arm_exchange(&reservation, 0)
            .unwrap()
            .is_none()
    );
    let (_, replayed, _, actor_nonce, original) = fixture
        .full
        .bind_prospective(
            reservation.clone(),
            registration.clone(),
            0,
            false,
            original_exchange_nonce,
        )
        .await
        .unwrap();
    assert!(!replayed);
    assert!(original.is_some());
    let bound = fixture
        .full
        .actor(registration.arm_id(), &registration.digest().unwrap())
        .unwrap();
    assert_eq!(bound.key.nonce, actor_nonce);
    assert_eq!(bound.key.nonce, pending.key.nonce);
    assert!(Arc::ptr_eq(
        &bound.runtime_installed,
        &pending.runtime_installed
    ));
    assert!(Arc::ptr_eq(&bound.failures, &pending.failures));
    assert_eq!(fixture.full.admitted.lock().unwrap().len(), 1);
    let (_, replayed, _, _, original) = fixture
        .full
        .bind_prospective(reservation, registration, 0, false, original_exchange_nonce)
        .await
        .unwrap();
    assert!(replayed);
    assert!(
        original.is_none(),
        "lost Bind ACK cannot reissue an Original nonce"
    );
    fixture.finish().await;
}

#[tokio::test]
async fn historical_pending_and_receipt_cannot_create_a_prospective_owner() {
    let fixture = OriginalIpcFixture::start_with_capacity(Uuid::new_v4(), 1).await;
    let registration = registration(fixture.root.path());
    assert!(matches!(
        fixture
            .full
            .reserve_prospective(prospective_registration(&registration), 0, true)
            .await,
        Err(EnforcerError::Conflict)
    ));
    assert!(matches!(
        fixture.full.register(registration, 0, false).await,
        Err(EnforcerError::NotReady)
    ));
    assert!(fixture.full.admitted.lock().unwrap().is_empty());
    assert!(fixture.full.actors.lock().unwrap().is_empty());
    assert!(fixture.full.prospective.lock().unwrap().is_empty());
    fixture.finish().await;
}

#[tokio::test]
async fn fenced_unbound_reservation_accepts_only_exact_stop_reconciliation() {
    let mut fixture = OriginalIpcFixture::start_with_capacity(Uuid::new_v4(), 1).await;
    let registration = registration(fixture.root.path());
    fixture.install(&registration, false);
    let prospective = prospective_registration(&registration);
    let reservation = fixture
        .full
        .reserve_prospective(prospective.clone(), 0, false)
        .await
        .unwrap();
    fixture.full.admitted.begin_recovery().unwrap();
    fixture.full.reserve_recovery_stops().unwrap();
    assert!(matches!(
        fixture.full.begin_prospective_arm_exchange(&reservation, 0),
        Err(EnforcerError::NotReady)
    ));
    assert!(matches!(
        fixture
            .full
            .bind_prospective(reservation.clone(), registration.clone(), 0, false, None)
            .await,
        Err(EnforcerError::NotReady)
    ));
    let replay = fixture
        .full
        .reserve_prospective(prospective, 7, true)
        .await
        .unwrap();
    assert_eq!(replay.reservation_nonce, reservation.reservation_nonce);
    let status = {
        let _admitted = fixture.full.admitted.lock().unwrap();
        fixture
            .full
            .restore_prospective_stop(&registration, 7, true)
            .unwrap()
            .unwrap()
    };
    assert!(status.prepared_once);
    assert_eq!(status.generation, 7);
    assert_eq!(status.phase, SlotPhaseV1::Stopping);
    fixture
        .full
        .native_stop_installed(registration.arm_id(), &registration.digest().unwrap())
        .unwrap();
    let (_, _, _, _, original) = fixture
        .full
        .bind_prospective(reservation, registration.clone(), 7, true, None)
        .await
        .unwrap();
    assert!(original.is_none());
    let bound = fixture
        .full
        .actor(registration.arm_id(), &registration.digest().unwrap())
        .unwrap();
    assert!(!bound.state.lock().unwrap().gate.original);
    assert!(matches!(
        fixture.full.revoke(
            registration.arm_id(),
            &registration.digest().unwrap(),
            7,
            bound.key.nonce,
            bound.original_registration_nonce,
            Uuid::new_v4()
        ),
        Err(EnforcerError::Unauthorized)
    ));
    assert!(matches!(
        fixture
            .full
            .prepare(registration.arm_id(), &registration.digest().unwrap(), 7),
        Err(EnforcerError::NotReady)
    ));
    assert_eq!(fixture.full.admitted.lock().unwrap().len(), 1);
    fixture.finish().await;
}

#[tokio::test]
async fn divergent_bind_token_retains_the_installed_exact_native_owner() {
    let mut fixture = OriginalIpcFixture::start_with_capacity(Uuid::new_v4(), 1).await;
    let registration = registration(fixture.root.path());
    fixture.install(&registration, false);
    let reservation = fixture
        .full
        .reserve_prospective(prospective_registration(&registration), 0, false)
        .await
        .unwrap();
    let arm_id = Uuid::parse_str(registration.arm_id()).unwrap();
    let before = fixture
        .full
        .prospective
        .lock()
        .unwrap()
        .get(&arm_id)
        .cloned()
        .unwrap();
    let mut divergent = reservation.clone();
    let original_exchange_nonce = fixture
        .full
        .begin_prospective_arm_exchange(&reservation, 0)
        .unwrap();
    divergent.reservation_nonce = Uuid::new_v4();
    assert!(matches!(
        fixture
            .full
            .bind_prospective(
                divergent,
                registration.clone(),
                0,
                false,
                original_exchange_nonce
            )
            .await,
        Err(EnforcerError::NotReady)
    ));
    let retained = fixture
        .full
        .prospective
        .lock()
        .unwrap()
        .get(&arm_id)
        .cloned()
        .unwrap();
    assert!(Arc::ptr_eq(&before, &retained));
    assert!(retained.runtime_installed.load(Ordering::SeqCst));
    assert!(fixture.full.actors.lock().unwrap().is_empty());
    assert_eq!(fixture.full.admitted.lock().unwrap().len(), 1);
    fixture
        .full
        .bind_prospective(reservation, registration, 0, false, original_exchange_nonce)
        .await
        .unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn lost_begin_exchange_ack_cannot_recover_an_original_from_replay() {
    let mut fixture = OriginalIpcFixture::start_with_capacity(Uuid::new_v4(), 1).await;
    let registration = registration(fixture.root.path());
    fixture.install(&registration, false);
    let reservation = fixture
        .full
        .reserve_prospective(prospective_registration(&registration), 0, false)
        .await
        .unwrap();
    // The first native response exists but is deliberately lost to the caller.
    assert!(
        fixture
            .full
            .begin_prospective_arm_exchange(&reservation, 0)
            .unwrap()
            .is_some()
    );
    let replay = fixture
        .full
        .begin_prospective_arm_exchange(&reservation, 0)
        .unwrap();
    assert!(replay.is_none());
    assert!(matches!(
        fixture
            .full
            .bind_prospective(reservation.clone(), registration.clone(), 0, false, replay)
            .await,
        Err(EnforcerError::Unauthorized)
    ));
    assert_eq!(fixture.full.admitted.lock().unwrap().len(), 1);
    assert!(fixture.full.actors.lock().unwrap().is_empty());
    let (status, _, _, _, original) = fixture
        .full
        .bind_prospective(reservation, registration.clone(), 0, true, None)
        .await
        .unwrap();
    assert!(status.prepared_once);
    assert!(original.is_none());
    let bound = fixture
        .full
        .actor(registration.arm_id(), &registration.digest().unwrap())
        .unwrap();
    assert!(!bound.state.lock().unwrap().gate.original);
    assert!(matches!(
        fixture.full.revoke(
            registration.arm_id(),
            &registration.digest().unwrap(),
            0,
            bound.key.nonce,
            bound.original_registration_nonce,
            Uuid::new_v4()
        ),
        Err(EnforcerError::Unauthorized)
    ));
    fixture.finish().await;
}
