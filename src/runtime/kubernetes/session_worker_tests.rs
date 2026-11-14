//! The real waiting mode on tokio duplex pipes and in real time. Tokio's
//! paused clock needs its `test-util` feature, which this crate does not
//! enable, so the two timing tests run the same code with a shorter handshake
//! window and pin the product window as a constant. The frames are written
//! here by hand, so no oracle comes from the encoder under test. Fixtures
//! prove no Pod, attach stream, node log or cluster behaviour.
use super::*;
use crate::runtime::kubernetes::bootstrap::MAX_FRAME_BYTES;
use serde_json::{Value, json};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncWriteExt, DuplexStream, ReadBuf};

/// Larger than any input of these tests: a write never waits for the worker.
const PIPE: usize = 64 * 1024;
/// Ceiling for an end that must come. It elapses only when a test fails.
const GUARD: Duration = Duration::from_secs(5);
/// Shortened handshake window of the two timing tests.
const WINDOW: Duration = Duration::from_millis(300);
const EXCHANGE: &str = "5f0c2a9e-3b7d-4c1a-9e42-6d8b1f3a7c05";

fn binding() -> Value {
    json!({
        "version": 2,
        "installation_id": "0a1b2c3d-0001-4a00-8a00-000000000001",
        "control_namespace_uuid": "0a1b2c3d-0002-4a00-8a00-000000000002",
        "broker_id": "0a1b2c3d-0003-4a00-8a00-000000000003",
        "incarnation_id": "0a1b2c3d-0004-4a00-8a00-000000000004",
        "session_id": "0a1b2c3d-0005-4a00-8a00-000000000005",
        "attempt_id": "0a1b2c3d-0006-4a00-8a00-000000000006",
        "reservation_id": "0a1b2c3d-0007-4a00-8a00-000000000007",
        "reservation_input_hash": "1".repeat(64),
        "authority_fingerprint": "2".repeat(64),
        "profile_hash": "a".repeat(64),
        "preparation_sha256": "f".repeat(64)
    })
}
fn frame(kind: &str) -> Value {
    json!({"kind": kind, "version": 2, "exchange_id": EXCHANGE, "binding": binding()})
}
/// Four-byte big-endian length, then the body.
fn envelope(body: &[u8]) -> Vec<u8> {
    let mut bytes = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
    bytes.extend_from_slice(body);
    bytes
}
fn document(value: &Value) -> Vec<u8> {
    envelope(&serde_json::to_vec(value).unwrap())
}
/// `written` is exactly one envelope, and its body is the Ready that echoes
/// the exchange id and the binding of `frame("hello")`.
fn assert_exactly_one_ready(written: &[u8]) {
    assert!(written.len() > 4, "a Ready envelope was expected");
    let declared = u32::from_be_bytes([written[0], written[1], written[2], written[3]]);
    assert_eq!(usize::try_from(declared).unwrap(), written.len() - 4);
    let ready: Value = serde_json::from_slice(&written[4..]).unwrap();
    assert_eq!(ready, frame("ready"));
}
/// Reads one whole envelope that the worker wrote, prefix included.
async fn read_envelope(observed: &mut DuplexStream) -> Vec<u8> {
    let mut prefix = [0u8; 4];
    observed.read_exact(&mut prefix).await.unwrap();
    let mut bytes = prefix.to_vec();
    bytes.resize(4 + usize::try_from(u32::from_be_bytes(prefix)).unwrap(), 0);
    observed.read_exact(&mut bytes[4..]).await.unwrap();
    bytes
}
/// Serves `input` on duplex pipes until the worker ends, and returns its end
/// with every byte it wrote. With `close_input` the input ends after its
/// bytes; otherwise it stays open until the worker has returned.
async fn run_worker(
    input: &[u8],
    close_input: bool,
) -> (Result<WorkerBootstrapEndV2, BootstrapError>, Vec<u8>) {
    let (mut controller, worker_input) = tokio::io::duplex(PIPE);
    let (worker_output, mut observed) = tokio::io::duplex(PIPE);
    controller.write_all(input).await.unwrap();
    let held_open = if close_input {
        drop(controller);
        None
    } else {
        Some(controller)
    };
    let result = tokio::time::timeout(
        GUARD,
        serve_waiting_bootstrap(worker_input, worker_output, worker_capacity().unwrap()),
    )
    .await
    .expect("the worker must end on this input");
    drop(held_open);
    let mut written = Vec::new();
    observed.read_to_end(&mut written).await.unwrap();
    (result, written)
}

/// Yields its scripted bytes, then fails with an error that is not an end of
/// input.
struct FailingReader {
    bytes: Vec<u8>,
    sent: usize,
}
impl AsyncRead for FailingReader {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let rest = &this.bytes[this.sent..];
        if rest.is_empty() {
            return Poll::Ready(Err(std::io::Error::other("scripted read failure")));
        }
        let count = rest.len().min(buffer.remaining());
        buffer.put_slice(&rest[..count]);
        this.sent += count;
        Poll::Ready(Ok(()))
    }
}
/// Refuses every write.
struct FailingWriter;
impl AsyncWrite for FailingWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[test]
fn exit_codes_never_report_success_without_a_handoff() {
    assert_eq!(
        (EXIT_PROTOCOL, EXIT_DEADLINE, EXIT_NO_HANDOFF, EXIT_IO),
        (64, 65, 66, 70)
    );
    let ends: [(Result<WorkerBootstrapEndV2, BootstrapError>, i32); 10] = [
        (Ok(WorkerBootstrapEndV2::EofBeforeHello), 66),
        (Ok(WorkerBootstrapEndV2::ReadyThenEof), 66),
        (Err(BootstrapError::Protocol), 64),
        (Err(BootstrapError::FrameSize), 64),
        (Err(BootstrapError::PartialFrame), 64),
        (Err(BootstrapError::Binding), 64),
        (Err(BootstrapError::Deadline), 65),
        (Err(BootstrapError::Io), 70),
        (Err(BootstrapError::Capacity), 70),
        (Err(BootstrapError::Unavailable), 70),
    ];
    for (end, code) in ends {
        assert_eq!(waiting_exit_code(&end), code, "{end:?}");
        assert_ne!(waiting_exit_code(&end), 0, "{end:?}");
        assert!(!waiting_diagnostic(&end).is_empty(), "{end:?}");
    }
}

#[tokio::test]
async fn ready_then_eof_ends_without_handoff() {
    let hello = document(&frame("hello"));
    let (result, written) = run_worker(&hello, true).await;
    assert_eq!(result, Ok(WorkerBootstrapEndV2::ReadyThenEof));
    assert_exactly_one_ready(&written);
    assert_eq!(waiting_exit_code(&result), EXIT_NO_HANDOFF);

    // The same Hello in seven-byte pieces: the first byte, the rest of the
    // prefix and the body each arrive across several reads.
    let (mut controller, worker_input) = tokio::io::duplex(PIPE);
    let (worker_output, mut observed) = tokio::io::duplex(PIPE);
    let feed = async move {
        for piece in hello.chunks(7) {
            controller.write_all(piece).await.unwrap();
            tokio::task::yield_now().await;
        }
        drop(controller);
    };
    let (result, ()) = tokio::time::timeout(GUARD, async {
        tokio::join!(
            serve_waiting_bootstrap(worker_input, worker_output, worker_capacity().unwrap()),
            feed
        )
    })
    .await
    .expect("a Hello in pieces, then the end of the input, must end the worker");
    assert_eq!(result, Ok(WorkerBootstrapEndV2::ReadyThenEof));
    let mut written = Vec::new();
    observed.read_to_end(&mut written).await.unwrap();
    assert_exactly_one_ready(&written);
}

/// A short real wait shows two things only: the worker is still pending after
/// more than the handshake window, and it wrote nothing after Ready. It cannot
/// show that no longer timer exists. That the path after Ready arms no timer
/// is a property of the code, read in `serve_within` and `end_after_ready`.
#[tokio::test]
async fn ready_then_silence_stays_pending_and_is_never_a_success() {
    let (mut controller, worker_input) = tokio::io::duplex(PIPE);
    let (worker_output, mut observed) = tokio::io::duplex(PIPE);
    controller
        .write_all(&document(&frame("hello")))
        .await
        .unwrap();
    let mut serving = Box::pin(serve_within(
        worker_input,
        worker_output,
        worker_capacity().unwrap(),
        WINDOW,
    ));
    assert!(
        tokio::time::timeout(WINDOW * 2, serving.as_mut())
            .await
            .is_err(),
        "after Ready the worker keeps waiting: the handshake window no longer applies"
    );
    assert_exactly_one_ready(&read_envelope(&mut observed).await);
    let mut next = [0u8; 1];
    assert!(
        tokio::time::timeout(WINDOW, observed.read(&mut next))
            .await
            .is_err(),
        "no byte follows the Ready frame while the worker waits"
    );
    // Only the end of the input ends the wait, and that end is not a success.
    drop(controller);
    let result = tokio::time::timeout(GUARD, serving)
        .await
        .expect("the end of the input must end the worker");
    assert_eq!(result, Ok(WorkerBootstrapEndV2::ReadyThenEof));
    assert_eq!(waiting_exit_code(&result), EXIT_NO_HANDOFF);
    let mut rest = Vec::new();
    observed.read_to_end(&mut rest).await.unwrap();
    assert!(rest.is_empty());
}

#[tokio::test]
async fn any_byte_after_ready_is_a_protocol_refusal_and_nothing_runs() {
    let hello = document(&frame("hello"));
    let start = document(&json!({"kind": "start", "version": 2, "argv": ["/bin/sh"]}));
    for after_ready in [hello.as_slice(), start.as_slice(), b"{".as_slice()] {
        let input = [hello.as_slice(), after_ready].concat();
        // The input stays open: the refusal does not wait for its end.
        let (result, written) = run_worker(&input, false).await;
        assert_eq!(result, Err(BootstrapError::Protocol));
        assert_exactly_one_ready(&written);
        assert_eq!(waiting_exit_code(&result), EXIT_PROTOCOL);
    }

    // A byte that arrives while the worker already waits after Ready.
    let (mut controller, worker_input) = tokio::io::duplex(PIPE);
    let (worker_output, mut observed) = tokio::io::duplex(PIPE);
    controller.write_all(&hello).await.unwrap();
    let late_byte = async {
        let ready = read_envelope(&mut observed).await;
        controller.write_all(b"{").await.unwrap();
        ready
    };
    let (result, mut written) = tokio::time::timeout(GUARD, async {
        tokio::join!(
            serve_waiting_bootstrap(worker_input, worker_output, worker_capacity().unwrap()),
            late_byte
        )
    })
    .await
    .expect("one byte after Ready must end the worker");
    assert_eq!(result, Err(BootstrapError::Protocol));
    observed.read_to_end(&mut written).await.unwrap();
    assert_exactly_one_ready(&written);
    drop(controller);
}

#[tokio::test]
async fn eof_before_hello_and_truncated_frames_are_distinguished() {
    let hello = document(&frame("hello"));
    let (result, written) = run_worker(&[], true).await;
    assert_eq!(result, Ok(WorkerBootstrapEndV2::EofBeforeHello));
    assert_eq!(waiting_exit_code(&result), EXIT_NO_HANDOFF);
    assert!(written.is_empty());

    // The input ends inside the prefix, right after it, and inside the body.
    for cut in [1, 2, 4, 4 + (hello.len() - 4) / 2, hello.len() - 1] {
        let (result, written) = run_worker(&hello[..cut], true).await;
        assert_eq!(result, Err(BootstrapError::PartialFrame), "{cut} bytes");
        assert_eq!(waiting_exit_code(&result), EXIT_PROTOCOL);
        assert!(written.is_empty(), "{cut} bytes");
    }

    // A length outside the bound is refused from the prefix alone: no body is
    // ever sent and the input stays open.
    let oversized = u32::try_from(MAX_FRAME_BYTES + 1).unwrap().to_be_bytes();
    for prefix in [oversized, [0; 4], [0xff; 4]] {
        let (result, written) = run_worker(&prefix, false).await;
        assert_eq!(result, Err(BootstrapError::FrameSize), "{prefix:?}");
        assert_eq!(waiting_exit_code(&result), EXIT_PROTOCOL);
        assert!(written.is_empty(), "{prefix:?}");
    }
}

#[tokio::test]
async fn non_hello_first_frame_and_noncanonical_hello_are_refused() {
    let mut unknown_field = frame("hello");
    unknown_field["argv"] = json!(["/bin/sh"]);
    let mut unknown_binding_field = frame("hello");
    unknown_binding_field["binding"]["credential"] = json!("none");
    let mut uppercase_uuid = frame("hello");
    uppercase_uuid["exchange_id"] = json!(EXCHANGE.to_uppercase());
    // The same Hello with its `version` field written twice. A lenient parser
    // accepts that text, so its refusal comes from the duplicate-key rule.
    let hello_text = serde_json::to_string(&frame("hello")).unwrap();
    let duplicate_key = format!("{{\"version\":2,{}", &hello_text[1..]);
    assert_eq!(
        serde_json::from_str::<Value>(&duplicate_key).unwrap(),
        frame("hello")
    );
    let refused = [
        document(&frame("ready")),
        document(&unknown_field),
        document(&unknown_binding_field),
        envelope(duplicate_key.as_bytes()),
        document(&uppercase_uuid),
    ];
    for (case, input) in refused.iter().enumerate() {
        let (result, written) = run_worker(input, true).await;
        assert_eq!(result, Err(BootstrapError::Protocol), "case {case}");
        assert_eq!(waiting_exit_code(&result), EXIT_PROTOCOL);
        assert!(written.is_empty(), "case {case}");
    }
}

/// Real time on a shortened window. The product window is the pinned
/// constant, which `serve_waiting_bootstrap` passes; the literal five seconds
/// are not waited for here.
#[tokio::test]
async fn handshake_deadline_arms_on_the_first_byte_and_maps_to_65() {
    assert_eq!(MAX_HANDSHAKE_TIME, Duration::from_secs(5));
    let (mut controller, worker_input) = tokio::io::duplex(PIPE);
    let (worker_output, mut observed) = tokio::io::duplex(PIPE);
    let mut serving = Box::pin(serve_within(
        worker_input,
        worker_output,
        worker_capacity().unwrap(),
        WINDOW,
    ));
    assert!(
        tokio::time::timeout(WINDOW * 2, serving.as_mut())
            .await
            .is_err(),
        "silence before the first byte does not arm the handshake window"
    );
    // A complete prefix, then a body that never comes.
    let first_byte = std::time::Instant::now();
    controller.write_all(&512u32.to_be_bytes()).await.unwrap();
    let result = tokio::time::timeout(GUARD, serving)
        .await
        .expect("a stalled body must end at the handshake deadline");
    assert!(
        first_byte.elapsed() >= WINDOW,
        "the window counts from the first byte"
    );
    assert_eq!(result, Err(BootstrapError::Deadline));
    assert_eq!(waiting_exit_code(&result), EXIT_DEADLINE);
    let mut written = Vec::new();
    observed.read_to_end(&mut written).await.unwrap();
    assert!(written.is_empty(), "no Ready byte follows a stalled Hello");
    drop(controller);

    // The window parameter of the tests cannot extend the product window.
    let extended = serve_within(
        tokio::io::empty(),
        tokio::io::sink(),
        worker_capacity().unwrap(),
        MAX_HANDSHAKE_TIME + Duration::from_millis(1),
    )
    .await;
    assert_eq!(extended, Err(BootstrapError::Deadline));
}

/// The refusals run inside a Tokio runtime on purpose: `Runtime::block_on`
/// panics there, so a refusal that tried to serve the mode would not return.
/// The mode alone is only classified: serving it would read the standard
/// input of the test process. The calls with `Vec<String>` also pin that the
/// decoded form of the arguments is still accepted.
#[tokio::test]
async fn dispatch_recognizes_only_the_exact_mode_and_refuses_extra_arguments_without_fallback() {
    assert_eq!(SESSION_WAIT_MODE_ARG, "kubernetes-session-wait-v2");
    let argv = |values: &[&str]| -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    };
    let business = [
        "exec",
        "--client",
        "codex",
        "--auth-mode",
        "api",
        "--transport",
        "batch",
        "--preset-version",
        "1",
    ];
    let not_the_mode: [&[&str]; 7] = [
        &[],
        &business,
        &["exec", SESSION_WAIT_MODE_ARG],
        &["kubernetes-session-wait-v1"],
        &["Kubernetes-Session-Wait-V2"],
        &["kubernetes-session-wait-v2 "],
        &["--kubernetes-session-wait-v2"],
    ];
    for values in not_the_mode {
        assert_eq!(dispatch_waiting_mode(&argv(values)), None, "{values:?}");
        assert_eq!(
            waiting_mode_request(&argv(values)),
            WaitingModeRequest::Other
        );
    }
    let refused: [&[&str]; 4] = [
        &[SESSION_WAIT_MODE_ARG, "anything"],
        &[SESSION_WAIT_MODE_ARG, SESSION_WAIT_MODE_ARG],
        &[SESSION_WAIT_MODE_ARG, ""],
        &[SESSION_WAIT_MODE_ARG, "exec", "--client", "codex"],
    ];
    for values in refused {
        assert_eq!(
            dispatch_waiting_mode(&argv(values)),
            Some(EXIT_PROTOCOL),
            "{values:?}"
        );
    }
    assert_eq!(
        waiting_mode_request(&argv(&[SESSION_WAIT_MODE_ARG])),
        WaitingModeRequest::Serve
    );

    // Raw arguments, as the binary passes them before it decodes anything.
    // After the mode, an argument that is not valid UTF-8 is refused like any
    // other and never becomes the binary's own decoding error. As a first
    // argument it is not the mode, even when it begins with the mode's bytes.
    // Such a value can only be built from bytes on Unix.
    #[cfg(unix)]
    {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let not_utf8 = || OsString::from_vec(vec![b'f', b'o', 0x80]);
        let mode = || OsString::from(SESSION_WAIT_MODE_ARG);
        let mode_then_invalid_byte = || {
            let mut bytes = SESSION_WAIT_MODE_ARG.as_bytes().to_vec();
            bytes.push(0x80);
            OsString::from_vec(bytes)
        };
        assert!(not_utf8().to_str().is_none());
        assert!(mode_then_invalid_byte().to_str().is_none());
        assert_eq!(
            dispatch_waiting_mode(&[mode(), not_utf8()]),
            Some(EXIT_PROTOCOL)
        );
        assert_eq!(
            dispatch_waiting_mode(&[mode(), not_utf8(), mode()]),
            Some(EXIT_PROTOCOL)
        );
        assert_eq!(dispatch_waiting_mode(&[not_utf8()]), None);
        assert_eq!(dispatch_waiting_mode(&[not_utf8(), mode()]), None);
        assert_eq!(dispatch_waiting_mode(&[mode_then_invalid_byte()]), None);
        assert_eq!(waiting_mode_request(&[mode()]), WaitingModeRequest::Serve);
    }

    // Text tripwire on this module only: outside its comments it names no
    // environment accessor and no provider path. It says nothing of callees:
    // building the runtime of the served mode reads one Tokio setting.
    let code: Vec<&str> = include_str!("session_worker.rs")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect();
    for forbidden in ["env::", "env!", "provider::", "run_from_environment"] {
        assert!(
            code.iter().all(|line| !line.contains(forbidden)),
            "{forbidden}"
        );
    }
}

#[tokio::test]
async fn io_failures_map_to_70() {
    let hello = document(&frame("hello"));
    // A read failure at each point where the end of the input means
    // something else: no handoff, a truncated frame, or the end after Ready.
    let points = [
        (&hello[..0], "before the first byte"),
        (&hello[..1], "inside the prefix"),
        (&hello[..hello.len() - 1], "inside the body"),
        (hello.as_slice(), "after Ready"),
    ];
    for (scripted, point) in points {
        let reader = FailingReader {
            bytes: scripted.to_vec(),
            sent: 0,
        };
        let result =
            serve_waiting_bootstrap(reader, tokio::io::sink(), worker_capacity().unwrap()).await;
        assert_eq!(result, Err(BootstrapError::Io), "{point}");
        assert_eq!(waiting_exit_code(&result), EXIT_IO, "{point}");
    }

    let result =
        serve_waiting_bootstrap(hello.as_slice(), FailingWriter, worker_capacity().unwrap()).await;
    assert_eq!(result, Err(BootstrapError::Io));
    assert_eq!(waiting_exit_code(&result), EXIT_IO);
}
