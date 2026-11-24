//! Test-only process/inspection helpers. No create/start/stop/remove engine commands.
#[path = "broker_runtime_checkpoint.rs"]
pub mod checkpoint;

use agentsandbox::broker::{enforcer::EnforcerError, lifecycle::LocalFailurePhaseV1};
use anyhow::{Context, Result, anyhow, bail, ensure};
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, process::Command as AsyncCommand};
use uuid::Uuid;

const DEFAULT_BIN: &str = env!("CARGO_BIN_EXE_agentsandbox");
pub const CAPACITY: u32 = 8;
pub const BROKER_SELECTION_ENV: &str = "AGENTSANDBOX_BROKER_SELECTION";
const OUTPUT_LIMIT: usize = 64 * 1024;
const LIVE_EVENT_LIMIT: usize = 256;
const LIVE_EVIDENCE_RESERVE: Duration = Duration::from_secs(15);

pub struct Selection {
    pub binary: PathBuf,
    pub program: PathBuf,
    pub endpoint: String,
    pub image: String,
    pub rootful: bool,
    pub native_start: bool,
    pub timeout_seconds: u32,
}

#[derive(PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerEvent {
    pub action: String,
    pub id: String,
    pub session: String,
    pub purpose: String,
    pub wall_ns: i64,
    pub exit_code: String,
    pub signal: String,
}

impl ContainerEvent {
    pub fn is_worker_creation_or_start(&self) -> bool {
        matches!(self.action.as_str(), "create" | "start")
    }
}

pub struct LiveWorkerEvents {
    pub until: DateTime<FixedOffset>,
    collector: tokio::task::JoinSet<Result<Vec<ContainerEvent>>>,
}

impl LiveWorkerEvents {
    pub async fn finish(mut self, session: Uuid) -> Result<Vec<ContainerEvent>> {
        let collection = self
            .collector
            .join_next()
            .await
            .context("live Docker event collector missing")?
            .context("live Docker event collector task failed")?;
        let mut events = collection?;
        let session = session.to_string();
        events.retain(|event| event.session == session);
        Ok(events)
    }
}

#[derive(serde::Deserialize)]
struct LiveDockerEvent {
    #[serde(rename = "Type")]
    kind: String,
    #[serde(rename = "Action")]
    action: String,
    #[serde(rename = "Actor")]
    actor: LiveDockerActor,
    #[serde(rename = "timeNano")]
    wall_ns: i64,
}

#[derive(serde::Deserialize)]
struct LiveDockerActor {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Attributes")]
    attributes: LiveWorkerLabels,
}

#[derive(serde::Deserialize)]
struct LiveWorkerLabels {
    #[serde(rename = "agentsandbox.session")]
    session: Uuid,
    #[serde(rename = "agentsandbox.purpose")]
    purpose: String,
}

fn live_worker_event(
    line: &[u8],
    since: DateTime<FixedOffset>,
    until: DateTime<FixedOffset>,
) -> Result<ContainerEvent> {
    let event: LiveDockerEvent =
        serde_json::from_slice(line).map_err(|_| anyhow!("invalid live Docker event frame"))?;
    let time = DateTime::from_timestamp_nanos(event.wall_ns);
    ensure!(
        event.kind == "container"
            && matches!(event.action.as_str(), "create" | "start")
            && event.actor.attributes.purpose == "run"
            && !event.actor.attributes.session.is_nil()
            && event.actor.id.len() == 64
            && event.actor.id.bytes().all(|byte| byte.is_ascii_hexdigit())
            && time >= since
            && time <= until,
        "live Docker event escaped the selected stream or its time window"
    );
    Ok(ContainerEvent {
        action: event.action,
        id: event.actor.id,
        session: event.actor.attributes.session.to_string(),
        purpose: event.actor.attributes.purpose,
        wall_ns: event.wall_ns,
        exit_code: String::new(),
        signal: String::new(),
    })
}

#[test]
fn docker_worker_live_frame_retains_only_projected_session_and_attempt() {
    let since = DateTime::parse_from_rfc3339("2026-11-19T14:15:16Z").unwrap();
    let until = since + chrono::TimeDelta::seconds(2);
    let private = "synthetic-unretained-docker-label";
    let mut wire = json!({
        "Type": "container",
        "Action": "create",
        "Actor": {
            "ID": "a".repeat(64),
            "Attributes": {
                "agentsandbox.purpose": "run",
                "agentsandbox.session": "11111111-1111-4111-8111-111111111111",
                "unused": private
            }
        },
        "timeNano": (since + chrono::TimeDelta::seconds(1)).timestamp_nanos_opt().unwrap()
    });
    let event = live_worker_event(&serde_json::to_vec(&wire).unwrap(), since, until).unwrap();
    assert!(event.is_worker_creation_or_start());
    assert!(!serde_json::to_string(&event).unwrap().contains(private));

    for (field, value) in [
        ("/timeNano", json!(until.timestamp_nanos_opt().unwrap() + 1)),
        ("/Action", json!("kill")),
        ("/Actor/Attributes/agentsandbox.purpose", json!("export")),
    ] {
        let original = std::mem::replace(wire.pointer_mut(field).unwrap(), value);
        assert!(live_worker_event(&serde_json::to_vec(&wire).unwrap(), since, until).is_err());
        *wire.pointer_mut(field).unwrap() = original;
    }
}

async fn collect_live_worker_events(
    mut response: reqwest::Response,
    since: DateTime<FixedOffset>,
    until: DateTime<FixedOffset>,
    earliest_end: Instant,
) -> Result<Vec<ContainerEvent>> {
    let mut frame = Vec::new();
    let mut events = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("Docker event stream failed")?
    {
        for part in chunk.split_inclusive(|byte| *byte == b'\n') {
            ensure!(
                frame.len() + part.len() <= OUTPUT_LIMIT,
                "Docker event frame exceeded its bound"
            );
            frame.extend_from_slice(part);
            if part.last() == Some(&b'\n') {
                ensure!(
                    events.len() < LIVE_EVENT_LIMIT,
                    "live Docker event count exceeded its bound"
                );
                events.push(live_worker_event(&frame, since, until)?);
                frame.clear();
            }
        }
    }
    ensure!(
        frame.is_empty(),
        "Docker event stream ended with a partial frame"
    );
    ensure!(
        Instant::now() >= earliest_end,
        "Docker event stream ended before its requested cutoff"
    );
    Ok(events)
}

fn worker_event_window(
    since: DateTime<FixedOffset>,
    observed_started_at: DateTime<FixedOffset>,
    until: DateTime<FixedOffset>,
) -> Result<(String, String)> {
    ensure!(
        since < observed_started_at && observed_started_at < until,
        "Docker event history window does not bracket the observed worker start"
    );
    // Both limits come from the daemon clock. RFC 3339 retains subsecond
    // precision, so two samples within the same second remain distinct.
    Ok((since.to_rfc3339(), until.to_rfc3339()))
}

fn docker_event_api_timestamp(value: DateTime<FixedOffset>) -> String {
    // The CLI accepts RFC 3339 and converts it for the daemon's /events API.
    format!(
        "{}.{:09}",
        value.timestamp(),
        value.timestamp_subsec_nanos()
    )
}

#[test]
fn docker_worker_event_window_preserves_nanosecond_boundaries() {
    let parse = |value: &str| DateTime::parse_from_rfc3339(value).unwrap();
    let since = parse("2026-11-19T14:15:16.123456780+00:00");
    let started = parse("2026-11-19T14:15:16.123456781+00:00");
    let until = parse("2026-11-19T14:15:16.123456782+00:00");
    let (since_arg, until_arg) = worker_event_window(since, started, until).unwrap();
    assert_eq!((parse(&since_arg), parse(&until_arg)), (since, until));
    assert_eq!(
        docker_event_api_timestamp(since),
        format!("{}.123456780", since.timestamp())
    );
    assert_eq!(
        docker_event_api_timestamp(until),
        format!("{}.123456782", until.timestamp())
    );
    assert!(worker_event_window(started, started, until).is_err());
    assert!(worker_event_window(since, started, started).is_err());
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
        let binary = match std::env::var_os("AGENTSANDBOX_TEST_BROKER_BIN") {
            Some(path) => {
                let path = PathBuf::from(path);
                ensure!(path.is_absolute(), "selected binary must be absolute");
                let metadata = path.symlink_metadata().context("inspect selected binary")?;
                ensure!(
                    metadata.is_file()
                        && metadata.permissions().mode() & 0o111 != 0
                        && path == path.canonicalize().context("resolve selected binary")?,
                    "selected binary must be a regular executable at a canonical path without links"
                );
                path
            }
            None => PathBuf::from(DEFAULT_BIN),
        };
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
        let native_start = match std::env::var("AGENTSANDBOX_TEST_BROKER_NATIVE_START").as_deref() {
            Ok("1") => true,
            Ok("0") | Err(std::env::VarError::NotPresent) => false,
            _ => bail!("AGENTSANDBOX_TEST_BROKER_NATIVE_START must be 0 or 1"),
        };
        let timeout_seconds = match std::env::var("AGENTSANDBOX_TEST_BROKER_TIMEOUT_SECONDS") {
            Ok(value) => value
                .parse::<u32>()
                .context("AGENTSANDBOX_TEST_BROKER_TIMEOUT_SECONDS must be an integer")?,
            Err(std::env::VarError::NotPresent) => 5,
            Err(error) => return Err(error).context("read selected broker worker timeout"),
        };
        ensure!(
            (5..=60).contains(&timeout_seconds),
            "AGENTSANDBOX_TEST_BROKER_TIMEOUT_SECONDS must be in 5..=60"
        );
        Ok(Self {
            binary,
            program,
            endpoint,
            image,
            rootful: std::env::var("AGENTSANDBOX_TEST_ALLOW_ROOTFUL").as_deref() == Ok("1"),
            native_start,
            timeout_seconds,
        })
    }

    pub async fn docker(&self, args: &[&str]) -> Result<Value> {
        serde_json::from_slice(&self.docker_output(args).await?)
            .context("invalid selected Docker inspection fields")
    }

    async fn docker_output(&self, args: &[&str]) -> Result<Vec<u8>> {
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
            Ok(bytes)
        })
        .await
        .context("Docker inspection exceeded three seconds")?
    }

    pub async fn docker_clock(&self) -> Result<DateTime<FixedOffset>> {
        let value = self
            .docker(&["info", "--format", "{{json .SystemTime}}"])
            .await?;
        DateTime::parse_from_rfc3339(value.as_str().context("Docker SystemTime missing")?)
            .context("invalid Docker SystemTime")
    }

    pub async fn open_worker_event_stream(
        &self,
        since: DateTime<FixedOffset>,
        daemon_sample_before: Instant,
        scenario_end: Instant,
    ) -> Result<LiveWorkerEvents> {
        let span = scenario_end
            .saturating_duration_since(daemon_sample_before)
            .checked_sub(LIVE_EVIDENCE_RESERVE)
            .context("insufficient time for a finite Docker event stream")?;
        let earliest_end = daemon_sample_before + span;
        ensure!(
            earliest_end > Instant::now(),
            "Docker event stream cutoff already expired"
        );
        let until = since
            .checked_add_signed(chrono::TimeDelta::from_std(span)?)
            .context("Docker event stream cutoff is not representable")?;
        let socket = self
            .endpoint
            .strip_prefix("unix://")
            .context("missing Unix Docker endpoint")?;
        ensure!(
            Path::new(socket).is_absolute(),
            "Docker events require one absolute Unix socket"
        );
        let http = reqwest::Client::builder()
            .unix_socket(PathBuf::from(socket))
            .http1_only()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let filters = json!({
            "type": ["container"],
            "label": ["agentsandbox.purpose=run"],
            "event": ["create", "start"]
        });
        let response = tokio::time::timeout(
            Duration::from_secs(3),
            http.get("http://localhost/events")
                .query(&[
                    ("since", docker_event_api_timestamp(since)),
                    ("until", docker_event_api_timestamp(until)),
                    ("filters", filters.to_string()),
                ])
                .send(),
        )
        .await
        .context("Docker daemon did not acknowledge the event stream before submission")?
        .context("open Docker event stream on the selected Unix socket")?;
        // Require a framed stream so a broken connection is not a valid EOF.
        ensure!(
            response.status() == reqwest::StatusCode::OK
                && response.version() == reqwest::Version::HTTP_11
                && response
                    .headers()
                    .get(reqwest::header::TRANSFER_ENCODING)
                    .is_some_and(|value| value.as_bytes() == b"chunked"),
            "Docker daemon did not open a chunked HTTP/1.1 event stream (status {}, version {:?})",
            response.status(),
            response.version()
        );
        let mut collector = tokio::task::JoinSet::new();
        collector.spawn(collect_live_worker_events(
            response,
            since,
            until,
            earliest_end,
        ));
        Ok(LiveWorkerEvents { until, collector })
    }

    pub async fn session_worker_events(
        &self,
        session: Uuid,
        since: DateTime<FixedOffset>,
        observed_started_at: DateTime<FixedOffset>,
        until: DateTime<FixedOffset>,
    ) -> Result<Vec<ContainerEvent>> {
        ensure!(
            !session.is_nil(),
            "Docker event history requires one session"
        );
        let (since_arg, until_arg) = worker_event_window(since, observed_started_at, until)?;
        let session = session.to_string();
        let bytes = self.docker_output(&[
            "events", "--since", &since_arg, "--until", &until_arg,
            "--filter", "type=container", "--filter", &format!("label=agentsandbox.session={session}"),
            "--format", r#"{"action":{{json .Action}},"id":{{json .Actor.ID}},"session":{{json (index .Actor.Attributes "agentsandbox.session")}},"purpose":{{json (index .Actor.Attributes "agentsandbox.purpose")}},"wall_ns":{{json .TimeNano}},"exit_code":{{json (index .Actor.Attributes "exitCode")}},"signal":{{json (index .Actor.Attributes "signal")}}}"#,
        ]).await?;
        let mut workers = Vec::new();
        for line in std::str::from_utf8(&bytes)?.lines() {
            let event: ContainerEvent =
                serde_json::from_str(line).context("invalid selected Docker event fields")?;
            let time = DateTime::from_timestamp_nanos(event.wall_ns);
            ensure!(
                event.session == session
                    && event.id.len() == 64
                    && event.id.bytes().all(|b| b.is_ascii_hexdigit())
                    && time >= since
                    && time <= until,
                "Docker event escaped selected session or time interval, or lacked a full CID"
            );
            match event.purpose.as_str() {
                "run" => workers.push(event),
                "import" | "export" => {} // Session helpers are expected to start independently.
                _ => bail!("Docker session event has no recognized sandbox purpose"),
            }
        }
        Ok(workers)
    }

    pub async fn inspect(&self, cid: &str) -> Result<Value> {
        ensure!(
            cid.len() == 64 && cid.bytes().all(|b| b.is_ascii_hexdigit()),
            "require immutable full CID"
        );
        self.docker(&["container", "inspect", "--format", r#"{"id":{{json .Id}},"image":{{json .Image}},"status":{{json .State.Status}},"running":{{json .State.Running}},"started_at":{{json .State.StartedAt}},"finished_at":{{json .State.FinishedAt}},"exit_code":{{json .State.ExitCode}},"oom_killed":{{json .State.OOMKilled}},"restarts":{{json .RestartCount}},"network":{{json .HostConfig.NetworkMode}},"privileged":{{json .HostConfig.Privileged}},"bind_count":{{if eq (json .HostConfig.Binds) "null"}}0{{else}}{{len .HostConfig.Binds}}{{end}},"mounts":[{{range $i, $m := .Mounts}}{{if $i}},{{end}}{"type":{{json $m.Type}},"name":{{json $m.Name}},"destination":{{json $m.Destination}},"rw":{{json $m.RW}},"driver":{{json $m.Driver}}}{{end}}]}"#, cid]).await
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
    graceful_shutdown: bool,
}
impl OwnedProcess {
    pub fn spawn(
        command: Command,
        log: &Path,
        readiness: bool,
    ) -> Result<(Self, Option<mpsc::Receiver<String>>)> {
        Self::spawn_with_selection(command, log, readiness, None)
    }

    fn spawn_with_selection(
        mut command: Command,
        log: &Path,
        readiness: bool,
        client_selection: Option<&Path>,
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
        if let Some(client_selection) = client_selection {
            command.env(BROKER_SELECTION_ENV, client_selection);
        }
        let mut child = command.spawn().context("spawn owned native process")?;
        let pipe = child.stderr.take().context("native child stderr")?;
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || drain_log(pipe, file, readiness.then_some(send)));
        Ok((
            Self {
                child,
                graceful_shutdown: false,
            },
            readiness.then_some(receive),
        ))
    }
    pub fn running(&mut self) -> Result<()> {
        ensure!(
            self.child.try_wait()?.is_none(),
            "owned native process exited; inspect its private bounded stderr"
        );
        Ok(())
    }
    pub fn has_exited(&mut self) -> io::Result<bool> {
        Ok(self.child.try_wait()?.is_some())
    }
    pub fn id(&self) -> u32 {
        self.child.id()
    }
    pub fn kill_owned(&mut self) -> Result<ExitStatus> {
        self.running()?;
        ensure!(
            !self.graceful_shutdown,
            "this child may only be terminated gracefully"
        );
        self.child.kill().context("kill only the owned Child")?;
        self.reap(Duration::from_secs(3))
    }
    pub fn require_graceful_shutdown(&mut self) {
        self.graceful_shutdown = true;
    }
    pub fn terminate_owned(&mut self) -> Result<ExitStatus> {
        self.running()?;
        let pid =
            libc::pid_t::try_from(self.child.id()).context("owned process ID out of range")?;
        // SAFETY: the Child is live or awaiting reap and its positive PID is owned by this fixture.
        if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
            return Err(std::io::Error::last_os_error()).context("terminate the owned Child");
        }
        self.reap(Duration::from_secs(7))
    }
    fn reap(&mut self, wait: Duration) -> Result<ExitStatus> {
        let limit = Instant::now() + wait;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            ensure!(
                Instant::now() < limit,
                "owned process did not reap after its shutdown signal"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            if self.graceful_shutdown {
                let _ = self.terminate_owned();
                return;
            }
            let _ = self.child.kill();
            let limit = Instant::now() + Duration::from_secs(2);
            while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < limit {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        // This owns only the exact children spawned by the fixture, never the enforcer.
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
    selection: &Selection,
    evidence: &Evidence,
    config: &Path,
    policy: &Path,
) -> Result<(OwnedProcess, String)> {
    let mut command = Command::new(&selection.binary);
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
            "--max-events",
            "10000",
        ])
        .arg("--worker-max-seconds")
        .arg(selection.timeout_seconds.to_string())
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

pub fn broker(selection: &Selection, evidence: &Evidence, generation: u8) -> Result<OwnedProcess> {
    let mut command = Command::new(&selection.binary);
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

pub fn run_client(
    selection: &Selection,
    evidence: &Evidence,
    client_selection: &Path,
    operation: Uuid,
    argv: &[String],
) -> Result<OwnedProcess> {
    let mut command = Command::new(&selection.binary);
    command
        .current_dir(evidence.root.join("source"))
        .args(["run", "--operation-id"])
        .arg(operation.to_string())
        .args(["--broker-wait-seconds", "600", "--timeout"])
        .arg(selection.timeout_seconds.to_string())
        .arg("--project")
        .arg(evidence.root.join("source"))
        .arg("--")
        .args(argv);
    Ok(OwnedProcess::spawn_with_selection(
        command,
        &evidence.root.join("logs/run-cli.stderr"),
        false,
        Some(client_selection),
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

const TERMINAL_FAILURE_STEPS: &[&str] = &[
    "terminal_observation_ack",
    "result_audit",
    "cleanup_confirmation",
    "cleanup_ack",
];

/// Test-only projection; arbitrary audit payloads must never enter test logs.
pub fn batch_failure_marker(page: &Value) -> Result<Option<Value>> {
    let items = page["items"]
        .as_array()
        .context("missing diagnostic items")?;
    ensure!(items.len() <= 1, "diagnostic filter exceeded its limit");
    let Some(event) = items.first() else {
        return Ok(None);
    };
    project_batch_failure_event(event).map(Some)
}

fn project_batch_failure_event(event: &Value) -> Result<Value> {
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
    let broker_step = match data.get("broker_step") {
        Some(Value::String(label)) if TERMINAL_FAILURE_STEPS.contains(&label.as_str()) => {
            Some(label.as_str())
        }
        label => optional_failure_label(
            label,
            &[
                "activity_spool",
                "failure_observed",
                "ownership_commit",
                "ownership_reload",
                "created_inspection",
                "created_exchange",
                "created_exchange_reload",
                "created_exchange_prepare_pending",
                "created_exchange_publish_pending",
                "arm_exchange",
                "registration",
                "load_after_registration",
                "full3_binding_publication",
                "readiness_before_claim",
                "preclaim_choice",
                "clock_before_claim",
                "native_claim_reserve",
                "native_claim_consume",
                "claim_exchange",
                "generation_before_prepare",
                "prepare",
            ],
        )?,
    };
    let enforcer_stage = nested_failure_stage(
        data.get("enforcer"),
        &["register", "before_claim", "prepare", "before_spawn"],
    )?;
    let runtime_stage = nested_failure_stage(
        data.get("runtime"),
        &[
            "observe_only",
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
    Ok(json!({
        "event_id":event_id,"audit_seq":audit_seq,"kind":"broker.batch.failure",
        "phase":phase,"broker_step":broker_step,"enforcer_stage":enforcer_stage,
        "runtime_stage":runtime_stage,"runtime_effect":"unknown"
    }))
}

pub fn terminal_failure_step(
    page: &Value,
    session_id: &str,
    admission_seq: i64,
) -> Option<&'static str> {
    if page.get("next_cursor") != Some(&Value::Null) {
        return None;
    }
    let [event] = page["items"].as_array()?.as_slice() else {
        return None;
    };
    if event["session_id"].as_str() != Some(session_id) || event["provenance"] != "observed" {
        return None;
    }
    let marker = batch_failure_marker(page).ok()??;
    if marker["phase"] != "worker" || marker["audit_seq"].as_i64()? <= admission_seq {
        return None;
    }
    let step = marker["broker_step"].as_str()?;
    TERMINAL_FAILURE_STEPS
        .iter()
        .copied()
        .find(|label| *label == step)
}

#[derive(Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalFailureRead {
    #[default]
    NotAttempted,
    AdmissionUnconfirmed,
    BudgetUnavailable,
    ReadFailed,
    Absent,
    Ambiguous,
    Unverified,
    Observed,
}

pub fn terminal_failure_read(
    page: &Value,
    session_id: &str,
    admission_seq: i64,
) -> (TerminalFailureRead, Option<&'static str>) {
    if let Some(step) = terminal_failure_step(page, session_id, admission_seq) {
        return (TerminalFailureRead::Observed, Some(step));
    }
    let read = match page["items"].as_array() {
        Some(items) if items.len() > 1 => TerminalFailureRead::Ambiguous,
        Some(items) if items.is_empty() && page.get("next_cursor") == Some(&Value::Null) => {
            TerminalFailureRead::Absent
        }
        _ => TerminalFailureRead::Unverified,
    };
    (read, None)
}

/// Diagnostic only. Page completeness and event identity do not change the
/// existing single-event authority check.
#[derive(serde::Serialize)]
pub struct ExecutorFailurePage {
    cursor: &'static str,
    pub(super) rows: Option<Vec<ExecutorFailureRow>>,
}

#[derive(serde::Serialize)]
pub(super) struct ExecutorFailureRow {
    session_matches: bool,
    runner_source: bool,
    observed_provenance: bool,
    after_admission: bool,
    phase: Option<LocalFailurePhaseV1>,
    broker_step: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    registration: Option<Value>,
    #[serde(skip)]
    pub(super) audit_key: Option<(Uuid, i64)>,
}

pub fn executor_failure_page(
    page: &Value,
    session: &str,
    admission_seq: i64,
) -> ExecutorFailurePage {
    let cursor = match page.get("next_cursor") {
        None => "unknown",
        Some(Value::Null) => "complete",
        Some(_) => "present",
    };
    let rows = page["items"]
        .as_array()
        .filter(|items| items.len() <= 2)
        .map(|items| {
            items
                .iter()
                .map(|event| {
                    let session_matches = event["session_id"].as_str() == Some(session);
                    let runner_source = event["source"] == "runner";
                    let observed_provenance = event["provenance"] == "observed";
                    let after_admission =
                        event["seq"].as_i64().is_some_and(|seq| seq > admission_seq);
                    let marker = project_batch_failure_event(event).ok();
                    let audit_key = marker
                        .as_ref()
                        .filter(|_| session_matches && observed_provenance && after_admission)
                        .and_then(|marker| {
                            Some((
                                Uuid::parse_str(marker["event_id"].as_str()?).ok()?,
                                marker["audit_seq"].as_i64()?,
                            ))
                        });
                    ExecutorFailureRow {
                        session_matches,
                        runner_source,
                        observed_provenance,
                        after_admission,
                        phase: marker.as_ref().and_then(|marker| {
                            serde_json::from_value(marker["phase"].clone()).ok()
                        }),
                        broker_step: marker
                            .as_ref()
                            .and_then(|marker| marker["broker_step"].as_str().map(str::to_owned)),
                        registration: audit_key
                            .filter(|_| event["data"]["broker_step"] == "registration")
                            .and_then(|_| project_registration(&event["data"]["registration"])),
                        audit_key,
                    }
                })
                .collect()
        });
    ExecutorFailurePage { cursor, rows }
}

fn project_registration(value: &Value) -> Option<Value> {
    let stage = value["stage"].as_str()?;
    let arm_state = value["arm_state"].as_str()?;
    match (stage, arm_state) {
        ("reserve_prospective", "pending") => {
            let mut projected = json!({"stage":stage,"arm_state":arm_state});
            if let Some(mode @ ("fresh" | "replay")) = value["reservation_mode"].as_str() {
                projected["reservation_mode"] = json!(mode);
            }
            if value["enforcer_code"].is_string()
                && let Ok(code) = EnforcerError::deserialize(&value["enforcer_code"])
            {
                projected["enforcer_code"] = json!(code);
            }
            Some(projected)
        }
        ("begin_exchange", "pending")
        | (
            "build_registration" | "bind_prospective" | "restore_assigned" | "register_legacy",
            "acknowledged",
        ) => Some(json!({"stage":stage,"arm_state":arm_state})),
        _ => None,
    }
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

pub fn worker_admission_seq(
    events: &[Value],
    worker: &agentsandbox::control::WorkerV1,
) -> Option<i64> {
    let mut admissions = events.iter().filter(|event| {
        let data = &event["data"];
        let input = &data["input"];
        let admitted = &data["worker_record"]["worker"];
        event["kind"] == "control.supervision.transition"
            && event["source"] == "gateway"
            && event["provenance"] == "observed"
            && event["session_id"] == worker.launch.session_id
            && data["version"].as_u64() == Some(2)
            && input["action"] == "admission"
            && input["request"]["admission"]["operation_id"] == worker.admission_operation_id
            && admitted["worker_id"] == worker.worker_id
            && admitted["attempt"]["attempt_id"] == worker.attempt.attempt_id
    });
    let admission = admissions.next()?;
    if admissions.next().is_some() {
        return None;
    }
    admission["seq"].as_i64().filter(|seq| *seq > 0)
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
