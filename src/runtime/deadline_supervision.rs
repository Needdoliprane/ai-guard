//! Batch scheduling and private, read-only observation ownership. No start
//! permission or terminal accounting is produced. The caller owns and pins
//! each observation/callback; selecting a control never drops that future.
use std::{future::Future, pin::Pin};

pub(super) enum Event<C, O, I> {
    Exited(C),
    Observed(O),
    Deadline,
    Interrupted(I),
}

pub(super) async fn next_event<C, D, I, T, O>(
    child: C,
    deadline: D,
    interrupt: I,
    tick: T,
    observation: Pin<&mut O>,
) -> Event<C::Output, O::Output, I::Output>
where
    C: Future,
    D: Future<Output = ()>,
    I: Future,
    T: Future,
    O: Future,
{
    tokio::pin!(child, deadline, interrupt, tick);
    let mut observation = observation;
    let mut observing = false;
    loop {
        tokio::select! {
            status = &mut child => return Event::Exited(status),
            _ = &mut deadline => return Event::Deadline,
            signal = &mut interrupt => return Event::Interrupted(signal),
            _ = &mut tick, if !observing => observing = true,
            result = observation.as_mut(), if observing => return Event::Observed(result),
        }
    }
}

use super::batch_io::{BatchIoCollected, BatchIoEvent, OwnedBatchIo};
use std::{
    process::ExitStatus,
    sync::{Arc, Mutex, PoisonError},
};
use tokio::time::Instant;

use super::managed::{BatchObservationScope, NativeBatchTerminal};

/// The direct compatibility path has no Managed daemon/allocation proof. Its
/// scope retains only its existing actual invocation and created immutable ID.
struct DirectScope {
    runtime: super::Runtime,
    target: String,
}
enum ObservationScope {
    Managed(BatchObservationScope),
    Direct(Arc<DirectScope>),
}
enum TerminalProof {
    Managed(NativeBatchTerminal),
    Direct {
        scope: Arc<DirectScope>,
        observed: super::RuntimeState,
    },
}
impl TerminalProof {
    fn evidence(&self) -> StopEvidence {
        match self {
            Self::Managed(proof) => StopEvidence::Managed(Box::new(proof.observed().clone())),
            Self::Direct { observed, .. } => StopEvidence::Bare(observed.clone()),
        }
    }
    fn same_direct(&self, expected: &Arc<DirectScope>, now: &super::RuntimeState) -> bool {
        match self {
            Self::Direct { scope, observed } => {
                Arc::ptr_eq(scope, expected)
                    && observed.status == now.status
                    && observed.running == now.running
                    && observed.started_at == now.started_at
                    && observed.finished_at == now.finished_at
                    && observed.exit_code == now.exit_code
                    && observed.oom_killed == now.oom_killed
            }
            Self::Managed(_) => false,
        }
    }
}

#[derive(Default)]
struct TerminalState {
    proof: Option<TerminalProof>,
    contradicted: bool,
    contradictory_observation: Option<StopEvidence>,
}

/// Work policy phase only. No caller bool/RuntimeState can close it: its optional
/// proof is produced by this operation's real observation scope, with distinct
/// Managed and direct-compatibility validation boundaries.
pub(super) struct WorkObservation {
    scope: Option<ObservationScope>,
    state: Mutex<TerminalState>,
    changed: tokio::sync::Notify,
}
impl WorkObservation {
    pub fn new(scope: Option<BatchObservationScope>) -> Arc<Self> {
        Arc::new(Self {
            scope: scope.map(ObservationScope::Managed),
            state: Mutex::new(TerminalState::default()),
            changed: tokio::sync::Notify::new(),
        })
    }
    pub fn for_direct(
        runtime: &super::Runtime,
        instance: &super::CreatedInstance,
    ) -> anyhow::Result<Arc<Self>> {
        anyhow::ensure!(
            instance.kind == runtime.kind && !instance.target.is_empty(),
            "direct batch observation does not match its retained created instance"
        );
        Ok(Arc::new(Self {
            scope: Some(ObservationScope::Direct(Arc::new(DirectScope {
                runtime: runtime.clone(),
                target: instance.target.clone(),
            }))),
            state: Mutex::new(TerminalState::default()),
            changed: tokio::sync::Notify::new(),
        }))
    }
    pub fn require_binding(
        &self,
        owner: &super::ManagedRuntime,
        target: &str,
        created: &super::ContainerObservation,
    ) -> anyhow::Result<()> {
        match self.scope.as_ref() {
            Some(ObservationScope::Managed(scope)) => scope.require_binding(owner, target, created),
            _ => anyhow::bail!("native Managed batch observation scope missing or mismatched"),
        }
    }
    pub fn require_direct_binding(
        &self,
        runtime: &super::Runtime,
        instance: &super::CreatedInstance,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            runtime.kind == instance.kind,
            "direct batch runtime kind changed"
        );
        self.require_direct_target(runtime, &instance.target)
    }
    pub fn require_direct_target(
        &self,
        runtime: &super::Runtime,
        target: &str,
    ) -> anyhow::Result<()> {
        match self.scope.as_ref() {
            Some(ObservationScope::Direct(scope)) => {
                anyhow::ensure!(
                    scope.target == target
                        && scope.runtime.kind == runtime.kind
                        && Arc::ptr_eq(&scope.runtime.anchor, &runtime.anchor),
                    "direct batch observation belongs to another invocation or instance"
                );
                Ok(())
            }
            _ => anyhow::bail!("native direct batch observation scope missing or mismatched"),
        }
    }
    pub fn terminal_proven(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.proof.is_some() && !state.contradicted
    }
    pub fn terminal_evidence(&self) -> Option<StopEvidence> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.contradicted {
            None
        } else {
            state.proof.as_ref().map(TerminalProof::evidence)
        }
    }
    fn work_completed(&self) -> bool {
        // A subsequent conflict cannot erase the exact terminal already seen
        // or retroactively change its finished work into a policy timeout.
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .proof
            .is_some()
    }
    fn observe_failed(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.proof.is_some() {
            state.contradicted = true;
        }
        drop(state);
        self.changed.notify_waiters();
    }
    pub async fn observe(&self) -> anyhow::Result<super::ContainerObservation> {
        let Some(ObservationScope::Managed(scope)) = self.scope.as_ref() else {
            anyhow::bail!("native Managed batch observation scope missing");
        };
        let result = scope.observe().await;
        let observation = match result {
            Ok(observation) => observation,
            Err(error) => {
                self.observe_failed();
                return Err(error);
            }
        };
        anyhow::ensure!(
            observation.belongs_to(scope),
            "native observation belongs to another batch scope"
        );
        let (observed, proof) = observation.into_parts();
        let contradicted = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(TerminalProof::Managed(old)) = state.proof.as_ref() {
                if proof.is_none() || !old.same_terminal(&observed) {
                    state.contradicted = true;
                    state.contradictory_observation =
                        Some(StopEvidence::Managed(Box::new(observed.clone())));
                }
            } else if let Some(proof) = proof {
                anyhow::ensure!(
                    proof.belongs_to(scope),
                    "terminal proof belongs to another native scope"
                );
                state.proof = Some(TerminalProof::Managed(proof));
            }
            state.contradicted
        };
        self.changed.notify_waiters();
        anyhow::ensure!(
            !contradicted,
            "native batch terminal observation changed; instance remains uncertain"
        );
        Ok(observed)
    }
    pub async fn inspect(&self) -> anyhow::Result<super::RuntimeState> {
        match self.scope.as_ref() {
            Some(ObservationScope::Managed(_)) => {
                let observed = self.observe().await?;
                super::managed::batch_runtime_state(&observed)
            }
            Some(ObservationScope::Direct(scope)) => {
                // No RuntimeState/JSON argument is accepted. Only the actual
                // pinned invocation's read of this retained CreatedInstance
                // can produce this weaker, direct-path terminal evidence.
                let observed = match scope.runtime.inspect_target(&scope.target).await {
                    Ok(observed) => observed,
                    Err(error) => {
                        self.observe_failed();
                        return Err(error);
                    }
                };
                let terminal = observed.validate_terminal(scope.runtime.kind).is_ok();
                let contradicted = {
                    let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                    if let Some(old) = state.proof.as_ref() {
                        if !terminal || !old.same_direct(scope, &observed) {
                            state.contradicted = true;
                            state.contradictory_observation =
                                Some(StopEvidence::Bare(observed.clone()));
                        }
                    } else if terminal {
                        state.proof = Some(TerminalProof::Direct {
                            scope: scope.clone(),
                            observed: observed.clone(),
                        });
                    }
                    state.contradicted
                };
                self.changed.notify_waiters();
                anyhow::ensure!(
                    !contradicted,
                    "direct batch terminal observation changed; instance remains uncertain"
                );
                Ok(observed)
            }
            None => anyhow::bail!("native batch observation scope missing"),
        }
    }
    async fn wait_deadline(&self, deadline: Instant) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.work_completed() {
                changed.await;
            } else {
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => {
                        if !self.work_completed() { return; }
                    }
                    _ = changed => {}
                }
            }
        }
    }
}
impl std::fmt::Debug for WorkObservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        f.debug_struct("WorkObservation")
            .field("terminal_proof_retained", &state.proof.is_some())
            .field("contradicted", &state.contradicted)
            .field(
                "counter_observation_retained",
                &state.contradictory_observation.is_some(),
            )
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cause {
    Deadline,
    Interrupted,
    OperationFailure,
    IoFailure,
}

#[derive(Default)]
struct CauseState {
    value: Mutex<Option<Cause>>,
    changed: tokio::sync::Notify,
}
#[derive(Clone, Default)]
pub(super) struct CauseLatch(Arc<CauseState>);
impl CauseLatch {
    pub fn get(&self) -> Option<Cause> {
        *self.0.value.lock().unwrap_or_else(PoisonError::into_inner)
    }
    pub fn latch(&self, cause: Cause) {
        let changed = {
            let mut current = self.0.value.lock().unwrap_or_else(PoisonError::into_inner);
            if current.is_none() {
                *current = Some(cause);
                true
            } else {
                false
            }
        };
        if changed {
            self.0.changed.notify_waiters();
        }
    }
    async fn wait_latched(&self) {
        loop {
            let changed = self.0.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.get().is_some() {
                return;
            }
            changed.await;
        }
    }
}

/// Actual result of this call's native stop, never a permission or receipt.
pub(super) struct NativeStop {
    pub observed_stopped: bool,
    pub evidence: Option<StopEvidence>,
    pub errors: Vec<anyhow::Error>,
    pub moved_outer_fallback: bool,
}

pub(super) enum StopEvidence {
    Managed(Box<super::ContainerObservation>),
    Bare(super::RuntimeState),
}
impl std::fmt::Debug for StopEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Managed(observed) => f
                .debug_struct("ManagedStopData")
                .field("running", &observed.process.running)
                .finish(),
            Self::Bare(observed) => f
                .debug_struct("BareStopData")
                .field("running", &observed.running)
                .finish(),
        }
    }
}

/// This exact decision is shared by the direct and Managed outer wrappers.
/// A late capture/writer error cannot erase an already validated terminal and
/// introduce another native fallback merely because no STOP had been needed.
pub(super) fn outer_fallback_decision(
    work: &WorkObservation,
    stop: Option<&NativeStop>,
) -> Option<bool> {
    if work.terminal_proven() {
        Some(true)
    } else {
        stop.and_then(|stop| stop.moved_outer_fallback.then_some(stop.observed_stopped))
    }
}

pub(super) async fn resolve_outer_fallback<F, Fut>(decision: Option<bool>, fallback: F) -> bool
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = bool>,
{
    match decision {
        Some(observed) => observed,
        None => fallback().await,
    }
}

enum External {
    Io(BatchIoEvent),
    Stopped(NativeStop),
    Latched,
}

/// All state is local to the owning call; its callback needs no Send/static.
pub(super) struct Supervisor<S: Future<Output = NativeStop>> {
    pub io: OwnedBatchIo,
    pub cause: CauseLatch,
    pub errors: Vec<anyhow::Error>,
    pub stop_result: Option<NativeStop>,
    pub work: Arc<WorkObservation>,
    stop: Pin<Box<S>>,
    deadline: Instant,
    attach: Option<ExitStatus>,
    io_finished: bool,
}

impl<S: Future<Output = NativeStop>> Supervisor<S> {
    pub fn new(io: OwnedBatchIo, deadline: Instant, cause: CauseLatch, stop: S) -> Self {
        Self {
            io,
            cause,
            deadline,
            errors: Vec::new(),
            stop_result: None,
            work: WorkObservation::new(None),
            stop: Box::pin(stop),
            attach: None,
            io_finished: false,
        }
    }
    pub fn with_work(mut self, work: Arc<WorkObservation>) -> Self {
        self.work = work;
        self
    }

    pub fn closing(&self) -> bool {
        self.cause.get().is_some()
    }
    pub fn attached_exit(&self) -> bool {
        self.attach.is_some() || self.io_finished
    }

    fn latch(&self, cause: Cause) {
        self.cause.latch(cause);
    }

    pub fn operation_failed(&mut self, error: anyhow::Error) {
        self.errors.push(error);
        self.latch(Cause::OperationFailure);
    }

    async fn step<O: Future>(&mut self, operation: Pin<&mut O>) -> Option<O::Output> {
        let closing = self.closing();
        let poll_stop = closing && self.stop_result.is_none();
        let poll_io = !self.io_finished;
        let deadline = self.deadline;
        let work = self.work.clone();
        let io = &mut self.io;
        let stop = self.stop.as_mut();
        let cause = self.cause.clone();
        let external = async {
            if !poll_io && !poll_stop && closing {
                std::future::pending::<External>().await
            } else {
                tokio::select! {
                    event = io.wait_event(), if poll_io => External::Io(event),
                    result = stop, if poll_stop => External::Stopped(result),
                    _ = cause.wait_latched(), if !closing => External::Latched,
                }
            }
        };
        let event = next_event(
            external,
            async {
                if closing {
                    std::future::pending::<()>().await;
                } else {
                    work.wait_deadline(deadline).await;
                }
            },
            async {
                if closing {
                    std::future::pending().await
                } else {
                    tokio::signal::ctrl_c().await
                }
            },
            std::future::ready(()),
            operation,
        )
        .await;
        match event {
            Event::Observed(result) => return Some(result),
            Event::Exited(External::Latched) => {}
            Event::Deadline => self.latch(Cause::Deadline),
            Event::Interrupted(signal) => {
                self.latch(Cause::Interrupted);
                self.errors.push(match signal {
                    Ok(()) => anyhow::anyhow!(
                        "session interrupted; stop requested for only its container"
                    ),
                    Err(error) => anyhow::Error::new(error),
                });
            }
            Event::Exited(External::Stopped(result)) => {
                self.io.begin_drain();
                if !result.errors.is_empty() || self.cause.get() == Some(Cause::Interrupted) {
                    self.io.request_transport_kill();
                }
                self.stop_result = Some(result);
            }
            Event::Exited(External::Io(BatchIoEvent::Exited(Ok(status)))) => {
                self.attach = Some(status);
            }
            Event::Exited(External::Io(BatchIoEvent::Exited(Err(_))))
            | Event::Exited(External::Io(BatchIoEvent::Fault)) => self.latch(Cause::IoFailure),
            Event::Exited(External::Io(BatchIoEvent::OwnerFinished)) => {
                self.io_finished = true;
                if self.attach.is_none() {
                    self.latch(Cause::IoFailure);
                }
            }
        }
        None
    }

    pub async fn drive<O: Future>(&mut self, operation: Pin<&mut O>) -> O::Output {
        let mut operation = operation;
        loop {
            if let Some(result) = self.step(operation.as_mut()).await {
                return result;
            }
        }
    }

    pub async fn finish_stop(&mut self) {
        while self.closing() && self.stop_result.is_none() {
            let pending = std::future::pending::<()>();
            tokio::pin!(pending);
            let _ = self.step(pending.as_mut()).await;
        }
    }

    pub async fn wait_attach(&mut self) {
        while !self.attached_exit() {
            let pending = std::future::pending::<()>();
            tokio::pin!(pending);
            let _ = self.step(pending.as_mut()).await;
        }
    }
}

/// Returned only after local callback and IO owner have actually completed.
/// Original causes and bounded partial captures remain owned, never formatted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NotificationState {
    NotEngaged,
    Engaged,
    Succeeded,
    Failed,
}

pub(super) struct Notification(Mutex<NotificationState>);
impl Notification {
    pub fn new() -> Self {
        Self(Mutex::new(NotificationState::NotEngaged))
    }
    pub fn get(&self) -> NotificationState {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
    pub async fn notify<F, Fut>(
        &self,
        state: super::RuntimeState,
        callback: &mut F,
    ) -> anyhow::Result<()>
    where
        F: FnMut(super::RuntimeState) -> Fut,
        Fut: Future<Output = anyhow::Result<()>>,
    {
        {
            let mut current = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            anyhow::ensure!(
                *current == NotificationState::NotEngaged,
                "started notification already engaged; it cannot be reissued"
            );
            *current = NotificationState::Engaged;
        }
        // No lock across caller code or await. This exact future is retained.
        let result = callback(state).await;
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = if result.is_ok() {
            NotificationState::Succeeded
        } else {
            NotificationState::Failed
        };
        result
    }
}
impl Default for Notification {
    fn default() -> Self {
        Self::new()
    }
}

pub(super) struct Failure {
    pub first_cause: Option<Cause>,
    pub moved_fallback: Option<bool>,
    pub causes: Vec<anyhow::Error>,
    pub io: Option<BatchIoCollected>,
    pub native_state: Option<super::RuntimeState>,
    pub native_stop: Option<StopEvidence>,
    pub runtime: Option<super::Runtime>,
    pub managed: Option<super::ManagedRuntime>,
    pub notification: Option<NotificationState>,
    /// Includes the native scope, original terminal and any counter-observation.
    pub work: Option<Arc<WorkObservation>>,
}
impl Failure {
    pub fn unmoved(error: anyhow::Error) -> Box<Self> {
        Box::new(Self {
            first_cause: None,
            moved_fallback: None,
            causes: vec![error],
            io: None,
            native_state: None,
            native_stop: None,
            runtime: None,
            managed: None,
            notification: None,
            work: None,
        })
    }
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("runtime operation failed; bounded diagnostics retained; reconciliation remains required")
    }
}
impl std::fmt::Debug for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchFailure")
            .field("first_cause", &self.first_cause)
            .field("causes", &self.causes.len())
            .field("io_retained", &self.io.is_some())
            .field("native_state_retained", &self.native_state.is_some())
            .field("native_stop_retained", &self.native_stop.is_some())
            .field("runtime_anchor_retained", &self.runtime.is_some())
            .field("managed_binding_retained", &self.managed.is_some())
            .field("work_observations_retained", &self.work.is_some())
            .field("notification", &self.notification)
            .finish()
    }
}
impl std::error::Error for Failure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.causes
            .first()
            .map(|cause| cause.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// Complete owned blocking work without blocking cooperative polling. Some
/// callers check then spawn in the same non-awaiting unit: this join can own an
/// emitted operation, so the owning call retains it without timeout/abort.
pub(super) async fn checked_blocking<T, F>(check: F) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
{
    use anyhow::Context;
    let runtime = tokio::runtime::Handle::try_current()
        .context("owned runtime verification needs a Tokio runtime")?;
    let current = runtime.clone();
    let joined = runtime.spawn_blocking(move || {
        let _entered = current.enter();
        check()
    });
    joined
        .await
        .context("owned runtime invocation verification did not complete")?
}
