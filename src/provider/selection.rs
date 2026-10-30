use super::*;
use anyhow::{Context, anyhow, bail};
use std::collections::BTreeSet;

pub(super) fn validate_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 80
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
        "invalid provider profile identifier"
    );
    Ok(())
}

pub(super) fn validate_model(model: Option<&str>) -> Result<()> {
    if let Some(model) = model {
        ensure!(
            !model.is_empty()
                && model.len() <= 128
                && model
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_./:-".contains(&c))
                && !model.starts_with('-'),
            "invalid provider model identifier"
        );
    }
    Ok(())
}

pub(super) fn validate_config(config: &ProviderConfigV1) -> Result<()> {
    ensure!(
        config.version == 1 && config.profiles.len() <= 64 && config.auto_priority.len() <= 64,
        "invalid provider configuration version or size"
    );
    let mut ids = BTreeSet::new();
    for profile in &config.profiles {
        validate_id(&profile.id)?;
        validate_model(profile.model.as_deref())?;
        ensure!(
            profile.revision > 0 && ids.insert(profile.id.as_str()),
            "ambiguous_configuration: duplicate or invalid provider profile"
        );
        let path = match &profile.credential {
            CredentialSourceV1::ApiEnvFile { path, key } => {
                ensure!(
                    profile.auth_mode == AuthMode::Api,
                    "invalid credential source for provider authentication mode"
                );
                if let Some(key) = key {
                    let allowed: &[&str] = match profile.client {
                        ClientKind::Codex => &["OPENAI_API_KEY", "CODEX_API_KEY"],
                        ClientKind::ClaudeCode => &["ANTHROPIC_API_KEY"],
                    };
                    ensure!(
                        allowed.contains(&key.as_str()),
                        "invalid credential variable for provider"
                    );
                }
                path
            }
            CredentialSourceV1::CodexAuthFile { path } => {
                ensure!(
                    profile.client == ClientKind::Codex
                        && profile.auth_mode == AuthMode::Subscription,
                    "invalid credential source for provider authentication mode"
                );
                path
            }
            CredentialSourceV1::ClaudeOAuthEnvFile { path } => {
                ensure!(
                    profile.client == ClientKind::ClaudeCode
                        && profile.auth_mode == AuthMode::Subscription,
                    "invalid credential source for provider authentication mode"
                );
                path
            }
        };
        ensure!(
            path.is_absolute(),
            "credential source requires an absolute path"
        );
        let _ = crate::control_fs::absolute_path(path)?;
    }
    let mut priority = BTreeSet::new();
    for id in &config.auto_priority {
        ensure!(
            ids.contains(id.as_str()) && priority.insert(id),
            "ambiguous_configuration: unknown or duplicate provider priority"
        );
    }
    if let Some(id) = &config.default_profile {
        ensure!(
            ids.contains(id.as_str()),
            "ambiguous_configuration: default provider profile does not exist"
        );
    }
    Ok(())
}

pub(super) fn load_config(path: &Path) -> Result<ProviderConfigV1> {
    let path = crate::control_fs::absolute_path(path)?;
    let file = PrivateFile::open(&path, 64 * 1024).map_err(|_| {
        anyhow!("credential_unsafe: provider configuration is unavailable or not private")
    })?;
    load_config_source(&file)
}

pub(super) fn load_config_source(file: &PrivateFile) -> Result<ProviderConfigV1> {
    file.revalidate()
        .context("provider configuration changed before preparation")?;
    let mut config: ProviderConfigV1 = serde_json::from_slice(file.bytes())
        .map_err(|_| anyhow!("invalid provider configuration JSON"))?;
    let parent = file
        .path()
        .parent()
        .context("provider configuration parent is unavailable")?;
    for profile in &mut config.profiles {
        let path = match &mut profile.credential {
            CredentialSourceV1::ApiEnvFile { path, .. }
            | CredentialSourceV1::CodexAuthFile { path }
            | CredentialSourceV1::ClaudeOAuthEnvFile { path } => path,
        };
        if path.is_relative() {
            ensure!(
                !path.as_os_str().is_empty()
                    && path
                        .components()
                        .all(|part| matches!(part, std::path::Component::Normal(_))),
                "credential source must not contain traversal or special path components"
            );
            *path = parent.join(&*path);
        }
        *path = crate::control_fs::absolute_path(path)?;
    }
    config.validate()?;
    file.revalidate()
        .context("provider configuration changed during preparation")?;
    Ok(config)
}

fn matches(profile: &ProviderProfileV1, selection: &ProviderSelection) -> bool {
    selection
        .client
        .is_none_or(|client| client == profile.client)
        && match selection.auth_mode {
            AuthPreference::Auto => true,
            AuthPreference::Api => profile.auth_mode == AuthMode::Api,
            AuthPreference::Subscription => profile.auth_mode == AuthMode::Subscription,
        }
}

fn choose<'a>(
    config: &'a ProviderConfigV1,
    selection: &ProviderSelection,
) -> Result<(&'a ProviderProfileV1, ChoiceSource)> {
    if let Some(id) = &selection.profile_id {
        validate_id(id)?;
        let profile = config
            .profiles
            .iter()
            .find(|profile| profile.id == *id)
            .context("credential_missing: requested provider profile is not configured")?;
        ensure!(
            matches(profile, selection),
            "ambiguous_configuration: explicit client or authentication conflicts with the selected profile"
        );
        return Ok((profile, ChoiceSource::Profile));
    }
    let mut candidates: Vec<_> = config
        .profiles
        .iter()
        .filter(|profile| matches(profile, selection))
        .collect();
    if selection.auth_mode == AuthPreference::Auto
        && candidates
            .iter()
            .any(|profile| profile.auth_mode == AuthMode::Subscription)
    {
        candidates.retain(|profile| profile.auth_mode == AuthMode::Subscription);
    }
    if let Some(id) = &config.default_profile
        && let Some(profile) = candidates.iter().find(|profile| profile.id == *id)
    {
        return Ok((profile, ChoiceSource::DefaultProfile));
    }
    if candidates.len() == 1 {
        return Ok((candidates[0], ChoiceSource::Auto));
    }
    for id in &config.auto_priority {
        if let Some(profile) = candidates.iter().find(|profile| profile.id == *id) {
            return Ok((profile, ChoiceSource::Auto));
        }
    }
    if candidates.is_empty() {
        bail!(
            "credential_missing: no configured profile matches the requested client and authentication mode"
        );
    }
    bail!("ambiguous_configuration: several provider profiles match without an explicit priority")
}

pub(super) fn prepare(
    config: &ProviderConfigV1,
    selection: &ProviderSelection,
    project: &Path,
    prompt: Option<&str>,
    binding_id: Uuid,
) -> Result<PreparedProvider> {
    config.validate()?;
    validate_model(selection.model.as_deref())?;
    let (profile, source) = choose(config, selection)?;
    let resolution = ProviderResolutionV1 {
        version: 1,
        client: profile.client,
        auth_mode: profile.auth_mode,
        transport: selection.transport,
        profile_id: profile.id.clone(),
        profile_revision: profile.revision,
        model: selection.model.clone().or_else(|| profile.model.clone()),
        credential_binding_id: binding_id,
        credential_names: vec![BOOTSTRAP_ENV.into()],
        invocation_preset_version: 1,
        provenance: ProviderChoiceProvenanceV1 {
            client: if selection.client.is_some() {
                ChoiceSource::Cli
            } else {
                source
            },
            auth_mode: if selection.auth_mode != AuthPreference::Auto {
                ChoiceSource::Cli
            } else {
                source
            },
            profile: source,
            model: if selection.model.is_some() {
                ChoiceSource::Cli
            } else {
                ChoiceSource::Profile
            },
        },
    };
    resolution.validate()?;
    let argv = bootstrap_argv(&resolution, prompt)?;
    let credential = credentials::prepare(profile, project)?;
    Ok(PreparedProvider {
        resolution,
        argv,
        credential,
    })
}

impl ClientKind {
    pub(super) fn argument(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
        }
    }
}
impl AuthMode {
    pub(super) fn argument(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Subscription => "subscription",
        }
    }
}
impl ClientTransport {
    pub(super) fn argument(self) -> &'static str {
        match self {
            Self::Batch => "batch",
            Self::InteractiveTty => "interactive-tty",
        }
    }
}

pub(super) struct BootstrapArguments {
    pub client: ClientKind,
    pub auth_mode: AuthMode,
    pub transport: ClientTransport,
    pub model: Option<String>,
    pub prompt: Option<String>,
}

fn validate_prompt(prompt: Option<&str>, transport: ClientTransport) -> Result<()> {
    ensure!(
        transport.is_interactive() || prompt.is_some(),
        "batch provider invocation requires a prompt"
    );
    if let Some(prompt) = prompt {
        ensure!(
            !prompt.is_empty() && prompt.len() <= 24 * 1024 && !prompt.contains('\0'),
            "provider prompt is empty, contains NUL or exceeds its bound"
        );
    }
    Ok(())
}

fn bootstrap_argv(resolution: &ProviderResolutionV1, prompt: Option<&str>) -> Result<Vec<String>> {
    validate_prompt(prompt, resolution.transport)?;
    let mut argv: Vec<String> = [
        BOOTSTRAP_PROGRAM,
        "exec",
        "--client",
        resolution.client.argument(),
        "--auth-mode",
        resolution.auth_mode.argument(),
        "--transport",
        resolution.transport.argument(),
        "--preset-version",
        "1",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if let Some(model) = &resolution.model {
        argv.extend(["--model".into(), model.clone()]);
    }
    if let Some(prompt) = prompt {
        argv.extend(["--prompt".into(), prompt.into()]);
    }
    Ok(argv)
}

pub(super) fn parse_bootstrap_argv(argv: &[String]) -> Result<BootstrapArguments> {
    ensure!(
        argv.len() >= 10
            && argv[0] == BOOTSTRAP_PROGRAM
            && argv[1] == "exec"
            && argv[2] == "--client"
            && argv[4] == "--auth-mode"
            && argv[6] == "--transport"
            && argv[8] == "--preset-version"
            && argv[9] == "1",
        "invalid provider bootstrap invocation"
    );
    let client = match argv[3].as_str() {
        "codex" => ClientKind::Codex,
        "claude-code" => ClientKind::ClaudeCode,
        _ => bail!("invalid provider bootstrap client"),
    };
    let auth_mode = match argv[5].as_str() {
        "api" => AuthMode::Api,
        "subscription" => AuthMode::Subscription,
        _ => bail!("invalid provider bootstrap authentication mode"),
    };
    let transport = match argv[7].as_str() {
        "batch" => ClientTransport::Batch,
        "interactive-tty" => ClientTransport::InteractiveTty,
        _ => bail!("invalid provider bootstrap transport"),
    };
    let mut at = 10;
    let model = if argv.get(at).is_some_and(|value| value == "--model") {
        at += 2;
        Some(
            argv.get(at - 1)
                .context("provider model argument is missing")?
                .clone(),
        )
    } else {
        None
    };
    let prompt = if argv.get(at).is_some_and(|value| value == "--prompt") {
        at += 2;
        Some(
            argv.get(at - 1)
                .context("provider prompt argument is missing")?
                .clone(),
        )
    } else {
        None
    };
    ensure!(
        at == argv.len(),
        "invalid trailing provider bootstrap arguments"
    );
    validate_model(model.as_deref())?;
    validate_prompt(prompt.as_deref(), transport)?;
    Ok(BootstrapArguments {
        client,
        auth_mode,
        transport,
        model,
        prompt,
    })
}
