use super::diagnostics::{EnforcerFailureReason, EnforcerStage, arm_clock_rejection};
use super::*;
use crate::{
    broker::{
        clock::{HostClock, NativeHostClock},
        discovery::{RoleLease, load_endpoint},
        local_ipc::{EndpointRecordV1, read_frame, write_frame},
    },
    control_fs::Directory,
    runner::broker::FreshStartPermission,
    runtime::{BeforeSpawnGuard, PrearmedStartControl},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Weak, atomic::AtomicBool},
    time::Instant,
};
use tokio::{net::UnixStream, sync::watch};

const RETAINED_CANCEL_TIMEOUT: Duration = Duration::from_secs(5);
const RETAINED_CANCEL_ATTEMPTS: usize = 3;

#[derive(Clone)]
pub struct EnforcerClient(Arc<Client>);
impl fmt::Debug for EnforcerClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EnforcerClient(<private>)")
    }
}
struct Client {
    root: Directory,
    executable: PathBuf,
    max_slots: u32,
    endpoint_root: Directory,
    endpoint: EndpointRecordV1,
    capability: [u8; 32],
    liveness: OwnedFd,
    replacement: Mutex<Option<EnforcerClient>>,
    unknown_slots: AtomicU64,
    lost_unknown: AtomicBool,
    accounted: Mutex<BTreeSet<String>>,
    auto_recovery: AtomicBool,
    recovery_gate: Mutex<()>,
    failed: AtomicBool,
    health: Mutex<Option<(u64, Instant)>>,
    // Retained exactly as long as the client, even if executor handles vanish.
    // A child failure must not force rereading a potentially blocked journal to
    // recover the original stop target/deadline.
    slots: Mutex<BTreeMap<String, Arc<LocalSlot>>>,
}
struct LocalSlot {
    registration: RegistrationV1,
    digest: String,
    generation: CancelGeneration,
    status: Mutex<(EnforcerSlotStatusV1, Instant)>,
    stop: watch::Sender<bool>,
    acknowledged: AtomicBool,
    cancel_durable: AtomicBool,
    // An exact durable Cancel replay stops this target without seeding a job's
    // shared generation, including when the enforcer itself must be recovered.
    retained_cancel: AtomicBool,
}

impl EnforcerClient {
    pub async fn connect_or_spawn(
        root: &Directory,
        executable: &Path,
        max_slots: u32,
    ) -> EnforcerResult<Self> {
        Self::connect_or_spawn_mode(root, executable, max_slots, true).await
    }

    async fn connect_or_spawn_mode(
        root: &Directory,
        executable: &Path,
        max_slots: u32,
        auto_recovery: bool,
    ) -> EnforcerResult<Self> {
        if !(1..=MAX_SLOTS).contains(&max_slots) || !executable.is_absolute() {
            return Err(EnforcerError::InvalidInput);
        }
        root.revalidate()
            .map_err(|_| EnforcerError::UnsafeFilesystem)?;
        let secret = capability(root, true)?;
        if let Some(endpoint) = load_endpoint(root)? {
            match Self::connect(root, executable, endpoint, secret, max_slots, auto_recovery).await
            {
                Ok(client) => return Ok(client),
                // A peer that accepts then fails authentication/handshake, or
                // times out, never grants takeover. Only a refused connection
                // can reach the lease probe below.
                Err(ConnectFailure::Absent) => {}
                Err(ConnectFailure::Other(error)) => return Err(error),
            }
        }
        // The child must take this lease itself. A successful probe is dropped
        // before spawn; two racers are still serialized by the child's lease.
        let lease = RoleLease::acquire(root)?;
        drop(lease);
        spawn_helper(root, executable, max_slots)?;
        let expires = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(endpoint) = load_endpoint(root)? {
                match Self::connect(root, executable, endpoint, secret, max_slots, auto_recovery)
                    .await
                {
                    Ok(client) => return Ok(client),
                    Err(ConnectFailure::Absent) => {}
                    Err(ConnectFailure::Other(error)) => return Err(error),
                }
            }
            if Instant::now() >= expires {
                return Err(EnforcerError::Unavailable);
            }
            tokio::time::sleep(POLL).await;
        }
    }

    async fn connect(
        root: &Directory,
        executable: &Path,
        endpoint: EndpointRecordV1,
        secret: [u8; 32],
        max_slots: u32,
        auto_recovery: bool,
    ) -> Result<Self, ConnectFailure> {
        let endpoint_root = root
            .persistent_child("endpoints", false)
            .map_err(|_| ConnectFailure::Other(EnforcerError::UnsafeFilesystem))?
            .ok_or(ConnectFailure::Other(EnforcerError::UnsafeFilesystem))?;
        // Distinguish a definitely refused connect from a silent live peer.
        let stream = endpoint.connect_checked(&endpoint_root).await;
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(crate::broker::local_ipc::IpcError::Unavailable) => {
                return Err(ConnectFailure::Absent);
            }
            Err(error) => return Err(ConnectFailure::Other(error.into())),
        };
        // Retain a kernel object, not a PID that could later be reused. The
        // monitor reads heartbeats; the final guard only polls HUP/ERR without
        // reading, blocking or touching the filesystem.
        // SAFETY: duplicate this live socket; the returned FD is newly owned.
        let fd = unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if fd < 0 {
            return Err(ConnectFailure::Other(EnforcerError::Unavailable));
        }
        // SAFETY: successful F_DUPFD_CLOEXEC returned a fresh owned descriptor.
        let liveness = unsafe { OwnedFd::from_raw_fd(fd) };
        let client = Self(Arc::new(Client {
            root: root.clone(),
            executable: executable.into(),
            max_slots,
            endpoint_root,
            endpoint,
            capability: secret,
            liveness,
            replacement: Mutex::new(None),
            unknown_slots: AtomicU64::new(0),
            lost_unknown: AtomicBool::new(false),
            accounted: Mutex::new(BTreeSet::new()),
            auto_recovery: AtomicBool::new(auto_recovery),
            recovery_gate: Mutex::new(()),
            failed: AtomicBool::new(false),
            health: Mutex::new(None),
            slots: Mutex::new(BTreeMap::new()),
        }));
        client
            .0
            .authenticate(&mut stream)
            .await
            .map_err(ConnectFailure::Other)?;
        write_frame(&mut stream, &Request::Watch {}, MAX_FRAME, IO_TIMEOUT)
            .await
            .map_err(|error| ConnectFailure::Other(error.into()))?;
        let response: Response = read_frame(&mut stream, MAX_FRAME, IO_TIMEOUT)
            .await
            .map_err(|error| ConnectFailure::Other(error.into()))?;
        match response {
            Response::Status { value }
                if value.epoch == client.0.endpoint.epoch()
                    && value.healthy
                    && value.max_slots == max_slots =>
            {
                client
                    .0
                    .unknown_slots
                    .store(value.slot_count as u64, Ordering::SeqCst);
                *client
                    .0
                    .health
                    .lock()
                    .map_err(|_| ConnectFailure::Other(EnforcerError::Unhealthy))? =
                    Some((value.progress, Instant::now()));
            }
            _ => return Err(ConnectFailure::Other(EnforcerError::Unhealthy)),
        }
        tokio::spawn(monitor(Arc::downgrade(&client.0), stream));
        Ok(client)
    }

    pub async fn register(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
    ) -> EnforcerResult<EnforcedSlot> {
        let active = self.active()?;
        active
            .register_current(registration, generation, false)
            .await
    }

    /// Recover only the original armed stop. This permanently revokes local
    /// start/transport permits and tells even a new child to retain ObserveOnly.
    pub async fn restore(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
    ) -> EnforcerResult<EnforcedSlot> {
        let active = self.active()?;
        active
            .register_current(registration, generation, true)
            .await
    }

    fn retain_registration(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
        observe_only: bool,
        retained_cancel: bool,
    ) -> EnforcerResult<(Arc<LocalSlot>, u64)> {
        let digest = registration.digest()?;
        let expected = generation.current();
        if (expected == u64::MAX && !observe_only) || self.0.failed.load(Ordering::SeqCst) {
            return Err(EnforcerError::Unhealthy);
        }
        let local = {
            let mut slots = self.0.slots.lock().map_err(|_| EnforcerError::Unhealthy)?;
            if self.0.failed.load(Ordering::SeqCst) {
                return Err(EnforcerError::Unhealthy);
            }
            if let Some(existing) = slots.get(&registration.arm.operation_id) {
                if existing.digest != digest || !Arc::ptr_eq(&existing.generation.0, &generation.0)
                {
                    return Err(EnforcerError::Conflict);
                }
                Arc::clone(existing)
            } else {
                if slots.len() >= self.0.max_slots as usize {
                    return Err(EnforcerError::Capacity);
                }
                let (stop, _) = watch::channel(observe_only);
                let local = Arc::new(LocalSlot {
                    status: Mutex::new((
                        EnforcerSlotStatusV1 {
                            arm_id: registration.arm.operation_id.clone(),
                            registration_hash: digest.clone(),
                            phase: SlotPhaseV1::Initializing,
                            prepared_once: observe_only,
                            generation: expected,
                            clock: None,
                            observation: None,
                            stop_failures: 0,
                        },
                        Instant::now(),
                    )),
                    registration,
                    digest,
                    generation,
                    stop,
                    acknowledged: AtomicBool::new(false),
                    cancel_durable: AtomicBool::new(retained_cancel),
                    retained_cancel: AtomicBool::new(retained_cancel),
                });
                slots.insert(
                    local.registration.arm.operation_id.clone(),
                    Arc::clone(&local),
                );
                local
            }
        };
        if retained_cancel {
            local.retained_cancel.store(true, Ordering::SeqCst);
            local.cancel_durable.store(true, Ordering::SeqCst);
        }
        if observe_only {
            // Revoke any previously issued local guard before the RPC. A lost
            // restoration ACK cannot leave an old start permit usable.
            local.stop.send_replace(true);
            local
                .status
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .0
                .prepared_once = true;
        }
        Ok((local, expected))
    }

    async fn register_current(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
        observe_only: bool,
    ) -> EnforcerResult<EnforcedSlot> {
        let (local, expected) =
            self.retain_registration(registration, generation, observe_only, false)?;
        // Retain before sending: an accepted request whose ACK is lost still
        // belongs to the recovery set, without consulting the journal writer.
        let expires = Instant::now() + REGISTER_TIMEOUT;
        let response = loop {
            let remaining = expires.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(EnforcerError::NotReady);
            }
            let response = self
                .0
                .request(
                    Request::Register {
                        registration: Box::new(local.registration.clone()),
                        generation: expected,
                        observe_only,
                    },
                    remaining,
                )
                .await?;
            if observe_only
                && matches!(
                    response,
                    Response::Error {
                        code: EnforcerError::NotReady
                    }
                )
            {
                tokio::time::sleep(Duration::from_millis(25)).await;
                continue;
            }
            break response;
        };
        let Response::Registered { value, replayed } = response else {
            return Err(response_error(response));
        };
        let value = *value;
        if value.arm_id != local.registration.arm.operation_id
            || value.registration_hash != local.digest
        {
            return Err(EnforcerError::Conflict);
        }
        let observed_generation = value.generation;
        accept_registration(&self.0, &local, value, replayed)?;
        if observe_only && !local.retained_cancel.load(Ordering::SeqCst) {
            local.generation.advance_to(observed_generation);
        }
        Ok(EnforcedSlot {
            client: self.clone(),
            local,
        })
    }

    /// Deliver only a previously durable Cancel's exact retained target. The
    /// caller has verified its command/prefix projection. No new start permit
    /// or shared job-generation change can be produced by this port.
    pub(crate) async fn cancel_retained(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
    ) -> EnforcerResult<()> {
        let digest = registration.digest()?;
        let active =
            self.active_for_retained_cancel(&registration.arm.operation_id, &digest, &generation)?;
        // These synchronous mutations precede every RPC, even if registration
        // was accepted remotely but its acknowledgement is lost.
        let (local, _) = active.retain_registration(registration, generation, true, true)?;
        let expires = Instant::now() + RETAINED_CANCEL_TIMEOUT;
        let mut failure = EnforcerError::Unavailable;
        for attempt in 0..RETAINED_CANCEL_ATTEMPTS {
            let current = match active.active() {
                Ok(current) => current,
                Err(error) => {
                    failure = error;
                    if attempt + 1 < RETAINED_CANCEL_ATTEMPTS {
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                    continue;
                }
            };
            let registered = retained_request(
                &current.0,
                Request::Register {
                    registration: Box::new(local.registration.clone()),
                    generation: local.generation.current(),
                    observe_only: true,
                },
                expires,
            )
            .await;
            match registered {
                Ok(Response::Registered { value, replayed }) => {
                    accept_registration(&current.0, &local, *value, replayed)?;
                }
                // NotReady may already have reserved the actor. A lost reply
                // may also hide acceptance: try its exact hash without Ready.
                Ok(Response::Error {
                    code: EnforcerError::NotReady | EnforcerError::Unavailable,
                })
                | Err(_) => {}
                Ok(response) => return Err(response_error(response)),
            }
            let observed = local
                .status
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .0
                .generation;
            let generation = observed.max(local.generation.current());
            match retained_request(
                &current.0,
                Request::Cancel {
                    arm_id: local.registration.arm.operation_id.clone(),
                    registration_hash: local.digest.clone(),
                    generation,
                },
                expires,
            )
            .await
            {
                Ok(Response::Acknowledged {}) => return Ok(()),
                Ok(Response::Error {
                    code: EnforcerError::Conflict,
                }) => {
                    // Another cancellation may have advanced this actor while
                    // this request was in flight. Observe only this same slot.
                    match retained_request(
                        &current.0,
                        Request::Slots {
                            arm_ids: vec![local.registration.arm.operation_id.clone()],
                        },
                        expires,
                    )
                    .await
                    {
                        Ok(Response::Slots { mut values }) if values.len() == 1 => {
                            let value = values.remove(0);
                            if value.arm_id != local.registration.arm.operation_id
                                || value.registration_hash != local.digest
                            {
                                return Err(EnforcerError::Conflict);
                            }
                            *local.status.lock().map_err(|_| EnforcerError::Unhealthy)? =
                                (value, Instant::now());
                        }
                        Ok(_) | Err(_) => {}
                    }
                    failure = EnforcerError::Conflict;
                }
                Ok(response) => failure = response_error(response),
                Err(error) => failure = error,
            }
            if Instant::now() >= expires {
                break;
            }
            if attempt + 1 < RETAINED_CANCEL_ATTEMPTS {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
        Err(failure)
    }

    fn active_for_retained_cancel(
        &self,
        arm_id: &str,
        digest: &str,
        generation: &CancelGeneration,
    ) -> EnforcerResult<Self> {
        let mut current = self.clone();
        // A target can have entered R1 after the root client was replaced.
        // Mark existing matching Arcs on every traversed generation before
        // returning its failure; never insert after a permanent recovery fuse.
        for _ in 0..128 {
            {
                let slots = current
                    .0
                    .slots
                    .lock()
                    .map_err(|_| EnforcerError::Unhealthy)?;
                if let Some(local) = slots.get(arm_id) {
                    if local.digest != digest || !Arc::ptr_eq(&local.generation.0, &generation.0) {
                        return Err(EnforcerError::Conflict);
                    }
                    local.retained_cancel.store(true, Ordering::SeqCst);
                    local.cancel_durable.store(true, Ordering::SeqCst);
                    local.stop.send_replace(true);
                    local
                        .status
                        .lock()
                        .map_err(|_| EnforcerError::Unhealthy)?
                        .0
                        .prepared_once = true;
                }
            }
            let next = current
                .0
                .replacement
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .clone();
            match next {
                Some(next) => current = next,
                None if current.0.failed.load(Ordering::SeqCst) => {
                    return Err(EnforcerError::Unhealthy);
                }
                None => return Ok(current),
            }
        }
        Err(EnforcerError::Unhealthy)
    }

    pub async fn status(&self) -> EnforcerResult<EnforcerStatusV1> {
        let active = self.active()?;
        match active.0.request(Request::Status {}, IO_TIMEOUT).await? {
            Response::Status { value } if value.epoch == active.0.endpoint.epoch() => Ok(value),
            response => Err(response_error(response)),
        }
    }

    /// Check the current helper before a new central admission. Status alone
    /// does not account for unknown targets, a failed old client or a partial
    /// stop-only recovery. Existing acknowledged facts remain readable without
    /// this gate; replaying an unknown POST can still create an admission.
    pub async fn require_admission_ready(&self) -> EnforcerResult<()> {
        let active = self.active()?;
        active.0.check_health()?;
        let response = active.0.request(Request::Status {}, IO_TIMEOUT).await?;
        let Response::Status { value } = response else {
            return Err(response_error(response));
        };
        active.0.check_health()?;
        let slots = active
            .0
            .slots
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        if value.epoch != active.0.endpoint.epoch()
            || !value.healthy
            || value.max_slots != active.0.max_slots
            || value.slot_count != slots.len()
            || slots
                .values()
                .any(|slot| !slot.acknowledged.load(Ordering::SeqCst))
        {
            return Err(EnforcerError::Unhealthy);
        }
        if value.slot_count >= value.max_slots as usize {
            return Err(EnforcerError::Capacity);
        }
        Ok(())
    }

    pub async fn slot_status(&self, arm_id: &str) -> EnforcerResult<EnforcerSlotStatusV1> {
        let active = self.active()?;
        let response = active
            .0
            .request(
                Request::Slots {
                    arm_ids: vec![arm_id.into()],
                },
                IO_TIMEOUT,
            )
            .await?;
        match response {
            Response::Slots { mut values } if values.len() == 1 => {
                values.pop().ok_or(EnforcerError::Unavailable)
            }
            response => Err(response_error(response)),
        }
    }

    pub async fn shutdown_if_idle(&self) -> EnforcerResult<()> {
        let active = self.active()?;
        let previous = active.0.auto_recovery.swap(false, Ordering::SeqCst);
        match active
            .0
            .request(Request::ShutdownIfIdle {}, IO_TIMEOUT)
            .await
        {
            Ok(Response::Acknowledged {}) => Ok(()),
            Ok(response) => {
                active.0.auto_recovery.store(previous, Ordering::SeqCst);
                Err(response_error(response))
            }
            Err(error) => {
                active.0.auto_recovery.store(previous, Ordering::SeqCst);
                Err(error)
            }
        }
    }

    fn active(&self) -> EnforcerResult<Self> {
        let mut active = self.clone();
        // The same bound as append-only discovery generations.
        for _ in 0..128 {
            let replacement = active
                .0
                .replacement
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .clone();
            match replacement {
                Some(next) => active = next,
                None if active.0.failed.load(Ordering::SeqCst) => {
                    return Err(EnforcerError::Unhealthy);
                }
                None => return Ok(active),
            }
        }
        Err(EnforcerError::Unhealthy)
    }
}

#[cfg(test)]
#[path = "cancel_tests.rs"]
mod retained_cancel_tests;

async fn retained_request(
    client: &Client,
    request: Request,
    expires: Instant,
) -> EnforcerResult<Response> {
    let remaining = expires.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(EnforcerError::Unavailable);
    }
    tokio::time::timeout(
        remaining,
        client.request(request, remaining.min(IO_TIMEOUT)),
    )
    .await
    .map_err(|_| EnforcerError::Unavailable)?
}

fn accept_registration(
    client: &Client,
    local: &LocalSlot,
    value: EnforcerSlotStatusV1,
    replayed: bool,
) -> EnforcerResult<()> {
    if value.arm_id != local.registration.arm.operation_id
        || value.registration_hash != local.digest
    {
        return Err(EnforcerError::Conflict);
    }
    let first = client
        .accounted
        .lock()
        .map_err(|_| EnforcerError::Unhealthy)?
        .insert(value.arm_id.clone());
    if first && replayed {
        let _ = client
            .unknown_slots
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                Some(count.saturating_sub(1))
            });
    }
    if value.phase != SlotPhaseV1::Ready {
        local.stop.send_replace(true);
    }
    *local.status.lock().map_err(|_| EnforcerError::Unhealthy)? = (value, Instant::now());
    local.acknowledged.store(true, Ordering::SeqCst);
    Ok(())
}

enum ConnectFailure {
    Absent,
    Other(EnforcerError),
}

impl Client {
    async fn authenticate(&self, stream: &mut UnixStream) -> EnforcerResult<()> {
        let challenge: Challenge = read_frame(stream, 1024, IO_TIMEOUT).await?;
        if challenge.epoch != self.endpoint.epoch() || challenge.nonce.is_nil() {
            return Err(EnforcerError::Unauthorized);
        }
        write_frame(
            stream,
            &Authentication {
                proof: proof(&self.capability, &challenge),
            },
            1024,
            IO_TIMEOUT,
        )
        .await?;
        Ok(())
    }
    async fn request(&self, request: Request, timeout: Duration) -> EnforcerResult<Response> {
        let mut stream = self.endpoint.connect_checked(&self.endpoint_root).await?;
        self.authenticate(&mut stream).await?;
        write_frame(&mut stream, &request, MAX_FRAME, IO_TIMEOUT).await?;
        Ok(read_frame(&mut stream, MAX_FRAME, timeout).await?)
    }
    fn fail(&self) {
        self.failed.store(true, Ordering::SeqCst);
        if let Ok(slots) = self.slots.lock() {
            for slot in slots.values() {
                slot.stop.send_replace(true);
            }
        }
    }
}

/// Losing this handle never releases the remote slot or its armed stop.
pub struct EnforcedSlot {
    client: EnforcerClient,
    local: Arc<LocalSlot>,
}
impl fmt::Debug for EnforcedSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EnforcedSlot(<private retained stop>)")
    }
}
impl EnforcedSlot {
    /// Call immediately before requesting a central start claim. A registration
    /// handle can also represent a retained ObserveOnly/quarantined target.
    pub fn check_ready_for_claim(&self, expected_generation: u64) -> EnforcerResult<()> {
        self.check_ready_for_claim_diagnosed(expected_generation)
            .map_err(|failure| failure.wire_code())
    }

    pub(crate) fn check_ready_for_claim_diagnosed(
        &self,
        expected_generation: u64,
    ) -> Result<(), EnforcerStageFailure> {
        let arm = &self.local.registration.arm;
        let failure = |reason, sample: Option<&HostClockSampleV1>| {
            EnforcerStageFailure::from_arm(arm, EnforcerStage::BeforeClaim, reason, sample)
        };
        self.client
            .0
            .check_health()
            .map_err(|_| failure(EnforcerFailureReason::HelperUnhealthy, None))?;
        let (status, received) = &*self
            .local
            .status
            .lock()
            .map_err(|_| failure(EnforcerFailureReason::LockPoisoned, None))?;
        if let Some(reason) = claim_snapshot_failure(
            status,
            expected_generation,
            received,
            &self.local.acknowledged,
            &self.local.generation,
            &self.local.stop,
        ) {
            return Err(failure(reason, None).with_slot_snapshot(status));
        }
        let clock = NativeHostClock::new()
            .map_err(|_| failure(EnforcerFailureReason::ClockUnavailable, None))?;
        let sample = clock
            .sample()
            .map_err(|_| failure(EnforcerFailureReason::ClockUnavailable, None))?;
        arm.validate_clock_check(&sample).map_err(|_| {
            failure(
                arm_clock_rejection(&arm.clock, &arm.deadline_ns_decimal, &sample),
                Some(&sample),
            )
        })
    }

    pub async fn prepare_start(
        &self,
        permission: FreshStartPermission,
        expected_generation: u64,
    ) -> EnforcerResult<PrearmedStartControl> {
        self.prepare_start_diagnosed(permission, expected_generation)
            .await
            .map_err(|failure| failure.wire_code())
    }

    pub(crate) async fn prepare_start_diagnosed(
        &self,
        permission: FreshStartPermission,
        expected_generation: u64,
    ) -> Result<PrearmedStartControl, EnforcerStageFailure> {
        let registration = &self.local.registration;
        let arm = &registration.arm;
        let failure = |reason, sample: Option<&HostClockSampleV1>| {
            EnforcerStageFailure::from_arm(arm, EnforcerStage::Prepare, reason, sample)
        };
        if permission.arm_id() != arm.operation_id
            || permission.worker_id() != arm.worker_id
            || permission.attempt_id() != arm.attempt_id
            || permission.control_namespace() != registration.receipt.control_namespace_uuid
        {
            return Err(failure(
                EnforcerFailureReason::PermissionBindingMismatch,
                None,
            ));
        }
        let subject_key = arm
            .subject_key()
            .map_err(|_| failure(EnforcerFailureReason::InvalidArm, None))?;
        if permission.subject_key() != subject_key {
            return Err(failure(
                EnforcerFailureReason::PermissionBindingMismatch,
                None,
            ));
        }
        if permission.generation() != expected_generation
            || self.local.generation.current() != expected_generation
            || expected_generation == u64::MAX
        {
            return Err(failure(EnforcerFailureReason::GenerationChanged, None));
        }
        if !valid_uuid(permission.emitter_id()) || !valid_uuid(permission.claim_operation_id()) {
            return Err(failure(
                EnforcerFailureReason::PermissionBindingMismatch,
                None,
            ));
        }
        self.client
            .0
            .check_health()
            .map_err(|_| failure(EnforcerFailureReason::HelperUnhealthy, None))?;
        let clock = NativeHostClock::new()
            .map_err(|_| failure(EnforcerFailureReason::ClockUnavailable, None))?;
        let sample = clock
            .sample()
            .map_err(|_| failure(EnforcerFailureReason::ClockUnavailable, None))?;
        arm.validate_clock_check(&sample).map_err(|_| {
            failure(
                arm_clock_rejection(&arm.clock, &arm.deadline_ns_decimal, &sample),
                Some(&sample),
            )
        })?;
        let response = self
            .client
            .0
            .request(
                Request::Prepare {
                    arm_id: arm.operation_id.clone(),
                    registration_hash: self.local.digest.clone(),
                    generation: expected_generation,
                },
                IO_TIMEOUT,
            )
            .await
            .map_err(|code| {
                failure(
                    EnforcerFailureReason::PrepareOutcomeUnknown(code),
                    Some(&sample),
                )
            })?;
        let value = match response {
            Response::Prepared { value } => *value,
            Response::Error { code } => {
                return Err(failure(
                    EnforcerFailureReason::RemoteRejected(code),
                    Some(&sample),
                ));
            }
            _ => {
                return Err(failure(
                    EnforcerFailureReason::ResponseMismatch,
                    Some(&sample),
                ));
            }
        };
        if value.arm_id != arm.operation_id || value.registration_hash != self.local.digest {
            return Err(failure(
                EnforcerFailureReason::ResponseBindingMismatch,
                Some(&sample),
            ));
        }
        *self
            .local
            .status
            .lock()
            .map_err(|_| failure(EnforcerFailureReason::LockPoisoned, Some(&sample)))? =
            (value, Instant::now());
        Ok(PrearmedStartControl::with_launch_profile(
            Box::new(SpawnGuard {
                client: self.client.clone(),
                local: Arc::clone(&self.local),
                expected_generation,
                clock,
            }),
            self.local.stop.subscribe(),
            registration.authority.docker_launch_profile.clone(),
        ))
    }

    /// The caller first invalidates memory, then persists CancelRequested, and
    /// only after its durable ACK requests this early stop. This is not disarm.
    pub async fn cancel(&self, generation: u64) -> EnforcerResult<()> {
        if generation != self.local.generation.current() {
            return Err(EnforcerError::Conflict);
        }
        self.local.cancel_durable.store(true, Ordering::SeqCst);
        self.local.stop.send_replace(true);
        let active = self.client.active()?;
        match active
            .0
            .request(
                Request::Cancel {
                    arm_id: self.local.registration.arm.operation_id.clone(),
                    registration_hash: self.local.digest.clone(),
                    generation,
                },
                IO_TIMEOUT,
            )
            .await?
        {
            Response::Acknowledged {} => Ok(()),
            response => Err(response_error(response)),
        }
    }
}

struct SpawnGuard {
    client: EnforcerClient,
    local: Arc<LocalSlot>,
    expected_generation: u64,
    clock: NativeHostClock,
}
impl BeforeSpawnGuard for SpawnGuard {
    fn check_before_spawn(self: Box<Self>, target: &str) -> anyhow::Result<()> {
        let arm = &self.local.registration.arm;
        let failure = |reason, sample: Option<&HostClockSampleV1>| {
            EnforcerStageFailure::from_arm(arm, EnforcerStage::BeforeSpawn, reason, sample)
        };
        let _gate = self
            .local
            .generation
            .0
            .gate
            .lock()
            .map_err(|_| failure(EnforcerFailureReason::LockPoisoned, None))?;
        self.client
            .0
            .check_health()
            .map_err(|_| failure(EnforcerFailureReason::HelperUnhealthy, None))?;
        let (status, received) = &*self
            .local
            .status
            .lock()
            .map_err(|_| failure(EnforcerFailureReason::LockPoisoned, None))?;
        if target != self.local.registration.owned.handle.id() {
            return Err(failure(EnforcerFailureReason::TargetMismatch, None).into());
        }
        if self.local.generation.current() != self.expected_generation
            || self.expected_generation == u64::MAX
        {
            return Err(failure(EnforcerFailureReason::GenerationChanged, None).into());
        }
        if let Some(reason) = spawn_snapshot_failure(status, self.expected_generation, received) {
            return Err(failure(reason, None).with_slot_snapshot(status).into());
        }
        if *self.local.stop.borrow() {
            return Err(failure(EnforcerFailureReason::StopSignalled, None).into());
        }
        let current = self
            .clock
            .sample()
            .map_err(|_| failure(EnforcerFailureReason::ClockUnavailable, None))?;
        let observed = status
            .clock
            .as_ref()
            .ok_or_else(|| failure(EnforcerFailureReason::ClockSnapshotMissing, Some(&current)))?;
        let current_ns = current
            .continuous_ns()
            .map_err(|_| failure(EnforcerFailureReason::NativeClockInvalid, Some(&current)))?;
        let observed_ns = observed
            .continuous_ns()
            .map_err(|_| failure(EnforcerFailureReason::SnapshotClockInvalid, Some(&current)))?;
        if current.method != observed.method || current.boot_id != observed.boot_id {
            return Err(failure(
                EnforcerFailureReason::SnapshotClockIdentityMismatch,
                Some(&current),
            )
            .into());
        }
        if current_ns < observed_ns {
            return Err(failure(EnforcerFailureReason::ClockRegressed, Some(&current)).into());
        }
        if current_ns - observed_ns > HEALTH_WINDOW.as_nanos() {
            return Err(failure(EnforcerFailureReason::ClockSnapshotStale, Some(&current)).into());
        }
        arm.validate_clock_check(&current).map_err(|_| {
            failure(
                arm_clock_rejection(&arm.clock, &arm.deadline_ns_decimal, &current),
                Some(&current),
            )
        })?;
        Ok(())
    }
}

fn claim_snapshot_failure(
    status: &EnforcerSlotStatusV1,
    expected_generation: u64,
    received: &Instant,
    acknowledged: &AtomicBool,
    generation: &CancelGeneration,
    stop: &watch::Sender<bool>,
) -> Option<EnforcerFailureReason> {
    if !acknowledged.load(Ordering::SeqCst) {
        Some(EnforcerFailureReason::SlotUnacknowledged)
    } else if status.prepared_once {
        Some(EnforcerFailureReason::SlotAlreadyPrepared)
    } else if status.phase != SlotPhaseV1::Ready {
        Some(EnforcerFailureReason::SlotPhaseMismatch)
    } else if expected_generation == u64::MAX
        || generation.current() != expected_generation
        || status.generation != expected_generation
    {
        Some(EnforcerFailureReason::GenerationChanged)
    } else if received.elapsed() > HEALTH_WINDOW {
        Some(EnforcerFailureReason::SlotSnapshotStale)
    } else if *stop.borrow() {
        Some(EnforcerFailureReason::StopSignalled)
    } else {
        None
    }
}

fn spawn_snapshot_failure(
    status: &EnforcerSlotStatusV1,
    expected_generation: u64,
    received: &Instant,
) -> Option<EnforcerFailureReason> {
    if status.generation != expected_generation {
        Some(EnforcerFailureReason::GenerationChanged)
    } else if status.phase != SlotPhaseV1::Ready {
        Some(EnforcerFailureReason::SlotPhaseMismatch)
    } else if !status.prepared_once {
        Some(EnforcerFailureReason::SlotNotPrepared)
    } else if received.elapsed() > HEALTH_WINDOW {
        Some(EnforcerFailureReason::SlotSnapshotStale)
    } else {
        None
    }
}

impl Client {
    fn check_health(&self) -> EnforcerResult<()> {
        let mut descriptor = libc::pollfd {
            fd: self.liveness.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialized pollfd for a socket retained by self, timeout
        // zero. POLLHUP is checked even if old heartbeat bytes remain queued.
        let polled = unsafe { libc::poll(&mut descriptor, 1, 0) };
        let health = self.health.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if polled < 0
            || descriptor.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
            || self.failed.load(Ordering::SeqCst)
            || self.lost_unknown.load(Ordering::SeqCst)
            || self.unknown_slots.load(Ordering::SeqCst) != 0
            || health.is_none_or(|(_, received)| received.elapsed() > HEALTH_WINDOW)
        {
            return Err(EnforcerError::Unhealthy);
        }
        Ok(())
    }
}

fn monitor(
    client: Weak<Client>,
    mut stream: UnixStream,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(async move {
        loop {
            let Some(client) = client.upgrade() else {
                return;
            };
            if monitor_once(&client, &mut stream).await.is_err() {
                client.fail();
                let recover = {
                    match client.recovery_gate.lock() {
                        Ok(_gate) => client.auto_recovery.load(Ordering::SeqCst),
                        Err(_) => false,
                    }
                };
                if recover {
                    recover_stops(client).await;
                }
                return;
            }
        }
    })
}
async fn monitor_once(client: &Client, stream: &mut UnixStream) -> EnforcerResult<()> {
    let response: Response = read_frame(stream, MAX_FRAME, IO_TIMEOUT).await?;
    let Response::Status { value } = response else {
        return Err(EnforcerError::Unhealthy);
    };
    if value.epoch != client.endpoint.epoch() || !value.healthy {
        return Err(EnforcerError::Unhealthy);
    }
    {
        let mut health = client.health.lock().map_err(|_| EnforcerError::Unhealthy)?;
        match *health {
            Some((previous, changed)) if previous == value.progress => {
                if changed.elapsed() > HEALTH_WINDOW {
                    return Err(EnforcerError::Unhealthy);
                }
            }
            Some((previous, _)) if previous > value.progress => {
                return Err(EnforcerError::Unhealthy);
            }
            _ => *health = Some((value.progress, Instant::now())),
        }
    }
    let retained: Vec<_> = {
        let slots = client.slots.lock().map_err(|_| EnforcerError::Unhealthy)?;
        slots
            .values()
            .filter(|slot| slot.acknowledged.load(Ordering::SeqCst))
            .cloned()
            .collect()
    };
    for chunk in retained.chunks(MAX_SLOT_QUERY) {
        let arm_ids = chunk
            .iter()
            .map(|slot| slot.registration.arm.operation_id.clone())
            .collect();
        let Response::Slots { values } = client
            .request(Request::Slots { arm_ids }, IO_TIMEOUT)
            .await?
        else {
            return Err(EnforcerError::Unhealthy);
        };
        if values.len() != chunk.len() {
            return Err(EnforcerError::Conflict);
        }
        for (slot, value) in chunk.iter().zip(values) {
            if value.registration_hash != slot.digest
                || value.arm_id != slot.registration.arm.operation_id
            {
                return Err(EnforcerError::Conflict);
            }
            if value.phase != SlotPhaseV1::Ready {
                slot.stop.send_replace(true);
            }
            *slot.status.lock().map_err(|_| EnforcerError::Unhealthy)? = (value, Instant::now());
        }
    }
    Ok(())
}

async fn recover_stops(client: Arc<Client>) {
    // This set was retained before every Register request. The failed bit is
    // already permanent, so no concurrent new target can enter this client.
    let retained: Vec<_> = match client.slots.lock() {
        Ok(slots) => slots.values().cloned().collect(),
        Err(_) => return,
    };
    loop {
        let replacement = match EnforcerClient::connect_or_spawn_mode(
            &client.root,
            &client.executable,
            client.max_slots,
            false,
        )
        .await
        {
            Ok(replacement) => replacement,
            Err(_) => {
                // A silent/occupied endpoint is not takeover permission. Its
                // existing actors may still enforce; keep the global fuse.
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        let new_epoch = replacement.0.endpoint.epoch() != client.endpoint.epoch();
        let unknown_lost = client.lost_unknown.load(Ordering::SeqCst)
            || (new_epoch && client.unknown_slots.load(Ordering::SeqCst) != 0);
        replacement
            .0
            .lost_unknown
            .store(unknown_lost, Ordering::SeqCst);
        let mut restored = true;
        for chunk in retained.chunks(16) {
            let outcomes = futures_util::future::join_all(
                chunk
                    .iter()
                    .map(|local| restore_slot(&replacement.0, Arc::clone(local))),
            )
            .await;
            // Never skip a later target because an earlier authority is down.
            restored &= outcomes.iter().all(Result::is_ok);
        }
        if restored {
            // Publish only after all targets were accepted as ObserveOnly, with
            // original immutable deadlines. Older guards keep their old failed
            // Client forever and can never regain permission through this link.
            let published = {
                match replacement.0.recovery_gate.lock() {
                    Ok(_gate) if !replacement.0.failed.load(Ordering::SeqCst) => {
                        if let Ok(mut next) = client.replacement.lock() {
                            replacement.0.auto_recovery.store(true, Ordering::SeqCst);
                            *next = Some(replacement.clone());
                            true
                        } else {
                            false
                        }
                    }
                    _ => false,
                }
            };
            if published {
                return;
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn restore_slot(client: &Arc<Client>, local: Arc<LocalSlot>) -> EnforcerResult<()> {
    {
        let mut slots = client.slots.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if slots.len() >= client.max_slots as usize
            && !slots.contains_key(&local.registration.arm.operation_id)
        {
            return Err(EnforcerError::Capacity);
        }
        slots.insert(
            local.registration.arm.operation_id.clone(),
            Arc::clone(&local),
        );
    }
    if local.retained_cancel.load(Ordering::SeqCst) {
        return EnforcerClient(Arc::clone(client))
            .cancel_retained(local.registration.clone(), local.generation.clone())
            .await;
    }
    let generation = local.generation.current();
    let response = client
        .request(
            Request::Register {
                registration: Box::new(local.registration.clone()),
                generation,
                observe_only: true,
            },
            REGISTER_TIMEOUT,
        )
        .await?;
    let Response::Registered { value, replayed } = response else {
        return Err(response_error(response));
    };
    let observed_generation = value.generation;
    accept_registration(client, &local, *value, replayed)?;
    local.generation.advance_to(observed_generation);
    if local.cancel_durable.load(Ordering::SeqCst) {
        match client
            .request(
                Request::Cancel {
                    arm_id: local.registration.arm.operation_id.clone(),
                    registration_hash: local.digest.clone(),
                    generation: local.generation.current(),
                },
                IO_TIMEOUT,
            )
            .await?
        {
            Response::Acknowledged {} => {}
            response => return Err(response_error(response)),
        }
    }
    Ok(())
}

fn response_error(response: Response) -> EnforcerError {
    match response {
        Response::Error { code } => code,
        _ => EnforcerError::Unavailable,
    }
}
fn valid_uuid(value: &str) -> bool {
    Uuid::parse_str(value)
        .is_ok_and(|value_id| !value_id.is_nil() && value_id.hyphenated().to_string() == value)
}

fn spawn_helper(root: &Directory, executable: &Path, max_slots: u32) -> EnforcerResult<()> {
    use std::os::unix::process::CommandExt;
    let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    // SAFETY: read-only process resource query into writable ABI storage.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) } != 0 {
        return Err(EnforcerError::Unavailable);
    }
    // SAFETY: successful getrlimit initialized the structure.
    let limit = unsafe { limit.assume_init() }.rlim_cur;
    let mut limit = i32::try_from(limit).map_err(|_| EnforcerError::Unavailable)?;
    // An existing FD can exceed a subsequently lowered soft limit. Inventory
    // those already open too; later opens are bounded by the current limit.
    for entry in std::fs::read_dir("/dev/fd").map_err(|_| EnforcerError::Unavailable)? {
        let entry = entry.map_err(|_| EnforcerError::Unavailable)?;
        let fd: i32 = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
            .ok_or(EnforcerError::Unavailable)?;
        limit = limit.max(fd.checked_add(1).ok_or(EnforcerError::Unavailable)?);
    }
    let mut command = std::process::Command::new(executable);
    command
        .arg("__broker-enforcer")
        .arg("--root")
        .arg(root.path())
        .arg("--max-slots")
        .arg(max_slots.to_string())
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: only async-signal-safe syscalls; no parent lock, token, socket or
    // incidental inherited FD may survive exec. CLOEXEC preserves Command's
    // exec-error pipe until exec, unlike closing all descriptors in pre_exec.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            #[cfg(target_os = "linux")]
            {
                if libc::syscall(
                    libc::SYS_close_range,
                    3_u32,
                    u32::MAX,
                    libc::CLOSE_RANGE_CLOEXEC,
                ) == 0
                {
                    return Ok(());
                }
                if !matches!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::ENOSYS | libc::EINVAL)
                ) {
                    return Err(std::io::Error::last_os_error());
                }
            }
            for fd in 3..limit {
                if libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) < 0
                    && std::io::Error::last_os_error().raw_os_error() != Some(libc::EBADF)
                {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|_| EnforcerError::Unavailable)?;
    // Reap the owned helper when it eventually exits; dropping this handle must
    // not kill it, and the reap thread owns no broker lease/credential FD.
    std::thread::Builder::new()
        .name("enforcer-reap".into())
        .spawn(move || {
            let _ = child.wait();
        })
        .map_err(|_| EnforcerError::Unavailable)?;
    Ok(())
}

#[cfg(test)]
mod stage_diagnostic_tests {
    use super::*;

    #[test]
    fn preclaim_snapshot_distinguishes_each_guard_and_keeps_first_failure_order() {
        let mut status = EnforcerSlotStatusV1 {
            arm_id: "11111111-1111-4111-8111-111111111111".into(),
            registration_hash: "1".repeat(64),
            phase: SlotPhaseV1::Ready,
            prepared_once: false,
            generation: 7,
            clock: None,
            observation: None,
            stop_failures: 0,
        };
        let generation = CancelGeneration::new();
        generation.advance_to(7);
        let acknowledged = AtomicBool::new(true);
        let (stop, _) = watch::channel(false);
        let fresh = Instant::now();
        let stale = fresh - HEALTH_WINDOW - Duration::from_nanos(1);
        let check = |status: &EnforcerSlotStatusV1, expected, received: &Instant| {
            claim_snapshot_failure(
                status,
                expected,
                received,
                &acknowledged,
                &generation,
                &stop,
            )
        };
        assert_eq!(check(&status, 7, &fresh), None);
        acknowledged.store(false, Ordering::SeqCst);
        status.prepared_once = true;
        status.phase = SlotPhaseV1::Quarantined;
        status.generation = 8;
        stop.send_replace(true);
        assert_eq!(
            check(&status, u64::MAX, &stale),
            Some(EnforcerFailureReason::SlotUnacknowledged)
        );
        acknowledged.store(true, Ordering::SeqCst);
        assert_eq!(
            check(&status, u64::MAX, &stale),
            Some(EnforcerFailureReason::SlotAlreadyPrepared)
        );
        status.prepared_once = false;
        assert_eq!(
            check(&status, u64::MAX, &stale),
            Some(EnforcerFailureReason::SlotPhaseMismatch)
        );
        status.phase = SlotPhaseV1::Ready;
        assert_eq!(
            check(&status, u64::MAX, &stale),
            Some(EnforcerFailureReason::GenerationChanged)
        );
        assert_eq!(
            check(&status, 8, &stale),
            Some(EnforcerFailureReason::GenerationChanged)
        );
        assert_eq!(
            check(&status, 7, &stale),
            Some(EnforcerFailureReason::GenerationChanged)
        );
        status.generation = 7;
        assert_eq!(
            check(&status, 7, &stale),
            Some(EnforcerFailureReason::SlotSnapshotStale)
        );
        assert_eq!(
            check(&status, 7, &fresh),
            Some(EnforcerFailureReason::StopSignalled)
        );
        stop.send_replace(false);
        assert_eq!(check(&status, 7, &fresh), None);
    }

    #[test]
    fn final_spawn_snapshot_distinguishes_generation_phase_permit_and_freshness() {
        let mut status = EnforcerSlotStatusV1 {
            arm_id: "11111111-1111-4111-8111-111111111111".into(),
            registration_hash: "1".repeat(64),
            phase: SlotPhaseV1::Ready,
            prepared_once: true,
            generation: 7,
            clock: None,
            observation: None,
            stop_failures: 0,
        };
        let received = Instant::now();
        assert_eq!(spawn_snapshot_failure(&status, 7, &received), None);
        assert_eq!(
            spawn_snapshot_failure(&status, 8, &received),
            Some(EnforcerFailureReason::GenerationChanged)
        );
        status.phase = SlotPhaseV1::Quarantined;
        assert_eq!(
            spawn_snapshot_failure(&status, 7, &received),
            Some(EnforcerFailureReason::SlotPhaseMismatch)
        );
        status.phase = SlotPhaseV1::Ready;
        status.prepared_once = false;
        assert_eq!(
            spawn_snapshot_failure(&status, 7, &received),
            Some(EnforcerFailureReason::SlotNotPrepared)
        );
        status.prepared_once = true;
        let stale = Instant::now() - HEALTH_WINDOW - Duration::from_nanos(1);
        assert_eq!(
            spawn_snapshot_failure(&status, 7, &stale),
            Some(EnforcerFailureReason::SlotSnapshotStale)
        );
    }
}
