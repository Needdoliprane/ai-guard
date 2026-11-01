//! Real CLI route/closed-error regression. No engine, volume, configuration,
//! certificate or genuine credential is involved in this invalid-input case.
mod support;

use agentsandbox::runtime::capture_stream;
use std::{process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command};

#[tokio::test]
async fn internal_model_tls_import_rejects_invalid_stdin_without_echo() {
    const CANARY: &str = "fixture-model-tls-stdin-canary-no-real-credential";
    const CLOSED_REFUSAL: &[u8] = b"Error: model TLS import refused\n";
    let directory = support::private_tempdir();
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .arg("__model-tls-import")
        .env_clear()
        .current_dir(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("start the existing product CLI");
    let mut stdin = child.stdin.take().expect("owned CLI stdin");
    let stdout = child.stdout.take().expect("owned CLI stdout");
    let stderr = child.stderr.take().expect("owned CLI stderr");
    let packet = format!("invalid import packet: {CANARY}\n");
    let observations = tokio::time::timeout(Duration::from_secs(5), async {
        // Inline futures have one owner: no spawned collector/writer can
        // outlive this scope if a timeout or fixture error cancels the work.
        tokio::join!(
            async move {
                let sent = stdin.write_all(packet.as_bytes()).await;
                let closed = stdin.shutdown().await;
                drop(stdin); // EOF even if the baseline exits before reading.
                sent.is_ok() && closed.is_ok()
            },
            child.wait(),
            capture_stream(stdout, 4096),
            capture_stream(stderr, 4096),
        )
    })
    .await;
    let (input_closed, status, stdout, stderr) = match observations {
        Ok(observations) => observations,
        Err(_) => {
            let _ = child.start_kill();
            let reaped = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            assert!(
                reaped.is_ok_and(|result| result.is_ok()),
                "reap this test's timed-out CLI child"
            );
            panic!("the invalid-input CLI did not finish within its fixture budget");
        }
    };
    let status = status.expect("join the owned CLI child");
    let stdout = stdout.expect("collect bounded CLI stdout");
    let stderr = stderr.expect("collect bounded CLI stderr");
    // Cleanup/EOF and all observations precede the product oracle. Captures
    // remain private: diagnostics print only status/counts, never their body.
    drop(directory);
    eprintln!(
        "model TLS invalid stdin: exit={:?} input_closed={input_closed} stdout_bytes={} stderr_bytes={} stdout_truncated={} stderr_truncated={}",
        status.code(),
        stdout.total_bytes,
        stderr.total_bytes,
        stdout.truncated,
        stderr.truncated
    );
    assert!(
        !stdout.truncated && !stderr.truncated,
        "CLI output must remain within its fixture bound"
    );
    assert!(
        !stdout
            .bytes
            .windows(CANARY.len())
            .chain(stderr.bytes.windows(CANARY.len()))
            .any(|bytes| bytes == CANARY.as_bytes()),
        "invalid input must not be echoed"
    );
    assert_eq!(
        status.code(),
        Some(1),
        "the internal import route must reject invalid stdin with exit1; baseline exit2 records the missing command route, not a vulnerability"
    );
    assert!(
        input_closed,
        "deliver the invalid packet and close stdin before refusal"
    );
    assert!(
        stdout.bytes.is_empty(),
        "a rejected import must not emit an ACK"
    );
    assert!(
        stderr.bytes == CLOSED_REFUSAL,
        "the importer must return its fixed opaque refusal"
    );
}
