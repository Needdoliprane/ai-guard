//! Regression candidates for the operator's real retained build plan.
//! No engine, provider, Git command or live checkout read is needed.
use super::{Fixture, PreparedSource, Scope, refusal};
use crate::runtime::retained_cli::RetainedInput;
use std::{fs, sync::Arc};

fn aligned_fixture() -> Fixture {
    let fixture = Fixture::new();
    fixture.write(
        "Cargo.toml",
        "[package]\nname = \"agentsandbox\"\nversion = \"0.0.1\"\n",
    );
    fixture.write(
        "Cargo.lock",
        "version = 4\n[[package]]\nname = \"agentsandbox\"\nversion = \"0.0.1\"\n",
    );
    fixture.write(
        "front/package.json",
        r#"{"name":"ai-guard-front","version":"0.0.1"}"#,
    );
    fixture.write(
        "front/package-lock.json",
        r#"{
          "name":"ai-guard-front","version":"0.0.1","lockfileVersion":3,
          "packages":{"":{"name":"ai-guard-front","version":"0.0.1"}}
        }"#,
    );
    fixture
}

fn prepared(fixture: &Fixture) -> PreparedSource {
    PreparedSource {
        snapshot: Arc::new(fixture.prepare(Scope::Build)),
    }
}

fn assignments(command: &super::super::PreparedCommand) -> Vec<&str> {
    command
        .arguments
        .windows(2)
        .filter(|pair| pair[0] == "--build-arg")
        .map(|pair| pair[1].as_str())
        .collect()
}

// Catches one component independently generating metadata or losing provider pins.
#[test]
fn one_build_plan_aligns_both_recipes_and_retains_provider_choices() {
    let fixture = aligned_fixture();
    let source = prepared(&fixture);
    let expected_id = format!("AI_GUARD_BUILD_ID={}", source.snapshot.digest());
    let revision = "0123456789abcdef0123456789abcdef01234567";
    let plan = source
        .build_plan(Some(revision), Some("1.2.3"), Some("4.5.6"))
        .unwrap();
    drop(source);
    let [control, worker] = plan.commands().unwrap();
    assert_eq!(
        assignments(&control),
        [
            "AI_GUARD_VERSION=0.0.1",
            expected_id.as_str(),
            "AI_GUARD_BUILD_REVISION=0123456789abcdef0123456789abcdef01234567",
        ]
    );
    assert_eq!(
        assignments(&worker),
        [
            "CODEX_VERSION=1.2.3",
            "CLAUDE_CODE_VERSION=4.5.6",
            "CLAUDE_ACTIVITY_PROFILE=v1-onfailure",
            "AI_GUARD_VERSION=0.0.1",
            expected_id.as_str(),
            "AI_GUARD_BUILD_REVISION=0123456789abcdef0123456789abcdef01234567",
        ]
    );
    assert!(control.arguments[2].ends_with("/images/control.Dockerfile"));
    assert!(worker.arguments[2].ends_with("/images/worker.Dockerfile"));
    assert_eq!(control.arguments[4], "agentsandbox-control:local");
    assert_eq!(worker.arguments[4], "agentsandbox-worker:local");
    assert_eq!(control.arguments.last(), worker.arguments.last());
    control.verify_before_spawn().unwrap();
    worker.verify_before_spawn().unwrap();
}

// Catches installing a hook profile the requested Claude Code cannot read.
#[test]
fn the_claude_activity_profile_follows_the_requested_release() {
    for (version, profile) in [
        ("2.1.294", "v1"),
        ("2.1.295-rc.1", "v1"),
        ("2.0.999", "v1"),
        ("1.99.400", "v1"),
        ("2.1.295", "v1-onfailure"),
        ("2.1.295+build.7", "v1-onfailure"),
        ("2.1.296", "v1-onfailure"),
        ("2.2.0", "v1-onfailure"),
        ("3.0.0-beta", "v1-onfailure"),
    ] {
        assert_eq!(
            super::super::build_identity::claude_activity_profile(version).unwrap(),
            profile,
            "{version}"
        );
    }
    for invalid in ["2.1", "latest", "2.1.295.1", "v2.1.295"] {
        assert!(super::super::build_identity::claude_activity_profile(invalid).is_err());
    }
    for (claude, expected) in [
        (Some("2.1.200"), Some("CLAUDE_ACTIVITY_PROFILE=v1")),
        (None, None),
    ] {
        let fixture = aligned_fixture();
        let source = prepared(&fixture);
        let plan = source.build_plan(None, Some("1.2.3"), claude).unwrap();
        drop(source);
        let [control, worker] = plan.commands().unwrap();
        assert!(
            !assignments(&control)
                .iter()
                .any(|value| value.starts_with("CLAUDE_"))
        );
        let profiles: Vec<_> = assignments(&worker)
            .into_iter()
            .filter(|value| value.starts_with("CLAUDE_ACTIVITY_PROFILE="))
            .collect();
        assert_eq!(profiles, expected.into_iter().collect::<Vec<_>>());
        worker.verify_before_spawn().unwrap();
    }
}

// Catches rereading the mutable checkout or inventing a revision when absent.
#[test]
fn build_plan_uses_captured_versions_after_live_source_changes() {
    let fixture = aligned_fixture();
    let source = prepared(&fixture);
    fixture.write("Cargo.toml", "unread live replacement");
    fixture.write("front/package.json", "unread live replacement");
    let commands = source
        .build_plan(None, None, None)
        .unwrap()
        .commands()
        .unwrap();
    for command in &commands {
        let args = assignments(command);
        assert_eq!(args[0], "AI_GUARD_VERSION=0.0.1");
        assert_eq!(args[2], "AI_GUARD_BUILD_REVISION=");
        command.verify_before_spawn().unwrap();
    }
}

// Catches accepting stale lock roots, split product versions or unknown manifests.
#[test]
fn build_plan_refuses_disagreement_or_unreadable_product_versions() {
    for (file, replacement) in [
        (
            "Cargo.toml",
            "[package]\nname = \"agentsandbox\"\nversion = \"0.0.2\"\n",
        ),
        (
            "Cargo.toml",
            "[package]\nname = \"agentsandbox\"\nversion.workspace = true\n",
        ),
        (
            "Cargo.toml",
            "[package]\nname = \"agentsandbox\"\nversion = \"bad version\"\n",
        ),
        (
            "Cargo.lock",
            "version = 4\n[[package]]\nname = \"agentsandbox\"\nversion = \"0.0.2\"\n",
        ),
        (
            "front/package.json",
            r#"{"name":"ai-guard-front","version":"0.0.2"}"#,
        ),
        (
            "front/package-lock.json",
            r#"{
              "name":"ai-guard-front","version":"0.0.2","lockfileVersion":3,
              "packages":{"":{"name":"ai-guard-front","version":"0.0.2"}}
            }"#,
        ),
    ] {
        let fixture = aligned_fixture();
        fixture.write(file, replacement);
        let source = prepared(&fixture);
        assert!(
            source.build_plan(None, None, None).is_err(),
            "accepted {file}"
        );
    }
    let fixture = aligned_fixture();
    fixture.write(
        "front/package.json",
        r#"{"name":"ai-guard-front","version":"0.0.2"}"#,
    );
    fixture.write(
        "front/package-lock.json",
        r#"{
          "name":"ai-guard-front","version":"0.0.2","lockfileVersion":3,
          "packages":{"":{"name":"ai-guard-front","version":"0.0.2"}}
        }"#,
    );
    assert!(prepared(&fixture).build_plan(None, None, None).is_err());
}

// Catches malformed optional metadata entering argv, including empty explicit input.
#[test]
fn build_plan_accepts_only_explicit_40_or_64_hex_revisions() {
    let fixture = aligned_fixture();
    let source = prepared(&fixture);
    for revision in [
        "",
        "main",
        "--file=/etc/passwd",
        "0123456789abcdef0123456789abcdef0123456g",
        "0123456789abcdef0123456789abcdef012345678",
    ] {
        assert!(source.build_plan(Some(revision), None, None).is_err());
    }
    for revision in [
        "0123456789ABCDEF0123456789ABCDEF01234567",
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    ] {
        assert!(source.build_plan(Some(revision), None, None).is_ok());
    }
}

// Catches plan creation or engine spawning after alteration of the sealed copy.
#[test]
fn changed_snapshot_refuses_build_plan_and_retained_spawn() {
    let fixture = aligned_fixture();
    let source = prepared(&fixture);
    let plan = source.build_plan(None, None, None).unwrap();
    let commands = plan.commands().unwrap();
    fs::write(source.snapshot.context().join("Cargo.toml"), "changed copy").unwrap();
    assert!(refusal(source.build_plan(None, None, None)).contains("prepared copy"));
    assert!(plan.commands().is_err());
    for command in &commands {
        assert!(command.verify_before_spawn().is_err());
    }
}

// Catches bypassing the plan's captured metadata through the public option grammar.
#[test]
fn arbitrary_build_metadata_cannot_be_injected_into_prepared_command() {
    let fixture = aligned_fixture();
    let source = prepared(&fixture);
    for assignment in [
        "AI_GUARD_VERSION=9.9.9",
        "AI_GUARD_BUILD_ID=made-up",
        "AI_GUARD_BUILD_REVISION=0123456789abcdef0123456789abcdef01234567",
    ] {
        assert!(
            source
                .build_command(
                    "images/control.Dockerfile",
                    &[
                        "build".into(),
                        "--tag".into(),
                        "agentsandbox-control:local".into(),
                        "--build-arg".into(),
                        assignment.into(),
                    ],
                )
                .is_err()
        );
    }
}

// Catches ignoring a recorded stop before reading source or selecting an engine.
#[tokio::test]
async fn interrupted_build_plan_returns_no_commands_before_reading_the_copy() {
    let fixture = aligned_fixture();
    let source = prepared(&fixture);
    fs::write(source.snapshot.context().join("Cargo.toml"), "changed copy").unwrap();
    let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let result = super::scope_with(super::Scripted(receiver), async {
        let interruption = super::interrupt::current().unwrap();
        interruption.request(libc::SIGTERM);
        let result = source.prepare_build_plan(None, None, None).await;
        assert_eq!(interruption.active(), 0);
        assert!(refusal(result).contains("interrupted by SIGTERM"));
        Ok(())
    })
    .await;
    result.unwrap();
}
