//! Private immutable journals of the owned profile. An anchor binds a journal
//! to the broker scope, the enrollment pins, its kind and the physical
//! directory identity. A journal never adopts a legacy, foreign or partial
//! namespace and is never repaired, compacted or reset: corruption or
//! saturation stops its publisher explicitly and leaves every byte in place.
//!
//! Layout: `.owned-owner.lock`, `owned-anchor.json`, then `op-%020d/` holding
//! `pending.json` (written and synchronized before any POST) and, once
//! validated, `ack.json`. Only the last operation may lack its `ack.json`.
//!
//! Two interrupted writes leave a state that provably preceded any POST and
//! are completed without rewriting or removing a byte: a directory holding
//! only the empty lock (initialization stopped before its anchor existed) is
//! initialized, and an empty last `op-*` directory (an append stopped before
//! its Pending existed) is reused by the next operation. Every other partial
//! state, including a torn record, is refused and kept.
//!
//! Opening synchronizes every retained record and directory once, because a
//! previous process may have stopped between a write and its
//! synchronization. Later scans verify the same complete history without
//! synchronizing it again: each mutation synchronizes the record it writes,
//! its directory and their ancestors before any POST or new head.
use super::*;
use crate::{
    broker_registry::owned_supervision::OwnedAcknowledgementSourceV1,
    control_fs::{PrivateEntryIdentity, persistent::Entry},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, ffi::OsStr, marker::PhantomData, sync::Mutex};

pub(super) const MAX_JOURNAL_OPERATIONS: u32 = 32_768;
pub(super) const MAX_JOURNAL_BYTES: u64 = 128 * 1024 * 1024;
const ENTRY_LIMIT: u32 = MAX_JOURNAL_OPERATIONS + 2;
const ANCHOR_BYTES: u64 = 16 * 1024;
const ENVELOPE_BYTES: u64 = 1024;
const ANCHOR_VERSION: u32 = 1;
const ACK_RECORD_VERSION: u32 = 1;
const LOCK: &str = ".owned-owner.lock";
const ANCHOR: &str = "owned-anchor.json";
const PENDING: &str = "pending.json";
const ACK: &str = "ack.json";
const HISTORY_DOMAIN: &[u8] = b"ai-guard:presence-owned-history:v1\0";

/// Upper bound of one record: the larger shared-contract bound plus envelope.
pub(super) const fn record_bytes(pending: usize, outcome: usize) -> u64 {
    let larger = if pending > outcome { pending } else { outcome };
    larger as u64 + ENVELOPE_BYTES
}
/// Smallest admitted journal: its anchor, one Pending and its acknowledgement.
pub(super) fn minimum_bytes<K: Kind>() -> u64 {
    ANCHOR_BYTES + 2 * K::RECORD_BYTES
}

/// One owned journal kind over the shared contract: its record bounds and the
/// chain rules re-checked on every scan, append and acknowledgement.
pub(super) trait Kind: Clone + PartialEq + Send + Sync + 'static {
    const NAME: &'static str;
    const RECORD_BYTES: u64;
    type Pending: Clone + PartialEq + Serialize + DeserializeOwned + Send + 'static;
    type Outcome: Clone + PartialEq + Serialize + DeserializeOwned + Send + 'static;
    /// Contract validation, pins and continuity with the previous ACK.
    fn check_pending(
        pins: &OwnedSupervisionPinsV1,
        previous: Option<&AckRecordV1<Self::Outcome>>,
        pending: &Self::Pending,
    ) -> Result<()>;
    /// The outcome must acknowledge exactly this Pending.
    fn check_outcome(
        pins: &OwnedSupervisionPinsV1,
        pending: &Self::Pending,
        source: OwnedAcknowledgementSourceV1,
        outcome: &Self::Outcome,
    ) -> Result<()>;
    fn audit_seq(outcome: &Self::Outcome) -> i64;
}

/// Durable acknowledgement as received or self-read, never rewritten.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AckRecordV1<R> {
    pub version: u32,
    pub source: OwnedAcknowledgementSourceV1,
    pub result: R,
}

#[derive(Clone, Copy)]
pub(super) struct JournalLimits {
    pub operations: u32,
    pub bytes: u64,
}

/// Whether a complete scan also synchronizes what it verifies.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Durability {
    /// At open only: every record, operation directory and ancestor.
    Synchronize,
    /// Every later scan: the same verification, no synchronization.
    Verify,
}

/// Retained head. The rolling commitment covers every historical byte and the
/// native identities catch a same-content inode replacement; both stay in
/// bounded memory only and are never an authorization or an on-disk format.
#[derive(Clone, PartialEq)]
pub(super) struct Head<K: Kind> {
    pub last: Option<AckRecordV1<K::Outcome>>,
    pub pending: Option<K::Pending>,
    count: u32,
    bytes: u64,
    history: [u8; 32],
    identities: Vec<PrivateEntryIdentity>,
    /// Physical identity of an empty last operation directory, reused by the
    /// next append; it holds no record and is not an operation.
    vacant: Option<(u64, u64)>,
    kind: PhantomData<K>,
}
impl<K: Kind> Head<K> {
    fn empty(bytes: u64) -> Self {
        Self {
            last: None,
            pending: None,
            count: 0,
            bytes,
            history: [0; 32],
            identities: Vec::new(),
            vacant: None,
            kind: PhantomData,
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct AnchorV1 {
    version: u32,
    profile: String,
    profile_version: u32,
    journal: String,
    scope: PresenceScope,
    pins: OwnedSupervisionPinsV1,
    directory_device: u64,
    directory_inode: u64,
}

pub(super) struct Journal<K: Kind> {
    root: Directory,
    lock: PrivateFile,
    anchor: PrivateFile,
    limits: JournalLimits,
    head: Mutex<Head<K>>,
}

fn fs<T>(value: anyhow::Result<T>) -> Result<T> {
    value.map_err(|_| PresenceError::UnsafeState)
}
fn foreign() -> PresenceError {
    block(OwnedPresenceBlock::ForeignJournal)
}
fn encode(p: &PreparedOwnedPresence, value: &impl Serialize, limit: u64) -> Result<Vec<u8>> {
    let screened = serde_json::to_value(value).map_err(|_| PresenceError::UnsafeState)?;
    p.screen(&screened)?;
    let bytes = serde_json::to_vec(value).map_err(|_| PresenceError::UnsafeState)?;
    if bytes.len() as u64 > limit {
        return Err(PresenceError::Capacity);
    }
    Ok(bytes)
}
fn read<T: DeserializeOwned>(
    p: &PreparedOwnedPresence,
    root: &Directory,
    name: &str,
    limit: u64,
) -> Result<(T, PrivateFile)> {
    let file = fs(root.persistent_read(name, limit, &[]))?.ok_or(PresenceError::UnsafeState)?;
    let value: Value =
        serde_json::from_slice(file.bytes()).map_err(|_| PresenceError::UnsafeState)?;
    p.screen(&value)
        .map_err(|_| PresenceError::UnsafeState)?;
    // Typed decoding from the original bytes also refuses duplicate fields.
    let value = serde_json::from_slice(file.bytes()).map_err(|_| PresenceError::UnsafeState)?;
    Ok((value, file))
}
fn save(root: &Directory, name: &str, bytes: &[u8]) -> Result<PrivateFile> {
    let file = fs(root.persistent_create(name, bytes))?;
    fs(file.persistent_sync())?;
    fs(root.durable_bootstrap())?;
    Ok(file)
}
fn operation_name(index: u32) -> String {
    format!("op-{index:020}")
}
fn is_operation(name: &str) -> bool {
    name.strip_prefix("op-")
        .is_some_and(|digits| digits.len() == 20 && digits.bytes().all(|b| b.is_ascii_digit()))
}
/// Only the owned lock, the owned anchor and operation directories exist.
fn check_names(entries: &[Entry]) -> Result<()> {
    for entry in entries {
        let known = match entry.name.as_str() {
            LOCK => !entry.directory && entry.bytes == 0,
            ANCHOR => !entry.directory,
            name => entry.directory && is_operation(name),
        };
        if !known {
            return Err(PresenceError::UnsafeState);
        }
    }
    Ok(())
}
/// Physical identity (device, inode) of a retained directory.
fn directory_identity(directory: &Directory) -> Result<(u64, u64)> {
    let facts = fs(fs(directory.try_clone_file())?
        .metadata()
        .map_err(Into::into))?;
    Ok((facts.dev(), facts.ino()))
}
fn fold<K: Kind>(
    head: &mut Head<K>,
    directory: &Directory,
    name: &str,
    bytes: &[u8],
) -> Result<()> {
    let (device, inode) = directory_identity(directory)?;
    let entry = fs(directory.private_entry(OsStr::new(name)))?.ok_or(PresenceError::UnsafeState)?;
    head.history = Sha256::new()
        .chain_update(HISTORY_DOMAIN)
        .chain_update(K::NAME.as_bytes())
        .chain_update([0u8])
        .chain_update(head.history)
        .chain_update(device.to_be_bytes())
        .chain_update(inode.to_be_bytes())
        .chain_update(name.as_bytes())
        .chain_update((bytes.len() as u64).to_be_bytes())
        .chain_update(bytes)
        .finalize()
        .into();
    head.bytes = head
        .bytes
        .checked_add(bytes.len() as u64)
        .ok_or(PresenceError::Capacity)?;
    head.identities.push(entry);
    Ok(())
}
fn check_record<K: Kind>(
    pins: &OwnedSupervisionPinsV1,
    pending: &K::Pending,
    record: &AckRecordV1<K::Outcome>,
    previous: Option<&AckRecordV1<K::Outcome>>,
) -> Result<()> {
    if record.version != ACK_RECORD_VERSION {
        return Err(PresenceError::UnsafeState);
    }
    K::check_outcome(pins, pending, record.source, &record.result)?;
    if previous.is_some_and(|last| K::audit_seq(&record.result) <= K::audit_seq(&last.result)) {
        return Err(acknowledgement(OwnedAckRejectionV1::Receipt));
    }
    Ok(())
}

impl<K: Kind> Journal<K> {
    /// Open or initialize one owned journal. A non-empty directory without the
    /// owned lock and anchor (for example a legacy presence journal) or an
    /// anchor of another scope, enrollment or kind is refused, never adopted.
    /// A directory holding only the empty owned lock is an initialization
    /// stopped before its anchor existed: no operation can precede the
    /// synchronized anchor, so the retained lock is reused and the anchor is
    /// created, exactly as for an empty directory.
    pub(super) fn open(
        p: &PreparedOwnedPresence,
        root: &Directory,
        limits: JournalLimits,
    ) -> Result<Self> {
        let root = root.clone();
        let entries = fs(root.persistent_entries(ENTRY_LIMIT))?;
        let has_lock = entries
            .iter()
            .any(|e| e.name == LOCK && !e.directory && e.bytes == 0);
        let has_anchor = entries.iter().any(|e| e.name == ANCHOR && !e.directory);
        let fresh = entries.is_empty() || (entries.len() == 1 && has_lock);
        if !fresh && !(has_lock && has_anchor) {
            return Err(foreign());
        }
        // Refuse unknown names BEFORE creating anything or reading records.
        check_names(&entries)?;
        let lock = if entries.is_empty() {
            save(&root, LOCK, &[])?
        } else {
            fs(root.persistent_read(LOCK, 0, &[]))?.ok_or(PresenceError::UnsafeState)?
        };
        if !fs(lock.persistent_try_lock())? {
            return Err(PresenceError::Conflict);
        }
        fs(lock.persistent_sync())?;
        let (directory_device, directory_inode) = directory_identity(&root)?;
        let expected = AnchorV1 {
            version: ANCHOR_VERSION,
            profile: OWNED_SUPERVISION_PROFILE_V1.into(),
            profile_version: OWNED_SUPERVISION_VERSION_V1,
            journal: K::NAME.into(),
            scope: p.scope.clone(),
            pins: p.pins.clone(),
            directory_device,
            directory_inode,
        };
        let anchor = if fresh {
            let bytes = encode(p, &expected, ANCHOR_BYTES)?;
            if bytes.len() as u64 > limits.bytes {
                return Err(PresenceError::Capacity);
            }
            save(&root, ANCHOR, &bytes)?
        } else {
            let (actual, file): (AnchorV1, _) = read(p, &root, ANCHOR, ANCHOR_BYTES)?;
            if actual != expected {
                return Err(foreign());
            }
            fs(file.persistent_sync())?;
            file
        };
        let head = Self::scan(p, &root, &anchor, limits, Durability::Synchronize)?;
        Ok(Self {
            root,
            lock,
            anchor,
            limits,
            head: Mutex::new(head),
        })
    }

    /// Validate the complete retained history; at open, also synchronize it.
    fn scan(
        p: &PreparedOwnedPresence,
        root: &Directory,
        anchor: &PrivateFile,
        limits: JournalLimits,
        durability: Durability,
    ) -> Result<Head<K>> {
        let synchronize = durability == Durability::Synchronize;
        let entries = fs(root.persistent_entries(ENTRY_LIMIT))?;
        check_names(&entries)?;
        if !entries.iter().any(|e| e.name == LOCK) || !entries.iter().any(|e| e.name == ANCHOR) {
            return Err(PresenceError::UnsafeState);
        }
        let operations: BTreeSet<&str> = entries
            .iter()
            .filter(|e| e.directory)
            .map(|e| e.name.as_str())
            .collect();
        let count = u32::try_from(operations.len()).map_err(|_| PresenceError::Capacity)?;
        if count > limits.operations {
            return Err(PresenceError::Capacity);
        }
        let mut head = Head::<K>::empty(anchor.bytes().len() as u64);
        for index in 1..=count {
            let name = operation_name(index);
            if !operations.contains(name.as_str()) {
                return Err(PresenceError::UnsafeState);
            }
            let directory =
                fs(root.persistent_child(&name, false))?.ok_or(PresenceError::UnsafeState)?;
            let children = fs(directory.persistent_entries(2))?;
            if children
                .iter()
                .any(|e| e.directory || !matches!(e.name.as_str(), PENDING | ACK))
            {
                return Err(PresenceError::UnsafeState);
            }
            if children.is_empty() {
                // An append stopped after creating this directory and before
                // its Pending existed, so no POST used it. Only the last
                // directory may be in that state; it is kept for reuse.
                if index != count {
                    return Err(PresenceError::UnsafeState);
                }
                head.vacant = Some(directory_identity(&directory)?);
                break;
            }
            let (pending, file): (K::Pending, _) =
                read(p, &directory, PENDING, K::RECORD_BYTES)?;
            K::check_pending(&p.pins, head.last.as_ref(), &pending)
                .map_err(|_| PresenceError::UnsafeState)?;
            if synchronize {
                fs(file.persistent_sync())?;
            }
            fold(&mut head, &directory, PENDING, file.bytes())?;
            if children.iter().any(|e| e.name == ACK) {
                let (record, file): (AckRecordV1<K::Outcome>, _) =
                    read(p, &directory, ACK, K::RECORD_BYTES)?;
                check_record::<K>(&p.pins, &pending, &record, head.last.as_ref())
                    .map_err(|_| PresenceError::UnsafeState)?;
                if synchronize {
                    fs(file.persistent_sync())?;
                }
                fold(&mut head, &directory, ACK, file.bytes())?;
                head.last = Some(record);
            } else {
                if index != count {
                    return Err(PresenceError::UnsafeState);
                }
                head.pending = Some(pending);
            }
            head.count = index;
            if synchronize {
                // This directory's entries; the shared ancestors once below.
                fs(directory.sync())?;
            }
            if head.bytes > limits.bytes {
                return Err(PresenceError::Capacity);
            }
        }
        // A retained Pending always keeps room for its acknowledgement.
        if head.pending.is_some()
            && head
                .bytes
                .checked_add(K::RECORD_BYTES)
                .is_none_or(|n| n > limits.bytes)
        {
            return Err(PresenceError::Capacity);
        }
        if synchronize {
            fs(root.durable_bootstrap())?;
        }
        Ok(head)
    }

    fn revalidate(&self) -> Result<()> {
        fs(self.root.revalidate())?;
        fs(self.lock.persistent_revalidate())?;
        fs(self.anchor.persistent_revalidate())?;
        Ok(())
    }

    /// Process-local recovery data for the next decision, never permission to
    /// POST: every mutation and every send follows a complete verification of
    /// the history in the same blocking action.
    pub(super) fn cached_head(&self) -> Result<Head<K>> {
        self.revalidate()?;
        let head = self.head.lock().map_err(|_| PresenceError::UnsafeState)?;
        Ok(head.clone())
    }

    /// Full verification compared with the retained head: a changed
    /// historical byte, inode or name since the last mutation refuses the
    /// operation. It re-reads every record without synchronizing it again.
    fn checked_head(&self, p: &PreparedOwnedPresence) -> Result<Head<K>> {
        self.revalidate()?;
        let expected = self.head.lock().map_err(|_| PresenceError::UnsafeState)?;
        let actual = Self::scan(p, &self.root, &self.anchor, self.limits, Durability::Verify)?;
        if actual != *expected {
            return Err(PresenceError::UnsafeState);
        }
        Ok(actual)
    }

    /// Write and synchronize a new Pending before any POST, then return it as
    /// decoded from its durable bytes: the send that follows uses exactly
    /// them, right after this complete verification. Exactly one Pending
    /// exists at a time and room for its acknowledgement is reserved first.
    pub(super) fn append(
        &self,
        p: &PreparedOwnedPresence,
        pending: K::Pending,
    ) -> Result<K::Pending> {
        let mut head = self.checked_head(p)?;
        if head.pending.is_some() {
            return Err(PresenceError::UnsafeState);
        }
        if head.count >= self.limits.operations {
            return Err(PresenceError::Capacity);
        }
        K::check_pending(&p.pins, head.last.as_ref(), &pending)?;
        let bytes = encode(p, &pending, K::RECORD_BYTES)?;
        // A later scan decodes these bytes and compares them with the head:
        // refuse before writing anything if they do not decode identically.
        let durable: K::Pending =
            serde_json::from_slice(&bytes).map_err(|_| PresenceError::UnsafeState)?;
        if durable != pending {
            return Err(PresenceError::UnsafeState);
        }
        if head
            .bytes
            .checked_add(bytes.len() as u64)
            .and_then(|n| n.checked_add(K::RECORD_BYTES))
            .is_none_or(|n| n > self.limits.bytes)
        {
            return Err(PresenceError::Capacity);
        }
        let index = head.count.checked_add(1).ok_or(PresenceError::Capacity)?;
        let directory = fs(self.root.persistent_child(&operation_name(index), true))?
            .ok_or(PresenceError::UnsafeState)?;
        // A vacant directory is reused only if it is still the one verified.
        let identity = directory_identity(&directory)?;
        if head.vacant.is_some_and(|vacant| vacant != identity) {
            return Err(PresenceError::UnsafeState);
        }
        if !fs(directory.persistent_entries(1))?.is_empty() {
            return Err(PresenceError::UnsafeState);
        }
        // `save` synchronizes the record, then this directory and every
        // ancestor, including the journal root holding its entry.
        save(&directory, PENDING, &bytes)?;
        fold(&mut head, &directory, PENDING, &bytes)?;
        head.count = index;
        head.pending = Some(pending);
        head.vacant = None;
        *self.head.lock().map_err(|_| PresenceError::UnsafeState)? = head;
        Ok(durable)
    }

    /// The retained Pending as re-read from disk by a complete verification,
    /// so a retry or restart sends exactly its durable bytes.
    pub(super) fn durable_pending(&self, p: &PreparedOwnedPresence) -> Result<K::Pending> {
        self.checked_head(p)?
            .pending
            .ok_or(PresenceError::UnsafeState)
    }

    /// Persist a validated acknowledgement before any new operation or head.
    pub(super) fn acknowledge(
        &self,
        p: &PreparedOwnedPresence,
        source: OwnedAcknowledgementSourceV1,
        result: K::Outcome,
    ) -> Result<()> {
        let mut head = self.checked_head(p)?;
        let pending = head.pending.clone().ok_or(PresenceError::UnsafeState)?;
        let record = AckRecordV1 {
            version: ACK_RECORD_VERSION,
            source,
            result,
        };
        check_record::<K>(&p.pins, &pending, &record, head.last.as_ref())?;
        let bytes = encode(p, &record, K::RECORD_BYTES)?;
        if head
            .bytes
            .checked_add(bytes.len() as u64)
            .is_none_or(|n| n > self.limits.bytes)
        {
            return Err(PresenceError::Capacity);
        }
        let directory = fs(self
            .root
            .persistent_child(&operation_name(head.count), false))?
        .ok_or(PresenceError::UnsafeState)?;
        save(&directory, ACK, &bytes)?;
        fold(&mut head, &directory, ACK, &bytes)?;
        head.last = Some(record);
        head.pending = None;
        *self.head.lock().map_err(|_| PresenceError::UnsafeState)? = head;
        Ok(())
    }
}
