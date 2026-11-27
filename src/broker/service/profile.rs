//! Private operator configuration. A profile routes authentication; its tenant
//! strings are expectations to compare with the gateway, never an identity.
use super::protocol::BrokerErrorCodeV1;
use crate::{
    broker::{
        egress_owner::resolve::{Resolver, ResolverConfigV1, SystemLookup},
        journal::JournalScopeV1,
    },
    control::{
        EgressLaunchV1, NetworkV1, RuntimeKindV1, content_hash,
        session_control::{
            LegacyConfirmedLaunchSpecV1, ProvisionLegacySessionTargetV1,
            SessionLaunchTargetSelectionV1, canonical_uuid,
        },
    },
    control_fs::{Directory, PrivateFile, absolute_path},
    provider::AuthPreference,
    runner::{RunOptions, broker::PreparedSubmission},
    runtime::{NetworkMode, RuntimeKind, validate_command},
    workspace::{AuthorizedSource, Limits, PreparedNamedWorkspace},
};
use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

const MAX_CONFIG_BYTES: u64 = 256 * 1024;
const MAX_LEGACY_NAMED_SOURCES_PER_PROFILE: usize = 100;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RegistryWireV1 {
    version: u32,
    pub(super) installation_id: Uuid,
    pub(super) root: PathBuf,
    pub(super) max_enforcer_slots: u32,
    /// Additional native recovery cells. A positive budget reserves one cell
    /// for bounded catch-up, leaving W-1 for permanently accepted STOP owners.
    #[serde(default)]
    pub(super) max_recovery_stop_owners: u32,
    pub(super) profiles: Vec<ControlProfileV1>,
    #[serde(default)]
    pub(super) additional_control_secret_files: Vec<PathBuf>,
    /// Per-session live provider activity budget; defaults when absent.
    #[serde(default)]
    pub(super) provider_activity: Option<ProviderActivityBudgetV1>,
}

/// Operator budget of live provider activity per session. The undelivered
/// backlog bound follows at one fifth of the events and a quarter of the bytes.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ProviderActivityBudgetV1 {
    max_events: u64,
    max_bytes: u64,
}
impl ProviderActivityBudgetV1 {
    pub(super) fn limits(self) -> Result<crate::provider_activity::ActivityLimits> {
        let limits = crate::provider_activity::ActivityLimits {
            max_events: self.max_events,
            max_bytes: self.max_bytes,
            max_backlog_events: (self.max_events / 5).max(1),
            max_backlog_bytes: (self.max_bytes / 4).max(64 * 1024),
        };
        limits.validate()?;
        Ok(limits)
    }
}

pub(super) struct Registry {
    pub(super) source: PrivateFile,
    pub(super) source_directory: Directory,
    pub(super) data: RegistryWireV1,
    start_profiles: BTreeMap<String, PrearmedStartProfile>,
    presence_profiles: BTreeMap<String, PathBuf>,
    legacy_named_sources: BTreeMap<String, LegacyNamedSources>,
    gateway_trust: BTreeMap<String, crate::tls::ClientTrust>,
    relay_profiles: BTreeMap<String, RelayProfileV1>,
    egress_profiles: BTreeMap<String, EgressLaunchV1>,
    egress_resolver: Option<Arc<Resolver>>,
    pub(super) relay_service:
        std::sync::OnceLock<std::sync::Arc<crate::broker::relay_owner::RelayRegistry>>,
}

/// The configured source of one initial intention. Initial can be recorded
/// before opening the source directory or collecting project data.
pub(super) struct LegacyNamedSource {
    source_path: PathBuf,
    source_roots: Vec<PathBuf>,
    identity_hash: String,
    manifest_hash: String,
    registry_file: PrivateFile,
    registry_directory: Directory,
    broker_root: Directory,
    config: Directory,
}

impl LegacyNamedSource {
    /// Open the exact source, verify its physical identity, and collect
    /// through that descriptor with the broker's retained control exclusions.
    pub(super) fn collect(
        self,
        control_parents: &[Directory],
    ) -> Result<PreparedNamedWorkspace, BrokerErrorCodeV1> {
        self.revalidate()?;
        let retained = [&self.broker_root, &self.config, &self.registry_directory]
            .into_iter()
            .chain(control_parents)
            .map(Directory::try_clone_file)
            .collect::<Result<Vec<_>>>()
            .map_err(|_| BrokerErrorCodeV1::Conflict)?;
        let exclusions = retained.iter().collect::<Vec<_>>();
        let source = AuthorizedSource::open(&self.source_path, &self.source_roots)
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let workspace = source
            .prepare_named(&self.identity_hash, Limits::default(), &exclusions)
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?
            .ok_or(BrokerErrorCodeV1::Conflict)?;
        let digest = content_hash("ai-guard:workspace-manifest:v1", workspace.manifest())
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        if digest != self.manifest_hash {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        self.revalidate()?;
        for parent in control_parents {
            parent
                .revalidate()
                .map_err(|_| BrokerErrorCodeV1::Conflict)?;
        }
        Ok(workspace)
    }

    fn revalidate(&self) -> Result<(), BrokerErrorCodeV1> {
        self.registry_file
            .revalidate()
            .and_then(|()| self.registry_directory.revalidate())
            .and_then(|()| self.broker_root.revalidate())
            .and_then(|()| self.config.revalidate())
            .map_err(|_| BrokerErrorCodeV1::Conflict)
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum PrearmedStartProfile {
    #[default]
    Legacy,
    DockerMinimal,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartProfileWireV1 {
    version: u32,
    mode: StartProfileMode,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum StartProfileMode {
    Legacy,
    DockerMinimal,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryWireV2 {
    version: u32,
    installation_id: Uuid,
    root: PathBuf,
    max_enforcer_slots: u32,
    #[serde(default)]
    max_recovery_stop_owners: u32,
    profiles: Vec<ControlProfileV2>,
    /// Private installation-wide approval and exact control IP exclusions.
    #[serde(default)]
    egress_resolver: Option<ResolverConfigV1>,
    #[serde(default)]
    additional_control_secret_files: Vec<PathBuf>,
    #[serde(default)]
    provider_activity: Option<ProviderActivityBudgetV1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlProfileV2 {
    id: String,
    gateway: String,
    tenant_id: String,
    control_namespace_uuid: Uuid,
    config_dir: PathBuf,
    runner_token_file: PathBuf,
    runtime: RuntimeKind,
    runtime_program: PathBuf,
    runtime_endpoint: String,
    image: String,
    policy_profile: String,
    network: NetworkMode,
    allow_rootful_runtime: bool,
    max_timeout_seconds: u32,
    allow_capture_output: bool,
    managed_clients: bool,
    #[serde(default)]
    providers_file: Option<PathBuf>,
    #[serde(default)]
    manual_credential_file: Option<PathBuf>,
    source_roots: Vec<PathBuf>,
    export_roots: Vec<PathBuf>,
    prearmed_start_profile: StartProfileWireV1,
    #[serde(default)]
    presence_file: Option<PathBuf>,
    #[serde(default)]
    legacy_named_sources: Vec<LegacyNamedSourceV1>,
    #[serde(default)]
    gateway_ca_bundle_file: Option<PathBuf>,
    #[serde(default)]
    relay: Option<RelayProfileV1>,
    #[serde(default)]
    egress: Option<EgressLaunchV1>,
}

type LegacyNamedSources = BTreeMap<(Uuid, u64), PathBuf>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyNamedSourceV1 {
    launch_target_id: String,
    target_revision: u64,
    source: PathBuf,
}

impl LegacyNamedSourceV1 {
    fn resolve(entries: Vec<Self>) -> Result<LegacyNamedSources> {
        ensure!(
            entries.len() <= MAX_LEGACY_NAMED_SOURCES_PER_PROFILE,
            "named broker source count exceeds its bound"
        );
        let mut sources = LegacyNamedSources::new();
        for entry in entries {
            let target = SessionLaunchTargetSelectionV1::new(
                canonical_uuid(&entry.launch_target_id)
                    .map_err(|_| anyhow!("invalid launch_target_id in named broker source"))?,
                entry.target_revision,
            )
            .map_err(|_| anyhow!("invalid target_revision in named broker source"))?;
            ensure!(
                sources
                    .insert(
                        (target.launch_target_id(), target.expected_target_revision()),
                        normalized(&entry.source)?,
                    )
                    .is_none(),
                "duplicate named broker target revision"
            );
        }
        Ok(sources)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayProfileV1 {
    version: u32,
    workload_operations: Vec<PathBuf>,
}
impl RelayProfileV1 {
    fn validate(&mut self) -> Result<()> {
        ensure!(
            self.version == 1 && (1..=100).contains(&self.workload_operations.len()),
            "invalid broker relay configuration"
        );
        let mut seen = std::collections::HashSet::new();
        for path in &mut self.workload_operations {
            *path = normalized(path)?;
            ensure!(
                seen.insert(path.clone()),
                "duplicate broker workload operation reference"
            );
        }
        Ok(())
    }
}
impl ControlProfileV2 {
    fn resolve(
        mut self,
    ) -> Result<(
        ControlProfileV1,
        PrearmedStartProfile,
        Option<PathBuf>,
        Option<RelayProfileV1>,
    )> {
        ensure!(
            self.prearmed_start_profile.version == 1,
            "unsupported broker start profile"
        );
        let selection = match self.prearmed_start_profile.mode {
            StartProfileMode::Legacy => PrearmedStartProfile::Legacy,
            StartProfileMode::DockerMinimal => {
                ensure!(
                    self.runtime == RuntimeKind::Docker,
                    "minimal Docker start profile requires Docker"
                );
                PrearmedStartProfile::DockerMinimal
            }
        };
        let presence = self.presence_file.as_deref().map(normalized).transpose()?;
        if let Some(relay) = self.relay.as_mut() {
            relay.validate()?;
        }
        Ok((
            ControlProfileV1 {
                id: self.id,
                gateway: self.gateway,
                tenant_id: self.tenant_id,
                control_namespace_uuid: self.control_namespace_uuid,
                config_dir: self.config_dir,
                runner_token_file: self.runner_token_file,
                runtime: self.runtime,
                runtime_program: self.runtime_program,
                runtime_endpoint: self.runtime_endpoint,
                image: self.image,
                policy_profile: self.policy_profile,
                network: self.network,
                allow_rootful_runtime: self.allow_rootful_runtime,
                max_timeout_seconds: self.max_timeout_seconds,
                allow_capture_output: self.allow_capture_output,
                managed_clients: self.managed_clients,
                providers_file: self.providers_file,
                manual_credential_file: self.manual_credential_file,
                source_roots: self.source_roots,
                export_roots: self.export_roots,
            },
            selection,
            presence,
            self.relay,
        ))
    }
}
impl fmt::Debug for Registry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BrokerRegistry(<private>)")
    }
}

/// Bind an offline target to the exact configured source while retaining the
/// registry directory, file and source descriptors through publication.
pub(crate) fn with_legacy_target_source<T>(
    registry_path: &Path,
    tenant_id: &str,
    namespace: Uuid,
    mut input: ProvisionLegacySessionTargetV1,
    publish: impl FnOnce(ProvisionLegacySessionTargetV1) -> Result<T>,
) -> Result<T> {
    input.validate()?;
    let target = &mut input.target;
    ensure!(
        target.catalogue.tenant_id == tenant_id,
        "named target tenant does not match the offline database"
    );
    let registry = absolute_path(registry_path)
        .and_then(|path| Registry::load(&path))
        .map_err(|_| anyhow!("private broker registry is unavailable or invalid"))?;
    ensure!(
        registry.data.version == 2,
        "named source attestation requires broker registry version 2"
    );
    let profile = registry
        .profile(&target.broker_profile_id)
        .map_err(|_| anyhow!("named target broker profile is not configured"))?;
    ensure!(
        profile.tenant_id == tenant_id && profile.control_namespace_uuid == namespace,
        "named broker profile tenant or control namespace does not match the offline database"
    );
    let selection = SessionLaunchTargetSelectionV1::new(
        target.catalogue.launch_target_id,
        target.catalogue.revision,
    )?;
    let source = registry.open_legacy_named_source_for_profile(profile, selection)?;
    let observed = source
        .identity_hash()
        .map_err(|_| anyhow!("named broker source identity is unavailable"))?;
    if let Some(expected) = &target.source_identity_hash {
        ensure!(
            expected == &observed,
            "named broker source identity conflicts with the target"
        );
    } else {
        target.version = 2;
        target.source_identity_hash = Some(observed);
    }
    registry
        .source
        .revalidate()
        .and_then(|()| registry.source_directory.revalidate())
        .map_err(|_| anyhow!("private broker registry changed before target publication"))?;
    source
        .revalidate()
        .map_err(|_| anyhow!("named broker source changed before target publication"))?;
    publish(input)
}

impl Registry {
    pub(super) fn load(path: &Path) -> Result<Self> {
        let source = PrivateFile::open(path, MAX_CONFIG_BYTES)
            .map_err(|_| anyhow!("private broker configuration is unavailable or unsafe"))?;
        let parent = source
            .path()
            .parent()
            .ok_or_else(|| anyhow!("invalid broker configuration parent"))?;
        let source_directory = Directory::open_existing(parent)
            .map_err(|_| anyhow!("broker configuration requires a private parent directory"))?;
        #[derive(Deserialize)]
        struct Version {
            version: u32,
        }
        let version: Version = serde_json::from_slice(source.bytes())
            .map_err(|_| anyhow!("invalid private broker configuration"))?;
        let mut start_profiles = BTreeMap::new();
        let mut presence_profiles = BTreeMap::new();
        let mut legacy_named_sources = BTreeMap::new();
        let mut gateway_ca_paths = BTreeMap::new();
        let mut relay_profiles = BTreeMap::new();
        let mut egress_profiles = BTreeMap::new();
        let mut egress_resolver = None;
        let mut data: RegistryWireV1 = match version.version {
            1 => serde_json::from_slice(source.bytes())
                .map_err(|_| anyhow!("invalid private broker configuration"))?,
            2 => {
                let wire: RegistryWireV2 = serde_json::from_slice(source.bytes())
                    .map_err(|_| anyhow!("invalid private broker configuration"))?;
                egress_resolver = wire
                    .egress_resolver
                    .map(|config| Resolver::from_operator_config(config, Arc::new(SystemLookup)))
                    .transpose()?
                    .map(Arc::new);
                let mut profiles = Vec::with_capacity(wire.profiles.len());
                for mut profile in wire.profiles {
                    if !profile.legacy_named_sources.is_empty() {
                        let sources = LegacyNamedSourceV1::resolve(std::mem::take(
                            &mut profile.legacy_named_sources,
                        ))?;
                        legacy_named_sources.insert(profile.id.clone(), sources);
                    }
                    if let Some(path) = &profile.gateway_ca_bundle_file {
                        gateway_ca_paths.insert(profile.id.clone(), normalized(path)?);
                    }
                    if let Some(egress) = profile.egress.take() {
                        egress.validate()?;
                        ensure!(
                            profile.policy_profile == "observe"
                                && profile.network == NetworkMode::None,
                            "broker egress requires observe policy and network none"
                        );
                        egress_profiles.insert(profile.id.clone(), egress);
                    }
                    let (profile, selection, presence, relay) = profile.resolve()?;
                    ensure!(
                        start_profiles
                            .insert(profile.id.clone(), selection)
                            .is_none(),
                        "duplicate broker profile identity"
                    );
                    if let Some(path) = presence {
                        presence_profiles.insert(profile.id.clone(), path);
                    }
                    if let Some(relay) = relay {
                        relay_profiles.insert(profile.id.clone(), relay);
                    }
                    profiles.push(profile);
                }
                RegistryWireV1 {
                    version: wire.version,
                    installation_id: wire.installation_id,
                    root: wire.root,
                    max_enforcer_slots: wire.max_enforcer_slots,
                    max_recovery_stop_owners: wire.max_recovery_stop_owners,
                    profiles,
                    additional_control_secret_files: wire.additional_control_secret_files,
                    provider_activity: wire.provider_activity,
                }
            }
            _ => return Err(anyhow!("unsupported broker configuration")),
        };
        ensure!(
            matches!(data.version, 1 | 2) && !data.installation_id.is_nil(),
            "unsupported broker configuration"
        );
        ensure!(
            (1..=1024).contains(&data.max_enforcer_slots),
            "invalid broker stop capacity"
        );
        ensure!(
            data.max_recovery_stop_owners <= 1024,
            "invalid broker recovery stop capacity"
        );
        ensure!(
            (1..=32).contains(&data.profiles.len()),
            "broker profile count exceeds its bound"
        );
        ensure!(
            data.additional_control_secret_files.len() <= 16,
            "broker control secret references exceed their bound"
        );
        if let Some(budget) = data.provider_activity {
            budget
                .limits()
                .map_err(|_| anyhow!("invalid broker provider activity budget"))?;
        }
        data.root = normalized(&data.root)?;
        for path in &mut data.additional_control_secret_files {
            *path = normalized(path)?;
        }
        let mut ids = std::collections::HashSet::new();
        for profile in &mut data.profiles {
            profile.validate(data.installation_id, egress_resolver.as_deref())?;
            if let Some(sources) = legacy_named_sources.get(&profile.id) {
                for source in sources.values() {
                    ensure!(
                        profile
                            .source_roots
                            .iter()
                            .any(|root| source.starts_with(root)),
                        "named broker source is outside the profile source roots"
                    );
                }
            }
            if let Some(relay) = relay_profiles.get(&profile.id) {
                for operation in &relay.workload_operations {
                    for root in profile.source_roots.iter().chain(&profile.export_roots) {
                        ensure!(
                            !operation.starts_with(root) && !root.starts_with(operation),
                            "broker issued workload references must be outside workspace and export roots"
                        );
                    }
                }
            }
            ensure!(
                ids.insert(profile.id.clone()),
                "duplicate broker profile identity"
            );
        }
        for operation in relay_profiles
            .values()
            .flat_map(|relay| &relay.workload_operations)
        {
            let parent = operation
                .parent()
                .ok_or_else(|| anyhow!("invalid issued workload parent"))?;
            for root in data
                .profiles
                .iter()
                .flat_map(|profile| profile.source_roots.iter().chain(&profile.export_roots))
            {
                ensure!(
                    !parent.starts_with(root) && !root.starts_with(parent),
                    "broker workload private parents must be disjoint from every workspace and export root"
                );
            }
        }
        // Capture explicit public roots before the listener is opened. Every
        // caller later reads this immutable memory, never a changed CA path.
        let mut gateway_trust = BTreeMap::new();
        for profile in &data.profiles {
            if let Some(path) = gateway_ca_paths.get(&profile.id) {
                ensure!(
                    reqwest::Url::parse(&profile.gateway)?.scheme() == "https",
                    "explicit broker gateway CA requires HTTPS"
                );
                let trust = crate::tls::ClientTrust::from_ca_file(path)
                    .map_err(|_| anyhow!("broker gateway CA is invalid or unavailable"))?;
                gateway_trust.insert(profile.id.clone(), trust);
            }
        }
        source
            .revalidate()
            .map_err(|_| anyhow!("private broker configuration changed"))?;
        Ok(Self {
            source,
            source_directory,
            data,
            start_profiles,
            presence_profiles,
            legacy_named_sources,
            gateway_trust,
            relay_profiles,
            egress_profiles,
            egress_resolver,
            relay_service: std::sync::OnceLock::new(),
        })
    }

    pub(super) fn profile(&self, id: &str) -> Result<&ControlProfileV1> {
        self.data
            .profiles
            .iter()
            .find(|profile| profile.id == id)
            .ok_or_else(|| anyhow!("broker request is not authorized"))
    }
    fn profile_for_scope(&self, scope: &JournalScopeV1) -> Result<&ControlProfileV1> {
        let profile = self.profile(scope.profile_id())?;
        ensure!(
            scope.installation_id() == self.data.installation_id
                && scope.tenant_id() == profile.tenant_id
                && scope.gateway_origin() == profile.gateway
                && scope.control_namespace_uuid() == profile.control_namespace_uuid,
            "named broker source scope does not match the operator profile"
        );
        Ok(profile)
    }

    #[cfg(test)]
    fn open_legacy_named_source(
        &self,
        scope: &JournalScopeV1,
        target: SessionLaunchTargetSelectionV1,
    ) -> Result<AuthorizedSource> {
        self.open_legacy_named_source_for_profile(self.profile_for_scope(scope)?, target)
    }

    fn open_legacy_named_source_for_profile(
        &self,
        profile: &ControlProfileV1,
        target: SessionLaunchTargetSelectionV1,
    ) -> Result<AuthorizedSource> {
        let path = self.legacy_named_source_path(profile, target)?;
        self.source.revalidate()?;
        self.source_directory.revalidate()?;
        let source = AuthorizedSource::open(path, &profile.source_roots)
            .map_err(|_| anyhow!("named broker source is unavailable or unsafe"))?;
        self.source.revalidate()?;
        self.source_directory.revalidate()?;
        Ok(source)
    }

    fn legacy_named_source_path(
        &self,
        profile: &ControlProfileV1,
        target: SessionLaunchTargetSelectionV1,
    ) -> Result<&Path> {
        let key = (target.launch_target_id(), target.expected_target_revision());
        self.legacy_named_sources
            .get(&profile.id)
            .and_then(|sources| sources.get(&key))
            .map(PathBuf::as_path)
            .ok_or_else(|| anyhow!("named broker source has no matching target revision"))
    }

    /// Derive the initial intention and retain the configured source selection
    /// for the authenticated scope and the broker's retained control root.
    pub(super) fn prepare_legacy_named_submission(
        &self,
        scope: &JournalScopeV1,
        material: &LegacyConfirmedLaunchSpecV1,
        broker_root: &Directory,
    ) -> Result<(PreparedSubmission, LegacyNamedSource), BrokerErrorCodeV1> {
        let plan = &material.plan;
        if plan.broker_profile_id != scope.profile_id()
            || plan.control_namespace_uuid != scope.control_namespace_uuid().to_string()
        {
            return Err(BrokerErrorCodeV1::Forbidden);
        }
        material
            .validate()
            .map_err(|_| BrokerErrorCodeV1::InvalidInput)?;
        let profile = self
            .profile_for_scope(scope)
            .map_err(|_| BrokerErrorCodeV1::Forbidden)?;
        let launch = &material.launch;
        // Named intent and its journal cannot carry the Docker launch profile
        // binding or controlled egress. Keep these operator choices explicit.
        if self.data.version != 2
            || self.start_profile(&profile.id) != PrearmedStartProfile::Legacy
            || self.egress(&profile.id).is_some()
            || profile.network != NetworkMode::None
            || launch.network != NetworkV1::None
            || launch.provider != "generic"
            || launch.security_preset.is_interactive()
        {
            return Err(BrokerErrorCodeV1::IncompatibleControl);
        }
        let runtime = match profile.runtime {
            RuntimeKind::Docker => RuntimeKindV1::Docker,
            RuntimeKind::Podman => RuntimeKindV1::Podman,
        };
        if launch.profile != profile.policy_profile
            || launch.runtime_kind != runtime
            || launch.image_ref != profile.image
            || launch.timeout_seconds > profile.max_timeout_seconds
            || (launch.capture_output && !profile.allow_capture_output)
            || broker_root.path() != self.data.root.as_path()
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let expected_identity = material
            .source_identity_hash
            .as_deref()
            .ok_or(BrokerErrorCodeV1::IncompatibleControl)?;
        let target =
            SessionLaunchTargetSelectionV1::new(plan.launch_target_id, plan.target_revision)
                .map_err(|_| BrokerErrorCodeV1::InvalidInput)?;
        let source_path = self
            .legacy_named_source_path(profile, target)
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let config = Directory::open_existing(&profile.config_dir)
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let mut options =
            profile.source_options(source_path, launch.timeout_seconds, launch.capture_output);
        options.argv = launch.command.argv.clone();
        let submission =
            PreparedSubmission::from_legacy_named_launch(options, config.clone(), material)
                .map_err(|_| BrokerErrorCodeV1::InvalidInput)?;
        let source = LegacyNamedSource {
            source_path: source_path.to_owned(),
            source_roots: profile.source_roots.clone(),
            identity_hash: expected_identity.to_owned(),
            manifest_hash: launch.workspace_manifest_digest.clone(),
            registry_file: self.source.clone(),
            registry_directory: self.source_directory.clone(),
            broker_root: broker_root.clone(),
            config,
        };
        source.revalidate()?;
        Ok((submission, source))
    }

    pub(super) fn start_profile(&self, id: &str) -> PrearmedStartProfile {
        self.start_profiles.get(id).copied().unwrap_or_default()
    }
    pub(super) fn egress(&self, id: &str) -> Option<&EgressLaunchV1> {
        self.egress_profiles.get(id)
    }
    #[allow(dead_code)] // The resolver is retained for the separate E3 runtime composition.
    pub(super) fn egress_resolver(&self) -> Option<Arc<Resolver>> {
        self.egress_resolver.clone()
    }
    pub(super) fn presence_path(&self, id: &str) -> Option<&Path> {
        self.presence_profiles.get(id).map(PathBuf::as_path)
    }
    pub(super) fn gateway_trust(&self, id: &str) -> crate::tls::ClientTrust {
        self.gateway_trust.get(id).cloned().unwrap_or_default()
    }
    pub(super) fn relay_operations(&self) -> BTreeMap<String, Vec<PathBuf>> {
        self.relay_profiles
            .iter()
            .map(|(profile, relay)| (profile.clone(), relay.workload_operations.clone()))
            .collect()
    }
    pub(super) fn relay_parents(&self) -> std::collections::BTreeSet<PathBuf> {
        self.relay_profiles
            .values()
            .flat_map(|relay| &relay.workload_operations)
            .filter_map(|path| path.parent().map(Path::to_owned))
            .collect()
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ControlProfileV1 {
    pub(super) id: String,
    pub(super) gateway: String,
    pub(super) tenant_id: String,
    pub(super) control_namespace_uuid: Uuid,
    pub(super) config_dir: PathBuf,
    pub(super) runner_token_file: PathBuf,
    pub(super) runtime: RuntimeKind,
    pub(super) runtime_program: PathBuf,
    pub(super) runtime_endpoint: String,
    pub(super) image: String,
    pub(super) policy_profile: String,
    pub(super) network: NetworkMode,
    pub(super) allow_rootful_runtime: bool,
    pub(super) max_timeout_seconds: u32,
    pub(super) allow_capture_output: bool,
    pub(super) managed_clients: bool,
    #[serde(default)]
    pub(super) providers_file: Option<PathBuf>,
    #[serde(default)]
    pub(super) manual_credential_file: Option<PathBuf>,
    pub(super) source_roots: Vec<PathBuf>,
    pub(super) export_roots: Vec<PathBuf>,
}
impl fmt::Debug for ControlProfileV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ControlProfileV1(<private>)")
    }
}
impl ControlProfileV1 {
    fn source_options(&self, source: &Path, timeout: u32, capture_output: bool) -> RunOptions {
        RunOptions {
            project: source.to_owned(),
            config_dir: self.config_dir.clone(),
            gateway: self.gateway.clone(),
            runtime: self.runtime,
            runtime_program: Some(self.runtime_program.clone()),
            runtime_endpoint: Some(self.runtime_endpoint.clone()),
            image: self.image.clone(),
            profile: self.policy_profile.clone(),
            network: self.network,
            allow_rootful_runtime: self.allow_rootful_runtime,
            timeout: u64::from(timeout),
            provider: "generic".into(),
            client: None,
            auth_mode: None,
            client_profile: None,
            providers_file: None,
            prompt: None,
            model: None,
            approve_hash: None,
            capture_output,
            interactive: false,
            secret_env_file: None,
            argv: Vec::new(),
        }
    }

    fn validate(&mut self, installation: Uuid, egress_resolver: Option<&Resolver>) -> Result<()> {
        let url = reqwest::Url::parse(&self.gateway)
            .map_err(|_| anyhow!("invalid broker gateway origin"))?;
        ensure!(
            url.path() == "/"
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "invalid broker gateway origin"
        );
        if let Some(resolver) = egress_resolver
            && let Some(ip) = url
                .host_str()
                .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
        {
            ensure!(
                resolver.excludes_control_ip(ip.to_canonical()),
                "broker egress control addresses must include the configured gateway IP"
            );
        }
        self.gateway = url.as_str().trim_end_matches('/').to_owned();
        JournalScopeV1::new(
            installation,
            &self.tenant_id,
            &self.gateway,
            self.control_namespace_uuid,
            &self.id,
        )
        .map_err(|_| anyhow!("invalid broker profile scope"))?;
        self.config_dir = normalized(&self.config_dir)?;
        self.runner_token_file = normalized(&self.runner_token_file)?;
        self.runtime_program = normalized(&self.runtime_program)?;
        self.providers_file = self.providers_file.as_deref().map(normalized).transpose()?;
        self.manual_credential_file = self
            .manual_credential_file
            .as_deref()
            .map(normalized)
            .transpose()?;
        ensure!(
            !self.runtime_endpoint.is_empty()
                && self.runtime_endpoint.len() <= 4096
                && !self.runtime_endpoint.chars().any(char::is_control),
            "invalid broker runtime endpoint"
        );
        ensure!(
            !self.image.is_empty()
                && self.image.len() <= 1024
                && !self.image.chars().any(char::is_whitespace),
            "invalid broker image reference"
        );
        ensure!(
            matches!(
                self.policy_profile.as_str(),
                "strict" | "review" | "observe"
            ),
            "invalid broker policy profile"
        );
        ensure!(
            (1..=86400).contains(&self.max_timeout_seconds),
            "invalid broker duration limit"
        );
        ensure!(
            !self.managed_clients || self.providers_file.is_some(),
            "managed clients require an explicit provider registry"
        );
        ensure!(
            (1..=8).contains(&self.source_roots.len())
                && (1..=8).contains(&self.export_roots.len()),
            "broker filesystem root count exceeds its bound"
        );
        for path in self
            .source_roots
            .iter_mut()
            .chain(self.export_roots.iter_mut())
        {
            *path = normalized(path)?;
        }
        for source in &self.source_roots {
            for export in &self.export_roots {
                ensure!(
                    !source.starts_with(export) && !export.starts_with(source),
                    "broker source and export roots must be disjoint"
                );
            }
        }
        Ok(())
    }
}

fn normalized(path: &Path) -> Result<PathBuf> {
    ensure!(
        path.is_absolute() && path.as_os_str().len() <= 4096 && path.components().count() <= 64,
        "broker control paths must be bounded absolute paths"
    );
    absolute_path(path).map_err(|_| anyhow!("invalid broker control path"))
}

/// The string containing these exact bytes is the Submit payload hashed by the
/// journal. The outer IPC authentication envelope is never persisted or hashed.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitV1 {
    pub version: u32,
    pub operation_id: Uuid,
    pub project: PathBuf,
    pub execution: RequestedExecutionV1,
    #[serde(default)]
    pub timeout_seconds: Option<u32>,
    #[serde(default)]
    pub capture_output: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RequestedExecutionV1 {
    Command {
        argv: Vec<String>,
    },
    Client {
        client: String,
        auth_mode: Option<AuthPreference>,
        client_profile: Option<String>,
        prompt: String,
        model: Option<String>,
    },
}
impl SubmitV1 {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(
            !bytes.is_empty() && bytes.len() <= 256 * 1024,
            "broker submission exceeds its bound"
        );
        let mut decoder = serde_json::Deserializer::from_slice(bytes);
        let mut ignored = false;
        let value: Self = serde_ignored::deserialize(&mut decoder, |_| ignored = true)
            .map_err(|_| anyhow!("invalid broker submission"))?;
        decoder
            .end()
            .map_err(|_| anyhow!("invalid broker submission"))?;
        ensure!(
            !ignored && value.version == 1 && !value.operation_id.is_nil(),
            "invalid broker submission"
        );
        Ok(value)
    }

    pub(super) fn options(
        &self,
        profile: &ControlProfileV1,
        source: &AuthorizedSource,
    ) -> Result<RunOptions> {
        ensure!(
            normalized(&self.project)? == source.path(),
            "broker source grant does not match submission"
        );
        let timeout = self
            .timeout_seconds
            .unwrap_or(profile.max_timeout_seconds.min(300));
        ensure!(
            timeout > 0 && timeout <= profile.max_timeout_seconds,
            "requested duration exceeds broker profile"
        );
        ensure!(
            !self.capture_output || profile.allow_capture_output,
            "capture is not enabled in broker profile"
        );
        let mut options = profile.source_options(source.path(), timeout, self.capture_output);
        options.secret_env_file = profile.manual_credential_file.clone();
        match &self.execution {
            RequestedExecutionV1::Command { argv } => {
                validate_command(argv).map_err(|_| anyhow!("invalid broker command"))?;
                options.argv = argv.clone();
            }
            RequestedExecutionV1::Client {
                client,
                auth_mode,
                client_profile,
                prompt,
                model,
            } => {
                ensure!(
                    profile.managed_clients
                        && matches!(client.as_str(), "codex" | "claude-code" | "auto"),
                    "managed client is not enabled in broker profile"
                );
                ensure!(
                    !prompt.is_empty() && prompt.len() <= 64 * 1024 && !prompt.contains('\0'),
                    "invalid broker client prompt"
                );
                ensure!(
                    client_profile.as_ref().is_none_or(|name| !name.is_empty()
                        && name.len() <= 80
                        && name.bytes().all(
                            |byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
                        )),
                    "invalid broker provider profile reference"
                );
                ensure!(
                    model.as_ref().is_none_or(|model| !model.is_empty()
                        && model.len() <= 256
                        && !model.chars().any(char::is_control)),
                    "invalid broker model reference"
                );
                options.client = Some(client.clone());
                options.auth_mode = *auth_mode;
                options.client_profile = client_profile.clone();
                options.providers_file = profile.providers_file.clone();
                options.prompt = Some(prompt.clone());
                options.model = model.clone();
                options.secret_env_file = None;
            }
        }
        Ok(options)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;

    fn registry_fixture_v1(base: &Path) -> serde_json::Value {
        // These paths and the loopback/Unix endpoints are data only. Registry
        // parsing must not open a runner token, runtime program or endpoint.
        json!({
            "version": 1,
            "installation_id": "11111111-1111-4111-8111-111111111111",
            "root": base.join("broker-state"),
            "max_enforcer_slots": 8,
            "profiles": [{
                "id": "strict-docker",
                "gateway": "http://127.0.0.1:8787",
                "tenant_id": "tenant-launch-profile-test",
                "control_namespace_uuid": "22222222-2222-4222-8222-222222222222",
                "config_dir": base.join("config-not-opened"),
                "runner_token_file": base.join("runner-token-not-opened"),
                "runtime": "docker",
                "runtime_program": base.join("docker-program-not-opened"),
                "runtime_endpoint": "unix:///not-opened/docker.sock",
                "image": "worker:synthetic",
                "policy_profile": "strict",
                "network": "none",
                "allow_rootful_runtime": false,
                "max_timeout_seconds": 30,
                "allow_capture_output": false,
                "managed_clients": false,
                "providers_file": null,
                "manual_credential_file": null,
                "source_roots": [base.join("source-not-opened")],
                "export_roots": [base.join("export-not-opened")]
            }],
            "additional_control_secret_files": []
        })
    }

    async fn check_named_workspace(
        registry: &Registry,
        scope: &JournalScopeV1,
        target_id: Uuid,
        source: &Path,
    ) -> Result<()> {
        use crate::{
            broker::{
                journal::{
                    ControlSecrets, FsExclusions, Journal, JournalError, JournalLimitsV1,
                    ReserveResult,
                },
                writer::WriterClient,
            },
            control::{
                LaunchSpecV1, NetworkV1, RestartPolicyV1, RuntimeKindV1, SecurityPresetV1,
                session_control::LegacyConfirmedPlanV1,
            },
            model::{CommandSpec, Outcome},
            policy::{Policy, evaluate},
            workspace::{Fingerprint, Manifest, scan_workspace_archive},
        };
        use sha2::{Digest, Sha256};
        use std::time::Duration;

        let file = source.join("tracked.txt");
        let contents = b"prepared contents";
        std::fs::write(&file, contents)?;
        let private = source.join("private-control");
        std::fs::create_dir(&private)?;
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700))?;
        std::fs::write(private.join("excluded.txt"), b"control fixture")?;
        let excluded = Directory::open_existing(&private)?;
        let manifest = Manifest::from([(
            "tracked.txt".into(),
            Fingerprint {
                sha256: hex::encode(Sha256::digest(contents)),
                bytes: contents.len() as u64,
                mode: Some(0o644),
            },
        )]);
        let broker_root = Directory::open_or_create(&registry.data.root)?;
        let profile = registry.profile(scope.profile_id())?;
        let config = Directory::open_or_create(&profile.config_dir)?;
        std::fs::write(
            config.path().join("control.txt"),
            b"private configuration fixture",
        )?;

        let session = Uuid::new_v4();
        let launch = LaunchSpecV1 {
            version: 1,
            session_id: session.to_string(),
            command: CommandSpec {
                argv: vec!["echo".into(), "sandbox-ok".into()],
                cwd: "/workspace".into(),
            },
            profile: profile.policy_profile.clone(),
            runtime_kind: RuntimeKindV1::Docker,
            authority_id: Uuid::new_v4().to_string(),
            image_ref: "worker:synthetic".into(),
            image_id: format!("sha256:{}", "b".repeat(64)),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1_073_741_824,
            pids_limit: 256,
            timeout_seconds: 30,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: RestartPolicyV1::No,
            workspace_manifest_digest: content_hash("ai-guard:workspace-manifest:v1", &manifest)?,
            workspace_import_policy_version: 1,
            provider: "generic".into(),
            capture_output: false,
            credential_binding_id: None,
            credential_names: vec![],
            provider_resolution: None,
            egress: None,
        };
        let selection = SessionLaunchTargetSelectionV1::new(target_id, 1)?;
        let identity = registry
            .open_legacy_named_source(scope, selection)?
            .identity_hash()?;
        let mut material = LegacyConfirmedLaunchSpecV1 {
            version: 2,
            plan: LegacyConfirmedPlanV1 {
                version: 1,
                binding_ref: Uuid::new_v4(),
                control_namespace_uuid: scope.control_namespace_uuid().to_string(),
                operation_id: Uuid::new_v4(),
                session_id: session,
                launch_target_id: target_id,
                target_revision: 1,
                broker_profile_id: scope.profile_id().into(),
                launch_spec_hash: launch.spec_hash()?,
                expires_at: chrono::DateTime::from_timestamp(4_102_444_800, 0).unwrap(),
            },
            launch,
            source_identity_hash: Some(identity),
        };
        let prepare = |material: &LegacyConfirmedLaunchSpecV1| {
            registry.prepare_legacy_named_submission(scope, material, &broker_root)
        };
        let (submission, authorized_source) = prepare(&material)?;
        let options = submission.intent().initial_options().run_options();
        assert_eq!(options.project.as_path(), source);
        assert_eq!(options.config_dir, profile.config_dir);
        assert_eq!(options.gateway, profile.gateway);
        assert_eq!(options.runtime, profile.runtime);
        assert_eq!(
            options.runtime_program.as_ref(),
            Some(&profile.runtime_program)
        );
        assert_eq!(
            options.runtime_endpoint.as_ref(),
            Some(&profile.runtime_endpoint)
        );
        assert_eq!(options.profile, material.launch.profile);
        assert_eq!(options.image, material.launch.image_ref);
        assert_eq!(options.timeout, u64::from(material.launch.timeout_seconds));
        assert_eq!(options.capture_output, material.launch.capture_output);
        assert_eq!(options.network, NetworkMode::None);
        assert!(options.secret_env_file.is_none());
        assert_eq!(options.argv, material.launch.command.argv);

        let journal_root = Directory::open_or_create(&broker_root.path().join("named-journal"))?;
        let journal = Journal::open(
            &journal_root,
            scope.clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(vec![])?,
        )?;
        let writer = WriterClient::start(journal, 8, Duration::from_secs(5))?;
        let operation = material.plan.operation_id;
        let binding_ref = material.plan.binding_ref;
        let ReserveResult::New(ticket) =
            writer.reserve_legacy_named(operation, binding_ref).await?
        else {
            panic!("the confirmed fixture must initially own the reservation");
        };
        let accepted = writer
            .accept_prepared(ticket, submission)
            .await
            .map_err(|failure| failure.error())?;
        let ReserveResult::Accepted(replayed) =
            writer.reserve_legacy_named(operation, binding_ref).await?
        else {
            panic!("an exact replay must carry the retained acceptance");
        };
        assert_eq!(replayed.head(), accepted.head());
        assert!(matches!(
            writer.reserve_legacy_named(operation, Uuid::new_v4()).await,
            Err(JournalError::Conflict)
        ));
        let (rebuilt, _) = prepare(&material)?;
        assert_eq!(replayed.intent_hash(), rebuilt.intent().intent_hash()?);
        let mut divergence = material.clone();
        divergence.source_identity_hash = Some("d".repeat(64));
        let (divergent, _) = prepare(&divergence)?;
        assert_ne!(replayed.intent_hash(), divergent.intent().intent_hash()?);

        // The initial request survives even when collecting the project
        // would now fail. Decisions use the command confirmed by the gateway.
        std::fs::write(&file, b"changed contents")?;
        for (executable, expected) in [("", Outcome::Deny), ("/usr/bin/whoami", Outcome::Review)] {
            let mut command = material.clone();
            command.launch.command.argv = vec![executable.into()];
            command.plan.target_revision = 3;
            command.plan.launch_spec_hash = command.launch.spec_hash()?;
            // Revision 3 is configured with a source that cannot be opened.
            let (initial, source) = prepare(&command)?;
            assert_eq!(
                evaluate(&Policy::default(), initial.intent().request_data()).outcome,
                expected
            );
            assert!(matches!(
                source.collect(std::slice::from_ref(&excluded)),
                Err(BrokerErrorCodeV1::Unavailable)
            ));
        }
        assert!(matches!(
            authorized_source.collect(std::slice::from_ref(&excluded)),
            Err(BrokerErrorCodeV1::Conflict)
        ));
        std::fs::write(&file, contents)?;
        let (_, authorized_source) = prepare(&material)?;
        let workspace = authorized_source.collect(std::slice::from_ref(&excluded))?;
        assert_eq!(workspace.manifest(), &manifest);

        // An invalid profile or launch is refused even if its source is
        // absent, so collecting could not have run before these checks.
        let mut missing = material.clone();
        missing.plan.launch_target_id = Uuid::new_v4();
        let profile_conflicts: [fn(&mut LaunchSpecV1); 5] = [
            |launch| launch.image_ref = "worker:other".into(),
            |launch| launch.runtime_kind = RuntimeKindV1::Podman,
            |launch| launch.timeout_seconds += 1,
            |launch| launch.capture_output = true,
            |launch| launch.profile = "strict".into(),
        ];
        for change in profile_conflicts {
            let mut incompatible = missing.clone();
            change(&mut incompatible.launch);
            incompatible.plan.launch_spec_hash = incompatible.launch.spec_hash()?;
            assert!(matches!(
                prepare(&incompatible),
                Err(BrokerErrorCodeV1::Conflict)
            ));
        }
        missing.launch.profile = "observe".into();
        missing.launch.network = NetworkV1::Bridge;
        missing.plan.launch_spec_hash = missing.launch.spec_hash()?;
        assert!(matches!(
            prepare(&missing),
            Err(BrokerErrorCodeV1::IncompatibleControl)
        ));
        material.version = 1;
        material.source_identity_hash = None;
        assert!(matches!(
            prepare(&material),
            Err(BrokerErrorCodeV1::IncompatibleControl)
        ));
        material.version = 2;
        material.source_identity_hash = Some("d".repeat(64));
        let (_, source_with_wrong_identity) = prepare(&material)?;
        assert!(matches!(
            source_with_wrong_identity.collect(std::slice::from_ref(&excluded)),
            Err(BrokerErrorCodeV1::Conflict)
        ));
        std::fs::write(&file, b"changed contents")?;
        let (_, workspace) = workspace.into_parts();
        let retained = scan_workspace_archive(workspace.archive.reopen()?, Limits::default())?;
        assert_eq!(retained, manifest);
        Ok(())
    }

    #[tokio::test]
    async fn registry_v2_resolves_only_the_exact_named_source_revision() -> Result<()> {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()?;
        let base = temporary.path().canonicalize()?;
        let directory = Directory::open_or_create(&base.join("registry"))?;
        let source_root = base.join("source-not-opened");
        let first = source_root.join("first");
        let second = source_root.join("second");
        let alias = source_root.join("alias");
        std::fs::create_dir_all(&first)?;
        std::fs::create_dir_all(&second)?;
        std::os::unix::fs::symlink(&second, &alias)?;

        let load = |wire: &serde_json::Value| -> Result<Registry> {
            let name = format!("registry-{}.json", Uuid::new_v4());
            let file = directory.persistent_create(&name, &serde_json::to_vec(wire)?)?;
            file.persistent_sync()?;
            directory.sync()?;
            Registry::load(file.path())
        };
        let target_id = Uuid::new_v4();
        let binding = |target_revision: u64, source: &Path| {
            json!({
                "launch_target_id": target_id,
                "target_revision": target_revision,
                "source": source,
            })
        };
        let scope = JournalScopeV1::new(
            "11111111-1111-4111-8111-111111111111".parse()?,
            "tenant-launch-profile-test",
            "http://127.0.0.1:8787",
            "22222222-2222-4222-8222-222222222222".parse()?,
            "strict-docker",
        )?;
        let select = |revision| SessionLaunchTargetSelectionV1::new(target_id, revision);
        let mut wire = registry_fixture_v1(&base);
        assert!(
            load(&wire)?
                .open_legacy_named_source(&scope, select(1)?)
                .is_err()
        );
        wire["profiles"][0]["legacy_named_sources"] = json!([binding(1, &first)]);
        assert!(load(&wire).is_err());
        wire["version"] = json!(2);
        wire["profiles"][0]["prearmed_start_profile"] = json!({"version": 1, "mode": "legacy"});
        wire["profiles"][0]["policy_profile"] = json!("review");
        wire["profiles"][0]["config_dir"] = json!(first.join("broker-config"));
        wire["profiles"][0]["legacy_named_sources"] =
            json!([binding(1, &first), binding(2, &second), binding(3, &alias),]);
        let registry = load(&wire)?;
        let retained = registry.open_legacy_named_source(&scope, select(1)?)?;
        assert_eq!(retained.path(), first.as_path());
        retained.revalidate()?;
        check_named_workspace(&registry, &scope, target_id, &first).await?;
        assert_eq!(
            registry
                .open_legacy_named_source(&scope, select(2)?)?
                .path(),
            second.as_path()
        );
        assert!(
            registry
                .open_legacy_named_source(&scope, select(3)?)
                .is_err()
        );
        assert!(
            registry
                .open_legacy_named_source(&scope, select(4)?)
                .is_err()
        );
        let foreign = JournalScopeV1::new(
            scope.installation_id(),
            scope.tenant_id(),
            scope.gateway_origin(),
            Uuid::new_v4(),
            scope.profile_id(),
        )?;
        assert!(
            registry
                .open_legacy_named_source(&foreign, select(1)?)
                .is_err()
        );

        wire["profiles"][0]["legacy_named_sources"] =
            json!([binding(1, &first), binding(1, &second)]);
        assert!(load(&wire).is_err());
        wire["profiles"][0]["legacy_named_sources"] = json!([binding(1, &base.join("outside"))]);
        assert!(load(&wire).is_err());
        wire["profiles"][0]["legacy_named_sources"] = json!([binding(u64::MAX, &first)]);
        assert!(load(&wire).is_err());
        wire["profiles"][0]["legacy_named_sources"] = serde_json::Value::Array(
            (1..=101)
                .map(|revision| binding(revision, &first))
                .collect(),
        );
        assert!(load(&wire).is_err());

        std::fs::write(registry.source.path(), b"{}")?;
        assert!(
            registry
                .open_legacy_named_source(&scope, select(1)?)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn registry_v2_accepts_explicit_minimal_docker_start_profile() {
        let temporary = tempfile::Builder::new()
            .prefix("broker-launch-registry-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap_or_else(|_| panic!("private registry fixture directory unavailable"));
        let base = temporary
            .path()
            .canonicalize()
            .unwrap_or_else(|_| panic!("private registry fixture root unavailable"));
        let directory = Directory::open_existing(&base)
            .unwrap_or_else(|_| panic!("private registry fixture root unsafe"));
        let v1 = registry_fixture_v1(&base);
        let v1_bytes = serde_json::to_vec(&v1).unwrap();
        let v1_file = directory
            .persistent_create("registry-v1.json", &v1_bytes)
            .unwrap_or_else(|_| panic!("private V1 registry fixture creation failed"));
        v1_file
            .persistent_sync()
            .unwrap_or_else(|_| panic!("private V1 registry fixture synchronization failed"));
        directory.sync().unwrap_or_else(|_| {
            panic!("private registry fixture namespace synchronization failed")
        });
        let legacy = Registry::load(v1_file.path())
            .unwrap_or_else(|_| panic!("the valid V1 registry fixture must load first"));
        assert!(
            legacy.profile("strict-docker").is_ok(),
            "the V1 positive control must expose its configured profile"
        );
        assert!(legacy.start_profile("strict-docker") == PrearmedStartProfile::Legacy);
        assert_eq!(legacy.data.max_recovery_stop_owners, 0);

        let mut v2 = v1;
        v2["version"] = json!(2);
        v2["profiles"][0]["prearmed_start_profile"] =
            json!({"version": 1, "mode": "docker_minimal"});
        let v2_bytes = serde_json::to_vec(&v2).unwrap();
        let v2_file = directory
            .persistent_create("registry-v2.json", &v2_bytes)
            .unwrap_or_else(|_| panic!("private V2 registry fixture creation failed"));
        v2_file
            .persistent_sync()
            .unwrap_or_else(|_| panic!("private V2 registry fixture synchronization failed"));
        directory.sync().unwrap_or_else(|_| {
            panic!("private registry fixture namespace synchronization failed")
        });
        let strict = Registry::load(v2_file.path());
        assert!(
            strict.is_ok(),
            "valid RegistryV2 with explicit minimal Docker start profile must load"
        );
        let strict = strict.unwrap();
        assert!(
            strict.profile("strict-docker").is_ok(),
            "the V2 registry must expose its configured profile"
        );
        assert!(strict.start_profile("strict-docker") == PrearmedStartProfile::DockerMinimal);
        assert_eq!(strict.data.max_recovery_stop_owners, 0);
        assert!(legacy.egress("strict-docker").is_none());
        assert!(strict.egress("strict-docker").is_none());
        assert!(legacy.egress_resolver().is_none());
        assert!(strict.egress_resolver().is_none());

        let mut configured = v2.clone();
        configured["egress_resolver"] = json!({
            "version": 1,
            "host_resolver_approved": true,
            "control_ip_exclusions": ["127.0.0.1", "2001:db8::2"]
        });
        let configured_file = directory
            .persistent_create(
                "registry-resolver.json",
                &serde_json::to_vec(&configured).unwrap(),
            )
            .unwrap();
        configured_file.persistent_sync().unwrap();
        directory.sync().unwrap();
        let configured_registry = Registry::load(configured_file.path()).unwrap();
        let resolver = configured_registry.egress_resolver().unwrap();
        assert!(Arc::ptr_eq(
            &resolver,
            &configured_registry.egress_resolver().unwrap()
        ));
        assert!(configured_registry.egress("strict-docker").is_none());

        configured["egress_resolver"]["control_ip_exclusions"] = json!(["2001:db8::2"]);
        let incomplete_file = directory
            .persistent_create(
                "registry-incomplete-resolver.json",
                &serde_json::to_vec(&configured).unwrap(),
            )
            .unwrap();
        incomplete_file.persistent_sync().unwrap();
        directory.sync().unwrap();
        assert!(Registry::load(incomplete_file.path()).is_err());

        let selection = json!({"version": 1, "profile": "broker_proxy_v1", "proxy_port": 3240});
        for (name, policy, network, egress, accepted) in [
            ("egress", "observe", "none", Some(selection.clone()), true),
            ("historical-bridge", "observe", "bridge", None, true),
            ("historical-none", "observe", "none", None, true),
            (
                "egress-bridge",
                "observe",
                "bridge",
                Some(selection.clone()),
                false,
            ),
            (
                "egress-strict",
                "strict",
                "none",
                Some(selection.clone()),
                false,
            ),
            (
                "egress-version",
                "observe",
                "none",
                Some(json!({"version": 2, "profile": "broker_proxy_v1", "proxy_port": 3240})),
                false,
            ),
            (
                "egress-extra",
                "observe",
                "none",
                Some(
                    json!({"version": 1, "profile": "broker_proxy_v1", "proxy_port": 3240, "extra": true}),
                ),
                false,
            ),
        ] {
            let mut configured = v2.clone();
            configured["profiles"][0]["policy_profile"] = json!(policy);
            configured["profiles"][0]["network"] = json!(network);
            if let Some(egress) = &egress {
                configured["profiles"][0]["egress"] = egress.clone();
            }
            let file = directory
                .persistent_create(name, &serde_json::to_vec(&configured).unwrap())
                .unwrap();
            file.persistent_sync().unwrap();
            directory.sync().unwrap();
            let loaded = Registry::load(file.path());
            assert_eq!(loaded.is_ok(), accepted, "{name}");
            if let Ok(loaded) = loaded {
                assert_eq!(
                    loaded
                        .egress("strict-docker")
                        .map(|value| serde_json::to_value(value).unwrap()),
                    egress
                );
            }
        }
        // Exercise both real private-file decoders and the shared loader
        // validation, without opening any referenced runtime or credential.
        for version in [1, 2] {
            for budget in [0_u32, 1, 1024, 1025, u32::MAX] {
                let mut configured = v2.clone();
                configured["version"] = json!(version);
                if version == 1 {
                    configured["profiles"][0]
                        .as_object_mut()
                        .unwrap()
                        .remove("prearmed_start_profile");
                }
                configured["max_recovery_stop_owners"] = json!(budget);
                let file = directory
                    .persistent_create(
                        &format!("recovery-budget-{version}-{budget}.json"),
                        &serde_json::to_vec(&configured).unwrap(),
                    )
                    .unwrap();
                file.persistent_sync().unwrap();
                directory.sync().unwrap();
                let loaded = Registry::load(file.path());
                if budget <= 1024 {
                    assert_eq!(loaded.unwrap().data.max_recovery_stop_owners, budget);
                } else {
                    assert!(
                        loaded.is_err(),
                        "out-of-range native recovery capacity was accepted"
                    );
                }
            }
        }
        // Every variant uses the real private-file parser. Referenced paths and
        // endpoints remain unopened data, including explicit Legacy in V2.
        for variant in 0..7 {
            let mut changed = v2.clone();
            let expected = variant == 0;
            match variant {
                0 => changed["profiles"][0]["prearmed_start_profile"]["mode"] = json!("legacy"),
                1 => {
                    changed["profiles"][0]
                        .as_object_mut()
                        .unwrap()
                        .remove("prearmed_start_profile");
                }
                2 => changed["profiles"][0]["prearmed_start_profile"]["version"] = json!(2),
                3 => changed["profiles"][0]["prearmed_start_profile"]["mode"] = json!("automatic"),
                4 => changed["profiles"][0]["prearmed_start_profile"]["unknown"] = json!(true),
                5 => changed["profiles"][0]["runtime"] = json!("podman"),
                _ => changed["version"] = json!(1),
            }
            let file = directory
                .persistent_create(
                    &format!("registry-variant-{variant}.json"),
                    &serde_json::to_vec(&changed).unwrap(),
                )
                .unwrap();
            file.persistent_sync().unwrap();
            directory.sync().unwrap();
            let loaded = Registry::load(file.path());
            assert_eq!(loaded.is_ok(), expected, "strict registry variant result");
            if let Ok(loaded) = loaded {
                assert!(loaded.start_profile("strict-docker") == PrearmedStartProfile::Legacy);
            }
        }
    }

    #[test]
    fn submit_cannot_select_broker_egress() {
        let mut payload = json!({
            "version": 1,
            "operation_id": Uuid::new_v4(),
            "project": "/synthetic/project",
            "execution": {"command": {"argv": ["/usr/bin/id"]}}
        });
        assert!(SubmitV1::parse(&serde_json::to_vec(&payload).unwrap()).is_ok());
        payload["egress"] = json!({"version": 1, "profile": "broker_proxy_v1", "proxy_port": 3240});
        assert!(SubmitV1::parse(&serde_json::to_vec(&payload).unwrap()).is_err());
    }
}
