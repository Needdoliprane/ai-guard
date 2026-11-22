//! Worker-only bootstrap. Credentials never appear in provider argv.

use super::{
    AuthMode, BOOTSTRAP_ENV, BOOTSTRAP_PROGRAM, ClientKind, ClientTransport,
    credentials::{BootstrapPayload, codex_auth, validate_secret},
    selection::parse_bootstrap_argv,
};
use crate::control_fs::Directory;
use anyhow::{Result, anyhow, bail, ensure};
use std::{
    collections::BTreeMap,
    ffi::{CString, OsStr, OsString},
    fmt,
    fs::File,
    io::Write,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

pub struct BootstrapExecution {
    program: &'static str,
    arguments: Vec<String>,
    environment: BTreeMap<String, String>,
    api_login: Option<String>,
    home: Directory,
    state: Directory,
    transport: ClientTransport,
    client: ClientKind,
}

impl fmt::Debug for BootstrapExecution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BootstrapExecution(<private environment>)")
    }
}

impl BootstrapExecution {
    pub fn program(&self) -> &'static str {
        self.program
    }
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }
    pub fn environment(&self) -> &BTreeMap<String, String> {
        &self.environment
    }
    pub fn needs_api_login(&self) -> bool {
        self.api_login.is_some()
    }
}

/// Compile a request into an exclusive environment and a private tmpfs home.
/// This prepares authentication material, but never runs a provider or model.
pub fn prepare_execution(
    argv: &[String],
    payload: &str,
    home: &Path,
) -> Result<BootstrapExecution> {
    let selected = parse_bootstrap_argv(argv)?;
    ensure!(
        payload.len() <= 64 * 1024,
        "invalid provider bootstrap credential size"
    );
    let payload: BootstrapPayload = serde_json::from_str(payload)
        .map_err(|_| anyhow!("invalid provider bootstrap credential"))?;
    ensure!(
        payload.version == 1
            && payload.client == selected.client
            && payload.auth_mode == selected.auth_mode,
        "provider bootstrap credential does not match the authorized client or authentication mode"
    );
    if selected.client == ClientKind::Codex && selected.auth_mode == AuthMode::Subscription {
        let _ = codex_auth(&payload.secret)?;
    } else {
        validate_secret(&payload.secret)?;
    }
    let home_path = crate::control_fs::absolute_path(home)?;
    ensure!(
        !home_path.starts_with("/workspace"),
        "provider authentication home must be outside the workspace"
    );
    let home = Directory::open_existing(&home_path).map_err(|_| {
        anyhow!("credential_unsafe: provider home must be an existing private owned directory")
    })?;
    let state = home.child(format!("provider-auth-{}", uuid::Uuid::new_v4()))?;
    let home_text = home
        .path()
        .to_str()
        .ok_or_else(|| anyhow!("provider home requires UTF-8"))?
        .to_owned();
    let mut environment = BTreeMap::from([
        ("HOME".into(), home_text.clone()),
        ("LANG".into(), "C.UTF-8".into()),
        (
            "PATH".into(),
            "/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
                .into(),
        ),
        ("CARGO_HOME".into(), format!("{home_text}/.cargo")),
        ("RUSTUP_HOME".into(), "/usr/local/rustup".into()),
        ("TMPDIR".into(), "/tmp".into()),
    ]);
    if selected.transport.is_interactive() {
        environment.insert("TERM".into(), "xterm-256color".into());
    }
    let program;
    let mut arguments = Vec::new();
    let mut api_login = None;
    match selected.client {
        ClientKind::Codex => {
            program = "/usr/local/bin/codex";
            environment.insert(
                "CODEX_HOME".into(),
                state
                    .path()
                    .to_str()
                    .ok_or_else(|| anyhow!("provider state requires UTF-8"))?
                    .into(),
            );
            if selected.auth_mode == AuthMode::Subscription {
                let (auth, _) = codex_auth(&payload.secret)?;
                write_new_private(&state, "auth.json", auth.as_bytes())?;
            } else if selected.transport == ClientTransport::InteractiveTty {
                api_login = Some(payload.secret);
            } else {
                environment.insert("CODEX_API_KEY".into(), payload.secret);
            }
            if selected.transport == ClientTransport::Batch {
                arguments.extend(
                    ["exec", "--json", "--skip-git-repo-check", "--ephemeral"]
                        .into_iter()
                        .map(str::to_owned),
                );
            }
            arguments.extend([
                "--sandbox".into(),
                "danger-full-access".into(),
                "-c".into(),
                "approval_policy=\"never\"".into(),
                "-c".into(),
                format!(
                    "forced_login_method=\"{}\"",
                    if selected.auth_mode == AuthMode::Api {
                        "api"
                    } else {
                        "chatgpt"
                    }
                ),
                "-c".into(),
                "cli_auth_credentials_store=\"file\"".into(),
            ]);
            // Official CLI overrides outrank project config. Provider routing
            // keys are also ignored by Codex in project-scoped config layers.
            // The built-in `openai` provider cannot be redefined as a custom one.
            arguments.extend(codex_route_arguments());
        }
        ClientKind::ClaudeCode => {
            program = "/usr/local/bin/claude";
            environment.insert(
                "CLAUDE_CONFIG_DIR".into(),
                state
                    .path()
                    .to_str()
                    .ok_or_else(|| anyhow!("provider state requires UTF-8"))?
                    .into(),
            );
            environment.insert(
                if selected.auth_mode == AuthMode::Api {
                    "ANTHROPIC_API_KEY"
                } else {
                    "CLAUDE_CODE_OAUTH_TOKEN"
                }
                .into(),
                payload.secret,
            );
            // Project/local settings can inject API keys or apiKeyHelper. Only
            // the new empty user configuration is eligible for this preset.
            arguments.extend(["--setting-sources", "user"].into_iter().map(str::to_owned));
            if selected.transport == ClientTransport::Batch {
                arguments.extend(
                    [
                        "--print",
                        "--output-format",
                        "stream-json",
                        "--verbose",
                        "--no-session-persistence",
                        "--permission-mode",
                        "acceptEdits",
                    ]
                    .into_iter()
                    .map(str::to_owned),
                );
            }
        }
    }
    if let Some(model) = selected.model {
        arguments.extend(["--model".into(), model]);
    }
    if let Some(prompt) = selected.prompt {
        arguments.extend(["--".into(), prompt]);
    }
    home.revalidate()?;
    state.revalidate()?;
    Ok(BootstrapExecution {
        program,
        arguments,
        environment,
        api_login,
        home,
        state,
        transport: selected.transport,
        client: selected.client,
    })
}

fn write_new_private(directory: &Directory, name: &str, bytes: &[u8]) -> Result<()> {
    directory.revalidate()?;
    let parent = directory.try_clone_file()?;
    let name =
        CString::new(name).map_err(|_| anyhow!("invalid provider authentication filename"))?;
    // SAFETY: retained checked private directory, a fixed filename, and an
    // exclusive no-follow creation. The resulting FD is immediately owned.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    ensure!(
        fd >= 0,
        "provider authentication file could not be created privately"
    );
    // SAFETY: successful openat transfers ownership of one live FD.
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(bytes)
        .map_err(|_| anyhow!("provider authentication file could not be written"))?;
    file.sync_all()
        .map_err(|_| anyhow!("provider authentication file could not be synchronized"))?;
    directory.revalidate()?;
    Ok(())
}

const PROBE_LIMIT: usize = 64 * 1024;

fn codex_route_arguments() -> Vec<String> {
    [
        "-c",
        "model_provider=\"openai\"",
        "-c",
        "openai_base_url=\"https://api.openai.com/v1\"",
        "-c",
        "chatgpt_base_url=\"https://chatgpt.com/backend-api/\"",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

async fn capture<R: AsyncRead + Unpin>(stream: R) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    stream
        .take((PROBE_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| anyhow!("provider probe output is unavailable"))?;
    ensure!(
        bytes.len() <= PROBE_LIMIT,
        "version_incompatible: provider probe output exceeds its bound"
    );
    Ok(bytes)
}

fn terminate_group(id: Option<u32>) {
    if let Some(id) = id.and_then(|id| i32::try_from(id).ok()) {
        // SAFETY: every probe/login is created in its own process group. A
        // negative child PID addresses that group, never the worker's group.
        unsafe {
            libc::kill(-id, libc::SIGKILL);
        }
    }
}

async fn bounded_output(
    program: &Path,
    args: &[String],
    environment: &BTreeMap<String, String>,
    cwd: &Path,
    limit: Duration,
) -> Result<Vec<u8>> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .env_clear()
        .envs(environment)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|_| anyhow!("client_missing: provider executable is absent or cannot start"))?;
    let id = child.id();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("provider probe stdout is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("provider probe stderr is unavailable"))?;
    let result = tokio::time::timeout(limit, async {
        let (status, out, err) = tokio::join!(child.wait(), capture(stdout), capture(stderr));
        let status = status.map_err(|_| anyhow!("provider probe status is unavailable"))?;
        let out = out?;
        let _ = err?;
        ensure!(
            status.success(),
            "version_incompatible: installed provider probe refused the requested capability"
        );
        Ok::<_, anyhow::Error>(out)
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) => {
            terminate_group(id);
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
            bail!("probe_failed: provider capability probe timed out")
        }
    }
}

impl BootstrapExecution {
    fn probe_environment(&self) -> BTreeMap<String, String> {
        let mut environment = self.environment.clone();
        for name in [
            "CODEX_API_KEY",
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ] {
            environment.remove(name);
        }
        environment
    }

    async fn probe(&self) -> Result<()> {
        let environment = self.probe_environment();
        let version = bounded_output(
            Path::new(self.program),
            &["--version".into()],
            &environment,
            self.home.path(),
            Duration::from_secs(5),
        )
        .await?;
        let version = std::str::from_utf8(&version)
            .map_err(|_| anyhow!("version_incompatible: provider version is not UTF-8"))?;
        let pattern = match self.client {
            ClientKind::Codex => r"^codex-cli [0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?\s*$",
            ClientKind::ClaudeCode => {
                r"^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)? \(Claude Code\)\s*$"
            }
        };
        ensure!(
            version.len() <= 128 && regex::Regex::new(pattern)?.is_match(version),
            "version_incompatible: provider version format is unsupported"
        );
        let args = if self.client == ClientKind::Codex && self.transport == ClientTransport::Batch {
            vec!["exec".into(), "--help".into()]
        } else {
            vec!["--help".into()]
        };
        let help = bounded_output(
            Path::new(self.program),
            &args,
            &environment,
            self.home.path(),
            Duration::from_secs(5),
        )
        .await?;
        let help = std::str::from_utf8(&help)
            .map_err(|_| anyhow!("version_incompatible: provider help is not UTF-8"))?;
        let required: &[&str] = match (self.client, self.transport) {
            (ClientKind::Codex, ClientTransport::Batch) => &[
                "--json",
                "--sandbox",
                "--skip-git-repo-check",
                "--ephemeral",
                "--config",
            ],
            (ClientKind::Codex, _) => &["--sandbox", "--config"],
            (ClientKind::ClaudeCode, ClientTransport::Batch) => &[
                "--setting-sources",
                "--print",
                "--output-format",
                "--verbose",
                "--no-session-persistence",
                "--permission-mode",
            ],
            (ClientKind::ClaudeCode, _) => &["--setting-sources"],
        };
        ensure!(
            required.iter().all(|flag| help.contains(flag)),
            "version_incompatible: installed provider lacks a required invocation capability"
        );
        if self.api_login.is_some() {
            let help = bounded_output(
                Path::new(self.program),
                &["login".into(), "--help".into()],
                &environment,
                self.home.path(),
                Duration::from_secs(5),
            )
            .await?;
            ensure!(
                String::from_utf8_lossy(&help).contains("--with-api-key"),
                "version_incompatible: installed Codex does not support private API login"
            );
        }
        Ok(())
    }

    async fn login(&self) -> Result<()> {
        let Some(secret) = &self.api_login else {
            return Ok(());
        };
        self.state.revalidate()?;
        let mut command = tokio::process::Command::new(self.program);
        command
            .args([
                "login",
                "--with-api-key",
                "-c",
                "cli_auth_credentials_store=\"file\"",
                "-c",
                "forced_login_method=\"api\"",
            ])
            .args(codex_route_arguments())
            .env_clear()
            .envs(self.probe_environment())
            .current_dir(self.home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = command.spawn().map_err(|_| {
            anyhow!("client_missing: provider executable cannot start for authentication")
        })?;
        let id = child.id();
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("provider private authentication input is unavailable"))?;
        let result = tokio::time::timeout(Duration::from_secs(15), async {
            let send = async {
                stdin
                    .write_all(secret.as_bytes())
                    .await
                    .map_err(|_| anyhow!("provider private authentication input failed"))?;
                stdin
                    .write_all(b"\n")
                    .await
                    .map_err(|_| anyhow!("provider private authentication input failed"))?;
                stdin
                    .shutdown()
                    .await
                    .map_err(|_| anyhow!("provider private authentication input failed"))?;
                drop(stdin);
                Ok::<_, anyhow::Error>(())
            };
            let (sent, status) = tokio::join!(send, child.wait());
            sent?;
            ensure!(
                status
                    .map_err(|_| anyhow!("provider authentication status is unavailable"))?
                    .success(),
                "login_required: Codex API authentication failed"
            );
            Ok::<_, anyhow::Error>(())
        })
        .await;
        match result {
            Ok(result) => result?,
            Err(_) => {
                terminate_group(id);
                let _ = child.start_kill();
                let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
                bail!("login_required: provider authentication timed out");
            }
        }
        self.state
            .read_private_file("auth.json", 64 * 1024)
            .map_err(|_| {
                anyhow!("credential_unsafe: Codex did not create a private authentication file")
            })?;
        Ok(())
    }
}

/// Standalone worker entry point: read the private handoff, validate the request,
/// then replace the current process with the provider on success.
///
/// This does not mutate the caller's process environment. Every probe, login and
/// final exec receives a reconstructed environment without the bootstrap entry.
/// The inherited entry remains in this process until replacement; no memory or
/// `/proc` scrubbing guarantee is implied.
pub fn run_from_environment(args: &[String]) -> Result<i32> {
    let payload = standalone_handoff(
        std::env::var_os(crate::egress::proxy_env::EGRESS_PROXY_MARKER).as_deref(),
        || std::env::var_os(BOOTSTRAP_ENV),
    )?;
    let mut argv = vec![BOOTSTRAP_PROGRAM.into()];
    argv.extend_from_slice(args);
    let execution = prepare_execution(&argv, &payload, Path::new("/home/agent"))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| anyhow!("provider bootstrap runtime is unavailable"))?;
    runtime.block_on(async {
        execution.probe().await?;
        execution.login().await
    })?;
    drop(runtime);
    execution.home.revalidate()?;
    execution.state.revalidate()?;
    let mut command = std::process::Command::new(execution.program);
    command
        .args(&execution.arguments)
        .env_clear()
        .envs(&execution.environment)
        .current_dir("/workspace");
    let _error = command.exec();
    bail!("client_missing: provider executable could not replace the bootstrap process")
}

fn standalone_handoff(
    egress_marker: Option<&OsStr>,
    read_handoff: impl FnOnce() -> Option<OsString>,
) -> Result<String> {
    // The helper binds its listener before the broker has the opening receipt.
    // This entry has no broker release signal after that durable ACK.
    ensure!(
        egress_marker.is_none(),
        "egress_transport_unavailable: the broker's durable channel opening has not released the provider"
    );
    read_handoff()
        .ok_or_else(|| anyhow!("credential_missing: provider bootstrap credential is unavailable"))?
        .into_string()
        .map_err(|_| anyhow!("invalid provider bootstrap credential encoding"))
}

impl BootstrapExecution {
    /// Resolved transport of the prepared request.
    pub fn transport(&self) -> ClientTransport {
        self.transport
    }

    /// Kubernetes session protocol v3 entry (G11 K3): the payload was given
    /// in memory to `prepare_execution` by the in-Pod supervisor, never read
    /// from the environment. This runs the same capability probe and private
    /// login as `run_from_environment`, then **spawns** the provider as a
    /// child instead of replacing the caller, so that the supervisor stays
    /// PID 1. Standard streams are piped, the child gets its own process
    /// group and is killed if its handle is dropped.
    ///
    /// The child environment is exactly the prepared one plus `extra`
    /// (the loopback egress proxy variables); a name already prepared is
    /// refused rather than overridden. The prepared environment, which holds
    /// the credential, is consumed and its values overwritten after the
    /// spawn; copies kept by the operating system for the child, and the
    /// child's own memory, are not covered.
    pub async fn spawn_child(
        mut self,
        workspace: &Path,
        extra: &BTreeMap<String, String>,
    ) -> Result<tokio::process::Child> {
        ensure!(
            extra
                .keys()
                .all(|name| !self.environment.contains_key(name)),
            "invalid provider bootstrap request: environment collision"
        );
        self.probe().await?;
        self.login().await?;
        self.home.revalidate()?;
        self.state.revalidate()?;
        let mut command = tokio::process::Command::new(self.program);
        command
            .args(&self.arguments)
            .env_clear()
            .envs(&self.environment)
            .envs(extra)
            .current_dir(workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let spawned = command.spawn();
        drop(command);
        for value in self.environment.values_mut() {
            overwrite(value);
        }
        if let Some(secret) = self.api_login.as_mut() {
            overwrite(secret);
        }
        spawned
            .map_err(|_| anyhow!("client_missing: provider executable is absent or cannot start"))
    }
}

/// Best-effort overwrite of a private value before it is dropped.
fn overwrite(value: &mut String) {
    // SAFETY: zero bytes keep the string valid UTF-8; each write targets an
    // exclusively borrowed byte.
    for byte in unsafe { value.as_mut_vec() }.iter_mut() {
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// Fixed diagnostics only: captured help/login output and payload are private.
pub fn public_error(error: &anyhow::Error) -> &'static str {
    let text = error.to_string();
    if text.starts_with("client_missing:") {
        "client_missing: install the configured provider CLI in the selected worker image"
    } else if text.starts_with("version_incompatible:") {
        "version_incompatible: installed provider lacks a required capability; select a compatible image"
    } else if text.starts_with("login_required:") {
        "login_required: prepare a valid credential for the requested provider authentication mode"
    } else if text.starts_with("credential_missing:") {
        "credential_missing: the private provider handoff is unavailable"
    } else if text.starts_with("credential_unsafe:") {
        "credential_unsafe: provider authentication storage is not private or has changed"
    } else if text.starts_with("probe_failed:") {
        "probe_failed: provider capability probe timed out; no model command was started"
    } else if text.starts_with("egress_transport_unavailable:") {
        "egress_transport_unavailable: the broker egress channel is not ready; no provider was started and no other network path is used"
    } else {
        "invalid provider bootstrap request; prepare a new authorized request"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn marked_standalone_provider_requires_broker_release_before_the_handoff() {
        for marker in ["http://127.0.0.1:3128", "invalid"] {
            let error = standalone_handoff(Some(OsStr::new(marker)), || {
                panic!("the standalone provider must not read its handoff before release");
            })
            .unwrap_err();
            assert!(public_error(&error).starts_with("egress_transport_unavailable:"));
        }
        let error = standalone_handoff(None, || None).unwrap_err();
        assert!(public_error(&error).starts_with("credential_missing:"));
    }

    #[test]
    #[ignore = "internal native subprocess fixture, invoked only by the bounded probe tests"]
    fn bounded_probe_fixture() {
        match std::env::var("AI_GUARD_NATIVE_FIXTURE_MODE").as_deref() {
            Ok("sleep") => {
                println!("canary-private-probe-capture-99");
                std::thread::sleep(Duration::from_secs(3));
            }
            Ok("overflow") => {
                let text = "canary-private-probe-capture-99".repeat(4096);
                std::io::stdout().write_all(text.as_bytes()).ok();
            }
            _ => panic!("native fixture mode required"),
        }
    }

    async fn fixture(mode: &str, limit: Duration) -> Result<Vec<u8>> {
        let program = std::env::current_exe().unwrap();
        let arguments = [
            "--exact",
            "provider::bootstrap::tests::bounded_probe_fixture",
            "--ignored",
            "--nocapture",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        let environment = BTreeMap::from([("AI_GUARD_NATIVE_FIXTURE_MODE".into(), mode.into())]);
        bounded_output(
            &program,
            &arguments,
            &environment,
            program.parent().unwrap(),
            limit,
        )
        .await
    }

    #[tokio::test]
    async fn probe_timeout_is_bounded_and_never_exposes_captured_output() {
        let started = Instant::now();
        let error = fixture("sleep", Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(error.to_string().contains("probe_failed"));
        assert!(!format!("{error:#}").contains("canary-private-probe-capture-99"));
    }

    #[tokio::test]
    async fn probe_capture_is_bounded_and_never_exposes_overflow_bytes() {
        let error = fixture("overflow", Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("exceeds its bound"));
        assert!(!format!("{error:#}").contains("canary-private-probe-capture-99"));
    }
}
