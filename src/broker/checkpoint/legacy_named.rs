//! Private named admission facts retained in the broker checkpoint.
use super::*;
use crate::control::session_control::{
    LegacyConfirmedPlanV1, LegacyNamedAdmissionReceiptV1, LegacyNamedSupervisedAdmissionV1,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyNamedAdmissionCheckpoint {
    version: u32,
    plan: LegacyConfirmedPlanV1,
    request: LegacyNamedSupervisedAdmissionV1,
    receipt: Option<LegacyNamedAdmissionReceiptV1>,
    #[serde(
        default,
        deserialize_with = "deserialize_archive_digest",
        skip_serializing_if = "Option::is_none"
    )]
    archive_digest: Option<String>,
}

fn deserialize_archive_digest<'de, D>(decoder: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    String::deserialize(decoder).map(Some)
}

/// The exact request and archive pinned by a V2 pending checkpoint.
pub(crate) struct LegacyNamedAdmissionPendingV2<'a> {
    pub(crate) plan: &'a LegacyConfirmedPlanV1,
    pub(crate) request: &'a LegacyNamedSupervisedAdmissionV1,
    pub(crate) archive_digest: &'a str,
}

impl LegacyNamedAdmissionCheckpoint {
    #[cfg(test)]
    pub(crate) fn new(
        plan: LegacyConfirmedPlanV1,
        request: LegacyNamedSupervisedAdmissionV1,
    ) -> JournalResult<Self> {
        Self::pending(1, plan, request, None)
    }

    #[allow(
        dead_code,
        reason = "the private named transport will persist the validated archive digest"
    )]
    pub(crate) fn pending_with_archive(
        plan: LegacyConfirmedPlanV1,
        request: LegacyNamedSupervisedAdmissionV1,
        archive_digest: String,
    ) -> JournalResult<Self> {
        Self::pending(2, plan, request, Some(archive_digest))
    }

    fn pending(
        version: u32,
        plan: LegacyConfirmedPlanV1,
        request: LegacyNamedSupervisedAdmissionV1,
        archive_digest: Option<String>,
    ) -> JournalResult<Self> {
        let value = Self {
            version,
            plan,
            request,
            receipt: None,
            archive_digest,
        };
        let namespace = uuid(&value.plan.control_namespace_uuid)?;
        value.validate_shape(namespace)?;
        Ok(value)
    }

    /// Retain the authenticated Store reply at the existing pending operation.
    /// Reconciliation can return the same receipt with different delivery metadata.
    #[allow(
        dead_code,
        reason = "the private named transport will supply the Store acknowledgement"
    )]
    pub(crate) fn acknowledge(
        mut self,
        mut receipt: LegacyNamedAdmissionReceiptV1,
    ) -> JournalResult<Self> {
        if let Some(previous) = &self.receipt {
            receipt.admission.replayed = previous.admission.replayed;
            ensure(equal(previous, &receipt)?)?;
        } else {
            self.receipt = Some(receipt);
        }
        let namespace = uuid(&self.plan.control_namespace_uuid)?;
        self.validate_shape(namespace)?;
        Ok(self)
    }

    pub(crate) fn plan(&self) -> &LegacyConfirmedPlanV1 {
        &self.plan
    }

    pub(crate) fn request(&self) -> &LegacyNamedSupervisedAdmissionV1 {
        &self.request
    }

    pub(crate) fn admission_result(&self) -> Option<&AdmissionResultV1> {
        self.receipt().map(|receipt| &receipt.admission)
    }

    pub(crate) fn receipt(&self) -> Option<&LegacyNamedAdmissionReceiptV1> {
        self.receipt.as_ref()
    }

    pub(crate) fn archive_digest(&self) -> Option<&str> {
        self.archive_digest.as_deref()
    }

    #[allow(
        dead_code,
        reason = "the broker orchestrator will dispatch the exact committed pending"
    )]
    pub(crate) fn pending_v2(&self) -> Option<LegacyNamedAdmissionPendingV2<'_>> {
        if self.version != 2 || self.receipt.is_some() {
            return None;
        }
        let namespace = uuid(&self.plan.control_namespace_uuid).ok()?;
        self.validate_shape(namespace).ok()?;
        Some(LegacyNamedAdmissionPendingV2 {
            plan: &self.plan,
            request: &self.request,
            archive_digest: self.archive_digest()?,
        })
    }

    pub(super) fn validate_shape(&self, namespace: Uuid) -> JournalResult<()> {
        match self.version {
            1 => ensure(self.archive_digest.is_none())?,
            2 => hash(self.archive_digest().ok_or(JournalError::Conflict)?)?,
            _ => return Err(JournalError::UnsupportedVersion),
        }
        let plan = &self.plan;
        ensure(
            uuid(&plan.control_namespace_uuid)? == namespace
                && !plan.launch_target_id.is_nil()
                && plan.target_revision > 0
                && plan.expires_at.timestamp() > 0
                && self.request.admission.admission.receipt.phase == DecisionPhaseV1::Initial,
        )?;
        checked(self.request.require_plan(plan))?;
        if let Some(saved) = &self.receipt {
            validate_legacy_named_admission_receipt(namespace, &self.request, saved)?;
        }
        ensure(canonical(self)?.len() <= MAX_OPERATION_BYTES)
    }

    pub(super) fn validate_transition(&self, previous: Option<&Self>) -> JournalResult<()> {
        match previous {
            None => ensure(self.receipt.is_none()),
            Some(old) => {
                ensure(
                    self.version == old.version
                        && self.archive_digest == old.archive_digest
                        && self.plan == old.plan
                        && equal(&self.request, &old.request)?,
                )?;
                ensure(old.receipt.is_none() || equal(&self.receipt, &old.receipt)?)
            }
        }
    }
}

/// The broker uses the same linked receipt checks at the HTTP boundary and
/// when reading an acknowledged checkpoint.
pub(crate) fn validate_legacy_named_admission_receipt(
    namespace: Uuid,
    request: &LegacyNamedSupervisedAdmissionV1,
    saved: &LegacyNamedAdmissionReceiptV1,
) -> JournalResult<()> {
    ensure(saved.version == 1)?;
    let result = &saved.admission;
    ensure(result.accepted == result.refusal.is_none())?;
    validation::bound_receipt(
        &result.receipt,
        &request.admission.admission.operation_id,
        &checked(request.admission.input_hash())?,
        result
            .worker
            .as_ref()
            .map(|worker| worker.worker_id.as_str()),
        result.worker.as_ref().map(|worker| worker.revision),
    )?;
    hash(&saved.audit_hash)?;
    match (result.accepted, &result.worker, &saved.grant) {
        (true, Some(worker), Some(grant)) => {
            validation::bound_worker(worker, &request.admission, namespace)?;
            ensure(
                worker.state == WorkerStateV1::Reserved
                    && worker.revision == 1
                    && worker.quota_held
                    && worker.attempt.container_id.is_none()
                    && worker.attempt.created_inspection.is_none()
                    && worker.attempt.create_operation_id.is_none()
                    && worker.attempt.start_emitter_id.is_none()
                    && worker.attempt.start_operation_id.is_none(),
            )?;
            uuid(&grant.operation_id)?;
            hash(&grant.input_hash)?;
            hash(&grant.audit_hash)?;
            ensure(
                grant.version == 1
                    && (1..=i64::MAX as u64).contains(&grant.authorization_revision)
                    && result.receipt.audit_seq.checked_add(1) == Some(grant.audit_seq)
                    && grant.audit_seq.checked_add(1) == Some(saved.audit_seq),
            )
        }
        (false, None, None) => {
            ensure(result.receipt.audit_seq.checked_add(1) == Some(saved.audit_seq))
        }
        _ => Err(JournalError::Conflict),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::session_control::LegacyConfirmedLaunchSpecV1;
    use serde_json::json;

    #[test]
    fn explicit_named_admission_requires_an_immutable_archive_digest() -> anyhow::Result<()> {
        let operation = Uuid::new_v4();
        let binding_ref = Uuid::new_v4();
        let session = Uuid::new_v4();
        let namespace = Uuid::new_v4();
        let launch: LaunchSpecV1 = serde_json::from_value(json!({
            "version": 1,
            "session_id": session,
            "command": {"argv": ["/usr/bin/id", "-u"], "cwd": "/workspace"},
            "profile": "strict",
            "runtime_kind": "docker",
            "authority_id": Uuid::new_v4(),
            "image_ref": "worker:fixture",
            "image_id": format!("sha256:{}", "b".repeat(64)),
            "network": "none",
            "cpu_millis": 2000,
            "memory_bytes": 1_073_741_824_u64,
            "pids_limit": 256,
            "timeout_seconds": 30,
            "security_preset": "fixed_sandbox_v2",
            "restart_policy": "no",
            "workspace_manifest_digest": "c".repeat(64),
            "workspace_import_policy_version": 1,
            "provider": "generic",
            "capture_output": false,
            "credential_binding_id": null,
            "credential_names": [],
        }))?;
        let plan = LegacyConfirmedPlanV1 {
            version: 1,
            binding_ref,
            control_namespace_uuid: namespace.to_string(),
            operation_id: operation,
            session_id: session,
            launch_target_id: Uuid::new_v4(),
            target_revision: 1,
            broker_profile_id: "fixture-profile".into(),
            launch_spec_hash: launch.spec_hash()?,
            expires_at: chrono::DateTime::from_timestamp(4_102_444_800, 0).unwrap(),
        };
        let material = LegacyConfirmedLaunchSpecV1 {
            version: 1,
            plan: plan.clone(),
            launch: launch.clone(),
        };
        let gateway = "http://127.0.0.1:8787";
        let options = serde_json::from_value(json!({
            "project": "/fixture/project",
            "config_dir": "/fixture/config",
            "gateway": gateway,
            "runtime": "docker",
            "image": launch.image_ref,
            "profile": launch.profile,
            "network": "none",
            "allow_rootful_runtime": false,
            "timeout": launch.timeout_seconds,
            "provider": launch.provider,
            "capture_output": false,
            "argv": launch.command.argv,
        }))?;
        let intent = ExecutionIntentV1::from_legacy_named_launch(options, &material)?;
        let scope = JournalScopeV1::new(
            Uuid::new_v4(),
            "fixture",
            gateway,
            namespace,
            &plan.broker_profile_id,
        )?;
        let binding: BindingManifestV1 = serde_json::from_value(json!({"kind": "none"}))?;
        let runner = CheckpointPayloadV1::initial(&intent)?;
        let decision_request = intent.request_data().clone();
        let receipt = DecisionReceiptV1 {
            version: 1,
            request_id: decision_request.request_id.clone(),
            request_hash: crate::policy::request_hash(&decision_request),
            phase: DecisionPhaseV1::Initial,
            audit_seq: 1,
            audit_hash: "d".repeat(64),
        };
        let answer = DecisionAnswerV1::new(
            Decision::new(Outcome::Allow, "fixture", "Exact policy decision."),
            receipt.clone(),
        )?;
        let request = LegacyNamedSupervisedAdmissionV1 {
            version: 1,
            binding_ref,
            broker_profile_id: plan.broker_profile_id.clone(),
            admission: SupervisedAdmissionV1 {
                version: 1,
                protocol: SupervisionProtocolV1::BrokerSupervisionV1,
                intent_hash: intent.intent_hash()?,
                admission: AdmissionV1 {
                    version: 1,
                    operation_id: operation.to_string(),
                    decision_request: decision_request.clone(),
                    receipt,
                    launch,
                },
            },
        };
        let decided = SupervisionCheckpointV1::new(
            namespace,
            vec![],
            vec![SupervisionOperationV1::Decision {
                request: decision_request,
                response: Some(answer),
            }],
        )?;
        let before = BrokerCheckpointV2::new(runner.clone(), decided.clone())?;
        before.validate_context(&intent, &scope, &binding)?;
        let checkpoint = |named| {
            BrokerCheckpointV2::new(
                runner.clone(),
                decided.clone().with_legacy_named_admission(named)?,
            )
        };

        let historical = canonical(&json!({
            "version": 1, "plan": plan, "request": request, "receipt": null
        }))?;
        let legacy: LegacyNamedAdmissionCheckpoint = serde_json::from_slice(&historical)?;
        assert_eq!(canonical(&legacy)?, historical);
        assert!(legacy.pending_v2().is_none());
        let mut unexpected: serde_json::Value = serde_json::from_slice(&historical)?;
        unexpected["archive_digest"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<LegacyNamedAdmissionCheckpoint>(unexpected).is_err());
        assert!(matches!(
            checkpoint(legacy)?.validate_context(&intent, &scope, &binding),
            Err(JournalError::Conflict)
        ));

        let digest = "e".repeat(64);
        let pending = LegacyNamedAdmissionCheckpoint::pending_with_archive(
            plan.clone(),
            request.clone(),
            digest.clone(),
        )?;
        let prepared = checkpoint(pending.clone())?;
        prepared.validate_transition(Some(&before))?;
        prepared.validate_context(&intent, &scope, &binding)?;
        let mut archive_free = serde_json::to_value(&prepared)?;
        archive_free["supervision"]["legacy_named_admission"]
            .as_object_mut()
            .unwrap()
            .remove("archive_digest");
        assert!(BrokerCheckpointV2::from_bytes(&serde_json::to_vec(&archive_free)?).is_err());
        let reopened = BrokerCheckpointV2::from_bytes(&prepared.canonical_bytes()?)?;
        let observed = reopened
            .supervision()
            .legacy_named_admission()
            .unwrap()
            .pending_v2()
            .unwrap();
        assert_eq!(observed.plan, &plan);
        assert_eq!(observed.request.input_hash()?, request.input_hash()?);
        assert_eq!(observed.archive_digest, digest);
        let swapped = checkpoint(LegacyNamedAdmissionCheckpoint::pending_with_archive(
            plan,
            request.clone(),
            "f".repeat(64),
        )?)?;
        assert!(matches!(
            swapped.validate_transition(Some(&reopened)),
            Err(JournalError::Conflict)
        ));

        let saved = LegacyNamedAdmissionReceiptV1 {
            version: 1,
            admission: AdmissionResultV1 {
                accepted: false,
                replayed: false,
                refusal: Some(AdmissionRefusalV1::Policy),
                worker: None,
                receipt: MutationReceiptV1 {
                    operation_id: operation.to_string(),
                    worker_id: None,
                    revision: None,
                    input_hash: request.admission.input_hash()?,
                    audit_seq: 2,
                    audit_hash: "1".repeat(64),
                },
            },
            grant: None,
            audit_seq: 3,
            audit_hash: "2".repeat(64),
        };
        let mut divergent = saved.clone();
        divergent.admission.receipt.input_hash = "0".repeat(64);
        assert!(pending.clone().acknowledge(divergent).is_err());
        let acknowledged = pending.acknowledge(saved.clone())?;
        assert!(acknowledged.pending_v2().is_none());
        assert_eq!(acknowledged.archive_digest(), Some(digest.as_str()));
        let acknowledged = checkpoint(acknowledged)?;
        acknowledged.validate_transition(Some(&reopened))?;
        let reopened = BrokerCheckpointV2::from_bytes(&acknowledged.canonical_bytes()?)?;
        let named = reopened.supervision().legacy_named_admission().unwrap();
        assert!(equal(named.receipt().unwrap(), &saved)?);
        Ok(())
    }
}
