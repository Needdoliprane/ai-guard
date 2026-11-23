//! Cold, stop-only broker audit reconciliation. The gateway/SQLite fixture is
//! real and remains at the same identity; no runtime or provider is started.
use super::super::owned_ipc_peer::OwnedIpcPeer;
use super::*;
use crate::{
    audit::{EventQuery, Store},
    broker::{
        checkpoint::SupervisionOperationV1,
        journal::{
            ControlSecrets, FsExclusions, Journal, JournalError, JournalLimitsV1,
            recovery::StopRecoveryReaderV1,
        },
        lifecycle::LocalCommandRequestV1,
        service::BrokerClient,
        writer::ProfileWriterCapabilities,
    },
    control::{ArmSubjectV1, QuotaPolicyV1, WorkerStateV1},
    control_fs::Directory,
    model::StoredEvent,
    redaction::Redactor,
    runner::broker::{ColdRecoverySeed, terminal_pending_seed_for_cold_audit_test},
};
use anyhow::Context;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const RECONCILE_PHASE_TIMEOUT: Duration = Duration::from_secs(60);

fn write_private(path: &std::path::Path, bytes: &[u8]) {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    std::io::Write::write_all(&mut file, bytes).unwrap();
    file.sync_all().unwrap();
}

pub(in crate::broker::service) async fn stop_only_service(
    seed: &ColdRecoverySeed,
) -> (Arc<Service>, Arc<ProfileJobs>, RecoveryEnforcerOwner) {
    let root = seed.root();
    let config_dir = seed.config();
    let source_root = root.join("sources");
    let export_root = root.join("exports");
    for path in [&source_root, &export_root] {
        fs::create_dir_all(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let token_path = config_dir.join("reconciler.token");
    let registry_path = config_dir.join("cold-recovery-registry.json");
    write_private(
        &registry_path,
        &serde_json::to_vec(&json!({
            "version":1,
            "installation_id":seed.scope().installation_id(),
            "root":root,
            "max_enforcer_slots":1,
            "profiles":[{
                "id":seed.scope().profile_id(),
                "gateway":seed.scope().gateway_origin(),
                "tenant_id":seed.scope().tenant_id(),
                "control_namespace_uuid":seed.scope().control_namespace_uuid(),
                "config_dir":config_dir,
                "runner_token_file":token_path,
                "runtime":"docker",
                "runtime_program":root.join("control/engine"),
                "runtime_endpoint":"unix:///cold-audit-test-unused",
                "image":"never-used:test",
                "policy_profile":"strict",
                "network":"none",
                "allow_rootful_runtime":false,
                "max_timeout_seconds":30,
                "allow_capture_output":false,
                "managed_clients":false,
                "source_roots":[source_root],
                "export_roots":[export_root]
            }]
        }))
        .unwrap(),
    );
    let registry =
        Arc::new(crate::broker::service::profile::Registry::load(&registry_path).unwrap());
    let material = Arc::new(Material::load(&registry).unwrap());
    let broker_root = Directory::open_existing(&registry.data.root).unwrap();

    let profile_root = Directory::open_existing(seed.journal_root()).unwrap();
    let reader = StopRecoveryReaderV1::open_existing(
        &profile_root,
        seed.scope().clone(),
        &material.exclusions,
        JournalLimitsV1::default(),
        ControlSecrets::new(material.literals.clone()).unwrap(),
    )
    .unwrap()
    .expect("cold service must reopen the existing profile only");
    let report = reader.scan();
    let capability =
        ProfileWriterCapabilities::from_recovery_report(&report, true, 64, Duration::from_secs(60))
            .unwrap();
    assert!(capability.is_stop_only());
    assert!(capability.full().is_none());
    let jobs = Arc::new(ProfileJobs::new());
    jobs.writer.set(capability).unwrap();
    drop(report);

    let (enforcer_temp, enforcer_root) = recovery_role_fixture();
    let child = recovery_child(&enforcer_root, None);
    let stop_enforcer = warmed_recovery_client(&enforcer_root).await;
    let service = Arc::new(Service {
        full_owners: FullOwners::new(registry.data.max_enforcer_slots as usize).unwrap(),
        cold_owners: retained_audit::ColdOwners::new(registry.data.max_enforcer_slots as usize)
            .unwrap(),
        registry,
        root: broker_root,
        material,
        profiles: BTreeMap::from([(seed.scope().profile_id().to_owned(), Arc::clone(&jobs))]),
        enforcer: None,
        stop_enforcer: Some(stop_enforcer),
        blocking: Arc::new(tokio::sync::Semaphore::new(1)),
        preparation: Arc::new(tokio::sync::Semaphore::new(1)),
        execution: Arc::new(tokio::sync::Semaphore::new(1)),
        stopping: Arc::new(AtomicBool::new(false)),
        fenced: Arc::new(AtomicBool::new(true)),
    });
    // The child is reaped before its private socket directory is removed.
    let owner = RecoveryEnforcerOwner {
        child: Some(child),
        directory: Some(enforcer_temp),
    };
    (service, jobs, owner)
}

pub(in crate::broker::service) struct RecoveryEnforcerOwner {
    child: Option<RecoveryChild>,
    directory: Option<tempfile::TempDir>,
}

impl Drop for RecoveryEnforcerOwner {
    fn drop(&mut self) {
        drop(self.child.take());
        drop(self.directory.take());
    }
}

async fn pending_result_ack(
    seed: &ColdRecoverySeed,
) -> (
    bool,
    bool,
    i64,
    String,
    (Uuid, i64, String),
    serde_json::Value,
) {
    let journal_root = Directory::open_existing(seed.journal_root()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let journal = loop {
        match Journal::open(
            &journal_root,
            seed.scope().clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(Vec::new()).unwrap(),
        ) {
            Ok(opened) => {
                break opened;
            }
            Err(JournalError::Busy) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "StopOnly writer lease did not release after loop join"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("cold full Journal reopen failed: {error}"),
        }
    };
    let job = journal
        .load(seed.operation_id())
        .unwrap()
        .expect("cold journal job remains present");
    let private: serde_json::Value = serde_json::to_value(job.checkpoint()).unwrap();
    let output_acknowledged = private["managed"]["output_acknowledged"]
        .as_bool()
        .expect("the fixture records managed output acknowledgement state");
    assert_eq!(output_acknowledged, seed.result_acknowledged());
    let result = job
        .broker_checkpoint_v4()
        .unwrap()
        .events()
        .iter()
        .find(|event| event.event_id() == seed.event_id())
        .expect("same V4 result event remains in the cold journal");
    assert_eq!(result.input_hash(), seed.event_input_hash());
    let seq = result
        .audit_seq()
        .expect("result outbox entry has a durable local ACK");
    let coverage = job
        .broker_checkpoint_v4()
        .unwrap()
        .events()
        .iter()
        .find(|event| event.event_id() == seed.coverage_event_id())
        .expect("the existing coverage event remains present");
    assert_eq!(coverage.input_hash(), seed.coverage_event_input_hash());
    let coverage_seq = coverage
        .audit_seq()
        .expect("coverage outbox entry has a durable local ACK");
    assert!(coverage_seq > 0);
    let unacknowledged = job
        .broker_checkpoint_v4()
        .unwrap()
        .events()
        .iter()
        .filter(|event| event.audit_seq().is_none())
        .map(|event| event.event_id())
        .collect::<Vec<_>>();
    assert!(
        unacknowledged.is_empty(),
        "cold reopen must find durable ACKs for both pending V4 events"
    );
    let cancel_present = job
        .broker_checkpoint_v3()
        .unwrap()
        .commands()
        .iter()
        .any(|command| {
            command.command_id() == seed.cancel_id()
                && matches!(command.request(), LocalCommandRequestV1::CancelRequested)
        });
    let operations = job
        .broker_checkpoint_v2()
        .unwrap()
        .supervision()
        .operations()
        .iter()
        .filter(|operation| {
            matches!(
                operation,
                SupervisionOperationV1::Arm { .. }
                    | SupervisionOperationV1::WorkerClaim { .. }
                    | SupervisionOperationV1::HelperClaim { .. }
            )
        })
        .collect::<Vec<_>>();
    (
        output_acknowledged,
        cancel_present,
        seq,
        result.input_hash().to_owned(),
        (
            coverage.event_id(),
            coverage_seq,
            coverage.input_hash().to_owned(),
        ),
        serde_json::to_value(operations).unwrap(),
    )
}

async fn check_stop_recovery_status(client: &BrokerClient, seed: &ColdRecoverySeed) -> Result<()> {
    let BrokerReplyV1::RecoveryStatus { status } = client
        .request(BrokerCommandV1::RecoveryStatus {
            operation_id: seed.operation_id(),
        })
        .await
        .context("authenticated STOP recovery status request failed")?
    else {
        anyhow::bail!("the STOP service did not return its recovery status");
    };
    ensure!(status.version == 1, "wrong recovery status version");
    ensure!(
        status.operation_id == seed.operation_id() && status.tenant_id == seed.scope().tenant_id(),
        "recovery status belongs to a different operation or tenant"
    );
    ensure!(
        status.journal_mode == JournalModeV1::StopOnly,
        "journal is not STOP-only"
    );
    ensure!(
        status.source == RecoveryDataSourceV1::ConfirmedJournal,
        "recovery status lacks confirmed journal provenance"
    );
    ensure!(
        status.runtime_evidence == RecoveryRuntimeEvidenceV1::NotQueried,
        "recovery status queried the runtime"
    );
    ensure!(
        !status.incomplete && status.reconciliation_required,
        "recovery status must be complete and require reconciliation"
    );

    let prior: Vec<SupervisionOperationV1> =
        serde_json::from_value(seed.arm_claim_snapshot().clone())?;
    let mut original = prior.iter().filter_map(|operation| match operation {
        SupervisionOperationV1::Arm {
            request,
            response: Some(_),
            ..
        } if matches!(request.subject, ArmSubjectV1::Worker) => Some(request),
        _ => None,
    });
    let arm = original
        .next()
        .context("worker Arm missing from the seed")?;
    ensure!(
        original.next().is_none(),
        "the seed retains multiple worker Arms"
    );
    let mut recovered = status
        .known_arms
        .iter()
        .filter(|target| matches!(target.subject, RecoveryTargetKindV1::Worker));
    let target = recovered
        .next()
        .context("worker Arm missing from recovery status")?;
    ensure!(
        recovered.next().is_none(),
        "multiple worker Arms were recovered"
    );
    ensure!(
        target.arm_id == arm.operation_id
            && target.worker_id == seed.worker_id()
            && target.attempt_id == seed.attempt_id(),
        "recovery status changed the original worker Arm identity"
    );
    ensure!(
        target.original_timeout_seconds == arm.timeout_seconds
            && target.original_deadline_ns_decimal == arm.deadline_ns_decimal
            && canonical(&target.original_clock)? == canonical(&arm.clock)?,
        "recovery status changed the original worker Arm deadline or clock"
    );
    Ok(())
}

async fn check_stop_result_refused(client: &BrokerClient, operation_id: Uuid) -> Result<()> {
    let reply = client
        .request(BrokerCommandV1::Result {
            version: JOB_RESULT_VERSION,
            operation_id,
        })
        .await?;
    ensure!(
        matches!(
            reply,
            BrokerReplyV1::Error {
                code: BrokerErrorCodeV1::NeedsReconciliation
            }
        ),
        "authenticated Result must remain fenced on the STOP service"
    );
    Ok(())
}

#[test]
fn cold_stop_only_reconcile_loop_publishes_pending_v4_without_start_capability() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let seed = runtime.block_on(terminal_pending_seed_for_cold_audit_test());
    let outcome = runtime.block_on(check_cold_stop_reconcile_loop(&seed));
    // If the owner drain timed out, its task is still owned by this runtime.
    // Keep the seed alive through shutdown without waiting forever for blocking I/O.
    runtime.shutdown_timeout(RECONCILE_PHASE_TIMEOUT);
    outcome
}

async fn check_cold_stop_reconcile_loop(seed: &ColdRecoverySeed) -> Result<()> {
    assert!(seed.is_terminal());
    assert!(!seed.quota_held());
    assert_eq!(seed.allocation_count(), 1);
    assert!(seed.result_pending());
    assert!(!seed.result_acknowledged());
    assert_eq!(seed.coverage_audit_seq(), None);
    assert!(seed.retained_gateway_is_live());
    eprintln!("cold_recovery_test phase=seed_verified");

    let store = Store::open_for_tenant(
        seed.database(),
        seed.scope().tenant_id(),
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let worker_before = store.worker(seed.worker_id()).unwrap().unwrap();
    assert_eq!(worker_before.state, WorkerStateV1::Terminal);
    assert!(!worker_before.quota_held);
    let policy = QuotaPolicyV1::default();
    let quota_before = store.worker_quota(&policy).unwrap();
    assert_eq!(quota_before.held_workers, 0);
    assert_eq!(quota_before.held_cpu_millis, 0);
    assert_eq!(quota_before.held_memory_bytes, 0);
    let usage_before = store.worker_usage().unwrap();
    assert_eq!(usage_before.len(), 1);
    assert!(!usage_before[0].billable);
    let initial_result_events = store
        .events(&EventQuery {
            kind: Some("broker.batch.result".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(initial_result_events.is_empty());
    let initial_coverage_events = store
        .events(&EventQuery {
            kind: Some("observer.coverage".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(initial_coverage_events.is_empty());
    let initial_head = store.head().unwrap();

    let (service, jobs, enforcer_owner) = stop_only_service(seed).await;
    assert!(service.recovery_only());
    assert!(matches!(
        service.require_starts().await,
        Err(BrokerErrorCodeV1::NeedsReconciliation)
    ));
    let capability = jobs.writer.get().unwrap();
    assert!(capability.is_stop_only());
    assert!(capability.full().is_none());
    let profile = service.registry.profile(seed.scope().profile_id()).unwrap();
    assert_eq!(profile.gateway, seed.scope().gateway_origin());
    eprintln!("cold_recovery_test phase=stop_only_ready");
    let serving = Arc::clone(&service);
    let peer = OwnedIpcPeer::start(move |stream| {
        let serving = Arc::clone(&serving);
        async move {
            Box::pin(serving.connection(stream)).await;
            Ok(())
        }
    })
    .await?;
    let preflight: Result<BrokerClient> = async {
        let client = BrokerClient::open(
            peer.root.clone(),
            seed.scope().profile_id().to_owned(),
            &profile.runner_token_file,
        )?;
        check_stop_recovery_status(&client, seed).await?;
        check_stop_result_refused(&client, seed.operation_id()).await?;
        Ok(client)
    }
    .await;
    let client = match preflight {
        Ok(client) => client,
        Err(error) => {
            peer.finish()
                .await
                .context("private IPC peer must drain after preflight")?;
            return Err(error);
        }
    };
    eprintln!("cold_recovery_test phase=retained_ipc_verified");

    let mut task = tokio::spawn(Arc::clone(&service).reconcile_loop());
    let deadline = tokio::time::Instant::now() + RECONCILE_PHASE_TIMEOUT;
    let published_events: Result<(StoredEvent, StoredEvent)> = async {
        loop {
            let coverage = store
                .events(&EventQuery {
                    kind: Some("observer.coverage".into()),
                    ..Default::default()
                })
                .context("read retained coverage from the Store")?
                .into_iter()
                .find(|event| event.event.id == seed.coverage_event_id().to_string());
            let result = store
                .events(&EventQuery {
                    kind: Some("broker.batch.result".into()),
                    ..Default::default()
                })
                .context("read retained result from the Store")?
                .into_iter()
                .find(|event| event.event.id == seed.event_id().to_string());
            if let (Some(coverage), Some(result)) = (coverage, result) {
                eprintln!("cold_recovery_test phase=event_pair_observed");
                return Ok((coverage, result));
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for both pending V4 events"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    .await;
    if published_events.is_err() {
        service.stopping.store(true, Ordering::SeqCst);
    }
    // Join the owner that confirms the local ACKs before testing Result again.
    // Check the refusal while the STOP peer still accepts IPC and before stopping the service.
    let delivery = tokio::time::timeout(RECONCILE_PHASE_TIMEOUT, service.cold_owners.drain()).await;
    let result_after = if published_events.is_ok() && matches!(&delivery, Ok(Ok(()))) {
        check_stop_result_refused(&client, seed.operation_id()).await
    } else {
        Ok(())
    };
    service.stopping.store(true, Ordering::SeqCst);
    let peer_outcome = peer.finish().await;
    let loop_outcome = tokio::time::timeout(RECONCILE_PHASE_TIMEOUT, &mut task).await;
    if loop_outcome.is_err() {
        task.abort();
        let _ = task.await;
    }
    let delivery = delivery.context("retained audit owner did not drain within its budget")?;
    peer_outcome.context("private IPC peer must drain before reopening the journal")?;
    loop_outcome
        .context("reconcile loop did not stop within its budget")?
        .context("reconcile loop task failed")?;
    let (seen_coverage, seen_result) = published_events?;
    delivery.context("retained audit owner did not confirm delivery")?;
    result_after?;
    eprintln!("cold_recovery_test phase=loop_joined");
    assert!(matches!(
        service.require_starts().await,
        Err(BrokerErrorCodeV1::NeedsReconciliation)
    ));
    assert!(jobs.writer.get().unwrap().full().is_none());
    assert_eq!(seed.runtime_calls(), seed.runtime_call_snapshot());
    assert_eq!(
        seed.current_decision_request_count(),
        seed.decision_request_count_snapshot(),
        "cold reconciliation does not call the decision endpoint"
    );

    let after_coverage = store
        .events(&EventQuery {
            kind: Some("observer.coverage".into()),
            ..Default::default()
        })
        .unwrap();
    let after_result = store
        .events(&EventQuery {
            kind: Some("broker.batch.result".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        after_coverage.len(),
        1,
        "coverage is committed exactly once"
    );
    assert_eq!(after_result.len(), 1, "result is committed exactly once");
    let coverage = &after_coverage[0];
    let result = &after_result[0];
    assert_eq!(coverage.event.id, seed.coverage_event_id().to_string());
    assert_eq!(result.event.id, seed.event_id().to_string());
    assert_eq!(coverage.seq, initial_head + 1);
    assert_eq!(result.seq, initial_head + 2);
    assert!(coverage.seq < result.seq, "coverage precedes the result");
    assert_eq!(store.head().unwrap(), initial_head + 2);
    assert_eq!(
        serde_json::to_value(&seen_coverage).unwrap(),
        serde_json::to_value(coverage).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&seen_result).unwrap(),
        serde_json::to_value(result).unwrap()
    );

    let worker = store.worker(seed.worker_id()).unwrap().unwrap();
    assert_eq!(worker.state, WorkerStateV1::Terminal);
    assert!(!worker.quota_held);
    assert_eq!(
        canonical(&worker).unwrap(),
        canonical(&worker_before).unwrap()
    );
    assert_eq!(
        canonical(&store.worker_quota(&policy).unwrap()).unwrap(),
        canonical(&quota_before).unwrap()
    );
    let allocations = store.worker_usage().unwrap();
    assert_eq!(allocations, usage_before);
    assert_eq!(allocations[0].attempt_id, seed.attempt_id());
    assert_eq!(allocations[0].source.container_id, seed.container_id());
    assert!(store.integrity().unwrap().valid);

    drop(service);
    drop(jobs);
    drop(enforcer_owner);
    eprintln!("cold_recovery_test phase=stop_only_owners_dropped");
    let (output_acknowledged, cancel_present, audit_seq, input_hash, coverage, arm_claim_snapshot) =
        pending_result_ack(seed).await;
    assert_eq!(output_acknowledged, seed.result_acknowledged());
    assert!(cancel_present);
    assert_eq!(input_hash, seed.event_input_hash());
    assert_eq!(audit_seq, result.seq);
    assert_eq!(coverage.0, seed.coverage_event_id());
    assert_eq!(coverage.1, after_coverage[0].seq);
    assert_eq!(coverage.2, seed.coverage_event_input_hash());
    assert_eq!(arm_claim_snapshot, *seed.arm_claim_snapshot());
    eprintln!("cold_recovery_test phase=reopened_ack_verified");
    Ok(())
}
