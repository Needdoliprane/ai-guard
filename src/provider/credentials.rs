use super::*;
use anyhow::{anyhow, bail};
use serde_json::Value;
use std::{collections::BTreeMap, fs};

const MAX_CREDENTIAL_BYTES: u64 = 64 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BootstrapPayload {
    pub version: u32,
    pub client: ClientKind,
    pub auth_mode: AuthMode,
    pub secret: String,
}

pub(super) fn validate_secret(secret: &str) -> Result<()> {
    ensure!(
        secret.len() >= 8 && secret.len() <= 32 * 1024 && !secret.chars().any(char::is_control),
        "credential_missing: selected provider credential is invalid"
    );
    Ok(())
}

fn dotenv(bytes: &[u8]) -> Result<BTreeMap<&str, &str>> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| anyhow!("invalid provider credential encoding"))?;
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| anyhow!("provider credential file requires literal NAME=value lines"))?;
        ensure!(
            !key.is_empty()
                && key.len() <= 128
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
                && !key.as_bytes()[0].is_ascii_digit()
                && !value.chars().any(char::is_control)
                && values.insert(key, value).is_none(),
            "ambiguous_configuration: invalid or duplicate provider credential variable"
        );
    }
    Ok(values)
}

pub(super) fn codex_auth(text: &str) -> Result<(String, Vec<String>)> {
    let value: Value = serde_json::from_str(text)
        .map_err(|_| anyhow!("login_required: invalid Codex subscription credential JSON"))?;
    let fields = value
        .as_object()
        .ok_or_else(|| anyhow!("login_required: invalid Codex subscription credential object"))?;
    ensure!(
        fields.keys().all(
            |key| ["auth_mode", "OPENAI_API_KEY", "tokens", "last_refresh"].contains(&key.as_str())
        ) && fields.get("auth_mode").is_none_or(|mode| mode == "chatgpt")
            && fields.get("OPENAI_API_KEY").is_none_or(Value::is_null),
        "login_required: Codex subscription credential contains an incompatible authentication mode"
    );
    ensure!(
        fields
            .get("last_refresh")
            .is_none_or(|value| value.is_null()
                || value
                    .as_str()
                    .is_some_and(|text| chrono::DateTime::parse_from_rfc3339(text).is_ok())),
        "login_required: invalid Codex subscription refresh metadata"
    );
    let tokens = fields
        .get("tokens")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("login_required: Codex subscription tokens are missing"))?;
    ensure!(
        tokens.keys().all(
            |key| ["id_token", "access_token", "refresh_token", "account_id"]
                .contains(&key.as_str())
        ),
        "login_required: unsupported Codex subscription token structure"
    );
    let access = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("login_required: Codex subscription access token is missing"))?;
    validate_secret(access)
        .map_err(|_| anyhow!("login_required: invalid Codex subscription token"))?;
    let mut secrets = Vec::new();
    for value in tokens.values() {
        if let Some(secret) = value.as_str() {
            ensure!(
                !secret.is_empty() && !secret.chars().any(char::is_control),
                "login_required: invalid Codex subscription token"
            );
            secrets.push(secret.into());
        } else {
            ensure!(
                value.is_null(),
                "login_required: unsupported Codex subscription token value"
            );
        }
    }
    let compact = serde_json::to_string(&value)
        .map_err(|_| anyhow!("invalid Codex subscription credential"))?;
    ensure!(
        compact.len() <= 48 * 1024,
        "Codex subscription credential exceeds its bound"
    );
    Ok((compact, secrets))
}

pub(super) fn prepare(profile: &ProviderProfileV1, project: &Path) -> Result<ProviderCredential> {
    let path = match &profile.credential {
        CredentialSourceV1::ApiEnvFile { path, .. }
        | CredentialSourceV1::CodexAuthFile { path }
        | CredentialSourceV1::ClaudeOAuthEnvFile { path } => path,
    };
    let path = crate::control_fs::absolute_path(path)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            bail!(if profile.auth_mode == AuthMode::Subscription {
                "login_required: provider subscription credential is unavailable"
            } else {
                "credential_missing: provider API credential is unavailable"
            });
        }
        Err(_) => bail!("credential_unsafe: provider credential metadata is inaccessible"),
        Ok(_) => {}
    }
    let source = PrivateFile::open(&path, MAX_CREDENTIAL_BYTES).map_err(|_| {
        anyhow!("credential_unsafe: provider credential must be a private owned regular file")
    })?;
    let project = crate::control_fs::absolute_path(project)?;
    if source.is_within_directory(&project)? {
        ensure!(
            source
                .path()
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    let name = name.to_ascii_lowercase();
                    name == ".env" || name.starts_with(".env.")
                }),
            "credential_unsafe: provider credentials inside the project must use an explicitly excluded dotenv filename"
        );
    }
    let (secret, mut secrets) = match &profile.credential {
        CredentialSourceV1::CodexAuthFile { .. } => {
            codex_auth(std::str::from_utf8(source.bytes()).map_err(|_| {
                anyhow!("login_required: invalid Codex subscription credential encoding")
            })?)?
        }
        CredentialSourceV1::ApiEnvFile { key, .. } => {
            let values = dotenv(source.bytes())?;
            let names: &[&str] = match profile.client {
                ClientKind::Codex => &["CODEX_API_KEY", "OPENAI_API_KEY"],
                ClientKind::ClaudeCode => &["ANTHROPIC_API_KEY"],
            };
            let chosen = match key {
                Some(key) => values.get(key.as_str()).copied(),
                None => {
                    let available: Vec<_> = names
                        .iter()
                        .filter_map(|key| values.get(key).copied())
                        .collect();
                    ensure!(
                        available.len() <= 1,
                        "ambiguous_configuration: select one Codex API credential alias explicitly"
                    );
                    available.first().copied()
                }
            }
            .ok_or_else(|| {
                anyhow!("credential_missing: selected provider API credential is absent")
            })?;
            validate_secret(chosen)?;
            (chosen.into(), vec![chosen.into()])
        }
        CredentialSourceV1::ClaudeOAuthEnvFile { .. } => {
            let values = dotenv(source.bytes())?;
            let chosen = values
                .get("CLAUDE_CODE_OAUTH_TOKEN")
                .ok_or_else(|| anyhow!("login_required: Claude subscription token is absent"))?;
            validate_secret(chosen)
                .map_err(|_| anyhow!("login_required: Claude subscription token is invalid"))?;
            ((*chosen).into(), vec![(*chosen).into()])
        }
    };
    let payload = BootstrapPayload {
        version: 1,
        client: profile.client,
        auth_mode: profile.auth_mode,
        secret,
    };
    let bootstrap = serde_json::to_string(&payload)
        .map_err(|_| anyhow!("provider bootstrap credential cannot be prepared"))?;
    ensure!(
        bootstrap.len() + BOOTSTRAP_ENV.len() + 2 <= MAX_CREDENTIAL_BYTES as usize,
        "provider bootstrap credential exceeds its bound"
    );
    secrets.push(bootstrap.clone());
    source.revalidate()?;
    Ok(ProviderCredential {
        source,
        bootstrap,
        secrets,
    })
}
