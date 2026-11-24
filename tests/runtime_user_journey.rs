//! Opt-in CLI journeys for real local workers and temporary control.
#![cfg(unix)]

#[path = "runtime_user_journey/podman_live.rs"]
mod podman_live;
#[path = "runtime_user_journey/same_tenant_capacity.rs"]
mod same_tenant_capacity;
mod support;

use agentsandbox::{
    control::{
        AllocationProvenanceV1, AllocationV1, MutationReceiptV1, NetworkV1, QuotaUsageV1,
        RuntimeKindV1, RuntimeObservationV1, RuntimeStatusV1, SecurityPresetV1, WorkerMutationV1,
        WorkerStateV1, WorkerTransitionV1, WorkerV1, content_hash,
    },
    control_fs::{Directory, MAX_PRIVATE_FILE_BYTES, PrivateFile, absolute_path},
    diagnostics::{engine, report::Reason},
    policy::Policy,
    runner::{AuditClient, CleanupState, RunSummary, read_token},
    runtime::{ContainerObservation, ManagedRuntime, Runtime, RuntimeKind},
};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs,
    io::Write,
    os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    process::{Output, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStderr, Command},
    time::{sleep, timeout},
};
use uuid::Uuid;

const CARGO_CLI: &str = env!("CARGO_BIN_EXE_agentsandbox");
const AUDITED_EXPORT_CLI: &str = "AGENTSANDBOX_TEST_AUDITED_EXPORT_CLI";
const AUDITED_EXPORT_CLI_SHA256: &str = "AGENTSANDBOX_TEST_AUDITED_EXPORT_CLI_SHA256";

const CLI_TIMEOUT: Duration = Duration::from_secs(45);
const RUN_TIMEOUT: Duration = Duration::from_secs(240);

fn cli(subcommand: &str) -> Command {
    cli_at(Path::new(CARGO_CLI), subcommand)
}

fn cli_at(binary: &Path, subcommand: &str) -> Command {
    let mut command = Command::new(binary);
    command
        .env_clear()
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .arg(subcommand);
    command
}

#[test]
fn audited_export_cli_selection_checks_path_and_expected_sha256_offline() -> Result<()> {
    use std::os::unix::fs::{PermissionsExt, symlink};

    assert_eq!(
        audited_export_cli(None, None)?.0,
        Path::new(env!("CARGO_BIN_EXE_agentsandbox")),
    );
    let root = support::private_tempdir();
    let root_path = root.path().canonicalize()?;
    let executable = root_path.join("cli");
    fs::write(&executable, b"abc")?;
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
    let digest =
        std::ffi::OsStr::new("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    let alias = root_path.join("alias");
    symlink(&executable, &alias)?;
    let missing = root_path.join("missing");
    let invalid = std::ffi::OsStr::new("not-a-digest");
    let different = "0".repeat(64);
    for (path, expected, message) in [
        (
            Some(Path::new("relative")),
            Some(digest),
            "must be absolute",
        ),
        (Some(missing.as_path()), Some(digest), "does not exist"),
        (Some(alias.as_path()), Some(digest), "must be canonical"),
        (
            Some(executable.as_path()),
            Some(invalid),
            "64 lowercase hex digits",
        ),
        (
            Some(executable.as_path()),
            Some(different.as_ref()),
            "SHA-256 mismatch",
        ),
        (Some(executable.as_path()), None, "must be set together"),
        (None, Some(digest), "must be set together"),
    ] {
        let Err(error) = audited_export_cli(path, expected) else {
            bail!("expected refusal: {message}");
        };
        assert!(
            error.to_string().contains(message),
            "unexpected refusal: {error}"
        );
    }
    assert_eq!(
        audited_export_cli(Some(&executable), Some(digest))?,
        (executable, Some(digest.to_str().unwrap().to_owned())),
    );
    Ok(())
}

fn audited_export_cli(
    path: Option<&Path>,
    expected: Option<&OsStr>,
) -> Result<(PathBuf, Option<String>)> {
    let (path, expected) = match (path, expected) {
        (None, None) => return Ok((PathBuf::from(CARGO_CLI), None)),
        (Some(path), Some(expected)) => (path, expected),
        _ => bail!("{AUDITED_EXPORT_CLI} and {AUDITED_EXPORT_CLI_SHA256} must be set together"),
    };
    ensure!(
        path.is_absolute(),
        "audited export CLI path must be absolute"
    );
    let canonical = path.canonicalize().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!("audited export CLI does not exist")
        } else {
            anyhow::Error::new(error).context("canonicalize the audited export CLI")
        }
    })?;
    ensure!(
        canonical.as_os_str() == path.as_os_str(),
        "audited export CLI path must be canonical"
    );
    let expected = expected
        .to_str()
        .filter(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .context("audited export CLI SHA-256 must be 64 lowercase hex digits")?;
    let is_executable =
        |metadata: &fs::Metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0;
    ensure!(
        is_executable(&fs::symlink_metadata(&canonical)?),
        "audited export CLI must be an executable regular file"
    );
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&canonical)?;
    ensure!(
        is_executable(&file.metadata()?),
        "audited export CLI must be an executable regular file"
    );
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash)?;
    let observed = hex::encode(hash.finalize());
    ensure!(observed == expected, "audited export CLI SHA-256 mismatch");
    Ok((canonical, Some(observed)))
}

async fn output(mut command: Command, phase: &str, budget: Duration) -> Result<Output> {
    timeout(budget, command.output())
        .await
        .with_context(|| format!("{phase} exceeded its test budget"))?
        .with_context(|| format!("could not run {phase}"))
}

async fn checked_output(command: Command, phase: &str, budget: Duration) -> Result<Vec<u8>> {
    let capture = output(command, phase, budget).await?;
    ensure!(
        capture.status.success(),
        "{phase} failed with status {}",
        capture.status
    );
    Ok(capture.stdout)
}

struct RuntimeDoctor {
    rootless: bool,
    server_version: Option<String>,
}

async fn doctor(
    binary: &Path,
    runtime: RuntimeKind,
    program: &Path,
    endpoint: &str,
    allow_rootful: bool,
) -> Result<RuntimeDoctor> {
    let mut command = cli_at(binary, "doctor");
    command
        .arg("--runtime")
        .arg(runtime.to_string())
        .arg("--runtime-program")
        .arg(program)
        .args(["--runtime-endpoint", endpoint]);
    if allow_rootful {
        command.arg("--allow-rootful-runtime");
    }
    let capture = output(command, "doctor", CLI_TIMEOUT).await.map_err(|_| {
        anyhow::anyhow!(
            "{}",
            json!({"status":"BLOCKED","stage":"doctor",
            "reason":"doctor_child_unavailable_or_timed_out","workers_started":0})
        )
    })?;
    if !capture.status.success() {
        // The CLI emits text. Reuse the product's closed diagnostic mapping
        // and preserve only that reason and the exit status in test evidence.
        let stderr = String::from_utf8_lossy(&capture.stderr);
        let (reason, situation) = stderr
            .lines()
            .find_map(|line| {
                let line = line.trim();
                let (reason, situation) =
                    engine::classify(line.strip_prefix("Error: ").unwrap_or(line));
                situation.map(|situation| (reason, Some(situation)))
            })
            .unwrap_or_else(|| engine::classify(""));
        let status = if reason == Reason::AccountHomeUnavailable {
            "BLOCKED"
        } else {
            "FAIL"
        };
        bail!(
            "{}",
            json!({"status":status, "stage":"doctor", "exit_code":capture.status.code(), "reason":reason, "situation":situation,"workers_started":0})
        );
    }
    // doctor prints its JSON first and the engine's version text afterwards.
    let mut decoder = serde_json::Deserializer::from_slice(&capture.stdout);
    let info = Value::deserialize(&mut decoder)?;
    ensure!(
        info["runtime"] == runtime.to_string(),
        "doctor selected a different runtime"
    );
    let rootless = info["rootless"]
        .as_bool()
        .context("doctor did not establish the runtime mode")?;
    Ok(RuntimeDoctor {
        rootless,
        server_version: info["server_version"].as_str().map(str::to_owned),
    })
}

struct Gateway {
    binary: PathBuf,
    child: Child,
    _stderr: BufReader<ChildStderr>,
    origin: String,
}

impl Gateway {
    async fn start(config: &Path, binary: &Path) -> Result<Self> {
        let mut command = cli_at(binary, "gateway");
        command
            .args(["--listen", "127.0.0.1:0", "--data"])
            .arg(config.join("audit.sqlite"))
            .arg("--runner-token-file")
            .arg(config.join("secrets/runner"))
            .arg("--viewer-token-file")
            .arg(config.join("secrets/viewer"))
            .arg("--policy")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("policies/default.json"));
        Self::start_command(command).await
    }

    async fn start_tenants(registry: &Path, policy: &Path) -> Result<Self> {
        let mut command = cli("gateway");
        command
            .args(["--listen", "127.0.0.1:0", "--tenants-file"])
            .arg(registry)
            .arg("--policy")
            .arg(policy);
        Self::start_command(command).await
    }

    async fn start_command(mut command: Command) -> Result<Self> {
        let binary = PathBuf::from(command.as_std().get_program());
        let mut child = command
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stderr = BufReader::new(child.stderr.take().context("gateway stderr unavailable")?);
        let mut line = String::new();
        timeout(Duration::from_secs(20), stderr.read_line(&mut line)).await??;
        let address: std::net::SocketAddr = line
            .strip_prefix("Gateway ")
            .and_then(|line| line.split_whitespace().next())
            .context("gateway did not report readiness")?
            .parse()?;
        ensure!(address.ip().is_loopback(), "QA gateway must use loopback");
        Ok(Self {
            binary,
            child,
            _stderr: stderr,
            origin: format!("http://{address}"),
        })
    }

    fn command(&self, subcommand: &str, config: &Path) -> Command {
        let mut command = cli_at(&self.binary, subcommand);
        command
            .arg("--config-dir")
            .arg(config)
            .args(["--gateway", &self.origin]);
        command
    }
}

struct RuntimeSelection {
    runtime: RuntimeKind,
    podman_rootless: Option<bool>,
    program: PathBuf,
    endpoint: String,
    image: String,
    allow_rootful: bool,
}

#[derive(PartialEq, Eq, serde::Serialize)]
struct LiveWorker {
    worker_id: String,
    session_id: String,
    attempt_id: String,
    container_id: String,
    image_id: String,
    stable_name: String,
    started_at: String,
}

impl LiveWorker {
    fn read(worker: &WorkerV1) -> Result<Self> {
        let cid = worker
            .attempt
            .container_id
            .as_ref()
            .context("live worker CID absent")?;
        ensure!(
            cid.len() == 64
                && cid
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "live worker CID is not a full container identifier"
        );
        let observation = worker
            .last_observation
            .as_ref()
            .context("live worker inspection absent")?;
        ensure!(
            observation.inspection.container_id == *cid && observation.restart_count == Some(0),
            "live worker observation does not attest the running container"
        );
        Ok(Self {
            worker_id: worker.worker_id.clone(),
            session_id: worker.launch.session_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            container_id: cid.clone(),
            image_id: worker.launch.image_id.clone(),
            stable_name: worker.attempt.stable_name.clone(),
            started_at: observation
                .started_at
                .clone()
                .context("live worker StartedAt absent")?,
        })
    }
}

struct LivePeak {
    workers: [LiveWorker; 3],
    quotas_at_peak: [Value; 2],
}

impl LivePeak {
    fn read(workers: [&WorkerV1; 3], quotas_at_peak: [Value; 2]) -> Result<Self> {
        let [a1, a2, b1] = workers.map(LiveWorker::read);
        Ok(Self {
            workers: [a1?, a2?, b1?],
            quotas_at_peak,
        })
    }
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct LiveDockerSnapshot {
    id: String,
    name: String,
    image: String,
    session: String,
    purpose: String,
    network: String,
    running: bool,
    paused: bool,
    restarting: bool,
    status: String,
    started_at: String,
    restart_count: u32,
}

#[derive(serde::Serialize)]
#[serde(untagged)]
enum LiveNativeSnapshot {
    Docker(LiveDockerSnapshot),
    Podman(podman_live::LivePodmanSnapshot),
}

fn same_started_at(left: &str, right: &str) -> bool {
    matches!((chrono::DateTime::parse_from_rfc3339(left), chrono::DateTime::parse_from_rfc3339(right)),
        (Ok(left), Ok(right)) if left == right)
}

impl RuntimeSelection {
    fn read_docker() -> Result<Self> {
        ensure!(
            std::env::var("AGENTSANDBOX_TEST_RUNTIME").as_deref() == Ok("docker"),
            "set AGENTSANDBOX_TEST_RUNTIME=docker explicitly"
        );
        Self::read(RuntimeKind::Docker, None)
    }

    fn read_parallel() -> Result<Self> {
        match std::env::var("AGENTSANDBOX_TEST_RUNTIME").as_deref() {
            Ok("docker") => Self::read_docker(),
            Ok("podman") => {
                let rootless = match std::env::var("AGENTSANDBOX_TEST_PODMAN_MODE").as_deref() {
                    Ok("rootless") => true,
                    Ok("rootful") => false,
                    _ => bail!("set AGENTSANDBOX_TEST_PODMAN_MODE=rootless or rootful explicitly"),
                };
                Self::read(RuntimeKind::Podman, Some(rootless))
            }
            _ => bail!("set AGENTSANDBOX_TEST_RUNTIME=docker or podman explicitly"),
        }
    }

    fn read(runtime: RuntimeKind, podman_rootless: Option<bool>) -> Result<Self> {
        let program = PathBuf::from(
            std::env::var_os("AGENTSANDBOX_TEST_RUNTIME_PROGRAM")
                .with_context(|| format!("set an explicit absolute {runtime} program"))?,
        );
        ensure!(program.is_absolute(), "{runtime} program must be absolute");
        let endpoint = std::env::var("AGENTSANDBOX_TEST_RUNTIME_ENDPOINT")
            .with_context(|| format!("set an explicit local Unix {runtime} endpoint"))?;
        let socket = Path::new(
            endpoint
                .strip_prefix("unix://")
                .context("expected a Unix endpoint")?,
        );
        ensure!(
            socket.is_absolute()
                && !endpoint.contains(['?', '#', '@'])
                && !endpoint.chars().any(char::is_control)
                && !socket.components().any(|part| part == Component::ParentDir)
                && fs::metadata(socket)?.file_type().is_socket(),
            "expected an existing local Unix socket"
        );
        let image =
            std::env::var("AGENTSANDBOX_TEST_IMAGE").context("set an explicit QA image ID")?;
        ensure!(
            image
                .strip_prefix("sha256:")
                .is_some_and(|digest| digest.len() == 64
                    && digest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))),
            "QA image must be given as a full local sha256 image ID"
        );
        let rootful_opt_in = std::env::var("AGENTSANDBOX_TEST_ALLOW_ROOTFUL").as_deref() == Ok("1");
        let allow_rootful = match podman_rootless {
            Some(false) => {
                ensure!(
                    rootful_opt_in,
                    "Podman rootful requires AGENTSANDBOX_TEST_ALLOW_ROOTFUL=1"
                );
                true
            }
            Some(true) => false,
            None => rootful_opt_in,
        };
        Ok(Self {
            runtime,
            podman_rootless,
            program,
            endpoint,
            image,
            allow_rootful,
        })
    }

    fn runtime_kind(&self) -> RuntimeKindV1 {
        match self.runtime {
            RuntimeKind::Docker => RuntimeKindV1::Docker,
            RuntimeKind::Podman => RuntimeKindV1::Podman,
        }
    }

    async fn preflight(&self) -> Result<RuntimeDoctor> {
        self.preflight_with_cli(Path::new(CARGO_CLI)).await
    }

    async fn preflight_with_cli(&self, binary: &Path) -> Result<RuntimeDoctor> {
        let diagnostic = doctor(
            binary,
            self.runtime,
            &self.program,
            &self.endpoint,
            self.allow_rootful,
        )
        .await?;
        let runtime = Runtime::with_program(self.runtime, &self.program)?;
        match self.runtime {
            RuntimeKind::Docker => {
                let engine = ManagedRuntime::connect_docker_with_runtime(
                    Uuid::new_v4(),
                    &self.endpoint,
                    runtime,
                )
                .await?;
                ensure!(
                    engine.resolve_image(&self.image).await? == self.image,
                    "local QA image identity differed"
                );
            }
            RuntimeKind::Podman => self.preflight_podman(&runtime, &diagnostic).await?,
        }
        Ok(diagnostic)
    }

    async fn preflight_podman(&self, runtime: &Runtime, diagnostic: &RuntimeDoctor) -> Result<()> {
        let expected = self
            .podman_rootless
            .context("expected Podman mode absent")?;
        let target = || vec!["--remote".into(), "--url".into(), self.endpoint.clone()];
        let mut info_args = target();
        info_args.extend(["info".into(), "--format".into(), "json".into()]);
        let info = runtime
            .cli(&info_args, Duration::from_secs(15))
            .await
            .map_err(|_| anyhow::anyhow!("selected Podman info failed before worker admission"))?;
        ensure!(!info.truncated, "selected Podman info exceeded its bound");
        let info: Value = serde_json::from_slice(&info.bytes)
            .map_err(|_| anyhow::anyhow!("selected Podman info is not valid JSON"))?;
        let rootless = info
            .pointer("/host/security/rootless")
            .and_then(Value::as_bool)
            .context("selected Podman endpoint did not explicitly report rootless mode")?;
        ensure!(
            rootless == expected && rootless == diagnostic.rootless,
            "selected Podman endpoint does not attest the requested mode"
        );
        // Inspect the local cache only. The CLI binds its real Podman authority
        // after durable authorization, so this preflight creates no marker.
        let mut image_args = target();
        image_args.extend([
            "image".into(),
            "inspect".into(),
            "--format".into(),
            "{{.Id}}".into(),
            self.image.clone(),
        ]);
        let image = runtime
            .cli(&image_args, Duration::from_secs(15))
            .await
            .map_err(|_| {
                anyhow::anyhow!("could not inspect the QA image in the selected Podman cache")
            })?;
        ensure!(
            !image.truncated,
            "selected Podman image identity exceeded its bound"
        );
        let identity = std::str::from_utf8(&image.bytes)?.trim();
        ensure!(
            self.image.strip_prefix("sha256:")
                == Some(identity.strip_prefix("sha256:").unwrap_or(identity)),
            "local QA image identity differed"
        );
        Ok(())
    }

    async fn inspect_live_workers(
        &self,
        workers: &[LiveWorker],
    ) -> Result<Vec<LiveDockerSnapshot>> {
        ensure!(
            self.runtime == RuntimeKind::Docker,
            "live container inspection requires Docker"
        );
        ensure!(
            (1..=100).contains(&workers.len())
                && workers
                    .iter()
                    .map(|worker| &worker.container_id)
                    .collect::<BTreeSet<_>>()
                    .len()
                    == workers.len(),
            "Docker inspection requires 1 to 100 distinct fixture CIDs"
        );
        // Same narrow field projection and Unix endpoint pattern as broker_runtime::inspect.
        // Avoid full inspect output, which can include container environment values.
        let format = r#"{"id":{{json .Id}},"name":{{json .Name}},"image":{{json .Image}},"session":{{json (index .Config.Labels "agentsandbox.session")}},"purpose":{{json (index .Config.Labels "agentsandbox.purpose")}},"network":{{json .HostConfig.NetworkMode}},"running":{{json .State.Running}},"paused":{{json .State.Paused}},"restarting":{{json .State.Restarting}},"status":{{json .State.Status}},"started_at":{{json .State.StartedAt}},"restart_count":{{json .RestartCount}}}"#;
        let mut child = Command::new(&self.program)
            .args([
                "--host",
                &self.endpoint,
                "container",
                "inspect",
                "--format",
                format,
            ])
            .args(workers.iter().map(|worker| &worker.container_id))
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let limit = 2048 * workers.len();
        let mut stdout = child
            .stdout
            .take()
            .context("Docker inspection output not available")?
            .take((limit + 1) as u64);
        let mut bytes = Vec::new();
        let result = timeout(Duration::from_secs(3), async {
            stdout.read_to_end(&mut bytes).await?;
            ensure!(
                bytes.len() <= limit,
                "Docker inspection output exceeded its bound"
            );
            Ok::<_, anyhow::Error>(child.wait().await?)
        })
        .await;
        let exit = match result {
            Ok(ok) => ok?,
            Err(_) => {
                let _ = child.start_kill();
                let _ = timeout(Duration::from_secs(3), child.wait()).await;
                bail!("Docker inspection exceeded its budget");
            }
        };
        ensure!(
            exit.success(),
            "Docker inspection did not complete successfully"
        );
        let facts: Vec<LiveDockerSnapshot> = serde_json::Deserializer::from_slice(&bytes)
            .into_iter()
            .collect::<std::result::Result<_, _>>()?;
        ensure!(
            facts.len() == workers.len(),
            "Docker inspection did not return the exact number of fixture containers"
        );
        for worker in workers {
            let matching: Vec<_> = facts
                .iter()
                .filter(|fact| fact.id == worker.container_id)
                .collect();
            let [fact] = matching.as_slice() else {
                bail!("Docker inspection returned an unexpected CID set");
            };
            ensure!(
                fact.name == format!("/{}", worker.stable_name)
                    && fact.image == worker.image_id
                    && fact.image == self.image
                    && fact.session == worker.session_id
                    && fact.purpose == "run"
                    && fact.network == "none"
                    && fact.running
                    && !fact.paused
                    && !fact.restarting
                    && fact.status == "running"
                    && fact.restart_count == 0
                    && same_started_at(&fact.started_at, &worker.started_at),
                "Docker did not attest the same running fixture container"
            );
        }
        Ok(facts)
    }

    fn run_command(
        &self,
        gateway: &Gateway,
        config: &Path,
        source: &Path,
        deadline: &str,
    ) -> Command {
        let mut run = gateway.command("run", config);
        run.arg("--project")
            .arg(source)
            .arg("--runtime")
            .arg(self.runtime.to_string())
            .arg("--runtime-program")
            .arg(&self.program)
            .args([
                "--runtime-endpoint",
                &self.endpoint,
                "--image",
                &self.image,
                "--profile",
                "strict",
                "--network",
                "none",
                "--timeout",
                deadline,
                "--capture-output",
            ]);
        if self.allow_rootful {
            run.arg("--allow-rootful-runtime");
        }
        run
    }
}

fn one_event<'a>(events: &'a [Value], kind: &str) -> Result<&'a Value> {
    let mut matching = events.iter().filter(|event| event["kind"] == kind);
    let event = matching.next().with_context(|| format!("missing {kind}"))?;
    ensure!(matching.next().is_none(), "duplicate {kind}");
    Ok(event)
}

#[tokio::test]
#[ignore = "requires an explicitly selected local Docker or rootless Podman socket, an approved control account, and a prepared local QA image"]
async fn real_cli_copies_a_project_and_exports_after_one_audited_worker() -> Result<()> {
    let cli_path = std::env::var_os(AUDITED_EXPORT_CLI);
    let expected_sha256 = std::env::var_os(AUDITED_EXPORT_CLI_SHA256);
    let (binary, sha256) = audited_export_cli(
        cli_path.as_deref().map(Path::new),
        expected_sha256.as_deref(),
    )?;
    if let Some(sha256) = &sha256 {
        eprintln!(
            "{}",
            json!({
                "stage":"cli_selection", "canonical_path":binary, "sha256":sha256,
            })
        );
    }
    let selection = match std::env::var("AGENTSANDBOX_TEST_RUNTIME").as_deref() {
        Ok("podman") => {
            ensure!(
                std::env::var("AGENTSANDBOX_TEST_PODMAN_MODE").as_deref() == Ok("rootless"),
                "audited export requires AGENTSANDBOX_TEST_PODMAN_MODE=rootless for Podman"
            );
            RuntimeSelection::read(RuntimeKind::Podman, Some(true))?
        }
        _ => RuntimeSelection::read_docker()?,
    };
    let runtime = selection.runtime.to_string();
    let rootless = selection.preflight_with_cli(&binary).await?.rootless;
    let program = &selection.program;
    let image = selection.image.as_str();

    // Keep this private source and control directory, including on failure, so
    // uncertain worker state can be diagnosed without touching other sessions.
    let directory = support::private_tempdir().keep().canonicalize()?;
    eprintln!("Retained runtime CLI fixture {}", directory.display());
    let config = directory.join("control");
    let source = directory.join("source");
    let destination = directory.join("export");
    let original = b"runtime user journey original\n";
    let private = format!("runtime-journey-private-{}", Uuid::new_v4());
    let dotenv = format!("TEST_ONLY_LOCAL_VALUE={private}\n");
    fs::create_dir(&source)?;
    fs::write(source.join("original.txt"), original)?;
    fs::write(source.join(".env"), &dotenv)?;
    let mut init = cli_at(&binary, "init");
    init.arg("--config-dir").arg(&config);
    checked_output(init, "init", CLI_TIMEOUT).await?;
    let mut gateway = Gateway::start(&config, &binary).await?;
    let viewer = read_token(&config.join("secrets/viewer"))?;
    let audit = AuditClient::new(&gateway.origin, &viewer)?;
    let integrity_before = audit.get_json("api/v1/integrity").await?;
    assert_eq!(integrity_before["valid"], true);

    let mut run = selection.run_command(&gateway, &config, &source, "30");
    run.args(["--", "/usr/bin/id"]);
    let run: RunSummary = serde_json::from_slice(&checked_output(run, "run", RUN_TIMEOUT).await?)?;
    assert_eq!(run.exit_code, 0);
    assert!(!run.timed_out);
    assert!(run.output_captured);
    assert_eq!(run.changed_files, Some(0));
    assert!(run.postprocessing_error.is_none());
    let session = run.session_id.to_string();
    let mut status = gateway.command("worker-status", &config);
    status.args(["--session", &session]);
    let status: Value =
        serde_json::from_slice(&checked_output(status, "worker-status", CLI_TIMEOUT).await?)?;
    let worker = &status["worker"];
    let worker_id = worker["worker_id"]
        .as_str()
        .context("terminal worker identity")?;
    assert_eq!(worker["state"], "terminal");
    assert_eq!(worker["quota_held"], false);
    assert_eq!(worker["cleanup_removed"], true);
    assert_eq!(worker["launch"]["command"]["argv"], json!(["/usr/bin/id"]));
    assert_eq!(worker["launch"]["runtime_kind"], runtime);
    assert_eq!(worker["launch"]["network"], "none");
    assert_eq!(worker["launch"]["image_id"], image);
    assert_eq!(worker["attempt"]["volume_name"], run.volume);
    let created = &worker["attempt"]["created_inspection"];
    assert_eq!(created["network"], "none");
    assert_eq!(created["security_preset"], "fixed_sandbox_v2");
    assert_eq!(created["cpu_millis"], 2000);
    assert_eq!(created["memory_bytes"], 1024_u64 * 1024 * 1024);
    assert_eq!(created["pids_limit"], 256);
    let registered = audit.get_json("api/v1/workers").await?;
    let registered = registered["items"]
        .as_array()
        .context("worker registry items")?;
    assert_eq!(registered.len(), 1);
    assert_eq!(registered[0]["worker_id"], worker_id);
    let usage = audit.get_json("api/v1/usage").await?;
    let allocation = usage["items"].as_array().context("usage items")?;
    assert_eq!(allocation.len(), 1);
    assert_eq!(allocation[0]["worker_id"], worker_id);
    assert_eq!(allocation[0]["attempt_id"], worker["attempt"]["attempt_id"]);
    assert_eq!(
        allocation[0]["source"]["container_id"],
        worker["attempt"]["container_id"]
    );
    assert_eq!(allocation[0]["source"]["spec_hash"], worker["spec_hash"]);
    if selection.runtime == RuntimeKind::Podman {
        assert_eq!(allocation[0]["source"]["runtime_kind"], runtime);
    }
    assert_eq!(allocation[0]["source"]["provenance"], "observed");
    assert_eq!(allocation[0]["billable"], false);
    assert!(
        allocation[0]["duration_ns"]
            .as_str()
            .context("allocation interval")?
            .parse::<u128>()?
            > 0
    );

    for _ in 0..2 {
        let mut resume = gateway.command("worker-resume", &config);
        resume
            .args([
                "--session",
                &session,
                "--wait-seconds",
                "5",
                "--runtime-program",
            ])
            .arg(program);
        let resumed: Value =
            serde_json::from_slice(&checked_output(resume, "worker-resume", CLI_TIMEOUT).await?)?;
        assert_eq!(resumed["worker"]["worker_id"], worker_id);
        assert_eq!(resumed["worker"]["state"], "terminal");
    }
    assert_eq!(audit.get_json("api/v1/usage").await?, usage);
    assert_eq!(audit.get_json("api/v1/quota").await?["held_workers"], 0);

    let mut export = gateway.command("export-workspace", &config);
    export
        .args(["--session", &session, "--runtime-program"])
        .arg(program)
        .arg("--output")
        .arg(&destination);
    checked_output(export, "export-workspace", RUN_TIMEOUT).await?;
    assert_eq!(fs::read(source.join("original.txt"))?, original);
    assert_eq!(fs::read(source.join(".env"))?, dotenv.as_bytes());
    assert_eq!(fs::read(destination.join("original.txt"))?, original);
    assert!(!destination.join(".env").exists());
    assert_eq!(fs::read_dir(&source)?.count(), 2);
    assert_eq!(fs::read_dir(&destination)?.count(), 1);

    let history = audit
        .get_json(&format!("api/v1/events?sandbox_id={session}&limit=100"))
        .await?;
    assert!(history["next_cursor"].is_null());
    let events = history["items"].as_array().context("session audit items")?;
    let requested = one_event(events, "command.requested")?;
    assert_eq!(requested["command"]["argv"], json!(["/usr/bin/id"]));
    let imported = one_event(events, "workspace.import.completed")?;
    assert_eq!(imported["data"]["files"], 1);
    assert_eq!(
        imported["data"]["manifest_digest"],
        worker["launch"]["workspace_manifest_digest"]
    );
    let submitted = one_event(events, "runtime.submitted")?;
    assert_eq!(submitted["data"]["volume"], run.volume);
    assert_eq!(submitted["data"]["network"], "none");
    let completed = one_event(events, "command.completed")?;
    assert_eq!(completed["data"]["worker_id"], worker_id);
    assert_eq!(completed["result"]["exit_code"], 0);
    assert!(
        completed["result"]["stdout"]
            .as_str()
            .is_some_and(|stdout| stdout.contains("uid=1000"))
    );
    let exported = one_event(events, "workspace.export.completed")?;
    assert_eq!(exported["data"]["files"], 1);
    let mut previous = 0;
    for event in [requested, imported, submitted, completed, exported] {
        let seq = event["seq"]
            .as_i64()
            .context("audit sequence unavailable")?;
        assert!(seq > previous, "runtime audit sequence is unordered");
        previous = seq;
    }
    let serialized = serde_json::to_string(events)?;
    assert!(!serialized.contains(&private));
    assert!(!serialized.contains(&viewer));
    let verified = checked_output(
        gateway.command("verify-audit", &config),
        "verify-audit",
        CLI_TIMEOUT,
    )
    .await?;
    let verified: Value = serde_json::from_slice(&verified)?;
    assert_eq!(verified["valid"], true);
    let event_count = |audit: &Value| audit["event_count"].as_u64().context("invalid audit count");
    assert!(event_count(&verified)? > event_count(&integrity_before)?);
    timeout(Duration::from_secs(5), gateway.child.kill()).await??;
    eprintln!(
        "{}",
        json!({
            "status":"PASS", "runtime":runtime, "rootless":rootless, "image_id":image,
            "session_id":session, "worker_id":worker_id, "audit_head":verified["head_hash"],
            "attempt_id":worker["attempt"]["attempt_id"],
            "container_id":worker["attempt"]["container_id"],
            "work_volume":worker["attempt"]["volume_name"],
            "authority_id":worker["launch"]["authority_id"],
        })
    );
    Ok(())
}

const PARALLEL_TIMEOUT: &str = "60";
const WORKER_CPU: u64 = 2000;
const WORKER_MEMORY: u64 = 1024 * 1024 * 1024;

struct ParallelJob {
    label: &'static str,
    tenant: &'static str,
    seconds: &'static str,
    root: PathBuf,
    marker: String,
}

impl ParallelJob {
    async fn prepare(&self, copied_credentials: Option<&Path>) -> Result<()> {
        Directory::open_or_create(&self.root)?.child("source")?;
        fs::write(self.source().join("marker.txt"), &self.marker)?;
        let config = self.config();
        if let Some(original) = copied_credentials {
            // Different config roots keep authority and checkpoints independent.
            // Only A's freshly generated credential pair is shared by its jobs.
            let credentials = Directory::open_or_create(&config)?.child("secrets")?;
            for role in ["runner", "viewer"] {
                let value = read_token(&original.join("secrets").join(role))?;
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(credentials.path().join(role))?
                    .write_all(value.as_bytes())?;
            }
        }
        let mut init = cli("init");
        init.arg("--config-dir").arg(config);
        checked_output(init, "parallel_fixture_init", CLI_TIMEOUT).await?;
        Ok(())
    }

    fn source(&self) -> PathBuf {
        self.root.join("source")
    }

    fn config(&self) -> PathBuf {
        self.root.join("control")
    }

    fn argv(&self) -> Vec<String> {
        vec!["/bin/sleep".into(), self.seconds.into()]
    }

    fn run_command(&self, gateway: &Gateway, selection: &RuntimeSelection) -> Command {
        let mut run =
            selection.run_command(gateway, &self.config(), &self.source(), PARALLEL_TIMEOUT);
        run.arg("--").args(self.argv());
        run
    }

    async fn run(&self, gateway: &Gateway, selection: &RuntimeSelection) -> Result<RunSummary> {
        let bytes = checked_output(
            self.run_command(gateway, selection),
            self.label,
            RUN_TIMEOUT,
        )
        .await?;
        self.normal_summary(&bytes)
    }

    fn normal_summary(&self, bytes: &[u8]) -> Result<RunSummary> {
        let summary: RunSummary = serde_json::from_slice(bytes)?;
        ensure!(
            summary.exit_code == 0
                && !summary.timed_out
                && summary.output_captured
                && summary.postprocessing_error.is_none()
                && summary.transport_stop_reason.is_none()
                && summary.changed_files == Some(0)
                && summary.cleanup.state == CleanupState::Removed,
            "{} did not complete and remove its worker normally",
            self.label
        );
        Ok(summary)
    }
}

async fn current_workers(client: &AuditClient, max: usize) -> Result<Vec<WorkerV1>> {
    workers_from_page(client.get_json("api/v1/workers").await?, max)
}

fn workers_from_page(page: Value, max: usize) -> Result<Vec<WorkerV1>> {
    ensure!(
        page["next_cursor"].is_null(),
        "recipe worker page is not complete"
    );
    let workers: Vec<WorkerV1> = serde_json::from_value(page["items"].clone())?;
    ensure!(
        workers.len() <= max,
        "unexpected worker in the recipe tenant"
    );
    Ok(workers)
}

async fn check_quota(client: &AuditClient, configured: u32, held: u32) -> Result<()> {
    validate_quota(client.get_json("api/v1/quota").await?, configured, held)
}

fn validate_quota(page: Value, configured: u32, held: u32) -> Result<()> {
    let reading: QuotaUsageV1 = serde_json::from_value(page)?;
    ensure!(
        reading.policy.max_active_workers == configured
            && u64::from(reading.policy.max_cpu_millis) == u64::from(configured) * WORKER_CPU
            && reading.policy.max_memory_bytes == u64::from(configured) * WORKER_MEMORY
            && reading.held_workers == held
            && reading.held_cpu_millis == u64::from(held) * WORKER_CPU
            && reading.held_memory_bytes == u64::from(held) * WORKER_MEMORY,
        "tenant quota projection differs from the expected reservation totals"
    );
    Ok(())
}

const PARALLEL_POLL: Duration = Duration::from_millis(250);
const PARALLEL_CAPACITY_RETRIES: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParallelRead {
    Workers,
    Quota,
}

impl ParallelRead {
    fn path(self) -> &'static str {
        match self {
            Self::Workers => "/api/v1/workers",
            Self::Quota => "/api/v1/quota",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewerCapacityEnvelope {
    error: ViewerCapacityError,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewerCapacityError {
    code: String,
    #[serde(rename = "message")]
    _message: String,
    retry_action: String,
}

fn retry_parallel_viewer_read(status: reqwest::StatusCode, body: &[u8]) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS
        && serde_json::from_slice::<ViewerCapacityEnvelope>(body).is_ok_and(|envelope| {
            envelope.error.code == "capacity_exhausted" && envelope.error.retry_action == "retry"
        })
}

const PARALLEL_TEST_CAPACITY: &[u8] =
    br#"{"error":{"code":"capacity_exhausted","message":"busy","retry_action":"retry"}}"#;

#[test]
fn parallel_viewer_only_retries_explicit_read_capacity() {
    let limited = reqwest::StatusCode::TOO_MANY_REQUESTS;
    let capacity = PARALLEL_TEST_CAPACITY;
    assert!(retry_parallel_viewer_read(limited, capacity));
    assert!(!retry_parallel_viewer_read(
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        capacity
    ));
    for refusal in [
        &br#"{"error":{"code":"write_rate_limited","message":"limit","retry_action":"retry"}}"#[..],
        &br#"{"error":{"code":"audit_read_budget_exhausted","message":"budget","retry_action":"contact_operator"}}"#[..],
        &br#"{"error":{"code":"capacity_exhausted","message":"busy","retry_action":"contact_operator"}}"#[..],
        &br#"{"error":{"code":"capacity_exhausted","code":"capacity_exhausted","message":"busy","retry_action":"retry"}}"#[..],
        &b"429"[..],
    ] {
        assert!(!retry_parallel_viewer_read(limited, refusal));
    }
}

struct ParallelViewerReads<'a> {
    client: reqwest::Client,
    origin: reqwest::Url,
    viewers: &'a [String; 2],
}

impl<'a> ParallelViewerReads<'a> {
    fn new(origin: &str, viewers: &'a [String; 2]) -> Result<Self> {
        let origin = reqwest::Url::parse(origin).context("invalid parallel recipe origin")?;
        ensure!(
            origin.scheme() == "http"
                && origin.host_str() == Some("127.0.0.1")
                && origin.port().is_some()
                && origin.username().is_empty()
                && origin.password().is_none()
                && origin.path() == "/"
                && origin.query().is_none()
                && origin.fragment().is_none(),
            "parallel viewer reads require the literal loopback fixture origin"
        );
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(5))
            .build()?;
        Ok(Self {
            client,
            origin,
            viewers,
        })
    }

    async fn read(&self, route: ParallelRead, tenant_index: usize) -> Result<Option<Value>> {
        let tenant = ["tenant-a", "tenant-b"][tenant_index];
        let viewer = &self.viewers[tenant_index];
        let path = route.path();
        let mut response = self
            .client
            .get(self.origin.join(path)?)
            .bearer_auth(viewer)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("parallel observation {tenant} GET {path} unavailable"))?;
        let status = response.status();
        let limit = match status {
            reqwest::StatusCode::OK => 512 * 1024,
            reqwest::StatusCode::TOO_MANY_REQUESTS => 4096,
            _ => bail!("parallel observation {tenant} GET {path} refused (HTTP {status})"),
        };
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("parallel observation {tenant} GET {path} incomplete"))?
        {
            ensure!(
                body.len().saturating_add(chunk.len()) <= limit,
                "parallel observation {tenant} GET {path} exceeds its response bound"
            );
            body.extend_from_slice(&chunk);
        }
        if retry_parallel_viewer_read(status, &body) {
            eprintln!("parallel observation {tenant} GET {path}: capacity_exhausted");
            return Ok(None);
        }
        ensure!(
            status == reqwest::StatusCode::OK,
            "parallel observation {tenant} GET {path} refused without a retryable capacity code (HTTP {status})"
        );
        Ok(Some(serde_json::from_slice(&body).map_err(|_| {
            anyhow::anyhow!("parallel observation {tenant} GET {path} returned invalid JSON")
        })?))
    }
}

async fn read_parallel_pair<Read>(
    read: &impl Fn(ParallelRead, usize) -> Read,
    route: ParallelRead,
) -> Result<Option<[Value; 2]>>
where
    Read: std::future::Future<Output = Result<Option<Value>>>,
{
    // Drain both bounded reads. Cancelling the HTTP future would not
    // release admission for an already-running gateway Store operation.
    let (left, right) = tokio::join!(read(route, 0), read(route, 1));
    Ok(left?.zip(right?).map(|(left, right)| [left, right]))
}

fn validate_parallel_quota([left, right]: [Value; 2], left_held: u32) -> Result<()> {
    validate_quota(left, 2, left_held)
        .map_err(|_| anyhow::anyhow!("parallel observation tenant-a: invalid quota projection"))?;
    validate_quota(right, 1, 1)
        .map_err(|_| anyhow::anyhow!("parallel observation tenant-b: invalid quota projection"))?;
    Ok(())
}

async fn observe_parallel<Read, Pause>(
    read: impl Fn(ParallelRead, usize) -> Read,
    pause: impl Fn(Duration) -> Pause,
    jobs: &[ParallelJob; 3],
    mut peak_witness: Option<tokio::sync::oneshot::Sender<Result<LivePeak>>>,
) -> Result<[String; 3]>
where
    Read: std::future::Future<Output = Result<Option<Value>>>,
    Pause: std::future::Future<Output = ()>,
{
    timeout(Duration::from_secs(140), async {
        let mut peak = None;
        let mut consecutive_capacity_refusals = 0;
        loop {
            if consecutive_capacity_refusals != 0 {
                ensure!(
                    consecutive_capacity_refusals <= PARALLEL_CAPACITY_RETRIES,
                    "parallel observation exhausted its consecutive capacity retries"
                );
                pause(PARALLEL_POLL).await;
            }
            let Some([left, right]) = read_parallel_pair(&read, ParallelRead::Workers).await?
            else {
                consecutive_capacity_refusals += 1;
                continue;
            };
            let a = workers_from_page(left, 2).map_err(|_| {
                anyhow::anyhow!("parallel observation tenant-a: invalid worker page")
            })?;
            let b = workers_from_page(right, 1).map_err(|_| {
                anyhow::anyhow!("parallel observation tenant-b: invalid worker page")
            })?;
            let live = |worker: &WorkerV1| {
                worker.state == WorkerStateV1::Running
                    && worker.quota_held
                    && worker.attempt.container_id.is_some()
                    && worker
                        .last_observation
                        .as_ref()
                        .is_some_and(|observation| observation.running == Some(true))
            };
            let state = |index: usize| {
                let candidates = if index == 2 { &b } else { &a };
                candidates
                    .iter()
                    .find(|worker| worker.launch.command.argv == jobs[index].argv())
            };
            let (a1, a2, b1) = (state(0), state(1), state(2));
            if let (Some(a1), Some(a2), Some(b1)) = (a1, a2, b1)
                && live(a2)
                && live(b1)
            {
                let ids = [&a1.worker_id, &a2.worker_id, &b1.worker_id];
                if peak.is_none() && live(a1) {
                    let Some(quota) = read_parallel_pair(&read, ParallelRead::Quota).await? else {
                        consecutive_capacity_refusals += 1;
                        continue;
                    };
                    validate_parallel_quota(quota.clone(), 2)?;
                    let ids = ids.map(String::clone);
                    if let Some(signal) = peak_witness.take() {
                        // The observer never waits for the consumer of this paired read.
                        let _ = signal.send(LivePeak::read([a1, a2, b1], quota));
                    }
                    peak = Some(ids);
                } else if let Some(expected) = &peak
                    && a1.state == WorkerStateV1::Terminal
                    && !a1.quota_held
                {
                    ensure!(
                        ids == expected.each_ref(),
                        "live recipe worker identities changed"
                    );
                    let Some(quota) = read_parallel_pair(&read, ParallelRead::Quota).await? else {
                        consecutive_capacity_refusals += 1;
                        continue;
                    };
                    validate_parallel_quota(quota, 1)?;
                    return Ok(expected.clone());
                }
            }
            if a2
                .into_iter()
                .chain(b1)
                .any(|worker| worker.state == WorkerStateV1::Terminal)
            {
                bail!("could not observe A1's normal completion while A2 and B1 remained active");
            }
            consecutive_capacity_refusals = 0;
            pause(PARALLEL_POLL).await;
        }
    })
    .await
    .context("parallel_live_observation_budget")?
}

fn parallel_observer_jobs() -> [ParallelJob; 3] {
    [
        ("A1", "tenant-a", "1"),
        ("A2", "tenant-a", "2"),
        ("B1", "tenant-b", "3"),
    ]
    .map(|(label, tenant, seconds)| ParallelJob {
        label,
        tenant,
        seconds,
        root: PathBuf::new(),
        marker: String::new(),
    })
}

#[tokio::test]
async fn parallel_viewer_reloads_both_tenants_before_signalling_peak() -> Result<()> {
    use agentsandbox::control::{
        AdmissionV1, RuntimeInspectionV1, RuntimeObservationV1, RuntimeStatusV1, WorkerAttemptV1,
    };
    use std::{
        cell::{Cell, RefCell},
        future::ready,
    };
    use tokio::sync::oneshot::{self, error::TryRecvError};

    let jobs = parallel_observer_jobs();
    let fixture: Value = serde_json::from_str(include_str!(
        "fixtures/broker_intent-managed-checkpoint.json"
    ))?;
    let admission: AdmissionV1 = serde_json::from_value(fixture["admission"].clone())?;
    let sample = |generation: u128| {
        std::array::from_fn::<_, 3, _>(|index| {
            let identity = generation * 10 + index as u128 + 1;
            let worker_id = Uuid::from_u128(identity).to_string();
            let mut launch = admission.launch.clone();
            launch.session_id = Uuid::from_u128(identity + 100).to_string();
            launch.command.argv = jobs[index].argv();
            let container_id = format!("{identity:064x}");
            let started_at = "2026-11-20T00:00:00Z".to_owned();
            let inspection = RuntimeInspectionV1 {
                authority_id: launch.authority_id.clone(),
                runtime_kind: launch.runtime_kind,
                container_id: container_id.clone(),
                stable_name: format!("worker-{identity}"),
                volume_name: format!("volume-{identity}"),
                image_id: launch.image_id.clone(),
                spec_hash: "d".repeat(64),
                cpu_millis: launch.cpu_millis,
                memory_bytes: launch.memory_bytes,
                pids_limit: launch.pids_limit,
                network: launch.network,
                security_preset: launch.security_preset,
                restart_policy: launch.restart_policy,
            };
            WorkerV1 {
                worker_id,
                admission_operation_id: admission.operation_id.clone(),
                request_id: admission.decision_request.request_id.clone(),
                receipt: admission.receipt.clone(),
                launch,
                spec_hash: inspection.spec_hash.clone(),
                policy_hash: "e".repeat(64),
                state: WorkerStateV1::Running,
                quota_held: true,
                revision: 5,
                attempt: WorkerAttemptV1 {
                    attempt_id: Uuid::from_u128(identity + 200).to_string(),
                    stable_name: inspection.stable_name.clone(),
                    volume_name: inspection.volume_name.clone(),
                    container_id: Some(container_id),
                    created_inspection: Some(inspection.clone()),
                    create_operation_id: None,
                    start_operation_id: None,
                    start_emitter_id: None,
                },
                observed_started_at: Some(started_at.clone()),
                allocation_inconsistent: false,
                last_observation: Some(RuntimeObservationV1 {
                    inspection,
                    observed_at: started_at.clone(),
                    status: RuntimeStatusV1::Running,
                    running: Some(true),
                    started_at: Some(started_at),
                    finished_at: None,
                    exit_code: None,
                    oom_killed: Some(false),
                    restart_count: Some(0),
                }),
                unknown_reason: None,
                cleanup_removed: None,
                cleanup_error: None,
            }
        })
    };
    let discarded = sample(1);
    let fresh = sample(2);
    let mut terminal = fresh.clone();
    terminal[0].state = WorkerStateV1::Terminal;
    terminal[0].quota_held = false;
    let observation = terminal[0].last_observation.as_mut().unwrap();
    observation.status = RuntimeStatusV1::Exited;
    observation.running = Some(false);
    observation.exit_code = Some(0);
    observation.finished_at = Some("2026-11-20T00:00:01Z".into());
    let expected = fresh.each_ref().map(|worker| worker.worker_id.clone());
    let page = |workers: &[WorkerV1]| Some(json!({"items": workers, "next_cursor": null}));
    let quota = |configured: u64, held: u64| {
        Some(json!({
            "policy": {
                "version": 1, "max_active_workers": configured, "max_cpu_millis": configured * WORKER_CPU,
                "max_memory_bytes": configured * WORKER_MEMORY, "max_timeout_seconds": 60,
            },
            "held_workers": held, "held_cpu_millis": held * WORKER_CPU, "held_memory_bytes": held * WORKER_MEMORY,
        }))
    };
    let capacity = || {
        assert!(retry_parallel_viewer_read(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            PARALLEL_TEST_CAPACITY,
        ));
        None
    };
    let script = [
        (ParallelRead::Workers, [page(&discarded[..2]), capacity()]),
        (
            ParallelRead::Workers,
            [page(&discarded[..2]), page(&discarded[2..])],
        ),
        (ParallelRead::Quota, [capacity(), quota(1, 1)]),
        (
            ParallelRead::Workers,
            [page(&fresh[..2]), page(&fresh[2..])],
        ),
        (ParallelRead::Quota, [quota(2, 2), quota(1, 1)]),
        (
            ParallelRead::Workers,
            [page(&terminal[..2]), page(&terminal[2..])],
        ),
        (ParallelRead::Quota, [quota(2, 1), quota(1, 1)]),
    ];
    let progress = RefCell::new((0, [false; 2]));
    let pauses = Cell::new(0);
    let (peak_tx, peak_rx) = oneshot::channel();
    let peak_rx = RefCell::new(peak_rx);
    let observed = observe_parallel(
        |route, tenant| {
            let (step, seen) = &mut *progress.borrow_mut();
            let (expected_route, replies) = script
                .get(*step)
                .expect("unexpected read after final quota");
            assert_eq!(route, *expected_route);
            assert!(
                !std::mem::replace(&mut seen[tenant], true),
                "tenant read twice in one pair"
            );
            let reply = replies[tenant].clone();
            if *seen == [true; 2] {
                *step += 1;
                *seen = [false; 2];
            }
            ready(Ok(reply))
        },
        |delay| {
            let wait = pauses.get();
            assert_eq!(delay, Duration::from_millis(250));
            assert_eq!(*progress.borrow(), ([1, 3, 5][wait], [false; 2]));
            let signal = peak_rx.borrow_mut().try_recv();
            if wait < 2 {
                assert!(
                    matches!(signal, Err(TryRecvError::Empty)),
                    "partial view signalled peak"
                );
            } else {
                let peak = signal
                    .expect("complete fresh pair must signal peak")
                    .expect("fresh peak must decode");
                assert_eq!(
                    peak.quotas_at_peak,
                    [quota(2, 2).unwrap(), quota(1, 1).unwrap()]
                );
                assert_eq!(
                    peak.workers.each_ref().map(|row| row.container_id.as_str()),
                    fresh
                        .each_ref()
                        .map(|worker| worker.attempt.container_id.as_deref().unwrap())
                );
                assert_eq!(peak.workers.map(|row| row.worker_id), expected);
            }
            pauses.set(wait + 1);
            ready(())
        },
        &jobs,
        Some(peak_tx),
    )
    .await?;
    assert_eq!(observed, expected);
    assert_eq!(*progress.borrow(), (script.len(), [false; 2]));
    assert_eq!(pauses.get(), 3);
    Ok(())
}

#[tokio::test]
async fn parallel_viewer_capacity_retries_stop_without_signalling_peak() {
    use std::{cell::Cell, future::ready};
    use tokio::sync::oneshot::{self, error::TryRecvError};

    let jobs = parallel_observer_jobs();
    let reads = [Cell::new(0), Cell::new(0)];
    let waits = Cell::new(0);
    let (peak_tx, mut peak_rx) = oneshot::channel();
    let error = observe_parallel(
        |route, tenant| {
            assert_eq!(route, ParallelRead::Workers);
            reads[tenant].set(reads[tenant].get() + 1);
            ready(Ok(None))
        },
        |delay| {
            assert_eq!(delay, Duration::from_millis(250));
            waits.set(waits.get() + 1);
            ready(())
        },
        &jobs,
        Some(peak_tx),
    )
    .await
    .unwrap_err();
    assert_eq!(reads.map(|count| count.get()), [4, 4]);
    assert_eq!(waits.get(), 3);
    assert!(
        error
            .to_string()
            .contains("exhausted its consecutive capacity retries")
    );
    assert!(matches!(peak_rx.try_recv(), Err(TryRecvError::Closed)));
}

struct LiveDashboard {
    child: Child,
    output: BufReader<tokio::process::ChildStdout>,
    stop_server: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

const LIVE_BROWSER_MAX_REGISTRY_REQUESTS: u64 = 64;

fn checked_live_browser_view(view: &Value, visible_rows: u64) -> Result<Value> {
    let requests = view["requests"]
        .as_u64()
        .filter(|count| (1..=LIVE_BROWSER_MAX_REGISTRY_REQUESTS).contains(count))
        .context("invalid live page request count")?;
    let responses = view["responses"]
        .as_u64()
        .filter(|count| (1..=requests).contains(count))
        .context("invalid live page response count")?;
    ensure!(
        view["matching_reads"] == 1
            && view["fresh"] == true
            && view["visible_rows"] == visible_rows,
        "live page visibility, freshness or response binding not attested"
    );
    let date = |field: &str| {
        view[field]
            .as_str()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
    };
    Ok(json!({
        "evaluated_at": date("evaluated_at").context("invalid gateway read date")?,
        "rendered_at": date("rendered_at").context("invalid browser assertion date")?,
        "requests": requests, "responses": responses, "matching_reads": 1,
        "fresh": true, "visible_rows": visible_rows,
    }))
}

#[test]
fn live_browser_view_requires_the_rendered_read_even_after_revalidation() -> Result<()> {
    let mut view = json!({
        "evaluated_at": "2026-11-20T09:00:01.123Z",
        "rendered_at": "2026-11-20T09:00:01.234Z",
        "requests": 3, "responses": 2, "matching_reads": 1, "fresh": true, "visible_rows": 2,
    });
    let accepted = checked_live_browser_view(&view, 2)?;
    assert_eq!(accepted["requests"], 3);
    assert_eq!(accepted["responses"], 2);
    assert_eq!(accepted["matching_reads"], 1);
    assert!(checked_live_browser_view(&view, 1).is_err());
    view["matching_reads"] = json!(2);
    assert!(checked_live_browser_view(&view, 2).is_err());
    view["matching_reads"] = json!(1);
    view["fresh"] = json!(false);
    assert!(checked_live_browser_view(&view, 2).is_err());
    view["fresh"] = json!(true);
    view["responses"] = json!(4);
    assert!(checked_live_browser_view(&view, 2).is_err());
    view["requests"] = json!(65);
    assert!(checked_live_browser_view(&view, 2).is_err());
    Ok(())
}

fn live_browser_failure(report: &Value) -> anyhow::Error {
    let status = if report["status"] == "BLOCKED" {
        "BLOCKED"
    } else {
        "FAIL"
    };
    let stage = match report["stage"].as_str() {
        Some(
            stage @ ("setup" | "launch" | "ready" | "peak" | "await_native" | "logout" | "cleanup"
            | "network"),
        ) => stage,
        _ => "unknown",
    };
    let privacy = match report["capture_privacy"].as_str() {
        Some(privacy @ ("sanitized_private_html_only" | "not_captured")) => privacy,
        _ => "unknown",
    };
    let cleanup_failed = report["cleanup_failed"] == true;
    let network_violation = report["network_violation"] == true;
    let diagnostics = if stage == "peak" {
        report["peak_reads"]
            .as_array()
            .filter(|reads| reads.len() == 2)
            .map(|reads| {
                let safe = reads
                    .iter()
                    .zip(["A", "B"])
                    .filter(|(read, tenant)| read["tenant"].as_str() == Some(*tenant))
                    .map(|(read, tenant)| {
                        let code = match read["code"].as_str() {
                            Some(code @ ("registry_wait" | "registry_status" | "registry_shape"
                                | "worker_binding" | "dom_timeout" | "dom_error" | "read_count"
                                | "evaluation_ambiguous" | "matched")) => code,
                            _ => "unexpected",
                        };
                        let dom_state = match read["dom_state"].as_str() {
                            Some(state @ ("loading" | "stale" | "transient" | "status_hidden"
                                | "evaluation_changed" | "evaluation_hidden" | "table_hidden"
                                | "row_count" | "row_identity" | "row_visibility" | "ready")) => state,
                            _ => "unseen",
                        };
                        let http_status = read["http_status"].as_u64().filter(|value| (100..=599).contains(value));
                        let count = |field: &str| read[field].as_u64().filter(|value| *value <= 255);
                        json!({"tenant":tenant,"code":code,"dom_state":dom_state,"http_status":http_status,
                            "requests":count("requests"),"responses":count("responses")})
                    })
                    .collect::<Vec<_>>();
                format!("; peak_reads={}", json!(safe))
            })
            .unwrap_or_default()
    } else if stage == "logout" {
        report["logout_checks"]
            .as_array()
            .filter(|checks| checks.len() == 2)
            .map(|checks| {
                let codes: Vec<_> = checks
                    .iter()
                    .map(|check| match check.as_str() {
                        Some(
                            code @ ("stream_not_open" | "logout_click" | "login_or_scope"
                            | "stream_not_aborted" | "stream_reopened" | "browser_guard"
                            | "verified"),
                        ) => code,
                        _ => "unexpected",
                    })
                    .collect();
                format!("; logout_checks_ab={}", json!(codes))
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    anyhow::anyhow!(
        "live dashboard {status} at {stage}; capture={privacy}; cleanup_failed={cleanup_failed}; network_violation={network_violation}{diagnostics}"
    )
}

fn require_live_logout(report: &Value) -> Result<()> {
    if *report != json!({"status":"LOGGED_OUT","tenants":2}) {
        return Err(live_browser_failure(report));
    }
    Ok(())
}

fn require_live_browser_close(report: &Value, native_confirmed: bool) -> Result<()> {
    let status = if native_confirmed { "PASS" } else { "CLOSED" };
    if *report != json!({"status":status,"stage":"complete"}) {
        return Err(live_browser_failure(report));
    }
    Ok(())
}

#[test]
fn live_browser_logout_protocol_requires_separate_purge_and_cleanup_witnesses() -> Result<()> {
    let previous_success = json!({"status":"PASS","stage":"complete"});
    let closed = json!({"status":"CLOSED","stage":"complete"});
    let logout = json!({"status":"LOGGED_OUT","tenants":2});
    require_live_logout(&logout)?;
    require_live_browser_close(&previous_success, true)?;
    require_live_browser_close(&closed, false)?;
    assert!(require_live_logout(&previous_success).is_err());
    assert!(require_live_logout(&json!({"status":"LOGGED_OUT","tenants":1})).is_err());
    assert!(require_live_browser_close(&logout, true).is_err());
    assert!(require_live_browser_close(&closed, true).is_err());
    assert!(require_live_browser_close(&previous_success, false).is_err());
    Ok(())
}

impl LiveDashboard {
    async fn send(&mut self, frame: Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(&frame)?;
        ensure!(
            bytes.len() < 16 * 1024,
            "live dashboard input exceeds its bound"
        );
        bytes.push(b'\n');
        self.child
            .stdin
            .as_mut()
            .context("live dashboard input closed")?
            .write_all(&bytes)
            .await?;
        Ok(())
    }

    async fn receive(&mut self) -> Result<Value> {
        let mut bytes = Vec::new();
        (&mut self.output)
            .take(4097)
            .read_until(b'\n', &mut bytes)
            .await?;
        ensure!(
            bytes.len() <= 4096 && bytes.last() == Some(&b'\n'),
            "missing complete live dashboard verdict"
        );
        serde_json::from_slice(&bytes).context("invalid live dashboard verdict")
    }

    async fn observe(
        mut self,
        signal: tokio::sync::oneshot::Receiver<Result<LivePeak>>,
        selection: &RuntimeSelection,
        doctor: &RuntimeDoctor,
    ) -> Result<Value> {
        let outcome = async {
            let LivePeak { workers, quotas_at_peak } = signal
                .await
                .context("Rust did not observe the three-worker peak")??;
            let displayed = timeout(Duration::from_secs(28), async {
                self.send(json!({"phase":"peak","workers":workers})).await?;
                self.receive().await
            })
            .await
            .context("live dashboard watchdog expired")??;
            if displayed["status"] != "DISPLAYED" || displayed["rows"] != json!([2, 1]) {
                return Err(live_browser_failure(&displayed));
            }
            let views = displayed["views"].as_array().context("missing live page stamps")?;
            ensure!(views.len() == 2, "missing actor scope in browser report");
            let stamps: Vec<_> = views
                .iter()
                .zip([2, 1])
                .map(|(view, visible_rows)| checked_live_browser_view(view, visible_rows))
                .collect::<Result<_>>()?;
            // The pipe barrier puts native inspection after the displayed witness.
            let (inspect_stage, effect): (_, Result<Vec<LiveNativeSnapshot>>) = match selection.runtime {
                RuntimeKind::Docker => ("docker_inspect_after_dom", selection
                    .inspect_live_workers(&workers).await
                    .map(|facts| facts.into_iter().map(LiveNativeSnapshot::Docker).collect())),
                RuntimeKind::Podman => ("podman_inspect_after_dom", podman_live::inspect(
                    selection, doctor, [&workers[..2], &workers[2..]], quotas_at_peak,
                ).await.map(|facts| facts.into_iter().map(LiveNativeSnapshot::Podman).collect())),
            };
            let runtime_kind = selection.runtime_kind();
            let native_confirmed = effect.is_ok();
            let mut logout_observed = false;
            let browser_exit = async {
                timeout(Duration::from_secs(15), async {
                    let phase = if native_confirmed { "logout_close" } else { "close" };
                    self.send(json!({"phase":phase})).await?;
                    if native_confirmed {
                        require_live_logout(&self.receive().await?)?;
                        logout_observed = true;
                    }
                    require_live_browser_close(&self.receive().await?, native_confirmed)
                })
                .await
                .context("live dashboard logout or closure report missing")?
            }.await;
            let facts = match effect {
                Ok(facts) => facts,
                Err(error) => {
                    eprintln!("{}", json!({"status":"FAIL","stage":inspect_stage,"runtime_kind":runtime_kind,"browser":"DISPLAYED",
                        "browser_reported_closed":browser_exit.is_ok(),"rows":[2,1],"views":stamps}));
                    if let Err(closure) = browser_exit { eprintln!("live dashboard secondary closure: {closure}"); }
                    return Err(error.context("native overlap not confirmed after the browser witness"));
                }
            };
            if let Err(error) = browser_exit {
                eprintln!(
                    "{}",
                    json!({
                        "status": "FAIL",
                        "stage": if logout_observed { "browser_close_after_native" } else { "browser_logout_after_native" },
                        "runtime_kind": runtime_kind,
                        "browser": "DISPLAYED",
                        "rows": [2, 1],
                        "views": stamps,
                        "native_match_count": facts.len(),
                        "logged_out": if logout_observed { 2 } else { 0 },
                    })
                );
                return Err(error);
            }
            Ok::<_, anyhow::Error>(json!({"status":"PASS","mode":"overlap_viewer","runtime_kind":runtime_kind,"rows":[2,1],
                "views":stamps,"inspected_after_dom":facts,"logged_out":2}))
        }
        .await;
        let exit = self.close().await;
        let report = outcome.inspect_err(|_| {
            if exit.is_err() {
                eprintln!("live dashboard cleanup also failed after the primary verdict");
            }
        })?;
        ensure!(exit?.success(), "live dashboard did not close successfully");
        Ok(report)
    }

    async fn close(mut self) -> Result<std::process::ExitStatus> {
        drop(self.child.stdin.take());
        let child_result = async {
            if let Ok(exit) = timeout(Duration::from_secs(8), self.child.wait()).await {
                return Ok(exit?);
            }
            if let Some(pid) = self
                .child
                .id()
                .and_then(|id| libc::pid_t::try_from(id).ok())
                .filter(|id| *id > 1)
            {
                // SAFETY: this is the positive PID of the owned, unreaped Node child.
                unsafe {
                    libc::kill(pid, libc::SIGTERM);
                }
            }
            if let Ok(exit) = timeout(Duration::from_secs(10), self.child.wait()).await {
                return Ok(exit?);
            }
            let _ = self.child.start_kill();
            let _ = timeout(Duration::from_secs(5), self.child.wait()).await;
            bail!("live dashboard browser cleanup not confirmed");
        }
        .await;
        let _ = self.stop_server.send(());
        match timeout(Duration::from_secs(5), &mut self.server).await {
            Ok(stopped) => stopped??,
            Err(_) => {
                self.server.abort();
                let _ = self.server.await;
                bail!("live dashboard server shutdown not confirmed");
            }
        }
        child_result
    }
}

struct DashboardCheck {
    front: PathBuf,
    node: PathBuf,
    modules: PathBuf,
    chromium: PathBuf,
    script: PathBuf,
}

impl DashboardCheck {
    fn read() -> Result<Option<Self>> {
        let Some(front) = std::env::var_os("AGENTSANDBOX_TEST_BROWSER_DIST") else {
            return Ok(None);
        };
        let front = PathBuf::from(front);
        ensure!(
            front.is_absolute() && front.join("index.html").is_file(),
            "QA frontend dist required"
        );
        let input = |name: &str| -> Result<PathBuf> {
            let path =
                PathBuf::from(std::env::var_os(name).with_context(|| format!("missing {name}"))?);
            ensure!(path.is_absolute(), "{name} must be absolute");
            Ok(path)
        };
        let node = input("AGENTSANDBOX_TEST_BROWSER_NODE")?;
        let modules = input("CODEX_PRIMARY_RUNTIME_NODE_MODULES")?;
        let chromium = input("PLAYWRIGHT_CHROMIUM_EXECUTABLE")?;
        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("front/tests/browser_runtime_journey.mjs");
        ensure!(
            node.is_file()
                && chromium.is_file()
                && script.is_file()
                && modules.join("playwright/package.json").is_file(),
            "QA Chromium driver or dependencies absent"
        );
        Ok(Some(Self {
            front,
            node,
            modules,
            chromium,
            script,
        }))
    }

    async fn start_live(
        &self,
        fixture: &Path,
        upstream: &str,
        viewers: &[String; 2],
        runtime_kind: RuntimeKindV1,
    ) -> Result<LiveDashboard> {
        let script = self
            .script
            .with_file_name("browser_runtime_live_window.mjs");
        ensure!(script.is_file(), "live dashboard conductor absent");
        let root = Directory::open_existing(fixture)?.child("dashboard-live")?;
        let home = root.child("home")?;
        let tmp = root.child("tmp")?;
        let artifacts = root.child("artifacts")?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let router = agentsandbox::api::dashboard_shared_router(upstream, self.front.clone())?;
        let mut child = Command::new(&self.node)
            .arg(script)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", home.path())
            .env("TMPDIR", tmp.path())
            .env("CODEX_PRIMARY_RUNTIME_NODE_MODULES", &self.modules)
            .env("PLAYWRIGHT_CHROMIUM_EXECUTABLE", &self.chromium)
            .env("AI_GUARD_TEST_ARTIFACT_DIR", artifacts.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let output = BufReader::new(
            child
                .stdout
                .take()
                .context("live dashboard output missing")?,
        );
        let (stop_server, shutdown) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = shutdown.await;
                })
                .await
        });
        let mut live = LiveDashboard {
            child,
            output,
            stop_server,
            server,
        };
        let ready = timeout(Duration::from_secs(40), async {
            live.send(json!({"version":1,"dashboard":origin,"viewers":[viewers[0],viewers[1]],"runtime_kind":runtime_kind}))
                .await?;
            live.receive().await
        })
        .await;
        match ready {
            Ok(Ok(report)) if report == json!({"status":"READY","tenants":2}) => Ok(live),
            failed => {
                let primary = match failed {
                    Ok(Ok(report)) => live_browser_failure(&report),
                    _ => anyhow::anyhow!("live dashboard was not ready before worker submission"),
                };
                if live.close().await.is_err() {
                    eprintln!("live dashboard cleanup also failed after readiness failure");
                }
                Err(primary)
            }
        }
    }

    async fn terminal(
        &self,
        fixture: &Path,
        upstream: &str,
        viewers: &[String; 2],
        workers: &[Value],
        runtime_kind: RuntimeKindV1,
    ) -> Result<Value> {
        let root = Directory::open_existing(fixture)?.child("dashboard-proof")?;
        let home = root.child("home")?;
        let tmp = root.child("tmp")?;
        let artifacts = root.child("artifacts")?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let router = agentsandbox::api::dashboard_shared_router(upstream, self.front.clone())?;
        let input = serde_json::to_vec(&json!({
            "version":1, "dashboard":origin, "viewers":{"tenant-a":viewers[0],"tenant-b":viewers[1]},
            "workers":workers, "runtime_kind":runtime_kind,
        }))?;
        ensure!(
            input.len() <= 16 * 1024,
            "QA browser input exceeds its bound"
        );
        let (stop, shutdown) = tokio::sync::oneshot::channel();
        let mut server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = shutdown.await;
                })
                .await
        });
        let report = self
            .run_browser(
                home.path(),
                tmp.path(),
                artifacts.path(),
                &input,
                runtime_kind,
            )
            .await;
        let _ = stop.send(());
        match timeout(Duration::from_secs(5), &mut server).await {
            Ok(stopped) => stopped??,
            Err(_) => {
                server.abort();
                let _ = server.await;
                bail!("QA dashboard did not shut down");
            }
        }
        report
    }

    async fn run_browser(
        &self,
        home: &Path,
        tmp: &Path,
        artifacts: &Path,
        input: &[u8],
        runtime_kind: RuntimeKindV1,
    ) -> Result<Value> {
        let mut child = Command::new(&self.node)
            .arg(&self.script)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", home)
            .env("TMPDIR", tmp)
            .env("CODEX_PRIMARY_RUNTIME_NODE_MODULES", &self.modules)
            .env("PLAYWRIGHT_CHROMIUM_EXECUTABLE", &self.chromium)
            .env("AI_GUARD_TEST_ARTIFACT_DIR", artifacts)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let result = timeout(Duration::from_secs(120), async {
            let mut stdin = child.stdin.take().context("QA browser stdin unavailable")?;
            stdin.write_all(input).await?;
            drop(stdin);
            let stdout = child
                .stdout
                .take()
                .context("QA browser stdout unavailable")?;
            let mut output = Vec::new();
            stdout.take(4097).read_to_end(&mut output).await?;
            ensure!(output.len() <= 4096, "QA browser verdict exceeds its bound");
            Ok::<_, anyhow::Error>((child.wait().await?, output))
        })
        .await;
        let (exit, output) = match result {
            Ok(Ok(pair)) => pair,
            _ => {
                // Default Playwright SIGTERM handling closes its detached Chromium first.
                if let Some(pid) = child
                    .id()
                    .and_then(|id| libc::pid_t::try_from(id).ok())
                    .filter(|id| *id > 1)
                {
                    // SAFETY: the owned Child still has an unreaped, positive PID.
                    unsafe {
                        libc::kill(pid, libc::SIGTERM);
                    }
                    if !matches!(
                        timeout(Duration::from_secs(10), child.wait()).await,
                        Ok(Ok(_))
                    ) {
                        let _ = child.start_kill();
                        let _ = timeout(Duration::from_secs(5), child.wait()).await;
                        bail!("QA browser watchdog expired; Chromium cleanup not confirmed");
                    }
                }
                bail!("QA browser conduit or watchdog failed");
            }
        };
        let report: Value =
            serde_json::from_slice(&output).context("invalid QA browser verdict")?;
        let stage = match report["stage"].as_str() {
            Some(
                stage @ ("input" | "launch" | "viewer_a" | "viewer_b" | "complete" | "cleanup"
                | "network"),
            ) => stage,
            _ => "unknown",
        };
        ensure!(
            exit.success()
                && stage == "complete"
                && report["status"] == "PASS"
                && report["workers"] == 3
                && report["tenant_denials"] == 3
                && report["audit_events"] == 6
                && report["runtime_kind"] == json!(runtime_kind),
            "QA browser failed at {stage}; see private dashboard-proof if captured"
        );
        Ok(
            json!({"status":"PASS", "mode":"terminal_viewer", "runtime_kind":runtime_kind,
                "workers":3, "tenant_denials":3, "audit_events":6}),
        )
    }
}

struct ExplicitStop {
    peak: [LiveWorker; 3],
    start_operation_id: String,
    start_emitter_id: String,
    started_seq: i64,
    monitor_epoch_id: Uuid,
    receipt: MutationReceiptV1,
    terminal_from_stop: Option<RuntimeObservationV1>,
}

async fn running_a1(viewer: &AuditClient, a1: &LiveWorker) -> Result<WorkerV1> {
    let worker: WorkerV1 = serde_json::from_value(
        viewer
            .get_json(&format!("api/v1/workers/{}", a1.worker_id))
            .await?,
    )?;
    ensure!(
        worker.state == WorkerStateV1::Running
            && worker.quota_held
            && worker
                .last_observation
                .as_ref()
                .is_some_and(|item| item.running == Some(true))
            && LiveWorker::read(&worker)? == *a1,
        "A1 was no longer the original Running instance before stop"
    );
    Ok(worker)
}

fn open_a1_monitor_epoch(
    config: &Path,
    origin: &reqwest::Url,
    tenant: &str,
    worker: &WorkerV1,
) -> Result<Option<Uuid>> {
    let filename = format!("{}.json", worker.launch.session_id);
    let path = config.join("worker-control-monitor").join(&filename);
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("cannot check A1 monitor journal"),
    }
    let file = PrivateFile::open(&path, 4096)?;
    let journal: Value = serde_json::from_slice(file.bytes())?;
    let epoch_id: Uuid = serde_json::from_value(journal["epoch_id"].clone())?;
    ensure!(!epoch_id.is_nil(), "A1 monitor epoch identity is missing");

    // Match the context of the production monitor, using its private
    // checkpoint and the worker identity already observed in the gateway.
    let plan = PrivateFile::open(
        &config.join("sessions").join(filename),
        MAX_PRIVATE_FILE_BYTES,
    )?;
    let plan: Value = serde_json::from_slice(plan.bytes())?;
    let checkpoint = &plan["managed"];
    ensure!(
        plan["session"] == worker.launch.session_id
            && plan["operation_id"] == worker.admission_operation_id
            && checkpoint["tenant_id"] == tenant
            && checkpoint["gateway_origin"] == origin.as_str()
            && checkpoint["config_root"] == serde_json::to_value(config)?
            && checkpoint["binding"].is_object()
            && checkpoint["owned"].is_object()
            && checkpoint["lock_identity"].is_object(),
        "A1 monitor checkpoint does not bind the live session"
    );
    let context_hash = content_hash(
        "ai-guard:worker-control-monitor-context:v1",
        &json!({
            "tenant": checkpoint["tenant_id"], "origin": checkpoint["gateway_origin"],
            "config": checkpoint["config_root"], "session": plan["session"],
            "operation": plan["operation_id"], "worker": worker.worker_id,
            "attempt": worker.attempt, "launch": worker.launch, "spec_hash": worker.spec_hash,
            "binding": checkpoint["binding"], "owned": checkpoint["owned"],
            "lock_identity": checkpoint["lock_identity"],
        }),
    )?;
    ensure!(
        journal
            == json!({
                "version":1, "context_hash":context_hash, "epoch_id":epoch_id, "revision":1,
                "operation_id":epoch_id, "operation_hash":content_hash(
                    "ai-guard:worker-control-monitor-open:v1", &epoch_id
                )?,
                "open":true, "pending":null, "intervals":[],
            }),
        "A1 monitor is not the fresh open epoch for the observed attempt"
    );
    Ok(Some(epoch_id))
}

fn stopped_runtime_observation(worker: &WorkerV1, native: &Value) -> Result<RuntimeObservationV1> {
    let observed: ContainerObservation = serde_json::from_value(native.clone())?;
    let durable = worker
        .last_observation
        .as_ref()
        .context("A1 CLI terminal acknowledgement has no durable observation")?;
    let process = &observed.process;
    ensure!(
        durable.status == RuntimeStatusV1::Exited
            && (process.status.as_deref() == Some("exited")
                || worker.launch.runtime_kind == RuntimeKindV1::Podman
                    && process.status.as_deref() == Some("stopped"))
            && durable.running == Some(false)
            && process.running == durable.running
            && durable.oom_killed == Some(false)
            && process.oom_killed == durable.oom_killed
            && durable.restart_count == Some(0)
            && observed.restart_count == durable.restart_count.map(u64::from)
            && durable.exit_code.is_some_and(|code| code != 0)
            && process.exit_code.map(i64::from) == durable.exit_code
            && observed.id.as_deref() == Some(durable.inspection.container_id.as_str())
            && observed.name == durable.inspection.stable_name
            && observed.image_id.as_deref() == Some(durable.inspection.image_id.as_str())
            && process
                .started_at
                .as_ref()
                .zip(durable.started_at.as_deref())
                .is_some_and(|(raw, saved)| same_started_at(&raw.to_rfc3339(), saved))
            && process
                .finished_at
                .as_ref()
                .zip(durable.finished_at.as_deref())
                .is_some_and(|(raw, saved)| same_started_at(&raw.to_rfc3339(), saved)),
        "A1 CLI terminal observation differs from the durable instance and result"
    );
    Ok(durable.clone())
}

async fn request_a1_stop(
    peak: tokio::sync::oneshot::Receiver<Result<LivePeak>>,
    gateway: &Gateway,
    selection: &RuntimeSelection,
    job: &ParallelJob,
    viewer: &AuditClient,
) -> Result<ExplicitStop> {
    let peak = peak
        .await
        .context("three-running-worker peak not observed")??
        .workers;
    let a1 = &peak[0];
    let config = absolute_path(&job.config())?;
    let origin = reqwest::Url::parse(&gateway.origin)?;
    let (running, started_seq, monitor_epoch_id) = timeout(CLI_TIMEOUT, async {
        loop {
            let worker = running_a1(viewer, a1).await?;
            let history = viewer
                .get_json(&format!(
                    "api/v1/events?sandbox_id={}&kind=command.started&limit=2",
                    a1.session_id,
                ))
                .await?;
            let items = history["items"]
                .as_array()
                .context("A1 start audit page missing")?;
            if !items.is_empty() {
                ensure!(
                    history["next_cursor"].is_null(),
                    "A1 start audit page incomplete"
                );
                let event = one_event(items, "command.started")?;
                ensure!(
                    event["session_id"] == a1.session_id
                        && event["data"]["running"] == true
                        && event["data"]["runtime_started_at"]
                            .as_str()
                            .is_some_and(|started| same_started_at(started, &a1.started_at)),
                    "A1 start acknowledgement does not bind the observed instance"
                );
                let sequence = event["seq"]
                    .as_i64()
                    .filter(|seq| *seq > 0)
                    .context("A1 start audit sequence absent")?;
                if let Some(epoch_id) =
                    open_a1_monitor_epoch(&config, &origin, job.tenant, &worker)?
                {
                    let running = running_a1(viewer, a1).await?;
                    ensure!(
                        running.attempt == worker.attempt && running.spec_hash == worker.spec_hash,
                        "A1 no longer matches the attested epoch"
                    );
                    break Ok::<_, anyhow::Error>((running, sequence, epoch_id));
                }
            }
            sleep(PARALLEL_POLL).await;
        }
    })
    .await
    .context(
        "A1 start acknowledgement and open monitor epoch did not arrive during the stop window",
    )??;
    let start_operation_id = running
        .attempt
        .start_operation_id
        .clone()
        .context("A1 start claim absent")?;
    let start_emitter_id = running
        .attempt
        .start_emitter_id
        .clone()
        .context("A1 start emitter absent")?;
    let mut command = gateway.command("worker-stop", &job.config());
    command
        .args(["--session", &a1.session_id, "--runtime-program"])
        .arg(&selection.program);
    let acknowledgement: Value =
        serde_json::from_slice(&checked_output(command, "A1 worker-stop", CLI_TIMEOUT).await?)?;
    let acknowledged: WorkerV1 = serde_json::from_value(acknowledgement["worker"].clone())?;
    ensure!(
        acknowledgement["session_id"] == a1.session_id
            && acknowledgement["tenant_id"] == job.tenant
            && acknowledgement["gateway_origin"].as_str() == Some(origin.as_str())
            && acknowledgement["registry_observation_scope"] == "last_durably_recorded"
            && LiveWorker::read(&acknowledged)? == *a1
            && acknowledged.attempt == running.attempt
            && acknowledged.spec_hash == running.spec_hash,
        "A1 CLI did not acknowledge a stop for the live owned attempt"
    );

    let journal = Directory::open_existing(&job.config().join("worker-stop-intents"))?;
    let files = fs::read_dir(journal.path())?
        .take(3)
        .collect::<std::io::Result<Vec<_>>>()?;
    ensure!(
        files.len() == 2,
        "expected one stop intent and its acknowledgement"
    );
    let intent = files
        .iter()
        .find(|entry| {
            let name = entry.file_name();
            name.to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .is_some_and(|stem| Uuid::parse_str(stem).is_ok_and(|id| id.to_string() == stem))
        })
        .context("private stop intent absent")?;
    let file = PrivateFile::open(&intent.path(), 4096)?;
    let mutation: WorkerMutationV1 = serde_json::from_slice(file.bytes())?;
    let intent_name = format!("{}.json", mutation.operation_id);
    ensure!(
        intent.file_name().to_str() == Some(intent_name.as_str())
            && mutation.version == 1
            && mutation.worker_id == a1.worker_id
            && mutation.attempt_id == a1.attempt_id
            && matches!(mutation.transition, WorkerTransitionV1::Stop)
            && mutation.expected_revision >= running.revision,
        "private stop intent differs from A1 acknowledgement"
    );
    let ack_file = PrivateFile::open(
        &journal
            .path()
            .join(format!("{}-ack.json", mutation.operation_id)),
        4096,
    )?;
    let receipt: MutationReceiptV1 = serde_json::from_slice(ack_file.bytes())?;
    let replay: MutationReceiptV1 = serde_json::from_value(
        viewer
            .get_json(&format!(
                "api/v1/workers/operations/{}",
                mutation.operation_id
            ))
            .await?,
    )?;
    let stop_revision = mutation
        .expected_revision
        .checked_add(1)
        .context("A1 stop revision overflow")?;
    ensure!(
        replay == receipt
            && receipt.operation_id == mutation.operation_id
            && receipt.worker_id.as_deref() == Some(a1.worker_id.as_str())
            && receipt.revision == Some(stop_revision)
            && receipt.input_hash == content_hash("ai-guard:worker-mutation:v1", &mutation)?
            && receipt.audit_seq > started_seq,
        "A1 stop lacks a matching durable acknowledgement after start"
    );
    let native = acknowledgement
        .get("runtime_observation")
        .context("A1 stop acknowledgement lacks its runtime observation field")?;
    let terminal_from_stop = match acknowledged.state {
        WorkerStateV1::Stopping
            if acknowledged.quota_held
                && acknowledged.revision == stop_revision
                && native.is_null() =>
        {
            None
        }
        WorkerStateV1::Terminal
            if !acknowledged.quota_held && acknowledged.revision > stop_revision =>
        {
            Some(stopped_runtime_observation(&acknowledged, native)?)
        }
        _ => bail!("A1 CLI stop acknowledgement conflicts with its durable receipt"),
    };
    Ok(ExplicitStop {
        peak,
        start_operation_id,
        start_emitter_id,
        started_seq,
        monitor_epoch_id,
        receipt,
        terminal_from_stop,
    })
}

async fn stopped_a1_summary(
    gateway: &Gateway,
    job: &ParallelJob,
    capture: Output,
    stop: &ExplicitStop,
) -> Result<RunSummary> {
    // A stopped business process causes the `run` command to fail normally.
    // A signal to the host CLI cannot satisfy this process-exit oracle.
    ensure!(
        capture.status.code() == Some(1),
        "A1 run did not return the normal CLI error code"
    );
    let a1 = &stop.peak[0];
    let mut command = gateway.command("worker-status", &job.config());
    command.args(["--session", &a1.session_id]);
    let response: Value =
        serde_json::from_slice(&checked_output(command, "A1 worker-status", CLI_TIMEOUT).await?)?;
    let worker: WorkerV1 = serde_json::from_value(response["worker"].clone())?;
    let summary: RunSummary = serde_json::from_value(response["result"].clone())?;
    let origin = reqwest::Url::parse(&gateway.origin)?;
    ensure!(
        response["session_id"] == a1.session_id
            && response["tenant_id"] == job.tenant
            && response["gateway_origin"].as_str() == Some(origin.as_str())
            && worker.state == WorkerStateV1::Terminal
            && !worker.quota_held
            && LiveWorker::read(&worker)? == *a1
            && worker.attempt.start_operation_id.as_deref()
                == Some(stop.start_operation_id.as_str())
            && worker.attempt.start_emitter_id.as_deref() == Some(stop.start_emitter_id.as_str())
            && stop
                .terminal_from_stop
                .as_ref()
                .is_none_or(|observed| worker.last_observation.as_ref() == Some(observed))
            && summary.session_id.to_string() == a1.session_id
            && summary.volume == worker.attempt.volume_name
            && summary.exit_code != 0
            && !summary.timed_out
            && summary.transport_stop_reason.is_none()
            && summary.output_captured
            && summary.changed_files == Some(0)
            && summary.cleanup.state == CleanupState::Removed,
        "A1 CLI status did not attest the original stopped worker and its terminal result"
    );
    if capture.stdout.is_empty() {
        ensure!(
            summary.postprocessing_error.as_deref()
                == Some(
                    "execution observation failed: legacy control supervision failed: control stop requested"
                ),
            "A1 returned no summary and no exact retained stop diagnostic"
        );
    } else {
        let printed: RunSummary = serde_json::from_slice(&capture.stdout)?;
        ensure!(
            serde_json::to_value(&printed)? == serde_json::to_value(&summary)?
                && summary.postprocessing_error.is_none(),
            "A1 CLI summary differs from the successful stopped-result postprocessing"
        );
    }
    Ok(summary)
}

async fn check_stop_audit(
    viewer: &AuditClient,
    worker: &WorkerV1,
    allocation: &AllocationV1,
    started_seq: i64,
    terminal_seq: i64,
    stop: Option<&ExplicitStop>,
) -> Result<()> {
    let session = &worker.launch.session_id;
    // These control records have session_id but no sandbox_id column.
    let history = viewer
        .get_json(&format!(
            "api/v1/events?kind=control.worker.transition&q={session}&limit=100",
        ))
        .await?;
    ensure!(
        history["next_cursor"].is_null(),
        "STOP scenario control audit page incomplete"
    );
    let records = history["items"]
        .as_array()
        .context("STOP control audit records absent")?;
    let records = records
        .iter()
        .filter(|event| event["session_id"] == *session)
        .collect::<Vec<_>>();
    ensure!(
        records.iter().all(
            |event| event["data"]["worker"]["worker_id"] == worker.worker_id
                && event["data"]["worker"]["attempt"]["attempt_id"] == worker.attempt.attempt_id
        ),
        "STOP scenario audit changed worker or attempt"
    );
    let action = |name: &str| {
        records
            .iter()
            .copied()
            .filter(|event| event["data"]["action"] == name)
            .collect::<Vec<_>>()
    };
    let claims = action("start_claim");
    let [claim] = claims.as_slice() else {
        bail!("STOP scenario requires one acknowledged start claim per worker");
    };
    ensure!(
        claim["data"]["operation_id"].as_str() == worker.attempt.start_operation_id.as_deref()
            && claim["data"]["worker"]["attempt"]["start_emitter_id"].as_str()
                == worker.attempt.start_emitter_id.as_deref()
            && worker.attempt.start_emitter_id.is_some()
            && claim["seq"]
                .as_i64()
                .is_some_and(|seq| seq > 0 && seq < started_seq),
        "STOP scenario did not preserve the original start claim"
    );
    let stops = action("stop");
    if let Some(stop) = stop {
        let [event] = stops.as_slice() else {
            bail!("A1 requires exactly one explicit stop event");
        };
        let observe = action("observe");
        let terminal = observe
            .iter()
            .copied()
            .filter(|event| event["data"]["worker"]["state"] == "terminal")
            .collect::<Vec<_>>();
        let [terminal] = terminal.as_slice() else {
            bail!("A1 requires exactly one terminal observation");
        };
        let receipt = &stop.receipt;
        if let Some(observed) = &stop.terminal_from_stop {
            ensure!(
                terminal["data"]["worker"]["last_observation"] == serde_json::to_value(observed)?,
                "A1 CLI terminal observation differs from the durable terminal audit"
            );
        }
        ensure!(
            started_seq == stop.started_seq
                && event["seq"] == receipt.audit_seq
                && event["hash"] == receipt.audit_hash
                && event["data"]["operation_id"] == receipt.operation_id
                && event["data"]["input_hash"] == receipt.input_hash
                && event["data"]["worker"]["revision"].as_u64() == receipt.revision
                && event["data"]["worker"]["state"] == "stopping"
                && event["data"]["worker"]["quota_held"] == true
                && event["data"]["worker"]["attempt"]["container_id"] == stop.peak[0].container_id
                && terminal["data"]["operation_id"] == allocation.terminal_observation_id
                && terminal["data"]["allocation"] == serde_json::to_value(allocation)?
                && terminal["data"]["worker"]["quota_held"] == false
                && terminal["seq"]
                    .as_i64()
                    .is_some_and(|seq| receipt.audit_seq < seq && seq < terminal_seq),
            "A1 stop receipt or terminal observation does not match the durable causal order"
        );
    } else {
        ensure!(stops.is_empty(), "A2 or B1 received an unexpected stop");
    }
    Ok(())
}

enum ParallelCompletion<'a> {
    Normal(&'a RunSummary),
    StopPeer(&'a RunSummary),
    StoppedA1(&'a RunSummary, &'a ExplicitStop),
}

async fn check_parallel_result(
    gateway: &Gateway,
    selection: &RuntimeSelection,
    job: &ParallelJob,
    completion: ParallelCompletion<'_>,
    worker: &WorkerV1,
    allocation: &AllocationV1,
    viewer: &AuditClient,
) -> Result<Value> {
    let (run, verify_control, stop) = match completion {
        ParallelCompletion::Normal(run) => (run, false, None),
        ParallelCompletion::StopPeer(run) => (run, true, None),
        ParallelCompletion::StoppedA1(run, proof) => (run, true, Some(proof)),
    };
    let runtime_kind = selection.runtime_kind();
    ensure!(
        worker.state == WorkerStateV1::Terminal
            && !worker.quota_held
            && worker.launch.session_id == run.session_id.to_string()
            && worker.attempt.volume_name == run.volume
            && worker.cleanup_removed == Some(true)
            && worker.launch.command.argv == job.argv()
            && worker.launch.runtime_kind == runtime_kind
            && worker.launch.image_id == selection.image,
        "{} terminal worker does not match the CLI result",
        job.label
    );
    let created = worker
        .attempt
        .created_inspection
        .as_ref()
        .context("missing Created inspection")?;
    let terminal = worker
        .last_observation
        .as_ref()
        .context("missing terminal observation")?;
    let container = worker
        .attempt
        .container_id
        .as_deref()
        .context("missing worker container ID")?;
    ensure!(
        created.container_id == container
            && created.authority_id == worker.launch.authority_id
            && created.spec_hash == worker.spec_hash
            && created.image_id == selection.image
            && created.runtime_kind == runtime_kind
            && created.network == NetworkV1::None
            && created.security_preset == SecurityPresetV1::FixedSandboxV2
            && u64::from(created.cpu_millis) == WORKER_CPU
            && created.memory_bytes == WORKER_MEMORY
            && created.pids_limit == 256
            && created.volume_name == run.volume
            && terminal.inspection == *created
            && terminal.running == Some(false)
            && terminal.exit_code == Some(run.exit_code)
            && terminal.oom_killed == Some(false)
            && terminal.restart_count == Some(0),
        "{} runtime observation is incomplete or differs from the confined worker",
        job.label
    );
    ensure!(
        allocation.worker_id == worker.worker_id
            && allocation.attempt_id == worker.attempt.attempt_id
            && !allocation.billable
            && allocation.source.provenance == AllocationProvenanceV1::Observed
            && allocation.source.runtime_kind == runtime_kind
            && allocation.source.container_id == container
            && allocation.source.authority_id == worker.launch.authority_id
            && allocation.source.spec_hash == worker.spec_hash
            && terminal.started_at.as_deref() == Some(allocation.started_at.as_str())
            && terminal.finished_at.as_deref() == Some(allocation.finished_at.as_str()),
        "{} ledger does not match the observed worker",
        job.label
    );

    let destination = job.root.join("export");
    let session = run.session_id.to_string();
    let mut export = gateway.command("export-workspace", &job.config());
    export
        .args(["--session", &session, "--runtime-program"])
        .arg(&selection.program)
        .arg("--output")
        .arg(&destination);
    checked_output(export, "parallel_export", RUN_TIMEOUT).await?;
    ensure!(
        fs::read(job.source().join("marker.txt"))? == job.marker.as_bytes()
            && fs::read(destination.join("marker.txt"))? == job.marker.as_bytes()
            && fs::read_dir(job.source())?.count() == 1
            && fs::read_dir(destination)?.count() == 1,
        "{} export does not contain exactly its original canary",
        job.label
    );

    let history = viewer
        .get_json(&format!("api/v1/events?sandbox_id={session}&limit=100"))
        .await?;
    ensure!(
        history["next_cursor"].is_null(),
        "recipe audit page is incomplete"
    );
    let events = history["items"]
        .as_array()
        .context("missing session audit items")?;
    let started = one_event(events, "command.started")?;
    let terminal_kind = if stop.is_some() {
        "command.failed"
    } else {
        "command.completed"
    };
    let completed = one_event(events, terminal_kind)?;
    let exported = one_event(events, "workspace.export.completed")?;
    ensure!(
        started["session_id"] == session
            && completed["session_id"] == session
            && completed["data"]["worker_id"] == worker.worker_id
            && completed["result"]["exit_code"] == run.exit_code
            && exported["data"]["files"] == 1
            && started["seq"]
                .as_i64()
                .is_some_and(|start| completed["seq"].as_i64().is_some_and(|end| start < end)),
        "{} audit does not identify the same worker result and export",
        job.label
    );
    if verify_control {
        let started_seq = started["seq"]
            .as_i64()
            .context("start audit sequence absent")?;
        let terminal_seq = completed["seq"]
            .as_i64()
            .context("terminal audit sequence absent")?;
        check_stop_audit(viewer, worker, allocation, started_seq, terminal_seq, stop).await?;
    }
    if stop.is_some() {
        ensure!(
            completed["result"]["timed_out"] == false
                && !events
                    .iter()
                    .any(|event| event["kind"] == "command.completed")
                && allocation.duration_ns.parse::<u128>()?
                    < Duration::from_secs(job.seconds.parse()?).as_nanos(),
            "A1 stopped result could be mistaken for a deadline or natural completion"
        );
    }
    Ok(
        json!({"label":job.label,"tenant":job.tenant,"session_id":session,"worker_id":worker.worker_id,
        "attempt_id":worker.attempt.attempt_id,"container_id":container,"volume":run.volume,
        "spec_hash":worker.spec_hash,"authority_id":worker.launch.authority_id,"image_id":worker.launch.image_id,
        "usage":allocation,"completion":if stop.is_some() { "explicit_stop" } else { "normal" },
        "cleanup":"worker_removed","export_matching_source":true,
        "terminal_audit":{"seq":completed["seq"],"id":completed["id"],"hash":completed["hash"]},
        "export_audit":{"seq":exported["seq"],"id":exported["id"],"hash":exported["hash"]}}),
    )
}

#[tokio::test]
#[ignore = "requires a real approved Docker or Podman account, explicit local Unix engine, and an immutable QA image with /bin/sleep and /bin/tar"]
async fn real_cli_runs_three_independent_workers_across_two_tenants() -> Result<()> {
    parallel_worker_journey(false).await
}

#[tokio::test]
#[ignore = "requires real Docker or Podman and a local immutable QA image; stops only the owned A1 worker via the CLI"]
async fn real_cli_explicit_stop_releases_only_a1_across_two_tenants() -> Result<()> {
    parallel_worker_journey(true).await
}

async fn parallel_worker_journey(stop_a1: bool) -> Result<()> {
    ensure!(
        !stop_a1
            || (std::env::var_os("AGENTSANDBOX_TEST_BROWSER_DIST").is_none()
                && std::env::var_os("AGENTSANDBOX_TEST_BROWSER_LIVE").is_none()),
        "explicit A1 stop uses the CLI proof; the browser recipe requires three normal results"
    );
    // No test directory, gateway, volume or worker before CLI doctor and the
    // immutable local image check. Doctor emits a closed failure category.
    let selection = RuntimeSelection::read_parallel()?;
    let dashboard = DashboardCheck::read()?;
    let live_browser = match std::env::var_os("AGENTSANDBOX_TEST_BROWSER_LIVE") {
        None => false,
        Some(mode) if mode == "1" => true,
        Some(_) => bail!("AGENTSANDBOX_TEST_BROWSER_LIVE must be 1"),
    };
    ensure!(
        !live_browser || dashboard.is_some(),
        "live dashboard requires AGENTSANDBOX_TEST_BROWSER_DIST"
    );
    let podman_live = live_browser && selection.runtime == RuntimeKind::Podman;
    ensure!(
        !podman_live || selection.podman_rootless == Some(true),
        "Podman live dashboard requires AGENTSANDBOX_TEST_PODMAN_MODE=rootless"
    );
    let doctor = selection.preflight().await?;
    ensure!(
        !podman_live || (doctor.rootless && !selection.allow_rootful),
        "Podman live dashboard requires an attested rootless engine"
    );
    let fixture = support::private_tempdir().keep().canonicalize()?;
    eprintln!("Retained three-worker fixture {}", fixture.display());
    let jobs = [
        ("A1", "tenant-a", "20"),
        ("A2", "tenant-a", "36"),
        ("B1", "tenant-b", "37"),
    ]
    .map(|(label, tenant, seconds)| ParallelJob {
        label,
        tenant,
        seconds,
        root: fixture.join(label),
        marker: Uuid::new_v4().to_string(),
    });
    jobs[0].prepare(None).await?;
    jobs[1].prepare(Some(&jobs[0].config())).await?;
    jobs[2].prepare(None).await?;
    let registry = fixture.join("tenants.json");
    let tenants = [(&jobs[0], 2), (&jobs[2], 1)].map(|(job, count)| json!({
        "tenant_id":job.tenant,"data":fixture.join(format!("{}.sqlite",job.tenant)),
        "runner_token_file":job.config().join("secrets/runner"),"viewer_token_file":job.config().join("secrets/viewer"),
        "worker_quota":{"version":1,"max_active_workers":count,"max_cpu_millis":count*WORKER_CPU,
            "max_memory_bytes":count*WORKER_MEMORY,"max_timeout_seconds":60}
    }));
    fs::write(
        &registry,
        serde_json::to_vec(&json!({"version":1,"tenants":tenants}))?,
    )?;
    let policy_path = fixture.join("policy.json");
    let mut policy = Policy::default();
    policy.allowed_argv = jobs.iter().map(ParallelJob::argv).collect();
    fs::write(&policy_path, serde_json::to_vec(&policy)?)?;
    let mut gateway = Gateway::start_tenants(&registry, &policy_path).await?;
    let viewers = [
        read_token(&jobs[0].config().join("secrets/viewer"))?,
        read_token(&jobs[2].config().join("secrets/viewer"))?,
    ];
    let a = AuditClient::new(&gateway.origin, &viewers[0])?;
    let b = AuditClient::new(&gateway.origin, &viewers[1])?;
    for (client, tenant, slots) in [(&a, "tenant-a", 2), (&b, "tenant-b", 1)] {
        ensure!(
            client.get_json("api/v1/identity").await?
                == json!({"tenant_id":tenant,"role":"viewer"}),
            "viewer identity differs from the fixture tenant"
        );
        check_quota(client, slots, 0).await?;
        ensure!(
            current_workers(client, slots as usize).await?.is_empty(),
            "recipe store is not new"
        );
    }

    let readers = ParallelViewerReads::new(&gateway.origin, &viewers)?;

    // Poll while all subprocesses run. A failed oracle does not drop the
    // other CLI futures or issue an unscoped runtime cleanup.
    let (peak_tx, peak_rx) = tokio::sync::oneshot::channel();
    let live_check = if live_browser {
        Some(
            dashboard
                .as_ref()
                .context("live dashboard absent")?
                .start_live(
                    &fixture,
                    &gateway.origin,
                    &viewers,
                    selection.runtime_kind(),
                )
                .await?,
        )
    } else {
        None
    };
    let peak_tx = (live_check.is_some() || stop_a1).then_some(peak_tx);
    let (one, two, three, live, peak_result) = tokio::join!(
        output(
            jobs[0].run_command(&gateway, &selection),
            jobs[0].label,
            RUN_TIMEOUT
        ),
        jobs[1].run(&gateway, &selection),
        jobs[2].run(&gateway, &selection),
        observe_parallel(
            |route, tenant| readers.read(route, tenant),
            sleep,
            &jobs,
            peak_tx
        ),
        async {
            match live_check {
                Some(check) => Ok::<_, anyhow::Error>((
                    Some(check.observe(peak_rx, &selection, &doctor).await),
                    None,
                )),
                None if stop_a1 => Ok((
                    None,
                    Some(request_a1_stop(peak_rx, &gateway, &selection, &jobs[0], &a).await?),
                )),
                None => Ok((None, None)),
            }
        }
    );
    let (live_ui, stop) = peak_result?;
    let one = one?;
    let first = if let Some(stop) = &stop {
        stopped_a1_summary(&gateway, &jobs[0], one, stop).await?
    } else {
        ensure!(
            one.status.success(),
            "{} failed with status {}",
            jobs[0].label,
            one.status
        );
        jobs[0].normal_summary(&one.stdout)?
    };
    let runs = [first, two?, three?];
    let live_ids = live?;

    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()?;
    let worker_not_found = json!({
        "error": "Worker ou opération introuvable.",
        "code": "worker_not_found",
    });
    for (index, worker_id) in live_ids.iter().enumerate() {
        let path = format!("api/v1/workers/{worker_id}");
        let (owner, other_viewer) = if jobs[index].tenant == "tenant-a" {
            (&a, &viewers[1])
        } else {
            (&b, &viewers[0])
        };
        ensure!(
            owner.get_json(&path).await?["worker_id"].as_str() == Some(worker_id.as_str()),
            "{} detail is unavailable to its owner",
            jobs[index].label
        );
        let response = http
            .get(format!("{}/{path}", gateway.origin))
            .bearer_auth(other_viewer)
            .send()
            .await?;
        ensure!(
            response.status() == reqwest::StatusCode::NOT_FOUND,
            "{} detail did not return 404 to the other tenant's viewer",
            jobs[index].label
        );
        ensure!(
            response.json::<Value>().await? == worker_not_found,
            "{} detail returned unexpected content to the other tenant's viewer",
            jobs[index].label
        );
    }

    let mut results = Vec::new();
    let mut starts = Vec::new();
    let mut finishes = Vec::new();
    for (client, indices) in [(&a, &([0, 1])[..]), (&b, &([2])[..])] {
        let records = current_workers(client, indices.len()).await?;
        ensure!(
            records.len() == indices.len(),
            "tenant worker inventory is incomplete"
        );
        let usage = client.get_json("api/v1/usage").await?;
        ensure!(
            usage["next_cursor"].is_null(),
            "recipe usage page is incomplete"
        );
        let allocations: Vec<AllocationV1> = serde_json::from_value(usage["items"].clone())?;
        ensure!(
            allocations.len() == indices.len(),
            "tenant allocation count differs from its workers"
        );
        check_quota(client, indices.len() as u32, 0).await?;
        for index in indices.iter().copied() {
            let run = &runs[index];
            let worker = records
                .iter()
                .find(|worker| worker.launch.session_id == run.session_id.to_string())
                .context("worker is absent from its owner's registry")?;
            ensure!(
                worker.worker_id == live_ids[index],
                "terminal worker differs from the live recipe identity"
            );
            if let Some(stop) = &stop {
                ensure!(
                    LiveWorker::read(worker)? == stop.peak[index],
                    "stopped scenario changed an immutable worker observation after the peak"
                );
            }
            let allocation = allocations
                .iter()
                .find(|row| row.worker_id == worker.worker_id)
                .context("observed usage is absent from its owner's ledger")?;
            let start = chrono::DateTime::parse_from_rfc3339(&allocation.started_at)?;
            let finish = chrono::DateTime::parse_from_rfc3339(&allocation.finished_at)?;
            ensure!(
                start < finish && allocation.duration_ns.parse::<u128>()? > 0,
                "invalid engine allocation interval"
            );
            starts.push(start);
            finishes.push(finish);
            let completion = match (&stop, index) {
                (Some(stop), 0) => ParallelCompletion::StoppedA1(run, stop),
                (Some(_), _) => ParallelCompletion::StopPeer(run),
                (None, _) => ParallelCompletion::Normal(run),
            };
            results.push(
                check_parallel_result(
                    &gateway,
                    &selection,
                    &jobs[index],
                    completion,
                    worker,
                    allocation,
                    client,
                )
                .await?,
            );
        }
        ensure!(
            client.get_json("api/v1/integrity").await?["valid"] == true,
            "recipe tenant audit is invalid"
        );
    }
    for field in [
        "session_id",
        "worker_id",
        "attempt_id",
        "container_id",
        "volume",
    ] {
        let identities: BTreeSet<_> = results
            .iter()
            .filter_map(|value| value[field].as_str())
            .collect();
        ensure!(
            identities.len() == 3,
            "recipe workers share or lack {field}"
        );
    }
    let latest_start = starts.iter().max().context("missing engine start")?;
    let first_finish = finishes.iter().min().context("missing engine finish")?;
    ensure!(
        latest_start < first_finish && finishes[0] < finishes[1] && finishes[0] < finishes[2],
        "engine intervals do not show three concurrent workers followed by A1 completing first"
    );
    let browser = if live_browser {
        (|| {
            let report = live_ui.context("missing live dashboard result")??;
            let inspected = report["inspected_after_dom"]
                .as_array()
                .context("missing native overlap observations")?;
            ensure!(
                inspected.len() == 3,
                "expected three native overlap observations"
            );
            for result in &results {
                let same_run = inspected.iter().any(|fact| {
                    fact["id"] == result["container_id"]
                        && fact["session"] == result["session_id"]
                        && fact["image"] == result["image_id"]
                        && fact["restart_count"] == 0
                        && fact["started_at"]
                            .as_str()
                            .zip(result["usage"]["started_at"].as_str())
                            .is_some_and(|(live, terminal)| same_started_at(live, terminal))
                });
                if !same_run {
                    let stage = match selection.runtime {
                        RuntimeKind::Docker => "docker_terminal_continuity",
                        RuntimeKind::Podman => "podman_terminal_continuity",
                    };
                    eprintln!(
                        "{}",
                        json!({"status":"FAIL","stage":stage,"runtime_kind":selection.runtime_kind(),"browser":"PASS","rows":[2,1],"views":report["views"]})
                    );
                    bail!(
                        "{} identity or StartedAt changed between live inspection and terminal ledger",
                        selection.runtime
                    );
                }
            }
            Ok(Some(report))
        })()
    } else {
        match dashboard {
            Some(check) => check
                .terminal(
                    &fixture,
                    &gateway.origin,
                    &viewers,
                    &results,
                    selection.runtime_kind(),
                )
                .await
                .map(Some),
            None => Ok(None),
        }
    };
    timeout(Duration::from_secs(5), gateway.child.kill()).await??;
    let browser = browser?;
    let scope = if stop_a1 {
        "three_parallel_audited_workers_with_explicit_a1_stop"
    } else {
        "three_parallel_audited_workers_with_normal_completion"
    };
    let mut report = json!({"status":"PASS","runtime":selection.runtime,"rootless":doctor.rootless,
        "runtime_server_version":doctor.server_version,
        "image_id":selection.image,"scope":scope,
        "worker_limits_at_peak":{"cpu_millis":3*WORKER_CPU,"memory_bytes":3*WORKER_MEMORY},
        "peak_slots":{"tenant-a":2,"tenant-b":1},"after_a1":{"tenant-a":1,"tenant-b":1},
        "foreign_worker_detail":{"http_status":404,"checked":live_ids.len()},
        "overlap_start":latest_start,"overlap_end":first_finish,"results":results,"browser":browser,"test_workspaces_retained":true});
    if let Some(stop) = &stop {
        report["a1_stop"] = json!({"receipt":stop.receipt,"session_id":stop.peak[0].session_id,
            "container_id":stop.peak[0].container_id,"start_operation_id":stop.start_operation_id,
            "monitor_epoch_id":stop.monitor_epoch_id,
            "returned_state":if stop.terminal_from_stop.is_some() { "terminal" } else { "stopping" }});
    }
    eprintln!("{report}");
    Ok(())
}
