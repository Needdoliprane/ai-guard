//! Software-only native regressions. Each parent owns one libtest child and
//! private filesystem namespace. The unavailable-runtime cases have no
//! executable; the named reconnect case uses the existing Rust runtime fixture.
//! Native threads/contexts and IPC are real.
use super::*;
use crate::broker::journal::{JournalScopeV1, RevisionRefV1};
use crate::broker::{clock::checked_deadline, discovery::load_endpoint};
use crate::control::{
    ArmReceiptV1, ArmSubjectV1, ArmV1, NetworkV1, RestartPolicyV1, RuntimeInspectionV1,
    RuntimeKindV1, SecurityPresetV1,
    session_control::{
        LegacyConfirmedPlanV1, LegacyNamedImportArmRequestV1, LegacyNamedImportClaimRequestV1,
    },
};
use serde_json::json;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
    sync::{OnceLock, Weak},
};

const CHILD: &str = "broker::enforcer::server::prospective::tests::native_legacy_prospective_child";
const WAIT: Duration = Duration::from_secs(8);
static OBSERVED: OnceLock<Mutex<Weak<State>>> = OnceLock::new();
static PAUSE_RESERVE_ACK: AtomicBool = AtomicBool::new(false);
static PAUSE_FAILURE_PUBLICATION: AtomicBool = AtomicBool::new(false);
static INVALIDATION_REACHED: AtomicBool = AtomicBool::new(false);
static RUNTIME_REJECTIONS: AtomicU64 = AtomicU64::new(0);
pub(super) fn record_runtime_rejection() {
    RUNTIME_REJECTIONS.fetch_add(1, Ordering::SeqCst);
}
pub(super) fn after_runtime_invalidation() {
    if PAUSE_FAILURE_PUBLICATION.load(Ordering::SeqCst) {
        INVALIDATION_REACHED.store(true, Ordering::SeqCst);
        while PAUSE_FAILURE_PUBLICATION.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
pub(super) async fn before_reserve_ack() {
    while PAUSE_RESERVE_ACK.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
pub(super) fn observe_state(state: &Arc<State>) {
    *OBSERVED
        .get_or_init(|| Mutex::new(Weak::new()))
        .lock()
        .unwrap() = Arc::downgrade(state);
}
async fn wait_for(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("causal native fixture wait");
}
async fn state() -> Arc<State> {
    wait_for(|| {
        OBSERVED
            .get()
            .is_some_and(|slot| slot.lock().unwrap().upgrade().is_some())
    })
    .await;
    OBSERVED.get().unwrap().lock().unwrap().upgrade().unwrap()
}
fn private_directory(path: &Path) -> Directory {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    Directory::open_existing(&path.canonicalize().unwrap()).unwrap()
}
async fn rpc(root: &Directory, request: Request) -> Response {
    let endpoint = load_endpoint(root).unwrap().unwrap();
    let endpoints = root.persistent_child("endpoints", false).unwrap().unwrap();
    let mut stream = endpoint.connect_checked(&endpoints).await.unwrap();
    let challenge: Challenge = read_frame(&mut stream, 1024, WAIT).await.unwrap();
    assert_eq!(challenge.epoch, endpoint.epoch());
    write_frame(
        &mut stream,
        &Authentication {
            proof: proof(&capability(root, false).unwrap(), &challenge),
        },
        1024,
        WAIT,
    )
    .await
    .unwrap();
    write_frame(&mut stream, &request, MAX_FRAME, WAIT)
        .await
        .unwrap();
    read_frame(&mut stream, MAX_FRAME, WAIT).await.unwrap()
}
async fn rpc_without_reading_reply(root: &Directory, request: Request) {
    let endpoint = load_endpoint(root).unwrap().unwrap();
    let endpoints = root.persistent_child("endpoints", false).unwrap().unwrap();
    let mut stream = endpoint.connect_checked(&endpoints).await.unwrap();
    let challenge: Challenge = read_frame(&mut stream, 1024, WAIT).await.unwrap();
    assert_eq!(challenge.epoch, endpoint.epoch());
    write_frame(
        &mut stream,
        &Authentication {
            proof: proof(&capability(root, false).unwrap(), &challenge),
        },
        1024,
        WAIT,
    )
    .await
    .unwrap();
    write_frame(&mut stream, &request, MAX_FRAME, WAIT)
        .await
        .unwrap();
    // Deliberately drop the authenticated connection after the request is
    // written. The caller observes the durable effect through the fixture,
    // then retries the identical request on a new authenticated connection.
}
async fn reserve(root: &Directory, registration: &RegistrationV1) -> ProspectiveReservationV1 {
    match rpc(
        root,
        Request::ReserveProspective {
            prospective: Box::new(registration.prospective().unwrap()),
            generation: 0,
            replay_only: false,
        },
    )
    .await
    {
        Response::ProspectiveReserved { reservation } => reservation,
        _ => panic!("fresh native owner must precede the Arm exchange"),
    }
}
async fn restore(root: &Directory, registration: &RegistrationV1) -> Response {
    tokio::time::timeout(WAIT, async {
        loop {
            let reply = rpc(
                root,
                Request::Register {
                    registration: Box::new(registration.clone()),
                    generation: 0,
                    observe_only: true,
                },
            )
            .await;
            if !matches!(
                reply,
                Response::Error {
                    code: EnforcerError::NotReady
                }
            ) {
                return reply;
            }
        }
    })
    .await
    .expect("native restoration installation")
}
fn expect_error(response: Response, wanted: EnforcerError) {
    assert!(matches!(response, Response::Error { code } if code == wanted));
}
fn expect_refusal(response: Response) {
    assert!(matches!(response, Response::Error { .. }));
}

// Same validated unavailable-invocation shape as the historical S0 oracle.
// Absence is checked before any native child spawn; it is not a fake Ready.
pub(super) fn registration(root: &Path) -> RegistrationV1 {
    let missing = root.join("never-created-runtime");
    let authority: crate::runtime::RuntimeAuthority = serde_json::from_value(json!({
        "authority_id":Uuid::new_v4(),"kind":"docker","endpoint":format!("unix://{}",root.join("missing.sock").display()),"engine_id":"synthetic-legacy-not-opened","podman":null,
        "invocation":{"policy_version":2,"kind":"docker","approved_entry":missing,"canonical_program":missing,
        "file_identity":{"device":1,"inode":2,"owner":0,"group":0,"mode":493,"length":1,"sha256":"b".repeat(64)},"environment":{},"cwd":root}
    })).unwrap();
    registration_with_authority(authority)
}

fn registration_with_authority(authority: crate::runtime::RuntimeAuthority) -> RegistrationV1 {
    let inspection = RuntimeInspectionV1 {
        authority_id: authority.authority_id.to_string(),
        runtime_kind: RuntimeKindV1::Docker,
        container_id: "a".repeat(64),
        stable_name: "aig-legacy-unavailable".into(),
        volume_name: "aig-legacy-unavailable-work".into(),
        image_id: format!("sha256:{}", "b".repeat(64)),
        spec_hash: "d".repeat(64),
        cpu_millis: 1000,
        memory_bytes: 1024 * 1024 * 1024,
        pids_limit: 256,
        network: NetworkV1::None,
        security_preset: SecurityPresetV1::FixedSandboxV2,
        restart_policy: RestartPolicyV1::No,
    };
    let owned = serde_json::from_value(json!({
        "handle":{"authority_fingerprint":authority.fingerprint(),"session":Uuid::new_v4(),"operation":Uuid::new_v4(),"id":inspection.container_id,"name":inspection.stable_name,"configuration_fingerprint_version":2},
        "observation":{"id":inspection.container_id,"name":inspection.stable_name,"image_id":inspection.image_id,"restart_count":0,
        "process":{"status":"created","running":false,"started_at":null,"finished_at":null,"exit_code":null,"oom_killed":false},"allocation":{"cpu":{"quota":100000,"period":100000},"memory_bytes":1024_u64*1024*1024,"pids_limit":256},"configuration_fingerprint":"c".repeat(64),"configuration_fingerprint_version":2,"observed_at":"2026-11-17T00:00:00Z","diagnostics":[]}
    })).unwrap();
    let clock = NativeHostClock::new().unwrap().sample().unwrap();
    let arm = ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: Uuid::new_v4().to_string(),
        attempt_id: Uuid::new_v4().to_string(),
        expected_revision: 1,
        intent_hash: "e".repeat(64),
        subject: ArmSubjectV1::Worker,
        inspection,
        authority_binding_hash: authority.fingerprint(),
        configuration_fingerprint_version: 2,
        configuration_fingerprint: "c".repeat(64),
        deadline_ns_decimal: checked_deadline(clock.continuous_ns().unwrap(), 3600)
            .unwrap()
            .to_string(),
        clock,
        timeout_seconds: 3600,
    };
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
        "a".repeat(64),
        "tenant-legacy-fixture".into(),
        arm,
        receipt,
        authority,
        owned,
    )
    .unwrap()
}

pub(super) fn named_import_registration(root: &Path) -> (RegistrationV1, NamedImportArmBirthV1) {
    named_import_from(registration(root))
}

fn named_import_from(base: RegistrationV1) -> (RegistrationV1, NamedImportArmBirthV1) {
    let (registration, birth, _) = named_import_from_with_plan(base);
    (registration, birth)
}

fn named_import_from_with_plan(
    base: RegistrationV1,
) -> (RegistrationV1, NamedImportArmBirthV1, LegacyConfirmedPlanV1) {
    let scope = JournalScopeV1::new(
        Uuid::new_v4(),
        &base.tenant_id,
        "https://broker-fixture.invalid",
        Uuid::new_v4(),
        "fixture",
    )
    .unwrap();
    let plan = LegacyConfirmedPlanV1 {
        version: 1,
        binding_ref: Uuid::new_v4(),
        control_namespace_uuid: scope.control_namespace_uuid().to_string(),
        operation_id: Uuid::new_v4(),
        session_id: base.owned.handle.session_id(),
        launch_target_id: Uuid::new_v4(),
        target_revision: 1,
        broker_profile_id: scope.profile_id().to_owned(),
        launch_spec_hash: "4".repeat(64),
        expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
    };
    let birth = NamedImportArmBirthV1::from_plan(
        &scope,
        &plan,
        &RevisionRefV1::new(4, &"5".repeat(64)).unwrap(),
    )
    .unwrap();
    let helper_id = base.owned.handle.operation_id().to_string();
    let mut arm = base.arm.clone();
    arm.operation_id = LegacyNamedImportArmRequestV1::operation_id(
        plan.binding_ref,
        arm.attempt_id.parse().unwrap(),
    );
    arm.expected_revision = 2;
    let admission_receipt = crate::control::MutationReceiptV1 {
        operation_id: plan.operation_id.to_string(),
        worker_id: Some(arm.worker_id.clone()),
        revision: Some(1),
        input_hash: "1".repeat(64),
        audit_seq: 1,
        audit_hash: "2".repeat(64),
    };
    arm.subject = ArmSubjectV1::Helper {
        helper_id,
        purpose: crate::control::HelperPurposeV1::Import,
        policy_version: 1,
        permission: Box::new(crate::control::HelperPermissionV1::Admission {
            receipt: admission_receipt,
        }),
    };
    arm.timeout_seconds = 120;
    arm.deadline_ns_decimal =
        checked_deadline(arm.clock.continuous_ns().unwrap(), arm.timeout_seconds)
            .unwrap()
            .to_string();
    let receipt = ArmReceiptV1 {
        version: 1,
        arm_id: arm.operation_id.clone(),
        worker_id: arm.worker_id.clone(),
        attempt_id: arm.attempt_id.clone(),
        input_hash: arm.input_hash().unwrap(),
        control_namespace_uuid: plan.control_namespace_uuid.clone(),
        audit_seq: 2,
        audit_hash: "3".repeat(64),
    };
    let registration = RegistrationV1::new(
        hex::encode(Sha256::digest(scope.gateway_origin().as_bytes())),
        base.tenant_id,
        arm,
        receipt,
        base.authority,
        base.owned,
    )
    .unwrap();
    (registration, birth, plan)
}

async fn named_prepare_request(
    native: &Arc<State>,
    registration: &RegistrationV1,
    plan: &LegacyConfirmedPlanV1,
    reservation: &ProspectiveReservationV1,
    custody_nonce: Uuid,
    original_exchange_nonce: Uuid,
) -> NamedImportPrepareV1 {
    let arm_id = Uuid::parse_str(&registration.arm.operation_id).unwrap();
    let helper_id = match &registration.arm.subject {
        ArmSubjectV1::Helper {
            helper_id,
            purpose: crate::control::HelperPurposeV1::Import,
            permission,
            ..
        } if matches!(
            permission.as_ref(),
            crate::control::HelperPermissionV1::Admission { .. }
        ) =>
        {
            helper_id.clone()
        }
        _ => panic!("fixture must retain its exact Import helper Arm"),
    };
    let claim = LegacyNamedImportClaimRequestV1 {
        version: 1,
        binding_ref: plan.binding_ref,
        namespace: plan.control_namespace_uuid.clone(),
        claim: crate::control::HelperStartClaimV1 {
            version: 1,
            operation_id: LegacyNamedImportClaimRequestV1::operation_id(plan.binding_ref, arm_id),
            helper_id,
            emitter_id: Uuid::new_v4().to_string(),
            arm_receipt: registration.receipt.clone(),
            clock_check: native.clock.sample().unwrap(),
        },
    };
    let mut request = NamedImportPrepareV1 {
        reservation: reservation.clone(),
        registration_hash: registration.digest().unwrap(),
        generation: reservation.cancel_generation,
        custody_nonce,
        original_exchange_nonce,
        claim,
        receipt: crate::control::MutationReceiptV1 {
            operation_id: String::new(),
            worker_id: None,
            revision: None,
            input_hash: String::new(),
            audit_seq: 0,
            audit_hash: String::new(),
        },
    };
    request.receipt = crate::control::MutationReceiptV1 {
        operation_id: request.claim.claim.operation_id.clone(),
        worker_id: Some(registration.arm.worker_id.clone()),
        revision: None,
        input_hash: request.claim.input_hash().unwrap(),
        audit_seq: registration.receipt.audit_seq + 1,
        audit_hash: "9".repeat(64),
    };
    request
}

async fn reserve_bind_named(
    root: &Directory,
    registration: &RegistrationV1,
    birth: &NamedImportArmBirthV1,
    custody_nonce: Uuid,
) -> (ProspectiveReservationV1, Uuid) {
    let reservation = match rpc(
        root,
        Request::ReserveNamedProspective {
            prospective: Box::new(registration.prospective().unwrap()),
            generation: 0,
            custody_nonce,
            birth: Box::new(birth.clone()),
        },
    )
    .await
    {
        Response::ProspectiveReserved { reservation } => reservation,
        _ => panic!("named reserve must retain its exact prospective target"),
    };
    let original_exchange_nonce = match rpc(
        root,
        Request::BeginNamedProspectiveArmExchange {
            reservation: reservation.clone(),
            generation: 0,
            custody_nonce,
        },
    )
    .await
    {
        Response::ProspectiveArmExchange {
            original_exchange_nonce: Some(nonce),
        } => nonce,
        _ => panic!("named arm exchange must issue one nonce"),
    };
    assert!(matches!(
        rpc(
            root,
            Request::BindNamedProspective {
                reservation: reservation.clone(),
                registration: Box::new(registration.clone()),
                generation: 0,
                original_exchange_nonce,
                custody_nonce,
            },
        )
        .await,
        Response::Registered { value, replayed: false } if value.phase == SlotPhaseV1::Ready
    ));
    (reservation, original_exchange_nonce)
}

async fn exercise(mode: &str, parent: &Directory) {
    if mode == "sealed_catchup_custody" {
        let lease = crate::broker::discovery::EnforcerRoleLeaseV2::create_new(
            parent.create_enforcer_root("sealed").unwrap(),
        )
        .unwrap();
        let root = parent.persistent_child("sealed", false).unwrap().unwrap();
        let owner = RecoveryAttemptOwner::install(
            lease.clone(),
            Some(Arc::new(NativeHostClock::new().unwrap())),
        )
        .unwrap();
        catchup_custody_case(owner, &root).await;
        drop(lease);
        assert!(matches!(
            crate::broker::discovery::EnforcerRoleLeaseV2::acquire_existing(&root),
            Err(crate::broker::local_ipc::IpcError::Conflict)
        ));
        return;
    }
    if mode == "missing_lock" {
        let root = private_directory(&parent.path().join("missing-lock"));
        root.persistent_create("auth", &[0x5a; 32])
            .unwrap()
            .persistent_sync()
            .unwrap();
        root.sync().unwrap();
        assert_eq!(
            run_internal_enforcer(&root, 1).await,
            Err(EnforcerError::UnsafeFilesystem)
        );
        assert!(
            root.persistent_read(".owner.lock", 0, &[])
                .unwrap()
                .is_none()
        );
        assert!(load_endpoint(&root).unwrap().is_none());
        return;
    }
    if mode == "exclusive" {
        let role = RoleLease::create_new(parent.create_enforcer_root("existing").unwrap()).unwrap();
        role.root()
            .persistent_create("auth", &[0x5a; 32])
            .unwrap()
            .persistent_sync()
            .unwrap();
        role.root().sync().unwrap();
        let lock_bytes = fs::read(role.root().path().join(".owner.lock")).unwrap();
        assert!(lock_bytes.is_empty());
        assert!(parent.create_enforcer_root("existing").is_err());
        assert!(
            run_internal_legacy_fresh(parent, "existing", 1, 0, Uuid::new_v4())
                .await
                .is_err()
        );
        assert_eq!(
            fs::read(role.root().path().join(".owner.lock")).unwrap(),
            lock_bytes
        );
        assert_eq!(capability(role.root(), false).unwrap(), [0x5a; 32]);
        return;
    }
    let w = match mode {
        "budget_w1" | "legacy_catchup_custody" => 1,
        "budget_w2" => 2,
        _ => 0,
    };
    let launch_nonce = Uuid::new_v4();
    let fresh_parent = parent.clone();
    let server = tokio::spawn(async move {
        run_internal_legacy_fresh(&fresh_parent, "r", 1, w, launch_nonce).await
    });
    let native = state().await;
    let root = parent.persistent_child("r", false).unwrap().unwrap();
    wait_for(|| load_endpoint(&root).unwrap().is_some()).await;
    let first = registration(root.path());
    if mode == "legacy_catchup_custody" {
        let owner = native.recovery_attempt.as_ref().unwrap().clone();
        let marker = Arc::new(());
        let weak_marker = Arc::downgrade(&marker);
        owner.fixture_pause_attempt(true);
        let current = owner.fixture_schedule_legacy(first);
        wait_for(|| owner.observations().2).await;
        let queued = owner.fixture_schedule_legacy(registration(root.path()));
        owner.fixture_native_panic(marker);
        let weak_cause = owner.fixture_cause_weak();
        owner.fixture_pause_attempt(false);
        wait_for(|| owner.fixture_thread_finished()).await;
        let weak_state = Arc::downgrade(&native);
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        drop(owner);
        drop(native);
        wait_for(|| weak_state.upgrade().is_none()).await;
        assert!(weak_cause.upgrade().is_some());
        assert!(weak_marker.upgrade().is_some());
        assert!(current.upgrade().is_some() && queued.upgrade().is_some());
        assert!(matches!(
            RoleLease::acquire_existing(&root),
            Err(crate::broker::local_ipc::IpcError::Conflict)
        ));
        return;
    }
    if mode == "old_custody" {
        assert!(matches!(
            restore(&root, &first).await,
            Response::Registered { .. }
        ));
        let actor = native
            .slots
            .lock()
            .unwrap()
            .get(first.arm_id())
            .unwrap()
            .clone();
        let marker = Arc::new(());
        let weak_marker = Arc::downgrade(&marker);
        *actor.native_panic.lock().unwrap() = Some(marker);
        let weak_actor = Arc::downgrade(&actor);
        let joined = native.owners.fixture_join_counter();
        wait_for(|| actor.failures.facts().uncollected_io).await;
        let weak_state = Arc::downgrade(&native);
        drop(actor);
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        drop(native);
        wait_for(|| weak_state.upgrade().is_none() && joined.load(Ordering::SeqCst) >= 2).await;
        // Both real joins completed (progress + old STOP). Neither is a
        // Stopped/Q proof and the actual opaque payload remains upper-owned.
        assert!(weak_actor.upgrade().is_some());
        assert!(weak_marker.upgrade().is_some());
        assert!(matches!(
            RoleLease::acquire_existing(&root),
            Err(crate::broker::local_ipc::IpcError::Conflict)
        ));
        return;
    }
    if mode == "cold" {
        assert!(matches!(
            rpc(&root, Request::ShutdownIfIdle {}).await,
            Response::Acknowledged {}
        ));
        server.await.unwrap().unwrap();
        drop(native);
        wait_for(|| OBSERVED.get().unwrap().lock().unwrap().upgrade().is_none()).await;
        let cold_root = root.clone();
        let cold =
            tokio::spawn(async move { run_internal_enforcer_with_budget(&cold_root, 1, 0).await });
        wait_for(|| OBSERVED.get().unwrap().lock().unwrap().upgrade().is_some()).await;
        let native = state().await;
        wait_for(|| {
            load_endpoint(&root)
                .unwrap()
                .is_some_and(|endpoint| endpoint.epoch() == native.epoch)
        })
        .await;
        match rpc(&root, Request::ProspectiveCapabilities {}).await {
            Response::ProspectiveCapabilities { value } => {
                assert_eq!(value.recovery_generation, Some(1));
                assert_eq!(value.fresh_launch_nonce, None);
            }
            _ => panic!("cold native capability"),
        }
        expect_error(
            rpc(
                &root,
                Request::ReserveProspective {
                    prospective: Box::new(first.prospective().unwrap()),
                    generation: 0,
                    replay_only: false,
                },
            )
            .await,
            EnforcerError::NotReady,
        );
        assert!(matches!(
            rpc(&root, Request::ShutdownIfIdle {}).await,
            Response::Acknowledged {}
        ));
        cold.await.unwrap().unwrap();
        return;
    }
    if mode.starts_with("budget_") {
        assert!(matches!(
            rpc(&root, Request::BeginRecovery {}).await,
            Response::RecoveryBegun { generation: 1 }
        ));
        let second = registration(root.path());
        let third = registration(root.path());
        assert!(matches!(
            restore(&root, &first).await,
            Response::Registered { .. }
        ));
        let second_reply = restore(&root, &second).await;
        if w == 2 {
            assert!(matches!(second_reply, Response::Registered { .. }));
        } else {
            expect_error(second_reply, EnforcerError::Capacity);
        }
        expect_error(
            rpc(
                &root,
                Request::Register {
                    registration: Box::new(third),
                    generation: 0,
                    observe_only: true,
                },
            )
            .await,
            EnforcerError::Capacity,
        );
        assert_eq!(native.admission.lock().unwrap().entries.len(), w as usize);
        assert!(native.recovery_incomplete.load(Ordering::SeqCst));
        wait_for(|| native.recovery_attempt.as_ref().unwrap().observations().0 > 0).await;
        assert_eq!(native.owned_active().unwrap(), Some(w));
        assert!(native.recovery_attempt.is_some());
        server.abort();
        return;
    }
    if mode == "fence_race" {
        PAUSE_RESERVE_ACK.store(true, Ordering::SeqCst);
        let request_root = root.clone();
        let request_first = first.clone();
        let reserve_request = tokio::spawn(async move {
            rpc(
                &request_root,
                Request::ReserveProspective {
                    prospective: Box::new(request_first.prospective().unwrap()),
                    generation: 0,
                    replay_only: false,
                },
            )
            .await
        });
        wait_for(|| native.owned_active().unwrap() == Some(1)).await;
        assert!(matches!(
            rpc(&root, Request::BeginRecovery {}).await,
            Response::RecoveryBegun { generation: 1 }
        ));
        assert!(
            native
                .prospective
                .lock()
                .unwrap()
                .values()
                .all(|owner| owner.state.lock().unwrap().stop_reserved)
        );
        PAUSE_RESERVE_ACK.store(false, Ordering::SeqCst);
        expect_error(reserve_request.await.unwrap(), EnforcerError::NotReady);
        server.abort();
        return;
    }
    if mode == "clock_before_reserve_ack" {
        let mut expiring = first;
        let clock = native.clock.sample().unwrap();
        let deadline = checked_deadline(clock.continuous_ns().unwrap(), 2).unwrap();
        expiring.arm.deadline_ns_decimal = deadline.to_string();
        expiring.arm.clock = clock;
        expiring.arm.timeout_seconds = 2;
        expiring.receipt.input_hash = expiring.arm.input_hash().unwrap();
        let target = expiring.prospective().unwrap();

        PAUSE_RESERVE_ACK.store(true, Ordering::SeqCst);
        let request_root = root.clone();
        let reserve_request = tokio::spawn(async move {
            rpc(
                &request_root,
                Request::ReserveProspective {
                    prospective: Box::new(target),
                    generation: 0,
                    replay_only: false,
                },
            )
            .await
        });
        wait_for(|| native.owned_active().unwrap() == Some(1)).await;
        let pending = native.prospective.lock().unwrap()[expiring.arm_id()].clone();
        expiring
            .arm
            .validate_clock_check(&native.clock.sample().unwrap())
            .expect("native installation observed before the deadline");

        // Prevent the native loop from publishing its own deadline STOP
        // until the reservation has answered. Otherwise either guard could
        // explain NotReady. The child watchdog bounds a blocked reply.
        let pending_state = pending.state.lock().unwrap();
        assert!(!pending_state.stop_reserved);
        wait_for(|| native.clock.sample().unwrap().continuous_ns().unwrap() >= deadline).await;
        assert!(!reserve_request.is_finished());
        PAUSE_RESERVE_ACK.store(false, Ordering::SeqCst);
        expect_error(reserve_request.await.unwrap(), EnforcerError::NotReady);
        drop(pending_state);
        server.abort();
        return;
    }
    if mode == "named_reconnect" {
        // Every rpc() opens a fresh authenticated IPC connection. A owns the
        // reservation custody; B represents a reconnect with a different
        // custody nonce while the same guardian and native owner remain live.
        let runtime = crate::runtime::Runtime::with_program(
            crate::runtime::RuntimeKind::Docker,
            &parent.path().join("engine"),
        )
        .unwrap();
        let managed = ManagedRuntime::connect_docker_with_runtime(
            Uuid::new_v4(),
            "unix:///fixture/docker.sock",
            runtime,
        )
        .await
        .unwrap();
        let (first, birth) =
            named_import_from(registration_with_authority(managed.binding().clone()));
        let custody_a = Uuid::new_v4();
        let custody_b = Uuid::new_v4();
        let reserved = match rpc(
            &root,
            Request::ReserveNamedProspective {
                prospective: Box::new(first.prospective().unwrap()),
                generation: 0,
                custody_nonce: custody_a,
                birth: Box::new(birth.clone()),
            },
        )
        .await
        {
            Response::ProspectiveReserved { reservation } => reservation,
            _ => panic!("A's named reserve must retain the native target"),
        };
        let pending = native
            .pending(&reserved, LegacyProspectiveRailV1::Named(custody_a))
            .unwrap();
        assert!(pending.runtime_installed.load(Ordering::SeqCst));
        let original_identity = pending.native_identity.lock().unwrap().prospective;
        assert!(original_identity.is_some());

        // Reconnected B cannot claim the same target through either the
        // named protocol with a different custody nonce or any legacy port.
        expect_error(
            rpc(
                &root,
                Request::ReserveNamedProspective {
                    prospective: Box::new(first.prospective().unwrap()),
                    generation: 0,
                    custody_nonce: custody_b,
                    birth: Box::new(birth.clone()),
                },
            )
            .await,
            EnforcerError::Conflict,
        );
        let mut replaced_birth = serde_json::to_value(&birth).unwrap();
        replaced_birth["arm_pending_head"]["hash"] = json!("6".repeat(64));
        expect_error(
            rpc(
                &root,
                Request::ReserveNamedProspective {
                    prospective: Box::new(first.prospective().unwrap()),
                    generation: 0,
                    custody_nonce: custody_a,
                    birth: Box::new(serde_json::from_value(replaced_birth).unwrap()),
                },
            )
            .await,
            EnforcerError::Conflict,
        );
        expect_error(
            rpc(
                &root,
                Request::BeginNamedProspectiveArmExchange {
                    reservation: reserved.clone(),
                    generation: 0,
                    custody_nonce: custody_b,
                },
            )
            .await,
            EnforcerError::Conflict,
        );
        expect_error(
            rpc(
                &root,
                Request::BindNamedProspective {
                    reservation: reserved.clone(),
                    registration: Box::new(first.clone()),
                    generation: 0,
                    original_exchange_nonce: Uuid::new_v4(),
                    custody_nonce: custody_b,
                },
            )
            .await,
            EnforcerError::Conflict,
        );
        expect_refusal(
            rpc(
                &root,
                Request::ReserveProspective {
                    prospective: Box::new(first.prospective().unwrap()),
                    generation: 0,
                    replay_only: false,
                },
            )
            .await,
        );
        expect_refusal(
            rpc(
                &root,
                Request::BeginProspectiveArmExchange {
                    reservation: reserved.clone(),
                    generation: 0,
                },
            )
            .await,
        );
        expect_refusal(
            rpc(
                &root,
                Request::BindProspective {
                    reservation: reserved.clone(),
                    registration: Box::new(first.clone()),
                    generation: 0,
                    observe_only: false,
                    original_exchange_nonce: Some(Uuid::new_v4()),
                },
            )
            .await,
        );
        expect_refusal(
            rpc(
                &root,
                Request::CancelProspective {
                    reservation: reserved.clone(),
                    generation: 0,
                },
            )
            .await,
        );
        expect_refusal(
            rpc(
                &root,
                Request::Register {
                    registration: Box::new(first.clone()),
                    generation: 0,
                    observe_only: true,
                },
            )
            .await,
        );
        expect_refusal(
            rpc(
                &root,
                Request::Register {
                    registration: Box::new(first.clone()),
                    generation: 0,
                    observe_only: false,
                },
            )
            .await,
        );

        // In particular, legacy Register must not turn the retry into the
        // guardian-wide recovery path or consume A's one-shot exchange.
        assert!(!pending.state.lock().unwrap().stop_reserved);
        assert!(matches!(
            pending.state.lock().unwrap().exchange,
            ExchangePhase::NotIssued
        ));

        let original_exchange_nonce = match rpc(
            &root,
            Request::BeginNamedProspectiveArmExchange {
                reservation: reserved.clone(),
                generation: 0,
                custody_nonce: custody_a,
            },
        )
        .await
        {
            Response::ProspectiveArmExchange {
                original_exchange_nonce: Some(nonce),
            } => nonce,
            _ => panic!("A retains the original one-shot Begin after B's reconnect"),
        };
        expect_error(
            rpc(
                &root,
                Request::BindNamedProspective {
                    reservation: reserved.clone(),
                    registration: Box::new(first.clone()),
                    generation: 0,
                    original_exchange_nonce,
                    custody_nonce: custody_b,
                },
            )
            .await,
            EnforcerError::Conflict,
        );
        assert!(!pending.state.lock().unwrap().stop_reserved);
        assert!(matches!(
            rpc(
                &root,
                Request::BindNamedProspective {
                    reservation: reserved,
                    registration: Box::new(first),
                    generation: 0,
                    original_exchange_nonce,
                    custody_nonce: custody_a,
                },
            )
            .await,
            Response::Registered { value, replayed: false }
                if value.phase == SlotPhaseV1::Ready
        ));
        assert_eq!(
            pending.native_identity.lock().unwrap().bound,
            original_identity
        );

        // The global historical recovery fence closes admission but must not
        // commandeer a named prospect or its bound slot. Its native deadline
        // continues to be owned by the same guardian independently.
        assert!(matches!(
            rpc(&root, Request::BeginRecovery {}).await,
            Response::RecoveryBegun { generation: 1 }
        ));
        assert!(!pending.state.lock().unwrap().stop_reserved);
        assert!(
            pending
                .state
                .lock()
                .unwrap()
                .bound
                .as_ref()
                .is_some_and(|actor| !actor.stop.load(Ordering::SeqCst))
        );
        let calls = fs::read_to_string(parent.path().join("log")).unwrap();
        assert_eq!(calls.lines().count(), 2);
        assert!(calls.lines().all(|line| line.starts_with("info ")));
        let facts = pending
            .named_stop_anchor
            .as_ref()
            .unwrap()
            .fixture_stop_facts(&pending.prospective);
        let mut wrong = serde_json::to_value(&facts).unwrap();
        wrong["command_input_hash"] = json!("4".repeat(64));
        let endpoint = NamedImportStopEndpoint::pin(&root, None).unwrap();
        let key = capability(&root, false).unwrap();
        assert_eq!(
            endpoint.exchange(&wrong, &key, || Ok(())).await,
            Err(EnforcerError::Conflict)
        );
        assert_eq!(
            endpoint
                .exchange(&facts, &key, || Err(EnforcerError::NotReady))
                .await,
            Err(EnforcerError::NotReady)
        );
        assert!(!pending.state.lock().unwrap().stop_reserved);
        endpoint.exchange(&facts, &key, || Ok(())).await.unwrap();
        endpoint.exchange(&facts, &key, || Ok(())).await.unwrap();
        let retained = native
            .pending(
                &pending.reservation,
                LegacyProspectiveRailV1::Named(custody_a),
            )
            .unwrap();
        assert!(Arc::ptr_eq(&retained, &pending));
        let guarded = retained.state.lock().unwrap();
        assert!(guarded.stop_reserved);
        let bound = guarded.bound.as_ref().unwrap();
        assert!(bound.stop.load(Ordering::SeqCst));
        assert_eq!(
            bound.registration.arm.deadline_ns_decimal,
            pending.prospective.arm.deadline_ns_decimal
        );
        drop(guarded);
        let key = pending.fixture_exit_key();
        wait_for(|| native.owners.keyed_finished(key)).await;
        server.abort();
        return;
    }
    if mode.starts_with("named_prepare_") {
        // The only runtime is the existing software fixture. The Prepare and
        // STOP assertions travel through the authenticated private endpoints;
        // the fixture does not start a container or invoke an external engine.
        let runtime = crate::runtime::Runtime::with_program(
            crate::runtime::RuntimeKind::Docker,
            &parent.path().join("engine"),
        )
        .unwrap();
        let managed = ManagedRuntime::connect_docker_with_runtime(
            Uuid::new_v4(),
            "unix:///fixture/docker.sock",
            runtime,
        )
        .await
        .unwrap();
        let (mut registration, birth, plan) =
            named_import_from_with_plan(registration_with_authority(managed.binding().clone()));
        if mode == "named_prepare_expired" {
            let clock = native.clock.sample().unwrap();
            registration.arm.timeout_seconds = 1;
            registration.arm.clock = clock;
            registration.arm.deadline_ns_decimal =
                checked_deadline(registration.arm.clock.continuous_ns().unwrap(), 1)
                    .unwrap()
                    .to_string();
            registration.receipt.input_hash = registration.arm.input_hash().unwrap();
            registration.validate().unwrap();
        }
        let custody_nonce = Uuid::new_v4();
        let (reservation, original_exchange_nonce) =
            reserve_bind_named(&root, &registration, &birth, custody_nonce).await;
        let pending = native
            .pending(&reservation, LegacyProspectiveRailV1::Named(custody_nonce))
            .unwrap();
        assert!(pending.runtime_installed.load(Ordering::SeqCst));

        let prepare = named_prepare_request(
            &native,
            &registration,
            &plan,
            &reservation,
            custody_nonce,
            original_exchange_nonce,
        )
        .await;

        if mode == "named_prepare_stop_first" {
            let facts = pending
                .named_stop_anchor
                .as_ref()
                .unwrap()
                .fixture_stop_facts(&pending.prospective);
            let endpoint = NamedImportStopEndpoint::pin(&root, None).unwrap();
            let key = capability(&root, false).unwrap();
            endpoint.exchange(&facts, &key, || Ok(())).await.unwrap();
            assert!(pending.state.lock().unwrap().stop_reserved);
            expect_error(
                rpc(
                    &root,
                    Request::PrepareNamedImport {
                        request: Box::new(prepare),
                    },
                )
                .await,
                EnforcerError::NotReady,
            );
            let bound = pending
                .state
                .lock()
                .unwrap()
                .bound
                .as_ref()
                .unwrap()
                .clone();
            assert!(!bound.state.lock().unwrap().prepared_once);
            let key = pending.fixture_exit_key();
            wait_for(|| native.owners.keyed_finished(key)).await;
            server.abort();
            return;
        }

        if mode == "named_prepare_expired" {
            let deadline = registration.arm.deadline_ns_decimal.parse::<u64>().unwrap();
            wait_for(|| native.clock.sample().unwrap().continuous_ns().unwrap() >= deadline).await;
            expect_error(
                rpc(
                    &root,
                    Request::PrepareNamedImport {
                        request: Box::new(prepare),
                    },
                )
                .await,
                EnforcerError::NotReady,
            );
            let bound = pending
                .state
                .lock()
                .unwrap()
                .bound
                .as_ref()
                .unwrap()
                .clone();
            assert!(!bound.state.lock().unwrap().prepared_once);
            let key = pending.fixture_exit_key();
            wait_for(|| native.owners.keyed_finished(key)).await;
            server.abort();
            return;
        }

        assert_eq!(mode, "named_prepare_success");
        let valid_wire = serde_json::to_value(&prepare).unwrap();
        let mut invalid: NamedImportPrepareV1 = serde_json::from_value(valid_wire.clone()).unwrap();
        invalid.receipt.audit_hash = "a".repeat(64);
        expect_error(
            rpc(
                &root,
                Request::PrepareNamedImport {
                    request: Box::new(invalid),
                },
            )
            .await,
            EnforcerError::Conflict,
        );
        assert!(
            !pending
                .state
                .lock()
                .unwrap()
                .bound
                .as_ref()
                .unwrap()
                .state
                .lock()
                .unwrap()
                .prepared_once
        );

        // Model the caller losing the ACK after the guardian commits the
        // one-shot Prepare. The second connection sends byte-equivalent input.
        let dropped_request: Request = serde_json::from_value(json!({
            "kind": "prepare_named_import",
            "request": valid_wire.clone(),
        }))
        .unwrap();
        let retry: Request = serde_json::from_value(json!({
            "kind": "prepare_named_import",
            "request": valid_wire,
        }))
        .unwrap();
        rpc_without_reading_reply(&root, dropped_request).await;
        wait_for(|| {
            pending
                .state
                .lock()
                .unwrap()
                .bound
                .as_ref()
                .is_some_and(|actor| actor.state.lock().unwrap().prepared_once)
        })
        .await;
        expect_error(rpc(&root, retry).await, EnforcerError::AlreadyPrepared);

        // A successful Prepare never owns or delays STOP. The exact original
        // guardian still accepts the separately authenticated STOP command.
        let facts = pending
            .named_stop_anchor
            .as_ref()
            .unwrap()
            .fixture_stop_facts(&pending.prospective);
        let endpoint = NamedImportStopEndpoint::pin(&root, None).unwrap();
        let key = capability(&root, false).unwrap();
        endpoint.exchange(&facts, &key, || Ok(())).await.unwrap();
        let guarded = pending.state.lock().unwrap();
        assert!(guarded.stop_reserved);
        let bound = guarded.bound.as_ref().unwrap();
        assert!(bound.stop.load(Ordering::SeqCst));
        assert!(bound.state.lock().unwrap().prepared_once);
        drop(guarded);
        let key = pending.fixture_exit_key();
        wait_for(|| native.owners.keyed_finished(key)).await;
        server.abort();
        return;
    }
    if mode == "deadline" {
        let mut expiring = first;
        let clock = native.clock.sample().unwrap();
        expiring.arm.deadline_ns_decimal = checked_deadline(clock.continuous_ns().unwrap(), 1)
            .unwrap()
            .to_string();
        expiring.arm.clock = clock;
        expiring.arm.timeout_seconds = 1;
        expiring.receipt.input_hash = expiring.arm.input_hash().unwrap();
        expiring.validate().unwrap();
        let reservation = reserve(&root, &expiring).await;
        let pending = native
            .pending(&reservation, LegacyProspectiveRailV1::Historical)
            .unwrap();
        // The native target/deadline is already retained. Making its private
        // filesystem unavailable to a new writer cannot suppress that deadline.
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o500)).unwrap();
        wait_for(|| {
            pending.state.lock().unwrap().stop_reserved
                && pending.failures.facts().retained_causes > 0
        })
        .await;
        assert!(pending.runtime_installed.load(Ordering::SeqCst));
        assert_eq!(native.admission.lock().unwrap().entries.len(), 1);
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        server.abort();
        return;
    }
    let reserved = reserve(&root, &first).await;
    let pending = native
        .pending(&reserved, LegacyProspectiveRailV1::Historical)
        .unwrap();
    assert!(pending.runtime_installed.load(Ordering::SeqCst));
    let original_thread = native.owners.keyed_thread_id(pending.key).unwrap();
    let original_identity = pending.native_identity.lock().unwrap().prospective.unwrap();
    assert_eq!(original_thread, original_identity.0);
    assert_eq!(native.owned_active().unwrap(), Some(1));
    if mode == "cancel_generation" || mode == "replay_generation" {
        if mode == "cancel_generation" {
            assert!(matches!(
                rpc(
                    &root,
                    Request::CancelProspective {
                        reservation: reserved.clone(),
                        generation: 10
                    }
                )
                .await,
                Response::Acknowledged {}
            ));
        } else {
            assert!(matches!(
                rpc(
                    &root,
                    Request::ReserveProspective {
                        prospective: Box::new(first.prospective().unwrap()),
                        generation: 10,
                        replay_only: true
                    }
                )
                .await,
                Response::ProspectiveReserved { .. }
            ));
        }
        match rpc(
            &root,
            Request::BindProspective {
                reservation: reserved,
                registration: Box::new(first),
                generation: 0,
                observe_only: true,
                original_exchange_nonce: None,
            },
        )
        .await
        {
            Response::Registered { value, .. } => {
                assert_eq!(value.generation, 10);
                assert!(value.prepared_once);
            }
            _ => panic!("restrictive Bind must retain admitted Cancel generation"),
        }
        assert_eq!(pending.state.lock().unwrap().current_generation, 10);
        assert_eq!(pending.reservation.cancel_generation, 0);
        server.abort();
        return;
    }
    if matches!(
        mode,
        "prospective_custody" | "bound_custody" | "prepare_invalidation"
    ) {
        if mode != "prospective_custody" {
            assert!(matches!(
                rpc(
                    &root,
                    Request::BindProspective {
                        reservation: reserved.clone(),
                        registration: Box::new(first.clone()),
                        generation: 0,
                        observe_only: true,
                        original_exchange_nonce: None
                    }
                )
                .await,
                Response::Registered { .. }
            ));
        }
        let marker = Arc::new(());
        let weak_marker = Arc::downgrade(&marker);
        if mode == "prepare_invalidation" {
            PAUSE_FAILURE_PUBLICATION.store(true, Ordering::SeqCst);
        }
        *pending.native_panic.lock().unwrap() = Some(marker);
        if mode == "prepare_invalidation" {
            wait_for(|| INVALIDATION_REACHED.load(Ordering::SeqCst)).await;
            assert!(!pending.runtime_installed.load(Ordering::SeqCst));
            assert!(!native.owners.failed());
            RUNTIME_REJECTIONS.store(0, Ordering::SeqCst);
            expect_error(
                rpc(
                    &root,
                    Request::Prepare {
                        arm_id: first.arm_id().into(),
                        registration_hash: first.digest().unwrap(),
                        generation: 0,
                    },
                )
                .await,
                EnforcerError::NotReady,
            );
            assert_eq!(RUNTIME_REJECTIONS.load(Ordering::SeqCst), 1);
            // No Ready phase is fabricated. This proves the actual installed
            // guard executes before older phase/health publication, not a
            // positive ManagedRuntime Ready or a successful business Prepare.
            PAUSE_FAILURE_PUBLICATION.store(false, Ordering::SeqCst);
        }
        wait_for(|| native.owners.keyed_finished(pending.key)).await;
        let weak_pending = Arc::downgrade(&pending);
        let weak_cause = Arc::downgrade(&pending.failures);
        let mut handle = native.owners.take_retiring(pending.key).unwrap();
        tokio::task::spawn_blocking(move || handle.join())
            .await
            .unwrap()
            .unwrap();
        let weak_state = Arc::downgrade(&native);
        drop(pending);
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        drop(native);
        wait_for(|| weak_state.upgrade().is_none()).await;
        assert!(weak_pending.upgrade().is_some());
        assert!(weak_cause.upgrade().is_some());
        assert!(weak_marker.upgrade().is_some());
        assert!(matches!(
            RoleLease::acquire_existing(&root),
            Err(crate::broker::local_ipc::IpcError::Conflict)
        ));
        return;
    }
    match mode {
        "capacity" => {
            let second = registration(root.path());
            expect_error(
                rpc(
                    &root,
                    Request::ReserveProspective {
                        prospective: Box::new(second.prospective().unwrap()),
                        generation: 0,
                        replay_only: false,
                    },
                )
                .await,
                EnforcerError::Capacity,
            );
            expect_error(
                rpc(&root, Request::ShutdownIfIdle {}).await,
                EnforcerError::NotIdle,
            );
            match rpc(&root, Request::Status {}).await {
                Response::Status { value } => assert_eq!(value.slot_count(), 1),
                _ => panic!("native status"),
            }
        }
        "lost_begin" => {
            assert!(matches!(
                rpc(
                    &root,
                    Request::BeginProspectiveArmExchange {
                        reservation: reserved.clone(),
                        generation: 0
                    }
                )
                .await,
                Response::ProspectiveArmExchange {
                    original_exchange_nonce: Some(_)
                }
            ));
            assert!(matches!(
                rpc(
                    &root,
                    Request::BeginProspectiveArmExchange {
                        reservation: reserved.clone(),
                        generation: 0
                    }
                )
                .await,
                Response::ProspectiveArmExchange {
                    original_exchange_nonce: None
                }
            ));
            let absent = registration(root.path());
            expect_error(
                rpc(
                    &root,
                    Request::ReserveProspective {
                        prospective: Box::new(absent.prospective().unwrap()),
                        generation: 0,
                        replay_only: true,
                    },
                )
                .await,
                EnforcerError::Conflict,
            );
            expect_error(
                rpc(
                    &root,
                    Request::Register {
                        registration: Box::new(absent),
                        generation: 0,
                        observe_only: false,
                    },
                )
                .await,
                EnforcerError::NotReady,
            );
        }
        "bind_same_context" => {
            let exchange = match rpc(
                &root,
                Request::BeginProspectiveArmExchange {
                    reservation: reserved.clone(),
                    generation: 0,
                },
            )
            .await
            {
                Response::ProspectiveArmExchange {
                    original_exchange_nonce: Some(nonce),
                } => nonce,
                _ => panic!("first exchange"),
            };
            // Cancel only the transport waiter. The genuine Bind mutation and
            // retained native runtime continue without another start_actor.
            let task_root = root.clone();
            let task_first = first.clone();
            let task_reservation = reserved.clone();
            let bind = tokio::spawn(async move {
                rpc(
                    &task_root,
                    Request::BindProspective {
                        reservation: task_reservation,
                        registration: Box::new(task_first),
                        generation: 0,
                        observe_only: false,
                        original_exchange_nonce: Some(exchange),
                    },
                )
                .await
            });
            wait_for(|| pending.native_identity.lock().unwrap().bound.is_some()).await;
            bind.abort();
            let bound = pending.state.lock().unwrap().bound.clone().unwrap();
            assert_eq!(
                pending.native_identity.lock().unwrap().bound,
                Some(original_identity)
            );
            assert_eq!(
                native.owners.keyed_thread_id(pending.key).unwrap(),
                original_thread
            );
            assert!(Arc::ptr_eq(
                &pending.runtime_installed,
                &bound.runtime_installed
            ));
            assert!(Arc::ptr_eq(&pending.failures, &bound.failures));
            assert!(matches!(
                rpc(
                    &root,
                    Request::BindProspective {
                        reservation: reserved.clone(),
                        registration: Box::new(first),
                        generation: 0,
                        observe_only: true,
                        original_exchange_nonce: None
                    }
                )
                .await,
                Response::Registered { replayed: true, .. }
            ));
            assert!(bound.state.lock().unwrap().prepared_once);
            assert!(bound.stop.load(Ordering::SeqCst));
        }
        "fence" => {
            assert!(matches!(
                rpc(&root, Request::BeginRecovery {}).await,
                Response::RecoveryBegun { generation: 1 }
            ));
            assert!(pending.state.lock().unwrap().stop_reserved);
            expect_error(
                rpc(
                    &root,
                    Request::BeginProspectiveArmExchange {
                        reservation: reserved.clone(),
                        generation: 0,
                    },
                )
                .await,
                EnforcerError::NotReady,
            );
            expect_error(
                rpc(
                    &root,
                    Request::BindProspective {
                        reservation: reserved.clone(),
                        registration: Box::new(first.clone()),
                        generation: 0,
                        observe_only: false,
                        original_exchange_nonce: Some(Uuid::new_v4()),
                    },
                )
                .await,
                EnforcerError::NotReady,
            );
            expect_error(
                rpc(
                    &root,
                    Request::Prepare {
                        arm_id: first.arm_id().into(),
                        registration_hash: first.digest().unwrap(),
                        generation: 0,
                    },
                )
                .await,
                EnforcerError::NotReady,
            );
            assert!(matches!(
                rpc(
                    &root,
                    Request::CancelProspective {
                        reservation: reserved,
                        generation: 1
                    }
                )
                .await,
                Response::Acknowledged {}
            ));
            assert_eq!(native.admission.lock().unwrap().entries.len(), 1);
        }
        _ => panic!("unknown native fixture mode"),
    }
    server.abort();
}

async fn catchup_custody_case(owner: RecoveryAttemptOwner, root: &Directory) {
    let marker = Arc::new(());
    let weak_marker = Arc::downgrade(&marker);
    let weak_cause = owner.fixture_cause_weak();
    owner.fixture_pause_attempt(true);
    let current = owner.fixture_schedule_legacy(registration(root.path()));
    wait_for(|| owner.observations().2).await;
    let queued = owner.fixture_schedule_legacy(registration(root.path()));
    owner.fixture_native_panic(marker);
    owner.fixture_pause_attempt(false);
    wait_for(|| owner.fixture_thread_finished()).await;
    drop(owner);
    assert!(weak_marker.upgrade().is_some());
    assert!(weak_cause.upgrade().is_some());
    assert!(current.upgrade().is_some() && queued.upgrade().is_some());
}

#[tokio::test]
#[ignore = "owned software subprocess helper"]
async fn native_legacy_prospective_child() {
    let path = std::env::var_os("AI_GUARD_LEGACY_NATIVE_ROOT").expect("owned test root");
    let mode = std::env::var("AI_GUARD_LEGACY_NATIVE_MODE").expect("owned test mode");
    let root = Directory::open_existing(&Path::new(&path).canonicalize().unwrap()).unwrap();
    exercise(&mode, &root).await;
}
struct OwnedChild(Option<std::process::Child>);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn run_child(mode: &str) {
    let private_tmp = if cfg!(target_os = "macos") {
        "/private/tmp"
    } else {
        "/tmp"
    };
    let temp = tempfile::Builder::new()
        .prefix("d7l-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(private_tmp)
        .unwrap();
    if mode == "named_reconnect" || mode.starts_with("named_prepare_") {
        let source = temp.path().join("engine.rs");
        let program = temp.path().join("engine");
        fs::write(
            &source,
            include_str!("../../../tests/runtime_fixtures/managed.rs"),
        )
        .unwrap();
        let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let compiled = Command::new(compiler)
            .args(["--edition=2024", "--crate-name", "named_reconnect_fixture"])
            .arg(source)
            .arg("-o")
            .arg(&program)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "the Rust runtime fixture must compile"
        );
        let config = program.with_extension("fixture");
        fs::create_dir(&config).unwrap();
        fs::write(
            config.join("AI_GUARD_MANAGED_ROOT"),
            temp.path().as_os_str().as_encoded_bytes(),
        )
        .unwrap();
        fs::write(config.join("AI_GUARD_MANAGED_CHILD"), "success").unwrap();
    }
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD, "--ignored", "--nocapture"])
        .env_clear()
        .env("AI_GUARD_LEGACY_NATIVE_ROOT", temp.path())
        .env("AI_GUARD_LEGACY_NATIVE_MODE", mode)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = OwnedChild(Some(child));
    let expires = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.0.as_mut().unwrap().try_wait().unwrap() {
            let output = child.0.take().unwrap().wait_with_output().unwrap();
            assert!(
                status.success(),
                "owned native child failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            break;
        }
        if Instant::now() >= expires {
            let mut process = child.0.take().unwrap();
            let _ = process.kill();
            let output = process.wait_with_output().unwrap();
            panic!(
                "owned native child timeout: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[test]
fn native_prearm_s1_is_reserved_before_ack_and_counts_idle() {
    run_child("capacity");
}
#[test]
fn lost_begin_and_absent_replay_never_recreate_original() {
    run_child("lost_begin");
}
#[test]
fn named_guardian_custody_survives_authenticated_reconnect() {
    run_child("named_reconnect");
}
#[test]
fn named_import_prepare_receipt_replay_and_stop_are_one_shot() {
    run_child("named_prepare_success");
    run_child("named_prepare_stop_first");
    run_child("named_prepare_expired");
}
#[test]
fn bind_keeps_actual_thread_runtime_key_and_failure_custody() {
    run_child("bind_same_context");
}
#[test]
fn recovery_gate_commits_stop_cohort_and_refuses_original_ports() {
    run_child("fence");
}
#[test]
fn recovery_races_native_ready_and_vetoes_reserve_ack() {
    run_child("fence_race");
}
#[test]
fn fresh_reservation_rechecks_clock_after_installation_before_ack() {
    run_child("clock_before_reserve_ack");
}
#[test]
fn retained_deadline_proceeds_with_writer_unavailable_and_no_waiter() {
    run_child("deadline");
}
#[test]
fn existing_empty_role_is_cold_fenced_after_real_shutdown() {
    run_child("cold");
}
#[test]
fn mkdirat_existing_root_never_adopts_lock_or_auth() {
    run_child("exclusive");
}
#[test]
fn existing_root_missing_lock_is_refused_without_creation() {
    run_child("missing_lock");
}
#[test]
fn unchanged_s1_w2_has_two_owners_and_one_real_catchup() {
    run_child("budget_w2");
}
#[test]
fn unchanged_s1_w1_has_one_owner_surplus_attempt_and_incomplete() {
    run_child("budget_w1");
}
#[test]
fn prospective_panic_payload_survives_native_join_and_listener_loss() {
    run_child("prospective_custody");
}
#[test]
fn bound_panic_payload_survives_native_join_and_listener_loss() {
    run_child("bound_custody");
}
#[test]
fn old_stop_panic_payload_survives_both_real_joins_and_state_loss() {
    run_child("old_custody");
}
#[test]
fn legacy_catchup_panic_keeps_real_current_queue_and_raw_payload() {
    run_child("legacy_catchup_custody");
}
#[test]
fn sealed_catchup_panic_keeps_real_lease_queue_and_raw_payload() {
    run_child("sealed_catchup_custody");
}
#[test]
fn cancel_before_bind_retains_monotone_generation_ten() {
    run_child("cancel_generation");
}
#[test]
fn replay_only_before_bind_retains_monotone_generation_ten() {
    run_child("replay_generation");
}
#[test]
fn native_installation_invalidation_vetoes_prepare_before_failure_publication() {
    run_child("prepare_invalidation");
}
