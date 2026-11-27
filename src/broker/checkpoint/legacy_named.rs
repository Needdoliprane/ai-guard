//! Private named admission facts retained in the broker checkpoint.
use super::*;
use crate::control::session_control::{
    LegacyConfirmedPlanV1, LegacyNamedAdmissionReceiptV1, LegacyNamedImportArmRequestV1,
    LegacyNamedSupervisedAdmissionV1,
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
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    archive_digest: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_present",
        skip_serializing_if = "Option::is_none"
    )]
    import_arm: Option<Box<LegacyNamedImportArmCheckpointV1>>,
}

fn deserialize_present<'de, D, T>(decoder: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(decoder).map(Some)
}

/// The retained control request and receipt. Neither proves that any runtime
/// effect is known or gives permission to start the helper.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyNamedImportArmCheckpointV1 {
    version: u32,
    arm: ArmV1,
    receipt: Option<ArmReceiptV1>,
}

impl LegacyNamedImportArmCheckpointV1 {
    pub(crate) fn receipt(&self) -> Option<&ArmReceiptV1> {
        self.receipt.as_ref()
    }
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
            import_arm: None,
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

    pub(crate) fn import_arm(&self) -> Option<&LegacyNamedImportArmCheckpointV1> {
        self.import_arm.as_deref()
    }

    pub(crate) fn import_arm_request(&self) -> Option<LegacyNamedImportArmRequestV1> {
        self.import_arm()
            .map(|record| LegacyNamedImportArmRequestV1 {
                version: 1,
                binding_ref: self.plan.binding_ref,
                namespace: self.plan.control_namespace_uuid.clone(),
                arm: record.arm.clone(),
            })
    }

    pub(crate) fn with_import_arm(mut self, arm: ArmV1) -> JournalResult<Self> {
        ensure(self.import_arm.is_none())?;
        self.import_arm = Some(Box::new(LegacyNamedImportArmCheckpointV1 {
            version: 1,
            arm,
            receipt: None,
        }));
        self.validate_shape(uuid(&self.plan.control_namespace_uuid)?)?;
        Ok(self)
    }

    pub(crate) fn acknowledge_import_arm(mut self, result: ArmResultV1) -> JournalResult<Self> {
        let saved = self.import_arm.as_mut().ok_or(JournalError::Conflict)?;
        ensure(equal(&saved.arm, &result.arm)?)?;
        if let Some(previous) = saved.receipt() {
            ensure(equal(previous, &result.receipt)?)?;
        } else {
            saved.receipt = Some(result.receipt);
        }
        self.validate_shape(uuid(&self.plan.control_namespace_uuid)?)?;
        Ok(self)
    }

    pub(crate) fn requires_strict_cas(&self) -> bool {
        self.pending_v2().is_some() || self.import_arm().is_some_and(|arm| arm.receipt().is_none())
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
            1 => ensure(self.archive_digest.is_none() && self.import_arm.is_none())?,
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
        if let Some(arm) = self.import_arm() {
            self.validate_import_arm(arm)?;
        }
        ensure(canonical(self)?.len() <= MAX_OPERATION_BYTES)
    }

    fn validate_import_arm(&self, saved: &LegacyNamedImportArmCheckpointV1) -> JournalResult<()> {
        if saved.version != 1 {
            return Err(JournalError::UnsupportedVersion);
        }
        let admission = self.receipt().ok_or(JournalError::Conflict)?;
        let parent = admission
            .admission
            .worker
            .as_ref()
            .ok_or(JournalError::Conflict)?;
        let arm = &saved.arm;
        let operation_id = LegacyNamedImportArmRequestV1::operation_id(
            self.plan.binding_ref,
            uuid(&parent.attempt.attempt_id)?,
        );
        ensure(
            self.version == 2
                && arm.operation_id == operation_id
                && arm.operation_id != self.plan.operation_id.to_string()
                && arm.intent_hash == self.request.admission.intent_hash
                && matches!(
                    &arm.subject,
                    ArmSubjectV1::Helper { purpose: HelperPurposeV1::Import, permission, .. }
                    if matches!(permission.as_ref(), HelperPermissionV1::Admission { receipt } if receipt == &admission.admission.receipt)
                ),
        )?;
        let prepare = WorkerMutationV1 {
            version: 1,
            operation_id: arm.operation_id.clone(),
            worker_id: parent.worker_id.clone(),
            attempt_id: parent.attempt.attempt_id.clone(),
            expected_revision: parent.revision,
            transition: WorkerTransitionV1::Prepare,
        };
        let prepared = checked(crate::audit::expected_worker_transition(parent, &prepare))?;
        checked(arm.validate_for_parent(&prepared))?;
        if let Some(receipt) = saved.receipt() {
            checked(receipt.validate_for_arm(arm))?;
            ensure(
                receipt.control_namespace_uuid == self.plan.control_namespace_uuid
                    && receipt.audit_seq > admission.audit_seq,
            )?;
        }
        Ok(())
    }

    pub(super) fn validate_transition(&self, previous: Option<&Self>) -> JournalResult<()> {
        match previous {
            None => ensure(self.receipt.is_none() && self.import_arm.is_none()),
            Some(old) => {
                ensure(
                    self.version == old.version
                        && self.archive_digest == old.archive_digest
                        && self.plan == old.plan
                        && equal(&self.request, &old.request)?,
                )?;
                ensure(old.receipt.is_none() || equal(&self.receipt, &old.receipt)?)?;
                match (old.import_arm(), self.import_arm()) {
                    (None, None) => Ok(()),
                    (None, Some(new)) => ensure(
                        old.admission_result().is_some_and(|result| result.accepted)
                            && new.receipt().is_none(),
                    ),
                    (Some(old), Some(new)) => ensure(
                        equal(&old.arm, &new.arm)?
                            && (old.receipt().is_none() || equal(&old.receipt, &new.receipt)?),
                    ),
                    (Some(_), None) => Err(JournalError::Conflict),
                }
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
    use crate::{
        broker::{
            journal::{ControlSecrets, FsExclusions, Journal, JournalLimitsV1, ReserveResult},
            lifecycle::BrokerCheckpointV3,
            writer::{CancelInputV1, WriterClient},
        },
        control::session_control::{LegacyConfirmedLaunchSpecV1, LegacyNamedInitialDecisionV1},
        control_fs::Directory,
        identity::IdentityReceiptV1,
        runner::{RunOptions, broker::PreparedSubmission},
    };
    use serde_json::json;
    use std::{os::unix::fs::PermissionsExt, time::Duration};

    #[tokio::test]
    async fn explicit_named_admission_requires_an_immutable_archive_digest() -> anyhow::Result<()> {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()?;
        let root = temporary.path().canonicalize()?;
        let config = Directory::open_existing(&root)?;
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
            version: 2,
            plan: plan.clone(),
            launch: launch.clone(),
            source_identity_hash: Some("a".repeat(64)),
        };
        let gateway = "http://127.0.0.1:8787";
        let options: RunOptions = serde_json::from_value(json!({
            "project": root,
            "config_dir": root,
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
        let intent = ExecutionIntentV1::from_legacy_named_launch(options.clone(), &material)?;
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
        let initial_decision = LegacyNamedInitialDecisionV1 {
            version: 1,
            binding_ref,
            request: decision_request.clone(),
            decision: answer.decision().clone(),
            receipt: receipt.clone(),
            replayed: false,
        };
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
        assert_eq!(
            canonical(&pending)?,
            canonical(&json!({
                "version": 2, "plan": plan, "request": request, "receipt": null,
                "archive_digest": digest
            }))?
        );
        let prepared = checkpoint(pending.clone())?;
        prepared.validate_transition(Some(&before))?;
        prepared.validate_context(&intent, &scope, &binding)?;
        let mut historical_material = material.clone();
        historical_material.version = 1;
        historical_material.source_identity_hash = None;
        let unattested =
            ExecutionIntentV1::from_legacy_named_launch(options.clone(), &historical_material)?;
        let mut unattested_request = request.clone();
        unattested_request.admission.intent_hash = unattested.intent_hash()?;
        let unattested_pending = LegacyNamedAdmissionCheckpoint::pending_with_archive(
            plan.clone(),
            unattested_request,
            digest.clone(),
        )?;
        assert!(matches!(
            checkpoint(unattested_pending)?.validate_context(&unattested, &scope, &binding),
            Err(JournalError::Conflict)
        ));
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
            plan.clone(),
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
        let acknowledged = pending.clone().acknowledge(saved.clone())?;
        assert!(acknowledged.pending_v2().is_none());
        assert_eq!(acknowledged.archive_digest(), Some(digest.as_str()));
        let acknowledged = checkpoint(acknowledged)?;
        acknowledged.validate_transition(Some(&reopened))?;
        let reopened = BrokerCheckpointV2::from_bytes(&acknowledged.canonical_bytes()?)?;
        let named = reopened.supervision().legacy_named_admission().unwrap();
        assert!(equal(named.receipt().unwrap(), &saved)?);

        let start_writer = async |name: &str| -> anyhow::Result<WriterClient> {
            let journal_dir = Directory::open_or_create(&root.join(name))?;
            let journal = Journal::open(
                &journal_dir,
                scope.clone(),
                &FsExclusions::new(),
                JournalLimitsV1::default(),
                ControlSecrets::new(vec![])?,
            )?;
            let writer = WriterClient::start(journal, 8, Duration::from_secs(5))?;
            let ReserveResult::New(ticket) =
                writer.reserve_legacy_named(operation, binding_ref).await?
            else {
                panic!("fixture must have a fresh operation");
            };
            let submission = PreparedSubmission::from_legacy_named_launch(
                options.clone(),
                config.clone(),
                &material,
            )?;
            writer
                .accept_prepared(ticket, submission)
                .await
                .map_err(|failure| failure.error())?;
            writer.prepare_legacy_named_initial_decision(&plan).await?;
            writer
                .acknowledge_legacy_named_initial_decision(&plan, &initial_decision)
                .await?;
            Ok(writer)
        };
        for stop_first in [true, false] {
            let writer = start_writer(&format!("journal-{stop_first}")).await?;
            if stop_first {
                let stop = writer
                    .stops()
                    .append_cancel(CancelInputV1::new(operation, Uuid::new_v4())?)
                    .await
                    .map_err(|failure| failure.error())?;
                assert!(stop.named_reservation().is_none());
                let stopped = writer.load(operation).await?.unwrap();
                let next = BrokerCheckpointV3::new(
                    prepared.clone(),
                    stopped.broker_checkpoint_v3().unwrap().commands().to_vec(),
                )?;
                assert!(matches!(
                    writer
                        .advance_lifecycle(operation, stopped.head().clone(), next)
                        .await,
                    Err(JournalError::Conflict)
                ));
                assert!(matches!(
                    writer
                        .record_legacy_named_admission(&plan, digest.clone())
                        .await,
                    Err(JournalError::Interrupted)
                ));
                continue;
            }
            let before = writer.load(operation).await?.unwrap();
            let (one, two) = tokio::join!(
                writer.record_legacy_named_admission(&plan, digest.clone()),
                writer.record_legacy_named_admission(&plan, digest.clone()),
            );
            let recorded = match (one, two) {
                (Ok(recorded), Err(JournalError::Conflict))
                | (Err(JournalError::Conflict), Ok(recorded)) => recorded,
                _ => panic!("exact concurrency must yield one permission to emit"),
            };
            assert_eq!(recorded.into_request().input_hash()?, request.input_hash()?);
            let next = BrokerCheckpointV3::new(prepared.clone(), Vec::new())?;
            assert!(matches!(
                writer
                    .advance_lifecycle(operation, before.head().clone(), next)
                    .await,
                Err(JournalError::Conflict)
            ));
            let stop_id = Uuid::new_v4();
            let stop = writer
                .stops()
                .append_cancel(CancelInputV1::new(operation, stop_id)?)
                .await
                .map_err(|failure| failure.error())?;
            assert!(!stop.incomplete());
            assert!(stop.stops().is_empty());
            let reservation = stop.named_reservation().unwrap();
            assert!(reservation.admission().receipt().is_none());
            assert_eq!(
                reservation.admission().request().input_hash()?,
                request.input_hash()?
            );
            let mut divergent = saved.clone();
            divergent.admission.receipt.input_hash = "0".repeat(64);
            assert!(reservation.cancellation(divergent).is_err());
            assert!(reservation.cancellation(saved.clone())?.is_none());
            writer
                .acknowledge_legacy_named_admission(&plan, &request, saved.clone())
                .await?;
            let stopped = writer
                .stops()
                .cancel_stop_projection(operation, stop_id)
                .await?
                .unwrap();
            assert!(stopped.named_reservation().is_none());
            let retained = writer.load_legacy_named_admission(operation).await?;
            assert_eq!(retained.archive_digest(), Some(digest.as_str()));
            assert!(equal(retained.receipt().unwrap(), &saved)?);
        }

        let worker = Uuid::new_v4();
        let stable_name = format!("aig-{}-{}", namespace.simple(), worker.simple());
        let reserved = WorkerV1 {
            worker_id: worker.to_string(),
            admission_operation_id: operation.to_string(),
            request_id: request
                .admission
                .admission
                .decision_request
                .request_id
                .clone(),
            receipt: request.admission.admission.receipt.clone(),
            launch: request.admission.admission.launch.clone(),
            spec_hash: plan.launch_spec_hash.clone(),
            policy_hash: "3".repeat(64),
            state: WorkerStateV1::Reserved,
            quota_held: true,
            revision: 1,
            attempt: WorkerAttemptV1 {
                attempt_id: Uuid::new_v4().to_string(),
                volume_name: format!("{stable_name}-work"),
                stable_name,
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
        };
        let admission = LegacyNamedAdmissionReceiptV1 {
            version: 1,
            admission: AdmissionResultV1 {
                accepted: true,
                replayed: false,
                refusal: None,
                worker: Some(reserved.clone()),
                receipt: MutationReceiptV1 {
                    worker_id: Some(worker.to_string()),
                    revision: Some(1),
                    ..saved.admission.receipt.clone()
                },
            },
            grant: Some(IdentityReceiptV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                input_hash: "4".repeat(64),
                authorization_revision: 1,
                audit_seq: 3,
                audit_hash: "5".repeat(64),
            }),
            audit_seq: 4,
            audit_hash: "6".repeat(64),
        };
        let helper_id = Uuid::new_v4();
        let helper = helper_execution_policy_v1(&reserved, HelperPurposeV1::Import, helper_id)?;
        let arm = ArmV1 {
            version: 1,
            operation_id: LegacyNamedImportArmRequestV1::operation_id(
                plan.binding_ref,
                uuid(&reserved.attempt.attempt_id)?,
            ),
            worker_id: reserved.worker_id.clone(),
            attempt_id: reserved.attempt.attempt_id.clone(),
            expected_revision: 2,
            intent_hash: request.admission.intent_hash.clone(),
            subject: ArmSubjectV1::Helper {
                helper_id: helper_id.to_string(),
                purpose: HelperPurposeV1::Import,
                policy_version: helper.version,
                permission: Box::new(HelperPermissionV1::Admission {
                    receipt: admission.admission.receipt.clone(),
                }),
            },
            inspection: RuntimeInspectionV1 {
                authority_id: helper.authority_id.clone(),
                runtime_kind: helper.runtime_kind,
                container_id: "7".repeat(64),
                stable_name: helper.stable_name.clone(),
                volume_name: helper.volume_name.clone(),
                image_id: helper.image_id.clone(),
                spec_hash: helper.spec_hash()?,
                cpu_millis: helper.cpu_millis,
                memory_bytes: helper.memory_bytes,
                pids_limit: helper.pids_limit,
                network: helper.network,
                security_preset: helper.security_preset,
                restart_policy: helper.restart_policy,
            },
            authority_binding_hash: "8".repeat(64),
            configuration_fingerprint_version: 2,
            configuration_fingerprint: "9".repeat(64),
            clock: HostClockSampleV1 {
                version: 1,
                method: HostClockMethodV1::DarwinMachContinuousV1,
                boot_id: Uuid::new_v4().to_string(),
                continuous_ns_decimal: "100000000000".into(),
                observed_utc: None,
            },
            timeout_seconds: 120,
            deadline_ns_decimal: "220000000000".into(),
        };
        let arm_result = ArmResultV1 {
            replayed: true,
            arm: arm.clone(),
            receipt: ArmReceiptV1 {
                version: 1,
                arm_id: arm.operation_id.clone(),
                worker_id: arm.worker_id.clone(),
                attempt_id: arm.attempt_id.clone(),
                input_hash: arm.input_hash()?,
                control_namespace_uuid: namespace.to_string(),
                audit_seq: 5,
                audit_hash: "a".repeat(64),
            },
        };
        let accepted = pending.clone().acknowledge(admission.clone())?;
        let named_arm = accepted.with_import_arm(arm.clone())?;
        assert!(matches!(
            checkpoint(named_arm.clone())?.validate_transition(Some(&prepared)),
            Err(JournalError::Conflict)
        ));

        for stop_first in [true, false] {
            let writer = start_writer(&format!("import-{stop_first}")).await?;
            let recorded = writer
                .record_legacy_named_admission(&plan, digest.clone())
                .await?;
            writer
                .acknowledge_legacy_named_admission(
                    &plan,
                    &recorded.into_request(),
                    admission.clone(),
                )
                .await?;
            let admitted = writer.load(operation).await?.unwrap();
            assert!(
                writer
                    .load_legacy_named_import_arm(operation)
                    .await?
                    .is_none()
            );
            if stop_first {
                writer
                    .stops()
                    .append_cancel(CancelInputV1::new(operation, Uuid::new_v4())?)
                    .await
                    .map_err(|failure| failure.error())?;
                let stale = BrokerCheckpointV3::new(checkpoint(named_arm.clone())?, Vec::new())?;
                assert!(matches!(
                    writer
                        .advance_lifecycle(operation, admitted.head().clone(), stale)
                        .await,
                    Err(JournalError::Conflict)
                ));
                let stopped = writer.load(operation).await?.unwrap();
                let next = BrokerCheckpointV3::new(
                    checkpoint(named_arm.clone())?,
                    stopped.broker_checkpoint_v3().unwrap().commands().to_vec(),
                )?;
                assert!(matches!(
                    writer
                        .advance_lifecycle(operation, stopped.head().clone(), next)
                        .await,
                    Err(JournalError::Conflict)
                ));
                assert!(matches!(
                    writer
                        .record_legacy_named_import_arm(&plan, &digest, arm.clone())
                        .await,
                    Err(JournalError::Interrupted)
                ));
                continue;
            }

            assert!(matches!(
                writer
                    .record_legacy_named_import_arm(&plan, &"0".repeat(64), arm.clone())
                    .await,
                Err(JournalError::Conflict)
            ));
            let mut different_attempt = arm.clone();
            different_attempt.attempt_id = Uuid::new_v4().to_string();
            assert!(matches!(
                writer
                    .record_legacy_named_import_arm(&plan, &digest, different_attempt)
                    .await,
                Err(JournalError::Conflict)
            ));
            let mut different_operation = arm.clone();
            different_operation.operation_id =
                LegacyNamedImportArmRequestV1::operation_id(Uuid::new_v4(), uuid(&arm.attempt_id)?);
            assert!(matches!(
                writer
                    .record_legacy_named_import_arm(&plan, &digest, different_operation)
                    .await,
                Err(JournalError::Conflict)
            ));
            assert_eq!(
                writer.load(operation).await?.unwrap().head(),
                admitted.head()
            );
            let (first, second) = tokio::join!(
                writer.record_legacy_named_import_arm(&plan, &digest, arm.clone()),
                writer.record_legacy_named_import_arm(&plan, &digest, arm.clone()),
            );
            let submitted = match (first, second) {
                (Ok(recorded), Err(JournalError::Conflict))
                | (Err(JournalError::Conflict), Ok(recorded)) => recorded.into_request(),
                _ => panic!("only one exact import Arm write may issue a ticket"),
            };
            let next = BrokerCheckpointV3::new(checkpoint(named_arm.clone())?, Vec::new())?;
            assert!(matches!(
                writer
                    .advance_lifecycle(operation, admitted.head().clone(), next)
                    .await,
                Err(JournalError::Conflict)
            ));

            let stop_id = Uuid::new_v4();
            let stop = writer
                .stops()
                .append_cancel(CancelInputV1::new(operation, stop_id)?)
                .await
                .map_err(|failure| failure.error())?;
            assert!(stop.incomplete());
            assert!(stop.named_reservation().is_none());
            assert!(stop.stops().is_empty());
            let snapshot = writer.stops().recovery_status(operation).await?.unwrap();
            assert!(snapshot.incomplete());
            assert_eq!(snapshot.pending_arms(), 1);
            let (retained, receipt) = writer
                .load_legacy_named_import_arm(operation)
                .await?
                .unwrap();
            assert!(equal(&retained, &submitted)?);
            assert!(receipt.is_none());
            let mut wrong = arm_result.clone();
            wrong.receipt.control_namespace_uuid = Uuid::new_v4().to_string();
            assert!(matches!(
                writer
                    .acknowledge_legacy_named_import_arm(operation, &retained, wrong)
                    .await,
                Err(JournalError::Conflict)
            ));
            writer
                .acknowledge_legacy_named_import_arm(operation, &retained, arm_result.clone())
                .await?;
            let replayed_stop = writer
                .stops()
                .cancel_stop_projection(operation, stop_id)
                .await?
                .unwrap();
            assert!(replayed_stop.incomplete());
            assert!(replayed_stop.named_reservation().is_none());
            let snapshot = writer.stops().recovery_status(operation).await?.unwrap();
            assert!(snapshot.incomplete());
            assert_eq!(snapshot.pending_arms(), 0);
            let acknowledged = writer.load(operation).await?.unwrap();
            let (retained, receipt) = writer
                .load_legacy_named_import_arm(operation)
                .await?
                .unwrap();
            assert!(equal(&retained, &submitted)?);
            assert!(equal(&receipt.unwrap(), &arm_result.receipt)?);
            let mut replay = arm_result.clone();
            replay.replayed = false;
            writer
                .acknowledge_legacy_named_import_arm(operation, &retained, replay)
                .await?;
            assert_eq!(
                writer.load(operation).await?.unwrap().head(),
                acknowledged.head()
            );
        }
        Ok(())
    }
}
