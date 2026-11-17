//! Physical-IO units only. These fixtures establish no engine/native authority.
//! Authored source is NOT_RUN here. The subprocess fixture is this Rust libtest
//! executable, not a shell or an impersonated Docker/Podman executable.
use super::*;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncWriteExt, ReadBuf};

struct FaultAfterData(bool);
impl AsyncRead for FaultAfterData {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if !self.0 {
            self.0 = true;
            buffer.put_slice(b"private-output");
            Poll::Ready(Ok(()))
        } else {
            // Deliberate unit FAULT_INJECTION, not an observed OS read failure.
            Poll::Ready(Err(io::Error::other("injected read fault")))
        }
    }
}

#[tokio::test]
async fn read_error_preserves_partial_capture_and_does_not_invent_eof() {
    let result = capture(Some(FaultAfterData(false)), 7).await;
    assert_eq!(result.capture.bytes, b"private");
    assert_eq!(result.capture.total_bytes, 14);
    assert!(result.capture.truncated);
    assert!(!result.eof);
    assert_eq!(result.error.unwrap().kind(), io::ErrorKind::Other);
}

#[derive(Debug)]
struct RetainedReadCause(Arc<()>);
impl fmt::Display for RetainedReadCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("typed fixture read cause")
    }
}
impl std::error::Error for RetainedReadCause {}

struct RetainedFaultAfterData {
    delivered: bool,
    error: Option<io::Error>,
}
impl AsyncRead for RetainedFaultAfterData {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if !self.delivered {
            self.delivered = true;
            buffer.put_slice(b"private-output");
            Poll::Ready(Ok(()))
        } else {
            // Deliberate unit fault through the actual AsyncRead error port,
            // with the same original error object moved into native custody.
            Poll::Ready(Err(self.error.take().expect("one read fault")))
        }
    }
}

#[tokio::test]
async fn retained_read_error_keeps_original_typed_cause_without_eof() {
    let marker = Arc::new(());
    let original = io::Error::new(
        io::ErrorKind::PermissionDenied,
        RetainedReadCause(Arc::clone(&marker)),
    );
    let original_cause = original
        .get_ref()
        .unwrap()
        .downcast_ref::<RetainedReadCause>()
        .unwrap() as *const RetainedReadCause;
    let mut reader = Some(RetainedFaultAfterData {
        delivered: false,
        error: Some(original),
    });
    let mut captured = empty_capture(7);
    assert!(!capture_retained(&mut reader, &mut captured, 7).await);
    let retained = captured.error.as_ref().unwrap();
    let retained_cause = retained
        .get_ref()
        .unwrap()
        .downcast_ref::<RetainedReadCause>()
        .unwrap();
    assert!(std::ptr::eq(original_cause, retained_cause));
    assert!(Arc::ptr_eq(&retained_cause.0, &marker));
    assert_eq!(retained.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(captured.capture.bytes, b"private");
    assert_eq!(captured.capture.total_bytes, 14);
    assert!(captured.capture.truncated);
    assert!(!captured.eof);
    // Neither the AsyncRead fixture nor this assertion creates a native receipt.
}

#[tokio::test]
async fn bounded_capture_keeps_draining_until_actual_duplex_eof() {
    let (mut writer, reader) = tokio::io::duplex(4);
    let mut writing = tokio::spawn(async move {
        writer.write_all(b"abcdefghijk").await?;
        writer.shutdown().await
    });
    let result = capture(Some(reader), 3).await;
    // Join the actual fixture writer before any assertions can fail.
    let written = (&mut writing).await;
    assert!(written.unwrap().is_ok());
    assert_eq!(result.capture.bytes, b"abc");
    assert_eq!(result.capture.total_bytes, 11);
    assert!(result.capture.truncated);
    assert!(result.eof);
    assert!(result.error.is_none());
}

fn fixture_command(mode: &str) -> tokio::process::Command {
    use std::process::Stdio;
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .env_clear()
        .args([
            "--exact",
            "runtime::batch_io::tests::fixture_child17",
            "--ignored",
            "--nocapture",
        ])
        .env("AI_GUARD_BATCH_IO_CHILD_FIXTURE17", mode)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

#[tokio::test]
async fn setup_fault_collects_real_child_and_retains_exact_original_input_fd() {
    use std::{io::Write, os::unix::fs::MetadataExt, process::Stdio};
    let mut input = tempfile::tempfile().unwrap();
    input.write_all(b"archive-fixture").unwrap();
    let before = input.metadata().unwrap();
    let child = fixture_command("explicit-unit-only")
        .stdin(Stdio::null()) // Actual setup fault: archive input has no pipe.
        .spawn()
        .unwrap();
    let mut owner = OwnedBatchIo::own(child, Some(input), 4096);
    let mut fault_observed = false;
    loop {
        match owner.wait_event().await {
            BatchIoEvent::Fault => {
                fault_observed = true;
                break;
            }
            BatchIoEvent::OwnerFinished => break,
            BatchIoEvent::Exited(_) => {}
        }
    }
    // Real collection is awaited before any assertion can fail.
    let collected = owner.collect().await.unwrap();
    let rendered = format!("{collected:?}");
    let retained = collected
        .retained_input
        .as_ref()
        .map(File::metadata)
        .transpose()
        .unwrap();
    assert!(fault_observed);
    assert!(collected.status.is_some_and(|status| status.success()));
    assert!(collected.stdout_eof && collected.stderr_eof);
    assert!(!collected.errors.is_empty());
    let retained = retained.unwrap();
    assert_eq!(
        (retained.dev(), retained.ino()),
        (before.dev(), before.ino())
    );
    assert!(!rendered.contains("native-private-stdout"));
    assert!(!rendered.contains("native-private-stderr"));
    assert!(matches!(
        owner.collect().await,
        Err(BatchIoCollectError::AlreadyCollected)
    ));
}

#[tokio::test]
async fn successful_io_returns_original_archive_for_the_callers_global_result() {
    use std::{
        io::{Seek, SeekFrom, Write},
        os::unix::fs::MetadataExt,
        process::Stdio,
    };
    let mut input = tempfile::tempfile().unwrap();
    input.write_all(b"archive-fixture").unwrap();
    input.seek(SeekFrom::Start(0)).unwrap();
    let before = input.metadata().unwrap();
    let child = fixture_command("read-archive")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut owner = OwnedBatchIo::own(child, Some(input), 4096);
    let collected = owner.collect().await.unwrap();
    let retained = collected
        .retained_input
        .as_ref()
        .map(File::metadata)
        .transpose()
        .unwrap();
    assert!(collected.errors.is_empty(), "{collected:?}");
    assert!(collected.status.is_some_and(|status| status.success()));
    assert!(collected.stdout_eof && collected.stderr_eof);
    let retained = retained.unwrap();
    assert_eq!(
        (retained.dev(), retained.ino()),
        (before.dev(), before.ino())
    );
}

#[tokio::test]
async fn ordinary_cli_uses_its_own_two_capture_limits_in_the_same_owner() {
    use std::process::Stdio;
    let child = fixture_command("explicit-unit-only")
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let mut owner = OwnedBatchIo::own_cli(child, 5, 7);
    let collected = owner.collect().await.unwrap();
    assert!(collected.errors.is_empty(), "{collected:?}");
    assert!(collected.status.is_some_and(|status| status.success()));
    assert!(collected.stdout_eof && collected.stderr_eof);
    assert_eq!(collected.stdout.bytes.len(), 5);
    assert_eq!(collected.stderr.bytes.len(), 7);
    assert!(collected.stdout.truncated && collected.stdout.total_bytes > 5);
    assert!(collected.stderr.truncated && collected.stderr.total_bytes > 7);
    assert!(collected.retained_input.is_none());
}

#[test]
#[ignore = "fixture subprocess; selected explicitly by the owning native IO test"]
fn fixture_child17() {
    let mode = std::env::var("AI_GUARD_BATCH_IO_CHILD_FIXTURE17").ok();
    if !matches!(mode.as_deref(), Some("explicit-unit-only" | "read-archive")) {
        return;
    }
    use std::io::{Read, Write};
    if mode.as_deref() == Some("read-archive") {
        let mut input = Vec::new();
        std::io::stdin().take(1024).read_to_end(&mut input).unwrap();
        assert_eq!(input, b"archive-fixture");
    }
    std::io::stdout()
        .write_all(b"native-private-stdout\n")
        .unwrap();
    std::io::stderr()
        .write_all(b"native-private-stderr\n")
        .unwrap();
}
