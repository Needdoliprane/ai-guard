//! SQLite namespace policy. SQLite's VFS exclusively owns open main/SHM files.
//! Private directories and qualified DAC/ACL semantics protect against other
//! local UIDs, not the control UID, root or storage administrators.
use crate::{
    control_fs::{Directory, PrivateEntryIdentity, absolute_path},
    human_authority::SourceStorageBindingV1,
};
use anyhow::{Context, Result, ensure};
use rusqlite::Connection;
use std::{
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Debug)]
struct DescriptorShortage;

impl std::fmt::Display for DescriptorShortage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("audit filesystem descriptor resources temporarily unavailable")
    }
}

/// Only filesystem validation introduces this marker. In particular, an
/// unrelated IO, SQLite, integrity or projection failure cannot become retryable.
pub(super) fn descriptor_shortage(error: &anyhow::Error) -> bool {
    error.is::<DescriptorShortage>()
}

pub(super) struct StoreStorage {
    parent: Directory,
    name: OsString,
    path: PathBuf,
    identity: PrivateEntryIdentity,
    created: bool,
    #[cfg(test)]
    pub(super) test_hook: super::resource_tests::StorageHook,
}

/// Serializes creation and initialization, including case/Unicode aliases on
/// case-insensitive filesystems, with one reserved lock per private directory.
/// This is a separate inode; closing it cannot release SQLite's POSIX locks.
pub(super) struct InitializationGuard {
    _lock: File,
    parent: Directory,
}

impl InitializationGuard {
    fn acquire(parent: &Directory) -> Result<Self> {
        let file = parent.sqlite_initialization_lock()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            // SAFETY: file is the retained reserved initialization-lock FD.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Self {
                    _lock: file,
                    parent: parent.clone(),
                });
            }
            let error = std::io::Error::last_os_error();
            ensure!(
                matches!(
                    error.raw_os_error(),
                    Some(libc::EWOULDBLOCK) | Some(libc::EINTR)
                ),
                "SQLite initialization lock failed: {error}"
            );
            ensure!(
                Instant::now() < deadline,
                "SQLite initialization is busy; retry after the other open or backup finishes"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl StoreStorage {
    pub(super) fn open(path: &Path) -> Result<(Self, InitializationGuard)> {
        Self::prepare(path, false, false)
    }

    pub(super) fn create_new(path: &Path) -> Result<(Self, InitializationGuard)> {
        Self::prepare(path, true, false)
    }

    pub(super) fn open_existing(path: &Path) -> Result<(Self, InitializationGuard)> {
        Self::prepare(path, false, true)
    }

    fn prepare(
        path: &Path,
        new_only: bool,
        existing_only: bool,
    ) -> Result<(Self, InitializationGuard)> {
        let path = absolute_path(path)?;
        let name = path
            .file_name()
            .context("audit database filename is missing")?
            .to_owned();
        // APFS/HFS+ may fold non-ASCII characters into reserved ASCII names
        // (for example long-s into s). Do not invent filesystem case folding.
        // Parent directory names may still contain Unicode.
        ensure!(
            name.as_bytes().is_ascii(),
            "audit database filename must use ASCII to avoid SQLite namespace aliases; preserve existing data and plan a controlled offline migration to an ASCII filename"
        );
        let lower = name.as_bytes().to_ascii_lowercase();
        ensure!(
            !lower.starts_with(b".ai-guard-sqlite-")
                && ![b"-wal".as_slice(), b"-shm", b"-journal"]
                    .iter()
                    .any(|suffix| lower.ends_with(suffix)),
            "audit database filename uses a reserved SQLite sidecar or initialization-lock name"
        );
        let parent_path = path.parent().context("audit database parent is missing")?;
        let parent = if existing_only {
            Directory::open_existing(parent_path)?
        } else {
            Directory::open_or_create(parent_path)?
        };
        // Inspect every existing entry before creating even the lock file.
        let existing = parent.private_entry(&name)?;
        ensure!(
            !existing_only || existing.is_some(),
            "existing audit database is required"
        );
        let sidecars = Self::check_sidecars(&parent, &name)?;
        ensure!(
            !new_only || (existing.is_none() && !sidecars),
            "migration backup destination or sidecar already exists"
        );
        ensure!(
            existing.is_some() || !sidecars,
            "orphan SQLite sidecars exist; preserve them and restore the intended database before reopening"
        );
        let guard = InitializationGuard::acquire(&parent)?;
        let existing = parent.private_entry(&name)?;
        ensure!(
            !existing_only || existing.is_some(),
            "existing audit database disappeared before open"
        );
        let sidecars = Self::check_sidecars(&parent, &name)?;
        ensure!(
            !new_only || (existing.is_none() && !sidecars),
            "migration backup destination or sidecar already exists"
        );
        ensure!(
            existing.is_some() || !sidecars,
            "orphan SQLite sidecars exist; preserve them and restore the intended database before reopening"
        );
        let created = existing.is_none();
        if created {
            // The exclusive new inode has no VFS users: every Store/backup open
            // in this directory waits on guard. Close BEFORE any VFS open.
            let file = parent.create_private_entry(&name)?;
            file.sync_all()?;
            drop(file);
            parent.sync()?;
        }
        let identity = parent
            .private_entry(&name)?
            .context("audit database disappeared during preparation")?;
        Ok((
            Self {
                parent,
                name,
                path,
                identity,
                created,
                #[cfg(test)]
                test_hook: super::resource_tests::StorageHook::default(),
            },
            guard,
        ))
    }

    fn check_sidecars(parent: &Directory, name: &OsStr) -> Result<bool> {
        let mut present = false;
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = name.to_os_string();
            sidecar.push(suffix);
            present |= parent.private_entry(&sidecar)?.is_some();
        }
        Ok(present)
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn created_here(&self) -> bool {
        self.created
    }

    pub(super) fn identity_key(&self) -> (PathBuf, PrivateEntryIdentity) {
        (self.path.clone(), self.identity)
    }

    /// Describe the retained main and parent using metadata only.
    pub(super) fn retained_binding(&self) -> Result<SourceStorageBindingV1> {
        self.verify_names()?;
        let ((parent_device, parent_inode), entry) = self
            .parent
            .private_entry_same_mount_with_parent(&self.name)?;
        ensure!(
            entry == self.identity,
            "retained SQLite main identity differs"
        );
        let (source_device, source_inode) = entry.physical_identity();
        let file_name = self
            .name
            .to_str()
            .context("retained SQLite filename is not UTF-8")?;
        self.verify_names()?;
        Ok(SourceStorageBindingV1 {
            parent_path: self.parent.path().to_owned(),
            file_name: file_name.to_owned(),
            parent_device,
            parent_inode,
            source_device,
            source_inode,
        })
    }

    /// Metadata/ACL only. Never opens a database or shared-memory inode.
    pub(super) fn verify_names(&self) -> Result<()> {
        self.verify_names_inner().map_err(|error| {
            if error.chain().any(|cause| {
                cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
                    matches!(io.raw_os_error(), Some(libc::EMFILE) | Some(libc::ENFILE))
                })
            }) {
                error.context(DescriptorShortage)
            } else {
                error
            }
        })
    }

    fn verify_names_inner(&self) -> Result<()> {
        #[cfg(test)]
        self.test_hook.run()?;
        ensure!(
            self.parent.private_entry(&self.name)? == Some(self.identity),
            "audit database identity changed or disappeared; recovery required"
        );
        Self::check_sidecars(&self.parent, &self.name)?;
        Ok(())
    }

    pub(super) fn verify(&self, connection: &Connection) -> Result<()> {
        self.verify_names()?;
        let mut moved: libc::c_int = -1;
        connection
            .file_control(Some("main"), rusqlite::FileControl::HasMoved(&mut moved))
            .context(
                "SQLite main-file identity check failed or is unsupported; recovery required",
            )?;
        ensure!(
            moved == 0,
            "SQLite main-file identity changed; recovery required"
        );
        Ok(())
    }

    /// Only after the destination connection has been closed, while the fresh
    /// backup's initialization guard still excludes other cooperating opens.
    pub(super) fn sync_closed(&self, guard: &InitializationGuard) -> Result<()> {
        ensure!(
            self.created && self.parent.path() == guard.parent.path(),
            "closed-file synchronization requires the new destination initialization guard"
        );
        self.verify_names()?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(&self.path)?;
        let facts = file.metadata()?;
        ensure!(
            facts.is_file() && (facts.dev(), facts.ino()) == self.identity.physical_identity(),
            "closed SQLite file synchronization identity differs"
        );
        file.sync_all()?;
        drop(file);
        self.verify_names()?;
        self.parent.sync()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn unavailable_vfs_identity_control_is_refused() {
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let (storage, _guard) = StoreStorage::open(&directory.path().join("audit.sqlite")).unwrap();
        let unsupported = Connection::open_in_memory().unwrap();
        assert!(storage.verify(&unsupported).is_err());
        storage.verify_names().unwrap();
        assert_eq!(std::fs::metadata(storage.path()).unwrap().len(), 0);
    }
}

/// Associate a connection with its retained physical main. This shared port
/// returns no owner, epoch, identity or business permission. The implementation
/// retains the reviewed exact-main/TEMP/backing checks and typed foreign error.
pub(super) fn verify_connection_main(
    backing: &StoreStorage,
    db: &rusqlite::Connection,
) -> anyhow::Result<()> {
    super::review_authority::verify_connection_main(backing, db)
}
