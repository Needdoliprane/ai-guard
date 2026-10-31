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
        let mut data: RegistryWireV1 = serde_json::from_slice(source.bytes())
            .map_err(|_| anyhow!("invalid private broker configuration"))?;
        ensure!(
            data.version == 1 && !data.installation_id.is_nil(),
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
            ensure!(
                ids.insert(profile.id.clone()),
                "duplicate broker profile identity"
            );
        }
        source
            .revalidate()
            .map_err(|_| anyhow!("private broker configuration changed"))?;
        Ok(Self {
            source,
            source_directory,
            data,
        })
    }

    pub(super) fn profile(&self, id: &str) -> Result<&ControlProfileV1> {
        self.data
            .profiles
            .iter()
            .find(|profile| profile.id == id)
            .ok_or_else(|| anyhow!("broker request is not authorized"))
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
