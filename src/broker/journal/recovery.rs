//! Read-only recovery of confirmed stop data. This never constructs a business
//! job, a start permission, a credential handoff or an enforcer registration.
use super::*;
use crate::{
    broker::{checkpoint::SupervisionOperationV1, lifecycle::LocalCommandRequestV1},
    control::{ArmReceiptV1, ArmSubjectV1, ArmV1, WorkerTransitionV1},
    runtime::{OwnedContainerSnapshot, RuntimeAuthority},
};

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
}
/// Retains the existing exclusive journal lease until the report is dropped.
/// Restore its stop data before attempting to open the ordinary writer.
pub struct StopRecoveryReportV1 {
    reader: StopRecoveryReaderV1,
    jobs: Vec<RecoveredJobV1>,
    issues: Vec<StopRecoveryIssueV1>,
    incomplete: bool,
}
pub struct RecoveredJobV1 {
    operation_id: Uuid,
    head: RevisionRefV1,
    managed_workspace: bool,
    cancel_command_ids: Vec<Uuid>,
    stops: Vec<RecoveredStopV1>,
}
pub struct RecoveredStopV1 {
    arm: ArmV1,
    receipt: ArmReceiptV1,
    authority: RuntimeAuthority,
    owned: OwnedContainerSnapshot,
    cancel_after_arm: Option<Uuid>,
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
    StopRecoveryIssueV1
);

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
        }
    }
}
impl RecoveredJobV1 {
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
    /// The last confirmed cancellation only applies to arms born before it.
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
        };
        if let Err(error) = report.reader.check_shared() {
            report.issue(None, StopRecoveryPhaseV1::SharedScope, error);
            return report;
        }
        let mut usage = Usage::default();
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
            match report.reader.read_job(operation_id, &mut usage) {
                Ok((Some(job), partial)) => {
                    report.jobs.push(job);
                    if partial {
                        report.issue(
                            Some(operation_id),
                            StopRecoveryPhaseV1::Job,
                            JournalError::Interrupted,
                        );
                    }
                }
                Ok((None, partial)) => {
                    if partial {
                        report.issue(
                            Some(operation_id),
                            StopRecoveryPhaseV1::Job,
                            JournalError::Interrupted,
                        );
                    }
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
        fscheck(directory.persistent_read(name, max, &self.exclusions.0))
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
    ) -> JournalResult<(Option<RecoveredJobV1>, bool)> {
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
        for entry in &inventory.entries {
            usage.add(entry.bytes, 0, true, &self.limits)?;
            if entry.directory {
                if entry.name != "bindings" {
                    return Err(JournalError::Corrupt);
                }
                // Inspect credentials only by metadata. Never open any blob FD.
                match fscheck(job.persistent_child("bindings", false)) {
                    Ok(Some(bindings)) => {
                        match fscheck(bindings.persistent_inventory(self.limits.job_entries)) {
                            Ok(items) => {
                                partial |= items.incomplete;
                                usage.add(0, items.visited, true, &self.limits)?;
                                for item in &items.entries {
                                    usage.add(item.bytes, 0, true, &self.limits)?;
                                    if item.directory {
                                        partial = true;
                                    }
                                }
                                binding_entries = Some(items.entries);
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
                return Ok((
                    None,
                    partial
                        || inventory.entries.iter().any(|entry| {
                            !matches!(entry.name.as_str(), "reservation.json" | "rejected.json")
                        }),
                ));
            }
            return Ok((None, true));
        };
        self.control.record(accepted_file.bytes())?;
        let accepted: Accepted = parse(accepted_file.bytes())?;
        if accepted.version != 1
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
        if bindings.version != 1
            || bindings.operation_id != operation_id
            || bindings.intent_hash != intent_hash
            || accepted.bindings_hash != digest("ai-guard:broker-bindings:v1", binding_file.bytes())
            || canonical(&bindings)? != binding_file.bytes()
        {
            return Err(JournalError::Corrupt);
        }
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
        let mut verifier = confirmed::ChainVerifierV1::new(
            &self.scope,
            &self.control,
            &intent,
            &bindings.binding,
            &accepted.initial_checkpoint,
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
                let cancel_after_arm = if confirmed
                    .last_cancel_operations
                    .is_some_and(|boundary| index < boundary)
                {
                    Some(*cancel_command_ids.last().ok_or(JournalError::Corrupt)?)
                } else {
                    None
                };
                stops.push(RecoveredStopV1 {
                    arm: request.clone(),
                    receipt: response.receipt.clone(),
                    authority: authority.clone(),
                    owned: owned.clone(),
                    cancel_after_arm,
                });
            }
        }
        fscheck(job.revalidate())?;
        self.check_shared()?;
        Ok((
            Some(RecoveredJobV1 {
                operation_id,
                head: current.head,
                managed_workspace: confirmed.payload.runner().broker_context().managed,
                cancel_command_ids,
                stops,
            }),
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
