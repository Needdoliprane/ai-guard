#![cfg(unix)]

use agentsandbox::provider::CredentialSourceV1;
use serde_json::{Value, json};
use std::{
    fs::{self, DirBuilder, OpenOptions, Permissions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Output,
    time::Duration,
};
use tokio::{process::Command, time::timeout};

const PRIVATE_FILE_MARKER: &str = "synthetic-private-reference";
const CONTENT_MARKER: &str = "synthetic-invalid-credential-content";

fn private_dir(path: &Path) -> PathBuf {
    DirBuilder::new().mode(0o700).create(path).unwrap();
    path.to_owned()
}

fn private_file(path: &Path, data: &[u8]) {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap()
        .write_all(data)
        .unwrap();
}

fn api_profile(id: &str, path: &str) -> Value {
    json!({
        "id": id, "revision": 7, "client": "codex", "auth_mode": "api",
        "model": "fixture-default-model",
        "credential": { "kind": "api_env_file", "path": path, "key": "OPENAI_API_KEY" }
    })
}

async fn check(config: &Path, providers: Option<&Path>, args: &[&str], json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .env_clear()
        .env("HOME", config)
        .env("XDG_CONFIG_HOME", config)
        .current_dir(config)
        .kill_on_drop(true)
        .args(["providers", "check", "--config-dir"])
        .arg(config)
        .args(args);
    if json {
        command.arg("--json");
    }
    if let Some(providers) = providers {
        command.arg("--providers-file").arg(providers);
    }
    let output = timeout(Duration::from_secs(15), command.output())
        .await
        .expect("provider diagnostic timed out")
        .expect("could not run provider diagnostic");
    for data in [&output.stdout, &output.stderr] {
        let data = String::from_utf8_lossy(data);
        assert!(!data.contains(PRIVATE_FILE_MARKER));
        assert!(!data.contains(CONTENT_MARKER));
        assert!(!data.contains(config.to_str().unwrap()));
        if let Some(path) = providers {
            assert!(!data.contains(path.to_str().unwrap()));
        }
    }
    output
}

fn report(output: Output, status: &str) -> Value {
    assert!(
        output.status.success(),
        "provider diagnostic failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["version"], 1);
    assert_eq!(result["credential_status"], status);
    assert_eq!(result["credentials_verified"], false);
    assert_eq!(result["worker_capabilities_verified"], false);
    assert!(result.get("credential_binding_id").is_none());
    result
}

#[tokio::test]
async fn checks_the_selected_reference_relative_to_the_explicit_profiles_file() {
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    let config = private_dir(&root_path.join("control"));
    let profiles = private_dir(&root_path.join("profiles"));
    let credential_dir = private_dir(&profiles.join("references"));
    fs::set_permissions(&credential_dir, Permissions::from_mode(0o755)).unwrap();
    let providers = profiles.join("custom.json");
    private_file(
        &credential_dir.join(PRIVATE_FILE_MARKER),
        CONTENT_MARKER.as_bytes(),
    );
    private_file(
        &providers,
        &serde_json::to_vec(&json!({
            "version": 1,
            "profiles": [api_profile("codex-api", &format!("references/{PRIVATE_FILE_MARKER}"))]
        }))
        .unwrap(),
    );
    let selection = [
        "--client",
        "codex",
        "--auth-mode",
        "api",
        "--client-profile",
        "codex-api",
        "--model",
        "fixture-explicit-model",
    ];
    let result = report(
        check(&config, Some(&providers), &selection, true).await,
        "present_unverified",
    );
    assert_eq!(result["profile_id"], "codex-api");
    assert_eq!(result["profile_revision"], 7);
    assert_eq!(result["client"], "codex");
    assert_eq!(result["auth_mode"], "api");
    assert_eq!(result["transport"], "batch");
    assert_eq!(result["model"], "fixture-explicit-model");
    assert_eq!(result["provenance"]["model"], "cli");
    assert_eq!(result["credential_source"], "api_env_file");

    let text = check(&config, Some(&providers), &selection, false).await;
    assert!(text.status.success() && text.stderr.is_empty());
    let stdout = String::from_utf8(text.stdout).unwrap();
    assert!(
        stdout.lines().any(|line| {
            line == "Provenance client=cli auth_mode=cli profile=profile model=cli"
        })
    );
    assert!(
        stdout
            .lines()
            .any(|line| line == "Credential metadata present_unverified")
    );

    let missing_profiles = profiles.join("missing-providers.json");
    let refusal = check(&config, Some(&missing_profiles), &selection, false).await;
    assert!(!refusal.status.success() && refusal.stdout.is_empty());
    let stderr = String::from_utf8(refusal.stderr).unwrap();
    assert!(
        stderr.contains(
            "verify or replace the --providers-file reference and preserve existing files"
        )
    );
    assert!(!stderr.contains("providers init"));
    assert!(!missing_profiles.exists() && providers.is_file());
}

#[tokio::test]
async fn automatic_selection_reports_absent_subscription_without_falling_back_to_api() {
    let root = tempfile::tempdir().unwrap();
    let config = private_dir(&root.path().canonicalize().unwrap().join("control"));
    let api_path = format!("{PRIVATE_FILE_MARKER}.env");
    let oauth_path = format!("{PRIVATE_FILE_MARKER}.oauth");
    private_file(&config.join(&api_path), CONTENT_MARKER.as_bytes());
    private_file(
        &config.join("providers.json"),
        &serde_json::to_vec(&json!({
            "version": 1,
            "default_profile": "codex-api",
            "auto_priority": ["codex-api", "claude-subscription"],
            "profiles": [api_profile("codex-api", &api_path), {
                "id": "claude-subscription", "revision": 1, "client": "claude_code",
                "auth_mode": "subscription", "model": "fixture-subscription-model",
                "credential": {"kind": "claude_oauth_env_file", "path": &oauth_path}
            }, {
                "id": "claude-subscription-legacy", "revision": 1, "client": "claude_code",
                "auth_mode": "subscription", "model": "fixture-subscription-model",
                "credential": {"kind": "claude_o_auth_env_file", "path": &oauth_path}
            }]
        }))
        .unwrap(),
    );
    let result = report(
        check(
            &config,
            None,
            &["--client", "auto", "--auth-mode", "auto", "--interactive"],
            true,
        )
        .await,
        "absent",
    );
    assert_eq!(result["profile_id"], "claude-subscription");
    assert_eq!(result["client"], "claude_code");
    assert_eq!(result["auth_mode"], "subscription");
    assert_eq!(result["transport"], "interactive_tty");
    assert_eq!(result["model"], "fixture-subscription-model");
    assert_eq!(result["credential_source"], "claude_oauth_env_file");

    let legacy = report(
        check(
            &config,
            None,
            &[
                "--client-profile",
                "claude-subscription-legacy",
                "--interactive",
            ],
            true,
        )
        .await,
        "absent",
    );
    assert_eq!(legacy["profile_id"], "claude-subscription-legacy");
    for field in [
        "client",
        "auth_mode",
        "transport",
        "model",
        "credential_source",
    ] {
        assert_eq!(
            legacy[field], result[field],
            "legacy credential changed {field}"
        );
    }
    let serialized = serde_json::to_value(CredentialSourceV1::ClaudeOAuthEnvFile {
        path: PathBuf::from(oauth_path),
    })
    .unwrap();
    assert_eq!(serialized["kind"], "claude_oauth_env_file");

    let conflict = check(
        &config,
        None,
        &[
            "--auth-mode",
            "api",
            "--client-profile",
            "claude-subscription",
        ],
        true,
    )
    .await;
    assert!(!conflict.status.success());
    assert!(conflict.stdout.is_empty());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("ambiguous_configuration"));
}

#[tokio::test]
async fn marks_non_private_files_and_linked_paths_as_suspect() {
    let root = tempfile::tempdir().unwrap();
    let config = private_dir(&root.path().canonicalize().unwrap().join("control"));
    let real_parent = private_dir(&config.join("real-parent"));
    let target = real_parent.join(PRIVATE_FILE_MARKER);
    private_file(&target, CONTENT_MARKER.as_bytes());

    let permissive = format!("{PRIVATE_FILE_MARKER}-permissive");
    private_file(&config.join(&permissive), CONTENT_MARKER.as_bytes());
    fs::set_permissions(config.join(&permissive), Permissions::from_mode(0o640)).unwrap();

    let final_symlink = format!("{PRIVATE_FILE_MARKER}-symlink");
    symlink(&target, config.join(&final_symlink)).unwrap();
    let missing_parent = format!("{PRIVATE_FILE_MARKER}-missing-parent");
    let through_missing_parent = format!("{missing_parent}/{PRIVATE_FILE_MARKER}");
    let parent_symlink = format!("{PRIVATE_FILE_MARKER}-parent");
    symlink(&real_parent, config.join(&parent_symlink)).unwrap();
    let through_parent_symlink = format!("{parent_symlink}/{PRIVATE_FILE_MARKER}");

    let hard_link = format!("{PRIVATE_FILE_MARKER}-hardlink");
    let hard_link_original = config.join(format!("{PRIVATE_FILE_MARKER}-original"));
    private_file(&hard_link_original, CONTENT_MARKER.as_bytes());
    fs::hard_link(hard_link_original, config.join(&hard_link)).unwrap();

    let cases = [
        ("permissive", permissive),
        ("final-symlink", final_symlink),
        ("parent-symlink", through_parent_symlink),
        ("missing-parent", through_missing_parent),
        ("hard-link", hard_link),
    ];
    let profiles: Vec<_> = cases
        .iter()
        .map(|(name, path)| api_profile(name, path))
        .collect();
    private_file(
        &config.join("providers.json"),
        &serde_json::to_vec(&json!({"version": 1, "profiles": profiles})).unwrap(),
    );
    for (name, _) in cases {
        let result = report(
            check(&config, None, &["--client-profile", name], true).await,
            "suspect",
        );
        assert_eq!(result["profile_id"], name);
    }
}
