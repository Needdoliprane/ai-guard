//! S0 RED only: real listener, authenticated IPC, native actor and flock.
//! Gates pause after authentication / before runtime reopen; never forge Ready.
use super::*;
use crate::{
    broker::{clock::checked_deadline, discovery::load_endpoint},
    control::{
        ArmReceiptV1, ArmSubjectV1, ArmV1, NetworkV1, RestartPolicyV1, RuntimeInspectionV1,
        RuntimeKindV1, SecurityPresetV1,
    },
    runtime::{OwnedContainerSnapshot, RuntimeAuthority},
};
use serde_json::json;
use std::{
    fs,
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Condvar, OnceLock, Weak},
};
use tokio::net::UnixListener;

const CHILD_ROOT: &str = "AI_GUARD_TEST_S0_ROLE";
const CHILD_CONTROL: &str = "AI_GUARD_TEST_S0_CONTROL";
const CHILD_MODE: &str = "AI_GUARD_TEST_S0_MODE";
const CHILD_TEST: &str = "broker::enforcer::server::lifetime_tests::native_server_lifetime_child";
const FIXTURE_KEY: [u8; 32] = [0x5a; 32];
const CONTROL_LIMIT: u32 = 4096;
const WAIT: Duration = Duration::from_secs(5);
const PRIVATE_TMP: &str = if cfg!(target_os = "macos") {
    "/private/tmp"
} else {
    "/tmp"
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Ipc,
    Actor,
    Idle,
}
impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Ipc => "ipc",
            Self::Actor => "actor",
            Self::Idle => "idle",
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    state_present: bool,
    progress: u64,
    progress_ticks: u64,
    progress_started: bool,
    progress_exited: bool,
    ipc_active: u64,
    authenticated: u64,
    actor_active: u64,
    actor_entered: u64,
    actor_gate_expired: bool,
    actor_arm: Option<String>,
    actor_digest: Option<String>,
}
struct Probe {
    mode: Mode,
    state: Mutex<Weak<State>>,
    changed: watch::Sender<()>,
    progress_ticks: AtomicU64,
    progress_started: AtomicBool,
    progress_exited: AtomicBool,
    ipc_active: AtomicU64,
    authenticated: AtomicU64,
    release_ipc: AtomicBool,
    actor_active: AtomicU64,
    actor_entered: AtomicU64,
    actor_gate_expired: AtomicBool,
    actor_identity: Mutex<Option<(String, String)>>,
    actor_gate: Mutex<bool>,
    actor_wake: Condvar,
}
static PROBE: OnceLock<Arc<Probe>> = OnceLock::new();
impl Probe {
    fn new(mode: Mode) -> Arc<Self> {
        let (changed, _) = watch::channel(());
        Arc::new(Self {
            mode,
            state: Mutex::new(Weak::new()),
            changed,
            progress_ticks: AtomicU64::new(0),
            progress_started: AtomicBool::new(false),
            progress_exited: AtomicBool::new(false),
            ipc_active: AtomicU64::new(0),
            authenticated: AtomicU64::new(0),
            release_ipc: AtomicBool::new(false),
            actor_active: AtomicU64::new(0),
            actor_entered: AtomicU64::new(0),
            actor_gate_expired: AtomicBool::new(false),
            actor_identity: Mutex::new(None),
            actor_gate: Mutex::new(false),
            actor_wake: Condvar::new(),
        })
    }
    fn wake(&self) {
        self.changed.send_replace(());
    }
    fn snapshot(&self) -> Snapshot {
        // Never retain State across an await, gate or probe of the lock.
        let progress = self
            .state
            .lock()
            .unwrap()
            .upgrade()
            .map(|state| state.progress.load(Ordering::SeqCst));
        let identity = self.actor_identity.lock().unwrap().clone();
        Snapshot {
            state_present: progress.is_some(),
            progress: progress.unwrap_or(0),
            progress_ticks: self.progress_ticks.load(Ordering::SeqCst),
            progress_started: self.progress_started.load(Ordering::SeqCst),
            progress_exited: self.progress_exited.load(Ordering::SeqCst),
            ipc_active: self.ipc_active.load(Ordering::SeqCst),
            authenticated: self.authenticated.load(Ordering::SeqCst),
            actor_active: self.actor_active.load(Ordering::SeqCst),
            actor_entered: self.actor_entered.load(Ordering::SeqCst),
            actor_gate_expired: self.actor_gate_expired.load(Ordering::SeqCst),
            actor_arm: identity.as_ref().map(|value| value.0.clone()),
            actor_digest: identity.map(|value| value.1),
        }
    }
    async fn wait(&self, ready: impl Fn(&Snapshot) -> bool) -> Result<Snapshot, String> {
        let mut changed = self.changed.subscribe();
        tokio::time::timeout(WAIT, async {
            loop {
                let snapshot = self.snapshot();
                if ready(&snapshot) {
                    return Ok(snapshot);
                }
                changed
                    .changed()
                    .await
                    .map_err(|_| "probe channel closed".to_string())?;
            }
        })
        .await
        .map_err(|_| "causal probe deadline expired".to_string())?
    }
}

pub(super) fn observe_state(state: &Arc<State>) {
    if let Some(probe) = PROBE.get() {
        *probe.state.lock().unwrap() = Arc::downgrade(state);
        probe.wake();
    }
}
pub(super) struct ProgressObservation(Option<Arc<Probe>>);
impl ProgressObservation {
    pub(super) fn enter() -> Self {
        let probe = PROBE.get().cloned();
        if let Some(probe) = &probe {
            probe.progress_started.store(true, Ordering::SeqCst);
            probe.wake();
        }
        Self(probe)
    }
}
impl Drop for ProgressObservation {
    fn drop(&mut self) {
        if let Some(probe) = &self.0 {
            probe.progress_exited.store(true, Ordering::SeqCst);
            probe.wake();
        }
    }
}
pub(super) fn progress_tick() {
    if let Some(probe) = PROBE.get() {
        probe.progress_ticks.fetch_add(1, Ordering::SeqCst);
        probe.wake();
    }
}
pub(super) struct IpcObservation(Arc<Probe>);
impl Drop for IpcObservation {
    fn drop(&mut self) {
        self.0.ipc_active.fetch_sub(1, Ordering::SeqCst);
        self.0.wake();
    }
}
pub(super) async fn authenticated_owner() -> Option<IpcObservation> {
    let probe = PROBE.get()?.clone();
    let mut changed = probe.changed.subscribe();
    probe.authenticated.fetch_add(1, Ordering::SeqCst);
    probe.ipc_active.fetch_add(1, Ordering::SeqCst);
    let owner = IpcObservation(Arc::clone(&probe));
    probe.wake();
    while probe.mode == Mode::Ipc && !probe.release_ipc.load(Ordering::SeqCst) {
        if changed.changed().await.is_err() {
            break;
        }
    }
    Some(owner)
}
pub(super) struct ActorObservation(Arc<Probe>);
impl Drop for ActorObservation {
    fn drop(&mut self) {
        self.0.actor_active.fetch_sub(1, Ordering::SeqCst);
        self.0.wake();
    }
}
pub(super) fn actor_before_reopen(actor: &Actor) -> Option<ActorObservation> {
    let probe = PROBE.get()?.clone();
    probe.actor_active.fetch_add(1, Ordering::SeqCst);
    *probe.actor_identity.lock().unwrap() = Some((
        actor.registration.arm.operation_id.clone(),
        actor.digest.clone(),
    ));
    probe.actor_entered.fetch_add(1, Ordering::SeqCst);
    let owner = ActorObservation(Arc::clone(&probe));
    probe.wake();
    let gate = probe.actor_gate.lock().unwrap();
    let (mut gate, deadline) = probe
        .actor_wake
        .wait_timeout_while(gate, WAIT * 3, |released| !*released)
        .unwrap();
    if deadline.timed_out() {
        probe.actor_gate_expired.store(true, Ordering::SeqCst);
        probe.wake();
        // Expiration diagnoses a slow/broken harness. It must never release
        // the actor into a runtime. Only cleanup of this owned child ends it.
        while !*gate {
            gate = probe.actor_wake.wait(gate).unwrap();
        }
    }
    Some(owner)
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Control {
    Snapshot,
    WaitAuthenticated,
    WaitActor,
    WaitNoIpc,
    AbortListener,
    WaitProgress { after_tick: u64 },
    ReleaseIpc,
    JoinIdle,
    WaitIdleGone,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    snapshot: Snapshot,
    listener_joined: bool,
    setup_error: Option<String>,
}
async fn child_main(root: Directory, mut control: UnixStream, mode: Mode) -> Result<(), String> {
    let probe = Probe::new(mode);
    PROBE
        .set(Arc::clone(&probe))
        .map_err(|_| "probe already installed")?;
    let serving_root = root.clone();
    let mut listener = Some(tokio::spawn(async move {
        run_internal_enforcer(&serving_root, 8).await
    }));
    probe
        .wait(|value| value.progress_started && value.progress_ticks > 0)
        .await?;
    // publish_endpoint follows the native clock spawn; use its real discovery
    // record as the readiness handshake, not a fabricated Ready marker.
    tokio::time::timeout(WAIT, async {
        loop {
            if load_endpoint(&root)
                .map_err(|_| "endpoint read failed")?
                .is_some()
            {
                return Ok::<_, String>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| "endpoint not published")??;
    write_frame(
        &mut control,
        &Report {
            snapshot: probe.snapshot(),
            listener_joined: false,
            setup_error: None,
        },
        CONTROL_LIMIT,
        WAIT,
    )
    .await
    .map_err(|_| "ready report failed")?;
    loop {
        let command: Control = read_frame(&mut control, CONTROL_LIMIT, WAIT * 3)
            .await
            .map_err(|_| "control closed")?;
        let mut joined = false;
        let result = match command {
            Control::Snapshot => Ok(probe.snapshot()),
            Control::WaitAuthenticated => {
                probe
                    .wait(|value| value.ipc_active > 0 && value.authenticated > 0)
                    .await
            }
            Control::WaitActor => {
                probe
                    .wait(|value| {
                        value.actor_entered == 1
                            && value.actor_active == 1
                            && !value.actor_gate_expired
                    })
                    .await
            }
            Control::WaitNoIpc => probe.wait(|value| value.ipc_active == 0).await,
            Control::AbortListener => {
                if let Some(task) = listener.take() {
                    task.abort();
                    joined = task.await.is_err_and(|error| error.is_cancelled());
                }
                if joined {
                    Ok(probe.snapshot())
                } else {
                    Err("listener abort did not join cancellation".into())
                }
            }
            Control::WaitProgress { after_tick } => {
                probe
                    .wait(|value| value.progress_exited || value.progress_ticks >= after_tick + 3)
                    .await
            }
            Control::ReleaseIpc => {
                probe.release_ipc.store(true, Ordering::SeqCst);
                probe.wake();
                Ok(probe.snapshot())
            }
            Control::JoinIdle => {
                if let Some(mut task) = listener.take() {
                    joined = matches!(tokio::time::timeout(WAIT, &mut task).await, Ok(Ok(Ok(()))));
                    if !joined {
                        task.abort();
                        let _ = task.await;
                    }
                }
                if joined {
                    Ok(probe.snapshot())
                } else {
                    Err("idle listener did not return".into())
                }
            }
            Control::WaitIdleGone => {
                let bounded = tokio::time::timeout(WAIT, async {
                    loop {
                        let snapshot = probe.snapshot();
                        if snapshot.progress_exited
                            && !snapshot.state_present
                            && snapshot.ipc_active == 0
                        {
                            return snapshot;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await;
                bounded.map_err(|_| "idle owners did not finish".into())
            }
        };
        let report = match result {
            Ok(snapshot) => Report {
                snapshot,
                listener_joined: joined,
                setup_error: None,
            },
            Err(error) => Report {
                snapshot: probe.snapshot(),
                listener_joined: joined,
                setup_error: Some(error),
            },
        };
        write_frame(&mut control, &report, CONTROL_LIMIT, WAIT)
            .await
            .map_err(|_| "control reply failed")?;
    }
}

#[test]
#[ignore = "internal S0 child; only the owned parent regression invokes this"]
fn native_server_lifetime_child() {
    let root = PathBuf::from(std::env::var_os(CHILD_ROOT).expect("owned S0 role required"));
    let control =
        PathBuf::from(std::env::var_os(CHILD_CONTROL).expect("owned S0 control required"));
    let mode = match std::env::var(CHILD_MODE).as_deref() {
        Ok("ipc") => Mode::Ipc,
        Ok("actor") => Mode::Actor,
        Ok("idle") => Mode::Idle,
        _ => panic!("owned S0 mode required"),
    };
    let root = Directory::open_existing(&root).unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let control = UnixStream::connect(control).await.unwrap();
            child_main(root, control, mode)
                .await
                .expect("S0 child control/observation failed");
        });
}

struct NativeChild(Child);
impl Drop for NativeChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
struct OwnedCase {
    child: Option<NativeChild>,
    control: UnixStream,
    root: Directory,
    temp: Option<tempfile::TempDir>,
}
impl Drop for OwnedCase {
    fn drop(&mut self) {
        // Only the Child object created below is killed, before its owned paths.
        drop(self.child.take());
        if std::thread::panicking()
            && let Some(temp) = self.temp.take()
        {
            eprintln!("private S0 diagnostics: {}", temp.keep().display());
        }
    }
}
impl OwnedCase {
    async fn new(mode: Mode) -> Result<Self, String> {
        let temp = tempfile::Builder::new()
            .prefix("s0-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(PRIVATE_TMP)
            .map_err(|_| "private fixture failed")?;
        let role = temp.path().join("r");
        fs::create_dir(&role).map_err(|_| "private role failed")?;
        fs::set_permissions(&role, fs::Permissions::from_mode(0o700))
            .map_err(|_| "private role mode failed")?;
        let root = Directory::open_existing(
            &role
                .canonicalize()
                .map_err(|_| "role canonicalization failed")?,
        )
        .map_err(|_| "role open failed")?;
        // This fixture is explicitly an existing/cold V1 role. Its empty lock
        // must already be durable; the server no longer bootstraps a missing
        // lock into fresh admission merely because the directory is empty.
        let lock = root
            .persistent_create(".owner.lock", &[])
            .map_err(|_| "fixture existing V1 lock failed")?;
        lock.persistent_sync()
            .map_err(|_| "fixture V1 lock sync failed")?;
        root.sync().map_err(|_| "fixture V1 root sync failed")?;
        let auth = root
            .persistent_create("auth", &FIXTURE_KEY)
            .map_err(|_| "fixture auth failed")?;
        auth.persistent_sync()
            .map_err(|_| "fixture auth sync failed")?;
        root.sync().map_err(|_| "fixture root sync failed")?;
        let control_path = temp.path().join("c");
        let listener = UnixListener::bind(&control_path).map_err(|_| "control bind failed")?;
        fs::set_permissions(&control_path, fs::Permissions::from_mode(0o600))
            .map_err(|_| "control mode failed")?;
        let stderr = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(temp.path().join("child.stderr"))
            .map_err(|_| "child diagnostics failed")?;
        let mut command =
            Command::new(std::env::current_exe().map_err(|_| "libtest path unavailable")?);
        command
            .args(["--exact", CHILD_TEST, "--ignored"])
            .env_clear()
            .env(CHILD_ROOT, &role)
            .env(CHILD_CONTROL, &control_path)
            .env(CHILD_MODE, mode.name())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr));
        // SAFETY: child-only setrlimit; no allocation or lock after fork.
        unsafe {
            command.pre_exec(|| {
                let bound = libc::rlimit {
                    rlim_cur: 64 * 1024,
                    rlim_max: 64 * 1024,
                };
                if libc::setrlimit(libc::RLIMIT_FSIZE, &bound) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
        let child = NativeChild(command.spawn().map_err(|_| "owned child spawn failed")?);
        let (mut control, _) = tokio::time::timeout(WAIT, listener.accept())
            .await
            .map_err(|_| "child control timeout")?
            .map_err(|_| "child control failed")?;
        let ready: Report = read_frame(&mut control, CONTROL_LIMIT, WAIT)
            .await
            .map_err(|_| "child not ready")?;
        if ready.setup_error.is_some() || !ready.snapshot.progress_started {
            return Err("child setup not causal".into());
        }
        Ok(Self {
            child: Some(child),
            control,
            root,
            temp: Some(temp),
        })
    }
    async fn command(&mut self, command: Control) -> Result<Report, String> {
        write_frame(&mut self.control, &command, CONTROL_LIMIT, WAIT)
            .await
            .map_err(|_| "control command failed")?;
        let report: Report = read_frame(&mut self.control, CONTROL_LIMIT, WAIT * 2)
            .await
            .map_err(|_| "control report failed")?;
        if let Some(error) = &report.setup_error {
            return Err(error.clone());
        }
        Ok(report)
    }
    async fn authenticate(&self) -> Result<UnixStream, String> {
        let endpoint = load_endpoint(&self.root)
            .map_err(|_| "discovery failed")?
            .ok_or("endpoint absent")?;
        let endpoints = self
            .root
            .persistent_child("endpoints", false)
            .map_err(|_| "endpoints failed")?
            .ok_or("endpoints absent")?;
        let mut stream = endpoint
            .connect_checked(&endpoints)
            .await
            .map_err(|_| "checked connection failed")?;
        let challenge: Challenge = read_frame(&mut stream, 1024, WAIT)
            .await
            .map_err(|_| "challenge failed")?;
        if challenge.epoch != endpoint.epoch() {
            return Err("epoch mismatch".into());
        }
        let authentication = Authentication {
            proof: proof(&FIXTURE_KEY, &challenge),
        };
        write_frame(&mut stream, &authentication, 1024, WAIT)
            .await
            .map_err(|_| "authentication failed")?;
        Ok(stream)
    }
    fn preserve(&mut self) {
        drop(self.child.take());
        if let Some(temp) = self.temp.take() {
            eprintln!("private S0 diagnostics: {}", temp.keep().display());
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LockIdentity {
    device: u64,
    inode: u64,
}
fn lock_identity(root: &Directory) -> Result<LockIdentity, String> {
    let facts = fs::symlink_metadata(root.path().join(".owner.lock"))
        .map_err(|_| "lock metadata failed")?;
    if !facts.is_file() || facts.nlink() != 1 || facts.mode() & 0o7777 != 0o600 {
        return Err("invalid fixture lock shape".into());
    }
    Ok(LockIdentity {
        device: facts.dev(),
        inode: facts.ino(),
    })
}
fn independent_lock_is_busy(root: &Directory, expected: LockIdentity) -> Result<bool, String> {
    if lock_identity(root)? != expected {
        return Err("lock inode changed before probe".into());
    }
    let busy = match RoleLease::acquire_existing(root) {
        Err(crate::broker::local_ipc::IpcError::Conflict) => true,
        Ok(lease) => {
            drop(lease);
            false
        }
        Err(_) => return Err("independent probe failed structurally".into()),
    };
    if lock_identity(root)? != expected {
        return Err("lock inode changed after probe".into());
    }
    Ok(busy)
}

fn unavailable_registration(root: &Path) -> RegistrationV1 {
    // Public fixture metadata only. The native gate fires before reopen, and
    // this nonexistent program must never execute any host/runtime command.
    let missing = root.join("unavailable-runtime");
    let authority: RuntimeAuthority = serde_json::from_value(json!({
        "authority_id": Uuid::new_v4(), "kind":"docker", "endpoint":format!("unix://{}", root.join("unavailable.sock").display()),
        "engine_id":"synthetic-s0-unavailable", "podman":null,
        "invocation":{"policy_version":2,"kind":"docker","approved_entry":missing,"canonical_program":missing,
            "file_identity":{"device":1,"inode":2,"owner":0,"group":0,"mode":493,"length":1,"sha256":"b".repeat(64)},"environment":{},"cwd":root}
    })).unwrap();
    let inspection = RuntimeInspectionV1 {
        authority_id: authority.authority_id.to_string(),
        runtime_kind: RuntimeKindV1::Docker,
        container_id: "a".repeat(64),
        stable_name: "aig-s0-unavailable".into(),
        volume_name: "aig-s0-unavailable-work".into(),
        image_id: format!("sha256:{}", "b".repeat(64)),
        spec_hash: "d".repeat(64),
        cpu_millis: 1000,
        memory_bytes: 1024 * 1024 * 1024,
        pids_limit: 256,
        network: NetworkV1::None,
        security_preset: SecurityPresetV1::FixedSandboxV2,
        restart_policy: RestartPolicyV1::No,
    };
    let owned: OwnedContainerSnapshot = serde_json::from_value(json!({
        "handle":{"authority_fingerprint":authority.fingerprint(),"session":Uuid::new_v4(),"operation":Uuid::new_v4(),"id":inspection.container_id,"name":inspection.stable_name,"configuration_fingerprint_version":2},
        "observation":{"id":inspection.container_id,"name":inspection.stable_name,"image_id":inspection.image_id,"restart_count":0,
            "process":{"status":"created","running":false,"started_at":null,"finished_at":null,"exit_code":null,"oom_killed":false},
            "allocation":{"cpu":{"quota":100000,"period":100000},"memory_bytes":1024_u64*1024*1024,"pids_limit":256},
            "configuration_fingerprint":"c".repeat(64),"configuration_fingerprint_version":2,"observed_at":"2026-11-01T00:00:00Z","diagnostics":[]}
    })).unwrap();
    let clock = NativeHostClock::new().unwrap().sample().unwrap();
    let arm = ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: Uuid::new_v4().to_string(),
        attempt_id: Uuid::new_v4().to_string(),
        expected_revision: 1,
        intent_hash: "e".repeat(64),
        subject: ArmSubjectV1::Worker,
        inspection,
        authority_binding_hash: authority.fingerprint(),
        configuration_fingerprint_version: 2,
        configuration_fingerprint: "c".repeat(64),
        deadline_ns_decimal: checked_deadline(clock.continuous_ns().unwrap(), 3600)
            .unwrap()
            .to_string(),
        clock,
        timeout_seconds: 3600,
    };
    let receipt = ArmReceiptV1 {
        version: 1,
        arm_id: arm.operation_id.clone(),
        worker_id: arm.worker_id.clone(),
        attempt_id: arm.attempt_id.clone(),
        input_hash: arm.input_hash().unwrap(),
        control_namespace_uuid: Uuid::new_v4().to_string(),
        audit_seq: 1,
        audit_hash: "f".repeat(64),
    };
    RegistrationV1::new(
        "a".repeat(64),
        "tenant-s0-fixture".into(),
        arm,
        receipt,
        authority,
        owned,
    )
    .unwrap()
}

#[derive(Debug)]
struct CaseObservation {
    mode: Mode,
    identity: LockIdentity,
    busy_before: bool,
    owner_before: Snapshot,
    listener_joined: bool,
    busy_after: bool,
    owner_after: Snapshot,
    progress_after: Snapshot,
    idle_released: Option<bool>,
}
async fn exercise(case: &mut OwnedCase, mode: Mode) -> Result<CaseObservation, String> {
    let identity = lock_identity(&case.root)?;
    let busy_before = independent_lock_is_busy(&case.root, identity)?;
    if !busy_before {
        return Err("positive live-listener flock control failed".into());
    }
    let mut stream = case.authenticate().await?;
    let owner_before = if mode == Mode::Ipc {
        case.command(Control::WaitAuthenticated).await?.snapshot
    } else if mode == Mode::Actor {
        let registration = unavailable_registration(case.root.path());
        let arm = registration.arm.operation_id.clone();
        let digest = registration
            .digest()
            .map_err(|_| "fixture registration digest failed")?;
        write_frame(
            &mut stream,
            &Request::Register {
                registration: Box::new(registration),
                generation: 0,
                observe_only: true,
            },
            MAX_FRAME,
            WAIT,
        )
        .await
        .map_err(|_| "register write failed")?;
        let entered = case.command(Control::WaitActor).await?.snapshot;
        let response: Response = read_frame(&mut stream, MAX_FRAME, WAIT)
            .await
            .map_err(|_| "register response failed")?;
        if !matches!(
            response,
            Response::Error {
                code: EnforcerError::NotReady
            }
        ) || entered.actor_arm.as_deref() != Some(arm.as_str())
            || entered.actor_digest.as_deref() != Some(digest.as_str())
        {
            return Err("real dispatcher/native identity control failed".into());
        }
        drop(stream);
        stream = case.authenticate().await?;
        write_frame(
            &mut stream,
            &Request::Slots {
                arm_ids: vec![arm.clone()],
            },
            MAX_FRAME,
            WAIT,
        )
        .await
        .map_err(|_| "slot query failed")?;
        let Response::Slots { values } = read_frame(&mut stream, MAX_FRAME, WAIT)
            .await
            .map_err(|_| "slot response failed")?
        else {
            return Err("reserved slot not visible".into());
        };
        if values.len() != 1
            || values[0].arm_id != arm
            || values[0].registration_hash != digest
            || values[0].phase != SlotPhaseV1::Initializing
            || !values[0].prepared_once
        {
            return Err("registration/ObserveOnly provenance changed".into());
        }
        case.command(Control::WaitNoIpc).await?.snapshot
    } else {
        write_frame(&mut stream, &Request::ShutdownIfIdle {}, MAX_FRAME, WAIT)
            .await
            .map_err(|_| "idle request failed")?;
        if !matches!(
            read_frame::<Response>(&mut stream, MAX_FRAME, WAIT)
                .await
                .map_err(|_| "idle ACK failed")?,
            Response::Acknowledged {}
        ) {
            return Err("idle shutdown refused".into());
        }
        let returned = case.command(Control::JoinIdle).await?;
        if !returned.listener_joined {
            return Err("idle listener was not joined".into());
        }
        let gone = case.command(Control::WaitIdleGone).await?.snapshot;
        return Ok(CaseObservation {
            mode,
            identity,
            busy_before,
            owner_before: gone.clone(),
            listener_joined: true,
            busy_after: independent_lock_is_busy(&case.root, identity)?,
            owner_after: gone.clone(),
            progress_after: gone,
            idle_released: Some(true),
        });
    };
    if !independent_lock_is_busy(&case.root, identity)? {
        return Err("live-owner flock control failed immediately before abort".into());
    }
    let aborted = case.command(Control::AbortListener).await?;
    let busy_after = independent_lock_is_busy(&case.root, identity)?;
    let owner_after = case.command(Control::Snapshot).await?.snapshot;
    let progress_after = case
        .command(Control::WaitProgress {
            after_tick: aborted.snapshot.progress_ticks,
        })
        .await?
        .snapshot;
    let idle_released = if mode == Mode::Ipc {
        case.command(Control::ReleaseIpc).await?;
        drop(stream);
        case.command(Control::WaitNoIpc).await?;
        // Abort currently leaks the clock's State reference. Report that as
        // the progress oracle; don't hide the flock result behind this wait.
        if progress_after.progress_exited {
            case.command(Control::WaitIdleGone).await?;
            Some(!independent_lock_is_busy(&case.root, identity)?)
        } else {
            None
        }
    } else {
        drop(stream);
        None
    };
    Ok(CaseObservation {
        mode,
        identity,
        busy_before,
        owner_before,
        listener_joined: aborted.listener_joined,
        busy_after,
        owner_after,
        progress_after,
        idle_released,
    })
}

#[tokio::test(flavor = "current_thread")]
async fn abandoning_listener_retains_flock_for_authenticated_ipc_and_native_actor() {
    let mut observations = Vec::new();
    for mode in [Mode::Ipc, Mode::Actor, Mode::Idle] {
        let result = match OwnedCase::new(mode).await {
            Ok(mut case) => {
                let observed = exercise(&mut case, mode).await;
                let preserve = match &observed {
                    Err(_) => true,
                    Ok(value) => {
                        (!value.busy_after && mode != Mode::Idle)
                            || !value.progress_after.progress_exited
                    }
                };
                if preserve {
                    case.preserve();
                }
                drop(case); // kill/wait only the child we spawned, before oracles
                observed
            }
            Err(error) => Err(error),
        };
        observations.push((mode, result));
    }
    for (mode, observed) in &observations {
        assert!(
            observed.is_ok(),
            "S0 causal/setup failure for {mode:?}: {observed:?}"
        );
        let value = observed.as_ref().unwrap();
        assert!(
            value.mode == *mode
                && value.identity.inode > 0
                && value.busy_before
                && value.listener_joined,
            "live lock / listener completion control failed: {value:?}"
        );
        match mode {
            Mode::Ipc => assert!(
                value.owner_before.ipc_active > 0
                    && value.owner_after.ipc_active > 0
                    && value.owner_after.authenticated > 0,
                "authenticated IPC must still be owned at the independent probe: {value:?}"
            ),
            Mode::Actor => assert!(
                value.owner_before.ipc_active == 0
                    && value.owner_after.ipc_active == 0
                    && value.owner_after.actor_active == 1
                    && value.owner_after.actor_entered == 1
                    && !value.owner_after.actor_gate_expired,
                "native actor must outlive the waiter, still gated before reopen: {value:?}"
            ),
            Mode::Idle => assert!(
                !value.busy_after
                    && value.idle_released == Some(true)
                    && !value.owner_after.state_present,
                "normal idle must release every owner: {value:?}"
            ),
        }
    }
    assert!(
        observations.iter().all(|(mode, value)| *mode == Mode::Idle
            || value.as_ref().is_ok_and(|value| value.busy_after)),
        "the same flock must remain excluded while an authenticated IPC or native actor is alive: {observations:?}"
    );
    assert!(
        observations.iter().all(|(_, value)| value
            .as_ref()
            .is_ok_and(|value| value.progress_after.progress_exited)),
        "listener abandonment must stop progression independently of business STOP owners: {observations:?}"
    );
}
