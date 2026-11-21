use super::*;
use crate::{
    control::*,
    runtime::{
        ContainerHandle, ContainerObservation, InvocationBindingV1, ManagedRuntime,
        OwnedContainerSnapshot, RuntimeAuthority, StartDirective,
    },
};
use std::sync::{Arc, Mutex};

#[path = "control_monitor.rs"]
mod control_monitor;
#[cfg(all(test, unix))]
mod stop_replay_tests;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManagedCheckpoint {
    pub tenant_id: String,
    pub gateway_origin: String,
    pub config_root: PathBuf,
    pub binding: Option<RuntimeAuthority>,
    pub authority_file: String,
    pub admission: AdmissionV1,
    pub worker: Option<WorkerV1>,
    pub owned: Option<OwnedContainerSnapshot>,
    pub pending: Option<WorkerMutationV1>,
    pub uncertain: Vec<WorkerMutationV1>,
    pub lock_identity: Option<LockIdentity>,
    pub before: Manifest,
    pub import: Option<HelperCheckpoint>,
    pub decision: Value,
    pub output_event: Option<Value>,
    pub output_acknowledged: bool,
    pub result: Option<RunSummary>,
    pub archive_name: String,
    pub archive_digest: String,
    pub credential_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_control_exclusion_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation_event: Option<Value>,
}

const MAX_FILE_EVENTS: usize = 200;
const MAX_RETAINED_FILE_EVENT_BYTES: usize = 120 * 1024;
const MAX_POSTPROCESSING_BYTES: usize = 3 * 1024 * 1024;
// Reserve growth for coverage, delivery diagnostics and summary projection.
// File selection measures the complete journal, rather than path byte lengths.
const POSTPROCESSING_GROWTH_RESERVE: usize = 128 * 1024;
const MAX_SUMMARY_DIAGNOSTIC_BYTES: usize = 8192 + 32;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DeclaredCollectionV1 {
    Pending,
    Complete,
    Partial,
    UnavailableAfterInterruption,
    UnavailableForTty,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredDeliveryV1 {
    state: DeclaredCollectionV1,
    normalized_events: Option<usize>,
    acknowledged_events: Option<usize>,
    dropped_events: Option<usize>,
    unknown_delivery_events: Option<usize>,
    not_attempted_events: Option<usize>,
    stdout_truncated: Option<bool>,
    unterminated_final_line_omitted: Option<bool>,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedDiffV1 {
    observation_id: Uuid,
    before_digest: String,
    after_digest: String,
    changed_files: usize,
    files: Vec<Value>,
    acknowledged_files: usize,
    completed: Value,
    completed_acknowledged: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PostprocessingV1 {
    version: u32,
    declared: DeclaredDeliveryV1,
    diff: Option<PreparedDiffV1>,
    diff_error: Option<String>,
    coverage_revision: u32,
    coverage: Option<Value>,
    coverage_acknowledged: bool,
    coverage_error: Option<String>,
    previous_delivery_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    summary_projection: Option<SummaryProjectionV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SummaryProjectionV1 {
    version: u32,
    previous_digest: String,
    intended: Option<String>,
    external: Option<String>,
    acknowledged: bool,
}

/// Separate from strict RunPlan1/2/3. This journal records delivery, never a
/// runtime start right or a terminal fact. The caller holds the attempt lock.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PostprocessingJournalV1 {
    version: u32,
    tenant_id: String,
    gateway_origin: String,
    session_id: Uuid,
    admission_operation_id: Uuid,
    worker_id: String,
    attempt_id: String,
    spec_hash: String,
    before_manifest_digest: String,
    output_event_digest: Option<String>,
    payload: PostprocessingV1,
    payload_digest: String,
}

fn postprocessing_name(state: &PlanState) -> Result<String> {
    Ok(format!(
        "{}.postprocessing-v1.json",
        state.snapshot()?.session
    ))
}
fn validate_postprocessing_journal(
    journal: &PostprocessingJournalV1,
    plan: &RunPlan,
    client: &AuditClient,
) -> Result<()> {
    let checkpoint = managed(plan)?;
    let worker = checkpoint
        .worker
        .as_ref()
        .context("postprocessing journal lacks its admitted worker")?;
    anyhow::ensure!(
        journal.version == 1
            && journal.tenant_id == checkpoint.tenant_id
            && journal.gateway_origin == checkpoint.gateway_origin
            && journal.session_id == plan.session
            && journal.admission_operation_id == plan.operation_id
            && journal.worker_id == worker.worker_id
            && journal.attempt_id == worker.attempt.attempt_id
            && journal.spec_hash == worker.spec_hash
            && journal.before_manifest_digest
                == content_hash("ai-guard:workspace-manifest:v1", &checkpoint.before)?
            && journal.payload_digest
                == content_hash("ai-guard:postprocessing-payload:v1", &journal.payload)?,
        "postprocessing journal identity or checksum conflicts with the original plan"
    );
    if let Some(value) = &checkpoint.output_event {
        anyhow::ensure!(
            journal.output_event_digest.as_deref()
                == Some(content_hash("ai-guard:postprocessing-output:v1", value)?.as_str()),
            "postprocessing journal output differs from the immutable terminal event"
        );
    }
    if let Some(digest) = &journal.output_event_digest {
        hash(digest)?;
    }
    journal.payload.validate(plan, client)?;
    anyhow::ensure!(
        serde_json::to_vec(journal)?.len() <= MAX_POSTPROCESSING_BYTES,
        "postprocessing journal exceeds its bound"
    );
    Ok(())
}
fn load_postprocessing(
    state: &PlanState,
    client: &AuditClient,
) -> Result<Option<PostprocessingJournalV1>> {
    let journal: Option<PostprocessingJournalV1> = state
        .directory
        .read_bounded(&postprocessing_name(state)?, MAX_POSTPROCESSING_BYTES)?;
    if let Some(journal) = &journal {
        validate_postprocessing_journal(journal, &state.snapshot()?, client)?;
    }
    Ok(journal)
}
fn create_postprocessing(
    state: &PlanState,
    client: &AuditClient,
    payload: PostprocessingV1,
    output: Option<&Value>,
) -> Result<()> {
    anyhow::ensure!(
        load_postprocessing(state, client)?.is_none(),
        "postprocessing journal already exists; resume its exact facts"
    );
    let plan = state.snapshot()?;
    let checkpoint = managed(&plan)?;
    let worker = checkpoint
        .worker
        .as_ref()
        .context("postprocessing journal requires worker admission")?;
    let journal = PostprocessingJournalV1 {
        version: 1,
        tenant_id: checkpoint.tenant_id.clone(),
        gateway_origin: checkpoint.gateway_origin.clone(),
        session_id: plan.session,
        admission_operation_id: plan.operation_id,
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        spec_hash: worker.spec_hash.clone(),
        before_manifest_digest: content_hash("ai-guard:workspace-manifest:v1", &checkpoint.before)?,
        output_event_digest: output
            .map(|value| content_hash("ai-guard:postprocessing-output:v1", value))
            .transpose()?,
        payload_digest: content_hash("ai-guard:postprocessing-payload:v1", &payload)?,
        payload,
    };
    validate_postprocessing_journal(&journal, &plan, client)?;
    state
        .directory
        .write(&postprocessing_name(state)?, &journal)
}
fn update_postprocessing(
    state: &PlanState,
    client: &AuditClient,
    change: impl FnOnce(&mut PostprocessingV1) -> Result<()>,
) -> Result<()> {
    let mut journal =
        load_postprocessing(state, client)?.context("postprocessing journal missing")?;
    change(&mut journal.payload)?;
    journal.payload_digest = content_hash("ai-guard:postprocessing-payload:v1", &journal.payload)?;
    validate_postprocessing_journal(&journal, &state.snapshot()?, client)?;
    state
        .directory
        .write(&postprocessing_name(state)?, &journal)
}

impl PostprocessingV1 {
    fn pending(
        interactive: bool,
        capture: Option<&crate::adapters::NormalizedCapture>,
        truncated: Option<bool>,
    ) -> Self {
        Self {
            version: 1,
            declared: DeclaredDeliveryV1 {
                state: if interactive {
                    DeclaredCollectionV1::UnavailableForTty
                } else {
                    DeclaredCollectionV1::Pending
                },
                normalized_events: capture.map(|capture| capture.events.len()),
                acknowledged_events: None,
                dropped_events: None,
                unknown_delivery_events: None,
                not_attempted_events: None,
                stdout_truncated: truncated,
                unterminated_final_line_omitted: capture
                    .map(|capture| capture.unterminated_final_line_omitted),
                error: None,
            },
            diff: None,
            diff_error: None,
            coverage_revision: 0,
            coverage: None,
            coverage_acknowledged: false,
            coverage_error: None,
            previous_delivery_error: None,
            summary_projection: None,
        }
    }
    fn needs_recovery(&self) -> bool {
        self.diff
            .as_ref()
            .is_none_or(|diff| !diff.completed_acknowledged)
            || self.declared.state == DeclaredCollectionV1::Pending
            || !self.coverage_acknowledged
    }
    fn validate(&self, plan: &RunPlan, client: &AuditClient) -> Result<()> {
        anyhow::ensure!(
            self.version == 1 && serde_json::to_vec(self)?.len() <= MAX_POSTPROCESSING_BYTES,
            "postprocessing checkpoint version or size is invalid"
        );
        let declared = &self.declared;
        for count in [
            declared.normalized_events,
            declared.acknowledged_events,
            declared.dropped_events,
            declared.unknown_delivery_events,
            declared.not_attempted_events,
        ]
        .into_iter()
        .flatten()
        {
            anyhow::ensure!(
                count <= crate::adapters::MAX_EVENTS_PER_CAPTURE,
                "declared event count exceeds the capture bound"
            );
        }
        if let (Some(total), Some(ack), Some(dropped), Some(unknown)) = (
            declared.normalized_events,
            declared.acknowledged_events,
            declared.dropped_events,
            declared.unknown_delivery_events,
        ) {
            anyhow::ensure!(
                ack.checked_add(dropped)
                    .and_then(|count| count.checked_add(unknown))
                    == Some(total),
                "declared event delivery counts conflict"
            );
        }
        if declared.state == DeclaredCollectionV1::Complete {
            anyhow::ensure!(
                declared.normalized_events == declared.acknowledged_events
                    && declared.dropped_events == Some(0)
                    && declared.unknown_delivery_events == Some(0)
                    && declared.not_attempted_events == Some(0)
                    && declared.error.is_none(),
                "complete declared capture conflicts with its counters"
            );
        }
        let counters_unknown = [
            declared.acknowledged_events,
            declared.dropped_events,
            declared.unknown_delivery_events,
            declared.not_attempted_events,
        ]
        .iter()
        .all(Option::is_none);
        match declared.state {
            DeclaredCollectionV1::Pending => anyhow::ensure!(
                counters_unknown && declared.error.is_none(),
                "pending capture contains final delivery counters"
            ),
            DeclaredCollectionV1::UnavailableAfterInterruption => anyhow::ensure!(
                counters_unknown && declared.error.is_some(),
                "interrupted capture must retain unknown delivery counts"
            ),
            DeclaredCollectionV1::UnavailableForTty => anyhow::ensure!(
                counters_unknown
                    && declared.normalized_events.is_none()
                    && declared.error.is_none(),
                "TTY capture must remain unavailable"
            ),
            DeclaredCollectionV1::Partial => anyhow::ensure!(
                declared.normalized_events.is_some()
                    && declared.acknowledged_events.is_some()
                    && declared.dropped_events.is_some()
                    && declared
                        .unknown_delivery_events
                        .is_some_and(|count| count <= 1)
                    && declared
                        .not_attempted_events
                        .zip(declared.dropped_events)
                        .is_some_and(|(unattempted, dropped)| unattempted <= dropped)
                    && declared.error.is_some(),
                "partial capture counters or failure are incomplete"
            ),
            DeclaredCollectionV1::Complete => {}
        }
        if let Some(diff) = &self.diff {
            anyhow::ensure!(
                !diff.observation_id.is_nil()
                    && diff.before_digest
                        == content_hash("ai-guard:workspace-manifest:v1", &managed(plan)?.before)?
                    && diff.files.len() <= diff.changed_files.min(MAX_FILE_EVENTS)
                    && diff.acknowledged_files <= diff.files.len()
                    && (!diff.completed_acknowledged
                        || diff.acknowledged_files == diff.files.len())
                    && diff.changed_files <= Limits::default().max_files.saturating_mul(2),
                "prepared diff conflicts with its manifest, count or acknowledgements"
            );
            hash(&diff.after_digest)?;
            let mut ids = std::collections::HashSet::new();
            let mut displays = std::collections::BTreeMap::<String, (usize, bool)>::new();
            for value in &diff.files {
                let event =
                    validate_retained_event(plan, client, value, MAX_RETAINED_FILE_EVENT_BYTES)?;
                anyhow::ensure!(
                    ["file.created", "file.modified", "file.deleted"]
                        .contains(&event.kind.as_str())
                        && event.data["observation_id"] == diff.observation_id.to_string()
                        && event.data["path"].is_string()
                        && ids.insert(event.id),
                    "prepared file event kind or identity conflicts"
                );
                let path = event.data["path"]
                    .as_str()
                    .context("prepared display path missing")?;
                let display = displays.entry(path.to_owned()).or_default();
                display.0 += 1;
                display.1 |= event.data["path_redacted"] == true;
                let fields = [
                    event.data.get("change_id"),
                    event.data.get("path_redacted"),
                    event.data.get("path_display_ambiguous"),
                ];
                anyhow::ensure!(
                    fields.iter().all(Option::is_none)
                        || (event.data["change_id"] == value["id"]
                            && event.data["path_redacted"].is_boolean()
                            && event.data["path_display_ambiguous"].is_boolean()),
                    "prepared opaque change identity or display metadata is invalid"
                );
            }
            for value in &diff.files {
                let display = displays
                    .get(
                        value["data"]["path"]
                            .as_str()
                            .context("prepared display path missing")?,
                    )
                    .context("prepared display group missing")?;
                let repeated = display.0 > 1;
                anyhow::ensure!(
                    !repeated || display.1,
                    "repeated display path has no redacted source"
                );
                let marker = value["data"].get("path_display_ambiguous");
                anyhow::ensure!(
                    marker.map_or(!repeated, |marker| marker.as_bool() == Some(repeated)),
                    "prepared display path ambiguity is not declared"
                );
            }
            let completed = validate_retained_event(plan, client, &diff.completed, 16 * 1024)?;
            anyhow::ensure!(
                completed.kind == "workspace.diff.completed"
                    && ids.insert(completed.id)
                    && completed.data["changed_files"] == diff.changed_files
                    && completed.data["emitted_changes"] == diff.files.len()
                    && completed.data["observation_id"] == diff.observation_id.to_string()
                    && completed.data["before_manifest_digest"] == diff.before_digest
                    && completed.data["after_manifest_digest"] == diff.after_digest
                    && completed.data["truncated"] == (diff.changed_files > diff.files.len())
                    && completed
                        .data
                        .get("omitted_changes")
                        .is_none_or(|omitted| *omitted == diff.changed_files - diff.files.len()),
                "prepared diff completion conflicts with its file batch"
            );
        }
        anyhow::ensure!(
            !self.coverage_acknowledged || self.coverage.is_some(),
            "coverage acknowledgement lacks its exact event"
        );
        if let Some(value) = &self.coverage {
            let event = validate_retained_event(plan, client, value, 16 * 1024)?;
            anyhow::ensure!(
                event.kind == "observer.coverage"
                    && self.coverage_revision > 0
                    && event.data["revision"] == self.coverage_revision,
                "retained coverage revision conflicts"
            );
        }
        for error in [
            &declared.error,
            &self.diff_error,
            &self.coverage_error,
            &self.previous_delivery_error,
        ]
        .into_iter()
        .flatten()
        {
            anyhow::ensure!(
                error.len() <= 8192 && client.redactor.text(error) == *error,
                "postprocessing diagnostic is invalid"
            );
        }
        if let Some(projection) = &self.summary_projection {
            anyhow::ensure!(
                projection.version == 1,
                "summary projection version is invalid"
            );
            hash(&projection.previous_digest)?;
            for diagnostic in [&projection.intended, &projection.external]
                .into_iter()
                .flatten()
            {
                anyhow::ensure!(
                    diagnostic.len() <= MAX_SUMMARY_DIAGNOSTIC_BYTES
                        && client.redactor.text(diagnostic) == *diagnostic,
                    "summary projection diagnostic exceeds its bound or contains sensitive data"
                );
            }
        }
        Ok(())
    }
}

fn validate_retained_event(
    plan: &RunPlan,
    client: &AuditClient,
    value: &Value,
    maximum: usize,
) -> Result<crate::model::Event> {
    anyhow::ensure!(
        serde_json::to_vec(value)?.len() <= maximum && client.redactor.value(value) == *value,
        "retained event exceeds its bound or contains sensitive data"
    );
    let event: crate::model::Event = serde_json::from_value(value.clone())?;
    event.validate()?;
    anyhow::ensure!(
        event.source == "runner"
            && event.provenance == "observed"
            && event.session_id == plan.request.session_id
            && event.project_id == plan.request.project_id
            && event.sandbox_id == plan.request.sandbox_id
            && event.agent_id == plan.request.agent_id
            && event.correlation_id == plan.request.correlation_id
            && event.trace_id == plan.request.trace_id,
        "retained event differs from its session identity"
    );
    Ok(event)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HelperCheckpoint {
    pub operation_id: Uuid,
    pub owned: Option<OwnedContainerSnapshot>,
    pub start_issued: bool,
    pub completed: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct LockIdentity {
    dev: u64,
    ino: u64,
}

#[cfg(unix)]
mod private_state {
    use super::*;
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::{MetadataExt, PermissionsExt},
        },
    };
    pub(super) struct Directory {
        file: fs::File,
        trusted: crate::control_fs::Directory,
    }
    impl Directory {
        pub(super) fn root(path: &Path) -> Result<Self> {
            let trusted = crate::control_fs::Directory::open_existing(
                &crate::control_fs::absolute_path(path)?,
            )?;
            Self::from_control(trusted)
        }
        pub(super) fn from_control(trusted: crate::control_fs::Directory) -> Result<Self> {
            let file = trusted.try_clone_file()?;
            Self::check_directory(&file)?;
            trusted.revalidate()?;
            Ok(Self { file, trusted })
        }
        fn check_directory(file: &fs::File) -> Result<()> {
            let m = file.metadata()?;
            anyhow::ensure!(
                m.is_dir() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o7777 == 0o700,
                "broker state directory must be owned by the current account with mode 0700"
            );
            Ok(())
        }
        pub(super) fn child(&self, name: &str) -> Result<Self> {
            self.trusted.revalidate()?;
            Self::from_control(self.trusted.child(name)?)
        }
        pub(super) fn sync(&self) -> Result<()> {
            self.trusted.sync()
        }
        pub(super) fn read<T: serde::de::DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
            self.read_bounded(name, 32 * 1024 * 1024)
        }
        pub(super) fn read_bounded<T: serde::de::DeserializeOwned>(
            &self,
            name: &str,
            maximum: usize,
        ) -> Result<Option<T>> {
            self.trusted.revalidate()?;
            let name = checked_name(name)?;
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ENOENT) {
                    return Ok(None);
                }
                return Err(error.into());
            }
            let file = unsafe { fs::File::from_raw_fd(fd) };
            check_private_file(&file)?;
            anyhow::ensure!(
                file.metadata()?.len() <= maximum as u64,
                "broker checkpoint exceeds its bound"
            );
            let read_limit = maximum
                .checked_add(1)
                .context("checkpoint read bound overflow")?;
            // from_reader(File) performs unbuffered byte reads. Use a fresh,
            // bounded buffer on this same verified FD before deserializing.
            let mut bytes = Vec::with_capacity(read_limit);
            std::io::copy(&mut file.take(u64::try_from(read_limit)?), &mut bytes)?;
            anyhow::ensure!(
                bytes.len() <= maximum,
                "broker checkpoint exceeds its bound"
            );
            Ok(Some(serde_json::from_slice(&bytes)?))
        }
        pub(super) fn write(&self, name: &str, value: &impl Serialize) -> Result<()> {
            self.trusted.revalidate()?;
            let target = checked_name(name)?;
            // Existing entries must be private regular files. A symlink/hardlink
            // is never overwritten to make a checkpoint appear valid.
            let existing_fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    target.as_ptr(),
                    libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if existing_fd >= 0 {
                check_private_file(&unsafe { fs::File::from_raw_fd(existing_fd) })?;
            } else {
                let error = std::io::Error::last_os_error();
                anyhow::ensure!(
                    error.raw_os_error() == Some(libc::ENOENT),
                    "broker checkpoint target is inaccessible or substituted"
                );
            }
            let temporary = checked_name(&format!(".checkpoint-{}", Uuid::new_v4().simple()))?;
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    temporary.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            anyhow::ensure!(fd >= 0, "cannot create private broker checkpoint");
            let mut file = unsafe { fs::File::from_raw_fd(fd) };
            let bytes = serde_json::to_vec(value)?;
            anyhow::ensure!(
                bytes.len() <= 32 * 1024 * 1024,
                "broker checkpoint exceeds its bound"
            );
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            self.trusted.revalidate()?;
            anyhow::ensure!(
                unsafe {
                    libc::renameat(
                        self.file.as_raw_fd(),
                        temporary.as_ptr(),
                        self.file.as_raw_fd(),
                        target.as_ptr(),
                    )
                } == 0,
                "cannot publish broker checkpoint"
            );
            self.file.sync_all()?;
            Ok(())
        }
        pub(super) fn copy_archive(&self, name: &str, mut source: fs::File) -> Result<String> {
            self.trusted.revalidate()?;
            let raw = checked_name(name)?;
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    raw.as_ptr(),
                    libc::O_RDWR
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            anyhow::ensure!(
                fd >= 0,
                "immutable workspace snapshot already exists or is inaccessible"
            );
            let mut destination = unsafe { fs::File::from_raw_fd(fd) };
            let mut digest = Sha256::new();
            let mut buffer = [0u8; 65536];
            let mut total = 0usize;
            loop {
                let n = source.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                total = total
                    .checked_add(n)
                    .context("workspace archive size overflow")?;
                anyhow::ensure!(
                    total <= Limits::default().max_archive_bytes(),
                    "workspace snapshot exceeds its archive bound"
                );
                digest.update(&buffer[..n]);
                destination.write_all(&buffer[..n])?;
            }
            destination.sync_all()?;
            self.file.sync_all()?;
            Ok(hex::encode(digest.finalize()))
        }
        pub(super) fn archive(&self, name: &str, expected: &str) -> Result<fs::File> {
            self.trusted.revalidate()?;
            use std::io::{Seek, SeekFrom};
            let raw = checked_name(name)?;
            let fd = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    raw.as_ptr(),
                    libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            anyhow::ensure!(fd >= 0, "immutable workspace snapshot is unavailable");
            let mut file = unsafe { fs::File::from_raw_fd(fd) };
            check_private_file(&file)?;
            anyhow::ensure!(
                file.metadata()?.len() <= Limits::default().max_archive_bytes() as u64,
                "workspace snapshot exceeds its bound"
            );
            let mut digest = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                digest.update(&buffer[..n]);
            }
            anyhow::ensure!(
                hex::encode(digest.finalize()) == expected,
                "immutable workspace snapshot changed; import refused"
            );
            file.seek(SeekFrom::Start(0))?;
            Ok(file)
        }
    }
    fn checked_name(name: &str) -> Result<CString> {
        anyhow::ensure!(
            !name.is_empty()
                && name.len() <= 128
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                && name != "."
                && name != "..",
            "invalid broker state entry name"
        );
        Ok(CString::new(name)?)
    }
    fn check_private_file(file: &fs::File) -> Result<()> {
        let m = file.metadata()?;
        anyhow::ensure!(
            m.is_file()
                && m.nlink() == 1
                && m.uid() == unsafe { libc::geteuid() }
                && m.permissions().mode() & 0o7777 == 0o600,
            "broker state file must be private, owned, regular and without hard links"
        );
        Ok(())
    }
    #[derive(Clone)]
    pub(super) struct AttemptGuard {
        inner: Arc<AttemptLock>,
    }
    struct AttemptLock {
        file: fs::File,
        directory: Directory,
        name: String,
        identity: LockIdentity,
    }
    impl AttemptGuard {
        pub(super) fn acquire(
            config: &Path,
            attempt: Uuid,
            expected: Option<LockIdentity>,
        ) -> Result<Self> {
            let directory = Directory::root(config)?.child("worker-locks")?;
            Self::acquire_in(directory, attempt, expected)
        }
        pub(super) fn acquire_in(
            directory: Directory,
            attempt: Uuid,
            expected: Option<LockIdentity>,
        ) -> Result<Self> {
            let name = format!("{attempt}.lock");
            let raw = checked_name(&name)?;
            let fd = unsafe {
                libc::openat(
                    directory.file.as_raw_fd(),
                    raw.as_ptr(),
                    libc::O_RDWR
                        | libc::O_CREAT
                        | libc::O_NONBLOCK
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            anyhow::ensure!(fd >= 0, "attempt lock is inaccessible or substituted");
            let file = unsafe { fs::File::from_raw_fd(fd) };
            check_private_file(&file)?;
            anyhow::ensure!(
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
                "another broker holds this attempt; no takeover or start is permitted"
            );
            let m = file.metadata()?;
            let identity = LockIdentity {
                dev: m.dev(),
                ino: m.ino(),
            };
            anyhow::ensure!(
                expected.is_none_or(|expected| expected == identity),
                "attempt lock inode changed; reconciliation requires diagnosis"
            );
            let guard = Self {
                inner: Arc::new(AttemptLock {
                    file,
                    directory,
                    name,
                    identity,
                }),
            };
            guard.ensure_current()?;
            Ok(guard)
        }
        pub(super) fn identity(&self) -> LockIdentity {
            self.inner.identity
        }
        pub(super) fn same_owner(&self, other: &Self) -> bool {
            Arc::ptr_eq(&self.inner, &other.inner)
        }
        pub(super) fn ensure_current(&self) -> Result<()> {
            self.inner.directory.trusted.revalidate()?;
            check_private_file(&self.inner.file)?;
            let raw = checked_name(&self.inner.name)?;
            let fd = unsafe {
                libc::openat(
                    self.inner.directory.file.as_raw_fd(),
                    raw.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            anyhow::ensure!(fd >= 0, "active attempt lock was removed or substituted");
            let current = unsafe { fs::File::from_raw_fd(fd) };
            check_private_file(&current)?;
            let m = current.metadata()?;
            anyhow::ensure!(
                self.inner.identity
                    == LockIdentity {
                        dev: m.dev(),
                        ino: m.ino()
                    },
                "active attempt lock inode changed"
            );
            Ok(())
        }
    }
}
#[cfg(unix)]
use private_state::{AttemptGuard, Directory};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postprocessing_payload_without_projection_keeps_its_legacy_bytes() {
        let legacy = r#"{"version":1,"declared":{"state":"pending","normalized_events":null,"acknowledged_events":null,"dropped_events":null,"unknown_delivery_events":null,"not_attempted_events":null,"stdout_truncated":null,"unterminated_final_line_omitted":null,"error":null},"diff":null,"diff_error":null,"coverage_revision":0,"coverage":null,"coverage_acknowledged":false,"coverage_error":null,"previous_delivery_error":null}"#;
        let pending = PostprocessingV1::pending(false, None, None);
        assert_eq!(serde_json::to_vec(&pending).unwrap(), legacy.as_bytes());
        let decoded: PostprocessingV1 = serde_json::from_str(legacy).unwrap();
        assert!(decoded.summary_projection.is_none());
        assert_eq!(serde_json::to_vec(&decoded).unwrap(), legacy.as_bytes());
    }

    #[test]
    fn launch_seed_is_distinct_for_scope_parent_intent_ref_and_invocation() {
        use crate::runtime::launch_profile::DockerLaunchProfileBindingV1;
        let profile = DockerLaunchProfileBindingV1::new(
            &"1".repeat(64),
            Uuid::from_u128(1),
            &"2".repeat(64),
            "unix:///not-opened/docker.sock",
            &"3".repeat(64),
        )
        .unwrap();
        let first = launch_authority_seed_name(&profile).unwrap();
        let mut names = std::collections::HashSet::from([first]);
        for field in 0..5 {
            let mut other = profile.clone();
            match field {
                0 => other.journal_scope_hash = "4".repeat(64),
                1 => other.parent_operation_id = Uuid::from_u128(2),
                2 => other.intent_hash = "5".repeat(64),
                3 => other.invocation_fingerprint = "6".repeat(64),
                _ => other.runtime_endpoint = "unix:///not-opened/other.sock".into(),
            }
            assert!(names.insert(launch_authority_seed_name(&other).unwrap()));
        }
        let mut invalid = profile;
        invalid.directory_ref = "../elsewhere".into();
        assert!(launch_authority_seed_name(&invalid).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn attempt_guard_rejects_concurrent_owners_substitution_and_linked_files() {
        let temp = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let attempt = Uuid::new_v4();
        let guard = AttemptGuard::acquire(temp.path(), attempt, None).unwrap();
        let identity = guard.identity();
        assert!(AttemptGuard::acquire(temp.path(), attempt, Some(identity)).is_err());
        let path = temp
            .path()
            .join("worker-locks")
            .join(format!("{attempt}.lock"));
        let moved = path.with_extension("parked");
        fs::rename(&path, &moved).unwrap();
        fs::write(&path, b"replacement").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(guard.ensure_current().is_err());
        assert!(AttemptGuard::acquire(temp.path(), attempt, Some(identity)).is_err());
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&moved, &path).unwrap();
        assert!(AttemptGuard::acquire(temp.path(), attempt, None).is_err());
        fs::remove_file(&path).unwrap();
        fs::hard_link(&moved, &path).unwrap();
        assert!(AttemptGuard::acquire(temp.path(), attempt, None).is_err());
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthoritySeed {
    version: u32,
    authority_id: Uuid,
    kind: RuntimeKind,
    endpoint: String,
    binding: Option<RuntimeAuthority>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    invocation: Option<InvocationBindingV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    docker_launch_profile: Option<crate::runtime::launch_profile::DockerLaunchProfileBindingV1>,
}
#[derive(Clone)]
struct PlanState {
    plan: Arc<Mutex<RunPlan>>,
    directory: Arc<Directory>,
    name: String,
}
impl PlanState {
    fn new(plan: RunPlan) -> Result<Self> {
        let directory = Directory::root(&plan.options.config_dir)?.child("sessions")?;
        let state = Self {
            name: format!("{}.json", plan.session),
            plan: Arc::new(Mutex::new(plan)),
            directory: Arc::new(directory),
        };
        let snapshot = state.snapshot()?;
        state.directory.write(&state.name, &snapshot)?;
        Ok(state)
    }
    fn snapshot(&self) -> Result<RunPlan> {
        Ok(self
            .plan
            .lock()
            .map_err(|_| anyhow::anyhow!("broker checkpoint lock poisoned"))?
            .clone())
    }
    fn update<T>(&self, change: impl FnOnce(&mut RunPlan) -> Result<T>) -> Result<T> {
        let mut current = self
            .plan
            .lock()
            .map_err(|_| anyhow::anyhow!("broker checkpoint lock poisoned"))?;
        let mut candidate = current.clone();
        let result = change(&mut candidate)?;
        self.directory.write(&self.name, &candidate)?;
        *current = candidate;
        Ok(result)
    }
    fn checkpoint(&self) -> Result<ManagedCheckpoint> {
        self.snapshot()?
            .managed
            .context("session has no managed checkpoint")
    }
    fn worker(&self) -> Result<WorkerV1> {
        self.checkpoint()?
            .worker
            .context("worker admission has not been acknowledged")
    }
    fn read_worker(&self, worker: WorkerV1) -> Result<()> {
        let mut plan = self
            .plan
            .lock()
            .map_err(|_| anyhow::anyhow!("broker checkpoint lock poisoned"))?;
        let checkpoint = plan.managed.as_mut().context("missing checkpoint")?;
        if checkpoint
            .worker
            .as_ref()
            .is_none_or(|current| worker.revision >= current.revision)
        {
            checkpoint.worker = Some(worker);
        }
        Ok(())
    }
}
impl AuditClient {
    pub(super) async fn get_typed<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let value = self
            .response(
                self.http
                    .get(self.control_url(reqwest::Method::GET, path)?)
                    .bearer_auth(&self.token),
            )
            .await?;
        serde_json::from_value(value).context("gateway returned an incompatible control response")
    }
    pub(super) async fn post_control<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        input: &impl Serialize,
    ) -> Result<T> {
        self.validate_identity_payload(input)?;
        let value = self
            .response(
                self.http
                    .post(self.control_url(reqwest::Method::POST, path)?)
                    .bearer_auth(&self.token)
                    .json(input),
            )
            .await?;
        serde_json::from_value(value).context("gateway returned an incompatible control response")
    }
}
fn managed(plan: &RunPlan) -> Result<&ManagedCheckpoint> {
    plan.managed
        .as_ref()
        .context("session has no managed checkpoint")
}
fn configured_origin(client: &AuditClient) -> String {
    client.base.as_str().to_owned()
}
async fn check_identity(client: &AuditClient, expected: Option<&str>) -> Result<String> {
    let identity: Value = client.get_typed("api/v1/identity").await?;
    anyhow::ensure!(
        identity["role"] == "runner",
        "worker control requires the trusted runner identity"
    );
    let tenant = identity["tenant_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .context("gateway identity lacks its tenant")?;
    anyhow::ensure!(
        expected.is_none_or(|expected| expected == tenant),
        "authenticated tenant differs from the private session binding"
    );
    Ok(tenant.into())
}
fn validate_worker(plan: &RunPlan, worker: &WorkerV1) -> Result<()> {
    let checkpoint = managed(plan)?;
    anyhow::ensure!(
        worker.admission_operation_id == plan.operation_id.to_string()
            && worker.request_id == checkpoint.admission.decision_request.request_id
            && worker.launch.spec_hash()? == checkpoint.admission.launch.spec_hash()?
            && worker.spec_hash == checkpoint.admission.launch.spec_hash()?,
        "gateway worker conflicts with the private admitted specification"
    );
    let id = Uuid::parse_str(&worker.worker_id)?;
    Uuid::parse_str(&worker.attempt.attempt_id)?;
    let suffix = format!("-{}", id.simple());
    anyhow::ensure!(
        worker.attempt.stable_name.starts_with("aig-")
            && worker.attempt.stable_name.ends_with(&suffix)
            && worker.attempt.volume_name == format!("{}-work", worker.attempt.stable_name),
        "gateway returned invalid physical worker names"
    );
    Ok(())
}
async fn refresh(client: &AuditClient, state: &PlanState) -> Result<WorkerV1> {
    let worker = refresh_readonly(client, state).await?;
    state.update(|_| Ok(()))?;
    Ok(worker)
}
async fn refresh_readonly(client: &AuditClient, state: &PlanState) -> Result<WorkerV1> {
    let old = state.worker()?;
    let worker: WorkerV1 = client
        .get_typed(&format!("api/v1/workers/{}", old.worker_id))
        .await?;
    validate_worker(&state.snapshot()?, &worker)?;
    state.read_worker(worker.clone())?;
    Ok(worker)
}
async fn mutate(
    client: &AuditClient,
    state: &PlanState,
    transition: WorkerTransitionV1,
) -> Result<MutationResultV1> {
    let worker = state.worker()?;
    let mutation = WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition,
    };
    state.update(|plan| {
        let checkpoint = plan.managed.as_mut().context("missing checkpoint")?;
        anyhow::ensure!(
            checkpoint.pending.is_none(),
            "a prior control response is unknown; reconcile before another mutation"
        );
        checkpoint.pending = Some(mutation.clone());
        Ok(())
    })?;
    let result: MutationResultV1 = client
        .post_control(
            &format!("api/v1/workers/{}/transitions", worker.worker_id),
            &mutation,
        )
        .await?;
    anyhow::ensure!(
        result.receipt.audit_seq > 0
            && result.receipt.audit_hash.len() == 64
            && result.receipt.operation_id == mutation.operation_id
            && result.receipt.input_hash == content_hash("ai-guard:worker-mutation:v1", &mutation)?,
        "gateway did not acknowledge the exact durable worker transition"
    );
    validate_worker(&state.snapshot()?, &result.worker)?;
    state.update(|plan| {
        let checkpoint = plan.managed.as_mut().context("missing checkpoint")?;
        checkpoint.worker = Some(result.worker.clone());
        checkpoint.pending = None;
        Ok(())
    })?;
    Ok(result)
}
async fn reconcile_pending(client: &AuditClient, state: &PlanState) -> Result<()> {
    let checkpoint = state.checkpoint()?;
    let Some(pending) = checkpoint.pending else {
        return Ok(());
    };
    // Retry only the same durable mutation identity. StartClaim replay always
    // returns AlreadyConsumed; callers of reconciliation can never emit start.
    let result: MutationResultV1 = client
        .post_control(
            &format!("api/v1/workers/{}/transitions", pending.worker_id),
            &pending,
        )
        .await?;
    anyhow::ensure!(
        result.receipt.operation_id == pending.operation_id
            && result.receipt.input_hash == content_hash("ai-guard:worker-mutation:v1", &pending)?,
        "replayed control receipt conflicts with the pending operation"
    );
    if matches!(pending.transition, WorkerTransitionV1::StartClaim { .. }) {
        anyhow::ensure!(
            result.disposition == Some(ClaimDispositionV1::AlreadyConsumed),
            "a replay must never return a fresh start disposition"
        );
    }
    state.update(|plan| {
        let checkpoint = plan.managed.as_mut().context("missing checkpoint")?;
        checkpoint.worker = Some(result.worker);
        checkpoint.pending = None;
        Ok(())
    })?;
    refresh(client, state).await?;
    Ok(())
}
fn image_capture(output: crate::runtime::Capture) -> Result<String> {
    anyhow::ensure!(
        !output.truncated,
        "runtime image reference exceeds its bound"
    );
    let raw = std::str::from_utf8(&output.bytes)?.trim();
    let raw = raw.strip_prefix("sha256:").unwrap_or(raw);
    anyhow::ensure!(
        raw.len() == 64 && raw.bytes().all(|b| b.is_ascii_hexdigit()),
        "runtime did not prove an immutable image ID"
    );
    Ok(format!("sha256:{raw}"))
}
async fn selected_endpoint(options: &RunOptions, runtime: &Runtime) -> Result<String> {
    if let Some(endpoint) = &options.runtime_endpoint {
        return Ok(endpoint.clone());
    }
    if options.runtime == RuntimeKind::Podman {
        return Ok("local".into());
    }
    docker_endpoint(runtime, None).await
}
async fn prepare_authority(
    plan: &RunPlan,
    approved: &Runtime,
) -> Result<(String, AuthoritySeed, String)> {
    prepare_authority_for_launch(plan, approved, None).await
}
fn launch_authority_seed_name(
    profile: &crate::runtime::launch_profile::DockerLaunchProfileBindingV1,
) -> Result<String> {
    profile.validate()?;
    let value = json!({"version":2,"kind":"docker","journal_scope_hash":profile.journal_scope_hash,
        "parent_operation_id":profile.parent_operation_id,"intent_hash":profile.intent_hash,
        "launch_profile":profile,"invocation_fingerprint":profile.invocation_fingerprint,
        "runtime_endpoint":profile.runtime_endpoint});
    let mut hash = Sha256::new();
    hash.update(b"ai-guard:runtime-authority-seed:v2\0");
    hash.update(crate::broker::journal::canonical(&value)?);
    Ok(format!(
        "docker-launch-v1-{}.json",
        hex::encode(hash.finalize())
    ))
}
async fn prepare_authority_for_launch(
    plan: &RunPlan,
    approved: &Runtime,
    launch: Option<&crate::runtime::launch_profile::RetainedDockerLaunchProfile>,
) -> Result<(String, AuthoritySeed, String)> {
    let endpoint = selected_endpoint(&plan.options, approved).await?;
    let name = if let Some(profile) = launch {
        profile.revalidate()?;
        let binding = profile.binding();
        anyhow::ensure!(
            plan.options.runtime == RuntimeKind::Docker
                && binding.runtime_endpoint == endpoint
                && binding.parent_operation_id == plan.operation_id
                && binding.invocation_fingerprint == approved.invocation().fingerprint(),
            "Docker start profile differs from preparation"
        );
        launch_authority_seed_name(binding)?
    } else {
        format!(
            "{}-{}-{}.json",
            plan.options.runtime,
            &hex::encode(Sha256::digest(endpoint.as_bytes()))[..24],
            &approved.invocation().fingerprint()[..24],
        )
    };
    let directory = Directory::root(&plan.options.config_dir)?.child("authorities")?;
    let mut seed = directory
        .read::<AuthoritySeed>(&name)?
        .unwrap_or(AuthoritySeed {
            version: if launch.is_some() { 2 } else { 1 },
            authority_id: Uuid::new_v4(),
            kind: plan.options.runtime,
            endpoint: endpoint.clone(),
            binding: None,
            invocation: Some(approved.invocation().clone()),
            docker_launch_profile: launch.map(|p| p.binding().clone()),
        });
    let expected_version = if launch.is_some() { 2 } else { 1 };
    anyhow::ensure!(
        seed.version == expected_version
            && !seed.authority_id.is_nil()
            && seed.kind == plan.options.runtime
            && seed.endpoint == endpoint,
        "runtime authority seed conflicts with the configured target"
    );
    anyhow::ensure!(
        seed.invocation.as_ref() == Some(approved.invocation()),
        "runtime authority invocation differs from the approved program"
    );
    anyhow::ensure!(
        seed.docker_launch_profile.as_ref() == launch.map(|p| p.binding()),
        "runtime authority start profile differs from accepted binding"
    );
    let image = if let Some(binding) = seed.binding.clone() {
        anyhow::ensure!(
            binding.docker_launch_profile.as_ref() == seed.docker_launch_profile.as_ref(),
            "runtime authority seed binding differs"
        );
        if launch.is_some() {
            anyhow::ensure!(
                binding.authority_id == seed.authority_id
                    && binding.kind == seed.kind
                    && binding.endpoint == seed.endpoint
                    && binding.invocation == seed.invocation,
                "strict runtime authority seed fields differ from its binding"
            );
        }
        let runtime = ManagedRuntime::reopen_with_runtime(binding, approved.clone()).await?;
        let runtime = if let Some(profile) = launch {
            runtime.retain_docker_launch_profile(profile.clone())?
        } else {
            runtime
        };
        runtime.doctor(plan.options.allow_rootful_runtime).await?;
        runtime.resolve_image(&plan.options.image).await?
    } else if seed.kind == RuntimeKind::Docker {
        let runtime = ManagedRuntime::connect_docker_with_runtime(
            seed.authority_id,
            &endpoint,
            approved.clone(),
        )
        .await?;
        let runtime = if let Some(profile) = launch {
            runtime.bind_docker_launch_profile(profile.clone())?
        } else {
            runtime
        };
        runtime.doctor(plan.options.allow_rootful_runtime).await?;
        let image = runtime.resolve_image(&plan.options.image).await?;
        seed.binding = Some(runtime.binding().clone());
        image
    } else {
        let mut args = crate::runtime::selected_podman_target_args(&endpoint)?;
        args.extend([
            "image".into(),
            "inspect".into(),
            "--format".into(),
            "{{.Id}}".into(),
            plan.options.image.clone(),
        ]);
        image_capture(approved.cli(&args, Duration::from_secs(15)).await?)?
    };
    directory.write(&name, &seed)?;
    Ok((name, seed, image))
}
async fn reopen_authority(
    state: &PlanState,
    client: &AuditClient,
    binding: RuntimeAuthority,
    program: Option<&Path>,
    adopt: bool,
) -> Result<ManagedRuntime> {
    let plan = state.snapshot()?;
    let checkpoint = managed(&plan)?;
    let previous_fingerprint = binding.invocation_fingerprint();
    let approved = if let Some(invocation) = &binding.invocation {
        let previous =
            Runtime::from_invocation(binding.kind, invocation.clone()).and_then(|runtime| {
                exclude_project_runtime(&runtime, &plan.options.project)?;
                Ok(runtime)
            });
        match previous {
            Ok(previous) => {
                if let Some(program) = program {
                    let proposed = approved_runtime(&plan.options, Some(program))?;
                    if proposed.invocation() == invocation {
                        anyhow::ensure!(checkpoint.invocation_event.is_none(), "an invocation adoption is pending; retry --adopt-runtime-program with the same --runtime-program candidate before runtime effects");
                        return ManagedRuntime::reopen_with_runtime(binding, previous).await;
                    }
                    anyhow::ensure!(adopt, "runtime program differs from the durable invocation; use --adopt-runtime-program for an audited verification of the same engine before effects");
                    proposed
                } else {
                    // Repeating the adoption flag for the same valid binding
                    // never creates a second event or a new start permission.
                    anyhow::ensure!(checkpoint.invocation_event.is_none(), "an invocation adoption is pending; retry --adopt-runtime-program with the same --runtime-program candidate before runtime effects");
                    return ManagedRuntime::reopen_with_runtime(binding, previous).await;
                }
            }
            Err(error) if !adopt => return Err(error.context("durable runtime invocation is unavailable or invalid; use --adopt-runtime-program to verify the same engine with a newly approved program before effects")),
            Err(_) => approved_runtime(&plan.options, program.or(Some(invocation.approved_entry.as_path())))?,
        }
    } else {
        anyhow::ensure!(
            adopt,
            "legacy runtime invocation is unbound; use --adopt-runtime-program to verify and durably bind the control program before effects"
        );
        approved_runtime(&plan.options, program)?
    };
    // Keep the historical invocation in the durable checkpoint until its
    // replacement has passed read-only engine proof and durable audit.
    let mut engine_binding = binding.clone();
    engine_binding.invocation = None;
    let runtime = ManagedRuntime::adopt_invocation(engine_binding, approved).await?;
    let fingerprint = runtime
        .binding()
        .invocation_fingerprint()
        .context("adoption lacks an invocation")?;
    let history = Directory::root(&plan.options.config_dir)?.child("invocation-history")?;
    let history_name = format!(
        "{}-{}.json",
        binding.authority_id,
        previous_fingerprint.as_deref().unwrap_or("unbound")
    );
    if let Some(previous) = history.read::<RuntimeAuthority>(&history_name)? {
        anyhow::ensure!(
            previous == binding,
            "runtime invocation history conflicts with the previous authority"
        );
    } else {
        history.write(&history_name, &binding)?;
    }
    let pending = if let Some(pending) = &checkpoint.invocation_event {
        anyhow::ensure!(
            pending["kind"] == "runtime.invocation.bound"
                && pending["session_id"].as_str() == plan.request.session_id.as_deref()
                && pending["data"]["authority_id"] == runtime.binding().authority_id.to_string()
                && pending["data"]["invocation_fingerprint"] == fingerprint
                && pending["data"]["previous_invocation_fingerprint"]
                    == serde_json::to_value(&previous_fingerprint)?,
            "pending invocation adoption differs from this session or program"
        );
        pending.clone()
    } else {
        let pending = event(
            &plan,
            "runtime.invocation.bound",
            json!({"authority_id":runtime.binding().authority_id,"previous_invocation_fingerprint":previous_fingerprint,"new_invocation_fingerprint":fingerprint,"invocation_fingerprint":fingerprint,"legacy_adoption":previous_fingerprint.is_none()}),
        );
        state.update(|plan| {
            plan.managed
                .as_mut()
                .context("missing checkpoint")?
                .invocation_event = Some(pending.clone());
            Ok(())
        })?;
        pending
    };
    let directory = Directory::root(&plan.options.config_dir)?.child("authorities")?;
    let mut seed = directory.read::<AuthoritySeed>(&checkpoint.authority_file)?;
    if let Some(seed) = &seed {
        anyhow::ensure!(
            seed.authority_id == runtime.binding().authority_id,
            "authority seed differs during invocation adoption"
        );
    }
    client.post_control_event(pending).await?;
    if let Some(seed) = &mut seed {
        seed.binding = Some(runtime.binding().clone());
        seed.invocation = runtime.binding().invocation.clone();
        directory.write(&checkpoint.authority_file, seed)?;
    }
    state.update(|plan| {
        let checkpoint = plan.managed.as_mut().context("missing checkpoint")?;
        checkpoint.binding = Some(runtime.binding().clone());
        checkpoint.invocation_event = None;
        Ok(())
    })?;
    Ok(runtime)
}
async fn bound_runtime(
    state: &PlanState,
    client: &AuditClient,
    program: Option<&Path>,
    adopt: bool,
) -> Result<ManagedRuntime> {
    let plan = state.snapshot()?;
    let checkpoint = managed(&plan)?;
    if let Some(binding) = checkpoint.binding.clone() {
        return reopen_authority(state, client, binding, program, adopt).await;
    }
    let directory = Directory::root(&plan.options.config_dir)?.child("authorities")?;
    let mut seed: AuthoritySeed = directory
        .read(&checkpoint.authority_file)?
        .context("private runtime authority seed missing")?;
    anyhow::ensure!(
        seed.authority_id.to_string() == checkpoint.admission.launch.authority_id
            && seed.kind == RuntimeKind::Podman,
        "runtime authority seed conflicts with the admitted launch"
    );
    let runtime = if let Some(binding) = seed.binding.clone() {
        reopen_authority(state, client, binding, program, adopt).await?
    } else {
        // Explicit bootstrap occurs only after the server acknowledged quota.
        let invocation = seed
            .invocation
            .clone()
            .context("runtime authority invocation is unbound; operator adoption is required")?;
        let approved = Runtime::from_invocation(seed.kind, invocation)?;
        exclude_project_runtime(&approved, &plan.options.project)?;
        if let Some(program) = program {
            anyhow::ensure!(
                approved_runtime(&plan.options, Some(program))?.invocation()
                    == approved.invocation(),
                "runtime program differs from the durable invocation"
            );
        }
        let runtime = ManagedRuntime::bootstrap_podman_for_launch(
            seed.authority_id,
            &seed.endpoint,
            approved,
            plan.options.allow_rootful_runtime,
        )
        .await?;
        seed.binding = Some(runtime.binding().clone());
        directory.write(&checkpoint.authority_file, &seed)?;
        runtime
    };
    runtime.doctor(plan.options.allow_rootful_runtime).await?;
    anyhow::ensure!(
        runtime.resolve_image(&plan.options.image).await? == checkpoint.admission.launch.image_id,
        "runtime image changed after admission; no worker resource was created"
    );
    state.update(|plan| {
        plan.managed.as_mut().context("missing checkpoint")?.binding =
            Some(runtime.binding().clone());
        Ok(())
    })?;
    Ok(runtime)
}

/// The refusal the operator reads: the audited rule and reason of the durable decision, and the
/// fact that nothing was reserved or launched.
fn policy_denial(answer: &Value) -> anyhow::Error {
    let rule = answer["decision"]["rule"]
        .as_str()
        .unwrap_or("rule unavailable");
    let reason = answer["decision"]["reason"]
        .as_str()
        .unwrap_or("The gateway did not provide a reason.");
    anyhow::anyhow!(
        "policy denied the command ({rule}). {reason} No worker was reserved or launched."
    )
}

async fn authorize_plan(
    plan: &mut RunPlan,
    client: &AuditClient,
    supplied: Option<&str>,
) -> Result<(DecisionReceiptV1, Value)> {
    let mut initial = plan.request.clone();
    initial.human_approved = false;
    initial.approval_hash = None;
    let mut answer = client.decide(&initial).await?;
    let mut chosen = initial;
    match answer.pointer("/decision/outcome").and_then(Value::as_str) {
        Some("allow") => {}
        Some("deny") => return Err(policy_denial(&answer)),
        Some("review") => {
            let hash = crate::policy::request_hash(&chosen);
            let pending = approval_path(&plan.options.config_dir, &hash)?;
            if !pending.exists() {
                write_json_new(&pending, plan)?;
            }
            let approval = if let Some(supplied) = supplied {
                supplied.to_owned()
            } else {
                if let Some(mode) = chosen.execution_mode {
                    eprintln!(
                        "Execution mode requiring review: {}",
                        serde_json::to_string(&mode)?
                    );
                }
                eprintln!("Human review required. Request digest: {hash}");
                anyhow::ensure!(
                    std::io::stdin().is_terminal(),
                    "repeat the same run with --approve-hash {hash}; nothing was launched"
                );
                eprint!("Type the full digest to approve this execution: ");
                std::io::stderr().flush()?;
                tokio::task::spawn_blocking(|| {
                    let mut input = String::new();
                    std::io::stdin()
                        .read_line(&mut input)
                        .map(|_| input.trim().to_owned())
                })
                .await??
            };
            anyhow::ensure!(
                approval == hash,
                "approval does not match this exact request; nothing was launched"
            );
            chosen.human_approved = true;
            chosen.approval_hash = Some(hash);
            answer = client.decide(&chosen).await?;
            if answer.pointer("/decision/outcome").and_then(Value::as_str) == Some("deny") {
                return Err(policy_denial(&answer));
            }
            anyhow::ensure!(
                answer.pointer("/decision/outcome").and_then(Value::as_str) == Some("allow"),
                "approved request did not receive a durable allow"
            );
        }
        _ => bail!("gateway returned an unknown policy outcome"),
    }
    let receipt: DecisionReceiptV1 = serde_json::from_value(answer["receipt"].clone())
        .context("gateway decision has no versioned durable receipt; worker launch is blocked")?;
    anyhow::ensure!(
        receipt.version == 1
            && receipt.audit_seq > 0
            && receipt.audit_hash.len() == 64
            && receipt.request_id == chosen.request_id
            && receipt.request_hash == crate::policy::request_hash(&chosen)
            && receipt.phase
                == if chosen.human_approved {
                    DecisionPhaseV1::Approved
                } else {
                    DecisionPhaseV1::Initial
                },
        "decision receipt is not bound to the exact authorized identity and phase"
    );
    plan.request = chosen;
    Ok((receipt, answer["decision"].clone()))
}
fn credential_digest(credentials: Option<&CredentialFile>) -> Result<Option<String>> {
    Ok(credentials.map(|credentials| credentials.file.digest().to_owned()))
}

/// Inputs supplied by the authenticated broker after its durable decision.
/// This builder performs no admission and creates no runtime resource.
pub(super) struct BrokerPreparation<'a> {
    pub plan: &'a RunPlan,
    pub scope: &'a crate::broker::journal::JournalScopeV1,
    pub binding: &'a crate::broker::journal::BindingManifestV1,
    pub answer: &'a crate::broker::checkpoint::DecisionAnswerV1,
    pub runtime: &'a Runtime,
    pub workspace: workspace::PreparedWorkspace,
    pub credentials: Option<&'a CredentialFile>,
    pub launch_profile: Option<&'a crate::runtime::launch_profile::RetainedDockerLaunchProfile>,
}

pub(super) fn broker_require_new_archive(config: &Path, operation: Uuid) -> Result<()> {
    let root = crate::control_fs::Directory::open_existing(config)?;
    let sessions = root.child("sessions")?;
    sessions.sync()?;
    root.sync()?;
    anyhow::ensure!(
        sessions
            .private_entry(std::ffi::OsStr::new(&format!("{operation}.tar")))?
            .is_none(),
        "unbound workspace archive requires reconciliation; original source was not reread"
    );
    Ok(())
}

pub(super) async fn broker_prepare(input: BrokerPreparation<'_>) -> Result<ManagedCheckpoint> {
    let plan = input.plan;
    anyhow::ensure!(!plan.options.interactive, "broker requires batch mode");
    super::validate_new_runtime_target(&plan.options)?;
    let root = crate::control_fs::Directory::open_existing(&plan.options.config_dir)?;
    if let Some(profile) = input.launch_profile {
        let mut scope_hash = Sha256::new();
        scope_hash.update(b"ai-guard:broker-journal-scope:v1\0");
        scope_hash.update(crate::broker::journal::canonical(input.scope)?);
        anyhow::ensure!(
            profile.binding().journal_scope_hash == hex::encode(scope_hash.finalize()),
            "Docker start profile scope differs from preparation"
        );
    }
    let authorities = root.child("authorities")?;
    authorities.sync()?;
    root.sync()?;
    let (authority_file, seed, image_id) =
        prepare_authority_for_launch(plan, input.runtime, input.launch_profile).await?;
    let archive_name = format!("{}.tar", plan.operation_id);
    // O_EXCL is the authority here. An orphan, including a partial file, is
    // preserved and cannot cause the original project to be copied again.
    let config = plan.options.config_dir.clone();
    let name = archive_name.clone();
    let archive = input.workspace.archive.reopen()?;
    let archive_digest = tokio::task::spawn_blocking(move || {
        Directory::root(&config)?
            .child("sessions")?
            .copy_archive(&name, archive)
    })
    .await??;
    let launch = LaunchSpecV1 {
        version: 1,
        session_id: plan.session.to_string(),
        command: plan.request.command.clone(),
        profile: plan.options.profile.clone(),
        runtime_kind: runtime_kind(plan.options.runtime),
        authority_id: seed.authority_id.to_string(),
        image_ref: plan.options.image.clone(),
        image_id,
        network: network_kind(plan.options.network),
        cpu_millis: 2000,
        memory_bytes: 1024 * 1024 * 1024,
        pids_limit: 256,
        timeout_seconds: u32::try_from(plan.options.timeout)?,
        security_preset: SecurityPresetV1::FixedSandboxV2,
        restart_policy: RestartPolicyV1::No,
        workspace_manifest_digest: content_hash(
            "ai-guard:workspace-manifest:v1",
            &input.workspace.manifest,
        )?,
        workspace_import_policy_version: 1,
        provider: plan.options.provider.clone(),
        capture_output: plan.options.capture_output,
        credential_binding_id: input.binding.binding_id().map(|id| id.to_string()),
        credential_names: input.binding.names().to_vec(),
        provider_resolution: plan.request.provider_resolution.clone(),
    };
    Ok(ManagedCheckpoint {
        tenant_id: input.scope.tenant_id().into(),
        gateway_origin: input.scope.gateway_origin().into(),
        config_root: plan.options.config_dir.clone(),
        binding: seed.binding,
        authority_file,
        admission: AdmissionV1 {
            version: 1,
            operation_id: plan.operation_id.to_string(),
            decision_request: plan.request.clone(),
            receipt: input.answer.receipt().clone(),
            launch,
        },
        worker: None,
        owned: None,
        pending: None,
        uncertain: Vec::new(),
        lock_identity: None,
        before: input.workspace.manifest,
        import: None,
        decision: serde_json::to_value(input.answer.decision())?,
        output_event: None,
        output_acknowledged: false,
        result: None,
        archive_name,
        archive_digest,
        credential_digest: credential_digest(input.credentials)?,
        workspace_control_exclusion_version: Some(1),
        invocation_event: None,
    })
}

pub(super) fn broker_archive(checkpoint: &ManagedCheckpoint) -> Result<fs::File> {
    let sessions = Directory::root(&checkpoint.config_root)?.child("sessions")?;
    let mut archive = sessions.archive(&checkpoint.archive_name, &checkpoint.archive_digest)?;
    let manifest = workspace::scan_workspace_archive(&mut archive, Limits::default())?;
    // Old checkpoints did not observe modes. Preserve exactly their recorded
    // coverage while still checking all paths, content and known modes.
    let manifest = workspace::manifest_at_recorded_coverage(&checkpoint.before, &manifest);
    anyhow::ensure!(
        manifest == checkpoint.before
            && content_hash("ai-guard:workspace-manifest:v1", &manifest)?
                == checkpoint.admission.launch.workspace_manifest_digest,
        "immutable archive manifest differs from the admitted workspace"
    );
    use std::io::{Seek, SeekFrom};
    archive.seek(SeekFrom::Start(0))?;
    Ok(archive)
}

/// Exact invocation only. Podman's existing bootstrap is retained, but is
/// reachable here only after an accepted admission has supplied the real worker.
pub(super) async fn broker_runtime(
    checkpoint: &ManagedCheckpoint,
    approved: &Runtime,
    allow_rootful: bool,
    launch: Option<&crate::runtime::launch_profile::RetainedDockerLaunchProfile>,
) -> Result<ManagedRuntime> {
    anyhow::ensure!(
        checkpoint.worker.is_some(),
        "runtime requires an admitted worker"
    );
    let runtime = if let Some(binding) = &checkpoint.binding {
        anyhow::ensure!(
            binding.invocation.as_ref() == Some(approved.invocation()),
            "runtime invocation differs from its durable binding"
        );
        ManagedRuntime::reopen_with_runtime(binding.clone(), approved.clone()).await?
    } else {
        let directory = Directory::root(&checkpoint.config_root)?.child("authorities")?;
        let mut seed: AuthoritySeed = directory
            .read(&checkpoint.authority_file)?
            .context("runtime authority seed is unavailable")?;
        anyhow::ensure!(
            seed.authority_id.to_string() == checkpoint.admission.launch.authority_id
                && seed.kind == RuntimeKind::Podman
                && seed.invocation.as_ref() == Some(approved.invocation()),
            "runtime authority differs from the admitted launch"
        );
        let runtime = if let Some(binding) = seed.binding.clone() {
            ManagedRuntime::reopen_with_runtime(binding, approved.clone()).await?
        } else {
            ManagedRuntime::bootstrap_podman_for_launch(
                seed.authority_id,
                &seed.endpoint,
                approved.clone(),
                allow_rootful,
            )
            .await?
        };
        seed.binding = Some(runtime.binding().clone());
        directory.write(&checkpoint.authority_file, &seed)?;
        runtime
    };
    let runtime = if let Some(profile) = launch {
        runtime.retain_docker_launch_profile(profile.clone())?
    } else {
        anyhow::ensure!(
            runtime.binding().docker_launch_profile.is_none(),
            "Docker start profile capability is unavailable"
        );
        runtime
    };
    runtime.doctor(allow_rootful).await?;
    anyhow::ensure!(
        runtime
            .resolve_image(&checkpoint.admission.launch.image_id)
            .await?
            == checkpoint.admission.launch.image_id,
        "admitted image changed"
    );
    Ok(runtime)
}

pub(super) fn broker_helper_spec(policy: &HelperExecutionPolicyV1) -> Result<ContainerSpec> {
    policy.validate()?;
    ContainerSpec::with_operation(
        Uuid::parse_str(&policy.session_id)?,
        if policy.purpose == HelperPurposeV1::Import {
            ContainerPurpose::Import
        } else {
            ContainerPurpose::Export
        },
        &policy.image_id,
        policy.command.argv.clone(),
        NetworkMode::None,
        Uuid::parse_str(&policy.helper_id)?,
    )?
    .with_security_preset(policy.security_preset)
    .with_managed_names(&policy.stable_name, &policy.volume_name)
}

pub(super) fn broker_helper_observation(
    parent: &WorkerV1,
    policy: &HelperExecutionPolicyV1,
    observation: &ContainerObservation,
) -> Result<RuntimeObservationV1> {
    // Reuse the existing pure allocation/process normalization. Override only
    // the specification identity, which belongs to this real helper policy.
    let mut normalized = observation_dto(parent, observation)?;
    normalized.inspection.spec_hash = policy.spec_hash()?;
    normalized.inspection.network = policy.network;
    normalized.inspection.security_preset = policy.security_preset;
    Ok(normalized)
}
#[cfg(test)]
mod credential_digest_tests {
    use super::*;
    #[test]
    fn prepared_credential_digest_does_not_reopen_a_substituted_snapshot_name() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("provider.env");
        fs::write(&source, "OPENAI_API_KEY=example-private-value\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let config = crate::control_fs::Directory::open_or_create(
            &temporary
                .path()
                .canonicalize()
                .unwrap()
                .join("private-config"),
        )
        .unwrap();
        let credential = credentials(&source, &config).unwrap();
        let snapshot = credential.file.path().to_owned();
        fs::rename(&snapshot, temporary.path().join("held-original")).unwrap();
        fs::write(&snapshot, "OPENAI_API_KEY=wrong-substitute-value\n").unwrap();
        let expected = hex::encode(Sha256::digest(b"OPENAI_API_KEY=example-private-value\n"));
        assert_eq!(
            credential_digest(Some(&credential)).unwrap(),
            Some(expected)
        );
    }
}
fn runtime_kind(kind: RuntimeKind) -> RuntimeKindV1 {
    match kind {
        RuntimeKind::Docker => RuntimeKindV1::Docker,
        RuntimeKind::Podman => RuntimeKindV1::Podman,
    }
}
fn network_kind(network: NetworkMode) -> NetworkV1 {
    match network {
        NetworkMode::None => NetworkV1::None,
        NetworkMode::Bridge => NetworkV1::Bridge,
    }
}
pub(super) fn inspection(
    worker: &WorkerV1,
    observation: &ContainerObservation,
) -> Result<RuntimeInspectionV1> {
    let cpu = observation
        .allocation
        .cpu
        .as_ref()
        .context("runtime CPU allocation is unknown")?;
    let millis = u128::from(cpu.quota)
        .checked_mul(1000)
        .context("runtime CPU allocation overflow")?;
    anyhow::ensure!(
        cpu.period > 0 && millis % u128::from(cpu.period) == 0,
        "runtime CPU allocation cannot be represented exactly"
    );
    anyhow::ensure!(
        observation.configuration_fingerprint.is_some(),
        "runtime sandbox configuration proof is unknown"
    );
    anyhow::ensure!(
        observation.configuration_fingerprint_version.unwrap_or(1)
            == worker
                .launch
                .security_preset
                .configuration_fingerprint_version(),
        "runtime sandbox configuration proof uses a different security preset"
    );
    Ok(RuntimeInspectionV1 {
        authority_id: worker.launch.authority_id.clone(),
        runtime_kind: worker.launch.runtime_kind,
        container_id: observation
            .id
            .clone()
            .context("runtime container ID is unknown")?,
        stable_name: observation.name.clone(),
        volume_name: worker.attempt.volume_name.clone(),
        image_id: observation
            .image_id
            .clone()
            .context("runtime image ID is unknown")?,
        spec_hash: worker.spec_hash.clone(),
        cpu_millis: u32::try_from(millis / u128::from(cpu.period))?,
        memory_bytes: observation
            .allocation
            .memory_bytes
            .context("runtime memory allocation is unknown")?,
        pids_limit: u32::try_from(
            observation
                .allocation
                .pids_limit
                .context("runtime PID allocation is unknown")?,
        )?,
        network: worker.launch.network,
        security_preset: worker.launch.security_preset,
        restart_policy: RestartPolicyV1::No,
    })
}
pub(super) fn observation_dto(
    worker: &WorkerV1,
    observation: &ContainerObservation,
) -> Result<RuntimeObservationV1> {
    let status = match observation.process.status.as_deref() {
        Some("created" | "configured" | "initialized") => RuntimeStatusV1::Created,
        Some("running") => RuntimeStatusV1::Running,
        Some("exited" | "stopped") => RuntimeStatusV1::Exited,
        Some("dead") => RuntimeStatusV1::Dead,
        _ => RuntimeStatusV1::Unknown,
    };
    Ok(RuntimeObservationV1 {
        inspection: inspection(worker, observation)?,
        observed_at: observation
            .observed_at
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        status,
        running: observation.process.running,
        started_at: observation.process.started_at.map(|date| {
            date.with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
        }),
        finished_at: observation.process.finished_at.map(|date| {
            date.with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
        }),
        exit_code: observation.process.exit_code.map(i64::from),
        oom_killed: observation.process.oom_killed,
        restart_count: observation.restart_count.map(u32::try_from).transpose()?,
    })
}
pub(super) fn worker_spec(
    plan: &RunPlan,
    worker: &WorkerV1,
    credentials: Option<&CredentialFile>,
) -> Result<ContainerSpec> {
    let mut spec = ContainerSpec::with_operation(
        plan.session,
        ContainerPurpose::Run,
        &worker.launch.image_id,
        worker.launch.command.argv.clone(),
        match worker.launch.network {
            NetworkV1::None => NetworkMode::None,
            NetworkV1::Bridge => NetworkMode::Bridge,
        },
        plan.operation_id,
    )?
    .with_security_preset(worker.launch.security_preset)
    .with_managed_names(&worker.attempt.stable_name, &worker.attempt.volume_name)?;
    if let Some(credentials) = credentials {
        spec = spec.with_secret_env_handle(credentials.file.private_file())?;
    }
    if worker.launch.security_preset.is_interactive() {
        anyhow::ensure!(
            credentials
                .is_none_or(|credentials| !credentials.names.iter().any(|name| name == "TERM")),
            "TERM is fixed by the interactive preset and cannot be supplied in a credential file"
        );
        let mut secrets = control_secrets(&plan.options.config_dir)?;
        secrets.extend(
            credentials
                .map(|credentials| credentials.secrets.clone())
                .unwrap_or_default(),
        );
        spec = spec.with_terminal_secrets(secrets)?;
    }
    Ok(spec)
}
async fn record_observation(
    client: &AuditClient,
    state: &PlanState,
    observed: &ContainerObservation,
) -> Result<()> {
    let worker = state.worker()?;
    if worker.state == WorkerStateV1::Terminal {
        let old = worker
            .last_observation
            .as_ref()
            .context("terminal worker has no durable observation")?;
        let new = observation_dto(&worker, observed)?;
        anyhow::ensure!(
            old.inspection == new.inspection
                && old.started_at == new.started_at
                && old.finished_at == new.finished_at
                && old.exit_code == new.exit_code
                && old.oom_killed == new.oom_killed
                && old.restart_count == new.restart_count,
            "current runtime terminal conflicts with the committed terminal"
        );
        return Ok(());
    }
    mutate(
        client,
        state,
        WorkerTransitionV1::Observe {
            observation: observation_dto(&worker, observed)?,
        },
    )
    .await?;
    Ok(())
}
async fn mark_unknown(client: &AuditClient, state: &PlanState, reason: &str) {
    let _ = try_mark_unknown(client, state, reason).await;
}
async fn try_mark_unknown(client: &AuditClient, state: &PlanState, reason: &str) -> Result<()> {
    // Never overwrite an unresolved Created operation. Its original bytes and
    // owned snapshot remain in the private checkpoint for exact reconciliation.
    let checkpoint = state.checkpoint()?;
    if checkpoint.owned.is_some()
        && checkpoint
            .pending
            .as_ref()
            .is_some_and(|pending| matches!(pending.transition, WorkerTransitionV1::Created { .. }))
    {
        bail!("Created acknowledgement is unresolved; control failure and owned instance retained");
    }
    if refresh(client, state).await?.quota_held {
        state.update(|plan| {
            let checkpoint = plan.managed.as_mut().context("missing checkpoint")?;
            if let Some(pending) = checkpoint.pending.take() {
                anyhow::ensure!(
                    checkpoint.uncertain.len() < 64,
                    "too many unresolved control operations"
                );
                checkpoint.uncertain.push(pending);
            }
            Ok(())
        })?;
        let reason = crate::redaction::clip(&client.redactor.text(reason), 1000);
        mutate(client, state, WorkerTransitionV1::Unknown { reason }).await?;
    }
    Ok(())
}
async fn post_saved_terminal(client: &AuditClient, state: &PlanState) -> Result<()> {
    let checkpoint = state.checkpoint()?;
    if !checkpoint.output_acknowledged
        && let Some(event) = checkpoint.output_event
    {
        client.post_control_event(event).await?;
        state.update(|plan| {
            plan.managed
                .as_mut()
                .context("missing checkpoint")?
                .output_acknowledged = true;
            Ok(())
        })?;
    }
    Ok(())
}

fn build_command_requested_event(plan: &RunPlan) -> Result<Value> {
    let mut requested = event(
        plan,
        "command.requested",
        json!({
            "request_id": plan.request.request_id,
            "profile": plan.options.profile,
            "network": plan.options.network,
            "image": plan.options.image,
            "runtime": plan.options.runtime,
            "provider": plan.options.provider,
            "mediation": launch_mediation_metadata(),
        }),
    );
    if let Some(mode) = plan.request.execution_mode {
        requested["data"]["execution_mode"] = serde_json::to_value(mode)?;
    }
    if let Some(resolution) = &plan.request.provider_resolution {
        requested["data"]["provider_resolution"] = serde_json::to_value(resolution)?;
    }
    requested["command"] = serde_json::to_value(&plan.request.command)?;
    Ok(requested)
}

pub(super) async fn execute_plan(
    mut plan: RunPlan,
    credentials: Option<CredentialFile>,
    client: AuditClient,
    supplied: Option<String>,
    config: crate::control_fs::Directory,
) -> Result<RunSummary> {
    anyhow::ensure!(
        config.path() == plan.options.config_dir,
        "retained configuration differs from this run"
    );
    config.revalidate()?;
    anyhow::ensure!(
        matches!(plan.version, 2 | 3) && !plan.operation_id.is_nil(),
        "legacy pending plans cannot launch workers; create a new audited request"
    );
    validate_provider_plan(&plan)?;
    let approved = approved_runtime(&plan.options, None)?;
    let _admission_guard =
        AttemptGuard::acquire(&plan.options.config_dir, plan.operation_id, None)?;
    let sessions = Directory::root(&plan.options.config_dir)?.child("sessions")?;
    anyhow::ensure!(
        sessions
            .read::<RunPlan>(&format!("{}.json", plan.session))?
            .is_none(),
        "this session already has a durable checkpoint; use worker-resume instead of launching again"
    );
    let requested = build_command_requested_event(&plan)?;
    client.post_control_event(requested).await?;
    let (receipt, decision) = authorize_plan(&mut plan, &client, supplied.as_deref()).await?;
    let tenant = check_identity(&client, None).await?;
    let (authority_file, seed, image_id) = prepare_authority(&plan, &approved).await?;
    let project = plan.options.project.clone();
    let excluded_control = config.try_clone_file()?;
    let prepared = tokio::task::spawn_blocking(move || {
        workspace::prepare_workspace_excluding(&project, Limits::default(), &[&excluded_control])
    })
    .await??;
    let archive_name = format!("{}.tar", plan.operation_id);
    let archive_digest = sessions.copy_archive(&archive_name, prepared.archive.reopen()?)?;
    let launch = LaunchSpecV1 {
        version: 1,
        session_id: plan.session.to_string(),
        command: plan.request.command.clone(),
        profile: plan.options.profile.clone(),
        runtime_kind: runtime_kind(plan.options.runtime),
        authority_id: seed.authority_id.to_string(),
        image_ref: plan.options.image.clone(),
        image_id,
        network: network_kind(plan.options.network),
        cpu_millis: 2000,
        memory_bytes: 1024 * 1024 * 1024,
        pids_limit: 256,
        timeout_seconds: u32::try_from(plan.options.timeout)?,
        security_preset: if plan.options.interactive {
            SecurityPresetV1::FixedSandboxInteractiveV3
        } else {
            SecurityPresetV1::FixedSandboxV2
        },
        restart_policy: RestartPolicyV1::No,
        workspace_manifest_digest: content_hash(
            "ai-guard:workspace-manifest:v1",
            &prepared.manifest,
        )?,
        workspace_import_policy_version: 1,
        provider: plan.options.provider.clone(),
        capture_output: plan.options.capture_output,
        credential_binding_id: plan
            .request
            .provider_resolution
            .as_ref()
            .map(|resolution| resolution.credential_binding_id.to_string())
            .or_else(|| credentials.as_ref().map(|_| Uuid::new_v4().to_string())),
        credential_names: credentials
            .as_ref()
            .map(|credentials| credentials.names.clone())
            .unwrap_or_default(),
        provider_resolution: plan.request.provider_resolution.clone(),
    };
    let admission = AdmissionV1 {
        version: 1,
        operation_id: plan.operation_id.to_string(),
        decision_request: plan.request.clone(),
        receipt,
        launch,
    };
    client.validate_identity_payload(&admission)?;
    plan.managed = Some(ManagedCheckpoint {
        tenant_id: tenant,
        gateway_origin: configured_origin(&client),
        config_root: plan.options.config_dir.clone(),
        binding: seed.binding,
        authority_file,
        admission: admission.clone(),
        worker: None,
        owned: None,
        pending: None,
        uncertain: Vec::new(),
        lock_identity: None,
        before: prepared.manifest.clone(),
        import: None,
        decision: client.redactor.value(&decision),
        output_event: None,
        output_acknowledged: false,
        result: None,
        archive_name,
        archive_digest,
        credential_digest: credential_digest(credentials.as_ref())?,
        workspace_control_exclusion_version: Some(1),
        invocation_event: None,
    });
    let state = PlanState::new(plan)?;
    let admitted: AdmissionResultV1 = client
        .post_control("api/v1/workers/reservations", &admission)
        .await?;
    anyhow::ensure!(
        admitted.receipt.audit_seq > 0
            && admitted.receipt.operation_id == admission.operation_id
            && admitted.receipt.input_hash == admission.input_hash()?,
        "gateway did not acknowledge this exact durable admission"
    );
    anyhow::ensure!(
        admitted.accepted,
        "worker admission refused ({:?}); no volume, import or worker was created",
        admitted.refusal
    );
    let worker = admitted
        .worker
        .context("accepted admission lacks its durable worker")?;
    validate_worker(&state.snapshot()?, &worker)?;
    let guard = AttemptGuard::acquire(
        &state.snapshot()?.options.config_dir,
        Uuid::parse_str(&worker.attempt.attempt_id)?,
        None,
    )?;
    let current: WorkerV1 = client
        .get_typed(&format!("api/v1/workers/{}", worker.worker_id))
        .await?;
    anyhow::ensure!(
        current.state == WorkerStateV1::Reserved && current.revision == worker.revision,
        "worker progressed while admission acknowledgement was pending; use worker-resume; no launch was repeated"
    );
    let fresh: RunPlan = state
        .directory
        .read(&state.name)?
        .context("session checkpoint disappeared before admission acknowledgement")?;
    validate_checkpoint(&fresh, &client)?;
    let state = PlanState {
        plan: Arc::new(Mutex::new(fresh)),
        directory: state.directory.clone(),
        name: state.name.clone(),
    };
    anyhow::ensure!(
        state
            .checkpoint()?
            .lock_identity
            .is_none_or(|expected| expected == guard.identity()),
        "attempt lock changed during admission recovery"
    );
    state.update(|plan| {
        let checkpoint = plan.managed.as_mut().context("missing checkpoint")?;
        checkpoint.worker = Some(current);
        checkpoint.lock_identity = Some(guard.identity());
        Ok(())
    })?;
    let runtime = bound_runtime(&state, &client, None, false).await?;
    runtime
        .doctor(state.snapshot()?.options.allow_rootful_runtime)
        .await?;
    mutate(&client, &state, WorkerTransitionV1::Prepare).await?;
    import_workspace(&runtime, &client, &state, &guard).await?;
    mutate(&client, &state, WorkerTransitionV1::CreateIssued).await?;
    launch_worker(
        &runtime,
        &client,
        &state,
        &guard,
        credentials.as_ref(),
        None,
    )
    .await
}
fn require_control_exclusion_for_first_start(state: &PlanState) -> Result<()> {
    let worker = state.worker()?;
    require_current_preset_for_first_start(
        worker.launch.security_preset,
        worker.quota_held,
        worker.attempt.start_operation_id.as_deref(),
    )?;
    if worker.quota_held && worker.attempt.start_operation_id.is_none() {
        anyhow::ensure!(
            state.checkpoint()?.workspace_control_exclusion_version == Some(1),
            "this legacy immutable workspace lacks control directory exclusion proof; create a fresh audited run; existing reservation, archive and claim evidence were retained"
        );
    }
    Ok(())
}
fn require_current_preset_for_first_start(
    preset: SecurityPresetV1,
    quota_held: bool,
    start_operation_id: Option<&str>,
) -> Result<()> {
    anyhow::ensure!(
        !quota_held || start_operation_id.is_some() || preset.requires_no_engine_logs(),
        "legacy fixed_sandbox_v1 cannot first-start with this broker; create a fresh audited request with fixed_sandbox_v2; existing reservation, archive and claim evidence were retained"
    );
    Ok(())
}

#[cfg(test)]
mod first_start_policy_tests {
    use super::*;

    #[test]
    fn legacy_first_start_is_refused_but_consumed_and_terminal_work_can_be_observed() {
        let error =
            require_current_preset_for_first_start(SecurityPresetV1::FixedSandboxV1, true, None)
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("fixed_sandbox_v1 cannot first-start")
        );
        assert!(
            error
                .to_string()
                .contains("reservation, archive and claim evidence were retained")
        );
        require_current_preset_for_first_start(
            SecurityPresetV1::FixedSandboxV1,
            true,
            Some("consumed"),
        )
        .unwrap();
        require_current_preset_for_first_start(SecurityPresetV1::FixedSandboxV1, false, None)
            .unwrap();
        require_current_preset_for_first_start(SecurityPresetV1::FixedSandboxV2, true, None)
            .unwrap();
    }
}
async fn launch_worker(
    runtime: &ManagedRuntime,
    client: &AuditClient,
    state: &PlanState,
    guard: &AttemptGuard,
    credentials: Option<&CredentialFile>,
    recovered: Option<OwnedContainerSnapshot>,
) -> Result<RunSummary> {
    require_control_exclusion_for_first_start(state)?;
    let plan = state.snapshot()?;
    let worker = state.worker()?;
    let spec = worker_spec(&plan, &worker, credentials)?;
    let decision = state.checkpoint()?.decision;
    let mut submitted = event(
        &plan,
        "runtime.submitted",
        json!({"container":worker.attempt.stable_name,"volume":worker.attempt.volume_name,"worker_id":worker.worker_id,"attempt_id":worker.attempt.attempt_id,"network":plan.options.network,"coverage":"top-level runtime lifecycle; nested calls are not mediated"}),
    );
    submitted["command"] = serde_json::to_value(&plan.request.command)?;
    submitted["decision"] = decision;
    client.post_control_event(submitted).await?;
    if plan.options.interactive {
        // Acknowledged before the durable start claim, no keypresses or screen contents.
        emit(client, &plan, "runtime.terminal.attach.authorized", json!({"execution_mode":plan.request.execution_mode,"security_preset":worker.launch.security_preset,"worker_id":worker.worker_id,"attempt_id":worker.attempt.attempt_id,"reattach":false})).await?;
    }
    let execution_deadline =
        tokio::time::Instant::now() + Duration::from_secs(plan.options.timeout);
    let supervision =
        control_monitor::PrestartOwner::new(runtime, client, state, guard, execution_deadline)?;
    let on_created = |created: OwnedContainerSnapshot| {
        let state = state.clone();
        let client = client.clone();
        let guard = &guard;
        let supervision = supervision.clone();
        async move {
            guard.ensure_current()?;
            state.update(|plan| {
                plan.managed.as_mut().context("missing checkpoint")?.owned = Some(created.clone());
                Ok(())
            })?;
            supervision.ensure_live()?;
            let worker = state.worker()?;
            if matches!(
                worker.state,
                WorkerStateV1::Preparing | WorkerStateV1::InDoubt
            ) && worker.attempt.container_id.is_none()
                && worker.attempt.created_inspection.is_none()
                && worker.attempt.start_operation_id.is_none()
            {
                mutate(
                    &client,
                    &state,
                    WorkerTransitionV1::Created {
                        inspection: inspection(&worker, &created.observation)?,
                    },
                )
                .await?;
            } else {
                anyhow::ensure!(
                    worker.state == WorkerStateV1::Created
                        && worker.attempt.created_inspection.as_ref()
                            == Some(&inspection(&worker, &created.observation)?),
                    "recovered Created instance conflicts with durable control"
                );
            }
            supervision.ensure_live()?;
            if state.worker()?.attempt.start_operation_id.is_some() {
                return Ok(StartDirective::ObserveOnly);
            }
            guard.ensure_current()?;
            let claimed = mutate(
                &client,
                &state,
                WorkerTransitionV1::StartClaim {
                    emitter_id: Uuid::new_v4().to_string(),
                },
            )
            .await?;
            guard.ensure_current()?;
            supervision.ensure_live()?;
            Ok(
                if claimed.disposition == Some(ClaimDispositionV1::EmitOnce) {
                    StartDirective::EmitOnceWithVeto(supervision.veto(created)?)
                } else {
                    StartDirective::ObserveOnly
                },
            )
        }
    };
    let (start_acknowledged, started) = tokio::sync::watch::channel(false);
    let on_started = |observed: ContainerObservation| {
        let state = state.clone();
        let client = client.clone();
        let start_acknowledged = start_acknowledged.clone();
        async move {
            record_observation(&client, &state, &observed).await?;
            let plan = state.snapshot()?;
            let mut event = event(
                &plan,
                "command.started",
                json!({"runtime_started_at":observed.process.started_at,"runtime_state":observed.process.status,"running":observed.process.running}),
            );
            event["command"] = serde_json::to_value(&plan.request.command)?;
            client.post_control_event_once(event).await?;
            // Tolerance begins only after both Running and command.started
            // have been acknowledged. The callback itself has no retry here.
            if state.worker()?.state == WorkerStateV1::Running {
                let _ = start_acknowledged.send(true);
            }
            Ok(())
        }
    };
    let (done, completed) = tokio::sync::watch::channel(false);
    // The original earlier deadline also bounds the first successful control
    // read. It is never rebased after readiness, claim or final admission.
    let execute = async {
        let result = async {
            supervision.wait_ready().await?;
            if let Some(recovered) = &recovered {
                runtime
                    .start_owned_retained(
                        &spec,
                        recovered,
                        None,
                        Duration::from_secs(plan.options.timeout),
                        256 * 1024,
                        on_created,
                        on_started,
                    )
                    .await
            } else {
                runtime
                    .execute_retained(
                        &spec,
                        None,
                        Duration::from_secs(plan.options.timeout),
                        256 * 1024,
                        on_created,
                        on_started,
                    )
                    .await
            }
        }
        .await;
        supervision.execution_finished();
        let _ = done.send(true);
        result
    };
    let monitor = control_monitor::run(
        runtime,
        client,
        state,
        guard,
        completed,
        started,
        execution_deadline,
        supervision.clone(),
    );
    let (execution, monitor) = tokio::join!(execute, monitor);
    // Recover only a native completion qualified by this same runtime. The
    // carrier's original cause/contexts remain owned beside that completion.
    let (execution, execution_error) = match execution {
        Ok(retained) => (Ok(retained), None),
        Err(error) => match runtime.take_completed_legacy_failure(&spec, error) {
            Ok((retained, error)) => (Ok(retained), Some(error)),
            Err(error) => (Err(error), None),
        },
    };
    if let Err(control) = monitor {
        let reason = client.redactor.text(&control.to_string());
        return match execution {
            Err(error) => {
                // Before start or unresolved effect: retain the actual error,
                // owned snapshot/IO, original control cause and audit refusal.
                let audit_error = try_mark_unknown(client, state, &reason).await.err();
                Err(control_monitor::RejectedLaunch {
                    cause: control.into(),
                    diagnostic: reason,
                    execution: None,
                    execution_error: Some(error),
                    summary: None,
                    finalization_error: None,
                    audit_error,
                }
                .into())
            }
            Ok(retained) => {
                reject_completed_execution(
                    runtime,
                    client,
                    state,
                    retained,
                    control.into(),
                    execution_error,
                    reason,
                )
                .await
            }
        };
    }
    let retained = match execution {
        Ok(retained) => retained,
        Err(error) => {
            mark_unknown(client, state, &redacted_error(&client.redactor, &error)).await;
            return Err(error);
        }
    };
    if let Some(error) = execution_error {
        let reason = redacted_error(&client.redactor, &error);
        return reject_completed_execution(runtime, client, state, retained, error, None, reason)
            .await;
    }
    let summary = match finish_execution(runtime, client, state, &retained, None).await {
        Ok(summary) => summary,
        Err(error) => {
            mark_unknown(
                client,
                state,
                &format!(
                    "terminal persistence or finalization failed: {}",
                    redacted_error(&client.redactor, &error)
                ),
            )
            .await;
            return Err(error);
        }
    };
    Ok(summary)
}

// A later observer failure does not erase a fully qualified native terminal.
// Finalization uses the existing durable ACK/usage/cleanup protocol exactly
// once; the result, primary cause and any subsequent refusal remain returned.
async fn reject_completed_execution(
    runtime: &ManagedRuntime,
    client: &AuditClient,
    state: &PlanState,
    retained: crate::runtime::ManagedRetainedExecution,
    cause: anyhow::Error,
    execution_error: Option<anyhow::Error>,
    reason: String,
) -> Result<RunSummary> {
    let control_deadline = cause
        .downcast_ref::<control_monitor::ControlFailure>()
        .and_then(control_monitor::ControlFailure::deadline_observation);
    let (summary, finalization_error, audit_error) =
        match finish_execution(runtime, client, state, &retained, control_deadline).await {
            Ok(mut summary) => {
                let prior = summary.postprocessing_error.take();
                summary.postprocessing_error = Some(crate::redaction::clip(
                    &match prior {
                        Some(prior) => format!("{prior}; execution observation failed: {reason}"),
                        None => format!("execution observation failed: {reason}"),
                    },
                    8192,
                ));
                let audit_error = state
                    .update(|plan| {
                        plan.managed.as_mut().context("missing checkpoint")?.result =
                            Some(summary.clone());
                        Ok(())
                    })
                    .err();
                (Some(summary), None, audit_error)
            }
            Err(error) => {
                // Only a still-held allocation may become InDoubt. A terminal
                // already durably acknowledged is never rewritten as unknown.
                let audit_error = try_mark_unknown(client, state, &reason).await.err();
                (None, Some(error), audit_error)
            }
        };
    Err(control_monitor::RejectedLaunch {
        cause,
        diagnostic: reason,
        execution: Some(retained),
        execution_error,
        summary,
        finalization_error,
        audit_error,
    }
    .into())
}

fn helper_spec(
    plan: &RunPlan,
    worker: &WorkerV1,
    purpose: ContainerPurpose,
    operation: Uuid,
    argv: Vec<String>,
) -> Result<ContainerSpec> {
    ContainerSpec::with_operation(
        plan.session,
        purpose,
        &worker.launch.image_id,
        argv,
        NetworkMode::None,
        operation,
    )?
    .with_managed_names(
        &format!("{}-h-{}", worker.attempt.stable_name, operation.simple()),
        &worker.attempt.volume_name,
    )
}
async fn import_workspace(
    runtime: &ManagedRuntime,
    client: &AuditClient,
    state: &PlanState,
    guard: &AttemptGuard,
) -> Result<()> {
    guard.ensure_current()?;
    let plan = state.snapshot()?;
    let checkpoint = managed(&plan)?;
    let worker = state.worker()?;
    anyhow::ensure!(
        checkpoint.import.is_none(),
        "workspace import has a prior emission checkpoint; inspect or stop it before another import"
    );
    emit(client,&plan,"workspace.import.requested",json!({"volume":worker.attempt.volume_name,"files":checkpoint.before.len(),"original_mounted":false})).await?;
    let namespace = worker
        .attempt
        .stable_name
        .strip_prefix("aig-")
        .and_then(|name| name.split('-').next())
        .context("server namespace missing")?;
    runtime
        .create_named_volume(
            &worker.attempt.volume_name,
            &[
                (
                    "agentsandbox.authority".into(),
                    worker.launch.authority_id.clone(),
                ),
                ("agentsandbox.worker".into(), worker.worker_id.clone()),
                ("agentsandbox.namespace".into(), namespace.into()),
            ],
        )
        .await?;
    let operation = Uuid::new_v4();
    state.update(|plan| {
        plan.managed.as_mut().context("missing checkpoint")?.import = Some(HelperCheckpoint {
            operation_id: operation,
            owned: None,
            start_issued: false,
            completed: false,
        });
        Ok(())
    })?;
    let spec = helper_spec(
        &plan,
        &worker,
        ContainerPurpose::Import,
        operation,
        vec![
            "/bin/tar".into(),
            "--extract".into(),
            "--file=-".into(),
            "--directory=/workspace".into(),
            "--no-same-owner".into(),
            "--no-same-permissions".into(),
        ],
    )?;
    let input = state
        .directory
        .archive(&checkpoint.archive_name, &checkpoint.archive_digest)?;
    let retained=runtime.execute_retained(&spec,Some(input),Duration::from_secs(120),16*1024,|owned|{
        let state=state.clone();let client=client.clone();let plan=plan.clone();async move {
            state.update(|plan|{let helper=plan.managed.as_mut().and_then(|m|m.import.as_mut()).context("import checkpoint missing")?;anyhow::ensure!(!helper.start_issued,"import start was already consumed");helper.owned=Some(owned.clone());helper.start_issued=true;Ok(())})?;
            emit(&client,&plan,"workspace.import.created",json!({"container_id":owned.handle.id(),"operation_id":operation})).await?;Ok(StartDirective::EmitOnce)
        }
    },|observation|{let client=client.clone();let plan=plan.clone();async move{emit(&client,&plan,"workspace.import.started",json!({"runtime_started_at":observation.process.started_at,"running":observation.process.running})).await}}).await?;
    anyhow::ensure!(
        retained.result().state.exit_code == 0 && !retained.result().timed_out,
        "workspace import failed; owned helper and volume retained"
    );
    emit(client,&plan,"workspace.import.completed",json!({"files":checkpoint.before.len(),"manifest_digest":worker.launch.workspace_manifest_digest})).await?;
    state.update(|plan| {
        plan.managed
            .as_mut()
            .and_then(|m| m.import.as_mut())
            .context("import checkpoint missing")?
            .completed = true;
        Ok(())
    })?;
    runtime.cleanup(&retained).await?;
    Ok(())
}
async fn managed_archive(
    runtime: &ManagedRuntime,
    client: &AuditClient,
    plan: &RunPlan,
    worker: &WorkerV1,
) -> Result<Vec<u8>> {
    emit(
        client,
        plan,
        "workspace.scan.requested",
        json!({"volume":worker.attempt.volume_name}),
    )
    .await?;
    let operation = Uuid::new_v4();
    let spec = helper_spec(
        plan,
        worker,
        ContainerPurpose::Export,
        operation,
        vec![
            "/bin/tar".into(),
            "--create".into(),
            "--file=-".into(),
            "--directory=/workspace".into(),
            ".".into(),
        ],
    )?;
    let retained = runtime
        .execute_retained(
            &spec,
            None,
            Duration::from_secs(120),
            Limits::default().max_archive_bytes(),
            |owned| {
                let client = client.clone();
                let plan = plan.clone();
                async move {
                    emit(
                        &client,
                        &plan,
                        "workspace.scan.created",
                        json!({"container_id":owned.handle.id(),"operation_id":operation}),
                    )
                    .await?;
                    Ok(StartDirective::EmitOnce)
                }
            },
            |observation| {
                let client = client.clone();
                let plan = plan.clone();
                async move {
                    emit(
                        &client,
                        &plan,
                        "workspace.scan.started",
                        json!({"runtime_started_at":observation.process.started_at}),
                    )
                    .await
                }
            },
        )
        .await?;
    anyhow::ensure!(
        retained.result().state.exit_code == 0
            && !retained.result().timed_out
            && !retained.result().stdout.truncated,
        "workspace scan failed or exceeded its archive bound"
    );
    emit(
        client,
        plan,
        "workspace.scan.completed",
        json!({"archive_bytes":retained.result().stdout.total_bytes}),
    )
    .await?;
    let bytes = retained.result().stdout.bytes.clone();
    runtime.cleanup(&retained).await?;
    Ok(bytes)
}
async fn managed_scan(
    runtime: &ManagedRuntime,
    client: &AuditClient,
    plan: &RunPlan,
    worker: &WorkerV1,
) -> Result<Manifest> {
    let bytes = managed_archive(runtime, client, plan, worker).await?;
    tokio::task::spawn_blocking(move || {
        workspace::scan_workspace_archive(bytes.as_slice(), Limits::default())
    })
    .await?
}

fn postprocessing_diagnostic(client: &AuditClient, error: &anyhow::Error) -> String {
    crate::redaction::clip(&redacted_error(&client.redactor, error), 8160)
}
fn manifest_mode_coverage(manifest: &Manifest) -> Result<&'static str> {
    if manifest.is_empty() {
        return Ok("not_applicable");
    }
    let mut known = 0;
    for fingerprint in manifest.values() {
        if serde_json::to_value(fingerprint)?
            .get("mode")
            .is_some_and(|mode| !mode.is_null())
        {
            known += 1;
        }
    }
    Ok(if known == manifest.len() {
        "complete"
    } else if known == 0 {
        "unknown"
    } else {
        "partial"
    })
}
async fn publish_trusted_diff(
    runtime: Option<&ManagedRuntime>,
    client: &AuditClient,
    state: &PlanState,
    recovered: bool,
) -> Result<()> {
    if load_postprocessing(state, client)?
        .context("postprocessing journal missing")?
        .payload
        .diff
        .is_none()
    {
        let plan = state.snapshot()?;
        let before = state.checkpoint()?.before;
        let after = managed_scan(
            runtime.context("trusted diff recovery requires its bound runtime")?,
            client,
            &plan,
            &state.worker()?,
        )
        .await?;
        let changes = workspace::diff_manifests(&before, &after);
        let count = changes.len();
        let observation = Uuid::new_v4();
        let phase = if recovered {
            "post_completion_recovery"
        } else {
            "after_completion"
        };
        let before_digest = content_hash("ai-guard:workspace-manifest:v1", &before)?;
        let after_digest = content_hash("ai-guard:workspace-manifest:v1", &after)?;
        let completed=client.redactor.value(&event(&plan,"workspace.diff.completed",json!({"observation_id":observation,"observation_phase":phase,"changed_files":count,"emitted_changes":0,"omitted_changes":count,"truncated":count>0,"omission_reasons":[],"limits":{"maximum_events":MAX_FILE_EVENTS,"maximum_event_bytes":MAX_RETAINED_FILE_EVENT_BYTES,"maximum_journal_bytes":MAX_POSTPROCESSING_BYTES,"reserved_growth_bytes":POSTPROCESSING_GROWTH_RESERVE},"before_manifest_digest":before_digest,"after_manifest_digest":after_digest,"comparison_coverage":{"content":"observed","baseline_modes":manifest_mode_coverage(&before)?,"observed_modes":manifest_mode_coverage(&after)?}})));
        let mut prepared = PreparedDiffV1 {
            observation_id: observation,
            before_digest,
            after_digest,
            changed_files: count,
            files: Vec::new(),
            acknowledged_files: 0,
            completed,
            completed_acknowledged: false,
        };
        let mut omission_reasons = Vec::new();
        if count > MAX_FILE_EVENTS {
            omission_reasons.push("event_count_limit");
        }
        let mut candidate =
            load_postprocessing(state, client)?.context("postprocessing journal missing")?;
        for change in changes.into_iter().take(MAX_FILE_EVENTS) {
            let mut data = serde_json::to_value(&change)?;
            data["observation_id"] = json!(observation);
            data["observation_phase"] = json!(phase);
            data["path_redacted"] = json!(client.redactor.text(&change.path) != change.path);
            data["path_display_ambiguous"] = json!(false);
            let mut file =
                client
                    .redactor
                    .value(&event(&plan, &format!("file.{}", change.change), data));
            let change_id = file["id"].clone();
            file["data"]["change_id"] = change_id;
            if serde_json::to_vec(&file)?.len() > MAX_RETAINED_FILE_EVENT_BYTES {
                omission_reasons.push("event_payload_limit");
                break;
            }
            prepared.files.push(file);
            update_diff_omissions(&mut prepared, &omission_reasons);
            candidate.payload.diff = Some(prepared.clone());
            if serde_json::to_vec(&candidate)?.len()
                > MAX_POSTPROCESSING_BYTES - POSTPROCESSING_GROWTH_RESERVE
            {
                prepared.files.pop();
                omission_reasons.push("journal_byte_budget");
                break;
            }
        }
        update_diff_omissions(&mut prepared, &omission_reasons);
        mark_path_display_ambiguity(&mut prepared.files)?;
        prepared.completed["timestamp"] =
            json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        // The bounded exact batch is synced before its first HTTP publication.
        update_postprocessing(state, client, |payload| {
            payload.diff = Some(prepared);
            Ok(())
        })?;
    }
    loop {
        let journal =
            load_postprocessing(state, client)?.context("postprocessing journal missing")?;
        let diff = journal.payload.diff.context("prepared file diff missing")?;
        if let Some(event) = diff.files.get(diff.acknowledged_files) {
            client.post_event(event.clone()).await?;
            update_postprocessing(state, client, |payload| {
                payload
                    .diff
                    .as_mut()
                    .context("prepared diff missing")?
                    .acknowledged_files += 1;
                Ok(())
            })?;
        } else {
            if !diff.completed_acknowledged {
                client.post_control_event(diff.completed).await?;
                update_postprocessing(state, client, |payload| {
                    payload
                        .diff
                        .as_mut()
                        .context("prepared diff missing")?
                        .completed_acknowledged = true;
                    Ok(())
                })?;
            }
            return Ok(());
        }
    }
}

fn update_diff_omissions(diff: &mut PreparedDiffV1, reasons: &[&str]) {
    diff.completed["data"]["emitted_changes"] = json!(diff.files.len());
    diff.completed["data"]["omitted_changes"] = json!(diff.changed_files - diff.files.len());
    diff.completed["data"]["truncated"] = json!(diff.changed_files > diff.files.len());
    diff.completed["data"]["omission_reasons"] = json!(reasons);
}

fn mark_path_display_ambiguity(files: &mut [Value]) -> Result<()> {
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for file in files.iter() {
        let path = file["data"]["path"]
            .as_str()
            .context("prepared display path missing")?;
        *counts.entry(path.to_owned()).or_default() += 1;
    }
    for file in files.iter_mut() {
        let path = file["data"]["path"]
            .as_str()
            .context("prepared display path missing")?;
        let ambiguous = counts.get(path).is_some_and(|count| *count > 1);
        file["data"]["path_display_ambiguous"] = json!(ambiguous);
    }
    Ok(())
}

async fn deliver_saved_coverage(client: &AuditClient, state: &PlanState) -> Result<()> {
    let payload = load_postprocessing(state, client)?
        .context("postprocessing journal missing")?
        .payload;
    if !payload.coverage_acknowledged
        && let Some(event) = payload.coverage
    {
        match client.post_control_event(event).await {
            Ok(_) => update_postprocessing(state, client, |payload| {
                payload.coverage_acknowledged = true;
                payload.coverage_error = None;
                Ok(())
            })?,
            Err(error) => {
                let diagnostic = postprocessing_diagnostic(client, &error);
                update_postprocessing(state, client, |payload| {
                    payload.coverage_error = Some(diagnostic.clone());
                    payload.previous_delivery_error = Some(diagnostic);
                    Ok(())
                })?;
                return Err(error);
            }
        }
    }
    Ok(())
}

async fn publish_postprocessing_coverage(client: &AuditClient, state: &PlanState) -> Result<()> {
    // Never replace an unacknowledged coverage Event with newer content under
    // its identity. Deliver that exact historical state before a new revision.
    deliver_saved_coverage(client, state).await?;
    let plan = state.snapshot()?;
    let payload = load_postprocessing(state, client)?
        .context("postprocessing journal missing")?
        .payload;
    let complete = payload
        .diff
        .as_ref()
        .is_some_and(|diff| diff.completed_acknowledged)
        && [
            DeclaredCollectionV1::Complete,
            DeclaredCollectionV1::UnavailableForTty,
        ]
        .contains(&payload.declared.state)
        && payload.diff_error.is_none()
        && payload.declared.error.is_none();
    let mut declared = payload.declared.clone();
    declared.error = declared
        .error
        .map(|error| crate::redaction::clip(&error, 1000));
    let mut data = json!({"revision":payload.coverage_revision,"provider":plan.options.provider,"collection":"lifecycle_and_workspace_diff","declared_collection":declared,"normalized_events":payload.declared.normalized_events,"posted_events":payload.declared.acknowledged_events,"dropped_events":payload.declared.dropped_events,"maximum_events":crate::adapters::MAX_EVENTS_PER_CAPTURE,"stdout_truncated":payload.declared.stdout_truncated,"unterminated_final_line_omitted":payload.declared.unterminated_final_line_omitted,"unknown_or_oversized_lines":"ignored","nested_authorization":false,"raw_output_capture":plan.options.capture_output,"postprocessing_complete":complete,"trusted_diff":if payload.diff.as_ref().is_some_and(|diff|diff.completed_acknowledged){"complete"}else{"pending"},"workspace_error":payload.diff_error.as_ref().map(|error|crate::redaction::clip(error,1000)),"previous_coverage_delivery_error":payload.previous_delivery_error.as_ref().map(|error|crate::redaction::clip(error,1000)),"baseline_mode_coverage":manifest_mode_coverage(&state.checkpoint()?.before)?});
    let diff_truncated = payload
        .diff
        .as_ref()
        .is_some_and(|diff| diff.changed_files > diff.files.len());
    data["trusted_diff_events_truncated"] = payload
        .diff
        .as_ref()
        .map(|_| json!(diff_truncated))
        .unwrap_or(Value::Null);
    let (phase, prepared, acknowledged) = match payload.diff.as_ref() {
        None => ("not_prepared", None, None),
        Some(diff) => (
            if diff.completed_acknowledged {
                "complete"
            } else if diff.acknowledged_files < diff.files.len() {
                "files_pending"
            } else {
                "summary_pending"
            },
            Some(diff.files.len()),
            Some(diff.acknowledged_files),
        ),
    };
    data["file_delivery"] = json!({"version":1,"phase":phase,"prepared_changes":prepared,"acknowledged_changes":acknowledged});
    data["trusted_diff_changed_files"] = payload
        .diff
        .as_ref()
        .map(|diff| json!(diff.changed_files))
        .unwrap_or(Value::Null);
    if payload
        .coverage
        .as_ref()
        .is_some_and(|event| event["data"] == data)
    {
        return Ok(());
    }
    let revision = payload
        .coverage_revision
        .checked_add(1)
        .context("coverage revision overflow")?;
    data["revision"] = json!(revision);
    let mut coverage = event(&plan, "observer.coverage", data);
    coverage["coverage"] = json!(if complete
        && !diff_truncated
        && !payload.declared.stdout_truncated.unwrap_or(false)
        && payload.declared.normalized_events != Some(crate::adapters::MAX_EVENTS_PER_CAPTURE)
    {
        "generic"
    } else {
        "partial"
    });
    let coverage = client.redactor.value(&coverage);
    update_postprocessing(state, client, |payload| {
        payload.coverage_revision = revision;
        payload.coverage = Some(coverage);
        payload.coverage_acknowledged = false;
        Ok(())
    })?;
    deliver_saved_coverage(client, state).await
}

fn sync_postprocessing_summary(client: &AuditClient, state: &PlanState) -> Result<()> {
    let payload = load_postprocessing(state, client)?
        .context("postprocessing journal missing")?
        .payload;
    let count = payload
        .diff
        .as_ref()
        .filter(|diff| diff.completed_acknowledged)
        .map(|diff| diff.changed_files);
    let mut errors = Vec::new();
    for error in [
        &payload.diff_error,
        &payload.declared.error,
        &payload.coverage_error,
    ]
    .into_iter()
    .flatten()
    {
        errors.push(error.clone());
    }
    if count.is_none() && payload.diff_error.is_none() {
        errors.push("trusted workspace diff remains unacknowledged".into());
    }
    if !payload.coverage_acknowledged && payload.coverage_error.is_none() {
        errors.push("postprocessing coverage remains unacknowledged".into());
    }
    let owned_diagnostic =
        (!errors.is_empty()).then(|| crate::redaction::clip(&errors.join("; "), 8160));
    let Some(mut summary) = state.checkpoint()?.result else {
        if let Some(error) = owned_diagnostic {
            bail!("postprocessing recovery incomplete: {error}");
        }
        return Ok(());
    };
    // Resolve a cut around the previous two-file projection before deciding
    // ownership. A different current value belongs to another producer and is
    // retained opaquely; its text never determines a failure category.
    let mut projection = payload.summary_projection;
    if let Some(previous) = &mut projection
        && !previous.acknowledged
        && (summary.postprocessing_error == previous.intended
            || content_hash("ai-guard:summary-error:v1", &summary.postprocessing_error)?
                == previous.previous_digest)
    {
        if summary.postprocessing_error != previous.intended {
            state.update(|plan| {
                let result = plan
                    .managed
                    .as_mut()
                    .context("missing checkpoint")?
                    .result
                    .as_mut()
                    .context("summary disappeared")?;
                result.postprocessing_error = previous.intended.clone();
                Ok(())
            })?;
            summary.postprocessing_error = previous.intended.clone();
        }
        previous.acknowledged = true;
        update_postprocessing(state, client, |payload| {
            payload.summary_projection = projection.clone();
            Ok(())
        })?;
    }
    let external = match &projection {
        Some(previous)
            if previous.acknowledged && summary.postprocessing_error == previous.intended =>
        {
            previous.external.clone()
        }
        _ => summary
            .postprocessing_error
            .as_ref()
            .map(|error| crate::redaction::clip(&client.redactor.text(error), 8192)),
    };
    let diagnostic = match (&external, &owned_diagnostic) {
        (None, None) => None,
        (Some(error), None) | (None, Some(error)) => Some(error.clone()),
        (Some(external), Some(owned)) if external == owned => Some(external.clone()),
        (Some(external), Some(owned)) => Some(crate::redaction::clip(
            &format!("{external}; {owned}"),
            8192,
        )),
    };
    if summary.changed_files == count
        && summary.postprocessing_error == diagnostic
        && projection.as_ref().is_some_and(|previous| {
            previous.acknowledged
                && previous.intended == diagnostic
                && previous.external == external
        })
    {
        return Ok(());
    }
    // Record the exact intended projection before changing only existing plan
    // fields. A crash never grants ownership over a different external value.
    let intent = SummaryProjectionV1 {
        version: 1,
        previous_digest: content_hash("ai-guard:summary-error:v1", &summary.postprocessing_error)?,
        intended: diagnostic.clone(),
        external,
        acknowledged: false,
    };
    update_postprocessing(state, client, |payload| {
        payload.summary_projection = Some(intent);
        Ok(())
    })?;
    state.update(|plan| {
        if let Some(summary) = plan
            .managed
            .as_mut()
            .context("missing checkpoint")?
            .result
            .as_mut()
        {
            summary.changed_files = count;
            summary.postprocessing_error = diagnostic.clone();
        }
        Ok(())
    })?;
    update_postprocessing(state, client, |payload| {
        payload
            .summary_projection
            .as_mut()
            .context("summary projection missing")?
            .acknowledged = true;
        Ok(())
    })?;
    Ok(())
}

async fn complete_postprocessing(
    runtime: Option<&ManagedRuntime>,
    client: &AuditClient,
    state: &PlanState,
    capture: Option<crate::adapters::NormalizedCapture>,
    recovered: bool,
) -> Result<()> {
    // A previous coverage refusal never prevents a new trusted observation.
    let _coverage_result = deliver_saved_coverage(client, state).await;
    let diff_result = publish_trusted_diff(runtime, client, state, recovered).await;
    update_postprocessing(state, client, |payload| {
        payload.diff_error = diff_result
            .as_ref()
            .err()
            .map(|error| postprocessing_diagnostic(client, error));
        Ok(())
    })?;
    if let Some(capture) = capture {
        let normalized = capture.events.len();
        let mut acknowledged = 0;
        let mut failure = None;
        let mut unknown = 0;
        for event in capture.events {
            match client.post_event(event).await {
                Ok(_) => acknowledged += 1,
                Err(error) => {
                    unknown = usize::from(
                        !matches!(error.downcast_ref::<EventPostFailure>(), Some(EventPostFailure::Rejected(status)) if status.is_client_error())
                            && !matches!(
                                error.downcast_ref::<EventPostFailure>(),
                                Some(EventPostFailure::RateLimitRetryExhausted)
                            ),
                    );
                    failure = Some(postprocessing_diagnostic(client, &error));
                    break;
                }
            }
        }
        update_postprocessing(state, client, |payload| {
            let declared = &mut payload.declared;
            declared.state = if failure.is_some() {
                DeclaredCollectionV1::Partial
            } else {
                DeclaredCollectionV1::Complete
            };
            declared.acknowledged_events = Some(acknowledged);
            declared.dropped_events = Some(normalized - acknowledged - unknown);
            declared.unknown_delivery_events = Some(unknown);
            declared.not_attempted_events = Some(if failure.is_some() {
                normalized - acknowledged - 1
            } else {
                0
            });
            declared.error = failure;
            Ok(())
        })?;
    } else {
        update_postprocessing(state, client, |payload| {
            if payload.declared.state == DeclaredCollectionV1::Pending {
                payload.declared.state = DeclaredCollectionV1::UnavailableAfterInterruption;
                payload.declared.error=Some("declared capture was not retained; its delivery counts remain unknown after interruption".into());
            }
            Ok(())
        })?;
    }
    if let Err(error) = publish_postprocessing_coverage(client, state).await {
        let diagnostic = postprocessing_diagnostic(client, &error);
        update_postprocessing(state, client, |payload| {
            payload.coverage_error = Some(diagnostic.clone());
            payload.previous_delivery_error = Some(diagnostic);
            Ok(())
        })?;
    }
    sync_postprocessing_summary(client, state)
}

async fn recover_postprocessing(
    runtime: Option<&ManagedRuntime>,
    client: &AuditClient,
    state: &PlanState,
) -> Result<()> {
    if load_postprocessing(state, client)?.is_none() {
        let plan = state.snapshot()?;
        let checkpoint = managed(&plan)?;
        let mut payload = PostprocessingV1::pending(plan.options.interactive, None, None);
        if !plan.options.interactive {
            payload.declared.state = DeclaredCollectionV1::UnavailableAfterInterruption;
            payload.declared.error = Some(
                "legacy declared capture is unavailable; delivery counts remain unknown".into(),
            );
        }
        create_postprocessing(state, client, payload, checkpoint.output_event.as_ref())?;
    }
    if load_postprocessing(state, client)?
        .context("postprocessing journal missing")?
        .payload
        .needs_recovery()
    {
        complete_postprocessing(runtime, client, state, None, true).await
    } else {
        sync_postprocessing_summary(client, state)
    }
}
async fn cleanup_committed(
    runtime: &ManagedRuntime,
    client: &AuditClient,
    state: &PlanState,
    handle: &ContainerHandle,
    terminal: &ContainerObservation,
) -> CleanupSummary {
    let removed = runtime.cleanup_terminal(handle, terminal).await;
    let error = removed
        .as_ref()
        .err()
        .map(|error| crate::redaction::clip(&redacted_error(&client.redactor, error), 1000));
    let acknowledged = mutate(
        client,
        state,
        WorkerTransitionV1::CleanupResult {
            removed: removed.is_ok(),
            error: error.clone(),
        },
    )
    .await
    .is_ok();
    let mut error_audit = if error.is_none() && acknowledged {
        CleanupErrorAudit::NotApplicable
    } else if acknowledged {
        CleanupErrorAudit::Acknowledged
    } else {
        CleanupErrorAudit::Unacknowledged
    };
    if let Some(error) = &error
        && let Ok(plan) = state.snapshot()
    {
        let compatibility = emit(
            client,
            &plan,
            "runtime.cleanup.failed",
            json!({"container":handle.name(),"terminal_result":"durably_recorded","error":error}),
        )
        .await
        .is_ok();
        if !compatibility {
            error_audit = CleanupErrorAudit::Unacknowledged;
        }
    }
    CleanupSummary {
        container: handle.name().into(),
        state: if removed.is_ok() && acknowledged {
            CleanupState::Removed
        } else {
            CleanupState::Unconfirmed
        },
        error: if removed.is_ok() && !acknowledged {
            Some(
                "container removal succeeded but durable cleanup acknowledgement is unknown".into(),
            )
        } else {
            error
        },
        error_audit,
    }
}
async fn finish_execution(
    runtime: &ManagedRuntime,
    client: &AuditClient,
    state: &PlanState,
    retained: &crate::runtime::ManagedRetainedExecution,
    control_deadline: Option<ControlDeadlineObservation>,
) -> Result<RunSummary> {
    let plan = state.snapshot()?;
    let execution = retained.result();
    let terminal = retained.terminal();
    let worker = state.worker()?;
    let failed = execution.state.exit_code != 0
        || execution.timed_out
        || execution.transport_stop_reason.is_some();
    let mut completed = event(
        &plan,
        if failed {
            "command.failed"
        } else {
            "command.completed"
        },
        json!({"runtime_started_at":terminal.process.started_at,"runtime_finished_at":terminal.process.finished_at,"output_captured":plan.options.capture_output,"worker_id":worker.worker_id}),
    );
    let mut result = json!({"exit_code":execution.state.exit_code,"runtime_attach_exit_code":execution.exit_code,"duration_ms":execution.duration_ms,"timed_out":execution.timed_out,"oom_killed":terminal.process.oom_killed,"stdout_bytes":execution.stdout.total_bytes,"stderr_bytes":execution.stderr.total_bytes,"stdout_truncated":execution.stdout.truncated,"stderr_truncated":execution.stderr.truncated});
    if let Some(observation) = control_deadline {
        result["control_deadline"] = serde_json::to_value(observation)?;
    }
    if plan.options.interactive {
        for field in [
            "stdout_bytes",
            "stderr_bytes",
            "stdout_truncated",
            "stderr_truncated",
        ] {
            result[field] = Value::Null;
        }
        result["execution_mode"] = serde_json::to_value(plan.request.execution_mode)?;
        result["transport_stop_reason"] = serde_json::to_value(&execution.transport_stop_reason)?;
    }
    if plan.options.capture_output {
        result["stdout"] = json!(crate::redaction::clip(
            &client
                .redactor
                .captured_text(&execution.stdout.bytes, execution.stdout.truncated),
            8192
        ));
        result["stderr"] = json!(crate::redaction::clip(
            &client
                .redactor
                .captured_text(&execution.stderr.bytes, execution.stderr.truncated),
            8192
        ));
    }
    completed["command"] = serde_json::to_value(&plan.request.command)?;
    completed["result"] = result;
    let capture = if plan.options.interactive {
        None
    } else {
        let context = crate::adapters::AdapterContext {
            session_id: plan.session.to_string(),
            project_id: plan.request.project_id.clone().unwrap_or_default(),
            sandbox_id: plan.request.sandbox_id.clone().unwrap_or_default(),
            agent_id: plan.request.agent_id.clone().unwrap_or_default(),
            correlation_id: plan.request.correlation_id.clone().unwrap_or_default(),
            trace_id: plan.request.trace_id.clone().unwrap_or_default(),
        };
        Some(crate::adapters::parse_redacted_capture(
            &plan.options.provider,
            &execution.stdout.bytes,
            execution.stdout.truncated,
            &context,
            &client.redactor,
        ))
    };
    let initial_summary = RunSummary {
        session_id: plan.session,
        volume: worker.attempt.volume_name.clone(),
        exit_code: execution.state.exit_code,
        timed_out: execution.timed_out,
        changed_files: None,
        output_captured: plan.options.capture_output,
        cleanup: CleanupSummary {
            container: retained.handle().name().into(),
            state: CleanupState::Unconfirmed,
            error: None,
            error_audit: CleanupErrorAudit::NotApplicable,
        },
        postprocessing_error: None,
        transport_stop_reason: execution.transport_stop_reason.clone(),
        control_deadline,
    };
    let completed = client.redactor.value(&completed);
    // Two files, deliberately ordered rather than claimed atomic: Pending is
    // synced first. A crash before the following plan write requires runtime
    // reconciliation and never turns this delivery journal into terminal proof.
    create_postprocessing(
        state,
        client,
        PostprocessingV1::pending(
            plan.options.interactive,
            capture.as_ref(),
            (!plan.options.interactive).then_some(execution.stdout.truncated),
        ),
        Some(&completed),
    )?;
    state.update(|plan| {
        let checkpoint = plan.managed.as_mut().context("missing checkpoint")?;
        checkpoint.output_event = Some(completed);
        checkpoint.result = Some(initial_summary.clone());
        Ok(())
    })?;
    record_observation(client, state, terminal).await?;
    anyhow::ensure!(
        state.worker()?.state == WorkerStateV1::Terminal && !state.worker()?.quota_held,
        "terminal allocation has not been durably acknowledged; instance retained"
    );
    post_saved_terminal(client, state).await?;
    let cleanup = cleanup_committed(runtime, client, state, retained.handle(), terminal).await;
    let mut summary = RunSummary {
        cleanup,
        ..initial_summary
    };
    if summary.cleanup.error_audit == CleanupErrorAudit::Unacknowledged {
        summary.postprocessing_error =
            Some("postprocessing skipped: cleanup state was not durably acknowledged".into());
        state.update(|plan| {
            plan.managed.as_mut().context("missing checkpoint")?.result = Some(summary.clone());
            Ok(())
        })?;
        return Ok(summary);
    }
    state.update(|plan| {
        plan.managed.as_mut().context("missing checkpoint")?.result = Some(summary.clone());
        Ok(())
    })?;
    complete_postprocessing(Some(runtime), client, state, capture, false).await?;
    state
        .checkpoint()?
        .result
        .context("postprocessing result missing")
}

fn validate_checkpoint(plan: &RunPlan, client: &AuditClient) -> Result<()> {
    let checkpoint = managed(plan)?;
    checkpoint.admission.validate()?;
    anyhow::ensure!(
        matches!(plan.version, 2 | 3)
            && plan.operation_id.to_string() == checkpoint.admission.operation_id
            && plan.session.to_string() == checkpoint.admission.launch.session_id
            && configured_origin(client) == checkpoint.gateway_origin
            && plan.options.config_dir == checkpoint.config_root
            && serde_json::to_value(&plan.request)?
                == serde_json::to_value(&checkpoint.admission.decision_request)?
            && plan.options.argv == checkpoint.admission.launch.command.argv
            && plan.options.profile == checkpoint.admission.launch.profile
            && runtime_kind(plan.options.runtime) == checkpoint.admission.launch.runtime_kind
            && network_kind(plan.options.network) == checkpoint.admission.launch.network
            && plan.options.timeout == u64::from(checkpoint.admission.launch.timeout_seconds)
            && plan.options.image == checkpoint.admission.launch.image_ref
            && plan.options.provider == checkpoint.admission.launch.provider
            && plan.options.capture_output == checkpoint.admission.launch.capture_output
            && plan.options.interactive
                == checkpoint.admission.launch.security_preset.is_interactive()
            && content_hash("ai-guard:workspace-manifest:v1", &checkpoint.before)?
                == checkpoint.admission.launch.workspace_manifest_digest,
        "private session checkpoint conflicts with its admitted specification, identity or origin"
    );
    validate_provider_plan(plan)?;
    if let Some(binding) = &checkpoint.binding {
        anyhow::ensure!(
            binding.authority_id.to_string() == checkpoint.admission.launch.authority_id
                && runtime_kind(binding.kind) == checkpoint.admission.launch.runtime_kind,
            "private runtime binding conflicts with the admitted authority"
        );
    }
    client.validate_identity_payload(&checkpoint.admission)?;
    Ok(())
}
async fn load_session(
    session: Uuid,
    config: &Path,
    gateway: &str,
) -> Result<(PlanState, AuditClient)> {
    let config_guard =
        crate::control_fs::Directory::open_existing(&crate::control_fs::absolute_path(config)?)?;
    let config = config_guard.path().to_path_buf();
    let directory = Directory::root(&config)?.child("sessions")?;
    let plan: RunPlan = directory
        .read(&format!("{session}.json"))?
        .context("session checkpoint not found")?;
    anyhow::ensure!(
        plan.session == session && plan.options.config_dir == config,
        "session identity or configuration root differs from its private checkpoint"
    );
    let token = read_token(&config.join("secrets/runner"))?;
    let mut client = AuditClient::new(gateway, &token)?;
    let checkpoint = managed(&plan)?;
    let can_prepare_credentials = checkpoint.workspace_control_exclusion_version == Some(1)
        || checkpoint
            .worker
            .as_ref()
            .is_some_and(|worker| worker.attempt.start_operation_id.is_some());
    let mask_credentials = if plan.version == 3 && can_prepare_credentials {
        // A missing private binding never prevents read/stop/export. A fresh
        // start below still requires its exact bytes and fails closed.
        frozen_provider_credentials(&plan, &config_guard)
            .ok()
            .flatten()
    } else {
        plan.options
            .secret_env_file
            .as_deref()
            .filter(|_| can_prepare_credentials)
            .and_then(|path| credentials(path, &config_guard).ok())
    };
    if let Some(credentials) = mask_credentials {
        client = client.with_secrets(credentials.secrets);
    }
    validate_checkpoint(&plan, &client)?;
    check_identity(&client, Some(&managed(&plan)?.tenant_id)).await?;
    let state = PlanState {
        plan: Arc::new(Mutex::new(plan)),
        directory: Arc::new(directory),
        name: format!("{session}.json"),
    };
    if state.checkpoint()?.worker.is_none() {
        let admission = state.checkpoint()?.admission;
        let receipt: MutationReceiptV1 = client
            .get_typed(&format!(
                "api/v1/workers/operations/{}",
                admission.operation_id
            ))
            .await
            .context("admission response is unknown; no runtime launch is permitted")?;
        anyhow::ensure!(
            receipt.operation_id == admission.operation_id
                && receipt.input_hash == admission.input_hash()?
                && receipt.audit_seq > 0,
            "admission recovery receipt conflicts with the private request"
        );
        if let Some(id) = receipt.worker_id {
            let worker: WorkerV1 = client.get_typed(&format!("api/v1/workers/{id}")).await?;
            validate_worker(&state.snapshot()?, &worker)?;
            state.read_worker(worker)?;
        }
    }
    if state.checkpoint()?.worker.is_some() {
        refresh_readonly(&client, &state).await?;
    }
    Ok((state, client))
}
fn private_credentials(plan: &RunPlan) -> Result<Option<CredentialFile>> {
    let config = crate::control_fs::Directory::open_existing(&plan.options.config_dir)?;
    let credential = if plan.version == 3 {
        frozen_provider_credentials(plan, &config)?
    } else {
        plan.options
            .secret_env_file
            .as_deref()
            .map(|path| credentials(path, &config))
            .transpose()?
    };
    validate_credential_scope(
        credential.as_ref(),
        &control_secrets(&plan.options.config_dir)?,
    )?;
    anyhow::ensure!(
        credential_digest(credential.as_ref())? == managed(plan)?.credential_digest,
        "credential source changed or is unavailable; a new admission is required before a fresh start"
    );
    Ok(credential)
}
fn lock_attempt(state: &PlanState) -> Result<AttemptGuard> {
    let plan = state.snapshot()?;
    let checkpoint = managed(&plan)?;
    let worker = checkpoint
        .worker
        .as_ref()
        .context("worker admission was refused")?;
    let guard = AttemptGuard::acquire(
        &plan.options.config_dir,
        Uuid::parse_str(&worker.attempt.attempt_id)?,
        checkpoint.lock_identity,
    )?;
    Ok(guard)
}
async fn locked_session(
    session: Uuid,
    config: &Path,
    gateway: &str,
) -> Result<(PlanState, AuditClient, AttemptGuard)> {
    let (stale, _) = load_session(session, config, gateway).await?;
    let guard = lock_attempt(&stale)?;
    // The owner may have completed between the read and lock acquisition.
    // Reload before publishing anything, so status/stop cannot erase its result.
    let (state, client) = load_session(session, config, gateway).await?;
    anyhow::ensure!(
        state
            .checkpoint()?
            .lock_identity
            .is_none_or(|expected| expected == guard.identity()),
        "attempt lock changed while acquiring session ownership"
    );
    state.update(|plan| {
        plan.managed
            .as_mut()
            .context("missing checkpoint")?
            .lock_identity = Some(guard.identity());
        Ok(())
    })?;
    Ok((state, client, guard))
}
async fn recover_owned(
    runtime: &ManagedRuntime,
    state: &PlanState,
) -> Result<Option<OwnedContainerSnapshot>> {
    let plan = state.snapshot()?;
    let checkpoint = managed(&plan)?;
    let worker = state.worker()?;
    if let Some(owned) = &checkpoint.owned {
        anyhow::ensure!(
            owned.observation.configuration_fingerprint.is_some()
                && owned.handle.session_id() == plan.session
                && owned.handle.operation_id() == plan.operation_id
                && owned.handle.name() == worker.attempt.stable_name
                && checkpoint
                    .binding
                    .as_ref()
                    .is_some_and(
                        |binding| owned.handle.authority_fingerprint() == binding.fingerprint()
                    )
                && worker
                    .attempt
                    .container_id
                    .as_ref()
                    .is_none_or(|id| id == owned.handle.id()),
            "owned runtime checkpoint conflicts with the server attempt"
        );
        if let Some(expected) = &worker.attempt.created_inspection {
            anyhow::ensure!(
                *expected == inspection(&worker, &owned.observation)?,
                "created runtime checkpoint conflicts with durable inspection"
            );
        }
        return Ok(Some(owned.clone()));
    }
    let spec = worker_spec(&plan, &worker, None)?;
    let recovered = runtime.lookup_created(&spec).await?;
    if let Some(owned) = &recovered {
        anyhow::ensure!(
            owned.observation.configuration_fingerprint.is_some()
                && worker
                    .attempt
                    .container_id
                    .as_ref()
                    .is_none_or(|id| id == owned.handle.id()),
            "recovered runtime instance conflicts with durable ownership"
        );
        state.update(|plan| {
            plan.managed.as_mut().context("missing checkpoint")?.owned = Some(owned.clone());
            Ok(())
        })?;
    }
    Ok(recovered)
}
fn action_status(
    state: &PlanState,
    current: Option<&ContainerObservation>,
    message: &str,
) -> Result<Value> {
    let plan = state.snapshot()?;
    let checkpoint = managed(&plan)?;
    Ok(
        json!({"session_id":plan.session,"tenant_id":checkpoint.tenant_id,"gateway_origin":checkpoint.gateway_origin,
        "worker":checkpoint.worker,"runtime_observation":current,"registry_observation_scope":"last_durably_recorded",
        "outputs":if checkpoint.result.is_some(){if plan.options.capture_output{"captured"}else{"not_captured"}}else{"unknown_after_broker_interruption"},
        "message":message,"result":checkpoint.result}),
    )
}
pub(super) async fn worker_status(session: Uuid, config: &Path, gateway: &str) -> Result<Value> {
    let (state, client) = load_session(session, config, gateway).await?;
    let mut status = action_status(
        &state,
        None,
        "Registry state read from the authenticated tenant; runtime facts carry their original observation time.",
    )?;
    status["postprocessing"] =
        serde_json::to_value(load_postprocessing(&state, &client)?.map(|journal| journal.payload))?;
    Ok(status)
}
async fn finalize_recovered(
    runtime: &ManagedRuntime,
    client: &AuditClient,
    state: &PlanState,
    owned: &OwnedContainerSnapshot,
    observed: &ContainerObservation,
) -> Result<Value> {
    anyhow::ensure!(
        owned.observation.configuration_fingerprint.is_some(),
        "created configuration proof is required for terminal recovery"
    );
    observed.validate_terminal(state.snapshot()?.options.runtime, &owned.observation)?;
    record_observation(client, state, observed).await?;
    anyhow::ensure!(
        state.worker()?.state == WorkerStateV1::Terminal && !state.worker()?.quota_held,
        "terminal runtime result was not acknowledged; instance retained"
    );
    post_saved_terminal(client, state).await?;
    if state.worker()?.cleanup_removed != Some(true) {
        let cleanup = cleanup_committed(runtime, client, state, &owned.handle, observed).await;
        state.update(|plan| {
            if let Some(result) = plan
                .managed
                .as_mut()
                .context("missing checkpoint")?
                .result
                .as_mut()
            {
                result.cleanup = cleanup;
            }
            Ok(())
        })?;
    }
    if load_postprocessing(state, client)?.is_some()
        || state.checkpoint()?.result.as_ref().is_none_or(|result| {
            result.changed_files.is_none() || result.postprocessing_error.is_some()
        })
    {
        anyhow::ensure!(
            state.worker()?.cleanup_removed == Some(true),
            "postprocessing recovery requires durable cleanup"
        );
        recover_postprocessing(Some(runtime), client, state).await?;
    }
    action_status(
        state,
        Some(observed),
        "Terminal allocation was recovered from runtime facts. Unavailable attach output remains unknown; no command was started again.",
    )
}
pub(super) async fn resume_worker(
    session: Uuid,
    config: &Path,
    gateway: &str,
    wait_seconds: u64,
    program: Option<&Path>,
    adopt: bool,
    interactive: bool,
) -> Result<Value> {
    anyhow::ensure!(
        wait_seconds <= 86400,
        "resume observation timeout exceeds 24 hours"
    );
    if let Some(status) = control_monitor::stop_interrupted_before_gateway(
        session,
        config,
        gateway,
        program,
        adopt,
        interactive,
    )
    .await?
    {
        return Ok(status);
    }
    boxed_resume_phase(|| {
        resume_authenticated(
            session,
            config,
            gateway,
            wait_seconds,
            program,
            adopt,
            interactive,
        )
    })
    .await
}

// Construct each recovery phase outside its caller's poll frame. In debug
// builds the combined poll frame otherwise leaves too little stack for the
// private checkpoint decoder, even though the future itself is much smaller.
#[inline(never)]
fn boxed_resume_phase<F: std::future::Future + Send>(
    construct: impl FnOnce() -> F,
) -> std::pin::Pin<Box<F>> {
    Box::pin(construct())
}

async fn resume_authenticated(
    session: Uuid,
    config: &Path,
    gateway: &str,
    wait_seconds: u64,
    program: Option<&Path>,
    adopt: bool,
    interactive: bool,
) -> Result<Value> {
    let (state, client, guard) = locked_session(session, config, gateway).await?;
    let plan = state.snapshot()?;
    anyhow::ensure!(
        !interactive || plan.options.interactive,
        "--interactive cannot convert an admitted batch session; create a new audited interactive run"
    );
    require_control_exclusion_for_first_start(&state)?;
    if state.checkpoint()?.pending.is_some() {
        reconcile_pending(&client, &state).await?;
    }
    let worker = refresh(&client, &state).await?;
    if plan.options.interactive && worker.quota_held && worker.attempt.start_operation_id.is_none()
    {
        anyhow::ensure!(
            interactive,
            "first-starting this interactive session requires worker-resume --interactive and a controlling terminal"
        );
        validate_interactive_options(&plan.options)?;
    }
    if interactive && worker.quota_held && worker.attempt.start_operation_id.is_some() {
        bail!(
            "interactive reattach after a consumed start claim is not implemented; no start, restart or exec was emitted; use worker-status, worker-resume without --interactive to observe, or worker-stop; allocation and evidence are retained"
        );
    }
    if worker.state == WorkerStateV1::Cancelled {
        return action_status(
            &state,
            None,
            "Reservation is cancelled; no worker will be started.",
        );
    }
    if worker.state == WorkerStateV1::Terminal && worker.cleanup_removed == Some(true) {
        return boxed_resume_phase(|| resume_terminal(&state, &client, &plan, program, adopt))
            .await;
    }
    boxed_resume_phase(|| {
        resume_runtime(
            &state,
            &client,
            &guard,
            &worker,
            wait_seconds,
            program,
            adopt,
        )
    })
    .await
}

async fn resume_terminal(
    state: &PlanState,
    client: &AuditClient,
    plan: &RunPlan,
    program: Option<&Path>,
    adopt: bool,
) -> Result<Value> {
    post_saved_terminal(client, state).await?;
    let journal = load_postprocessing(state, client)?;
    if journal.is_some()
        || state.checkpoint()?.result.as_ref().is_none_or(|result| {
            result.changed_files.is_none() || result.postprocessing_error.is_some()
        })
    {
        if journal
            .as_ref()
            .is_none_or(|journal| journal.payload.diff.is_none())
        {
            let runtime = bound_runtime(state, client, program, adopt).await?;
            runtime.doctor(plan.options.allow_rootful_runtime).await?;
            recover_postprocessing(Some(&runtime), client, state).await?;
        } else {
            recover_postprocessing(None, client, state).await?;
        }
    }
    action_status(
        state,
        None,
        "Terminal allocation and cleanup are durable. Retained postprocessing facts were reconciled; no worker command was started again.",
    )
}

async fn resume_runtime(
    state: &PlanState,
    client: &AuditClient,
    guard: &AttemptGuard,
    worker: &WorkerV1,
    wait_seconds: u64,
    program: Option<&Path>,
    adopt: bool,
) -> Result<Value> {
    // A fresh start may use only the exact credential bytes admitted earlier.
    // Prepare them before any recovery helper and retain their handles through
    // create. Consumed starts and terminal reconciliation need no provider key.
    let credentials = if worker.attempt.start_operation_id.is_none() && worker.quota_held {
        private_credentials(&state.snapshot()?)?
    } else {
        None
    };
    let runtime = bound_runtime(state, client, program, adopt).await?;
    runtime
        .doctor(state.snapshot()?.options.allow_rootful_runtime)
        .await?;
    if worker.state == WorkerStateV1::Reserved {
        return boxed_resume_phase(|| {
            resume_preparation(
                &runtime,
                client,
                state,
                guard,
                worker,
                credentials.as_ref(),
                wait_seconds,
            )
        })
        .await;
    }
    if matches!(
        worker.state,
        WorkerStateV1::Preparing | WorkerStateV1::InDoubt
    ) && worker.attempt.create_operation_id.is_none()
        && worker.attempt.container_id.is_none()
        && worker.attempt.created_inspection.is_none()
        && worker.attempt.start_operation_id.is_none()
        && worker.quota_held
        && !worker.allocation_inconsistent
    {
        return boxed_resume_phase(|| {
            resume_preparation(
                &runtime,
                client,
                state,
                guard,
                worker,
                credentials.as_ref(),
                wait_seconds,
            )
        })
        .await;
    }
    boxed_resume_phase(|| {
        resume_owned(
            &runtime,
            client,
            state,
            guard,
            credentials.as_ref(),
            wait_seconds,
        )
    })
    .await
}

async fn resume_preparation(
    runtime: &ManagedRuntime,
    client: &AuditClient,
    state: &PlanState,
    guard: &AttemptGuard,
    worker: &WorkerV1,
    credentials: Option<&CredentialFile>,
    wait_seconds: u64,
) -> Result<Value> {
    if worker.state == WorkerStateV1::Reserved {
        mutate(client, state, WorkerTransitionV1::Prepare).await?;
        import_workspace(runtime, client, state, guard).await?;
        mutate(client, state, WorkerTransitionV1::CreateIssued).await?;
        let result = launch_worker(runtime, client, state, guard, credentials, None).await?;
        return Ok(serde_json::to_value(result)?);
    }
    if worker
        .unknown_reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with("stop requested"))
    {
        return action_status(
            state,
            None,
            "A stop was requested during preparation. Its reservation remains held until safe operator reconciliation.",
        );
    }
    let checkpoint = state.checkpoint()?;
    let ready = if checkpoint.import.is_some() {
        recover_import(runtime, client, state, guard, wait_seconds).await?
    } else {
        if worker.state == WorkerStateV1::InDoubt {
            // The helper checkpoint is persisted before helper creation.
            // Its absence therefore proves no import emission by this
            // attempt. Validate the immutable input and retain the OS
            // guard before recovering preparation; existing volume
            // ownership is still inspected by import_workspace.
            let _archive = state
                .directory
                .archive(&checkpoint.archive_name, &checkpoint.archive_digest)?;
            guard.ensure_current()?;
            mutate(client, state, WorkerTransitionV1::Prepare).await?;
        }
        import_workspace(runtime, client, state, guard).await?;
        true
    };
    if ready {
        if state.worker()?.state == WorkerStateV1::InDoubt {
            mutate(client, state, WorkerTransitionV1::Prepare).await?;
        }
        mutate(client, state, WorkerTransitionV1::CreateIssued).await?;
        let result = launch_worker(runtime, client, state, guard, credentials, None).await?;
        return Ok(serde_json::to_value(result)?);
    }
    mark_unknown(
        client,
        state,
        "broker interrupted during import; no complete helper/copy proof is available",
    )
    .await;
    action_status(
        state,
        None,
        "Import emission or contents remain uncertain. Helper, volume and reservation were retained; no import was repeated.",
    )
}

async fn resume_owned(
    runtime: &ManagedRuntime,
    client: &AuditClient,
    state: &PlanState,
    guard: &AttemptGuard,
    credentials: Option<&CredentialFile>,
    wait_seconds: u64,
) -> Result<Value> {
    let Some(owned) = recover_owned(runtime, state).await? else {
        mark_unknown(client,state,"owned runtime instance was not found in a successful targeted inventory; terminal state remains unknown").await;
        return action_status(
            state,
            None,
            "No owned instance was observed. This does not prove termination or authorize another start; quota remains held.",
        );
    };
    let mut observed = runtime.observe(&owned.handle).await?;
    if observed.process.running == Some(true)
        && state.worker()?.attempt.start_operation_id.is_some()
    {
        guard.ensure_current()?;
        // Start the exact owned stop without waiting for a journal read. A
        // reopened monitor cannot reconstruct the original monotonic lifetime.
        let (stop, epoch) = tokio::join!(
            runtime.stop_snapshot_owned(&owned),
            control_monitor::require_stop_on_active_resume(state),
        );
        match stop {
            Ok(stopped) => observed = stopped,
            Err(_) => {
                let reason = match epoch {
                    Err(error) => redacted_error(&client.redactor, &error),
                    Ok(()) => "active resume cannot verify the original execution lifetime".into(),
                };
                mark_unknown(client, state, &reason).await;
                return action_status(
                    state,
                    Some(&observed),
                    "The original control monitor lifetime cannot be verified after interruption. A targeted stop was requested but is unconfirmed; no fresh window, start or quota release was granted.",
                );
            }
        }
    }
    let worker = state.worker()?;
    let claim_uncertain = state
        .checkpoint()?
        .uncertain
        .iter()
        .any(|mutation| matches!(mutation.transition, WorkerTransitionV1::StartClaim { .. }));
    if worker.attempt.start_operation_id.is_none()
        && !claim_uncertain
        && matches!(
            worker.state,
            WorkerStateV1::Preparing | WorkerStateV1::Created | WorkerStateV1::InDoubt
        )
        && observed.process.started_at.is_none()
        && observed.process.running == Some(false)
    {
        let result = launch_worker(runtime, client, state, guard, credentials, Some(owned)).await?;
        return Ok(serde_json::to_value(result)?);
    }
    boxed_resume_phase(|| {
        observe_resumed(
            runtime,
            client,
            state,
            guard,
            &owned,
            observed,
            wait_seconds,
        )
    })
    .await
}

async fn observe_resumed(
    runtime: &ManagedRuntime,
    client: &AuditClient,
    state: &PlanState,
    guard: &AttemptGuard,
    owned: &OwnedContainerSnapshot,
    mut observed: ContainerObservation,
    wait_seconds: u64,
) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(wait_seconds);
    let mut recorded_running = false;
    loop {
        guard.ensure_current()?;
        if observed
            .validate_terminal(state.snapshot()?.options.runtime, &owned.observation)
            .is_ok()
            && state.worker()?.attempt.start_operation_id.is_some()
        {
            return finalize_recovered(runtime, client, state, owned, &observed).await;
        }
        if observed.process.running == Some(true) {
            observed.validate_identity_allocation(&owned.observation)?;
            if !recorded_running {
                record_observation(client, state, &observed).await?;
                recorded_running = true;
            }
            if state.worker()?.state == WorkerStateV1::Stopping {
                runtime.stop_snapshot_owned(owned).await?;
            }
        } else {
            mark_unknown(client,state,"runtime process is not proven terminal or has no durable start claim; pending effects remain unknown").await;
            return action_status(
                state,
                Some(&observed),
                "No terminal proof is available. Prior start emission may be unresolved; no second start or quota release was performed.",
            );
        }
        if tokio::time::Instant::now() >= deadline {
            return action_status(
                state,
                Some(&observed),
                "Worker is still observed running. This resume command's observation window ended; attach output from the interrupted broker is unknown.",
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        refresh(client, state).await?;
        observed = runtime.observe(&owned.handle).await?;
    }
}
async fn out_of_band_stop(
    client: &AuditClient,
    state: &PlanState,
    transition: WorkerTransitionV1,
) -> Result<WorkerV1> {
    let plan = state.snapshot()?;
    let worker = state.worker()?;
    let mutation = WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition,
    };
    let directory = Directory::root(&plan.options.config_dir)?.child("worker-stop-intents")?;
    directory.write(&format!("{}.json", mutation.operation_id), &mutation)?;
    let result = submit_stop_intent(client, &plan, &mutation).await?;
    directory.write(
        &format!("{}-ack.json", mutation.operation_id),
        &result.receipt,
    )?;
    Ok(result.worker)
}

async fn submit_stop_intent(
    client: &AuditClient,
    plan: &RunPlan,
    mutation: &WorkerMutationV1,
) -> Result<MutationResultV1> {
    let result: MutationResultV1 = client
        .post_control(
            &format!("api/v1/workers/{}/transitions", mutation.worker_id),
            mutation,
        )
        .await?;
    anyhow::ensure!(
        result.receipt.operation_id == mutation.operation_id
            && result.receipt.input_hash == content_hash("ai-guard:worker-mutation:v1", mutation)?
            && result.receipt.audit_seq > 0,
        "stop intent lacks its exact durable acknowledgement"
    );
    validate_worker(plan, &result.worker)?;
    Ok(result)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReservedStopIntentV1 {
    version: u32,
    tenant_id: String,
    gateway_origin: String,
    session_id: Uuid,
    admission_operation_id: Uuid,
    config_root: PathBuf,
    mutation: WorkerMutationV1,
}

impl ReservedStopIntentV1 {
    fn validate(&self, plan: &RunPlan, worker: &WorkerV1) -> Result<u64> {
        let checkpoint = managed(plan)?;
        let mutation = &self.mutation;
        let cancelled_revision = mutation
            .expected_revision
            .checked_add(1)
            .context("reservation cancellation revision is exhausted")?;
        let revision = if worker.state == WorkerStateV1::Cancelled {
            cancelled_revision
        } else {
            mutation.expected_revision
        };
        anyhow::ensure!(
            self.version == 1
                && self.tenant_id == checkpoint.tenant_id
                && self.gateway_origin == checkpoint.gateway_origin
                && self.session_id == plan.session
                && self.admission_operation_id == plan.operation_id
                && self.config_root == checkpoint.config_root
                && mutation.version == 1
                && Uuid::parse_str(&mutation.operation_id)
                    .is_ok_and(|operation| !operation.is_nil()
                        && operation.to_string() == mutation.operation_id)
                && mutation.worker_id == worker.worker_id
                && mutation.attempt_id == worker.attempt.attempt_id
                && matches!(mutation.transition, WorkerTransitionV1::CancelReserved)
                && mutation.expected_revision > 0
                && worker.revision == revision,
            "saved reservation cancellation differs from the authenticated worker or revision"
        );
        Ok(cancelled_revision)
    }

    fn validate_acknowledgement(
        &self,
        receipt: &MutationReceiptV1,
        worker: &WorkerV1,
        cancelled_revision: u64,
    ) -> Result<()> {
        let mutation = &self.mutation;
        anyhow::ensure!(
            receipt.operation_id == mutation.operation_id
                && receipt.input_hash == content_hash("ai-guard:worker-mutation:v1", mutation)?
                && receipt.audit_seq > 0
                && receipt.audit_hash.len() == 64
                && receipt
                    .audit_hash
                    .bytes()
                    .all(|byte| { byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) })
                && receipt.worker_id.as_deref() == Some(mutation.worker_id.as_str())
                && receipt.revision == Some(cancelled_revision)
                && worker.worker_id == mutation.worker_id
                && worker.attempt.attempt_id == mutation.attempt_id
                && worker.revision == cancelled_revision
                && worker.state == WorkerStateV1::Cancelled
                && !worker.quota_held
                && worker.attempt.container_id.is_none()
                && worker.attempt.create_operation_id.is_none()
                && worker.attempt.start_operation_id.is_none(),
            "reservation cancellation lacks its exact durable acknowledgement"
        );
        Ok(())
    }
}

async fn reconcile_reserved_stop(
    client: &AuditClient,
    state: &PlanState,
) -> Result<Option<WorkerV1>> {
    let plan = state.snapshot()?;
    let previous = state.worker()?;
    let reserve = previous.state == WorkerStateV1::Reserved;
    let attempt = Uuid::parse_str(&previous.attempt.attempt_id)?;
    let config = crate::control_fs::Directory::open_existing(&plan.options.config_dir)?;
    let Some(journal) = config.persistent_child("worker-stop-intents", reserve)? else {
        return Ok(None);
    };
    let Some(journal) = journal.persistent_child(&attempt.to_string(), reserve)? else {
        return Ok(None);
    };
    let lock = AttemptGuard::acquire_in(Directory::from_control(journal.clone())?, attempt, None)
        .context("cannot acquire the private reservation cancellation lock")?;
    let journal = Directory::from_control(journal)?;
    let worker = refresh_readonly(client, state).await?;
    anyhow::ensure!(
        worker.worker_id == previous.worker_id
            && worker.attempt.attempt_id == previous.attempt.attempt_id
            && worker.revision >= previous.revision,
        "gateway worker changed during reservation cancellation"
    );
    if !matches!(
        worker.state,
        WorkerStateV1::Reserved | WorkerStateV1::Cancelled
    ) {
        return Ok(None);
    }
    let acknowledged = journal.read_bounded::<MutationReceiptV1>("ack.json", 4096)?;
    let saved = journal.read_bounded::<ReservedStopIntentV1>("intent.json", 4096)?;
    anyhow::ensure!(
        acknowledged.is_none() || saved.is_some(),
        "saved cancellation acknowledgement lacks its exact intent"
    );
    let request = match saved {
        Some(request) => request,
        None if worker.state == WorkerStateV1::Cancelled => return Ok(None),
        None => {
            let checkpoint = managed(&plan)?;
            let request = ReservedStopIntentV1 {
                version: 1,
                tenant_id: checkpoint.tenant_id.clone(),
                gateway_origin: checkpoint.gateway_origin.clone(),
                session_id: plan.session,
                admission_operation_id: plan.operation_id,
                config_root: checkpoint.config_root.clone(),
                mutation: WorkerMutationV1 {
                    version: 1,
                    operation_id: Uuid::new_v4().to_string(),
                    worker_id: worker.worker_id.clone(),
                    attempt_id: worker.attempt.attempt_id.clone(),
                    expected_revision: worker.revision,
                    transition: WorkerTransitionV1::CancelReserved,
                },
            };
            lock.ensure_current()?;
            journal.write("intent.json", &request)?;
            request
        }
    };
    let cancelled_revision = request.validate(&plan, &worker)?;
    if let Some(receipt) = acknowledged {
        request.validate_acknowledgement(&receipt, &worker, cancelled_revision)?;
        let current: MutationReceiptV1 = client
            .get_typed(&format!(
                "api/v1/workers/operations/{}",
                request.mutation.operation_id,
            ))
            .await?;
        anyhow::ensure!(
            current == receipt,
            "saved reservation cancellation acknowledgement differs from the gateway"
        );
        lock.ensure_current()?;
        // A previous rename may be visible even if its directory sync failed.
        journal.sync()?;
        return Ok(Some(worker));
    }

    // Exact retry distinguishes an uncommitted cancellation from a committed
    // one with an unavailable response. The Store checks that identity before CAS.
    lock.ensure_current()?;
    let result = submit_stop_intent(client, &plan, &request.mutation).await?;
    request.validate_acknowledgement(&result.receipt, &result.worker, cancelled_revision)?;
    lock.ensure_current()?;
    journal.write("ack.json", &result.receipt)?;
    Ok(Some(result.worker))
}

pub(super) async fn stop_worker(
    session: Uuid,
    config: &Path,
    gateway: &str,
    program: Option<&Path>,
    adopt: bool,
) -> Result<Value> {
    let (state, client) = load_session(session, config, gateway).await?;
    if matches!(
        state.worker()?.state,
        WorkerStateV1::Reserved | WorkerStateV1::Cancelled
    ) && let Some(acknowledged) = Box::pin(reconcile_reserved_stop(&client, &state)).await?
    {
        state.read_worker(acknowledged)?;
        return action_status(
            &state,
            None,
            "The never-prepared reservation was cancelled durably.",
        );
    }
    let worker = state.worker()?;
    if [WorkerStateV1::Cancelled, WorkerStateV1::Terminal].contains(&worker.state) {
        return action_status(
            &state,
            None,
            "Worker is already terminal or cancelled; no stop was emitted.",
        );
    }
    let transition = if worker.attempt.container_id.is_some() {
        WorkerTransitionV1::Stop
    } else {
        WorkerTransitionV1::Unknown {
            reason: "stop requested during preparation; worker allocation remains uncertain".into(),
        }
    };
    let acknowledged = out_of_band_stop(&client, &state, transition).await?;
    state.read_worker(acknowledged)?;
    let (state, client, guard) = match locked_session(session, config, gateway).await {
        Ok(context) => context,
        Err(_) => {
            return action_status(
                &state,
                None,
                "Stop intent is durable. The current broker may process it; runtime stop is not yet confirmed.",
            );
        }
    };
    guard.ensure_current()?;
    let runtime = bound_runtime(&state, &client, program, adopt).await?;
    if let Some(owned) = recover_owned(&runtime, &state).await? {
        let observed = runtime.stop_snapshot_owned(&owned).await?;
        if observed
            .validate_terminal(state.snapshot()?.options.runtime, &owned.observation)
            .is_ok()
            && state.worker()?.attempt.start_operation_id.is_some()
        {
            return finalize_recovered(&runtime, &client, &state, &owned, &observed).await;
        }
        mark_unknown(&client,&state,"stop observed no running process, but terminal/start-emission proof remains incomplete").await;
        return action_status(
            &state,
            Some(&observed),
            "The owned instance was observed not running. Pending earlier runtime calls remain uncertain; quota was retained.",
        );
    }
    if let Some(helper) = state.checkpoint()?.import.and_then(|helper| helper.owned) {
        let _ = runtime.stop_snapshot_owned(&helper).await;
    }
    mark_unknown(
        &client,
        &state,
        "stop requested but no owned worker terminal proof is available",
    )
    .await;
    action_status(
        &state,
        None,
        "No owned terminal worker proof is available; helper/volume evidence and reservation remain retained.",
    )
}
async fn publish_export_audit_events(
    client: &AuditClient,
    plan: &RunPlan,
    worker_id: Option<&str>,
    legacy_local_read: bool,
    destination: &Path,
    report: &workspace::ExportReport,
) -> Result<()> {
    let manifest_digest = content_hash("ai-guard:workspace-manifest:v1", &report.manifest)?;
    let destination_hash = content_hash(
        "ai-guard:export-destination:v1",
        &destination.to_string_lossy(),
    )?;
    let mut policy_data = json!({
        "version": 1,
        "request_id": plan.request.request_id,
        "manifest_digest": manifest_digest,
        "destination_hash": destination_hash,
        "omitted_entries": report.omitted_entries,
        "files": report.manifest.len(),
    });
    let mut completed_data = json!({
        "files": report.manifest.len(),
        "omitted_entries": report.omitted_entries,
        "manifest_digest": manifest_digest,
        "destination_hash": destination_hash,
    });
    if let Some(worker_id) = worker_id {
        policy_data["worker_id"] = json!(worker_id);
        completed_data["worker_id"] = json!(worker_id);
    }
    if legacy_local_read {
        policy_data["legacy_local_read"] = json!(true);
        completed_data["legacy_local_read"] = json!(true);
    }
    // Emit the descriptive policy summary on the generic audit stream before the
    // control completion event, leaving closed broker.batch.export facts intact.
    client
        .post_event(event(plan, "workspace.export.policy", policy_data))
        .await?;
    emit(client, plan, "workspace.export.completed", completed_data).await?;
    Ok(())
}

pub(super) async fn export_workspace(
    session: Uuid,
    output: PathBuf,
    config: &Path,
    gateway: &str,
    program: Option<&Path>,
    adopt: bool,
) -> Result<usize> {
    let destination = crate::control_fs::absolute_path(&output)?;
    let (state, client, _guard) = locked_session(session, config, gateway).await?;
    let worker = state.worker()?;
    anyhow::ensure!(
        worker.state == WorkerStateV1::Terminal && !worker.quota_held,
        "export requires a durably terminal worker; stop or resume it first"
    );
    let runtime = bound_runtime(&state, &client, program, adopt).await?;
    let plan = state.snapshot()?;
    runtime.doctor(plan.options.allow_rootful_runtime).await?;
    emit(&client,&plan,"workspace.export.requested",json!({"worker_id":worker.worker_id,"volume":worker.attempt.volume_name,"original_mounted":false})).await?;
    let bytes = managed_archive(&runtime, &client, &plan, &worker).await?;
    let original = plan.options.project.clone();
    let report = tokio::task::spawn_blocking(move || {
        workspace::extract_workspace_with_report(
            bytes.as_slice(),
            &output,
            &original,
            Limits::default(),
        )
    })
    .await??;
    publish_export_audit_events(
        &client,
        &plan,
        Some(&worker.worker_id),
        false,
        &destination,
        &report,
    )
    .await?;
    Ok(report.manifest.len())
}

pub(super) async fn export_legacy_workspace(
    plan: RunPlan,
    session: Uuid,
    output: PathBuf,
    config: &Path,
    gateway: &str,
    program: Option<&Path>,
    adopt: bool,
) -> Result<usize> {
    anyhow::ensure!(
        plan.version == 1
            && plan.managed.is_none()
            && plan.session == session
            && plan.options.config_dir
                == crate::control_fs::Directory::open_existing(&crate::control_fs::absolute_path(
                    config
                )?)?
                .path(),
        "legacy session identity or private configuration differs"
    );
    anyhow::ensure!(
        adopt,
        "legacy export has no durable invocation; explicitly approve it with --adopt-runtime-program"
    );
    let destination = crate::control_fs::absolute_path(&output)?;
    let approved = approved_runtime(&plan.options, program)?;
    let token = read_token(&config.join("secrets/runner"))?;
    let client = AuditClient::new(gateway, &token)?;
    anyhow::ensure!(
        configured_origin(&client)
            == configured_origin(&AuditClient::new(&plan.options.gateway, &token)?),
        "legacy export must use the original authenticated gateway origin"
    );
    check_identity(&client, Some("local")).await?;
    let expected_volume = crate::runtime::volume_name(session);
    let events = client
        .get_json(&format!(
            "api/v1/events?sandbox_id={session}&kind=runtime.submitted&limit=2"
        ))
        .await?;
    let items = events["items"]
        .as_array()
        .context("legacy audit query returned no event collection")?;
    anyhow::ensure!(
        items.len() == 1
            && items[0]["event"]["session_id"] == session.to_string()
            && items[0]["event"]["data"]["volume"] == expected_volume,
        "legacy volume is not linked to one submission in the authenticated local audit"
    );
    emit(
        &client,
        &plan,
        "workspace.export.requested",
        json!({"session_id":session,"legacy_local_read":true,"original_mounted":false}),
    )
    .await?;
    let (_name, seed, image) = prepare_authority(&plan, &approved).await?;
    let runtime = ManagedRuntime::reopen_with_runtime(
        seed.binding.context(
            "legacy Podman export requires a pre-existing private control authority binding",
        )?,
        approved,
    )
    .await?;
    let operation = Uuid::new_v4();
    let spec = ContainerSpec::with_operation(
        session,
        ContainerPurpose::Export,
        &image,
        vec![
            "/bin/tar".into(),
            "--create".into(),
            "--file=-".into(),
            "--directory=/workspace".into(),
            ".".into(),
        ],
        NetworkMode::None,
        operation,
    )?
    .with_managed_names(
        &format!("agentsandbox-legacy-export-{}", operation.simple()),
        &expected_volume,
    )?;
    let retained = runtime
        .execute_retained(
            &spec,
            None,
            Duration::from_secs(120),
            Limits::default().max_archive_bytes(),
            |owned| {
                let client = client.clone();
                let plan = plan.clone();
                async move {
                    emit(
                        &client,
                        &plan,
                        "workspace.scan.created",
                        json!({"container_id":owned.handle.id(),"legacy_local_read":true}),
                    )
                    .await?;
                    Ok(StartDirective::EmitOnce)
                }
            },
            |_| async { Ok(()) },
        )
        .await?;
    anyhow::ensure!(
        retained.result().state.exit_code == 0
            && !retained.result().stdout.truncated
            && !retained.result().timed_out,
        "legacy archive helper failed; evidence retained"
    );
    emit(
        &client,
        &plan,
        "workspace.scan.completed",
        json!({"legacy_local_read":true,"archive_bytes":retained.result().stdout.total_bytes}),
    )
    .await?;
    let bytes = retained.result().stdout.bytes.clone();
    runtime.cleanup(&retained).await?;
    let original = plan.options.project.clone();
    let report = tokio::task::spawn_blocking(move || {
        workspace::extract_workspace_with_report(
            bytes.as_slice(),
            &output,
            &original,
            Limits::default(),
        )
    })
    .await??;
    publish_export_audit_events(&client, &plan, None, true, &destination, &report).await?;
    Ok(report.manifest.len())
}

async fn recover_import(
    runtime: &ManagedRuntime,
    client: &AuditClient,
    state: &PlanState,
    guard: &AttemptGuard,
    wait_seconds: u64,
) -> Result<bool> {
    guard.ensure_current()?;
    let plan = state.snapshot()?;
    let checkpoint = managed(&plan)?;
    let worker = state.worker()?;
    anyhow::ensure!(
        worker.quota_held
            && worker.attempt.create_operation_id.is_none()
            && worker.attempt.container_id.is_none()
            && worker.attempt.start_operation_id.is_none()
            && worker.attempt.created_inspection.is_none()
            && !worker.allocation_inconsistent,
        "preparation cannot be recovered after a client creation or start intent"
    );
    let Some(helper) = checkpoint.import.clone() else {
        return Ok(false);
    };
    // Open and hash the immutable snapshot before any recovery permission.
    let archive = state
        .directory
        .archive(&checkpoint.archive_name, &checkpoint.archive_digest)?;
    let spec = helper_spec(
        &plan,
        &worker,
        ContainerPurpose::Import,
        helper.operation_id,
        vec![
            "/bin/tar".into(),
            "--extract".into(),
            "--file=-".into(),
            "--directory=/workspace".into(),
            "--no-same-owner".into(),
            "--no-same-permissions".into(),
        ],
    )?;
    let found = runtime.lookup_created(&spec).await?;
    let Some(found) = found else {
        return Ok(helper.completed);
    };
    if let Some(owned) = &helper.owned {
        anyhow::ensure!(
            owned.handle.id() == found.handle.id()
                && owned.observation.configuration_fingerprint.is_some(),
            "import helper identity changed; no instance adopted"
        );
        found
            .observation
            .validate_identity_allocation(&owned.observation)?;
    }
    state.update(|plan| {
        plan.managed
            .as_mut()
            .and_then(|checkpoint| checkpoint.import.as_mut())
            .context("missing import checkpoint")?
            .owned = Some(found.clone());
        Ok(())
    })?;
    let mut observed = found.observation.clone();
    let mut writer_acknowledged = false;
    if !helper.start_issued {
        anyhow::ensure!(
            observed.process.started_at.is_none() && observed.process.running == Some(false),
            "helper started without its durable local emission claim"
        );
        let retained=runtime.start_owned_retained(&spec,&found,Some(archive),Duration::from_secs(120),16*1024,|owned|{
            let state=state.clone();let client=client.clone();let plan=plan.clone();async move {
                state.update(|plan|{let helper=plan.managed.as_mut().and_then(|checkpoint|checkpoint.import.as_mut()).context("missing import checkpoint")?;
                    anyhow::ensure!(!helper.start_issued,"helper emission was already consumed");helper.start_issued=true;helper.owned=Some(owned.clone());Ok(())})?;
                emit(&client,&plan,"workspace.import.created",json!({"container_id":owned.handle.id(),"recovered_before_first_start":true})).await?;
                Ok(StartDirective::EmitOnce)
            }
        },|_|async{Ok(())}).await?;
        observed = retained.terminal().clone();
        writer_acknowledged = true;
    } else {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(wait_seconds);
        while observed.process.running == Some(true) && tokio::time::Instant::now() < deadline {
            let current = refresh(client, state).await?;
            if current.state == WorkerStateV1::Stopping
                || current
                    .unknown_reason
                    .as_deref()
                    .is_some_and(|reason| reason.starts_with("stop requested"))
            {
                runtime.stop_snapshot_owned(&found).await?;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            observed = runtime.observe(&found.handle).await?;
        }
    }
    if observed
        .validate_terminal(plan.options.runtime, &found.observation)
        .is_err()
        || observed.process.exit_code != Some(0)
    {
        return Ok(false);
    }
    if !helper.completed && !writer_acknowledged {
        // A terminal tar after parent death does not independently prove that
        // all archive bytes reached stdin. Compare the real volume contents.
        let actual = managed_scan(runtime, client, &plan, &worker).await?;
        let actual = workspace::manifest_at_recorded_coverage(&checkpoint.before, &actual);
        anyhow::ensure!(
            actual == checkpoint.before,
            "recovered import does not match the immutable admitted copy manifest"
        );
    }
    emit(client,&plan,"workspace.import.recovered",json!({"operation_id":helper.operation_id,"container_id":found.handle.id(),"manifest_digest":worker.launch.workspace_manifest_digest,"runtime_started_at":observed.process.started_at,"runtime_finished_at":observed.process.finished_at})).await?;
    state.update(|plan| {
        plan.managed
            .as_mut()
            .and_then(|checkpoint| checkpoint.import.as_mut())
            .context("missing import checkpoint")?
            .completed = true;
        Ok(())
    })?;
    if let Err(error) = runtime.cleanup_terminal(&found.handle, &observed).await {
        emit(client,&plan,"workspace.import.cleanup.failed",json!({"container_id":found.handle.id(),"error":crate::redaction::clip(&redacted_error(&client.redactor,&error),1000)})).await?;
    }
    Ok(true)
}

#[cfg(all(test, unix))]
mod export_and_mediation_tests {
    use super::*;
    use crate::{
        api::{GatewayState, TenantGateway, gateway_router},
        audit::{EventQuery, Store},
        policy::Policy,
    };
    use std::os::unix::fs::PermissionsExt;

    struct LiveTestGateway {
        _directory: tempfile::TempDir,
        store: Arc<Store>,
        client: AuditClient,
        server: tokio::task::JoinHandle<()>,
    }

    impl Drop for LiveTestGateway {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    impl LiveTestGateway {
        async fn start() -> Self {
            let directory = tempfile::Builder::new()
                .permissions(fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap();
            let store = Arc::new(
                Store::open(
                    &directory.path().join("audit.sqlite"),
                    Redactor::new(vec![]),
                    1000,
                )
                .unwrap(),
            );
            let state = GatewayState::for_tenants(
                vec![TenantGateway {
                    tenant_id: "local".into(),
                    store: store.clone(),
                    runner_token: "fixture-runner".into(),
                    viewer_token: "fixture-viewer".into(),
                }],
                Policy::default(),
                None,
                Redactor::new(vec![]),
            )
            .unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let client = AuditClient::new(
                &format!("http://{}", listener.local_addr().unwrap()),
                "fixture-runner",
            )
            .unwrap();
            let server = tokio::spawn(async move {
                axum::serve(listener, gateway_router(state)).await.unwrap();
            });
            Self {
                _directory: directory,
                store,
                client,
                server,
            }
        }
    }

    pub(super) fn build_fixture_plan(root: &Path, with_provider: bool) -> RunPlan {
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("README.txt"), "ordinary project data").unwrap();
        let config = root.join("config");
        if !config.exists() {
            crate::operations::initialize_config(&config).unwrap();
        }
        let mut options = RunOptions {
            project: project.clone(),
            config_dir: config.clone(),
            gateway: "http://127.0.0.1:1".into(),
            runtime: RuntimeKind::Docker,
            runtime_program: None,
            runtime_endpoint: Some("unix:///fixture.sock".into()),
            image: "worker:fixture".into(),
            profile: if with_provider { "observe" } else { "strict" }.into(),
            network: if with_provider {
                NetworkMode::Bridge
            } else {
                NetworkMode::None
            },
            allow_rootful_runtime: false,
            timeout: 5,
            provider: "generic".into(),
            client: None,
            auth_mode: None,
            client_profile: None,
            providers_file: None,
            prompt: None,
            model: None,
            approve_hash: None,
            capture_output: false,
            interactive: false,
            secret_env_file: None,
            argv: if with_provider {
                Vec::new()
            } else {
                vec!["/usr/bin/id".into()]
            },
        };
        if with_provider {
            let source = project.join(".env");
            fs::write(&source, "OPENAI_API_KEY=fixture-provider-secret-value\n").unwrap();
            fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
            write_json_new(
                &config.join("providers.json"),
                &crate::provider::ProviderConfigV1 {
                    version: 1,
                    default_profile: Some("codex-api".into()),
                    auto_priority: vec!["codex-api".into()],
                    profiles: vec![crate::provider::ProviderProfileV1 {
                        id: "codex-api".into(),
                        revision: 1,
                        client: crate::provider::ClientKind::Codex,
                        auth_mode: crate::provider::AuthMode::Api,
                        model: None,
                        credential: crate::provider::CredentialSourceV1::ApiEnvFile {
                            path: source,
                            key: Some("OPENAI_API_KEY".into()),
                        },
                    }],
                },
            )
            .unwrap();
            options.client = Some("codex".into());
            options.auth_mode = Some(crate::provider::AuthPreference::Api);
            options.prompt = Some("inspect".into());
        }
        choose_plan(options).unwrap()
    }

    #[tokio::test]
    async fn command_requested_event_includes_launch_only_mediation_and_passes_control_ingest() {
        let gateway = LiveTestGateway::start().await;
        for with_provider in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().canonicalize().unwrap();
            let plan = build_fixture_plan(&root, with_provider);
            let before_hash = crate::policy::request_hash(&plan.request);
            let requested = build_command_requested_event(&plan).unwrap();
            assert_eq!(
                requested["data"]["mediation"],
                json!({"scope": "launch_only", "tool_decision_channel": false})
            );
            assert_eq!(crate::policy::request_hash(&plan.request), before_hash);
            gateway.client.post_control_event(requested).await.unwrap();
        }
        let rows = gateway
            .store
            .events(&EventQuery {
                order: Some("asc".into()),
                ..EventQuery::default()
            })
            .unwrap();
        let requested_rows: Vec<_> = rows
            .iter()
            .filter(|row| row.event.kind == "command.requested")
            .collect();
        assert_eq!(requested_rows.len(), 2);
        for row in requested_rows {
            assert_eq!(
                row.event.data["mediation"],
                json!({"scope": "launch_only", "tool_decision_channel": false})
            );
        }
    }

    #[tokio::test]
    async fn export_audit_events_bind_manifest_and_destination_hashes_without_raw_paths() {
        let gateway = LiveTestGateway::start().await;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let plan = build_fixture_plan(&root, false);

        let mut tar_builder = tar::Builder::new(Vec::new());
        let mut kept_header = tar::Header::new_gnu();
        let kept_bytes = b"verified exported content";
        kept_header.set_entry_type(tar::EntryType::Regular);
        kept_header.set_mode(0o644);
        kept_header.set_size(kept_bytes.len() as u64);
        tar_builder
            .append_data(&mut kept_header, "kept.txt", &kept_bytes[..])
            .unwrap();
        let mut hidden_header = tar::Header::new_gnu();
        let hidden_bytes = b"[core]\n";
        hidden_header.set_entry_type(tar::EntryType::Regular);
        hidden_header.set_mode(0o644);
        hidden_header.set_size(hidden_bytes.len() as u64);
        tar_builder
            .append_data(&mut hidden_header, ".git/config", &hidden_bytes[..])
            .unwrap();
        let archive = tar_builder.into_inner().unwrap();

        let secret_parent = root.join("secret-customer-canary-dir");
        fs::create_dir(&secret_parent).unwrap();
        let managed_dest = secret_parent.join("export-managed-out");
        let managed_report = workspace::extract_workspace_with_report(
            archive.as_slice(),
            &managed_dest,
            &plan.options.project,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(managed_report.manifest.len(), 1);
        assert_eq!(managed_report.omitted_entries, 1);

        let managed_abs = crate::control_fs::absolute_path(&managed_dest).unwrap();
        publish_export_audit_events(
            &gateway.client,
            &plan,
            Some("worker-export-1"),
            false,
            &managed_abs,
            &managed_report,
        )
        .await
        .unwrap();

        let legacy_dest = secret_parent.join("export-legacy-out");
        let legacy_report = workspace::extract_workspace_with_report(
            archive.as_slice(),
            &legacy_dest,
            &plan.options.project,
            Limits::default(),
        )
        .unwrap();
        let legacy_abs = crate::control_fs::absolute_path(&legacy_dest).unwrap();
        publish_export_audit_events(
            &gateway.client,
            &plan,
            None,
            true,
            &legacy_abs,
            &legacy_report,
        )
        .await
        .unwrap();

        let expected_manifest_digest =
            content_hash("ai-guard:workspace-manifest:v1", &managed_report.manifest).unwrap();
        let expected_managed_dest_hash = content_hash(
            "ai-guard:export-destination:v1",
            &managed_abs.to_string_lossy(),
        )
        .unwrap();
        let expected_legacy_dest_hash = content_hash(
            "ai-guard:export-destination:v1",
            &legacy_abs.to_string_lossy(),
        )
        .unwrap();
        assert_ne!(expected_managed_dest_hash, expected_legacy_dest_hash);

        let rows = gateway
            .store
            .events(&EventQuery {
                order: Some("asc".into()),
                ..EventQuery::default()
            })
            .unwrap();
        let policies: Vec<_> = rows
            .iter()
            .filter(|row| row.event.kind == "workspace.export.policy")
            .collect();
        let completions: Vec<_> = rows
            .iter()
            .filter(|row| row.event.kind == "workspace.export.completed")
            .collect();
        assert_eq!(policies.len(), 2);
        assert_eq!(completions.len(), 2);

        assert_eq!(policies[0].event.data["version"], 1);
        assert_eq!(
            policies[0].event.data["request_id"],
            plan.request.request_id
        );
        assert_eq!(
            policies[0].event.data["manifest_digest"],
            expected_manifest_digest
        );
        assert_eq!(
            policies[0].event.data["destination_hash"],
            expected_managed_dest_hash
        );
        assert_eq!(policies[0].event.data["omitted_entries"], 1);
        assert_eq!(policies[0].event.data["files"], 1);
        assert_eq!(policies[0].event.data["worker_id"], "worker-export-1");

        assert_eq!(completions[0].event.data["files"], 1);
        assert_eq!(completions[0].event.data["omitted_entries"], 1);
        assert_eq!(
            completions[0].event.data["manifest_digest"],
            expected_manifest_digest
        );
        assert_eq!(
            completions[0].event.data["destination_hash"],
            expected_managed_dest_hash
        );
        assert_eq!(completions[0].event.data["worker_id"], "worker-export-1");
        assert!(completions[0].event.data.get("legacy_local_read").is_none());

        assert_eq!(
            policies[1].event.data["destination_hash"],
            expected_legacy_dest_hash
        );
        assert_eq!(policies[1].event.data["legacy_local_read"], true);
        assert!(policies[1].event.data.get("worker_id").is_none());

        assert_eq!(
            completions[1].event.data["destination_hash"],
            expected_legacy_dest_hash
        );
        assert_eq!(completions[1].event.data["legacy_local_read"], true);
        assert!(completions[1].event.data.get("worker_id").is_none());

        for row in policies.into_iter().chain(completions) {
            let serialized = serde_json::to_string(&row.event).unwrap();
            for sensitive_fragment in [
                "secret-customer-canary-dir",
                "export-managed-out",
                "export-legacy-out",
            ] {
                assert!(
                    !serialized.contains(sensitive_fragment),
                    "raw destination fragment leaked in {}: {serialized}",
                    row.event.kind
                );
            }
        }

        let traversal_err = export_workspace(
            Uuid::new_v4(),
            PathBuf::from("../escape"),
            &root.join("missing-config"),
            "http://127.0.0.1:1",
            None,
            false,
        )
        .await
        .unwrap_err();
        assert!(
            traversal_err
                .to_string()
                .contains("control path must not contain parent traversal"),
            "{traversal_err}"
        );
    }
}
