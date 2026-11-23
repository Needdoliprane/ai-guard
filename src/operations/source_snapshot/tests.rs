//! Unit tests of the prepared source: capture, confirmation, verification of
//! the copy, the closed argument grammar, the interruption scope logic and
//! what the operator is told about each end of a prepared command.
//!
//! The lifecycle of a prepared source across real engine CLI processes is
//! covered end to end by `tests/source_lifecycle.rs`.
mod build_identity;

use super::{
    PreparedSource,
    capture::{
        BUILD_IGNORE_SHA256, Captured, Limits, MAX_COMPOSE_BYTES, MAX_DEPTH, PREFIX, Scope,
        Snapshot, SourceRoot, capture,
    },
    concluded,
    interrupt::{self, SignalSource, scope_with},
};
use crate::runtime::{
    Capture, RuntimeCommandFailure, RuntimeKind,
    retained_cli::{
        Collected, Interruption, InvocationEnd, InvocationFailure, Stop, StopReason, UnknownEnd,
    },
};
use anyhow::{Result, anyhow};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::{
        fs::{MetadataExt, PermissionsExt, symlink},
        process::ExitStatusExt,
    },
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const DOCKERIGNORE: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/.dockerignore"));
const COMPOSE_FILE: &str = "deploy/docker/compose.yaml";

/// Build inputs of the synthetic source.
const INPUTS: [&str; 23] = [
    ".dockerignore",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "LICENSE",
    "images/control.Dockerfile",
    "images/worker.Dockerfile",
    "src/main.rs",
    "src/lib.rs",
    "src/bin/ai-guard-provider.rs",
    "src/x/secrets/extra.rs",
    "front/package.json",
    "front/package-lock.json",
    "front/tsconfig.json",
    "front/vite.config.ts",
    "front/index.html",
    "front/src/main.ts",
    "front/src/style.css",
    "front/src/views/extra.ts",
    "front/src/theme/extra.css",
    "front/public/favicon.svg",
    "policies/default.json",
    "policies/extra.json",
];
/// Files of the synthetic source that the build allowlist does not admit.
const OTHERS: [&str; 10] = [
    "notes.txt",
    "src/data.bin",
    "src/x/notes.md",
    "policies/nested/deep.json",
    "front/extra.json",
    "front/src/notes.md",
    "front/public/other.svg",
    "images/notes.md",
    "target/debug/junk.rs",
    COMPOSE_FILE,
];

/// A synthetic distribution and a private temporary directory of its own, so
/// what a capture leaves behind can be listed exactly.
struct Fixture {
    source: PathBuf,
    temporary: PathBuf,
    _root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .expect("private test directory");
        let base = root
            .path()
            .canonicalize()
            .expect("canonical test directory");
        let fixture = Self {
            source: base.join("project"),
            temporary: base.join("tmp"),
            _root: root,
        };
        fs::create_dir(&fixture.temporary).expect("temporary directory");
        fs::set_permissions(&fixture.temporary, fs::Permissions::from_mode(0o700))
            .expect("temporary directory mode");
        fixture.populate();
        fixture
    }

    /// Writes the synthetic distribution at the source path.
    fn populate(&self) {
        for relative in INPUTS.iter().chain(&OTHERS) {
            self.write(relative, &format!("synthetic input: {relative}\n"));
        }
        fs::write(self.source.join(".dockerignore"), DOCKERIGNORE).expect("build allowlist");
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.source.join(relative);
        fs::create_dir_all(path.parent().expect("relative file has a parent"))
            .expect("source directory");
        fs::write(path, content).expect("source file");
    }

    fn capture(&self, scope: Scope, limits: Limits) -> Result<Captured> {
        capture(&self.source, scope, limits, &self.temporary, None)
    }

    fn prepare(&self, scope: Scope) -> Snapshot {
        self.capture(scope, Limits::default())
            .and_then(|captured| captured.confirm(None))
            .expect("the synthetic source is prepared")
    }

    /// Names left in the private temporary directory.
    fn leftovers(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.temporary)
            .expect("temporary directory listing")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }
}

/// The message of a refusal, with its causes.
fn refusal<T>(result: Result<T>) -> String {
    match result {
        Ok(_) => panic!("accepted what must be refused"),
        Err(error) => format!("{error:#}"),
    }
}

/// The message of a refusal without its causes: the text of an operating
/// system error under it differs between platforms.
fn headline<T>(result: Result<T>) -> String {
    match result {
        Ok(_) => panic!("accepted what must be refused"),
        Err(error) => error.to_string(),
    }
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).expect("metadata").mode() & 0o7777
}

/// Relative path, mode and bytes of every entry below `root`.
fn tree(root: &Path) -> BTreeMap<String, (u32, Option<Vec<u8>>)> {
    fn visit(root: &Path, relative: &Path, entries: &mut BTreeMap<String, (u32, Option<Vec<u8>>)>) {
        for entry in fs::read_dir(root.join(relative)).expect("tree listing") {
            let relative = relative.join(entry.expect("tree entry").file_name());
            let path = root.join(&relative);
            let directory = fs::symlink_metadata(&path).expect("metadata").is_dir();
            let bytes = (!directory).then(|| fs::read(&path).expect("content"));
            entries.insert(
                relative.to_string_lossy().into_owned(),
                (mode(&path), bytes),
            );
            if directory {
                visit(root, &relative, entries);
            }
        }
    }
    let mut entries = BTreeMap::new();
    visit(root, Path::new(""), &mut entries);
    entries
}

fn field<'a>(line: &'a str, name: &str) -> &'a str {
    line.split_whitespace()
        .find_map(|item| item.strip_prefix(name))
        .unwrap_or_else(|| panic!("the provenance line has no {name} field: {line}"))
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn capture_is_private_content_stable_and_its_release_removes_only_the_copy() {
    let fixture = Fixture::new();
    let original = tree(&fixture.source);
    let first = fixture.prepare(Scope::Build);
    let second = fixture.prepare(Scope::Build);

    let copy = tree(first.context());
    let files: Vec<&str> = copy
        .iter()
        .filter(|(_, (_, bytes))| bytes.is_some())
        .map(|(path, _)| path.as_str())
        .collect();
    let mut expected: Vec<&str> = INPUTS.to_vec();
    expected.sort_unstable();
    assert_eq!(files, expected, "the copy holds exactly the build inputs");
    for (path, (bits, bytes)) in &copy {
        match bytes {
            Some(bytes) => {
                assert_eq!(*bits, 0o600, "{path}");
                assert_eq!(
                    bytes,
                    &fs::read(fixture.source.join(path)).unwrap(),
                    "{path}"
                );
                assert!(first.holds(path), "{path}");
            }
            None => assert_eq!(*bits, 0o700, "{path}"),
        }
    }
    assert_eq!(mode(first.root()), 0o700);
    assert_eq!(mode(first.context()), 0o700);
    assert_eq!(mode(&first.root().join("manifest.json")), 0o600);
    assert!(first.env_file().is_none());
    assert!(
        first
            .root()
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(PREFIX))
    );

    let (line, again) = (first.provenance_line(), second.provenance_line());
    assert!(line.starts_with("Prepared public distribution (build): sha256="));
    assert_eq!(field(&line, "sha256="), field(&again, "sha256="));
    assert_eq!(field(&line, "files="), INPUTS.len().to_string());
    assert_ne!(first.root(), second.root());
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(first.root().join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["sha256"], field(&line, "sha256="));
    assert_eq!(manifest["entries"].as_array().unwrap().len(), INPUTS.len());

    let kept = second.root().to_owned();
    drop(first);
    assert_eq!(
        fixture.leftovers().len(),
        1,
        "only the first copy is removed"
    );
    assert!(kept.is_dir());
    second.verify().expect("the other copy is untouched");
    drop(second);
    assert!(fixture.leftovers().is_empty());
    assert_eq!(
        tree(&fixture.source),
        original,
        "the original is never written"
    );
}

#[test]
fn allowlist_matches_the_pinned_dockerignore_and_admits_nested_rust_sources() {
    // The capture allowlist mirrors the `.dockerignore` of this repository,
    // and the CLI refuses a build source whose file has another digest.
    assert_eq!(
        hex::encode(Sha256::digest(DOCKERIGNORE)),
        BUILD_IGNORE_SHA256,
        ".dockerignore changed: update BUILD_IGNORE_SHA256 and the allowlist of \
         Scope::selects_file in src/operations/source_snapshot/capture.rs together"
    );
    // The repository ships no per-Dockerfile ignore file: the CLI refuses a
    // build source that holds one.
    let images = Path::new(env!("CARGO_MANIFEST_DIR")).join("images");
    for entry in fs::read_dir(&images).expect("images directory of the repository") {
        let name = entry.expect("images entry").file_name();
        assert!(
            !name.to_string_lossy().ends_with(".Dockerfile.dockerignore"),
            "images/{} is a per-Dockerfile ignore file: the capture refuses such a file",
            name.to_string_lossy()
        );
    }
    let fixture = Fixture::new();
    let snapshot = fixture.prepare(Scope::Build);
    // A component named like a private directory does not hide a build input.
    assert!(snapshot.holds("src/x/secrets/extra.rs"));
    for other in OTHERS {
        assert!(!snapshot.holds(other), "{other} is not a build input");
        assert!(!snapshot.context().join(other).exists(), "{other}");
    }
}

#[test]
fn bounds_refuse_an_oversized_source_and_leave_nothing_behind() {
    let fixture = Fixture::new();
    // Every other input of the synthetic source is far smaller than this one.
    fixture.write("src/big.rs", &"x".repeat(4096));
    let per_file = refusal(fixture.capture(
        Scope::Build,
        Limits {
            file_bytes: 4095,
            ..Limits::default()
        },
    ));
    assert!(
        per_file.contains("per-file bound of 4095 bytes") && per_file.contains("src/big.rs"),
        "{per_file}"
    );
    assert!(fixture.leftovers().is_empty());

    let total = refusal(fixture.capture(
        Scope::Build,
        Limits {
            bytes: 64,
            ..Limits::default()
        },
    ));
    assert!(total.contains("total bound of 64 bytes"), "{total}");
    assert!(fixture.leftovers().is_empty());

    let entries = refusal(fixture.capture(
        Scope::Build,
        Limits {
            entries: 3,
            ..Limits::default()
        },
    ));
    assert!(entries.contains("bound of 3 entries"), "{entries}");
    assert!(fixture.leftovers().is_empty());

    for limits in [
        Limits {
            entries: 0,
            ..Limits::default()
        },
        Limits {
            entries: Limits::default().entries + 1,
            ..Limits::default()
        },
        Limits {
            bytes: Limits::default().bytes + 1,
            ..Limits::default()
        },
        Limits {
            file_bytes: Limits::default().file_bytes + 1,
            ..Limits::default()
        },
    ] {
        let invalid = refusal(fixture.capture(Scope::Build, limits));
        assert!(
            invalid.contains("invalid prepared-source bounds"),
            "{invalid}"
        );
    }

    fixture.write(COMPOSE_FILE, &"#".repeat(MAX_COMPOSE_BYTES as usize + 1));
    let compose = refusal(fixture.capture(Scope::Compose(RuntimeKind::Docker), Limits::default()));
    assert!(
        compose.contains("per-file bound") && compose.contains(COMPOSE_FILE),
        "{compose}"
    );
    assert!(fixture.leftovers().is_empty());
}

#[test]
fn links_special_files_and_deep_paths_are_refused_by_name() {
    let linked_file = Fixture::new();
    symlink(
        linked_file.source.join("Cargo.toml"),
        linked_file.source.join("src/linked.rs"),
    )
    .unwrap();
    let message = refusal(linked_file.capture(Scope::Build, Limits::default()));
    assert!(
        message.contains("must not be a symbolic link") && message.contains("src/linked.rs"),
        "{message}"
    );
    assert!(linked_file.leftovers().is_empty());

    // A linked directory is refused, not silently left out of the copy.
    let linked_directory = Fixture::new();
    fs::remove_dir_all(linked_directory.source.join("front/src/views")).unwrap();
    symlink(
        linked_directory.source.join("front/src/theme"),
        linked_directory.source.join("front/src/views"),
    )
    .unwrap();
    let message = refusal(linked_directory.capture(Scope::Build, Limits::default()));
    assert!(
        message.contains("must not be a symbolic link") && message.contains("front/src/views"),
        "{message}"
    );

    let linked_compose = Fixture::new();
    fs::rename(
        linked_compose.source.join(COMPOSE_FILE),
        linked_compose.source.join("deploy/docker/real.yaml"),
    )
    .unwrap();
    symlink("real.yaml", linked_compose.source.join(COMPOSE_FILE)).unwrap();
    let message =
        refusal(linked_compose.capture(Scope::Compose(RuntimeKind::Docker), Limits::default()));
    assert!(
        message.contains("must not be a symbolic link") && message.contains(COMPOSE_FILE),
        "{message}"
    );

    let special = Fixture::new();
    let pipe = std::ffi::CString::new(
        special
            .source
            .join("src/pipe.rs")
            .to_str()
            .expect("UTF-8 test path"),
    )
    .unwrap();
    // SAFETY: `pipe` is a NUL-terminated path owned by this test.
    assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) }, 0);
    let message = refusal(special.capture(Scope::Build, Limits::default()));
    assert!(
        message.contains("must be a regular file") && message.contains("src/pipe.rs"),
        "{message}"
    );
    assert!(special.leftovers().is_empty());

    let deep = Fixture::new();
    let nested = "d/".repeat(MAX_DEPTH);
    deep.write(&format!("src/{nested}deep.rs"), "deep\n");
    let message = refusal(deep.capture(Scope::Build, Limits::default()));
    assert!(
        message.contains(&format!("exceeds the bound of {MAX_DEPTH} components"))
            && message.contains("src/d/d/"),
        "{message}"
    );

    // Links and special files outside the selected paths are not inputs.
    let unrelated = Fixture::new();
    symlink("notes.txt", unrelated.source.join("notes-link")).unwrap();
    symlink(
        "deep.json",
        unrelated.source.join("policies/nested/link.json"),
    )
    .unwrap();
    symlink("../LICENSE", unrelated.source.join("policies/license.txt")).unwrap();
    let snapshot = unrelated.prepare(Scope::Build);
    assert!(!snapshot.context().join("notes-link").exists());
}

#[test]
fn a_source_file_writable_by_group_is_accepted_and_counted() {
    let fixture = Fixture::new();
    let target = fixture.source.join("src/lib.rs");
    let counted = |bits: u32| -> usize {
        fs::set_permissions(&target, fs::Permissions::from_mode(bits)).unwrap();
        let snapshot = fixture.prepare(Scope::Build);
        assert_eq!(mode(&snapshot.context().join("src/lib.rs")), 0o600);
        field(&snapshot.provenance_line(), "shared_writable=")
            .parse()
            .expect("a count")
    };
    // Whatever the umask of the test process made of the other files.
    let private = counted(0o600);
    assert_eq!(counted(0o664), private + 1);
    assert_eq!(counted(0o646), private + 1);
}

#[test]
fn a_source_changed_between_capture_and_confirmation_is_refused_with_its_path() {
    let changed = Fixture::new();
    let captured = changed.capture(Scope::Build, Limits::default()).unwrap();
    // Another length: the refusal does not depend on timestamp granularity.
    changed.write("Cargo.toml", "rewritten\n");
    assert_eq!(
        refusal(captured.confirm(None)),
        "source changed while it was being prepared: Cargo.toml"
    );
    assert!(changed.leftovers().is_empty());

    let added = Fixture::new();
    let captured = added.capture(Scope::Build, Limits::default()).unwrap();
    added.write("src/added.rs", "added after the enumeration\n");
    assert_eq!(
        refusal(captured.confirm(None)),
        "source changed while it was being prepared: src/added.rs"
    );
    assert!(added.leftovers().is_empty());

    let removed = Fixture::new();
    let captured = removed.capture(Scope::Build, Limits::default()).unwrap();
    fs::remove_file(removed.source.join("policies/extra.json")).unwrap();
    assert_eq!(
        refusal(captured.confirm(None)),
        "source changed while it was being prepared: policies/extra.json"
    );
    assert!(removed.leftovers().is_empty());

    // A change outside the selected paths is not a change of the inputs.
    let unrelated = Fixture::new();
    let captured = unrelated.capture(Scope::Build, Limits::default()).unwrap();
    unrelated.write("notes.txt", "rewritten\n");
    unrelated.write("src/data.bin", "rewritten\n");
    captured
        .confirm(None)
        .expect("unrelated files are not inputs");
}

#[test]
fn read_stable_returns_the_record_of_the_descriptor_it_read() {
    let fixture = Fixture::new();
    let source = SourceRoot::open(&fixture.source).unwrap();
    let relative = Path::new("Cargo.toml");
    let (bytes, record) = source.read_stable(relative, 1024, 1024, 0).unwrap();
    assert_eq!(bytes, fs::read(fixture.source.join(relative)).unwrap());
    assert_eq!(source.record(relative).unwrap(), record);

    // Another file with the same bytes under the same name is another record.
    let staged = fixture.source.join("Cargo.toml.staged");
    fs::write(&staged, &bytes).unwrap();
    fs::rename(&staged, fixture.source.join(relative)).unwrap();
    assert_ne!(source.record(relative).unwrap(), record);

    let bounded = refusal(source.read_stable(relative, 1, 1024, 0));
    assert!(bounded.contains("per-file bound of 1 bytes"), "{bounded}");
    // What the copy already holds counts against the bound of the whole copy.
    let taken = 1024 - bytes.len() as u64;
    source.read_stable(relative, 1024, 1024, taken).unwrap();
    let exhausted = refusal(source.read_stable(relative, 1024, 1024, taken + 1));
    assert!(
        exhausted.contains("total bound of 1024 bytes") && exhausted.contains("Cargo.toml"),
        "{exhausted}"
    );
    symlink("Cargo.toml", fixture.source.join("linked.toml")).unwrap();
    let linked = refusal(source.read_stable(Path::new("linked.toml"), 1024, 1024, 0));
    assert!(linked.contains("symbolic link"), "{linked}");
    let directory = refusal(source.read_stable(Path::new("src"), 1024, 1024, 0));
    assert!(directory.contains("must be a regular file"), "{directory}");
    let outside = refusal(source.read_stable(Path::new("../project/Cargo.toml"), 1024, 1024, 0));
    assert!(outside.contains("invalid source path"), "{outside}");
}

#[test]
fn a_stop_request_ends_the_capture_and_leaves_nothing_behind() {
    let fixture = Fixture::new();
    let stop = Interruption::new();
    stop.request(libc::SIGINT);
    let stopped = capture(
        &fixture.source,
        Scope::Build,
        Limits::default(),
        &fixture.temporary,
        Some(&stop),
    );
    let error = match stopped {
        Ok(_) => panic!("the capture ignored a stop request"),
        Err(error) => error,
    };
    assert_eq!(
        error.downcast_ref::<InvocationFailure>(),
        Some(&InvocationFailure::Interrupted {
            signal: libc::SIGINT
        })
    );
    assert!(fixture.leftovers().is_empty());

    // The same request between the capture and its confirmation.
    let captured = fixture.capture(Scope::Build, Limits::default()).unwrap();
    let error = match captured.confirm(Some(&stop)) {
        Ok(_) => panic!("the confirmation ignored a stop request"),
        Err(error) => error,
    };
    assert!(error.downcast_ref::<InvocationFailure>().is_some());
    assert!(fixture.leftovers().is_empty());
}

#[test]
fn verify_rejects_altered_missing_and_extra_entries_of_the_copy() {
    let fixture = Fixture::new();

    let altered = fixture.prepare(Scope::Build);
    altered.verify().expect("an untouched copy verifies");
    let target = altered.context().join("Cargo.toml");
    let mut bytes = fs::read(&target).unwrap();
    bytes.push(b'#');
    fs::write(&target, &bytes).unwrap();
    let message = refusal(altered.verify());
    assert!(
        message.starts_with("prepared copy content changed: Cargo.toml"),
        "{message}"
    );

    let same_length = fixture.prepare(Scope::Build);
    let target = same_length.context().join("src/lib.rs");
    let mut bytes = fs::read(&target).unwrap();
    bytes[0] ^= 1;
    fs::write(&target, &bytes).unwrap();
    let message = refusal(same_length.verify());
    assert!(
        message.starts_with("prepared copy content changed: src/lib.rs"),
        "{message}"
    );

    let missing = fixture.prepare(Scope::Build);
    fs::remove_file(missing.context().join("src/lib.rs")).unwrap();
    assert_eq!(
        refusal(missing.verify()),
        "prepared copy entry disappeared: src/lib.rs"
    );

    let extra = fixture.prepare(Scope::Build);
    fs::write(extra.context().join("images/extra.txt"), "planted\n").unwrap();
    let message = refusal(extra.verify());
    assert!(
        message.starts_with("unexpected entry in the prepared copy: images/extra.txt"),
        "{message}"
    );

    let extra_directory = fixture.prepare(Scope::Build);
    fs::create_dir(extra_directory.context().join("planted")).unwrap();
    let message = refusal(extra_directory.verify());
    assert!(
        message.starts_with("unexpected entry in the prepared copy: planted"),
        "{message}"
    );

    let linked = fixture.prepare(Scope::Build);
    let target = linked.context().join("LICENSE");
    fs::remove_file(&target).unwrap();
    symlink(fixture.source.join("LICENSE"), &target).unwrap();
    let message = refusal(linked.verify());
    assert!(
        message.starts_with("prepared copy content changed: LICENSE"),
        "{message}"
    );

    // The original has no say once the copy exists.
    let independent = fixture.prepare(Scope::Build);
    fixture.write("Cargo.toml", "rewritten after preparation\n");
    fs::remove_file(fixture.source.join("src/lib.rs")).unwrap();
    independent
        .verify()
        .expect("the copy does not depend on the original");
}

#[test]
fn compose_scope_captures_exactly_one_file_and_an_empty_environment_file() {
    let fixture = Fixture::new();
    let snapshot = fixture.prepare(Scope::Compose(RuntimeKind::Docker));
    let copy = tree(snapshot.context());
    let files: Vec<&str> = copy
        .iter()
        .filter(|(_, (_, bytes))| bytes.is_some())
        .map(|(path, _)| path.as_str())
        .collect();
    assert_eq!(files, [COMPOSE_FILE]);
    assert_eq!(
        copy[COMPOSE_FILE].1.as_deref(),
        Some(
            fs::read(fixture.source.join(COMPOSE_FILE))
                .unwrap()
                .as_slice()
        )
    );
    assert!(snapshot.holds(COMPOSE_FILE) && !snapshot.holds("Cargo.toml"));
    let env_file = snapshot
        .env_file()
        .expect("a Compose scope has an env file");
    assert!(env_file.starts_with(snapshot.root()) && !env_file.starts_with(snapshot.context()));
    assert_eq!(fs::read(env_file).unwrap(), b"");
    assert_eq!(mode(env_file), 0o600);
    assert!(
        snapshot
            .provenance_line()
            .starts_with("Prepared public distribution (compose): sha256=")
    );
    assert_eq!(field(&snapshot.provenance_line(), "files="), "1");

    fs::write(env_file, "PLANTED=1\n").unwrap();
    let message = refusal(snapshot.verify());
    assert!(
        message.starts_with("prepared Compose environment file changed"),
        "{message}"
    );
    drop(snapshot);

    // The other engine's file is not this scope's file.
    let message = refusal(
        fixture
            .capture(Scope::Compose(RuntimeKind::Podman), Limits::default())
            .and_then(|captured| captured.confirm(None)),
    );
    assert_eq!(
        message,
        "compose file is missing: deploy/podman/compose.yaml"
    );
    assert!(fixture.leftovers().is_empty());
}

#[test]
fn build_and_compose_commands_accept_only_their_closed_grammar() {
    let fixture = Fixture::new();
    let build = PreparedSource {
        snapshot: Arc::new(fixture.prepare(Scope::Build)),
    };
    let context = build.snapshot.context().to_str().unwrap().to_owned();
    let plain = build
        .build_command(
            "images/control.Dockerfile",
            &strings(&["build", "--tag", "agentsandbox-control:local"]),
        )
        .unwrap();
    assert_eq!(
        plain.arguments,
        strings(&[
            "build",
            "--file",
            &format!("{context}/images/control.Dockerfile"),
            "--tag",
            "agentsandbox-control:local",
            &context,
        ])
    );
    let versioned = build
        .build_command(
            "images/worker.Dockerfile",
            &strings(&[
                "build",
                "--tag",
                "agentsandbox-worker:local",
                "--build-arg",
                "CODEX_VERSION=1.2.3",
                "--build-arg",
                "CLAUDE_CODE_VERSION=4.5.6",
            ]),
        )
        .unwrap();
    assert_eq!(
        versioned.arguments,
        strings(&[
            "build",
            "--file",
            &format!("{context}/images/worker.Dockerfile"),
            "--tag",
            "agentsandbox-worker:local",
            "--build-arg",
            "CODEX_VERSION=1.2.3",
            "--build-arg",
            "CLAUDE_CODE_VERSION=4.5.6",
            &context,
        ])
    );

    let tagged = ["build", "--tag", "agentsandbox-control:local"];
    for dockerfile in [
        "images/missing.Dockerfile",
        "Cargo.toml",
        "images/../Cargo.toml",
        "images/notes.md",
        "/images/control.Dockerfile",
        "images/nested/control.Dockerfile",
    ] {
        let message = refusal(build.build_command(dockerfile, &strings(&tagged)));
        assert!(
            message.contains("not a prepared build input"),
            "{dockerfile}: {message}"
        );
    }
    for options in [
        &["build"][..],
        &["build", "--tag"],
        &["compose", "--tag", "agentsandbox-control:local"],
        &["build", "--file", "agentsandbox-control:local"],
        &["build", "--tag", "-agentsandbox:local"],
        &["build", "--tag", "agentsandbox-control"],
        &["build", "--tag", "/etc/passwd:local"],
        &["build", "--tag", "Agentsandbox:local"],
        &["build", "--tag", "a:b", "--tag", "c:d"],
        &["build", "--tag", "a:b", "--file", "/etc/passwd"],
        &["build", "--tag", "a:b", "--build-context", "extra=/etc"],
        &["build", "--tag", "a:b", "--build-arg"],
        &["build", "--tag", "a:b", "--build-arg", "CODEX_VERSION"],
        &["build", "--tag", "a:b", "--build-arg", "OTHER=1.2.3"],
        &[
            "build",
            "--tag",
            "a:b",
            "--build-arg",
            "CODEX_VERSION=latest",
        ],
        &[
            "build",
            "--tag",
            "a:b",
            "--build-arg",
            "CODEX_VERSION=1.2.3",
            "--build-arg",
            "CODEX_VERSION=1.2.4",
        ],
        &["build", "--tag", "a:b", "/etc"],
    ] {
        assert!(
            build
                .build_command("images/control.Dockerfile", &strings(options))
                .is_err(),
            "accepted build options {options:?}"
        );
    }
    let down = ["compose", "--project-name", "agentsandbox", "down"];
    let message = refusal(build.compose_command(&strings(&down)));
    assert!(message.contains("holds no Compose file"), "{message}");

    let compose = PreparedSource {
        snapshot: Arc::new(fixture.prepare(Scope::Compose(RuntimeKind::Docker))),
    };
    let root = compose.snapshot.root().to_str().unwrap().to_owned();
    assert_eq!(
        compose.compose_command(&strings(&down)).unwrap().arguments,
        strings(&[
            "compose",
            "--project-name",
            "agentsandbox",
            "--env-file",
            &format!("{root}/compose-empty.env"),
            "--file",
            &format!("{root}/context/{COMPOSE_FILE}"),
            "down",
        ])
    );
    let up = compose
        .compose_command(&strings(&[
            "compose",
            "--project-name",
            "agentsandbox",
            "up",
            "--detach",
            "--force-recreate",
        ]))
        .unwrap();
    assert_eq!(
        up.arguments[7..],
        strings(&["up", "--detach", "--force-recreate"])
    );
    // Another valid stack name is carried to Compose as the project name, nothing else changes.
    let named = compose
        .compose_command(&strings(&["compose", "--project-name", "qa-other", "down"]))
        .unwrap();
    assert_eq!(
        named.arguments[..3],
        strings(&["compose", "--project-name", "qa-other"])
    );
    assert_eq!(
        named.arguments[3..],
        compose.compose_command(&strings(&down)).unwrap().arguments[3..]
    );
    for options in [
        &["compose"][..],
        &["compose", "--project-name", "agentsandbox"],
        &["compose", "--project-name", "Other", "down"],
        &["compose", "--project-name", "-other", "down"],
        &["compose", "--project-name", "oth er", "down"],
        &["compose", "--project-name", "../other", "down"],
        &["compose", "--project-name", "", "down"],
        &["build", "--project-name", "agentsandbox", "down"],
        &["compose", "--file", "/etc/compose.yaml", "down"],
        &["compose", "--project-name", "agentsandbox", "up"],
        &[
            "compose",
            "--project-name",
            "agentsandbox",
            "up",
            "--detach",
        ],
        &["compose", "--project-name", "agentsandbox", "restart"],
        &[
            "compose",
            "--project-name",
            "agentsandbox",
            "down",
            "--volumes",
        ],
        &[
            "compose",
            "--project-name",
            "agentsandbox",
            "--project-directory",
            "/etc",
            "down",
        ],
        &[
            "compose",
            "--project-name",
            "agentsandbox",
            "--file",
            "/etc/compose.yaml",
            "down",
        ],
    ] {
        assert!(
            compose.compose_command(&strings(options)).is_err(),
            "accepted Compose options {options:?}"
        );
    }
    let message = refusal(compose.build_command("images/control.Dockerfile", &strings(&tagged)));
    assert!(message.contains("holds no build inputs"), "{message}");
}

/// Stop requests a test sends, standing for the process's signals.
struct Scripted(tokio::sync::mpsc::UnboundedReceiver<i32>);

impl SignalSource for Scripted {
    async fn next(&mut self) -> i32 {
        match self.0.recv().await {
            Some(number) => number,
            None => std::future::pending().await,
        }
    }
}

struct DropWitness(Arc<AtomicBool>);

impl Drop for DropWitness {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn scope_returns_the_result_of_a_command_that_no_signal_interrupts() {
    assert!(!interrupt::active() && interrupt::current().is_none());
    let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    scope_with(Scripted(receiver), async {
        assert!(interrupt::active());
        let interruption = interrupt::current().expect("a scope is active");
        assert!(interruption.requested().is_none() && interruption.active() == 0);
        Ok(())
    })
    .await
    .expect("the command's own success");

    let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let failed = scope_with(Scripted(receiver), async {
        Err(anyhow!("the command's own failure"))
    })
    .await;
    assert_eq!(refusal(failed), "the command's own failure");
    assert!(!interrupt::active());
}

#[tokio::test]
async fn scope_drops_an_idle_command_on_a_signal_and_says_nothing_was_running() {
    let dropped = Arc::new(AtomicBool::new(false));
    let witness = DropWitness(Arc::clone(&dropped));
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    sender.send(libc::SIGINT).unwrap();
    let interrupted = scope_with(Scripted(receiver), async move {
        let _witness = witness;
        std::future::pending::<()>().await;
        Ok(())
    })
    .await;
    assert_eq!(
        refusal(interrupted),
        "interrupted by SIGINT: no engine build or Compose command was running"
    );
    assert!(
        dropped.load(Ordering::SeqCst),
        "the command and what it owned are dropped before the scope returns"
    );
}

#[tokio::test]
async fn scope_keeps_a_command_with_a_live_hold_and_returns_its_own_result() {
    let finished = Arc::new(AtomicBool::new(false));
    let done = Arc::clone(&finished);
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let result = scope_with(Scripted(receiver), async move {
        let interruption = interrupt::current().expect("a scope is active");
        let hold = interruption.hold();
        sender.send(libc::SIGTERM).expect("the scope listens");
        while interruption.requested().is_none() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(interruption.requested(), Some(libc::SIGTERM));
        // The scope knows the request. A command it dropped never gets here.
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(hold);
        done.store(true, Ordering::SeqCst);
        Err(anyhow!("the command's own result"))
    })
    .await;
    assert!(finished.load(Ordering::SeqCst));
    assert_eq!(refusal(result), "the command's own result");
}

#[tokio::test]
async fn scope_returns_the_error_of_a_command_whose_hold_is_kept_for_good() {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let result = scope_with(Scripted(receiver), async move {
        let interruption = interrupt::current().expect("a scope is active");
        let hold = interruption.hold();
        sender.send(libc::SIGHUP).expect("the scope listens");
        while interruption.requested().is_none() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        // An unknown termination never becomes idle.
        hold.keep();
        assert_eq!(interruption.active(), 1);
        Err(anyhow!("termination is unknown"))
    })
    .await;
    assert_eq!(refusal(result), "termination is unknown");
}

/// Stands for output a command wrote: no message may ever carry it.
const CAPTURED: &str = "CAPTURED-OUTPUT-CANARY";
const COPY: &str = "/private/tmp/ai-guard-distribution-synthetic";
const KEPT: &str = "The prepared copy is kept at /private/tmp/ai-guard-distribution-synthetic and is \
                    not removed by ai-guard; see docs/setup/local/README.md#prepared-source-retained";
const COLLECTED: &str = "and both of its output pipes reached end of file";

/// Wait status of a process that exited by itself with `code`.
fn exited(code: i32) -> ExitStatus {
    ExitStatus::from_raw(code << 8)
}

/// Wait status of a process that `signal` ended.
fn signalled(signal: i32) -> ExitStatus {
    ExitStatus::from_raw(signal)
}

fn collected(
    status: ExitStatus,
    stopped: Option<Stop>,
    input_changed: Option<anyhow::Error>,
) -> InvocationEnd {
    let capture = || Capture {
        bytes: CAPTURED.as_bytes().to_vec(),
        total_bytes: CAPTURED.len() as u64,
        truncated: false,
    };
    InvocationEnd::Collected(Collected {
        status,
        stdout: capture(),
        stderr: capture(),
        stopped,
        input_released: true,
        input_changed,
    })
}

fn unknown_end(
    stop: Stop,
    leader_reaped: bool,
    wait_uncertain: bool,
    open: [bool; 2],
) -> UnknownEnd {
    UnknownEnd {
        runtime: RuntimeKind::Docker,
        leader_pid: 4242,
        process_group: 4242,
        stop,
        leader_reaped,
        wait_uncertain,
        stdout_open: open[0],
        stderr_open: open[1],
        retained_at: None,
    }
}

/// The error the operator gets for `end`, a command bounded by `seconds`.
fn reported(subject: &str, seconds: u64, end: InvocationEnd) -> anyhow::Error {
    let kind = if subject.starts_with("podman") {
        RuntimeKind::Podman
    } else {
        RuntimeKind::Docker
    };
    let bound = Duration::from_secs(seconds);
    let error = match concluded(kind, subject, bound, Path::new(COPY), end) {
        Ok(_) => panic!("an end without a result was reported as a success"),
        Err(error) => error,
    };
    let everything = format!("{error:#}\n{error:?}");
    assert!(
        !everything.contains(CAPTURED),
        "captured output entered a message: {everything}"
    );
    error
}

fn message(subject: &str, seconds: u64, end: InvocationEnd) -> String {
    format!("{:#}", reported(subject, seconds, end))
}

#[test]
fn a_command_stopped_by_its_owner_is_reported_with_the_signals_sent_and_its_real_status() {
    let killed = signalled(libc::SIGKILL);
    let done = exited(0);
    let timed_out = Stop {
        reason: StopReason::TimedOut,
        forwarded: None,
        kill_sent: true,
    };

    // Killed at its bound: the status is the one SIGKILL left.
    let group = "the process group of the CLI process for docker compose";
    assert_eq!(
        message(
            "docker compose",
            180,
            collected(killed, Some(timed_out), None)
        ),
        format!(
            "docker compose exceeded its 180 s bound: SIGKILL was sent to {group}; that process \
             exited ({killed}) {COLLECTED}"
        )
    );
    // The CLI process had exited by itself and something kept its output
    // open until the bound: its own status is reported, and said to be so.
    assert_eq!(
        message(
            "docker compose",
            180,
            collected(done, Some(timed_out), None)
        ),
        format!(
            "docker compose exceeded its 180 s bound: SIGKILL was sent to {group}; that process \
             exited ({done}) {COLLECTED}. The CLI process had already exited when SIGKILL was \
             sent; the command was not collected earlier, usually because a process it started \
             kept an output pipe open"
        )
    );

    // An interruption that ends by SIGKILL: after the grace, on a repeated
    // signal or because the bound elapsed during the grace. The stop does not
    // record which, and the message claims none of them.
    let group = "the process group of the CLI process for podman build";
    let interrupted = Stop {
        reason: StopReason::Interrupted {
            signal: libc::SIGTERM,
        },
        forwarded: Some(libc::SIGTERM),
        kill_sent: true,
    };
    assert_eq!(
        message(
            "podman build",
            1800,
            collected(killed, Some(interrupted), None)
        ),
        format!(
            "interrupted by SIGTERM: SIGTERM was forwarded to {group}, then SIGKILL was sent; \
             that process exited ({killed}) {COLLECTED}"
        )
    );

    // A signal the process group did not accept is not reported as sent.
    let refused = Stop {
        reason: StopReason::Cancelled,
        forwarded: None,
        kill_sent: false,
    };
    assert_eq!(
        message("podman build", 1800, collected(done, Some(refused), None)),
        format!(
            "podman build was cancelled: no signal was accepted by {group}; that process exited \
             ({done}) {COLLECTED}"
        )
    );

    // A copy that also changed is added to the stop, never instead of it.
    let changed = anyhow!("prepared copy content changed: Cargo.toml");
    let text = message(
        "podman build",
        1800,
        collected(killed, Some(interrupted), Some(changed)),
    );
    assert!(
        text.starts_with("interrupted by SIGTERM: ")
            && text.ends_with(
                ". The prepared copy also changed while it was running (prepared copy content \
                 changed: Cargo.toml)"
            ),
        "{text}"
    );
}

#[test]
fn an_unknown_termination_states_only_what_was_sent_and_seen() {
    let unknown = |stop, leader_reaped, wait_uncertain, open| {
        message(
            "docker build",
            1800,
            InvocationEnd::Unknown(unknown_end(stop, leader_reaped, wait_uncertain, open)),
        )
    };
    let stop = |reason, forwarded, kill_sent| Stop {
        reason,
        forwarded,
        kill_sent,
    };
    let interrupted = StopReason::Interrupted {
        signal: libc::SIGINT,
    };

    // SIGKILL was accepted and the observation time passed.
    assert_eq!(
        unknown(
            stop(StopReason::TimedOut, None, true),
            false,
            false,
            [true, false]
        ),
        format!(
            "docker build termination is unknown: SIGKILL was sent to process group 4242 and, 2 s \
             later, the CLI process had not been reaped and stdout was still open. {KEPT}"
        )
    );
    // The wait status was taken by something else after the output closed:
    // nothing was requested, nothing was sent and no delay was observed.
    assert_eq!(
        unknown(
            stop(StopReason::WaitUnavailable, None, false),
            false,
            true,
            [false, false]
        ),
        format!(
            "docker build termination is unknown: no signal was sent to process group 4242 and \
             the wait status of the CLI process could not be obtained. {KEPT}"
        )
    );
    // The same during a stop: the group can no longer be named, so no signal.
    assert_eq!(
        unknown(
            stop(StopReason::Cancelled, None, false),
            false,
            true,
            [true, true]
        ),
        format!(
            "docker build termination is unknown: no signal was sent to process group 4242 and \
             the wait status of the CLI process could not be obtained and stdout was still open \
             and stderr was still open. {KEPT}"
        )
    );
    // The operator signal was forwarded, then the wait became uncertain.
    assert_eq!(
        unknown(
            stop(interrupted, Some(libc::SIGINT), false),
            false,
            true,
            [false, true]
        ),
        format!(
            "docker build termination is unknown: SIGINT was forwarded to process group 4242, \
             SIGKILL was not sent and the wait status of the CLI process could not be obtained \
             and stderr was still open. {KEPT}"
        )
    );

    // The place the input recorded wins over the root known to the caller.
    let mut end = unknown_end(
        stop(interrupted, Some(libc::SIGINT), true),
        true,
        false,
        [true, true],
    );
    end.retained_at = Some(PathBuf::from("/private/tmp/ai-guard-distribution-recorded"));
    let text = message("docker build", 1800, InvocationEnd::Unknown(end));
    assert!(
        text.starts_with(
            "docker build termination is unknown: SIGINT was forwarded to process group 4242, \
             then SIGKILL was sent and, 2 s later, stdout was still open and stderr was still \
             open. The prepared copy is kept at /private/tmp/ai-guard-distribution-recorded and"
        ),
        "{text}"
    );
}

#[test]
fn refused_aborted_and_self_ended_commands_keep_their_typed_cause() {
    let docker = RuntimeKind::Docker;
    let conclude = |end| {
        concluded(
            docker,
            "docker build",
            Duration::from_secs(1800),
            Path::new(COPY),
            end,
        )
    };

    // A refusal that is not one of the consumer's own reasons keeps its text.
    let unavailable =
        "docker is unavailable; install and start a container runtime. No local fallback exists";
    assert_eq!(
        message(
            "docker build",
            1800,
            InvocationEnd::Refused(anyhow!(unavailable))
        ),
        format!("docker build was not started: {unavailable}")
    );
    let error = reported(
        "docker build",
        1800,
        InvocationEnd::Refused(InvocationFailure::Capacity.into()),
    );
    assert_eq!(
        format!("{error:#}"),
        "docker build was not started: another retained engine CLI command is still owned by \
         this control process"
    );
    assert_eq!(
        error.downcast_ref::<InvocationFailure>(),
        Some(&InvocationFailure::Capacity)
    );
    let interrupted = InvocationFailure::Interrupted {
        signal: libc::SIGHUP,
    };
    assert_eq!(
        message(
            "docker build",
            1800,
            InvocationEnd::Refused(interrupted.into())
        ),
        "interrupted by SIGHUP: docker build was not started"
    );
    let changed = anyhow!("prepared copy content changed: Cargo.toml")
        .context(InvocationFailure::InputChanged);
    assert_eq!(
        message("docker build", 1800, InvocationEnd::Refused(changed)),
        "the prepared copy changed before docker build started (prepared copy content changed: \
         Cargo.toml); docker build was not started"
    );

    // An owner that ended without a report: the copy is said to stay only
    // when it was not released.
    let released = reported(
        "docker build",
        1800,
        InvocationEnd::Aborted {
            input_released: true,
        },
    );
    assert_eq!(
        format!("{released:#}"),
        "docker build ended without a collected result: the retained engine CLI owner is \
         unavailable"
    );
    assert_eq!(
        released.downcast_ref::<InvocationFailure>(),
        Some(&InvocationFailure::Unavailable)
    );
    assert_eq!(
        message(
            "docker build",
            1800,
            InvocationEnd::Aborted {
                input_released: false
            }
        ),
        format!(
            "docker build ended without a collected result. {KEPT}: the retained engine CLI owner \
             is unavailable"
        )
    );

    // A command that ended by itself: its output on success, the existing
    // typed failure otherwise, and no result when its copy changed meanwhile.
    let output = conclude(collected(exited(0), None, None)).expect("a self-ended success");
    assert_eq!(output.bytes, CAPTURED.as_bytes());
    let failed = reported("docker build", 1800, collected(exited(7), None, None));
    assert!(failed.downcast_ref::<RuntimeCommandFailure>().is_some());
    assert_eq!(
        format!("{failed:#}"),
        format!("docker command failed ({})", exited(7))
    );
    let changed = anyhow!("prepared copy content changed: Cargo.toml");
    assert_eq!(
        message(
            "docker build",
            1800,
            collected(exited(0), None, Some(changed))
        ),
        format!(
            "the prepared copy changed while docker build was running (prepared copy content \
             changed: Cargo.toml); the CLI process exited ({}) but its result is not bound to \
             the reported digest",
            exited(0)
        )
    );
}

#[test]
fn a_same_length_rewrite_between_capture_and_confirmation_is_refused_by_its_timestamps() {
    use std::io::Write as _;
    let fixture = Fixture::new();
    let target = fixture.source.join("Cargo.toml");
    let before = fs::metadata(&target).unwrap();
    let captured = fixture.capture(Scope::Build, Limits::default()).unwrap();

    // The same file, rewritten in place with as many bytes: of its record only
    // the timestamps can differ.
    let rewritten = b"rewritten after it was read\n";
    assert_eq!(rewritten.len() as u64, before.len());
    let mut file = fs::OpenOptions::new().write(true).open(&target).unwrap();
    file.write_all(rewritten).unwrap();
    // Set explicitly, so the refusal does not depend on how fine the clock of
    // the filesystem is.
    file.set_modified(before.modified().unwrap() + Duration::from_secs(10))
        .unwrap();
    drop(file);
    let after = fs::metadata(&target).unwrap();
    assert_eq!(
        (after.dev(), after.ino(), after.len(), after.mode()),
        (before.dev(), before.ino(), before.len(), before.mode())
    );
    assert_eq!(fs::read(&target).unwrap(), rewritten);

    assert_eq!(
        refusal(captured.confirm(None)),
        "source changed while it was being prepared: Cargo.toml"
    );
    assert!(fixture.leftovers().is_empty());
}

#[test]
fn a_source_directory_replaced_by_an_identical_tree_is_refused_at_confirmation() {
    let fixture = Fixture::new();
    let captured = fixture.capture(Scope::Build, Limits::default()).unwrap();
    // The directory that was read is moved aside, untouched, and a tree with
    // the same names takes its path: the selected set still matches and every
    // record read through the held directory still is what was captured.
    let aside = fixture.source.with_file_name("read-and-moved-aside");
    fs::rename(&fixture.source, &aside).unwrap();
    fixture.populate();
    assert_eq!(tree(&fixture.source), tree(&aside));

    assert_eq!(
        refusal(captured.confirm(None)),
        format!(
            "source directory changed while it was being prepared: {}",
            fixture.source.display()
        )
    );
    assert!(fixture.leftovers().is_empty());
}

#[test]
fn a_source_reached_through_a_symbolic_link_is_refused_and_nothing_is_created() {
    let fixture = Fixture::new();
    let base = fixture.source.parent().unwrap();
    // The last component is a link.
    let alias = base.join("alias");
    symlink(&fixture.source, &alias).unwrap();
    // A link among the ancestors.
    let through = base.join("through");
    symlink(base, &through).unwrap();

    for linked in [alias, through.join("project")] {
        assert!(linked.join("Cargo.toml").is_file(), "the link resolves");
        let refused = headline(capture(
            &linked,
            Scope::Build,
            Limits::default(),
            &fixture.temporary,
            None,
        ));
        assert_eq!(
            refused,
            format!(
                "source directory is unavailable or reached through a symbolic link: {}",
                linked.display()
            )
        );
        assert_eq!(headline(SourceRoot::open(&linked)), refused);
        assert!(fixture.leftovers().is_empty());
    }
}

#[test]
fn a_selected_name_with_a_control_character_or_invalid_utf8_is_refused() {
    let control = Fixture::new();
    control.write(
        "src/bad\nname.rs",
        "a selected name that holds a line feed\n",
    );
    assert_eq!(
        refusal(control.capture(Scope::Build, Limits::default())),
        "invalid source path: \"src/bad\\nname.rs\""
    );
    assert!(control.leftovers().is_empty());

    // The same name outside the selected paths is not an input.
    let unrelated = Fixture::new();
    unrelated.write("notes/bad\nname.rs", "not a build input\n");
    unrelated.prepare(Scope::Build);

    // APFS refuses such a name when it is created.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let invalid = Fixture::new();
        let name = std::ffi::OsStr::from_bytes(b"bad\xffname.rs");
        fs::write(invalid.source.join("src").join(name), "not UTF-8\n").unwrap();
        assert_eq!(
            refusal(invalid.capture(Scope::Build, Limits::default())),
            "source path must be UTF-8: src/bad\u{fffd}name.rs"
        );
        assert!(invalid.leftovers().is_empty());
    }
}

#[test]
fn a_copy_whose_root_was_replaced_fails_its_verification() {
    let fixture = Fixture::new();
    let snapshot = fixture.prepare(Scope::Build);
    snapshot.verify().expect("an untouched copy verifies");
    let root = snapshot.root().to_owned();
    // The copy is moved aside, whole, and a private directory takes its path.
    let aside = fixture.temporary.join("copy-moved-aside");
    fs::rename(&root, &aside).unwrap();
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();

    assert_eq!(
        refusal(snapshot.verify()),
        "control directory path identity changed; reopen the intended private configuration"
    );
    assert!(aside.join("context/Cargo.toml").is_file());
}

#[test]
fn a_build_source_whose_ignore_files_differ_from_the_compiled_list_is_refused() {
    let digest = |bytes: &[u8]| hex::encode(Sha256::digest(bytes));
    let differs = |bytes: &[u8]| {
        format!(
            "the build input list of this CLI mirrors a .dockerignore whose SHA-256 is {}; the \
             .dockerignore of the selected distribution has SHA-256 {}. Build with the CLI built \
             from that distribution, or run the engine build directly",
            digest(DOCKERIGNORE),
            digest(bytes)
        )
    };

    // One rule more than the shipped file: the engine would admit `extra/`,
    // which the list compiled into this CLI does not copy.
    let extended = Fixture::new();
    let mut rules = DOCKERIGNORE.to_vec();
    rules.extend_from_slice(b"!extra/\n");
    fs::write(extended.source.join(".dockerignore"), &rules).unwrap();
    extended.write("extra/ca.pem", "admitted by this distribution only\n");
    let original = tree(&extended.source);
    assert_eq!(
        refusal(extended.capture(Scope::Build, Limits::default())),
        differs(&rules)
    );
    assert!(extended.leftovers().is_empty());
    assert_eq!(tree(&extended.source), original);
    // The Compose scope hands no ignore file to the engine.
    extended.prepare(Scope::Compose(RuntimeKind::Docker));

    // Other line endings are other bytes.
    let converted = Fixture::new();
    let rules: Vec<u8> = DOCKERIGNORE
        .iter()
        .flat_map(|byte| match byte {
            b'\n' => vec![b'\r', b'\n'],
            other => vec![*other],
        })
        .collect();
    fs::write(converted.source.join(".dockerignore"), &rules).unwrap();
    assert_eq!(
        refusal(converted.capture(Scope::Build, Limits::default())),
        differs(&rules)
    );
    assert!(converted.leftovers().is_empty());

    // A per-Dockerfile ignore file: a builder may apply it instead of the
    // file the list mirrors.
    let per_dockerfile = Fixture::new();
    per_dockerfile.write("images/control.Dockerfile.dockerignore", "**\n");
    let original = tree(&per_dockerfile.source);
    assert_eq!(
        refusal(per_dockerfile.capture(Scope::Build, Limits::default())),
        "the build input list of this CLI mirrors .dockerignore only: the per-Dockerfile ignore \
         file images/control.Dockerfile.dockerignore of the selected distribution is not \
         reconciled with it, and a builder may apply that file instead. Remove it, or run the \
         engine build directly"
    );
    assert!(per_dockerfile.leftovers().is_empty());
    assert_eq!(tree(&per_dockerfile.source), original);
}

#[test]
fn absolute_source_and_temporary_roots_are_escaped_only_when_rendered() {
    let mut fixture = Fixture::new();
    let parent = fixture.source.parent().expect("source parent").to_owned();
    let source = parent.join("project\n\r\x1broot\\literal");
    let temporary = parent.join("tmp\n\r\x1broot\\literal");
    fs::rename(&fixture.source, &source).expect("rename only the synthetic source");
    fs::rename(&fixture.temporary, &temporary)
        .expect("rename only the private temporary directory");
    fixture.source = source;
    fixture.temporary = temporary;
    let original = tree(&fixture.source);

    // Exercise capture and verification through the real directory names.
    // Rendering must not rewrite the operational path or the manifest binding.
    let snapshot = fixture.prepare(Scope::Build);
    snapshot
        .verify()
        .expect("the copy remains bound to its bytes");
    assert!(snapshot.context().starts_with(&fixture.temporary));
    assert_eq!(
        fs::read(snapshot.context().join("Cargo.toml")).expect("copied input"),
        fs::read(fixture.source.join("Cargo.toml")).expect("original input")
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(snapshot.root().join("manifest.json")).unwrap()).unwrap();
    assert_eq!(
        manifest["source"].as_str(),
        fixture.source.to_str(),
        "structured provenance keeps the actual source name"
    );
    assert_eq!(tree(&fixture.source), original);
    eprintln!("R1_ABSOLUTE_ROOT_CAPTURE_CONFIRMED");

    let provenance = snapshot.provenance_line();
    assert!(
        !provenance.chars().any(char::is_control),
        "absolute source controls must be escaped in provenance: {provenance:?}"
    );
    assert!(
        provenance.ends_with("/project\\n\\r\\u{1b}root\\\\literal"),
        "escaped source name must be unambiguous: {provenance:?}"
    );

    let retained = super::retained(snapshot.root());
    assert!(
        !retained.chars().any(char::is_control),
        "temporary-root controls must be escaped in the recovery message: {retained:?}"
    );
    assert!(
        retained.contains("/tmp\\n\\r\\u{1b}root\\\\literal/ai-guard-distribution-"),
        "escaped temporary name must remain recognizable: {retained:?}"
    );

    let missing = fixture.source.join("missing\n\r\x1broot\\literal");
    let unavailable = headline(SourceRoot::open(&missing));
    assert!(
        !unavailable.chars().any(char::is_control),
        "absolute source controls must be escaped in a refusal: {unavailable:?}"
    );
    assert!(
        unavailable
            .ends_with("/project\\n\\r\\u{1b}root\\\\literal/missing\\n\\r\\u{1b}root\\\\literal"),
        "escaped unavailable path must be unambiguous: {unavailable:?}"
    );

    drop(snapshot);
    assert!(fixture.leftovers().is_empty());
    assert_eq!(tree(&fixture.source), original);
}

#[test]
fn a_refusal_never_prints_a_control_character_of_a_source_name() {
    let printable = |message: &str| {
        assert!(!message.chars().any(char::is_control), "{message:?}");
    };

    // Any link below `src/` is refused, whatever its name: the name is
    // printed before any rule on names applies.
    let linked = Fixture::new();
    symlink(
        "main.rs",
        linked
            .source
            .join("src/a\x1b[2J\nPrepared public distribution (build): sha256=0"),
    )
    .unwrap();
    let message = refusal(linked.capture(Scope::Build, Limits::default()));
    assert_eq!(
        message,
        "source input must not be a symbolic link: src/a\\u{1b}[2J\\nPrepared public \
         distribution (build): sha256=0"
    );
    printable(&message);
    assert!(linked.leftovers().is_empty());

    // One component beyond the depth bound, with a carriage return in the
    // name of the file itself: every directory above it is accepted.
    let deep = Fixture::new();
    let nested = "d/".repeat(MAX_DEPTH - 1);
    deep.write(&format!("src/{nested}x\rdeep.rs"), "deep\n");
    let message = refusal(deep.capture(Scope::Build, Limits::default()));
    assert!(
        message.starts_with(&format!(
            "source path exceeds the bound of {MAX_DEPTH} components or 4096 bytes: src/d/d/"
        )) && message.ends_with("/d/x\\rdeep.rs"),
        "{message:?}"
    );
    printable(&message);
    assert!(deep.leftovers().is_empty());

    // A backslash of a name is doubled, so an escape cannot be forged.
    let literal = Fixture::new();
    symlink("main.rs", literal.source.join("src/a\\nb")).unwrap();
    assert_eq!(
        refusal(literal.capture(Scope::Build, Limits::default())),
        "source input must not be a symbolic link: src/a\\\\nb"
    );

    // A name longer than a message shows is cut, and the cut is marked.
    let long = Fixture::new();
    let directory = long
        .source
        .join("src")
        .join(format!("{}/", "n".repeat(200)).repeat(6));
    fs::create_dir_all(&directory).unwrap();
    symlink("main.rs", directory.join("link")).unwrap();
    let message = refusal(long.capture(Scope::Build, Limits::default()));
    let shown = message
        .strip_prefix("source input must not be a symbolic link: src/nnn")
        .unwrap_or_else(|| panic!("{message:?}"));
    assert!(shown.ends_with("nnn…"), "{message:?}");
    assert_eq!(shown.chars().count() + "src/nnn".len(), 1025, "{message:?}");

    // APFS refuses such a name when it is created.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let invalid = Fixture::new();
        let name = std::ffi::OsStr::from_bytes(b"a\xff\x1b[0m.rs");
        fs::write(invalid.source.join("src").join(name), "not UTF-8\n").unwrap();
        let message = refusal(invalid.capture(Scope::Build, Limits::default()));
        assert_eq!(
            message,
            "source path must be UTF-8: src/a\u{fffd}\\u{1b}[0m.rs"
        );
        printable(&message);
        assert!(invalid.leftovers().is_empty());
    }
}

/// Gives a directory its mode back when the test ends, so the test's own tree
/// can be removed.
#[cfg(target_os = "linux")]
struct Restored(PathBuf, u32);

#[cfg(target_os = "linux")]
impl Drop for Restored {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(self.1));
    }
}

// Elsewhere every directory down to the source must also be readable.
#[cfg(target_os = "linux")]
#[test]
fn a_source_below_a_directory_that_can_only_be_traversed_is_prepared() {
    let mut fixture = Fixture::new();
    let gate = fixture.source.with_file_name("gate");
    fs::create_dir(&gate).unwrap();
    fs::rename(&fixture.source, gate.join("project")).unwrap();
    fixture.source = gate.join("project");
    // Search and write without read: this account goes through the directory
    // but cannot list it.
    fs::set_permissions(&gate, fs::Permissions::from_mode(0o311)).unwrap();
    let _restored = Restored(gate.clone(), 0o700);
    if fs::read_dir(&gate).is_ok() {
        eprintln!(
            "NOT_RUN: this account lists a directory it has no read permission on (privileged \
             process); a search-only ancestor cannot be produced here"
        );
        return;
    }
    assert!(fixture.source.join("Cargo.toml").is_file());

    let snapshot = fixture.prepare(Scope::Build);
    assert!(snapshot.holds("Cargo.toml") && snapshot.holds("src/x/secrets/extra.rs"));
    assert!(
        snapshot
            .provenance_line()
            .ends_with(&format!(" source={}", fixture.source.display()))
    );
    drop(snapshot);
    assert!(fixture.leftovers().is_empty());
    eprintln!("executed: the source was prepared below a directory of mode 0311");
}

#[test]
fn a_temporary_directory_that_is_relative_or_in_the_source_is_refused_before_any_creation() {
    let fixture = Fixture::new();
    for inside in ["scratch", "src/scratch"] {
        let directory = fixture.source.join(inside);
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    }
    // A link outside the source that leads into it.
    let linked = fixture.source.with_file_name("linked-tmp");
    symlink(fixture.source.join("scratch"), &linked).unwrap();
    let original = tree(&fixture.source);
    let attempt = |temporary: &Path| {
        let refused = refusal(capture(
            &fixture.source,
            Scope::Build,
            Limits::default(),
            temporary,
            None,
        ));
        assert_eq!(tree(&fixture.source), original, "{}", temporary.display());
        assert!(fixture.leftovers().is_empty(), "{}", temporary.display());
        refused
    };

    // An empty or relative value would be resolved against the working
    // directory, which is the source itself in the documented commands.
    for relative in ["", "relative-tmp", "./relative-tmp"] {
        assert_eq!(
            attempt(Path::new(relative)),
            format!(
                "the temporary directory must be an absolute path, but TMPDIR is \"{relative}\"; \
                 set TMPDIR to an absolute directory outside the selected source"
            )
        );
    }

    // The source itself, a directory the capture does not enter and one it
    // does enter.
    for temporary in [
        fixture.source.clone(),
        fixture.source.join("scratch"),
        fixture.source.join("src/scratch"),
    ] {
        assert_eq!(
            attempt(&temporary),
            format!(
                "the temporary directory {} (TMPDIR) is the selected source {} or lies inside \
                 it; set TMPDIR to a directory outside the source",
                temporary.display(),
                fixture.source.display()
            )
        );
    }

    // Reached through a link: refused as any linked temporary directory is,
    // and nothing was created behind the link.
    let message = attempt(&linked);
    assert!(
        message.starts_with(&format!(
            "cannot create the private prepared-source directory under {} (TMPDIR). Set TMPDIR to",
            linked.display()
        )),
        "{message}"
    );

    // A directory beside the source is where the copy goes.
    fixture.prepare(Scope::Build);
}

#[test]
fn a_copy_is_removed_only_while_its_path_still_names_the_directory_that_was_prepared() {
    const PLACED: &[u8] = b"placed by the test\n";
    let fixture = Fixture::new();

    // The copy is moved aside and another private directory takes its path.
    let replaced = fixture.prepare(Scope::Build);
    let root = replaced.root().to_owned();
    let aside = fixture.temporary.join("copy-moved-aside");
    fs::rename(&root, &aside).unwrap();
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let sentinel = root.join("not-the-copy.txt");
    fs::write(&sentinel, PLACED).unwrap();
    let moved = tree(&aside);
    drop(replaced);
    assert_eq!(
        fs::read(&sentinel).ok().as_deref(),
        Some(PLACED),
        "the directory that took the path of the copy was removed"
    );
    // The copy is whole where it was moved: nothing was removed by either name.
    assert_eq!(tree(&aside), moved);
    let mut expected = vec![
        "copy-moved-aside".to_owned(),
        root.file_name().unwrap().to_string_lossy().into_owned(),
    ];
    expected.sort();
    assert_eq!(fixture.leftovers(), expected);
    fs::remove_dir_all(&root).unwrap();
    fs::remove_dir_all(&aside).unwrap();

    // The copy is moved away and nothing takes its path.
    let moved_away = fixture.prepare(Scope::Build);
    let aside = fixture.temporary.join("copy-moved-away");
    fs::rename(moved_away.root(), &aside).unwrap();
    let moved = tree(&aside);
    drop(moved_away);
    assert_eq!(tree(&aside), moved);
    assert_eq!(fixture.leftovers(), ["copy-moved-away"]);
    fs::remove_dir_all(&aside).unwrap();

    // A copy that was not touched is removed, as before.
    drop(fixture.prepare(Scope::Build));
    assert!(fixture.leftovers().is_empty());
}

#[test]
fn the_descriptor_limit_is_named_whichever_open_met_it() {
    use super::capture::descriptor_limit;
    use crate::control_fs::{Directory, PrivateFile};
    const NAMED: &str = "the open-file limit of this process or of the system was reached while \
        the source was being prepared; raise the descriptor limit (`ulimit -n`) and run the \
        command again";
    let system = |code: i32| std::io::Error::from_raw_os_error(code);
    let named = |error: anyhow::Error| descriptor_limit(error).to_string() == NAMED;
    let input = "source input is unavailable or is a symbolic link: src/lib.rs";

    // An open that carries the system error as its cause.
    for code in [libc::EMFILE, libc::ENFILE] {
        assert!(named(anyhow::Error::new(system(code)).context(input)));
    }
    // Another system error, and a refusal whose own text only ends like the
    // error of the limit, keep their first message.
    assert!(!named(
        anyhow::Error::new(system(libc::EACCES)).context(input)
    ));
    assert!(!named(anyhow!(
        "source input must not be a symbolic link: src/{}",
        system(libc::EMFILE)
    )));

    // The private-directory primitives print the system error of a refused
    // open at the end of their own message. Their real text is taken here,
    // with the error of the limit in the place of the one produced.
    let fixture = Fixture::new();
    let directory = Directory::open_existing(&fixture.temporary).unwrap();
    let name = std::ffi::OsStr::new("taken");
    drop(directory.create_private_entry(name).unwrap());
    let created = refusal(directory.create_private_entry(name));
    let opened = refusal(PrivateFile::open(&fixture.temporary.join("missing"), 16));
    for (message, produced) in [(created, libc::EEXIST), (opened, libc::ENOENT)] {
        let produced = system(produced).to_string();
        assert!(message.ends_with(&produced), "{message}");
        assert!(!named(anyhow!(message.clone())), "{message}");
        let at_the_limit = message.replace(&produced, &system(libc::EMFILE).to_string());
        assert!(
            named(anyhow!(at_the_limit).context("cannot write the prepared copy of src/lib.rs")),
            "{message}"
        );
    }
}
