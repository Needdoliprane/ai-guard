//! Native STOP cells. Their actual OS handles and constructed Tokio contexts
//! precede confirmation; a permit or a detached spawn is not that receipt.
use super::*;
use crate::broker::enforcer::native_stop_failure::CauseTarget;
use std::sync::mpsc::{self, SyncSender};

pub(super) struct NativeStopWorker {
    // No retirement right is created here. The exact target retains this
    // handle even when startup, native IO or the foreground notification fails.
    _handle: std::thread::JoinHandle<()>,
}
impl NativeStopWorker {
    pub(super) fn is_live(&self) -> bool {
        !self._handle.is_finished()
    }
}

pub(super) fn ensure(actor: Arc<StopActor>, clock: Option<Arc<NativeHostClock>>) -> bool {
    if actor.worker_owned.load(Ordering::SeqCst) {
        return false;
    }
    let Ok(mut next) = actor.next_start.lock() else {
        return false;
    };
    if *next > Instant::now()
        || actor
            .worker_owned
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
    {
        return false;
    }
    *next = Instant::now() + Duration::from_secs(1);
    drop(next);
    if install(Arc::clone(&actor), clock).is_err() {
        failed(&actor);
        // Only a failed spawn has no native owner. A runtime constructor or
        // OS call in an installed thread never permits a replacement thread.
        if actor
            .native_worker
            .lock()
            .is_ok_and(|owner| owner.is_none())
        {
            actor.worker_owned.store(false, Ordering::SeqCst);
        }
    }
    true
}

fn failed(actor: &StopActor) {
    if let Ok(mut value) = actor.value.lock() {
        value.phase = StopPhaseV1::Quarantined;
        value.stop_failures = value.stop_failures.saturating_add(1);
    }
}
fn target(actor: &StopActor) -> CauseTarget {
    let value = actor.value.lock().unwrap_or_else(|p| p.into_inner());
    CauseTarget::fixed(actor.registration.arm_id(), &value.registration_hash)
}

fn install(actor: Arc<StopActor>, clock: Option<Arc<NativeHostClock>>) -> EnforcerResult<()> {
    #[cfg(test)]
    if actor.fail_spawn_once.swap(false, Ordering::SeqCst) {
        record_fault(&actor.fault_marker);
        return Err(EnforcerError::Unavailable);
    }
    let (start, started) = mpsc::sync_channel(1);
    let worker_actor = Arc::clone(&actor);
    let handle = std::thread::Builder::new()
        .name("enforcer-recovered-stop".into())
        .stack_size(1024 * 1024)
        .spawn(move || {
            let actor = worker_actor;
            // The start channel only orders handle installation. It carries
            // no permission to start a business process or restore Full/Q.
            if started.recv().is_err() {
                return;
            }
            #[cfg(test)]
            record_started(&actor.started_marker);
            let runtime = loop {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => break runtime,
                    Err(error) => {
                        actor.failures.context_failure(target(&actor), error);
                        failed(&actor);
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            };
            actor.native_ready.store(true, Ordering::SeqCst);
            let managed = loop {
                match actor.failures.execute(
                    &runtime,
                    target(&actor),
                    crate::runtime::ManagedRuntime::reopen_native(
                        actor.registration.authority.clone(),
                    ),
                ) {
                    Some(settled) => break settled.value,
                    None => {
                        failed(&actor);
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            };
            let mut stopping = false;
            let mut last_ns = None;
            loop {
                stopping |= stop_due(&actor.registration, clock.as_deref(), &mut last_ns)
                    || actor.stop.load(Ordering::SeqCst);
                if let Ok(mut value) = actor.value.lock() {
                    value.phase = if stopping {
                        StopPhaseV1::StopRequested
                    } else {
                        StopPhaseV1::StopReady
                    };
                }
                if stopping {
                    let result = actor.failures.execute(
                        &runtime,
                        target(&actor),
                        managed.stop_snapshot_owned_native(&actor.registration.owned),
                    );
                    let failed = match result {
                        Some(settled) => {
                            actor
                                .failures
                                .observed_stop(target(&actor), settled.value.observation);
                            if let Some(cause) = settled.value.kill_failure {
                                actor
                                    .failures
                                    .returned(target(&actor), cause, settled.proof);
                                true
                            } else {
                                false
                            }
                        }
                        None => true,
                    };
                    if let Ok(mut value) = actor.value.lock() {
                        value.phase = StopPhaseV1::Quarantined;
                        if failed {
                            value.stop_failures = value.stop_failures.saturating_add(1);
                        }
                    }
                }
                // A stopped observation never drains an older start. Keep
                // this exact immutable snapshot/context and original deadline.
                std::thread::sleep(Duration::from_millis(250));
            }
        })
        .map_err(|error| {
            actor.failures.context_failure(target(&actor), error);
            EnforcerError::Unavailable
        })?;
    *actor
        .native_worker
        .lock()
        .map_err(|_| EnforcerError::Unhealthy)? = Some(NativeStopWorker { _handle: handle });
    #[cfg(test)]
    record_marker(
        &actor
            .started_marker
            .as_ref()
            .map(|path| path.with_extension("spawned")),
    );
    // The transport never waits for native startup. Its caller returns
    // NotReady until the native thread itself publishes runtime installation.
    // This preserves another target's Cancel/transport progress while an OS
    // resource or constructor is unavailable.
    start.try_send(()).map_err(|_| EnforcerError::Unavailable)
}

fn stop_due(
    registration: &RegistrationV1,
    clock: Option<&NativeHostClock>,
    last_ns: &mut Option<u128>,
) -> bool {
    !clock
        .and_then(|clock| clock.sample().ok())
        .is_some_and(|sample| {
            let Ok(now) = sample.continuous_ns() else {
                return false;
            };
            let monotone = last_ns.is_none_or(|last| now >= last);
            *last_ns = Some(now);
            monotone && registration.arm.validate_clock_check(&sample).is_ok()
        })
}

/// One W cell is reserved for real bounded catch-up attempts. It has a real
/// runtime and retained handle before the role is published. An attempt does
/// not issue an active-coverage ACK, retire a target or release its quota.
#[derive(Clone)]
pub(in crate::broker::enforcer) struct RecoveryAttemptOwner(Arc<AttemptState>);
enum AttemptLease {
    Sealed {
        _lease: EnforcerRoleLeaseV2,
    },
    Legacy {
        _lease: Arc<crate::broker::discovery::RoleLease>,
    },
}
// Both variants carry large durable stop receipts; keep value semantics.
#[allow(clippy::large_enum_variant)]
enum AttemptRequest {
    Recovery(RestoreRequiredStopV1),
    // No source, ledger identity or durable Cancel is fabricated for V1.
    // This is an exact, stop-only bounded attempt after native fencing.
    Legacy {
        registration: RegistrationV1,
        digest: String,
    },
}
impl AttemptRequest {
    fn registration(&self) -> &RegistrationV1 {
        match self {
            Self::Recovery(value) => &value.registration,
            Self::Legacy { registration, .. } => registration,
        }
    }
    fn forced_stop(&self) -> bool {
        match self {
            Self::Recovery(value) => value.durable_cancel.is_some(),
            Self::Legacy { .. } => true,
        }
    }
    fn digest(&self) -> &str {
        match self {
            Self::Recovery(value) => &value.registration_hash,
            Self::Legacy { digest, .. } => digest,
        }
    }
}
struct AttemptState {
    sender: SyncSender<Arc<AttemptRequest>>,
    // Keep queued snapshots under the same upper owner after native panic or
    // return. A closure-local Receiver would drop this real queue on unwind.
    receiver: Mutex<mpsc::Receiver<Arc<AttemptRequest>>>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    ready: AtomicBool,
    stopping: AtomicBool,
    outstanding: AtomicU64,
    normal_idle_exit: AtomicBool,
    idle_joined: AtomicBool,
    native_failed: Option<Arc<AtomicBool>>,
    // The historical sealed transport has no NativeOwners at construction.
    // Its single W context therefore pins this ACTUAL lease/handle/queue cell
    // until process exit or verified idle closure. Only actual joins can
    // release this pin; a timeout or a retirement receipt cannot do so.
    sealed_lease_pin: Mutex<Option<Arc<AttemptState>>>,
    attempts: AtomicU64,
    failures: AtomicU64,
    cause_history: Arc<NativeStopFailures>,
    // The target currently in native IO remains visible and owned if a call
    // never returns. Timeout of a transport waiter cannot clear this value.
    current: Mutex<Option<Arc<AttemptRequest>>>,
    _lease: AttemptLease,
    #[cfg(test)]
    before_attempt_pause: AtomicBool,
    #[cfg(test)]
    native_panic: Mutex<Option<Arc<()>>>,
}
impl RecoveryAttemptOwner {
    pub(super) fn failure_facts(&self) -> super::super::native_stop_failure::FailureFacts {
        self.0.cause_history.facts()
    }
    pub(in crate::broker::enforcer) fn observations(&self) -> (u64, u64, bool) {
        (
            self.0.attempts.load(Ordering::SeqCst),
            self.0.failures.load(Ordering::SeqCst),
            self.0
                .current
                .lock()
                .map_or(true, |current| current.is_some()),
        )
    }
    pub(in crate::broker::enforcer) fn install(
        lease: EnforcerRoleLeaseV2,
        clock: Option<Arc<NativeHostClock>>,
    ) -> EnforcerResult<Self> {
        Self::install_owned(AttemptLease::Sealed { _lease: lease }, clock, None)
    }
    pub(in crate::broker::enforcer) fn install_legacy(
        lease: Arc<crate::broker::discovery::RoleLease>,
        clock: Arc<NativeHostClock>,
        owners: &crate::broker::enforcer::server::lifetime::NativeOwners,
    ) -> EnforcerResult<Self> {
        lease.revalidate()?;
        Self::install_owned(
            AttemptLease::Legacy { _lease: lease },
            Some(clock),
            Some(owners),
        )
    }
    fn install_owned(
        lease: AttemptLease,
        clock: Option<Arc<NativeHostClock>>,
        owners: Option<&crate::broker::enforcer::server::lifetime::NativeOwners>,
    ) -> EnforcerResult<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Arc<AttemptRequest>>(1);
        let state = Arc::new(AttemptState {
            sender,
            receiver: Mutex::new(receiver),
            handle: Mutex::new(None),
            ready: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            outstanding: AtomicU64::new(0),
            normal_idle_exit: AtomicBool::new(false),
            idle_joined: AtomicBool::new(false),
            native_failed: owners.map(|owner| owner.failure_latch()),
            sealed_lease_pin: Mutex::new(None),
            attempts: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            cause_history: NativeStopFailures::new(),
            current: Mutex::new(None),
            _lease: lease,
            #[cfg(test)]
            before_attempt_pause: AtomicBool::new(false),
            #[cfg(test)]
            native_panic: Mutex::new(None),
        });
        if let Some(owners) = owners {
            // Actual cell/queue/custody, not a diagnostic or JoinHandle alone.
            // This precedes spawn as well as every possible runtime/panic exit.
            owners.retain_legacy_catchup(Self(Arc::clone(&state)))?;
        } else {
            *state
                .sealed_lease_pin
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)? = Some(Arc::clone(&state));
        }
        let (start, started) = mpsc::sync_channel(1);
        let (ready, installed) = mpsc::sync_channel(1);
        let worker = Arc::clone(&state);
        let handle = std::thread::Builder::new()
            .name("enforcer-recovery-attempt".into())
            .stack_size(1024 * 1024)
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if started.recv().is_err() {
                        return;
                    }
                    let runtime = loop {
                        match tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                        {
                            Ok(runtime) => break runtime,
                            Err(error) => {
                                worker
                                    .cause_history
                                    .context_failure(CauseTarget::context(), error);
                                worker.failures.fetch_add(1, Ordering::SeqCst);
                                std::thread::sleep(Duration::from_secs(1));
                            }
                        }
                    };
                    worker.ready.store(true, Ordering::SeqCst);
                    let _ = ready.try_send(());
                    loop {
                        if worker.stopping.load(Ordering::SeqCst)
                            && worker.outstanding.load(Ordering::SeqCst) == 0
                        {
                            break;
                        }
                        let request = match worker
                            .receiver
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .recv_timeout(Duration::from_millis(25))
                        {
                            Ok(request) => request,
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        };
                        // Keep the full exact request/Cancel while all runtime IO
                        // is pending. The control cache retains its original proof.
                        let Ok(mut current) = worker.current.lock() else {
                            return;
                        };
                        *current = Some(Arc::clone(&request));
                        drop(current);
                        #[cfg(test)]
                        {
                            while worker.before_attempt_pause.load(Ordering::SeqCst) {
                                std::thread::sleep(Duration::from_millis(10));
                            }
                            if let Some(payload) = worker.native_panic.lock().unwrap().take() {
                                std::panic::panic_any(payload);
                            }
                        }
                        let mut last_ns = None;
                        let registration = request.registration();
                        let due = request.forced_stop()
                            || stop_due(registration, clock.as_deref(), &mut last_ns);
                        let target = CauseTarget::retained(
                            registration.arm_id(),
                            request.digest(),
                            Arc::clone(&request),
                        );
                        let failed = if due {
                            match worker.cause_history.execute(
                                &runtime,
                                target.clone(),
                                crate::runtime::ManagedRuntime::reopen_native(
                                    registration.authority.clone(),
                                ),
                            ) {
                                Some(managed) => {
                                    match worker.cause_history.execute(
                                        &runtime,
                                        target.clone(),
                                        managed
                                            .value
                                            .stop_snapshot_owned_native(&registration.owned),
                                    ) {
                                        Some(settled) => {
                                            worker.cause_history.observed_stop(
                                                target.clone(),
                                                settled.value.observation,
                                            );
                                            if let Some(cause) = settled.value.kill_failure {
                                                worker.cause_history.returned(
                                                    target.clone(),
                                                    cause,
                                                    settled.proof,
                                                );
                                                true
                                            } else {
                                                false
                                            }
                                        }
                                        None => true,
                                    }
                                }
                                None => true,
                            }
                        } else {
                            false
                        };
                        worker.attempts.fetch_add(1, Ordering::SeqCst);
                        if failed {
                            worker.failures.fetch_add(1, Ordering::SeqCst);
                        }
                        // This is reached only after the real runtime future has
                        // returned/collected. It releases an attempt, not an Arm.
                        if let Ok(mut current) = worker.current.lock() {
                            *current = None;
                        }
                        worker.outstanding.fetch_sub(1, Ordering::SeqCst);
                    }
                    if worker.stopping.load(Ordering::SeqCst)
                        && worker.outstanding.load(Ordering::SeqCst) == 0
                    {
                        worker.normal_idle_exit.store(true, Ordering::SeqCst);
                    }
                }));
                worker.ready.store(false, Ordering::SeqCst);
                if let Err(payload) = result {
                    worker.cause_history.native_thread_failed(payload);
                    worker.failures.fetch_add(1, Ordering::SeqCst);
                }
                if !worker.normal_idle_exit.load(Ordering::SeqCst)
                    && let Some(failed) = &worker.native_failed
                {
                    failed.store(true, Ordering::SeqCst);
                }
            })
            .map_err(|error| {
                state
                    .cause_history
                    .context_failure(CauseTarget::context(), error);
                EnforcerError::Unavailable
            })?;
        *state.handle.lock().map_err(|_| EnforcerError::Unhealthy)? = Some(handle);
        start.send(()).map_err(|_| EnforcerError::Unavailable)?;
        // A failed notification keeps the actual native thread/lease/handle.
        installed
            .recv_timeout(IO_TIMEOUT)
            .map_err(|_| EnforcerError::NotReady)?;
        Ok(Self(state))
    }
    pub(super) fn try_schedule(&self, request: RestoreRequiredStopV1) -> EnforcerResult<()> {
        self.try_schedule_owned(AttemptRequest::Recovery(request))
    }
    pub(in crate::broker::enforcer) fn try_schedule_legacy(
        &self,
        registration: RegistrationV1,
    ) -> EnforcerResult<()> {
        let digest = registration.digest()?;
        self.try_schedule_owned(AttemptRequest::Legacy {
            registration,
            digest,
        })
    }
    fn try_schedule_owned(&self, request: AttemptRequest) -> EnforcerResult<()> {
        self.try_schedule_arc(Arc::new(request))
    }
    fn try_schedule_arc(&self, request: Arc<AttemptRequest>) -> EnforcerResult<()> {
        if !self.0.ready.load(Ordering::SeqCst) || self.0.stopping.load(Ordering::SeqCst) {
            return Err(EnforcerError::NotReady);
        }
        if !self
            .0
            .handle
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
        {
            return Err(EnforcerError::Unhealthy);
        }
        self.0.outstanding.fetch_add(1, Ordering::SeqCst);
        if self.0.stopping.load(Ordering::SeqCst) {
            self.0.outstanding.fetch_sub(1, Ordering::SeqCst);
            return Err(EnforcerError::Unavailable);
        }
        let result = self
            .0
            .sender
            .try_send(request)
            .map_err(|_| EnforcerError::Capacity);
        if result.is_err() {
            self.0.outstanding.fetch_sub(1, Ordering::SeqCst);
        }
        result
    }
    pub(in crate::broker::enforcer) fn reserve_legacy_idle_shutdown(&self) -> EnforcerResult<()> {
        if !matches!(self.0._lease, AttemptLease::Legacy { .. })
            || self.0.outstanding.load(Ordering::SeqCst) != 0
        {
            return Err(EnforcerError::NotIdle);
        }
        self.0.stopping.store(true, Ordering::SeqCst);
        if self.0.outstanding.load(Ordering::SeqCst) != 0 {
            return Err(EnforcerError::NotIdle);
        }
        Ok(())
    }
    pub(in crate::broker::enforcer) async fn finish_legacy_idle(&self) -> EnforcerResult<()> {
        self.join_idle().await
    }

    /// The caller holds the shared admission map. All real W submissions
    /// acquire that same lock, so an idle reservation cannot miss queued work.
    pub(in crate::broker::enforcer) fn check_sealed_idle(&self) -> EnforcerResult<()> {
        if !matches!(self.0._lease, AttemptLease::Sealed { .. })
            || !self.0.ready.load(Ordering::SeqCst)
            || self.0.stopping.load(Ordering::SeqCst)
            || !self
                .0
                .handle
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .as_ref()
                .is_some_and(|handle| !handle.is_finished())
        {
            return Err(EnforcerError::Unhealthy);
        }
        if self.0.outstanding.load(Ordering::SeqCst) != 0
            || self.0.cause_history.facts().uncollected_io
            || self
                .0
                .current
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .is_some()
        {
            return Err(EnforcerError::NotIdle);
        }
        Ok(())
    }

    /// Same admission lock, after the irreversible idle transition won.
    pub(in crate::broker::enforcer) fn seal_idle(&self) {
        self.0.stopping.store(true, Ordering::SeqCst);
    }

    pub(super) async fn join_idle(&self) -> EnforcerResult<()> {
        if !self.0.stopping.load(Ordering::SeqCst) || self.0.outstanding.load(Ordering::SeqCst) != 0
        {
            return Err(EnforcerError::NotIdle);
        }
        let handle = self
            .0
            .handle
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .take()
            .ok_or(EnforcerError::Unhealthy)?;
        tokio::task::spawn_blocking(move || handle.join().map_err(|_| EnforcerError::Unhealthy))
            .await
            .map_err(|_| EnforcerError::Unhealthy)??;
        self.0.idle_joined.store(true, Ordering::SeqCst);
        if !self.known_idle_joined() {
            return Err(EnforcerError::Unhealthy);
        }
        Ok(())
    }

    /// Release only after the same sealed W and Full owners have joined.
    pub(super) fn release_joined_sealed_idle(&self) -> EnforcerResult<()> {
        if !self.known_idle_joined() {
            return Err(EnforcerError::Unhealthy);
        }
        let mut pin = self
            .0
            .sealed_lease_pin
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        if !pin
            .as_ref()
            .is_some_and(|owner| Arc::ptr_eq(owner, &self.0))
        {
            return Err(EnforcerError::Unhealthy);
        }
        drop(pin.take());
        Ok(())
    }

    pub(in crate::broker::enforcer) fn known_idle_joined(&self) -> bool {
        self.0.idle_joined.load(Ordering::SeqCst)
            && self.0.normal_idle_exit.load(Ordering::SeqCst)
            && self.0.outstanding.load(Ordering::SeqCst) == 0
            && self.0.current.lock().is_ok_and(|current| current.is_none())
            && self.0.handle.lock().is_ok_and(|handle| handle.is_none())
            && !self.0.cause_history.facts().uncollected_io
    }
    #[cfg(test)]
    pub(in crate::broker::enforcer) fn fixture_pause_attempt(&self, pause: bool) {
        self.0.before_attempt_pause.store(pause, Ordering::SeqCst);
    }
    #[cfg(test)]
    pub(in crate::broker::enforcer) fn fixture_native_panic(&self, payload: Arc<()>) {
        *self.0.native_panic.lock().unwrap() = Some(payload);
    }
    #[cfg(test)]
    pub(in crate::broker::enforcer) fn fixture_cause_weak(
        &self,
    ) -> std::sync::Weak<NativeStopFailures> {
        Arc::downgrade(&self.0.cause_history)
    }
    #[cfg(test)]
    pub(in crate::broker::enforcer) fn fixture_schedule_legacy(
        &self,
        registration: RegistrationV1,
    ) -> std::sync::Weak<dyn std::any::Any + Send + Sync> {
        let digest = registration.digest().unwrap();
        let request = Arc::new(AttemptRequest::Legacy {
            registration,
            digest,
        });
        let opaque: Arc<dyn std::any::Any + Send + Sync> = request.clone();
        let weak = Arc::downgrade(&opaque);
        self.try_schedule_arc(request).unwrap();
        weak
    }
    #[cfg(test)]
    pub(in crate::broker::enforcer) fn fixture_thread_finished(&self) -> bool {
        self.0
            .handle
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
    }
}
