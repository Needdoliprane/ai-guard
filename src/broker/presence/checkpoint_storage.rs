//! Separate immutable stream for schema12 checkpoint operations. V1 heartbeat
//! records and their anchors are never reinterpreted or migrated implicitly.
use super::*;
use crate::{audit::Store, broker_registry::*};
use sha2::{Digest, Sha256};
use std::{ffi::OsStr, os::unix::fs::MetadataExt, sync::Mutex};

const BYTES: u64 = 3 * MAX_BROKER_CHECKPOINT_BYTES as u64;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pending {
    pub version: u32,
    pub identity: BrokerIdentityViewV1,
    pub checkpoint: BrokerCheckpointV1,
    pub input_hash: String,
}
#[derive(PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Anchor {
    version: u32,
    scope: PresenceScope,
    broker_id: String,
    credential_id: String,
    incarnation_id: String,
    authority_id: String,
    device: u64,
    inode: u64,
}
#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct Head {
    pub count: u32,
    pub pending: Option<Pending>,
    pub last: Option<(Pending, BrokerCheckpointResultV1)>,
    bytes: u64,
    hash: [u8; 32],
    identities: Vec<crate::control_fs::PrivateEntryIdentity>,
}
pub(super) struct Stream {
    root: Directory,
    lock: PrivateFile,
    anchor: PrivateFile,
    head: Mutex<Head>,
}
fn fs<T>(value: anyhow::Result<T>) -> Result<T> {
    value.map_err(|_| PresenceError::UnsafeState)
}
fn encode(p: &PreparedPresence, value: &impl Serialize) -> Result<Vec<u8>> {
    let value = serde_json::to_value(value).map_err(|_| PresenceError::UnsafeState)?;
    p.authority.screen(&value)?;
    let bytes = serde_json::to_vec(&value).map_err(|_| PresenceError::UnsafeState)?;
    if bytes.len() as u64 > BYTES {
        return Err(PresenceError::Capacity);
    }
    Ok(bytes)
}
fn read<T: for<'de> Deserialize<'de>>(
    p: &PreparedPresence,
    root: &Directory,
    name: &str,
) -> Result<(T, PrivateFile)> {
    let file = fs(root.persistent_read(name, BYTES, &[]))?.ok_or(PresenceError::UnsafeState)?;
    let value: Value =
        serde_json::from_slice(file.bytes()).map_err(|_| PresenceError::UnsafeState)?;
    p.authority.screen(&value)?;
    Ok((
        serde_json::from_slice(file.bytes()).map_err(|_| PresenceError::UnsafeState)?,
        file,
    ))
}
fn save(root: &Directory, name: &str, bytes: &[u8]) -> Result<PrivateFile> {
    let file = fs(root.persistent_create(name, bytes))?;
    fs(file.persistent_sync())?;
    fs(root.durable_bootstrap())?;
    Ok(file)
}
fn name(index: u32) -> String {
    format!("op-{index:020}")
}
fn fold(head: &mut Head, root: &Directory, name: &str, bytes: &[u8]) -> Result<()> {
    let facts = fs(fs(root.try_clone_file())?.metadata().map_err(Into::into))?;
    let identity = fs(root.private_entry(OsStr::new(name)))?.ok_or(PresenceError::UnsafeState)?;
    head.hash = Sha256::new()
        .chain_update(b"ai-guard:broker-checkpoint-stream:v1\0")
        .chain_update(head.hash)
        .chain_update(facts.dev().to_be_bytes())
        .chain_update(facts.ino().to_be_bytes())
        .chain_update(name.as_bytes())
        .chain_update((bytes.len() as u64).to_be_bytes())
        .chain_update(bytes)
        .finalize()
        .into();
    head.bytes = head
        .bytes
        .checked_add(bytes.len() as u64)
        .ok_or(PresenceError::Capacity)?;
    head.identities.push(identity);
    Ok(())
}
pub(super) fn validate_pending(p: &PreparedPresence, pending: &Pending) -> Result<()> {
    let identity = &pending.identity;
    let snapshot = &pending.checkpoint.snapshot;
    pending
        .checkpoint
        .validate()
        .map_err(|_| PresenceError::UnsafeState)?;
    if pending.version != 1
        || identity.version != 1
        || !matches!(identity.schema_version, 12 | 13)
        || identity.tenant_id != p.authority.scope.expected_tenant_id
        || identity.control_namespace_uuid != p.authority.scope.control_namespace_uuid.to_string()
        || identity.broker_id != p.authority.config.broker_id
        || identity.credential_id != p.authority.config.credential_id
        || identity.incarnation_id != p.authority.config.incarnation_id
        || snapshot.authority.authority_id != p.authority.config.authority_id
        || snapshot.authority.runtime_kind != p.authority.scope.runtime_kind
        || Store::broker_checkpoint_input_hash(identity, &pending.checkpoint)
            .map_err(|_| PresenceError::UnsafeState)?
            != pending.input_hash
    {
        return Err(PresenceError::UnsafeState);
    }
    Ok(())
}
pub(super) fn validate_result(pending: &Pending, result: &BrokerCheckpointResultV1) -> Result<()> {
    let receipt = &result.receipt;
    if result.checkpoint != pending.checkpoint
        || receipt.operation_id != pending.checkpoint.operation_id
        || receipt.broker_id != pending.identity.broker_id
        || receipt.grant_revision != pending.identity.grant_revision
        || receipt.input_hash != pending.input_hash
        || receipt.audit_seq <= 0
        || receipt.audit_hash.len() != 64
        || !receipt
            .audit_hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(PresenceError::Protocol);
    }
    Ok(())
}
impl Stream {
    pub(super) fn open(p: &PreparedPresence) -> Result<Self> {
        let root = p
            .checkpoint_state
            .as_ref()
            .ok_or(PresenceError::InvalidConfig)?
            .clone();
        let entries = fs(root.persistent_entries(32770))?;
        let fresh = entries.is_empty();
        if !fresh
            && (!entries
                .iter()
                .any(|e| e.name == ".owner.lock" && !e.directory && e.bytes == 0)
                || !entries
                    .iter()
                    .any(|e| e.name == "anchor.json" && !e.directory))
        {
            return Err(PresenceError::UnsafeState);
        }
        Self::check_names(&entries)?;
        let lock = if fresh {
            save(&root, ".owner.lock", &[])?
        } else {
            fs(root.persistent_read(".owner.lock", 0, &[]))?.ok_or(PresenceError::UnsafeState)?
        };
        if !fs(lock.persistent_try_lock())? {
            return Err(PresenceError::Conflict);
        }
        fs(lock.persistent_sync())?;
        let facts = fs(fs(root.try_clone_file())?.metadata().map_err(Into::into))?;
        let expected = Anchor {
            version: 1,
            scope: p.authority.scope.clone(),
            broker_id: p.authority.config.broker_id.clone(),
            credential_id: p.authority.config.credential_id.clone(),
            incarnation_id: p.authority.config.incarnation_id.clone(),
            authority_id: p.authority.config.authority_id.clone(),
            device: facts.dev(),
            inode: facts.ino(),
        };
        let anchor = if fresh {
            save(&root, "anchor.json", &encode(p, &expected)?)?
        } else {
            let (actual, file): (Anchor, _) = read(p, &root, "anchor.json")?;
            if actual != expected {
                return Err(PresenceError::UnsafeState);
            }
            file
        };
        fs(anchor.persistent_sync())?;
        let head = Self::scan(p, &root, &anchor)?;
        Ok(Self {
            root,
            lock,
            anchor,
            head: Mutex::new(head),
        })
    }
    fn check_names(entries: &[crate::control_fs::persistent::Entry]) -> Result<()> {
        for entry in entries {
            if entry.name == ".owner.lock" {
                if entry.directory || entry.bytes != 0 {
                    return Err(PresenceError::UnsafeState);
                }
            } else if entry.name == "anchor.json" {
                if entry.directory {
                    return Err(PresenceError::UnsafeState);
                }
            } else if !entry.directory
                || !entry
                    .name
                    .strip_prefix("op-")
                    .is_some_and(|s| s.len() == 20 && s.bytes().all(|b| b.is_ascii_digit()))
            {
                return Err(PresenceError::UnsafeState);
            }
        }
        Ok(())
    }
    fn scan(p: &PreparedPresence, root: &Directory, anchor: &PrivateFile) -> Result<Head> {
        let entries = fs(root.persistent_entries(32770))?;
        Self::check_names(&entries)?;
        let count = entries.iter().filter(|e| e.directory).count();
        if count
            > p.authority
                .config
                .checkpoint_max_operations
                .ok_or(PresenceError::InvalidConfig)? as usize
        {
            return Err(PresenceError::Capacity);
        }
        let mut head = Head {
            bytes: anchor.bytes().len() as u64,
            ..Head::default()
        };
        for index in 1..=count as u32 {
            let directory = fs(root.persistent_child(&name(index), false))?
                .ok_or(PresenceError::UnsafeState)?;
            let children = fs(directory.persistent_entries(2))?;
            if children
                .iter()
                .any(|e| e.directory || !matches!(e.name.as_str(), "pending.json" | "receipt.json"))
            {
                return Err(PresenceError::UnsafeState);
            }
            let (pending, file): (Pending, _) = read(p, &directory, "pending.json")?;
            validate_pending(p, &pending)?;
            let previous_sequence = head
                .last
                .as_ref()
                .map_or(0, |(previous, _)| previous.checkpoint.sequence);
            if pending.checkpoint.previous_sequence != previous_sequence {
                return Err(PresenceError::UnsafeState);
            }
            if let Some((previous, _)) = &head.last
                && (pending.identity.grant_revision != previous.identity.grant_revision
                    || pending.identity.binding_revision < previous.identity.binding_revision)
            {
                return Err(PresenceError::UnsafeState);
            }
            fs(file.persistent_sync())?;
            fold(&mut head, &directory, "pending.json", file.bytes())?;
            if children.iter().any(|e| e.name == "receipt.json") {
                let (result, file): (BrokerCheckpointResultV1, _) =
                    read(p, &directory, "receipt.json")?;
                validate_result(&pending, &result)?;
                if result.replayed
                    || head
                        .last
                        .as_ref()
                        .is_some_and(|(_, last)| result.receipt.audit_seq <= last.receipt.audit_seq)
                {
                    return Err(PresenceError::UnsafeState);
                }
                fs(file.persistent_sync())?;
                fold(&mut head, &directory, "receipt.json", file.bytes())?;
                head.last = Some((pending, result));
            } else {
                if index != count as u32 {
                    return Err(PresenceError::UnsafeState);
                }
                head.pending = Some(pending);
            }
            head.count = index;
            fs(directory.durable_bootstrap())?;
        }
        let max = p
            .config
            .checkpoint_max_state_bytes
            .ok_or(PresenceError::InvalidConfig)?;
        if head.bytes > max
            || head.pending.is_some() && head.bytes.checked_add(BYTES).is_none_or(|n| n > max)
        {
            return Err(PresenceError::Capacity);
        }
        fs(root.durable_bootstrap())?;
        Ok(head)
    }
    pub(super) fn checked_head(&self, p: &PreparedPresence) -> Result<Head> {
        fs(self.root.revalidate())?;
        fs(self.lock.persistent_revalidate())?;
        fs(self.anchor.persistent_revalidate())?;
        let expected = self.head.lock().map_err(|_| PresenceError::UnsafeState)?;
        let actual = Self::scan(p, &self.root, &self.anchor)?;
        if actual != *expected {
            return Err(PresenceError::UnsafeState);
        }
        Ok(actual)
    }
    pub(super) fn pending(&self, p: &PreparedPresence) -> Result<Option<Pending>> {
        Ok(self.checked_head(p)?.pending)
    }
    pub(super) fn append(&self, p: &PreparedPresence, pending: Pending) -> Result<()> {
        let mut head = self.checked_head(p)?;
        validate_pending(p, &pending)?;
        if head.pending.is_some()
            || pending.checkpoint.previous_sequence
                != head.last.as_ref().map_or(0, |(v, _)| v.checkpoint.sequence)
        {
            return Err(PresenceError::Conflict);
        }
        let bytes = encode(p, &pending)?;
        let max = p
            .config
            .checkpoint_max_state_bytes
            .ok_or(PresenceError::InvalidConfig)?;
        if head.count
            >= p.authority
                .config
                .checkpoint_max_operations
                .ok_or(PresenceError::InvalidConfig)?
            || head
                .bytes
                .checked_add(bytes.len() as u64)
                .and_then(|n| n.checked_add(BYTES))
                .is_none_or(|n| n > max)
        {
            return Err(PresenceError::Capacity);
        }
        let index = head.count.checked_add(1).ok_or(PresenceError::Capacity)?;
        let directory = fs(self.root.persistent_child(&name(index), true))?
            .ok_or(PresenceError::UnsafeState)?;
        if !fs(directory.persistent_entries(1))?.is_empty() {
            return Err(PresenceError::UnsafeState);
        }
        let _file = save(&directory, "pending.json", &bytes)?;
        fs(self.root.durable_bootstrap())?;
        fold(&mut head, &directory, "pending.json", &bytes)?;
        head.count = index;
        head.pending = Some(pending);
        *self.head.lock().map_err(|_| PresenceError::UnsafeState)? = head;
        Ok(())
    }
    pub(super) fn acknowledge(
        &self,
        p: &PreparedPresence,
        mut result: BrokerCheckpointResultV1,
    ) -> Result<()> {
        let mut head = self.checked_head(p)?;
        let pending = head.pending.clone().ok_or(PresenceError::UnsafeState)?;
        validate_result(&pending, &result)?;
        if head
            .last
            .as_ref()
            .is_some_and(|(_, last)| result.receipt.audit_seq <= last.receipt.audit_seq)
        {
            return Err(PresenceError::Protocol);
        }
        result.replayed = false;
        let bytes = encode(p, &result)?;
        let directory = fs(self.root.persistent_child(&name(head.count), false))?
            .ok_or(PresenceError::UnsafeState)?;
        let _file = save(&directory, "receipt.json", &bytes)?;
        fold(&mut head, &directory, "receipt.json", &bytes)?;
        head.last = Some((pending, result));
        head.pending = None;
        *self.head.lock().map_err(|_| PresenceError::UnsafeState)? = head;
        Ok(())
    }
}
