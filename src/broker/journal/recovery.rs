//! Confirmed STOP metadata and restricted cancellation capabilities. The reader
//! is read-only; StopJournal can append only CancelRequested through the shared
//! publisher. Neither opens credential blobs or constructs a business job,
//! fresh-start permission, credential handoff or enforcer registration.
use super::*;
use crate::{
    broker::{checkpoint::SupervisionOperationV1, lifecycle::LocalCommandRequestV1},
    control::{ArmReceiptV1, ArmSubjectV1, ArmV1, WorkerTransitionV1},
    runtime::{OwnedContainerSnapshot, RuntimeAuthority},
};
use std::sync::atomic::{AtomicBool, Ordering};

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
    reader: StopRecoveryReaderV1,
    jobs: Vec<RecoveredJobV1>,
    issues: Vec<StopRecoveryIssueV1>,
    incomplete: bool,
    issues_truncated: bool,
    observed_jobs: HashSet<Uuid>,
    rejected_floors: HashMap<Uuid, String>,
    actor_claimed: AtomicBool,
}
pub struct RecoveredJobV1 {
    operation_id: Uuid,
    head: RevisionRefV1,
    managed_workspace: bool,
    cancel_command_ids: Vec<Uuid>,
    stops: Vec<RecoveredStopV1>,
    chain: Vec<RevisionRefV1>,
    command_receipts: HashMap<Uuid, LocalCommandReceiptV1>,
    cancel_boundaries: HashMap<Uuid, usize>,
    pending_arms: usize,
    metadata: Option<StopMetadata>,
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
}

/// A verified, immutable cancellation prefix. Its retained reader shares the
/// writer's existing lock FD; it neither acquires nor releases another lease.
/// No business job, credential bytes or start capability can be obtained here.
pub struct CancelStopProjectionV1 {
    reader: StopRecoveryReaderV1,
    operation_id: Uuid,
    command_id: Uuid,
    receipt: LocalCommandReceiptV1,
    stops: Vec<RecoveredStopV1>,
    cancel_prefixes: Vec<ConfirmedCancelPrefixV1>,
    incomplete: bool,
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
    reader: StopRecoveryReaderV1,
    operation_id: Uuid,
    head: RevisionRefV1,
    cancel_receipts: Vec<LocalCommandReceiptV1>,
    stops: Vec<RecoveredStopV1>,
    pending_arms: usize,
    incomplete: bool,
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

impl StopJobSnapshotV1 {
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
        reader.check_shared()?;
        writer.floors.insert(operation_id, recovered.head.clone());
        writer.observed_jobs.insert(operation_id);
        Ok(Some((
            reader,
            recovered,
            partial || scope_partial || self.scope_incomplete,
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
        project_cancel(reader, recovered, incomplete, command_id)
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
            reader,
            operation_id,
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
            let result = project_cancel(reader, recovered, incomplete, command_id)?
                .ok_or(JournalError::Conflict)?;
            #[cfg(test)]
            stop_before_ack()?;
            return Ok(result);
        }
        let metadata = recovered.metadata.as_ref().ok_or(JournalError::Corrupt)?;
        let id = command_id.to_string();
        if metadata.checkpoint.outbox().is_some_and(|cp| cp.events().iter().any(|event| event.event_id() == command_id))
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
        let next = match metadata.checkpoint.outbox() {
            Some(cp) => {
                LoadedCheckpoint::V4(BrokerCheckpointV4::new(lifecycle, cp.events().to_vec())?)
            }
            None => LoadedCheckpoint::V3(lifecycle),
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
            usage,
        )?;
        writer.floors.insert(operation_id, head.clone());
        writer.observed_jobs.insert(operation_id);
        let (reader, confirmed, incomplete) = self
            .read_locked(operation_id, &mut writer)?
            .ok_or(JournalError::Corrupt)?;
        if confirmed.head != head {
            return Err(JournalError::Corrupt);
        }
        let result = project_cancel(reader, confirmed, incomplete, command_id)?
            .ok_or(JournalError::Corrupt)?;
        #[cfg(test)]
        stop_before_ack()?;
        Ok(result)
    }
}

fn project_cancel(
    reader: StopRecoveryReaderV1,
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
    let cancel_prefixes = recovered
        .stops
        .iter()
        .filter(|stop| stop.birth_index < boundary)
        .map(|stop| recovered.cancel_prefix(stop))
        .collect::<JournalResult<Vec<_>>>()?;
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
        incomplete,
    }))
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
        // No core escaped failed admission. Success consumes this frame: a
        // second factory cannot create another mutex/actor on the same FD.
        report.actor_claimed.store(false, Ordering::SeqCst);
    }
    result
}

fn admit_claimed_report(
    report: &StopRecoveryReportV1,
    force_stops: bool,
) -> JournalResult<ExistingProfileV1> {
    let mut writer = Writer {
        observed_jobs: report.observed_jobs.clone(),
        rejected_floors: report.rejected_floors.clone(),
        ..Writer::default()
    };
    for job in &report.jobs {
        if writer
            .floors
            .insert(job.operation_id, job.head.clone())
            .is_some()
        {
            return Err(JournalError::Corrupt);
        }
        writer.observed_jobs.insert(job.operation_id);
    }
    if writer.observed_jobs.len() > report.reader.limits.jobs as usize {
        return Err(JournalError::LimitExceeded);
    }
    let core = Arc::new(Inner {
        scope: report.reader.scope.clone(),
        scope_hash: report.reader.scope_hash.clone(),
        directory: report.reader.directory.clone(),
        jobs: report.reader.jobs.clone(),
        lock: report.reader.lock.clone(),
        exclusions: report.reader.exclusions.clone(),
        limits: report.reader.limits.clone(),
        control: report.reader.control.clone(),
        writer: Mutex::new(writer),
    });
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
    pub fn scope(&self) -> &JournalScopeV1 {
        &self.reader.scope
    }
    pub fn jobs(&self) -> &[RecoveredJobV1] {
        &self.jobs
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
            reader: self,
            jobs: Vec::new(),
            issues: Vec::new(),
            incomplete: false,
            issues_truncated: false,
            observed_jobs: HashSet::new(),
            rejected_floors: HashMap::new(),
            actor_claimed: AtomicBool::new(false),
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
        if reservation.version != 1
            || reservation.operation_id != operation_id
            || reservation.scope_hash != self.scope_hash
        {
            return Err(JournalError::Corrupt);
        }
        valid_hash(&reservation.submit_hash).map_err(|_| JournalError::Corrupt)?;
        let accepted_file = self.read_file(&job, "accepted.json", SMALL)?;
        let rejected_file = self.read_file(&job, "rejected.json", SMALL)?;
        if accepted_file.is_some() && rejected_file.is_some() {
            return Err(JournalError::Corrupt);
        }
        let Some(accepted_file) = accepted_file else {
            if let Some(file) = rejected_file {
                self.control.record(file.bytes())?;
                let rejected: Rejected = parse(file.bytes())?;
                if rejected.version != 1
                    || rejected.scope_hash != self.scope_hash
                    || rejected.operation_id != operation_id
                    || rejected.submit_hash != reservation.submit_hash
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
                stops.push(RecoveredStopV1 {
                    arm: request.clone(),
                    receipt: response.receipt.clone(),
                    authority: authority.clone(),
                    owned: owned.clone(),
                    cancel_after_arm,
                    birth_index: index,
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
                cancel_boundaries: confirmed.cancel_boundaries,
                pending_arms,
                metadata: Some(StopMetadata {
                    intent,
                    binding: bindings.binding,
                    launch_binding: bindings.docker_launch_profile,
                    checkpoint: confirmed.payload,
                    legacy_managed_seen: confirmed.legacy_managed_seen,
                    last_cancel_operations: confirmed.last_cancel_operations,
                }),
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
