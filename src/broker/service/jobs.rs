//! Bounded process-local execution coordination. Durable facts stay in Journal.
use super::{profile::ControlProfileV1, protocol::*};
use crate::{
    broker::{
        checkpoint::SupervisionOperationV1,
        enforcer::CancelGeneration,
        journal::{
            AcceptedJobV1, JournalEntryKindV1, JournalEntryV1, JournalError, JournalStatusV1,
        },
        lifecycle::LocalCommandRequestV1,
        writer::ProfileWriterCapabilities,
    },
    control::DecisionReceiptV1,
    model::Outcome,
    runner::broker::BrokerRunState,
    workspace::AuthorizedSource,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex as AsyncMutex, OnceCell, OwnedMutexGuard};
use uuid::Uuid;

const MAX_JOBS: usize = 256;

#[cfg(test)]
mod retirement_audit_tests {
    use super::*;

    #[test]
    fn audit_sweep_visits_later_jobs_without_creating_or_waiting_for_earlier_jobs() {
        let jobs = ProfileJobs::new();
        for id in 1..=6 {
            jobs.job(Uuid::from_u128(id)).unwrap();
        }
        let first = jobs.retirement_audit_batch().unwrap();
        let second = jobs.retirement_audit_batch().unwrap();
        assert_eq!(first.len(), 4);
        assert_eq!(second.len(), 4);
        assert!(second.contains(&Uuid::from_u128(5)));
        assert!(second.contains(&Uuid::from_u128(6)));
        assert_eq!(jobs.jobs.lock().unwrap().len(), 6);
        assert!(jobs.known(Uuid::from_u128(7)).is_none());
    }

    #[test]
    fn retirement_probe_cursor_rotates_one_entry_and_remains_metadata_after_cancel() {
        let job = Job::new();
        let ids = [Uuid::from_u128(3), Uuid::from_u128(1), Uuid::from_u128(2)];
        for expected in [1, 2, 3, 1] {
            assert_eq!(
                job.next_retirement_query(&ids).unwrap(),
                Some(Uuid::from_u128(expected))
            );
        }
        job.cancel.invalidate();
        assert_eq!(
            job.next_retirement_query(&ids).unwrap(),
            Some(Uuid::from_u128(2))
        );
        assert_eq!(job.next_retirement_query(&[]).unwrap(), None);
        assert!(job.next_retirement_query(&[Uuid::nil()]).is_err());
    }
}

pub(super) struct ProfileJobs {
    pub(super) writer: OnceCell<ProfileWriterCapabilities>,
    identity: Arc<AsyncMutex<()>>,
    jobs: Mutex<BTreeMap<Uuid, Arc<Job>>>,
    retirement_audit_cursor: Mutex<Option<Uuid>>,
    pub(super) retirement_startup: super::retirement_startup::StartupSlot,
    #[cfg(test)]
    retirement_snapshot_gate: Mutex<Option<RetirementSnapshotGate>>,
    #[cfg(test)]
    retirement_after_q_gate: Mutex<Option<RetirementSnapshotGate>>,
    #[cfg(test)]
    retirement_before_q_gate: Mutex<Option<RetirementSnapshotGate>>,
}
#[cfg(test)]
struct RetirementSnapshotGate {
    entered: tokio::sync::oneshot::Sender<()>,
    resume: tokio::sync::oneshot::Receiver<()>,
    completed: tokio::sync::oneshot::Sender<()>,
}
impl ProfileJobs {
    pub(super) fn new() -> Self {
        Self {
            writer: OnceCell::new(),
            identity: Arc::new(AsyncMutex::new(())),
            jobs: Mutex::new(BTreeMap::new()),
            retirement_audit_cursor: Mutex::new(None),
            retirement_startup: super::retirement_startup::StartupSlot::default(),
            #[cfg(test)]
            retirement_snapshot_gate: Mutex::new(None),
            #[cfg(test)]
            retirement_after_q_gate: Mutex::new(None),
            #[cfg(test)]
            retirement_before_q_gate: Mutex::new(None),
        }
    }
    #[cfg(test)]
    pub(super) fn gate_retirement_snapshot_for_test(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        let (entered, observe) = tokio::sync::oneshot::channel();
        let (resume, wait) = tokio::sync::oneshot::channel();
        let (completed, done) = tokio::sync::oneshot::channel();
        let mut gate = self.retirement_snapshot_gate.lock().unwrap();
        assert!(gate.is_none(), "one per-profile test barrier");
        *gate = Some(RetirementSnapshotGate {
            entered,
            resume: wait,
            completed,
        });
        (observe, resume, done)
    }
    #[cfg(test)]
    pub(super) async fn retirement_snapshot_boundary_for_test(
        &self,
    ) -> Option<tokio::sync::oneshot::Sender<()>> {
        let gate = { self.retirement_snapshot_gate.lock().unwrap().take() };
        let gate = gate?;
        let _ = gate.entered.send(());
        let _ = gate.resume.await;
        Some(gate.completed)
    }
    #[cfg(test)]
    pub(super) fn gate_retirement_after_q_for_test(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        let (entered, observe) = tokio::sync::oneshot::channel();
        let (resume, wait) = tokio::sync::oneshot::channel();
        let (completed, done) = tokio::sync::oneshot::channel();
        let mut gate = self.retirement_after_q_gate.lock().unwrap();
        assert!(gate.is_none(), "one per-profile post-Q test barrier");
        *gate = Some(RetirementSnapshotGate {
            entered,
            resume: wait,
            completed,
        });
        (observe, resume, done)
    }
    #[cfg(test)]
    pub(super) async fn retirement_after_q_boundary_for_test(
        &self,
    ) -> Option<tokio::sync::oneshot::Sender<()>> {
        let gate = { self.retirement_after_q_gate.lock().unwrap().take() };
        let gate = gate?;
        let _ = gate.entered.send(());
        let _ = gate.resume.await;
        Some(gate.completed)
    }
    #[cfg(test)]
    pub(super) fn gate_retirement_before_q_for_test(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        let (entered, observe) = tokio::sync::oneshot::channel();
        let (resume, wait) = tokio::sync::oneshot::channel();
        let (completed, done) = tokio::sync::oneshot::channel();
        let mut gate = self.retirement_before_q_gate.lock().unwrap();
        assert!(gate.is_none(), "one per-profile pre-Q test barrier");
        *gate = Some(RetirementSnapshotGate {
            entered,
            resume: wait,
            completed,
        });
        (observe, resume, done)
    }
    #[cfg(test)]
    pub(super) async fn retirement_before_q_boundary_for_test(
        &self,
    ) -> Option<tokio::sync::oneshot::Sender<()>> {
        let gate = { self.retirement_before_q_gate.lock().unwrap().take() };
        let gate = gate?;
        let _ = gate.entered.send(());
        let _ = gate.resume.await;
        Some(gate.completed)
    }
    /// Covers only identity checks and their local durable publication. Never
    /// hold this across a provider/runtime/HTTP call or the execution lifetime.
    /// Cancel invalidates its memory generation before waiting for this gate.
    pub(super) async fn identity_gate(&self) -> Result<OwnedMutexGuard<()>, BrokerErrorCodeV1> {
        tokio::time::timeout(
            Duration::from_secs(15),
            Arc::clone(&self.identity).lock_owned(),
        )
        .await
        .map_err(|_| BrokerErrorCodeV1::Busy)
    }
    pub(super) fn job(&self, operation: Uuid) -> Result<Arc<Job>, BrokerErrorCodeV1> {
        if operation.is_nil() {
            return Err(BrokerErrorCodeV1::InvalidInput);
        }
        let mut jobs = self
            .jobs
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        if let Some(job) = jobs.get(&operation) {
            return Ok(Arc::clone(job));
        }
        if jobs.len() >= MAX_JOBS {
            return Err(BrokerErrorCodeV1::LimitExceeded);
        }
        let job = Arc::new(Job::new());
        jobs.insert(operation, Arc::clone(&job));
        Ok(job)
    }
    pub(super) fn known(&self, operation: Uuid) -> Option<Arc<Job>> {
        self.jobs.lock().ok()?.get(&operation).cloned()
    }
    /// Process-local exclusion over the original known Job. Authentication
    /// and durable/native evidence remain necessary for every guardian action.
    pub(super) fn try_retirement_query(
        &self,
        operation: Uuid,
    ) -> Result<Option<RetirementQueryGuard>, BrokerErrorCodeV1> {
        if operation.is_nil() {
            return Err(BrokerErrorCodeV1::InvalidInput);
        }
        let job = self
            .jobs
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?
            .get(&operation)
            .cloned()
            .ok_or(BrokerErrorCodeV1::Unavailable)?;
        let Ok(guard) = Arc::clone(&job.gate).try_lock_owned() else {
            return Ok(None);
        };
        Ok(Some(RetirementQueryGuard {
            job,
            operation,
            _guard: guard,
        }))
    }
    /// A bounded round-robin metadata sweep. A broken job cannot permanently
    /// keep later jobs behind its audit retries; this never creates a job.
    pub(super) fn retirement_audit_batch(&self) -> Result<Vec<Uuid>, BrokerErrorCodeV1> {
        use std::ops::Bound::{Excluded, Unbounded};
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let mut cursor = self
            .retirement_audit_cursor
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let after = cursor.unwrap_or(Uuid::nil());
        let batch = jobs
            .range((Excluded(after), Unbounded))
            .chain(jobs.range(..=after))
            .map(|(id, _)| *id)
            .take(4)
            .collect::<Vec<_>>();
        if let Some(last) = batch.last() {
            *cursor = Some(*last);
        }
        Ok(batch)
    }
    pub(super) fn invalidate_all(&self) {
        if let Ok(jobs) = self.jobs.lock() {
            for job in jobs.values() {
                job.cancel.invalidate();
            }
        }
    }
    pub(super) fn has_retained_retirement(&self) -> bool {
        self.retirement_startup.has_retained()
            || self
                .jobs
                .lock()
                .map(|jobs| {
                    jobs.values()
                        .any(|job| job.preclaim.has_retained_retirement())
                })
                .unwrap_or(true)
    }
}

pub(super) struct Job {
    pub(super) cancel: CancelGeneration,
    pub(super) preclaim: super::preclaim::PreclaimControlV1,
    gate: Arc<AsyncMutex<()>>,
    running: AtomicBool,
    cancelling: AtomicUsize,
    cancel_commands: Mutex<BTreeMap<Uuid, CancelState>>,
    error: Mutex<Option<BrokerErrorCodeV1>>,
    source: Mutex<Option<Arc<AuthorizedSource>>>,
    reconciliation: Mutex<(u32, Option<Instant>)>,
    retirement_query_cursor: Mutex<Option<Uuid>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CancelState {
    Pending,
    Unknown,
    Confirmed,
}
impl Job {
    fn new() -> Self {
        let cancel = CancelGeneration::new();
        Self {
            preclaim: super::preclaim::PreclaimControlV1::new(cancel.clone()),
            cancel,
            gate: Arc::new(AsyncMutex::new(())),
            running: AtomicBool::new(false),
            cancelling: AtomicUsize::new(0),
            cancel_commands: Mutex::new(BTreeMap::new()),
            error: Mutex::new(None),
            source: Mutex::new(None),
            reconciliation: Mutex::new((0, None)),
            retirement_query_cursor: Mutex::new(None),
        }
    }
    /// A failed attempt does not queue another execution behind a live one.
    /// Cancel deliberately does not acquire this gate.
    pub(super) fn enter(self: &Arc<Self>) -> Result<RunningJob, BrokerErrorCodeV1> {
        if self.cancelling.load(Ordering::SeqCst) != 0 {
            return Err(BrokerErrorCodeV1::Busy);
        }
        if self
            .cancel_commands
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?
            .values()
            .any(|state| *state != CancelState::Confirmed)
        {
            return Err(BrokerErrorCodeV1::NeedsReconciliation);
        }
        let guard = Arc::clone(&self.gate)
            .try_lock_owned()
            .map_err(|_| BrokerErrorCodeV1::Busy)?;
        let generation = self.cancel.current();
        if self.cancelling.load(Ordering::SeqCst) != 0 {
            return Err(BrokerErrorCodeV1::Busy);
        }
        self.running.store(true, Ordering::SeqCst);
        Ok(RunningJob {
            job: Arc::clone(self),
            generation,
            _guard: guard,
        })
    }
    pub(super) fn begin_cancel(
        self: &Arc<Self>,
        command: Uuid,
    ) -> Result<Option<CancellingJob>, BrokerErrorCodeV1> {
        let mut commands = self
            .cancel_commands
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let unknown = match commands.get(&command) {
            Some(CancelState::Confirmed) => return Ok(None),
            Some(CancelState::Pending) => return Err(BrokerErrorCodeV1::Busy),
            Some(CancelState::Unknown) => true,
            None => false,
        };
        if !unknown && commands.len() >= 64 {
            return Err(BrokerErrorCodeV1::LimitExceeded);
        }
        commands.insert(command, CancelState::Pending);
        self.cancelling.fetch_add(1, Ordering::SeqCst);
        if !unknown {
            self.cancel.invalidate();
        }
        Ok(Some(CancellingJob {
            job: Arc::clone(self),
            command,
            retain_pending: unknown,
        }))
    }
    /// This cache records a durable request, never successful stop delivery.
    /// Replaying it must not begin another cancellation or revoke later work.
    pub(super) fn has_acknowledged_cancel(&self, command: Uuid) -> Result<bool, BrokerErrorCodeV1> {
        Ok(self
            .cancel_commands
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?
            .get(&command)
            == Some(&CancelState::Confirmed))
    }
    pub(super) fn remember_cancellations(&self, durable: &AcceptedJobV1) {
        let Some(checkpoint) = durable.broker_checkpoint_v3() else {
            return;
        };
        let ids = checkpoint
            .commands()
            .iter()
            .filter(|command| matches!(command.request(), LocalCommandRequestV1::CancelRequested))
            .map(|command| command.command_id())
            .collect::<Vec<_>>();
        let _ = self.remember_cancel_ids(&ids);
    }
    /// Recovery can retain cancellation identities without reconstructing a
    /// business job or opening any provider credential.
    pub(super) fn remember_cancel_ids(&self, ids: &[Uuid]) -> Result<(), BrokerErrorCodeV1> {
        if ids.len() > 64 || ids.iter().any(Uuid::is_nil) {
            return Err(BrokerErrorCodeV1::NeedsReconciliation);
        }
        let mut commands = self
            .cancel_commands
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let new_ids = ids.iter().filter(|id| !commands.contains_key(id)).count();
        if commands.len().saturating_add(new_ids) > 64 {
            return Err(BrokerErrorCodeV1::LimitExceeded);
        }
        for id in ids {
            commands.insert(*id, CancelState::Confirmed);
        }
        Ok(())
    }
    pub(super) fn source(&self) -> Result<Option<Arc<AuthorizedSource>>, BrokerErrorCodeV1> {
        Ok(self
            .source
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?
            .clone())
    }
    pub(super) fn set_source(
        &self,
        source: Option<Arc<AuthorizedSource>>,
    ) -> Result<(), BrokerErrorCodeV1> {
        *self
            .source
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)? = source;
        Ok(())
    }
    /// Select one confirmed retirement for this process's next metadata probe.
    /// The cursor is scheduling data and cannot authorize a guardian action.
    pub(super) fn next_retirement_query(
        &self,
        ids: &[Uuid],
    ) -> Result<Option<Uuid>, BrokerErrorCodeV1> {
        if ids.iter().any(Uuid::is_nil) {
            return Err(BrokerErrorCodeV1::NeedsReconciliation);
        }
        let mut cursor = self
            .retirement_query_cursor
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let next = cursor
            .and_then(|after| ids.iter().copied().filter(|id| *id > after).min())
            .or_else(|| ids.iter().copied().min());
        if let Some(next) = next {
            *cursor = Some(next);
        }
        Ok(next)
    }
    pub(super) fn result(&self, error: Option<BrokerErrorCodeV1>) {
        if let Ok(mut current) = self.error.lock() {
            *current = error;
        }
    }
    /// Bound autonomous retry pressure and revision consumption. An explicit
    /// authenticated command remains available after this budget is exhausted.
    pub(super) fn autonomous_attempt(&self) -> bool {
        let Ok(mut retry) = self.reconciliation.lock() else {
            return false;
        };
        let (count, last) = &mut *retry;
        let delay = Duration::from_secs(5_u64.saturating_mul(1_u64 << (*count).min(6)));
        if *count >= 8 || last.is_some_and(|last| last.elapsed() < delay) {
            return false;
        }
        *count += 1;
        *last = Some(Instant::now());
        true
    }
    pub(super) fn execution_status(&self) -> (bool, Option<BrokerErrorCodeV1>) {
        let running = self.running.load(Ordering::SeqCst);
        let error = match self.error.lock() {
            Ok(error) => *error,
            Err(_) => Some(BrokerErrorCodeV1::Unavailable),
        };
        (running, error)
    }

    fn enrich(&self, status: &mut JobStatusV1) {
        (status.executor_task_running, status.executor_error) = self.execution_status();
    }
}

/// Metadata coordination only. The same mutex excludes an existing Full
/// producer, while acquisition leaves running/cancellation state unchanged.
pub(super) struct RetirementQueryGuard {
    job: Arc<Job>,
    operation: Uuid,
    _guard: OwnedMutexGuard<()>,
}
impl RetirementQueryGuard {
    pub(super) fn operation_id(&self) -> Uuid {
        self.operation
    }
    pub(super) fn job(&self) -> &Arc<Job> {
        &self.job
    }
}

pub(super) struct RunningJob {
    job: Arc<Job>,
    generation: u64,
    _guard: OwnedMutexGuard<()>,
}
impl RunningJob {
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }
}
impl Drop for RunningJob {
    fn drop(&mut self) {
        self.job.running.store(false, Ordering::SeqCst);
    }
}
pub(super) struct CancellingJob {
    job: Arc<Job>,
    command: Uuid,
    retain_pending: bool,
}
impl CancellingJob {
    pub(super) fn acknowledged(&mut self) {
        if let Ok(mut commands) = self.job.cancel_commands.lock() {
            commands.insert(self.command, CancelState::Confirmed);
        }
    }
    pub(super) fn unknown(&mut self) {
        self.retain_pending = true;
    }
}
impl Drop for CancellingJob {
    fn drop(&mut self) {
        if let Ok(mut commands) = self.job.cancel_commands.lock()
            && commands.get(&self.command) == Some(&CancelState::Pending)
        {
            if self.retain_pending {
                commands.insert(self.command, CancelState::Unknown);
            } else {
                commands.remove(&self.command);
            }
        }
        self.job.cancelling.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(super) fn journal_error(error: JournalError) -> BrokerErrorCodeV1 {
    use BrokerErrorCodeV1 as Code;
    match error {
        JournalError::InvalidInput | JournalError::ControlSecret => Code::InvalidInput,
        JournalError::UnsupportedVersion => Code::IncompatibleControl,
        JournalError::TooLarge | JournalError::LimitExceeded => Code::LimitExceeded,
        JournalError::Busy => Code::Busy,
        JournalError::Conflict => Code::Conflict,
        JournalError::Interrupted => Code::Interrupted,
        JournalError::Corrupt | JournalError::UnsafeFilesystem => Code::NeedsReconciliation,
        JournalError::Io => Code::Unavailable,
    }
}

pub(super) fn base_status(operation_id: Uuid, state: JobStateV1) -> JobStatusV1 {
    JobStatusV1 {
        operation_id,
        state,
        head_revision: None,
        head_hash: None,
        intent_hash: None,
        cancel_requested: false,
        approval_submitted: false,
        failure_count: 0,
        pending_rpc_count: None,
        worker_state: None,
        quota_held: None,
        runtime_status: None,
        last_observed_at: None,
        review_request_hash: None,
        last_decision: None,
        executor_task_running: false,
        executor_error: None,
    }
}

pub(super) fn status(
    operation: Uuid,
    durable: Option<JournalStatusV1>,
    local: Option<&Job>,
) -> JobStatusV1 {
    let mut result = match durable {
        None => base_status(operation, JobStateV1::Absent),
        Some(JournalStatusV1::ReservedInProcess) => {
            base_status(operation, JobStateV1::ReservedInProcess)
        }
        Some(JournalStatusV1::Interrupted) => base_status(operation, JobStateV1::Interrupted),
        Some(JournalStatusV1::Rejected(_)) => base_status(operation, JobStateV1::Rejected),
        Some(JournalStatusV1::Accepted(job)) => {
            if let Some(local) = local {
                local.remember_cancellations(&job);
            }
            accepted_status(&job)
        }
    };
    if let Some(local) = local {
        local.enrich(&mut result);
    }
    result
}

pub(super) fn entry_status(entry: &JournalEntryV1, local: Option<&Job>) -> JobStatusV1 {
    let state = match entry.kind() {
        JournalEntryKindV1::ReservedInProcess => JobStateV1::ReservedInProcess,
        JournalEntryKindV1::Interrupted => JobStateV1::Interrupted,
        JournalEntryKindV1::Rejected => JobStateV1::Rejected,
        JournalEntryKindV1::Accepted => JobStateV1::Accepted,
    };
    let mut result = base_status(entry.operation_id(), state);
    result.head_revision = entry.head().map(|head| head.revision());
    result.head_hash = entry.head().map(|head| head.hash().to_owned());
    result.cancel_requested = entry.cancel_requested();
    result.approval_submitted = entry.approval_submitted();
    result.failure_count = entry.failure_count();
    result.pending_rpc_count = entry.pending_rpc_count();
    result.worker_state = entry.worker_state().copied();
    result.quota_held = entry.quota_held();
    result.runtime_status = entry.runtime_status().copied();
    result.last_observed_at = entry.last_observed_at().map(str::to_owned);
    if let Some(local) = local {
        local.enrich(&mut result);
    }
    result
}

/// The same closed V1 diagnostic fields, from an actor-acknowledged projection.
/// No checkpoint, review receipt or runner material is recovered from this view.
pub(super) fn last_confirmed_status(
    projection: &crate::broker::journal::status_projection::LastConfirmedStatus,
    local: Option<&Job>,
) -> JobStatusV1 {
    let mut result = entry_status(projection.entry(), local);
    result.intent_hash = projection.intent_hash().map(str::to_owned);
    result.last_decision = projection.last_decision().cloned();
    result.review_request_hash = projection.review_request_hash().map(str::to_owned);
    result
}

fn accepted_status(job: &AcceptedJobV1) -> JobStatusV1 {
    let mut result = base_status(job.operation_id(), JobStateV1::Accepted);
    result.head_revision = Some(job.head().revision());
    result.head_hash = Some(job.head().hash().to_owned());
    result.intent_hash = Some(job.intent_hash().to_owned());
    result.last_decision = last_decision(job);
    if let Some(cp) = job.broker_checkpoint_v2() {
        result.pending_rpc_count = Some(
            cp.supervision()
                .operations()
                .iter()
                .filter(|op| !op.has_response())
                .count(),
        );
    }
    if let Some(cp) = job.broker_checkpoint_v3() {
        for command in cp.commands() {
            match command.request() {
                LocalCommandRequestV1::CancelRequested => result.cancel_requested = true,
                LocalCommandRequestV1::ApprovalSubmitted { .. } => result.approval_submitted = true,
                LocalCommandRequestV1::FailureObserved { .. } => result.failure_count += 1,
            }
        }
    }
    let context = job.checkpoint().broker_context();
    if let Some(worker) = context.worker {
        result.worker_state = Some(worker.state);
        result.quota_held = Some(worker.quota_held);
        if let Some(observation) = &worker.last_observation {
            result.runtime_status = Some(observation.status);
            result.last_observed_at = Some(observation.observed_at.clone());
        }
    }
    if !context.managed && !context.request.human_approved && !result.cancel_requested {
        result.review_request_hash = review(job).map(|(hash, _)| hash);
    }
    result
}

pub(super) fn last_decision(job: &AcceptedJobV1) -> Option<crate::model::Decision> {
    job.broker_checkpoint_v2()?
        .supervision()
        .operations()
        .iter()
        .rev()
        .find_map(|operation| match operation {
            SupervisionOperationV1::Decision {
                response: Some(answer),
                ..
            } => Some(answer.decision().clone()),
            _ => None,
        })
}

/// Return only the initial, acknowledged real Review. The client's supplied
/// hash is compared with this value before appending ApprovalSubmitted.
pub(super) fn review(job: &AcceptedJobV1) -> Option<(String, DecisionReceiptV1)> {
    let current = job.checkpoint().broker_context();
    if current.managed || current.request.human_approved {
        return None;
    }
    job.broker_checkpoint_v2()?
        .supervision()
        .operations()
        .iter()
        .find_map(|op| match op {
            SupervisionOperationV1::Decision {
                request,
                response: Some(answer),
            } if !request.human_approved
                && answer.decision().outcome == Outcome::Review
                && request.request_id == current.request.request_id =>
            {
                Some((
                    crate::policy::request_hash(request),
                    answer.receipt().clone(),
                ))
            }
            _ => None,
        })
}

/// A routing profile is not permission to reinterpret a frozen intention with
/// another configuration, runtime invocation, image, network or provider source.
pub(super) fn require_profile(
    job: &AcceptedJobV1,
    profile: &ControlProfileV1,
) -> Result<(), BrokerErrorCodeV1> {
    let state = BrokerRunState::from_job(job).map_err(journal_error)?;
    let options = state.options();
    let matches = options.config_dir == profile.config_dir
        && options.gateway == profile.gateway
        && options.runtime == profile.runtime
        && options.runtime_program.as_ref() == Some(&profile.runtime_program)
        && options.runtime_endpoint.as_deref() == Some(profile.runtime_endpoint.as_str())
        && options.image == profile.image
        && options.profile == profile.policy_profile
        && options.network == profile.network
        && options.allow_rootful_runtime == profile.allow_rootful_runtime
        && options.timeout <= u64::from(profile.max_timeout_seconds)
        && (!options.capture_output || profile.allow_capture_output)
        && !options.interactive
        && (options.client.is_none() || profile.managed_clients)
        && (options.client.is_none() || options.providers_file == profile.providers_file);
    if matches {
        Ok(())
    } else {
        Err(BrokerErrorCodeV1::Conflict)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_generation_invalidates_while_execution_gate_is_held() {
        let jobs = ProfileJobs::new();
        let operation = Uuid::new_v4();
        let job = jobs.job(operation).expect("job");
        let other = jobs.job(operation).expect("same job");
        let running = job.enter().expect("single flight");
        assert!(matches!(other.enter(), Err(BrokerErrorCodeV1::Busy)));
        let before = job.cancel.current();
        assert!(other.cancel.invalidate() > before);
        assert_eq!(job.cancel.current(), other.cancel.current());
        let mut status = base_status(operation, JobStateV1::Accepted);
        job.enrich(&mut status);
        assert!(status.executor_task_running);
        drop(running);
        assert!(other.enter().is_ok());
    }

    #[test]
    fn local_error_is_visible_without_fabricating_a_durable_terminal() {
        let job = Job::new();
        job.result(Some(BrokerErrorCodeV1::Unavailable));
        let status = status(
            Uuid::new_v4(),
            Some(JournalStatusV1::Interrupted),
            Some(&job),
        );
        assert_eq!(status.state, JobStateV1::Interrupted);
        assert_eq!(status.executor_error, Some(BrokerErrorCodeV1::Unavailable));
        assert!(status.worker_state.is_none());
        assert!(status.quota_held.is_none());
        assert!(status.runtime_status.is_none());
    }

    #[test]
    fn acknowledged_cancel_replay_does_not_invalidate_a_later_execution() {
        let job = Arc::new(Job::new());
        let command = Uuid::new_v4();
        assert!(!job.has_acknowledged_cancel(command).expect("cache"));
        let mut cancellation = job
            .begin_cancel(command)
            .expect("cancel")
            .expect("new command");
        let cancelled_generation = job.cancel.current();
        assert!(matches!(job.enter(), Err(BrokerErrorCodeV1::Busy)));
        assert!(matches!(
            job.begin_cancel(command),
            Err(BrokerErrorCodeV1::Busy)
        ));
        cancellation.acknowledged();
        assert!(job.has_acknowledged_cancel(command).expect("durable cache"));
        drop(cancellation);
        let export = job.enter().expect("fresh post-cancel command");
        assert_eq!(export.generation(), cancelled_generation);
        assert!(job.begin_cancel(command).expect("replay").is_none());
        assert_eq!(job.cancel.current(), export.generation());
    }

    #[test]
    fn unknown_cancel_can_be_retried_without_reinvalidating_or_forgetting_it() {
        let job = Arc::new(Job::new());
        let command = Uuid::new_v4();
        let mut first = job.begin_cancel(command).unwrap().unwrap();
        let generation = job.cancel.current();
        first.unknown();
        drop(first);
        assert!(matches!(
            job.enter(),
            Err(BrokerErrorCodeV1::NeedsReconciliation)
        ));
        // A partial diagnostic cannot prove that an unknown write is absent.
        job.remember_cancel_ids(&[Uuid::new_v4()]).unwrap();
        assert!(matches!(
            job.enter(),
            Err(BrokerErrorCodeV1::NeedsReconciliation)
        ));
        let retry = job.begin_cancel(command).unwrap().unwrap();
        assert_eq!(job.cancel.current(), generation);
        drop(retry); // Even a not-queued retry retains the earlier unknown write.
        assert!(matches!(
            job.enter(),
            Err(BrokerErrorCodeV1::NeedsReconciliation)
        ));
        let mut retry = job.begin_cancel(command).unwrap().unwrap();
        retry.acknowledged();
        drop(retry);
        assert_eq!(job.enter().unwrap().generation(), generation);

        let fresh = job.begin_cancel(Uuid::new_v4()).unwrap().unwrap();
        drop(fresh); // A fresh not-queued write has no durable ambiguity.
        assert!(job.enter().is_ok());
    }
}
