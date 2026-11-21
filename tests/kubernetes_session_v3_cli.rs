//! Real `ai-guard-provider` binary on real pipes for the two protocol v3
//! modes (G11 K3): argument dispatch and the refusal exit codes reachable
//! without a credential, a workspace or a provider. The Hello is a synthetic,
//! non-secret fixture independent of the production encoder; the Ready is a
//! declared echo. No worker image, container, attach transport, cluster,
//! provider CLI or model is exercised.
#![cfg(unix)]

use serde_json::{Value, json};
use std::{os::unix::ffi::OsStringExt, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

const SESSION: &str = "kubernetes-session-v3";
const EXPORT: &str = "kubernetes-workspace-export-v3";
const WATCHDOG: Duration = Duration::from_secs(20);
/// Never a real credential: it must not reach any output.
const CANARY: &str = "k3-cli-environment-canary-41f2";

fn hello() -> Vec<u8> {
    let body = serde_json::to_vec(&json!({
        "kind": "hello",
        "version": 2,
        "exchange_id": "00000000-0000-4000-8000-000000000008",
        "binding": {
            "version": 2,
            "installation_id": "00000000-0000-4000-8000-000000000001",
            "control_namespace_uuid": "00000000-0000-4000-8000-000000000002",
            "broker_id": "00000000-0000-4000-8000-000000000003",
            "incarnation_id": "00000000-0000-4000-8000-000000000004",
            "session_id": "00000000-0000-4000-8000-000000000005",
            "attempt_id": "00000000-0000-4000-8000-000000000006",
            "reservation_id": "00000000-0000-4000-8000-000000000007",
            "reservation_input_hash": "a".repeat(64),
            "authority_fingerprint": "b".repeat(64),
            "profile_hash": "c".repeat(64),
            "preparation_sha256": "d".repeat(64),
        }
    }))
    .unwrap();
    let mut envelope = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
    envelope.extend_from_slice(&body);
    envelope
}

/// One v3 frame, built here rather than by the code under test.
fn frame(channel: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = u32::try_from(payload.len() + 1)
        .unwrap()
        .to_be_bytes()
        .to_vec();
    bytes.push(channel);
    bytes.extend_from_slice(payload);
    bytes
}

struct Ran {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn run(args: Vec<std::ffi::OsString>, input: &[u8]) -> Ran {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ai-guard-provider"))
        .args(args)
        .env_clear()
        .env("AI_GUARD_PROVIDER_BOOTSTRAP", CANARY)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let feed = async {
        // The child may refuse before reading: a closed pipe is not an error.
        let _ = stdin.write_all(input).await;
        drop(stdin);
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (_, read_out, read_err, status) = tokio::time::timeout(WATCHDOG, async {
        tokio::join!(
            feed,
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err),
            child.wait()
        )
    })
    .await
    .expect("the binary ended within the test watchdog");
    read_out.unwrap();
    read_err.unwrap();
    let ran = Ran {
        code: status.unwrap().code(),
        stdout: out,
        stderr: err,
    };
    for bytes in [&ran.stdout, &ran.stderr] {
        assert!(
            !bytes
                .windows(CANARY.len())
                .any(|window| window == CANARY.as_bytes()),
            "the handoff environment reached an output"
        );
    }
    ran
}

fn args(values: &[&str]) -> Vec<std::ffi::OsString> {
    values.iter().map(Into::into).collect()
}

/// Splits stdout into the Ready envelope and the v3 frames after it.
fn after_ready(stdout: &[u8]) -> (Value, Vec<(u8, Value)>) {
    let length = u32::from_be_bytes(stdout[..4].try_into().unwrap()) as usize;
    let ready: Value = serde_json::from_slice(&stdout[4..4 + length]).unwrap();
    let mut rest = &stdout[4 + length..];
    let mut frames = Vec::new();
    while !rest.is_empty() {
        let length = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
        let channel = rest[4];
        let payload: Value = serde_json::from_slice(&rest[5..4 + length]).unwrap();
        frames.push((channel, payload));
        rest = &rest[4 + length..];
    }
    (ready, frames)
}

#[tokio::test]
async fn an_argument_after_either_v3_mode_exits_64_before_reading_anything() {
    for mode in [SESSION, EXPORT] {
        let ran = run(args(&[mode, "--help"]), &hello()).await;
        assert_eq!(ran.code, Some(64));
        assert!(ran.stdout.is_empty());
        let invalid = vec![mode.into(), std::ffi::OsString::from_vec(vec![0xff])];
        let ran = run(invalid, b"").await;
        assert_eq!(ran.code, Some(64));
        assert!(ran.stdout.is_empty());
    }
}

#[tokio::test]
async fn an_input_that_ends_before_the_handoff_exits_66_without_any_frame() {
    for mode in [SESSION, EXPORT] {
        let ran = run(args(&[mode]), b"").await;
        assert_eq!(
            ran.code,
            Some(66),
            "{}",
            String::from_utf8_lossy(&ran.stderr)
        );
        assert!(ran.stdout.is_empty());
        let ran = run(args(&[mode]), &hello()).await;
        assert_eq!(
            ran.code,
            Some(66),
            "{}",
            String::from_utf8_lossy(&ran.stderr)
        );
        let (ready, frames) = after_ready(&ran.stdout);
        assert_eq!(ready["kind"], "ready");
        assert!(frames.is_empty());
    }
}

#[tokio::test]
async fn a_frame_out_of_order_is_refused_with_a_closed_reason_and_exit_64() {
    let start = frame(0, br#"{"kind":"start","version":3}"#);
    let unknown_channel = frame(9, b"x");
    for (mode, input) in [
        (SESSION, start.clone()),
        (SESSION, unknown_channel.clone()),
        (EXPORT, unknown_channel),
        (EXPORT, frame(1, b"stdin before start")),
    ] {
        let mut bytes = hello();
        bytes.extend_from_slice(&input);
        let ran = run(args(&[mode]), &bytes).await;
        assert_eq!(
            ran.code,
            Some(64),
            "{}",
            String::from_utf8_lossy(&ran.stderr)
        );
        let (_, frames) = after_ready(&ran.stdout);
        assert_eq!(
            frames,
            [(0, json!({"kind":"refused","version":3,"reason":"protocol"}))]
        );
    }
}

#[tokio::test]
async fn a_handoff_for_another_profile_is_refused_before_any_effect() {
    // A well-formed handoff whose profile cannot hash to the Hello's.
    let handoff = serde_json::to_vec(&json!({
        "kind": "handoff",
        "version": 3,
        "profile": "{\"version\":2}",
        "bootstrap": format!("{{\"version\":1,\"client\":\"codex\",\"auth_mode\":\"api\",\"secret\":\"{CANARY}\"}}"),
        "program": "/usr/local/bin/ai-guard-provider",
        "args": ["exec"],
        "archive_bytes": 0,
        "archive_sha256": "e".repeat(64),
    }))
    .unwrap();
    let mut bytes = hello();
    bytes.extend_from_slice(&frame(0, &handoff));
    let ran = run(args(&[SESSION]), &bytes).await;
    assert_eq!(
        ran.code,
        Some(67),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let (_, frames) = after_ready(&ran.stdout);
    assert_eq!(
        frames,
        [(
            0,
            json!({"kind":"refused","version":3,"reason":"profile_mismatch"})
        )]
    );
}
