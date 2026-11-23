//! Shared helpers of the source-lifecycle targets: fixture engine, oracles and
//! failure classification. Included by `tests/source_lifecycle.rs` and by
//! `tests/retained_cli_lifecycle.rs`, so each target uses only a part of it.
//!
//! Two affirmative oracles are used, never a guess:
//!
//! - **alive**: the fixture process answers a probe it was asked after the fact;
//! - **ended**: the test acquires the exclusive lock that the process held on
//!   its own `alive-<role>` file. The kernel releases it only when the process
//!   is gone.
//!
//! Every panic message of the two targets starts with one prefix:
//! `DEFECT:` (planned assertion showing a defect), `CONTRACT:` (the assertion
//! only states the new contract), `FAIL:` (unplanned product failure),
//! `SETUP:` (fixture or environment did not reach the scenario) or `UNKNOWN:`
//! (the recorded facts do not allow a class).
#![allow(dead_code)]

use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions, TryLockError},
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        OnceLock,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

pub const POLL: Duration = Duration::from_millis(10);
pub const ROLES: [&str; 2] = ["leader", "descendant"];
/// Written to standard error by a fixture `burst`; never expected in a message.
pub const STDERR_CANARY: &str = "FIXTURE-STDERR-CANARY";
/// The last thing a consuming fixture command writes before it exits by itself.
pub const ENGINE_STDOUT: &str = "fixture\n";
/// Name prefix of a prepared source under the temporary directory.
pub const PREPARED_PREFIX: &str = "ai-guard-distribution-";

const FIXTURE_SOURCE: &str = include_str!("../runtime_fixtures/source_consumer.rs");

/// Compiles the fixture engine once per test binary, with the compiler Cargo
/// itself uses. The result is copied, not linked, into each test's directory.
pub fn fixture_binary() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let directory = Path::new(env!("CARGO_TARGET_TMPDIR"));
        fs::create_dir_all(directory).expect("SETUP: test scratch directory");
        let stem = format!("{}-source-consumer", env!("CARGO_CRATE_NAME"));
        let source = directory.join(format!("{stem}.rs"));
        let staged = directory.join(format!("{stem}.{}.tmp", std::process::id()));
        let binary = directory.join(&stem);
        fs::write(&source, FIXTURE_SOURCE).expect("SETUP: fixture source");
        let output = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args(["--edition=2024", "--crate-name", "source_consumer_fixture"])
            .arg(&source)
            .arg("-o")
            .arg(&staged)
            .output()
            .expect("SETUP: the Rust compiler is unavailable at test time");
        assert!(
            output.status.success(),
            "SETUP: the fixture engine does not compile: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::rename(&staged, &binary).expect("SETUP: fixture binary");
        binary
    })
}

/// Installs the fixture as an engine CLI named `name` in `control`.
pub fn install_engine(control: &Path, name: &str) -> PathBuf {
    let program = control.join(name);
    fs::copy(fixture_binary(), &program).expect("SETUP: fixture engine copy");
    program
}

pub fn private_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("SETUP: private test directory")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Liveness {
    /// The role never took its lock: no such process was started.
    NeverStarted,
    /// The lock is held: the process exists.
    Alive,
    /// The lock was acquired by the test: the process is gone.
    Ended,
}

pub fn liveness_at(state: &Path, role: &str) -> Liveness {
    let Ok(file) = OpenOptions::new()
        .read(true)
        .write(true)
        .open(state.join(format!("alive-{role}")))
    else {
        return Liveness::NeverStarted;
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            Liveness::Ended
        }
        Err(TryLockError::WouldBlock) => Liveness::Alive,
        Err(TryLockError::Error(error)) => {
            panic!("SETUP: the liveness lock of {role} cannot be tried: {error}")
        }
    }
}

/// One engine CLI invocation recorded by the fixture.
#[derive(Clone, Debug)]
pub struct Invocation {
    pub number: u64,
    pub verb: String,
    pub pid: u32,
    pub started: SystemTime,
    pub arguments: Vec<String>,
    observed: PathBuf,
}

impl Invocation {
    /// Arguments without the engine-global `--host <endpoint>` option.
    pub fn engine_arguments(&self) -> &[String] {
        match self.arguments.first().map(String::as_str) {
            Some("--host") => self.arguments.get(2..).unwrap_or_default(),
            _ => &self.arguments,
        }
    }
    pub fn value(&self, flag: &str) -> Option<&str> {
        self.arguments
            .windows(2)
            .find(|pair| pair[0] == flag)
            .map(|pair| pair[1].as_str())
    }
    /// Bytes the fixture copied under this name, exactly as it read them.
    pub fn observed(&self, name: &str) -> Option<Vec<u8>> {
        fs::read(self.observed.join(name)).ok()
    }
    /// The Compose interpolation variables observed by the engine fixture.
    pub fn compose_environment(&self) -> Vec<String> {
        let text = fs::read_to_string(self.observed.join("environment")).unwrap_or_default();
        text.lines().map(str::to_owned).collect()
    }
    /// Every file the build consumer read in its context: relative path, bytes.
    pub fn context_files(&self) -> BTreeMap<String, Vec<u8>> {
        let mut files = BTreeMap::new();
        collect_files(&self.observed.join("context"), Path::new(""), &mut files);
        files
    }
    /// `(kind, mode, relative path)` of each context entry; `root` and `parent`
    /// describe the context directory and the directory that contains it.
    pub fn entries(&self) -> Vec<(String, u32, String)> {
        let text = fs::read_to_string(self.observed.join("entries")).unwrap_or_default();
        text.lines()
            .filter_map(|line| {
                let mut parts = line.splitn(3, ' ');
                let kind = parts.next()?.to_owned();
                let mode = u32::from_str_radix(parts.next()?, 8).ok()?;
                Some((kind, mode, parts.next()?.to_owned()))
            })
            .collect()
    }
}

fn collect_files(root: &Path, relative: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
    let Ok(entries) = fs::read_dir(root.join(relative)) else {
        return;
    };
    for entry in entries.flatten() {
        let relative = relative.join(entry.file_name());
        let path = root.join(&relative);
        if path.is_dir() {
            collect_files(root, &relative, files);
        } else if let Ok(bytes) = fs::read(&path) {
            files.insert(relative.to_string_lossy().into_owned(), bytes);
        }
    }
}

/// The fixture's state directory, `<program>.fixture/`.
pub struct FixtureState {
    root: PathBuf,
    probes: AtomicU32,
}

impl FixtureState {
    pub fn for_program(program: &Path) -> Self {
        let root = program.with_extension("fixture");
        fs::create_dir_all(&root).expect("SETUP: fixture state directory");
        Self {
            root,
            probes: AtomicU32::new(0),
        }
    }
    pub fn path(&self) -> &Path {
        &self.root
    }
    pub fn plan(&self, directives: &[(&str, &str)]) {
        let text: String = directives
            .iter()
            .map(|(key, value)| format!("{key}={value}\n"))
            .collect();
        fs::write(self.root.join("plan"), text).expect("SETUP: fixture plan");
    }
    pub fn exists(&self, name: &str) -> bool {
        self.root.join(name).exists()
    }
    pub fn read(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.root.join(name)).ok()
    }
    /// Names of the state files that start with `prefix`.
    pub fn names(&self, prefix: &str) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(prefix))
            .collect();
        names.sort();
        names
    }
    /// Time until the fixture published `name`, or `None` after `window`.
    pub fn wait_for(&self, name: &str, window: Duration) -> Option<Duration> {
        let started = Instant::now();
        loop {
            if self.exists(name) {
                return Some(started.elapsed());
            }
            if started.elapsed() >= window {
                return None;
            }
            std::thread::sleep(POLL);
        }
    }
    /// `ready-<role>` is written by the process itself, after it took its lock
    /// and really read its first input.
    pub fn wait_ready(&self, role: &str, window: Duration) -> Option<Duration> {
        self.wait_for(&format!("ready-{role}"), window)
    }
    /// Asks a new question and reports whether `role` answered it in time.
    pub fn probe(&self, role: &str, window: Duration) -> bool {
        let question = self.probes.fetch_add(1, Ordering::SeqCst) + 1;
        fs::write(self.root.join(format!("probe-{question}")), b"").expect("SETUP: fixture probe");
        self.wait_for(&format!("ack-{role}-{question}"), window)
            .is_some()
    }
    pub fn liveness(&self, role: &str) -> Liveness {
        liveness_at(&self.root, role)
    }
    /// True once the test acquired the lock of a role that had started.
    pub fn ended(&self, role: &str, window: Duration) -> bool {
        let started = Instant::now();
        loop {
            if self.liveness(role) == Liveness::Ended {
                return true;
            }
            if started.elapsed() >= window {
                return false;
            }
            std::thread::sleep(POLL);
        }
    }
    pub fn invocations(&self) -> Vec<Invocation> {
        let mut invocations = Vec::new();
        for number in 1.. {
            let record = self.root.join(format!("started-{number}"));
            let Ok(text) = fs::read_to_string(&record) else {
                break;
            };
            let mut parts = text.split_whitespace();
            let observed = self.root.join("observed").join(number.to_string());
            invocations.push(Invocation {
                number,
                verb: parts.next().unwrap_or_default().to_owned(),
                pid: parts.next().and_then(|pid| pid.parse().ok()).unwrap_or(0),
                started: fs::metadata(&record)
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH),
                arguments: fs::read_to_string(observed.join("arguments"))
                    .ok()
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or_default(),
                observed,
            });
        }
        invocations
    }
    pub fn consumers(&self, verb: &str) -> Vec<Invocation> {
        self.invocations()
            .into_iter()
            .filter(|invocation| invocation.verb == verb)
            .collect()
    }
    pub fn calls(&self) -> String {
        self.read("calls").unwrap_or_default()
    }
    /// Number of `effect <target> <verb>` lines: commands that reached the engine.
    pub fn effects(&self, verb: &str) -> usize {
        self.calls()
            .lines()
            .filter(|line| line.starts_with("effect ") && line.ends_with(&format!(" {verb}")))
            .count()
    }
    pub fn release(&self) {
        let _ = fs::write(self.root.join("release"), b"");
    }
    pub fn quit(&self) {
        let _ = fs::write(self.root.join("quit"), b"");
    }
    /// One line per role for a failure message.
    pub fn describe(&self) -> String {
        let roles: Vec<String> = ROLES
            .iter()
            .map(|role| {
                format!(
                    "{role}: ready={} lock={:?}",
                    self.exists(&format!("ready-{role}")),
                    self.liveness(role)
                )
            })
            .collect();
        let setup = self
            .read("setup-failed")
            .map(|detail| format!("; fixture setup failure: {detail}"))
            .unwrap_or_default();
        format!("{}{setup}", roles.join("; "))
    }
}

impl Drop for FixtureState {
    /// Also runs on panic: no fixture process outlives its test for long.
    fn drop(&mut self) {
        self.release();
        self.quit();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(3)
            && ROLES
                .iter()
                .any(|role| liveness_at(&self.root, role) == Liveness::Alive)
        {
            std::thread::sleep(POLL);
        }
    }
}

/// What the product had done when an observation window was missed.
pub enum Product {
    Pending,
    Ended(String),
}

/// Classifies a missing `ready-<role>` from recorded facts.
pub fn missed_ready(
    state: &FixtureState,
    role: &str,
    verb: &str,
    spawned: SystemTime,
    waited: Duration,
    product: &Product,
) -> String {
    if let Product::Ended(result) = product {
        return format!("FAIL: the product ended before ready-{role} was published: {result}");
    }
    let started = state
        .invocations()
        .into_iter()
        .find(|invocation| invocation.verb == verb);
    match started {
        Some(invocation) => format!(
            "SETUP: {verb} (invocation {}) started {:?} after the product was spawned, but \
             ready-{role} was not published within {waited:?}; {}",
            invocation.number,
            invocation
                .started
                .duration_since(spawned)
                .unwrap_or_default(),
            state.describe()
        ),
        None => format!(
            "UNKNOWN: before spawn: no {verb} invocation within {waited:?} while the product is \
             still pending; calls: {:?}; {}",
            state.calls(),
            state.describe()
        ),
    }
}

pub fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

pub fn signal_name(signal: i32) -> &'static str {
    match signal {
        libc::SIGHUP => "SIGHUP",
        libc::SIGINT => "SIGINT",
        libc::SIGTERM => "SIGTERM",
        _ => "an unexpected signal",
    }
}

/// `N` of a canary written as `live-version N`.
pub fn live_version(bytes: &[u8]) -> Option<u64> {
    std::str::from_utf8(bytes)
        .ok()?
        .trim()
        .strip_prefix("live-version ")?
        .parse()
        .ok()
}

/// Files `preflight_build_inputs` requires in a build source.
pub const REQUIRED: [&str; 19] = [
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ".dockerignore",
    "LICENSE",
    "images/control.Dockerfile",
    "images/worker.Dockerfile",
    "src/main.rs",
    "src/lib.rs",
    "src/bin/ai-guard-provider.rs",
    "front/package.json",
    "front/package-lock.json",
    "front/tsconfig.json",
    "front/vite.config.ts",
    "front/index.html",
    "front/src/main.ts",
    "front/src/style.css",
    "front/public/favicon.svg",
    "policies/default.json",
];
/// Further files the build allowlist (`.dockerignore`) admits.
pub const ADMITTED: [&str; 6] = [
    "src/bin/ai-guard-relay.rs",
    "src/nested/secrets/extra.rs",
    "front/src/views/extra.ts",
    "front/src/theme/extra.css",
    "policies/extra.json",
    "images/extra.Dockerfile",
];
/// Files of the synthetic source that are not build inputs.
pub const NOT_BUILD_INPUTS: [&str; 12] = [
    "private-notes.txt",
    "src/data.bin",
    "src/nested/notes.md",
    "front/extra.json",
    "front/src/notes.md",
    "front/public/other.svg",
    "policies/nested/deep.json",
    "images/notes.md",
    "target/debug/junk.rs",
    "tests/sample.rs",
    "docs/guide.md",
    COMPOSE_FILE,
];
pub const COMPOSE_FILE: &str = "deploy/docker/compose.yaml";

/// The build inputs of the synthetic source: required and admitted files.
pub fn build_inputs() -> Vec<&'static str> {
    REQUIRED.iter().chain(ADMITTED.iter()).copied().collect()
}

/// A small distribution with synthetic content. `.dockerignore` is the
/// repository's own allowlist; nothing else comes from the repository.
pub fn write_synthetic_source(root: &Path) {
    for relative in build_inputs().into_iter().chain(NOT_BUILD_INPUTS) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("relative file has a parent"))
            .expect("SETUP: synthetic source directory");
        if relative == ".dockerignore" {
            fs::copy(
                Path::new(env!("CARGO_MANIFEST_DIR")).join(".dockerignore"),
                &path,
            )
            .expect("SETUP: repository build allowlist");
        } else if relative == COMPOSE_FILE {
            fs::write(&path, "services: {}\n").expect("SETUP: synthetic Compose file");
        } else if let Some(manifest) = match relative {
            "Cargo.toml" => Some("[package]\nname = \"agentsandbox\"\nversion = \"0.0.1\"\n"),
            "Cargo.lock" => {
                Some("version = 4\n[[package]]\nname = \"agentsandbox\"\nversion = \"0.0.1\"\n")
            }
            "front/package.json" => Some(r#"{"name":"ai-guard-front","version":"0.0.1"}"#),
            "front/package-lock.json" => Some(
                r#"{
                  "name":"ai-guard-front","version":"0.0.1","lockfileVersion":3,
                  "packages":{"":{"name":"ai-guard-front","version":"0.0.1"}}
                }"#,
            ),
            _ => None,
        } {
            fs::write(&path, manifest).expect("SETUP: aligned synthetic build manifest");
        } else {
            fs::write(&path, format!("synthetic distribution input: {relative}\n"))
                .expect("SETUP: synthetic source file");
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub directory: bool,
    pub bytes: u64,
    pub sha256: String,
    pub mode: u32,
    pub modified: (i64, i64),
}

/// Path, size, SHA-256, mode and modification time of every entry below `root`.
pub fn tree_manifest(root: &Path) -> BTreeMap<String, TreeEntry> {
    fn visit(root: &Path, relative: &Path, manifest: &mut BTreeMap<String, TreeEntry>) {
        for entry in fs::read_dir(root.join(relative)).expect("SETUP: tree listing") {
            let relative = relative.join(entry.expect("SETUP: tree entry").file_name());
            let path = root.join(&relative);
            let metadata = fs::symlink_metadata(&path).expect("SETUP: tree entry metadata");
            let directory = metadata.is_dir();
            manifest.insert(
                relative.to_string_lossy().into_owned(),
                TreeEntry {
                    directory,
                    bytes: if directory { 0 } else { metadata.len() },
                    sha256: if metadata.is_file() {
                        hex::encode(Sha256::digest(
                            fs::read(&path).expect("SETUP: tree entry content"),
                        ))
                    } else {
                        String::new()
                    },
                    mode: metadata.mode() & 0o7777,
                    modified: (metadata.mtime(), metadata.mtime_nsec()),
                },
            );
            if directory {
                visit(root, &relative, manifest);
            }
        }
    }
    let mut manifest = BTreeMap::new();
    visit(root, Path::new(""), &mut manifest);
    manifest
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DistributionDigest {
    pub sha256: String,
    pub files: u64,
    pub bytes: u64,
}

/// The digest a provenance line reports for a set of prepared files.
///
/// Layout: the domain separator `ai-guard:public-distribution:v1\0`, then for
/// each file in ascending order of its relative path: the path length as a
/// big-endian `u64`, the path bytes, the file length as a big-endian `u64`, and
/// the lowercase hexadecimal SHA-256 of the file content (64 ASCII bytes).
pub fn distribution_digest(files: &BTreeMap<String, Vec<u8>>) -> DistributionDigest {
    let mut digest = Sha256::new();
    digest.update(b"ai-guard:public-distribution:v1\0");
    let mut total = 0_u64;
    for (path, bytes) in files {
        digest.update((path.len() as u64).to_be_bytes());
        digest.update(path.as_bytes());
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(hex::encode(Sha256::digest(bytes)).as_bytes());
        total += bytes.len() as u64;
    }
    DistributionDigest {
        sha256: hex::encode(digest.finalize()),
        files: files.len() as u64,
        bytes: total,
    }
}

#[derive(Clone, Debug)]
pub struct Provenance {
    pub scope: String,
    pub digest: DistributionDigest,
}

/// Provenance lines of a command's standard error:
/// `Prepared public distribution (<scope>): sha256=<hex> files=<n> bytes=<n> …`.
pub fn provenance_lines(stderr: &str) -> Vec<Provenance> {
    stderr
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("Prepared public distribution (")?;
            let (scope, fields) = rest.split_once("): ")?;
            let field = |name: &str| {
                fields
                    .split_whitespace()
                    .find_map(|item| item.strip_prefix(name))
            };
            Some(Provenance {
                scope: scope.to_owned(),
                digest: DistributionDigest {
                    sha256: field("sha256=")?.to_owned(),
                    files: field("files=")?.parse().ok()?,
                    bytes: field("bytes=")?.parse().ok()?,
                },
            })
        })
        .collect()
}

/// Options of one `agentsandbox` child process.
pub struct Spawn {
    pub umask: u32,
    /// Signals the child inherits as ignored. SIGINT, SIGTERM and SIGHUP are
    /// otherwise reset to their default disposition, whatever the test runner
    /// itself inherited.
    pub ignored: Vec<i32>,
}

impl Default for Spawn {
    fn default() -> Self {
        Self {
            umask: 0o022,
            ignored: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Exit {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub elapsed: Duration,
}

impl Exit {
    pub fn describe(&self) -> String {
        format!(
            "{} after {:?}; stderr tail:\n{}",
            self.status,
            self.elapsed,
            tail(&self.stderr, 12)
        )
    }
}

/// A running `agentsandbox` command, leader of its own process group.
pub struct Running {
    child: Child,
    pub pid: u32,
    pub spawned: Instant,
    pub spawned_at: SystemTime,
    stdout: PathBuf,
    stderr: PathBuf,
    exit: Option<Exit>,
}

impl Running {
    /// Sends `signal` to the control process only, never to its process group.
    pub fn signal(&mut self, signal: i32) {
        if self.poll().is_none() {
            // SAFETY: the child is unreaped, so its PID still names it.
            let sent = unsafe { libc::kill(self.pid as libc::pid_t, signal) };
            assert_eq!(sent, 0, "SETUP: the signal could not be sent");
        }
    }
    pub fn poll(&mut self) -> Option<Exit> {
        if self.exit.is_none()
            && let Some(status) = self.child.try_wait().expect("SETUP: child status")
        {
            self.exit = Some(Exit {
                status,
                stdout: fs::read_to_string(&self.stdout).unwrap_or_default(),
                stderr: fs::read_to_string(&self.stderr).unwrap_or_default(),
                elapsed: self.spawned.elapsed(),
            });
        }
        self.exit.clone()
    }
    pub fn wait(&mut self, window: Duration) -> Option<Exit> {
        let started = Instant::now();
        loop {
            if let Some(exit) = self.poll() {
                return Some(exit);
            }
            if started.elapsed() >= window {
                return None;
            }
            std::thread::sleep(POLL);
        }
    }
    pub fn product(&mut self) -> Product {
        match self.poll() {
            Some(exit) => Product::Ended(exit.describe()),
            None => Product::Pending,
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if self.poll().is_none() {
            // SAFETY: the child is unreaped and leads its own process group, so
            // the group identifier still names that group only.
            unsafe {
                libc::kill(-(self.pid as libc::pid_t), libc::SIGKILL);
            }
            let _ = self.child.wait();
        }
    }
}

/// One operator scenario: a control directory with the fixture engine, a
/// synthetic distribution, a configuration directory and a private `TMPDIR`
/// whose leftovers can be listed exactly.
pub struct Operator {
    pub program: PathBuf,
    pub state: FixtureState,
    pub source: PathBuf,
    pub config: PathBuf,
    pub tmp: PathBuf,
    outputs: AtomicU32,
    // Declared last: the state above signals `quit` before the tree is removed.
    root: tempfile::TempDir,
}

impl Operator {
    pub fn prepare() -> Self {
        let root = private_tempdir();
        let base = root
            .path()
            .canonicalize()
            .expect("SETUP: canonical test directory");
        let control = base.join("control");
        fs::create_dir(&control).expect("SETUP: control directory");
        let program = install_engine(&control, "docker");
        let source = base.join("project");
        write_synthetic_source(&source);
        let tmp = base.join("tmp");
        fs::create_dir(&tmp).expect("SETUP: private temporary directory");
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o700))
            .expect("SETUP: private temporary directory mode");
        Self {
            state: FixtureState::for_program(&program),
            program,
            source,
            config: base.join("config"),
            tmp,
            outputs: AtomicU32::new(0),
            root,
        }
    }
    pub fn initialize_config(&self) {
        agentsandbox::operations::initialize_config(&self.config)
            .expect("SETUP: control configuration");
    }
    /// Starts `agentsandbox <action> --runtime-program <fixture> --source
    /// <synthetic source> <further arguments>` in its own process group.
    pub fn spawn(&self, action: &str, further: &[&str], options: &Spawn) -> Running {
        let number = self.outputs.fetch_add(1, Ordering::SeqCst) + 1;
        let base = self.source.parent().expect("source has a parent");
        let stdout = base.join(format!("command-{number}.stdout"));
        let stderr = base.join(format!("command-{number}.stderr"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        command
            .arg(action)
            .arg("--runtime-program")
            .arg(&self.program)
            .arg("--source")
            .arg(&self.source)
            .args(further)
            .env("TMPDIR", &self.tmp)
            .stdin(Stdio::null())
            .stdout(File::create(&stdout).expect("SETUP: command output file"))
            .stderr(File::create(&stderr).expect("SETUP: command output file"))
            // A group signal sent to the wrong group then ends this command
            // visibly instead of the test runner.
            .process_group(0);
        let umask = options.umask as libc::mode_t;
        let ignored = options.ignored.clone();
        // SAFETY: the hook only calls async-signal-safe `signal` and `umask`
        // and reads memory owned by the closure.
        unsafe {
            command.pre_exec(move || {
                for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
                    let disposition = if ignored.contains(&signal) {
                        libc::SIG_IGN
                    } else {
                        libc::SIG_DFL
                    };
                    libc::signal(signal, disposition);
                }
                libc::umask(umask);
                Ok(())
            });
        }
        let spawned_at = SystemTime::now();
        let spawned = Instant::now();
        let child = command
            .spawn()
            .expect("SETUP: agentsandbox was not started");
        Running {
            pid: child.id(),
            child,
            spawned,
            spawned_at,
            stdout,
            stderr,
            exit: None,
        }
    }
    /// Runs a command that is expected to end by itself.
    pub fn run(&self, action: &str, further: &[&str], options: &Spawn) -> Exit {
        let window = Duration::from_secs(120);
        let mut running = self.spawn(action, further, options);
        running.wait(window).unwrap_or_else(|| {
            panic!(
                "UNKNOWN: agentsandbox {action} is still pending after {window:?}; calls: {:?}; {}",
                self.state.calls(),
                self.state.describe()
            )
        })
    }
    /// Names left in the private temporary directory.
    pub fn temporary_entries(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.tmp)
            .expect("SETUP: private temporary directory listing")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
    /// Prepared sources currently present in the private temporary directory.
    pub fn prepared_directories(&self) -> Vec<PathBuf> {
        self.temporary_entries()
            .into_iter()
            .filter(|name| name.starts_with(PREPARED_PREFIX))
            .map(|name| self.tmp.join(name))
            .collect()
    }
    pub fn source_file(&self, relative: &str) -> Vec<u8> {
        fs::read(self.source.join(relative)).expect("SETUP: synthetic source file")
    }
}

/// Canary files of a consumer-level material directory.
pub const MATERIAL_FILES: [(&str, &str); 3] = [
    ("images/control.Dockerfile", "material canary: dockerfile\n"),
    ("Cargo.toml", "material canary: manifest\n"),
    ("src/main.rs", "material canary: source\n"),
];

/// Material one retained command reads: a real private directory.
///
/// Its `Drop` is the release witness. Whoever releases the material runs it:
/// it tries each role's liveness lock without blocking, records
/// `released-while-alive-<role>` or `released-clean` in the fixture state, and
/// only then removes the directory. Tests keep the path, never a handle.
pub struct Material {
    root: PathBuf,
    state: PathBuf,
}

impl Material {
    pub fn create(parent: &Path, state: &FixtureState) -> Self {
        let root = parent.join("material");
        for (relative, content) in MATERIAL_FILES {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("relative file has a parent"))
                .expect("SETUP: material directory");
            fs::write(path, content).expect("SETUP: material file");
        }
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .expect("SETUP: material directory mode");
        Self {
            root,
            state: state.path().to_path_buf(),
        }
    }
    pub fn path(&self) -> &Path {
        &self.root
    }
    /// Engine arguments that name only this material.
    pub fn arguments(&self) -> Vec<String> {
        vec![
            "build".into(),
            "--file".into(),
            self.root
                .join(MATERIAL_FILES[0].0)
                .to_string_lossy()
                .into_owned(),
            "--tag".into(),
            "fixture:local".into(),
            self.root.to_string_lossy().into_owned(),
        ]
    }
    /// The canaries still hold their first content.
    pub fn verify(&self) -> Result<(), String> {
        for (relative, content) in MATERIAL_FILES {
            match fs::read(self.root.join(relative)) {
                Ok(bytes) if bytes == content.as_bytes() => {}
                Ok(_) => return Err(format!("material changed: {relative}")),
                Err(error) => return Err(format!("material unavailable: {relative}: {error}")),
            }
        }
        Ok(())
    }
}

impl Drop for Material {
    fn drop(&mut self) {
        let alive: Vec<&str> = ROLES
            .into_iter()
            .filter(|role| liveness_at(&self.state, role) == Liveness::Alive)
            .collect();
        if alive.is_empty() {
            let _ = fs::write(self.state.join("released-clean"), b"");
        }
        for role in alive {
            let _ = fs::write(self.state.join(format!("released-while-alive-{role}")), b"");
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// One consumer-level scenario: the fixture engine as approved runtime
/// program, its plan, and the path of the material handed to the command.
pub struct Consumer {
    pub program: PathBuf,
    pub state: FixtureState,
    pub material: PathBuf,
    // Declared last: the state above signals `quit` before the tree is removed.
    root: tempfile::TempDir,
}

impl Consumer {
    pub fn prepare(directives: &[(&str, &str)]) -> (Self, Material) {
        let root = private_tempdir();
        let base = root
            .path()
            .canonicalize()
            .expect("SETUP: canonical test directory");
        let control = base.join("control");
        fs::create_dir(&control).expect("SETUP: control directory");
        let program = install_engine(&control, "docker");
        let state = FixtureState::for_program(&program);
        state.plan(directives);
        let material = Material::create(&base, &state);
        (
            Self {
                program,
                state,
                material: material.path().to_path_buf(),
                root,
            },
            material,
        )
    }
    pub fn runtime(&self) -> agentsandbox::runtime::Runtime {
        agentsandbox::runtime::Runtime::with_program(
            agentsandbox::runtime::RuntimeKind::Docker,
            &self.program,
        )
        .unwrap_or_else(|error| panic!("SETUP: the fixture engine was not approved: {error:#}"))
    }
    /// The release witness files written so far.
    pub fn witnesses(&self) -> Vec<String> {
        self.state.names("released-")
    }
}
