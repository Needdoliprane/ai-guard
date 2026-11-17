//! Native pre-Arm reservations. A receipt-less snapshot is not RegistrationV1
//! and cannot Prepare or retire. Its actual thread, handle, runtime and S slot
//! are retained before the Arm RPC and transferred to the exact bound actor.
use super::*;

struct ProspectiveState {
    bound: Option<Arc<FullActor>>,
    stop_reserved: bool,
    stop_failures: u64,
    exchange: ExchangePhase,
}

#[derive(Clone, Copy)]
enum ExchangePhase {
    NotIssued,
    MayHaveIssued { original_nonce: Uuid },
}

pub(super) struct ProspectiveActor {
    prospective: ProspectiveRegistrationV1,
    reservation: ProspectiveReservationV1,
    pub(super) key: ActorKey,
    original_registration_nonce: Uuid,
    state: Mutex<ProspectiveState>,
    wake: Condvar,
    worker_owned: AtomicBool,
    pub(super) runtime_installed: Arc<AtomicBool>,
    owners: NativeOwners,
    pub(super) failures: Arc<NativeStopFailures>,
    #[cfg(test)]
    test_exit: AtomicBool,
}

impl ProspectiveActor {
    fn cause_target(&self) -> CauseTarget {
        CauseTarget::fixed(
            self.prospective.arm_id(),
            &self.reservation.prospective_hash,
        )
    }
    pub(super) fn reserve_stop(&self) -> EnforcerResult<()> {
        let mut state = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        state.stop_reserved = true;
        if let Some(bound) = &state.bound {
            let mut actor = bound.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
            if !actor.gate.is_closing() {
                actor.gate.reserve_stop()?;
                actor.status.phase = SlotPhaseV1::Stopping;
            }
        }
        Ok(())
    }

    fn validate_replay(
        &self,
        prospective: &ProspectiveRegistrationV1,
        epoch: Uuid,
        generation: u64,
        replay_only: bool,
    ) -> EnforcerResult<()> {
        self.reservation
            .validate_for(prospective, epoch, self.reservation.cancel_generation)?;
        if generation != self.reservation.cancel_generation {
            if !replay_only || generation < self.reservation.cancel_generation {
                return Err(EnforcerError::Conflict);
            }
            self.reserve_stop()?;
            let state = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
            if let Some(bound) = &state.bound {
                let mut actor = bound.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
                actor.status.generation = actor.status.generation.max(generation);
            }
            self.wake();
        }
        Ok(())
    }

    pub(super) fn wake(&self) {
        self.wake.notify_all();
    }

    fn failed(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.stop_failures = state.stop_failures.saturating_add(1);
            if let Some(bound) = &state.bound {
                bound.failed();
            }
        }
    }

    fn start(self: &Arc<Self>, clock: Option<Arc<NativeHostClock>>) -> EnforcerResult<()> {
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
            .name("enforcer-prospective-stop".into())
            .stack_size(1024 * 1024)
            .spawn(move || {
                // This worker cannot install its runtime until NativeOwners
                // owns this exact key's actual JoinHandle and retained lease.
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
                self.worker_owned.store(false, Ordering::SeqCst);
                self.failed();
                return Err(EnforcerError::Unavailable);
            }
        };
        self.owners.actor_keyed(self.key, handle)?;
        retained.send(()).map_err(|_| EnforcerError::Unhealthy)
    }

    fn run(self: Arc<Self>, clock: Option<Arc<NativeHostClock>>) {
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
                        // The same real thread remains owned. An unavailable
                        // runtime never yields a reservation ACK or replacement.
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            };
            self.runtime_installed.store(true, Ordering::SeqCst);
            self.run_installed(clock, runtime)
        }));
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
        clock: Option<Arc<NativeHostClock>>,
        runtime: tokio::runtime::Runtime,
    ) -> EnforcerResult<()> {
        let mut last_ns = None;
        let mut stopping = false;
        // There is no provider or start operation in this cell. Reopening the
        // exact persisted invocation is needed only when STOP is actually due.
        let mut managed = None;
        loop {
            #[cfg(test)]
            if self.test_exit.load(Ordering::SeqCst) {
                return Ok(());
            }
            let sample = clock.as_ref().and_then(|clock| clock.sample().ok());
            let live = sample.as_ref().is_some_and(|sample| {
                sample.continuous_ns().is_ok_and(|now| {
                    let monotone = last_ns.is_none_or(|last| now >= last);
                    last_ns = Some(now);
                    monotone && self.prospective.arm.validate_clock_check(sample).is_ok()
                })
            });
            let bound = {
                let mut state = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
                stopping |= !live || state.stop_reserved;
                state.stop_reserved |= stopping;
                state.bound.clone()
            };
            if let Some(bound) = bound {
                // Same thread, same retained key, same runtime installation
                // proof and original immutable target. Bind does not spawn.
                drop(managed);
                return bound.run_installed(clock, runtime);
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
            let state = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
            let _waiting = self
                .wake
                .wait_timeout(state, Duration::from_millis(250))
                .map_err(|_| EnforcerError::Unhealthy)?;
        }
    }
}

impl FullState {
    /// Caller holds the shared admission gate. This is an exact continuation
    /// of a native pre-Arm owner, never a factory from a recovered record.
    /// It returns only a STOP status, with no registration/original grant.
    pub(in crate::broker::enforcer) fn restore_prospective_stop(
        &self,
        registration: &RegistrationV1,
        generation: u64,
        stop_requested: bool,
    ) -> EnforcerResult<Option<EnforcerSlotStatusV1>> {
        registration.validate()?;
        let arm_id =
            Uuid::parse_str(registration.arm_id()).map_err(|_| EnforcerError::InvalidInput)?;
        let digest = registration.digest()?;
        let mut actors = self.actors.lock().map_err(|_| EnforcerError::Unhealthy)?;
        let Some(pending) = self
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .get(&arm_id)
            .cloned()
        else {
            return Ok(None);
        };
        pending.prospective.matches_registration(registration)?;
        if generation < pending.reservation.cancel_generation {
            return Err(EnforcerError::Conflict);
        }
        if !pending.runtime_installed.load(Ordering::SeqCst) || self.owners.failed() {
            return Err(EnforcerError::NotReady);
        }
        let mut state = pending.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        state.stop_reserved |= stop_requested;
        let actor = if let Some(actor) = &state.bound {
            if actor.digest != digest
                || actors
                    .get(&arm_id)
                    .is_none_or(|current| !Arc::ptr_eq(current, actor))
            {
                return Err(EnforcerError::Conflict);
            }
            Arc::clone(actor)
        } else {
            if actors.contains_key(&arm_id) {
                return Err(EnforcerError::Conflict);
            }
            let mut gate = OriginalGate::new(false);
            if state.stop_reserved {
                gate.reserve_stop()?;
            }
            let actor = Arc::new(FullActor {
                registration: registration.clone(),
                digest,
                key: pending.key,
                original_registration_nonce: pending.original_registration_nonce,
                state: Mutex::new(ActorState {
                    status: EnforcerSlotStatusV1 {
                        arm_id: arm_id.to_string(),
                        registration_hash: registration.digest()?,
                        phase: SlotPhaseV1::Initializing,
                        prepared_once: true,
                        generation,
                        clock: None,
                        observation: None,
                        stop_failures: state.stop_failures,
                    },
                    gate,
                }),
                wake: Condvar::new(),
                exit: AtomicBool::new(false),
                runtime_installed: Arc::clone(&pending.runtime_installed),
                failures: Arc::clone(&pending.failures),
                owners: self.owners.clone(),
            });
            state.bound = Some(Arc::clone(&actor));
            actors.insert(arm_id, Arc::clone(&actor));
            actor
        };
        let mut bound = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if bound.gate.is_closing() {
            return Err(EnforcerError::Conflict);
        }
        bound.gate.restore();
        bound.status.prepared_once = true;
        bound.status.generation = bound.status.generation.max(generation);
        if state.stop_reserved {
            bound.gate.reserve_stop()?;
            bound.status.phase = SlotPhaseV1::Stopping;
        }
        pending.wake();
        actor.wake.notify_all();
        Ok(Some(bound.status.clone()))
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
        let arm_id =
            Uuid::parse_str(prospective.arm_id()).map_err(|_| EnforcerError::InvalidInput)?;
        let digest = prospective.digest()?;
        // A previous ambiguous RPC may only rediscover an already retained
        // native cell. A historical Pending payload is never a fresh factory.
        let existing = self
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .get(&arm_id)
            .cloned();
        let actor = if let Some(actor) = existing {
            actor.validate_replay(&prospective, self.epoch, generation, replay_only)?;
            actor
        } else {
            if replay_only {
                return Err(EnforcerError::Conflict);
            }
            self.complete().await?;
            if self
                .ledger_io(false, move |ledger| ledger.known_entry(arm_id))
                .await?
                .is_some()
            {
                return Err(EnforcerError::Conflict);
            }
            let sample = self
                .clock
                .as_ref()
                .ok_or(EnforcerError::Unhealthy)?
                .sample()
                .map_err(|_| EnforcerError::Unhealthy)?;
            prospective
                .arm
                .validate_clock_check(&sample)
                .map_err(|_| EnforcerError::NotReady)?;
            let mut admitted = self.admitted.lock().map_err(|_| EnforcerError::Unhealthy)?;
            self.admitted.check_original()?;
            if self.closed.load(Ordering::SeqCst) || self.owners.failed() {
                return Err(EnforcerError::Unhealthy);
            }
            let mut pending = self
                .prospective
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            if let Some(actor) = pending.get(&arm_id) {
                actor.validate_replay(&prospective, self.epoch, generation, replay_only)?;
                Arc::clone(actor)
            } else {
                if admitted.contains_key(prospective.arm_id()) {
                    return Err(EnforcerError::Conflict);
                }
                if admitted.len() >= self.max_slots as usize {
                    return Err(EnforcerError::Capacity);
                }
                let nonce = Uuid::new_v4();
                let reservation = ProspectiveReservationV1 {
                    epoch: self.epoch,
                    admission_generation: 0,
                    cancel_generation: generation,
                    reservation_nonce: nonce,
                    arm_id: prospective.arm_id().to_owned(),
                    prospective_hash: digest,
                };
                reservation.validate_for(&prospective, self.epoch, generation)?;
                let actor = Arc::new(ProspectiveActor {
                    prospective,
                    reservation,
                    key: ActorKey {
                        arm_id,
                        epoch: self.epoch,
                        nonce,
                    },
                    original_registration_nonce: Uuid::new_v4(),
                    state: Mutex::new(ProspectiveState {
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
                    test_exit: AtomicBool::new(false),
                });
                admitted.insert(
                    actor.prospective.arm_id().to_owned(),
                    RegistryOwner::Prospective,
                );
                pending.insert(arm_id, Arc::clone(&actor));
                actor
            }
        };
        actor.start(self.clock.clone())?;
        let expires = Instant::now() + REGISTER_TIMEOUT - Duration::from_secs(1);
        loop {
            if self.owners.failed() {
                return Err(EnforcerError::Unhealthy);
            }
            if actor.runtime_installed.load(Ordering::SeqCst) {
                if !replay_only {
                    let sample = self
                        .clock
                        .as_ref()
                        .ok_or(EnforcerError::Unhealthy)?
                        .sample()
                        .map_err(|_| EnforcerError::Unhealthy)?;
                    actor
                        .prospective
                        .arm
                        .validate_clock_check(&sample)
                        .map_err(|_| EnforcerError::NotReady)?;
                }
                let _admitted = self.admitted.lock().map_err(|_| EnforcerError::Unhealthy)?;
                if !replay_only {
                    self.admitted.check_original()?;
                    if actor
                        .state
                        .lock()
                        .map_err(|_| EnforcerError::Unhealthy)?
                        .stop_reserved
                    {
                        return Err(EnforcerError::NotReady);
                    }
                }
                // This ACK follows a real NativeOwners handoff and runtime
                // construction. It grants neither Prepare nor an Arm receipt.
                return Ok(actor.reservation.clone());
            }
            if !replay_only && self.admitted.fenced() {
                return Err(EnforcerError::NotReady);
            }
            if Instant::now() >= expires {
                return Err(EnforcerError::NotReady);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// This native transition precedes any central Arm RPC. Its first nonce
    /// is issued once; losing that response never creates another Original
    /// continuation from Pending DATA or a replay of this command.
    pub(super) fn begin_prospective_arm_exchange(
        &self,
        reservation: &ProspectiveReservationV1,
        generation: u64,
    ) -> EnforcerResult<Option<Uuid>> {
        let _admitted = self.admitted.lock().map_err(|_| EnforcerError::Unhealthy)?;
        let arm_id =
            Uuid::parse_str(&reservation.arm_id).map_err(|_| EnforcerError::InvalidInput)?;
        let pending = self
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .get(&arm_id)
            .cloned()
            .ok_or(EnforcerError::Conflict)?;
        reservation.validate_for(
            &pending.prospective,
            self.epoch,
            pending.reservation.cancel_generation,
        )?;
        if reservation.reservation_nonce != pending.reservation.reservation_nonce
            || reservation.admission_generation != pending.reservation.admission_generation
            || generation < pending.reservation.cancel_generation
        {
            return Err(EnforcerError::Conflict);
        }
        if self.closed.load(Ordering::SeqCst)
            || self.owners.failed()
            || !pending.runtime_installed.load(Ordering::SeqCst)
        {
            return Err(EnforcerError::NotReady);
        }
        let mut state = pending.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if matches!(state.exchange, ExchangePhase::MayHaveIssued { .. }) {
            return Ok(None);
        }
        self.admitted.check_original()?;
        if generation != pending.reservation.cancel_generation || state.stop_reserved {
            return Err(EnforcerError::NotReady);
        }
        let sample = self
            .clock
            .as_ref()
            .ok_or(EnforcerError::Unhealthy)?
            .sample()
            .map_err(|_| EnforcerError::Unhealthy)?;
        pending
            .prospective
            .arm
            .validate_clock_check(&sample)
            .map_err(|_| EnforcerError::NotReady)?;
        let original_nonce = Uuid::new_v4();
        state.exchange = ExchangePhase::MayHaveIssued { original_nonce };
        Ok(Some(original_nonce))
    }

    pub(super) async fn bind_prospective(
        self: &Arc<Self>,
        reservation: ProspectiveReservationV1,
        registration: RegistrationV1,
        generation: u64,
        observe_only: bool,
        original_exchange_nonce: Option<Uuid>,
    ) -> EnforcerResult<(
        EnforcerSlotStatusV1,
        bool,
        Full3RegistrationBindingV1,
        Uuid,
        Option<Uuid>,
    )> {
        registration.validate()?;
        let arm_id =
            Uuid::parse_str(registration.arm_id()).map_err(|_| EnforcerError::InvalidInput)?;
        let digest = registration.digest()?;
        let head = self.complete().await?;
        if !observe_only {
            let sample = self
                .clock
                .as_ref()
                .ok_or(EnforcerError::Unhealthy)?
                .sample()
                .map_err(|_| EnforcerError::Unhealthy)?;
            registration
                .arm
                .validate_clock_check(&sample)
                .map_err(|_| EnforcerError::NotReady)?;
        }
        let (actor, replayed) = {
            let mut admitted = self.admitted.lock().map_err(|_| EnforcerError::Unhealthy)?;
            if !observe_only {
                self.admitted.check_original()?;
            }
            if self.closed.load(Ordering::SeqCst) || self.owners.failed() {
                return Err(EnforcerError::Unhealthy);
            }
            let mut actors = self.actors.lock().map_err(|_| EnforcerError::Unhealthy)?;
            let pending = self
                .prospective
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .get(&arm_id)
                .cloned()
                .ok_or(EnforcerError::Conflict)?;
            pending.prospective.matches_registration(&registration)?;
            reservation.validate_for(
                &pending.prospective,
                self.epoch,
                pending.reservation.cancel_generation,
            )?;
            if (!observe_only && generation != pending.reservation.cancel_generation)
                || (observe_only && generation < pending.reservation.cancel_generation)
            {
                return Err(EnforcerError::Conflict);
            }
            if reservation.reservation_nonce != pending.reservation.reservation_nonce
                || reservation.admission_generation != pending.reservation.admission_generation
                || !pending.runtime_installed.load(Ordering::SeqCst)
            {
                return Err(EnforcerError::NotReady);
            }
            let mut state = pending.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
            if !observe_only
                && !matches!(state.exchange,
                ExchangePhase::MayHaveIssued { original_nonce } if Some(original_nonce) == original_exchange_nonce)
            {
                return Err(EnforcerError::Unauthorized);
            }
            if let Some(actor) = &state.bound {
                if actor.digest != digest
                    || actors
                        .get(&arm_id)
                        .is_none_or(|current| !Arc::ptr_eq(current, actor))
                {
                    return Err(EnforcerError::Conflict);
                }
                if observe_only {
                    let mut bound = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
                    if bound.gate.is_closing() {
                        return Err(EnforcerError::Conflict);
                    }
                    bound.gate.restore();
                    bound.gate.reserve_stop()?;
                    bound.status.prepared_once = true;
                    bound.status.generation = bound.status.generation.max(generation);
                } else if !actor
                    .state
                    .lock()
                    .map_err(|_| EnforcerError::Unhealthy)?
                    .gate
                    .original
                {
                    return Err(EnforcerError::Unauthorized);
                }
                (Arc::clone(actor), true)
            } else {
                if admitted.get(registration.arm_id()) != Some(&RegistryOwner::Prospective)
                    || actors.contains_key(&arm_id)
                {
                    return Err(EnforcerError::Conflict);
                }
                let mut gate = OriginalGate::new(!observe_only);
                if observe_only || state.stop_reserved {
                    gate.reserve_stop()?;
                }
                // An expired/cancelled prospective cell cannot mint a late
                // Original, even when the role itself remains fresh.
                if !observe_only && state.stop_reserved {
                    return Err(EnforcerError::NotReady);
                }
                let actor = Arc::new(FullActor {
                    registration,
                    digest: digest.clone(),
                    key: pending.key,
                    original_registration_nonce: pending.original_registration_nonce,
                    state: Mutex::new(ActorState {
                        status: EnforcerSlotStatusV1 {
                            arm_id: arm_id.to_string(),
                            registration_hash: digest.clone(),
                            phase: SlotPhaseV1::Initializing,
                            prepared_once: observe_only,
                            generation,
                            clock: None,
                            observation: None,
                            stop_failures: state.stop_failures,
                        },
                        gate,
                    }),
                    wake: Condvar::new(),
                    exit: AtomicBool::new(false),
                    runtime_installed: Arc::clone(&pending.runtime_installed),
                    failures: Arc::clone(&pending.failures),
                    owners: self.owners.clone(),
                });
                state.bound = Some(Arc::clone(&actor));
                actors.insert(arm_id, Arc::clone(&actor));
                admitted.insert(arm_id.to_string(), RegistryOwner::Full);
                pending.wake();
                (actor, false)
            }
        };
        let expires = Instant::now() + REGISTER_TIMEOUT - Duration::from_secs(1);
        loop {
            {
                let _admitted = self.admitted.lock().map_err(|_| EnforcerError::Unhealthy)?;
                if !observe_only {
                    self.admitted.check_original()?;
                }
                let state = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
                if self.owners.failed() || state.gate.is_closing() {
                    return Err(EnforcerError::NotReady);
                }
                if state.status.phase == SlotPhaseV1::Ready || observe_only {
                    let binding = data(Full3RegistrationBindingV1::new(
                        arm_id,
                        data(RetirementDigestV1::parse(&digest))?,
                        head.ledger().clone(),
                        self.epoch,
                    ))?;
                    return Ok((
                        state.status.clone(),
                        replayed,
                        binding,
                        actor.key.nonce,
                        if replayed || observe_only {
                            None
                        } else {
                            Some(actor.original_registration_nonce)
                        },
                    ));
                }
            }
            if Instant::now() >= expires {
                return Err(EnforcerError::NotReady);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Fixture-only native collection after the case has ended. This creates
    /// no application exit, retirement/Q, audit or quota release evidence.
    #[cfg(test)]
    pub(super) async fn finish_unbound_prospective_fixture(self: &Arc<Self>) -> EnforcerResult<()> {
        let pending: Vec<_> = self
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .values()
            .cloned()
            .collect();
        for actor in pending {
            if actor
                .state
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .bound
                .is_some()
            {
                continue;
            }
            actor.test_exit.store(true, Ordering::SeqCst);
            actor.wake();
            if actor.worker_owned.load(Ordering::SeqCst) {
                let mut native = self.owners.take_retiring(actor.key)?;
                tokio::task::spawn_blocking(move || native.join())
                    .await
                    .map_err(|_| EnforcerError::Unhealthy)??;
            }
            self.prospective
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .remove(&actor.key.arm_id);
            self.admitted
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .remove(actor.prospective.arm_id());
        }
        Ok(())
    }
}
