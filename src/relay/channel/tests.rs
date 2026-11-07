use super::*;
use crate::relay::{RelayAckV1, RelayEndpointV1, RelayInboxV1, RelayReceiptV1, RelaySendV1};
use std::io::{Cursor, Write};

const CHANNEL: &str = "10000000-0000-4000-8000-000000000001";
const CORRELATION: &str = "20000000-0000-4000-8000-000000000001";
const OPERATION: &str = "30000000-0000-4000-8000-000000000001";
const GRANT: &str = "40000000-0000-4000-8000-000000000001";
const MESSAGE: &str = "50000000-0000-4000-8000-000000000001";

fn handshake() -> HandshakeV1 {
    HandshakeV1 {
        version: 1,
        channel_id: CHANNEL.into(),
        epoch: 7,
        limits: ChannelLimitsV1::default(),
        helper_version: 1,
    }
}

fn endpoint() -> RelayEndpointV1 {
    RelayEndpointV1 {
        worker_id: "60000000-0000-4000-8000-000000000001".into(),
        attempt_id: "70000000-0000-4000-8000-000000000001".into(),
        generation: 1,
    }
}

fn request(payload: RequestPayloadV1) -> RequestV1 {
    RequestV1 {
        version: 1,
        channel_id: CHANNEL.into(),
        epoch: 7,
        correlation_id: CORRELATION.into(),
        payload,
    }
}

fn send() -> RequestV1 {
    request(RequestPayloadV1::Send(RelaySendV1 {
        version: 1,
        operation_id: OPERATION.into(),
        grant_id: GRANT.into(),
        ttl_seconds: 30,
        body: "hello\nworld".into(),
    }))
}

fn inbox() -> RequestV1 {
    request(RequestPayloadV1::Inbox(InboxRequestV1 {
        after: None,
        limit: 1,
    }))
}

fn receipt(operation: OperationV1, resource: &str) -> ResponseV1 {
    let receipt = RelayReceiptV1 {
        version: 1,
        operation_id: OPERATION.into(),
        resource_id: resource.into(),
        audit_seq: 9,
        audit_hash: "a".repeat(64),
        replayed: false,
    };
    ResponseV1 {
        version: 1,
        channel_id: CHANNEL.into(),
        epoch: 7,
        correlation_id: CORRELATION.into(),
        operation,
        result: ResponseResultV1::Ok(match operation {
            OperationV1::Send => ReplyV1::Send(receipt),
            OperationV1::Ack => ReplyV1::Ack(receipt),
            OperationV1::Inbox => panic!("receipt fixture only supports mutations"),
        }),
    }
}

fn empty_inbox(correlation: String) -> ResponseV1 {
    ResponseV1 {
        version: 1,
        channel_id: CHANNEL.into(),
        epoch: 7,
        correlation_id: correlation,
        operation: OperationV1::Inbox,
        result: ResponseResultV1::Ok(ReplyV1::Inbox(RelayInboxV1 {
            version: 1,
            items: vec![],
            next_after: None,
        })),
    }
}

fn open() -> ChannelState {
    let mut state = ChannelState::new(handshake(), endpoint()).unwrap();
    state.accept_ready(&handshake()).unwrap();
    state
}

fn framed(json: &[u8]) -> Vec<u8> {
    let mut bytes = (json.len() as u32).to_be_bytes().to_vec();
    bytes.extend_from_slice(json);
    bytes
}

fn read(bytes: Vec<u8>, limit: FrameLimit) -> Result<Option<FrameV1>, ChannelError> {
    FrameReader::new(Cursor::new(bytes)).read(limit)
}

#[test]
fn codec_uses_length_prefix_and_preserves_embedded_newlines() {
    let raw = format!(
        r#"{{"version":1,"type":"request","channel_id":"{CHANNEL}","epoch":7,"correlation_id":"{CORRELATION}","operation":"send","payload":{{"version":1,"operation_id":"{OPERATION}","grant_id":"{GRANT}","ttl_seconds":30,"body":"hello\nworld"}}}}"#
    );
    let frame = read(framed(raw.as_bytes()), FrameLimit::Request)
        .unwrap()
        .unwrap();
    let FrameV1::Request(decoded) = &frame else {
        panic!("expected request")
    };
    let RequestPayloadV1::Send(payload) = &decoded.payload else {
        panic!("expected send")
    };
    assert_eq!(payload.body, "hello\nworld");
    let mut writer = FrameWriter::new(Vec::new());
    writer.write(&frame).unwrap();
    let encoded = writer.into_inner();
    assert_eq!(
        u32::from_be_bytes(encoded[..4].try_into().unwrap()) as usize,
        encoded.len() - 4
    );
    assert!(read(encoded, FrameLimit::Request).unwrap().is_some());
}

#[test]
fn oversized_header_is_rejected_before_payload_is_read() {
    let mut bytes = 278_529_u32.to_be_bytes().to_vec();
    bytes.extend_from_slice(b"unread");
    let mut reader = FrameReader::new(Cursor::new(bytes));
    assert!(matches!(
        reader.read(FrameLimit::Request),
        Err(ChannelError::FrameSize)
    ));
    assert_eq!(reader.into_inner().position(), 4);
}

#[test]
fn invalid_frame_poisoning_prevents_resynchronization_or_second_dispatch() {
    for bad in [
        vec![0, 0, 0, 0],
        vec![0, 0, 1],
        vec![0, 0, 0, 3, b'{'],
        framed(&[0xff]),
        framed(b"{}"),
    ] {
        let mut reader = FrameReader::new(Cursor::new(bad));
        assert!(reader.read(FrameLimit::Request).is_err());
        assert!(matches!(
            reader.read(FrameLimit::Request),
            Err(ChannelError::Closed)
        ));
    }
    let mut reader = FrameReader::new(Cursor::new(Vec::<u8>::new()));
    assert!(reader.read(FrameLimit::Request).unwrap().is_none());
    assert!(matches!(
        reader.read(FrameLimit::Request),
        Err(ChannelError::Closed)
    ));
}

#[test]
fn recursive_duplicates_depth_and_unknown_authority_fields_are_refused() {
    let valid = format!(
        r#"{{"version":1,"type":"request","channel_id":"{CHANNEL}","epoch":7,"correlation_id":"{CORRELATION}","operation":"inbox","payload":{{"after":null,"limit":1}}}}"#
    );
    let invalid = [
        valid.replacen("\"version\":1", "\"version\":1,\"version\":1", 1),
        valid.replace("\"limit\":1", "\"limit\":1,\"limit\":1"),
        valid.replace("\"limit\":1", "\"limit\":1,\"tenant\":\"A\""),
        valid.replace(
            "\"epoch\":7",
            "\"epoch\":7,\"url\":\"http://example.invalid\"",
        ),
        valid.replace("\"epoch\":7", "\"epoch\":7,\"credential\":\"private\""),
        valid.replace("\"epoch\":7", "\"epoch\":7,\"argv\":[\"sh\"]"),
        valid.replacen("\"version\":1", "\"version\":2", 1),
        valid.replace("\"inbox\"", "\"grant\""),
        valid.replace("\"after\":null,", ""),
        valid.replace(
            "\"limit\":1",
            &format!("\"limit\":1,\"deep\":{}0{}", "[".repeat(65), "]".repeat(65)),
        ),
    ];
    for json in invalid {
        assert!(read(framed(json.as_bytes()), FrameLimit::Request).is_err());
    }
}

#[test]
fn business_payload_cannot_borrow_envelope_headroom() {
    let prefix = format!(
        r#"{{"version":1,"type":"request","channel_id":"{CHANNEL}","epoch":7,"correlation_id":"{CORRELATION}","operation":"send","payload":"#
    );
    let payload = format!(
        r#"{{"version":1,"operation_id":"{OPERATION}","grant_id":"{GRANT}","ttl_seconds":30,"body":"ok"}}"#
    );
    let padding = " ".repeat(262_145 - payload.len());
    let raw = format!("{prefix}{{{padding}{} }}", &payload[1..]);
    assert!(raw.len() < 278_528);
    assert!(matches!(
        read(framed(raw.as_bytes()), FrameLimit::Request),
        Err(ChannelError::BusinessSize)
    ));
}

#[test]
fn receipts_and_inbox_payloads_keep_their_own_raw_encoded_limits() {
    for (operation, payload, maximum) in [
        (
            "send",
            format!(
                r#"{{"version":1,"operation_id":"{OPERATION}","resource_id":"{OPERATION}","audit_seq":9,"audit_hash":"{}","replayed":false}}"#,
                "a".repeat(64)
            ),
            16_384,
        ),
        (
            "inbox",
            r#"{"version":1,"items":[],"next_after":null}"#.into(),
            2_097_152,
        ),
    ] {
        let prefix = format!(
            r#"{{"version":1,"type":"response","channel_id":"{CHANNEL}","epoch":7,"correlation_id":"{CORRELATION}","operation":"{operation}","result":"ok","payload":"#
        );
        let padding = " ".repeat(maximum + 1 - payload.len());
        let raw = format!("{prefix}{{{padding}{} }}", &payload[1..]);
        assert!(raw.len() < 2_113_536);
        assert!(matches!(
            read(framed(raw.as_bytes()), FrameLimit::Response),
            Err(ChannelError::BusinessSize)
        ));
    }
}

#[test]
fn json_container_overhead_is_bounded_before_value_decoding() {
    let raw = format!(r#"{{"ignored":[{}]}}"#, "0,".repeat(16_385) + "0");
    assert!(raw.len() < 278_528);
    assert!(matches!(
        read(framed(raw.as_bytes()), FrameLimit::Request),
        Err(ChannelError::Capacity)
    ));
}

#[test]
fn body_text_does_not_consume_the_json_container_budget() {
    let mut input = send();
    let RequestPayloadV1::Send(payload) = &mut input.payload else {
        unreachable!()
    };
    payload.body = "[{:,".repeat(16_384);
    let mut writer = FrameWriter::new(Vec::new());
    writer.write(&FrameV1::Request(input)).unwrap();
    assert!(
        read(writer.into_inner(), FrameLimit::Request)
            .unwrap()
            .is_some()
    );
}

#[test]
fn request_bounds_are_validated_before_admission() {
    let mut state = open();
    let mut invalid = send();
    let RequestPayloadV1::Send(payload) = &mut invalid.payload else {
        unreachable!()
    };
    payload.body = "é".repeat(32_769);
    assert!(state.admit_request(&invalid).is_err());
    assert!(state.is_closed());

    for payload in [
        RequestPayloadV1::Inbox(InboxRequestV1 {
            after: Some(-1),
            limit: 1,
        }),
        RequestPayloadV1::Inbox(InboxRequestV1 {
            after: None,
            limit: 33,
        }),
        RequestPayloadV1::Ack(RelayAckV1 {
            version: 2,
            operation_id: OPERATION.into(),
            message_id: MESSAGE.into(),
        }),
        RequestPayloadV1::Send(RelaySendV1 {
            version: 1,
            operation_id: OPERATION.into(),
            grant_id: GRANT.into(),
            ttl_seconds: 0,
            body: "ok".into(),
        }),
        RequestPayloadV1::Send(RelaySendV1 {
            version: 1,
            operation_id: OPERATION.into(),
            grant_id: GRANT.into(),
            ttl_seconds: 1,
            body: r#"{"nested":{"x":1,"x":2}}"#.into(),
        }),
    ] {
        assert!(open().admit_request(&request(payload)).is_err());
    }
}

#[test]
fn ready_must_echo_exact_binding_and_limits_before_requests() {
    let mut state = ChannelState::new(handshake(), endpoint()).unwrap();
    assert!(state.admit_request(&inbox()).is_err());
    assert!(state.is_closed());
    let mut state = ChannelState::new(handshake(), endpoint()).unwrap();
    let mut ready = handshake();
    ready.epoch += 1;
    assert!(state.accept_ready(&ready).is_err());
    let mut state = ChannelState::new(handshake(), endpoint()).unwrap();
    let mut ready = handshake();
    ready.limits.request_frame_bytes -= 1;
    assert!(state.accept_ready(&ready).is_err());
}

#[test]
fn hello_and_ready_round_trip_the_closed_handshake() {
    for frame in [FrameV1::Hello(handshake()), FrameV1::Ready(handshake())] {
        let mut writer = FrameWriter::new(Vec::new());
        writer.write(&frame).unwrap();
        let decoded = read(writer.into_inner(), FrameLimit::Control)
            .unwrap()
            .unwrap();
        let handshake_value = match decoded {
            FrameV1::Hello(value) | FrameV1::Ready(value) => value,
            _ => panic!("expected handshake"),
        };
        assert_eq!(handshake_value, handshake());
    }
}

#[test]
fn channel_epoch_and_canonical_non_nil_correlations_are_enforced() {
    for invalid in [
        RequestV1 {
            epoch: 8,
            ..inbox()
        },
        RequestV1 {
            channel_id: OPERATION.into(),
            ..inbox()
        },
        RequestV1 {
            correlation_id: "00000000-0000-0000-0000-000000000000".into(),
            ..inbox()
        },
        RequestV1 {
            correlation_id: "{20000000-0000-4000-8000-000000000001}".into(),
            ..inbox()
        },
    ] {
        assert!(open().admit_request(&invalid).is_err());
    }
}

#[test]
fn close_frame_checks_binding_and_retains_any_unresolved_exchange() {
    let mut state = open();
    state.admit_request(&send()).unwrap();
    let close = CloseV1 {
        version: 1,
        channel_id: CHANNEL.into(),
        epoch: 7,
        reason: CloseReasonV1::Revoked,
    };
    state.accept_close(&close).unwrap();
    assert!(state.is_closed());
    assert!(state.has_pending_exchange());
    let mut state = open();
    assert!(state.accept_close(&CloseV1 { epoch: 8, ..close }).is_err());
    assert!(state.is_closed());
}

#[test]
fn no_second_inflight_or_reused_correlation_is_admitted() {
    let mut state = open();
    state.admit_request(&inbox()).unwrap();
    let second = RequestV1 {
        correlation_id: OPERATION.into(),
        ..inbox()
    };
    assert!(matches!(
        state.admit_request(&second),
        Err(ChannelError::Busy)
    ));
    assert!(state.is_closed());

    let mut state = open();
    state.admit_request(&inbox()).unwrap();
    state
        .accept_response(&empty_inbox(CORRELATION.into()))
        .unwrap();
    assert!(matches!(
        state.admit_request(&inbox()),
        Err(ChannelError::Correlation)
    ));
}

#[test]
fn pending_exchange_closes_admission_without_reading_a_second_object() {
    let mut state = open();
    state.admit_request(&send()).unwrap();
    let mut writer = FrameWriter::new(Vec::new());
    writer.write(&FrameV1::Request(inbox())).unwrap();
    let mut reader = FrameReader::new(Cursor::new(writer.into_inner()));
    assert!(matches!(
        state.read_request(&mut reader),
        Err(ChannelError::Busy)
    ));
    assert!(state.is_closed());
    assert!(state.has_pending_exchange());
    assert_eq!(reader.into_inner().position(), 0);
}

#[test]
fn combined_request_reader_closes_state_on_eof_or_bad_frame() {
    let mut state = open();
    let mut reader = FrameReader::new(Cursor::new(framed(b"{}")));
    assert!(state.read_request(&mut reader).is_err());
    assert!(state.is_closed());
    let mut state = open();
    let mut reader = FrameReader::new(Cursor::new(Vec::<u8>::new()));
    assert!(state.read_request(&mut reader).unwrap().is_none());
    assert!(state.is_closed());
    let mut state = open();
    let mut writer = FrameWriter::new(Vec::new());
    writer.write(&FrameV1::Request(inbox())).unwrap();
    let mut reader = FrameReader::new(Cursor::new(writer.into_inner()));
    assert!(state.read_request(&mut reader).unwrap().is_some());
    assert!(state.has_pending_exchange());
}

#[test]
fn writer_refuses_escaped_business_expansion_before_any_prefix_is_written() {
    let mut input = send();
    let RequestPayloadV1::Send(payload) = &mut input.payload else {
        unreachable!()
    };
    payload.body = "\0".repeat(65_536);
    let mut writer = FrameWriter::new(Vec::new());
    assert!(matches!(
        writer.write(&FrameV1::Request(input)),
        Err(ChannelError::BusinessSize)
    ));
    assert!(matches!(
        writer.write(&FrameV1::Request(inbox())),
        Err(ChannelError::Closed)
    ));
    assert!(writer.into_inner().is_empty());
}

#[test]
fn response_must_match_pending_operation_and_mutation_resource() {
    for invalid in [
        ResponseV1 {
            epoch: 8,
            ..receipt(OperationV1::Send, OPERATION)
        },
        ResponseV1 {
            correlation_id: OPERATION.into(),
            ..receipt(OperationV1::Send, OPERATION)
        },
        receipt(OperationV1::Ack, MESSAGE),
        receipt(OperationV1::Send, MESSAGE),
    ] {
        let mut state = open();
        state.admit_request(&send()).unwrap();
        assert!(state.accept_response(&invalid).is_err());
        assert!(state.is_closed());
        assert!(state.has_pending_exchange());
    }
    let mut state = open();
    assert!(
        state
            .accept_response(&receipt(OperationV1::Send, OPERATION))
            .is_err()
    );
    let mut state = open();
    state.admit_request(&send()).unwrap();
    state
        .accept_response(&receipt(OperationV1::Send, OPERATION))
        .unwrap();
    assert!(!state.has_pending_exchange());

    let mut state = open();
    state
        .admit_request(&request(RequestPayloadV1::Ack(RelayAckV1 {
            version: 1,
            operation_id: OPERATION.into(),
            message_id: MESSAGE.into(),
        })))
        .unwrap();
    state
        .accept_response(&receipt(OperationV1::Ack, MESSAGE))
        .unwrap();
}

#[test]
fn inbox_response_is_checked_against_the_retained_endpoint() {
    use crate::relay::{RelayMessageMetadataV1, RelayMessageV1};
    let make = |destination| {
        let mut response = empty_inbox(CORRELATION.into());
        response.result = ResponseResultV1::Ok(ReplyV1::Inbox(RelayInboxV1 {
            version: 1,
            items: vec![RelayMessageV1 {
                sequence: 1,
                metadata: RelayMessageMetadataV1 {
                    version: 1,
                    message_id: MESSAGE.into(),
                    grant_id: GRANT.into(),
                    source: RelayEndpointV1 {
                        worker_id: GRANT.into(),
                        ..endpoint()
                    },
                    destination,
                    channel: "work".into(),
                    created_at: "2026-11-07T10:00:00Z".into(),
                    expires_at: "2026-11-07T10:00:30Z".into(),
                    body_bytes: 5,
                    body_sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
                        .into(),
                    acknowledged: false,
                    body_pruned: false,
                },
                body: "hello".into(),
            }],
            next_after: Some(1),
        }));
        response
    };
    let mut state = open();
    state.admit_request(&inbox()).unwrap();
    state.accept_response(&make(endpoint())).unwrap();
    let mut state = open();
    state.admit_request(&inbox()).unwrap();
    let other = RelayEndpointV1 {
        worker_id: GRANT.into(),
        ..endpoint()
    };
    assert!(state.accept_response(&make(other)).is_err());
    assert!(state.is_closed());
    assert!(state.has_pending_exchange());
}

#[test]
fn correlation_budget_counts_completed_reads_and_allows_explicit_business_replay() {
    let mut state = open();
    for suffix in 1..=1024 {
        let correlation = format!("20000000-0000-4000-8000-{suffix:012x}");
        let current = RequestV1 {
            correlation_id: correlation.clone(),
            ..inbox()
        };
        state.admit_request(&current).unwrap();
        state.accept_response(&empty_inbox(correlation)).unwrap();
    }
    let fresh = RequestV1 {
        correlation_id: "20000000-0000-4000-8000-000000001000".into(),
        ..inbox()
    };
    assert!(matches!(
        state.admit_request(&fresh),
        Err(ChannelError::Capacity)
    ));
    assert!(state.is_closed());

    let mut state = open();
    state.admit_request(&send()).unwrap();
    state
        .accept_response(&receipt(OperationV1::Send, OPERATION))
        .unwrap();
    let replay = RequestV1 {
        correlation_id: GRANT.into(),
        ..send()
    };
    state.admit_request(&replay).unwrap();
}

#[test]
fn response_has_exactly_one_closed_result_and_never_an_arbitrary_error() {
    let raw = format!(
        r#"{{"version":1,"type":"response","channel_id":"{CHANNEL}","epoch":7,"correlation_id":"{CORRELATION}","operation":"send","result":"error","error":"forbidden"}}"#
    );
    assert!(
        read(framed(raw.as_bytes()), FrameLimit::Response)
            .unwrap()
            .is_some()
    );
    for invalid in [
        raw.replace("\"forbidden\"", "\"remote arbitrary text\""),
        raw.replace(
            "\"error\":\"forbidden\"",
            "\"error\":\"forbidden\",\"payload\":{}",
        ),
        raw.replace(
            "\"error\":\"forbidden\"",
            "\"error\":\"forbidden\",\"outcome_unknown\":false",
        ),
        format!("{}{}", raw, " ".repeat(16_384)),
    ] {
        assert!(read(framed(invalid.as_bytes()), FrameLimit::Response).is_err());
    }
}

#[cfg(unix)]
#[test]
fn fragmented_unix_stream_carries_a_complete_frame_without_line_delimiters() {
    use std::{os::unix::net::UnixStream, time::Duration};
    let (mut producer, consumer) = UnixStream::pair().unwrap();
    consumer
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    producer
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let raw = format!(
        r#"{{"version":1,"type":"close","channel_id":"{CHANNEL}","epoch":7,"reason":"shutdown"}}"#
    );
    let bytes = framed(raw.as_bytes());
    let task = std::thread::spawn(move || {
        for byte in bytes {
            producer.write_all(&[byte]).unwrap();
        }
    });
    let mut reader = FrameReader::new(consumer);
    assert!(matches!(
        reader.read(FrameLimit::Control).unwrap(),
        Some(FrameV1::Close(_))
    ));
    assert!(reader.read(FrameLimit::Control).unwrap().is_none());
    task.join().unwrap();
}
