//! Typed provider selection and private credential preparation.

use crate::control_fs::PrivateFile;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    path::{Path, PathBuf},
};
use uuid::Uuid;
pub mod bootstrap;
mod credentials;
mod selection;

pub const BOOTSTRAP_ENV: &str = "AI_GUARD_PROVIDER_BOOTSTRAP";
pub const BOOTSTRAP_PROGRAM: &str = "/usr/local/bin/ai-guard-provider";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Codex,
    #[value(alias = "claude")]
    ClaudeCode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    Api,
    Subscription,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum AuthPreference {
    #[default]
    Auto,
    Api,
    Subscription,
}

impl From<AuthMode> for AuthPreference {
    fn from(mode: AuthMode) -> Self {
        match mode {
            AuthMode::Api => Self::Api,
            AuthMode::Subscription => Self::Subscription,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ClientTransport {
    #[default]
    Batch,
    InteractiveTty,
}

impl ClientTransport {
    pub fn is_interactive(self) -> bool {
        self == Self::InteractiveTty
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChoiceSource {
    Cli,
    Profile,
    DefaultProfile,
    Auto,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderChoiceProvenanceV1 {
    pub client: ChoiceSource,
    pub auth_mode: ChoiceSource,
    pub profile: ChoiceSource,
    pub model: ChoiceSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum CredentialSourceV1 {
    ApiEnvFile {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
    },
    CodexAuthFile {
        path: PathBuf,
    },
    #[serde(rename = "claude_oauth_env_file", alias = "claude_o_auth_env_file")]
    ClaudeOAuthEnvFile {
        path: PathBuf,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderProfileV1 {
    pub id: String,
    pub revision: u32,
    pub client: ClientKind,
    pub auth_mode: AuthMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub credential: CredentialSourceV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfigV1 {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    #[serde(default)]
    pub auto_priority: Vec<String>,
    pub profiles: Vec<ProviderProfileV1>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProviderSelection {
    pub client: Option<ClientKind>,
    pub auth_mode: AuthPreference,
    pub profile_id: Option<String>,
    pub transport: ClientTransport,
    pub model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderResolutionV1 {
    pub version: u32,
    pub client: ClientKind,
    pub auth_mode: AuthMode,
    pub transport: ClientTransport,
    pub profile_id: String,
    pub profile_revision: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub credential_binding_id: Uuid,
    pub credential_names: Vec<String>,
    pub invocation_preset_version: u32,
    pub provenance: ProviderChoiceProvenanceV1,
}

impl ProviderResolutionV1 {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1
                && self.invocation_preset_version == 1
                && self.profile_revision > 0
                && !self.credential_binding_id.is_nil(),
            "invalid provider resolution version or binding"
        );
        selection::validate_id(&self.profile_id)?;
        selection::validate_model(self.model.as_deref())?;
        ensure!(
            self.credential_names == [BOOTSTRAP_ENV],
            "invalid provider credential names"
        );
        Ok(())
    }
    pub fn validate_argv(&self, argv: &[String]) -> Result<()> {
        self.validate()?;
        let parsed = selection::parse_bootstrap_argv(argv)?;
        ensure!(
            parsed.client == self.client
                && parsed.auth_mode == self.auth_mode
                && parsed.transport == self.transport
                && parsed.model == self.model,
            "provider invocation differs from the authorized resolution"
        );
        Ok(())
    }
    pub fn provider_name(&self) -> &'static str {
        match self.client {
            ClientKind::Codex => "codex",
            ClientKind::ClaudeCode => "claude",
        }
    }
}

/// Compare the frozen prompt through the existing strict bootstrap parser.
pub(crate) fn validate_frozen_prompt(argv: &[String], prompt: Option<&str>) -> Result<()> {
    let arguments = selection::parse_bootstrap_argv(argv)?;
    ensure!(
        arguments.prompt.as_deref() == prompt,
        "provider invocation differs from its frozen prompt"
    );
    Ok(())
}

/// Validate a frozen handoff purely, returning decoded strings for the journal guard.
pub(crate) fn validate_frozen_credential(argv: &[String], env_bytes: &[u8]) -> Result<Vec<String>> {
    fn strings(value: &serde_json::Value, output: &mut Vec<String>) {
        match value {
            serde_json::Value::String(value) => output.push(value.clone()),
            serde_json::Value::Object(fields) => {
                for (key, value) in fields {
                    output.push(key.clone());
                    strings(value, output);
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    strings(value, output);
                }
            }
            _ => {}
        }
    }
    let validated = (|| -> Result<Vec<String>> {
        let selected = selection::parse_bootstrap_argv(argv)?;
        ensure!(
            env_bytes.len() <= 64 * 1024,
            "invalid private bootstrap size"
        );
        let text = std::str::from_utf8(env_bytes)?;
        let prefix = format!("{BOOTSTRAP_ENV}=");
        let payload = text
            .strip_prefix(&prefix)
            .and_then(|value| value.strip_suffix('\n'))
            .ok_or_else(|| anyhow::anyhow!("invalid private bootstrap framing"))?;
        ensure!(
            !payload.contains(['\n', '\r']),
            "invalid private bootstrap framing"
        );
        let typed: credentials::BootstrapPayload = serde_json::from_str(payload)?;
        ensure!(
            typed.version == 1
                && typed.client == selected.client
                && typed.auth_mode == selected.auth_mode,
            "private bootstrap conflicts with the frozen client or authentication mode"
        );
        let mut output = Vec::new();
        strings(&serde_json::from_str(payload)?, &mut output);
        if selected.client == ClientKind::Codex && selected.auth_mode == AuthMode::Subscription {
            let (_, secrets) = credentials::codex_auth(&typed.secret)?;
            output.extend(secrets);
            strings(&serde_json::from_str(&typed.secret)?, &mut output);
        } else {
            credentials::validate_secret(&typed.secret)?;
        }
        Ok(output)
    })();
    validated.map_err(|_| anyhow::anyhow!("invalid private frozen provider handoff"))
}

pub struct ProviderCredential {
    source: PrivateFile,
    bootstrap: String,
    secrets: Vec<String>,
}

impl fmt::Debug for ProviderCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProviderCredential(<private>)")
    }
}
impl fmt::Display for ProviderCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<private provider credential>")
    }
}
impl ProviderCredential {
    pub fn bootstrap_value(&self) -> &str {
        &self.bootstrap
    }
    pub fn canonical_env_bytes(&self) -> Vec<u8> {
        format!("{BOOTSTRAP_ENV}={}\n", self.bootstrap).into_bytes()
    }
    pub fn secrets(&self) -> Vec<String> {
        self.secrets.clone()
    }
    pub fn source(&self) -> &PrivateFile {
        &self.source
    }
    pub fn validate_source(&self) -> Result<()> {
        self.source.revalidate()
    }
}

#[derive(Debug)]
pub struct PreparedProvider {
    pub resolution: ProviderResolutionV1,
    pub argv: Vec<String>,
    pub credential: ProviderCredential,
}

impl ProviderConfigV1 {
    pub fn validate(&self) -> Result<()> {
        selection::validate_config(self)
    }
}

pub fn load_config(path: &Path) -> Result<ProviderConfigV1> {
    selection::load_config(path)
}

/// Parse and validate a retained private source, resolving credential paths
/// against that source's checked parent rather than the process directory.
pub fn load_config_source(source: &PrivateFile) -> Result<ProviderConfigV1> {
    selection::load_config_source(source)
}

/// Pure profile choice for operator inspection. Validates the configuration and
/// the requested model exactly as `prepare` does, and opens no credential.
pub fn choose_profile<'a>(
    config: &'a ProviderConfigV1,
    selection: &ProviderSelection,
) -> Result<(&'a ProviderProfileV1, ChoiceSource)> {
    selection::choose_profile(config, selection)
}

pub fn prepare(
    config: &ProviderConfigV1,
    selection: &ProviderSelection,
    project: &Path,
    prompt: Option<&str>,
    binding_id: Uuid,
) -> Result<PreparedProvider> {
    selection::prepare(config, selection, project, prompt, binding_id)
}
