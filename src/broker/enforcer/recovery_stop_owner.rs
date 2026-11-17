//! Native STOP cells. Their actual OS handles and constructed Tokio contexts
//! precede confirmation; a permit or a detached spawn is not that receipt.
use super::*;
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
                    Err(_) => {
                        failed(&actor);
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            };
            actor.native_ready.store(true, Ordering::SeqCst);
            let managed = loop {
                match runtime.block_on(crate::runtime::ManagedRuntime::reopen(
                    actor.registration.authority.clone(),
                )) {
                    Ok(managed) => break managed,
                    Err(_) => {
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
                    let result =
                        runtime.block_on(managed.stop_snapshot_owned(&actor.registration.owned));
                    if let Ok(mut value) = actor.value.lock() {
                        value.phase = StopPhaseV1::Quarantined;
                        if result.is_err() {
                            value.stop_failures = value.stop_failures.saturating_add(1);
                        }
                    }
                }
                // A stopped observation never drains an older start. Keep
                // this exact immutable snapshot/context and original deadline.
                std::thread::sleep(Duration::from_millis(250));
            }
        })
        .map_err(|_| EnforcerError::Unavailable)?;
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
pub(super) struct RecoveryAttemptOwner(Arc<AttemptState>);
struct AttemptState {
    sender: SyncSender<RestoreRequiredStopV1>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    ready: AtomicBool,
    attempts: AtomicU64,
    failures: AtomicU64,
    // The target currently in native IO remains visible and owned if a call
    // never returns. Timeout of a transport waiter cannot clear this value.
    current: Mutex<Option<Arc<RestoreRequiredStopV1>>>,
    _lease: EnforcerRoleLeaseV2,
}
impl RecoveryAttemptOwner {
    pub(super) fn observations(&self) -> (u64, u64, bool) {
        (
            self.0.attempts.load(Ordering::SeqCst),
            self.0.failures.load(Ordering::SeqCst),
            self.0
                .current
                .lock()
                .map_or(true, |current| current.is_some()),
        )
    }
    pub(super) fn install(
        lease: EnforcerRoleLeaseV2,
        clock: Option<Arc<NativeHostClock>>,
    ) -> EnforcerResult<Self> {
        let (sender, receiver) = mpsc::sync_channel::<RestoreRequiredStopV1>(1);
        let state = Arc::new(AttemptState {
            sender,
            handle: Mutex::new(None),
            ready: AtomicBool::new(false),
            attempts: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            current: Mutex::new(None),
            _lease: lease,
        });
        let (start, started) = mpsc::sync_channel(1);
        let (ready, installed) = mpsc::sync_channel(1);
        let worker = Arc::clone(&state);
        let handle = std::thread::Builder::new()
            .name("enforcer-recovery-attempt".into())
            .stack_size(1024 * 1024)
            .spawn(move || {
                if started.recv().is_err() {
                    return;
                }
                let runtime = loop {
                    match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(runtime) => break runtime,
                        Err(_) => {
                            worker.failures.fetch_add(1, Ordering::SeqCst);
                            std::thread::sleep(Duration::from_secs(1));
                        }
                    }
                };
                worker.ready.store(true, Ordering::SeqCst);
                let _ = ready.try_send(());
                while let Ok(request) = receiver.recv() {
                    let request = Arc::new(request);
                    // Keep the full exact request/Cancel while all runtime IO
                    // is pending. The control cache retains its original proof.
                    let Ok(mut current) = worker.current.lock() else {
                        return;
                    };
                    *current = Some(Arc::clone(&request));
                    drop(current);
                    let mut last_ns = None;
                    let due = request.durable_cancel.is_some()
                        || stop_due(&request.registration, clock.as_deref(), &mut last_ns);
                    let result = if due {
                        runtime.block_on(async {
                            let managed = crate::runtime::ManagedRuntime::reopen(
                                request.registration.authority.clone(),
                            )
                            .await?;
                            managed
                                .stop_snapshot_owned(&request.registration.owned)
                                .await
                                .map(|_| ())
                        })
                    } else {
                        Ok(())
                    };
                    worker.attempts.fetch_add(1, Ordering::SeqCst);
                    if result.is_err() {
                        worker.failures.fetch_add(1, Ordering::SeqCst);
                    }
                    // This is reached only after the real runtime future has
                    // returned/collected. It releases an attempt, not an Arm.
                    if let Ok(mut current) = worker.current.lock() {
                        *current = None;
                    }
                }
            })
            .map_err(|_| EnforcerError::Unavailable)?;
        *state.handle.lock().map_err(|_| EnforcerError::Unhealthy)? = Some(handle);
        start.send(()).map_err(|_| EnforcerError::Unavailable)?;
        // A failed notification keeps the actual native thread/lease/handle.
        installed
            .recv_timeout(IO_TIMEOUT)
            .map_err(|_| EnforcerError::NotReady)?;
        Ok(Self(state))
    }
    pub(super) fn try_schedule(&self, request: RestoreRequiredStopV1) -> EnforcerResult<()> {
        if !self.0.ready.load(Ordering::SeqCst) {
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
        self.0
            .sender
            .try_send(request)
            .map_err(|_| EnforcerError::Capacity)
    }
}
