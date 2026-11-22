//! Restricted, separate stdio exec for the worker activity helper
//! (`/usr/local/bin/ai-guard-activity serve-v1`).
//!
//! This is an observation channel owned by the execution owner: it carries
//! provider-declared hook submissions from the worker and the broker's acks
//! back. It is not a workload identity, a start permit, a terminal or quota
//! witness, nor a permission decision. A missing, failed or ended channel is a
//! loss of coverage that the owner must record as such.
//!
//! Only [`ManagedRuntime::open_activity_channel`] constructs a transport, and
//! only for a running container whose owned snapshot and authorized spec
//! revalidate against a fresh inspection (the relay exec checks). The argv is
//! fixed: no shell, no TTY, no detached exec, literal-empty credential
//! overrides and the worker UID.
#![cfg_attr(not(test), allow(dead_code))]
use super::relay::{
    EMPTY_EXEC_ENV, RelayExecError, validate_helper_environment, validate_running_snapshot,
};
use super::*;
use crate::activity_channel::{
    helper::MAX_PENDING_HOOKS,
    wire::{
        AckV1, BrokerFrameV1, FrameReader, FrameWriter, HelperFrameV1, HelperSequence,
        PROTOCOL_VERSION, ReadyV1, StatusV1, WireError,
    },
};
use std::{collections::BTreeSet, process::ExitStatus};
use tokio::{
    io::AsyncReadExt,
    process::{Child, ChildStderr, ChildStdin, ChildStdout},
    task::JoinHandle,
    time::timeout_at,
};

const HELPER: &str = "/usr/local/bin/ai-guard-activity";
const STDERR_LIMIT: usize = 16 * 1024;
const CLOSE_TIME: Duration = Duration::from_secs(5);
const EOF_GRACE: Duration = Duration::from_secs(2);
/// Exec start plus the helper's `hello`.
const HANDSHAKE_TIME: Duration = Duration::from_secs(15);
/// Hook seqs read and not yet acked. The helper serves at most
/// `MAX_PENDING_HOOKS`; more outstanding seqs is a helper protocol violation.
const MAX_OUTSTANDING: usize = MAX_PENDING_HOOKS * 4;

/// Closed diagnostics never retain stderr, inspection JSON, frames or OS text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivityExecError {
    Identity,
    Profile,
    Authority,
    Unavailable,
    Closed,
    Protocol,
    Timeout,
}
impl std::fmt::Display for ActivityExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Identity => "activity exec incarnation or configuration is not current",
            Self::Profile => "activity exec profile is not approved or available",
            Self::Authority => "activity exec authority is unavailable or changed",
            Self::Unavailable => "activity exec transport is unavailable",
            Self::Closed => "activity exec transport is closed",
            Self::Protocol => "activity helper stream is incompatible",
            Self::Timeout => "activity helper did not answer in time",
        })
    }
}
impl std::error::Error for ActivityExecError {}
fn relay(error: RelayExecError) -> ActivityExecError {
    match error {
        RelayExecError::Identity => ActivityExecError::Identity,
        RelayExecError::Profile => ActivityExecError::Profile,
        RelayExecError::Authority => ActivityExecError::Authority,
        RelayExecError::Unavailable
        | RelayExecError::Closed
        | RelayExecError::NeedsReconciliation => ActivityExecError::Unavailable,
    }
}
fn wire(error: WireError) -> ActivityExecError {
    match error {
        WireError::Timeout => ActivityExecError::Timeout,
        WireError::Closed => ActivityExecError::Closed,
        WireError::Io => ActivityExecError::Unavailable,
        WireError::PartialFrame
        | WireError::FrameSize
        | WireError::Protocol
        | WireError::Capacity => ActivityExecError::Protocol,
    }
}

impl ManagedRuntime {
    /// Open the activity channel of one running, owned and authorized worker.
    ///
    /// Checks without engine IO first (owned handle, running snapshot, spec
    /// binding: same session/operation/fingerprint version and `Run` purpose,
    /// approved exec profile), then a fresh inspection must
    /// show the same running incarnation, authorized identity, complete
    /// configuration, closed inherited environment and private secret file.
    /// A successful spawn is not a handshake: call
    /// [`ActivityExecTransport::handshake`] next.
    pub(crate) async fn open_activity_channel(
        &self,
        snapshot: OwnedContainerSnapshot,
        authorized: &ContainerSpec,
    ) -> std::result::Result<ActivityExecTransport, ActivityExecError> {
        self.validate_handle(&snapshot.handle)
            .map_err(|_| ActivityExecError::Identity)?;
        validate_running_snapshot(&snapshot, &snapshot.observation).map_err(relay)?;
        bind_spec(&snapshot, authorized)?;
        let profile = self.relay_profile().map_err(relay)?;
        // Raw facts are read transiently; no environment or inspection byte is
        // copied into errors or the transport owner.
        let facts = self
            .inspect_facts(&snapshot.handle.id)
            .await
            .map_err(|_| ActivityExecError::Authority)?;
        let observed = parse_container_observation_version(
            &facts.bytes,
            self.binding.kind,
            snapshot.handle.fingerprint_version(),
        )
        .map_err(|_| ActivityExecError::Identity)?;
        validate_running_snapshot(&snapshot, &observed).map_err(relay)?;
        validate_authorized_identity(authorized, &observed)
            .map_err(|_| ActivityExecError::Profile)?;
        validate_configuration(authorized, &facts.bytes, self.binding.kind)
            .map_err(|_| ActivityExecError::Profile)?;
        validate_helper_environment(&facts.bytes).map_err(relay)?;
        authorized
            .validate_secret_env_file()
            .map_err(|_| ActivityExecError::Profile)?;
        let (mut command, retained) = self.relay_command().map_err(relay)?;
        command.args(activity_exec_args(self.binding.kind, &snapshot.handle.id)?);
        // inspect_facts verified engine authority immediately before the exact
        // inspection; no await follows this incarnation check before spawn.
        if self.relay_profile().map_err(relay)? != profile {
            return Err(ActivityExecError::Profile);
        }
        self.runtime
            .anchor
            .verify(self.runtime.invocation(), self.binding.kind)
            .map_err(|_| ActivityExecError::Authority)?;
        ActivityExecTransport::spawn(command, snapshot, retained)
    }
}

/// The owned snapshot must be the `Run` container of its authorized spec. The
/// worker's network mode (none, bridge, or egress-filtered) is not checked
/// here: the channel is an exec stdio transport that never uses that network,
/// and `validate_configuration` still compares the inspected network with
/// the spec.
fn bind_spec(
    snapshot: &OwnedContainerSnapshot,
    authorized: &ContainerSpec,
) -> std::result::Result<(), ActivityExecError> {
    if snapshot.handle.session != authorized.session
        || snapshot.handle.operation != authorized.operation_id()
        || snapshot.handle.fingerprint_version() != authorized.fingerprint_version()
        || !matches!(authorized.purpose, ContainerPurpose::Run)
    {
        return Err(ActivityExecError::Profile);
    }
    Ok(())
}

/// Docker's non-TTY exec installs no escape proxy; Podman explicitly disables
/// its detach sequence. Only literal-empty overrides of fixed credential and
/// configuration names are passed. No secret, shell, fd passing, free command,
/// TTY or detached exec is admitted.
fn activity_exec_args(
    kind: RuntimeKind,
    id: &str,
) -> std::result::Result<Vec<String>, ActivityExecError> {
    if !valid_immutable_id(id) {
        return Err(ActivityExecError::Identity);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivityExecState {
    /// Handshake pending or frames flowing.
    Open,
    /// The owner closed the helper's stdin; remaining frames may still drain.
    Draining,
    /// The helper ended its stdout. This proves neither helper nor worker exit.
    Ended,
    /// An IO, timeout or protocol error: the stream is unusable.
    Failed,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ActivityExecClosure {
    pub(crate) local_joined: bool,
    pub(crate) cli_exit_code: Option<i32>,
    pub(crate) cli_kill_requested: bool,
}

/// Bounded, opaque summary of the helper's stderr.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ActivityStderrSummary {
    pub(crate) observed_bytes: usize,
    pub(crate) exceeded: bool,
    pub(crate) unknown_diagnostic: bool,
    pub(crate) io_failed: bool,
    /// Bits: protocol 1, timeout 2, unavailable 4, unsafe_filesystem 8, capacity 16.
    pub(crate) diagnostic_kinds: u8,
}

async fn drain_stderr(mut input: ChildStderr) -> ActivityStderrSummary {
    let mut report = ActivityStderrSummary::default();
    let mut bytes = [0u8; 1024];
    let mut line = [0u8; 64];
    let mut used = 0;
    loop {
        let count = match input.read(&mut bytes).await {
            Ok(0) => break,
            Ok(count) => count,
            Err(_) => {
                report.io_failed = true;
                break;
            }
        };
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
                    b"activity:protocol\n" => 1,
                    b"activity:timeout\n" => 2,
                    b"activity:unavailable\n" => 4,
                    b"activity:unsafe_filesystem\n" => 8,
                    b"activity:capacity\n" => 16,
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

/// Owner of the exec CLI, its three pipes, the stderr drainer and the retained
/// engine configuration. Frames read are order-checked ([`HelperSequence`]);
/// an ack is written only for a hook seq read and not yet acked. It owns no
/// worker terminal, quota, usage or audit authority.
pub(crate) struct ActivityExecTransport {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<JoinHandle<ActivityStderrSummary>>,
    stderr_summary: Option<ActivityStderrSummary>,
    reader: FrameReader<HelperFrameV1>,
    writer: FrameWriter,
    sequence: HelperSequence,
    outstanding: BTreeSet<u64>,
    ready_sent: bool,
    snapshot: OwnedContainerSnapshot,
    _profile: Option<RetainedDockerLaunchProfile>,
    state: ActivityExecState,
    cli_status: Option<ExitStatus>,
    cli_kill_requested: bool,
}
impl std::fmt::Debug for ActivityExecTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActivityExecTransport")
            .field("state", &self.state)
            .field("local_joined", &self.local_joined())
            .finish_non_exhaustive()
    }
}
impl ActivityExecTransport {
    fn spawn(
        mut command: Command,
        snapshot: OwnedContainerSnapshot,
        profile: Option<RetainedDockerLaunchProfile>,
    ) -> std::result::Result<Self, ActivityExecError> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|_| ActivityExecError::Unavailable)?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        // An impossible incomplete pipe setup is retained as failed; no spawned
        // child is lost through a fallible early return.
        let complete = stdin.is_some() && stdout.is_some() && stderr_pipe.is_some();
        Ok(Self {
            child: Some(child),
            stdin,
            stdout,
            stderr: stderr_pipe.map(|pipe| tokio::spawn(drain_stderr(pipe))),
            stderr_summary: None,
            reader: FrameReader::new(),
            writer: FrameWriter::new(),
            sequence: HelperSequence::new(),
            outstanding: BTreeSet::new(),
            ready_sent: false,
            snapshot,
            _profile: profile,
            state: if complete {
                ActivityExecState::Open
            } else {
                ActivityExecState::Failed
            },
            cli_status: None,
            cli_kill_requested: false,
        })
    }

    fn fail<T>(&mut self, error: ActivityExecError) -> std::result::Result<T, ActivityExecError> {
        self.state = ActivityExecState::Failed;
        Err(error)
    }

    /// Read the helper's `hello` within 15 s of the call and answer `ready`.
    pub(crate) async fn handshake(&mut self) -> std::result::Result<(), ActivityExecError> {
        if self.state != ActivityExecState::Open || self.ready_sent {
            return Err(ActivityExecError::Closed);
        }
        let deadline = Instant::now() + HANDSHAKE_TIME;
        let Some(stdout) = self.stdout.as_mut() else {
            return self.fail(ActivityExecError::Closed);
        };
        let frame = match timeout_at(deadline, self.reader.read(stdout)).await {
            Ok(Ok(Some(frame))) => frame,
            Ok(Ok(None)) => return self.fail(ActivityExecError::Closed),
            Ok(Err(error)) => return self.fail(wire(error)),
            Err(_) => return self.fail(ActivityExecError::Timeout),
        };
        if !matches!(frame, HelperFrameV1::Hello(_)) || self.sequence.accept(&frame).is_err() {
            return self.fail(ActivityExecError::Protocol);
        }
        let ready = BrokerFrameV1::Ready(ReadyV1 {
            version: PROTOCOL_VERSION,
        });
        self.write(&ready).await?;
        self.ready_sent = true;
        Ok(())
    }

    /// Next `hook` or `loss` frame, or `None` once the helper ends its stdout.
    /// Cancellation-safe: dropping this future between or inside frames keeps
    /// the partial frame for the next call. The first byte of a frame has no
    /// deadline; a started frame has 5 s.
    pub(crate) async fn next_frame(
        &mut self,
    ) -> std::result::Result<Option<HelperFrameV1>, ActivityExecError> {
        if matches!(
            self.state,
            ActivityExecState::Ended | ActivityExecState::Failed
        ) || !self.ready_sent
        {
            return Err(ActivityExecError::Closed);
        }
        let Some(stdout) = self.stdout.as_mut() else {
            return self.fail(ActivityExecError::Closed);
        };
        let frame = match self.reader.read(stdout).await {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                self.state = ActivityExecState::Ended;
                return Ok(None);
            }
            Err(error) => return self.fail(wire(error)),
        };
        if self.sequence.accept(&frame).is_err() {
            return self.fail(ActivityExecError::Protocol);
        }
        if let HelperFrameV1::Hook(hook) = &frame {
            if self.outstanding.len() >= MAX_OUTSTANDING {
                return self.fail(ActivityExecError::Protocol);
            }
            self.outstanding.insert(hook.seq);
        }
        Ok(Some(frame))
    }

    /// Acknowledge one outstanding hook seq. `Recorded` must only follow the
    /// owner's durable recording of that submission.
    pub(crate) async fn acknowledge(
        &mut self,
        seq: u64,
        status: StatusV1,
    ) -> std::result::Result<(), ActivityExecError> {
        if self.state != ActivityExecState::Open || !self.outstanding.contains(&seq) {
            return Err(ActivityExecError::Closed);
        }
        self.write(&BrokerFrameV1::Ack(AckV1 { seq, status }))
            .await?;
        self.outstanding.remove(&seq);
        Ok(())
    }

    /// Hook seqs read and not yet acknowledged.
    pub(crate) fn outstanding(&self) -> impl Iterator<Item = u64> + '_ {
        self.outstanding.iter().copied()
    }

    async fn write(&mut self, frame: &BrokerFrameV1) -> std::result::Result<(), ActivityExecError> {
        let Some(stdin) = self.stdin.as_mut() else {
            return self.fail(ActivityExecError::Closed);
        };
        match self.writer.write(stdin, frame).await {
            Ok(()) => Ok(()),
            Err(error) => self.fail(wire(error)),
        }
    }

    /// Send EOF to the helper: it refuses waiting hooks, reports its final
    /// losses and ends. Keep reading [`Self::next_frame`] until `None`.
    pub(crate) fn close_input(&mut self) {
        drop(self.stdin.take());
        self.writer.close();
        if self.state == ActivityExecState::Open {
            self.state = ActivityExecState::Draining;
        }
    }

    pub(crate) fn state(&self) -> ActivityExecState {
        self.state
    }

    pub(crate) fn stderr_summary(&self) -> Option<ActivityStderrSummary> {
        self.stderr_summary
    }

    fn local_joined(&self) -> bool {
        self.child.is_none() && self.stderr.is_none()
    }

    /// The owned, revalidated snapshot this channel was opened for. Frames
    /// from this transport concern that worker incarnation only.
    pub(crate) fn snapshot(&self) -> &OwnedContainerSnapshot {
        &self.snapshot
    }

    /// Close both pipes, then wait/kill only the CLI. One absolute 5 s deadline
    /// covers the child and stderr join. Cancellation leaves their handles here
    /// for an explicit retry; no destructor claims completion.
    pub(crate) async fn close_and_join(&mut self) -> ActivityExecClosure {
        self.close_input();
        drop(self.stdout.take());
        let deadline = Instant::now() + CLOSE_TIME;
        if let Some(child) = self.child.as_mut() {
            let status = match timeout_at(Instant::now() + EOF_GRACE, child.wait()).await {
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
                    self.stderr_summary = Some(ActivityStderrSummary {
                        io_failed: true,
                        ..ActivityStderrSummary::default()
                    });
                    self.stderr.take();
                }
                Err(_) => task.abort(), // retained until a later join confirms it.
            }
        }
        ActivityExecClosure {
            local_joined: self.local_joined(),
            cli_exit_code: self.cli_status.and_then(|status| status.code()),
            cli_kill_requested: self.cli_kill_requested,
        }
    }
}
impl Drop for ActivityExecTransport {
    fn drop(&mut self) {
        drop(self.stdin.take());
        drop(self.stdout.take());
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
        if let Some(task) = self.stderr.as_ref() {
            task.abort();
        }
        // Drop is only a fail-closed fallback: it neither joins nor records.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity_channel::wire::{
        ClientV1, EventV1, HelloV1, HookV1, LossReasonV1, LossV1, SubmitV1, decode_body,
        encode_framed,
    };
    use crate::runtime::{ObservedAllocation, ProcessObservation};
    use serde_json::json;
    use std::io::{Read, Write};

    const QA_CASE: &str = "AI_GUARD_ACTIVITY_STDIO_QA_CASE";

    fn owned(authority: &str, running: bool) -> OwnedContainerSnapshot {
        let started = chrono::DateTime::parse_from_rfc3339("2026-11-21T10:00:00Z").unwrap();
        OwnedContainerSnapshot {
            handle: ContainerHandle {
                authority_fingerprint: authority.into(),
                session: Uuid::from_u128(1),
                operation: Uuid::from_u128(2),
                id: "a".repeat(64),
                name: "aig-activity-fixture".into(),
                configuration_fingerprint_version: Some(2),
            },
            observation: ContainerObservation {
                id: Some("a".repeat(64)),
                name: "aig-activity-fixture".into(),
                image_id: Some(format!("sha256:{}", "b".repeat(64))),
                restart_count: Some(0),
                process: ProcessObservation {
                    status: Some(if running { "running" } else { "exited" }.into()),
                    running: Some(running),
                    started_at: Some(started),
                    finished_at: None,
                    exit_code: None,
                    oom_killed: Some(false),
                },
                allocation: serde_json::from_value::<ObservedAllocation>(json!({
                    "cpu": {"quota":200000, "period":100000},
                    "memory_bytes": 1073741824_u64,
                    "pids_limit":256
                }))
                .unwrap(),
                configuration_fingerprint: Some("d".repeat(64)),
                configuration_fingerprint_version: Some(2),
                observed_at: chrono::Utc::now(),
                diagnostics: vec![],
            },
        }
    }

    /// A Docker authority pinned to this test executable for construction
    /// only. The refusals below happen before it could ever be invoked.
    fn managed() -> (tempfile::TempDir, ManagedRuntime) {
        use crate::control_fs::Directory;
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = Directory::open_existing(&temp.path().canonicalize().unwrap()).unwrap();
        let runtime =
            Runtime::with_program(RuntimeKind::Docker, &std::env::current_exe().unwrap()).unwrap();
        let binding = DockerLaunchProfileBindingV1::new(
            &"1".repeat(64),
            Uuid::from_u128(1),
            &"2".repeat(64),
            "unix:///not-opened/activity-docker.sock",
            &runtime.invocation().fingerprint(),
        )
        .unwrap();
        let retained = RetainedDockerLaunchProfile::create(&root, &binding).unwrap();
        let managed = ManagedRuntime {
            binding: RuntimeAuthority {
                authority_id: Uuid::from_u128(2),
                kind: RuntimeKind::Docker,
                endpoint: binding.runtime_endpoint.clone(),
                engine_id: Some("software-only-engine".into()),
                invocation: Some(runtime.invocation().clone()),
                docker_launch_profile: Some(binding.clone()),
                podman: None,
            },
            runtime,
            launch_profile: Some(retained),
        };
        (temp, managed)
    }

    #[test]
    fn docker_and_podman_activity_exec_arguments_are_fixed_and_use_the_exact_cid() {
        let cid = "a".repeat(64);
        let docker = activity_exec_args(RuntimeKind::Docker, &cid).unwrap();
        let expected = vec![
            "exec",
            "--interactive",
            "--tty=false",
            "--privileged=false",
            "--env=AI_GUARD_PROVIDER_BOOTSTRAP=",
            "--env=OPENAI_API_KEY=",
            "--env=CODEX_API_KEY=",
            "--env=ANTHROPIC_API_KEY=",
            "--env=CLAUDE_CODE_OAUTH_TOKEN=",
            "--env=CODEX_HOME=",
            "--env=CLAUDE_CONFIG_DIR=",
            "--user",
            "1000:1000",
            cid.as_str(),
            "/usr/local/bin/ai-guard-activity",
            "serve-v1",
        ];
        assert_eq!(docker, expected);
        let podman = activity_exec_args(RuntimeKind::Podman, &cid).unwrap();
        let mut podman_expected = expected.clone();
        podman_expected.insert(4, "--detach-keys=");
        assert_eq!(podman, podman_expected);
        for invalid in [
            "",
            "worker",
            "--privileged",
            &"a".repeat(63),
            &"g".repeat(64),
        ] {
            assert_eq!(
                activity_exec_args(RuntimeKind::Docker, invalid),
                Err(ActivityExecError::Identity)
            );
        }
    }

    #[tokio::test]
    async fn opening_refuses_foreign_stopped_or_unbound_snapshots_before_engine_io() {
        let (_temp, managed) = managed();
        let authority = managed.binding.fingerprint();
        let snapshot = owned(&authority, true);
        let authorized = ContainerSpec::with_operation(
            snapshot.handle.session,
            ContainerPurpose::Run,
            "worker:fixture",
            vec!["/usr/bin/id".into()],
            NetworkMode::None,
            snapshot.handle.operation,
        )
        .unwrap();
        // Identity failures: foreign authority, then a stopped incarnation.
        // An engine call would have failed as Authority instead.
        for refused in [owned(&"c".repeat(64), true), owned(&authority, false)] {
            assert_eq!(
                managed
                    .open_activity_channel(refused, &authorized)
                    .await
                    .unwrap_err(),
                ActivityExecError::Identity
            );
        }
        let mut stopped = owned(&authority, true);
        stopped.observation.process.finished_at = Some(chrono::Utc::now().into());
        assert_eq!(
            managed
                .open_activity_channel(stopped, &authorized)
                .await
                .unwrap_err(),
            ActivityExecError::Identity
        );
        // Profile failures: another session, another purpose.
        let other_session = ContainerSpec::with_operation(
            Uuid::from_u128(9),
            ContainerPurpose::Run,
            "worker:fixture",
            vec!["/usr/bin/id".into()],
            NetworkMode::None,
            snapshot.handle.operation,
        )
        .unwrap();
        let import = ContainerSpec::with_operation(
            snapshot.handle.session,
            ContainerPurpose::Import,
            "worker:fixture",
            vec!["/usr/bin/id".into()],
            NetworkMode::None,
            snapshot.handle.operation,
        )
        .unwrap();
        for spec in [other_session, import] {
            assert_eq!(
                managed
                    .open_activity_channel(snapshot.clone(), &spec)
                    .await
                    .unwrap_err(),
                ActivityExecError::Profile
            );
        }
    }

    #[test]
    fn a_run_spec_binds_on_any_network_while_other_purposes_or_identities_do_not() {
        let mut snapshot = owned(&"c".repeat(64), true);
        let spec = |purpose, network, session, operation| {
            ContainerSpec::with_operation(
                session,
                purpose,
                "worker:fixture",
                vec!["/usr/local/bin/ai-guard-provider".into()],
                network,
                operation,
            )
            .unwrap()
        };
        let (session, operation) = (snapshot.handle.session, snapshot.handle.operation);
        // Provider sessions run on bridge (egress-filtered later): admitted.
        for network in [NetworkMode::Bridge, NetworkMode::None] {
            let authorized = spec(ContainerPurpose::Run, network, session, operation);
            snapshot.handle.configuration_fingerprint_version =
                Some(authorized.fingerprint_version());
            assert_eq!(bind_spec(&snapshot, &authorized), Ok(()));
        }
        let bridged = spec(
            ContainerPurpose::Run,
            NetworkMode::Bridge,
            session,
            operation,
        );
        for refused in [
            spec(
                ContainerPurpose::Import,
                NetworkMode::Bridge,
                session,
                operation,
            ),
            spec(
                ContainerPurpose::Export,
                NetworkMode::None,
                session,
                operation,
            ),
            spec(
                ContainerPurpose::Run,
                NetworkMode::Bridge,
                Uuid::from_u128(9),
                operation,
            ),
            spec(
                ContainerPurpose::Run,
                NetworkMode::Bridge,
                session,
                Uuid::from_u128(9),
            ),
        ] {
            assert_eq!(
                bind_spec(&snapshot, &refused),
                Err(ActivityExecError::Profile)
            );
        }
        snapshot.handle.configuration_fingerprint_version = Some(bridged.fingerprint_version() + 1);
        assert_eq!(
            bind_spec(&snapshot, &bridged),
            Err(ActivityExecError::Profile)
        );
        // The network never reaches the exec argv (see the golden test above).
        let argv = activity_exec_args(RuntimeKind::Podman, &"a".repeat(64)).unwrap();
        assert!(
            !argv
                .iter()
                .any(|arg| arg.contains("network") || arg == "bridge")
        );
    }

    fn qa_command(case: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "runtime::managed::activity::tests::stdio_child",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_clear()
            .env(QA_CASE, case);
        command
    }

    /// libtest writes its own prefix before the child function runs.
    async fn skip_to_marker(transport: &mut ActivityExecTransport) {
        let stdout = transport.stdout.as_mut().unwrap();
        tokio::time::timeout(Duration::from_secs(20), async {
            let mut seen = Vec::new();
            while !seen.ends_with(b"QA_READY\n") {
                assert!(seen.len() < 4096);
                let mut byte = [0u8; 1];
                stdout.read_exact(&mut byte).await.unwrap();
                seen.push(byte[0]);
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn the_transport_handshakes_checks_order_and_acks_only_outstanding_hooks() {
        let mut transport =
            ActivityExecTransport::spawn(qa_command("helper"), owned(&"c".repeat(64), true), None)
                .unwrap();
        assert_eq!(
            transport.acknowledge(1, StatusV1::Recorded).await,
            Err(ActivityExecError::Closed)
        );
        assert_eq!(
            transport.next_frame().await,
            Err(ActivityExecError::Closed),
            "no frame before the handshake"
        );
        assert_eq!(transport.snapshot().handle.id, "a".repeat(64));
        skip_to_marker(&mut transport).await;
        transport.handshake().await.unwrap();
        let Some(HelperFrameV1::Hook(hook)) = transport.next_frame().await.unwrap() else {
            panic!("expected the first hook");
        };
        assert_eq!((hook.seq, hook.submit.event), (1, EventV1::PreToolUse));
        assert_eq!(transport.outstanding().collect::<Vec<_>>(), [1]);
        assert_eq!(
            transport.acknowledge(2, StatusV1::Recorded).await,
            Err(ActivityExecError::Closed)
        );
        transport.acknowledge(1, StatusV1::Recorded).await.unwrap();
        assert_eq!(
            transport.acknowledge(1, StatusV1::Recorded).await,
            Err(ActivityExecError::Closed)
        );
        assert_eq!(
            transport.next_frame().await.unwrap(),
            Some(HelperFrameV1::Loss(LossV1 {
                seq: 2,
                count: 4,
                reason: LossReasonV1::ClientUnrecorded
            }))
        );
        transport.close_input();
        assert_eq!(transport.state(), ActivityExecState::Draining);
        assert_eq!(transport.next_frame().await, Ok(None));
        assert_eq!(transport.state(), ActivityExecState::Ended);
        let closure = transport.close_and_join().await;
        assert!(closure.local_joined && !closure.cli_kill_requested);
        assert_eq!(closure.cli_exit_code, Some(0));
        let stderr = transport.stderr_summary().unwrap();
        assert_eq!(stderr.diagnostic_kinds, 2);
        assert_eq!(stderr.observed_bytes, b"activity:timeout\n".len());
        assert!(!stderr.unknown_diagnostic && !stderr.exceeded && !stderr.io_failed);
    }

    #[tokio::test]
    async fn a_sequence_gap_fails_the_stream() {
        let mut transport =
            ActivityExecTransport::spawn(qa_command("gap"), owned(&"c".repeat(64), true), None)
                .unwrap();
        skip_to_marker(&mut transport).await;
        transport.handshake().await.unwrap();
        assert_eq!(
            transport.next_frame().await,
            Err(ActivityExecError::Protocol)
        );
        assert_eq!(transport.state(), ActivityExecState::Failed);
        assert_eq!(transport.next_frame().await, Err(ActivityExecError::Closed));
        let closure = transport.close_and_join().await;
        assert!(closure.local_joined);
    }

    fn write_frame_sync<F: crate::activity_channel::wire::Frame>(frame: &F) {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(&encode_framed(frame).unwrap()).unwrap();
        stdout.flush().unwrap();
    }

    fn read_frame_sync() -> BrokerFrameV1 {
        let mut stdin = std::io::stdin().lock();
        let mut prefix = [0u8; 4];
        stdin.read_exact(&mut prefix).unwrap();
        let mut body = vec![0u8; u32::from_be_bytes(prefix) as usize];
        stdin.read_exact(&mut body).unwrap();
        decode_body(&body).unwrap()
    }

    /// Internal fake helper of the transport tests; not a runtime qualification.
    #[test]
    #[ignore = "internal child process of the activity transport tests"]
    fn stdio_child() {
        let case = std::env::var(QA_CASE).unwrap();
        {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(b"QA_READY\n").unwrap();
            stdout.flush().unwrap();
        }
        write_frame_sync(&HelperFrameV1::Hello(HelloV1 { version: 1 }));
        assert!(matches!(read_frame_sync(), BrokerFrameV1::Ready(_)));
        let submit = SubmitV1 {
            version: 1,
            client: ClientV1::ClaudeCode,
            event: EventV1::PreToolUse,
            gate: true,
            stdin_total_bytes: 2,
            stdin_truncated: false,
            payload_invalid_utf8: false,
            payload: "{}".into(),
        };
        let first = if case == "gap" { 2 } else { 1 };
        write_frame_sync(&HelperFrameV1::Hook(HookV1 { seq: first, submit }));
        if case == "helper" {
            assert_eq!(
                read_frame_sync(),
                BrokerFrameV1::Ack(AckV1 {
                    seq: 1,
                    status: StatusV1::Recorded
                })
            );
            write_frame_sync(&HelperFrameV1::Loss(LossV1 {
                seq: 2,
                count: 4,
                reason: LossReasonV1::ClientUnrecorded,
            }));
        }
        let mut rest = Vec::new();
        let _ = std::io::stdin().lock().read_to_end(&mut rest);
        let _ = std::io::stderr().write_all(b"activity:timeout\n");
        std::process::exit(0);
    }
}
