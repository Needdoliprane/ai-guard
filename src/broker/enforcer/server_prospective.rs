//! Legacy native admission. DATA can rediscover an owner, never manufacture
//! one. Every prospective, bound or restored target consumes this ONE registry.
//! The real native thread/runtime/key and failure custody survive Bind.
use super::*;
use lifetime::ActorKey;
use std::sync::Condvar;

#[cfg(test)]
#[derive(Default)]
pub(super) struct NativeIdentity {
    pub(super) prospective: Option<(std::thread::ThreadId, usize)>,
    pub(super) bound: Option<(std::thread::ThreadId, usize)>,
}
#[cfg(test)]
pub(super) fn observe_state(state: &Arc<State>) {
    tests::observe_state(state);
}
#[cfg(test)]
pub(super) fn fixture_registration(root: &std::path::Path) -> RegistrationV1 {
    tests::registration(root)
}

pub(super) struct LegacyAdmission {
    pub(super) generation: u64,
    pub(super) entries: BTreeSet<String>,
}
impl LegacyAdmission {
    pub(super) fn new(fresh: bool) -> Self {
        Self {
            generation: u64::from(!fresh),
            entries: BTreeSet::new(),
        }
    }
}

#[derive(Clone, Copy)]
enum ExchangePhase {
    NotIssued,
    MayHaveIssued { original_nonce: Uuid },
}
struct PendingState {
    bound: Option<Arc<Actor>>,
    stop_reserved: bool,
    stop_failures: u64,
    exchange: ExchangePhase,
}
pub(super) struct ProspectiveActor {
    prospective: ProspectiveRegistrationV1,
    reservation: ProspectiveReservationV1,
    key: ActorKey,
    state: Mutex<PendingState>,
    wake: Condvar,
    worker_owned: AtomicBool,
    runtime_installed: Arc<AtomicBool>,
    failures: Arc<NativeStopFailures>,
    owners: NativeOwners,
    #[cfg(test)]
    native_identity: Arc<Mutex<NativeIdentity>>,
    #[cfg(test)]
    pub(super) test_exit: Arc<AtomicBool>,
}
impl ProspectiveActor {
    #[cfg(test)]
    pub(super) fn fixture_exit_key(&self) -> ActorKey {
        self.test_exit.store(true, Ordering::SeqCst);
        self.wake.notify_all();
        self.key
    }
    fn cause_target(&self) -> CauseTarget {
        CauseTarget::fixed(
            self.prospective.arm_id(),
            &self.reservation.prospective_hash,
        )
    }
    fn failed(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.stop_failures = state.stop_failures.saturating_add(1);
            if let Some(bound) = &state.bound {
                actor_failed(bound);
            }
        }
    }
    fn reserve_stop(&self) -> EnforcerResult<()> {
        // Set the latch while holding the same pending mutex used by Bind.
        // A concurrent transfer therefore cannot escape its exact STOP target.
        let mut value = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        value.stop_reserved = true;
        if let Some(actor) = &value.bound {
            actor.stop.store(true, Ordering::SeqCst);
            let mut bound = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
            bound.prepared_once = true;
        }
        self.wake.notify_all();
        Ok(())
    }
    fn validate_reservation(
        &self,
        reservation: &ProspectiveReservationV1,
        epoch: Uuid,
    ) -> EnforcerResult<()> {
        reservation.validate_for(&self.prospective, epoch, self.reservation.cancel_generation)?;
        if reservation.reservation_nonce != self.reservation.reservation_nonce
            || reservation.admission_generation != self.reservation.admission_generation
        {
            return Err(EnforcerError::Conflict);
        }
        Ok(())
    }
    fn start(self: &Arc<Self>, clock: Arc<NativeHostClock>) -> EnforcerResult<()> {
        if self
            .worker_owned
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(());
        }
        let work = Arc::clone(self);
        let (retained, start) = std::sync::mpsc::sync_channel(1);
        let handle = match std::thread::Builder::new()
            .name("enforcer-legacy-prospective".into())
            .stack_size(1024 * 1024)
            .spawn(move || {
                if start.recv().is_ok() {
                    work.run(clock);
                } else {
                    work.failed();
                    work.owners.actor_exited();
                }
            }) {
            Ok(handle) => handle,
            Err(error) => {
                self.failures.context_failure(self.cause_target(), error);
                // No actual handle exists on failed spawn. The registry entry
                // remains occupied; only this same target can retry startup.
                self.worker_owned.store(false, Ordering::SeqCst);
                self.failed();
                return Err(EnforcerError::Unavailable);
            }
        };
        self.owners.actor_keyed(self.key, handle)?;
        retained.send(()).map_err(|_| EnforcerError::Unhealthy)
    }
    fn run(self: Arc<Self>, clock: Arc<NativeHostClock>) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let runtime = loop {
                #[cfg(test)]
                if self.test_exit.load(Ordering::SeqCst) {
                    return Ok(());
                }
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => break runtime,
                    Err(error) => {
                        self.failures.context_failure(self.cause_target(), error);
                        self.failed();
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            };
            let runtime = Box::new(runtime);
            #[cfg(test)]
            if let Ok(mut identity) = self.native_identity.lock() {
                identity.prospective = Some((
                    std::thread::current().id(),
                    (&*runtime as *const tokio::runtime::Runtime) as usize,
                ));
            }
            self.runtime_installed.store(true, Ordering::SeqCst);
            self.run_installed(clock, runtime)
        }));
        self.runtime_installed.store(false, Ordering::SeqCst);
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.failures.native_thread_failed(Box::new(error));
                self.failed();
                self.owners.actor_exited();
            }
            Err(payload) => {
                self.failures.native_thread_failed(payload);
                self.failed();
                self.owners.actor_exited();
            }
        }
    }
    fn run_installed(
        self: &Arc<Self>,
        clock: Arc<NativeHostClock>,
        runtime: Box<tokio::runtime::Runtime>,
    ) -> EnforcerResult<()> {
        let mut last_ns = None;
        let mut stopping = false;
        let mut managed = None;
        loop {
            #[cfg(test)]
            if self.test_exit.load(Ordering::SeqCst) {
                return Ok(());
            }
            let live = clock.sample().ok().is_some_and(|sample| {
                sample.continuous_ns().is_ok_and(|now| {
                    let monotone = last_ns.is_none_or(|last| now >= last);
                    last_ns = Some(now);
                    monotone && self.prospective.arm.validate_clock_check(&sample).is_ok()
                })
            });
            let bound = {
                let mut value = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
                stopping |= !live || value.stop_reserved;
                value.stop_reserved |= stopping;
                value.bound.clone()
            };
            if let Some(bound) = bound {
                // The exact original key, constructed runtime and raw failure
                // custody move to this continuation; Bind never starts an actor.
                drop(managed);
                run_bound_actor(&bound, clock, runtime);
                return Ok(());
            }
            if stopping {
                if managed.is_none() {
                    match self.failures.execute(
                        &runtime,
                        self.cause_target(),
                        ManagedRuntime::reopen_native(self.prospective.authority.clone()),
                    ) {
                        Some(settled) => managed = Some(settled.value),
                        None => self.failed(),
                    }
                }
                if let Some(managed) = &managed {
                    match self.failures.execute(
                        &runtime,
                        self.cause_target(),
                        managed.stop_snapshot_owned_native(&self.prospective.owned),
                    ) {
                        Some(settled) => {
                            self.failures
                                .observed_stop(self.cause_target(), settled.value.observation);
                            if let Some(cause) = settled.value.kill_failure {
                                self.failures
                                    .returned(self.cause_target(), cause, settled.proof);
                                self.failed();
                            }
                        }
                        None => self.failed(),
                    }
                }
            }
            let value = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
            let _waiting = self
                .wake
                .wait_timeout(value, Duration::from_millis(250))
                .map_err(|_| EnforcerError::Unhealthy)?;
        }
    }
}

impl State {
    pub(super) fn owned_active(&self) -> EnforcerResult<Option<u32>> {
        if self.owners.failed() {
            return Ok(None);
        }
        let mut installed = BTreeSet::new();
        for actor in self
            .slots
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .values()
        {
            if actor.runtime_installed.load(Ordering::SeqCst) {
                installed.insert(actor.registration.arm_id().to_owned());
            }
        }
        for actor in self
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .values()
        {
            if actor.runtime_installed.load(Ordering::SeqCst) {
                installed.insert(actor.prospective.arm_id().to_owned());
            }
        }
        Ok(Some(
            u32::try_from(installed.len()).map_err(|_| EnforcerError::Unhealthy)?,
        ))
    }
    pub(super) fn permanent_capacity(&self) -> usize {
        self.max_slots as usize + self.max_recovery_stop_owners.saturating_sub(1) as usize
    }
    pub(super) fn check_original(&self, admission: &LegacyAdmission) -> EnforcerResult<()> {
        if self.recovery_requested.load(Ordering::SeqCst) || admission.generation != 0 {
            return Err(EnforcerError::NotReady);
        }
        if self.closing.load(Ordering::SeqCst) || !self.admission_healthy() {
            return Err(EnforcerError::Unhealthy);
        }
        Ok(())
    }
    pub(super) fn begin_recovery(&self) -> EnforcerResult<u64> {
        // Restrictive intent precedes the lock: every post-Ready recheck sees
        // it even if the transport waiter disappears before the native commit.
        self.recovery_requested.store(true, Ordering::SeqCst);
        let mut admission = self
            .admission
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        admission.generation = admission.generation.max(1);
        for actor in self
            .slots
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .values()
        {
            actor.stop.store(true, Ordering::SeqCst);
            actor
                .state
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .prepared_once = true;
        }
        for actor in self
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .values()
        {
            actor.reserve_stop()?;
        }
        // The ACK commits the entire retained cohort, never only a flag.
        Ok(admission.generation)
    }
    fn pending(
        &self,
        reservation: &ProspectiveReservationV1,
    ) -> EnforcerResult<Arc<ProspectiveActor>> {
        let actor = self
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .get(&reservation.arm_id)
            .cloned()
            .ok_or(EnforcerError::Conflict)?;
        actor.validate_reservation(reservation, self.epoch)?;
        Ok(actor)
    }
    pub(super) fn cancel_prospective(
        &self,
        reservation: &ProspectiveReservationV1,
        generation: u64,
    ) -> EnforcerResult<()> {
        let _admission = self
            .admission
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        let actor = self.pending(reservation)?;
        if generation < actor.reservation.cancel_generation {
            return Err(EnforcerError::Conflict);
        }
        actor.reserve_stop()?;
        if let Some(bound) = &actor
            .state
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .bound
        {
            let mut value = bound.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
            value.generation = value.generation.max(generation);
        }
        Ok(())
    }
    pub(super) async fn reserve_prospective(
        self: &Arc<Self>,
        prospective: ProspectiveRegistrationV1,
        generation: u64,
        replay_only: bool,
    ) -> EnforcerResult<ProspectiveReservationV1> {
        prospective.validate()?;
        if generation == u64::MAX && !replay_only {
            return Err(EnforcerError::InvalidInput);
        }
        let digest = prospective.digest()?;
        let actor = {
            let mut admission = self
                .admission
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            let mut pending = self
                .prospective
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            if let Some(actor) = pending.get(prospective.arm_id()) {
                actor.reservation.validate_for(
                    &prospective,
                    self.epoch,
                    actor.reservation.cancel_generation,
                )?;
                if generation < actor.reservation.cancel_generation
                    || (!replay_only && generation != actor.reservation.cancel_generation)
                {
                    return Err(EnforcerError::Conflict);
                }
                if replay_only {
                    actor.reserve_stop()?;
                    let value = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
                    if let Some(bound) = &value.bound {
                        let mut status =
                            bound.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
                        status.generation = status.generation.max(generation);
                    }
                } else {
                    self.check_original(&admission)?;
                }
                Arc::clone(actor)
            } else {
                if replay_only {
                    return Err(EnforcerError::Conflict);
                }
                self.check_original(&admission)?;
                let sample = self.clock.sample().map_err(|_| EnforcerError::Unhealthy)?;
                prospective
                    .arm
                    .validate_clock_check(&sample)
                    .map_err(|_| EnforcerError::NotReady)?;
                if admission.entries.contains(prospective.arm_id()) {
                    return Err(EnforcerError::Conflict);
                }
                if admission.entries.len() >= self.max_slots as usize {
                    return Err(EnforcerError::Capacity);
                }
                let nonce = Uuid::new_v4();
                let reservation = ProspectiveReservationV1 {
                    epoch: self.epoch,
                    admission_generation: admission.generation,
                    cancel_generation: generation,
                    reservation_nonce: nonce,
                    arm_id: prospective.arm_id().to_owned(),
                    prospective_hash: digest,
                };
                reservation.validate_for(&prospective, self.epoch, generation)?;
                let actor = Arc::new(ProspectiveActor {
                    key: ActorKey {
                        arm_id: Uuid::parse_str(prospective.arm_id())
                            .map_err(|_| EnforcerError::InvalidInput)?,
                        epoch: self.epoch,
                        nonce,
                    },
                    prospective,
                    reservation,
                    state: Mutex::new(PendingState {
                        bound: None,
                        stop_reserved: false,
                        stop_failures: 0,
                        exchange: ExchangePhase::NotIssued,
                    }),
                    wake: Condvar::new(),
                    worker_owned: AtomicBool::new(false),
                    runtime_installed: Arc::new(AtomicBool::new(false)),
                    failures: NativeStopFailures::new(),
                    owners: self.owners.clone(),
                    #[cfg(test)]
                    native_identity: Arc::new(Mutex::new(NativeIdentity::default())),
                    #[cfg(test)]
                    test_exit: Arc::new(AtomicBool::new(false)),
                });
                admission
                    .entries
                    .insert(actor.prospective.arm_id().to_owned());
                pending.insert(actor.prospective.arm_id().to_owned(), Arc::clone(&actor));
                actor
            }
        };
        // A replay cannot manufacture the native resources that were absent
        // on its ambiguous first admission. It can only rediscover a retained
        // cell; a fresh retry may still retry a positively failed spawn.
        if !replay_only {
            actor.start(Arc::clone(&self.clock))?;
        }
        #[cfg(test)]
        tests::before_reserve_ack().await;
        let expires = Instant::now() + REGISTER_TIMEOUT - Duration::from_secs(1);
        loop {
            {
                let admission = self
                    .admission
                    .lock()
                    .map_err(|_| EnforcerError::Unhealthy)?;
                if !replay_only {
                    self.check_original(&admission)?;
                }
                if self.owners.failed() {
                    return Err(EnforcerError::Unhealthy);
                }
                if actor.runtime_installed.load(Ordering::SeqCst) {
                    if !replay_only {
                        let sample = self.clock.sample().map_err(|_| EnforcerError::Unhealthy)?;
                        actor
                            .prospective
                            .arm
                            .validate_clock_check(&sample)
                            .map_err(|_| EnforcerError::NotReady)?;
                        if actor
                            .state
                            .lock()
                            .map_err(|_| EnforcerError::Unhealthy)?
                            .stop_reserved
                        {
                            return Err(EnforcerError::NotReady);
                        }
                    }
                    return Ok(actor.reservation.clone());
                }
            }
            if Instant::now() >= expires {
                return Err(EnforcerError::NotReady);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    pub(super) fn begin_prospective_arm_exchange(
        &self,
        reservation: &ProspectiveReservationV1,
        generation: u64,
    ) -> EnforcerResult<Option<Uuid>> {
        let admission = self
            .admission
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        let actor = self.pending(reservation)?;
        if generation < actor.reservation.cancel_generation {
            return Err(EnforcerError::Conflict);
        }
        let mut value = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if matches!(value.exchange, ExchangePhase::MayHaveIssued { .. }) {
            return Ok(None);
        }
        self.check_original(&admission)?;
        if generation != actor.reservation.cancel_generation
            || value.stop_reserved
            || !actor.runtime_installed.load(Ordering::SeqCst)
        {
            return Err(EnforcerError::NotReady);
        }
        let sample = self.clock.sample().map_err(|_| EnforcerError::Unhealthy)?;
        actor
            .prospective
            .arm
            .validate_clock_check(&sample)
            .map_err(|_| EnforcerError::NotReady)?;
        let original_nonce = Uuid::new_v4();
        value.exchange = ExchangePhase::MayHaveIssued { original_nonce };
        Ok(Some(original_nonce))
    }
    pub(super) async fn bind_prospective(
        self: &Arc<Self>,
        reservation: ProspectiveReservationV1,
        registration: RegistrationV1,
        generation: u64,
        observe_only: bool,
        original_exchange_nonce: Option<Uuid>,
    ) -> EnforcerResult<Response> {
        let digest = registration.digest()?;
        let (actor, replayed) = {
            let admission = self
                .admission
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            if !observe_only {
                self.check_original(&admission)?;
            }
            if self.closing.load(Ordering::SeqCst) || self.owners.failed() {
                return Err(EnforcerError::Unhealthy);
            }
            let pending = self.pending(&reservation)?;
            pending.prospective.matches_registration(&registration)?;
            if generation < pending.reservation.cancel_generation
                || (!observe_only && generation != pending.reservation.cancel_generation)
            {
                return Err(EnforcerError::Conflict);
            }
            if !pending.runtime_installed.load(Ordering::SeqCst) {
                return Err(EnforcerError::NotReady);
            }
            let mut value = pending.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
            if !observe_only {
                if value.stop_reserved {
                    return Err(EnforcerError::NotReady);
                }
                if !matches!(value.exchange, ExchangePhase::MayHaveIssued { original_nonce } if Some(original_nonce) == original_exchange_nonce)
                {
                    return Err(EnforcerError::Unauthorized);
                }
                let sample = self.clock.sample().map_err(|_| EnforcerError::Unhealthy)?;
                registration
                    .arm
                    .validate_clock_check(&sample)
                    .map_err(|_| EnforcerError::NotReady)?;
            }
            let mut slots = self.slots.lock().map_err(|_| EnforcerError::Unhealthy)?;
            if let Some(actor) = &value.bound {
                if actor.digest != digest
                    || slots
                        .get(registration.arm_id())
                        .is_none_or(|current| !Arc::ptr_eq(current, actor))
                {
                    return Err(EnforcerError::Conflict);
                }
                if observe_only {
                    actor.stop.store(true, Ordering::SeqCst);
                    let mut bound = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
                    bound.prepared_once = true;
                    bound.generation = bound.generation.max(generation);
                }
                (Arc::clone(actor), true)
            } else {
                if !admission.entries.contains(registration.arm_id())
                    || slots.contains_key(registration.arm_id())
                {
                    return Err(EnforcerError::Conflict);
                }
                let actor = Arc::new(Actor {
                    _lease: Arc::clone(&self._lease),
                    registration,
                    digest: digest.clone(),
                    state: Mutex::new(EnforcerSlotStatusV1 {
                        arm_id: reservation.arm_id.clone(),
                        registration_hash: digest,
                        phase: SlotPhaseV1::Initializing,
                        prepared_once: observe_only,
                        generation,
                        clock: None,
                        observation: None,
                        stop_failures: value.stop_failures,
                    }),
                    usable: AtomicBool::new(false),
                    stop: AtomicBool::new(observe_only || value.stop_reserved),
                    runtime_installed: Arc::clone(&pending.runtime_installed),
                    failures: Arc::clone(&pending.failures),
                    original: !observe_only,
                    #[cfg(test)]
                    native_identity: Arc::clone(&pending.native_identity),
                    #[cfg(test)]
                    test_exit: Arc::clone(&pending.test_exit),
                });
                value.stop_reserved |= observe_only;
                value.bound = Some(Arc::clone(&actor));
                slots.insert(actor.registration.arm_id().to_owned(), Arc::clone(&actor));
                pending.wake.notify_all();
                (actor, false)
            }
        };
        let expires = Instant::now()
            + if observe_only {
                Duration::from_millis(100)
            } else {
                REGISTER_TIMEOUT - Duration::from_secs(1)
            };
        loop {
            {
                let admission = self
                    .admission
                    .lock()
                    .map_err(|_| EnforcerError::Unhealthy)?;
                if !observe_only {
                    self.check_original(&admission)?;
                }
                if self.owners.failed() || !actor.runtime_installed.load(Ordering::SeqCst) {
                    return Err(EnforcerError::NotReady);
                }
                if observe_only || actor.usable.load(Ordering::SeqCst) {
                    if !observe_only && actor.stop.load(Ordering::SeqCst) {
                        return Err(EnforcerError::NotReady);
                    }
                    if !observe_only {
                        let sample = self.clock.sample().map_err(|_| EnforcerError::Unhealthy)?;
                        actor
                            .registration
                            .arm
                            .validate_clock_check(&sample)
                            .map_err(|_| EnforcerError::NotReady)?;
                    }
                    return Ok(Response::Registered {
                        value: Box::new(actor.snapshot()?),
                        replayed,
                    });
                }
            }
            if Instant::now() >= expires {
                return Err(EnforcerError::NotReady);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    pub(super) async fn restore_prospective(
        self: &Arc<Self>,
        registration: RegistrationV1,
        generation: u64,
    ) -> EnforcerResult<Response> {
        let reservation = self
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .get(registration.arm_id())
            .ok_or(EnforcerError::Conflict)?
            .reservation
            .clone();
        self.bind_prospective(reservation, registration, generation, true, None)
            .await
    }
}

#[cfg(test)]
#[path = "server_prospective_tests.rs"]
mod tests;
