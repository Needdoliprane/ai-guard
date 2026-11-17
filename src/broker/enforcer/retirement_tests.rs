use super::*;
use crate::{
    broker::{discovery::EnforcerRoleLeaseV2, journal::JournalScopeV1},
    control_fs::Directory,
};
use serde_json::json;
use std::sync::{OnceLock, Weak};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::mpsc::sync_channel,
};

#[path = "native_admission_tests.rs"]
mod native_admission_tests;

#[path = "retirement_prospective_tests.rs"]
mod prospective_tests;

#[path = "retirement_continuation_regression_tests.rs"]
mod continuation_regressions;

/// Startup-only opt-in for short Unix socket fixture paths in read-only-root
/// QA. Keep the historical /tmp default instead of macOS's often long TMPDIR.
fn short_ipc_fixture_root() -> PathBuf {
    let configured = std::env::var_os("AI_GUARD_TEST_D5_IPC_ROOT").map(PathBuf::from);
    let requested = configured.as_deref().unwrap_or_else(|| Path::new("/tmp"));
    assert!(
        requested.is_absolute(),
        "AI_GUARD_TEST_D5_IPC_ROOT must name an absolute directory"
    );
    let canonical = std::fs::canonicalize(requested)
        .expect("AI_GUARD_TEST_D5_IPC_ROOT or default /tmp must exist");
    assert!(
        canonical.is_dir(),
        "the D5 IPC fixture root must be a directory"
    );
    if configured.is_some() {
        assert_eq!(
            requested,
            canonical.as_path(),
            "AI_GUARD_TEST_D5_IPC_ROOT must already be canonical"
        );
    }
    canonical
}

// Explicitly SYNTHETIC Ready for IPC capability tests only. The real original
// thread and NativeOwners handoff/join remain in use. No runtime is invoked,
// observed or qualified, and this hook cannot be selected outside cfg(test).
pub(super) struct SyntheticReadyControl {
    actor: Mutex<Weak<FullActor>>,
    return_allowed: AtomicBool,
    force_exit: AtomicBool,
    stop_observed: AtomicBool,
    ready_allowed: AtomicBool,
}
static SYNTHETIC_READY: OnceLock<Mutex<BTreeMap<Uuid, Arc<SyntheticReadyControl>>>> =
    OnceLock::new();
fn synthetic_registry() -> &'static Mutex<BTreeMap<Uuid, Arc<SyntheticReadyControl>>> {
    SYNTHETIC_READY.get_or_init(|| Mutex::new(BTreeMap::new()))
}
pub(super) fn synthetic_ready_original(actor: &Arc<FullActor>) -> Option<EnforcerResult<()>> {
    let control = synthetic_registry()
        .lock()
        .unwrap()
        .get(&actor.key.arm_id)
        .cloned()?;
    *control.actor.lock().unwrap() = Arc::downgrade(actor);
    let clock = NativeHostClock::new().unwrap();
    loop {
        if control.force_exit.load(Ordering::SeqCst) {
            return Some(Err(EnforcerError::Unhealthy));
        }
        let mut state = actor.state.lock().unwrap();
        if actor.exit.load(Ordering::SeqCst) && control.return_allowed.load(Ordering::SeqCst) {
            return Some(Ok(()));
        }
        state.status.clock = Some(clock.sample().unwrap());
        if !state.gate.is_closing() {
            let stopping = state.gate.stop_reserved;
            control.stop_observed.store(stopping, Ordering::SeqCst);
            if stopping || control.ready_allowed.load(Ordering::SeqCst) {
                state.status.phase = if stopping {
                    SlotPhaseV1::Stopping
                } else {
                    SlotPhaseV1::Ready
                };
            }
        }
        let _waiting = actor
            .wake
            .wait_timeout(state, Duration::from_millis(10))
            .unwrap();
    }
}
impl SyntheticReadyControl {
    fn release(&self) {
        self.return_allowed.store(true, Ordering::SeqCst);
        if let Some(actor) = self.actor.lock().unwrap().upgrade() {
            actor.wake.notify_all();
        }
    }
    fn force(&self) {
        self.force_exit.store(true, Ordering::SeqCst);
        self.release();
    }
}

pub(super) struct OriginalIpcFixture {
    files: Option<tempfile::TempDir>,
    root: Directory,
    full: Arc<FullState>,
    state: Arc<super::super::recovery::RecoveryState>,
    server: Option<tokio::task::JoinHandle<EnforcerResult<()>>>,
    controls: Vec<(Uuid, Arc<SyntheticReadyControl>)>,
    max_slots: u32,
}
impl OriginalIpcFixture {
    async fn start() -> Self {
        Self::start_for_installation(Uuid::new_v4()).await
    }
    pub(super) async fn start_for_installation(installation_id: Uuid) -> Self {
        Self::start_with_capacity(installation_id, 8).await
    }
    pub(super) async fn start_with_capacity(installation_id: Uuid, max_slots: u32) -> Self {
        use crate::broker::local_ipc::PrivateListener;
        let files = tempfile::Builder::new()
            .prefix("f3r-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(short_ipc_fixture_root())
            .unwrap();
        let parent = Directory::open_existing(files.path()).unwrap();
        let ledger = Arc::new(
            RetirementLedger::initialize_new(
                parent.create_enforcer_root("role").unwrap(),
                installation_id,
            )
            .unwrap(),
        );
        let root = ledger.lease().ledger_root().clone();
        capability(&root, true).unwrap();
        let listener = PrivateListener::bind_new(&ledger.lease().listener_root().unwrap()).unwrap();
        let state = super::super::recovery::full_fixture_state(
            Arc::clone(&ledger),
            listener.endpoint(),
            max_slots,
        )
        .unwrap();
        ledger
            .lease()
            .publish_endpoint(listener.endpoint())
            .unwrap();
        let full = state.full_owner().unwrap().unwrap();
        let serving = Arc::clone(&state);
        let server =
            tokio::spawn(
                async move { super::super::recovery::serve_listener(listener, serving).await },
            );
        Self {
            files: Some(files),
            root,
            full,
            state,
            server: Some(server),
            controls: Vec::new(),
            max_slots,
        }
    }
    pub(super) fn install(
        &mut self,
        registration: &RegistrationV1,
        allow_return: bool,
    ) -> Arc<SyntheticReadyControl> {
        self.install_arm(registration.arm_id(), allow_return)
    }
    pub(super) fn install_arm(
        &mut self,
        arm_id: &str,
        allow_return: bool,
    ) -> Arc<SyntheticReadyControl> {
        let id = Uuid::parse_str(arm_id).unwrap();
        let control = Arc::new(SyntheticReadyControl {
            actor: Mutex::new(Weak::new()),
            return_allowed: AtomicBool::new(allow_return),
            force_exit: AtomicBool::new(false),
            stop_observed: AtomicBool::new(false),
            ready_allowed: AtomicBool::new(true),
        });
        assert!(
            synthetic_registry()
                .lock()
                .unwrap()
                .insert(id, Arc::clone(&control))
                .is_none()
        );
        self.controls.push((id, Arc::clone(&control)));
        control
    }
    pub(super) async fn client(&self) -> super::super::client::EnforcerFullClientV3 {
        super::super::client::EnforcerFullClientV3::connect_existing(&self.root, self.max_slots)
            .await
            .unwrap()
    }
    pub(super) async fn register_new(
        &self,
        client: &super::super::client::EnforcerFullClientV3,
        registration: RegistrationV1,
        generation: CancelGeneration,
    ) -> EnforcerResult<EnforcedSlot> {
        let target = prospective_registration(&registration);
        client
            .reserve_prospective(target.clone(), generation.clone(), false)
            .await?;
        let exchange = client
            .begin_prospective_arm_exchange(&target, &generation)
            .await?;
        client
            .bind_prospective(registration, generation, exchange)
            .await
    }
    async fn raw_bind(
        &self,
        registration: RegistrationV1,
        generation: u64,
        observe_only: bool,
    ) -> EnforcerResult<FullReply> {
        let reservation = match self
            .raw(FullCommand::ReserveProspective {
                prospective: Box::new(prospective_registration(&registration)),
                generation,
                replay_only: false,
            })
            .await?
        {
            FullReply::ProspectiveReserved { reservation } => reservation,
            FullReply::Error { code } => return Err(code),
            _ => return Err(EnforcerError::Conflict),
        };
        let original_exchange_nonce = match self
            .raw(FullCommand::BeginProspectiveArmExchange {
                reservation: reservation.clone(),
                generation,
            })
            .await?
        {
            FullReply::ProspectiveArmExchange {
                original_exchange_nonce,
            } => original_exchange_nonce,
            FullReply::Error { code } => return Err(code),
            _ => return Err(EnforcerError::Conflict),
        };
        self.raw(FullCommand::BindProspective {
            reservation,
            registration: Box::new(registration),
            generation,
            observe_only,
            original_exchange_nonce,
        })
        .await
    }
    pub(super) async fn stop_client(
        &self,
    ) -> EnforcerResult<crate::broker::enforcer::EnforcerStopClient> {
        crate::broker::enforcer::EnforcerStopClient::connect_or_spawn_existing_sealed(
            &self.root,
            &std::env::current_exe().map_err(|_| EnforcerError::Unavailable)?,
            self.max_slots,
        )
        .await
    }
    /// Stop only this fixture's listener task, keeping its exact endpoint
    /// inode and role/native owners. No namespace is moved, removed or restored.
    pub(super) async fn stop_listener_preserving_endpoint(&mut self) -> EnforcerResult<()> {
        let listener_root = self.full.ledger.lease().listener_root()?;
        let endpoint = crate::broker::discovery::load_endpoint(&self.root)?
            .ok_or(EnforcerError::Unavailable)?;
        endpoint.validate_at(&listener_root)?;
        let mut server = self.server.take().ok_or(EnforcerError::Conflict)?;
        self.state.shutdown_requested.store(true, Ordering::SeqCst);
        match tokio::time::timeout(Duration::from_secs(3), &mut server).await {
            Ok(joined) => joined.map_err(|_| EnforcerError::Unavailable)??,
            Err(_) => {
                self.server = Some(server);
                return Err(EnforcerError::Unavailable);
            }
        }
        endpoint.validate_at(&listener_root)?;
        // The retained socket is safe but has no listening FD. This probe
        // cannot emit a handshake or command, and pins the intended fault.
        match endpoint.connect_checked(&listener_root).await {
            Err(crate::broker::local_ipc::IpcError::Unavailable) => Ok(()),
            Ok(stream) => {
                drop(stream);
                Err(EnforcerError::Conflict)
            }
            Err(_) => Err(EnforcerError::Conflict),
        }
    }
    async fn raw(&self, command: FullCommand) -> EnforcerResult<FullReply> {
        let (mut stream, session) = self.raw_connection().await?;
        super::super::recovery::recovery_v3::request_full(
            &mut stream,
            &session,
            command,
            REGISTER_TIMEOUT,
        )
        .await
    }
    async fn raw_connection(
        &self,
    ) -> EnforcerResult<(
        tokio::net::UnixStream,
        super::super::recovery::recovery_v3::Session3,
    )> {
        let endpoint = crate::broker::discovery::load_endpoint(&self.root)?
            .ok_or(EnforcerError::Unavailable)?;
        let endpoints = self.full.ledger.lease().listener_root()?;
        let mut stream = endpoint.connect_checked(&endpoints).await?;
        let binding = crate::broker::discovery::recovery_root_binding(&self.root, &endpoint)?;
        let (session, _) = super::super::recovery::recovery_v3::connect_full(
            &mut stream,
            &capability(&self.root, false)?,
            endpoint.epoch(),
            &binding,
            self.max_slots,
        )
        .await?;
        Ok((stream, session))
    }
    fn identity(&self, unused: &UnusedPrepareRevoked, job_id: Uuid) -> RetirementIdentityV1 {
        let registration = unused.registration();
        let scope = JournalScopeV1::new(
            *unused.ledger().installation_id(),
            &registration.tenant_id,
            "http://127.0.0.1:8787",
            Uuid::parse_str(&registration.receipt.control_namespace_uuid).unwrap(),
            "real-ipc-test",
        )
        .unwrap();
        RetirementIdentityV1::new(
            scope,
            job_id,
            RetirementDigestV1::parse(&registration.arm.intent_hash).unwrap(),
            registration.arm.clone(),
            registration.receipt.clone(),
            RetirementDigestV1::parse(&registration.digest().unwrap()).unwrap(),
            unused.epoch(),
            unused.generation(),
            unused.retirement_id(),
            RetirementReasonV1::UnusedPrepareRevoked,
        )
        .unwrap()
    }
    fn command(
        &self,
        unused: &UnusedPrepareRevoked,
        job_id: Uuid,
    ) -> (RetirementIdentityV1, FullCommand) {
        let identity = self.identity(unused, job_id);
        let evidence = RetirementEvidenceV1::new(
            identity.digest().unwrap(),
            unused.actor_nonce(),
            unused.revocation_nonce(),
        )
        .unwrap();
        let audit_ack = RetirementAuditAckV1::new(
            Uuid::new_v4(),
            RetirementDigestV1::parse(&"a".repeat(64)).unwrap(),
            1,
        )
        .unwrap();
        (
            identity.clone(),
            FullCommand::RetireUnused {
                identity: Box::new(identity),
                evidence,
                audit_ack,
            },
        )
    }
    pub(super) async fn finish(mut self) {
        for (_, control) in &self.controls {
            control.force();
        }
        self.full
            .finish_unbound_prospective_fixture()
            .await
            .unwrap();
        let actors: Vec<_> = self.full.actors.lock().unwrap().values().cloned().collect();
        for actor in actors {
            if let Ok(mut handle) = self.full.owners.take_retiring(actor.key) {
                handle.join().unwrap();
            }
        }
        let expires = Instant::now() + Duration::from_secs(3);
        loop {
            let completed = self
                .full
                .retiring
                .lock()
                .unwrap()
                .values()
                .all(|operation| operation.receiver.borrow().is_some());
            if completed {
                break;
            }
            assert!(
                Instant::now() < expires,
                "test coordinator did not return after its actual native join"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        self.full.actors.lock().unwrap().clear();
        let joiner = self.full.joiner.lock().unwrap().take();
        if let Some(joiner) = joiner {
            // Teardown joins support work; an unhealthy role remains fenced.
            // This is deliberately no Joined wrapper, Q, or retirement.
            let _ = self.full.owners.finish_sealed_idle(joiner).await;
        }
        self.state.shutdown_requested.store(true, Ordering::SeqCst);
        if let Some(server) = self.server.take() {
            tokio::time::timeout(Duration::from_secs(3), server)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
        for (id, _) in &self.controls {
            synthetic_registry().lock().unwrap().remove(id);
        }
        if self.full.owners.failed() {
            let preserved = self.files.take().unwrap().keep();
            eprintln!(
                "private synthetic-Ready IPC role retained without a retirement claim: {}",
                preserved.display()
            );
        }
    }
}
impl Drop for OriginalIpcFixture {
    fn drop(&mut self) {
        let abandoned = self.server.is_some();
        for (id, control) in &self.controls {
            control.force();
            synthetic_registry().lock().unwrap().remove(id);
        }
        if let Some(server) = self.server.take() {
            server.abort();
        }
        if (std::thread::panicking() || abandoned)
            && let Some(files) = self.files.take()
        {
            eprintln!("private failed IPC fixture: {}", files.keep().display());
        }
    }
}

#[tokio::test]
async fn register_ack_discarded_then_new_client_replay_cannot_recreate_original_witness() {
    let mut fixture = OriginalIpcFixture::start().await;
    let lost_registration = registration(fixture.root.path());
    fixture.install(&lost_registration, false);
    // The reply crosses the real authenticated socket but is discarded before
    // any EnforcedSlot/live original capability is created for the caller.
    assert!(matches!(
        fixture
            .raw_bind(lost_registration.clone(), 0, false)
            .await
            .unwrap(),
        FullReply::Registered {
            replayed: false,
            ..
        }
    ));
    let recovered = fixture.client().await;
    let replay = recovered
        .register(lost_registration.clone(), CancelGeneration::new())
        .await
        .unwrap();
    assert!(matches!(
        replay.revoke_unused(Uuid::new_v4()).await,
        Err(EnforcerError::Unauthorized)
    ));
    let actor = fixture
        .full
        .actor(
            &lost_registration.arm.operation_id,
            &lost_registration.digest().unwrap(),
        )
        .unwrap();
    assert!(matches!(
        fixture
            .raw(FullCommand::RevokeUnused {
                arm_id: lost_registration.arm.operation_id.clone(),
                registration_hash: RetirementDigestV1::parse(&lost_registration.digest().unwrap())
                    .unwrap(),
                generation: 0,
                actor_nonce: actor.key.nonce,
                original_registration_nonce: actor.key.nonce,
                retirement_id: Uuid::new_v4()
            })
            .await
            .unwrap(),
        FullReply::Error {
            code: EnforcerError::Unauthorized
        }
    ));
    let fresh_registration = registration(fixture.root.path());
    fixture.install(&fresh_registration, false);
    let generation = CancelGeneration::new();
    let original = fixture
        .register_new(&recovered, fresh_registration.clone(), generation.clone())
        .await
        .unwrap();
    // A replay through the SAME owner retains its already-issued capability.
    let same_owner = recovered
        .register(fresh_registration, generation)
        .await
        .unwrap();
    original.revoke_unused(Uuid::new_v4()).await.unwrap();
    assert!(same_owner.revoke_unused(Uuid::new_v4()).await.is_err());
    fixture.finish().await;
}

#[tokio::test]
async fn claim_reservation_cannot_be_consumed_for_another_slot_or_pending_arm() {
    let mut fixture = OriginalIpcFixture::start().await;
    let client = fixture.client().await;
    let a = registration(fixture.root.path());
    let b = registration(fixture.root.path());
    fixture.install(&a, false);
    fixture.install(&b, false);
    let slot_a = fixture
        .register_new(&client, a.clone(), CancelGeneration::new())
        .await
        .unwrap();
    let slot_b = fixture
        .register_new(&client, b.clone(), CancelGeneration::new())
        .await
        .unwrap();
    let reservation = slot_a.reserve_claim(0).unwrap();
    assert_eq!(
        slot_b.consume_claim_reservation(reservation, &b.arm, 0),
        Err(EnforcerError::Conflict)
    );
    assert!(matches!(
        slot_a.revoke_unused(Uuid::new_v4()).await,
        Err(EnforcerError::Conflict)
    ));
    slot_b.revoke_unused(Uuid::new_v4()).await.unwrap();
    let c = registration(fixture.root.path());
    fixture.install(&c, false);
    let slot_c = fixture
        .register_new(&client, c.clone(), CancelGeneration::new())
        .await
        .unwrap();
    let reservation = slot_c.reserve_claim(0).unwrap();
    slot_c
        .consume_claim_reservation(reservation, &c.arm, 0)
        .unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn two_jobs_can_finalize_and_replay_the_same_retirement_uuid_independently() {
    let mut fixture = OriginalIpcFixture::start().await;
    let client = fixture.client().await;
    let retirement_id = Uuid::new_v4();
    let first_job = Uuid::new_v4();
    let mut identities = Vec::new();
    let shared_namespace = Uuid::new_v4().to_string();
    for job_id in [first_job, Uuid::new_v4()] {
        let mut registration = registration(fixture.root.path());
        registration.receipt.control_namespace_uuid = shared_namespace.clone();
        fixture.install(&registration, true);
        let slot = fixture
            .register_new(&client, registration, CancelGeneration::new())
            .await
            .unwrap();
        let unused = slot.revoke_unused(retirement_id).await.unwrap();
        let (identity, command) = fixture.command(&unused, job_id);
        let reply = fixture.raw(command).await.unwrap();
        let (kind, code) = match &reply {
            FullReply::Status { .. } => ("status", None),
            FullReply::Slots { .. } => ("slots", None),
            FullReply::Registered { .. } => ("registered", None),
            FullReply::ProspectiveReserved { .. } => ("prospective_reserved", None),
            FullReply::ProspectiveArmExchange { .. } => ("prospective_arm_exchange", None),
            FullReply::Prepared { .. } => ("prepared", None),
            FullReply::Revoked { .. } => ("revoked", None),
            FullReply::Retirement {
                receipt: Some(_), ..
            } => ("retirement_some", None),
            FullReply::Retirement { receipt: None, .. } => ("retirement_none", None),
            FullReply::Acknowledged {} => ("acknowledged", None),
            FullReply::Error { code } => ("error", Some(*code)),
        };
        assert!(
            matches!(
                reply,
                FullReply::Retirement {
                    receipt: Some(_),
                    ..
                }
            ),
            "expected retirement_some; reply_kind={kind}; reply_code={code:?}"
        );
        identities.push(identity);
    }
    for identity in &identities {
        assert!(client.query_retirement(identity).await.unwrap().is_some());
    }
    let mut divergent = registration(fixture.root.path());
    divergent.receipt.control_namespace_uuid = shared_namespace;
    fixture.install(&divergent, true);
    let slot = fixture
        .register_new(&client, divergent, CancelGeneration::new())
        .await
        .unwrap();
    let unused = slot.revoke_unused(retirement_id).await.unwrap();
    let (_, command) = fixture.command(&unused, first_job);
    assert!(matches!(
        fixture.raw(command).await.unwrap(),
        FullReply::Error {
            code: EnforcerError::Conflict
        }
    ));
    fixture.finish().await;
}

#[tokio::test]
async fn full_retire_waiters_and_eof_do_not_exhaust_independent_stop_admission() {
    use super::super::recovery::recovery_v3;
    let mut fixture = OriginalIpcFixture::start().await;
    let client = fixture.client().await;
    let blocked_registration = registration(fixture.root.path());
    let blocked = fixture.install(&blocked_registration, false);
    let slot = fixture
        .register_new(&client, blocked_registration, CancelGeneration::new())
        .await
        .unwrap();
    let unused = slot.revoke_unused(Uuid::new_v4()).await.unwrap();
    let (identity, command) = fixture.command(&unused, Uuid::new_v4());
    let other = registration(fixture.root.path());
    let other_control = fixture.install(&other, false);
    let _other_slot = fixture
        .register_new(&client, other.clone(), CancelGeneration::new())
        .await
        .unwrap();
    let mut waiters = Vec::new();
    for index in 0..40 {
        if let Ok((mut stream, session)) = fixture.raw_connection().await
            && recovery_v3::send_full_request_only_for_test(&mut stream, &session, command.clone())
                .await
                .is_ok()
        {
            if index % 2 == 0 {
                drop(stream);
            } else {
                waiters.push(stream);
            }
        }
    }
    let expires = Instant::now() + Duration::from_secs(3);
    while !matches!(
        fixture
            .full
            .ledger
            .query(&identity)
            .unwrap()
            .as_ref()
            .map(|entry| entry.phase()),
        Some(RetirementLedgerPhaseV1::Tombstone { .. })
    ) {
        assert!(Instant::now() < expires);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let accepted = tokio::time::timeout(
        Duration::from_secs(2),
        recovery_v3::deliver_synthetic_stop_only_for_test(&fixture.root, other),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(accepted.phase(), super::super::StopPhaseV1::StopRequested);
    let expires = Instant::now() + Duration::from_secs(2);
    while !other_control.stop_observed.load(Ordering::SeqCst) {
        assert!(Instant::now() < expires);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(fixture.full.ledger.query(&identity).unwrap().is_some());
    assert!(!blocked.return_allowed.load(Ordering::SeqCst));
    drop(waiters);
    blocked.release();
    let expires = Instant::now() + Duration::from_secs(3);
    loop {
        if matches!(
            fixture
                .full
                .ledger
                .query(&identity)
                .unwrap()
                .as_ref()
                .map(|entry| entry.phase()),
            Some(RetirementLedgerPhaseV1::Final { .. })
        ) {
            break;
        }
        assert!(Instant::now() < expires);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    fixture.finish().await;
}

fn registration(root: &Path) -> RegistrationV1 {
    let authority: RuntimeAuthority = serde_json::from_value(json!({
        "authority_id":Uuid::new_v4(),"kind":"docker","endpoint":format!("unix://{}",root.join("absent.sock").display()),
        "engine_id":"explicit-unavailable-test-data","podman":null,"invocation":{"policy_version":2,"kind":"docker",
        "approved_entry":root.join("absent-runtime"),"canonical_program":root.join("absent-runtime"),
        "file_identity":{"device":1,"inode":2,"owner":0,"group":0,"mode":493,"length":1,"sha256":"b".repeat(64)},"environment":{},"cwd":root}
    })).unwrap();
    let cid = "a".repeat(64);
    let image = format!("sha256:{}", "b".repeat(64));
    let inspection: crate::control::RuntimeInspectionV1 = serde_json::from_value(json!({
        "authority_id":authority.authority_id,"runtime_kind":"docker","container_id":cid,"stable_name":"real-join-test",
        "volume_name":"real-join-test-work","image_id":image,"spec_hash":"d".repeat(64),"cpu_millis":1000,
        "memory_bytes":1073741824_u64,"pids_limit":256,"network":"none","security_preset":"fixed_sandbox_v2","restart_policy":"no"
    })).unwrap();
    let owned: OwnedContainerSnapshot = serde_json::from_value(json!({"handle":{
        "authority_fingerprint":authority.fingerprint(),"session":Uuid::new_v4(),"operation":Uuid::new_v4(),"id":cid,
        "name":"real-join-test","configuration_fingerprint_version":2},"observation":{"id":cid,"name":"real-join-test","image_id":image,
        "restart_count":0,"process":{"status":"running","running":true,"started_at":"2026-11-02T00:00:00Z","finished_at":null,"exit_code":null,"oom_killed":false},
        "allocation":{"cpu":{"quota":100000,"period":100000},"memory_bytes":1073741824_u64,"pids_limit":256},
        "configuration_fingerprint":"c".repeat(64),"configuration_fingerprint_version":2,"observed_at":"2026-11-02T00:00:00Z","diagnostics":[]}}
    )).unwrap();
    let clock = NativeHostClock::new().unwrap().sample().unwrap();
    let arm: ArmV1 = serde_json::from_value(json!({"version":1,"operation_id":Uuid::new_v4(),"worker_id":Uuid::new_v4(),"attempt_id":Uuid::new_v4(),
        "expected_revision":1,"intent_hash":"e".repeat(64),"subject":{"kind":"worker"},"inspection":inspection,
        "authority_binding_hash":authority.fingerprint(),"configuration_fingerprint_version":2,"configuration_fingerprint":"c".repeat(64),
        "deadline_ns_decimal":crate::broker::clock::checked_deadline(clock.continuous_ns().unwrap(),3600).unwrap().to_string(),"clock":clock,"timeout_seconds":3600})).unwrap();
    let receipt = ArmReceiptV1 {
        version: 1,
        arm_id: arm.operation_id.clone(),
        worker_id: arm.worker_id.clone(),
        attempt_id: arm.attempt_id.clone(),
        input_hash: arm.input_hash().unwrap(),
        control_namespace_uuid: Uuid::new_v4().to_string(),
        audit_seq: 1,
        audit_hash: "f".repeat(64),
    };
    RegistrationV1::new(
        hex::encode(Sha256::digest(b"http://127.0.0.1:8787")),
        "tenant-real-join".into(),
        arm,
        receipt,
        authority,
        owned,
    )
    .unwrap()
}

fn prospective_registration(registration: &RegistrationV1) -> ProspectiveRegistrationV1 {
    ProspectiveRegistrationV1::new(
        registration.control_origin_hash.clone(),
        registration.tenant_id.clone(),
        registration.arm.clone(),
        registration.authority.clone(),
        registration.owned.clone(),
    )
    .unwrap()
}

#[tokio::test]
async fn tombstone_retains_capacity_until_real_handle_returns_then_q_is_replayable() {
    let files = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let parent = Directory::open_existing(files.path()).unwrap();
    let installation = Uuid::new_v4();
    let ledger = Arc::new(
        RetirementLedger::initialize_new(
            parent.create_enforcer_root("role").unwrap(),
            installation,
        )
        .unwrap(),
    );
    let epoch = Uuid::new_v4();
    let admitted = NativeAdmissionRegistry::fresh();
    let full = FullState::new(
        Arc::clone(&ledger),
        epoch,
        1,
        Some(Arc::new(NativeHostClock::new().unwrap())),
        Arc::clone(&admitted),
    )
    .unwrap();
    let registration = registration(files.path());
    let arm_id = Uuid::parse_str(&registration.arm.operation_id).unwrap();
    let key = ActorKey {
        arm_id,
        epoch,
        nonce: Uuid::new_v4(),
    };
    let retirement_id = Uuid::new_v4();
    let scope: JournalScopeV1 = serde_json::from_value(json!({"version":1,"installation_id":installation,"tenant_id":"tenant-real-join",
        "gateway_origin":"http://127.0.0.1:8787","control_namespace_uuid":registration.receipt.control_namespace_uuid,"profile_id":"real-join"})).unwrap();
    let identity = RetirementIdentityV1::new(
        scope,
        Uuid::new_v4(),
        RetirementDigestV1::parse(&registration.arm.intent_hash).unwrap(),
        registration.arm.clone(),
        registration.receipt.clone(),
        RetirementDigestV1::parse(&registration.digest().unwrap()).unwrap(),
        epoch,
        0,
        retirement_id,
        RetirementReasonV1::UnusedPrepareRevoked,
    )
    .unwrap();
    let mut gate = OriginalGate::new(true);
    let revocation_nonce = gate.revoke(retirement_id).unwrap();
    let evidence =
        RetirementEvidenceV1::new(identity.digest().unwrap(), key.nonce, revocation_nonce).unwrap();
    gate.close(retirement_id, revocation_nonce).unwrap();
    let ack = RetirementAuditAckV1::new(
        Uuid::new_v4(),
        RetirementDigestV1::parse(&"f".repeat(64)).unwrap(),
        1,
    )
    .unwrap();
    let accepted = ClosingAcceptanceV1::new(
        identity.clone(),
        evidence.clone(),
        ack.clone(),
        Uuid::new_v4(),
    )
    .unwrap();
    let actor = Arc::new(FullActor {
        registration,
        digest: identity.registration_hash().as_str().into(),
        key,
        original_registration_nonce: Uuid::new_v4(),
        state: Mutex::new(ActorState {
            status: EnforcerSlotStatusV1 {
                arm_id: arm_id.to_string(),
                registration_hash: identity.registration_hash().as_str().into(),
                phase: SlotPhaseV1::Ready,
                prepared_once: false,
                generation: 0,
                clock: None,
                observation: None,
                stop_failures: 0,
            },
            gate,
        }),
        wake: Condvar::new(),
        exit: AtomicBool::new(false),
        runtime_installed: Arc::new(AtomicBool::new(false)),
        failures: NativeStopFailures::new(),
        owners: full.owners.clone(),
    });
    full.actors
        .lock()
        .unwrap()
        .insert(arm_id, Arc::clone(&actor));
    full.closed_arms.lock().unwrap().insert(arm_id);
    admitted
        .lock()
        .unwrap()
        .insert(arm_id.to_string(), RegistryOwner::Full);
    let (release, blocked) = sync_channel::<()>(1);
    let (entered, observed_entry) = sync_channel(1);
    let handle = std::thread::spawn(move || {
        entered.send(()).unwrap();
        let _ = blocked.recv();
    });
    observed_entry.recv_timeout(Duration::from_secs(3)).unwrap();
    full.owners.actor_keyed(key, handle).unwrap();
    let owner = Arc::clone(&full);
    let task =
        tokio::spawn(async move { owner.coordinate(RetiringActor { actor, accepted }).await });
    let expires = Instant::now() + Duration::from_secs(3);
    loop {
        if matches!(
            ledger.query(&identity).unwrap().as_ref().map(|e| e.phase()),
            Some(RetirementLedgerPhaseV1::Tombstone { .. })
        ) {
            break;
        }
        assert!(Instant::now() < expires, "real durable T was not published");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(!task.is_finished(), "T is not a native join");
    assert_eq!(
        admitted.lock().unwrap().len(),
        1,
        "capacity remains held before actual handle return"
    );
    // A separate STOP reservation uses no IO/join/ledger mutex.
    let mut other_stop = OriginalGate::new(true);
    other_stop.reserve_stop().unwrap();
    release.send(()).unwrap();
    let (receipt, head) = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(*head.revision(), 3);
    assert!(admitted.lock().unwrap().is_empty());
    assert!(full.actors.lock().unwrap().is_empty());
    let (replayed_receipt, replayed_head) =
        full.retire(identity.clone(), evidence, ack).await.unwrap();
    assert_eq!(
        receipt.canonical_bytes().unwrap(),
        replayed_receipt.canonical_bytes().unwrap()
    );
    assert_eq!(*replayed_head.revision(), 3);
    assert!(
        admitted.lock().unwrap().is_empty(),
        "ACK loss replay cannot decrement twice"
    );
    let stored = ledger.query(&identity).unwrap().unwrap();
    let RetirementLedgerPhaseV1::Final {
        receipt: replay, ..
    } = stored.phase()
    else {
        panic!("Q not durable");
    };
    assert_eq!(
        receipt.canonical_bytes().unwrap(),
        replay.canonical_bytes().unwrap()
    );
    assert_eq!(
        full.check_restore(&arm_id.to_string()),
        Err(EnforcerError::Conflict)
    );
    full.finish_idle().await.unwrap();
    drop(full);
    let bytes = receipt.canonical_bytes().unwrap();
    drop(ledger);
    let role = parent.persistent_child("role", false).unwrap().unwrap();
    let reopened =
        RetirementLedger::open_existing(EnforcerRoleLeaseV2::acquire_existing(&role).unwrap())
            .unwrap();
    let stored = reopened.query(&identity).unwrap().unwrap();
    let RetirementLedgerPhaseV1::Final { receipt, .. } = stored.phase() else {
        panic!("reopened Q absent");
    };
    assert_eq!(bytes, receipt.canonical_bytes().unwrap());
    // The fixture thread above is real, but no runtime was invoked. This is
    // actor ownership/publication evidence only, never engine qualification.
}

// These tests exercise the very same arbiter used by Prepare and the native
// STOP loop. They make no assertion about a model or container runtime.
#[test]
fn prepare_ack_loss_and_stop_reservation_permanently_exclude_unused() {
    let mut prepared = OriginalGate::new(true);
    prepared.prepare().expect("original prepare wins");
    assert_eq!(
        prepared.revoke(Uuid::new_v4()),
        Err(EnforcerError::AlreadyPrepared)
    );
    let mut stopped = OriginalGate::new(true);
    stopped
        .reserve_stop()
        .expect("first native STOP reservation");
    assert_eq!(stopped.revoke(Uuid::new_v4()), Err(EnforcerError::Conflict));
}

#[test]
fn restoration_never_recreates_original_authority() {
    let mut restored = OriginalGate::new(false);
    assert_eq!(restored.prepare(), Err(EnforcerError::AlreadyPrepared));
    assert_eq!(
        restored.revoke(Uuid::new_v4()),
        Err(EnforcerError::Unauthorized)
    );
    let mut original = OriginalGate::new(true);
    original.restore();
    assert_eq!(original.prepare(), Err(EnforcerError::AlreadyPrepared));
    assert_eq!(
        original.revoke(Uuid::new_v4()),
        Err(EnforcerError::Unauthorized)
    );
}

#[test]
fn closing_wins_irreversibly_against_prepare_stop_and_restore() {
    let mut gate = OriginalGate::new(true);
    let retirement_id = Uuid::new_v4();
    let nonce = gate.revoke(retirement_id).expect("revoke original");
    gate.close(retirement_id, nonce)
        .expect("Closing arbitration");
    assert_eq!(gate.prepare(), Err(EnforcerError::Conflict));
    assert_eq!(gate.reserve_stop(), Err(EnforcerError::Conflict));
    gate.restore();
    assert!(gate.is_closing());
    assert_eq!(
        gate.revoke(Uuid::new_v4()),
        Err(EnforcerError::Unauthorized)
    );
}

#[test]
fn slot_claim_or_revocation_has_one_winner_without_changing_job_generation() {
    let generation = CancelGeneration::new();
    let latch = SlotLatch::default();
    latch
        .reserve_claim()
        .expect("claim reserved before durable request");
    assert_eq!(latch.revoke(), Err(EnforcerError::Conflict));
    assert_eq!(generation.current(), 0);
    let other = SlotLatch::default();
    other.revoke().expect("retirement wins");
    assert_eq!(other.reserve_claim(), Err(EnforcerError::Conflict));
    assert_eq!(generation.current(), 0);
}

#[test]
fn prepare_and_revoke_or_stop_and_closing_have_exactly_one_winner() {
    use std::sync::Barrier;
    for stopping in [false, true] {
        for _ in 0..8 {
            let id = Uuid::new_v4();
            let mut original = OriginalGate::new(true);
            let nonce = if stopping {
                original.revoke(id).unwrap()
            } else {
                Uuid::new_v4()
            };
            let state = Arc::new(Mutex::new(original));
            let barrier = Arc::new(Barrier::new(2));
            let contender = Arc::clone(&state);
            let start = Arc::clone(&barrier);
            let first = std::thread::spawn(move || {
                start.wait();
                let mut guard = contender.lock().unwrap();
                if stopping {
                    guard.reserve_stop()
                } else {
                    guard.prepare()
                }
            });
            barrier.wait();
            let second = {
                let mut guard = state.lock().unwrap();
                if stopping {
                    guard.close(id, nonce)
                } else {
                    guard.revoke(id).map(|_| ())
                }
            };
            assert_ne!(first.join().unwrap().is_ok(), second.is_ok());
        }
    }
}

#[tokio::test]
async fn explicit_full3_and_stop3_use_separate_authenticated_connections() {
    use super::super::{EnforcerStopClientV3, client::EnforcerFullClientV3};
    let files = tempfile::Builder::new()
        .prefix("f3-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(short_ipc_fixture_root())
        .unwrap();
    let parent = Directory::open_existing(files.path()).unwrap();
    let owned_parent = parent.clone();
    let installation = Uuid::new_v4();
    let server = tokio::spawn(async move {
        run_internal_full3_fresh(&owned_parent, "role", installation, 2).await
    });
    let expires = Instant::now() + Duration::from_secs(6);
    let (root, full) = loop {
        if let Some(root) = parent.persistent_child("role", false).unwrap()
            && let Ok(full) = EnforcerFullClientV3::connect_existing(&root, 2).await
        {
            break (root, full);
        }
        assert!(
            Instant::now() < expires,
            "real Full3 listener did not become available"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    full.require_admission_ready().await.unwrap();
    let auth = std::fs::read(root.path().join("auth")).unwrap();
    let genesis = std::fs::read(root.path().join("retirement/genesis.json")).unwrap();
    assert!(
        run_internal_full3_fresh(&parent, "role", installation, 2)
            .await
            .is_err()
    );
    assert_eq!(auth, std::fs::read(root.path().join("auth")).unwrap());
    assert_eq!(
        genesis,
        std::fs::read(root.path().join("retirement/genesis.json")).unwrap()
    );
    let stops = EnforcerStopClientV3::connect_or_spawn_for_recovery(
        &root,
        &std::env::current_exe().unwrap(),
        2,
    )
    .await
    .unwrap();
    let status = stops.status().await.unwrap();
    assert!(!status.admission_available());
    assert_eq!(status.retirement_count(), None);
    assert_eq!(status.ledger_state_reason(), "recovery_transport_only");
    full.shutdown_if_idle().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(stops);
    drop(full);
}

#[tokio::test]
async fn explicit_full3_refuses_a_live_legacy_role_without_fallback_or_rekey() {
    use super::super::{EnforcerClient, client::EnforcerFullClientV3};
    let files = tempfile::Builder::new()
        .prefix("f3l-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(short_ipc_fixture_root())
        .unwrap();
    let parent = Directory::open_existing(files.path()).unwrap();
    let root = parent.persistent_child("legacy", true).unwrap().unwrap();
    capability(&root, true).unwrap();
    let owned = root.clone();
    let server = tokio::spawn(async move { super::super::run_internal_enforcer(&owned, 2).await });
    let expires = Instant::now() + Duration::from_secs(3);
    while crate::broker::discovery::load_endpoint(&root)
        .unwrap()
        .is_none()
    {
        assert!(Instant::now() < expires);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let auth = std::fs::read(root.path().join("auth")).unwrap();
    assert!(
        EnforcerFullClientV3::connect_existing(&root, 2)
            .await
            .is_err()
    );
    assert_eq!(auth, std::fs::read(root.path().join("auth")).unwrap());
    assert!(
        root.persistent_child("retirement", false)
            .unwrap()
            .is_none()
    );
    let legacy = EnforcerClient::connect_or_spawn(&root, &std::env::current_exe().unwrap(), 2)
        .await
        .unwrap();
    legacy.shutdown_if_idle().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
