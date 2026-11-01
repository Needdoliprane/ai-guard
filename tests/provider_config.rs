#![cfg(unix)]

use agentsandbox::provider::{
    AuthMode, AuthPreference, ClientKind, ClientTransport, CredentialSourceV1, ProviderConfigV1,
    ProviderProfileV1, ProviderSelection, load_config, load_config_source, prepare,
};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};
use uuid::Uuid;

const CODEX_KEY: &str = "canary-codex-private-key-42";
const CLAUDE_KEY: &str = "canary-claude-private-key-43";
const OAUTH: &str = "canary-subscription-access-token-44";

fn private(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn project(root: &Path) -> std::path::PathBuf {
    let path = root.join("project");
    fs::create_dir_all(&path).unwrap();
    path
}

fn profile(id: &str, client: ClientKind, auth_mode: AuthMode, path: &Path) -> ProviderProfileV1 {
    ProviderProfileV1 {
        id: id.into(),
        revision: 1,
        client,
        auth_mode,
        model: None,
        credential: match (client, auth_mode) {
            (_, AuthMode::Api) => CredentialSourceV1::ApiEnvFile {
                path: path.into(),
                key: None,
            },
            (ClientKind::Codex, AuthMode::Subscription) => {
                CredentialSourceV1::CodexAuthFile { path: path.into() }
            }
            (ClientKind::ClaudeCode, AuthMode::Subscription) => {
                CredentialSourceV1::ClaudeOAuthEnvFile { path: path.into() }
            }
        },
    }
}

fn config(profiles: Vec<ProviderProfileV1>) -> ProviderConfigV1 {
    ProviderConfigV1 {
        version: 1,
        default_profile: None,
        auto_priority: Vec::new(),
        profiles,
    }
}

fn selection(client: Option<ClientKind>, auth_mode: AuthPreference) -> ProviderSelection {
    ProviderSelection {
        client,
        auth_mode,
        profile_id: None,
        transport: ClientTransport::Batch,
        model: None,
    }
}

#[test]
fn claude_oauth_source_accepts_documented_and_legacy_names_without_rewriting() {
    let documented = serde_json::json!({
        "kind": "claude_oauth_env_file", "path": "/not-opened/claude.env"
    });
    let legacy = serde_json::json!({
        "kind": "claude_o_auth_env_file", "path": "/not-opened/claude.env"
    });
    let parsed: CredentialSourceV1 = serde_json::from_value(documented.clone())
        .expect("the documented subscription credential name must be accepted");
    let previous: CredentialSourceV1 = serde_json::from_value(legacy)
        .expect("existing generated configurations must remain readable");
    assert_eq!(parsed, previous);
    assert_eq!(serde_json::to_value(parsed).unwrap(), documented);
    let root = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = root.path().join("providers.json");
    let legacy_config = serde_json::json!({
        "version": 1,
        "profiles": [{
            "id": "claude-subscription", "revision": 1,
            "client": "claude_code", "auth_mode": "subscription",
            "credential": {"kind": "claude_o_auth_env_file", "path": "/not-opened/claude.env"}
        }]
    })
    .to_string();
    private(&path, &legacy_config);
    let loaded =
        load_config(&path).expect("legacy source should load without accessing its credential");
    assert_eq!(loaded.profiles[0].credential, previous);
    assert_eq!(fs::read_to_string(&path).unwrap(), legacy_config);
    assert!(
        serde_json::from_value::<CredentialSourceV1>(serde_json::json!({
            "kind": "claude_oauth_env_file", "path": "/not-opened/claude.env", "unknown": true
        }))
        .is_err(),
        "accepting the documented name must preserve the closed schema"
    );
}

#[test]
fn all_eight_combinations_prepare_one_private_handoff_without_other_keys() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    fs::create_dir(&project).unwrap();
    let api = project.join(".env");
    private(
        &api,
        &format!("OPENAI_API_KEY={CODEX_KEY}\nANTHROPIC_API_KEY={CLAUDE_KEY}\n"),
    );
    let codex_auth = root.path().join("codex-auth.json");
    private(
        &codex_auth,
        &format!(
            r#"{{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{{"access_token":"{OAUTH}","refresh_token":"canary-refresh-token-45"}}}}"#
        ),
    );
    let claude_auth = root.path().join("claude-oauth.env");
    private(
        &claude_auth,
        &format!("CLAUDE_CODE_OAUTH_TOKEN={OAUTH}\nANTHROPIC_API_KEY={CLAUDE_KEY}\n"),
    );
    for client in [ClientKind::Codex, ClientKind::ClaudeCode] {
        for auth_mode in [AuthMode::Api, AuthMode::Subscription] {
            let source = match (client, auth_mode) {
                (_, AuthMode::Api) => &api,
                (ClientKind::Codex, _) => &codex_auth,
                (ClientKind::ClaudeCode, _) => &claude_auth,
            };
            for transport in [ClientTransport::Batch, ClientTransport::InteractiveTty] {
                let mut selected = selection(Some(client), auth_mode.into());
                selected.transport = transport;
                let binding = Uuid::new_v4();
                let prepared = prepare(
                    &config(vec![profile("chosen", client, auth_mode, source)]),
                    &selected,
                    &project,
                    Some("Read only the project summary"),
                    binding,
                )
                .unwrap();
                assert_eq!(prepared.resolution.credential_binding_id, binding);
                assert_eq!(
                    prepared.resolution.credential_names,
                    ["AI_GUARD_PROVIDER_BOOTSTRAP"]
                );
                prepared.resolution.validate().unwrap();
                prepared.resolution.validate_argv(&prepared.argv).unwrap();
                let public = serde_json::to_string(&prepared.resolution).unwrap();
                let expected = match (client, auth_mode) {
                    (ClientKind::Codex, AuthMode::Api) => CODEX_KEY,
                    (ClientKind::ClaudeCode, AuthMode::Api) => CLAUDE_KEY,
                    _ => OAUTH,
                };
                assert!(prepared.credential.bootstrap_value().contains(expected));
                for secret in [CODEX_KEY, CLAUDE_KEY, OAUTH] {
                    assert!(!public.contains(secret));
                    assert!(!format!("{prepared:?}").contains(secret));
                    assert!(!format!("{}", prepared.credential).contains(secret));
                    assert!(!prepared.argv.iter().any(|part| part.contains(secret)));
                    if secret != expected {
                        assert!(!prepared.credential.bootstrap_value().contains(secret));
                    }
                }
                assert!(
                    prepared
                        .credential
                        .canonical_env_bytes()
                        .starts_with(b"AI_GUARD_PROVIDER_BOOTSTRAP=")
                );
            }
        }
    }
}

#[test]
fn auto_prefers_configured_subscription_and_explicit_client_wins() {
    let root = tempfile::tempdir().unwrap();
    let api = root.path().join("api.env");
    let sub = root.path().join("oauth.env");
    private(
        &api,
        &format!("OPENAI_API_KEY={CODEX_KEY}\nANTHROPIC_API_KEY={CLAUDE_KEY}\n"),
    );
    private(&sub, &format!("CLAUDE_CODE_OAUTH_TOKEN={OAUTH}\n"));
    let cfg = config(vec![
        profile("api", ClientKind::Codex, AuthMode::Api, &api),
        profile(
            "subscription",
            ClientKind::ClaudeCode,
            AuthMode::Subscription,
            &sub,
        ),
    ]);
    let automatic = prepare(
        &cfg,
        &selection(None, AuthPreference::Auto),
        &project(root.path()),
        Some("hello"),
        Uuid::new_v4(),
    )
    .unwrap();
    assert_eq!(automatic.resolution.client, ClientKind::ClaudeCode);
    assert_eq!(automatic.resolution.auth_mode, AuthMode::Subscription);
    let explicit = prepare(
        &cfg,
        &selection(Some(ClientKind::Codex), AuthPreference::Auto),
        &project(root.path()),
        Some("hello"),
        Uuid::new_v4(),
    )
    .unwrap();
    assert_eq!(explicit.resolution.client, ClientKind::Codex);
    assert_eq!(explicit.resolution.auth_mode, AuthMode::Api);
}

#[test]
fn configured_subscription_failure_never_falls_back_to_api() {
    let root = tempfile::tempdir().unwrap();
    let api = root.path().join("api.env");
    private(&api, &format!("ANTHROPIC_API_KEY={CLAUDE_KEY}\n"));
    let cfg = config(vec![
        profile(
            "subscription",
            ClientKind::ClaudeCode,
            AuthMode::Subscription,
            &root.path().join("missing.env"),
        ),
        profile("api", ClientKind::ClaudeCode, AuthMode::Api, &api),
    ]);
    let error = prepare(
        &cfg,
        &selection(None, AuthPreference::Auto),
        &project(root.path()),
        Some("hello"),
        Uuid::new_v4(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("login_required"));
    let only_api = config(vec![cfg.profiles[1].clone()]);
    let error = prepare(
        &only_api,
        &selection(None, AuthPreference::Subscription),
        &project(root.path()),
        Some("hello"),
        Uuid::new_v4(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("credential_missing"));
}

#[test]
fn ambiguous_aliases_and_candidates_fail_without_secret_errors() {
    let root = tempfile::tempdir().unwrap();
    let api = root.path().join("api.env");
    private(
        &api,
        &format!("CODEX_API_KEY={CODEX_KEY}\nOPENAI_API_KEY={OAUTH}\n"),
    );
    let mut cfg = config(vec![profile("one", ClientKind::Codex, AuthMode::Api, &api)]);
    let error = prepare(
        &cfg,
        &selection(None, AuthPreference::Auto),
        &project(root.path()),
        Some("hello"),
        Uuid::new_v4(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("ambiguous_configuration"));
    assert!(!format!("{error:#}").contains(CODEX_KEY));
    if let CredentialSourceV1::ApiEnvFile { key, .. } = &mut cfg.profiles[0].credential {
        *key = Some("CODEX_API_KEY".into());
    }
    let prepared = prepare(
        &cfg,
        &selection(None, AuthPreference::Auto),
        &project(root.path()),
        Some("hello"),
        Uuid::new_v4(),
    )
    .unwrap();
    assert!(!prepared.credential.bootstrap_value().contains(OAUTH));
    cfg.profiles
        .push(profile("two", ClientKind::Codex, AuthMode::Api, &api));
    assert!(
        prepare(
            &cfg,
            &selection(None, AuthPreference::Auto),
            &project(root.path()),
            Some("hello"),
            Uuid::new_v4()
        )
        .unwrap_err()
        .to_string()
        .contains("ambiguous_configuration")
    );
}

#[test]
fn only_explicit_excluded_dotenv_inside_project_is_accepted_and_source_is_anchored() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    fs::create_dir(&project).unwrap();
    let source = project.join("credential.env");
    private(&source, &format!("OPENAI_API_KEY={CODEX_KEY}\n"));
    let cfg = config(vec![profile(
        "chosen",
        ClientKind::Codex,
        AuthMode::Api,
        &source,
    )]);
    assert!(
        prepare(
            &cfg,
            &selection(None, AuthPreference::Auto),
            &project,
            Some("hello"),
            Uuid::new_v4()
        )
        .is_err()
    );
    let dotenv = project.join(".env");
    fs::rename(&source, &dotenv).unwrap();
    let cfg = config(vec![profile(
        "chosen",
        ClientKind::Codex,
        AuthMode::Api,
        &dotenv,
    )]);
    let prepared = prepare(
        &cfg,
        &selection(None, AuthPreference::Auto),
        &project,
        Some("hello"),
        Uuid::new_v4(),
    )
    .unwrap();
    fs::rename(&dotenv, project.join("held")).unwrap();
    private(&dotenv, &format!("OPENAI_API_KEY={OAUTH}\n"));
    assert!(prepared.credential.validate_source().is_err());
    assert!(prepared.credential.bootstrap_value().contains(CODEX_KEY));
}

#[test]
fn private_configuration_is_strict_and_errors_do_not_echo_invalid_values() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("providers.json");
    private(
        &path,
        r#"{"version":1,"profiles":[],"default_profile":null,"auto_priority":[]}"#,
    );
    assert_eq!(load_config(&path).unwrap().version, 1);
    private(&path, &format!(r#"{{"version":"{CODEX_KEY}"}}"#));
    let error = load_config(&path).unwrap_err();
    assert!(!format!("{error:#}").contains(CODEX_KEY));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(load_config(&path).is_err());
}

#[test]
fn resolution_rejects_mutated_bootstrap_shape_but_allows_a_bounded_prompt() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("api.env");
    private(&source, &format!("OPENAI_API_KEY={CODEX_KEY}\n"));
    let prepared = prepare(
        &config(vec![profile(
            "chosen",
            ClientKind::Codex,
            AuthMode::Api,
            &source,
        )]),
        &selection(None, AuthPreference::Auto),
        &project(root.path()),
        Some("--ignore this as an option"),
        Uuid::new_v4(),
    )
    .unwrap();
    let mut wrong = prepared.argv.clone();
    wrong.push("--env".into());
    assert!(prepared.resolution.validate_argv(&wrong).is_err());
    let mut resolution = prepared.resolution;
    resolution.auth_mode = AuthMode::Subscription;
    assert!(resolution.validate_argv(&prepared.argv).is_err());
    resolution.credential_names.push("ANTHROPIC_API_KEY".into());
    assert!(resolution.validate().is_err());
}

#[test]
fn codex_subscription_rejects_secret_material_disguised_as_refresh_metadata() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("auth.json");
    private(
        &source,
        &format!(
            r#"{{"auth_mode":"chatgpt","last_refresh":"{CODEX_KEY}","tokens":{{"access_token":"{OAUTH}"}}}}"#
        ),
    );
    let result = prepare(
        &config(vec![profile(
            "chosen",
            ClientKind::Codex,
            AuthMode::Subscription,
            &source,
        )]),
        &selection(None, AuthPreference::Subscription),
        &project(root.path()),
        Some("hello"),
        Uuid::new_v4(),
    );
    assert!(
        result.is_err(),
        "only recognized authentication metadata may accompany subscription tokens"
    );
}

#[test]
fn regression_relative_credentials_are_anchored_to_the_private_configuration_parent() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("only-here.env");
    private(&source, &format!("OPENAI_API_KEY={CODEX_KEY}\n"));
    let mut cfg = config(vec![profile(
        "chosen",
        ClientKind::Codex,
        AuthMode::Api,
        Path::new("only-here.env"),
    )]);
    let file = root.path().join("providers.json");
    private(&file, &serde_json::to_string(&cfg).unwrap());
    let loaded = load_config(&file).unwrap();
    let anchored = agentsandbox::control_fs::PrivateFile::open(
        &agentsandbox::control_fs::absolute_path(&file).unwrap(),
        64 * 1024,
    )
    .unwrap();
    assert_eq!(loaded, load_config_source(&anchored).unwrap());
    let prepared = prepare(
        &loaded,
        &selection(None, AuthPreference::Auto),
        &project(root.path()),
        Some("hello"),
        Uuid::new_v4(),
    )
    .unwrap();
    assert!(prepared.credential.bootstrap_value().contains(CODEX_KEY));
    if let CredentialSourceV1::ApiEnvFile { path, .. } = &mut cfg.profiles[0].credential {
        *path = "../only-here.env".into();
    }
    private(&file, &serde_json::to_string(&cfg).unwrap());
    assert!(load_config(&file).is_err());
    assert!(load_config_source(&anchored).is_err());
}
