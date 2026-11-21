use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{future::Future, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::{Instant, timeout},
};
use uuid::Uuid;
mod batch_io;
mod control_outcome;
pub(crate) use control_outcome::{
    NativeCollected, NativeCollectionOutcome, NativeIoPhase, NativeNoChild, NativeRuntimeOutcome,
    NativeStopFailure, NativeStopResult, NativeUncollectedCall, OwnedStopObservation,
};
#[cfg(test)]
mod deadline_driver_tests;
#[cfg(test)]
mod deadline_notification_tests;
mod deadline_supervision;
#[cfg(test)]
mod deadline_supervision_tests;
mod invocation;
pub mod kubernetes;
pub(crate) mod launch_profile;
mod managed;
pub mod observation;
pub(crate) use managed::relay::{RelayExecError, RelayExecPermit, RelayExecTransport};
pub(crate) use managed::selected_podman_target_args;
pub(crate) mod model_tls;
mod prestart;
pub mod retained_cli;
pub mod terminal;
#[cfg(unix)]
pub(crate) use invocation::control_service_account;
pub use invocation::{ExecutableDirectoryIdentityV1, ExecutableIdentityV1, InvocationBindingV1};
pub use managed::{
    ContainerHandle, ManagedPrearmedExecution, ManagedPrearmedOutcome, ManagedRetainedExecution,
    ManagedRuntime, ManagedRuntimeError, OwnedContainerSnapshot, RuntimeAuthority, StartDirective,
};
pub(crate) use prestart::{RuntimePrestartStage, RuntimePrestartStageFailure};

/// Last synchronous check by the trusted broker, after all awaited preparation.
/// This consumes the permit; it does not make process spawn atomic with the engine.
pub trait BeforeSpawnGuard: Send {
    fn check_before_spawn(self: Box<Self>, target: &str) -> Result<()>;
}

/// Additional legacy control-owner veto; this grants no start or enforcer right.
/// Only the trusted runner can construct it from its real observer/claim owner.
pub struct LegacyStartVeto {
    guard: Box<dyn BeforeSpawnGuard>,
}
impl LegacyStartVeto {
    pub(crate) fn new(guard: Box<dyn BeforeSpawnGuard>) -> Self {
        Self { guard }
    }
    fn check(self, target: &str) -> Result<()> {
        self.guard.check_before_spawn(target)
    }
}
impl std::fmt::Debug for LegacyStartVeto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LegacyStartVeto([private consumed restriction])")
    }
}

/// A fresh start claim and the stop notification of an independently armed enforcer.
/// Neither this notification nor the transport establishes a worker's terminal state.
pub struct PrearmedStartControl {
    guard: Box<dyn BeforeSpawnGuard>,
    stop: tokio::sync::watch::Receiver<bool>,
    launch_profile: Option<Box<launch_profile::DockerLaunchProfileBindingV1>>,
}
impl PrearmedStartControl {
    pub fn new(guard: Box<dyn BeforeSpawnGuard>, stop: tokio::sync::watch::Receiver<bool>) -> Self {
        Self {
            guard,
            stop,
            launch_profile: None,
        }
    }
    pub(crate) fn with_launch_profile(
        guard: Box<dyn BeforeSpawnGuard>,
        stop: tokio::sync::watch::Receiver<bool>,
        launch_profile: Option<launch_profile::DockerLaunchProfileBindingV1>,
    ) -> Self {
        Self {
            guard,
            stop,
            launch_profile: launch_profile.map(Box::new),
        }
    }
}
struct ProfileBoundSpawnGuard {
    inner: Box<dyn BeforeSpawnGuard>,
    expected: Option<Box<launch_profile::DockerLaunchProfileBindingV1>>,
    prepared: Option<launch_profile::DockerLaunchProfileBindingV1>,
}
impl BeforeSpawnGuard for ProfileBoundSpawnGuard {
    fn check_before_spawn(self: Box<Self>, target: &str) -> Result<()> {
        ensure!(
            self.expected.as_deref() == self.prepared.as_ref(),
            "prepared Docker start profile differs from the armed control"
        );
        self.inner.check_before_spawn(target)
    }
}
impl std::fmt::Debug for PrearmedStartControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrearmedStartControl([private consumed permit])")
    }
}

/// Process-local result returned only after the broker confirms the real Final
/// retirement checkpoint. Its identifiers and reference are data; they do not
/// establish runtime terminal state, quota release or permission to clean up.
/// It is deliberately neither Clone nor serializable/deserializable.
pub struct AbandonedBeforeClaim {
    operation_id: Uuid,
    arm_id: Uuid,
    final_revision: crate::broker::journal::RevisionRefV1,
}

impl AbandonedBeforeClaim {
    /// The trusted broker calls this only after verifying Final for this exact
    /// operation/Arm. A valid reference alone never establishes that Final.
    pub(crate) fn new(
        operation_id: Uuid,
        arm_id: Uuid,
        final_revision: crate::broker::journal::RevisionRefV1,
    ) -> Result<Self> {
        ensure!(
            !operation_id.is_nil() && !arm_id.is_nil() && final_revision.revision() > 0,
            "invalid abandoned-before-claim result identity or final revision"
        );
        let final_revision = crate::broker::journal::RevisionRefV1::new(
            final_revision.revision(),
            final_revision.hash(),
        )?;
        Ok(Self {
            operation_id,
            arm_id,
            final_revision,
        })
    }

    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }
    pub fn arm_id(&self) -> Uuid {
        self.arm_id
    }
    pub fn final_revision(&self) -> &crate::broker::journal::RevisionRefV1 {
        &self.final_revision
    }
}

impl std::fmt::Debug for AbandonedBeforeClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AbandonedBeforeClaim([private confirmed result])")
    }
}

#[derive(Debug)]
pub enum PrearmedStartDirective {
    ObserveOnly,
    EmitOnce(PrearmedStartControl),
    AbandonedBeforeClaim(AbandonedBeforeClaim),
}

/// Captured transport facts. A daemon transport has no locally reset business
/// deadline, so it cannot manufacture a `timed_out` worker result.
pub struct PrearmedExecutionResult {
    pub stdout: Capture,
    pub stderr: Capture,
    pub transport_exit_code: Option<i32>,
    pub duration_ms: u128,
}
impl std::fmt::Debug for PrearmedExecutionResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrearmedExecutionResult([private transport capture])")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeKind {
    Docker,
    Podman,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMode {
    None,
    Bridge,
}
#[derive(Clone, Copy, Debug)]
pub enum ContainerPurpose {
    Import,
    Run,
    Export,
}
#[derive(Clone, Debug)]
pub struct ContainerSpec {
    pub session: Uuid,
    pub purpose: ContainerPurpose,
    pub image: String,
    pub argv: Vec<String>,
    pub network: NetworkMode,
    operation: Uuid,
    secret_env_file: Option<crate::control_fs::PrivateFile>,
    managed_names: Option<(String, String)>,
    security_preset: crate::control::SecurityPresetV1,
    terminal_secrets: TerminalSecrets,
}
#[derive(Clone, Default)]
struct TerminalSecrets(Vec<String>);
impl std::fmt::Debug for TerminalSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[private terminal masking dictionary]")
    }
}
impl ContainerSpec {
    pub fn new(
        session: Uuid,
        purpose: ContainerPurpose,
        image: &str,
        argv: Vec<String>,
        network: NetworkMode,
    ) -> Result<Self> {
        Self::with_operation(session, purpose, image, argv, network, Uuid::new_v4())
    }
    /// Reconstruct a persisted attempt without generating a new creation identity.
    pub fn with_operation(
        session: Uuid,
        purpose: ContainerPurpose,
        image: &str,
        argv: Vec<String>,
        network: NetworkMode,
        operation: Uuid,
    ) -> Result<Self> {
        validate_image(image)?;
        validate_command(&argv)?;
        ensure!(
            !operation.is_nil(),
            "runtime operation identity must not be nil"
        );
        Ok(Self {
            session,
            purpose,
            image: image.into(),
            argv,
            network,
            operation,
            secret_env_file: None,
            managed_names: None,
            security_preset: crate::control::SecurityPresetV1::FixedSandboxV2,
            terminal_secrets: TerminalSecrets::default(),
        })
    }
    pub fn operation_id(&self) -> Uuid {
        self.operation
    }
    /// Select an already admitted preset when reconstructing a historical
    /// instance. Legacy presets may be inspected, but cannot create or start.
    pub fn with_security_preset(mut self, preset: crate::control::SecurityPresetV1) -> Self {
        self.security_preset = preset;
        self
    }
    pub fn with_terminal_secrets(mut self, secrets: Vec<String>) -> Result<Self> {
        ensure!(
            self.security_preset.is_interactive() && matches!(self.purpose, ContainerPurpose::Run),
            "terminal masking is valid only for an interactive worker"
        );
        // Validate its strict memory bound before any create/start effect.
        let _ = terminal::DisplayFilter::new(secrets.clone())?;
        self.terminal_secrets = TerminalSecrets(secrets);
        Ok(self)
    }
    fn fingerprint_version(&self) -> u32 {
        self.security_preset.configuration_fingerprint_version()
    }
    fn require_current_creation_policy(&self) -> Result<()> {
        ensure!(
            self.security_preset.requires_no_engine_logs(),
            "legacy fixed_sandbox_v1 cannot create or first-start a worker; create a fresh audited request with fixed_sandbox_v2; existing reservation and evidence were retained"
        );
        Ok(())
    }
    /// Names must come from authenticated control, not a tenant's project or
    /// session identifier. Only named volumes can be selected, never host paths.
    pub fn with_managed_names(mut self, name: &str, volume: &str) -> Result<Self> {
        validate_managed_name(name)?;
        validate_managed_name(volume)?;
        self.managed_names = Some((name.into(), volume.into()));
        Ok(self)
    }
    pub fn with_secret_env_file(self, path: &std::path::Path) -> Result<Self> {
        ensure!(
            path.is_absolute(),
            "credential injection requires an absolute env file"
        );
        let path = crate::control_fs::absolute_path(path)?;
        self.with_secret_env_handle(crate::control_fs::PrivateFile::open(&path, 64 * 1024)?)
    }
    pub fn with_secret_env_handle(mut self, file: crate::control_fs::PrivateFile) -> Result<Self> {
        ensure!(
            file.bytes().len() <= 64 * 1024,
            "credential env file must be no larger than 64 KiB"
        );
        if !matches!(self.purpose, ContainerPurpose::Run) || self.network != NetworkMode::Bridge {
            bail!(
                "credential injection is valid only for a network-enabled worker and a regular absolute env file"
            );
        }
        file.revalidate()?;
        self.secret_env_file = Some(file);
        Ok(self)
    }
    fn validate_secret_env_file(&self) -> Result<()> {
        if let Some(file) = &self.secret_env_file {
            file.revalidate()?;
        }
        Ok(())
    }
    pub fn name(&self) -> String {
        if let Some((name, _)) = &self.managed_names {
            return name.clone();
        }
        format!(
            "agentsandbox-{}-{}-{}",
            self.session,
            match self.purpose {
                ContainerPurpose::Import => "import",
                ContainerPurpose::Run => "run",
                ContainerPurpose::Export => "export",
            },
            self.operation.simple()
        )
    }
    pub fn volume(&self) -> String {
        self.managed_names
            .as_ref()
            .map(|(_, volume)| volume.clone())
            .unwrap_or_else(|| volume_name(self.session))
    }
    pub fn create_args(&self, runtime: RuntimeKind) -> Result<Vec<String>> {
        self.require_current_creation_policy()?;
        self.validate_secret_env_file()?;
        validate_image(&self.image)?;
        validate_command(&self.argv)?;
        ensure!(
            !self.security_preset.is_interactive() || matches!(self.purpose, ContainerPurpose::Run),
            "the interactive preset is reserved for worker execution"
        );
        let (home_mount_flag, home_mount) = match runtime {
            RuntimeKind::Docker => (
                "--tmpfs",
                "/home/agent:rw,nosuid,nodev,size=64m,uid=1000,gid=1000,mode=700",
            ),
            RuntimeKind::Podman => (
                "--mount",
                "type=tmpfs,destination=/home/agent,tmpfs-size=64m,tmpfs-mode=0700,notmpcopyup,U=true,nosuid,nodev",
            ),
        };
        let mut args = vec![
            "create".into(),
            "--pull=never".into(),
            "--name".into(),
            self.name(),
            "--label".into(),
            format!("agentsandbox.session={}", self.session),
            "--label".into(),
            format!(
                "agentsandbox.purpose={}",
                match self.purpose {
                    ContainerPurpose::Import => "import",
                    ContainerPurpose::Run => "run",
                    ContainerPurpose::Export => "export",
                }
            ),
            "--label".into(),
            format!("agentsandbox.operation={}", self.operation),
            "--read-only".into(),
            "--log-driver".into(),
            "none".into(),
            "--cap-drop".into(),
            "ALL".into(),
            "--security-opt".into(),
            "no-new-privileges".into(),
            "--user".into(),
            "1000:1000".into(),
            "--network".into(),
            match self.network {
                NetworkMode::None => "none",
                NetworkMode::Bridge => "bridge",
            }
            .into(),
            "--pids-limit".into(),
            "256".into(),
            "--memory".into(),
            "1g".into(),
            "--cpus".into(),
            "2".into(),
            "--workdir".into(),
            "/workspace".into(),
            "--tmpfs".into(),
            "/tmp:rw,nosuid,nodev,noexec,size=256m,mode=1777".into(),
            home_mount_flag.into(),
            home_mount.into(),
            "--env".into(),
            "HOME=/home/agent".into(),
            "--env".into(),
            "LANG=C.UTF-8".into(),
            "--mount".into(),
            format!("type=volume,source={},target=/workspace", self.volume()),
            "--entrypoint".into(),
            self.argv[0].clone(),
        ];
        if matches!(self.purpose, ContainerPurpose::Import) {
            args.push("--interactive".into());
        }
        if self.security_preset.is_interactive() {
            args.extend([
                "--interactive".into(),
                "--tty".into(),
                "--env".into(),
                format!("TERM={}", terminal::TERM),
            ]);
        }
        if runtime == RuntimeKind::Podman {
            args.extend(["--userns".into(), "keep-id:uid=1000,gid=1000".into()]);
        }
        if let Some(file) = &self.secret_env_file {
            args.extend([
                "--env-file".into(),
                file.path()
                    .to_str()
                    .context("credential path must be UTF-8")?
                    .into(),
            ]);
        }
        args.extend(["--".into(), self.image.clone()]);
        args.extend(self.argv.iter().skip(1).cloned());
        Ok(args)
    }
}
fn validate_managed_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 128
            && name.as_bytes()[0].is_ascii_alphanumeric()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "managed container and volume names must contain only letters, digits, underscore or hyphen and begin with a letter or digit"
    );
    Ok(())
}
pub fn validate_image(image: &str) -> Result<()> {
    if image.is_empty()
        || image.starts_with('-')
        || image.len() > 256
        || !image
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-:@".contains(&b))
    {
        bail!("invalid worker image reference");
    }
    Ok(())
}
pub fn validate_command(argv: &[String]) -> Result<()> {
    if argv.is_empty()
        || argv.len() > 256
        || argv.iter().map(|a| a.len()).sum::<usize>() > 64 * 1024
        || argv.iter().any(|a| a.contains('\0'))
        || serde_json::to_vec(argv)?.len() > 96 * 1024
    {
        bail!("command is empty, too large, or contains NUL");
    }
    let program = &argv[0];
    if program.is_empty()
        || program.starts_with('-')
        || program.chars().any(char::is_control)
        || program.contains('\\')
        || program.split('/').any(|p| p == "..")
    {
        bail!("invalid worker executable path");
    }
    Ok(())
}
pub fn volume_name(session: Uuid) -> String {
    format!("agentsandbox-work-{session}")
}
impl RuntimeKind {
    pub fn program(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }
}
impl std::fmt::Display for RuntimeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.program())
    }
}
impl std::fmt::Display for NetworkMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "none",
            Self::Bridge => "bridge",
        })
    }
}

#[derive(Debug)]
pub struct Capture {
    pub bytes: Vec<u8>,
    pub total_bytes: u64,
    pub truncated: bool,
}

/// Failed engine invocation. Generic formatting deliberately excludes stderr;
/// callers with a secret dictionary may redact the bounded capture explicitly.
pub struct RuntimeCommandFailure {
    runtime: RuntimeKind,
    status: std::process::ExitStatus,
    stderr: Capture,
    private_redactions: Vec<String>,
}
/// Private context, never formats stdout or captured bytes. Stderr for a
/// nonzero exit remains in its original RuntimeCommandFailure source.
struct CliEvidence(batch_io::BatchIoCollected);
impl std::fmt::Display for CliEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("control invocation collected; bounded diagnostics retained")
    }
}
impl std::fmt::Debug for CliEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CliEvidence")
            .field("stdout_bytes", &self.0.stdout.total_bytes)
            .field("stderr_moved_to_source", &true)
            .finish()
    }
}
impl RuntimeCommandFailure {
    /// Raw control evidence, including its truncation boundary. Never log this
    /// capture directly; redact it before decoding and formatting diagnostics.
    pub fn stderr_capture(&self) -> &Capture {
        &self.stderr
    }
    pub fn redacted_stderr(&self, redactor: &crate::redaction::Redactor) -> String {
        redactor
            .with_secrets(self.private_redactions.clone())
            .captured_text(&self.stderr.bytes, self.stderr.truncated)
    }
    pub(crate) fn add_private_redactions(&mut self, values: Vec<String>) {
        self.private_redactions.extend(values);
    }
}
impl std::fmt::Display for RuntimeCommandFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} command failed ({})", self.runtime, self.status)
    }
}
impl std::fmt::Debug for RuntimeCommandFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeCommandFailure")
            .field("runtime", &self.runtime)
            .field("status", &self.status)
            .field("stderr_bytes", &self.stderr.bytes.len())
            .field("stderr_total_bytes", &self.stderr.total_bytes)
            .field("stderr_truncated", &self.stderr.truncated)
            .finish()
    }
}
impl std::error::Error for RuntimeCommandFailure {}
pub async fn capture_stream<R: AsyncRead + Unpin>(mut reader: R, limit: usize) -> Result<Capture> {
    let mut capture = Capture {
        bytes: Vec::with_capacity(limit.min(16 * 1024)),
        total_bytes: 0,
        truncated: false,
    };
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        capture.total_bytes = capture.total_bytes.saturating_add(count as u64);
        let retain = count.min(limit.saturating_sub(capture.bytes.len()));
        capture.bytes.extend_from_slice(&buffer[..retain]);
        if retain < count {
            capture.truncated = true;
        }
    }
    Ok(capture)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeState {
    #[serde(rename = "Status", alias = "status", default)]
    pub status: String,
    #[serde(rename = "Running", alias = "running")]
    pub running: bool,
    #[serde(rename = "StartedAt", alias = "startedAt", default)]
    pub started_at: String,
    #[serde(rename = "FinishedAt", alias = "finishedAt", default)]
    pub finished_at: String,
    #[serde(rename = "ExitCode", alias = "exitCode")]
    pub exit_code: i64,
    #[serde(rename = "OOMKilled", alias = "oomKilled", default)]
    pub oom_killed: bool,
}
impl RuntimeState {
    pub fn has_started(&self) -> bool {
        chrono::DateTime::parse_from_rfc3339(&self.started_at).is_ok_and(|t| t.timestamp() > 0)
    }
    fn validate_terminal(&self, kind: RuntimeKind) -> Result<()> {
        // Podman distinguishes a stopped process from its cleaned-up state.
        let terminal =
            self.status == "exited" || kind == RuntimeKind::Podman && self.status == "stopped";
        ensure!(
            terminal && !self.running,
            "runtime did not prove a terminal state"
        );
        ensure!(
            self.exit_code >= 0,
            "runtime did not report an observed exit code"
        );
        let started = chrono::DateTime::parse_from_rfc3339(&self.started_at)
            .context("runtime terminal state has no valid StartedAt")?;
        let finished = chrono::DateTime::parse_from_rfc3339(&self.finished_at)
            .context("runtime terminal state has no valid FinishedAt")?;
        ensure!(
            started.timestamp() > 0 && finished >= started,
            "runtime terminal timestamps are inconsistent"
        );
        Ok(())
    }
}
pub fn parse_runtime_state(bytes: &[u8]) -> Result<RuntimeState> {
    serde_json::from_slice(bytes).context("runtime returned malformed container state")
}

#[derive(Clone, Debug)]
pub struct Runtime {
    pub kind: RuntimeKind,
    invocation: InvocationBindingV1,
    anchor: std::sync::Arc<invocation::InvocationAnchor>,
    observer: Option<observation::ObservationSink>,
    observation: Option<observation::Scope>,
    native_control: Option<control_outcome::NativeControlScope>,
}
#[derive(Debug, Serialize)]
pub struct RuntimeInfo {
    pub runtime: String,
    pub rootless: bool,
    pub server_version: Option<String>,
}
#[derive(Debug)]
pub struct ExecutionResult {
    pub stdout: Capture,
    pub stderr: Capture,
    pub state: RuntimeState,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u128,
    /// A filtered transport loss/operator stop is distinct from the worker exit status.
    pub transport_stop_reason: Option<String>,
}

#[derive(Debug)]
struct CreatedInstance {
    kind: RuntimeKind,
    name: String,
    target: String,
}

#[derive(Default)]
struct TransportTasks(Vec<tokio::task::AbortHandle>);
impl Drop for TransportTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

fn ensure_prearmed_live(stop: &tokio::sync::watch::Receiver<bool>) -> Result<()> {
    ensure!(
        stop.has_changed().is_ok() && !*stop.borrow(),
        "prearmed stop is already signalled or its enforcer disconnected"
    );
    Ok(())
}

async fn wait_prearmed_stop(stop: &mut tokio::sync::watch::Receiver<bool>) -> Result<()> {
    loop {
        if *stop.borrow_and_update() {
            return Ok(());
        }
        stop.changed()
            .await
            .context("prearmed enforcer notification disconnected; reconciliation required")?;
    }
}

async fn drain_prearmed_transport(child: &mut tokio::process::Child) -> Result<()> {
    match timeout(Duration::from_secs(10), child.wait()).await {
        Ok(status) => {
            status.context("prearmed runtime attach wait failed")?;
            Ok(())
        }
        Err(_) => {
            // This is only the engine CLI transport, never the worker process.
            let _ = child.start_kill();
            bail!(
                "prearmed runtime attach did not exit within its stop drainage bound; worker state unknown"
            )
        }
    }
}

/// Cancellation cannot run async finalization. Request a snapshot-verified target stop,
/// without releasing allocation or destroying evidence; subsequent reconciliation stays mandatory.
struct InteractiveCancellation {
    runtime: ManagedRuntime,
    target: String,
    baseline: ContainerObservation,
    armed: bool,
}
impl Drop for InteractiveCancellation {
    fn drop(&mut self) {
        if self.armed
            && let Ok(executor) = tokio::runtime::Handle::try_current()
        {
            let runtime = self.runtime.clone();
            let target = self.target.clone();
            let baseline = self.baseline.clone();
            executor.spawn(async move {
                let _ = runtime.stop_snapshot_target(&target, &baseline).await;
            });
        }
    }
}

/// A terminal result whose created instance remains available for inspection.
/// Dropping this value never removes that instance; cleanup is explicit.
#[derive(Debug)]
pub struct RetainedExecution {
    result: ExecutionResult,
    instance: CreatedInstance,
}
impl RetainedExecution {
    pub fn result(&self) -> &ExecutionResult {
        &self.result
    }
    pub fn container_name(&self) -> &str {
        &self.instance.name
    }
}

struct AttemptFailure {
    error: anyhow::Error,
    instance: Option<CreatedInstance>,
    stop_observed: bool,
}
impl AttemptFailure {
    fn unowned(error: anyhow::Error) -> Self {
        Self {
            error,
            instance: None,
            stop_observed: false,
        }
    }
    fn retained_error(self) -> anyhow::Error {
        match self.instance {
            Some(instance) => self.error.context(format!(
                "session instance {} retained for diagnosis; {}",
                instance.name,
                if self.stop_observed {
                    "runtime observed it not running"
                } else {
                    "stop unknown: runtime unavailable or instance may still be running"
                }
            )),
            None => self.error,
        }
    }
}

fn record_cli_exit(trace: &Option<observation::Scope>, status: Option<std::process::ExitStatus>) {
    if let Some(status) = status {
        observation::record(
            trace,
            if status.success() {
                observation::Phase::StopCommandExitedOk
            } else {
                observation::Phase::StopCommandExitedError
            },
        );
    }
}

impl Runtime {
    pub fn new(kind: RuntimeKind) -> Result<Self> {
        Self::from_invocation(kind, InvocationBindingV1::factory(kind)?)
    }
    pub fn with_program(kind: RuntimeKind, program: &std::path::Path) -> Result<Self> {
        Self::from_invocation(kind, InvocationBindingV1::approve(kind, program)?)
    }
    pub fn from_invocation(kind: RuntimeKind, invocation: InvocationBindingV1) -> Result<Self> {
        let anchor = std::sync::Arc::new(invocation.pin(kind)?);
        Ok(Self {
            kind,
            invocation,
            anchor,
            observer: None,
            observation: None,
            native_control: None,
        })
    }
    /// Optional bounded diagnostics only; never authorizes an operation.
    pub fn with_observation(mut self, observer: observation::ObservationSink) -> Self {
        self.observer = Some(observer);
        self.observation = None;
        self
    }
    fn observed_operation(&self, operation: Uuid) -> Self {
        let mut runtime = self.clone();
        runtime.observation = self.observer.as_ref().map(|sink| sink.operation(operation));
        runtime
    }
    fn trace_target(&self, target: &str) -> Option<observation::Scope> {
        self.observation
            .as_ref()
            .map(|scope| scope.instance(target))
    }
    pub fn invocation(&self) -> &InvocationBindingV1 {
        &self.invocation
    }
    pub fn command(&self) -> Result<Command> {
        self.anchor.verify(&self.invocation, self.kind)?;
        let mut command = self.anchor.command()?;
        command
            .env_clear()
            .envs(&self.invocation.environment)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        Ok(command)
    }
    async fn command_async(&self) -> Result<Command> {
        let runtime = self.clone();
        deadline_supervision::checked_blocking(move || runtime.command()).await
    }
    async fn spawn_capture_command(
        &self,
        mut command: Command,
        args: &[String],
        environment: &[(&str, &str)],
    ) -> Result<(batch_io::OwnedBatchIo, Instant)> {
        let runtime = self.clone();
        let args = args.to_vec();
        let environment: Vec<(String, String)> = environment
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        deadline_supervision::checked_blocking(move || {
            // Full second check and spawn stay in one non-awaiting unit. No
            // scheduler-resumption gap is inserted after the final check.
            runtime.anchor.verify(&runtime.invocation, runtime.kind)?;
            let stop = args.first().map(String::as_str) == Some("kill") && args.len() == 2;
            let trace = if stop { runtime.trace_target(&args[1]) } else { None };
            observation::record(&trace, observation::Phase::StopSpawnRequested);
            let child = command.args(args).envs(environment)
                .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
                .with_context(|| format!("{} is unavailable; install and start a container runtime. No local fallback exists", runtime.kind))?;
            let armed = Instant::now();
            observation::record(&trace, observation::Phase::StopSpawnReturned);
            Ok((batch_io::OwnedBatchIo::own_cli_observed(child, 256 * 1024, 64 * 1024, trace), armed))
        }).await
    }
    pub async fn cli(&self, args: &[String], duration: Duration) -> Result<Capture> {
        self.cli_env(args, duration, &[]).await
    }
    pub async fn cli_env(
        &self,
        args: &[String],
        duration: Duration,
        environment: &[(&str, &str)],
    ) -> Result<Capture> {
        self.capture_cli(self.command_async().await?, args, duration, environment)
            .await
    }
    async fn capture_cli(
        &self,
        command: Command,
        args: &[String],
        duration: Duration,
        environment: &[(&str, &str)],
    ) -> Result<Capture> {
        let stop = args.first().map(String::as_str) == Some("kill") && args.len() == 2;
        let trace = if stop {
            self.trace_target(&args[1])
        } else {
            None
        };
        observation::record(&trace, observation::Phase::StopInvocationRequested);
        let result = self
            .capture_cli_inner(command, args, duration, environment)
            .await;
        observation::record(
            &trace,
            if result.is_ok() {
                observation::Phase::StopInvocationReturnedOk
            } else {
                observation::Phase::StopInvocationReturnedError
            },
        );
        result
    }
    async fn capture_cli_inner(
        &self,
        command: Command,
        args: &[String],
        duration: Duration,
        environment: &[(&str, &str)],
    ) -> Result<Capture> {
        ensure!(
            environment
                .iter()
                .all(|(key, value)| *key == "AGENTSANDBOX_CONFIG_DIR"
                    && std::path::Path::new(value).is_absolute()
                    && !value.chars().any(char::is_control)),
            "runtime environment override is not an approved absolute control configuration directory"
        );
        if let Some(scope) = &self.native_control {
            let (io, armed) = scope.spawn(command, args, environment).await?;
            let mut owned = io.lock().await;
            let result = self.collect_cli_capture(&mut owned, armed, duration).await;
            if owned.is_collected() {
                scope.collected();
            } else if owned.collection_unknown() {
                scope.unknown();
            }
            return result;
        }
        let (mut owned, armed) = self
            .spawn_capture_command(command, args, environment)
            .await?;
        self.collect_cli_capture(&mut owned, armed, duration).await
    }
    async fn collect_cli_capture(
        &self,
        owned: &mut batch_io::OwnedBatchIo,
        armed: Instant,
        duration: Duration,
    ) -> Result<Capture> {
        let mut causes = Vec::new();
        let observed = tokio::time::timeout_at(armed + duration, async {
            match owned.wait_event().await {
                batch_io::BatchIoEvent::Exited(Ok(_)) | batch_io::BatchIoEvent::OwnerFinished => {}
                batch_io::BatchIoEvent::Exited(Err(_)) | batch_io::BatchIoEvent::Fault => {
                    causes.push(anyhow::anyhow!(
                        "runtime control IO unavailable; original causes retained"
                    ));
                    owned.request_transport_kill();
                }
            }
        })
        .await;
        if observed.is_err() {
            causes.push(anyhow::anyhow!("{} control command timed out", self.kind));
            owned.request_transport_kill();
        }
        owned.begin_drain();
        let mut io = match owned.collect().await {
            Ok(io) => io,
            Err(error) => {
                causes.push(anyhow::Error::new(error));
                return Err(anyhow::Error::new(deadline_supervision::Failure {
                    first_cause: None,
                    moved_fallback: None,
                    causes,
                    io: None,
                    native_state: None,
                    native_stop: None,
                    runtime: Some(self.clone()),
                    managed: None,
                    notification: None,
                    work: None,
                }));
            }
        };
        causes.append(&mut io.errors);
        if !io.stdout_eof || !io.stderr_eof {
            causes.push(anyhow::anyhow!(
                "control capture ended without EOF; partial evidence retained"
            ));
        }
        let Some(status) = io.status else {
            causes.push(anyhow::anyhow!("runtime control wait status is unknown"));
            return Err(anyhow::Error::new(deadline_supervision::Failure {
                first_cause: None,
                moved_fallback: None,
                causes,
                io: Some(io),
                native_state: None,
                native_stop: None,
                runtime: Some(self.clone()),
                managed: None,
                notification: None,
                work: None,
            }));
        };
        if !status.success() {
            let stderr = std::mem::replace(
                &mut io.stderr,
                Capture {
                    bytes: Vec::new(),
                    total_bytes: 0,
                    truncated: false,
                },
            );
            let failure = RuntimeCommandFailure {
                runtime: self.kind,
                status,
                stderr,
                private_redactions: Vec::new(),
            };
            if causes.is_empty() {
                // Preserve the established downcast/redaction interface while
                // retaining the other bounded bytes behind a private context.
                return Err(anyhow::Error::new(failure).context(CliEvidence(io)));
            }
            causes.push(anyhow::Error::new(failure));
        }
        if !causes.is_empty() {
            return Err(anyhow::Error::new(deadline_supervision::Failure {
                first_cause: None,
                moved_fallback: None,
                causes,
                io: Some(io),
                native_state: None,
                native_stop: None,
                runtime: Some(self.clone()),
                managed: None,
                notification: None,
                work: None,
            }));
        }
        Ok(io.stdout)
    }
    pub async fn doctor(&self, allow_rootful: bool) -> Result<RuntimeInfo> {
        self.doctor_on(Vec::new(), allow_rootful).await
    }
    /// Read the Podman session target without creating a managed authority.
    pub async fn diagnose_podman(
        &self,
        endpoint: &str,
        allow_rootful: bool,
    ) -> Result<(RuntimeInfo, Capture)> {
        ensure!(
            self.kind == RuntimeKind::Podman,
            "Podman diagnostics require an approved Podman executable"
        );
        let mut target = selected_podman_target_args(endpoint)?;
        let info = self.doctor_on(target.clone(), allow_rootful).await?;
        target.push("version".into());
        let version = self.cli(&target, Duration::from_secs(15)).await?;
        ensure!(
            !version.truncated,
            "runtime version exceeds its response bound"
        );
        Ok((info, version))
    }
    async fn doctor_on(&self, mut target: Vec<String>, allow_rootful: bool) -> Result<RuntimeInfo> {
        let format = match self.kind {
            RuntimeKind::Docker => "{{json .}}",
            RuntimeKind::Podman => "json",
        };
        target.extend(["info".into(), "--format".into(), format.into()]);
        let output = self.cli(&target, Duration::from_secs(15)).await?;
        if output.truncated {
            bail!("runtime info exceeds its response bound");
        }
        parse_runtime_info(self.kind, &output.bytes, allow_rootful)
    }
    pub async fn create_volume(&self, session: Uuid) -> Result<()> {
        self.create_named_volume(
            &volume_name(session),
            &[("agentsandbox.session".into(), session.to_string())],
        )
        .await
    }
    pub async fn create_named_volume(
        &self,
        volume: &str,
        labels: &[(String, String)],
    ) -> Result<()> {
        self.cli(&named_volume_args(volume, labels)?, Duration::from_secs(30))
            .await?;
        Ok(())
    }
    pub async fn inspect(&self, spec: &ContainerSpec) -> Result<RuntimeState> {
        self.inspect_target(&spec.name()).await
    }
    async fn inspect_target(&self, target: &str) -> Result<RuntimeState> {
        let output = self
            .cli(
                &[
                    "inspect".into(),
                    "--format".into(),
                    "{{json .State}}".into(),
                    target.into(),
                ],
                Duration::from_secs(10),
            )
            .await?;
        parse_runtime_state(&output.bytes)
    }
    async fn remove_container(&self, target: &str) -> Result<()> {
        self.cli(
            &["rm".into(), "--force".into(), target.into()],
            Duration::from_secs(20),
        )
        .await?;
        Ok(())
    }
    async fn kill_container(&self, target: &str) -> Result<()> {
        self.cli(&["kill".into(), target.into()], Duration::from_secs(15))
            .await?;
        Ok(())
    }
    /// Remove only the instance owned by this successful execution. The
    /// borrowed terminal result remains available even if cleanup fails.
    pub async fn cleanup(&self, execution: &RetainedExecution) -> Result<()> {
        ensure!(
            execution.instance.kind == self.kind,
            "retained instance belongs to another runtime kind"
        );
        self.remove_container(&execution.instance.target).await
    }
    async fn stop_retained_instance(&self, instance: &CreatedInstance) -> bool {
        fn not_running(state: &RuntimeState) -> bool {
            !state.running
                && matches!(
                    state.status.as_str(),
                    "created" | "initialized" | "configured" | "exited" | "stopped" | "dead"
                )
        }
        if self
            .inspect_target(&instance.target)
            .await
            .is_ok_and(|state| not_running(&state))
        {
            return true;
        }
        let _ = self.kill_container(&instance.target).await;
        self.inspect_target(&instance.target)
            .await
            .is_ok_and(|state| not_running(&state))
    }
    /// Run without deleting the created instance. The caller must durably
    /// finalize the result before invoking cleanup. Errors after create attempt
    /// a targeted stop and retain the instance, with unknown stop reported.
    pub async fn execute_retained<F, Fut>(
        &self,
        spec: &ContainerSpec,
        input: Option<std::fs::File>,
        duration: Duration,
        stdout_limit: usize,
        on_started: F,
    ) -> Result<RetainedExecution>
    where
        F: FnMut(RuntimeState) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        self.observed_operation(spec.operation_id())
            .execute_attempt(spec, input, duration, stdout_limit, on_started)
            .await
            .map_err(AttemptFailure::retained_error)
    }
    /// The callback is invoked only after inspect proves that the container
    /// started. This compatibility wrapper automatically cleans up its owned
    /// instance; durable worker finalization must use execute_retained instead.
    pub async fn execute<F, Fut>(
        &self,
        spec: &ContainerSpec,
        input: Option<std::fs::File>,
        duration: Duration,
        stdout_limit: usize,
        on_started: F,
    ) -> Result<ExecutionResult>
    where
        F: FnMut(RuntimeState) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        match self
            .observed_operation(spec.operation_id())
            .execute_attempt(spec, input, duration, stdout_limit, on_started)
            .await
        {
            Ok(retained) => {
                self.cleanup(&retained)
                    .await
                    .context("session container cleanup failed; its volume is retained")?;
                Ok(retained.result)
            }
            Err(failure) => {
                if let Some(instance) = failure.instance
                    && let Err(cleanup) = self.remove_container(&instance.target).await
                {
                    return Err(failure.error)
                        .context(format!("session container cleanup also failed: {cleanup}"));
                }
                Err(failure.error)
            }
        }
    }
    async fn execute_attempt<F, Fut>(
        &self,
        spec: &ContainerSpec,
        input: Option<std::fs::File>,
        duration: Duration,
        stdout_limit: usize,
        on_started: F,
    ) -> std::result::Result<RetainedExecution, AttemptFailure>
    where
        F: FnMut(RuntimeState) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        if spec.security_preset.is_interactive() {
            return Err(AttemptFailure::unowned(anyhow::anyhow!(
                "interactive execution requires ManagedRuntime and a durable admission; no engine command was emitted"
            )));
        }
        if duration.is_zero() || duration > Duration::from_secs(24 * 60 * 60) {
            return Err(AttemptFailure::unowned(anyhow::anyhow!(
                "runtime timeout must be between 1 second and 24 hours"
            )));
        }
        let args = spec
            .create_args(self.kind)
            .map_err(AttemptFailure::unowned)?;
        // A failed create does not grant ownership of that name. In particular,
        // a conflict must never remove another invocation's existing helper.
        let created = self
            .cli(
                {
                    spec.validate_secret_env_file()
                        .map_err(AttemptFailure::unowned)?;
                    &args
                },
                Duration::from_secs(30),
            )
            .await
            .context("container creation failed with automatic image pulls disabled (--pull=never); prepare the selected image explicitly if it is absent locally")
            .map_err(AttemptFailure::unowned)?;
        // A successful CLI status without a usable returned reference still
        // does not prove ownership of whatever currently has the requested
        // name. Never adopt, stop, or remove that name as a fallback.
        let target = (|| -> Result<String> {
            ensure!(
                !created.truncated,
                "runtime create reference exceeds its bound"
            );
            let target = std::str::from_utf8(&created.bytes)?.trim();
            ensure!(
                !target.is_empty()
                    && target.len() <= 128
                    && !target.starts_with('-')
                    && target
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
                "runtime did not return a valid created instance reference"
            );
            Ok(target.into())
        })()
        .map_err(|error| {
            AttemptFailure::unowned(error.context(format!(
                "create response for requested name {} unusable; ownership unknown, runtime state unknown; no stop or cleanup attempted",
                spec.name()
            )))
        })?;
        let instance = CreatedInstance {
            kind: self.kind,
            name: spec.name(),
            target,
        };
        let mut moved_batch_fallback = None;
        let execution = async {
            // The compatibility path also checks the selected logging and
            // stream profile before the first attach/start. Never persist raw
            // inspection bytes or deserialize Config.Env.
            let facts = self
                .cli(
                    &[
                        "inspect".into(),
                        "--format".into(),
                        "{{json .}}".into(),
                        instance.target.clone(),
                    ],
                    Duration::from_secs(10),
                )
                .await?;
            ensure!(!facts.truncated, "runtime inspection exceeds its bound");
            managed::validate_stream_configuration(spec, &facts.bytes, self.kind)?;
            let work = deadline_supervision::WorkObservation::for_direct(self, &instance)?;
            self.execute_created(
                &instance,
                input,
                duration,
                stdout_limit,
                None,
                None,
                Some(work),
                None,
                on_started,
            )
            .await
            .map_err(|failure| {
                moved_batch_fallback = failure.moved_fallback;
                anyhow::Error::new(*failure)
            })
        }
        .await;
        match execution {
            Ok(result) => Ok(RetainedExecution { result, instance }),
            Err(error) => {
                let stop_observed =
                    deadline_supervision::resolve_outer_fallback(moved_batch_fallback, || {
                        self.stop_retained_instance(&instance)
                    })
                    .await;
                Err(AttemptFailure {
                    error,
                    instance: Some(instance),
                    stop_observed,
                })
            }
        }
    }
    #[allow(clippy::too_many_arguments)]
    async fn execute_created<F, Fut>(
        &self,
        instance: &CreatedInstance,
        input: Option<std::fs::File>,
        duration: Duration,
        stdout_limit: usize,
        managed: Option<(&ManagedRuntime, &ContainerObservation)>,
        interactive_secrets: Option<&[String]>,
        work: Option<std::sync::Arc<deadline_supervision::WorkObservation>>,
        legacy_veto: Option<LegacyStartVeto>,
        mut on_started: F,
    ) -> Result<ExecutionResult, Box<deadline_supervision::Failure>>
    where
        F: FnMut(RuntimeState) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        use deadline_supervision::{
            Cause, CauseLatch, Failure, Notification, NotificationState, Supervisor,
        };
        if let Some(secrets) = interactive_secrets {
            if input.is_some() {
                return Err(Failure::unmoved(anyhow::anyhow!(
                    "interactive transport cannot consume an archive input"
                )));
            }
            // The interactive monitor remains a distinct, uncorrected path.
            return self
                .execute_interactive(
                    instance,
                    duration,
                    managed,
                    secrets,
                    legacy_veto,
                    on_started,
                )
                .await
                .map_err(Failure::unmoved);
        }
        let work = match (managed, work) {
            (Some((authority, baseline)), Some(work)) => {
                work.require_binding(authority, &instance.target, baseline)
                    .map_err(Failure::unmoved)?;
                work
            }
            (None, Some(work)) => {
                work.require_direct_binding(self, instance)
                    .map_err(Failure::unmoved)?;
                work
            }
            _ => {
                return Err(Failure::unmoved(anyhow::anyhow!(
                    "batch observation owner/scope missing or mismatched"
                )));
            }
        };
        if let Some((authority, _)) = managed {
            authority
                .verify_authority()
                .await
                .map_err(Failure::unmoved)?;
        }
        let trace = self.trace_target(&instance.target);
        let runtime = self.clone();
        let authority = managed.map(|(authority, _)| authority.clone());
        let spawn_trace = trace.clone();
        let target = instance.target.clone();
        let (io, start) = deadline_supervision::checked_blocking(move || {
            // The original complete final command check stays adjacent to the
            // effect. The join owns the spawned result, never an untracked PID.
            let mut command = match authority {
                Some(authority) => authority.command()?,
                None => runtime.command()?,
            };
            command.arg("start").arg("--attach");
            if input.is_some() {
                command.arg("--interactive").stdin(Stdio::piped());
            }
            command.arg(&target);
            if let Some(veto) = legacy_veto {
                veto.check(&target)?;
            }
            observation::record(&spawn_trace, observation::Phase::StartSpawnRequested);
            let child = command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context("cannot attach to the session container")?;
            let armed = Instant::now();
            observation::record(&spawn_trace, observation::Phase::StartSpawnReturned);
            if let Some(trace) = &spawn_trace {
                trace.at(observation::Phase::DeadlineArmed, Some(armed + duration));
            }
            // Own Child and all stdio before the blocking result can be yielded.
            Ok((
                batch_io::OwnedBatchIo::own_observed(child, input, stdout_limit, spawn_trace),
                armed,
            ))
        })
        .await
        .map_err(Failure::unmoved)?;
        let deadline = start + duration;
        let cause = CauseLatch::default();
        let stop_cause = cause.clone();
        let stop_work = work.clone();
        let stop = self.stop_batch_native(instance, managed, &stop_work, &stop_cause);
        let mut supervisor = Supervisor::new(io, deadline, cause, stop)
            .with_work(work.clone())
            .with_observation(trace.clone());
        let mut interval = tokio::time::interval(Duration::from_millis(200));
        let mut observed_start = false;
        let notification = Notification::new();
        while !observed_start && !supervisor.attached_exit() && !supervisor.closing() {
            // Waiting for the next tick is itself supervised. A STOP here must
            // not start another native observation or callback afterward.
            let tick = interval.tick();
            tokio::pin!(tick);
            supervisor.drive(tick.as_mut()).await;
            if supervisor.closing() || supervisor.attached_exit() {
                break;
            }
            let result = {
                let observation = async {
                    let state = self
                        .inspect_batch_state(&instance.target, managed, &work)
                        .await?;
                    if state.has_started() {
                        notification.notify(state, &mut on_started).await?;
                        Ok::<bool, anyhow::Error>(true)
                    } else {
                        Ok(false)
                    }
                };
                tokio::pin!(observation);
                supervisor.drive(observation.as_mut()).await
            };
            match result {
                Ok(started) => observed_start = started,
                Err(error) => {
                    supervisor.operation_failed(error);
                }
            }
        }
        supervisor.finish_stop().await;
        supervisor.wait_attach().await;
        supervisor.finish_stop().await;
        let already_closing = supervisor.closing();
        let mut state = None;
        let final_result = {
            let final_observation = async {
                let observed = self
                    .inspect_batch_state(&instance.target, managed, &work)
                    .await?;
                state = Some(observed.clone());
                // Preserve the existing owed final notification, including
                // Started observed only after STOP. Engaged/failed is NOT retry.
                if notification.get() == NotificationState::NotEngaged && observed.has_started() {
                    notification.notify(observed, &mut on_started).await?;
                }
                Ok::<(), anyhow::Error>(())
            };
            tokio::pin!(final_observation);
            supervisor.drive(final_observation.as_mut()).await
        };
        if let Err(error) = final_result {
            supervisor.operation_failed(error);
        }
        supervisor.finish_stop().await;
        if !already_closing && supervisor.closing() {
            // The final inspection raced the first STOP. Reconcile only the
            // terminal facts after that stop; never re-enter on_started.
            match self
                .inspect_batch_state(&instance.target, managed, &work)
                .await
            {
                Ok(observed) => state = Some(observed),
                Err(error) => supervisor.operation_failed(error),
            }
        }
        let validity = match state.as_ref() {
            Some(state) if state.has_started() => state.validate_terminal(self.kind),
            _ => Err(anyhow::anyhow!(
                "runtime never reported a StartedAt value; command start cannot be confirmed"
            )),
        };
        if let Err(error) = validity {
            let was_closing = supervisor.closing();
            supervisor.operation_failed(error);
            supervisor.finish_stop().await;
            if !was_closing {
                // The original outer failure fallback is now completed here.
                // This read is diagnostic reconciliation, not a start retry.
                match self
                    .inspect_batch_state(&instance.target, managed, &work)
                    .await
                {
                    Ok(observed) => state = Some(observed),
                    Err(error) => supervisor.operation_failed(error),
                }
            }
        }
        // No consuming timeout: the same IO owner's handle is awaited to actual
        // completion. Its own unchanged budgets retain late outcomes/causes.
        let mut collected = match supervisor.io.collect().await {
            Ok(io) => {
                observation::record(&trace, observation::Phase::DrainCollected);
                Some(io)
            }
            Err(error) => {
                observation::record(&trace, observation::Phase::DrainCollectionFailed);
                supervisor.errors.push(anyhow::Error::new(error));
                None
            }
        };
        if let Some(io) = collected.as_mut() {
            supervisor.errors.append(&mut io.errors);
            if !io.stdout_eof || !io.stderr_eof {
                supervisor.errors.push(anyhow::anyhow!(
                    "batch capture ended without EOF; partial evidence retained"
                ));
            }
            if io.status.is_none() {
                supervisor
                    .errors
                    .push(anyhow::anyhow!("batch attach has no observed wait status"));
            }
        }
        let moved_fallback =
            deadline_supervision::outer_fallback_decision(&work, supervisor.stop_result.as_ref());
        let mut native_stop = None;
        if let Some(stop) = supervisor.stop_result.as_mut() {
            supervisor.errors.append(&mut stop.errors);
            native_stop = stop.evidence.take();
        }
        let timed_out = supervisor.cause.get() == Some(Cause::Deadline);
        match (collected, state) {
            (Some(io), Some(state)) if supervisor.errors.is_empty() => match io.status {
                Some(status) => Ok(ExecutionResult {
                    stdout: io.stdout,
                    stderr: io.stderr,
                    state,
                    exit_code: status.code(),
                    timed_out,
                    duration_ms: start.elapsed().as_millis(),
                    transport_stop_reason: None,
                }),
                None => Err(Box::new(Failure {
                    first_cause: supervisor.cause.get(),
                    moved_fallback,
                    causes: vec![anyhow::anyhow!("batch attach wait not collected")],
                    io: Some(io),
                    native_state: Some(state),
                    native_stop,
                    runtime: Some(self.clone()),
                    managed: managed.map(|(authority, _)| authority.clone()),
                    notification: Some(notification.get()),
                    work: Some(work),
                })),
            },
            (io, state) => Err(Box::new(Failure {
                first_cause: supervisor.cause.get(),
                moved_fallback,
                causes: supervisor.errors,
                io,
                native_state: state,
                native_stop,
                runtime: Some(self.clone()),
                managed: managed.map(|(authority, _)| authority.clone()),
                notification: Some(notification.get()),
                work: Some(work),
            })),
        }
    }

    /// The same owned stop future used by the real batch supervisor. A cached
    /// terminal shortcut has performed no native fallback: contradiction must
    /// not preserve that exemption through moved_outer_fallback (R2-F01).
    async fn stop_batch_native(
        &self,
        instance: &CreatedInstance,
        managed: Option<(&ManagedRuntime, &ContainerObservation)>,
        work: &deadline_supervision::WorkObservation,
        cause: &deadline_supervision::CauseLatch,
    ) -> deadline_supervision::NativeStop {
        use deadline_supervision::{Cause, NativeStop, StopEvidence};
        observation::record(
            &self.trace_target(&instance.target),
            observation::Phase::StopRequested,
        );
        let binding = match managed {
            Some((authority, baseline)) => {
                work.require_binding(authority, &instance.target, baseline)
            }
            None => work.require_direct_binding(self, instance),
        };
        if let Err(error) = binding {
            return NativeStop {
                observed_stopped: false,
                evidence: None,
                errors: vec![error],
                moved_outer_fallback: false,
            };
        }
        let moved_outer_fallback = matches!(
            cause.get(),
            Some(Cause::OperationFailure | Cause::IoFailure)
        );
        if let Some(evidence) = work.terminal_evidence() {
            return NativeStop {
                observed_stopped: true,
                evidence: Some(evidence),
                errors: Vec::new(),
                moved_outer_fallback: false,
            };
        }
        match managed {
            Some((authority, baseline)) => {
                match authority
                    .stop_snapshot_target(&instance.target, baseline)
                    .await
                {
                    Ok(observed) => NativeStop {
                        observed_stopped: true,
                        evidence: Some(StopEvidence::Managed(Box::new(observed))),
                        errors: Vec::new(),
                        moved_outer_fallback,
                    },
                    Err(error) => NativeStop {
                        observed_stopped: false,
                        evidence: None,
                        errors: vec![error],
                        moved_outer_fallback,
                    },
                }
            }
            None if moved_outer_fallback => self.collect_batch_fallback(instance).await,
            None => match self.kill_container(&instance.target).await {
                Ok(()) => NativeStop {
                    observed_stopped: false,
                    evidence: None,
                    errors: Vec::new(),
                    moved_outer_fallback,
                },
                Err(error) => NativeStop {
                    observed_stopped: false,
                    evidence: None,
                    errors: vec![error],
                    moved_outer_fallback,
                },
            },
        }
    }

    async fn inspect_batch_state(
        &self,
        target: &str,
        managed: Option<(&ManagedRuntime, &ContainerObservation)>,
        work: &deadline_supervision::WorkObservation,
    ) -> Result<RuntimeState> {
        // Binding was checked against this exact Runtime/CreatedInstance before
        // spawn. The private scope performs the same actual native inspection,
        // closes WORK on a validated terminal, then returns descriptive state.
        if let Some((authority, created)) = managed {
            work.require_binding(authority, target, created)?;
        } else {
            work.require_direct_target(self, target)?;
        }
        let result = work.inspect().await;
        if result
            .as_ref()
            .is_ok_and(|state| state.validate_terminal(self.kind).is_ok())
        {
            observation::record(
                &self.trace_target(target),
                observation::Phase::TerminalInspected,
            );
        }
        result
    }

    /// Same operation ordering as the bare Runtime's original error fallback.
    /// Preserve its causes instead of reducing the actual stop to a caller bool.
    async fn collect_batch_fallback(
        &self,
        instance: &CreatedInstance,
    ) -> deadline_supervision::NativeStop {
        use deadline_supervision::{NativeStop, StopEvidence};
        let not_running = |state: &RuntimeState| {
            !state.running
                && matches!(
                    state.status.as_str(),
                    "created" | "initialized" | "configured" | "exited" | "stopped" | "dead"
                )
        };
        let mut errors = Vec::new();
        match self.inspect_target(&instance.target).await {
            Ok(state) if not_running(&state) => {
                return NativeStop {
                    observed_stopped: true,
                    evidence: Some(StopEvidence::Bare(state)),
                    errors,
                    moved_outer_fallback: true,
                };
            }
            Ok(_) => {}
            Err(error) => errors.push(error),
        }
        if let Err(error) = self.kill_container(&instance.target).await {
            errors.push(error);
        }
        let state = match self.inspect_target(&instance.target).await {
            Ok(state) => Some(state),
            Err(error) => {
                errors.push(error);
                None
            }
        };
        let observed_stopped = state.as_ref().is_some_and(not_running);
        if !observed_stopped {
            errors.push(anyhow::anyhow!("bare Runtime stop remains unknown"));
        }
        NativeStop {
            observed_stopped,
            evidence: state.map(StopEvidence::Bare),
            errors,
            moved_outer_fallback: true,
        }
    }
    /// Batch transport for a service whose enforcer was armed before the claim.
    /// There is deliberately no Ctrl-C subscription or business duration here.
    #[allow(clippy::too_many_arguments)]
    async fn execute_created_prearmed<F, Fut>(
        &self,
        instance: &CreatedInstance,
        input: Option<std::fs::File>,
        stdout_limit: usize,
        managed: (&ManagedRuntime, &ContainerObservation),
        control: PrearmedStartControl,
        mut on_started: F,
    ) -> Result<ExecutionResult>
    where
        F: FnMut(RuntimeState) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let (authority, _) = managed;
        authority.verify_authority().await.with_context(|| {
            RuntimePrestartStageFailure::new(
                RuntimePrestartStage::AuthorityBeforeSpawn,
                &instance.target,
            )
        })?;
        let (mut command, prepared_profile, _retained_profile) =
            authority.prearmed_command().with_context(|| {
                RuntimePrestartStageFailure::new(
                    RuntimePrestartStage::InvocationBeforeSpawn,
                    &instance.target,
                )
            })?;
        command.arg("start").arg("--attach");
        if input.is_some() {
            command.arg("--interactive").stdin(Stdio::piped());
        }
        command
            .arg(&instance.target)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let PrearmedStartControl {
            guard,
            mut stop,
            launch_profile,
        } = control;
        let guard: Box<dyn BeforeSpawnGuard> = Box::new(ProfileBoundSpawnGuard {
            inner: guard,
            expected: launch_profile,
            prepared: prepared_profile,
        });
        ensure_prearmed_live(&stop).with_context(|| {
            RuntimePrestartStageFailure::new(
                RuntimePrestartStage::StopBeforeSpawn,
                &instance.target,
            )
        })?;
        // These closures allocate diagnostic metadata only on an error.
        let guard_failure = || {
            RuntimePrestartStageFailure::new(
                RuntimePrestartStage::GuardBeforeSpawn,
                &instance.target,
            )
        };
        let spawn_failure =
            || RuntimePrestartStageFailure::new(RuntimePrestartStage::Spawn, &instance.target);
        // Keep the final trusted check adjacent to spawn: no await, command
        // preparation, filesystem access or callback is permitted between them.
        guard
            .check_before_spawn(&instance.target)
            .with_context(guard_failure)?;
        let mut child = command.spawn().with_context(spawn_failure)?;
        let mut tasks = TransportTasks::default();
        let stdout = tokio::spawn(capture_stream(
            child.stdout.take().context("missing stdout")?,
            stdout_limit,
        ));
        tasks.0.push(stdout.abort_handle());
        let stderr = tokio::spawn(capture_stream(
            child.stderr.take().context("missing stderr")?,
            256 * 1024,
        ));
        tasks.0.push(stderr.abort_handle());
        let writer = if let Some(file) = input {
            let mut sink = child.stdin.take().context("missing archive stdin")?;
            let task = tokio::spawn(async move {
                let mut source = tokio::fs::File::from_std(file);
                tokio::io::copy(&mut source, &mut sink).await
            });
            tasks.0.push(task.abort_handle());
            Some(task)
        } else {
            None
        };
        let start = Instant::now();
        let mut observed_start = false;
        let (status, state) = {
            // Poll inspection AND its callback as a select branch. A pending
            // writer/HTTP callback must not hide the independent stop signal.
            let started = async {
                let mut interval = tokio::time::interval(Duration::from_millis(200));
                loop {
                    interval.tick().await;
                    let state = self
                        .execution_inspect(&instance.target, Some(managed))
                        .await
                        .with_context(|| {
                            RuntimePrestartStageFailure::new(
                                RuntimePrestartStage::StartedObservation,
                                &instance.target,
                            )
                        })?;
                    if state.has_started() {
                        on_started(state).await.with_context(|| {
                            RuntimePrestartStageFailure::new(
                                RuntimePrestartStage::StartedObservation,
                                &instance.target,
                            )
                        })?;
                        return Ok::<(), anyhow::Error>(());
                    }
                }
            };
            tokio::pin!(started);
            let status = loop {
                tokio::select! {
                    // A known stop/closed notifier wins over simultaneous EOF.
                    biased;
                    stopped = wait_prearmed_stop(&mut stop) => {
                        drain_prearmed_transport(&mut child).await?;
                        stopped?;
                        bail!("independent stop signalled; owned instance retained for terminal reconciliation");
                    }
                    status = child.wait() => break status?,
                    result = &mut started, if !observed_start => {
                        result?;
                        observed_start = true;
                    }
                }
            };
            let state = tokio::select! {
                biased;
                stopped = wait_prearmed_stop(&mut stop) => {
                    stopped?;
                    bail!("independent stop signalled during observation; terminal reconciliation required");
                }
                state = self.execution_inspect(&instance.target, Some(managed)) => state.with_context(|| {
                    RuntimePrestartStageFailure::new(
                        RuntimePrestartStage::StartedObservation,
                        &instance.target,
                    )
                })?,
            };
            (|| -> Result<()> {
                ensure!(
                    state.has_started(),
                    "runtime never reported a StartedAt value; command start cannot be confirmed"
                );
                state.validate_terminal(self.kind)
            })()
            .with_context(|| {
                RuntimePrestartStageFailure::new(
                    RuntimePrestartStage::StartedObservation,
                    &instance.target,
                )
            })?;
            if !observed_start {
                // EOF may race a callback that has already sent its RPC. Keep
                // that exact future alive: invoking the callback again could
                // create a different operation instead of replaying the first.
                tokio::select! {
                    biased;
                    stopped = wait_prearmed_stop(&mut stop) => {
                        stopped?;
                        bail!("independent stop signalled during started acknowledgement; terminal reconciliation required");
                    }
                    result = &mut started => result?,
                }
            }
            (status, state)
        };
        let finish = async {
            if let Some(writer) = writer {
                match timeout(Duration::from_secs(5), writer).await {
                    Ok(Ok(Ok(_))) => {}
                    other if status.success() => {
                        bail!("workspace archive transfer failed: {other:?}")
                    }
                    _ => {}
                }
            }
            let stdout = timeout(Duration::from_secs(10), stdout)
                .await
                .context("stdout pipe did not close")???;
            let stderr = timeout(Duration::from_secs(10), stderr)
                .await
                .context("stderr pipe did not close")???;
            Ok(ExecutionResult {
                stdout,
                stderr,
                state,
                exit_code: status.code(),
                // Internal compatibility value only. The public prearmed result
                // discards this field; only the enforcer records deadline facts.
                timed_out: false,
                duration_ms: start.elapsed().as_millis(),
                transport_stop_reason: None,
            })
        };
        tokio::select! {
            biased;
            stopped = wait_prearmed_stop(&mut stop) => {
                stopped?;
                bail!("independent stop signalled during observation; terminal reconciliation required");
            }
            result = finish => result,
        }
    }
    async fn execute_interactive<F, Fut>(
        &self,
        instance: &CreatedInstance,
        duration: Duration,
        managed: Option<(&ManagedRuntime, &ContainerObservation)>,
        secrets: &[String],
        legacy_veto: Option<LegacyStartVeto>,
        mut on_started: F,
    ) -> Result<ExecutionResult>
    where
        F: FnMut(RuntimeState) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let (managed_runtime, created) =
            managed.context("interactive execution requires the durable managed runtime")?;
        managed_runtime.verify_authority().await?;
        let mut command = managed_runtime.command()?;
        command
            .args([
                "start",
                "--attach",
                "--interactive",
                "--detach-keys=ctrl-x,ctrl-x",
            ])
            .arg(&instance.target);
        // Register external signals before raw mode. A typed Ctrl-C is an input byte, not SIGINT.
        let mut signals = terminal::ExternalSignals::new()?;
        let target = instance.target.clone();
        // Own and join setup/spawn off the polling thread. A slow foreground
        // banner/PTY/FD operation must not prevent the control owner revoking
        // admission; the same veto is consumed only at the real spawn boundary.
        let (mut session, mut child) =
            deadline_supervision::checked_blocking(move || match legacy_veto {
                Some(veto) => terminal::TerminalSession::spawn_with_veto(command, veto, target),
                None => terminal::TerminalSession::spawn(command),
            })
            .await?;
        let mut cancellation = InteractiveCancellation {
            runtime: managed_runtime.clone(),
            target: instance.target.clone(),
            baseline: created.clone(),
            armed: true,
        };
        let start = Instant::now();
        let deadline = start + duration;
        let mut interval = tokio::time::interval(Duration::from_millis(200));
        let mut observed_start = false;
        let mut timed_out = false;
        let mut transport_issue = None;
        let mut attach_exit = None;
        let execution: Result<()> = async {
            let (drain, draining) = tokio::sync::watch::channel(false);
            let pump = session.pump(secrets.to_vec(), draining);
            tokio::pin!(pump);
            let mut child_exited = false;
            let drain_deadline = tokio::time::sleep(terminal::DRAIN_TIMEOUT);
            tokio::pin!(drain_deadline);
            loop {
                tokio::select! {
                    status = child.wait(), if !child_exited => {
                        attach_exit = status?.code();
                        child_exited = true;
                        drain_deadline.as_mut().reset(Instant::now() + terminal::DRAIN_TIMEOUT);
                        drain.send(true).context("terminal pump ended before runtime drain request")?;
                        // A transport exit does not discard the queued tail or prove worker termination.
                    }
                    _ = &mut drain_deadline, if child_exited => {
                        transport_issue = Some(terminal::TransportIssue::Failure("runtime terminal drain did not observe EOF within its bound".into()));
                        break;
                    }
                    result = &mut pump => {
                        transport_issue = Some(match result {
                            Ok(end) => terminal::TransportIssue::End(end),
                            Err(error) => terminal::TransportIssue::Failure(error.to_string()),
                        });
                        break;
                    }
                    _ = tokio::time::sleep_until(deadline) => {
                        timed_out = true;
                        transport_issue = Some(terminal::TransportIssue::Deadline);
                        break;
                    }
                    _ = signals.received() => {
                        transport_issue = Some(terminal::TransportIssue::ExternalSignal);
                        break;
                    }
                    _ = interval.tick(), if !observed_start => {
                        let state = self.execution_inspect(&instance.target, managed).await?;
                        if state.has_started() { on_started(state).await?; observed_start = true; }
                    }
                }
            }
            Ok(())
        }
        .await;
        // Restore before any error can reach caller diagnostics, and still request stop if restoration failed.
        let restored = session.restore();
        let first_observed = self.execution_inspect(&instance.target, managed).await;
        let already_terminal = first_observed
            .as_ref()
            .is_ok_and(|state| state.validate_terminal(self.kind).is_ok());
        let mut transport_reason = transport_issue.and_then(|issue| issue.reason(already_terminal));
        let mut state = first_observed;
        if !already_terminal || execution.is_err() || timed_out {
            if transport_reason.is_none() {
                transport_reason =
                    Some("runtime attach ended without terminal worker proof".into());
            }
            // A concurrent real worker exit may make kill fail. Only inspection decides its outcome.
            let _ = self.execution_kill(&instance.target, managed).await;
            let _ = child.kill().await;
            let stop_deadline = Instant::now() + Duration::from_secs(10);
            loop {
                state = self.execution_inspect(&instance.target, managed).await;
                if state
                    .as_ref()
                    .is_ok_and(|state| state.validate_terminal(self.kind).is_ok())
                {
                    break;
                }
                ensure!(
                    Instant::now() < stop_deadline,
                    "interactive worker terminal state is unknown after stop; allocation and instance retained"
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        let _ = timeout(Duration::from_secs(2), child.wait()).await;
        execution?;
        restored?;
        let state = state?;
        if !observed_start && state.has_started() {
            on_started(state.clone()).await?;
        }
        ensure!(
            state.has_started(),
            "runtime never independently reported the interactive worker start"
        );
        state.validate_terminal(self.kind)?;
        cancellation.armed = false;
        // The merged PTY stream is never a stdout/stderr capture. The runner exposes these counters as unknown.
        let empty = || Capture {
            bytes: Vec::new(),
            total_bytes: 0,
            truncated: false,
        };
        Ok(ExecutionResult {
            stdout: empty(),
            stderr: empty(),
            state,
            exit_code: attach_exit,
            timed_out,
            duration_ms: start.elapsed().as_millis(),
            transport_stop_reason: transport_reason,
        })
    }
    async fn execution_inspect(
        &self,
        target: &str,
        managed: Option<(&ManagedRuntime, &ContainerObservation)>,
    ) -> Result<RuntimeState> {
        match managed {
            Some((managed, created)) => managed.inspect_process(target, created).await,
            None => self.inspect_target(target).await,
        }
    }
    async fn execution_kill(
        &self,
        target: &str,
        managed: Option<(&ManagedRuntime, &ContainerObservation)>,
    ) -> Result<()> {
        match managed {
            Some((managed, created)) => managed
                .stop_snapshot_target(target, created)
                .await
                .map(|_| ()),
            None => self.kill_container(target).await,
        }
    }
}

/// Exact configured CPU allocation, reduced to an integer ratio of cores.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuQuota {
    pub quota: u64,
    pub period: u64,
}
impl CpuQuota {
    fn reduced(quota: u64, period: u64) -> Self {
        let (mut a, mut b) = (quota, period);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        Self {
            quota: quota / a,
            period: period / a,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedAllocation {
    pub cpu: Option<CpuQuota>,
    pub memory_bytes: Option<u64>,
    pub pids_limit: Option<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationIssue {
    Missing,
    Invalid,
    Unbounded,
    Inconsistent,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationDiagnostic {
    pub field: String,
    pub issue: ObservationIssue,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcessObservation {
    pub status: Option<String>,
    pub running: Option<bool>,
    pub started_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub finished_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub exit_code: Option<u32>,
    pub oom_killed: Option<bool>,
}
/// Whitelisted runtime facts. Environment, provider output and raw inspect JSON
/// are deliberately absent so this value can be passed to durable control.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContainerObservation {
    pub id: Option<String>,
    pub name: String,
    pub image_id: Option<String>,
    pub restart_count: Option<u64>,
    pub process: ProcessObservation,
    pub allocation: ObservedAllocation,
    /// Digest of selected command, mount and sandbox configuration; excludes Env.
    #[serde(default)]
    pub configuration_fingerprint: Option<String>,
    /// Broker-private digest schema. Missing means the historical v1 digest;
    /// it must never be upgraded by rehashing an existing Created checkpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_fingerprint_version: Option<u32>,
    pub observed_at: chrono::DateTime<chrono::Utc>,
    pub diagnostics: Vec<ObservationDiagnostic>,
}
impl ContainerObservation {
    fn fingerprint_version(&self) -> u32 {
        self.configuration_fingerprint_version.unwrap_or(1)
    }
    pub fn validate_identity_allocation(&self, created: &Self) -> Result<()> {
        ensure!(
            self.fingerprint_version() == created.fingerprint_version()
                && (created.configuration_fingerprint.is_none()
                    || self.configuration_fingerprint == created.configuration_fingerprint),
            "runtime sandbox configuration is missing or changed"
        );
        ensure!(
            self.id.is_some() && self.id == created.id,
            "runtime instance identity is missing or changed"
        );
        ensure!(
            !self.name.is_empty() && self.name == created.name,
            "runtime instance name is missing or changed"
        );
        ensure!(
            self.image_id.is_some() && self.image_id == created.image_id,
            "runtime immutable image identity is missing or changed"
        );
        ensure!(
            self.restart_count == Some(0) && created.restart_count == Some(0),
            "runtime restart history is unknown or nonzero"
        );
        ensure!(
            self.allocation.cpu.is_some()
                && self.allocation.memory_bytes.is_some()
                && self.allocation.pids_limit.is_some()
                && self.allocation == created.allocation,
            "runtime allocation is incomplete or changed"
        );
        Ok(())
    }
    /// A terminal allocation requires complete, unchanged identity and limits.
    /// Runtime elapsed wall time is intentionally not used for this proof.
    pub fn validate_terminal(&self, kind: RuntimeKind, created: &Self) -> Result<()> {
        self.validate_identity_allocation(created)?;
        let terminal = self.process.status.as_deref() == Some("exited")
            || kind == RuntimeKind::Podman && self.process.status.as_deref() == Some("stopped");
        ensure!(
            terminal && self.process.running == Some(false),
            "runtime did not prove a terminal process"
        );
        ensure!(
            self.process.exit_code.is_some(),
            "runtime terminal exit code is unknown"
        );
        let started = self
            .process
            .started_at
            .context("runtime process start is unknown")?;
        let finished = self
            .process
            .finished_at
            .context("runtime process finish is unknown")?;
        ensure!(
            finished >= started,
            "runtime terminal timestamps are inconsistent"
        );
        Ok(())
    }
}

// Only deserialize facts needed by the control contract. Unknown fields,
// including Config.Env and State.Error, are ignored before they can escape.
#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
struct InspectFacts {
    #[serde(alias = "ID")]
    id: Option<String>,
    name: Option<String>,
    image: Option<String>,
    restart_count: Option<i64>,
    #[serde(default)]
    state: InspectProcess,
    #[serde(default)]
    host_config: InspectAllocation,
}
#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
struct InspectProcess {
    status: Option<String>,
    running: Option<bool>,
    started_at: Option<String>,
    finished_at: Option<String>,
    exit_code: Option<i64>,
    #[serde(rename = "OOMKilled")]
    oom_killed: Option<bool>,
}
#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
struct InspectAllocation {
    #[serde(rename = "NanoCpus", alias = "NanoCPUs")]
    nano_cpus: Option<i64>,
    cpu_quota: Option<i64>,
    cpu_period: Option<i64>,
    memory: Option<i64>,
    pids_limit: Option<i64>,
}
fn diagnostic(diagnostics: &mut Vec<ObservationDiagnostic>, field: &str, issue: ObservationIssue) {
    diagnostics.push(ObservationDiagnostic {
        field: field.into(),
        issue,
    });
}
fn observed_positive(
    value: Option<i64>,
    field: &str,
    diagnostics: &mut Vec<ObservationDiagnostic>,
) -> Option<u64> {
    match value {
        Some(value) if value > 0 => Some(value as u64),
        Some(value) => {
            diagnostic(
                diagnostics,
                field,
                if value == 0 || value == -1 {
                    ObservationIssue::Unbounded
                } else {
                    ObservationIssue::Invalid
                },
            );
            None
        }
        None => {
            diagnostic(diagnostics, field, ObservationIssue::Missing);
            None
        }
    }
}
fn observed_date(
    value: Option<String>,
    field: &str,
    diagnostics: &mut Vec<ObservationDiagnostic>,
) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    match value {
        Some(value) => match chrono::DateTime::parse_from_rfc3339(&value) {
            Ok(date) if date.timestamp() > 0 => Some(date),
            _ => {
                diagnostic(diagnostics, field, ObservationIssue::Invalid);
                None
            }
        },
        None => {
            diagnostic(diagnostics, field, ObservationIssue::Missing);
            None
        }
    }
}
fn immutable_id(
    value: Option<String>,
    field: &str,
    diagnostics: &mut Vec<ObservationDiagnostic>,
) -> Option<String> {
    match value {
        Some(value) if valid_immutable_id(value.strip_prefix("sha256:").unwrap_or(&value)) => {
            Some(value)
        }
        Some(_) => {
            diagnostic(diagnostics, field, ObservationIssue::Invalid);
            None
        }
        None => {
            diagnostic(diagnostics, field, ObservationIssue::Missing);
            None
        }
    }
}
fn valid_immutable_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}
pub fn parse_container_observation(
    bytes: &[u8],
    kind: RuntimeKind,
) -> Result<ContainerObservation> {
    parse_container_observation_version(bytes, kind, 1)
}
fn parse_container_observation_version(
    bytes: &[u8],
    kind: RuntimeKind,
    fingerprint_version: u32,
) -> Result<ContainerObservation> {
    ensure!(
        matches!(fingerprint_version, 1..=3),
        "unsupported private runtime configuration fingerprint version"
    );
    ensure!(
        bytes.len() <= 256 * 1024,
        "runtime inspection exceeds its bound"
    );
    let facts: InspectFacts = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("runtime returned malformed inspection facts"))?;
    let mut diagnostics = Vec::new();
    let id = immutable_id(facts.id, "id", &mut diagnostics);
    let image_id = immutable_id(facts.image, "image_id", &mut diagnostics).map(|id| {
        if id.starts_with("sha256:") {
            id
        } else {
            format!("sha256:{id}")
        }
    });
    let name = facts
        .name
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_owned();
    if name.is_empty() {
        diagnostic(&mut diagnostics, "name", ObservationIssue::Missing);
    }
    let restart_count = match facts.restart_count {
        Some(n) if n >= 0 => Some(n as u64),
        Some(_) => {
            diagnostic(&mut diagnostics, "restart_count", ObservationIssue::Invalid);
            None
        }
        None => {
            diagnostic(&mut diagnostics, "restart_count", ObservationIssue::Missing);
            None
        }
    };
    let host = facts.host_config;
    let nano = host
        .nano_cpus
        .filter(|n| *n > 0)
        .map(|n| CpuQuota::reduced(n as u64, 1_000_000_000));
    let quota = match (host.cpu_quota, host.cpu_period) {
        (Some(q), Some(p)) if q > 0 && p > 0 => Some(CpuQuota::reduced(q as u64, p as u64)),
        _ => None,
    };
    let cpu = match (nano, quota) {
        (Some(n), Some(q)) if n != q => {
            diagnostic(&mut diagnostics, "cpu", ObservationIssue::Inconsistent);
            None
        }
        (Some(n), None)
            if host.cpu_quota.is_some_and(|q| q > 0) || host.cpu_period.is_some_and(|p| p < 0) =>
        {
            diagnostic(&mut diagnostics, "cpu", ObservationIssue::Inconsistent);
            let _ = n;
            None
        }
        (Some(n), _) => Some(n),
        (None, Some(q)) => Some(q),
        (None, None) => {
            diagnostic(
                &mut diagnostics,
                "cpu",
                if host.nano_cpus.is_none() && host.cpu_quota.is_none() {
                    ObservationIssue::Missing
                } else {
                    ObservationIssue::Unbounded
                },
            );
            None
        }
    };
    let allocation = ObservedAllocation {
        cpu,
        memory_bytes: observed_positive(host.memory, "memory_bytes", &mut diagnostics),
        pids_limit: observed_positive(host.pids_limit, "pids_limit", &mut diagnostics),
    };
    let state = facts.state;
    if state.running.is_none() {
        diagnostic(&mut diagnostics, "running", ObservationIssue::Missing);
    }
    let exit_code = match state.exit_code {
        Some(code) => match u32::try_from(code) {
            Ok(code) => Some(code),
            Err(_) => {
                diagnostic(&mut diagnostics, "exit_code", ObservationIssue::Invalid);
                None
            }
        },
        None => {
            diagnostic(&mut diagnostics, "exit_code", ObservationIssue::Missing);
            None
        }
    };
    let process = ProcessObservation {
        status: state.status,
        running: state.running,
        started_at: observed_date(state.started_at, "started_at", &mut diagnostics),
        finished_at: observed_date(state.finished_at, "finished_at", &mut diagnostics),
        exit_code,
        oom_killed: state.oom_killed,
    };
    if process
        .started_at
        .zip(process.finished_at)
        .is_some_and(|(start, finish)| finish < start)
    {
        diagnostic(
            &mut diagnostics,
            "finished_at",
            ObservationIssue::Inconsistent,
        );
    }
    Ok(ContainerObservation {
        id,
        name,
        image_id,
        restart_count,
        process,
        allocation,
        configuration_fingerprint: managed::configuration_fingerprint(
            bytes,
            fingerprint_version,
            kind,
        ),
        configuration_fingerprint_version: (fingerprint_version != 1)
            .then_some(fingerprint_version),
        observed_at: chrono::Utc::now(),
        diagnostics,
    })
}

fn named_volume_args(volume: &str, labels: &[(String, String)]) -> Result<Vec<String>> {
    validate_managed_name(volume)?;
    ensure!(labels.len() <= 16, "too many volume labels");
    let mut args = vec!["volume".into(), "create".into()];
    for (key, value) in labels {
        ensure!(
            !key.is_empty()
                && key.len() <= 128
                && key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                && value.len() <= 256
                && !value.chars().any(char::is_control),
            "invalid control volume label"
        );
        args.extend(["--label".into(), format!("{key}={value}")]);
    }
    args.push(volume.into());
    Ok(args)
}

fn parse_runtime_info(kind: RuntimeKind, bytes: &[u8], allow_rootful: bool) -> Result<RuntimeInfo> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).context("runtime info is not JSON")?;
    let rootless = match kind {
        RuntimeKind::Docker => value
            .get("SecurityOptions")
            .and_then(|v| v.as_array())
            .is_some_and(|a| {
                a.iter().any(|v| {
                    v.as_str()
                        .is_some_and(|s| s == "name=rootless" || s == "rootless")
                })
            }),
        RuntimeKind::Podman => value
            .pointer("/host/security/rootless")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    };
    if !rootless && !allow_rootful {
        bail!(
            "runtime does not report rootless mode; configure rootless {} or explicitly use --allow-rootful-runtime (Docker Desktop/VM isolation must be assessed separately)",
            kind
        );
    }
    let server_version = value
        .get("ServerVersion")
        .or_else(|| value.pointer("/version/Version"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    Ok(RuntimeInfo {
        runtime: kind.to_string(),
        rootless,
        server_version,
    })
}

#[cfg(test)]
mod capture_error_tests {
    use super::*;

    #[test]
    fn abandoned_before_claim_requires_non_nil_identities_and_a_final_revision() {
        use crate::broker::journal::RevisionRefV1;
        let operation = Uuid::from_u128(1);
        let arm = Uuid::from_u128(2);
        let final_revision = RevisionRefV1::new(7, &"a".repeat(64)).unwrap();
        for (operation, arm, reference) in [
            (Uuid::nil(), arm, final_revision.clone()),
            (operation, Uuid::nil(), final_revision.clone()),
            (
                operation,
                arm,
                RevisionRefV1::new(0, &"a".repeat(64)).unwrap(),
            ),
        ] {
            assert!(AbandonedBeforeClaim::new(operation, arm, reference).is_err());
        }
        let retirement = AbandonedBeforeClaim::new(operation, arm, final_revision).unwrap();
        assert_eq!(retirement.operation_id(), operation);
        assert_eq!(retirement.arm_id(), arm);
        assert_eq!(retirement.final_revision().revision(), 7);
        assert_eq!(retirement.final_revision().hash(), "a".repeat(64));
    }

    #[test]
    fn profile_guard_compares_complete_binding_before_consuming_existing_guard() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        struct Count(Arc<AtomicUsize>);
        impl BeforeSpawnGuard for Count {
            fn check_before_spawn(self: Box<Self>, target: &str) -> Result<()> {
                ensure!(target == "synthetic-cid");
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
        let count = Arc::new(AtomicUsize::new(0));
        let binding = launch_profile::DockerLaunchProfileBindingV1::new(
            &"1".repeat(64),
            Uuid::from_u128(1),
            &"2".repeat(64),
            "unix:///not-opened/docker.sock",
            &"3".repeat(64),
        )
        .unwrap();
        for (expected, prepared, ok) in [
            (None, None, true),
            (Some(binding.clone()), Some(binding.clone()), true),
            (None, Some(binding.clone()), false),
            (Some(binding.clone()), None, false),
        ] {
            let before = count.load(Ordering::SeqCst);
            let guard = Box::new(ProfileBoundSpawnGuard {
                inner: Box::new(Count(count.clone())),
                expected: expected.map(Box::new),
                prepared,
            });
            assert_eq!(guard.check_before_spawn("synthetic-cid").is_ok(), ok);
            assert_eq!(count.load(Ordering::SeqCst), before + usize::from(ok));
        }
        let mut other = binding.clone();
        other.parent_operation_id = Uuid::from_u128(4);
        let before = count.load(Ordering::SeqCst);
        let guard = Box::new(ProfileBoundSpawnGuard {
            inner: Box::new(Count(count.clone())),
            expected: Some(Box::new(binding)),
            prepared: Some(other),
        });
        assert!(guard.check_before_spawn("synthetic-cid").is_err());
        assert_eq!(count.load(Ordering::SeqCst), before);
    }

    #[tokio::test]
    async fn failed_control_capture_never_formats_raw_stderr() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("failure.rs");
        let binary = temporary.path().join("failure");
        std::fs::write(&source, r#"use std::io::Write;fn main(){std::io::stderr().write_all(&vec![b'!';4097*16]).unwrap();std::process::exit(7)}"#).unwrap();
        let mut compiler =
            Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
        compiler
            .args(["--edition=2024", "--crate-name", "capture_failure_fixture"])
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .kill_on_drop(true);
        let compiled = timeout(Duration::from_secs(30), compiler.output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            compiled.status.success(),
            "controlled Rust fixture must compile"
        );
        let mut failure = Runtime::with_program(RuntimeKind::Docker, &binary)
            .unwrap()
            .capture_cli(Command::new(binary), &[], Duration::from_secs(10), &[])
            .await
            .unwrap_err();
        assert!(
            !format!("{failure:#}").contains("!!!!"),
            "raw captured stderr must not enter the generic error chain"
        );
        assert!(
            !format!("{failure:?}").contains("!!!!"),
            "debug output must not expose captured bytes"
        );
        let captured = failure
            .downcast_ref::<RuntimeCommandFailure>()
            .unwrap()
            .stderr_capture();
        assert!(captured.truncated);
        assert_eq!(captured.bytes.len(), 64 * 1024);
        assert_eq!(captured.total_bytes, 4097 * 16);
        failure
            .downcast_mut::<RuntimeCommandFailure>()
            .unwrap()
            .add_private_redactions(vec!["!".repeat(4097)]);
        let redactor = crate::redaction::Redactor::new(vec![]);
        let safe = failure
            .downcast_ref::<RuntimeCommandFailure>()
            .unwrap()
            .redacted_stderr(&redactor);
        assert!(
            !safe.contains("!!!!"),
            "runtime-private values must join the caller dictionary before masking"
        );
        assert!(safe.contains("[REDACTED]"));
    }
}
