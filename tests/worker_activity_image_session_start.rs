//! Opt-in image-level smoke test for the production activity helper and hook
//! binary. It uses only a synthetic SessionStart payload; no provider client,
//! credential, external network, or host filesystem mount is involved.
//!
//! Run with `AI_GUARD_TEST_WORKER_IMAGE` naming an image built from the pinned
//! worker Dockerfile and `AI_GUARD_TEST_DOCKER_BIN` set to an absolute Docker
//! executable path. The test is ignored in the ordinary test suite.

use agentsandbox::{
    activity_channel::wire::{
        AckV1, BrokerFrameV1, ClientV1, EventV1, FrameReader, FrameWriter, HelperFrameV1, ReadyV1,
        StatusV1,
    },
    control_fs::Directory,
    provider_activity::{
        ActivityClient, ActivityCollector, ActivityContext, ActivityLimits, ActivitySpool, HookAck,
        HookFrame, HookTransport, pump_hooks,
    },
    redaction::Redactor,
};
use anyhow::{Context, bail, ensure};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, ChildStdin, ChildStdout, Command},
    time::{Instant, sleep, timeout},
};
use uuid::Uuid;

const TEST_TIMEOUT: Duration = Duration::from_secs(45);
const HOOK_STDIN: &[u8] = br#"{"hook_event_name":"SessionStart","session_id":"image-fixture-session","source":"startup","model":"synthetic-fixture"}"#;

struct ContainerCleanup {
    docker: PathBuf,
    name: String,
    armed: bool,
}

impl ContainerCleanup {
    async fn remove(mut self) -> anyhow::Result<()> {
        let output = timeout(
            Duration::from_secs(5),
            Command::new(&self.docker)
                .args(["rm", "--force", &self.name])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .context("remove the isolated activity container")??;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            ensure!(
                stderr.contains("No such container") || stderr.contains("No such object"),
                "could not remove isolated activity container"
            );
        }
        self.armed = false;
        Ok(())
    }
}

impl Drop for ContainerCleanup {
    fn drop(&mut self) {
        // Best-effort fallback if cancellation bypasses the explicit bounded
        // cleanup below.
        if self.armed {
            let _ = std::process::Command::new(&self.docker)
                .args(["rm", "--force", &self.name])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
    }
}

struct ImageHookPump {
    input: Option<ChildStdin>,
    output: ChildStdout,
    reader: FrameReader<HelperFrameV1>,
    writer: FrameWriter,
    hook: Option<Child>,
    docker: PathBuf,
    container: String,
    hook_frame_seen: bool,
    acknowledged: Option<(u64, HookAck)>,
}

impl HookTransport for ImageHookPump {
    async fn next(&mut self) -> anyhow::Result<Option<HookFrame>> {
        if self.hook_frame_seen {
            ensure!(
                self.acknowledged == Some((1, HookAck::Recorded)),
                "SessionStart was not acknowledged as recorded"
            );
            let hook = self
                .hook
                .take()
                .context("synthetic hook process is missing")?;
            let output = timeout(TEST_TIMEOUT, hook.wait_with_output())
                .await
                .context("synthetic hook process exceeded its deadline")??;
            ensure!(
                output.status.success(),
                "synthetic hook exited unsuccessfully"
            );
            ensure!(output.stdout.is_empty(), "hook wrote unexpected stdout");
            ensure!(output.stderr.is_empty(), "hook wrote unexpected stderr");
            ensure!(
                self.loss_marker_count().await? == 0,
                "successful ACK left a loss marker in the worker image"
            );
            self.input.take(); // helper sees EOF only after the hook received ACK
        }

        match timeout(TEST_TIMEOUT, self.reader.read(&mut self.output))
            .await
            .context("image helper frame exceeded its deadline")??
        {
            Some(HelperFrameV1::Hook(hook)) => {
                ensure!(
                    !self.hook_frame_seen,
                    "image emitted a hook after SessionStart"
                );
                ensure!(hook.seq == 1, "expected the first helper hook sequence");
                ensure!(
                    hook.submit.client == ClientV1::ClaudeCode
                        && hook.submit.event == EventV1::SessionStart
                        && !hook.submit.gate
                        && !hook.submit.stdin_truncated
                        && !hook.submit.payload_invalid_utf8,
                    "expected a complete synthetic Claude SessionStart"
                );
                ensure!(hook.submit.payload.contains("image-fixture-session"));
                self.hook_frame_seen = true;
                Ok(Some(HookFrame::Hook {
                    seq: hook.seq,
                    event: hook.submit.event,
                    payload: hook.submit.payload,
                    truncated: false,
                }))
            }
            Some(HelperFrameV1::Loss(loss)) => Ok(Some(HookFrame::Loss {
                seq: loss.seq,
                count: loss.count,
                reason: format!("{:?}", loss.reason),
            })),
            None => Ok(None),
            Some(other) => bail!("unexpected activity helper frame: {other:?}"),
        }
    }

    async fn ack(&mut self, seq: u64, ack: HookAck) -> anyhow::Result<()> {
        ensure!(
            self.hook_frame_seen && seq == 1 && self.acknowledged.is_none(),
            "missing, out-of-order, or duplicate SessionStart ACK"
        );
        let status = match ack {
            HookAck::Recorded => StatusV1::Recorded,
            HookAck::Refused => StatusV1::Refused,
        };
        let input = self.input.as_mut().context("helper stdin already closed")?;
        self.writer
            .write(input, &BrokerFrameV1::Ack(AckV1 { seq, status }))
            .await
            .context("write ACK to production image helper")?;
        self.acknowledged = Some((seq, ack));
        Ok(())
    }
}

impl ImageHookPump {
    async fn loss_marker_count(&self) -> anyhow::Result<usize> {
        let output = timeout(
            Duration::from_secs(3),
            Command::new(&self.docker)
                .args([
                    "exec",
                    &self.container,
                    "/usr/bin/find",
                    "/home/agent/.ai-guard-activity/loss",
                    "-mindepth",
                    "1",
                    "-maxdepth",
                    "1",
                    "-type",
                    "f",
                    "-print",
                    "-quit",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .context("inspect the private worker loss directory")??;
        ensure!(output.status.success(), "loss directory inspection failed");
        Ok(usize::from(!output.stdout.is_empty()))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an explicitly built worker image and isolated Docker daemon"]
async fn production_image_session_start_is_spooled_before_recorded_ack() {
    let result = timeout(TEST_TIMEOUT, run_image_case()).await;
    assert!(result.is_ok(), "image activity test timed out");
    result.unwrap().unwrap();
}

async fn run_image_case() -> anyhow::Result<()> {
    let docker = required_absolute_env("AI_GUARD_TEST_DOCKER_BIN")?;
    let image = std::env::var("AI_GUARD_TEST_WORKER_IMAGE")
        .context("AI_GUARD_TEST_WORKER_IMAGE must name the pinned test image")?;
    ensure!(!image.trim().is_empty(), "worker image name is empty");
    let image_id = inspect_test_image(&docker, &image).await?;
    let container = format!("ai-guard-activity-{}", Uuid::new_v4());
    let cleanup = ContainerCleanup {
        docker: docker.clone(),
        name: container.clone(),
        armed: true,
    };
    let result = run_image_container(&docker, &image_id, &container).await;
    let cleanup_result = cleanup.remove().await;
    result?;
    cleanup_result
}

async fn run_image_container(docker: &Path, image_id: &str, container: &str) -> anyhow::Result<()> {
    let mut helper = Command::new(docker)
        .args([
            "run",
            "--pull",
            "never",
            "--interactive",
            "--name",
            container,
            "--network",
            "none",
            "--read-only",
            "--user",
            "1000:1000",
            "--tmpfs",
            "/home/agent/.ai-guard-activity:rw,nosuid,nodev,noexec,uid=1000,gid=1000,mode=0700",
            image_id,
            "/usr/local/bin/ai-guard-activity",
            "serve-v1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("start the actual worker image helper")?;
    let mut input = helper.stdin.take().context("helper stdin is unavailable")?;
    let output = helper
        .stdout
        .take()
        .context("helper stdout is unavailable")?;
    let mut reader = FrameReader::<HelperFrameV1>::new();
    let mut output = output;
    let hello = timeout(TEST_TIMEOUT, reader.read(&mut output))
        .await
        .context("image helper handshake exceeded its deadline")??;
    ensure!(
        hello
            == Some(HelperFrameV1::Hello(
                agentsandbox::activity_channel::wire::HelloV1 { version: 1 }
            )),
        "expected production helper Hello"
    );
    let mut writer = FrameWriter::new();
    writer
        .write(&mut input, &BrokerFrameV1::Ready(ReadyV1 { version: 1 }))
        .await
        .context("send Ready to production helper")?;
    wait_for_helper_socket(docker, container).await?;
    verify_provider_commands_absent(docker, container).await?;
    let hook = spawn_hook(docker, container).await?;

    let temporary = tempfile::Builder::new()
        .prefix("ai-guard-image-activity-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .context("create private activity spool directory")?;
    let spool_directory = Directory::open_existing(temporary.path())?;
    let session = Uuid::new_v4();
    let collector = ActivityCollector::new(
        ActivitySpool::create(&spool_directory, session)?,
        ActivityContext {
            session,
            session_id: Some("image-fixture-session".into()),
            project_id: Some("image-fixture-project".into()),
            sandbox_id: Some("image-fixture-sandbox".into()),
            agent_id: Some("image-fixture-agent".into()),
            correlation_id: None,
            trace_id: None,
            worker_id: Some("image-fixture-worker".into()),
            attempt_id: Some("image-fixture-attempt".into()),
            client: ActivityClient::ClaudeCode,
            interactive: true,
            client_version_label: Some("synthetic-image-fixture".into()),
        },
        Redactor::new(Vec::new()),
        ActivityLimits::default(),
    )?;
    let transport = ImageHookPump {
        input: Some(input),
        output,
        reader,
        writer,
        hook: Some(hook),
        docker: docker.to_owned(),
        container: container.to_owned(),
        hook_frame_seen: false,
        acknowledged: None,
    };
    let outcome = pump_hooks(collector.clone(), transport).await;
    ensure!(outcome.hooks == 1, "expected one SessionStart hook");
    ensure!(outcome.recorded == 1 && outcome.refused == 0);
    ensure!(!outcome.transport_error, "activity transport failed");
    ensure!(
        outcome.declared_losses == 0,
        "helper declared lost activity"
    );
    let summary = collector.summary()?;
    ensure!(summary.hook.records == 1 && summary.hook.mapped == 1);
    ensure!(summary.spooled_events > 0, "SessionStart was not spooled");

    drop(collector);
    let helper_status = timeout(TEST_TIMEOUT, helper.wait())
        .await
        .context("image helper process did not exit after ACK and EOF")??;
    ensure!(
        helper_status.success(),
        "image helper exited unsuccessfully"
    );
    Ok(())
}

async fn inspect_test_image(docker: &Path, image: &str) -> anyhow::Result<String> {
    let output = timeout(
        Duration::from_secs(5),
        Command::new(docker)
            .args([
                "image",
                "inspect",
                "--format",
                "{{.Id}}\n{{index .Config.Labels \"ai-guard.provider.codex.requested-version\"}}\n{{index .Config.Labels \"ai-guard.provider.claude-code.requested-version\"}}",
                image,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("inspect test worker image labels")??;
    ensure!(output.status.success(), "worker image inspection failed");
    let inspected = std::str::from_utf8(&output.stdout)?;
    let mut lines = inspected.lines();
    let image_id = lines.next().context("image inspect omitted immutable ID")?;
    let codex = lines.next().context("image inspect omitted Codex label")?;
    let claude = lines.next().context("image inspect omitted Claude label")?;
    ensure!(
        lines.next().is_none(),
        "image inspect returned extra fields"
    );
    ensure!(codex.is_empty(), "test image must not install Codex");
    ensure!(claude.is_empty(), "test image must not install Claude Code");
    let digest = image_id
        .strip_prefix("sha256:")
        .context("image ID is not sha256")?;
    ensure!(digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()));
    Ok(image_id.to_owned())
}

async fn spawn_hook(docker: &Path, container: &str) -> anyhow::Result<Child> {
    let mut child = Command::new(docker)
        .args([
            "exec",
            "--interactive",
            container,
            "/usr/local/bin/ai-guard-activity",
            "hook-v1",
            "claude-code",
            "SessionStart",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("start the actual image hook command")?;
    let mut input = child.stdin.take().context("hook stdin is unavailable")?;
    input.write_all(HOOK_STDIN).await?;
    drop(input);
    Ok(child)
}

async fn verify_provider_commands_absent(docker: &Path, container: &str) -> anyhow::Result<()> {
    for command in ["claude", "codex"] {
        let path = format!("/usr/local/bin/{command}");
        let result = timeout(
            Duration::from_secs(3),
            Command::new(docker)
                .args(["exec", container, "/usr/bin/test", "!", "-x", &path])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .status(),
        )
        .await
        .with_context(|| format!("check that {command} is absent"))??;
        ensure!(result.success(), "provider command {command} is installed");
    }
    Ok(())
}

async fn wait_for_helper_socket(docker: &Path, container: &str) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let result = timeout(
            Duration::from_secs(1),
            Command::new(docker)
                .args([
                    "exec",
                    container,
                    "/usr/bin/test",
                    "-S",
                    "/home/agent/.ai-guard-activity/activity-v1.sock",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await;
        if matches!(result, Ok(Ok(output)) if output.status.success()) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("production helper socket did not become ready");
        }
        sleep(Duration::from_millis(25)).await;
    }
}

fn required_absolute_env(name: &str) -> anyhow::Result<PathBuf> {
    let path = std::env::var_os(name).context(format!("{name} must be set"))?;
    let path = PathBuf::from(path);
    ensure!(path.is_absolute(), "{name} must be an absolute path");
    ensure!(path.is_file(), "{name} must name an existing file");
    Ok(path)
}
