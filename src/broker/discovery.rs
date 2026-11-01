//! Leases and append-only transport announcements, not business authorization.
//! Role roots are exclusive private namespaces (broker and enforcer are distinct).
//! A replacement process announces a new endpoint without unlinking old sockets
//! or changing any job, grant, claim, deadline or credential.

use super::local_ipc::{EndpointRecordV1, IpcError, IpcResult};
use crate::control_fs::{Directory, PrivateFile};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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

/// Explicit recovery transport only. This does not parse a ledger, establish a
/// ledger identity, create a role namespace or grant Full admission. Clones
/// keep the same O_RDWR descriptor/flock alive for retained STOP actors.
#[derive(Clone)]
pub(crate) struct EnforcerRoleLeaseV2 {
    root: Directory,
    lock: crate::control_fs::persistent::EnforcerLeaseFile,
}

impl fmt::Debug for EnforcerRoleLeaseV2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EnforcerRoleLeaseV2(<private recovery transport>)")
    }
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
    /// Recovery of a broker V1 role never creates a lock or bootstraps storage.
    /// The empty-file contract remains V1; nonempty V2 locks are not upgraded
    /// or reinterpreted by this path.
    pub(crate) fn acquire_existing(root: &Directory) -> IpcResult<Self> {
        root.revalidate().map_err(fs_error)?;
        let lock = root
            .persistent_read(".owner.lock", 0, &[])
            .map_err(fs_error)?
            .ok_or(IpcError::UnsafeFilesystem)?;
        if !lock.persistent_try_lock().map_err(fs_error)? {
            return Err(IpcError::Conflict);
        }
        let lease = Self {
            root: root.clone(),
            lock,
        };
        lease.revalidate()?;
        Ok(lease)
    }

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

    pub(crate) fn listener_root_existing(&self) -> IpcResult<Directory> {
        self.revalidate()?;
        self.root
            .persistent_child("endpoints", false)
            .map_err(fs_error)?
            .ok_or(IpcError::UnsafeFilesystem)
    }

    pub(crate) fn publish_endpoint_existing(&self, endpoint: &EndpointRecordV1) -> IpcResult<()> {
        self.revalidate()?;
        publish_endpoint(&self.root, endpoint, false)?;
        self.revalidate()
    }

    pub(crate) fn publish_endpoint(&self, endpoint: &EndpointRecordV1) -> IpcResult<()> {
        self.revalidate()?;
        publish_endpoint(&self.root, endpoint, true)?;
        self.revalidate()
    }
}

impl EnforcerRoleLeaseV2 {
    /// Explicitly join an existing role namespace, never a fresh factory path.
    /// An empty legacy lock may support this restricted transport but does not
    /// become a V2 retirement authority or obtain a verified ledger identity.
    pub(crate) fn acquire_existing(root: &Directory) -> IpcResult<Self> {
        let lock = crate::control_fs::persistent::EnforcerLeaseFile::acquire_existing(root)
            .map_err(fs_error)?
            .ok_or(IpcError::Conflict)?;
        let lease = Self {
            root: root.clone(),
            lock,
        };
        // Recovery neither creates missing namespaces nor bootstraps storage.
        lease.listener_root()?;
        lease
            .root
            .persistent_child("announcements", false)
            .map_err(fs_error)?
            .ok_or(IpcError::UnsafeFilesystem)?;
        lease.revalidate()?;
        Ok(lease)
    }

    pub(crate) fn revalidate(&self) -> IpcResult<()> {
        self.root.revalidate().map_err(fs_error)?;
        self.lock.revalidate().map_err(fs_error)
    }

    pub(crate) fn listener_root(&self) -> IpcResult<Directory> {
        self.revalidate()?;
        self.root
            .persistent_child("endpoints", false)
            .map_err(fs_error)?
            .ok_or(IpcError::UnsafeFilesystem)
    }

    pub(crate) fn publish_endpoint(&self, endpoint: &EndpointRecordV1) -> IpcResult<()> {
        self.revalidate()?;
        publish_endpoint(&self.root, endpoint, false)?;
        self.revalidate()
    }

    pub(crate) fn root_binding(&self, endpoint: &EndpointRecordV1) -> IpcResult<String> {
        let native = self.lock.native_binding().map_err(fs_error)?;
        let endpoints = self.listener_root()?;
        endpoint.validate_at(&endpoints)?;
        let binding = root_binding(native, endpoint)?;
        endpoint.validate_at(&endpoints)?;
        self.revalidate()?;
        Ok(binding)
    }
}

/// Client-side expectation for the explicit recovery handshake. It inspects
/// the root/lock entry and checked endpoint without opening or locking the
/// regular lock inode, and cannot create any authority or discover Full.
pub(crate) fn recovery_root_binding(
    root: &Directory,
    endpoint: &EndpointRecordV1,
) -> IpcResult<String> {
    let native = root.enforcer_recovery_binding().map_err(fs_error)?;
    let endpoints = root
        .persistent_child("endpoints", false)
        .map_err(fs_error)?
        .ok_or(IpcError::UnsafeFilesystem)?;
    endpoint.validate_at(&endpoints)?;
    let binding = root_binding(native, endpoint)?;
    endpoint.validate_at(&endpoints)?;
    if root.enforcer_recovery_binding().map_err(fs_error)? != native {
        return Err(IpcError::UnsafeFilesystem);
    }
    root.revalidate().map_err(fs_error)?;
    Ok(binding)
}

fn root_binding(native: [u64; 6], endpoint: &EndpointRecordV1) -> IpcResult<String> {
    let endpoint = serde_json::to_vec(endpoint).map_err(|_| IpcError::InvalidInput)?;
    let mut digest = Sha256::new();
    digest.update(b"ai-guard:enforcer-root-binding:v2\0");
    for value in native {
        digest.update(value.to_be_bytes());
    }
    digest.update((endpoint.len() as u64).to_be_bytes());
    digest.update(endpoint);
    Ok(hex::encode(digest.finalize()))
}

fn publish_endpoint(
    root: &Directory,
    endpoint: &EndpointRecordV1,
    create_namespaces: bool,
) -> IpcResult<()> {
    let endpoints = root
        .persistent_child("endpoints", create_namespaces)
        .map_err(fs_error)?
        .ok_or(IpcError::UnsafeFilesystem)?;
    // Publication only writes metadata for the verified retained namespace.
    endpoint.validate_at(&endpoints)?;
    let announcements = root
        .persistent_child("announcements", create_namespaces)
        .map_err(fs_error)?
        .ok_or(IpcError::UnsafeFilesystem)?;
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
    // No mutable pointer or overwrite. Partial evidence is never repaired;
    // clients never fall back from a partial highest announcement.
    let file = announcements
        .persistent_create(&record_name(next), &bytes)
        .map_err(fs_error)?;
    file.persistent_sync().map_err(fs_error)?;
    announcements.sync().map_err(fs_error)?;
    endpoint.validate_at(&endpoints)
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
