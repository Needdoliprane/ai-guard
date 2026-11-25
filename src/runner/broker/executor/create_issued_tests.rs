//! Real loopback gateway, SQLite and Writer, with synthetic helper history.
//! The worker port under test ends at acknowledgement. No engine is invoked.
use super::{export_retirement_tests::seed_before_create_issued, *};
use crate::{
    broker::journal::retirement_live_fixture::INVOCATION_DATA_PROGRAM, runtime::RuntimeAuthority,
};
use tokio::time::{Instant, timeout_at};

#[test]
fn create_issued_commit_without_http_reply_keeps_exact_pending() -> Result<()> {
    with_prepared_worker(
        Duration::from_secs(30),
        |handle, fixture, execution, worker| {
            handle.block_on(boxed_execution_phase(|| {
                exercise_http_cut(fixture, execution, worker)
            }))
        },
    )
}

#[test]
fn create_issued_writer_reply_timeout_leaves_durable_ack_unconfirmed_to_caller() -> Result<()> {
    // The existing generic Writer barrier releases itself after ten seconds.
    // Its hold must exceed the fixture client's receive deadline.
    with_prepared_worker(
        Duration::from_secs(8),
        |handle, fixture, execution, worker| {
            handle.block_on(boxed_execution_phase(|| {
                exercise_writer_reply_timeout(fixture, execution, worker)
            }))
        },
    )
}

fn with_prepared_worker(
    writer_timeout: Duration,
    exercise: fn(&tokio::runtime::Handle, &Fixture, &Execution<'_>, &WorkerV1) -> Result<()>,
) -> Result<()> {
    let scheduler = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let handle = scheduler.handle().clone();
    scheduler.block_on(async move {
        // Keep the I/O driver polling while the reused, synchronous seed runs.
        tokio::task::spawn_blocking(move || {
            let fixture = handle.block_on(Fixture::new_with_writer_timeout(writer_timeout));
            let peer = handle.block_on(NativePeer::for_admission(&fixture));
            let runtime =
                Runtime::with_program(RuntimeKind::Docker, Path::new(INVOCATION_DATA_PROGRAM))?;
            let authority: RuntimeAuthority = serde_json::from_value(json!({
                "authority_id": Uuid::new_v4(), "kind": "docker",
                "endpoint": "unix:///create-issued-fixture.sock", "engine_id": "create-issued-fixture",
                "invocation": runtime.invocation(), "podman": null,
            }))?;
            let context = handle.block_on(AuthenticatedBatchContext::for_executor_test(
                fixture.client.clone(),
                fixture.scope.clone(),
                fixture.config.clone(),
                runtime,
            ))?;
            let executor = BatchExecutor::new(
                fixture.writer().clone(),
                peer.client.clone(),
                Arc::new(NativeHostClock::new()?),
            );
            let operation = Uuid::new_v4();
            let accepted = fixture.accept(operation, false);
            let execution = handle.block_on(Execution::open(&executor, operation, &context))?;
            let worker =
                seed_before_create_issued(&handle, &execution, &fixture, &accepted, &authority)?;
            handle.block_on(boxed_execution_phase(|| ack_verification_cleanup(&execution)))?;

            execution.stage(LocalFailurePhaseV1::ImportVerification);
            let result = exercise(&handle, &fixture, &execution, &worker);
            let slots = handle.block_on(peer.client.status())?.slot_count();
            drop(execution);
            drop(executor);
            handle.block_on(peer.close());
            ensure!(slots == 0, "the fixture must not occupy a native worker slot");
            result
        })
        .await?
    })
}

async fn ack_verification_cleanup(execution: &Execution<'_>) -> Result<()> {
    // The shared seed finishes ImportVerification last, using the real
    // helper observation RPC. Model its cleanup receipt without removing a container.
    let (_, draft) = execution.load().await?;
    let Some(Operation::HelperObservation {
        request,
        response: Some(_),
    }) = draft.operations.last()
    else {
        anyhow::bail!("fixture lacks the acknowledged ImportVerification terminal");
    };
    ensure!(
        request.verification.is_some(),
        "fixture terminal lacks verification"
    );
    let cleanup = HelperCleanupV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: request.helper_id.clone(),
        arm_receipt: request.arm_receipt.clone(),
        removed: true,
        error: None,
    };
    let (ack, _) = execution
        .rpc(Operation::HelperCleanup {
            request: cleanup,
            response: None,
        })
        .await?;
    ensure!(
        matches!(
            ack,
            Operation::HelperCleanup {
                response: Some(_),
                ..
            }
        ),
        "fixture lacks the ImportVerification cleanup ACK"
    );
    Ok(())
}

struct BeforeCreateIssued {
    journal_revision: u64,
    operations: usize,
    worker_posts: usize,
    arm_posts: usize,
    audit_seq: i64,
}

impl BeforeCreateIssued {
    fn read(fixture: &Fixture, execution: &Execution<'_>, worker: &WorkerV1) -> Result<Self> {
        let job = fixture
            .journal()
            .load(execution.operation)?
            .context("fixture journal is missing")?;
        let draft = Draft::from_job(&job)?;
        ensure!(
            worker.state == WorkerStateV1::Preparing
                && worker.quota_held
                && worker.attempt.create_operation_id.is_none()
                && same(draft.worker()?, worker)?,
            "fixture must start before the CreateIssued mutation"
        );
        Ok(Self {
            journal_revision: job.head().revision(),
            operations: draft.operations.len(),
            worker_posts: fixture.worker_requests.load(Ordering::SeqCst),
            arm_posts: fixture.arm_requests.load(Ordering::SeqCst),
            audit_seq: fixture.store.head()?,
        })
    }
}

async fn exercise_http_cut(
    fixture: &Fixture,
    execution: &Execution<'_>,
    worker: &WorkerV1,
) -> Result<()> {
    let before = BeforeCreateIssued::read(fixture, execution, worker)?;
    fixture
        .drop_next_worker_transition_response
        .store(true, Ordering::SeqCst);
    let error = tokio::time::timeout(
        Duration::from_secs(60),
        boxed_execution_phase(|| execution.mutate_confirmed(WorkerTransitionV1::CreateIssued)),
    )
    .await
    .context("CreateIssued client call did not finish after the HTTP response cut")?
    .err()
    .context("a cut HTTP reply must not return a confirmed mutation")?;
    ensure!(
        !fixture
            .drop_next_worker_transition_response
            .load(Ordering::SeqCst),
        "the successful handler response must have been cut"
    );

    let current = fixture
        .journal()
        .load(execution.operation)?
        .context("CreateIssued journal missing")?;
    let draft = Draft::from_job(&current)?;
    let Some(Operation::WorkerMutation {
        request,
        response: None,
    }) = draft.operations.last()
    else {
        anyhow::bail!("the local journal must end in a pending mutation");
    };
    ensure!(
        current.head().revision() == before.journal_revision + 1
            && draft.operations.len() == before.operations + 1
            && same(draft.worker()?, worker)?
            && draft
                .run
                .plan
                .managed
                .as_ref()
                .is_some_and(|managed| managed.owned.is_none())
            && matches!(request.transition, WorkerTransitionV1::CreateIssued)
            && request.worker_id == worker.worker_id
            && request.attempt_id == worker.attempt.attempt_id
            && request.expected_revision == worker.revision,
        "the exact CreateIssued must remain pending, with the original worker and no ownership"
    );
    assert_single_sqlite_create_issued(fixture, &before, worker, request)?;
    assert_report(execution, &error, json!({"stage":"http_exchange"}))
}

fn assert_single_sqlite_create_issued(
    fixture: &Fixture,
    before: &BeforeCreateIssued,
    worker: &WorkerV1,
    request: &WorkerMutationV1,
) -> Result<MutationReceiptV1> {
    let receipt = fixture
        .store
        .worker_operation(&request.operation_id)?
        .context("SQLite has no committed mutation for the pending operation")?;
    let applied = fixture
        .store
        .worker(&worker.worker_id)?
        .context("SQLite worker missing")?;
    let events = fixture.store.events(&EventQuery {
        after: Some(before.audit_seq),
        limit: Some(2),
        ..Default::default()
    })?;
    let [event] = events.as_slice() else {
        anyhow::bail!("expected exactly one new durable SQLite event");
    };
    ensure!(
        fixture.worker_requests.load(Ordering::SeqCst) == before.worker_posts + 1
            && fixture.arm_requests.load(Ordering::SeqCst) == before.arm_posts
            && receipt.operation_id == request.operation_id
            && receipt.worker_id.as_deref() == Some(request.worker_id.as_str())
            && receipt.revision == Some(worker.revision + 1)
            && receipt.input_hash == content_hash("ai-guard:worker-mutation:v1", request)?
            && receipt.audit_seq == before.audit_seq + 1
            && event.seq == receipt.audit_seq
            && event.hash == receipt.audit_hash
            && event.event.kind == "control.worker.transition"
            && event.event.data["action"] == "create_issued"
            && event.event.data["operation_id"] == request.operation_id,
        "the one SQLite commit and POST must match the pending CreateIssued"
    );
    ensure!(
        applied.revision == worker.revision + 1
            && applied.state == WorkerStateV1::Preparing
            && applied.quota_held
            && applied.attempt.attempt_id == request.attempt_id
            && applied.attempt.create_operation_id.as_deref()
                == Some(request.operation_id.as_str())
            && applied.attempt.container_id.is_none()
            && applied.attempt.start_operation_id.is_none()
            && applied.last_observation.is_none()
            && fixture.store.worker_usage()?.is_empty(),
        "CreateIssued may advance intent only, with no worker runtime facts or quota release"
    );
    Ok(receipt)
}

async fn exercise_writer_reply_timeout(
    fixture: &Fixture,
    execution: &Execution<'_>,
    worker: &WorkerV1,
) -> Result<()> {
    let before = BeforeCreateIssued::read(fixture, execution, worker)?;
    let request = WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition: WorkerTransitionV1::CreateIssued,
    };
    let pending = Operation::WorkerMutation {
        request: request.clone(),
        response: None,
    };
    // Confirm the pending first. The next advance on this isolated Writer
    // belongs to the ACK of the real transition response.
    execution
        .patch(|draft| {
            draft.operations.push(pending.clone());
            Ok(())
        })
        .await?;
    let (prepared, draft) = execution.load().await?;
    ensure!(
        prepared.head().revision() == before.journal_revision + 1
            && same(
                draft.operations.last().context("missing pending")?,
                &pending
            )?
            && fixture
                .store
                .worker_operation(&request.operation_id)?
                .is_none()
            && fixture.worker_requests.load(Ordering::SeqCst) == before.worker_posts,
        "the pending must be durable before any CreateIssued POST"
    );
    drop(draft);
    let mut barrier = fixture
        .writer()
        .advance_reply_barrier_for_test(false)
        .await?;
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut rpc = boxed_execution_phase(|| execution.rpc_checked_at_head(pending.clone(), None));
    tokio::select! {
        biased;
        _ = &mut rpc => anyhow::bail!("CreateIssued returned before the ACK Writer barrier"),
        reached = timeout_at(deadline, barrier.entered()) => {
            reached.context("CreateIssued did not reach the ACK Writer barrier")?;
        }
    }
    let error = timeout_at(deadline, rpc)
        .await
        .context("CreateIssued did not return at the Writer deadline")?
        .err()
        .context("CreateIssued returned a confirmed ACK while the Writer reply was held")?;
    barrier.release_for_test();
    ensure!(
        matches!(
            error.downcast_ref::<WriterCallFailure>(),
            Some(WriterCallFailure::ReplyTimeout)
        ),
        "the Writer client must report a typed reply timeout"
    );
    assert_report(
        execution,
        &error,
        json!({"stage":"ack_advance","cause":"reply_timeout"}),
    )?;

    // FIFO read after release. A barrier entry alone can also follow a
    // failed advance, so only the exact retained ACK confirms publication.
    let (current, draft) = timeout_at(deadline, execution.load())
        .await
        .context("failed to bound the confirming Writer read")??;
    let receipt = assert_single_sqlite_create_issued(fixture, &before, worker, &request)?;
    let Some(
        last @ Operation::WorkerMutation {
            response: Some(response),
            ..
        },
    ) = draft.operations.last()
    else {
        anyhow::bail!("the exact local ACK must be durable despite the caller timeout");
    };
    ensure!(
        current.head().revision() == prepared.head().revision() + 1
            && draft.operations.len() == before.operations + 1
            && same(&without_response(last), &pending)?
            && response.receipt == receipt
            && same(draft.worker()?, &response.worker)?
            && draft
                .run
                .plan
                .managed
                .as_ref()
                .is_some_and(|managed| managed.owned.is_none()),
        "the only local advance must acknowledge the one committed CreateIssued"
    );
    Ok(())
}

fn assert_report(execution: &Execution<'_>, error: &anyhow::Error, expected: Value) -> Result<()> {
    let report = serde_json::to_value(diagnostics::failure_report(execution.phase(), error))?;
    ensure!(
        report["phase"] == "worker"
            && report["broker_step"] == "failure_observed"
            && report["runtime_effect"] == "unknown"
            && report["create_issued"] == expected,
        "CreateIssued must report its exact closed boundary with runtime effect unknown"
    );
    Ok(())
}
