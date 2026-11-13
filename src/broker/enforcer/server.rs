use super::*;
use crate::{
    broker::{
        clock::{HostClock, NativeHostClock},
        discovery::RoleLease,
        local_ipc::{PrivateListener, read_frame, write_frame},
    },
    control_fs::Directory,
    runtime::ManagedRuntime,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
    time::Instant,
};
use tokio::{net::UnixStream, sync::watch};

#[path = "server_lifetime.rs"]
pub(super) mod lifetime;
use lifetime::{ListenerOwner, NativeOwners};

#[cfg(test)]
#[path = "server_lifetime_tests.rs"]
mod lifetime_tests;

struct State {
    _lease: Arc<RoleLease>,
    owners: NativeOwners,
    epoch: Uuid,
    max_slots: u32,
    capability: [u8; 32],
    clock: Arc<NativeHostClock>,
    slots: Mutex<BTreeMap<String, Arc<Actor>>>,
    closing: AtomicBool,
    healthy: AtomicBool,
    progress: AtomicU64,
    shutdown: watch::Sender<bool>,
}
struct Actor {
    _lease: Arc<RoleLease>,
    registration: RegistrationV1,
    digest: String,
    state: Mutex<EnforcerSlotStatusV1>,
    usable: AtomicBool,
    stop: AtomicBool,
}

/// Internal entry point of the same executable. The caller opens an existing
/// private role root; this function neither installs nor manages an OS service.
pub async fn run_internal_enforcer(root: &Directory, max_slots: u32) -> EnforcerResult<()> {
    if !(1..=MAX_SLOTS).contains(&max_slots) {
        return Err(EnforcerError::InvalidInput);
    }
    let lease = Arc::new(RoleLease::acquire(root)?);
    let secret = capability(root, false)?;
    let listener_root = lease.listener_root()?;
    let listener = PrivateListener::bind_new(&listener_root)?;
    let clock = Arc::new(NativeHostClock::new().map_err(|_| EnforcerError::Unhealthy)?);
    clock.sample().map_err(|_| EnforcerError::Unhealthy)?;
    let (shutdown, mut shutdown_receiver) = watch::channel(false);
    let progress_done = Arc::new(AtomicBool::new(false));
    let (owners, joiner) =
        NativeOwners::start(Arc::clone(&lease), Arc::clone(&progress_done), max_slots)?;
    let state = Arc::new(State {
        _lease: Arc::clone(&lease),
        owners: owners.clone(),
        epoch: listener.endpoint().epoch(),
        max_slots,
        capability: secret,
        clock,
        slots: Mutex::new(BTreeMap::new()),
        closing: AtomicBool::new(false),
        healthy: AtomicBool::new(true),
        progress: AtomicU64::new(1),
        shutdown,
    });
    let mut listener_owner = Some(ListenerOwner::new(
        owners,
        Arc::clone(&progress_done),
        Arc::downgrade(&state),
        joiner,
    ));
    #[cfg(test)]
    lifetime_tests::observe_state(&state);
    start_progress(Arc::clone(&state), Arc::clone(&progress_done))?;
    lease.publish_endpoint(listener.endpoint())?;
    let connections = Arc::new(tokio::sync::Semaphore::new(32));
    loop {
        tokio::select! {
            changed = shutdown_receiver.changed() => {
                if changed.is_err() {
                    return Err(EnforcerError::Unavailable);
                }
                if *shutdown_receiver.borrow() {
                    listener_owner.take().ok_or(EnforcerError::Unhealthy)?
                        .finish_idle().await?;
                    return Ok(());
                }
            }
            accepted = listener.accept_checked() => {
                match accepted {
                    Ok(stream) => {
                        let Ok(permit) = Arc::clone(&connections).try_acquire_owned() else {
                            // Closing this unprocessed connection gives no ACK.
                            drop(stream);
                            continue;
                        };
                        let state = Arc::clone(&state);
                        tokio::spawn(async move {
                            let _permit = permit;
                            let _ = serve_connection(stream, state).await;
                        });
                    }
                    Err(crate::broker::local_ipc::IpcError::Unauthorized) => {}
                    Err(_) => {
                        // Do not exit and abandon actors because discovery or
                        // listener metadata was changed. Existing deadlines
                        // continue; new starts lose the healthy handshake.
                        state.healthy.store(false, Ordering::SeqCst);
                        tokio::time::sleep(POLL).await;
                    }
                }
            }
        }
    }
}

fn start_progress(state: Arc<State>, done: Arc<AtomicBool>) -> EnforcerResult<()> {
    let owners = state.owners.clone();
    let lease = Arc::clone(&state._lease);
    let clock = Arc::clone(&state.clock);
    let weak_state = Arc::downgrade(&state);
    let progress_owners = owners.clone();
    let handle = std::thread::Builder::new()
        .name("enforcer-clock".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let _lease = lease;
            let _done = lifetime::ProgressDone(Arc::clone(&done), progress_owners);
            #[cfg(test)]
            let _lifetime_progress = lifetime_tests::ProgressObservation::enter();
            let mut last = None;
            while !done.load(Ordering::SeqCst) {
                let Some(state) = weak_state.upgrade() else {
                    return;
                };
                match clock.sample().and_then(|sample| {
                    sample
                        .continuous_ns()
                        .map_err(|_| crate::broker::clock::ClockError::InvalidReading)
                }) {
                    Ok(now) if last.is_none_or(|previous| now >= previous) => {
                        last = Some(now);
                        if state
                            .progress
                            .try_update(Ordering::SeqCst, Ordering::SeqCst, |progress| {
                                progress.checked_add(1)
                            })
                            .is_err()
                        {
                            state.healthy.store(false, Ordering::SeqCst);
                            return;
                        }
                        #[cfg(test)]
                        lifetime_tests::progress_tick();
                    }
                    _ => {
                        state.healthy.store(false, Ordering::SeqCst);
                        return;
                    }
                }
                // Do not retain State through a sleep or native termination.
                drop(state);
                std::thread::sleep(Duration::from_millis(25));
            }
        });
    match handle {
        Ok(handle) => owners.progress(Some(handle)),
        Err(_) => {
            owners.progress(None)?;
            Err(EnforcerError::Unavailable)
        }
    }
}

async fn serve_connection(mut stream: UnixStream, state: Arc<State>) -> EnforcerResult<()> {
    let challenge = Challenge {
        epoch: state.epoch,
        nonce: Uuid::new_v4(),
    };
    write_frame(&mut stream, &challenge, 1024, IO_TIMEOUT).await?;
    let authentication: Authentication = read_frame(&mut stream, 1024, IO_TIMEOUT).await?;
    let expected = proof(&state.capability, &challenge);
    if authentication.proof.len() != expected.len()
        || authentication
            .proof
            .bytes()
            .zip(expected.bytes())
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            != 0
    {
        return Err(EnforcerError::Unauthorized);
    }
    #[cfg(test)]
    let _lifetime_ipc = lifetime_tests::authenticated_owner().await;
    let request: Request = read_frame(&mut stream, MAX_FRAME, IO_TIMEOUT).await?;
    if matches!(request, Request::Watch {}) {
        loop {
            let response = dispatch(&state, Request::Status {}).await?;
            write_frame(&mut stream, &response, MAX_FRAME, IO_TIMEOUT).await?;
            tokio::time::sleep(POLL).await;
        }
    }
    let shutdown_request = matches!(request, Request::ShutdownIfIdle {});
    let response = match dispatch(&state, request).await {
        Ok(response) => response,
        Err(code) => Response::Error { code },
    };
    let shutdown = shutdown_request && matches!(response, Response::Acknowledged {});
    let written = write_frame(&mut stream, &response, MAX_FRAME, IO_TIMEOUT).await;
    if shutdown {
        state.shutdown.send_replace(true);
    }
    written?;
    Ok(())
}

async fn dispatch(state: &Arc<State>, request: Request) -> EnforcerResult<Response> {
    match request {
        Request::Status {} => {
            let slots = state.slots.lock().map_err(|_| EnforcerError::Unhealthy)?;
            Ok(Response::Status {
                value: EnforcerStatusV1 {
                    epoch: state.epoch,
                    slot_count: slots.len(),
                    max_slots: state.max_slots,
                    progress: state.progress.load(Ordering::SeqCst),
                    healthy: state.admission_healthy() && !state.closing.load(Ordering::SeqCst),
                },
            })
        }
        Request::Watch {} => Err(EnforcerError::InvalidInput),
        Request::Slots { arm_ids } => {
            if arm_ids.is_empty()
                || arm_ids.len() > MAX_SLOT_QUERY
                || arm_ids.iter().collect::<BTreeSet<_>>().len() != arm_ids.len()
            {
                return Err(EnforcerError::InvalidInput);
            }
            let slots = state.slots.lock().map_err(|_| EnforcerError::Unhealthy)?;
            let mut values = Vec::with_capacity(arm_ids.len());
            for arm_id in arm_ids {
                let actor = slots.get(&arm_id).ok_or(EnforcerError::Conflict)?;
                values.push(actor.snapshot()?);
            }
            Ok(Response::Slots { values })
        }
        Request::Register {
            registration,
            generation,
            observe_only,
        } => register(state, *registration, generation, observe_only).await,
        Request::Prepare {
            arm_id,
            registration_hash,
            generation,
        } => {
            if !state.admission_healthy() {
                return Err(EnforcerError::Unhealthy);
            }
            let actor = state.actor(&arm_id, &registration_hash)?;
            let current = state.clock.sample().map_err(|_| EnforcerError::Unhealthy)?;
            actor
                .registration
                .arm
                .validate_clock_check(&current)
                .map_err(|_| EnforcerError::NotReady)?;
            let mut value = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
            if generation == u64::MAX
                || value.generation != generation
                || value.phase != SlotPhaseV1::Ready
                || !actor.usable.load(Ordering::SeqCst)
                || actor.stop.load(Ordering::SeqCst)
            {
                return Err(EnforcerError::NotReady);
            }
            if value.prepared_once {
                return Err(EnforcerError::AlreadyPrepared);
            }
            // This is consumed before the ACK. Lost ACK is not a new permit.
            value.prepared_once = true;
            value.clock = Some(current);
            Ok(Response::Prepared {
                value: Box::new(value.clone()),
            })
        }
        Request::Cancel {
            arm_id,
            registration_hash,
            generation,
        } => {
            let actor = state.actor(&arm_id, &registration_hash)?;
            let mut value = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
            if generation < value.generation {
                return Err(EnforcerError::Conflict);
            }
            value.generation = generation;
            actor.stop.store(true, Ordering::SeqCst);
            Ok(Response::Acknowledged {})
        }
        Request::ShutdownIfIdle {} => {
            let slots = state.slots.lock().map_err(|_| EnforcerError::Unhealthy)?;
            if !slots.is_empty() {
                return Err(EnforcerError::NotIdle);
            }
            state.closing.store(true, Ordering::SeqCst);
            Ok(Response::Acknowledged {})
        }
    }
}

impl State {
    fn admission_healthy(&self) -> bool {
        self.healthy.load(Ordering::SeqCst) && !self.owners.failed()
    }

    fn actor(&self, arm_id: &str, digest: &str) -> EnforcerResult<Arc<Actor>> {
        let slots = self.slots.lock().map_err(|_| EnforcerError::Unhealthy)?;
        let actor = slots.get(arm_id).ok_or(EnforcerError::Conflict)?;
        if actor.digest != digest {
            return Err(EnforcerError::Conflict);
        }
        Ok(Arc::clone(actor))
    }
}
impl Actor {
    fn snapshot(&self) -> EnforcerResult<EnforcerSlotStatusV1> {
        self.state
            .lock()
            .map(|value| value.clone())
            .map_err(|_| EnforcerError::Unhealthy)
    }
}

async fn register(
    state: &Arc<State>,
    registration: RegistrationV1,
    generation: u64,
    observe_only: bool,
) -> EnforcerResult<Response> {
    if (generation == u64::MAX && !observe_only) || !state.admission_healthy() {
        return Err(EnforcerError::Unhealthy);
    }
    let digest = registration.digest()?;
    let (actor, replayed) = {
        let mut slots = state.slots.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if state.closing.load(Ordering::SeqCst) {
            return Err(EnforcerError::Unavailable);
        }
        if let Some(actor) = slots.get(&registration.arm.operation_id) {
            if actor.digest != digest {
                return Err(EnforcerError::Conflict);
            }
            if observe_only {
                actor
                    .state
                    .lock()
                    .map_err(|_| EnforcerError::Unhealthy)?
                    .prepared_once = true;
            }
            (Arc::clone(actor), true)
        } else {
            if slots.len() >= state.max_slots as usize {
                return Err(EnforcerError::Capacity);
            }
            let actor = Arc::new(Actor {
                _lease: Arc::clone(&state._lease),
                state: Mutex::new(EnforcerSlotStatusV1 {
                    arm_id: registration.arm.operation_id.clone(),
                    registration_hash: digest.clone(),
                    phase: SlotPhaseV1::Initializing,
                    prepared_once: observe_only,
                    generation,
                    clock: None,
                    observation: None,
                    stop_failures: 0,
                }),
                registration,
                digest,
                usable: AtomicBool::new(false),
                stop: AtomicBool::new(false),
            });
            // Capacity is retained even if thread creation or authority reopening
            // fails. No failed preparation can turn into a claimed Ready slot.
            slots.insert(
                actor.registration.arm.operation_id.clone(),
                Arc::clone(&actor),
            );
            (actor, false)
        }
    };
    if !replayed {
        // The reservation is retained, but thread creation and ownership
        // handoff take place after releasing the slot map.
        let handle = start_actor(
            Arc::clone(&actor),
            Arc::clone(&state.clock),
            state.owners.clone(),
        );
        match handle {
            Ok(handle) => {
                if state.owners.actor(Some(handle)).is_err() {
                    actor_failed(&actor);
                }
            }
            Err(_) => {
                let _ = state.owners.actor(None);
                actor_failed(&actor);
            }
        }
    }
    // A recovery sweep must reserve every target even if one authority is
    // unavailable. Short replies let the client continue its bounded sweep;
    // actors already reserved here keep working independently.
    let wait = if observe_only {
        Duration::from_millis(100)
    } else {
        REGISTER_TIMEOUT - Duration::from_secs(1)
    };
    let expires = Instant::now() + wait;
    while !actor.usable.load(Ordering::SeqCst) {
        if Instant::now() >= expires {
            return Err(EnforcerError::NotReady);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Ok(Response::Registered {
        value: Box::new(actor.snapshot()?),
        replayed,
    })
}

fn start_actor(
    actor: Arc<Actor>,
    clock: Arc<NativeHostClock>,
    owners: NativeOwners,
) -> EnforcerResult<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("enforcer-stop".into())
        .stack_size(1024 * 1024)
        .spawn(move || {
            let _exit = lifetime::ActorExit(owners);
            #[cfg(test)]
            let _lifetime_actor = lifetime_tests::actor_before_reopen(&actor);
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => {
                    actor_failed(&actor);
                    return;
                }
            };
            let managed = loop {
                match runtime.block_on(ManagedRuntime::reopen(actor.registration.authority.clone()))
                {
                    Ok(managed) => break managed,
                    Err(_) => {
                        actor_failed(&actor);
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            };
            // Stop machinery and invocation are ready before this release. No
            // event dispatcher, writer or HTTP is used by the remaining loop.
            run_actor(actor, clock, runtime, managed);
        })
        .map_err(|_| EnforcerError::Unavailable)
}

fn actor_failed(actor: &Actor) {
    if let Ok(mut state) = actor.state.lock() {
        state.phase = SlotPhaseV1::Quarantined;
        state.stop_failures = state.stop_failures.saturating_add(1);
    }
}

fn run_actor(
    actor: Arc<Actor>,
    clock: Arc<NativeHostClock>,
    runtime: tokio::runtime::Runtime,
    managed: ManagedRuntime,
) {
    let mut last_ns = None;
    let mut stopping = actor.stop.load(Ordering::SeqCst);
    loop {
        let sample = clock.sample();
        let live = match &sample {
            Ok(sample) => {
                let ns = sample.continuous_ns();
                match ns {
                    Ok(ns) if last_ns.is_none_or(|previous| ns >= previous) => {
                        last_ns = Some(ns);
                        actor.registration.arm.validate_clock_check(sample).is_ok()
                    }
                    _ => false,
                }
            }
            Err(_) => false,
        };
        stopping |= !live || actor.stop.load(Ordering::SeqCst);
        if let Ok(mut value) = actor.state.lock() {
            if let Ok(sample) = sample {
                value.clock = Some(sample);
            }
            value.phase = if stopping {
                SlotPhaseV1::Stopping
            } else {
                SlotPhaseV1::Ready
            };
            actor.usable.store(true, Ordering::SeqCst);
        } else {
            // Poisoned state is not permission to lose the already armed stop.
            stopping = true;
        }
        if stopping {
            let result = runtime.block_on(managed.stop_snapshot_owned(&actor.registration.owned));
            if let Ok(mut value) = actor.state.lock() {
                value.phase = SlotPhaseV1::Quarantined;
                match result {
                    Ok(observation) => value.observation = Some(observation),
                    Err(_) => value.stop_failures = value.stop_failures.saturating_add(1),
                }
            }
            // A stop observation is not D5 fencing. Even a currently stopped
            // CID must remain targeted against an older start arriving later.
            std::thread::sleep(Duration::from_millis(250));
        } else {
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::recovery::{RecoveryState, serve_connection as serve_recovery_connection};
    use super::*;
    use serde_json::{Value, json};

    #[tokio::test]
    async fn recovery_auth_offers_stops_only_when_admission_closed() {
        let epoch = Uuid::new_v4();
        let clock = Arc::new(NativeHostClock::new().expect("native test clock"));
        clock.sample().expect("native test clock sample");
        // The original RED exercised the only available V1 handler. The
        // explicit V2 handler now supplies recovery without changing V1 wire.
        let state = Arc::new(RecoveryState::new(
            epoch,
            1,
            [0x5a; 32],
            "a".repeat(64),
            Some(clock),
            None,
        ));
        let value = state.status().expect("read admission state");
        assert!(
            !value.admission_available(),
            "Full admission must be closed in this test"
        );

        let (server, mut client) = UnixStream::pair().expect("native Unix stream pair");
        let connection = tokio::spawn(serve_recovery_connection(server, state));
        let challenge = read_frame::<Value>(&mut client, 4096, IO_TIMEOUT).await;

        // Join before checking the challenge so a failed oracle cannot leave
        // the real connection task waiting for authentication.
        connection.abort();
        match connection.await {
            Err(error) => assert!(error.is_cancelled(), "connection task panicked: {error}"),
            Ok(result) => assert!(result.is_err(), "unauthenticated connection completed"),
        }
        drop(client);

        let challenge = challenge.expect("read the real enforcer challenge frame");
        let returned_epoch = challenge
            .get("epoch")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok());
        assert_eq!(
            returned_epoch,
            Some(epoch),
            "challenge must bind this epoch"
        );
        let nonce = challenge
            .get("nonce")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .expect("challenge must contain a UUID nonce");
        assert!(!nonce.is_nil(), "challenge nonce must be non-nil");

        assert_eq!(
            challenge.get("protocol").and_then(Value::as_u64),
            Some(2),
            "recovery must negotiate protocol 2 while Full admission is closed"
        );
        assert_eq!(
            challenge.get("offered_roles"),
            Some(&json!(["stops_only"])),
            "closed admission must offer StopsOnly and exclude Full"
        );
    }
}
