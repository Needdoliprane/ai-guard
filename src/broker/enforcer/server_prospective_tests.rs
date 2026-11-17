//! Software-only native regressions. Each parent owns one libtest child and
//! private filesystem namespace; the runtime executable does not exist, so no
//! engine/provider command is invoked. Native threads/contexts and IPC are real.
use super::*;
use crate::broker::{clock::checked_deadline, discovery::load_endpoint};
use crate::control::{
    ArmReceiptV1, ArmSubjectV1, ArmV1, NetworkV1, RestartPolicyV1, RuntimeInspectionV1,
    RuntimeKindV1, SecurityPresetV1,
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

// Same validated unavailable-invocation shape as the historical S0 oracle.
// Absence is checked before any native child spawn; it is not a fake Ready.
pub(super) fn registration(root: &Path) -> RegistrationV1 {
    let missing = root.join("never-created-runtime");
    let authority: crate::runtime::RuntimeAuthority = serde_json::from_value(json!({
        "authority_id":Uuid::new_v4(),"kind":"docker","endpoint":format!("unix://{}",root.join("missing.sock").display()),"engine_id":"synthetic-legacy-not-opened","podman":null,
        "invocation":{"policy_version":2,"kind":"docker","approved_entry":missing,"canonical_program":missing,
        "file_identity":{"device":1,"inode":2,"owner":0,"group":0,"mode":493,"length":1,"sha256":"b".repeat(64)},"environment":{},"cwd":root}
    })).unwrap();
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
        let pending = native.pending(&reservation).unwrap();
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
    let pending = native.pending(&reserved).unwrap();
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
