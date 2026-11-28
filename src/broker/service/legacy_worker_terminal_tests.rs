use super::super::*;
use super::{
    LegacyTerminalPublicationStateV1, ObservedLegacyWorkerTerminal, ValidatedLegacyWorkerTerminal,
    legacy_terminal_publication_state, observation_target_facts, require_archived_invocation,
    require_current_worker,
};
use crate::{
    audit::Store,
    broker::{
        checkpoint::SupervisionOperationV1,
        discovery::load_endpoint,
        journal::{
            StopJournal,
            recovery::legacy_worker_terminal::{
                LegacyWorkerTerminalEvidenceV1, ReservedLegacyWorkerTerminalObservationV1,
                RetainedLegacyWorkerTerminalObservationV1,
            },
        },
        writer::{CancelInputV1, StopWriterClient},
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
use tokio::sync::Notify;

tokio::task_local! {
    static TERMINAL_RECOVERY_PAUSE: (Arc<Notify>, Arc<Notify>);
}

pub(super) async fn pause_after_terminal_recovery_guard() {
    if let Ok((entered, resume)) = TERMINAL_RECOVERY_PAUSE.try_with(Clone::clone) {
        entered.notify_one();
        resume.notified().await;
    }
}

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
async fn legacy_terminal_publication_distinguishes_exact_parent_from_possible_replay() -> Result<()>
{
    use LegacyTerminalPublicationStateV1::{ExactParent, PossibleReplay};

    let seed = running_seed_for_cold_terminal_test().await;
    let operation = seed.operation_id;
    let root = Directory::open_existing(&seed.journal_root)?;
    let report = StopRecoveryReaderV1::open_existing(
        &root,
        seed.scope().clone(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new())?,
    )?
    .context("existing Legacy journal missing")?
    .scan();
    let capabilities =
        ProfileWriterCapabilities::from_recovery_report(&report, true, 1, Duration::from_secs(10))?;
    assert!(capabilities.is_stop_only());
    let stops = capabilities.stops();
    let command = Uuid::new_v4();
    stops
        .append_cancel(CancelInputV1::new(operation, command)?)
        .await?;
    let initial = stops
        .legacy_worker_terminal_evidence(operation, command)
        .await?;
    let mut observation = synthetic_terminal_observation(&initial)?;
    observation.restart_count = Some(0);
    let observed = ObservedLegacyWorkerTerminal {
        validated: ValidatedLegacyWorkerTerminal { evidence: initial },
        observation,
    };
    observed
        .validated
        .validate_terminal_observation(observed.observation())?;
    let request = observed.terminal_mutation()?;
    let terminal =
        crate::audit::expected_worker_transition(observed.evidence().worker(), &request)?;
    let stale = stops
        .legacy_worker_terminal_evidence(operation, command)
        .await?;
    let Ok(ReservedLegacyWorkerTerminalObservationV1::Appended(pending)) = stops
        .reserve_legacy_worker_terminal_observation(observed.validated.evidence, request)
        .await
    else {
        anyhow::bail!("fixture terminal pending missing");
    };
    let proof = stops
        .legacy_worker_terminal_evidence(operation, command)
        .await?;
    let other_stop = stops
        .legacy_worker_terminal_evidence(operation, seed.cancel_id)
        .await?;
    assert_eq!(proof.head(), pending.head());
    assert_eq!(other_stop.head(), pending.head());

    // The remaining checks use only snapshots. Record both stores' positions
    // after the test fixture has retained the exact pending body.
    let store = seed.store();
    let parent = store
        .worker(&seed.worker_id)?
        .context("fixture Worker missing")?;
    let core_head = store.head()?;
    let quota = canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?;
    let reads = seed.worker_get_count();
    let posts = seed.post_count();
    let calls = fs::read(seed.root().join("calls"))?;
    let classify = |worker| legacy_terminal_publication_state(&proof, &pending, worker);
    assert_eq!(classify(&parent), Ok(ExactParent));
    assert_eq!(classify(&terminal), Ok(PossibleReplay));
    assert_eq!(terminal.state, WorkerStateV1::Terminal);
    assert!(!terminal.quota_held);

    let mut wrong_cid = terminal.clone();
    wrong_cid.attempt.container_id = None;
    assert_eq!(classify(&wrong_cid), Err(BrokerErrorCodeV1::Conflict));
    let mut wrong_revision = terminal.clone();
    wrong_revision.revision += 1;
    assert_eq!(classify(&wrong_revision), Err(BrokerErrorCodeV1::Conflict));
    let mut wrong_observation = terminal.clone();
    wrong_observation.last_observation = parent.last_observation.clone();
    assert_eq!(
        classify(&wrong_observation),
        Err(BrokerErrorCodeV1::Conflict)
    );
    let mut wrong_parent = parent.clone();
    wrong_parent.revision += 1;
    assert_eq!(classify(&wrong_parent), Err(BrokerErrorCodeV1::Conflict));
    assert_eq!(
        legacy_terminal_publication_state(&stale, &pending, &parent),
        Err(BrokerErrorCodeV1::Conflict)
    );
    assert_eq!(
        legacy_terminal_publication_state(&other_stop, &pending, &terminal),
        Err(BrokerErrorCodeV1::Conflict)
    );

    assert_eq!(store.head()?, core_head);
    assert_eq!(
        canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?,
        quota
    );
    assert!(store.worker_usage()?.is_empty());
    assert_eq!(seed.worker_get_count(), reads);
    assert_eq!(seed.post_count(), posts);
    assert_eq!(fs::read(seed.root().join("calls"))?, calls);
    let after = stops
        .legacy_worker_terminal_observation(operation)
        .await?
        .context("fixture pending disappeared")?;
    assert_eq!(after.head(), pending.head());
    assert_eq!(canonical(after.request())?, canonical(pending.request())?);
    Ok(())
}

#[tokio::test]
async fn legacy_terminal_publication_replays_a_lost_core_reply_with_one_durable_effect()
-> Result<()> {
    async fn reopen_writer(
        service: &mut Service,
        journal_root: &Path,
        scope: &JournalScopeV1,
    ) -> Result<()> {
        use crate::broker::journal::JournalError;

        service.profiles.remove(scope.profile_id());
        let reopened = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match terminal_publication_stops(journal_root, scope) {
                    Err(error)
                        if error.downcast_ref::<JournalError>() == Some(&JournalError::Busy) =>
                    {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    result => break result,
                }
            }
        })
        .await??;
        let jobs = Arc::new(ProfileJobs::new());
        jobs.writer.set(reopened).expect("one reopened STOP writer");
        service.profiles.insert(scope.profile_id().to_owned(), jobs);
        Ok(())
    }

    let seed = running_seed_for_cold_terminal_test().await;
    let operation = seed.operation_id;
    let scope = seed.scope();
    let (mut service, writer, pending) = stage_terminal_publication(
        seed.root(),
        seed.config(),
        &seed.journal_root,
        scope,
        operation,
        seed.cancel_id,
    )
    .await?;
    let store = seed.store();
    let before = store.head()?;
    let parent = store
        .worker(&seed.worker_id)?
        .context("fixture worker missing")?;
    assert_eq!(
        store.worker_quota(&QuotaPolicyV1::default())?.held_workers,
        1
    );
    assert!(store.worker_usage()?.is_empty());
    let reads = seed.worker_get_count();
    let posts = seed.post_count();
    let calls_path = seed.root().join("calls");
    let calls = fs::read(&calls_path)?;

    let jobs = service.profile_jobs(scope.profile_id())?;
    let job = jobs.known(operation).context("retained Job missing")?;
    let entered = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let first = TERMINAL_RECOVERY_PAUSE.scope(
        (Arc::clone(&entered), Arc::clone(&resume)),
        service.reconcile_legacy_worker_terminal(scope, operation, pending.head()),
    );
    let concurrent = async {
        tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
        assert!(!job.is_running());
        assert!(matches!(
            tokio::time::timeout(
                Duration::from_secs(5),
                service.reconcile_legacy_worker_terminal(scope, operation, pending.head()),
            )
            .await?,
            Err(BrokerErrorCodeV1::Busy)
        ));
        assert_eq!(seed.worker_get_count(), reads);
        assert_eq!(seed.post_count(), posts);
        assert_eq!(fs::read(&calls_path)?, calls);

        // This fixture has no guardian. Cancel must publish its local receipt
        // while recovery holds the Job gate, then report failed delivery.
        let command = Uuid::new_v4();
        let generation = job.cancel.current();
        let stop_head = tokio::time::timeout(Duration::from_secs(5), async {
            assert!(matches!(
                service
                    .cancel(writer.clone(), Arc::clone(&jobs), operation, command)
                    .await,
                Err(BrokerErrorCodeV1::NeedsReconciliation)
            ));
            let stop = writer
                .cancel_stop_projection(operation, command)
                .await?
                .context("the STOP must be durable while terminal recovery is paused")?;
            assert_eq!(stop.operation_id(), operation);
            assert_eq!(canonical(stop.scope())?, canonical(scope)?);
            assert_eq!(stop.receipt().command_id(), command);
            assert_eq!(
                stop.receipt().head().revision(),
                pending.head().revision() + 1
            );
            assert!(job.has_acknowledged_cancel(command)?);
            assert!(job.cancel.current() > generation);
            Ok::<_, anyhow::Error>(stop.receipt().head().clone())
        })
        .await??;
        resume.notify_one();
        Ok::<_, anyhow::Error>(stop_head)
    };

    seed.arm_transition_response_cut_for_test();
    assert!(seed.transition_response_cut_pending_for_test());
    let ((), later_stop_head) = tokio::try_join!(
        async {
            assert!(matches!(
                first.await,
                Err(BrokerErrorCodeV1::NeedsReconciliation)
            ));
            Ok::<(), anyhow::Error>(())
        },
        concurrent
    )?;
    drop(jobs);
    assert!(
        !seed.transition_response_cut_pending_for_test(),
        "the successful transition handler must consume the response cut"
    );
    assert_eq!(seed.worker_get_count(), reads + 1);
    assert_eq!(seed.post_count(), posts + 1);
    let core_head = store.head()?;
    assert_ne!(core_head, before);
    let terminal = store
        .worker(&seed.worker_id)?
        .context("terminal Worker missing")?;
    assert_eq!(terminal.state, WorkerStateV1::Terminal);
    assert_eq!(terminal.revision, parent.revision + 1);
    assert_eq!(terminal.attempt, parent.attempt);
    assert!(!terminal.quota_held);
    let quota = store.worker_quota(&QuotaPolicyV1::default())?;
    assert_eq!(quota.held_workers, 0);
    let usage = store.worker_usage()?;
    assert_eq!(usage.len(), 1);
    assert_eq!(
        usage[0].terminal_observation_id,
        pending.request().operation_id
    );
    assert!(!usage[0].billable);
    let after_cut = writer
        .legacy_worker_terminal_observation(operation)
        .await?
        .context("the lost response must leave the exact pending request")?;
    assert_eq!(after_cut.head(), &later_stop_head);
    assert_eq!(after_cut.stop().command_id(), seed.cancel_id);
    assert_eq!(
        canonical(after_cut.request())?,
        canonical(pending.request())?
    );

    drop(writer);
    reopen_writer(&mut service, &seed.journal_root, scope).await?;
    assert!(matches!(
        service
            .reconcile_legacy_worker_terminal(scope, operation, pending.head())
            .await,
        Err(BrokerErrorCodeV1::NeedsReconciliation)
    ));
    let jobs = service.profile_jobs(scope.profile_id())?;
    assert!(jobs.known(operation).is_none());
    assert_eq!(seed.worker_get_count(), reads + 1);
    assert_eq!(seed.post_count(), posts + 1);
    jobs.job(operation)?;
    drop(jobs);
    let confirmed = service
        .reconcile_legacy_worker_terminal(scope, operation, pending.head())
        .await?;
    assert_eq!(confirmed.head().revision(), later_stop_head.revision() + 1);
    pending.validate_response(confirmed.response())?;
    assert_eq!(
        canonical(&confirmed.response().worker)?,
        canonical(&terminal)?
    );
    assert_eq!(seed.worker_get_count(), reads + 2);
    assert_eq!(seed.post_count(), posts + 2);
    assert_eq!(store.head()?, core_head);
    assert_eq!(
        canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?,
        canonical(&quota)?
    );
    assert_eq!(canonical(&store.worker_usage()?)?, canonical(&usage)?);

    reopen_writer(&mut service, &seed.journal_root, scope).await?;
    let _occupied_execution = service.execution.try_acquire()?;
    let cold = service
        .reconcile_legacy_worker_terminal(scope, operation, pending.head())
        .await?;
    assert_eq!(cold.head(), confirmed.head());
    assert_eq!(
        canonical(cold.response())?,
        canonical(confirmed.response())?
    );
    assert_eq!(seed.worker_get_count(), reads + 2);
    assert_eq!(seed.post_count(), posts + 2);
    assert_eq!(store.head()?, core_head);
    assert_eq!(canonical(&store.worker_usage()?)?, canonical(&usage)?);
    assert_eq!(fs::read(calls_path)?, calls);
    Ok(())
}

#[tokio::test]
async fn legacy_terminal_publication_refuses_a_divergent_core_worker_before_post() -> Result<()> {
    let seed = running_seed_for_cold_terminal_test().await;
    let operation = seed.operation_id;
    let scope = seed.scope();
    let (service, writer, pending) = stage_terminal_publication(
        seed.root(),
        seed.config(),
        &seed.journal_root,
        scope,
        operation,
        seed.cancel_id,
    )
    .await?;
    let mut other = pending.request().clone();
    other.operation_id = Uuid::new_v4().to_string();
    let WorkerTransitionV1::Observe { observation } = &mut other.transition else {
        anyhow::bail!("expected a terminal Observe in fixture");
    };
    observation.exit_code = Some(23);
    let store = seed.store();
    let other = store.mutate_worker(&other)?;
    assert_eq!(other.worker.state, WorkerStateV1::Terminal);
    let head = store.head()?;
    let quota = canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?;
    let usage = canonical(&store.worker_usage()?)?;
    let reads = seed.worker_get_count();
    let posts = seed.post_count();
    let calls_path = seed.root().join("calls");
    let calls = fs::read(&calls_path)?;
    assert!(matches!(
        service
            .reconcile_legacy_worker_terminal(scope, operation, pending.head())
            .await,
        Err(BrokerErrorCodeV1::Conflict)
    ));
    assert_eq!(seed.worker_get_count(), reads + 1);
    assert_eq!(seed.post_count(), posts);
    assert_eq!(store.head()?, head);
    assert_eq!(
        canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?,
        quota
    );
    assert_eq!(canonical(&store.worker_usage()?)?, usage);
    assert_eq!(fs::read(calls_path)?, calls);
    let after = writer
        .legacy_worker_terminal_observation(operation)
        .await?
        .context("divergence must keep the original pending")?;
    assert_eq!(after.head(), pending.head());
    assert_eq!(canonical(after.request())?, canonical(pending.request())?);
    Ok(())
}

fn terminal_publication_stops(
    root: &Path,
    scope: &JournalScopeV1,
) -> Result<ProfileWriterCapabilities> {
    let root = Directory::open_existing(root)?;
    let report = StopRecoveryReaderV1::open_existing(
        &root,
        scope.clone(),
        &FsExclusions::new(),
        JournalLimitsV1::default(),
        ControlSecrets::new(Vec::new())?,
    )?
    .context("retained Legacy lease missing")?
    .scan();
    let capabilities =
        ProfileWriterCapabilities::from_recovery_report(&report, true, 1, Duration::from_secs(10))?;
    assert!(capabilities.is_stop_only());
    Ok(capabilities)
}

async fn stage_terminal_publication(
    root: &Path,
    config: &Path,
    journal_root: &Path,
    scope: &JournalScopeV1,
    operation: Uuid,
    command: Uuid,
) -> Result<(
    Service,
    StopWriterClient,
    RetainedLegacyWorkerTerminalObservationV1,
)> {
    let capabilities = terminal_publication_stops(journal_root, scope)?;
    let writer = capabilities.stops();
    let evidence = writer
        .legacy_worker_terminal_evidence(operation, command)
        .await?;
    let mut service = read_only_service(root, config, &evidence)?;
    let jobs = Arc::new(ProfileJobs::new());
    jobs.writer
        .set(capabilities)
        .expect("one retained STOP writer");
    jobs.job(operation)?;
    service.profiles.insert(scope.profile_id().to_owned(), jobs);
    let mut observation = synthetic_terminal_observation(&evidence)?;
    observation.restart_count = Some(0);
    let observed = ObservedLegacyWorkerTerminal {
        validated: ValidatedLegacyWorkerTerminal { evidence },
        observation,
    };
    observed
        .validated
        .validate_terminal_observation(observed.observation())?;
    let request = observed.terminal_mutation()?;
    let Ok(ReservedLegacyWorkerTerminalObservationV1::Appended(pending)) = writer
        .reserve_legacy_worker_terminal_observation(observed.validated.evidence, request)
        .await
    else {
        anyhow::bail!("test could not retain terminal observation");
    };
    Ok((service, writer, pending))
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
    jobs.job(seed.operation_id)?;
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
            .reconcile_legacy_worker_terminal(proof.scope(), operation, proof.head())
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
    let after_restore = fs::read(&calls)?;

    let store = seed.store();
    let core_head = store.head()?;
    let quota = store.worker_quota(&QuotaPolicyV1::default())?;
    let reads = seed.worker_get_count();
    let posts = seed.post_count();
    let mut observed = service
        .observe_legacy_worker_terminal(&stops, &proof)
        .await?;
    let after_observe = fs::read(&calls)?;
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

    // The private flow uses the same guarded read, then reserves and publishes
    // the observation before returning its exact Core receipt.
    let confirmed = service
        .reconcile_legacy_worker_terminal(proof.scope(), operation, proof.head())
        .await?;
    let after_reconcile = fs::read(&calls)?;
    assert_eq!(confirmed.head().revision(), proof.head().revision() + 2);
    assert_eq!(
        confirmed.response().receipt.operation_id,
        mutation.operation_id
    );
    assert_eq!(confirmed.response().worker.attempt, proof.worker().attempt);
    assert_eq!(confirmed.response().worker.state, WorkerStateV1::Terminal);
    assert_eq!(
        confirmed
            .response()
            .worker
            .last_observation
            .as_ref()
            .and_then(|value| value.exit_code),
        Some(17)
    );
    assert_eq!(seed.worker_get_count(), reads + 5);
    assert_eq!(seed.post_count(), posts + 1);
    assert_eq!(
        store.worker_quota(&QuotaPolicyV1::default())?.held_workers,
        0
    );
    assert_eq!(store.worker_usage()?.len(), 1);
    assert_eq!(
        canonical(
            &store
                .worker(&seed.worker_id)?
                .context("terminal Worker missing")?
        )?,
        canonical(&confirmed.response().worker)?
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
    let check_command = |line: &str| {
        if line == "info" || line.starts_with("inspect ") {
            return true;
        }
        let verb = match line.split_ascii_whitespace().next() {
            Some("kill") => "kill",
            Some("rm") => "rm",
            Some("ps") => "ps",
            Some("start") => "start",
            Some("create") => "create",
            Some("image") => "image",
            Some("volume") => "volume",
            _ => "unknown",
        };
        // The fixture appends commands. Locate the earliest snapshot with
        // this rejected command.
        let phase = [
            ("pre_latch", before_latch.as_slice()),
            ("restore", after_restore.as_slice()),
            ("observe", after_observe.as_slice()),
            ("reconcile", after_reconcile.as_slice()),
        ]
        .into_iter()
        .find_map(|(phase, snapshot)| {
            let snapshot = std::str::from_utf8(snapshot).ok()?;
            snapshot
                .lines()
                .any(|earlier| earlier == line)
                .then_some(phase)
        })
        .unwrap_or("drop");
        eprintln!("g09_unexpected_command={verb}:{phase}");
        false
    };
    assert!(commands.lines().all(check_command));
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
