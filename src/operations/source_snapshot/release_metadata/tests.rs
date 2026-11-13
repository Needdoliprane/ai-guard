//! Unit tests of the release metadata: the TOML subset, the strict lock
//! readers, the declared facts of recipes, the provider-version rule, the
//! published image labels, the licence forms, the artefact reader, the
//! unknown-durability publication path, the canonical manifest, the
//! classification of failed verify steps and the retained-entry accessors.
//!
//! Real repository files are read with `include_bytes!`; their oracles are
//! independent line scans or `serde_json::Value` traversals, not the readers
//! under test. Synthetic inputs are written by each test. The prepared source
//! of the last two tests is made by `capture` and `confirm` on a fixture of
//! this file: test code only.
use super::super::capture::{self, Limits, Scope, Snapshot};
use super::{
    Derived, LockRefusal, MAX_DECLARATION_BYTES, Refusal, StepLost,
    artifact::{self, AFTER_READ, ArtifactDigest, BEFORE_OPEN, OPENED},
    cargo_lock,
    compare::{self, Unverified},
    cyclonedx,
    declared::{self, Known},
    document::{self, DeclaredInputs},
    not_judged, npm_lock, output,
    toml_subset::{self, Item, Refused, Value},
};
use crate::{
    control_fs::{self, publication::FAIL_AFTER_RENAME},
    runtime::retained_cli::{Interruption, InvocationFailure},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::json;
use sha2::{Digest, Sha256, Sha512};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CString, OsStr, OsString},
    fs,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

const CARGO_LOCK: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.lock"));
const CARGO_TOML: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
const TOOLCHAIN: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/rust-toolchain.toml"));
const NPM_LOCK: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/front/package-lock.json"
));
const NPM_MANIFEST: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/front/package.json"));
const CONTROL_RECIPE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/images/control.Dockerfile"
));
const WORKER_RECIPE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/images/worker.Dockerfile"
));
const DOCKERIGNORE: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/.dockerignore"));

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("a repository file is UTF-8")
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// A new 0700 directory and its canonical path.
fn private_directory() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("private test directory");
    let base = root
        .path()
        .canonicalize()
        .expect("canonical test directory");
    (root, base)
}

fn repository_recipes() -> [(&'static str, &'static [u8]); 2] {
    [
        ("images/control.Dockerfile", CONTROL_RECIPE),
        ("images/worker.Dockerfile", WORKER_RECIPE),
    ]
}

fn toml_refusal(input: &[u8]) -> Refused {
    match toml_subset::parse(input) {
        Ok(_) => panic!(
            "the TOML subset accepted {:?}",
            String::from_utf8_lossy(input)
        ),
        Err(refused) => refused,
    }
}

fn lock_refusal<T>(result: Result<T, LockRefusal>) -> LockRefusal {
    match result {
        Ok(_) => panic!("a lock outside format 1 was accepted"),
        Err(refusal) => refusal,
    }
}

/// 1-based line of the first (or last) occurrence of `marker`.
fn line_of(input: &str, marker: &str, last: bool) -> usize {
    let at = if last {
        input.rfind(marker)
    } else {
        input.find(marker)
    }
    .expect("the marker is in the input");
    input[..at].matches('\n').count() + 1
}

/// `key = value` lines of a TOML section, read by splitting each line once on
/// ` = `; the oracle of the subset reader for simple sections.
fn section_lines<'a>(input: &'a str, header: &str) -> Vec<(&'a str, &'a str)> {
    input
        .lines()
        .skip_while(|line| line.trim() != header)
        .skip(1)
        .take_while(|line| !line.starts_with('['))
        .filter_map(|line| line.split_once(" = "))
        .collect()
}

#[test]
fn toml_subset_reads_the_repository_manifests_and_refuses_constructs_outside_its_grammar() {
    for (name, bytes) in [
        ("Cargo.toml", CARGO_TOML),
        ("rust-toolchain.toml", TOOLCHAIN),
        ("Cargo.lock", CARGO_LOCK),
    ] {
        let items = toml_subset::parse(bytes)
            .unwrap_or_else(|refused| panic!("{name} refused: {refused:?}"));
        let headers = items
            .iter()
            .filter(|item| matches!(item, Item::Header { .. }))
            .count();
        let pairs = items.len() - headers;
        let expected_headers = text(bytes)
            .lines()
            .filter(|line| line.starts_with('['))
            .count();
        let expected_pairs = text(bytes)
            .lines()
            .filter(|line| {
                line.chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_alphanumeric())
                    && line.contains(" = ")
            })
            .count();
        assert_eq!(headers, expected_headers, "{name}: table headers");
        assert_eq!(pairs, expected_pairs, "{name}: key/value pairs");
    }
    // Values of the repository's own release profile and toolchain, against
    // the text of their lines.
    let facts = declared::read(CARGO_TOML, TOOLCHAIN, &repository_recipes());
    let mut profile = BTreeMap::new();
    for (key, value) in section_lines(text(CARGO_TOML), "[profile.release]") {
        let scalar = match value {
            "true" => declared::ScalarV1::Bool(true),
            "false" => declared::ScalarV1::Bool(false),
            quoted if quoted.starts_with('"') => {
                declared::ScalarV1::String(quoted.trim_matches('"').to_owned())
            }
            number => declared::ScalarV1::Integer(number.parse().expect("an integer")),
        };
        profile.insert(key.to_owned(), scalar);
    }
    assert!(
        !profile.is_empty(),
        "the repository declares a release profile"
    );
    assert_eq!(
        facts.release_profile,
        Known::declared(profile, "Cargo.toml")
    );
    // A release profile written in another legal TOML form, which Cargo
    // applies like a [profile.release] table, is unknown with its line, never
    // declared empty; the package of the same file stays declared.
    let package_lines = "[package]\nname = \"a\"\nversion = \"1.0.0\"\n";
    let other_profile_forms = [
        (
            format!(
                "{package_lines}\n[profile]\nrelease = {{ debug = true, overflow-checks = false }}\n"
            ),
            6,
        ),
        (
            format!("profile = {{ release = {{ opt-level = 0 }} }}\n\n{package_lines}"),
            1,
        ),
        (
            format!("{package_lines}\n[[profile.release]]\nlto = true\n"),
            5,
        ),
        (
            format!(
                "{package_lines}\n[profile.release]\nlto = true\n\n[profile.release.build-override]\nopt-level = 0\n"
            ),
            8,
        ),
        (format!("{package_lines}\n[[profile]]\nrelease = 1\n"), 5),
        (
            format!("{package_lines}\n[profile.release]\nlto = [\"a\"]\n"),
            6,
        ),
        (
            format!("{package_lines}\n[profile.release]\nlto = {{ a = 1 }}\n"),
            6,
        ),
    ];
    for (input, line) in &other_profile_forms {
        let facts = declared::read(input.as_bytes(), TOOLCHAIN, &[]);
        match &facts.release_profile {
            Known::Unknown { reason } => assert!(
                reason.starts_with(&format!("Cargo.toml line {line}: "))
                    && reason.contains("[profile.release]"),
                "{input:?}: {reason}"
            ),
            Known::Declared { value, .. } => {
                panic!("{input:?} gives a declared release profile {value:?}")
            }
        }
        assert_eq!(
            facts.cargo_package,
            Known::declared(("a".to_owned(), "1.0.0".to_owned()), "Cargo.toml"),
            "{input:?}"
        );
    }
    // Two reasons in one file: the first in file order is kept, whichever
    // rule gives the second.
    let two_reasons = [
        (
            format!(
                "{package_lines}\n[profile]\nrelease = {{ lto = true }}\n\n[profile.release.build-override]\nopt-level = 0\n"
            ),
            "Cargo.toml line 6: [profile] sets release as an inline table;",
        ),
        (
            format!(
                "{package_lines}\n[profile.release.build-override]\nopt-level = 0\n\n[profile.release]\nlto = [\"a\"]\n"
            ),
            "Cargo.toml line 5: the nested table [profile.release.build-override] changes",
        ),
    ];
    for (input, first) in &two_reasons {
        match &declared::read(input.as_bytes(), TOOLCHAIN, &[]).release_profile {
            Known::Unknown { reason } => assert!(reason.starts_with(first), "{input:?}: {reason}"),
            Known::Declared { value, .. } => {
                panic!("{input:?} gives a declared release profile {value:?}")
            }
        }
    }
    // A package written another way: name, version and licence are unknown
    // with the line, not "declared by no [package] table".
    let other_package_forms = [
        (
            "package = { name = \"a\", version = \"1.0.0\", license = \"MIT\" }\n",
            1,
        ),
        (
            "[[package]]\nname = \"a\"\nversion = \"1.0.0\"\nlicense = \"MIT\"\n",
            1,
        ),
    ];
    for (input, line) in other_package_forms {
        let facts = declared::read(input.as_bytes(), TOOLCHAIN, &[]);
        match (&facts.cargo_package, &facts.package_license) {
            (Known::Unknown { reason: package }, Known::Unknown { reason: license }) => {
                assert!(
                    package.starts_with(&format!("Cargo.toml line {line}: "))
                        && package.contains("[package] table only"),
                    "{input:?}: {package}"
                );
                assert_eq!(package, license, "{input:?}");
            }
            other => panic!("{input:?} gives a declared package or licence: {other:?}"),
        }
        assert_eq!(
            facts.release_profile,
            Known::declared(BTreeMap::<String, declared::ScalarV1>::new(), "Cargo.toml"),
            "{input:?}"
        );
    }
    let toolchain = section_lines(text(TOOLCHAIN), "[toolchain]");
    let channel = toolchain
        .iter()
        .find(|(key, _)| *key == "channel")
        .map(|(_, value)| value.trim_matches('"').to_owned())
        .expect("a channel line");
    let components: Vec<String> = toolchain
        .iter()
        .find(|(key, _)| *key == "components")
        .map(|(_, value)| {
            value
                .trim_matches(['[', ']'])
                .split(',')
                .map(|item| item.trim().trim_matches('"').to_owned())
                .filter(|item| !item.is_empty())
                .collect()
        })
        .unwrap_or_default();
    match &facts.toolchain {
        Known::Declared { value, from } => {
            assert_eq!(from, "rust-toolchain.toml");
            assert_eq!(value.channel, channel);
            assert_eq!(value.components, components);
        }
        Known::Unknown { reason } => panic!("the repository toolchain is unknown: {reason}"),
    }
    match toml_subset::parse(b"[dependencies]\nx = { version = \"1\", default-features = false }\n")
    {
        Ok(items) => assert_eq!(
            items[1],
            Item::Pair {
                key: "x".into(),
                value: Value::Table(vec![
                    ("version".into(), Value::String("1".into())),
                    ("default-features".into(), Value::Bool(false)),
                ]),
                line: 2,
            }
        ),
        Err(refused) => panic!("an inline table was refused: {refused:?}"),
    }

    let deep = format!("a = {}{}\n", "[".repeat(17), "]".repeat(17));
    let nested_16 = format!("a = {}{}\n", "[".repeat(16), "]".repeat(16));
    assert!(toml_subset::parse(nested_16.as_bytes()).is_ok());
    let cases: [(&str, usize, &str); 15] = [
        ("a = 1\n\"quoted\" = 2\n", 2, "quoted keys"),
        ("a.b = 1\n", 1, "dotted keys"),
        ("a = \"\"\"x\"\"\"\n", 1, "multi-line strings"),
        ("a = 1.5\n", 1, "floats"),
        ("a = 1979-05-27\n", 1, "dates"),
        ("a = 0x10\n", 1, "non-decimal"),
        ("a = 1_000\n", 1, "underscores"),
        ("a = \"\\x41\"\n", 1, "escape sequence"),
        ("a = 1\na = 2\n", 2, "key is repeated"),
        ("[t]\nk = 1\n[t]\n", 3, "header is repeated"),
        ("a = { b = 1,\n c = 2 }\n", 1, "one line"),
        (deep.as_str(), 1, "nested deeper"),
        ("\n\na = tru\n", 3, "outside the supported forms"),
        ("a = 1 b = 2\n", 1, "unexpected text"),
        ("a = \"x\ny\"\n", 1, "not terminated"),
    ];
    for (input, line, reason) in cases {
        let refused = toml_refusal(input.as_bytes());
        assert_eq!(
            refused.line, line,
            "line of the refusal of {input:?}: {refused:?}"
        );
        assert!(
            refused.reason.contains(reason),
            "reason of the refusal of {input:?}: {refused:?}"
        );
    }
    let refused = toml_refusal(b"a = 1\n\xff\n");
    assert_eq!((refused.line, refused.reason), (2, "the file is not UTF-8"));
}

#[test]
fn cargo_lock_of_the_repository_resolves_every_dependency() {
    let lock = cargo_lock::parse(CARGO_LOCK).expect("the repository Cargo.lock is read");
    let lines: Vec<&str> = text(CARGO_LOCK).lines().collect();
    let packages = lines.iter().filter(|line| **line == "[[package]]").count();
    let checksums = lines
        .iter()
        .filter(|line| line.starts_with("checksum = \""))
        .count();
    let dependency_lines = lines.iter().filter(|line| line.starts_with(" \"")).count();
    assert!(packages > 1, "the repository lock lists packages");
    assert_eq!(lock.packages.len(), packages);
    assert_eq!(lock.registry_packages(), checksums);
    assert_eq!(lock.path_packages(), 1);
    assert_eq!(packages, checksums + 1);
    let edges: usize = lock
        .packages
        .iter()
        .map(|package| package.dependencies.len())
        .sum();
    assert_eq!(
        edges, dependency_lines,
        "every dependency line resolves once"
    );
    // The root block: the one package without a source line.
    let blocks: Vec<&str> = text(CARGO_LOCK).split("[[package]]\n").skip(1).collect();
    let roots: Vec<&&str> = blocks
        .iter()
        .filter(|block| !block.contains("\nsource = "))
        .collect();
    assert_eq!(roots.len(), 1);
    let root = &lock.packages[lock.root];
    assert!(roots[0].starts_with(&format!(
        "name = \"{}\"\nversion = \"{}\"\n",
        root.name, root.version
    )));
    let root_lines = roots[0]
        .lines()
        .filter(|line| line.starts_with(" \""))
        .count();
    assert_eq!(root.dependencies.len(), root_lines);
    assert!(root.checksum.is_none());
    for package in &lock.packages {
        for dependency in &package.dependencies {
            assert!(*dependency < lock.packages.len());
        }
        let unique: BTreeSet<&usize> = package.dependencies.iter().collect();
        assert_eq!(unique.len(), package.dependencies.len());
    }
}

const REGISTRY: &str = "registry+https://github.com/rust-lang/crates.io-index";

fn registry_package(name: &str, version: &str) -> String {
    format!(
        "[[package]]\nname = \"{name}\"\nversion = \"{version}\"\nsource = \"{REGISTRY}\"\nchecksum \
         = \"{}\"\n\n",
        "a".repeat(64)
    )
}

fn root_package(dependencies: &[&str]) -> String {
    let listed: String = dependencies
        .iter()
        .map(|dependency| format!(" \"{dependency}\",\n"))
        .collect();
    format!("[[package]]\nname = \"root\"\nversion = \"0.1.0\"\ndependencies = [\n{listed}]\n\n")
}

#[test]
fn cargo_lock_refusals_name_the_line_and_the_reason() {
    let control = format!(
        "version = 4\n\n{}{}",
        root_package(&["leaf"]),
        registry_package("leaf", "1.0.0")
    );
    let lock = cargo_lock::parse(control.as_bytes()).expect("the control lock is read");
    assert_eq!(lock.packages.len(), 2);
    assert_eq!(lock.packages[lock.root].dependencies, vec![1]);

    let base = |dependencies: &[&str], rest: &str| {
        format!("version = 4\n\n{}{rest}", root_package(dependencies))
    };
    let leaf = registry_package("leaf", "1.0.0");
    let source_form = format!("leaf 1.0.0 ({REGISTRY})");
    let cases: [(String, &str, bool, &str); 15] = [
        (
            control.replacen("version = 4", "version = 3", 1),
            "version = 3",
            false,
            "must be 4",
        ),
        (
            format!("{control}[metadata]\nx = \"y\"\n"),
            "[metadata]",
            false,
            "accepts [[package]] tables only",
        ),
        (
            format!("{control}replace = \"x\"\n"),
            "replace = ",
            false,
            "package key replace",
        ),
        (
            base(
                &["leaf"],
                &leaf.replace(REGISTRY, "git+https://example.invalid/leaf#0"),
            ),
            "[[package]]",
            true,
            "source other than the crates.io registry",
        ),
        (
            base(&["leaf"], &leaf.replace(&"a".repeat(64), "not-hex")),
            "[[package]]",
            true,
            "no checksum of 64",
        ),
        (
            base(
                &["leaf"],
                &leaf.replace(&format!("checksum = \"{}\"\n", "a".repeat(64)), ""),
            ),
            "[[package]]",
            true,
            "no checksum of 64",
        ),
        (
            format!(
                "version = 4\n\n{}",
                root_package(&[]).replace(
                    "version = \"0.1.0\"\n",
                    &format!("version = \"0.1.0\"\nchecksum = \"{}\"\n", "b".repeat(64))
                )
            ),
            "[[package]]",
            false,
            "carries a checksum",
        ),
        (
            format!("{control}[[package]]\nname = \"second\"\nversion = \"1.0.0\"\n"),
            "[[package]]",
            true,
            "more than one path package",
        ),
        (
            format!("{control}{leaf}"),
            "[[package]]",
            true,
            "listed twice",
        ),
        (
            base(&[source_form.as_str()], &leaf),
            "dependencies = [",
            false,
            "the (source) form",
        ),
        (
            base(&["absent"], &leaf),
            "dependencies = [",
            false,
            "names no package",
        ),
        (
            base(
                &["leaf"],
                &format!("{leaf}{}", registry_package("leaf", "2.0.0")),
            ),
            "dependencies = [",
            false,
            "several packages without a version",
        ),
        (
            base(
                &["leaf"],
                &leaf.replace("name = \"leaf\"", "name = \"le af\""),
            ),
            "[[package]]",
            true,
            "outside [A-Za-z0-9_-]",
        ),
        (
            base(&["leaf"], &leaf.replace("name = \"leaf\"\n", "")),
            "[[package]]",
            true,
            "has no name",
        ),
        (
            base(&["leaf", "leaf"], &leaf),
            "dependencies = [",
            false,
            "listed twice",
        ),
    ];
    for (input, marker, last, reason) in &cases {
        let refusal = lock_refusal(cargo_lock::parse(input.as_bytes()));
        assert_eq!(refusal.file, "Cargo.lock");
        assert_eq!(
            refusal.line,
            Some(line_of(input, marker, *last)),
            "line of the refusal {refusal} of:\n{input}"
        );
        assert!(
            refusal.reason.contains(reason),
            "reason of the refusal {refusal} of:\n{input}"
        );
        assert!(refusal.to_string().starts_with("Cargo.lock:"));
    }
    let without_version = control.replacen("version = 4\n", "", 1);
    let refusal = lock_refusal(cargo_lock::parse(without_version.as_bytes()));
    assert_eq!(refusal.line, None);
    assert!(refusal.reason.contains("no version = 4"), "{refusal}");
}

fn sri_sha512(bytes: &[u8]) -> String {
    format!("sha512-{}", STANDARD.encode(Sha512::digest(bytes)))
}

#[test]
fn npm_lock_of_the_repository_maps_each_integrity_to_sha512_hex() {
    let lock = npm_lock::parse(NPM_LOCK, NPM_MANIFEST).expect("the repository lock is read");
    let value: serde_json::Value = serde_json::from_slice(NPM_LOCK).expect("JSON");
    let entries = value["packages"].as_object().expect("packages");
    let installed: BTreeMap<&str, &serde_json::Value> = entries
        .iter()
        .filter_map(|(key, entry)| key.strip_prefix("node_modules/").map(|name| (name, entry)))
        .collect();
    assert_eq!(
        installed.len() + 1,
        entries.len(),
        "only the root is not installed"
    );
    assert_eq!(lock.packages.len(), installed.len());
    let flag = |entry: &serde_json::Value, key: &str| entry[key].as_bool() == Some(true);
    for package in &lock.packages {
        let entry = installed[package.name.as_str()];
        assert_eq!(package.version, entry["version"].as_str().expect("version"));
        assert_eq!(
            package.resolved,
            entry["resolved"].as_str().expect("resolved")
        );
        let integrity = entry["integrity"].as_str().expect("integrity");
        let decoded = STANDARD
            .decode(integrity.strip_prefix("sha512-").expect("sha512"))
            .expect("base64");
        assert_eq!(package.sha512_hex, hex::encode(decoded));
        assert_eq!(package.sha512_hex.len(), 128);
        assert_eq!(package.dev, flag(entry, "dev"));
        assert_eq!(package.optional, flag(entry, "optional"));
        assert_eq!(package.install_script, flag(entry, "hasInstallScript"));
        assert_eq!(package.license.as_deref(), entry["license"].as_str());
        let mut expected: BTreeSet<&str> = BTreeSet::new();
        for key in ["dependencies", "optionalDependencies", "peerDependencies"] {
            if let Some(names) = entry[key].as_object() {
                expected.extend(
                    names
                        .keys()
                        .map(String::as_str)
                        .filter(|name| installed.contains_key(name)),
                );
            }
        }
        let resolved: BTreeSet<&str> = package
            .dependencies
            .iter()
            .map(|index| lock.packages[*index].name.as_str())
            .collect();
        assert_eq!(resolved, expected, "dependencies of {}", package.name);
    }
    let root_names: BTreeSet<&str> = ["dependencies", "devDependencies"]
        .iter()
        .filter_map(|key| entries[""][*key].as_object())
        .flat_map(|names| names.keys().map(String::as_str))
        .collect();
    let resolved_root: BTreeSet<&str> = lock
        .root
        .dependencies
        .iter()
        .map(|index| lock.packages[*index].name.as_str())
        .collect();
    assert_eq!(resolved_root, root_names);

    // Known answer: the integrity of synthetic bytes, computed here.
    let tarball = b"synthetic tarball bytes, not a package";
    let lock_bytes = serde_json::to_vec(&json!({
        "name": "front", "version": "1.0.0", "lockfileVersion": 3, "requires": true,
        "packages": {
            "": { "name": "front", "version": "1.0.0", "devDependencies": { "leaf": "1.0.0" } },
            "node_modules/leaf": {
                "version": "1.0.0",
                "resolved": "https://registry.npmjs.org/leaf/-/leaf-1.0.0.tgz",
                "integrity": sri_sha512(tarball), "dev": true, "license": "MIT"
            }
        }
    }))
    .expect("JSON");
    let manifest =
        br#"{"name": "front", "version": "1.0.0", "devDependencies": {"leaf": "1.0.0"}}"#;
    let synthetic = npm_lock::parse(&lock_bytes, manifest).expect("the synthetic lock is read");
    assert_eq!(
        synthetic.packages[0].sha512_hex,
        hex::encode(Sha512::digest(tarball))
    );
}

fn npm_case(top: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec_pretty(&top).expect("JSON")
}

fn npm_top(packages: serde_json::Value) -> serde_json::Value {
    json!({ "name": "front", "version": "1.0.0", "lockfileVersion": 3, "requires": true,
            "packages": packages })
}

fn npm_root() -> serde_json::Value {
    json!({ "name": "front", "version": "1.0.0", "devDependencies": { "leaf": "1.0.0" } })
}

fn npm_leaf() -> serde_json::Value {
    json!({
        "version": "1.0.0",
        "resolved": "https://registry.npmjs.org/leaf/-/leaf-1.0.0.tgz",
        "integrity": sri_sha512(b"leaf"),
        "dev": true,
        "license": "MIT"
    })
}

const NPM_CASE_MANIFEST: &[u8] =
    br#"{"name": "front", "version": "1.0.0", "devDependencies": {"leaf": "1.0.0"}}"#;

#[test]
fn npm_lock_refusals_cover_unknown_keys_nested_layouts_and_unsafe_urls() {
    let packages = |leaf: serde_json::Value| json!({ "": npm_root(), "node_modules/leaf": leaf });
    let with = |key: &str, value: serde_json::Value| {
        let mut leaf = npm_leaf();
        leaf[key] = value;
        leaf
    };
    let without = |key: &str| {
        let mut leaf = npm_leaf();
        leaf.as_object_mut().expect("object").remove(key);
        leaf
    };
    let control = npm_case(npm_top(packages(npm_leaf())));
    npm_lock::parse(&control, NPM_CASE_MANIFEST).expect("the control lock is read");
    let mut optional_peer = with("peerDependencies", json!({ "ghost": "^1" }));
    optional_peer["peerDependenciesMeta"] = json!({ "ghost": { "optional": true } });
    npm_lock::parse(
        &npm_case(npm_top(packages(optional_peer))),
        NPM_CASE_MANIFEST,
    )
    .expect("a missing optional peer is admitted");

    let mut unknown_top = npm_top(packages(npm_leaf()));
    unknown_top["workspaces"] = json!([]);
    let mut version_2 = npm_top(packages(npm_leaf()));
    version_2["lockfileVersion"] = json!(2);
    let nested = json!({ "": npm_root(), "node_modules/leaf": npm_leaf(),
                         "node_modules/leaf/node_modules/inner": npm_leaf() });
    let linked =
        json!({ "": npm_root(), "node_modules/leaf": npm_leaf(), "packages/inner": npm_leaf() });
    let aliased = with("name", json!("other"));
    let two_hashes = with(
        "integrity",
        json!(format!("{} {}", sri_sha512(b"a"), sri_sha512(b"b"))),
    );
    let mut missing_peer = with("peerDependencies", json!({ "ghost": "^1" }));
    missing_peer["peerDependenciesMeta"] = json!({});
    let mut root_optional = npm_root();
    root_optional["optionalDependencies"] = json!({ "leaf": "1.0.0" });
    let mut root_peer = npm_root();
    root_peer["peerDependencies"] = json!({ "leaf": "^1" });
    let mut root_peer_meta = npm_root();
    root_peer_meta["peerDependenciesMeta"] = json!({ "leaf": { "optional": true } });
    let cases: [(Vec<u8>, &[u8], &str); 23] = [
        (
            npm_case(npm_top(
                json!({ "": root_optional, "node_modules/leaf": npm_leaf() }),
            )),
            NPM_CASE_MANIFEST,
            "the root entry declares optionalDependencies or peerDependencies",
        ),
        (
            npm_case(npm_top(
                json!({ "": root_peer, "node_modules/leaf": npm_leaf() }),
            )),
            NPM_CASE_MANIFEST,
            "the root entry declares optionalDependencies or peerDependencies",
        ),
        (
            npm_case(npm_top(
                json!({ "": root_peer_meta, "node_modules/leaf": npm_leaf() }),
            )),
            NPM_CASE_MANIFEST,
            "the root entry declares optionalDependencies or peerDependencies",
        ),
        (
            npm_case(npm_top(packages(with("devOptional", json!(true))))),
            NPM_CASE_MANIFEST,
            "is marked devOptional",
        ),
        (
            // A wrongly typed field: serde_json would quote the value found.
            npm_case(npm_top(packages(with(
                "dev",
                json!("https://user:CANARY-SECRET@registry.npmjs.org/leaf/-/leaf-1.0.0.tgz"),
            )))),
            NPM_CASE_MANIFEST,
            "a field has a type or value outside format 1",
        ),
        (
            npm_case(npm_top(packages(with("link", json!(true))))),
            NPM_CASE_MANIFEST,
            "unknown field",
        ),
        (npm_case(unknown_top), NPM_CASE_MANIFEST, "unknown field"),
        (npm_case(version_2), NPM_CASE_MANIFEST, "lockfileVersion 2"),
        (
            npm_case(npm_top(packages(without("integrity")))),
            NPM_CASE_MANIFEST,
            "has no integrity",
        ),
        (
            npm_case(npm_top(packages(without("resolved")))),
            NPM_CASE_MANIFEST,
            "has no resolved URL",
        ),
        (npm_case(npm_top(nested)), NPM_CASE_MANIFEST, "nested"),
        (
            npm_case(npm_top(linked)),
            NPM_CASE_MANIFEST,
            "node_modules/<name>",
        ),
        (
            npm_case(npm_top(packages(with(
                "resolved",
                json!("https://user:CANARY-SECRET@registry.npmjs.org/leaf/-/leaf-1.0.0.tgz"),
            )))),
            NPM_CASE_MANIFEST,
            "without user information",
        ),
        (
            npm_case(npm_top(packages(with(
                "resolved",
                json!("https://registry.npmjs.org/leaf/-/leaf-1.0.0.tgz?token=CANARY-SECRET"),
            )))),
            NPM_CASE_MANIFEST,
            "without user information",
        ),
        (
            npm_case(npm_top(packages(with(
                "resolved",
                json!("http://registry.npmjs.org/leaf/-/leaf-1.0.0.tgz"),
            )))),
            NPM_CASE_MANIFEST,
            "is not an https URL",
        ),
        (
            npm_case(npm_top(packages(with(
                "integrity",
                json!("sha1-AAAAAAAAAAAAAAAAAAAAAAAAAAA="),
            )))),
            NPM_CASE_MANIFEST,
            "sha512",
        ),
        (
            npm_case(npm_top(packages(two_hashes))),
            NPM_CASE_MANIFEST,
            "exactly one sha512",
        ),
        (
            npm_case(npm_top(packages(with(
                "dependencies",
                json!({ "ghost": "^1" }),
            )))),
            NPM_CASE_MANIFEST,
            "no node_modules/ghost entry",
        ),
        (
            npm_case(npm_top(packages(missing_peer))),
            NPM_CASE_MANIFEST,
            "requires the peer ghost",
        ),
        (
            npm_case(npm_top(packages(aliased))),
            NPM_CASE_MANIFEST,
            "aliases",
        ),
        (
            npm_case(npm_top(json!({ "node_modules/leaf": npm_leaf() }))),
            NPM_CASE_MANIFEST,
            "no root package entry",
        ),
        (
            control.clone(),
            br#"{"name": "front", "version": "1.0.0", "devDependencies": {"other": "1.0.0"}}"#
                .as_slice(),
            "does not match front/package.json",
        ),
        (
            control.clone(),
            br#"{"name": "front", "version": "2.0.0", "devDependencies": {"leaf": "1.0.0"}}"#
                .as_slice(),
            "does not match front/package.json",
        ),
    ];
    for (lock, manifest, reason) in &cases {
        let refusal = lock_refusal(npm_lock::parse(lock, manifest));
        let message = refusal.to_string();
        assert!(
            message.contains(reason),
            "{message} does not name {reason:?} for:\n{}",
            String::from_utf8_lossy(lock)
        );
        assert!(
            !message.contains("CANARY-SECRET"),
            "a refusal repeats a credential-bearing URL: {message}"
        );
        assert_eq!(refusal.file, npm_lock::NPM_LOCK);
    }
    // front/package.json: the two dependency kinds that format 1 does not
    // compare with the lock root, and a wrongly typed field whose value is
    // never repeated.
    let manifest_cases: [(&[u8], &str); 3] = [
        (
            br#"{"name": "front", "version": "1.0.0", "devDependencies": {"leaf": "1.0.0"}, "optionalDependencies": {"other": "1.0.0"}}"#,
            "declares optionalDependencies or peerDependencies",
        ),
        (
            br#"{"name": "front", "version": "1.0.0", "devDependencies": {"leaf": "1.0.0"}, "peerDependencies": {"other": "^1"}}"#,
            "declares optionalDependencies or peerDependencies",
        ),
        (
            br#"{"name": "front", "version": "1.0.0", "devDependencies": "https://user:CANARY-SECRET@registry.npmjs.org/leaf"}"#,
            "a field has a type or value outside format 1",
        ),
    ];
    for (manifest, reason) in manifest_cases {
        let refusal = lock_refusal(npm_lock::parse(&control, manifest));
        let message = refusal.to_string();
        assert!(
            message.contains(reason),
            "{message} does not name {reason:?}"
        );
        assert!(
            !message.contains("CANARY-SECRET"),
            "a refusal repeats a value of front/package.json: {message}"
        );
        assert_eq!(refusal.file, npm_lock::NPM_MANIFEST);
    }
    // A repeated key: serde_json maps would keep the last one silently.
    let leaf = serde_json::to_string(&npm_leaf()).expect("JSON");
    let repeated = format!(
        "{{\"name\": \"front\", \"version\": \"1.0.0\", \"lockfileVersion\": 3, \"packages\": {{\"\": \
         {}, \"node_modules/leaf\": {leaf}, \"node_modules/leaf\": {leaf}}}}}",
        npm_root()
    );
    let refusal = lock_refusal(npm_lock::parse(repeated.as_bytes(), NPM_CASE_MANIFEST));
    assert!(refusal.to_string().contains("repeated"), "{refusal}");
}

/// Every external `FROM` reference of a recipe text, and its internal
/// stages, by splitting the `FROM` lines: the oracle of the recipe reader.
fn scanned_images(recipe: &str) -> (BTreeSet<String>, usize) {
    let mut aliases: BTreeSet<String> = BTreeSet::new();
    let mut external = BTreeSet::new();
    let mut internal = 0;
    for line in recipe.lines().filter(|line| line.starts_with("FROM ")) {
        let words: Vec<&str> = line.split_whitespace().collect();
        if aliases.contains(&words[1].to_ascii_lowercase()) {
            internal += 1;
        } else {
            external.insert(words[1].to_owned());
        }
        if words.len() == 4 && words[2].eq_ignore_ascii_case("AS") {
            aliases.insert(words[3].to_ascii_lowercase());
        }
    }
    (external, internal)
}

fn invocations(recipe: &declared::RecipeV1) -> Vec<Known<declared::InvocationV1>> {
    match &recipe.cargo_build_invocations {
        Known::Declared { value, .. } => value.clone(),
        Known::Unknown { reason } => panic!("{} has unknown invocations: {reason}", recipe.path),
    }
}

fn whole_recipe_unknown(recipe: &declared::RecipeV1) -> bool {
    matches!(recipe.base_images, Known::Unknown { .. })
        && matches!(recipe.cargo_build_invocations, Known::Unknown { .. })
}

#[test]
fn recipes_reader_extracts_base_images_and_marks_shell_lines_unknown() {
    let facts = declared::read(CARGO_TOML, TOOLCHAIN, &repository_recipes());
    let (references, unknown) = document::base_images(&facts.recipes);
    let mut expected = BTreeSet::new();
    let mut internal = 0;
    for (_, bytes) in repository_recipes() {
        let (external, stages) = scanned_images(text(bytes));
        expected.extend(external);
        internal += stages;
    }
    assert_eq!(
        references.iter().cloned().collect::<BTreeSet<_>>(),
        expected
    );
    assert_eq!(
        references.len(),
        3,
        "three external references at this base"
    );
    assert_eq!(internal, 1, "one internal stage at this base");
    assert!(unknown.is_empty());
    let declared_stages: usize = facts
        .recipes
        .iter()
        .filter_map(|recipe| recipe.base_images.value())
        .flatten()
        .filter(|image| image.value().is_some_and(|image| image.internal_stage))
        .count();
    assert_eq!(declared_stages, internal);
    assert!(facts.recipes.iter().all(|recipe| {
        recipe.base_images.value().is_some_and(|images| {
            images
                .iter()
                .all(|image| image.value().is_some_and(|image| !image.pinned_by_digest))
        })
    }));
    let mut found = Vec::new();
    for recipe in &facts.recipes {
        let bytes = repository_recipes()
            .into_iter()
            .find(|(path, _)| *path == recipe.path)
            .map(|(_, bytes)| bytes)
            .expect("a repository recipe");
        assert_eq!(recipe.sha256, sha256_hex(bytes));
        let lines: Vec<Vec<String>> = text(bytes)
            .lines()
            .filter_map(|line| line.strip_prefix("RUN cargo build "))
            .map(|rest| {
                ["cargo", "build"]
                    .into_iter()
                    .map(str::to_owned)
                    .chain(rest.split_whitespace().map(str::to_owned))
                    .collect()
            })
            .collect();
        let declared_invocations = invocations(recipe);
        assert_eq!(declared_invocations.len(), lines.len(), "{}", recipe.path);
        for (invocation, argv) in declared_invocations.iter().zip(lines) {
            let Known::Declared { value, .. } = invocation else {
                panic!("{}: an admitted invocation is unknown", recipe.path);
            };
            assert_eq!(value.argv, argv);
            assert_eq!(value.profile, "release");
            assert!(matches!(value.target, Known::Unknown { .. }));
            found.push(value.clone());
        }
    }
    assert_eq!(found.len(), 2, "two declared invocations at this base");

    let synthetic = |input: &str| declared::recipe("images/synthetic.Dockerfile", input.as_bytes());
    let only_invocation = |input: &str| {
        let recipe = synthetic(input);
        let mut all = invocations(&recipe);
        assert_eq!(all.len(), 1, "one invocation in {input:?}");
        all.remove(0)
    };
    let argument = synthetic("FROM ${BASE}\n");
    match &argument.base_images {
        Known::Declared { value, .. } => {
            assert_eq!(value.len(), 1);
            assert!(matches!(value[0], Known::Unknown { .. }));
        }
        Known::Unknown { reason } => panic!("one FROM with a build argument: {reason}"),
    }
    assert!(matches!(
        only_invocation("FROM rust:1 AS build\nRUN cargo build --release && echo done\n"),
        Known::Unknown { .. }
    ));
    assert!(matches!(
        only_invocation("FROM rust:1\nRUN echo start \\\n    && cargo build --release\n"),
        Known::Unknown { .. }
    ));
    match only_invocation("FROM rust:1\nRUN cargo build \\\n    --release --locked\n") {
        Known::Declared { value, .. } => {
            assert_eq!(value.argv, ["cargo", "build", "--release", "--locked"]);
            assert_eq!(value.profile, "release");
        }
        Known::Unknown { reason } => panic!("a continued admitted invocation: {reason}"),
    }
    match only_invocation("FROM rust:1\nRUN cargo build --profile=ci\n") {
        Known::Declared { value, .. } => assert_eq!(value.profile, "ci"),
        Known::Unknown { reason } => panic!("--profile=ci: {reason}"),
    }
    match only_invocation("FROM rust:1\nRUN cargo build\n") {
        Known::Declared { value, .. } => assert_eq!(value.profile, "dev"),
        Known::Unknown { reason } => panic!("no profile option: {reason}"),
    }
    match only_invocation("FROM rust:1\nRUN cargo build -r --target aarch64-unknown-linux-gnu\n") {
        Known::Declared { value, .. } => {
            assert_eq!(value.profile, "release");
            assert_eq!(
                value.target,
                Known::declared(
                    "aarch64-unknown-linux-gnu".to_owned(),
                    "images/synthetic.Dockerfile"
                )
            );
        }
        Known::Unknown { reason } => panic!("--target: {reason}"),
    }
    for refused in [
        "FROM rust:1\nRUN cargo build --release --profile ci\n",
        "FROM rust:1\nRUN cargo build -rq\n",
        "FROM rust:1\nRUN cargo build --target a --target b\n",
        "FROM rust:1\nRUN [\"cargo\", \"build\"]\n",
        "FROM rust:1\nRUN cd /build; cargo build --release\n",
    ] {
        assert!(
            matches!(only_invocation(refused), Known::Unknown { .. }),
            "{refused:?}"
        );
    }
    let none = synthetic(
        "FROM rust:1\nENV PATH=/usr/local/cargo/bin:/bin\nCOPY --from=x /usr/local/cargo/ /usr/local/cargo/\n",
    );
    assert!(
        invocations(&none).is_empty(),
        "ENV and COPY are not invocations"
    );
    for whole in [
        "# syntax=docker/dockerfile:1\nFROM rust:1\nRUN cargo build --release\n",
        "# escape=`\nFROM rust:1\n",
        "FROM rust:1\nRUN cat <<EOF\nx\nEOF\n",
        "FROM rust:1\nRUN echo a \\\n# a comment\n    && echo b\n",
        "FROM rust:1\nRUN echo a \\\n\n    && echo b\n",
    ] {
        assert!(whole_recipe_unknown(&synthetic(whole)), "{whole:?}");
    }
    let no_from = synthetic("RUN echo hello\n");
    assert!(
        matches!(no_from.base_images, Known::Unknown { .. }),
        "no FROM gives an unknown, not an empty list"
    );
    let digest = format!(
        "FROM example.invalid/base@sha256:{} AS pinned\n",
        "c".repeat(64)
    );
    let pinned = synthetic(&digest);
    assert!(pinned.base_images.value().is_some_and(|images| {
        images
            .iter()
            .all(|image| image.value().is_some_and(|image| image.pinned_by_digest))
    }));
}

#[test]
fn provider_version_rule_is_the_intersection_with_the_worker_recipe() {
    let longest_accepted = format!("1.2.3+{}", "a".repeat(94));
    let too_long = format!("1.2.3+{}", "a".repeat(95));
    assert_eq!(longest_accepted.len(), 100);
    assert_eq!(too_long.len(), 101);
    for accepted in [
        "1.2.3",
        "1.2.3-rc.1",
        "1.2.3+b.5",
        longest_accepted.as_str(),
    ] {
        declared::validate_declared_provider_version(accepted)
            .unwrap_or_else(|error| panic!("{accepted} refused: {error:#}"));
    }
    for refused in [
        "1.2.3-rc.1+b.5",
        "1.2.3--x",
        "latest",
        "1.2",
        too_long.as_str(),
    ] {
        assert!(
            declared::validate_declared_provider_version(refused).is_err(),
            "{refused} accepted"
        );
    }
    // Each rule refuses a value the other accepts: the rule is an intersection.
    let recipe = regex::Regex::new(declared::RECIPE_VERSION_PATTERN).expect("pattern");
    assert!(crate::operations::validate_provider_version("1.2.3-rc.1+b.5").is_ok());
    assert!(!recipe.is_match("1.2.3-rc.1+b.5"));
    assert!(crate::operations::validate_provider_version("1.2.3--x").is_err());
    assert!(recipe.is_match("1.2.3--x"));
    // Drift detector: the real worker recipe applies exactly this pattern to
    // both provider versions.
    let quoted = format!("'{}'", declared::RECIPE_VERSION_PATTERN);
    assert_eq!(text(WORKER_RECIPE).matches(quoted.as_str()).count(), 2);
    let facts = declared::read(CARGO_TOML, TOOLCHAIN, &repository_recipes());
    assert_eq!(
        facts.codex_package,
        Known::declared(
            declared::CODEX_PACKAGE.to_owned(),
            "images/worker.Dockerfile"
        )
    );
    assert_eq!(
        facts.claude_code_package,
        Known::declared(
            declared::CLAUDE_CODE_PACKAGE.to_owned(),
            "images/worker.Dockerfile"
        )
    );
}

/// The image labels that CONTRACT section 4 and the release guides publish:
/// each written once, with its documented value, in a `LABEL` instruction of
/// the final stage of its recipe. A drift detector of the recipe text only:
/// labels are observed on a built image, which no test here builds.
#[test]
fn published_image_labels_are_declared_once_in_the_final_stage() {
    let published: [(&str, &[u8], &[&str]); 2] = [
        (
            "images/control.Dockerfile",
            CONTROL_RECIPE,
            &["ai-guard.image.recipe=\"images/control.Dockerfile\""],
        ),
        (
            "images/worker.Dockerfile",
            WORKER_RECIPE,
            &[
                "ai-guard.image.recipe=\"images/worker.Dockerfile\"",
                "ai-guard.provider.codex.requested-version=\"${CODEX_VERSION}\"",
                "ai-guard.provider.claude-code.requested-version=\"${CLAUDE_CODE_VERSION}\"",
            ],
        ),
    ];
    for (path, bytes, labels) in published {
        let lines: Vec<&str> = text(bytes).lines().collect();
        let last_from = lines
            .iter()
            .rposition(|line| line.starts_with("FROM "))
            .unwrap_or_else(|| panic!("{path} has no FROM line"));
        for label in labels {
            let holding: Vec<usize> = lines
                .iter()
                .enumerate()
                .filter(|(_, line)| line.contains(label))
                .map(|(index, _)| index)
                .collect();
            assert_eq!(holding.len(), 1, "{path}: {label} is not written once");
            let mut start = holding[0];
            while start > 0 && lines[start - 1].ends_with('\\') {
                start -= 1;
            }
            assert!(
                lines[start].starts_with("LABEL "),
                "{path}: {label} is not in a LABEL instruction"
            );
            assert!(
                start > last_from,
                "{path}: {label} is not in the final stage"
            );
        }
        // No other ai-guard.* key: ai-guard.build.inputs-identity stays
        // reserved for the proposed wire PW-3.
        assert_eq!(
            text(bytes).matches("ai-guard.").count(),
            labels.len(),
            "{path}: an ai-guard.* key outside the published list"
        );
    }
}

#[test]
fn licence_expression_form_only_for_well_formed_expressions() {
    for expression in [
        "MIT",
        "Apache-2.0",
        "(MIT OR Apache-2.0)",
        "GPL-2.0-or-later WITH Classpath-exception-2.0",
        "Apache-2.0 AND (MIT OR BSD-3-Clause)",
        "GPL-2.0+",
    ] {
        let form = serde_json::to_value(cyclonedx::licence(expression)).expect("JSON");
        assert_eq!(
            form,
            json!({ "expression": expression, "acknowledgement": "declared" }),
            "{expression}"
        );
    }
    for name in [
        "SEE LICENSE IN LICENSE.md",
        "MIT OR",
        "(MIT",
        "MIT)",
        "",
        "MIT or Apache-2.0",
        "WITH",
    ] {
        let form = serde_json::to_value(cyclonedx::licence(name)).expect("JSON");
        assert_eq!(
            form,
            json!({ "license": { "name": name, "acknowledgement": "declared" } }),
            "{name:?}"
        );
    }
    let lock: serde_json::Value = serde_json::from_slice(NPM_LOCK).expect("JSON");
    let licences: Vec<&str> = lock["packages"]
        .as_object()
        .expect("packages")
        .iter()
        .filter(|(key, _)| !key.is_empty())
        .filter_map(|(_, entry)| entry["license"].as_str())
        .collect();
    assert!(!licences.is_empty());
    for licence in &licences {
        assert!(cyclonedx::well_formed_licence(licence), "{licence}");
    }
    let parsed = npm_lock::parse(NPM_LOCK, NPM_MANIFEST).expect("the repository lock is read");
    assert_eq!(
        parsed
            .packages
            .iter()
            .filter(|package| package.license.is_some())
            .count(),
        licences.len()
    );
}

#[test]
fn an_artifact_linked_during_hashing_is_refused() {
    let (_root, base) = private_directory();
    let bound = base.join("agentsandbox");
    let content = b"synthetic artefact bytes\n".repeat(1000);
    fs::write(&bound, &content).expect("artefact");
    // Explicit: the reader refuses group write, whatever the test's umask.
    fs::set_permissions(&bound, fs::Permissions::from_mode(0o644)).expect("artefact mode");
    // A second name before the read: the artefact is bound.
    fs::hard_link(&bound, base.join("agentsandbox-copy")).expect("hard link");
    let digest = artifact::digest(&bound, None).expect("a hard-linked artefact is bound");
    assert_eq!(
        digest,
        ArtifactDigest {
            name: "agentsandbox".to_owned(),
            bytes: content.len() as u64,
            sha256: sha256_hex(&fs::read(&bound).expect("artefact")),
        }
    );

    // A second name added after the last byte was read, before the checks.
    let hashed = base.join("ai-guard-diagnose");
    fs::write(&hashed, &content).expect("artefact");
    fs::set_permissions(&hashed, fs::Permissions::from_mode(0o644)).expect("artefact mode");
    let linked = base.join("linked-during-hashing");
    let target = linked.clone();
    AFTER_READ.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |path: &Path| {
            fs::hard_link(path, target).expect("hard link during hashing");
        }))
    });
    let refused = match artifact::digest(&hashed, None) {
        Ok(digest) => panic!("an artefact linked while hashed was bound: {digest:?}"),
        Err(error) => format!("{error:#}"),
    };
    assert!(
        linked.exists(),
        "the hook did not run, so the refusal is not about the link"
    );
    assert!(refused.contains("changed while it was hashed"), "{refused}");
    // The same file, linked before the read, is bound.
    let again = artifact::digest(&hashed, None).expect("bound once the link exists");
    assert_eq!(again.sha256, sha256_hex(&content));

    let names = artifact::check_paths(&[bound.clone(), base.join("other").join("agentsandbox")]);
    assert!(names.is_err(), "two artefacts with one name are refused");
    for name in [
        ".hidden",
        "credentials.env",
        "store.sqlite",
        "key.pem",
        "a b",
    ] {
        assert!(
            artifact::check_paths(&[base.join(name)]).is_err(),
            "{name} accepted"
        );
    }
}

/// A new FIFO at `path`.
fn make_fifo(path: &Path) {
    let name = CString::new(path.as_os_str().as_bytes()).expect("FIFO path");
    // SAFETY: a NUL-terminated path and a mode; no pointer outlives the call.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0, "FIFO");
}

/// What `open_regular` reports for `name` below `directory`, and every name
/// this thread passed to `openat` meanwhile.
fn inspect(directory: &fs::File, name: &str) -> (Result<&'static str, String>, Vec<OsString>) {
    OPENED.with(|opened| *opened.borrow_mut() = Some(Vec::new()));
    let kind = match artifact::open_regular(directory, OsStr::new(name)) {
        Ok(artifact::Entry::Regular(_)) => Ok("regular"),
        Ok(artifact::Entry::SymbolicLink) => Ok("symbolic link"),
        Ok(artifact::Entry::Other) => Ok("not opened"),
        Ok(artifact::Entry::Replaced) => Ok("replaced"),
        Err(error) => Err(format!("{name} could not be inspected: {error}")),
    };
    let opened = OPENED
        .with(|opened| opened.borrow_mut().take())
        .unwrap_or_default();
    (kind, opened)
}

#[test]
fn an_artifact_name_is_opened_only_once_it_is_seen_as_a_regular_file() {
    let (_root, base) = private_directory();
    let regular = base.join("agentsandbox");
    fs::write(&regular, b"synthetic artefact bytes\n").expect("artefact");
    fs::set_permissions(&regular, fs::Permissions::from_mode(0o644)).expect("artefact mode");
    std::os::unix::fs::symlink(&regular, base.join("symbolic")).expect("symbolic link");
    fs::create_dir(base.join("directory")).expect("directory");
    make_fifo(&base.join("fifo"));
    // The kernel refuses open(2) on a socket (ENXIO on Linux, EOPNOTSUPP on
    // macOS): an open before the type check fails on it, whatever opens it.
    let _socket = std::os::unix::net::UnixListener::bind(base.join("socket")).expect("socket");
    // An open made before the type check, by any path, can wait forever on
    // the FIFO (for a writer, or in a read). So the socket is looked at before
    // the FIFO, and the checks run on another thread whose wait this thread
    // bounds: such a defect fails the test instead of hanging it. The
    // recorder is a thread-local of the checking thread.
    let (sender, receiver) = mpsc::channel();
    let checks = std::thread::spawn(move || {
        let directory = fs::File::open(&base).expect("artefact directory");
        // The recorder sees the one open of a regular file, so an empty record
        // below means that no openat was made for that name.
        assert_eq!(
            inspect(&directory, "agentsandbox"),
            (Ok("regular"), vec![OsString::from("agentsandbox")])
        );
        for (name, kind) in [
            ("symbolic", "symbolic link"),
            ("directory", "not opened"),
            ("socket", "not opened"),
            ("fifo", "not opened"),
        ] {
            assert_eq!(
                inspect(&directory, name),
                (Ok(kind), Vec::<OsString>::new()),
                "{name}"
            );
        }
        // A character device is refused without being opened.
        let devices = fs::File::open("/dev").expect("/dev");
        assert_eq!(
            inspect(&devices, "null"),
            (Ok("not opened"), Vec::<OsString>::new())
        );
        assert!(artifact::open_regular(&directory, OsStr::new("absent")).is_err());
        // Through the reader, the socket and the FIFO get the regular-file
        // refusal and nothing waits for a writer.
        for name in ["socket", "fifo"] {
            let refused = match artifact::digest(&base.join(name), None) {
                Ok(digest) => panic!("a {name} was bound: {digest:?}"),
                Err(error) => format!("{error:#}"),
            };
            assert!(refused.contains("must be a regular file"), "{refused}");
        }
        let symbolic = match artifact::digest(&base.join("symbolic"), None) {
            Ok(digest) => panic!("a symbolic link was bound: {digest:?}"),
            Err(error) => format!("{error:#}"),
        };
        assert!(
            symbolic.contains("is unavailable or is a symbolic link"),
            "{symbolic}"
        );
        sender
            .send(())
            .expect("the test receives the end of the checks");
    });
    // A bound against a hang, not a latency measurement.
    match receiver.recv_timeout(Duration::from_secs(30)) {
        Ok(()) => checks.join().expect("the checking thread ended"),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("an inspection waited on an entry that must not be opened")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => match checks.join() {
            Err(payload) => std::panic::resume_unwind(payload),
            Ok(()) => panic!("the checks ended without reporting"),
        },
    }
}

#[test]
fn an_artifact_name_replaced_after_its_type_was_read_is_refused() {
    let (_root, base) = private_directory();
    let bound = base.join("agentsandbox");
    let replacement = base.join("replacement");
    let write = |path: &Path| {
        fs::write(path, b"synthetic artefact bytes\n").expect("artefact");
        fs::set_permissions(path, fs::Permissions::from_mode(0o644)).expect("artefact mode");
    };
    // The hook renames `from` over the bound name between `fstatat` and
    // `openat`, on the thread that sets it.
    let swap_in = |from: PathBuf| {
        let to = bound.clone();
        BEFORE_OPEN.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                fs::rename(from, to).expect("swap between fstatat and openat");
            }))
        });
    };
    write(&bound);
    let directory = fs::File::open(&base).expect("artefact directory");

    // Another regular file: the descriptor names another inode than the one
    // whose type was read. It was opened once, then refused.
    write(&replacement);
    swap_in(replacement.clone());
    assert_eq!(
        inspect(&directory, "agentsandbox"),
        (Ok("replaced"), vec![OsString::from("agentsandbox")])
    );
    assert!(!replacement.exists(), "the hook did not run");
    write(&replacement);
    swap_in(replacement.clone());
    let refused = match artifact::digest(&bound, None) {
        Ok(digest) => panic!("an artefact replaced while it was opened was bound: {digest:?}"),
        Err(error) => format!("{error:#}"),
    };
    assert!(!replacement.exists(), "the hook did not run");
    assert!(
        refused.contains("was replaced while it was opened; nothing was bound"),
        "{refused}"
    );

    // A FIFO: its open does not wait for a writer, and it is refused as
    // another inode. The hook is a thread-local, so the inspection runs on
    // the thread that sets it; this thread bounds the wait.
    make_fifo(&base.join("fifo"));
    let (sender, receiver) = mpsc::channel();
    let inspection = std::thread::spawn({
        let base = base.clone();
        move || {
            let directory = fs::File::open(&base).expect("artefact directory");
            let (from, to) = (base.join("fifo"), base.join("agentsandbox"));
            BEFORE_OPEN.with(|slot| {
                *slot.borrow_mut() = Some(Box::new(move || {
                    fs::rename(from, to).expect("FIFO swapped in between fstatat and openat");
                }))
            });
            sender
                .send(inspect(&directory, "agentsandbox"))
                .expect("the test receives the result");
        }
    });
    let result = match receiver.recv_timeout(Duration::from_secs(10)) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("the open of a FIFO swapped in waited for a writer")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => match inspection.join() {
            Err(payload) => std::panic::resume_unwind(payload),
            Ok(()) => panic!("the inspection ended without a result"),
        },
    };
    inspection.join().expect("the inspection thread ended");
    assert_eq!(
        result,
        (Ok("replaced"), vec![OsString::from("agentsandbox")])
    );
}

#[test]
fn an_artifact_entry_name_must_be_one_path_component() {
    let (_root, base) = private_directory();
    fs::create_dir(base.join("directory")).expect("directory");
    let inner = base.join("directory").join("agentsandbox");
    fs::write(&inner, b"synthetic artefact bytes\n").expect("artefact");
    fs::set_permissions(&inner, fs::Permissions::from_mode(0o644)).expect("artefact mode");
    let directory = fs::File::open(&base).expect("artefact directory");
    // Without the check, `.`, `..` and `directory/` resolve to directories,
    // `directory/agentsandbox` to a regular file through another component,
    // and the empty name fails with ENOENT.
    for name in [
        OsStr::new(""),
        OsStr::new("."),
        OsStr::new(".."),
        OsStr::new("directory/"),
        OsStr::new("directory/agentsandbox"),
        OsStr::from_bytes(b"agent\0sandbox"),
    ] {
        OPENED.with(|opened| *opened.borrow_mut() = Some(Vec::new()));
        let inspected = artifact::open_regular(&directory, name).map(|_| ());
        let opened = artifact::open_at(
            &directory,
            name,
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
        .map(|_| ());
        let recorded = OPENED
            .with(|opened| opened.borrow_mut().take())
            .unwrap_or_default();
        for (call, result) in [("open_regular", inspected), ("open_at", opened)] {
            match result {
                Err(error) => assert_eq!(
                    error.kind(),
                    std::io::ErrorKind::InvalidInput,
                    "{call} {name:?}: {error}"
                ),
                Ok(()) => panic!("{call} resolved {name:?} below the directory"),
            }
        }
        assert!(recorded.is_empty(), "{name:?} reached openat: {recorded:?}");
    }
    assert!(matches!(
        artifact::open_regular(&directory, OsStr::new("directory")),
        Ok(artifact::Entry::Other)
    ));
}

#[test]
fn an_artifact_open_admits_only_the_read_only_lookup_flags() {
    let (_root, base) = private_directory();
    fs::create_dir(base.join("directory")).expect("directory");
    let directory = fs::File::open(&base).expect("artefact directory");
    let open = |name: &str, flags: libc::c_int| {
        OPENED.with(|opened| *opened.borrow_mut() = Some(Vec::new()));
        let result = artifact::open_at(&directory, OsStr::new(name), flags).map(|_| ());
        let recorded = OPENED
            .with(|opened| opened.borrow_mut().take())
            .unwrap_or_default();
        (result, recorded)
    };
    // With these flags libc reads a mode argument that `open_at` never
    // passes. Without the refusal, the name reaches the recorder, then
    // `openat` creates `created`, or on Linux an unnamed file in `directory`
    // (another error where the filesystem lacks O_TMPFILE).
    let refused = |name: &str, flags: libc::c_int| {
        let (result, recorded) = open(name, flags);
        match result {
            Err(error) => assert_eq!(
                error.kind(),
                std::io::ErrorKind::InvalidInput,
                "{name} {flags:#o}: {error}"
            ),
            Ok(()) => panic!("open_at accepted the flags {flags:#o} for {name}"),
        }
        assert!(
            recorded.is_empty(),
            "{name} {flags:#o} reached openat: {recorded:?}"
        );
    };
    refused(
        "created",
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
    );
    #[cfg(any(target_os = "linux", target_os = "android"))]
    refused(
        "directory",
        libc::O_RDWR | libc::O_TMPFILE | libc::O_CLOEXEC,
    );
    assert!(!base.join("created").exists(), "an entry was created");
    // O_TMPFILE holds O_DIRECTORY, which a directory lookup needs: the
    // lookup flags are admitted.
    let (result, recorded) = open(
        "directory",
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
    );
    assert_eq!(
        (result.map_err(|error| error.to_string()), recorded),
        (Ok(()), vec![OsString::from("directory")])
    );
}

#[test]
fn publication_with_unknown_durability_is_reported_and_removes_nothing() {
    let (_root, base) = private_directory();
    let parent = base.join("releases");
    let source = base.join("source");
    fs::create_dir(&parent).expect("parent");
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).expect("parent mode");
    fs::create_dir(&source).expect("source");
    let output = parent.join("release-1");
    let files: [(&'static str, Vec<u8>); 3] = [
        (document::MANIFEST_FILE, b"synthetic manifest\n".to_vec()),
        (document::SBOM_FILE, b"synthetic inventory\n".to_vec()),
        (document::SUMS_FILE, b"synthetic sums\n".to_vec()),
    ];
    let published = files.clone();
    let destination_output = output.clone();
    // The test hook is a thread-local: the publication runs on this thread
    // alone, never on a blocking pool.
    let result = std::thread::spawn(move || {
        let destination =
            output::destination(&destination_output, &source).expect("a new destination");
        FAIL_AFTER_RENAME.with(|flag| flag.set(true));
        let result =
            output::publish(&destination, &published).map_err(|error| format!("{error:#}"));
        FAIL_AFTER_RENAME.with(|flag| flag.set(false));
        result
    })
    .join()
    .expect("the publication thread ended");
    let message = match result {
        Ok(()) => panic!("an unknown durability was reported as a success"),
        Err(message) => message,
    };
    assert!(
        message.contains(
            "the rename was performed but its durability is unknown; ai-guard removes neither name"
        ),
        "{message}"
    );
    assert!(message.contains(&capture::shown(&output)), "{message}");
    assert!(
        message.contains(&capture::shown(&parent.join(".release-1.release-staging"))),
        "{message}"
    );
    assert!(message.contains("release verify --manifest"), "{message}");
    // The rename happened and nothing was removed: the final directory holds
    // every document as written.
    let mut names: Vec<String> = fs::read_dir(&output)
        .expect("the published directory is kept")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    let mut expected: Vec<String> = files.iter().map(|(name, _)| (*name).to_owned()).collect();
    expected.sort();
    assert_eq!(names, expected);
    for (name, bytes) in &files {
        assert_eq!(&fs::read(output.join(name)).expect("document"), bytes);
    }
    assert!(!parent.join(".release-1.release-staging").exists());

    // A second publication at the same place is refused before anything is
    // created: nothing is replaced.
    let source = base.join("source");
    assert!(output::destination(&output, &source).is_err());
    let open = parent.join("open");
    fs::create_dir(&open).expect("directory");
    fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).expect("mode");
    let refused = match output::destination(&open.join("release"), &source) {
        Ok(_) => panic!("a parent with mode 0755 was accepted"),
        Err(error) => format!("{error:#}"),
    };
    assert!(refused.contains("mkdir -m 700"), "{refused}");
}

#[test]
fn a_publication_that_did_not_rename_names_only_what_is_known_and_removes_nothing() {
    let (_root, base) = private_directory();
    let parent = base.join("releases");
    let source = base.join("source");
    fs::create_dir(&parent).expect("parent");
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).expect("parent mode");
    fs::create_dir(&source).expect("source");
    let files: [(&'static str, Vec<u8>); 3] = [
        (document::MANIFEST_FILE, b"synthetic manifest\n".to_vec()),
        (document::SBOM_FILE, b"synthetic inventory\n".to_vec()),
        (document::SUMS_FILE, b"synthetic sums\n".to_vec()),
    ];
    let holds_the_documents = |directory: &Path| {
        let mut names: Vec<String> = fs::read_dir(directory)
            .expect("the staging directory is kept")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        let mut expected: Vec<String> = files.iter().map(|(name, _)| (*name).to_owned()).collect();
        expected.sort();
        assert_eq!(names, expected, "{}", directory.display());
        for (name, bytes) in &files {
            assert_eq!(&fs::read(directory.join(name)).expect("document"), bytes);
        }
    };
    // Each hook runs on this thread, after the final name was last seen
    // absent and before the rename.
    let publish_with = |at: &Path, hook: output::BeforePublish| {
        let destination = output::destination(at, &source).expect("a new destination");
        output::BEFORE_PUBLISH.with(|slot| *slot.borrow_mut() = Some(hook));
        match output::publish(&destination, &files) {
            Ok(()) => panic!("a publication that did not rename was reported as a success"),
            Err(error) => format!("{error:#}"),
        }
    };

    // An entry created at the final name: the rename refuses to replace it.
    let output = parent.join("release-1");
    let staging = parent.join(".release-1.release-staging");
    let occupied = output.clone();
    let message = publish_with(
        &output,
        Box::new(move || {
            fs::create_dir(&occupied).expect("an entry at the final name");
            fs::write(occupied.join("marker"), b"kept\n").expect("marker");
        }),
    );
    for known in [
        format!(
            "the staging directory was not renamed to {} (the cause is not reported",
            capture::shown(&output)
        ),
        format!(
            "ai-guard replaced and removed nothing and did not move the staging directory it \
             created at {}",
            capture::shown(&staging)
        ),
    ] {
        assert!(message.contains(&known), "{message}");
    }
    assert_eq!(
        fs::read_dir(&output).expect("the entry is kept").count(),
        1,
        "nothing was renamed into the entry"
    );
    assert_eq!(fs::read(output.join("marker")).expect("marker"), b"kept\n");
    holds_the_documents(&staging);

    // The staging directory moved by someone else: the publication step
    // refuses before the rename, and the message does not claim that the
    // directory is still where it was created.
    let output = parent.join("release-2");
    let staging = parent.join(".release-2.release-staging");
    let moved = parent.join("moved-staging");
    let (from, to) = (staging.clone(), moved.clone());
    let message = publish_with(
        &output,
        Box::new(move || fs::rename(from, to).expect("the staging directory moved")),
    );
    assert!(
        message.contains(&format!(
            "the staging directory was not renamed to {}",
            capture::shown(&output)
        )),
        "{message}"
    );
    assert!(
        !message.contains(&format!("is kept at {}", capture::shown(&staging))),
        "{message}"
    );
    assert!(!output.exists() && !staging.exists());
    holds_the_documents(&moved);

    // Each hook below runs on this thread, after the documents were staged
    // and synchronised and before the final name is last checked absent.
    let check_with = |at: &Path, hook: output::BeforePublish| {
        let destination = output::destination(at, &source).expect("a new destination");
        output::BEFORE_FINAL_CHECK.with(|slot| *slot.borrow_mut() = Some(hook));
        match output::publish(&destination, &files) {
            Ok(()) => panic!("a publication refused before the rename was reported as a success"),
            Err(error) => format!("{error:#}"),
        }
    };
    let known_facts = |output: &Path, staging: &Path| {
        [
            format!(
                "{} was not confirmed absent before the rename",
                capture::shown(output)
            ),
            format!(
                "did not move the staging directory it created at {}",
                capture::shown(staging)
            ),
        ]
    };

    // An entry created at the final name: it is kept and nothing is renamed.
    let output = parent.join("release-3");
    let staging = parent.join(".release-3.release-staging");
    let occupied = output.clone();
    let message = check_with(
        &output,
        Box::new(move || {
            fs::create_dir(&occupied).expect("an entry at the final name");
            fs::write(occupied.join("marker"), b"kept\n").expect("marker");
        }),
    );
    for known in known_facts(&output, &staging) {
        assert!(message.contains(&known), "{message}");
    }
    assert_eq!(
        fs::read_dir(&output).expect("the entry is kept").count(),
        1,
        "nothing was renamed into the entry"
    );
    assert_eq!(fs::read(output.join("marker")).expect("marker"), b"kept\n");
    holds_the_documents(&staging);

    // The parent moved by someone else: the staging directory moved with it,
    // so the message claims no appearance and no place it did not check.
    let output = parent.join("release-4");
    let staging = parent.join(".release-4.release-staging");
    let moved = base.join("moved-releases");
    let (from, to) = (parent.clone(), moved.clone());
    let message = check_with(
        &output,
        Box::new(move || fs::rename(from, to).expect("the parent moved")),
    );
    for known in known_facts(&output, &staging) {
        assert!(message.contains(&known), "{message}");
    }
    assert!(!message.contains("appeared while"), "{message}");
    assert!(
        !message.contains(&format!("is kept at {}", capture::shown(&staging))),
        "{message}"
    );
    assert!(!parent.exists() && !moved.join("release-4").exists());
    holds_the_documents(&moved.join(".release-4.release-staging"));
}

fn repository_derived() -> Derived {
    let cargo = cargo_lock::parse(CARGO_LOCK).expect("the repository Cargo.lock is read");
    let npm = npm_lock::parse(NPM_LOCK, NPM_MANIFEST).expect("the repository lock is read");
    let facts = declared::read(CARGO_TOML, TOOLCHAIN, &repository_recipes());
    let inputs: [(&str, &[u8]); 7] = [
        ("Cargo.lock", CARGO_LOCK),
        ("Cargo.toml", CARGO_TOML),
        ("front/package-lock.json", NPM_LOCK),
        ("front/package.json", NPM_MANIFEST),
        ("images/control.Dockerfile", CONTROL_RECIPE),
        ("images/worker.Dockerfile", WORKER_RECIPE),
        ("rust-toolchain.toml", TOOLCHAIN),
    ];
    let entries: Vec<document::EntryV1> = inputs
        .iter()
        .map(|(path, bytes)| document::EntryV1 {
            path: (*path).to_owned(),
            bytes: bytes.len() as u64,
            sha256: sha256_hex(bytes),
        })
        .collect();
    let source = document::SourceV1 {
        scope: "build".to_owned(),
        digest: document::DigestV1 {
            alg: "SHA-256".to_owned(),
            layout: capture::DIGEST_LAYOUT.to_owned(),
            value: sha256_hex(b"synthetic source digest of a unit test"),
        },
        files: entries.len() as u64,
        bytes: entries.iter().map(|entry| entry.bytes).sum(),
        entries,
    };
    let locks = document::LocksV1 {
        cargo: document::CargoLockV1 {
            path: cargo_lock::CARGO_LOCK.to_owned(),
            bytes: CARGO_LOCK.len() as u64,
            sha256: sha256_hex(CARGO_LOCK),
            lock_version: cargo_lock::LOCK_VERSION,
            packages: cargo.packages.len() as u64,
            registry_packages: cargo.registry_packages() as u64,
            path_packages: cargo.path_packages() as u64,
        },
        npm: document::NpmLockV1 {
            path: npm_lock::NPM_LOCK.to_owned(),
            bytes: NPM_LOCK.len() as u64,
            sha256: sha256_hex(NPM_LOCK),
            lockfile_version: npm.lockfile_version,
            packages: npm.packages.len() as u64,
            dev: npm.packages.iter().filter(|package| package.dev).count() as u64,
            optional: npm
                .packages
                .iter()
                .filter(|package| package.optional)
                .count() as u64,
            install_scripts: Vec::new(),
        },
    };
    Derived {
        source,
        cargo,
        npm,
        declared: facts,
        locks,
    }
}

#[test]
fn canonical_manifest_round_trips_and_refuses_unknown_fields() {
    let derived = repository_derived();
    let inputs = DeclaredInputs {
        codex: Some("1.2.3".to_owned()),
        claude_code: None,
    };
    let identity = document::inputs_identity(
        &derived.source.digest.value,
        inputs.codex.as_deref(),
        inputs.claude_code.as_deref(),
    );
    let sbom = cyclonedx::sbom(&derived, &inputs, &identity).expect("inventory");
    let artifacts = [ArtifactDigest {
        name: "agentsandbox".to_owned(),
        bytes: 3,
        sha256: sha256_hex(b"abc"),
    }];
    let manifest = document::manifest(&derived, &inputs, &artifacts, &sbom);
    let bytes = document::canonical(&manifest).expect("canonical bytes");
    assert_eq!(bytes.last(), Some(&b'\n'));
    let parsed = document::parse_canonical(&bytes).expect("canonical bytes read back");
    assert_eq!(parsed, manifest);
    assert_eq!(document::canonical(&parsed).expect("bytes"), bytes);
    assert_eq!(
        document::canonical(&document::manifest(&derived, &inputs, &artifacts, &sbom))
            .expect("bytes"),
        bytes,
        "the same inputs give the same bytes"
    );
    assert_eq!(
        cyclonedx::sbom(&derived, &inputs, &identity).expect("inventory"),
        sbom
    );
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
    assert_eq!(value["build"]["target"]["status"], "unknown");
    assert_eq!(value["artifacts"][0]["profile"]["status"], "unknown");
    assert_eq!(value["artifacts"][0]["target"]["status"], "unknown");
    assert_eq!(
        value["build"]["inputs_identity"]["value"],
        identity.as_str()
    );

    let reserialised = |value: &serde_json::Value| {
        let mut bytes = serde_json::to_vec_pretty(value).expect("JSON");
        bytes.push(b'\n');
        bytes
    };
    let mut extra = value.clone();
    extra["extra"] = json!(1);
    assert!(document::parse_canonical(&reserialised(&extra)).is_err());
    let mut nested = value.clone();
    nested["build"]["target"]["value"] = json!("x86_64-unknown-linux-gnu");
    assert!(document::parse_canonical(&reserialised(&nested)).is_err());
    let mut mixed = value.clone();
    mixed["build"]["toolchain"]["reason"] = json!("both declared and unknown");
    assert!(document::parse_canonical(&reserialised(&mixed)).is_err());
    let spaced = String::from_utf8(bytes.clone())
        .expect("UTF-8")
        .replacen("\n", "\n ", 1);
    assert!(document::parse_canonical(spaced.as_bytes()).is_err());
    let mut unterminated = bytes.clone();
    unterminated.pop();
    assert!(document::parse_canonical(&unterminated).is_err());
    // The build-input identity, recomputed here from its documented layout.
    let mut hash = Sha256::new();
    hash.update(b"ai-guard:build-inputs:v1\0");
    hash.update(64_u64.to_be_bytes());
    hash.update(derived.source.digest.value.as_bytes());
    hash.update([1_u8]);
    hash.update(5_u64.to_be_bytes());
    hash.update(b"1.2.3");
    hash.update([0_u8]);
    hash.update(0_u64.to_be_bytes());
    assert_eq!(identity, hex::encode(hash.finalize()));
    // Fixed known answers, computed outside the product code: the 120-byte
    // layout input written with printf and hashed by `shasum -a 256`.
    let zeros = "0".repeat(64);
    assert_eq!(
        document::inputs_identity(&zeros, Some("1.2.3"), None),
        "593c4c98c508dcdf88d90ba65b9acfff9b55be108bcfe75d5873c78b110432dd"
    );
    assert_eq!(
        document::inputs_identity(&zeros, None, Some("2.0.1")),
        "ecdb5147b5dd88a831814bd47b017ea08e35eb54bf20b846938c56c95e7ea425"
    );
}

#[test]
fn a_source_divergence_without_a_listed_entry_names_the_differing_field() {
    let entry = |path: &str| document::EntryV1 {
        path: path.to_owned(),
        bytes: 3,
        sha256: sha256_hex(path.as_bytes()),
    };
    let prepared = document::SourceV1 {
        scope: "build".to_owned(),
        digest: document::DigestV1 {
            alg: "sha256".to_owned(),
            layout: "ai-guard:public-distribution:v1".to_owned(),
            value: "a".repeat(64),
        },
        files: 2,
        bytes: 6,
        entries: vec![entry("Cargo.lock"), entry("Cargo.toml")],
    };
    let mut files = prepared.clone();
    files.files = 3;
    let message = compare::source_divergence(&files, &prepared).to_string();
    assert!(
        message.ends_with("; no entry differs, source.files differs"),
        "{message}"
    );
    let mut value = prepared.clone();
    value.digest.value = "b".repeat(64);
    let message = compare::source_divergence(&value, &prepared).to_string();
    assert!(
        message.ends_with("; no entry differs, source.digest.value differs"),
        "{message}"
    );
    // A listed entry explains the divergence: no field is appended.
    let mut changed = prepared.clone();
    changed.entries[1].sha256 = sha256_hex(b"other bytes");
    let message = compare::source_divergence(&changed, &prepared).to_string();
    assert!(message.contains("changed: Cargo.toml;"), "{message}");
    assert!(!message.contains("no entry differs"), "{message}");
}

#[test]
fn verify_names_a_stop_request_or_a_lost_step_instead_of_an_unreadable_input() {
    let unverified = |refusal: Refusal| match refusal {
        Refusal::Unverified(unverified) => unverified,
        Refusal::Divergent(divergence) => panic!("a failed step was judged: {divergence}"),
    };
    // A stop request seen by a step of this module, under a context line.
    let stopped = anyhow::Error::from(InvocationFailure::Interrupted {
        signal: libc::SIGINT,
    })
    .context("artefact /synthetic/agentsandbox");
    assert_eq!(
        unverified(not_judged("--artifact", stopped, None)),
        Unverified::Interrupted {
            signal: libc::SIGINT,
            reason: None,
        }
    );
    // The preparation words its own interruption: the stop request decides,
    // and the step's words are kept.
    let stop = Interruption::new();
    stop.request(libc::SIGTERM);
    let preparation = anyhow::anyhow!("interrupted by SIGTERM while the source was being prepared");
    assert_eq!(
        unverified(not_judged("--source", preparation, Some(&stop))),
        Unverified::Interrupted {
            signal: libc::SIGTERM,
            reason: Some("interrupted by SIGTERM while the source was being prepared".to_owned()),
        }
    );
    // A step that ended without a result.
    let lost = anyhow::Error::from(StepLost {
        step: "derivation",
        consequence: super::NOTHING_PUBLISHED.to_owned(),
    });
    let incomplete = unverified(not_judged("--source", lost, None));
    assert!(
        matches!(incomplete, Unverified::Incomplete { .. }),
        "{incomplete:?}"
    );
    // Anything else, without a stop request: the input is unreadable.
    let refused = anyhow::anyhow!("an artefact must be a regular file");
    let reported = unverified(not_judged("--artifact", refused, None));
    assert!(
        matches!(
            reported,
            Unverified::Unreadable {
                input: "--artifact",
                ..
            }
        ),
        "{reported:?}"
    );
    // None of the outcomes that are not about an input says it was unreadable.
    for outcome in [
        Unverified::Conflict {
            reason: "--source-only verifies no artefact and cannot be combined with --artifact",
        },
        Unverified::Interrupted {
            signal: libc::SIGINT,
            reason: None,
        },
        incomplete,
    ] {
        let text = outcome.to_string();
        assert!(text.starts_with("UNVERIFIED: "), "{text}");
        assert!(
            !text.contains("could not be read under its policy"),
            "{text}"
        );
        assert!(
            !text.contains("DIVERGENT") && !text.contains("MATCH"),
            "{text}"
        );
    }
}

/// A distribution of real build files in a private directory, prepared by
/// `capture` and `confirm` into a private temporary directory of its own.
struct Retained {
    snapshot: Snapshot,
    originals: BTreeMap<String, Vec<u8>>,
    _root: tempfile::TempDir,
}

fn retained() -> Retained {
    let (root, base) = private_directory();
    let source = base.join("project");
    let temporary = base.join("tmp");
    fs::create_dir(&temporary).expect("temporary directory");
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o700)).expect("mode");
    let inputs: [(&str, &[u8]); 9] = [
        (".dockerignore", DOCKERIGNORE),
        ("Cargo.lock", CARGO_LOCK),
        ("Cargo.toml", CARGO_TOML),
        ("front/package-lock.json", NPM_LOCK),
        ("front/package.json", NPM_MANIFEST),
        ("images/control.Dockerfile", CONTROL_RECIPE),
        ("images/worker.Dockerfile", WORKER_RECIPE),
        ("rust-toolchain.toml", TOOLCHAIN),
        ("src/main.rs", b"fn main() {}\n"),
    ];
    let mut originals = BTreeMap::new();
    for (relative, bytes) in inputs {
        let path = source.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("source directory");
        fs::write(path, bytes).expect("source file");
        originals.insert(relative.to_owned(), bytes.to_vec());
    }
    // Not build inputs: never part of the copy.
    fs::write(source.join("notes.txt"), b"not a build input\n").expect("file");
    fs::write(source.join(".env"), b"SYNTHETIC=not-a-secret\n").expect("file");
    let snapshot = capture::capture(&source, Scope::Build, Limits::default(), &temporary, None)
        .and_then(|captured| captured.confirm(None))
        .expect("the fixture is prepared");
    Retained {
        snapshot,
        originals,
        _root: root,
    }
}

#[test]
fn retained_reads_return_the_captured_bytes_or_nothing() {
    let fixture = retained();
    let snapshot = &fixture.snapshot;
    let names = [
        "Cargo.lock",
        "front/package-lock.json",
        "images/worker.Dockerfile",
    ];
    let read = snapshot
        .read_retained(&names, MAX_DECLARATION_BYTES)
        .expect("retained entries are read");
    assert_eq!(read.len(), names.len());
    for (name, bytes) in names.iter().zip(&read) {
        assert_eq!(bytes, &fixture.originals[*name], "{name}");
    }
    let lock_length = CARGO_LOCK.len() as u64;
    for (refused, bound) in [
        (vec![], MAX_DECLARATION_BYTES),
        (vec!["notes.txt"], MAX_DECLARATION_BYTES),
        (vec![".env"], MAX_DECLARATION_BYTES),
        (vec!["Cargo.lock", "Cargo.lock"], MAX_DECLARATION_BYTES),
        (vec!["Cargo.lock"], lock_length - 1),
        (vec!["Cargo.lock"], control_fs::MAX_PRIVATE_FILE_BYTES + 1),
    ] {
        assert!(
            snapshot.read_retained(&refused, bound).is_err(),
            "{refused:?} with bound {bound} was read"
        );
    }
    let many: Vec<&str> = std::iter::repeat_n("Cargo.lock", 65).collect();
    assert!(
        snapshot
            .read_retained(&many, MAX_DECLARATION_BYTES)
            .is_err()
    );
    // The copy changed after it was prepared: no byte is returned at all,
    // even for an entry that did not change.
    fs::write(
        snapshot.context().join("Cargo.toml"),
        b"[package]\nname = \"other\"\n",
    )
    .expect("the private copy is writable by its owner");
    assert!(
        snapshot
            .read_retained(&["Cargo.lock"], MAX_DECLARATION_BYTES)
            .is_err()
    );
}

#[test]
fn digest_is_rederived_from_the_retained_entries_with_the_recorded_layout() {
    let fixture = retained();
    let snapshot = &fixture.snapshot;
    let entries: Vec<(String, u64, String)> = snapshot
        .entries()
        .map(|entry| {
            (
                entry.name().to_owned(),
                entry.bytes(),
                entry.sha256().to_owned(),
            )
        })
        .collect();
    let expected: Vec<(String, u64, String)> = fixture
        .originals
        .iter()
        .map(|(name, bytes)| (name.clone(), bytes.len() as u64, sha256_hex(bytes)))
        .collect();
    assert_eq!(entries, expected, "sorted entries of the build inputs only");
    assert_eq!(
        snapshot.total_bytes(),
        expected.iter().map(|(_, bytes, _)| bytes).sum::<u64>()
    );
    let mut domain = capture::DIGEST_LAYOUT.as_bytes().to_vec();
    domain.push(0);
    let mut hash = Sha256::new();
    hash.update(&domain);
    for (name, bytes, sha256) in &entries {
        hash.update((name.len() as u64).to_be_bytes());
        hash.update(name.as_bytes());
        hash.update(bytes.to_be_bytes());
        hash.update(sha256.as_bytes());
    }
    assert_eq!(snapshot.digest(), hex::encode(hash.finalize()));
    assert_eq!(capture::DIGEST_LAYOUT, "ai-guard:public-distribution:v1");
    assert_eq!(sha256_hex(DOCKERIGNORE), capture::BUILD_IGNORE_SHA256);
}
