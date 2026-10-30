//! Private immutable invocation metadata, never an execution authorization.
use crate::{
    model::DecisionRequest,
    provider::{AuthPreference, ClientKind},
    runner::RunOptions,
    runtime::{NetworkMode, RuntimeKind},
};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fmt, path::PathBuf};
use uuid::Uuid;

const MAX_PLAN_BYTES: usize = 32 * 1024 * 1024;
const MAX_INTENT_BYTES: usize = 256 * 1024;
const DOMAIN: &[u8] = b"ai-guard.execution-intent.v1\0";

pub type IntentResult<T> = Result<T, IntentError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentError {
    SourceTooLarge,
    IntentTooLarge,
    InvalidPlan,
    InvalidIntent,
    UnsupportedVersion,
    NonInitial,
    Incoherent,
}
impl fmt::Display for IntentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::SourceTooLarge => "execution plan exceeds its input bound",
            Self::IntentTooLarge => "execution intent exceeds its bound",
            Self::InvalidPlan => "invalid private execution plan",
            Self::InvalidIntent => "invalid execution intent",
            Self::UnsupportedVersion => "unsupported execution intent or plan version",
            Self::NonInitial => "execution intent requires an unapplied initial request",
            Self::Incoherent => "execution intent fields are inconsistent",
        })
    }
}
impl std::error::Error for IntentError {}

// New closed projection. Historical RunOptions Serde remains unchanged.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InitialOptionsDataV1 {
    project: PathBuf,
    config_dir: PathBuf,
    gateway: String,
    runtime: RuntimeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime_program: Option<PathBuf>,
    #[serde(default)]
    runtime_endpoint: Option<String>,
    image: String,
    profile: String,
    network: NetworkMode,
    allow_rootful_runtime: bool,
    timeout: u64,
    provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auth_mode: Option<AuthPreference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    providers_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    approve_hash: Option<String>,
    capture_output: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    interactive: bool,
    secret_env_file: Option<PathBuf>,
    argv: Vec<String>,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct InitialOptionsV1(InitialOptionsDataV1);
impl<'de> Deserialize<'de> for InitialOptionsV1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        InitialOptionsDataV1::deserialize(d)
            .map(Self)
            .map_err(|_| D::Error::custom("invalid initial execution options"))
    }
}
impl fmt::Debug for InitialOptionsV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("InitialOptionsV1(<private>)")
    }
}
impl From<RunOptions> for InitialOptionsV1 {
    fn from(v: RunOptions) -> Self {
        Self(InitialOptionsDataV1::from(v))
    }
}
impl From<RunOptions> for InitialOptionsDataV1 {
    fn from(v: RunOptions) -> Self {
        Self {
            project: v.project,
            config_dir: v.config_dir,
            gateway: v.gateway,
            runtime: v.runtime,
            runtime_program: v.runtime_program,
            runtime_endpoint: v.runtime_endpoint,
            image: v.image,
            profile: v.profile,
            network: v.network,
            allow_rootful_runtime: v.allow_rootful_runtime,
            timeout: v.timeout,
            provider: v.provider,
            client: v.client,
            auth_mode: v.auth_mode,
            client_profile: v.client_profile,
            providers_file: v.providers_file,
            prompt: v.prompt,
            model: v.model,
            approve_hash: v.approve_hash,
            capture_output: v.capture_output,
            interactive: v.interactive,
            secret_env_file: v.secret_env_file,
            argv: v.argv,
        }
    }
}
impl InitialOptionsV1 {
    pub(crate) fn run_options(&self) -> RunOptions {
        self.0.run_options()
    }
    fn managed_selection(&self) -> bool {
        self.0.managed_selection()
    }
    fn validate(&self, before_resolution: bool) -> IntentResult<()> {
        self.0.validate(before_resolution)
    }
}
impl InitialOptionsDataV1 {
    fn run_options(&self) -> RunOptions {
        RunOptions {
            project: self.project.clone(),
            config_dir: self.config_dir.clone(),
            gateway: self.gateway.clone(),
            runtime: self.runtime,
            runtime_program: self.runtime_program.clone(),
            runtime_endpoint: self.runtime_endpoint.clone(),
            image: self.image.clone(),
            profile: self.profile.clone(),
            network: self.network,
            allow_rootful_runtime: self.allow_rootful_runtime,
            timeout: self.timeout,
            provider: self.provider.clone(),
            client: self.client.clone(),
            auth_mode: self.auth_mode,
            client_profile: self.client_profile.clone(),
            providers_file: self.providers_file.clone(),
            prompt: self.prompt.clone(),
            model: self.model.clone(),
            approve_hash: self.approve_hash.clone(),
            capture_output: self.capture_output,
            interactive: self.interactive,
            secret_env_file: self.secret_env_file.clone(),
            argv: self.argv.clone(),
        }
    }
    fn managed_selection(&self) -> bool {
        self.client.is_some()
            || self.auth_mode.is_some()
            || self.client_profile.is_some()
            || self.providers_file.is_some()
            || self.prompt.is_some()
            || self.model.is_some()
    }
    fn validate(&self, before_resolution: bool) -> IntentResult<()> {
        for path in [&self.project, &self.config_dir].into_iter().chain(
            [
                &self.runtime_program,
                &self.providers_file,
                &self.secret_env_file,
            ]
            .into_iter()
            .flatten(),
        ) {
            text(path.to_str().ok_or(IntentError::InvalidIntent)?, 4096)?;
        }
        text(&self.gateway, 4096)?;
        let origin = reqwest::Url::parse(&self.gateway).map_err(|_| IntentError::InvalidIntent)?;
        let loopback = origin.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        require(
            (origin.scheme() == "https" || origin.scheme() == "http" && loopback)
                && origin.host_str().is_some()
                && origin.username().is_empty()
                && origin.password().is_none()
                && origin.path() == "/"
                && origin.query().is_none()
                && origin.fragment().is_none(),
        )?;
        crate::runtime::validate_image(&self.image).map_err(|_| IntentError::InvalidIntent)?;
        require(
            (1..=86400).contains(&self.timeout)
                && ["strict", "review", "observe"].contains(&self.profile.as_str())
                && (self.network == NetworkMode::None || self.profile == "observe")
                && ["generic", "codex", "claude", "claude-code"].contains(&self.provider.as_str())
                && !(self.interactive && self.capture_output),
        )?;
        if self.approve_hash.is_some() {
            return Err(IntentError::NonInitial);
        }
        if let Some(endpoint) = &self.runtime_endpoint {
            text(endpoint, 4096)?;
        }
        if let Some(client) = &self.client {
            require(["auto", "codex", "claude-code"].contains(&client.as_str()))?;
        }
        if let Some(profile) = &self.client_profile {
            require(
                !profile.is_empty()
                    && profile.len() <= 80
                    && profile
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
            )?;
        }
        if let Some(model) = &self.model {
            require(
                !model.is_empty()
                    && model.len() <= 128
                    && !model.starts_with('-')
                    && model
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_./:-".contains(&b)),
            )?;
        }
        if let Some(prompt) = &self.prompt {
            text(prompt, 24 * 1024)?;
        }
        require(
            self.secret_env_file.is_none()
                || self.profile == "observe" && self.network == NetworkMode::Bridge,
        )?;
        if before_resolution {
            require(
                self.argv.is_empty()
                    && self.provider == "generic"
                    && self.secret_env_file.is_none(),
            )?;
        } else {
            crate::runtime::validate_command(&self.argv).map_err(|_| IntentError::InvalidIntent)?;
        }
        Ok(())
    }
}

#[derive(Clone, Serialize)]
#[serde(transparent)]
pub struct InitialRequestV1(DecisionRequest);
impl fmt::Debug for InitialRequestV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("InitialRequestV1(<private>)")
    }
}
impl<'de> Deserialize<'de> for InitialRequestV1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        DecisionRequest::deserialize(d)
            .map(Self)
            .map_err(|_| D::Error::custom("invalid initial execution request"))
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderCredentialRefDataV1 {
    binding_id: Uuid,
    file_name: String,
    digest: String,
}
#[derive(Clone, Serialize)]
#[serde(transparent)]
pub struct ProviderCredentialRefV1(ProviderCredentialRefDataV1);
impl<'de> Deserialize<'de> for ProviderCredentialRefV1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        ProviderCredentialRefDataV1::deserialize(d)
            .map(Self)
            .map_err(|_| D::Error::custom("invalid execution credential reference"))
    }
}
impl fmt::Debug for ProviderCredentialRefV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProviderCredentialRefV1(<private>)")
    }
}
impl ProviderCredentialRefV1 {
    pub fn binding_id(&self) -> Uuid {
        self.0.binding_id
    }
    pub fn file_name(&self) -> &str {
        &self.0.file_name
    }
    pub fn digest(&self) -> &str {
        &self.0.digest
    }
    fn validate(&self) -> IntentResult<()> {
        require(
            !self.0.binding_id.is_nil()
                && self.0.file_name == format!("{}.json", self.0.binding_id)
                && self.0.digest.len() == 64
                && self
                    .0
                    .digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        )
    }
}

#[derive(Clone, Serialize)]
pub struct ExecutionIntentV1 {
    version: u32,
    plan_version: u32,
    operation_id: Uuid,
    session: Uuid,
    initial_options: InitialOptionsV1,
    initial_request: InitialRequestV1,
    provider_input: Option<InitialOptionsV1>,
    provider_credential: Option<ProviderCredentialRefV1>,
}
impl fmt::Debug for ExecutionIntentV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ExecutionIntentV1(<private>)")
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentWire {
    version: u32,
    plan_version: u32,
    operation_id: Uuid,
    session: Uuid,
    initial_options: InitialOptionsV1,
    initial_request: InitialRequestV1,
    provider_input: Option<InitialOptionsV1>,
    provider_credential: Option<ProviderCredentialRefV1>,
}
impl<'de> Deserialize<'de> for ExecutionIntentV1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Typed decoding rejects duplicate/unknown keys before canonical Value conversion.
        let v =
            IntentWire::deserialize(d).map_err(|_| D::Error::custom("invalid execution intent"))?;
        let intent = Self {
            version: v.version,
            plan_version: v.plan_version,
            operation_id: v.operation_id,
            session: v.session,
            initial_options: v.initial_options,
            initial_request: v.initial_request,
            provider_input: v.provider_input,
            provider_credential: v.provider_credential,
        };
        intent
            .validate()
            .map_err(|_| D::Error::custom("invalid execution intent"))?;
        Ok(intent)
    }
}
impl ExecutionIntentV1 {
    pub fn from_plan_bytes(bytes: &[u8]) -> IntentResult<Self> {
        if bytes.len() > MAX_PLAN_BYTES {
            return Err(IntentError::SourceTooLarge);
        }
        crate::runner::execution_intent_from_plan_bytes(bytes)
    }
    pub fn from_bytes(bytes: &[u8]) -> IntentResult<Self> {
        if bytes.len() > MAX_INTENT_BYTES {
            return Err(IntentError::IntentTooLarge);
        }
        serde_json::from_slice(bytes).map_err(|_| IntentError::InvalidIntent)
    }
    pub(crate) fn from_projection(
        plan_version: u32,
        operation_id: Uuid,
        session: Uuid,
        options: RunOptions,
        request: DecisionRequest,
        provider_input: Option<RunOptions>,
        credential: Option<(Uuid, String, String)>,
    ) -> IntentResult<Self> {
        let intent = Self {
            version: 1,
            plan_version,
            operation_id,
            session,
            initial_options: options.into(),
            initial_request: InitialRequestV1(request),
            provider_input: provider_input.map(Into::into),
            provider_credential: credential.map(|(binding_id, file_name, digest)| {
                ProviderCredentialRefV1(ProviderCredentialRefDataV1 {
                    binding_id,
                    file_name,
                    digest,
                })
            }),
        };
        intent.validate()?;
        Ok(intent)
    }
    pub fn validate(&self) -> IntentResult<()> {
        self.validated_bytes().map(|_| ())
    }
    pub fn canonical_bytes(&self) -> IntentResult<Vec<u8>> {
        self.validated_bytes()
    }
    pub fn intent_hash(&self) -> IntentResult<String> {
        let bytes = self.validated_bytes()?;
        let mut hash = Sha256::new();
        hash.update(DOMAIN);
        hash.update(bytes);
        Ok(hex::encode(hash.finalize()))
    }
    pub fn version(&self) -> u32 {
        self.version
    }
    pub fn plan_version(&self) -> u32 {
        self.plan_version
    }
    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }
    pub fn session(&self) -> Uuid {
        self.session
    }
    pub fn initial_options(&self) -> &InitialOptionsV1 {
        &self.initial_options
    }
    pub fn initial_request(&self) -> &InitialRequestV1 {
        &self.initial_request
    }
    pub fn provider_input(&self) -> Option<&InitialOptionsV1> {
        self.provider_input.as_ref()
    }
    pub fn provider_credential(&self) -> Option<&ProviderCredentialRefV1> {
        self.provider_credential.as_ref()
    }
    pub(crate) fn request_data(&self) -> &DecisionRequest {
        &self.initial_request.0
    }

    fn validated_bytes(&self) -> IntentResult<Vec<u8>> {
        self.validate_shape()?;
        let value = serde_json::to_value(self).map_err(|_| IntentError::InvalidIntent)?;
        let bytes = serde_json::to_vec(&ordered(value)).map_err(|_| IntentError::InvalidIntent)?;
        if bytes.len() > MAX_INTENT_BYTES {
            return Err(IntentError::IntentTooLarge);
        }
        Ok(bytes)
    }
    fn validate_shape(&self) -> IntentResult<()> {
        if self.version != 1 || !(1..=3).contains(&self.plan_version) {
            return Err(IntentError::UnsupportedVersion);
        }
        require(!self.operation_id.is_nil() && !self.session.is_nil())?;
        self.initial_options.validate(false)?;
        let request = &self.initial_request.0;
        require(Uuid::parse_str(&request.request_id).is_ok_and(|id| !id.is_nil()))?;
        if request.human_approved || request.approval_hash.is_some() {
            return Err(IntentError::NonInitial);
        }
        for field in [
            &request.project_id,
            &request.sandbox_id,
            &request.agent_id,
            &request.session_id,
            &request.correlation_id,
            &request.trace_id,
        ]
        .into_iter()
        .flatten()
        {
            require(field.len() <= 256 && !field.chars().any(char::is_control))?;
        }
        if let Some(session) = &request.session_id {
            require(Uuid::parse_str(session).is_ok_and(|id| id == self.session))?;
        }
        require(
            request.profile == self.initial_options.0.profile
                && request.command.argv == self.initial_options.0.argv
                && request.command.cwd == "/workspace"
                && request.execution_mode.is_some() == self.initial_options.0.interactive
                && crate::policy::valid_provider_context(request),
        )?;
        if let Some(credential) = &self.provider_credential {
            credential.validate()?;
        }
        crate::runner::validate_execution_intent_provider(self)?;
        if self.plan_version != 3 {
            require(
                !self.initial_options.managed_selection()
                    && self.provider_input.is_none()
                    && self.provider_credential.is_none(),
            )?;
            return Ok(());
        }
        let input = self
            .provider_input
            .as_ref()
            .ok_or(IntentError::Incoherent)?;
        input.validate(true)?;
        require(input.managed_selection())?;
        let mut resolved = input.clone();
        resolved.0.argv = self.initial_options.0.argv.clone();
        resolved.0.provider = self.initial_options.0.provider.clone();
        require(resolved == self.initial_options)?;
        let resolution = request
            .provider_resolution
            .as_ref()
            .ok_or(IntentError::Incoherent)?;
        if let Some(client) = input.0.client.as_deref().filter(|value| *value != "auto") {
            require(match resolution.client {
                ClientKind::Codex => client == "codex",
                ClientKind::ClaudeCode => client == "claude-code",
            })?;
        }
        if let Some(auth) = input
            .0
            .auth_mode
            .filter(|value| *value != AuthPreference::Auto)
        {
            require(auth == AuthPreference::from(resolution.auth_mode))?;
        }
        if let Some(profile) = &input.0.client_profile {
            require(profile == &resolution.profile_id)?;
        }
        if let Some(model) = &input.0.model {
            require(Some(model) == resolution.model.as_ref())?;
        }
        crate::provider::validate_frozen_prompt(
            &self.initial_options.0.argv,
            input.0.prompt.as_deref(),
        )
        .map_err(|_| IntentError::Incoherent)?;
        Ok(())
    }
}
fn require(valid: bool) -> IntentResult<()> {
    if valid {
        Ok(())
    } else {
        Err(IntentError::Incoherent)
    }
}
fn text(value: &str, max: usize) -> IntentResult<()> {
    require(!value.is_empty() && value.len() <= max && !value.contains('\0'))
}
fn ordered(value: Value) -> Value {
    match value {
        Value::Object(fields) => {
            let fields = fields
                .into_iter()
                .map(|(key, value)| (key, ordered(value)))
                .collect::<std::collections::BTreeMap<_, _>>();
            Value::Object(fields.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(ordered).collect()),
        other => other,
    }
}
