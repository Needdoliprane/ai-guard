#![cfg(unix)]

use agentsandbox::provider::{
    AuthMode, AuthPreference, ClientKind, ClientTransport, CredentialSourceV1, ProviderConfigV1,
    ProviderProfileV1, ProviderSelection, bootstrap, prepare,
};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
    process::Command,
};
use uuid::Uuid;

const CANARY: &str = "canary-bootstrap-private-provider-secret-55";

fn private(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn prepared(
    root: &Path,
    client: ClientKind,
    auth_mode: AuthMode,
    transport: ClientTransport,
) -> agentsandbox::provider::PreparedProvider {
    let project = root.join("project");
    fs::create_dir_all(&project).unwrap();
    let source = root.join("provider.env");
    let credential = match (client, auth_mode) {
        (ClientKind::Codex, AuthMode::Subscription) => {
            private(
                &source,
                &format!(
                    r#"{{"auth_mode":"chatgpt","tokens":{{"access_token":"{CANARY}","refresh_token":"canary-private-refresh-56"}}}}"#
                ),
            );
            CredentialSourceV1::CodexAuthFile { path: source }
        }
        (ClientKind::ClaudeCode, AuthMode::Subscription) => {
            private(
                &source,
                &format!(
                    "CLAUDE_CODE_OAUTH_TOKEN={CANARY}\nANTHROPIC_API_KEY=unselected-provider-api-secret\n"
                ),
            );
            CredentialSourceV1::ClaudeOAuthEnvFile { path: source }
        }
        (_, AuthMode::Api) => {
            private(
                &source,
                &format!("OPENAI_API_KEY={CANARY}\nANTHROPIC_API_KEY={CANARY}\n"),
            );
            CredentialSourceV1::ApiEnvFile {
                path: source,
                key: None,
            }
        }
    };
    let cfg = ProviderConfigV1 {
        version: 1,
        default_profile: None,
        auto_priority: Vec::new(),
        profiles: vec![ProviderProfileV1 {
            id: "chosen".into(),
            revision: 1,
            client,
            auth_mode,
            model: Some("a-model".into()),
            credential,
        }],
    };
    let selected = ProviderSelection {
        client: Some(client),
        auth_mode: AuthPreference::from(auth_mode),
        profile_id: None,
        transport,
        model: None,
    };
    prepare(
        &cfg,
        &selected,
        &project,
        Some("--a prompt is data"),
        Uuid::new_v4(),
    )
    .unwrap()
}

#[test]
fn all_eight_bootstrap_plans_create_private_homes_and_exclusive_authentication() {
    for client in [ClientKind::Codex, ClientKind::ClaudeCode] {
        for auth_mode in [AuthMode::Api, AuthMode::Subscription] {
            for transport in [ClientTransport::Batch, ClientTransport::InteractiveTty] {
                let root = tempfile::tempdir().unwrap();
                let home = root.path().join("home");
                fs::create_dir(&home).unwrap();
                fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
                let prepared = prepared(root.path(), client, auth_mode, transport);
                let execution = bootstrap::prepare_execution(
                    &prepared.argv,
                    prepared.credential.bootstrap_value(),
                    &home,
                )
                .unwrap();
                assert!(!format!("{execution:?}").contains(CANARY));
                assert!(
                    !execution
                        .arguments()
                        .iter()
                        .any(|argument| argument.contains(CANARY))
                );
                assert!(
                    !execution
                        .environment()
                        .contains_key("AI_GUARD_PROVIDER_BOOTSTRAP")
                );
                assert!(!execution.environment().contains_key("OPENAI_API_KEY"));
                if client == ClientKind::Codex {
                    let codex_home = Path::new(&execution.environment()["CODEX_HOME"]);
                    assert!(
                        codex_home.is_dir(),
                        "CODEX_HOME must exist before API login"
                    );
                    assert_eq!(
                        fs::metadata(codex_home).unwrap().permissions().mode() & 0o777,
                        0o700
                    );
                    assert!(!execution.environment().contains_key("ANTHROPIC_API_KEY"));
                    assert!(
                        !execution
                            .environment()
                            .contains_key("CLAUDE_CODE_OAUTH_TOKEN")
                    );
                    assert_eq!(
                        execution.needs_api_login(),
                        auth_mode == AuthMode::Api && transport == ClientTransport::InteractiveTty
                    );
                    assert_eq!(
                        execution.environment().contains_key("CODEX_API_KEY"),
                        auth_mode == AuthMode::Api && transport == ClientTransport::Batch
                    );
                    if auth_mode == AuthMode::Subscription {
                        let auth = codex_home.join("auth.json");
                        assert_eq!(
                            fs::metadata(&auth).unwrap().permissions().mode() & 0o777,
                            0o600
                        );
                        assert!(fs::read_to_string(auth).unwrap().contains(CANARY));
                    }
                } else {
                    assert!(!execution.environment().contains_key("CODEX_API_KEY"));
                    assert_eq!(
                        execution.environment().contains_key("ANTHROPIC_API_KEY"),
                        auth_mode == AuthMode::Api
                    );
                    assert_eq!(
                        execution
                            .environment()
                            .contains_key("CLAUDE_CODE_OAUTH_TOKEN"),
                        auth_mode == AuthMode::Subscription
                    );
                    assert!(
                        execution
                            .arguments()
                            .windows(2)
                            .any(|pair| pair == ["--setting-sources", "user"])
                    );
                }
            }
        }
    }
}

#[test]
fn malformed_or_mismatched_payload_cannot_select_another_provider() {
    let root = tempfile::tempdir().unwrap();
    let prepared = prepared(
        root.path(),
        ClientKind::Codex,
        AuthMode::Api,
        ClientTransport::Batch,
    );
    let wrong = prepared
        .credential
        .bootstrap_value()
        .replace("\"codex\"", "\"claude_code\"");
    let error = bootstrap::prepare_execution(&prepared.argv, &wrong, root.path()).unwrap_err();
    assert!(!format!("{error:#}").contains(CANARY));
    let error = bootstrap::prepare_execution(&prepared.argv, CANARY, root.path()).unwrap_err();
    assert!(!format!("{error:#}").contains(CANARY));
}

#[test]
fn unsafe_home_is_rejected_without_following_a_symlink_or_traversal() {
    let root = tempfile::tempdir().unwrap();
    let prepared = prepared(
        root.path(),
        ClientKind::Codex,
        AuthMode::Subscription,
        ClientTransport::Batch,
    );
    let target = root.path().join("actual-home");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
    let link = root.path().join("home");
    symlink(&target, &link).unwrap();
    assert!(
        bootstrap::prepare_execution(&prepared.argv, prepared.credential.bootstrap_value(), &link)
            .is_err()
    );
    assert_eq!(fs::read_dir(target).unwrap().count(), 0);
    assert!(
        bootstrap::prepare_execution(
            &prepared.argv,
            prepared.credential.bootstrap_value(),
            &root.path().join("project/../actual-home")
        )
        .is_err()
    );
}

#[test]
fn bootstrap_binary_errors_are_bounded_and_never_echo_payload_or_injected_args() {
    let output = Command::new(env!("CARGO_BIN_EXE_ai-guard-provider"))
        .env_clear()
        .env("AI_GUARD_PROVIDER_BOOTSTRAP", CANARY)
        .arg(CANARY)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.len() < 512);
    assert!(!String::from_utf8_lossy(&output.stderr).contains(CANARY));
}

#[tokio::test]
async fn bootstrap_error_preserves_the_callers_environment_in_an_isolated_process() {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .env_clear()
        .env("AI_GUARD_PROVIDER_BOOTSTRAP", CANARY)
        .args([
            "--exact",
            "bootstrap_environment_error_fixture",
            "--ignored",
            "--nocapture",
        ])
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(5), command.output())
        .await
        .expect("isolated bootstrap environment fixture timed out")
        .unwrap();
    assert!(
        output.status.success(),
        "isolated bootstrap environment contract failed"
    );
    assert!(output.stdout.len() + output.stderr.len() < 8 * 1024);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(CANARY));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(CANARY));
}

#[test]
#[ignore = "internal isolated process fixture, invoked by the bootstrap environment regression"]
fn bootstrap_environment_error_fixture() {
    assert!(bootstrap::run_from_environment(&["invalid-request".into()]).is_err());
    assert!(
        std::env::var("AI_GUARD_PROVIDER_BOOTSTRAP").as_deref() == Ok(CANARY),
        "a safe library error path must not mutate the process environment"
    );
}

#[test]
fn regression_codex_pins_official_provider_routes_in_its_cli_layer() {
    let root = tempfile::tempdir().unwrap();
    let prepared = prepared(
        root.path(),
        ClientKind::Codex,
        AuthMode::Api,
        ClientTransport::Batch,
    );
    let home = root.path().join("home");
    fs::create_dir(&home).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    let execution =
        bootstrap::prepare_execution(&prepared.argv, prepared.credential.bootstrap_value(), &home)
            .unwrap();
    for expected in [
        "model_provider=\"openai\"",
        "openai_base_url=\"https://api.openai.com/v1\"",
        "chatgpt_base_url=\"https://chatgpt.com/backend-api/\"",
    ] {
        assert!(
            execution
                .arguments()
                .windows(2)
                .any(|pair| pair == ["-c", expected])
        );
    }
    for name in [
        "OPENAI_BASE_URL",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "ANTHROPIC_API_KEY",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ] {
        assert!(!execution.environment().contains_key(name));
    }
}
