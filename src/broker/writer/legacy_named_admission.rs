//! The first confirmed pending write is the only authority to issue a new
//! named admission. A loaded pending request authorizes reconciliation only.

use super::{BrokerCheckpointV2, BrokerCheckpointV3, JournalError, JournalResult, WriterClient};
use crate::{
    broker::{
        checkpoint::{
            DecisionAnswerV1, LegacyNamedAdmissionCheckpoint, SupervisionCheckpointV1,
            SupervisionOperationV1,
        },
        journal::{AcceptedJobV1, canonical},
        lifecycle::LocalCommandRequestV1,
    },
    control::{
        AdmissionV1, ArmReceiptV1, ArmResultV1, ArmV1, SupervisedAdmissionV1,
        SupervisionProtocolV1,
        session_control::{
            LegacyConfirmedPlanV1, LegacyNamedAdmissionReceiptV1, LegacyNamedImportArmRequestV1,
            LegacyNamedInitialDecisionV1, LegacyNamedSupervisedAdmissionV1,
        },
    },
};
use uuid::Uuid;

pub(in crate::broker) struct RecordedLegacyNamedAdmission(LegacyNamedSupervisedAdmissionV1);

impl RecordedLegacyNamedAdmission {
    pub(in crate::broker) fn into_request(self) -> LegacyNamedSupervisedAdmissionV1 {
        self.0
    }
}

/// Acknowledgement of the first local write grants a single control request.
/// The persisted view never reconstructs this value.
#[allow(
    dead_code,
    reason = "the named import transport is composed separately"
)]
pub(in crate::broker) struct RecordedLegacyNamedImportArm(LegacyNamedImportArmRequestV1);

impl RecordedLegacyNamedImportArm {
    pub(in crate::broker) fn into_request(self) -> LegacyNamedImportArmRequestV1 {
        self.0
    }
}

impl WriterClient {
    pub(in crate::broker) async fn prepare_legacy_named_initial_decision(
        &self,
        plan: &LegacyConfirmedPlanV1,
    ) -> JournalResult<()> {
        let job = self.load_named_job(plan).await?;
        let current = super::lifecycle(&job)?;
        require_open(&current)?;
        let request = job.intent().request_data();
        let operations = current.broker().supervision().operations();
        match operations {
            [] => {
                let next = with_initial_decision(&current, request.clone(), None)?;
                self.advance_lifecycle(job.operation_id(), job.head().clone(), next)
                    .await?;
            }
            [SupervisionOperationV1::Decision { request: saved, .. }]
                if canonical(saved)? == canonical(request)? => {}
            _ => return Err(JournalError::Conflict),
        }
        Ok(())
    }

    pub(in crate::broker) async fn acknowledge_legacy_named_initial_decision(
        &self,
        plan: &LegacyConfirmedPlanV1,
        decision: &LegacyNamedInitialDecisionV1,
    ) -> JournalResult<()> {
        decision
            .validate()
            .map_err(|_| JournalError::InvalidInput)?;
        let job = self.load_named_job(plan).await?;
        let current = super::lifecycle(&job)?;
        let [SupervisionOperationV1::Decision { request, response }] =
            current.broker().supervision().operations()
        else {
            return Err(JournalError::Conflict);
        };
        if decision.binding_ref != plan.binding_ref
            || canonical(request)? != canonical(&decision.request)?
        {
            return Err(JournalError::Conflict);
        }
        let answer = DecisionAnswerV1::new(decision.decision.clone(), decision.receipt.clone())?;
        if let Some(saved) = response {
            return if canonical(saved)? == canonical(&answer)? {
                Ok(())
            } else {
                Err(JournalError::Conflict)
            };
        }
        let next = with_initial_decision(&current, request.clone(), Some(answer))?;
        self.advance_lifecycle(job.operation_id(), job.head().clone(), next)
            .await?;
        Ok(())
    }

    pub(in crate::broker) async fn record_legacy_named_admission(
        &self,
        plan: &LegacyConfirmedPlanV1,
        archive_digest: String,
    ) -> JournalResult<RecordedLegacyNamedAdmission> {
        let job = self.load_named_job(plan).await?;
        let current = super::lifecycle(&job)?;
        require_open(&current)?;
        if current
            .broker()
            .supervision()
            .legacy_named_admission()
            .is_some()
        {
            return Err(JournalError::Conflict);
        }
        let request = request_from_initial_decision(&job, &current)?;
        let pending = LegacyNamedAdmissionCheckpoint::pending_with_archive(
            plan.clone(),
            request,
            archive_digest,
        )?;
        let next = with_admission(&current, pending.clone())?;
        let receipt = self
            .advance_lifecycle(job.operation_id(), job.head().clone(), next)
            .await?;
        let saved = self
            .load(job.operation_id())
            .await?
            .ok_or(JournalError::Corrupt)?;
        let confirmed = retained_admission(&saved)?
            .pending_v2()
            .ok_or(JournalError::Conflict)?;
        if saved.head() != receipt.head()
            || confirmed.plan != plan
            || Some(confirmed.archive_digest) != pending.archive_digest()
            || canonical(confirmed.request)? != canonical(pending.request())?
        {
            return Err(JournalError::Conflict);
        }
        Ok(RecordedLegacyNamedAdmission(confirmed.request.clone()))
    }

    pub(in crate::broker) async fn load_legacy_named_admission(
        &self,
        operation: Uuid,
    ) -> JournalResult<LegacyNamedAdmissionCheckpoint> {
        let job = self
            .load(operation)
            .await?
            .ok_or(JournalError::InvalidInput)?;
        Ok(retained_admission(&job)?.clone())
    }

    pub(in crate::broker) async fn acknowledge_legacy_named_admission(
        &self,
        plan: &LegacyConfirmedPlanV1,
        request: &LegacyNamedSupervisedAdmissionV1,
        receipt: LegacyNamedAdmissionReceiptV1,
    ) -> JournalResult<()> {
        let job = self.load_named_job(plan).await?;
        let retained = retained_admission(&job)?;
        if retained.plan() != plan || canonical(retained.request())? != canonical(request)? {
            return Err(JournalError::Conflict);
        }
        let acknowledged = retained.clone().acknowledge(receipt)?;
        if retained.receipt().is_none() {
            let current = super::lifecycle(&job)?;
            let next = with_admission(&current, acknowledged)?;
            self.advance_lifecycle(job.operation_id(), job.head().clone(), next)
                .await?;
        }
        Ok(())
    }

    pub(in crate::broker) async fn record_legacy_named_import_arm(
        &self,
        plan: &LegacyConfirmedPlanV1,
        archive_digest: &str,
        arm: ArmV1,
    ) -> JournalResult<RecordedLegacyNamedImportArm> {
        let job = self.load_named_job(plan).await?;
        let current = super::lifecycle(&job)?;
        require_open(&current)?;
        let retained = retained_admission(&job)?;
        if retained.archive_digest() != Some(archive_digest) {
            return Err(JournalError::Conflict);
        }
        let pending = retained.clone().with_import_arm(arm)?;
        let request = pending.import_arm_request().ok_or(JournalError::Conflict)?;
        let receipt = self
            .advance_lifecycle(
                job.operation_id(),
                job.head().clone(),
                with_admission(&current, pending)?,
            )
            .await?;
        let saved = self
            .load(job.operation_id())
            .await?
            .ok_or(JournalError::Corrupt)?;
        let confirmed = retained_admission(&saved)?;
        let confirmed_request = confirmed
            .import_arm_request()
            .ok_or(JournalError::Conflict)?;
        if saved.head() != receipt.head()
            || confirmed.plan() != plan
            || confirmed.archive_digest() != Some(archive_digest)
            || confirmed
                .import_arm()
                .is_none_or(|arm| arm.receipt().is_some())
            || canonical(&confirmed_request)? != canonical(&request)?
        {
            return Err(JournalError::Conflict);
        }
        Ok(RecordedLegacyNamedImportArm(confirmed_request))
    }

    /// A reloaded request is for reconciliation. Only the original CAS
    /// acknowledgement can authorize its first submission.
    pub(in crate::broker) async fn load_legacy_named_import_arm(
        &self,
        operation: Uuid,
    ) -> JournalResult<Option<(LegacyNamedImportArmRequestV1, Option<ArmReceiptV1>)>> {
        let job = self
            .load(operation)
            .await?
            .ok_or(JournalError::InvalidInput)?;
        let retained = retained_admission(&job)?;
        Ok(retained.import_arm_request().map(|request| {
            let receipt = retained.import_arm().and_then(|arm| arm.receipt()).cloned();
            (request, receipt)
        }))
    }

    /// Retain the exact response to a prior request, including after STOP.
    pub(in crate::broker) async fn acknowledge_legacy_named_import_arm(
        &self,
        operation: Uuid,
        request: &LegacyNamedImportArmRequestV1,
        result: ArmResultV1,
    ) -> JournalResult<()> {
        let job = self
            .load(operation)
            .await?
            .ok_or(JournalError::InvalidInput)?;
        let retained = retained_admission(&job)?;
        let exact = retained
            .import_arm_request()
            .ok_or(JournalError::Conflict)?;
        if canonical(&exact)? != canonical(request)? {
            return Err(JournalError::Conflict);
        }
        let acknowledged = retained.clone().acknowledge_import_arm(result)?;
        if retained
            .import_arm()
            .and_then(|arm| arm.receipt())
            .is_none()
        {
            let current = super::lifecycle(&job)?;
            let next = with_admission(&current, acknowledged)?;
            self.advance_lifecycle(operation, job.head().clone(), next)
                .await?;
        }
        Ok(())
    }

    async fn load_named_job(&self, plan: &LegacyConfirmedPlanV1) -> JournalResult<AcceptedJobV1> {
        let job = self
            .load(plan.operation_id)
            .await?
            .ok_or(JournalError::InvalidInput)?;
        if job.intent().legacy_named_plan() != Some(plan) {
            return Err(JournalError::Conflict);
        }
        Ok(job)
    }
}

fn require_open(current: &BrokerCheckpointV3) -> JournalResult<()> {
    if current
        .commands()
        .iter()
        .any(|command| matches!(command.request(), LocalCommandRequestV1::CancelRequested))
    {
        return Err(JournalError::Interrupted);
    }
    Ok(())
}

fn retained_admission(job: &AcceptedJobV1) -> JournalResult<&LegacyNamedAdmissionCheckpoint> {
    let retained = job
        .broker_checkpoint_v2()
        .and_then(|checkpoint| checkpoint.supervision().legacy_named_admission())
        .filter(|named| named.archive_digest().is_some())
        .ok_or(JournalError::Conflict)?;
    if job.intent().legacy_named_plan() != Some(retained.plan()) {
        return Err(JournalError::Conflict);
    }
    Ok(retained)
}

fn request_from_initial_decision(
    job: &AcceptedJobV1,
    current: &BrokerCheckpointV3,
) -> JournalResult<LegacyNamedSupervisedAdmissionV1> {
    let material = job
        .intent()
        .legacy_named_launch()
        .ok_or(JournalError::Conflict)?;
    let [
        SupervisionOperationV1::Decision {
            request,
            response: Some(answer),
        },
    ] = current.broker().supervision().operations()
    else {
        return Err(JournalError::Conflict);
    };
    if answer.decision().outcome != crate::model::Outcome::Allow {
        return Err(JournalError::Conflict);
    }
    Ok(LegacyNamedSupervisedAdmissionV1 {
        version: 1,
        binding_ref: material.plan.binding_ref,
        broker_profile_id: job.scope().profile_id().to_owned(),
        admission: SupervisedAdmissionV1 {
            version: 1,
            protocol: SupervisionProtocolV1::BrokerSupervisionV1,
            intent_hash: job.intent_hash().to_owned(),
            admission: AdmissionV1 {
                version: 1,
                operation_id: job.operation_id().to_string(),
                decision_request: request.clone(),
                receipt: answer.receipt().clone(),
                launch: material.launch.clone(),
            },
        },
    })
}

fn with_admission(
    current: &BrokerCheckpointV3,
    admission: LegacyNamedAdmissionCheckpoint,
) -> JournalResult<BrokerCheckpointV3> {
    let supervision = current
        .broker()
        .supervision()
        .clone()
        .with_legacy_named_admission(admission)?;
    let broker = BrokerCheckpointV2::new(current.broker().runner().clone(), supervision)?;
    BrokerCheckpointV3::new(broker, current.commands().to_vec())
}

fn with_initial_decision(
    current: &BrokerCheckpointV3,
    request: crate::model::DecisionRequest,
    response: Option<DecisionAnswerV1>,
) -> JournalResult<BrokerCheckpointV3> {
    let supervision = SupervisionCheckpointV1::new(
        current.broker().supervision().control_namespace_uuid(),
        Vec::new(),
        vec![SupervisionOperationV1::Decision { request, response }],
    )?;
    let broker = BrokerCheckpointV2::new(current.broker().runner().clone(), supervision)?;
    BrokerCheckpointV3::new(broker, current.commands().to_vec())
}
