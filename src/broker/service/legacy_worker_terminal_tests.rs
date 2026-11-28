use super::super::*;
use super::{observation_target_facts, require_archived_invocation, require_current_worker};
use crate::{
    audit::Store,
    broker::{
        checkpoint::SupervisionOperationV1,
        discovery::load_endpoint,
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
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    process::{Child, Command, Stdio},
};

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

#[tokio::test]
async fn cold_legacy_terminal_read_requires_the_exact_latch_and_preserves_state() -> Result<()> {
    let seed = running_seed_for_cold_terminal_test().await;
    let root = Directory::open_existing(&seed.journal_root)?;
    let report = Arc::new(
        StopRecoveryReaderV1::open_existing(
            &root,
            seed.scope().clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(Vec::new())?,
        )?
        .context("existing Legacy journal missing")?
        .scan(),
    );
    let capabilities =
        ProfileWriterCapabilities::from_recovery_report(&report, true, 1, Duration::from_secs(10))?;
    assert!(capabilities.is_stop_only());
    let stops = capabilities.stops();
    let proof = stops
        .legacy_worker_terminal_evidence(seed.operation_id, seed.cancel_id)
        .await?;
    let profile_id = proof.scope().profile_id().to_owned();
    let (authority, owned) = proof.target();
    assert_eq!(authority.kind, RuntimeKind::Docker);
    let cid = owned.handle.id().to_owned();
    let start = chrono::DateTime::parse_from_rfc3339(
        proof
            .worker()
            .observed_started_at
            .as_deref()
            .context("worker start missing")?,
    )?;
    let finish = start + chrono::Duration::seconds(3);
    // Only the existing synthetic CLI sees this terminal fact. The Service
    // still obtains it through bounded inspect of the retained CID.
    let inspect_path = seed.root().join("inspect.json");
    let mut inspect: serde_json::Value = serde_json::from_slice(&fs::read(&inspect_path)?)?;
    inspect["State"]["Status"] = json!("exited");
    inspect["State"]["Running"] = json!(false);
    inspect["State"]["FinishedAt"] = json!(finish.to_rfc3339());
    inspect["State"]["ExitCode"] = json!(17);
    fs::write(inspect_path, serde_json::to_vec(&inspect)?)?;

    let (guardian, peer) = start_legacy_stop_guardian().await?;
    let mut service = read_only_service(seed.root(), seed.config(), &proof)?;
    let jobs = Arc::new(ProfileJobs::new());
    jobs.writer
        .set(capabilities)
        .expect("one retained STOP writer");
    service.profiles.insert(profile_id.clone(), jobs);
    service.stop_enforcer = Some(guardian.clone());
    let operation = seed.operation_id;
    let command = seed.cancel_id;
    let calls = seed.root().join("calls");
    let before_latch = fs::read(&calls)?;
    assert!(matches!(
        service
            .observe_legacy_worker_terminal(&profile_id, operation, command)
            .await,
        Err(BrokerErrorCodeV1::NeedsReconciliation)
    ));
    assert!(fs::read(&calls)? == before_latch);

    let job_index = report
        .jobs()
        .iter()
        .position(|job| job.operation_id() == operation)
        .context("STOP report missing the retained operation")?;
    let expected_targets = report.jobs()[job_index].stops().len();
    assert!(expected_targets > 0);
    // Establish the durable STOP latch through the existing Stops2 actor.
    // The guardian sees an already terminal synthetic worker after restore.
    let (restored, partial) = guardian
        .restore_report_job(Arc::clone(&report), job_index, 1)
        .await;
    assert!(!partial);
    assert_eq!(restored, expected_targets);

    let store = seed.store();
    let core_head = store.head()?;
    let quota = store.worker_quota(&QuotaPolicyV1::default())?;
    let reads = seed.worker_get_count();
    let posts = seed.post_count();
    let mut observed = service
        .observe_legacy_worker_terminal(&profile_id, operation, command)
        .await?;
    let terminal = observed.observation();
    assert_eq!(terminal.id.as_deref(), Some(cid.as_str()));
    assert_eq!(terminal.process.finished_at, Some(finish));
    assert_eq!(terminal.process.exit_code, Some(17));
    assert!(observation_target_facts(observed.evidence())? == observation_target_facts(&proof)?);
    let mutation = observed.terminal_mutation()?;
    let same_mutation = observed.terminal_mutation()?;
    assert_eq!(canonical(&mutation)?, canonical(&same_mutation)?);
    assert_ne!(mutation.operation_id, proof.claim().1.operation_id);
    assert_ne!(mutation.operation_id, command.to_string());
    let projected = crate::audit::expected_worker_transition(proof.worker(), &mutation)?;
    assert_eq!(projected.state, WorkerStateV1::Terminal);
    assert!(!projected.quota_held);
    assert_eq!(
        projected.attempt.container_id.as_deref(),
        Some(cid.as_str())
    );
    assert_eq!(
        projected
            .last_observation
            .as_ref()
            .and_then(|value| value.exit_code),
        Some(17)
    );
    for restart_count in [None, Some(1)] {
        observed.observation.restart_count = restart_count;
        assert!(matches!(
            observed.terminal_mutation(),
            Err(BrokerErrorCodeV1::NeedsReconciliation)
        ));
    }
    assert_eq!(seed.worker_get_count(), reads + 2);
    assert_eq!(seed.post_count(), posts);
    assert_eq!(store.head()?, core_head);
    assert!(canonical(&store.worker_quota(&QuotaPolicyV1::default())?)? == canonical(&quota)?);
    assert!(store.worker_usage()?.is_empty());
    assert_eq!(
        stops
            .recovery_status(operation)
            .await?
            .context("Legacy journal disappeared")?
            .head(),
        proof.head()
    );
    drop(service);
    drop(guardian);
    drop(peer);
    let commands = fs::read_to_string(calls)?;
    assert!(
        commands
            .lines()
            .any(|line| line.strip_prefix("inspect ") == Some(cid.as_str()))
    );
    assert!(
        commands
            .lines()
            .all(|line| line == "info" || line.starts_with("inspect "))
    );
    Ok(())
}

struct LegacyStopPeer {
    child: Child,
    _directory: tempfile::TempDir,
}

impl Drop for LegacyStopPeer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn start_legacy_stop_guardian() -> Result<(EnforcerStopClient, LegacyStopPeer)> {
    let directory = tempfile::Builder::new()
        .prefix("aig-g09-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(std::env::temp_dir().canonicalize()?)?;
    let root = Directory::open_existing(directory.path())?;
    anyhow::ensure!(
        root.path()
            .join("endpoints/i-0000000000000000/s")
            .as_os_str()
            .as_encoded_bytes()
            .len()
            <= 103,
        "Legacy STOP fixture requires a short private TMPDIR"
    );
    root.persistent_create("auth", &[0xa5; 32])?;
    root.persistent_create(".owner.lock", &[])?;
    root.persistent_child("endpoints", true)?
        .context("fixture endpoints missing")?;
    root.persistent_child("announcements", true)?
        .context("fixture announcements missing")?;
    let executable = std::env::current_exe()?;
    let child = Command::new(&executable)
        .args([
            "--exact",
            "broker::service::server::cancel_retry_tests::native_recovery_server_child",
            "--ignored",
        ])
        .env_clear()
        .env("AI_GUARD_TEST_RECOVERY_ROOT", root.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut peer = LegacyStopPeer {
        child,
        _directory: directory,
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            anyhow::ensure!(
                peer.child.try_wait()?.is_none(),
                "Legacy STOP fixture child exited"
            );
            if load_endpoint(&root)?.is_some() {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    let client = EnforcerStopClient::connect_or_spawn_for_recovery(&root, &executable, 8).await?;
    Ok((client, peer))
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
