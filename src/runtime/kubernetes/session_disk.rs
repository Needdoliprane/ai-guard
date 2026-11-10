//! Fixed private V2 inventory. Ambiguous/partial writes are never repaired.
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
pub(super) struct SessionDisk {
    root: Directory,
    protection: SessionJournalProtection,
    lock: PrivateFile,
    pub prepared: PrivateFile,
    lock_id: PrivateEntryIdentity,
    prepared_id: PrivateEntryIdentity,
}
fn inventory(root: &Directory) -> SessionResult<Vec<String>> {
    let mut names = Vec::new();
    for entry in root
        .persistent_entries(2)
        .map_err(|_| SessionError::Corrupt)?
    {
        let maximum = match entry.name.as_str() {
            ".session-v2.lock" => 0,
            "prepared-v2.json" => MAX_PREPARATION,
            _ => return Err(SessionError::Corrupt),
        };
        if entry.directory || entry.bytes > maximum {
            return Err(SessionError::Corrupt);
        }
        names.push(entry.name);
    }
    Ok(names)
}
impl SessionDisk {
    pub fn acquire(
        root: Directory,
        protection: SessionJournalProtection,
        bytes: &[u8],
    ) -> SessionResult<Self> {
        let excluded = protection.exclusions(&root)?;
        let names = inventory(&root)?;
        let create = names.is_empty();
        if !create
            && (names.len() != 2
                || !names.iter().any(|n| n == ".session-v2.lock")
                || !names.iter().any(|n| n == "prepared-v2.json"))
        {
            return Err(SessionError::Corrupt);
        }
        let lock = if create {
            root.durable_bootstrap()
                .map_err(|_| SessionError::Unavailable)?;
            let lock = root
                .persistent_create(".session-v2.lock", &[])
                .map_err(|_| SessionError::Unavailable)?;
            lock.persistent_sync()
                .map_err(|_| SessionError::Unavailable)?;
            root.sync().map_err(|_| SessionError::Unavailable)?;
            lock
        } else {
            root.persistent_read(".session-v2.lock", 0, &excluded)
                .map_err(|_| SessionError::Corrupt)?
                .ok_or(SessionError::Corrupt)?
        };
        if !lock
            .persistent_try_lock()
            .map_err(|_| SessionError::Unavailable)?
        {
            return Err(SessionError::Busy);
        }
        // Existing bytes are never overwritten, even for a different confirmed
        // reservation. New files are exclusive and remain after failed sync.
        let prepared = if create {
            let file = root
                .persistent_create("prepared-v2.json", bytes)
                .map_err(|_| SessionError::Unavailable)?;
            file.persistent_sync()
                .map_err(|_| SessionError::Unavailable)?;
            root.sync().map_err(|_| SessionError::Unavailable)?;
            file
        } else {
            let file = root
                .persistent_read("prepared-v2.json", MAX_PREPARATION, &excluded)
                .map_err(|_| SessionError::Corrupt)?
                .ok_or(SessionError::Corrupt)?;
            if file.bytes() != bytes {
                return Err(SessionError::Conflict);
            }
            file
        };
        let lock_id = root
            .private_entry(OsStr::new(".session-v2.lock"))
            .map_err(|_| SessionError::Corrupt)?
            .ok_or(SessionError::Corrupt)?;
        let prepared_id = root
            .private_entry(OsStr::new("prepared-v2.json"))
            .map_err(|_| SessionError::Corrupt)?
            .ok_or(SessionError::Corrupt)?;
        // Reopening the same fully present bytes after an ambiguous earlier
        // fsync must establish durability again, never infer it from visibility.
        lock.persistent_sync()
            .map_err(|_| SessionError::Unavailable)?;
        prepared
            .persistent_sync()
            .map_err(|_| SessionError::Unavailable)?;
        root.sync().map_err(|_| SessionError::Unavailable)?;
        let disk = Self {
            root,
            protection,
            lock,
            prepared,
            lock_id,
            prepared_id,
        };
        disk.revalidate()?;
        Ok(disk)
    }
    pub fn revalidate(&self) -> SessionResult<()> {
        let excluded = self.protection.exclusions(&self.root)?;
        if inventory(&self.root)?.len() != 2 {
            return Err(SessionError::Corrupt);
        }
        for (name, expected, retained, max) in [
            (".session-v2.lock", self.lock_id, &self.lock, 0),
            (
                "prepared-v2.json",
                self.prepared_id,
                &self.prepared,
                MAX_PREPARATION,
            ),
        ] {
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
        }
        self.protection.screen(self.prepared.bytes())?;
        self.root.revalidate().map_err(|_| SessionError::Corrupt)
    }
}
