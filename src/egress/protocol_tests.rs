//! Pure state/DTO tests on synthetic bytes, without runtime or network effects.
use super::protocol::{
    DirectionV1, EgressCapabilityV1, EgressChannelState, EgressMessageV1, EgressProtocolError,
    EgressReasonV1,
};

fn open(flow_id: u64) -> EgressMessageV1 {
    EgressMessageV1::Open {
        flow_id,
        authority: "api.example.test:443".into(),
    }
}
fn ready() -> EgressChannelState {
    let mut state = EgressChannelState::new(0);
    state
        .accept_capability(Some(&EgressCapabilityV1::default()), 0)
        .unwrap();
    state
}
fn accepted(state: &mut EgressChannelState, flow_id: u64, now: u64) {
    state.on_worker(&open(flow_id), now).unwrap();
    state
        .on_broker(&EgressMessageV1::Accepted { flow_id }, now)
        .unwrap();
}

#[test]
fn egress_protocol_unavailable_round_trips_without_inventing_a_narrower_cause() {
    for message in [
        EgressMessageV1::Rejected { flow_id: 1, reason: EgressReasonV1::Unavailable },
        EgressMessageV1::Reset { flow_id: 1, reason: EgressReasonV1::Unavailable },
        EgressMessageV1::Closed { flow_id: 1, reason: EgressReasonV1::Unavailable },
    ] {
        let encoded = message.encode_payload().unwrap();
        let decoded = EgressMessageV1::decode_payload(&encoded).unwrap();
        assert_eq!(decoded.encode_payload().unwrap(), encoded);
        assert!(matches!(decoded,
            EgressMessageV1::Rejected { reason: EgressReasonV1::Unavailable, .. }
            | EgressMessageV1::Reset { reason: EgressReasonV1::Unavailable, .. }
            | EgressMessageV1::Closed { reason: EgressReasonV1::Unavailable, .. }));
    }
    assert!(EgressMessageV1::decode_payload(
        br#"{"operation":"rejected","flow_id":1,"reason":"unknown_guess"}"#
    ).is_err());
}

#[test]
fn egress_protocol_end_of_stream_is_a_closed_observation_only() {
    let message = EgressMessageV1::Closed { flow_id: 1, reason: EgressReasonV1::EndOfStream };
    let encoded = message.encode_payload().unwrap();
    assert!(matches!(EgressMessageV1::decode_payload(&encoded).unwrap(),
        EgressMessageV1::Closed { flow_id: 1, reason: EgressReasonV1::EndOfStream }));
    for message in [
        EgressMessageV1::Rejected { flow_id: 1, reason: EgressReasonV1::EndOfStream },
        EgressMessageV1::Reset { flow_id: 1, reason: EgressReasonV1::EndOfStream },
    ] { assert_eq!(message.encode_payload(), Err(EgressProtocolError::Protocol)); }
    assert_eq!(EgressMessageV1::decode_payload(
        br#"{"operation":"reset","flow_id":1,"reason":"end_of_stream"}"#
    ).err(), Some(EgressProtocolError::Protocol));
}

#[test]
fn egress_protocol_requires_explicit_capability_and_current_handshake() {
    let mut absent = EgressChannelState::new(0);
    assert_eq!(
        absent.accept_capability(None, 0),
        Err(EgressProtocolError::Capability)
    );
    assert!(absent.is_closed());
    let mut unknown = EgressCapabilityV1::default();
    unknown.version = 2;
    assert!(
        EgressChannelState::new(0)
            .accept_capability(Some(&unknown), 0)
            .is_err()
    );
    let mut raised = EgressCapabilityV1::default();
    raised.limits.data_bytes += 1;
    assert!(
        EgressChannelState::new(0)
            .accept_capability(Some(&raised), 0)
            .is_err()
    );
    assert_eq!(
        EgressChannelState::new(0).accept_capability(Some(&EgressCapabilityV1::default()), 5_000),
        Err(EgressProtocolError::Timeout)
    );
    let mut state = ready();
    assert!(
        state
            .accept_capability(Some(&EgressCapabilityV1::default()), 1)
            .is_err()
    );
}

#[test]
fn egress_protocol_rejects_identity_fields_duplicates_and_unknown_operations() {
    let fixtures: &[&[u8]] = &[
        br#"{"operation":"open","flow_id":1,"authority":"api.example.test:443","tenant":"B"}"#,
        br#"{"operation":"open","flow_id":1,"authority":"api.example.test:443","flow_id":2}"#,
        br#"{"operation":"open","operation":"accepted","flow_id":1,"authority":"api.example.test:443"}"#,
        br#"{"operation":"release","flow_id":1}"#,
        br#"{"operation":"ready","flow_id":1}"#,
        br#"{"operation":"open","flow_id":0,"authority":"api.example.test:443"}"#,
        br#"{"operation":"open","flow_id":1,"authority":"https://api.example.test/"}"#,
        br#"{"operation":"open","flow_id":1,"authority":"127.0.0.1:443"}"#,
        br#"{"operation":"data","flow_id":1,"data_b64":"AA==","bytes":1}"#,
        br#"{"operation":"data","flow_id":1,"data_b64":"AA==","data_b64":"AQ=="}"#,
    ];
    for input in fixtures {
        assert!(EgressMessageV1::decode_payload(input).is_err());
    }
    let parsed = EgressMessageV1::decode_payload(
        br#"{"operation":"open","flow_id":1,"authority":"api.example.test:443"}"#,
    )
    .unwrap();
    assert!(matches!(parsed, EgressMessageV1::Open { flow_id: 1, .. }));
}

#[test]
fn egress_protocol_bounds_canonical_binary_payload_before_decode() {
    let binary = [0_u8, 255, 0, 17];
    let message = EgressMessageV1::data(1, &binary).unwrap();
    let encoded = message.encode_payload().unwrap();
    assert_eq!(message.decoded_data().unwrap().unwrap(), binary);
    assert_eq!(
        EgressMessageV1::decode_payload(&encoded)
            .unwrap()
            .decoded_data()
            .unwrap()
            .unwrap(),
        binary
    );
    for value in ["", "AA", "AB==", "AA===", " AA==", "_w=="] {
        let raw = format!(r#"{{"operation":"data","flow_id":1,"data_b64":"{value}"}}"#);
        assert!(EgressMessageV1::decode_payload(raw.as_bytes()).is_err());
    }
    assert!(EgressMessageV1::data(1, &vec![1; 16_384]).is_ok());
    assert!(EgressMessageV1::data(1, &vec![1; 16_385]).is_err());
    assert!(EgressMessageV1::decode_payload(&vec![b' '; 24_577]).is_err());
    let oversized = format!(
        r#"{{"operation":"data","flow_id":1,"data_b64":"{}"}}"#,
        "A".repeat(21_852)
    );
    assert!(EgressMessageV1::decode_payload(oversized.as_bytes()).is_err());
}

#[test]
fn egress_protocol_worker_cannot_accept_or_send_before_broker_acceptance() {
    let mut forged = ready();
    forged.on_worker(&open(1), 0).unwrap();
    assert!(
        forged
            .on_worker(&EgressMessageV1::Accepted { flow_id: 1 }, 1)
            .is_err()
    );
    let mut early = ready();
    early.on_worker(&open(1), 0).unwrap();
    assert!(
        early
            .on_worker(&EgressMessageV1::data(1, b"early").unwrap(), 1)
            .is_err()
    );
    assert!(early.is_closed());
    assert_eq!(early.flow_count(), 0);
}

#[test]
fn egress_protocol_credit_is_per_direction_and_reserved_until_forwarded() {
    let mut state = ready();
    accepted(&mut state, 1, 0);
    state
        .on_broker(
            &EgressMessageV1::Credit {
                flow_id: 1,
                bytes: 4,
            },
            1,
        )
        .unwrap();
    state
        .on_worker(&EgressMessageV1::data(1, b"1234").unwrap(), 2)
        .unwrap();
    assert_eq!(state.buffered_bytes(), 4);
    state
        .release_buffer(1, DirectionV1::WorkerToBroker, 4)
        .unwrap();
    assert_eq!(state.buffered_bytes(), 0);
    assert!(
        state
            .on_broker(&EgressMessageV1::data(1, b"x").unwrap(), 3)
            .is_err()
    );
    let mut total = ready();
    accepted(&mut total, 1, 0);
    accepted(&mut total, 2, 0);
    total
        .on_broker(
            &EgressMessageV1::Credit {
                flow_id: 1,
                bytes: 262_144,
            },
            1,
        )
        .unwrap();
    assert!(
        total
            .on_broker(
                &EgressMessageV1::Credit {
                    flow_id: 2,
                    bytes: 1
                },
                1
            )
            .is_err()
    );
}

#[test]
fn egress_protocol_buffered_data_does_not_replenish_credit_or_survive_reset() {
    let mut state = ready();
    accepted(&mut state, 1, 0);
    state
        .on_broker(
            &EgressMessageV1::Credit {
                flow_id: 1,
                bytes: 262_144,
            },
            1,
        )
        .unwrap();
    state
        .on_worker(&EgressMessageV1::data(1, &[7; 16_384]).unwrap(), 2)
        .unwrap();
    assert!(
        state
            .on_broker(
                &EgressMessageV1::Credit {
                    flow_id: 1,
                    bytes: 1
                },
                3
            )
            .is_err()
    );
    let mut reset = ready();
    accepted(&mut reset, 1, 0);
    reset
        .on_broker(
            &EgressMessageV1::Credit {
                flow_id: 1,
                bytes: 4,
            },
            1,
        )
        .unwrap();
    reset
        .on_worker(&EgressMessageV1::data(1, b"1234").unwrap(), 2)
        .unwrap();
    reset
        .on_broker(
            &EgressMessageV1::Reset {
                flow_id: 1,
                reason: EgressReasonV1::Revoked,
            },
            3,
        )
        .unwrap();
    assert_eq!((reset.flow_count(), reset.buffered_bytes()), (0, 0));
    assert!(reset.on_worker(&open(1), 4).is_err());
}

#[test]
fn egress_protocol_half_close_is_per_flow_and_keeps_the_other_direction_open() {
    let mut state = ready();
    accepted(&mut state, 1, 0);
    accepted(&mut state, 2, 0);
    state
        .on_worker(&EgressMessageV1::HalfClose { flow_id: 1 }, 1)
        .unwrap();
    assert!(!state.is_closed());
    state
        .on_worker(
            &EgressMessageV1::Credit {
                flow_id: 1,
                bytes: 4,
            },
            2,
        )
        .unwrap();
    state
        .on_broker(&EgressMessageV1::data(1, b"reply").unwrap(), 3)
        .unwrap_err();
    let mut valid = ready();
    accepted(&mut valid, 1, 0);
    valid
        .on_worker(&EgressMessageV1::HalfClose { flow_id: 1 }, 1)
        .unwrap();
    valid
        .on_worker(
            &EgressMessageV1::Credit {
                flow_id: 1,
                bytes: 5,
            },
            2,
        )
        .unwrap();
    valid
        .on_broker(&EgressMessageV1::data(1, b"reply").unwrap(), 3)
        .unwrap();
    valid
        .release_buffer(1, DirectionV1::BrokerToWorker, 5)
        .unwrap();
    valid
        .on_broker(&EgressMessageV1::HalfClose { flow_id: 1 }, 4)
        .unwrap();
    assert!(!valid.is_closed());
    valid.close();
    assert_eq!(valid.flow_count(), 0);
    assert!(valid.is_closed());
}

#[test]
fn egress_protocol_limits_pending_live_and_open_rate_without_unbounded_replay_state() {
    let mut pending = ready();
    for flow in 1..=4 {
        pending.on_worker(&open(flow), 0).unwrap();
    }
    assert!(pending.on_worker(&open(5), 0).is_err());
    let mut rate = ready();
    for flow in 1..=8 {
        rate.on_worker(&open(flow), 0).unwrap();
        rate.on_broker(
            &EgressMessageV1::Rejected {
                flow_id: flow,
                reason: EgressReasonV1::Denied,
            },
            0,
        )
        .unwrap();
    }
    assert!(rate.on_worker(&open(9), 0).is_err());
    let mut live = ready();
    for flow in 1..=16 {
        accepted(&mut live, flow, flow * 250);
    }
    assert!(live.on_worker(&open(17), 4_250).is_err());
}

#[test]
fn egress_protocol_deadlines_unknown_flows_and_clock_rollback_close_the_epoch() {
    let mut pending = ready();
    pending.on_worker(&open(1), 0).unwrap();
    assert_eq!(
        pending.check_deadlines(10_000),
        Err(EgressProtocolError::Timeout)
    );
    let mut idle = ready();
    accepted(&mut idle, 1, 0);
    assert_eq!(
        idle.check_deadlines(60_000),
        Err(EgressProtocolError::Timeout)
    );
    let mut unknown = ready();
    assert!(
        unknown
            .on_broker(&EgressMessageV1::Accepted { flow_id: 7 }, 1)
            .is_err()
    );
    let mut rollback = ready();
    accepted(&mut rollback, 1, 10);
    assert_eq!(rollback.check_deadlines(9), Err(EgressProtocolError::Clock));
    assert!(rollback.is_closed());
}

#[test]
fn egress_protocol_half_close_retires_unused_credit_but_preserves_queued_bytes() {
    let mut empty = ready();
    accepted(&mut empty, 1, 0);
    accepted(&mut empty, 2, 0);
    empty
        .on_broker(
            &EgressMessageV1::Credit {
                flow_id: 1,
                bytes: 262_144,
            },
            1,
        )
        .unwrap();
    empty
        .on_worker(&EgressMessageV1::HalfClose { flow_id: 1 }, 2)
        .unwrap();
    empty
        .on_broker(
            &EgressMessageV1::Credit {
                flow_id: 2,
                bytes: 262_144,
            },
            3,
        )
        .unwrap();
    assert!(!empty.is_closed());
    empty
        .on_worker(
            &EgressMessageV1::Credit {
                flow_id: 1,
                bytes: 5,
            },
            4,
        )
        .unwrap();
    empty
        .on_broker(&EgressMessageV1::data(1, b"reply").unwrap(), 5)
        .unwrap();
    empty
        .release_buffer(1, DirectionV1::BrokerToWorker, 5)
        .unwrap();

    let queued = || {
        let mut state = ready();
        accepted(&mut state, 1, 0);
        accepted(&mut state, 2, 0);
        state
            .on_broker(
                &EgressMessageV1::Credit {
                    flow_id: 1,
                    bytes: 262_144,
                },
                1,
            )
            .unwrap();
        state
            .on_worker(&EgressMessageV1::data(1, b"1234").unwrap(), 2)
            .unwrap();
        state
            .on_worker(&EgressMessageV1::HalfClose { flow_id: 1 }, 3)
            .unwrap();
        state
    };
    let mut blocked = queued();
    assert_eq!(blocked.buffered_bytes(), 4);
    assert_eq!(
        blocked.on_broker(
            &EgressMessageV1::Credit {
                flow_id: 2,
                bytes: 262_141
            },
            4
        ),
        Err(EgressProtocolError::Credit)
    );
    let mut drained = queued();
    drained
        .on_broker(
            &EgressMessageV1::Credit {
                flow_id: 2,
                bytes: 262_140,
            },
            4,
        )
        .unwrap();
    assert_eq!(drained.buffered_bytes(), 4);
    drained
        .release_buffer(1, DirectionV1::WorkerToBroker, 4)
        .unwrap();
    assert_eq!(drained.buffered_bytes(), 0);
    drained
        .on_broker(
            &EgressMessageV1::Credit {
                flow_id: 2,
                bytes: 4,
            },
            5,
        )
        .unwrap();
    assert!(!drained.is_closed());
    assert_eq!(drained.flow_count(), 2);
}

#[test]
fn egress_protocol_control_size_precedes_unknown_or_escaped_dto_decoding() {
    // Each is a control frame above2048 but below24576, with no variable-data
    // permission. Size must win before the unknown/escaped DTO is decoded.
    let escaped = "\\u0061".repeat(400);
    let plain = "a".repeat(2_100);
    let fixtures = [
        format!(r#"{{"operation":"accepted","flow_id":1,"junk":"{escaped}"}}"#),
        format!(r#"{{"junk":"{escaped}","flow_id":1,"operation":"accepted"}}"#),
        format!(r#"{{"operation":"accepted","flow_id":1,"junk":"{plain}"}}"#),
    ];
    for input in fixtures {
        assert!((2_049..=24_576).contains(&input.len()));
        assert_eq!(
            EgressMessageV1::decode_payload(input.as_bytes()).err(),
            Some(EgressProtocolError::Size)
        );
    }
}
