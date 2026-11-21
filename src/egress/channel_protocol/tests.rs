use super::*;
use tokio::io::{AsyncWriteExt, duplex};

fn handshake() -> CommonEgressHandshakeV1 {
    CommonEgressHandshakeV1 {
        version: 1,
        protocol: CHANNEL_CONTRACT.into(),
        claim_id: IdV1::parse("00000000-0000-4000-8000-000000000001").unwrap(),
        binding_id: IdV1::parse("00000000-0000-4000-8000-000000000002").unwrap(),
        transport_epoch: 1,
        context_digest: DigestV1::of(b"synthetic context"),
        challenge: IdV1::parse("00000000-0000-4000-8000-000000000003").unwrap(),
        helper_version: 1,
        capability: EgressCapabilityV1::default(),
    }
}

#[tokio::test]
async fn fragmented_handshake_roundtrip_preserves_exact_echo() {
    let expected = handshake();
    let bytes = serde_json::to_vec(&expected).unwrap();
    let (mut sender, mut receiver) = duplex(1);
    let send = async {
        for byte in (bytes.len() as u32)
            .to_be_bytes()
            .iter()
            .chain(bytes.iter())
        {
            sender.write_all(&[*byte]).await.unwrap();
        }
    };
    let receive = read_handshake(&mut receiver, Instant::now() + HANDSHAKE_TIME);
    let (_, decoded) = tokio::join!(send, receive);
    assert_eq!(decoded.unwrap(), expected);
}

#[tokio::test]
async fn handshake_writer_is_an_exact_ready_echo() {
    let expected = handshake();
    let (mut sender, mut receiver) = duplex(1);
    let deadline = Instant::now() + HANDSHAKE_TIME;
    let (written, ready) = tokio::join!(
        write_handshake(&mut sender, &expected, deadline),
        read_handshake(&mut receiver, deadline)
    );
    assert_eq!(written, Ok(()));
    assert_eq!(ready.unwrap(), expected);
}

#[tokio::test]
async fn partial_message_reader_survives_cancelled_wait() {
    let message = EgressMessageV1::Accepted { flow_id: 1 };
    let bytes = message.encode_payload().unwrap();
    let prefix = (bytes.len() as u32).to_be_bytes();
    let (mut sender, mut receiver) = duplex(4096);
    let mut reader = EgressFrameReader::new();
    sender.write_all(&prefix[..2]).await.unwrap();
    assert!(
        timeout_at(
            Instant::now() + Duration::from_millis(10),
            reader.read(&mut receiver)
        )
        .await
        .is_err()
    );
    sender.write_all(&prefix[2..]).await.unwrap();
    sender.write_all(&bytes).await.unwrap();
    assert!(matches!(
        reader.read(&mut receiver).await.unwrap().unwrap(),
        EgressMessageV1::Accepted { flow_id: 1 }
    ));
}

#[tokio::test]
async fn fragmented_handshake_keeps_its_original_deadline() {
    let bytes = serde_json::to_vec(&handshake()).unwrap();
    let (mut sender, mut receiver) = duplex(4096);
    sender
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await
        .unwrap();
    sender.write_all(&bytes[..bytes.len() - 3]).await.unwrap();
    let send = async {
        for byte in &bytes[bytes.len() - 3..] {
            tokio::time::sleep(Duration::from_millis(8)).await;
            let _ = sender.write_all(&[*byte]).await;
        }
    };
    let receive = read_handshake(&mut receiver, Instant::now() + Duration::from_millis(15));
    let (_, result) = tokio::join!(send, receive);
    assert_eq!(result, Err(CommonEgressChannelError::Timeout));
}

#[tokio::test]
async fn oversized_handshake_fails_without_waiting_for_body() {
    let (mut sender, mut receiver) = duplex(4);
    sender.write_all(&4097_u32.to_be_bytes()).await.unwrap();
    assert_eq!(
        read_handshake(&mut receiver, Instant::now() + HANDSHAKE_TIME).await,
        Err(CommonEgressChannelError::Size)
    );
}

#[tokio::test]
async fn handshake_unknown_fields_and_changed_capability_are_rejected() {
    let mut value = serde_json::to_value(handshake()).unwrap();
    value["extra"] = true.into();
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut framed = (bytes.len() as u32).to_be_bytes().to_vec();
    framed.extend_from_slice(&bytes);
    assert_eq!(
        read_handshake(&mut &framed[..], Instant::now() + HANDSHAKE_TIME).await,
        Err(CommonEgressChannelError::Protocol)
    );
    let mut wrong = handshake();
    wrong.capability.limits.live_flows += 1;
    assert_eq!(wrong.validate(), Err(CommonEgressChannelError::Protocol));
}

#[tokio::test]
async fn expired_absolute_handshake_deadline_is_not_renewed() {
    let mut input = &b"\0\0\0\x01{}"[..];
    assert_eq!(
        read_handshake(&mut input, Instant::now()).await,
        Err(CommonEgressChannelError::Timeout)
    );
}

#[tokio::test]
async fn message_size_partial_eof_and_unknown_operation_are_closed() {
    let mut reader = EgressFrameReader::new();
    assert!(matches!(
        reader.read(&mut &24577_u32.to_be_bytes()[..]).await,
        Err(CommonEgressChannelError::Size)
    ));
    assert!(matches!(
        reader.read(&mut &b""[..]).await,
        Err(CommonEgressChannelError::Closed)
    ));
    let mut reader = EgressFrameReader::new();
    assert!(matches!(
        reader.read(&mut &b"\0\0"[..]).await,
        Err(CommonEgressChannelError::PartialFrame)
    ));
    let bytes = br#"{"operation":"connected","flow_id":1}"#;
    let mut framed = (bytes.len() as u32).to_be_bytes().to_vec();
    framed.extend_from_slice(bytes);
    let mut reader = EgressFrameReader::new();
    assert!(matches!(
        reader.read(&mut &framed[..]).await,
        Err(CommonEgressChannelError::Protocol)
    ));
}
