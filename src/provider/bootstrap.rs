//! Worker-only bootstrap. Credentials never appear in provider argv.

use super::{
    AuthMode, BOOTSTRAP_ENV, BOOTSTRAP_PROGRAM, ClientKind, ClientTransport,
    credentials::{BootstrapPayload, codex_auth, validate_secret},
    selection::parse_bootstrap_argv,
};
use crate::{
    control_fs::Directory,
    egress::{bootstrap_release, ids::IdV1, proxy_env},
};
use anyhow::{Result, anyhow, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::{CString, OsStr, OsString},
    fmt,
    fs::File,
    io::{IsTerminal, Read, Write},
    os::{
        fd::{AsFd, AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt, process::CommandExt},
    },
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

pub struct BootstrapExecution {
    program: &'static str,
    arguments: Vec<String>,
    /// Index of the first model/prompt argument; probed options go before it.
    options_end: usize,
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
    let options_end = arguments.len();
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
        options_end,
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
/// Claude Code batch option that forwards subagent text to the stream, used
/// only when the installed client's help lists it.
const FORWARD_SUBAGENT_TEXT: &str = "--forward-subagent-text";

/// Optional capabilities read from the installed client's help.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ProbeFacts {
    forward_subagent_text: bool,
}

/// Recognize an option entry before its description, including a short alias.
fn help_lists_option(help: &str, flag: &str) -> bool {
    // Keep line endings so a bare option name at EOF is ambiguous.
    help.split_inclusive('\n').any(|line| {
        let entry = line.trim_start();
        // Option columns fit within six spaces. Wrapped descriptions sit deeper.
        if line.len() - entry.len() > 6 {
            return false;
        }
        let entry = match entry.as_bytes() {
            [b'-', short, b',', ..] if short.is_ascii_alphanumeric() => entry[3..].trim_start(),
            _ => entry,
        };
        entry.strip_prefix(flag).is_some_and(|suffix| {
            suffix.starts_with([' ', '\t', ',', '=', '\n']) || suffix.starts_with("\r\n")
        })
    })
}

/// Require the existing invocation flags and report optional batch capabilities.
fn help_facts(client: ClientKind, transport: ClientTransport, help: &str) -> Result<ProbeFacts> {
    let required: &[&str] = match (client, transport) {
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
    Ok(ProbeFacts {
        forward_subagent_text: client == ClientKind::ClaudeCode
            && transport == ClientTransport::Batch
            && help_lists_option(help, FORWARD_SUBAGENT_TEXT),
    })
}

/// SHA-256 of the managed hook configurations shipped in
/// `images/provider-activity/`; the worker image installs exactly one of them
/// per client, read-only. A unit test recomputes them from those files, so the
/// image build stages that compile this library need no extra input.
const CLAUDE_ACTIVITY_V1_SHA256: &str =
    "d48e956269d67e48d3fab8bf3cc9f9310353230a85e7db8ff2189ed93d0ac7f3";
const CLAUDE_ACTIVITY_V1_ONFAILURE_SHA256: &str =
    "5cbd734bbac216717bf8a6afcf3a899207ee890dcc33e15ceb52ceabf8f3b308";
const CODEX_ACTIVITY_V1_SHA256: &str =
    "4522907b33691f430fe5cca3cc16547acb9bf4a7cbe1a286db43e9425714b1a7";
const CLAUDE_MANAGED_SETTINGS: &str = "/etc/claude-code/managed-settings.json";
const CODEX_REQUIREMENTS: &str = "/etc/codex/requirements.toml";
const MANAGED_CONFIG_LIMIT: u64 = 64 * 1024;
const ACTIVITY_REFUSAL: &str = "activity_unavailable: the provider activity hook configuration is absent, not a root-owned read-only file, or not an expected version";

fn managed_activity_config(client: ClientKind) -> (&'static Path, &'static [&'static str]) {
    match client {
        ClientKind::ClaudeCode => (
            Path::new(CLAUDE_MANAGED_SETTINGS),
            &[
                CLAUDE_ACTIVITY_V1_SHA256,
                CLAUDE_ACTIVITY_V1_ONFAILURE_SHA256,
            ],
        ),
        ClientKind::Codex => (Path::new(CODEX_REQUIREMENTS), &[CODEX_ACTIVITY_V1_SHA256]),
    }
}

fn activity_check(condition: bool) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(anyhow!(ACTIVITY_REFUSAL))
    }
}

/// The installed managed hook configuration must be a regular file owned by
/// `owner`, without any write bit, in an `owner`-owned directory that group
/// and others cannot write, and byte-identical to one expected version. The
/// bytes are read from the same no-follow descriptor whose metadata is checked.
fn verify_managed_activity_config(path: &Path, expected: &[&str], owner: u32) -> Result<()> {
    let parent = path.parent().ok_or_else(|| anyhow!(ACTIVITY_REFUSAL))?;
    let directory = std::fs::symlink_metadata(parent).map_err(|_| anyhow!(ACTIVITY_REFUSAL))?;
    activity_check(
        directory.file_type().is_dir() && directory.uid() == owner && directory.mode() & 0o022 == 0,
    )?;
    let name = CString::new(path.as_os_str().as_bytes()).map_err(|_| anyhow!(ACTIVITY_REFUSAL))?;
    // SAFETY: a NUL-terminated absolute path; O_NOFOLLOW refuses a final
    // symlink and O_NONBLOCK keeps a FIFO from blocking the bootstrap.
    let fd = unsafe {
        libc::open(
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOCTTY,
        )
    };
    activity_check(fd >= 0)?;
    // SAFETY: a successful open transfers ownership of one live descriptor.
    let file = unsafe { File::from_raw_fd(fd) };
    let facts = file.metadata().map_err(|_| anyhow!(ACTIVITY_REFUSAL))?;
    activity_check(
        facts.file_type().is_file()
            && facts.uid() == owner
            && facts.mode() & 0o222 == 0
            && facts.len() <= MANAGED_CONFIG_LIMIT,
    )?;
    let mut bytes = Vec::new();
    file.take(MANAGED_CONFIG_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!(ACTIVITY_REFUSAL))?;
    let digest = hex::encode(Sha256::digest(&bytes));
    activity_check(
        bytes.len() as u64 <= MANAGED_CONFIG_LIMIT
            && expected.iter().any(|candidate| *candidate == digest),
    )
}

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

    async fn probe(&self) -> Result<ProbeFacts> {
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
        let facts = help_facts(self.client, self.transport, help)?;
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
        Ok(facts)
    }

    /// Add options that the help probe showed this installed client supports.
    fn apply_probe_facts(&mut self, facts: ProbeFacts) {
        if facts.forward_subagent_text {
            self.arguments
                .insert(self.options_end, FORWARD_SUBAGENT_TEXT.into());
            self.options_end += 1;
        }
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
    let mut argv = vec![BOOTSTRAP_PROGRAM.into()];
    argv.extend_from_slice(args);
    let (payload, proxy_port) = standalone_handoff(
        &argv,
        std::env::var_os(proxy_env::EGRESS_PROXY_MARKER).as_deref(),
        std::env::var_os(bootstrap_release::HELD_CHALLENGE_ENV).as_deref(),
        read_pid1_release,
        || std::env::var_os(BOOTSTRAP_ENV),
    )?;
    let mut execution = prepare_execution(&argv, &payload, Path::new("/home/agent"))?;
    if let Some(port) = proxy_port {
        execution.environment.extend(
            proxy_env::proxy_environment(port)
                .into_iter()
                .map(|(name, value)| (name.into(), value)),
        );
    }
    // No provider command (not even a probe) runs without the managed hooks
    // that feed the activity channel.
    let (managed, expected) = managed_activity_config(execution.client);
    verify_managed_activity_config(managed, expected, 0)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| anyhow!("provider bootstrap runtime is unavailable"))?;
    let facts = runtime.block_on(async {
        let facts = execution.probe().await?;
        execution.login().await?;
        Ok::<_, anyhow::Error>(facts)
    })?;
    drop(runtime);
    execution.apply_probe_facts(facts);
    verify_managed_activity_config(managed, expected, 0)?;
    execution.home.revalidate()?;
    execution.state.revalidate()?;
    let mut command = std::process::Command::new(execution.program);
    command
        .args(&execution.arguments)
        .env_clear()
        .envs(&execution.environment)
        .current_dir("/workspace");
    if proxy_port.is_some() {
        command.stdin(Stdio::null());
    }
    let _error = command.exec();
    bail!("client_missing: provider executable could not replace the bootstrap process")
}

const RELEASE_UNAVAILABLE: &str =
    "egress_transport_unavailable: the held provider release input is missing or invalid";

fn read_pid1_release(challenge: &IdV1) -> Result<()> {
    let stdin = std::io::stdin();
    ensure!(
        std::process::id() == 1 && !stdin.is_terminal(),
        "{RELEASE_UNAVAILABLE}"
    );
    // An unbuffered read keeps input consumption at the frame plus one byte.
    let input = stdin
        .as_fd()
        .try_clone_to_owned()
        .map_err(|_| anyhow!("{RELEASE_UNAVAILABLE}"))?;
    read_release_frame(File::from(input), challenge)
}

fn read_release_frame(input: impl Read, challenge: &IdV1) -> Result<()> {
    ensure!(
        bootstrap_release::receive_frame(input, challenge),
        "{RELEASE_UNAVAILABLE}"
    );
    Ok(())
}

fn standalone_handoff(
    argv: &[String],
    egress_marker: Option<&OsStr>,
    held_challenge: Option<&OsStr>,
    read_release: impl FnOnce(&IdV1) -> Result<()>,
    read_handoff: impl FnOnce() -> Option<OsString>,
) -> Result<(String, Option<u16>)> {
    let proxy_port = match (egress_marker, held_challenge) {
        (None, None) => None,
        (Some(marker), Some(challenge)) => {
            let port = marker
                .to_str()
                .and_then(proxy_env::parse_marker)
                .ok_or_else(|| anyhow!("{RELEASE_UNAVAILABLE}"))?;
            let challenge = challenge
                .to_str()
                .and_then(|value| IdV1::parse(value).ok())
                .ok_or_else(|| anyhow!("{RELEASE_UNAVAILABLE}"))?;
            let selected =
                parse_bootstrap_argv(argv).map_err(|_| anyhow!("{RELEASE_UNAVAILABLE}"))?;
            ensure!(
                selected.transport == ClientTransport::Batch,
                "{RELEASE_UNAVAILABLE}"
            );
            read_release(&challenge)?;
            Some(port)
        }
        _ => bail!("{RELEASE_UNAVAILABLE}"),
    };
    let payload = read_handoff()
        .ok_or_else(|| anyhow!("credential_missing: provider bootstrap credential is unavailable"))?
        .into_string()
        .map_err(|_| anyhow!("invalid provider bootstrap credential encoding"))?;
    Ok((payload, proxy_port))
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
    } else if text.starts_with("activity_unavailable:") {
        "activity_unavailable: the worker image lacks the expected provider activity hooks; build the worker image from this release"
    } else {
        "invalid provider bootstrap request; prepare a new authorized request"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use std::{cell::Cell, io};

    #[test]
    fn marked_standalone_provider_requires_broker_release_before_the_handoff() {
        for marker in ["http://127.0.0.1:3128", "invalid"] {
            let error = standalone_handoff(
                &[],
                Some(OsStr::new(marker)),
                None,
                |_| panic!("the legacy marker must not consume stdin"),
                || panic!("the legacy marker must not read credentials"),
            )
            .unwrap_err();
            assert!(public_error(&error).starts_with("egress_transport_unavailable:"));
        }
        let error = standalone_handoff(
            &[],
            None,
            None,
            |_| panic!("the unmarked bootstrap must not consume stdin"),
            || None,
        )
        .unwrap_err();
        assert!(public_error(&error).starts_with("credential_missing:"));
        let (payload, port) = standalone_handoff(
            &[],
            None,
            None,
            |_| panic!("the unmarked bootstrap must not consume stdin"),
            || Some("synthetic handoff".into()),
        )
        .unwrap();
        assert_eq!(payload, "synthetic handoff");
        assert_eq!(port, None);
    }

    #[test]
    fn held_provider_releases_credentials_only_after_the_exact_frame_and_eof() {
        const CHALLENGE: &str = "11111111-1111-4111-8111-111111111111";
        const FRAME: &[u8] = b"AI-GUARD-EGRESS-RELEASE-V1 11111111-1111-4111-8111-111111111111\n";
        let argv = [
            BOOTSTRAP_PROGRAM,
            "exec",
            "--client",
            "codex",
            "--auth-mode",
            "api",
            "--transport",
            "batch",
            "--preset-version",
            "1",
            "--prompt",
            "synthetic",
        ]
        .map(str::to_owned);
        let marker = Some(OsStr::new("http://127.0.0.1:3128"));
        let held = Some(OsStr::new(CHALLENGE));
        let eof = Cell::new(false);
        let credentials = Cell::new(0);
        let input = io::Read::chain(io::Read::chain(&FRAME[..9], &FRAME[9..]), EofWitness(&eof));
        let (payload, port) = standalone_handoff(
            &argv,
            marker,
            held,
            |challenge| {
                assert_eq!(bootstrap_release::encode_frame(challenge).as_slice(), FRAME);
                read_release_frame(input, challenge)
            },
            || {
                assert!(
                    eof.get(),
                    "credentials were requested before the end of stdin"
                );
                credentials.set(credentials.get() + 1);
                Some("synthetic handoff".into())
            },
        )
        .unwrap();
        assert_eq!(credentials.get(), 1);
        assert_eq!(payload, "synthetic handoff");
        assert_eq!(port, Some(3128));

        let repeated = [FRAME, FRAME].concat();
        let wrong = b"AI-GUARD-EGRESS-RELEASE-V1 22222222-2222-4222-8222-222222222222\n";
        let mut unreadable = FRAME.to_vec();
        unreadable[FRAME.len() - 2] = 0xff;
        for input in [
            &[][..],
            &FRAME[..FRAME.len() - 1],
            repeated.as_slice(),
            wrong,
            unreadable.as_slice(),
        ] {
            let error = standalone_handoff(
                &argv,
                marker,
                held,
                |challenge| read_release_frame(input, challenge),
                || panic!("invalid release must not read credentials"),
            )
            .unwrap_err();
            assert!(public_error(&error).starts_with("egress_transport_unavailable:"));
        }
        let mut extra = io::Cursor::new([FRAME, &[b'x'; 512]].concat());
        let error = standalone_handoff(
            &argv,
            marker,
            held,
            |challenge| read_release_frame(&mut extra, challenge),
            || panic!("extra input must not read credentials"),
        )
        .unwrap_err();
        assert!(public_error(&error).starts_with("egress_transport_unavailable:"));
        assert_eq!(extra.position(), 65);
        let error = standalone_handoff(
            &argv,
            marker,
            held,
            |challenge| read_release_frame(io::Read::chain(FRAME, Unreadable), challenge),
            || panic!("IO failure at the end of stdin must not read credentials"),
        )
        .unwrap_err();
        assert!(public_error(&error).starts_with("egress_transport_unavailable:"));

        for (marker, held) in [
            (None, held),
            (Some(OsStr::new("invalid marker")), held),
            (marker, Some(OsStr::new("invalid challenge"))),
        ] {
            let error = standalone_handoff(
                &argv,
                marker,
                held,
                |_| panic!("invalid held configuration must not consume stdin"),
                || panic!("invalid held configuration must not read credentials"),
            )
            .unwrap_err();
            assert!(public_error(&error).starts_with("egress_transport_unavailable:"));
        }
        let mut interactive = argv;
        interactive[7] = "interactive-tty".into();
        let error = standalone_handoff(
            &interactive,
            marker,
            held,
            |_| panic!("the held mode must reject interactive input before reading stdin"),
            || panic!("the held mode must reject interactive input before credentials"),
        )
        .unwrap_err();
        assert!(public_error(&error).starts_with("egress_transport_unavailable:"));
    }

    struct EofWitness<'a>(&'a Cell<bool>);
    impl Read for EofWitness<'_> {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            self.0.set(true);
            Ok(0)
        }
    }

    struct Unreadable;
    impl Read for Unreadable {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("synthetic input failure"))
        }
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

    fn private_dir(mode: u32) -> tempfile::TempDir {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(mode)).unwrap();
        temp
    }

    fn installed(directory: &Path, bytes: &[u8], mode: u32) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.join("managed-settings.json");
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    fn own_uid() -> u32 {
        // SAFETY: read-only process credential query.
        unsafe { libc::geteuid() }
    }

    /// The repository file, read at test time (never compiled into the lib).
    fn shipped(relative: &str) -> Vec<u8> {
        std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("images/provider-activity")
                .join(relative),
        )
        .unwrap()
    }
    const CLAUDE_V1: &str = "claude-code/managed-settings.v1.json";
    const CLAUDE_V1_ONFAILURE: &str = "claude-code/managed-settings.v1-onfailure.json";
    const CODEX_V1: &str = "codex/requirements.toml";

    #[test]
    fn compiled_digests_are_those_of_the_shipped_hook_files() {
        for (relative, digest) in [
            (CLAUDE_V1, CLAUDE_ACTIVITY_V1_SHA256),
            (CLAUDE_V1_ONFAILURE, CLAUDE_ACTIVITY_V1_ONFAILURE_SHA256),
            (CODEX_V1, CODEX_ACTIVITY_V1_SHA256),
        ] {
            assert_eq!(
                hex::encode(Sha256::digest(shipped(relative))),
                digest,
                "images/provider-activity/{relative} changed: update its digest in bootstrap.rs"
            );
        }
    }

    fn refused(result: Result<()>) {
        let error = result.unwrap_err();
        assert!(error.to_string().starts_with("activity_unavailable:"));
        assert!(public_error(&error).starts_with("activity_unavailable:"));
    }

    #[test]
    fn managed_hooks_must_be_an_owned_read_only_expected_version() {
        let (_, claude) = managed_activity_config(ClientKind::ClaudeCode);
        let (_, codex) = managed_activity_config(ClientKind::Codex);
        let directory = private_dir(0o755);
        let (claude_v1, claude_onfailure, codex_v1) = (
            shipped(CLAUDE_V1),
            shipped(CLAUDE_V1_ONFAILURE),
            shipped(CODEX_V1),
        );
        for (bytes, expected) in [
            (claude_v1.as_slice(), claude),
            (claude_onfailure.as_slice(), claude),
            (codex_v1.as_slice(), codex),
        ] {
            let path = installed(directory.path(), bytes, 0o444);
            verify_managed_activity_config(&path, expected, own_uid()).unwrap();
            std::fs::remove_file(&path).unwrap();
        }
        // Changed bytes, another client's file, a writable file, another owner.
        let mut changed = claude_v1.clone();
        changed.extend_from_slice(b" ");
        for (bytes, mode, owner) in [
            (changed.as_slice(), 0o444, own_uid()),
            (codex_v1.as_slice(), 0o444, own_uid()),
            (claude_v1.as_slice(), 0o644, own_uid()),
            (claude_v1.as_slice(), 0o440, own_uid().wrapping_add(1)),
        ] {
            let path = installed(directory.path(), bytes, mode);
            refused(verify_managed_activity_config(&path, claude, owner));
            std::fs::remove_file(&path).unwrap();
        }
        // Absent file, final symlink, group/other-writable directory.
        refused(verify_managed_activity_config(
            &directory.path().join("absent.json"),
            claude,
            own_uid(),
        ));
        let target = installed(directory.path(), &claude_v1, 0o444);
        let link = directory.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        refused(verify_managed_activity_config(&link, claude, own_uid()));
        let open = private_dir(0o777);
        let path = installed(open.path(), &claude_v1, 0o444);
        refused(verify_managed_activity_config(&path, claude, own_uid()));
    }

    #[test]
    fn shipped_hook_configs_call_the_fixed_client_for_every_allowlisted_event() {
        use crate::activity_channel::wire::{CODEX_EVENTS, EventV1};
        for (relative, on_failure) in [(CLAUDE_V1, None), (CLAUDE_V1_ONFAILURE, Some("block"))] {
            let settings: serde_json::Value = serde_json::from_slice(&shipped(relative)).unwrap();
            assert_eq!(settings["allowManagedHooksOnly"], serde_json::json!(true));
            let hooks = settings["hooks"].as_object().unwrap();
            assert_eq!(hooks.len(), EventV1::ALL.len());
            for event in EventV1::ALL {
                let groups = hooks[event.as_str()].as_array().unwrap();
                assert_eq!(groups.len(), 1);
                let commands = groups[0]["hooks"].as_array().unwrap();
                assert_eq!(commands.len(), 1);
                assert_eq!(commands[0]["type"], "command");
                assert_eq!(
                    commands[0]["command"],
                    format!(
                        "/usr/local/bin/ai-guard-activity hook-v1 claude-code {}",
                        event.as_str()
                    )
                );
                assert_eq!(commands[0]["timeout"], 30);
                assert_eq!(commands[0]["onFailure"].as_str(), on_failure);
            }
        }
        let codex = String::from_utf8(shipped(CODEX_V1)).unwrap();
        assert!(codex.contains("\nallow_managed_hooks_only = true\n"));
        assert_eq!(codex.matches("[[hooks.").count(), 2 * CODEX_EVENTS.len());
        for event in CODEX_EVENTS {
            assert!(codex.contains(&format!(
                "command = \"/usr/local/bin/ai-guard-activity hook-v1 codex {}\"",
                event.as_str()
            )));
        }
    }

    #[test]
    fn subagent_text_is_forwarded_only_when_claude_batch_help_lists_it() {
        let batch = concat!(
            "Options:\n",
            "  --setting-sources <sources>\n",
            "  --print\n",
            "  --output-format <format>\n",
            "  --verbose\n",
            "  --no-session-persistence\n",
            "  --permission-mode <mode>",
        );
        let listed = format!("{batch}\n  --forward-subagent-text\n");
        let aliased = format!("{batch}\n  -f, --forward-subagent-text  Forward text.");
        let facts = |client, transport, help: &str| help_facts(client, transport, help).unwrap();
        assert!(
            facts(ClientKind::ClaudeCode, ClientTransport::Batch, &listed).forward_subagent_text
        );
        assert!(
            facts(ClientKind::ClaudeCode, ClientTransport::Batch, &aliased).forward_subagent_text
        );
        assert!(
            !facts(ClientKind::ClaudeCode, ClientTransport::Batch, batch).forward_subagent_text
        );
        assert!(
            !facts(
                ClientKind::ClaudeCode,
                ClientTransport::InteractiveTty,
                &listed
            )
            .forward_subagent_text
        );
        let error =
            help_facts(ClientKind::ClaudeCode, ClientTransport::Batch, "--print").unwrap_err();
        assert!(error.to_string().starts_with("version_incompatible:"));

        let temp = private_dir(0o700);
        let home = Directory::open_existing(&temp.path().canonicalize().unwrap()).unwrap();
        let state = home.child("state").unwrap();
        let arguments: Vec<String> = [
            "--setting-sources",
            "user",
            "--print",
            "--output-format",
            "stream-json",
            "--model",
            "a-model",
            "--",
            "--forward-subagent-text is data",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let mut execution = BootstrapExecution {
            program: "/usr/local/bin/claude",
            arguments: arguments.clone(),
            options_end: 5,
            environment: BTreeMap::new(),
            api_login: None,
            home,
            state,
            transport: ClientTransport::Batch,
            client: ClientKind::ClaudeCode,
        };
        for entry in [
            "  --trace   Help for the former --forward-subagent-text option",
            "          --forward-subagent-text was removed",
            "  --forward-subagent-text-format <format>",
            "  --forward-subagent-text",
            "  -f, --forward-subagent-text",
        ] {
            let help = format!("{batch}\n{entry}");
            execution.apply_probe_facts(facts(
                ClientKind::ClaudeCode,
                ClientTransport::Batch,
                &help,
            ));
            assert_eq!(execution.arguments(), arguments.as_slice());
        }
        execution.apply_probe_facts(facts(
            ClientKind::ClaudeCode,
            ClientTransport::Batch,
            &listed,
        ));
        let mut expected = arguments.clone();
        expected.insert(5, "--forward-subagent-text".into());
        assert_eq!(execution.arguments(), expected.as_slice());
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
