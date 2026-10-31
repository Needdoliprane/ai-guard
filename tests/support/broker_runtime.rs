//! Test-only process/inspection helpers. No create/start/stop/remove engine commands.
use agentsandbox::broker::lifecycle::LocalFailurePhaseV1;
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, process::Command as AsyncCommand};
use uuid::Uuid;

pub const BIN: &str = env!("CARGO_BIN_EXE_agentsandbox");
pub const CAPACITY: u32 = 8;
const OUTPUT_LIMIT: usize = 64 * 1024;

pub struct Selection {
    pub program: PathBuf,
    pub endpoint: String,
    pub image: String,
    pub rootful: bool,
}
impl Selection {
    pub fn read() -> Result<Self> {
        ensure!(
            std::env::var("AGENTSANDBOX_TEST_RUNTIME").as_deref() == Ok("docker"),
            "select Docker explicitly"
        );
        ensure!(
            std::env::var("AGENTSANDBOX_TEST_RETAIN_ENFORCER").as_deref() == Ok("1"),
            "acknowledge retained nonempty enforcer with AGENTSANDBOX_TEST_RETAIN_ENFORCER=1"
        );
        let program = PathBuf::from(
            std::env::var_os("AGENTSANDBOX_TEST_RUNTIME_PROGRAM")
                .context("select absolute runtime program")?,
        );
        ensure!(
            program.is_absolute() && program.is_file(),
            "runtime program must exist at an absolute path"
        );
        let endpoint = std::env::var("AGENTSANDBOX_TEST_RUNTIME_ENDPOINT")
            .context("select local Unix endpoint")?;
        ensure!(
            endpoint.starts_with("unix:///") && !endpoint.chars().any(char::is_whitespace),
            "only an explicit local Unix endpoint is allowed"
        );
        let image = std::env::var("AGENTSANDBOX_TEST_IMAGE")
            .context("select existing immutable image ID")?;
        ensure!(
            image
                .strip_prefix("sha256:")
                .is_some_and(|hash| hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())),
            "image must be a full lowercase sha256 ID; no tag or pull"
        );
        Ok(Self {
            program,
            endpoint,
            image,
            rootful: std::env::var("AGENTSANDBOX_TEST_ALLOW_ROOTFUL").as_deref() == Ok("1"),
        })
    }

    pub async fn docker(&self, args: &[&str]) -> Result<Value> {
        let mut child = AsyncCommand::new(&self.program)
            .args(["--host", &self.endpoint])
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("spawn selected Docker inspection")?;
        let mut stdout = child.stdout.take().context("Docker stdout pipe")?;
        let read = async move {
            let mut bytes = Vec::new();
            (&mut stdout)
                .take((OUTPUT_LIMIT + 1) as u64)
                .read_to_end(&mut bytes)
                .await?;
            ensure!(
                bytes.len() <= OUTPUT_LIMIT,
                "Docker inspection exceeded output bound"
            );
            Ok::<_, anyhow::Error>(bytes)
        };
        tokio::time::timeout(Duration::from_secs(3), async {
            let (status, bytes) =
                tokio::try_join!(async { Ok::<_, anyhow::Error>(child.wait().await?) }, read)?;
            ensure!(
                status.success(),
                "Docker inspection failed (status only: {status})"
            );
            serde_json::from_slice(&bytes).context("invalid selected Docker inspection fields")
        })
        .await
        .context("Docker inspection exceeded three seconds")?
    }

    pub async fn inspect(&self, cid: &str) -> Result<Value> {
        ensure!(
            cid.len() == 64 && cid.bytes().all(|b| b.is_ascii_hexdigit()),
            "require immutable full CID"
        );
        self.docker(&["container", "inspect", "--format", r#"{"id":{{json .Id}},"image":{{json .Image}},"status":{{json .State.Status}},"running":{{json .State.Running}},"started_at":{{json .State.StartedAt}},"finished_at":{{json .State.FinishedAt}},"exit_code":{{json .State.ExitCode}},"oom_killed":{{json .State.OOMKilled}},"restarts":{{json .RestartCount}},"network":{{json .HostConfig.NetworkMode}},"privileged":{{json .HostConfig.Privileged}},"bind_count":{{len .HostConfig.Binds}},"mounts":[{{range $i, $m := .Mounts}}{{if $i}},{{end}}{"type":{{json $m.Type}},"name":{{json $m.Name}},"destination":{{json $m.Destination}},"rw":{{json $m.RW}},"driver":{{json $m.Driver}}}{{end}}]}"#, cid]).await
    }
}

pub struct Evidence {
    pub root: PathBuf,
}
impl Evidence {
    pub fn new() -> Result<Self> {
        #[cfg(target_os = "macos")]
        let base = "/private/tmp";
        #[cfg(not(target_os = "macos"))]
        let base = "/tmp";
        let root = tempfile::Builder::new()
            .prefix("br-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(base)?
            .keep()
            .canonicalize()?;
        let evidence = Self { root };
        for name in [
            "source", "exports", "operator", "control", "backups", "logs",
        ] {
            evidence.directory(name)?;
        }
        evidence.write("retention.json", &json!({"version":1,"cleanup":"NOT_RUN: D5 remains open; retain enforcer, root, exact runtime resources","enforcer_root":evidence.root.join("b/enforcer"),"runtime_resources":"see worker.json, recovery snapshots and the durable control store; do not prune","guard_kill_authorized":false}))?;
        eprintln!(
            "retained broker runtime evidence: {}",
            evidence.root.display()
        );
        Ok(evidence)
    }
    pub fn directory(&self, name: &str) -> Result<PathBuf> {
        let path = self.root.join(name);
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(path)
    }
    pub fn write(&self, name: &str, value: &impl serde::Serialize) -> Result<()> {
        write_private(&self.root.join(name), &serde_json::to_vec_pretty(value)?)
    }
}

pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub fn write_new_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub struct OwnedProcess {
    child: Child,
}
impl OwnedProcess {
    pub fn spawn(
        mut command: Command,
        log: &Path,
        readiness: bool,
    ) -> Result<(Self, Option<mpsc::Receiver<String>>)> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(log)?;
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command.spawn().context("spawn owned native process")?;
        let pipe = child.stderr.take().context("native child stderr")?;
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || drain_log(pipe, file, readiness.then_some(send)));
        Ok((Self { child }, readiness.then_some(receive)))
    }
    pub fn running(&mut self) -> Result<()> {
        ensure!(
            self.child.try_wait()?.is_none(),
            "owned native process exited; inspect its private bounded stderr"
        );
        Ok(())
    }
    pub fn id(&self) -> u32 {
        self.child.id()
    }
    pub fn kill_owned(&mut self) -> Result<ExitStatus> {
        self.running()?;
        self.child
            .kill()
            .context("kill only the owned broker Child")?;
        let limit = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            ensure!(
                Instant::now() < limit,
                "owned process did not reap after SIGKILL"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let limit = Instant::now() + Duration::from_secs(2);
            while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < limit {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        // This owns only gateway/broker children. Never discovers or kills the enforcer.
    }
}

fn drain_log(mut pipe: impl Read, mut file: File, mut ready: Option<mpsc::Sender<String>>) {
    let mut written = 0;
    let mut first = Vec::new();
    let mut buffer = [0; 4096];
    while let Ok(count) = pipe.read(&mut buffer) {
        if count == 0 {
            break;
        }
        let retained = count.min(OUTPUT_LIMIT - written);
        let _ = file.write_all(&buffer[..retained]);
        written += retained;
        if ready.is_some() {
            for byte in &buffer[..count] {
                if *byte == b'\n' {
                    if let Some(sender) = ready.take() {
                        let _ = sender.send(String::from_utf8_lossy(&first).into_owned());
                    }
                    break;
                }
                if first.len() < 1024 {
                    first.push(*byte);
                } else {
                    ready.take();
                    break;
                }
            }
        }
    }
    let _ = file.sync_all();
}

pub fn gateway(
    evidence: &Evidence,
    config: &Path,
    policy: &Path,
) -> Result<(OwnedProcess, String)> {
    let mut command = Command::new(BIN);
    command
        .args([
            "gateway",
            "--listen",
            "127.0.0.1:0",
            "--worker-limit",
            "1",
            "--worker-cpu-millis",
            "2000",
            "--worker-memory-bytes",
            "1073741824",
            "--worker-max-seconds",
            "5",
            "--max-events",
            "10000",
        ])
        .arg("--data")
        .arg(evidence.root.join("control/audit.sqlite"))
        .arg("--runner-token-file")
        .arg(config.join("secrets/runner"))
        .arg("--viewer-token-file")
        .arg(config.join("secrets/viewer"))
        .arg("--policy")
        .arg(policy)
        .arg("--initialize-supervised-stores")
        .arg(evidence.root.join("backups"));
    let (process, receiver) =
        OwnedProcess::spawn(command, &evidence.root.join("logs/gateway.stderr"), true)?;
    let line = receiver
        .context("gateway readiness receiver")?
        .recv_timeout(Duration::from_secs(15))
        .context("gateway readiness timed out; inspect private stderr")?;
    let address = line
        .strip_prefix("Gateway ")
        .and_then(|s| s.split_whitespace().next())
        .context("gateway did not report readiness; inspect private stderr")?;
    let address: std::net::SocketAddr = address.parse().context("invalid gateway bound address")?;
    ensure!(
        address.ip().is_loopback() && address.port() != 0,
        "gateway must bind loopback"
    );
    Ok((process, format!("http://{address}")))
}

pub fn broker(evidence: &Evidence, generation: u8) -> Result<OwnedProcess> {
    let mut command = Command::new(BIN);
    command
        .args(["broker", "serve", "--config"])
        .arg(evidence.root.join("operator/registry.json"));
    Ok(OwnedProcess::spawn(
        command,
        &evidence
            .root
            .join(format!("logs/broker-{generation}.stderr")),
        false,
    )?
    .0)
}

pub async fn get<T: DeserializeOwned>(
    client: &reqwest::Client,
    origin: &str,
    token: &str,
    route: &str,
) -> Result<T> {
    let response = client
        .get(format!("{origin}/api/v1/{route}"))
        .bearer_auth(token)
        .send()
        .await?;
    ensure!(
        response.status().is_success(),
        "gateway read {route} refused with status {}",
        response.status()
    );
    let bytes = response.bytes().await?;
    ensure!(
        bytes.len() <= 2 * 1024 * 1024,
        "gateway evidence exceeds bound"
    );
    serde_json::from_slice(&bytes).context("invalid gateway evidence")
}

pub async fn audit(client: &reqwest::Client, origin: &str, token: &str) -> Result<Vec<Value>> {
    let page: Value = get(client, origin, token, "events?limit=1000&order=asc").await?;
    ensure!(
        page["next_cursor"].is_null(),
        "audit evidence exceeds one bounded page"
    );
    page["items"]
        .as_array()
        .cloned()
        .context("missing real audit items")
}

/// Test-only projection; arbitrary audit payloads must never enter test logs.
pub fn batch_failure_marker(page: &Value) -> Result<Option<Value>> {
    let items = page["items"]
        .as_array()
        .context("missing diagnostic items")?;
    ensure!(items.len() <= 1, "diagnostic filter exceeded its limit");
    let Some(event) = items.first() else {
        return Ok(None);
    };
    ensure!(
        event["kind"] == "broker.batch.failure" && event["source"] == "runner",
        "unexpected diagnostic event identity"
    );
    let raw_id = event["id"]
        .as_str()
        .context("missing diagnostic event ID")?;
    let event_id = Uuid::parse_str(raw_id).map_err(|_| anyhow!("invalid diagnostic event ID"))?;
    ensure!(
        !event_id.is_nil() && event_id.to_string() == raw_id,
        "noncanonical diagnostic event ID"
    );
    let audit_seq = event["seq"]
        .as_i64()
        .context("missing diagnostic audit seq")?;
    ensure!(audit_seq > 0, "invalid diagnostic audit seq");
    let data = &event["data"];
    ensure!(
        data["version"].as_u64() == Some(1) && data["runtime_effect"] == "unknown",
        "unsupported diagnostic evidence contract"
    );
    let phase: LocalFailurePhaseV1 = serde_json::from_value(data["phase"].clone())
        .map_err(|_| anyhow!("invalid diagnostic phase"))?;
    let broker_step = optional_failure_label(
        data.get("broker_step"),
        &[
            "arm_exchange",
            "registration",
            "load_after_registration",
            "readiness_before_claim",
            "clock_before_claim",
            "claim_exchange",
            "generation_before_prepare",
            "prepare",
        ],
    )?;
    let enforcer_stage = nested_failure_stage(
        data.get("enforcer"),
        &["register", "before_claim", "prepare", "before_spawn"],
    )?;
    let runtime_stage = nested_failure_stage(
        data.get("runtime"),
        &[
            "authority_before_spawn",
            "invocation_before_spawn",
            "stop_before_spawn",
            "guard_before_spawn",
            "spawn",
            "started_observation",
        ],
    )?;
    ensure!(
        broker_step.is_some() || enforcer_stage.is_some() || runtime_stage.is_some(),
        "missing coded diagnostic origin"
    );
    // Only this projection is persisted. Neither arbitrary data nor any error
    // body/argv is copied. An outer exchange label is not evidence of an ACK.
    Ok(Some(json!({
        "event_id":event_id,"audit_seq":audit_seq,"kind":"broker.batch.failure",
        "phase":phase,"broker_step":broker_step,"enforcer_stage":enforcer_stage,
        "runtime_stage":runtime_stage,"runtime_effect":"unknown"
    })))
}

fn optional_failure_label<'a>(
    value: Option<&'a Value>,
    allowed: &[&str],
) -> Result<Option<&'a str>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(label)) if allowed.contains(&label.as_str()) => Ok(Some(label)),
        _ => bail!("invalid diagnostic stage label"),
    }
}

fn nested_failure_stage<'a>(value: Option<&'a Value>, allowed: &[&str]) -> Result<Option<&'a str>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(fields)) => {
            let stage = optional_failure_label(fields.get("stage"), allowed)?;
            ensure!(stage.is_some(), "missing nested diagnostic stage");
            Ok(stage)
        }
        _ => bail!("invalid nested diagnostic stage"),
    }
}

pub async fn only_worker(
    client: &reqwest::Client,
    origin: &str,
    token: &str,
) -> Result<Option<agentsandbox::control::WorkerV1>> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Page {
        items: Vec<agentsandbox::control::WorkerV1>,
        next_cursor: Option<String>,
    }
    let mut page: Page = get(client, origin, token, "workers?limit=2").await?;
    ensure!(
        page.next_cursor.is_none() && page.items.len() <= 1,
        "fresh tenant control contains more than one worker"
    );
    Ok(page.items.pop())
}

pub fn worker_emissions(events: &[Value], worker: &str) -> Result<(Vec<Value>, Vec<Value>)> {
    let mut arms = Vec::new();
    let mut claims = Vec::new();
    for event in events {
        let input = &event["data"]["input"];
        if input["request"]["worker_id"] != worker {
            continue;
        }
        match input["action"].as_str() {
            Some("arm") if input["request"]["subject"]["kind"] == "worker" => {
                arms.push(event.clone())
            }
            Some("worker_claim") => claims.push(event.clone()),
            _ => {}
        }
    }
    if arms.len() != 1 || claims.len() != 1 {
        bail!(
            "expected exactly one real worker Arm and one claim: arms={}, claims={}",
            arms.len(),
            claims.len()
        );
    }
    Ok((arms, claims))
}
