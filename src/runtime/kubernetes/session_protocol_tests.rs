//! Protocol v3 codec, control messages, order and the broker channel.
//!
//! Real code over in-memory pipes (`tokio::io::duplex`); the worker side of
//! the channel tests is a script of hand-built frames. These tests qualify
//! no kubelet, API server, attach transport, worker image or provider.
use super::*;
use crate::{
    provider::{
        AuthMode, ChoiceSource, ClientKind, ClientTransport, ProviderChoiceProvenanceV1,
        ProviderResolutionV1,
    },
    runtime::kubernetes::{
        KubernetesAuthorityV1,
        session::SessionBootstrapBindingV2,
        session_profile::{
            KubernetesSessionBindingV2, KubernetesSessionIdsV2, KubernetesSessionInvocationV2,
            KubernetesSessionResourcesV2,
        },
    },
    workspace::{Limits, scan_workspace_archive},
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

/// Synthetic credential canary: never a real key.
pub(crate) const SECRET_CANARY: &str = "k3-private-canary-7c41e9d2b8a0";
pub(crate) const PROMPT_CANARY: &str = "k3 private prompt canary 5d0b";

// ---- shared fixture ---------------------------------------------------------

pub(crate) fn tar_of(files: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, bytes, mode) in files {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(bytes.len() as u64);
        header.set_mode(*mode);
        header.set_mtime(0);
        builder.append_data(&mut header, path, *bytes).unwrap();
    }
    builder.into_inner().unwrap()
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(crate) fn payload_json(secret: &str) -> String {
    serde_json::json!({"version":1,"client":"codex","auth_mode":"api","secret":secret}).to_string()
}

/// One valid profile, its Hello binding, its invocation and its archive.
pub(crate) struct ProfileFixture {
    pub(crate) profile: KubernetesSessionProfileV2,
    pub(crate) binding: SessionBootstrapBindingV2,
    pub(crate) args: Vec<String>,
    pub(crate) archive: Vec<u8>,
}
impl ProfileFixture {
    pub(crate) fn handoff(&self) -> HandoffV3 {
        self.handoff_with(&payload_json(SECRET_CANARY), &self.archive)
    }
    pub(crate) fn handoff_with(&self, payload: &str, archive: &[u8]) -> HandoffV3 {
        HandoffV3::new(
            &self.profile,
            payload,
            crate::provider::BOOTSTRAP_PROGRAM,
            &self.args,
            archive.len() as u64,
            &sha256_hex(archive),
        )
        .unwrap()
    }
}

pub(crate) fn arguments(transport: ClientTransport) -> Vec<String> {
    let transport = match transport {
        ClientTransport::Batch => "batch",
        ClientTransport::InteractiveTty => "interactive-tty",
    };
    [
        "exec",
        "--client",
        "codex",
        "--auth-mode",
        "api",
        "--transport",
        transport,
        "--preset-version",
        "1",
        "--prompt",
        PROMPT_CANARY,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

pub(crate) fn fixture(archive: Vec<u8>) -> ProfileFixture {
    fixture_with(archive, ClientTransport::Batch)
}

pub(crate) fn fixture_with(archive: Vec<u8>, transport: ClientTransport) -> ProfileFixture {
    let authority = KubernetesAuthorityV1::new(
        Uuid::from_u128(1),
        "https://127.0.0.1:6443",
        "11111111-1111-4111-8111-111111111111",
        "tenant-a",
        "22222222-2222-4222-8222-222222222222",
        &"a".repeat(64),
    )
    .unwrap();
    let ids = || KubernetesSessionIdsV2 {
        installation_id: Uuid::from_u128(10),
        control_namespace_uuid: Uuid::from_u128(3),
        broker_id: Uuid::from_u128(4),
        incarnation_id: Uuid::from_u128(5),
        session_id: Uuid::from_u128(7),
        attempt_id: Uuid::from_u128(8),
    };
    let binding = KubernetesSessionBindingV2::new(&authority, ids()).unwrap();
    let provider = ProviderResolutionV1 {
        version: 1,
        client: ClientKind::Codex,
        auth_mode: AuthMode::Api,
        transport,
        profile_id: "default".into(),
        profile_revision: 1,
        model: None,
        credential_binding_id: Uuid::from_u128(9),
        credential_names: vec![crate::provider::BOOTSTRAP_ENV.into()],
        invocation_preset_version: 1,
        provenance: ProviderChoiceProvenanceV1 {
            client: ChoiceSource::Cli,
            auth_mode: ChoiceSource::Cli,
            profile: ChoiceSource::Cli,
            model: ChoiceSource::Cli,
        },
    };
    let args = arguments(transport);
    let manifest = scan_workspace_archive(archive.as_slice(), Limits::default()).unwrap();
    let profile = KubernetesSessionProfileV2::build(
        &authority,
        &binding,
        &format!("local/worker@sha256:{}", "b".repeat(64)),
        KubernetesSessionResourcesV2 {
            cpu_millis: 2000,
            memory_mib: 1024,
            temporary_mib: 64,
            home_mib: 64,
            deadline_seconds: 60,
        },
        &provider,
        KubernetesSessionInvocationV2::new(crate::provider::BOOTSTRAP_PROGRAM, &args).unwrap(),
        &manifest,
    )
    .unwrap();
    let ids = ids();
    let hello = SessionBootstrapBindingV2 {
        version: 2,
        installation_id: ids.installation_id,
        control_namespace_uuid: ids.control_namespace_uuid,
        broker_id: ids.broker_id,
        incarnation_id: ids.incarnation_id,
        session_id: ids.session_id,
        attempt_id: ids.attempt_id,
        reservation_id: Uuid::from_u128(11).to_string(),
        reservation_input_hash: "c".repeat(64),
        authority_fingerprint: authority.fingerprint(),
        profile_hash: profile.profile_hash().unwrap(),
        preparation_sha256: "d".repeat(64),
    };
    ProfileFixture {
        profile,
        binding: hello,
        args,
        archive,
    }
}

pub(crate) fn default_archive() -> Vec<u8> {
    tar_of(&[
        ("README.md", b"hello\n", 0o644),
        ("bin/tool", b"#!x\n", 0o755),
    ])
}

fn frame_bytes(channel: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = u32::try_from(payload.len() + 1)
        .unwrap()
        .to_be_bytes()
        .to_vec();
    frame.push(channel);
    frame.extend_from_slice(payload);
    frame
}

async fn read_one(bytes: &[u8], limit: usize) -> Result<Option<FrameV3>> {
    let mut input = bytes;
    FrameReaderV3::new().read(&mut input, limit).await
}

// ---- codec ------------------------------------------------------------------

#[tokio::test]
async fn frames_round_trip_on_every_channel_and_end_cleanly_between_frames() {
    let channels = [
        ChannelV3::Control,
        ChannelV3::Stdin,
        ChannelV3::Stdout,
        ChannelV3::Stderr,
        ChannelV3::Egress,
        ChannelV3::Archive,
    ];
    let mut stream = Vec::new();
    for (index, channel) in channels.iter().enumerate() {
        let payload = vec![index as u8; index + 1];
        let frame = encode_frame(*channel, &payload).unwrap();
        assert_eq!(frame, frame_bytes(channel.byte(), &payload));
        stream.extend_from_slice(&frame);
    }
    let mut input = stream.as_slice();
    let mut reader = FrameReaderV3::new();
    for (index, channel) in channels.iter().enumerate() {
        let frame = reader
            .read(&mut input, MAX_CONTROL_BYTES)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(frame.channel, *channel);
        assert_eq!(frame.payload, vec![index as u8; index + 1]);
        assert!(reader.at_boundary());
    }
    assert!(
        reader
            .read(&mut input, MAX_CONTROL_BYTES)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn bounds_are_judged_before_allocation_and_refusals_are_final() {
    // Encoder bounds.
    assert_eq!(
        encode_frame(ChannelV3::Stdout, &[]).err(),
        Some(SessionProtocolError::FrameSize)
    );
    assert!(encode_frame(ChannelV3::Stdout, &vec![1; MAX_DATA_BYTES]).is_ok());
    assert_eq!(
        encode_frame(ChannelV3::Stdout, &vec![1; MAX_DATA_BYTES + 1]).err(),
        Some(SessionProtocolError::FrameSize)
    );
    assert!(encode_frame(ChannelV3::Control, &vec![b' '; MAX_CONTROL_BYTES]).is_ok());
    assert_eq!(
        encode_frame(ChannelV3::Control, &vec![b' '; MAX_CONTROL_BYTES + 1]).err(),
        Some(SessionProtocolError::FrameSize)
    );
    // Reader bounds: only the header is sent, so a reader that allocated or
    // waited for the body would report PartialFrame instead.
    let header = |length: u32, channel: u8| {
        let mut bytes = length.to_be_bytes().to_vec();
        bytes.push(channel);
        bytes
    };
    for (bytes, expected) in [
        (
            0_u32.to_be_bytes().to_vec(),
            SessionProtocolError::FrameSize,
        ),
        (header(1, 2), SessionProtocolError::FrameSize),
        (
            header((MAX_HANDOFF_BYTES + 2) as u32, 0),
            SessionProtocolError::FrameSize,
        ),
        (
            u32::MAX.to_be_bytes().to_vec(),
            SessionProtocolError::FrameSize,
        ),
        (
            header((MAX_DATA_BYTES + 2) as u32, 2),
            SessionProtocolError::FrameSize,
        ),
        (
            header((MAX_CONTROL_BYTES + 2) as u32, 0),
            SessionProtocolError::FrameSize,
        ),
        (header(2, 6), SessionProtocolError::Protocol),
        (header(2, 255), SessionProtocolError::Protocol),
    ] {
        assert_eq!(
            read_one(&bytes, MAX_CONTROL_BYTES).await.err(),
            Some(expected)
        );
    }
    // The Handoff bound applies only when the caller admits it.
    let large = frame_bytes(0, &vec![b' '; MAX_CONTROL_BYTES + 1]);
    assert!(read_one(&large, MAX_HANDOFF_BYTES).await.unwrap().is_some());
    // An end inside a frame is not a clean end; a refused reader stays
    // refused.
    let full = frame_bytes(2, b"abc");
    for cut in 1..full.len() {
        assert_eq!(
            read_one(&full[..cut], MAX_CONTROL_BYTES).await.err(),
            Some(SessionProtocolError::PartialFrame)
        );
    }
    let mut input: &[u8] = &header(2, 9);
    let mut reader = FrameReaderV3::new();
    assert!(reader.read(&mut input, MAX_CONTROL_BYTES).await.is_err());
    let mut valid: &[u8] = &full;
    assert_eq!(
        reader.read(&mut valid, MAX_CONTROL_BYTES).await.err(),
        Some(SessionProtocolError::Closed)
    );
}

#[tokio::test]
async fn a_dropped_read_keeps_the_partial_frame() {
    let (mut writer, mut reader_side) = tokio::io::duplex(1024);
    let frame = frame_bytes(2, b"partial-then-complete");
    let mut reader = FrameReaderV3::new();
    writer.write_all(&frame[..3]).await.unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(50),
            reader.read(&mut reader_side, MAX_CONTROL_BYTES)
        )
        .await
        .is_err()
    );
    writer.write_all(&frame[3..9]).await.unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(50),
            reader.read(&mut reader_side, MAX_CONTROL_BYTES)
        )
        .await
        .is_err()
    );
    assert!(!reader.at_boundary());
    writer.write_all(&frame[9..]).await.unwrap();
    let read = reader
        .read(&mut reader_side, MAX_CONTROL_BYTES)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.payload, b"partial-then-complete");
}

// ---- control messages -------------------------------------------------------

fn control_payload(frame: &[u8]) -> &[u8] {
    &frame[FRAME_HEADER_BYTES..]
}

#[test]
fn worker_messages_are_closed_canonical_and_versioned() {
    let messages = [
        WorkerControlV3::Imported(ImportedV3 {
            manifest_sha256: "e".repeat(64),
            files: 2,
            bytes: 10,
        }),
        WorkerControlV3::Started,
        WorkerControlV3::Exited(ExitedV3 {
            code: Some(3),
            signal: None,
        }),
        WorkerControlV3::Exited(ExitedV3 {
            code: None,
            signal: Some(9),
        }),
        WorkerControlV3::ArchiveEnd(ArchiveEndV3 {
            archive_bytes: 1024,
            archive_sha256: "f".repeat(64),
        }),
        WorkerControlV3::Refused(RefusalV3::InteractiveUnsupported),
    ];
    for message in messages {
        let frame = message.encode().unwrap();
        assert_eq!(
            WorkerControlV3::decode(control_payload(&frame)).unwrap(),
            message
        );
    }
    for refused in [
        r#"{"kind":"started","version":2}"#,
        r#"{"kind":"started","version":3,"extra":1}"#,
        r#"{"kind":"started","version":3,"version":3}"#,
        r#"{"kind":"exited","version":3,"code":1}"#,
        r#"{"kind":"exited","version":3,"code":1,"signal":9}"#,
        r#"{"kind":"exited","version":3,"code":null,"signal":null}"#,
        r#"{"kind":"exited","version":3,"code":256,"signal":null}"#,
        r#"{"kind":"refused","version":3,"reason":"made_up"}"#,
        r#"{"kind":"imported","version":3,"manifest_sha256":"E","files":1,"bytes":1}"#,
        r#"{"kind":"handoff","version":3}"#,
        r#"{"kind":"start","version":3}"#,
        r#"[]"#,
        "not json",
    ] {
        assert!(
            WorkerControlV3::decode(refused.as_bytes()).is_err(),
            "{refused}"
        );
    }
}

#[test]
fn broker_messages_are_closed_and_only_the_handoff_takes_the_larger_bound() {
    let fixture = fixture(default_archive());
    for message in [
        BrokerControlV3::Start,
        BrokerControlV3::StdinClose,
        BrokerControlV3::Stop(StopReasonV3::Revoked),
    ] {
        let frame = message.encode().unwrap();
        let decoded = BrokerControlV3::decode(control_payload(&frame)).unwrap();
        assert_eq!(format!("{decoded:?}"), format!("{message:?}"));
    }
    let handoff = BrokerControlV3::Handoff(Box::new(fixture.handoff()));
    // Debug never shows the secret, the prompt or the profile.
    let debug = format!("{handoff:?}");
    assert!(!debug.contains(SECRET_CANARY) && !debug.contains(PROMPT_CANARY));
    let frame = handoff.encode().unwrap();
    let BrokerControlV3::Handoff(decoded) =
        BrokerControlV3::decode(control_payload(&frame)).unwrap()
    else {
        panic!("a handoff decodes as a handoff");
    };
    assert_eq!(
        decoded.profile_bytes(),
        fixture.profile.canonical_bytes().unwrap()
    );
    assert_eq!(decoded.bootstrap().expose(), payload_json(SECRET_CANARY));
    assert_eq!(decoded.args(), fixture.args.as_slice());
    assert_eq!(decoded.archive_bytes(), fixture.archive.len() as u64);
    assert_eq!(decoded.archive_sha256(), sha256_hex(&fixture.archive));
    // A large handoff is admitted; any other message above 16 KiB is not.
    let padded = format!(
        r#"{{"kind":"start","version":3{}}}"#,
        " ".repeat(MAX_CONTROL_BYTES)
    );
    assert_eq!(
        BrokerControlV3::decode(padded.as_bytes()).err(),
        Some(SessionProtocolError::FrameSize)
    );
    for refused in [
        r#"{"kind":"start","version":3,"extra":true}"#,
        r#"{"kind":"start","version":4}"#,
        r#"{"kind":"stop","version":3,"reason":"because"}"#,
        r#"{"kind":"stop","version":3}"#,
        r#"{"kind":"started","version":3}"#,
        r#"{"kind":"handoff","version":3,"profile":"{}","bootstrap":"x","program":"/bin/sh","args":[],"archive_bytes":0,"archive_sha256":"00"}"#,
    ] {
        assert!(
            BrokerControlV3::decode(refused.as_bytes()).is_err(),
            "{refused}"
        );
    }
    // A handoff whose program is not the bootstrap program is refused.
    assert!(
        HandoffV3::new(
            &fixture.profile,
            &payload_json(SECRET_CANARY),
            "/bin/sh",
            &fixture.args,
            0,
            &"0".repeat(64),
        )
        .is_err()
    );
}

// ---- order ------------------------------------------------------------------

#[test]
fn the_worker_accepts_only_the_session_order() {
    let fixture = fixture(default_archive());
    let handoff = || BrokerControlV3::Handoff(Box::new(fixture.handoff()));
    let archive = fixture.archive.len();
    let mut order = WorkerInboundOrderV3::new(SessionModeV3::Session);
    assert_eq!(order.control_limit(), MAX_HANDOFF_BYTES);
    // Nothing but a Handoff first.
    for early in [BrokerControlV3::Start, BrokerControlV3::StdinClose] {
        assert!(order.clone().admit_control(&early).is_err());
    }
    assert!(order.clone().admit_data(ChannelV3::Archive, 1).is_err());
    assert!(order.clone().admit_data(ChannelV3::Stdin, 1).is_err());
    order.admit_control(&handoff()).unwrap();
    assert_eq!(order.control_limit(), MAX_CONTROL_BYTES);
    // A second Handoff, a Start before Imported, stdin during the archive.
    assert!(order.clone().admit_control(&handoff()).is_err());
    assert!(
        order
            .clone()
            .admit_control(&BrokerControlV3::Start)
            .is_err()
    );
    assert!(order.clone().admit_data(ChannelV3::Stdin, 1).is_err());
    assert!(
        order
            .clone()
            .admit_data(ChannelV3::Archive, archive + 1)
            .is_err()
    );
    order.admit_data(ChannelV3::Archive, archive - 1).unwrap();
    assert!(!order.archive_complete());
    order.admit_data(ChannelV3::Archive, 1).unwrap();
    assert!(order.archive_complete());
    assert!(order.clone().admit_data(ChannelV3::Archive, 1).is_err());
    assert!(
        order
            .clone()
            .admit_control(&BrokerControlV3::Start)
            .is_err()
    );
    order.imported().unwrap();
    assert!(order.clone().imported().is_err());
    order.admit_control(&BrokerControlV3::Start).unwrap();
    order.admit_data(ChannelV3::Stdin, 10).unwrap();
    order.admit_data(ChannelV3::Egress, 10).unwrap();
    assert!(order.clone().admit_data(ChannelV3::Stdout, 1).is_err());
    assert!(order.clone().admit_data(ChannelV3::Archive, 1).is_err());
    assert!(
        order
            .clone()
            .admit_control(&BrokerControlV3::Start)
            .is_err()
    );
    order.admit_control(&BrokerControlV3::StdinClose).unwrap();
    assert!(
        order
            .clone()
            .admit_control(&BrokerControlV3::StdinClose)
            .is_err()
    );
    assert!(order.clone().admit_data(ChannelV3::Stdin, 1).is_err());
    order.admit_data(ChannelV3::Egress, 1).unwrap();
    order
        .admit_control(&BrokerControlV3::Stop(StopReasonV3::Cancelled))
        .unwrap();
    // Nothing after Stop.
    assert!(order.clone().admit_data(ChannelV3::Egress, 1).is_err());
    assert!(
        order
            .clone()
            .admit_control(&BrokerControlV3::Stop(StopReasonV3::Cancelled))
            .is_err()
    );

    let mut export = WorkerInboundOrderV3::new(SessionModeV3::Export);
    assert!(export.clone().admit_control(&handoff()).is_err());
    assert!(export.clone().admit_data(ChannelV3::Archive, 1).is_err());
    export.admit_control(&BrokerControlV3::Start).unwrap();
    assert!(export.clone().admit_data(ChannelV3::Stdin, 1).is_err());
    assert!(
        export
            .clone()
            .admit_control(&BrokerControlV3::Start)
            .is_err()
    );
    export
        .admit_control(&BrokerControlV3::Stop(StopReasonV3::Deadline))
        .unwrap();
}

#[test]
fn the_broker_checks_its_own_sends_and_every_worker_frame() {
    let fixture = fixture(default_archive());
    let archive = &fixture.archive;
    let mut order = BrokerOrderV3::new(SessionModeV3::Session);
    // No worker frame before the Handoff; no Start before Imported.
    assert!(order.receive_control(&WorkerControlV3::Started).is_err());
    assert!(order.send_control(&BrokerControlV3::Start).is_err());
    order
        .send_control(&BrokerControlV3::Handoff(Box::new(fixture.handoff())))
        .unwrap();
    let imported = WorkerControlV3::Imported(ImportedV3 {
        manifest_sha256: fixture.profile.retained_workspace_commitment().0.to_owned(),
        files: 2,
        bytes: 10,
    });
    // Imported before the whole archive was sent is out of order.
    assert!(order.receive_control(&imported).is_err());
    order.send_data(ChannelV3::Archive, archive).unwrap();
    assert!(order.send_data(ChannelV3::Archive, b"x").is_err());
    let mut mismatched = BrokerOrderV3::new(SessionModeV3::Session);
    mismatched
        .send_control(&BrokerControlV3::Handoff(Box::new(fixture.handoff())))
        .unwrap();
    mismatched.send_data(ChannelV3::Archive, archive).unwrap();
    let wrong = WorkerControlV3::Imported(ImportedV3 {
        manifest_sha256: "0".repeat(64),
        files: 2,
        bytes: 10,
    });
    assert_eq!(
        mismatched.receive_control(&wrong).err(),
        Some(SessionProtocolError::Mismatch)
    );
    order.receive_control(&imported).unwrap();
    assert!(order.receive_data(ChannelV3::Stdout, b"x").is_err());
    assert!(order.send_data(ChannelV3::Stdin, b"x").is_err());
    assert!(!order.egress_open());
    order.send_control(&BrokerControlV3::Start).unwrap();
    assert!(order.egress_open());
    order.send_data(ChannelV3::Stdin, b"x").unwrap();
    assert!(order.receive_data(ChannelV3::Egress, b"x").is_err());
    order.receive_control(&WorkerControlV3::Started).unwrap();
    for channel in [ChannelV3::Stdout, ChannelV3::Stderr, ChannelV3::Egress] {
        order.receive_data(channel, b"x").unwrap();
    }
    assert!(order.receive_data(ChannelV3::Archive, b"x").is_err());
    assert!(order.receive_data(ChannelV3::Stdin, b"x").is_err());
    order
        .receive_control(&WorkerControlV3::Exited(ExitedV3 {
            code: Some(0),
            signal: None,
        }))
        .unwrap();
    assert!(order.ended());
    assert!(order.receive_data(ChannelV3::Stdout, b"x").is_err());
    assert_eq!(
        order.send_data(ChannelV3::Stdin, b"x").err(),
        Some(SessionProtocolError::Closed)
    );

    // An archive that differs from its declared digest is refused locally.
    let mut tampered = BrokerOrderV3::new(SessionModeV3::Session);
    tampered
        .send_control(&BrokerControlV3::Handoff(Box::new(fixture.handoff())))
        .unwrap();
    let mut changed = archive.clone();
    changed[600] ^= 1;
    assert_eq!(
        tampered.send_data(ChannelV3::Archive, &changed).err(),
        Some(SessionProtocolError::Mismatch)
    );

    // Export: the declared end must equal the bytes received.
    let mut export = BrokerOrderV3::new(SessionModeV3::Export);
    assert!(export.receive_data(ChannelV3::Archive, b"x").is_err());
    export.send_control(&BrokerControlV3::Start).unwrap();
    export.receive_data(ChannelV3::Archive, b"abc").unwrap();
    let mut bad = BrokerOrderV3::new(SessionModeV3::Export);
    bad.send_control(&BrokerControlV3::Start).unwrap();
    bad.receive_data(ChannelV3::Archive, b"abc").unwrap();
    assert_eq!(
        bad.receive_control(&WorkerControlV3::ArchiveEnd(ArchiveEndV3 {
            archive_bytes: 4,
            archive_sha256: sha256_hex(b"abc"),
        }))
        .err(),
        Some(SessionProtocolError::Mismatch)
    );
    export
        .receive_control(&WorkerControlV3::ArchiveEnd(ArchiveEndV3 {
            archive_bytes: 3,
            archive_sha256: sha256_hex(b"abc"),
        }))
        .unwrap();
    assert!(export.ended());
}

// ---- broker channel ---------------------------------------------------------

async fn worker_frame<R: AsyncRead + Unpin>(reader: &mut FrameReaderV3, input: &mut R) -> FrameV3 {
    tokio::time::timeout(
        Duration::from_secs(5),
        reader.read(input, MAX_HANDOFF_BYTES),
    )
    .await
    .expect("a frame within the test watchdog")
    .unwrap()
    .expect("a frame, not the end")
}

async fn next_event(events: &mut SessionEventsV3) -> Result<Option<WorkerEventV3>> {
    tokio::time::timeout(Duration::from_secs(5), events.next())
        .await
        .expect("an event within the test watchdog")
}

/// The whole session order through the real broker channel against a
/// scripted worker: Handoff and archive, Imported, Start, Started, stdin,
/// stdout/stderr, egress byte-exact both ways (gated until Start), Exited
/// and the clean end.
#[tokio::test]
async fn broker_channel_runs_the_session_order_and_carries_egress_byte_exact() {
    let fixture = fixture(default_archive());
    let (broker_side, mut worker) = tokio::io::duplex(256 * 1024);
    let mut channel = SessionChannelV3::broker(broker_side, SessionModeV3::Session).unwrap();
    let mut egress = channel.take_egress().unwrap();
    assert!(channel.take_egress().is_none());
    let (mut sender, mut events, _) = channel.split();
    // Egress written before Start is held, not framed.
    egress.write_all(b"HELLO-BEFORE-START").await.unwrap();
    sender
        .send_control(BrokerControlV3::Handoff(Box::new(fixture.handoff())))
        .await
        .unwrap();
    sender.send_archive(&fixture.archive).await.unwrap();
    let mut reader = FrameReaderV3::new();
    let first = worker_frame(&mut reader, &mut worker).await;
    assert_eq!(first.channel, ChannelV3::Control);
    assert!(
        first
            .payload
            .windows(SECRET_CANARY.len())
            .any(|window| window == SECRET_CANARY.as_bytes()),
        "the handoff carries the payload to the worker"
    );
    let mut received = Vec::new();
    while received.len() < fixture.archive.len() {
        let frame = worker_frame(&mut reader, &mut worker).await;
        assert_eq!(frame.channel, ChannelV3::Archive);
        received.extend_from_slice(&frame.payload);
    }
    assert_eq!(received, fixture.archive);
    let (manifest_sha256, files, bytes) = fixture.profile.retained_workspace_commitment();
    let imported = WorkerControlV3::Imported(ImportedV3 {
        manifest_sha256: manifest_sha256.to_owned(),
        files,
        bytes,
    });
    worker.write_all(&imported.encode().unwrap()).await.unwrap();
    assert!(matches!(
        next_event(&mut events).await.unwrap(),
        Some(WorkerEventV3::Control(WorkerControlV3::Imported(_)))
    ));
    sender.send_control(BrokerControlV3::Start).await.unwrap();
    let start = worker_frame(&mut reader, &mut worker).await;
    assert!(matches!(
        BrokerControlV3::decode(&start.payload).unwrap(),
        BrokerControlV3::Start
    ));
    // The held egress bytes now leave, unchanged, after Start.
    let held = worker_frame(&mut reader, &mut worker).await;
    assert_eq!(held.channel, ChannelV3::Egress);
    assert_eq!(held.payload, b"HELLO-BEFORE-START");
    worker
        .write_all(&WorkerControlV3::Started.encode().unwrap())
        .await
        .unwrap();
    assert!(matches!(
        next_event(&mut events).await.unwrap(),
        Some(WorkerEventV3::Control(WorkerControlV3::Started))
    ));
    sender.send_stdin(b"to-the-provider").await.unwrap();
    let stdin = worker_frame(&mut reader, &mut worker).await;
    assert_eq!(
        (stdin.channel, stdin.payload.as_slice()),
        (ChannelV3::Stdin, &b"to-the-provider"[..])
    );
    // Egress from the worker reaches the G05 side byte-exact, including
    // bytes that look like frames.
    let opaque: Vec<u8> = (0..=255_u8).chain(frame_bytes(0, b"{}")).collect();
    worker
        .write_all(&encode_frame(ChannelV3::Egress, &opaque).unwrap())
        .await
        .unwrap();
    let mut echoed = vec![0_u8; opaque.len()];
    egress.read_exact(&mut echoed).await.unwrap();
    assert_eq!(echoed, opaque);
    worker
        .write_all(&encode_frame(ChannelV3::Stdout, b"out").unwrap())
        .await
        .unwrap();
    worker
        .write_all(&encode_frame(ChannelV3::Stderr, b"err").unwrap())
        .await
        .unwrap();
    assert!(
        matches!(next_event(&mut events).await.unwrap(), Some(WorkerEventV3::Stdout(bytes)) if bytes == b"out")
    );
    assert!(
        matches!(next_event(&mut events).await.unwrap(), Some(WorkerEventV3::Stderr(bytes)) if bytes == b"err")
    );
    sender
        .send_control(BrokerControlV3::StdinClose)
        .await
        .unwrap();
    assert!(sender.send_stdin(b"late").await.is_err());
    let exited = WorkerControlV3::Exited(ExitedV3 {
        code: Some(0),
        signal: None,
    });
    worker.write_all(&exited.encode().unwrap()).await.unwrap();
    worker.shutdown().await.unwrap();
    assert!(matches!(
        next_event(&mut events).await.unwrap(),
        Some(WorkerEventV3::Control(WorkerControlV3::Exited(_)))
    ));
    assert!(next_event(&mut events).await.unwrap().is_none());
    drop(sender);
}

#[tokio::test]
async fn broker_channel_refuses_a_worker_frame_out_of_order_and_an_early_end() {
    // Started before any Handoff.
    let (broker_side, mut worker) = tokio::io::duplex(64 * 1024);
    let channel = SessionChannelV3::broker(broker_side, SessionModeV3::Session).unwrap();
    let (_sender, mut events, _) = channel.split();
    worker
        .write_all(&WorkerControlV3::Started.encode().unwrap())
        .await
        .unwrap();
    assert_eq!(
        next_event(&mut events).await.err(),
        Some(SessionProtocolError::Protocol)
    );
    assert_eq!(
        next_event(&mut events).await.err(),
        Some(SessionProtocolError::Closed)
    );
    // An end of stream before the worker's end message is not clean.
    let (broker_side, worker) = tokio::io::duplex(64 * 1024);
    let channel = SessionChannelV3::broker(broker_side, SessionModeV3::Export).unwrap();
    let (_sender, mut events, _) = channel.split();
    drop(worker);
    assert_eq!(
        next_event(&mut events).await.err(),
        Some(SessionProtocolError::Closed)
    );
}

#[tokio::test]
async fn a_stop_closes_egress_and_no_egress_frame_follows_it() {
    let fixture = fixture(default_archive());
    let (broker_side, mut worker) = tokio::io::duplex(256 * 1024);
    let mut channel = SessionChannelV3::broker(broker_side, SessionModeV3::Session).unwrap();
    let mut egress = channel.take_egress().unwrap();
    let (mut sender, mut events, _) = channel.split();
    sender
        .send_control(BrokerControlV3::Handoff(Box::new(fixture.handoff())))
        .await
        .unwrap();
    sender.send_archive(&fixture.archive).await.unwrap();
    let mut reader = FrameReaderV3::new();
    let mut seen = 0;
    while seen < fixture.archive.len() + 1 {
        let frame = worker_frame(&mut reader, &mut worker).await;
        seen += if frame.channel == ChannelV3::Archive {
            frame.payload.len()
        } else {
            1
        };
    }
    let (manifest_sha256, files, bytes) = fixture.profile.retained_workspace_commitment();
    worker
        .write_all(
            &WorkerControlV3::Imported(ImportedV3 {
                manifest_sha256: manifest_sha256.to_owned(),
                files,
                bytes,
            })
            .encode()
            .unwrap(),
        )
        .await
        .unwrap();
    next_event(&mut events).await.unwrap();
    sender.send_control(BrokerControlV3::Start).await.unwrap();
    sender
        .send_control(BrokerControlV3::Stop(StopReasonV3::Cancelled))
        .await
        .unwrap();
    egress.write_all(b"after-stop").await.unwrap();
    let start = worker_frame(&mut reader, &mut worker).await;
    assert!(matches!(
        BrokerControlV3::decode(&start.payload).unwrap(),
        BrokerControlV3::Start
    ));
    let stop = worker_frame(&mut reader, &mut worker).await;
    assert!(matches!(
        BrokerControlV3::decode(&stop.payload).unwrap(),
        BrokerControlV3::Stop(StopReasonV3::Cancelled)
    ));
    sender.close().await;
    // Nothing else is written: the next read is the end of the stream.
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), worker.read_to_end(&mut rest))
        .await
        .unwrap()
        .unwrap();
    assert!(rest.is_empty());
}
