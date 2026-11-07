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
            let trusted = self.trusted.child(name)?;
            let file = trusted.try_clone_file()?;
            Self::check_directory(&file)?;
            trusted.revalidate()?;
            Ok(Self { file, trusted })
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
            Ok(Some(serde_json::from_reader(file)?))
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
    pub(super) struct AttemptGuard {
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
                file,
                directory,
                name,
                identity,
            };
            guard.ensure_current()?;
            Ok(guard)
        }
        pub(super) fn identity(&self) -> LockIdentity {
            self.identity
        }
        pub(super) fn ensure_current(&self) -> Result<()> {
            self.directory.trusted.revalidate()?;
            check_private_file(&self.file)?;
            let raw = checked_name(&self.name)?;
            let fd = unsafe {
                libc::openat(
                    self.directory.file.as_raw_fd(),
                    raw.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            anyhow::ensure!(fd >= 0, "active attempt lock was removed or substituted");
            let current = unsafe { fs::File::from_raw_fd(fd) };
            check_private_file(&current)?;
            let m = current.metadata()?;
            anyhow::ensure!(
                self.identity
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
    async fn get_typed<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        serde_json::from_value(self.get_json(path).await?)
            .context("gateway returned an incompatible control response")
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
                    .post(self.base.join(path)?)
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
    let identity = client.get_json("api/v1/identity").await?;
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
        anyhow::ensure!(
            endpoint == "local" || endpoint.starts_with("unix:///"),
            "Podman managed sessions require local or an explicit Unix endpoint; SSH/TCP authority is not implemented"
        );
        let mut args = if endpoint == "local" {
            vec!["--remote=false".into()]
        } else {
            vec!["--remote".into(), "--url".into(), endpoint]
        };
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
    client.post_event(pending).await?;
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
        let runtime = ManagedRuntime::bootstrap_podman_with_runtime(
            seed.authority_id,
            &seed.endpoint,
            approved,
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
        Some("deny") => bail!("policy denied the command; no worker was reserved or launched"),
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
            ManagedRuntime::bootstrap_podman_with_runtime(
                seed.authority_id,
                &seed.endpoint,
                approved.clone(),
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
    // A Created acknowledgement can be retried with its original identity.
    // Do not advance the registry revision with Unknown first: doing so would
    // invalidate a still-uncommitted Created or hide a committed Created with
    // no start claim. The private checkpoint keeps the owned immutable ID and
    // exact pending request; recovery reconciles it before any first start.
    if state.checkpoint().is_ok_and(|checkpoint| {
        checkpoint.owned.is_some()
            && checkpoint.pending.as_ref().is_some_and(|pending| {
                matches!(pending.transition, WorkerTransitionV1::Created { .. })
            })
    }) {
        return;
    }
    if refresh(client, state)
        .await
        .is_ok_and(|worker| worker.quota_held)
    {
        // Preserve the original operation identity; an acknowledgement failure
        // must not be silently retried as a new terminal or fresh start permit.
        if state
            .update(|plan| {
                let checkpoint = plan.managed.as_mut().context("missing checkpoint")?;
                if let Some(pending) = checkpoint.pending.take() {
                    anyhow::ensure!(
                        checkpoint.uncertain.len() < 64,
                        "too many unresolved control operations"
                    );
                    checkpoint.uncertain.push(pending);
                }
                Ok(())
            })
            .is_err()
        {
            return;
        }
        let reason = crate::redaction::clip(&client.redactor.text(reason), 1000);
        let _ = mutate(client, state, WorkerTransitionV1::Unknown { reason }).await;
    }
}
async fn post_saved_terminal(client: &AuditClient, state: &PlanState) -> Result<()> {
    let checkpoint = state.checkpoint()?;
    if !checkpoint.output_acknowledged
        && let Some(event) = checkpoint.output_event
    {
        client.post_event(event).await?;
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
    let mut requested = event(
        &plan,
        "command.requested",
        json!({"request_id":plan.request.request_id,"profile":plan.options.profile,"network":plan.options.network,"image":plan.options.image,"runtime":plan.options.runtime,"provider":plan.options.provider}),
    );
    if let Some(mode) = plan.request.execution_mode {
        requested["data"]["execution_mode"] = serde_json::to_value(mode)?;
    }
    if let Some(resolution) = &plan.request.provider_resolution {
        requested["data"]["provider_resolution"] = serde_json::to_value(resolution)?;
    }
    requested["command"] = serde_json::to_value(&plan.request.command)?;
    client.post_event(requested).await?;
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
    client.post_event(submitted).await?;
    if plan.options.interactive {
        // Acknowledged before the durable start claim, no keypresses or screen contents.
        emit(client, &plan, "runtime.terminal.attach.authorized", json!({"execution_mode":plan.request.execution_mode,"security_preset":worker.launch.security_preset,"worker_id":worker.worker_id,"attempt_id":worker.attempt.attempt_id,"reattach":false})).await?;
    }
    let on_created = |created: OwnedContainerSnapshot| {
        let state = state.clone();
        let client = client.clone();
        let guard = &guard;
        async move {
            guard.ensure_current()?;
            state.update(|plan| {
                plan.managed.as_mut().context("missing checkpoint")?.owned = Some(created.clone());
                Ok(())
            })?;
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
            Ok(
                if claimed.disposition == Some(ClaimDispositionV1::EmitOnce) {
                    StartDirective::EmitOnce
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
            client.post_event(event).await?;
            // Tolerance begins only after both Running and command.started
            // have been acknowledged. The callback itself has no retry here.
            if state.worker()?.state == WorkerStateV1::Running {
                let _ = start_acknowledged.send(true);
            }
            Ok(())
        }
    };
    let (done, completed) = tokio::sync::watch::channel(false);
    // This earlier bound is deliberately conservative: it cannot extend the
    // runtime's own lifetime while preparation/start observation is pending.
    let execution_deadline =
        tokio::time::Instant::now() + Duration::from_secs(plan.options.timeout);
    let execute = async {
        let result = if let Some(recovered) = &recovered {
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
        };
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
    );
    let (execution, monitor) = tokio::join!(execute, monitor);
    let retained = match execution {
        Ok(retained) => retained,
        Err(error) => {
            mark_unknown(client, state, &redacted_error(&client.redactor, &error)).await;
            return Err(error);
        }
    };
    let mut summary = match finish_execution(runtime, client, state, &retained).await {
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
    if let Err(error) = monitor {
        summary.postprocessing_error = Some(crate::redaction::clip(
            &format!(
                "control observation failed during execution: {}",
                redacted_error(&client.redactor, &error)
            ),
            8192,
        ));
        state.update(|plan| {
            plan.managed.as_mut().context("missing checkpoint")?.result = Some(summary.clone());
            Ok(())
        })?;
    }
    Ok(summary)
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
    };
    state.update(|plan| {
        let checkpoint = plan.managed.as_mut().context("missing checkpoint")?;
        checkpoint.output_event = Some(client.redactor.value(&completed));
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
    let postprocessing=async {
        if plan.options.interactive {
            let mut coverage = event(&plan,"observer.coverage",json!({"provider":plan.options.provider,"collection":"lifecycle_and_workspace_diff","provider_events":"unavailable_for_tty","nested_authorization":false,"raw_output_capture":false,"separate_stdout_stderr":"unavailable_for_tty"}));
            coverage["coverage"] = json!("generic");
            client.post_event(coverage).await?;
        } else {
        let context=crate::adapters::AdapterContext{session_id:plan.session.to_string(),project_id:plan.request.project_id.clone().unwrap_or_default(),sandbox_id:plan.request.sandbox_id.clone().unwrap_or_default(),agent_id:plan.request.agent_id.clone().unwrap_or_default(),correlation_id:plan.request.correlation_id.clone().unwrap_or_default(),trace_id:plan.request.trace_id.clone().unwrap_or_default()};
        let capture = crate::adapters::parse_redacted_capture(
            &plan.options.provider,
            &execution.stdout.bytes,
            execution.stdout.truncated,
            &context,
            &client.redactor,
        );
        let normalized = capture.events.len();
        for event in capture.events {
            client.post_event(event).await?;
        }
        emit(client,&plan,"observer.coverage",json!({"provider":plan.options.provider,"collection":"after_completion","normalized_events":normalized,"maximum_events":crate::adapters::MAX_EVENTS_PER_CAPTURE,"stdout_truncated":execution.stdout.truncated,"unterminated_final_line_omitted":capture.unterminated_final_line_omitted,"unknown_or_oversized_lines":"ignored","nested_authorization":false,"raw_output_capture":plan.options.capture_output})).await?;
        }
        let after=managed_scan(runtime,client,&plan,&state.worker()?).await?;let changes=workspace::diff_manifests(&state.checkpoint()?.before,&after);let count=changes.len();
        for change in changes.into_iter().take(200){emit(client,&plan,&format!("file.{}",change.change),serde_json::to_value(change)?).await?;}
        emit(client,&plan,"workspace.diff.completed",json!({"changed_files":count,"emitted_changes":count.min(200),"truncated":count>200})).await?;
        Ok::<usize,anyhow::Error>(count)
    }.await;
    match postprocessing {
        Ok(count) => summary.changed_files = Some(count),
        Err(error) => {
            summary.postprocessing_error = Some(crate::redaction::clip(
                &redacted_error(&client.redactor, &error),
                8192,
            ))
        }
    }
    state.update(|plan| {
        plan.managed.as_mut().context("missing checkpoint")?.result = Some(summary.clone());
        Ok(())
    })?;
    Ok(summary)
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
    let (state, _client) = load_session(session, config, gateway).await?;
    action_status(
        &state,
        None,
        "Registry state read from the authenticated tenant; runtime facts carry their original observation time.",
    )
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
        post_saved_terminal(&client, &state).await?;
        return action_status(
            &state,
            None,
            "Terminal allocation and cleanup are already durable; no runtime action was repeated.",
        );
    }
    // A fresh start may use only the exact credential bytes admitted earlier.
    // Prepare them before any recovery helper and retain their handles through
    // create. Consumed starts and terminal reconciliation need no provider key.
    let credentials = if worker.attempt.start_operation_id.is_none() && worker.quota_held {
        private_credentials(&state.snapshot()?)?
    } else {
        None
    };
    let runtime = bound_runtime(&state, &client, program, adopt).await?;
    runtime
        .doctor(state.snapshot()?.options.allow_rootful_runtime)
        .await?;
    if worker.state == WorkerStateV1::Reserved {
        mutate(&client, &state, WorkerTransitionV1::Prepare).await?;
        import_workspace(&runtime, &client, &state, &guard).await?;
        mutate(&client, &state, WorkerTransitionV1::CreateIssued).await?;
        let result = launch_worker(
            &runtime,
            &client,
            &state,
            &guard,
            credentials.as_ref(),
            None,
        )
        .await?;
        return Ok(serde_json::to_value(result)?);
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
        if worker
            .unknown_reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("stop requested"))
        {
            return action_status(
                &state,
                None,
                "A stop was requested during preparation. Its reservation remains held until safe operator reconciliation.",
            );
        }
        let checkpoint = state.checkpoint()?;
        let ready = if checkpoint.import.is_some() {
            recover_import(&runtime, &client, &state, &guard, wait_seconds).await?
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
                mutate(&client, &state, WorkerTransitionV1::Prepare).await?;
            }
            import_workspace(&runtime, &client, &state, &guard).await?;
            true
        };
        if ready {
            if state.worker()?.state == WorkerStateV1::InDoubt {
                mutate(&client, &state, WorkerTransitionV1::Prepare).await?;
            }
            mutate(&client, &state, WorkerTransitionV1::CreateIssued).await?;
            let result = launch_worker(
                &runtime,
                &client,
                &state,
                &guard,
                credentials.as_ref(),
                None,
            )
            .await?;
            return Ok(serde_json::to_value(result)?);
        }
        mark_unknown(
            &client,
            &state,
            "broker interrupted during import; no complete helper/copy proof is available",
        )
        .await;
        return action_status(
            &state,
            None,
            "Import emission or contents remain uncertain. Helper, volume and reservation were retained; no import was repeated.",
        );
    }
    let Some(owned) = recover_owned(&runtime, &state).await? else {
        mark_unknown(&client,&state,"owned runtime instance was not found in a successful targeted inventory; terminal state remains unknown").await;
        return action_status(
            &state,
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
            control_monitor::require_stop_on_active_resume(&state),
        );
        match stop {
            Ok(stopped) => observed = stopped,
            Err(_) => {
                let reason = match epoch {
                    Err(error) => redacted_error(&client.redactor, &error),
                    Ok(()) => "active resume cannot verify the original execution lifetime".into(),
                };
                mark_unknown(&client, &state, &reason).await;
                return action_status(
                    &state,
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
        let result = launch_worker(
            &runtime,
            &client,
            &state,
            &guard,
            credentials.as_ref(),
            Some(owned),
        )
        .await?;
        return Ok(serde_json::to_value(result)?);
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(wait_seconds);
    let mut recorded_running = false;
    loop {
        guard.ensure_current()?;
        if observed
            .validate_terminal(state.snapshot()?.options.runtime, &owned.observation)
            .is_ok()
            && state.worker()?.attempt.start_operation_id.is_some()
        {
            return finalize_recovered(&runtime, &client, &state, &owned, &observed).await;
        }
        if observed.process.running == Some(true) {
            observed.validate_identity_allocation(&owned.observation)?;
            if !recorded_running {
                record_observation(&client, &state, &observed).await?;
                recorded_running = true;
            }
            if state.worker()?.state == WorkerStateV1::Stopping {
                runtime.stop_snapshot_owned(&owned).await?;
            }
        } else {
            mark_unknown(&client,&state,"runtime process is not proven terminal or has no durable start claim; pending effects remain unknown").await;
            return action_status(
                &state,
                Some(&observed),
                "No terminal proof is available. Prior start emission may be unresolved; no second start or quota release was performed.",
            );
        }
        if tokio::time::Instant::now() >= deadline {
            return action_status(
                &state,
                Some(&observed),
                "Worker is still observed running. This resume command's observation window ended; attach output from the interrupted broker is unknown.",
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        refresh(&client, &state).await?;
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
    let result: MutationResultV1 = client
        .post_control(
            &format!("api/v1/workers/{}/transitions", worker.worker_id),
            &mutation,
        )
        .await?;
    anyhow::ensure!(
        result.receipt.operation_id == mutation.operation_id
            && result.receipt.input_hash == content_hash("ai-guard:worker-mutation:v1", &mutation)?
            && result.receipt.audit_seq > 0,
        "stop intent lacks its exact durable acknowledgement"
    );
    validate_worker(&plan, &result.worker)?;
    directory.write(
        &format!("{}-ack.json", mutation.operation_id),
        &result.receipt,
    )?;
    Ok(result.worker)
}
pub(super) async fn stop_worker(
    session: Uuid,
    config: &Path,
    gateway: &str,
    program: Option<&Path>,
    adopt: bool,
) -> Result<Value> {
    let (state, client) = load_session(session, config, gateway).await?;
    let worker = state.worker()?;
    if [WorkerStateV1::Cancelled, WorkerStateV1::Terminal].contains(&worker.state) {
        return action_status(
            &state,
            None,
            "Worker is already terminal or cancelled; no stop was emitted.",
        );
    }
    let transition = if worker.state == WorkerStateV1::Reserved {
        WorkerTransitionV1::CancelReserved
    } else if worker.attempt.container_id.is_some() {
        WorkerTransitionV1::Stop
    } else {
        WorkerTransitionV1::Unknown {
            reason: "stop requested during preparation; worker allocation remains uncertain".into(),
        }
    };
    let acknowledged = out_of_band_stop(&client, &state, transition).await?;
    state.read_worker(acknowledged.clone())?;
    if acknowledged.state == WorkerStateV1::Cancelled {
        return action_status(
            &state,
            None,
            "The never-prepared reservation was cancelled durably.",
        );
    }
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
pub(super) async fn export_workspace(
    session: Uuid,
    output: PathBuf,
    config: &Path,
    gateway: &str,
    program: Option<&Path>,
    adopt: bool,
) -> Result<usize> {
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
    let manifest = tokio::task::spawn_blocking(move || {
        workspace::extract_workspace(bytes.as_slice(), &output, &original, Limits::default())
    })
    .await??;
    emit(
        &client,
        &plan,
        "workspace.export.completed",
        json!({"files":manifest.len()}),
    )
    .await?;
    Ok(manifest.len())
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
    let manifest = tokio::task::spawn_blocking(move || {
        workspace::extract_workspace(bytes.as_slice(), &output, &original, Limits::default())
    })
    .await??;
    emit(
        &client,
        &plan,
        "workspace.export.completed",
        json!({"files":manifest.len(),"legacy_local_read":true}),
    )
    .await?;
    Ok(manifest.len())
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
