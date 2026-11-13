//! Private operator configuration. A profile routes authentication; its tenant
//! strings are expectations to compare with the gateway, never an identity.
use crate::{
    broker::journal::JournalScopeV1,
    control_fs::{Directory, PrivateFile, absolute_path},
    provider::AuthPreference,
    runner::RunOptions,
    runtime::{NetworkMode, RuntimeKind, validate_command},
    workspace::AuthorizedSource,
};
use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
};
use uuid::Uuid;

const MAX_CONFIG_BYTES: u64 = 256 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RegistryWireV1 {
    version: u32,
    pub(super) installation_id: Uuid,
    pub(super) root: PathBuf,
    pub(super) max_enforcer_slots: u32,
    pub(super) profiles: Vec<ControlProfileV1>,
    #[serde(default)]
    pub(super) additional_control_secret_files: Vec<PathBuf>,
}

pub(super) struct Registry {
    pub(super) source: PrivateFile,
    pub(super) source_directory: Directory,
    pub(super) data: RegistryWireV1,
    start_profiles: BTreeMap<String, PrearmedStartProfile>,
    presence_profiles: BTreeMap<String, PathBuf>,
    gateway_trust: BTreeMap<String, crate::tls::ClientTrust>,
    relay_profiles: BTreeMap<String, RelayProfileV1>,
    pub(super) relay_service:
        std::sync::OnceLock<std::sync::Arc<crate::broker::relay_owner::RelayRegistry>>,
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
    profiles: Vec<ControlProfileV2>,
    #[serde(default)]
    additional_control_secret_files: Vec<PathBuf>,
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
    gateway_ca_bundle_file: Option<PathBuf>,
    #[serde(default)]
    relay: Option<RelayProfileV1>,
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
        let mut gateway_ca_paths = BTreeMap::new();
        let mut relay_profiles = BTreeMap::new();
        let mut data: RegistryWireV1 = match version.version {
            1 => serde_json::from_slice(source.bytes())
                .map_err(|_| anyhow!("invalid private broker configuration"))?,
            2 => {
                let wire: RegistryWireV2 = serde_json::from_slice(source.bytes())
                    .map_err(|_| anyhow!("invalid private broker configuration"))?;
                let mut profiles = Vec::with_capacity(wire.profiles.len());
                for profile in wire.profiles {
                    if let Some(path) = &profile.gateway_ca_bundle_file {
                        gateway_ca_paths.insert(profile.id.clone(), normalized(path)?);
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
                    profiles,
                    additional_control_secret_files: wire.additional_control_secret_files,
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
            (1..=32).contains(&data.profiles.len()),
            "broker profile count exceeds its bound"
        );
        ensure!(
            data.additional_control_secret_files.len() <= 16,
            "broker control secret references exceed their bound"
        );
        data.root = normalized(&data.root)?;
        for path in &mut data.additional_control_secret_files {
            *path = normalized(path)?;
        }
        let mut ids = std::collections::HashSet::new();
        for profile in &mut data.profiles {
            profile.validate(data.installation_id)?;
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
            gateway_trust,
            relay_profiles,
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
    pub(super) fn start_profile(&self, id: &str) -> PrearmedStartProfile {
        self.start_profiles.get(id).copied().unwrap_or_default()
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
    fn validate(&mut self, installation: Uuid) -> Result<()> {
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
        let mut options = RunOptions {
            project: source.path().to_owned(),
            config_dir: profile.config_dir.clone(),
            gateway: profile.gateway.clone(),
            runtime: profile.runtime,
            runtime_program: Some(profile.runtime_program.clone()),
            runtime_endpoint: Some(profile.runtime_endpoint.clone()),
            image: profile.image.clone(),
            profile: profile.policy_profile.clone(),
            network: profile.network,
            allow_rootful_runtime: profile.allow_rootful_runtime,
            timeout: u64::from(timeout),
            provider: "generic".into(),
            client: None,
            auth_mode: None,
            client_profile: None,
            providers_file: None,
            prompt: None,
            model: None,
            approve_hash: None,
            capture_output: self.capture_output,
            interactive: false,
            secret_env_file: profile.manual_credential_file.clone(),
            argv: Vec::new(),
        };
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
        // These paths and the loopback/Unix endpoints are data only. Registry
        // parsing must not open a runner token, runtime program or endpoint.
        let v1 = json!({
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
        });
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
}
