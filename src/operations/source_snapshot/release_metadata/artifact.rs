//! Bounded streaming reader of the artefacts a manifest binds by name, length
//! and SHA-256.
//!
//! An artefact is a regular file owned by root or the effective UID, without
//! group/other write and without set-ID or sticky bit, on a supported local
//! filesystem, reached from `/` through directories that pass the export
//! ancestor policy and never through a symbolic link. The type of its name
//! is read with `fstatat(AT_SYMLINK_NOFOLLOW)` before any open, so that a
//! device node, a FIFO, a socket or a directory is refused without being
//! opened; the opened descriptor must then have the device and inode read
//! first. An entry renamed over the name between both calls can be opened
//! once, without blocking and never as a controlling terminal, before that
//! identity check refuses it. Any link count is accepted: a second name adds
//! no write path to the inode, whose owner and mode are checked. The bytes
//! are bound to one observed state of the held descriptor (device, inode,
//! owner, group, mode, link count, length and both timestamps with
//! nanoseconds): after the read that state is compared on the held
//! descriptor and on the name opened again (read the same way), and the
//! directory chain is walked again from `/`. A link added or removed while
//! hashing changes the link count and the change time, and is refused.
//!
//! This reader hashes; it never trusts an artefact as configuration. On macOS
//! the extended ACL of the file itself is not inspected here (the control
//! policy that would do it is private to `control_fs`). It holds owned
//! `unsafe` code and needs the second review of read paths.
use super::super::capture::shown;
use super::stopped;
use crate::{control_fs, runtime::retained_cli::Interruption};
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    ffi::{CString, OsStr},
    fs::{File, Metadata, OpenOptions},
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ArtifactDigest {
    pub(super) name: String,
    pub(super) bytes: u64,
    pub(super) sha256: String,
}

pub(super) const MAX_ARTIFACTS: usize = 64;
pub(super) const MAX_ARTIFACT_BYTES: u64 = 4 << 30;
const BUFFER_BYTES: usize = 64 * 1024;
const STOP_INTERVAL: u64 = 8 * 1024 * 1024;
const MAX_NAME_BYTES: usize = 128;
/// A guard against mistakes, not a security boundary: only a name, a length
/// and a digest are ever recorded, never content.
const REFUSED_SUFFIXES: [&str; 9] = [
    ".env", ".db", ".sqlite", ".sqlite3", "-wal", "-shm", ".key", ".pem", ".p12",
];
const DIRECTORY_FLAGS: libc::c_int =
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
// A name is opened only once `fstatat` saw a regular file. If it is replaced
// between both calls, O_NONBLOCK keeps a FIFO from blocking and O_NOCTTY keeps
// a terminal from becoming the controlling terminal before the identity check
// refuses the descriptor.
const FILE_FLAGS: libc::c_int =
    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOCTTY;
/// Every flag `open_at` admits: the two read-only lookup sets above. `openat`
/// is variadic, and libc reads a mode argument from it when the flags hold
/// `O_CREAT`, or every bit of `O_TMPFILE` on Linux and Android (the pinned
/// libc defines no `O_TMPFILE` for macOS). The assertions below check at
/// compile time that these flags hold neither.
const LOOKUP_FLAGS: libc::c_int = DIRECTORY_FLAGS | FILE_FLAGS;
const _: () = assert!(LOOKUP_FLAGS & libc::O_CREAT == 0);
#[cfg(any(target_os = "linux", target_os = "android"))]
const _: () = assert!(LOOKUP_FLAGS & libc::O_TMPFILE != libc::O_TMPFILE);

#[cfg(test)]
pub(super) type AfterRead = Box<dyn FnOnce(&Path)>;
#[cfg(test)]
pub(super) type BeforeOpen = Box<dyn FnOnce()>;
#[cfg(test)]
thread_local! {
    /// Runs once, on this thread, between the last byte read and the checks
    /// that follow it.
    pub(super) static AFTER_READ: std::cell::RefCell<Option<AfterRead>> =
        const { std::cell::RefCell::new(None) };
    /// Runs once, on this thread, after `fstatat` saw a regular file and
    /// before that name is opened.
    pub(super) static BEFORE_OPEN: std::cell::RefCell<Option<BeforeOpen>> =
        const { std::cell::RefCell::new(None) };
    /// While it holds a list, every name this thread passes to `openat`, in
    /// order.
    pub(super) static OPENED: std::cell::RefCell<Option<Vec<std::ffi::OsString>>> =
        const { std::cell::RefCell::new(None) };
}

/// `[A-Za-z0-9._+-]{1,128}`, not starting with `.`, without a suffix of the
/// guard list.
fn valid_name(name: &str) -> bool {
    (1..=MAX_NAME_BYTES).contains(&name.len())
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
        && !REFUSED_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

/// The recorded name of an artefact path. Nothing is opened.
fn name_of(path: &Path) -> Result<String> {
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .with_context(|| format!("artefact path has no UTF-8 file name: {}", shown(path)))?;
    ensure!(
        valid_name(name),
        "artefact name {} is refused: it must be 1 to {MAX_NAME_BYTES} of [A-Za-z0-9._+-], not \
         start with a dot and not end with {}",
        shown(Path::new(name)),
        REFUSED_SUFFIXES.join(", ")
    );
    Ok(name.to_owned())
}

/// At most [`MAX_ARTIFACTS`] paths, each with a valid name, no two with the
/// same name. Checked before anything is read.
pub(super) fn check_paths(paths: &[PathBuf]) -> Result<Vec<String>> {
    ensure!(
        paths.len() <= MAX_ARTIFACTS,
        "at most {MAX_ARTIFACTS} artefacts can be bound; {} were given",
        paths.len()
    );
    let mut names = Vec::with_capacity(paths.len());
    let mut seen = BTreeSet::new();
    for path in paths {
        let absolute = control_fs::absolute_path(path)
            .with_context(|| format!("artefact path is refused: {}", shown(path)))?;
        let name = name_of(&absolute)?;
        ensure!(
            seen.insert(name.clone()),
            "two artefacts have the name {name}; a manifest binds each name once"
        );
        names.push(name);
    }
    Ok(names)
}

/// `name` as one entry name of a directory: not empty, not `.` or `..`,
/// without `/` and without NUL. Checked by both `*at` callers below, so that
/// the kernel resolves no other component relative to the directory.
fn component(name: &OsStr) -> std::io::Result<CString> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a name must be one path component",
        ));
    }
    CString::new(bytes)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "name holds a NUL"))
}

/// Opens the entry `name` of the directory `parent` with `flags`, which may
/// hold only bits of `LOOKUP_FLAGS`: any other flag, including one that
/// creates an entry, is refused with `InvalidInput` and nothing is opened.
pub(super) fn open_at(parent: &File, name: &OsStr, flags: libc::c_int) -> std::io::Result<File> {
    if flags & !LOOKUP_FLAGS != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "an artefact entry is opened only with the read-only lookup flags",
        ));
    }
    let encoded = component(name)?;
    #[cfg(test)]
    OPENED.with(|opened| {
        if let Some(names) = opened.borrow_mut().as_mut() {
            names.push(name.to_owned());
        }
    });
    // SAFETY: `parent` owns a valid directory descriptor and `encoded` is one
    // NUL-terminated component, checked by `component` above. `openat` is
    // variadic and no mode argument is passed: `flags` holds only bits of
    // `LOOKUP_FLAGS`, checked at the top of this function, and that constant
    // holds neither `O_CREAT` nor every bit of `O_TMPFILE` (asserted at
    // compile time after its definition), so libc reads no mode. No pointer
    // outlives the call.
    let descriptor = unsafe { libc::openat(parent.as_raw_fd(), encoded.as_ptr(), flags) };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a successful `openat` returned a descriptor nobody else owns.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

/// What a name of an artefact directory holds, read before it is opened.
pub(super) enum Entry {
    /// A regular file, opened; its descriptor has the device and inode that
    /// were read before the open.
    Regular(File),
    SymbolicLink,
    /// A directory, FIFO, socket or device node: never opened.
    Other,
    /// A regular file was read, but the descriptor opened next names another
    /// inode: the name was replaced between both calls.
    Replaced,
}

/// Reads the type of `name` below `directory` without following a link, and
/// opens it only when it is a regular file.
pub(super) fn open_regular(directory: &File, name: &OsStr) -> std::io::Result<Entry> {
    let encoded = component(name)?;
    let mut facts = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `directory` owns a valid directory descriptor, `encoded` is one
    // NUL-terminated component, checked by `component` above, and `facts` is
    // writable; AT_SYMLINK_NOFOLLOW inspects the entry itself and opens
    // nothing.
    let result = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            encoded.as_ptr(),
            facts.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a successful fstatat initialized the complete stat structure.
    let facts = unsafe { facts.assume_init() };
    match facts.st_mode & libc::S_IFMT {
        libc::S_IFREG => {}
        libc::S_IFLNK => return Ok(Entry::SymbolicLink),
        _ => return Ok(Entry::Other),
    }
    // Native stat widths differ between Darwin and Linux.
    #[allow(clippy::unnecessary_cast)]
    let identity = (facts.st_dev as u64, facts.st_ino as u64);
    #[cfg(test)]
    {
        let hook = BEFORE_OPEN.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
    }
    let file = open_at(directory, name, FILE_FLAGS)?;
    let opened = file.metadata()?;
    if (opened.dev(), opened.ino()) != identity {
        return Ok(Entry::Replaced);
    }
    Ok(Entry::Regular(file))
}

/// Every directory from `/` down to `path`, an absolute path of plain
/// components, each opened below the one before it without following a link.
fn open_chain(path: &Path) -> std::io::Result<Vec<File>> {
    let mut chain = vec![
        OpenOptions::new()
            .read(true)
            .custom_flags(DIRECTORY_FLAGS)
            .open("/")?,
    ];
    for part in path.components() {
        if let Component::Normal(name) = part {
            let Some(parent) = chain.last() else {
                break;
            };
            let next = open_at(parent, name, DIRECTORY_FLAGS)?;
            chain.push(next);
        }
    }
    Ok(chain)
}

fn identities(chain: &[File]) -> Result<Vec<(u64, u64)>> {
    chain
        .iter()
        .map(|directory| {
            let facts = directory
                .metadata()
                .context("artefact directory identity is unavailable")?;
            Ok((facts.dev(), facts.ino()))
        })
        .collect()
}

/// The comparison of `control_fs::same_file_state`, which is private there.
fn same_state(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
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

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no pointers or preconditions.
    unsafe { libc::geteuid() }
}

/// The file policy of an artefact. No link-count rule (see the module text).
fn admit(facts: &Metadata, file: &File) -> Result<()> {
    ensure!(facts.is_file(), "an artefact must be a regular file");
    ensure!(
        facts.uid() == 0 || facts.uid() == effective_uid(),
        "an artefact must belong to root or to the effective UID"
    );
    ensure!(
        facts.mode() & 0o7022 == 0,
        "an artefact must not be writable by group or others, nor carry a set-ID or sticky bit"
    );
    control_fs::validate_filesystem_permissions(file)
}

/// Streams the whole file through SHA-256; a stop request is looked at every
/// 8 MiB. The length read must be the length observed before.
fn hash(file: &mut File, length: u64, stop: Option<&Interruption>) -> Result<String> {
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_BYTES];
    let mut total = 0_u64;
    let mut next_check = STOP_INTERVAL;
    loop {
        let count = match file.read(&mut buffer) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error).context("the artefact cannot be read"),
        };
        if count == 0 {
            break;
        }
        total += count as u64;
        ensure!(total <= length, "the artefact grew while it was hashed");
        digest.update(&buffer[..count]);
        if total >= next_check {
            stopped(stop)?;
            next_check = total + STOP_INTERVAL;
        }
    }
    ensure!(total == length, "the artefact shrank while it was hashed");
    Ok(hex::encode(digest.finalize()))
}

/// Name, length and SHA-256 of one artefact, read under the policy above.
pub(super) fn digest(path: &Path, stop: Option<&Interruption>) -> Result<ArtifactDigest> {
    stopped(stop)?;
    let absolute = control_fs::absolute_path(path)
        .with_context(|| format!("artefact path is refused: {}", shown(path)))?;
    let name = name_of(&absolute)?;
    let at = || format!("artefact {}", shown(&absolute));
    let parent = absolute
        .parent()
        .with_context(|| format!("{} has no parent directory", at()))?;
    let chain = open_chain(parent).with_context(|| {
        format!(
            "the directory of {} is unavailable or reached through a symbolic link",
            at()
        )
    })?;
    for directory in &chain {
        control_fs::check_export_directory(directory).with_context(at)?;
    }
    let recorded = identities(&chain)?;
    let directory = chain
        .last()
        .with_context(|| format!("the directory of {} is unavailable", at()))?;
    let entry = open_regular(directory, OsStr::new(&name))
        .with_context(|| format!("{} is unavailable or is a symbolic link", at()))?;
    let mut file = match entry {
        Entry::Regular(file) => file,
        Entry::SymbolicLink => bail!("{} is unavailable or is a symbolic link", at()),
        Entry::Other => bail!("{}: an artefact must be a regular file", at()),
        Entry::Replaced => bail!(
            "{} was replaced while it was opened; nothing was bound",
            at()
        ),
    };
    let before = file
        .metadata()
        .with_context(|| format!("{} cannot be inspected", at()))?;
    admit(&before, &file).with_context(at)?;
    ensure!(
        before.len() <= MAX_ARTIFACT_BYTES,
        "{} exceeds the bound of {MAX_ARTIFACT_BYTES} bytes",
        at()
    );
    let sha256 = hash(&mut file, before.len(), stop).with_context(at)?;
    #[cfg(test)]
    {
        let hook = AFTER_READ.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook {
            hook(&absolute);
        }
    }
    let held = file
        .metadata()
        .with_context(|| format!("{} cannot be inspected", at()))?;
    ensure!(
        same_state(&before, &held),
        "{} changed while it was hashed (content, owner, mode, link count or timestamps); \
         nothing was bound",
        at()
    );
    let named = match open_regular(directory, OsStr::new(&name))
        .with_context(|| format!("{} disappeared while it was hashed", at()))?
    {
        Entry::Regular(file) => file,
        Entry::SymbolicLink | Entry::Other | Entry::Replaced => bail!(
            "{} no longer names the file that was hashed; nothing was bound",
            at()
        ),
    };
    let now = named
        .metadata()
        .with_context(|| format!("{} cannot be inspected", at()))?;
    ensure!(
        same_state(&before, &now),
        "{} no longer names the file that was hashed; nothing was bound",
        at()
    );
    let walked = open_chain(parent)
        .context("an artefact directory disappeared while it was hashed")
        .and_then(|current| identities(&current))
        .with_context(at)?;
    ensure!(
        walked == recorded,
        "the directories leading to {} changed while it was hashed; nothing was bound",
        at()
    );
    Ok(ArtifactDigest {
        name,
        bytes: before.len(),
        sha256,
    })
}

/// Every artefact, in the order given, then sorted by name. A stop request
/// is looked at before each one.
pub(super) fn digest_all(
    paths: &[PathBuf],
    stop: Option<&Interruption>,
) -> Result<Vec<ArtifactDigest>> {
    check_paths(paths)?;
    let mut digests = Vec::with_capacity(paths.len());
    for path in paths {
        stopped(stop)?;
        digests.push(digest(path, stop)?);
    }
    digests.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(digests)
}
