//! Test-only control harness. Real binaries/IPC, synthetic unavailable target
//! metadata, no engine, worker, model or SyntheticReady. Never linked in product.
use agentsandbox::{
    broker::{
        clock::{HostClock, NativeHostClock},
        enforcer::RegistrationV1,
        local_ipc::{EndpointRecordV1, read_frame, write_frame},
        service::protocol::BrokerErrorCodeV1,
    },
    control::{ArmReceiptV1, ArmV1},
    control_fs::Directory,
    runtime::{OwnedContainerSnapshot, RuntimeAuthority},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{FileExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
    time::Instant,
};
use uuid::Uuid;

pub const BIN: &str = env!("CARGO_BIN_EXE_agentsandbox");
pub const TENANT: &str = "tenant-d7-real-cli";
pub const PROFILE: &str = "d7-real-cli";
pub const TOKEN: &str = "synthetic-d7-cli-runner-aaaaaaaaaaaaaaaa";
pub const WAIT: Duration = Duration::from_secs(15);
pub const MAX_BROKER_STDERR_BYTES: usize = 1024 * 1024;

pub struct BrokerStderr {
    file: fs::File,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum GateComponent {
    Broker,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum GateEvent {
    LaunchAdmissionRefused,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum GateStage {
    StartGate,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartGateRecord {
    version: u8,
    component: GateComponent,
    event: GateEvent,
    stage: GateStage,
    operation_id: Uuid,
    reason_code: BrokerErrorCodeV1,
}
impl BrokerStderr {
    pub fn create(fixture: &Fixture) -> Result<Self> {
        let file = fs::OpenOptions::new()
            .read(true)
            .append(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(fixture.root.join("legacy-broker.stderr"))?;
        Ok(Self { file })
    }
    fn stdio(&self) -> Result<Stdio> {
        Ok(self.file.try_clone()?.into())
    }
    fn read_bounded(&self) -> Result<Vec<u8>> {
        let metadata = self.file.metadata()?;
        // SAFETY: read-only UID query for this owned private capture.
        ensure!(
            metadata.is_file()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o7777 == 0o600
                && metadata.len() <= MAX_BROKER_STDERR_BYTES as u64,
            "private broker stderr metadata/size differs"
        );
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let remaining = (MAX_BROKER_STDERR_BYTES + 1).saturating_sub(bytes.len());
            if remaining == 0 {
                break;
            }
            let count = self
                .file
                .read_at(&mut buffer[..remaining.min(4096)], bytes.len() as u64)?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        ensure!(
            bytes.len() <= MAX_BROKER_STDERR_BYTES
                && self.file.metadata()?.len() <= MAX_BROKER_STDERR_BYTES as u64,
            "private broker stderr exceeded its bound"
        );
        Ok(bytes)
    }
    pub fn startup_fence_confirmed(&self) -> Result<bool> {
        const MARKER: &[u8] = b"broker recovery: Legacy native admission fence unavailable; new starts remain refused";
        Ok(self
            .read_bounded()?
            .split(|byte| *byte == b'\n')
            .any(|line| line == MARKER))
    }
    pub fn exact_start_gate_refusal(&self, operation: Uuid) -> Result<bool> {
        let mut count = 0usize;
        for line in self.read_bounded()?.split(|byte| *byte == b'\n') {
            if let Ok(StartGateRecord {
                version: 1,
                component: GateComponent::Broker,
                event: GateEvent::LaunchAdmissionRefused,
                stage: GateStage::StartGate,
                operation_id,
                reason_code: BrokerErrorCodeV1::NeedsReconciliation,
            }) = serde_json::from_slice::<StartGateRecord>(line)
                && operation_id == operation
            {
                count += 1;
            }
        }
        Ok(count == 1)
    }
    pub fn check_bound(&self) -> Result<()> {
        self.read_bounded().map(|_| ())
    }
}

pub struct Fixture {
    temp: Option<tempfile::TempDir>,
    pub root: PathBuf,
}
pub fn combine(primary: Result<()>, secondary: Result<()>) -> Result<()> {
    match (primary, secondary) {
        (Err(first), Err(cleanup)) => Err(first.context(cleanup)),
        (Err(first), Ok(())) => Err(first),
        (Ok(()), Err(cleanup)) => Err(cleanup),
        (Ok(()), Ok(())) => Ok(()),
    }
}
pub fn finish(fixture: &mut Fixture, outcome: Result<()>, cleanup: Result<()>) -> Result<()> {
    combine(outcome, cleanup)?;
    // Only the caller's positive cleanup result permits removal. Any early
    // return, panic or cancellation instead reaches the preserving Drop.
    if let Some(temp) = fixture.temp.take() {
        if let Err(error) = temp.close() {
            eprintln!(
                "private D7 fixture cleanup failed: {}",
                fixture.root.display()
            );
            return Err(error.into());
        }
    }
    Ok(())
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.preserve();
    }
}
impl Fixture {
    pub fn new() -> Result<Self> {
        let base = if cfg!(target_os = "macos") {
            "/private/tmp"
        } else {
            "/tmp"
        };
        let temp = tempfile::Builder::new()
            .prefix("d7i-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(base)?;
        let root = temp.path().canonicalize()?;
        Ok(Self {
            temp: Some(temp),
            root,
        })
    }
    pub fn preserve(&mut self) {
        if let Some(temp) = self.temp.take() {
            eprintln!(
                "private D7 control fixture retained: {}",
                temp.keep().display()
            );
        }
    }
    pub fn dir(&self, name: &str) -> Result<PathBuf> {
        let path = self.root.join(name);
        fs::create_dir(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        Ok(path)
    }
}
pub fn private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
pub fn registry(
    f: &Fixture,
    gateway: &str,
    namespace: Uuid,
    s: u32,
    w: u32,
) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let root = f.dir("state")?;
    let config = f.dir("config")?;
    let source = f.dir("source")?;
    let export = f.dir("export")?;
    private(&config.join("runner"), TOKEN.as_bytes())?;
    // An existing directory with exactly zero entries, not an in-memory flag
    // and not a SQL journal interpreted as granting Fresh authority.
    fs::create_dir(root.join("journals"))?;
    fs::set_permissions(root.join("journals"), fs::Permissions::from_mode(0o700))?;
    let path = config.join("registry.json");
    private(
        &path,
        &serde_json::to_vec(
            &json!({"version":2,"installation_id":Uuid::new_v4(),"root":root,
        "max_enforcer_slots":s,"max_recovery_stop_owners":w,"profiles":[{"id":PROFILE,"gateway":gateway,"tenant_id":TENANT,"control_namespace_uuid":namespace,
        "config_dir":config,"runner_token_file":config.join("runner"),"runtime":"docker","runtime_program":"/usr/bin/false",
        "runtime_endpoint":format!("unix://{}",f.root.join("absent-engine.sock").display()),"image":"never-used-control-fixture","policy_profile":"strict","network":"none","allow_rootful_runtime":false,
        "max_timeout_seconds":300,"allow_capture_output":false,"managed_clients":false,"providers_file":null,"manual_credential_file":null,
        "source_roots":[source],"export_roots":[export],"prearmed_start_profile":{"version":1,"mode":"legacy"}}],"additional_control_secret_files":[]}),
        )?,
    )?;
    Ok((path, root, source))
}
pub fn serve(config: &Path, root: &Path) -> Result<Child> {
    serve_stderr(config, root, Stdio::null())
}
pub fn serve_with_stderr(config: &Path, root: &Path, capture: &BrokerStderr) -> Result<Child> {
    serve_stderr(config, root, capture.stdio()?)
}
fn serve_stderr(config: &Path, root: &Path, stderr: Stdio) -> Result<Child> {
    Ok(Command::new(BIN)
        .args(["broker", "serve", "--config"])
        .arg(config)
        .env_clear()
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .kill_on_drop(true)
        .spawn()?)
}
pub async fn fresh_child(parent: &Path, name: &str, s: u32, w: u32, nonce: Uuid) -> Result<Child> {
    let mut child = Command::new(BIN)
        .args([
            "__broker-enforcer",
            "--legacy-fresh",
            "--role-name",
            name,
            "--root",
        ])
        .arg(parent)
        .args([
            "--max-slots",
            &s.to_string(),
            "--max-recovery-stop-owners",
            &w.to_string(),
        ])
        .env_clear()
        .current_dir(parent)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let written = match child.stdin.take() {
        Some(mut input) => {
            match tokio::time::timeout(WAIT, input.write_all(nonce.as_bytes())).await {
                Ok(result) => result.context("private child nonce write"),
                Err(error) => {
                    Err(anyhow::Error::new(error).context("private child nonce write timeout"))
                }
            }
        }
        None => Err(anyhow::anyhow!("private child pipe missing")),
    };
    if let Err(error) = written {
        return match kill_fixture_child(&mut child).await {
            Ok(()) => Err(error),
            Err(cleanup) => {
                // Keep the directly owned child handle if its real collection
                // did not complete; the caller also retains the private root.
                std::mem::forget(child);
                Err(error.context(cleanup))
            }
        };
    }
    Ok(child)
}
pub async fn normal_broker_stop(child: &mut Child) -> Result<()> {
    ensure!(
        child.try_wait()?.is_none(),
        "broker exited before requested shutdown"
    );
    let pid = child.id().context("owned live broker PID")?;
    // SAFETY: still-running child directly spawned and retained by this test.
    ensure!(
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) } == 0,
        "owned broker signal failed"
    );
    let status = tokio::time::timeout(WAIT, child.wait())
        .await
        .context("broker normal shutdown timeout")??;
    ensure!(status.success(), "broker normal shutdown failed");
    Ok(())
}
pub async fn kill_fixture_child(child: &mut Child) -> Result<()> {
    if child.try_wait()?.is_none() {
        child.start_kill()?;
    }
    tokio::time::timeout(WAIT, child.wait())
        .await
        .context("owned fixture child reap timeout")??;
    // Physical fixture cleanup only. Not native Q, terminal/quota or an engine
    // observation. Only synthetic absent-engine target data is admitted here.
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Announcement {
    version: u32,
    sequence: u32,
    endpoint: EndpointRecordV1,
}
pub fn announcements(role: &Path) -> Result<Vec<PathBuf>> {
    let directory = role.join("announcements");
    let entries = match fs::read_dir(directory) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for e in entries {
        let p = e?.path();
        if p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("e-") && n.ends_with(".json"))
        {
            out.push(p);
        }
    }
    out.sort();
    ensure!(out.len() <= 128, "discovery generation bound");
    Ok(out)
}
fn read_private(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let meta = file.metadata()?;
    // SAFETY: read-only UID query for this private fixture process.
    ensure!(
        meta.is_file()
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.mode() & 0o7777 == 0o600
            && meta.len() <= limit,
        "unexpected fixture private file"
    );
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "private file grew");
    Ok(bytes)
}
#[derive(Clone)]
pub struct Peer {
    pub role: PathBuf,
    pub endpoint: EndpointRecordV1,
    secret: [u8; 32],
}
pub fn latest_endpoint(role: &Path) -> Result<EndpointRecordV1> {
    let paths = announcements(role)?;
    let path = paths.last().context("no role publication")?;
    let bytes = read_private(path, 4096)?;
    let record: Announcement = serde_json::from_slice(&bytes)?;
    ensure!(
        record.version == 1 && record.sequence > 0 && serde_json::to_vec(&record)? == bytes,
        "invalid actual announcement"
    );
    Ok(record.endpoint)
}
impl Peer {
    pub fn read(role: &Path) -> Result<Self> {
        let endpoint = latest_endpoint(role)?;
        let secret: [u8; 32] = read_private(&role.join("auth"), 32)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid actual auth length"))?;
        Ok(Self {
            role: role.into(),
            endpoint,
            secret,
        })
    }
    pub async fn rpc(&self, request: Value) -> Result<Value> {
        let root = Directory::open_existing(&self.role.join("endpoints"))?;
        let mut socket = self.endpoint.connect_checked(&root).await?;
        let challenge: Value = read_frame(&mut socket, 1024, Duration::from_secs(3)).await?;
        let epoch = Uuid::parse_str(challenge["epoch"].as_str().context("challenge epoch")?)?;
        let nonce = Uuid::parse_str(challenge["nonce"].as_str().context("challenge nonce")?)?;
        ensure!(
            epoch == self.endpoint.epoch() && !nonce.is_nil(),
            "actual native challenge mismatch"
        );
        let mut hash = Sha256::new();
        hash.update(b"ai-guard:enforcer-auth:v1\0");
        hash.update(self.secret);
        hash.update(epoch.as_bytes());
        hash.update(nonce.as_bytes());
        write_frame(
            &mut socket,
            &json!({"proof":hex::encode(hash.finalize())}),
            1024,
            Duration::from_secs(3),
        )
        .await?;
        write_frame(&mut socket, &request, 512 * 1024, Duration::from_secs(3)).await?;
        Ok(read_frame(&mut socket, 512 * 1024, Duration::from_secs(5)).await?)
    }
    pub async fn capabilities(&self) -> Result<Value> {
        let reply = self.rpc(json!({"kind":"prospective_capabilities"})).await?;
        ensure!(
            reply["kind"] == "prospective_capabilities",
            "native capability not confirmed"
        );
        Ok(reply["value"].clone())
    }
    pub async fn idle_shutdown(&self) -> Result<()> {
        let reply = self.rpc(json!({"kind":"shutdown_if_idle"})).await?;
        ensure!(
            reply["kind"] == "acknowledged",
            "nonempty/unknown helper not eligible for idle shutdown"
        );
        Ok(())
    }
}
pub async fn wait_peer(role: &Path, child: &mut Child) -> Result<Peer> {
    let until = Instant::now() + WAIT;
    loop {
        ensure!(
            child.try_wait()?.is_none(),
            "owned child exited before publication"
        );
        if let Ok(peer) = Peer::read(role) {
            if peer.rpc(json!({"kind":"status"})).await.is_ok() {
                return Ok(peer);
            }
        }
        ensure!(Instant::now() < until, "actual helper publication timeout");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
pub async fn wait_broker(role: &Path, child: &mut Child) -> Result<()> {
    let until = Instant::now() + WAIT;
    loop {
        ensure!(
            child.try_wait()?.is_none(),
            "owned broker exited before publication"
        );
        if latest_endpoint(role).is_ok() {
            return Ok(());
        }
        ensure!(Instant::now() < until, "actual broker publication timeout");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
pub async fn wait_lease_released(role: &Path) -> Result<()> {
    let until = Instant::now() + WAIT;
    loop {
        let lock = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(role.join(".owner.lock"))?;
        // SAFETY: nonblocking probe of only this test's retained role inode.
        let available =
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        if available {
            ensure!(
                unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) } == 0,
                "unlock fixture probe"
            );
            return Ok(());
        }
        ensure!(
            Instant::now() < until,
            "fixture role still owned after idle shutdown"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
pub fn target(
    role: &Path,
    gateway: &str,
    namespace: Uuid,
) -> Result<(RegistrationV1, Value, String)> {
    let authority: RuntimeAuthority = serde_json::from_value(
        json!({"authority_id":Uuid::new_v4(),"kind":"docker","endpoint":format!("unix://{}",role.join("absent.sock").display()),"engine_id":"synthetic-absent-engine","podman":null,
        "invocation":{"policy_version":2,"kind":"docker","approved_entry":role.join("absent-program"),"canonical_program":role.join("absent-program"),"file_identity":{"device":1,"inode":2,"owner":0,"group":0,"mode":493,"length":1,"sha256":"b".repeat(64)},"environment":{},"cwd":role}}),
    )?;
    let cid = "a".repeat(64);
    let image = format!("sha256:{}", "b".repeat(64));
    let owned: OwnedContainerSnapshot = serde_json::from_value(
        json!({"handle":{"authority_fingerprint":authority.fingerprint(),"session":Uuid::new_v4(),"operation":Uuid::new_v4(),"id":cid,"name":"software-only-d7-target","configuration_fingerprint_version":2},"observation":{"id":cid,"name":"software-only-d7-target","image_id":image,"restart_count":0,"process":{"status":"running","running":true,"started_at":"2026-11-17T00:00:00Z","finished_at":null,"exit_code":null,"oom_killed":false},"allocation":{"cpu":{"quota":100000,"period":100000},"memory_bytes":1073741824_u64,"pids_limit":256},"configuration_fingerprint":"c".repeat(64),"configuration_fingerprint_version":2,"observed_at":"2026-11-17T00:00:00Z","diagnostics":[]}}),
    )?;
    let clock = NativeHostClock::new()?.sample()?;
    let arm: ArmV1 = serde_json::from_value(
        json!({"version":1,"operation_id":Uuid::new_v4(),"worker_id":Uuid::new_v4(),"attempt_id":Uuid::new_v4(),"expected_revision":1,"intent_hash":"e".repeat(64),"subject":{"kind":"worker"},"inspection":{"authority_id":authority.authority_id,"runtime_kind":"docker","container_id":cid,"stable_name":"software-only-d7-target","volume_name":"software-only-d7-work","image_id":image,"spec_hash":"d".repeat(64),"cpu_millis":1000,"memory_bytes":1073741824_u64,"pids_limit":256,"network":"none","security_preset":"fixed_sandbox_v2","restart_policy":"no"},"authority_binding_hash":authority.fingerprint(),"configuration_fingerprint_version":2,"configuration_fingerprint":"c".repeat(64),"deadline_ns_decimal":agentsandbox::broker::clock::checked_deadline(clock.continuous_ns()?,300)?.to_string(),"clock":clock,"timeout_seconds":300}),
    )?;
    let receipt = ArmReceiptV1 {
        version: 1,
        arm_id: arm.operation_id.clone(),
        worker_id: arm.worker_id.clone(),
        attempt_id: arm.attempt_id.clone(),
        input_hash: arm.input_hash()?,
        control_namespace_uuid: namespace.to_string(),
        audit_seq: 1,
        audit_hash: "f".repeat(64),
    };
    let prospective = json!({"control_origin_hash":hex::encode(Sha256::digest(gateway.as_bytes())),"tenant_id":TENANT,"arm":arm,"authority":authority,"owned":owned});
    let registration = RegistrationV1::new(
        hex::encode(Sha256::digest(gateway.as_bytes())),
        TENANT.into(),
        arm,
        receipt,
        authority,
        owned,
    )?;
    let mut hash = Sha256::new();
    hash.update(b"ai-guard:enforcer-registration:v1\0");
    hash.update(serde_json::to_vec(&registration)?);
    Ok((registration, prospective, hex::encode(hash.finalize())))
}
