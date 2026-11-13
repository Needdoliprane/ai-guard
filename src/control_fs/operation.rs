//! Private local operation records. These bytes confer no remote authority.
use super::{Directory, PrivateFile, absolute_path};
use std::{fmt, path::Path, sync::atomic::{AtomicBool, Ordering}};

const REQUEST_CAP: u64 = 65_536;
const RECORD_CAP: u64 = 4_096;
const NAMES: [&str; 6] = ["request.json", "metadata.json", "current.json", "current-next.json", "receipt.json", "owner.lock"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationRecord { Request, Metadata, State, PendingState, Receipt }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationFsError { UnsafeOrIncomplete, Busy, NotPublished, DurabilityUnknown }

impl fmt::Display for OperationFsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsafeOrIncomplete => "private operation is unsafe or incomplete",
            Self::Busy => "private operation is in use",
            Self::NotPublished => "private operation was not published",
            Self::DurabilityUnknown => "private operation publication is unresolved",
        })
    }
}
impl std::error::Error for OperationFsError {}
type Result<T> = std::result::Result<T, OperationFsError>;

/// One retained real flock, immutable preparation, and a CAS-published pointer.
/// Cached records remain readable after taint, but can never admit more work.
pub struct DurableOperationDirectory {
    parent: Directory,
    directory: Directory,
    lock: PrivateFile,
    request: PrivateFile,
    metadata: PrivateFile,
    state: PrivateFile,
    pending: Option<PrivateFile>,
    receipt: Option<PrivateFile>,
    tainted: AtomicBool,
}

fn safe<T>(result: anyhow::Result<T>) -> Result<T> {
    result.map_err(|_| OperationFsError::UnsafeOrIncomplete)
}
fn bounded(bytes: &[u8], cap: u64) -> Result<()> {
    if bytes.is_empty() || bytes.len() as u64 > cap {
        return Err(OperationFsError::UnsafeOrIncomplete);
    }
    Ok(())
}
fn required(directory: &Directory, name: &str, cap: u64) -> Result<PrivateFile> {
    safe(directory.persistent_read(name, cap, &[]))?
        .ok_or(OperationFsError::UnsafeOrIncomplete)
}
fn closed_inventory(directory: &Directory) -> Result<()> {
    let inventory = safe(directory.persistent_inventory(7))?;
    if inventory.incomplete || inventory.rejected != 0
        || inventory.entries.iter().any(|entry| entry.directory || !NAMES.contains(&entry.name.as_str()))
    {
        return Err(OperationFsError::UnsafeOrIncomplete);
    }
    Ok(())
}

impl DurableOperationDirectory {
    pub fn prepare_new(parent: &Directory, name: &str, request: &[u8], metadata: &[u8], initial_state: &[u8]) -> Result<()> {
        bounded(request, REQUEST_CAP)?;
        bounded(metadata, RECORD_CAP)?;
        bounded(initial_state, RECORD_CAP)?;
        // Reuse the existing component validator, and never adopt an old child.
        if safe(parent.persistent_child(name, false))?.is_some() {
            return Err(OperationFsError::NotPublished);
        }
        parent.require_absent_child(name).map_err(|_| OperationFsError::NotPublished)?;
        let stage_name = format!("operation-stage-{}", uuid::Uuid::new_v4());
        let stage = parent.exclusive_child(&stage_name).map_err(|_| OperationFsError::NotPublished)?;
        for (name, bytes) in [("request.json", request), ("metadata.json", metadata), ("current.json", initial_state), ("owner.lock", &[][..])] {
            let file = stage.persistent_create(name, bytes).map_err(|_| OperationFsError::NotPublished)?;
            file.persistent_sync().map_err(|_| OperationFsError::NotPublished)?;
        }
        stage.sync().map_err(|_| OperationFsError::NotPublished)?;
        match parent.publish_child_exclusive(&stage, name) {
            Ok(()) => safe(parent.revalidate()).map_err(|_| OperationFsError::DurabilityUnknown),
            Err(super::publication::PublicationFailure::NotPublished) => Err(OperationFsError::NotPublished),
            Err(super::publication::PublicationFailure::DurabilityUnknown) => Err(OperationFsError::DurabilityUnknown),
        }
        // No cleanup or reuse of staging anchors after its rename.
    }

    pub fn open_existing(path: &Path) -> Result<Self> {
        let path = safe(absolute_path(path))?;
        let containing = path.parent().ok_or(OperationFsError::UnsafeOrIncomplete)?;
        let name = path.file_name().and_then(|name| name.to_str()).ok_or(OperationFsError::UnsafeOrIncomplete)?;
        let parent = safe(Directory::open_existing(containing))?;
        let directory = safe(parent.persistent_child(name, false))?.ok_or(OperationFsError::UnsafeOrIncomplete)?;
        closed_inventory(&directory)?;
        let lock = required(&directory, "owner.lock", 0)?;
        if !safe(lock.persistent_try_lock())? { return Err(OperationFsError::Busy); }
        let operation = Self {
            request: required(&directory, "request.json", REQUEST_CAP)?,
            metadata: required(&directory, "metadata.json", RECORD_CAP)?,
            state: required(&directory, "current.json", RECORD_CAP)?,
            pending: safe(directory.persistent_read("current-next.json", RECORD_CAP, &[]))?,
            receipt: safe(directory.persistent_read("receipt.json", RECORD_CAP, &[]))?,
            parent, directory, lock, tainted: AtomicBool::new(false),
        };
        operation.validate_inner()?;
        // FS-C1: establish a new complete barrier under the real retained flock.
        for file in operation.files() { safe(file.persistent_sync())?; }
        safe(operation.directory.sync())?;
        safe(operation.parent.sync())?;
        #[cfg(test)]
        checkpoint(Checkpoint::ReopenAfterRealParentSync)?;
        operation.validate_inner()?;
        Ok(operation)
    }

    pub fn record(&self, which: OperationRecord) -> Option<&PrivateFile> {
        match which {
            OperationRecord::Request => Some(&self.request),
            OperationRecord::Metadata => Some(&self.metadata),
            OperationRecord::State => Some(&self.state),
            OperationRecord::PendingState => self.pending.as_ref(),
            OperationRecord::Receipt => self.receipt.as_ref(),
        }
    }

    pub fn revalidate(&self) -> Result<()> { self.finish(self.validate_inner()) }

    pub fn publish_state(&mut self, next: &[u8]) -> Result<()> {
        let result = (|| {
            self.validate_inner()?;
            bounded(next, RECORD_CAP)?;
            if self.pending.is_some() { return Err(OperationFsError::UnsafeOrIncomplete); }
            self.pending = Some(safe(self.directory.persistent_create("current-next.json", next))?);
            self.publish_pending_inner()
        })();
        self.finish(result)
    }

    pub fn publish_retained_pending_state(&mut self) -> Result<()> {
        let result = self.publish_pending_inner();
        self.finish(result)
    }

    pub fn record_receipt(&mut self, exact_receipt: &[u8]) -> Result<()> {
        let result = (|| {
            self.validate_inner()?;
            bounded(exact_receipt, RECORD_CAP)?;
            if let Some(existing) = &self.receipt {
                if existing.bytes() != exact_receipt { return Err(OperationFsError::UnsafeOrIncomplete); }
            } else {
                self.receipt = Some(safe(self.directory.persistent_create("receipt.json", exact_receipt))?);
            }
            let receipt = self.receipt.as_ref().ok_or(OperationFsError::UnsafeOrIncomplete)?;
            // FS-C2 applies equally to newly created AND identical existing bytes.
            safe(receipt.persistent_sync()).map_err(|_| OperationFsError::DurabilityUnknown)?;
            safe(self.directory.sync()).map_err(|_| OperationFsError::DurabilityUnknown)?;
            #[cfg(test)]
            checkpoint(Checkpoint::ReceiptAfterRealDirectorySync)?;
            self.validate_inner().map_err(|_| OperationFsError::DurabilityUnknown)
        })();
        self.finish(result)
    }

    fn files(&self) -> Vec<&PrivateFile> {
        let mut files = vec![&self.lock, &self.request, &self.metadata, &self.state];
        files.extend(self.pending.iter());
        files.extend(self.receipt.iter());
        files
    }

    fn validate_inner(&self) -> Result<()> {
        if self.tainted.load(Ordering::Acquire) { return Err(OperationFsError::UnsafeOrIncomplete); }
        bounded(self.request.bytes(), REQUEST_CAP)?;
        bounded(self.metadata.bytes(), RECORD_CAP)?;
        bounded(self.state.bytes(), RECORD_CAP)?;
        if !self.lock.bytes().is_empty() { return Err(OperationFsError::UnsafeOrIncomplete); }
        for file in self.pending.iter().chain(self.receipt.iter()) { bounded(file.bytes(), RECORD_CAP)?; }
        safe(self.parent.revalidate())?;
        safe(self.directory.revalidate())?;
        if self.directory.path().parent() != Some(self.parent.path()) { return Err(OperationFsError::UnsafeOrIncomplete); }
        closed_inventory(&self.directory)?;
        for file in self.files() { safe(file.persistent_revalidate())?; }
        // An optional pathname appearing since the original read is not adopted.
        for (name, present) in [("current-next.json", self.pending.is_some()), ("receipt.json", self.receipt.is_some())] {
            if safe(self.directory.private_entry(std::ffi::OsStr::new(name)))?.is_some() != present {
                return Err(OperationFsError::UnsafeOrIncomplete);
            }
        }
        Ok(())
    }

    fn finish<T>(&self, result: Result<T>) -> Result<T> {
        if result.is_err() { self.tainted.store(true, Ordering::Release); }
        result
    }

    fn publish_pending_inner(&mut self) -> Result<()> {
        self.validate_inner()?;
        let next = self.pending.as_ref().ok_or(OperationFsError::UnsafeOrIncomplete)?;
        bounded(next.bytes(), RECORD_CAP)?;
        let digest = next.digest().to_owned();
        safe(next.persistent_sync())?;
        // From this call onward a failure can follow a real rename: no rollback claim.
        self.directory.persistent_publish(next, &self.state, "current.json")
            .map_err(|_| OperationFsError::DurabilityUnknown)?;
        self.directory.sync().map_err(|_| OperationFsError::DurabilityUnknown)?;
        let current = required(&self.directory, "current.json", RECORD_CAP)
            .map_err(|_| OperationFsError::DurabilityUnknown)?;
        if current.digest() != digest.as_str() { return Err(OperationFsError::DurabilityUnknown); }
        self.state = current;
        self.pending = None;
        self.validate_inner().map_err(|_| OperationFsError::DurabilityUnknown)
    }
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Checkpoint { ReopenAfterRealParentSync, ReceiptAfterRealDirectorySync }
#[cfg(test)]
thread_local! { static FAIL_WRAPPER_AFTER_REAL_SYNC: std::cell::Cell<Option<Checkpoint>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
fn checkpoint(point: Checkpoint) -> Result<()> {
    if FAIL_WRAPPER_AFTER_REAL_SYNC.with(|fault| {
        if fault.get() == Some(point) { fault.set(None); true } else { false }
    }) { return Err(OperationFsError::DurabilityUnknown); }
    Ok(())
}

#[cfg(test)]
mod tests;
