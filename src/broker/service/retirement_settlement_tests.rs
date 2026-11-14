//! Real warm Submitted/Final continuation through the existing service pump.
//! SyntheticReady replaces runtime observation only. The original registration,
//! native join, authenticated Q, writer Final and canonical keeper are genuine.
//! No engine, namespace replacement/restoration or NativeResume is selected.
use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    broker::{
        enforcer::{EnforcedSlot, EnforcerError, retirement::test_support::OriginalUnusedFixture},
        intent::ExecutionIntentV1,
        journal::retirement_live_fixture::{INVOCATION_DATA_PROGRAM, live_fixture_with_gateway},
        service::preclaim::AbandonBeforeClaimV1,
        writer::BeginUnusedRetirement,
    },
    control::QuotaPolicyV1,
    runner::AuditClient,
};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

// Public synthetic credentials; no environment/keychain/provider credentials.
const RUNNER: &str = "runner-settlement-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const VIEWER: &str = "viewer-settlement-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct OwnedGateway {
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}
impl OwnedGateway {
    async fn start(listener: TcpListener, store: Arc<Store>, tenant: &str) -> Result<Self> {
        let state = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: tenant.into(),
                store,
                runner_token: RUNNER.into(),
                viewer_token: VIEWER.into(),
            }],
            crate::policy::Policy::default(),
            None,
            Redactor::new(Vec::new()),
        )?;
        let (stop, receive) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, gateway_router(state))
                .with_graceful_shutdown(async move {
                    let _ = receive.await;
                })
                .await
        });
        Ok(Self {
            stop: Some(stop),
            task: Some(task),
        })
    }
    async fn finish(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(mut task) = self.task.take() {
            match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
                Ok(result) => result??,
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    return Err(anyhow!("owned settlement gateway did not drain"));
                }
            }
        }
        Ok(())
    }
}
impl Drop for OwnedGateway {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}
fn registry(
    root: &Path,
    scope: &JournalScopeV1,
    intent: &ExecutionIntentV1,
    filesystem_roots: &Path,
) -> Result<Arc<Registry>> {
    for name in ["sources", "exports"] {
        fs::create_dir(filesystem_roots.join(name))?;
        fs::set_permissions(
            filesystem_roots.join(name),
            fs::Permissions::from_mode(0o700),
        )?;
    }
    let config = root.join("config");
    private_file(&config.join("runner.token"), RUNNER.as_bytes())?;
    let options = intent.initial_options().run_options();
    let path = config.join("registry.json");
    private_file(
        &path,
        &serde_json::to_vec(&json!({
            "version":1,"installation_id":scope.installation_id(),"root":root,
            "max_enforcer_slots":8,"profiles":[{
                "id":scope.profile_id(),"gateway":scope.gateway_origin(),"tenant_id":scope.tenant_id(),
                "control_namespace_uuid":scope.control_namespace_uuid(),"config_dir":config,
                "runner_token_file":config.join("runner.token"),"runtime":"docker",
                "runtime_program":INVOCATION_DATA_PROGRAM,"runtime_endpoint":options.runtime_endpoint,
                "image":options.image,"policy_profile":options.profile,"network":options.network,
                "allow_rootful_runtime":false,"max_timeout_seconds":120,"allow_capture_output":false,
                "managed_clients":false,"manual_credential_file":options.secret_env_file,
                "source_roots":[filesystem_roots.join("sources")],"export_roots":[filesystem_roots.join("exports")]
            }]
        }))?,
    )?;
    Ok(Arc::new(Registry::load(&path)?))
}
fn final_rows(store: &Store) -> Result<Vec<crate::model::StoredEvent>> {
    store.events(&EventQuery {
        kind: Some("broker.retirement.final".into()),
        limit: Some(16),
        ..EventQuery::default()
    })
}

#[derive(Clone, Copy)]
enum Scenario {
    Settle,
    PendingBeforeFinal,
    FinalNotQueued,
    FinalQueued,
    FinalCompletedConsumed,
    ForeignCore,
    AbsentLocalSlot,
    RetainedVerified,
    CancelledAfterFinal,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_original_submitted_final_settles_after_lost_q_without_new_final() {
    composed_settlement(Scenario::Settle).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_original_submitted_pending_recovers_q_then_final_without_new_retire() {
    composed_settlement(Scenario::PendingBeforeFinal).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_original_not_queued_final_retries_its_retained_input_before_another_q() {
    composed_settlement(Scenario::FinalNotQueued).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_original_inflight_final_keeps_its_owner_then_consumes_completed_without_readmission()
{
    composed_settlement(Scenario::FinalQueued).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_original_stale_pending_reads_consumed_final_with_closed_registry_without_readmission()
{
    composed_settlement(Scenario::FinalCompletedConsumed).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_public_final_from_foreign_core_cannot_query_or_clear_keeper() {
    composed_settlement(Scenario::ForeignCore).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn absent_local_slot_without_opaque_completion_keeps_original_keeper() {
    composed_settlement(Scenario::AbsentLocalSlot).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_retained_verified_input_prevents_another_query_wrapper() {
    composed_settlement(Scenario::RetainedVerified).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirmed_cleanup_after_cancel_preserves_generation_and_closed_admission() {
    composed_settlement(Scenario::CancelledAfterFinal).await;
}

async fn composed_settlement(scenario: Scenario) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let live = live_fixture_with_gateway(&origin, true);
    let root = live._root.path().canonicalize().unwrap();
    let store = Arc::new(live.store);
    let filesystem_roots = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let gateway = OwnedGateway::start(
        listener,
        Arc::clone(&store),
        live.journal.scope().tenant_id(),
    )
    .await
    .unwrap();
    let jobs = Arc::new(ProfileJobs::new());
    let job = jobs.job(live.intent.operation_id()).unwrap();
    let (native, full, slot) = OriginalUnusedFixture::register_with_generation(
        live.journal.scope().installation_id(),
        live.registration,
        job.cancel.clone(),
    )
    .await
    .unwrap();
    let writer = WriterClient::start(live.journal.clone(), 16, Duration::from_secs(10)).unwrap();
    jobs.writer
        .set(ProfileWriterCapabilities::from_full(writer.clone()))
        .map_err(|_| anyhow!("fresh settlement writer factory"))
        .unwrap();
    let retained_registry = registry(
        &root,
        live.journal.scope(),
        &live.intent,
        &filesystem_roots.path().canonicalize().unwrap(),
    )
    .unwrap();
    let profile = retained_registry
        .profile(live.journal.scope().profile_id())
        .unwrap()
        .clone();
    let service = Arc::new(Service {
        full_owners: FullOwners::new(8).unwrap(),
        cold_owners: retained_audit::ColdOwners::new(8).unwrap(),
        material: Arc::new(Material::load(&retained_registry).unwrap()),
        registry: retained_registry,
        root: Directory::open_existing(&root).unwrap(),
        profiles: BTreeMap::from([(profile.id.clone(), Arc::clone(&jobs))]),
        enforcer: Some(ServiceEnforcer::Native(ExecutionEnforcerV1::Full3(
            full.clone(),
        ))),
        stop_enforcer: None,
        blocking: Arc::new(Semaphore::new(2)),
        preparation: Arc::new(Semaphore::new(1)),
        execution: Arc::new(Semaphore::new(1)),
        stopping: Arc::new(AtomicBool::new(true)),
        fenced: Arc::new(AtomicBool::new(true)),
    });
    // These actual owners outlive the fallible observation, including its RED.
    let observed = submitted_final_case(
        &service,
        &profile,
        &jobs,
        &job,
        &writer,
        &full,
        &slot,
        &store,
        &live.intent,
        live.head,
        &origin,
        &root,
        scenario,
    )
    .await;
    native.finish().await;
    let drained = gateway.finish().await;
    if observed.is_err() || drained.is_err() || service.has_retained_retirement() {
        let _journal_root = live._root.keep();
        let _store_root = live._store_root.keep();
        let _filesystem_roots = filesystem_roots.keep();
        eprintln!("settlement: private roots retained after a failed observation");
    }
    observed.unwrap();
    drained.unwrap();
}

#[allow(clippy::too_many_arguments)]
async fn submitted_final_case(
    service: &Arc<Service>,
    profile: &ControlProfileV1,
    jobs: &Arc<ProfileJobs>,
    job: &Arc<Job>,
    writer: &WriterClient,
    full: &EnforcerFullClientV3,
    slot: &EnforcedSlot,
    store: &Store,
    intent: &ExecutionIntentV1,
    head: crate::broker::journal::RevisionRefV1,
    origin: &str,
    root: &Path,
    scenario: Scenario,
) -> Result<()> {
    let operation = intent.operation_id();
    let producer = job.enter()?;
    let generation = job.cancel.current();
    let binding = slot.full3_registration_binding()?;
    let arm = binding.arm_id();
    writer
        .bind_full3_registration(operation, head, binding)
        .await?;
    job.preclaim
        .announce_registration(operation, arm, full.epoch(), generation)?;
    job.preclaim
        .mark_original_unclaimed(arm, full.epoch(), generation)?;
    let request = AbandonBeforeClaimV1 {
        version: 1,
        operation_id: operation,
        arm_id: arm,
        command_id: Uuid::new_v4(),
        expected_epoch: full.epoch(),
        generation,
    };
    let signal = job.preclaim.request(request.clone())?;
    let mut event = signal.event().clone();
    let data = intent.request_data();
    event.session_id = data.session_id.clone();
    event.project_id = data.project_id.clone();
    event.sandbox_id = data.sandbox_id.clone();
    event.agent_id = data.agent_id.clone();
    event.correlation_id = data.correlation_id.clone();
    event.trace_id = data.trace_id.clone();
    let client = AuditClient::new(origin, RUNNER)?;
    writer.enqueue_event(operation, event.clone()).await?;
    let sequence = client
        .post_control_event(serde_json::to_value(event)?)
        .await?;
    writer
        .acknowledge_event(operation, request.command_id, sequence)
        .await?;
    signal.confirm_audit()?;
    let unused = slot.revoke_unused(request.command_id).await?;
    let current = writer
        .load(operation)
        .await?
        .ok_or_else(|| anyhow!("actual job missing"))?;
    let pending = writer
        .begin_unused_retirement(BeginUnusedRetirement::new(
            operation,
            current.head().clone(),
            unused,
        )?)
        .await?;
    let identity = pending.identity().clone();
    let final_event_id = pending.final_event_id();
    let sequence = client
        .post_control_event(serde_json::to_value(pending.intention_event())?)
        .await?;
    writer
        .acknowledge_event(operation, pending.intention_event_id(), sequence)
        .await?;
    let authorized = writer.authorize_unused_retirement(pending).await?;
    job.preclaim.mark_retiring(arm, full.epoch(), generation)?;
    let owner = job
        .preclaim
        .retain_authorized_submission(&request, authorized)
        .map_err(|(code, _original)| anyhow::Error::new(code))?;
    let verified = full
        .retire_unused_retained(&owner, || {
            job.cancel.with_current_retirement(generation, || {
                job.preclaim.mark_retirement_submitted(&request, &owner)
            })
        })
        .await?;
    let q_digest = verified.receipt().digest()?;
    let q_head = canonical(verified.head())?;
    if matches!(
        scenario,
        Scenario::FinalNotQueued | Scenario::FinalQueued | Scenario::FinalCompletedConsumed
    ) {
        drop(producer);
        return submitted_final_input_case(
            service,
            profile,
            jobs,
            job,
            writer,
            full,
            slot,
            store,
            owner,
            verified,
            final_event_id,
            scenario,
        )
        .await;
    }
    if matches!(scenario, Scenario::PendingBeforeFinal) {
        drop(producer);
        return submitted_pending_case(
            service,
            profile,
            jobs,
            job,
            writer,
            full,
            slot,
            store,
            owner,
            verified,
            final_event_id,
        )
        .await;
    }
    let final_receipt = writer.stops().finalize_retirement(verified).await?;
    let before = writer
        .stops()
        .recovery_status(operation)
        .await?
        .ok_or_else(|| anyhow!("actual Final snapshot missing"))?;
    ensure!(
        owner.is_submitted()? && owner.matches_recovery_origin(&before),
        "Submitted owner lost its real original Journal before settlement"
    );
    let before_entry = &before.retirements()[0];
    ensure!(
        before_entry.finalized().is_some() && full.has_local_slot(&identity.arm().operation_id)?,
        "fixture must retain a present local slot after actual writer Final"
    );
    let worker = canonical(
        &store
            .worker(&identity.arm().worker_id)?
            .ok_or_else(|| anyhow!("actual worker missing"))?,
    )?;
    let quota = canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?;
    let usage = canonical(&store.worker_usage()?)?;
    full.lose_query_reply_after_successes_for_test(0)?;
    ensure!(
        matches!(
            full.query_retirement_retained(&owner).await,
            Err(EnforcerError::Unavailable)
        ),
        "the fixture did not lose a genuinely authenticated post-F Q reply"
    );
    ensure!(
        Arc::ptr_eq(
            &job.preclaim
                .retirement_submission_for_test(arm)
                .ok_or_else(|| anyhow!("canonical owner missing after lost Q"))?,
            &owner
        ),
        "post-F Q failure replaced or dropped the actual canonical owner"
    );
    // The real Job guard excludes Q work, but promised audit is independent.
    Arc::clone(service)
        .reconcile_retirement_audit_profile(profile.clone())
        .await?;
    ensure!(
        job.preclaim.has_retained_retirement()
            && full.has_local_slot(&identity.arm().operation_id)?,
        "service bypassed the actual live Job guard"
    );
    ensure!(
        final_rows(store)?.len() == 1,
        "Job guard starved the promised final audit"
    );
    drop(producer);
    let expected_generation = if matches!(scenario, Scenario::CancelledAfterFinal) {
        job.cancel.invalidate()
    } else {
        generation
    };
    let should_settle = matches!(scenario, Scenario::Settle | Scenario::CancelledAfterFinal);
    match scenario {
        Scenario::FinalNotQueued | Scenario::FinalQueued | Scenario::FinalCompletedConsumed => {
            return Err(anyhow!(
                "Final input case reached the already-Final fixture"
            ));
        }
        Scenario::PendingBeforeFinal => {
            return Err(anyhow!("Pending case reached the already-Final fixture"));
        }
        Scenario::Settle | Scenario::CancelledAfterFinal => {
            // A failed current Q round keeps the actual holder; only the next
            // successful round may complete. This is a real verified reply loss.
            full.lose_query_reply_after_successes_for_test(0)?;
            Arc::clone(service)
                .reconcile_retirement_audit_profile(profile.clone())
                .await?;
            ensure!(
                job.preclaim.has_retained_retirement()
                    && full.has_local_slot(&identity.arm().operation_id)?,
                "unknown settlement Q released its original keeper or slot"
            );
            Arc::clone(service)
                .reconcile_retirement_audit_profile(profile.clone())
                .await?;
            ensure!(
                !job.preclaim.has_retained_retirement()
                    && !full.has_local_slot(&identity.arm().operation_id)?,
                "confirmed original Final did not settle its exact keeper and local slot"
            );
        }
        Scenario::ForeignCore => {
            let current = writer
                .stops()
                .recovery_status(operation)
                .await?
                .ok_or_else(|| anyhow!("current original snapshot missing"))?;
            let (_copied, foreign) = independent_journal(root, writer.scope())?;
            let foreign_writer = WriterClient::start(foreign, 16, Duration::from_secs(10))?;
            let copied = foreign_writer
                .stops()
                .recovery_status(operation)
                .await?
                .ok_or_else(|| anyhow!("foreign snapshot missing"))?;
            ensure!(
                canonical(current.scope())? == canonical(copied.scope())?
                    && current.head() == copied.head()
                    && canonical(&current.retirements())? == canonical(&copied.retirements())?
                    && !owner.matches_recovery_origin(&copied),
                "foreign-core fixture changed public history or reused physical origin"
            );
            full.lose_query_reply_after_successes_for_test(0)?;
            let guard = jobs
                .try_retirement_query(operation)?
                .ok_or_else(|| anyhow!("actual settlement guard unavailable"))?;
            ensure!(
                retirement_settlement::settle_confirmed_final(
                    &guard,
                    &writer.stops(),
                    full,
                    &copied,
                    request.command_id,
                )
                .await
                .is_err(),
                "foreign core cleared a real original keeper"
            );
            drop(guard);
            ensure!(
                matches!(
                    full.query_retirement_retained(&owner).await,
                    Err(EnforcerError::Unavailable)
                ),
                "foreign source reached authenticated Q before refusal"
            );
        }
        Scenario::AbsentLocalSlot => {
            let original = full
                .query_retirement_retained(&owner)
                .await?
                .ok_or_else(|| anyhow!("real Q missing for absent-slot precondition"))?;
            ensure!(
                !original.completes_retained_submission(&owner),
                "fresh Q already claims canonical local completion"
            );
            // This real legacy local primitive removes the slot but seals no
            // canonical completion. Its absence cannot manufacture that proof.
            full.retire_local_slot(&original)?;
            drop(original);
            let current = writer
                .stops()
                .recovery_status(operation)
                .await?
                .ok_or_else(|| anyhow!("original Final missing after local removal"))?;
            full.lose_query_reply_after_successes_for_test(0)?;
            let guard = jobs
                .try_retirement_query(operation)?
                .ok_or_else(|| anyhow!("actual settlement guard unavailable"))?;
            ensure!(
                retirement_settlement::settle_confirmed_final(
                    &guard,
                    &writer.stops(),
                    full,
                    &current,
                    request.command_id,
                )
                .await
                .is_err(),
                "slot absence fabricated canonical completion"
            );
            drop(guard);
            ensure!(
                matches!(
                    full.query_retirement_retained(&owner).await,
                    Err(EnforcerError::Unavailable)
                ),
                "absent slot reached Q instead of remaining unknown"
            );
        }
        Scenario::RetainedVerified => {
            let original = full
                .query_retirement_retained(&owner)
                .await?
                .ok_or_else(|| anyhow!("real Q missing for retained-input precondition"))?;
            ensure!(
                !original.completes_retained_submission(&owner),
                "sealed Q must never be retained by its own owner"
            );
            owner
                .retain_verified(original)
                .map_err(|_same_input| anyhow!("fresh actual Q was not retained"))?;
            full.lose_query_reply_after_successes_for_test(0)?;
            Arc::clone(service)
                .reconcile_retirement_audit_profile(profile.clone())
                .await?;
            ensure!(
                matches!(
                    full.query_retirement_retained(&owner).await,
                    Err(EnforcerError::Unavailable)
                ),
                "retained input was bypassed with another wrapper"
            );
        }
    }
    if !should_settle {
        ensure!(
            Arc::ptr_eq(
                &job.preclaim
                    .retirement_submission_for_test(arm)
                    .ok_or_else(|| anyhow!("negative case lost canonical holder"))?,
                &owner
            ),
            "negative case replaced or released the actual keeper"
        );
    }
    let after = writer
        .load(operation)
        .await?
        .ok_or_else(|| anyhow!("settled job missing"))?;
    let finalized = after
        .broker_checkpoint_v5()
        .and_then(|c| c.retirements().first())
        .and_then(|e| e.finalized())
        .ok_or_else(|| anyhow!("settled Final disappeared"))?;
    let birth = after
        .outbox_receipt(final_event_id, None)
        .ok_or_else(|| anyhow!("Final birth receipt missing"))?;
    ensure!(
        finalized.receipt().digest()? == q_digest
            && birth.head() == final_receipt.head()
            && final_rows(store)?.len() == 1,
        "settlement repeated or changed the actual Final"
    );
    if should_settle {
        let replay = full
            .query_retirement_retained(&owner)
            .await?
            .ok_or_else(|| anyhow!("actual Q missing after settlement"))?;
        ensure!(
            canonical(replay.head())? == q_head && replay.receipt().digest()? == q_digest,
            "settlement changed the native retirement ledger or receipt"
        );
    }
    ensure!(
        matches!(
            slot.reserve_claim(expected_generation),
            Err(EnforcerError::NotReady)
        ),
        "old revoked slot acquired a claim after settlement"
    );
    ensure!(
        service.stopping.load(Ordering::Acquire)
            && service.fenced.load(Ordering::Acquire)
            && service.require_starts().await.is_err()
            && job.cancel.current() == expected_generation,
        "memory cleanup reopened admission or reset cancellation generation"
    );
    ensure!(
        worker
            == canonical(
                &store
                    .worker(&identity.arm().worker_id)?
                    .ok_or_else(|| anyhow!("worker disappeared"))?
            )?
            && quota == canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?
            && usage == canonical(&store.worker_usage()?)?,
        "settlement fabricated a worker terminal, quota release or usage"
    );
    ensure!(
        jobs.has_retained_retirement() == !should_settle,
        "settlement reported the wrong remaining process ownership"
    );
    service.full_owners.drain().await?;
    service.cold_owners.drain().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn submitted_pending_case(
    service: &Arc<Service>,
    profile: &ControlProfileV1,
    jobs: &Arc<ProfileJobs>,
    job: &Arc<Job>,
    writer: &WriterClient,
    full: &EnforcerFullClientV3,
    slot: &EnforcedSlot,
    store: &Store,
    owner: Arc<crate::broker::service::preclaim::RetirementSubmission>,
    verified: crate::broker::enforcer::retirement::VerifiedRetirementReceipt,
    final_event_id: Uuid,
) -> Result<()> {
    let identity = owner.identity().clone();
    let operation = *identity.job_operation_id();
    let arm = Uuid::parse_str(&identity.arm().operation_id)?;
    let generation = *identity.generation();
    let setup_guard = jobs
        .try_retirement_query(operation)?
        .ok_or_else(|| anyhow!("actual Pending setup guard unavailable"))?;
    let before = writer
        .stops()
        .recovery_status(operation)
        .await?
        .ok_or_else(|| anyhow!("actual Pending snapshot missing"))?;
    ensure!(
        !before.incomplete()
            && before.retirements().len() == 1
            && owner.matches_recovery_origin(&before)
            && owner.is_submitted()?,
        "Pending fixture lacks the actual complete Submitted origin"
    );
    let pending = before.retirements()[0].request();
    let pending_bytes = canonical(pending)?;
    ensure!(
        before.retirements()[0].finalized().is_none()
            && pending.audit_ack().is_some()
            && pending.identity().digest()? == identity.digest()?
            && pending.evidence().digest()? == owner.evidence().digest()?
            && canonical(&pending.audit_ack())? == canonical(&Some(owner.audit_ack()))?
            && pending.final_event_id() == final_event_id
            && full.has_local_slot(&identity.arm().operation_id)?
            && final_rows(store)?.is_empty()
            && matches!(
                writer.stops().recover_final_retirement_state(
                    operation,
                    arm,
                    *identity.retirement_id(),
                ),
                Ok(crate::broker::writer::FinalRetirementRecovery::Absent)
            ),
        "Pending fixture already has Final, lacks its intention ACK, or owns another Final input"
    );
    let initial = writer
        .load(operation)
        .await?
        .ok_or_else(|| anyhow!("actual Pending job missing"))?;
    let original_intent = initial.intent().canonical_bytes()?;
    let operations = initial
        .broker_checkpoint_v2()
        .ok_or_else(|| anyhow!("actual Pending supervision missing"))?
        .supervision()
        .operations();
    let original_operations = canonical(&operations)?;
    ensure!(
        !operations.iter().any(|entry| matches!(
            entry,
            crate::broker::checkpoint::SupervisionOperationV1::WorkerClaim { .. }
                | crate::broker::checkpoint::SupervisionOperationV1::HelperClaim { .. }
        )) && initial.outbox_receipt(final_event_id, None).is_none(),
        "Pending fixture already consumed a claim or committed a Final event"
    );
    let q_digest = verified.receipt().digest()?;
    let q_head = canonical(verified.head())?;
    let q_accepted = canonical(verified.accepted())?;
    let q_identity = canonical(verified.identity())?;
    ensure!(
        verified.identity().digest()? == identity.digest()?
            && !verified.completes_retained_submission(&owner),
        "actual RetireUnused reply lacks the original Q or already claims local removal"
    );
    let worker = canonical(
        &store
            .worker(&identity.arm().worker_id)?
            .ok_or_else(|| anyhow!("actual Pending worker missing"))?,
    )?;
    let quota = canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?;
    let usage = canonical(&store.worker_usage()?)?;
    // Lose a genuine authenticated result wrapper before Final admission. This
    // is not a lost wire ACK, and no test-created receipt replaces the result.
    drop(verified);
    ensure!(
        Arc::ptr_eq(
            &job.preclaim
                .retirement_submission_for_test(arm)
                .ok_or_else(|| anyhow!("Pending setup lost its canonical owner"))?,
            &owner
        ) && owner.is_submitted()?
            && owner.reserve_attempt() == Err(EnforcerError::Conflict)
            && job
                .preclaim
                .submitted_retirement_for_settlement(&identity)?
                .is_some_and(|continuation| Arc::ptr_eq(continuation.owner(), &owner))
            && job.cancel.current() == generation,
        "dropping Q changed the original Submitted owner or reopened submission"
    );
    // Only this marker establishes setup: P/intention ACK, native join/Q,
    // canonical Submitted without F, and actual wrapper loss all precede it.
    eprintln!("BROKER_SUBMITTED_PENDING_BASELINE_SETUP_CONFIRMED");
    drop(setup_guard);
    Arc::clone(service)
        .reconcile_retirement_audit_profile(profile.clone())
        .await?;
    let settled = writer
        .stops()
        .recovery_status(operation)
        .await?
        .ok_or_else(|| anyhow!("Pending recovery snapshot missing after pump"))?;
    ensure!(
        settled.retirements().len() == 1
            && settled.retirements()[0].finalized().is_some()
            && !job.preclaim.has_retained_retirement()
            && !jobs.has_retained_retirement()
            && !full.has_local_slot(&identity.arm().operation_id)?,
        "submitted Pending with genuine Q did not converge to its exact Final and local release"
    );
    ensure!(
        owner.matches_recovery_origin(&settled)
            && canonical(settled.retirements()[0].request())? == pending_bytes
            && settled.retirements()[0]
                .finalized()
                .ok_or_else(|| anyhow!("settled Pending Final disappeared"))?
                .receipt()
                .digest()?
                == q_digest,
        "Pending reconciliation changed its original origin, request, ACK, deadline, or Q receipt"
    );
    let after = writer
        .load(operation)
        .await?
        .ok_or_else(|| anyhow!("reconciled Pending job missing"))?;
    let final_birth = after
        .outbox_receipt(final_event_id, None)
        .ok_or_else(|| anyhow!("reconciled Pending Final birth receipt missing"))?
        .head()
        .clone();
    let final_events = final_rows(store)?;
    ensure!(
        final_events.len() == 1 && final_events[0].event.id == final_event_id.to_string(),
        "Pending reconciliation did not deliver its single fixed Final event"
    );
    let replay = full
        .query_retirement_retained(&owner)
        .await?
        .ok_or_else(|| anyhow!("actual Q missing after Pending reconciliation"))?;
    ensure!(
        replay.receipt().digest()? == q_digest
            && canonical(replay.head())? == q_head
            && canonical(replay.accepted())? == q_accepted
            && canonical(replay.identity())? == q_identity,
        "Pending reconciliation created another native retirement effect"
    );
    // Stable native Q proves no new durable retirement effect, not a count of
    // idempotent RPCs. The unchanged consumed-attempt guard also stays closed.
    ensure!(
        owner.reserve_attempt() == Err(EnforcerError::Conflict)
            && matches!(slot.reserve_claim(generation), Err(EnforcerError::NotReady))
            && service.stopping.load(Ordering::Acquire)
            && service.fenced.load(Ordering::Acquire)
            && service.require_starts().await.is_err()
            && job.cancel.current() == generation
            && after.intent().canonical_bytes()? == original_intent
            && canonical(
                &after
                    .broker_checkpoint_v2()
                    .ok_or_else(|| anyhow!("reconciled Pending supervision missing"))?
                    .supervision()
                    .operations(),
            )? == original_operations,
        "Pending reconciliation reopened submission or changed an Arm, claim, intent, or generation"
    );
    ensure!(
        worker
            == canonical(
                &store
                    .worker(&identity.arm().worker_id)?
                    .ok_or_else(|| anyhow!("Pending worker disappeared"))?
            )?
            && quota == canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?
            && usage == canonical(&store.worker_usage()?)?,
        "Pending reconciliation fabricated a worker terminal, quota release, or usage"
    );
    Arc::clone(service)
        .reconcile_retirement_audit_profile(profile.clone())
        .await?;
    let repeated = writer
        .load(operation)
        .await?
        .ok_or_else(|| anyhow!("replayed Pending job missing"))?;
    ensure!(
        repeated.head() == after.head()
            && repeated
                .outbox_receipt(final_event_id, None)
                .is_some_and(|birth| birth.head() == &final_birth)
            && canonical(&final_rows(store)?)? == canonical(&final_events)?
            && !job.preclaim.has_retained_retirement()
            && !full.has_local_slot(&identity.arm().operation_id)?,
        "Pending replay repeated Final or changed its completed local retirement"
    );
    service.full_owners.drain().await?;
    service.cold_owners.drain().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn submitted_final_input_case(
    service: &Arc<Service>,
    profile: &ControlProfileV1,
    jobs: &Arc<ProfileJobs>,
    job: &Arc<Job>,
    writer: &WriterClient,
    full: &EnforcerFullClientV3,
    slot: &EnforcedSlot,
    store: &Store,
    owner: Arc<crate::broker::service::preclaim::RetirementSubmission>,
    verified: crate::broker::enforcer::retirement::VerifiedRetirementReceipt,
    final_event_id: Uuid,
    scenario: Scenario,
) -> Result<()> {
    let identity = owner.identity().clone();
    let operation = *identity.job_operation_id();
    let arm = Uuid::parse_str(&identity.arm().operation_id)?;
    let generation = *identity.generation();
    let stops = writer.stops();
    let guard = jobs
        .try_retirement_query(operation)?
        .ok_or_else(|| anyhow!("actual Final input setup guard unavailable"))?;
    let before = stops
        .recovery_status(operation)
        .await?
        .ok_or_else(|| anyhow!("actual Final input Pending missing"))?;
    ensure!(
        !before.incomplete()
            && before.retirements().len() == 1
            && before.retirements()[0].finalized().is_none()
            && before.retirements()[0].request().audit_ack().is_some()
            && before.retirements()[0].request().final_event_id() == final_event_id
            && owner.matches_recovery_origin(&before)
            && owner.is_submitted()?
            && final_rows(store)?.is_empty()
            && !stops.has_retained_retirement(),
        "Final input fixture lacks exact Submitted P/ACK or already owns Final"
    );
    let pending_bytes = canonical(before.retirements()[0].request())?;
    let initial = writer
        .load(operation)
        .await?
        .ok_or_else(|| anyhow!("actual Final input job missing"))?;
    let intent_bytes = initial.intent().canonical_bytes()?;
    let original_operations = canonical(
        &initial
            .broker_checkpoint_v2()
            .ok_or_else(|| anyhow!("actual Final input supervision missing"))?
            .supervision()
            .operations(),
    )?;
    let q_digest = verified.receipt().digest()?;
    let q_head = canonical(verified.head())?;
    let q_accepted = canonical(verified.accepted())?;
    let q_identity = canonical(verified.identity())?;
    ensure!(
        verified.identity().digest()? == identity.digest()?
            && !verified.completes_retained_submission(&owner)
            && full.has_local_slot(&identity.arm().operation_id)?,
        "Final input fixture did not retain the genuine unsealed Q and local slot"
    );
    let worker = canonical(
        &store
            .worker(&identity.arm().worker_id)?
            .ok_or_else(|| anyhow!("actual Final input worker missing"))?,
    )?;
    let quota = canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?;
    let usage = canonical(&store.worker_usage()?)?;
    let continuation = job
        .preclaim
        .submitted_retirement_for_settlement(&identity)?
        .ok_or_else(|| anyhow!("actual Final input canonical carrier missing"))?;
    ensure!(
        Arc::ptr_eq(continuation.owner(), &owner),
        "Final input setup selected another keeper"
    );
    let mut handoff = job
        .preclaim
        .reserve_final_input(&guard, &continuation, verified)?;
    let input = handoff.take_input()?;

    match scenario {
        Scenario::FinalNotQueued => {
            let pause = writer.pause_for_test().await?;
            ensure!(
                pause.is_paused(),
                "actual Final pressure writer did not pause"
            );
            // The fixture's real writer queue has capacity 16. Poll ordinary
            // read commands into it while its existing actor pause is held;
            // the seventeenth refusal proves pressure without private sender access.
            let mut queued = (0..16)
                .map(|_| Box::pin(writer.status(operation)))
                .collect::<Vec<_>>();
            for waiter in &mut queued {
                ensure!(
                    futures_util::poll!(waiter.as_mut()).is_pending(),
                    "ordinary Final pressure read did not enter the paused real queue"
                );
            }
            ensure!(
                matches!(
                    writer.status(operation).await,
                    Err(crate::broker::journal::JournalError::Busy)
                ) && pause.is_paused(),
                "ordinary queue saturation was not established before Final"
            );
            let returned = match stops.finalize_retirement(input).await {
                Err(crate::broker::writer::RetirementFailure::NotQueued {
                    error: crate::broker::journal::JournalError::Busy,
                    input,
                }) => input,
                Err(failure) => {
                    return Err(anyhow!(
                        "actual Final pressure did not return its input: {:?}",
                        failure.error()
                    ));
                }
                Ok(_) => return Err(anyhow!("Final entered a queue proven full")),
            };
            ensure!(
                pause.is_paused()
                    && returned.receipt().digest()? == q_digest
                    && canonical(returned.head())? == q_head
                    && canonical(returned.accepted())? == q_accepted
                    && canonical(returned.identity())? == q_identity
                    && !stops.has_retained_retirement(),
                "NotQueued Final changed its actual input or left another registry owner"
            );
            handoff.retain_returned(returned)?;
            drop(handoff);
            let retryable = job
                .preclaim
                .retained_final_for_settlement(&identity)?
                .ok_or_else(|| anyhow!("NotQueued Final lost its retryable carrier"))?;
            ensure!(
                Arc::ptr_eq(retryable.owner(), &owner)
                    && job
                        .preclaim
                        .submitted_retirement_for_settlement(&identity)?
                        .is_none(),
                "retryable Final escaped through the ordinary Submitted selector"
            );
            // Abandon this exact borrowed token before taking its input. Drop
            // must return that same wrapper to retryable retention, not Q again.
            let borrowed = job
                .preclaim
                .take_retained_final_input(&guard, &retryable)?
                .ok_or_else(|| anyhow!("NotQueued Final has no actual retained input"))?;
            ensure!(
                job.preclaim
                    .retained_final_for_settlement(&identity)?
                    .is_none(),
                "a borrowed Final input remained selectable by a second handoff"
            );
            drop(borrowed);
            ensure!(
                job.preclaim
                    .retained_final_for_settlement(&identity)?
                    .is_some_and(|retained| Arc::ptr_eq(retained.owner(), &owner)),
                "dropping the borrowed Final input did not restore its actual keeper"
            );
            drop(pause);
            for waiter in queued {
                ensure!(
                    waiter.await?.is_some(),
                    "ordinary pressure read did not drain"
                );
            }
            eprintln!("BROKER_SUBMITTED_NOT_QUEUED_INPUT_SETUP_CONFIRMED");
        }
        Scenario::FinalQueued => {
            let pause = stops.pause_for_test().await?;
            ensure!(pause.is_paused(), "actual Final owner writer did not pause");
            let receive = match stops.enqueue_final_for_test(input) {
                Ok(receive) => receive,
                Err(crate::broker::writer::RetirementFailure::NotQueued { input, .. }) => {
                    handoff.retain_returned(input)?;
                    return Err(anyhow!(
                        "Final owner fixture did not enqueue its actual input"
                    ));
                }
                Err(failure) => {
                    return Err(anyhow!(
                        "Final owner enqueue became unknown: {:?}",
                        failure.error()
                    ));
                }
            };
            // The real writer now owns the only input; losing its reply and
            // dropping the empty handoff cannot manufacture a replacement Q.
            drop(receive);
            drop(handoff);
            ensure!(
                matches!(
                    stops.recover_final_retirement_state(operation, arm, *identity.retirement_id()),
                    Ok(crate::broker::writer::FinalRetirementRecovery::InFlight)
                ) && job
                    .preclaim
                    .retained_final_for_settlement(&identity)?
                    .is_none()
                    && job
                        .preclaim
                        .submitted_retirement_for_settlement(&identity)?
                        .is_some_and(|retained| Arc::ptr_eq(retained.owner(), &owner)),
                "queued Final did not retain the real writer input independently of its waiter"
            );
            // A failed InFlight round must not touch Q. Leave this genuine Q
            // reply-loss fault armed until the post-F local-cleanup round.
            full.lose_query_reply_after_successes_for_test(0)?;
            let inflight = tokio::time::timeout(
                Duration::from_secs(30),
                retirement_settlement::settle_confirmed_final(
                    &guard,
                    &stops,
                    full,
                    &before,
                    *identity.retirement_id(),
                ),
            )
            .await
            .map_err(|_| anyhow!("InFlight observation exceeded its fixture watchdog"))?;
            ensure!(
                pause.is_paused()
                    && matches!(inflight, Err(BrokerErrorCodeV1::NeedsReconciliation))
                    && matches!(
                        stops.recover_final_retirement_state(
                            operation,
                            arm,
                            *identity.retirement_id()
                        ),
                        Ok(crate::broker::writer::FinalRetirementRecovery::InFlight)
                    )
                    && full.has_local_slot(&identity.arm().operation_id)?
                    && job
                        .preclaim
                        .retirement_submission_for_test(arm)
                        .is_some_and(|retained| Arc::ptr_eq(&retained, &owner)),
                "InFlight Final was queried, released, replaced, or allowed its pause to expire"
            );
            drop(pause);
            ensure!(
                writer.status(operation).await?.is_some() && stops.has_retained_retirement(),
                "actual Final did not finish with its unconsumed registry outcome"
            );
            // Do not recover Completed here: recovery consumes it. The real
            // pump below must consume that original outcome itself.
            eprintln!("BROKER_SUBMITTED_COMPLETED_INPUT_SETUP_CONFIRMED");
        }
        Scenario::FinalCompletedConsumed => {
            let receipt = stops.finalize_retirement(input).await?;
            drop(handoff);
            let durable = stops
                .recovery_status(operation)
                .await?
                .ok_or_else(|| anyhow!("actual consumed Final snapshot missing"))?;
            ensure!(
                durable.retirements().len() == 1
                    && durable.retirements()[0].finalized().is_some()
                    && canonical(durable.retirements()[0].request())? == pending_bytes
                    && owner.matches_recovery_origin(&durable)
                    && durable.head() == receipt.head()
                    && matches!(
                        stops.recover_final_retirement_state(
                            operation,
                            arm,
                            *identity.retirement_id()
                        ),
                        Ok(crate::broker::writer::FinalRetirementRecovery::Absent)
                    )
                    && !stops.has_retained_retirement()
                    && job.preclaim.has_retained_retirement()
                    && full.has_local_slot(&identity.arm().operation_id)?
                    && final_rows(store)?.is_empty(),
                "consumed Final fixture lacks durable F, registry absence, or its actual keeper"
            );
            // Final's real awaited result consumed its registry output. Close
            // admission before reconciling the older P: another Final request
            // would now return NotQueued instead of silently replaying F.
            stops.close_retirement_owners()?;
            eprintln!("BROKER_SUBMITTED_CONSUMED_FINAL_SETUP_CONFIRMED");
            retirement_settlement::settle_confirmed_final(
                &guard,
                &stops,
                full,
                &before,
                *identity.retirement_id(),
            )
            .await?;
            let settled = stops
                .recovery_status(operation)
                .await?
                .ok_or_else(|| anyhow!("consumed Final disappeared after stale-P cleanup"))?;
            ensure!(
                settled.head() == durable.head()
                    && canonical(&settled.retirements())? == canonical(&durable.retirements())?
                    && !jobs.has_retained_retirement()
                    && !stops.has_retained_retirement()
                    && !full.has_local_slot(&identity.arm().operation_id)?
                    && final_rows(store)?.is_empty(),
                "stale Pending did not clean its consumed Final without readmission or metadata publication"
            );
        }
        _ => return Err(anyhow!("invalid Final input scenario")),
    }
    drop(guard);
    if matches!(scenario, Scenario::FinalNotQueued) {
        full.lose_query_reply_after_successes_for_test(0)?;
    }
    Arc::clone(service)
        .reconcile_retirement_audit_profile(profile.clone())
        .await?;
    let after_lost_q = stops
        .recovery_status(operation)
        .await?
        .ok_or_else(|| anyhow!("actual Final missing after retained-input pump"))?;
    if !matches!(scenario, Scenario::FinalCompletedConsumed) {
        ensure!(
            after_lost_q.retirements().len() == 1
                && after_lost_q.retirements()[0].finalized().is_some()
                && canonical(after_lost_q.retirements()[0].request())? == pending_bytes
                && owner.matches_recovery_origin(&after_lost_q)
                && !stops.has_retained_retirement()
                && job.preclaim.has_retained_retirement()
                && full.has_local_slot(&identity.arm().operation_id)?
                && job
                    .preclaim
                    .retained_final_for_settlement(&identity)?
                    .is_none()
                && job
                    .preclaim
                    .submitted_retirement_for_settlement(&identity)?
                    .is_some_and(|retained| Arc::ptr_eq(retained.owner(), &owner)),
            "retained Final input was bypassed, not consumed, or released before post-F Q"
        );
    }
    Arc::clone(service)
        .reconcile_retirement_audit_profile(profile.clone())
        .await?;
    let after = writer
        .load(operation)
        .await?
        .ok_or_else(|| anyhow!("actual Final input completion missing"))?;
    let snapshot = stops
        .recovery_status(operation)
        .await?
        .ok_or_else(|| anyhow!("actual Final input completion snapshot missing"))?;
    let finalized = snapshot
        .retirements()
        .first()
        .and_then(|entry| entry.finalized())
        .ok_or_else(|| anyhow!("retained Final input did not produce Final"))?;
    let birth = after
        .outbox_receipt(final_event_id, None)
        .ok_or_else(|| anyhow!("retained Final input lost its fixed Final event birth"))?
        .head()
        .clone();
    let events = final_rows(store)?;
    let replay = full
        .query_retirement_retained(&owner)
        .await?
        .ok_or_else(|| anyhow!("actual Q missing after Final input cleanup"))?;
    ensure!(
        snapshot.retirements().len() == 1
            && canonical(snapshot.retirements()[0].request())? == pending_bytes
            && owner.matches_recovery_origin(&snapshot)
            && finalized.receipt().digest()? == q_digest
            && replay.receipt().digest()? == q_digest
            && canonical(replay.head())? == q_head
            && canonical(replay.accepted())? == q_accepted
            && canonical(replay.identity())? == q_identity
            && events.len() == 1
            && events[0].event.id == final_event_id.to_string()
            && !jobs.has_retained_retirement()
            && !stops.has_retained_retirement()
            && !full.has_local_slot(&identity.arm().operation_id)?,
        "same Final input did not converge once through its original F/Q/local release"
    );
    ensure!(
        owner.reserve_attempt() == Err(EnforcerError::Conflict)
            && matches!(slot.reserve_claim(generation), Err(EnforcerError::NotReady))
            && job.cancel.current() == generation
            && service.stopping.load(Ordering::Acquire)
            && service.fenced.load(Ordering::Acquire)
            && service.require_starts().await.is_err()
            && after.intent().canonical_bytes()? == intent_bytes
            && canonical(
                &after
                    .broker_checkpoint_v2()
                    .ok_or_else(|| anyhow!("Final input completion supervision missing"))?
                    .supervision()
                    .operations()
            )? == original_operations
            && worker
                == canonical(
                    &store
                        .worker(&identity.arm().worker_id)?
                        .ok_or_else(|| anyhow!("Final input worker disappeared"))?
                )?
            && quota == canonical(&store.worker_quota(&QuotaPolicyV1::default())?)?
            && usage == canonical(&store.worker_usage()?)?,
        "Final input recovery changed admission, original deadline, claims, worker, quota, or usage"
    );
    Arc::clone(service)
        .reconcile_retirement_audit_profile(profile.clone())
        .await?;
    let repeated = writer
        .load(operation)
        .await?
        .ok_or_else(|| anyhow!("Final input replay job missing"))?;
    ensure!(
        repeated.head() == after.head()
            && repeated
                .outbox_receipt(final_event_id, None)
                .is_some_and(|receipt| receipt.head() == &birth)
            && canonical(&final_rows(store)?)? == canonical(&events)?
            && !jobs.has_retained_retirement()
            && !stops.has_retained_retirement(),
        "Final input replay admitted another Final or restored a keeper"
    );
    service.full_owners.drain().await?;
    service.cold_owners.drain().await?;
    Ok(())
}

// Copy only this newly owned idle fixture journal. Never copy its native role,
// listener, registry credential or any existing user/control namespace.
fn copy_journal_data(
    source: &Path,
    destination: &Path,
    entries: &mut usize,
    bytes: &mut u64,
) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        *entries = entries
            .checked_sub(1)
            .ok_or_else(|| anyhow!("fixture entry budget"))?;
        let metadata = fs::symlink_metadata(entry.path())?;
        let target = destination.join(entry.file_name());
        if metadata.file_type().is_dir() {
            fs::create_dir(&target)?;
            fs::set_permissions(&target, metadata.permissions())?;
            copy_journal_data(&entry.path(), &target, entries, bytes)?;
        } else {
            ensure!(
                metadata.file_type().is_file(),
                "fixture refuses special files"
            );
            *bytes = bytes
                .checked_sub(metadata.len())
                .ok_or_else(|| anyhow!("fixture byte budget"))?;
            ensure!(
                fs::copy(entry.path(), target)? == metadata.len(),
                "fixture journal changed while copied"
            );
        }
    }
    Ok(())
}

fn independent_journal(
    root: &Path,
    scope: &JournalScopeV1,
) -> Result<(tempfile::TempDir, Journal)> {
    let copied = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()?;
    let mut entries = 512;
    let mut bytes = 8 * 1024 * 1024;
    copy_journal_data(
        &root.join("journals").join(scope.profile_id()),
        copied.path(),
        &mut entries,
        &mut bytes,
    )?;
    let directory = Directory::open_existing(&copied.path().canonicalize()?)?;
    let journal = Journal::open(
        &directory,
        scope.clone(),
        &FsExclusions::new(),
        JournalLimitsV1::new(256 * 1024 * 1024, 256, 256, 512 * 1024 * 1024, 256, 128)?,
        ControlSecrets::new(Vec::new())?,
    )?;
    Ok((copied, journal))
}
