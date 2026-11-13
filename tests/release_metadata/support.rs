//! Shared helpers of `tests/release_metadata.rs` and
//! `tests/release_metadata_verify.rs`: a real `agentsandbox release …`
//! process, a synthetic distribution whose locks, manifests and recipes are
//! the repository's own files, and independent oracles.
//!
//! `harness::Operator::spawn` inserts `--runtime-program` and `--source`
//! after the action, which `release` does not take: this module has its own
//! spawner with the same signal and umask setup, its own process group and
//! an explicit `TMPDIR`. From the harness it uses the scenario directories,
//! `build`, `provenance_lines`, `distribution_digest` and `tree_manifest`.
//!
//! Panic prefixes follow the harness: `FAIL:` (product failure), `SETUP:`
//! (fixture or environment did not reach the scenario) and `ENVIRONMENT:` (an
//! observed fact of the test environment, not a product result).
#![allow(dead_code)]

use crate::harness::{Operator, POLL, build_inputs, tail};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::{self, File},
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, Instant},
};

pub const MANIFEST_FILE: &str = "release-manifest.json";
pub const SBOM_FILE: &str = "sbom.cdx.json";
pub const SUMS_FILE: &str = "SHA256SUMS";
pub const DOCUMENTS: [&str; 3] = [MANIFEST_FILE, SBOM_FILE, SUMS_FILE];
pub const PROVENANCE: &str = "Prepared public distribution (build)";
/// Repository files that replace the synthetic content of the harness
/// source, whose plain-text locks no strict reader would accept.
pub const REPOSITORY_FILES: [&str; 7] = [
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "front/package.json",
    "front/package-lock.json",
    "images/control.Dockerfile",
    "images/worker.Dockerfile",
];
pub const WINDOW: Duration = Duration::from_secs(300);

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn repository() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Copies the repository files of [`REPOSITORY_FILES`] over the synthetic
/// files of `source`.
pub fn use_repository_files(source: &Path) {
    for relative in REPOSITORY_FILES {
        fs::copy(repository().join(relative), source.join(relative))
            .unwrap_or_else(|error| panic!("SETUP: repository file {relative}: {error}"));
    }
}

/// Every regular file below `from`, copied to the same relative path below
/// `to`. Contents only: modes and times are those of new files.
pub fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("SETUP: copy destination");
    for entry in fs::read_dir(from).expect("SETUP: copy listing") {
        let entry = entry.expect("SETUP: copy entry");
        let target = to.join(entry.file_name());
        let kind = entry.file_type().expect("SETUP: copy entry type");
        if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else if kind.is_file() {
            fs::copy(entry.path(), target).expect("SETUP: copy file");
        }
    }
}

pub fn make_private_directory(path: &Path) {
    fs::create_dir_all(path).expect("SETUP: private directory");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .expect("SETUP: private directory mode");
}

/// `path` and every missing directory above it, each of mode 0755 whatever
/// the umask of the test process: the artefact and manifest readers refuse
/// group or other write on every ancestor.
pub fn make_public_directories(path: &Path) {
    let mut missing = Vec::new();
    let mut current = path;
    while !current.exists() {
        missing.push(current.to_path_buf());
        current = current
            .parent()
            .expect("SETUP: a directory with an existing ancestor");
    }
    for directory in missing.iter().rev() {
        fs::create_dir(directory).expect("SETUP: directory");
        fs::set_permissions(directory, fs::Permissions::from_mode(0o755))
            .expect("SETUP: directory mode");
    }
}

/// `bytes` at `path`, of mode 0644 whatever the umask of the test process.
pub fn write_public_file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("SETUP: file");
    fs::set_permissions(path, fs::Permissions::from_mode(0o644)).expect("SETUP: file mode");
}

/// Names directly below `directory`, sorted.
pub fn names(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .expect("SETUP: directory listing")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// A release scenario: the harness operator scenario (synthetic source with
/// the repository's locks, manifests and recipes, a private `TMPDIR`), a
/// private parent for outputs, a directory of artefacts and one for logs.
pub struct Scenario {
    pub operator: Operator,
    pub base: PathBuf,
    pub outputs: PathBuf,
    pub artifacts: PathBuf,
    pub logs: PathBuf,
    runs: AtomicU32,
}

impl Scenario {
    pub fn new() -> Self {
        let operator = Operator::prepare();
        use_repository_files(&operator.source);
        let base = operator
            .source
            .parent()
            .expect("SETUP: the source has a parent")
            .to_path_buf();
        let outputs = base.join("outputs");
        make_private_directory(&outputs);
        let artifacts = base.join("artifacts");
        fs::create_dir(&artifacts).expect("SETUP: artefact directory");
        fs::set_permissions(&artifacts, fs::Permissions::from_mode(0o755))
            .expect("SETUP: artefact directory mode");
        let logs = base.join("logs");
        fs::create_dir(&logs).expect("SETUP: log directory");
        Self {
            operator,
            base,
            outputs,
            artifacts,
            logs,
            runs: AtomicU32::new(0),
        }
    }

    pub fn source(&self) -> &Path {
        &self.operator.source
    }

    pub fn tmp(&self) -> &Path {
        &self.operator.tmp
    }

    /// The build inputs of the source and their bytes, as the oracle of the
    /// prepared digest: every allowlisted file the harness writes.
    pub fn build_inputs(&self) -> BTreeMap<String, Vec<u8>> {
        build_inputs()
            .into_iter()
            .map(|relative| (relative.to_owned(), self.operator.source_file(relative)))
            .collect()
    }

    /// An artefact of `bytes` named `name` in the artefact directory, 0644.
    pub fn artifact(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.artifacts.join(name);
        write_public_file(&path, bytes);
        path
    }

    pub fn options(&self) -> Options {
        Options {
            tmpdir: self.operator.tmp.clone(),
            umask: 0o022,
            current_dir: None,
        }
    }

    pub fn spawn<I, S>(&self, arguments: I, options: &Options) -> Release
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let number = self.runs.fetch_add(1, Ordering::SeqCst) + 1;
        spawn_release(arguments, options, &self.logs, number)
    }

    pub fn run<I, S>(&self, arguments: I, options: &Options) -> Outcome
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut running = self.spawn(arguments, options);
        running.wait(WINDOW).unwrap_or_else(|| {
            panic!("SETUP: agentsandbox release is still pending after {WINDOW:?}")
        })
    }

    /// `release manifest --source <source> --output <outputs>/<leaf>` with
    /// `further` arguments.
    pub fn manifest(&self, leaf: &str, further: &[OsString]) -> (PathBuf, Outcome) {
        let output = self.outputs.join(leaf);
        let mut arguments = vec![
            OsString::from("release"),
            "manifest".into(),
            "--source".into(),
            self.operator.source.clone().into_os_string(),
            "--output".into(),
            output.clone().into_os_string(),
        ];
        arguments.extend(further.iter().cloned());
        let outcome = self.run(&arguments, &self.options());
        (output, outcome)
    }

    /// As [`Scenario::manifest`], and the command must succeed.
    pub fn published(&self, leaf: &str, further: &[OsString]) -> PathBuf {
        let (output, outcome) = self.manifest(leaf, further);
        outcome.succeeded("release manifest");
        output
    }

    pub fn verify(&self, source: &Path, manifest: &Path, further: &[OsString]) -> Outcome {
        let mut arguments = vec![
            OsString::from("release"),
            "verify".into(),
            "--source".into(),
            source.as_os_str().to_owned(),
            "--manifest".into(),
            manifest.as_os_str().to_owned(),
        ];
        arguments.extend(further.iter().cloned());
        self.run(&arguments, &self.options())
    }

    /// The private `TMPDIR` holds nothing: no prepared copy was left.
    pub fn assert_tmp_empty(&self, context: &str) {
        let left = self.operator.temporary_entries();
        assert!(
            left.is_empty(),
            "FAIL: {context}: the private temporary directory is not empty: {left:?}"
        );
    }

    /// The command ran nothing of the preparation: no provenance line and
    /// an empty `TMPDIR`.
    pub fn assert_before_preparation(&self, outcome: &Outcome, context: &str) {
        assert!(
            !outcome.stderr.contains(PROVENANCE),
            "FAIL: {context}: the source was prepared before the refusal; stderr tail:\n{}",
            tail(&outcome.stderr, 12)
        );
        self.assert_tmp_empty(context);
    }
}

impl Default for Scenario {
    fn default() -> Self {
        Self::new()
    }
}

/// `--artifact <path>` arguments.
pub fn artifact_arguments(paths: &[&Path]) -> Vec<OsString> {
    paths
        .iter()
        .flat_map(|path| [OsString::from("--artifact"), path.as_os_str().to_owned()])
        .collect()
}

pub fn arguments(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

pub struct Options {
    pub tmpdir: PathBuf,
    pub umask: u32,
    pub current_dir: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub elapsed: Duration,
}

impl Outcome {
    pub fn describe(&self) -> String {
        format!(
            "{} after {:?}; stdout:\n{}\nstderr tail:\n{}",
            self.status,
            self.elapsed,
            tail(&self.stdout, 12),
            tail(&self.stderr, 12)
        )
    }

    pub fn succeeded(&self, context: &str) {
        assert!(
            self.status.success(),
            "FAIL: {context} did not succeed: {}",
            self.describe()
        );
    }

    /// A product refusal: exit status exactly 1 (clap usage errors exit with
    /// 2 and never count) and the message fragment on stderr.
    pub fn refused(&self, fragment: &str, context: &str) {
        assert_eq!(
            self.status.code(),
            Some(1),
            "FAIL: {context}: not a product refusal: {}",
            self.describe()
        );
        assert!(
            self.stderr.contains(fragment),
            "FAIL: {context}: the refusal does not say {fragment:?}: {}",
            self.describe()
        );
    }

    pub fn first_stdout_line(&self) -> &str {
        self.stdout.lines().next().unwrap_or_default()
    }
}

/// A running `agentsandbox release` command, leader of its own process group.
pub struct Release {
    child: Child,
    pub pid: u32,
    started: Instant,
    stdout: PathBuf,
    stderr: PathBuf,
    exit: Option<Outcome>,
}

impl Release {
    pub fn poll(&mut self) -> Option<Outcome> {
        if self.exit.is_none()
            && let Some(status) = self.child.try_wait().expect("SETUP: child status")
        {
            self.exit = Some(Outcome {
                status,
                stdout: fs::read_to_string(&self.stdout).unwrap_or_default(),
                stderr: fs::read_to_string(&self.stderr).unwrap_or_default(),
                elapsed: self.started.elapsed(),
            });
        }
        self.exit.clone()
    }

    pub fn wait(&mut self, window: Duration) -> Option<Outcome> {
        let started = Instant::now();
        loop {
            if let Some(outcome) = self.poll() {
                return Some(outcome);
            }
            if started.elapsed() >= window {
                return None;
            }
            std::thread::sleep(POLL);
        }
    }

    /// What the command wrote to standard error so far.
    pub fn stderr_so_far(&self) -> String {
        fs::read_to_string(&self.stderr).unwrap_or_default()
    }

    /// Sends `signal` to the command process only.
    pub fn signal(&mut self, signal: i32) -> bool {
        if self.poll().is_some() {
            return false;
        }
        // SAFETY: the child is unreaped, so its PID still names it.
        unsafe { libc::kill(self.pid as libc::pid_t, signal) == 0 }
    }
}

impl Drop for Release {
    fn drop(&mut self) {
        if self.poll().is_none() {
            // SAFETY: the child is unreaped and leads its own process group.
            unsafe {
                libc::kill(-(self.pid as libc::pid_t), libc::SIGKILL);
            }
            let _ = self.child.wait();
        }
    }
}

pub fn spawn_release<I, S>(arguments: I, options: &Options, logs: &Path, number: u32) -> Release
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let stdout = logs.join(format!("release-{number}.stdout"));
    let stderr = logs.join(format!("release-{number}.stderr"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    command
        .args(arguments)
        .env("TMPDIR", &options.tmpdir)
        .stdin(Stdio::null())
        .stdout(File::create(&stdout).expect("SETUP: command output file"))
        .stderr(File::create(&stderr).expect("SETUP: command output file"))
        .process_group(0);
    if let Some(directory) = &options.current_dir {
        command.current_dir(directory);
    }
    let umask = options.umask as libc::mode_t;
    // SAFETY: the hook only calls the async-signal-safe `signal` and `umask`.
    unsafe {
        command.pre_exec(move || {
            for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
                libc::signal(signal, libc::SIG_DFL);
            }
            libc::umask(umask);
            Ok(())
        });
    }
    let started = Instant::now();
    let child = command
        .spawn()
        .expect("SETUP: agentsandbox release was not started");
    Release {
        pid: child.id(),
        child,
        started,
        stdout,
        stderr,
        exit: None,
    }
}

/// The three documents of a published directory.
pub fn documents(directory: &Path) -> [Vec<u8>; 3] {
    DOCUMENTS.map(|name| {
        fs::read(directory.join(name))
            .unwrap_or_else(|error| panic!("FAIL: {name} is missing: {error}"))
    })
}

pub fn manifest_json(directory: &Path) -> serde_json::Value {
    serde_json::from_slice(&documents(directory)[0]).expect("FAIL: the manifest is not JSON")
}

pub fn sbom_json(directory: &Path) -> serde_json::Value {
    serde_json::from_slice(&documents(directory)[1]).expect("FAIL: the SBOM is not JSON")
}

/// `SHA256SUMS` as coreutils writes it in text mode, sorted by name.
pub fn sums_text(manifest: &[u8], sbom: &[u8]) -> String {
    format!(
        "{}  {MANIFEST_FILE}\n{}  {SBOM_FILE}\n",
        sha256_hex(manifest),
        sha256_hex(sbom)
    )
}

/// A copy of a published directory at `to`, in files of mode 0644, below
/// directories of mode 0755.
pub fn copy_documents(from: &Path, to: &Path) {
    make_public_directories(to);
    fs::set_permissions(to, fs::Permissions::from_mode(0o755)).expect("SETUP: copy mode");
    for name in DOCUMENTS {
        fs::copy(from.join(name), to.join(name)).expect("SETUP: copy of a document");
        fs::set_permissions(to.join(name), fs::Permissions::from_mode(0o644))
            .expect("SETUP: copy mode");
    }
}

/// Rewrites `release-manifest.json` of `directory` with `edit` applied to
/// its text, and recomputes `SHA256SUMS`, as a tamperer would.
pub fn rewrite_manifest(directory: &Path, edit: impl FnOnce(String) -> String) {
    let path = directory.join(MANIFEST_FILE);
    let original = fs::read_to_string(&path).expect("SETUP: manifest text");
    let edited = edit(original.clone());
    assert_ne!(edited, original, "SETUP: the edit changed nothing");
    fs::write(path, &edited).expect("SETUP: edited manifest");
    let sbom = fs::read(directory.join(SBOM_FILE)).expect("SETUP: SBOM");
    fs::write(
        directory.join(SUMS_FILE),
        sums_text(edited.as_bytes(), &sbom),
    )
    .expect("SETUP: recomputed sums");
}

/// Replaces the first occurrence of `from` in the file at `path`.
pub fn edit_file(path: &Path, from: &str, to: &str) {
    let text = fs::read_to_string(path).expect("SETUP: file to edit");
    assert!(
        text.contains(from),
        "SETUP: {from:?} is not in {}",
        path.display()
    );
    fs::write(path, text.replacen(from, to, 1)).expect("SETUP: edited file");
}

/// The build-input identity of the documented layout
/// `ai-guard:build-inputs:v1`, computed here.
pub fn inputs_identity(digest: &str, codex: Option<&str>, claude_code: Option<&str>) -> String {
    let mut hash = Sha256::new();
    hash.update(b"ai-guard:build-inputs:v1\0");
    hash.update(64_u64.to_be_bytes());
    hash.update(digest.as_bytes());
    for declared in [codex, claude_code] {
        let value = declared.unwrap_or_default();
        hash.update([u8::from(declared.is_some())]);
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value.as_bytes());
    }
    hex::encode(hash.finalize())
}

/// Link count and inode of a path, for an observation line.
pub fn link_facts(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(facts) => format!(
            "nlink={} ino={} mode={:o} uid={} {}",
            facts.nlink(),
            facts.ino(),
            facts.mode() & 0o7777,
            facts.uid(),
            path.display()
        ),
        Err(error) => format!("unavailable ({error}): {}", path.display()),
    }
}

/// The first `[[package]]` count and checksum lines of a `Cargo.lock`,
/// scanned by lines.
pub fn cargo_lock_counts(text: &str) -> (usize, usize) {
    let packages = text.lines().filter(|line| *line == "[[package]]").count();
    let checksums = text
        .lines()
        .filter(|line| line.starts_with("checksum = \""))
        .count();
    (packages, checksums)
}
