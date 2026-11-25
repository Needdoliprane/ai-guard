//! Real native enforcer/Unix IPC with deliberately unavailable runtime metadata.
//! This covers retained stop intent, not an actor Ready or a Docker/Podman stop.
#![cfg(any(target_os = "linux", target_os = "macos"))]

use super::*;
use crate::{
    broker::{clock::checked_deadline, enforcer::run_internal_enforcer},
    control::{
        ArmReceiptV1, ArmSubjectV1, ArmV1, NetworkV1, RestartPolicyV1, RuntimeInspectionV1,
        RuntimeKindV1, SecurityPresetV1,
    },
    runtime::{OwnedContainerSnapshot, RuntimeAuthority},
};
use serde_json::json;
use std::{
    fs, io,
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    process::{Child, Command},
};

const CHILD_ROOT: &str = "AI_GUARD_TEST_RETAINED_CANCEL_ROOT";
#[cfg(target_os = "macos")]
const PRIVATE_TMP: &str = "/private/tmp";
#[cfg(target_os = "linux")]
const PRIVATE_TMP: &str = "/tmp";

struct FailureEvidence(Option<tempfile::TempDir>);
impl Drop for FailureEvidence {
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Some(directory) = self.0.take()
        {
            eprintln!(
                "private native Cancel diagnostics retained at {}",
                directory.keep().display()
            );
        }
    }
}

struct NativeChild(Child);
impl Drop for NativeChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn unavailable_registration(root: &Path, suffix: &str) -> RegistrationV1 {
    // The program does not exist: reopening must remain NotReady and must not
    // execute any host program. These are explicit test data, not engine facts.
    let missing = root.join("unavailable-runtime");
    let authority: RuntimeAuthority = serde_json::from_value(json!({
        "authority_id": Uuid::new_v4(),
        "kind": "docker",
        "endpoint": format!("unix://{}", root.join("unavailable.sock").display()),
        "engine_id": "synthetic-unavailable-engine",
        "podman": null,
        "invocation": {
            "policy_version": 2,
            "kind": "docker",
            "approved_entry": missing,
            "canonical_program": missing,
            "file_identity": {
                "device": 1, "inode": 2, "owner": 0, "group": 0,
                "mode": 493, "length": 1, "sha256": "b".repeat(64)
            },
            "environment": {}, "cwd": root
        }
    }))
    .unwrap();
    let inspection = RuntimeInspectionV1 {
        authority_id: authority.authority_id.to_string(),
        runtime_kind: RuntimeKindV1::Docker,
        container_id: hex::encode(Sha256::digest(suffix.as_bytes())),
        stable_name: format!("aig-cancel-{suffix}"),
        volume_name: format!("aig-cancel-{suffix}-work"),
        image_id: format!("sha256:{}", "b".repeat(64)),
        spec_hash: "d".repeat(64),
        cpu_millis: 1000,
        memory_bytes: 1024 * 1024 * 1024,
        pids_limit: 256,
        network: NetworkV1::None,
        security_preset: SecurityPresetV1::FixedSandboxV2,
        restart_policy: RestartPolicyV1::No,
    };
    let owned: OwnedContainerSnapshot = serde_json::from_value(json!({
        "handle": {
            "authority_fingerprint": authority.fingerprint(),
            "session": Uuid::new_v4(), "operation": Uuid::new_v4(),
            "id": inspection.container_id, "name": inspection.stable_name,
            "configuration_fingerprint_version": 2
        },
        "observation": {
            "id": inspection.container_id, "name": inspection.stable_name,
            "image_id": inspection.image_id, "restart_count": 0,
            "process": {
                "status": "running", "running": true,
                "started_at": "2026-10-31T00:00:00Z", "finished_at": null,
                "exit_code": null, "oom_killed": false
            },
            "allocation": {
                "cpu": {"quota": 100000, "period": 100000},
                "memory_bytes": 1024_u64 * 1024 * 1024, "pids_limit": 256
            },
            "configuration_fingerprint": "c".repeat(64),
            "configuration_fingerprint_version": 2,
            "observed_at": "2026-10-31T00:00:00Z", "diagnostics": []
        }
    }))
    .unwrap();
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
        "tenant-retained-cancel".into(),
        arm,
        receipt,
        authority,
        owned,
    )
    .unwrap()
}

#[test]
#[ignore = "internal native child, invoked only by the retained Cancel test"]
fn native_cancel_server_child() {
    let root = std::env::var_os(CHILD_ROOT).expect("private test role root");
    let root = Directory::open_existing(Path::new(&root)).unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_internal_enforcer(&root, 8))
        .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn retained_cancel_real_ipc_retries_without_seeding_or_targeting_later_slots() {
    // Leave room for the role/endpoints/generation/socket suffix within macOS's
    // sockaddr_un limit; the default /var/folders temporary path is too long.
    let evidence = FailureEvidence(Some(
        tempfile::Builder::new()
            .prefix("cr-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(PRIVATE_TMP)
            .unwrap(),
    ));
    let base = evidence.0.as_ref().unwrap().path().canonicalize().unwrap();
    let role = base.join("enforcer");
    fs::create_dir(&role).unwrap();
    fs::set_permissions(&role, fs::Permissions::from_mode(0o700)).unwrap();
    let root = Directory::open_existing(&role).unwrap();
    capability(&root, true).unwrap();
    let executable = std::env::current_exe().unwrap();
    let stderr = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(base.join("child.stderr"))
        .unwrap();
    let mut command = Command::new(&executable);
    command
        .args([
            "--exact",
            "broker::enforcer::client::retained_cancel_tests::native_cancel_server_child",
            "--ignored",
        ])
        .env_clear()
        .env(CHILD_ROOT, &role)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr));
    // SAFETY: the child-only hook uses only setrlimit and errno; it neither
    // allocates nor takes a lock after fork. It caps retained stderr at 64KiB.
    unsafe {
        command.pre_exec(|| {
            let limit = libc::rlimit {
                rlim_cur: 64 * 1024,
                rlim_max: 64 * 1024,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
    }
    let mut child = NativeChild(command.spawn().unwrap());
    let expires = Instant::now() + Duration::from_secs(5);
    while load_endpoint(&root).unwrap().is_none() {
        assert!(child.0.try_wait().unwrap().is_none(), "native child exited");
        assert!(Instant::now() < expires, "native child did not publish IPC");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let client = EnforcerClient::connect_or_spawn(&root, &executable, 8)
        .await
        .unwrap();
    assert!(client.status().await.unwrap().healthy());
    let old = unavailable_registration(&base, "old");
    let later = unavailable_registration(&base, "later");
    for (registration, generation) in [(&old, 7), (&later, 1)] {
        // Normal registration waits 44s for an actor that cannot reopen its
        // runtime. Intentionally stop reading after 100ms, then prove by Slots
        // that authentication and reservation succeeded without ObserveOnly.
        assert!(matches!(
            client
                .0
                .request(
                    Request::Register {
                        registration: Box::new(registration.clone()),
                        generation,
                        observe_only: false,
                    },
                    Duration::from_millis(100),
                )
                .await,
            Err(EnforcerError::Unavailable)
        ));
        let reserved = client
            .slot_status(&registration.arm.operation_id)
            .await
            .unwrap();
        assert_eq!(reserved.arm_id, registration.arm.operation_id);
        assert_eq!(reserved.registration_hash, registration.digest().unwrap());
        assert_eq!(reserved.generation, generation);
        assert!(!reserved.prepared_once);
    }
    let generation = CancelGeneration::new();
    assert_eq!(generation.invalidate(), 1);
    let (later_local, _) = client
        .retain_registration(later.clone(), generation.clone(), false, false)
        .unwrap();
    // The real server waits 100ms for this deliberately unavailable actor.
    // Interrupt the caller before its Register acknowledgement: retained intent
    // must already exist, and another exact call must still deliver Cancel.
    assert!(
        tokio::time::timeout(
            Duration::from_millis(1),
            client.cancel_retained(old.clone(), generation.clone()),
        )
        .await
        .is_err()
    );
    let old_local = Arc::clone(
        client
            .0
            .slots
            .lock()
            .unwrap()
            .get(&old.arm.operation_id)
            .unwrap(),
    );
    assert!(old_local.cancel_durable.load(Ordering::SeqCst));
    assert!(old_local.retained_cancel.load(Ordering::SeqCst));
    assert!(*old_local.stop.borrow());
    assert!(old_local.status.lock().unwrap().0.prepared_once);
    assert_eq!(generation.current(), 1);
    client
        .cancel_retained(old.clone(), generation.clone())
        .await
        .unwrap();
    let stopped = client.slot_status(&old.arm.operation_id).await.unwrap();
    let untouched = client.slot_status(&later.arm.operation_id).await.unwrap();
    assert_eq!(stopped.generation, 7, "Cancel must use the remote maximum");
    assert!(stopped.prepared_once);
    assert_eq!(untouched.generation, 1);
    assert!(
        !untouched.prepared_once,
        "a later slot must not be restored"
    );
    assert!(!*later_local.stop.borrow());
    assert!(!later_local.retained_cancel.load(Ordering::SeqCst));
    assert_eq!(generation.current(), 1, "no shared generation seed");
    assert_eq!(
        old_local.registration.arm.deadline_ns_decimal,
        old.arm.deadline_ns_decimal
    );
    let mut divergent = old.clone();
    divergent.control_origin_hash = "b".repeat(64);
    assert!(matches!(
        client.cancel_retained(divergent, generation.clone()).await,
        Err(EnforcerError::Conflict)
    ));

    // A failed original client must still mark a known matching Arc so an
    // already captured recovery inventory sees the durable early-stop intent.
    let failed = unavailable_registration(&base, "failed");
    let (failed_local, _) = client
        .retain_registration(failed.clone(), generation.clone(), false, false)
        .unwrap();
    client.0.fail();
    assert!(matches!(
        client.cancel_retained(failed, generation.clone()).await,
        Err(EnforcerError::Unhealthy)
    ));
    assert!(failed_local.cancel_durable.load(Ordering::SeqCst));
    assert!(failed_local.retained_cancel.load(Ordering::SeqCst));
    assert!(*failed_local.stop.borrow());

    let replacement = EnforcerClient::connect_or_spawn(&root, &executable, 8)
        .await
        .unwrap();
    let only_replacement = unavailable_registration(&base, "replacement");
    let (replacement_local, _) = replacement
        .retain_registration(only_replacement.clone(), generation.clone(), false, false)
        .unwrap();
    assert!(
        !client
            .0
            .slots
            .lock()
            .unwrap()
            .contains_key(&only_replacement.arm.operation_id)
    );
    replacement.0.fail();
    *client.0.replacement.lock().unwrap() = Some(replacement.clone());
    assert!(matches!(
        client
            .cancel_retained(only_replacement, generation.clone())
            .await,
        Err(EnforcerError::Unhealthy)
    ));
    assert!(replacement_local.cancel_durable.load(Ordering::SeqCst));
    assert!(replacement_local.retained_cancel.load(Ordering::SeqCst));
    assert!(*replacement_local.stop.borrow());
    let count = client.0.slots.lock().unwrap().len();
    let replacement_count = replacement.0.slots.lock().unwrap().len();
    assert!(matches!(
        client
            .cancel_retained(
                unavailable_registration(&base, "unknown"),
                generation.clone()
            )
            .await,
        Err(EnforcerError::Unhealthy)
    ));
    assert_eq!(
        client.0.slots.lock().unwrap().len(),
        count,
        "no post-fuse insertion"
    );
    assert_eq!(
        replacement.0.slots.lock().unwrap().len(),
        replacement_count,
        "no insertion into a failed replacement"
    );
    assert_eq!(generation.current(), 1);
}

#[test]
fn native_worker_release_submits_only_with_the_original_live_slot_and_target() {
    use crate::{
        egress::ids::IdV1,
        runtime::bootstrap_stdin::{BootstrapStdinSubmitError, NativeBootstrapRelease},
    };
    use std::{cell::Cell, sync::mpsc};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Case {
        Submit,
        Cancel,
        Stop,
        StopDuringVeto,
        HealthContended,
        SlotLost,
        WrongAuthority,
        WrongAttempt,
        WrongCid,
        Expired,
        ChannelClosed,
    }

    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(PRIVATE_TMP)
        .unwrap();
    let root_path = temp.path().canonicalize().unwrap();
    let root = Directory::open_existing(&root_path).unwrap();
    let registration = unavailable_registration(&root_path, "release");
    let foreign = unavailable_registration(&root_path, "foreign");
    let clock = Arc::new(NativeHostClock::new().unwrap());

    for case in [
        Case::Submit,
        Case::Cancel,
        Case::Stop,
        Case::StopDuringVeto,
        Case::HealthContended,
        Case::SlotLost,
        Case::WrongAuthority,
        Case::WrongAttempt,
        Case::WrongCid,
        Case::Expired,
        Case::ChannelClosed,
    ] {
        // The Unix pair supplies local liveness only. All slot and runtime
        // observations are fixture data, with no guardian or engine launched.
        let (_peer, live) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut client = EnforcerClient::unavailable_for_test(&root).unwrap();
        Arc::get_mut(&mut client.0).unwrap().liveness = live.into();
        client.0.failed.store(false, Ordering::SeqCst);
        *client.0.health.lock().unwrap() = Some((1, Instant::now()));
        let sample = clock.sample().unwrap();
        let arm = &registration.arm;
        let digest = registration.digest().unwrap();
        let generation = CancelGeneration::new();
        let (stop, _stop_rx) = watch::channel(false);
        let status = EnforcerSlotStatusV1 {
            arm_id: arm.operation_id.clone(),
            registration_hash: digest.clone(),
            phase: SlotPhaseV1::Ready,
            prepared_once: true,
            generation: generation.current(),
            clock: Some(sample.clone()),
            observation: None,
            stop_failures: 0,
        };
        let mut deadline = WorkerReleaseDeadline::new(arm, &sample, &sample).unwrap();
        if case == Case::Expired {
            deadline.deadline_ns = sample.continuous_ns().unwrap();
        }
        let local = Arc::new(LocalSlot {
            registration: registration.clone(),
            digest,
            generation: generation.clone(),
            status: Mutex::new((status, Instant::now())),
            stop,
            acknowledged: AtomicBool::new(true),
            cancel_durable: AtomicBool::new(false),
            retained_cancel: AtomicBool::new(false),
            latch: retirement::SlotLatch::default(),
        });
        local.latch.reserve_claim().unwrap();
        if case != Case::SlotLost {
            client
                .0
                .slots
                .lock()
                .unwrap()
                .insert(arm.operation_id.clone(), Arc::clone(&local));
        }
        let permit = Box::new(WorkerBootstrapRelease {
            client,
            local: Arc::clone(&local),
            expected_generation: generation.current(),
            clock: Arc::clone(&clock),
            deadline,
        });
        if case == Case::Cancel {
            generation.invalidate();
        } else if case == Case::Stop {
            local.stop.send_replace(true);
        }
        let attempt = IdV1::parse(if case == Case::WrongAttempt {
            &foreign.arm.attempt_id
        } else {
            &arm.attempt_id
        })
        .unwrap();
        let authority = if case == Case::WrongAuthority {
            &foreign.authority
        } else {
            &registration.authority
        };
        let observed = if case == Case::WrongCid {
            &foreign.owned
        } else {
            &registration.owned
        };
        let contended = (case == Case::HealthContended).then(|| permit.client.clone());
        let submit = |entered: Option<mpsc::Sender<()>>| {
            let writes = Cell::new(0);
            let result = permit.submit(
                authority,
                observed,
                &attempt,
                Box::new(|| {
                    if let Some(entered) = entered {
                        entered.send(()).unwrap();
                    }
                    anyhow::ensure!(case != Case::ChannelClosed, "fixture channel closed");
                    if case == Case::StopDuringVeto {
                        local.stop.send_replace(true);
                    }
                    Ok(())
                }),
                Box::new(|| {
                    writes.set(writes.get() + 1);
                    Ok(())
                }),
            );
            (result, writes.get())
        };
        let (result, writes) = if let Some(client) = contended {
            let health = client.0.health.lock().unwrap();
            std::thread::scope(|scope| {
                let (entered_tx, entered_rx) = mpsc::channel();
                let (release_tx, release_rx) = mpsc::channel();
                let release =
                    scope.spawn(move || release_tx.send(submit(Some(entered_tx))).unwrap());
                // This signal comes from inside the generation gate.
                let entered = entered_rx.recv_timeout(Duration::from_secs(5));
                let (cancel_tx, cancel_rx) = mpsc::channel();
                let generation = &generation;
                let cancel = scope.spawn(move || cancel_tx.send(generation.invalidate()).unwrap());
                let released = release_rx.recv_timeout(Duration::from_secs(5));
                let invalidated = cancel_rx.recv_timeout(Duration::from_secs(5));

                // Release the mutex even on a regression so both threads can
                // finish before the test reports a failed bounded check.
                drop(health);
                release.join().unwrap();
                cancel.join().unwrap();
                entered.expect("release entered the generation gate");
                assert_eq!(
                    invalidated.expect("invalidation finished while health was locked"),
                    1
                );
                released.expect("release refused while health was locked")
            })
        } else {
            submit(None)
        };
        let expected = if case == Case::Submit {
            Ok(())
        } else {
            Err(BootstrapStdinSubmitError::Refused)
        };
        assert_eq!(result, expected, "{case:?}");
        assert_eq!(writes, usize::from(case == Case::Submit), "{case:?}");
    }
}
