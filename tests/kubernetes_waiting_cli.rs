//! Real `ai-guard-provider` standard streams, with no provider handoff or
//! credentials. The Ready is a declared echo, never a proof of identity or
//! execution. No worker, container, attach transport or cluster is exercised.
#![cfg(unix)]

use anyhow::Context;
use serde_json::Value;
use std::{
    ffi::OsString,
    fs, io,
    os::unix::{ffi::OsStringExt, fs::PermissionsExt},
    process::{Child, Command, ExitStatus, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{ChildStderr, ChildStdin, ChildStdout},
    time::{Instant, MissedTickBehavior, interval, timeout_at},
};

const MODE: &str = "kubernetes-session-wait-v2";
const SUPERVISOR: Duration = Duration::from_secs(15);
const STDOUT_CAP: usize = 16_388;
const STDERR_CAP: usize = 8_192;

// Synthetic, non-secret Hello from the deferred real-stdio acceptance plan.
// Its 847-byte body is independent of the production frame encoder.
const HELLO_BODY: &str = concat!(
    r#"{"kind":"hello","version":2,"exchange_id":"00000000-0000-4000-8000-000000000008","binding":{"version":2,"#,
    r#""installation_id":"00000000-0000-4000-8000-000000000001","#,
    r#""control_namespace_uuid":"00000000-0000-4000-8000-000000000002","#,
    r#""broker_id":"00000000-0000-4000-8000-000000000003","#,
    r#""incarnation_id":"00000000-0000-4000-8000-000000000004","#,
    r#""session_id":"00000000-0000-4000-8000-000000000005","#,
    r#""attempt_id":"00000000-0000-4000-8000-000000000006","#,
    r#""reservation_id":"00000000-0000-4000-8000-000000000007","#,
    r#""reservation_input_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","#,
    r#""authority_fingerprint":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","#,
    r#""profile_hash":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","#,
    r#""preparation_sha256":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"}}"#,
    "\n",
);

enum Input<'a> {
    Eof,
    ThenEof(&'a [u8]),
    HoldOpen(&'a [u8]),
}

struct Observed {
    status: ExitStatus,
    stdin_writer_open_at_exit: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Observed {
    fn assert_exit(&self, code: i32) {
        assert_eq!(
            self.status.code(),
            Some(code),
            "actual child status: {}; stderr: {}",
            self.status,
            String::from_utf8_lossy(&self.stderr)
        );
        assert!(!self.status.success(), "no waiting-mode end is a success");
    }

    fn assert_empty_stdout(&self, code: i32) {
        self.assert_exit(code);
        assert!(self.stdout.is_empty(), "unexpected standard output");
    }
}

// Own the actual std child so cancellation or unwinding can synchronously
// kill AND reap it, even when the Tokio runtime is being dropped. Pipe IO is
// asynchronous; no blocking reader task can outlive this owner.
struct OwnedChild {
    child: Child,
    stdin: Option<ChildStdin>,
    reaped: bool,
}

impl OwnedChild {
    async fn feed_and_wait(&mut self, input: Input<'_>) -> io::Result<(ExitStatus, bool)> {
        match input {
            Input::Eof => {}
            Input::ThenEof(bytes) | Input::HoldOpen(bytes) => {
                let stdin = self
                    .stdin
                    .as_mut()
                    .ok_or_else(|| io::Error::other("child stdin is unavailable"))?;
                stdin.write_all(bytes).await?;
                stdin.flush().await?;
            }
        }
        if !matches!(input, Input::HoldOpen(_)) {
            drop(self.stdin.take());
        }

        // Poll only for an observed terminal status. In particular no timer
        // drops a held writer to manufacture EOF or serve as the exit oracle.
        let mut ticks = interval(Duration::from_millis(10));
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            if let Some(status) = self.child.try_wait()? {
                let stdin_writer_open_at_exit = self.stdin.is_some();
                self.reaped = true;
                drop(self.stdin.take());
                return Ok((status, stdin_writer_open_at_exit));
            }
            ticks.tick().await;
        }
    }

    fn stop_and_reap(&mut self) -> io::Result<()> {
        drop(self.stdin.take());
        if self.reaped {
            return Ok(());
        }
        let observed = self.child.try_wait();
        if matches!(observed, Ok(Some(_))) {
            self.reaped = true;
            return Ok(());
        }
        // Always wait, including when kill reports an error: dropping Child
        // alone would not reap it. Any cleanup error fails the harness.
        let killed = self.child.kill();
        let waited = self.child.wait();
        self.reaped = waited.is_ok();
        waited?;
        killed?;
        observed.map(|_| ())
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Err(error) = self.stop_and_reap() {
            if !std::thread::panicking() {
                panic!("failed to kill/reap waiting-mode child: {error}");
            }
            use std::io::Write as _;
            let _ = writeln!(io::stderr().lock(), "child cleanup failed: {error}");
        }
    }
}

async fn capture<R: AsyncRead + Unpin>(
    mut pipe: R,
    cap: usize,
    stream: &'static str,
) -> io::Result<Vec<u8>> {
    // Allocate only the fixed cap plus one BEFORE reading. The extra byte
    // detects overflow; no read_to_end or unbounded allocation happens first.
    let mut bytes = vec![0; cap + 1];
    let mut used = 0;
    loop {
        let count = pipe.read(&mut bytes[used..]).await?;
        if count == 0 {
            bytes.truncate(used);
            return Ok(bytes);
        }
        used += count;
        if used > cap {
            return Err(io::Error::other(format!("{stream} exceeded {cap} bytes")));
        }
    }
}

async fn run_case(args: &[OsString], input: Input<'_>) -> anyhow::Result<Observed> {
    let root = tempfile::Builder::new()
        .prefix("ai-guard-kubernetes-waiting-cli-")
        .tempdir()?;
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))?;
    let home = root.path().join("home");
    let tmp = root.path().join("tmp");
    for path in [&home, &tmp] {
        fs::create_dir(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_ai-guard-provider"));
    command
        .args(args)
        .env_clear()
        .env("HOME", &home)
        .env("TMPDIR", &tmp)
        .env("TMP", &tmp)
        .env("TEMP", &tmp)
        .current_dir(root.path())
        .stdin(if matches!(input, Input::Eof) {
            Stdio::null()
        } else {
            Stdio::piped()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut owned = OwnedChild {
        child: command.spawn().context("spawn actual provider binary")?,
        stdin: None,
        reaped: false,
    };
    let deadline = Instant::now() + SUPERVISOR;
    owned.stdin = owned
        .child
        .stdin
        .take()
        .map(ChildStdin::from_std)
        .transpose()?;
    let stdout = ChildStdout::from_std(owned.child.stdout.take().context("child stdout")?)?;
    let stderr = ChildStderr::from_std(owned.child.stderr.take().context("child stderr")?)?;
    let result = timeout_at(deadline, async {
        tokio::try_join!(
            owned.feed_and_wait(input),
            capture(stdout, STDOUT_CAP, "stdout"),
            capture(stderr, STDERR_CAP, "stderr"),
        )
    })
    .await;

    // The joined futures (and their pipes) have ended before cleanup. This
    // also runs after timeout, cap overflow, input failure or a read failure.
    owned.stop_and_reap().context("kill/reap actual child")?;
    drop(owned);
    root.close()
        .context("remove only the fresh private fixture")?;
    let ((status, stdin_writer_open_at_exit), stdout, stderr) =
        result.context("15s child supervisor elapsed")??;
    Ok(Observed {
        status,
        stdin_writer_open_at_exit,
        stdout,
        stderr,
    })
}

#[tokio::test]
async fn mode_eof_before_hello_exits_66_without_stdout() -> anyhow::Result<()> {
    run_case(&[MODE.into()], Input::Eof)
        .await?
        .assert_empty_stdout(66);
    Ok(())
}

#[tokio::test]
async fn hello_then_eof_emits_one_correlated_declared_ready_and_exits_66() -> anyhow::Result<()> {
    let mut input = 847u32.to_be_bytes().to_vec();
    assert_eq!(HELLO_BODY.len(), 847);
    input.extend_from_slice(HELLO_BODY.as_bytes());
    let observed = run_case(&[MODE.into()], Input::ThenEof(&input)).await?;
    observed.assert_exit(66);

    assert!(observed.stdout.len() >= 4, "missing Ready envelope");
    let prefix: [u8; 4] = observed.stdout[..4].try_into()?;
    let body_len = usize::try_from(u32::from_be_bytes(prefix))?;
    assert!(
        (1..=16_384).contains(&body_len),
        "Ready length outside its bound"
    );
    assert_eq!(
        observed.stdout.len(),
        4 + body_len,
        "expected exactly one complete envelope"
    );
    let ready: Value = serde_json::from_slice(&observed.stdout[4..])?;
    let mut declared_echo: Value = serde_json::from_str(HELLO_BODY)?;
    declared_echo["kind"] = Value::String("ready".into());
    assert_eq!(
        ready, declared_echo,
        "Ready must echo version, exchange and full binding"
    );
    Ok(())
}

#[tokio::test]
async fn mode_with_extra_argument_exits_64_without_stdout() -> anyhow::Result<()> {
    run_case(&[MODE.into(), "x".into()], Input::Eof)
        .await?
        .assert_empty_stdout(64);
    Ok(())
}

#[tokio::test]
async fn mode_with_non_utf8_extra_argument_exits_64_without_stdout() -> anyhow::Result<()> {
    run_case(&[MODE.into(), OsString::from_vec(vec![0xff])], Input::Eof)
        .await?
        .assert_empty_stdout(64);
    Ok(())
}

#[tokio::test]
async fn truncated_prefix_then_eof_exits_64_without_stdout() -> anyhow::Result<()> {
    run_case(&[MODE.into()], Input::ThenEof(&[0]))
        .await?
        .assert_empty_stdout(64);
    Ok(())
}

#[tokio::test]
async fn held_prefix_exits_65_while_stdin_remains_open() -> anyhow::Result<()> {
    let observed = run_case(&[MODE.into()], Input::HoldOpen(&[0])).await?;
    observed.assert_empty_stdout(65);
    assert!(
        observed.stdin_writer_open_at_exit,
        "the actual stdin writer must remain owned until the child is terminal"
    );
    Ok(())
}

#[tokio::test]
async fn non_utf8_first_argument_exits_126_without_stdout() -> anyhow::Result<()> {
    run_case(&[OsString::from_vec(vec![0xff])], Input::Eof)
        .await?
        .assert_empty_stdout(126);
    Ok(())
}
