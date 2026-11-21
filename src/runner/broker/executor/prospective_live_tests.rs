//! Real writer/Gateway/SQLite/native IPC boundary tests. Created snapshots are
//! explicit synthetic DATA and the native fixture supplies SyntheticReady;
//! no ManagedRuntime, engine effect or provider is invoked or qualified.
use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    broker::{
        clock::NativeHostClock,
        enforcer::{EnforcerClient, retirement::test_support::ProspectiveFixture},
        journal::retirement_live_fixture::{
            INVOCATION_DATA_PROGRAM, LiveFixture, live_fixture_with_created_helper_for_prospective,
            live_fixture_with_gateway, live_fixture_with_pending_arm_for_prospective,
        },
    },
    control_fs::Directory,
    redaction::Redactor,
    runtime::{Runtime, RuntimeKind},
};
use std::{path::Path, sync::atomic::AtomicUsize};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

const RUNNER: &str = "runner-prospective-boundary-aaaaaaaaaaaaaaaaaaaaaaaa";
const VIEWER: &str = "viewer-prospective-boundary-bbbbbbbbbbbbbbbbbbbbbbbb";

struct BoundaryFixture {
    roots: Option<(tempfile::TempDir, tempfile::TempDir)>,
    store: Arc<crate::audit::Store>,
    journal: crate::broker::journal::Journal,
    intent: crate::broker::intent::ExecutionIntentV1,
    registration: RegistrationV1,
}
impl From<LiveFixture> for BoundaryFixture {
    fn from(value: LiveFixture) -> Self {
        Self {
            roots: Some((value._root, value._store_root)),
            store: Arc::new(value.store),
            journal: value.journal,
            intent: value.intent,
            registration: value.registration,
        }
    }
}
impl Drop for BoundaryFixture {
    fn drop(&mut self) {
        // Writer Drop is deliberately nonblocking. Preserve these private
        // roots rather than deleting a namespace before its native return is
        // independently observed; the test does not invent a writer join.
        if let Some((journal, store)) = self.roots.take() {
            let _journal = journal.keep();
            let _store = store.keep();
        }
    }
}

#[derive(Default)]
struct Posts {
    total: AtomicUsize,
    arm: AtomicUsize,
    claim: AtomicUsize,
    cleanup: AtomicUsize,
}
struct Gateway {
    stop: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}
impl Gateway {
    async fn start(
        listener: TcpListener,
        fixture: &BoundaryFixture,
        posts: Arc<Posts>,
    ) -> Result<Self> {
        // The same real database is retained; no response DTO or fake receipt
        // is substituted for the authenticated HTTP endpoint.
        let state = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: fixture.journal.scope().tenant_id().into(),
                store: Arc::clone(&fixture.store),
                runner_token: RUNNER.into(),
                viewer_token: VIEWER.into(),
            }],
            crate::policy::Policy::default(),
            None,
            Redactor::new(vec![]),
        )?;
        let cleanup_path = format!(
            "/control/api/v1/workers/{}/helper-cleanup",
            fixture.registration.arm().worker_id
        );
        let app = gateway_router(state).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let posts = Arc::clone(&posts);
                let is_cleanup = request.uri().path() == cleanup_path.as_str();
                async move {
                    if request.method() == axum::http::Method::POST {
                        posts.total.fetch_add(1, Ordering::SeqCst);
                        if is_cleanup {
                            posts.cleanup.fetch_add(1, Ordering::SeqCst);
                        }
                        if request.uri().path().ends_with("/arms") {
                            posts.arm.fetch_add(1, Ordering::SeqCst);
                        }
                        if request.uri().path().ends_with("/armed-start-claims")
                            || request.uri().path().ends_with("/helper-start-claims")
                        {
                            posts.claim.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    next.run(request).await
                }
            },
        ));
        let (stop, receive) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = receive.await;
                })
                .await
        });
        Ok(Self { stop, task })
    }
    async fn finish(self) -> Result<()> {
        let _ = self.stop.send(());
        tokio::time::timeout(Duration::from_secs(5), self.task).await???;
        Ok(())
    }
}

async fn context(fixture: &BoundaryFixture, origin: &str) -> Result<AuthenticatedBatchContext> {
    let runtime = Runtime::with_program(RuntimeKind::Docker, Path::new(INVOCATION_DATA_PROGRAM))?;
    AuthenticatedBatchContext::for_executor_test(
        AuditClient::new(origin, RUNNER)?,
        fixture.journal.scope().clone(),
        Directory::open_existing(&fixture.intent.initial_options().run_options().config_dir)?,
        runtime,
    )
    .await
}

#[derive(Clone, Copy)]
enum Refusal {
    Capacity,
    Cancelled,
    WriterUnknown,
}

async fn refused_before_arm(scenario: Refusal) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let fixture = BoundaryFixture::from(live_fixture_with_created_helper_for_prospective(&origin));
    let posts = Arc::new(Posts::default());
    let gateway = Gateway::start(listener, &fixture, Arc::clone(&posts)).await?;
    let context = context(&fixture, &origin).await?;
    let mut native = ProspectiveFixture::start(fixture.journal.scope().installation_id(), 1).await;
    let full = native.client().await;
    if matches!(scenario, Refusal::Capacity) {
        let target = fixture.registration.prospective()?;
        native.install(&target);
        full.reserve_prospective(target, context.cancel_generation().clone(), false)
            .await?;
    }
    let enforcer = ExecutionEnforcerV1::Full3(full);
    let result = async {
        let writer = WriterClient::start(fixture.journal.clone(), 16, Duration::from_secs(10))?;
        let executor =
            BatchExecutor::new(writer.clone(), enforcer, Arc::new(NativeHostClock::new()?));
        let execution = Execution::open(&executor, fixture.intent.operation_id(), &context).await?;
        let (before, draft) = execution.load().await?;
        ensure!(
            !draft
                .operations
                .iter()
                .any(|op| matches!(op, Operation::Arm { .. })),
            "Created fixture already has an Arm"
        );
        let helper = draft
            .helpers
            .iter()
            .find(|helper| helper.owned().is_some())
            .context("Created helper missing")?;
        if matches!(scenario, Refusal::Cancelled) {
            context.cancel_generation().invalidate();
        }
        let target = fixture.registration.prospective()?;
        let mut barrier = if matches!(scenario, Refusal::WriterUnknown) {
            Some(writer.advance_reply_barrier_for_test(true).await?)
        } else { None };
        let attempt = execution.arm_and_claim_bound(
                target.authority(),
                helper.owned().unwrap().clone(),
                Some((helper.policy(), helper.permission())),
            );
        tokio::pin!(attempt);
        if let Some(held) = barrier.as_mut() {
            tokio::select! {
                _ = &mut attempt => return Err(anyhow!("writer failed before reaching its real publication barrier")),
                entered = tokio::time::timeout(Duration::from_secs(5), held.entered()) => {
                    entered.context("writer publication barrier was not reached")?;
                },
            }
        }
        drop(barrier);
        let outcome = attempt.await;
        let _error = outcome
            .err()
            .context("unprotected fresh Arm was accepted")?;
        ensure!(
            posts.arm.load(Ordering::SeqCst) == 0 && posts.claim.load(Ordering::SeqCst) == 0,
            "refusal occurred after HTTP Arm/claim"
        );
        let strict = execution.load().await;
        if matches!(scenario, Refusal::Capacity | Refusal::WriterUnknown) {
            if matches!(scenario, Refusal::WriterUnknown) {
                ensure!(executor.enforcer.full3().unwrap().require_new_prearm_identity().is_err(),
                    "unknown pending publication left new identities enabled");
                ensure!(executor.enforcer.status().await?.slot_count() == 0,
                    "native owner was allocated before the failed pending publication");
            }
            let known = match strict {
                Ok((head, draft)) => {
                    let pending: Vec<_> = draft.operations.iter().filter_map(|op| match op {
                        Operation::Arm {request,response:None,..} => Some(request), _=>None,
                    }).collect();
                    ensure!(pending.len() == 1, "refusal lost or duplicated the exact pending Arm");
                    Some((head.head().clone(), pending[0].operation_id.clone(), pending[0].deadline_ns_decimal.clone()))
                }
                Err(_) if matches!(scenario, Refusal::WriterUnknown) => None,
                Err(error) => return Err(error),
            };
            let retry = execution.arm_and_claim_bound(target.authority(), helper.owned().unwrap().clone(),
                Some((helper.policy(), helper.permission()))).await;
            ensure!(retry.is_err(), "unowned pending retry acquired a new permission");
            ensure!(posts.arm.load(Ordering::SeqCst) == 0 && posts.claim.load(Ordering::SeqCst) == 0,
                "unowned/unknown pending retry issued an Arm or claim RPC");
            match (known, execution.load().await) {
                (Some((head, arm_id, deadline)), Ok((after, draft))) => {
                    let pending: Vec<_> = draft.operations.iter().filter_map(|op| match op {
                        Operation::Arm {request,..} => Some(request), _=>None,
                    }).collect();
                    let head_changed = after.head() != &head;
                    let count = pending.len();
                    let id_same = pending.first().is_some_and(|arm| arm.operation_id == arm_id);
                    let deadline_same = pending.first().is_some_and(|arm| arm.deadline_ns_decimal == deadline);
                    ensure!(!head_changed && count == 1 && id_same && deadline_same,
                        "refused retry changed pending identity, deadline or durable head: head_changed={head_changed} count={count} id_same={id_same} deadline_same={deadline_same} revision_before={} revision_after={}",
                        head.revision(), after.head().revision());
                }
                (None, Err(_)) => {}, // Strict publication remains UNKNOWN.
                _ => return Err(anyhow!("unknown pending publication was silently reclassified")),
            }
        } else {
            let (after, draft) = strict?;
            ensure!(before.head() == after.head(), "pre-ACK refusal changed the durable head");
            ensure!(!draft.operations.iter().any(|op| matches!(op, Operation::Arm {..})), "refusal persisted a new Arm");
        }
        Ok(())
    }
    .await;
    native.finish().await;
    gateway.finish().await?;
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_prospective_capacity_refuses_before_arm_rpc() {
    Box::pin(refused_before_arm(Refusal::Capacity))
        .await
        .unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_generation_refuses_before_arm_rpc() {
    Box::pin(refused_before_arm(Refusal::Cancelled))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_pending_writer_ack_never_generates_another_arm_or_rpc() {
    Box::pin(refused_before_arm(Refusal::WriterUnknown))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_arm_ack_replays_same_owner_and_deadline_without_claim() {
    let result = Box::pin(lost_ack()).await;
    result.unwrap();
}
async fn lost_ack() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let fixture = BoundaryFixture::from(live_fixture_with_pending_arm_for_prospective(&origin));
    let posts = Arc::new(Posts::default());
    let gateway = Gateway::start(listener, &fixture, Arc::clone(&posts)).await?;
    let context = context(&fixture, &origin).await?;
    let target = fixture.registration.prospective()?;
    let mut native = ProspectiveFixture::start(fixture.journal.scope().installation_id(), 1).await;
    native.install(&target);
    let full = native.client().await;
    full.reserve_prospective(target.clone(), context.cancel_generation().clone(), false)
        .await?;
    let _issued = full
        .begin_prospective_arm_exchange(&target, context.cancel_generation())
        .await?;
    let result = async {
        let writer = WriterClient::start(fixture.journal.clone(), 16, Duration::from_secs(10))?;
        let executor = BatchExecutor::new(
            writer.clone(),
            ExecutionEnforcerV1::Full3(full.clone()),
            Arc::new(NativeHostClock::new()?),
        );
        let execution = Execution::open(&executor, fixture.intent.operation_id(), &context).await?;
        let (_, draft) = execution.load().await?;
        let helper = draft
            .helpers
            .iter()
            .find(|helper| helper.owned().is_some())
            .context("pending helper missing")?;
        ensure!(
            draft
                .operations
                .iter()
                .any(|op| matches!(op, Operation::Arm { response: None, .. })),
            "lost ACK pending missing"
        );
        let before = serde_json::to_vec(&fixture.store.worker(&target.arm().worker_id)?)?;
        let outcome = execution
            .arm_and_claim_bound(
                target.authority(),
                helper.owned().unwrap().clone(),
                Some((helper.policy(), helper.permission())),
            )
            .await?;
        ensure!(
            matches!(outcome, PrearmedStartDirective::ObserveOnly),
            "pending Arm replay created a fresh permission"
        );
        ensure!(
            posts.arm.load(Ordering::SeqCst) == 1 && posts.claim.load(Ordering::SeqCst) == 0,
            "lost ACK replay emitted an extra claim or Arm"
        );
        ensure!(
            before == serde_json::to_vec(&fixture.store.worker(&target.arm().worker_id)?)?,
            "idempotent Arm replay changed worker state"
        );
        let (_, draft) = execution.load().await?;
        let arms: Vec<_> = draft
            .operations
            .iter()
            .filter_map(|op| match op {
                Operation::Arm {
                    request,
                    response: Some(response),
                    ..
                } => Some((request, response)),
                _ => None,
            })
            .collect();
        ensure!(
            arms.len() == 1
                && arms[0].0.operation_id == target.arm_id()
                && arms[0].0.deadline_ns_decimal == target.arm().deadline_ns_decimal
                && arms[0].1.receipt.input_hash == fixture.registration.receipt().input_hash,
            "lost ACK changed identity, input or original deadline"
        );
        full.reserve_prospective(target, context.cancel_generation().clone(), true)
            .await?;
        Ok(())
    }
    .await;
    native.finish().await;
    gateway.finish().await?;
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_cleanup_reconciles_same_pending_after_writer_ack_loss() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let fixture = BoundaryFixture::from(live_fixture_with_gateway(&origin, true));
    let posts = Arc::new(Posts::default());
    let gateway = Gateway::start(listener, &fixture, Arc::clone(&posts)).await?;
    let exercise =
        boxed_execution_phase(|| exercise_import_cleanup(&fixture, &origin, &posts)).await;
    let stopped = gateway.finish().await;
    exercise.and(stopped)
}

async fn exercise_import_cleanup(
    fixture: &BoundaryFixture,
    origin: &str,
    posts: &Posts,
) -> Result<()> {
    let context = context(fixture, origin).await?;
    let writer = WriterClient::start(fixture.journal.clone(), 16, Duration::from_secs(10))?;
    let executor = BatchExecutor::new(
        writer.clone(),
        EnforcerClient::unavailable_for_test(context.config())?,
        Arc::new(NativeHostClock::new()?),
    );
    let operation = fixture.intent.operation_id();
    let execution = Execution::open(&executor, operation, &context).await?;
    let arm = fixture.registration.arm();
    let ArmSubjectV1::Helper {
        helper_id,
        purpose: HelperPurposeV1::Import,
        ..
    } = &arm.subject
    else {
        return Err(anyhow!("fixture must contain an acknowledged Import Arm"));
    };
    let arm_receipt = fixture.registration.receipt();
    // Both requests are synthetic data. No runtime or guardian can consume the claim.
    boxed_execution_phase(|| {
        execution.rpc(Operation::HelperClaim {
            request: HelperStartClaimV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                helper_id: helper_id.clone(),
                emitter_id: Uuid::new_v4().to_string(),
                arm_receipt: arm_receipt.clone(),
                clock_check: arm.clock.clone(),
            },
            response: None,
        })
    })
    .await
    .context("seed durable Import claim")?;
    let finished_at = "2026-10-30T12:00:02Z".to_owned();
    boxed_execution_phase(|| {
        execution.rpc(Operation::HelperObservation {
            request: HelperObservationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                helper_id: helper_id.clone(),
                arm_receipt: arm_receipt.clone(),
                verification: None,
                observation: RuntimeObservationV1 {
                    inspection: arm.inspection.clone(),
                    observed_at: finished_at.clone(),
                    status: RuntimeStatusV1::Exited,
                    running: Some(false),
                    started_at: Some("2026-10-30T12:00:00Z".into()),
                    finished_at: Some(finished_at),
                    exit_code: Some(0),
                    oom_killed: Some(false),
                    restart_count: Some(0),
                },
            },
            response: None,
        })
    })
    .await
    .context("seed durable Import terminal observation")?;

    let request = HelperCleanupV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: helper_id.clone(),
        arm_receipt: arm_receipt.clone(),
        removed: true,
        error: None,
    };
    let pending = Operation::HelperCleanup {
        request: request.clone(),
        response: None,
    };
    let audit_head = fixture.store.head()?;
    let posts_before = posts.total.load(Ordering::SeqCst);
    let expected = writer
        .load(operation)
        .await?
        .context("terminal Import job missing")?
        .head()
        .clone();
    let revision = expected.revision();
    let mut barrier = writer
        .cleanup_advance_reply_barrier_for_test(operation, expected, request.input_hash()?)
        .await?;
    let mut attempt = boxed_execution_phase(|| execution.rpc(pending.clone()));
    let started = tokio::time::Instant::now();
    let deadline = started + Duration::from_secs(5);
    let failure = tokio::select! {
        result = &mut attempt => {
            let category = match result {
                Ok(_) => "ok".to_owned(),
                Err(error) => error.downcast_ref::<JournalError>()
                    .map_or_else(|| "non_journal_error".to_owned(), |error| format!("{error:?}")),
            };
            Some(("Cleanup returned before its Writer reply barrier", category))
        }
        reached = tokio::time::timeout_at(deadline, barrier.entered()) => reached
            .err()
            .map(|_| ("Cleanup did not reach its real Writer reply barrier", "deadline_elapsed".to_owned())),
    };
    if let Some((message, category)) = failure {
        let sampled = tokio::time::Instant::now();
        let http_cleanup_entries = posts.cleanup.load(Ordering::SeqCst);
        let http_new_post_entries = posts.total.load(Ordering::SeqCst) - posts_before;
        let trace = barrier
            .cleanup_trace_for_test(started.into_std(), sampled.min(deadline).into_std())
            .unwrap_or_else(|| "UNKNOWN".to_owned());
        drop(attempt);
        drop(barrier);
        return Err(anyhow!(
            "{message}; cleanup_trace cause={category} observed_ms={} {trace} http_cleanup_path_entries_at_sample={http_cleanup_entries} http_post_entries_since_seed_at_sample={http_new_post_entries}",
            sampled.duration_since(started).as_millis()
        ));
    }
    drop(barrier);
    let error = attempt
        .await
        .err()
        .context("lost Writer ACK accepted for Cleanup")?;
    ensure!(
        matches!(error.downcast_ref::<JournalError>(), Some(JournalError::Io)),
        "expected the Writer's uncertain publication error"
    );
    let (pending_revision, saved) = import_cleanup_checkpoint(&execution).await?;
    ensure!(
        pending_revision == revision + 1 && same(&saved, &pending)?,
        "the exact pending Cleanup must have committed before the lost ACK"
    );
    ensure!(
        posts.total.load(Ordering::SeqCst) == posts_before
            && posts.cleanup.load(Ordering::SeqCst) == 0
            && fixture
                .store
                .worker_operation(&request.operation_id)?
                .is_none()
            && fixture.store.head()? == audit_head,
        "Cleanup reached the gateway or Store before the Writer returned an ACK"
    );
    drop(execution);

    let resumed = Execution::open(&executor, operation, &context).await?;
    boxed_execution_phase(|| resumed.reconcile_pending()).await?;
    let (ack_revision, saved) = import_cleanup_checkpoint(&resumed).await?;
    ensure!(
        ack_revision == pending_revision + 1 && same(&without_response(&saved), &pending)?,
        "reconciliation must ACK the original Cleanup request"
    );
    let Operation::HelperCleanup {
        response: Some(ack),
        ..
    } = saved
    else {
        return Err(anyhow!("durable Cleanup ACK missing after reconciliation"));
    };
    let receipt = fixture
        .store
        .worker_operation(&request.operation_id)?
        .context("reconciled Cleanup Store receipt missing")?;
    ensure!(
        posts.total.load(Ordering::SeqCst) == posts_before + 1
            && posts.cleanup.load(Ordering::SeqCst) == 1
            && fixture.store.head()? == audit_head + 1
            && receipt == ack
            && ack.audit_seq == audit_head + 1
            && ack.operation_id == request.operation_id
            && ack.input_hash == request.input_hash()?
            && ack.worker_id.as_deref() == Some(arm_receipt.worker_id.as_str())
            && ack.revision.is_none(),
        "reconciliation must persist one matching Store receipt and journal ACK"
    );
    Ok(())
}

async fn import_cleanup_checkpoint(execution: &Execution<'_>) -> Result<(u64, Operation)> {
    let (job, draft) = execution.load().await?;
    let mut cleanups = draft
        .operations
        .into_iter()
        .filter(|op| matches!(op, Operation::HelperCleanup { .. }));
    let cleanup = cleanups
        .next()
        .context("Cleanup missing in the durable journal")?;
    ensure!(
        cleanups.next().is_none(),
        "multiple Cleanup operations in the durable journal"
    );
    Ok((job.head().revision(), cleanup))
}
