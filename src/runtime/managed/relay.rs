//! Restricted, separate stdio exec for the worker relay helper.
//!
//! This is an internal byte transport, not a workload identity, a start permit
//! or a terminal/quota witness. Production admission consumes the broker's
//! opaque verified source and one fresh Core exec claim. The registry retains
//! this owner until explicit close/join and exact terminal reconciliation.

use super::*;
use futures_util::task::AtomicWaker;
use std::{
    io,
    pin::Pin,
    process::ExitStatus,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context as TaskContext, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf},
    process::{Child, ChildStderr, ChildStdin, ChildStdout},
    task::JoinHandle,
    time::timeout_at,
};

const HELPER: &str = "/usr/local/bin/ai-guard-relay";
const STDERR_LIMIT: usize = 16 * 1024;
const CLOSE_TIME: Duration = Duration::from_secs(5);
const EOF_GRACE: Duration = Duration::from_secs(2);
// The handoff currently carries only AI_GUARD_PROVIDER_BOOTSTRAP in Config.Env;
// the other names are the supported clients' recognized credential/config
// selectors. Each override is a literal empty value, never a secret in argv.
const EMPTY_EXEC_ENV: &[&str] = &[
    "AI_GUARD_PROVIDER_BOOTSTRAP",
    "OPENAI_API_KEY",
    "CODEX_API_KEY",
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CODEX_HOME",
    "CLAUDE_CONFIG_DIR",
];

/// Closed diagnostics never retain stderr, inspection JSON or OS error text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RelayExecError {
    Identity,
    Profile,
    Authority,
    Unavailable,
    Closed,
    NeedsReconciliation,
}
impl std::fmt::Display for RelayExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Identity => "relay exec incarnation or configuration is not current",
            Self::Profile => "relay exec profile is not approved or available",
            Self::Authority => "relay exec authority is unavailable or changed",
            Self::Unavailable => "relay exec transport is unavailable",
            Self::Closed => "relay exec transport is closed",
            Self::NeedsReconciliation => {
                "relay exec outcome is unknown; retain the owner and reconcile"
            }
        })
    }
}
impl std::error::Error for RelayExecError {}

/// A consumed capability distinct from every start guard. Its private admission
/// field is built only from the broker's opaque composition. Neither a CID nor
/// worker frames can produce this value.
pub(crate) struct RelayExecPermit {
    snapshot: OwnedContainerSnapshot,
    authorized: ContainerSpec,
    authority: String,
    invocation: String,
    profile: RelayExecProfile,
    _admission: sealed::DurableRelayAdmission,
}
mod sealed {
    pub(super) struct DurableRelayAdmission {
        pub(super) _claim: crate::runner::relay_admission::FreshRelayExecClaim,
        pub(super) authority: std::sync::Arc<crate::broker::relay_owner::DispatchAuthority>,
    }
}
#[derive(Clone, PartialEq, Eq)]
enum RelayExecProfile {
    Docker(DockerLaunchProfileBindingV1),
    Podman(PodmanBinding),
}
impl std::fmt::Debug for RelayExecPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RelayExecPermit([private consumed admission])")
    }
}

impl ManagedRuntime {
    /// The issuer accepts one opaque broker composition, not free runtime DTOs
    /// or a caller assertion. Its fresh SDK claim is consumed exactly once.
    pub(crate) fn prepare_relay_exec(
        &self,
        admission: crate::broker::relay_owner::VerifiedRelayExecAdmission,
    ) -> std::result::Result<RelayExecPermit, RelayExecError> {
        let (fresh, snapshot, authorized, authority) = admission.into_parts();
        let input = fresh.admission();
        if input.inspection.container_id != snapshot.handle.id
            || input.session_id != snapshot.handle.session.to_string()
            || input.session_id != authorized.session.to_string()
            || input.authority_binding_hash != self.binding.fingerprint()
            || input.configuration_fingerprint_version != snapshot.handle.fingerprint_version()
            || Some(&input.configuration_fingerprint)
                != snapshot.observation.configuration_fingerprint.as_ref()
            || input.started_at
                != snapshot
                    .observation
                    .process
                    .started_at
                    .as_ref()
                    .map(|at| at.to_rfc3339())
                    .unwrap_or_default()
        {
            return Err(RelayExecError::Identity);
        }
        validate_running_snapshot(&snapshot, &snapshot.observation)?;
        validate_authorized_identity(&authorized, &snapshot.observation)
            .map_err(|_| RelayExecError::Profile)?;
        Ok(RelayExecPermit {
            snapshot,
            authorized,
            authority: self.binding.fingerprint(),
            invocation: self.runtime.invocation().fingerprint(),
            profile: self.relay_profile()?,
            _admission: sealed::DurableRelayAdmission {
                _claim: fresh,
                authority,
            },
        })
    }

    pub(crate) async fn revalidate_relay_channel(
        &self,
        snapshot: &OwnedContainerSnapshot,
        authorized: &ContainerSpec,
    ) -> std::result::Result<(), RelayExecError> {
        self.validate_handle(&snapshot.handle)
            .map_err(|_| RelayExecError::Identity)?;
        self.relay_profile()?;
        let facts = self
            .inspect_facts(&snapshot.handle.id)
            .await
            .map_err(|_| RelayExecError::Authority)?;
        let observed = parse_container_observation_version(
            &facts.bytes,
            self.binding.kind,
            snapshot.handle.fingerprint_version(),
        )
        .map_err(|_| RelayExecError::Identity)?;
        validate_running_snapshot(snapshot, &observed)?;
        validate_authorized_identity(authorized, &observed).map_err(|_| RelayExecError::Profile)?;
        validate_configuration(authorized, &facts.bytes, self.binding.kind)
            .map_err(|_| RelayExecError::Profile)?;
        validate_helper_environment(&facts.bytes)?;
        authorized
            .validate_secret_env_file()
            .map_err(|_| RelayExecError::Profile)
    }
    /// No public/raw exec port is supplied.
    /// Inspection races remain possible: the engine owns process creation, so
    /// a successful CLI spawn is not a ready handshake or a workload effect.
    pub(crate) async fn open_relay_channel(
        &self,
        permit: RelayExecPermit,
    ) -> std::result::Result<RelayExecTransport, RelayExecError> {
        self.validate_handle(&permit.snapshot.handle)
            .map_err(|_| RelayExecError::Identity)?;
        if self.binding.fingerprint() != permit.authority
            || self.runtime.invocation().fingerprint() != permit.invocation
            || self.relay_profile()? != permit.profile
            || permit.snapshot.handle.session != permit.authorized.session
            || permit.snapshot.handle.operation != permit.authorized.operation_id()
            || permit.snapshot.handle.fingerprint_version()
                != permit.authorized.fingerprint_version()
            || !matches!(permit.authorized.purpose, ContainerPurpose::Run)
            || permit.authorized.network != NetworkMode::None
        {
            return Err(RelayExecError::Profile);
        }
        validate_running_snapshot(&permit.snapshot, &permit.snapshot.observation)?;
        permit
            ._admission
            .authority
            .current_binding()
            .await
            .map_err(|_| RelayExecError::Authority)?;
        // Re-read raw facts only transiently; no environment or raw inspection
        // bytes are copied into errors, observations or the transport owner.
        let facts = self
            .inspect_facts(&permit.snapshot.handle.id)
            .await
            .map_err(|_| RelayExecError::Authority)?;
        let observed = parse_container_observation_version(
            &facts.bytes,
            self.binding.kind,
            permit.snapshot.handle.fingerprint_version(),
        )
        .map_err(|_| RelayExecError::Identity)?;
        validate_running_snapshot(&permit.snapshot, &observed)?;
        validate_authorized_identity(&permit.authorized, &observed)
            .map_err(|_| RelayExecError::Profile)?;
        validate_configuration(&permit.authorized, &facts.bytes, self.binding.kind)
            .map_err(|_| RelayExecError::Profile)?;
        permit
            .authorized
            .validate_secret_env_file()
            .map_err(|_| RelayExecError::Profile)?;
        validate_helper_environment(&facts.bytes)?;
        // This is not prearmed_command: relay has a separate capability and
        // never consumes or reconstructs a start claim. STOP is untouched.
        let (mut command, retained) = self.relay_command()?;
        command.args(relay_exec_args(
            self.binding.kind,
            &permit.snapshot.handle.id,
        )?);
        // inspect_facts verified engine authority immediately before the exact
        // inspection. No await follows that incarnation check before spawn.
        if self.relay_profile()? != permit.profile {
            return Err(RelayExecError::Profile);
        }
        self.runtime
            .anchor
            .verify(self.runtime.invocation(), self.binding.kind)
            .map_err(|_| RelayExecError::Authority)?;
        permit
            ._admission
            .authority
            .check_deadline()
            .map_err(|_| RelayExecError::Authority)?;
        let mut transport = RelayExecTransport::spawn(command, permit.snapshot, retained)?;
        transport._admission = Some(permit._admission);
        Ok(transport)
    }

    fn relay_profile(&self) -> std::result::Result<RelayExecProfile, RelayExecError> {
        if self.binding.invocation.as_ref() != Some(self.runtime.invocation())
            || self.runtime.invocation().policy_version != 2
        {
            return Err(RelayExecError::Authority);
        }
        match self.binding.kind {
            RuntimeKind::Docker => {
                self.binding
                    .validate_docker_launch_profile()
                    .map_err(|_| RelayExecError::Profile)?;
                let binding = self
                    .binding
                    .docker_launch_profile
                    .as_ref()
                    .ok_or(RelayExecError::Profile)?;
                let retained = self
                    .launch_profile
                    .as_ref()
                    .ok_or(RelayExecError::Profile)?;
                if retained.binding() != binding {
                    return Err(RelayExecError::Profile);
                }
                retained.revalidate().map_err(|_| RelayExecError::Profile)?;
                Ok(RelayExecProfile::Docker(binding.clone()))
            }
            RuntimeKind::Podman => {
                if self.launch_profile.is_some() || self.binding.docker_launch_profile.is_some() {
                    return Err(RelayExecError::Profile);
                }
                let binding = self
                    .binding
                    .podman
                    .as_ref()
                    .ok_or(RelayExecError::Profile)?;
                if binding.effective_uid != effective_uid()
                    || binding.selection_env != podman_selection_env(self.runtime.invocation())
                {
                    return Err(RelayExecError::Profile);
                }
                Ok(RelayExecProfile::Podman(binding.clone()))
            }
        }
    }

    fn relay_command(
        &self,
    ) -> std::result::Result<(Command, Option<RetainedDockerLaunchProfile>), RelayExecError> {
        self.relay_profile()?;
        let mut command = self.command().map_err(|_| RelayExecError::Authority)?;
        let retained = self.launch_profile.clone();
        if let Some(profile) = &retained {
            command.arg("--config").arg(profile.directory_path());
        }
        Ok((command, retained))
    }
}

/// Docker's non-TTY hijack does not install an escape proxy; Podman explicitly
/// disables its configurable detach sequence. Only literal-empty overrides of
/// fixed credential/config names are accepted. No secret value, shell, fd
/// passing, free command, TTY or detached exec is admitted.
fn relay_exec_args(
    kind: RuntimeKind,
    id: &str,
) -> std::result::Result<Vec<String>, RelayExecError> {
    if !valid_immutable_id(id) {
        return Err(RelayExecError::Identity);
    }
    let mut args = vec![
        "exec".into(),
        "--interactive".into(),
        "--tty=false".into(),
        "--privileged=false".into(),
    ];
    if kind == RuntimeKind::Podman {
        args.push("--detach-keys=".into());
    }
    args.extend(EMPTY_EXEC_ENV.iter().map(|key| format!("--env={key}=")));
    args.extend([
        "--user".into(),
        "1000:1000".into(),
        id.into(),
        HELPER.into(),
        "serve-v1".into(),
    ]);
    Ok(args)
}

fn validate_running_snapshot(
    owned: &OwnedContainerSnapshot,
    observed: &ContainerObservation,
) -> std::result::Result<(), RelayExecError> {
    let baseline = &owned.observation;
    let started = baseline
        .process
        .started_at
        .filter(|date| date.timestamp() > 0);
    if !valid_immutable_id(&owned.handle.id)
        || owned.handle.session.is_nil()
        || owned.handle.operation.is_nil()
        || baseline.id.as_deref() != Some(owned.handle.id.as_str())
        || baseline.name != owned.handle.name
        || baseline.fingerprint_version() != owned.handle.fingerprint_version()
        || !matches!(baseline.fingerprint_version(), 2 | 3)
        || !baseline
            .configuration_fingerprint
            .as_deref()
            .is_some_and(valid_immutable_id)
        || !baseline.image_id.as_deref().is_some_and(|image| {
            image
                .strip_prefix("sha256:")
                .is_some_and(valid_immutable_id)
        })
        || baseline.validate_identity_allocation(baseline).is_err()
        || observed.validate_identity_allocation(baseline).is_err()
        || baseline.process.running != Some(true)
        || baseline.process.status.as_deref() != Some("running")
        || baseline.process.finished_at.is_some()
        || started.is_none()
        || observed.process.started_at != started
        || observed.process.running != Some(true)
        || observed.process.status.as_deref() != Some("running")
        || observed.process.finished_at.is_some()
    {
        return Err(RelayExecError::Identity);
    }
    Ok(())
}

/// Exec inherits container Config.Env. A missing --env flag does not make it
/// clean. Recognized credential/config names are overridden with literal empty
/// values. Other entries must be fixed public image values or short version
/// strings; unknown keys, duplicates and opaque values remain closed.
fn validate_helper_environment(bytes: &[u8]) -> std::result::Result<(), RelayExecError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Environment {
        config: Values,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Values {
        env: Vec<String>,
    }
    const ALLOWED: &[&str] = &[
        "HOME=/home/agent",
        "RUSTUP_HOME=/usr/local/rustup",
        "CARGO_HOME=/home/agent/.cargo",
        "PATH=/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        "LANG=C.UTF-8",
        "TERM=xterm-256color",
        "TMPDIR=/tmp",
        "container=podman",
    ];
    let environment: Environment =
        serde_json::from_slice(bytes).map_err(|_| RelayExecError::Profile)?;
    let mut seen = std::collections::HashSet::new();
    if environment.config.env.len() > ALLOWED.len() + EMPTY_EXEC_ENV.len() + 2
        || environment.config.env.iter().any(|entry| {
            let Some((key, value)) = entry.split_once('=') else {
                return true;
            };
            if !seen.insert(key) {
                return true;
            }
            !ALLOWED.contains(&entry.as_str())
                && !EMPTY_EXEC_ENV.contains(&key)
                && !(key == "NODE_VERSION" && short_version(value, Some("24")))
                && !(key == "YARN_VERSION" && short_version(value, Some("1")))
        })
    {
        return Err(RelayExecError::Profile);
    }
    Ok(())
}
fn short_version(value: &str, major: Option<&str>) -> bool {
    if value.len() > 20 {
        return false;
    }
    let parts: Vec<_> = value.split('.').collect();
    parts.len() == 3
        && major.is_none_or(|major| parts[0] == major)
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.len() <= 5
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RelayExecState {
    Open,
    OutcomeUnknown,
    /// Local tasks joined and the exact worker independently observed terminal.
    /// This does not commit a terminal, free quota, or automatically reopen.
    Closed,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct RelayExecClosure {
    pub(crate) local_joined: bool,
    pub(crate) cli_exit_code: Option<i32>,
    pub(crate) cli_kill_requested: bool,
}
#[derive(Default)]
struct Health {
    closed: AtomicBool,
    reader: AtomicWaker,
    writer: AtomicWaker,
}
impl Health {
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.reader.wake();
        self.writer.wake();
    }
    fn check(&self) -> io::Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "relay exec transport closed",
            ))
        } else {
            Ok(())
        }
    }
}

/// A borrow cannot extract or outlive a child pipe. ControlOwner owns framing,
/// 5s absolute/2s idle deadlines and cancellation; no runtime task reads stdout
/// behind it. Dropping the serving future returns these borrows to this owner.
pub(crate) struct RelayExecReader<'a> {
    pipe: &'a mut ChildStdout,
    health: Arc<Health>,
}
pub(crate) struct RelayExecWriter<'a> {
    pipe: &'a mut Option<ChildStdin>,
    health: Arc<Health>,
}
impl AsyncRead for RelayExecReader<'_> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.health.reader.register(cx.waker());
        if let Err(error) = self.health.check() {
            return Poll::Ready(Err(error));
        }
        let before = buffer.filled().len();
        let result = Pin::new(&mut *self.pipe).poll_read(cx, buffer);
        if matches!(result, Poll::Ready(Err(_))) {
            self.health.close();
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "relay exec read failed",
            )));
        }
        // EOF is returned as EOF once. It never constitutes helper or worker
        // termination; the owner must subsequently close and join the child.
        if matches!(result, Poll::Ready(Ok(())))
            && buffer.filled().len() == before
            && buffer.remaining() != 0
        {
            self.health.close();
        }
        result
    }
}
impl AsyncWrite for RelayExecWriter<'_> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.health.writer.register(cx.waker());
        if let Err(error) = self.health.check() {
            return Poll::Ready(Err(error));
        }
        let Some(pipe) = self.pipe.as_mut() else {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "relay exec stdin closed",
            )));
        };
        match Pin::new(pipe).poll_write(cx, bytes) {
            Poll::Ready(Err(_)) => {
                self.health.close();
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "relay exec write failed",
                )))
            }
            other => other,
        }
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        self.health.writer.register(cx.waker());
        if let Err(error) = self.health.check() {
            return Poll::Ready(Err(error));
        }
        let Some(pipe) = self.pipe.as_mut() else {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "relay exec stdin closed",
            )));
        };
        match Pin::new(pipe).poll_flush(cx) {
            Poll::Ready(Err(_)) => {
                self.health.close();
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "relay exec flush failed",
                )))
            }
            other => other,
        }
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        // EOF remains available after an IO error so closing never requires a
        // healthy channel or any provider/start permission. ChildStdin's Unix
        // shutdown is a no-op: dropping the owner's actual pipe is necessary
        // to send EOF. Keep stdout open so the helper's final bytes can drain.
        drop(self.pipe.take());
        Poll::Ready(Ok(()))
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct StderrSummary {
    observed_bytes: usize,
    exceeded: bool,
    unknown_diagnostic: bool,
    io_failed: bool,
    diagnostic_kinds: u8,
}
async fn drain_stderr(mut input: ChildStderr, health: Arc<Health>) -> StderrSummary {
    let mut report = StderrSummary::default();
    let mut bytes = [0u8; 1024];
    let mut line = [0u8; 64];
    let mut used = 0;
    loop {
        let count = match input.read(&mut bytes).await {
            Ok(0) => break,
            Ok(count) => count,
            Err(_) => {
                report.io_failed = true;
                health.close();
                break;
            }
        };
        health.close(); // helper stderr is error-only; interrupt pending IO.
        if count > STDERR_LIMIT.saturating_sub(report.observed_bytes) {
            report.observed_bytes = STDERR_LIMIT;
            report.exceeded = true;
            break; // drop the stderr FD; never retain excess bytes.
        }
        report.observed_bytes += count;
        for byte in &bytes[..count] {
            if used == line.len() {
                report.unknown_diagnostic = true;
                used = 0;
            }
            line[used] = *byte;
            used += 1;
            if *byte == b'\n' {
                report.diagnostic_kinds |= match &line[..used] {
                    b"relay_helper:protocol\n" => 1,
                    b"relay_helper:timeout\n" => 2,
                    b"relay_helper:unavailable\n" => 4,
                    b"relay_helper:unsafe_filesystem\n" => 8,
                    b"relay_helper:capacity\n" => 16,
                    _ => {
                        report.unknown_diagnostic = true;
                        0
                    }
                };
                used = 0;
            }
        }
    }
    if used != 0 {
        report.unknown_diagnostic = true;
    }
    report
}

/// Registry-retained owner of the CLI, all three pipes, drainer and retained
/// engine configuration. It owns no worker terminal/quota/usage authority.
pub(crate) struct RelayExecTransport {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<JoinHandle<StderrSummary>>,
    stderr_summary: Option<StderrSummary>,
    health: Arc<Health>,
    snapshot: OwnedContainerSnapshot,
    _profile: Option<RetainedDockerLaunchProfile>,
    state: RelayExecState,
    cli_status: Option<ExitStatus>,
    cli_kill_requested: bool,
    _admission: Option<sealed::DurableRelayAdmission>,
}
impl std::fmt::Debug for RelayExecTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayExecTransport")
            .field("state", &self.state)
            .field("local_joined", &self.local_joined())
            .finish_non_exhaustive()
    }
}
impl RelayExecTransport {
    fn spawn(
        mut command: Command,
        snapshot: OwnedContainerSnapshot,
        profile: Option<RetainedDockerLaunchProfile>,
    ) -> std::result::Result<Self, RelayExecError> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|_| RelayExecError::Unavailable)?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let health = Arc::new(Health::default());
        // Even an impossible incomplete pipe setup is retained as unknown; no
        // spawned child is lost through a fallible early return.
        let complete = stdin.is_some() && stdout.is_some() && stderr_pipe.is_some();
        if !complete {
            health.close();
        }
        let stderr = stderr_pipe.map(|pipe| tokio::spawn(drain_stderr(pipe, Arc::clone(&health))));
        Ok(Self {
            child: Some(child),
            stdin,
            stdout,
            stderr,
            stderr_summary: None,
            health,
            snapshot,
            _profile: profile,
            state: if complete {
                RelayExecState::Open
            } else {
                RelayExecState::OutcomeUnknown
            },
            cli_status: None,
            cli_kill_requested: false,
            _admission: None,
        })
    }
    pub(crate) fn io_pair(
        &mut self,
    ) -> std::result::Result<(RelayExecReader<'_>, RelayExecWriter<'_>), RelayExecError> {
        if self.state != RelayExecState::Open || self.health.check().is_err() {
            self.state = RelayExecState::OutcomeUnknown;
            return Err(RelayExecError::Closed);
        }
        let input = self.stdout.as_mut().ok_or(RelayExecError::Closed)?;
        if self.stdin.is_none() {
            return Err(RelayExecError::Closed);
        }
        Ok((
            RelayExecReader {
                pipe: input,
                health: Arc::clone(&self.health),
            },
            RelayExecWriter {
                pipe: &mut self.stdin,
                health: Arc::clone(&self.health),
            },
        ))
    }
    pub(crate) fn state(&self) -> RelayExecState {
        if self.state == RelayExecState::Open && self.health.closed.load(Ordering::SeqCst) {
            RelayExecState::OutcomeUnknown
        } else {
            self.state
        }
    }
    fn local_joined(&self) -> bool {
        self.child.is_none() && self.stderr.is_none()
    }

    /// Close admission and stdin, then wait/kill only the CLI. One absolute 5s
    /// deadline covers local child and stderr join. Cancellation leaves their
    /// handles inside self for explicit retry; no destructor claims completion.
    pub(crate) async fn close_and_join(&mut self) -> RelayExecClosure {
        if self.state != RelayExecState::Closed {
            self.state = RelayExecState::OutcomeUnknown;
        }
        self.health.close();
        drop(self.stdin.take());
        drop(self.stdout.take());
        let deadline = Instant::now() + CLOSE_TIME;
        if let Some(child) = self.child.as_mut() {
            let first = timeout_at(Instant::now() + EOF_GRACE, child.wait()).await;
            let status = match first {
                Ok(Ok(status)) => Some(status),
                _ => {
                    self.cli_kill_requested = true;
                    let _ = child.start_kill();
                    match timeout_at(deadline, child.wait()).await {
                        Ok(Ok(status)) => Some(status),
                        _ => None,
                    }
                }
            };
            if let Some(status) = status {
                self.cli_status = Some(status);
                self.child.take();
            }
        }
        if let Some(task) = self.stderr.as_mut() {
            match timeout_at(deadline, &mut *task).await {
                Ok(Ok(report)) => {
                    self.stderr_summary = Some(report);
                    self.stderr.take();
                }
                Ok(Err(_)) => {
                    self.stderr_summary = Some(StderrSummary {
                        io_failed: true,
                        ..StderrSummary::default()
                    });
                    self.stderr.take();
                }
                Err(_) => {
                    task.abort();
                } // retain until a subsequent join confirms cancellation.
            }
        }
        RelayExecClosure {
            local_joined: self.local_joined(),
            cli_exit_code: self.cli_status.and_then(|status| status.code()),
            cli_kill_requested: self.cli_kill_requested,
        }
    }

    /// The CLI cannot prove its remote helper died. This conservative route
    /// needs the exact worker independently observed terminal, after local join.
    /// It observes only and does not stop/remove a worker, mutate audit, settle
    /// quota/usage or allow a new channel without a new registry admission.
    pub(crate) async fn reconcile_stopped(
        &mut self,
        runtime: &ManagedRuntime,
    ) -> std::result::Result<ContainerObservation, RelayExecError> {
        if !self.local_joined() || self.state() != RelayExecState::OutcomeUnknown {
            return Err(RelayExecError::NeedsReconciliation);
        }
        let observed = runtime
            .observe(&self.snapshot.handle)
            .await
            .map_err(|_| RelayExecError::Authority)?;
        observed
            .validate_terminal(runtime.binding.kind, &self.snapshot.observation)
            .map_err(|_| RelayExecError::Identity)?;
        if observed.process.started_at != self.snapshot.observation.process.started_at {
            return Err(RelayExecError::Identity);
        }
        self.state = RelayExecState::Closed;
        Ok(observed)
    }
}
impl Drop for RelayExecTransport {
    fn drop(&mut self) {
        self.health.close();
        drop(self.stdin.take());
        drop(self.stdout.take());
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
        if let Some(task) = self.stderr.as_ref() {
            task.abort();
        }
        // Drop is only a fail-closed fallback. It neither joins nor changes the
        // registry's durable unknown state, and cannot authorize replacement.
    }
}

#[cfg(test)]
mod tests;
