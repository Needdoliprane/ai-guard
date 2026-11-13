//! `agentsandbox release verify` end to end, with the real binary: a match
//! only when every comparison holds, a divergence with its category for a
//! format-1 manifest of this CLI, and `UNVERIFIED` for what this CLI cannot
//! judge, never one for the other.
#![cfg(unix)]

#[path = "source_lifecycle/harness.rs"]
mod harness;
#[path = "release_metadata/support.rs"]
mod support;

use std::{
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
use support::{
    MANIFEST_FILE, Outcome, PROVENANCE, SBOM_FILE, SUMS_FILE, Scenario, artifact_arguments,
    copy_documents, copy_tree, edit_file, make_public_directories, manifest_json, rewrite_manifest,
    sha256_hex, sums_text,
};

/// One change applied to a fresh copy of a source.
type Change = Box<dyn Fn(&Path)>;

fn change(apply: impl Fn(&Path) + 'static) -> Change {
    Box::new(apply)
}

/// Appends to `Cargo.lock` a table that release manifest format 1 refuses:
/// the source still prepares, but its lock can no longer be read.
fn append_refused_lock_table(source: &Path) {
    let path = source.join("Cargo.lock");
    let mut text = fs::read_to_string(&path).expect("SETUP: Cargo.lock");
    text.push_str("\n[metadata]\n");
    fs::write(path, text).expect("SETUP: Cargo.lock");
}

/// `hex` with its first character replaced by another hexadecimal digit.
fn changed_hex(hex: &str) -> String {
    let mut characters: Vec<char> = hex.chars().collect();
    characters[0] = if characters[0] == '0' { '1' } else { '0' };
    characters.into_iter().collect()
}

/// A published manifest of the scenario source, binding two artefacts.
struct Published {
    scenario: Scenario,
    manifest: PathBuf,
    artifacts: Vec<PathBuf>,
}

fn published(further: &[OsString]) -> Published {
    let scenario = Scenario::new();
    let artifacts = vec![
        scenario.artifact("agentsandbox", &b"control CLI bytes\n".repeat(50)),
        scenario.artifact("ai-guard-diagnose", b"diagnostic bytes\n"),
    ];
    let mut arguments = artifact_arguments(&[artifacts[0].as_path(), artifacts[1].as_path()]);
    arguments.extend(further.iter().cloned());
    let manifest = scenario.published("release", &arguments);
    Published {
        scenario,
        manifest,
        artifacts,
    }
}

fn assert_match(outcome: &Outcome, context: &str) {
    outcome.succeeded(context);
    assert_eq!(
        outcome.first_stdout_line(),
        "release verify: MATCH",
        "FAIL: {context}: {}",
        outcome.describe()
    );
}

fn assert_divergent(outcome: &Outcome, category: &str, context: &str) {
    outcome.refused(&format!("DIVERGENT {category}:"), context);
    assert!(
        !outcome.stderr.contains("UNVERIFIED"),
        "FAIL: {context}: a divergence is also called unverified: {}",
        outcome.describe()
    );
    assert!(
        !outcome.stdout.lines().any(|line| line.contains("MATCH")),
        "FAIL: {context}: a divergence printed a match: {}",
        outcome.describe()
    );
}

fn assert_unverified(outcome: &Outcome, fragment: &str, context: &str) {
    outcome.refused("UNVERIFIED", context);
    assert!(
        outcome.stderr.contains(fragment),
        "FAIL: {context}: the reason {fragment:?} is not named: {}",
        outcome.describe()
    );
    assert!(
        !outcome.stderr.contains("DIVERGENT"),
        "FAIL: {context}: an unverified input is called divergent: {}",
        outcome.describe()
    );
    assert!(
        !outcome.stdout.lines().any(|line| line.contains("MATCH")),
        "FAIL: {context}: an unverified input printed a match: {}",
        outcome.describe()
    );
}

fn artefact_arguments_of(paths: &[PathBuf]) -> Vec<OsString> {
    let paths: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
    artifact_arguments(&paths)
}

/// A fresh copy of the manifest directory of `published`, at `name`.
fn manifest_copy(published: &Published, name: &str) -> PathBuf {
    let copy = published.scenario.base.join("copies").join(name);
    copy_documents(&published.manifest, &copy);
    copy
}

/// A fresh copy of the source of `published`, at `name`.
fn source_copy(published: &Published, name: &str) -> PathBuf {
    let copy = published.scenario.base.join("sources").join(name);
    copy_tree(published.scenario.source(), &copy);
    copy
}

#[test]
fn verify_matches_an_unchanged_source_and_its_artifacts_from_another_location() {
    let published = published(&[]);
    let scenario = &published.scenario;
    let moved = scenario.base.join("moved").join("project");
    fs::create_dir_all(moved.parent().expect("parent")).expect("SETUP: directory");
    fs::rename(scenario.source(), &moved).expect("SETUP: the source is moved");
    let copied = scenario.base.join("copied").join("release");
    copy_documents(&published.manifest, &copied);
    let elsewhere = scenario.base.join("elsewhere");
    make_public_directories(&elsewhere);
    let mut artifacts = Vec::new();
    for path in &published.artifacts {
        let copy = elsewhere.join(path.file_name().expect("name"));
        fs::copy(path, &copy).expect("SETUP: artefact copy");
        artifacts.push(copy);
    }
    let outcome = scenario.verify(&moved, &copied, &artefact_arguments_of(&artifacts));
    assert_match(&outcome, "an unchanged source and its artefacts elsewhere");
    assert!(
        outcome.stdout.contains("artifacts  2/2 matched"),
        "FAIL: {}",
        outcome.describe()
    );
    assert!(
        outcome.stdout.contains("target     unknown (not observed)"),
        "FAIL: {}",
        outcome.describe()
    );
    scenario.assert_tmp_empty("after release verify");
}

#[test]
fn one_changed_byte_in_a_bound_input_is_refused_with_its_category() {
    let published = published(&[]);
    let scenario = &published.scenario;
    let all = artefact_arguments_of(&published.artifacts);
    let control = scenario.verify(scenario.source(), &published.manifest, &all);
    assert_match(&control, "the control case");

    let lock = fs::read_to_string(scenario.source().join("Cargo.lock")).expect("SETUP");
    let checksum_line = lock
        .lines()
        .find(|line| line.starts_with("checksum = \""))
        .expect("SETUP: a checksum line")
        .to_owned();
    let changed_checksum = {
        let mut characters: Vec<char> = checksum_line.chars().collect();
        let position = "checksum = \"".len();
        characters[position] = if characters[position] == 'a' {
            'b'
        } else {
            'a'
        };
        characters.into_iter().collect::<String>()
    };
    let npm = fs::read_to_string(scenario.source().join("front/package-lock.json")).expect("SETUP");
    let integrity_line = npm
        .lines()
        .find(|line| line.contains("\"integrity\": \"sha512-"))
        .expect("SETUP: an integrity line")
        .to_owned();
    let changed_integrity = {
        let start = integrity_line.find("sha512-").expect("SETUP") + "sha512-".len();
        let mut characters: Vec<char> = integrity_line.chars().collect();
        characters[start] = if characters[start] == 'A' { 'B' } else { 'A' };
        characters.into_iter().collect::<String>()
    };
    let sources: [(&str, &str, Change); 7] = [
        (
            "one byte of a Rust source",
            "src/main.rs",
            change(|source: &Path| {
                let path = source.join("src/main.rs");
                let mut bytes = fs::read(&path).expect("SETUP");
                bytes[0] ^= 0x01;
                fs::write(path, bytes).expect("SETUP");
            }),
        ),
        (
            "one hexadecimal character of a Cargo.lock checksum",
            "Cargo.lock",
            change(move |source: &Path| {
                edit_file(
                    &source.join("Cargo.lock"),
                    &checksum_line,
                    &changed_checksum,
                )
            }),
        ),
        (
            "one base64 character of an npm integrity",
            "front/package-lock.json",
            change(move |source: &Path| {
                edit_file(
                    &source.join("front/package-lock.json"),
                    &integrity_line,
                    &changed_integrity,
                )
            }),
        ),
        (
            "one byte of a recipe",
            "images/control.Dockerfile",
            change(|source: &Path| {
                edit_file(
                    &source.join("images/control.Dockerfile"),
                    "WORKDIR /build/front",
                    "WORKDIR /build/fronT",
                )
            }),
        ),
        (
            "an added allowlisted file",
            "src/added.rs",
            change(|source: &Path| {
                fs::write(source.join("src/added.rs"), b"// added\n").expect("SETUP")
            }),
        ),
        (
            "a removed allowlisted file that is not required",
            "src/nested/secrets/extra.rs",
            change(|source: &Path| {
                fs::remove_file(source.join("src/nested/secrets/extra.rs")).expect("SETUP")
            }),
        ),
        (
            "a Cargo.lock table that format 1 cannot read",
            "Cargo.lock",
            change(|source: &Path| append_refused_lock_table(source)),
        ),
    ];
    for (index, (label, named, apply)) in sources.iter().enumerate() {
        let source = source_copy(&published, &format!("case-{index}"));
        apply(source.as_path());
        let outcome = scenario.verify(&source, &published.manifest, &all);
        assert_divergent(&outcome, "source", label);
        assert!(
            outcome.stderr.contains(named),
            "FAIL: {label}: the divergence does not name {named}: {}",
            outcome.describe()
        );
    }

    // The lock table of the last case is one that format 1 refuses: a manifest
    // of that source is refused, while verify names the source difference
    // before reading the lock, instead of calling the source unreadable.
    let unreadable_lock = source_copy(&published, "unreadable-lock");
    append_refused_lock_table(&unreadable_lock);
    let outcome = scenario.run(
        [
            OsString::from("release"),
            "manifest".into(),
            "--source".into(),
            unreadable_lock.into_os_string(),
            "--output".into(),
            scenario.outputs.join("refused-lock").into_os_string(),
        ],
        &scenario.options(),
    );
    outcome.refused(
        "release manifest format 1 accepts only a complete lock it can resolve",
        "a manifest of a source whose lock format 1 cannot read",
    );

    let required = source_copy(&published, "required");
    fs::remove_file(required.join("front/src/style.css")).expect("SETUP");
    let outcome = scenario.verify(&required, &published.manifest, &all);
    assert_unverified(&outcome, "--source", "a removed required file");

    let changed = scenario.base.join("changed-artifact");
    make_public_directories(&changed);
    let mut artifacts = Vec::new();
    for path in &published.artifacts {
        let copy = changed.join(path.file_name().expect("name"));
        fs::copy(path, &copy).expect("SETUP: artefact copy");
        artifacts.push(copy);
    }
    let mut bytes = fs::read(&artifacts[0]).expect("SETUP");
    bytes[0] ^= 0x01;
    fs::write(&artifacts[0], bytes).expect("SETUP");
    let outcome = scenario.verify(
        scenario.source(),
        &published.manifest,
        &artefact_arguments_of(&artifacts),
    );
    assert_divergent(&outcome, "artifact-changed", "one byte of an artefact");
    assert!(
        outcome.stderr.contains("agentsandbox"),
        "FAIL: {}",
        outcome.describe()
    );

    let sbom = manifest_copy(&published, "sbom-byte");
    let mut bytes = fs::read(sbom.join(SBOM_FILE)).expect("SETUP");
    bytes[0] = b' ';
    fs::write(sbom.join(SBOM_FILE), bytes).expect("SETUP");
    let outcome = scenario.verify(scenario.source(), &sbom, &all);
    assert_divergent(&outcome, "sums", "one byte of sbom.cdx.json");

    let lock_count = manifest_copy(&published, "lock-count");
    let packages = manifest_json(&lock_count)["locks"]["cargo"]["packages"]
        .as_u64()
        .expect("FAIL: a package count");
    rewrite_manifest(&lock_count, |text| {
        text.replacen(
            &format!("\"packages\": {packages},"),
            &format!("\"packages\": {},", packages + 1),
            1,
        )
    });
    let outcome = scenario.verify(scenario.source(), &lock_count, &all);
    assert_divergent(&outcome, "lock", "a lock count edited with sums recomputed");
    assert!(
        outcome.stderr.contains("Cargo.lock packages"),
        "FAIL: {}",
        outcome.describe()
    );

    let identity = manifest_copy(&published, "inputs-identity");
    let value = manifest_json(&identity)["build"]["inputs_identity"]["value"]
        .as_str()
        .expect("FAIL: a build-input identity")
        .to_owned();
    rewrite_manifest(&identity, |text| {
        text.replacen(&value, &changed_hex(&value), 1)
    });
    let outcome = scenario.verify(scenario.source(), &identity, &all);
    assert_divergent(
        &outcome,
        "inputs-identity",
        "one character of build.inputs_identity with sums recomputed",
    );

    let recipe = manifest_copy(&published, "recipe-digest");
    let recipe_digest = manifest_json(&recipe)["build"]["recipes"][0]["sha256"]
        .as_str()
        .expect("FAIL: a recipe digest")
        .to_owned();
    rewrite_manifest(&recipe, |text| {
        // The same digest is listed earlier in source.entries: only the one
        // in build.recipes is changed.
        let start = text.find("\"recipes\": [").expect("SETUP: build.recipes");
        let (head, tail) = text.split_at(start);
        format!(
            "{head}{}",
            tail.replacen(&recipe_digest, &changed_hex(&recipe_digest), 1)
        )
    });
    let outcome = scenario.verify(scenario.source(), &recipe, &all);
    assert_divergent(
        &outcome,
        "declared",
        "one character of a recipe digest in build.recipes with sums recomputed",
    );
    assert!(
        outcome.stderr.contains("build.recipes"),
        "FAIL: {}",
        outcome.describe()
    );

    let inventory = manifest_copy(&published, "inventory");
    let sbom_path = inventory.join(SBOM_FILE);
    let before = fs::read(&sbom_path).expect("SETUP: SBOM");
    edit_file(
        &sbom_path,
        "\"bomFormat\": \"CycloneDX\"",
        "\"bomFormat\": \"CycloneDx\"",
    );
    let after = fs::read(&sbom_path).expect("SETUP: SBOM");
    assert_eq!(before.len(), after.len(), "SETUP: one byte was replaced");
    rewrite_manifest(&inventory, |text| {
        text.replacen(&sha256_hex(&before), &sha256_hex(&after), 1)
    });
    let outcome = scenario.verify(scenario.source(), &inventory, &all);
    assert_divergent(
        &outcome,
        "inventory",
        "one byte of sbom.cdx.json with its record and sums recomputed",
    );
}

#[test]
fn verify_requires_every_listed_artifact_unless_source_only_is_explicit() {
    let published = published(&[]);
    let scenario = &published.scenario;
    let source = scenario.source();
    let first_only = artefact_arguments_of(&published.artifacts[..1]);
    let outcome = scenario.verify(source, &published.manifest, &first_only);
    assert_divergent(&outcome, "artifact-missing", "one listed artefact missing");
    assert!(
        outcome.stderr.contains("ai-guard-diagnose"),
        "FAIL: {}",
        outcome.describe()
    );

    let extra = scenario.artifact("ai-guard-audit-checkpoint", b"not listed\n");
    let mut with_extra = published.artifacts.clone();
    with_extra.push(extra);
    let outcome = scenario.verify(
        source,
        &published.manifest,
        &artefact_arguments_of(&with_extra),
    );
    assert_divergent(&outcome, "artifact-unlisted", "one unlisted artefact");
    assert!(
        outcome.stderr.contains("ai-guard-audit-checkpoint"),
        "FAIL: {}",
        outcome.describe()
    );

    let renamed = scenario.base.join("renamed");
    make_public_directories(&renamed);
    let renamed_first = renamed.join("agentsandbox-renamed");
    fs::copy(&published.artifacts[0], &renamed_first).expect("SETUP: copy");
    let outcome = scenario.verify(
        source,
        &published.manifest,
        &artefact_arguments_of(&[renamed_first, published.artifacts[1].clone()]),
    );
    assert_divergent(
        &outcome,
        "artifact-missing",
        "the same bytes under another name",
    );

    let outcome = scenario.verify(
        source,
        &published.manifest,
        &[OsString::from("--source-only")],
    );
    outcome.succeeded("--source-only");
    assert_eq!(
        outcome.first_stdout_line(),
        "release verify: SOURCE MATCH; artifacts NOT VERIFIED (2 listed)",
        "FAIL: {}",
        outcome.describe()
    );
    assert!(
        !outcome
            .stdout
            .lines()
            .any(|line| line == "release verify: MATCH"),
        "FAIL: --source-only printed the heading of a full match: {}",
        outcome.describe()
    );
    let outcome = scenario.verify(source, &published.manifest, &{
        let mut arguments = vec![OsString::from("--source-only")];
        arguments.extend(first_only.iter().cloned());
        arguments
    });
    assert_unverified(&outcome, "--source-only", "--source-only with --artifact");
}

#[test]
fn a_tampered_format_1_manifest_is_divergent_format() {
    let published = published(&[OsString::from("--codex-version"), OsString::from("1.2.3")]);
    let scenario = &published.scenario;
    let all = artefact_arguments_of(&published.artifacts);
    let control = scenario.verify(scenario.source(), &published.manifest, &all);
    assert_match(&control, "the control case");

    let unknown = manifest_copy(&published, "unknown-field");
    rewrite_manifest(&unknown, |text| {
        text.replacen("{\n  ", "{\n  \"extra\": 1,\n  ", 1)
    });
    let whitespace = manifest_copy(&published, "whitespace");
    rewrite_manifest(&whitespace, |text| text.replacen("\n  ", "\n   ", 1));
    let version = manifest_copy(&published, "refused-version");
    rewrite_manifest(&version, |text| {
        text.replacen(
            "\"requested_version\": \"1.2.3\"",
            "\"requested_version\": \"1.2.3-rc.1+b.5\"",
            1,
        )
    });
    let sums = manifest_copy(&published, "third-line");
    let manifest = fs::read(sums.join(MANIFEST_FILE)).expect("SETUP");
    let sbom = fs::read(sums.join(SBOM_FILE)).expect("SETUP");
    fs::write(
        sums.join(SUMS_FILE),
        format!("{}{}  extra\n", sums_text(&manifest, &sbom), "0".repeat(64)),
    )
    .expect("SETUP");
    for (label, directory) in [
        ("an unknown field", &unknown),
        ("non-canonical whitespace", &whitespace),
        ("a provider version the recipe refuses", &version),
        ("a third line in SHA256SUMS", &sums),
    ] {
        let outcome = scenario.verify(scenario.source(), directory, &all);
        assert_divergent(&outcome, "format", label);
        assert!(
            !outcome.stderr.contains(PROVENANCE),
            "FAIL: {label}: a malformed manifest was judged after preparing the source"
        );
    }
}

#[test]
fn a_manifest_this_cli_cannot_judge_is_unverified() {
    let published = published(&[]);
    let scenario = &published.scenario;
    let all = artefact_arguments_of(&published.artifacts);
    let manifest = manifest_json(&published.manifest);
    let producer_version = manifest["producer"]["version"]
        .as_str()
        .expect("FAIL: a producer version")
        .to_owned();
    let allowlist = manifest["producer"]["allowlist_sha256"]
        .as_str()
        .expect("FAIL: an allowlist digest")
        .to_owned();

    let format_version = manifest_copy(&published, "format-version");
    rewrite_manifest(&format_version, |text| {
        text.replacen("\"format_version\": 1,", "\"format_version\": 2,", 1)
    });
    let other_version = manifest_copy(&published, "producer-version");
    rewrite_manifest(&other_version, |text| {
        text.replacen(
            &format!("\"version\": \"{producer_version}\""),
            "\"version\": \"9.9.9\"",
            1,
        )
    });
    let other_allowlist = manifest_copy(&published, "producer-allowlist");
    rewrite_manifest(&other_allowlist, |text| {
        text.replacen(&allowlist, &"f".repeat(64), 1)
    });
    let other_format = manifest_copy(&published, "format");
    rewrite_manifest(&other_format, |text| {
        text.replacen(
            "\"format\": \"ai-guard.release-manifest\"",
            "\"format\": \"another.manifest\"",
            1,
        )
    });
    let not_json = manifest_copy(&published, "not-json");
    rewrite_manifest(&not_json, |_| "not a JSON document\n".to_owned());
    for (label, directory, reason) in [
        (
            "format_version 2",
            &format_version,
            "unsupported format_version 2",
        ),
        (
            "another producer version",
            &other_version,
            "produced by agentsandbox 9.9.9",
        ),
        (
            "another allowlist",
            &other_allowlist,
            "produced by agentsandbox",
        ),
        (
            "another format",
            &other_format,
            "not an ai-guard release manifest",
        ),
        ("not JSON", &not_json, "not an ai-guard release manifest"),
    ] {
        let outcome = scenario.verify(scenario.source(), directory, &all);
        assert_unverified(&outcome, reason, label);
        scenario.assert_before_preparation(&outcome, label);
    }
}

#[test]
fn an_unreadable_input_is_unverified_not_divergent() {
    let published = published(&[]);
    let scenario = &published.scenario;
    let all = artefact_arguments_of(&published.artifacts);

    let mut absent = published.artifacts.clone();
    absent[1] = scenario.artifacts.join("absent-artefact");
    let outcome = scenario.verify(
        scenario.source(),
        &published.manifest,
        &artefact_arguments_of(&absent),
    );
    assert_unverified(&outcome, "--artifact", "an absent artefact path");

    let writable = manifest_copy(&published, "group-writable");
    fs::set_permissions(&writable, fs::Permissions::from_mode(0o775)).expect("SETUP: mode");
    let outcome = scenario.verify(scenario.source(), &writable, &all);
    assert_unverified(
        &outcome,
        MANIFEST_FILE,
        "a group-writable manifest directory",
    );

    let missing = manifest_copy(&published, "missing-sums");
    fs::remove_file(missing.join(SUMS_FILE)).expect("SETUP");
    let outcome = scenario.verify(scenario.source(), &missing, &all);
    assert_unverified(&outcome, SUMS_FILE, "a missing SHA256SUMS");

    let linked = manifest_copy(&published, "hard-linked");
    let elsewhere = scenario.base.join("linked-manifest.json");
    fs::rename(linked.join(MANIFEST_FILE), &elsewhere).expect("SETUP");
    fs::hard_link(elsewhere, linked.join(MANIFEST_FILE)).expect("SETUP: hard link");
    let outcome = scenario.verify(scenario.source(), &linked, &all);
    assert_unverified(&outcome, MANIFEST_FILE, "a hard-linked manifest file");
}
