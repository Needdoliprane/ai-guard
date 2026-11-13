//! Fixed private V2 inventory r2. Ambiguous/partial writes are never repaired.
//! The writer lock of a prepared journal is taken before any inventory
//! judgement, so a live owner is reported Busy and never Corrupt. A lock whose
//! preparation is absent is judged without taking its flock: it is a lone lock
//! (Corrupt, preserved) or a creator between its exclusive create and its
//! preparation, which an opener must never preempt; such an opener sees a
//! transient Corrupt and the creator finishes. An observe-only reading creates
//! nothing.
use super::*;
use crate::control_fs::PrivateEntryIdentity;
use std::ffi::OsStr;

pub(crate) struct SessionJournalProtection {
    files: Vec<PrivateFile>,
    secrets: Vec<String>,
    excluded: Vec<(Directory, String, PrivateEntryIdentity)>,
}
impl SessionJournalProtection {
    pub(crate) fn new(files: Vec<PrivateFile>, secrets: Vec<String>) -> SessionResult<Self> {
        if files.len() > 64
            || secrets.len() > 64
            || secrets.iter().any(|s| s.is_empty() || s.len() > 4096)
            || secrets.iter().map(String::len).sum::<usize>() > 256 * 1024
        {
            return Err(SessionError::Capacity);
        }
        Ok(Self {
            files,
            secrets,
            excluded: Vec::new(),
        })
    }
    pub(crate) fn exclude_known_file(
        &mut self,
        parent: &Directory,
        name: &str,
    ) -> SessionResult<()> {
        if self.files.len() + self.excluded.len() == 64 {
            return Err(SessionError::Capacity);
        }
        let identity = parent
            .private_entry(OsStr::new(name))
            .map_err(|_| SessionError::UnsafeMaterial)?
            .ok_or(SessionError::UnsafeMaterial)?;
        self.excluded.push((parent.clone(), name.into(), identity));
        Ok(())
    }
    fn screen_value(&self, value: &Value) -> SessionResult<()> {
        match value {
            Value::String(s) => self.screen_literal(s.as_bytes()),
            Value::Array(v) => {
                for value in v {
                    self.screen_value(value)?;
                }
                Ok(())
            }
            Value::Object(v) => {
                for (key, value) in v {
                    self.screen_literal(key.as_bytes())?;
                    self.screen_value(value)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    fn screen_literal(&self, bytes: &[u8]) -> SessionResult<()> {
        if self
            .secrets
            .iter()
            .any(|s| bytes.windows(s.len()).any(|w| w == s.as_bytes()))
        {
            return Err(SessionError::UnsafeMaterial);
        }
        Ok(())
    }
    pub(super) fn screen(&self, bytes: &[u8]) -> SessionResult<()> {
        self.screen_literal(bytes)?;
        self.screen_value(
            &crate::json::decode_unique(bytes, 64).map_err(|_| SessionError::Corrupt)?,
        )
    }
    fn exclusions(&self, root: &Directory) -> SessionResult<Vec<PrivateEntryIdentity>> {
        root.revalidate().map_err(|_| SessionError::Corrupt)?;
        self.screen_literal(root.path().as_os_str().as_encoded_bytes())?;
        let mut output = Vec::new();
        for file in &self.files {
            file.persistent_revalidate()
                .map_err(|_| SessionError::UnsafeMaterial)?;
            let parent =
                Directory::open_existing(file.path().parent().ok_or(SessionError::UnsafeMaterial)?)
                    .map_err(|_| SessionError::UnsafeMaterial)?;
            let id = parent
                .private_entry(
                    file.path()
                        .file_name()
                        .ok_or(SessionError::UnsafeMaterial)?,
                )
                .map_err(|_| SessionError::UnsafeMaterial)?
                .ok_or(SessionError::UnsafeMaterial)?;
            output.push(id);
        }
        for (parent, name, identity) in &self.excluded {
            parent
                .revalidate()
                .map_err(|_| SessionError::UnsafeMaterial)?;
            if parent
                .private_entry(OsStr::new(name))
                .map_err(|_| SessionError::UnsafeMaterial)?
                != Some(*identity)
            {
                return Err(SessionError::UnsafeMaterial);
            }
            output.push(*identity);
        }
        Ok(output)
    }
}
const LOCK: &str = ".session-v2.lock";
const PREPARED: &str = "prepared-v2.json";
const WORKSPACE: &str = "workspace-v2.json";
const POD: &str = "pod-v2.json";
const ABANDONED: &str = "abandoned-v2.json";

struct Retained {
    file: PrivateFile,
    id: PrivateEntryIdentity,
}
pub(super) struct SessionDisk {
    root: Directory,
    protection: SessionJournalProtection,
    lock: PrivateFile,
    pub prepared: PrivateFile,
    lock_id: PrivateEntryIdentity,
    prepared_id: PrivateEntryIdentity,
    workspace: Option<Retained>,
    pod: Option<Retained>,
    abandoned: Option<Retained>,
}
/// Bytes of one observe-only reading, copied while the writer lock was held.
pub(super) struct RetainedBytes {
    pub prepared: Vec<u8>,
    pub workspace: Option<Vec<u8>>,
    pub pod: Option<Vec<u8>>,
    pub abandoned: Option<Vec<u8>>,
}

/// Fixed inventory r2: lock, preparation, workspace claim, exact request and
/// the before-effect abandonment. Unknown names, including future effect
/// checkpoints, are refused as Corrupt and preserved, so an older reader also
/// fails closed.
#[derive(Clone, Copy)]
struct Names {
    lock: bool,
    prepared: bool,
    workspace: bool,
    pod: bool,
    abandoned: bool,
}
/// Presence of the workspace claim, the exact request and the abandonment.
type Chain = (bool, bool, bool);
impl Names {
    fn is_empty(self) -> bool {
        !(self.lock || self.prepared || self.workspace || self.pod || self.abandoned)
    }
    fn only_lock(self) -> bool {
        self.lock && !(self.prepared || self.workspace || self.pod || self.abandoned)
    }
    /// Every present file requires all of its predecessors; the abandonment
    /// may follow the preparation in any before-effect phase.
    fn chain(self) -> SessionResult<Chain> {
        if !self.lock || !self.prepared || (self.pod && !self.workspace) {
            return Err(SessionError::Corrupt);
        }
        Ok((self.workspace, self.pod, self.abandoned))
    }
}
fn inventory(root: &Directory) -> SessionResult<Names> {
    let mut names = Names {
        lock: false,
        prepared: false,
        workspace: false,
        pod: false,
        abandoned: false,
    };
    for entry in root
        .persistent_entries(5)
        .map_err(|_| SessionError::Corrupt)?
    {
        let (present, maximum) = match entry.name.as_str() {
            LOCK => (&mut names.lock, 0),
            PREPARED => (&mut names.prepared, MAX_PREPARATION),
            WORKSPACE => (&mut names.workspace, MAX_WORKSPACE_RECORD),
            POD => (&mut names.pod, MAX_POD_REQUEST),
            ABANDONED => (&mut names.abandoned, MAX_ABANDONMENT),
            _ => return Err(SessionError::Corrupt),
        };
        if entry.directory || entry.bytes > maximum || *present {
            return Err(SessionError::Corrupt);
        }
        *present = true;
    }
    Ok(names)
}
fn read(
    root: &Directory,
    name: &str,
    maximum: u64,
    excluded: &[PrivateEntryIdentity],
) -> SessionResult<PrivateFile> {
    root.persistent_read(name, maximum, excluded)
        .map_err(|_| SessionError::Corrupt)?
        .ok_or(SessionError::Corrupt)
}
fn identity(root: &Directory, name: &str) -> SessionResult<PrivateEntryIdentity> {
    root.private_entry(OsStr::new(name))
        .map_err(|_| SessionError::Corrupt)?
        .ok_or(SessionError::Corrupt)
}
fn retained(
    root: &Directory,
    name: &str,
    maximum: u64,
    excluded: &[PrivateEntryIdentity],
) -> SessionResult<Retained> {
    let file = read(root, name, maximum, excluded)?;
    Ok(Retained {
        file,
        id: identity(root, name)?,
    })
}
/// The writer lock is created only in an empty directory. A lock that already
/// exists, including one created concurrently, takes the open path, which
/// checks its preparation before any flock.
fn create_lock(
    root: &Directory,
    excluded: &[PrivateEntryIdentity],
) -> SessionResult<(PrivateFile, bool)> {
    let names = inventory(root)?;
    if names.lock {
        return open_lock(root, excluded);
    }
    if !names.is_empty() {
        return Err(SessionError::Corrupt);
    }
    root.durable_bootstrap()
        .map_err(|_| SessionError::Unavailable)?;
    match root.persistent_create(LOCK, &[]) {
        Ok(lock) => {
            lock.persistent_sync()
                .map_err(|_| SessionError::Unavailable)?;
            root.sync().map_err(|_| SessionError::Unavailable)?;
            Ok((lock, true))
        }
        Err(_) => open_lock(root, excluded),
    }
}
fn open_lock(
    root: &Directory,
    excluded: &[PrivateEntryIdentity],
) -> SessionResult<(PrivateFile, bool)> {
    root.persistent_read(LOCK, 0, excluded)
        .map_err(|_| SessionError::Unavailable)?
        .map(|lock| (lock, false))
        .ok_or(SessionError::Unavailable)
}
/// An opener never takes the flock of a lock whose preparation is absent: it
/// is a lone lock or the window of a creator that holds no preparation yet,
/// and that creator must be able to finish. A creator writes its preparation
/// only after its own flock, so a live owner is always prepared.
fn require_preparation(root: &Directory) -> SessionResult<()> {
    identity(root, PREPARED).map(|_| ())
}
impl SessionDisk {
    pub fn acquire(
        root: Directory,
        protection: SessionJournalProtection,
        bytes: &[u8],
    ) -> SessionResult<Self> {
        let excluded = protection.exclusions(&root)?;
        let (lock, created) = match root
            .persistent_read(LOCK, 0, &excluded)
            .map_err(|_| SessionError::Corrupt)?
        {
            Some(lock) => (lock, false),
            None => create_lock(&root, &excluded)?,
        };
        if !created {
            require_preparation(&root)?;
        }
        if !lock
            .persistent_try_lock()
            .map_err(|_| SessionError::Unavailable)?
        {
            return Err(SessionError::Busy);
        }
        // Only the flock owner judges the inventory: a live owner is Busy,
        // never Corrupt. A lone lock stays Corrupt and is never repaired.
        let names = inventory(&root)?;
        // Existing bytes are never overwritten, even for a different confirmed
        // reservation. New files are exclusive and remain after failed sync.
        let (prepared, workspace, pod, abandoned) = if created {
            if !names.only_lock() {
                return Err(SessionError::Corrupt);
            }
            let file = root
                .persistent_create(PREPARED, bytes)
                .map_err(|_| SessionError::Unavailable)?;
            file.persistent_sync()
                .map_err(|_| SessionError::Unavailable)?;
            root.sync().map_err(|_| SessionError::Unavailable)?;
            (file, None, None, None)
        } else {
            let (workspace, pod, abandoned) = names.chain()?;
            let file = read(&root, PREPARED, MAX_PREPARATION, &excluded)?;
            if file.bytes() != bytes {
                return Err(SessionError::Conflict);
            }
            let workspace = workspace
                .then(|| retained(&root, WORKSPACE, MAX_WORKSPACE_RECORD, &excluded))
                .transpose()?;
            let pod = pod
                .then(|| retained(&root, POD, MAX_POD_REQUEST, &excluded))
                .transpose()?;
            let abandoned = abandoned
                .then(|| retained(&root, ABANDONED, MAX_ABANDONMENT, &excluded))
                .transpose()?;
            (file, workspace, pod, abandoned)
        };
        let lock_id = identity(&root, LOCK)?;
        let prepared_id = identity(&root, PREPARED)?;
        // Reopening the same fully present bytes after an ambiguous earlier
        // fsync must establish durability again, never infer it from visibility.
        lock.persistent_sync()
            .map_err(|_| SessionError::Unavailable)?;
        prepared
            .persistent_sync()
            .map_err(|_| SessionError::Unavailable)?;
        for file in [&workspace, &pod, &abandoned].into_iter().flatten() {
            file.file
                .persistent_sync()
                .map_err(|_| SessionError::Unavailable)?;
        }
        root.sync().map_err(|_| SessionError::Unavailable)?;
        let disk = Self {
            root,
            protection,
            lock,
            prepared,
            lock_id,
            prepared_id,
            workspace,
            pod,
            abandoned,
        };
        disk.revalidate()?;
        Ok(disk)
    }

    /// Observe-only reading: takes the flock of an existing prepared lock
    /// (Busy for a live owner), judges the inventory, copies the retained
    /// bytes and drops the lock before returning. It never creates a file;
    /// an empty directory is not a journal.
    pub fn inspect(
        root: Directory,
        protection: SessionJournalProtection,
    ) -> SessionResult<RetainedBytes> {
        let excluded = protection.exclusions(&root)?;
        let Some(lock) = root
            .persistent_read(LOCK, 0, &excluded)
            .map_err(|_| SessionError::Corrupt)?
        else {
            return Err(if inventory(&root)?.is_empty() {
                SessionError::InvalidInput
            } else {
                SessionError::Corrupt
            });
        };
        require_preparation(&root)?;
        if !lock
            .persistent_try_lock()
            .map_err(|_| SessionError::Unavailable)?
        {
            return Err(SessionError::Busy);
        }
        let chain = inventory(&root)?.chain()?;
        let copy = |name: &str, maximum: u64| -> SessionResult<Vec<u8>> {
            let file = read(&root, name, maximum, &excluded)?;
            protection.screen(file.bytes())?;
            Ok(file.bytes().to_vec())
        };
        let snapshot = RetainedBytes {
            prepared: copy(PREPARED, MAX_PREPARATION)?,
            workspace: chain
                .0
                .then(|| copy(WORKSPACE, MAX_WORKSPACE_RECORD))
                .transpose()?,
            pod: chain.1.then(|| copy(POD, MAX_POD_REQUEST)).transpose()?,
            abandoned: chain
                .2
                .then(|| copy(ABANDONED, MAX_ABANDONMENT))
                .transpose()?,
        };
        if inventory(&root)?.chain()? != chain {
            return Err(SessionError::Corrupt);
        }
        drop(lock);
        Ok(snapshot)
    }

    pub fn screen(&self, bytes: &[u8]) -> SessionResult<()> {
        self.protection.screen(bytes)
    }
    pub fn workspace_bytes(&self) -> Option<&[u8]> {
        self.workspace
            .as_ref()
            .map(|retained| retained.file.bytes())
    }
    pub fn pod_bytes(&self) -> Option<&[u8]> {
        self.pod.as_ref().map(|retained| retained.file.bytes())
    }
    pub fn abandoned_bytes(&self) -> Option<&[u8]> {
        self.abandoned
            .as_ref()
            .map(|retained| retained.file.bytes())
    }
    /// Creates the next file only; never rewrites, renames or removes one.
    /// Nothing follows a retained abandonment.
    pub fn retain_workspace(&mut self, bytes: &[u8]) -> SessionResult<()> {
        if self.workspace.is_some() || self.pod.is_some() || self.abandoned.is_some() {
            return Err(SessionError::Conflict);
        }
        self.workspace = Some(self.create(WORKSPACE, bytes)?);
        self.revalidate()
    }
    pub fn retain_pod(&mut self, bytes: &[u8]) -> SessionResult<()> {
        if self.workspace.is_none() || self.pod.is_some() || self.abandoned.is_some() {
            return Err(SessionError::Conflict);
        }
        self.pod = Some(self.create(POD, bytes)?);
        self.revalidate()
    }
    pub fn retain_abandoned(&mut self, bytes: &[u8]) -> SessionResult<()> {
        if self.abandoned.is_some() {
            return Err(SessionError::Conflict);
        }
        self.abandoned = Some(self.create(ABANDONED, bytes)?);
        self.revalidate()
    }
    fn create(&self, name: &str, bytes: &[u8]) -> SessionResult<Retained> {
        self.revalidate()?;
        let file = self
            .root
            .persistent_create(name, bytes)
            .map_err(|_| SessionError::Unavailable)?;
        file.persistent_sync()
            .map_err(|_| SessionError::Unavailable)?;
        self.root.sync().map_err(|_| SessionError::Unavailable)?;
        Ok(Retained {
            file,
            id: identity(&self.root, name)?,
        })
    }

    pub fn revalidate(&self) -> SessionResult<()> {
        let excluded = self.protection.exclusions(&self.root)?;
        if inventory(&self.root)?.chain()?
            != (
                self.workspace.is_some(),
                self.pod.is_some(),
                self.abandoned.is_some(),
            )
        {
            return Err(SessionError::Corrupt);
        }
        let mut files = vec![
            (LOCK, self.lock_id, &self.lock, 0),
            (PREPARED, self.prepared_id, &self.prepared, MAX_PREPARATION),
        ];
        if let Some(retained) = &self.workspace {
            files.push((WORKSPACE, retained.id, &retained.file, MAX_WORKSPACE_RECORD));
        }
        if let Some(retained) = &self.pod {
            files.push((POD, retained.id, &retained.file, MAX_POD_REQUEST));
        }
        if let Some(retained) = &self.abandoned {
            files.push((ABANDONED, retained.id, &retained.file, MAX_ABANDONMENT));
        }
        for (name, expected, retained, max) in files {
            if self
                .root
                .private_entry(OsStr::new(name))
                .map_err(|_| SessionError::Corrupt)?
                != Some(expected)
            {
                return Err(SessionError::Corrupt);
            }
            retained
                .persistent_revalidate()
                .map_err(|_| SessionError::Corrupt)?;
            let file = self
                .root
                .persistent_read(name, max, &excluded)
                .map_err(|_| SessionError::Corrupt)?
                .ok_or(SessionError::Corrupt)?;
            if file.bytes() != retained.bytes() {
                return Err(SessionError::Corrupt);
            }
            if name != LOCK {
                self.protection.screen(retained.bytes())?;
            }
        }
        self.root.revalidate().map_err(|_| SessionError::Corrupt)
    }
}
