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
        enforcer::CancelGeneration,
        journal::{AcceptedJobV1, RevisionRefV1, canonical},
        lifecycle::LocalCommandRequestV1,
    },
    control::{
        AdmissionV1, ArmReceiptV1, ArmResultV1, ArmV1, MutationReceiptV1, SupervisedAdmissionV1,
        SupervisionProtocolV1,
        session_control::{
            LegacyConfirmedPlanV1, LegacyNamedAdmissionReceiptV1, LegacyNamedImportArmRequestV1,
            LegacyNamedImportClaimRequestV1, LegacyNamedImportObservationPhaseV1,
            LegacyNamedImportObservationRequestV1, LegacyNamedImportVerificationArmRequestV2,
            LegacyNamedInitialDecisionV1, LegacyNamedSupervisedAdmissionV1,
        },
    },
    runtime::{NamedImportVerificationCreated, OwnedContainerSnapshot, RuntimeAuthority},
};
use std::task::Poll;
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

#[allow(
    dead_code,
    reason = "the verification Arm caller is composed separately"
)]
pub(in crate::broker) enum LegacyNamedImportVerificationArmWrite {
    Recorded(Box<RecordedLegacyNamedImportVerificationArm>),
    Replayed(Box<LegacyNamedImportVerificationArmRequestV2>),
}

/// The confirmed birth of a verification Arm at one exact journal head.
/// The job retains the request and its Created prefix. Only the live caller
/// that confirms the first write receives this one-shot acknowledgement.
#[allow(
    dead_code,
    reason = "the verification Arm caller is composed separately"
)]
pub(in crate::broker) struct RecordedLegacyNamedImportVerificationArm {
    job: AcceptedJobV1,
    generation: CancelGeneration,
    expected_generation: u64,
}

#[allow(
    dead_code,
    reason = "the verification Arm caller is composed separately"
)]
impl RecordedLegacyNamedImportVerificationArm {
    pub(in crate::broker) fn job(&self) -> &AcceptedJobV1 {
        &self.job
    }

    pub(in crate::broker) fn generation(&self) -> (&CancelGeneration, u64) {
        (&self.generation, self.expected_generation)
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

/// One-shot authority for the caller's immediate local Create effect. It is
/// deliberately non-Clone and cannot be reconstructed from retained journal
/// state; the caller must keep its generation local to the effect path.
pub(crate) struct AuthorizedLegacyNamedImportCreate {
    operation_id: Uuid,
    helper_id: Uuid,
    policy_hash: String,
    readonly_spec_hash: String,
    import_policy: crate::control::HelperExecutionPolicyV1,
    import_snapshot: OwnedContainerSnapshot,
    authority: RuntimeAuthority,
    generation: CancelGeneration,
    expected_generation: u64,
    confirmed_head: RevisionRefV1,
    facts_hash: String,
}

impl AuthorizedLegacyNamedImportCreate {
    pub(crate) fn operation_id(&self) -> Uuid {
        self.operation_id
    }

    pub(crate) fn import_policy(&self) -> &crate::control::HelperExecutionPolicyV1 {
        &self.import_policy
    }

    pub(crate) fn import_snapshot(&self) -> &OwnedContainerSnapshot {
        &self.import_snapshot
    }

    pub(crate) fn generation(&self) -> (&CancelGeneration, u64) {
        (&self.generation, self.expected_generation)
    }

    pub(crate) fn validate_runtime_request(
        &self,
        parent_operation: Uuid,
        policy: &crate::control::HelperExecutionPolicyV1,
        spec: &crate::runtime::ContainerSpec,
        authority: &RuntimeAuthority,
    ) -> anyhow::Result<()> {
        policy.validate()?;
        self.import_policy.validate()?;
        anyhow::ensure!(
            policy.purpose == crate::control::HelperPurposeV1::ImportVerification
                && policy.helper_id.parse::<Uuid>()? == self.helper_id
                && policy.spec_hash()? == self.policy_hash
                && crate::broker::checkpoint::import_verification_spec_hash(policy)?
                    == self.readonly_spec_hash
                && self.import_policy.purpose == crate::control::HelperPurposeV1::Import
                && self.import_policy.helper_id.parse::<Uuid>()?
                    == self.import_snapshot.handle.operation_id()
                && self.import_policy.helper_id.parse::<Uuid>()? != self.helper_id
                && self.import_policy.session_id == policy.session_id
                && self.import_policy.volume_name == policy.volume_name
                && self.import_policy.parent_worker_id == policy.parent_worker_id
                && self.import_policy.parent_attempt_id == policy.parent_attempt_id
                && self.import_policy.parent_spec_hash == policy.parent_spec_hash
                && self.import_policy.workspace_manifest_digest == policy.workspace_manifest_digest
                && self.import_policy.authority_id == policy.authority_id
                && self.import_snapshot.handle.session_id()
                    == self.import_policy.session_id.parse::<Uuid>()?
                && self.import_snapshot.handle.authority_fingerprint() == authority.fingerprint()
                && parent_operation == self.operation_id
                && parent_operation != self.helper_id
                && spec.operation_id() == self.helper_id
                && spec.session == policy.session_id.parse::<Uuid>()?
                && matches!(
                    spec.purpose,
                    crate::runtime::ContainerPurpose::ImportVerification
                )
                && spec.network == crate::runtime::NetworkMode::None
                && spec.image == policy.image_id
                && spec.argv == policy.command.argv
                && spec.is_read_only_workspace()
                && spec.has_managed_names(&policy.stable_name, &policy.volume_name)
                && spec.has_no_runtime_credentials_or_egress()
                && spec.security_preset() == policy.security_preset
                && self.authority == *authority
                && authority.authority_id.to_string() == policy.authority_id
                && matches!(
                    (authority.kind, policy.runtime_kind),
                    (
                        crate::runtime::RuntimeKind::Docker,
                        crate::control::RuntimeKindV1::Docker
                    ) | (
                        crate::runtime::RuntimeKind::Podman,
                        crate::control::RuntimeKindV1::Podman
                    )
                ),
            "named ImportVerification Create capability does not match its durable plan"
        );
        anyhow::ensure!(
            self.confirmed_head.revision() > 0 && self.facts_hash.len() == 64,
            "named ImportVerification Create capability is malformed"
        );
        Ok(())
    }

    pub(in crate::broker) fn confirmed_head(&self) -> &RevisionRefV1 {
        &self.confirmed_head
    }

    pub(crate) fn facts_hash(&self) -> &str {
        &self.facts_hash
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

    /// Persist the single read-only ImportVerification Create boundary. Only
    /// the caller whose exact first append was confirmed receives a one-shot
    /// token; exact replay and uncertain publication never mint another one.
    pub(in crate::broker) async fn record_legacy_named_import_verification_create(
        &self,
        plan: &LegacyConfirmedPlanV1,
        terminal_receipt: &MutationReceiptV1,
        binding: &RuntimeAuthority,
        generation: &CancelGeneration,
        expected_generation: u64,
    ) -> JournalResult<Option<AuthorizedLegacyNamedImportCreate>> {
        for _ in 0..ACK_CAS_ATTEMPTS {
            let job = self.load_named_job(plan).await?;
            let current = super::lifecycle(&job)?;
            let retained = retained_admission(&job)?;
            if retained.import_verification_create().is_some() {
                let replayed = retained
                    .clone()
                    .with_import_verification_create(terminal_receipt.clone(), binding)?;
                if replayed.import_verification_create().is_none() {
                    return Err(JournalError::Corrupt);
                }
                return Ok(None);
            }

            generation
                .with_current_generation(expected_generation, || ())
                .map_err(|_| JournalError::Interrupted)?;
            require_open(&current)?;
            let pending = retained
                .clone()
                .with_import_verification_create(terminal_receipt.clone(), binding)?;
            let expected = pending
                .import_verification_create()
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
                        .import_verification_create()
                        .ok_or(JournalError::Conflict)?;
                    if saved_job.head() != receipt.head()
                        || canonical(saved)? != canonical(&expected)?
                    {
                        return Err(JournalError::Conflict);
                    }
                    let policy = retained
                        .import_verification_plan()
                        .ok_or(JournalError::Corrupt)?
                        .preparation()
                        .policy();
                    let helper_id =
                        Uuid::parse_str(&policy.helper_id).map_err(|_| JournalError::Corrupt)?;
                    let facts_hash = saved.facts_hash()?;
                    let policy_hash = policy.spec_hash().map_err(|_| JournalError::Corrupt)?;
                    let readonly_spec_hash =
                        super::super::checkpoint::import_verification_spec_hash(policy)?;
                    let import_policy = retained
                        .import_creation()
                        .ok_or(JournalError::Corrupt)?
                        .policy()
                        .clone();
                    let import_snapshot = retained
                        .import_creation()
                        .and_then(HelperPreparationV1::owned)
                        .ok_or(JournalError::Corrupt)?
                        .clone();
                    let confirmed_head = receipt.head().clone();
                    let authorization = generation
                        .with_current_generation(expected_generation, || {
                            AuthorizedLegacyNamedImportCreate {
                                operation_id: plan.operation_id,
                                helper_id,
                                policy_hash,
                                readonly_spec_hash,
                                import_policy,
                                import_snapshot,
                                authority: binding.clone(),
                                generation: generation.clone(),
                                expected_generation,
                                confirmed_head,
                                facts_hash,
                            }
                        })
                        .map_err(|_| JournalError::Interrupted)?;
                    return Ok(Some(authorization));
                }
                Err(LifecycleWriteFailure::BeforePublication(JournalError::Conflict)) => continue,
                Err(failure) => return Err(failure.journal_error()),
            }
        }
        Err(JournalError::Conflict)
    }

    /// Retain the exact runtime observation in the original accepted job's
    /// scope. A concurrent STOP can advance the head while the observation
    /// remains eligible for retention. Success confirms the snapshot only.
    #[allow(
        dead_code,
        reason = "the named verification Service caller is composed separately"
    )]
    pub(in crate::broker) async fn acknowledge_legacy_named_import_verification_created(
        &self,
        admitted: &AcceptedJobV1,
        created: NamedImportVerificationCreated,
    ) -> JournalResult<()> {
        let (operation, create_facts_hash, binding, owned) = created.into_writer_parts();
        let plan = admitted
            .intent()
            .legacy_named_plan()
            .ok_or(JournalError::Conflict)?;
        let scope = canonical(admitted.scope())?;
        if operation != admitted.operation_id()
            || operation != plan.operation_id
            || canonical(self.scope())? != scope
        {
            return Err(JournalError::Conflict);
        }

        for _ in 0..ACK_CAS_ATTEMPTS {
            let job = self.load_named_job(plan).await?;
            if job.intent_hash() != admitted.intent_hash() || canonical(job.scope())? != scope {
                return Err(JournalError::Conflict);
            }
            let current = super::lifecycle(&job)?;
            let retained = retained_admission(&job)?;
            let acknowledged = retained.clone().with_import_verification_created(
                &create_facts_hash,
                &binding,
                owned.clone(),
            )?;
            if retained.import_verification_created().is_some() {
                return Ok(());
            }
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

    /// Retain the first verification Arm for the exact accepted job. A fresh
    /// result confirms the local write only. Replays return the retained body.
    #[allow(
        dead_code,
        reason = "the verification Arm caller is composed separately"
    )]
    pub(in crate::broker) async fn record_legacy_named_import_verification_arm(
        &self,
        admitted: &AcceptedJobV1,
        request: LegacyNamedImportVerificationArmRequestV2,
        binding: &RuntimeAuthority,
        generation: &CancelGeneration,
        expected_generation: u64,
    ) -> Result<LegacyNamedImportVerificationArmWrite, NamedImportArmWriteFailure> {
        let plan = admitted
            .intent()
            .legacy_named_plan()
            .ok_or(JournalError::Conflict)?;
        let operation = admitted.operation_id();
        let scope = canonical(admitted.scope())?;
        if operation != plan.operation_id || canonical(self.scope())? != scope {
            return Err(JournalError::Conflict.into());
        }

        for _ in 0..ACK_CAS_ATTEMPTS {
            let job = self.load_named_job(plan).await?;
            if job.intent_hash() != admitted.intent_hash() || canonical(job.scope())? != scope {
                return Err(JournalError::Conflict.into());
            }
            let retained = retained_admission(&job)?;
            retained
                .import_verification_arm_facts()?
                .ok_or(JournalError::Conflict)?
                .validate_request(&request, binding)?;
            let body = canonical(&request)?;
            if let Some(saved) = retained.import_verification_arm_request() {
                if canonical(saved)? != body {
                    return Err(JournalError::Conflict.into());
                }
                return Ok(LegacyNamedImportVerificationArmWrite::Replayed(Box::new(
                    saved.clone(),
                )));
            }

            let current = super::lifecycle(&job)?;
            require_open(&current)?;
            let pending = retained
                .clone()
                .with_import_verification_arm(request.clone())?;
            let next = with_admission(&current, pending)?;
            let mut advance = Box::pin(self.advance_lifecycle_with_publication(
                operation,
                job.head().clone(),
                next,
            ));
            // This first poll reaches the synchronous Writer enqueue. Order
            // it with STOP, then wait for disk and the reply outside the gate.
            let first = std::future::poll_fn(|context| {
                Poll::Ready(generation.with_current_generation(expected_generation, || {
                    advance.as_mut().poll(context)
                }))
            })
            .await
            .map_err(|_| JournalError::Interrupted)?;
            let publication = match first {
                Poll::Ready(result) => result,
                Poll::Pending => advance.await,
            };
            let receipt = match publication {
                Ok(receipt) => receipt,
                Err(LifecycleWriteFailure::BeforePublication(JournalError::Conflict)) => continue,
                Err(failure) => return Err(failure.into()),
            };

            return async {
                let saved = self.load(operation).await?.ok_or(JournalError::Corrupt)?;
                let confirmed = retained_admission(&saved)?
                    .import_verification_arm_request()
                    .ok_or(JournalError::Conflict)?;
                if saved.head() != receipt.head()
                    || saved.intent_hash() != admitted.intent_hash()
                    || canonical(saved.scope())? != scope
                    || canonical(confirmed)? != body
                {
                    return Err(JournalError::Conflict);
                }
                generation
                    .with_current_generation(expected_generation, || {
                        LegacyNamedImportVerificationArmWrite::Recorded(Box::new(
                            RecordedLegacyNamedImportVerificationArm {
                                job: saved,
                                generation: generation.clone(),
                                expected_generation,
                            },
                        ))
                    })
                    .map_err(|_| JournalError::Interrupted)
            }
            .await
            .map_err(NamedImportArmWriteFailure::ReloadAfterPublication);
        }
        Err(JournalError::Conflict.into())
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
