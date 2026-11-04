//! Authenticated presence of the trusted service, independent of job admission.
//! Local receipts are immutable recovery evidence, never execution authority.
use crate::{
    broker_registry::{BrokerAuthorityV1, BrokerRuntimeKindV1},
    control_fs::{Directory, PrivateFile},
    redaction::Redactor,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fmt,
    net::IpAddr,
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{Semaphore, watch};
use uuid::Uuid;

mod actor;
#[cfg(test)]
mod hash_tests;
mod http;
#[cfg(test)]
mod http_tests;
#[cfg(test)]
mod preparation_tests;
mod storage;

pub(crate) type Result<T> = std::result::Result<T, PresenceError>;
const RECORD_BYTES: u64 = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PresenceError {
    InvalidConfig,
    UnsafeFilesystem,
    UnsafeState,
    Capacity,
    Authentication,
    Forbidden,
    Conflict,
    Clock,
    Protocol,
    Transport,
    Unavailable,
    Stopped,
}
impl PresenceError {
    pub(crate) fn fences_admission(self) -> bool {
        matches!(self, Self::InvalidConfig | Self::UnsafeFilesystem)
    }
}
impl fmt::Display for PresenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidConfig => "broker presence configuration is invalid or unsafe",
            Self::UnsafeFilesystem => "broker presence private material changed or is unavailable",
            Self::UnsafeState => "broker presence evidence requires operator reconciliation",
            Self::Capacity => {
                "broker presence retained capacity is exhausted; contact the operator"
            }
            Self::Authentication => "broker presence authentication was refused",
            Self::Forbidden => "broker presence scope or grant was refused",
            Self::Conflict => "broker presence revisions require operator reconciliation",
            Self::Clock => "broker presence control clock requires operator reconciliation",
            Self::Protocol => "broker presence response failed validation",
            Self::Transport => "broker presence transport attempts were exhausted",
            Self::Unavailable => "broker presence is unavailable",
            Self::Stopped => "broker presence was stopped",
        })
    }
}
impl std::error::Error for PresenceError {}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PresenceScope {
    pub installation_id: Uuid,
    pub profile_id: String,
    pub gateway: String,
    pub expected_tenant_id: String,
    pub control_namespace_uuid: Uuid,
    pub runtime_kind: BrokerRuntimeKindV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PresenceConfigV1 {
    version: u32,
    broker_id: String,
    credential_id: String,
    incarnation_id: String,
    authority_id: String,
    broker_token_file: PathBuf,
    #[serde(default)]
    ca_bundle_file: Option<PathBuf>,
    state_dir: PathBuf,
    #[serde(default = "interval_default")]
    interval_seconds: u32,
    #[serde(default = "operations_default")]
    max_operations: u32,
    #[serde(default = "bytes_default")]
    max_state_bytes: u64,
}
fn interval_default() -> u32 {
    30
}
fn operations_default() -> u32 {
    8192
}
fn bytes_default() -> u64 {
    64 * 1024 * 1024
}

pub(crate) struct PreparedPresence {
    scope: PresenceScope,
    config: PresenceConfigV1,
    files: Vec<PrivateFile>,
    roots: Vec<Directory>,
    state: Directory,
    token: String,
    secrets: Vec<String>,
    ca: Option<PrivateFile>,
    stream: Option<storage::Stream>,
}
impl fmt::Debug for PreparedPresence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PreparedPresence(<private>)")
    }
}
impl PreparedPresence {
    /// Strictly read-only. In particular, no lock, anchor or state directory is
    /// created until every profile's protected material has been admitted.
    pub(crate) fn load(
        path: &Path,
        mut scope: PresenceScope,
        known_secrets: Vec<String>,
    ) -> Result<Self> {
        normalized(path)?;
        let source =
            PrivateFile::open(path, RECORD_BYTES).map_err(|_| PresenceError::InvalidConfig)?;
        let config: PresenceConfigV1 =
            serde_json::from_slice(source.bytes()).map_err(|_| PresenceError::InvalidConfig)?;
        if config.version != 1
            || !(5..=150).contains(&config.interval_seconds)
            || !(1..=32768).contains(&config.max_operations)
            || config.max_state_bytes == 0
            || config.max_state_bytes > 128 * 1024 * 1024
        {
            return Err(PresenceError::InvalidConfig);
        }
        for id in [
            &config.broker_id,
            &config.credential_id,
            &config.incarnation_id,
        ] {
            uuid(id)?;
        }
        BrokerAuthorityV1 {
            authority_id: config.authority_id.clone(),
            runtime_kind: scope.runtime_kind,
        }
        .validate()
        .map_err(|_| PresenceError::InvalidConfig)?;
        if scope.installation_id.is_nil()
            || scope.control_namespace_uuid.is_nil()
            || scope.profile_id.is_empty()
            || scope.profile_id.len() > 80
            || !scope
                .profile_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Err(PresenceError::InvalidConfig);
        }
        crate::tenancy::validate_tenant_id(&scope.expected_tenant_id)
            .map_err(|_| PresenceError::InvalidConfig)?;
        let origin = origin(&scope.gateway)?;
        scope.gateway = origin.as_str().trim_end_matches('/').into();
        normalized(&config.broker_token_file)?;
        normalized(&config.state_dir)?;
        let token_file = PrivateFile::open(&config.broker_token_file, 4097)
            .map_err(|_| PresenceError::InvalidConfig)?;
        let token =
            std::str::from_utf8(token_file.bytes()).map_err(|_| PresenceError::InvalidConfig)?;
        let token = token.strip_suffix('\n').unwrap_or(token);
        if !(32..=4096).contains(&token.len())
            || !token.is_ascii()
            || token
                .bytes()
                .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        {
            return Err(PresenceError::InvalidConfig);
        }
        let token = token.to_owned();
        let ca = config
            .ca_bundle_file
            .as_ref()
            .map(|path| {
                normalized(path)?;
                if origin.scheme() != "https" {
                    return Err(PresenceError::InvalidConfig);
                }
                let file =
                    PrivateFile::open(path, 65_536).map_err(|_| PresenceError::InvalidConfig)?;
                http::certificates(&file)?;
                Ok(file)
            })
            .transpose()?;
        let state = Directory::open_existing(&config.state_dir)
            .map_err(|_| PresenceError::InvalidConfig)?;
        let mut files = vec![source, token_file];
        if let Some(ca) = &ca {
            files.push(ca.clone());
        }
        let mut roots = vec![state.clone()];
        for file in &files {
            let parent = file.path().parent().ok_or(PresenceError::InvalidConfig)?;
            if !roots.iter().any(|root| root.path() == parent) {
                roots.push(
                    Directory::open_existing(parent).map_err(|_| PresenceError::InvalidConfig)?,
                );
            }
        }
        let mut prepared = Self {
            scope,
            config,
            files,
            roots,
            state,
            token,
            secrets: known_secrets,
            ca,
            stream: None,
        };
        prepared.secrets.push(prepared.token.clone());
        prepared.screen_material()?;
        prepared.revalidate()?;
        Ok(prepared)
    }
    pub(crate) fn protected_files(&self) -> Vec<PrivateFile> {
        self.files.clone()
    }
    pub(crate) fn protected_roots(&self) -> Vec<Directory> {
        self.roots.clone()
    }
    pub(crate) fn secret_literals(&self) -> Vec<String> {
        vec![self.token.clone()]
    }
    pub(crate) fn finalize(&mut self, values: &[String]) -> Result<()> {
        if self.stream.is_some() {
            return Err(PresenceError::UnsafeState);
        }
        self.secrets.extend_from_slice(values);
        self.secrets.sort();
        self.secrets.dedup();
        if self.secrets.len() > 64 || self.secrets.iter().any(|s| s.is_empty() || s.len() > 4096) {
            return Err(PresenceError::InvalidConfig);
        }
        self.screen_material()
            .map_err(|_| PresenceError::InvalidConfig)?;
        self.revalidate()?;
        self.stream = Some(storage::Stream::open(self)?);
        Ok(())
    }
    pub(crate) fn revalidate(&self) -> Result<()> {
        for file in &self.files {
            file.revalidate()
                .map_err(|_| PresenceError::UnsafeFilesystem)?;
        }
        for root in &self.roots {
            root.revalidate()
                .map_err(|_| PresenceError::UnsafeFilesystem)?;
        }
        Ok(())
    }
    fn screen_material(&self) -> Result<()> {
        let source = serde_json::from_slice::<Value>(self.files[0].bytes())
            .map_err(|_| PresenceError::InvalidConfig)?;
        self.screen(&source)
            .map_err(|_| PresenceError::InvalidConfig)?;
        self.screen(&serde_json::to_value(&self.scope).map_err(|_| PresenceError::InvalidConfig)?)?;
        for file in &self.files {
            self.screen_text(file.path().to_str().ok_or(PresenceError::InvalidConfig)?)?;
        }
        for root in &self.roots {
            self.screen_text(root.path().to_str().ok_or(PresenceError::InvalidConfig)?)?;
        }
        Ok(())
    }
    fn screen_text(&self, text: &str) -> Result<()> {
        if self
            .secrets
            .iter()
            .any(|secret| !secret.is_empty() && text.contains(secret))
            || Redactor::new(self.secrets.clone()).text(text) != text
        {
            return Err(PresenceError::Protocol);
        }
        Ok(())
    }
    fn screen(&self, value: &Value) -> Result<()> {
        match value {
            Value::String(s) => self.screen_text(s),
            Value::Array(values) => values.iter().try_for_each(|v| self.screen(v)),
            Value::Object(values) => values.iter().try_for_each(|(key, v)| {
                self.screen_text(key)?;
                self.screen(v)
            }),
            _ => Ok(()),
        }
    }
    pub(crate) async fn run(
        self,
        shutdown: watch::Receiver<bool>,
        attempts: Arc<Semaphore>,
        io: Arc<Semaphore>,
    ) -> Result<()> {
        actor::run(self, shutdown, attempts, io).await
    }
}
fn uuid(value: &str) -> Result<()> {
    crate::broker_registry::registry_uuid(value).map_err(|_| PresenceError::InvalidConfig)
}
fn normalized(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return Err(PresenceError::InvalidConfig);
    }
    // Components drops redundant separators and '.', so also compare spelling.
    let rebuilt: PathBuf = path.components().collect();
    if rebuilt.as_os_str() != path.as_os_str() {
        return Err(PresenceError::InvalidConfig);
    }
    Ok(())
}
fn origin(value: &str) -> Result<reqwest::Url> {
    if value.len() > 2048 {
        return Err(PresenceError::InvalidConfig);
    }
    let url = reqwest::Url::parse(value).map_err(|_| PresenceError::InvalidConfig)?;
    let local = url.host_str().is_some_and(|host| {
        host.trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || !(url.scheme() == "https" || url.scheme() == "http" && local)
    {
        return Err(PresenceError::InvalidConfig);
    }
    Ok(url)
}
