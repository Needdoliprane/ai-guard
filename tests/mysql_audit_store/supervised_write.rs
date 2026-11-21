use agentsandbox::{
    control::{
        AdmissionRefusalV1, AdmissionResultV1, ControlFailure, QuotaPolicyV1, SecurityPresetV1,
        SupervisedAdmissionV1, SupervisionProtocolV1, WorkerStateV1, content_hash,
    },
    mysql_storage::{
        MysqlProbeError,
        audit::{AuditHead, MysqlAuditStore},
    },
    policy::{self, Policy},
};
use anyhow::{Context, Result, ensure};
use mysql_async::{Conn, prelude::Queryable};
use serde_json::Value;
use std::time::Duration;
use uuid::Uuid;

/// The caller must supply independent handles to the same disposable, prepared schema.
pub(super) async fn exercise(
    bootstrapper: &MysqlAuditStore,
    reopened: &MysqlAuditStore,
    mut maintenance: Conn,
) -> Result<()> {
    bootstrapper.initialize_supervised().await?;
    let policy = Policy::default();
    let quota = QuotaPolicyV1 {
        max_active_workers: 1,
        ..Default::default()
    };
    let request = supervised_request(bootstrapper, &policy).await?;
    let historical_head = reopened.verify().await?;
    let historical_rows = super::fixture_prefix(&mut maintenance, historical_head.seq).await?;
    ensure!(
        historical_rows.len() == 3,
        "expected two genesis rows and the decision"
    );

    let reserved = admit(bootstrapper, &request, &policy, &quota).await?;
    let worker = reserved
        .worker
        .as_ref()
        .context("reservation omitted the worker")?;
    ensure!(
        reserved.accepted
            && reserved.refusal.is_none()
            && worker.state == WorkerStateV1::Reserved
            && worker.quota_held
            && worker.revision == 1
            && reserved.receipt.worker_id.as_deref() == Some(worker.worker_id.as_str())
            && reserved.receipt.revision == Some(worker.revision),
        "accepted admission must reserve the worker and return its receipt"
    );
    let reservation_head = append_receipt(reopened, &historical_head, &request, &reserved).await?;
    let expected_quota = (
        1,
        u64::from(request.admission.launch.cpu_millis),
        request.admission.launch.memory_bytes,
    );
    ensure!(held_quota(reopened, &quota).await? == expected_quota);

    let replay = reopened
        .replay_supervised_admission(&request, &quota)
        .await?
        .context("reservation is missing after reopening")?;
    ensure!(
        replay.replayed
            && replay.accepted
            && replay.refusal.is_none()
            && replay.receipt == reserved.receipt
            && serde_json::to_value(&replay.worker)? == serde_json::to_value(worker)?,
        "reopening changed the reserved worker or its receipt"
    );
    assert_eq!(
        reopened
            .worker_operation(&request.admission.operation_id)
            .await?,
        Some(reserved.receipt.clone())
    );
    ensure!(
        reopened.verify().await? == reservation_head,
        "replay appended to the journal"
    );

    let mut divergent = request.clone();
    divergent.intent_hash = content_hash("qa:divergent-supervised-intent", &request.intent_hash)?;
    let error = reopened
        .replay_supervised_admission(&divergent, &quota)
        .await
        .err()
        .context("a changed intent reused the original reservation operation")?;
    ensure!(
        error.downcast_ref::<ControlFailure>() == Some(&ControlFailure::Conflict),
        "divergent replay returned an unexpected error: {error}"
    );
    ensure!(
        reopened.verify().await? == reservation_head,
        "replay conflict appended to the journal"
    );

    let next_request = supervised_request(bootstrapper, &policy).await?;
    let before_refusals = reopened.verify().await?;
    let inactive = admit(reopened, &next_request, &policy, &quota)
        .await
        .err()
        .context("a reopened handle authorized a fresh supervised admission")?;
    ensure!(
        inactive.downcast_ref::<ControlFailure>() == Some(&ControlFailure::SupervisionInactive),
        "fresh admission after reopening returned an unexpected error: {inactive}"
    );
    ensure!(
        reopened.verify().await? == before_refusals
            && reopened
                .replay_supervised_admission(&next_request, &quota)
                .await?
                .is_none(),
        "inactive handle persisted a fresh operation"
    );

    let exhausted = admit(bootstrapper, &next_request, &policy, &quota).await?;
    ensure!(
        !exhausted.accepted
            && exhausted.refusal == Some(AdmissionRefusalV1::Quota)
            && exhausted.worker.is_none()
            && exhausted.receipt.worker_id.is_none()
            && exhausted.receipt.revision.is_none(),
        "exhausted quota must produce an audited refusal without a second reservation"
    );
    let refusal_head =
        append_receipt(reopened, &before_refusals, &next_request, &exhausted).await?;
    let replay = reopened
        .replay_supervised_admission(&next_request, &quota)
        .await?
        .context("audited quota refusal is missing after reopening")?;
    ensure!(
        replay.replayed
            && !replay.accepted
            && replay.refusal == exhausted.refusal
            && replay.worker.is_none()
            && replay.receipt == exhausted.receipt,
        "reopening changed the audited quota refusal"
    );
    assert_eq!(
        reopened
            .worker_operation(&next_request.admission.operation_id)
            .await?,
        Some(exhausted.receipt)
    );
    ensure!(
        reopened.verify().await? == refusal_head
            && held_quota(reopened, &quota).await? == expected_quota
    );
    ensure!(
        super::fixture_prefix(&mut maintenance, historical_head.seq).await? == historical_rows,
        "supervised admissions changed the existing audit history"
    );

    let fault_quota = QuotaPolicyV1 {
        max_active_workers: 2,
        ..quota
    };
    let fault_request = supervised_request(bootstrapper, &policy).await?;
    let before_fault = reopened.verify().await?;
    let rows_before_fault = super::fixture_prefix(&mut maintenance, before_fault.seq).await?;
    // Preserve the existing binding and reject the next one after worker and attempt insertion.
    let fault_setup = format!(
        "ALTER TABLE ai_guard_worker_supervision \
         ADD CONSTRAINT ai_guard_qa_block_supervised_write CHECK (event_seq <= {})",
        reserved.receipt.audit_seq
    );
    tokio::time::timeout(Duration::from_secs(8), maintenance.query_drop(fault_setup))
        .await?
        .map_err(|_| anyhow::anyhow!("fixture supervision fault setup failed"))?;
    let failed = admit(bootstrapper, &fault_request, &policy, &fault_quota).await;
    tokio::time::timeout(
        Duration::from_secs(8),
        maintenance.query_drop(
            "ALTER TABLE ai_guard_worker_supervision DROP CHECK ai_guard_qa_block_supervised_write",
        ),
    )
    .await?
    .map_err(|_| anyhow::anyhow!("fixture supervision fault cleanup failed"))?;
    let error = failed
        .err()
        .context("the supervision binding constraint did not abort admission")?;
    ensure!(
        error.downcast_ref::<MysqlProbeError>() == Some(&MysqlProbeError::ServerRefused(3819)),
        "unexpected error when persisting the supervision binding: {error}"
    );
    ensure!(
        reopened.verify().await? == before_fault
            && super::fixture_prefix(&mut maintenance, before_fault.seq).await?
                == rows_before_fault
            && reopened
                .replay_supervised_admission(&fault_request, &fault_quota)
                .await?
                .is_none()
            && held_quota(reopened, &fault_quota).await? == expected_quota,
        "a failed supervision binding changed the audit, receipts or held quota"
    );
    let retried = admit(bootstrapper, &fault_request, &policy, &fault_quota).await?;
    ensure!(retried.accepted && retried.refusal.is_none());
    append_receipt(reopened, &before_fault, &fault_request, &retried).await?;
    ensure!(
        held_quota(reopened, &fault_quota).await?
            == (2, expected_quota.1 * 2, expected_quota.2 * 2),
        "retry after a confirmed rollback did not reserve exactly one worker"
    );
    Ok(())
}

async fn supervised_request(
    store: &MysqlAuditStore,
    policy: &Policy,
) -> Result<SupervisedAdmissionV1> {
    let (mut admission, _) = super::worker::admission(store, policy).await?;
    admission.launch.authority_id = Uuid::new_v4().to_string();
    admission.launch.security_preset = SecurityPresetV1::FixedSandboxV2;
    Ok(SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: content_hash("qa:supervised-write-intent", &admission.operation_id)?,
        admission,
    })
}

async fn admit(
    store: &MysqlAuditStore,
    request: &SupervisedAdmissionV1,
    policy: &Policy,
    quota: &QuotaPolicyV1,
) -> Result<AdmissionResultV1> {
    store
        .admit_supervised(
            request,
            policy,
            policy::evaluate(policy, &request.admission.decision_request),
            Value::Null,
            quota,
        )
        .await
}

async fn append_receipt(
    store: &MysqlAuditStore,
    before: &AuditHead,
    request: &SupervisedAdmissionV1,
    result: &AdmissionResultV1,
) -> Result<AuditHead> {
    let head = store.verify().await?;
    ensure!(
        !result.replayed
            && head.seq == before.seq + 1
            && result.receipt.audit_seq == head.seq
            && result.receipt.audit_hash == head.hash
            && result.receipt.operation_id == request.admission.operation_id
            && result.receipt.input_hash == request.input_hash()?,
        "new admission receipt must identify the appended audit event and its input"
    );
    Ok(head)
}

async fn held_quota(store: &MysqlAuditStore, quota: &QuotaPolicyV1) -> Result<(u32, u64, u64)> {
    let usage = store.worker_quota(quota).await?;
    Ok((
        usage.held_workers,
        usage.held_cpu_millis,
        usage.held_memory_bytes,
    ))
}
