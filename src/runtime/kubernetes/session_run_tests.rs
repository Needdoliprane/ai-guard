//! The broker run over the real in-process protocol v3 supervisor, with real
//! child processes (`/bin/cat` as the provider echo and as the egress
//! helper, `/bin/sleep`) on in-memory broker streams. Hello and Ready are
//! exchanged on the raw streams first, as the attach bracket does.
//!
//! What this does not qualify: no kubelet, API server, attach relay,
//! container, worker image, PID 1, real provider, G05 egress owner or model
//! call. The credential payload is a synthetic canary.
use super::*;
use crate::runtime::kubernetes::{
    session_protocol::{
        SessionModeV3,
        tests::{
            PROMPT_CANARY, ProfileFixture, SECRET_CANARY, default_archive,
            fixture as profile_fixture,
        },
    },
    session_supervisor::{
        SupervisorConfigV3, SupervisorEndV3, serve,
        tests::{Dirs, config, fresh_dirs, hello_envelope},
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task::JoinHandle,
};

const WATCHDOG: Duration = Duration::from_secs(30);

#[derive(Default)]
struct Collected {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}
impl SessionOutputV2 for Collected {
    fn stdout(&mut self, bytes: &[u8]) {
        self.stdout.extend_from_slice(bytes);
    }
    fn stderr(&mut self, bytes: &[u8]) {
        self.stderr.extend_from_slice(bytes);
    }
}

fn waits() -> SessionRunWaitsV2 {
    SessionRunWaitsV2 {
        import: Duration::from_secs(10),
        start: Duration::from_secs(10),
        end: Duration::from_secs(10),
    }
}

/// Hello and Ready on the raw streams, then the broker channel on them.
async fn channel(
    fixture: &ProfileFixture,
    config: SupervisorConfigV3,
) -> (SessionChannelV3, JoinHandle<SupervisorEndV3>) {
    let (mut to_worker, worker_input) = tokio::io::duplex(512 * 1024);
    let (worker_output, mut from_worker) = tokio::io::duplex(512 * 1024);
    let task = tokio::spawn(async move {
        serve(SessionModeV3::Session, worker_input, worker_output, &config).await
    });
    to_worker
        .write_all(&hello_envelope(&fixture.binding))
        .await
        .unwrap();
    let mut prefix = [0_u8; 4];
    from_worker.read_exact(&mut prefix).await.unwrap();
    let mut ready = vec![0_u8; u32::from_be_bytes(prefix) as usize];
    from_worker.read_exact(&mut ready).await.unwrap();
    let ready: serde_json::Value = serde_json::from_slice(&ready).unwrap();
    assert_eq!(ready["kind"], "ready");
    let channel = SessionChannelV3::broker(
        tokio::io::join(from_worker, to_worker),
        SessionModeV3::Session,
    )
    .unwrap();
    (channel, task)
}

#[allow(
    clippy::too_many_arguments,
    reason = "one fixture run: provider, archive, stdin, stop and sink"
)]
async fn run_into<O: SessionOutputV2>(
    fixture: &ProfileFixture,
    dirs: &Dirs,
    program: &str,
    args: &[&str],
    archive: &[u8],
    stdin: &[u8],
    stop: &mut watch::Receiver<Option<StopReasonV3>>,
    mut output: O,
) -> (SessionRunReportV2, O, SupervisorEndV3) {
    let (channel, task) = channel(fixture, config(dirs, program, args, &[])).await;
    let report = tokio::time::timeout(
        WATCHDOG,
        run_session(
            channel,
            fixture.handoff(),
            archive,
            stdin,
            &mut output,
            stop,
            waits(),
        ),
    )
    .await
    .expect("the run ended within the test watchdog");
    let end = tokio::time::timeout(WATCHDOG, task)
        .await
        .expect("the supervisor ended within the test watchdog")
        .unwrap();
    (report, output, end)
}

async fn run(
    fixture: &ProfileFixture,
    dirs: &Dirs,
    program: &str,
    args: &[&str],
    archive: &[u8],
    stdin: &[u8],
    stop: &mut watch::Receiver<Option<StopReasonV3>>,
) -> (SessionRunReportV2, Collected, SupervisorEndV3) {
    run_into(
        fixture,
        dirs,
        program,
        args,
        archive,
        stdin,
        stop,
        Collected::default(),
    )
    .await
}

fn assert_no_canary(bytes: &[u8]) {
    for canary in [SECRET_CANARY, PROMPT_CANARY] {
        assert!(
            !bytes
                .windows(canary.len())
                .any(|window| window == canary.as_bytes()),
            "a private canary reached the provider output"
        );
    }
}

fn committed(fixture: &ProfileFixture) -> ImportedV3 {
    let (manifest_sha256, files, bytes) = fixture.profile.retained_workspace_commitment();
    ImportedV3 {
        manifest_sha256: manifest_sha256.to_owned(),
        files,
        bytes,
    }
}

#[tokio::test]
async fn a_session_imports_runs_its_stdin_and_reports_the_declared_exit() {
    let fixture = profile_fixture(default_archive());
    let dirs = fresh_dirs();
    let (_stop, mut stop) = watch::channel(None);
    let (report, output, end) = run(
        &fixture,
        &dirs,
        "/bin/cat",
        &[],
        &fixture.archive,
        b"provider input\n",
        &mut stop,
    )
    .await;
    assert_eq!(report.imported, Some(committed(&fixture)));
    assert!(report.started);
    assert_eq!(report.stop_sent, None);
    assert_eq!(
        report.end,
        SessionRunEndV2::Exited(ExitedV3 {
            code: Some(0),
            signal: None
        })
    );
    assert!(report.stream_ended);
    assert_eq!(output.stdout, b"provider input\n");
    assert_eq!(report.stdout_bytes, 15);
    assert_eq!(report.stderr_bytes, output.stderr.len() as u64);
    assert_no_canary(&output.stdout);
    assert_no_canary(&output.stderr);
    assert_eq!(end, SupervisorEndV3::Complete);
    // The import wrote the committed workspace.
    assert_eq!(
        std::fs::read(dirs.workspace.path().join("README.md")).unwrap(),
        b"hello\n"
    );
}

/// Requests a Stop on the first provider output (so only once the provider
/// runs), then another one on every later output: those are never sent.
struct StopOnOutput {
    requester: watch::Sender<Option<StopReasonV3>>,
    stdout_bytes: u64,
}
impl SessionOutputV2 for StopOnOutput {
    fn stdout(&mut self, bytes: &[u8]) {
        let reason = if self.stdout_bytes == 0 {
            StopReasonV3::Cancelled
        } else {
            StopReasonV3::Deadline
        };
        self.requester.send_replace(Some(reason));
        self.stdout_bytes += bytes.len() as u64;
    }
    fn stderr(&mut self, _: &[u8]) {}
}

#[tokio::test]
async fn a_stop_during_the_run_is_sent_once_and_the_declared_signal_reported() {
    let fixture = profile_fixture(default_archive());
    let dirs = fresh_dirs();
    let (requester, mut stop) = watch::channel(None);
    let sink = StopOnOutput {
        requester,
        stdout_bytes: 0,
    };
    // `yes` writes until it is killed.
    let (report, sink, end) = run_into(
        &fixture,
        &dirs,
        "/usr/bin/yes",
        &[],
        &fixture.archive,
        b"",
        &mut stop,
        sink,
    )
    .await;
    assert!(report.started);
    assert!(sink.stdout_bytes > 0);
    assert_eq!(report.stdout_bytes, sink.stdout_bytes);
    // Cancelled was read first; the later Deadline was never sent.
    assert_eq!(report.stop_sent, Some(StopReasonV3::Cancelled));
    assert_eq!(
        report.end,
        SessionRunEndV2::Exited(ExitedV3 {
            code: None,
            signal: Some(libc::SIGKILL)
        })
    );
    assert_eq!(end, SupervisorEndV3::Complete);
}

#[tokio::test]
async fn a_stop_before_start_is_answered_by_the_stopped_refusal() {
    let fixture = profile_fixture(default_archive());
    let dirs = fresh_dirs();
    let (_requester, mut stop) = watch::channel(Some(StopReasonV3::Revoked));
    let (report, output, _) = run(
        &fixture,
        &dirs,
        "/bin/cat",
        &[],
        &fixture.archive,
        b"never sent",
        &mut stop,
    )
    .await;
    assert_eq!(report.imported, Some(committed(&fixture)));
    assert!(!report.started);
    assert_eq!(report.stop_sent, Some(StopReasonV3::Revoked));
    assert_eq!(report.end, SessionRunEndV2::Refused(RefusalV3::Stopped));
    assert!(output.stdout.is_empty());
}

#[tokio::test]
async fn archive_bytes_other_than_the_handoff_are_refused_by_the_broker_before_sending() {
    let fixture = profile_fixture(default_archive());
    let dirs = fresh_dirs();
    let (_requester, mut stop) = watch::channel(None);
    // Same length, one byte changed: the Handoff digest no longer matches,
    // and the broker refuses its own last archive frame.
    let mut archive = fixture.archive.clone();
    archive[0] ^= 0x01;
    let (report, output, _) = run(&fixture, &dirs, "/bin/cat", &[], &archive, b"", &mut stop).await;
    assert_eq!(
        report.end,
        SessionRunEndV2::Lost {
            phase: SessionRunPhaseV2::Handoff,
            protocol: true
        }
    );
    assert_eq!(report.imported, None);
    assert!(!report.started);
    assert!(output.stdout.is_empty());
    assert!(
        std::fs::read_dir(dirs.workspace.path())
            .unwrap()
            .next()
            .is_none(),
        "nothing imported"
    );
}
