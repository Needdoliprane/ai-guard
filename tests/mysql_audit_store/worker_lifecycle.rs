use agentsandbox::{
    control::{
        AdmissionRefusalV1, AllocationProvenanceV1, AllocationSourceV1, AllocationV1,
        ClaimDispositionV1, ControlFailure, MutationReceiptV1, MutationResultV1, QuotaPolicyV1,
        RuntimeInspectionV1, RuntimeObservationV1, RuntimeStatusV1, WorkerMutationV1,
        WorkerStateV1, WorkerTransitionV1, WorkerV1,
    },
    control_fs::Directory,
    mysql_storage::{MysqlProbeError, audit::MysqlAuditStore},
    policy::{self, Policy},
};
use anyhow::{Context, ensure};
use mysql_async::{Conn, prelude::Queryable};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt, path::PathBuf, time::Duration};

const RESTORE_TENANT: &str = "tenant-qa-sql05";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreWitness {
    source_server: uuid::Uuid,
    source_port: u16,
    namespace: uuid::Uuid,
    mtls: bool,
    claim: WorkerMutationV1,
    receipt: MutationReceiptV1,
    worker: WorkerV1,
    payload_bytes: u64,
}

#[tokio::test]
#[ignore = "requires two isolated MySQL 8.4 QA servers and an operator-prepared backup restore"]
async fn restored_mysql_legacy_claim_retains_receipt_and_quota() -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(120), Box::pin(restore_phase()))
        .await
        .context("SQL-05 restore phase timed out")?
}

async fn restore_phase() -> anyhow::Result<()> {
    let prepare = match std::env::var("AI_GUARD_MYSQL_RESTORE_PHASE")?.as_str() {
        "prepare" => true,
        "verify" => false,
        _ => anyhow::bail!("SQL-05 restore phase must be prepare or verify"),
    };
    let path = PathBuf::from(
        std::env::var_os("AI_GUARD_MYSQL_RESTORE_WITNESS")
            .context("SQL-05 private witness path is required")?,
    );
    let parent = Directory::open_existing(path.parent().context("witness parent is missing")?)?;
    ensure!(path.file_name().is_some(), "witness filename is missing");
    let expected = if prepare {
        ensure!(
            path.symlink_metadata()
                .is_err_and(|err| err.kind() == std::io::ErrorKind::NotFound),
            "SQL-05 witness path must be new"
        );
        None
    } else {
        Some(serde_json::from_slice::<RestoreWitness>(
            super::load(&path, 16_384)?.bytes(),
        )?)
    };
    let fixture = super::identity::FixtureFiles::open_gateway("AI_GUARD_MYSQL_RESTORE_CONFIG")?;
    let namespace = fixture.target.namespace;
    let mtls = fixture.target.client_cert_file.is_some();
    let mut connection = fixture.maintenance().await?;
    let (server, database, account) = connection
        .query_first::<(String, String, String), _>(
            "SELECT @@server_uuid, DATABASE(), CURRENT_USER()",
        )
        .await
        .map_err(|_| anyhow::anyhow!("cannot verify the SQL-05 maintenance session"))?
        .context("SQL-05 maintenance session is missing")?;
    let server = uuid::Uuid::parse_str(&server)?;
    ensure!(
        !server.is_nil()
            && database == fixture.target.database
            && account.rsplit_once('@').map(|(user, _)| user)
                == Some(fixture.target.maintenance_username.as_str()),
        "SQL-05 maintenance session uses a different server identity, database or account"
    );
    let store = fixture.target.open(
        &fixture.target.maintenance_username,
        fixture.credentials(&fixture.maintenance_password)?,
        RESTORE_TENANT,
        namespace,
        128,
    )?;
    let policy = Policy::default();
    let quota = QuotaPolicyV1 {
        max_active_workers: 1,
        ..Default::default()
    };
    if let Some(expected) = expected {
        ensure!(
            server != expected.source_server
                && fixture.target.port != expected.source_port
                && namespace == expected.namespace
                && mtls == expected.mtls,
            "SQL-05 requires a separate restored server with the original namespace and profile"
        );
        connection.disconnect().await?;
        check_restored_claim(&store, &quota, &expected).await?;
        let (next, _) = super::worker::admission(&store, &policy).await?;
        let refused = store
            .admit_worker(
                &next,
                &policy,
                policy::evaluate(&policy, &next.decision_request),
                json!(null),
                &quota,
            )
            .await?;
        ensure!(
            !refused.accepted
                && refused.refusal == Some(AdmissionRefusalV1::Quota)
                && refused.worker.is_none(),
            "restored reservation did not prevent a second admission"
        );
        check_held_quota(&store, &quota, &expected.worker).await?;
        println!("SQL05_RESTORED_LEGACY_CLAIM_VERIFIED");
    } else {
        super::identity::require_fresh(&mut connection).await?;
        connection.disconnect().await?;
        store.initialize().await?;
        let (claim, committed) = prepare_restore_claim(&store, &policy, &quota).await?;
        let head = store.verify().await?;
        ensure!(
            head.seq == committed.receipt.audit_seq && head.hash == committed.receipt.audit_hash,
            "the consumed claim must be the source audit head"
        );
        check_held_quota(&store, &quota, &committed.worker).await?;
        let witness = RestoreWitness {
            source_server: server,
            source_port: fixture.target.port,
            namespace,
            mtls,
            claim,
            receipt: committed.receipt,
            worker: committed.worker,
            payload_bytes: head.payload_bytes,
        };
        parent.revalidate()?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        serde_json::to_writer(&file, &witness)?;
        file.sync_all()?;
        println!("SQL05_SOURCE_CLAIM_PREPARED");
    }
    Ok(())
}

async fn prepare_restore_claim(
    store: &MysqlAuditStore,
    policy: &Policy,
    quota: &QuotaPolicyV1,
) -> anyhow::Result<(WorkerMutationV1, MutationResultV1)> {
    let (admission, _) = super::worker::admission(store, policy).await?;
    let admitted = store
        .admit_worker(
            &admission,
            policy,
            policy::evaluate(policy, &admission.decision_request),
            json!(null),
            quota,
        )
        .await?;
    let reserved = admitted
        .worker
        .context("SQL-05 worker admission was refused")?;
    let preparing = mutate(store, &reserved, WorkerTransitionV1::Prepare)
        .await?
        .worker;
    let issued = mutate(store, &preparing, WorkerTransitionV1::CreateIssued)
        .await?
        .worker;
    let created = mutate(
        store,
        &issued,
        WorkerTransitionV1::Created {
            inspection: inspection(&issued),
        },
    )
    .await?
    .worker;
    let claim = mutation(
        &created,
        WorkerTransitionV1::StartClaim {
            emitter_id: uuid::Uuid::new_v4().to_string(),
        },
    );
    let committed = store.mutate_worker(&claim).await?;
    ensure!(
        committed.disposition == Some(ClaimDispositionV1::EmitOnce)
            && committed.worker.state == WorkerStateV1::StartIssued,
        "source did not commit exactly one start claim"
    );
    Ok((claim, committed))
}

async fn check_restored_claim(
    store: &MysqlAuditStore,
    quota: &QuotaPolicyV1,
    expected: &RestoreWitness,
) -> anyhow::Result<()> {
    let before = store.verify().await?;
    ensure!(
        before.seq == expected.receipt.audit_seq
            && before.hash == expected.receipt.audit_hash
            && before.payload_bytes == expected.payload_bytes,
        "restored audit differs from the claim-point witness"
    );
    ensure!(
        store
            .worker_operation(&expected.claim.operation_id)
            .await?
            .as_ref()
            == Some(&expected.receipt),
        "the original claim receipt must exist before replay"
    );
    let replay = store.mutate_worker(&expected.claim).await?;
    ensure!(
        replay.disposition == Some(ClaimDispositionV1::AlreadyConsumed)
            && replay.receipt == expected.receipt,
        "restored claim did not replay the consumed receipt"
    );
    let worker = store
        .worker(&expected.claim.worker_id)
        .await?
        .context("restored worker is missing")?;
    ensure!(
        worker.state == WorkerStateV1::StartIssued
            && worker.attempt == expected.worker.attempt
            && worker.revision == expected.worker.revision,
        "restored worker lost its original attempt or revision"
    );
    check_held_quota(store, quota, &expected.worker).await?;
    ensure!(
        worker.quota_held && store.verify().await? == before,
        "claim replay changed quota ownership or audit"
    );
    Ok(())
}

async fn check_held_quota(
    store: &MysqlAuditStore,
    quota: &QuotaPolicyV1,
    worker: &WorkerV1,
) -> anyhow::Result<()> {
    let held = store.worker_quota(quota).await?;
    ensure!(
        worker.quota_held
            && held.held_workers == 1
            && held.held_cpu_millis == u64::from(worker.launch.cpu_millis)
            && held.held_memory_bytes == worker.launch.memory_bytes,
        "SQL-05 did not retain the original reservation"
    );
    Ok(())
}

pub(super) async fn exercise(
    store: &MysqlAuditStore,
    peer: &MysqlAuditStore,
    mut maintenance: Conn,
    reopen: impl Fn() -> anyhow::Result<MysqlAuditStore>,
) -> anyhow::Result<()> {
    let policy = Policy::default();
    let quota = QuotaPolicyV1 {
        max_active_workers: 1,
        ..Default::default()
    };
    anyhow::ensure!(store.worker_quota(&quota).await?.held_workers == 0);
    let (admission, _) = super::worker::admission(store, &policy).await?;
    let admitted = store
        .admit_worker(
            &admission,
            &policy,
            policy::evaluate(&policy, &admission.decision_request),
            json!(null),
            &quota,
        )
        .await?;
    let mut worker = admitted.worker.context("worker admission was refused")?;
    anyhow::ensure!(worker.state == WorkerStateV1::Reserved && worker.quota_held);

    worker = mutate(store, &worker, WorkerTransitionV1::Prepare)
        .await?
        .worker;
    anyhow::ensure!(worker.state == WorkerStateV1::Preparing);

    // Two handles race from the same revision. Exactly one CAS may advance it.
    let left = mutation(&worker, WorkerTransitionV1::CreateIssued);
    let right = mutation(&worker, WorkerTransitionV1::CreateIssued);
    let (left_result, right_result) =
        tokio::join!(store.mutate_worker(&left), peer.mutate_worker(&right),);
    let (issued, winner, lost) = match (left_result, right_result) {
        (Ok(result), Err(error)) => (left, result, error),
        (Err(error), Ok(result)) => (right, result, error),
        (Ok(_), Ok(_)) => anyhow::bail!("stale worker revision passed concurrent CAS"),
        (Err(left), Err(right)) => {
            anyhow::bail!("both concurrent worker CAS mutations failed: {left}; {right}")
        }
    };
    anyhow::ensure!(lost.downcast_ref::<ControlFailure>() == Some(&ControlFailure::Conflict));
    worker = winner.worker.clone();
    let replay = peer.mutate_worker(&issued).await?;
    anyhow::ensure!(
        replay.receipt == winner.receipt,
        "CAS replay changed its receipt"
    );
    anyhow::ensure!(worker.state == WorkerStateV1::Preparing);
    let mut different_input = issued.clone();
    different_input.expected_revision = winner.worker.revision;
    let error = peer.mutate_worker(&different_input).await.unwrap_err();
    anyhow::ensure!(error.downcast_ref::<ControlFailure>() == Some(&ControlFailure::Conflict));

    worker = mutate(
        store,
        &worker,
        WorkerTransitionV1::Created {
            inspection: inspection(&worker),
        },
    )
    .await?
    .worker;
    anyhow::ensure!(worker.state == WorkerStateV1::Created);

    let claim = mutation(
        &worker,
        WorkerTransitionV1::StartClaim {
            emitter_id: uuid::Uuid::new_v4().to_string(),
        },
    );
    let claim_result = store.mutate_worker(&claim).await?;
    anyhow::ensure!(
        claim_result.disposition == Some(ClaimDispositionV1::EmitOnce),
        "first start claim must be emitted once"
    );
    let duplicate_claim = peer.mutate_worker(&claim).await?;
    anyhow::ensure!(
        duplicate_claim.receipt == claim_result.receipt
            && duplicate_claim.disposition == Some(ClaimDispositionV1::AlreadyConsumed),
        "replayed start claim must not emit twice"
    );
    worker = claim_result.worker;

    let running = observation(&worker, RuntimeStatusV1::Running);
    worker = mutate(
        store,
        &worker,
        WorkerTransitionV1::Observe {
            observation: running,
        },
    )
    .await?
    .worker;
    anyhow::ensure!(worker.state == WorkerStateV1::Running);

    let mut incomplete = observation(&worker, RuntimeStatusV1::Exited);
    incomplete.exit_code = None;
    worker = mutate(
        store,
        &worker,
        WorkerTransitionV1::Observe {
            observation: incomplete,
        },
    )
    .await?
    .worker;
    anyhow::ensure!(worker.state == WorkerStateV1::InDoubt && worker.quota_held);

    let head_before_failure = store.verify().await?;
    let quota_before_failure = store.worker_quota(&quota).await?;
    maintenance
        .query_drop(
            "ALTER TABLE ai_guard_worker_allocation \
             ADD CONSTRAINT ai_guard_qa_block_worker_allocation CHECK (OCTET_LENGTH(snapshot) = 0)",
        )
        .await?;
    let terminal = mutation(
        &worker,
        WorkerTransitionV1::Observe {
            observation: observation(&worker, RuntimeStatusV1::Exited),
        },
    );
    let failed = store.mutate_worker(&terminal).await;
    maintenance
        .query_drop(
            "ALTER TABLE ai_guard_worker_allocation \
             DROP CHECK ai_guard_qa_block_worker_allocation",
        )
        .await?;
    let error = failed.expect_err("allocation constraint must abort terminal transition");
    anyhow::ensure!(
        error.downcast_ref::<MysqlProbeError>() == Some(&MysqlProbeError::ServerRefused(3819)),
        "allocation failure returned unexpected error: {error}"
    );
    anyhow::ensure!(
        reopen()?.verify().await? == head_before_failure,
        "failed transition changed audit chain"
    );
    anyhow::ensure!(
        serde_json::to_value(store.worker_quota(&quota).await?)?
            == serde_json::to_value(quota_before_failure)?,
        "failed transition changed quota projection"
    );
    let read_allocation =
        "SELECT snapshot,event_seq FROM ai_guard_worker_allocation WHERE attempt_id=?";
    let absent = maintenance
        .exec_first::<(Vec<u8>, i64), _, _>(read_allocation, (&worker.attempt.attempt_id,))
        .await?;
    anyhow::ensure!(absent.is_none(), "failed transition left an allocation");

    let terminal_result = peer.mutate_worker(&terminal).await?;
    anyhow::ensure!(terminal_result.worker.state == WorkerStateV1::Terminal);
    anyhow::ensure!(!terminal_result.worker.quota_held);
    anyhow::ensure!(
        store.worker_quota(&quota).await?.held_workers == 0,
        "terminal worker retained quota"
    );
    let reopened = reopen()?;
    let head_after_terminal = reopened.verify().await?;
    let replay_terminal = reopened.mutate_worker(&terminal).await?;
    anyhow::ensure!(
        replay_terminal.receipt == terminal_result.receipt,
        "terminal retry did not replay exact receipt"
    );
    anyhow::ensure!(
        reopened.verify().await? == head_after_terminal,
        "terminal replay changed the audit head"
    );
    assert_eq!(
        reopened.worker_operation(&admission.operation_id).await?,
        Some(admitted.receipt)
    );
    assert_eq!(
        reopened.worker_operation(&claim.operation_id).await?,
        Some(claim_result.receipt)
    );
    assert_eq!(
        reopened.worker_operation(&terminal.operation_id).await?,
        Some(terminal_result.receipt.clone())
    );
    assert_eq!(reopened.verify().await?, head_after_terminal);
    let (payload, seq) = maintenance
        .exec_first::<(Vec<u8>, i64), _, _>(read_allocation, (&worker.attempt.attempt_id,))
        .await?
        .context("terminal mutation has no allocation projection")?;
    let allocation: AllocationV1 = serde_json::from_slice(&payload)?;
    anyhow::ensure!(
        seq == terminal_result.receipt.audit_seq
            && allocation.terminal_observation_id == terminal.operation_id
            && allocation.worker_id == worker.worker_id
            && allocation.duration_ns == "2000000000"
            && allocation.method_version == 1
            && !allocation.billable,
        "terminal allocation differs from the observed interval"
    );

    let (second_admission, _) = super::worker::admission(store, &policy).await?;
    let second = store
        .admit_worker(
            &second_admission,
            &policy,
            policy::evaluate(&policy, &second_admission.decision_request),
            json!(null),
            &quota,
        )
        .await?;
    anyhow::ensure!(second.accepted, "released worker quota was not reusable");
    anyhow::ensure!(store.worker_quota(&quota).await?.held_workers == 1);
    let second_worker = second.worker.context("second reservation lacks a worker")?;
    let cancelled = mutate(store, &second_worker, WorkerTransitionV1::CancelReserved).await?;
    anyhow::ensure!(store.worker_quota(&quota).await?.held_workers == 0);
    verify_worker_reads(
        &reopened,
        [terminal_result.worker, cancelled.worker],
        &mut maintenance,
    )
    .await?;
    verify_usage_page(&reopened, &mut maintenance, &policy, &quota, allocation).await?;
    Ok(())
}

async fn verify_usage_page(
    store: &MysqlAuditStore,
    maintenance: &mut Conn,
    policy: &Policy,
    quota: &QuotaPolicyV1,
    first: AllocationV1,
) -> anyhow::Result<()> {
    let (admission, _) = super::worker::admission(store, policy).await?;
    let admitted = store
        .admit_worker(
            &admission,
            policy,
            policy::evaluate(policy, &admission.decision_request),
            json!(null),
            quota,
        )
        .await?;
    let reserved = admitted
        .worker
        .context("usage fixture admission was refused")?;
    assert_eq!(store.worker_usage_page(None, 100).await?, [first.clone()]);

    let prepared = mutate(store, &reserved, WorkerTransitionV1::Prepare)
        .await?
        .worker;
    let issued = mutate(store, &prepared, WorkerTransitionV1::CreateIssued)
        .await?
        .worker;
    let mut runtime = inspection(&issued);
    runtime.container_id = "d".repeat(64);
    let created = mutate(
        store,
        &issued,
        WorkerTransitionV1::Created {
            inspection: runtime,
        },
    )
    .await?
    .worker;
    let claimed = mutate(
        store,
        &created,
        WorkerTransitionV1::StartClaim {
            emitter_id: uuid::Uuid::new_v4().to_string(),
        },
    )
    .await?
    .worker;
    let mut terminal = observation(&claimed, RuntimeStatusV1::Exited);
    terminal.started_at = Some("2026-11-20T10:00:04Z".into());
    terminal.observed_at = "2026-11-20T10:00:07Z".into();
    terminal.finished_at = Some(terminal.observed_at.clone());
    let mutation = mutation(
        &claimed,
        WorkerTransitionV1::Observe {
            observation: terminal,
        },
    );
    let result = store.mutate_worker(&mutation).await?;
    assert_eq!(result.worker.state, WorkerStateV1::Terminal);

    let second = AllocationV1 {
        attempt_id: claimed.attempt.attempt_id,
        worker_id: claimed.worker_id,
        terminal_observation_id: mutation.operation_id,
        started_at: "2026-11-20T10:00:04.000000000Z".into(),
        finished_at: "2026-11-20T10:00:07.000000000Z".into(),
        duration_ns: "3000000000".into(),
        cpu_millicore_nanoseconds: (3_000_000_000u128 * u128::from(claimed.launch.cpu_millis))
            .to_string(),
        memory_byte_nanoseconds: (3_000_000_000u128 * u128::from(claimed.launch.memory_bytes))
            .to_string(),
        method_version: 1,
        billable: false,
        source: AllocationSourceV1 {
            authority_id: claimed.launch.authority_id,
            runtime_kind: claimed.launch.runtime_kind,
            container_id: "d".repeat(64),
            spec_hash: claimed.spec_hash,
            provenance: AllocationProvenanceV1::Observed,
        },
    };
    assert_ne!(first.attempt_id, second.attempt_id);
    let mut expected = [first, second];
    expected.sort_by(|left, right| left.attempt_id.cmp(&right.attempt_id));
    let [first, second] = &expected;
    let absent = "00000000-0000-0000-0000-000000000000";
    assert_eq!(store.worker_usage_page(None, 100).await?, expected);
    assert_eq!(
        store.worker_usage_page(Some(absent), 1).await?,
        [first.clone()]
    );
    assert_eq!(
        store.worker_usage_page(Some(&first.attempt_id), 1).await?,
        [second.clone()]
    );
    assert!(
        store
            .worker_usage_page(Some(&second.attempt_id), 1)
            .await?
            .is_empty()
    );
    let invalid_uuid = "00000000-0000-0000-0000-00000000000A";
    for (cursor, limit) in [(None, 0), (None, 101), (Some(invalid_uuid), 1)] {
        assert_eq!(
            store
                .worker_usage_page(cursor, limit)
                .await
                .unwrap_err()
                .downcast_ref::<ControlFailure>(),
            Some(&ControlFailure::InvalidRequest)
        );
    }

    let head = store.verify().await?;
    let mut divergent = second.clone();
    divergent.billable = true;
    let update = "UPDATE ai_guard_worker_allocation SET snapshot=? WHERE attempt_id=?";
    maintenance
        .exec_drop(
            update,
            (serde_json::to_vec(&divergent)?, &second.attempt_id),
        )
        .await?;
    assert_eq!(maintenance.affected_rows(), 1);
    let refused = store.worker_usage_page(None, 1).await;
    maintenance
        .exec_drop(update, (serde_json::to_vec(second)?, &second.attempt_id))
        .await?;
    assert_eq!(maintenance.affected_rows(), 1);
    assert_eq!(
        refused.unwrap_err().to_string(),
        "MySQL worker allocation differs from its audit event"
    );
    assert_eq!(store.verify().await?, head);
    Ok(())
}

async fn verify_worker_reads(
    store: &MysqlAuditStore,
    mut workers: [WorkerV1; 2],
    maintenance: &mut Conn,
) -> anyhow::Result<()> {
    workers.sort_by(|a, b| a.worker_id.cmp(&b.worker_id));
    let [first, second] = &workers;
    let first_json = serde_json::to_value(first)?;
    let second_json = serde_json::to_value(second)?;
    let absent = "00000000-0000-0000-0000-000000000000";
    let invalid_uuid = "00000000-0000-0000-0000-00000000000A";

    assert_eq!(
        serde_json::to_value(store.worker(&first.worker_id).await?)?,
        first_json
    );
    assert!(store.worker(absent).await?.is_none());
    assert!(store.worker_operation(absent).await?.is_none());
    assert_eq!(
        serde_json::to_value(store.workers_page(None, 100).await?)?,
        json!([first_json, second_json])
    );
    assert_eq!(
        serde_json::to_value(store.workers_page(Some(absent), 1).await?)?,
        json!([first_json])
    );
    assert_eq!(
        serde_json::to_value(store.workers_page(Some(&first.worker_id), 1).await?)?,
        json!([second_json])
    );
    assert!(
        store
            .workers_page(Some(&second.worker_id), 1)
            .await?
            .is_empty()
    );
    assert_eq!(
        store
            .worker(invalid_uuid)
            .await
            .unwrap_err()
            .downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::InvalidRequest)
    );
    assert_eq!(
        store
            .worker_operation(invalid_uuid)
            .await
            .unwrap_err()
            .downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::InvalidRequest)
    );
    for (cursor, limit) in [(None, 0), (None, 101), (Some(invalid_uuid), 1)] {
        assert_eq!(
            store
                .workers_page(cursor, limit)
                .await
                .unwrap_err()
                .downcast_ref::<ControlFailure>(),
            Some(&ControlFailure::InvalidRequest)
        );
    }

    let head = store.verify().await?;
    let audit_rows = super::fixture_prefix(maintenance, head.seq).await?;
    // A fault outside the requested worker and first page must refuse both reads.
    let snapshot_hash: String = maintenance
        .exec_first(
            "SELECT snapshot_hash FROM ai_guard_worker WHERE worker_id=?",
            (&second.worker_id,),
        )
        .await?
        .context("second worker lacks a snapshot hash")?;
    let replacement = different_hash(&snapshot_hash);
    let update = "UPDATE ai_guard_worker SET snapshot_hash=? WHERE worker_id=?";
    maintenance
        .exec_drop(update, (&replacement, &second.worker_id))
        .await?;
    assert_eq!(maintenance.affected_rows(), 1);
    let detail = store.worker(&first.worker_id).await;
    let page = store.workers_page(None, 1).await;
    let operation = store.worker_operation(&first.admission_operation_id).await;
    maintenance
        .exec_drop(update, (&snapshot_hash, &second.worker_id))
        .await?;
    assert_eq!(maintenance.affected_rows(), 1);
    assert_eq!(
        detail.unwrap_err().to_string(),
        "MySQL worker snapshot differs from its audit event"
    );
    assert_eq!(
        page.unwrap_err().to_string(),
        "MySQL worker snapshot differs from its audit event"
    );
    assert_eq!(
        operation.unwrap_err().to_string(),
        "MySQL worker snapshot differs from its audit event"
    );

    super::integrity::replace_hash(maintenance, head.seq, &different_hash(&head.hash)).await?;
    let detail = store.worker(&first.worker_id).await;
    let page = store.workers_page(None, 1).await;
    let operation = store.worker_operation(&first.admission_operation_id).await;
    super::integrity::replace_hash(maintenance, head.seq, &head.hash).await?;
    assert_eq!(
        detail.unwrap_err().to_string(),
        "MySQL audit chain mismatch"
    );
    assert_eq!(page.unwrap_err().to_string(), "MySQL audit chain mismatch");
    assert_eq!(
        operation.unwrap_err().to_string(),
        "MySQL audit chain mismatch"
    );
    assert_eq!(store.verify().await?, head);
    assert_eq!(
        super::fixture_prefix(maintenance, head.seq).await?,
        audit_rows
    );
    Ok(())
}

fn different_hash(hash: &str) -> String {
    if hash.starts_with('0') {
        "1".repeat(64)
    } else {
        "0".repeat(64)
    }
}

async fn mutate(
    store: &MysqlAuditStore,
    worker: &WorkerV1,
    transition: WorkerTransitionV1,
) -> anyhow::Result<MutationResultV1> {
    store.mutate_worker(&mutation(worker, transition)).await
}

fn mutation(worker: &WorkerV1, transition: WorkerTransitionV1) -> WorkerMutationV1 {
    WorkerMutationV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition,
    }
}

fn inspection(worker: &WorkerV1) -> RuntimeInspectionV1 {
    RuntimeInspectionV1 {
        authority_id: worker.launch.authority_id.clone(),
        runtime_kind: worker.launch.runtime_kind,
        container_id: worker
            .attempt
            .container_id
            .clone()
            .unwrap_or_else(|| "c".repeat(64)),
        stable_name: worker.attempt.stable_name.clone(),
        volume_name: worker.attempt.volume_name.clone(),
        image_id: worker.launch.image_id.clone(),
        spec_hash: worker.spec_hash.clone(),
        cpu_millis: worker.launch.cpu_millis,
        memory_bytes: worker.launch.memory_bytes,
        pids_limit: worker.launch.pids_limit,
        network: worker.launch.network,
        security_preset: worker.launch.security_preset,
        restart_policy: worker.launch.restart_policy,
    }
}

fn observation(worker: &WorkerV1, status: RuntimeStatusV1) -> RuntimeObservationV1 {
    let terminal = matches!(status, RuntimeStatusV1::Exited);
    RuntimeObservationV1 {
        inspection: inspection(worker),
        observed_at: if terminal {
            "2026-11-20T10:00:02Z"
        } else {
            "2026-11-20T10:00:01Z"
        }
        .into(),
        status,
        running: Some(!terminal),
        started_at: Some("2026-11-20T10:00:00Z".into()),
        finished_at: terminal.then(|| "2026-11-20T10:00:02Z".into()),
        exit_code: terminal.then_some(0),
        oom_killed: Some(false),
        restart_count: Some(0),
    }
}
