//! Confirmed STOP metadata and restricted cancellation capabilities. The reader
//! is read-only; StopJournal can append only CancelRequested through the shared
//! publisher. Neither opens credential blobs or constructs a business job,
//! fresh-start permission, credential handoff or enforcer registration.
use super::*;
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
use crate::control::{ArmedStartClaimV1, MutationReceiptV1};
use crate::{
    audit::legacy_session_prepare::LegacyNamedCancelReservationV1,
    broker::{
        checkpoint::{LegacyNamedAdmissionCheckpoint, SupervisionOperationV1},
        lifecycle::{LocalCommandRequestV1, LocalCommandV1},
        retirement_checkpoint::Full3RegistrationBindingV1,
    },
    control::{
        ArmReceiptV1, ArmSubjectV1, ArmV1, RuntimeStatusV1, WorkerMutationV1, WorkerStateV1,
        WorkerTransitionV1, WorkerV1, session_control::LegacyNamedAdmissionReceiptV1,
    },
    runtime::{OwnedContainerSnapshot, RuntimeAuthority},
};
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};
#[cfg(all(test, unix))]
#[path = "recovery_retirement_tests.rs"]
mod retirement_owner_tests;

pub struct StopRecoveryReaderV1 {
    scope: JournalScopeV1,
    scope_hash: String,
    namespace: Directory,
    directory: Directory,
    jobs: Directory,
    lock: PrivateFile,
    exclusions: FsExclusions,
    limits: JournalLimitsV1,
    control: ControlSecrets,
    synchronize_reads: bool,
}
/// Retains the existing exclusive lease while stop targets are restored. One
/// profile actor can then inherit that same FD before this report is dropped;
/// admission never releases and reacquires the lease or grants a fresh start.
pub struct StopRecoveryReportV1 {
    reader: Arc<StopRecoveryReaderV1>,
    jobs: Vec<RecoveredJobV1>,
    issues: Vec<StopRecoveryIssueV1>,
    incomplete: bool,
    issues_truncated: bool,
    observed_jobs: HashSet<Uuid>,
    rejected_floors: HashMap<Uuid, String>,
    actor_claimed: AtomicBool,
    retained_core: OnceLock<Arc<Inner>>,
}
pub struct RecoveredJobV1 {
    operation_id: Uuid,
    head: RevisionRefV1,
    managed_workspace: bool,
    cancel_command_ids: Vec<Uuid>,
    stops: Vec<RecoveredStopV1>,
    chain: Vec<RevisionRefV1>,
    command_receipts: HashMap<Uuid, LocalCommandReceiptV1>,
    outbox_receipts: HashMap<(Uuid, Option<i64>), OutboxReceiptV1>,
    cancel_boundaries: HashMap<Uuid, usize>,
    pending_arms: usize,
    named_import_arm_revision: Option<u64>,
    metadata: Option<Box<StopMetadata>>,
    retirements: Vec<crate::broker::retirement_checkpoint::RetirementEntryV1>,
}
struct StopMetadata {
    intent: ExecutionIntentV1,
    binding: BindingManifestV1,
    launch_binding: Option<DockerLaunchProfileBindingV1>,
    checkpoint: LoadedCheckpoint,
    legacy_managed_seen: bool,
    last_cancel_operations: Option<usize>,
}
pub struct RecoveredStopV1 {
    arm: ArmV1,
    receipt: ArmReceiptV1,
    authority: RuntimeAuthority,
    owned: OwnedContainerSnapshot,
    cancel_after_arm: Option<Uuid>,
    birth_index: usize,
    full3_binding: Option<crate::broker::retirement_checkpoint::Full3RegistrationBindingV1>,
    retirement: Option<crate::broker::retirement_checkpoint::RetirementEntryV1>,
}

/// Verified V5 facts, retained under the startup reader's lease. The claim
/// was acknowledged before STOP and the same running attempt is still recorded.
/// This evidence can corroborate a terminal observation. It grants no authority
/// to publish completion or release the worker's allocation.
pub(in crate::broker) struct Full3WorkerTerminalEvidenceV1 {
    report: Arc<StopRecoveryReportV1>,
    operation: Uuid,
    head: RevisionRefV1,
    directory: Directory,
    target: RecoveredStopV1,
    binding: Full3RegistrationBindingV1,
    stop: LocalCommandReceiptV1,
    worker: WorkerV1,
}

struct RecoveredFull3WorkerClaim {
    directory: Directory,
    target: RecoveredStopV1,
    binding: Full3RegistrationBindingV1,
    stop: LocalCommandReceiptV1,
    running_worker: Option<WorkerV1>,
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    claim: ArmedStartClaimV1,
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    claim_receipt: MutationReceiptV1,
}

impl Full3WorkerTerminalEvidenceV1 {
    pub(in crate::broker) fn scope(&self) -> &JournalScopeV1 {
        self.report.scope()
    }

    pub(in crate::broker) fn operation_id(&self) -> Uuid {
        self.operation
    }

    pub(in crate::broker) fn head(&self) -> &RevisionRefV1 {
        &self.head
    }

    pub(in crate::broker) fn arm(&self) -> &ArmV1 {
        self.target.arm()
    }

    pub(in crate::broker) fn arm_receipt(&self) -> &ArmReceiptV1 {
        self.target.receipt()
    }

    pub(in crate::broker) fn authority(&self) -> &RuntimeAuthority {
        self.target.authority()
    }

    pub(in crate::broker) fn owned(&self) -> &OwnedContainerSnapshot {
        self.target.owned()
    }

    pub(in crate::broker) fn binding(&self) -> &Full3RegistrationBindingV1 {
        &self.binding
    }

    pub(in crate::broker) fn cancel_receipt(&self) -> &LocalCommandReceiptV1 {
        &self.stop
    }

    pub(in crate::broker) fn worker(&self) -> &WorkerV1 {
        &self.worker
    }

    pub(in crate::broker) fn belongs_to(&self, report: &Arc<StopRecoveryReportV1>) -> bool {
        Arc::ptr_eq(&self.report, report)
    }

    pub(in crate::broker) fn revalidate(&self) -> JournalResult<()> {
        self.report.revalidate_full3_observation_lease()?;
        fscheck(self.directory.revalidate())?;
        self.report.revalidate_full3_observation_lease()
    }
}

/// A verified, immutable cancellation prefix that shares the lock FD of the
/// existing writer or startup reader.
/// No business job, credential bytes or start capability can be obtained here.
pub struct CancelStopProjectionV1 {
    reader: Arc<StopRecoveryReaderV1>,
    operation_id: Uuid,
    command_id: Uuid,
    receipt: LocalCommandReceiptV1,
    stops: Vec<RecoveredStopV1>,
    cancel_prefixes: Vec<ConfirmedCancelPrefixV1>,
    named_reservation: Option<Box<NamedReservationStop>>,
    named_import: Option<Box<NamedImportStop>>,
    incomplete: bool,
}

/// A confirmed STOP against a named admission before any preparation intent.
pub(in crate::broker) struct NamedReservationStop {
    admission: LegacyNamedAdmissionCheckpoint,
    cancellation_id: String,
}

impl NamedReservationStop {
    pub(in crate::broker) fn admission(&self) -> &LegacyNamedAdmissionCheckpoint {
        &self.admission
    }

    pub(in crate::broker) fn cancellation(
        &self,
        receipt: LegacyNamedAdmissionReceiptV1,
    ) -> JournalResult<Option<(LegacyNamedCancelReservationV1, WorkerV1)>> {
        let confirmed = self.admission.clone().acknowledge(receipt)?;
        let receipt = confirmed.receipt().ok_or(JournalError::Corrupt)?;
        let Some(reserved) = &receipt.admission.worker else {
            // The exact admission was refused. There is no quota to release.
            return Ok(None);
        };
        let request = LegacyNamedCancelReservationV1 {
            version: 1,
            binding_ref: confirmed.plan().binding_ref,
            namespace: confirmed.plan().control_namespace_uuid.clone(),
            admission: receipt.admission.receipt.clone(),
            mutation: WorkerMutationV1 {
                version: 1,
                operation_id: self.cancellation_id.clone(),
                worker_id: reserved.worker_id.clone(),
                attempt_id: reserved.attempt.attempt_id.clone(),
                expected_revision: reserved.revision,
                transition: WorkerTransitionV1::CancelReserved,
            },
        };
        request.input_hash().map_err(|_| JournalError::Conflict)?;
        Ok(Some((request, reserved.clone())))
    }
}

/// Facts for a named import whose Created observation and Arm pending record
/// were retained before STOP. The Arm ACK can be absent. This projection carries no RuntimeAuthority and
/// cannot be used as a start or runtime-operation capability.
pub(in crate::broker) struct NamedImportStop {
    admission: LegacyNamedAdmissionCheckpoint,
    evidence: crate::broker::checkpoint::LegacyNamedCreatedImportEvidenceV1,
    arm_pending_head: RevisionRefV1,
    stop_head: RevisionRefV1,
    command_id: Uuid,
}

#[allow(
    dead_code,
    reason = "consumed by the named import STOP reconciliation lot"
)]
impl NamedImportStop {
    pub(in crate::broker) fn admission(&self) -> &LegacyNamedAdmissionCheckpoint {
        &self.admission
    }
    pub(in crate::broker) fn evidence(
        &self,
    ) -> &crate::broker::checkpoint::LegacyNamedCreatedImportEvidenceV1 {
        &self.evidence
    }
    pub(in crate::broker) fn arm_pending_head(&self) -> &RevisionRefV1 {
        &self.arm_pending_head
    }
    pub(in crate::broker) fn stop_head(&self) -> &RevisionRefV1 {
        &self.stop_head
    }
    pub(in crate::broker) fn command_id(&self) -> Uuid {
        self.command_id
    }
}

/// Facts of one verified named STOP, borrowed from the projection that owns
/// the reader lease. The packet carries no authority and excludes a Core Arm
/// receipt, which can arrive after STOP without changing these facts.
#[derive(Serialize)]
pub(in crate::broker) struct NamedImportStopFactsV1<'a> {
    version: u32,
    scope: &'a JournalScopeV1,
    operation_id: Uuid,
    binding_ref: Uuid,
    arm_pending_head: &'a RevisionRefV1,
    command_id: Uuid,
    command_input_hash: &'a str,
    stop_head: &'a RevisionRefV1,
    arm_id: &'a str,
    arm_input_hash: String,
    owned_snapshot_hash: String,
    runtime_binding_hash: &'a str,
}

/// Fingerprint the exact retained Created snapshot using journal canonical JSON.
pub(in crate::broker) fn named_import_stop_owned_hash(
    owned: &OwnedContainerSnapshot,
) -> JournalResult<String> {
    Ok(digest(
        "ai-guard:legacy-named-import-stop-owned:v1",
        &canonical(owned)?,
    ))
}

/// Metadata attested by the existing reader's verified revision chain. This
/// type has no deserializer or caller constructor and grants no new mutation.
#[derive(Clone)]
pub(crate) struct ConfirmedCancelPrefixV1 {
    head: RevisionRefV1,
    receipts: Vec<LocalCommandReceiptV1>,
}
impl ConfirmedCancelPrefixV1 {
    pub(crate) fn head(&self) -> &RevisionRefV1 {
        &self.head
    }
    pub(crate) fn receipts(&self) -> &[LocalCommandReceiptV1] {
        &self.receipts
    }
}

/// An existing profile's metadata-only cancellation capability. No API can
/// obtain a business journal, credential source, or fresh-start permission.
#[derive(Clone)]
pub struct StopJournal {
    core: Arc<Inner>,
    scope_incomplete: bool,
}
pub struct StopJobSnapshotV1 {
    // Original retained producer, not reconstructed from scope or contents.
    origin: Arc<Inner>,
    reader: StopRecoveryReaderV1,
    operation_id: Uuid,
    head: RevisionRefV1,
    cancel_receipts: Vec<LocalCommandReceiptV1>,
    stops: Vec<RecoveredStopV1>,
    pending_arms: usize,
    incomplete: bool,
    retirements: Vec<crate::broker::retirement_checkpoint::RetirementEntryV1>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopRecoveryPhaseV1 {
    Inventory,
    SharedScope,
    Job,
}
pub struct StopRecoveryIssueV1 {
    operation_id: Option<Uuid>,
    phase: StopRecoveryPhaseV1,
    error: JournalError,
}
macro_rules! opaque {
    ($($kind:ty),+ $(,)?) => {$(impl fmt::Debug for $kind {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(concat!(stringify!($kind), "(<private>)"))
        }
    })+};
}
opaque!(
    StopRecoveryReaderV1,
    StopRecoveryReportV1,
    RecoveredJobV1,
    RecoveredStopV1,
    StopRecoveryIssueV1,
    CancelStopProjectionV1,
    StopJournal,
    StopJobSnapshotV1
);

pub(in crate::broker) mod cold_audit;
pub mod inventory;
#[allow(dead_code, reason = "the Stops writer query is a separate integration")]
pub(in crate::broker) mod legacy_worker_terminal;

impl StopJobSnapshotV1 {
    pub(crate) fn retirements(&self) -> &[crate::broker::retirement_checkpoint::RetirementEntryV1] {
        &self.retirements
    }
    pub fn scope(&self) -> &JournalScopeV1 {
        &self.reader.scope
    }
    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }
    pub fn head(&self) -> &RevisionRefV1 {
        &self.head
    }
    pub fn cancel_receipts(&self) -> &[LocalCommandReceiptV1] {
        &self.cancel_receipts
    }
    pub fn stops(&self) -> &[RecoveredStopV1] {
        &self.stops
    }
    pub fn pending_arms(&self) -> usize {
        self.pending_arms
    }
    pub fn incomplete(&self) -> bool {
        self.incomplete
    }
}

impl CancelStopProjectionV1 {
    pub(in crate::broker) fn named_reservation(&self) -> Option<&NamedReservationStop> {
        self.named_reservation.as_deref()
    }
    #[allow(
        dead_code,
        reason = "consumed by the named import STOP reconciliation lot"
    )]
    pub(in crate::broker) fn named_import(&self) -> Option<&NamedImportStop> {
        self.named_import.as_deref()
    }

    /// Derive the wire facts for this command from its verified journal prefix.
    /// Absence of a projection leaves the native target's status unknown.
    #[allow(
        dead_code,
        reason = "the named guardian STOP transport is composed separately"
    )]
    pub(in crate::broker) fn named_import_stop_facts(
        &self,
    ) -> JournalResult<Option<NamedImportStopFactsV1<'_>>> {
        self.revalidate()?;
        let Some(named) = self.named_import() else {
            return Ok(None);
        };
        let scope = self.scope();
        let plan = named.admission().plan();
        let evidence = named.evidence();
        let arm = evidence.arm();
        let receipt = self.receipt();
        let cancel = LocalCommandV1::new(
            self.operation_id(),
            &arm.intent_hash,
            self.command_id(),
            LocalCommandRequestV1::CancelRequested,
        )?;
        if plan.operation_id != self.operation_id()
            || plan.control_namespace_uuid != scope.control_namespace_uuid().to_string()
            || plan.broker_profile_id != scope.profile_id()
            || named.command_id() != receipt.command_id()
            || named.stop_head() != receipt.head()
            || receipt.command_id() != cancel.command_id()
            || receipt.input_hash() != cancel.input_hash()
        {
            return Err(JournalError::Conflict);
        }
        Ok(Some(NamedImportStopFactsV1 {
            version: 1,
            scope,
            operation_id: self.operation_id(),
            binding_ref: plan.binding_ref,
            arm_pending_head: named.arm_pending_head(),
            command_id: receipt.command_id(),
            command_input_hash: receipt.input_hash(),
            stop_head: receipt.head(),
            arm_id: &arm.operation_id,
            arm_input_hash: arm.input_hash().map_err(|_| JournalError::Corrupt)?,
            owned_snapshot_hash: named_import_stop_owned_hash(evidence.owned())?,
            runtime_binding_hash: evidence.runtime_binding_hash(),
        }))
    }

    pub(crate) fn cancel_prefix_for_stop(&self, index: usize) -> Option<&ConfirmedCancelPrefixV1> {
        self.cancel_prefixes.get(index)
    }
    pub(crate) fn revalidate(&self) -> JournalResult<()> {
        self.reader.check_shared()
    }
    pub fn scope(&self) -> &JournalScopeV1 {
        &self.reader.scope
    }
    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }
    pub fn command_id(&self) -> Uuid {
        self.command_id
    }
    pub fn receipt(&self) -> &LocalCommandReceiptV1 {
        &self.receipt
    }
    pub fn stops(&self) -> &[RecoveredStopV1] {
        &self.stops
    }
    /// Job or retained profile coverage could not be proved complete. Known
    /// targets remain usable even when unrelated metadata is unavailable.
    pub fn incomplete(&self) -> bool {
        self.incomplete
    }
}

impl Journal {
    pub(crate) fn matches_recovery_origin(&self, snapshot: &StopJobSnapshotV1) -> bool {
        Arc::ptr_eq(&self.0, &snapshot.origin)
    }

    /// Read STOP-only facts on the writer's bounded FS thread. In contrast to
    /// load/status/command_receipt, this never opens the credential blob.
    pub fn cancel_stop_projection(
        &self,
        operation_id: Uuid,
        command_id: Uuid,
    ) -> JournalResult<Option<CancelStopProjectionV1>> {
        StopJournal::from_full(self).cancel_stop_projection(operation_id, command_id)
    }
}

type StopRead = (StopRecoveryReaderV1, RecoveredJobV1, bool);

// Recovery bounds the control records it actually reads independently of blob
// metadata. A broken blob cannot revoke a proven STOP projection. Its observed
// bytes still diagnose the physical budget; overflow is unknown, never zero.
// Mutation uses stop_budget_usage's unchanged strict accounting, not this data.
struct RecoveryBlobBytes {
    scope: Option<u64>,
    job: Option<u64>,
}
impl Default for RecoveryBlobBytes {
    fn default() -> Self {
        Self {
            scope: Some(0),
            job: Some(0),
        }
    }
}
impl RecoveryBlobBytes {
    fn next_job(&mut self) {
        self.job = Some(0);
    }
    fn observe(&mut self, bytes: u64) {
        self.scope = self.scope.and_then(|total| total.checked_add(bytes));
        self.job = self.job.and_then(|total| total.checked_add(bytes));
    }
    fn exceeds_or_unknown(&self, records: &Usage, limits: &JournalLimitsV1) -> bool {
        self.scope
            .and_then(|bytes| records.bytes.checked_add(bytes))
            .is_none_or(|bytes| bytes > limits.scope_bytes)
            || self
                .job
                .and_then(|bytes| records.job_bytes.checked_add(bytes))
                .is_none_or(|bytes| bytes > limits.job_bytes)
    }
}

impl StopJournal {
    pub(crate) fn from_full(journal: &Journal) -> Self {
        Self {
            core: Arc::clone(&journal.0),
            scope_incomplete: false,
        }
    }
    pub fn scope(&self) -> &JournalScopeV1 {
        &self.core.scope
    }

    fn read_locked(
        &self,
        operation_id: Uuid,
        writer: &mut Writer,
    ) -> JournalResult<Option<StopRead>> {
        let result = self.read_locked_inner(operation_id, writer);
        super::note_status_read_failure(&self.core, &result);
        result
    }
    fn read_locked_inner(
        &self,
        operation_id: Uuid,
        writer: &mut Writer,
    ) -> JournalResult<Option<StopRead>> {
        if operation_id.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        let reader = StopRecoveryReaderV1::from_core(&self.core)?;
        if fscheck(
            self.core
                .jobs
                .persistent_child(&operation_id.to_string(), false),
        )?
        .is_none()
        {
            return if writer.observed_jobs.contains(&operation_id)
                || writer.floors.contains_key(&operation_id)
                || writer.rejected_floors.contains_key(&operation_id)
            {
                Err(JournalError::Conflict)
            } else {
                Ok(None)
            };
        }
        if !writer.observed_jobs.contains(&operation_id)
            && writer.observed_jobs.len() >= self.core.limits.jobs as usize
        {
            return Err(JournalError::LimitExceeded);
        }
        // Existence itself is observed even when the accepted prefix is later
        // found interrupted. A subsequent disappearance cannot become Absent.
        writer.observed_jobs.insert(operation_id);
        let (recovered, rejected_hash, partial) = reader.read_job(
            operation_id,
            &mut Usage::default(),
            &mut RecoveryBlobBytes::default(),
        )?;
        if writer
            .rejected_floors
            .get(&operation_id)
            .is_some_and(|old| Some(old) != rejected_hash.as_ref())
            || writer.floors.contains_key(&operation_id) && rejected_hash.is_some()
        {
            return Err(JournalError::Conflict);
        }
        if let Some(hash) = rejected_hash {
            writer.rejected_floors.insert(operation_id, hash);
            return Err(JournalError::Interrupted);
        }
        let Some(recovered) = recovered else {
            return Err(if writer.floors.contains_key(&operation_id) {
                JournalError::Conflict
            } else {
                JournalError::Interrupted
            });
        };
        if writer.rejected_floors.contains_key(&operation_id) {
            return Err(JournalError::Conflict);
        }
        if let Some(floor) = writer.floors.get(&operation_id) {
            let index = usize::try_from(floor.revision).map_err(|_| JournalError::Corrupt)?;
            if recovered.chain.get(index) != Some(floor) {
                return Err(JournalError::Conflict);
            }
        }
        // Scope inventory is diagnostic here, never a reason to discard a
        // selected job's proven stops. Mutation separately requires exact usage.
        let scope_partial =
            reader
                .inventory_scopes(&mut Usage::default())
                .map_or(true, |(entries, partial)| {
                    partial
                        || entries.iter().any(|entry| {
                            !entry.directory
                                || Uuid::parse_str(&entry.name)
                                    .map_or(true, |id| id.is_nil() || id.to_string() != entry.name)
                        })
                });
        if scope_partial
            || self.scope_incomplete
            || self.core.budget_incomplete.load(Ordering::SeqCst)
        {
            self.core
                .status_observation_failed
                .store(true, Ordering::SeqCst);
        }
        reader.check_shared()?;
        writer.floors.insert(operation_id, recovered.head.clone());
        writer.observed_jobs.insert(operation_id);
        Ok(Some((
            reader,
            recovered,
            partial
                || scope_partial
                || self.scope_incomplete
                || self.core.budget_incomplete.load(Ordering::SeqCst),
        )))
    }

    pub fn cancel_stop_projection(
        &self,
        operation_id: Uuid,
        command_id: Uuid,
    ) -> JournalResult<Option<CancelStopProjectionV1>> {
        if command_id.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        let mut writer = self.core.writer.lock().map_err(|_| JournalError::Corrupt)?;
        let Some((reader, recovered, incomplete)) = self.read_locked(operation_id, &mut writer)?
        else {
            return Ok(None);
        };
        project_cancel(Arc::new(reader), recovered, incomplete, command_id)
    }

    pub fn recovery_status(&self, operation_id: Uuid) -> JournalResult<Option<StopJobSnapshotV1>> {
        let mut writer = self.core.writer.lock().map_err(|_| JournalError::Corrupt)?;
        let Some((reader, recovered, incomplete)) = self.read_locked(operation_id, &mut writer)?
        else {
            return Ok(None);
        };
        let cancel_receipts = recovered
            .cancel_command_ids
            .iter()
            .map(|id| {
                recovered
                    .command_receipts
                    .get(id)
                    .cloned()
                    .ok_or(JournalError::Corrupt)
            })
            .collect::<JournalResult<Vec<_>>>()?;
        Ok(Some(StopJobSnapshotV1 {
            origin: Arc::clone(&self.core),
            reader,
            operation_id,
            retirements: recovered.retirements,
            head: recovered.head,
            cancel_receipts,
            stops: recovered.stops,
            pending_arms: recovered.pending_arms,
            incomplete,
        }))
    }

    pub fn append_cancel(
        &self,
        operation_id: Uuid,
        command_id: Uuid,
    ) -> JournalResult<CancelStopProjectionV1> {
        if command_id.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        let mut writer = self.core.writer.lock().map_err(|_| JournalError::Corrupt)?;
        let (reader, recovered, incomplete) = self
            .read_locked(operation_id, &mut writer)?
            .ok_or(JournalError::InvalidInput)?;
        if recovered.command_receipts.contains_key(&command_id) {
            let result = project_cancel(Arc::new(reader), recovered, incomplete, command_id)?
                .ok_or(JournalError::Conflict)?;
            #[cfg(test)]
            stop_before_ack()?;
            return Ok(result);
        }
        let metadata = recovered.metadata.as_ref().ok_or(JournalError::Corrupt)?;
        let id = command_id.to_string();
        if metadata.checkpoint.retirement().is_some_and(|cp| cp.retirements().iter().any(|entry| *entry.request().identity().retirement_id() == command_id || entry.request().intention_event_id() == command_id || entry.request().final_event_id() == command_id))
            || metadata.checkpoint.outbox().is_some_and(|cp| cp.events().iter().any(|event| event.event_id() == command_id))
            || metadata.checkpoint.supervised().is_some_and(|cp| cp.supervision().operations().iter().any(|op| {
                matches!(op, SupervisionOperationV1::Export { request, .. } if request.operation_id == id)
            }))
        { return Err(JournalError::Conflict); }
        let command = crate::broker::lifecycle::LocalCommandV1::new(
            operation_id,
            &metadata
                .intent
                .intent_hash()
                .map_err(|_| JournalError::Corrupt)?,
            command_id,
            LocalCommandRequestV1::CancelRequested,
        )?;
        let mut commands = metadata
            .checkpoint
            .lifecycle()
            .map_or_else(Vec::new, |cp| cp.commands().to_vec());
        commands.push(command);
        let broker = metadata.checkpoint.supervised().cloned().map_or_else(
            || {
                BrokerCheckpointV2::new(
                    metadata.checkpoint.runner().clone(),
                    crate::broker::checkpoint::SupervisionCheckpointV1::new(
                        self.core.scope.control_namespace_uuid(),
                        Vec::new(),
                        Vec::new(),
                    )?,
                )
            },
            Ok,
        )?;
        let lifecycle = BrokerCheckpointV3::new(broker, commands)?;
        let next = if let Some(cp) = metadata.checkpoint.retirement() {
            LoadedCheckpoint::V5(cp.with_outbox(BrokerCheckpointV4::new(
                lifecycle,
                cp.outbox().events().to_vec(),
            )?)?)
        } else {
            match metadata.checkpoint.outbox() {
                Some(cp) => {
                    LoadedCheckpoint::V4(BrokerCheckpointV4::new(lifecycle, cp.events().to_vec())?)
                }
                None => LoadedCheckpoint::V3(lifecycle),
            }
        };
        // The only mutable field is the appended Cancel. Canonical inner data
        // comparisons make this restriction explicit, including opaque events.
        if canonical(metadata.checkpoint.runner())? != canonical(next.runner())? {
            return Err(JournalError::Conflict);
        }
        if let Some(cp) = metadata.checkpoint.supervised()
            && canonical(cp)? != canonical(next.supervised().ok_or(JournalError::Corrupt)?)?
        {
            return Err(JournalError::Conflict);
        }
        if let Some(cp) = metadata.checkpoint.outbox()
            && canonical(&cp.events())?
                != canonical(&next.outbox().ok_or(JournalError::Corrupt)?.events())?
        {
            return Err(JournalError::Conflict);
        }
        confirmed::context_checkpoint_with_launch(
            &self.core.scope,
            &self.core.control,
            &next,
            &metadata.intent,
            &metadata.binding,
            metadata.launch_binding.as_ref(),
        )?;
        confirmed::check_transition(
            &metadata.checkpoint,
            &next,
            metadata.legacy_managed_seen,
            metadata.last_cancel_operations,
        )?;
        let intent_hash = metadata
            .intent
            .intent_hash()
            .map_err(|_| JournalError::Corrupt)?;
        let (bytes, head) = encode_checkpoint(
            &self.core,
            operation_id,
            &intent_hash,
            &recovered.head,
            &next,
        )?;
        let usage = stop_budget_usage(&self.core, operation_id)?;
        reader.check_shared()?;
        publish_revision(
            &self.core,
            operation_id,
            &intent_hash,
            &recovered.head,
            &head,
            &bytes,
            PublicationResources {
                usage,
                budget: retirement::PublicationBudget::Cancel,
                writer: &mut writer,
            },
        )?;
        writer.floors.insert(operation_id, head.clone());
        writer.observed_jobs.insert(operation_id);
        let (reader, confirmed, incomplete) = self
            .read_locked(operation_id, &mut writer)?
            .ok_or(JournalError::Corrupt)?;
        if confirmed.head != head {
            return Err(JournalError::Corrupt);
        }
        let result = project_cancel(Arc::new(reader), confirmed, incomplete, command_id)?
            .ok_or(JournalError::Corrupt)?;
        #[cfg(test)]
        stop_before_ack()?;
        Ok(result)
    }
}

impl StopJournal {
    pub(crate) fn finalize_retirement(
        &self,
        verified: &crate::broker::enforcer::retirement::VerifiedRetirementReceipt,
    ) -> Result<CheckpointReceiptV1, super::retirement::RetirementMutationError> {
        let identity = verified.identity().clone();
        let operation = *identity.job_operation_id();
        let mut writer = self.core.writer.lock().map_err(|_| JournalError::Corrupt)?;
        if writer.cold_audit.upgrade().is_some() {
            return Err(super::retirement::RetirementMutationError::ColdReserved);
        }
        let (reader, recovered, _) = self
            .read_locked(operation, &mut writer)?
            .ok_or(JournalError::InvalidInput)?;
        let metadata = recovered.metadata.as_ref().ok_or(JournalError::Corrupt)?;
        let current = metadata
            .checkpoint
            .retirement()
            .ok_or(JournalError::Conflict)?;
        let next = crate::broker::journal::retirement::finalized_checkpoint(
            current,
            &metadata.intent,
            &identity,
            verified.receipt().clone(),
            verified.head().clone(),
            verified.accepted().clone(),
        )?;
        if canonical(current)? == canonical(&next)? {
            return Ok(CheckpointReceiptV1 {
                operation_id: operation,
                head: recovered.head,
            });
        }
        let publication =
            retirement::PublicationBudget::retirement_final(operation, current, &next, verified)?;
        let next = LoadedCheckpoint::V5(next);
        confirmed::context_checkpoint_with_launch(
            &self.core.scope,
            &self.core.control,
            &next,
            &metadata.intent,
            &metadata.binding,
            metadata.launch_binding.as_ref(),
        )?;
        confirmed::check_transition(
            &metadata.checkpoint,
            &next,
            metadata.legacy_managed_seen,
            metadata.last_cancel_operations,
        )?;
        let intent_hash = metadata
            .intent
            .intent_hash()
            .map_err(|_| JournalError::Corrupt)?;
        let (bytes, head) =
            encode_checkpoint(&self.core, operation, &intent_hash, &recovered.head, &next)?;
        let usage = stop_budget_usage(&self.core, operation)?;
        reader.check_shared()?;
        publish_revision(
            &self.core,
            operation,
            &intent_hash,
            &recovered.head,
            &head,
            &bytes,
            PublicationResources {
                usage,
                budget: publication,
                writer: &mut writer,
            },
        )?;
        writer.floors.insert(operation, head.clone());
        writer.observed_jobs.insert(operation);
        let (_, confirmed, _) = self
            .read_locked(operation, &mut writer)?
            .ok_or(JournalError::Corrupt)?;
        if confirmed.head != head {
            return Err(JournalError::Corrupt.into());
        }
        #[cfg(test)]
        stop_before_ack()?;
        Ok(CheckpointReceiptV1 {
            operation_id: operation,
            head,
        })
    }
}

impl StopJournal {
    pub(crate) fn pending_retirement_events(
        &self,
        operation: Uuid,
    ) -> JournalResult<Vec<crate::broker::outbox::OutboxEventV1>> {
        let mut writer = self.core.writer.lock().map_err(|_| JournalError::Corrupt)?;
        let (_, recovered, _) = self
            .read_locked(operation, &mut writer)?
            .ok_or(JournalError::InvalidInput)?;
        let metadata = recovered.metadata.as_ref().ok_or(JournalError::Conflict)?;
        let Some(checkpoint) = metadata.checkpoint.retirement() else {
            // A confirmed V1–V4 checkpoint cannot own retirement events.
            return Ok(Vec::new());
        };
        Ok(checkpoint
            .outbox()
            .events()
            .iter()
            .filter(|event| {
                event.audit_seq().is_none()
                    && checkpoint.retirements().iter().any(|entry| {
                        entry.request().intention_event_id() == event.event_id()
                            || (entry.finalized().is_some()
                                && entry.request().final_event_id() == event.event_id())
                    })
            })
            .cloned()
            .collect())
    }
    pub(crate) fn acknowledge_retirement_events(
        &self,
        operation: Uuid,
        acknowledgements: Vec<(Uuid, String, i64)>,
    ) -> JournalResult<Vec<OutboxReceiptV1>> {
        if operation.is_nil() || acknowledgements.is_empty() || acknowledgements.len() > 512 {
            return Err(JournalError::InvalidInput);
        }
        let mut writer = self.core.writer.lock().map_err(|_| JournalError::Corrupt)?;
        if writer.cold_audit.upgrade().is_some() {
            return Err(JournalError::Busy);
        }
        let (reader, recovered, _) = self
            .read_locked(operation, &mut writer)?
            .ok_or(JournalError::InvalidInput)?;
        let metadata = recovered.metadata.as_ref().ok_or(JournalError::Corrupt)?;
        let current = metadata
            .checkpoint
            .retirement()
            .ok_or(JournalError::Conflict)?;
        let mut events = current.outbox().events().to_vec();
        let mut ids = HashSet::new();
        for (id, hash, seq) in &acknowledgements {
            if !ids.insert(*id)
                || *seq <= 0
                || !current.retirements().iter().any(|entry| {
                    entry.request().intention_event_id() == *id
                        || (entry.finalized().is_some() && entry.request().final_event_id() == *id)
                })
            {
                return Err(JournalError::Conflict);
            }
            let event = events
                .iter_mut()
                .find(|event| event.event_id() == *id)
                .ok_or(JournalError::Conflict)?;
            if event.input_hash() != hash {
                return Err(JournalError::Conflict);
            }
            *event = event.acknowledged(*seq)?;
        }
        let next = current.with_outbox(BrokerCheckpointV4::new(
            current.outbox().lifecycle().clone(),
            events,
        )?)?;
        if canonical(current)? != canonical(&next)? {
            let publication =
                retirement::PublicationBudget::retirement_ack(operation, current, &next, &ids)?;
            let next = LoadedCheckpoint::V5(next);
            confirmed::context_checkpoint_with_launch(
                &self.core.scope,
                &self.core.control,
                &next,
                &metadata.intent,
                &metadata.binding,
                metadata.launch_binding.as_ref(),
            )?;
            confirmed::check_transition(
                &metadata.checkpoint,
                &next,
                metadata.legacy_managed_seen,
                metadata.last_cancel_operations,
            )?;
            let intent_hash = metadata
                .intent
                .intent_hash()
                .map_err(|_| JournalError::Corrupt)?;
            let (bytes, head) =
                encode_checkpoint(&self.core, operation, &intent_hash, &recovered.head, &next)?;
            let usage = stop_budget_usage(&self.core, operation)?;
            reader.check_shared()?;
            publish_revision(
                &self.core,
                operation,
                &intent_hash,
                &recovered.head,
                &head,
                &bytes,
                PublicationResources {
                    usage,
                    budget: publication,
                    writer: &mut writer,
                },
            )?;
            writer.floors.insert(operation, head);
            writer.observed_jobs.insert(operation);
        }
        let (_, recovered, _) = self
            .read_locked(operation, &mut writer)?
            .ok_or(JournalError::Corrupt)?;
        let receipts = acknowledgements
            .iter()
            .map(|(id, _, seq)| {
                recovered
                    .outbox_receipts
                    .get(&(*id, Some(*seq)))
                    .cloned()
                    .ok_or(JournalError::Corrupt)
            })
            .collect::<JournalResult<Vec<_>>>()?;
        #[cfg(test)]
        stop_before_ack()?;
        Ok(receipts)
    }
}

fn project_cancel(
    reader: Arc<StopRecoveryReaderV1>,
    recovered: RecoveredJobV1,
    incomplete: bool,
    command_id: Uuid,
) -> JournalResult<Option<CancelStopProjectionV1>> {
    let Some(boundary) = recovered.cancel_boundaries.get(&command_id).copied() else {
        return if recovered.command_receipts.contains_key(&command_id) {
            Err(JournalError::Conflict)
        } else {
            Ok(None)
        };
    };
    let receipt = recovered
        .command_receipts
        .get(&command_id)
        .cloned()
        .ok_or(JournalError::Corrupt)?;
    let named_import = named_import_stop(&recovered, command_id, &receipt).map(Box::new);
    let cancel_prefixes = recovered
        .stops
        .iter()
        .filter(|stop| stop.birth_index < boundary)
        .map(|stop| recovered.cancel_prefix(stop))
        .collect::<JournalResult<Vec<_>>>()?;
    let named_reservation = named_reservation_stop(&recovered).map(Box::new);
    let stops = recovered
        .stops
        .into_iter()
        .filter(|stop| stop.birth_index < boundary)
        .map(|mut stop| {
            stop.cancel_after_arm = Some(command_id);
            stop
        })
        .collect();
    Ok(Some(CancelStopProjectionV1 {
        reader,
        operation_id: recovered.operation_id,
        command_id,
        receipt,
        stops,
        cancel_prefixes,
        named_reservation,
        named_import,
        incomplete,
    }))
}

fn first_named_import_stop_receipt(job: &RecoveredJobV1) -> Option<(u64, &LocalCommandReceiptV1)> {
    let arm_revision = job.named_import_arm_revision?;
    job.cancel_command_ids().iter().find_map(|command| {
        let receipt = job.cancel_receipt(*command)?;
        (receipt.head().revision() > arm_revision).then_some((arm_revision, receipt))
    })
}

fn named_import_stop(
    recovered: &RecoveredJobV1,
    command_id: Uuid,
    stop_receipt: &LocalCommandReceiptV1,
) -> Option<NamedImportStop> {
    let metadata = recovered.metadata.as_ref()?;
    let admission = metadata
        .checkpoint
        .supervised()?
        .supervision()
        .legacy_named_admission()?;
    let plan = admission.plan();
    if admission.import_creation().is_none()
        || metadata.intent.legacy_named_plan() != Some(plan)
        || plan.operation_id != recovered.operation_id
    {
        return None;
    }
    let arm_revision = recovered.named_import_arm_revision?;
    let arm_index = usize::try_from(arm_revision).ok()?;
    // Both births must be independently confirmed by the exact verified
    // chain. A STOP in the same checkpoint revision is order-ambiguous.
    if stop_receipt.command_id() != command_id
        || arm_revision >= stop_receipt.head().revision()
        || recovered.chain.get(arm_index).is_none()
        || !recovered
            .chain
            .iter()
            .any(|head| head == stop_receipt.head())
    {
        return None;
    }
    Some(NamedImportStop {
        admission: admission.clone(),
        evidence: admission.created_import_evidence()?,
        arm_pending_head: recovered.chain.get(arm_index)?.clone(),
        stop_head: stop_receipt.head().clone(),
        command_id,
    })
}

fn named_reservation_stop(recovered: &RecoveredJobV1) -> Option<NamedReservationStop> {
    let metadata = recovered.metadata.as_ref()?;
    let supervision = metadata.checkpoint.supervised()?.supervision();
    let admission = supervision.legacy_named_admission()?;
    let first_stop = *recovered.cancel_command_ids.first()?;
    // Once any preparation or runtime operation is retained, STOP must
    // use its own recovery path. A lost receipt cannot restore this authority.
    if metadata.legacy_managed_seen
        || metadata.intent.legacy_named_plan() != Some(admission.plan())
        || admission.archive_digest().is_none()
        || admission.import_creation().is_some()
        || admission.import_arm().is_some()
        || admission
            .admission_result()
            .is_some_and(|result| !result.accepted)
        || !supervision.helpers().is_empty()
        || supervision
            .operations()
            .iter()
            .any(|operation| !matches!(operation, SupervisionOperationV1::Decision { .. }))
    {
        return None;
    }
    Some(NamedReservationStop {
        admission: admission.clone(),
        cancellation_id: reservation_cancellation_id(recovered.operation_id, first_stop),
    })
}

fn reservation_cancellation_id(operation: Uuid, first_stop: Uuid) -> String {
    let domain = format!("ai-guard:legacy-named-cancel-reservation-operation:v1:{operation}");
    crate::model::bound_event_id(domain.as_bytes(), first_stop)
}

#[cfg(test)]
#[test]
fn named_cancellation_replays_within_its_job_and_separates_jobs() {
    let stop = Uuid::from_u128(1);
    let original = Uuid::from_u128(2);
    let retained = reservation_cancellation_id(original, stop);
    assert_eq!(retained, reservation_cancellation_id(original, stop));
    assert_ne!(
        retained,
        reservation_cancellation_id(Uuid::from_u128(3), stop)
    );
    assert_ne!(
        retained,
        reservation_cancellation_id(original, Uuid::from_u128(4))
    );
}

/// Count every safely inspected entry even when its schema/name is invalid.
/// Failed metadata, truncation or unknown nested directories refuse mutation.
fn stop_budget_usage(core: &Inner, operation_id: Uuid) -> JournalResult<Usage> {
    let mut usage = Usage::default();
    let selected_name = operation_id.to_string();
    for entry in fscheck(core.directory.persistent_entries(core.limits.scope_entries))? {
        if entry.directory && entry.name != "jobs" {
            return Err(JournalError::UnsafeFilesystem);
        }
        usage.add(entry.bytes, 1, false, &core.limits)?;
    }
    for entry in fscheck(core.jobs.persistent_entries(core.limits.scope_entries))? {
        let selected = entry.name == selected_name;
        usage.add(entry.bytes, 1, selected, &core.limits)?;
        if !entry.directory {
            continue;
        }
        usage.jobs = usage
            .jobs
            .checked_add(1)
            .ok_or(JournalError::LimitExceeded)?;
        usage.add(0, 0, false, &core.limits)?;
        let mut job_usage = Usage::default();
        job_usage.add(0, 1, true, &core.limits)?;
        let mut revisions = 0_u32;
        let job = fscheck(core.jobs.persistent_child(&entry.name, false))?
            .ok_or(JournalError::UnsafeFilesystem)?;
        for file in fscheck(job.persistent_entries(core.limits.job_entries))? {
            usage.add(file.bytes, 1, selected, &core.limits)?;
            job_usage.add(file.bytes, 1, true, &core.limits)?;
            if file.name.starts_with("rev-") && file.name.ends_with(".json") {
                revisions = revisions
                    .checked_add(1)
                    .ok_or(JournalError::LimitExceeded)?;
                if revisions > core.limits.revisions {
                    return Err(JournalError::LimitExceeded);
                }
            }
            if file.directory {
                if file.name != "bindings" && file.name != crate::runtime::launch_profile::DIRECTORY
                {
                    return Err(JournalError::UnsafeFilesystem);
                }
                let bindings = fscheck(job.persistent_child(&file.name, false))?
                    .ok_or(JournalError::UnsafeFilesystem)?;
                for binding in fscheck(bindings.persistent_entries(core.limits.job_entries))? {
                    if binding.directory {
                        return Err(JournalError::UnsafeFilesystem);
                    }
                    usage.add(binding.bytes, 1, selected, &core.limits)?;
                    job_usage.add(binding.bytes, 1, true, &core.limits)?;
                }
            }
        }
    }
    Ok(usage)
}

pub(super) fn admit_report(
    report: &StopRecoveryReportV1,
    force_stops: bool,
) -> JournalResult<ExistingProfileV1> {
    report.reader.check_shared()?;
    if report
        .actor_claimed
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(JournalError::Busy);
    }
    let result = admit_claimed_report(report, force_stops);
    if result.is_err() {
        // Failure permits retry of this retained frame, preserving its existing
        // OnceLock/core/mutex. Success consumes the single actor factory.
        report.actor_claimed.store(false, Ordering::SeqCst);
    }
    result
}

fn admit_claimed_report(
    report: &StopRecoveryReportV1,
    force_stops: bool,
) -> JournalResult<ExistingProfileV1> {
    let core = Arc::clone(report.retained_core()?);
    let mut stops = StopJournal {
        core,
        scope_incomplete: report.incomplete,
    };
    if !force_stops && !report.incomplete {
        // The only internal factory creating a full facade from an existing
        // frame. Its lease is the same retained Arc; no flock/open/create.
        let full = Journal(Arc::clone(&stops.core));
        full.check_writer()?;
        if full.usage(None).is_ok() {
            // Existing-only equivalents of Journal::open's durable admission
            // barriers. The retained FD lease is never released or re-locked.
            fscheck(report.reader.namespace.durable_bootstrap())?;
            io(report.reader.lock.persistent_sync())?;
            let scope = report
                .reader
                .read_file(&report.reader.directory, "scope.json", SMALL)?
                .ok_or(JournalError::Corrupt)?;
            io(scope.persistent_sync())?;
            io(report.reader.directory.sync())?;
            io(report.reader.jobs.sync())?;
            io(report.reader.directory.sync())?;
            report.reader.check_shared()?;
            #[cfg(test)]
            full_namespace_durable()?;
            return Ok(ExistingProfileV1::Full(full));
        }
        stops.scope_incomplete = true;
    }
    Ok(ExistingProfileV1::Stops(stops))
}

impl StopRecoveryReportV1 {
    #[cfg(test)]
    pub(super) fn test_retained_core(&self) -> JournalResult<Arc<Inner>> {
        self.retained_core().map(Arc::clone)
    }
    fn retained_core(&self) -> JournalResult<&Arc<Inner>> {
        self.reader.check_shared()?;
        if self.observed_jobs.len() > self.reader.limits.jobs as usize {
            return Err(JournalError::LimitExceeded);
        }
        let mut seen = HashSet::new();
        if self.jobs.iter().any(|job| !seen.insert(job.operation_id)) {
            return Err(JournalError::Corrupt);
        }
        // All fallible anchor, cardinality and uniqueness validation precedes
        // OnceLock initialization. The closure only clones retained owners and
        // installs their single in-memory writer mutex; no open/flock or IO.
        Ok(self.retained_core.get_or_init(|| {
            let mut writer = Writer {
                observed_jobs: self.observed_jobs.clone(),
                rejected_floors: self.rejected_floors.clone(),
                ..Writer::default()
            };
            for job in &self.jobs {
                writer.floors.insert(job.operation_id, job.head.clone());
                writer.observed_jobs.insert(job.operation_id);
            }
            Arc::new(Inner {
                scope: self.reader.scope.clone(),
                scope_hash: self.reader.scope_hash.clone(),
                directory: self.reader.directory.clone(),
                jobs: self.reader.jobs.clone(),
                lock: self.reader.lock.clone(),
                exclusions: self.reader.exclusions.clone(),
                limits: self.reader.limits.clone(),
                control: self.reader.control.clone(),
                writer: Mutex::new(writer),
                budget_incomplete: AtomicBool::new(self.incomplete),
                status_observation_failed: AtomicBool::new(false),
            })
        }))
    }
    fn pre_factory_core(&self) -> JournalResult<Arc<Inner>> {
        if self.actor_claimed.load(Ordering::SeqCst) {
            return Err(JournalError::Busy);
        }
        self.retained_core().map(Arc::clone)
    }
    #[cfg(test)]
    pub(crate) fn finalize_retirement(
        &mut self,
        verified: crate::broker::enforcer::retirement::VerifiedRetirementReceipt,
    ) -> JournalResult<CheckpointReceiptV1> {
        self.finalize_retirement_retained(&verified)
    }
    pub(crate) fn retirement_factory_consumed(&self) -> bool {
        self.actor_claimed.load(Ordering::Acquire)
    }
    pub(crate) fn finalize_retirement_retained(
        &mut self,
        verified: &crate::broker::enforcer::retirement::VerifiedRetirementReceipt,
    ) -> JournalResult<CheckpointReceiptV1> {
        if self.actor_claimed.load(Ordering::SeqCst) {
            return Err(JournalError::Busy);
        }
        let operation = *verified.identity().job_operation_id();
        // A startup completion is restricted to the exact confirmed retirement
        // already present in this frame. It cannot discover/introduce a new P.
        let known = self
            .jobs
            .iter()
            .find(|job| job.operation_id == operation)
            .and_then(|job| {
                job.retirements.iter().find(|entry| {
                    entry.request().identity().retirement_id()
                        == verified.identity().retirement_id()
                })
            })
            .ok_or(JournalError::Conflict)?;
        if canonical(known.request().identity())? != canonical(verified.identity())?
            || known.request().audit_ack().is_none()
        {
            return Err(JournalError::Conflict);
        }
        let core = self.pre_factory_core()?;
        let stops = StopJournal {
            core,
            scope_incomplete: self.incomplete,
        };
        let receipt = stops
            .finalize_retirement(verified)
            .map_err(JournalError::from)?;
        let mut writer = stops
            .core
            .writer
            .lock()
            .map_err(|_| JournalError::Corrupt)?;
        let (_, mut recovered, incomplete) = stops
            .read_locked(operation, &mut writer)?
            .ok_or(JournalError::Corrupt)?;
        recovered.metadata = None;
        let slot = self
            .jobs
            .iter_mut()
            .find(|job| job.operation_id == operation)
            .ok_or(JournalError::Conflict)?;
        *slot = recovered;
        if incomplete {
            self.incomplete = true;
        }
        Ok(receipt)
    }

    /// Resolve only an already committed Final after a retained IO ambiguity.
    /// This shares the original reader/core/floors and never publishes a revision,
    /// admits a writer actor, queries the guardian or recreates a live witness.
    pub(crate) fn confirm_retirement_retained(
        &mut self,
        verified: &crate::broker::enforcer::retirement::VerifiedRetirementReceipt,
    ) -> JournalResult<CheckpointReceiptV1> {
        let operation = *verified.identity().job_operation_id();
        let known = self
            .jobs
            .iter()
            .find(|job| job.operation_id == operation)
            .and_then(|job| {
                job.retirements.iter().find(|entry| {
                    entry.request().identity().retirement_id()
                        == verified.identity().retirement_id()
                })
            })
            .ok_or(JournalError::Conflict)?;
        if canonical(known.request().identity())? != canonical(verified.identity())?
            || known.request().audit_ack().is_none()
        {
            return Err(JournalError::Conflict);
        }
        let stops = StopJournal {
            core: self.pre_factory_core()?,
            scope_incomplete: self.incomplete,
        };
        let mut writer = stops
            .core
            .writer
            .lock()
            .map_err(|_| JournalError::Corrupt)?;
        let (reader, mut recovered, incomplete) = stops
            .read_locked(operation, &mut writer)?
            .ok_or(JournalError::Conflict)?;
        let metadata = recovered.metadata.as_ref().ok_or(JournalError::Corrupt)?;
        let current = metadata
            .checkpoint
            .retirement()
            .ok_or(JournalError::Conflict)?;
        let entry = current
            .retirements()
            .iter()
            .find(|entry| {
                entry.request().identity().retirement_id() == verified.identity().retirement_id()
            })
            .ok_or(JournalError::Conflict)?;
        // An absent F is never permission to try the mutation again. Compare
        // the complete immutable P, then reuse the production Q/F checks.
        if entry.finalized().is_none() || canonical(entry.request())? != canonical(known.request())?
        {
            return Err(JournalError::Conflict);
        }
        let exact = crate::broker::journal::retirement::finalized_checkpoint(
            current,
            &metadata.intent,
            verified.identity(),
            verified.receipt().clone(),
            verified.head().clone(),
            verified.accepted().clone(),
        )?;
        if canonical(current)? != canonical(&exact)? {
            return Err(JournalError::Conflict);
        }
        reader.check_shared()?;
        let receipt = CheckpointReceiptV1 {
            operation_id: operation,
            head: recovered.head.clone(),
        };
        recovered.metadata = None;
        *self
            .jobs
            .iter_mut()
            .find(|job| job.operation_id == operation)
            .ok_or(JournalError::Conflict)? = recovered;
        self.incomplete |= incomplete;
        Ok(receipt)
    }

    pub fn scope(&self) -> &JournalScopeV1 {
        &self.reader.scope
    }
    pub fn jobs(&self) -> &[RecoveredJobV1] {
        &self.jobs
    }

    pub(in crate::broker) fn named_import_stop_jobs(&self) -> impl Iterator<Item = usize> + '_ {
        self.jobs
            .iter()
            .enumerate()
            .filter_map(|(index, job)| first_named_import_stop_receipt(job).map(|_| index))
    }

    /// Resolve the first confirmed STOP after this report's named Import Arm.
    /// Reread just the selected job under the same reader and check the
    /// retained prefix before projecting it across a later checkpoint ACK.
    pub(in crate::broker) fn named_import_stop_projection(
        &self,
        job_index: usize,
    ) -> JournalResult<Option<CancelStopProjectionV1>> {
        let retained = self.jobs.get(job_index).ok_or(JournalError::InvalidInput)?;
        let Some((arm_revision, receipt)) = first_named_import_stop_receipt(retained) else {
            return Ok(None);
        };

        let (recovered, rejected, partial) = self.reader.read_job(
            retained.operation_id(),
            &mut Usage::default(),
            &mut RecoveryBlobBytes::default(),
        )?;
        let (Some(recovered), None) = (recovered, rejected) else {
            return Err(JournalError::Conflict);
        };
        let floor =
            usize::try_from(retained.head().revision()).map_err(|_| JournalError::Corrupt)?;
        if recovered.chain.get(floor) != Some(retained.head())
            || recovered.named_import_arm_revision != Some(arm_revision)
        {
            return Err(JournalError::Conflict);
        }
        let projection = project_cancel(
            Arc::clone(&self.reader),
            recovered,
            self.incomplete || partial,
            receipt.command_id(),
        )?
        .ok_or(JournalError::Conflict)?;
        if projection.receipt().head() != receipt.head()
            || projection.receipt().input_hash() != receipt.input_hash()
        {
            return Err(JournalError::Conflict);
        }
        projection.revalidate()?;
        Ok(projection.named_import().is_some().then_some(projection))
    }

    /// Read the original Running attempt and claim under this report's lease,
    /// before an admitted writer can change the verified checkpoint.
    pub(in crate::broker) fn full3_worker_terminal_evidence(
        self: &Arc<Self>,
        scope: &JournalScopeV1,
        operation: Uuid,
        head: &RevisionRefV1,
        stop_command: Uuid,
        expected_binding: &Full3RegistrationBindingV1,
    ) -> JournalResult<Full3WorkerTerminalEvidenceV1> {
        let recovered = self.full3_worker_claim_before_stop(
            scope,
            operation,
            head,
            stop_command,
            expected_binding,
        )?;
        let evidence = Full3WorkerTerminalEvidenceV1 {
            report: Arc::clone(self),
            operation,
            head: head.clone(),
            directory: recovered.directory,
            target: recovered.target,
            binding: recovered.binding,
            stop: recovered.stop,
            worker: recovered.running_worker.ok_or(JournalError::Interrupted)?,
        };
        evidence.revalidate()?;
        Ok(evidence)
    }

    /// The original cold fixture also exercises a claim without a Running
    /// observation. Both paths use the same exact pre-STOP acknowledgement.
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    pub(crate) fn full3_worker_claim_before_stop_for_test(
        &self,
        scope: &JournalScopeV1,
        operation: Uuid,
        head: &RevisionRefV1,
        stop_command: Uuid,
        expected_binding: &Full3RegistrationBindingV1,
    ) -> JournalResult<(LocalCommandReceiptV1, ArmedStartClaimV1, MutationReceiptV1)> {
        let recovered = self.full3_worker_claim_before_stop(
            scope,
            operation,
            head,
            stop_command,
            expected_binding,
        )?;
        Ok((recovered.stop, recovered.claim, recovered.claim_receipt))
    }

    fn revalidate_full3_observation_lease(&self) -> JournalResult<()> {
        if self.incomplete
            || self.actor_claimed.load(Ordering::SeqCst)
            || self.retained_core.get().is_some()
        {
            return Err(JournalError::Interrupted);
        }
        self.revalidate()
    }

    fn full3_worker_claim_before_stop(
        &self,
        scope: &JournalScopeV1,
        operation: Uuid,
        head: &RevisionRefV1,
        stop_command: Uuid,
        expected_binding: &Full3RegistrationBindingV1,
    ) -> JournalResult<RecoveredFull3WorkerClaim> {
        self.revalidate_full3_observation_lease()?;
        if canonical(self.scope())? != canonical(scope)?
            || !self
                .jobs()
                .iter()
                .any(|job| job.operation_id() == operation && job.head() == head)
        {
            return Err(JournalError::Conflict);
        }
        let (Some(job), None, false) = self.reader.read_job(
            operation,
            &mut Usage::default(),
            &mut RecoveryBlobBytes::default(),
        )?
        else {
            return Err(JournalError::Interrupted);
        };
        if job.head() != head {
            return Err(JournalError::Conflict);
        }

        let mut targets = job
            .stops()
            .iter()
            .enumerate()
            .filter(|(_, target)| matches!(&target.arm().subject, ArmSubjectV1::Worker));
        let (target_index, target) = targets.next().ok_or(JournalError::Interrupted)?;
        let binding = target
            .full3_registration_binding()
            .ok_or(JournalError::Interrupted)?;
        if targets.next().is_some()
            || target.retirement().is_some()
            || target.cancel_after_arm() != Some(stop_command)
            || target.owned().handle.operation_id() != operation
            || target.owned().handle.id() != target.arm().inspection.container_id
            || binding.arm_id().to_string() != target.arm().operation_id
            || binding.ledger().installation_id() != &scope.installation_id()
            || canonical(binding)? != canonical(expected_binding)?
        {
            return Err(JournalError::Conflict);
        }
        let stop_prefix = job.cancel_prefix(target)?;
        let stop = stop_prefix
            .receipts()
            .iter()
            .find(|receipt| receipt.command_id() == stop_command)
            .ok_or(JournalError::Conflict)?;
        let metadata = job.metadata.as_ref().ok_or(JournalError::Corrupt)?;
        let v5 = metadata
            .checkpoint
            .retirement()
            .ok_or(JournalError::UnsupportedVersion)?;
        let operations = v5.outbox().lifecycle().broker().supervision().operations();
        let mut claims = operations
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches!(entry, SupervisionOperationV1::WorkerClaim { .. }));
        let (claim_index, claim) = claims.next().ok_or(JournalError::Interrupted)?;
        let SupervisionOperationV1::WorkerClaim {
            request,
            response: Some(response),
        } = claim
        else {
            return Err(JournalError::Interrupted);
        };
        if claims.next().is_some()
            || target.birth_index() >= claim_index
            || canonical(&request.arm_receipt)? != canonical(target.receipt())?
            || response.receipt.audit_seq <= target.receipt().audit_seq
            || response.worker.attempt.container_id.as_deref() != Some(target.owned().handle.id())
        {
            return Err(JournalError::Conflict);
        }

        // A claim can enter the chain pending before STOP and receive its ACK
        // later. Check the exact revision preceding STOP rather than the
        // operation count at the cancellation boundary.
        let stop_revision =
            usize::try_from(stop.head().revision()).map_err(|_| JournalError::Corrupt)?;
        let prefix = job
            .chain
            .get(..stop_revision)
            .ok_or(JournalError::Corrupt)?;
        let initial = prefix.first().ok_or(JournalError::Corrupt)?;
        let before_stop = prefix.last().ok_or(JournalError::Corrupt)?;
        let directory = fscheck(
            self.reader
                .jobs
                .persistent_child(&operation.to_string(), false),
        )?
        .ok_or(JournalError::Interrupted)?;
        let mut verifier = confirmed::ChainVerifierV1::new_with_launch_profile(
            self.scope(),
            &self.reader.control,
            &metadata.intent,
            &metadata.binding,
            initial,
            metadata.launch_binding.as_ref(),
        )?;
        for reference in prefix {
            let revision = reference.revision();
            let file = self
                .reader
                .read_file(&directory, &revision_name(revision), CHECKPOINT)?
                .ok_or(JournalError::Interrupted)?;
            verifier.push(revision, file.bytes())?;
        }
        let prefix = verifier.finish(before_stop)?;
        let prior = prefix
            .payload
            .retirement()
            .ok_or(JournalError::UnsupportedVersion)?;
        let prior_binding = prior
            .bindings()
            .iter()
            .find(|entry| entry.arm_id() == binding.arm_id())
            .ok_or(JournalError::Interrupted)?;
        let prior_operations = prior
            .outbox()
            .lifecycle()
            .broker()
            .supervision()
            .operations();
        let prior_claim = prior_operations
            .get(claim_index)
            .ok_or(JournalError::Interrupted)?;
        let before = prefix.payload.runner().broker_context();
        if canonical(prior_binding)? != canonical(binding)?
            || canonical(prior_claim)? != canonical(claim)?
            || before.binding != Some(target.authority())
        {
            return Err(JournalError::Conflict);
        }
        let current = metadata.checkpoint.runner().broker_context();
        let running_worker = match before.worker {
            Some(worker) if worker.state == WorkerStateV1::Running => {
                let observation = worker
                    .last_observation
                    .as_ref()
                    .filter(|observation| {
                        observation.status == RuntimeStatusV1::Running
                            && observation.running == Some(true)
                    })
                    .ok_or(JournalError::Conflict)?;
                let started_at = worker
                    .observed_started_at
                    .as_deref()
                    .filter(|start| {
                        chrono::DateTime::parse_from_rfc3339(start)
                            .is_ok_and(|time| time.timestamp() > 0)
                    })
                    .ok_or(JournalError::Conflict)?;
                if before.tenant_id != Some(scope.tenant_id())
                    || current.tenant_id != before.tenant_id
                    || current.binding != before.binding
                    || canonical(&before.owned)? != canonical(&Some(target.owned()))?
                    || canonical(&current.owned)? != canonical(&before.owned)?
                    || !worker.quota_held
                    || worker.allocation_inconsistent
                    || worker.admission_operation_id != operation.to_string()
                    || worker.worker_id != target.arm().worker_id
                    || worker.attempt.attempt_id != target.arm().attempt_id
                    || worker.attempt.created_inspection.as_ref() != Some(&target.arm().inspection)
                    || worker.attempt != response.worker.attempt
                    || worker.attempt.start_operation_id.as_deref()
                        != Some(request.operation_id.as_str())
                    || worker.attempt.start_emitter_id.as_deref()
                        != Some(request.emitter_id.as_str())
                    || worker.revision <= response.worker.revision
                    || observation.started_at.as_deref() != Some(started_at)
                    || canonical(&current.worker)? != canonical(&Some(worker))?
                {
                    return Err(JournalError::Conflict);
                }
                Some(worker.clone())
            }
            _ => None,
        };
        fscheck(directory.revalidate())?;
        self.revalidate_full3_observation_lease()?;
        let binding = binding.clone();
        #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
        let (claim, claim_receipt) = (request.clone(), response.receipt.clone());
        let stop = stop.clone();
        let target = job
            .stops
            .into_iter()
            .nth(target_index)
            .ok_or(JournalError::Corrupt)?;
        Ok(RecoveredFull3WorkerClaim {
            directory,
            target,
            binding,
            stop,
            running_worker,
            #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
            claim,
            #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
            claim_receipt,
        })
    }

    pub fn issues(&self) -> &[StopRecoveryIssueV1] {
        &self.issues
    }
    pub fn incomplete(&self) -> bool {
        self.incomplete
    }
    /// Recheck the retained lease and common anchors on the filesystem thread
    /// before handing off a recovered job and after restoring its stop targets.
    pub fn revalidate(&self) -> JournalResult<()> {
        self.reader.check_shared()
    }
    fn issue(
        &mut self,
        operation_id: Option<Uuid>,
        phase: StopRecoveryPhaseV1,
        error: JournalError,
    ) {
        self.incomplete = true;
        // Error memory is bounded independently of untrusted entry names.
        if self.issues.len() < self.reader.limits.scope_entries as usize {
            self.issues.push(StopRecoveryIssueV1 {
                operation_id,
                phase,
                error,
            });
        } else {
            self.issues_truncated = true;
        }
    }
}
impl RecoveredJobV1 {
    pub(crate) fn retirements(&self) -> &[crate::broker::retirement_checkpoint::RetirementEntryV1] {
        &self.retirements
    }
    pub(crate) fn cancel_prefix_for_stop(
        &self,
        index: usize,
    ) -> JournalResult<ConfirmedCancelPrefixV1> {
        self.cancel_prefix(self.stops.get(index).ok_or(JournalError::InvalidInput)?)
    }
    fn cancel_prefix(&self, stop: &RecoveredStopV1) -> JournalResult<ConfirmedCancelPrefixV1> {
        let mut receipts = Vec::new();
        // Order comes from exact references in the verified chain, never from
        // comparing revision numbers supplied by a peer or caller.
        let confirmed = self
            .chain
            .iter()
            .map(|head| (head.revision(), head.hash()))
            .collect::<HashSet<_>>();
        // The lifecycle command order is itself preserved by verified replay,
        // including two commands first confirmed in the same revision.
        for command in &self.cancel_command_ids {
            if self
                .cancel_boundaries
                .get(command)
                .is_some_and(|boundary| stop.birth_index < *boundary)
            {
                let receipt = self
                    .command_receipts
                    .get(command)
                    .ok_or(JournalError::Corrupt)?;
                if !confirmed.contains(&(receipt.head().revision(), receipt.head().hash())) {
                    return Err(JournalError::Corrupt);
                }
                receipts.push(receipt.clone());
            }
        }
        if receipts.len() > 64
            || receipts.len()
                != self
                    .cancel_boundaries
                    .iter()
                    .filter(|(_, boundary)| stop.birth_index < **boundary)
                    .count()
        {
            return Err(JournalError::Corrupt);
        }
        Ok(ConfirmedCancelPrefixV1 {
            head: self.head.clone(),
            receipts,
        })
    }
    pub(crate) fn cancel_receipt(&self, command: Uuid) -> Option<&LocalCommandReceiptV1> {
        self.command_receipts.get(&command)
    }
    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }
    pub fn head(&self) -> &RevisionRefV1 {
        &self.head
    }
    pub fn managed_workspace(&self) -> bool {
        self.managed_workspace
    }
    pub fn cancel_command_ids(&self) -> &[Uuid] {
        &self.cancel_command_ids
    }
    pub fn stops(&self) -> &[RecoveredStopV1] {
        &self.stops
    }
}
impl RecoveredStopV1 {
    pub(crate) fn full3_registration_binding(
        &self,
    ) -> Option<&crate::broker::retirement_checkpoint::Full3RegistrationBindingV1> {
        self.full3_binding.as_ref()
    }
    pub(crate) fn retirement(
        &self,
    ) -> Option<&crate::broker::retirement_checkpoint::RetirementEntryV1> {
        self.retirement.as_ref()
    }
    pub(crate) fn birth_index(&self) -> usize {
        self.birth_index
    }
    pub fn arm(&self) -> &ArmV1 {
        &self.arm
    }
    pub fn receipt(&self) -> &ArmReceiptV1 {
        &self.receipt
    }
    pub fn authority(&self) -> &RuntimeAuthority {
        &self.authority
    }
    pub fn owned(&self) -> &OwnedContainerSnapshot {
        &self.owned
    }
    /// The confirmed cancellation selecting this stop applies only to Arms
    /// born before its boundary (the last Cancel for a startup report).
    pub fn cancel_after_arm(&self) -> Option<Uuid> {
        self.cancel_after_arm
    }
}
impl StopRecoveryIssueV1 {
    pub fn operation_id(&self) -> Option<Uuid> {
        self.operation_id
    }
    pub fn phase(&self) -> StopRecoveryPhaseV1 {
        self.phase
    }
    pub fn error(&self) -> JournalError {
        self.error
    }
}

impl StopRecoveryReaderV1 {
    fn from_core(core: &Inner) -> JournalResult<Self> {
        // This namespace FD is an observation only. Job reads use the original
        // retained directory capabilities, revalidated before and after reads.
        let namespace_path = core
            .directory
            .path()
            .parent()
            .ok_or(JournalError::Corrupt)?;
        let namespace = fscheck(Directory::open_existing(namespace_path))?;
        let reader = Self {
            scope: core.scope.clone(),
            scope_hash: core.scope_hash.clone(),
            namespace,
            directory: core.directory.clone(),
            jobs: core.jobs.clone(),
            lock: core.lock.clone(),
            exclusions: core.exclusions.clone(),
            limits: core.limits.clone(),
            control: core.control.clone(),
            synchronize_reads: true,
        };
        reader.check_shared()?;
        Ok(reader)
    }
    pub fn open_existing(
        root: &Directory,
        scope: JournalScopeV1,
        exclusions: &FsExclusions,
        limits: JournalLimitsV1,
        control: ControlSecrets,
    ) -> JournalResult<Option<Self>> {
        scope.validate()?;
        let bytes = canonical(&scope)?;
        control.record(&bytes)?;
        if bytes.len() as u64 > SMALL
            || limits.scope_entries < 3
            || bytes.len() as u64 > limits.scope_bytes
        {
            return Err(JournalError::LimitExceeded);
        }
        fscheck(root.revalidate())?;
        let Some(namespace) = fscheck(root.persistent_child("broker-journal-v1", false))? else {
            return Ok(None);
        };
        let scope_hash = digest("ai-guard:broker-journal-scope:v1", &bytes);
        let directory = fscheck(namespace.persistent_child(&scope_hash, false))?
            .ok_or(JournalError::Conflict)?;
        // The same existing lease as Journal::open. Never create, replace or take over.
        let lock = fscheck(directory.persistent_read(".writer.lock", 0, &exclusions.0))?
            .ok_or(JournalError::Interrupted)?;
        if !fscheck(lock.persistent_try_lock())? {
            return Err(JournalError::Busy);
        }
        let jobs =
            fscheck(directory.persistent_child("jobs", false))?.ok_or(JournalError::Interrupted)?;
        let reader = Self {
            scope,
            scope_hash,
            namespace,
            directory,
            jobs,
            lock,
            exclusions: exclusions.clone(),
            limits,
            control,
            synchronize_reads: false,
        };
        reader.check_shared()?;
        Ok(Some(reader))
    }

    fn check_shared(&self) -> JournalResult<()> {
        fscheck(self.lock.persistent_revalidate())?;
        fscheck(self.namespace.revalidate())?;
        fscheck(self.directory.revalidate())?;
        fscheck(self.jobs.revalidate())?;
        let file = self
            .read_file(&self.directory, "scope.json", SMALL)?
            .ok_or(JournalError::Interrupted)?;
        self.control.record(file.bytes())?;
        let scope: JournalScopeV1 = parse(file.bytes())?;
        if canonical(&scope)? != file.bytes() || scope.0 != self.scope.0 {
            return Err(JournalError::Conflict);
        }
        Ok(())
    }

    pub fn scan(self) -> StopRecoveryReportV1 {
        let mut report = StopRecoveryReportV1 {
            reader: Arc::new(self),
            jobs: Vec::new(),
            issues: Vec::new(),
            incomplete: false,
            issues_truncated: false,
            observed_jobs: HashSet::new(),
            rejected_floors: HashMap::new(),
            actor_claimed: AtomicBool::new(false),
            retained_core: OnceLock::new(),
        };
        if let Err(error) = report.reader.check_shared() {
            report.issue(None, StopRecoveryPhaseV1::SharedScope, error);
            return report;
        }
        let mut usage = Usage::default();
        let mut blobs = RecoveryBlobBytes::default();
        let inventory = match report.reader.inventory_scopes(&mut usage) {
            Ok((entries, partial)) => {
                if partial {
                    report.issue(None, StopRecoveryPhaseV1::Inventory, JournalError::Corrupt);
                }
                entries
            }
            Err(error) => {
                report.issue(None, StopRecoveryPhaseV1::Inventory, error);
                return report;
            }
        };
        let mut ids = Vec::new();
        for entry in inventory {
            match Uuid::parse_str(&entry.name) {
                Ok(id) if entry.directory && !id.is_nil() && id.to_string() == entry.name => {
                    ids.push(id)
                }
                _ => report.issue(None, StopRecoveryPhaseV1::Inventory, JournalError::Corrupt),
            }
        }
        ids.sort_unstable();
        for operation_id in ids {
            if usage.jobs >= report.reader.limits.jobs {
                report.issue(
                    None,
                    StopRecoveryPhaseV1::Inventory,
                    JournalError::LimitExceeded,
                );
                break;
            }
            usage.jobs += 1;
            usage.job_bytes = 0;
            usage.job_entries = 0;
            blobs.next_job();
            report.observed_jobs.insert(operation_id);
            let recovered = report.reader.read_job(operation_id, &mut usage, &mut blobs);
            let partial_error = if blobs.exceeds_or_unknown(&usage, &report.reader.limits) {
                JournalError::LimitExceeded
            } else {
                JournalError::Interrupted
            };
            match recovered {
                Ok((Some(mut job), None, partial)) => {
                    // Startup reports retain stop facts, not an entire copy of
                    // every job's prompts, outbox bodies and runner payload.
                    job.metadata = None;
                    report.jobs.push(job);
                    if partial {
                        report.issue(Some(operation_id), StopRecoveryPhaseV1::Job, partial_error);
                    }
                }
                Ok((None, rejected_hash, partial)) => {
                    if let Some(hash) = rejected_hash {
                        report.rejected_floors.insert(operation_id, hash);
                    }
                    if partial {
                        report.issue(Some(operation_id), StopRecoveryPhaseV1::Job, partial_error);
                    }
                }
                Ok((Some(_), Some(_), _)) => {
                    report.issue(
                        Some(operation_id),
                        StopRecoveryPhaseV1::Job,
                        JournalError::Corrupt,
                    );
                }
                Err(error) => report.issue(Some(operation_id), StopRecoveryPhaseV1::Job, error),
            }
            if usage.bytes > report.reader.limits.scope_bytes
                || usage.entries > report.reader.limits.scope_entries
            {
                report.issue(
                    None,
                    StopRecoveryPhaseV1::Inventory,
                    JournalError::LimitExceeded,
                );
                break;
            }
        }
        if let Err(error) = report.reader.check_shared() {
            // No projection survives a lost shared anchor or lease.
            report.jobs.clear();
            report.issue(None, StopRecoveryPhaseV1::SharedScope, error);
        }
        report
    }

    fn inventory_scopes(
        &self,
        usage: &mut Usage,
    ) -> JournalResult<(Vec<crate::control_fs::persistent::Entry>, bool)> {
        let namespace = fscheck(
            self.namespace
                .persistent_inventory(self.limits.scope_entries),
        )?;
        let mut partial = namespace.incomplete;
        for entry in &namespace.entries {
            if !entry.directory || entry.name != self.scope_hash {
                partial = true;
            }
        }
        usage.add(0, namespace.visited, false, &self.limits)?;
        for entry in &namespace.entries {
            usage.add(entry.bytes, 0, false, &self.limits)?;
        }
        let directory = fscheck(
            self.directory
                .persistent_inventory(self.limits.scope_entries),
        )?;
        partial |= directory.incomplete;
        for entry in &directory.entries {
            if !matches!(entry.name.as_str(), ".writer.lock" | "scope.json" | "jobs")
                || entry.directory != (entry.name == "jobs")
            {
                partial = true;
            }
        }
        usage.add(0, directory.visited, false, &self.limits)?;
        for entry in &directory.entries {
            usage.add(entry.bytes, 0, false, &self.limits)?;
        }
        let jobs = fscheck(self.jobs.persistent_inventory(self.limits.scope_entries))?;
        partial |= jobs.incomplete;
        usage.add(0, jobs.visited, false, &self.limits)?;
        for entry in &jobs.entries {
            usage.add(entry.bytes, 0, false, &self.limits)?;
        }
        Ok((jobs.entries, partial))
    }

    fn read_file(
        &self,
        directory: &Directory,
        name: &str,
        max: u64,
    ) -> JournalResult<Option<PrivateFile>> {
        let file = fscheck(directory.persistent_read(name, max, &self.exclusions.0))?;
        if self.synchronize_reads
            && let Some(file) = &file
        {
            // Live delivery must not mistake a visible, unknown-outcome
            // metadata publication for a durable Cancel. No blob is read.
            io(file.persistent_sync())?;
        }
        Ok(file)
    }
    fn read_record<T: DeserializeOwned + Serialize>(
        &self,
        directory: &Directory,
        name: &str,
        max: u64,
    ) -> JournalResult<T> {
        let file = self
            .read_file(directory, name, max)?
            .ok_or(JournalError::Interrupted)?;
        self.control.record(file.bytes())?;
        let value: T = parse(file.bytes())?;
        if canonical(&value)? != file.bytes() {
            return Err(JournalError::Corrupt);
        }
        Ok(value)
    }

    fn read_job(
        &self,
        operation_id: Uuid,
        usage: &mut Usage,
        blobs: &mut RecoveryBlobBytes,
    ) -> JournalResult<(Option<RecoveredJobV1>, Option<String>, bool)> {
        self.check_shared()?;
        let job = fscheck(self.jobs.persistent_child(&operation_id.to_string(), false))?
            .ok_or(JournalError::Interrupted)?;
        let inventory = fscheck(job.persistent_inventory(self.limits.job_entries))?;
        let mut partial = inventory.incomplete;
        usage.add(0, 1, true, &self.limits)?;
        // Count rejected names as entries too. Unknown sizes make this incomplete,
        // never a claim that physical namespace usage has been qualified.
        usage.add(0, inventory.visited, true, &self.limits)?;
        let mut revision_count = 0_u32;
        let mut revisions = Vec::new();
        let mut binding_entries = None;
        let mut launch_entries = None;
        for entry in &inventory.entries {
            if entry.name == crate::runtime::launch_profile::DIRECTORY {
                // Also covers a launch parent substituted by a regular file.
                blobs.observe(entry.bytes);
            } else {
                usage.add(entry.bytes, 0, true, &self.limits)?;
            }
            if entry.directory {
                if entry.name != "bindings"
                    && entry.name != crate::runtime::launch_profile::DIRECTORY
                {
                    return Err(JournalError::Corrupt);
                }
                // Inspect credentials only by metadata. Never open any blob FD.
                match fscheck(job.persistent_child(&entry.name, false)) {
                    Ok(Some(bindings)) => {
                        match fscheck(bindings.persistent_inventory(self.limits.job_entries)) {
                            Ok(items) => {
                                partial |= items.incomplete;
                                usage.add(0, items.visited, true, &self.limits)?;
                                for item in &items.entries {
                                    blobs.observe(item.bytes);
                                    if item.directory {
                                        partial = true;
                                    }
                                }
                                if entry.name == crate::runtime::launch_profile::DIRECTORY {
                                    launch_entries = Some(items.entries);
                                } else {
                                    binding_entries = Some(items.entries);
                                }
                            }
                            Err(_) => partial = true,
                        }
                    }
                    _ => partial = true,
                }
            } else if let Some(number) = entry
                .name
                .strip_prefix("rev-")
                .and_then(|name| name.strip_suffix(".json"))
            {
                if number.len() != 20 || !number.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(JournalError::Corrupt);
                }
                let revision = number.parse::<u64>().map_err(|_| JournalError::Corrupt)?;
                revision_count = revision_count
                    .checked_add(1)
                    .ok_or(JournalError::LimitExceeded)?;
                revisions.push(revision);
            } else if entry.name == "current-next.json" {
                partial = true;
            } else if entry.name == crate::runtime::launch_profile::DIRECTORY {
                // A substituted blob parent is not a control record. Count its
                // metadata and retain the independently proven STOP projection.
                partial = true;
            } else if !matches!(
                entry.name.as_str(),
                "reservation.json"
                    | "intent.json"
                    | "bindings.json"
                    | "current.json"
                    | "accepted.json"
                    | "rejected.json"
            ) {
                return Err(JournalError::Corrupt);
            }
        }
        partial |= blobs.exceeds_or_unknown(usage, &self.limits);
        if revision_count > self.limits.revisions {
            return Err(JournalError::LimitExceeded);
        }
        let reservation: Reservation = self.read_record(&job, "reservation.json", SMALL)?;
        if reservation.operation_id != operation_id || reservation.scope_hash != self.scope_hash {
            return Err(JournalError::Corrupt);
        }
        valid_hash(&reservation.submit_hash).map_err(|_| JournalError::Corrupt)?;
        reservation.validate_submission()?;
        let accepted_file = self.read_file(&job, "accepted.json", SMALL)?;
        let rejected_file = self.read_file(&job, "rejected.json", SMALL)?;
        if accepted_file.is_some() && rejected_file.is_some() {
            return Err(JournalError::Corrupt);
        }
        let Some(accepted_file) = accepted_file else {
            if let Some(file) = rejected_file {
                self.control.record(file.bytes())?;
                let rejected: Rejected = parse(file.bytes())?;
                if rejected.scope_hash != self.scope_hash
                    || rejected.operation_id != operation_id
                    || rejected.submit_hash != reservation.submit_hash
                    || !rejected.matches_reservation_format(&reservation)
                    || canonical(&rejected)? != file.bytes()
                {
                    return Err(JournalError::Corrupt);
                }
                let rejected_hash = digest("ai-guard:broker-rejection:v1", file.bytes());
                fscheck(job.revalidate())?;
                if self.synchronize_reads {
                    io(job.sync())?;
                    io(self.jobs.sync())?;
                    io(self.directory.sync())?;
                }
                self.check_shared()?;
                return Ok((
                    None,
                    Some(rejected_hash),
                    partial
                        || inventory.entries.iter().any(|entry| {
                            !matches!(entry.name.as_str(), "reservation.json" | "rejected.json")
                        }),
                ));
            }
            return Ok((None, None, true));
        };
        self.control.record(accepted_file.bytes())?;
        let accepted: Accepted = parse(accepted_file.bytes())?;
        if !matches!(accepted.version, 1 | 2)
            || accepted.operation_id != operation_id
            || accepted.scope_hash != self.scope_hash
            || accepted.submit_hash != reservation.submit_hash
            || canonical(&accepted)? != accepted_file.bytes()
        {
            return Err(JournalError::Corrupt);
        }
        let intent_file = self
            .read_file(&job, "intent.json", 256 * 1024)?
            .ok_or(JournalError::Interrupted)?;
        self.control.record(intent_file.bytes())?;
        let intent = ExecutionIntentV1::from_bytes(intent_file.bytes())
            .map_err(|_| JournalError::Corrupt)?;
        let intent_hash = intent.intent_hash().map_err(|_| JournalError::Corrupt)?;
        if intent.operation_id() != operation_id
            || accepted.intent_hash != intent_hash
            || !reservation.matches_intent(&intent)
            || intent
                .canonical_bytes()
                .map_err(|_| JournalError::Corrupt)?
                != intent_file.bytes()
        {
            return Err(JournalError::Corrupt);
        }
        let binding_file = self
            .read_file(&job, "bindings.json", SMALL)?
            .ok_or(JournalError::Interrupted)?;
        self.control.record(binding_file.bytes())?;
        let bindings: Bindings = parse(binding_file.bytes())?;
        if bindings.version != accepted.version
            || bindings.operation_id != operation_id
            || bindings.intent_hash != intent_hash
            || accepted.bindings_hash != bindings.hash(binding_file.bytes())?
            || canonical(&bindings)? != binding_file.bytes()
        {
            return Err(JournalError::Corrupt);
        }
        bindings.validate_launch(accepted.version, &self.scope_hash, operation_id, &intent)?;
        partial |= match &bindings.docker_launch_profile {
            Some(_) => launch_entries.as_ref().is_none_or(|entries| {
                entries.len() != 1
                    || entries[0].directory
                    || entries[0].name != crate::runtime::launch_profile::FILE
                    || entries[0].bytes != 3
            }),
            None => launch_entries.is_some(),
        };
        validate_binding_metadata(&intent, &bindings.binding)?;
        // A valid expected blob alone does not make the credential directory
        // complete. Compare its entire metadata inventory with the manifest,
        // without opening any blob or discarding confirmed stop projections.
        partial |= match bindings.binding.kind() {
            BindingKindV1::None => binding_entries
                .as_ref()
                .is_some_and(|entries| !entries.is_empty()),
            BindingKindV1::Manual | BindingKindV1::Provider => {
                binding_entries.as_ref().is_none_or(|entries| {
                    entries.len() != 1
                        || entries[0].directory
                        || Some(entries[0].name.as_str()) != bindings.binding.file_name()
                })
            }
        };
        if bindings.binding.kind() != BindingKindV1::None {
            // Presence of a safe metadata entry is diagnostic only. No secret
            // contents, digest or credential validity are asserted by recovery.
            let name = bindings.binding.file_name().ok_or(JournalError::Corrupt)?;
            match fscheck(job.persistent_child("bindings", false)) {
                Ok(Some(directory)) => {
                    if !matches!(
                        directory.private_entry(std::ffi::OsStr::new(name)),
                        Ok(Some(_))
                    ) {
                        partial = true;
                    }
                }
                _ => partial = true,
            }
        }
        let current: Current = self.read_record(&job, "current.json", SMALL)?;
        if current.version != 1
            || current.operation_id != operation_id
            || current.intent_hash != intent_hash
            || current.head.revision >= u64::from(self.limits.revisions)
        {
            return Err(JournalError::Corrupt);
        }
        valid_hash(&current.head.hash).map_err(|_| JournalError::Corrupt)?;
        partial |= revisions
            .iter()
            .any(|revision| *revision > current.head.revision);
        let mut verifier = confirmed::ChainVerifierV1::new_with_launch_profile(
            &self.scope,
            &self.control,
            &intent,
            &bindings.binding,
            &accepted.initial_checkpoint,
            bindings.docker_launch_profile.as_ref(),
        )?;
        for revision in 0..=current.head.revision {
            let file = self
                .read_file(&job, &revision_name(revision), CHECKPOINT)?
                .ok_or(JournalError::Interrupted)?;
            verifier.push(revision, file.bytes())?;
        }
        let confirmed = verifier.finish(&current.head)?;
        // Historical V1 managed execution has no durable supervision Arm. A
        // later empty runner checkpoint cannot prove that execution has stopped.
        partial |= confirmed.legacy_managed_seen;
        let cancel_command_ids = confirmed
            .payload
            .lifecycle()
            .map_or_else(Vec::new, |value| {
                value
                    .commands()
                    .iter()
                    .filter(|command| {
                        matches!(command.request(), LocalCommandRequestV1::CancelRequested)
                    })
                    .map(|command| command.command_id())
                    .collect::<Vec<_>>()
            });
        let mut stops = Vec::new();
        let mut pending_arms = 0_usize;
        if let Some(value) = confirmed.payload.supervised() {
            if let Some(named) = value.supervision().legacy_named_admission() {
                let arm = named.import_arm();
                // A confirmed creation intent may have run. It and the
                // named Arm lack a native STOP target in this checkpoint.
                partial |= named.import_creation().is_some() || arm.is_some();
                pending_arms += usize::from(arm.is_some_and(|arm| arm.receipt().is_none()));
            }
            for (index, operation) in value.supervision().operations().iter().enumerate() {
                // WorkerClaim and HelperClaim already require a prior ACKed Arm
                // in V2 validation. Its older generic StartClaim mutation does
                // not: retain that uncovered execution as incomplete as well.
                // Created without any claim remains a known pre-start state.
                if let SupervisionOperationV1::WorkerMutation { request, .. } = operation
                    && matches!(request.transition, WorkerTransitionV1::StartClaim { .. })
                    && !value.supervision().operations()[..index]
                        .iter()
                        .any(|prior| {
                            matches!(prior, SupervisionOperationV1::Arm {
                            request: arm, response: Some(_), ..
                        } if matches!(arm.subject, ArmSubjectV1::Worker)
                            && arm.worker_id == request.worker_id
                            && arm.attempt_id == request.attempt_id)
                        })
                {
                    partial = true;
                }
                let SupervisionOperationV1::Arm {
                    owned,
                    request,
                    response,
                    ..
                } = operation
                else {
                    continue;
                };
                let Some(response) = response else {
                    pending_arms += 1;
                    partial = true;
                    continue;
                };
                if canonical(request)? != canonical(&response.arm)? {
                    return Err(JournalError::Corrupt);
                }
                response
                    .receipt
                    .validate_for_arm(request)
                    .map_err(|_| JournalError::Corrupt)?;
                let authority = confirmed
                    .payload
                    .runner()
                    .broker_context()
                    .binding
                    .ok_or(JournalError::Corrupt)?;
                if authority.invocation.is_none() {
                    return Err(JournalError::Corrupt);
                }
                let cancel_after_arm = match cancel_command_ids.last() {
                    Some(command_id) => {
                        let boundary = confirmed
                            .cancel_boundaries
                            .get(command_id)
                            .ok_or(JournalError::Corrupt)?;
                        (index < *boundary).then_some(*command_id)
                    }
                    None => None,
                };
                let retirement = confirmed
                    .payload
                    .retirement()
                    .and_then(|value| {
                        value.retirements().iter().find(|entry| {
                            entry.request().identity().arm().operation_id == request.operation_id
                        })
                    })
                    .cloned();
                if retirement
                    .as_ref()
                    .is_some_and(|entry| entry.finalized().is_some())
                {
                    continue;
                }
                let full3_binding = confirmed
                    .payload
                    .retirement()
                    .and_then(|value| {
                        value
                            .bindings()
                            .iter()
                            .find(|entry| entry.arm_id().to_string() == request.operation_id)
                    })
                    .cloned();
                stops.push(RecoveredStopV1 {
                    arm: request.clone(),
                    receipt: response.receipt.clone(),
                    authority: authority.clone(),
                    owned: owned.clone(),
                    cancel_after_arm,
                    birth_index: index,
                    full3_binding,
                    retirement,
                });
            }
        }
        fscheck(job.revalidate())?;
        if self.synchronize_reads {
            io(job.sync())?;
            io(self.jobs.sync())?;
            io(self.directory.sync())?;
        }
        self.check_shared()?;
        Ok((
            Some(RecoveredJobV1 {
                operation_id,
                head: current.head,
                managed_workspace: confirmed.payload.runner().broker_context().managed,
                cancel_command_ids,
                stops,
                chain: confirmed.chain,
                command_receipts: confirmed.command_receipts,
                outbox_receipts: confirmed.outbox_receipts,
                cancel_boundaries: confirmed.cancel_boundaries,
                pending_arms,
                named_import_arm_revision: confirmed.named_import_arm_revision,
                retirements: confirmed
                    .payload
                    .retirement()
                    .map_or_else(Vec::new, |value| value.retirements().to_vec()),
                metadata: Some(Box::new(StopMetadata {
                    intent,
                    binding: bindings.binding,
                    launch_binding: bindings.docker_launch_profile,
                    checkpoint: confirmed.payload,
                    legacy_managed_seen: confirmed.legacy_managed_seen,
                    last_cancel_operations: confirmed.last_cancel_operations,
                })),
            }),
            None,
            partial,
        ))
    }
}

/// Metadata-only counterpart of the checks in load_binding. It cannot qualify a
/// credential's contents and never calls a credential parser or opens its blob.
fn validate_binding_metadata(
    intent: &ExecutionIntentV1,
    binding: &BindingManifestV1,
) -> JournalResult<()> {
    let options = intent.initial_options().run_options();
    if binding.kind() == BindingKindV1::None {
        return if options.secret_env_file.is_none() && intent.provider_credential().is_none() {
            Ok(())
        } else {
            Err(JournalError::Corrupt)
        };
    }
    if binding.binding_id().is_none_or(|id| id.is_nil()) || binding.stored_bytes() == 0 {
        return Err(JournalError::Corrupt);
    }
    for value in [
        binding.source_sha(),
        binding.stored_sha(),
        binding.handoff_sha(),
    ] {
        valid_hash(value.ok_or(JournalError::Corrupt)?).map_err(|_| JournalError::Corrupt)?;
    }
    match binding.kind() {
        BindingKindV1::None => Err(JournalError::Corrupt),
        BindingKindV1::Manual => {
            if intent.provider_credential().is_some()
                || options.secret_env_file.is_none()
                || binding.file_name() != Some("manual-env-v1.bin")
                || binding.stored_bytes() > 64 * 1024
                || binding.stored_sha() != binding.handoff_sha()
                || binding.names().is_empty()
            {
                return Err(JournalError::Corrupt);
            }
            let mut names = HashSet::new();
            for name in binding.names() {
                if name.is_empty()
                    || name.len() > 128
                    || !names.insert(name)
                    || !name.bytes().all(|byte| {
                        byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_'
                    })
                    || name.as_bytes()[0].is_ascii_digit()
                    || [
                        "JEV_API_KEY",
                        "JEV_API_KEY_FILE",
                        "PATH",
                        "HOME",
                        "LD_PRELOAD",
                        "LD_LIBRARY_PATH",
                        "NODE_OPTIONS",
                        "PYTHONPATH",
                        "BASH_ENV",
                        "ENV",
                    ]
                    .contains(&name.as_str())
                {
                    return Err(JournalError::Corrupt);
                }
            }
            Ok(())
        }
        BindingKindV1::Provider => {
            let reference = intent.provider_credential().ok_or(JournalError::Corrupt)?;
            let resolution = intent
                .request_data()
                .provider_resolution
                .as_ref()
                .ok_or(JournalError::Corrupt)?;
            if options.secret_env_file.is_some()
                || binding.binding_id() != Some(reference.binding_id())
                || binding.file_name() != Some(reference.file_name())
                || binding.stored_sha() != Some(reference.digest())
                || binding.source_sha() != Some(reference.digest())
                || binding.stored_bytes() > 512 * 1024
                || binding.names() != [crate::provider::BOOTSTRAP_ENV]
                || binding.names() != resolution.credential_names
            {
                return Err(JournalError::Corrupt);
            }
            Ok(())
        }
    }
}
