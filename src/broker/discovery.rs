//! Leases and append-only transport announcements, not business authorization.
//! Role roots are exclusive private namespaces (broker and enforcer are distinct).
//! A replacement process announces a new endpoint without unlinking old sockets
//! or changing any job, grant, claim, deadline or credential.

use super::local_ipc::{EndpointRecordV1, IpcError, IpcResult};
use crate::control_fs::{Directory, PrivateFile};
use serde::{Deserialize, Serialize};
use std::fmt;

const MAX_ANNOUNCEMENTS: u32 = 128;
const MAX_RECORD_BYTES: u64 = 4096;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnnouncementV1 {
    version: u32,
    sequence: u32,
    endpoint: EndpointRecordV1,
}

pub(crate) struct RoleLease {
    root: Directory,
    lock: PrivateFile,
}
impl fmt::Debug for RoleLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RoleLease(<private>)")
    }
}

fn fs_error(error: anyhow::Error) -> IpcError {
    if error.is::<crate::control_fs::persistent::PersistentLimit>() {
        IpcError::LimitExceeded
    } else {
        IpcError::UnsafeFilesystem
    }
}

impl RoleLease {
    pub(crate) fn acquire(root: &Directory) -> IpcResult<Self> {
        root.revalidate().map_err(fs_error)?;
        let lock = match root
            .persistent_read(".owner.lock", 0, &[])
            .map_err(fs_error)?
        {
            Some(file) => file,
            None => match root.persistent_create(".owner.lock", &[]) {
                Ok(file) => file,
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists) =>
                {
                    root.persistent_read(".owner.lock", 0, &[])
                        .map_err(fs_error)?
                        .ok_or(IpcError::Unavailable)?
                }
                Err(error) => return Err(fs_error(error)),
            },
        };
        if !lock.persistent_try_lock().map_err(fs_error)? {
            return Err(IpcError::Conflict);
        }
        lock.persistent_sync().map_err(fs_error)?;
        root.durable_bootstrap().map_err(fs_error)?;
        Ok(Self {
            root: root.clone(),
            lock,
        })
    }

    fn revalidate(&self) -> IpcResult<()> {
        self.root.revalidate().map_err(fs_error)?;
        self.lock.persistent_revalidate().map_err(fs_error)
    }

    pub(crate) fn listener_root(&self) -> IpcResult<Directory> {
        self.revalidate()?;
        self.root
            .persistent_child("endpoints", true)
            .map_err(fs_error)?
            .ok_or(IpcError::Unavailable)
    }

    pub(crate) fn publish_endpoint(&self, endpoint: &EndpointRecordV1) -> IpcResult<()> {
        self.revalidate()?;
        let endpoints = self.listener_root()?;
        // Check the real retained namespace/entry before publishing its record.
        // Publication only writes metadata, never a role or permission.
        endpoint.validate_at(&endpoints)?;
        let announcements = self
            .root
            .persistent_child("announcements", true)
            .map_err(fs_error)?
            .ok_or(IpcError::Unavailable)?;
        let highest = highest_sequence(&announcements)?;
        let next = highest.checked_add(1).ok_or(IpcError::LimitExceeded)?;
        if next > MAX_ANNOUNCEMENTS {
            return Err(IpcError::LimitExceeded);
        }
        let record = AnnouncementV1 {
            version: 1,
            sequence: next,
            endpoint: endpoint.clone(),
        };
        let bytes = serde_json::to_vec(&record).map_err(|_| IpcError::InvalidInput)?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(IpcError::TooLarge);
        }
        // No mutable pointer or overwrite. A partial prior announcement remains
        // evidence; a new leased process can announce its own new transport.
        // Clients never fall back from a partial highest announcement.
        let file = announcements
            .persistent_create(&record_name(next), &bytes)
            .map_err(fs_error)?;
        file.persistent_sync().map_err(fs_error)?;
        announcements.sync().map_err(fs_error)?;
        self.revalidate()
    }
}

fn record_name(sequence: u32) -> String {
    format!("e-{sequence:04}.json")
}

fn highest_sequence(announcements: &Directory) -> IpcResult<u32> {
    let entries = announcements
        .persistent_entries(MAX_ANNOUNCEMENTS)
        .map_err(fs_error)?;
    let mut sequences = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.directory || entry.bytes > MAX_RECORD_BYTES {
            return Err(IpcError::UnsafeFilesystem);
        }
        let sequence: u32 = entry
            .name
            .strip_prefix("e-")
            .and_then(|name| name.strip_suffix(".json"))
            .and_then(|number| number.parse().ok())
            .ok_or(IpcError::UnsafeFilesystem)?;
        if sequence == 0 || sequence > MAX_ANNOUNCEMENTS || record_name(sequence) != entry.name {
            return Err(IpcError::UnsafeFilesystem);
        }
        sequences.push(sequence);
    }
    sequences.sort_unstable();
    for (index, sequence) in sequences.iter().enumerate() {
        if usize::try_from(*sequence).map_err(|_| IpcError::UnsafeFilesystem)? != index + 1 {
            return Err(IpcError::UnsafeFilesystem);
        }
    }
    Ok(sequences.last().copied().unwrap_or(0))
}

/// Reading discovery never creates a role, acquires a lease or starts a process.
pub(crate) fn load_endpoint(role_root: &Directory) -> IpcResult<Option<EndpointRecordV1>> {
    role_root.revalidate().map_err(fs_error)?;
    let Some(announcements) = role_root
        .persistent_child("announcements", false)
        .map_err(fs_error)?
    else {
        return Ok(None);
    };
    let sequence = highest_sequence(&announcements)?;
    if sequence == 0 {
        return Ok(None);
    }
    let file = announcements
        .persistent_read(&record_name(sequence), MAX_RECORD_BYTES, &[])
        .map_err(fs_error)?
        .ok_or(IpcError::UnsafeFilesystem)?;
    let record: AnnouncementV1 =
        serde_json::from_slice(file.bytes()).map_err(|_| IpcError::UnsafeFilesystem)?;
    if record.version != 1
        || record.sequence != sequence
        || serde_json::to_vec(&record).map_err(|_| IpcError::UnsafeFilesystem)? != file.bytes()
    {
        return Err(IpcError::UnsafeFilesystem);
    }
    role_root.revalidate().map_err(fs_error)?;
    Ok(Some(record.endpoint))
}
