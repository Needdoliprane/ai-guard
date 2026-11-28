use super::diagnostics::{EnforcerFailureReason, EnforcerStage, arm_clock_rejection};
use super::*;
use super::{
    recovery::recovery_v3,
    retirement::{FullCommand, FullReply},
};
use crate::broker::{retirement::*, retirement_checkpoint::Full3RegistrationBindingV1};
use crate::{
    broker::{
        clock::{HostClock, NativeHostClock},
        discovery::{RoleLease, load_endpoint},
        local_ipc::{EndpointRecordV1, read_frame, write_frame},
    },
    control_fs::Directory,
    egress::ids::IdV1,
    runner::broker::FreshStartPermission,
    runtime::{
        PrearmedStartControl,
        bootstrap_stdin::{
            BootstrapStdinSubmitError, NativeBootstrapRelease, NativeConnectGuard,
            NativeConnectRefusal,
        },
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::{Path, PathBuf},
    process::{Child, Stdio},
    sync::{Weak, atomic::AtomicBool},
    time::Instant,
};
use tokio::{net::UnixStream, sync::watch};

const RETAINED_CANCEL_TIMEOUT: Duration = Duration::from_secs(5);
const RETAINED_CANCEL_ATTEMPTS: usize = 3;
// DATA retention only. These slots do not become native S/W owners until a
// real installation ACK; surplus must still reach bounded native catch-up.
const MAX_LEGACY_RETAINED_STOPS: usize = 16_384;

#[path = "client_legacy.rs"]
mod legacy;
use legacy::LegacyTransport;
#[cfg(test)]
pub(in crate::broker) use legacy::connect_named_legacy_fixture;
pub(crate) use legacy::{
    LegacyNamedProspectiveBind, LegacyNamedProspectiveBindAck, LegacyNamedProspectiveFailure,
};

#[derive(Clone)]
pub struct EnforcerClient(Arc<Client>);
impl fmt::Debug for EnforcerClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EnforcerClient(<private>)")
    }
}
struct Client {
    authority: SlotAuthorityV1,
    legacy: Option<Arc<LegacyTransport>>,
    root: Directory,
    executable: PathBuf,
    max_slots: u32,
    endpoint_root: Directory,
    endpoint: EndpointRecordV1,
    capability: [u8; 32],
    liveness: OwnedFd,
    #[cfg(test)]
    owned_peer_pid_for_test: Option<u32>,
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
enum SlotAuthorityV1 {
    Legacy,
    Full3(Arc<FullTransport>),
}
struct FullTransport {
    ledger: crate::broker::retirement::LedgerIdentityV1,
    binding: String,
    registrations: Mutex<BTreeMap<String, RetainedFullRegistration>>,
    // Retained before the Arm RPC. An unknown outcome or a dropped waiter
    // cannot remove its exact pending identity, snapshot or native reservation.
    prospective: Mutex<BTreeMap<String, Arc<LocalProspective>>>,
    // A failed/unknown first pending publication cannot cause another Arm ID
    // to be generated from a later empty-looking journal read.
    prearm_write_unknown: AtomicBool,
    #[cfg(test)]
    lose_query_reply_after_successes: Mutex<Option<usize>>,
    #[cfg(test)]
    lose_next_bind_reply_after_native: AtomicBool,
}
struct LocalProspective {
    target: ProspectiveRegistrationV1,
    digest: String,
    generation: CancelGeneration,
    reservation: Mutex<Option<ProspectiveReservationV1>>,
    bound: AtomicBool,
    original_exchange_nonce: Mutex<Option<Uuid>>,
}
/// Process capability created only from the authenticated native phase
/// transition. It is never deserialized, cloned or reconstructed from DATA.
pub(crate) struct ProspectiveArmExchange {
    epoch: Uuid,
    arm_id: String,
    prospective_hash: String,
    generation: u64,
    original_exchange_nonce: Option<Uuid>,
    legacy_named_reservation: Option<legacy::LegacyNamedProspectiveReservation>,
}
impl ProspectiveArmExchange {
    pub(crate) fn permits_original(&self) -> bool {
        self.original_exchange_nonce.is_some()
    }
    pub(crate) fn into_restrictive(mut self) -> Self {
        self.original_exchange_nonce = None;
        self
    }
}
struct OriginalRegistration {
    nonce: Uuid,
}
struct RetainedFullRegistration {
    binding: Full3RegistrationBindingV1,
    actor_nonce: Uuid,
    // A verified fresh ACK alone creates this live capability. Replay data
    // never reconstructs it; an existing actual owner may retain its own.
    original: Option<OriginalRegistration>,
}

#[derive(Clone)]
pub(crate) struct EnforcerFullClientV3(EnforcerClient);
impl fmt::Debug for EnforcerFullClientV3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EnforcerFullClientV3(<private explicit Full3>)")
    }
}

/// Local custody from one Fresh launch. A reconnecting client has no Child
/// and cannot request the service-level idle drain.
pub(crate) struct FreshEnforcerOwner {
    client: EnforcerFullClientV3,
    process: SpawnedHelper,
    shutdown_acknowledged: bool,
}

impl FreshEnforcerOwner {
    /// `Ok(false)` means the guardian acknowledged idle shutdown and its
    /// exact child exit remains pending.
    pub(crate) async fn shutdown_if_idle(&mut self) -> EnforcerResult<bool> {
        let child = self.process.0.as_mut().ok_or(EnforcerError::Unavailable)?;
        let pid = child.id();
        let reply = self
            .client
            .0
            .0
            .full_request_before_write(
                self.client.full()?,
                FullCommand::ShutdownIfIdle {},
                IO_TIMEOUT,
                Some(pid),
                || {
                    // This is a nonblocking wait check on the actual Child.
                    // A disappeared or uncollectable child grants no send.
                    match child.try_wait() {
                        Ok(None) => Ok(()),
                        _ => Err(EnforcerError::Unavailable),
                    }
                },
            )
            .await?;
        match reply {
            FullReply::Acknowledged {} => {}
            FullReply::Error { code } => return Err(code),
            _ => return Err(EnforcerError::Unauthorized),
        }
        self.shutdown_acknowledged = true;
        let deadline = Instant::now() + IO_TIMEOUT;
        while !self.reap_acknowledged_exit()? {
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(POLL).await;
        }
        Ok(true)
    }

    pub(crate) fn reap_acknowledged_exit(&mut self) -> EnforcerResult<bool> {
        if !self.shutdown_acknowledged {
            return Ok(false);
        }
        let Some(child) = self.process.0.as_mut() else {
            return Ok(true);
        };
        let Some(status) = child.try_wait().map_err(|_| EnforcerError::Unavailable)? else {
            return Ok(false);
        };
        if !status.success() {
            return Err(EnforcerError::Unhealthy);
        }
        self.process.0 = None;
        Ok(true)
    }
}

pub(crate) struct UnusedPrepareRevoked {
    registration: RegistrationV1,
    ledger: LedgerIdentityV1,
    epoch: Uuid,
    generation: u64,
    retirement_id: Uuid,
    actor_nonce: Uuid,
    revocation_nonce: Uuid,
}

pub(crate) struct ClaimReservation {
    local: Arc<LocalSlot>,
    arm_id: String,
    registration_hash: String,
    epoch: Uuid,
    generation: u64,
}
impl UnusedPrepareRevoked {
    pub(crate) fn registration(&self) -> &RegistrationV1 {
        &self.registration
    }
    pub(crate) fn ledger(&self) -> &LedgerIdentityV1 {
        &self.ledger
    }
    pub(crate) fn epoch(&self) -> Uuid {
        self.epoch
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub(crate) fn retirement_id(&self) -> Uuid {
        self.retirement_id
    }
    pub(crate) fn actor_nonce(&self) -> Uuid {
        self.actor_nonce
    }
    pub(crate) fn revocation_nonce(&self) -> Uuid {
        self.revocation_nonce
    }
}

pub(crate) struct VerifiedRetirementReceipt {
    identity: RetirementIdentityV1,
    receipt: RetiredReceiptV1,
    head: LedgerHeadV1,
    accepted: ClosingAcceptanceV1,
    // Opaque process proof, never serialized: only the strict Final check and
    // actual local-slot removal below can seal the same retained owner here.
    local_completion: Option<Arc<crate::broker::service::preclaim::RetirementSubmission>>,
}
impl VerifiedRetirementReceipt {
    pub(crate) fn completes_retained_submission(
        &self,
        owner: &Arc<crate::broker::service::preclaim::RetirementSubmission>,
    ) -> bool {
        self.local_completion
            .as_ref()
            .is_some_and(|completed| Arc::ptr_eq(completed, owner))
    }
    pub(crate) fn identity(&self) -> &RetirementIdentityV1 {
        &self.identity
    }
    pub(crate) fn receipt(&self) -> &RetiredReceiptV1 {
        &self.receipt
    }
    pub(crate) fn head(&self) -> &LedgerHeadV1 {
        &self.head
    }
    pub(crate) fn accepted(&self) -> &ClosingAcceptanceV1 {
        &self.accepted
    }
    // Closed enforcer client decomposition; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn into_parts(
        self,
    ) -> (
        RetirementIdentityV1,
        RetiredReceiptV1,
        LedgerHeadV1,
        ClosingAcceptanceV1,
    ) {
        (self.identity, self.receipt, self.head, self.accepted)
    }
}

fn full_data<T>(result: crate::broker::journal::JournalResult<T>) -> EnforcerResult<T> {
    result.map_err(|_| EnforcerError::InvalidInput)
}

impl CancelGeneration {
    /// Linearize the final retirement guard and submission with cancellation,
    /// using the same gate as invalidate/advance_to. No IO runs under this gate.
    pub(crate) fn with_current_retirement<T>(
        &self,
        expected: u64,
        action: impl FnOnce() -> EnforcerResult<T>,
    ) -> EnforcerResult<T> {
        self.with_current_generation(expected, action)?
    }
}

async fn write_request_in_generation(
    stream: &mut UnixStream,
    request: &Request,
    generation: Option<(&CancelGeneration, u64)>,
) -> Result<(), LegacyNamedProspectiveFailure> {
    use crate::broker::local_ipc::{IpcError, encode_frame};
    use LegacyNamedProspectiveFailure::{BeforeSend, OutcomeUnknown};
    use std::io::ErrorKind;
    use tokio::time::{Instant, timeout_at};

    let Some((cancel, expected)) = generation else {
        return write_frame(stream, request, MAX_FRAME, IO_TIMEOUT)
            .await
            .map_err(|error| OutcomeUnknown(error.into()));
    };
    let frame =
        encode_frame(request, MAX_FRAME, IO_TIMEOUT).map_err(|error| BeforeSend(error.into()))?;
    let deadline = Instant::now() + IO_TIMEOUT;
    let mut written = 0;
    while written < frame.len() {
        let write_error = move |error: IpcError| {
            if written == 0 {
                BeforeSend(error.into())
            } else {
                OutcomeUnknown(error.into())
            }
        };
        timeout_at(deadline, stream.writable())
            .await
            .map_err(|_| write_error(IpcError::Timeout))?
            .map_err(|_| write_error(IpcError::Unavailable))?;
        let write = || {
            if Instant::now() >= deadline {
                return Err(IpcError::Timeout);
            }
            match stream.try_write(&frame[written..]) {
                Ok(0) => Err(IpcError::Unavailable),
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(None),
                Err(_) => Err(IpcError::Unavailable),
            }
        };
        // Readiness is a hint. Order each attempt to write the first byte
        // with STOP. Once any bytes are accepted, finish this same frame.
        let accepted = if written == 0 {
            cancel
                .with_current_generation(expected, write)
                .map_err(BeforeSend)?
        } else {
            write()
        };
        if let Some(bytes) = accepted.map_err(write_error)? {
            written += bytes;
        }
    }
    Ok(())
}

#[cfg(test)]
#[tokio::test]
async fn stale_named_generation_writes_no_business_frame() -> anyhow::Result<()> {
    let (mut writer, reader) = UnixStream::pair()?;
    let generation = CancelGeneration::new();
    let expected = generation.current();
    generation.invalidate();
    assert_eq!(
        write_request_in_generation(
            &mut writer,
            &Request::Status {},
            Some((&generation, expected))
        )
        .await,
        Err(LegacyNamedProspectiveFailure::BeforeSend(
            EnforcerError::NotReady
        )),
    );
    assert_eq!(
        reader.try_read(&mut [0]).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    Ok(())
}

#[cfg(test)]
#[tokio::test]
async fn named_request_invalidated_while_waiting_to_write_sends_no_frame() -> anyhow::Result<()> {
    use std::io::ErrorKind;
    use tokio::io::AsyncReadExt;

    let (mut writer, mut reader) = UnixStream::pair()?;
    tokio::time::timeout(IO_TIMEOUT, writer.writable()).await??;
    let filler = [b'x'; 4096];
    let mut filled = 0;
    let mut saturated = false;
    for _ in 0..1024 {
        match writer.try_write(&filler) {
            Ok(0) => anyhow::bail!("fixture socket accepted no filler bytes"),
            Ok(written) => filled += written,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                saturated = true;
                break;
            }
            Err(error) => return Err(error.into()),
        }
    }
    anyhow::ensure!(saturated && filled > 0, "fixture socket did not fill");

    let cancel = CancelGeneration::new();
    let expected = cancel.current();
    let request = Request::Status {};
    let mut write = Box::pin(write_request_in_generation(
        &mut writer,
        &request,
        Some((&cancel, expected)),
    ));
    assert!(futures_util::poll!(write.as_mut()).is_pending());
    cancel.invalidate();
    let mut drained = vec![0; filled];
    tokio::time::timeout(IO_TIMEOUT, reader.read_exact(&mut drained)).await??;
    assert!(drained.iter().all(|&byte| byte == filler[0]));
    assert_eq!(
        tokio::time::timeout(IO_TIMEOUT, write).await?,
        Err(LegacyNamedProspectiveFailure::BeforeSend(
            EnforcerError::NotReady
        )),
    );
    drop(writer);
    assert_eq!(
        tokio::time::timeout(IO_TIMEOUT, reader.read(&mut [0])).await??,
        0,
    );
    Ok(())
}

#[cfg(test)]
#[tokio::test]
async fn guarded_named_request_keeps_the_existing_frame_bytes() -> anyhow::Result<()> {
    use tokio::io::AsyncReadExt;

    let cancel = CancelGeneration::new();
    let body = br#"{"kind":"status"}"#;
    let mut expected = u32::try_from(body.len())?.to_be_bytes().to_vec();
    expected.extend_from_slice(body);
    for generation in [None, Some((&cancel, cancel.current()))] {
        let (mut writer, mut reader) = UnixStream::pair()?;
        write_request_in_generation(&mut writer, &Request::Status {}, generation)
            .await
            .map_err(LegacyNamedProspectiveFailure::code)?;
        drop(writer);
        let mut frame = Vec::new();
        tokio::time::timeout(IO_TIMEOUT, reader.read_to_end(&mut frame)).await??;
        assert_eq!(frame, expected);
    }
    Ok(())
}

impl EnforcerFullClientV3 {
    #[cfg(test)]
    pub(crate) fn lose_next_bind_reply_after_native_for_test(&self) -> EnforcerResult<()> {
        self.full()?
            .lose_next_bind_reply_after_native
            .store(true, Ordering::SeqCst);
        Ok(())
    }
    pub(crate) async fn connect_existing(root: &Directory, max_slots: u32) -> EnforcerResult<Self> {
        if !(1..=MAX_SLOTS).contains(&max_slots) {
            return Err(EnforcerError::InvalidInput);
        }
        root.revalidate()
            .map_err(|_| EnforcerError::UnsafeFilesystem)?;
        let endpoint = load_endpoint(root)?.ok_or(EnforcerError::Unavailable)?;
        let endpoint_root = root
            .persistent_child("endpoints", false)
            .map_err(|_| EnforcerError::UnsafeFilesystem)?
            .ok_or(EnforcerError::UnsafeFilesystem)?;
        let secret = capability(root, false)?;
        let binding = crate::broker::discovery::recovery_root_binding(root, &endpoint)?;
        let mut stream = endpoint.connect_checked(&endpoint_root).await?;
        let (session, ledger) =
            recovery_v3::connect_full(&mut stream, &secret, endpoint.epoch(), &binding, max_slots)
                .await?;
        let mut watch = recovery_v3::FullWatchReader::start(&mut stream, session).await?;
        let FullReply::Status { value } = watch.read(&mut stream).await? else {
            return Err(EnforcerError::Unhealthy);
        };
        if value.epoch != endpoint.epoch() || value.max_slots != max_slots || !value.healthy {
            return Err(EnforcerError::Unauthorized);
        }
        // SAFETY: duplicate only this live authenticated watch socket. The
        // nonblocking final spawn guard polls its retained kernel object.
        let fd = unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if fd < 0 {
            return Err(EnforcerError::Unavailable);
        }
        // SAFETY: successful F_DUPFD_CLOEXEC returned a fresh owned descriptor.
        let liveness = unsafe { OwnedFd::from_raw_fd(fd) };
        let client = EnforcerClient(Arc::new(Client {
            legacy: None,
            authority: SlotAuthorityV1::Full3(Arc::new(FullTransport {
                ledger,
                binding,
                registrations: Mutex::new(BTreeMap::new()),
                prospective: Mutex::new(BTreeMap::new()),
                prearm_write_unknown: AtomicBool::new(false),
                #[cfg(test)]
                lose_query_reply_after_successes: Mutex::new(None),
                #[cfg(test)]
                lose_next_bind_reply_after_native: AtomicBool::new(false),
            })),
            root: root.clone(),
            executable: PathBuf::new(),
            max_slots,
            endpoint_root,
            endpoint,
            capability: secret,
            liveness,
            #[cfg(test)]
            owned_peer_pid_for_test: None,
            replacement: Mutex::new(None),
            // Full3 admission is arbitrated against the server's shared
            // registry per Arm; another job's known STOP is not a global fuse.
            unknown_slots: AtomicU64::new(0),
            lost_unknown: AtomicBool::new(false),
            accounted: Mutex::new(BTreeSet::new()),
            auto_recovery: AtomicBool::new(false),
            recovery_gate: Mutex::new(()),
            failed: AtomicBool::new(false),
            health: Mutex::new(Some((value.progress, Instant::now()))),
            slots: Mutex::new(BTreeMap::new()),
        }));
        let weak = Arc::downgrade(&client.0);
        tokio::spawn(async move {
            loop {
                let Some(client) = weak.upgrade() else {
                    return;
                };
                let result = match watch.read(&mut stream).await {
                    Ok(FullReply::Status { value }) => {
                        record_health_and_slots(&client, value).await
                    }
                    _ => Err(EnforcerError::Unhealthy),
                };
                if result.is_err() {
                    client.fail();
                    return;
                }
                drop(client);
            }
        });
        Ok(Self(client))
    }

    pub(crate) async fn spawn_fresh_with_budget(
        parent: &Directory,
        name: &str,
        executable: &Path,
        installation_id: Uuid,
        max_slots: u32,
        max_recovery_stop_owners: u32,
    ) -> EnforcerResult<(Self, FreshEnforcerOwner)> {
        if name.is_empty()
            || name.len() > 100
            || name == "."
            || name == ".."
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            || !executable.is_absolute()
            || installation_id.is_nil()
            || !(1..=MAX_SLOTS).contains(&max_slots)
            || max_recovery_stop_owners > MAX_SLOTS
        {
            return Err(EnforcerError::InvalidInput);
        }
        if parent
            .persistent_child(name, false)
            .map_err(|_| EnforcerError::UnsafeFilesystem)?
            .is_some()
        {
            return Err(EnforcerError::Conflict);
        }
        let process = spawn_helper_process(
            parent,
            executable,
            max_slots,
            HelperMode::Full3Fresh {
                name,
                installation_id,
                max_recovery_stop_owners,
            },
        )?;
        let expires = Instant::now() + REGISTER_TIMEOUT;
        loop {
            if let Some(root) = parent
                .persistent_child(name, false)
                .map_err(|_| EnforcerError::UnsafeFilesystem)?
            {
                match Self::connect_existing(&root, max_slots).await {
                    Ok(client) => {
                        let owner = FreshEnforcerOwner {
                            client: client.clone(),
                            process,
                            shutdown_acknowledged: false,
                        };
                        return Ok((client, owner));
                    }
                    Err(EnforcerError::Unavailable | EnforcerError::Unauthorized)
                        if Instant::now() < expires => {}
                    Err(error) => return Err(error),
                }
            }
            if Instant::now() >= expires {
                return Err(EnforcerError::Unavailable);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    pub(crate) async fn register(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
    ) -> EnforcerResult<EnforcedSlot> {
        self.0
            .register_current(registration, generation, false)
            .await
    }
    pub(crate) fn require_new_prearm_identity(&self) -> EnforcerResult<()> {
        if self.full()?.prearm_write_unknown.load(Ordering::SeqCst) {
            return Err(EnforcerError::NotReady);
        }
        Ok(())
    }
    pub(crate) fn fence_unknown_prearm_write(&self) -> EnforcerResult<()> {
        self.full()?
            .prearm_write_unknown
            .store(true, Ordering::SeqCst);
        Ok(())
    }
    /// Called only from the managed runtime's validated Created callback,
    /// before persisting/sending an Arm. A historical pending Arm may only
    /// reconnect to an already installed exact prospective owner.
    pub(crate) async fn reserve_prospective(
        &self,
        target: ProspectiveRegistrationV1,
        generation: CancelGeneration,
        replay_only: bool,
    ) -> EnforcerResult<()> {
        let full = self.full()?;
        let expected = generation.current();
        if expected == u64::MAX {
            return Err(EnforcerError::NotReady);
        }
        let digest = target.digest()?;
        let retained = {
            let mut values = full
                .prospective
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            if let Some(old) = values.get(target.arm_id()) {
                if old.digest != digest || !Arc::ptr_eq(&old.generation.0, &generation.0) {
                    return Err(EnforcerError::Conflict);
                }
                Arc::clone(old)
            } else {
                if values.len() >= self.0.0.max_slots as usize {
                    return Err(EnforcerError::Capacity);
                }
                let retained = Arc::new(LocalProspective {
                    target,
                    digest,
                    generation,
                    reservation: Mutex::new(None),
                    bound: AtomicBool::new(false),
                    original_exchange_nonce: Mutex::new(None),
                });
                values.insert(retained.target.arm_id().to_owned(), Arc::clone(&retained));
                retained
            }
        };
        // Never hold the client map across the native factory/handshake. The
        // server rechecks its shared admission generation after every wait.
        let reply = self
            .0
            .0
            .full_request(
                full,
                FullCommand::ReserveProspective {
                    prospective: Box::new(retained.target.clone()),
                    generation: expected,
                    replay_only,
                },
                REGISTER_TIMEOUT,
            )
            .await?;
        let FullReply::ProspectiveReserved { reservation } = reply else {
            return Err(match reply {
                FullReply::Error { code } => code,
                _ => EnforcerError::Unauthorized,
            });
        };
        reservation.validate_for(
            &retained.target,
            self.epoch(),
            reservation.cancel_generation,
        )?;
        if (!replay_only && expected != reservation.cancel_generation)
            || (replay_only && expected < reservation.cancel_generation)
        {
            return Err(EnforcerError::Conflict);
        }
        let mut previous = retained
            .reservation
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        if let Some(old) = previous.as_ref() {
            if serde_json::to_vec(old).map_err(|_| EnforcerError::InvalidInput)?
                != serde_json::to_vec(&reservation).map_err(|_| EnforcerError::InvalidInput)?
            {
                return Err(EnforcerError::Conflict);
            }
        } else {
            *previous = Some(reservation);
        }
        // Cancellation during native installation is restrictive and retains
        // the installed STOP owner, while forbidding the subsequent Arm RPC.
        if retained.generation.current() != expected {
            return Err(EnforcerError::NotReady);
        }
        Ok(())
    }
    pub(crate) async fn begin_prospective_arm_exchange(
        &self,
        target: &ProspectiveRegistrationV1,
        generation: &CancelGeneration,
    ) -> EnforcerResult<ProspectiveArmExchange> {
        let expected = generation.current();
        if expected == u64::MAX {
            return Err(EnforcerError::NotReady);
        }
        let full = self.full()?;
        let retained = full
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .get(target.arm_id())
            .cloned()
            .ok_or(EnforcerError::NotReady)?;
        if retained.digest != target.digest()?
            || !Arc::ptr_eq(&retained.generation.0, &generation.0)
        {
            return Err(EnforcerError::Conflict);
        }
        let reservation = retained
            .reservation
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .clone()
            .ok_or(EnforcerError::NotReady)?;
        // Clear any previous process grant before sending. If this reply is
        // lost, the native monotone phase cannot issue an Original again.
        *retained
            .original_exchange_nonce
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)? = None;
        let reply = self
            .0
            .0
            .full_request(
                full,
                FullCommand::BeginProspectiveArmExchange {
                    reservation,
                    generation: expected,
                },
                REGISTER_TIMEOUT,
            )
            .await?;
        let FullReply::ProspectiveArmExchange {
            original_exchange_nonce,
        } = reply
        else {
            return Err(match reply {
                FullReply::Error { code } => code,
                _ => EnforcerError::Unauthorized,
            });
        };
        if original_exchange_nonce.is_some_and(|nonce| nonce.is_nil()) {
            return Err(EnforcerError::Unauthorized);
        }
        if generation.current() != expected {
            return Err(EnforcerError::NotReady);
        }
        Ok(ProspectiveArmExchange {
            epoch: self.epoch(),
            arm_id: target.arm_id().into(),
            prospective_hash: retained.digest.clone(),
            generation: expected,
            original_exchange_nonce,
            legacy_named_reservation: None,
        })
    }
    pub(crate) async fn bind_prospective(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
        exchange: ProspectiveArmExchange,
    ) -> EnforcerResult<EnforcedSlot> {
        if exchange.legacy_named_reservation.is_some() {
            return Err(EnforcerError::Unauthorized);
        }
        let observe_only = !exchange.permits_original();
        {
            let values = self
                .full()?
                .prospective
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            let retained = values
                .get(registration.arm_id())
                .ok_or(EnforcerError::NotReady)?;
            retained.target.matches_registration(&registration)?;
            if exchange.epoch != self.epoch()
                || exchange.arm_id != registration.arm_id()
                || exchange.prospective_hash != retained.digest
                || exchange.generation != generation.current()
            {
                return Err(EnforcerError::Conflict);
            }
            if !Arc::ptr_eq(&retained.generation.0, &generation.0)
                || retained
                    .reservation
                    .lock()
                    .map_err(|_| EnforcerError::Unhealthy)?
                    .is_none()
            {
                return Err(EnforcerError::Conflict);
            }
            *retained
                .original_exchange_nonce
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)? = exchange.original_exchange_nonce;
        }
        self.0
            .register_current(registration, generation, observe_only)
            .await
    }
    pub(crate) async fn restore(
        &self,
        _registration: RegistrationV1,
        _generation: CancelGeneration,
    ) -> EnforcerResult<EnforcedSlot> {
        // A loaded Arm without an exact assignment must retain its original
        // metadata STOP route. It cannot be adopted by this Full3 role.
        Err(EnforcerError::Unauthorized)
    }
    pub(crate) fn ledger_identity(&self) -> EnforcerResult<&LedgerIdentityV1> {
        Ok(&self.full()?.ledger)
    }
    pub(crate) fn verify_installation(&self, expected: Uuid) -> EnforcerResult<()> {
        if expected.is_nil() || self.ledger_identity()?.installation_id() != &expected {
            return Err(EnforcerError::Conflict);
        }
        Ok(())
    }
    pub(crate) fn validate_registration_binding(
        &self,
        registration: &RegistrationV1,
        binding: &Full3RegistrationBindingV1,
    ) -> EnforcerResult<()> {
        let digest = registration.digest()?;
        super::execution::validate_assignment(
            self.ledger_identity()?,
            self.epoch(),
            &registration.arm.operation_id,
            &digest,
            binding,
        )
    }
    pub(crate) async fn restore_with_binding(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
        binding: &Full3RegistrationBindingV1,
    ) -> EnforcerResult<EnforcedSlot> {
        self.validate_registration_binding(&registration, binding)?;
        self.0
            .register_current(registration, generation, true)
            .await
    }
    pub(crate) async fn cancel_retained_bound(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
        binding: &Full3RegistrationBindingV1,
    ) -> EnforcerResult<()> {
        self.validate_registration_binding(&registration, binding)?;
        // The retained-cancel machinery dispatches through SlotAuthorityV1::
        // Full3 and this client's disabled legacy recovery. No conversion or
        // fallback is exposed. Partial ledger/old epoch keeps metadata STOP.
        self.0.cancel_retained(registration, generation).await
    }
    pub(crate) async fn status(&self) -> EnforcerResult<EnforcerStatusV1> {
        match self
            .0
            .0
            .full_request(self.full()?, FullCommand::Status {}, IO_TIMEOUT)
            .await?
        {
            FullReply::Status { value }
                if value.epoch == self.epoch()
                    && value.max_slots == self.0.0.max_slots
                    && value.slot_count <= value.max_slots as usize =>
            {
                Ok(value)
            }
            FullReply::Error { code } => Err(code),
            _ => Err(EnforcerError::Unauthorized),
        }
    }
    pub(crate) fn epoch(&self) -> Uuid {
        self.0.0.endpoint.epoch()
    }
    pub(crate) async fn require_admission_ready(&self) -> EnforcerResult<()> {
        self.require_new_prearm_identity()?;
        self.0.0.check_health()?;
        match self
            .0
            .0
            .full_request(self.full()?, FullCommand::Status {}, IO_TIMEOUT)
            .await?
        {
            FullReply::Status { value }
                if value.epoch == self.epoch()
                    && value.healthy
                    && value.max_slots == self.0.0.max_slots =>
            {
                if value.slot_count >= value.max_slots as usize {
                    Err(EnforcerError::Capacity)
                } else {
                    Ok(())
                }
            }
            FullReply::Error { code } => Err(code),
            _ => Err(EnforcerError::Unhealthy),
        }
    }
    #[cfg(test)]
    pub(crate) async fn shutdown_if_idle(&self) -> EnforcerResult<()> {
        match self
            .0
            .0
            .full_request(self.full()?, FullCommand::ShutdownIfIdle {}, IO_TIMEOUT)
            .await?
        {
            FullReply::Acknowledged {} => Ok(()),
            FullReply::Error { code } => Err(code),
            _ => Err(EnforcerError::Unauthorized),
        }
    }

    fn full(&self) -> EnforcerResult<&FullTransport> {
        match &self.0.0.authority {
            SlotAuthorityV1::Full3(full) => Ok(full),
            SlotAuthorityV1::Legacy => Err(EnforcerError::Unauthorized),
        }
    }
    #[cfg(test)]
    pub(crate) async fn retire_unused(
        &self,
        authorized: crate::broker::writer::AuthorizedRetirement,
    ) -> EnforcerResult<VerifiedRetirementReceipt> {
        let (identity, evidence, audit_ack) = full_data(authorized.into_parts())?;
        let expected_evidence = evidence.clone();
        let expected_ack = audit_ack.clone();
        let reply = self
            .0
            .0
            .full_request(
                self.full()?,
                FullCommand::RetireUnused {
                    identity: Box::new(identity.clone()),
                    evidence,
                    audit_ack,
                },
                REGISTER_TIMEOUT,
            )
            .await?;
        let verified = self
            .verify_retirement(&identity, reply)?
            .ok_or(EnforcerError::Conflict)?;
        if full_data(verified.accepted.evidence().digest())?
            != full_data(expected_evidence.digest())?
            || serde_json::to_vec(verified.accepted.audit_ack())
                .map_err(|_| EnforcerError::InvalidInput)?
                != serde_json::to_vec(&expected_ack).map_err(|_| EnforcerError::InvalidInput)?
        {
            return Err(EnforcerError::Unauthorized);
        }
        Ok(verified)
    }

    /// Productive retirement always borrows the original already retained by
    /// Preclaim. A dropped caller or failed connect cannot destroy that owner.
    pub(crate) async fn retire_unused_retained<F>(
        &self,
        owner: &Arc<crate::broker::service::preclaim::RetirementSubmission>,
        before_write: F,
    ) -> EnforcerResult<VerifiedRetirementReceipt>
    where
        F: FnOnce() -> EnforcerResult<()> + Send,
    {
        owner.reserve_attempt()?;
        self.verify_installation(owner.identity().scope().installation_id())?;
        if *owner.identity().original_epoch() != self.epoch() {
            return Err(EnforcerError::Conflict);
        }
        let reply = self
            .0
            .0
            .full_request_before_write(
                self.full()?,
                FullCommand::RetireUnused {
                    identity: Box::new(owner.identity().clone()),
                    evidence: owner.evidence().clone(),
                    audit_ack: owner.audit_ack().clone(),
                },
                REGISTER_TIMEOUT,
                None,
                before_write,
            )
            .await?;
        let verified = self
            .verify_retirement(owner.identity(), reply)?
            .ok_or(EnforcerError::Conflict)?;
        self.verify_retained_retirement(owner, &verified)?;
        Ok(verified)
    }

    fn verify_retained_retirement(
        &self,
        owner: &crate::broker::service::preclaim::RetirementSubmission,
        verified: &VerifiedRetirementReceipt,
    ) -> EnforcerResult<()> {
        if !owner.is_submitted()?
            || full_data(verified.identity.digest())? != full_data(owner.identity().digest())?
            || full_data(verified.accepted.evidence().digest())?
                != full_data(owner.evidence().digest())?
            || serde_json::to_vec(verified.accepted.audit_ack())
                .map_err(|_| EnforcerError::InvalidInput)?
                != serde_json::to_vec(owner.audit_ack()).map_err(|_| EnforcerError::InvalidInput)?
        {
            return Err(EnforcerError::Unauthorized);
        }
        Ok(())
    }

    /// Query never changes the submission phase or regenerates an original.
    /// None/error retains the exact submitted owner and its uncertainty.
    pub(crate) async fn query_retirement_retained(
        &self,
        owner: &Arc<crate::broker::service::preclaim::RetirementSubmission>,
    ) -> EnforcerResult<Option<VerifiedRetirementReceipt>> {
        if !owner.is_submitted()? {
            return Err(EnforcerError::Conflict);
        }
        let verified = self.query_retirement(owner.identity()).await?;
        if let Some(verified) = verified.as_ref() {
            self.verify_retained_retirement(owner, verified)?;
        }
        Ok(verified)
    }
    pub(crate) fn retire_local_slot(
        &self,
        verified: &VerifiedRetirementReceipt,
    ) -> EnforcerResult<()> {
        if verified.identity.original_epoch() != &self.epoch()
            || !full_data(verified.receipt.ledger().matches(&self.full()?.ledger))?
        {
            return Err(EnforcerError::Conflict);
        }
        let _gate = self
            .0
            .0
            .recovery_gate
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        let mut slots = self
            .0
            .0
            .slots
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        if let Some(slot) = slots.get(&verified.identity.arm().operation_id)
            && (!slot.latch.revoked()
                || slot.digest != verified.identity.registration_hash().as_str())
        {
            return Err(EnforcerError::Conflict);
        }
        slots.remove(&verified.identity.arm().operation_id);
        // The verified native retirement receipt includes the original's
        // actual join. This existing semantic exit can also release the
        // matching prospective payload; a status/Drop cannot reach this port.
        self.full()?
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .remove(&verified.identity.arm().operation_id);
        Ok(())
    }

    /// An ordinary checkpoint receipt or Q alone is insufficient to release
    /// an original keeper. Verify this reader-attested Final first, then seal
    /// the actual removal of the present matching LocalSlot without an await.
    pub(crate) fn retire_retained_local_after_final(
        &self,
        owner: &Arc<crate::broker::service::preclaim::RetirementSubmission>,
        verified: &mut VerifiedRetirementReceipt,
        snapshot: &crate::broker::journal::StopJobSnapshotV1,
    ) -> EnforcerResult<()> {
        self.verify_retained_retirement(owner, verified)?;
        if verified.completes_retained_submission(owner) {
            return Ok(());
        }
        if !owner.matches_recovery_origin(snapshot)
            || snapshot.incomplete()
            || snapshot.operation_id() != *owner.identity().job_operation_id()
            || full_data(crate::broker::journal::canonical(snapshot.scope()))?
                != full_data(crate::broker::journal::canonical(owner.identity().scope()))?
        {
            return Err(EnforcerError::Conflict);
        }
        let mut matching = snapshot.retirements().iter().filter(|entry| {
            entry.request().identity().retirement_id() == owner.identity().retirement_id()
        });
        let entry = matching.next().ok_or(EnforcerError::Conflict)?;
        if matching.next().is_some() {
            return Err(EnforcerError::Conflict);
        }
        let pending = entry.request();
        let finalized = entry.finalized().ok_or(EnforcerError::Conflict)?;
        if full_data(pending.identity().digest())? != full_data(owner.identity().digest())?
            || full_data(pending.evidence().digest())? != full_data(owner.evidence().digest())?
            || serde_json::to_vec(pending.audit_ack().ok_or(EnforcerError::Conflict)?)
                .map_err(|_| EnforcerError::InvalidInput)?
                != serde_json::to_vec(owner.audit_ack()).map_err(|_| EnforcerError::InvalidInput)?
            || full_data(finalized.receipt().digest())? != full_data(verified.receipt.digest())?
        {
            return Err(EnforcerError::Conflict);
        }
        let _gate = self
            .0
            .0
            .recovery_gate
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        let mut slots = self
            .0
            .0
            .slots
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        let arm_id = &owner.identity().arm().operation_id;
        let local = slots.get(arm_id).ok_or(EnforcerError::Conflict)?;
        if !local.latch.revoked() || local.digest != owner.identity().registration_hash().as_str() {
            return Err(EnforcerError::Conflict);
        }
        slots.remove(arm_id);
        // Only the verified native Final/join path above can release this
        // client's prospective retention. A cancelled waiter never does.
        self.full()?
            .prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .remove(arm_id);
        verified.local_completion = Some(Arc::clone(owner));
        Ok(())
    }
    /// Process-local retention observation for reconciliation scheduling only.
    /// Presence or absence grants no permission and proves no quiescence.
    pub(crate) fn has_local_slot(&self, arm_id: &str) -> EnforcerResult<bool> {
        if !valid_uuid(arm_id) {
            return Err(EnforcerError::InvalidInput);
        }
        Ok(self
            .0
            .0
            .slots
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .contains_key(arm_id))
    }
    #[cfg(test)]
    pub(crate) fn lose_query_reply_after_successes_for_test(&self, n: usize) -> EnforcerResult<()> {
        *self
            .full()?
            .lose_query_reply_after_successes
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)? = Some(n);
        Ok(())
    }
    pub(crate) async fn query_retirement(
        &self,
        identity: &RetirementIdentityV1,
    ) -> EnforcerResult<Option<VerifiedRetirementReceipt>> {
        let reply = self
            .0
            .0
            .full_request(
                self.full()?,
                FullCommand::QueryRetirement {
                    identity: Box::new(identity.clone()),
                },
                IO_TIMEOUT,
            )
            .await?;
        let verified = self.verify_retirement(identity, reply)?;
        #[cfg(test)]
        if verified.is_some() {
            let lose = {
                let mut remaining = self
                    .full()?
                    .lose_query_reply_after_successes
                    .lock()
                    .map_err(|_| EnforcerError::Unhealthy)?;
                match *remaining {
                    Some(0) => {
                        *remaining = None;
                        true
                    }
                    Some(count) => {
                        *remaining = Some(count - 1);
                        false
                    }
                    None => false,
                }
            };
            if lose {
                // Lose only the actual authenticated and fully verified Q.
                // Never construct a substitute reply, wrapper or permission.
                drop(verified);
                return Err(EnforcerError::Unavailable);
            }
        }
        Ok(verified)
    }
    fn verify_retirement(
        &self,
        identity: &RetirementIdentityV1,
        reply: FullReply,
    ) -> EnforcerResult<Option<VerifiedRetirementReceipt>> {
        // No RecoveredPriorEpoch producer exists. Complete storage and a new
        // MAC session cannot exclude an intervening STOP across guardian epochs.
        if *identity.original_epoch() != self.epoch() {
            return Err(EnforcerError::Conflict);
        }
        let FullReply::Retirement {
            receipt,
            head,
            accepted,
        } = reply
        else {
            return Err(match reply {
                FullReply::Error { code } => code,
                _ => EnforcerError::Unauthorized,
            });
        };
        if !full_data(head.ledger().matches(&self.full()?.ledger))? {
            return Err(EnforcerError::Unauthorized);
        }
        let Some(receipt) = receipt else {
            return Ok(None);
        };
        let accepted = accepted.ok_or(EnforcerError::Unauthorized)?;
        let identity_hash = full_data(identity.digest())?;
        let closing_hash = full_data(accepted.digest())?;
        let tombstone = full_data(RetiredTombstoneV1::new(
            self.full()?.ledger.clone(),
            identity_hash.clone(),
            closing_hash.clone(),
        ))?;
        if !full_data(receipt.ledger().matches(&self.full()?.ledger))?
            || receipt.retirement_id() != identity.retirement_id()
            || receipt.identity_hash() != &identity_hash
            || full_data(accepted.identity().digest())? != identity_hash
            || receipt.finalization().closing_hash() != &closing_hash
            || receipt.finalization().tombstone_hash() != &full_data(tombstone.digest())?
            || receipt.finalization().joined().original_epoch() != identity.original_epoch()
            || receipt.finalization().joined().original_actor_nonce()
                != accepted.evidence().original_actor_nonce()
        {
            return Err(EnforcerError::Unauthorized);
        }
        Ok(Some(VerifiedRetirementReceipt {
            identity: identity.clone(),
            receipt: *receipt,
            head,
            accepted: *accepted,
            local_completion: None,
        }))
    }
}

impl EnforcedSlot {
    pub(crate) fn full3_registration_binding(&self) -> EnforcerResult<Full3RegistrationBindingV1> {
        let SlotAuthorityV1::Full3(full) = &self.client.0.authority else {
            return Err(EnforcerError::Unauthorized);
        };
        full.registrations
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .get(&self.local.registration.arm.operation_id)
            .map(|registration| registration.binding.clone())
            .ok_or(EnforcerError::NotReady)
    }
    pub(crate) async fn revoke_unused(
        &self,
        retirement_id: Uuid,
    ) -> EnforcerResult<UnusedPrepareRevoked> {
        let generation = self.local.generation.current();
        self.check_ready_for_claim(generation)?;
        let SlotAuthorityV1::Full3(full) = &self.client.0.authority else {
            return Err(EnforcerError::Unauthorized);
        };
        let (actor_nonce, original_registration_nonce) = full
            .registrations
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .get(&self.local.registration.arm.operation_id)
            .map(|registration| {
                registration
                    .original
                    .as_ref()
                    .map(|original| (registration.actor_nonce, original.nonce))
            })
            .ok_or(EnforcerError::NotReady)?
            .ok_or(EnforcerError::Unauthorized)?;
        // Revoke every old slot handle/guard before sending any RPC. This
        // never invalidates the shared job generation or another slot.
        self.local.latch.revoke()?;
        self.local.stop.send_replace(true);
        let response = self
            .client
            .0
            .full_request(
                full,
                FullCommand::RevokeUnused {
                    arm_id: self.local.registration.arm.operation_id.clone(),
                    registration_hash: full_data(RetirementDigestV1::parse(&self.local.digest))?,
                    generation,
                    actor_nonce,
                    original_registration_nonce,
                    retirement_id,
                },
                IO_TIMEOUT,
            )
            .await?;
        match response {
            FullReply::Revoked { revocation_nonce } => Ok(UnusedPrepareRevoked {
                registration: self.local.registration.clone(),
                ledger: full.ledger.clone(),
                epoch: self.client.0.endpoint.epoch(),
                generation,
                retirement_id,
                actor_nonce,
                revocation_nonce,
            }),
            FullReply::Error { code } => Err(code),
            _ => Err(EnforcerError::Unauthorized),
        }
    }
}

impl Client {
    async fn full_request(
        &self,
        full: &FullTransport,
        command: FullCommand,
        timeout: Duration,
    ) -> EnforcerResult<FullReply> {
        self.full_request_before_write(full, command, timeout, None, || Ok(()))
            .await
    }

    async fn full_request_before_write<F>(
        &self,
        full: &FullTransport,
        command: FullCommand,
        timeout: Duration,
        expected_peer_pid: Option<u32>,
        before_write: F,
    ) -> EnforcerResult<FullReply>
    where
        F: FnOnce() -> EnforcerResult<()> + Send,
    {
        let mut stream = self.endpoint.connect_checked(&self.endpoint_root).await?;
        if let Some(pid) = expected_peer_pid
            && peer_pid(&stream) != Some(pid)
        {
            return Err(EnforcerError::Unauthorized);
        }
        let binding = crate::broker::discovery::recovery_root_binding(&self.root, &self.endpoint)?;
        if binding != full.binding {
            return Err(EnforcerError::Unauthorized);
        }
        let (session, ledger) = recovery_v3::connect_full(
            &mut stream,
            &self.capability,
            self.endpoint.epoch(),
            &binding,
            self.max_slots,
        )
        .await?;
        if !full_data(ledger.matches(&full.ledger))? {
            return Err(EnforcerError::Unauthorized);
        }
        recovery_v3::request_full_before_write(
            &mut stream,
            &session,
            command,
            timeout,
            before_write,
        )
        .await
    }

    async fn request_full_legacy(
        &self,
        full: &FullTransport,
        request: Request,
        timeout: Duration,
    ) -> EnforcerResult<Response> {
        let command = match request {
            Request::ProspectiveCapabilities {}
            | Request::NegotiateReservationMotif {}
            | Request::ReserveProspective { .. }
            | Request::ReserveNamedProspective { .. }
            | Request::BeginProspectiveArmExchange { .. }
            | Request::BeginNamedProspectiveArmExchange { .. }
            | Request::BindProspective { .. }
            | Request::BindNamedProspective { .. }
            | Request::CancelProspective { .. }
            | Request::BeginRecovery {} => return Err(EnforcerError::Unauthorized),
            Request::Status {} => FullCommand::Status {},
            Request::Slots { arm_ids } => FullCommand::Slots { arm_ids },
            Request::Register {
                registration,
                generation,
                observe_only,
            } => {
                let reservation = {
                    let values = full
                        .prospective
                        .lock()
                        .map_err(|_| EnforcerError::Unhealthy)?;
                    match values.get(registration.arm_id()) {
                        Some(retained) if !retained.bound.load(Ordering::SeqCst) => {
                            retained.target.matches_registration(&registration)?;
                            let reservation = retained
                                .reservation
                                .lock()
                                .map_err(|_| EnforcerError::Unhealthy)?
                                .clone()
                                .ok_or(EnforcerError::NotReady)?;
                            reservation.validate_for(
                                &retained.target,
                                self.endpoint.epoch(),
                                reservation.cancel_generation,
                            )?;
                            if (!observe_only && generation != reservation.cancel_generation)
                                || (observe_only && generation < reservation.cancel_generation)
                            {
                                return Err(EnforcerError::Conflict);
                            }
                            // Consume before connect/write, not after an ACK.
                            // An unknown Bind must never re-export the same
                            // Original grant through a generic Register retry.
                            let mut original = retained
                                .original_exchange_nonce
                                .lock()
                                .map_err(|_| EnforcerError::Unhealthy)?;
                            let original_exchange_nonce = original.take();
                            let original_exchange_nonce = if observe_only {
                                None
                            } else {
                                original_exchange_nonce
                            };
                            Some((reservation, original_exchange_nonce))
                        }
                        _ => None,
                    }
                };
                match reservation {
                    Some((reservation, original_exchange_nonce)) => FullCommand::BindProspective {
                        reservation,
                        registration,
                        generation,
                        observe_only,
                        original_exchange_nonce,
                    },
                    None => FullCommand::Register {
                        registration,
                        generation,
                        observe_only,
                    },
                }
            }
            Request::Prepare {
                arm_id,
                registration_hash,
                generation,
            } => FullCommand::Prepare {
                arm_id,
                registration_hash: full_data(RetirementDigestV1::parse(&registration_hash))?,
                generation,
            },
            Request::Cancel {
                arm_id,
                registration_hash,
                generation,
            } => FullCommand::Cancel {
                arm_id,
                registration_hash: full_data(RetirementDigestV1::parse(&registration_hash))?,
                generation,
            },
            Request::ShutdownIfIdle {} => FullCommand::ShutdownIfIdle {},
            Request::Watch {} => {
                return Err(EnforcerError::Unauthorized);
            }
        };
        let expected_register = match &command {
            FullCommand::Register {
                registration,
                generation,
                observe_only,
            }
            | FullCommand::BindProspective {
                registration,
                generation,
                observe_only,
                ..
            } => Some((
                registration.arm.operation_id.clone(),
                registration.digest()?,
                *generation,
                *observe_only,
            )),
            _ => None,
        };
        #[cfg(test)]
        let is_bind = matches!(&command, FullCommand::BindProspective { .. });
        let reply = self.full_request(full, command, timeout).await?;
        #[cfg(test)]
        if is_bind
            && matches!(&reply, FullReply::Registered { .. })
            && full
                .lose_next_bind_reply_after_native
                .swap(false, Ordering::SeqCst)
        {
            // Discard only publication of the real authenticated native reply,
            // before creating a local Original or marking this slot bound.
            return Err(EnforcerError::Unavailable);
        }
        Ok(match reply {
            FullReply::Status { value } => Response::Status { value },
            FullReply::Slots { values } => Response::Slots { values },
            FullReply::Prepared { value } => Response::Prepared { value },
            FullReply::Registered {
                value,
                replayed,
                binding,
                actor_nonce,
                original_registration_nonce,
            } => {
                let (arm, digest, generation, observe_only) =
                    expected_register.ok_or(EnforcerError::Conflict)?;
                if binding.arm_id().to_string() != arm
                    || binding.registration_hash().as_str() != digest
                    || binding.original_epoch() != self.endpoint.epoch()
                    || !full_data(binding.ledger().matches(&full.ledger))?
                    || value.arm_id != arm
                    || value.registration_hash != digest
                    || (replayed && original_registration_nonce.is_some())
                    || (!replayed
                        && !observe_only
                        && (original_registration_nonce.is_none()
                            || value.phase != SlotPhaseV1::Ready
                            || value.prepared_once
                            || value.generation != generation))
                    || (observe_only && original_registration_nonce.is_some())
                {
                    return Err(EnforcerError::Unauthorized);
                }
                let mut retained = full
                    .registrations
                    .lock()
                    .map_err(|_| EnforcerError::Unhealthy)?;
                if let Some(old) = retained.get(&arm) {
                    if old.binding.registration_hash() != binding.registration_hash()
                        || old.actor_nonce != actor_nonce
                        || !replayed
                    {
                        return Err(EnforcerError::Conflict);
                    }
                } else {
                    retained.insert(
                        arm,
                        RetainedFullRegistration {
                            binding,
                            actor_nonce,
                            original: original_registration_nonce
                                .map(|nonce| OriginalRegistration { nonce }),
                        },
                    );
                }
                if let Some(prospective) = full
                    .prospective
                    .lock()
                    .map_err(|_| EnforcerError::Unhealthy)?
                    .get(&value.arm_id)
                {
                    prospective.bound.store(true, Ordering::SeqCst);
                }
                Response::Registered { value, replayed }
            }
            FullReply::Acknowledged {} => Response::Acknowledged {},
            FullReply::Error { code } => Response::Error { code },
            _ => return Err(EnforcerError::Unauthorized),
        })
    }
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
    latch: retirement::SlotLatch,
}

impl EnforcerClient {
    /// Executor fixtures whose path must complete without a guardian. No
    /// process, transport, monitor or recovery authority is available.
    #[cfg(test)]
    pub(crate) fn unavailable_for_test(root: &Directory) -> anyhow::Result<Self> {
        Ok(Self(Arc::new(Client {
            authority: SlotAuthorityV1::Legacy,
            legacy: None,
            root: root.clone(),
            executable: PathBuf::new(),
            max_slots: 0,
            endpoint_root: root.clone(),
            endpoint: EndpointRecordV1::unavailable_for_test(),
            capability: [0; 32],
            liveness: root.try_clone_file()?.into(),
            owned_peer_pid_for_test: None,
            replacement: Mutex::new(None),
            unknown_slots: AtomicU64::new(0),
            lost_unknown: AtomicBool::new(false),
            accounted: Mutex::new(BTreeSet::new()),
            auto_recovery: AtomicBool::new(false),
            recovery_gate: Mutex::new(()),
            failed: AtomicBool::new(true),
            health: Mutex::new(None),
            slots: Mutex::new(BTreeMap::new()),
        })))
    }

    /// Reconnect/recover an existing Legacy namespace with W=0. Missing auth
    /// or lock is refused. Genuine fresh creation uses the parent/name factory
    /// selected by the broker, never adoption of an existing empty directory.
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
        Self::connect_existing_legacy_mode(root, executable, max_slots, 0, auto_recovery).await
    }

    async fn connect(
        root: &Directory,
        executable: &Path,
        endpoint: EndpointRecordV1,
        secret: [u8; 32],
        max_slots: u32,
        max_recovery_stop_owners: u32,
        auto_recovery: bool,
    ) -> Result<Self, ConnectFailure> {
        let (client, stream) = Self::connect_legacy_watch(
            root,
            executable,
            endpoint,
            secret,
            max_slots,
            max_recovery_stop_owners,
            LegacyWatchOptions {
                auto_recovery,
                #[cfg(test)]
                owned_peer_pid: None,
            },
        )
        .await?;
        tokio::spawn(monitor(Arc::downgrade(&client.0), stream));
        Ok(client)
    }

    /// Authenticate only the already-spawned fixture child. Keep the live
    /// watch descriptor for the health check, with no reader task or recovery.
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    pub(crate) async fn connect_owned_legacy_peer_for_test(
        root: &Directory,
        child_pid: u32,
        max_slots: u32,
    ) -> EnforcerResult<Self> {
        if child_pid == 0 || !(1..=MAX_SLOTS).contains(&max_slots) {
            return Err(EnforcerError::InvalidInput);
        }
        let endpoint = load_endpoint(root)?.ok_or(EnforcerError::Unavailable)?;
        let secret = capability(root, false)?;
        let (client, _watch) = Self::connect_legacy_watch(
            root,
            Path::new(""),
            endpoint,
            secret,
            max_slots,
            0,
            LegacyWatchOptions {
                auto_recovery: false,
                owned_peer_pid: Some(child_pid),
            },
        )
        .await
        .map_err(|error| match error {
            ConnectFailure::Absent => EnforcerError::Unavailable,
            ConnectFailure::Other(error) => error,
        })?;
        Ok(client)
    }

    async fn connect_legacy_watch(
        root: &Directory,
        executable: &Path,
        endpoint: EndpointRecordV1,
        secret: [u8; 32],
        max_slots: u32,
        max_recovery_stop_owners: u32,
        options: LegacyWatchOptions,
    ) -> Result<(Self, UnixStream), ConnectFailure> {
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
        #[cfg(test)]
        if options
            .owned_peer_pid
            .is_some_and(|expected| peer_pid(&stream) != Some(expected))
        {
            return Err(ConnectFailure::Other(EnforcerError::Unauthorized));
        }
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
            legacy: Some(Arc::new(LegacyTransport::new(max_recovery_stop_owners))),
            authority: SlotAuthorityV1::Legacy,
            root: root.clone(),
            executable: executable.into(),
            max_slots,
            endpoint_root,
            endpoint,
            capability: secret,
            liveness,
            #[cfg(test)]
            owned_peer_pid_for_test: options.owned_peer_pid,
            replacement: Mutex::new(None),
            unknown_slots: AtomicU64::new(0),
            lost_unknown: AtomicBool::new(false),
            accounted: Mutex::new(BTreeSet::new()),
            auto_recovery: AtomicBool::new(options.auto_recovery),
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
        Ok((client, stream))
    }

    pub async fn register(
        &self,
        registration: RegistrationV1,
        generation: CancelGeneration,
    ) -> EnforcerResult<EnforcedSlot> {
        let active = self.active_with_historical_registration(Some(registration.arm_id()))?;
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
        let active = self.active_with_historical_registration(Some(registration.arm_id()))?;
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
            self.0
                .require_historical_legacy_registration(registration.arm_id())?;
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
                let limit = if observe_only && self.0.legacy.is_some() {
                    MAX_LEGACY_RETAINED_STOPS
                } else {
                    self.0.max_slots as usize
                };
                if slots.len() >= limit {
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
                    latch: retirement::SlotLatch::default(),
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
        let observe_only = self.0.legacy_observe_only(&registration, observe_only)?;
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
        if self.0.legacy.is_some() {
            self.0.legacy_bound(local.registration.arm_id())?;
        }
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
                current.0.require_historical_legacy_registration(arm_id)?;
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
        if active.0.legacy.is_some() {
            active.legacy_require_new_prearm_identity()?;
            let value = active.legacy_capabilities().await?;
            if value.recovery_generation.is_some()
                || value.recovery_incomplete
                || value.owned_active != Some(value.known_stop_slots)
            {
                return Err(EnforcerError::NotReady);
            }
        }
        #[cfg(test)]
        if active.0.owned_peer_pid_for_test.is_some() {
            // This fixture has no monitor. Sample the authenticated owned
            // child on demand before checking the normal health window.
            record_health_and_slots(&active.0, active.status().await?).await?;
        }
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
            || value.slot_count != active.0.legacy_known_slots(&slots)?
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
        self.active_with_historical_registration(None)
    }

    fn active_with_historical_registration(&self, arm_id: Option<&str>) -> EnforcerResult<Self> {
        let mut active = self.clone();
        // The same bound as append-only discovery generations.
        for _ in 0..128 {
            if let Some(arm_id) = arm_id {
                active.0.require_historical_legacy_registration(arm_id)?;
            }
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
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
pub(crate) use retained_cancel_tests::released_connect_fixture;

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

struct LegacyWatchOptions {
    auto_recovery: bool,
    #[cfg(test)]
    owned_peer_pid: Option<u32>,
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
        self.request_in_generation(request, timeout, None)
            .await
            .map_err(LegacyNamedProspectiveFailure::code)
    }

    async fn request_in_generation(
        &self,
        request: Request,
        timeout: Duration,
        generation: Option<(&CancelGeneration, u64)>,
    ) -> Result<Response, LegacyNamedProspectiveFailure> {
        use LegacyNamedProspectiveFailure::{BeforeSend, OutcomeUnknown};

        if let SlotAuthorityV1::Full3(full) = &self.authority {
            if generation.is_some() {
                return Err(BeforeSend(EnforcerError::Unauthorized));
            }
            return self
                .request_full_legacy(full, request, timeout)
                .await
                .map_err(OutcomeUnknown);
        }
        let request = self.adapt_legacy_request(request).map_err(BeforeSend)?;
        #[cfg(test)]
        let is_bind = matches!(
            &request,
            Request::BindProspective { .. } | Request::BindNamedProspective { .. }
        );
        #[cfg(test)]
        let is_begin = matches!(
            &request,
            Request::BeginProspectiveArmExchange { .. }
                | Request::BeginNamedProspectiveArmExchange { .. }
        );
        #[cfg(test)]
        if is_bind
            && self.legacy.as_ref().is_some_and(|legacy| {
                legacy
                    .fail_next_bind_before_connect
                    .swap(false, Ordering::SeqCst)
            })
        {
            return Err(BeforeSend(EnforcerError::Unavailable));
        }
        let mut stream = self
            .endpoint
            .connect_checked(&self.endpoint_root)
            .await
            .map_err(|error| BeforeSend(error.into()))?;
        #[cfg(test)]
        if self
            .owned_peer_pid_for_test
            .is_some_and(|expected| peer_pid(&stream) != Some(expected))
        {
            return Err(BeforeSend(EnforcerError::Unauthorized));
        }
        self.authenticate(&mut stream).await.map_err(BeforeSend)?;
        write_request_in_generation(&mut stream, &request, generation).await?;
        let response = read_frame(&mut stream, MAX_FRAME, timeout)
            .await
            .map_err(|error| OutcomeUnknown(error.into()))?;
        #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
        retained_cancel_tests::hold_register_reply(&request, &response).await;
        #[cfg(test)]
        if is_begin
            && matches!(&response, Response::ProspectiveArmExchange { .. })
            && self
                .legacy
                .as_ref()
                .is_some_and(|legacy| legacy.lose_next_begin_reply.swap(false, Ordering::SeqCst))
        {
            return Err(OutcomeUnknown(EnforcerError::Unavailable));
        }
        #[cfg(test)]
        if is_bind
            && matches!(&response, Response::Registered { .. })
            && self
                .legacy
                .as_ref()
                .is_some_and(|legacy| legacy.lose_next_bind_reply.swap(false, Ordering::SeqCst))
        {
            return Err(OutcomeUnknown(EnforcerError::Unavailable));
        }
        Ok(response)
    }
    /// Negotiate motif support on one authenticated connection, before any
    /// reservation write. `Ok(false)` means the peer closed the unknown
    /// negotiation without a response, exactly as the historical decoder does;
    /// a decoded but unexpected reply is a protocol failure, not a fallback.
    async fn negotiate_reservation_motif(&self, stream: &mut UnixStream) -> EnforcerResult<bool> {
        if write_frame(
            stream,
            &Request::NegotiateReservationMotif {},
            MAX_FRAME,
            IO_TIMEOUT,
        )
        .await
        .is_err()
        {
            return Ok(false);
        }
        match read_frame::<Response>(stream, MAX_FRAME, IO_TIMEOUT).await {
            Ok(Response::ReservationMotifSupported { epoch }) if epoch == self.endpoint.epoch() => {
                Ok(true)
            }
            Ok(_) => Err(EnforcerError::Unavailable),
            Err(_) => Ok(false),
        }
    }
    /// Transport of one Legacy fresh reservation attempt. The negotiation and
    /// the single reservation write share one authenticated connection and
    /// incarnation. Against a peer that refused the negotiation, the
    /// reservation is written once, unnegotiated, on a fresh connection. A
    /// lost, unknown or incomplete reservation response is never resent.
    async fn request_reservation(
        &self,
        request: Request,
        timeout: Duration,
        generation: Option<(&CancelGeneration, u64)>,
    ) -> Result<(Response, bool), LegacyNamedProspectiveFailure> {
        use LegacyNamedProspectiveFailure::{BeforeSend, OutcomeUnknown};

        if let SlotAuthorityV1::Full3(full) = &self.authority {
            if generation.is_some() {
                return Err(BeforeSend(EnforcerError::Unauthorized));
            }
            return self
                .request_full_legacy(full, request, timeout)
                .await
                .map_err(OutcomeUnknown)
                .map(|response| (response, false));
        }
        let mut stream = self
            .endpoint
            .connect_checked(&self.endpoint_root)
            .await
            .map_err(|error| BeforeSend(error.into()))?;
        #[cfg(test)]
        if self
            .owned_peer_pid_for_test
            .is_some_and(|expected| peer_pid(&stream) != Some(expected))
        {
            return Err(BeforeSend(EnforcerError::Unauthorized));
        }
        self.authenticate(&mut stream).await.map_err(BeforeSend)?;
        let negotiated = self
            .negotiate_reservation_motif(&mut stream)
            .await
            .map_err(BeforeSend)?;
        if !negotiated {
            stream = self
                .endpoint
                .connect_checked(&self.endpoint_root)
                .await
                .map_err(|error| BeforeSend(error.into()))?;
            #[cfg(test)]
            if self
                .owned_peer_pid_for_test
                .is_some_and(|expected| peer_pid(&stream) != Some(expected))
            {
                return Err(BeforeSend(EnforcerError::Unauthorized));
            }
            self.authenticate(&mut stream).await.map_err(BeforeSend)?;
        }
        write_request_in_generation(&mut stream, &request, generation).await?;
        let response = read_frame(&mut stream, MAX_FRAME, timeout)
            .await
            .map_err(|error| OutcomeUnknown(error.into()))?;
        #[cfg(test)]
        if matches!(
            &response,
            Response::ProspectiveReserved { .. } | Response::ProspectiveRefused { .. }
        ) && self
            .legacy
            .as_ref()
            .is_some_and(|legacy| legacy.lose_next_reserve_reply.swap(false, Ordering::SeqCst))
        {
            return Err(OutcomeUnknown(EnforcerError::Unavailable));
        }
        Ok((response, negotiated))
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
    pub(crate) fn reserve_claim(
        &self,
        expected_generation: u64,
    ) -> EnforcerResult<retirement::ClaimReservation> {
        self.check_ready_for_claim(expected_generation)?;
        self.local.latch.reserve_claim()?;
        Ok(ClaimReservation {
            local: Arc::clone(&self.local),
            arm_id: self.local.registration.arm.operation_id.clone(),
            registration_hash: self.local.digest.clone(),
            epoch: self.client.0.endpoint.epoch(),
            generation: expected_generation,
        })
    }
    /// Consume a reservation for this exact local owner and the particular
    /// Arm about to be included in the caller's durable pending claim.
    pub(crate) fn consume_claim_reservation(
        &self,
        reservation: ClaimReservation,
        pending_arm: &ArmV1,
        expected_generation: u64,
    ) -> EnforcerResult<()> {
        if !Arc::ptr_eq(&reservation.local, &self.local)
            || reservation.arm_id != self.local.registration.arm.operation_id
            || reservation.arm_id != pending_arm.operation_id
            || reservation.registration_hash != self.local.digest
            || reservation.epoch != self.client.0.endpoint.epoch()
            || reservation.generation != expected_generation
            || self.local.generation.current() != expected_generation
            || !self.local.latch.claim_reserved()
            || self
                .local
                .registration
                .arm
                .input_hash()
                .map_err(|_| EnforcerError::InvalidInput)?
                != pending_arm
                    .input_hash()
                    .map_err(|_| EnforcerError::InvalidInput)?
        {
            return Err(EnforcerError::Conflict);
        }
        self.check_ready_for_claim(expected_generation)
    }
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
        if self.local.latch.revoked() {
            return Err(failure(EnforcerFailureReason::StopSignalled, None));
        }
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
        if self.local.latch.revoked() {
            return Err(failure(EnforcerFailureReason::StopSignalled, None));
        }
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
        if value.arm_id != permission.arm_id()
            || value.registration_hash != self.local.digest
            || value.generation != permission.generation()
            || value.phase != SlotPhaseV1::Ready
            || !value.prepared_once
        {
            return Err(failure(
                EnforcerFailureReason::ResponseBindingMismatch,
                Some(&sample),
            ));
        }
        let clock = Arc::new(clock);
        let native_release: Option<Box<dyn NativeBootstrapRelease>> =
            if matches!(&arm.subject, crate::control::ArmSubjectV1::Worker)
                && arm.inspection.network == crate::control::NetworkV1::None
                && arm.inspection.security_preset
                    == crate::control::SecurityPresetV1::FixedSandboxV2
                && self.local.latch.claim_reserved()
                && self.local.acknowledged.load(Ordering::SeqCst)
                && value.phase == SlotPhaseV1::Ready
                && value.prepared_once
                && value.generation == expected_generation
            {
                value
                    .clock
                    .as_ref()
                    .and_then(|observed| WorkerReleaseDeadline::new(arm, &sample, observed))
                    .map(|deadline| {
                        Box::new(WorkerBootstrapRelease {
                            client: self.client.clone(),
                            local: Arc::clone(&self.local),
                            expected_generation,
                            clock: clock.clone(),
                            deadline,
                        }) as Box<dyn NativeBootstrapRelease>
                    })
            } else {
                None
            };
        *self
            .local
            .status
            .lock()
            .map_err(|_| failure(EnforcerFailureReason::LockPoisoned, Some(&sample)))? =
            (value, Instant::now());
        let guard = SpawnGuard {
            client: self.client.clone(),
            local: Arc::clone(&self.local),
            permission,
            clock,
        };
        Ok(PrearmedStartControl::with_owned_spawn_check(
            move |authority, created| guard.check_before_spawn(authority, created),
            self.local.stop.subscribe(),
            registration.authority.docker_launch_profile.clone(),
            native_release,
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
        let active = self
            .client
            .active_with_historical_registration(Some(self.local.registration.arm_id()))?;
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

/// Process-local authorization retained only by the original Prepare result.
struct WorkerBootstrapRelease {
    client: EnforcerClient,
    local: Arc<LocalSlot>,
    expected_generation: u64,
    clock: Arc<dyn HostClock>,
    deadline: WorkerReleaseDeadline,
}

impl NativeBootstrapRelease for WorkerBootstrapRelease {
    fn submit(
        self: Box<Self>,
        authority: &RuntimeAuthority,
        observed: &OwnedContainerSnapshot,
        attempt: &IdV1,
        check_release: Box<dyn FnOnce() -> anyhow::Result<()> + '_>,
        write: Box<dyn FnOnce() -> Result<(), BootstrapStdinSubmitError> + '_>,
    ) -> Result<Box<dyn NativeConnectGuard>, BootstrapStdinSubmitError> {
        use BootstrapStdinSubmitError::Refused;

        let registration = &self.local.registration;
        let arm = &registration.arm;
        let original = &registration.owned;
        let expected = self.expected_generation;
        self.local
            .generation
            .with_current_generation(expected, || {
                if authority != &registration.authority
                    || attempt.as_str() != arm.attempt_id
                    || observed.handle.id() != original.handle.id()
                    || observed.handle.name() != original.handle.name()
                    || observed.handle.session_id() != original.handle.session_id()
                    || observed.handle.operation_id() != original.handle.operation_id()
                    || observed.handle.authority_fingerprint() != arm.authority_binding_hash
                    || observed.handle.fingerprint_version()
                        != arm.configuration_fingerprint_version
                    || observed
                        .observation
                        .validate_identity_allocation(&original.observation)
                        .is_err()
                {
                    return Err(Refused);
                }
                check_release().map_err(|_| Refused)?;

                self.with_live_slot(|_| write()).map_err(|_| Refused)?
            })
            .map_err(|_| Refused)??;
        Ok(self)
    }
}

impl NativeConnectGuard for WorkerBootstrapRelease {
    fn clock(&self) -> &dyn HostClock {
        self.clock.as_ref()
    }

    fn with_current(
        &self,
        poll: &mut dyn FnMut(&HostClockSampleV1),
    ) -> Result<(), NativeConnectRefusal> {
        self.local
            .generation
            .with_current_generation(self.expected_generation, || self.with_live_slot(poll))
            .map_err(|error| match error {
                EnforcerError::NotReady => NativeConnectRefusal::Stopped,
                _ => NativeConnectRefusal::Unavailable,
            })?
    }
}

impl WorkerBootstrapRelease {
    /// Both callers hold the original generation gate. Keep exact slot
    /// membership, the authenticated snapshot and STOP through the effect.
    fn with_live_slot<T>(
        &self,
        effect: impl FnOnce(&HostClockSampleV1) -> T,
    ) -> Result<T, NativeConnectRefusal> {
        use NativeConnectRefusal::{Stopped, Unavailable};

        let arm = &self.local.registration.arm;
        let client = &self.client.0;
        let slots = client.slots.try_lock().map_err(|_| Unavailable)?;
        if !slots
            .get(&arm.operation_id)
            .is_some_and(|slot| Arc::ptr_eq(slot, &self.local))
        {
            return Err(Stopped);
        }
        let state = self.local.status.try_lock().map_err(|_| Unavailable)?;
        let (status, received) = &*state;
        let stop = self.local.stop.borrow();
        if *stop
            || !self.local.latch.claim_reserved()
            || self.local.cancel_durable.load(Ordering::SeqCst)
            || self.local.retained_cancel.load(Ordering::SeqCst)
        {
            return Err(Stopped);
        }
        if !self.local.acknowledged.load(Ordering::SeqCst)
            || status.arm_id != arm.operation_id
            || status.registration_hash != self.local.digest
        {
            return Err(Unavailable);
        }
        if let Some(reason) = spawn_snapshot_failure(status, self.expected_generation, received) {
            return Err(match reason {
                EnforcerFailureReason::GenerationChanged
                | EnforcerFailureReason::SlotPhaseMismatch => Stopped,
                _ => Unavailable,
            });
        }
        let observed_clock = status.clock.as_ref().ok_or(Unavailable)?;
        client.try_check_health().map_err(|_| Unavailable)?;
        let current_clock = self.clock.sample().map_err(|_| Unavailable)?;
        self.deadline
            .check(&arm.clock, observed_clock, &current_clock)?;
        Ok(effect(&current_clock))
    }
}

struct WorkerReleaseDeadline {
    prepared_floor_ns: u128,
    deadline_ns: u128,
}

impl WorkerReleaseDeadline {
    fn new(
        arm: &ArmV1,
        prepare_clock: &HostClockSampleV1,
        observed_clock: &HostClockSampleV1,
    ) -> Option<Self> {
        // Prepare has already validated the local reading and the original Arm.
        arm.validate_clock_check(observed_clock).ok()?;
        Some(Self {
            prepared_floor_ns: prepare_clock
                .continuous_ns()
                .ok()?
                .max(observed_clock.continuous_ns().ok()?),
            deadline_ns: arm.deadline_ns_decimal.parse().ok()?,
        })
    }

    fn check(
        &self,
        original: &HostClockSampleV1,
        observed: &HostClockSampleV1,
        current: &HostClockSampleV1,
    ) -> Result<(), NativeConnectRefusal> {
        use NativeConnectRefusal::{Expired, Unavailable};

        if original.method != current.method
            || original.boot_id != current.boot_id
            || observed.method != current.method
            || observed.boot_id != current.boot_id
        {
            return Err(Unavailable);
        }
        let now = current.continuous_ns().map_err(|_| Unavailable)?;
        let previous = observed.continuous_ns().map_err(|_| Unavailable)?;
        if now < self.prepared_floor_ns || now < previous {
            return Err(Unavailable);
        }
        if now >= self.deadline_ns {
            return Err(Expired);
        }
        if now - previous > HEALTH_WINDOW.as_nanos() {
            return Err(Unavailable);
        }
        Ok(())
    }
}

struct SpawnGuard {
    client: EnforcerClient,
    local: Arc<LocalSlot>,
    // Keep the exact live claim (including its emitter) in the single-use
    // guard until the last check for the registered spawn.
    permission: FreshStartPermission,
    clock: Arc<NativeHostClock>,
}
impl SpawnGuard {
    fn check_before_spawn(
        self,
        authority: &RuntimeAuthority,
        created: &OwnedContainerSnapshot,
    ) -> anyhow::Result<()> {
        let arm = &self.local.registration.arm;
        let failure = |reason, sample: Option<&HostClockSampleV1>| {
            EnforcerStageFailure::from_arm(arm, EnforcerStage::BeforeSpawn, reason, sample)
        };
        if self.local.latch.revoked() {
            return Err(failure(EnforcerFailureReason::StopSignalled, None).into());
        }
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
        if !self
            .local
            .registration
            .matches_spawn_target(authority, created)
        {
            return Err(failure(EnforcerFailureReason::TargetMismatch, None).into());
        }
        let expected_generation = self.permission.generation();
        if self.local.generation.current() != expected_generation || expected_generation == u64::MAX
        {
            return Err(failure(EnforcerFailureReason::GenerationChanged, None).into());
        }
        if status.arm_id != self.permission.arm_id()
            || status.registration_hash != self.local.digest
        {
            return Err(failure(EnforcerFailureReason::ResponseBindingMismatch, None).into());
        }
        if let Some(reason) = spawn_snapshot_failure(status, expected_generation, received) {
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
        self.check_health_with_lock(false)
    }

    /// Release already owns the cancellation gate. Refuse a busy health snapshot.
    fn try_check_health(&self) -> EnforcerResult<()> {
        self.check_health_with_lock(true)
    }

    fn check_health_with_lock(&self, nonblocking: bool) -> EnforcerResult<()> {
        let mut descriptor = libc::pollfd {
            fd: self.liveness.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialized pollfd for a socket retained by self, timeout
        // zero. POLLHUP is checked even if old heartbeat bytes remain queued.
        let polled = unsafe { libc::poll(&mut descriptor, 1, 0) };
        let health = if nonblocking {
            self.health.try_lock().ok()
        } else {
            self.health.lock().ok()
        }
        .ok_or(EnforcerError::Unhealthy)?;
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
    record_health_and_slots(client, value).await
}

async fn record_health_and_slots(client: &Client, value: EnforcerStatusV1) -> EnforcerResult<()> {
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
            .filter(|slot| slot.acknowledged.load(Ordering::SeqCst) && !slot.latch.revoked())
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
        let replacement = match EnforcerClient::connect_existing_legacy_mode(
            &client.root,
            &client.executable,
            client.max_slots,
            client
                .legacy
                .as_ref()
                .map_or(0, |legacy| legacy.recovery_budget),
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
        client.require_historical_legacy_registration(local.registration.arm_id())?;
        if slots.len() >= MAX_LEGACY_RETAINED_STOPS
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

// Closed enforcer recovery helper; production wiring pending.
#[allow(dead_code)]
pub(super) fn spawn_recovery_helper(
    root: &Directory,
    executable: &Path,
    max_slots: u32,
) -> EnforcerResult<()> {
    spawn_recovery_helper_with_budget(root, executable, max_slots, 0)
}

pub(super) fn spawn_recovery_helper_with_budget(
    root: &Directory,
    executable: &Path,
    max_slots: u32,
    max_recovery_stop_owners: u32,
) -> EnforcerResult<()> {
    if !(1..=MAX_SLOTS).contains(&max_slots)
        || max_recovery_stop_owners > MAX_SLOTS
        || !executable.is_absolute()
    {
        return Err(EnforcerError::InvalidInput);
    }
    spawn_helper_config(
        root,
        executable,
        max_slots,
        HelperMode::Recovery {
            max_recovery_stop_owners,
        },
    )
}

enum HelperMode<'a> {
    LegacyExisting {
        max_recovery_stop_owners: u32,
    },
    LegacyFresh {
        name: &'a str,
        max_recovery_stop_owners: u32,
        launch_nonce: Uuid,
    },
    Recovery {
        max_recovery_stop_owners: u32,
    },
    Full3Fresh {
        name: &'a str,
        installation_id: Uuid,
        max_recovery_stop_owners: u32,
    },
}

fn spawn_helper_config(
    root: &Directory,
    executable: &Path,
    max_slots: u32,
    mode: HelperMode<'_>,
) -> EnforcerResult<()> {
    spawn_helper_process(root, executable, max_slots, mode)?.detach()
}

fn spawn_helper_process(
    root: &Directory,
    executable: &Path,
    max_slots: u32,
    mode: HelperMode<'_>,
) -> EnforcerResult<SpawnedHelper> {
    use std::io::Write;
    use std::os::unix::process::CommandExt;
    let launch_nonce = match &mode {
        HelperMode::LegacyFresh { launch_nonce, .. } => Some(*launch_nonce),
        _ => None,
    };
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
    match mode {
        HelperMode::LegacyExisting {
            max_recovery_stop_owners,
        } => {
            command
                .arg("--max-recovery-stop-owners")
                .arg(max_recovery_stop_owners.to_string());
        }
        HelperMode::LegacyFresh {
            name,
            max_recovery_stop_owners,
            ..
        } => {
            command
                .arg("--legacy-fresh")
                .arg("--role-name")
                .arg(name)
                .arg("--max-recovery-stop-owners")
                .arg(max_recovery_stop_owners.to_string())
                .stdin(Stdio::piped());
        }
        HelperMode::Recovery {
            max_recovery_stop_owners,
        } => {
            command
                .arg("--recovery-only")
                .arg("--max-recovery-stop-owners")
                .arg(max_recovery_stop_owners.to_string());
        }
        HelperMode::Full3Fresh {
            name,
            installation_id,
            max_recovery_stop_owners,
        } => {
            command
                .arg("--full3-fresh")
                .arg("--role-name")
                .arg(name)
                .arg("--installation-id")
                .arg(installation_id.to_string())
                .arg("--max-recovery-stop-owners")
                .arg(max_recovery_stop_owners.to_string());
        }
    }
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
    let bootstrap = if let Some(nonce) = launch_nonce {
        match child.stdin.take() {
            Some(mut pipe) => pipe
                .write_all(nonce.as_bytes())
                .map_err(|_| EnforcerError::Unavailable),
            None => Err(EnforcerError::Unavailable),
        }
    } else {
        Ok(())
    };
    let process = SpawnedHelper(Some(child));
    bootstrap?;
    Ok(process)
}

struct SpawnedHelper(Option<Child>);

impl SpawnedHelper {
    fn detach(&mut self) -> EnforcerResult<()> {
        let Some(mut child) = self.0.take() else {
            return Ok(());
        };
        // Reap when the helper exits. This thread carries no broker lease or
        // credential FD, and relinquishing custody never signals the helper.
        std::thread::Builder::new()
            .name("enforcer-reap".into())
            .spawn(move || {
                let _ = child.wait();
            })
            .map_err(|_| EnforcerError::Unavailable)?;
        Ok(())
    }
}

impl Drop for SpawnedHelper {
    fn drop(&mut self) {
        let _ = self.detach();
    }
}

#[cfg(target_os = "macos")]
fn peer_pid(stream: &UnixStream) -> Option<u32> {
    let mut pid: libc::pid_t = 0;
    let expected_len = std::mem::size_of_val(&pid) as libc::socklen_t;
    let mut len = expected_len;
    // SAFETY: LOCAL_PEERPID fills one initialized pid_t for this Unix socket;
    // both pointers are writable for the complete native object sizes.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut len,
        )
    };
    (rc == 0 && len == expected_len && pid > 0).then_some(pid as u32)
}

#[cfg(not(target_os = "macos"))]
fn peer_pid(stream: &UnixStream) -> Option<u32> {
    u32::try_from(stream.peer_cred().ok()?.pid()?).ok()
}

#[cfg(test)]
mod stage_diagnostic_tests {
    use super::*;

    #[test]
    fn native_release_deadline_rejects_expiry_regression_stale_and_foreign_clock() {
        use crate::control::HostClockMethodV1;

        let sample = |ns: u128| HostClockSampleV1 {
            version: 1,
            method: HostClockMethodV1::LinuxBoottimeV1,
            boot_id: "11111111-1111-4111-8111-111111111111".into(),
            continuous_ns_decimal: ns.to_string(),
            observed_utc: None,
        };
        let original = sample(1);
        let deadline = WorkerReleaseDeadline {
            prepared_floor_ns: 100,
            deadline_ns: 2_000_000_000,
        };
        let before = sample(1_999_999_999);
        assert!(deadline.check(&original, &before, &before).is_ok());
        assert_eq!(
            deadline.check(&original, &before, &sample(2_000_000_000)),
            Err(NativeConnectRefusal::Expired)
        );
        assert!(deadline.check(&original, &sample(10), &sample(99)).is_err());
        assert!(
            deadline
                .check(&original, &sample(120), &sample(119))
                .is_err()
        );
        assert!(
            deadline
                .check(&original, &sample(100), &sample(750_000_101))
                .is_err()
        );

        let mut foreign = before.clone();
        foreign.boot_id = "22222222-2222-4222-8222-222222222222".into();
        assert!(deadline.check(&original, &before, &foreign).is_err());
        let mut wrong_method = before.clone();
        wrong_method.method = HostClockMethodV1::DarwinMachContinuousV1;
        assert!(deadline.check(&original, &wrong_method, &before).is_err());
    }

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
