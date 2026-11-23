//! Descriptor-relative control files on supported local Unix filesystems.
//!
//! The namespace is trusted through ownership, modes and the platform ACL
//! policy, including every ancestor. Retained FDs and revalidation detect
//! accidental replacement; they do not isolate the control account from its
//! own UID, root, storage administrators or engine administrators. FDs are
//! process-local capabilities and must never be serialized.
#![cfg(unix)]

// The non-test expansion is exactly the supplied body. The test expansion
// executes that same body once and returns its original result unchanged.
macro_rules! measure_control_fs {
    ($operation:ident, $path:expr, $bytes:expr, $body:block) => {{
        #[cfg(test)]
        {
            crate::control_fs::cost_probe::measure(
                crate::control_fs::cost_probe::Operation::$operation,
                $path,
                || $body,
                $bytes,
            )
        }
        #[cfg(not(test))]
        $body
    }};
}

#[cfg(test)]
pub(crate) mod cost_probe;
pub mod operation;
pub(crate) mod persistent;
pub(crate) mod publication;
mod source_owner;
pub(crate) use source_owner::InitialParentObservationFailure;

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    ffi::{CString, OsStr, OsString},
    fmt,
    fs::{File, Metadata, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt, fs::OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
    sync::Arc,
};

/// Absolute ceiling in addition to each caller's smaller input bound.
pub const MAX_PRIVATE_FILE_BYTES: u64 = 32 * 1024 * 1024;
const DIRECTORY_FLAGS: libc::c_int =
    libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
const FILE_FLAGS: libc::c_int = libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;

/// Normalize a caller's path without resolving arbitrary filesystem symlinks.
/// Parent traversal is refused. On macOS only the fixed OS aliases /var, /tmp
/// and /etc are mapped to /private; every other symlink remains for walk to deny.
pub fn absolute_path(path: &Path) -> Result<PathBuf> {
    let joined = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .context("control current directory is unavailable")?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(name) => normalized.push(name),
            Component::CurDir => {}
            _ => anyhow::bail!("control path must not contain parent traversal"),
        }
    }
    #[cfg(target_os = "macos")]
    for alias in ["var", "tmp", "etc"] {
        let prefix = Path::new("/").join(alias);
        if let Ok(suffix) = normalized.strip_prefix(&prefix) {
            normalized = Path::new("/private").join(alias).join(suffix);
            break;
        }
    }
    validate_absolute(&normalized)?;
    Ok(normalized)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
}

/// The identity of a checked directory entry, without opening its inode.
/// SQLite owns all descriptors to its main file and shared-memory file: closing
/// a second raw descriptor could discard another connection's POSIX locks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PrivateEntryIdentity {
    device: u64,
    inode: u64,
}

impl Identity {
    fn of(facts: &Metadata) -> Self {
        Self {
            device: facts.dev(),
            inode: facts.ino(),
        }
    }
}

struct Anchor {
    file: File,
    path: PathBuf,
    chain: Vec<Identity>,
    private: bool,
}

impl Anchor {
    fn revalidate(&self) -> Result<()> {
        measure_control_fs!(DirectoryRevalidate, &self.path, |_| 0, {
            let current = walk(&self.path, false, self.private)?;
            ensure!(
                current.chain == self.chain,
                "control directory path identity changed; reopen the intended private configuration"
            );
            Ok(())
        })
    }
}

/// A retained private directory, owned by the effective UID with mode 0700.
/// Existing paths are inspected, never repaired, moved or removed.
#[derive(Clone)]
pub struct Directory(Arc<Anchor>);

impl fmt::Debug for Directory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Directory(<private control handle>)")
    }
}

impl Directory {
    pub fn open_existing(path: &Path) -> Result<Self> {
        Ok(Self(Arc::new(walk(path, false, true)?)))
    }

    /// Create missing components privately, without altering existing entries.
    pub fn open_or_create(path: &Path) -> Result<Self> {
        Ok(Self(Arc::new(walk(path, true, true)?)))
    }

    /// Open or create a single private child using the retained directory FD.
    pub fn child(&self, name: impl AsRef<OsStr>) -> Result<Self> {
        self.open_child(name.as_ref(), true)
    }

    /// Open a single existing private child using the retained directory FD.
    pub(crate) fn open_existing_child(&self, name: impl AsRef<OsStr>) -> Result<Self> {
        self.open_child(name.as_ref(), false)
    }

    fn open_child(&self, name: &OsStr, create: bool) -> Result<Self> {
        self.revalidate()?;
        let encoded = child_name(name)?;
        let file = open_child_directory(&self.0.file, &encoded, create)?;
        let facts = check_directory(&file, true)?;
        let mut chain = self.0.chain.clone();
        chain.push(Identity::of(&facts));
        let anchor = Anchor {
            file,
            path: self.0.path.join(name),
            chain,
            private: true,
        };
        anchor.revalidate()?;
        Ok(Self(Arc::new(anchor)))
    }

    /// Provenance only. Revalidate immediately before handing a name to a CLI.
    pub fn path(&self) -> &Path {
        &self.0.path
    }

    pub fn revalidate(&self) -> Result<()> {
        self.0.revalidate()
    }

    /// Inspect a private regular entry without opening or closing the file.
    /// The retained private parent establishes the other-UID boundary while
    /// pathname ACL inspection is performed; same-UID/root mutation is outside
    /// that boundary. Absence is returned only for ENOENT, never for errors.
    pub(crate) fn private_entry(&self, name: &OsStr) -> Result<Option<PrivateEntryIdentity>> {
        self.revalidate()?;
        self.private_entry_after_parent_revalidation(name)
    }

    /// Entry inspection within a synchronous compound validator whose
    /// immediately preceding operation freshly revalidated this same parent.
    /// This helper does not validate the path and must not retain or reuse a
    /// successful parent check for a later operation, after a wait or failure.
    fn private_entry_after_parent_revalidation(
        &self,
        name: &OsStr,
    ) -> Result<Option<PrivateEntryIdentity>> {
        let encoded = child_name(name)?;
        let inspect = || -> Result<Option<PrivateEntryIdentity>> {
            let Some(facts) = named_entry_metadata(&self.0.file, &encoded)
                .context("private database entry metadata unavailable")?
            else {
                return Ok(None);
            };
            ensure!(
                is_private_regular_entry(&facts),
                "private database entry requires a regular single-link file, effective-UID ownership and mode 0600; existing data is never repaired"
            );
            #[allow(clippy::unnecessary_cast)]
            Ok(Some(PrivateEntryIdentity {
                device: facts.st_dev as u64,
                inode: facts.st_ino as u64,
            }))
        };
        let before = inspect()?;
        if before.is_some() {
            check_named_private_acl(&self.0.path.join(name))?;
            ensure!(
                inspect()? == before,
                "private database entry identity changed during inspection"
            );
        }
        Ok(before)
    }

    /// Create a new private entry exclusively. Only use for a new SQLite main
    /// inode before any VFS connection to that inode can exist.
    pub(crate) fn create_private_entry(&self, name: &OsStr) -> Result<File> {
        self.revalidate()?;
        let name = child_name(name)?;
        // SAFETY: retained directory FD and single C component; exclusive create.
        let fd = unsafe {
            libc::openat(
                self.0.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | FILE_FLAGS,
                0o600,
            )
        };
        ensure!(
            fd >= 0,
            "create new private database entry: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: successful openat transfers a new owned descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        check_private_file(&file)?;
        Ok(file)
    }

    /// Reopen an existing private entry for reading and appending, never
    /// following a link. Absence is `None` only for ENOENT; an entry that is
    /// not a private regular single-link file is refused, never repaired.
    pub(crate) fn open_private_entry_rw(&self, name: &OsStr) -> Result<Option<File>> {
        if self.private_entry(name)?.is_none() {
            return Ok(None);
        }
        let encoded = child_name(name)?;
        // SAFETY: retained directory FD and one C component; no creation.
        let fd = unsafe {
            libc::openat(
                self.0.file.as_raw_fd(),
                encoded.as_ptr(),
                libc::O_RDWR | FILE_FLAGS,
            )
        };
        ensure!(
            fd >= 0,
            "private entry cannot be reopened without following a link: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: successful openat transfers a new owned descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        check_private_file(&file)?;
        Ok(Some(file))
    }

    /// Lock the exact private entry through its retained writable descriptor.
    /// The file description keeps the lock until its last descriptor closes.
    pub(crate) fn try_lock_private_entry(&self, name: &OsStr, file: &File) -> Result<bool> {
        let facts = check_private_file(file)?;
        let identity = PrivateEntryIdentity {
            device: facts.dev(),
            inode: facts.ino(),
        };
        ensure!(
            self.private_entry(name)? == Some(identity),
            "private writer entry identity changed before lock"
        );
        let fd = file.as_raw_fd();
        // SAFETY: inspect flags on the retained descriptor; neither call changes them.
        let (status, flags) = unsafe {
            (
                libc::fcntl(fd, libc::F_GETFL),
                libc::fcntl(fd, libc::F_GETFD),
            )
        };
        ensure!(
            status >= 0
                && status & libc::O_ACCMODE == libc::O_RDWR
                && flags >= 0
                && flags & libc::FD_CLOEXEC != 0,
            "private writer requires a retained O_RDWR CLOEXEC descriptor"
        );
        // SAFETY: retained private FD; nonblocking whole-file advisory lock.
        if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(false);
            }
            return Err(error).context("private writer lock acquisition failed");
        }
        let after = check_private_file(file)?;
        ensure!(
            Identity::of(&after) == Identity::of(&facts)
                && self.private_entry(name)? == Some(identity),
            "private writer entry identity changed after lock"
        );
        Ok(true)
    }

    /// Open the separately reserved SQLite initialization lock, never a main
    /// or sidecar inode. Its lifetime does not interfere with VFS POSIX locks.
    pub(crate) fn sqlite_initialization_lock(&self) -> Result<File> {
        let name = OsStr::new(".ai-guard-sqlite-open.lock");
        if self.private_entry(name)?.is_none() {
            match self.create_private_entry(name) {
                Ok(file) => return Ok(file),
                Err(error) if self.private_entry(name)?.is_none() => return Err(error),
                Err(_) => {}
            }
        }
        let file = open_file(&self.0.file, &child_name(name)?)?;
        check_private_file(&file)?;
        Ok(file)
    }

    pub(crate) fn sync(&self) -> Result<()> {
        measure_control_fs!(DirectorySync, self.path(), |_| 0, {
            self.revalidate()?;
            measure_control_fs!(DirectorySyncAll, self.path(), |_| 0, {
                self.0.file.sync_all()
            })
            .context("private directory synchronization failed")
        })
    }

    /// Clone the already validated descriptor for other trusted control code.
    pub(crate) fn try_clone_file(&self) -> Result<File> {
        self.revalidate()?;
        self.0
            .file
            .try_clone()
            .context("retained control directory descriptor could not be cloned")
    }

    /// Open an existing private regular child for a streaming read whose size
    /// the caller bounds itself. The metadata is the identity before reading;
    /// compare it with the file metadata afterwards to detect a change.
    pub(crate) fn open_private_stream(&self, name: impl AsRef<OsStr>) -> Result<(File, Metadata)> {
        self.revalidate()?;
        let encoded = child_name(name.as_ref())?;
        let file = open_file(&self.0.file, &encoded)?;
        let facts = check_private_file(&file)?;
        Ok((file, facts))
    }

    pub fn read_private_file(
        &self,
        name: impl AsRef<OsStr>,
        max_bytes: u64,
    ) -> Result<PrivateFile> {
        self.revalidate()?;
        read_private(self.0.clone(), name.as_ref(), max_bytes)
    }

    /// Prepare exact bytes under this directory, independently of ambient TMPDIR.
    /// Only this new file is eligible for identity-checked cleanup.
    pub fn snapshot(&self, bytes: &[u8]) -> Result<Snapshot> {
        ensure!(
            bytes.len() as u64 <= MAX_PRIVATE_FILE_BYTES,
            "private snapshot exceeds the supported size limit"
        );
        self.revalidate()?;
        let name = OsString::from(format!("snapshot-{}", uuid::Uuid::new_v4()));
        let encoded = child_name(&name)?;
        // SAFETY: parent FD is retained; the name is one NUL-terminated component.
        // O_EXCL creates a new entry and never opens or truncates existing data.
        let fd = unsafe {
            libc::openat(
                self.0.file.as_raw_fd(),
                encoded.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | FILE_FLAGS,
                0o600,
            )
        };
        ensure!(
            fd >= 0,
            "private snapshot creation failed: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: a successful openat returned a uniquely owned descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        let facts = file
            .metadata()
            .context("private snapshot metadata unavailable")?;
        let mut inner = FileInner {
            path: self.0.path.join(&name),
            parent: self.0.clone(),
            name,
            file,
            facts,
            bytes: Vec::new(),
            digest: String::new(),
            remove_on_drop: true,
        };
        check_private_file(&inner.file)?;
        inner
            .file
            .write_all(bytes)
            .context("private snapshot write failed")?;
        inner
            .file
            .sync_all()
            .context("private snapshot synchronization failed")?;
        inner.facts = check_private_file(&inner.file)?;
        inner.bytes = bytes.to_vec();
        inner.digest = hex::encode(Sha256::digest(bytes));
        let snapshot = Snapshot(PrivateFile(Arc::new(inner)));
        snapshot.revalidate()?;
        Ok(snapshot)
    }
}

struct FileInner {
    parent: Arc<Anchor>,
    name: OsString,
    path: PathBuf,
    file: File,
    facts: Metadata,
    bytes: Vec<u8>,
    digest: String,
    remove_on_drop: bool,
}

impl Drop for FileInner {
    fn drop(&mut self) {
        if self.remove_on_drop {
            // Best effort only. Failure leaves a residual snapshot for diagnosis.
            // The retained FD selects the original parent even after its rename.
            unlink_created(&self.parent.file, &self.name, Identity::of(&self.facts));
        }
    }
}

/// Exact bytes read from a checked 0600/euid, regular, single-link file FD.
/// Clones share the FD and, for snapshots, the final-handle cleanup lifetime.
#[derive(Clone)]
pub struct PrivateFile(Arc<FileInner>);

impl fmt::Debug for PrivateFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PrivateFile(<private control handle>)")
    }
}

impl PrivateFile {
    pub fn open(path: &Path, max_bytes: u64) -> Result<Self> {
        validate_absolute(path)?;
        let parent = path
            .parent()
            .context("private file requires a parent directory")?;
        let name = path
            .file_name()
            .context("private file requires a filename")?;
        let anchor = Arc::new(walk(parent, false, false)?);
        read_private(anchor, name, max_bytes)
    }

    pub fn bytes(&self) -> &[u8] {
        &self.0.bytes
    }

    /// SHA-256 of the prepared bytes, never a reopening of the pathname.
    pub fn digest(&self) -> &str {
        &self.0.digest
    }

    /// Provenance only; `revalidate` is required before a pathname handoff.
    pub fn path(&self) -> &Path {
        &self.0.path
    }

    /// Compare filesystem identities, including case/Unicode aliases of a
    /// project path. The project itself need not satisfy the control DAC policy.
    pub(crate) fn is_within_directory(&self, path: &Path) -> Result<bool> {
        self.revalidate()?;
        validate_absolute(path)?;
        let mut directory = OpenOptions::new()
            .read(true)
            .custom_flags(DIRECTORY_FLAGS)
            .open("/")
            .context("project filesystem root cannot be inspected")?;
        for component in path.components() {
            if let Component::Normal(name) = component {
                directory = open_child_directory(&directory, &child_name(name)?, false)?;
            }
        }
        let identity = Identity::of(
            &directory
                .metadata()
                .context("project directory identity is unavailable")?,
        );
        Ok(self.0.parent.chain.contains(&identity))
    }

    pub fn revalidate(&self) -> Result<()> {
        self.0.parent.revalidate()?;
        let held = check_private_file(&self.0.file)?;
        ensure!(
            same_file_state(&held, &self.0.facts),
            "private file changed after preparation"
        );
        let encoded = child_name(&self.0.name)?;
        let current = open_file(&self.0.parent.file, &encoded)?;
        let current_facts = check_private_file(&current)?;
        ensure!(
            same_file_state(&current_facts, &self.0.facts),
            "private file pathname no longer identifies the prepared file"
        );
        Ok(())
    }
}

/// An ephemeral private file whose bytes and SHA-256 are fixed at preparation.
#[derive(Clone)]
pub struct Snapshot(PrivateFile);

impl fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Snapshot(<private control handle>)")
    }
}

impl Snapshot {
    pub fn path(&self) -> &Path {
        self.0.path()
    }

    pub fn digest(&self) -> &str {
        self.0.digest()
    }

    pub fn revalidate(&self) -> Result<()> {
        self.0.revalidate()
    }

    pub fn private_file(&self) -> PrivateFile {
        self.0.clone()
    }
}

fn validate_absolute(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "control path must be absolute");
    let bytes = path.as_os_str().as_bytes();
    ensure!(
        !bytes.contains(&0)
            && (bytes == b"/"
                || !bytes
                    .split(|&byte| byte == b'/')
                    .skip(1)
                    .any(|part| part.is_empty() || part == b"." || part == b"..")),
        "control path must have canonical components without links or traversal"
    );
    Ok(())
}

fn child_name(name: &OsStr) -> Result<CString> {
    let bytes = name.as_bytes();
    ensure!(
        !bytes.is_empty() && bytes != b"." && bytes != b".." && !bytes.contains(&b'/'),
        "control child name must be a single non-traversing component"
    );
    CString::new(bytes).context("control child name contains NUL")
}

/// Observe only DAC metadata for the named entry relative to a checked parent.
/// Absence means the parent exists and the final entry does not. The private
/// credential handoff still checks ACLs and validates the contents separately.
pub(crate) fn private_file_metadata_present(path: &Path) -> Result<bool> {
    validate_absolute(path)?;
    let parent = walk(
        path.parent()
            .context("private file requires a parent directory")?,
        false,
        false,
    )?;
    let name = child_name(
        path.file_name()
            .context("private file requires a filename")?,
    )?;
    let facts = named_entry_metadata(&parent.file, &name)?;
    parent.revalidate()?;
    if let Some(facts) = &facts {
        ensure!(
            is_private_regular_entry(facts),
            "private file metadata is unsafe"
        );
    }
    Ok(facts.is_some())
}

fn is_private_regular_entry(facts: &libc::stat) -> bool {
    facts.st_mode & libc::S_IFMT == libc::S_IFREG
        && facts.st_uid == effective_uid()
        && facts.st_mode & 0o7777 == 0o600
        && facts.st_nlink == 1
}

fn named_entry_metadata(file: &File, name: &CString) -> std::io::Result<Option<libc::stat>> {
    let mut facts = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: retained directory FD, one C component and writable stat.
    if unsafe {
        libc::fstatat(
            file.as_raw_fd(),
            name.as_ptr(),
            facts.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } == 0
    {
        // SAFETY: successful fstatat initialized the stat structure.
        return Ok(Some(unsafe { facts.assume_init() }));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ENOENT) {
        Ok(None)
    } else {
        Err(error)
    }
}

fn walk(path: &Path, create: bool, private: bool) -> Result<Anchor> {
    validate_absolute(path)?;
    let names: Vec<&OsStr> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(DIRECTORY_FLAGS)
        .open("/")
        .context("control filesystem root cannot be opened")?;
    let facts = check_directory(&file, names.is_empty() && private)?;
    let mut chain = vec![Identity::of(&facts)];
    for (index, name) in names.iter().enumerate() {
        let encoded = child_name(name)?;
        let next = open_child_directory(&file, &encoded, create)?;
        let facts = check_directory(&next, private && index + 1 == names.len())?;
        // The child's root/control ownership and non-mutable policy is checked
        // even when its parent is the explicitly allowed root-owned sticky dir.
        chain.push(Identity::of(&facts));
        file = next;
    }
    let anchor = Anchor {
        file,
        path: path.to_owned(),
        chain,
        private,
    };
    Ok(anchor)
}

fn open_child_directory(parent: &File, name: &CString, create: bool) -> Result<File> {
    // SAFETY: parent owns a valid FD and name is a single C string component.
    let mut fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | DIRECTORY_FLAGS,
        )
    };
    if fd < 0 && create && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
        // SAFETY: mkdirat is relative to the retained parent; mode cannot expose
        // a new directory through ordinary group/other DAC permissions.
        let created = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
        ensure!(
            created == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EEXIST),
            "private control directory creation failed: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: same retained parent/name; O_NOFOLLOW also covers EEXIST races.
        fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | DIRECTORY_FLAGS,
            )
        };
    }
    if fd < 0 {
        return Err(std::io::Error::last_os_error())
            .context("control directory must exist without symbolic links");
    }
    // SAFETY: a successful openat returned a uniquely owned FD.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no pointers or preconditions.
    unsafe { libc::geteuid() }
}

/// A host utility in a namespace protected from unrelated local writers.
/// Revalidation detects replacement; root and the control UID remain trusted.
/// This is not a runtime invocation approval, worker capability or audit receipt.
pub struct TrustedExecutable {
    parent: Directory,
    name: OsString,
    file: File,
    facts: Metadata,
    path: PathBuf,
}

impl TrustedExecutable {
    pub fn open(path: &Path) -> Result<Self> {
        ensure!(
            path.is_absolute(),
            "trusted executable needs an absolute path"
        );
        let path = absolute_path(path)?;
        let name = path
            .file_name()
            .context("trusted executable needs a basename")?
            .to_owned();
        let parent = Directory(Arc::new(walk(
            path.parent().context("trusted executable needs a parent")?,
            false,
            false,
        )?));
        let file = open_file(&parent.0.file, &child_name(&name)?)?;
        let facts = check_executable(&file)?;
        let value = Self {
            parent,
            name,
            file,
            facts,
            path,
        };
        value.revalidate()?;
        Ok(value)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn revalidate(&self) -> Result<()> {
        self.parent.revalidate()?;
        let current = open_file(&self.parent.0.file, &child_name(&self.name)?)?;
        ensure!(
            same_file_state(&self.facts, &check_executable(&self.file)?)
                && same_file_state(&self.facts, &check_executable(&current)?),
            "trusted executable identity, content metadata or permissions changed"
        );
        Ok(())
    }
}

fn check_executable(file: &File) -> Result<Metadata> {
    let facts = file
        .metadata()
        .context("trusted executable metadata unavailable")?;
    ensure!(
        facts.is_file()
            && (facts.uid() == 0 || facts.uid() == effective_uid())
            && facts.mode() & 0o022 == 0
            && facts.mode() & 0o111 != 0
            && facts.mode() & 0o6000 == 0,
        "trusted executable needs root/control ownership, execute permission and no group/other writes or set-ID"
    );
    validate_filesystem_permissions(file)?;
    check_acl(file, false)?;
    Ok(facts)
}

/// Check an export namespace component without imposing private-control modes
/// or changing existing permissions. Callers retain and validate the complete
/// ancestry; this does not authorize a new pathname or distrust source content.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn check_export_directory(file: &File) -> Result<()> {
    check_directory(file, false).map(|_| ()).context(
        "unsafe export parent or ancestor; choose a root/effective-UID-owned path without group/other modification and with supported filesystem/ACL permissions; existing paths are never repaired",
    )
}

fn check_directory(file: &File, private: bool) -> Result<Metadata> {
    let facts = file
        .metadata()
        .context("control directory metadata unavailable")?;
    ensure!(facts.is_dir(), "control path component must be a directory");
    validate_filesystem_permissions(file)?;
    if private {
        ensure!(
            facts.uid() == effective_uid() && facts.mode() & 0o7777 == 0o700,
            "private control directory requires effective-UID ownership and mode 0700; use a new private destination, existing paths are never repaired"
        );
    } else {
        ensure!(
            facts.uid() == 0 || facts.uid() == effective_uid(),
            "control ancestor must belong to root or the effective UID"
        );
        let sticky_root = facts.uid() == 0 && facts.mode() & 0o1000 != 0;
        ensure!(
            facts.mode() & 0o022 == 0 || sticky_root,
            "control ancestor permits group/other modification; choose a private path outside the mutable directory"
        );
    }
    check_acl(file, private)?;
    Ok(facts)
}

fn check_private_file(file: &File) -> Result<Metadata> {
    let facts = file
        .metadata()
        .context("private file metadata unavailable")?;
    ensure!(
        facts.is_file()
            && facts.uid() == effective_uid()
            && facts.mode() & 0o7777 == 0o600
            && facts.nlink() == 1,
        "private control file requires a regular single-link file, effective-UID ownership and mode 0600; existing data is never repaired"
    );
    validate_filesystem_permissions(file)?;
    check_acl(file, true)?;
    Ok(facts)
}

fn open_file(parent: &File, name: &CString) -> Result<File> {
    // SAFETY: valid retained directory FD and one NUL-terminated component.
    // NONBLOCK ensures a FIFO cannot block before fstat rejects its type.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | FILE_FLAGS,
        )
    };
    ensure!(
        fd >= 0,
        "private file cannot be opened without following a link: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: new owned descriptor transferred exactly once.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn read_private(parent: Arc<Anchor>, name: &OsStr, max_bytes: u64) -> Result<PrivateFile> {
    ensure!(
        max_bytes <= MAX_PRIVATE_FILE_BYTES,
        "private file bound exceeds the supported size limit"
    );
    let encoded = child_name(name)?;
    let mut file = open_file(&parent.file, &encoded)?;
    let before = check_private_file(&file)?;
    ensure!(
        before.len() <= max_bytes,
        "private file exceeds the configured size limit"
    );
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .context("private file bounded read failed")?;
    ensure!(
        bytes.len() as u64 <= max_bytes,
        "private file grew beyond the configured size limit"
    );
    let facts = check_private_file(&file)?;
    ensure!(
        same_file_state(&before, &facts),
        "private file changed during its bounded read"
    );
    let digest = hex::encode(Sha256::digest(&bytes));
    let private_file = PrivateFile(Arc::new(FileInner {
        path: parent.path.join(name),
        parent,
        name: name.to_owned(),
        file,
        facts,
        bytes,
        digest,
        remove_on_drop: false,
    }));
    private_file.revalidate()?;
    Ok(private_file)
}

fn same_file_state(left: &Metadata, right: &Metadata) -> bool {
    Identity::of(left) == Identity::of(right)
        && left.uid() == right.uid()
        && left.gid() == right.gid()
        && left.mode() == right.mode()
        && left.nlink() == right.nlink()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

/// Read an explicitly projected TLS file without weakening private-file rules.
/// The namespace is protected from unrelated UIDs; root and the control UID
/// remain trusted. Returned bytes are a snapshot, not an immutable pathname.
pub(crate) fn read_trusted_projected_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    ensure!(
        max_bytes <= MAX_PRIVATE_FILE_BYTES,
        "projected file limit invalid"
    );
    let path = absolute_path(path)?;
    let parent = walk(
        path.parent().context("projected file needs a parent")?,
        false,
        false,
    )?;
    let name = child_name(path.file_name().context("projected file needs a name")?)?;
    let check = |file: &File| -> Result<Metadata> {
        let facts = file.metadata()?;
        ensure!(
            facts.is_file()
                && facts.nlink() == 1
                && (facts.uid() == 0 || facts.uid() == effective_uid())
                && facts.mode() & 0o7022 == 0,
            "projected file requires a trusted regular single-link, non-writable projection"
        );
        validate_filesystem_permissions(file)?;
        check_acl(file, false)?;
        Ok(facts)
    };
    let mut file = open_file(&parent.file, &name)?;
    let before = check(&file)?;
    #[cfg(test)]
    projected_tests::after_open(&path);
    ensure!(before.len() <= max_bytes, "projected file exceeds limit");
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= max_bytes,
        "projected file grew beyond limit"
    );
    parent.revalidate()?;
    let named = open_file(&parent.file, &name)?;
    ensure!(
        same_file_state(&before, &check(&file)?) && same_file_state(&before, &check(&named)?),
        "projected file identity or content metadata changed"
    );
    parent.revalidate()?;
    Ok(bytes)
}

#[cfg(test)]
mod projected_tests;

fn unlink_created(parent: &File, name: &OsStr, identity: Identity) {
    let Ok(name) = child_name(name) else {
        return;
    };
    let mut facts = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: valid FD, C name and output pointer. NOFOLLOW identifies the entry
    // itself; a replacement link is never followed or removed.
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            facts.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return;
    }
    // SAFETY: fstatat initialized facts on success.
    let facts = unsafe { facts.assume_init() };
    // Native stat widths differ between Darwin/Linux and 32/64-bit ABIs.
    #[allow(clippy::unnecessary_cast)]
    let current = Identity {
        device: facts.st_dev as u64,
        inode: facts.st_ino as u64,
    };
    if current == identity && facts.st_mode & libc::S_IFMT == libc::S_IFREG && facts.st_nlink == 1 {
        // SAFETY: only this created identity is eligible. Trusted-parent DAC
        // excludes another UID changing the entry between fstatat and unlinkat;
        // concurrent same-UID/root mutation is explicitly outside this guarantee.
        unsafe {
            libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0);
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn validate_filesystem_permissions(file: &File) -> Result<()> {
    let mut facts = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: valid FD and writable statfs output.
    ensure!(
        unsafe { libc::fstatfs(file.as_raw_fd(), facts.as_mut_ptr()) } == 0,
        "control filesystem inspection failed"
    );
    // SAFETY: successful fstatfs initialized facts and the fixed type string.
    let facts = unsafe { facts.assume_init() };
    let kind = unsafe { std::ffi::CStr::from_ptr(facts.f_fstypename.as_ptr()) }.to_bytes();
    check_darwin_filesystem(kind, facts.f_flags)
}

#[cfg(target_os = "macos")]
fn check_darwin_filesystem(kind: &[u8], flags: u32) -> Result<()> {
    ensure!(
        flags & libc::MNT_LOCAL as u32 != 0 && matches!(kind, b"apfs" | b"hfs"),
        "control filesystem type is outside the supported local APFS/HFS policy; choose a supported private destination"
    );
    // Darwin SDK sys/mount.h defines MNT_IGNORE_OWNERSHIP (alias
    // MNT_UNKNOWNPERMISSIONS) as 0x00200000. Ownership/mode observations cannot
    // establish the DAC boundary when the mount ignores object ownership.
    ensure!(
        flags & 0x00200000 == 0,
        "control/runtime filesystem ignores ownership permissions; enable ownership on a supported volume or use a new private destination"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn validate_filesystem_permissions(file: &File) -> Result<()> {
    let mut facts = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: valid FD and writable statfs output.
    ensure!(
        unsafe { libc::fstatfs(file.as_raw_fd(), facts.as_mut_ptr()) } == 0,
        "control filesystem inspection failed"
    );
    // SAFETY: successful fstatfs initialized facts.
    let kind = unsafe { facts.assume_init() }.f_type as u64;
    // Supported local POSIX-ACL semantics: ext*, XFS, Btrfs, tmpfs, overlayfs,
    // ramfs and F2FS. This policy allowlist is not an executed qualification.
    ensure!(
        matches!(
            kind,
            0xef53 | 0x58465342 | 0x9123683e | 0x01021994 | 0x794c7630 | 0x858458f6 | 0xf2f52010
        ),
        "control filesystem type is outside the supported local POSIX-ACL policy; network/FUSE ACL semantics require separate support"
    );
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn validate_filesystem_permissions(_file: &File) -> Result<()> {
    anyhow::bail!("control files require a supported local Linux or macOS filesystem")
}

#[cfg(target_os = "linux")]
fn check_acl(_file: &File, _private: bool) -> Result<()> {
    // On the supported local POSIX-ACL filesystems, the mode group bits expose
    // ACL_MASK. 0700/0600 exclude other principals; mode022=0 excludes ancestor
    // writes through named entries as well as ordinary group/other permissions.
    Ok(())
}

#[cfg(target_os = "linux")]
fn check_named_private_acl(_path: &Path) -> Result<()> {
    // Directory validation already restricted the filesystem to supported
    // local POSIX-ACL semantics. fstatat verified mode0600, including ACL_MASK.
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn check_named_private_acl(_path: &Path) -> Result<()> {
    anyhow::bail!("private database ACL inspection is unsupported on this platform")
}

#[cfg(target_os = "macos")]
fn check_named_private_acl(path: &Path) -> Result<()> {
    use std::ffi::c_void;
    unsafe extern "C" {
        fn acl_get_link_np(path: *const libc::c_char, kind: libc::c_int) -> *mut c_void;
        fn acl_get_entry(
            acl: *mut c_void,
            index: libc::c_int,
            entry: *mut *mut c_void,
        ) -> libc::c_int;
        fn acl_free(value: *mut c_void) -> libc::c_int;
    }
    let path = CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: absolute C path under the retained/revalidated private parent;
    // Darwin's link variant does not follow the final pathname component.
    let acl = unsafe { acl_get_link_np(path.as_ptr(), 0x100) };
    if acl.is_null() {
        let error = std::io::Error::last_os_error();
        ensure!(
            error.raw_os_error() == Some(libc::ENOENT),
            "private database ACL inspection failed: {error}"
        );
        return Ok(());
    }
    let mut entry = std::ptr::null_mut();
    // SAFETY: live allocated ACL and writable entry output. Only an empty ACL
    // is qualified for a private file, matching the FD-based control policy.
    let status = unsafe { acl_get_entry(acl, 0, &mut entry) };
    let error = std::io::Error::last_os_error();
    // SAFETY: release exactly this ACL allocation, not the database descriptor.
    unsafe { acl_free(acl) };
    ensure!(
        status == -1 && error.raw_os_error() == Some(libc::EINVAL),
        "private database entry has an extended ACL or ACL inspection failed; copy into a new private destination, existing ACLs are never removed"
    );
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn check_acl(_file: &File, _private: bool) -> Result<()> {
    anyhow::bail!("control ACL inspection is unsupported on this platform")
}

#[cfg(target_os = "macos")]
fn check_acl(file: &File, private: bool) -> Result<()> {
    use std::ffi::c_void;
    // Darwin sys/acl.h: ACL_TYPE_EXTENDED=0x100, FIRST_ENTRY=0, NEXT_ENTRY=-1,
    // ALLOW=1, DENY=2. acl_get_entry returns -1/EINVAL at iteration end.
    unsafe extern "C" {
        fn acl_get_fd_np(fd: libc::c_int, kind: libc::c_int) -> *mut c_void;
        fn acl_get_entry(
            acl: *mut c_void,
            index: libc::c_int,
            entry: *mut *mut c_void,
        ) -> libc::c_int;
        fn acl_get_tag_type(entry: *mut c_void, tag: *mut libc::c_int) -> libc::c_int;
        fn acl_get_permset_mask_np(entry: *mut c_void, mask: *mut u64) -> libc::c_int;
        fn acl_free(value: *mut c_void) -> libc::c_int;
    }
    struct Acl(*mut c_void);
    impl Drop for Acl {
        fn drop(&mut self) {
            // SAFETY: the ACL allocation is released exactly once, after use.
            unsafe {
                acl_free(self.0);
            }
        }
    }
    // SAFETY: descriptor is retained and EXTENDED is the Darwin ACL type.
    let pointer = unsafe { acl_get_fd_np(file.as_raw_fd(), 0x100) };
    if pointer.is_null() {
        let error = std::io::Error::last_os_error();
        ensure!(
            error.raw_os_error() == Some(libc::ENOENT),
            "control ACL inspection failed: {error}"
        );
        return Ok(());
    }
    let acl = Acl(pointer);
    let mut index = 0;
    // Read/list, execute/search, read attributes/xattrs/security, synchronize.
    // Reject unknown bits as well as every write/delete/ownership/security bit.
    let read_only_mask: u64 = (1 << 1) | (1 << 3) | (1 << 7) | (1 << 9) | (1 << 11) | (1 << 20);
    loop {
        let mut entry = std::ptr::null_mut();
        // SAFETY: acl is live; output is a writable pointer to an ACL entry.
        let status = unsafe { acl_get_entry(acl.0, index, &mut entry) };
        if status != 0 {
            let error = std::io::Error::last_os_error();
            ensure!(
                status == -1 && error.raw_os_error() == Some(libc::EINVAL),
                "control ACL iteration failed: {error}"
            );
            return Ok(());
        }
        ensure!(!entry.is_null(), "control ACL entry could not be inspected");
        ensure!(
            !private,
            "private control file/directory has an extended ACL; copy into a new private destination, existing ACLs are never removed"
        );
        let mut tag = 0;
        let mut mask = 0;
        // SAFETY: entry belongs to the live ACL; tag/mask are writable outputs.
        ensure!(
            unsafe { acl_get_tag_type(entry, &mut tag) } == 0
                && unsafe { acl_get_permset_mask_np(entry, &mut mask) } == 0,
            "control ancestor ACL entry inspection failed"
        );
        ensure!(
            tag == 2 || (tag == 1 && mask & !read_only_mask == 0),
            "control ancestor ACL permits modification or has unsupported semantics; choose a new private destination"
        );
        index = -1;
    }
}

#[cfg(test)]
mod descriptor_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn cloned_directory_retains_verified_identity_and_lives_after_the_handle() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = temporary.path().canonicalize().unwrap();
        let directory = Directory::open_existing(&path).unwrap();
        let clone = directory.try_clone_file().unwrap();
        let expected = Identity::of(&directory.0.file.metadata().unwrap());
        // SAFETY: clone owns a live descriptor and F_GETFD has no extra argument.
        assert_ne!(
            unsafe { libc::fcntl(clone.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        drop(directory);
        assert!(Identity::of(&clone.metadata().unwrap()) == expected);
    }

    #[test]
    fn cloning_a_directory_refuses_a_changed_path_identity() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let path = base.join("control");
        let directory = Directory::open_or_create(&path).unwrap();
        std::fs::rename(&path, base.join("previous")).unwrap();
        Directory::open_or_create(&path).unwrap();
        assert!(directory.try_clone_file().is_err());
    }

    #[test]
    fn source_containment_compares_directory_identities_without_requiring_private_project_modes() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let project = base.join("project");
        let other = base.join("other");
        std::fs::create_dir(&project).unwrap();
        std::fs::create_dir(&other).unwrap();
        std::fs::set_permissions(&project, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o777)).unwrap();
        let source = project.join("credentials");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&source)
            .unwrap();
        file.write_all(b"fixture").unwrap();
        drop(file);
        let source = PrivateFile::open(&source, 1024).unwrap();
        assert!(source.is_within_directory(&project).unwrap());
        assert!(source.is_within_directory(&base).unwrap());
        assert!(!source.is_within_directory(&other).unwrap());
        assert!(source.is_within_directory(&base.join("absent")).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn darwin_ownership_ignoring_mounts_are_refused_even_for_local_apfs_hfs() {
        // Darwin SDK sys/mount.h: MNT_IGNORE_OWNERSHIP=0x00200000,
        // also named MNT_UNKNOWNPERMISSIONS. No real mount is modified here.
        let ignore_ownership = 0x00200000;
        let local = libc::MNT_LOCAL as u32;
        for kind in [b"apfs".as_slice(), b"hfs".as_slice()] {
            assert!(check_darwin_filesystem(kind, local).is_ok());
            assert!(check_darwin_filesystem(kind, local | ignore_ownership).is_err());
        }
        assert!(check_darwin_filesystem(b"apfs", 0).is_err());
        assert!(check_darwin_filesystem(b"smbfs", local).is_err());
    }
}
