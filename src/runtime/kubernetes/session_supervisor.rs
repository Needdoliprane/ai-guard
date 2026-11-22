//! Worker side of session protocol v3 (G11 K3, requirement R4): the two modes
//! `kubernetes-session-v3` and `kubernetes-workspace-export-v3` of the
//! existing `ai-guard-provider` binary, run as the Pod's PID 1.
//!
//! Session mode. After the unchanged v2 Hello/Ready, the supervisor accepts
//! one Handoff and validates it before any effect: the profile bytes must be
//! canonical and hash to the Hello `profile_hash`, their binding must equal
//! the Hello identifiers, the business invocation must match the profile's
//! `invocation_sha256`, an interactive transport is refused
//! (`interactive_unsupported`: no PTY exists in this slice), and the
//! credential payload goes through the existing `prepare_execution` checks.
//! The archive is received into a buffer reserved for exactly the declared
//! size, its SHA-256 compared, then scanned with the existing workspace rules
//! (`scan_workspace_archive`: no link, special file, traversal, duplicate or
//! conflict, workspace `Limits`), its manifest compared with the profile
//! commitment, and only then written into the empty workspace with
//! descriptor-anchored, exclusive, no-follow creations. `imported` declares
//! the verified commitment.
//!
//! On `start` the supervisor spawns the unchanged G05 egress helper
//! (`ai-guard-relay serve-egress-v1 --port 3128`) and the provider as
//! **children** (never `exec`: the supervisor stays PID 1), with piped
//! standard streams and the prepared environment plus the loopback proxy
//! variables. Provider stdout and stderr become STDOUT and STDERR frames,
//! STDIN frames feed its stdin, and the helper's stdio is carried byte-exact
//! on EGRESS frames. When the provider exits, its standard streams are
//! drained for at most `DRAIN_TIME`, `exited` declares its status, the
//! helper's stdin is closed, and every remaining process is killed: with
//! `kill(-1, SIGKILL)` and a reap loop when the supervisor is PID 1, and by
//! process group otherwise (tests, or a runtime that injects its own init).
//!
//! Export mode. After Hello/Ready and the broker's `start`, the supervisor
//! streams a tar of the read-only workspace on ARCHIVE frames, then
//! `archive_end` with the byte count and SHA-256 of the archive it wrote, and
//! exits 0. Only regular files and directories are archived; any other entry
//! refuses the export (`export_unsupported_entry`). No program is executed.
//!
//! Nothing here is an observation: every frame the supervisor writes is
//! declared. Standard output carries only Ready and protocol frames; the node
//! runtime keeps it in its logs, so no secret may travel on it. The Handoff
//! payload is never written to a frame or to stderr; the provider receives
//! the credential by design and its own output is not filtered here. Only one
//! fixed diagnostic sentence per end goes to stderr.
use super::{
    bootstrap::{
        AcceptedHelloV2, BootstrapCapacity, BootstrapError, MAX_HANDSHAKE_TIME, RESERVED_BYTES,
        WaitingForAttachHelloV2, bootstrap_waiting_for_attach_hello,
    },
    session_pod::{SESSION_RUN_MODE_ARG, WORKSPACE_EXPORT_MODE_ARG},
    session_profile::{KubernetesSessionInvocationV2, KubernetesSessionProfileV2},
    session_protocol::{
        ArchiveEndV3, BrokerControlV3, ChannelV3, ExitedV3, FrameReaderV3, FrameV3, HandoffV3,
        ImportedV3, MAX_DATA_BYTES, RefusalV3, SessionModeV3, SessionProtocolError,
        WorkerControlV3, WorkerInboundOrderV3, encode_frame,
    },
    session_worker::{EXIT_DEADLINE, EXIT_IO, EXIT_NO_HANDOFF, EXIT_PROTOCOL},
};
use crate::workspace::{Limits, Manifest, scan_workspace_archive, validate_relative_path};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::{CString, OsStr, OsString},
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt, process::ExitStatusExt},
    },
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    process::{Child, ChildStdin},
    sync::{Semaphore, mpsc},
    task::JoinHandle,
    time::{Instant, sleep_until, timeout, timeout_at},
};

/// The protocol reached its end marker: `exited` (session) or `archive_end`
/// (export) was written. Never a provider or session success.
pub const EXIT_COMPLETE: i32 = 0;
/// A Handoff, import, start or export was refused with a closed reason.
pub const EXIT_REFUSED: i32 = 67;
/// The broker sent `stop` before `start` (session) or during the export.
pub const EXIT_STOPPED: i32 = 68;
/// The input ended or failed after the Handoff (session) or the `start`
/// (export), before the end marker. The provider, if any, was killed.
pub const EXIT_BROKER_LOST: i32 = 69;

/// After Ready, the Handoff (session) or `start` (export) must arrive within
/// this window. The broker sends it right after its second observation.
pub(crate) const HANDOFF_TIME: Duration = Duration::from_secs(30);
/// From the Handoff, the whole archive must arrive and be written.
pub(crate) const IMPORT_TIME: Duration = Duration::from_secs(600);
/// From `imported`, `start` must arrive (the broker records its permit).
pub(crate) const START_TIME: Duration = Duration::from_secs(120);
/// After the provider exited, its pipes are drained at most this long: a
/// descendant may keep them open.
pub(crate) const DRAIN_TIME: Duration = Duration::from_secs(1);
/// The helper may end on its own once its stdin is closed.
const HELPER_CLOSE_TIME: Duration = Duration::from_secs(1);
/// Queued frames are written within this bound at the end.
const FLUSH_TIME: Duration = Duration::from_secs(5);
/// Reap loop of PID 1 after `kill(-1, SIGKILL)`.
const REAP_TIME: Duration = Duration::from_secs(2);
/// Loopback port of the egress helper inside the Pod network namespace.
pub(crate) const EGRESS_PORT: u16 = 3128;
pub(crate) const EGRESS_HELPER: &str = "/usr/local/bin/ai-guard-relay";
const WORKSPACE: &str = "/workspace";
const HOME: &str = "/home/agent";
/// Encoded frames queued towards standard output.
const OUTPUT_FRAMES: usize = 8;
/// Data frames queued towards one child's stdin.
const CHILD_INPUT_FRAMES: usize = 4;
const MAX_TREE_DEPTH: usize = 64;

/// How one supervisor run ended. `Refused` carries the closed reason that
/// was sent to the broker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SupervisorEndV3 {
    Complete,
    NoHandoff,
    Protocol,
    Deadline,
    Refused(RefusalV3),
    Stopped,
    BrokerLost,
    Io,
}
pub(crate) fn exit_code(end: SupervisorEndV3) -> i32 {
    match end {
        SupervisorEndV3::Complete => EXIT_COMPLETE,
        SupervisorEndV3::NoHandoff => EXIT_NO_HANDOFF,
        SupervisorEndV3::Protocol => EXIT_PROTOCOL,
        SupervisorEndV3::Deadline => EXIT_DEADLINE,
        SupervisorEndV3::Refused(_) => EXIT_REFUSED,
        SupervisorEndV3::Stopped => EXIT_STOPPED,
        SupervisorEndV3::BrokerLost => EXIT_BROKER_LOST,
        SupervisorEndV3::Io => EXIT_IO,
    }
}
fn diagnostic(end: SupervisorEndV3) -> &'static str {
    match end {
        SupervisorEndV3::Complete => "kubernetes protocol v3: end marker written",
        SupervisorEndV3::NoHandoff => "kubernetes protocol v3: input ended before the handoff",
        SupervisorEndV3::Protocol => "kubernetes protocol v3: frame refused",
        SupervisorEndV3::Deadline => "kubernetes protocol v3: protocol window elapsed",
        SupervisorEndV3::Refused(_) => "kubernetes protocol v3: request refused",
        SupervisorEndV3::Stopped => "kubernetes protocol v3: stopped by the broker",
        SupervisorEndV3::BrokerLost => "kubernetes protocol v3: input ended before the end marker",
        SupervisorEndV3::Io => "kubernetes protocol v3: local stream or runtime failure",
    }
}

/// How the end of a session reaches the processes it leaves behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReapScopeV3 {
    /// PID 1 of the container: `kill(-1, SIGKILL)` reaches every other
    /// process of its PID namespace, then the orphans are reaped.
    ContainerInit,
    /// Not PID 1: only the process groups of the provider and the helper.
    ProcessGroups,
}

/// How the provider is started.
pub(crate) enum LauncherV3 {
    /// `prepare_execution`, then `BootstrapExecution::spawn_child`.
    Provider,
    /// Tests only: the payload still goes through `prepare_execution`, then
    /// this fixed program runs instead of the provider.
    #[cfg(test)]
    Fixed {
        program: PathBuf,
        args: Vec<String>,
        environment: BTreeMap<String, String>,
    },
}

pub(crate) struct SupervisorConfigV3 {
    pub(crate) workspace: PathBuf,
    pub(crate) home: PathBuf,
    pub(crate) helper_program: PathBuf,
    pub(crate) helper_args: Vec<String>,
    pub(crate) egress_port: u16,
    pub(crate) handoff_time: Duration,
    pub(crate) import_time: Duration,
    pub(crate) start_time: Duration,
    pub(crate) launcher: LauncherV3,
    pub(crate) reap: ReapScopeV3,
}
impl SupervisorConfigV3 {
    fn production() -> Self {
        Self {
            workspace: PathBuf::from(WORKSPACE),
            home: PathBuf::from(HOME),
            helper_program: PathBuf::from(EGRESS_HELPER),
            helper_args: vec![
                "serve-egress-v1".into(),
                "--port".into(),
                EGRESS_PORT.to_string(),
            ],
            egress_port: EGRESS_PORT,
            handoff_time: HANDOFF_TIME,
            import_time: IMPORT_TIME,
            start_time: START_TIME,
            launcher: LauncherV3::Provider,
            reap: if std::process::id() == 1 {
                ReapScopeV3::ContainerInit
            } else {
                ReapScopeV3::ProcessGroups
            },
        }
    }
}

/// Fixed proxy environment of the provider, mirroring G05's broker-egress
/// worker environment. These values only help clients find the loopback
/// helper; removing them yields no route (the Pod NetworkPolicy denies all).
fn proxy_environment(port: u16) -> BTreeMap<String, String> {
    let proxy = format!("http://127.0.0.1:{port}");
    let mut environment: BTreeMap<String, String> =
        ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"]
            .into_iter()
            .map(|name| (name.to_owned(), proxy.clone()))
            .collect();
    for name in ["NO_PROXY", "no_proxy"] {
        environment.insert(name.into(), "localhost,127.0.0.1,::1".into());
    }
    environment.insert("AI_GUARD_EGRESS_PROXY".into(), proxy);
    environment
}

// ---- dispatch ----------------------------------------------------------------

/// Entry for the provider binary, on its raw arguments after the program
/// name. `Some(code)` only when the first argument is exactly one of the two
/// v3 modes: alone, the mode runs on the process standard streams; followed
/// by anything (valid UTF-8 or not) it returns `EXIT_PROTOCOL` without
/// building a runtime or reading anything. Every other argv returns `None`.
pub fn dispatch_v3_mode<A: AsRef<OsStr>>(args: &[A]) -> Option<i32> {
    let (mode, extra) = args.split_first()?;
    let mode = match mode.as_ref() {
        value if value == OsStr::new(SESSION_RUN_MODE_ARG) => SessionModeV3::Session,
        value if value == OsStr::new(WORKSPACE_EXPORT_MODE_ARG) => SessionModeV3::Export,
        _ => return None,
    };
    if !extra.is_empty() {
        diagnose("kubernetes protocol v3: argument after the mode; refused");
        return Some(EXIT_PROTOCOL);
    }
    Some(run_on_process_stdio(mode))
}

fn run_on_process_stdio(mode: SessionModeV3) -> i32 {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        diagnose("kubernetes protocol v3: local runtime unavailable");
        return EXIT_IO;
    };
    let config = SupervisorConfigV3::production();
    let end = runtime.block_on(serve(
        mode,
        tokio::io::stdin(),
        tokio::io::stdout(),
        &config,
    ));
    // Same reason as the waiting mode: Tokio's blocking stdin read cannot be
    // cancelled, and waiting for it would withhold the exit code.
    runtime.shutdown_background();
    diagnose(diagnostic(end));
    exit_code(end)
}

fn diagnose(sentence: &'static str) {
    let _ = writeln!(std::io::stderr().lock(), "{sentence}");
}

/// Serves one v3 mode on any pair of streams.
pub(crate) async fn serve<R, W>(
    mode: SessionModeV3,
    mut input: R,
    mut output: W,
    config: &SupervisorConfigV3,
) -> SupervisorEndV3
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let hello = match accept_hello(&mut input, &mut output).await {
        Ok(hello) => hello,
        Err(end) => return end,
    };
    let mut output = Output::start(output);
    let end = match mode {
        SessionModeV3::Session => session(&mut input, &mut output, config, &hello).await,
        SessionModeV3::Export => export(&mut input, &mut output, config).await,
    };
    let flushed = output.finish().await;
    match (end, flushed) {
        (SupervisorEndV3::Complete, false) => SupervisorEndV3::Io,
        (end, _) => end,
    }
}

async fn accept_hello<R, W>(
    input: &mut R,
    output: &mut W,
) -> Result<AcceptedHelloV2, SupervisorEndV3>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let capacity = Arc::new(Semaphore::new(RESERVED_BYTES as usize))
        .try_acquire_many_owned(RESERVED_BYTES)
        .map_err(|_| SupervisorEndV3::Io)
        .and_then(|permit| BootstrapCapacity::new(permit).map_err(|_| SupervisorEndV3::Io))?;
    match bootstrap_waiting_for_attach_hello(input, output, &capacity, MAX_HANDSHAKE_TIME).await {
        Ok(WaitingForAttachHelloV2::ReadyWritten(hello)) => Ok(hello),
        Ok(WaitingForAttachHelloV2::EofBeforeHello) => Err(SupervisorEndV3::NoHandoff),
        Err(
            BootstrapError::Protocol
            | BootstrapError::FrameSize
            | BootstrapError::PartialFrame
            | BootstrapError::Binding,
        ) => Err(SupervisorEndV3::Protocol),
        Err(BootstrapError::Deadline) => Err(SupervisorEndV3::Deadline),
        Err(BootstrapError::Io | BootstrapError::Capacity | BootstrapError::Unavailable) => {
            Err(SupervisorEndV3::Io)
        }
    }
}

// ---- output ------------------------------------------------------------------

/// The one writer of standard output after Ready: encoded frames, in order.
struct Output {
    frames: Option<mpsc::Sender<Vec<u8>>>,
    task: Option<JoinHandle<bool>>,
}
impl Output {
    fn start<W: AsyncWrite + Send + Unpin + 'static>(mut output: W) -> Self {
        let (frames, mut queue) = mpsc::channel::<Vec<u8>>(OUTPUT_FRAMES);
        let task = tokio::spawn(async move {
            while let Some(frame) = queue.recv().await {
                if output.write_all(&frame).await.is_err() || output.flush().await.is_err() {
                    return false;
                }
            }
            true
        });
        Self {
            frames: Some(frames),
            task: Some(task),
        }
    }
    fn sender(&self) -> Option<&mpsc::Sender<Vec<u8>>> {
        self.frames.as_ref()
    }
    async fn control(&self, message: WorkerControlV3) -> bool {
        let Ok(frame) = message.encode() else {
            return false;
        };
        match self.frames.as_ref() {
            Some(frames) => timeout(FLUSH_TIME, frames.send(frame))
                .await
                .is_ok_and(|sent| sent.is_ok()),
            None => false,
        }
    }
    /// Closes the queue and waits for the written end, within `FLUSH_TIME`.
    async fn finish(&mut self) -> bool {
        self.frames.take();
        match self.task.take() {
            Some(task) => matches!(timeout(FLUSH_TIME, task).await, Ok(Ok(true))),
            None => false,
        }
    }
}

// ---- inbound -----------------------------------------------------------------

enum Inbound {
    Control(BrokerControlV3),
    Data(ChannelV3, Vec<u8>),
    End,
}

/// One frame from the broker, decoded and admitted by the order.
async fn inbound<R: AsyncRead + Unpin>(
    reader: &mut FrameReaderV3,
    input: &mut R,
    order: &mut WorkerInboundOrderV3,
) -> Result<Inbound, SessionProtocolError> {
    let Some(frame) = reader.read(input, order.control_limit()).await? else {
        return Ok(Inbound::End);
    };
    admit(frame, order)
}
fn admit(
    frame: FrameV3,
    order: &mut WorkerInboundOrderV3,
) -> Result<Inbound, SessionProtocolError> {
    if frame.channel == ChannelV3::Control {
        let decoded = BrokerControlV3::decode(&frame.payload);
        frame.scrub();
        let message = decoded?;
        order.admit_control(&message)?;
        return Ok(Inbound::Control(message));
    }
    order.admit_data(frame.channel, frame.payload.len())?;
    Ok(Inbound::Data(frame.channel, frame.payload))
}

/// End of a refused frame before `start`: the closed refusal is sent first.
async fn refuse_frame(output: &Output, error: SessionProtocolError) -> SupervisorEndV3 {
    match error {
        SessionProtocolError::Io | SessionProtocolError::Closed => SupervisorEndV3::BrokerLost,
        SessionProtocolError::Deadline => {
            output
                .control(WorkerControlV3::Refused(RefusalV3::Deadline))
                .await;
            SupervisorEndV3::Deadline
        }
        _ => {
            output
                .control(WorkerControlV3::Refused(RefusalV3::Protocol))
                .await;
            SupervisorEndV3::Protocol
        }
    }
}
async fn refuse(output: &Output, reason: RefusalV3) -> SupervisorEndV3 {
    output.control(WorkerControlV3::Refused(reason)).await;
    SupervisorEndV3::Refused(reason)
}
async fn stopped(output: &Output) -> SupervisorEndV3 {
    output
        .control(WorkerControlV3::Refused(RefusalV3::Stopped))
        .await;
    SupervisorEndV3::Stopped
}

// ---- session mode --------------------------------------------------------------

async fn session<R: AsyncRead + Unpin>(
    input: &mut R,
    output: &mut Output,
    config: &SupervisorConfigV3,
    hello: &AcceptedHelloV2,
) -> SupervisorEndV3 {
    let mut reader = FrameReaderV3::new();
    let mut order = WorkerInboundOrderV3::new(SessionModeV3::Session);
    let until = Instant::now() + config.handoff_time;
    let handoff = match timeout_at(until, inbound(&mut reader, input, &mut order)).await {
        Err(_) => return refuse_frame(output, SessionProtocolError::Deadline).await,
        Ok(Err(SessionProtocolError::Io)) => return SupervisorEndV3::Io,
        Ok(Err(error)) => return refuse_frame(output, error).await,
        Ok(Ok(Inbound::End)) => return SupervisorEndV3::NoHandoff,
        Ok(Ok(Inbound::Control(BrokerControlV3::Handoff(handoff)))) => handoff,
        Ok(Ok(_)) => return refuse_frame(output, SessionProtocolError::Protocol).await,
    };
    let (profile, prepared) = match validate_handoff(hello, &handoff, config) {
        Ok(validated) => validated,
        Err(reason) => return refuse(output, reason).await,
    };
    let import_until = Instant::now() + config.import_time;
    let archive =
        match receive_archive(&mut reader, input, &mut order, &handoff, import_until).await {
            Ok(archive) => archive,
            Err(Received::Stopped) => return stopped(output).await,
            Err(Received::Refused(reason)) => return refuse(output, reason).await,
            Err(Received::Frame(error)) => return refuse_frame(output, error).await,
            Err(Received::Lost) => return SupervisorEndV3::BrokerLost,
        };
    let expected = handoff.archive_sha256().to_owned();
    drop(handoff);
    let workspace = config.workspace.clone();
    let import = tokio::task::spawn_blocking(move || {
        import_archive(&archive, &expected, &profile, &workspace)
    });
    let imported = match timeout_at(import_until, import).await {
        Err(_) => return refuse_frame(output, SessionProtocolError::Deadline).await,
        Ok(Err(_)) => return refuse(output, RefusalV3::ImportFailed).await,
        Ok(Ok(Err(reason))) => return refuse(output, reason).await,
        Ok(Ok(Ok(imported))) => imported,
    };
    if order.imported().is_err() || !output.control(WorkerControlV3::Imported(imported)).await {
        return SupervisorEndV3::Io;
    }
    let start_until = Instant::now() + config.start_time;
    match timeout_at(start_until, inbound(&mut reader, input, &mut order)).await {
        Err(_) => return refuse_frame(output, SessionProtocolError::Deadline).await,
        Ok(Err(error)) => return refuse_frame(output, error).await,
        Ok(Ok(Inbound::End)) => return SupervisorEndV3::BrokerLost,
        Ok(Ok(Inbound::Control(BrokerControlV3::Start))) => {}
        Ok(Ok(Inbound::Control(BrokerControlV3::Stop(_)))) => return stopped(output).await,
        Ok(Ok(_)) => return refuse_frame(output, SessionProtocolError::Protocol).await,
    }
    run(reader, input, order, output, config, prepared).await
}

enum PreparedV3 {
    Provider(crate::provider::bootstrap::BootstrapExecution),
    #[cfg(test)]
    Fixed {
        program: PathBuf,
        args: Vec<String>,
        environment: BTreeMap<String, String>,
    },
}

/// Every check of the Handoff, before any byte of the archive is accepted.
fn validate_handoff(
    hello: &AcceptedHelloV2,
    handoff: &HandoffV3,
    config: &SupervisorConfigV3,
) -> Result<(KubernetesSessionProfileV2, PreparedV3), RefusalV3> {
    let profile = KubernetesSessionProfileV2::from_canonical_bytes(handoff.profile_bytes())
        .map_err(|_| RefusalV3::ProfileMismatch)?;
    let ids = profile
        .retained_ids()
        .map_err(|_| RefusalV3::ProfileMismatch)?;
    let profile_ids = [
        ids.installation_id,
        ids.control_namespace_uuid,
        ids.broker_id,
        ids.incarnation_id,
        ids.session_id,
        ids.attempt_id,
    ];
    if profile.profile_hash().ok().as_deref() != Some(hello.profile_hash())
        || profile.retained_authority_fingerprint() != hello.authority_fingerprint()
        || profile_ids != hello.ids()
    {
        return Err(RefusalV3::ProfileMismatch);
    }
    let invocation = KubernetesSessionInvocationV2::new(handoff.program(), handoff.args())
        .map_err(|_| RefusalV3::InvocationMismatch)?;
    profile
        .verify_invocation(invocation)
        .map_err(|_| RefusalV3::InvocationMismatch)?;
    let provider = profile
        .retained_provider()
        .map_err(|_| RefusalV3::ProfileMismatch)?;
    if provider.transport.is_interactive() {
        return Err(RefusalV3::InteractiveUnsupported);
    }
    let (_, _, committed_bytes) = profile.retained_workspace_commitment();
    if committed_bytes > handoff.archive_bytes() {
        return Err(RefusalV3::ArchiveMismatch);
    }
    let argv: Vec<String> = std::iter::once(handoff.program().to_owned())
        .chain(handoff.args().iter().cloned())
        .collect();
    let execution = crate::provider::bootstrap::prepare_execution(
        &argv,
        handoff.bootstrap().expose(),
        &config.home,
    )
    .map_err(|_| RefusalV3::CredentialInvalid);
    let prepared = match &config.launcher {
        LauncherV3::Provider => PreparedV3::Provider(execution?),
        #[cfg(test)]
        LauncherV3::Fixed {
            program,
            args,
            environment,
        } => {
            drop(execution?);
            PreparedV3::Fixed {
                program: program.clone(),
                args: args.clone(),
                environment: environment.clone(),
            }
        }
    };
    Ok((profile, prepared))
}

enum Received {
    Stopped,
    Refused(RefusalV3),
    Frame(SessionProtocolError),
    Lost,
}

async fn receive_archive<R: AsyncRead + Unpin>(
    reader: &mut FrameReaderV3,
    input: &mut R,
    order: &mut WorkerInboundOrderV3,
    handoff: &HandoffV3,
    until: Instant,
) -> Result<Vec<u8>, Received> {
    let declared = usize::try_from(handoff.archive_bytes())
        .map_err(|_| Received::Refused(RefusalV3::ArchiveMismatch))?;
    let mut archive = Vec::new();
    archive
        .try_reserve_exact(declared)
        .map_err(|_| Received::Refused(RefusalV3::ImportFailed))?;
    while !order.archive_complete() {
        match timeout_at(until, inbound(reader, input, order)).await {
            Err(_) => return Err(Received::Frame(SessionProtocolError::Deadline)),
            Ok(Err(SessionProtocolError::Io)) => return Err(Received::Lost),
            Ok(Err(error)) => return Err(Received::Frame(error)),
            Ok(Ok(Inbound::End)) => return Err(Received::Lost),
            Ok(Ok(Inbound::Data(ChannelV3::Archive, bytes))) => archive.extend_from_slice(&bytes),
            Ok(Ok(Inbound::Control(BrokerControlV3::Stop(_)))) => return Err(Received::Stopped),
            Ok(Ok(_)) => return Err(Received::Frame(SessionProtocolError::Protocol)),
        }
    }
    Ok(archive)
}

/// Digest, scan, commitment, then the write. Blocking: run off the runtime.
fn import_archive(
    archive: &[u8],
    expected_sha256: &str,
    profile: &KubernetesSessionProfileV2,
    workspace: &Path,
) -> Result<ImportedV3, RefusalV3> {
    if hex::encode(Sha256::digest(archive)) != expected_sha256 {
        return Err(RefusalV3::ArchiveMismatch);
    }
    let manifest = scan_workspace_archive(archive, Limits::default())
        .map_err(|_| RefusalV3::WorkspaceInvalid)?;
    profile
        .verify_workspace(&manifest)
        .map_err(|_| RefusalV3::WorkspaceMismatch)?;
    write_workspace(archive, &manifest, workspace)?;
    let (manifest_sha256, files, bytes) = profile.retained_workspace_commitment();
    Ok(ImportedV3 {
        manifest_sha256: manifest_sha256.to_owned(),
        files,
        bytes,
    })
}

fn c_name(name: &OsStr) -> Result<CString, RefusalV3> {
    CString::new(name.as_bytes()).map_err(|_| RefusalV3::ImportFailed)
}
fn open_at(
    parent: &File,
    name: &CString,
    flags: libc::c_int,
    mode: libc::c_uint,
) -> std::io::Result<File> {
    // SAFETY: a retained directory FD and one NUL-terminated component; the
    // resulting FD is owned at once.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            mode,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a successful openat transfers one live FD.
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn open_directory(path: &Path) -> std::io::Result<File> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: one NUL-terminated path; the resulting FD is owned at once.
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a successful open transfers one live FD.
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn child_directory(parent: &File, name: &CString, create: bool) -> Result<File, RefusalV3> {
    if create {
        // SAFETY: relative to a retained directory FD, one component.
        let made = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o755) };
        if made != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
            return Err(RefusalV3::ImportFailed);
        }
    }
    open_at(parent, name, libc::O_RDONLY | libc::O_DIRECTORY, 0)
        .map_err(|_| RefusalV3::ImportFailed)
}
fn components(path: &Path) -> Vec<&OsStr> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect()
}
/// Opens (and creates) the directories of `names` below `root`.
fn directories(root: &File, names: &[&OsStr]) -> Result<File, RefusalV3> {
    let mut current = root.try_clone().map_err(|_| RefusalV3::ImportFailed)?;
    for name in names {
        current = child_directory(&current, &c_name(name)?, true)?;
    }
    Ok(current)
}

/// Writes an archive that `scan_workspace_archive` accepted into an empty
/// workspace. Every creation is relative to a retained directory FD,
/// exclusive and no-follow; file contents are hashed again against the
/// scanned manifest, which the profile commitment already matched.
fn write_workspace(archive: &[u8], manifest: &Manifest, workspace: &Path) -> Result<(), RefusalV3> {
    let root = open_directory(workspace).map_err(|_| RefusalV3::ImportFailed)?;
    let facts = root.metadata().map_err(|_| RefusalV3::ImportFailed)?;
    let named = std::fs::symlink_metadata(workspace).map_err(|_| RefusalV3::ImportFailed)?;
    if facts.dev() != named.dev() || facts.ino() != named.ino() {
        return Err(RefusalV3::ImportFailed);
    }
    if std::fs::read_dir(workspace)
        .map_err(|_| RefusalV3::ImportFailed)?
        .next()
        .is_some()
    {
        return Err(RefusalV3::WorkspaceNotEmpty);
    }
    let mut entries = tar::Archive::new(archive);
    for entry in entries.entries().map_err(|_| RefusalV3::ImportFailed)? {
        let mut entry = entry.map_err(|_| RefusalV3::ImportFailed)?;
        let path = entry
            .path()
            .map_err(|_| RefusalV3::ImportFailed)
            .and_then(|path| validate_relative_path(&path).map_err(|_| RefusalV3::ImportFailed))?;
        let kind = entry.header().entry_type();
        let names = components(&path);
        if names.is_empty() {
            continue;
        }
        if kind.is_dir() {
            directories(&root, &names)?;
            continue;
        }
        if !kind.is_file() {
            return Err(RefusalV3::WorkspaceInvalid);
        }
        let key = path.to_str().ok_or(RefusalV3::ImportFailed)?;
        let fingerprint = manifest.get(key).ok_or(RefusalV3::ImportFailed)?;
        let mode = match fingerprint.mode {
            Some(mode @ (0o644 | 0o755)) => mode,
            _ => return Err(RefusalV3::WorkspaceMismatch),
        };
        let (name, parents) = names.split_last().ok_or(RefusalV3::ImportFailed)?;
        let parent = directories(&root, parents)?;
        let mut file = open_at(
            &parent,
            &c_name(name)?,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )
        .map_err(|_| RefusalV3::ImportFailed)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 16 * 1024];
        let mut written = 0_u64;
        loop {
            let count = entry
                .read(&mut buffer)
                .map_err(|_| RefusalV3::ImportFailed)?;
            if count == 0 {
                break;
            }
            written += count as u64;
            hasher.update(&buffer[..count]);
            file.write_all(&buffer[..count])
                .map_err(|_| RefusalV3::ImportFailed)?;
        }
        if written != fingerprint.bytes || hex::encode(hasher.finalize()) != fingerprint.sha256 {
            return Err(RefusalV3::WorkspaceMismatch);
        }
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(mode))
            .map_err(|_| RefusalV3::ImportFailed)?;
        file.sync_all().map_err(|_| RefusalV3::ImportFailed)?;
    }
    root.sync_all().map_err(|_| RefusalV3::ImportFailed)
}

// ---- running -----------------------------------------------------------------

/// Feeds one child's stdin from a bounded queue; closing the queue closes
/// the stdin once the queued bytes are written.
fn feeder(mut stdin: ChildStdin) -> mpsc::Sender<Vec<u8>> {
    let (sender, mut queue) = mpsc::channel::<Vec<u8>>(CHILD_INPUT_FRAMES);
    tokio::spawn(async move {
        while let Some(bytes) = queue.recv().await {
            if stdin.write_all(&bytes).await.is_err() {
                break;
            }
        }
        drop(stdin);
    });
    sender
}

/// Reads one optional stream; a missing stream never resolves.
async fn read_some<S: AsyncRead + Unpin>(
    stream: &mut Option<S>,
    buffer: &mut [u8],
) -> std::io::Result<usize> {
    match stream.as_mut() {
        Some(stream) => stream.read(buffer).await,
        None => std::future::pending().await,
    }
}

fn kill_group(group: Option<u32>) {
    if let Some(group) = group
        .and_then(|group| i32::try_from(group).ok())
        .filter(|group| *group > 1)
    {
        // SAFETY: each child was created in its own process group; a
        // negative id addresses that group, never the supervisor's.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
}

/// PID 1 only: every other process of the PID namespace, then the orphans.
async fn reap_container() {
    // SAFETY: as PID 1 of the container, -1 addresses every other process of
    // its PID namespace; the caller checked the scope.
    unsafe {
        libc::kill(-1, libc::SIGKILL);
    }
    let until = Instant::now() + REAP_TIME;
    loop {
        let mut status = 0;
        // SAFETY: reaps any child without blocking.
        let reaped = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if reaped < 0 || Instant::now() >= until {
            return;
        }
        if reaped == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

fn spawn_helper(config: &SupervisorConfigV3) -> std::io::Result<Child> {
    tokio::process::Command::new(&config.helper_program)
        .args(&config.helper_args)
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Fixed G05 diagnostics only; they go to the container log.
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .process_group(0)
        .spawn()
}

async fn spawn_provider(prepared: PreparedV3, config: &SupervisorConfigV3) -> Option<Child> {
    let proxy = proxy_environment(config.egress_port);
    match prepared {
        PreparedV3::Provider(execution) => {
            execution.spawn_child(&config.workspace, &proxy).await.ok()
        }
        #[cfg(test)]
        PreparedV3::Fixed {
            program,
            args,
            mut environment,
        } => {
            environment.extend(proxy);
            tokio::process::Command::new(program)
                .args(args)
                .env_clear()
                .envs(environment)
                .current_dir(&config.workspace)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .process_group(0)
                .spawn()
                .ok()
        }
    }
}

/// Data frame towards standard output, refused only by its own bound.
fn data_frame(channel: ChannelV3, bytes: &[u8]) -> Option<Vec<u8>> {
    encode_frame(channel, bytes).ok()
}

#[allow(
    clippy::too_many_lines,
    reason = "one event loop: every branch shares the same owned streams"
)]
async fn run<R: AsyncRead + Unpin>(
    mut reader: FrameReaderV3,
    input: &mut R,
    mut order: WorkerInboundOrderV3,
    output: &mut Output,
    config: &SupervisorConfigV3,
    prepared: PreparedV3,
) -> SupervisorEndV3 {
    let mut helper = match spawn_helper(config) {
        Ok(helper) => helper,
        Err(_) => return refuse(output, RefusalV3::EgressUnavailable).await,
    };
    let helper_group = helper.id();
    let mut helper_stdout = helper.stdout.take();
    let mut helper_stdin = helper.stdin.take().map(feeder);
    let Some(mut child) = spawn_provider(prepared, config).await else {
        drop(helper_stdin.take());
        kill_group(helper_group);
        let _ = timeout(HELPER_CLOSE_TIME, helper.wait()).await;
        return refuse(output, RefusalV3::ProviderUnavailable).await;
    };
    let child_group = child.id();
    let mut child_stdin = child.stdin.take().map(feeder);
    let mut child_stdout = child.stdout.take();
    let mut child_stderr = child.stderr.take();
    if !output.control(WorkerControlV3::Started).await {
        kill_group(child_group);
        kill_group(helper_group);
        return SupervisorEndV3::Io;
    }
    let Some(frames) = output.sender().cloned() else {
        kill_group(child_group);
        kill_group(helper_group);
        return SupervisorEndV3::Io;
    };
    let mut out_buffer = vec![0_u8; MAX_DATA_BYTES];
    let mut err_buffer = vec![0_u8; MAX_DATA_BYTES];
    let mut egress_buffer = vec![0_u8; MAX_DATA_BYTES];
    let mut pending_out: Option<Vec<u8>> = None;
    let mut pending_in: Option<(ChannelV3, Vec<u8>)> = None;
    let mut input_open = true;
    let mut cause: Option<SupervisorEndV3> = None;
    let mut status = None;
    let mut drain_until: Option<Instant> = None;
    loop {
        if status.is_some()
            && pending_out.is_none()
            && child_stdout.is_none()
            && child_stderr.is_none()
        {
            break;
        }
        let drain_deadline =
            drain_until.unwrap_or_else(|| Instant::now() + Duration::from_secs(86_400));
        let reading_input = input_open && pending_in.is_none() && status.is_none();
        // The queue that the pending input waits for, cloned for this round
        // so that a handler may close the original.
        let input_queue = match pending_in.as_ref().map(|(channel, _)| *channel) {
            Some(ChannelV3::Stdin) => child_stdin.clone(),
            Some(_) => helper_stdin.clone(),
            None => None,
        };
        tokio::select! {
            biased;
            () = sleep_until(drain_deadline), if drain_until.is_some() => break,
            waited = child.wait(), if status.is_none() => {
                status = Some(waited.ok());
                drain_until = Some(Instant::now() + DRAIN_TIME);
                child_stdin.take();
            }
            permit = frames.reserve(), if pending_out.is_some() => match permit {
                Ok(permit) => {
                    if let Some(frame) = pending_out.take() {
                        permit.send(frame);
                    }
                }
                Err(_) => {
                    cause.get_or_insert(SupervisorEndV3::Io);
                    kill_group(child_group);
                    pending_out = None;
                    child_stdout = None;
                    child_stderr = None;
                    helper_stdout = None;
                }
            },
            permit = async {
                match input_queue.as_ref() {
                    Some(queue) => queue.reserve().await.ok(),
                    None => None,
                }
            }, if pending_in.is_some() => {
                // A closed queue (child or helper gone) drops the bytes.
                if let (Some(permit), Some((_, bytes))) = (permit, pending_in.take()) {
                    permit.send(bytes);
                }
                pending_in = None;
            }
            received = inbound(&mut reader, input, &mut order), if reading_input => match received {
                Ok(Inbound::Data(channel, bytes)) => pending_in = Some((channel, bytes)),
                Ok(Inbound::Control(BrokerControlV3::StdinClose)) => {
                    child_stdin.take();
                }
                Ok(Inbound::Control(BrokerControlV3::Stop(_))) => {
                    // Nothing is accepted after Stop: the next input event
                    // (often the broker's close) is not read.
                    kill_group(child_group);
                    let _ = child.start_kill();
                    input_open = false;
                }
                Ok(Inbound::Control(_)) => {
                    cause.get_or_insert(SupervisorEndV3::Protocol);
                    kill_group(child_group);
                    let _ = child.start_kill();
                    input_open = false;
                }
                Ok(Inbound::End) | Err(SessionProtocolError::Io) => {
                    cause.get_or_insert(SupervisorEndV3::BrokerLost);
                    kill_group(child_group);
                    let _ = child.start_kill();
                    input_open = false;
                }
                Err(_) => {
                    cause.get_or_insert(SupervisorEndV3::Protocol);
                    kill_group(child_group);
                    let _ = child.start_kill();
                    input_open = false;
                }
            },
            read = read_some(&mut child_stdout, &mut out_buffer), if pending_out.is_none() && child_stdout.is_some() => match read {
                Ok(count) if count > 0 => pending_out = data_frame(ChannelV3::Stdout, &out_buffer[..count]),
                _ => child_stdout = None,
            },
            read = read_some(&mut child_stderr, &mut err_buffer), if pending_out.is_none() && child_stderr.is_some() => match read {
                Ok(count) if count > 0 => pending_out = data_frame(ChannelV3::Stderr, &err_buffer[..count]),
                _ => child_stderr = None,
            },
            read = read_some(&mut helper_stdout, &mut egress_buffer), if pending_out.is_none() && helper_stdout.is_some() && status.is_none() => match read {
                Ok(count) if count > 0 => pending_out = data_frame(ChannelV3::Egress, &egress_buffer[..count]),
                _ => helper_stdout = None,
            },
        }
    }
    drop(frames);
    if status.is_none() {
        // The drain bound ended the loop before the exit was seen: the
        // provider is killed and waited for a bounded time.
        kill_group(child_group);
        let _ = child.start_kill();
        status = Some(
            timeout(DRAIN_TIME, child.wait())
                .await
                .ok()
                .and_then(Result::ok),
        );
    }
    let declared = status.flatten().map_or(
        ExitedV3 {
            code: None,
            signal: Some(libc::SIGKILL),
        },
        |status| ExitedV3 {
            code: status.code(),
            signal: if status.code().is_some() {
                None
            } else {
                status.signal()
            },
        },
    );
    let exited = output.control(WorkerControlV3::Exited(declared)).await;
    // Close the helper, then reach every remaining process.
    drop(helper_stdin.take());
    drop(helper_stdout.take());
    if timeout(HELPER_CLOSE_TIME, helper.wait()).await.is_err() {
        kill_group(helper_group);
        let _ = helper.start_kill();
        let _ = timeout(HELPER_CLOSE_TIME, helper.wait()).await;
    }
    match config.reap {
        ReapScopeV3::ContainerInit => reap_container().await,
        ReapScopeV3::ProcessGroups => {
            kill_group(child_group);
            kill_group(helper_group);
        }
    }
    match cause {
        Some(cause) => cause,
        None if exited => SupervisorEndV3::Complete,
        None => SupervisorEndV3::Io,
    }
}

// ---- export mode ----------------------------------------------------------------

async fn export<R: AsyncRead + Unpin>(
    input: &mut R,
    output: &mut Output,
    config: &SupervisorConfigV3,
) -> SupervisorEndV3 {
    let mut reader = FrameReaderV3::new();
    let mut order = WorkerInboundOrderV3::new(SessionModeV3::Export);
    let until = Instant::now() + config.handoff_time;
    match timeout_at(until, inbound(&mut reader, input, &mut order)).await {
        Err(_) => return refuse_frame(output, SessionProtocolError::Deadline).await,
        Ok(Err(SessionProtocolError::Io)) => return SupervisorEndV3::Io,
        Ok(Err(error)) => return refuse_frame(output, error).await,
        Ok(Ok(Inbound::End)) => return SupervisorEndV3::NoHandoff,
        Ok(Ok(Inbound::Control(BrokerControlV3::Start))) => {}
        Ok(Ok(Inbound::Control(BrokerControlV3::Stop(_)))) => return stopped(output).await,
        Ok(Ok(_)) => return refuse_frame(output, SessionProtocolError::Protocol).await,
    }
    let Some(frames) = output.sender().cloned() else {
        return SupervisorEndV3::Io;
    };
    let cancelled = Arc::new(AtomicBool::new(false));
    let sink = ArchiveSink {
        frames,
        buffer: Vec::with_capacity(MAX_DATA_BYTES),
        hasher: Sha256::new(),
        total: 0,
        cancelled: cancelled.clone(),
    };
    let workspace = config.workspace.clone();
    let mut exporting = tokio::task::spawn_blocking(move || export_workspace(&workspace, sink));
    let mut cause = None;
    let result = tokio::select! {
        biased;
        done = &mut exporting => done,
        received = inbound(&mut reader, input, &mut order) => {
            cause = Some(match received {
                Ok(Inbound::Control(BrokerControlV3::Stop(_))) => SupervisorEndV3::Stopped,
                Ok(Inbound::End) | Err(SessionProtocolError::Io) => SupervisorEndV3::BrokerLost,
                _ => SupervisorEndV3::Protocol,
            });
            // The export thread stops at its next frame; a blocked output
            // cannot hold this end beyond the flush bound.
            cancelled.store(true, Ordering::Release);
            match timeout(FLUSH_TIME, &mut exporting).await {
                Ok(done) => done,
                Err(_) => Ok(Err(RefusalV3::ExportFailed)),
            }
        }
    };
    if let Some(cause) = cause {
        if cause == SupervisorEndV3::Stopped {
            return stopped(output).await;
        }
        if cause == SupervisorEndV3::Protocol {
            return refuse_frame(output, SessionProtocolError::Protocol).await;
        }
        return cause;
    }
    match result {
        Ok(Ok(end)) => {
            if output.control(WorkerControlV3::ArchiveEnd(end)).await {
                SupervisorEndV3::Complete
            } else {
                SupervisorEndV3::Io
            }
        }
        Ok(Err(reason)) => refuse(output, reason).await,
        Err(_) => refuse(output, RefusalV3::ExportFailed).await,
    }
}

/// Synchronous sink of the export tar: frames of at most 64 KiB, hashed.
struct ArchiveSink {
    frames: mpsc::Sender<Vec<u8>>,
    buffer: Vec<u8>,
    hasher: Sha256,
    total: u64,
    cancelled: Arc<AtomicBool>,
}
impl ArchiveSink {
    fn emit(&mut self) -> std::io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        if self.cancelled.load(Ordering::Acquire) {
            return Err(std::io::Error::other("export cancelled"));
        }
        let frame = encode_frame(ChannelV3::Archive, &self.buffer)
            .map_err(|_| std::io::Error::other("archive frame"))?;
        self.buffer.clear();
        self.frames
            .blocking_send(frame)
            .map_err(|_| std::io::Error::other("output closed"))
    }
    fn end(mut self) -> std::io::Result<ArchiveEndV3> {
        self.emit()?;
        Ok(ArchiveEndV3 {
            archive_bytes: self.total,
            archive_sha256: hex::encode(self.hasher.finalize()),
        })
    }
}
impl Write for ArchiveSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let room = MAX_DATA_BYTES - self.buffer.len();
        let take = room.min(bytes.len());
        let accepted = &bytes[..take];
        let total = self
            .total
            .checked_add(take as u64)
            .filter(|total| *total <= Limits::default().max_archive_bytes() as u64)
            .ok_or_else(|| std::io::Error::other("archive limit"))?;
        self.total = total;
        self.hasher.update(accepted);
        self.buffer.extend_from_slice(accepted);
        if self.buffer.len() == MAX_DATA_BYTES {
            self.emit()?;
        }
        Ok(take)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct ExportCount {
    entries: usize,
    bytes: u64,
}

/// Names of one directory, by its FD, sorted. `.` and `..` are skipped.
fn directory_names(directory: &File) -> Result<Vec<OsString>, RefusalV3> {
    // SAFETY: duplicates a retained FD; the copy is owned by the stream.
    let fd = unsafe { libc::dup(directory.as_raw_fd()) };
    if fd < 0 {
        return Err(RefusalV3::ExportFailed);
    }
    // SAFETY: a live FD whose ownership passes to the directory stream.
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        // SAFETY: fdopendir failed, so the FD is still ours.
        unsafe { libc::close(fd) };
        return Err(RefusalV3::ExportFailed);
    }
    let mut names = Vec::new();
    loop {
        // SAFETY: a live stream owned by this function.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        // SAFETY: readdir returned a valid entry with a NUL-terminated name.
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
        let bytes = name.to_bytes();
        if bytes != b"." && bytes != b".." {
            names.push(OsStr::from_bytes(bytes).to_owned());
        }
    }
    // SAFETY: closes the stream and its FD exactly once.
    unsafe { libc::closedir(stream) };
    names.sort();
    Ok(names)
}

fn export_workspace(workspace: &Path, sink: ArchiveSink) -> Result<ArchiveEndV3, RefusalV3> {
    let root = open_directory(workspace).map_err(|_| RefusalV3::ExportFailed)?;
    let mut builder = tar::Builder::new(sink);
    let mut count = ExportCount {
        entries: 0,
        bytes: 0,
    };
    export_directory(&root, Path::new(""), 0, &mut builder, &mut count)?;
    let sink = builder.into_inner().map_err(|_| RefusalV3::ExportFailed)?;
    sink.end().map_err(|_| RefusalV3::ExportFailed)
}

fn export_directory(
    directory: &File,
    prefix: &Path,
    depth: usize,
    builder: &mut tar::Builder<ArchiveSink>,
    count: &mut ExportCount,
) -> Result<(), RefusalV3> {
    let limits = Limits::default();
    if depth >= MAX_TREE_DEPTH {
        return Err(RefusalV3::ExportLimit);
    }
    for name in directory_names(directory)? {
        let encoded = CString::new(name.as_bytes()).map_err(|_| RefusalV3::ExportFailed)?;
        let path = prefix.join(&name);
        let mut facts = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: relative to a retained directory FD, no-follow, writable out.
        let stat = unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                encoded.as_ptr(),
                facts.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if stat != 0 {
            return Err(RefusalV3::ExportFailed);
        }
        // SAFETY: a successful fstatat initialized the structure.
        let facts = unsafe { facts.assume_init() };
        count.entries += 1;
        if count.entries > limits.max_files {
            return Err(RefusalV3::ExportLimit);
        }
        let mut header = tar::Header::new_gnu();
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        match facts.st_mode & libc::S_IFMT {
            libc::S_IFDIR => {
                let child = open_at(directory, &encoded, libc::O_RDONLY | libc::O_DIRECTORY, 0)
                    .map_err(|_| RefusalV3::ExportFailed)?;
                header.set_entry_type(tar::EntryType::Directory);
                header.set_mode(0o755);
                header.set_size(0);
                builder
                    .append_data(&mut header, &path, std::io::empty())
                    .map_err(|_| RefusalV3::ExportFailed)?;
                export_directory(&child, &path, depth + 1, builder, count)?;
            }
            libc::S_IFREG => {
                let file = open_at(directory, &encoded, libc::O_RDONLY, 0)
                    .map_err(|_| RefusalV3::ExportFailed)?;
                let opened = file.metadata().map_err(|_| RefusalV3::ExportFailed)?;
                if !opened.is_file() || opened.ino() != facts.st_ino {
                    return Err(RefusalV3::ExportFailed);
                }
                let size = opened.len();
                count.bytes = count
                    .bytes
                    .checked_add(size)
                    .ok_or(RefusalV3::ExportLimit)?;
                if size > limits.max_file_bytes || count.bytes > limits.max_bytes {
                    return Err(RefusalV3::ExportLimit);
                }
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(if opened.mode() & 0o111 != 0 {
                    0o755
                } else {
                    0o644
                });
                header.set_size(size);
                builder
                    .append_data(&mut header, &path, file.take(size))
                    .map_err(|_| RefusalV3::ExportFailed)?;
            }
            _ => return Err(RefusalV3::ExportUnsupportedEntry),
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "session_supervisor_tests.rs"]
pub(crate) mod tests;
