//! Fixed inventory and retained identities. No deletion, replacement or repair.
use super::*;
use crate::control_fs::PrivateEntryIdentity;
use std::{collections::BTreeMap, ffi::OsStr};

pub(super) struct ExcludedFile {
    parent: Directory,
    name: String,
    identity: PrivateEntryIdentity,
}
pub(super) struct FileCommit {
    identity: PrivateEntryIdentity,
    digest: String,
    bytes: u64,
}
pub(super) struct Disk {
    pub root: Directory,
    pub protection: AdmissionJournalProtection,
    // Clones are never exposed; this retained description owns the flock.
    lock: PrivateFile,
    pub known: BTreeMap<String, FileCommit>,
}

impl AdmissionJournalProtection {
    /// Metadata-only capture for SQLite and other protected identities. Never
    /// open a database inode through PrivateFile merely to exclude it.
    pub(crate) fn exclude_known_file(
        &mut self,
        parent: &Directory,
        name: &str,
    ) -> JournalResult<()> {
        let identity = parent
            .private_entry(OsStr::new(name))
            .map_err(|_| JournalError::UnsafeMaterial)?
            .ok_or(JournalError::InvalidInput)?;
        if !self.excluded.iter().any(|file| file.identity == identity) {
            if self.excluded.len() >= 64 {
                return Err(JournalError::Capacity);
            }
            self.excluded.push(ExcludedFile {
                parent: parent.clone(),
                name: name.into(),
                identity,
            });
        }
        Ok(())
    }
    pub(super) fn exclusions(&self, root: &Directory) -> JournalResult<Vec<PrivateEntryIdentity>> {
        if self.secrets.len() > 64
            || self
                .secrets
                .iter()
                .any(|secret| secret.is_empty() || secret.len() > 4096)
            || self.secrets.iter().map(String::len).sum::<usize>() > 256 * 1024
        {
            return Err(JournalError::Capacity);
        }
        root.revalidate().map_err(|_| JournalError::Corrupt)?;
        self.screen_path(root.path())?;
        let mut identities = Vec::new();
        for file in &self.files {
            file.revalidate()
                .map_err(|_| JournalError::UnsafeMaterial)?;
            self.screen_path(file.path())?;
            let parent =
                Directory::open_existing(file.path().parent().ok_or(JournalError::InvalidInput)?)
                    .map_err(|_| JournalError::UnsafeMaterial)?;
            let identity = parent
                .private_entry(file.path().file_name().ok_or(JournalError::InvalidInput)?)
                .map_err(|_| JournalError::UnsafeMaterial)?
                .ok_or(JournalError::UnsafeMaterial)?;
            if !identities.contains(&identity) {
                identities.push(identity);
                if identities.len() > 64 {
                    return Err(JournalError::Capacity);
                }
            }
        }
        for file in &self.excluded {
            file.parent
                .revalidate()
                .map_err(|_| JournalError::UnsafeMaterial)?;
            self.screen_path(file.parent.path())?;
            self.screen_path(&file.parent.path().join(&file.name))?;
            if file
                .parent
                .private_entry(OsStr::new(&file.name))
                .map_err(|_| JournalError::UnsafeMaterial)?
                != Some(file.identity)
            {
                return Err(JournalError::UnsafeMaterial);
            }
            if !identities.contains(&file.identity) {
                identities.push(file.identity);
            }
        }
        if identities.len() > 64 {
            return Err(JournalError::Capacity);
        }
        Ok(identities)
    }
    fn screen_path(&self, path: &std::path::Path) -> JournalResult<()> {
        records::screen(
            path.to_str()
                .ok_or(JournalError::UnsafeMaterial)?
                .as_bytes(),
            &self.secrets,
        )
    }
    pub(super) fn screen_record(&self, bytes: &[u8]) -> JournalResult<()> {
        records::screen(bytes, &self.secrets)?;
        let value = super::super::json::decode(bytes).map_err(|_| JournalError::Corrupt)?;
        records::screen_value(&value, &self.secrets)
    }
}

pub(super) fn inventory(root: &Directory) -> JournalResult<Vec<String>> {
    let entries = root
        .persistent_entries(records::FILES.len() as u32)
        .map_err(|_| JournalError::Corrupt)?;
    let mut total = 0u64;
    let mut names = Vec::new();
    for entry in entries {
        let maximum = records::FILES
            .iter()
            .find(|(name, _)| *name == entry.name)
            .map(|(_, maximum)| *maximum)
            .ok_or(JournalError::Corrupt)?;
        if entry.directory || entry.bytes > maximum {
            return Err(JournalError::Corrupt);
        }
        total = total
            .checked_add(entry.bytes)
            .ok_or(JournalError::Corrupt)?;
        names.push(entry.name);
    }
    if total > records::TOTAL_BYTES {
        return Err(JournalError::Corrupt);
    }
    Ok(names)
}

impl Disk {
    pub fn acquire(
        root: Directory,
        protection: AdmissionJournalProtection,
        create: bool,
    ) -> JournalResult<Option<Self>> {
        let exclusions = protection.exclusions(&root)?;
        let entries = inventory(&root)?;
        if entries.is_empty() && !create {
            return Ok(None);
        }
        if create && !entries.is_empty() {
            return Err(JournalError::Conflict);
        }
        if !create && !entries.iter().any(|name| name == ".writer.lock") {
            return Err(JournalError::Corrupt);
        }
        let lock = if create {
            root.durable_bootstrap()
                .map_err(|_| JournalError::Unavailable)?;
            let lock = root
                .persistent_create(".writer.lock", &[])
                .map_err(|_| JournalError::Unavailable)?;
            lock.persistent_sync()
                .map_err(|_| JournalError::Unavailable)?;
            root.sync().map_err(|_| JournalError::Unavailable)?;
            lock
        } else {
            root.persistent_read(".writer.lock", 0, &exclusions)
                .map_err(|_| JournalError::Corrupt)?
                .ok_or(JournalError::Corrupt)?
        };
        if !lock
            .persistent_try_lock()
            .map_err(|_| JournalError::Unavailable)?
        {
            return Err(JournalError::Busy);
        }
        let identity = root
            .private_entry(OsStr::new(".writer.lock"))
            .map_err(|_| JournalError::Corrupt)?
            .ok_or(JournalError::Corrupt)?;
        lock.persistent_revalidate()
            .map_err(|_| JournalError::Corrupt)?;
        let mut known = BTreeMap::new();
        known.insert(
            ".writer.lock".into(),
            FileCommit {
                identity,
                digest: records::digest(&[]),
                bytes: 0,
            },
        );
        let result = Self {
            root,
            protection,
            lock,
            known,
        };
        Ok(Some(result))
    }
    pub fn read(&self, name: &str) -> JournalResult<Option<PrivateFile>> {
        let maximum = records::FILES
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, maximum)| *maximum)
            .ok_or(JournalError::InvalidInput)?;
        let exclusions = self.protection.exclusions(&self.root)?;
        self.root
            .persistent_read(name, maximum, &exclusions)
            .map_err(|_| JournalError::Corrupt)
    }
    pub fn remember_file(&mut self, name: &str, file: &PrivateFile) -> JournalResult<()> {
        let identity = self
            .root
            .private_entry(OsStr::new(name))
            .map_err(|_| JournalError::Corrupt)?
            .ok_or(JournalError::Corrupt)?;
        // The named identity is captured before checking the retained descriptor
        // again, so a replacement cannot be adopted as the file just read.
        file.persistent_revalidate()
            .map_err(|_| JournalError::Corrupt)?;
        self.known.insert(
            name.into(),
            FileCommit {
                identity,
                digest: file.digest().into(),
                bytes: file.bytes().len() as u64,
            },
        );
        Ok(())
    }
    pub fn revalidate(&self) -> JournalResult<()> {
        self.protection.exclusions(&self.root)?;
        self.lock
            .persistent_revalidate()
            .map_err(|_| JournalError::Corrupt)?;
        let entries = inventory(&self.root)?;
        if entries.len() != self.known.len()
            || entries.iter().any(|name| !self.known.contains_key(name))
        {
            return Err(JournalError::Corrupt);
        }
        for (name, expected) in &self.known {
            // Excluded identities are checked BEFORE persistent_read opens an inode.
            if self
                .root
                .private_entry(OsStr::new(name))
                .map_err(|_| JournalError::Corrupt)?
                != Some(expected.identity)
            {
                return Err(JournalError::Corrupt);
            }
            let file = self.read(name)?.ok_or(JournalError::Corrupt)?;
            if file.digest() != expected.digest || file.bytes().len() as u64 != expected.bytes {
                return Err(JournalError::Corrupt);
            }
        }
        self.root.revalidate().map_err(|_| JournalError::Corrupt)
    }
    pub fn append(&mut self, name: &str, bytes: &[u8]) -> JournalResult<()> {
        self.revalidate()?;
        self.protection.screen_record(bytes)?;
        let maximum = records::FILES
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, maximum)| *maximum)
            .ok_or(JournalError::InvalidInput)?;
        if bytes.len() as u64 > maximum {
            return Err(JournalError::Capacity);
        }
        let file = if self.known.contains_key(name) {
            let file = self.read(name)?.ok_or(JournalError::Corrupt)?;
            if file.bytes() != bytes {
                return Err(JournalError::Conflict);
            }
            file
        } else {
            self.root
                .persistent_create(name, bytes)
                .map_err(|_| JournalError::Unavailable)?
        };
        file.persistent_sync()
            .map_err(|_| JournalError::Unavailable)?;
        self.root.sync().map_err(|_| JournalError::Unavailable)?;
        file.persistent_revalidate()
            .map_err(|_| JournalError::Corrupt)?;
        self.remember_file(name, &file)?;
        self.revalidate()
    }
    pub fn resynchronize(&self) -> JournalResult<()> {
        self.revalidate()?;
        for name in self.known.keys() {
            self.read(name)?
                .ok_or(JournalError::Corrupt)?
                .persistent_sync()
                .map_err(|_| JournalError::Unavailable)?;
        }
        self.root.sync().map_err(|_| JournalError::Unavailable)?;
        self.revalidate()
    }
    pub fn commitment(&self, name: &str) -> JournalResult<&str> {
        self.known
            .get(name)
            .map(|record| record.digest.as_str())
            .ok_or(JournalError::Corrupt)
    }
}
