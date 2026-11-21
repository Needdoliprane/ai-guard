use agentsandbox::{
    control::{
        AdmissionRefusalV1, AdmissionV1, ControlFailure, DecisionPhaseV1, DecisionReceiptV1,
        QuotaPolicyV1,
    },
    mysql_storage::audit::MysqlAuditStore,
    policy::{self, Policy},
};
use anyhow::Context;
use serde_json::json;

pub(super) async fn exercise(
    first: &MysqlAuditStore,
    second: &MysqlAuditStore,
) -> anyhow::Result<()> {
    let policy = Policy::default();
    let quota = QuotaPolicyV1 {
        max_active_workers: 1,
        ..Default::default()
    };
    let (left, left_decision) = admission(first, &policy).await?;
    let (right, right_decision) = admission(second, &policy).await?;
    anyhow::ensure!(
        left_decision.seq != right_decision.seq,
        "decisions must be distinct"
    );

    let (left_result, right_result) = tokio::join!(
        first.admit_worker(
            &left,
            &policy,
            policy::evaluate(&policy, &left.decision_request),
            json!(null),
            &quota
        ),
        second.admit_worker(
            &right,
            &policy,
            policy::evaluate(&policy, &right.decision_request),
            json!(null),
            &quota
        ),
    );
    let left_result = left_result?;
    let right_result = right_result?;
    let accepted_count = usize::from(left_result.accepted) + usize::from(right_result.accepted);
    anyhow::ensure!(
        accepted_count == 1,
        "concurrent admissions must reserve exactly one worker"
    );
    let refused = if left_result.accepted {
        &right_result
    } else {
        &left_result
    };
    anyhow::ensure!(
        !refused.accepted && refused.refusal == Some(AdmissionRefusalV1::Quota),
        "quota refusal must be explicit and audited"
    );

    let accepted = if left_result.accepted { &left } else { &right };
    let accepted_result = if left_result.accepted {
        &left_result
    } else {
        &right_result
    };
    let before_replay = first.verify().await?;
    let replay = second
        .replay_admission(accepted, &quota)
        .await?
        .context("accepted receipt must be present without a new policy evaluation")?;
    anyhow::ensure!(
        replay.replayed,
        "exact admission must replay on the other handle"
    );
    anyhow::ensure!(
        serde_json::to_value(&replay.receipt)? == serde_json::to_value(&accepted_result.receipt)?,
        "replay must return the exact stored receipt"
    );
    anyhow::ensure!(
        replay.worker.as_ref().map(|worker| &worker.state)
            == accepted_result.worker.as_ref().map(|worker| &worker.state),
        "replay must preserve the reserved worker state"
    );
    let refused_input = if left_result.accepted { &right } else { &left };
    let replay = first
        .replay_admission(refused_input, &quota)
        .await?
        .context("audited quota refusal must replay")?;
    anyhow::ensure!(
        replay.replayed
            && !replay.accepted
            && replay.refusal == refused.refusal
            && replay.receipt == refused.receipt
            && replay.worker.is_none(),
        "replayed quota refusal differs from its original receipt"
    );
    anyhow::ensure!(
        first.verify().await? == before_replay,
        "replay changed audit head"
    );

    let mut conflict = accepted.clone();
    conflict.launch.image_ref.push_str("-different");
    let error = second
        .admit_worker(
            &conflict,
            &policy,
            policy::evaluate(&policy, &conflict.decision_request),
            json!(null),
            &quota,
        )
        .await
        .unwrap_err();
    anyhow::ensure!(
        error.downcast_ref::<ControlFailure>() == Some(&ControlFailure::Conflict),
        "same operation ID with changed input must conflict"
    );
    anyhow::ensure!(
        first.verify().await? == before_replay,
        "conflict changed audit head"
    );
    let (mut bad_receipt, _) = admission(first, &policy).await?;
    let before_bad_receipt = first.verify().await?;
    bad_receipt.receipt.audit_hash = "f".repeat(64);
    let error = first
        .admit_worker(
            &bad_receipt,
            &policy,
            policy::evaluate(&policy, &bad_receipt.decision_request),
            json!(null),
            &quota,
        )
        .await
        .unwrap_err();
    anyhow::ensure!(
        error.downcast_ref::<ControlFailure>() == Some(&ControlFailure::InvalidRequest),
        "invalid decision receipt must be refused"
    );
    anyhow::ensure!(
        first.verify().await? == before_bad_receipt,
        "invalid decision receipt changed audit head"
    );

    let usage = first.worker_quota(&quota).await?;
    anyhow::ensure!(
        usage.held_workers == 1,
        "one reservation must hold one worker quota"
    );
    anyhow::ensure!(
        usage.held_cpu_millis == u64::from(accepted.launch.cpu_millis)
            && usage.held_memory_bytes == accepted.launch.memory_bytes,
        "quota usage must reflect the reserved worker"
    );
    Ok(())
}

pub(super) async fn admission(
    store: &MysqlAuditStore,
    policy: &Policy,
) -> anyhow::Result<(AdmissionV1, agentsandbox::model::StoredEvent)> {
    let mut admission: AdmissionV1 = serde_json::from_str(include_str!(
        "../fixtures/control/admission-fixed-sandbox-v1.json"
    ))?;
    let session_id = uuid::Uuid::new_v4().to_string();
    let request = {
        let mut request = super::decision::request("strict", &["/usr/bin/id"]);
        request.session_id = Some(session_id.clone());
        request
    };
    let stored = store
        .decision(
            &request,
            Some((
                policy::evaluate(policy, &request),
                json!({"source": "mysql worker admission exercise"}),
            )),
        )
        .await?
        .context("fixture decision was not stored")?;
    admission.version = 1;
    admission.operation_id = uuid::Uuid::new_v4().to_string();
    admission.decision_request = request.clone();
    admission.receipt = DecisionReceiptV1 {
        version: 1,
        request_id: request.request_id.clone(),
        request_hash: policy::request_hash(&request),
        phase: DecisionPhaseV1::Initial,
        audit_seq: stored.seq,
        audit_hash: stored.hash.clone(),
    };
    admission.launch.session_id = session_id;
    admission.launch.command = request.command.clone();
    admission.launch.profile = request.profile.clone();
    admission.launch.runtime_kind = agentsandbox::control::RuntimeKindV1::Docker;
    admission.launch.authority_id = "mysql-worker-fixture".into();
    admission.launch.image_ref = "worker:fixture".into();
    admission.launch.image_id = format!("sha256:{}", "a".repeat(64));
    admission.launch.memory_bytes = 1024 * 1024 * 1024;
    admission.launch.timeout_seconds = 60;
    admission.launch.network = agentsandbox::control::NetworkV1::None;
    admission.launch.workspace_manifest_digest = "b".repeat(64);
    admission.launch.workspace_import_policy_version = 1;
    admission.launch.provider = "generic".into();
    admission.launch.capture_output = false;
    admission.launch.credential_binding_id = None;
    admission.launch.credential_names.clear();
    anyhow::ensure!(
        matches!(stored.event.kind.as_str(), "security.decision"),
        "expected a typed decision event"
    );
    Ok((admission, stored))
}
