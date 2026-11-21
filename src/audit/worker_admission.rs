use super::decision_codec::{decision_execution_mode, decision_provider_resolution};
use crate::{
    control::{
        AdmissionRefusalV1, AdmissionResultV1, AdmissionV1, AllocationV1, DecisionReceiptV1,
        LaunchSpecV1, MutationReceiptV1, QuotaPolicyV1, QuotaUsageV1, RuntimeObservationV1,
        WorkerAttemptV1, WorkerStateV1, WorkerV1, content_hash, invalid,
    },
    model::{Decision, DecisionRequest, Outcome, StoredEvent},
    policy::{self, Policy},
    redaction::Redactor,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ControlRecord {
    pub(crate) version: u32,
    pub(crate) operation_id: String,
    pub(crate) action: String,
    pub(crate) input_hash: String,
    pub(crate) previous_revision: Option<u64>,
    pub(crate) worker: Option<WorkerV1>,
    pub(crate) snapshot_hash: Option<String>,
    pub(crate) observation: Option<RuntimeObservationV1>,
    pub(crate) allocation: Option<AllocationV1>,
    pub(crate) refusal: Option<AdmissionRefusalV1>,
    pub(crate) admission_policy: Option<Value>,
}

pub(crate) fn validate_input(
    admission: &AdmissionV1,
    quota: &QuotaPolicyV1,
    redactor: &Redactor,
) -> Result<String> {
    admission.validate()?;
    quota.validate()?;
    let value = serde_json::to_value(admission)?;
    invalid(redactor.value(&value) == value && serde_json::to_vec(&value)?.len() <= 65536)?;
    admission.input_hash()
}

pub(crate) fn require_receipt(receipt: &DecisionReceiptV1, stored: &StoredEvent) -> Result<()> {
    invalid(
        stored.seq == receipt.audit_seq
            && stored.hash == receipt.audit_hash
            && stored
                .event
                .decision
                .as_ref()
                .is_some_and(|decision| decision.outcome == Outcome::Allow),
    )
}

pub(crate) fn build_record(
    admission: &AdmissionV1,
    policy: &Policy,
    fresh: Decision,
    jev: Value,
    usage: &QuotaUsageV1,
    allocate: impl FnOnce() -> Result<(String, String, String)>,
) -> Result<ControlRecord> {
    let request = &admission.decision_request;
    let input_hash = admission.input_hash()?;
    let quota = &usage.policy;
    let current = policy::evaluate(policy, request);
    let refusal = if current.outcome != Outcome::Allow || fresh.outcome != Outcome::Allow {
        Some(AdmissionRefusalV1::Policy)
    } else if !fits_quota(&admission.launch, usage) {
        Some(AdmissionRefusalV1::Quota)
    } else {
        None
    };
    let policy_hash = content_hash("ai-guard:admission-policy:v1", &(policy, quota))?;
    let worker = if refusal.is_none() {
        let (namespace, worker_id, attempt_id) = allocate()?;
        let physical = format!(
            "aig-{}-{}",
            namespace.replace('-', ""),
            worker_id.replace('-', "")
        );
        Some(WorkerV1 {
            worker_id,
            admission_operation_id: admission.operation_id.clone(),
            request_id: request.request_id.clone(),
            receipt: admission.receipt.clone(),
            launch: admission.launch.clone(),
            spec_hash: admission.launch.spec_hash()?,
            policy_hash: policy_hash.clone(),
            state: WorkerStateV1::Reserved,
            quota_held: true,
            revision: 1,
            attempt: WorkerAttemptV1 {
                attempt_id,
                stable_name: physical.clone(),
                volume_name: format!("{physical}-work"),
                container_id: None,
                created_inspection: None,
                create_operation_id: None,
                start_operation_id: None,
                start_emitter_id: None,
            },
            observed_started_at: None,
            allocation_inconsistent: false,
            last_observation: None,
            unknown_reason: None,
            cleanup_removed: None,
            cleanup_error: None,
        })
    } else {
        None
    };
    Ok(ControlRecord {
        version: 1,
        operation_id: admission.operation_id.clone(),
        action: "admit".into(),
        input_hash,
        previous_revision: None,
        snapshot_hash: worker
            .as_ref()
            .map(|w| content_hash("ai-guard:worker-snapshot:v1", w))
            .transpose()?,
        worker,
        observation: None,
        allocation: None,
        refusal,
        admission_policy: Some(
            json!({"policy_hash":policy_hash,"deterministic":current,"fresh":fresh,"jev":jev,"quota":quota}),
        ),
    })
}

pub(crate) fn fits_quota(launch: &LaunchSpecV1, usage: &QuotaUsageV1) -> bool {
    let quota = &usage.policy;
    usage.held_workers < quota.max_active_workers
        && usage
            .held_cpu_millis
            .checked_add(u64::from(launch.cpu_millis))
            .is_some_and(|cpu| cpu <= u64::from(quota.max_cpu_millis))
        && usage
            .held_memory_bytes
            .checked_add(launch.memory_bytes)
            .is_some_and(|memory| memory <= quota.max_memory_bytes)
        && launch.timeout_seconds <= quota.max_timeout_seconds
}

pub(crate) fn receipt(stored: &StoredEvent, record: &ControlRecord) -> MutationReceiptV1 {
    MutationReceiptV1 {
        operation_id: record.operation_id.clone(),
        worker_id: record.worker.as_ref().map(|w| w.worker_id.clone()),
        revision: record.worker.as_ref().map(|w| w.revision),
        input_hash: record.input_hash.clone(),
        audit_seq: stored.seq,
        audit_hash: stored.hash.clone(),
    }
}
pub(crate) fn admission_result(stored: &StoredEvent, record: &ControlRecord) -> AdmissionResultV1 {
    AdmissionResultV1 {
        replayed: false,
        accepted: record.worker.is_some(),
        refusal: record.refusal,
        worker: record.worker.clone(),
        receipt: receipt(stored, record),
    }
}

pub(crate) fn historical_admission(
    worker: &WorkerV1,
    record: &ControlRecord,
    source: &StoredEvent,
    seq: i64,
) -> Result<AdmissionV1> {
    let event = &source.event;
    let approved = event.data["human_approved"]
        .as_bool()
        .context("admission receipt phase is missing")?;
    anyhow::ensure!(
        source.seq < seq
            && source.hash == worker.receipt.audit_hash
            && event.kind == "security.decision"
            && event.source == "gateway"
            && event.provenance == "observed"
            && event
                .decision
                .as_ref()
                .is_some_and(|decision| decision.outcome == Outcome::Allow)
            && event.data["request_hash"].as_str() == Some(worker.receipt.request_hash.as_str()),
        "admission receipt does not reference an earlier allow"
    );
    let request = DecisionRequest {
        request_id: event.data["request_id"]
            .as_str()
            .context("admission receipt request is missing")?
            .into(),
        profile: event.data["profile"]
            .as_str()
            .context("admission receipt profile is missing")?
            .into(),
        command: event
            .command
            .clone()
            .context("admission command is missing")?,
        project_id: event.project_id.clone(),
        sandbox_id: event.sandbox_id.clone(),
        agent_id: event.agent_id.clone(),
        session_id: event.session_id.clone(),
        correlation_id: event.correlation_id.clone(),
        trace_id: event.trace_id.clone(),
        human_approved: approved,
        approval_hash: approved.then(|| worker.receipt.request_hash.clone()),
        execution_mode: decision_execution_mode(&event.data)?,
        provider_resolution: decision_provider_resolution(&event.data)?,
    };
    let admission = AdmissionV1 {
        version: 1,
        operation_id: record.operation_id.clone(),
        decision_request: request,
        receipt: worker.receipt.clone(),
        launch: worker.launch.clone(),
    };
    admission.validate()?;
    anyhow::ensure!(
        admission.input_hash()? == record.input_hash,
        "admission input differs from its linked decision or journal"
    );
    Ok(admission)
}
