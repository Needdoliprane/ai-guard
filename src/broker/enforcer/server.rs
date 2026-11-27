use super::native_stop_failure::{CauseTarget, NativeStopFailures};
use super::recovery::RecoveryAttemptOwner;
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

#[path = "server_prospective.rs"]
mod prospective;
use prospective::{LegacyAdmission, ProspectiveActor};

#[cfg(test)]
#[path = "server_lifetime_tests.rs"]
mod lifetime_tests;
#[cfg(test)]
#[path = "server_prospective_fixture.rs"]
pub(super) mod test_support;

struct State {
    _lease: Arc<RoleLease>,
    owners: NativeOwners,
    epoch: Uuid,
    max_slots: u32,
    max_recovery_stop_owners: u32,
    capability: [u8; 32],
    clock: Arc<NativeHostClock>,
    slots: Arc<Mutex<BTreeMap<String, Arc<Actor>>>>,
    prospective: Arc<Mutex<BTreeMap<String, Arc<ProspectiveActor>>>>,
    admission: Mutex<LegacyAdmission>,
    recovery_requested: AtomicBool,
    recovery_attempt: Option<RecoveryAttemptOwner>,
    fresh_launch_nonce: Option<Uuid>,
    recovery_incomplete: AtomicBool,
    closing: AtomicBool,
    healthy: AtomicBool,
    progress: AtomicU64,
    shutdown: watch::Sender<bool>,
    #[cfg(test)]
    reserve_probe: test_support::ReserveProbe,
}
struct Actor {
    _lease: Arc<RoleLease>,
    registration: RegistrationV1,
    digest: String,
    state: Mutex<EnforcerSlotStatusV1>,
    usable: AtomicBool,
    stop: AtomicBool,
    runtime_installed: Arc<AtomicBool>,
    failures: Arc<NativeStopFailures>,
    original: bool,
    #[cfg(test)]
    native_identity: Arc<Mutex<prospective::NativeIdentity>>,
    #[cfg(test)]
    test_exit: Arc<AtomicBool>,
    #[cfg(test)]
    native_panic: Arc<Mutex<Option<Arc<()>>>>,
}

/// Internal entry point of the same executable. The caller opens an existing
/// private role root; this function neither installs nor manages an OS service.
pub async fn run_internal_enforcer(root: &Directory, max_slots: u32) -> EnforcerResult<()> {
    run_internal_enforcer_with_budget(root, max_slots, 0).await
}

/// Existing Legacy namespaces are never a fresh admission factory. Cold roles
/// are fenced before the listener, scan or any endpoint publication.
pub async fn run_internal_enforcer_with_budget(
    root: &Directory,
    max_slots: u32,
    max_recovery_stop_owners: u32,
) -> EnforcerResult<()> {
    let lease = Arc::new(RoleLease::acquire_existing(root)?);
    let secret = capability(root, false)?;
    run_legacy(lease, secret, max_slots, max_recovery_stop_owners, None).await
}

/// The child itself consumes mkdirat provenance and exclusively creates the
/// empty V1 lock and auth. A serialized claim or empty-looking existing root
/// cannot enter this branch; failure preserves its partial namespace.
pub async fn run_internal_legacy_fresh(
    parent: &Directory,
    role_name: &str,
    max_slots: u32,
    max_recovery_stop_owners: u32,
    launch_nonce: Uuid,
) -> EnforcerResult<()> {
    validate_budget(max_slots, max_recovery_stop_owners)?;
    if launch_nonce.is_nil() {
        return Err(EnforcerError::InvalidInput);
    }
    let fresh = parent
        .create_enforcer_root(role_name)
        .map_err(|_| EnforcerError::UnsafeFilesystem)?;
    let lease = Arc::new(RoleLease::create_new(fresh)?);
    let mut secret = [0; 32];
    use std::io::Read;
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut secret))
        .map_err(|_| EnforcerError::Unavailable)?;
    let auth = lease
        .root()
        .persistent_create("auth", &secret)
        .map_err(|_| EnforcerError::UnsafeFilesystem)?;
    auth.persistent_sync()
        .map_err(|_| EnforcerError::UnsafeFilesystem)?;
    lease
        .root()
        .sync()
        .map_err(|_| EnforcerError::UnsafeFilesystem)?;
    run_legacy(
        lease,
        secret,
        max_slots,
        max_recovery_stop_owners,
        Some(launch_nonce),
    )
    .await
}

fn validate_budget(max_slots: u32, recovery: u32) -> EnforcerResult<()> {
    if !(1..=MAX_SLOTS).contains(&max_slots) || recovery > MAX_SLOTS {
        return Err(EnforcerError::InvalidInput);
    }
    Ok(())
}

async fn run_legacy(
    lease: Arc<RoleLease>,
    secret: [u8; 32],
    max_slots: u32,
    max_recovery_stop_owners: u32,
    fresh_launch_nonce: Option<Uuid>,
) -> EnforcerResult<()> {
    let fresh = fresh_launch_nonce.is_some();
    validate_budget(max_slots, max_recovery_stop_owners)?;
    let listener_root = lease.listener_root()?;
    let listener = PrivateListener::bind_new(&listener_root)?;
    let clock = Arc::new(NativeHostClock::new().map_err(|_| EnforcerError::Unhealthy)?);
    clock.sample().map_err(|_| EnforcerError::Unhealthy)?;
    let (shutdown, mut shutdown_receiver) = watch::channel(false);
    let progress_done = Arc::new(AtomicBool::new(false));
    let permanent = max_slots + max_recovery_stop_owners.saturating_sub(1);
    let (owners, joiner) =
        NativeOwners::start(Arc::clone(&lease), Arc::clone(&progress_done), permanent)?;
    let slots = Arc::new(Mutex::new(BTreeMap::new()));
    let prospective = Arc::new(Mutex::new(BTreeMap::new()));
    owners.retain_legacy_targets(Arc::clone(&slots), Arc::clone(&prospective))?;
    // The single reserved catch-up context precedes endpoint publication. It
    // retains this same lease and never consumes an accepted target's owner.
    let recovery_attempt = if max_recovery_stop_owners > 0 {
        Some(RecoveryAttemptOwner::install_legacy(
            Arc::clone(&lease),
            Arc::clone(&clock),
            &owners,
        )?)
    } else {
        None
    };
    let state = Arc::new(State {
        _lease: Arc::clone(&lease),
        owners: owners.clone(),
        epoch: listener.endpoint().epoch(),
        max_slots,
        max_recovery_stop_owners,
        capability: secret,
        clock,
        slots,
        prospective,
        admission: Mutex::new(LegacyAdmission::new(fresh)),
        recovery_requested: AtomicBool::new(!fresh),
        recovery_attempt,
        fresh_launch_nonce,
        recovery_incomplete: AtomicBool::new(!fresh),
        closing: AtomicBool::new(false),
        healthy: AtomicBool::new(true),
        progress: AtomicU64::new(1),
        shutdown,
        #[cfg(test)]
        reserve_probe: test_support::ReserveProbe::default(),
    });
    let mut listener_owner = Some(ListenerOwner::new(
        owners,
        Arc::clone(&progress_done),
        Arc::downgrade(&state),
        joiner,
    ));
    #[cfg(test)]
    lifetime_tests::observe_state(&state);
    #[cfg(test)]
    prospective::observe_state(&state);
    #[cfg(test)]
    test_support::observe_state(&state);
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
                    if let Some(attempt) = &state.recovery_attempt {
                        attempt.finish_legacy_idle().await?;
                    }
                    state.owners.release_legacy_idle_custody()?;
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
    let mut request: Request = read_frame(&mut stream, MAX_FRAME, IO_TIMEOUT).await?;
    // Motif support is negotiated on this same authenticated connection and
    // incarnation, before the reservation write. It reuses the admission
    // above; it reserves nothing and grants nothing by itself.
    let mut motif_negotiated = false;
    if matches!(request, Request::NegotiateReservationMotif {}) {
        #[cfg(test)]
        if state.reserve_probe.refuses_negotiation() {
            // Historical decoder boundary: the unknown kind closes this
            // connection without any response frame.
            return Err(EnforcerError::InvalidInput);
        }
        write_frame(
            &mut stream,
            &Response::ReservationMotifSupported { epoch: state.epoch },
            MAX_FRAME,
            IO_TIMEOUT,
        )
        .await?;
        request = read_frame(&mut stream, MAX_FRAME, IO_TIMEOUT).await?;
        motif_negotiated = true;
    }
    #[cfg(test)]
    state.reserve_probe.observe_request(&request);
    if matches!(request, Request::Watch {}) {
        loop {
            let response = dispatch(&state, Request::Status {}, false).await?;
            write_frame(&mut stream, &response, MAX_FRAME, IO_TIMEOUT).await?;
            tokio::time::sleep(POLL).await;
        }
    }
    let shutdown_request = matches!(request, Request::ShutdownIfIdle {});
    let response = match dispatch(&state, request, motif_negotiated).await {
        Ok(response) => response,
        Err(code) => Response::Error { code },
    };
    #[cfg(test)]
    if matches!(response, Response::ProspectiveCapabilities { .. }) {
        state.reserve_probe.hold_capabilities_reply().await?;
    }
    let shutdown = shutdown_request && matches!(response, Response::Acknowledged {});
    let written = write_frame(&mut stream, &response, MAX_FRAME, IO_TIMEOUT).await;
    if shutdown {
        state.shutdown.send_replace(true);
    }
    written?;
    Ok(())
}

async fn dispatch(
    state: &Arc<State>,
    request: Request,
    motif_negotiated: bool,
) -> EnforcerResult<Response> {
    match request {
        Request::Status {} => {
            let admitted = state
                .admission
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            Ok(Response::Status {
                value: EnforcerStatusV1 {
                    epoch: state.epoch,
                    slot_count: admitted.entries.len(),
                    max_slots: state.max_slots,
                    progress: state.progress.load(Ordering::SeqCst),
                    healthy: state.admission_healthy() && !state.closing.load(Ordering::SeqCst),
                },
            })
        }
        Request::Watch {} => Err(EnforcerError::InvalidInput),
        // Valid only as the connection preamble handled before dispatch.
        Request::NegotiateReservationMotif {} => Err(EnforcerError::InvalidInput),
        Request::ProspectiveCapabilities {} => {
            let admitted = state
                .admission
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            let owned_active = state.owned_active()?;
            let known_stop_slots =
                u32::try_from(admitted.entries.len()).map_err(|_| EnforcerError::Unhealthy)?;
            Ok(Response::ProspectiveCapabilities {
                value: LegacyProspectiveCapabilitiesV1 {
                    version: 1,
                    epoch: state.epoch,
                    max_slots: state.max_slots,
                    max_recovery_stop_owners: state.max_recovery_stop_owners,
                    recovery_generation: (admitted.generation != 0).then_some(admitted.generation),
                    fresh_launch_nonce: state.fresh_launch_nonce,
                    known_stop_slots,
                    owned_active,
                    recovery_incomplete: state.recovery_incomplete.load(Ordering::SeqCst)
                        || owned_active != Some(known_stop_slots),
                },
            })
        }
        Request::ReserveProspective {
            prospective,
            generation,
            replay_only,
        } => {
            match state
                .reserve_prospective(
                    *prospective,
                    generation,
                    prospective::LegacyProspectiveSource::Historical { replay_only },
                )
                .await
            {
                Ok(reservation) => Ok(Response::ProspectiveReserved { reservation }),
                Err(refusal) => Ok(refusal.into_response(motif_negotiated)),
            }
        }
        Request::ReserveNamedProspective {
            prospective,
            generation,
            custody_nonce,
            birth,
        } => {
            match state
                .reserve_prospective(
                    *prospective,
                    generation,
                    prospective::LegacyProspectiveSource::Named {
                        custody_nonce,
                        birth,
                    },
                )
                .await
            {
                Ok(reservation) => Ok(Response::ProspectiveReserved { reservation }),
                Err(refusal) => Ok(refusal.into_response(motif_negotiated)),
            }
        }
        Request::BeginProspectiveArmExchange {
            reservation,
            generation,
        } => Ok(Response::ProspectiveArmExchange {
            original_exchange_nonce: state.begin_prospective_arm_exchange(
                &reservation,
                generation,
                LegacyProspectiveRailV1::Historical,
            )?,
        }),
        Request::BeginNamedProspectiveArmExchange {
            reservation,
            generation,
            custody_nonce,
        } => Ok(Response::ProspectiveArmExchange {
            original_exchange_nonce: state.begin_prospective_arm_exchange(
                &reservation,
                generation,
                LegacyProspectiveRailV1::Named(custody_nonce),
            )?,
        }),
        Request::BindProspective {
            reservation,
            registration,
            generation,
            observe_only,
            original_exchange_nonce,
        } => {
            state
                .bind_prospective(
                    reservation,
                    *registration,
                    generation,
                    observe_only,
                    original_exchange_nonce,
                    LegacyProspectiveRailV1::Historical,
                )
                .await
        }
        Request::BindNamedProspective {
            reservation,
            registration,
            generation,
            original_exchange_nonce,
            custody_nonce,
        } => {
            state
                .bind_prospective(
                    reservation,
                    *registration,
                    generation,
                    false,
                    Some(original_exchange_nonce),
                    LegacyProspectiveRailV1::Named(custody_nonce),
                )
                .await
        }
        Request::CancelProspective {
            reservation,
            generation,
        } => {
            state.cancel_prospective(&reservation, generation)?;
            Ok(Response::Acknowledged {})
        }
        Request::BeginRecovery {} => Ok(Response::RecoveryBegun {
            generation: state.begin_recovery_for_historical(None)?,
        }),
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
            let admitted = state
                .admission
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            state.require_historical_prospective(&arm_id)?;
            state.check_original(&admitted)?;
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
            check_runtime_installed(&actor)?;
            if generation == u64::MAX
                || value.generation != generation
                || value.phase != SlotPhaseV1::Ready
                || !actor.usable.load(Ordering::SeqCst)
                || actor.stop.load(Ordering::SeqCst)
                || !actor.original
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
            let _admitted = state
                .admission
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            state.require_historical_prospective(&arm_id)?;
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
            let admitted = state
                .admission
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            if !admitted.entries.is_empty() {
                return Err(EnforcerError::NotIdle);
            }
            if let Some(attempt) = &state.recovery_attempt {
                attempt.reserve_legacy_idle_shutdown()?;
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
    fn cause_target(&self) -> CauseTarget {
        CauseTarget::fixed(self.registration.arm_id(), &self.digest)
    }
    fn snapshot(&self) -> EnforcerResult<EnforcerSlotStatusV1> {
        self.state
            .lock()
            .map(|value| value.clone())
            .map_err(|_| EnforcerError::Unhealthy)
    }
}

fn check_runtime_installed(actor: &Actor) -> EnforcerResult<()> {
    if !actor.runtime_installed.load(Ordering::SeqCst) {
        #[cfg(test)]
        prospective::record_runtime_rejection();
        return Err(EnforcerError::NotReady);
    }
    Ok(())
}

async fn register(
    state: &Arc<State>,
    registration: RegistrationV1,
    generation: u64,
    observe_only: bool,
) -> EnforcerResult<Response> {
    let digest = registration.digest()?;
    if observe_only {
        state.begin_recovery_for_historical(Some(registration.arm_id()))?;
        let retained_prospective = state
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .contains_key(registration.arm_id());
        if retained_prospective {
            return state.restore_prospective(registration, generation).await;
        }
    }
    let (actor, replayed) = {
        let mut admitted = state
            .admission
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        state.require_historical_prospective(registration.arm_id())?;
        let mut slots = state.slots.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if state.closing.load(Ordering::SeqCst) {
            return Err(EnforcerError::Unavailable);
        }
        if let Some(actor) = slots.get(&registration.arm.operation_id) {
            if actor.digest != digest {
                return Err(EnforcerError::Conflict);
            }
            // A generic receipt replay can never recover Original. Only the
            // in-process prospective continuation may bind that authority.
            let mut value = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
            value.prepared_once = true;
            value.generation = value.generation.max(generation);
            if observe_only || state.recovery_requested.load(Ordering::SeqCst) {
                actor.stop.store(true, Ordering::SeqCst);
            }
            drop(value);
            (Arc::clone(actor), true)
        } else {
            if !observe_only {
                return Err(EnforcerError::NotReady);
            }
            if admitted.entries.contains(registration.arm_id()) {
                // An already installed unbound prospective owner is the only
                // possible continuation for this identity; never duplicate it.
                return Err(EnforcerError::Conflict);
            }
            if admitted.entries.len() >= state.permanent_capacity() {
                state.recovery_incomplete.store(true, Ordering::SeqCst);
                if let Some(attempt) = &state.recovery_attempt {
                    let _ = attempt.try_schedule_legacy(registration);
                }
                return Err(EnforcerError::Capacity);
            }
            let actor = Arc::new(Actor {
                _lease: Arc::clone(&state._lease),
                state: Mutex::new(EnforcerSlotStatusV1 {
                    arm_id: registration.arm.operation_id.clone(),
                    registration_hash: digest.clone(),
                    phase: SlotPhaseV1::Initializing,
                    prepared_once: true,
                    generation,
                    clock: None,
                    observation: None,
                    stop_failures: 0,
                }),
                registration,
                digest,
                usable: AtomicBool::new(false),
                stop: AtomicBool::new(true),
                runtime_installed: Arc::new(AtomicBool::new(false)),
                failures: NativeStopFailures::new(),
                original: false,
                #[cfg(test)]
                native_identity: Arc::new(Mutex::new(prospective::NativeIdentity::default())),
                #[cfg(test)]
                test_exit: Arc::new(AtomicBool::new(false)),
                #[cfg(test)]
                native_panic: Arc::new(Mutex::new(None)),
            });
            // Capacity is retained even if thread creation or authority reopening
            // fails. No failed preparation can turn into a claimed Ready slot.
            slots.insert(
                actor.registration.arm.operation_id.clone(),
                Arc::clone(&actor),
            );
            admitted
                .entries
                .insert(actor.registration.arm_id().to_owned());
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
    while if observe_only {
        !actor.runtime_installed.load(Ordering::SeqCst)
    } else {
        !actor.usable.load(Ordering::SeqCst)
    } {
        if Instant::now() >= expires {
            return Err(EnforcerError::NotReady);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    // A native Ready publication is not an admission bypass after recovery.
    let admitted = state
        .admission
        .lock()
        .map_err(|_| EnforcerError::Unhealthy)?;
    if state.owners.failed() {
        return Err(EnforcerError::Unhealthy);
    }
    if !observe_only {
        state.check_original(&admitted)?;
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
    let failures = Arc::clone(&actor.failures);
    let target = actor.cause_target();
    std::thread::Builder::new()
        .name("enforcer-stop".into())
        .stack_size(1024 * 1024)
        .spawn(move || {
            let _exit = lifetime::ActorExit(owners);
            #[cfg(test)]
            let _lifetime_actor = lifetime_tests::actor_before_reopen(&actor);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let runtime = loop {
                    match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(runtime) => break runtime,
                        Err(error) => {
                            actor.failures.context_failure(actor.cause_target(), error);
                            actor_failed(&actor);
                            std::thread::sleep(Duration::from_secs(1));
                        }
                    }
                };
                actor.runtime_installed.store(true, Ordering::SeqCst);
                run_bound_actor(&actor, clock, Box::new(runtime));
            }));
            if let Err(payload) = result {
                actor.runtime_installed.store(false, Ordering::SeqCst);
                #[cfg(test)]
                prospective::after_runtime_invalidation();
                actor.failures.native_thread_failed(payload);
                actor_failed(&actor);
            }
        })
        .map_err(|error| {
            failures.context_failure(target, error);
            EnforcerError::Unavailable
        })
}

fn actor_failed(actor: &Actor) {
    if let Ok(mut state) = actor.state.lock() {
        state.phase = SlotPhaseV1::Quarantined;
        state.stop_failures = state.stop_failures.saturating_add(1);
    }
}

fn run_bound_actor(
    actor: &Arc<Actor>,
    clock: Arc<NativeHostClock>,
    runtime: Box<tokio::runtime::Runtime>,
) {
    #[cfg(test)]
    if let Ok(mut identity) = actor.native_identity.lock() {
        identity.bound = Some((
            std::thread::current().id(),
            (&*runtime as *const tokio::runtime::Runtime) as usize,
        ));
    }
    let managed = loop {
        #[cfg(test)]
        if actor.test_exit.load(Ordering::SeqCst) {
            return;
        }
        #[cfg(test)]
        if let Some(payload) = actor.native_panic.lock().unwrap().take() {
            std::panic::panic_any(payload);
        }
        match actor.failures.execute(
            &runtime,
            actor.cause_target(),
            ManagedRuntime::reopen_native(actor.registration.authority.clone()),
        ) {
            Some(managed) => break managed.value,
            None => {
                actor_failed(actor);
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    };
    run_actor(Arc::clone(actor), clock, runtime, managed);
}

fn run_actor(
    actor: Arc<Actor>,
    clock: Arc<NativeHostClock>,
    runtime: Box<tokio::runtime::Runtime>,
    managed: ManagedRuntime,
) {
    let mut last_ns = None;
    let mut stopping = actor.stop.load(Ordering::SeqCst);
    loop {
        #[cfg(test)]
        if actor.test_exit.load(Ordering::SeqCst) {
            return;
        }
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
            let result = actor.failures.execute(
                &runtime,
                actor.cause_target(),
                managed.stop_snapshot_owned_native(&actor.registration.owned),
            );
            let mut observation = None;
            let failed = match result {
                Some(settled) => {
                    observation = Some(settled.value.observation.clone());
                    actor
                        .failures
                        .observed_stop(actor.cause_target(), settled.value.observation);
                    if let Some(cause) = settled.value.kill_failure {
                        actor
                            .failures
                            .returned(actor.cause_target(), cause, settled.proof);
                        true
                    } else {
                        false
                    }
                }
                None => true,
            };
            if let Ok(mut value) = actor.state.lock() {
                value.phase = SlotPhaseV1::Quarantined;
                if let Some(observation) = observation {
                    value.observation = Some(observation);
                }
                if failed {
                    value.stop_failures = value.stop_failures.saturating_add(1);
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
