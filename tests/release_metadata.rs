//! `agentsandbox release manifest` end to end, with the real binary: what it
//! binds, what it refuses, what it never writes and where it publishes.
//!
//! Every scenario is a synthetic distribution of the harness whose locks,
//! manifests and recipes are the repository's own files. No engine runs,
//! except the compiled fixture engine of `tests/source_lifecycle` for the one
//! `build` used as a cross-check. Nothing here qualifies an image build, a
//! Docker or Podman engine, or a CycloneDX schema validation.
#![cfg(unix)]

#[path = "source_lifecycle/harness.rs"]
mod harness;
#[path = "release_metadata/support.rs"]
mod support;

use base64::{Engine, engine::general_purpose::STANDARD};
use harness::{
    FixtureState, Spawn, distribution_digest, install_engine, provenance_lines, tree_manifest,
};
use sha2::{Digest, Sha512};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CString, OsString},
    fs::{self, File},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use support::{
    DOCUMENTS, Options, PROVENANCE, Scenario, WINDOW, arguments, artifact_arguments,
    cargo_lock_counts, copy_tree, documents, edit_file, inputs_identity, link_facts,
    make_private_directory, make_public_directories, manifest_json, names, sbom_json, sha256_hex,
    sums_text, write_public_file,
};

fn staging(output: &Path) -> PathBuf {
    let leaf = output
        .file_name()
        .expect("an output has a name")
        .to_string_lossy();
    output.with_file_name(format!(".{leaf}.release-staging"))
}

fn assert_nothing_published(output: &Path, context: &str) {
    assert!(
        !output.exists(),
        "FAIL: {context}: an output directory exists: {}",
        output.display()
    );
    assert!(
        !staging(output).exists(),
        "FAIL: {context}: a staging directory exists"
    );
}

fn string_field<'a>(value: &'a serde_json::Value, context: &str) -> &'a str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("FAIL: {context} is not a string: {value}"))
}

#[test]
fn manifest_binds_the_digest_that_build_reports_for_the_same_unchanged_source() {
    let mut scenario = Scenario::new();
    let control = scenario
        .operator
        .program
        .parent()
        .expect("SETUP: the engine program has a directory")
        .to_path_buf();
    scenario.operator.program = install_engine(&control, "podman");
    scenario.operator.state = FixtureState::for_program(&scenario.operator.program);
    let expected = distribution_digest(&scenario.build_inputs());

    let built = scenario
        .operator
        .run("build", &["--runtime", "podman"], &Spawn::default());
    assert!(
        built.status.success(),
        "SETUP: the fixture build did not succeed: {}",
        built.describe()
    );
    let reported: Vec<_> = provenance_lines(&built.stderr)
        .into_iter()
        .filter(|line| line.scope == "build")
        .collect();
    assert_eq!(reported.len(), 1, "SETUP: one build provenance line");
    let reported = &reported[0].digest;
    assert_eq!(
        reported, &expected,
        "SETUP: the build did not report the documented digest of its inputs"
    );
    scenario.assert_tmp_empty("after build");

    let (output, outcome) = scenario.manifest(
        "bound",
        &arguments(&["--expect-source-digest", reported.sha256.as_str()]),
    );
    outcome.succeeded("release manifest --expect-source-digest");
    assert!(
        outcome
            .stdout
            .contains("expected source digest matched (declared by the operator)"),
        "FAIL: the summary does not report the expectation: {}",
        outcome.describe()
    );
    let manifest = manifest_json(&output);
    assert_eq!(
        manifest["source"]["digest"]["value"],
        expected.sha256.as_str()
    );
    assert_eq!(
        manifest["source"]["digest"]["layout"],
        "ai-guard:public-distribution:v1"
    );
    assert_eq!(manifest["source"]["files"], expected.files);
    assert_eq!(manifest["source"]["bytes"], expected.bytes);
    assert!(
        outcome
            .stdout
            .contains(&format!("source     sha256={} ", expected.sha256)),
        "FAIL: the summary names another source digest: {}",
        outcome.describe()
    );
    scenario.assert_tmp_empty("after release manifest");
}

#[test]
fn manifest_entries_and_lock_hashes_are_those_of_the_original_bytes() {
    let scenario = Scenario::new();
    let output = scenario.published("entries", &[]);
    let manifest = manifest_json(&output);
    let entries: Vec<(String, u64, String)> = manifest["source"]["entries"]
        .as_array()
        .expect("FAIL: entries")
        .iter()
        .map(|entry| {
            (
                string_field(&entry["path"], "an entry path").to_owned(),
                entry["bytes"].as_u64().expect("FAIL: entry bytes"),
                string_field(&entry["sha256"], "an entry digest").to_owned(),
            )
        })
        .collect();
    let expected: Vec<(String, u64, String)> = scenario
        .build_inputs()
        .iter()
        .map(|(path, bytes)| (path.clone(), bytes.len() as u64, sha256_hex(bytes)))
        .collect();
    assert_eq!(
        entries, expected,
        "FAIL: the entries are not the build inputs"
    );

    let cargo_lock = fs::read(scenario.source().join("Cargo.lock")).expect("SETUP: Cargo.lock");
    let npm_lock =
        fs::read(scenario.source().join("front/package-lock.json")).expect("SETUP: npm lock");
    let cargo = &manifest["locks"]["cargo"];
    assert_eq!(cargo["sha256"], sha256_hex(&cargo_lock).as_str());
    assert_eq!(cargo["bytes"], cargo_lock.len() as u64);
    let (packages, checksums) = cargo_lock_counts(std::str::from_utf8(&cargo_lock).expect("UTF-8"));
    assert_eq!(cargo["packages"], packages as u64);
    assert_eq!(cargo["registry_packages"], checksums as u64);
    assert_eq!(cargo["path_packages"], 1);
    assert_eq!(cargo["lock_version"], 4);
    let npm = &manifest["locks"]["npm"];
    assert_eq!(npm["sha256"], sha256_hex(&npm_lock).as_str());
    assert_eq!(npm["bytes"], npm_lock.len() as u64);
    let lock: serde_json::Value = serde_json::from_slice(&npm_lock).expect("SETUP: npm lock");
    let installed: Vec<(&String, &serde_json::Value)> = lock["packages"]
        .as_object()
        .expect("SETUP: packages")
        .iter()
        .filter(|(key, _)| key.starts_with("node_modules/"))
        .collect();
    let count = |key: &str| {
        installed
            .iter()
            .filter(|(_, entry)| entry[key].as_bool() == Some(true))
            .count() as u64
    };
    assert_eq!(npm["packages"], installed.len() as u64);
    assert_eq!(npm["dev"], count("dev"));
    assert_eq!(npm["optional"], count("optional"));
    let mut scripts: Vec<String> = installed
        .iter()
        .filter(|(_, entry)| entry["hasInstallScript"].as_bool() == Some(true))
        .map(|(key, _)| key.trim_start_matches("node_modules/").to_owned())
        .collect();
    scripts.sort();
    assert_eq!(npm["install_scripts"], serde_json::json!(scripts));
    // The package is the root of Cargo.lock, which Cargo.toml names.
    let cargo_toml = fs::read_to_string(scenario.source().join("Cargo.toml")).expect("SETUP");
    let package_line = |key: &str| {
        cargo_toml
            .lines()
            .skip_while(|line| *line != "[package]")
            .find_map(|line| line.strip_prefix(&format!("{key} = \"")))
            .map(|rest| rest.trim_end_matches('"').to_owned())
            .expect("SETUP: a [package] line")
    };
    assert_eq!(manifest["package"]["name"], package_line("name").as_str());
    assert_eq!(
        manifest["package"]["version"],
        package_line("version").as_str()
    );
    assert_eq!(manifest["package"]["license"]["status"], "declared");

    let mut expected_names: Vec<String> = DOCUMENTS.iter().map(|name| (*name).to_owned()).collect();
    expected_names.sort();
    assert_eq!(names(&output), expected_names, "FAIL: the published files");
    assert_eq!(
        fs::metadata(&output).expect("output").mode() & 0o7777,
        0o700
    );
    for name in DOCUMENTS {
        let facts = fs::metadata(output.join(name)).expect("document");
        assert_eq!(facts.mode() & 0o7777, 0o600, "FAIL: mode of {name}");
        assert_eq!(facts.nlink(), 1);
    }
    let [manifest_bytes, sbom_bytes, sums] = documents(&output);
    assert_eq!(
        String::from_utf8(sums).expect("UTF-8"),
        sums_text(&manifest_bytes, &sbom_bytes)
    );
    assert!(manifest_bytes.ends_with(b"}\n"));
    scenario.assert_tmp_empty("after release manifest");
}

#[test]
fn same_inputs_give_byte_identical_documents_whatever_the_path_umask_or_tmpdir() {
    let scenario = Scenario::new();
    let bytes = b"artefact bytes, bound by name, length and digest\n".repeat(64);
    let first_artifact = scenario.artifact("agentsandbox", &bytes);
    let first = scenario.published("first", &artifact_arguments(&[first_artifact.as_path()]));

    let copy = scenario
        .base
        .join("elsewhere-PATH-COMPONENT")
        .join("project");
    copy_tree(scenario.source(), &copy);
    let other_tmp = scenario.base.join("tmp-second");
    make_private_directory(&other_tmp);
    let other_artifacts = scenario.base.join("artifacts-second");
    make_public_directories(&other_artifacts);
    let second_artifact = other_artifacts.join("agentsandbox");
    write_public_file(&second_artifact, &bytes);
    let second = scenario.outputs.join("second");
    let mut arguments: Vec<OsString> = vec![
        "release".into(),
        "manifest".into(),
        "--source".into(),
        copy.clone().into_os_string(),
        "--output".into(),
        second.clone().into_os_string(),
    ];
    arguments.extend(artifact_arguments(&[second_artifact.as_path()]));
    let outcome = scenario.run(
        &arguments,
        &Options {
            tmpdir: other_tmp.clone(),
            umask: 0o077,
            current_dir: None,
        },
    );
    outcome.succeeded("the second release manifest");
    let [first_manifest, first_sbom, first_sums] = documents(&first);
    let [second_manifest, second_sbom, second_sums] = documents(&second);
    assert!(
        first_manifest == second_manifest,
        "FAIL: the manifests differ"
    );
    assert!(first_sbom == second_sbom, "FAIL: the inventories differ");
    assert_eq!(first_sums, second_sums);
    assert!(
        names(&other_tmp).is_empty(),
        "FAIL: a prepared copy was left"
    );
}

#[test]
fn outputs_hold_no_host_path_and_no_excluded_file_content() {
    let scenario = Scenario::new();
    let source = scenario
        .base
        .join("source-CANARY-SOURCE-DIRECTORY")
        .join("project");
    copy_tree(scenario.source(), &source);
    let excluded: [(&str, &str); 5] = [
        (".env", "CANARY-DOT-ENV-CONTENT"),
        (".claude/settings.json", "CANARY-CLAUDE-SETTINGS-CONTENT"),
        ("front/.env.local", "CANARY-FRONT-ENV-CONTENT"),
        ("docs/x.md", "CANARY-DOCS-CONTENT"),
        ("src/notes.secret", "CANARY-SOURCE-SECRET-CONTENT"),
    ];
    for (relative, canary) in excluded {
        let path = source.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("SETUP: directory");
        fs::write(path, canary).expect("SETUP: excluded file");
    }
    let tmp = scenario.base.join("tmp-CANARY-TMPDIR-DIRECTORY");
    make_private_directory(&tmp);
    let artifacts = scenario.base.join("artifacts-CANARY-ARTIFACT-DIRECTORY");
    make_public_directories(&artifacts);
    let artifact = artifacts.join("agentsandbox");
    write_public_file(&artifact, b"artefact content is never copied\n");
    let parent = scenario.base.join("out-CANARY-OUTPUT-PARENT");
    make_private_directory(&parent);
    let output = parent.join("release");
    let before = tree_manifest(&source);

    let mut arguments: Vec<OsString> = vec![
        "release".into(),
        "manifest".into(),
        "--source".into(),
        source.clone().into_os_string(),
        "--output".into(),
        output.clone().into_os_string(),
    ];
    arguments.extend(artifact_arguments(&[artifact.as_path()]));
    let outcome = scenario.run(
        &arguments,
        &Options {
            tmpdir: tmp.clone(),
            umask: 0o022,
            current_dir: None,
        },
    );
    outcome.succeeded("release manifest with canaries");

    let base = scenario.base.to_string_lossy().into_owned();
    let mut canaries: Vec<&str> = excluded.iter().map(|(_, canary)| *canary).collect();
    canaries.extend([
        "CANARY-SOURCE-DIRECTORY",
        "CANARY-TMPDIR-DIRECTORY",
        "CANARY-ARTIFACT-DIRECTORY",
        "CANARY-OUTPUT-PARENT",
        "artefact content is never copied",
        base.as_str(),
    ]);
    for (name, document) in DOCUMENTS.iter().zip(documents(&output)) {
        let text = String::from_utf8_lossy(&document);
        for canary in &canaries {
            assert!(
                !text.contains(canary),
                "FAIL: {name} holds {canary:?}, a host path or excluded content"
            );
        }
    }
    let shown_output = output.to_string_lossy().into_owned();
    assert!(
        outcome.stdout.contains(&shown_output),
        "FAIL: the summary does not name the output as given: {}",
        outcome.describe()
    );
    let elsewhere = outcome.stdout.replace(&shown_output, "");
    for canary in &canaries {
        assert!(
            !elsewhere.contains(canary),
            "FAIL: stdout holds {canary:?} outside the output path: {}",
            outcome.describe()
        );
    }
    let manifest = manifest_json(&output);
    let listed: BTreeSet<&str> = manifest["source"]["entries"]
        .as_array()
        .expect("FAIL: entries")
        .iter()
        .map(|entry| string_field(&entry["path"], "an entry path"))
        .collect();
    for (relative, _) in excluded {
        assert!(!listed.contains(relative), "FAIL: {relative} is an entry");
    }
    assert!(
        names(&tmp).is_empty(),
        "FAIL: a prepared copy was left in TMPDIR"
    );
    assert!(
        tree_manifest(&source) == before,
        "FAIL: the source tree changed"
    );
}

#[test]
fn incomplete_or_unknown_lock_content_is_refused_before_any_output() {
    let repository_lock =
        fs::read_to_string(support::repository().join("Cargo.lock")).expect("SETUP: Cargo.lock");
    let first_checksum = repository_lock
        .lines()
        .find(|line| line.starts_with("checksum = "))
        .expect("SETUP: a checksum line")
        .to_owned();
    let repository_npm = fs::read_to_string(support::repository().join("front/package-lock.json"))
        .expect("SETUP: package-lock.json");
    let first_integrity = repository_npm
        .lines()
        .find(|line| line.trim_start().starts_with("\"integrity\": \""))
        .expect("SETUP: an integrity line")
        .to_owned();
    let registry = "source = \"registry+https://github.com/rust-lang/crates.io-index\"";
    let cases: [(&str, &str, String, String, &str); 12] = [
        (
            "dangling dependency",
            "Cargo.lock",
            " \"anyhow\",\n".into(),
            " \"absent-crate\",\n \"anyhow\",\n".into(),
            "absent-crate names no package",
        ),
        (
            "git source",
            "Cargo.lock",
            registry.into(),
            "source = \"git+https://example.invalid/crate#0\"".into(),
            "source other than the crates.io registry",
        ),
        (
            "missing checksum",
            "Cargo.lock",
            format!("{first_checksum}\n"),
            String::new(),
            "no checksum of 64",
        ),
        (
            "unknown Cargo key",
            "Cargo.lock",
            "[[package]]\nname = \"agentsandbox\"\n".into(),
            "[[package]]\nname = \"agentsandbox\"\nreplace = \"x\"\n".into(),
            "package key replace",
        ),
        (
            "lock version 3",
            "Cargo.lock",
            "version = 4".into(),
            "version = 3".into(),
            "must be 4",
        ),
        (
            "npm unknown key",
            "front/package-lock.json",
            "\"lockfileVersion\": 3,".into(),
            "\"lockfileVersion\": 3,\n  \"extraKey\": true,".into(),
            "unknown field",
        ),
        (
            "lockfileVersion 2",
            "front/package-lock.json",
            "\"lockfileVersion\": 3,".into(),
            "\"lockfileVersion\": 2,".into(),
            "lockfileVersion 2",
        ),
        (
            "missing integrity",
            "front/package-lock.json",
            format!("{first_integrity}\n"),
            String::new(),
            "has no integrity",
        ),
        (
            "nested node_modules",
            "front/package-lock.json",
            "\"node_modules/fdir\": {".into(),
            "\"node_modules/vite/node_modules/fdir\": {".into(),
            "nested",
        ),
        (
            "resolved URL with user information",
            "front/package-lock.json",
            "\"resolved\": \"https://registry.npmjs.org/".into(),
            "\"resolved\": \"https://user:CANARY-CREDENTIAL@registry.npmjs.org/".into(),
            "without user information",
        ),
        (
            "package.json and lock differ",
            "front/package.json",
            "\"vite\": \"7.3.7\"".into(),
            "\"vite\": \"7.3.7\",\n    \"left-pad\": \"1.3.0\"".into(),
            "does not match front/package.json",
        ),
        (
            "Cargo.toml and Cargo.lock root differ",
            "Cargo.toml",
            // The scenario uses the repository Cargo.toml (REPOSITORY_FILES):
            // edit its own package version, whatever its current value.
            format!("version = \"{}\"", env!("CARGO_PKG_VERSION")),
            "version = \"999.0.0\"".into(),
            "the lock is stale",
        ),
    ];
    for (label, file, from, to, fragment) in &cases {
        let scenario = Scenario::new();
        edit_file(&scenario.source().join(file), from, to);
        let before = tree_manifest(scenario.source());
        let (output, outcome) = scenario.manifest("refused", &[]);
        outcome.refused(fragment, label);
        let named: &str = if *file == "Cargo.toml" {
            "Cargo.toml declares package"
        } else {
            *file
        };
        assert!(
            outcome.stderr.contains(named),
            "FAIL: {label}: the refusal does not name {named}: {}",
            outcome.describe()
        );
        assert!(
            !outcome.stderr.contains("CANARY-CREDENTIAL"),
            "FAIL: {label}: a refusal repeats a credential-bearing URL"
        );
        assert_nothing_published(&output, label);
        scenario.assert_tmp_empty(label);
        assert!(
            tree_manifest(scenario.source()) == before,
            "FAIL: {label}: the source tree changed"
        );
    }
}

#[test]
fn output_must_be_a_new_directory_under_a_private_parent_outside_the_source() {
    let scenario = Scenario::new();
    let outputs = scenario.outputs.clone();
    let existing = outputs.join("existing");
    fs::create_dir(&existing).expect("SETUP: existing directory");
    fs::write(existing.join("kept"), b"kept\n").expect("SETUP: existing content");
    fs::write(outputs.join("file"), b"a file\n").expect("SETUP: existing file");
    let open = scenario.base.join("open");
    fs::create_dir(&open).expect("SETUP: open parent");
    fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).expect("SETUP: mode");
    let link = scenario.base.join("linked-outputs");
    symlink(&outputs, &link).expect("SETUP: symbolic link");
    fs::create_dir(outputs.join(".stale.release-staging")).expect("SETUP: stale staging");
    fs::set_permissions(scenario.source(), fs::Permissions::from_mode(0o700))
        .expect("SETUP: private source directory");
    let traversal = format!("{}/../outputs/release", outputs.display());
    let cases: [(&str, OsString, &str); 7] = [
        (
            "existing directory",
            existing.clone().into_os_string(),
            "already exists",
        ),
        (
            "existing file",
            outputs.join("file").into_os_string(),
            "already exists",
        ),
        (
            "parent 0755",
            open.join("release").into_os_string(),
            "mkdir -m 700",
        ),
        (
            "parent through a symbolic link",
            link.join("release").into_os_string(),
            "mkdir -m 700",
        ),
        (
            "parent traversal",
            OsString::from(traversal),
            "--output is refused",
        ),
        (
            "parent is the source",
            scenario.source().join("release").into_os_string(),
            "outside the source directory",
        ),
        (
            "stale staging",
            outputs.join("stale").into_os_string(),
            "staging directory of an earlier run",
        ),
    ];
    let before = tree_manifest(&outputs);
    let source_before = tree_manifest(scenario.source());
    for (label, output, fragment) in &cases {
        let outcome = scenario.run(
            [
                OsString::from("release"),
                "manifest".into(),
                "--source".into(),
                scenario.source().as_os_str().to_owned(),
                "--output".into(),
                output.clone(),
            ],
            &scenario.options(),
        );
        outcome.refused(fragment, label);
        scenario.assert_before_preparation(&outcome, label);
        assert!(
            tree_manifest(&outputs) == before,
            "FAIL: {label}: the output parent changed"
        );
        assert!(
            tree_manifest(scenario.source()) == source_before,
            "FAIL: {label}: the source changed"
        );
    }
    eprintln!(
        "NOT_RUN: a 0700 parent owned by another UID needs a second account, which this test \
         does not create"
    );
    // A relative output is resolved from the current directory.
    let outcome = scenario.run(
        [
            OsString::from("release"),
            "manifest".into(),
            "--source".into(),
            scenario.source().as_os_str().to_owned(),
            "--output".into(),
            "relative-release".into(),
        ],
        &Options {
            tmpdir: scenario.tmp().to_path_buf(),
            umask: 0o022,
            current_dir: Some(outputs.clone()),
        },
    );
    outcome.succeeded("a relative --output");
    let mut expected: Vec<String> = DOCUMENTS.iter().map(|name| (*name).to_owned()).collect();
    expected.sort();
    assert_eq!(names(&outputs.join("relative-release")), expected);
}

#[test]
fn an_interrupted_manifest_publishes_nothing() {
    const SPARSE_BYTES: u64 = 448 << 20;
    let scenario = Scenario::new();
    let mut arguments: Vec<OsString> = vec![
        "release".into(),
        "manifest".into(),
        "--source".into(),
        scenario.source().as_os_str().to_owned(),
        "--output".into(),
    ];
    let output = scenario.outputs.join("interrupted");
    arguments.push(output.clone().into_os_string());
    for index in 0..8 {
        let path = scenario.artifacts.join(format!("sparse-{index}.bin"));
        File::create(&path)
            .and_then(|file| file.set_len(SPARSE_BYTES))
            .expect("SETUP: sparse artefact");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("SETUP: mode");
        arguments.extend(artifact_arguments(&[path.as_path()]));
    }
    let mut running = scenario.spawn(&arguments, &scenario.options());
    let started = Instant::now();
    loop {
        if running.stderr_so_far().contains(PROVENANCE) {
            break;
        }
        if let Some(outcome) = running.poll() {
            panic!(
                "SETUP: the command ended before its source was prepared: {}",
                outcome.describe()
            );
        }
        assert!(
            started.elapsed() < WINDOW,
            "SETUP: no provenance line within {WINDOW:?}"
        );
        std::thread::sleep(harness::POLL);
    }
    let sent = running.signal(libc::SIGINT);
    let outcome = running
        .wait(Duration::from_secs(600))
        .unwrap_or_else(|| panic!("SETUP: the interrupted command is still pending"));
    assert!(
        sent && !outcome.status.success(),
        "SETUP: the command finished before the signal reached it, so nothing was interrupted: {}",
        outcome.describe()
    );
    outcome.refused(
        "interrupted by SIGINT before publication",
        "SIGINT after the preparation",
    );
    assert_nothing_published(&output, "after SIGINT");
    scenario.assert_tmp_empty("after SIGINT");
}

#[test]
fn expected_source_digest_is_checked_before_any_read_or_output() {
    let scenario = Scenario::new();
    let actual = distribution_digest(&scenario.build_inputs()).sha256;
    let other = sha256_hex(b"the digest of another source");
    assert_ne!(actual, other);
    let (output, outcome) = scenario.manifest(
        "expected",
        &arguments(&["--expect-source-digest", other.as_str()]),
    );
    outcome.refused(
        &format!("the prepared source digest {actual} differs from the expected {other}"),
        "another digest",
    );
    assert!(
        outcome.stderr.contains(PROVENANCE),
        "FAIL: the expectation was judged without a preparation"
    );
    assert_nothing_published(&output, "another digest");
    scenario.assert_tmp_empty("another digest");

    let uppercase = actual.to_ascii_uppercase();
    assert_ne!(uppercase, actual, "SETUP: the digest has letters");
    for malformed in [&actual[..63], uppercase.as_str(), "not-a-digest"] {
        let (output, outcome) = scenario.manifest(
            "malformed",
            &arguments(&["--expect-source-digest", malformed]),
        );
        outcome.refused(
            "--expect-source-digest must be 64 lowercase hexadecimal characters",
            malformed,
        );
        scenario.assert_before_preparation(&outcome, malformed);
        assert_nothing_published(&output, malformed);
    }
}

/// `cargo:<name>@<version>` → checksum, for every registry block of a
/// `Cargo.lock`, scanned by lines.
fn lock_checksums(text: &str) -> BTreeMap<String, String> {
    let field = |block: &str, key: &str| {
        block
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key} = \"")))
            .map(|rest| rest.trim_end_matches('"').to_owned())
    };
    text.split("[[package]]\n")
        .skip(1)
        .filter_map(|block| {
            let checksum = field(block, "checksum")?;
            Some((
                format!(
                    "cargo:{}@{}",
                    field(block, "name")?,
                    field(block, "version")?
                ),
                checksum,
            ))
        })
        .collect()
}

fn components_by_ref(sbom: &serde_json::Value) -> BTreeMap<String, serde_json::Value> {
    sbom["components"]
        .as_array()
        .expect("FAIL: components")
        .iter()
        .map(|component| {
            (
                string_field(&component["bom-ref"], "a bom-ref").to_owned(),
                component.clone(),
            )
        })
        .collect()
}

fn has_property(component: &serde_json::Value, name: &str, value: &str) -> bool {
    component["properties"]
        .as_array()
        .is_some_and(|properties| {
            properties
                .iter()
                .any(|property| property["name"] == name && property["value"] == value)
        })
}

#[test]
fn sbom_lists_locked_components_with_their_registry_hashes_and_names_what_locks_omit() {
    let scenario = Scenario::new();
    let output = scenario.published("inventory", &[]);
    let sbom = sbom_json(&output);
    let manifest = manifest_json(&output);
    assert_eq!(sbom["bomFormat"], "CycloneDX");
    assert_eq!(sbom["specVersion"], "1.6");
    assert_eq!(sbom["version"], 1);
    assert!(sbom.get("serialNumber").is_none());
    assert!(sbom["metadata"].get("timestamp").is_none());
    assert_eq!(
        sbom["metadata"]["lifecycles"],
        serde_json::json!([{ "phase": "pre-build" }])
    );
    let [_, sbom_bytes, _] = documents(&output);
    assert!(
        !String::from_utf8_lossy(&sbom_bytes).contains("\"purl\""),
        "FAIL: format 1 writes no package URL"
    );
    let components = components_by_ref(&sbom);

    let lock_text = fs::read_to_string(scenario.source().join("Cargo.lock")).expect("SETUP");
    let checksums = lock_checksums(&lock_text);
    let crates: Vec<(&String, &serde_json::Value)> = components
        .iter()
        .filter(|(reference, _)| reference.starts_with("cargo:"))
        .collect();
    assert_eq!(crates.len(), checksums.len(), "FAIL: crate components");
    for (reference, component) in &crates {
        assert_eq!(component["hashes"][0]["alg"], "SHA-256");
        assert_eq!(
            component["hashes"][0]["content"],
            checksums[*reference].as_str(),
            "FAIL: hash of {reference}"
        );
        assert!(
            component.get("scope").is_none(),
            "FAIL: {reference} has a scope"
        );
        assert!(
            component.get("licenses").is_none(),
            "FAIL: {reference} has a licence"
        );
        assert!(has_property(component, "ai-guard:scope", "unresolved"));
    }

    let npm_lock: serde_json::Value = serde_json::from_slice(
        &fs::read(scenario.source().join("front/package-lock.json")).expect("SETUP"),
    )
    .expect("SETUP: JSON");
    let entries = npm_lock["packages"].as_object().expect("SETUP: packages");
    let mut npm_count = 0;
    for (key, entry) in entries {
        let Some(name) = key.strip_prefix("node_modules/") else {
            continue;
        };
        npm_count += 1;
        let reference = format!(
            "npm:{name}@{}",
            string_field(&entry["version"], "a version")
        );
        let component = components
            .get(&reference)
            .unwrap_or_else(|| panic!("FAIL: {reference} is not listed"));
        let integrity = string_field(&entry["integrity"], "an integrity");
        let digest = STANDARD
            .decode(integrity.trim_start_matches("sha512-"))
            .expect("SETUP: base64");
        assert_eq!(component["hashes"][0]["alg"], "SHA-512");
        assert_eq!(
            component["hashes"][0]["content"],
            hex::encode(digest).as_str()
        );
        let dev = entry["dev"].as_bool() == Some(true);
        assert_eq!(
            component.get("scope").is_some(),
            dev,
            "FAIL: scope of {reference}"
        );
        if dev {
            assert_eq!(component["scope"], "excluded");
        }
        if let Some(license) = entry["license"].as_str() {
            assert_eq!(component["licenses"][0]["expression"], license);
            assert_eq!(component["licenses"][0]["acknowledgement"], "declared");
        }
        assert_eq!(component["externalReferences"][0]["type"], "distribution");
    }
    let npm_components = components
        .keys()
        .filter(|reference| reference.starts_with("npm:"))
        .count();
    assert_eq!(
        npm_components,
        npm_count + 1,
        "FAIL: npm components and the front root"
    );
    let front = format!(
        "npm:{}@{}",
        string_field(&npm_lock["name"], "the lock name"),
        string_field(&npm_lock["version"], "the lock version")
    );
    assert!(has_property(
        &components[&front],
        "ai-guard:first-party",
        "true"
    ));

    let images: Vec<_> = components
        .iter()
        .filter(|(reference, _)| reference.starts_with("image:"))
        .collect();
    assert_eq!(images.len(), 3, "FAIL: base images at this base");
    for (reference, image) in images {
        let (_, digest) = reference
            .rsplit_once("@sha256:")
            .expect("all external base images are pinned by digest");
        assert_eq!(image["type"], "container");
        assert!(has_property(image, "ai-guard:pinning", "digest"));
        assert!(has_property(
            image,
            "ai-guard:version-evidence",
            "declared:digest"
        ));
        assert_eq!(
            image["hashes"],
            serde_json::json!([{ "alg": "SHA-256", "content": digest }])
        );
    }
    let compositions = sbom["compositions"].as_array().expect("FAIL: compositions");
    assert!(compositions.contains(&serde_json::json!({
        "aggregate": "incomplete",
        "assemblies": [string_field(&manifest["package"]["name"], "the package name")]
    })));
    assert_eq!(
        sbom["metadata"]["component"]["bom-ref"],
        manifest["package"]["name"]
    );
    let metadata_properties = &sbom["metadata"];
    assert!(has_property(
        metadata_properties,
        "ai-guard:source-digest",
        string_field(&manifest["source"]["digest"]["value"], "the source digest")
    ));
    assert!(has_property(
        metadata_properties,
        "ai-guard:package-identifiers",
        "none"
    ));
    assert_eq!(manifest["sbom"]["sha256"], sha256_hex(&sbom_bytes).as_str());

    // Known answer: one synthetic package whose integrity is computed here.
    let tarball = b"synthetic tarball bytes of a release test";
    let synthetic = Scenario::new();
    fs::write(
        synthetic.source().join("front/package.json"),
        br#"{"name": "synthetic-front", "version": "1.0.0", "devDependencies": {"leaf": "1.0.0"}}"#,
    )
    .expect("SETUP: package.json");
    let lock = serde_json::json!({
        "name": "synthetic-front", "version": "1.0.0", "lockfileVersion": 3, "requires": true,
        "packages": {
            "": { "name": "synthetic-front", "version": "1.0.0",
                  "devDependencies": { "leaf": "1.0.0" } },
            "node_modules/leaf": {
                "version": "1.0.0",
                "resolved": "https://registry.npmjs.org/leaf/-/leaf-1.0.0.tgz",
                "integrity": format!("sha512-{}", STANDARD.encode(Sha512::digest(tarball))),
                "dev": true, "license": "SEE LICENSE IN LICENSE.md"
            }
        }
    });
    fs::write(
        synthetic.source().join("front/package-lock.json"),
        serde_json::to_vec_pretty(&lock).expect("JSON"),
    )
    .expect("SETUP: package-lock.json");
    let output = synthetic.published("known-answer", &[]);
    let components = components_by_ref(&sbom_json(&output));
    let leaf = &components["npm:leaf@1.0.0"];
    assert_eq!(
        leaf["hashes"][0]["content"],
        hex::encode(Sha512::digest(tarball)).as_str()
    );
    assert_eq!(
        leaf["licenses"][0],
        serde_json::json!({ "license": {
            "name": "SEE LICENSE IN LICENSE.md", "acknowledgement": "declared" } })
    );
}

#[test]
fn declared_provider_versions_change_the_build_input_identity_and_are_marked_declared() {
    let scenario = Scenario::new();
    let digest = distribution_digest(&scenario.build_inputs()).sha256;
    let artifact = scenario.artifact("agentsandbox", b"declared artefact\n");
    let none = scenario.published("none", &[]);
    let mut codex_arguments = arguments(&["--codex-version", "1.2.3"]);
    codex_arguments.extend(artifact_arguments(&[artifact.as_path()]));
    let codex = scenario.published("codex", &codex_arguments);
    let claude = scenario.published("claude", &arguments(&["--claude-code-version", "1.2.3"]));
    let identity = |output: &Path| {
        string_field(
            &manifest_json(output)["build"]["inputs_identity"]["value"],
            "the build-input identity",
        )
        .to_owned()
    };
    let identities = [
        identity(none.as_path()),
        identity(codex.as_path()),
        identity(claude.as_path()),
    ];
    assert_eq!(identities[0], inputs_identity(&digest, None, None));
    assert_eq!(identities[1], inputs_identity(&digest, Some("1.2.3"), None));
    assert_eq!(identities[2], inputs_identity(&digest, None, Some("1.2.3")));
    assert_eq!(identities.iter().collect::<BTreeSet<_>>().len(), 3);
    for output in [&none, &codex, &claude] {
        let manifest = manifest_json(output);
        assert_eq!(manifest["build"]["target"]["status"], "unknown");
        assert_eq!(
            manifest["build"]["inputs_identity"]["layout"],
            "ai-guard:build-inputs:v1"
        );
    }
    let manifest = manifest_json(&codex);
    let bound = &manifest["artifacts"][0];
    assert_eq!(bound["profile"]["status"], "unknown");
    assert_eq!(bound["target"]["status"], "unknown");
    let client = &manifest["build"]["provider_clients"];
    assert_eq!(client.as_array().map(Vec::len), Some(1));
    assert_eq!(client[0]["build_argument"], "CODEX_VERSION");
    assert_eq!(client[0]["requested_version"], "1.2.3");
    assert_eq!(client[0]["package"]["status"], "declared");
    assert_eq!(client[0]["package"]["value"], "@openai/codex");
    assert!(string_field(&client[0]["evidence"], "the evidence").contains("declared"));
    let components = components_by_ref(&sbom_json(&codex));
    assert!(has_property(
        &components["provider:@openai/codex@1.2.3"],
        "ai-guard:version-evidence",
        "declared:build-argument"
    ));
    assert_eq!(
        manifest_json(&none)["build"]["provider_clients"],
        serde_json::json!([])
    );

    for (option, version) in [
        ("--codex-version", "latest"),
        ("--claude-code-version", "1.2.3-rc.1+b.5"),
    ] {
        let (output, outcome) =
            scenario.manifest("refused-version", &arguments(&[option, version]));
        outcome.refused(&format!("{option} is refused"), version);
        scenario.assert_before_preparation(&outcome, version);
        assert_nothing_published(&output, version);
    }
}

#[test]
fn artifacts_are_bound_by_name_size_and_sha256_and_unsafe_ones_are_refused() {
    let scenario = Scenario::new();
    let plain = scenario.artifact("agentsandbox", &b"plain artefact\n".repeat(100));
    let linked = scenario.artifact("ai-guard-diagnose", b"hard-linked artefact\n");
    fs::hard_link(
        &linked,
        scenario.artifacts.join("ai-guard-diagnose-second-name"),
    )
    .expect("SETUP: hard link");
    assert_eq!(fs::metadata(&linked).expect("SETUP").nlink(), 2);
    let output = scenario.published(
        "bound",
        &artifact_arguments(&[plain.as_path(), linked.as_path()]),
    );
    let manifest = manifest_json(&output);
    let artifacts = manifest["artifacts"].as_array().expect("FAIL: artifacts");
    assert_eq!(artifacts.len(), 2);
    for (entry, path) in artifacts.iter().zip([&plain, &linked]) {
        let bytes = fs::read(path).expect("SETUP");
        assert_eq!(
            entry["name"],
            path.file_name()
                .expect("name")
                .to_string_lossy()
                .into_owned()
        );
        assert_eq!(entry["bytes"], bytes.len() as u64);
        assert_eq!(entry["sha256"], sha256_hex(&bytes).as_str());
        let keys: BTreeSet<&str> = entry
            .as_object()
            .expect("FAIL: an artefact entry")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from(["bytes", "evidence", "name", "profile", "sha256", "target"]),
            "FAIL: an artefact records more than its name, length and digest"
        );
    }

    let symbolic = scenario.artifacts.join("symbolic");
    symlink(&plain, &symbolic).expect("SETUP: symbolic link");
    let writable = scenario.artifact("writable", b"writable\n");
    fs::set_permissions(&writable, fs::Permissions::from_mode(0o666)).expect("SETUP: mode");
    let open = scenario.base.join("open-artifacts");
    fs::create_dir(&open).expect("SETUP: open directory");
    fs::set_permissions(&open, fs::Permissions::from_mode(0o777)).expect("SETUP: mode");
    let under_open = open.join("agentsandbox");
    fs::write(&under_open, b"under an open directory\n").expect("SETUP: artefact");
    let fifo = scenario.artifacts.join("fifo");
    let fifo_name = CString::new(fifo.as_os_str().as_bytes()).expect("SETUP: path");
    // SAFETY: a NUL-terminated path and a mode; no pointer outlives the call.
    assert_eq!(
        unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o644) },
        0,
        "SETUP: FIFO"
    );
    let directory = scenario.artifacts.join("directory");
    fs::create_dir(&directory).expect("SETUP: directory");
    let dotenv = scenario.artifact(".env", b"SYNTHETIC=1\n");
    let elsewhere = scenario.base.join("elsewhere");
    fs::create_dir(&elsewhere).expect("SETUP: directory");
    let duplicate = elsewhere.join("agentsandbox");
    fs::write(&duplicate, b"another agentsandbox\n").expect("SETUP: artefact");
    let after_preparation: [(&str, Vec<&Path>, &str); 5] = [
        (
            "symbolic link",
            vec![symbolic.as_path()],
            "is unavailable or is a symbolic link",
        ),
        (
            "mode 0666",
            vec![writable.as_path()],
            "must not be writable by group or others",
        ),
        (
            "parent 0777",
            vec![under_open.as_path()],
            "unsafe export parent or ancestor",
        ),
        ("FIFO", vec![fifo.as_path()], "must be a regular file"),
        (
            "directory",
            vec![directory.as_path()],
            "must be a regular file",
        ),
    ];
    for (label, paths, fragment) in &after_preparation {
        let (output, outcome) = scenario.manifest("unsafe", &artifact_arguments(paths));
        outcome.refused(fragment, label);
        assert_nothing_published(&output, label);
        scenario.assert_tmp_empty(label);
    }
    let before_preparation: [(&str, Vec<&Path>, &str); 2] = [
        (".env name", vec![dotenv.as_path()], "artefact name"),
        (
            "one name twice",
            vec![plain.as_path(), duplicate.as_path()],
            "two artefacts have the name",
        ),
    ];
    for (label, paths, fragment) in &before_preparation {
        let (output, outcome) = scenario.manifest("unsafe", &artifact_arguments(paths));
        outcome.refused(fragment, label);
        scenario.assert_before_preparation(&outcome, label);
        assert_nothing_published(&output, label);
    }
}

#[test]
fn the_cli_just_built_by_cargo_is_bound_as_an_artifact() {
    let scenario = Scenario::new();
    let binary = Path::new(env!("CARGO_BIN_EXE_agentsandbox"));
    // Observations, not assertions: how Cargo placed its output is not this
    // command's contract.
    eprintln!("observation: {}", link_facts(binary));
    if let Some(deps) = binary.parent().map(|directory| directory.join("deps"))
        && let Ok(entries) = fs::read_dir(deps)
    {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("agentsandbox-") && !name.contains('.') {
                eprintln!("observation: {}", link_facts(&entry.path()));
            }
        }
    }
    let (output, outcome) = scenario.manifest("cli", &artifact_arguments(&[binary]));
    if !outcome.status.success() {
        // Only an artefact-policy refusal of where and how Cargo wrote the
        // file is a fact of the environment: its directories, filesystem,
        // owner, a symbolic link on its path, or the mode the build umask
        // gave it. Anything else is a product failure.
        let environment = [
            "unsafe export parent or ancestor",
            "filesystem type is outside",
            "reached through a symbolic link",
            "must belong to root or to the effective UID",
            "must not be writable by group or others",
        ];
        if outcome.status.code() == Some(1)
            && environment
                .iter()
                .any(|fragment| outcome.stderr.contains(fragment))
        {
            panic!(
                "ENVIRONMENT: the artefact reader refused the CLI Cargo built here; record this \
                 message as a fact of the test environment, not as a PASS: {}",
                outcome.describe()
            );
        }
        panic!(
            "FAIL: release manifest did not bind the CLI Cargo built here: {}",
            outcome.describe()
        );
    }
    let manifest = manifest_json(&output);
    let entry = &manifest["artifacts"][0];
    let bytes = fs::read(binary).expect("SETUP: the test binary");
    assert_eq!(entry["name"], "agentsandbox");
    assert_eq!(entry["bytes"], fs::metadata(binary).expect("SETUP").len());
    assert_eq!(entry["bytes"], bytes.len() as u64);
    assert_eq!(entry["sha256"], sha256_hex(&bytes).as_str());
}
