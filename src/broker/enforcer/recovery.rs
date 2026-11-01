//! Explicit protocol-2 recovery transport. This epoch never grants admission,
//! a retirement witness, or a conversion of a restored actor into a start slot.
use super::*;
use crate::broker::{
    clock::{HostClock, NativeHostClock},
    discovery::{EnforcerRoleLeaseV2, load_endpoint, recovery_root_binding},
    journal::{
        CancelStopProjectionV1, JournalScopeV1, RevisionRefV1, canonical,
        recovery::{ConfirmedCancelPrefixV1, RecoveredStopV1, StopRecoveryReportV1},
    },
    lifecycle::LocalCommandReceiptV1,
    local_ipc::{EndpointRecordV1, PrivateListener, read_frame, write_frame},
};
use crate::control_fs::Directory;
use hmac::{Hmac, Mac};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Instant,
};
use tokio::net::UnixStream;

const HANDSHAKE_FRAME: u32 = 4096;
const RETRY_WINDOW: Duration = Duration::from_secs(10);
const MAX_PENDING_ATTEMPTS: usize = 16;

#[path = "recovery_v3.rs"]
mod recovery_v3;
pub use recovery_v3::{EnforcerStopClientV3, EnforcerStopStatusV3};

#[cfg(test)]
#[path = "recovery_auth_binding_tests.rs"]
mod auth_binding_tests;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RequestedRole {
    StopsOnly,
    Full,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum LedgerState {
    Unverified { reason: String },
}
fn unverified_ledger() -> LedgerState {
    LedgerState::Unverified {
        reason: "recovery_transport_only".into(),
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChallengeV2 {
    protocol: u32,
    role: String,
    epoch: Uuid,
    nonce: Uuid,
    root_binding: String,
    offered_roles: Vec<RequestedRole>,
    ledger_state: LedgerState,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthenticationV2 {
    requested_role: RequestedRole,
    proof: String,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum AuthenticationReplyV2 {
    Granted {
        granted_role: RequestedRole,
        epoch: Uuid,
        nonce: Uuid,
        root_binding: String,
        ledger_state: LedgerState,
        proof: String,
    },
    FullUnavailable {},
}

fn auth_mac(
    key: &[u8; 32],
    challenge: &ChallengeV2,
    requested: RequestedRole,
    server: bool,
) -> EnforcerResult<Hmac<Sha256>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| EnforcerError::InvalidInput)?;
    mac.update(if server {
        b"ai-guard:enforcer-server-auth:v2\0"
    } else {
        b"ai-guard:enforcer-client-auth:v2\0"
    });
    // A fixed closed tuple supplies canonical field order and binds both the
    // entire challenge (including availability) and the explicitly asked role.
    let bytes = serde_json::to_vec(&(
        challenge.protocol,
        &challenge.role,
        challenge.epoch,
        challenge.nonce,
        &challenge.root_binding,
        &challenge.offered_roles,
        &challenge.ledger_state,
        requested,
    ))
    .map_err(|_| EnforcerError::InvalidInput)?;
    mac.update(&bytes);
    Ok(mac)
}
fn auth_proof(
    key: &[u8; 32],
    challenge: &ChallengeV2,
    role: RequestedRole,
    server: bool,
) -> EnforcerResult<String> {
    Ok(hex::encode(
        auth_mac(key, challenge, role, server)?
            .finalize()
            .into_bytes(),
    ))
}
fn verify_auth(
    key: &[u8; 32],
    challenge: &ChallengeV2,
    role: RequestedRole,
    server: bool,
    proof: &str,
) -> EnforcerResult<()> {
    if !hex_hash(proof) {
        return Err(EnforcerError::Unauthorized);
    }
    let bytes = hex::decode(proof).map_err(|_| EnforcerError::Unauthorized)?;
    auth_mac(key, challenge, role, server)?
        .verify_slice(&bytes)
        .map_err(|_| EnforcerError::Unauthorized)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StopSourceV1 {
    scope_hash: String,
    job_id: Uuid,
    intent_hash: String,
    confirmed_head: RevisionRefV1,
    arm_operation_index: usize,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelReferenceV1 {
    command_id: Uuid,
    input_hash: String,
    confirmed_head: RevisionRefV1,
}
impl CancelReferenceV1 {
    fn from_receipt(receipt: &LocalCommandReceiptV1) -> Self {
        Self {
            command_id: receipt.command_id(),
            input_hash: receipt.input_hash().into(),
            confirmed_head: receipt.head().clone(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreRequiredStopV1 {
    version: u32,
    request_id: Uuid,
    registration: RegistrationV1,
    registration_hash: String,
    source: StopSourceV1,
    cancel_generation: u64,
    durable_cancel: Option<CancelReferenceV1>,
    cancel_prefix: Vec<CancelReferenceV1>,
}
impl RestoreRequiredStopV1 {
    fn validate(&self) -> EnforcerResult<()> {
        self.registration.validate()?;
        if self.version != 1
            || self.request_id.is_nil()
            || self.source.job_id.is_nil()
            || !hex_hash(&self.source.scope_hash)
            || !hex_hash(&self.source.intent_hash)
            || self.source.intent_hash != self.registration.arm.intent_hash
            || self.registration_hash != self.registration.digest()?
            || self.cancel_prefix.len() > 64
            || self.cancel_prefix.last() != self.durable_cancel.as_ref()
            || self.cancel_prefix.iter().any(|cancel| {
                cancel.command_id.is_nil()
                    || !hex_hash(&cancel.input_hash)
                    || cancel.confirmed_head.revision() > self.source.confirmed_head.revision()
            })
            || self
                .cancel_prefix
                .iter()
                .map(|cancel| cancel.command_id)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.cancel_prefix.len()
            || self.durable_cancel.as_ref().is_some_and(|cancel| {
                cancel.command_id.is_nil()
                    || !hex_hash(&cancel.input_hash)
                    || cancel.confirmed_head.revision() > self.source.confirmed_head.revision()
            })
        {
            return Err(EnforcerError::InvalidInput);
        }
        Ok(())
    }
    fn identity(&self) -> EnforcerResult<Vec<u8>> {
        serde_json::to_vec(&(
            &self.registration_hash,
            &self.source.scope_hash,
            self.source.job_id,
            &self.source.intent_hash,
            self.source.arm_operation_index,
        ))
        .map_err(|_| EnforcerError::InvalidInput)
    }
    fn semantic_bytes(&self) -> EnforcerResult<Vec<u8>> {
        // request_id identifies one transport request, not a new desired stop.
        serde_json::to_vec(&(
            &self.source,
            &self.registration_hash,
            self.cancel_generation,
            &self.durable_cancel,
            &self.cancel_prefix,
        ))
        .map_err(|_| EnforcerError::InvalidInput)
    }
}

fn prefix_order(
    current: &[CancelReferenceV1],
    incoming: &[CancelReferenceV1],
) -> EnforcerResult<std::cmp::Ordering> {
    if incoming.starts_with(current) {
        return Ok(incoming.len().cmp(&current.len()));
    }
    if current.starts_with(incoming) {
        return Ok(std::cmp::Ordering::Less);
    }
    Err(EnforcerError::Conflict)
}

// Owning these proofs keeps the actual reader/lease alive across every awaited
// connection and replay. There is deliberately no constructor from a detached
// RecoveredStop, caller JSON, or a caller-supplied trust flag.
#[derive(Clone)]
enum StopOwner {
    Report(Arc<StopRecoveryReportV1>),
    Cancel(Arc<CancelStopProjectionV1>),
}
impl StopOwner {
    fn revalidate(&self) -> EnforcerResult<()> {
        match self {
            Self::Report(report) => report.revalidate(),
            Self::Cancel(projection) => projection.revalidate(),
        }
        .map_err(|_| EnforcerError::UnsafeFilesystem)
    }
}
#[derive(Clone)]
struct RetainedRestore {
    request: RestoreRequiredStopV1,
    owner: StopOwner,
}
type RetainedBatch = Vec<EnforcerResult<RetainedRestore>>;
#[derive(Default)]
struct RestoreBatchResult {
    confirmed: usize,
    failure: Option<EnforcerError>,
}
impl RestoreBatchResult {
    fn failed(error: EnforcerError) -> Self {
        Self {
            confirmed: 0,
            failure: Some(error),
        }
    }
    fn record(&mut self, result: EnforcerResult<()>) {
        if let Err(error) = result {
            self.failure.get_or_insert(error);
        }
    }
}
fn retained_restore(
    owner: StopOwner,
    scope: &JournalScopeV1,
    job_id: Uuid,
    prefix: &ConfirmedCancelPrefixV1,
    target: &RecoveredStopV1,
    cancel: Option<&LocalCommandReceiptV1>,
    generation: u64,
) -> EnforcerResult<RetainedRestore> {
    owner.revalidate()?;
    let registration = RegistrationV1::new(
        hex::encode(Sha256::digest(scope.gateway_origin().as_bytes())),
        scope.tenant_id().into(),
        target.arm().clone(),
        target.receipt().clone(),
        target.authority().clone(),
        target.owned().clone(),
    )?;
    if registration.receipt.control_namespace_uuid != scope.control_namespace_uuid().to_string() {
        return Err(EnforcerError::InvalidInput);
    }
    let cancel_prefix = if let Some(cancel) = cancel {
        if !prefix.receipts().iter().any(|known| {
            known.command_id() == cancel.command_id()
                && known.input_hash() == cancel.input_hash()
                && known.head() == cancel.head()
        }) {
            return Err(EnforcerError::Conflict);
        }
        prefix
            .receipts()
            .iter()
            .map(CancelReferenceV1::from_receipt)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let request = RestoreRequiredStopV1 {
        version: 1,
        request_id: Uuid::new_v4(),
        registration_hash: registration.digest()?,
        registration,
        source: StopSourceV1 {
            scope_hash: hex::encode(Sha256::digest(
                canonical(scope).map_err(|_| EnforcerError::InvalidInput)?,
            )),
            job_id,
            intent_hash: target.arm().intent_hash.clone(),
            confirmed_head: prefix.head().clone(),
            arm_operation_index: target.birth_index(),
        },
        cancel_generation: generation,
        durable_cancel: cancel_prefix.last().cloned(),
        cancel_prefix,
    };
    request.validate()?;
    Ok(RetainedRestore { request, owner })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopPhaseV1 {
    StopRestoring,
    StopReady,
    StopRequested,
    Quarantined,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopSlotV1 {
    arm_id: String,
    registration_hash: String,
    phase: StopPhaseV1,
    generation: u64,
    cancel_command_id: Option<Uuid>,
    durable_cancel: Option<CancelReferenceV1>,
    cancel_prefix: Vec<CancelReferenceV1>,
    stop_failures: u64,
}
impl StopSlotV1 {
    pub fn arm_id(&self) -> &str {
        &self.arm_id
    }
    pub fn registration_hash(&self) -> &str {
        &self.registration_hash
    }
    pub fn phase(&self) -> StopPhaseV1 {
        self.phase
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn cancel_command_id(&self) -> Option<Uuid> {
        self.cancel_command_id
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnforcerStopStatusV2 {
    epoch: Uuid,
    slot_count: usize,
    max_slots: u32,
    progress: u64,
    transport_healthy: bool,
    clock_healthy: bool,
    admission_available: bool,
    ledger_state: LedgerState,
}
impl EnforcerStopStatusV2 {
    pub fn slot_count(&self) -> usize {
        self.slot_count
    }
    pub fn transport_healthy(&self) -> bool {
        self.transport_healthy
    }
    pub fn admission_available(&self) -> bool {
        self.admission_available
    }
    pub fn progress(&self) -> u64 {
        self.progress
    }
    pub fn clock_healthy(&self) -> bool {
        self.clock_healthy
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StopRequest {
    StatusStops {},
    WatchStops {},
    RestoreRequiredStop {
        value: Box<RestoreRequiredStopV1>,
    },
    QueryStop {
        arm_id: String,
        registration_hash: String,
    },
    DeliverDurableStop {
        value: Box<RestoreRequiredStopV1>,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StopResponse {
    StatusStops { value: EnforcerStopStatusV2 },
    StopReserved { value: StopSlotV1, replayed: bool },
    StopStatus { value: StopSlotV1 },
    StopAccepted { value: StopSlotV1 },
    Error { code: EnforcerError },
}

pub(super) struct RecoveryState {
    pub(super) epoch: Uuid,
    pub(super) max_slots: u32,
    pub(super) capability: [u8; 32],
    pub(super) root_binding: String,
    pub(super) clock: Option<Arc<NativeHostClock>>,
    lease: Option<EnforcerRoleLeaseV2>,
    // Retained only by explicitly extended listeners. Missing anchors cannot
    // authenticate V3; legacy V2 state and transport checks remain unchanged.
    transport_v3: Option<(Directory, EndpointRecordV1)>,
    slots: Mutex<BTreeMap<String, Arc<StopActor>>>,
    progress: AtomicU64,
    clock_healthy: AtomicBool,
    transport_healthy: AtomicBool,
    transport_failed_at: AtomicU64,
    #[cfg(test)]
    faults: TestFaults,
}
impl RecoveryState {
    pub(super) fn new(
        epoch: Uuid,
        max_slots: u32,
        capability: [u8; 32],
        root_binding: String,
        clock: Option<Arc<NativeHostClock>>,
        lease: Option<EnforcerRoleLeaseV2>,
    ) -> Self {
        Self {
            epoch,
            max_slots,
            capability,
            root_binding,
            clock,
            lease,
            transport_v3: None,
            slots: Mutex::new(BTreeMap::new()),
            progress: AtomicU64::new(1),
            clock_healthy: AtomicBool::new(false),
            transport_healthy: AtomicBool::new(true),
            transport_failed_at: AtomicU64::new(0),
            #[cfg(test)]
            faults: TestFaults::from_environment(),
        }
    }
    fn retain_transport_v3(
        &mut self,
        root: &Directory,
        endpoint: &EndpointRecordV1,
    ) -> EnforcerResult<()> {
        let lease = self.lease.as_ref().ok_or(EnforcerError::UnsafeFilesystem)?;
        if endpoint.epoch() != self.epoch
            || lease.root_binding(endpoint)? != self.root_binding
            || recovery_root_binding(root, endpoint)? != self.root_binding
        {
            return Err(EnforcerError::UnsafeFilesystem);
        }
        self.transport_v3 = Some((root.clone(), endpoint.clone()));
        Ok(())
    }
    async fn check_transport_v3(self: &Arc<Self>) -> EnforcerResult<()> {
        self.check_transport().await?;
        let anchors = self.transport_v3.clone();
        let lease = self.lease.clone();
        let epoch = self.epoch;
        let binding = self.root_binding.clone();
        let checked = tokio::task::spawn_blocking(move || {
            let (root, endpoint) = anchors.ok_or(EnforcerError::UnsafeFilesystem)?;
            let lease = lease.ok_or(EnforcerError::UnsafeFilesystem)?;
            if endpoint.epoch() != epoch
                || lease.root_binding(&endpoint)? != binding
                || recovery_root_binding(&root, &endpoint)? != binding
            {
                return Err(EnforcerError::UnsafeFilesystem);
            }
            let current = load_endpoint(&root)?.ok_or(EnforcerError::UnsafeFilesystem)?;
            if current.epoch() != epoch
                || lease.root_binding(&current)? != binding
                || recovery_root_binding(&root, &current)? != binding
            {
                return Err(EnforcerError::UnsafeFilesystem);
            }
            lease.revalidate()?;
            root.revalidate()
                .map_err(|_| EnforcerError::UnsafeFilesystem)
        })
        .await
        .map_err(|_| EnforcerError::Unavailable)
        .and_then(|checked| checked);
        if checked.is_err() {
            self.transport_failed();
        }
        checked
    }
    pub(super) fn status(&self) -> EnforcerResult<EnforcerStopStatusV2> {
        let slot_count = self
            .slots
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .len();
        Ok(EnforcerStopStatusV2 {
            epoch: self.epoch,
            slot_count,
            max_slots: self.max_slots,
            progress: self.progress.load(Ordering::SeqCst),
            transport_healthy: self.transport_healthy.load(Ordering::SeqCst),
            clock_healthy: self.clock_healthy.load(Ordering::SeqCst),
            admission_available: false,
            ledger_state: unverified_ledger(),
        })
    }
    async fn check_transport(self: &Arc<Self>) -> EnforcerResult<()> {
        let lease = self.lease.clone();
        let checked = tokio::task::spawn_blocking(move || match lease {
            Some(lease) => lease.revalidate().map_err(EnforcerError::from),
            None => Ok(()),
        })
        .await
        .map_err(|_| EnforcerError::Unavailable)?;
        if checked.is_err() {
            self.transport_failed();
        } else if self.progress.load(Ordering::SeqCst)
            > self.transport_failed_at.load(Ordering::SeqCst)
        {
            // A structural recheck and actual post-incident transport progress
            // may recover STOP availability; neither grants ledger/Full health.
            self.transport_healthy.store(true, Ordering::SeqCst);
        }
        checked
    }
    fn transport_failed(&self) {
        self.transport_failed_at
            .store(self.progress.load(Ordering::SeqCst), Ordering::SeqCst);
        self.transport_healthy.store(false, Ordering::SeqCst);
    }
}
struct StopActor {
    registration: RegistrationV1,
    identity: Vec<u8>,
    value: Mutex<StopSlotV1>,
    stop: AtomicBool,
    // This is startup ownership, never join/quiescence/retirement evidence.
    worker_owned: AtomicBool,
    next_start: Mutex<Instant>,
    _lease: EnforcerRoleLeaseV2,
    #[cfg(test)]
    fail_spawn_once: AtomicBool,
    #[cfg(test)]
    started_marker: Option<PathBuf>,
    #[cfg(test)]
    fault_marker: Option<PathBuf>,
}
impl StopActor {
    fn snapshot(&self) -> EnforcerResult<StopSlotV1> {
        self.value
            .lock()
            .map(|value| value.clone())
            .map_err(|_| EnforcerError::Unhealthy)
    }
}

/// No key, role directory, lock, endpoint parent or ledger is initialized here.
/// Existing V1 daemons retain their own lease; this path never kills them.
pub async fn run_internal_recovery_enforcer(
    root: &Directory,
    max_slots: u32,
) -> EnforcerResult<()> {
    if !(1..=MAX_SLOTS).contains(&max_slots) {
        return Err(EnforcerError::InvalidInput);
    }
    let capability = capability(root, false)?;
    let lease = EnforcerRoleLeaseV2::acquire_existing(root)?;
    let listener = PrivateListener::bind_new(&lease.listener_root()?)?;
    let root_binding = lease.root_binding(listener.endpoint())?;
    let mut state = RecoveryState::new(
        listener.endpoint().epoch(),
        max_slots,
        capability,
        root_binding,
        NativeHostClock::new().ok().map(Arc::new),
        Some(lease.clone()),
    );
    state.retain_transport_v3(root, listener.endpoint())?;
    let state = Arc::new(state);
    lease.publish_endpoint(listener.endpoint())?;
    let connections = Arc::new(tokio::sync::Semaphore::new(32));
    let mut tasks = tokio::task::JoinSet::new();
    let mut progress = tokio::time::interval(Duration::from_millis(25));
    let mut maintenance = tokio::time::interval(POLL);
    loop {
        tokio::select! {
            _ = progress.tick() => {
                // Clock failure changes clock health, never transport progress.
                state.clock_healthy.store(state.clock.as_ref().is_some_and(|clock| clock.sample().is_ok()), Ordering::SeqCst);
                if state.progress.try_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_add(1)).is_err() {
                    state.transport_failed();
                }
            },
            _ = maintenance.tick() => {
                let actors = match state.slots.lock() {
                    Ok(slots) => slots.values().filter(|actor| !actor.worker_owned.load(Ordering::SeqCst)).cloned().collect::<Vec<_>>(),
                    Err(_) => { state.transport_failed(); continue; },
                };
                let mut attempted = 0;
                for actor in actors {
                    if ensure_stop_actor(actor, state.clock.clone()) { attempted += 1; }
                    if attempted == MAX_PENDING_ATTEMPTS { break; }
                }
            },
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
            accepted = listener.accept_checked() => match accepted {
                Ok(stream) => {
                    let Ok(permit) = Arc::clone(&connections).try_acquire_owned() else { continue; };
                    let state = Arc::clone(&state);
                    tasks.spawn(async move { let _permit = permit; let _ = serve_connection(stream, state).await; });
                },
                Err(crate::broker::local_ipc::IpcError::Unauthorized) => {},
                Err(_) => {
                    state.transport_failed();
                    // Do not abandon already armed actors when new transport
                    // metadata becomes unusable. Their leases/deadlines live.
                    tokio::time::sleep(POLL).await;
                },
            }
        }
    }
}

pub(super) async fn serve_connection(
    mut stream: UnixStream,
    state: Arc<RecoveryState>,
) -> EnforcerResult<()> {
    let handshake_started = Instant::now();
    state.check_transport().await?;
    let challenge = recovery_v3::challenge_for(&state);
    write_frame(&mut stream, &challenge, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
    let authentication = match read_frame(&mut stream, HANDSHAKE_FRAME, IO_TIMEOUT).await? {
        recovery_v3::FirstAuthentication::V2(authentication) => authentication,
        recovery_v3::FirstAuthentication::V3(negotiate) => {
            return recovery_v3::serve(
                stream,
                state,
                challenge,
                negotiate,
                handshake_started + recovery_v3::HANDSHAKE_WINDOW,
            )
            .await;
        }
    };
    verify_auth(
        &state.capability,
        &challenge,
        authentication.requested_role,
        false,
        &authentication.proof,
    )?;
    if authentication.requested_role != RequestedRole::StopsOnly {
        write_frame(
            &mut stream,
            &AuthenticationReplyV2::FullUnavailable {},
            HANDSHAKE_FRAME,
            IO_TIMEOUT,
        )
        .await?;
        return Ok(());
    }
    let response = AuthenticationReplyV2::Granted {
        granted_role: RequestedRole::StopsOnly,
        epoch: challenge.epoch,
        nonce: challenge.nonce,
        root_binding: challenge.root_binding.clone(),
        ledger_state: challenge.ledger_state.clone(),
        proof: auth_proof(
            &state.capability,
            &challenge,
            RequestedRole::StopsOnly,
            true,
        )?,
    };
    write_frame(&mut stream, &response, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
    // Closed parsing is the role allowlist. Full commands are refused before
    // dispatch and can never reach the legacy register/prepare implementation.
    let raw: serde_json::Value = read_frame(&mut stream, MAX_FRAME, IO_TIMEOUT).await?;
    let request = match serde_json::from_value::<StopRequest>(raw) {
        Ok(request) => request,
        Err(_) => {
            write_frame(
                &mut stream,
                &StopResponse::Error {
                    code: EnforcerError::Unauthorized,
                },
                MAX_FRAME,
                IO_TIMEOUT,
            )
            .await?;
            return Ok(());
        }
    };
    #[cfg(test)]
    if state.faults.close_before(&request) {
        // Fault injection closes a real authenticated fixture connection
        // before dispatch. It never fabricates a reservation or effect ACK.
        return Err(EnforcerError::Unavailable);
    }
    if matches!(request, StopRequest::WatchStops {}) {
        loop {
            state.check_transport().await?;
            let value = state.status()?;
            write_frame(
                &mut stream,
                &StopResponse::StatusStops { value },
                MAX_FRAME,
                IO_TIMEOUT,
            )
            .await?;
            tokio::time::sleep(POLL).await;
        }
    }
    let response = match dispatch(&state, request) {
        Ok(response) => response,
        Err(code) => StopResponse::Error { code },
    };
    #[cfg(test)]
    state.faults.pause_after_reserved(&response).await?;
    write_frame(&mut stream, &response, MAX_FRAME, IO_TIMEOUT).await?;
    Ok(())
}
fn dispatch(state: &Arc<RecoveryState>, request: StopRequest) -> EnforcerResult<StopResponse> {
    match request {
        StopRequest::StatusStops {} => Ok(StopResponse::StatusStops {
            value: state.status()?,
        }),
        StopRequest::WatchStops {} => Err(EnforcerError::InvalidInput),
        StopRequest::RestoreRequiredStop { value } => reserve(state, *value),
        StopRequest::QueryStop {
            arm_id,
            registration_hash,
        } => {
            let actor = state
                .slots
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .get(&arm_id)
                .cloned()
                .ok_or(EnforcerError::Conflict)?;
            if actor.registration.digest()? != registration_hash {
                return Err(EnforcerError::Conflict);
            }
            Ok(StopResponse::StopStatus {
                value: actor.snapshot()?,
            })
        }
        StopRequest::DeliverDurableStop { value } => {
            value.validate()?;
            let cancel = value
                .durable_cancel
                .as_ref()
                .ok_or(EnforcerError::InvalidInput)?;
            let identity = value.identity()?;
            let actor = state
                .slots
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .get(&value.registration.arm.operation_id)
                .cloned()
                .ok_or(EnforcerError::Conflict)?;
            if actor.identity != identity {
                return Err(EnforcerError::Conflict);
            }
            let mut slot = actor.value.lock().map_err(|_| EnforcerError::Unhealthy)?;
            let order = prefix_order(&slot.cancel_prefix, &value.cancel_prefix)?;
            slot.generation = slot.generation.max(value.cancel_generation);
            if order != std::cmp::Ordering::Less {
                slot.cancel_command_id = Some(cancel.command_id);
                slot.durable_cancel = Some(cancel.clone());
                slot.cancel_prefix = value.cancel_prefix;
            }
            slot.phase = StopPhaseV1::StopRequested;
            actor.stop.store(true, Ordering::SeqCst);
            Ok(StopResponse::StopAccepted {
                value: slot.clone(),
            })
        }
    }
}
fn reserve(
    state: &Arc<RecoveryState>,
    request: RestoreRequiredStopV1,
) -> EnforcerResult<StopResponse> {
    request.validate()?;
    let identity = request.identity()?;
    let (actor, replayed) = {
        let mut slots = state.slots.lock().map_err(|_| EnforcerError::Unhealthy)?;
        let key = request.registration.arm.operation_id.clone();
        if let Some(actor) = slots.get(&key) {
            if actor.identity != identity {
                return Err(EnforcerError::Conflict);
            }
            (Arc::clone(actor), true)
        } else {
            if slots.len() >= state.max_slots as usize {
                return Err(EnforcerError::Capacity);
            }
            let lease = state.lease.clone().ok_or(EnforcerError::UnsafeFilesystem)?;
            let actor = Arc::new(StopActor {
                registration: request.registration,
                identity,
                value: Mutex::new(StopSlotV1 {
                    arm_id: key.clone(),
                    registration_hash: request.registration_hash,
                    phase: StopPhaseV1::StopRestoring,
                    generation: request.cancel_generation,
                    cancel_command_id: None,
                    durable_cancel: None,
                    cancel_prefix: Vec::new(),
                    stop_failures: 0,
                }),
                stop: AtomicBool::new(false),
                worker_owned: AtomicBool::new(false),
                next_start: Mutex::new(Instant::now()),
                _lease: lease,
                #[cfg(test)]
                fail_spawn_once: AtomicBool::new(state.faults.take_spawn_failure(&key)),
                #[cfg(test)]
                started_marker: state.faults.started_marker.clone(),
                #[cfg(test)]
                fault_marker: state.faults.fault_marker.clone(),
            });
            slots.insert(key, Arc::clone(&actor));
            (actor, false)
        }
    };
    // Reservation precedes spawning. No runtime reopen, thread creation or
    // engine IO occurs while the actor map is locked, including on failure.
    ensure_stop_actor(Arc::clone(&actor), state.clock.clone());
    Ok(StopResponse::StopReserved {
        value: actor.snapshot()?,
        replayed,
    })
}
fn ensure_stop_actor(actor: Arc<StopActor>, clock: Option<Arc<NativeHostClock>>) -> bool {
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
    if start_stop_actor(Arc::clone(&actor), clock).is_err() {
        if let Ok(mut value) = actor.value.lock() {
            value.phase = StopPhaseV1::Quarantined;
            value.stop_failures = value.stop_failures.saturating_add(1);
        }
        // spawn returned Err: no OS thread ever owned this attempt. A runtime
        // reopen failure inside a live thread never clears this ownership.
        actor.worker_owned.store(false, Ordering::SeqCst);
    }
    true
}
fn start_stop_actor(
    actor: Arc<StopActor>,
    clock: Option<Arc<NativeHostClock>>,
) -> EnforcerResult<()> {
    #[cfg(test)]
    if actor.fail_spawn_once.swap(false, Ordering::SeqCst) {
        record_fault(&actor.fault_marker);
        return Err(EnforcerError::Unavailable);
    }
    #[cfg(test)]
    let spawned_marker = actor
        .started_marker
        .as_ref()
        .map(|marker| marker.with_extension("spawned"));
    std::thread::Builder::new()
        .name("enforcer-recovered-stop".into())
        .stack_size(1024 * 1024)
        .spawn(move || {
            #[cfg(test)]
            record_started(&actor.started_marker);
            let runtime = loop {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => break runtime,
                    Err(_) => {
                        if let Ok(mut value) = actor.value.lock() {
                            value.phase = StopPhaseV1::Quarantined;
                            value.stop_failures = value.stop_failures.saturating_add(1);
                        }
                        // Keep the same OS thread while its runtime resources
                        // are unavailable. No second worker or fake join.
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            };
            let managed = loop {
                match runtime.block_on(crate::runtime::ManagedRuntime::reopen(
                    actor.registration.authority.clone(),
                )) {
                    Ok(managed) => break managed,
                    Err(_) => {
                        if let Ok(mut value) = actor.value.lock() {
                            value.phase = StopPhaseV1::Quarantined;
                            value.stop_failures = value.stop_failures.saturating_add(1);
                        }
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            };
            let mut stopping = false;
            let mut last_ns = None;
            loop {
                let live = clock
                    .as_ref()
                    .and_then(|clock| clock.sample().ok())
                    .is_some_and(|sample| {
                        let Ok(now) = sample.continuous_ns() else {
                            return false;
                        };
                        let monotone = last_ns.is_none_or(|last| now >= last);
                        last_ns = Some(now);
                        monotone && actor.registration.arm.validate_clock_check(&sample).is_ok()
                    });
                stopping |= !live || actor.stop.load(Ordering::SeqCst);
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
                // A transport/observation success is never a drained old start or
                // a retirement witness. Keep the exact CID and original deadline.
                std::thread::sleep(Duration::from_millis(250));
            }
        })
        .map_err(|_| EnforcerError::Unavailable)?;
    #[cfg(test)]
    record_marker(&spawned_marker);
    Ok(())
}

/// An authenticated StopsOnly capability. No Deref, From, start guard or Full
/// conversion exists. Each request reconnects explicitly in protocol 2.
#[derive(Clone)]
pub struct EnforcerStopClient(Arc<StopClient>);
#[derive(Clone, Copy, PartialEq, Eq)]
enum StopProtocol {
    V2,
    V3,
}
#[cfg(test)]
std::thread_local! {
    static TEST_CREATED_STOP_CLIENTS: std::cell::RefCell<Vec<std::sync::Weak<StopClient>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}
/// Harness-only weak ownership observation. Extinction is not an actor join,
/// engine drain, lease proof or D5 quiescence certificate.
#[cfg(test)]
pub(crate) struct TestStopClientOwners(Vec<std::sync::Weak<StopClient>>);
#[cfg(test)]
impl TestStopClientOwners {
    pub(crate) fn is_extinct(&self) -> bool {
        self.0.iter().all(|owner| owner.strong_count() == 0)
    }
}
struct StopClient {
    root: Directory,
    executable: PathBuf,
    max_slots: u32,
    capability: [u8; 32],
    protocol: StopProtocol,
    cache: Mutex<StopCache>,
    changed: Arc<tokio::sync::Notify>,
    watch_failed: AtomicBool,
    attempts: Arc<tokio::sync::Semaphore>,
    #[cfg(test)]
    observed_watch_epoch: Mutex<Option<Uuid>>,
    #[cfg(test)]
    open_barrier: Mutex<Option<Arc<TestOpenBarrier>>>,
    #[cfg(test)]
    observed_epochs: Mutex<Vec<Uuid>>,
}
#[derive(Default)]
struct StopCache {
    epoch: Option<Uuid>,
    observation_version: u64,
    next_attempt: u64,
    entries: BTreeMap<String, PendingRestore>,
}
struct PendingRestore {
    retained: RetainedRestore,
    revision: u64,
    acknowledged: Option<(Uuid, u64)>,
    in_flight: Option<u64>,
    retry_at: Instant,
    failures: u32,
}
struct RestoreAttempt {
    key: String,
    revision: u64,
    token: u64,
    retained: RetainedRestore,
    _capacity: tokio::sync::OwnedSemaphorePermit,
}
enum Claim {
    Acknowledged,
    Busy,
    Attempt(Box<RestoreAttempt>),
}
struct StopConnection {
    stream: UnixStream,
    epoch: Uuid,
    v3: Option<recovery_v3::Session3>,
}

#[derive(Default)]
struct WatchProgress {
    epoch: Option<Uuid>,
    progress: u64,
    advanced_at: Option<Instant>,
}
impl WatchProgress {
    fn accept(
        &mut self,
        expected: Uuid,
        value: &EnforcerStopStatusV2,
        now: Instant,
    ) -> EnforcerResult<()> {
        if value.epoch != expected || !value.transport_healthy || value.admission_available {
            return Err(EnforcerError::Unhealthy);
        }
        if self.epoch != Some(expected) {
            self.epoch = Some(expected);
            self.progress = value.progress;
            self.advanced_at = Some(now);
            return Ok(());
        }
        if value.progress < self.progress {
            return Err(EnforcerError::Unhealthy);
        }
        if value.progress > self.progress {
            self.progress = value.progress;
            self.advanced_at = Some(now);
        }
        if self
            .advanced_at
            .is_none_or(|last| now.saturating_duration_since(last) > HEALTH_WINDOW)
        {
            return Err(EnforcerError::Unhealthy);
        }
        // Clock health is deliberately absent: an unknown clock makes STOP
        // conservative but does not invalidate advancing transport progress.
        Ok(())
    }
}
impl fmt::Debug for EnforcerStopClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EnforcerStopClient(<private>)")
    }
}
impl EnforcerStopClient {
    #[cfg(test)]
    pub(crate) fn take_created_clients_for_test() -> TestStopClientOwners {
        TEST_CREATED_STOP_CLIENTS.with(|owners| TestStopClientOwners(owners.take()))
    }
    pub async fn connect_or_spawn_for_recovery(
        root: &Directory,
        executable: &Path,
        max_slots: u32,
    ) -> EnforcerResult<Self> {
        Self::connect_with_protocol(root, executable, max_slots, StopProtocol::V2).await
    }
    async fn connect_with_protocol(
        root: &Directory,
        executable: &Path,
        max_slots: u32,
        protocol: StopProtocol,
    ) -> EnforcerResult<Self> {
        if !(1..=MAX_SLOTS).contains(&max_slots) || !executable.is_absolute() {
            return Err(EnforcerError::InvalidInput);
        }
        root.revalidate()
            .map_err(|_| EnforcerError::UnsafeFilesystem)?;
        let capability = capability(root, false)?;
        let client = Self(Arc::new(StopClient {
            root: root.clone(),
            executable: executable.into(),
            max_slots,
            capability,
            protocol,
            cache: Mutex::new(StopCache::default()),
            changed: Arc::new(tokio::sync::Notify::new()),
            watch_failed: AtomicBool::new(false),
            attempts: Arc::new(tokio::sync::Semaphore::new(MAX_PENDING_ATTEMPTS)),
            #[cfg(test)]
            observed_watch_epoch: Mutex::new(None),
            #[cfg(test)]
            open_barrier: Mutex::new(None),
            #[cfg(test)]
            observed_epochs: Mutex::new(Vec::new()),
        }));
        #[cfg(test)]
        TEST_CREATED_STOP_CLIENTS.with(|owners| {
            owners.borrow_mut().push(Arc::downgrade(&client.0));
        });
        client.ensure_open().await?;
        tokio::spawn(monitor_recovery(Arc::downgrade(&client.0)));
        Ok(client)
    }
    fn observe_connection(
        &self,
        connection: StopConnection,
        expected_version: u64,
    ) -> Result<StopConnection, OpenFailure> {
        let mut cache = self
            .0
            .cache
            .lock()
            .map_err(|_| OpenFailure::Other(EnforcerError::Unhealthy))?;
        // The version was captured before open's filesystem/network IO. A
        // successor published during that IO cannot be overwritten even when
        // this old connection passed its final endpoint check before a pause.
        if cache.observation_version != expected_version {
            drop(cache);
            return Err(OpenFailure::Superseded);
        }
        if cache.epoch != Some(connection.epoch) {
            cache.observation_version = cache
                .observation_version
                .checked_add(1)
                .ok_or(OpenFailure::Other(EnforcerError::Capacity))?;
            cache.epoch = Some(connection.epoch);
            #[cfg(test)]
            if let Ok(mut observed) = self.0.observed_epochs.lock() {
                observed.push(connection.epoch);
            }
            self.0.changed.notify_one();
        }
        Ok(connection)
    }
    async fn open_observed(&self) -> Result<StopConnection, OpenFailure> {
        let version = {
            let cache = self
                .0
                .cache
                .lock()
                .map_err(|_| OpenFailure::Other(EnforcerError::Unhealthy))?;
            cache.observation_version
        };
        let connection = self.open().await?;
        self.observe_connection(connection, version)
    }
    async fn ensure_open(&self) -> EnforcerResult<StopConnection> {
        let observation_expires = Instant::now() + RETRY_WINDOW;
        loop {
            match self.open_observed().await {
                Ok(connection) => return Ok(connection),
                Err(OpenFailure::Absent) => break,
                Err(OpenFailure::Other(error)) => return Err(error),
                Err(OpenFailure::Superseded) => {
                    if Instant::now() >= observation_expires {
                        return Err(EnforcerError::Unavailable);
                    }
                }
            }
        }
        let lease = EnforcerRoleLeaseV2::acquire_existing(&self.0.root)?;
        drop(lease);
        super::client::spawn_recovery_helper(&self.0.root, &self.0.executable, self.0.max_slots)?;
        let expires = Instant::now() + RETRY_WINDOW;
        loop {
            match self.open_observed().await {
                Ok(connection) => return Ok(connection),
                Err(OpenFailure::Absent) => {}
                Err(OpenFailure::Other(error)) => return Err(error),
                Err(OpenFailure::Superseded) => {}
            }
            if std::time::Instant::now() >= expires {
                return Err(EnforcerError::Unavailable);
            }
            tokio::time::sleep(POLL).await;
        }
    }
    #[cfg(test)]
    pub(crate) fn observed_watch_epoch(&self) -> Option<Uuid> {
        *self.0.observed_watch_epoch.lock().ok()?
    }
    #[cfg(test)]
    pub(crate) fn pause_next_checked_open(&self) -> Arc<TestOpenBarrier> {
        let barrier = Arc::new(TestOpenBarrier::default());
        *self.0.open_barrier.lock().expect("private open barrier") = Some(Arc::clone(&barrier));
        barrier
    }
    #[cfg(test)]
    pub(crate) fn observation_history(&self) -> Vec<Uuid> {
        self.0
            .observed_epochs
            .lock()
            .expect("private epoch observation history")
            .clone()
    }
    #[cfg(test)]
    pub(crate) fn connection_epoch(&self) -> Option<Uuid> {
        self.0.cache.lock().ok().and_then(|cache| cache.epoch)
    }
    async fn open(&self) -> Result<StopConnection, OpenFailure> {
        let root = &self.0.root;
        root.revalidate()
            .map_err(|_| OpenFailure::Other(EnforcerError::UnsafeFilesystem))?;
        if capability(root, false).map_err(OpenFailure::Other)? != self.0.capability {
            return Err(OpenFailure::Other(EnforcerError::Unauthorized));
        }
        let endpoint = load_endpoint(root)
            .map_err(|e| OpenFailure::Other(e.into()))?
            .ok_or(OpenFailure::Absent)?;
        let endpoints = root
            .persistent_child("endpoints", false)
            .map_err(|_| OpenFailure::Other(EnforcerError::UnsafeFilesystem))?
            .ok_or(OpenFailure::Other(EnforcerError::UnsafeFilesystem))?;
        let binding =
            recovery_root_binding(root, &endpoint).map_err(|e| OpenFailure::Other(e.into()))?;
        let handshake_deadline = (self.0.protocol == StopProtocol::V3)
            .then(|| Instant::now() + recovery_v3::HANDSHAKE_WINDOW);
        let mut stream = match endpoint.connect_checked(&endpoints).await {
            Ok(stream) => stream,
            Err(crate::broker::local_ipc::IpcError::Unavailable) => {
                return Err(OpenFailure::Absent);
            }
            Err(error) => return Err(OpenFailure::Other(error.into())),
        };
        let frame_timeout = match handshake_deadline {
            Some(deadline) => recovery_v3::time_left(deadline).map_err(OpenFailure::Other)?,
            None => IO_TIMEOUT,
        };
        let challenge: ChallengeV2 = if handshake_deadline.is_some() {
            recovery_v3::read_preface(&mut stream, frame_timeout)
                .await
                .map_err(OpenFailure::Other)?
        } else {
            read_frame(&mut stream, HANDSHAKE_FRAME, frame_timeout)
                .await
                .map_err(|e| OpenFailure::Other(e.into()))?
        };
        if challenge.protocol != 2
            || challenge.role != "enforcer"
            || challenge.epoch != endpoint.epoch()
            || challenge.nonce.is_nil()
            || challenge.root_binding != binding
            || challenge.offered_roles != [RequestedRole::StopsOnly]
            || challenge.ledger_state != unverified_ledger()
        {
            return Err(OpenFailure::Other(EnforcerError::Unauthorized));
        }
        let v3 = if let Some(deadline) = handshake_deadline {
            Some(
                recovery_v3::client_handshake(
                    &mut stream,
                    &challenge,
                    &self.0.capability,
                    deadline,
                    RequestedRole::StopsOnly,
                    self.0.max_slots,
                )
                .await
                .map_err(OpenFailure::Other)?,
            )
        } else {
            let authentication = AuthenticationV2 {
                requested_role: RequestedRole::StopsOnly,
                proof: auth_proof(
                    &self.0.capability,
                    &challenge,
                    RequestedRole::StopsOnly,
                    false,
                )
                .map_err(OpenFailure::Other)?,
            };
            write_frame(&mut stream, &authentication, HANDSHAKE_FRAME, IO_TIMEOUT)
                .await
                .map_err(|e| OpenFailure::Other(e.into()))?;
            let reply: AuthenticationReplyV2 = read_frame(&mut stream, HANDSHAKE_FRAME, IO_TIMEOUT)
                .await
                .map_err(|e| OpenFailure::Other(e.into()))?;
            match reply {
                AuthenticationReplyV2::Granted {
                    granted_role: RequestedRole::StopsOnly,
                    epoch,
                    nonce,
                    root_binding,
                    ledger_state,
                    proof,
                } if epoch == challenge.epoch
                    && nonce == challenge.nonce
                    && root_binding == binding
                    && ledger_state == challenge.ledger_state =>
                {
                    verify_auth(
                        &self.0.capability,
                        &challenge,
                        RequestedRole::StopsOnly,
                        true,
                        &proof,
                    )
                    .map_err(OpenFailure::Other)?;
                }
                _ => return Err(OpenFailure::Other(EnforcerError::Unauthorized)),
            }
            None
        };
        if recovery_root_binding(root, &endpoint).map_err(|e| OpenFailure::Other(e.into()))?
            != binding
        {
            return Err(OpenFailure::Other(EnforcerError::UnsafeFilesystem));
        }
        if load_endpoint(root)
            .map_err(|e| OpenFailure::Other(e.into()))?
            .is_none_or(|latest| latest.epoch() != endpoint.epoch())
        {
            return Err(OpenFailure::Other(EnforcerError::Conflict));
        }
        if let Some(deadline) = handshake_deadline {
            recovery_v3::time_left(deadline).map_err(OpenFailure::Other)?;
        }
        #[cfg(test)]
        {
            let barrier = self
                .0
                .open_barrier
                .lock()
                .ok()
                .and_then(|barrier| barrier.clone());
            if let Some(barrier) = barrier {
                barrier.pause(endpoint.epoch()).await;
            }
        }
        Ok(StopConnection {
            stream,
            epoch: endpoint.epoch(),
            v3,
        })
    }
    async fn request(&self, request: StopRequest) -> EnforcerResult<(Uuid, StopResponse)> {
        let mut connection = self.ensure_open().await?;
        if connection.v3.is_some() {
            let response = recovery_v3::request(&mut connection, request).await?;
            return Ok((connection.epoch, response.into_legacy()));
        }
        write_frame(&mut connection.stream, &request, MAX_FRAME, IO_TIMEOUT).await?;
        let response: StopResponse =
            read_frame(&mut connection.stream, MAX_FRAME, IO_TIMEOUT).await?;
        match response {
            StopResponse::Error { code } => Err(code),
            response => Ok((connection.epoch, response)),
        }
    }
    pub async fn status(&self) -> EnforcerResult<EnforcerStopStatusV2> {
        match self.request(StopRequest::StatusStops {}).await? {
            (epoch, StopResponse::StatusStops { value })
                if value.epoch == epoch
                    && value.max_slots == self.0.max_slots
                    && !value.admission_available
                    && !self.0.watch_failed.load(Ordering::SeqCst) =>
            {
                Ok(value)
            }
            _ => Err(EnforcerError::Unavailable),
        }
    }
    #[cfg(test)]
    pub(crate) async fn negotiate_v3_frame_for_test(
        &self,
    ) -> EnforcerResult<(Uuid, String, Uuid, Uuid, EnforcerResult<serde_json::Value>)> {
        tokio::time::timeout(Duration::from_secs(6), async {
            // This is a separate, ungranted connection. In particular, do not
            // call ensure_open(), which has already authenticated Stops2.
            let root = &self.0.root;
            root.revalidate()
                .map_err(|_| EnforcerError::UnsafeFilesystem)?;
            let key = capability(root, false)?;
            if key != self.0.capability {
                return Err(EnforcerError::Unauthorized);
            }
            let endpoint = load_endpoint(root)?.ok_or(EnforcerError::Unavailable)?;
            let endpoints = root
                .persistent_child("endpoints", false)
                .map_err(|_| EnforcerError::UnsafeFilesystem)?
                .ok_or(EnforcerError::UnsafeFilesystem)?;
            let binding = recovery_root_binding(root, &endpoint)?;
            let mut stream = endpoint.connect_checked(&endpoints).await?;
            let challenge: ChallengeV2 =
                read_frame(&mut stream, HANDSHAKE_FRAME, IO_TIMEOUT).await?;
            if challenge.protocol != 2
                || challenge.role != "enforcer"
                || challenge.epoch != endpoint.epoch()
                || challenge.nonce.is_nil()
                || challenge.root_binding != binding
                || challenge.offered_roles != [RequestedRole::StopsOnly]
                || challenge.ledger_state != unverified_ledger()
            {
                return Err(EnforcerError::Unauthorized);
            }
            let requested_role = RequestedRole::StopsOnly;
            let client_nonce = Uuid::new_v4();
            if client_nonce.is_nil() || client_nonce == challenge.nonce {
                return Err(EnforcerError::InvalidInput);
            }
            // N uses the seven values actually received in the closed V2
            // preface, followed by protocol3, requested role and client nonce.
            let transcript = serde_json::to_vec(&(
                challenge.protocol,
                &challenge.role,
                challenge.epoch,
                challenge.nonce,
                &challenge.root_binding,
                &challenge.offered_roles,
                &challenge.ledger_state,
                3_u32,
                requested_role,
                client_nonce,
            ))
            .map_err(|_| EnforcerError::InvalidInput)?;
            let mut mac =
                Hmac::<Sha256>::new_from_slice(&key).map_err(|_| EnforcerError::InvalidInput)?;
            mac.update(b"ai-guard:enforcer-client-negotiate:v3\0");
            mac.update(&transcript);
            let proof = hex::encode(mac.finalize().into_bytes());
            write_frame(
                &mut stream,
                &serde_json::json!({
                    "kind": "negotiate_v3",
                    "protocol": 3_u32,
                    "requested_role": requested_role,
                    "client_nonce": client_nonce,
                    "proof": proof,
                }),
                HANDSHAKE_FRAME,
                IO_TIMEOUT,
            )
            .await?;
            // Separate a setup/write error from the behavioral absence of a
            // Challenge3 after the exact authenticated Negotiate3 was sent.
            let response: EnforcerResult<serde_json::Value> =
                read_frame(&mut stream, HANDSHAKE_FRAME, IO_TIMEOUT)
                    .await
                    .map_err(EnforcerError::from);
            if recovery_root_binding(root, &endpoint)? != binding {
                return Err(EnforcerError::UnsafeFilesystem);
            }
            if load_endpoint(root)?.is_none_or(|current| current.epoch() != endpoint.epoch()) {
                return Err(EnforcerError::Conflict);
            }
            Ok((
                challenge.epoch,
                binding,
                challenge.nonce,
                client_nonce,
                response,
            ))
        })
        .await
        .map_err(|_| EnforcerError::Unavailable)?
    }
    pub async fn query_stop(
        &self,
        arm_id: &str,
        registration_hash: &str,
    ) -> EnforcerResult<StopSlotV1> {
        match self
            .request(StopRequest::QueryStop {
                arm_id: arm_id.into(),
                registration_hash: registration_hash.into(),
            })
            .await?
        {
            (_, StopResponse::StopStatus { value })
                if value.arm_id == arm_id && value.registration_hash == registration_hash =>
            {
                Ok(value)
            }
            _ => Err(EnforcerError::Conflict),
        }
    }
    fn retain_in(
        cache: &mut StopCache,
        max_slots: u32,
        mut retained: RetainedRestore,
    ) -> EnforcerResult<String> {
        let key = retained.request.registration.arm.operation_id.clone();
        if let Some(previous) = cache.entries.get_mut(&key) {
            if previous.retained.request.identity()? != retained.request.identity()? {
                return Err(EnforcerError::Conflict);
            }
            let order = prefix_order(
                &previous.retained.request.cancel_prefix,
                &retained.request.cancel_prefix,
            )?;
            let generation = previous
                .retained
                .request
                .cancel_generation
                .max(retained.request.cancel_generation);
            if order != std::cmp::Ordering::Greater {
                retained = previous.retained.clone();
            }
            retained.request.cancel_generation = generation;
            if previous.retained.request.semantic_bytes()? != retained.request.semantic_bytes()? {
                previous.revision = previous
                    .revision
                    .checked_add(1)
                    .ok_or(EnforcerError::Capacity)?;
                previous.retained = retained;
                previous.retry_at = Instant::now();
                previous.failures = 0;
                // An in-flight older version retains ownership of its token;
                // its eventual ACK cannot clear this new desired revision.
            }
        } else {
            if cache.entries.len() >= max_slots as usize {
                return Err(EnforcerError::Capacity);
            }
            cache.entries.insert(
                key.clone(),
                PendingRestore {
                    retained,
                    revision: 1,
                    acknowledged: None,
                    in_flight: None,
                    retry_at: Instant::now(),
                    failures: 0,
                },
            );
        }
        Ok(key)
    }
    fn retain_batch(&self, prepared: RetainedBatch) -> (Vec<String>, RestoreBatchResult) {
        let mut outcome = RestoreBatchResult::default();
        let mut keys = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        {
            let Ok(mut cache) = self.0.cache.lock() else {
                return (keys, RestoreBatchResult::failed(EnforcerError::Unhealthy));
            };
            // Memory-only publication of the whole known set. The monitor
            // cannot claim A before B/C have also been considered and retained.
            for target in prepared {
                match target
                    .and_then(|target| Self::retain_in(&mut cache, self.0.max_slots, target))
                {
                    Ok(key) if seen.insert(key.clone()) => keys.push(key),
                    Ok(_) => {}
                    Err(error) => {
                        outcome.failure.get_or_insert(error);
                    }
                }
            }
        }
        self.0.changed.notify_one();
        (keys, outcome)
    }
    fn claim(&self, key: &str) -> EnforcerResult<Claim> {
        let mut cache = self.0.cache.lock().map_err(|_| EnforcerError::Unhealthy)?;
        let entry = cache.entries.get(key).ok_or(EnforcerError::Conflict)?;
        if cache
            .epoch
            .is_some_and(|epoch| entry.acknowledged == Some((epoch, entry.revision)))
        {
            return Ok(Claim::Acknowledged);
        }
        if cache.epoch.is_none() || entry.in_flight.is_some() || entry.retry_at > Instant::now() {
            return Ok(Claim::Busy);
        }
        let Ok(capacity) = Arc::clone(&self.0.attempts).try_acquire_owned() else {
            return Ok(Claim::Busy);
        };
        cache.next_attempt = cache
            .next_attempt
            .checked_add(1)
            .ok_or(EnforcerError::Capacity)?;
        let token = cache.next_attempt;
        let entry = cache.entries.get_mut(key).ok_or(EnforcerError::Conflict)?;
        entry.in_flight = Some(token);
        Ok(Claim::Attempt(Box::new(RestoreAttempt {
            key: key.into(),
            revision: entry.revision,
            token,
            retained: entry.retained.clone(),
            _capacity: capacity,
        })))
    }
    fn finish_attempt(
        &self,
        attempt: &RestoreAttempt,
        result: &EnforcerResult<Uuid>,
    ) -> EnforcerResult<()> {
        let mut cache = self.0.cache.lock().map_err(|_| EnforcerError::Unhealthy)?;
        let epoch = cache.epoch;
        let entry = cache
            .entries
            .get_mut(&attempt.key)
            .ok_or(EnforcerError::Conflict)?;
        if entry.in_flight != Some(attempt.token) {
            return Err(EnforcerError::Conflict);
        }
        entry.in_flight = None;
        if entry.revision == attempt.revision {
            match result {
                Ok(accepted_epoch) if epoch == Some(*accepted_epoch) => {
                    entry.acknowledged = Some((*accepted_epoch, attempt.revision));
                    entry.failures = 0;
                }
                Err(_) => {
                    entry.failures = entry.failures.saturating_add(1);
                    entry.retry_at = Instant::now() + POLL * (1_u32 << entry.failures.min(4));
                }
                _ => entry.retry_at = Instant::now(),
            }
        } else {
            entry.retry_at = Instant::now();
        }
        self.0.changed.notify_waiters();
        Ok(())
    }
    fn spawn_attempt(
        &self,
        attempt: Box<RestoreAttempt>,
    ) -> tokio::sync::oneshot::Receiver<EnforcerResult<()>> {
        let client = self.clone();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        // The operation owns its token independently of the foreground waiter.
        // Cancelling that waiter cannot strand the retained target in-flight.
        tokio::spawn(async move {
            let result = client.perform_attempt(attempt.retained.clone()).await;
            let finished = client.finish_attempt(&attempt, &result);
            drop(attempt);
            let _ = sender.send(finished.and(result.map(|_| ())));
        });
        receiver
    }
    async fn wait_retained_key(&self, key: String) -> EnforcerResult<()> {
        let expires = Instant::now() + REGISTER_TIMEOUT;
        loop {
            if Instant::now() >= expires {
                return Err(EnforcerError::Unavailable);
            }
            match self.claim(&key)? {
                Claim::Acknowledged => return Ok(()),
                Claim::Attempt(attempt) => {
                    self.spawn_attempt(attempt)
                        .await
                        .map_err(|_| EnforcerError::Unavailable)??;
                    // A successful RPC can now be historical (or a newer
                    // desired revision can exist). Count only the current
                    // epoch/revision ACK on the next claim, never that RPC alone.
                }
                Claim::Busy => {
                    let _ = tokio::time::timeout(POLL, self.0.changed.notified()).await;
                }
            }
        }
    }
    fn start_owned_batch(
        &self,
        prepare: impl FnOnce() -> RetainedBatch + Send + 'static,
    ) -> tokio::sync::oneshot::Receiver<RestoreBatchResult> {
        let client = self.clone();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        // This task owns preparation, every proof and every internal waiter.
        // Dropping the public receiver does not cancel the remaining B/C work.
        tokio::spawn(async move {
            let result = match tokio::task::spawn_blocking(prepare).await {
                Ok(prepared) => client.restore_prepared_batch(prepared).await,
                Err(_) => RestoreBatchResult::failed(EnforcerError::Unavailable),
            };
            let _ = sender.send(result);
        });
        receiver
    }
    async fn restore_prepared_batch(&self, prepared: RetainedBatch) -> RestoreBatchResult {
        let (keys, mut outcome) = self.retain_batch(prepared);
        if keys.is_empty() {
            return outcome;
        }
        // Authenticate the current announcement before reusing any cached ACK.
        // All targets are already retained; no FS/network IO holds their map.
        if let Err(error) = self.ensure_open().await {
            outcome.failure.get_or_insert(error);
            return outcome;
        }
        let mut waiters = tokio::task::JoinSet::new();
        let confirmation_keys = keys.clone();
        for key in keys {
            if waiters.len() >= MAX_PENDING_ATTEMPTS
                && let Some(result) = waiters.join_next().await
            {
                outcome.record(result.unwrap_or(Err(EnforcerError::Unavailable)));
            }
            let client = self.clone();
            waiters.spawn(async move { client.wait_retained_key(key).await });
        }
        while let Some(result) = waiters.join_next().await {
            outcome.record(result.unwrap_or(Err(EnforcerError::Unavailable)));
        }
        // Take one current-epoch view for the result as well: E2 may have been
        // observed after one waiter returned its valid but now historical ACK.
        match self.0.cache.lock() {
            Ok(cache) => {
                outcome.confirmed = confirmation_keys
                    .iter()
                    .filter(|key| {
                        cache.entries.get(*key).is_some_and(|entry| {
                            cache.epoch.is_some_and(|epoch| {
                                entry.acknowledged == Some((epoch, entry.revision))
                            })
                        })
                    })
                    .count();
                if outcome.confirmed != confirmation_keys.len() {
                    outcome.failure.get_or_insert(EnforcerError::Unavailable);
                }
            }
            Err(_) => {
                outcome.failure.get_or_insert(EnforcerError::Unhealthy);
            }
        }
        outcome
    }
    async fn perform_attempt(&self, mut retained: RetainedRestore) -> EnforcerResult<Uuid> {
        let owner = retained.owner.clone();
        tokio::task::spawn_blocking(move || owner.revalidate())
            .await
            .map_err(|_| EnforcerError::Unavailable)??;
        match self
            .request(StopRequest::RestoreRequiredStop {
                value: Box::new(retained.request.clone()),
            })
            .await?
        {
            (reserved_epoch, StopResponse::StopReserved { value, .. })
                if value.arm_id == retained.request.registration.arm.operation_id
                    && value.registration_hash == retained.request.registration_hash =>
            {
                if retained.request.durable_cancel.is_some() {
                    retained.request.cancel_generation =
                        retained.request.cancel_generation.max(value.generation);
                    match self
                        .request(StopRequest::DeliverDurableStop {
                            value: Box::new(retained.request.clone()),
                        })
                        .await?
                    {
                        (accepted_epoch, StopResponse::StopAccepted { value: stopped })
                            if accepted_epoch == reserved_epoch
                                && stopped.arm_id == value.arm_id
                                && stopped.registration_hash == value.registration_hash
                                && stopped.durable_cancel == retained.request.durable_cancel
                                && stopped.cancel_prefix == retained.request.cancel_prefix
                                && stopped.generation == retained.request.cancel_generation => {}
                        _ => return Err(EnforcerError::Conflict),
                    }
                }
                Ok(reserved_epoch)
            }
            _ => Err(EnforcerError::Conflict),
        }
    }
    pub(crate) async fn restore_report_job(
        &self,
        report: Arc<StopRecoveryReportV1>,
        job_index: usize,
        generation: u64,
    ) -> (usize, bool) {
        let receiver = self.start_owned_batch(move || {
            let Some(job) = report.jobs().get(job_index) else {
                return vec![Err(EnforcerError::InvalidInput)];
            };
            job.stops()
                .iter()
                .enumerate()
                .map(|(index, target)| {
                    let cancel = target
                        .cancel_after_arm()
                        .map(|command| {
                            job.cancel_receipt(command)
                                .ok_or(EnforcerError::InvalidInput)
                        })
                        .transpose()?;
                    let prefix = job
                        .cancel_prefix_for_stop(index)
                        .map_err(|_| EnforcerError::InvalidInput)?;
                    retained_restore(
                        StopOwner::Report(Arc::clone(&report)),
                        report.scope(),
                        job.operation_id(),
                        &prefix,
                        target,
                        cancel,
                        generation,
                    )
                })
                .collect()
        });
        let outcome = receiver
            .await
            .unwrap_or_else(|_| RestoreBatchResult::failed(EnforcerError::Unavailable));
        // These are control confirmations, never observed stopped workers.
        (outcome.confirmed, outcome.failure.is_some())
    }
    pub(crate) async fn deliver_projection(
        &self,
        projection: Arc<CancelStopProjectionV1>,
        generation: u64,
    ) -> EnforcerResult<()> {
        let receiver = self.start_owned_batch(move || {
            projection
                .stops()
                .iter()
                .enumerate()
                .map(|(index, target)| {
                    let prefix = projection
                        .cancel_prefix_for_stop(index)
                        .ok_or(EnforcerError::InvalidInput)?;
                    retained_restore(
                        StopOwner::Cancel(Arc::clone(&projection)),
                        projection.scope(),
                        projection.operation_id(),
                        prefix,
                        target,
                        Some(projection.receipt()),
                        generation,
                    )
                })
                .collect()
        });
        let outcome = receiver.await.map_err(|_| EnforcerError::Unavailable)?;
        outcome.failure.map_or(Ok(()), Err)
    }
}
type WatchReader = (
    Uuid,
    tokio::sync::mpsc::Receiver<EnforcerResult<StopResponse>>,
    tokio::task::JoinHandle<()>,
);
async fn close_watch(watch: &mut Option<WatchReader>) {
    if let Some((_, _, task)) = watch.take() {
        task.abort();
        let _ = task.await;
    }
}
async fn begin_watch(
    connection: &mut StopConnection,
) -> EnforcerResult<Option<recovery_v3::Watch3Reader>> {
    if connection.v3.is_some() {
        return recovery_v3::start_watch(connection).await.map(Some);
    }
    write_frame(
        &mut connection.stream,
        &StopRequest::WatchStops {},
        MAX_FRAME,
        IO_TIMEOUT,
    )
    .await?;
    Ok(None)
}
async fn monitor_recovery(weak: std::sync::Weak<StopClient>) {
    let mut watch = None::<WatchReader>;
    let mut progress = WatchProgress::default();
    loop {
        let Some(inner) = weak.upgrade() else {
            close_watch(&mut watch).await;
            return;
        };
        let client = EnforcerStopClient(inner);
        if watch.is_none() {
            if let Ok(mut connection) = client.ensure_open().await
                && let Ok(mut watch3) = begin_watch(&mut connection).await
            {
                let epoch = connection.epoch;
                let (sender, receiver) = tokio::sync::mpsc::channel(4);
                let task = tokio::spawn(async move {
                    loop {
                        // This dedicated reader is never cancelled by a
                        // Notify/timer while part-way through a frame.
                        let frame = match &mut watch3 {
                            Some(reader) => reader
                                .read(&mut connection.stream)
                                .await
                                .map(|value| StopResponse::StatusStops { value }),
                            None => read_frame(&mut connection.stream, MAX_FRAME, IO_TIMEOUT)
                                .await
                                .map_err(EnforcerError::from),
                        };
                        let terminal = frame.is_err();
                        if sender.send(frame).await.is_err() || terminal {
                            return;
                        }
                    }
                });
                watch = Some((epoch, receiver, task));
            }
            if watch.is_none() {
                client.0.watch_failed.store(true, Ordering::SeqCst);
            }
        }
        if Arc::strong_count(&client.0) == 1 {
            // A reconnect must not make this Weak monitor the sole owner of
            // the client/proof cache and keep a dropped service alive forever.
            drop(client);
            close_watch(&mut watch).await;
            return;
        }
        let keys = client
            .0
            .cache
            .lock()
            .ok()
            .map(|cache| {
                cache
                    .entries
                    .iter()
                    .filter(|(_, entry)| {
                        entry.in_flight.is_none()
                            && entry.retry_at <= Instant::now()
                            && cache.epoch.is_some_and(|epoch| {
                                entry.acknowledged != Some((epoch, entry.revision))
                            })
                    })
                    .take(MAX_PENDING_ATTEMPTS)
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for key in keys {
            if let Ok(Claim::Attempt(attempt)) = client.claim(&key) {
                // Per-Arm tokens plus the shared permit bound both foreground
                // and background work. A slow A never awaits inside B's sweep.
                drop(client.spawn_attempt(attempt));
            }
        }
        let changed = Arc::clone(&client.0.changed);
        drop(client);
        let frame = if let Some((epoch, receiver, _)) = &mut watch {
            tokio::select! {
                frame = receiver.recv() => Some((*epoch, frame)),
                _ = changed.notified() => None,
                _ = tokio::time::sleep(POLL) => None,
            }
        } else {
            let _ = tokio::time::timeout(POLL, changed.notified()).await;
            None
        };
        if let Some((expected, frame)) = frame {
            let Some(inner) = weak.upgrade() else {
                close_watch(&mut watch).await;
                return;
            };
            let client = EnforcerStopClient(inner);
            let current_epoch = client.0.cache.lock().ok().and_then(|cache| cache.epoch);
            let accepted = match frame {
                Some(Ok(StopResponse::StatusStops { value }))
                    if current_epoch == Some(expected) =>
                {
                    progress.accept(expected, &value, Instant::now())
                }
                _ => Err(EnforcerError::Unhealthy),
            };
            if accepted.is_ok() {
                client.0.watch_failed.store(false, Ordering::SeqCst);
                #[cfg(test)]
                if let Ok(mut observed) = client.0.observed_watch_epoch.lock() {
                    *observed = Some(expected);
                }
            } else {
                if !client.0.watch_failed.swap(true, Ordering::SeqCst) {
                    eprintln!(
                        "enforcer recovery: watch epoch/progress is unavailable; pending STOP targets retained"
                    );
                }
                close_watch(&mut watch).await;
            }
        }
    }
}
enum OpenFailure {
    Absent,
    Other(EnforcerError),
    Superseded,
}

/// Test-only pause after a genuine checked/authenticated open, immediately
/// before that connection can publish its epoch into the client cache.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestOpenBarrier {
    claimed: AtomicBool,
    entered: Mutex<Option<Uuid>>,
    released: AtomicBool,
    passed: AtomicBool,
}
#[cfg(test)]
impl TestOpenBarrier {
    async fn pause(&self, epoch: Uuid) {
        if self.claimed.swap(true, Ordering::SeqCst) {
            return;
        }
        *self.entered.lock().expect("private checked-open epoch") = Some(epoch);
        let expires = Instant::now() + Duration::from_secs(5);
        while !self.released.load(Ordering::SeqCst) && Instant::now() < expires {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        self.passed.store(true, Ordering::SeqCst);
    }
    pub(crate) fn entered_epoch(&self) -> Option<Uuid> {
        *self.entered.lock().expect("private checked-open epoch")
    }
    pub(crate) fn is_blocked(&self) -> bool {
        self.entered_epoch().is_some() && !self.passed.load(Ordering::SeqCst)
    }
    pub(crate) fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
    }
}

// These faults exist only in the libtest executable explicitly spawned by
// recovery tests. Production binaries have neither the environment reader nor
// the fault fields/branches. Markers contain fixed fixture bytes, no payload.
#[cfg(test)]
struct TestFaults {
    stage: String,
    arm_id: Option<String>,
    armed: AtomicBool,
    barrier_arm: Option<String>,
    barrier_root: Option<PathBuf>,
    barrier_once: AtomicBool,
    repeat_barrier: bool,
    started_marker: Option<PathBuf>,
    fault_marker: Option<PathBuf>,
}
#[cfg(test)]
impl TestFaults {
    fn from_environment() -> Self {
        Self {
            stage: std::env::var("AI_GUARD_TEST_RECOVERY_FAULT").unwrap_or_default(),
            arm_id: std::env::var("AI_GUARD_TEST_RECOVERY_FAULT_ARM").ok(),
            armed: AtomicBool::new(true),
            barrier_arm: std::env::var("AI_GUARD_TEST_RECOVERY_BARRIER_ARM").ok(),
            barrier_root: std::env::var_os("AI_GUARD_TEST_RECOVERY_BARRIER_ROOT")
                .map(PathBuf::from),
            barrier_once: AtomicBool::new(true),
            repeat_barrier: std::env::var_os("AI_GUARD_TEST_RECOVERY_REPEAT_BARRIER").is_some(),
            started_marker: std::env::var_os("AI_GUARD_TEST_RECOVERY_STARTED_MARKER")
                .map(PathBuf::from),
            fault_marker: std::env::var_os("AI_GUARD_TEST_RECOVERY_FAULT_MARKER")
                .map(PathBuf::from),
        }
    }
    fn selected(&self, arm_id: &str) -> bool {
        self.arm_id
            .as_deref()
            .is_none_or(|selected| selected == arm_id)
    }
    fn close_before(&self, request: &StopRequest) -> bool {
        let selected = match request {
            StopRequest::RestoreRequiredStop { value } if self.stage == "before_reserve" => {
                self.selected(&value.registration.arm.operation_id)
            }
            StopRequest::DeliverDurableStop { value } if self.stage == "before_deliver" => {
                self.selected(&value.registration.arm.operation_id)
            }
            _ => false,
        };
        let consumed = selected && self.armed.swap(false, Ordering::SeqCst);
        if consumed {
            record_fault(&self.fault_marker);
        }
        consumed
    }
    fn take_spawn_failure(&self, arm_id: &str) -> bool {
        self.stage == "spawn_once"
            && self.selected(arm_id)
            && self.armed.swap(false, Ordering::SeqCst)
    }
    async fn pause_after_reserved(&self, response: &StopResponse) -> EnforcerResult<()> {
        let StopResponse::StopReserved { value, .. } = response else {
            return Ok(());
        };
        if self.barrier_arm.as_deref() != Some(value.arm_id.as_str())
            || (!self.repeat_barrier && !self.barrier_once.swap(false, Ordering::SeqCst))
        {
            return Ok(());
        }
        let Some(root) = &self.barrier_root else {
            return Err(EnforcerError::InvalidInput);
        };
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let marker = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join("reserved"));
        match marker {
            Ok(mut marker) => marker
                .write_all(b"fixture-reserved\n")
                .map_err(|_| EnforcerError::Unavailable)?,
            Err(error)
                if self.repeat_barrier && error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(EnforcerError::Unavailable),
        }
        let expires = std::time::Instant::now() + Duration::from_secs(5);
        while !root.join("release").exists() {
            if std::time::Instant::now() >= expires {
                return Err(EnforcerError::Unavailable);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    }
}
#[cfg(test)]
fn record_started(marker: &Option<PathBuf>) {
    record_marker(marker);
    if let Some(path) = marker {
        let expires = Instant::now() + Duration::from_secs(5);
        while !path.with_extension("release").exists() && Instant::now() < expires {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
#[cfg(test)]
fn record_marker(marker: &Option<PathBuf>) {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    if let Some(path) = marker {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .expect("private fixture start marker");
        file.write_all(b"s")
            .expect("record actual fixture thread entry");
    }
}
#[cfg(test)]
fn record_fault(marker: &Option<PathBuf>) {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    if let Some(path) = marker {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .expect("private fixture consumed-fault marker");
        file.write_all(b"fault-consumed\n")
            .expect("record genuine fixture fault consumption");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::os::unix::fs::PermissionsExt;

    fn state() -> Arc<RecoveryState> {
        Arc::new(RecoveryState::new(
            Uuid::new_v4(),
            8,
            [0x5a; 32],
            "a".repeat(64),
            None,
            None,
        ))
    }

    #[tokio::test]
    async fn recovery_transport_recovers_after_safe_lease_check_and_new_progress() {
        let temp = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = Directory::open_existing(&temp.path().canonicalize().unwrap()).unwrap();
        root.persistent_create("auth", &[0x5a; 32]).unwrap();
        root.persistent_create(".owner.lock", &[]).unwrap();
        root.persistent_child("endpoints", true).unwrap().unwrap();
        root.persistent_child("announcements", true)
            .unwrap()
            .unwrap();
        let lease = EnforcerRoleLeaseV2::acquire_existing(&root).unwrap();
        let state = Arc::new(RecoveryState::new(
            Uuid::new_v4(),
            8,
            [0x5a; 32],
            "a".repeat(64),
            None,
            Some(lease),
        ));
        // This injects the reported transport state, not an OS EMFILE event.
        // The revalidation below uses the actual retained filesystem lease.
        state.transport_failed();
        state.check_transport().await.unwrap();
        assert!(!state.status().unwrap().transport_healthy());
        state.progress.store(2, Ordering::SeqCst);
        state.check_transport().await.unwrap();
        let recovered = state.status().unwrap();
        assert!(recovered.transport_healthy());
        assert!(!recovered.clock_healthy());
        assert!(!recovered.admission_available());
    }

    #[tokio::test]
    async fn recovery_watch_validates_epoch_and_progress_independently_of_clock_health() {
        let state = state();
        let (server, mut client) = UnixStream::pair().unwrap();
        let task = tokio::spawn(serve_connection(server, Arc::clone(&state)));
        let challenge: ChallengeV2 = read_frame(&mut client, HANDSHAKE_FRAME, IO_TIMEOUT)
            .await
            .unwrap();
        let proof = auth_proof(&[0x5a; 32], &challenge, RequestedRole::StopsOnly, false).unwrap();
        write_frame(
            &mut client,
            &json!({"requested_role":"stops_only","proof":proof}),
            HANDSHAKE_FRAME,
            IO_TIMEOUT,
        )
        .await
        .unwrap();
        let _: AuthenticationReplyV2 = read_frame(&mut client, HANDSHAKE_FRAME, IO_TIMEOUT)
            .await
            .unwrap();
        write_frame(
            &mut client,
            &json!({"kind":"watch_stops"}),
            MAX_FRAME,
            IO_TIMEOUT,
        )
        .await
        .unwrap();
        let first = read_frame::<StopResponse>(&mut client, MAX_FRAME, IO_TIMEOUT).await;
        state.progress.store(2, Ordering::SeqCst);
        let second = read_frame::<StopResponse>(&mut client, MAX_FRAME, IO_TIMEOUT).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        drop(client);
        let StopResponse::StatusStops { value: first } = first.unwrap() else {
            panic!("first real Watch status");
        };
        let StopResponse::StatusStops { value: second } = second.unwrap() else {
            panic!("second real Watch status");
        };
        assert!(!first.clock_healthy && !second.clock_healthy);
        assert!(second.progress > first.progress);
        let now = Instant::now();
        let mut policy = WatchProgress::default();
        policy.accept(challenge.epoch, &first, now).unwrap();
        policy.accept(challenge.epoch, &second, now + POLL).unwrap();
        let mut wrong_epoch = second.clone();
        wrong_epoch.epoch = Uuid::new_v4();
        assert!(
            policy
                .accept(challenge.epoch, &wrong_epoch, now + POLL * 2)
                .is_err()
        );
        let mut backwards = second.clone();
        backwards.progress = first.progress;
        assert!(
            policy
                .accept(challenge.epoch, &backwards, now + POLL * 2)
                .is_err()
        );
        assert!(
            policy
                .accept(challenge.epoch, &second, now + HEALTH_WINDOW + POLL * 2)
                .is_err(),
            "continued frames without advancement must become unhealthy"
        );
    }

    #[tokio::test]
    async fn recovery_auth_rejects_role_tampering_and_domain_replay() {
        let state = state();
        let (server, mut client) = UnixStream::pair().unwrap();
        let task = tokio::spawn(serve_connection(server, state));
        let challenge: Value = read_frame(&mut client, HANDSHAKE_FRAME, IO_TIMEOUT)
            .await
            .unwrap();
        let challenge: ChallengeV2 = serde_json::from_value(challenge).unwrap();
        let proof = auth_proof(&[0x5a; 32], &challenge, RequestedRole::StopsOnly, false).unwrap();
        assert!(
            verify_auth(
                &[0x5a; 32],
                &challenge,
                RequestedRole::StopsOnly,
                true,
                &proof
            )
            .is_err()
        );
        write_frame(
            &mut client,
            &json!({ "requested_role": "full", "proof": proof }),
            HANDSHAKE_FRAME,
            IO_TIMEOUT,
        )
        .await
        .unwrap();
        let result = task.await.unwrap();
        assert!(matches!(result, Err(EnforcerError::Unauthorized)));
    }

    #[tokio::test]
    async fn recovery_full_role_is_explicitly_unavailable() {
        let (server, mut client) = UnixStream::pair().unwrap();
        let task = tokio::spawn(serve_connection(server, state()));
        let challenge: ChallengeV2 = read_frame(&mut client, HANDSHAKE_FRAME, IO_TIMEOUT)
            .await
            .unwrap();
        let proof = auth_proof(&[0x5a; 32], &challenge, RequestedRole::Full, false).unwrap();
        write_frame(
            &mut client,
            &json!({ "requested_role": "full", "proof": proof }),
            HANDSHAKE_FRAME,
            IO_TIMEOUT,
        )
        .await
        .unwrap();
        let reply: Value = read_frame(&mut client, HANDSHAKE_FRAME, IO_TIMEOUT)
            .await
            .unwrap();
        task.await.unwrap().unwrap();
        assert_eq!(reply, json!({"kind":"full_unavailable"}));
    }

    #[tokio::test]
    async fn recovery_stops_role_rejects_all_full_dispatches() {
        for kind in [
            "register",
            "prepare",
            "prepare_bound",
            "claim",
            "start",
            "cleanup",
            "export",
            "finalize_retirement",
        ] {
            let (server, mut client) = UnixStream::pair().unwrap();
            let task = tokio::spawn(serve_connection(server, state()));
            let challenge: ChallengeV2 = read_frame(&mut client, HANDSHAKE_FRAME, IO_TIMEOUT)
                .await
                .unwrap();
            let proof =
                auth_proof(&[0x5a; 32], &challenge, RequestedRole::StopsOnly, false).unwrap();
            write_frame(
                &mut client,
                &json!({ "requested_role": "stops_only", "proof": proof }),
                HANDSHAKE_FRAME,
                IO_TIMEOUT,
            )
            .await
            .unwrap();
            let reply: AuthenticationReplyV2 = read_frame(&mut client, HANDSHAKE_FRAME, IO_TIMEOUT)
                .await
                .unwrap();
            let AuthenticationReplyV2::Granted { proof, .. } = reply else {
                panic!("StopsOnly grant required");
            };
            verify_auth(
                &[0x5a; 32],
                &challenge,
                RequestedRole::StopsOnly,
                true,
                &proof,
            )
            .unwrap();
            write_frame(&mut client, &json!({"kind":kind}), MAX_FRAME, IO_TIMEOUT)
                .await
                .unwrap();
            let reply: Value = read_frame(&mut client, MAX_FRAME, IO_TIMEOUT)
                .await
                .unwrap();
            task.await.unwrap().unwrap();
            assert_eq!(
                reply,
                json!({"kind":"error","code":"unauthorized"}),
                "{kind} must be refused before dispatch"
            );
        }
    }

    #[test]
    fn recovery_transport_does_not_cap_or_parse_ledger_bytes() {
        let temp = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = temp.path().canonicalize().unwrap();
        let root = Directory::open_existing(&path).unwrap();
        root.persistent_create(".owner.lock", &[]).unwrap();
        root.persistent_child("endpoints", true).unwrap().unwrap();
        root.persistent_child("announcements", true)
            .unwrap()
            .unwrap();
        // Sparse, private test data: no ledger parsing, allocation, append or
        // admission authority is needed to retain this same lock for STOP.
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path.join(".owner.lock"))
            .unwrap();
        lock.set_len(33 * 1024 * 1024).unwrap();
        drop(lock);
        let lease = EnforcerRoleLeaseV2::acquire_existing(&root)
            .expect("structurally safe large ledger must preserve STOP transport");
        lease.revalidate().unwrap();
        let binding = root.enforcer_recovery_binding().unwrap();
        assert_ne!(binding[3], 0);
        assert_ne!(binding[4], 0);
    }
}
