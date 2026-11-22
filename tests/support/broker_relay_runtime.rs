//! Shared software runtime witness and public relay wire helpers.
//! Runtime observations remain synthetic; protocol and strict parser bodies
//! are mechanically moved from the historical handoff support.
use super::{IMAGE, private_dir};
use agentsandbox::{
    audit::Store,
    control::{WorkerStateV1, WorkerV1},
    control_fs::Directory,
    relay::{
        RelayEndpointV1,
        channel::{
            self, CloseReasonV1, FrameLimit, FrameReader, FrameV1, FrameWriter, RequestPayloadV1,
            RequestV1, ResponseResultV1, helper,
        },
    },
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::{Mutex, oneshot, watch},
    time::{sleep, timeout},
};
use uuid::Uuid;

// Only the Full3 fixture opts in. The historical handoff shares this file
// without collecting or emitting a diagnostic.
#[allow(dead_code)]
pub(super) mod diagnostics {
    use agentsandbox::{
        broker::service::protocol::{BrokerErrorCodeV1, JobStatusV1},
        control::WorkerStateV1,
    };
    use std::{
        collections::{BTreeMap, VecDeque},
        fmt::Write,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        time::Instant,
    };

    // Reserve 4 KiB for the post-cleanup ticket; combined output stays 16 KiB.
    pub(crate) const OUTPUT_BYTES: usize = 12 * 1024;
    const TRANSITIONS: usize = 64;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    pub(crate) enum Phase {
        GatewayControlProtocol,
        GatewayDecision,
        GatewayAdmission,
        GatewayTransition,
        GatewayArm,
        GatewayHelperClaim,
        GatewayHelperObservation,
        GatewayHelperCleanup,
        GatewayWorkerClaim,
        IpcSubmit,
        IpcRelayStatus,
        IpcJobStatus,
        RuntimeFrame,
        RuntimeInfo,
        RuntimeImage,
        RuntimeVolume,
        RuntimeCreate,
        RuntimeStart,
        RuntimeInspect,
        RuntimeInventory,
        RuntimeStop,
        RuntimeRemove,
        RuntimeExec,
        RuntimeStartReply,
        ImportInput,
        WitnessTerminal,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    pub(crate) enum Purpose {
        Unknown,
        Run,
        Import,
        Export,
    }
    impl Purpose {
        pub(crate) fn from_label(value: &str) -> Self {
            match value {
                "run" => Self::Run,
                "import" => Self::Import,
                "export" => Self::Export,
                _ => Self::Unknown,
            }
        }
    }
    #[derive(Clone, Copy, Debug)]
    pub(crate) enum Outcome {
        Enter,
        Ok,
        NonSuccess,
        Error,
        Unfinished,
    }
    #[derive(Clone, Default)]
    struct Metric {
        entered: u64,
        completed: u64,
        ok: u64,
        non_success: u64,
        errors: u64,
        unfinished: u64,
        total_us: u64,
        max_us: u64,
        last_at_us: u64,
    }
    #[derive(Clone, Copy)]
    struct Transition {
        phase: Phase,
        purpose: Purpose,
        outcome: Outcome,
        at_us: u64,
        duration_us: u64,
    }
    #[derive(Default)]
    struct State {
        metrics: BTreeMap<(Phase, Purpose), Metric>,
        transitions: VecDeque<Transition>,
        evicted: u64,
        // Only fixed-size/numeric public Status fields; never hash/content.
        job: Option<JobProgress>,
    }
    #[derive(Debug)]
    struct JobProgress {
        head_revision: Option<u64>,
        pending_rpc_count: Option<usize>,
        failure_count: usize,
        worker_state: Option<WorkerStateV1>,
        executor_running: bool,
        executor_error: Option<BrokerErrorCodeV1>,
    }
    pub(crate) struct Trace {
        started: Instant,
        state: Mutex<State>,
        emitted: AtomicBool,
    }
    pub(crate) struct Scope {
        trace: Arc<Trace>,
        phase: Phase,
        purpose: Purpose,
        started: Instant,
        finished: bool,
    }
    fn micros(instant: Instant) -> u64 {
        u64::try_from(instant.elapsed().as_micros()).unwrap_or(u64::MAX)
    }
    impl Trace {
        pub(crate) fn new() -> Arc<Self> {
            Arc::new(Self {
                started: Instant::now(),
                state: Mutex::new(State::default()),
                emitted: AtomicBool::new(false),
            })
        }
        pub(crate) fn begin(self: &Arc<Self>, phase: Phase, purpose: Purpose) -> Scope {
            self.record(phase, purpose, Outcome::Enter, 0);
            Scope {
                trace: Arc::clone(self),
                phase,
                purpose,
                started: Instant::now(),
                finished: false,
            }
        }
        fn record(&self, phase: Phase, purpose: Purpose, outcome: Outcome, duration_us: u64) {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            let at_us = micros(self.started);
            let metric = state.metrics.entry((phase, purpose)).or_default();
            match outcome {
                Outcome::Enter => metric.entered = metric.entered.saturating_add(1),
                _ => {
                    metric.completed = metric.completed.saturating_add(1);
                    metric.total_us = metric.total_us.saturating_add(duration_us);
                    metric.max_us = metric.max_us.max(duration_us);
                    match outcome {
                        Outcome::Ok => metric.ok = metric.ok.saturating_add(1),
                        Outcome::NonSuccess => {
                            metric.non_success = metric.non_success.saturating_add(1)
                        }
                        Outcome::Error => metric.errors = metric.errors.saturating_add(1),
                        Outcome::Unfinished => {
                            metric.unfinished = metric.unfinished.saturating_add(1)
                        }
                        Outcome::Enter => {}
                    }
                }
            }
            metric.last_at_us = at_us;
            if state.transitions.len() == TRANSITIONS {
                state.transitions.pop_front();
                state.evicted = state.evicted.saturating_add(1);
            }
            state.transitions.push_back(Transition {
                phase,
                purpose,
                outcome,
                at_us,
                duration_us,
            });
        }
        pub(crate) fn observe_job(&self, job: &JobStatusV1) {
            if let Ok(mut state) = self.state.lock() {
                state.job = Some(JobProgress {
                    head_revision: job.head_revision,
                    pending_rpc_count: job.pending_rpc_count,
                    failure_count: job.failure_count,
                    worker_state: job.worker_state,
                    executor_running: job.executor_task_running,
                    executor_error: job.executor_error,
                });
            }
        }
        pub(crate) fn emit_phase_counts(&self) {
            let Ok(state) = self.state.lock() else {
                println!("phase=fixture_metrics category=unavailable");
                return;
            };
            for ((phase, purpose), m) in &state.metrics {
                // Relative fixture-clock times only; no runtime or private value.
                println!(
                    "phase=fixture_metric name={phase:?} purpose={purpose:?} enter={} end={} ok={} non_success={} error={} unfinished={} max_us={} last_at_us={}",
                    m.entered,
                    m.completed,
                    m.ok,
                    m.non_success,
                    m.errors,
                    m.unfinished,
                    m.max_us,
                    m.last_at_us,
                );
            }
        }
        pub(crate) fn emit_failure_once(&self) {
            if self.emitted.swap(true, Ordering::Relaxed) {
                return;
            }
            let mut out = String::from(
                "full3_prerequisite_diagnostic_v1\nlocal_helper_observation_ack=UNKNOWN local_helper_cleanup_ack=UNKNOWN\ngateway=handler_response_only witness=synthetic_handler_result broker_status=cached_projection\nok=phase_function_completed non_success=non_2xx_or_unexpected_ipc_reply\n",
            );
            if let Ok(state) = self.state.lock() {
                let _ = writeln!(
                    out,
                    "elapsed_us={} retained_transitions={} evicted_transitions={} last_ipc_job={:?}",
                    micros(self.started),
                    state.transitions.len(),
                    state.evicted,
                    state.job
                );
                for ((phase, purpose), m) in &state.metrics {
                    let _ = writeln!(
                        out,
                        "metric {phase:?}/{purpose:?} enter={} end={} ok={} non_success={} error={} unfinished={} total_us={} max_us={} last_at_us={}",
                        m.entered,
                        m.completed,
                        m.ok,
                        m.non_success,
                        m.errors,
                        m.unfinished,
                        m.total_us,
                        m.max_us,
                        m.last_at_us
                    );
                }
                for t in &state.transitions {
                    let _ = writeln!(
                        out,
                        "transition {:?}/{:?} {:?} at_us={} duration_us={}",
                        t.phase, t.purpose, t.outcome, t.at_us, t.duration_us
                    );
                }
            } else {
                out.push_str("diagnostic_state=UNAVAILABLE\n");
            }
            // Every rendered value above is an enum, numeric value or ASCII
            // constant. Cap this one emission, including its truncation marker.
            const CUT: &str = "diagnostic_output=TRUNCATED\n";
            if out.len() > OUTPUT_BYTES {
                out.truncate(OUTPUT_BYTES - CUT.len());
                out.push_str(CUT);
            }
            eprint!("{out}");
        }
    }
    impl Scope {
        pub(crate) fn finish(mut self, outcome: Outcome) {
            self.trace
                .record(self.phase, self.purpose, outcome, micros(self.started));
            self.finished = true;
        }
    }
    impl Drop for Scope {
        fn drop(&mut self) {
            if !self.finished {
                // A dropped scope does not distinguish cancellation, early
                // return or unwind. Do not invent a successful RPC or ACK.
                self.trace.record(
                    self.phase,
                    self.purpose,
                    Outcome::Unfinished,
                    micros(self.started),
                );
            }
        }
    }
}

pub(super) struct Container {
    pub(super) facts: Value,
    pub(super) done: watch::Sender<bool>,
    pub(super) helper_root: Directory,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportCounts {
    pub imports_created: usize,
    pub imports_removed: usize,
    pub verification_created: usize,
    pub workers_created: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum EngineFailure {
    OwnerJoin,
    ListenerAccept,
    ArgumentFrame,
    CommandDispatch,
    CreateGrammar,
    RuntimeCommand,
}
impl std::fmt::Display for EngineFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

pub(super) struct Engine {
    pub(super) next_id: u64,
    pub(super) containers: BTreeMap<String, Container>,
    pub(super) volumes: BTreeMap<String, Value>,
    pub(super) calls: Vec<Vec<String>>,
    pub(super) execs: BTreeMap<String, usize>,
    pub(super) errors: BTreeMap<EngineFailure, usize>,
    pub(super) root: PathBuf,
    pub(super) import_gate: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
    pub(super) import_counts: ImportCounts,
}
impl Engine {
    pub(super) fn note_failure(&mut self, reason: EngineFailure) {
        let count = self.errors.entry(reason).or_default();
        *count = count.saturating_add(1);
    }

    fn find(&self, value: &str) -> Option<String> {
        self.containers.iter().find_map(|(id, c)| {
            (id == value
                || c.facts["Name"]
                    .as_str()
                    .is_some_and(|name| name.trim_start_matches('/') == value))
            .then(|| id.clone())
        })
    }
    fn terminal(&mut self, id: &str) -> Result<()> {
        let container = self
            .containers
            .get_mut(id)
            .context("unknown synthetic CID")?;
        let started = container.facts["State"]["StartedAt"]
            .as_str()
            .context("synthetic StartedAt")?;
        let mut finished = chrono::Utc::now();
        let start = chrono::DateTime::parse_from_rfc3339(started)?;
        if finished <= start {
            finished = start.with_timezone(&chrono::Utc) + chrono::Duration::milliseconds(1);
        }
        container.facts["State"]["Status"] = json!("exited");
        container.facts["State"]["Running"] = json!(false);
        container.facts["State"]["FinishedAt"] = json!(finished.to_rfc3339());
        container.done.send_replace(true);
        Ok(())
    }
}
fn option(args: &[String], key: &str) -> Result<String> {
    args.windows(2)
        .find_map(|p| (p[0] == key).then(|| p[1].clone()))
        .ok_or_else(|| anyhow!("fixture missing option {key}"))
}
fn values(args: &[String], key: &str) -> Vec<String> {
    args.windows(2)
        .filter_map(|p| (p[0] == key).then(|| p[1].clone()))
        .collect()
}

/// This canary calls the actual fixture create path, including its mutation
/// boundary. It must fail against review89's first-occurrence parser.
pub fn contradictory_create_flags_are_refused_before_fixture_mutation() -> Result<()> {
    use agentsandbox::runtime::{ContainerPurpose, ContainerSpec, NetworkMode, RuntimeKind};
    let spec = ContainerSpec::new(
        Uuid::new_v4(),
        ContainerPurpose::Run,
        IMAGE,
        vec!["/usr/bin/id".into()],
        NetworkMode::None,
    )?;
    let baseline = spec.create_args(RuntimeKind::Docker)?;
    let base = std::fs::canonicalize(std::env::temp_dir())?;
    let control = tempfile::tempdir_in(&base)?;
    let mut engine = Engine {
        next_id: 1,
        containers: BTreeMap::new(),
        volumes: BTreeMap::new(),
        calls: vec![],
        execs: BTreeMap::new(),
        errors: BTreeMap::new(),
        import_gate: None,
        import_counts: Default::default(),
        root: control.path().into(),
    };
    let id = create(&mut engine, &baseline)?;
    ensure!(
        engine.containers[&id].facts["HostConfig"]["NetworkMode"] == "none",
        "valid production-generated create is the positive control"
    );
    for (purpose, argv, stdin) in [
        (
            ContainerPurpose::Import,
            vec![
                "/bin/tar",
                "--extract",
                "--file=-",
                "--directory=/workspace",
                "--no-same-owner",
                "--no-same-permissions",
            ],
            true,
        ),
        (
            ContainerPurpose::Export,
            vec![
                "/bin/tar",
                "--create",
                "--file=-",
                "--directory=/workspace",
                ".",
            ],
            false,
        ),
    ] {
        let spec = ContainerSpec::new(
            Uuid::new_v4(),
            purpose,
            IMAGE,
            argv.into_iter().map(String::from).collect(),
            NetworkMode::None,
        )?;
        let id = create(&mut engine, &spec.create_args(RuntimeKind::Docker)?)?;
        ensure!(
            engine.containers[&id].facts["Config"]["OpenStdin"] == stdin
                && engine.containers[&id].facts["Config"]["StdinOnce"] == stdin,
            "valid production helper create preserves parsed stream flags"
        );
    }

    let pull = baseline
        .iter()
        .position(|value| value == "--pull=never")
        .context("production create omitted no-pull option")?;
    let mut without_pull = baseline.clone();
    without_pull.remove(pull);
    let pull_cases = [
        vec![],
        vec!["--pull=always"],
        vec!["--pull=missing"],
        vec!["--pull", "never"],
        vec!["--pull=never", "--pull=never"],
    ];
    for replacement in pull_cases {
        let mut args = without_pull.clone();
        args.splice(pull..pull, replacement.into_iter().map(String::from));
        let root = tempfile::tempdir_in(&base)?;
        let mut rejected = Engine {
            next_id: 1,
            containers: BTreeMap::new(),
            volumes: BTreeMap::new(),
            calls: vec![],
            execs: BTreeMap::new(),
            errors: BTreeMap::new(),
            import_gate: None,
            import_counts: Default::default(),
            root: root.path().into(),
        };
        let error = create(&mut rejected, &args)
            .err()
            .context("invalid pull mode admitted")?;
        ensure!(
            matches!(
                error.downcast_ref::<EngineFailure>(),
                Some(EngineFailure::CreateGrammar)
            ),
            "invalid pull mode did not reach the create grammar"
        );
        ensure!(
            rejected.next_id == 1
                && rejected.containers.is_empty()
                && rejected.volumes.is_empty()
                && rejected.import_counts == ImportCounts::default()
                && std::fs::read_dir(root.path())?.next().is_none(),
            "invalid pull mode mutated the fixture"
        );
    }

    let mut contradictions: Vec<Vec<String>> = [
        vec!["--pull=never"],
        vec!["--pull=always"],
        vec!["--network", "host"],
        vec!["--privileged=true"],
        vec!["--tty=true"],
        vec!["--mount", "type=bind,source=/private,target=/workspace"],
        vec!["--network=host"],
        vec!["--mount=type=bind,source=/private,target=/workspace"],
        vec!["--read-only=false"],
        vec!["--cap-add", "ALL"],
        vec!["--security-opt", "seccomp=unconfined"],
        vec!["--interactive"],
        vec!["--env", "HOME=/home/agent"],
        vec!["--label", "agentsandbox.purpose=export"],
        vec!["--tmpfs", "/tmp:rw,exec"],
        vec!["-t"],
        vec!["--env=OPENAI_API_KEY=public-fixture"],
        vec!["--env-file=/private/fixture"],
        vec!["--interactive=false"],
        vec!["--read-only=true"],
    ]
    .into_iter()
    .map(|items| items.into_iter().map(String::from).collect())
    .collect();
    for key in [
        "--name",
        "--log-driver",
        "--cap-drop",
        "--security-opt",
        "--user",
        "--network",
        "--pids-limit",
        "--memory",
        "--cpus",
        "--workdir",
        "--entrypoint",
    ] {
        contradictions.push(vec![key.into(), option(&baseline, key)?]);
    }
    contradictions.push(vec!["--read-only".into()]);
    for (case, extra) in contradictions.into_iter().enumerate() {
        let root = tempfile::tempdir_in(&base)?;
        let mut engine = Engine {
            next_id: 1,
            containers: BTreeMap::new(),
            volumes: BTreeMap::new(),
            calls: vec![],
            execs: BTreeMap::new(),
            errors: BTreeMap::new(),
            import_gate: None,
            import_counts: Default::default(),
            root: root.path().into(),
        };
        let mut args = baseline.clone();
        let separator = args
            .iter()
            .position(|arg| arg == "--")
            .context("production separator")?;
        args.splice(separator..separator, extra);
        ensure!(
            create(&mut engine, &args).is_err(),
            "contradictory create case {case} was accepted"
        );
        ensure!(
            engine.next_id == 1
                && engine.containers.is_empty()
                && std::fs::read_dir(root.path())?.next().is_none(),
            "invalid create case {case} mutated fixture state"
        );
    }
    Ok(())
}
struct ValidatedCreate {
    name: String,
    config: Value,
    host: Value,
    mounts: Value,
}

// A fixture grammar, not an alternate runtime policy. It accepts only the
// exact option forms emitted by these software scenarios. Unknown options and
// duplicate scalars/named values fail before create can mutate any state.
fn validate_create(args: &[String]) -> Result<ValidatedCreate> {
    ensure!(
        args.first().map(String::as_str) == Some("create"),
        "create command"
    );
    let mut singles = BTreeMap::<&str, &str>::new();
    let mut flags = std::collections::BTreeSet::new();
    let mut labels = BTreeMap::<&str, &str>::new();
    let mut environment = BTreeMap::<&str, &str>::new();
    let mut tmpfs = BTreeMap::<&str, &str>::new();
    let mut at = 1;
    while args.get(at).map(String::as_str) != Some("--") {
        let key = args
            .get(at)
            .context("missing create image separator")?
            .as_str();
        if matches!(key, "--pull=never" | "--read-only" | "--interactive") {
            ensure!(flags.insert(key), "duplicate create flag");
            at += 1;
            continue;
        }
        ensure!(
            matches!(
                key,
                "--name"
                    | "--log-driver"
                    | "--cap-drop"
                    | "--security-opt"
                    | "--user"
                    | "--network"
                    | "--pids-limit"
                    | "--memory"
                    | "--cpus"
                    | "--workdir"
                    | "--mount"
                    | "--entrypoint"
                    | "--label"
                    | "--env"
                    | "--tmpfs"
            ),
            "unknown or inline create option"
        );
        let value = args
            .get(at + 1)
            .context("create option without value")?
            .as_str();
        ensure!(
            !value.is_empty() && !value.starts_with('-'),
            "invalid create option value"
        );
        match key {
            "--label" | "--env" | "--tmpfs" => {
                let separator = if key == "--tmpfs" { ':' } else { '=' };
                let (name, content) = value.split_once(separator).context("named create value")?;
                let target = match key {
                    "--label" => &mut labels,
                    "--env" => &mut environment,
                    _ => &mut tmpfs,
                };
                ensure!(
                    target.insert(name, content).is_none(),
                    "duplicate named create value"
                );
            }
            _ => {
                ensure!(
                    singles.insert(key, value).is_none(),
                    "duplicate scalar create option"
                );
            }
        }
        at += 2;
    }
    ensure!(
        args.get(at + 1).map(String::as_str) == Some(IMAGE),
        "fixture immutable image"
    );
    let command = args.get(at + 2..).context("create command suffix")?;
    let required = |key: &str| {
        singles
            .get(key)
            .copied()
            .ok_or_else(|| anyhow!("missing create option {key}"))
    };
    for (key, expected) in [
        ("--log-driver", "none"),
        ("--cap-drop", "ALL"),
        ("--security-opt", "no-new-privileges"),
        ("--user", "1000:1000"),
        ("--network", "none"),
        ("--pids-limit", "256"),
        ("--memory", "1g"),
        ("--cpus", "2"),
        ("--workdir", "/workspace"),
    ] {
        ensure!(required(key)? == expected, "fixture create option {key}");
    }
    ensure!(
        flags.contains("--pull=never"),
        "fixture requires an explicit no-pull create"
    );
    ensure!(
        singles.len() == 12 && flags.contains("--read-only"),
        "create option inventory"
    );
    ensure!(labels.len() == 3, "create label inventory");
    for key in ["agentsandbox.session", "agentsandbox.operation"] {
        let value = labels.get(key).context("create identity label")?;
        let uuid = Uuid::parse_str(value)?;
        ensure!(
            !uuid.is_nil() && uuid.to_string() == *value,
            "create identity label format"
        );
    }
    let purpose = *labels
        .get("agentsandbox.purpose")
        .context("create purpose")?;
    let interactive = flags.contains("--interactive");
    ensure!(interactive == (purpose == "import"), "fixture stdin policy");
    let expected_command: &[&str] = match purpose {
        "run" => {
            ensure!(
                required("--entrypoint")? == "/usr/bin/id",
                "fixture worker entrypoint"
            );
            &[]
        }
        "import" => {
            ensure!(
                required("--entrypoint")? == "/bin/tar",
                "fixture import entrypoint"
            );
            &[
                "--extract",
                "--file=-",
                "--directory=/workspace",
                "--no-same-owner",
                "--no-same-permissions",
            ]
        }
        "export" => {
            ensure!(
                required("--entrypoint")? == "/bin/tar",
                "fixture export entrypoint"
            );
            &["--create", "--file=-", "--directory=/workspace", "."]
        }
        _ => bail!("unknown fixture purpose"),
    };
    ensure!(command == expected_command, "fixture command suffix");
    ensure!(
        environment == BTreeMap::from([("HOME", "/home/agent"), ("LANG", "C.UTF-8")]),
        "fixture environment differs"
    );
    ensure!(
        tmpfs
            == BTreeMap::from([
                ("/tmp", "rw,nosuid,nodev,noexec,size=256m,mode=1777"),
                (
                    "/home/agent",
                    "rw,nosuid,nodev,size=64m,uid=1000,gid=1000,mode=700"
                )
            ]),
        "fixture tmpfs differs"
    );
    let valid_name = |name: &str| {
        !name.is_empty()
            && name.len() <= 128
            && name.as_bytes()[0].is_ascii_alphanumeric()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    };
    let name = required("--name")?;
    ensure!(valid_name(name), "fixture container name");
    let mut mount = BTreeMap::new();
    for part in required("--mount")?.split(',') {
        let (key, value) = part.split_once('=').context("fixture mount syntax")?;
        ensure!(
            mount.insert(key, value).is_none(),
            "duplicate fixture mount field"
        );
    }
    ensure!(
        mount.len() == 3
            && mount.get("type") == Some(&"volume")
            && mount.get("target") == Some(&"/workspace"),
        "fixture mount inventory"
    );
    let volume = *mount.get("source").context("fixture volume source")?;
    ensure!(valid_name(volume), "fixture volume name");
    let environment: Vec<String> = environment
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    let memory_gib: u64 = required("--memory")?
        .strip_suffix('g')
        .context("fixture memory unit")?
        .parse()?;
    // Defaults below follow from the closed grammar rejecting every option
    // that could change them. Configurable facts come from the validated parse.
    Ok(ValidatedCreate {
        name: name.into(),
        config: json!({"Labels":labels,"User":required("--user")?,"WorkingDir":required("--workdir")?,
            "Entrypoint":[required("--entrypoint")?],"Cmd":command,"Env":environment,
            "Tty":false,"OpenStdin":interactive,"StdinOnce":interactive}),
        host: json!({"LogConfig":{"Type":required("--log-driver")?,"Config":{}},
            "NanoCpus":required("--cpus")?.parse::<u64>()? * 1_000_000_000,
            "CpuQuota":0,"CpuPeriod":0,"Memory":memory_gib * 1_073_741_824,
            "PidsLimit":required("--pids-limit")?.parse::<u64>()?,"ReadonlyRootfs":flags.contains("--read-only"),
            "Privileged":false,"AutoRemove":false,"Tmpfs":tmpfs,"NetworkMode":required("--network")?,
            "CapDrop":[required("--cap-drop")?],"SecurityOpt":[required("--security-opt")?],"RestartPolicy":{"Name":"no"}}),
        mounts: json!([{"Type":mount["type"],"Name":volume,"Destination":mount["target"],"RW":true}]),
    })
}

fn create(engine: &mut Engine, args: &[String]) -> Result<String> {
    let parsed = validate_create(args).context(EngineFailure::CreateGrammar)?;
    ensure!(
        engine.find(&parsed.name).is_none(),
        "duplicate synthetic create"
    );
    let sequence = engine.next_id;
    engine.next_id += 1;
    let id = format!("{sequence:064x}");
    let helper_path = private_dir(&engine.root, &format!("h{sequence}"))?;
    let helper_root = Directory::open_existing(&helper_path)?;
    let (done, _) = watch::channel(false);
    match parsed.config["Labels"]["agentsandbox.purpose"].as_str() {
        Some("import") => engine.import_counts.imports_created += 1,
        Some("export") => engine.import_counts.verification_created += 1,
        Some("run") => engine.import_counts.workers_created += 1,
        _ => {}
    }
    let facts = json!({"Id":id,"Name":format!("/{}", parsed.name),"Image":IMAGE,"RestartCount":0,
      "State":{"Status":"created","Running":false,"StartedAt":"0001-01-01T00:00:00Z","FinishedAt":"0001-01-01T00:00:00Z","ExitCode":0,"OOMKilled":false},
      "Config":parsed.config,"HostConfig":parsed.host,"Mounts":parsed.mounts});
    engine.containers.insert(
        id.clone(),
        Container {
            facts,
            done,
            helper_root,
        },
    );
    Ok(id)
}
async fn argument_frame(stream: &mut UnixStream) -> Result<Vec<String>> {
    let count = stream.read_u32().await? as usize;
    ensure!(count <= 256, "fixture argv count");
    let mut args = Vec::with_capacity(count);
    let mut total = 0usize;
    for _ in 0..count {
        let length = stream.read_u32().await? as usize;
        total = total
            .checked_add(length)
            .context("fixture length overflow")?;
        ensure!(total <= 96 * 1024, "fixture argv bytes");
        let mut bytes = vec![0; length];
        stream.read_exact(&mut bytes).await?;
        args.push(String::from_utf8(bytes)?);
    }
    Ok(args)
}
async fn response(stream: &mut UnixStream, code: i32, bytes: &[u8]) -> Result<()> {
    stream.write_all(&code.to_be_bytes()).await?;
    stream.write_all(bytes).await?;
    stream.shutdown().await?;
    Ok(())
}
#[allow(dead_code)] // The Full3 fixture selects the observed entry instead.
pub(super) async fn engine_connection(stream: UnixStream, state: Arc<Mutex<Engine>>) -> Result<()> {
    engine_connection_observed(stream, state, None).await
}
pub(super) async fn engine_connection_observed(
    mut stream: UnixStream,
    state: Arc<Mutex<Engine>>,
    trace: Option<Arc<diagnostics::Trace>>,
) -> Result<()> {
    use diagnostics::{Outcome, Phase, Purpose};
    let frame = trace
        .as_ref()
        .map(|t| t.begin(Phase::RuntimeFrame, Purpose::Unknown));
    let args = timeout(Duration::from_secs(5), argument_frame(&mut stream))
        .await
        .context(EngineFailure::ArgumentFrame)?
        .context(EngineFailure::ArgumentFrame)?;
    if let Some(frame) = frame {
        frame.finish(Outcome::Ok);
    }
    let at = args
        .iter()
        .position(|a| {
            matches!(
                a.as_str(),
                "info"
                    | "image"
                    | "volume"
                    | "create"
                    | "start"
                    | "inspect"
                    | "ps"
                    | "kill"
                    | "stop"
                    | "rm"
                    | "exec"
            )
        })
        .context(EngineFailure::CommandDispatch)?;
    let call = &args[at..];
    let (phase, purpose) = {
        let mut engine = state.lock().await;
        engine.calls.push(args.clone());
        let phase = match call[0].as_str() {
            "info" => Phase::RuntimeInfo,
            "image" => Phase::RuntimeImage,
            "volume" => Phase::RuntimeVolume,
            "create" => Phase::RuntimeCreate,
            "start" => Phase::RuntimeStart,
            "inspect" => Phase::RuntimeInspect,
            "ps" => Phase::RuntimeInventory,
            "kill" | "stop" => Phase::RuntimeStop,
            "rm" => Phase::RuntimeRemove,
            "exec" => Phase::RuntimeExec,
            _ => Phase::RuntimeFrame,
        };
        let purpose = if call[0] == "create" {
            values(call, "--label")
                .into_iter()
                .find_map(|label| {
                    label
                        .strip_prefix("agentsandbox.purpose=")
                        .map(Purpose::from_label)
                })
                .unwrap_or(Purpose::Unknown)
        } else {
            call.last()
                .and_then(|target| engine.find(target))
                .and_then(|id| engine.containers.get(&id))
                .and_then(|c| c.facts["Config"]["Labels"]["agentsandbox.purpose"].as_str())
                .map(Purpose::from_label)
                .unwrap_or(Purpose::Unknown)
        };
        (phase, purpose)
    };
    let command = trace.as_ref().map(|t| t.begin(phase, purpose));
    let result = engine_command(stream, call, state, trace).await;
    if let Some(command) = command {
        command.finish(if result.is_ok() {
            Outcome::Ok
        } else {
            Outcome::Error
        });
    }
    result
}
async fn engine_command(
    mut stream: UnixStream,
    call: &[String],
    state: Arc<Mutex<Engine>>,
    trace: Option<Arc<diagnostics::Trace>>,
) -> Result<()> {
    match call[0].as_str() {
        "info" => response(&mut stream, 0, br#"{"ID":"software-relay-fixture-engine","SecurityOptions":["name=rootless"],"ServerVersion":"fixture-only"}"#).await,
        "image" => response(&mut stream, 0, format!("{IMAGE}\n").as_bytes()).await,
        "volume" => {
            let name = call.last().context("volume name")?.clone();
            let mut state = state.lock().await;
            if call.get(1).map(String::as_str) == Some("create") {
                let labels: BTreeMap<String, String> = values(call, "--label")
                    .into_iter().map(|value| {
                        let (key, value) = value.split_once('=').context("volume label")?;
                        Ok((key.into(), value.into()))
                    }).collect::<Result<_>>()?;
                ensure!(!state.volumes.contains_key(&name), "duplicate volume create");
                state.volumes.insert(name.clone(), json!({"Name":name,"Driver":"local","Scope":"local","Labels":labels,"Options":{}}));
                drop(state);
                response(&mut stream, 0, name.as_bytes()).await
            } else {
                ensure!(call.get(1).map(String::as_str) == Some("inspect"), "volume command");
                let value = state.volumes.get(&name).cloned();
                drop(state);
                match value {
                    Some(value) => response(&mut stream, 0, &serde_json::to_vec(&value)?).await,
                    None => response(&mut stream, 125, b"").await,
                }
            }
        }
        "create" => {
            let id = create(&mut *state.lock().await, call)?;
            response(&mut stream, 0, id.as_bytes()).await
        }
        "inspect" => {
            let state = state.lock().await;
            let value = state.find(call.last().context("inspect identity")?)
                .map(|id| state.containers[&id].facts.clone());
            drop(state);
            match value {
                Some(value) => {
                    let value = if call.iter().any(|a| a == "{{json .State}}") {
                        value["State"].clone()
                    } else { value };
                    response(&mut stream, 0, &serde_json::to_vec(&value)?).await
                }
                None => response(&mut stream, 125, b"").await,
            }
        }
        "ps" => {
            let state = state.lock().await;
            let filters = values(call, "--filter");
            let name = filters.iter().find_map(|s| s.strip_prefix("name="))
                .map(|s| s.trim_start_matches('^').trim_end_matches('$'));
            let ids: Vec<_> = state.containers.iter().filter(|(_, c)| {
                name.is_none_or(|name| c.facts["Name"].as_str()
                    .is_some_and(|n| n.trim_start_matches('/') == name))
            }).map(|(id, _)| id.clone()).collect();
            drop(state);
            response(&mut stream, 0, ids.join("\n").as_bytes()).await
        }
        "kill" | "stop" => {
            let mut state = state.lock().await;
            let id = state.find(call.last().context("stop CID")?).context("unknown stop CID")?;
            state.terminal(&id)?;
            drop(state);
            response(&mut stream, 0, b"").await
        }
        "rm" => {
            let mut state = state.lock().await;
            let id = state.find(call.last().context("remove CID")?).context("unknown remove CID")?;
            ensure!(state.containers[&id].facts["State"]["Running"] == false, "remove before terminal");
            if state.containers[&id].facts["Config"]["Labels"]["agentsandbox.purpose"] == "import" {
                state.import_counts.imports_removed += 1;
            }
            state.containers.remove(&id);
            drop(state);
            response(&mut stream, 0, b"").await
        }
        "start" => start_witness(stream, call, state, trace).await,
        "exec" => exec_helper(stream, call, state).await,
        _ => bail!("unsupported synthetic runtime operation"),
    }
}

async fn start_witness(
    mut stream: UnixStream,
    call: &[String],
    state: Arc<Mutex<Engine>>,
    trace: Option<Arc<diagnostics::Trace>>,
) -> Result<()> {
    use diagnostics::{Outcome, Phase, Purpose};
    let (id, purpose, mut done) = {
        let mut state = state.lock().await;
        let id = state
            .find(call.last().context("start CID")?)
            .context("unknown start CID")?;
        let container = state.containers.get_mut(&id).context("start container")?;
        ensure!(
            container.facts["State"]["Status"] == "created",
            "second start"
        );
        container.facts["State"]["Status"] = json!("running");
        container.facts["State"]["Running"] = json!(true);
        container.facts["State"]["StartedAt"] = json!(chrono::Utc::now().to_rfc3339());
        let purpose = container.facts["Config"]["Labels"]["agentsandbox.purpose"]
            .as_str()
            .context("start purpose")?
            .to_owned();
        (id, purpose, container.done.subscribe())
    };
    let category = Purpose::from_label(&purpose);
    let reply = trace
        .as_ref()
        .map(|t| t.begin(Phase::RuntimeStartReply, category));
    stream.write_all(&0i32.to_be_bytes()).await?;
    if let Some(reply) = reply {
        reply.finish(Outcome::Ok);
    }
    if purpose == "run" {
        if !*done.borrow() {
            done.changed().await?;
        }
    } else {
        if purpose == "import" {
            let input_read = trace
                .as_ref()
                .map(|t| t.begin(Phase::ImportInput, category));
            let mut input = Vec::new();
            (&mut stream)
                .take(1024 * 1024)
                .read_to_end(&mut input)
                .await?;
            ensure!(input.len() < 1024 * 1024, "fixture input bound");
            if let Some(input_read) = input_read {
                input_read.finish(Outcome::Ok);
            }
            let gate = state.lock().await.import_gate.take();
            if let Some((entered, resume)) = gate {
                ensure!(
                    !input.is_empty(),
                    "import gate requires a transferred archive"
                );
                if entered.send(()).is_ok() {
                    // Dropping the test side releases the gate on every error.
                    // A test that loses its waiter cannot leave the witness paused.
                    let _ = timeout(Duration::from_secs(45), resume).await;
                }
            }
        } else {
            // Empty authorized source: valid empty tar, not a fabricated file change.
            stream.write_all(&[0; 1024]).await?;
        }
        let terminal = trace
            .as_ref()
            .map(|t| t.begin(Phase::WitnessTerminal, category));
        state.lock().await.terminal(&id)?;
        if let Some(terminal) = terminal {
            terminal.finish(Outcome::Ok);
        }
    }
    Ok(())
}

async fn exec_helper(
    mut stream: UnixStream,
    call: &[String],
    state: Arc<Mutex<Engine>>,
) -> Result<()> {
    let helper_at = call
        .iter()
        .position(|a| a == "/usr/local/bin/ai-guard-relay")
        .context("exec is not the fixed helper")?;
    ensure!(
        helper_at > 0 && call[helper_at + 1..] == ["serve-v1"],
        "helper argv differs"
    );
    let id = &call[helper_at - 1];
    let mut expected: Vec<String> = ["exec", "--interactive", "--tty=false", "--privileged=false"]
        .into_iter()
        .map(String::from)
        .collect();
    for key in [
        "AI_GUARD_PROVIDER_BOOTSTRAP",
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "ANTHROPIC_API_KEY",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
    ] {
        expected.push(format!("--env={key}="));
    }
    expected.extend([
        "--user".into(),
        "1000:1000".into(),
        id.clone(),
        "/usr/local/bin/ai-guard-relay".into(),
        "serve-v1".into(),
    ]);
    ensure!(
        call == expected,
        "helper exec must match the closed production argv"
    );
    let root = {
        let mut state = state.lock().await;
        let container = state.containers.get(id).context("exec CID")?;
        ensure!(
            container.facts["State"]["Running"] == true,
            "exec without running worker"
        );
        let root = container.helper_root.clone();
        *state.execs.entry(id.clone()).or_default() += 1;
        root
    };
    stream.write_all(&0i32.to_be_bytes()).await?;
    let (input, output) = stream.into_split();
    helper::serve(root, input, output)
        .await
        .map_err(anyhow::Error::from)
}

pub async fn exchange(
    stream: &mut UnixStream,
    channel_id: &str,
    epoch: u64,
    payload: RequestPayloadV1,
) -> Result<ResponseResultV1> {
    match relay_attempt(stream, channel_id, epoch, payload).await? {
        RelayAttempt::Response(response) => Ok(response),
        RelayAttempt::Closed(_) => bail!("relay closed instead of responding"),
    }
}

pub enum RelayAttempt {
    Response(ResponseResultV1),
    Closed(Option<CloseReasonV1>),
}
fn closed_io(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::NotConnected
            | io::ErrorKind::UnexpectedEof
    )
}

pub async fn relay_attempt(
    stream: &mut UnixStream,
    channel_id: &str,
    epoch: u64,
    payload: RequestPayloadV1,
) -> Result<RelayAttempt> {
    let operation = payload.operation();
    let correlation = Uuid::new_v4().to_string();
    let frame = FrameV1::Request(RequestV1 {
        version: 1,
        channel_id: channel_id.into(),
        epoch,
        correlation_id: correlation.clone(),
        payload,
    });
    // The public codec validates and writes the complete length-prefixed frame
    // into bounded memory. No private FrameV1 API or alternate wire grammar.
    let mut encoded = Vec::new();
    FrameWriter::new(&mut encoded)
        .write(&frame)
        .map_err(|_| anyhow!("frame encode"))?;
    if let Err(error) = timeout(Duration::from_secs(15), stream.write_all(&encoded)).await? {
        if closed_io(&error) {
            return Ok(RelayAttempt::Closed(None));
        }
        return Err(error.into());
    }
    let length = match timeout(Duration::from_secs(15), stream.read_u32()).await? {
        Ok(length) => length as usize,
        Err(error) if closed_io(&error) => return Ok(RelayAttempt::Closed(None)),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        length > 0 && length <= channel::MAX_RESPONSE_FRAME_BYTES,
        "response bound"
    );
    let mut bytes = vec![0; length + 4];
    bytes[..4].copy_from_slice(&(length as u32).to_be_bytes());
    timeout(Duration::from_secs(15), stream.read_exact(&mut bytes[4..])).await??;
    match FrameReader::new(bytes.as_slice())
        .read(FrameLimit::Response)
        .map_err(|_| anyhow!("frame decode"))?
    {
        Some(FrameV1::Response(response)) => {
            ensure!(
                response.operation == operation
                    && response.channel_id == channel_id
                    && response.epoch == epoch
                    && response.correlation_id == correlation,
                "response scope/correlation"
            );
            Ok(RelayAttempt::Response(response.result))
        }
        Some(FrameV1::Close(close)) => {
            ensure!(
                close.channel_id == channel_id && close.epoch == epoch,
                "close scope"
            );
            Ok(RelayAttempt::Closed(Some(close.reason)))
        }
        _ => bail!("unexpected public codec frame"),
    }
}

pub async fn worker_terminal(store: &Store, endpoint: &RelayEndpointV1) -> Result<WorkerV1> {
    wait_worker_terminal(store, endpoint, |_| {}).await
}

pub(super) async fn wait_worker_terminal(
    store: &Store,
    endpoint: &RelayEndpointV1,
    mut observe: impl FnMut(&Result<Option<WorkerV1>>),
) -> Result<WorkerV1> {
    timeout(Duration::from_secs(15), async {
        loop {
            let read = store.worker(&endpoint.worker_id);
            observe(&read);
            let worker = read?.context("worker missing")?;
            if worker.state == WorkerStateV1::Terminal && !worker.quota_held {
                return Ok(worker);
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await?
}
