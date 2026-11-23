//! A real DeferredRun crosses the normal shutdown helper while its first
//! retained V4 POST is committed in SQLite and its HTTP receipt is withheld.
//! The seed's runtime observations are synthetic; this does not qualify a
//! signal, runtime termination, an execution deadline or enforcer retirement.
use super::*;
use crate::{
    audit::{EventQuery, Store},
    broker::{discovery::load_endpoint, local_ipc::IpcError},
    runner::broker::{ColdRecoverySeed, terminal_pending_seed_for_cold_audit_test},
};
use std::{
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    process::{Child, Command, Stdio},
    time::Instant,
};

struct NativeChild(Child);

impl Drop for NativeChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    std::io::Write::write_all(&mut file, bytes).unwrap();
    file.sync_all().unwrap();
}

/// Start the same fresh native test peer used by the BatchExecutor fixture.
/// The child creates its own role and lease before publishing the endpoint.
async fn empty_full_enforcer(seed: &ColdRecoverySeed) -> (EnforcerClient, NativeChild) {
    let path = seed.root().join("fo");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    let parent = Directory::open_existing(&path).unwrap();
    let nonce = Uuid::new_v4();
    let executable = std::env::current_exe().unwrap();
    let mut child = NativeChild(
        Command::new(&executable)
            .args([
                "--exact",
                "runner::broker::executor::journal::tests::native_fresh_recovery_peer_child",
                "--ignored",
            ])
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("AI_GUARD_TEST_RECOVERY_FRESH_PEER_PARENT", parent.path())
            .env("AI_GUARD_TEST_RECOVERY_FRESH_PEER_NONCE", nonce.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let client = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            let code = status.code();
            panic!("native Full fixture exited before IPC, code={code:?}");
        }
        if let Some(root) = parent.persistent_child("enforcer", false).unwrap() {
            match load_endpoint(&root) {
                Ok(Some(_)) => {
                    break EnforcerClient::connect_or_spawn(&root, &executable, 8)
                        .await
                        .unwrap();
                }
                // The owned fixture can still be writing its first announcement.
                Ok(None) | Err(IpcError::UnsafeFilesystem) => {}
                Err(error) => panic!("native Full fixture discovery failed: {error:?}"),
            }
        }
        assert!(
            Instant::now() < deadline,
            "native test peer did not publish IPC"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let capabilities = serde_json::to_value(client.legacy_capabilities().await.unwrap()).unwrap();
    assert!(
        capabilities.get("fresh_launch_nonce") == Some(&json!(nonce)),
        "native Full fixture nonce mismatch"
    );
    client.require_admission_ready().await.unwrap();
    assert_eq!(client.status().await.unwrap().slot_count(), 0);
    (client, child)
}

async fn full_service(seed: &ColdRecoverySeed) -> (Arc<Service>, WriterClient, NativeChild) {
    let source_root = seed.root().join("full-sources");
    let export_root = seed.root().join("full-exports");
    for path in [&source_root, &export_root] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let registry_path = seed.config().join("full-owner-registry.json");
    write_private(
        &registry_path,
        &serde_json::to_vec(&json!({
            "version":1,
            "installation_id":seed.scope().installation_id(),
            "root":seed.root(),
            "max_enforcer_slots":1,
            "profiles":[{
                "id":seed.scope().profile_id(),
                "gateway":seed.scope().gateway_origin(),
                "tenant_id":seed.scope().tenant_id(),
                "control_namespace_uuid":seed.scope().control_namespace_uuid(),
                "config_dir":seed.config(),
                "runner_token_file":seed.config().join("reconciler.token"),
                "runtime":"docker",
                "runtime_program":seed.root().join("control/engine"),
                "runtime_endpoint":"unix:///cancel-recovery-fixture.sock",
                "image":"agentsandbox-worker:local",
                "policy_profile":"strict",
                "network":"none",
                "allow_rootful_runtime":false,
                "max_timeout_seconds":600,
                "allow_capture_output":false,
                "managed_clients":false,
                "source_roots":[source_root],
                "export_roots":[export_root]
            }]
        }))
        .unwrap(),
    );
    let registry = Arc::new(Registry::load(&registry_path).unwrap());
    let material = Arc::new(Material::load(&registry).unwrap());
    let journal = Journal::open(
        &Directory::open_existing(seed.journal_root()).unwrap(),
        seed.scope().clone(),
        &material.exclusions,
        JournalLimitsV1::default(),
        ControlSecrets::new(material.literals.clone()).unwrap(),
    )
    .unwrap();
    let writer = WriterClient::start(journal, 64, Duration::from_secs(60)).unwrap();
    let jobs = Arc::new(ProfileJobs::new());
    jobs.writer
        .set(ProfileWriterCapabilities::from_full(writer.clone()))
        .unwrap();
    let (enforcer, child) = empty_full_enforcer(seed).await;
    let service = Arc::new(Service {
        full_owners: FullOwners::new(1).unwrap(),
        cold_owners: retained_audit::ColdOwners::new(1).unwrap(),
        registry,
        root: Directory::open_existing(seed.root()).unwrap(),
        material,
        profiles: BTreeMap::from([(seed.scope().profile_id().into(), jobs)]),
        enforcer: Some(ServiceEnforcer::Native(enforcer.into())),
        stop_enforcer: None,
        blocking: Arc::new(Semaphore::new(1)),
        preparation: Arc::new(Semaphore::new(1)),
        execution: Arc::new(Semaphore::new(1)),
        stopping: Arc::new(AtomicBool::new(false)),
        fenced: Arc::new(AtomicBool::new(false)),
    });
    assert!(!service.recovery_only());
    assert!(service.profiles.values().all(|jobs| {
        let caps = jobs.writer.get().unwrap();
        !caps.is_stop_only() && caps.full().is_some() && caps.cold_audit().is_none()
    }));
    (service, writer, child)
}

async fn prepare_full_run(
    seed: &ColdRecoverySeed,
    service: &Arc<Service>,
    writer: &WriterClient,
) -> (Arc<Job>, DeferredRun) {
    let profile = service
        .registry
        .profile(seed.scope().profile_id())
        .unwrap()
        .clone();
    let credential = service.material.reconciler(&profile).unwrap();
    let verified = VerifiedProfile::authenticate(&service.registry, &profile.id, &credential, true)
        .await
        .unwrap();
    let jobs = service.profile_jobs(&profile.id).unwrap();
    let job = jobs.job(seed.operation_id()).unwrap();
    let running = job.enter().unwrap();
    let context = service
        .context(verified, profile, &job, running.generation(), None)
        .await
        .unwrap();
    let run = DeferredRun {
        owner: service.reserve_full_owner().unwrap(),
        executor: service.executor(writer.clone()).unwrap(),
        operation: seed.operation_id(),
        context,
        job: Arc::clone(&job),
        _running: running,
        _capacity: Arc::clone(&service.execution).try_acquire_owned().unwrap(),
        stopping: Arc::clone(&service.stopping),
    };
    (job, run)
}

#[tokio::test(flavor = "current_thread")]
async fn full_reconciliation_acknowledges_pending_result_with_unavailable_activity_spool() {
    let seed = terminal_pending_seed_for_cold_audit_test().await;
    let (service, writer, _child) = full_service(&seed).await;
    let spool = seed.root().join("provider-activity");
    write_private(&spool, b"synthetic unavailable activity directory");
    let before = writer.load(seed.operation_id()).await.unwrap().unwrap();
    assert!(needs_autonomous_reconciliation(&before));
    assert!(seed.is_terminal() && seed.result_pending() && !seed.result_acknowledged());

    let (job, run) = prepare_full_run(&seed, &service, &writer).await;
    run.start().unwrap();
    tokio::time::timeout(Duration::from_secs(60), service.full_owners.drain())
        .await
        .expect("the owned reconciliation must finish")
        .unwrap();

    assert_eq!(job.execution_status(), (false, None));
    let after = writer.load(seed.operation_id()).await.unwrap().unwrap();
    let store = Store::open_for_tenant(
        seed.database(),
        seed.scope().tenant_id(),
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    for (kind, event_id, input_hash) in [
        (
            "broker.batch.result",
            seed.event_id(),
            seed.event_input_hash(),
        ),
        (
            "observer.coverage",
            seed.coverage_event_id(),
            seed.coverage_event_input_hash(),
        ),
    ] {
        let rows = store
            .events(&EventQuery {
                kind: Some(kind.into()),
                ..Default::default()
            })
            .unwrap();
        let [row] = rows.as_slice() else {
            panic!("expected one committed event for {kind}");
        };
        let event = after
            .broker_checkpoint_v4()
            .unwrap()
            .events()
            .iter()
            .find(|event| event.event_id() == event_id)
            .unwrap();
        assert_eq!(event.audit_seq(), Some(row.seq));
        assert!(
            event.input_hash() == input_hash,
            "the retained event hash changed"
        );
        assert!(
            serde_json::to_value(event.event()).unwrap()
                == serde_json::to_value(&row.event).unwrap(),
            "the gateway must commit the exact retained event"
        );
    }
    assert!(seed.runtime_calls() == seed.runtime_call_snapshot());
    assert_eq!(
        fs::read(&spool).unwrap(),
        b"synthetic unavailable activity directory"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn acknowledged_result_and_scan_leave_recovery_eligible_until_cleanup_is_confirmed() {
    use crate::{
        broker::checkpoint::SupervisionOperationV1,
        control::{WorkerStateV1, WorkerTransitionV1},
        runner::broker::terminal_cleanup_jobs_for_test,
    };

    let jobs = terminal_cleanup_jobs_for_test().await;
    let original_worker = jobs[0].checkpoint().broker_context().worker.unwrap();
    for (job, expected_cleanup) in jobs.iter().zip([None, Some(false), Some(true)]) {
        assert_eq!(job.operation_id(), jobs[0].operation_id());
        let worker = job.checkpoint().broker_context().worker.unwrap();
        assert_eq!(worker.worker_id, original_worker.worker_id);
        assert_eq!(
            worker.attempt.attempt_id,
            original_worker.attempt.attempt_id
        );
        assert_eq!(worker.state, WorkerStateV1::Terminal);
        assert!(!worker.quota_held);
        assert_eq!(worker.cleanup_removed, expected_cleanup);

        let lifecycle = job.broker_checkpoint_v3().unwrap();
        assert!(lifecycle.commands().iter().all(|command| {
            !matches!(command.request(), LocalCommandRequestV1::CancelRequested)
        }));
        let operations = lifecycle.broker().supervision().operations();
        assert!(operations.iter().all(SupervisionOperationV1::has_response));
        let cleanup_receipt = operations
            .iter()
            .rev()
            .find_map(|operation| match operation {
                SupervisionOperationV1::WorkerMutation {
                    request,
                    response: Some(answer),
                } if let WorkerTransitionV1::CleanupResult { removed, .. } =
                    &request.transition =>
                {
                    assert_eq!(request.worker_id, worker.worker_id);
                    assert_eq!(request.attempt_id, worker.attempt.attempt_id);
                    assert_eq!(answer.worker.revision, worker.revision);
                    assert!(answer.receipt.audit_seq > 0);
                    Some(*removed)
                }
                _ => None,
            });
        assert_eq!(cleanup_receipt, expected_cleanup);

        let events = job.broker_checkpoint_v4().unwrap().events();
        assert!(events.iter().all(|entry| entry.audit_seq().is_some()));
        for kind in ["broker.batch.result", "broker.batch.scan"] {
            assert!(events.iter().any(|entry| entry.event().kind == kind));
        }
    }

    assert_eq!(
        jobs.each_ref().map(needs_autonomous_reconciliation),
        [true, true, false],
        "only the confirmed removal settles the completed worker's remaining cleanup"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn normal_shutdown_retains_a_full_deferred_run_until_its_owned_future_finishes() {
    let seed = terminal_pending_seed_for_cold_audit_test().await;
    assert!(seed.is_terminal() && seed.result_pending());
    assert!(!seed.result_acknowledged());
    let store = Store::open_for_tenant(
        seed.database(),
        seed.scope().tenant_id(),
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let initial_head = store.head().unwrap();
    let worker_before = store.worker(seed.worker_id()).unwrap().unwrap();
    let (service, writer, child) = full_service(&seed).await;
    let (job, run) = prepare_full_run(&seed, &service, &writer).await;
    let accepted = writer.load(seed.operation_id()).await.unwrap().unwrap();
    let expected = accepted
        .broker_checkpoint_v4()
        .unwrap()
        .events()
        .iter()
        .filter(|entry| entry.audit_seq().is_none())
        .map(|entry| entry.event().clone())
        .collect::<Vec<_>>();
    assert_eq!(expected.len(), 2);
    assert_eq!(expected[0].id, seed.coverage_event_id().to_string());
    assert_eq!(expected[1].id, seed.event_id().to_string());
    let (response_entered, release_response) = seed.pause_next_audit_response_for_test();

    // This is the actual Full scheduling path, with its real accepted journal,
    // authenticated batch context, single-flight guard and execution permit.
    run.start().unwrap();
    let owner_finished = async {
        loop {
            let status = jobs::status(seed.operation_id(), None, Some(&job));
            if !status.executor_task_running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::select! {
        response = tokio::time::timeout(Duration::from_secs(60), response_entered) => {
            response
                .expect("the Full owner reaches the real gateway POST")
                .expect("the gateway owns the response-release barrier");
        }
        () = owner_finished => {
            let durable = writer.load(seed.operation_id()).await.unwrap().unwrap();
            let status = jobs::status(seed.operation_id(),
                Some(JournalStatusV1::Accepted(durable)), Some(&job));
            panic!("Full owner ended before the audit barrier: error={:?}, head={:?}, pending_rpc_count={:?}, permits={}",
                status.executor_error, status.head_revision, status.pending_rpc_count,
                service.execution.available_permits());
        }
    }
    assert_eq!(store.head().unwrap(), initial_head + 1);
    let committed = store
        .events(&EventQuery {
            limit: Some(1000),
            ..Default::default()
        })
        .unwrap();
    let first = committed
        .iter()
        .filter(|row| row.event.id == expected[0].id)
        .collect::<Vec<_>>();
    assert_eq!(first.len(), 1);
    assert_eq!(
        serde_json::to_value(&first[0].event).unwrap(),
        serde_json::to_value(&expected[0]).unwrap()
    );
    assert!(committed.iter().all(|row| row.event.id != expected[1].id));
    assert!(matches!(job.enter(), Err(BrokerErrorCodeV1::Busy)));
    assert_eq!(service.execution.available_permits(), 0);
    eprintln!("full_owner_test phase=post_committed_response_held owner_running=true permits=0");

    let broker_root = service
        .root
        .persistent_child("broker", true)
        .unwrap()
        .unwrap();
    let lease = RoleLease::acquire(&broker_root).unwrap();
    assert!(matches!(
        RoleLease::acquire(&broker_root),
        Err(IpcError::Conflict)
    ));
    let producer = tokio::spawn(async {});
    while !producer.is_finished() {
        tokio::task::yield_now().await;
    }
    let (presence_shutdown, _) = watch::channel(false);
    let mut presence_tasks = JoinSet::new();
    let mut requests = JoinSet::new();
    // The same shutdown-before-release ordering as serve_mode. There is no
    // listener/client/reconciler future left that could cause this wait.
    let mut shutdown = Box::pin(async {
        let result = service
            .shutdown(
                producer,
                &presence_shutdown,
                &mut presence_tasks,
                &mut requests,
            )
            .await;
        drop(lease);
        result
    });
    let before_release = tokio::time::timeout(Duration::from_millis(100), shutdown.as_mut()).await;
    let returned_early = before_release.is_ok();
    let lease_held_before_release =
        matches!(RoleLease::acquire(&broker_root), Err(IpcError::Conflict));
    let guard_held_before_release = matches!(job.enter(), Err(BrokerErrorCodeV1::Busy));
    let capacity_held_before_release = service.execution.available_permits() == 0;
    eprintln!(
        "full_owner_test phase=shutdown_observed returned_early={returned_early} lease_held={lease_held_before_release} guard_held={guard_held_before_release} capacity_held={capacity_held_before_release}"
    );

    // Release and reap before the final RED assertion, including on baseline.
    release_response
        .send(())
        .expect("the Full HTTP response remains held");
    let result = match before_release {
        Ok(result) => result,
        Err(_) => tokio::time::timeout(Duration::from_secs(60), shutdown.as_mut())
            .await
            .expect("shutdown joins the Full owner after receipt release"),
    };
    drop(shutdown);
    // Task liveness is proved by the real RunningJob and execution permit.
    // ACK count is a separate durable fact: an invalidated Full invocation may
    // finish with NeedsReconciliation and leave events pending. Do not wait for
    // an invented successful business outcome to clean up this ownership test.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let status = jobs::status(seed.operation_id(), None, Some(&job));
        let permits = service.execution.available_permits();
        if !status.executor_task_running && permits == 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Full owner still live after release: running={}, error={:?}, permits={permits}",
            status.executor_task_running,
            status.executor_error,
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let live = writer.load(seed.operation_id()).await.unwrap().unwrap();
    let status = jobs::status(seed.operation_id(), None, Some(&job));
    let pending_rpc_count = live.broker_checkpoint_v2().map(|checkpoint| {
        checkpoint
            .supervision()
            .operations()
            .iter()
            .filter(|operation| !operation.has_response())
            .count()
    });
    let committed = store
        .events(&EventQuery {
            limit: Some(1000),
            ..Default::default()
        })
        .unwrap();
    let mut acknowledged = 0;
    let mut stored = 0;
    for expected in &expected {
        let rows = committed
            .iter()
            .filter(|row| row.event.id == expected.id)
            .collect::<Vec<_>>();
        assert!(rows.len() <= 1, "a retained event must not be duplicated");
        stored += rows.len();
        let local = live
            .broker_checkpoint_v4()
            .unwrap()
            .events()
            .iter()
            .find(|entry| entry.event().id == expected.id)
            .unwrap();
        assert!(
            serde_json::to_value(local.event()).unwrap() == serde_json::to_value(expected).unwrap(),
            "the exact retained event is immutable"
        );
        if let Some(sequence) = local.audit_seq() {
            acknowledged += 1;
            assert_eq!(rows.len(), 1, "a local ACK requires a committed event");
            assert_eq!(
                sequence, rows[0].seq,
                "a local ACK is the real Store receipt"
            );
        }
    }
    let runtime_unchanged = seed.runtime_calls() == seed.runtime_call_snapshot();
    let decisions_unchanged =
        seed.current_decision_request_count() == seed.decision_request_count_snapshot();
    let operations = live
        .broker_checkpoint_v2()
        .unwrap()
        .supervision()
        .operations()
        .iter()
        .filter(|operation| {
            matches!(
                operation,
                crate::broker::checkpoint::SupervisionOperationV1::Arm { .. }
                    | crate::broker::checkpoint::SupervisionOperationV1::WorkerClaim { .. }
                    | crate::broker::checkpoint::SupervisionOperationV1::HelperClaim { .. }
            )
        })
        .collect::<Vec<_>>();
    let arm_claims_unchanged =
        serde_json::to_value(operations).unwrap() == *seed.arm_claim_snapshot();
    let worker = store.worker(seed.worker_id()).unwrap().unwrap();
    let worker_unchanged =
        serde_json::to_value(&worker).unwrap() == serde_json::to_value(&worker_before).unwrap();
    eprintln!(
        "full_owner_test phase=owner_finished running={} error={:?} pending_rpc_count={:?} acknowledged={acknowledged} pending_events={} stored={stored} permits={} shutdown_ok={} runtime_unchanged={runtime_unchanged} decisions_unchanged={decisions_unchanged} arm_claims_unchanged={arm_claims_unchanged} worker_unchanged={worker_unchanged}",
        status.executor_task_running,
        status.executor_error,
        pending_rpc_count,
        expected.len() - acknowledged,
        service.execution.available_permits(),
        result.is_ok(),
    );
    assert!(runtime_unchanged && decisions_unchanged && arm_claims_unchanged && worker_unchanged);
    assert!(matches!(
        status.executor_error,
        None | Some(BrokerErrorCodeV1::NeedsReconciliation)
    ));
    assert_eq!(worker.state, crate::control::WorkerStateV1::Terminal);
    assert!(!worker.quota_held);
    assert!(
        job.enter().is_ok(),
        "the actual Full guard was released after completion"
    );
    assert!(RoleLease::acquire(&broker_root).is_ok());
    assert_eq!(
        service
            .full_enforcer()
            .unwrap()
            .status()
            .await
            .unwrap()
            .slot_count(),
        0
    );
    drop(live);
    drop(accepted);
    drop(writer);
    drop(service);
    drop(child);
    assert!(guard_held_before_release && capacity_held_before_release);
    assert!(
        !returned_early,
        "normal shutdown must join the accepted Full DeferredRun before releasing its lease"
    );
    assert!(
        lease_held_before_release,
        "the same broker role lease stays held until the Full owner is joined"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn full_slot_saturation_is_refused_before_a_real_resume_acknowledgement() {
    let seed = terminal_pending_seed_for_cold_audit_test().await;
    let (service, writer, child) = full_service(&seed).await;
    service.require_starts().await.unwrap();
    let profile = service
        .registry
        .profile(seed.scope().profile_id())
        .unwrap()
        .clone();
    let credential = service.material.reconciler(&profile).unwrap();
    let jobs = service.profile_jobs(&profile.id).unwrap();
    let project = profile.source_roots[0].join("resume-capacity-project");
    fs::create_dir(&project).unwrap();
    fs::set_permissions(&project, fs::Permissions::from_mode(0o700)).unwrap();
    write_private(
        &project.join("input.txt"),
        b"Full owner admission fixture\n",
    );
    let operation = Uuid::new_v4();
    let submit = SubmitV1 {
        version: 1,
        operation_id: operation,
        project,
        // This argv is data for a future worker. No Batch future is polled by
        // setup, so neither this command nor an engine is executed here.
        execution: crate::broker::service::RequestedExecutionV1::Command {
            argv: vec!["/usr/bin/id".into()],
        },
        timeout_seconds: Some(30),
        capture_output: false,
    };
    let verified = VerifiedProfile::authenticate(&service.registry, &profile.id, &credential, true)
        .await
        .unwrap();
    let payload = serde_json::to_string(&submit).unwrap();
    let initial = service
        .submit(
            AuthorizedRequest {
                profile: profile.clone(),
                verified,
                writer: writer.clone(),
                jobs: Arc::clone(&jobs),
            },
            payload.clone(),
            &credential,
        )
        .await
        .unwrap();
    assert!(
        matches!(&initial.reply, BrokerReplyV1::Accepted { operation_id, .. } if *operation_id == operation)
    );
    assert!(
        initial.run.is_some(),
        "the real Submit constructed a DeferredRun"
    );
    // Keep the accepted intention/source, but do not schedule its initial run.
    // This is private-handler setup, not a simulated acceptance reply.
    drop(initial);
    let accepted = writer.load(operation).await.unwrap().unwrap();
    jobs::require_profile(&accepted, &profile).unwrap();
    assert!(!accepted.checkpoint().broker_context().managed);
    assert!(!needs_autonomous_reconciliation(&accepted));
    let rejected_operation = Uuid::new_v4();
    let mut rejected_submit = serde_json::to_value(&submit).unwrap();
    rejected_submit["operation_id"] = json!(rejected_operation);
    rejected_submit["project"] = json!(submit.project.join("missing-project"));
    let rejected_payload = serde_json::to_string(&rejected_submit).unwrap();
    let verified = VerifiedProfile::authenticate(&service.registry, &profile.id, &credential, true)
        .await
        .unwrap();
    let rejected = service
        .submit(
            AuthorizedRequest {
                profile: profile.clone(),
                verified,
                writer: writer.clone(),
                jobs: Arc::clone(&jobs),
            },
            rejected_payload.clone(),
            &credential,
        )
        .await
        .unwrap();
    assert!(
        rejected.run.is_none()
            && matches!(&rejected.reply,
        BrokerReplyV1::Rejected { operation_id, replayed: false, .. } if *operation_id == rejected_operation)
    );
    drop(rejected);
    let job = jobs.job(operation).unwrap();
    assert!(job.enter().is_ok());
    assert_eq!(service.execution.available_permits(), 1);
    let runtime_before = seed.runtime_calls();
    let decisions_before = seed.current_decision_request_count();
    let store = Store::open_for_tenant(
        seed.database(),
        seed.scope().tenant_id(),
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let worker_before = serde_json::to_value(store.worker(seed.worker_id()).unwrap()).unwrap();

    // Deterministically extend only the bookkeeping window between releasing
    // a task's execution permit and its JoinHandle becoming finished. This
    // controlled owner does not imitate a decision, worker or runtime effect.
    let released = Arc::new(tokio::sync::Notify::new());
    let finish = Arc::new(tokio::sync::Notify::new());
    let task_released = Arc::clone(&released);
    let task_finish = Arc::clone(&finish);
    let first_capacity = Arc::clone(&service.execution).try_acquire_owned().unwrap();
    service
        .full_owners
        .try_reserve()
        .unwrap()
        .start(async move {
            drop(first_capacity);
            task_released.notify_one();
            task_finish.notified().await;
            Ok(())
        })
        .unwrap();
    released.notified().await;
    assert_eq!(service.execution.available_permits(), 1);
    assert!(matches!(
        service.full_owners.try_reserve(),
        Err(retained_audit::OwnerError::Busy)
    ));

    // Existing payloads are still compared by the actor while every Full slot
    // is busy. Hide the original source temporarily: replay must use the
    // accepted intention, not reload the project or create another ticket.
    let hidden_project = submit
        .project
        .with_file_name("retained-resume-capacity-project");
    fs::rename(&submit.project, &hidden_project).unwrap();
    let replay_checks = async {
        let verified = VerifiedProfile::authenticate(&service.registry, &profile.id, &credential, true).await?;
        let replay = service.submit(AuthorizedRequest {
            profile: profile.clone(), verified, writer: writer.clone(), jobs: Arc::clone(&jobs),
        }, payload, &credential).await?;
        let exact_replay = replay.run.is_none() && matches!(&replay.reply,
            BrokerReplyV1::Accepted { operation_id, intent_hash, replayed: true, .. }
                if *operation_id == operation && intent_hash.as_str() == accepted.intent_hash());
        drop(replay);
        let verified = VerifiedProfile::authenticate(&service.registry, &profile.id, &credential, true).await?;
        let rejected_replay = service.submit(AuthorizedRequest {
            profile: profile.clone(), verified, writer: writer.clone(), jobs: Arc::clone(&jobs),
        }, rejected_payload, &credential).await?;
        let exact_rejected_replay = rejected_replay.run.is_none() && matches!(&rejected_replay.reply,
            BrokerReplyV1::Rejected { operation_id, replayed: true, .. } if *operation_id == rejected_operation);
        drop(rejected_replay);
        let mut divergent = serde_json::to_value(&submit).unwrap();
        divergent["timeout_seconds"] = json!(31);
        let verified = VerifiedProfile::authenticate(&service.registry, &profile.id, &credential, true).await?;
        let divergence_conflict = matches!(service.submit(AuthorizedRequest {
            profile: profile.clone(), verified, writer: writer.clone(), jobs: Arc::clone(&jobs),
        }, serde_json::to_string(&divergent).unwrap(), &credential).await,
            Err(BrokerErrorCodeV1::Conflict));
        let fresh_operation = Uuid::new_v4();
        divergent["operation_id"] = json!(fresh_operation);
        let verified = VerifiedProfile::authenticate(&service.registry, &profile.id, &credential, true).await?;
        let fresh_busy = matches!(service.submit(AuthorizedRequest {
            profile: profile.clone(), verified, writer: writer.clone(), jobs: Arc::clone(&jobs),
        }, serde_json::to_string(&divergent).unwrap(), &credential).await,
            Err(BrokerErrorCodeV1::Busy));
        let fresh_has_no_ticket = writer.status(fresh_operation).await.map_err(jobs::journal_error)?.is_none();
        Ok::<_, BrokerErrorCodeV1>((exact_replay, exact_rejected_replay, divergence_conflict, fresh_busy, fresh_has_no_ticket))
    }.await;
    fs::rename(&hidden_project, &submit.project).unwrap();

    let command = Uuid::new_v4();
    let verified = VerifiedProfile::authenticate(&service.registry, &profile.id, &credential, true)
        .await
        .unwrap();
    let response = service
        .resume(
            AuthorizedRequest {
                profile,
                verified,
                writer: writer.clone(),
                jobs: Arc::clone(&jobs),
            },
            operation,
            command,
        )
        .await;
    let mut acknowledgement_produced = false;
    let mut deferred_produced = false;
    let mut refusal = None;
    let mut late_start_error = None;
    match response {
        Ok(mut response) => {
            acknowledgement_produced = matches!(&response.reply,
                BrokerReplyV1::CommandAcknowledged { operation_id, command_id, .. }
                    if *operation_id == operation && *command_id == command);
            if let Some(run) = response.run.take() {
                deferred_produced = true;
                // The preimage reached this late admission and returned Busy.
                // With a pre-reserved token this entire reply branch must be
                // unreachable while the predecessor still owns its only slot.
                late_start_error = run.start().err();
            }
        }
        Err(code) => refusal = Some(code),
    }
    eprintln!(
        "full_owner_test phase=resume_capacity_observed acknowledgement_produced={acknowledgement_produced} deferred_produced={deferred_produced} refusal={refusal:?} late_start_error={late_start_error:?}"
    );
    // Always release and join the predecessor before the regression oracle.
    finish.notify_one();
    service.full_owners.drain().await.unwrap();
    let after = writer.load(operation).await.unwrap().unwrap();
    let resume_recorded = after.broker_checkpoint_v4().is_some_and(|checkpoint| {
        checkpoint
            .events()
            .iter()
            .any(|event| event.event_id() == command)
    });
    let no_batch_operations = after
        .broker_checkpoint_v2()
        .is_none_or(|checkpoint| checkpoint.supervision().operations().is_empty());
    let batch_not_polled = no_batch_operations
        && !after.checkpoint().broker_context().managed
        && seed.runtime_calls() == runtime_before
        && seed.current_decision_request_count() == decisions_before;
    let worker_unchanged =
        serde_json::to_value(store.worker(seed.worker_id()).unwrap()).unwrap() == worker_before;
    let capacity_released = service.execution.available_permits() == 1;
    let guard_released = job.enter().is_ok();
    eprintln!(
        "full_owner_test phase=resume_capacity_cleanup resume_recorded={resume_recorded} batch_not_polled={batch_not_polled} worker_unchanged={worker_unchanged} capacity_released={capacity_released} guard_released={guard_released}"
    );
    drop(after);
    drop(accepted);
    drop(writer);
    drop(service);
    drop(child);
    let (exact_replay, exact_rejected_replay, divergence_conflict, fresh_busy, fresh_has_no_ticket) =
        replay_checks.unwrap();
    eprintln!(
        "full_owner_test phase=submit_capacity_replays exact_replay={exact_replay} exact_rejected_replay={exact_rejected_replay} divergence_conflict={divergence_conflict} fresh_busy={fresh_busy} fresh_has_no_ticket={fresh_has_no_ticket}"
    );
    assert!(
        exact_replay
            && exact_rejected_replay
            && divergence_conflict
            && fresh_busy
            && fresh_has_no_ticket
    );
    assert!(batch_not_polled && worker_unchanged && capacity_released && guard_released);
    assert!(
        !resume_recorded,
        "a capacity refusal must not append a new durable Resume command"
    );
    assert!(
        refusal == Some(BrokerErrorCodeV1::Busy)
            && !acknowledgement_produced
            && !deferred_produced
            && late_start_error.is_none(),
        "Full slot saturation must be refused before the real Resume acknowledgement, not at DeferredRun.start after ACK"
    );
}

/// These controlled futures test only the typed owner mechanism. The tests above
/// separately proves its connection to a real accepted Full DeferredRun.
mod registry {
    use super::*;
    use std::{
        future::{Future, poll_fn},
        pin::Pin,
        sync::atomic::AtomicUsize,
        task::Poll,
    };
    use tokio::sync::Notify;

    struct NotifyOnDrop(Arc<Notify>);
    impl Drop for NotifyOnDrop {
        fn drop(&mut self) {
            self.0.notify_one();
        }
    }

    async fn poll_once<F: Future>(future: &mut Pin<Box<F>>) -> Poll<F::Output> {
        poll_fn(|context| Poll::Ready(future.as_mut().poll(context))).await
    }

    #[tokio::test(flavor = "current_thread")]
    async fn full_registry_bounds_reservations_and_refuses_late_future_without_polling() {
        let owners = FullOwners::new(1).unwrap();
        let reserved = owners.try_reserve().unwrap();
        assert!(matches!(
            owners.try_reserve(),
            Err(retained_audit::OwnerError::Busy)
        ));
        drop(reserved);
        let late = owners.try_reserve().unwrap();
        owners.close().unwrap();
        owners.drain().await.unwrap();
        let polls = Arc::new(AtomicUsize::new(0));
        let task_polls = Arc::clone(&polls);
        assert!(matches!(
            late.start(async move {
                task_polls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
            Err(retained_audit::OwnerError::Closed)
        ));
        assert_eq!(polls.load(Ordering::SeqCst), 0);
        assert!(matches!(
            owners.try_reserve(),
            Err(retained_audit::OwnerError::Closed)
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn full_registry_keeps_business_and_panic_failures_after_reap_and_cancelled_drain() {
        for panic_owner in [false, true] {
            let owners = FullOwners::new(2).unwrap();
            let failed = owners.try_reserve().unwrap();
            let held = owners.try_reserve().unwrap();
            let failure_finished = Arc::new(Notify::new());
            let task_finished = Arc::clone(&failure_finished);
            failed
                .start(async move {
                    let _completed = NotifyOnDrop(task_finished);
                    if panic_owner {
                        panic!("controlled Full owner panic");
                    }
                    Err(BrokerErrorCodeV1::NeedsReconciliation)
                })
                .unwrap();
            let started = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let completed = Arc::new(AtomicBool::new(false));
            let task_started = Arc::clone(&started);
            let task_release = Arc::clone(&release);
            let task_completed = Arc::clone(&completed);
            held.start(async move {
                task_started.notify_one();
                task_release.notified().await;
                task_completed.store(true, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
            started.notified().await;
            failure_finished.notified().await;
            // Reap the failed handle and reuse its slot before observing drain.
            let reused = owners.try_reserve().unwrap();
            drop(reused);
            owners.close().unwrap();
            let mut abandoned = Box::pin(owners.drain());
            assert!(matches!(poll_once(&mut abandoned).await, Poll::Pending));
            drop(abandoned);
            let mut replacement = Box::pin(owners.drain());
            assert!(matches!(poll_once(&mut replacement).await, Poll::Pending));
            release.notify_one();
            assert_eq!(
                replacement.await,
                Err(retained_audit::DrainError::OwnerFailed)
            );
            assert!(completed.load(Ordering::SeqCst));
            assert_eq!(
                owners.drain().await,
                Err(retained_audit::DrainError::OwnerFailed)
            );
        }
    }
}
