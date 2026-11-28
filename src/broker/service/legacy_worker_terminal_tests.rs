use super::super::*;
use super::{require_archived_invocation, require_current_worker};
use crate::{
    audit::Store,
    broker::{
        checkpoint::SupervisionOperationV1,
        journal::{StopJournal, recovery::legacy_worker_terminal::LegacyWorkerTerminalEvidenceV1},
    },
    control::{RuntimeStatusV1, WorkerStateV1, WorkerTransitionV1, WorkerV1},
    runner::broker::{ColdRecoverySeed, terminal_pending_seed_for_cold_audit_test},
};
use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};

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

    let service = read_only_service(&seed, &proof)?;
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

fn retained_running(seed: &ColdRecoverySeed) -> Result<(LegacyWorkerTerminalEvidenceV1, WorkerV1)> {
    let root = Directory::open_existing(seed.journal_root())?;
    let journal = Journal::open(
        &root,
        seed.scope().clone(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new())?,
    )?;
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
    seed: &ColdRecoverySeed,
    proof: &LegacyWorkerTerminalEvidenceV1,
) -> Result<Service> {
    let (authority, _) = proof.target();
    let invocation = authority.invocation.as_ref().unwrap();
    let root = seed.root();
    let config = seed.config();
    let scope = seed.scope();
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
