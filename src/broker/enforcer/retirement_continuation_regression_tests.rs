//! Source-only software regressions: actual native factories, IPC, Q and joins;
//! synthetic Ready/receipt/audit metadata, no engine or allocation conclusion.
use super::*;

#[tokio::test]
async fn bound_native_actor_retains_the_real_unavailable_runtime_cause() {
    let fixture = OriginalIpcFixture::start_with_capacity(Uuid::new_v4(), 1).await;
    let client = fixture.client().await;
    let registration = registration(fixture.root.path());
    let target = prospective_registration(&registration);
    let generation = CancelGeneration::new();
    // No synthetic Ready hook is installed for this Arm. The native bound
    // consumer really calls reopen_native on the unavailable fixture program.
    client
        .reserve_prospective(target.clone(), generation.clone(), false)
        .await
        .unwrap();
    let exchange = client
        .begin_prospective_arm_exchange(&target, &generation)
        .await
        .unwrap();
    let restricted = client
        .bind_prospective(
            registration.clone(),
            generation,
            exchange.into_restrictive(),
        )
        .await
        .unwrap();
    let actor = fixture
        .full
        .actor(registration.arm_id(), &registration.digest().unwrap())
        .unwrap();
    let arm = Uuid::parse_str(registration.arm_id()).unwrap();
    let pending = fixture
        .full
        .prospective
        .lock()
        .unwrap()
        .get(&arm)
        .cloned()
        .unwrap();
    assert!(Arc::ptr_eq(&actor.failures, &pending.failures));
    let expires = Instant::now() + Duration::from_secs(3);
    loop {
        if actor
            .failures
            .inspect_first_for_test(|cause| cause.is_some())
        {
            break;
        }
        assert!(
            Instant::now() < expires,
            "the true native reopen failure did not reach its owner"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let original_object = actor.failures.inspect_first_for_test(|cause| {
        cause.unwrap().root_cause() as *const dyn std::error::Error as *const () as usize
    });
    // The exact first object survives another real reopen attempt in the
    // bound native consumer, without serializing or formatting that cause.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    actor.failures.inspect_first_for_test(|cause| {
        assert_eq!(
            cause.unwrap().root_cause() as *const dyn std::error::Error as *const () as usize,
            original_object
        );
    });
    assert!(actor.failures.facts().retained_causes >= 1);
    assert!(actor.failures.facts().omissions_exact);
    assert_eq!(fixture.full.admitted.lock().unwrap().len(), 1);
    assert!(restricted.revoke_unused(Uuid::new_v4()).await.is_err());
    // Fixture teardown only, after the assertions: physically finish this
    // test's native thread, without creating T/Q, terminal or quota evidence.
    actor.exit.store(true, Ordering::SeqCst);
    actor.wake.notify_all();
    fixture.finish().await;
}

#[tokio::test]
async fn verified_native_q_releases_s1_for_the_next_exact_prospective_arm() {
    let mut fixture = OriginalIpcFixture::start_with_capacity(Uuid::new_v4(), 1).await;
    let client = fixture.client().await;
    let first = registration(fixture.root.path());
    let first_id = first.arm_id().to_owned();
    fixture.install(&first, true);
    let first_slot = fixture
        .register_new(&client, first, CancelGeneration::new())
        .await
        .unwrap();
    let second = registration(fixture.root.path());
    fixture.install(&second, true);
    let second_target = prospective_registration(&second);
    let second_hash = second_target.digest().unwrap();
    let second_deadline = second.arm.deadline_ns_decimal.clone();
    let second_generation = CancelGeneration::new();
    assert!(matches!(
        client
            .reserve_prospective(second_target.clone(), second_generation.clone(), false)
            .await,
        Err(EnforcerError::Capacity)
    ));
    assert_eq!(fixture.full.admitted.lock().unwrap().len(), 1);
    assert!(matches!(
        fixture
            .full
            .reserve_prospective(second_target.clone(), 0, false)
            .await,
        Err(EnforcerError::Capacity)
    ));
    let unused = first_slot.revoke_unused(Uuid::new_v4()).await.unwrap();
    let (identity, command) = fixture.command(&unused, Uuid::new_v4());
    assert!(matches!(
        fixture.raw(command).await.unwrap(),
        FullReply::Retirement {
            receipt: Some(_),
            ..
        }
    ));
    // The native coordinator only emits Q after the actual keyed handle join.
    let verified = client.query_retirement(&identity).await.unwrap().unwrap();
    assert_eq!(
        verified.receipt().finalization().joined().original_epoch(),
        identity.original_epoch()
    );
    assert_eq!(
        verified
            .receipt()
            .finalization()
            .joined()
            .original_actor_nonce(),
        &unused.actor_nonce()
    );
    assert!(fixture.full.admitted.lock().unwrap().is_empty());
    assert!(fixture.full.actors.lock().unwrap().is_empty());
    assert!(fixture.full.prospective.lock().unwrap().is_empty());
    assert!(
        client.has_local_slot(&first_id).unwrap(),
        "Q lookup alone cannot discard a caller slot"
    );
    client.retire_local_slot(&verified).unwrap();
    assert!(!client.has_local_slot(&first_id).unwrap());
    // This succeeds only when both client and native S=1 retentions have been
    // released by verified native retirement, never by Drop or a status.
    client
        .reserve_prospective(second_target.clone(), second_generation.clone(), false)
        .await
        .unwrap();
    let exchange = client
        .begin_prospective_arm_exchange(&second_target, &second_generation)
        .await
        .unwrap();
    assert!(exchange.permits_original());
    let second_slot = client
        .bind_prospective(second.clone(), second_generation, exchange)
        .await
        .unwrap();
    assert_eq!(
        second_hash,
        prospective_registration(&second).digest().unwrap()
    );
    assert_eq!(second_deadline, second.arm.deadline_ns_decimal);
    assert_eq!(fixture.full.admitted.lock().unwrap().len(), 1);
    assert_eq!(fixture.full.prospective.lock().unwrap().len(), 1);
    assert!(second_slot.revoke_unused(Uuid::new_v4()).await.is_ok());
    fixture.finish().await;
}

#[tokio::test]
async fn lost_bind_reply_then_generic_register_cannot_reexport_original_nonce() {
    let mut fixture = OriginalIpcFixture::start_with_capacity(Uuid::new_v4(), 1).await;
    let client = fixture.client().await;
    let registration = registration(fixture.root.path());
    let arm_id = Uuid::parse_str(registration.arm_id()).unwrap();
    let digest = registration.digest().unwrap();
    fixture.install(&registration, false);
    let target = prospective_registration(&registration);
    let generation = CancelGeneration::new();
    client
        .reserve_prospective(target.clone(), generation.clone(), false)
        .await
        .unwrap();
    let exchange = client
        .begin_prospective_arm_exchange(&target, &generation)
        .await
        .unwrap();
    assert!(exchange.permits_original());
    client.lose_next_bind_reply_after_native_for_test().unwrap();
    assert!(matches!(
        client
            .bind_prospective(registration.clone(), generation.clone(), exchange)
            .await,
        Err(EnforcerError::Unavailable)
    ));
    let bound = fixture.full.actor(registration.arm_id(), &digest).unwrap();
    let original_key = bound.key;
    assert!(bound.runtime_installed.load(Ordering::SeqCst));
    assert!(client.has_local_slot(registration.arm_id()).unwrap());
    // bound=false in request_full_legacy after a true Bind reply was discarded.
    // The nonce taken before that write must not be re-exported by Register.
    assert!(matches!(
        client
            .register(registration.clone(), generation.clone())
            .await,
        Err(EnforcerError::Unauthorized)
    ));
    assert!(
        fixture
            .full
            .actor(registration.arm_id(), &digest)
            .unwrap()
            .key
            == original_key
    );
    assert_eq!(fixture.full.admitted.lock().unwrap().len(), 1);
    assert_eq!(fixture.full.prospective.lock().unwrap().len(), 1);
    assert!(
        fixture
            .full
            .prospective
            .lock()
            .unwrap()
            .contains_key(&arm_id)
    );
    let replay = client
        .begin_prospective_arm_exchange(&target, &generation)
        .await
        .unwrap();
    assert!(!replay.permits_original());
    let restricted = client
        .bind_prospective(registration, generation, replay)
        .await
        .unwrap();
    assert!(matches!(
        restricted.revoke_unused(Uuid::new_v4()).await,
        Err(EnforcerError::Unauthorized | EnforcerError::NotReady)
    ));
    fixture.finish().await;
}
