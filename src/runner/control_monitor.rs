//! Post-start read-only observation: no start, claim, or mandatory audit retry.
//! Open epochs survive uncertainty; wall timestamps never restore a deadline.
use super::*;
use std::future::Future;
use tokio::{sync::watch, time::Instant};

#[path = "control_monitor/prestart.rs"]
mod prestart;
pub(super) use prestart::{PrestartOwner, RejectedLaunch};

const WINDOW: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const ARMING_TIMEOUT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(500);
const MAX_INTERVALS: usize = 64;
const MAX_JOURNAL_BYTES: usize = 128 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Transient {
    ConnectionInterrupted,
    Timeout,
    Busy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RefreshFailure {
    Transient(Transient, Option<Duration>),
    Unauthorized,
    Forbidden,
    InvalidResponse,
    IdentityOrRevision,
    Unclassified,
}
impl std::fmt::Display for RefreshFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Transient(Transient::ConnectionInterrupted, _) => {
                "control read transport interrupted"
            }
            Self::Transient(Transient::Timeout, _) => "bounded control read timed out",
            Self::Transient(Transient::Busy, _) => "control read temporarily rate limited",
            Self::Unauthorized => "control read is unauthenticated",
            Self::Forbidden => "control read is forbidden",
            Self::InvalidResponse => "control read response is incomplete or invalid",
            Self::IdentityOrRevision => "control read identity or revision changed",
            Self::Unclassified => "control read failed without a retryable classification",
        })
    }
}
impl std::error::Error for RefreshFailure {}
fn transport_failure(error: &reqwest::Error) -> RefreshFailure {
    if error.is_timeout() {
        return RefreshFailure::Transient(Transient::Timeout, None);
    }
    // is_connect also includes TLS verification failures. Whitelist I/O kinds,
    // never error strings; unknown DNS/TLS/protocol errors remain fatal.
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = cause {
        if let Some(io) = error.downcast_ref::<std::io::Error>()
            && matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::NotConnected
                    | std::io::ErrorKind::UnexpectedEof
            )
        {
            return RefreshFailure::Transient(Transient::ConnectionInterrupted, None);
        }
        cause = error.source();
    }
    RefreshFailure::Unclassified
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransportFailureEnvelope {
    error: crate::control_transport::ControlTransportFailureV1,
}
fn classified_transport_failure(status: reqwest::StatusCode, value: Value) -> RefreshFailure {
    use crate::control_transport::ControlTransportFailureReasonV1 as Reason;
    let Ok(envelope) = serde_json::from_value::<TransportFailureEnvelope>(value) else {
        return RefreshFailure::InvalidResponse;
    };
    if !envelope.error.validate() {
        return RefreshFailure::InvalidResponse;
    }
    let transient = match (status, envelope.error.reason) {
        (reqwest::StatusCode::BAD_GATEWAY, Reason::UpstreamConnectionUnavailable)
        | (reqwest::StatusCode::BAD_GATEWAY, Reason::UpstreamConnectionInterrupted) => {
            Transient::ConnectionInterrupted
        }
        (reqwest::StatusCode::GATEWAY_TIMEOUT, Reason::UpstreamTimeout) => Transient::Timeout,
        _ => return RefreshFailure::InvalidResponse,
    };
    RefreshFailure::Transient(transient, None)
}
async fn read_worker(
    client: &AuditClient,
    expected: &WorkerV1,
) -> std::result::Result<WorkerV1, RefreshFailure> {
    let id =
        Uuid::parse_str(&expected.worker_id).map_err(|_| RefreshFailure::IdentityOrRevision)?;
    if id.is_nil() || id.to_string() != expected.worker_id {
        return Err(RefreshFailure::IdentityOrRevision);
    }
    let url = client
        .control_url(reqwest::Method::GET, &format!("api/v1/workers/{id}"))
        .map_err(|_| RefreshFailure::IdentityOrRevision)?;
    let mut response = client
        .http
        .get(url)
        .bearer_auth(&client.token)
        .send()
        .await
        .map_err(|error| transport_failure(&error))?;
    let status = response.status();
    match status {
        reqwest::StatusCode::OK
        | reqwest::StatusCode::BAD_GATEWAY
        | reqwest::StatusCode::GATEWAY_TIMEOUT => {}
        reqwest::StatusCode::UNAUTHORIZED => return Err(RefreshFailure::Unauthorized),
        reqwest::StatusCode::FORBIDDEN => return Err(RefreshFailure::Forbidden),
        reqwest::StatusCode::TOO_MANY_REQUESTS => {
            // A long delta hint exhausts the fixed window. Wall-clock dates are
            // ignored; no hint or later response can renew the interval.
            let delay = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_secs);
            return Err(RefreshFailure::Transient(Transient::Busy, delay));
        }
        // An audit/storage or unclassified 503 is never retried. Only H's
        // closed, status-consistent transport DTO may qualify below.
        _ => return Err(RefreshFailure::Unclassified),
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| transport_failure(&e))? {
        if bytes.len().saturating_add(chunk.len()) > 512 * 1024 {
            return Err(RefreshFailure::InvalidResponse);
        }
        bytes.extend_from_slice(&chunk);
    }
    let value =
        crate::json::decode_unique(&bytes, 64).map_err(|_| RefreshFailure::InvalidResponse)?;
    if client.redactor.value(&value) != value {
        return Err(RefreshFailure::InvalidResponse);
    }
    if status != reqwest::StatusCode::OK {
        return Err(classified_transport_failure(status, value));
    }
    let worker: WorkerV1 =
        serde_json::from_value(value.clone()).map_err(|_| RefreshFailure::InvalidResponse)?;
    // Serialization equality rejects omitted nullable fields at every level,
    // not only unknown fields. Recovery needs the complete current response.
    if serde_json::to_value(&worker).map_err(|_| RefreshFailure::InvalidResponse)? != value {
        return Err(RefreshFailure::InvalidResponse);
    }
    validate_response(expected, &worker)?;
    Ok(worker)
}
fn worker_hash(worker: &WorkerV1) -> Result<String> {
    content_hash("ai-guard:worker-control-monitor-response:v1", worker)
}
fn validate_response(
    expected: &WorkerV1,
    worker: &WorkerV1,
) -> std::result::Result<(), RefreshFailure> {
    let receipt =
        serde_json::to_value(&worker.receipt).map_err(|_| RefreshFailure::InvalidResponse)?;
    let expected_receipt =
        serde_json::to_value(&expected.receipt).map_err(|_| RefreshFailure::InvalidResponse)?;
    let launch =
        serde_json::to_value(&worker.launch).map_err(|_| RefreshFailure::InvalidResponse)?;
    let expected_launch =
        serde_json::to_value(&expected.launch).map_err(|_| RefreshFailure::InvalidResponse)?;
    let immutable = worker.worker_id == expected.worker_id
        && worker.admission_operation_id == expected.admission_operation_id
        && worker.request_id == expected.request_id
        && worker.spec_hash == expected.spec_hash
        && worker.policy_hash == expected.policy_hash
        && worker.attempt == expected.attempt
        && receipt == expected_receipt
        && launch == expected_launch;
    if !immutable
        || worker.allocation_inconsistent
        || worker.revision == 0
        || !matches!(
            worker.state,
            WorkerStateV1::Running | WorkerStateV1::Stopping | WorkerStateV1::Terminal
        )
        || worker.revision < expected.revision
        || (worker.revision == expected.revision
            && worker_hash(worker).map_err(|_| RefreshFailure::InvalidResponse)?
                != worker_hash(expected).map_err(|_| RefreshFailure::InvalidResponse)?)
    {
        return Err(RefreshFailure::IdentityOrRevision);
    }
    if worker.state == WorkerStateV1::Terminal && !consistent_terminal(worker) {
        return Err(RefreshFailure::InvalidResponse);
    }
    Ok(())
}

// Match the existing Store terminal contract defensively. This does not prove
// that the runtime is stopped locally or release quota on the client's behalf.
fn consistent_terminal(worker: &WorkerV1) -> bool {
    let Some(observed) = &worker.last_observation else {
        return false;
    };
    let Some(created) = &worker.attempt.created_inspection else {
        return false;
    };
    let inspection = &observed.inspection;
    if worker.quota_held
        || worker.unknown_reason.is_some()
        || worker.attempt.start_operation_id.is_none()
        || observed.restart_count != Some(0)
        || observed.running != Some(false)
        || !matches!(
            observed.status,
            RuntimeStatusV1::Exited | RuntimeStatusV1::Dead
        )
        || observed.exit_code.is_none()
        || inspection != created
        || inspection.authority_id != worker.launch.authority_id
        || inspection.runtime_kind != worker.launch.runtime_kind
        || Some(inspection.container_id.as_str()) != worker.attempt.container_id.as_deref()
        || inspection.stable_name != worker.attempt.stable_name
        || inspection.volume_name != worker.attempt.volume_name
        || inspection.image_id != worker.launch.image_id
        || inspection.spec_hash != worker.spec_hash
        || inspection.cpu_millis != worker.launch.cpu_millis
        || inspection.memory_bytes != worker.launch.memory_bytes
        || inspection.pids_limit != worker.launch.pids_limit
        || inspection.network != worker.launch.network
        || inspection.security_preset != worker.launch.security_preset
        || inspection.restart_policy != worker.launch.restart_policy
    {
        return false;
    }
    let parse = |date: &str| {
        if date.len() > 64 {
            None
        } else {
            chrono::DateTime::parse_from_rfc3339(date).ok()
        }
    };
    let (Some(started), Some(finished), Some(first_started), Some(_)) = (
        observed.started_at.as_deref().and_then(parse),
        observed.finished_at.as_deref().and_then(parse),
        worker.observed_started_at.as_deref().and_then(parse),
        parse(&observed.observed_at),
    ) else {
        return false;
    };
    started.timestamp() > 0
        && first_started == started
        && finished >= started
        && (finished - started)
            .num_nanoseconds()
            .is_some_and(|duration| duration >= 0)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recovery {
    request_id: Uuid,
    request_sequence: u64,
    worker_revision: u64,
    worker_hash: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DegradedInterval {
    interval_id: Uuid,
    first_request_id: Uuid,
    first_request_sequence: u64,
    request_started_at: String,
    cause: Transient,
    recovery: Option<Recovery>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum JournalAction {
    Degraded {
        interval: DegradedInterval,
    },
    Recovered {
        interval_id: Uuid,
        recovery: Recovery,
    },
    ExecutionCompleted,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteIntent {
    operation_id: Uuid,
    action: JournalAction,
}
impl WriteIntent {
    fn new(action: JournalAction) -> Self {
        Self {
            operation_id: Uuid::new_v4(),
            action,
        }
    }
    fn hash(&self) -> Result<String> {
        content_hash("ai-guard:worker-control-monitor-write:v1", self)
    }
}
/// Preparation and commit retain one operation identity. Cancellation never
/// means rollback; the write task owns its intent and reopening an epoch stops.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalV1 {
    version: u32,
    context_hash: String,
    epoch_id: Uuid,
    revision: u64,
    operation_id: Uuid,
    operation_hash: String,
    open: bool,
    pending: Option<WriteIntent>,
    intervals: Vec<DegradedInterval>,
}
impl JournalV1 {
    fn validate(&self, context: &str) -> Result<()> {
        anyhow::ensure!(
            self.version == 1
                && self.context_hash == context
                && !self.epoch_id.is_nil()
                && !self.operation_id.is_nil()
                && self.revision > 0
                && self.intervals.len() <= MAX_INTERVALS
                && crate::control::hash(&self.context_hash).is_ok()
                && crate::control::hash(&self.operation_hash).is_ok()
                && (self.open || self.pending.is_none())
                && serde_json::to_vec(self)?.len() <= MAX_JOURNAL_BYTES,
            "control monitor journal is invalid or bound to another attempt"
        );
        if let Some(pending) = &self.pending {
            anyhow::ensure!(
                pending.operation_id == self.operation_id && pending.hash()? == self.operation_hash,
                "control monitor journal pending identity differs"
            );
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut last_sequence = 0;
        let mut floor = 0;
        for (index, interval) in self.intervals.iter().enumerate() {
            anyhow::ensure!(
                !interval.interval_id.is_nil()
                    && ids.insert(interval.interval_id)
                    && !interval.first_request_id.is_nil()
                    && interval.first_request_sequence > last_sequence
                    && interval.request_started_at.len() <= 64
                    && chrono::DateTime::parse_from_rfc3339(&interval.request_started_at).is_ok(),
                "control monitor interval is invalid"
            );
            last_sequence = interval.first_request_sequence;
            if let Some(recovery) = &interval.recovery {
                anyhow::ensure!(
                    !recovery.request_id.is_nil()
                        && recovery.request_sequence > last_sequence
                        && recovery.worker_revision >= floor
                        && recovery.worker_revision > 0
                        && crate::control::hash(&recovery.worker_hash).is_ok(),
                    "control monitor recovery is invalid"
                );
                floor = recovery.worker_revision;
                last_sequence = recovery.request_sequence;
            } else {
                anyhow::ensure!(
                    index + 1 == self.intervals.len(),
                    "unfinished control interval is not last"
                );
            }
        }
        Ok(())
    }
    fn apply(&mut self, action: &JournalAction) -> Result<()> {
        anyhow::ensure!(
            self.open,
            "a closed control monitor epoch cannot be renewed"
        );
        match action {
            JournalAction::Degraded { interval } => {
                anyhow::ensure!(
                    self.intervals.len() < MAX_INTERVALS
                        && self.intervals.last().is_none_or(|v| v.recovery.is_some()),
                    "control monitor journal capacity or interval conflict"
                );
                self.intervals.push(interval.clone());
            }
            JournalAction::Recovered {
                interval_id,
                recovery,
            } => {
                let interval = self
                    .intervals
                    .last_mut()
                    .context("missing degraded interval")?;
                anyhow::ensure!(
                    interval.interval_id == *interval_id && interval.recovery.is_none(),
                    "control monitor recovery conflicts with its degraded interval"
                );
                interval.recovery = Some(recovery.clone());
            }
            // Only the execution future ended: no terminal or quota is inferred.
            JournalAction::ExecutionCompleted => self.open = false,
        }
        Ok(())
    }
}
struct MonitorJournal {
    directory: Directory,
    name: String,
    context: String,
    current: Mutex<JournalV1>,
}
impl MonitorJournal {
    fn create(config: &Path, session: Uuid, context: String) -> Result<Arc<Self>> {
        let directory = Directory::root(config)?.child("worker-control-monitor")?;
        let name = format!("{session}.json");
        anyhow::ensure!(
            directory
                .read_bounded::<JournalV1>(&name, MAX_JOURNAL_BYTES)?
                .is_none(),
            "a prior control monitor epoch requires reconciliation; no fresh window was granted"
        );
        let epoch_id = Uuid::new_v4();
        let current = JournalV1 {
            version: 1,
            context_hash: context.clone(),
            epoch_id,
            revision: 1,
            operation_id: epoch_id,
            operation_hash: content_hash("ai-guard:worker-control-monitor-open:v1", &epoch_id)?,
            open: true,
            pending: None,
            intervals: Vec::new(),
        };
        current.validate(&context)?;
        directory.write(&name, &current)?;
        Ok(Arc::new(Self {
            directory,
            name,
            context,
            current: Mutex::new(current),
        }))
    }
    fn write(&self, intent: WriteIntent) -> Result<()> {
        let mut current = self
            .current
            .lock()
            .map_err(|_| anyhow::anyhow!("control monitor journal lock poisoned"))?;
        let stored = self
            .directory
            .read_bounded::<JournalV1>(&self.name, MAX_JOURNAL_BYTES)?
            .context("control monitor epoch disappeared")?;
        stored.validate(&self.context)?;
        anyhow::ensure!(
            content_hash("ai-guard:worker-control-monitor-state:v1", &stored)?
                == content_hash("ai-guard:worker-control-monitor-state:v1", &*current)?,
            "control monitor journal changed outside its owner"
        );
        let hash = intent.hash()?;
        if current.operation_id == intent.operation_id {
            anyhow::ensure!(
                current.operation_hash == hash,
                "control monitor write identity has conflicting content"
            );
            if current.pending.is_none() {
                return Ok(());
            }
        } else {
            anyhow::ensure!(
                current.pending.is_none(),
                "a control monitor write remains unconfirmed"
            );
            let mut prepared = current.clone();
            prepared.revision = prepared
                .revision
                .checked_add(1)
                .context("control monitor revision exhausted")?;
            prepared.operation_id = intent.operation_id;
            prepared.operation_hash = hash;
            prepared.pending = Some(intent.clone());
            let mut candidate = prepared.clone();
            candidate.pending = None;
            candidate.apply(&intent.action)?;
            candidate.validate(&self.context)?;
            prepared.validate(&self.context)?;
            self.directory.write(&self.name, &prepared)?;
            *current = prepared;
        }
        let mut candidate = current.clone();
        candidate.pending = None;
        candidate.apply(&intent.action)?;
        candidate.validate(&self.context)?;
        self.directory.write(&self.name, &candidate)?;
        *current = candidate;
        Ok(())
    }
    async fn append(self: &Arc<Self>, intent: WriteIntent) -> Result<()> {
        let journal = self.clone();
        // Dropping this handle does not cancel the write. Its exact operation
        // remains owned, while the open epoch forbids a fresh monitor on resume.
        tokio::task::spawn_blocking(move || journal.write(intent)).await??;
        Ok(())
    }
}
fn context_hash(state: &PlanState) -> Result<String> {
    let plan = state.snapshot()?;
    let checkpoint = managed(&plan)?;
    let worker = state.worker()?;
    content_hash(
        "ai-guard:worker-control-monitor-context:v1",
        &json!({
            "tenant": checkpoint.tenant_id, "origin": checkpoint.gateway_origin,
            "config": checkpoint.config_root, "session": plan.session, "operation": plan.operation_id,
            "worker": worker.worker_id, "attempt": worker.attempt, "launch": worker.launch,
            "spec_hash": worker.spec_hash, "binding": checkpoint.binding,
            "owned": checkpoint.owned, "lock_identity": checkpoint.lock_identity,
        }),
    )
}

fn local_bound_session(
    session: Uuid,
    config: &Path,
    gateway: &str,
) -> Result<(PlanState, AuditClient)> {
    let root =
        crate::control_fs::Directory::open_existing(&crate::control_fs::absolute_path(config)?)?;
    let config = root.path().to_path_buf();
    let directory = Directory::root(&config)?.child("sessions")?;
    let plan: RunPlan = directory
        .read(&format!("{session}.json"))?
        .context("session checkpoint not found")?;
    anyhow::ensure!(
        plan.session == session && plan.options.config_dir == config,
        "session identity or configuration root differs from its private checkpoint"
    );
    let client = AuditClient::new(gateway, &read_token(&config.join("secrets/runner"))?)?;
    validate_checkpoint(&plan, &client)?;
    let checkpoint = managed(&plan)?;
    anyhow::ensure!(
        !checkpoint.tenant_id.is_empty(),
        "private session has no tenant binding"
    );
    client.validate_identity_payload(
        &json!({"tenant_id":checkpoint.tenant_id,"gateway_origin":checkpoint.gateway_origin}),
    )?;
    let state = PlanState {
        plan: Arc::new(Mutex::new(plan)),
        directory: Arc::new(directory),
        name: format!("{session}.json"),
    };
    if let Some(worker) = state.checkpoint()?.worker {
        validate_worker(&state.snapshot()?, &worker)?;
        client.validate_identity_payload(&worker)?;
    }
    Ok((state, client))
}

/// This port can only stop an already-consumed, exact locally bound attempt.
/// It grants no RPC identity, start, adoption, mutation, or quota release. The
/// ordinary authenticated resume path still owns terminal reconciliation.
pub(super) async fn stop_interrupted_before_gateway(
    session: Uuid,
    config: &Path,
    gateway: &str,
    program: Option<&Path>,
    adopt: bool,
    interactive: bool,
) -> Result<Option<Value>> {
    let (initial, _) = local_bound_session(session, config, gateway)?;
    let initial_plan = initial.snapshot()?;
    anyhow::ensure!(
        !interactive || initial_plan.options.interactive,
        "--interactive cannot convert an admitted batch session"
    );
    let Some(worker) = initial.checkpoint()?.worker else {
        return Ok(None);
    };
    if worker.attempt.start_operation_id.is_none()
        || [WorkerStateV1::Terminal, WorkerStateV1::Cancelled].contains(&worker.state)
    {
        return Ok(None);
    }
    let guard = lock_attempt(&initial)?;
    // Reload under the same inode lock before trusting any target. Do not
    // rewrite the checkpoint or copy another authority into this attempt.
    let (state, client) = local_bound_session(session, config, gateway)?;
    let plan = state.snapshot()?;
    let checkpoint = managed(&plan)?;
    anyhow::ensure!(
        checkpoint.lock_identity == Some(guard.identity())
            && context_hash(&initial)? == context_hash(&state)?,
        "local stop binding changed while acquiring attempt ownership"
    );
    let worker = state.worker()?;
    if [WorkerStateV1::Terminal, WorkerStateV1::Cancelled].contains(&worker.state) {
        return Ok(None);
    }
    anyhow::ensure!(
        worker.quota_held && worker.attempt.start_operation_id.is_some(),
        "local stop requires a previously consumed, bound attempt"
    );
    let owned = checkpoint
        .owned
        .clone()
        .context("local stop has no immutable owned snapshot; no target can be safely inferred")?;
    let binding = checkpoint
        .binding
        .clone()
        .context("local stop has no persisted runtime authority")?;
    anyhow::ensure!(
        worker.attempt.container_id.as_deref() == Some(owned.handle.id())
            && worker.attempt.created_inspection.is_some(),
        "local stop identity/configuration proof is incomplete"
    );
    if adopt {
        // A positively validated replacement selects the EXISTING authenticated
        // adoption path. Do not catch failures of private identity/FS checks or
        // reopen another engine locally. That path must authenticate, verify
        // the same engine and commit its original adoption event before effects.
        let entry = program.or_else(|| {
            binding
                .invocation
                .as_ref()
                .map(|invocation| invocation.approved_entry.as_path())
        });
        let candidate = approved_runtime(&plan.options, entry)?;
        if binding
            .invocation
            .as_ref()
            .is_none_or(|invocation| candidate.invocation() != invocation)
        {
            return Ok(None);
        }
    }
    anyhow::ensure!(
        binding.invocation.is_some(),
        "local stop invocation is unbound; an authenticated adoption is required"
    );
    anyhow::ensure!(
        checkpoint.invocation_event.is_none(),
        "local stop cannot bypass a pending authenticated invocation adoption"
    );
    // The STOP-only port always uses the identical persisted invocation.
    let runtime = reopen_authority(&state, &client, binding, program, false).await?;
    let recovered = recover_owned(&runtime, &state)
        .await?
        .context("local owned snapshot disappeared")?;
    anyhow::ensure!(
        recovered.handle.id() == owned.handle.id(),
        "local stop recovered another instance"
    );
    let observed = runtime.observe(&owned.handle).await?;
    observed.validate_identity_allocation(&owned.observation)?;
    if observed.process.running != Some(true) {
        return Ok(None);
    }
    guard.ensure_current()?;
    // The stop starts before any journal read or gateway request can block it.
    let (stopped, _) = tokio::join!(
        runtime.stop_snapshot_owned(&owned),
        require_stop_on_active_resume(&state)
    );
    let status = match stopped {
        Ok(stopped) => action_status(
            &state,
            Some(&stopped),
            "The interrupted monitor's original lifetime cannot be restored. The exact locally bound instance was observed not running after a targeted stop. Gateway authentication, terminal accounting and quota remain unreconciled; resume again when the gateway is available.",
        )?,
        Err(_) => action_status(
            &state,
            Some(&observed),
            "The interrupted monitor's original lifetime cannot be restored. A targeted stop of the exact locally bound instance was requested but is unconfirmed. Gateway authentication, terminal accounting and quota remain unreconciled; no fresh window or start was granted.",
        )?,
    };
    Ok(Some(status))
}
/// An active process after restart has no portable original monotonic deadline.
/// An open, absent, or unverifiable epoch must stop, even after a healthy poll.
/// A closed epoch also never authorizes renewing an already-ended execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResumeBlock {
    Absent,
    Unverifiable,
    Unfinished,
    Completed,
}
impl std::fmt::Display for ResumeBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Absent => "active resume has no verifiable control monitor epoch",
            Self::Unverifiable => "active resume control monitor epoch is invalid or unavailable",
            Self::Unfinished => "active resume found an unfinished control monitor epoch",
            Self::Completed => "a completed control monitor epoch cannot renew the original lifetime of a still-running instance",
        })
    }
}
impl std::error::Error for ResumeBlock {}
fn resume_block(config: &Path, session: Uuid, context: &str) -> ResumeBlock {
    let read = || -> Result<Option<JournalV1>> {
        Directory::root(config)?
            .child("worker-control-monitor")?
            .read_bounded(&format!("{session}.json"), MAX_JOURNAL_BYTES)
    };
    match read() {
        Ok(None) => ResumeBlock::Absent,
        Err(_) => ResumeBlock::Unverifiable,
        Ok(Some(record)) if record.validate(context).is_err() => ResumeBlock::Unverifiable,
        Ok(Some(record))
            if record.open
                || record.pending.is_some()
                || record
                    .intervals
                    .iter()
                    .any(|interval| interval.recovery.is_none()) =>
        {
            ResumeBlock::Unfinished
        }
        Ok(Some(_)) => ResumeBlock::Completed,
    }
}
pub(super) async fn require_stop_on_active_resume(state: &PlanState) -> Result<()> {
    let plan = state.snapshot()?;
    let context = context_hash(state)?;
    let read = tokio::task::spawn_blocking(move || {
        resume_block(&plan.options.config_dir, plan.session, &context)
    });
    let reason = tokio::time::timeout(Duration::from_secs(1), read)
        .await
        .context("control monitor epoch read remained unconfirmed")??;
    Err(reason.into())
}
#[derive(Clone, Copy)]
struct Policy {
    window: Duration,
    request_timeout: Duration,
    poll: Duration,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            window: WINDOW,
            request_timeout: REQUEST_TIMEOUT,
            poll: POLL,
        }
    }
}
struct Observation {
    stopping: bool,
    revision: u64,
    hash: String,
}
struct RetryWindow {
    deadline: Instant,
    interval_id: Uuid,
}
impl RetryWindow {
    fn start(request_started: Instant, original: Instant, duration: Duration, id: Uuid) -> Self {
        Self {
            deadline: request_started
                .checked_add(duration)
                .unwrap_or(request_started)
                .min(original),
            interval_id: id,
        }
    }
}
enum Wake<T> {
    Value(T),
    Completed,
    Cancelled,
    Stop,
    Deadline,
}
async fn interruptible<F, K>(
    future: F,
    completed: &mut watch::Receiver<bool>,
    deadline: Instant,
    known_stop: &K,
) -> Result<Wake<F::Output>>
where
    F: Future,
    K: Fn() -> Result<bool>,
{
    tokio::pin!(future);
    let mut check = tokio::time::interval(Duration::from_millis(50));
    loop {
        if *completed.borrow() {
            return Ok(Wake::Completed);
        }
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => return Ok(Wake::Deadline),
            changed = completed.changed() => {
                if changed.is_err() { return Ok(if *completed.borrow() { Wake::Completed } else { Wake::Cancelled }); }
                if *completed.borrow() { return Ok(Wake::Completed); }
            },
            _ = check.tick() => { if known_stop()? { return Ok(Wake::Stop); } },
            value = &mut future => return Ok(Wake::Value(value)),
        }
    }
}
#[allow(clippy::too_many_arguments)]
async fn observe_until_complete<R, RFut, S, SFut, J, JFut, K>(
    mut completed: watch::Receiver<bool>,
    original_deadline: Instant,
    policy: Policy,
    mut refresh: R,
    mut stop_owned: S,
    mut journal: J,
    known_stop: K,
) -> Result<()>
where
    R: FnMut() -> RFut,
    RFut: Future<Output = std::result::Result<Observation, RefreshFailure>>,
    S: FnMut() -> SFut,
    SFut: Future<Output = Result<()>>,
    J: FnMut(WriteIntent) -> JFut,
    JFut: Future<Output = Result<()>>,
    K: Fn() -> Result<bool>,
{
    let result = async {
        let mut window: Option<RetryWindow> = None;
        let mut sequence = 0u64;
        let mut next_poll = Instant::now();
        loop {
            let deadline = window.as_ref().map_or(original_deadline, |v| v.deadline);
            match interruptible(tokio::time::sleep_until(next_poll), &mut completed, deadline, &known_stop).await? {
                Wake::Value(()) => {}, Wake::Completed => break,
                Wake::Stop => bail!("control stop requested"),
                Wake::Cancelled => bail!("control monitor cancelled; execution result remains unknown"),
                Wake::Deadline => bail!("control observation window or original execution deadline expired"),
            }
            sequence = sequence.checked_add(1).context("control monitor request sequence exhausted")?;
            let request_id = Uuid::new_v4();
            let started = Instant::now();
            let wall = chrono::Utc::now().to_rfc3339();
            // Sample BEFORE awaiting the first failing request. Its timeout is
            // part of the window, never extra time added to sixty seconds.
            let request_deadline = started.checked_add(policy.request_timeout.min(policy.window)).unwrap_or(started).min(deadline);
            let response = match interruptible(refresh(), &mut completed, request_deadline, &known_stop).await? {
                Wake::Value(v) => v, Wake::Completed => break,
                Wake::Stop => bail!("control stop requested"),
                Wake::Cancelled => bail!("control monitor cancelled; execution result remains unknown"),
                Wake::Deadline if Instant::now() < deadline => Err(RefreshFailure::Transient(Transient::Timeout, None)),
                Wake::Deadline => bail!("control observation window or original execution deadline expired"),
            };
            match response {
                Ok(observed) if observed.stopping => {
                    stop_owned().await?;
                    match interruptible(std::future::pending::<()>(), &mut completed, deadline, &|| Ok(false)).await? {
                        Wake::Completed => break,
                        _ => bail!("targeted stop requested; execution completion remains unknown"),
                    }
                },
                Ok(observed) => {
                    if let Some(active) = &window {
                        let intent = WriteIntent::new(JournalAction::Recovered { interval_id: active.interval_id,
                            recovery: Recovery { request_id, request_sequence: sequence, worker_revision: observed.revision, worker_hash: observed.hash } });
                        match interruptible(journal(intent), &mut completed, deadline, &known_stop).await? {
                            Wake::Value(result) => result?,
                            _ => bail!("control recovery write remained unconfirmed; its identity is retained, not rolled back"),
                        }
                        window = None;
                    }
                    next_poll = Instant::now().checked_add(policy.poll).unwrap_or(original_deadline);
                },
                Err(RefreshFailure::Transient(cause, retry_after)) => {
                    if window.is_none() {
                        let interval_id = Uuid::new_v4();
                        let active = RetryWindow::start(started, original_deadline, policy.window, interval_id);
                        let intent = WriteIntent::new(JournalAction::Degraded { interval: DegradedInterval {
                            interval_id, first_request_id: request_id, first_request_sequence: sequence,
                            request_started_at: wall, cause, recovery: None } });
                        match interruptible(journal(intent), &mut completed, active.deadline, &known_stop).await? {
                            Wake::Value(result) => result?,
                            _ => bail!("control degraded write remained unconfirmed; its identity is retained, not rolled back"),
                        }
                        window = Some(active);
                    }
                    let delay = retry_after.unwrap_or(policy.poll).max(policy.poll);
                    next_poll = Instant::now().checked_add(delay).unwrap_or(original_deadline);
                },
                Err(failure) => return Err(failure.into()),
            }
        }
        // No earlier write is outstanding. Start the exact closure once, but
        // never hold execution completion behind storage. append() transfers
        // ownership of its intent to the blocking writer on its first poll;
        // dropping this waiter is not rollback. An unconfirmed epoch stays
        // conservative at reopen, and Ok here does not claim durable closure.
        let close = WriteIntent::new(JournalAction::ExecutionCompleted);
        let close = journal(close);
        tokio::pin!(close);
        std::future::poll_fn(|cx| match close.as_mut().poll(cx) {
            std::task::Poll::Ready(result) => std::task::Poll::Ready(result),
            std::task::Poll::Pending => std::task::Poll::Ready(Ok(())),
        }).await
    }.await;
    if result.is_err() {
        let _ = stop_owned().await;
    }
    result
}
// The durable epoch must arm promptly after ACK even before any GET begins.
// Queueing/creation/fsync all count from the sample before launching the writer.
async fn await_epoch_creation<F, S, SFut, K>(
    create: F,
    completed: &mut watch::Receiver<bool>,
    arming_started: Instant,
    original_deadline: Instant,
    known_stop: &K,
    stop_owned: &S,
) -> Result<Arc<MonitorJournal>>
where
    F: Future<Output = Result<Arc<MonitorJournal>>>,
    S: Fn() -> SFut,
    SFut: Future<Output = Result<()>>,
    K: Fn() -> Result<bool>,
{
    let arming_deadline = arming_started
        .checked_add(ARMING_TIMEOUT)
        .unwrap_or(arming_started)
        .min(original_deadline);
    let result = match interruptible(create, completed, arming_deadline, known_stop).await {
        Ok(Wake::Value(result)) => result,
        Ok(_) => Err(anyhow::anyhow!(
            "control monitor epoch creation remained unconfirmed"
        )),
        Err(error) => Err(error),
    };
    if result.is_err() {
        let _ = stop_owned().await;
    }
    result
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    runtime: &ManagedRuntime,
    client: &AuditClient,
    state: &PlanState,
    guard: &AttemptGuard,
    completed: watch::Receiver<bool>,
    started: watch::Receiver<bool>,
    original_deadline: Instant,
    owner: Arc<PrestartOwner>,
) -> std::result::Result<(), prestart::ControlFailure> {
    if let Err(error) = owner.require_context(runtime, state, guard) {
        let failure = owner.revoke(error);
        owner.retain_stop_until_completed(completed).await;
        return Err(failure);
    }
    let _lease = match owner.own_monitor() {
        Ok(lease) => lease,
        Err(error) => {
            let failure = owner.revoke(error);
            owner.retain_stop_until_completed(completed).await;
            return Err(failure);
        }
    };
    let result = run_observation(
        client,
        state,
        guard,
        completed.clone(),
        started,
        original_deadline,
        &owner,
    )
    .await;
    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            // Revoke synchronously BEFORE any stop await. The same observer
            // remains the stop owner even if a start was already admitted.
            let failure = owner.revoke(error);
            owner.retain_stop_until_completed(completed).await;
            Err(failure)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_observation(
    client: &AuditClient,
    state: &PlanState,
    guard: &AttemptGuard,
    mut completed: watch::Receiver<bool>,
    mut started: watch::Receiver<bool>,
    original_deadline: Instant,
    owner: &Arc<PrestartOwner>,
) -> Result<()> {
    let stop_owned = || owner.stop_owned();
    let known_stop = || Ok(owner.revoked() || state.worker()?.state == WorkerStateV1::Stopping);
    let prepare = async {
        // A read begun before both acknowledgements never gains tolerance.
        while !*started.borrow() {
            let read = async { refresh_readonly(client, state).await };
            match interruptible(read, &mut completed, original_deadline.min(Instant::now() + REQUEST_TIMEOUT), &known_stop).await? {
                Wake::Value(result) => { result?; owner.confirm_read()?; }, Wake::Completed => return Ok(None),
                _ => bail!("control observation before start acknowledgement failed or was interrupted"),
            }
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(original_deadline) => bail!("original execution deadline expired before start acknowledgement"),
                changed = completed.changed() => { if changed.is_err() || *completed.borrow() { return Ok(None); } },
                changed = started.changed() => { changed.context("start acknowledgement owner disappeared")?; },
                _ = tokio::time::sleep(POLL) => {},
            }
        }
        guard.ensure_current()?;
        anyhow::ensure!(state.worker()?.state == WorkerStateV1::Running,
            "tolerant control observation requires acknowledged Running state");
        let plan = state.snapshot()?;
        let context = context_hash(state)?;
        Ok(Some((plan, context)))
    }.await;
    let (plan, context) = match prepare {
        Ok(Some(prepared)) => prepared,
        Ok(None) => return Ok(()),
        Err(error) => return Err(error),
    };
    let arming_started = Instant::now();
    let create = tokio::task::spawn_blocking(move || {
        MonitorJournal::create(&plan.options.config_dir, plan.session, context)
    });
    let journal = await_epoch_creation(
        async { create.await? },
        &mut completed,
        arming_started,
        original_deadline,
        &known_stop,
        &stop_owned,
    )
    .await?;
    observe_until_complete(
        completed,
        original_deadline,
        Policy::default(),
        || async {
            guard
                .ensure_current()
                .map_err(|_| RefreshFailure::IdentityOrRevision)?;
            let expected = state
                .worker()
                .map_err(|_| RefreshFailure::IdentityOrRevision)?;
            let worker = read_worker(client, &expected).await?;
            // Reject a late response below either the request's or latest floor.
            let current = state
                .worker()
                .map_err(|_| RefreshFailure::IdentityOrRevision)?;
            validate_response(&current, &worker)?;
            validate_worker(
                &state
                    .snapshot()
                    .map_err(|_| RefreshFailure::IdentityOrRevision)?,
                &worker,
            )
            .map_err(|_| RefreshFailure::IdentityOrRevision)?;
            let hash = worker_hash(&worker).map_err(|_| RefreshFailure::InvalidResponse)?;
            let observed = Observation {
                stopping: worker.state == WorkerStateV1::Stopping,
                revision: worker.revision,
                hash,
            };
            state
                .read_worker(worker)
                .map_err(|_| RefreshFailure::IdentityOrRevision)?;
            Ok(observed)
        },
        stop_owned,
        |intent| journal.append(intent),
        known_stop,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    fn private_root() -> tempfile::TempDir {
        tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap()
    }
    fn journal(root: &tempfile::TempDir) -> Arc<MonitorJournal> {
        MonitorJournal::create(
            &root.path().canonicalize().unwrap(),
            Uuid::from_u128(1),
            "1".repeat(64),
        )
        .unwrap()
    }
    fn record(journal: &MonitorJournal) -> JournalV1 {
        journal.directory.read(&journal.name).unwrap().unwrap()
    }
    // Completion itself must not wait for storage. Independently observe the
    // already-owned close writer when a test needs its durable final record.
    async fn closed_record(journal: &MonitorJournal) -> JournalV1 {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let saved = record(journal);
                if !saved.open {
                    return saved;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("owned epoch closure did not become durable")
    }
    fn healthy() -> Observation {
        Observation {
            stopping: false,
            revision: 7,
            hash: "2".repeat(64),
        }
    }
    fn policy() -> Policy {
        Policy {
            poll: Duration::from_millis(1),
            ..Policy::default()
        }
    }
    fn degraded() -> WriteIntent {
        WriteIntent::new(JournalAction::Degraded {
            interval: DegradedInterval {
                interval_id: Uuid::from_u128(20),
                first_request_id: Uuid::from_u128(21),
                first_request_sequence: 1,
                request_started_at: "2026-11-07T00:00:00Z".into(),
                cause: Transient::Busy,
                recovery: None,
            },
        })
    }
    fn recovered() -> WriteIntent {
        WriteIntent::new(JournalAction::Recovered {
            interval_id: Uuid::from_u128(20),
            recovery: Recovery {
                request_id: Uuid::from_u128(22),
                request_sequence: 2,
                worker_revision: 7,
                worker_hash: "2".repeat(64),
            },
        })
    }

    // Same regression as the original RED: the first transient read must not
    // stop. A third read ends execution after the second read recovered durably.
    #[tokio::test]
    async fn short_transient_refresh_does_not_stop_owned_worker() {
        let root = private_root();
        let journal = journal(&root);
        let (done, completed) = watch::channel(false);
        let reads = AtomicUsize::new(0);
        let stops = AtomicUsize::new(0);
        let result = observe_until_complete(
            completed,
            Instant::now() + Duration::from_secs(5),
            policy(),
            || {
                let call = reads.fetch_add(1, Ordering::SeqCst);
                let done = done.clone();
                async move {
                    if call == 0 {
                        Err(RefreshFailure::Transient(
                            Transient::ConnectionInterrupted,
                            None,
                        ))
                    } else {
                        if call == 2 {
                            let _ = done.send(true);
                        }
                        Ok(healthy())
                    }
                }
            },
            || async {
                stops.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |intent| journal.append(intent),
            || Ok(false),
        )
        .await;
        assert!(
            result.is_ok(),
            "a recovered read must not terminate monitoring: {result:?}"
        );
        assert_eq!(reads.load(Ordering::SeqCst), 3);
        assert_eq!(stops.load(Ordering::SeqCst), 0);
        let saved = closed_record(&journal).await;
        assert!(!saved.open);
        assert!(saved.pending.is_none());
        assert_eq!(saved.intervals.len(), 1);
        assert_eq!(
            saved.intervals[0]
                .recovery
                .as_ref()
                .unwrap()
                .request_sequence,
            2
        );
        assert_eq!(
            saved.intervals[0]
                .recovery
                .as_ref()
                .unwrap()
                .worker_revision,
            7
        );
    }
    #[tokio::test]
    async fn completion_preempts_a_blocked_refresh() {
        let root = private_root();
        let journal = journal(&root);
        let (done, completed) = watch::channel(false);
        let stops = AtomicUsize::new(0);
        let monitor = observe_until_complete(
            completed,
            Instant::now() + Duration::from_secs(30),
            policy(),
            || {
                let done = done.clone();
                async move {
                    let _ = done.send(true);
                    std::future::pending::<std::result::Result<Observation, RefreshFailure>>().await
                }
            },
            || async {
                stops.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |intent| journal.append(intent),
            || Ok(false),
        );
        let result = tokio::time::timeout(Duration::from_secs(5), monitor).await;
        assert!(
            result.is_ok(),
            "a pending read must not hold completion hostage"
        );
        assert!(result.unwrap().is_ok());
        assert_eq!(stops.load(Ordering::SeqCst), 0);
        assert!(!closed_record(&journal).await.open);
    }

    // Review regression: only epoch closure is blocked. done=true must release
    // the execution/monitor join instead of waiting its original thirty seconds.
    // The retained close may still commit afterwards with the same identity.
    #[tokio::test]
    async fn completed_execution_preempts_blocked_epoch_closure_and_retains_write_identity() {
        let root = private_root();
        let journal = journal(&root);
        let (done, completed) = watch::channel(false);
        let retained = Mutex::new(None);
        let stops = AtomicUsize::new(0);
        let monitor = observe_until_complete(
            completed,
            Instant::now() + Duration::from_secs(30),
            policy(),
            || {
                let done = done.clone();
                async move {
                    let _ = done.send(true);
                    std::future::pending::<std::result::Result<Observation, RefreshFailure>>().await
                }
            },
            || async {
                stops.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |intent| {
                assert!(matches!(intent.action, JournalAction::ExecutionCompleted));
                *retained.lock().unwrap() = Some(intent);
                std::future::pending::<Result<()>>()
            },
            || Ok(false),
        );
        let returned = tokio::time::timeout(Duration::from_secs(5), monitor).await;
        let intent = retained
            .lock()
            .unwrap()
            .take()
            .expect("epoch closure was not attempted");
        let operation_id = intent.operation_id;
        assert!(!operation_id.is_nil());
        assert!(
            record(&journal).open,
            "an unconfirmed close is not a durable closure"
        );
        // A cancelled waiter does not prove rollback: the exact outstanding
        // operation is deliberately committed before the final RED assertion.
        journal.write(intent).unwrap();
        let committed = record(&journal);
        assert_eq!(committed.operation_id, operation_id);
        assert!(!committed.open);
        assert!(
            returned.is_ok(),
            "epoch closure held execution completion until its original deadline"
        );
    }
    #[tokio::test]
    async fn observed_stop_targets_owned_worker_immediately() {
        let root = private_root();
        let journal = journal(&root);
        let (done, completed) = watch::channel(false);
        let stops = AtomicUsize::new(0);
        observe_until_complete(
            completed,
            Instant::now() + Duration::from_secs(5),
            policy(),
            || async {
                Ok(Observation {
                    stopping: true,
                    ..healthy()
                })
            },
            || async {
                stops.fetch_add(1, Ordering::SeqCst);
                let _ = done.send(true);
                Ok(())
            },
            |intent| journal.append(intent),
            || Ok(false),
        )
        .await
        .unwrap();
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        assert!(!closed_record(&journal).await.open);
    }
    #[test]
    fn first_request_timeout_is_inside_fixed_window_and_original_deadline_wins() {
        let first = Instant::now();
        let window = RetryWindow::start(
            first,
            first + Duration::from_secs(120),
            WINDOW,
            Uuid::from_u128(1),
        );
        assert_eq!(
            window
                .deadline
                .duration_since(first + Duration::from_secs(15)),
            Duration::from_secs(45)
        );
        assert_eq!(window.deadline, first + Duration::from_secs(60));
        let shorter = RetryWindow::start(
            first,
            first + Duration::from_secs(17),
            WINDOW,
            Uuid::from_u128(2),
        );
        assert_eq!(shorter.deadline, first + Duration::from_secs(17));
    }
    #[tokio::test]
    async fn long_retry_hint_cannot_reset_window_or_trigger_another_read() {
        let root = private_root();
        let journal = journal(&root);
        let (_done, completed) = watch::channel(false);
        let reads = AtomicUsize::new(0);
        let stops = AtomicUsize::new(0);
        let bounded = Policy {
            window: Duration::from_millis(50),
            ..policy()
        };
        let monitor = observe_until_complete(
            completed,
            Instant::now() + Duration::from_secs(30),
            bounded,
            || async {
                reads.fetch_add(1, Ordering::SeqCst);
                Err(RefreshFailure::Transient(
                    Transient::Busy,
                    Some(Duration::from_secs(86400)),
                ))
            },
            || async {
                stops.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |intent| journal.append(intent),
            || Ok(false),
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(5), monitor)
                .await
                .unwrap()
                .is_err()
        );
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        let saved = record(&journal);
        assert!(saved.open);
        assert!(saved.intervals.len() <= 1);
        assert!(saved.intervals.iter().all(|v| v.recovery.is_none()));
    }
    #[tokio::test]
    async fn fatal_read_failures_do_not_enter_retry_window() {
        for failure in [
            RefreshFailure::Unauthorized,
            RefreshFailure::Forbidden,
            RefreshFailure::InvalidResponse,
            RefreshFailure::IdentityOrRevision,
            RefreshFailure::Unclassified,
        ] {
            let root = private_root();
            let journal = journal(&root);
            let (_done, completed) = watch::channel(false);
            let reads = AtomicUsize::new(0);
            let stops = AtomicUsize::new(0);
            let result = observe_until_complete(
                completed,
                Instant::now() + Duration::from_secs(5),
                policy(),
                || async {
                    reads.fetch_add(1, Ordering::SeqCst);
                    Err(failure)
                },
                || async {
                    stops.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
                |intent| journal.append(intent),
                || Ok(false),
            )
            .await;
            assert!(result.is_err());
            assert_eq!(reads.load(Ordering::SeqCst), 1);
            assert_eq!(stops.load(Ordering::SeqCst), 1);
            assert!(record(&journal).intervals.is_empty());
        }
    }
    #[tokio::test]
    async fn cancellation_and_original_deadline_preempt_blocked_reads() {
        for cancel in [true, false] {
            let root = private_root();
            let journal = journal(&root);
            let (done, completed) = watch::channel(false);
            let owner = Mutex::new(Some(done));
            let stops = AtomicUsize::new(0);
            let deadline = Instant::now()
                + if cancel {
                    Duration::from_secs(30)
                } else {
                    Duration::from_millis(50)
                };
            let monitor = observe_until_complete(
                completed,
                deadline,
                policy(),
                || {
                    if cancel {
                        owner.lock().unwrap().take();
                    }
                    std::future::pending::<std::result::Result<Observation, RefreshFailure>>()
                },
                || async {
                    stops.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
                |intent| journal.append(intent),
                || Ok(false),
            );
            assert!(
                tokio::time::timeout(Duration::from_secs(5), monitor)
                    .await
                    .unwrap()
                    .is_err()
            );
            assert_eq!(stops.load(Ordering::SeqCst), 1);
            assert!(record(&journal).open);
        }
    }
    #[tokio::test]
    async fn observed_stop_and_cancellation_preempt_blocked_journal_without_assuming_rollback() {
        for cancel in [false, true] {
            let root = private_root();
            let journal = journal(&root);
            let (done, completed) = watch::channel(false);
            let owner = Mutex::new(Some(done));
            let stops = AtomicUsize::new(0);
            let stop_known = AtomicBool::new(false);
            let retained = Mutex::new(None);
            let monitor = observe_until_complete(
                completed,
                Instant::now() + Duration::from_secs(30),
                policy(),
                || async { Err(RefreshFailure::Transient(Transient::Busy, None)) },
                || async {
                    stops.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
                |intent| {
                    *retained.lock().unwrap() = Some(intent);
                    if cancel {
                        owner.lock().unwrap().take();
                    } else {
                        stop_known.store(true, Ordering::SeqCst);
                    }
                    std::future::pending::<Result<()>>()
                },
                || Ok(stop_known.load(Ordering::SeqCst)),
            );
            assert!(
                tokio::time::timeout(Duration::from_secs(5), monitor)
                    .await
                    .unwrap()
                    .is_err()
            );
            assert_eq!(stops.load(Ordering::SeqCst), 1);
            // The same real write can still commit after monitor cancellation.
            let intent = retained.lock().unwrap().take().unwrap();
            let id = intent.operation_id;
            journal.write(intent).unwrap();
            let saved = record(&journal);
            assert_eq!(saved.operation_id, id);
            assert!(saved.open);
            assert!(saved.intervals[0].recovery.is_none());
            assert!(
                MonitorJournal::create(
                    &root.path().canonicalize().unwrap(),
                    Uuid::from_u128(1),
                    "1".repeat(64)
                )
                .is_err()
            );
        }
    }
    #[tokio::test]
    async fn failed_required_journal_write_stops_without_another_read() {
        let root = private_root();
        let journal = journal(&root);
        let (_done, completed) = watch::channel(false);
        let reads = AtomicUsize::new(0);
        let stops = AtomicUsize::new(0);
        let path = root
            .path()
            .join("worker-control-monitor")
            .join(&journal.name);
        // The real anchored private-file check rejects this unsafe replacement.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let result = observe_until_complete(
            completed,
            Instant::now() + Duration::from_secs(5),
            policy(),
            || async {
                reads.fetch_add(1, Ordering::SeqCst);
                Err(RefreshFailure::Transient(Transient::Busy, None))
            },
            || async {
                stops.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |intent| journal.append(intent),
            || Ok(false),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert_eq!(stops.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn journal_replay_is_idempotent_and_open_or_closed_epoch_never_grants_a_fresh_window() {
        let root = private_root();
        let journal = journal(&root);
        let first = degraded();
        journal.write(first.clone()).unwrap();
        let revision = record(&journal).revision;
        journal.write(first.clone()).unwrap();
        assert_eq!(record(&journal).revision, revision);
        let mut conflict = first;
        conflict.action = JournalAction::ExecutionCompleted;
        assert!(journal.write(conflict).is_err());
        journal.write(recovered()).unwrap();
        let saved = record(&journal);
        assert!(saved.open);
        assert!(saved.intervals[0].recovery.is_some());
        assert!(saved.validate(&"3".repeat(64)).is_err());
        assert!(
            MonitorJournal::create(
                &root.path().canonicalize().unwrap(),
                Uuid::from_u128(1),
                "1".repeat(64)
            )
            .is_err()
        );
        let close = WriteIntent::new(JournalAction::ExecutionCompleted);
        journal.write(close.clone()).unwrap();
        let revision = record(&journal).revision;
        journal.write(close).unwrap();
        assert_eq!(record(&journal).revision, revision);
        assert!(!record(&journal).open);
        assert!(journal.write(degraded()).is_err());
        assert!(
            MonitorJournal::create(
                &root.path().canonicalize().unwrap(),
                Uuid::from_u128(1),
                "1".repeat(64)
            )
            .is_err()
        );
    }
    #[tokio::test]
    async fn cancelling_an_already_started_blocking_write_preserves_its_exact_commit() {
        let root = private_root();
        let journal = journal(&root);
        let held = journal.current.lock().unwrap();
        let intent = degraded();
        let id = intent.operation_id;
        let task_journal = journal.clone();
        let (entered, started) = tokio::sync::oneshot::channel();
        let task = tokio::task::spawn_blocking(move || {
            let _ = entered.send(());
            task_journal.write(intent)
        });
        started.await.unwrap();
        task.abort();
        drop(held);
        task.await.unwrap().unwrap();
        let saved = record(&journal);
        assert_eq!(saved.operation_id, id);
        assert!(saved.open);
        assert_eq!(saved.intervals.len(), 1);
        assert!(saved.pending.is_none());
    }
    #[test]
    fn journal_capacity_and_prepared_identity_fail_closed() {
        let root = private_root();
        let journal = journal(&root);
        for index in 0..64u64 {
            let mut first = degraded();
            if let JournalAction::Degraded { interval } = &mut first.action {
                interval.interval_id = Uuid::from_u128(100 + u128::from(index));
                interval.first_request_sequence = index * 2 + 1;
            }
            journal.write(first).unwrap();
            let mut next = recovered();
            if let JournalAction::Recovered {
                interval_id,
                recovery,
            } = &mut next.action
            {
                *interval_id = Uuid::from_u128(100 + u128::from(index));
                recovery.request_sequence = index * 2 + 2;
            }
            journal.write(next).unwrap();
        }
        let before = fs::read(
            root.path()
                .join("worker-control-monitor")
                .join(&journal.name),
        )
        .unwrap();
        assert!(journal.write(degraded()).is_err());
        assert_eq!(
            fs::read(
                root.path()
                    .join("worker-control-monitor")
                    .join(&journal.name)
            )
            .unwrap(),
            before
        );
        let mut pending = record(&journal);
        let intent = WriteIntent::new(JournalAction::ExecutionCompleted);
        pending.operation_id = intent.operation_id;
        pending.operation_hash = intent.hash().unwrap();
        pending.pending = Some(intent);
        assert!(pending.validate(&"1".repeat(64)).is_ok());
        pending.operation_hash = "4".repeat(64);
        assert!(pending.validate(&"1".repeat(64)).is_err());
    }

    #[test]
    fn raw_oversized_journal_is_unverifiable_and_refused_without_rewrite() {
        let root = private_root();
        let journal = journal(&root);
        let path = root
            .path()
            .join("worker-control-monitor")
            .join(&journal.name);
        let mut padded = fs::read(&path).unwrap();
        padded.resize(MAX_JOURNAL_BYTES + 1, b' ');
        fs::write(&path, &padded).unwrap();
        // Valid JSON can still exceed the on-disk journal bound. Check the
        // reopen classification before write() has any chance to normalize it.
        let resumed = resume_block(
            &root.path().canonicalize().unwrap(),
            Uuid::from_u128(1),
            &"1".repeat(64),
        );
        let written = journal.write(degraded());
        let after = fs::read(&path).unwrap();
        assert!(
            resumed == ResumeBlock::Unverifiable && written.is_err() && after == padded,
            "oversized raw journal accepted or rewritten: resume={resumed:?}, write={written:?}, bytes_before={}, bytes_after={}",
            padded.len(),
            after.len()
        );
    }

    #[tokio::test]
    async fn post_ack_epoch_creation_has_its_own_bounded_arming_deadline() {
        let (_owner, mut completed) = watch::channel(false);
        let stops = AtomicUsize::new(0);
        let stop = || async {
            stops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let known_stop = || Ok(false);
        let arming = await_epoch_creation(
            std::future::pending::<Result<Arc<MonitorJournal>>>(),
            &mut completed,
            Instant::now(),
            Instant::now() + Duration::from_secs(300),
            &known_stop,
            &stop,
        );
        // The extra five seconds are a harness watchdog, not execution time
        // granted to the product. No GET has begun and no 60s window exists.
        let returned = tokio::time::timeout(REQUEST_TIMEOUT + Duration::from_secs(5), arming).await;
        assert!(
            returned.is_ok() && stops.load(Ordering::SeqCst) == 1,
            "post-ACK epoch creation waited for the original execution deadline instead of its 15s arming bound: returned={}, stops={}",
            returned.is_ok(),
            stops.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn restart_with_missing_open_recovered_or_invalid_epoch_cannot_grant_a_new_window() {
        let root = private_root();
        let config = root.path().canonicalize().unwrap();
        let session = Uuid::from_u128(1);
        let context = "1".repeat(64);
        assert_eq!(
            resume_block(&config, session, &context),
            ResumeBlock::Absent
        );
        let journal = journal(&root);
        assert_eq!(
            resume_block(&config, session, &context),
            ResumeBlock::Unfinished
        );
        journal.write(degraded()).unwrap();
        journal.write(recovered()).unwrap();
        assert_eq!(
            resume_block(&config, session, &context),
            ResumeBlock::Unfinished
        );
        assert_eq!(
            resume_block(&config, session, &"2".repeat(64)),
            ResumeBlock::Unverifiable
        );
        journal
            .write(WriteIntent::new(JournalAction::ExecutionCompleted))
            .unwrap();
        assert_eq!(
            resume_block(&config, session, &context),
            ResumeBlock::Completed
        );
        fs::write(
            root.path()
                .join("worker-control-monitor")
                .join(&journal.name),
            b"{invalid}",
        )
        .unwrap();
        assert_eq!(
            resume_block(&config, session, &context),
            ResumeBlock::Unverifiable
        );
    }

    fn worker() -> WorkerV1 {
        let id = Uuid::from_u128(51).to_string();
        WorkerV1 {
            worker_id: id.clone(),
            admission_operation_id: Uuid::from_u128(52).to_string(),
            request_id: Uuid::from_u128(53).to_string(),
            receipt: DecisionReceiptV1 {
                version: 1,
                request_id: Uuid::from_u128(53).to_string(),
                request_hash: "1".repeat(64),
                phase: DecisionPhaseV1::Initial,
                audit_seq: 1,
                audit_hash: "2".repeat(64),
            },
            launch: LaunchSpecV1 {
                version: 1,
                session_id: Uuid::from_u128(54).to_string(),
                command: crate::model::CommandSpec {
                    argv: vec!["true".into()],
                    cwd: "/workspace".into(),
                },
                profile: "observe".into(),
                runtime_kind: RuntimeKindV1::Docker,
                authority_id: Uuid::from_u128(55).to_string(),
                image_ref: "local-fixture".into(),
                image_id: format!("sha256:{}", "3".repeat(64)),
                network: NetworkV1::None,
                cpu_millis: 1000,
                memory_bytes: 1024 * 1024 * 1024,
                pids_limit: 128,
                timeout_seconds: 300,
                security_preset: SecurityPresetV1::FixedSandboxV2,
                restart_policy: RestartPolicyV1::No,
                workspace_manifest_digest: "4".repeat(64),
                workspace_import_policy_version: 1,
                provider: "generic".into(),
                capture_output: false,
                credential_binding_id: None,
                credential_names: Vec::new(),
                provider_resolution: None,
            },
            spec_hash: "5".repeat(64),
            policy_hash: "6".repeat(64),
            state: WorkerStateV1::Running,
            quota_held: true,
            revision: 7,
            attempt: WorkerAttemptV1 {
                attempt_id: Uuid::from_u128(56).to_string(),
                stable_name: format!("aig-fixture-{}", id.replace('-', "")),
                volume_name: format!("aig-fixture-{}-work", id.replace('-', "")),
                container_id: Some("owned-fixture-cid".into()),
                created_inspection: None,
                create_operation_id: Some(Uuid::from_u128(57).to_string()),
                start_operation_id: Some(Uuid::from_u128(58).to_string()),
                start_emitter_id: Some(Uuid::from_u128(59).to_string()),
            },
            observed_started_at: Some("2026-11-07T00:00:00Z".into()),
            allocation_inconsistent: false,
            last_observation: None,
            unknown_reason: None,
            cleanup_removed: None,
            cleanup_error: None,
        }
    }

    #[test]
    fn higher_revision_terminal_requires_released_quota_and_terminal_observation() {
        // Source contract: audit/control.rs::apply_worker_transition only emits
        // Terminal after classify_observation returns Terminal, sets quota_held
        // false, and stores the incoming observation. These invalid copies are
        // defensive client inputs, not responses the current Store can produce.
        let mut expected = worker();
        expected.launch.cpu_millis = 2000;
        expected.launch.pids_limit = 256;
        expected.spec_hash = expected.launch.spec_hash().unwrap();
        expected.attempt.container_id = Some("a".repeat(64));
        let inspection = RuntimeInspectionV1 {
            authority_id: expected.launch.authority_id.clone(),
            runtime_kind: expected.launch.runtime_kind,
            container_id: expected.attempt.container_id.clone().unwrap(),
            stable_name: expected.attempt.stable_name.clone(),
            volume_name: expected.attempt.volume_name.clone(),
            image_id: expected.launch.image_id.clone(),
            spec_hash: expected.spec_hash.clone(),
            cpu_millis: expected.launch.cpu_millis,
            memory_bytes: expected.launch.memory_bytes,
            pids_limit: expected.launch.pids_limit,
            network: expected.launch.network,
            security_preset: expected.launch.security_preset,
            restart_policy: expected.launch.restart_policy,
        };
        expected.attempt.created_inspection = Some(inspection.clone());
        expected.last_observation = Some(RuntimeObservationV1 {
            inspection: inspection.clone(),
            observed_at: "2026-11-07T00:00:00Z".into(),
            status: RuntimeStatusV1::Running,
            running: Some(true),
            started_at: Some("2026-11-07T00:00:00Z".into()),
            finished_at: None,
            exit_code: None,
            oom_killed: Some(false),
            restart_count: Some(0),
        });
        let mut terminal = expected.clone();
        terminal.revision = 8;
        terminal.state = WorkerStateV1::Terminal;
        terminal.quota_held = false;
        terminal.last_observation = Some(RuntimeObservationV1 {
            inspection,
            observed_at: "2026-11-07T00:00:01Z".into(),
            status: RuntimeStatusV1::Exited,
            running: Some(false),
            started_at: Some("2026-11-07T00:00:00Z".into()),
            finished_at: Some("2026-11-07T00:00:01Z".into()),
            exit_code: Some(0),
            oom_killed: Some(false),
            restart_count: Some(0),
        });
        assert!(
            validate_response(&expected, &terminal).is_ok(),
            "coherent Terminal DTO must remain admissible"
        );
        let mut quota_still_held = terminal.clone();
        quota_still_held.quota_held = true;
        let mut missing_observation = terminal;
        missing_observation.last_observation = None;
        let quota_result = validate_response(&expected, &quota_still_held);
        let observation_result = validate_response(&expected, &missing_observation);
        assert!(
            quota_result.is_err() && observation_result.is_err(),
            "incoherent higher-revision Terminal DTO accepted: quota_held={quota_result:?}, missing_observation={observation_result:?}"
        );
    }

    // A real resume entry-point regression. The engine is the repository's
    // explicit Rust CLI fixture, not Docker/Podman or a business process. Its
    // running state is declared by the test; owned/configuration observations
    // and the targeted kill are made through the real ManagedRuntime adapter.
    #[tokio::test]
    async fn active_resume_stops_exact_locally_bound_instance_while_gateway_stays_unavailable() {
        use axum::{Router, http::StatusCode};
        let root = private_root();
        let base = root.path().canonicalize().unwrap();
        let config = base.join("config");
        crate::operations::initialize_config(&config).unwrap();
        let config = config.canonicalize().unwrap();
        let project = base.join("project");
        fs::create_dir(&project).unwrap();
        let bin = base.join("trusted-fixture");
        fs::create_dir(&bin).unwrap();
        let source = bin.join("engine.rs");
        fs::write(
            &source,
            include_str!("../../tests/runtime_fixtures/runner_terminal.rs"),
        )
        .unwrap();
        let program = bin.join("docker");
        let compiled =
            std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
                .args(["--edition=2024", "--crate-name", "monitor_resume_fixture"])
                .arg(&source)
                .arg("-o")
                .arg(&program)
                .output()
                .unwrap();
        assert!(
            compiled.status.success(),
            "explicit Rust fixture compilation failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let metadata = program.with_extension("fixture");
        fs::create_dir(&metadata).unwrap();
        fs::write(metadata.join("root"), base.to_str().unwrap()).unwrap();
        fs::write(metadata.join("case"), "monitor-resume-offline").unwrap();

        let gateway_calls = Arc::new(AtomicUsize::new(0));
        let calls = gateway_calls.clone();
        let app = Router::new().fallback(move || {
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                StatusCode::SERVICE_UNAVAILABLE
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (shutdown, stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });

        let mut worker = worker();
        worker.launch.cpu_millis = 2000;
        worker.launch.pids_limit = 256;
        worker.launch.image_id = format!("sha256:{}", "b".repeat(64));
        worker.launch.command.argv = vec!["/usr/bin/id".into()];
        worker.launch.workspace_manifest_digest =
            content_hash("ai-guard:workspace-manifest:v1", &Manifest::default()).unwrap();
        worker.attempt.stable_name = format!(
            "aig-{}-{}",
            Uuid::from_u128(70).simple(),
            worker.worker_id.replace('-', "")
        );
        worker.attempt.volume_name = format!("{}-work", worker.attempt.stable_name);
        let session = Uuid::parse_str(&worker.launch.session_id).unwrap();
        let operation = Uuid::parse_str(&worker.admission_operation_id).unwrap();
        let request = DecisionRequest {
            request_id: worker.request_id.clone(),
            profile: worker.launch.profile.clone(),
            command: worker.launch.command.clone(),
            project_id: None,
            sandbox_id: None,
            agent_id: None,
            session_id: Some(session.to_string()),
            correlation_id: None,
            trace_id: None,
            human_approved: false,
            approval_hash: None,
            execution_mode: None,
            provider_resolution: None,
        };
        worker.receipt.request_hash = crate::policy::request_hash(&request);
        worker.spec_hash = worker.launch.spec_hash().unwrap();
        let runtime = ManagedRuntime::connect_docker_with_runtime(
            Uuid::parse_str(&worker.launch.authority_id).unwrap(),
            &format!("unix://{}", base.join("fixture.sock").display()),
            Runtime::with_program(RuntimeKind::Docker, &program).unwrap(),
        )
        .await
        .unwrap();
        let spec = ContainerSpec::with_operation(
            session,
            ContainerPurpose::Run,
            &worker.launch.image_id,
            worker.launch.command.argv.clone(),
            NetworkMode::None,
            operation,
        )
        .unwrap()
        .with_security_preset(worker.launch.security_preset)
        .with_managed_names(&worker.attempt.stable_name, &worker.attempt.volume_name)
        .unwrap();
        let cid = "a".repeat(64);
        let instance = base.join("containers").join(&cid);
        fs::create_dir_all(&instance).unwrap();
        fs::create_dir_all(base.join("volumes")).unwrap();
        for (name, value) in [
            ("name", worker.attempt.stable_name.as_str()),
            ("volume", worker.attempt.volume_name.as_str()),
            ("purpose", "run"),
            ("state", "created"),
            ("cmd", ""),
            ("network", "none"),
            ("entry", "/usr/bin/id"),
        ] {
            fs::write(instance.join(name), value).unwrap();
        }
        fs::write(instance.join("labels"), serde_json::to_vec(&json!({
            "agentsandbox.session": session.to_string(), "agentsandbox.operation": operation.to_string(), "agentsandbox.purpose": "run",
        })).unwrap()).unwrap();
        // lookup_created validates the complete fixture sandbox against the
        // admitted command, session, operation, network, image and limits.
        let owned = runtime.lookup_created(&spec).await.unwrap().unwrap();
        assert_eq!(owned.handle.id(), cid);
        assert!(owned.observation.configuration_fingerprint.is_some());
        worker.attempt.container_id = Some(cid.clone());
        worker.attempt.created_inspection = Some(inspection(&worker, &owned.observation).unwrap());
        fs::write(instance.join("state"), "running").unwrap();
        let running = runtime.observe(&owned.handle).await.unwrap();
        running
            .validate_identity_allocation(&owned.observation)
            .unwrap();
        assert_eq!(running.process.running, Some(true));
        worker.last_observation = Some(observation_dto(&worker, &running).unwrap());
        worker.observed_started_at = worker.last_observation.as_ref().unwrap().started_at.clone();

        let options = RunOptions {
            project,
            config_dir: config.clone(),
            gateway: origin.clone(),
            runtime: RuntimeKind::Docker,
            runtime_program: Some(program.clone()),
            runtime_endpoint: Some(runtime.binding().endpoint.clone()),
            image: worker.launch.image_ref.clone(),
            profile: worker.launch.profile.clone(),
            network: NetworkMode::None,
            allow_rootful_runtime: false,
            timeout: u64::from(worker.launch.timeout_seconds),
            provider: "generic".into(),
            client: None,
            auth_mode: None,
            client_profile: None,
            providers_file: None,
            prompt: None,
            model: None,
            approve_hash: None,
            capture_output: false,
            interactive: false,
            secret_env_file: None,
            argv: worker.launch.command.argv.clone(),
        };
        let admission = AdmissionV1 {
            version: 1,
            operation_id: operation.to_string(),
            decision_request: request.clone(),
            receipt: worker.receipt.clone(),
            launch: worker.launch.clone(),
        };
        admission.validate().unwrap();
        let plan = RunPlan {
            version: 2,
            operation_id: operation,
            options,
            request,
            session,
            provider_input: None,
            provider_credential: None,
            credential_source: None,
            managed: Some(ManagedCheckpoint {
                tenant_id: "local-fixture-tenant".into(),
                gateway_origin: format!("{origin}/"),
                config_root: config.clone(),
                binding: Some(runtime.binding().clone()),
                authority_file: "unused-authority.json".into(),
                admission,
                worker: Some(worker.clone()),
                owned: Some(owned.clone()),
                pending: None,
                uncertain: Vec::new(),
                lock_identity: None,
                before: Manifest::default(),
                import: None,
                decision: json!({"outcome":"allow"}),
                output_event: None,
                output_acknowledged: false,
                result: None,
                archive_name: "unused-archive.tar".into(),
                archive_digest: "0".repeat(64),
                credential_digest: None,
                workspace_control_exclusion_version: Some(1),
                invocation_event: None,
            }),
        };
        let token = read_token(&config.join("secrets/runner")).unwrap();
        let client = AuditClient::new(&origin, &token).unwrap();
        validate_checkpoint(&plan, &client).unwrap();
        validate_worker(&plan, &worker).unwrap();
        let state = PlanState::new(plan).unwrap();
        let guard = lock_attempt(&state).unwrap();
        state
            .update(|plan| {
                plan.managed.as_mut().unwrap().lock_identity = Some(guard.identity());
                Ok(())
            })
            .unwrap();
        let epoch =
            MonitorJournal::create(&config, session, context_hash(&state).unwrap()).unwrap();
        epoch.write(degraded()).unwrap();
        assert_eq!(
            resume_block(&config, session, &context_hash(&state).unwrap()),
            ResumeBlock::Unfinished
        );
        assert_eq!(
            recover_owned(&runtime, &state)
                .await
                .unwrap()
                .unwrap()
                .handle
                .id(),
            cid
        );
        drop(guard); // Simulate loss of the monitor owner; no Instant is restored.
        let checkpoint_before = state.checkpoint().unwrap();
        assert_eq!(
            client
                .http
                .get(client.base.join("api/v1/identity").unwrap())
                .bearer_auth(&token)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );

        let resumed = tokio::time::timeout(
            Duration::from_secs(20),
            super::super::resume_worker(session, &config, &origin, 0, Some(&program), false, false),
        )
        .await;
        let calls_after_resume = fs::read_to_string(base.join("calls")).unwrap();
        let state_after_resume = fs::read_to_string(instance.join("state")).unwrap();
        let checkpoint_after: RunPlan = state.directory.read(&state.name).unwrap().unwrap();
        assert_eq!(
            client
                .http
                .get(client.base.join("api/v1/identity").unwrap())
                .bearer_auth(&token)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        shutdown.send(()).unwrap();
        server.await.unwrap();
        // Positive fixture canary and cleanup precede the final RED oracle, so
        // failure to reach kill is not confused with an incapable fixture.
        let stopped = runtime.stop_snapshot_owned(&owned).await.unwrap();
        assert_eq!(stopped.process.running, Some(false));
        assert_eq!(gateway_calls.load(Ordering::SeqCst) >= 2, true);
        assert!(
            resumed.is_ok(),
            "resume did not return within its independent harness bound"
        );
        let after = checkpoint_after.managed.unwrap();
        assert_eq!(after.tenant_id, checkpoint_before.tenant_id);
        assert_eq!(after.gateway_origin, checkpoint_before.gateway_origin);
        assert_eq!(after.worker.as_ref().unwrap().worker_id, worker.worker_id);
        assert_eq!(after.worker.as_ref().unwrap().attempt, worker.attempt);
        assert!(
            after.worker.as_ref().unwrap().quota_held,
            "offline stop cannot invent durable terminal accounting"
        );
        assert!(
            record(&epoch).open,
            "a restart must not fabricate recovery or a fresh epoch"
        );
        assert_eq!(
            calls_after_resume
                .lines()
                .filter(|line| line.starts_with("start ") || line.starts_with("create "))
                .count(),
            0
        );
        assert_eq!(
            calls_after_resume
                .lines()
                .filter(|line| *line == format!("kill {cid}"))
                .count(),
            1,
            "the real resume entry did not stop its exact locally bound instance while gateway was unavailable"
        );
        assert_eq!(state_after_resume, "exited");
    }

    #[tokio::test]
    async fn authenticated_resume_can_adopt_valid_replacement_before_stopping_same_instance() {
        // Keep setup's HTTP/runtime state machine separate from the much larger
        // resume state machine. The resumed Run instance is declared below;
        // trusted postprocessing may execute one separate Export helper.
        struct Fixture {
            root: tempfile::TempDir,
            base: PathBuf,
            config: PathBuf,
            bin: PathBuf,
            program: PathBuf,
            origin: String,
            client: AuditClient,
            store: Arc<crate::audit::Store>,
            shutdown: tokio::sync::oneshot::Sender<()>,
            server: tokio::task::JoinHandle<()>,
            session: Uuid,
            worker: WorkerV1,
            owned: OwnedContainerSnapshot,
            runtime: ManagedRuntime,
            state: PlanState,
            epoch: Arc<MonitorJournal>,
            cid: String,
            instance: PathBuf,
        }
        async fn setup() -> Fixture {
            use axum::{
                extract::Request,
                middleware::{self, Next},
            };
            let root = private_root();
            let base = root.path().canonicalize().unwrap();
            let config = base.join("config");
            crate::operations::initialize_config(&config).unwrap();
            let config = config.canonicalize().unwrap();
            let project = base.join("project");
            fs::create_dir(&project).unwrap();
            let bin = base.join("trusted-adoption-fixture");
            fs::create_dir(&bin).unwrap();
            let source = bin.join("engine.rs");
            fs::write(
                &source,
                include_str!("../../tests/runtime_fixtures/runner_terminal.rs"),
            )
            .unwrap();
            let program = bin.join("docker");
            let compiled = std::process::Command::new(
                std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()),
            )
            .args(["--edition=2024", "--crate-name", "monitor_adoption_fixture"])
            .arg(&source)
            .arg("-o")
            .arg(&program)
            .output()
            .unwrap();
            assert!(
                compiled.status.success(),
                "Rust fixture compilation failed: {}",
                String::from_utf8_lossy(&compiled.stderr)
            );
            let metadata = program.with_extension("fixture");
            fs::create_dir(&metadata).unwrap();
            fs::write(metadata.join("root"), base.to_str().unwrap()).unwrap();
            fs::write(metadata.join("case"), "monitor-adoption-resume").unwrap();
            let token = read_token(&config.join("secrets/runner")).unwrap();
            let viewer = read_token(&config.join("secrets/viewer")).unwrap();
            let redactor = Redactor::new(vec![token.clone(), viewer.clone()]);
            let store = Arc::new(
                crate::audit::Store::open(&base.join("adoption.sqlite"), redactor.clone(), 1000)
                    .unwrap(),
            );
            let gateway = crate::api::GatewayState::new(
                store.clone(),
                token.clone(),
                viewer,
                crate::policy::Policy::default(),
                None,
                redactor,
            );
            let marker_root = base.clone();
            let marker_store = store.clone();
            let app = crate::api::gateway_router(gateway).layer(middleware::from_fn(
                move |request: Request, next: Next| {
                    let root = marker_root.clone();
                    let store = marker_store.clone();
                    async move {
                        let transition = request.uri().path().ends_with("/transitions");
                        let response = next.run(request).await;
                        if transition
                            && response.status().is_success()
                            && store
                                .workers()
                                .unwrap()
                                .iter()
                                .any(|worker| worker.state == WorkerStateV1::Terminal)
                        {
                            // The existing engine fixture permits cleanup only after
                            // the real Store has acknowledged the terminal transition.
                            fs::write(root.join("terminal_ack"), "durable terminal").unwrap();
                        }
                        response
                    }
                },
            ));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let (shutdown, stopped) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = stopped.await;
                    })
                    .await
                    .unwrap();
            });
            let client = AuditClient::new(&origin, &token).unwrap();
            assert_eq!(
                check_identity(&client, Some("local")).await.unwrap(),
                "local"
            );

            let draft = worker();
            let session = Uuid::parse_str(&draft.launch.session_id).unwrap();
            let operation = Uuid::parse_str(&draft.admission_operation_id).unwrap();
            let request = DecisionRequest {
                request_id: draft.request_id.clone(),
                profile: "observe".into(),
                command: crate::model::CommandSpec {
                    argv: vec!["/usr/bin/id".into()],
                    cwd: "/workspace".into(),
                },
                project_id: None,
                sandbox_id: None,
                agent_id: None,
                session_id: Some(session.to_string()),
                correlation_id: None,
                trace_id: None,
                human_approved: false,
                approval_hash: None,
                execution_mode: None,
                provider_resolution: None,
            };
            let decision = Box::pin(client.decide(&request)).await.unwrap();
            let receipt: DecisionReceiptV1 =
                serde_json::from_value(decision["receipt"].clone()).unwrap();
            let mut launch = draft.launch.clone();
            launch.command = request.command.clone();
            launch.cpu_millis = 2000;
            launch.pids_limit = 256;
            launch.image_id = format!("sha256:{}", "b".repeat(64));
            launch.workspace_manifest_digest =
                content_hash("ai-guard:workspace-manifest:v1", &Manifest::default()).unwrap();
            let admission = AdmissionV1 {
                version: 1,
                operation_id: operation.to_string(),
                decision_request: request.clone(),
                receipt,
                launch,
            };
            let admitted: AdmissionResultV1 =
                Box::pin(client.post_control("api/v1/workers/reservations", &admission))
                    .await
                    .unwrap();
            assert!(admitted.accepted);
            let mut worker = admitted.worker.unwrap();
            async fn transition(
                client: &AuditClient,
                worker: &mut WorkerV1,
                action: WorkerTransitionV1,
            ) {
                let mutation = WorkerMutationV1 {
                    version: 1,
                    operation_id: Uuid::new_v4().to_string(),
                    worker_id: worker.worker_id.clone(),
                    attempt_id: worker.attempt.attempt_id.clone(),
                    expected_revision: worker.revision,
                    transition: action,
                };
                let result: MutationResultV1 = Box::pin(client.post_control(
                    &format!("api/v1/workers/{}/transitions", worker.worker_id),
                    &mutation,
                ))
                .await
                .unwrap();
                *worker = result.worker;
            }
            transition(&client, &mut worker, WorkerTransitionV1::Prepare).await;
            transition(&client, &mut worker, WorkerTransitionV1::CreateIssued).await;
            let runtime = ManagedRuntime::connect_docker_with_runtime(
                Uuid::parse_str(&worker.launch.authority_id).unwrap(),
                &format!("unix://{}", base.join("fixture.sock").display()),
                Runtime::with_program(RuntimeKind::Docker, &program).unwrap(),
            )
            .await
            .unwrap();
            let spec = ContainerSpec::with_operation(
                session,
                ContainerPurpose::Run,
                &worker.launch.image_id,
                request.command.argv.clone(),
                NetworkMode::None,
                operation,
            )
            .unwrap()
            .with_security_preset(worker.launch.security_preset)
            .with_managed_names(&worker.attempt.stable_name, &worker.attempt.volume_name)
            .unwrap();
            let cid = "a".repeat(64);
            let instance = base.join("containers").join(&cid);
            fs::create_dir_all(&instance).unwrap();
            fs::create_dir_all(base.join("volumes")).unwrap();
            for (name, value) in [
                ("name", worker.attempt.stable_name.as_str()),
                ("volume", worker.attempt.volume_name.as_str()),
                ("purpose", "run"),
                ("state", "created"),
                ("cmd", ""),
                ("network", "none"),
                ("entry", "/usr/bin/id"),
            ] {
                fs::write(instance.join(name), value).unwrap();
            }
            fs::write(
                instance.join("labels"),
                serde_json::to_vec(&json!({"agentsandbox.session":session.to_string(),
            "agentsandbox.operation":operation.to_string(), "agentsandbox.purpose":"run"}))
                .unwrap(),
            )
            .unwrap();
            let owned = runtime.lookup_created(&spec).await.unwrap().unwrap();
            let created = inspection(&worker, &owned.observation).unwrap();
            transition(
                &client,
                &mut worker,
                WorkerTransitionV1::Created {
                    inspection: created,
                },
            )
            .await;
            transition(
                &client,
                &mut worker,
                WorkerTransitionV1::StartClaim {
                    emitter_id: Uuid::new_v4().to_string(),
                },
            )
            .await;
            // A declared fixture process state, never a start invocation or business command.
            fs::write(instance.join("state"), "running").unwrap();
            let running = runtime.observe(&owned.handle).await.unwrap();
            let observed = observation_dto(&worker, &running).unwrap();
            transition(
                &client,
                &mut worker,
                WorkerTransitionV1::Observe {
                    observation: observed,
                },
            )
            .await;
            assert_eq!(worker.state, WorkerStateV1::Running);
            let options = RunOptions {
                project,
                config_dir: config.clone(),
                gateway: origin.clone(),
                runtime: RuntimeKind::Docker,
                runtime_program: Some(program.clone()),
                runtime_endpoint: Some(runtime.binding().endpoint.clone()),
                image: worker.launch.image_ref.clone(),
                profile: "observe".into(),
                network: NetworkMode::None,
                allow_rootful_runtime: false,
                timeout: u64::from(worker.launch.timeout_seconds),
                provider: "generic".into(),
                client: None,
                auth_mode: None,
                client_profile: None,
                providers_file: None,
                prompt: None,
                model: None,
                approve_hash: None,
                capture_output: false,
                interactive: false,
                secret_env_file: None,
                argv: request.command.argv.clone(),
            };
            let plan = RunPlan {
                version: 2,
                operation_id: operation,
                options,
                request,
                session,
                provider_input: None,
                provider_credential: None,
                credential_source: None,
                managed: Some(ManagedCheckpoint {
                    tenant_id: "local".into(),
                    gateway_origin: format!("{origin}/"),
                    config_root: config.clone(),
                    binding: Some(runtime.binding().clone()),
                    authority_file: "unused-authority.json".into(),
                    admission,
                    worker: Some(worker.clone()),
                    owned: Some(owned.clone()),
                    pending: None,
                    uncertain: Vec::new(),
                    lock_identity: None,
                    before: Manifest::default(),
                    import: None,
                    decision: decision["decision"].clone(),
                    output_event: None,
                    output_acknowledged: false,
                    result: None,
                    archive_name: "unused-archive.tar".into(),
                    archive_digest: "0".repeat(64),
                    credential_digest: None,
                    workspace_control_exclusion_version: Some(1),
                    invocation_event: None,
                }),
            };
            validate_checkpoint(&plan, &client).unwrap();
            validate_worker(&plan, &worker).unwrap();
            let state = PlanState::new(plan).unwrap();
            let guard = lock_attempt(&state).unwrap();
            state
                .update(|plan| {
                    plan.managed.as_mut().unwrap().lock_identity = Some(guard.identity());
                    Ok(())
                })
                .unwrap();
            let epoch =
                MonitorJournal::create(&config, session, context_hash(&state).unwrap()).unwrap();
            epoch.write(degraded()).unwrap();
            drop(guard);
            Fixture {
                root,
                base,
                config,
                bin,
                program,
                origin,
                client,
                store,
                shutdown,
                server,
                session,
                worker,
                owned,
                runtime,
                state,
                epoch,
                cid,
                instance,
            }
        }
        let Fixture {
            root: _root,
            base,
            config,
            bin,
            program,
            origin,
            client,
            store,
            shutdown,
            server,
            session,
            worker,
            owned,
            runtime,
            state,
            epoch: _epoch,
            cid,
            instance,
        } = Box::pin(setup()).await;
        let replacement = bin.join("docker-replacement");
        fs::copy(&program, &replacement).unwrap();
        let replacement_metadata = replacement.with_extension("fixture");
        fs::create_dir(&replacement_metadata).unwrap();
        fs::write(replacement_metadata.join("root"), base.to_str().unwrap()).unwrap();
        fs::write(replacement_metadata.join("case"), "monitor-adoption-resume").unwrap();
        let candidate = Runtime::with_program(RuntimeKind::Docker, &replacement).unwrap();
        let candidate_fingerprint = candidate.invocation().fingerprint();
        let candidate_runtime =
            ManagedRuntime::adopt_invocation(runtime.binding().clone(), candidate)
                .await
                .unwrap();
        assert_eq!(
            candidate_runtime.binding().engine_id,
            runtime.binding().engine_id
        );
        fs::rename(&program, bin.join("historical-program-unavailable")).unwrap();
        // A healthy authenticated gateway must still admit the explicit,
        // audited adoption path; the local STOP-only precheck cannot adopt.
        assert_eq!(
            check_identity(&client, Some("local")).await.unwrap(),
            "local"
        );
        let resumed = tokio::time::timeout(
            Duration::from_secs(30),
            Box::pin(super::super::resume_worker(
                session,
                &config,
                &origin,
                0,
                Some(&replacement),
                true,
                false,
            )),
        )
        .await;
        let calls_after = fs::read_to_string(base.join("calls")).unwrap();
        let adoption_events = store
            .events(&crate::audit::EventQuery {
                kind: Some("runtime.invocation.bound".into()),
                ..Default::default()
            })
            .unwrap();
        let durable_worker = store.worker(&worker.worker_id).unwrap().unwrap();
        let saved: RunPlan = state.directory.read(&state.name).unwrap().unwrap();
        assert_eq!(
            check_identity(&client, Some("local")).await.unwrap(),
            "local"
        );
        assert!(store.integrity().unwrap().valid);
        shutdown.send(()).unwrap();
        server.await.unwrap();
        // Candidate engine/stop capability remains a positive cleanup canary
        // on RED; its kill occurs after the product trace was captured above.
        if instance.is_dir() && fs::read_to_string(instance.join("state")).unwrap() == "running" {
            assert_eq!(
                candidate_runtime
                    .stop_snapshot_owned(&owned)
                    .await
                    .unwrap()
                    .process
                    .running,
                Some(false)
            );
        }
        assert_eq!(durable_worker.worker_id, worker.worker_id);
        assert_eq!(durable_worker.attempt.attempt_id, worker.attempt.attempt_id);
        assert!(durable_worker.attempt.container_id.as_deref() == Some(cid.as_str()));
        assert_eq!(
            durable_worker.attempt.start_operation_id,
            worker.attempt.start_operation_id
        );
        assert_eq!(durable_worker.state, WorkerStateV1::Terminal);
        assert!(!durable_worker.quota_held);
        assert_eq!(durable_worker.cleanup_removed, Some(true));
        assert!(durable_worker.cleanup_error.is_none());
        assert!(!instance.exists());

        // Parse every create/start record, including malformed whitespace or
        // purposes. Only the single postprocessing Export pair is admissible.
        let mut export_created = None;
        let mut export_started = None;
        let mut exact_kills = 0;
        for line in calls_after.lines() {
            if !matches!(
                line.split_whitespace().next(),
                Some("create" | "start" | "kill")
            ) {
                continue;
            }
            let mut fields = line.split(' ');
            match (fields.next(), fields.next(), fields.next(), fields.next()) {
                (Some(operation @ ("create" | "start")), Some(id), Some("export"), None) => {
                    assert!(
                        id.len() == 64
                            && id.bytes().all(|byte| {
                                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
                            })
                            && id != cid.as_str(),
                        "malformed or reused fixture Export identity"
                    );
                    let previous = if operation == "create" {
                        assert_eq!(exact_kills, 1, "Export created before the exact Run stop");
                        export_created.replace(id)
                    } else {
                        assert!(
                            export_created == Some(id),
                            "Export started before its creation"
                        );
                        export_started.replace(id)
                    };
                    assert!(previous.is_none(), "duplicate fixture Export invocation");
                }
                (Some("kill"), Some(id), None, None) => {
                    assert!(
                        id == cid.as_str(),
                        "a different fixture instance was killed"
                    );
                    exact_kills += 1;
                }
                _ => panic!("unexpected fixture create/start/kill record"),
            }
        }
        let export_id = export_created.expect("trusted recovery did not create its Export helper");
        assert!(
            export_started == Some(export_id),
            "Export helper was not started exactly once"
        );
        assert_eq!(exact_kills, 1);
        assert!(!base.join("containers").join(export_id).exists());

        let single_event = |kind: &str| {
            let events = store
                .events(&crate::audit::EventQuery {
                    kind: Some(kind.into()),
                    limit: Some(2),
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(events.len(), 1, "{kind} must be audited exactly once");
            let event = events.into_iter().next().unwrap();
            assert!(
                event.event.session_id.as_deref() == Some(worker.launch.session_id.as_str()),
                "{kind} was not attributed to the original session"
            );
            event
        };
        let scan_requested = single_event("workspace.scan.requested");
        let scan_created = single_event("workspace.scan.created");
        let scan_started = single_event("workspace.scan.started");
        let scan_completed = single_event("workspace.scan.completed");
        let diff_completed = single_event("workspace.diff.completed");
        assert!(
            scan_requested.seq < scan_created.seq
                && scan_created.seq < scan_started.seq
                && scan_started.seq < scan_completed.seq
                && scan_completed.seq < diff_completed.seq,
            "trusted recovery audit sequence is not ordered"
        );
        assert!(scan_created.event.data["container_id"].as_str() == Some(export_id));
        assert!(
            scan_created.event.data["operation_id"]
                .as_str()
                .and_then(|operation| Uuid::parse_str(operation).ok())
                .is_some_and(|operation| !operation.is_nil()),
            "trusted recovery helper operation is not a valid UUID"
        );
        assert!(diff_completed.event.data["changed_files"].as_u64() == Some(0));
        assert!(
            diff_completed.event.data["observation_phase"].as_str()
                == Some("post_completion_recovery")
        );
        let postprocessing = load_postprocessing(&state, &client)
            .unwrap()
            .expect("terminal recovery did not retain its postprocessing journal");
        let payload = &postprocessing.payload;
        assert_eq!(
            payload.declared.state,
            DeclaredCollectionV1::UnavailableAfterInterruption
        );
        assert!(
            payload.declared.error.as_deref()
                == Some("legacy declared capture is unavailable; delivery counts remain unknown")
        );
        assert!(
            [
                payload.declared.normalized_events,
                payload.declared.acknowledged_events,
                payload.declared.dropped_events,
                payload.declared.unknown_delivery_events,
                payload.declared.not_attempted_events,
            ]
            .iter()
            .all(Option::is_none),
            "unavailable declared capture acquired delivery counts"
        );
        let diff = payload
            .diff
            .as_ref()
            .expect("retained trusted diff is missing");
        assert!(diff.completed_acknowledged);
        assert_eq!(diff.changed_files, 0);
        assert!(diff.files.is_empty());
        assert_eq!(diff.acknowledged_files, 0);
        assert!(diff.completed["id"].as_str() == Some(diff_completed.event.id.as_str()));
        assert!(payload.diff_error.is_none());
        assert!(payload.coverage_acknowledged);
        assert!(payload.coverage_error.is_none());
        assert!(!payload.needs_recovery());
        let coverage = payload
            .coverage
            .as_ref()
            .expect("retained coverage is missing");
        let coverage_event = single_event("observer.coverage");
        assert!(coverage["id"].as_str() == Some(coverage_event.event.id.as_str()));
        assert!(coverage["coverage"].as_str() == Some("partial"));
        assert!(coverage["data"]["trusted_diff"].as_str() == Some("complete"));
        assert!(coverage["data"]["postprocessing_complete"].as_bool() == Some(false));
        assert!(
            coverage["data"]["declared_collection"]["state"].as_str()
                == Some("unavailable_after_interruption")
        );

        let checkpoint = saved.managed.as_ref().unwrap();
        let binding = checkpoint.binding.as_ref().unwrap();
        assert!(
            checkpoint.result.is_none(),
            "recovery fabricated a RunSummary"
        );
        assert!(binding == candidate_runtime.binding());
        assert!(
            binding.invocation_fingerprint().as_deref() == Some(candidate_fingerprint.as_str())
        );
        assert_eq!(adoption_events.len(), 1);
        assert!(adoption_events[0].seq < scan_requested.seq);
        assert!(
            adoption_events[0].event.data["invocation_fingerprint"].as_str()
                == Some(candidate_fingerprint.as_str())
        );
        assert!(
            matches!(resumed.as_ref(), Ok(Err(error)) if error.to_string()
                == "postprocessing recovery incomplete: legacy declared capture is unavailable; delivery counts remain unknown"),
            "authenticated recovery did not report its exact unavailable-capture refusal"
        );
    }
    type Reply = (u16, Option<&'static str>, Vec<u8>);
    async fn peer(
        replies: Vec<Reply>,
    ) -> (
        AuditClient,
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
        Arc<AtomicUsize>,
    ) {
        use axum::{
            Router,
            body::Body,
            extract::{Request, State},
            http::{HeaderMap, Response},
            middleware::{self, Next},
            routing::get,
        };
        let queue = Arc::new(Mutex::new(VecDeque::from(replies)));
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let app =
            Router::new()
                .route(
                    "/control/api/v1/workers/{worker}",
                    get(
                        move |State(queue): State<Arc<Mutex<VecDeque<Reply>>>>,
                              headers: HeaderMap| {
                            async move {
                                assert_eq!(
                                    headers.get("authorization").unwrap(),
                                    "Bearer synthetic-monitor-token"
                                );
                                let (status, retry, body) = queue
                                    .lock()
                                    .unwrap()
                                    .pop_front()
                                    .expect("unexpected extra HTTP read");
                                let mut builder = Response::builder().status(status);
                                if let Some(value) = retry {
                                    builder = builder.header("Retry-After", value);
                                }
                                builder.body(Body::from(body)).unwrap()
                            }
                        },
                    ),
                )
                .with_state(queue)
                .layer(middleware::from_fn(move |request: Request, next: Next| {
                    let count = count.clone();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        assert!(request.uri().path().starts_with("/control/api/v1/workers/"));
                        next.run(request).await
                    }
                }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = AuditClient::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            "synthetic-monitor-token",
        )
        .unwrap();
        let (shutdown, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        (client, shutdown, task, requests)
    }
    #[tokio::test]
    async fn private_worker_http_classifies_statuses_without_retrying_audit_or_auth_failures() {
        let (client, shutdown, task, requests) = peer(vec![
            (429, Some("120"), Vec::new()),
            (503, Some("1"), Vec::new()),
            (401, None, Vec::new()),
            (403, None, Vec::new()),
            (500, None, Vec::new()),
        ])
        .await;
        for expected in [
            RefreshFailure::Transient(Transient::Busy, Some(Duration::from_secs(120))),
            RefreshFailure::Unclassified,
            RefreshFailure::Unauthorized,
            RefreshFailure::Forbidden,
            RefreshFailure::Unclassified,
        ] {
            assert_eq!(read_worker(&client, &worker()).await.unwrap_err(), expected);
        }
        assert_eq!(requests.load(Ordering::SeqCst), 5);
        shutdown.send(()).unwrap();
        task.await.unwrap();
    }
    #[tokio::test]
    async fn private_worker_http_accepts_only_closed_status_consistent_transport_failures() {
        // Break caught: treating a status, malformed envelope or unknown field
        // as permission to keep running, instead of requiring H's exact DTO.
        let envelope = json!({"error":{
            "version":1,
            "code":"control_transport_failure",
            "phase":"exchange",
            "outcome":"unknown",
            "retry_action":"reconcile_same_operation",
            "reason":"upstream_connection_unavailable"
        }});
        let mut cases = Vec::new();
        for (status, reason, expected) in [
            (
                502,
                "upstream_connection_unavailable",
                Transient::ConnectionInterrupted,
            ),
            (
                502,
                "upstream_connection_interrupted",
                Transient::ConnectionInterrupted,
            ),
            (504, "upstream_timeout", Transient::Timeout),
        ] {
            let mut value = envelope.clone();
            value["error"]["reason"] = json!(reason);
            cases.push((
                status,
                value.to_string().into_bytes(),
                RefreshFailure::Transient(expected, None),
            ));
        }
        for (field, invalid) in [
            ("version", json!(2)),
            ("code", json!("control_transport_protocol_failure")),
            ("phase", json!("response")),
            ("outcome", json!("not_dispatched")),
            ("retry_action", json!("contact_operator")),
            ("reason", json!("unknown_reason")),
            ("extra", json!(true)),
        ] {
            let mut value = envelope.clone();
            value["error"][field] = invalid;
            cases.push((
                502,
                value.to_string().into_bytes(),
                RefreshFailure::InvalidResponse,
            ));
        }
        let mut extra_envelope = envelope.clone();
        extra_envelope["extra"] = json!(true);
        cases.push((
            502,
            extra_envelope.to_string().into_bytes(),
            RefreshFailure::InvalidResponse,
        ));
        let mut missing = envelope.clone();
        missing["error"].as_object_mut().unwrap().remove("version");
        cases.push((
            502,
            missing.to_string().into_bytes(),
            RefreshFailure::InvalidResponse,
        ));
        for (status, reason) in [
            (502, "upstream_timeout"),
            (504, "upstream_connection_unavailable"),
            (504, "upstream_connection_interrupted"),
        ] {
            let mut value = envelope.clone();
            value["error"]["reason"] = json!(reason);
            cases.push((
                status,
                value.to_string().into_bytes(),
                RefreshFailure::InvalidResponse,
            ));
        }
        let dto = envelope.to_string();
        for body in [
            dto.replacen("\"version\":1", "\"version\":1,\"version\":1", 1),
            format!(
                "{{\"error\":{},\"error\":{}}}",
                envelope["error"], envelope["error"]
            ),
            format!("{dto}{{}}"),
            "{}".into(),
            "{\"error\":{\"code\":\"control_transport_protocol_failure\"}}".into(),
        ] {
            cases.push((502, body.into_bytes(), RefreshFailure::InvalidResponse));
        }
        // Even a valid DTO under another status is not a transport failure.
        cases.push((503, dto.as_bytes().to_vec(), RefreshFailure::Unclassified));
        cases.push((200, dto.into_bytes(), RefreshFailure::InvalidResponse));
        let expected: Vec<_> = cases.iter().map(|(_, _, failure)| *failure).collect();
        let replies = cases
            .into_iter()
            .map(|(status, body, _)| (status, None, body))
            .collect();
        let (client, shutdown, task, requests) = peer(replies).await;
        for (index, failure) in expected.iter().enumerate() {
            assert_eq!(
                read_worker(&client, &worker()).await.unwrap_err(),
                *failure,
                "case {index}"
            );
        }
        assert_eq!(requests.load(Ordering::SeqCst), expected.len());
        shutdown.send(()).unwrap();
        task.await.unwrap();
    }
    #[tokio::test]
    async fn real_http_429_recovers_through_same_monitor_and_durable_journal() {
        let expected = worker();
        let body = serde_json::to_vec(&expected).unwrap();
        let (client, shutdown, task, requests) = peer(vec![
            (429, None, Vec::new()),
            (200, None, body.clone()),
            (200, None, body),
        ])
        .await;
        let root = private_root();
        let journal = journal(&root);
        let (done, completed) = watch::channel(false);
        let reads = AtomicUsize::new(0);
        let stops = AtomicUsize::new(0);
        let result = observe_until_complete(
            completed,
            Instant::now() + Duration::from_secs(5),
            policy(),
            || {
                let call = reads.fetch_add(1, Ordering::SeqCst);
                let done = done.clone();
                let client = &client;
                let expected = &expected;
                async move {
                    let worker = read_worker(client, expected).await?;
                    let observed = Observation {
                        stopping: false,
                        revision: worker.revision,
                        hash: worker_hash(&worker).map_err(|_| RefreshFailure::InvalidResponse)?,
                    };
                    if call == 2 {
                        let _ = done.send(true);
                    }
                    Ok(observed)
                }
            },
            || async {
                stops.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |intent| journal.append(intent),
            || Ok(false),
        )
        .await;
        shutdown.send(()).unwrap();
        task.await.unwrap();
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(requests.load(Ordering::SeqCst), 3);
        assert_eq!(stops.load(Ordering::SeqCst), 0);
        let saved = closed_record(&journal).await;
        assert!(!saved.open);
        assert_eq!(saved.intervals.len(), 1);
        assert_eq!(saved.intervals[0].cause, Transient::Busy);
        assert_eq!(
            saved.intervals[0]
                .recovery
                .as_ref()
                .unwrap()
                .request_sequence,
            2
        );
    }
    #[tokio::test]
    async fn private_worker_http_requires_complete_same_attempt_and_nonregressing_revision() {
        let expected = worker();
        let value = serde_json::to_value(&expected).unwrap();
        let mut stale = value.clone();
        stale["revision"] = json!(6);
        let mut changed = value.clone();
        changed["attempt"]["attempt_id"] = json!(Uuid::from_u128(99).to_string());
        let mut same_revision = value.clone();
        same_revision["state"] = json!("stopping");
        let mut partial = value.clone();
        partial.as_object_mut().unwrap().remove("last_observation");
        let mut secret = value.clone();
        secret["unknown_reason"] = json!("synthetic-monitor-token");
        let mut advanced = value.clone();
        advanced["revision"] = json!(8);
        advanced["state"] = json!("stopping");
        let mut replies: Vec<Reply> = [
            value,
            stale,
            changed,
            same_revision,
            partial,
            secret,
            advanced,
        ]
        .into_iter()
        .map(|value| (200, None, serde_json::to_vec(&value).unwrap()))
        .collect();
        replies.push((
            200,
            None,
            b"{\"worker_id\":\"a\",\"worker_id\":\"b\"}".to_vec(),
        ));
        replies.push((200, None, vec![b'x'; 512 * 1024 + 1]));
        let (client, shutdown, task, requests) = peer(replies).await;
        assert_eq!(read_worker(&client, &expected).await.unwrap().revision, 7);
        for failure in [
            RefreshFailure::IdentityOrRevision,
            RefreshFailure::IdentityOrRevision,
            RefreshFailure::IdentityOrRevision,
            RefreshFailure::InvalidResponse,
            RefreshFailure::InvalidResponse,
        ] {
            assert_eq!(read_worker(&client, &expected).await.unwrap_err(), failure);
        }
        let accepted = read_worker(&client, &expected).await.unwrap();
        assert_eq!(accepted.revision, 8);
        assert_eq!(accepted.state, WorkerStateV1::Stopping);
        assert_eq!(
            read_worker(&client, &expected).await.unwrap_err(),
            RefreshFailure::InvalidResponse
        );
        assert_eq!(
            read_worker(&client, &expected).await.unwrap_err(),
            RefreshFailure::InvalidResponse
        );
        assert_eq!(requests.load(Ordering::SeqCst), 9);
        shutdown.send(()).unwrap();
        task.await.unwrap();
    }
}
