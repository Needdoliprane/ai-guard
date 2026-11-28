use super::launch_profile::{DockerLaunchProfileBindingV1, RetainedDockerLaunchProfile};
use super::*;
use sha2::{Digest, Sha256};
use std::path::Path;

mod batch_observation;
pub(super) use batch_observation::{
    BatchObservationScope, NativeBatchTerminal, runtime_state as batch_runtime_state,
};

#[cfg(all(test, unix))]
mod deadline_semantic_tests;
#[cfg(all(test, unix))]
mod legacy_completion_tests;

pub(crate) mod activity;
pub(crate) mod relay;

#[cfg(all(test, unix))]
mod preclaim_outcome_tests {
    use super::*;
    use crate::{broker::journal::RevisionRefV1, control_fs::Directory};
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        sync::{Arc, Mutex},
    };

    /// Uses the existing native CLI fixture, not a Docker/Podman engine. This
    /// checks only runtime dispatch after a crate-internal retirement result;
    /// it does not establish durable Final, actor join or real confinement.
    #[tokio::test]
    async fn abandoned_prearmed_ports_retain_created_without_start_construction_or_later_cli() {
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("engine.rs");
        let program = root.join("engine");
        fs::write(
            &source,
            include_str!("../../tests/runtime_fixtures/managed.rs"),
        )
        .unwrap();
        let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let compiled = tokio::time::timeout(
            Duration::from_secs(30),
            Command::new(compiler)
                .args([
                    "--edition=2024",
                    "--crate-name",
                    "abandoned_prearmed_fixture",
                ])
                .arg(&source)
                .arg("-o")
                .arg(&program)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .expect("the controlled Rust CLI fixture compiles within its harness budget")
        .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let metadata = program.with_extension("fixture");
        fs::create_dir(&metadata).unwrap();
        fs::write(
            metadata.join("AI_GUARD_MANAGED_ROOT"),
            root.as_os_str().as_encoded_bytes(),
        )
        .unwrap();
        fs::write(metadata.join("AI_GUARD_MANAGED_CHILD"), b"success").unwrap();
        let runtime = Runtime::with_program(RuntimeKind::Docker, &program).unwrap();
        let managed = ManagedRuntime::connect_docker_with_runtime(
            Uuid::from_u128(3),
            "unix:///fixture/docker.sock",
            runtime,
        )
        .await
        .unwrap();
        let profile_binding = DockerLaunchProfileBindingV1::new(
            &"1".repeat(64),
            Uuid::from_u128(1),
            &"2".repeat(64),
            &managed.binding.endpoint,
            &managed.runtime.invocation().fingerprint(),
        )
        .unwrap();
        let directory = Directory::open_existing(&root).unwrap();
        let profile = RetainedDockerLaunchProfile::create(&directory, &profile_binding).unwrap();
        let profile_file = profile
            .directory_path()
            .join(crate::runtime::launch_profile::FILE);
        let managed = managed.bind_docker_launch_profile(profile).unwrap();
        // A start-command construction would now fail, independently of whether
        // the fixture would report a successful start. STOP does not need it.
        fs::remove_file(profile_file).unwrap();
        assert!(managed.prearmed_command().is_err());
        let spec = ContainerSpec::with_operation(
            Uuid::from_u128(4),
            ContainerPurpose::Run,
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            vec!["/usr/bin/id".into()],
            NetworkMode::None,
            Uuid::from_u128(5),
        )
        .unwrap()
        .with_managed_names("aig-fixture-worker", "aig-fixture-worker-work")
        .unwrap();
        let recorded = Arc::new(Mutex::new(None));
        let callback_recorded = Arc::clone(&recorded);
        let callback_root = root.clone();
        let outcome = managed
            .execute_prearmed_retained(
                &spec,
                None,
                1024,
                move |owned| {
                    *callback_recorded.lock().unwrap() = Some((
                        owned,
                        fs::read_to_string(callback_root.join("log")).unwrap(),
                    ));
                    async {
                        Ok(PrearmedStartDirective::AbandonedBeforeClaim(
                            AbandonedBeforeClaim::new(
                                Uuid::from_u128(1),
                                Uuid::from_u128(6),
                                RevisionRefV1::new(7, &"a".repeat(64)).unwrap(),
                            )
                            .unwrap(),
                        ))
                    }
                },
                |_| async { bail!("abandoned instance cannot invoke started callback") },
            )
            .await
            .unwrap();
        let ManagedPrearmedOutcome::Abandoned { owned, retirement } = outcome else {
            panic!("abandoned instance cannot become an executed transport");
        };
        let (callback_owned, callback_log) = recorded.lock().unwrap().take().unwrap();
        assert_eq!(
            serde_json::to_value(&owned).unwrap(),
            serde_json::to_value(callback_owned).unwrap()
        );
        assert_eq!(retirement.final_revision().revision(), 7);
        assert_eq!(fs::read_to_string(root.join("log")).unwrap(), callback_log);
        assert_eq!(fs::read_to_string(root.join("state")).unwrap(), "created");
        let callback_recorded = Arc::clone(&recorded);
        let callback_root = root.clone();
        let outcome = managed
            .start_owned_prearmed_retained(
                &spec,
                &owned,
                None,
                1024,
                move |owned| {
                    *callback_recorded.lock().unwrap() = Some((
                        owned,
                        fs::read_to_string(callback_root.join("log")).unwrap(),
                    ));
                    async {
                        Ok(PrearmedStartDirective::AbandonedBeforeClaim(
                            AbandonedBeforeClaim::new(
                                Uuid::from_u128(1),
                                Uuid::from_u128(6),
                                RevisionRefV1::new(7, &"a".repeat(64)).unwrap(),
                            )
                            .unwrap(),
                        ))
                    }
                },
                |_| async { bail!("abandoned instance cannot invoke started callback") },
            )
            .await
            .unwrap();
        let ManagedPrearmedOutcome::Abandoned {
            owned: recovered, ..
        } = outcome
        else {
            panic!("abandoned recovered instance cannot become an executed transport");
        };
        let (callback_owned, callback_log) = recorded.lock().unwrap().take().unwrap();
        assert_eq!(
            serde_json::to_value(&recovered).unwrap(),
            serde_json::to_value(callback_owned).unwrap()
        );
        assert_eq!(recovered.handle.id(), owned.handle.id());
        let log = fs::read_to_string(root.join("log")).unwrap();
        assert_eq!(log, callback_log);
        assert_eq!(
            log.lines()
                .filter(|line| line.starts_with("create "))
                .count(),
            1
        );
        for forbidden in ["start ", "kill ", "rm "] {
            assert!(!log.lines().any(|line| line.starts_with(forbidden)));
        }
        assert_eq!(fs::read_to_string(root.join("state")).unwrap(), "created");
    }
}

#[cfg(test)]
mod launch_profile_tests {
    use super::*;
    use crate::control_fs::Directory;
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn only_prearmed_command_uses_retained_profile_and_stop_does_not_open_blob() {
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = Directory::open_existing(&temp.path().canonicalize().unwrap()).unwrap();
        // The test binary is approved but never executed by this command test.
        let runtime =
            Runtime::with_program(RuntimeKind::Docker, &std::env::current_exe().unwrap()).unwrap();
        let profile_binding = DockerLaunchProfileBindingV1::new(
            &"1".repeat(64),
            Uuid::from_u128(1),
            &"2".repeat(64),
            "unix:///not-opened/docker.sock",
            &runtime.invocation().fingerprint(),
        )
        .unwrap();
        let retained = RetainedDockerLaunchProfile::create(&root, &profile_binding).unwrap();
        let mut authority = RuntimeAuthority {
            authority_id: Uuid::from_u128(2),
            kind: RuntimeKind::Docker,
            endpoint: profile_binding.runtime_endpoint.clone(),
            engine_id: Some("synthetic-engine".into()),
            invocation: Some(runtime.invocation().clone()),
            docker_launch_profile: None,
            podman: None,
        };
        let legacy_bytes = serde_json::to_value(&authority).unwrap();
        let old_fingerprint = authority.fingerprint();
        assert!(
            !legacy_bytes
                .as_object()
                .unwrap()
                .contains_key("docker_launch_profile")
        );
        authority.docker_launch_profile = Some(profile_binding.clone());
        assert_eq!(
            authority.fingerprint(),
            old_fingerprint,
            "historical engine fingerprint stays unchanged"
        );
        authority.validate_docker_launch_profile().unwrap();
        let managed = ManagedRuntime {
            runtime,
            binding: authority,
            launch_profile: Some(retained.clone()),
        };
        let (mut command, prepared, held) = managed.prearmed_command().unwrap();
        command.arg("start").arg("--attach").arg("synthetic-cid");
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_os_string())
            .collect();
        assert_eq!(
            args,
            vec![
                "--host".into(),
                profile_binding.runtime_endpoint.clone().into(),
                "--config".into(),
                retained.directory_path().as_os_str().to_owned(),
                "start".into(),
                "--attach".into(),
                "synthetic-cid".into()
            ]
        );
        assert_eq!(prepared, Some(profile_binding));
        assert!(held.is_some());
        fs::remove_file(
            retained
                .directory_path()
                .join(crate::runtime::launch_profile::FILE),
        )
        .unwrap();
        assert!(managed.prearmed_command().is_err());
        let mut stop = managed.command().unwrap();
        stop.arg("stop").arg("synthetic-cid");
        assert!(
            !stop.as_std().get_args().any(|arg| arg == "--config"),
            "STOP command construction has no blob dependency"
        );
        let without_cap = ManagedRuntime {
            launch_profile: None,
            ..managed
        };
        assert!(without_cap.prearmed_command().is_err());
    }
}

/// This binding belongs in the broker's private checkpoint, never in an API DTO.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeAuthority {
    pub authority_id: Uuid,
    pub kind: RuntimeKind,
    pub endpoint: String,
    pub engine_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation: Option<InvocationBindingV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) docker_launch_profile: Option<DockerLaunchProfileBindingV1>,
    podman: Option<PodmanBinding>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PodmanBinding {
    graph_root: String,
    run_root: String,
    marker_volume: String,
    nonce: Uuid,
    effective_uid: u32,
    selection_env: std::collections::BTreeMap<String, Option<String>>,
}
impl std::fmt::Debug for RuntimeAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeAuthority")
            .field("authority_id", &self.authority_id)
            .field("kind", &self.kind)
            .field("endpoint", &self.endpoint)
            .field("engine_id", &self.engine_id)
            .field("podman_marker", &self.podman.as_ref().map(|_| "[private]"))
            .finish()
    }
}
impl RuntimeAuthority {
    pub(crate) fn validate_docker_launch_profile(&self) -> Result<()> {
        if let Some(profile) = &self.docker_launch_profile {
            profile.validate()?;
            ensure!(
                self.kind == RuntimeKind::Docker
                    && self.endpoint == profile.runtime_endpoint
                    && self
                        .invocation
                        .as_ref()
                        .is_some_and(|invocation| invocation.policy_version == 2
                            && invocation.fingerprint() == profile.invocation_fingerprint),
                "Docker start profile conflicts with authority"
            );
        }
        Ok(())
    }
    pub fn invocation_fingerprint(&self) -> Option<String> {
        self.invocation
            .as_ref()
            .map(InvocationBindingV1::fingerprint)
    }
    pub fn fingerprint(&self) -> String {
        // Hash each length-prefixed component to avoid ambiguous concatenation.
        let mut digest = Sha256::new();
        let mut add = |value: &[u8]| {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value);
        };
        add(self.authority_id.as_bytes());
        add(self.kind.program().as_bytes());
        add(self.endpoint.as_bytes());
        add(self.engine_id.as_deref().unwrap_or_default().as_bytes());
        if let Some(podman) = &self.podman {
            add(podman.graph_root.as_bytes());
            add(podman.run_root.as_bytes());
            add(podman.marker_volume.as_bytes());
            add(podman.nonce.as_bytes());
            add(&podman.effective_uid.to_be_bytes());
            for (key, value) in &podman.selection_env {
                add(key.as_bytes());
                add(&[u8::from(value.is_some())]);
                add(value.as_deref().unwrap_or_default().as_bytes());
            }
        }
        hex::encode(digest.finalize())
    }
}
#[derive(Debug)]
pub enum ManagedRuntimeError {
    BootstrapRequired,
    InvocationMigrationRequired,
}
impl std::fmt::Display for ManagedRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BootstrapRequired => f.write_str("Podman authority requires explicit control-marker bootstrap and a private persisted binding"),
            Self::InvocationMigrationRequired => f.write_str("runtime invocation migration required: explicitly adopt an approved runtime after read-only authority verification and persist its audit/checkpoint before effects"),
        }
    }
}
impl std::error::Error for ManagedRuntimeError {}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerHandle {
    authority_fingerprint: String,
    session: Uuid,
    operation: Uuid,
    id: String,
    name: String,
    /// Private digest selection for this immutable instance. Absence means v1;
    /// legacy handles remain byte-for-byte serializable without a new field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    configuration_fingerprint_version: Option<u32>,
}
impl ContainerHandle {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn session_id(&self) -> Uuid {
        self.session
    }
    pub fn operation_id(&self) -> Uuid {
        self.operation
    }
    pub fn authority_fingerprint(&self) -> &str {
        &self.authority_fingerprint
    }
    pub(crate) fn fingerprint_version(&self) -> u32 {
        self.configuration_fingerprint_version.unwrap_or(1)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OwnedContainerSnapshot {
    pub handle: ContainerHandle,
    pub observation: ContainerObservation,
}

/// Fresh Created evidence for a named ImportVerification helper. It is
/// produced only after strict runtime inspection and consumed as one value by
/// the Writer acknowledgement; it grants no start or cleanup authority.
pub(crate) struct NamedImportVerificationCreated {
    parent_operation: Uuid,
    create_facts_hash: String,
    authority: RuntimeAuthority,
    owned: OwnedContainerSnapshot,
}

impl NamedImportVerificationCreated {
    #[cfg(test)]
    pub(crate) fn snapshot_for_test(&self) -> &OwnedContainerSnapshot {
        &self.owned
    }

    pub(crate) fn into_writer_parts(
        self,
    ) -> (Uuid, String, RuntimeAuthority, OwnedContainerSnapshot) {
        (
            self.parent_operation,
            self.create_facts_hash,
            self.authority,
            self.owned,
        )
    }
}
/// Only the first durable control claim may produce EmitOnce. A replay must
/// return ObserveOnly; this directive is consumed and the callback is FnOnce.
#[derive(Debug)]
pub enum StartDirective {
    EmitOnce,
    /// The existing first-start grant plus a consumed control-owner veto.
    EmitOnceWithVeto(LegacyStartVeto),
    ObserveOnly,
}
#[derive(Debug)]
pub struct ManagedRetainedExecution {
    execution: RetainedExecution,
    created: OwnedContainerSnapshot,
    terminal: ContainerObservation,
}
impl ManagedRetainedExecution {
    pub fn result(&self) -> &ExecutionResult {
        self.execution.result()
    }
    pub fn created(&self) -> &OwnedContainerSnapshot {
        &self.created
    }
    pub fn terminal(&self) -> &ContainerObservation {
        &self.terminal
    }
    pub fn handle(&self) -> &ContainerHandle {
        &self.created.handle
    }
}

/// Retained daemon transport facts and independently inspectable ownership.
/// Dropping this value neither removes the instance nor disarms its enforcer.
pub struct ManagedPrearmedExecution {
    result: PrearmedExecutionResult,
    created: OwnedContainerSnapshot,
    terminal: ContainerObservation,
}
impl std::fmt::Debug for ManagedPrearmedExecution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ManagedPrearmedExecution([private retained execution])")
    }
}
impl ManagedPrearmedExecution {
    pub fn result(&self) -> &PrearmedExecutionResult {
        &self.result
    }
    pub fn created(&self) -> &OwnedContainerSnapshot {
        &self.created
    }
    pub fn terminal(&self) -> &ContainerObservation {
        &self.terminal
    }
    pub fn handle(&self) -> &ContainerHandle {
        &self.created.handle
    }
}
impl ManagedPrearmedExecution {
    fn from_retained(retained: ManagedRetainedExecution) -> Self {
        let result = retained.execution.result;
        Self {
            result: PrearmedExecutionResult {
                stdout: result.stdout,
                stderr: result.stderr,
                transport_exit_code: result.exit_code,
                duration_ms: result.duration_ms,
            },
            created: retained.created,
            terminal: retained.terminal,
        }
    }
}

/// Abandonment retains the exact Created snapshot and Final reference without
/// manufacturing a transport capture, terminal observation or usage result.
// Keep both public outcomes owned by value, including the established executed
// result. This transport does not add a heap allocation to the existing path.
#[allow(clippy::large_enum_variant)]
pub enum ManagedPrearmedOutcome {
    Executed(ManagedPrearmedExecution),
    Abandoned {
        owned: OwnedContainerSnapshot,
        retirement: AbandonedBeforeClaim,
    },
}

impl std::fmt::Debug for ManagedPrearmedOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Executed(_) => {
                f.write_str("ManagedPrearmedOutcome::Executed([private retained execution])")
            }
            Self::Abandoned { .. } => {
                f.write_str("ManagedPrearmedOutcome::Abandoned([private retained instance])")
            }
        }
    }
}

// The private dispatcher preserves the same ownership until its public port
// chooses the legacy or prearmed result; it adds no transport allocation.
#[allow(clippy::large_enum_variant)]
enum OwnedExecutionOutcome {
    Executed(ManagedRetainedExecution),
    Abandoned {
        owned: OwnedContainerSnapshot,
        retirement: AbandonedBeforeClaim,
    },
}

impl OwnedExecutionOutcome {
    fn into_legacy(self) -> Result<ManagedRetainedExecution> {
        match self {
            Self::Executed(retained) => Ok(retained),
            Self::Abandoned { .. } => {
                bail!("unused-slot retirement requires the prearmed transport")
            }
        }
    }

    fn into_prearmed(self) -> ManagedPrearmedOutcome {
        match self {
            Self::Executed(retained) => {
                ManagedPrearmedOutcome::Executed(ManagedPrearmedExecution::from_retained(retained))
            }
            Self::Abandoned { owned, retirement } => {
                ManagedPrearmedOutcome::Abandoned { owned, retirement }
            }
        }
    }
}

/// Retains an actual legacy transport result across a later native check or
/// fallback failure. These fields grant no terminal/accounting authority.
struct RetainedLegacyExecutionFailure {
    cause: anyhow::Error,
    _runtime: ManagedRuntime,
    owned: Option<OwnedContainerSnapshot>,
    transport: Option<ExecutionResult>,
    fallback_stop: Option<ContainerObservation>,
    fallback_error: Option<anyhow::Error>,
    terminal_validation_error: Option<anyhow::Error>,
    completed: Option<ManagedRetainedExecution>,
}
impl std::fmt::Debug for RetainedLegacyExecutionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetainedLegacyExecutionFailure")
            .field("owned", &self.owned.is_some())
            .field("transport", &self.transport.is_some())
            .field("fallback_stop", &self.fallback_stop.is_some())
            .field("fallback_error", &self.fallback_error.is_some())
            .field(
                "terminal_validation_error",
                &self.terminal_validation_error.is_some(),
            )
            .field("completed", &self.completed.is_some())
            .finish_non_exhaustive()
    }
}
impl std::fmt::Display for RetainedLegacyExecutionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "legacy owned execution retained: {}", self.cause)
    }
}
impl std::error::Error for RetainedLegacyExecutionFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

enum OwnedStartDirective {
    ObserveOnly,
    Legacy(Option<LegacyStartVeto>),
    Prearmed(PrearmedStartControl),
    AbandonedBeforeClaim(AbandonedBeforeClaim),
}
impl From<StartDirective> for OwnedStartDirective {
    fn from(value: StartDirective) -> Self {
        match value {
            StartDirective::ObserveOnly => Self::ObserveOnly,
            StartDirective::EmitOnce => Self::Legacy(None),
            StartDirective::EmitOnceWithVeto(veto) => Self::Legacy(Some(veto)),
        }
    }
}
impl From<PrearmedStartDirective> for OwnedStartDirective {
    fn from(value: PrearmedStartDirective) -> Self {
        match value {
            PrearmedStartDirective::ObserveOnly => Self::ObserveOnly,
            PrearmedStartDirective::EmitOnce(control) => Self::Prearmed(control),
            PrearmedStartDirective::AbandonedBeforeClaim(retirement) => {
                Self::AbandonedBeforeClaim(retirement)
            }
        }
    }
}
#[derive(Clone, Debug)]
pub struct ManagedRuntime {
    runtime: Runtime,
    binding: RuntimeAuthority,
    launch_profile: Option<RetainedDockerLaunchProfile>,
}
impl ManagedRuntime {
    pub async fn connect_docker(authority_id: Uuid, endpoint: &str) -> Result<Self> {
        Self::connect_docker_with_runtime(
            authority_id,
            endpoint,
            Runtime::new(RuntimeKind::Docker)?,
        )
        .await
    }
    pub async fn connect_docker_with_runtime(
        authority_id: Uuid,
        endpoint: &str,
        runtime: Runtime,
    ) -> Result<Self> {
        validate_endpoint(RuntimeKind::Docker, endpoint)?;
        ensure!(
            runtime.kind == RuntimeKind::Docker,
            "runtime kind conflicts with Docker authority"
        );
        ensure!(
            !authority_id.is_nil(),
            "runtime authority identity must not be nil"
        );
        let mut managed = Self {
            binding: RuntimeAuthority {
                authority_id,
                kind: RuntimeKind::Docker,
                endpoint: endpoint.into(),
                engine_id: None,
                invocation: Some(runtime.invocation().clone()),
                docker_launch_profile: None,
                podman: None,
            },
            runtime,
            launch_profile: None,
        };
        managed.binding.engine_id = Some(managed.docker_engine_id().await?);
        Ok(managed)
    }
    /// No marker is created by connecting. Call bootstrap_podman explicitly
    /// after durable control authorization, then persist binding privately.
    pub async fn connect_podman(_authority_id: Uuid, endpoint: &str) -> Result<Self> {
        validate_endpoint(RuntimeKind::Podman, endpoint)?;
        Err(ManagedRuntimeError::BootstrapRequired.into())
    }
    pub async fn bootstrap_podman(authority_id: Uuid, endpoint: &str) -> Result<Self> {
        Self::bootstrap_podman_with_runtime(
            authority_id,
            endpoint,
            Runtime::new(RuntimeKind::Podman)?,
        )
        .await
    }
    pub async fn bootstrap_podman_with_runtime(
        authority_id: Uuid,
        endpoint: &str,
        runtime: Runtime,
    ) -> Result<Self> {
        Self::bootstrap_podman_for_launch(authority_id, endpoint, runtime, false).await
    }
    pub(crate) async fn bootstrap_podman_for_launch(
        authority_id: Uuid,
        endpoint: &str,
        runtime: Runtime,
        allow_rootful: bool,
    ) -> Result<Self> {
        selected_podman_target_args(endpoint)?;
        ensure!(
            runtime.kind == RuntimeKind::Podman,
            "runtime kind conflicts with Podman authority"
        );
        ensure!(
            !authority_id.is_nil(),
            "runtime authority identity must not be nil"
        );
        let mut managed = Self {
            binding: RuntimeAuthority {
                authority_id,
                kind: RuntimeKind::Podman,
                endpoint: endpoint.into(),
                engine_id: None,
                invocation: Some(runtime.invocation().clone()),
                docker_launch_profile: None,
                podman: None,
            },
            runtime,
            launch_profile: None,
        };
        let info = managed.podman_info(Some(allow_rootful)).await?;
        let marker = PodmanBinding {
            graph_root: info.store.graph_root,
            run_root: info.store.run_root,
            marker_volume: format!(
                "aig-authority-{}-{}",
                authority_id.simple(),
                Uuid::new_v4().simple()
            ),
            nonce: Uuid::new_v4(),
            effective_uid: effective_uid(),
            selection_env: podman_selection_env(managed.runtime.invocation()),
        };
        let output = managed
            .cli_sensitive(
                &[
                    "volume".into(),
                    "create".into(),
                    "--label".into(),
                    format!("agentsandbox.authority={authority_id}"),
                    "--label".into(),
                    format!("agentsandbox.binding={}", marker.nonce),
                    marker.marker_volume.clone(),
                ],
                Duration::from_secs(20),
                &[marker.marker_volume.clone(), marker.nonce.to_string()],
                None,
            )
            .await?;
        ensure!(
            !output.truncated
                && std::str::from_utf8(&output.bytes).ok().map(str::trim)
                    == Some(marker.marker_volume.as_str()),
            "Podman marker creation response is ambiguous; no marker adopted or removed"
        );
        managed.binding.podman = Some(marker);
        managed.verify_authority().await?;
        Ok(managed)
    }
    pub async fn reopen(binding: RuntimeAuthority) -> Result<Self> {
        let invocation = binding
            .invocation
            .clone()
            .ok_or(ManagedRuntimeError::InvocationMigrationRequired)?;
        let runtime = Runtime::from_invocation(binding.kind, invocation)?;
        Self::reopen_with_runtime(binding, runtime).await
    }
    /// Native control outcome for an already charged STOP cell. This is not
    /// an adoption/start capability and never derives collection from Display.
    pub(crate) async fn reopen_native(binding: RuntimeAuthority) -> NativeRuntimeOutcome<Self> {
        let runtime = match binding
            .invocation
            .clone()
            .ok_or(ManagedRuntimeError::InvocationMigrationRequired)
            .map_err(anyhow::Error::from)
            .and_then(|invocation| Runtime::from_invocation(binding.kind, invocation))
        {
            Ok(runtime) => runtime,
            Err(error) => return super::control_outcome::no_child(Err(error)),
        };
        let scope = super::control_outcome::NativeControlScope::new(runtime);
        let result = Self::validated_reopen(binding, scope.runtime()).await;
        scope.finish(result)
    }
    pub async fn reopen_with_runtime(binding: RuntimeAuthority, runtime: Runtime) -> Result<Self> {
        let invocation = binding
            .invocation
            .as_ref()
            .ok_or(ManagedRuntimeError::InvocationMigrationRequired)?;
        ensure!(
            runtime.kind == binding.kind && invocation == runtime.invocation(),
            "approved runtime invocation differs from the persisted authority"
        );
        Self::validated_reopen(binding, runtime).await
    }
    /// Verify the same engine and Podman marker using only info/inspect. The
    /// caller must audit and persist the resulting binding before any effect.
    pub async fn adopt_invocation(mut binding: RuntimeAuthority, runtime: Runtime) -> Result<Self> {
        ensure!(
            runtime.kind == binding.kind,
            "runtime kind conflicts with historical authority"
        );
        ensure!(
            binding.docker_launch_profile.is_none()
                || binding.invocation.as_ref() == Some(runtime.invocation()),
            "strict Docker start profile invocation cannot be adopted; a new acceptance is required"
        );
        binding.invocation = Some(runtime.invocation().clone());
        Self::validated_reopen(binding, runtime).await
    }
    async fn validated_reopen(binding: RuntimeAuthority, runtime: Runtime) -> Result<Self> {
        binding.validate_docker_launch_profile()?;
        validate_endpoint(binding.kind, &binding.endpoint)?;
        ensure!(
            !binding.authority_id.is_nil(),
            "runtime authority identity must not be nil"
        );
        match binding.kind {
            RuntimeKind::Docker => ensure!(
                binding.engine_id.is_some() && binding.podman.is_none(),
                "Docker binding is incomplete"
            ),
            RuntimeKind::Podman => ensure!(
                binding.engine_id.is_none() && binding.podman.is_some(),
                "Podman binding is incomplete; explicit bootstrap required"
            ),
        }
        let managed = Self {
            runtime,
            binding,
            launch_profile: None,
        };
        // Legacy Podman environment remains part of the engine fingerprint. It
        // is never replayed; adoption verifies storage roots and the private
        // marker under the newly approved environment instead.
        managed.verify_authority().await?;
        Ok(managed)
    }
    pub fn binding(&self) -> &RuntimeAuthority {
        &self.binding
    }
    pub(crate) fn bind_docker_launch_profile(
        mut self,
        profile: RetainedDockerLaunchProfile,
    ) -> Result<Self> {
        ensure!(
            self.binding.docker_launch_profile.is_none(),
            "Docker start profile already bound"
        );
        self.binding.docker_launch_profile = Some(profile.binding().clone());
        self.binding.validate_docker_launch_profile()?;
        profile.revalidate()?;
        self.launch_profile = Some(profile);
        Ok(self)
    }
    pub(crate) fn retain_docker_launch_profile(
        mut self,
        profile: RetainedDockerLaunchProfile,
    ) -> Result<Self> {
        ensure!(
            self.binding.docker_launch_profile.as_ref() == Some(profile.binding()),
            "Docker start profile binding changed"
        );
        self.binding.validate_docker_launch_profile()?;
        profile.revalidate()?;
        self.launch_profile = Some(profile);
        Ok(self)
    }
    pub(super) fn prearmed_command(
        &self,
    ) -> Result<(
        Command,
        Option<DockerLaunchProfileBindingV1>,
        Option<RetainedDockerLaunchProfile>,
    )> {
        let mut command = self.command()?;
        let retained = match &self.binding.docker_launch_profile {
            Some(binding) => {
                let profile = self
                    .launch_profile
                    .as_ref()
                    .context("Docker start profile capability is unavailable")?;
                ensure!(
                    profile.binding() == binding,
                    "Docker start profile capability differs"
                );
                profile.revalidate()?;
                command.arg("--config").arg(profile.directory_path());
                Some(profile.clone())
            }
            None => {
                ensure!(
                    self.launch_profile.is_none(),
                    "unexpected Docker start profile capability"
                );
                None
            }
        };
        Ok((
            command,
            self.binding.docker_launch_profile.clone(),
            retained,
        ))
    }
    pub async fn doctor(&self, allow_rootful: bool) -> Result<RuntimeInfo> {
        self.verify_authority().await?;
        let format = if self.binding.kind == RuntimeKind::Docker {
            "{{json .}}"
        } else {
            "json"
        };
        let output = self
            .cli(
                &["info".into(), "--format".into(), format.into()],
                Duration::from_secs(15),
            )
            .await?;
        ensure!(!output.truncated, "runtime info exceeds its response bound");
        if self.binding.kind == RuntimeKind::Docker {
            #[derive(Deserialize)]
            struct Engine {
                #[serde(rename = "ID")]
                id: String,
            }
            let info: Engine = serde_json::from_slice(&output.bytes)
                .map_err(|_| anyhow::anyhow!("Docker authority inspection is invalid"))?;
            ensure!(
                self.binding.engine_id.as_deref() == Some(info.id.as_str()),
                "Docker authority changed during diagnostics"
            );
        }
        parse_runtime_info(self.binding.kind, &output.bytes, allow_rootful)
    }
    /// Diagnostic operation identity only, never a start/cleanup permission.
    pub async fn resolve_image_observed(&self, operation: Uuid, image_ref: &str) -> Result<String> {
        let mut scoped = self.clone();
        scoped.runtime = self.runtime.observed_operation(operation);
        super::observation::record(
            &scoped.runtime.observation,
            super::observation::Phase::ImageResolutionRequested,
        );
        let result = scoped.resolve_image(image_ref).await;
        super::observation::record(
            &scoped.runtime.observation,
            if result.is_ok() {
                super::observation::Phase::ImageResolutionReturned
            } else {
                super::observation::Phase::ImageResolutionFailed
            },
        );
        result
    }
    pub async fn resolve_image(&self, image_ref: &str) -> Result<String> {
        validate_image(image_ref)?;
        self.verify_authority().await?;
        let output = self
            .cli(
                &[
                    "image".into(),
                    "inspect".into(),
                    "--format".into(),
                    "{{.Id}}".into(),
                    image_ref.into(),
                ],
                Duration::from_secs(15),
            )
            .await
            .context("could not verify the selected image in the local runtime cache; prepare it explicitly if absent, automatic image pulls are disabled")?;
        let id = std::str::from_utf8(&output.bytes)
            .ok()
            .map(str::trim)
            .filter(|_| !output.truncated)
            .context("runtime image inspection is invalid")?;
        let raw = id.strip_prefix("sha256:").unwrap_or(id);
        ensure!(
            valid_immutable_id(raw),
            "runtime did not expose an immutable image ID"
        );
        Ok(format!("sha256:{raw}"))
    }
    pub async fn create_named_volume(
        &self,
        volume: &str,
        labels: &[(String, String)],
    ) -> Result<()> {
        self.create_named_volume_inner(volume, labels, None).await
    }

    pub(crate) async fn create_named_volume_for_generation(
        &self,
        volume: &str,
        labels: &[(String, String)],
        cancel: &CancelGeneration,
        expected: u64,
    ) -> Result<()> {
        self.create_named_volume_inner(
            volume,
            labels,
            Some(GenerationSpawnGuard::new(cancel, expected)),
        )
        .await
    }

    async fn create_named_volume_inner(
        &self,
        volume: &str,
        labels: &[(String, String)],
        spawn_guard: Option<GenerationSpawnGuard>,
    ) -> Result<()> {
        ensure!(
            !labels.is_empty(),
            "managed workspace volume requires control ownership labels"
        );
        self.verify_authority().await?;
        let output = self
            .cli_sensitive(
                &named_volume_args(volume, labels)?,
                Duration::from_secs(30),
                &[],
                spawn_guard,
            )
            .await?;
        ensure!(
            !output.truncated
                && std::str::from_utf8(&output.bytes).ok().map(str::trim) == Some(volume),
            "workspace volume creation response is ambiguous; no volume adopted or removed"
        );
        self.verify_authority().await?;
        let output = self
            .cli(
                &[
                    "volume".into(),
                    "inspect".into(),
                    "--format".into(),
                    "{{json .}}".into(),
                    volume.into(),
                ],
                Duration::from_secs(10),
            )
            .await?;
        ensure!(
            !output.truncated,
            "workspace volume inspection exceeds its bound"
        );
        let facts: WorkspaceVolume = serde_json::from_slice(&output.bytes).map_err(|_| {
            anyhow::anyhow!("workspace volume ownership inspection is invalid; no volume adopted")
        })?;
        ensure!(
            facts.name == volume
                && facts.driver == "local"
                && facts.scope == "local"
                && facts
                    .options
                    .as_ref()
                    .is_none_or(std::collections::BTreeMap::is_empty)
                && labels
                    .iter()
                    .all(|(key, value)| facts.labels.get(key) == Some(value)),
            "workspace volume ownership, driver or host mount options conflict; no volume adopted or removed"
        );
        Ok(())
    }
    pub(super) fn command(&self) -> Result<Command> {
        ensure!(
            self.binding.invocation.as_ref() == Some(self.runtime.invocation()),
            "runtime invocation differs from the persisted authority"
        );
        let mut command = self.runtime.command()?;
        // A frozen explicit target wins over mutable CLI context selections.
        for name in [
            "DOCKER_HOST",
            "DOCKER_CONTEXT",
            "DOCKER_TLS_VERIFY",
            "DOCKER_CERT_PATH",
            "DOCKER_API_VERSION",
            "CONTAINER_HOST",
            "CONTAINER_CONNECTION",
        ] {
            command.env_remove(name);
        }
        match self.binding.kind {
            RuntimeKind::Docker => {
                command.arg("--host").arg(&self.binding.endpoint);
            }
            RuntimeKind::Podman => {
                command.args(podman_target_args(&self.binding.endpoint)?);
            }
        }
        Ok(command)
    }
    pub(super) async fn command_async(&self) -> Result<Command> {
        let runtime = self.clone();
        super::deadline_supervision::checked_blocking(move || runtime.command()).await
    }
    async fn cli(&self, args: &[String], duration: Duration) -> Result<Capture> {
        self.cli_sensitive(args, duration, &[], None).await
    }
    async fn cli_sensitive(
        &self,
        args: &[String],
        duration: Duration,
        extra: &[String],
        spawn_guard: Option<GenerationSpawnGuard>,
    ) -> Result<Capture> {
        let mut private_redactions = extra.to_vec();
        if let Some(binding) = &self.binding.podman {
            private_redactions.extend([binding.marker_volume.clone(), binding.nonce.to_string()]);
        }
        self.runtime
            .capture_cli_with_generation(
                self.command_async().await?,
                args,
                duration,
                &[],
                spawn_guard,
            )
            .await
            .map_err(|mut error| {
                // Keep original bytes and capture metadata until the caller can
                // mask its credentials and these private markers together.
                if let Some(failure) = error.downcast_mut::<RuntimeCommandFailure>() {
                    failure.add_private_redactions(private_redactions);
                }
                error.context(
                    "runtime control command failed or unavailable; observation is unknown",
                )
            })
    }
    async fn docker_engine_id(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct Engine {
            #[serde(rename = "ID")]
            id: String,
        }
        let output = self
            .cli(
                &["info".into(), "--format".into(), "{{json .}}".into()],
                Duration::from_secs(15),
            )
            .await?;
        ensure!(
            !output.truncated,
            "Docker authority inspection exceeds its bound"
        );
        let info: Engine = serde_json::from_slice(&output.bytes)
            .map_err(|_| anyhow::anyhow!("Docker authority inspection is invalid"))?;
        ensure!(
            !info.id.is_empty()
                && info.id.len() <= 256
                && info
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b)),
            "Docker did not expose a valid engine ID"
        );
        Ok(info.id)
    }
    async fn podman_info(&self, launch_allow_rootful: Option<bool>) -> Result<PodmanInfo> {
        let output = self
            .cli(
                &["info".into(), "--format".into(), "json".into()],
                Duration::from_secs(15),
            )
            .await?;
        ensure!(
            !output.truncated,
            "Podman authority inspection exceeds its bound"
        );
        // Use the same info response to check launch policy and bind storage.
        if let Some(allow_rootful) = launch_allow_rootful {
            parse_runtime_info(RuntimeKind::Podman, &output.bytes, allow_rootful)?;
        }
        let info: PodmanInfo = serde_json::from_slice(&output.bytes)
            .map_err(|_| anyhow::anyhow!("Podman authority inspection is invalid"))?;
        ensure!(
            Path::new(&info.store.graph_root).is_absolute()
                && Path::new(&info.store.run_root).is_absolute(),
            "Podman storage authority paths must be absolute"
        );
        Ok(info)
    }
    pub(super) async fn verify_authority(&self) -> Result<()> {
        match self.binding.kind {
            RuntimeKind::Docker => ensure!(
                self.binding.engine_id.as_deref() == Some(self.docker_engine_id().await?.as_str()),
                "Docker engine authority changed; runtime state is unknown"
            ),
            RuntimeKind::Podman => {
                let binding = self
                    .binding
                    .podman
                    .as_ref()
                    .ok_or(ManagedRuntimeError::BootstrapRequired)?;
                ensure!(
                    binding.effective_uid == effective_uid(),
                    "Podman control account changed; runtime state is unknown"
                );
                let info = self.podman_info(None).await?;
                ensure!(
                    info.store.graph_root == binding.graph_root
                        && info.store.run_root == binding.run_root,
                    "Podman storage authority changed; runtime state is unknown"
                );
                #[derive(Deserialize)]
                #[serde(rename_all = "PascalCase")]
                struct Marker {
                    name: String,
                    labels: std::collections::BTreeMap<String, String>,
                }
                let output = self
                    .cli(
                        &[
                            "volume".into(),
                            "inspect".into(),
                            "--format".into(),
                            "{{json .}}".into(),
                            binding.marker_volume.clone(),
                        ],
                        Duration::from_secs(10),
                    )
                    .await?;
                ensure!(
                    !output.truncated,
                    "Podman control marker inspection exceeds its bound"
                );
                let marker: Marker = serde_json::from_slice(&output.bytes).map_err(|_| {
                    anyhow::anyhow!(
                        "Podman control marker is missing or invalid; runtime state is unknown"
                    )
                })?;
                ensure!(
                    marker.name == binding.marker_volume
                        && marker.labels.get("agentsandbox.authority")
                            == Some(&self.binding.authority_id.to_string())
                        && marker.labels.get("agentsandbox.binding")
                            == Some(&binding.nonce.to_string()),
                    "Podman control marker changed; runtime state is unknown"
                );
            }
        }
        Ok(())
    }
    fn validate_handle(&self, handle: &ContainerHandle) -> Result<()> {
        ensure!(
            handle.authority_fingerprint == self.binding.fingerprint()
                && valid_immutable_id(&handle.id)
                && !handle.operation.is_nil()
                && matches!(handle.fingerprint_version(), 1..=3),
            "owned container handle does not match the runtime authority"
        );
        Ok(())
    }
    async fn observe_id(&self, id: &str, fingerprint_version: u32) -> Result<ContainerObservation> {
        let output = self.inspect_facts(id).await?;
        let observation = parse_container_observation_version(
            &output.bytes,
            self.binding.kind,
            fingerprint_version,
        )?;
        ensure!(
            observation.id.as_deref() == Some(id),
            "runtime inspected a different immutable instance"
        );
        Ok(observation)
    }
    async fn inspect_facts(&self, id: &str) -> Result<Capture> {
        ensure!(
            valid_immutable_id(id),
            "runtime target must be a complete immutable container ID"
        );
        self.verify_authority().await?;
        let output = self
            .cli(
                &[
                    "inspect".into(),
                    "--format".into(),
                    "{{json .}}".into(),
                    id.into(),
                ],
                Duration::from_secs(10),
            )
            .await?;
        ensure!(!output.truncated, "runtime inspection exceeds its bound");
        Ok(output)
    }
    pub async fn observe(&self, handle: &ContainerHandle) -> Result<ContainerObservation> {
        self.validate_handle(handle)?;
        let observed = self
            .observe_id(&handle.id, handle.fingerprint_version())
            .await?;
        ensure!(
            observed.name == handle.name,
            "runtime owned instance name changed"
        );
        Ok(observed)
    }
    /// A missing result means only an empty, successful targeted inventory.
    /// It never authorizes another start or frees a reservation.
    pub async fn lookup_created(
        &self,
        spec: &ContainerSpec,
    ) -> Result<Option<OwnedContainerSnapshot>> {
        self.lookup_created_with_binding(spec, None, false).await
    }

    /// Recover only a helper proved to be Created and never started.
    #[cfg(unix)]
    #[allow(dead_code, reason = "the private named recovery caller is separate")]
    pub(crate) async fn lookup_never_started_import(
        &self,
        spec: &ContainerSpec,
    ) -> Result<Option<OwnedContainerSnapshot>> {
        ensure!(
            matches!(spec.purpose, ContainerPurpose::Import),
            "named import recovery requires an import helper"
        );
        self.lookup_created_with_binding(spec, None, true).await
    }

    /// Read-only recovery lookup for the exact held worker. A returned
    /// snapshot is Created evidence and carries no permission to start.
    #[cfg(unix)]
    #[allow(dead_code)] // Broker composition supplies the caller.
    pub(crate) async fn lookup_created_bootstrap(
        &self,
        spec: &ContainerSpec,
    ) -> Result<Option<OwnedContainerSnapshot>> {
        let held = self
            .checked_held_bootstrap_for(spec, true)?
            .context("held bootstrap worker binding is missing")?;
        self.lookup_created_with_binding(spec, Some(held), false)
            .await
    }

    async fn lookup_created_with_binding(
        &self,
        spec: &ContainerSpec,
        held: Option<&HeldBootstrapBinding>,
        require_never_started: bool,
    ) -> Result<Option<OwnedContainerSnapshot>> {
        ensure!(
            spec.managed_names.is_some(),
            "managed lookup requires control-assigned names"
        );
        self.verify_authority().await?;
        let output = self
            .cli(
                &[
                    "ps".into(),
                    "--all".into(),
                    "--no-trunc".into(),
                    "--filter".into(),
                    format!("name=^{}$", spec.name()),
                    "--format".into(),
                    "{{.ID}}".into(),
                ],
                Duration::from_secs(10),
            )
            .await?;
        ensure!(
            !output.truncated,
            "runtime targeted inventory exceeds its bound"
        );
        let text = std::str::from_utf8(&output.bytes)
            .map_err(|_| anyhow::anyhow!("runtime targeted inventory is invalid"))?;
        let ids: Vec<_> = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        ensure!(
            ids.len() <= 1 && ids.iter().all(|id| valid_immutable_id(id)),
            "runtime targeted inventory is ambiguous or invalid; no instance adopted"
        );
        let Some(id) = ids.first() else {
            return Ok(None);
        };
        let facts = if held.is_some() {
            self.inspect_bootstrap_facts(id).await?
        } else {
            self.inspect_facts(id).await?
        };
        let observed = parse_container_observation_version(
            &facts.bytes,
            self.binding.kind,
            spec.fingerprint_version(),
        )?;
        ensure!(
            observed.id.as_deref() == Some(*id) && observed.name == spec.name(),
            "runtime lookup identity conflicts with the reserved attempt"
        );
        if let Some(held) = held {
            validate_created(spec, &observed)?;
            ensure!(
                observed.configuration_fingerprint.is_some(),
                "retained bootstrap lacks its created confinement fingerprint"
            );
            validate_configuration_for(spec, &facts.bytes, self.binding.kind, Some(held))?;
        } else {
            validate_authorized_identity(spec, &observed)?;
            validate_configuration(spec, &facts.bytes, self.binding.kind)?;
        }
        if require_never_started {
            validate_created(spec, &observed)?;
            validate_unstarted_process_timestamps(&facts.bytes)?;
        }
        let handle = ContainerHandle {
            authority_fingerprint: self.binding.fingerprint(),
            session: spec.session,
            operation: spec.operation_id(),
            id: (*id).into(),
            name: spec.name(),
            configuration_fingerprint_version: (spec.fingerprint_version() != 1)
                .then_some(spec.fingerprint_version()),
        };
        Ok(Some(OwnedContainerSnapshot {
            handle,
            observation: observed,
        }))
    }
    async fn inspect_bootstrap_facts(&self, id: &str) -> Result<Capture> {
        self.inspect_facts(id)
            .await
            .map_err(|_| anyhow::anyhow!("held bootstrap runtime inspection failed"))
    }

    #[cfg(unix)]
    pub(super) async fn inspect_bootstrap_running(
        &self,
        created: &OwnedContainerSnapshot,
        binding: &HeldBootstrapBinding,
    ) -> Result<Option<OwnedContainerSnapshot>> {
        self.validate_handle(&created.handle)?;
        let facts = self.inspect_bootstrap_facts(created.handle.id()).await?;
        validate_bootstrap_stdin_flags(&facts.bytes, self.binding.kind)?;
        validate_bootstrap_environment(&facts.bytes, binding)?;
        let observation = parse_container_observation_version(
            &facts.bytes,
            self.binding.kind,
            created.handle.fingerprint_version(),
        )?;
        observation.validate_identity_allocation(&created.observation)?;
        if observation.process.running == Some(true)
            && observation.process.status.as_deref() == Some("running")
            && observation.process.started_at.is_some()
        {
            return Ok(Some(OwnedContainerSnapshot {
                handle: created.handle.clone(),
                observation,
            }));
        }
        ensure!(
            observation.process.started_at.is_none()
                && observation.process.running == Some(false)
                && matches!(
                    observation.process.status.as_deref(),
                    Some("created" | "configured" | "initialized")
                ),
            "runtime did not prove a live retained bootstrap incarnation"
        );
        Ok(None)
    }
    pub(super) async fn inspect_process(
        &self,
        id: &str,
        created: &ContainerObservation,
    ) -> Result<RuntimeState> {
        let observation = self.observe_id(id, created.fingerprint_version()).await?;
        observation.validate_identity_allocation(created)?;
        process_state(&observation)
    }
    pub(super) async fn kill_target(&self, id: &str) -> Result<()> {
        ensure!(
            valid_immutable_id(id),
            "runtime stop target must be an immutable ID"
        );
        self.verify_authority().await?;
        self.cli(&["kill".into(), id.into()], Duration::from_secs(15))
            .await?;
        Ok(())
    }
    /// Stop only the exact instance and sandbox configuration captured by the
    /// caller. An unavailable or divergent inspection never authorizes a kill.
    /// This proves a stopped process, not a terminal allocation or usage record.
    pub async fn stop_snapshot_owned(
        &self,
        owned: &OwnedContainerSnapshot,
    ) -> Result<ContainerObservation> {
        self.stop_snapshot_owned_result(owned)
            .await
            .map(|result| result.observation)
    }
    /// Run the same owned stop and report what the engine confirmed.
    pub(crate) async fn stop_snapshot_owned_observation(
        &self,
        owned: &OwnedContainerSnapshot,
    ) -> Result<OwnedStopObservation> {
        self.stop_snapshot_owned_result(owned)
            .await
            .map(|result| result.stop_observation)
    }
    pub(crate) async fn stop_snapshot_owned_native(
        &self,
        owned: &OwnedContainerSnapshot,
    ) -> NativeRuntimeOutcome<NativeStopResult> {
        let scope = super::control_outcome::NativeControlScope::new(self.runtime.clone());
        let mut scoped = self.clone();
        scoped.runtime = scope.runtime();
        let result = scoped.stop_snapshot_owned_result(owned).await;
        scope.finish(result)
    }
    async fn stop_snapshot_owned_result(
        &self,
        owned: &OwnedContainerSnapshot,
    ) -> Result<NativeStopResult> {
        self.validate_handle(&owned.handle)?;
        ensure!(
            owned.observation.id.as_deref() == Some(owned.handle.id.as_str())
                && owned.observation.name == owned.handle.name
                && owned.observation.fingerprint_version() == owned.handle.fingerprint_version(),
            "owned stop snapshot does not match its handle"
        );
        self.stop_snapshot_target_result(&owned.handle.id, &owned.observation)
            .await
    }
    pub(super) async fn stop_snapshot_target(
        &self,
        id: &str,
        baseline: &ContainerObservation,
    ) -> Result<ContainerObservation> {
        self.stop_snapshot_target_result(id, baseline)
            .await
            .map(|result| result.observation)
    }
    async fn stop_snapshot_target_result(
        &self,
        id: &str,
        baseline: &ContainerObservation,
    ) -> Result<NativeStopResult> {
        ensure!(
            valid_immutable_id(id)
                && baseline.id.as_deref() == Some(id)
                && matches!(baseline.fingerprint_version(), 1..=3)
                && baseline
                    .configuration_fingerprint
                    .as_deref()
                    .is_some_and(valid_immutable_id)
                && baseline.image_id.as_deref().is_some_and(|image| {
                    valid_immutable_id(image.strip_prefix("sha256:").unwrap_or(image))
                }),
            "owned stop snapshot lacks complete immutable configuration proof"
        );
        baseline.validate_identity_allocation(baseline)?;
        let observed = self.observe_id(id, baseline.fingerprint_version()).await?;
        observed.validate_identity_allocation(baseline)?;
        if stopped_process(&observed) {
            return Ok(NativeStopResult {
                observation: observed,
                stop_observation: OwnedStopObservation::AlreadyStopped,
                kill_failure: None,
            });
        }
        ensure!(
            observed.process.running == Some(true)
                && matches!(
                    observed.process.status.as_deref(),
                    Some("running" | "paused" | "restarting")
                ),
            "runtime stop process state is unknown or inconsistent"
        );
        // kill_target revalidates engine authority immediately before the effect.
        // A failed command can race a natural exit; only the next exact inspection
        // can establish the result. Inspect and kill are not an atomic engine API.
        let kill_failure = self.kill_target(id).await.err();
        if self.runtime.native_control.as_ref().is_some_and(|scope| {
            matches!(
                scope.phase(),
                NativeIoPhase::Pending | NativeIoPhase::Unknown
            )
        }) {
            // Do not emit a follow-up CLI over a still-owned/unknown native
            // command. Its exact original error and scope remain together.
            return Err(kill_failure
                .unwrap_or_else(|| anyhow::anyhow!("owned kill has no native collection proof")));
        }
        let followed = async {
            let observed = self.observe_id(id, baseline.fingerprint_version()).await?;
            observed.validate_identity_allocation(baseline)?;
            ensure!(
                stopped_process(&observed),
                "runtime did not prove the exact owned snapshot stopped"
            );
            Ok(observed)
        }
        .await;
        match followed {
            Ok(observation) => Ok(NativeStopResult {
                observation,
                stop_observation: match &kill_failure {
                    None => OwnedStopObservation::EngineAcknowledged,
                    Some(_) => OwnedStopObservation::Unconfirmed,
                },
                kill_failure,
            }),
            Err(followup_failure) => Err(anyhow::Error::new(NativeStopFailure {
                kill_failure,
                followup_failure,
            })),
        }
    }
    /// Legacy handle-only stop for callers without a captured sandbox baseline.
    /// Managed supervision must use stop_snapshot_owned instead.
    pub async fn stop_owned(&self, handle: &ContainerHandle) -> Result<ContainerObservation> {
        self.validate_handle(handle)?;
        if let Ok(observed) = self.observe(handle).await
            && observed.process.running == Some(false)
            && matches!(
                observed.process.status.as_deref(),
                Some("created" | "configured" | "initialized" | "exited" | "stopped" | "dead")
            )
        {
            return Ok(observed);
        }
        let _ = self.kill_target(&handle.id).await;
        let observed = self.observe(handle).await?;
        ensure!(
            observed.process.running == Some(false),
            "runtime did not prove the owned instance stopped"
        );
        Ok(observed)
    }
    /// A callback failure retains the created instance. ObserveOnly never emits
    /// start, even if the current process appears never to have started.
    pub async fn execute_retained<C, CFut, F, Fut>(
        &self,
        spec: &ContainerSpec,
        input: Option<std::fs::File>,
        duration: Duration,
        stdout_limit: usize,
        on_created: C,
        on_started: F,
    ) -> Result<ManagedRetainedExecution>
    where
        C: FnOnce(OwnedContainerSnapshot) -> CFut,
        CFut: Future<Output = Result<StartDirective>>,
        F: FnMut(ContainerObservation) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        ensure!(
            self.binding.docker_launch_profile.is_none(),
            "strict Docker start profile requires the prearmed start port"
        );
        spec.require_current_creation_policy()?;
        ensure!(
            !duration.is_zero() && duration <= Duration::from_secs(24 * 60 * 60),
            "runtime timeout must be between 1 second and 24 hours"
        );
        let mut scoped = self.clone();
        scoped.runtime = self.runtime.observed_operation(spec.operation_id());
        super::observation::record(
            &scoped.runtime.observation,
            super::observation::Phase::Submitted,
        );
        let result = scoped
            .execute_retained_observed(spec, input, duration, stdout_limit, on_created, on_started)
            .await;
        super::observation::record(
            &scoped.runtime.observation,
            if result.is_ok() {
                super::observation::Phase::OperationReturnedOk
            } else {
                super::observation::Phase::OperationReturnedError
            },
        );
        result
    }
    #[allow(clippy::too_many_arguments)]
    async fn execute_retained_observed<C, CFut, F, Fut>(
        &self,
        spec: &ContainerSpec,
        input: Option<std::fs::File>,
        duration: Duration,
        stdout_limit: usize,
        on_created: C,
        on_started: F,
    ) -> Result<ManagedRetainedExecution>
    where
        C: FnOnce(OwnedContainerSnapshot) -> CFut,
        CFut: Future<Output = Result<StartDirective>>,
        F: FnMut(ContainerObservation) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let handle = self.create_owned_handle(spec, false, None).await?;
        self.execute_owned(
            spec,
            handle,
            None,
            OwnedStartInput::Historical(input),
            Some(duration),
            stdout_limit,
            |created| async move { on_created(created).await.map(Into::into) },
            on_started,
        )
        .await
        .and_then(OwnedExecutionOutcome::into_legacy)
    }
    fn checked_held_bootstrap_for<'spec>(
        &self,
        spec: &'spec ContainerSpec,
        bootstrap_stdin: bool,
    ) -> Result<Option<&'spec HeldBootstrapBinding>> {
        let held = spec.held_bootstrap_for(bootstrap_stdin)?;
        ensure!(
            held.is_none_or(|binding| {
                binding.runtime == self.binding.kind
                    && binding.authority_id == self.binding.authority_id
            }),
            "held bootstrap authority differs from the authorized worker"
        );
        Ok(held)
    }

    async fn create_owned_handle(
        &self,
        spec: &ContainerSpec,
        bootstrap_stdin: bool,
        spawn_guard: Option<GenerationSpawnGuard>,
    ) -> Result<ContainerHandle> {
        spec.ensure_generic_create_start_allowed()?;
        self.create_owned_handle_inner(spec, bootstrap_stdin, spawn_guard)
            .await
    }

    async fn create_owned_handle_inner(
        &self,
        spec: &ContainerSpec,
        bootstrap_stdin: bool,
        spawn_guard: Option<GenerationSpawnGuard>,
    ) -> Result<ContainerHandle> {
        spec.require_current_creation_policy()?;
        ensure!(
            spec.managed_names.is_some(),
            "managed creation requires control-assigned names"
        );
        self.checked_held_bootstrap_for(spec, bootstrap_stdin)?;
        self.verify_authority().await?;
        super::observation::record(
            &self.runtime.observation,
            super::observation::Phase::CreateRequested,
        );
        let output = self
            .cli_sensitive(
                &spec.create_args_for(self.binding.kind, bootstrap_stdin)?,
                Duration::from_secs(30),
                &[],
                spawn_guard,
            )
            .await
            .context("container creation failed with automatic image pulls disabled (--pull=never); prepare the selected image explicitly if it is absent locally")?;
        let id = std::str::from_utf8(&output.bytes)
            .ok()
            .map(str::trim)
            .filter(|id| !output.truncated && valid_immutable_id(id))
            .context("runtime create response is ambiguous; ownership unknown, no adoption, stop or cleanup attempted")?
            .to_owned();
        super::observation::record(
            &self.runtime.trace_target(&id),
            super::observation::Phase::Created,
        );
        Ok(ContainerHandle {
            authority_fingerprint: self.binding.fingerprint(),
            session: spec.session,
            operation: spec.operation_id(),
            id: id.clone(),
            name: spec.name(),
            configuration_fingerprint_version: Some(spec.fingerprint_version()),
        })
    }
    /// Create and inspect the exact import helper under the live cancellation
    /// generation. The caller retains the durable preparation and owns any
    /// reconciliation when the create response or inspection is unavailable.
    pub(crate) async fn create_owned_import_for_generation(
        &self,
        spec: &ContainerSpec,
        cancel: &CancelGeneration,
        expected: u64,
    ) -> Result<OwnedContainerSnapshot> {
        ensure!(
            matches!(spec.purpose, ContainerPurpose::Import)
                && spec.network == NetworkMode::None
                && spec.secret_env_file.is_none()
                && spec.egress_proxy_port.is_none()
                && spec.held_bootstrap.is_none()
                && !spec.security_preset.is_interactive()
                && self.binding.docker_launch_profile.is_none(),
            "guarded import creation requires an ordinary confined helper"
        );
        ensure!(
            spec.image
                .strip_prefix("sha256:")
                .is_some_and(valid_immutable_id),
            "guarded import creation requires the admitted immutable image ID"
        );
        let mut scoped = self.clone();
        scoped.runtime = self.runtime.observed_operation(spec.operation_id());
        let handle = scoped
            .create_owned_handle(
                spec,
                false,
                Some(GenerationSpawnGuard::new(cancel, expected)),
            )
            .await?;
        let observation = scoped
            .inspect_created(spec, handle.id(), None, true)
            .await?;
        ensure!(
            observation.configuration_fingerprint.is_some(),
            "created import helper lacks a configuration fingerprint"
        );
        Ok(OwnedContainerSnapshot {
            handle,
            observation,
        })
    }

    /// Consume the Writer's one-shot authorization for a named, read-only
    /// ImportVerification Create. This boundary never starts the created child.
    pub(crate) async fn create_owned_named_import_verification_for_generation(
        &self,
        spec: &ContainerSpec,
        parent_operation: Uuid,
        policy: &crate::control::HelperExecutionPolicyV1,
        authorization: crate::broker::writer::AuthorizedLegacyNamedImportCreate,
        check_admission: impl FnOnce() -> Result<()> + Send + 'static,
    ) -> Result<NamedImportVerificationCreated> {
        authorization.validate_runtime_request(parent_operation, policy, spec, &self.binding)?;
        let (generation, expected_generation) = authorization.generation();
        generation
            .with_current_generation(expected_generation, || ())
            .context("named ImportVerification Create authorization was canceled before runtime inspection")?;
        ensure!(
            spec.security_preset == crate::control::SecurityPresetV1::FixedSandboxV2
                && spec
                    .image
                    .strip_prefix("sha256:")
                    .is_some_and(valid_immutable_id)
                && self.binding.docker_launch_profile.is_none(),
            "named ImportVerification requires its exact immutable confined helper spec"
        );
        let mut scoped = self.clone();
        scoped.runtime = self.runtime.observed_operation(spec.operation_id());
        scoped.verify_authority().await?;
        scoped
            .verify_named_import_verification_workspace(policy)
            .await?;
        scoped
            .verify_retained_import_helper(
                authorization.import_policy(),
                authorization.import_snapshot(),
            )
            .await?;
        let handle = scoped
            .create_owned_handle_inner(
                spec,
                false,
                Some(GenerationSpawnGuard {
                    generation: generation.clone(),
                    expected: expected_generation,
                    capture_admission: Some(Box::new(check_admission)),
                }),
            )
            .await?;
        let observation = scoped
            .inspect_created(spec, handle.id(), None, true)
            .await?;
        ensure!(
            observation.configuration_fingerprint.is_some(),
            "created named ImportVerification lacks a configuration fingerprint"
        );
        Ok(NamedImportVerificationCreated {
            parent_operation,
            create_facts_hash: authorization.facts_hash().to_owned(),
            authority: self.binding.clone(),
            owned: OwnedContainerSnapshot {
                handle,
                observation,
            },
        })
    }

    async fn verify_named_import_verification_workspace(
        &self,
        policy: &crate::control::HelperExecutionPolicyV1,
    ) -> Result<()> {
        let namespace = policy
            .stable_name
            .strip_prefix("aig-")
            .and_then(|rest| rest.split('-').next())
            .filter(|value| !value.is_empty())
            .context("named ImportVerification has no canonical workspace namespace")?;
        let expected = [
            ("agentsandbox.authority", policy.authority_id.as_str()),
            ("agentsandbox.worker", policy.parent_worker_id.as_str()),
            ("agentsandbox.namespace", namespace),
        ];
        let output = self
            .cli(
                &[
                    "volume".into(),
                    "inspect".into(),
                    "--format".into(),
                    "{{json .}}".into(),
                    policy.volume_name.clone(),
                ],
                Duration::from_secs(10),
            )
            .await?;
        ensure!(
            !output.truncated,
            "named ImportVerification workspace inspection exceeds its bound"
        );
        let volume: WorkspaceVolume = serde_json::from_slice(&output.bytes)
            .context("named ImportVerification workspace inspection is invalid")?;
        ensure!(
            volume.name == policy.volume_name
                && volume.driver == "local"
                && volume.scope == "local"
                && volume
                    .options
                    .as_ref()
                    .is_none_or(std::collections::BTreeMap::is_empty)
                && volume.labels.len() == expected.len()
                && expected
                    .iter()
                    .all(|(key, value)| volume.labels.get(*key).map(String::as_str) == Some(*value)),
            "named ImportVerification requires the exact existing control-owned workspace volume"
        );
        Ok(())
    }

    async fn verify_retained_import_helper(
        &self,
        policy: &crate::control::HelperExecutionPolicyV1,
        retained: &OwnedContainerSnapshot,
    ) -> Result<()> {
        ensure!(
            policy.purpose == crate::control::HelperPurposeV1::Import
                && policy.runtime_kind
                    == match self.binding.kind {
                        RuntimeKind::Docker => crate::control::RuntimeKindV1::Docker,
                        RuntimeKind::Podman => crate::control::RuntimeKindV1::Podman,
                    }
                && retained.handle.operation_id() == policy.helper_id.parse::<Uuid>()?
                && retained.handle.session_id() == policy.session_id.parse::<Uuid>()?
                && retained.handle.authority_fingerprint() == self.binding.fingerprint()
                && policy.authority_id == self.binding.authority_id.to_string(),
            "the exact named Import helper is not retained under this runtime authority"
        );
        let import_spec = crate::runner::broker::broker_helper_spec(policy)?;
        let facts = self.inspect_facts(retained.handle.id()).await?;
        let observation = parse_container_observation_version(
            &facts.bytes,
            self.binding.kind,
            retained.handle.fingerprint_version(),
        )?;
        ensure!(
            observation.id.as_deref() == Some(retained.handle.id()),
            "the retained named Import helper identity changed before verification Create"
        );
        validate_authorized_identity(&import_spec, &observation)?;
        observation.validate_terminal(self.binding.kind, &retained.observation)?;
        ensure!(
            observation.process.exit_code == Some(0) && observation.restart_count == Some(0),
            "the retained Import helper did not complete successfully without restart"
        );
        retained
            .observation
            .validate_identity_allocation(&observation)?;
        validate_configuration_for(&import_spec, &facts.bytes, self.binding.kind, None)?;
        Ok(())
    }

    async fn inspect_created(
        &self,
        spec: &ContainerSpec,
        id: &str,
        held_bootstrap: Option<&HeldBootstrapBinding>,
        require_never_started: bool,
    ) -> Result<ContainerObservation> {
        let facts = if held_bootstrap.is_some() {
            self.inspect_bootstrap_facts(id).await?
        } else {
            self.inspect_facts(id).await?
        };
        let observation = parse_container_observation_version(
            &facts.bytes,
            self.binding.kind,
            spec.fingerprint_version(),
        )?;
        ensure!(
            observation.id.as_deref() == Some(id),
            "runtime inspected a different immutable instance"
        );
        validate_created(spec, &observation)?;
        if require_never_started {
            validate_unstarted_process_timestamps(&facts.bytes)?;
        }
        validate_configuration_for(spec, &facts.bytes, self.binding.kind, held_bootstrap)?;
        Ok(observation)
    }

    /// Start an already owned, strictly recovered Created instance. The durable
    /// control callback must return a fresh permit; replay never creates or starts.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_owned_retained<C, CFut, F, Fut>(
        &self,
        spec: &ContainerSpec,
        owned: &OwnedContainerSnapshot,
        input: Option<std::fs::File>,
        duration: Duration,
        stdout_limit: usize,
        on_ready: C,
        on_started: F,
    ) -> Result<ManagedRetainedExecution>
    where
        C: FnOnce(OwnedContainerSnapshot) -> CFut,
        CFut: Future<Output = Result<StartDirective>>,
        F: FnMut(ContainerObservation) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        spec.require_current_creation_policy()?;
        self.execute_owned(
            spec,
            owned.handle.clone(),
            Some(&owned.observation),
            OwnedStartInput::Historical(input),
            Some(duration),
            stdout_limit,
            |created| async move { on_ready(created).await.map(Into::into) },
            on_started,
        )
        .await
        .and_then(OwnedExecutionOutcome::into_legacy)
    }
    /// Create a batch instance, then consume only a fresh, independently armed
    /// claim. This port never subscribes a session to process-wide signals.
    pub async fn execute_prearmed_retained<C, CFut, F, Fut>(
        &self,
        spec: &ContainerSpec,
        input: Option<std::fs::File>,
        stdout_limit: usize,
        on_created: C,
        on_started: F,
    ) -> Result<ManagedPrearmedOutcome>
    where
        C: FnOnce(OwnedContainerSnapshot) -> CFut,
        CFut: Future<Output = Result<PrearmedStartDirective>>,
        F: FnMut(ContainerObservation) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        self.execute_prearmed_with_input(
            spec,
            OwnedStartInput::Historical(input),
            stdout_limit,
            on_created,
            on_started,
        )
        .await
    }

    /// The trusted broker supplies the admitted waiting-bootstrap spec, the
    /// explicit owned handoff and a fresh prearmed claim. Caller verification
    /// of the bootstrap image/entrypoint and durable release stays with the
    /// broker.
    #[cfg(unix)]
    #[allow(dead_code)] // Broker composition is a separate lot.
    pub(crate) async fn execute_prearmed_bootstrap_retained<C, CFut>(
        &self,
        spec: &ContainerSpec,
        stdout_limit: usize,
        on_created: C,
        on_bootstrap_ready: bootstrap_stdin::BootstrapReady<'_>,
    ) -> Result<ManagedPrearmedOutcome>
    where
        C: FnOnce(OwnedContainerSnapshot) -> CFut,
        CFut: Future<Output = Result<PrearmedStartDirective>>,
    {
        self.execute_prearmed_bootstrap(spec, None, stdout_limit, on_created, on_bootstrap_ready)
            .await
    }

    /// Start the exact retained Created instance, with its original
    /// inspection as the baseline. The broker still needs a fresh EmitOnce
    /// from on_created before this port can attach.
    #[cfg(unix)]
    #[allow(dead_code)] // Broker composition supplies the caller.
    pub(crate) async fn start_owned_prearmed_bootstrap_retained<C, CFut>(
        &self,
        spec: &ContainerSpec,
        owned: &OwnedContainerSnapshot,
        stdout_limit: usize,
        on_created: C,
        on_bootstrap_ready: bootstrap_stdin::BootstrapReady<'_>,
    ) -> Result<ManagedPrearmedOutcome>
    where
        C: FnOnce(OwnedContainerSnapshot) -> CFut,
        CFut: Future<Output = Result<PrearmedStartDirective>>,
    {
        self.execute_prearmed_bootstrap(
            spec,
            Some(owned),
            stdout_limit,
            on_created,
            on_bootstrap_ready,
        )
        .await
    }

    #[cfg(unix)]
    async fn execute_prearmed_bootstrap<C, CFut>(
        &self,
        spec: &ContainerSpec,
        owned: Option<&OwnedContainerSnapshot>,
        stdout_limit: usize,
        on_created: C,
        on_bootstrap_ready: bootstrap_stdin::BootstrapReady<'_>,
    ) -> Result<ManagedPrearmedOutcome>
    where
        C: FnOnce(OwnedContainerSnapshot) -> CFut,
        CFut: Future<Output = Result<PrearmedStartDirective>>,
    {
        let binding = self
            .checked_held_bootstrap_for(spec, true)?
            .context("held bootstrap worker binding is missing")?
            .clone();
        let (handle, baseline) = match owned {
            Some(owned) => (owned.handle.clone(), Some(&owned.observation)),
            None => (self.create_owned_handle(spec, true, None).await?, None),
        };
        self.execute_owned(
            spec,
            handle,
            baseline,
            OwnedStartInput::Bootstrap {
                ready: on_bootstrap_ready,
                binding,
            },
            None,
            stdout_limit,
            |created| async move { on_created(created).await.map(Into::into) },
            |_| std::future::ready(Ok(())),
        )
        .await
        .map(OwnedExecutionOutcome::into_prearmed)
    }

    async fn execute_prearmed_with_input<C, CFut, F, Fut>(
        &self,
        spec: &ContainerSpec,
        input: OwnedStartInput<'_>,
        stdout_limit: usize,
        on_created: C,
        on_started: F,
    ) -> Result<ManagedPrearmedOutcome>
    where
        C: FnOnce(OwnedContainerSnapshot) -> CFut,
        CFut: Future<Output = Result<PrearmedStartDirective>>,
        F: FnMut(ContainerObservation) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        ensure!(
            !spec.security_preset.is_interactive(),
            "prearmed batch transport refuses interactive sessions"
        );
        let handle = self
            .create_owned_handle(spec, input.is_bootstrap(), None)
            .await?;
        self.execute_owned(
            spec,
            handle,
            None,
            input,
            None,
            stdout_limit,
            |created| async move { on_created(created).await.map(Into::into) },
            on_started,
        )
        .await
        .map(OwnedExecutionOutcome::into_prearmed)
    }
    /// Recovered Created instances require a newly returned EmitOnce control.
    /// Stored/replayed claims produce ObserveOnly and can never re-emit start.
    pub async fn start_owned_prearmed_retained<C, CFut, F, Fut>(
        &self,
        spec: &ContainerSpec,
        owned: &OwnedContainerSnapshot,
        input: Option<std::fs::File>,
        stdout_limit: usize,
        on_ready: C,
        on_started: F,
    ) -> Result<ManagedPrearmedOutcome>
    where
        C: FnOnce(OwnedContainerSnapshot) -> CFut,
        CFut: Future<Output = Result<PrearmedStartDirective>>,
        F: FnMut(ContainerObservation) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        spec.require_current_creation_policy()?;
        ensure!(
            !spec.security_preset.is_interactive(),
            "prearmed batch transport refuses interactive sessions"
        );
        self.execute_owned(
            spec,
            owned.handle.clone(),
            Some(&owned.observation),
            OwnedStartInput::Historical(input),
            None,
            stdout_limit,
            |created| async move { on_ready(created).await.map(Into::into) },
            on_started,
        )
        .await
        .map(OwnedExecutionOutcome::into_prearmed)
    }
    #[allow(clippy::too_many_arguments)]
    async fn execute_owned<C, CFut, F, Fut>(
        &self,
        spec: &ContainerSpec,
        handle: ContainerHandle,
        baseline: Option<&ContainerObservation>,
        input: OwnedStartInput<'_>,
        duration: Option<Duration>,
        stdout_limit: usize,
        on_created: C,
        on_started: F,
    ) -> Result<OwnedExecutionOutcome>
    where
        C: FnOnce(OwnedContainerSnapshot) -> CFut,
        CFut: Future<Output = Result<OwnedStartDirective>>,
        F: FnMut(ContainerObservation) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        spec.ensure_generic_create_start_allowed()?;
        ensure!(
            duration.is_none() || self.binding.docker_launch_profile.is_none(),
            "strict Docker start profile requires the prearmed start port"
        );
        ensure!(
            duration.is_none() || !input.is_bootstrap(),
            "bootstrap stdin requires prearmed execution"
        );
        let held_bootstrap = spec.held_bootstrap_for(input.is_bootstrap())?;
        self.validate_handle(&handle)?;
        ensure!(
            handle.session == spec.session
                && handle.operation == spec.operation_id()
                && handle.name == spec.name()
                && handle.fingerprint_version() == spec.fingerprint_version(),
            "owned instance does not belong to this exact operation"
        );
        if let Some(duration) = duration {
            ensure!(
                !duration.is_zero() && duration <= Duration::from_secs(24 * 60 * 60),
                "runtime timeout must be bounded"
            );
        }
        let id = handle.id.clone();
        let mut moved_batch_fallback = None;
        let mut completed_transport: Option<ExecutionResult> = None;
        let mut stop_snapshot = baseline.map(|observation| OwnedContainerSnapshot {
            handle: handle.clone(),
            observation: observation.clone(),
        });
        let result: Result<OwnedExecutionOutcome> = async {
            let observation = self
                .inspect_created(spec, &id, held_bootstrap, false)
                .await?;
            ensure!(
                !input.is_bootstrap() || observation.configuration_fingerprint.is_some(),
                "retained bootstrap lacks its created confinement fingerprint"
            );
            if let Some(baseline) = baseline {
                ensure!(
                    baseline.configuration_fingerprint.is_some(),
                    "recovered instance lacks its created configuration proof"
                );
                validate_created(spec, baseline)?;
                observation.validate_identity_allocation(baseline)?;
            }
            let created = OwnedContainerSnapshot {
                handle: handle.clone(),
                observation,
            };
            stop_snapshot = Some(created.clone());
            let (control, legacy_veto) = match on_created(created.clone()).await? {
                OwnedStartDirective::ObserveOnly => {
                    let error = anyhow::Error::new(RuntimePrestartStageFailure::observe_only());
                    return Err(error.context(
                        "owned instance retained for reconciliation without emitting start",
                    ));
                }
                OwnedStartDirective::Legacy(veto) => {
                    ensure!(
                        duration.is_some(),
                        "legacy start cannot authorize prearmed transport"
                    );
                    (None, veto)
                }
                OwnedStartDirective::Prearmed(control) => {
                    ensure!(
                        duration.is_none(),
                        "prearmed start cannot authorize legacy transport"
                    );
                    (Some(control), None)
                }
                OwnedStartDirective::AbandonedBeforeClaim(retirement) => {
                    ensure!(
                        duration.is_none(),
                        "unused-slot retirement requires the prearmed transport"
                    );
                    return Ok(OwnedExecutionOutcome::Abandoned {
                        owned: created,
                        retirement,
                    });
                }
            };
            let instance = CreatedInstance {
                kind: self.binding.kind,
                name: handle.name.clone(),
                target: id.clone(),
            };
            let batch_work = if duration.is_some() && !spec.security_preset.is_interactive() {
                Some(super::deadline_supervision::WorkObservation::new(Some(
                    self.batch_observation_scope(&id, &created.observation)?,
                )))
            } else {
                None
            };
            let on_started = tokio::sync::Mutex::new(on_started);
            let callback = |_| async {
                // RuntimeState cannot represent every unknown fact. Inspect
                // again instead of promoting its compatibility bool defaults.
                let observation = match batch_work.as_ref() {
                    Some(work) => {
                        // Preserve observe(handle)'s original pure checks as
                        // well as the actual observation/created validation.
                        self.validate_handle(&handle)?;
                        work.require_binding(self, &handle.id, &created.observation)?;
                        let observed = work.observe().await?;
                        ensure!(
                            observed.name == handle.name,
                            "runtime owned instance name changed"
                        );
                        observed
                    }
                    None => self.observe(&handle).await?,
                };
                observation.validate_identity_allocation(&created.observation)?;
                let mut callback = on_started.lock().await;
                callback(observation).await
            };
            let result = if let Some(control) = control {
                self.runtime
                    .execute_created_prearmed(
                        &instance,
                        input,
                        stdout_limit,
                        self,
                        &created,
                        control,
                        callback,
                    )
                    .await?
            } else {
                let OwnedStartInput::Historical(input) = input else {
                    bail!("bootstrap stdin cannot use the legacy start transport");
                };
                self.runtime
                    .execute_created(
                        &instance,
                        input,
                        duration.context("missing legacy runtime duration")?,
                        stdout_limit,
                        Some((self, &created.observation)),
                        spec.security_preset
                            .is_interactive()
                            .then_some(spec.terminal_secrets.0.as_slice()),
                        batch_work.clone(),
                        legacy_veto,
                        callback,
                    )
                    .await
                    .map_err(|failure| {
                        moved_batch_fallback = failure.moved_fallback;
                        anyhow::Error::new(*failure)
                    })?
            };
            completed_transport = Some(result);
            #[cfg(all(test, unix))]
            legacy_completion_tests::after_transport(&handle.id);
            let terminal = self.observe(&handle).await?;
            terminal.validate_terminal(self.binding.kind, &created.observation)?;
            let result = completed_transport
                .take()
                .context("completed transport result is absent")?;
            Ok(OwnedExecutionOutcome::Executed(ManagedRetainedExecution {
                execution: RetainedExecution { result, instance },
                created,
                terminal,
            }))
        }
        .await;
        match result {
            Ok(result) => Ok(result),
            Err(error) => {
                if duration.is_none() {
                    // The independent enforcer stays armed. Do not block this
                    // transport's stop drainage on another engine/HTTP operation.
                    return Err(error.context(format!(
                        "owned instance {} retained; independent enforcement and terminal reconciliation remain required", handle.id
                    )));
                }
                let mut fallback_stop = None;
                let mut fallback_error = None;
                let stopped = super::deadline_supervision::resolve_outer_fallback(
                    moved_batch_fallback,
                    || async {
                        match stop_snapshot.as_ref() {
                            Some(snapshot) => match self.stop_snapshot_owned(snapshot).await {
                                Ok(observed) => {
                                    fallback_stop = Some(observed);
                                    true
                                }
                                Err(cause) => {
                                    fallback_error = Some(cause);
                                    false
                                }
                            },
                            None => false,
                        }
                    },
                )
                .await;
                let diagnosis = if stopped {
                    "runtime observed it not running"
                } else {
                    "stop unknown: authority unavailable or changed; instance may still be running"
                };
                let cause = error.context(format!(
                    "owned instance {} retained for diagnosis; {diagnosis}",
                    handle.id
                ));
                // A successful stop observation can still be only Created.
                // Qualify the actual fallback against the original native
                // baseline, and only after a complete transport/IO result.
                let mut completed = None;
                let mut terminal_validation_error = None;
                if completed_transport.is_some()
                    && let (Some(created), Some(terminal)) =
                        (stop_snapshot.as_ref(), fallback_stop.as_ref())
                {
                    match self.validate_handle(&created.handle).and_then(|()| {
                        terminal.validate_terminal(self.binding.kind, &created.observation)?;
                        if let Some(result) = completed_transport.as_ref() {
                            result.state.validate_terminal(self.binding.kind)?;
                            let started =
                                chrono::DateTime::parse_from_rfc3339(&result.state.started_at)?;
                            let finished =
                                chrono::DateTime::parse_from_rfc3339(&result.state.finished_at)?;
                            ensure!(
                                terminal.process.started_at == Some(started)
                                    && terminal.process.finished_at == Some(finished)
                                    && terminal.process.exit_code
                                        == u32::try_from(result.state.exit_code).ok(),
                                "fallback terminal contradicts the completed transport observation"
                            );
                        }
                        Ok(())
                    }) {
                        Ok(()) => {
                            if let Some(result) = completed_transport.take() {
                                completed = Some(ManagedRetainedExecution {
                                    execution: RetainedExecution {
                                        result,
                                        instance: CreatedInstance {
                                            kind: self.binding.kind,
                                            name: created.handle.name.clone(),
                                            target: created.handle.id.clone(),
                                        },
                                    },
                                    created: created.clone(),
                                    terminal: terminal.clone(),
                                });
                            }
                        }
                        Err(error) => terminal_validation_error = Some(error),
                    }
                }
                Err(RetainedLegacyExecutionFailure {
                    cause,
                    _runtime: self.clone(),
                    owned: stop_snapshot,
                    transport: completed_transport,
                    fallback_stop,
                    fallback_error,
                    terminal_validation_error,
                    completed,
                }
                .into())
            }
        }
    }
    /// Consume once only the completion produced by this exact retained
    /// runtime after its real fallback passed full terminal validation. The
    /// original error (including contexts and ancillary causes) stays owned.
    /// This port performs no runtime operation or additional stop.
    pub(crate) fn take_completed_legacy_failure(
        &self,
        spec: &ContainerSpec,
        mut error: anyhow::Error,
    ) -> std::result::Result<(ManagedRetainedExecution, anyhow::Error), anyhow::Error> {
        let completed = error
            .downcast_mut::<RetainedLegacyExecutionFailure>()
            .and_then(|failure| {
                if std::sync::Arc::ptr_eq(&self.runtime.anchor, &failure._runtime.runtime.anchor)
                    && self.binding.fingerprint() == failure._runtime.binding.fingerprint()
                    && failure.completed.as_ref().is_some_and(|completed| {
                        let handle = completed.handle();
                        handle.session == spec.session
                            && handle.operation == spec.operation_id()
                            && handle.name == spec.name()
                            && handle.fingerprint_version() == spec.fingerprint_version()
                    })
                {
                    failure.completed.take()
                } else {
                    None
                }
            });
        match completed {
            Some(completed) => Ok((completed, error)),
            None => Err(error),
        }
    }
    /// Reconciliation observes only; it can never emit start or infer disappearance.
    pub async fn resume_terminal(&self, handle: &ContainerHandle) -> Result<ContainerObservation> {
        let observed = self.observe(handle).await?;
        observed.validate_terminal(self.binding.kind, &observed)?;
        Ok(observed)
    }
    pub async fn cleanup(&self, retained: &ManagedRetainedExecution) -> Result<()> {
        self.cleanup_terminal(retained.handle(), &retained.terminal)
            .await
    }
    /// Caller must first commit terminal observation and its mandatory audit.
    /// This operation never destroys the retained result on cleanup failure.
    pub async fn cleanup_terminal(
        &self,
        handle: &ContainerHandle,
        committed: &ContainerObservation,
    ) -> Result<()> {
        self.cleanup_terminal_inner(handle, committed, None).await
    }

    pub(crate) async fn cleanup_terminal_for_generation(
        &self,
        handle: &ContainerHandle,
        committed: &ContainerObservation,
        generation: &CancelGeneration,
        expected: u64,
    ) -> Result<()> {
        self.cleanup_terminal_inner(
            handle,
            committed,
            Some(GenerationSpawnGuard::new(generation, expected)),
        )
        .await
    }

    async fn cleanup_terminal_inner(
        &self,
        handle: &ContainerHandle,
        committed: &ContainerObservation,
        spawn_guard: Option<GenerationSpawnGuard>,
    ) -> Result<()> {
        self.validate_handle(handle)?;
        let mut scoped = self.clone();
        scoped.runtime = self.runtime.observed_operation(handle.operation_id());
        let trace = scoped.runtime.trace_target(handle.id());
        super::observation::record(&trace, super::observation::Phase::CleanupRequested);
        let result = scoped
            .cleanup_terminal_observed(handle, committed, spawn_guard)
            .await;
        super::observation::record(
            &trace,
            if result.is_ok() {
                super::observation::Phase::CleanupReturnedOk
            } else {
                super::observation::Phase::CleanupReturnedError
            },
        );
        result
    }
    async fn cleanup_terminal_observed(
        &self,
        handle: &ContainerHandle,
        committed: &ContainerObservation,
        spawn_guard: Option<GenerationSpawnGuard>,
    ) -> Result<()> {
        let now = self.observe(handle).await?;
        now.validate_terminal(self.binding.kind, committed)?;
        ensure!(
            now.process.started_at == committed.process.started_at
                && now.process.finished_at == committed.process.finished_at
                && now.process.exit_code == committed.process.exit_code,
            "runtime terminal process changed after acknowledgement"
        );
        self.verify_authority().await?;
        self.cli_sensitive(
            &["rm".into(), handle.id.clone()],
            Duration::from_secs(20),
            &[],
            spawn_guard,
        )
        .await?;
        Ok(())
    }
}
fn stopped_process(observed: &ContainerObservation) -> bool {
    observed.process.running == Some(false)
        && matches!(
            observed.process.status.as_deref(),
            Some("created" | "configured" | "initialized" | "exited" | "stopped" | "dead")
        )
}
fn process_state(observed: &ContainerObservation) -> Result<RuntimeState> {
    Ok(RuntimeState {
        status: observed
            .process
            .status
            .clone()
            .context("runtime status is unknown")?,
        running: observed
            .process
            .running
            .context("runtime running state is unknown")?,
        started_at: observed
            .process
            .started_at
            .map(|d| d.to_rfc3339())
            .unwrap_or_default(),
        finished_at: observed
            .process
            .finished_at
            .map(|d| d.to_rfc3339())
            .unwrap_or_default(),
        exit_code: observed.process.exit_code.map(i64::from).unwrap_or(-1),
        oom_killed: observed.process.oom_killed.unwrap_or(false),
    })
}
fn validate_created(spec: &ContainerSpec, observed: &ContainerObservation) -> Result<()> {
    validate_authorized_identity(spec, observed)?;
    ensure!(
        observed.process.running == Some(false)
            && matches!(
                observed.process.status.as_deref(),
                Some("created" | "configured" | "initialized")
            ),
        "runtime did not prove a fresh never-started instance"
    );
    ensure!(
        observed.process.started_at.is_none(),
        "created instance already has a process start"
    );
    Ok(())
}

fn validate_unstarted_process_timestamps(bytes: &[u8]) -> Result<()> {
    let facts: super::InspectFacts = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("runtime returned malformed inspection facts"))?;
    let zero = chrono::DateTime::parse_from_rfc3339("0001-01-01T00:00:00Z")?;
    ensure!(
        [facts.state.started_at, facts.state.finished_at]
            .into_iter()
            .all(|value| value.as_deref().is_some_and(|timestamp| {
                chrono::DateTime::parse_from_rfc3339(timestamp).is_ok_and(|time| time == zero)
            })),
        "runtime did not explicitly report the import process as never started or finished"
    );
    Ok(())
}
fn validate_authorized_identity(
    spec: &ContainerSpec,
    observed: &ContainerObservation,
) -> Result<()> {
    ensure!(
        observed.name == spec.name() && observed.image_id.is_some(),
        "runtime did not prove the created name and immutable image"
    );
    let requested = spec
        .image
        .strip_prefix("sha256:")
        .filter(|id| valid_immutable_id(id))
        .context("managed creation requires a pre-authorized immutable sha256 image ID")?;
    ensure!(
        observed
            .image_id
            .as_deref()
            .and_then(|id| id.strip_prefix("sha256:").or(Some(id)))
            == Some(requested),
        "runtime immutable image differs from the authorized image"
    );
    ensure!(
        observed.restart_count == Some(0),
        "runtime restart history is unknown or nonzero"
    );
    ensure!(
        observed.allocation.cpu
            == Some(CpuQuota {
                quota: 2,
                period: 1
            })
            && observed.allocation.memory_bytes == Some(1024 * 1024 * 1024)
            && observed.allocation.pids_limit == Some(256),
        "runtime created allocation differs from the requested bounded limits"
    );
    Ok(())
}
#[derive(Deserialize)]
struct PodmanInfo {
    store: PodmanStore,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PodmanStore {
    graph_root: String,
    run_root: String,
}
pub(crate) fn selected_podman_target_args(endpoint: &str) -> Result<Vec<String>> {
    // The remote-only client can select a connection despite --remote=false.
    ensure!(
        endpoint != "local" || cfg!(any(target_os = "linux", target_os = "freebsd")),
        "Podman local mode requires a native engine; supply an explicit Unix socket on this platform"
    );
    podman_target_args(endpoint)
}

fn podman_target_args(endpoint: &str) -> Result<Vec<String>> {
    validate_endpoint(RuntimeKind::Podman, endpoint)?;
    if endpoint == "local" {
        return Ok(vec!["--remote=false".into()]);
    }
    Ok(vec!["--remote".into(), "--url".into(), endpoint.into()])
}

fn validate_endpoint(kind: RuntimeKind, endpoint: &str) -> Result<()> {
    if kind == RuntimeKind::Podman && endpoint == "local" {
        return Ok(());
    }
    ensure!(
        endpoint.starts_with("unix://")
            && endpoint.len() <= 4096
            && !endpoint.chars().any(char::is_control)
            && !endpoint.contains(['?', '#', '@']),
        "managed runtime requires an explicit local Unix endpoint in the form unix:///absolute/path.sock (or 'local' for a native Podman engine on Linux)"
    );
    let path = Path::new(&endpoint[7..]);
    ensure!(
        path.is_absolute()
            && !path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir)),
        "runtime endpoint must be an absolute Unix socket path"
    );
    Ok(())
}

#[cfg(test)]
#[test]
fn podman_diagnostics_and_managed_commands_share_local_targets() {
    assert_eq!(podman_target_args("local").unwrap(), ["--remote=false"]);
    let native_target = selected_podman_target_args("local");
    if cfg!(any(target_os = "linux", target_os = "freebsd")) {
        assert_eq!(native_target.unwrap(), ["--remote=false"]);
    } else {
        assert!(native_target.is_err());
    }
    let socket = "unix:///tmp/podman-qa.sock";
    assert_eq!(
        selected_podman_target_args(socket).unwrap(),
        podman_target_args(socket).unwrap(),
    );
    assert_eq!(
        podman_target_args(socket).unwrap(),
        ["--remote", "--url", socket]
    );
    for incompatible in [
        "",
        "a-default-connection",
        "tcp://127.0.0.1:1234",
        "ssh://elsewhere.invalid/podman.sock",
        "unix://relative.sock",
        "unix:///tmp/podman.sock?connection=qa",
    ] {
        assert!(
            selected_podman_target_args(incompatible).is_err(),
            "{incompatible}"
        );
    }
}

#[cfg(test)]
#[test]
fn podman_endpoint_refusal_shows_expected_unix_format() {
    // A bare socket path (no unix:// scheme) is the common operator mistake on
    // macOS; the refusal must show the exact accepted format instead of a vague
    // "explicit local Unix endpoint" hint.
    let err = selected_podman_target_args("/tmp/podman-qa.sock").unwrap_err();
    let message = format!("{err:#}");
    assert!(
        message.contains("unix:///absolute/path.sock"),
        "refusal must show the unix:// format: {message}"
    );
}

// Configuration is checked while private inspect bytes are in scope, but is not
// copied into an observation or error. Config.Env is never deserialized.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ConfigurationFacts {
    config: CheckedConfig,
    host_config: CheckedHost,
    mounts: Vec<CheckedMount>,
    network_settings: Option<CheckedNetworks>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CheckedConfig {
    labels: std::collections::BTreeMap<String, String>,
    user: String,
    working_dir: String,
    entrypoint: Vec<String>,
    cmd: Option<Vec<String>>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CheckedHost {
    privileged: bool,
    #[serde(rename = "ReadonlyRootfs")]
    readonly_rootfs: bool,
    network_mode: String,
    cap_drop: Vec<String>,
    cap_add: Option<Vec<String>>,
    security_opt: Vec<String>,
    restart_policy: CheckedRestart,
    auto_remove: bool,
    tmpfs: std::collections::BTreeMap<String, String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CheckedRestart {
    name: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CheckedMount {
    #[serde(rename = "Type")]
    kind: String,
    name: Option<String>,
    destination: String,
    #[serde(rename = "RW")]
    rw: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CheckedNetworks {
    networks: Option<std::collections::BTreeMap<String, IgnoredNetworkDetails>>,
}
#[derive(Serialize, Deserialize)]
struct IgnoredNetworkDetails {}

// Keep ConfigurationFacts and its serialization unchanged: v1 checkpoints bind
// exactly that historical JSON. Stream facts are a separate v2 digest input.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct StreamConfigurationFacts {
    config: CheckedStreams,
    host_config: CheckedLogging,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CheckedStreams {
    tty: bool,
    open_stdin: bool,
    stdin_once: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InteractiveEnvironment {
    config: InteractiveTerm,
}
#[derive(Serialize)]
struct InteractiveTerm {
    term: String,
}
impl<'de> Deserialize<'de> for InteractiveTerm {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Environment {
            env: Vec<String>,
        }
        let environment = Environment::deserialize(deserializer)?;
        let terms: Vec<_> = environment
            .env
            .iter()
            .filter_map(|value| value.strip_prefix("TERM="))
            .collect();
        if terms.len() != 1 || terms[0] != super::terminal::TERM {
            return Err(serde::de::Error::custom(
                "interactive TERM differs from the fixed preset",
            ));
        }
        Ok(Self {
            term: terms[0].into(),
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CheckedLogging {
    log_config: CheckedLog,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct CheckedLog {
    #[serde(rename = "Type")]
    kind: String,
    config: Option<std::collections::BTreeMap<String, String>>,
    // Podman exposes these separately from Config. Default empty metadata is
    // compatible with none; a path, tag or nonzero bound is not an authorized
    // logging option. Docker normally omits all three fields.
    path: Option<String>,
    tag: Option<String>,
    size: Option<String>,
}
pub(super) fn configuration_fingerprint(
    bytes: &[u8],
    version: u32,
    kind: RuntimeKind,
) -> Option<String> {
    let mut facts: ConfigurationFacts = serde_json::from_slice(bytes).ok()?;
    if matches!(version, 2 | 3)
        && kind == RuntimeKind::Podman
        && facts.config.user == "1000:1000"
        && let Some(home) = facts.host_config.tmpfs.get_mut("/home/agent")
        && valid_podman_home_mount(home)
    {
        // Podman resolves U to the configured container owner on first start.
        // Keep the remaining options and their order in the fingerprint.
        *home = home
            .split(',')
            .map(|option| {
                if option == "U" {
                    "uid=1000,gid=1000"
                } else {
                    option
                }
            })
            .collect::<Vec<_>>()
            .join(",");
    }
    facts.host_config.cap_drop.sort();
    facts.host_config.security_opt.sort();
    if let Some(add) = facts.host_config.cap_add.as_mut() {
        add.sort();
    }
    facts.mounts.sort_by(|a, b| {
        (&a.kind, &a.destination, &a.name, a.rw).cmp(&(&b.kind, &b.destination, &b.name, b.rw))
    });
    let mut hash = Sha256::new();
    match version {
        1 => {
            hash.update(b"ai-guard:runtime-config:v1\0");
            hash.update(serde_json::to_vec(&facts).ok()?);
        }
        2 => {
            let streams: StreamConfigurationFacts = serde_json::from_slice(bytes).ok()?;
            hash.update(b"ai-guard:runtime-config:v2\0");
            hash.update(serde_json::to_vec(&(facts, streams)).ok()?);
        }
        3 => {
            let streams: StreamConfigurationFacts = serde_json::from_slice(bytes).ok()?;
            // Config.Env is transient private input; only the allowlisted fixed TERM enters the digest.
            let environment: InteractiveEnvironment = serde_json::from_slice(bytes).ok()?;
            hash.update(b"ai-guard:runtime-config:v3\0");
            hash.update(serde_json::to_vec(&(facts, streams, environment)).ok()?);
        }
        _ => return None,
    }
    Some(hex::encode(hash.finalize()))
}
pub(super) fn validate_stream_configuration(
    spec: &ContainerSpec,
    bytes: &[u8],
    kind: RuntimeKind,
) -> Result<()> {
    validate_stream_configuration_for(spec, bytes, kind, None)
}

fn validate_stream_configuration_for(
    spec: &ContainerSpec,
    bytes: &[u8],
    kind: RuntimeKind,
    held_bootstrap: Option<&HeldBootstrapBinding>,
) -> Result<()> {
    if !spec.security_preset.requires_no_engine_logs() {
        ensure!(
            held_bootstrap.is_none(),
            "bootstrap stdin requires strict stream inspection"
        );
        return Ok(());
    }
    let streams: StreamConfigurationFacts = serde_json::from_slice(bytes).map_err(|_| {
        anyhow::anyhow!("runtime inspection lacks required logging or stream configuration facts")
    })?;
    let log = streams.host_config.log_config;
    ensure!(
        log.kind == "none"
            && log
                .config
                .as_ref()
                .is_none_or(std::collections::BTreeMap::is_empty)
            && log.path.as_deref().is_none_or(str::is_empty)
            && log.tag.as_deref().is_none_or(str::is_empty)
            && log.size.as_deref().is_none_or(|size| {
                matches!(size, "" | "0B") || kind == RuntimeKind::Podman && size == "-1B"
            }),
        "runtime logging differs from the required none driver without options"
    );
    if let Some(binding) = held_bootstrap {
        validate_bootstrap_stdin_flags(bytes, kind)?;
        return validate_bootstrap_environment(bytes, binding);
    }
    let import = matches!(spec.purpose, ContainerPurpose::Import);
    if spec.security_preset.is_interactive() {
        ensure!(
            matches!(spec.purpose, ContainerPurpose::Run),
            "interactive preset cannot be used by a helper"
        );
        let _: InteractiveEnvironment = serde_json::from_slice(bytes).map_err(|_| {
            anyhow::anyhow!("runtime interactive TERM differs from the fixed preset")
        })?;
        // Docker 29.2.1 create -i sets StdinOnce=true. Transport loss therefore requests stop;
        // this profile does not claim reattach support. Podman's distinct profile requires false.
        ensure!(
            streams.config.tty
                && streams.config.open_stdin
                && streams.config.stdin_once == (kind == RuntimeKind::Docker),
            "runtime TTY or stdin flags differ from the authorized interactive profile"
        );
        return Ok(());
    }
    // Docker create --interactive sets both AttachStdin and StdinOnce; Podman
    // reports StdinOnce=false because that flag is not implemented there.
    ensure!(
        !streams.config.tty
            && streams.config.stdin_once == (import && kind == RuntimeKind::Docker)
            && streams.config.open_stdin == import,
        "runtime TTY or stdin flags differ from the authorized batch or import profile"
    );
    Ok(())
}
fn validate_configuration(spec: &ContainerSpec, bytes: &[u8], kind: RuntimeKind) -> Result<()> {
    spec.held_bootstrap_for(false)?;
    validate_configuration_for(spec, bytes, kind, None)
}

pub(super) fn validate_bootstrap_stdin_flags(bytes: &[u8], kind: RuntimeKind) -> Result<()> {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Inspect {
        config: Streams,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct Streams {
        tty: bool,
        open_stdin: bool,
        attach_stdin: bool,
        stdin_once: bool,
    }
    let streams = serde_json::from_slice::<Inspect>(bytes)
        .map_err(|_| anyhow::anyhow!("runtime inspection lacks bootstrap stdin flags"))?
        .config;
    let docker = kind == RuntimeKind::Docker;
    ensure!(
        !streams.tty
            && streams.open_stdin
            && streams.attach_stdin == docker
            && streams.stdin_once == docker,
        "runtime stdin flags differ from the retained bootstrap profile"
    );
    Ok(())
}

pub(super) fn validate_bootstrap_environment(
    bytes: &[u8],
    binding: &HeldBootstrapBinding,
) -> Result<()> {
    use crate::egress::proxy_env::parse_marker;
    use serde::de::{Error, SeqAccess, Visitor};

    #[derive(Deserialize)]
    struct Inspect {
        #[serde(rename = "Config")]
        config: Config,
    }

    #[derive(Deserialize)]
    struct Config {
        #[serde(rename = "Env")]
        env: HeldEnvironment,
    }

    #[derive(Default)]
    struct HeldEnvironment {
        attempt: Option<IdV1>,
        proxy_port: Option<u16>,
    }

    impl<'de> Deserialize<'de> for HeldEnvironment {
        fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            struct EnvironmentVisitor;
            impl<'de> Visitor<'de> for EnvironmentVisitor {
                type Value = HeldEnvironment;

                fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str("container environment entries")
                }

                fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
                where
                    A: SeqAccess<'de>,
                {
                    let mut held = HeldEnvironment::default();
                    // Keep only the two markers. Each other entry is dropped
                    // before reading the next, including credential values.
                    while let Some(entry) = sequence.next_element::<String>()? {
                        let (key, value) = entry.split_once('=').unwrap_or((entry.as_str(), ""));
                        match key {
                            HELD_CHALLENGE_ENV if held.attempt.is_none() => {
                                held.attempt = Some(IdV1::parse(value).map_err(A::Error::custom)?);
                            }
                            EGRESS_PROXY_MARKER if held.proxy_port.is_none() => {
                                held.proxy_port = Some(parse_marker(value).ok_or_else(|| {
                                    A::Error::custom("invalid held proxy marker")
                                })?);
                            }
                            HELD_CHALLENGE_ENV | EGRESS_PROXY_MARKER => {
                                return Err(A::Error::custom("duplicate held bootstrap marker"));
                            }
                            _ => {}
                        }
                    }
                    Ok(held)
                }
            }
            deserializer.deserialize_seq(EnvironmentVisitor)
        }
    }

    let Inspect { config } = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("runtime inspection lacks valid held bootstrap markers"))?;
    ensure!(
        config.env.attempt.as_ref() == Some(&binding.attempt)
            && config.env.proxy_port == Some(binding.proxy_port),
        "runtime held bootstrap markers differ from the authorized attempt and profile"
    );
    Ok(())
}

fn validate_configuration_for(
    spec: &ContainerSpec,
    bytes: &[u8],
    kind: RuntimeKind,
    held_bootstrap: Option<&HeldBootstrapBinding>,
) -> Result<()> {
    validate_stream_configuration_for(spec, bytes, kind, held_bootstrap)?;
    let facts: ConfigurationFacts = serde_json::from_slice(bytes).map_err(|_| {
        anyhow::anyhow!("runtime inspection lacks required sandbox configuration facts")
    })?;
    let config = facts.config;
    let purpose = match spec.purpose {
        ContainerPurpose::Import => "import",
        ContainerPurpose::ImportVerification => "import_verification",
        ContainerPurpose::Run => "run",
        ContainerPurpose::Export => "export",
    };
    ensure!(
        config.labels.get("agentsandbox.session") == Some(&spec.session.to_string())
            && config.labels.get("agentsandbox.operation")
                == Some(&spec.operation_id().to_string())
            && config
                .labels
                .get("agentsandbox.purpose")
                .map(String::as_str)
                == Some(purpose),
        "runtime instance labels conflict with the reserved control attempt; no instance adopted"
    );
    ensure!(
        config.user == "1000:1000"
            && config.working_dir == "/workspace"
            && config.entrypoint == spec.argv[..1]
            && config.cmd.unwrap_or_default() == spec.argv[1..],
        "runtime command or execution identity differs from the authorized specification"
    );
    let host = facts.host_config;
    if let Some(networks) = facts
        .network_settings
        .and_then(|settings| settings.networks)
    {
        ensure!(
            match spec.network {
                NetworkMode::None =>
                    networks.is_empty() || networks.len() == 1 && networks.contains_key("none"),
                NetworkMode::Bridge =>
                    networks.len() == 1
                        && (kind == RuntimeKind::Podman || networks.contains_key("bridge")),
            },
            "runtime attached networks differ from the authorized network mode"
        );
    }
    ensure!(
        host.tmpfs.len() == 2
            && host
                .tmpfs
                .get("/tmp")
                .zip(host.tmpfs.get("/home/agent"))
                .is_some_and(|(tmp, home)| match kind {
                    RuntimeKind::Docker => valid_tmpfs(tmp, 256 * 1024 * 1024, 0o1777, None, true)
                        && valid_tmpfs(home, 64 * 1024 * 1024, 0o700, Some((1000, 1000)), false),
                    RuntimeKind::Podman => valid_podman_tmpfs(
                        tmp,
                        256 * 1024 * 1024,
                        0o1777,
                        &["rw", "nosuid", "nodev", "noexec", "rprivate", "tmpcopyup"],
                    ) && valid_podman_home_mount(home),
                }),
        "runtime temporary filesystem limits or restrictions differ from the sandbox preset"
    );
    ensure!(
        !host.privileged
            && host.readonly_rootfs
            && !host.auto_remove
            && host.network_mode == spec.network.to_string(),
        "runtime isolation or network differs from the authorized specification"
    );
    let capabilities_dropped = match kind {
        RuntimeKind::Docker => host
            .cap_drop
            .iter()
            .any(|cap| cap.eq_ignore_ascii_case("ALL")),
        // Podman reports CapDrop as a difference from its configured default
        // bounding set. The effective and bounding sets are reported separately.
        RuntimeKind::Podman => podman_reports_empty_capability_sets(bytes),
    };
    ensure!(
        capabilities_dropped
            && host.cap_add.as_ref().is_none_or(Vec::is_empty)
            && host
                .security_opt
                .iter()
                .any(|opt| opt == "no-new-privileges" || opt == "no-new-privileges=true")
            && matches!(host.restart_policy.name.as_str(), "no" | ""),
        "runtime capability, privilege or restart policy differs from the sandbox preset"
    );
    let mut workspaces = 0;
    for mount in facts.mounts {
        match (mount.kind.as_str(), mount.destination.as_str()) {
            ("volume", "/workspace")
                if mount.name.as_deref() == Some(spec.volume().as_str())
                    && mount.rw == !spec.read_only_workspace =>
            {
                workspaces += 1
            }
            ("tmpfs", "/tmp" | "/home/agent") => {}
            _ => bail!(
                "runtime mounts differ from the authorized workspace and temporary filesystems"
            ),
        }
    }
    ensure!(
        workspaces == 1,
        "runtime did not prove the exclusive named workspace volume"
    );
    Ok(())
}

pub(super) fn podman_reports_empty_capability_sets(bytes: &[u8]) -> bool {
    #[derive(Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct CapabilitySets {
        effective_caps: serde_json::Value,
        bounding_caps: serde_json::Value,
    }
    // Podman encodes nil slices as null. A missing field still fails
    // deserialization and cannot be taken as an empty capability set.
    serde_json::from_slice::<CapabilitySets>(bytes).is_ok_and(|sets| {
        [sets.effective_caps, sets.bounding_caps]
            .iter()
            .all(|caps| caps.is_null() || caps.as_array().is_some_and(Vec::is_empty))
    })
}

fn effective_uid() -> u32 {
    #[cfg(unix)]
    // SAFETY: geteuid has no arguments or memory preconditions.
    {
        unsafe { libc::geteuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

#[cfg(test)]
mod log_policy_tests {
    use super::*;
    use serde_json::{Value, json};

    fn facts() -> Value {
        json!({
            "Id":"a".repeat(64), "Name":"/fixture", "Image":format!("sha256:{}", "b".repeat(64)), "RestartCount":0,
            "State":{"Status":"created", "Running":false},
            "Config": {"Labels":{}, "User":"1000:1000", "WorkingDir":"/workspace",
                "Entrypoint":["/usr/bin/id"], "Cmd":[], "Tty":false,
                "OpenStdin":false, "StdinOnce":false, "Env":["PRIVATE=excluded"]},
            "HostConfig": {"Privileged":false, "ReadonlyRootfs":true, "NetworkMode":"none",
                "NanoCpus":2000000000_u64, "Memory":1073741824_u64, "PidsLimit":256,
                "CapDrop":["ALL"], "SecurityOpt":["no-new-privileges"],
                "RestartPolicy":{"Name":"no"}, "AutoRemove":false, "Tmpfs":{},
                "LogConfig":{"Type":"none", "Config":{}}},
            "Mounts":[]
        })
    }
    fn spec(purpose: ContainerPurpose) -> ContainerSpec {
        ContainerSpec::new(
            Uuid::new_v4(),
            purpose,
            "worker:fixture",
            vec!["/usr/bin/id".into()],
            NetworkMode::None,
        )
        .unwrap()
    }
    fn named_import_verification_spec() -> ContainerSpec {
        spec(ContainerPurpose::ImportVerification)
            .with_managed_names(
                "aig-parent-h-0123456789abcdef0123456789abcdef",
                "aig-parent-work",
            )
            .unwrap()
            .with_read_only_workspace_volume()
            .unwrap()
    }
    fn bytes(value: &Value) -> Vec<u8> {
        serde_json::to_vec(value).unwrap()
    }

    #[test]
    fn named_import_verification_args_mount_exact_workspace_read_only_for_both_engines() {
        let verification = named_import_verification_spec();
        let docker = verification.create_args(RuntimeKind::Docker).unwrap();
        assert!(
            docker.iter().any(|arg| {
                arg == &format!(
                    "type=volume,source={},target=/workspace,readonly",
                    verification.volume()
                )
            }),
            "Docker mount must be explicitly read-only"
        );
        assert!(docker.windows(2).any(|pair| pair == ["--network", "none"]));
        assert!(!docker.iter().any(|arg| arg == "--env-file"));
        assert!(!docker.iter().any(|arg| arg == "--interactive"));

        let podman = verification.create_args(RuntimeKind::Podman).unwrap();
        assert!(
            podman.iter().any(|arg| {
                arg == &format!(
                    "type=volume,source={},target=/workspace,ro",
                    verification.volume()
                )
            }),
            "Podman mount must be explicitly read-only"
        );
        assert!(podman.windows(2).any(|pair| pair == ["--network", "none"]));
        assert!(!podman.iter().any(|arg| arg == "--env-file"));
        assert!(!podman.iter().any(|arg| arg == "--interactive"));
    }

    #[test]
    fn named_import_verification_inspection_rejects_writable_or_extra_workspace_mounts() {
        let verification = named_import_verification_spec();
        let mut observed = facts();
        observed["Config"]["Labels"] = json!({
            "agentsandbox.session": verification.session,
            "agentsandbox.operation": verification.operation_id(),
            "agentsandbox.purpose": "import_verification",
        });
        observed["HostConfig"]["Tmpfs"] = json!({
            "/tmp": "rw,nosuid,nodev,noexec,size=256m,mode=1777",
            "/home/agent": "rw,nosuid,nodev,size=64m,uid=1000,gid=1000,mode=700",
        });
        observed["Mounts"] = json!([{
            "Type": "volume",
            "Name": verification.volume(),
            "Destination": "/workspace",
            "RW": false,
        }]);
        validate_configuration(&verification, &bytes(&observed), RuntimeKind::Docker).unwrap();

        let mut podman = observed.clone();
        podman["HostConfig"]["Tmpfs"] = json!({
            "/tmp": "rw,nosuid,nodev,noexec,size=256m,mode=1777,rprivate,tmpcopyup",
            "/home/agent": "size=64m,mode=0700,U,nosuid,nodev,rprivate",
        });
        podman["HostConfig"]["CapDrop"] = json!(["CAP_CHOWN", "CAP_DAC_OVERRIDE"]);
        podman["EffectiveCaps"] = json!(null);
        podman["BoundingCaps"] = json!(null);
        validate_configuration(&verification, &bytes(&podman), RuntimeKind::Podman).unwrap();

        observed["Mounts"][0]["RW"] = json!(true);
        assert!(
            validate_configuration(&verification, &bytes(&observed), RuntimeKind::Docker).is_err()
        );
        observed["Mounts"][0]["RW"] = json!(false);
        observed["Mounts"].as_array_mut().unwrap().push(json!({
            "Type": "volume",
            "Name": verification.volume(),
            "Destination": "/workspace",
            "RW": false,
        }));
        assert!(
            validate_configuration(&verification, &bytes(&observed), RuntimeKind::Docker).is_err()
        );
    }

    #[test]
    fn generic_effect_gate_rejects_named_verification_but_keeps_ordinary_purposes() {
        let verification = named_import_verification_spec();
        assert!(verification.ensure_generic_create_start_allowed().is_err());

        for purpose in [
            ContainerPurpose::Import,
            ContainerPurpose::Run,
            ContainerPurpose::Export,
        ] {
            assert!(spec(purpose).ensure_generic_create_start_allowed().is_ok());
        }
    }

    #[test]
    fn generic_import_mount_arguments_remain_byte_for_byte_unchanged() {
        let import = spec(ContainerPurpose::Import);
        let args = import.create_args(RuntimeKind::Docker).unwrap();
        assert!(args.iter().any(|arg| {
            arg == &format!("type=volume,source={},target=/workspace", import.volume())
        }));
        assert!(
            !args
                .iter()
                .any(|arg| arg.ends_with(",readonly") || arg.ends_with(",ro"))
        );
    }

    #[test]
    fn podman_home_mount_inspection_requires_exact_private_tmpfs_contract() {
        let worker = spec(ContainerPurpose::Run);
        let mut observed = facts();
        observed["Config"]["Labels"] = json!({
            "agentsandbox.session": worker.session,
            "agentsandbox.operation": worker.operation_id(),
            "agentsandbox.purpose": "run",
        });
        observed["Mounts"] = json!([{
            "Type": "volume", "Name": worker.volume(), "Destination": "/workspace", "RW": true,
        }]);
        let home = "size=64m,mode=0700,U,nosuid,nodev,rprivate";
        let tmp = "rw,nosuid,nodev,noexec,size=256m,mode=1777,rprivate,tmpcopyup";
        observed["HostConfig"]["Tmpfs"] = json!({"/home/agent": home, "/tmp": tmp});
        observed["HostConfig"]["CapDrop"] = json!(["CAP_CHOWN", "CAP_DAC_OVERRIDE"]);
        observed["EffectiveCaps"] = json!(null);
        observed["BoundingCaps"] = json!(null);
        validate_configuration(&worker, &bytes(&observed), RuntimeKind::Podman).unwrap();
        let mut reordered = observed.clone();
        reordered["HostConfig"]["Tmpfs"]["/home/agent"] =
            json!("nodev,rprivate,U,mode=700,nosuid,size=65536k");
        validate_configuration(&worker, &bytes(&reordered), RuntimeKind::Podman).unwrap();
        for (mount, options) in [
            ("/home/agent", home.replace(",U", "")),
            ("/home/agent", home.replace(",U", ",U=false")),
            ("/home/agent", home.replace("64m", "65m")),
            ("/home/agent", home.replace("0700", "0755")),
            ("/home/agent", home.replace("nosuid", "suid")),
            ("/home/agent", home.replace("nodev", "dev")),
            ("/home/agent", home.replace("rprivate", "rshared")),
            ("/home/agent", format!("{home},tmpcopyup")),
            ("/home/agent", format!("{home},ro")),
            ("/home/agent", format!("{home},uid=1000,gid=1000")),
            ("/home/agent", format!("{home},size=64m")),
            ("/tmp", tmp.replace("noexec", "exec")),
            ("/tmp", tmp.replace("256m", "128m")),
            ("/tmp", tmp.replace("1777", "0777")),
            ("/tmp", format!("{tmp},U")),
        ] {
            let mut invalid = observed.clone();
            invalid["HostConfig"]["Tmpfs"][mount] = json!(options);
            let error =
                validate_configuration(&worker, &bytes(&invalid), RuntimeKind::Podman).unwrap_err();
            assert!(
                error.to_string().contains("temporary filesystem"),
                "{mount}: {options}"
            );
        }
        let mut extra = observed;
        extra["HostConfig"]["Tmpfs"]["/var/tmp"] = json!(tmp);
        assert!(validate_configuration(&worker, &bytes(&extra), RuntimeKind::Podman).is_err());
    }

    #[test]
    fn podman_capability_inspection_requires_both_empty_sets() {
        for empty in [json!(null), json!([])] {
            let complete = json!({"EffectiveCaps":empty.clone(),"BoundingCaps":empty});
            assert!(podman_reports_empty_capability_sets(&bytes(&complete)));
            for field in ["EffectiveCaps", "BoundingCaps"] {
                let mut missing = complete.clone();
                missing.as_object_mut().unwrap().remove(field);
                assert!(!podman_reports_empty_capability_sets(&bytes(&missing)));
                for value in [json!(["CAP_CHOWN"]), json!(""), json!({}), json!([null])] {
                    let mut invalid = complete.clone();
                    invalid[field] = value;
                    assert!(!podman_reports_empty_capability_sets(&bytes(&invalid)));
                }
            }
        }
    }

    #[test]
    fn interactive_v3_inspection_binds_tty_and_fixed_term_without_secret_environment() {
        let worker = spec(ContainerPurpose::Run)
            .with_security_preset(crate::control::SecurityPresetV1::FixedSandboxInteractiveV3);
        let mut observed = facts();
        observed["Config"]["Tty"] = json!(true);
        observed["Config"]["OpenStdin"] = json!(true);
        observed["Config"]["StdinOnce"] = json!(true);
        observed["Config"]["Env"] = json!(["TERM=xterm-256color", "PRIVATE=first-private-value"]);
        validate_stream_configuration(&worker, &bytes(&observed), RuntimeKind::Docker).unwrap();
        assert!(
            validate_stream_configuration(&worker, &bytes(&observed), RuntimeKind::Podman).is_err()
        );
        let fingerprint =
            configuration_fingerprint(&bytes(&observed), 3, RuntimeKind::Docker).unwrap();
        observed["Config"]["Env"] = json!(["PRIVATE=second-private-value", "TERM=xterm-256color"]);
        assert_eq!(
            configuration_fingerprint(&bytes(&observed), 3, RuntimeKind::Docker).unwrap(),
            fingerprint
        );
        for (field, value) in [
            ("Tty", json!(false)),
            ("OpenStdin", json!(false)),
            ("StdinOnce", json!(false)),
        ] {
            let mut invalid = observed.clone();
            invalid["Config"][field] = value;
            assert!(
                validate_stream_configuration(&worker, &bytes(&invalid), RuntimeKind::Docker)
                    .is_err()
            );
        }
        for environment in [
            json!([]),
            json!(["TERM=dumb"]),
            json!(["TERM=xterm-256color", "TERM=xterm-256color"]),
        ] {
            let mut invalid = observed.clone();
            invalid["Config"]["Env"] = environment;
            assert!(
                validate_stream_configuration(&worker, &bytes(&invalid), RuntimeKind::Docker)
                    .is_err()
            );
            assert!(configuration_fingerprint(&bytes(&invalid), 3, RuntimeKind::Docker).is_none());
        }
        observed["Config"]["StdinOnce"] = json!(false);
        validate_stream_configuration(&worker, &bytes(&observed), RuntimeKind::Podman).unwrap();
        let args = worker.create_args(RuntimeKind::Docker).unwrap();
        assert!(args.iter().any(|arg| arg == "--tty"));
        assert!(args.iter().any(|arg| arg == "--interactive"));
        assert!(
            args.windows(2)
                .any(|args| args == ["--env", "TERM=xterm-256color"])
        );
        assert!(args.windows(2).any(|args| args == ["--log-driver", "none"]));
    }

    #[test]
    fn podman_runtime_fingerprint_binds_the_resolved_home_owner_across_start() {
        let mut created = facts();
        created["HostConfig"]["Tmpfs"] = json!({
            "/tmp": "rw,nosuid,nodev,noexec,size=256m,mode=1777,rprivate,tmpcopyup",
            "/home/agent": "size=64m,mode=0700,U,nosuid,nodev,rprivate",
        });
        created["Config"]["Env"] = json!(["TERM=xterm-256color"]);
        let mut started = created.clone();
        started["HostConfig"]["Tmpfs"]["/home/agent"] =
            json!("size=64m,mode=0700,uid=1000,gid=1000,nosuid,nodev,rprivate");
        for version in [2, 3] {
            let fingerprint = |document: &Value| {
                configuration_fingerprint(&bytes(document), version, RuntimeKind::Podman).unwrap()
            };
            assert_eq!(fingerprint(&created), fingerprint(&started));
            for home in [
                "size=64m,mode=0700,uid=1001,gid=1000,nosuid,nodev,rprivate",
                "size=64m,mode=0700,uid=1000,gid=1001,nosuid,nodev,rprivate",
                "size=64m,mode=0755,U,nosuid,nodev,rprivate",
                "size=65m,mode=0700,U,nosuid,nodev,rprivate",
                "size=64m,mode=0700,U=false,nosuid,nodev,rprivate",
                "size=64m,mode=0700,U,U,nosuid,nodev,rprivate",
                "size=64m,mode=0700,U,nosuid,nodev,rprivate,tmpcopyup",
            ] {
                let mut changed = started.clone();
                changed["HostConfig"]["Tmpfs"]["/home/agent"] = json!(home);
                assert_ne!(fingerprint(&created), fingerprint(&changed));
            }
            let mut wrong_user = created.clone();
            wrong_user["Config"]["User"] = json!("1001:1000");
            assert_ne!(fingerprint(&created), fingerprint(&wrong_user));
        }
        assert_ne!(
            configuration_fingerprint(&bytes(&created), 1, RuntimeKind::Podman).unwrap(),
            configuration_fingerprint(&bytes(&started), 1, RuntimeKind::Podman).unwrap()
        );
        assert_ne!(
            configuration_fingerprint(&bytes(&created), 2, RuntimeKind::Docker).unwrap(),
            configuration_fingerprint(&bytes(&started), 2, RuntimeKind::Docker).unwrap()
        );
    }

    #[test]
    fn historical_fingerprint_is_unchanged_and_v2_binds_streams_without_environment() {
        let original = facts();
        let old = configuration_fingerprint(&bytes(&original), 1, RuntimeKind::Docker).unwrap();
        assert_eq!(
            old,
            "694ac612677265f6f82c2f02c30d3ff752bfea82541bdd082e65c210639cdf3f"
        );
        let current = configuration_fingerprint(&bytes(&original), 2, RuntimeKind::Docker).unwrap();
        assert_ne!(old, current);
        for (section, field, changed) in [
            ("Config", "Tty", json!(true)),
            ("Config", "OpenStdin", json!(true)),
            ("Config", "StdinOnce", json!(true)),
            (
                "HostConfig",
                "LogConfig",
                json!({"Type":"local","Config":{}}),
            ),
        ] {
            let mut value = original.clone();
            value[section][field] = changed;
            assert_eq!(
                configuration_fingerprint(&bytes(&value), 1, RuntimeKind::Docker).unwrap(),
                old
            );
            assert_ne!(
                configuration_fingerprint(&bytes(&value), 2, RuntimeKind::Docker).unwrap(),
                current
            );
        }
        let mut secret = original.clone();
        secret["Config"]["Env"] = json!(["PRIVATE=different-secret"]);
        assert_eq!(
            configuration_fingerprint(&bytes(&secret), 1, RuntimeKind::Docker).unwrap(),
            old
        );
        assert_eq!(
            configuration_fingerprint(&bytes(&secret), 2, RuntimeKind::Docker).unwrap(),
            current
        );
        let mut historical = original.clone();
        for field in ["Tty", "OpenStdin", "StdinOnce"] {
            historical["Config"].as_object_mut().unwrap().remove(field);
        }
        historical["HostConfig"]
            .as_object_mut()
            .unwrap()
            .remove("LogConfig");
        assert_eq!(
            configuration_fingerprint(&bytes(&historical), 1, RuntimeKind::Docker).unwrap(),
            old
        );
        assert!(configuration_fingerprint(&bytes(&historical), 2, RuntimeKind::Docker).is_none());
    }

    #[test]
    fn logging_and_stream_profile_fail_closed_before_start() {
        let worker = spec(ContainerPurpose::Run);
        let import = spec(ContainerPurpose::Import);
        validate_stream_configuration(&worker, &bytes(&facts()), RuntimeKind::Docker).unwrap();
        let mut input = facts();
        input["Config"]["OpenStdin"] = json!(true);
        validate_stream_configuration(&import, &bytes(&input), RuntimeKind::Podman).unwrap();
        assert!(
            validate_stream_configuration(&import, &bytes(&input), RuntimeKind::Docker).is_err()
        );
        input["Config"]["StdinOnce"] = json!(true);
        validate_stream_configuration(&import, &bytes(&input), RuntimeKind::Docker).unwrap();
        assert!(
            validate_stream_configuration(&import, &bytes(&input), RuntimeKind::Podman).is_err()
        );
        assert!(
            validate_stream_configuration(&worker, &bytes(&input), RuntimeKind::Docker).is_err()
        );
        for value in [
            json!(null),
            json!({}),
            json!({"Type":"local","Config":{}}),
            json!({"Type":"none","Config":{"max-size":"1m"}}),
            json!({"Type":"none","Path":"/tmp/engine.log"}),
            json!({"Type":"none","Tag":"secret"}),
            json!({"Type":"none","Size":"1MB"}),
            json!({"Type":"none","Unexpected":"value"}),
        ] {
            let mut input = facts();
            input["HostConfig"]["LogConfig"] = value;
            for kind in [RuntimeKind::Docker, RuntimeKind::Podman] {
                assert!(validate_stream_configuration(&worker, &bytes(&input), kind).is_err());
            }
        }
        for field in ["Tty", "OpenStdin", "StdinOnce"] {
            let mut changed = facts();
            changed["Config"][field] = json!(true);
            assert!(
                validate_stream_configuration(&worker, &bytes(&changed), RuntimeKind::Docker)
                    .is_err()
            );
            changed["Config"].as_object_mut().unwrap().remove(field);
            assert!(
                validate_stream_configuration(&worker, &bytes(&changed), RuntimeKind::Docker)
                    .is_err()
            );
        }
        // Podman carries empty auxiliary values and can report -1B for an
        // unset limit even when the logging driver is none.
        let mut podman = facts();
        podman["HostConfig"]["LogConfig"] =
            json!({"Type":"none","Config":null,"Path":"","Tag":"","Size":"0B"});
        validate_stream_configuration(&worker, &bytes(&podman), RuntimeKind::Podman).unwrap();
        podman["HostConfig"]["LogConfig"]["Size"] = json!("-1B");
        validate_stream_configuration(&worker, &bytes(&podman), RuntimeKind::Podman).unwrap();
        assert!(
            validate_stream_configuration(&worker, &bytes(&podman), RuntimeKind::Docker).is_err()
        );
        podman["HostConfig"]["LogConfig"]["Config"] = json!({"max-size":"1m"});
        assert!(
            validate_stream_configuration(&worker, &bytes(&podman), RuntimeKind::Podman).is_err()
        );
    }

    #[test]
    fn historical_handles_and_observations_keep_their_digest_version() {
        let handle_json = json!({"authority_fingerprint":"old-authority", "session":Uuid::new_v4(),
            "operation":Uuid::new_v4(), "id":"a".repeat(64), "name":"old-worker"});
        let handle: ContainerHandle = serde_json::from_value(handle_json.clone()).unwrap();
        assert_eq!(handle.fingerprint_version(), 1);
        assert_eq!(serde_json::to_value(handle).unwrap(), handle_json);
        let legacy =
            parse_container_observation_version(&bytes(&facts()), RuntimeKind::Docker, 1).unwrap();
        let serialized = serde_json::to_value(&legacy).unwrap();
        assert!(
            serialized
                .get("configuration_fingerprint_version")
                .is_none()
        );
        assert!(!serialized.to_string().contains("PRIVATE"));
        let recovered: ContainerObservation = serde_json::from_value(serialized.clone()).unwrap();
        assert_eq!(serde_json::to_value(&recovered).unwrap(), serialized);
        let current =
            parse_container_observation_version(&bytes(&facts()), RuntimeKind::Docker, 2).unwrap();
        assert_eq!(current.configuration_fingerprint_version, Some(2));
        current.validate_identity_allocation(&current).unwrap();
        recovered.validate_identity_allocation(&recovered).unwrap();
        assert!(current.validate_identity_allocation(&recovered).is_err());
    }
}
fn podman_selection_env(
    invocation: &InvocationBindingV1,
) -> std::collections::BTreeMap<String, Option<String>> {
    let mut selection = std::collections::BTreeMap::new();
    for key in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_RUNTIME_DIR",
        "CONTAINERS_CONF",
        "CONTAINERS_CONF_OVERRIDE",
        "CONTAINERS_STORAGE_CONF",
        "STORAGE_DRIVER",
        "STORAGE_OPTS",
        "PODMAN_CONNECTIONS_CONF",
    ] {
        let value = invocation.environment.get(key).cloned();
        selection.insert(key.into(), value);
    }
    selection
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WorkspaceVolume {
    name: String,
    driver: String,
    scope: String,
    labels: std::collections::BTreeMap<String, String>,
    options: Option<std::collections::BTreeMap<String, String>>,
}

fn parse_tmpfs_options(options: &str) -> Option<std::collections::BTreeMap<&str, Option<&str>>> {
    let mut parsed = std::collections::BTreeMap::new();
    for option in options.split(',') {
        let (key, value) = option
            .split_once('=')
            .map_or((option, None), |(key, value)| (key, Some(value)));
        if parsed.insert(key, value).is_some() {
            return None;
        }
    }
    Some(parsed)
}

fn valid_podman_tmpfs(options: &str, size: u64, mode: u32, flags: &[&str]) -> bool {
    // Podman 6.1 reports its engine-set ownership through U and leaves the
    // default read/write flag implicit for an explicit type=tmpfs home mount.
    let Some(mut parsed) = parse_tmpfs_options(options) else {
        return false;
    };
    let measured_size = parsed.remove("size").flatten().and_then(parse_memory_size);
    let measured_mode = parsed
        .remove("mode")
        .flatten()
        .and_then(|value| u32::from_str_radix(value, 8).ok());
    measured_size == Some(size)
        && measured_mode == Some(mode)
        && parsed.len() == flags.len()
        && flags.iter().all(|flag| parsed.get(flag) == Some(&None))
}

fn valid_podman_home_mount(options: &str) -> bool {
    valid_podman_tmpfs(
        options,
        64 * 1024 * 1024,
        0o700,
        &["U", "nosuid", "nodev", "rprivate"],
    )
}

fn valid_tmpfs(
    options: &str,
    size: u64,
    mode: u32,
    owner: Option<(u32, u32)>,
    noexec: bool,
) -> bool {
    let Some(parsed) = parse_tmpfs_options(options) else {
        return false;
    };
    if !["rw", "nosuid", "nodev"]
        .iter()
        .all(|flag| parsed.contains_key(flag))
        || ["ro", "suid", "dev"]
            .iter()
            .any(|flag| parsed.contains_key(flag))
        || noexec && parsed.contains_key("exec")
        || noexec && !parsed.contains_key("noexec")
    {
        return false;
    }
    let measured_size = parsed
        .get("size")
        .and_then(|value| *value)
        .and_then(parse_memory_size);
    let measured_mode = parsed
        .get("mode")
        .and_then(|value| *value)
        .and_then(|value| u32::from_str_radix(value, 8).ok());
    let owner_matches = owner.is_none_or(|(uid, gid)| {
        parsed
            .get("uid")
            .and_then(|value| *value)
            .and_then(|value| value.parse::<u32>().ok())
            == Some(uid)
            && parsed
                .get("gid")
                .and_then(|value| *value)
                .and_then(|value| value.parse::<u32>().ok())
                == Some(gid)
    });
    measured_size == Some(size) && measured_mode == Some(mode) && owner_matches
}
fn parse_memory_size(value: &str) -> Option<u64> {
    let (digits, multiplier) = match value.as_bytes().last().copied() {
        Some(b'k' | b'K') => (&value[..value.len() - 1], 1024),
        Some(b'm' | b'M') => (&value[..value.len() - 1], 1024 * 1024),
        Some(b'g' | b'G') => (&value[..value.len() - 1], 1024 * 1024 * 1024),
        _ => (value, 1),
    };
    digits.parse::<u64>().ok()?.checked_mul(multiplier)
}

#[cfg(all(test, unix))]
mod named_import_verification_volume_tests {
    use super::*;
    use crate::{control::*, model::CommandSpec};
    use std::{fs, os::unix::fs::PermissionsExt};

    fn policy() -> HelperExecutionPolicyV1 {
        let helper_id = Uuid::from_u128(4);
        HelperExecutionPolicyV1 {
            version: 1,
            parent_worker_id: Uuid::from_u128(1).to_string(),
            parent_attempt_id: Uuid::from_u128(2).to_string(),
            parent_spec_hash: "a".repeat(64),
            session_id: Uuid::from_u128(3).to_string(),
            workspace_manifest_digest: "b".repeat(64),
            helper_id: helper_id.to_string(),
            purpose: HelperPurposeV1::ImportVerification,
            authority_id: Uuid::from_u128(5).to_string(),
            runtime_kind: RuntimeKindV1::Docker,
            image_id: format!("sha256:{}", "c".repeat(64)),
            stable_name: format!("aig-ns-worker-h-{}", helper_id.simple()),
            volume_name: "aig-ns-work".into(),
            command: CommandSpec {
                argv: vec![
                    "/bin/tar".into(),
                    "--create".into(),
                    "--file=-".into(),
                    "--directory=/workspace".into(),
                    ".".into(),
                ],
                cwd: "/workspace".into(),
            },
            network: NetworkV1::None,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            cpu_millis: 2000,
            memory_bytes: 1_073_741_824,
            pids_limit: 256,
            timeout_seconds: 120,
            restart_policy: RestartPolicyV1::No,
            credential_binding_id: None,
            credential_names: Vec::new(),
        }
    }

    #[tokio::test]
    async fn mismatched_existing_workspace_stops_before_any_runtime_create() {
        let temporary = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("engine.rs");
        let program = root.join("engine");
        fs::write(
            &source,
            include_str!("../../tests/runtime_fixtures/managed.rs"),
        )
        .unwrap();
        let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let compiled = tokio::time::timeout(
            Duration::from_secs(30),
            Command::new(compiler)
                .args(["--edition=2024", "--crate-name", "named_volume_fixture"])
                .arg(&source)
                .arg("-o")
                .arg(&program)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .expect("the local Rust CLI fixture compiles within its harness budget")
        .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let metadata = program.with_extension("fixture");
        fs::create_dir(&metadata).unwrap();
        fs::write(
            metadata.join("AI_GUARD_MANAGED_ROOT"),
            root.as_os_str().as_encoded_bytes(),
        )
        .unwrap();
        fs::write(metadata.join("AI_GUARD_MANAGED_CHILD"), b"success").unwrap();

        let runtime = Runtime::with_program(RuntimeKind::Docker, &program).unwrap();
        let managed = ManagedRuntime::connect_docker_with_runtime(
            Uuid::from_u128(5),
            "unix:///fixture/docker.sock",
            runtime,
        )
        .await
        .unwrap();
        managed.verify_authority().await.unwrap();
        let error = managed
            .verify_named_import_verification_workspace(&policy())
            .await
            .expect_err("a foreign/missing workspace volume must not be adopted");
        assert!(
            error
                .to_string()
                .contains("exact existing control-owned workspace volume")
        );
        let log = fs::read_to_string(root.join("log")).unwrap();
        assert!(log.lines().any(|line| line.starts_with("volume inspect ")));
        assert!(!log.lines().any(|line| line.starts_with("volume create ")));
        assert!(!log.lines().any(|line| line.starts_with("create ")));
        assert!(!log.lines().any(|line| line.starts_with("start ")));
    }
}
