//! Retained Service recovery over real Journal/Writer, gateway SQLite and
//! authenticated native IPC. SyntheticReady supplies no engine observation.
use super::*;
use crate::broker::{
    enforcer::EnforcedSlot,
    journal::{JournalError, retirement_live_fixture::LiveFixture},
    service::preclaim::{AbandonBeforeClaimV1, RetainedRetirementV1},
    writer::PendingUnusedRetirement,
};

#[path = "recovery_status_compatibility_tests.rs"]
mod recovery_status_compatibility_tests;

#[path = "recovery_status_v2_tests.rs"]
mod recovery_status_v2_tests;

fn diagnostic_phase(label: &'static str) {
    eprintln!("d5-service-recovery phase={label}");
}

fn diagnostic_future_storage<F: std::future::Future>(label: &'static str, future: &F) {
    // This measures the Future value's storage, not the stack used by poll.
    eprintln!(
        "d5-service-recovery phase={label} measurement=future_storage bytes={}",
        std::mem::size_of_val(future)
    );
}

// Construct the large case future inside a separate call boundary. The
// enclosing test polls while the synchronous Journal/SQLite fixture runs;
// constructing an inline async block there retains its aggregate storage in
// that caller even if the already-constructed future is boxed afterwards.
#[inline(never)]
fn boxed_recovery_phase<F: std::future::Future>(
    construct: impl FnOnce() -> F,
) -> std::pin::Pin<Box<F>> {
    Box::pin(construct())
}

async fn audited_pending(
    writer: &WriterClient,
    client: &AuditClient,
    intent: &ExecutionIntentV1,
    full: &EnforcerFullClientV3,
    slot: &EnforcedSlot,
) -> Result<PendingUnusedRetirement> {
    let operation = intent.operation_id();
    let retirement = Uuid::new_v4();
    let arm = slot.full3_registration_binding()?.arm_id();
    audit_abandonment(writer, client, intent, retirement, arm, full.epoch()).await?;
    let expected = writer
        .load(operation)
        .await?
        .ok_or_else(|| anyhow!("actual Pending fixture job missing"))?
        .head()
        .clone();
    let pending = writer
        .begin_unused_retirement(BeginUnusedRetirement::new(
            operation,
            expected,
            slot.revoke_unused(retirement).await?,
        )?)
        .await?;
    let sequence = client
        .post_control_event(serde_json::to_value(pending.intention_event())?)
        .await?;
    writer
        .acknowledge_event(operation, pending.intention_event_id(), sequence)
        .await?;
    Ok(pending)
}

async fn actual_report(root: &Path, scope: &JournalScopeV1) -> Result<StopRecoveryReportV1> {
    let directory = Directory::open_existing(&root.join("journals").join(scope.profile_id()))?;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match StopRecoveryReaderV1::open_existing(
                &directory,
                scope.clone(),
                &FsExclusions::new(),
                JournalLimitsV1::new(256 * 1024 * 1024, 256, 256, 512 * 1024 * 1024, 256, 128)?,
                ControlSecrets::new(Vec::new())?,
            ) {
                Ok(Some(reader)) => break Ok(reader.scan()),
                Err(JournalError::Busy) => tokio::task::yield_now().await,
                _ => break Err(anyhow!("actual retained Service reader unavailable")),
            }
        }
    })
    .await?
}

fn retained_service(
    root: &Path,
    registry: Arc<Registry>,
    jobs: Arc<ProfileJobs>,
    enforcer: Option<ExecutionEnforcerV1>,
    stop_enforcer: Option<EnforcerStopClient>,
) -> Result<Arc<Service>> {
    let material = Arc::new(Material::load(&registry)?);
    let profile = registry.data.profiles[0].id.clone();
    Ok(Arc::new(Service {
        full_owners: FullOwners::new(registry.data.max_enforcer_slots as usize)?,
        cold_owners: retained_audit::ColdOwners::new(registry.data.max_enforcer_slots as usize)?,
        registry,
        root: Directory::open_existing(root)?,
        material,
        profiles: BTreeMap::from([(profile, jobs)]),
        enforcer: enforcer.map(ServiceEnforcer::Native),
        stop_enforcer,
        blocking: Arc::new(Semaphore::new(2)),
        preparation: Arc::new(Semaphore::new(1)),
        execution: Arc::new(Semaphore::new(1)),
        stopping: Arc::new(AtomicBool::new(false)),
        fenced: Arc::new(AtomicBool::new(true)),
    }))
}

async fn close_service(service: &Service) -> Result<()> {
    let reconciler = tokio::spawn(std::future::pending());
    let (presence, _) = watch::channel(false);
    service
        .shutdown(
            reconciler,
            &presence,
            &mut JoinSet::new(),
            &mut JoinSet::new(),
        )
        .await?;
    ensure!(
        service.stopping.load(Ordering::Acquire)
            && service.full_owners.try_reserve().is_err()
            && service.cold_owners.try_reserve().is_err()
            && service.has_retained_retirement()
            && service.require_starts().await.is_err(),
        "actual shutdown must retain its original D5 owner with admission closed"
    );
    Ok(())
}

fn cancel_envelope(profile: &str, operation: Uuid, command: Uuid) -> Result<EnvelopeV1> {
    // Use the closed production envelope codec, never a private handler call.
    Ok(serde_json::from_slice(&serde_json::to_vec(&EnvelopeV1 {
        version: 1,
        profile_id: profile.into(),
        credential: RUNNER.into(),
        request: BrokerCommandV1::Cancel {
            operation_id: operation,
            command_id: command,
        },
    })?)?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retained_shutdown_accepts_fresh_authenticated_cancel_and_delivers_exact_stop() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    diagnostic_phase("cancel.fixture.before");
    let LiveFixture {
        _root,
        _store_root,
        store,
        journal,
        intent,
        head,
        registration,
    } = live_fixture_with_gateway(&origin, true);
    diagnostic_phase("cancel.fixture.after");
    let scope = journal.scope().clone();
    let store = Arc::new(store);
    let observations = Arc::new(GatewayObservations::default());
    diagnostic_phase("cancel.gateway.before");
    let gateway_future = OwnedGateway::start(
        listener,
        Arc::clone(&store),
        scope.tenant_id(),
        Arc::clone(&observations),
    );
    diagnostic_future_storage("cancel.gateway.future", &gateway_future);
    let gateway = gateway_future.await.unwrap();
    diagnostic_phase("cancel.gateway.after");
    let registration_hash = registration.digest().unwrap();
    let worker = registration.arm().worker_id.clone();
    diagnostic_phase("cancel.register.before");
    let register_future = OriginalUnusedFixture::register(scope.installation_id(), registration);
    diagnostic_future_storage("cancel.register.future", &register_future);
    let (native, full, slot) = register_future.await.unwrap();
    diagnostic_phase("cancel.register.after");
    let outcome_future = boxed_recovery_phase(|| async {
        diagnostic_phase("cancel.case.enter");
        let operation = intent.operation_id();
        let writer = WriterClient::start(journal.clone(), 16, Duration::from_secs(10))?;
        let arm = slot.full3_registration_binding()?.arm_id();
        writer
            .bind_full3_registration(operation, head, slot.full3_registration_binding()?)
            .await?;
        let pending = audited_pending(
            &writer,
            &AuditClient::new(&origin, RUNNER)?,
            &intent,
            &full,
            &slot,
        )
        .await?;
        let identity = pending.identity().clone();
        // Retain the genuine producer's Pending, before authorize/Retire. The
        // original Arm still needs STOP; no Q/Final or terminal is fabricated.
        drop(writer);
        drop(journal);
        let root = _root.path().canonicalize()?;
        let report = actual_report(&root, &scope).await?;
        let capabilities = ProfileWriterCapabilities::from_recovery_report(
            &report,
            true,
            16,
            Duration::from_secs(10),
        )?;
        ensure!(
            capabilities.is_stop_only(),
            "recovery must grant only Stops"
        );
        let stops = capabilities.stops();
        let jobs = Arc::new(ProfileJobs::new());
        jobs.writer
            .set(capabilities)
            .map_err(|_| anyhow!("single retained Service factory"))?;
        let local = jobs.job(operation)?;
        local
            .preclaim
            .announce_registration(operation, arm, full.epoch(), 0)?;
        local
            .preclaim
            .mark_original_unclaimed(arm, full.epoch(), 0)?;
        local
            .preclaim
            .request(AbandonBeforeClaimV1 {
                version: 1,
                operation_id: operation,
                arm_id: arm,
                command_id: *identity.retirement_id(),
                expected_epoch: full.epoch(),
                generation: 0,
            })?
            .confirm_audit()?;
        local.preclaim.mark_retiring(arm, full.epoch(), 0)?;
        local.preclaim.retain_retirement(
            arm,
            full.epoch(),
            0,
            RetainedRetirementV1::Pending { _owner: pending },
        );
        let registry =
            configured_registry(&root, &scope, &intent, &_store_root.path().canonicalize()?)?;
        let stop_client = native.stop_client().await?;
        let service = retained_service(
            &root,
            registry,
            Arc::clone(&jobs),
            None,
            Some(stop_client.clone()),
        )?;
        let before = stops.recovery_status(operation).await?.unwrap();
        ensure!(
            before.stops().len() == 1,
            "retained Pending must still own one STOP target"
        );
        let worker_before = canonical(&store.worker(&worker)?.unwrap())?;
        let quota_before = canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?;
        let usage_before = canonical(&store.worker_usage()?)?;
        diagnostic_phase("cancel.shutdown.before");
        close_service(&service).await?;
        diagnostic_phase("cancel.shutdown.after");
        let command = Uuid::new_v4();
        ensure!(
            stops
                .cancel_stop_projection(operation, command)
                .await?
                .is_none(),
            "the Cancel under test must be fresh"
        );
        let authenticated_before = observations.runner_identity_requests.load(Ordering::SeqCst);
        diagnostic_phase("cancel.dispatch.before");
        let dispatch_future =
            service.dispatch(cancel_envelope(scope.profile_id(), operation, command)?);
        diagnostic_future_storage("cancel.dispatch.future", &dispatch_future);
        let reply = dispatch_future.await;
        diagnostic_phase("cancel.dispatch.after");
        let authenticated = observations.runner_identity_requests.load(Ordering::SeqCst)
            == authenticated_before + 1;
        let projection = stops.cancel_stop_projection(operation, command).await?;
        let delivered = stop_client
            .query_stop(&arm.to_string(), &registration_hash)
            .await;
        let after = stops.recovery_status(operation).await?.unwrap();
        let controls_unchanged = canonical(&store.worker(&worker)?.unwrap())? == worker_before
            && canonical(&store.worker_quota(&QuotaPolicyV1::default())?)? == quota_before
            && canonical(&store.worker_usage()?)? == usage_before;
        let durable = projection.as_ref().is_some_and(|projection| {
            projection.receipt().command_id() == command && projection.stops().len() == 1
        });
        let exact_stop = delivered.as_ref().is_ok_and(|status| {
            status.cancel_command_id() == Some(command)
                && status.phase() == crate::broker::enforcer::StopPhaseV1::StopRequested
        });
        ensure!(
            authenticated
                && controls_unchanged
                && service.has_retained_retirement()
                && local.preclaim.has_retained_retirement()
                && service.require_starts().await.is_err()
                && after.retirements().len() == 1
                && after.retirements()[0].finalized().is_none()
                && canonical(after.retirements()[0].request().identity())? == canonical(&identity)?,
            "fresh Cancel must preserve authentication, D5 ownership, identity and allocation facts"
        );
        ensure!(
            matches!(reply, Ok(Response { reply: BrokerReplyV1::CommandAcknowledged { operation_id, command_id, .. }, .. })
                if operation_id == operation && command_id == command)
                && durable
                && exact_stop,
            "retained shutdown stranded fresh authenticated Cancel: durable={durable} exact_stop={exact_stop}"
        );
        Ok(())
    });
    diagnostic_future_storage("cancel.case.future", &outcome_future);
    diagnostic_phase("cancel.case.before_await");
    let outcome: Result<()> = outcome_future.await;
    diagnostic_phase("cancel.case.after_await");
    diagnostic_phase("cancel.cleanup.native.before");
    native.finish().await;
    diagnostic_phase("cancel.cleanup.native.after");
    diagnostic_phase("cancel.cleanup.gateway.before");
    let drained = gateway.finish().await;
    diagnostic_phase("cancel.cleanup.gateway.after");
    outcome.unwrap();
    drained.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retained_startup_retries_same_read_only_q_after_lost_verified_reply() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    diagnostic_phase("startup.fixture.before");
    let LiveFixture {
        _root,
        _store_root,
        store,
        journal,
        intent,
        head,
        registration,
    } = live_fixture_with_gateway(&origin, true);
    diagnostic_phase("startup.fixture.after");
    let worker = registration.arm().worker_id.clone();
    let scope = journal.scope().clone();
    let store = Arc::new(store);
    diagnostic_phase("startup.gateway.before");
    let gateway_future = OwnedGateway::start(
        listener,
        Arc::clone(&store),
        scope.tenant_id(),
        Arc::new(GatewayObservations::default()),
    );
    diagnostic_future_storage("startup.gateway.future", &gateway_future);
    let gateway = gateway_future.await.unwrap();
    diagnostic_phase("startup.gateway.after");
    diagnostic_phase("startup.register.before");
    let register_future = OriginalUnusedFixture::register(scope.installation_id(), registration);
    diagnostic_future_storage("startup.register.future", &register_future);
    let (native, full, slot) = register_future.await.unwrap();
    diagnostic_phase("startup.register.after");
    let outcome_future = boxed_recovery_phase(|| async {
        diagnostic_phase("startup.case.enter");
        let operation = intent.operation_id();
        let writer = WriterClient::start(journal.clone(), 16, Duration::from_secs(10))?;
        writer
            .bind_full3_registration(operation, head, slot.full3_registration_binding()?)
            .await?;
        let pending = audited_pending(
            &writer,
            &AuditClient::new(&origin, RUNNER)?,
            &intent,
            &full,
            &slot,
        )
        .await?;
        let verified = full
            .retire_unused(writer.authorize_unused_retirement(pending).await?)
            .await?;
        let identity = verified.identity().clone();
        let q_hash = verified.receipt().digest()?;
        let q_head = canonical(verified.head())?;
        let q_acceptance = canonical(verified.accepted())?;
        // Only the actual native producer can issue Q. Lose this wrapper and
        // then one fully verified Query reply; neither loss authorizes Retire.
        drop(verified);
        drop(writer);
        drop(journal);
        let root = _root.path().canonicalize()?;
        let report = actual_report(&root, &scope).await?;
        let pending_job = report
            .jobs()
            .iter()
            .find(|job| job.operation_id() == operation)
            .unwrap();
        let pending_revision = pending_job.head().revision();
        ensure!(
            pending_job.retirements().len() == 1
                && pending_job.retirements()[0].finalized().is_none(),
            "actual report must retain P while guardian already owns Q"
        );
        let jobs = Arc::new(ProfileJobs::new());
        jobs.job(operation)?;
        let owner = jobs.retirement_startup.install(report)?;
        let registry =
            configured_registry(&root, &scope, &intent, &_store_root.path().canonicalize()?)?;
        let profile = registry.profile(scope.profile_id())?.clone();
        let service = retained_service(
            &root,
            registry,
            Arc::clone(&jobs),
            Some(ExecutionEnforcerV1::Full3(full.clone())),
            None,
        )?;
        let worker_before = canonical(&store.worker(&worker)?.unwrap())?;
        let quota_before = canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?;
        let usage_before = canonical(&store.worker_usage()?)?;
        diagnostic_phase("startup.shutdown.before");
        close_service(&service).await?;
        diagnostic_phase("startup.shutdown.after");
        full.lose_query_reply_after_successes_for_test(0)?;
        diagnostic_phase("startup.first_resume.before");
        let first = service
            .resume_startup_profile(&profile, &jobs, Arc::clone(&owner))
            .await;
        diagnostic_phase("startup.first_resume.after");
        ensure!(
            matches!(first, Err(BrokerErrorCodeV1::NeedsReconciliation))
                && jobs.writer.get().is_none()
                && Arc::ptr_eq(&jobs.retirement_startup.retained()?.unwrap(), &owner)
                && full.has_local_slot(&identity.arm().operation_id)?,
            "lost verified Q must keep the same report/owner and closed factory"
        );
        diagnostic_phase("startup.second_resume.before");
        let second = service
            .resume_startup_profile(&profile, &jobs, Arc::clone(&owner))
            .await;
        diagnostic_phase("startup.second_resume.after");
        let replay = full
            .query_retirement(&identity)
            .await?
            .ok_or_else(|| anyhow!("same authenticated Q disappeared"))?;
        ensure!(
            replay.receipt().digest()? == q_hash
                && canonical(replay.identity())? == canonical(&identity)?
                && canonical(replay.head())? == q_head
                && canonical(replay.accepted())? == q_acceptance
                && canonical(&store.worker(&worker)?.unwrap())? == worker_before
                && canonical(&store.worker_quota(&QuotaPolicyV1::default())?)? == quota_before
                && canonical(&store.worker_usage()?)? == usage_before
                && service.require_starts().await.is_err(),
            "Q recovery changed guardian ledger, identity, authority or allocation facts"
        );
        ensure!(
            second.is_ok(),
            "same retained startup owner cannot repeat read-only Q after its lost verified reply: code={:?}",
            second.err()
        );
        let capabilities = jobs
            .writer
            .get()
            .ok_or_else(|| anyhow!("same-owner handoff missing"))?;
        let completed = owner.completed_report()?;
        let snapshot = capabilities
            .stops()
            .recovery_status(operation)
            .await?
            .unwrap();
        let finalized = snapshot.retirements()[0]
            .finalized()
            .ok_or_else(|| anyhow!("same Q Final missing"))?;
        let pending_audit = capabilities
            .stops()
            .pending_retirement_events(operation)
            .await?;
        ensure!(
            capabilities.is_stop_only()
                && !jobs.retirement_startup.has_retained()
                && completed.retirement_factory_consumed()
                && snapshot.head().revision() == pending_revision + 1
                && finalized.receipt().digest()? == q_hash
                && canonical(finalized.pending().identity())? == canonical(&identity)?
                && pending_audit.len() == 1
                && pending_audit[0].event_id() == finalized.pending().final_event_id()
                && pending_audit[0].event().kind == "broker.retirement.final"
                && final_rows(&store)?.is_empty()
                && !full.has_local_slot(&identity.arm().operation_id)?,
            "same-Q confirmation must preserve exact Final/audit and one restricted factory"
        );
        let committed_head = snapshot.head().clone();
        ensure!(
            service
                .resume_startup_profile(&profile, &jobs, Arc::clone(&owner))
                .await
                .is_err()
                && capabilities
                    .stops()
                    .recovery_status(operation)
                    .await?
                    .unwrap()
                    .head()
                    == &committed_head,
            "completed owner admitted another factory or checkpoint mutation"
        );
        Ok(())
    });
    diagnostic_future_storage("startup.case.future", &outcome_future);
    diagnostic_phase("startup.case.before_await");
    let outcome: Result<()> = outcome_future.await;
    diagnostic_phase("startup.case.after_await");
    diagnostic_phase("startup.cleanup.native.before");
    native.finish().await;
    diagnostic_phase("startup.cleanup.native.after");
    diagnostic_phase("startup.cleanup.gateway.before");
    let drained = gateway.finish().await;
    diagnostic_phase("startup.cleanup.gateway.after");
    outcome.unwrap();
    drained.unwrap();
}
