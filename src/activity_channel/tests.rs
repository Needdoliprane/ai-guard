//! Seams: the wire codec (encode/decode, bounds, poisoning), the hook client
//! as a real process and in-process against a real helper, and the helper's
//! broker channel. Sockets, pipes and processes are real; no provider runs.
use super::{
    LOSS_DIRECTORY, SOCKET_NAME,
    helper::{HelperError, MAX_PENDING_HOOKS, serve},
    hook::{self, BLOCKED_LINE, EXIT_BLOCK, EXIT_CONTINUE, EXIT_USAGE, HookSettings},
    wire::*,
};
use crate::control_fs::Directory;
use serde_json::json;
use std::{
    ffi::OsString,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    net::{
        UnixStream,
        unix::{OwnedReadHalf, OwnedWriteHalf},
    },
    task::JoinHandle,
    time::{Instant, timeout},
};

const QA_ROOT: &str = "AI_GUARD_ACTIVITY_QA_ROOT";
const QA_ARGS: &str = "AI_GUARD_ACTIVITY_QA_ARGS";
const QA_MARKER: &[u8] = b"QA_ACTIVITY_HOOK_START\n";
/// Harness waits only; product deadlines are never relaxed in these tests.
const WAIT: Duration = Duration::from_secs(20);

fn submit(client: ClientV1, event: EventV1, payload: &str) -> SubmitV1 {
    SubmitV1 {
        version: 1,
        client,
        event,
        gate: event.is_gate(),
        stdin_total_bytes: payload.len() as u64,
        stdin_truncated: false,
        payload_invalid_utf8: false,
        payload: payload.into(),
    }
}

fn framed(body: &[u8]) -> Vec<u8> {
    let mut bytes = (body.len() as u32).to_be_bytes().to_vec();
    bytes.extend_from_slice(body);
    bytes
}

fn arguments(text: &str) -> Vec<OsString> {
    text.split(' ').map(OsString::from).collect()
}

// ---------------------------------------------------------------- wire codec

#[test]
fn frames_have_the_exact_v1_json_shapes_behind_a_big_endian_length() {
    let frame = ClientFrameV1::Submit(submit(
        ClientV1::ClaudeCode,
        EventV1::PreToolUse,
        "{\"tool_name\":\"Bash\"}\n",
    ));
    let bytes = encode_framed(&frame).unwrap();
    let length = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
    assert_eq!(length, bytes.len() - 4);
    let value: serde_json::Value = serde_json::from_slice(&bytes[4..]).unwrap();
    assert_eq!(
        value,
        json!({"type":"submit","version":1,"client":"claude-code","event":"PreToolUse",
               "gate":true,"stdin_total_bytes":21,"stdin_truncated":false,
               "payload_invalid_utf8":false,"payload":"{\"tool_name\":\"Bash\"}\n"})
    );
    assert_eq!(decode_body::<ClientFrameV1>(&bytes[4..]).unwrap(), frame);

    let hook = HelperFrameV1::Hook(HookV1 {
        seq: 7,
        submit: submit(ClientV1::Codex, EventV1::Stop, ""),
    });
    let shapes = [
        (
            serde_json::to_value(&hook).unwrap(),
            json!({"type":"hook","seq":7,"submit":{"version":1,"client":"codex","event":"Stop",
                   "gate":false,"stdin_total_bytes":0,"stdin_truncated":false,
                   "payload_invalid_utf8":false,"payload":""}}),
        ),
        (
            serde_json::to_value(HelperFrameV1::Hello(HelloV1 { version: 1 })).unwrap(),
            json!({"type":"hello","version":1}),
        ),
        (
            serde_json::to_value(HelperFrameV1::Loss(LossV1 {
                seq: 8,
                count: 3,
                reason: LossReasonV1::OversizedFrame,
            }))
            .unwrap(),
            json!({"type":"loss","seq":8,"count":3,"reason":"oversized_frame"}),
        ),
        (
            serde_json::to_value(BrokerFrameV1::Ready(ReadyV1 { version: 1 })).unwrap(),
            json!({"type":"ready","version":1}),
        ),
        (
            serde_json::to_value(BrokerFrameV1::Ack(AckV1 {
                seq: 7,
                status: StatusV1::Recorded,
            }))
            .unwrap(),
            json!({"type":"ack","seq":7,"status":"recorded"}),
        ),
        (
            serde_json::to_value(ReplyFrameV1::Reply(ReplyV1 {
                version: 1,
                status: StatusV1::Refused,
            }))
            .unwrap(),
            json!({"type":"reply","version":1,"status":"refused"}),
        ),
    ];
    for (actual, expected) in shapes {
        assert_eq!(actual, expected);
    }
    for reason in LossReasonV1::ALL {
        let text = serde_json::to_value(reason).unwrap();
        assert!(
            [
                "client_unrecorded",
                "helper_capacity",
                "oversized_frame",
                "invalid_frame",
                "peer_refused"
            ]
            .contains(&text.as_str().unwrap())
        );
    }
}

#[test]
fn unknown_fields_versions_and_inconsistent_submissions_are_refused() {
    let valid = serde_json::to_value(ClientFrameV1::Submit(submit(
        ClientV1::ClaudeCode,
        EventV1::Stop,
        "ab",
    )))
    .unwrap();
    assert!(decode_body::<ClientFrameV1>(valid.to_string().as_bytes()).is_ok());
    let mutations: [(&str, serde_json::Value); 9] = [
        ("extra", json!(true)),
        ("type", json!("hook")),
        ("version", json!(2)),
        ("gate", json!(true)),
        ("event", json!("Unknown")),
        ("client", json!("other")),
        ("stdin_total_bytes", json!(1)),
        ("stdin_total_bytes", json!(3)),
        ("payload_invalid_utf8", json!(true)),
    ];
    for (field, value) in mutations {
        let mut changed = valid.clone();
        changed[field] = value;
        assert_eq!(
            decode_body::<ClientFrameV1>(changed.to_string().as_bytes()),
            Err(WireError::Protocol),
            "{field} must be refused"
        );
    }
    let mut missing = valid.clone();
    missing.as_object_mut().unwrap().remove("payload");
    assert!(decode_body::<ClientFrameV1>(missing.to_string().as_bytes()).is_err());
    let duplicate = br#"{"type":"submit","version":1,"version":1,"client":"codex","event":"Stop","gate":false,"stdin_total_bytes":0,"stdin_truncated":false,"payload_invalid_utf8":false,"payload":""}"#;
    assert!(decode_body::<ClientFrameV1>(duplicate).is_err());
    // Codex has no SubagentStop hook in this protocol version.
    let codex = submit(ClientV1::Codex, EventV1::SubagentStop, "");
    assert_eq!(codex.validate(), Err(WireError::Protocol));
    // Nested submissions refuse unknown fields as well.
    let nested = br#"{"type":"hook","seq":1,"submit":{"version":1,"client":"codex","event":"Stop","gate":false,"stdin_total_bytes":0,"stdin_truncated":false,"payload_invalid_utf8":false,"payload":"","tenant":"a"}}"#;
    assert_eq!(
        decode_body::<HelperFrameV1>(nested),
        Err(WireError::Protocol)
    );
    for body in [
        br#"{"type":"hook","seq":0,"submit":{"version":1,"client":"codex","event":"Stop","gate":false,"stdin_total_bytes":0,"stdin_truncated":false,"payload_invalid_utf8":false,"payload":""}}"#.as_slice(),
        br#"{"type":"loss","seq":1,"count":0,"reason":"invalid_frame"}"#,
        br#"{"type":"loss","seq":1,"count":1,"reason":"other"}"#,
        br#"{"type":"hello","version":2}"#,
    ] {
        assert_eq!(decode_body::<HelperFrameV1>(body), Err(WireError::Protocol));
    }
    for body in [
        br#"{"type":"ack","seq":0,"status":"recorded"}"#.as_slice(),
        br#"{"type":"ack","seq":1,"status":"allow"}"#,
        br#"{"type":"ack","seq":1,"status":"recorded","decision":"allow"}"#,
        br#"{"type":"ready","version":2}"#,
        br#"{"type":"hello","version":1}"#,
    ] {
        assert_eq!(decode_body::<BrokerFrameV1>(body), Err(WireError::Protocol));
    }
    assert_eq!(
        decode_body::<ReplyFrameV1>(br#"{"type":"reply","version":1,"status":"allow"}"#),
        Err(WireError::Protocol)
    );
}

#[tokio::test]
async fn advertised_lengths_are_checked_before_any_body_is_read() {
    for (prefix, control) in [
        ((MAX_SUBMIT_FRAME_BYTES + 1) as u32, false),
        (0, false),
        ((MAX_CONTROL_FRAME_BYTES + 1) as u32, true),
    ] {
        // Only the prefix is ever written; a body read would wait forever.
        let (mut peer, mut local) = UnixStream::pair().unwrap();
        peer.write_all(&prefix.to_be_bytes()).await.unwrap();
        let result = if control {
            timeout(WAIT, FrameReader::<BrokerFrameV1>::new().read(&mut local))
                .await
                .unwrap()
                .map(|_| ())
        } else {
            timeout(WAIT, FrameReader::<ClientFrameV1>::new().read(&mut local))
                .await
                .unwrap()
                .map(|_| ())
        };
        assert_eq!(result, Err(WireError::FrameSize));
        drop(peer);
    }
    // A complete frame of exactly the control bound is still accepted shape-wise.
    let ack = encode_framed(&BrokerFrameV1::Ack(AckV1 {
        seq: u64::MAX,
        status: StatusV1::Recorded,
    }))
    .unwrap();
    assert!(ack.len() - 4 <= MAX_CONTROL_FRAME_BYTES);
}

#[tokio::test]
async fn any_error_or_eof_poisons_the_reader_without_resynchronizing() {
    let ready = encode_framed(&BrokerFrameV1::Ready(ReadyV1 { version: 1 })).unwrap();
    let mut bytes = framed(b"{\"type\":\"ready\",\"version\":1,\"x\":1}");
    bytes.extend_from_slice(&ready);
    let (mut peer, mut local) = UnixStream::pair().unwrap();
    peer.write_all(&bytes).await.unwrap();
    let mut reader = FrameReader::<BrokerFrameV1>::new();
    assert_eq!(reader.read(&mut local).await, Err(WireError::Protocol));
    assert_eq!(reader.read(&mut local).await, Err(WireError::Closed));

    let (mut peer, mut local) = UnixStream::pair().unwrap();
    peer.write_all(&ready).await.unwrap();
    drop(peer);
    let mut reader = FrameReader::<BrokerFrameV1>::new();
    assert!(matches!(
        reader.read(&mut local).await,
        Ok(Some(BrokerFrameV1::Ready(_)))
    ));
    assert_eq!(reader.read(&mut local).await, Ok(None));
    assert_eq!(reader.read(&mut local).await, Err(WireError::Closed));

    // EOF inside a frame is a partial frame, not a clean end.
    let (mut peer, mut local) = UnixStream::pair().unwrap();
    peer.write_all(&ready[..ready.len() - 1]).await.unwrap();
    drop(peer);
    assert_eq!(
        FrameReader::<BrokerFrameV1>::new().read(&mut local).await,
        Err(WireError::PartialFrame)
    );
}

#[tokio::test]
async fn a_cancelled_read_keeps_its_partial_frame_for_the_next_poll() {
    let ack = encode_framed(&BrokerFrameV1::Ack(AckV1 {
        seq: 3,
        status: StatusV1::Refused,
    }))
    .unwrap();
    let (mut peer, mut local) = UnixStream::pair().unwrap();
    let mut reader = FrameReader::<BrokerFrameV1>::new();
    peer.write_all(&ack[..6]).await.unwrap();
    assert!(
        timeout(Duration::from_millis(50), reader.read(&mut local))
            .await
            .is_err()
    );
    peer.write_all(&ack[6..]).await.unwrap();
    assert_eq!(
        timeout(WAIT, reader.read(&mut local)).await.unwrap(),
        Ok(Some(BrokerFrameV1::Ack(AckV1 {
            seq: 3,
            status: StatusV1::Refused
        })))
    );
}

#[test]
fn the_payload_bound_counts_its_json_escaping() {
    let plain = "a".repeat(MAX_PAYLOAD_BYTES);
    assert_eq!(bounded_payload(&plain), (plain.as_str(), false));
    let largest = ClientFrameV1::Submit(SubmitV1 {
        stdin_total_bytes: u64::MAX,
        stdin_truncated: true,
        ..submit(ClientV1::ClaudeCode, EventV1::PostToolUseFailure, &plain)
    });
    assert!(encode_framed(&largest).unwrap().len() - 4 <= MAX_SUBMIT_FRAME_BYTES);
    let quotes = "\"".repeat(MAX_PAYLOAD_BYTES);
    let (cut, truncated) = bounded_payload(&quotes);
    assert!(truncated);
    assert_eq!(cut.len(), MAX_PAYLOAD_BYTES / 2);
    let controls = "\u{1}".repeat(MAX_PAYLOAD_BYTES);
    let (cut, truncated) = bounded_payload(&controls);
    assert!(truncated);
    assert_eq!(cut.len(), MAX_PAYLOAD_BYTES / 6);
    let escaped = ClientFrameV1::Submit(SubmitV1 {
        stdin_total_bytes: quotes.len() as u64,
        stdin_truncated: true,
        ..submit(
            ClientV1::Codex,
            EventV1::PreToolUse,
            bounded_payload(&quotes).0,
        )
    });
    assert!(encode_framed(&escaped).unwrap().len() - 4 <= MAX_SUBMIT_FRAME_BYTES);
    // A character is never split by the cut.
    let wide = "é".repeat(MAX_PAYLOAD_BYTES / 2 + 1);
    let (cut, truncated) = bounded_payload(&wide);
    assert!(truncated && cut.len() == MAX_PAYLOAD_BYTES && cut.chars().all(|c| c == 'é'));
}

#[test]
fn the_helper_stream_needs_one_hello_then_contiguous_sequence_numbers() {
    let hook = |seq| {
        HelperFrameV1::Hook(HookV1 {
            seq,
            submit: submit(ClientV1::Codex, EventV1::Stop, ""),
        })
    };
    let loss = |seq| {
        HelperFrameV1::Loss(LossV1 {
            seq,
            count: 1,
            reason: LossReasonV1::InvalidFrame,
        })
    };
    let hello = HelperFrameV1::Hello(HelloV1 { version: 1 });
    let mut sequence = HelperSequence::new();
    assert_eq!(sequence.accept(&hook(1)), Err(WireError::Protocol));
    let mut sequence = HelperSequence::new();
    sequence.accept(&hello).unwrap();
    sequence.accept(&hook(1)).unwrap();
    sequence.accept(&loss(2)).unwrap();
    assert_eq!(sequence.last(), 2);
    for wrong in [hook(2), loss(4), hello.clone()] {
        let mut copy = HelperSequence::new();
        copy.accept(&hello).unwrap();
        copy.accept(&hook(1)).unwrap();
        copy.accept(&loss(2)).unwrap();
        assert_eq!(copy.accept(&wrong), Err(WireError::Protocol));
    }
}

// ---------------------------------------------------------- hook submission

#[test]
fn invocations_are_closed_and_only_gate_names_block_when_unusable() {
    assert_eq!(
        hook::parse(&arguments("claude-code SubagentStop")),
        hook::Invocation::Valid {
            client: ClientV1::ClaudeCode,
            event: EventV1::SubagentStop
        }
    );
    for (text, gate) in [
        ("codex SubagentStop", false),
        ("bogus PreToolUse", true),
        ("claude-code UserPromptSubmit extra", true),
        ("claude-code", false),
        ("claude-code Allow", false),
    ] {
        assert_eq!(
            hook::parse(&arguments(text)),
            hook::Invocation::Invalid { gate },
            "{text}"
        );
    }
    let settings = HookSettings {
        root: PathBuf::from("/nonexistent/ai-guard-activity-qa"),
        ..HookSettings::production()
    };
    let mut stderr = Vec::new();
    assert_eq!(
        hook::run(
            &settings,
            &arguments("codex SubagentStop"),
            std::io::empty(),
            &mut stderr
        ),
        EXIT_USAGE
    );
    assert!(stderr.is_empty());
    assert_eq!(
        hook::run(
            &settings,
            &arguments("bogus PreToolUse"),
            std::io::empty(),
            &mut stderr
        ),
        EXIT_BLOCK
    );
    assert_eq!(stderr, BLOCKED_LINE);
}

#[test]
fn stdin_bytes_become_a_flagged_utf8_prefix() {
    let captured = |kept: &[u8], total: u64, complete: bool| hook::Captured {
        kept: kept.to_vec(),
        total,
        complete,
    };
    let whole = hook::submission(
        ClientV1::ClaudeCode,
        EventV1::Stop,
        &captured(b"{\"a\":1}", 7, true),
    );
    assert_eq!(
        (
            whole.payload.as_str(),
            whole.stdin_truncated,
            whole.payload_invalid_utf8
        ),
        ("{\"a\":1}", false, false)
    );
    whole.validate().unwrap();
    // A character split by the byte bound is dropped from the end.
    let cut = hook::submission(
        ClientV1::ClaudeCode,
        EventV1::Stop,
        &captured("aé".as_bytes()[..2].as_ref(), 9, true),
    );
    assert_eq!(
        (
            cut.payload.as_str(),
            cut.stdin_truncated,
            cut.stdin_total_bytes
        ),
        ("a", true, 9)
    );
    cut.validate().unwrap();
    // Invalid UTF-8 is never repaired: an empty payload and the flag.
    let invalid = hook::submission(
        ClientV1::Codex,
        EventV1::PreToolUse,
        &captured(b"ok\xff{}", 5, true),
    );
    assert_eq!(
        (
            invalid.payload.as_str(),
            invalid.payload_invalid_utf8,
            invalid.stdin_truncated
        ),
        ("", true, false)
    );
    invalid.validate().unwrap();
    // A drain that hit its deadline is flagged even when the bytes are short.
    let late = hook::submission(ClientV1::Codex, EventV1::Stop, &captured(b"{}", 2, false));
    assert!(late.stdin_truncated);
    late.validate().unwrap();
}

// ------------------------------------------------------------- helper seams

fn fixture_root() -> (tempfile::TempDir, Directory) {
    let temporary = tempfile::Builder::new().prefix("aga-").tempdir().unwrap();
    std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let directory = Directory::open_existing(&temporary.path().canonicalize().unwrap()).unwrap();
    (temporary, directory)
}

struct Broker {
    reader: FrameReader<HelperFrameV1>,
    writer: FrameWriter,
    input: OwnedReadHalf,
    output: Option<OwnedWriteHalf>,
}
impl Broker {
    fn start(root: Directory) -> (Self, JoinHandle<Result<(), HelperError>>) {
        let (broker, helper) = UnixStream::pair().unwrap();
        let (input, output) = helper.into_split();
        let task = tokio::spawn(serve(root, input, output));
        let (input, output) = broker.into_split();
        (
            Self {
                reader: FrameReader::new(),
                writer: FrameWriter::new(),
                input,
                output: Some(output),
            },
            task,
        )
    }

    async fn next(&mut self) -> Option<HelperFrameV1> {
        timeout(WAIT, self.reader.read(&mut self.input))
            .await
            .expect("helper frame within the harness wait")
            .expect("valid helper frame")
    }

    async fn send(&mut self, frame: BrokerFrameV1) {
        let output = self.output.as_mut().unwrap();
        self.writer.write(output, &frame).await.unwrap();
    }

    async fn ready(&mut self) {
        assert_eq!(
            self.next().await,
            Some(HelperFrameV1::Hello(HelloV1 { version: 1 }))
        );
        self.send(BrokerFrameV1::Ready(ReadyV1 { version: 1 }))
            .await;
    }

    async fn hook(&mut self) -> HookV1 {
        match self.next().await {
            Some(HelperFrameV1::Hook(hook)) => hook,
            other => panic!("expected a hook frame, got {other:?}"),
        }
    }

    async fn ack(&mut self, seq: u64, status: StatusV1) {
        self.send(BrokerFrameV1::Ack(AckV1 { seq, status })).await;
    }
}

async fn join(task: JoinHandle<Result<(), HelperError>>) -> Result<(), HelperError> {
    timeout(WAIT, task)
        .await
        .expect("helper owner closes within the harness wait")
        .unwrap()
}

async fn client(root: &Directory) -> UnixStream {
    let path = root.path().join(SOCKET_NAME);
    let started = Instant::now();
    loop {
        if let Ok(stream) = UnixStream::connect(&path).await {
            return stream;
        }
        assert!(started.elapsed() < WAIT, "helper socket never appeared");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn send_submit(stream: &mut UnixStream, submission: SubmitV1) {
    write_frame(stream, &ClientFrameV1::Submit(submission))
        .await
        .unwrap();
}

async fn reply(stream: &mut UnixStream) -> Option<StatusV1> {
    match timeout(WAIT, FrameReader::<ReplyFrameV1>::new().read(stream))
        .await
        .unwrap()
    {
        Ok(Some(ReplyFrameV1::Reply(reply))) => Some(reply.status),
        _ => None,
    }
}

#[tokio::test]
async fn hello_precedes_any_socket_and_eof_before_ready_ends_cleanly() {
    let (_temporary, root) = fixture_root();
    let (mut broker, task) = Broker::start(root.clone());
    assert_eq!(
        broker.next().await,
        Some(HelperFrameV1::Hello(HelloV1 { version: 1 }))
    );
    assert!(!root.path().join(SOCKET_NAME).exists());
    assert!(!root.path().join(LOSS_DIRECTORY).exists());
    drop(broker);
    assert_eq!(join(task).await, Ok(()));
    assert!(!root.path().join(SOCKET_NAME).exists());
}

#[tokio::test]
async fn submissions_get_contiguous_seqs_and_each_ack_reaches_its_own_client() {
    let (_temporary, root) = fixture_root();
    let (mut broker, task) = Broker::start(root.clone());
    broker.ready().await;
    let mut first = client(&root).await;
    send_submit(
        &mut first,
        submit(ClientV1::ClaudeCode, EventV1::PreToolUse, "{\"n\":1}"),
    )
    .await;
    let one = broker.hook().await;
    assert_eq!(one.seq, 1);
    assert_eq!(one.submit.payload, "{\"n\":1}");
    let mut second = client(&root).await;
    send_submit(
        &mut second,
        submit(ClientV1::Codex, EventV1::UserPromptSubmit, "{\"n\":2}"),
    )
    .await;
    let two = broker.hook().await;
    assert_eq!((two.seq, two.submit.client), (2, ClientV1::Codex));
    // Acks out of order still reach the client that submitted each seq.
    broker.ack(2, StatusV1::Refused).await;
    broker.ack(1, StatusV1::Recorded).await;
    assert_eq!(reply(&mut second).await, Some(StatusV1::Refused));
    assert_eq!(reply(&mut first).await, Some(StatusV1::Recorded));
    // A second ack for the same seq is a broker protocol error.
    broker.ack(1, StatusV1::Recorded).await;
    assert_eq!(join(task).await, Err(HelperError::Protocol));
    assert!(!root.path().join(SOCKET_NAME).exists());
}

#[tokio::test]
async fn the_pending_capacity_refuses_one_more_hook_and_reports_it() {
    let (_temporary, root) = fixture_root();
    let (mut broker, task) = Broker::start(root.clone());
    broker.ready().await;
    let mut waiting = Vec::new();
    for index in 0..MAX_PENDING_HOOKS {
        let mut stream = client(&root).await;
        send_submit(
            &mut stream,
            submit(
                ClientV1::ClaudeCode,
                EventV1::PreToolUse,
                &index.to_string(),
            ),
        )
        .await;
        let hook = broker.hook().await;
        assert_eq!(hook.seq, index as u64 + 1);
        waiting.push((hook.seq, stream));
    }
    let mut extra = client(&root).await;
    assert_eq!(reply(&mut extra).await, Some(StatusV1::Refused));
    assert_eq!(
        broker.next().await,
        Some(HelperFrameV1::Loss(LossV1 {
            seq: MAX_PENDING_HOOKS as u64 + 1,
            count: 1,
            reason: LossReasonV1::HelperCapacity
        }))
    );
    for (seq, _) in &waiting {
        broker.ack(*seq, StatusV1::Recorded).await;
    }
    for (_, stream) in &mut waiting {
        assert_eq!(reply(stream).await, Some(StatusV1::Recorded));
    }
    drop(broker);
    assert_eq!(join(task).await, Ok(()));
}

#[tokio::test]
async fn oversized_and_invalid_client_frames_are_refused_and_counted() {
    let (_temporary, root) = fixture_root();
    let (mut broker, task) = Broker::start(root.clone());
    broker.ready().await;
    let mut oversized = client(&root).await;
    oversized
        .write_all(&((MAX_SUBMIT_FRAME_BYTES + 1) as u32).to_be_bytes())
        .await
        .unwrap();
    assert_eq!(reply(&mut oversized).await, Some(StatusV1::Refused));
    let mut invalid = client(&root).await;
    invalid
        .write_all(&framed(b"{\"type\":\"submit\",\"version\":1}"))
        .await
        .unwrap();
    assert_eq!(reply(&mut invalid).await, Some(StatusV1::Refused));
    let mut reported = Vec::new();
    while reported.len() < 2 {
        match broker.next().await {
            Some(HelperFrameV1::Loss(loss)) => reported.push((loss.reason, loss.count)),
            other => panic!("unexpected frame {other:?}"),
        }
    }
    reported.sort_by_key(|(reason, _)| format!("{reason:?}"));
    assert_eq!(
        reported,
        [
            (LossReasonV1::InvalidFrame, 1),
            (LossReasonV1::OversizedFrame, 1)
        ]
    );
    drop(broker);
    assert_eq!(join(task).await, Ok(()));
}

#[tokio::test]
async fn closing_broker_stdin_refuses_waiting_hooks_and_ends_the_helper() {
    let (_temporary, root) = fixture_root();
    let (mut broker, task) = Broker::start(root.clone());
    broker.ready().await;
    let mut waiting = client(&root).await;
    send_submit(
        &mut waiting,
        submit(ClientV1::ClaudeCode, EventV1::UserPromptSubmit, "{}"),
    )
    .await;
    assert_eq!(broker.hook().await.seq, 1);
    drop(broker.output.take());
    assert_eq!(reply(&mut waiting).await, Some(StatusV1::Refused));
    assert_eq!(join(task).await, Ok(()));
    assert!(!root.path().join(SOCKET_NAME).exists());
    assert_eq!(broker.next().await, None);
}

#[tokio::test]
async fn loss_markers_are_counted_removed_and_reported_while_other_entries_stay() {
    let (_temporary, root) = fixture_root();
    for _ in 0..3 {
        hook::leave_loss_marker(root.path());
    }
    let loss = root.path().join(LOSS_DIRECTORY);
    std::fs::write(loss.join("notes"), b"kept").unwrap();
    assert_eq!(std::fs::read_dir(&loss).unwrap().count(), 4);
    let (mut broker, task) = Broker::start(root.clone());
    broker.ready().await;
    assert_eq!(
        broker.next().await,
        Some(HelperFrameV1::Loss(LossV1 {
            seq: 1,
            count: 3,
            reason: LossReasonV1::ClientUnrecorded
        }))
    );
    let names: Vec<_> = std::fs::read_dir(&loss)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, [OsString::from("notes")]);
    hook::leave_loss_marker(root.path());
    assert_eq!(
        broker.next().await,
        Some(HelperFrameV1::Loss(LossV1 {
            seq: 2,
            count: 1,
            reason: LossReasonV1::ClientUnrecorded
        }))
    );
    drop(broker);
    assert_eq!(join(task).await, Ok(()));
}

#[tokio::test]
async fn a_replaced_socket_ends_the_helper_instead_of_serving_elsewhere() {
    let (_temporary, root) = fixture_root();
    let (mut broker, task) = Broker::start(root.clone());
    broker.ready().await;
    drop(client(&root).await);
    let socket = root.path().join(SOCKET_NAME);
    std::fs::remove_file(&socket).unwrap();
    let _impostor = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    assert_eq!(join(task).await, Err(HelperError::UnsafeFilesystem));
}

// --------------------------------------------- hook client against a helper

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hook_client_submits_bounded_stdin_and_follows_the_broker_status() {
    let (_temporary, root) = fixture_root();
    let (mut broker, task) = Broker::start(root.clone());
    broker.ready().await;
    let settings = HookSettings {
        root: root.path().to_owned(),
        ..HookSettings::production()
    };
    // 1 MiB + 100 bytes without JSON escapes: exactly 1 MiB is kept, the rest
    // is counted and the payload flagged truncated.
    let mut input = b"{".to_vec();
    input.resize(MAX_PAYLOAD_BYTES + 100, b'x');
    let submitted = {
        let settings = settings.clone();
        let input = input.clone();
        tokio::spawn(async move {
            let mut stderr = Vec::new();
            let code = hook::run_async(
                &settings,
                &arguments("claude-code PreToolUse"),
                std::io::Cursor::new(input),
                &mut stderr,
            )
            .await;
            (code, stderr)
        })
    };
    let hook = broker.hook().await;
    assert_eq!(hook.submit.client, ClientV1::ClaudeCode);
    assert_eq!(hook.submit.event, EventV1::PreToolUse);
    assert!(hook.submit.gate && hook.submit.stdin_truncated);
    assert_eq!(hook.submit.stdin_total_bytes, input.len() as u64);
    assert_eq!(hook.submit.payload.as_bytes(), &input[..MAX_PAYLOAD_BYTES]);
    broker.ack(hook.seq, StatusV1::Recorded).await;
    let (code, stderr) = timeout(WAIT, submitted).await.unwrap().unwrap();
    assert_eq!((code, stderr.as_slice()), (EXIT_CONTINUE, b"".as_slice()));

    let refused = {
        let settings = settings.clone();
        tokio::spawn(async move {
            let mut stderr = Vec::new();
            let code = hook::run_async(
                &settings,
                &arguments("codex UserPromptSubmit"),
                std::io::Cursor::new(b"{\"prompt\":\"hi\"}".to_vec()),
                &mut stderr,
            )
            .await;
            (code, stderr)
        })
    };
    let hook = broker.hook().await;
    assert_eq!(hook.submit.payload, "{\"prompt\":\"hi\"}");
    broker.ack(hook.seq, StatusV1::Refused).await;
    let (code, stderr) = timeout(WAIT, refused).await.unwrap().unwrap();
    assert_eq!((code, stderr.as_slice()), (EXIT_BLOCK, BLOCKED_LINE));
    drop(broker);
    assert_eq!(join(task).await, Ok(()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unacked_gate_blocks_at_its_deadline() {
    let (_temporary, root) = fixture_root();
    let (mut broker, task) = Broker::start(root.clone());
    broker.ready().await;
    let settings = HookSettings {
        root: root.path().to_owned(),
        ..HookSettings::production()
    };
    let started = Instant::now();
    let gate = tokio::spawn(async move {
        let mut stderr = Vec::new();
        let code = hook::run_async(
            &settings,
            &arguments("claude-code PreToolUse"),
            std::io::Cursor::new(b"{}".to_vec()),
            &mut stderr,
        )
        .await;
        (code, stderr)
    });
    let hook = broker.hook().await;
    let (code, stderr) = timeout(Duration::from_secs(40), gate)
        .await
        .unwrap()
        .unwrap();
    let elapsed = started.elapsed();
    assert_eq!((code, stderr.as_slice()), (EXIT_BLOCK, BLOCKED_LINE));
    assert!(elapsed >= hook::GATE_DEADLINE, "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(30), "{elapsed:?}");
    // The late ack is still routed without failing the helper.
    broker.ack(hook.seq, StatusV1::Recorded).await;
    drop(broker);
    assert_eq!(join(task).await, Ok(()));
}

// ------------------------------------------------ hook client as a process

struct ProcessResult {
    code: Option<i32>,
    stdout_after_start: Vec<u8>,
    stderr: Vec<u8>,
    elapsed: Duration,
}

/// Run the real hook entry in a child process (this test binary, ignored
/// child below) with real stdin/stdout/stderr pipes and exit code.
async fn hook_process(root: &Path, args: &str, stdin: &[u8]) -> ProcessResult {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "activity_channel::tests::hook_process_child",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env(QA_ROOT, root)
        .env(QA_ARGS, args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let started = Instant::now();
    let mut child = command.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(stdin).await.unwrap();
    drop(input);
    let output = timeout(Duration::from_secs(60), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    let elapsed = started.elapsed();
    let start = output
        .stdout
        .windows(QA_MARKER.len())
        .position(|window| window == QA_MARKER)
        .expect("child reached the hook entry");
    ProcessResult {
        code: output.status.code(),
        stdout_after_start: output.stdout[start + QA_MARKER.len()..].to_vec(),
        stderr: output.stderr,
        elapsed,
    }
}

#[tokio::test]
async fn a_gate_process_without_helper_blocks_with_one_line_and_no_stdout() {
    let (_temporary, root) = fixture_root();
    let result = hook_process(
        root.path(),
        "claude-code PreToolUse",
        b"{\"tool_name\":\"Bash\"}",
    )
    .await;
    assert_eq!(result.code, Some(EXIT_BLOCK));
    assert_eq!(result.stderr, BLOCKED_LINE);
    assert!(result.stdout_after_start.is_empty());
    assert!(
        result.elapsed >= hook::GATE_DEADLINE,
        "{:?}",
        result.elapsed
    );
    assert!(
        result.elapsed < Duration::from_secs(30),
        "{:?}",
        result.elapsed
    );
    assert!(!root.path().join(LOSS_DIRECTORY).exists());
}

#[tokio::test]
async fn a_non_gate_process_without_helper_continues_and_leaves_one_marker() {
    let (_temporary, root) = fixture_root();
    let result = hook_process(root.path(), "codex Stop", b"{\"stop\":true}").await;
    assert_eq!(result.code, Some(EXIT_CONTINUE));
    assert!(result.stderr.is_empty());
    assert!(result.stdout_after_start.is_empty());
    assert!(result.elapsed < hook::GATE_DEADLINE, "{:?}", result.elapsed);
    let markers: Vec<_> = std::fs::read_dir(root.path().join(LOSS_DIRECTORY))
        .unwrap()
        .map(|entry| entry.unwrap())
        .collect();
    assert_eq!(markers.len(), 1);
    let name = markers[0].file_name().into_string().unwrap();
    assert!(name.starts_with("u-") && name.len() == 34);
    let facts = markers[0].metadata().unwrap();
    assert_eq!(
        (facts.len(), facts.permissions().mode() & 0o7777),
        (0, 0o600)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recorded_gate_process_exits_zero_without_any_output() {
    let (_temporary, root) = fixture_root();
    let (mut broker, task) = Broker::start(root.clone());
    broker.ready().await;
    let path = root.path().to_owned();
    let process =
        tokio::spawn(async move { hook_process(&path, "codex PreToolUse", b"{\"cmd\":1}").await });
    let hook = broker.hook().await;
    assert_eq!(hook.submit.payload, "{\"cmd\":1}");
    assert_eq!(hook.submit.stdin_total_bytes, 9);
    broker.ack(hook.seq, StatusV1::Recorded).await;
    let result = process.await.unwrap();
    assert_eq!(result.code, Some(EXIT_CONTINUE));
    assert!(result.stderr.is_empty());
    assert!(result.stdout_after_start.is_empty());
    drop(broker);
    assert_eq!(join(task).await, Ok(()));
}

/// Internal child of the process tests above; not a provider qualification.
#[test]
#[ignore = "internal child process of the activity hook process tests"]
fn hook_process_child() {
    let root = std::env::var_os(QA_ROOT).expect("QA root");
    let args = std::env::var(QA_ARGS).expect("QA arguments");
    let settings = HookSettings {
        root: PathBuf::from(root),
        ..HookSettings::production()
    };
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(QA_MARKER).unwrap();
    stdout.flush().unwrap();
    drop(stdout);
    let code = hook::run(
        &settings,
        &arguments(&args),
        std::io::stdin(),
        &mut std::io::stderr(),
    );
    std::process::exit(code);
}
