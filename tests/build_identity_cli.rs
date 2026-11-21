//! Real binaries in separately admitted QA; these tests are not executed on the host.
use agentsandbox::build_identity::{BuildStatusV1, Component, MAX_STATUS_BYTES};
use std::process::{Command, Stdio};

const CARGO_VERSION: &str = env!("CARGO_PKG_VERSION");
const TOOLS: [(&str, &str); 6] = [
    (env!("CARGO_BIN_EXE_agentsandbox"), "agentsandbox"),
    (env!("CARGO_BIN_EXE_ai-guard-diagnose"), "ai-guard-diagnose"),
    (env!("CARGO_BIN_EXE_ai-guard-provider"), "ai-guard-provider"),
    (env!("CARGO_BIN_EXE_ai-guard-demo-pki"), "ai-guard-demo-pki"),
    (env!("CARGO_BIN_EXE_ai-guard-relay"), "ai-guard-relay"),
    (
        env!("CARGO_BIN_EXE_ai-guard-audit-checkpoint"),
        "ai-guard-audit-checkpoint",
    ),
];

fn invoke(binary: &str, arguments: &[&str]) -> std::process::Output {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    // No provider credentials, configuration, tools on PATH or stdin payload.
    let output = Command::new(binary)
        .args(arguments)
        .current_dir(directory.path())
        .env_clear()
        .env("PATH", "")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        std::fs::read_dir(directory.path())
            .unwrap()
            .next()
            .is_none()
    );
    assert!(output.stdout.len() <= MAX_STATUS_BYTES);
    assert!(output.stderr.len() <= MAX_STATUS_BYTES);
    output
}

#[test]
fn six_real_tools_report_one_version_without_configuration_or_external_tools() {
    for (binary, component) in TOOLS {
        for flag in ["--version", "-V"] {
            let output = invoke(binary, &[flag]);
            assert_eq!(output.status.code(), Some(0), "{component}");
            let text = String::from_utf8(output.stdout).unwrap();
            let lines: Vec<_> = text.lines().collect();
            assert_eq!(lines.len(), 3);
            assert_eq!(lines[0], format!("{component} {CARGO_VERSION}"));
            let identity =
                serde_json::to_value(BuildStatusV1::current(Component::Agentsandbox)).unwrap();
            assert_eq!(
                lines[1],
                format!(
                    "build: {}",
                    identity["build"]["id"].as_str().unwrap_or("unknown")
                )
            );
            assert_eq!(
                lines[2],
                format!(
                    "revision: {}",
                    identity["build"]["revision"].as_str().unwrap_or("unknown")
                )
            );
            assert!(output.stderr.is_empty());
        }
    }
}

#[test]
fn six_real_tools_refuse_extra_version_arguments_before_bootstrap() {
    for (binary, component) in TOOLS {
        for arguments in [
            vec!["--version", "--help"],
            vec!["--version", "synthetic-private-input"],
            vec!["synthetic-private-input", "-V"],
            vec!["-Vsynthetic-private-input"],
            vec!["--version=synthetic-private-input"],
            vec!["--version-synthetic-private-input"],
        ] {
            let output = invoke(binary, &arguments);
            assert_eq!(output.status.code(), Some(2), "{component}");
            assert!(output.stdout.is_empty());
            assert!(!String::from_utf8_lossy(&output.stderr).contains("synthetic-private-input"));
        }
    }
}

#[test]
fn agentsandbox_static_json_has_the_same_delivery_identity_as_the_library() {
    let output = invoke(TOOLS[0].0, &["version", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let status = BuildStatusV1::from_json(&output.stdout, Component::Agentsandbox).unwrap();
    assert_eq!(status, BuildStatusV1::current(Component::Agentsandbox));
    let text = invoke(TOOLS[0].0, &["version"]);
    assert_eq!(text.status.code(), Some(0));
    assert!(
        String::from_utf8(text.stdout)
            .unwrap()
            .starts_with(&format!("agentsandbox {CARGO_VERSION}\nbuild: "))
    );
    let invalid = invoke(
        TOOLS[0].0,
        &["version", "--json", "synthetic-private-input"],
    );
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&invalid.stderr).contains("synthetic-private-input"));
}
