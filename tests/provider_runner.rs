#![cfg(unix)]

use std::{fs, path::Path};

fn run(root: &Path, arguments: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .current_dir(root)
        .args(arguments)
        .output()
        .unwrap()
}

#[test]
fn managed_client_without_private_configuration_has_an_actionable_diagnostic() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    agentsandbox::operations::initialize_config(&root.join(".agentsandbox")).unwrap();
    fs::write(root.join("README.txt"), "project").unwrap();
    let output = run(
        &root,
        &[
            "run",
            "--client",
            "codex",
            "--auth-mode",
            "api",
            "--profile",
            "observe",
            "--network",
            "bridge",
            "--prompt",
            "inspect",
        ],
    );
    assert!(!output.status.success());
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains("providers init"), "{diagnostic}");
    assert!(
        root.join(".agentsandbox/sessions")
            .read_dir()
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn managed_selection_cannot_be_combined_with_manual_provider_argv() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    agentsandbox::operations::initialize_config(&root.join(".agentsandbox")).unwrap();
    let output = run(&root, &["run", "--client", "codex", "--", "/usr/bin/id"]);
    assert!(!output.status.success());
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains("manual argv"), "{diagnostic}");
}

#[test]
fn manual_run_still_requires_an_explicit_command() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    agentsandbox::operations::initialize_config(&root.join(".agentsandbox")).unwrap();
    let output = run(&root, &["run"]);
    assert!(!output.status.success());
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains("manual argv"), "{diagnostic}");
}

#[test]
fn provider_initialization_writes_only_explicit_references_and_status_hides_paths() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let env = root.join("private-canary-source.env");
    // Init intentionally does not require the secret file to exist yet.
    let output = run(
        &root,
        &["providers", "init", "--env-file", env.to_str().unwrap()],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = fs::read(root.join(".agentsandbox/providers.json")).unwrap();
    let config: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(config["profiles"].as_array().unwrap().len(), 2);
    assert_eq!(
        config["profiles"][0]["credential"]["path"],
        env.to_str().unwrap()
    );
    let status = run(&root, &["providers", "status", "--json"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let text = String::from_utf8(status.stdout).unwrap();
    let status: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(status["credentials_verified"], false);
    assert_eq!(status["worker_capabilities_verified"], false);
    assert!(!text.contains("private-canary-source"));
    assert!(!text.contains(root.to_str().unwrap()));
    let repeated = run(
        &root,
        &["providers", "init", "--env-file", env.to_str().unwrap()],
    );
    assert!(
        !repeated.status.success(),
        "existing configuration was overwritten"
    );
    assert_eq!(
        fs::read(root.join(".agentsandbox/providers.json")).unwrap(),
        bytes
    );
}
