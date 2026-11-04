use super::*;
use crate::{audit::Store, broker_registry::*};
use sha2::{Digest, Sha256};
use std::{ffi::OsStr, os::unix::fs::MetadataExt, sync::Mutex};

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Anchor {
    version: u32,
    scope: PresenceScope,
    broker_id: String,
    credential_id: String,
    incarnation_id: String,
    authority_id: String,
    directory_device: u64,
    directory_inode: u64,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Pending {
    pub version: u32,
    pub identity: BrokerIdentityViewV1,
    pub prior: BrokerViewV1,
    pub heartbeat: BrokerHeartbeatV1,
    pub input_hash: String,
}
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Acknowledged {
    pub pending: Pending,
    pub result: BrokerMutationResultV1,
}
#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct Head {
    pub count: u32,
    pub bytes: u64,
    pub last: Option<Acknowledged>,
    pub pending: Option<Pending>,
    // Bounded memory only, never authorization or a new on-disk format. The
    // rolling commitment covers every historical byte; native identities catch
    // same-content inode replacement without retaining historical file FDs.
    history_hash: [u8; 32],
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
    p.screen(&value)?;
    let bytes = serde_json::to_vec(&value).map_err(|_| PresenceError::UnsafeState)?;
    if bytes.len() as u64 > RECORD_BYTES {
        return Err(PresenceError::Capacity);
    }
    Ok(bytes)
}
fn read<T: for<'de> Deserialize<'de>>(
    p: &PreparedPresence,
    root: &Directory,
    name: &str,
) -> Result<(T, PrivateFile)> {
    let file =
        fs(root.persistent_read(name, RECORD_BYTES, &[]))?.ok_or(PresenceError::UnsafeState)?;
    let value: Value =
        serde_json::from_slice(file.bytes()).map_err(|_| PresenceError::UnsafeState)?;
    p.screen(&value)?;
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
fn commit_record(head: &mut Head, directory: &Directory, name: &str, bytes: &[u8]) -> Result<()> {
    let facts = fs(fs(directory.try_clone_file())?
        .metadata()
        .map_err(Into::into))?;
    let identity =
        fs(directory.private_entry(OsStr::new(name)))?.ok_or(PresenceError::UnsafeState)?;
    head.history_hash = Sha256::new()
        .chain_update(b"ai-guard:presence-live-history:v1\0")
        .chain_update(head.history_hash)
        .chain_update(facts.dev().to_be_bytes())
        .chain_update(facts.ino().to_be_bytes())
        .chain_update(name.as_bytes())
        .chain_update((bytes.len() as u64).to_be_bytes())
        .chain_update(bytes)
        .finalize()
        .into();
    head.identities.push(identity);
    Ok(())
}
fn reserve_pending(p: &PreparedPresence, head: &Head) -> Result<()> {
    if head.pending.is_some()
        && head
            .bytes
            .checked_add(RECORD_BYTES)
            .is_none_or(|n| n > p.config.max_state_bytes)
    {
        return Err(PresenceError::Capacity);
    }
    Ok(())
}

impl Stream {
    pub(super) fn open(p: &PreparedPresence) -> Result<Self> {
        let root = p.state.clone();
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
        // Reject unknown names BEFORE creating anything or opening regular
        // records. Never adopt a leftover lock-only or partial namespace.
        for entry in &entries {
            if entry.name == ".owner.lock" || entry.name == "anchor.json" {
                continue;
            }
            if !entry.directory
                || !entry
                    .name
                    .strip_prefix("op-")
                    .is_some_and(|n| n.len() == 20 && n.bytes().all(|b| b.is_ascii_digit()))
            {
                return Err(PresenceError::UnsafeState);
            }
        }
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
            scope: p.scope.clone(),
            broker_id: p.config.broker_id.clone(),
            credential_id: p.config.credential_id.clone(),
            incarnation_id: p.config.incarnation_id.clone(),
            authority_id: p.config.authority_id.clone(),
            directory_device: facts.dev(),
            directory_inode: facts.ino(),
        };
        let anchor = if fresh {
            let bytes = encode(p, &expected)?;
            if bytes.len() as u64 > p.config.max_state_bytes {
                return Err(PresenceError::Capacity);
            }
            save(&root, "anchor.json", &bytes)?
        } else {
            let (actual, file): (Anchor, _) = read(p, &root, "anchor.json")?;
            if actual != expected {
                return Err(PresenceError::UnsafeState);
            }
            fs(file.persistent_sync())?;
            file
        };
        let head = Self::scan(p, &root, &anchor)?;
        Ok(Self {
            root,
            lock,
            anchor,
            head: Mutex::new(head),
        })
    }
    fn scan(p: &PreparedPresence, root: &Directory, anchor: &PrivateFile) -> Result<Head> {
        let entries = fs(root.persistent_entries(32770))?;
        for entry in &entries {
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
                    .is_some_and(|n| n.len() == 20 && n.bytes().all(|b| b.is_ascii_digit()))
            {
                return Err(PresenceError::UnsafeState);
            }
        }
        if !entries.iter().any(|e| e.name == ".owner.lock")
            || !entries.iter().any(|e| e.name == "anchor.json")
        {
            return Err(PresenceError::UnsafeState);
        }
        let mut head = Head {
            bytes: anchor.bytes().len() as u64,
            ..Head::default()
        };
        let operation_names: std::collections::BTreeSet<_> = entries
            .iter()
            .filter(|e| e.directory)
            .map(|e| e.name.as_str())
            .collect();
        let operation_count = operation_names.len();
        if operation_count > 32768 {
            return Err(PresenceError::Capacity);
        }
        for index in 1..=operation_count as u32 {
            let name = operation_name(index);
            if !operation_names.contains(name.as_str()) {
                return Err(PresenceError::UnsafeState);
            }
            let directory =
                fs(root.persistent_child(&name, false))?.ok_or(PresenceError::UnsafeState)?;
            let children = fs(directory.persistent_entries(2))?;
            if children
                .iter()
                .any(|e| e.directory || !matches!(e.name.as_str(), "pending.json" | "receipt.json"))
            {
                return Err(PresenceError::UnsafeState);
            }
            let (pending, file): (Pending, _) = read(p, &directory, "pending.json")?;
            validate_pending(p, &pending)?;
            validate_chain(head.last.as_ref(), &pending)?;
            head.bytes = head
                .bytes
                .checked_add(file.bytes().len() as u64)
                .ok_or(PresenceError::Capacity)?;
            fs(file.persistent_sync())?;
            commit_record(&mut head, &directory, "pending.json", file.bytes())?;
            if children.iter().any(|e| e.name == "receipt.json") {
                let (result, receipt): (BrokerMutationResultV1, _) =
                    read(p, &directory, "receipt.json")?;
                validate_result(&pending, &result)?;
                if head
                    .last
                    .as_ref()
                    .is_some_and(|last| result.receipt.audit_seq <= last.result.receipt.audit_seq)
                {
                    return Err(PresenceError::UnsafeState);
                }
                if result.replayed {
                    return Err(PresenceError::UnsafeState);
                }
                head.bytes = head
                    .bytes
                    .checked_add(receipt.bytes().len() as u64)
                    .ok_or(PresenceError::Capacity)?;
                fs(receipt.persistent_sync())?;
                commit_record(&mut head, &directory, "receipt.json", receipt.bytes())?;
                head.last = Some(Acknowledged { pending, result });
            } else {
                if index != operation_count as u32 {
                    return Err(PresenceError::UnsafeState);
                }
                head.pending = Some(pending);
            }
            head.count = index;
            fs(directory.durable_bootstrap())?;
            if head.bytes > p.config.max_state_bytes {
                return Err(PresenceError::Capacity);
            }
        }
        if head.bytes > p.config.max_state_bytes {
            return Err(PresenceError::Capacity);
        }
        reserve_pending(p, &head)?;
        fs(root.durable_bootstrap())?;
        Ok(head)
    }
    fn revalidate(&self) -> Result<()> {
        fs(self.root.revalidate())?;
        fs(self.lock.persistent_revalidate())?;
        fs(self.anchor.persistent_revalidate())?;
        Ok(())
    }
    /// Process-local recovery data, never permission to POST. Mutation paths
    /// separately validate and synchronize the complete retained history.
    pub(super) fn cached_head(&self) -> Result<Head> {
        self.revalidate()?;
        let head = self.head.lock().map_err(|_| PresenceError::UnsafeState)?;
        Ok(head.clone())
    }
    fn check_history(&self, p: &PreparedPresence, head: &Head) -> Result<()> {
        let observed = Self::scan(p, &self.root, &self.anchor)?;
        if observed != *head {
            return Err(PresenceError::UnsafeState);
        }
        Ok(())
    }
    pub(super) fn append(&self, p: &PreparedPresence, pending: Pending) -> Result<()> {
        self.revalidate()?;
        let mut head = self.head.lock().map_err(|_| PresenceError::UnsafeState)?;
        if head.pending.is_some() {
            return Err(PresenceError::UnsafeState);
        }
        self.check_history(p, &head)?;
        if head.count >= p.config.max_operations {
            return Err(PresenceError::Capacity);
        }
        validate_pending(p, &pending)?;
        validate_chain(head.last.as_ref(), &pending)?;
        let bytes = encode(p, &pending)?;
        if head
            .bytes
            .checked_add(bytes.len() as u64)
            .and_then(|n| n.checked_add(RECORD_BYTES))
            .is_none_or(|n| n > p.config.max_state_bytes)
        {
            return Err(PresenceError::Capacity);
        }
        let index = head.count.checked_add(1).ok_or(PresenceError::Capacity)?;
        let directory = fs(self.root.persistent_child(&operation_name(index), true))?
            .ok_or(PresenceError::UnsafeState)?;
        if !fs(directory.persistent_entries(1))?.is_empty() {
            return Err(PresenceError::UnsafeState);
        }
        save(&directory, "pending.json", &bytes)?;
        fs(self.root.durable_bootstrap())?;
        commit_record(&mut head, &directory, "pending.json", &bytes)?;
        head.bytes += bytes.len() as u64;
        head.count = index;
        head.pending = Some(pending);
        Ok(())
    }
    pub(super) fn revalidate_pending(&self, p: &PreparedPresence) -> Result<Pending> {
        self.revalidate()?;
        let head = self.head.lock().map_err(|_| PresenceError::UnsafeState)?;
        self.check_history(p, &head)?;
        reserve_pending(p, &head)?;
        let expected = head.pending.as_ref().ok_or(PresenceError::UnsafeState)?;
        let directory = fs(self
            .root
            .persistent_child(&operation_name(head.count), false))?
        .ok_or(PresenceError::UnsafeState)?;
        let entries = fs(directory.persistent_entries(1))?;
        if entries.len() != 1 || entries[0].name != "pending.json" || entries[0].directory {
            return Err(PresenceError::UnsafeState);
        }
        let (actual, file): (Pending, _) = read(p, &directory, "pending.json")?;
        if encode(p, &actual)? != encode(p, expected)? {
            return Err(PresenceError::UnsafeState);
        }
        fs(file.persistent_sync())?;
        fs(directory.durable_bootstrap())?;
        Ok(actual)
    }
    pub(super) fn acknowledge(
        &self,
        p: &PreparedPresence,
        mut result: BrokerMutationResultV1,
    ) -> Result<()> {
        let pending = self.revalidate_pending(p)?;
        validate_result(&pending, &result)?;
        result.replayed = false;
        let bytes = encode(p, &result)?;
        let mut head = self.head.lock().map_err(|_| PresenceError::UnsafeState)?;
        if head
            .last
            .as_ref()
            .is_some_and(|last| result.receipt.audit_seq <= last.result.receipt.audit_seq)
        {
            return Err(PresenceError::Protocol);
        }
        if head
            .bytes
            .checked_add(bytes.len() as u64)
            .is_none_or(|n| n > p.config.max_state_bytes)
        {
            return Err(PresenceError::Capacity);
        }
        let directory = fs(self
            .root
            .persistent_child(&operation_name(head.count), false))?
        .ok_or(PresenceError::UnsafeState)?;
        save(&directory, "receipt.json", &bytes)?;
        commit_record(&mut head, &directory, "receipt.json", &bytes)?;
        head.bytes += bytes.len() as u64;
        head.last = Some(Acknowledged { pending, result });
        head.pending = None;
        Ok(())
    }
}
fn validate_pending(p: &PreparedPresence, pending: &Pending) -> Result<()> {
    if pending.version != 1 {
        return Err(PresenceError::UnsafeState);
    }
    super::actor::validate_handshake(p, &pending.identity, &pending.prior)?;
    let hb = &pending.heartbeat;
    hb.validate().map_err(|_| PresenceError::UnsafeState)?;
    let previous = pending
        .prior
        .broker
        .last_heartbeat
        .as_ref()
        .filter(|h| h.incarnation_id == hb.incarnation_id)
        .map_or(0, |h| h.sequence);
    if hb.expected_heartbeat_revision != pending.prior.broker.heartbeat_revision
        || hb.incarnation_id != pending.identity.incarnation_id
        || previous.checked_add(1) != Some(hb.sequence)
        || Store::broker_heartbeat_input_hash(&pending.identity, hb)
            .map_err(|_| PresenceError::UnsafeState)?
            != pending.input_hash
    {
        return Err(PresenceError::UnsafeState);
    }
    Ok(())
}
fn validate_chain(previous: Option<&Acknowledged>, next: &Pending) -> Result<()> {
    if let Some(previous) = previous {
        if next.prior.broker != previous.result.broker
            || next.identity.binding_revision < previous.pending.identity.binding_revision
            || next.identity.grant_revision != previous.pending.identity.grant_revision
        {
            return Err(PresenceError::UnsafeState);
        }
    } else if next
        .prior
        .broker
        .last_heartbeat
        .as_ref()
        .is_some_and(|h| h.incarnation_id == next.identity.incarnation_id)
    {
        return Err(PresenceError::UnsafeState);
    }
    Ok(())
}
pub(super) fn validate_result(pending: &Pending, result: &BrokerMutationResultV1) -> Result<()> {
    let receipt = &result.receipt;
    let hb = result
        .broker
        .last_heartbeat
        .as_ref()
        .ok_or(PresenceError::Protocol)?;
    let received = parse_time(&hb.received_at).map_err(|_| PresenceError::Protocol)?;
    let not_after =
        parse_time(&pending.prior.broker.not_after).map_err(|_| PresenceError::Protocol)?;
    let expiry = received
        .checked_add_signed(chrono::Duration::seconds(i64::from(
            pending.prior.broker.lease_seconds,
        )))
        .ok_or(PresenceError::Protocol)?
        .min(not_after);
    let expected_hb = BrokerHeartbeatReceiptV1 {
        incarnation_id: pending.heartbeat.incarnation_id.clone(),
        sequence: pending.heartbeat.sequence,
        received_at: format_time(received),
        lease_expires_at: format_time(expiry),
        heartbeat_provenance: BrokerClaimProvenanceV1::Declared,
        reception_provenance: BrokerReceptionProvenanceV1::Observed,
    };
    let mut expected = pending.prior.broker.clone();
    expected.heartbeat_revision = expected
        .heartbeat_revision
        .checked_add(1)
        .ok_or(PresenceError::Protocol)?;
    expected.last_heartbeat = Some(expected_hb);
    if result.broker != expected
        || received
            < parse_time(&pending.prior.evaluated_at).map_err(|_| PresenceError::Protocol)?
        || received >= not_after
        || receipt.operation_id != pending.heartbeat.operation_id
        || receipt.broker_id != pending.identity.broker_id
        || receipt.grant_revision != pending.identity.grant_revision
        || receipt.heartbeat_revision != expected.heartbeat_revision
        || receipt.input_hash != pending.input_hash
        || receipt.audit_seq <= 0
        || !hash(&receipt.audit_hash)
        || !hash(&receipt.input_hash)
    {
        return Err(PresenceError::Protocol);
    }
    Ok(())
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
