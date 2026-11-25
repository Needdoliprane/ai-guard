//! Bounded, filtered capture of a distribution's public inputs.
//!
//! The selected source is read without following links, file by file, each
//! through one descriptor whose identity and state are compared before and
//! after the read. The bytes go to a new private directory under the temporary
//! directory. A second pass then confirms that the selected set and every
//! captured original still are what was read.
//!
//! This is a sequence of stable reads, not an atomic snapshot of the tree. An
//! original is compared by device, inode, length, mode and its two timestamps:
//! a rewrite that keeps its length and lands within the precision of those
//! timestamps is not seen. The owner, modes, link count and filesystem of the
//! source are not judged: the copy is private whatever they are, and `admit`
//! is the one place where a stricter rule on a source file would go.
use crate::{
    control_fs::{self, Directory, PrivateFile},
    runtime::{
        RuntimeKind,
        retained_cli::{Interruption, InvocationFailure, StopReason, UnknownEnd, signal_name},
    },
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CString, OsStr},
    fs::{self, File, Metadata},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Component, Path, PathBuf},
};

/// Name prefix of a prepared source under the temporary directory.
pub(super) const PREFIX: &str = "ai-guard-distribution-";
/// Entries looked at under the source root and its selected directories.
const MAX_ENTRIES: usize = 16_384;
const MAX_BYTES: u64 = 256 * 1024 * 1024;
pub(super) const MAX_COMPOSE_BYTES: u64 = 1024 * 1024;
pub(super) const MAX_DEPTH: usize = 64;
const MAX_PATH_BYTES: usize = 4096;
/// Characters of a source or copy name shown in a message.
const MAX_SHOWN_CHARACTERS: usize = 1024;
/// One handle for every directory a walk can have entered at once. With
/// fewer, walkdir gives the oldest handle up by reading that directory to its
/// end into memory, in one step that neither the entry bound nor a stop
/// request can interrupt.
const MAX_OPEN_DIRECTORIES: usize = MAX_DEPTH + 2;
const DIGEST_DOMAIN: &[u8] = b"ai-guard:public-distribution:v1\0";
/// `DIGEST_DOMAIN` without its terminating NUL: the name of the layout the
/// digest is computed with, for a document that records the digest.
pub(super) const DIGEST_LAYOUT: &str = "ai-guard:public-distribution:v1";
// The recorded layout name and the domain the digest is computed with cannot
// drift apart: a change of one without the other does not compile.
const _: () = {
    let layout = DIGEST_LAYOUT.as_bytes();
    assert!(DIGEST_DOMAIN.len() == layout.len() + 1);
    assert!(DIGEST_DOMAIN[layout.len()] == 0);
    let mut index = 0;
    while index < layout.len() {
        assert!(DIGEST_DOMAIN[index] == layout[index]);
        index += 1;
    }
};
const MANIFEST: &str = "manifest.json";
const COMPOSE_ENV_FILE: &str = "compose-empty.env";
const BUILD_IGNORE_OVERRIDE: &str = "podman-build-ignore";
const BUILD_IGNORE_OVERRIDE_BYTES: &[u8] = b".dockerignore\n";
pub(super) const UNKNOWN_MARKER: &str = "RETAINED-UNKNOWN.json";

/// The ignore file the build allowlist of `Scope::selects_file` mirrors.
const BUILD_IGNORE_FILE: &str = ".dockerignore";
/// SHA-256 of the `.dockerignore` shipped with the distribution this CLI is
/// built from. The allowlist is that file, compiled in: a selected
/// distribution whose `.dockerignore` has other bytes is refused, because the
/// engine would apply rules the copy was not selected with. Change the file,
/// the allowlist and this digest together.
pub(super) const BUILD_IGNORE_SHA256: &str =
    "e0bca73fb949b5e9fddb7ef0ccb7c7fb35d79de59ee8dedc9979526baae77b18";
/// Suffix of a per-Dockerfile ignore file. The shipped distribution has none,
/// and a builder may apply such a file instead of `.dockerignore`.
const DOCKERFILE_IGNORE_SUFFIX: &str = ".Dockerfile.dockerignore";

const DIRECTORY_FLAGS: libc::c_int =
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
/// A directory that is only gone through on the way to another one. On Linux
/// it is opened without being read, so search permission on it is enough.
/// `O_DIRECTORY` stays: `O_PATH | O_NOFOLLOW` alone would open a symbolic link
/// itself. Elsewhere such a directory is opened for reading, like the others.
#[cfg(target_os = "linux")]
const ANCESTOR_FLAGS: libc::c_int =
    libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
#[cfg(not(target_os = "linux"))]
const ANCESTOR_FLAGS: libc::c_int = DIRECTORY_FLAGS;
// O_NONBLOCK: opening a FIFO never blocks before `fstat` refuses its type.
const FILE_FLAGS: libc::c_int =
    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;

/// What a refusal says first when its cause is the descriptor limit.
const DESCRIPTOR_LIMIT: &str = "the open-file limit of this process or of the system was reached \
    while the source was being prepared; raise the descriptor limit (`ulimit -n`) and run the \
    command again";
/// Messages of the private-directory primitives that end with the system
/// error of a refused open instead of carrying it as their cause.
const FORMATTED_OPEN_FAILURES: [&str; 2] = [
    "create new private database entry: ",
    "private file cannot be opened without following a link: ",
];

/// Names the descriptor limit when it is what refused a preparation, whichever
/// open met it first: the first message would otherwise speak of a source
/// input, a control directory or the copy.
pub(super) fn descriptor_limit(error: anyhow::Error) -> anyhow::Error {
    let exhausted = [libc::EMFILE, libc::ENFILE].into_iter().any(|code| {
        let reported = std::io::Error::from_raw_os_error(code).to_string();
        error.chain().any(|cause| {
            let typed = cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|failure| failure.raw_os_error() == Some(code));
            let text = cause.to_string();
            typed
                || FORMATTED_OPEN_FAILURES.iter().any(|prefix| {
                    text.strip_prefix(prefix)
                        .is_some_and(|rest| rest == reported)
                })
        })
    });
    if exhausted {
        error.context(DESCRIPTOR_LIMIT)
    } else {
        error
    }
}

/// Bounds of one capture. Only smaller values than the defaults are accepted.
#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub(super) entries: usize,
    pub(super) bytes: u64,
    pub(super) file_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            entries: MAX_ENTRIES,
            bytes: MAX_BYTES,
            file_bytes: control_fs::MAX_PRIVATE_FILE_BYTES,
        }
    }
}

/// What one operator command hands to the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Scope {
    /// The files the build allowlist (`.dockerignore`) admits and that file
    /// itself. A per-Dockerfile ignore file is selected only to be refused.
    Build,
    /// Exactly `deploy/<kind>/compose.yaml`.
    Compose(RuntimeKind),
}

impl Scope {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Compose(_) => "compose",
        }
    }

    /// Relative path of the Compose file of this scope.
    pub(super) fn compose_file(self) -> Option<String> {
        match self {
            Self::Build => None,
            Self::Compose(kind) => Some(format!("deploy/{}/compose.yaml", kind.program())),
        }
    }

    fn file_bound(self, limits: &Limits) -> u64 {
        match self {
            Self::Build => limits.file_bytes,
            Self::Compose(_) => limits.file_bytes.min(MAX_COMPOSE_BYTES),
        }
    }

    /// Refuses a build input that would have the engine apply ignore rules
    /// other than those this CLI selected the copy with. `sha256` is the
    /// digest of the bytes read for `name`, before anything of it is written.
    ///
    /// The copy of a build therefore holds exactly one ignore file, the one
    /// the allowlist mirrors: `.containerignore` names are not selected at all.
    fn reconcile(self, name: &str, sha256: &str) -> Result<()> {
        if self != Self::Build {
            return Ok(());
        }
        ensure!(
            name != BUILD_IGNORE_FILE || sha256 == BUILD_IGNORE_SHA256,
            "the build input list of this CLI mirrors a {BUILD_IGNORE_FILE} whose SHA-256 is \
             {BUILD_IGNORE_SHA256}; the {BUILD_IGNORE_FILE} of the selected distribution has \
             SHA-256 {sha256}. Build with the CLI built from that distribution, or run the \
             engine build directly"
        );
        ensure!(
            !name.ends_with(DOCKERFILE_IGNORE_SUFFIX),
            "the build input list of this CLI mirrors {BUILD_IGNORE_FILE} only: the \
             per-Dockerfile ignore file {name} of the selected distribution is not reconciled \
             with it, and a builder may apply that file instead. Remove it, or run the engine \
             build directly"
        );
        Ok(())
    }

    /// The closed allowlist. It mirrors the `.dockerignore` whose digest is
    /// `BUILD_IGNORE_SHA256`: change the file, this list and that digest
    /// together. A `*` of that file never crosses a directory, and `**/` also
    /// stands for no directory at all.
    fn selects_file(self, parts: &[&OsStr]) -> bool {
        let named = |index: usize, name: &str| parts.get(index).is_some_and(|part| *part == name);
        let ends = |suffix: &str| {
            parts
                .last()
                .is_some_and(|part| part.as_bytes().ends_with(suffix.as_bytes()))
        };
        match self {
            Self::Compose(kind) => {
                parts.len() == 3
                    && named(0, "deploy")
                    && named(1, kind.program())
                    && named(2, "compose.yaml")
            }
            Self::Build => match parts.len() {
                0 => false,
                1 => [
                    BUILD_IGNORE_FILE,
                    "Cargo.toml",
                    "Cargo.lock",
                    "rust-toolchain.toml",
                    "LICENSE",
                ]
                .iter()
                .any(|name| named(0, name)),
                length => {
                    named(0, "src") && ends(".rs")
                        || named(0, "schema") && ends(".sql")
                        || length == 2 && named(0, "policies") && ends(".json")
                        || length == 2
                            && named(0, "front")
                            && [
                                "package.json",
                                "package-lock.json",
                                "tsconfig.json",
                                "vite.config.ts",
                                "index.html",
                            ]
                            .iter()
                            .any(|name| named(1, name))
                        || length >= 3
                            && named(0, "front")
                            && named(1, "src")
                            && (ends(".ts") || ends(".css"))
                        || length == 3
                            && named(0, "front")
                            && named(1, "public")
                            && named(2, "favicon.svg")
                        || length == 2
                            && named(0, "images")
                            && (ends(".Dockerfile") || ends(DOCKERFILE_IGNORE_SUFFIX))
                        || length == 4
                            && named(0, "images")
                            && named(1, "provider-activity")
                            && (named(2, "claude-code")
                                && (named(3, "managed-settings.v1.json")
                                    || named(3, "managed-settings.v1-onfailure.json"))
                                || named(2, "codex") && named(3, "requirements.toml"))
                }
            },
        }
    }

    /// Directories the enumeration enters.
    fn selects_directory(self, parts: &[&OsStr]) -> bool {
        let named = |index: usize, name: &str| parts.get(index).is_some_and(|part| *part == name);
        match self {
            Self::Compose(kind) => match parts.len() {
                1 => named(0, "deploy"),
                2 => named(0, "deploy") && named(1, kind.program()),
                _ => false,
            },
            Self::Build => match parts.len() {
                0 => false,
                1 => ["src", "schema", "policies", "front", "images"]
                    .iter()
                    .any(|name| named(0, name)),
                length => {
                    named(0, "src")
                        || named(0, "schema")
                        || named(0, "front") && named(1, "src")
                        || length == 2 && named(0, "front") && named(1, "public")
                        || length == 2 && named(0, "images") && named(1, "provider-activity")
                        || length == 3
                            && named(0, "images")
                            && named(1, "provider-activity")
                            && (named(2, "claude-code") || named(2, "codex"))
                }
            },
        }
    }
}

/// Identity and state of a source file, taken from the descriptor that
/// produced its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FileRecord {
    device: u64,
    inode: u64,
    len: u64,
    mode: u32,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl FileRecord {
    fn of(facts: &Metadata) -> Self {
        Self {
            device: facts.dev(),
            inode: facts.ino(),
            len: facts.len(),
            mode: facts.mode(),
            mtime: (facts.mtime(), facts.mtime_nsec()),
            ctime: (facts.ctime(), facts.ctime_nsec()),
        }
    }

    fn shared_writable(&self) -> bool {
        self.mode & 0o022 != 0
    }
}

/// The single place for a rule on a source file's own attributes. Owner, modes,
/// link count and filesystem are deliberately not judged here: sources that
/// build today (a clone made under umask 002, a checkout on a network or
/// virtual filesystem) stay accepted, and the copy is private in any case.
fn admit(relative: &Path, facts: &Metadata) -> Result<()> {
    ensure!(
        facts.is_file(),
        "source input must be a regular file: {}",
        relative.display()
    );
    Ok(())
}

fn open_at(parent: &File, name: &OsStr, flags: libc::c_int) -> std::io::Result<File> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "name holds a NUL"))?;
    // SAFETY: `parent` owns a valid directory descriptor and `name` is one
    // NUL-terminated component; no pointer outlives the call.
    let descriptor = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a successful `openat` returned a descriptor nobody else owns.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

/// Every directory from `/` down to `path`, an absolute path made of plain
/// components, each opened below the one before it and never through a
/// symbolic link. The last one is opened for reading, the others only to be
/// gone through. The descriptors are what later identity checks compare.
fn open_chain(path: &Path) -> std::io::Result<Vec<File>> {
    let names: Vec<&OsStr> = path
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    let flags = |opened: usize| {
        if opened == names.len() {
            DIRECTORY_FLAGS
        } else {
            ANCESTOR_FLAGS
        }
    };
    let mut chain = Vec::with_capacity(names.len() + 1);
    let mut current = fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags(0))
        .open("/")?;
    for (index, name) in names.iter().enumerate() {
        let next = open_at(&current, name, flags(index + 1))?;
        chain.push(std::mem::replace(&mut current, next));
    }
    chain.push(current);
    Ok(chain)
}

/// The selected source directory, opened from `/` without following a link.
/// It is read through these descriptors and never written.
pub(super) struct SourceRoot {
    path: PathBuf,
    chain: Vec<File>,
}

impl SourceRoot {
    /// `canonical` is the canonical path the operator command resolved.
    pub(super) fn open(canonical: &Path) -> Result<Self> {
        let path = control_fs::absolute_path(canonical)?;
        let chain = open_chain(&path).with_context(|| {
            format!(
                "source directory is unavailable or reached through a symbolic link: {}",
                shown(&path)
            )
        })?;
        Ok(Self { path, chain })
    }

    fn root(&self) -> Result<&File> {
        self.chain
            .last()
            .context("source root descriptor is missing")
    }

    /// Device and inode of the source directory.
    fn identity(&self) -> Result<(u64, u64)> {
        let facts = self
            .root()?
            .metadata()
            .context("source directory identity is unavailable")?;
        Ok((facts.dev(), facts.ino()))
    }

    /// The path still names the directories that were opened.
    fn revalidate(&self) -> Result<()> {
        let current = Self::open(&self.path)?;
        let changed = || {
            format!(
                "source directory changed while it was being prepared: {}",
                shown(&self.path)
            )
        };
        ensure!(current.chain.len() == self.chain.len(), changed());
        for (held, named) in self.chain.iter().zip(&current.chain) {
            let held = held.metadata().with_context(changed)?;
            let named = named.metadata().with_context(changed)?;
            ensure!(
                (held.dev(), held.ino()) == (named.dev(), named.ino()),
                changed()
            );
        }
        Ok(())
    }

    /// Opens one entry below the root, component by component, never through a
    /// symbolic link.
    fn open_beneath(&self, relative: &Path, directory: bool) -> Result<File> {
        let unavailable = |error: std::io::Error| {
            anyhow::Error::new(error).context(format!(
                "source input is unavailable or is a symbolic link: {}",
                relative.display()
            ))
        };
        let mut parts = Vec::new();
        for part in relative.components() {
            let Component::Normal(name) = part else {
                bail!("invalid source path: {}", relative.display());
            };
            parts.push(name);
        }
        let Some((last, parents)) = parts.split_last() else {
            bail!("invalid source path: {}", relative.display());
        };
        let mut current: Option<File> = None;
        for name in parents {
            let parent = match &current {
                Some(parent) => parent,
                None => self.root()?,
            };
            current = Some(open_at(parent, name, DIRECTORY_FLAGS).map_err(unavailable)?);
        }
        let parent = match &current {
            Some(parent) => parent,
            None => self.root()?,
        };
        let flags = if directory {
            DIRECTORY_FLAGS
        } else {
            FILE_FLAGS
        };
        open_at(parent, last, flags).map_err(unavailable)
    }

    /// Bytes of one regular file and the record of the descriptor that
    /// produced them. `file_bound` is the bound of one file, `total_bound` the
    /// bound of the whole copy and `total_taken` what the copy holds so far.
    pub(super) fn read_stable(
        &self,
        relative: &Path,
        file_bound: u64,
        total_bound: u64,
        total_taken: u64,
    ) -> Result<(Vec<u8>, FileRecord)> {
        let mut file = self.open_beneath(relative, false)?;
        let facts = file
            .metadata()
            .with_context(|| format!("source input is unavailable: {}", relative.display()))?;
        admit(relative, &facts)?;
        let before = FileRecord::of(&facts);
        ensure!(
            before.len <= file_bound,
            "source input exceeds the per-file bound of {file_bound} bytes: {}",
            relative.display()
        );
        ensure!(
            before.len <= total_bound.saturating_sub(total_taken),
            "source inputs exceed the total bound of {total_bound} bytes of the prepared copy \
             at: {}",
            relative.display()
        );
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(before.len + 1)
            .read_to_end(&mut bytes)
            .with_context(|| format!("source input cannot be read: {}", relative.display()))?;
        let facts = file
            .metadata()
            .with_context(|| format!("source input is unavailable: {}", relative.display()))?;
        ensure!(
            FileRecord::of(&facts) == before && bytes.len() as u64 == before.len,
            "source file changed while it was being read: {}",
            relative.display()
        );
        Ok((bytes, before))
    }

    /// The record the path names now.
    pub(super) fn record(&self, relative: &Path) -> Result<FileRecord> {
        let facts = self
            .open_beneath(relative, false)?
            .metadata()
            .with_context(|| format!("source input is unavailable: {}", relative.display()))?;
        Ok(FileRecord::of(&facts))
    }
}

/// A path as a message shows it: bounded, with every control character and
/// backslash escaped. Absolute roots are operational paths, not validated
/// source-entry names, and need the same rendering before terminal output.
/// The returned text must never be used to open a path or bind its identity.
pub(super) fn shown(name: &Path) -> String {
    let text = name.to_string_lossy();
    let mut shown = String::new();
    for (position, character) in text.chars().enumerate() {
        if position == MAX_SHOWN_CHARACTERS {
            shown.push('…');
            break;
        }
        if character.is_control() || character == '\\' {
            shown.extend(character.escape_default());
        } else {
            shown.push(character);
        }
    }
    shown
}

/// A failed step of a walk below `root`, without walkdir's own rendering,
/// which prints the path as it is.
fn walk_failed(what: &str, root: &Path, error: walkdir::Error) -> anyhow::Error {
    let at = error
        .path()
        .map(|path| path.strip_prefix(root).unwrap_or(path))
        .filter(|path| !path.as_os_str().is_empty())
        .map_or_else(
            || what.to_owned(),
            |path| format!("{what}: {}", shown(path)),
        );
    match error.into_io_error() {
        Some(cause) => anyhow::Error::new(cause).context(at),
        None => anyhow!(at),
    }
}

/// UTF-8, bounded, made of plain components and free of control characters:
/// the name goes into messages, the manifest and an engine argument.
fn validate(relative: &Path) -> Result<&str> {
    let name = relative
        .to_str()
        .with_context(|| format!("source path must be UTF-8: {}", shown(relative)))?;
    ensure!(
        name.len() <= MAX_PATH_BYTES && relative.components().count() <= MAX_DEPTH,
        "source path exceeds the bound of {MAX_DEPTH} components or {MAX_PATH_BYTES} bytes: {}",
        shown(relative)
    );
    ensure!(
        relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
            && !name.chars().any(char::is_control),
        "invalid source path: {name:?}"
    );
    Ok(name)
}

/// Calls `selected` for every regular file of the scope, in directory order.
///
/// A symbolic link is refused wherever the scope would take its path as a file
/// or enter it as a directory: a linked directory is not silently left out. A
/// special file is refused where a file would be taken.
fn enumerate(
    source: &SourceRoot,
    scope: Scope,
    limits: &Limits,
    stop: Option<&Interruption>,
    mut selected: impl FnMut(&Path, &str) -> Result<()>,
) -> Result<()> {
    let mut observed = 0_usize;
    // Not sorted: sorting would collect an arbitrarily large directory before
    // the entry bound applies. For the same reason no handle is given up
    // before its directory was left.
    let mut walk = walkdir::WalkDir::new(&source.path)
        .follow_links(false)
        .max_depth(MAX_DEPTH + 1)
        .max_open(MAX_OPEN_DIRECTORIES)
        .into_iter();
    while let Some(entry) = walk.next() {
        if let Some(signal) = stop.and_then(Interruption::requested) {
            return Err(InvocationFailure::Interrupted { signal }.into());
        }
        let entry =
            entry.map_err(|error| walk_failed("source enumeration failed", &source.path, error))?;
        let relative = entry
            .path()
            .strip_prefix(&source.path)
            .context("source enumeration left the selected directory")?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        observed += 1;
        ensure!(
            observed <= limits.entries,
            "source exceeds the bound of {} entries in its selected directories",
            limits.entries
        );
        let parts: Vec<&OsStr> = relative.components().map(|part| part.as_os_str()).collect();
        let kind = entry.file_type();
        let as_file = scope.selects_file(&parts);
        let as_directory = scope.selects_directory(&parts);
        if kind.is_symlink() {
            // Refused before any rule on names applies to it.
            ensure!(
                !as_file && !as_directory,
                "source input must not be a symbolic link: {}",
                shown(relative)
            );
        } else if kind.is_dir() {
            if as_directory {
                validate(relative)?;
            } else {
                walk.skip_current_dir();
            }
        } else if as_file {
            let name = validate(relative)?;
            ensure!(
                kind.is_file(),
                "source input must be a regular file: {}",
                relative.display()
            );
            selected(relative, name)?;
        }
    }
    Ok(())
}

/// The private root of one prepared source. Dropping it removes the tree while
/// its path still names the directory that was created; a removal that fails
/// or is not attempted is reported, never silent.
struct Storage {
    directory: Option<tempfile::TempDir>,
    /// Device and inode of the directory that was created.
    identity: (u64, u64),
}

impl Storage {
    /// A new `0700` directory under `parent`, the temporary directory, then
    /// the private directory policy on it and on every ancestor.
    ///
    /// `parent` must be absolute and outside `source`. Both are checked before
    /// anything is created, so no value of `TMPDIR` has the original written.
    fn create(parent: &Path, source: &SourceRoot) -> Result<(Self, Directory)> {
        // A relative or empty value would be resolved against the working
        // directory, which is the selected source in the documented commands.
        ensure!(
            parent.is_absolute(),
            "the temporary directory must be an absolute path, but TMPDIR is \"{}\"; set TMPDIR \
             to an absolute directory outside the selected source",
            shown(parent)
        );
        let refused = || {
            format!(
                "cannot create the private prepared-source directory under {} (TMPDIR). Set TMPDIR \
                 to a directory owned by you or root, not writable by group or others, on a \
                 supported local filesystem, with no symbolic link in its path",
                shown(parent)
            )
        };
        // Compared by identity along the directories that lead to it: another
        // spelling of the path cannot hide the source.
        let leading = control_fs::absolute_path(parent)
            .and_then(|path| Ok(open_chain(&path)?))
            .with_context(refused)?;
        let origin = source.identity()?;
        for directory in &leading {
            let facts = directory.metadata().with_context(refused)?;
            ensure!(
                (facts.dev(), facts.ino()) != origin,
                "the temporary directory {} (TMPDIR) is the selected source {} or lies inside \
                 it; set TMPDIR to a directory outside the source",
                shown(parent),
                shown(&source.path)
            );
        }
        drop(leading);
        let directory = tempfile::Builder::new()
            .prefix(PREFIX)
            // The private mode is requested at creation: no captured byte ever
            // exists under a wider mode.
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(parent)
            .with_context(refused)?;
        let created = directory.path().to_owned();
        let identity = match fs::symlink_metadata(&created) {
            Ok(facts) => (facts.dev(), facts.ino()),
            Err(error) => {
                // What the path names now is not known to be what was created.
                let _ = directory.keep();
                return Err(error).with_context(refused);
            }
        };
        // Owned from here on: a refusal below removes what was just created.
        let storage = Self {
            directory: Some(directory),
            identity,
        };
        let private = control_fs::absolute_path(&created)
            .and_then(|path| Directory::open_existing(&path))
            .with_context(refused)?;
        Ok((storage, private))
    }
}

impl Drop for Storage {
    fn drop(&mut self) {
        let Some(directory) = self.directory.take() else {
            return;
        };
        let path = directory.path().to_owned();
        // The tree is removed by name. This comparison is a check, not a lock:
        // it leaves the instant up to the removal, and only this account or
        // root can move a private directory or put another one at its name.
        let named = fs::symlink_metadata(&path).map(|facts| (facts.dev(), facts.ino()));
        if named.as_ref().ok() == Some(&self.identity) {
            if let Err(error) = directory.close() {
                eprintln!(
                    "warning: prepared source at {} could not be removed: {error}",
                    shown(&path)
                );
            }
            return;
        }
        // The copy was moved, or something else took its name: neither is
        // removed through this path.
        let _ = directory.keep();
        let found = match named {
            Ok(_) => "another entry has that name".to_owned(),
            Err(error) => error.to_string(),
        };
        eprintln!(
            "warning: prepared source at {} was not removed: the path no longer names the \
             directory that was prepared ({found}); nothing was removed",
            shown(&path)
        );
    }
}

/// Private directories of the copy, created on first use.
///
/// Only the directories that lead to the one in use stay open, so the copy
/// holds at most one descriptor per level of depth, however many directories
/// the source has. A directory that is needed again is opened again.
struct Destinations {
    root: Directory,
    known: BTreeMap<PathBuf, Directory>,
}

impl Destinations {
    fn directory(&mut self, relative: &Path) -> Result<Directory> {
        if relative.as_os_str().is_empty() {
            return Ok(self.root.clone());
        }
        if let Some(known) = self.known.get(relative) {
            return Ok(known.clone());
        }
        self.known.retain(|kept, _| relative.starts_with(kept));
        let parent = self.directory(relative.parent().unwrap_or(Path::new("")))?;
        let name = relative
            .file_name()
            .context("prepared directory has no name")?;
        let directory = parent.child(name)?;
        self.known.insert(relative.to_owned(), directory.clone());
        Ok(directory)
    }
}

struct CapturedFile {
    path: String,
    bytes: u64,
    sha256: String,
    record: FileRecord,
}

/// A copy whose originals have not been confirmed yet.
pub(super) struct Captured {
    source: SourceRoot,
    scope: Scope,
    limits: Limits,
    private: Directory,
    context: Directory,
    files: Vec<CapturedFile>,
    bytes: u64,
    // Declared last: the directory handles above are closed before the tree
    // is removed.
    storage: Storage,
}

/// Copies the files of `scope` from `source` into a new private directory
/// under `temporary`. A stop request is looked at between entries.
pub(super) fn capture(
    source: &Path,
    scope: Scope,
    limits: Limits,
    temporary: &Path,
    stop: Option<&Interruption>,
) -> Result<Captured> {
    copy(source, scope, limits, temporary, stop).map_err(descriptor_limit)
}

fn copy(
    source: &Path,
    scope: Scope,
    limits: Limits,
    temporary: &Path,
    stop: Option<&Interruption>,
) -> Result<Captured> {
    ensure!(
        limits.entries > 0
            && limits.entries <= MAX_ENTRIES
            && limits.bytes <= MAX_BYTES
            && limits.file_bytes <= control_fs::MAX_PRIVATE_FILE_BYTES,
        "invalid prepared-source bounds"
    );
    let source = SourceRoot::open(source)?;
    let (storage, private) = Storage::create(temporary, &source)?;
    let context = private.child("context")?;
    let file_bound = scope.file_bound(&limits);
    let mut destinations = Destinations {
        root: context.clone(),
        known: BTreeMap::new(),
    };
    let mut files = Vec::new();
    let mut total = 0_u64;
    enumerate(&source, scope, &limits, stop, |relative, name| {
        // The bound is checked on the descriptor before a byte is read, and
        // nothing is created in the copy for a refused input.
        let (bytes, record) = source.read_stable(relative, file_bound, limits.bytes, total)?;
        let sha256 = hex::encode(Sha256::digest(&bytes));
        scope.reconcile(name, &sha256)?;
        total += bytes.len() as u64;
        let parent = destinations.directory(relative.parent().unwrap_or(Path::new("")))?;
        let file_name = relative
            .file_name()
            .context("source input has no file name")?;
        parent
            .create_private_entry(file_name)
            .and_then(|mut output| Ok(output.write_all(&bytes)?))
            .with_context(|| format!("cannot write the prepared copy of {name}"))?;
        files.push(CapturedFile {
            path: name.to_owned(),
            bytes: bytes.len() as u64,
            sha256,
            record,
        });
        Ok(())
    })?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    // A build context always holds the ignore file that was reconciled.
    ensure!(
        scope != Scope::Build || files.iter().any(|file| file.path == BUILD_IGNORE_FILE),
        "required build input is unavailable: {BUILD_IGNORE_FILE}"
    );
    Ok(Captured {
        source,
        scope,
        limits,
        private,
        context,
        files,
        bytes: total,
        storage,
    })
}

impl Captured {
    /// Confirms that the source still is what was read: the same selected
    /// paths, the same record for every captured original, the same directory
    /// behind the source path. The copy then becomes a prepared snapshot and
    /// nothing of the original stays open.
    pub(super) fn confirm(self, stop: Option<&Interruption>) -> Result<Snapshot> {
        self.confirmed(stop).map_err(descriptor_limit)
    }

    fn confirmed(self, stop: Option<&Interruption>) -> Result<Snapshot> {
        // Bound first, so dropped last on a refusal: the directory handles are
        // closed before the tree is removed.
        let Self {
            storage,
            source,
            scope,
            limits,
            private,
            context,
            files,
            bytes,
        } = self;
        let changed = |path: &str| format!("source changed while it was being prepared: {path}");
        let mut named = BTreeSet::new();
        enumerate(&source, scope, &limits, stop, |_, name| {
            named.insert(name.to_owned());
            Ok(())
        })?;
        for file in &files {
            ensure!(named.remove(&file.path), changed(&file.path));
        }
        if let Some(added) = named.first() {
            bail!(changed(added));
        }
        for file in &files {
            if let Some(signal) = stop.and_then(Interruption::requested) {
                return Err(InvocationFailure::Interrupted { signal }.into());
            }
            let now = source
                .record(Path::new(&file.path))
                .with_context(|| changed(&file.path))?;
            ensure!(now == file.record, changed(&file.path));
        }
        source.revalidate()?;
        let (source_device, source_inode) = source.identity()?;
        let source_path = source.path.clone();
        // Nothing of the original is held from here on: only its recorded
        // identity remains, as provenance.
        drop(source);

        let shared_writable = files
            .iter()
            .filter(|file| file.record.shared_writable())
            .count();
        let mut digest = Sha256::new();
        digest.update(DIGEST_DOMAIN);
        for file in &files {
            digest.update((file.path.len() as u64).to_be_bytes());
            digest.update(file.path.as_bytes());
            digest.update(file.bytes.to_be_bytes());
            digest.update(file.sha256.as_bytes());
        }
        let files: Vec<Entry> = files
            .into_iter()
            .map(|file| Entry {
                path: file.path,
                bytes: file.bytes,
                sha256: file.sha256,
            })
            .collect();
        if let Some(compose) = scope.compose_file() {
            ensure!(
                files.len() == 1 && files[0].path == compose,
                "compose file is missing: {compose}"
            );
        }
        let env_file = match scope {
            Scope::Build => None,
            Scope::Compose(_) => {
                private.create_private_entry(OsStr::new(COMPOSE_ENV_FILE))?;
                Some(private.path().join(COMPOSE_ENV_FILE))
            }
        };
        let build_ignore_override = if scope == Scope::Build {
            let mut output = private.create_private_entry(OsStr::new(BUILD_IGNORE_OVERRIDE))?;
            output.write_all(BUILD_IGNORE_OVERRIDE_BYTES)?;
            Some(PrivateFile::open(
                &private.path().join(BUILD_IGNORE_OVERRIDE),
                BUILD_IGNORE_OVERRIDE_BYTES.len() as u64,
            )?)
        } else {
            None
        };
        let snapshot = Snapshot {
            scope,
            private,
            context,
            files,
            digest: hex::encode(digest.finalize()),
            bytes,
            shared_writable,
            source: source_path,
            source_device,
            source_inode,
            env_file,
            build_ignore_override,
            _storage: storage,
        };
        snapshot.write_manifest()?;
        snapshot.verify()?;
        Ok(snapshot)
    }
}

#[derive(Serialize)]
struct Entry {
    path: String,
    bytes: u64,
    sha256: String,
}

/// One captured entry as it was recorded at capture time: its validated
/// relative name, its length and the SHA-256 of the captured bytes. Nothing is
/// read, and no path into the copy is given.
#[derive(Clone, Copy)]
pub(super) struct RetainedEntry<'a>(&'a Entry);

impl<'a> RetainedEntry<'a> {
    pub(super) fn name(self) -> &'a str {
        &self.0.path
    }

    pub(super) fn bytes(self) -> u64 {
        self.0.bytes
    }

    /// Lowercase hexadecimal SHA-256 of the captured bytes.
    pub(super) fn sha256(self) -> &'a str {
        &self.0.sha256
    }
}

#[derive(Serialize)]
struct Manifest<'a> {
    version: u8,
    scope: &'a str,
    source: String,
    source_device: u64,
    source_inode: u64,
    sha256: &'a str,
    files: usize,
    bytes: u64,
    shared_writable: usize,
    entries: &'a [Entry],
}

#[derive(Serialize)]
struct UnknownMarker<'a> {
    version: u8,
    written_at: String,
    control_pid: u32,
    runtime: &'a str,
    verb: &'a str,
    leader_pid: u32,
    process_group: u32,
    stop_reason: String,
    forwarded_signal: Option<String>,
    kill_sent: bool,
    leader_reaped: bool,
    wait_uncertain: bool,
    stdout_open: bool,
    stderr_open: bool,
    scope: &'a str,
    sha256: &'a str,
}

/// The prepared copy of one operator command and what identifies its bytes.
///
/// ```text
/// <root>/context/…              engine-visible tree, directories 0700, files 0600
/// <root>/compose-empty.env      Compose scope only
/// <root>/manifest.json          provenance for selected source entries only
/// <root>/podman-build-ignore    generated private engine rule, not source input
/// <root>/RETAINED-UNKNOWN.json  written only when a termination is unknown
/// ```
pub(super) struct Snapshot {
    scope: Scope,
    private: Directory,
    context: Directory,
    /// Sorted by path.
    files: Vec<Entry>,
    digest: String,
    bytes: u64,
    shared_writable: usize,
    source: PathBuf,
    source_device: u64,
    source_inode: u64,
    env_file: Option<PathBuf>,
    /// Private Podman override; outside the engine-visible context and absent
    /// from the source manifest and its digest.
    build_ignore_override: Option<PrivateFile>,
    // Declared last: the directory handles above are closed before the tree
    // is removed.
    _storage: Storage,
}

impl Snapshot {
    pub(super) fn scope(&self) -> Scope {
        self.scope
    }

    /// The private root. It is a name for messages and recovery, never an
    /// engine argument.
    pub(super) fn root(&self) -> &Path {
        self.private.path()
    }

    /// The engine-visible tree.
    pub(super) fn context(&self) -> &Path {
        self.context.path()
    }

    pub(super) fn env_file(&self) -> Option<&Path> {
        self.env_file.as_deref()
    }

    pub(super) fn build_ignore_override(&self) -> Option<&PrivateFile> {
        self.build_ignore_override.as_ref()
    }

    /// `relative` is one of the captured files.
    pub(super) fn holds(&self, relative: &str) -> bool {
        self.files
            .binary_search_by(|entry| entry.path.as_str().cmp(relative))
            .is_ok()
    }

    /// The value printed after `sha256=` in the provenance line.
    pub(super) fn digest(&self) -> &str {
        &self.digest
    }

    /// The sum of the captured lengths, printed after `bytes=` in the
    /// provenance line.
    pub(super) fn total_bytes(&self) -> u64 {
        self.bytes
    }

    /// The captured entries, sorted by name: the list the digest was computed
    /// over, in its order.
    pub(super) fn entries(&self) -> impl ExactSizeIterator<Item = RetainedEntry<'_>> + '_ {
        self.files.iter().map(RetainedEntry)
    }

    /// The bytes of the captured entries `names`, in that order, read from the
    /// copy. Refused before any read: no name or more than 64, a name given
    /// twice or not captured, a bound above the private-file bound, an entry
    /// longer than `max_bytes`. The copy is verified before the first read and
    /// after the last, and each entry is read through one private-file
    /// descriptor whose length and SHA-256 must equal the record. All or
    /// nothing: no path, file or directory of the copy is returned, the
    /// original is never reopened and nothing is written.
    pub(super) fn read_retained(&self, names: &[&str], max_bytes: u64) -> Result<Vec<Vec<u8>>> {
        const MAX_NAMES: usize = 64;
        ensure!(
            !names.is_empty() && names.len() <= MAX_NAMES,
            "a read of the prepared copy names from 1 to {MAX_NAMES} entries, not {}",
            names.len()
        );
        ensure!(
            max_bytes <= control_fs::MAX_PRIVATE_FILE_BYTES,
            "a read of the prepared copy is bounded by at most {} bytes per entry",
            control_fs::MAX_PRIVATE_FILE_BYTES
        );
        let mut named = BTreeSet::new();
        let mut wanted = Vec::with_capacity(names.len());
        for &name in names {
            let shown_name = || shown(Path::new(name));
            ensure!(
                named.insert(name),
                "entry named twice in a read of the prepared copy: {}",
                shown_name()
            );
            let index = self
                .files
                .binary_search_by(|entry| entry.path.as_str().cmp(name))
                .map_err(|_| anyhow!("not an entry of the prepared copy: {}", shown_name()))?;
            let entry = &self.files[index];
            ensure!(
                entry.bytes <= max_bytes,
                "prepared copy entry {} is longer than the bound of {max_bytes} bytes",
                entry.path
            );
            wanted.push(entry);
        }
        self.verify()?;
        let mut contents = Vec::with_capacity(wanted.len());
        for entry in wanted {
            let changed = || format!("prepared copy content changed: {}", entry.path);
            let file = PrivateFile::open(&self.context.path().join(&entry.path), entry.bytes)
                .with_context(changed)?;
            ensure!(
                file.bytes().len() as u64 == entry.bytes && file.digest() == entry.sha256,
                changed()
            );
            contents.push(file.bytes().to_vec());
        }
        self.verify()?;
        Ok(contents)
    }

    /// The digest identifies the prepared bytes. It is not an image digest.
    pub(super) fn provenance_line(&self) -> String {
        format!(
            "Prepared public distribution ({}): sha256={} files={} bytes={} shared_writable={} \
             device={} inode={} source={}",
            self.scope.label(),
            self.digest,
            self.files.len(),
            self.bytes,
            self.shared_writable,
            self.source_device,
            self.source_inode,
            shown(&self.source)
        )
    }

    fn write_manifest(&self) -> Result<()> {
        let manifest = serde_json::to_vec_pretty(&Manifest {
            version: 1,
            scope: self.scope.label(),
            source: self.source.to_string_lossy().into_owned(),
            source_device: self.source_device,
            source_inode: self.source_inode,
            sha256: &self.digest,
            files: self.files.len(),
            bytes: self.bytes,
            shared_writable: self.shared_writable,
            entries: &self.files,
        })?;
        self.private
            .create_private_entry(OsStr::new(MANIFEST))
            .and_then(|mut output| Ok(output.write_all(&manifest)?))
            .context("cannot write the prepared-source manifest")
    }

    /// The copy still is exactly what was captured: every expected file with
    /// its length and SHA-256, private directories, nothing missing and
    /// nothing extra in the engine-visible tree. The original is never
    /// reopened: once prepared, it has no say in what the engine reads.
    pub(super) fn verify(&self) -> Result<()> {
        self.private.revalidate()?;
        self.context.revalidate()?;
        let root = self.context.path();
        let expected: BTreeMap<&str, &Entry> = self
            .files
            .iter()
            .map(|entry| (entry.path.as_str(), entry))
            .collect();
        let directories: BTreeSet<&Path> = self
            .files
            .iter()
            .flat_map(|entry| Path::new(&entry.path).ancestors().skip(1))
            .filter(|ancestor| !ancestor.as_os_str().is_empty())
            .collect();
        let mut seen = BTreeSet::new();
        // An unexpected entry ends the walk at once, so the walk never visits
        // more than the expected entries and one more.
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .max_depth(MAX_DEPTH + 1)
            .max_open(MAX_OPEN_DIRECTORIES)
        {
            let entry = entry
                .map_err(|error| walk_failed("prepared copy entry is unavailable", root, error))?;
            let relative = entry
                .path()
                .strip_prefix(root)
                .context("prepared copy enumeration left its directory")?;
            if relative.as_os_str().is_empty() {
                continue;
            }
            // Not a name this CLI wrote: whoever planted the entry chose it.
            let unexpected =
                || format!("unexpected entry in the prepared copy: {}", shown(relative));
            if entry.file_type().is_dir() {
                ensure!(directories.contains(relative), unexpected());
                Directory::open_existing(entry.path()).with_context(|| {
                    format!("prepared copy directory changed: {}", relative.display())
                })?;
                continue;
            }
            let name = relative.to_str().with_context(unexpected)?;
            let wanted = expected.get(name).with_context(unexpected)?;
            let changed = || format!("prepared copy content changed: {name}");
            let file = PrivateFile::open(entry.path(), wanted.bytes).with_context(changed)?;
            ensure!(
                file.bytes().len() as u64 == wanted.bytes && file.digest() == wanted.sha256,
                changed()
            );
            seen.insert(name.to_owned());
        }
        if let Some(missing) = self
            .files
            .iter()
            .find(|entry| !seen.contains(entry.path.as_str()))
        {
            bail!("prepared copy entry disappeared: {}", missing.path);
        }
        if let Some(env_file) = &self.env_file {
            let file = PrivateFile::open(env_file, 0)
                .context("prepared Compose environment file changed")?;
            ensure!(
                file.bytes().is_empty(),
                "prepared Compose environment file changed"
            );
        }
        if self.scope == Scope::Build {
            let expected = BUILD_IGNORE_OVERRIDE_BYTES;
            let file = self
                .build_ignore_override
                .as_ref()
                .context("prepared Podman ignore file is missing")?;
            file.revalidate()
                .context("prepared Podman ignore file changed")?;
            ensure!(
                file.bytes() == expected,
                "prepared Podman ignore file changed"
            );
            let reopened = PrivateFile::open(
                &self.private.path().join(BUILD_IGNORE_OVERRIDE),
                expected.len() as u64,
            )
            .context("prepared Podman ignore file changed")?;
            ensure!(
                reopened.bytes() == expected,
                "prepared Podman ignore file changed"
            );
        } else {
            ensure!(
                self.build_ignore_override.is_none(),
                "unexpected prepared Podman ignore file"
            );
        }
        self.context.revalidate()
    }

    /// Records, beside the copy, why it stays. Nothing is released here, and a
    /// marker that cannot be written changes nothing about retention.
    pub(super) fn retain_unknown(&self, verb: &str, end: &UnknownEnd) -> PathBuf {
        let marker = UnknownMarker {
            version: 1,
            written_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            control_pid: std::process::id(),
            runtime: end.runtime.program(),
            verb,
            leader_pid: end.leader_pid,
            process_group: end.process_group,
            stop_reason: match end.stop.reason {
                StopReason::Cancelled => "cancelled".to_owned(),
                StopReason::Interrupted { signal } => {
                    format!("interrupted by {}", signal_name(signal))
                }
                StopReason::TimedOut => "bound elapsed".to_owned(),
                StopReason::OutputUnavailable => "output could not be read".to_owned(),
                StopReason::WaitUnavailable => "wait status unavailable".to_owned(),
            },
            forwarded_signal: end
                .stop
                .forwarded
                .map(|signal| signal_name(signal).into_owned()),
            kill_sent: end.stop.kill_sent,
            leader_reaped: end.leader_reaped,
            wait_uncertain: end.wait_uncertain,
            stdout_open: end.stdout_open,
            stderr_open: end.stderr_open,
            scope: self.scope.label(),
            sha256: &self.digest,
        };
        let written = serde_json::to_vec_pretty(&marker)
            .map_err(anyhow::Error::new)
            .and_then(|bytes| {
                let mut output = self
                    .private
                    .create_private_entry(OsStr::new(UNKNOWN_MARKER))?;
                Ok(output.write_all(&bytes)?)
            });
        if let Err(error) = written {
            eprintln!(
                "warning: {UNKNOWN_MARKER} could not be written in {}: {error:#}",
                shown(self.root())
            );
        }
        self.root().to_owned()
    }
}
