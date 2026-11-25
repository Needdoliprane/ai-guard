//! G09 Legacy fresh-reservation refusal motif. Real native factory, IPC and
//! runtime contexts; no engine, provider or SyntheticReady. Each oracle drives
//! one reservation attempt through its own authenticated response and checks
//! the closed motif chosen on the refusing branch, the historical Legacy
//! compatibility boundary and the absence of any resend or extra effect.
use super::*;
use crate::broker::clock::checked_deadline;
use crate::broker::enforcer::server::test_support::{LegacyIpcFixture, registration};
use serde_json::json;

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

/// Same validated unavailable-invocation shape as the historical fixture, with
/// a foreign boot identity so the pre-allocation clock check must refuse it.
fn other_boot_target(root: &std::path::Path) -> ProspectiveRegistrationV1 {
    let mut registration = registration(root);
    let other_boot = Uuid::from_u128(1).to_string();
    registration.arm.clock.boot_id = if registration.arm.clock.boot_id == other_boot {
        Uuid::from_u128(2).to_string()
    } else {
        other_boot
    };
    registration.receipt.input_hash = registration.arm.input_hash().unwrap();
    registration.prospective().unwrap()
}

fn short_deadline_target(root: &std::path::Path, seconds: u32) -> ProspectiveRegistrationV1 {
    let mut registration = registration(root);
    let clock = NativeHostClock::new().unwrap().sample().unwrap();
    registration.arm.deadline_ns_decimal =
        checked_deadline(clock.continuous_ns().unwrap(), seconds)
            .unwrap()
            .to_string();
    registration.arm.clock = clock;
    registration.arm.timeout_seconds = seconds;
    registration.receipt.input_hash = registration.arm.input_hash().unwrap();
    registration.prospective().unwrap()
}

/// One authenticated Legacy request without any motif negotiation, exactly as
/// a historical client writes it.
async fn raw_rpc(root: &Directory, request: Request) -> Response {
    let endpoint = load_endpoint(root).unwrap().unwrap();
    let endpoints = root.persistent_child("endpoints", false).unwrap().unwrap();
    let mut stream = endpoint.connect_checked(&endpoints).await.unwrap();
    let challenge: Challenge = read_frame(&mut stream, 1024, IO_TIMEOUT).await.unwrap();
    assert_eq!(challenge.epoch, endpoint.epoch());
    write_frame(
        &mut stream,
        &Authentication {
            proof: proof(&capability(root, false).unwrap(), &challenge),
        },
        1024,
        IO_TIMEOUT,
    )
    .await
    .unwrap();
    write_frame(&mut stream, &request, MAX_FRAME, IO_TIMEOUT)
        .await
        .unwrap();
    read_frame(&mut stream, MAX_FRAME, REGISTER_TIMEOUT)
        .await
        .unwrap()
}

fn no_grant(
    client: &EnforcerClient,
    target: &ProspectiveRegistrationV1,
    generation: &CancelGeneration,
) {
    assert!(
        client
            .retained_prospective(target, generation)
            .unwrap()
            .reservation
            .lock()
            .unwrap()
            .is_none(),
        "a refused attempt must not retain a reservation grant"
    );
}

#[tokio::test]
async fn clock_rejected_before_allocation_carries_its_own_closed_motif() {
    let fixture = LegacyIpcFixture::start(1, 0).await;
    let client = connect(&fixture, 1, 0).await;
    let target = other_boot_target(fixture.root().path());
    let generation = CancelGeneration::new();
    let refusal = client
        .legacy_reserve_prospective_diagnosed(target.clone(), generation.clone(), false)
        .await
        .unwrap_err();
    assert_eq!(refusal.code(), EnforcerError::NotReady);
    assert_eq!(
        refusal.motif(),
        Some(LegacyReservationMotifV1::ClockRejectedBeforeAllocation)
    );
    assert_eq!(fixture.reserve_requests(), 1);
    let caps = client.legacy_capabilities().await.unwrap();
    assert_eq!(caps.known_stop_slots, 0);
    assert_eq!(caps.owned_active, Some(0));
    no_grant(&client, &target, &generation);
    drop(client);
    fixture.finish().await;
}

#[tokio::test]
async fn clock_rejected_at_control_before_ack_carries_distinct_motif() {
    let fixture = LegacyIpcFixture::start(1, 0).await;
    let client = connect(&fixture, 1, 0).await;
    let target = short_deadline_target(fixture.root().path(), 2);
    let deadline: u128 = target.arm().deadline_ns_decimal.parse().unwrap();
    let generation = CancelGeneration::new();
    let (reached, resume) = fixture.pause_reserve_ack();
    let reserve = tokio::spawn({
        let client = client.clone();
        let target = target.clone();
        let generation = generation.clone();
        async move {
            client
                .legacy_reserve_prospective_diagnosed(target, generation, false)
                .await
        }
    });
    reached.await.unwrap();
    let expires = Instant::now() + Duration::from_secs(8);
    while NativeHostClock::new()
        .unwrap()
        .sample()
        .unwrap()
        .continuous_ns()
        .unwrap()
        < deadline
    {
        assert!(Instant::now() < expires, "deadline did not elapse");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    resume.send(()).unwrap();
    let refusal = tokio::time::timeout(Duration::from_secs(10), reserve)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(refusal.code(), EnforcerError::NotReady);
    assert_eq!(
        refusal.motif(),
        Some(LegacyReservationMotifV1::ClockRejectedBeforeAck)
    );
    // The cell stays retained after allocation, but no grant was delivered.
    assert_eq!(
        client.legacy_capabilities().await.unwrap().known_stop_slots,
        1
    );
    no_grant(&client, &target, &generation);
    drop(client);
    fixture.finish().await;
}

#[tokio::test]
async fn runtime_unavailable_at_install_deadline_carries_distinct_motif() {
    let fixture = LegacyIpcFixture::start(1, 0).await;
    let client = connect(&fixture, 1, 0).await;
    let target = registration(fixture.root().path()).prospective().unwrap();
    let generation = CancelGeneration::new();
    let (reached, resume) = fixture.pause_before_start();
    let reserve = tokio::spawn({
        let client = client.clone();
        let target = target.clone();
        let generation = generation.clone();
        async move {
            client
                .legacy_reserve_prospective_diagnosed(target, generation, false)
                .await
        }
    });
    reached.await.unwrap();
    // The native worker exits before installing any runtime context; the
    // reservation must wait for the real installation deadline, not a grant.
    fixture.exit_pending_before_install(target.arm_id());
    resume.send(()).unwrap();
    let refusal = tokio::time::timeout(REGISTER_TIMEOUT + Duration::from_secs(10), reserve)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(refusal.code(), EnforcerError::NotReady);
    assert_eq!(
        refusal.motif(),
        Some(LegacyReservationMotifV1::InstallDeadlineElapsed)
    );
    no_grant(&client, &target, &generation);
    drop(client);
    fixture.finish().await;
}

#[tokio::test]
async fn stop_locked_before_ack_with_valid_clock_carries_distinct_motif() {
    let fixture = LegacyIpcFixture::start(1, 0).await;
    let client = connect(&fixture, 1, 0).await;
    let target = registration(fixture.root().path()).prospective().unwrap();
    let generation = CancelGeneration::new();
    let (reached, resume) = fixture.pause_reserve_ack();
    let reserve = tokio::spawn({
        let client = client.clone();
        let target = target.clone();
        let generation = generation.clone();
        async move {
            client
                .legacy_reserve_prospective_diagnosed(target, generation, false)
                .await
        }
    });
    reached.await.unwrap();
    let reservation = fixture.pending_reservation(target.arm_id()).unwrap();
    assert!(matches!(
        client
            .0
            .request(
                Request::CancelProspective {
                    reservation,
                    generation: 1,
                },
                IO_TIMEOUT,
            )
            .await
            .unwrap(),
        Response::Acknowledged {}
    ));
    resume.send(()).unwrap();
    let refusal = tokio::time::timeout(Duration::from_secs(10), reserve)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(refusal.code(), EnforcerError::NotReady);
    assert_eq!(
        refusal.motif(),
        Some(LegacyReservationMotifV1::StopReservedBeforeAck)
    );
    assert_eq!(
        client.legacy_capabilities().await.unwrap().known_stop_slots,
        1
    );
    no_grant(&client, &target, &generation);
    drop(client);
    fixture.finish().await;
}

#[tokio::test]
async fn nominal_negotiated_reservation_succeeds_without_motif() {
    let fixture = LegacyIpcFixture::start(1, 0).await;
    let client = connect(&fixture, 1, 0).await;
    let target = registration(fixture.root().path()).prospective().unwrap();
    let generation = CancelGeneration::new();
    client
        .legacy_reserve_prospective_diagnosed(target.clone(), generation.clone(), false)
        .await
        .unwrap();
    assert_eq!(fixture.reserve_requests(), 1);
    let caps = client.legacy_capabilities().await.unwrap();
    assert_eq!(caps.known_stop_slots, 1);
    assert_eq!(caps.owned_active, Some(1));
    assert!(
        client
            .retained_prospective(&target, &generation)
            .unwrap()
            .reservation
            .lock()
            .unwrap()
            .is_some()
    );
    drop(client);
    fixture.finish().await;
}

#[tokio::test]
async fn unnegotiated_legacy_query_keeps_exact_historical_response() {
    let fixture = LegacyIpcFixture::start(1, 0).await;
    // Historical refusal: exactly the closed error frame, no motif field.
    let reply = raw_rpc(
        fixture.root(),
        Request::ReserveProspective {
            prospective: Box::new(other_boot_target(fixture.root().path())),
            generation: 0,
            replay_only: false,
        },
    )
    .await;
    let wire = serde_json::to_value(&reply).unwrap();
    assert_eq!(wire, json!({"kind":"error","code":"not_ready"}));
    let historical: HistoricalResponse = serde_json::from_value(wire).unwrap();
    assert!(matches!(historical, HistoricalResponse::Error { code } if code == "not_ready"));
    // Historical success: exactly the reservation frame, no motif field.
    let reply = raw_rpc(
        fixture.root(),
        Request::ReserveProspective {
            prospective: Box::new(registration(fixture.root().path()).prospective().unwrap()),
            generation: 0,
            replay_only: false,
        },
    )
    .await;
    assert!(matches!(reply, Response::ProspectiveReserved { .. }));
    let wire = serde_json::to_value(&reply).unwrap();
    assert_eq!(wire.as_object().unwrap().len(), 2);
    assert!(wire.get("motif").is_none());
    let _: HistoricalResponse = serde_json::from_value(wire).unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn new_client_against_old_peer_sends_one_reservation_and_gets_legacy_reply() {
    // Two slots: the refused attempt keeps its exact local target retained,
    // as every unknown admission does; the nominal attempt must still fit.
    let fixture = LegacyIpcFixture::start(2, 0).await;
    // The fixture peer refuses the negotiation frame exactly as the historical
    // decoder does: the connection closes without any response.
    fixture.refuse_motif_negotiation();
    let client = connect(&fixture, 2, 0).await;
    let refused = other_boot_target(fixture.root().path());
    let refusal = client
        .legacy_reserve_prospective_diagnosed(refused.clone(), CancelGeneration::new(), false)
        .await
        .unwrap_err();
    assert_eq!(refusal.code(), EnforcerError::NotReady);
    assert_eq!(refusal.motif(), None);
    assert_eq!(fixture.reserve_requests(), 1);
    let nominal = registration(fixture.root().path()).prospective().unwrap();
    client
        .legacy_reserve_prospective_diagnosed(nominal, CancelGeneration::new(), false)
        .await
        .unwrap();
    assert_eq!(fixture.reserve_requests(), 2);
    assert_eq!(
        client.legacy_capabilities().await.unwrap().known_stop_slots,
        1
    );
    drop(client);
    fixture.finish().await;
}

#[tokio::test]
async fn concurrent_compatible_peers_keep_motifs_on_their_own_attempts() {
    let fixture = LegacyIpcFixture::start(2, 0).await;
    let peer_a = connect(&fixture, 2, 0).await;
    let peer_b = connect(&fixture, 2, 0).await;
    let target_a = other_boot_target(fixture.root().path());
    let target_b = registration(fixture.root().path()).prospective().unwrap();
    let generation_b = CancelGeneration::new();
    let (reached, resume) = fixture.pause_reserve_ack();
    let reserve_b = tokio::spawn({
        let peer_b = peer_b.clone();
        let target_b = target_b.clone();
        let generation_b = generation_b.clone();
        async move {
            peer_b
                .legacy_reserve_prospective_diagnosed(target_b, generation_b, false)
                .await
        }
    });
    reached.await.unwrap();
    // Peer A refuses on its own attempt while peer B is still in flight.
    let refusal_a = peer_a
        .legacy_reserve_prospective_diagnosed(target_a.clone(), CancelGeneration::new(), false)
        .await
        .unwrap_err();
    assert_eq!(
        refusal_a.motif(),
        Some(LegacyReservationMotifV1::ClockRejectedBeforeAllocation)
    );
    assert!(!reserve_b.is_finished());
    let reservation_b = fixture.pending_reservation(target_b.arm_id()).unwrap();
    assert!(matches!(
        peer_b
            .0
            .request(
                Request::CancelProspective {
                    reservation: reservation_b,
                    generation: 1,
                },
                IO_TIMEOUT,
            )
            .await
            .unwrap(),
        Response::Acknowledged {}
    ));
    resume.send(()).unwrap();
    let refusal_b = tokio::time::timeout(Duration::from_secs(10), reserve_b)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(
        refusal_b.motif(),
        Some(LegacyReservationMotifV1::StopReservedBeforeAck)
    );
    assert_ne!(refusal_a.motif(), refusal_b.motif());
    assert_eq!(refusal_a.code(), EnforcerError::NotReady);
    assert_eq!(refusal_b.code(), EnforcerError::NotReady);
    no_grant(&peer_b, &target_b, &generation_b);
    drop(peer_a);
    drop(peer_b);
    fixture.finish().await;
}

#[tokio::test]
async fn lost_refusal_response_keeps_motif_unavailable_and_send_count_one() {
    let fixture = LegacyIpcFixture::start(1, 0).await;
    let client = connect(&fixture, 1, 0).await;
    let target = other_boot_target(fixture.root().path());
    let generation = CancelGeneration::new();
    client
        .legacy_transport()
        .unwrap()
        .lose_next_reserve_reply
        .store(true, Ordering::SeqCst);
    let refusal = client
        .legacy_reserve_prospective_diagnosed(target.clone(), generation.clone(), false)
        .await
        .unwrap_err();
    assert_eq!(refusal.code(), EnforcerError::Unavailable);
    assert_eq!(refusal.motif(), None);
    assert_eq!(fixture.reserve_requests(), 1);
    let caps = client.legacy_capabilities().await.unwrap();
    assert_eq!(caps.known_stop_slots, 0);
    assert_eq!(caps.owned_active, Some(0));
    // No resend, no grant, no Arm exchange can follow the lost response.
    assert_eq!(fixture.reserve_requests(), 1);
    no_grant(&client, &target, &generation);
    assert!(matches!(
        client
            .legacy_begin_prospective_arm_exchange(&target, &generation)
            .await,
        Err(EnforcerError::NotReady)
    ));
    drop(client);
    fixture.finish().await;
}

/// Minimal mirror of the historical wire decoder: unknown kinds and unknown
/// fields are refused, exactly like the pre-motif Request/Response enums.
#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum HistoricalRequest {
    ReserveProspective {
        prospective: serde_json::Value,
        generation: u64,
        replay_only: bool,
    },
    Status {},
}
#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum HistoricalResponse {
    Error { code: String },
    ProspectiveReserved { reservation: serde_json::Value },
}

#[test]
fn motif_wire_is_closed_and_historical_decoders_reject_unknown_frames() {
    for (motif, label) in [
        (
            LegacyReservationMotifV1::ClockRejectedBeforeAllocation,
            "clock_rejected_before_allocation",
        ),
        (
            LegacyReservationMotifV1::ClockRejectedBeforeAck,
            "clock_rejected_before_ack",
        ),
        (
            LegacyReservationMotifV1::InstallDeadlineElapsed,
            "install_deadline_elapsed",
        ),
        (
            LegacyReservationMotifV1::StopReservedBeforeAck,
            "stop_reserved_before_ack",
        ),
    ] {
        assert_eq!(serde_json::to_value(motif).unwrap(), json!(label));
        assert_eq!(
            serde_json::from_value::<LegacyReservationMotifV1>(json!(label)).unwrap(),
            motif
        );
    }
    assert!(serde_json::from_value::<LegacyReservationMotifV1>(json!("invented_motif")).is_err());
    let refusal = LegacyReservationRefusalV1::with_motif(
        EnforcerError::NotReady,
        LegacyReservationMotifV1::ClockRejectedBeforeAllocation,
    );
    assert_eq!(
        serde_json::to_value(refusal).unwrap(),
        json!({"code":"not_ready","motif":"clock_rejected_before_allocation"})
    );
    assert_eq!(
        serde_json::to_value(LegacyReservationRefusalV1::from(EnforcerError::NotReady)).unwrap(),
        json!({"code":"not_ready"})
    );
    // The historical decoder refuses the negotiation request and any
    // motif-bearing refusal frame; the historical error frame still decodes.
    assert!(
        serde_json::from_value::<HistoricalRequest>(json!({"kind":"negotiate_reservation_motif"}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<HistoricalResponse>(
            json!({"kind":"prospective_refused","code":"not_ready","motif":"clock_rejected_before_allocation"})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<HistoricalResponse>(json!({"kind":"error","code":"not_ready"}))
            .is_ok()
    );
}
