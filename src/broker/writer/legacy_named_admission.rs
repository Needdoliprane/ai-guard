//! The first confirmed pending write is the only authority to issue a new
//! named admission. A loaded pending request authorizes reconciliation only.

use super::{
    BrokerCheckpointV2, BrokerCheckpointV3, JournalError, JournalResult, LifecycleWriteFailure,
    WriterClient,
};
use crate::{
    broker::{
        checkpoint::{
            DecisionAnswerV1, HelperPreparationV1, LegacyNamedAdmissionCheckpoint,
            LegacyNamedImportVerificationPlanV1, SupervisionCheckpointV1, SupervisionOperationV1,
        },
        journal::{AcceptedJobV1, RevisionRefV1, canonical},
        lifecycle::LocalCommandRequestV1,
    },
    control::{
        AdmissionV1, ArmReceiptV1, ArmResultV1, ArmV1, MutationReceiptV1, SupervisedAdmissionV1,
        SupervisionProtocolV1,
        session_control::{
            LegacyConfirmedPlanV1, LegacyNamedAdmissionReceiptV1, LegacyNamedImportArmRequestV1,
            LegacyNamedImportClaimRequestV1, LegacyNamedImportObservationPhaseV1,
            LegacyNamedImportObservationRequestV1, LegacyNamedInitialDecisionV1,
            LegacyNamedSupervisedAdmissionV1,
        },
    },
    runtime::{OwnedContainerSnapshot, RuntimeAuthority},
};
use uuid::Uuid;

const ACK_CAS_ATTEMPTS: usize = 16;

pub(in crate::broker) struct RecordedLegacyNamedAdmission(LegacyNamedSupervisedAdmissionV1);

impl RecordedLegacyNamedAdmission {
    pub(in crate::broker) fn into_request(self) -> LegacyNamedSupervisedAdmissionV1 {
        self.0
    }
}

/// The confirmed first local create boundary. A journal read may recover the
/// preparation data but never reconstruct this acknowledgement.
pub(in crate::broker) struct RecordedLegacyNamedImportCreation(AcceptedJobV1);

impl RecordedLegacyNamedImportCreation {
    #[cfg(test)]
    pub(in crate::broker) fn job(&self) -> &AcceptedJobV1 {
        &self.0
    }

    pub(in crate::broker) fn into_job(self) -> AcceptedJobV1 {
        self.0
    }
}

/// Acknowledgement of the first local write grants a single control request.
/// The persisted view never reconstructs this value.
#[allow(
    dead_code,
    reason = "the named import transport is composed separately"
)]
pub(in crate::broker) struct RecordedLegacyNamedImportArm {
    request: LegacyNamedImportArmRequestV1,
    birth: RevisionRefV1,
}

impl RecordedLegacyNamedImportArm {
    pub(in crate::broker) fn request(&self) -> &LegacyNamedImportArmRequestV1 {
        &self.request
    }

    pub(in crate::broker) fn birth(&self) -> &RevisionRefV1 {
        &self.birth
    }
}

/// Persisted input and optional exact Store ACK for one imported-helper phase.
/// Absence of an ACK means the same request must be reconciled, not rebuilt.
pub(in crate::broker) struct RecordedLegacyNamedImportObservation {
    request: LegacyNamedImportObservationRequestV1,
    receipt: Option<MutationReceiptV1>,
}

/// Immutable named ImportVerification intent returned by the Writer. The
/// insertion bit distinguishes a fresh persisted plan from an exact replay;
/// neither outcome grants a runtime or native capability.
pub(in crate::broker) struct RecordedLegacyNamedImportVerificationPlan {
    plan: LegacyNamedImportVerificationPlanV1,
    inserted: bool,
}

impl RecordedLegacyNamedImportVerificationPlan {
    pub(in crate::broker) fn preparation(&self) -> &HelperPreparationV1 {
        self.plan.preparation()
    }

    pub(in crate::broker) fn parent_revision(&self) -> u64 {
        self.plan.parent_revision()
    }

    pub(in crate::broker) fn inserted(&self) -> bool {
        self.inserted
    }
}

impl RecordedLegacyNamedImportObservation {
    pub(in crate::broker) fn request(&self) -> &LegacyNamedImportObservationRequestV1 {
        &self.request
    }

    pub(in crate::broker) fn receipt(&self) -> Option<&MutationReceiptV1> {
        self.receipt.as_ref()
    }
}

#[derive(Debug)]
pub(in crate::broker) enum NamedImportArmWriteFailure {
    BeforePublication(JournalError),
    PublicationUnconfirmed(JournalError),
    ReloadAfterPublication(JournalError),
}

impl NamedImportArmWriteFailure {
    fn journal_error(self) -> JournalError {
        match self {
            Self::BeforePublication(error)
            | Self::PublicationUnconfirmed(error)
            | Self::ReloadAfterPublication(error) => error,
        }
    }
}

impl From<JournalError> for NamedImportArmWriteFailure {
    fn from(error: JournalError) -> Self {
        Self::BeforePublication(error)
    }
}

impl From<LifecycleWriteFailure> for NamedImportArmWriteFailure {
    fn from(failure: LifecycleWriteFailure) -> Self {
        match failure {
            LifecycleWriteFailure::BeforePublication(error) => Self::BeforePublication(error),
            LifecycleWriteFailure::PublicationUnconfirmed(error) => {
                Self::PublicationUnconfirmed(error)
            }
        }
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

    pub(in crate::broker) async fn record_legacy_named_import_creation(
        &self,
        plan: &LegacyConfirmedPlanV1,
        archive_digest: &str,
        helper_id: Uuid,
        binding: &RuntimeAuthority,
    ) -> JournalResult<RecordedLegacyNamedImportCreation> {
        let job = self.load_named_job(plan).await?;
        let current = super::lifecycle(&job)?;
        require_open(&current)?;
        let retained = retained_admission(&job)?;
        if retained.archive_digest() != Some(archive_digest) {
            return Err(JournalError::Conflict);
        }
        let pending = retained.clone().with_import_creation(helper_id, binding)?;
        let expected = pending
            .import_creation()
            .ok_or(JournalError::Corrupt)?
            .clone();
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
        let preparation = confirmed.import_creation().ok_or(JournalError::Conflict)?;
        if saved.head() != receipt.head()
            || confirmed.plan() != plan
            || confirmed.archive_digest() != Some(archive_digest)
            || confirmed.import_arm().is_some()
            || canonical(preparation)? != canonical(&expected)?
        {
            return Err(JournalError::Conflict);
        }
        Ok(RecordedLegacyNamedImportCreation(saved))
    }

    /// Append the first exact Created observation to an existing intent.
    /// An observation already made may be retained after STOP.
    pub(in crate::broker) async fn acknowledge_legacy_named_import_creation(
        &self,
        plan: &LegacyConfirmedPlanV1,
        archive_digest: &str,
        binding: &RuntimeAuthority,
        owned: OwnedContainerSnapshot,
    ) -> JournalResult<()> {
        for _ in 0..ACK_CAS_ATTEMPTS {
            let job = self.load_named_job(plan).await?;
            let retained = retained_admission(&job)?;
            if retained.archive_digest() != Some(archive_digest) {
                return Err(JournalError::Conflict);
            }
            let preparation = retained.import_creation().ok_or(JournalError::Conflict)?;
            let acknowledged = retained.clone().with_import_owned(binding, owned.clone())?;
            if preparation.owned().is_some() {
                return Ok(());
            }
            let current = super::lifecycle(&job)?;
            let next = with_admission(&current, acknowledged)?;
            // STOP can advance the head between this read and the CAS. Rebuild
            // from the latest prefix with the same inspected snapshot.
            match self
                .advance_lifecycle_with_publication(job.operation_id(), job.head().clone(), next)
                .await
            {
                Ok(_) => return Ok(()),
                Err(LifecycleWriteFailure::BeforePublication(JournalError::Conflict)) => continue,
                Err(failure) => return Err(failure.journal_error()),
            }
        }
        Err(JournalError::Conflict)
    }

    pub(in crate::broker) async fn record_legacy_named_import_arm(
        &self,
        plan: &LegacyConfirmedPlanV1,
        archive_digest: &str,
        arm: ArmV1,
    ) -> JournalResult<RecordedLegacyNamedImportArm> {
        self.record_legacy_named_import_arm_diagnosed(plan, archive_digest, arm)
            .await
            .map_err(NamedImportArmWriteFailure::journal_error)
    }

    pub(in crate::broker) async fn record_legacy_named_import_arm_diagnosed(
        &self,
        plan: &LegacyConfirmedPlanV1,
        archive_digest: &str,
        arm: ArmV1,
    ) -> Result<RecordedLegacyNamedImportArm, NamedImportArmWriteFailure> {
        let job = self.load_named_job(plan).await?;
        let current = super::lifecycle(&job)?;
        require_open(&current)?;
        let retained = retained_admission(&job)?;
        if retained.archive_digest() != Some(archive_digest)
            || retained
                .import_creation()
                .and_then(|prep| prep.owned())
                .is_none()
        {
            return Err(NamedImportArmWriteFailure::BeforePublication(
                JournalError::Conflict,
            ));
        }
        let pending = retained.clone().with_import_arm(arm)?;
        let request = pending.import_arm_request().ok_or(JournalError::Conflict)?;
        let receipt = self
            .advance_lifecycle_with_publication(
                job.operation_id(),
                job.head().clone(),
                with_admission(&current, pending)?,
            )
            .await?;
        async {
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
            Ok(RecordedLegacyNamedImportArm {
                request: confirmed_request,
                birth: receipt.head().clone(),
            })
        }
        .await
        .map_err(NamedImportArmWriteFailure::ReloadAfterPublication)
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
        for _ in 0..ACK_CAS_ATTEMPTS {
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
            let acknowledged = retained.clone().acknowledge_import_arm(result.clone())?;
            if retained
                .import_arm()
                .and_then(|arm| arm.receipt())
                .is_some()
            {
                return Ok(());
            }
            let current = super::lifecycle(&job)?;
            let next = with_admission(&current, acknowledged)?;
            // A concurrent STOP advances the prefix. Keep the received Arm
            // and rebuild the acknowledgement against the new head.
            match self
                .advance_lifecycle_with_publication(operation, job.head().clone(), next)
                .await
            {
                Ok(_) => return Ok(()),
                Err(LifecycleWriteFailure::BeforePublication(JournalError::Conflict)) => continue,
                Err(failure) => return Err(failure.journal_error()),
            }
        }
        Err(JournalError::Conflict)
    }

    /// Persist claim data against the acknowledged Arm. The live native
    /// binding and permission to dispatch belong to a separate boundary.
    #[allow(
        dead_code,
        reason = "named claim transport and native binding are composed separately"
    )]
    pub(in crate::broker) async fn record_legacy_named_import_claim(
        &self,
        plan: &LegacyConfirmedPlanV1,
        request: LegacyNamedImportClaimRequestV1,
    ) -> JournalResult<()> {
        let job = self.load_named_job(plan).await?;
        let current = super::lifecycle(&job)?;
        require_open(&current)?;
        let pending = retained_admission(&job)?
            .clone()
            .with_import_claim(request)?;
        let next = with_admission(&current, pending)?;
        self.advance_lifecycle(job.operation_id(), job.head().clone(), next)
            .await?;
        Ok(())
    }

    /// Preserve the exact receipt of a retained request, including after STOP.
    #[allow(dead_code, reason = "named claim transport is composed separately")]
    pub(in crate::broker) async fn acknowledge_legacy_named_import_claim(
        &self,
        operation: Uuid,
        request: &LegacyNamedImportClaimRequestV1,
        receipt: MutationReceiptV1,
    ) -> JournalResult<()> {
        for _ in 0..ACK_CAS_ATTEMPTS {
            let job = self
                .load(operation)
                .await?
                .ok_or(JournalError::InvalidInput)?;
            let retained = retained_admission(&job)?;
            let saved = retained.import_claim().ok_or(JournalError::Conflict)?;
            if canonical(saved.request())? != canonical(request)? {
                return Err(JournalError::Conflict);
            }
            let acknowledged = retained.clone().acknowledge_import_claim(receipt.clone())?;
            if saved.receipt().is_some() {
                return Ok(());
            }
            let current = super::lifecycle(&job)?;
            let next = with_admission(&current, acknowledged)?;
            match self
                .advance_lifecycle_with_publication(operation, job.head().clone(), next)
                .await
            {
                Ok(_) => return Ok(()),
                Err(LifecycleWriteFailure::BeforePublication(JournalError::Conflict)) => continue,
                Err(failure) => return Err(failure.journal_error()),
            }
        }
        Err(JournalError::Conflict)
    }

    /// Persist the one named ImportVerification intent before any helper
    /// creation or Arm. A first insert is refused after STOP; an exact replay
    /// only returns the retained plan and never reconstructs a capability.
    pub(in crate::broker) async fn record_legacy_named_import_verification_plan(
        &self,
        plan: &LegacyConfirmedPlanV1,
        helper_id: Uuid,
    ) -> JournalResult<RecordedLegacyNamedImportVerificationPlan> {
        for _ in 0..ACK_CAS_ATTEMPTS {
            let job = self.load_named_job(plan).await?;
            let current = super::lifecycle(&job)?;
            let retained = retained_admission(&job)?;
            if retained.import_verification_plan().is_some() {
                let replayed = retained.clone().with_import_verification_plan(helper_id)?;
                let plan = replayed
                    .import_verification_plan()
                    .ok_or(JournalError::Corrupt)?
                    .clone();
                return Ok(RecordedLegacyNamedImportVerificationPlan {
                    plan,
                    inserted: false,
                });
            }

            require_open(&current)?;
            let pending = retained.clone().with_import_verification_plan(helper_id)?;
            let expected = pending
                .import_verification_plan()
                .ok_or(JournalError::Corrupt)?
                .clone();
            let next = with_admission(&current, pending)?;
            match self
                .advance_lifecycle_with_publication(plan.operation_id, job.head().clone(), next)
                .await
            {
                Ok(receipt) => {
                    let saved_job = self
                        .load(plan.operation_id)
                        .await?
                        .ok_or(JournalError::Corrupt)?;
                    let saved = retained_admission(&saved_job)?
                        .import_verification_plan()
                        .ok_or(JournalError::Conflict)?;
                    if saved_job.head() != receipt.head()
                        || canonical(saved)? != canonical(&expected)?
                    {
                        return Err(JournalError::Conflict);
                    }
                    return Ok(RecordedLegacyNamedImportVerificationPlan {
                        plan: saved.clone(),
                        inserted: true,
                    });
                }
                Err(LifecycleWriteFailure::BeforePublication(JournalError::Conflict)) => continue,
                Err(failure) => return Err(failure.journal_error()),
            }
        }
        Err(JournalError::Conflict)
    }

    /// Load only the retained intent. This data proves what was planned, not
    /// whether a helper was created, armed, or permitted to start.
    pub(in crate::broker) async fn load_legacy_named_import_verification_plan(
        &self,
        operation: Uuid,
    ) -> JournalResult<Option<LegacyNamedImportVerificationPlanV1>> {
        let job = self
            .load(operation)
            .await?
            .ok_or(JournalError::InvalidInput)?;
        Ok(retained_admission(&job)?
            .import_verification_plan()
            .cloned())
    }

    /// Journal one exact Started or Terminal observation before the caller
    /// submits it to Store. This is evidence only and never authorizes start.
    /// Replaying the same body returns its retained ACK, if already present.
    pub(in crate::broker) async fn record_legacy_named_import_observation(
        &self,
        plan: &LegacyConfirmedPlanV1,
        request: LegacyNamedImportObservationRequestV1,
    ) -> JournalResult<RecordedLegacyNamedImportObservation> {
        request.validate().map_err(|_| JournalError::InvalidInput)?;
        for _ in 0..ACK_CAS_ATTEMPTS {
            let job = self.load_named_job(plan).await?;
            let retained = retained_admission(&job)?;
            if let Some((saved, receipt)) = retained.import_observation(request.phase) {
                if canonical(saved)? != canonical(&request)? {
                    return Err(JournalError::Conflict);
                }
                return Ok(RecordedLegacyNamedImportObservation {
                    request,
                    receipt: receipt.cloned(),
                });
            }
            let updated = retained.clone().with_import_observation(request.clone())?;
            let current = super::lifecycle(&job)?;
            let next = with_admission(&current, updated)?;
            match self
                .advance_lifecycle_with_publication(plan.operation_id, job.head().clone(), next)
                .await
            {
                Ok(_) => {
                    return Ok(RecordedLegacyNamedImportObservation {
                        request,
                        receipt: None,
                    });
                }
                Err(LifecycleWriteFailure::BeforePublication(JournalError::Conflict)) => continue,
                Err(failure) => return Err(failure.journal_error()),
            }
        }
        Err(JournalError::Conflict)
    }

    /// Load only an existing phase identity for restart reconciliation. No
    /// absent request can be synthesized from a checkpoint read.
    pub(in crate::broker) async fn load_legacy_named_import_observation(
        &self,
        operation: Uuid,
        phase: LegacyNamedImportObservationPhaseV1,
    ) -> JournalResult<Option<RecordedLegacyNamedImportObservation>> {
        let job = self
            .load(operation)
            .await?
            .ok_or(JournalError::InvalidInput)?;
        let retained = retained_admission(&job)?;
        Ok(retained
            .import_observation(phase)
            .map(|(request, receipt)| RecordedLegacyNamedImportObservation {
                request: request.clone(),
                receipt: receipt.cloned(),
            }))
    }

    /// Save the exact Store receipt for the previously journaled request.
    /// Reload/retry after CAS contention preserves a concurrent STOP prefix.
    pub(in crate::broker) async fn acknowledge_legacy_named_import_observation(
        &self,
        operation: Uuid,
        request: &LegacyNamedImportObservationRequestV1,
        receipt: MutationReceiptV1,
    ) -> JournalResult<()> {
        request
            .validate_receipt(&receipt)
            .map_err(|_| JournalError::Conflict)?;
        for _ in 0..ACK_CAS_ATTEMPTS {
            let job = self
                .load(operation)
                .await?
                .ok_or(JournalError::InvalidInput)?;
            let retained = retained_admission(&job)?;
            let (saved, previous_receipt) = retained
                .import_observation(request.phase)
                .ok_or(JournalError::Conflict)?;
            if canonical(saved)? != canonical(request)? {
                return Err(JournalError::Conflict);
            }
            let updated = retained
                .clone()
                .acknowledge_import_observation(request, receipt.clone())?;
            if previous_receipt.is_some() {
                return Ok(());
            }
            let current = super::lifecycle(&job)?;
            let next = with_admission(&current, updated)?;
            match self
                .advance_lifecycle_with_publication(operation, job.head().clone(), next)
                .await
            {
                Ok(_) => return Ok(()),
                Err(LifecycleWriteFailure::BeforePublication(JournalError::Conflict)) => continue,
                Err(failure) => return Err(failure.journal_error()),
            }
        }
        Err(JournalError::Conflict)
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
