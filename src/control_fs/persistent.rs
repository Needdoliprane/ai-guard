//! Durable records in an exclusive private business namespace.
//!
//! This module never uses SQLite/VFS creation or locking helpers. Existing
//! regular entries are inspected without opening them before obtaining an FD.
//! The embedding control must exclude databases from this namespace; known
//! protected identities provide an additional, non-exhaustive check. Failed
//! operations retain partial evidence and never repair or remove it.

use super::*;

#[derive(Debug)]
pub(crate) struct PersistentLimit;
impl fmt::Display for PersistentLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("persistent entry count exceeds its bound")
    }
}
impl std::error::Error for PersistentLimit {}
use std::os::unix::fs::FileExt;

/// Logical file bytes; directories consume an entry but no logical file bytes.
pub(crate) struct Entry {
    pub name: String,
    pub directory: bool,
    pub bytes: u64,
}

/// A bounded metadata inventory, not an admission of an entire namespace.
/// Rejected names are counted but never returned or opened as regular files.
pub(crate) struct Inventory {
    pub entries: Vec<Entry>,
    pub visited: u32,
    pub rejected: u32,
    pub incomplete: bool,
}

impl Directory {
    /// Synchronize this directory and all its retained, verified ancestors.
    /// `open_or_create` does not synchronize newly created ancestors. Keep the
    /// complete chain alive until every child and parent has been synchronized.
    pub(crate) fn durable_bootstrap(&self) -> Result<()> {
        self.revalidate()?;
        let root = OpenOptions::new()
            .read(true)
            .custom_flags(DIRECTORY_FLAGS)
            .open("/")
            .context("persistent filesystem root is unavailable")?;
        let mut files = vec![root];
        let names: Vec<&OsStr> = self
            .0
            .path
            .components()
            .filter_map(|component| match component {
                Component::Normal(name) => Some(name),
                _ => None,
            })
            .collect();
        let root_facts = check_directory(&files[0], names.is_empty())?;
        ensure!(
            self.0.chain.first() == Some(&Identity::of(&root_facts)),
            "persistent filesystem root identity changed"
        );
        for (index, name) in names.iter().enumerate() {
            let parent = files
                .last()
                .context("persistent ancestor descriptor is unavailable")?;
            let child = open_child_directory(parent, &child_name(name)?, false)?;
            let facts = check_directory(&child, index + 1 == names.len())?;
            ensure!(
                self.0.chain.get(index + 1) == Some(&Identity::of(&facts)),
                "persistent ancestor identity changed"
            );
            files.push(child);
        }
        ensure!(
            files.len() == self.0.chain.len(),
            "persistent ancestor chain changed"
        );
        self.revalidate()?;
        for file in files.iter().rev() {
            file.sync_all()
                .context("persistent ancestor synchronization failed")?;
        }
        self.revalidate()
    }

    /// A child opened for reading is never created or synchronized implicitly.
    /// Creation synchronizes both child and parent, including an existing child.
    pub(crate) fn persistent_child(&self, name: &str, create: bool) -> Result<Option<Self>> {
        self.revalidate()?;
        let encoded = persistent_name(name)?;
        if inspect_directory(self, name, &encoded)?.is_none() {
            if !create {
                return Ok(None);
            }
            // SAFETY: retained private parent FD and a checked single component.
            // Failure leaves a newly created entry untouched for diagnosis.
            if unsafe { libc::mkdirat(self.0.file.as_raw_fd(), encoded.as_ptr(), 0o700) } != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::EEXIST) {
                    return Err(error).context("persistent directory creation failed");
                }
            }
        }
        let expected = inspect_directory(self, name, &encoded)?
            .context("persistent directory disappeared before opening")?;
        let file = open_child_directory(&self.0.file, &encoded, false)?;
        let facts = check_directory(&file, true)?;
        ensure!(
            entry_identity(&facts) == expected,
            "persistent directory identity changed during opening"
        );
        let mut chain = self.0.chain.clone();
        chain.push(Identity::of(&facts));
        let child = Self(Arc::new(Anchor {
            file,
            path: self.0.path.join(name),
            chain,
            private: true,
        }));
        child.revalidate()?;
        if create {
            child.sync()?;
            self.sync()?;
        }
        Ok(Some(child))
    }

    /// Read an immutable bounded record after metadata-only inode admission.
    pub(crate) fn persistent_read(
        &self,
        name: &str,
        max: u64,
        exclusions: &[PrivateEntryIdentity],
    ) -> Result<Option<PrivateFile>> {
        ensure!(
            max <= MAX_PRIVATE_FILE_BYTES,
            "persistent input bound is unsupported"
        );
        let encoded = persistent_name(name)?;
        let Some(identity) = self.private_entry(OsStr::new(name))? else {
            return Ok(None);
        };
        // Reject an aliased SQLite inode BEFORE opening/closing an FD that could
        // release the process's POSIX locks on that inode.
        ensure!(
            !exclusions.contains(&identity),
            "protected file identity in persistent namespace"
        );
        let mut file = open_file(&self.0.file, &encoded)?;
        let before = check_private_file(&file)?;
        ensure!(
            identity == entry_identity(&before),
            "persistent file changed during opening"
        );
        ensure!(before.len() <= max, "persistent file exceeds its bound");
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(max + 1)
            .read_to_end(&mut bytes)
            .context("persistent bounded read failed")?;
        ensure!(
            bytes.len() as u64 <= max,
            "persistent file grew beyond its bound"
        );
        let facts = check_private_file(&file)?;
        ensure!(
            same_file_state(&before, &facts),
            "persistent file changed during reading"
        );
        let result = PrivateFile(Arc::new(FileInner {
            file,
            path: self.path().join(name),
            parent: self.0.clone(),
            name: name.into(),
            facts,
            digest: hex::encode(Sha256::digest(&bytes)),
            bytes,
            remove_on_drop: false,
        }));
        result.persistent_revalidate()?;
        Ok(Some(result))
    }

    /// Exclusive immutable record, with partial failures retained. Caller must
    /// synchronize this file AND its parent before acknowledging it.
    pub(crate) fn persistent_create(&self, name: &str, bytes: &[u8]) -> Result<PrivateFile> {
        ensure!(
            bytes.len() as u64 <= MAX_PRIVATE_FILE_BYTES,
            "persistent record exceeds its bound"
        );
        let encoded = persistent_name(name)?;
        self.revalidate()?;
        if self.private_entry(OsStr::new(name))?.is_some() {
            return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists).into());
        }
        // SAFETY: checked component and retained private parent. O_EXCL never
        // opens, truncates or repairs an existing inode, including a symlink.
        let fd = unsafe {
            libc::openat(
                self.0.file.as_raw_fd(),
                encoded.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | FILE_FLAGS,
                0o600,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error())
                .context("persistent exclusive creation failed");
        }
        // SAFETY: successful openat transfers exactly one owned descriptor.
        let mut file = unsafe { File::from_raw_fd(fd) };
        check_private_file(&file)?;
        file.write_all(bytes)
            .context("persistent record write failed")?;
        let facts = check_private_file(&file)?;
        let result = PrivateFile(Arc::new(FileInner {
            file,
            path: self.path().join(name),
            parent: self.0.clone(),
            name: name.into(),
            facts,
            digest: hex::encode(Sha256::digest(bytes)),
            bytes: bytes.to_vec(),
            remove_on_drop: false,
        }));
        result.persistent_revalidate()?;
        Ok(result)
    }

    /// Replace only the journal pointer after a retained-identity comparison.
    /// Mutex/flock excludes cooperating writers; private parent DAC excludes
    /// other UIDs. Hostile same-UID/root races are outside this policy.
    pub(crate) fn persistent_publish(
        &self,
        temporary: &PrivateFile,
        current: &PrivateFile,
        target: &str,
    ) -> Result<()> {
        ensure!(
            target == "current.json" && temporary.0.name == OsStr::new("current-next.json"),
            "unsupported persistent publication name"
        );
        let target = persistent_name(target)?;
        let source = child_name(&temporary.0.name)?;
        self.revalidate()?;
        ensure!(
            temporary.0.parent.chain == self.0.chain
                && current.0.parent.chain == self.0.chain
                && temporary.0.parent.path == self.0.path
                && current.0.parent.path == self.0.path
                && current.0.name == OsStr::new("current.json")
                && entry_identity(&temporary.0.facts) != entry_identity(&current.0.facts),
            "persistent publication parent or identity mismatch"
        );
        temporary.persistent_sync()?;
        // Final CAS check before replacement: held old FD and metadata-only
        // pathname inspection, without opening another FD to the old inode.
        current.persistent_revalidate()?;
        // SAFETY: checked names in the same retained private parent. No other
        // business record is eligible for this narrowly scoped replacement.
        if unsafe {
            libc::renameat(
                self.0.file.as_raw_fd(),
                source.as_ptr(),
                self.0.file.as_raw_fd(),
                target.as_ptr(),
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error())
                .context("persistent pointer publication failed");
        }
        let expected = entry_identity(&temporary.0.facts);
        ensure!(
            self.private_entry(OsStr::new("current.json"))? == Some(expected),
            "published persistent pointer identity changed"
        );
        self.sync()?;
        ensure!(
            self.private_entry(OsStr::new("current.json"))? == Some(expected),
            "durable persistent pointer identity changed"
        );
        Ok(())
    }

    /// Bounded metadata enumeration includes every temporary/partial entry.
    /// No regular-file inode is opened here, including unexpected entries.
    /// Recovery may continue past an isolated unsafe entry. The strict business
    /// enumeration below retains its original fail-fast behavior.
    pub(crate) fn persistent_inventory(&self, limit: u32) -> Result<Inventory> {
        self.revalidate()?;
        // SAFETY: duplicate only a retained directory FD, never a database FD.
        let fd = unsafe { libc::fcntl(self.0.file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error()).context("recovery directory unavailable");
        }
        // SAFETY: fdopendir takes ownership on success; fd is a directory.
        let pointer = unsafe { libc::fdopendir(fd) };
        if pointer.is_null() {
            let error = std::io::Error::last_os_error();
            // SAFETY: failed fdopendir left this owned DIRECTORY descriptor open.
            unsafe { libc::close(fd) };
            return Err(error).context("recovery inventory unavailable");
        }
        let stream = DirectoryStream(pointer);
        // SAFETY: live stream, serialized by the caller's exclusive journal lease.
        unsafe { libc::rewinddir(stream.0) };
        let mut inventory = Inventory {
            entries: Vec::new(),
            visited: 0,
            rejected: 0,
            incomplete: false,
        };
        loop {
            clear_errno();
            // SAFETY: live stream, errno cleared to distinguish EOF from error.
            let next = unsafe { libc::readdir(stream.0) };
            if next.is_null() {
                inventory.incomplete |= std::io::Error::last_os_error().raw_os_error() != Some(0);
                break;
            }
            // SAFETY: copy this valid dirent name before the next readdir.
            let bytes = unsafe { std::ffi::CStr::from_ptr((*next).d_name.as_ptr()) }.to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            if inventory.visited >= limit {
                inventory.incomplete = true;
                break;
            }
            inventory.visited += 1;
            let admitted = (|| -> Result<Entry> {
                let name = std::str::from_utf8(bytes)
                    .context("recovery name is unsupported")?
                    .to_owned();
                let encoded = persistent_name(&name)?;
                let facts =
                    named_metadata(self, &encoded)?.context("recovery entry disappeared")?;
                let directory = facts.st_mode & libc::S_IFMT == libc::S_IFDIR;
                let logical_bytes = if directory {
                    let child = self
                        .persistent_child(&name, false)?
                        .context("recovery directory disappeared")?;
                    ensure!(
                        entry_identity(&child.0.file.metadata()?) == native_identity(&facts),
                        "recovery directory changed"
                    );
                    0
                } else {
                    // Metadata and ACL inspection only: no file FD is opened.
                    ensure!(
                        self.private_entry(OsStr::new(&name))? == Some(native_identity(&facts)),
                        "recovery entry changed"
                    );
                    u64::try_from(facts.st_size).context("recovery size is unsupported")?
                };
                Ok(Entry {
                    name,
                    directory,
                    bytes: logical_bytes,
                })
            })();
            match admitted {
                Ok(entry) => inventory.entries.push(entry),
                Err(_) => {
                    inventory.rejected += 1;
                    inventory.incomplete = true;
                }
            }
        }
        inventory.incomplete |= stream.close().is_err();
        // A changed shared anchor invalidates all entries, unlike a bad sibling.
        self.revalidate()?;
        Ok(inventory)
    }

    pub(crate) fn persistent_entries(&self, limit: u32) -> Result<Vec<Entry>> {
        self.revalidate()?;
        // SAFETY: retained directory FD; the new descriptor is independently
        // owned and atomically CLOEXEC, including before fdopendir takes it.
        let fd = unsafe { libc::fcntl(self.0.file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error())
                .context("persistent directory descriptor duplication failed");
        }
        // SAFETY: fdopendir owns this new DIRECTORY FD on success. It never
        // receives a regular-file FD and cannot disturb SQLite's POSIX locks.
        let stream = unsafe { libc::fdopendir(fd) };
        if stream.is_null() {
            let error = std::io::Error::last_os_error();
            // SAFETY: failed fdopendir leaves this descriptor with the caller.
            unsafe { libc::close(fd) };
            return Err(error).context("persistent directory enumeration failed");
        }
        let stream = DirectoryStream(stream);
        // SAFETY: live owned stream; journal writer mutex serializes enumeration
        // because dup shares the directory offset.
        unsafe { libc::rewinddir(stream.0) };
        let mut entries = Vec::new();
        loop {
            clear_errno();
            // SAFETY: live stream, errno cleared to distinguish EOF from error.
            // Copy the returned name before calling readdir again.
            let next = unsafe { libc::readdir(stream.0) };
            if next.is_null() {
                let error = std::io::Error::last_os_error();
                ensure!(
                    error.raw_os_error() == Some(0),
                    "persistent directory read failed"
                );
                break;
            }
            // SAFETY: readdir returned a live dirent with NUL-terminated name.
            let bytes = unsafe { std::ffi::CStr::from_ptr((*next).d_name.as_ptr()) }.to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            if entries.len() as u64 >= u64::from(limit) {
                return Err(PersistentLimit.into());
            }
            let name = std::str::from_utf8(bytes)
                .context("persistent entry name is unsupported")?
                .to_owned();
            let encoded = persistent_name(&name)?;
            let facts = named_metadata(self, &encoded)?
                .context("persistent entry disappeared during enumeration")?;
            let directory = facts.st_mode & libc::S_IFMT == libc::S_IFDIR;
            let logical_bytes = if directory {
                let child = self
                    .persistent_child(&name, false)?
                    .context("persistent directory disappeared during enumeration")?;
                ensure!(
                    entry_identity(&child.0.file.metadata()?) == native_identity(&facts),
                    "persistent directory changed during enumeration"
                );
                0
            } else {
                ensure!(
                    self.private_entry(OsStr::new(&name))? == Some(native_identity(&facts)),
                    "persistent file changed during enumeration"
                );
                u64::try_from(facts.st_size).context("persistent file size is unsupported")?
            };
            entries.push(Entry {
                name,
                directory,
                bytes: logical_bytes,
            });
        }
        stream.close()?;
        self.revalidate()?;
        Ok(entries)
    }
}

/// Closed ASCII names generated by the embedding journal, never caller paths.
fn persistent_name(name: &str) -> Result<CString> {
    ensure!(
        !name.is_empty()
            && name.len() <= 128
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            && name != "."
            && name != "..",
        "unsupported persistent entry name"
    );
    child_name(OsStr::new(name))
}

fn named_metadata(parent: &Directory, name: &CString) -> Result<Option<libc::stat>> {
    let mut facts = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: retained parent FD, one C component and writable output. NOFOLLOW
    // observes a symlink itself and never opens a regular-file inode.
    if unsafe {
        libc::fstatat(
            parent.0.file.as_raw_fd(),
            name.as_ptr(),
            facts.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOENT) {
            return Ok(None);
        }
        return Err(error).context("persistent entry metadata is unavailable");
    }
    // SAFETY: successful fstatat initialized the complete stat structure.
    Ok(Some(unsafe { facts.assume_init() }))
}

fn inspect_directory(
    parent: &Directory,
    name: &str,
    encoded: &CString,
) -> Result<Option<PrivateEntryIdentity>> {
    let Some(before) = named_metadata(parent, encoded)? else {
        return Ok(None);
    };
    ensure!(
        before.st_mode & libc::S_IFMT == libc::S_IFDIR
            && before.st_uid == effective_uid()
            && before.st_mode & 0o7777 == 0o700,
        "persistent directory requires effective-UID ownership and mode 0700"
    );
    check_named_private_acl(&parent.0.path.join(name))?;
    let after = named_metadata(parent, encoded)?
        .context("persistent directory disappeared during inspection")?;
    ensure!(
        native_identity(&before) == native_identity(&after)
            && after.st_mode & libc::S_IFMT == libc::S_IFDIR
            && after.st_uid == effective_uid()
            && after.st_mode & 0o7777 == 0o700,
        "persistent directory changed during inspection"
    );
    Ok(Some(native_identity(&after)))
}

fn native_identity(facts: &libc::stat) -> PrivateEntryIdentity {
    // Native stat widths differ between Darwin/Linux and 32/64-bit ABIs.
    #[allow(clippy::unnecessary_cast)]
    PrivateEntryIdentity {
        device: facts.st_dev as u64,
        inode: facts.st_ino as u64,
    }
}

fn entry_identity(facts: &Metadata) -> PrivateEntryIdentity {
    PrivateEntryIdentity {
        device: facts.dev(),
        inode: facts.ino(),
    }
}

struct DirectoryStream(*mut libc::DIR);

impl DirectoryStream {
    fn close(mut self) -> Result<()> {
        let pointer = std::mem::replace(&mut self.0, std::ptr::null_mut());
        // SAFETY: consume exactly this live fdopendir result, even on failure.
        if unsafe { libc::closedir(pointer) } != 0 {
            return Err(std::io::Error::last_os_error())
                .context("persistent directory close failed");
        }
        Ok(())
    }
}

impl Drop for DirectoryStream {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: error-path fallback owns one live fdopendir allocation.
            // An earlier operation already failed, so no success can hide this.
            unsafe { libc::closedir(self.0) };
        }
    }
}

#[cfg(target_os = "linux")]
fn clear_errno() {
    // SAFETY: libc returns this thread's valid errno storage.
    unsafe { *libc::__errno_location() = 0 };
}

#[cfg(target_os = "macos")]
fn clear_errno() {
    // SAFETY: Darwin libc returns this thread's valid errno storage.
    unsafe { *libc::__error() = 0 };
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn clear_errno() {
    // Directory admission refuses unsupported platforms before reaching here.
    // No unsupported errno ABI is assumed.
}

impl PrivateFile {
    /// Retained FD, exact cache and metadata-only name validation. Deliberately
    /// never opens a second descriptor to the source pathname.
    pub(crate) fn persistent_revalidate(&self) -> Result<()> {
        self.0.parent.revalidate()?;
        let before = check_private_file(&self.0.file)?;
        ensure!(
            same_file_state(&before, &self.0.facts),
            "persistent retained file changed"
        );
        ensure!(
            self.0.bytes.len() as u64 == before.len()
                && before.len() <= MAX_PRIVATE_FILE_BYTES
                && self.0.digest == hex::encode(Sha256::digest(&self.0.bytes)),
            "persistent cached bytes changed"
        );
        let mut digest = Sha256::new();
        let mut offset = 0usize;
        let mut buffer = [0u8; 65536];
        while offset < self.0.bytes.len() {
            let wanted = buffer.len().min(self.0.bytes.len() - offset);
            let count = match self.0.file.read_at(&mut buffer[..wanted], offset as u64) {
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error).context("persistent retained read failed"),
            };
            ensure!(count != 0, "persistent retained file was truncated");
            digest.update(&buffer[..count]);
            offset += count;
        }
        ensure!(
            self.0.digest == hex::encode(digest.finalize()),
            "persistent retained contents changed"
        );
        let after = check_private_file(&self.0.file)?;
        ensure!(
            same_file_state(&before, &after),
            "persistent file changed during retained reading"
        );
        let parent = Directory(self.0.parent.clone());
        ensure!(
            parent.private_entry(&self.0.name)? == Some(entry_identity(&after)),
            "persistent pathname identity changed"
        );
        Ok(())
    }

    pub(crate) fn persistent_sync(&self) -> Result<()> {
        self.persistent_revalidate()?;
        self.0
            .file
            .sync_all()
            .context("persistent file synchronization failed")?;
        self.persistent_revalidate()
    }

    /// Clones share Arc/File: dropping one clone never explicitly unlocks.
    /// Independent opens have their own file descriptions and see WouldBlock.
    pub(crate) fn persistent_try_lock(&self) -> Result<bool> {
        self.persistent_revalidate()?;
        // SAFETY: retained CLOEXEC FD, nonblocking advisory whole-file lock.
        if unsafe { libc::flock(self.0.file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            self.persistent_revalidate()?;
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            return Ok(false);
        }
        Err(error).context("persistent writer lock failed")
    }
}
