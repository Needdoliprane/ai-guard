use super::super::*;
use super::{require_archived_invocation, require_current_worker};
use crate::{
    audit::Store,
    broker::{
        checkpoint::SupervisionOperationV1,
        journal::{StopJournal, recovery::legacy_worker_terminal::LegacyWorkerTerminalEvidenceV1},
    },
    control::{QuotaPolicyV1, RuntimeStatusV1, WorkerStateV1, WorkerTransitionV1, WorkerV1},
    runner::broker::{
        ColdRecoverySeed, running_seed_for_cold_terminal_test,
        terminal_pending_seed_for_cold_audit_test,
    },
    runtime::{ContainerObservation, RuntimeKind},
};
use anyhow::Context;
use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt, path::Path};

#[tokio::test]
async fn legacy_terminal_target_requires_the_same_current_running_worker() -> Result<()> {
    let seed = terminal_pending_seed_for_cold_audit_test().await;
    let (proof, running) = retained_running(&seed)?;
    assert_eq!(running.state, WorkerStateV1::Running);
    assert!(running.quota_held);
    require_current_worker(proof.worker(), &running)?;

    let mut changed_revision = running.clone();
    changed_revision.revision += 1;
    assert_eq!(
        require_current_worker(proof.worker(), &changed_revision),
        Err(BrokerErrorCodeV1::Conflict)
    );
    let mut changed_cid = running.clone();
    changed_cid.attempt.container_id = Some("f".repeat(64));
    assert_eq!(
        require_current_worker(proof.worker(), &changed_cid),
        Err(BrokerErrorCodeV1::Conflict)
    );
    let mut unheld = running.clone();
    unheld.quota_held = false;
    assert_eq!(
        require_current_worker(proof.worker(), &unheld),
        Err(BrokerErrorCodeV1::NeedsReconciliation)
    );

    let service = read_only_service(seed.root(), seed.config(), &proof)?;
    let profile = service.registry.profile(seed.scope().profile_id())?;
    require_archived_invocation(profile, &proof)?;
    let mut other_program = profile.clone();
    other_program.runtime_program = seed.root().join("different-engine");
    assert!(matches!(
        require_archived_invocation(&other_program, &proof),
        Err(BrokerErrorCodeV1::Conflict)
    ));

    // Core already committed the terminal mutation whose reply the fixture
    // lost. A successful identity/read exchange still supplies no new target.
    let store = Store::open_existing_for_tenant(
        seed.database(),
        seed.scope().tenant_id(),
        Redactor::new(Vec::new()),
        1000,
    )?;
    let committed = store.worker(seed.worker_id())?.unwrap();
    assert_eq!(committed.state, WorkerStateV1::Terminal);
    assert!(!committed.quota_held);
    let head = store.head()?;
    let runtime_calls = seed.runtime_calls();
    assert!(matches!(
        service.validate_legacy_worker_terminal(proof).await,
        Err(BrokerErrorCodeV1::NeedsReconciliation)
    ));
    assert_eq!(store.head()?, head);
    assert_eq!(seed.runtime_calls(), runtime_calls);
    Ok(())
}

#[tokio::test]
async fn legacy_terminal_target_reads_the_exact_running_worker_without_effects() -> Result<()> {
    let seed = running_seed_for_cold_terminal_test().await;
    let journal = open_recovery_journal(&seed.journal_root, seed.scope())?;
    let stops = StopJournal::from_full(&journal);
    let proof = stops.legacy_worker_terminal_evidence(seed.operation_id, seed.cancel_id)?;
    assert_eq!(proof.head(), proof.cancel_receipt().head());
    let service = read_only_service(seed.root(), seed.config(), &proof)?;
    let head = proof.head().clone();
    let expected = observation_target_facts(&proof)?;

    let store = seed.store();
    let worker = store
        .worker(&seed.worker_id)?
        .context("current Running Worker missing")?;
    assert_eq!(worker.state, WorkerStateV1::Running);
    assert!(worker.quota_held);
    assert_eq!(worker.admission_operation_id, seed.operation_id.to_string());
    let core_head = store.head()?;
    let quota = store.worker_quota(&QuotaPolicyV1::default())?;
    assert_eq!(quota.held_workers, 1);
    assert_eq!(quota.held_cpu_millis, u64::from(worker.launch.cpu_millis));
    assert_eq!(quota.held_memory_bytes, worker.launch.memory_bytes);
    assert!(store.worker_usage()?.is_empty());
    let reads = seed.worker_get_count();
    let posts = seed.post_count();
    let calls = std::fs::read(seed.root().join("calls"))?;

    let validated = service.validate_legacy_worker_terminal(proof).await?;
    let actual = validated.evidence();
    assert_eq!(actual.operation_id(), seed.operation_id);
    assert_eq!(actual.cancel_receipt().command_id(), seed.cancel_id);
    assert!(
        observation_target_facts(actual)? == expected,
        "the authenticated read must retain the original scope, STOP, Arm, claim, authority and Worker"
    );
    assert!(canonical(actual.worker())? == canonical(&worker)?);
    assert_eq!(seed.worker_get_count(), reads + 1);
    assert_eq!(seed.post_count(), posts);
    assert_eq!(store.head()?, core_head);
    assert!(
        canonical(&store.worker_quota(&QuotaPolicyV1::default())?)? == canonical(&quota)?,
        "a read preserves the exact allocation held for this Worker"
    );
    assert!(store.worker_usage()?.is_empty());
    assert!(
        canonical(
            &store
                .worker(&seed.worker_id)?
                .context("Worker disappeared after GET")?
        )? == canonical(&worker)?
    );
    let after = stops
        .recovery_status(seed.operation_id)?
        .context("journal disappeared after GET")?;
    assert_eq!(after.head(), &head);
    assert!(service.fenced.load(Ordering::Acquire));
    assert!(std::fs::read(seed.root().join("calls"))? == calls);
    Ok(())
}

#[tokio::test]
async fn legacy_terminal_observation_must_match_the_retained_created_and_claim_facts() -> Result<()>
{
    let seed = running_seed_for_cold_terminal_test().await;
    let journal = open_recovery_journal(&seed.journal_root, seed.scope())?;
    let proof = StopJournal::from_full(&journal)
        .legacy_worker_terminal_evidence(seed.operation_id, seed.cancel_id)?;
    let service = read_only_service(seed.root(), seed.config(), &proof)?;
    let validated = service.validate_legacy_worker_terminal(proof).await?;

    let store = seed.store();
    let core_head = store.head()?;
    let posts = seed.post_count();
    let calls = std::fs::read(seed.root().join("calls"))?;
    let terminal = synthetic_terminal_observation(validated.evidence())?;
    assert!(terminal.configuration_fingerprint.as_ref().is_some());
    let _: () = validated.validate_terminal_observation(&terminal)?;

    let mut missing_start = terminal.clone();
    missing_start.process.started_at = None;
    assert_eq!(
        validated.validate_terminal_observation(&missing_start),
        Err(BrokerErrorCodeV1::NeedsReconciliation)
    );
    let mut missing_finish = terminal.clone();
    missing_finish.process.finished_at = None;
    assert_eq!(
        validated.validate_terminal_observation(&missing_finish),
        Err(BrokerErrorCodeV1::NeedsReconciliation)
    );
    let mut indeterminate = terminal.clone();
    indeterminate.process.status = Some("running".into());
    indeterminate.process.running = Some(true);
    assert_eq!(
        validated.validate_terminal_observation(&indeterminate),
        Err(BrokerErrorCodeV1::NeedsReconciliation)
    );

    let mut changed_start = terminal.clone();
    changed_start.process.started_at = changed_start
        .process
        .started_at
        .map(|started| started + chrono::Duration::seconds(1));
    assert_eq!(
        validated.validate_terminal_observation(&changed_start),
        Err(BrokerErrorCodeV1::Conflict)
    );
    let mut changed_cid = terminal.clone();
    changed_cid.id = Some("f".repeat(64));
    assert_eq!(
        validated.validate_terminal_observation(&changed_cid),
        Err(BrokerErrorCodeV1::Conflict)
    );
    let mut changed_configuration = terminal.clone();
    changed_configuration.configuration_fingerprint = Some("e".repeat(64));
    assert_eq!(
        validated.validate_terminal_observation(&changed_configuration),
        Err(BrokerErrorCodeV1::Conflict)
    );
    let mut changed_allocation = terminal.clone();
    changed_allocation.allocation.memory_bytes = changed_allocation
        .allocation
        .memory_bytes
        .map(|value| value + 1);
    assert_eq!(
        validated.validate_terminal_observation(&changed_allocation),
        Err(BrokerErrorCodeV1::Conflict)
    );
    let mut reversed_time = terminal.clone();
    reversed_time.process.finished_at = reversed_time
        .process
        .started_at
        .map(|started| started - chrono::Duration::seconds(1));
    assert_eq!(
        validated.validate_terminal_observation(&reversed_time),
        Err(BrokerErrorCodeV1::Conflict)
    );
    let mut observed_before_finish = terminal.clone();
    observed_before_finish.observed_at -= chrono::Duration::seconds(1);
    assert_eq!(
        validated.validate_terminal_observation(&observed_before_finish),
        Err(BrokerErrorCodeV1::Conflict)
    );

    assert_eq!(store.head()?, core_head);
    assert_eq!(seed.post_count(), posts);
    assert_eq!(std::fs::read(seed.root().join("calls"))?, calls);
    Ok(())
}

fn synthetic_terminal_observation(
    proof: &LegacyWorkerTerminalEvidenceV1,
) -> Result<ContainerObservation> {
    let (authority, created) = proof.target();
    let start = proof
        .worker()
        .observed_started_at
        .as_deref()
        .context("the retained Running Worker has no start instant")?;
    let started_at = chrono::DateTime::parse_from_rfc3339(start)?;
    let finished_at = started_at + chrono::Duration::seconds(2);
    let mut observed = created.observation.clone();
    observed.process.status = Some(
        match authority.kind {
            RuntimeKind::Docker | RuntimeKind::Podman => "exited",
        }
        .into(),
    );
    observed.process.running = Some(false);
    observed.process.started_at = Some(started_at);
    observed.process.finished_at = Some(finished_at);
    observed.process.exit_code = Some(0);
    observed.process.oom_killed = Some(false);
    observed.observed_at = finished_at.with_timezone(&chrono::Utc);
    Ok(observed)
}

fn observation_target_facts(proof: &LegacyWorkerTerminalEvidenceV1) -> Result<Vec<u8>> {
    let stop = proof.cancel_receipt();
    Ok(canonical(&(
        proof.scope(),
        proof.head(),
        stop.command_id(),
        stop.input_hash(),
        stop.head(),
        proof.arm(),
        proof.claim(),
        proof.target(),
        proof.worker(),
    ))?)
}

fn open_recovery_journal(root: &Path, scope: &JournalScopeV1) -> Result<Journal> {
    let root = Directory::open_existing(root)?;
    Ok(Journal::open(
        &root,
        scope.clone(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new())?,
    )?)
}

fn retained_running(seed: &ColdRecoverySeed) -> Result<(LegacyWorkerTerminalEvidenceV1, WorkerV1)> {
    let journal = open_recovery_journal(seed.journal_root(), seed.scope())?;
    let job = journal.load(seed.operation_id())?.unwrap();
    let running = job
        .broker_checkpoint_v2()
        .unwrap()
        .supervision()
        .operations()
        .iter()
        .find_map(|operation| match operation {
            SupervisionOperationV1::WorkerMutation {
                request,
                response: Some(result),
            } if matches!(&request.transition, WorkerTransitionV1::Observe { observation }
                if observation.status == RuntimeStatusV1::Running) =>
            {
                Some(result.worker.clone())
            }
            _ => None,
        })
        .unwrap();
    let proof = StopJournal::from_full(&journal)
        .legacy_worker_terminal_evidence(seed.operation_id(), seed.cancel_id())?;
    Ok((proof, running))
}

fn read_only_service(
    root: &Path,
    config: &Path,
    proof: &LegacyWorkerTerminalEvidenceV1,
) -> Result<Service> {
    let (authority, _) = proof.target();
    let invocation = authority.invocation.as_ref().unwrap();
    let scope = proof.scope();
    let registry_path = config.join("terminal-reader-registry.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&registry_path)?;
    file.write_all(&serde_json::to_vec(&json!({
        "version":1,
        "installation_id":scope.installation_id(),
        "root":root,
        "max_enforcer_slots":1,
        "profiles":[{
            "id":scope.profile_id(),
            "gateway":scope.gateway_origin(),
            "tenant_id":scope.tenant_id(),
            "control_namespace_uuid":scope.control_namespace_uuid(),
            "config_dir":config,
            "runner_token_file":config.join("reconciler.token"),
            "runtime":authority.kind,
            "runtime_program":invocation.approved_entry,
            "runtime_endpoint":authority.endpoint,
            "image":proof.worker().launch.image_ref,
            "policy_profile":proof.worker().launch.profile,
            "network":"none",
            "allow_rootful_runtime":false,
            "max_timeout_seconds":600,
            "allow_capture_output":false,
            "managed_clients":false,
            "source_roots":[root.join("sources")],
            "export_roots":[root.join("exports")]
        }]
    }))?)?;
    file.sync_all()?;
    let registry = Arc::new(Registry::load(&registry_path)?);
    let material = Arc::new(Material::load(&registry)?);
    Ok(Service {
        registry,
        material,
        root: Directory::open_existing(root)?,
        profiles: BTreeMap::new(),
        enforcer: None,
        stop_enforcer: None,
        blocking: Arc::new(Semaphore::new(1)),
        preparation: Arc::new(Semaphore::new(1)),
        execution: Arc::new(Semaphore::new(1)),
        cold_owners: retained_audit::ColdOwners::new(1)?,
        full_owners: FullOwners::new(1)?,
        stopping: Arc::new(AtomicBool::new(false)),
        fenced: Arc::new(AtomicBool::new(true)),
    })
}
