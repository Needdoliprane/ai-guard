//! The protocol v3 supervisor over in-memory broker streams, with real child
//! processes on real OS pipes: `/bin/cat` (provider echo and egress helper),
//! `/usr/bin/false`, `/bin/sleep`, and this test binary as an ignored
//! process fixture that leaves a descendant behind. The supervisor is not
//! PID 1 here, so the reap scope is the process groups, never
//! `kill(-1, SIGKILL)`.
//!
//! What this does not qualify: no kubelet, API server, attach transport,
//! container, worker image, PID-1 signal handling, real provider, real G05
//! helper (`/bin/cat` stands in for its stdio) or model call. The credential
//! payload is a synthetic canary.
use super::*;
use crate::{
    provider::ClientTransport,
    runtime::kubernetes::{
        session::SessionBootstrapBindingV2,
        session_protocol::{
            BrokerControlV3, FRAME_HEADER_BYTES, MAX_CONTROL_BYTES, SessionChannelV3, StopReasonV3,
            WorkerEventV3,
            tests::{
                PROMPT_CANARY, ProfileFixture, SECRET_CANARY, default_archive,
                fixture as profile_fixture, fixture_with, payload_json, sha256_hex, tar_of,
            },
        },
    },
};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use uuid::Uuid;

const WATCHDOG: Duration = Duration::from_secs(20);
const FIXTURE_ENV: &str = "AI_GUARD_K3_FIXTURE";

/// Process fixture: leaves one `/bin/sleep` in its process group, prints its
/// pid and exits 0 without waiting for it.
#[test]
#[ignore = "internal process fixture, run only as a child of the supervisor tests"]
fn orphan_fixture() {
    assert_eq!(std::env::var(FIXTURE_ENV).as_deref(), Ok("orphan"));
    let child = std::process::Command::new("/bin/sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    println!("GRANDCHILD={}", child.id());
    std::mem::forget(child);
}

pub(crate) fn hello_envelope(binding: &SessionBootstrapBindingV2) -> Vec<u8> {
    let body = serde_json::to_vec(&json!({
        "kind": "hello",
        "version": 2,
        "exchange_id": Uuid::new_v4().to_string(),
        "binding": {
            "version": binding.version,
            "installation_id": binding.installation_id.to_string(),
            "control_namespace_uuid": binding.control_namespace_uuid.to_string(),
            "broker_id": binding.broker_id.to_string(),
            "incarnation_id": binding.incarnation_id.to_string(),
            "session_id": binding.session_id.to_string(),
            "attempt_id": binding.attempt_id.to_string(),
            "reservation_id": binding.reservation_id,
            "reservation_input_hash": binding.reservation_input_hash,
            "authority_fingerprint": binding.authority_fingerprint,
            "profile_hash": binding.profile_hash,
            "preparation_sha256": binding.preparation_sha256,
        }
    }))
    .unwrap();
    let mut envelope = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
    envelope.extend_from_slice(&body);
    envelope
}

pub(crate) struct Dirs {
    pub(crate) workspace: tempfile::TempDir,
    pub(crate) home: tempfile::TempDir,
}
pub(crate) fn fresh_dirs() -> Dirs {
    let dirs = Dirs {
        workspace: tempfile::tempdir().unwrap(),
        home: tempfile::tempdir().unwrap(),
    };
    std::fs::set_permissions(dirs.home.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dirs
}

pub(crate) fn config(
    dirs: &Dirs,
    program: &str,
    args: &[&str],
    environment: &[(&str, &str)],
) -> SupervisorConfigV3 {
    SupervisorConfigV3 {
        workspace: dirs.workspace.path().to_owned(),
        home: dirs.home.path().to_owned(),
        helper_program: PathBuf::from("/bin/cat"),
        helper_args: Vec::new(),
        egress_port: EGRESS_PORT,
        handoff_time: Duration::from_secs(5),
        import_time: Duration::from_secs(10),
        start_time: Duration::from_secs(5),
        launcher: LauncherV3::Fixed {
            program: PathBuf::from(program),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            environment: environment
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
        },
        reap: ReapScopeV3::ProcessGroups,
    }
}

/// The broker side of one supervisor run over two in-memory pipes.
struct Harness {
    to_worker: DuplexStream,
    from_worker: DuplexStream,
    reader: FrameReaderV3,
    task: JoinHandle<SupervisorEndV3>,
    /// Every byte the worker wrote after Ready.
    seen: Vec<u8>,
}
impl Harness {
    fn start(mode: SessionModeV3, config: SupervisorConfigV3) -> Self {
        let (to_worker, worker_input) = tokio::io::duplex(512 * 1024);
        let (worker_output, from_worker) = tokio::io::duplex(512 * 1024);
        let task =
            tokio::spawn(async move { serve(mode, worker_input, worker_output, &config).await });
        Self {
            to_worker,
            from_worker,
            reader: FrameReaderV3::new(),
            task,
            seen: Vec::new(),
        }
    }
    async fn hello(&mut self, binding: &SessionBootstrapBindingV2) {
        self.to_worker
            .write_all(&hello_envelope(binding))
            .await
            .unwrap();
        let mut prefix = [0_u8; 4];
        watched(self.from_worker.read_exact(&mut prefix))
            .await
            .unwrap();
        let mut body = vec![0_u8; u32::from_be_bytes(prefix) as usize];
        watched(self.from_worker.read_exact(&mut body))
            .await
            .unwrap();
        let ready: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(ready["kind"], "ready");
        assert_eq!(
            ready["binding"]["profile_hash"],
            binding.profile_hash.as_str()
        );
    }
    async fn send(&mut self, frame: &[u8]) {
        self.to_worker.write_all(frame).await.unwrap();
    }
    async fn control(&mut self, message: BrokerControlV3) {
        let frame = message.encode().unwrap();
        self.send(&frame).await;
    }
    async fn handoff(&mut self, fixture: &ProfileFixture, archive: &[u8]) {
        self.handoff_payload(fixture, &payload_json(SECRET_CANARY), archive)
            .await;
    }
    async fn handoff_payload(&mut self, fixture: &ProfileFixture, payload: &str, archive: &[u8]) {
        let handoff = fixture.handoff_with(payload, archive);
        self.control(BrokerControlV3::Handoff(Box::new(handoff)))
            .await;
        for chunk in archive.chunks(MAX_DATA_BYTES) {
            self.send(&encode_frame(ChannelV3::Archive, chunk).unwrap())
                .await;
        }
    }
    async fn frame(&mut self) -> Option<FrameV3> {
        let frame = watched(self.reader.read(&mut self.from_worker, MAX_CONTROL_BYTES))
            .await
            .unwrap();
        if let Some(frame) = &frame {
            self.seen.extend_from_slice(&frame.payload);
        }
        frame
    }
    /// Data frames until the next control message.
    async fn until_control(&mut self) -> (WorkerControlV3, Vec<FrameV3>) {
        let mut data = Vec::new();
        loop {
            let frame = self
                .frame()
                .await
                .expect("a control message before the end");
            if frame.channel == ChannelV3::Control {
                return (WorkerControlV3::decode(&frame.payload).unwrap(), data);
            }
            data.push(frame);
        }
    }
    /// The end of the run: no more frame, then the supervisor's end.
    async fn end(mut self) -> (SupervisorEndV3, Vec<u8>) {
        assert!(
            self.frame().await.is_none(),
            "no frame after the end message"
        );
        let end = watched(self.task).await.unwrap();
        (end, self.seen)
    }
}

async fn watched<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(WATCHDOG, future)
        .await
        .expect("the step did not end within the test watchdog")
}

fn assert_no_canary(bytes: &[u8]) {
    for canary in [SECRET_CANARY, PROMPT_CANARY] {
        assert!(
            !bytes
                .windows(canary.len())
                .any(|window| window == canary.as_bytes()),
            "a private canary reached the worker output"
        );
    }
}

fn workspace_is_empty(dirs: &Dirs) -> bool {
    std::fs::read_dir(dirs.workspace.path())
        .unwrap()
        .next()
        .is_none()
}

fn committed(fixture: &ProfileFixture) -> WorkerControlV3 {
    let (manifest_sha256, files, bytes) = fixture.profile.retained_workspace_commitment();
    WorkerControlV3::Imported(ImportedV3 {
        manifest_sha256: manifest_sha256.to_owned(),
        files,
        bytes,
    })
}

/// Hello, Handoff, import, Start and Started on a fresh harness.
async fn started(fixture: &ProfileFixture, config: SupervisorConfigV3) -> Harness {
    let mut harness = Harness::start(SessionModeV3::Session, config);
    harness.hello(&fixture.binding).await;
    harness.handoff(fixture, &fixture.archive).await;
    let (imported, data) = harness.until_control().await;
    assert!(data.is_empty());
    assert_eq!(imported, committed(fixture));
    harness.control(BrokerControlV3::Start).await;
    let (started, data) = harness.until_control().await;
    assert!(data.is_empty());
    assert_eq!(started, WorkerControlV3::Started);
    harness
}

#[tokio::test]
async fn session_imports_runs_frames_stdio_and_egress_then_declares_the_exit() {
    let fixture = profile_fixture(default_archive());
    let dirs = fresh_dirs();
    let mut harness = started(&fixture, config(&dirs, "/bin/cat", &[], &[])).await;
    // The import wrote exactly the committed files and modes.
    let workspace = dirs.workspace.path();
    assert_eq!(
        std::fs::read(workspace.join("README.md")).unwrap(),
        b"hello\n"
    );
    assert_eq!(std::fs::read(workspace.join("bin/tool")).unwrap(), b"#!x\n");
    let mode = |path: &str| {
        std::fs::metadata(workspace.join(path))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777
    };
    assert_eq!((mode("README.md"), mode("bin/tool")), (0o644, 0o755));
    // Provider stdio through /bin/cat, egress through the /bin/cat helper.
    let opaque: Vec<u8> = (0..=255_u8).rev().collect();
    harness
        .send(&encode_frame(ChannelV3::Stdin, b"provider input\n").unwrap())
        .await;
    harness
        .send(&encode_frame(ChannelV3::Egress, &opaque).unwrap())
        .await;
    let (mut stdout, mut egress) = (Vec::new(), Vec::new());
    while stdout.len() < 15 || egress.len() < opaque.len() {
        let frame = harness.frame().await.unwrap();
        match frame.channel {
            ChannelV3::Stdout => stdout.extend_from_slice(&frame.payload),
            ChannelV3::Egress => egress.extend_from_slice(&frame.payload),
            other => panic!("unexpected frame on {other:?}"),
        }
    }
    assert_eq!(stdout, b"provider input\n");
    assert_eq!(egress, opaque);
    harness.control(BrokerControlV3::StdinClose).await;
    let (exited, data) = harness.until_control().await;
    assert!(data.is_empty());
    assert_eq!(
        exited,
        WorkerControlV3::Exited(ExitedV3 {
            code: Some(0),
            signal: None
        })
    );
    let (end, seen) = harness.end().await;
    assert_eq!(end, SupervisorEndV3::Complete);
    assert_eq!(exit_code(end), EXIT_COMPLETE);
    assert_no_canary(&seen);
}

#[tokio::test]
async fn exit_code_and_stop_signal_are_declared_as_reported() {
    let fixture = profile_fixture(default_archive());
    let dirs = fresh_dirs();
    let mut harness = started(&fixture, config(&dirs, "/usr/bin/false", &[], &[])).await;
    let (exited, _) = harness.until_control().await;
    assert_eq!(
        exited,
        WorkerControlV3::Exited(ExitedV3 {
            code: Some(1),
            signal: None
        })
    );
    assert_eq!(harness.end().await.0, SupervisorEndV3::Complete);

    let dirs = fresh_dirs();
    let mut harness = started(&fixture, config(&dirs, "/bin/sleep", &["30"], &[])).await;
    harness
        .control(BrokerControlV3::Stop(StopReasonV3::Cancelled))
        .await;
    let (exited, _) = harness.until_control().await;
    assert_eq!(
        exited,
        WorkerControlV3::Exited(ExitedV3 {
            code: None,
            signal: Some(libc::SIGKILL)
        })
    );
    assert_eq!(harness.end().await.0, SupervisorEndV3::Complete);
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 checks existence only.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[tokio::test]
async fn processes_left_by_the_provider_are_killed_after_the_exit() {
    let fixture = profile_fixture(default_archive());
    let dirs = fresh_dirs();
    let program = std::env::current_exe().unwrap();
    let program = program.to_str().unwrap().to_owned();
    let mut harness = started(
        &fixture,
        config(
            &dirs,
            &program,
            &[
                "--exact",
                "runtime::kubernetes::session_supervisor::tests::orphan_fixture",
                "--ignored",
                "--nocapture",
            ],
            &[(FIXTURE_ENV, "orphan")],
        ),
    )
    .await;
    let (exited, data) = harness.until_control().await;
    assert_eq!(
        exited,
        WorkerControlV3::Exited(ExitedV3 {
            code: Some(0),
            signal: None
        })
    );
    let stdout: Vec<u8> = data
        .iter()
        .filter(|frame| frame.channel == ChannelV3::Stdout)
        .flat_map(|frame| frame.payload.clone())
        .collect();
    let text = String::from_utf8(stdout).unwrap();
    let pid: i32 = text
        .lines()
        .find_map(|line| line.strip_prefix("GRANDCHILD="))
        .expect("the fixture printed its descendant")
        .parse()
        .unwrap();
    assert_eq!(harness.end().await.0, SupervisorEndV3::Complete);
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while alive(pid) && std::time::Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!alive(pid), "the descendant outlived the session end");
}

/// One refused Handoff: the closed reason, exit 67, no workspace write and
/// no private canary on any frame.
async fn refused_handoff(
    fixture: &ProfileFixture,
    payload: &str,
    archive: &[u8],
    binding: &SessionBootstrapBindingV2,
    expected: RefusalV3,
) {
    let dirs = fresh_dirs();
    let mut harness = Harness::start(SessionModeV3::Session, config(&dirs, "/bin/cat", &[], &[]));
    harness.hello(binding).await;
    harness.handoff_payload(fixture, payload, archive).await;
    let (refused, data) = harness.until_control().await;
    assert!(data.is_empty());
    assert_eq!(refused, WorkerControlV3::Refused(expected));
    let (end, seen) = harness.end().await;
    assert_eq!(end, SupervisorEndV3::Refused(expected));
    assert_eq!(exit_code(end), EXIT_REFUSED);
    assert_no_canary(&seen);
    assert!(
        workspace_is_empty(&dirs),
        "{expected:?} wrote the workspace"
    );
}

#[tokio::test]
async fn handoff_is_refused_on_profile_invocation_credential_and_interactive_mismatch() {
    let fixture = profile_fixture(default_archive());
    let archive = fixture.archive.clone();
    let payload = payload_json(SECRET_CANARY);
    // The Hello names another profile hash.
    let mut other = fixture.binding.clone();
    other.profile_hash = "0".repeat(64);
    refused_handoff(
        &fixture,
        &payload,
        &archive,
        &other,
        RefusalV3::ProfileMismatch,
    )
    .await;
    // The Hello names another session.
    let mut other = fixture.binding.clone();
    other.session_id = Uuid::from_u128(99);
    refused_handoff(
        &fixture,
        &payload,
        &archive,
        &other,
        RefusalV3::ProfileMismatch,
    )
    .await;
    // Another prompt than the committed one.
    let mut changed = profile_fixture(default_archive());
    let last = changed.args.len() - 1;
    changed.args[last] = "another prompt".into();
    refused_handoff(
        &changed,
        &payload,
        &archive,
        &fixture.binding,
        RefusalV3::InvocationMismatch,
    )
    .await;
    // A credential for another client than the authorized invocation.
    let claude =
        json!({"version":1,"client":"claude_code","auth_mode":"api","secret":SECRET_CANARY})
            .to_string();
    refused_handoff(
        &fixture,
        &claude,
        &archive,
        &fixture.binding,
        RefusalV3::CredentialInvalid,
    )
    .await;
    let truncated = format!("{{\"version\":1,\"secret\":\"{SECRET_CANARY}\"");
    refused_handoff(
        &fixture,
        &truncated,
        &archive,
        &fixture.binding,
        RefusalV3::CredentialInvalid,
    )
    .await;
    // Interactive transport: refused explicitly, never faked.
    let interactive = fixture_with(default_archive(), ClientTransport::InteractiveTty);
    refused_handoff(
        &interactive,
        &payload,
        &interactive.archive,
        &interactive.binding,
        RefusalV3::InteractiveUnsupported,
    )
    .await;
}

fn raw_entry(path: &[u8], kind: tar::EntryType, link: Option<&str>, data: &[u8]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.as_old_mut().name[..path.len()].copy_from_slice(path);
    header.set_entry_type(kind);
    header.set_mode(0o644);
    header.set_size(data.len() as u64);
    if let Some(link) = link {
        header.set_link_name(link).unwrap();
    }
    header.set_cksum();
    builder.append(&header, data).unwrap();
    // Padding so the archive is not smaller than the committed 10 bytes.
    let mut padding = tar::Header::new_gnu();
    padding.set_path("padding").unwrap();
    padding.set_size(16);
    padding.set_mode(0o644);
    padding.set_cksum();
    builder.append(&padding, &[0_u8; 16][..]).unwrap();
    builder.into_inner().unwrap()
}

#[tokio::test]
async fn archive_with_a_link_a_traversal_or_another_manifest_is_never_written() {
    let fixture = profile_fixture(default_archive());
    let payload = payload_json(SECRET_CANARY);
    let binding = fixture.binding.clone();
    let link = raw_entry(b"link", tar::EntryType::Symlink, Some("/etc/passwd"), b"");
    refused_handoff(
        &fixture,
        &payload,
        &link,
        &binding,
        RefusalV3::WorkspaceInvalid,
    )
    .await;
    let hard = raw_entry(b"hard", tar::EntryType::Link, Some("README.md"), b"");
    refused_handoff(
        &fixture,
        &payload,
        &hard,
        &binding,
        RefusalV3::WorkspaceInvalid,
    )
    .await;
    let traversal = raw_entry(b"../escape", tar::EntryType::Regular, None, b"x");
    refused_handoff(
        &fixture,
        &payload,
        &traversal,
        &binding,
        RefusalV3::WorkspaceInvalid,
    )
    .await;
    let absolute = raw_entry(b"/tmp/escape", tar::EntryType::Regular, None, b"x");
    refused_handoff(
        &fixture,
        &payload,
        &absolute,
        &binding,
        RefusalV3::WorkspaceInvalid,
    )
    .await;
    // A valid archive whose manifest is not the committed one.
    let other = tar_of(&[
        ("README.md", b"HELLO\n", 0o644),
        ("bin/tool", b"#!x\n", 0o755),
    ]);
    refused_handoff(
        &fixture,
        &payload,
        &other,
        &binding,
        RefusalV3::WorkspaceMismatch,
    )
    .await;
    let mode = tar_of(&[
        ("README.md", b"hello\n", 0o755),
        ("bin/tool", b"#!x\n", 0o755),
    ]);
    refused_handoff(
        &fixture,
        &payload,
        &mode,
        &binding,
        RefusalV3::WorkspaceMismatch,
    )
    .await;
}

#[tokio::test]
async fn archive_bytes_that_differ_from_the_handoff_or_exceed_it_are_refused() {
    let fixture = profile_fixture(default_archive());
    let dirs = fresh_dirs();
    // Same length, other content: the digest differs.
    let mut harness = Harness::start(SessionModeV3::Session, config(&dirs, "/bin/cat", &[], &[]));
    harness.hello(&fixture.binding).await;
    let handoff = fixture.handoff();
    harness
        .control(BrokerControlV3::Handoff(Box::new(handoff)))
        .await;
    let mut tampered = fixture.archive.clone();
    tampered[700] ^= 0xff;
    harness
        .send(&encode_frame(ChannelV3::Archive, &tampered).unwrap())
        .await;
    let (refused, _) = harness.until_control().await;
    assert_eq!(
        refused,
        WorkerControlV3::Refused(RefusalV3::ArchiveMismatch)
    );
    assert_eq!(
        harness.end().await.0,
        SupervisorEndV3::Refused(RefusalV3::ArchiveMismatch)
    );
    assert!(workspace_is_empty(&dirs));
    // One byte more than declared: a protocol refusal, before any import.
    let dirs = fresh_dirs();
    let mut harness = Harness::start(SessionModeV3::Session, config(&dirs, "/bin/cat", &[], &[]));
    harness.hello(&fixture.binding).await;
    harness
        .control(BrokerControlV3::Handoff(Box::new(fixture.handoff())))
        .await;
    let mut longer = fixture.archive.clone();
    longer.push(0);
    harness
        .send(&encode_frame(ChannelV3::Archive, &longer).unwrap())
        .await;
    let (refused, _) = harness.until_control().await;
    assert_eq!(refused, WorkerControlV3::Refused(RefusalV3::Protocol));
    let (end, _) = harness.end().await;
    assert_eq!(
        (end, exit_code(end)),
        (SupervisorEndV3::Protocol, EXIT_PROTOCOL)
    );
    assert!(workspace_is_empty(&dirs));
    // A data frame above 64 KiB is refused from its header.
    let dirs = fresh_dirs();
    let mut harness = Harness::start(SessionModeV3::Session, config(&dirs, "/bin/cat", &[], &[]));
    harness.hello(&fixture.binding).await;
    harness
        .control(BrokerControlV3::Handoff(Box::new(fixture.handoff())))
        .await;
    let mut header = u32::try_from(MAX_DATA_BYTES + 2)
        .unwrap()
        .to_be_bytes()
        .to_vec();
    header.push(ChannelV3::Archive.byte());
    harness.send(&header).await;
    let (refused, _) = harness.until_control().await;
    assert_eq!(refused, WorkerControlV3::Refused(RefusalV3::Protocol));
    assert_eq!(harness.end().await.0, SupervisorEndV3::Protocol);
}

#[tokio::test]
async fn a_workspace_that_is_not_empty_is_refused_and_kept() {
    let fixture = profile_fixture(default_archive());
    let dirs = fresh_dirs();
    std::fs::write(dirs.workspace.path().join("README.md"), b"planted").unwrap();
    let mut harness = Harness::start(SessionModeV3::Session, config(&dirs, "/bin/cat", &[], &[]));
    harness.hello(&fixture.binding).await;
    harness.handoff(&fixture, &fixture.archive).await;
    let (refused, _) = harness.until_control().await;
    assert_eq!(
        refused,
        WorkerControlV3::Refused(RefusalV3::WorkspaceNotEmpty)
    );
    assert_eq!(
        harness.end().await.0,
        SupervisorEndV3::Refused(RefusalV3::WorkspaceNotEmpty)
    );
    assert_eq!(
        std::fs::read(dirs.workspace.path().join("README.md")).unwrap(),
        b"planted"
    );
}

#[tokio::test]
async fn frames_out_of_order_ends_and_windows_have_their_own_exit() {
    let fixture = profile_fixture(default_archive());
    // Start before the Handoff.
    let dirs = fresh_dirs();
    let mut harness = Harness::start(SessionModeV3::Session, config(&dirs, "/bin/cat", &[], &[]));
    harness.hello(&fixture.binding).await;
    harness.control(BrokerControlV3::Start).await;
    let (refused, _) = harness.until_control().await;
    assert_eq!(refused, WorkerControlV3::Refused(RefusalV3::Protocol));
    assert_eq!(harness.end().await.0, SupervisorEndV3::Protocol);
    // The input ends after Ready: no handoff, nothing written.
    let mut harness = Harness::start(SessionModeV3::Session, config(&dirs, "/bin/cat", &[], &[]));
    harness.hello(&fixture.binding).await;
    harness.to_worker.shutdown().await.unwrap();
    let (end, seen) = harness.end().await;
    assert_eq!(
        (end, exit_code(end)),
        (SupervisorEndV3::NoHandoff, EXIT_NO_HANDOFF)
    );
    assert!(seen.is_empty());
    // The input ends before the Hello.
    let mut harness = Harness::start(SessionModeV3::Session, config(&dirs, "/bin/cat", &[], &[]));
    harness.to_worker.shutdown().await.unwrap();
    assert_eq!(
        watched(harness.task).await.unwrap(),
        SupervisorEndV3::NoHandoff
    );
    // Stop after Imported, before Start.
    let mut harness = Harness::start(SessionModeV3::Session, config(&dirs, "/bin/cat", &[], &[]));
    harness.hello(&fixture.binding).await;
    harness.handoff(&fixture, &fixture.archive).await;
    assert_eq!(harness.until_control().await.0, committed(&fixture));
    harness
        .control(BrokerControlV3::Stop(StopReasonV3::Revoked))
        .await;
    assert_eq!(
        harness.until_control().await.0,
        WorkerControlV3::Refused(RefusalV3::Stopped)
    );
    let (end, _) = harness.end().await;
    assert_eq!(
        (end, exit_code(end)),
        (SupervisorEndV3::Stopped, EXIT_STOPPED)
    );
    // No Handoff within its window.
    let dirs = fresh_dirs();
    let mut short = config(&dirs, "/bin/cat", &[], &[]);
    short.handoff_time = Duration::from_millis(200);
    let mut harness = Harness::start(SessionModeV3::Session, short);
    harness.hello(&fixture.binding).await;
    assert_eq!(
        harness.until_control().await.0,
        WorkerControlV3::Refused(RefusalV3::Deadline)
    );
    let (end, _) = harness.end().await;
    assert_eq!(
        (end, exit_code(end)),
        (SupervisorEndV3::Deadline, EXIT_DEADLINE)
    );
    // The broker disappears while the provider runs: it is killed, the exit
    // is still declared, and the end says the broker was lost.
    let dirs = fresh_dirs();
    let harness = started(&fixture, config(&dirs, "/bin/sleep", &["30"], &[])).await;
    let Harness {
        to_worker,
        mut from_worker,
        task,
        ..
    } = harness;
    drop(to_worker);
    let mut rest = Vec::new();
    watched(from_worker.read_to_end(&mut rest)).await.unwrap();
    let message = WorkerControlV3::decode(&rest[FRAME_HEADER_BYTES..]).unwrap();
    assert_eq!(
        message,
        WorkerControlV3::Exited(ExitedV3 {
            code: None,
            signal: Some(libc::SIGKILL)
        })
    );
    let end = watched(task).await.unwrap();
    assert_eq!(
        (end, exit_code(end)),
        (SupervisorEndV3::BrokerLost, EXIT_BROKER_LOST)
    );
}

#[tokio::test]
async fn export_streams_a_tar_of_regular_files_and_directories_with_its_digest() {
    let dirs = fresh_dirs();
    let root = dirs.workspace.path();
    std::fs::create_dir_all(root.join("src/deep")).unwrap();
    std::fs::write(root.join("README.md"), b"exported\n").unwrap();
    std::fs::write(root.join("src/deep/run.sh"), b"#!/bin/true\n").unwrap();
    std::fs::set_permissions(
        root.join("src/deep/run.sh"),
        std::fs::Permissions::from_mode(0o750),
    )
    .unwrap();
    std::fs::create_dir(root.join("empty")).unwrap();
    let fixture = profile_fixture(default_archive());
    let mut harness = Harness::start(SessionModeV3::Export, config(&dirs, "/bin/cat", &[], &[]));
    harness.hello(&fixture.binding).await;
    harness.control(BrokerControlV3::Start).await;
    let (end_marker, data) = harness.until_control().await;
    let archive: Vec<u8> = data
        .iter()
        .inspect(|frame| assert_eq!(frame.channel, ChannelV3::Archive))
        .flat_map(|frame| frame.payload.clone())
        .collect();
    assert_eq!(
        end_marker,
        WorkerControlV3::ArchiveEnd(ArchiveEndV3 {
            archive_bytes: archive.len() as u64,
            archive_sha256: sha256_hex(&archive),
        })
    );
    let (end, _) = harness.end().await;
    assert_eq!(
        (end, exit_code(end)),
        (SupervisorEndV3::Complete, EXIT_COMPLETE)
    );
    // The broker side reads it with the existing workspace rules.
    let manifest = scan_workspace_archive(archive.as_slice(), Limits::default()).unwrap();
    let entries: Vec<(&str, u64, Option<u32>)> = manifest
        .iter()
        .map(|(path, fingerprint)| (path.as_str(), fingerprint.bytes, fingerprint.mode))
        .collect();
    assert_eq!(
        entries,
        [
            ("README.md", 9, Some(0o644)),
            ("src/deep/run.sh", 12, Some(0o755))
        ]
    );
    // A symbolic link refuses the export: nothing is followed.
    std::os::unix::fs::symlink("/etc/passwd", root.join("link")).unwrap();
    let mut harness = Harness::start(SessionModeV3::Export, config(&dirs, "/bin/cat", &[], &[]));
    harness.hello(&fixture.binding).await;
    harness.control(BrokerControlV3::Start).await;
    let (refused, _) = harness.until_control().await;
    assert_eq!(
        refused,
        WorkerControlV3::Refused(RefusalV3::ExportUnsupportedEntry)
    );
    let (end, _) = harness.end().await;
    assert_eq!(
        (end, exit_code(end)),
        (
            SupervisorEndV3::Refused(RefusalV3::ExportUnsupportedEntry),
            EXIT_REFUSED
        )
    );
    // Export never accepts a Handoff, and nothing is streamed before Start.
    let mut harness = Harness::start(SessionModeV3::Export, config(&dirs, "/bin/cat", &[], &[]));
    harness.hello(&fixture.binding).await;
    harness
        .control(BrokerControlV3::Handoff(Box::new(fixture.handoff())))
        .await;
    assert_eq!(
        harness.until_control().await.0,
        WorkerControlV3::Refused(RefusalV3::Protocol)
    );
    assert_eq!(harness.end().await.0, SupervisorEndV3::Protocol);
}

/// The next event of a broker channel, which must exist.
async fn next_v3_event(
    events: &mut crate::runtime::kubernetes::session_protocol::SessionEventsV3,
) -> crate::runtime::kubernetes::session_protocol::WorkerEventV3 {
    watched(events.next())
        .await
        .unwrap()
        .expect("an event before the end")
}

/// The real broker channel against the real supervisor, with `/bin/cat`
/// as provider and helper. Only the Hello/Ready is hand-written here; the
/// attach transport is covered by the loopback attach tests.
#[tokio::test]
async fn broker_channel_and_supervisor_complete_a_session_together() {
    let fixture = profile_fixture(default_archive());
    let dirs = fresh_dirs();
    let mut harness = Harness::start(SessionModeV3::Session, config(&dirs, "/bin/cat", &[], &[]));
    harness.hello(&fixture.binding).await;
    let Harness {
        to_worker,
        from_worker,
        task,
        ..
    } = harness;
    let mut channel = SessionChannelV3::broker(
        tokio::io::join(from_worker, to_worker),
        SessionModeV3::Session,
    )
    .unwrap();
    let mut egress = channel.take_egress().unwrap();
    let (mut sender, mut events, _) = channel.split();
    sender
        .send_control(BrokerControlV3::Handoff(Box::new(fixture.handoff())))
        .await
        .unwrap();
    sender.send_archive(&fixture.archive).await.unwrap();
    assert!(matches!(
        next_v3_event(&mut events).await,
        WorkerEventV3::Control(WorkerControlV3::Imported(_))
    ));
    sender.send_control(BrokerControlV3::Start).await.unwrap();
    assert!(matches!(
        next_v3_event(&mut events).await,
        WorkerEventV3::Control(WorkerControlV3::Started)
    ));
    sender.send_stdin(b"round trip\n").await.unwrap();
    egress.write_all(b"egress-bytes").await.unwrap();
    let mut echoed = [0_u8; 12];
    watched(egress.read_exact(&mut echoed)).await.unwrap();
    assert_eq!(&echoed, b"egress-bytes");
    let mut stdout = Vec::new();
    while stdout.len() < 11 {
        match next_v3_event(&mut events).await {
            WorkerEventV3::Stdout(bytes) => stdout.extend_from_slice(&bytes),
            other => panic!("unexpected event {other:?}"),
        }
    }
    assert_eq!(stdout, b"round trip\n");
    sender
        .send_control(BrokerControlV3::StdinClose)
        .await
        .unwrap();
    assert!(matches!(
        next_v3_event(&mut events).await,
        WorkerEventV3::Control(WorkerControlV3::Exited(ExitedV3 {
            code: Some(0),
            signal: None
        }))
    ));
    assert!(watched(events.next()).await.unwrap().is_none());
    sender.close().await;
    assert_eq!(watched(task).await.unwrap(), SupervisorEndV3::Complete);
}

#[test]
fn v3_modes_dispatch_on_the_exact_raw_first_argument() {
    use std::os::unix::ffi::OsStringExt;
    assert_eq!(dispatch_v3_mode::<&str>(&[]), None);
    assert_eq!(dispatch_v3_mode(&["kubernetes-session-v2"]), None);
    assert_eq!(dispatch_v3_mode(&["exec", SESSION_RUN_MODE_ARG]), None);
    for mode in [SESSION_RUN_MODE_ARG, WORKSPACE_EXPORT_MODE_ARG] {
        assert_eq!(dispatch_v3_mode(&[mode, "x"]), Some(EXIT_PROTOCOL));
        let invalid = OsString::from_vec(vec![0xff]);
        assert_eq!(
            dispatch_v3_mode(&[OsString::from(mode), invalid]),
            Some(EXIT_PROTOCOL)
        );
    }
    let exits: Vec<i32> = [
        SupervisorEndV3::Complete,
        SupervisorEndV3::Protocol,
        SupervisorEndV3::Deadline,
        SupervisorEndV3::NoHandoff,
        SupervisorEndV3::Refused(RefusalV3::Protocol),
        SupervisorEndV3::Stopped,
        SupervisorEndV3::BrokerLost,
        SupervisorEndV3::Io,
    ]
    .into_iter()
    .map(exit_code)
    .collect();
    assert_eq!(exits, [0, 64, 65, 66, 67, 68, 69, 70]);
}
