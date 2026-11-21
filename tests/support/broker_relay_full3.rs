//! Full3 service composition. Only runtime observations are synthetic.
//! All authorities are created by owned real CLI processes and Core routes.
use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    broker::{
        local_ipc::EndpointRecordV1,
        service::{self, RequestedExecutionV1, SubmitV1, encode_submit, protocol::*},
    },
    control::WorkerStateV1,
    control_fs::{Directory, PrivateFile},
    operations::initialize_config,
    policy::Policy,
    redaction::Redactor,
    relay::{RelayEndpointV1, RelayGrantV1, RelayReceiptV1, channel::helper},
};
use anyhow::{Context, Result, bail, ensure};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::{self, Next},
    response::Response,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::{TcpListener, UnixListener, UnixStream},
    process::{Child, Command},
    sync::{Mutex, oneshot},
    task::{JoinHandle, JoinSet},
    time::{sleep, timeout},
};
use uuid::Uuid;
#[cfg(target_os = "macos")]
#[path = "owned_darwin.rs"]
mod owned_darwin;
#[cfg(target_os = "macos")]
use owned_darwin::DarwinIdentity;
/// The Fresh guardian custody proof uses Darwin audit tokens. Other hosts keep
/// the pre-started guardian fixtures and refuse a Fresh fixture explicitly.
#[cfg(not(target_os = "macos"))]
struct DarwinIdentity;
#[cfg(not(target_os = "macos"))]
impl DarwinIdentity {
    fn capture(_peer: &UnixStream, _broker_pid: u32, _program: &Path) -> Result<Self> {
        anyhow::bail!("Fresh guardian custody requires Darwin audit tokens")
    }
    async fn close(self, _peer: &UnixStream, _broker_pid: u32, _program: &Path) -> Result<()> {
        anyhow::bail!("Fresh guardian custody requires Darwin audit tokens")
    }
}

pub const BIN: &str = env!("CARGO_BIN_EXE_agentsandbox");
pub const A_RUNNER: &str = "broker-handoff-a-runner-public-fixture-0123456789";
pub const B_RUNNER: &str = "broker-handoff-b-runner-public-fixture-0123456789";
const A_VIEWER: &str = "broker-handoff-a-viewer-public-fixture-0123456789";
const B_VIEWER: &str = "broker-handoff-b-viewer-public-fixture-0123456789";
const IMAGE: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

pub struct Faults {
    pub lose_claim: AtomicBool,
    pub lost: AtomicUsize,
    pub claims: AtomicUsize,
    pub sends: AtomicUsize,
    pub acks: AtomicUsize,
    trace: Arc<diagnostics::Trace>,
}
async fn fault(State(f): State<Arc<Faults>>, request: Request, next: Next) -> Response {
    use diagnostics::{Outcome, Phase, Purpose};
    // Classify only; never retain a URI, body, header or identifier. A handler
    // response is not proof that the broker consumed or durably ACKed it.
    let path = request
        .uri()
        .path()
        .strip_prefix("/control")
        .unwrap_or(request.uri().path());
    let phase = if request.method() == Method::GET && path == "/api/v1/control/protocol" {
        Some(Phase::GatewayControlProtocol)
    } else if request.method() == Method::POST {
        match path {
            "/api/decide" | "/api/v1/decide" => Some(Phase::GatewayDecision),
            "/api/v1/workers/supervised-reservations" => Some(Phase::GatewayAdmission),
            p if p.starts_with("/api/v1/workers/") => match p.rsplit('/').next() {
                Some("transitions") => Some(Phase::GatewayTransition),
                Some("arms") => Some(Phase::GatewayArm),
                Some("helper-start-claims") => Some(Phase::GatewayHelperClaim),
                Some("helper-observations") => Some(Phase::GatewayHelperObservation),
                Some("helper-cleanup") => Some(Phase::GatewayHelperCleanup),
                Some("armed-start-claims") => Some(Phase::GatewayWorkerClaim),
                _ => None,
            },
            _ => None,
        }
    } else {
        None
    };
    let diagnostic = phase.map(|phase| f.trace.begin(phase, Purpose::Unknown));
    if request.method() == Method::POST {
        let path = request.uri().path();
        if matches!(
            path,
            "/api/v1/relay/messages" | "/control/api/v1/relay/messages"
        ) {
            f.sends.fetch_add(1, Ordering::SeqCst);
        } else if (path.starts_with("/api/v1/relay/messages/")
            || path.starts_with("/control/api/v1/relay/messages/"))
            && path.ends_with("/ack")
        {
            f.acks.fetch_add(1, Ordering::SeqCst);
        }
    }
    let claim = request.method() == Method::POST
        && request
            .uri()
            .path()
            .starts_with("/control/api/v1/relay/channels/")
        && request.uri().path().ends_with("/claims");
    if claim {
        f.claims.fetch_add(1, Ordering::SeqCst);
    }
    let response = next.run(request).await;
    if let Some(diagnostic) = diagnostic {
        diagnostic.finish(if response.status().is_success() {
            Outcome::Ok
        } else {
            Outcome::NonSuccess
        });
    }
    if claim
        && response.status() == StatusCode::CREATED
        && f.lose_claim.swap(false, Ordering::SeqCst)
    {
        f.lost.fetch_add(1, Ordering::SeqCst);
        let (parts, _) = response.into_parts();
        return Response::from_parts(
            parts,
            Body::from_stream(futures_util::stream::once(async {
                Err::<Vec<u8>, io::Error>(io::Error::from(io::ErrorKind::ConnectionReset))
            })),
        );
    }
    response
}
use agentsandbox::{
    broker::clock::{HostClock, NativeHostClock},
    control::{ArmV1, HostClockSampleV1},
};
use sha2::{Digest, Sha256};
#[path = "broker_relay_runtime.rs"]
mod runtime;
use runtime::{Engine, EngineFailure, diagnostics, engine_connection_observed};
pub use runtime::{
    ImportCounts, contradictory_create_flags_are_refused_before_fixture_mutation, exchange,
    worker_terminal,
};

// Diagnostic bounds are independent of the product's deadlines and readers.
const OBSERVATION_JOBS: usize = 2;
const OBSERVATION_FRAMES: usize = 64;
const OBSERVATION_METADATA: usize = 128;
const OBSERVATION_PAYLOAD_BYTES: u64 = 1024 * 1024;
const OBSERVATION_OUTPUT_BYTES: usize = 4 * 1024;

#[derive(Clone)]
struct ObservedJob {
    job: Uuid,
    // Never rendered. Only a successfully received Status can populate this.
    head: Option<(u64, String)>,
}
#[derive(Default)]
struct ObservationState {
    jobs: Vec<ObservedJob>,
    jobs_limited: bool,
    timeout: Option<TimeoutObservationTicket>,
}
#[derive(Clone)]
pub struct TimeoutObservationTicket {
    broker_root: PathBuf,
    selected: Uuid,
    jobs: Vec<ObservedJob>,
    jobs_limited: bool,
    clock: Option<HostClockSampleV1>,
}
#[derive(Default, serde::Serialize)]
struct MetadataObservation {
    slot: usize,
    selected: bool,
    ipc_head_known: bool,
    ipc_head: u64,
    ipc_frame_size_known: bool,
    ipc_frame_bytes: u64,
    // Every size and revision-file count below is observed AFTER cleanup.
    complete: bool,
    bound_hit: bool,
    entries: usize,
    frames: usize,
    frame_bytes: u64,
    smallest_frame_bytes: u64,
    largest_frame_bytes: u64,
    highest_revision_file: u64,
}
#[derive(Default, serde::Serialize)]
struct RequestAcknowledgements {
    requests: usize,
    // A non-null response in the confirmed immutable local checkpoint only.
    acknowledged: usize,
}
#[derive(Default, serde::Serialize)]
struct DeadlineObservation {
    arms: usize,
    comparable: usize,
    known: bool,
    expired: usize,
    remaining_min_ns: u64,
    remaining_max_ns: u64,
}
impl DeadlineObservation {
    fn observe(&mut self, arm: &ArmV1, clock: Option<&HostClockSampleV1>) {
        self.arms += 1;
        let comparable = (|| -> Result<u64> {
            arm.validate()?;
            let clock = clock.context("unknown capture clock")?;
            let now = clock.continuous_ns()?;
            ensure!(
                clock.method == arm.clock.method
                    && clock.boot_id == arm.clock.boot_id
                    && now >= arm.clock.continuous_ns()?,
                "incomparable capture clock"
            );
            let deadline = arm.deadline_ns_decimal.parse::<u128>()?;
            Ok(u64::try_from(deadline.saturating_sub(now))?)
        })();
        if let Ok(remaining) = comparable {
            self.remaining_min_ns = if self.comparable == 0 {
                remaining
            } else {
                self.remaining_min_ns.min(remaining)
            };
            self.remaining_max_ns = self.remaining_max_ns.max(remaining);
            self.comparable += 1;
            self.expired += usize::from(remaining == 0);
        }
        self.known = self.arms > 0 && self.arms == self.comparable;
    }
}
#[derive(Default, serde::Serialize)]
struct ImportOutcomeObservation {
    exit_zero: usize,
    exit_nonzero: usize,
    exit_unknown: usize,
    terminal_complete: usize,
    cleanup_removed: usize,
    cleanup_not_removed: usize,
    cleanup_removed_unknown: usize,
    cleanup_error_present: usize,
}
impl ImportOutcomeObservation {
    // Called only for this head's validated Import operations. Keep missing
    // values distinct, and never copy transport output or cleanup error text.
    fn observe(&mut self, kind: &str, request: &Value, acknowledged: bool) {
        if !acknowledged {
            return;
        }
        match kind {
            "helper_observation" => {
                let observed = &request["observation"];
                match observed["exit_code"].as_i64() {
                    Some(0) => self.exit_zero += 1,
                    Some(_) => self.exit_nonzero += 1,
                    None => self.exit_unknown += 1,
                }
                self.terminal_complete += usize::from(
                    observed["exit_code"].as_i64().is_some()
                        && observed["running"].as_bool() == Some(false)
                        && !observed["started_at"].is_null()
                        && !observed["finished_at"].is_null(),
                );
            }
            "helper_cleanup" => {
                match request["removed"].as_bool() {
                    Some(true) => self.cleanup_removed += 1,
                    Some(false) => self.cleanup_not_removed += 1,
                    None => self.cleanup_removed_unknown += 1,
                }
                self.cleanup_error_present += usize::from(!request["error"].is_null());
            }
            _ => {}
        }
    }
}
#[derive(Default, serde::Serialize)]
struct ConfirmedFrameObservation {
    known: bool,
    revision: u64,
    bytes: usize,
    version: u64,
    // These are post-cleanup reader costs, never the live writer's costs.
    post_cleanup_read_us: u64,
    post_cleanup_hash_decode_us: u64,
    supervision_known: bool,
    import_helpers: usize,
    import_owned: usize,
    verification_helpers: usize,
    verification_owned: usize,
    import_arms: RequestAcknowledgements,
    import_claims: RequestAcknowledgements,
    import_observations: RequestAcknowledgements,
    import_cleanup: RequestAcknowledgements,
    import_outcomes: ImportOutcomeObservation,
    local_failures_known: bool,
    local_failure_count: usize,
    last_failure_phase: Option<agentsandbox::broker::lifecycle::LocalFailurePhaseV1>,
    last_failure_code: Option<agentsandbox::broker::lifecycle::LocalFailureCodeV1>,
    helper_deadline_at_capture: DeadlineObservation,
    worker_deadline_at_capture: DeadlineObservation,
    // This bounded inspection does not validate the whole chain or bindings.
    chain_validation_known: bool,
    bindings_payload_known: bool,
}
#[derive(serde::Serialize)]
struct TicketObservation {
    version: u8,
    cleanup_joined: bool,
    timeout_selected_known: bool,
    jobs_limited: bool,
    namespace_known: bool,
    capture_clock_known: bool,
    payload_attempted: bool,
    payload_within_bound_known: bool,
    payload_bound_hit: bool,
    metadata_attempted_after_join: bool,
    performance_qualified: bool,
    errors: usize,
    truncated: bool,
    jobs: Vec<MetadataObservation>,
    confirmed_frame: ConfirmedFrameObservation,
}
fn observation_micros(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}
impl TimeoutObservationTicket {
    pub fn emit_after_cleanup(self, cleanup_joined: bool) {
        let mut output = TicketObservation {
            version: 1,
            cleanup_joined,
            timeout_selected_known: self.jobs.iter().any(|entry| entry.job == self.selected),
            jobs_limited: self.jobs_limited,
            namespace_known: false,
            capture_clock_known: self
                .clock
                .as_ref()
                .is_some_and(|clock| clock.validate().is_ok()),
            payload_attempted: false,
            payload_within_bound_known: false,
            payload_bound_hit: false,
            metadata_attempted_after_join: cleanup_joined,
            performance_qualified: false,
            errors: 0,
            truncated: false,
            jobs: Vec::new(),
            confirmed_frame: ConfirmedFrameObservation::default(),
        };
        // No task is detached and no filesystem timeout is claimed. The reads
        // are synchronous, count/byte bounded and only follow a successful join.
        if cleanup_joined {
            match observation_scope(&self.broker_root) {
                Ok(scope) => {
                    output.namespace_known = true;
                    for (slot, job) in self.jobs.iter().take(OBSERVATION_JOBS).enumerate() {
                        let mut metadata = MetadataObservation {
                            slot,
                            selected: job.job == self.selected,
                            ipc_head_known: job.head.is_some(),
                            ipc_head: job.head.as_ref().map_or(0, |head| head.0),
                            ..MetadataObservation::default()
                        };
                        let job_root = scope.join("jobs").join(job.job.to_string());
                        if observe_metadata(&job_root, &mut metadata).is_err() {
                            output.errors += 1;
                        }
                        // Exactly one revision payload, named by the last IPC
                        // reply frozen at timeout. A newer revision file cannot
                        // substitute for it, including after broker shutdown.
                        if job.job == self.selected {
                            match job.head.as_ref() {
                                Some((revision, hash)) if metadata.ipc_frame_size_known => {
                                    if metadata.ipc_frame_bytes > OBSERVATION_PAYLOAD_BYTES {
                                        output.payload_bound_hit = true;
                                        output.errors += 1;
                                        output.jobs.push(metadata);
                                        continue;
                                    }
                                    output.payload_within_bound_known = true;
                                    output.payload_attempted = true;
                                    match observe_confirmed_frame(
                                        &job_root,
                                        job.job,
                                        *revision,
                                        hash,
                                        self.clock.as_ref(),
                                    ) {
                                        Ok(frame) => output.confirmed_frame = frame,
                                        Err(_) => output.errors += 1,
                                    }
                                }
                                _ => output.errors += 1,
                            }
                        }
                        output.jobs.push(metadata);
                    }
                }
                Err(_) => output.errors += 1,
            }
        }
        const PREFIX: &str = "full3_timeout_ticket ";
        match serde_json::to_string(&output) {
            Ok(line) if PREFIX.len() + line.len() + 1 <= OBSERVATION_OUTPUT_BYTES => {
                eprintln!("{PREFIX}{line}");
            }
            // A complete closed-label record replaces, rather than slices, JSON.
            _ => eprintln!(
                "full3_timeout_ticket {{\"version\":1,\"truncated\":true,\"coverage_known\":false,\"performance_qualified\":false}}"
            ),
        }
    }
}
fn observation_scope(broker_root: &Path) -> Result<PathBuf> {
    let namespace = broker_root.join("journals/a/broker-journal-v1");
    ensure!(
        std::fs::symlink_metadata(&namespace)?.file_type().is_dir(),
        "unknown namespace"
    );
    let mut entries = std::fs::read_dir(namespace)?;
    let scope = entries.next().context("unknown journal scope")??.path();
    ensure!(entries.next().is_none(), "ambiguous journal scope");
    ensure!(
        std::fs::symlink_metadata(&scope)?.file_type().is_dir(),
        "unknown journal scope"
    );
    Ok(scope)
}
fn observe_metadata(root: &Path, output: &mut MetadataObservation) -> Result<()> {
    ensure!(
        std::fs::symlink_metadata(root)?.file_type().is_dir(),
        "unknown job directory"
    );
    let entries = std::fs::read_dir(root)?;
    for entry in entries {
        let entry = entry?;
        if output.entries == OBSERVATION_METADATA {
            output.bound_hit = true;
            return Ok(());
        }
        let name = entry.file_name();
        let name = name.to_str().context("unknown entry name")?;
        let revision = if name.starts_with("rev-") {
            let number = name
                .strip_prefix("rev-")
                .and_then(|name| name.strip_suffix(".json"))
                .context("unknown revision name")?;
            ensure!(
                number.len() == 20 && number.bytes().all(|byte| byte.is_ascii_digit()),
                "unknown revision name"
            );
            Some(number.parse::<u64>()?)
        } else {
            None
        };
        if revision.is_some() && output.frames == OBSERVATION_FRAMES {
            output.bound_hit = true;
            return Ok(());
        }
        let metadata = std::fs::symlink_metadata(entry.path())?;
        output.entries += 1;
        ensure!(!metadata.file_type().is_symlink(), "unknown symlink entry");
        if let Some(revision) = revision {
            ensure!(metadata.file_type().is_file(), "unknown revision file");
            let bytes = metadata.len();
            output.smallest_frame_bytes = if output.frames == 0 {
                bytes
            } else {
                output.smallest_frame_bytes.min(bytes)
            };
            output.largest_frame_bytes = output.largest_frame_bytes.max(bytes);
            output.frame_bytes = output
                .frame_bytes
                .checked_add(bytes)
                .context("unknown frame volume")?;
            output.highest_revision_file = output.highest_revision_file.max(revision);
            if output.ipc_head_known && revision == output.ipc_head {
                output.ipc_frame_size_known = true;
                output.ipc_frame_bytes = bytes;
            }
            output.frames += 1;
        }
        // No directory traversal and no binding/credential file is opened.
    }
    output.complete = true;
    Ok(())
}
fn observe_confirmed_frame(
    root: &Path,
    job: Uuid,
    revision: u64,
    hash: &str,
    clock: Option<&HostClockSampleV1>,
) -> Result<ConfirmedFrameObservation> {
    let started = std::time::Instant::now();
    let bytes = read_bytes(
        &root.join(format!("rev-{revision:020}.json")),
        OBSERVATION_PAYLOAD_BYTES,
    )?;
    let read_us = observation_micros(started);
    let started = std::time::Instant::now();
    let value = confirmed_checkpoint_value(&bytes, Some(hash))?;
    let hash_decode_us = observation_micros(started);
    ensure!(
        value["operation_id"].as_str() == Some(job.to_string().as_str())
            && value["revision"].as_u64() == Some(revision),
        "unknown confirmed frame scope"
    );
    let mut output = ConfirmedFrameObservation {
        known: true,
        revision,
        bytes: bytes.len(),
        version: value["version"].as_u64().context("unknown frame version")?,
        post_cleanup_read_us: read_us,
        post_cleanup_hash_decode_us: hash_decode_us,
        ..ConfirmedFrameObservation::default()
    };
    let lifecycle = match output.version {
        3 => Some(&value["payload"]),
        4 => Some(&value["payload"]["lifecycle"]),
        5 => Some(&value["payload"]["outbox"]["lifecycle"]),
        _ => None,
    };
    if let Some(lifecycle) = lifecycle {
        project_local_failures(lifecycle, &mut output)?;
    }
    let supervision = match output.version {
        1 => return Ok(output),
        2 => &value["payload"]["supervision"],
        3 => &value["payload"]["broker"]["supervision"],
        4 => &value["payload"]["lifecycle"]["broker"]["supervision"],
        5 => &value["payload"]["outbox"]["lifecycle"]["broker"]["supervision"],
        _ => bail!("unknown frame version"),
    };
    // Validate the exact selected supervision record, without recursively
    // collecting unrelated nested copies or opening any private binding.
    let _: agentsandbox::broker::checkpoint::SupervisionCheckpointV1 =
        serde_json::from_value(supervision.clone())?;
    let helpers = supervision["helpers"]
        .as_array()
        .context("unknown helpers")?;
    let operations = supervision["operations"]
        .as_array()
        .context("unknown operations")?;
    let mut imports = Vec::new();
    for helper in helpers {
        match helper["policy"]["purpose"].as_str() {
            Some("import") => {
                imports.push(
                    helper["policy"]["helper_id"]
                        .as_str()
                        .context("unknown helper")?,
                );
                output.import_helpers += 1;
                output.import_owned += usize::from(!helper["owned"].is_null());
            }
            Some("import_verification") => {
                output.verification_helpers += 1;
                output.verification_owned += usize::from(!helper["owned"].is_null());
            }
            _ => {}
        }
    }
    for operation in operations {
        let request = &operation["request"];
        let acknowledged = !operation["response"].is_null();
        let kind = operation["kind"].as_str();
        if kind == Some("arm") {
            let arm: ArmV1 = serde_json::from_value(request.clone())?;
            if acknowledged {
                match request["subject"]["kind"].as_str() {
                    Some("worker") => output.worker_deadline_at_capture.observe(&arm, clock),
                    Some("helper") => output.helper_deadline_at_capture.observe(&arm, clock),
                    _ => {}
                }
            }
            if request["subject"]["purpose"] == "import" {
                output.import_arms.requests += 1;
                output.import_arms.acknowledged += usize::from(acknowledged);
            }
        } else if request["helper_id"]
            .as_str()
            .is_some_and(|id| imports.contains(&id))
        {
            if let Some(kind) = kind {
                output.import_outcomes.observe(kind, request, acknowledged);
            }
            let pair = match kind {
                Some("helper_claim") => Some(&mut output.import_claims),
                Some("helper_observation") => Some(&mut output.import_observations),
                Some("helper_cleanup") => Some(&mut output.import_cleanup),
                _ => None,
            };
            if let Some(pair) = pair {
                pair.requests += 1;
                pair.acknowledged += usize::from(acknowledged);
            }
        }
    }
    output.supervision_known = true;
    Ok(output)
}

fn project_local_failures(lifecycle: &Value, output: &mut ConfirmedFrameObservation) -> Result<()> {
    use agentsandbox::broker::lifecycle::{LocalCommandRequestV1, LocalCommandV1};
    let commands = lifecycle["commands"]
        .as_array()
        .context("unknown local commands")?;
    // Match the product's command-count bound before allocating typed records.
    ensure!(commands.len() <= 64, "local commands exceed their bound");
    let commands: Vec<LocalCommandV1> = serde_json::from_value(Value::Array(commands.clone()))?;
    for command in commands {
        if let LocalCommandRequestV1::FailureObserved { phase, code } = command.request() {
            output.local_failure_count += 1;
            output.last_failure_phase = Some(*phase);
            output.last_failure_code = Some(*code);
        }
    }
    output.local_failures_known = true;
    Ok(())
}

fn private_dir(parent: &Path, name: &str) -> Result<PathBuf> {
    let path = parent.join(name);
    std::fs::create_dir(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    Ok(path)
}
fn private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn fresh(root: &Path, tenant: &str) -> Result<Arc<Store>> {
    let store = Store::create_supervised_for_tenant(
        &root.join(format!("{tenant}.sqlite")),
        tenant,
        Redactor::new(vec![
            A_RUNNER.into(),
            B_RUNNER.into(),
            A_VIEWER.into(),
            B_VIEWER.into(),
        ]),
        10000,
        &root.join(format!("{tenant}-before3.sqlite")),
    )?;
    store.migrate_identity_v4(&root.join(format!("{tenant}-before4.sqlite")))?;
    store.migrate_human_access_v5(&root.join(format!("{tenant}-before5.sqlite")))?;
    store.migrate_workload_v6(&root.join(format!("{tenant}-before6.sqlite")))?;
    Ok(Arc::new(store))
}

const ARM_SECONDS: u32 = 90;
// Harness observation only: allow ongoing IPC/durable progress under load.
// Product Arm deadlines, polling and every acceptance assertion stay unchanged.
const FULL3_ENDPOINT_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    FullA,
    FullB,
    Legacy,
}
impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::FullA => "full-a",
            Self::FullB => "full-b",
            Self::Legacy => "enforcer",
        }
    }
}

/// Own only real subprocesses, not simulated guardian state.
struct ObservedChild {
    child: Child,
}
impl ObservedChild {
    fn spawn(command: &mut Command, diagnostic_path: PathBuf) -> Result<Self> {
        Self::spawn_with_custody(command, diagnostic_path, true)
    }

    fn spawn_with_custody(
        command: &mut Command,
        diagnostic_path: PathBuf,
        kill_on_drop: bool,
    ) -> Result<Self> {
        // A file, not a pipe: no reader task is needed for custody or cleanup.
        let stderr = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&diagnostic_path)?;
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .kill_on_drop(kill_on_drop)
            .spawn()?;
        Ok(Self { child })
    }

    async fn stop(&mut self) -> Result<()> {
        let signal = if self.child.try_wait()?.is_none() {
            self.child.start_kill()
        } else {
            Ok(())
        };
        let waited = timeout(Duration::from_secs(5), self.child.wait()).await;
        // Reap the owned Child even if the signal was refused.
        signal?;
        waited??;
        Ok(())
    }
}

struct RetainedFixtureEndpoint {
    role: Directory,
    announcements: Directory,
    publication: PrivateFile,
    endpoints: Directory,
    endpoint: EndpointRecordV1,
    socket: PathBuf,
}
impl RetainedFixtureEndpoint {
    fn capture(role: Directory) -> Result<Self> {
        let announcements = Directory::open_existing(&role.path().join("announcements"))?;
        ensure!(
            std::fs::read_dir(announcements.path())?.count() == 1,
            "expected one fixture endpoint publication"
        );
        let publication = announcements.read_private_file("e-0001.json", 4096)?;
        let record: Value = serde_json::from_slice(publication.bytes())?;
        ensure!(
            record["version"] == 1 && record["sequence"] == 1,
            "invalid fixture endpoint publication"
        );
        let endpoint: EndpointRecordV1 = serde_json::from_value(record["endpoint"].clone())?;
        let endpoints = Directory::open_existing(&role.path().join("endpoints"))?;
        let socket = endpoints.path().join(endpoint.directory_name()).join("s");
        Ok(Self {
            role,
            announcements,
            publication,
            endpoints,
            endpoint,
            socket,
        })
    }

    fn revalidate(&self) -> Result<()> {
        self.role.revalidate()?;
        self.announcements.revalidate()?;
        self.publication.revalidate()?;
        self.endpoints.revalidate()?;
        ensure!(
            std::fs::read_dir(self.announcements.path())?.count() == 1,
            "fixture endpoint publication was replaced"
        );
        Ok(())
    }

    async fn checked_peer(&self) -> Result<UnixStream> {
        self.revalidate()?;
        let stream = self.endpoint.connect_checked(&self.endpoints).await?;
        self.revalidate()?;
        Ok(stream)
    }

    async fn assert_closed(&self) -> Result<()> {
        self.revalidate()?;
        assert_ipc_closed(&self.socket).await
    }
}

struct FreshGuardian {
    endpoint: RetainedFixtureEndpoint,
    identity: DarwinIdentity,
}
impl FreshGuardian {
    async fn capture(broker_root: &Path, installation: Uuid, broker_pid: u32) -> Result<Self> {
        let root = Directory::open_existing(broker_root)?;
        let role_path = root.path().join(Role::FullA.name());
        let role = Directory::open_existing(&role_path)?;
        ensure!(role.path() == role_path, "unexpected Full3 role location");
        let genesis = read_json(&role.path().join("retirement/genesis.json"), 4096)?;
        ensure!(
            genesis["installation_id"] == installation.to_string(),
            "foreign Full3 installation"
        );
        let endpoint = RetainedFixtureEndpoint::capture(role)?;
        let peer = endpoint.checked_peer().await?;
        let identity = DarwinIdentity::capture(&peer, broker_pid, Path::new(BIN))?;
        root.revalidate()?;
        endpoint.revalidate()?;
        Ok(Self { endpoint, identity })
    }

    async fn close(self, broker_pid: u32) -> Result<()> {
        let peer = self.endpoint.checked_peer().await?;
        if let Err(error) = self.identity.close(&peer, broker_pid, Path::new(BIN)).await {
            // The platform module emits closed category strings and numeric
            // native return codes. Filesystem errors never reach this log.
            println!("phase=fixture_guardian_native {error}");
            return Err(error);
        }
        self.endpoint.assert_closed().await
    }
}

async fn assert_ipc_closed(socket: &Path) -> Result<()> {
    let result = timeout(Duration::from_secs(2), UnixStream::connect(socket)).await?;
    ensure!(
        matches!(result,
            Err(error) if matches!(
                error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            )
        ),
        "fixture IPC closure not confirmed"
    );
    Ok(())
}

pub struct ImportGate {
    reached: oneshot::Receiver<()>,
    resume: oneshot::Sender<()>,
}
impl ImportGate {
    pub async fn wait(&mut self) -> Result<()> {
        // Harness bound for a saturated host; the product Arm deadline is unchanged.
        timeout(Duration::from_secs(300), &mut self.reached)
            .await
            .context("phase=waiting_for_import category=fixture_timeout")?
            .context("phase=waiting_for_import category=gate_disconnected")?;
        Ok(())
    }

    pub fn release(self) -> Result<()> {
        self.resume
            .send(())
            .map_err(|_| anyhow::anyhow!("phase=held_import category=gate_already_released"))
    }
}

pub struct Fixture {
    pub root: PathBuf,
    pub a: Arc<Store>,
    pub b: Arc<Store>,
    pub origin: String,
    pub faults: Arc<Faults>,
    pub broker_root: PathBuf,
    pub operations: Vec<PathBuf>,
    broker_tmp: Directory,
    registry: PathBuf,
    runner_a: PathBuf,
    installation: Uuid,
    selection: Role,
    broker: Option<ObservedChild>,
    fresh_guardian: Option<FreshGuardian>,
    fresh_start_attempted: bool,
    enforcers: Vec<(Role, ObservedChild)>,
    engine: Arc<Mutex<Engine>>,
    engine_stop: Option<oneshot::Sender<()>>,
    engine_task: JoinHandle<()>,
    server_stop: Option<oneshot::Sender<()>>,
    server_task: JoinHandle<io::Result<()>>,
    broker_starts: u32,
    observation: StdMutex<ObservationState>,
}
impl Fixture {
    /// Pre-started Full3 (and optional Legacy) guardians, then an Existing broker.
    pub async fn new(selection: Role) -> Result<Self> {
        ensure!(
            selection != Role::FullB,
            "foreign role is not an initial history"
        );
        Self::start(selection, false).await
    }

    /// The production `--enforcer-full3-new` selection: the broker creates its
    /// sole Full3 guardian, whose custody this fixture attests before submit.
    pub async fn new_fresh() -> Result<Self> {
        Self::start(Role::FullA, true).await
    }

    async fn start(selection: Role, fresh_role: bool) -> Result<Self> {
        let base = std::fs::canonicalize(std::env::temp_dir())?;
        let root = tempfile::Builder::new()
            .prefix("f3r-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(base)?
            .keep();
        let a = fresh(&root, "tenant-a")?;
        let b = fresh(&root, "tenant-b")?;
        let faults = Arc::new(Faults {
            lose_claim: AtomicBool::new(false),
            lost: AtomicUsize::new(0),
            claims: AtomicUsize::new(0),
            sends: AtomicUsize::new(0),
            acks: AtomicUsize::new(0),
            trace: diagnostics::Trace::new(),
        });
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let app = gateway_router(GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: a.clone(),
                    runner_token: A_RUNNER.into(),
                    viewer_token: A_VIEWER.into(),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: b.clone(),
                    runner_token: B_RUNNER.into(),
                    viewer_token: B_VIEWER.into(),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )?)
        .layer(middleware::from_fn_with_state(faults.clone(), fault));
        let engine = Arc::new(Mutex::new(Engine {
            next_id: 1,
            containers: BTreeMap::new(),
            volumes: BTreeMap::new(),
            calls: vec![],
            execs: BTreeMap::new(),
            errors: BTreeMap::new(),
            import_gate: None,
            import_counts: Default::default(),
            root: root.clone(),
        }));
        let socket = root.join("r");
        let engine_listener = UnixListener::bind(&socket)?;
        let witness_source = root.join("runtime.rs");
        private_file(
            &witness_source,
            include_bytes!("../runtime_fixtures/relay_stdio.rs"),
        )?;
        let program = root.join("runtime");
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let mut command = Command::new(rustc);
        command
            .args(["--edition=2024", "-o"])
            .arg(&program)
            .arg(&witness_source)
            .env("TMPDIR", &root);
        let mut compiler = ObservedChild::spawn(&mut command, root.join("runtime-compile.stderr"))?;
        let compiled = timeout(Duration::from_secs(30), compiler.child.wait()).await;
        let cleanup = compiler.stop().await;
        cleanup?;
        ensure!(
            compiled??.success(),
            "compile strict std-only fixture witness failed"
        );
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o500))?;
        private_file(
            &program.with_extension("fixture"),
            socket.as_os_str().as_encoded_bytes(),
        )?;
        let operator = private_dir(&root, "operator")?;
        let runner_a = operator.join("a.runner");
        let runner_b = operator.join("b.runner");
        private_file(&runner_a, A_RUNNER.as_bytes())?;
        private_file(&runner_b, B_RUNNER.as_bytes())?;
        let sources = private_dir(&root, "sources")?;
        let exports = private_dir(&root, "exports")?;
        let broker_tmp = Directory::open_existing(&private_dir(&root, "broker-tmp")?)?;
        let configs = private_dir(&root, "configs")?;
        let ops = private_dir(&root, "ops")?;
        let operations = vec![ops.join("a1"), ops.join("a2")];
        let broker_root = private_dir(&root, "b")?;
        let installation = Uuid::new_v4();
        let mut profiles = Vec::new();
        for (id, tenant, store, token) in [
            ("a", "tenant-a", &a, &runner_a),
            ("b", "tenant-b", &b, &runner_b),
        ] {
            let config = private_dir(&configs, id)?;
            initialize_config(&config)?;
            let mut profile = json!({"id":id,"gateway":origin,"tenant_id":tenant,
                "control_namespace_uuid":store.supervision_state()?.control_namespace_uuid,
                "config_dir":config,"runner_token_file":token,"runtime":"docker","runtime_program":program,
                "runtime_endpoint":"unix:///software-relay-fixture.sock","image":IMAGE,"policy_profile":"strict",
                "network":"none","allow_rootful_runtime":false,"max_timeout_seconds":ARM_SECONDS,
                "allow_capture_output":false,"managed_clients":false,"providers_file":null,"manual_credential_file":null,
                "source_roots":[sources],"export_roots":[exports],"prearmed_start_profile":{"version":1,"mode":"docker_minimal"}});
            if id == "a" {
                profile["relay"] = json!({"version":1,"workload_operations":operations});
            }
            profiles.push(profile);
        }
        let registry = operator.join("broker.json");
        private_file(
            &registry,
            &serde_json::to_vec(&json!({"version":2,"installation_id":installation,
            "root":broker_root,"max_enforcer_slots":32,"profiles":profiles,"additional_control_secret_files":[]}))?,
        )?;
        // Start asynchronous owners only after every fallible local preparation.
        let (server_stop, stopped) = oneshot::channel();
        let server_task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        });
        let (engine_stop, mut stopped) = oneshot::channel();
        let shared = engine.clone();
        let runtime_trace = Arc::clone(&faults.trace);
        let engine_task = tokio::spawn(async move {
            let mut owners = JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = &mut stopped => break,
                    result = owners.join_next(), if !owners.is_empty() => {
                        if let Some(Err(_)) = result {
                            shared.lock().await.note_failure(EngineFailure::OwnerJoin);
                        }
                    }
                    accepted = engine_listener.accept(), if owners.len() < 64 => {
                        match accepted {
                            Ok((stream,_)) => {
                                let state = shared.clone();
                                let trace = Arc::clone(&runtime_trace);
                                owners.spawn(async move {
                                    if let Err(error) = engine_connection_observed(stream, state.clone(), Some(trace)).await {
                                        let category = error.downcast_ref::<EngineFailure>()
                                            .copied()
                                            .unwrap_or(EngineFailure::RuntimeCommand);
                                        state.lock().await.note_failure(category);
                                    }
                                });
                            }
                            Err(_) => {
                                shared.lock().await.note_failure(EngineFailure::ListenerAccept);
                                break;
                            }
                        }
                    }
                }
            }
            owners.abort_all();
            while owners.join_next().await.is_some() {}
        });
        let mut fixture = Self {
            root,
            a,
            b,
            origin,
            faults,
            broker_root,
            operations,
            broker_tmp,
            registry,
            runner_a,
            installation,
            selection,
            broker: None,
            fresh_guardian: None,
            fresh_start_attempted: false,
            enforcers: Vec::new(),
            engine,
            engine_stop: Some(engine_stop),
            engine_task,
            server_stop: Some(server_stop),
            server_task,
            broker_starts: 0,
            observation: StdMutex::new(ObservationState::default()),
        };
        let started = if fresh_role {
            fixture.start_broker_mode(selection, true).await
        } else {
            async {
                // This real Full3 role exists even for the missing-binding history.
                fixture.start_guardian(Role::FullA).await?;
                if selection == Role::Legacy {
                    fixture.start_guardian(Role::Legacy).await?;
                }
                fixture.start_broker(selection).await
            }
            .await
        };
        if let Err(error) = started {
            let cleanup = fixture.close().await;
            return Err(error.context(if cleanup.is_ok() {
                "Full3 fixture precondition failed after joined cleanup"
            } else {
                "Full3 fixture precondition failed; cleanup also failed"
            }));
        }
        Ok(fixture)
    }

    async fn start_guardian(&mut self, role: Role) -> Result<()> {
        ensure!(
            !self.enforcers.iter().any(|(known, _)| *known == role),
            "guardian already owned"
        );
        let mut command = Command::new(BIN);
        command
            .arg("__broker-enforcer")
            .arg("--max-slots")
            .arg("32")
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", &self.root)
            .env("TMPDIR", self.broker_tmp.path())
            .current_dir(&self.root);
        if role == Role::Legacy {
            let root = private_dir(&self.broker_root, role.name())?;
            private_file(&root.join("auth"), &[0x51; 32])?;
            command.arg("--root").arg(root);
        } else {
            command
                .arg("--root")
                .arg(&self.broker_root)
                .arg("--full3-fresh")
                .arg("--role-name")
                .arg(role.name())
                .arg("--installation-id")
                .arg(self.installation.to_string());
        }
        let process = ObservedChild::spawn(
            &mut command,
            self.root.join(format!("{}.stderr", role.name())),
        )?;
        self.enforcers.push((role, process));
        timeout(Duration::from_secs(10), async {
            loop {
                let process = &mut self.enforcers.last_mut().context("guardian owner")?.1;
                ensure!(
                    process.child.try_wait()?.is_none(),
                    "real guardian exited before announcement"
                );
                if self
                    .broker_root
                    .join(role.name())
                    .join("announcements/e-0001.json")
                    .is_file()
                {
                    if role != Role::Legacy {
                        let genesis = read_json(
                            &self
                                .broker_root
                                .join(role.name())
                                .join("retirement/genesis.json"),
                            4096,
                        )?;
                        ensure!(
                            genesis["installation_id"] == self.installation.to_string(),
                            "real genesis installation"
                        );
                    }
                    return Ok::<_, anyhow::Error>(());
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await?
    }
    async fn start_broker(&mut self, role: Role) -> Result<()> {
        self.start_broker_mode(role, false).await
    }

    fn live_broker_pid(&mut self) -> Result<u32> {
        let broker = &mut self
            .broker
            .as_mut()
            .context("missing fixture broker owner")?
            .child;
        ensure!(
            broker.try_wait()?.is_none(),
            "fixture broker is no longer live"
        );
        broker.id().context("missing live fixture broker identity")
    }

    async fn bind_fresh_guardian(&mut self) -> Result<()> {
        let broker_pid = self.live_broker_pid()?;
        let guardian =
            FreshGuardian::capture(&self.broker_root, self.installation, broker_pid).await?;
        ensure!(
            self.live_broker_pid()? == broker_pid,
            "fixture broker owner changed"
        );
        self.fresh_guardian = Some(guardian);
        Ok(())
    }

    async fn start_broker_mode(&mut self, role: Role, fresh: bool) -> Result<()> {
        ensure!(self.broker.is_none(), "broker already owned");
        self.broker_tmp.revalidate()?;
        self.broker_starts += 1;
        let mut command = Command::new(BIN);
        command
            .args(["broker", "serve", "--config"])
            .arg(&self.registry)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", &self.root)
            .env("TMPDIR", self.broker_tmp.path())
            .current_dir(&self.root);
        if role != Role::Legacy {
            command
                .arg(if fresh {
                    "--enforcer-full3-new"
                } else {
                    "--enforcer-full3-existing"
                })
                .arg(role.name());
        }
        if fresh {
            ensure!(role == Role::FullA, "unexpected Fresh role");
            ensure!(
                matches!(std::fs::symlink_metadata(self.broker_root.join(role.name())), Err(e) if e.kind() == io::ErrorKind::NotFound),
                "Fresh role path must be absent"
            );
        }
        self.broker = Some(ObservedChild::spawn_with_custody(
            &mut command,
            self.root
                .join(format!("broker-{}.stderr", self.broker_starts)),
            !fresh,
        )?);
        self.fresh_start_attempted = fresh;
        self.selection = role;
        timeout(Duration::from_secs(60), async {
            loop {
                ensure!(
                    self.broker
                        .as_mut()
                        .context("broker owner")?
                        .child
                        .try_wait()?
                        .is_none(),
                    "real broker exited before IPC preflight"
                );
                if fresh && self.fresh_guardian.is_none() {
                    if self.bind_fresh_guardian().await.is_err() {
                        sleep(Duration::from_millis(25)).await;
                        continue;
                    }
                    println!("phase=fixture_start_guardian category=identity_attested");
                }
                if matches!(
                    service::request(
                        &self.broker_root,
                        "a",
                        A_RUNNER,
                        BrokerCommandV1::List {
                            after: None,
                            limit: 1
                        }
                    )
                    .await,
                    Ok(BrokerReplyV1::Page { .. })
                ) {
                    return Ok::<_, anyhow::Error>(());
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await?
    }
    pub async fn restart(&mut self, role: Role) -> Result<(Uuid, Uuid)> {
        let before = self.broker_epoch()?;
        if role == Role::FullB && !self.enforcers.iter().any(|(known, _)| *known == role) {
            self.start_guardian(role).await?;
        }
        if let Some(mut broker) = self.broker.take() {
            broker.stop().await?;
        }
        self.start_broker(role).await?;
        let after = self.broker_epoch()?;
        ensure!(
            before != after,
            "restart must select a new broker process announcement"
        );
        Ok((before, after))
    }
    fn broker_epoch(&self) -> Result<Uuid> {
        announcement_epoch(&self.broker_root.join("broker"))
    }
    pub async fn job_status(&self, job: Uuid) -> Result<JobStatusV1> {
        use diagnostics::{Outcome, Phase, Purpose};
        let diagnostic = self
            .faults
            .trace
            .begin(Phase::IpcJobStatus, Purpose::Unknown);
        let reply = service::request(
            &self.broker_root,
            "a",
            A_RUNNER,
            BrokerCommandV1::Status { operation_id: job },
        )
        .await;
        diagnostic.finish(match &reply {
            Ok(BrokerReplyV1::Status { .. }) => Outcome::Ok,
            Ok(_) => Outcome::NonSuccess,
            Err(_) => Outcome::Error,
        });
        match reply? {
            BrokerReplyV1::Status { status } => {
                self.faults.trace.observe_job(&status);
                if status.operation_id == job
                    && let Ok(mut observation) = self.observation.lock()
                    && let Some(entry) = observation.jobs.iter_mut().find(|entry| entry.job == job)
                    && let Some((revision, hash)) =
                        status.head_revision.zip(status.head_hash.as_ref())
                    && hash.len() == 64
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    entry.head = Some((revision, hash.clone()));
                }
                Ok(status)
            }
            _ => bail!("real job status refused"),
        }
    }
    pub async fn wait_endpoint(&self, job: Uuid) -> Result<RelayEndpointV1> {
        let result = timeout(FULL3_ENDPOINT_OBSERVATION_TIMEOUT, async {
            loop {
                let status = self.status("a", job).await?;
                ensure!(
                    status.version == 1
                        && status.operation_id == job
                        && status.tenant_id == "tenant-a"
                        && status.configured
                        && status.source == RelayStatusSourceV1::BrokerRetainedRegistry
                        && !status.reconciliation_required,
                    "relay status identity/configuration changed or requires reconciliation"
                );
                if let Some(endpoint) = &status.endpoint {
                    let worker = self
                        .a
                        .worker(&endpoint.worker_id)?
                        .context("reported endpoint worker")?;
                    ensure!(
                        worker.admission_operation_id == job.to_string()
                            && worker.attempt.attempt_id == endpoint.attempt_id
                            && endpoint.generation == 1,
                        "reported endpoint differs from the real job/worker attempt"
                    );
                }
                if status.phase == Some(RelayAttachmentPhaseV1::AwaitingIssuedWorkload) {
                    ensure!(
                        status.last_error.is_none()
                            && status.freshness
                                == RelayStatusFreshnessV1::RetainedAtLastObservation,
                        "Awaiting must come from the new broker registry"
                    );
                    return status.endpoint.context("Awaiting endpoint");
                }
                // List/Page can be ready before this new registry has a fact.
                // Do not turn a refused IPC reply or an Unknown/Closed phase
                // into a transient; status() already propagates every refusal.
                ensure!(
                    status.phase.is_none()
                        && status.freshness == RelayStatusFreshnessV1::NotObserved
                        && matches!(
                            status.last_error,
                            None | Some(RelayStatusErrorV1::Unavailable)
                        ),
                    "relay prerequisite is neither Awaiting nor an unobserved transient"
                );
                let job_status = self.job_status(job).await?;
                ensure!(
                    (job_status.failure_count == 0 && job_status.executor_error.is_none())
                        || job_status.worker_state == Some(WorkerStateV1::Running),
                    "Running prerequisite failed in real executor; no history is fabricated"
                );
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                // Memory-only snapshot, before cleanup or another IPC response.
                // Clock failure remains unknown; no diagnostic can extend Arm.
                let clock = NativeHostClock::new().and_then(|clock| clock.sample()).ok();
                if let Ok(mut observation) = self.observation.lock()
                    && observation.timeout.is_none()
                {
                    observation.timeout = Some(TimeoutObservationTicket {
                        broker_root: self.broker_root.clone(),
                        selected: job,
                        jobs: observation.jobs.clone(),
                        jobs_limited: observation.jobs_limited,
                        clock,
                    });
                }
                Err(error.into())
            }
        };
        if result.is_err() {
            self.faults.trace.emit_failure_once();
        }
        result
    }
    pub async fn resume_reply(&self, job: Uuid) -> Result<BrokerReplyV1> {
        service::request(
            &self.broker_root,
            "a",
            A_RUNNER,
            BrokerCommandV1::Resume {
                operation_id: job,
                command_id: Uuid::new_v4(),
            },
        )
        .await
    }
    pub fn runner_file(&self) -> &Path {
        &self.runner_a
    }
    pub async fn hold_import(&self) -> Result<ImportGate> {
        let (entered, reached) = oneshot::channel();
        let (resume, release) = oneshot::channel();
        let mut engine = self.engine.lock().await;
        ensure!(
            engine.import_gate.is_none() && engine.import_counts.imports_created == 0,
            "phase=install_import_gate category=fixture_state"
        );
        engine.import_gate = Some((entered, release));
        Ok(ImportGate { reached, resume })
    }

    pub async fn import_counts(&self) -> ImportCounts {
        self.engine.lock().await.import_counts
    }

    pub async fn submit(&self, profile: &str, seconds: u32) -> Result<Uuid> {
        use diagnostics::{Outcome, Phase, Purpose};
        let operation_id = Uuid::new_v4();
        let submit = SubmitV1 {
            version: 1,
            operation_id,
            project: self.root.as_path().join("sources"),
            execution: RequestedExecutionV1::Command {
                argv: vec!["/usr/bin/id".into()],
            },
            timeout_seconds: Some(seconds),
            capture_output: false,
        };
        let diagnostic = self.faults.trace.begin(Phase::IpcSubmit, Purpose::Unknown);
        let reply = service::request(
            &self.broker_root,
            profile,
            if profile == "a" { A_RUNNER } else { B_RUNNER },
            BrokerCommandV1::Submit {
                payload: encode_submit(&submit)?,
            },
        )
        .await;
        diagnostic.finish(match &reply {
            Ok(BrokerReplyV1::Accepted { .. }) => Outcome::Ok,
            Ok(_) => Outcome::NonSuccess,
            Err(_) => Outcome::Error,
        });
        let reply = reply?;
        ensure!(
            matches!(reply, BrokerReplyV1::Accepted { .. }),
            "real broker did not accept fixture submit"
        );
        if profile == "a"
            && let Ok(mut observation) = self.observation.lock()
        {
            if observation.jobs.len() < OBSERVATION_JOBS {
                observation.jobs.push(ObservedJob {
                    job: operation_id,
                    head: None,
                });
            } else {
                observation.jobs_limited = true;
            }
        }
        Ok(operation_id)
    }
    pub fn timeout_observation_ticket(&self) -> Option<TimeoutObservationTicket> {
        self.observation.lock().ok()?.timeout.clone()
    }
    pub async fn status(&self, profile: &str, job: Uuid) -> Result<RelayStatusV1> {
        use diagnostics::{Outcome, Phase, Purpose};
        let diagnostic = self
            .faults
            .trace
            .begin(Phase::IpcRelayStatus, Purpose::Unknown);
        let reply = service::request(
            &self.broker_root,
            profile,
            if profile == "a" { A_RUNNER } else { B_RUNNER },
            BrokerCommandV1::RelayStatus { operation_id: job },
        )
        .await;
        diagnostic.finish(match &reply {
            Ok(BrokerReplyV1::RelayStatus { .. }) => Outcome::Ok,
            Ok(_) => Outcome::NonSuccess,
            Err(_) => Outcome::Error,
        });
        match reply? {
            BrokerReplyV1::RelayStatus { status } => Ok(status),
            _ => bail!("relay status refused"),
        }
    }
    pub async fn issue(&self, index: usize, endpoint: &RelayEndpointV1) -> Result<String> {
        let manifest = self.root.as_path().join(format!("issue-{index}.json"));
        private_file(
            &manifest,
            &serde_json::to_vec(
                &json!({"action":"issue","version":1,"expected_revision":0,"subject":{"kind":"worker","worker_id":endpoint.worker_id,"attempt_id":endpoint.attempt_id,"generation":1},"expires_at":(chrono::Utc::now()+chrono::Duration::minutes(5)).to_rfc3339()}),
            )?,
        )?;
        let control = self.root.as_path().join(format!("control-{index}.json"));
        private_file(
            &control,
            &serde_json::to_vec(
                &json!({"version":1,"gateway":self.origin,"expected_tenant_id":"tenant-a","control_namespace_uuid":self.a.supervision_state()?.control_namespace_uuid,"runner_token_file":self.runner_a,"ca_bundle_file":null}),
            )?,
        )?;
        for (action, args) in [
            (
                "prepare",
                vec![
                    "--input".to_owned(),
                    manifest.display().to_string(),
                    "--control-file".into(),
                    control.display().to_string(),
                    "--output-dir".into(),
                    self.operations[index].display().to_string(),
                ],
            ),
            (
                "apply",
                vec![
                    "--operation-dir".into(),
                    self.operations[index].display().to_string(),
                    "--control-file".into(),
                    control.display().to_string(),
                ],
            ),
        ] {
            let mut command = Command::new(BIN);
            command
                .args(["workload", action])
                .args(args)
                .env_clear()
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("HOME", self.root.as_path())
                .env("TMPDIR", self.broker_tmp.path())
                .current_dir(self.root.as_path());
            let mut owned = ObservedChild::spawn(
                &mut command,
                self.root.join(format!("workload-{index}-{action}.stderr")),
            )?;
            let status = timeout(Duration::from_secs(15), owned.child.wait()).await;
            let cleanup = owned.stop().await;
            cleanup?;
            ensure!(status??.success(), "real workload {action} failed");
        }
        let recovery: Value = serde_json::from_slice(&std::fs::read(
            self.operations[index].join("recovery.json"),
        )?)?;
        Ok(recovery["request"]["credential_id"]
            .as_str()
            .context("issued credential ID")?
            .into())
    }
    pub async fn ready(&self, job: Uuid) -> Result<RelayStatusV1> {
        timeout(Duration::from_secs(15), async {
            loop {
                let status = self.status("a", job).await?;
                if status.core_state
                    == Some(agentsandbox::relay::admission::RelayChannelStateV1::Ready)
                {
                    return Ok(status);
                }
                ensure!(
                    status.phase != Some(RelayAttachmentPhaseV1::Closed),
                    "relay closed before Ready"
                );
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await?
    }
    pub async fn exec_count(&self) -> usize {
        self.engine.lock().await.execs.values().sum()
    }
    pub fn committed_claims(&self) -> Result<usize> {
        let db = self.readonly_a()?;
        let mut query = db.prepare("SELECT payload FROM events WHERE kind='relay_channel.v1'")?;
        let mut rows = query.query([])?;
        let mut count = 0;
        while let Some(row) = rows.next()? {
            let payload: String = row.get(0)?;
            let event: Value = serde_json::from_str(&payload)?;
            count += usize::from(event["data"]["input"]["action"] == "claim");
        }
        Ok(count)
    }
    fn readonly_a(&self) -> Result<rusqlite::Connection> {
        let db = rusqlite::Connection::open_with_flags(
            self.root.join("tenant-a.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        db.busy_timeout(Duration::from_secs(2))?;
        Ok(db)
    }
    pub fn relay_effects(&self) -> Result<(Vec<(String, bool)>, Vec<String>)> {
        let db = self.readonly_a()?;
        let mut query =
            db.prepare("SELECT message_id,acknowledged FROM relay_messages ORDER BY message_id")?;
        let messages = query
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut query =
            db.prepare("SELECT operation_id FROM relay_operations ORDER BY operation_id")?;
        let operations = query
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((messages, operations))
    }
    pub fn tenant_directory(&self) -> Result<PathBuf> {
        let mut entries = std::fs::read_dir(self.broker_root.join("broker/relay"))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<Vec<_>>>()?;
        ensure!(
            entries.len() == 1,
            "two endpoints must share exactly one tenant IO namespace"
        );
        Ok(entries.remove(0))
    }
    pub fn durable_operation(&self, operation: &str) -> Result<Value> {
        let directory = self
            .tenant_directory()?
            .join("relay_v1")
            .join(format!("op-{operation}"));
        for name in ["request.json", "dispatch.json", "receipt.json"] {
            ensure!(
                directory.join(name).is_file(),
                "durable operation is missing {name}"
            );
        }
        Ok(serde_json::from_slice(&std::fs::read(
            directory.join("request.json"),
        )?)?)
    }
    pub async fn closed(&self, job: Uuid) -> Result<()> {
        timeout(Duration::from_secs(15), async {
            loop {
                let status = self.status("a", job).await?;
                if status.core_state
                    == Some(agentsandbox::relay::admission::RelayChannelStateV1::Closed)
                {
                    ensure!(
                        status.local_joined == Some(true),
                        "Core Closed without a joined local owner"
                    );
                    return Ok::<_, anyhow::Error>(());
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await??;
        Ok(())
    }
    pub async fn helper_connection(
        &self,
        endpoint: &RelayEndpointV1,
    ) -> Result<(UnixStream, String, u64)> {
        let worker = self.a.worker(&endpoint.worker_id)?.context("worker")?;
        let cid = &worker
            .attempt
            .created_inspection
            .as_ref()
            .context("created inspection")?
            .container_id;
        let root = self
            .engine
            .lock()
            .await
            .containers
            .get(cid)
            .context("synthetic helper CID")?
            .helper_root
            .clone();
        timeout(Duration::from_secs(5), async {
            loop {
                for entry in std::fs::read_dir(root.path())? {
                    let entry = entry?;
                    if entry.file_type()?.is_dir() {
                        if let Ok(discovered) =
                            helper::discover(&root, &entry.file_name().to_string_lossy())
                        {
                            let stream = discovered.connect_checked(&root).await?;
                            return Ok::<_, anyhow::Error>((
                                stream,
                                discovered.channel_id().into(),
                                discovered.epoch(),
                            ));
                        }
                    }
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await?
    }
    pub fn latest_fact(&self, job: Uuid) -> Result<Value> {
        let relay = self.broker_root.join("broker/relay");
        for tenant in std::fs::read_dir(relay)? {
            let directory = tenant?.path().join("relay_channels_v1");
            let mut names = std::fs::read_dir(directory)?
                .map(|e| e.map(|e| e.path()))
                .collect::<std::io::Result<Vec<_>>>()?;
            names.sort();
            for path in names.into_iter().rev() {
                if path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("fact-"))
                {
                    let value: Value = serde_json::from_slice(&std::fs::read(path)?)?;
                    if value["fact"]["job_id"] == job.to_string() {
                        return Ok(value["fact"].clone());
                    }
                }
            }
        }
        bail!("no channel fact for job")
    }
    pub async fn cancel(&self, profile: &str, job: Uuid) -> Result<()> {
        let reply = service::request(
            &self.broker_root,
            profile,
            if profile == "a" { A_RUNNER } else { B_RUNNER },
            BrokerCommandV1::Cancel {
                operation_id: job,
                command_id: Uuid::new_v4(),
            },
        )
        .await?;
        ensure!(
            matches!(reply, BrokerReplyV1::CommandAcknowledged { .. }),
            "cancel not durably acknowledged"
        );
        Ok(())
    }

    pub async fn history(&self, job: Uuid, original_role: Role) -> Result<History> {
        let status = self.job_status(job).await?;
        let namespace = self.broker_root.join("journals/a/broker-journal-v1");
        let scopes = std::fs::read_dir(namespace)?.collect::<io::Result<Vec<_>>>()?;
        ensure!(scopes.len() == 1, "fixture requires one real journal scope");
        let job_root = scopes[0].path().join("jobs").join(job.to_string());
        // Read the immutable revision named by this confirmed IPC response.
        // A newer writer head can advance without invalidating this snapshot.
        let revision = status.head_revision.context("confirmed real revision")?;
        let bytes = read_bytes(
            &job_root.join(format!("rev-{revision:020}.json")),
            32 * 1024 * 1024,
        )?;
        let value = confirmed_checkpoint_value(&bytes, status.head_hash.as_deref())?;
        let mut arms = Vec::new();
        collect_operations(&value, "arm", &mut arms);
        let arms: Vec<_> = arms
            .into_iter()
            .filter(|operation| {
                operation["request"]["subject"]["kind"] == "worker"
                    && !operation["response"].is_null()
            })
            .collect();
        ensure!(arms.len() == 1, "exactly one real acknowledged Worker Arm");
        let operation = arms[0];
        let arm: ArmV1 = serde_json::from_value(operation["request"].clone())?;
        let mut claims = Vec::new();
        collect_operations(&value, "worker_claim", &mut claims);
        let claims: Vec<_> = claims
            .into_iter()
            .filter(|op| !op["response"].is_null())
            .collect();
        ensure!(claims.len() == 1, "one real acknowledged WorkerClaim");
        let mut assignments = Vec::new();
        collect_named_arrays(&value, "full3_registrations", &mut assignments);
        ensure!(assignments.len() <= 1, "unambiguous V5 assignment set");
        let matching: Vec<_> = assignments
            .into_iter()
            .flatten()
            .filter(|entry| entry["arm_id"] == arm.operation_id)
            .collect();
        ensure!(matching.len() <= 1, "one original assignment per Arm");
        let binding: Option<Value> = matching.first().map(|entry| (**entry).clone());
        match (original_role, binding.as_ref()) {
            (Role::Legacy, None) => {}
            (Role::Legacy, Some(_)) => {
                bail!("legacy prerequisite unexpectedly has a Full3 assignment")
            }
            (_, Some(binding)) => {
                let role_root = self.broker_root.join(original_role.name());
                let genesis_bytes = read_bytes(&role_root.join("retirement/genesis.json"), 4096)?;
                let genesis: Value = serde_json::from_slice(&genesis_bytes)?;
                for key in ["installation_id", "role_id", "init_id", "native_binding"] {
                    ensure!(
                        binding["ledger"][key] == genesis[key],
                        "assignment must name the real original genesis"
                    );
                }
                let mut hash = Sha256::new();
                hash.update(b"ai-guard:enforcer-retirement-genesis:v1\0");
                hash.update(&genesis_bytes);
                ensure!(
                    binding["ledger"]["genesis_hash"] == hex::encode(hash.finalize())
                        && binding["original_epoch"] == announcement_epoch(&role_root)?.to_string(),
                    "assignment must retain the original authenticated guardian epoch"
                );
            }
            (_, None) => bail!("Full3 prerequisite has no confirmed assignment"),
        }
        let endpoint = RelayEndpointV1 {
            worker_id: arm.worker_id.clone(),
            attempt_id: arm.attempt_id.clone(),
            generation: 1,
        };
        let worker = self.a.worker(&endpoint.worker_id)?.context("real worker")?;
        ensure!(
            worker.state == WorkerStateV1::Running && worker.quota_held,
            "history prerequisite requires a real Running worker with quota"
        );
        let cid = &arm.inspection.container_id;
        let starts = self
            .engine
            .lock()
            .await
            .calls
            .iter()
            .filter(|call| call.iter().any(|arg| arg == "start") && call.last() == Some(cid))
            .count();
        ensure!(
            starts == 1,
            "synthetic worker CID must have exactly one actual CLI start"
        );
        let history = History {
            arm,
            binding,
            endpoint,
            claim: (*claims[0]).clone(),
            owned: operation["owned"].clone(),
            starts,
        };
        history.require_live()?;
        Ok(history)
    }
    pub async fn assert_no_relay_effects(&self) -> Result<()> {
        ensure!(
            self.exec_count().await == 0 && self.committed_claims()? == 0,
            "no helper exec or durable Core relay claim before issue"
        );
        ensure!(
            self.faults.claims.load(Ordering::SeqCst) == 0
                && self.faults.sends.load(Ordering::SeqCst) == 0
                && self.faults.acks.load(Ordering::SeqCst) == 0,
            "no relay claim/send/ACK HTTP request before issue"
        );
        ensure!(
            self.operations.iter().all(|path| !path.exists()),
            "product must not issue workload implicitly"
        );
        let (messages, operations) = self.relay_effects()?;
        ensure!(
            messages.is_empty() && operations.is_empty(),
            "no Store relay effects"
        );
        let root = self.broker_root.join("broker/relay");
        if root.is_dir() {
            for tenant in std::fs::read_dir(root)? {
                let outbox = tenant?.path().join("relay_v1");
                if outbox.is_dir() {
                    for operation in std::fs::read_dir(outbox)? {
                        ensure!(
                            !operation?.file_name().to_string_lossy().starts_with("op-"),
                            "no durable relay operation before issue"
                        );
                    }
                }
            }
        }
        Ok(())
    }
    pub async fn assert_assignment_refused(&self, job: Uuid) -> Result<()> {
        ensure!(
            matches!(
                self.resume_reply(job).await?,
                BrokerReplyV1::Error {
                    code: BrokerErrorCodeV1::NeedsReconciliation
                }
            ),
            "foreign/missing assignment must explicitly refuse real Resume"
        );
        match service::request(
            &self.broker_root,
            "a",
            A_RUNNER,
            BrokerCommandV1::RelayStatus { operation_id: job },
        )
        .await?
        {
            BrokerReplyV1::RelayStatus { status } => ensure!(
                status.freshness == RelayStatusFreshnessV1::NotObserved
                    && status.endpoint.is_none()
                    && status.phase.is_none(),
                "failed restoration must not install a new relay owner"
            ),
            BrokerReplyV1::Error {
                code: BrokerErrorCodeV1::NeedsReconciliation,
            } => {}
            _ => bail!("unexpected scoped relay refusal"),
        }
        self.assert_no_relay_effects().await
    }
    pub async fn shutdown_jobs(&mut self, jobs: &[Uuid], role: Role) -> Result<()> {
        if self.selection != role {
            self.restart(role).await?;
        }
        for job in jobs {
            self.cancel("a", *job).await?;
        }
        Ok(())
    }
    pub async fn close(self) -> Result<()> {
        self.close_inner(None::<fn(&Path)>).await.0
    }

    pub async fn close_with_broker_stopped(
        self,
        check_journal: impl FnOnce(&Path),
    ) -> (Result<()>, usize) {
        self.close_inner(Some(check_journal)).await
    }

    async fn close_inner(
        mut self,
        check_journal: Option<impl FnOnce(&Path)>,
    ) -> (Result<()>, usize) {
        let mut failed = false;
        if let Some(stop) = self.engine_stop.take() {
            let _ = stop.send(());
        }
        let engine_joined = matches!(
            timeout(Duration::from_secs(5), &mut self.engine_task).await,
            Ok(Ok(()))
        );
        if !engine_joined {
            self.engine_task.abort();
            let _ = timeout(Duration::from_secs(5), &mut self.engine_task).await;
        }
        let engine_closed = engine_joined && assert_ipc_closed(&self.root.join("r")).await.is_ok();
        failed |= !engine_closed;
        println!(
            "phase=fixture_runtime_close category={}",
            if engine_closed {
                "joined_ipc_absent"
            } else {
                "not_confirmed"
            }
        );

        // The guardian may hold an admitted slot even when diagnostics fail
        // before the business ACKs. No process signal is authorized until the
        // dedicated synthetic runtime has been joined and its IPC is gone.
        let guardian_closed = if self.fresh_start_attempted && engine_closed {
            self.close_fresh_guardian().await.is_ok()
        } else {
            false
        };
        if self.fresh_start_attempted {
            failed |= !guardian_closed;
            println!(
                "phase=fixture_guardian_close category={}",
                if guardian_closed {
                    "forced_fixture_reaped_ipc_absent"
                } else {
                    "not_confirmed"
                }
            );
        }
        let (broker_closed, read_closed_journal) = if !self.fresh_start_attempted || guardian_closed
        {
            match self.broker.take() {
                Some(mut owner) => {
                    let endpoint = Directory::open_existing(&self.broker_root.join("broker"))
                        .and_then(RetainedFixtureEndpoint::capture);
                    let endpoint_is_bound = match &endpoint {
                        Ok(endpoint) => endpoint.checked_peer().await.is_ok(),
                        Err(_) => false,
                    };
                    let reaped = owner.stop().await.is_ok();
                    let ipc_absent = if reaped && endpoint_is_bound {
                        match endpoint {
                            Ok(endpoint) => endpoint.assert_closed().await.is_ok(),
                            Err(_) => false,
                        }
                    } else {
                        false
                    };
                    println!("phase=fixture_broker_close reaped={reaped} ipc_absent={ipc_absent}");
                    let confirmed = reaped && ipc_absent;
                    (confirmed, confirmed)
                }
                None => {
                    println!("phase=fixture_broker_close category=not_started");
                    (true, false)
                }
            }
        } else {
            // A broker that is still live is the only reaper of this detached
            // descendant. Preserve that custody if identity or exit is unknown.
            println!("phase=fixture_broker_close category=retained_for_guardian_custody");
            (false, false)
        };
        failed |= !broker_closed;
        if read_closed_journal && let Some(check_journal) = check_journal {
            check_journal(&self.root);
        }
        for (_, guardian) in &mut self.enforcers {
            failed |= guardian.stop().await.is_err();
        }
        if let Some(stop) = self.server_stop.take() {
            let _ = stop.send(());
        }
        let gateway_status = match timeout(Duration::from_secs(5), &mut self.server_task).await {
            Ok(Ok(Ok(()))) => "joined",
            Ok(Ok(Err(_))) => "serve_error",
            Ok(Err(_)) => "join_error",
            Err(_) => {
                self.server_task.abort();
                let _ = timeout(Duration::from_secs(5), &mut self.server_task).await;
                "deadline"
            }
        };
        println!("phase=fixture_gateway_close category={gateway_status}");
        failed |= gateway_status != "joined";

        self.faults.trace.emit_phase_counts();
        let engine = self.engine.lock().await;
        let mut witness_failures = 0usize;
        for (category, count) in &engine.errors {
            println!("phase=fixture_witness reason={category:?} count={count}");
            witness_failures = witness_failures.saturating_add(*count);
        }
        println!("phase=fixture_witness error_count={witness_failures}");
        let closure = if failed {
            Err(anyhow::anyhow!(
                "owned synthetic fixture closure not confirmed"
            ))
        } else {
            Ok(())
        };
        (closure, witness_failures)
    }

    async fn close_fresh_guardian(&mut self) -> Result<()> {
        if self.fresh_guardian.is_none() {
            timeout(Duration::from_secs(4), async {
                loop {
                    if self.bind_fresh_guardian().await.is_ok() {
                        return;
                    }
                    sleep(Duration::from_millis(25)).await;
                }
            })
            .await
            .context("Fresh guardian identity not attested")?;
        }
        let broker_pid = self.live_broker_pid()?;
        let guardian = self
            .fresh_guardian
            .take()
            .context("missing Fresh guardian identity")?;
        timeout(Duration::from_secs(16), guardian.close(broker_pid))
            .await
            .context("Fresh guardian closure timeout")??;
        ensure!(
            self.live_broker_pid()? == broker_pid,
            "fixture broker did not retain reaping custody"
        );
        Ok(())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.fresh_start_attempted
            && let Some(child) = self.broker.as_mut()
        {
            if child.child.id().is_some() {
                let _ = child.child.start_kill();
            }
        }
        for (_, child) in &mut self.enforcers {
            if child.child.id().is_some() {
                let _ = child.child.start_kill();
            }
        }
        if let Some(stop) = self.engine_stop.take() {
            let _ = stop.send(());
        }
        if let Some(stop) = self.server_stop.take() {
            let _ = stop.send(());
        }
    }
}

/// Read-only observations of real private records, never constructors of authority.
pub struct History {
    pub endpoint: RelayEndpointV1,
    arm: ArmV1,
    binding: Option<Value>,
    claim: Value,
    owned: Value,
    starts: usize,
}
impl History {
    pub fn require_live(&self) -> Result<()> {
        let now = NativeHostClock::new()?.sample()?;
        ensure!(
            now.method == self.arm.clock.method
                && now.boot_id == self.arm.clock.boot_id
                && now.continuous_ns_decimal.parse::<u128>()?
                    < self.arm.deadline_ns_decimal.parse::<u128>()?
                && self.arm.timeout_seconds == ARM_SECONDS,
            "authority oracle must remain inside the original Arm/method/boot/deadline"
        );
        Ok(())
    }
    pub fn unchanged(&self, after: &History) -> Result<()> {
        self.require_live()?;
        after.require_live()?;
        ensure!(
            self.endpoint == after.endpoint
                && serde_json::to_value(&self.arm)? == serde_json::to_value(&after.arm)?
                && self.binding == after.binding
                && self.claim == after.claim
                && self.owned == after.owned
                && self.starts == after.starts,
            "recovery must retain exact original assignment/Arm/claim/CID without start"
        );
        Ok(())
    }
}
// The immutable frame selects the existing Journal hash domain. The expected
// hash still comes from the confirmed IPC response or the real Journal receipt.
fn confirmed_checkpoint_value(bytes: &[u8], expected_hash: Option<&str>) -> Result<Value> {
    let value: Value = serde_json::from_slice(bytes)?;
    let domain = match value["version"].as_u64() {
        Some(1) => b"ai-guard:broker-checkpoint:v1\0".as_slice(),
        Some(2) => b"ai-guard:broker-checkpoint:v2\0".as_slice(),
        Some(3) => b"ai-guard:broker-checkpoint:v3\0".as_slice(),
        Some(4) => b"ai-guard:broker-checkpoint:v4\0".as_slice(),
        Some(5) => b"ai-guard:broker-checkpoint:v5\0".as_slice(),
        _ => bail!("unsupported fixture checkpoint version"),
    };
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(bytes);
    let hash = hex::encode(hash.finalize());
    ensure!(
        expected_hash == Some(hash.as_str()),
        "history must match a confirmed IPC head"
    );
    Ok(value)
}

pub fn history_hash_checks_real_journal_frames() -> Result<()> {
    use agentsandbox::{
        broker::{
            checkpoint::{BrokerCheckpointV2, SupervisionCheckpointV1},
            intent::ExecutionIntentV1,
            journal::{
                BindingInputs, ControlSecrets, FsExclusions, Journal, JournalLimitsV1,
                JournalScopeV1, ReserveResult, RevisionRefV1,
            },
            lifecycle::BrokerCheckpointV3,
            outbox::BrokerCheckpointV4,
        },
        runner::CheckpointPayloadV1,
    };
    let temp = tempfile::tempdir().context("history canary: create fixture directory")?;
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700))
        .context("history canary: set private fixture mode")?;
    let root = temp
        .path()
        .canonicalize()
        .context("history canary: canonicalize fixture directory")?;
    let mut plan: Value =
        serde_json::from_str(include_str!("../fixtures/broker_intent-plan-v3.json"))
            .context("history canary: parse fixture plan")?;
    plan["version"] = json!(1);
    plan["options"]["profile"] = json!("strict");
    plan["options"]["network"] = json!("none");
    plan["options"]["provider"] = json!("generic");
    plan["options"]["argv"] = json!(["/usr/bin/id"]);
    for field in [
        "client",
        "auth_mode",
        "client_profile",
        "providers_file",
        "prompt",
        "model",
    ] {
        plan["options"]
            .as_object_mut()
            .context("fixture options")?
            .remove(field);
    }
    plan["request"]["profile"] = json!("strict");
    plan["request"]["agent_id"] = json!("generic-22222222-2222-4222-8222-222222222222");
    plan["request"]["command"]["argv"] = json!(["/usr/bin/id"]);
    plan["request"]
        .as_object_mut()
        .context("fixture request")?
        .remove("provider_resolution");
    plan["provider_input"] = Value::Null;
    plan["provider_credential"] = Value::Null;
    let intent = ExecutionIntentV1::from_plan_bytes(
        &serde_json::to_vec(&plan).context("history canary: encode fixture plan")?,
    )
    .context("history canary: validate fixture intent")?;
    let operation = intent.operation_id();
    let journal = Journal::open(
        &Directory::open_existing(&root).context("history canary: open fixture directory")?,
        JournalScopeV1::new(
            Uuid::new_v4(),
            "fixture-history",
            "http://127.0.0.1:8787",
            Uuid::new_v4(),
            "fixture-history",
        )
        .context("history canary: construct journal scope")?,
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new())
            .context("history canary: construct empty control secrets")?,
    )
    .context("history canary: open journal")?;
    let ReserveResult::New(ticket) = journal
        .reserve(operation, b"history hash regression")
        .context("history canary: reserve fresh operation")?
    else {
        bail!("fresh fixture reservation required");
    };
    let initial = CheckpointPayloadV1::initial(&intent)
        .context("history canary: construct initial V1 payload")?;
    let accepted = journal
        .accept(ticket, &intent, BindingInputs::None, &initial)
        .context("history canary: accept initial V1 checkpoint")?;
    let scopes = std::fs::read_dir(root.join("broker-journal-v1"))
        .context("history canary: open journal scope inventory")?
        .collect::<io::Result<Vec<_>>>()
        .context("history canary: read journal scope inventory")?;
    ensure!(scopes.len() == 1, "one regression journal scope");
    let job_root = scopes[0].path().join("jobs").join(operation.to_string());
    let check = |head: &RevisionRefV1, version: u64| -> Result<()> {
        let bytes = read_bytes(
            &job_root.join(format!("rev-{:020}.json", head.revision())),
            1024 * 1024,
        )?;
        // The expected digest is produced independently by the real Journal.
        let value = confirmed_checkpoint_value(&bytes, Some(head.hash()))?;
        ensure!(value["version"] == version, "real Journal frame version");
        ensure!(
            confirmed_checkpoint_value(&bytes, None).is_err(),
            "missing expected hash must refuse"
        );
        let wrong_hash = if head.hash() == "0".repeat(64) {
            "1".repeat(64)
        } else {
            "0".repeat(64)
        };
        ensure!(
            confirmed_checkpoint_value(&bytes, Some(&wrong_hash)).is_err(),
            "discordant expected hash must refuse"
        );
        let mut unknown = value;
        unknown["version"] = json!(99);
        let error = confirmed_checkpoint_value(&serde_json::to_vec(&unknown)?, Some(head.hash()))
            .err()
            .context("unknown version must refuse")?;
        ensure!(
            error.to_string() == "unsupported fixture checkpoint version",
            "unknown version must refuse before digest comparison"
        );
        Ok(())
    };
    check(accepted.head(), 1).context("history canary: check real V1 frame")?;
    let broker = BrokerCheckpointV2::new(
        initial,
        SupervisionCheckpointV1::new(journal.scope().control_namespace_uuid(), vec![], vec![])
            .context("history canary: construct supervision in the journal namespace")?,
    )
    .context("history canary: construct V2 checkpoint")?;
    let supervised = journal
        .advance_supervised(operation, accepted.head().clone(), &broker)
        .context("history canary: advance V1 to supervised V2")?;
    check(supervised.head(), 2).context("history canary: check real V2 frame")?;
    let lifecycle = BrokerCheckpointV3::new(broker, vec![])
        .context("history canary: construct V3 checkpoint")?;
    let lifecycle_receipt = journal
        .advance_lifecycle(operation, supervised.head().clone(), &lifecycle)
        .context("history canary: advance V2 to lifecycle V3")?;
    check(lifecycle_receipt.head(), 3).context("history canary: check real V3 frame")?;
    let outbox = BrokerCheckpointV4::new(lifecycle, vec![])
        .context("history canary: construct V4 checkpoint")?;
    let outbox_receipt = journal
        .advance_outbox(operation, lifecycle_receipt.head().clone(), &outbox)
        .context("history canary: advance V3 to outbox V4")?;
    check(outbox_receipt.head(), 4).context("history canary: check real V4 frame")?;
    // V5 is private and remains checked by the real Full3 parent history oracle.
    Ok(())
}

pub fn timeout_import_outcome_projection() -> Result<()> {
    use agentsandbox::broker::lifecycle::{
        LocalCommandRequestV1, LocalCommandV1, LocalFailureCodeV1, LocalFailurePhaseV1,
    };
    let mut output = ConfirmedFrameObservation::default();
    let success = json!({"observation":{
        "exit_code":0,"running":false,"started_at":"present","finished_at":"present",
        "private":"projection-private-sentinel"
    }});
    output
        .import_outcomes
        .observe("helper_observation", &success, true);
    output
        .import_outcomes
        .observe("helper_observation", &success, false);
    output.import_outcomes.observe(
        "helper_observation",
        &json!({"observation":{"exit_code":7,"running":false,"started_at":"present","finished_at":"present"}}),
        true,
    );
    output.import_outcomes.observe(
        "helper_observation",
        &json!({"observation":{"exit_code":null,"running":null,"started_at":null,"finished_at":null}}),
        true,
    );
    output.import_outcomes.observe(
        "helper_cleanup",
        &json!({"removed":true,"error":null}),
        true,
    );
    output.import_outcomes.observe(
        "helper_cleanup",
        &json!({"removed":false,"error":"projection-private-sentinel"}),
        true,
    );
    output
        .import_outcomes
        .observe("helper_cleanup", &json!({}), true);
    ensure!(
        output.import_outcomes.exit_zero == 1
            && output.import_outcomes.exit_nonzero == 1
            && output.import_outcomes.exit_unknown == 1
            && output.import_outcomes.terminal_complete == 2
            && output.import_outcomes.cleanup_removed == 1
            && output.import_outcomes.cleanup_not_removed == 1
            && output.import_outcomes.cleanup_removed_unknown == 1
            && output.import_outcomes.cleanup_error_present == 1,
        "acknowledged outcomes must retain false and unknown separately"
    );
    let command = LocalCommandV1::new(
        Uuid::new_v4(),
        &"a".repeat(64),
        Uuid::new_v4(),
        LocalCommandRequestV1::FailureObserved {
            phase: LocalFailurePhaseV1::ImportVerification,
            code: LocalFailureCodeV1::TransportFailed,
        },
    )?;
    project_local_failures(&json!({"commands":[command]}), &mut output)?;
    ensure!(
        output.local_failures_known
            && output.local_failure_count == 1
            && output.last_failure_phase == Some(LocalFailurePhaseV1::ImportVerification)
            && output.last_failure_code == Some(LocalFailureCodeV1::TransportFailed),
        "local failure projection must preserve only typed phase and code"
    );
    ensure!(
        !serde_json::to_string(&output)?.contains("projection-private-sentinel"),
        "timeout outcome projection must omit private text"
    );
    ensure!(
        project_local_failures(&json!({"commands":[{"request":{"kind":"failure_observed","phase":"private-phase","code":"unavailable"}}]}), &mut ConfirmedFrameObservation::default()).is_err(),
        "unknown lifecycle input cannot become a known empty failure projection"
    );
    Ok(())
}

fn read_bytes(path: &Path, max: u64) -> Result<Vec<u8>> {
    let parent = Directory::open_existing(path.parent().context("private fixture parent")?)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("private fixture record")?;
    Ok(parent.read_private_file(name, max)?.bytes().to_vec())
}
fn read_json(path: &Path, max: u64) -> Result<Value> {
    Ok(serde_json::from_slice(&read_bytes(path, max)?)?)
}
fn announcement_epoch(role: &Path) -> Result<Uuid> {
    let mut entries = std::fs::read_dir(role.join("announcements"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    ensure!(
        !entries.is_empty() && entries.len() <= 128,
        "bounded real announcement inventory"
    );
    entries.sort();
    let value = read_json(entries.last().context("real announcement")?, 4096)?;
    Ok(Uuid::parse_str(
        value["endpoint"]["epoch"]
            .as_str()
            .context("real process epoch")?,
    )?)
}
fn collect_operations<'a>(value: &'a Value, kind: &str, found: &mut Vec<&'a Value>) {
    match value {
        Value::Object(fields) => {
            if fields.get("kind").and_then(Value::as_str) == Some(kind)
                && fields.contains_key("request")
            {
                found.push(value);
            }
            for child in fields.values() {
                collect_operations(child, kind, found);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_operations(child, kind, found);
            }
        }
        _ => {}
    }
}
fn collect_named_arrays<'a>(value: &'a Value, key: &str, found: &mut Vec<&'a Vec<Value>>) {
    match value {
        Value::Object(fields) => {
            if let Some(Value::Array(items)) = fields.get(key) {
                found.push(items);
            }
            for child in fields.values() {
                collect_named_arrays(child, key, found);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_named_arrays(child, key, found);
            }
        }
        _ => {}
    }
}
pub async fn grant(
    f: &Fixture,
    source: RelayEndpointV1,
    destination: RelayEndpointV1,
) -> Result<RelayReceiptV1> {
    let input = RelayGrantV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: Uuid::new_v4().to_string(),
        source,
        destination,
        channel: "software-fixture".into(),
        expires_at: (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
    };
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?
        .post(format!("{}/api/v1/relay/grants", f.origin))
        .bearer_auth(A_RUNNER)
        .json(&input)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}
