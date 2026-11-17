//! Real native Legacy factory/IPC/handles/runtime, no SyntheticReady or engine.
//! No positive Claim/start or terminal/Q/quota conclusion is made here.
use super::*;
use crate::broker::enforcer::server::test_support::{LegacyIpcFixture, registration};

async fn connect(fixture: &LegacyIpcFixture, s: u32, w: u32) -> EnforcerClient {
    let root = fixture.root();
    EnforcerClient::connect(
        root,
        &std::env::current_exe().unwrap(),
        load_endpoint(root).unwrap().unwrap(),
        capability(root, false).unwrap(),
        s,
        w,
        false,
    )
    .await
    .ok()
    .unwrap()
}

#[tokio::test]
async fn native_s1_reservation_precedes_arm_and_status_shutdown_count_it() {
    let fixture = LegacyIpcFixture::start(1, 1).await;
    let client = connect(&fixture, 1, 1).await;
    let first = registration(fixture.root().path());
    client
        .legacy_reserve_prospective(first.prospective().unwrap(), CancelGeneration::new(), false)
        .await
        .unwrap();
    let observed = client.legacy_capabilities().await.unwrap();
    assert_eq!(observed.known_stop_slots, 1);
    assert_eq!(observed.owned_active, Some(1));
    assert_eq!(client.status().await.unwrap().slot_count(), 1);
    assert!(matches!(
        client.shutdown_if_idle().await,
        Err(EnforcerError::NotIdle)
    ));
    let second = registration(fixture.root().path());
    assert!(matches!(
        client
            .legacy_reserve_prospective(
                second.prospective().unwrap(),
                CancelGeneration::new(),
                false
            )
            .await,
        Err(EnforcerError::Capacity)
    ));
    // These are actual context installation observations before any Arm RPC,
    // not S/W arithmetic, a stopped container, or a usable engine permit.
    drop(client);
    fixture.finish().await;
}

#[tokio::test]
async fn receipt_only_generic_register_adapter_is_restrictive_before_wire() {
    let fixture = LegacyIpcFixture::start(1, 0).await;
    let client = connect(&fixture, 1, 0).await;
    let registration = registration(fixture.root().path());
    assert!(client.0.legacy_observe_only(&registration, false).unwrap());
    let adapted = client
        .0
        .adapt_legacy_request(Request::Register {
            registration: Box::new(registration),
            generation: 0,
            observe_only: false,
        })
        .unwrap();
    assert!(matches!(
        adapted,
        Request::Register {
            observe_only: true,
            ..
        }
    ));
    assert_eq!(
        client.legacy_capabilities().await.unwrap().known_stop_slots,
        0
    );
    // Same old V1 shape with observe_only=true, before transport. No receipt
    // metadata or old peer's permissive behavior can supply Original here.
    drop(client);
    fixture.finish().await;
}

#[tokio::test]
async fn reconnect_cannot_recreate_fresh_startup_from_native_launch_nonce() {
    let fixture = LegacyIpcFixture::start(1, 0).await;
    let client = connect(&fixture, 1, 0).await;
    let value = client.legacy_capabilities().await.unwrap();
    assert_eq!(value.fresh_launch_nonce, Some(fixture.launch_nonce()));
    assert!(!client.legacy_fresh_startup().await.unwrap());
    let generation = client.legacy_begin_recovery().await.unwrap();
    assert!(generation > 0);
    assert!(!client.legacy_fresh_startup().await.unwrap());
    assert!(matches!(
        client.require_admission_ready().await,
        Err(EnforcerError::NotReady)
    ));
    let target = registration(fixture.root().path()).prospective().unwrap();
    assert!(
        client
            .legacy_reserve_prospective(target, CancelGeneration::new(), false)
            .await
            .is_err()
    );
    drop(client);
    fixture.finish().await;
}

#[tokio::test]
async fn lost_real_begin_reply_retains_s1_and_replay_has_no_original() {
    let fixture = LegacyIpcFixture::start(1, 0).await;
    let client = connect(&fixture, 1, 0).await;
    let registration = registration(fixture.root().path());
    let target = registration.prospective().unwrap();
    let generation = CancelGeneration::new();
    client
        .legacy_reserve_prospective(target.clone(), generation.clone(), false)
        .await
        .unwrap();
    client
        .legacy_transport()
        .unwrap()
        .lose_next_begin_reply
        .store(true, Ordering::SeqCst);
    assert!(matches!(
        client
            .legacy_begin_prospective_arm_exchange(&target, &generation)
            .await,
        Err(EnforcerError::Unavailable)
    ));
    let replay = client
        .legacy_begin_prospective_arm_exchange(&target, &generation)
        .await
        .unwrap();
    assert!(!replay.permits_original());
    assert_eq!(
        client.legacy_capabilities().await.unwrap().owned_active,
        Some(1)
    );
    let slot = client
        .legacy_bind_prospective(registration, generation, replay)
        .await
        .unwrap();
    assert!(slot.local.status.lock().unwrap().0.prepared_once);
    assert!(slot.check_ready_for_claim(0).is_err());
    drop(slot);
    drop(client);
    fixture.finish().await;
}

#[tokio::test]
async fn original_nonce_is_consumed_before_failed_bind_connect_and_generic_replay() {
    let fixture = LegacyIpcFixture::start(1, 0).await;
    let client = connect(&fixture, 1, 0).await;
    let registration = registration(fixture.root().path());
    let target = registration.prospective().unwrap();
    let generation = CancelGeneration::new();
    client
        .legacy_reserve_prospective(target.clone(), generation.clone(), false)
        .await
        .unwrap();
    let exchange = client
        .legacy_begin_prospective_arm_exchange(&target, &generation)
        .await
        .unwrap();
    assert!(exchange.permits_original());
    client
        .legacy_transport()
        .unwrap()
        .fail_next_bind_before_connect
        .store(true, Ordering::SeqCst);
    assert!(matches!(
        client
            .legacy_bind_prospective(registration.clone(), generation.clone(), exchange)
            .await,
        Err(EnforcerError::Unavailable)
    ));
    assert!(
        client
            .retained_prospective(&target, &generation)
            .unwrap()
            .original_exchange_nonce
            .lock()
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        client.register(registration, generation).await,
        Err(EnforcerError::Unauthorized)
    ));
    assert_eq!(
        client.legacy_capabilities().await.unwrap().known_stop_slots,
        1
    );
    drop(client);
    fixture.finish().await;
}

#[tokio::test]
async fn lost_real_restrictive_bind_reply_cannot_become_original_via_register() {
    let fixture = LegacyIpcFixture::start(1, 0).await;
    let client = connect(&fixture, 1, 0).await;
    let registration = registration(fixture.root().path());
    let target = registration.prospective().unwrap();
    let generation = CancelGeneration::new();
    client
        .legacy_reserve_prospective(target.clone(), generation.clone(), false)
        .await
        .unwrap();
    let exchange = client
        .legacy_begin_prospective_arm_exchange(&target, &generation)
        .await
        .unwrap()
        .into_restrictive();
    client
        .legacy_transport()
        .unwrap()
        .lose_next_bind_reply
        .store(true, Ordering::SeqCst);
    assert!(matches!(
        client
            .legacy_bind_prospective(registration.clone(), generation.clone(), exchange)
            .await,
        Err(EnforcerError::Unavailable)
    ));
    assert!(
        client
            .0
            .slots
            .lock()
            .unwrap()
            .contains_key(registration.arm_id())
    );
    assert!(matches!(
        client
            .register(registration.clone(), generation.clone())
            .await,
        Err(EnforcerError::Unauthorized)
    ));
    let replay = client
        .legacy_begin_prospective_arm_exchange(&target, &generation)
        .await
        .unwrap();
    assert!(!replay.permits_original());
    let slot = client
        .legacy_bind_prospective(registration, generation, replay)
        .await
        .unwrap();
    assert!(slot.local.status.lock().unwrap().0.prepared_once);
    assert_eq!(client.status().await.unwrap().slot_count(), 1);
    drop(slot);
    drop(client);
    fixture.finish().await;
}

#[test]
fn negotiated_legacy_counts_are_closed_bounded_and_keep_unknown_distinct() {
    let wire = serde_json::json!({"version":1,"epoch":Uuid::new_v4(),"max_slots":1,"max_recovery_stop_owners":1,"recovery_generation":1,"fresh_launch_nonce":null,"known_stop_slots":1,"owned_active":null,"recovery_incomplete":true});
    let capabilities: LegacyProspectiveCapabilitiesV1 =
        serde_json::from_value(wire.clone()).unwrap();
    capabilities.validate(capabilities.epoch, 1, 1).unwrap();
    assert!(capabilities.owned_active.is_none());
    let mut too_many = wire.clone();
    too_many["known_stop_slots"] = serde_json::json!(2);
    let excess: LegacyProspectiveCapabilitiesV1 = serde_json::from_value(too_many).unwrap();
    assert!(excess.validate(excess.epoch, 1, 1).is_err());
    let mut missing = wire.clone();
    missing.as_object_mut().unwrap().remove("owned_active");
    assert!(serde_json::from_value::<LegacyProspectiveCapabilitiesV1>(missing).is_err());
    let mut unknown = wire;
    unknown["full3_ledger"] = serde_json::json!("invented");
    assert!(serde_json::from_value::<LegacyProspectiveCapabilitiesV1>(unknown).is_err());
    // Codec boundary only. These constructed DATA values certify no owner.
}
