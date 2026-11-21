use super::*;
use crate::egress::ids::{CHANNEL_CONTRACT, DigestV1, IdV1};
use crate::egress::{
    channel_protocol::{CommonEgressHandshakeV1, read_handshake, write_handshake, write_message},
    protocol::EgressCapabilityV1,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

fn hello() -> CommonEgressHandshakeV1 {
    CommonEgressHandshakeV1 {
        version: 1,
        protocol: CHANNEL_CONTRACT.into(),
        claim_id: IdV1::parse("00000000-0000-4000-8000-000000000001").unwrap(),
        binding_id: IdV1::parse("00000000-0000-4000-8000-000000000002").unwrap(),
        transport_epoch: 1,
        context_digest: DigestV1::of(b"synthetic helper context"),
        challenge: IdV1::parse("00000000-0000-4000-8000-000000000003").unwrap(),
        helper_version: 1,
        capability: EgressCapabilityV1::default(),
    }
}
fn args(raw: &[&str]) -> Vec<OsString> {
    raw.iter().map(OsString::from).collect()
}

#[test]
fn mode_selection_is_explicit_closed_and_keeps_relay_v1_separate() {
    assert_eq!(
        parse_mode(args(&["serve-v1"])).unwrap(),
        HelperMode::RelayV1
    );
    assert_eq!(
        parse_mode(args(&["serve-egress-v1", "--port", "3128"])).unwrap(),
        HelperMode::EgressV1(LoopbackConfig::new(3128).unwrap())
    );
    for raw in [
        vec![],
        vec!["serve-v2"],
        vec!["serve-v1", "extra"],
        vec!["serve-egress-v1"],
        vec!["serve-egress-v1", "--port", "0"],
        vec!["serve-egress-v1", "--port", "03128"],
        vec!["serve-egress-v1", "--port", "+3128"],
        vec!["serve-egress-v1", "--port", "65536"],
        vec!["serve-egress-v1", "--port", "127.0.0.1:3128"],
        vec!["serve-egress-v1", "--port", "3128", "extra"],
    ] {
        assert_eq!(
            parse_mode(args(&raw)),
            Err(CommonEgressHelperError::Protocol)
        );
    }
    use std::os::unix::ffi::OsStringExt;
    assert_eq!(
        parse_mode(vec![
            OsString::from("serve-egress-v1"),
            OsString::from("--port"),
            OsString::from_vec(vec![0xff])
        ]),
        Err(CommonEgressHelperError::Protocol)
    );
}

async fn fixture() -> (
    tokio::task::JoinHandle<Result<()>>,
    tokio::io::DuplexStream,
    tokio::io::DuplexStream,
    std::net::SocketAddr,
) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let (mut broker_output, helper_input) = duplex(65_536);
    let (helper_output, mut broker_input) = duplex(65_536);
    let expected = hello();
    let received = expected.clone();
    let task = tokio::spawn(async move {
        let started = Instant::now();
        let deadline = started + HANDSHAKE_TIME;
        let mut input = helper_input;
        let hello = read_handshake(&mut input, deadline).await?;
        run(hello, started, deadline, listener, input, helper_output).await
    });
    write_handshake(
        &mut broker_output,
        &expected,
        Instant::now() + HANDSHAKE_TIME,
    )
    .await
    .unwrap();
    assert_eq!(
        read_handshake(&mut broker_input, Instant::now() + HANDSHAKE_TIME)
            .await
            .unwrap(),
        received
    );
    (task, broker_output, broker_input, address)
}

#[tokio::test]
async fn real_loopback_tunnel_waits_for_broker_connected_and_credit_then_closes_on_pipe_eof() {
    let (task, mut output, mut input, address) = fixture().await;
    let mut client = TcpStream::connect(address).await.unwrap();
    client
        .write_all(b"CONNECT API.EXAMPLE.TEST:443 HTTP/1.1\r\nHost: API.EXAMPLE.TEST:443\r\n\r\n")
        .await
        .unwrap();
    let mut reader = EgressFrameReader::new();
    let open = reader.read(&mut input).await.unwrap().unwrap();
    assert!(
        matches!(open, EgressMessageV1::Open { flow_id: 1, ref authority } if authority == "api.example.test:443")
    );
    let mut probe = [0_u8; 1];
    assert!(
        timeout_at(
            Instant::now() + Duration::from_millis(20),
            client.read(&mut probe)
        )
        .await
        .is_err()
    );
    write_message(
        &mut output,
        &EgressMessageV1::Accepted { flow_id: 1 },
        Instant::now() + FRAME_TIME,
    )
    .await
    .unwrap();
    let mut success = [0_u8; 39];
    client.read_exact(&mut success).await.unwrap();
    assert_eq!(&success, b"HTTP/1.1 200 Connection Established\r\n\r\n");
    assert!(matches!(
        reader.read(&mut input).await.unwrap().unwrap(),
        EgressMessageV1::Credit {
            flow_id: 1,
            bytes: 16_384
        }
    ));
    client.write_all(b"opaque client").await.unwrap();
    assert!(
        timeout_at(
            Instant::now() + Duration::from_millis(20),
            reader.read(&mut input)
        )
        .await
        .is_err()
    );
    write_message(
        &mut output,
        &EgressMessageV1::Credit {
            flow_id: 1,
            bytes: 16_384,
        },
        Instant::now() + FRAME_TIME,
    )
    .await
    .unwrap();
    assert_eq!(
        reader
            .read(&mut input)
            .await
            .unwrap()
            .unwrap()
            .decoded_data()
            .unwrap()
            .unwrap(),
        b"opaque client"
    );
    write_message(
        &mut output,
        &EgressMessageV1::data(1, b"opaque broker").unwrap(),
        Instant::now() + FRAME_TIME,
    )
    .await
    .unwrap();
    let mut reply = [0_u8; 13];
    client.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"opaque broker");
    assert!(matches!(
        reader.read(&mut input).await.unwrap().unwrap(),
        EgressMessageV1::Credit {
            flow_id: 1,
            bytes: 13
        }
    ));
    drop(output);
    assert!(
        timeout_at(Instant::now() + Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .is_ok()
    );
    assert_eq!(client.read(&mut probe).await.unwrap(), 0);
    assert!(TcpStream::connect(address).await.is_err());
}

#[tokio::test]
async fn premature_client_data_is_cancelled_without_a_data_frame() {
    let (task, mut output, mut input, address) = fixture().await;
    let mut client = TcpStream::connect(address).await.unwrap();
    client
        .write_all(b"CONNECT api.example.test:443 HTTP/1.1\r\nHost: api.example.test:443\r\n\r\n")
        .await
        .unwrap();
    let mut reader = EgressFrameReader::new();
    assert!(matches!(
        reader.read(&mut input).await.unwrap().unwrap(),
        EgressMessageV1::Open { flow_id: 1, .. }
    ));
    client.write_all(b"premature").await.unwrap();
    assert!(matches!(
        reader.read(&mut input).await.unwrap().unwrap(),
        EgressMessageV1::Reset {
            flow_id: 1,
            reason: EgressReasonV1::Cancelled
        }
    ));
    // A late acceptance of the cancelled ID is a channel protocol failure.
    write_message(
        &mut output,
        &EgressMessageV1::Accepted { flow_id: 1 },
        Instant::now() + FRAME_TIME,
    )
    .await
    .unwrap();
    assert_eq!(task.await.unwrap(), Err(CommonEgressHelperError::Protocol));
}

#[tokio::test]
async fn broker_data_before_connected_closes_every_real_client() {
    let (task, mut output, mut input, address) = fixture().await;
    let mut client = TcpStream::connect(address).await.unwrap();
    client
        .write_all(b"CONNECT api.example.test:443 HTTP/1.1\r\nHost: api.example.test:443\r\n\r\n")
        .await
        .unwrap();
    let mut reader = EgressFrameReader::new();
    assert!(matches!(
        reader.read(&mut input).await.unwrap().unwrap(),
        EgressMessageV1::Open { flow_id: 1, .. }
    ));
    write_message(
        &mut output,
        &EgressMessageV1::data(1, b"forbidden early bytes").unwrap(),
        Instant::now() + FRAME_TIME,
    )
    .await
    .unwrap();
    assert_eq!(task.await.unwrap(), Err(CommonEgressHelperError::Protocol));
    let mut probe = [0_u8; 1];
    assert!(matches!(client.read(&mut probe).await, Ok(0) | Err(_)));
}
