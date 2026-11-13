//! Sealed retirement authority. This mutex covers filesystem publication only;
//! Prepare/STOP owners never acquire it while reserving an effect.
use super::{EnforcerError, EnforcerResult};
use crate::{
    broker::{discovery::EnforcerRoleLeaseV2, retirement::*},
    control_fs::{Directory, PrivateFile, persistent::ExclusiveEnforcerRoot},
};
use serde::Serialize;
use std::{collections::BTreeMap, sync::Mutex};
use uuid::Uuid;

const MAX_ARMS: usize = 1024;
const MAX_LOGICAL_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_REVISIONS: u64 = 4096;
const MAX_RECORD_BYTES: u64 = 8 * 1024 * 1024;
const MAX_FLOORS: usize = 4096;
const MAX_LOCK_BYTES: usize = 8 * 1024 * 1024;
const MAX_FRAME_BYTES: usize = 2 * 1024;

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// Each name identifies the boundary after its corresponding durable sync.
// Preserve these explicit fault-point names across replay regressions.
#[allow(clippy::enum_variant_names)]
enum FaultStage {
    DeltaSynced,
    FloorSynced,
    PointerSynced,
    TombstoneSynced,
}
macro_rules! fault {
    ($stage:ident) => {{
        #[cfg(test)]
        tests::checkpoint(FaultStage::$stage)?;
    }};
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LedgerScanPhaseV1 {
    Seal,
    Genesis,
    Floors,
    Deltas,
    Tombstones,
    Pointer,
    Budget,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LedgerFailureV1 {
    InvalidOrMissing,
    Tainted,
    BoundExceeded,
}
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "coverage", rename_all = "snake_case")]
pub(crate) enum LedgerCoverageV1 {
    Complete {
        head: LedgerHeadV1,
        arms: u32,
    },
    Partial {
        ledger: Option<LedgerIdentityV1>,
        phase: LedgerScanPhaseV1,
        reason: LedgerFailureV1,
    },
    Unverified,
}

pub(crate) struct RetirementLedger {
    lease: EnforcerRoleLeaseV2,
    state: Mutex<State>,
    _owner: crate::control_fs::persistent::EnforcerLedgerOwner,
}
struct State {
    coverage: LedgerCoverageV1,
    identity: Option<LedgerIdentityV1>,
    head: Option<LedgerHeadV1>,
    current: Option<PrivateFile>,
    records: Vec<PrivateFile>,
    floors: Vec<LedgerFloorV1>,
    lock_bytes: Vec<u8>,
    entries: BTreeMap<Uuid, RetirementLedgerEntryV1>,
    logical_bytes: u64,
    reserved_bytes: u64,
    reserved_lock: usize,
    reserved_revisions: u64,
}

/// Internal publication handles are not quiescence proofs or serialized rights.
pub(crate) struct ClosingCommitted {
    ledger: LedgerIdentityV1,
    arm_id: Uuid,
    entry_hash: RetirementDigestV1,
    accepted: ClosingAcceptanceV1,
}
pub(crate) struct TombstoneCommitted {
    ledger: LedgerIdentityV1,
    arm_id: Uuid,
    entry_hash: RetirementDigestV1,
    tombstone: RetiredTombstoneV1,
}

impl ClosingCommitted {
    pub(crate) fn accepted(&self) -> &ClosingAcceptanceV1 {
        &self.accepted
    }
}
impl TombstoneCommitted {
    pub(crate) fn tombstone(&self) -> &RetiredTombstoneV1 {
        &self.tombstone
    }
}

fn disk<T>(result: anyhow::Result<T>) -> EnforcerResult<T> {
    result.map_err(|_| EnforcerError::UnsafeFilesystem)
}
fn record<T>(result: crate::broker::journal::JournalResult<T>) -> EnforcerResult<T> {
    result.map_err(|_| EnforcerError::InvalidInput)
}
fn require(ok: bool) -> EnforcerResult<()> {
    if ok {
        Ok(())
    } else {
        Err(EnforcerError::Conflict)
    }
}
fn frame(bytes: &[u8]) -> EnforcerResult<Vec<u8>> {
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(EnforcerError::Capacity);
    }
    let mut framed = Vec::with_capacity(bytes.len() + 4);
    framed.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    framed.extend_from_slice(bytes);
    Ok(framed)
}
fn frames(bytes: &[u8]) -> EnforcerResult<(Vec<&[u8]>, bool)> {
    if bytes.len() > MAX_LOCK_BYTES {
        return Err(EnforcerError::Capacity);
    }
    let mut result = Vec::new();
    let mut tail = bytes;
    while !tail.is_empty() {
        if tail.len() < 4 || result.len() > MAX_FLOORS {
            return Ok((result, false));
        }
        let size = u32::from_be_bytes([tail[0], tail[1], tail[2], tail[3]]) as usize;
        if size == 0 || size > MAX_FRAME_BYTES || tail.len() - 4 < size {
            return Ok((result, false));
        }
        result.push(&tail[4..4 + size]);
        tail = &tail[4 + size..];
    }
    Ok((result, true))
}
fn delta_name(revision: u64) -> String {
    format!("d-{revision:04}.json")
}
fn tombstone_name(arm: Uuid) -> String {
    format!("t-{arm}.json")
}
fn child(root: &Directory, name: &str) -> EnforcerResult<Directory> {
    disk(root.persistent_child(name, false))?.ok_or(EnforcerError::UnsafeFilesystem)
}
fn read(root: &Directory, name: &str) -> EnforcerResult<PrivateFile> {
    disk(root.persistent_read(name, MAX_RECORD_BYTES, &[]))?.ok_or(EnforcerError::UnsafeFilesystem)
}
fn create(root: &Directory, name: &str, bytes: &[u8]) -> EnforcerResult<PrivateFile> {
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(EnforcerError::Capacity);
    }
    let file = disk(root.persistent_create(name, bytes))?;
    disk(file.persistent_sync())?;
    disk(root.sync())?;
    Ok(file)
}

impl State {
    fn empty() -> Self {
        Self {
            coverage: LedgerCoverageV1::Unverified,
            identity: None,
            head: None,
            current: None,
            records: Vec::new(),
            floors: Vec::new(),
            lock_bytes: Vec::new(),
            entries: BTreeMap::new(),
            logical_bytes: 0,
            reserved_bytes: 0,
            reserved_lock: 0,
            reserved_revisions: 0,
        }
    }
    fn complete(&self) -> EnforcerResult<()> {
        if matches!(self.coverage, LedgerCoverageV1::Complete { .. }) {
            Ok(())
        } else {
            Err(EnforcerError::Unhealthy)
        }
    }
    fn fail(&mut self, phase: LedgerScanPhaseV1, reason: LedgerFailureV1) {
        self.coverage = LedgerCoverageV1::Partial {
            ledger: self.identity.clone(),
            phase,
            reason,
        };
    }
    fn refresh(&mut self) -> EnforcerResult<()> {
        let head = self.head.clone().ok_or(EnforcerError::Unhealthy)?;
        self.coverage = LedgerCoverageV1::Complete {
            head,
            arms: self.entries.len() as u32,
        };
        Ok(())
    }
}

impl RetirementLedger {
    /// The proof is consumed before any initialization. On failure all bytes
    /// remain; an existing path can never obtain another factory capability.
    pub(crate) fn initialize_new(
        fresh: ExclusiveEnforcerRoot,
        installation_id: Uuid,
    ) -> EnforcerResult<Self> {
        if installation_id.is_nil() {
            return Err(EnforcerError::InvalidInput);
        }
        let lease = EnforcerRoleLeaseV2::create_new(fresh)?;
        let root = lease.ledger_root();
        let ledger = disk(root.persistent_child("retirement", true))?
            .ok_or(EnforcerError::UnsafeFilesystem)?;
        disk(ledger.persistent_child("deltas", true))?;
        disk(ledger.persistent_child("tombstones", true))?;
        let genesis = record(GenesisCoreV1::new(
            installation_id,
            Uuid::new_v4(),
            Uuid::new_v4(),
            disk(lease.ledger_lock().native_binding())?,
        ))?;
        let identity = record(LedgerIdentityV1::from_genesis(&genesis))?;
        create(&ledger, "genesis.json", &record(genesis.canonical_bytes())?)?;
        let head = record(LedgerHeadV1::new(
            identity.clone(),
            0,
            record(genesis.digest())?,
        ))?;
        create(&ledger, "current.json", &record(head.canonical_bytes())?)?;
        // Genesis and current are durable before the seal and Floor0.
        let seal = record(RoleSealV2::new(identity.clone()))?;
        let floor = record(LedgerFloorV1::new(identity, 0, None, None))?;
        let mut bytes = frame(&record(seal.canonical_bytes())?)?;
        bytes.extend(frame(&record(floor.canonical_bytes())?)?);
        disk(lease.ledger_lock().append_ledger(&[], &bytes))?;
        disk(root.sync())?;
        let result = Self::open_existing(lease)?;
        result
            .state
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .complete()?;
        Ok(result)
    }

    pub(crate) fn open_existing(lease: EnforcerRoleLeaseV2) -> EnforcerResult<Self> {
        lease.revalidate()?;
        let owner = lease
            .ledger_lock()
            .claim_ledger_owner()
            .map_err(|_| EnforcerError::Conflict)?;
        let mut state = State::empty();
        if Self::scan(&lease, &mut state).is_err() {
            // The structural transport stays usable. Never reopen/reset auth,
            // genesis, inode, lock bytes, or an invalid suffix to repair it.
            lease.ledger_lock().taint_ledger();
            if !matches!(state.coverage, LedgerCoverageV1::Partial { .. }) {
                state.fail(LedgerScanPhaseV1::Seal, LedgerFailureV1::InvalidOrMissing);
            }
        }
        Ok(Self {
            lease,
            state: Mutex::new(state),
            _owner: owner,
        })
    }

    pub(crate) fn lease(&self) -> EnforcerRoleLeaseV2 {
        self.lease.clone()
    }
    pub(crate) fn coverage(&self) -> EnforcerResult<LedgerCoverageV1> {
        let mut state = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if state.complete().is_ok() {
            let _ = self.preflight(&mut state);
        }
        Ok(state.coverage.clone())
    }
    /// Positive retained records may fence restoration even when another file
    /// makes inventory partial. Absence here is never evidence of absence.
    pub(crate) fn known_entry(&self, arm: Uuid) -> EnforcerResult<Option<RetirementLedgerEntryV1>> {
        Ok(self
            .state
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .entries
            .get(&arm)
            .cloned())
    }
    /// Snapshot of positive prefix keys, including a Partial scan. These keys
    /// grant neither admission, verified retirement nor recovered quiescence.
    pub(crate) fn known_arms(&self) -> EnforcerResult<Vec<Uuid>> {
        let state = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        let mut arms: Vec<_> = state.entries.keys().copied().collect();
        arms.sort_unstable();
        Ok(arms)
    }
    pub(crate) fn query(
        &self,
        identity: &RetirementIdentityV1,
    ) -> EnforcerResult<Option<RetirementLedgerEntryV1>> {
        let mut state = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        state.complete()?;
        self.preflight(&mut state)?;
        let arm = Uuid::parse_str(&identity.arm().operation_id)
            .map_err(|_| EnforcerError::InvalidInput)?;
        if let Some(entry) = state.entries.get(&arm) {
            require(record(entry.accepted().identity().digest())? == record(identity.digest())?)?;
            return Ok(Some(entry.clone()));
        }
        Ok(None)
    }

    fn scan(lease: &EnforcerRoleLeaseV2, state: &mut State) -> EnforcerResult<()> {
        state.fail(LedgerScanPhaseV1::Seal, LedgerFailureV1::InvalidOrMissing);
        state.lock_bytes = disk(lease.ledger_lock().ledger_bytes())?;
        if state.lock_bytes.is_empty() {
            state.coverage = LedgerCoverageV1::Unverified;
            return Ok(());
        }
        let lock_snapshot = state.lock_bytes.clone();
        let (chunks, mut prefix_complete) = frames(&lock_snapshot)?;
        if chunks.len() < 2 {
            return Err(EnforcerError::Unhealthy);
        }
        let seal = record(RoleSealV2::from_canonical_bytes(chunks[0]))?;
        let identity = seal.ledger().clone();
        state.identity = Some(identity.clone());
        require(*identity.native_binding() == disk(lease.ledger_lock().native_binding())?)?;
        state.fail(
            LedgerScanPhaseV1::Genesis,
            LedgerFailureV1::InvalidOrMissing,
        );
        let root = child(lease.ledger_root(), "retirement")?;
        let genesis_file = read(&root, "genesis.json")?;
        let genesis = record(GenesisCoreV1::from_canonical_bytes(genesis_file.bytes()))?;
        require(
            record(LedgerIdentityV1::from_genesis(&genesis))?
                .matches(&identity)
                .map_err(|_| EnforcerError::Conflict)?,
        )?;
        let genesis_head = record(LedgerHeadV1::new(
            identity.clone(),
            0,
            record(genesis.digest())?,
        ))?;
        state.logical_bytes = state.lock_bytes.len() as u64
            + genesis_file.bytes().len() as u64
            + 2 * record(genesis_head.canonical_bytes())?.len() as u64;
        state.records.push(genesis_file);
        state.head = Some(genesis_head);
        state.fail(LedgerScanPhaseV1::Floors, LedgerFailureV1::InvalidOrMissing);
        for (index, chunk) in chunks[1..].iter().enumerate() {
            let parsed: EnforcerResult<LedgerFloorV1> = (|| {
                let floor = record(LedgerFloorV1::from_canonical_bytes(chunk))?;
                require(
                    record(floor.ledger().matches(&identity))? && *floor.revision() == index as u64,
                )?;
                let expected = state
                    .floors
                    .last()
                    .map(|old| record(old.digest()))
                    .transpose()?;
                require(floor.previous_floor_hash() == &expected)?;
                Ok(floor)
            })();
            match parsed {
                Ok(floor) => state.floors.push(floor),
                Err(_) => {
                    prefix_complete = false;
                    break;
                }
            }
        }
        require(state.floors.len() <= MAX_FLOORS)?;
        let deltas = child(&root, "deltas")?;
        let tombstones = child(&root, "tombstones")?;
        let delta_entries = disk(deltas.persistent_entries(MAX_REVISIONS as u32 + 1))?;
        state.fail(LedgerScanPhaseV1::Deltas, LedgerFailureV1::InvalidOrMissing);
        // A delta ahead of the floor has not committed any entry; it is never
        // silently adopted. Exactly committed floors may advance current.
        let delta_count_matches = delta_entries.len() + 1 == state.floors.len();
        for revision in 1..state.floors.len() as u64 {
            let file = read(&deltas, &delta_name(revision))?;
            let delta = record(RetirementDeltaV1::from_canonical_bytes(file.bytes()))?;
            let old_head = state.head.as_ref().ok_or(EnforcerError::Unhealthy)?;
            require(
                record(delta.ledger().matches(&identity))?
                    && *delta.revision() == revision
                    && delta.previous_hash() == old_head.hash(),
            )?;
            let hash = record(delta.digest())?;
            require(state.floors[revision as usize].delta_hash() == &Some(hash.clone()))?;
            record(delta.validate_previous(state.entries.get(delta.arm_id())))?;
            Self::check_retirement_id(state, *delta.arm_id(), delta.transition().entry())?;
            state
                .entries
                .insert(*delta.arm_id(), delta.transition().entry().clone());
            let head = record(LedgerHeadV1::new(identity.clone(), revision, hash))?;
            state.logical_bytes +=
                file.bytes().len() as u64 + 2 * record(head.canonical_bytes())?.len() as u64;
            state.records.push(file);
            state.head = Some(head);
        }
        // Keep positive entries from the validated contiguous prefix for
        // fencing, even when an invalid suffix closes all Full permissions.
        if !prefix_complete || !delta_count_matches {
            state.fail(LedgerScanPhaseV1::Floors, LedgerFailureV1::InvalidOrMissing);
            return Err(EnforcerError::Unhealthy);
        }
        disk(lease.ledger_lock().synchronize_ledger(&state.lock_bytes))?;
        disk(lease.ledger_root().sync())?;
        state.fail(
            LedgerScanPhaseV1::Tombstones,
            LedgerFailureV1::InvalidOrMissing,
        );
        let names = disk(tombstones.persistent_entries(MAX_ARMS as u32 + 1))?;
        for name in &names {
            require(!name.directory)?;
            let arm = name
                .name
                .strip_prefix("t-")
                .and_then(|n| n.strip_suffix(".json"))
                .and_then(|n| Uuid::parse_str(n).ok())
                .ok_or(EnforcerError::Conflict)?;
            require(tombstone_name(arm) == name.name && state.entries.contains_key(&arm))?;
        }
        for (arm, entry) in &state.entries {
            let expected = record(RetiredTombstoneV1::new(
                identity.clone(),
                record(entry.accepted().identity().digest())?,
                record(entry.accepted().digest())?,
            ))?;
            let file =
                disk(tombstones.persistent_read(&tombstone_name(*arm), MAX_RECORD_BYTES, &[]))?;
            if let Some(file) = file {
                require(file.bytes() == record(expected.canonical_bytes())?)?;
                state.logical_bytes += file.bytes().len() as u64;
                state.records.push(file);
            } else {
                require(matches!(entry.phase(), RetirementLedgerPhaseV1::Closing))?;
            }
        }
        state.fail(
            LedgerScanPhaseV1::Pointer,
            LedgerFailureV1::InvalidOrMissing,
        );
        let current = read(&root, "current.json")?;
        let pointer = record(LedgerHeadV1::from_canonical_bytes(current.bytes()))?;
        let head = state.head.as_ref().ok_or(EnforcerError::Unhealthy)?;
        require(
            record(pointer.ledger().matches(&identity))? && pointer.revision() <= head.revision(),
        )?;
        // Only the exact last prepared CAS is recoverable; a rollback of more
        // than one publication is diagnosed against the retained floor.
        if pointer.revision() != head.revision() {
            require(*pointer.revision() + 1 == *head.revision())?;
            let last_delta = record(RetirementDeltaV1::from_canonical_bytes(
                read(&deltas, &delta_name(*head.revision()))?.bytes(),
            ))?;
            require(pointer.hash() == last_delta.previous_hash())?;
            let next = match disk(root.persistent_read("current-next.json", MAX_RECORD_BYTES, &[]))?
            {
                Some(next) => next,
                None => create(&root, "current-next.json", &record(head.canonical_bytes())?)?,
            };
            require(next.bytes() == record(head.canonical_bytes())?)?;
            disk(root.persistent_publish(&next, &current, "current.json"))?;
        } else {
            require(pointer.hash() == head.hash())?;
        }
        state.current = Some(read(&root, "current.json")?);
        // Nothing except the three recognized namespaces/files may hide bytes.
        let entries = disk(root.persistent_entries(8))?;
        require(
            entries.len() == 4
                && entries.iter().all(|e| match e.name.as_str() {
                    "genesis.json" | "current.json" => !e.directory,
                    "deltas" | "tombstones" => e.directory,
                    _ => false,
                }),
        )?;
        state.fail(LedgerScanPhaseV1::Budget, LedgerFailureV1::BoundExceeded);
        Self::recompute_reservations(state)?;
        Self::check_budget(state, 0, 0, 0)?;
        state.refresh()
    }

    pub(crate) fn begin_closing(
        &self,
        accepted: ClosingAcceptanceV1,
    ) -> EnforcerResult<ClosingCommitted> {
        let mut state = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        state.complete()?;
        self.preflight(&mut state)?;
        let identity = state.identity.clone().ok_or(EnforcerError::Unhealthy)?;
        let arm_id = Uuid::parse_str(&accepted.identity().arm().operation_id)
            .map_err(|_| EnforcerError::InvalidInput)?;
        if let Some(entry) = state.entries.get(&arm_id) {
            require(record(entry.accepted().digest())? == record(accepted.digest())?)?;
            return Ok(ClosingCommitted {
                ledger: identity,
                arm_id,
                entry_hash: record(entry.digest())?,
                accepted: entry.accepted().clone(),
            });
        }
        if state.entries.len() >= MAX_ARMS {
            return Err(EnforcerError::Capacity);
        }
        let tombstone = record(RetiredTombstoneV1::new(
            identity.clone(),
            record(accepted.identity().digest())?,
            record(accepted.digest())?,
        ))?;
        let entry = record(RetirementLedgerEntryV1::new(
            accepted,
            record(tombstone.digest())?,
            RetirementLedgerPhaseV1::Closing,
        ))?;
        Self::check_retirement_id(&state, arm_id, &entry)?;
        // Reserve all subsequent records before committing Closing.
        let (reserved, lock, revisions) = Self::remaining_reservation(&identity, &entry)?;
        let old_reservation = (
            state.reserved_bytes,
            state.reserved_lock,
            state.reserved_revisions,
        );
        state.reserved_bytes += reserved;
        state.reserved_lock += lock;
        state.reserved_revisions += revisions;
        let result = self.publish(
            &mut state,
            arm_id,
            RetirementLedgerTransitionV1::BeginClosing(entry.clone()),
        );
        if result.is_err() && matches!(state.coverage, LedgerCoverageV1::Complete { .. }) {
            (
                state.reserved_bytes,
                state.reserved_lock,
                state.reserved_revisions,
            ) = old_reservation;
        }
        result?;
        Ok(ClosingCommitted {
            ledger: identity,
            arm_id,
            entry_hash: record(entry.digest())?,
            accepted: entry.accepted().clone(),
        })
    }

    pub(crate) fn commit_tombstone(
        &self,
        closing: &ClosingCommitted,
    ) -> EnforcerResult<TombstoneCommitted> {
        let mut state = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        state.complete()?;
        self.preflight(&mut state)?;
        let identity = state.identity.clone().ok_or(EnforcerError::Unhealthy)?;
        require(record(closing.ledger.matches(&identity))?)?;
        let old = state
            .entries
            .get(&closing.arm_id)
            .cloned()
            .ok_or(EnforcerError::Conflict)?;
        if !matches!(old.phase(), RetirementLedgerPhaseV1::Closing) {
            return Ok(TombstoneCommitted {
                arm_id: closing.arm_id,
                entry_hash: record(old.digest())?,
                tombstone: record(RetiredTombstoneV1::new(
                    identity.clone(),
                    record(old.accepted().identity().digest())?,
                    record(old.accepted().digest())?,
                ))?,
                ledger: identity,
            });
        }
        require(record(old.digest())? == closing.entry_hash)?;
        let tombstone = record(RetiredTombstoneV1::new(
            identity.clone(),
            record(old.accepted().identity().digest())?,
            record(old.accepted().digest())?,
        ))?;
        let root = child(self.lease.ledger_root(), "retirement")?;
        let tombstones = child(&root, "tombstones")?;
        let bytes = record(tombstone.canonical_bytes())?;
        let mutation = (|| {
            if let Some(existing) = disk(tombstones.persistent_read(
                &tombstone_name(closing.arm_id),
                MAX_RECORD_BYTES,
                &[],
            ))? {
                require(existing.bytes() == bytes)?;
                disk(existing.persistent_sync())?;
                disk(tombstones.sync())?;
                Ok(existing)
            } else {
                create(&tombstones, &tombstone_name(closing.arm_id), &bytes)
            }
        })();
        let file = match mutation {
            Ok(file) => file,
            Err(error) => {
                self.taint(&mut state, LedgerScanPhaseV1::Tombstones);
                return Err(error);
            }
        };
        if !state.records.iter().any(|old| old.path() == file.path()) {
            state.logical_bytes += bytes.len() as u64;
            state.records.push(file);
        }
        #[cfg(test)]
        if let Err(error) = tests::checkpoint(FaultStage::TombstoneSynced) {
            self.taint(&mut state, LedgerScanPhaseV1::Tombstones);
            return Err(error);
        }
        let entry = record(RetirementLedgerEntryV1::new(
            old.accepted().clone(),
            old.expected_tombstone_hash().clone(),
            RetirementLedgerPhaseV1::Tombstone {
                tombstone_hash: record(tombstone.digest())?,
            },
        ))?;
        // The old reservation included the now durable tombstone. Recompute
        // only after its corresponding delta has been acknowledged.
        let prior_reservation = Self::remaining_reservation(&identity, &old)?;
        state.reserved_bytes = state.reserved_bytes.saturating_sub(prior_reservation.0);
        state.reserved_lock = state.reserved_lock.saturating_sub(prior_reservation.1);
        state.reserved_revisions = state.reserved_revisions.saturating_sub(prior_reservation.2);
        let next_reservation = Self::remaining_reservation(&identity, &entry)?;
        state.reserved_bytes += next_reservation.0;
        state.reserved_lock += next_reservation.1;
        state.reserved_revisions += next_reservation.2;
        self.publish(
            &mut state,
            closing.arm_id,
            RetirementLedgerTransitionV1::CommitTombstone(entry.clone()),
        )?;
        Ok(TombstoneCommitted {
            ledger: identity,
            arm_id: closing.arm_id,
            entry_hash: record(entry.digest())?,
            tombstone,
        })
    }

    /// Only the retained native owner can produce `JoinedQuiescence`. Its
    /// durable record is inspected here after consuming that live permission;
    /// no deserialized record, status, bool or historical join enters this API.
    pub(crate) fn finalize(
        &self,
        closing: &ClosingCommitted,
        tombstone: &TombstoneCommitted,
        joined: super::retirement::JoinedQuiescence,
    ) -> EnforcerResult<(RetiredReceiptV1, LedgerHeadV1)> {
        let mut state = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        state.complete()?;
        self.preflight(&mut state)?;
        let ledger = state.identity.clone().ok_or(EnforcerError::Unhealthy)?;
        require(
            record(closing.ledger.matches(&ledger))?
                && record(tombstone.ledger.matches(&ledger))?
                && closing.arm_id == tombstone.arm_id,
        )?;
        let previous = state
            .entries
            .get(&closing.arm_id)
            .cloned()
            .ok_or(EnforcerError::Conflict)?;
        require(!matches!(
            previous.phase(),
            RetirementLedgerPhaseV1::Closing
        ))?;
        let accepted = previous.accepted();
        let identity_hash = record(accepted.identity().digest())?;
        let closing_hash = record(accepted.digest())?;
        let tombstone_hash = previous.expected_tombstone_hash().clone();
        require(
            record(closing.accepted.digest())? == closing_hash
                && record(tombstone.tombstone.ledger().matches(&ledger))?
                && tombstone.tombstone.identity_hash() == &identity_hash
                && tombstone.tombstone.closing_hash() == &closing_hash
                && record(tombstone.tombstone.digest())? == tombstone_hash,
        )?;

        // Replays may have obtained the opaque publication handles at C, T or
        // Q. Each must still commit this exact immutable acceptance and phase.
        let closing_entry = record(RetirementLedgerEntryV1::new(
            accepted.clone(),
            tombstone_hash.clone(),
            RetirementLedgerPhaseV1::Closing,
        ))?;
        let tombstone_entry = record(RetirementLedgerEntryV1::new(
            accepted.clone(),
            tombstone_hash.clone(),
            RetirementLedgerPhaseV1::Tombstone {
                tombstone_hash: tombstone_hash.clone(),
            },
        ))?;
        let closing_entry_hash = record(closing_entry.digest())?;
        let tombstone_entry_hash = record(tombstone_entry.digest())?;
        let previous_hash = record(previous.digest())?;
        require(
            closing.entry_hash == closing_entry_hash
                || closing.entry_hash == tombstone_entry_hash
                || closing.entry_hash == previous_hash,
        )?;
        require(
            tombstone.entry_hash == tombstone_entry_hash || tombstone.entry_hash == previous_hash,
        )?;

        let joined_record = joined.into_record();
        record(joined_record.validate())?;
        require(
            joined_record.identity_hash() == &identity_hash
                && joined_record.closing_hash() == &closing_hash
                && joined_record.tombstone_hash() == &tombstone_hash
                && joined_record.original_epoch() == accepted.identity().original_epoch()
                && joined_record.original_actor_nonce()
                    == accepted.evidence().original_actor_nonce(),
        )?;
        if let RetirementLedgerPhaseV1::Final { receipt, .. } = previous.phase() {
            return Ok((
                receipt.as_ref().clone(),
                state.head.clone().ok_or(EnforcerError::Unhealthy)?,
            ));
        }
        let finalization = record(FinalizationCoreV1::new(
            identity_hash.clone(),
            closing_hash,
            tombstone_hash.clone(),
            joined_record,
        ))?;
        let receipt = record(RetiredReceiptV1::new(
            ledger.clone(),
            *accepted.identity().retirement_id(),
            identity_hash,
            finalization,
        ))?;
        let next = record(RetirementLedgerEntryV1::new(
            accepted.clone(),
            tombstone_hash.clone(),
            RetirementLedgerPhaseV1::Final {
                tombstone_hash,
                receipt: Box::new(receipt.clone()),
            },
        ))?;
        let reserved = Self::remaining_reservation(&ledger, &previous)?;
        let remaining = state
            .reserved_bytes
            .checked_sub(reserved.0)
            .zip(state.reserved_lock.checked_sub(reserved.1))
            .zip(state.reserved_revisions.checked_sub(reserved.2));
        match remaining {
            Some(((bytes, lock), revisions)) => {
                state.reserved_bytes = bytes;
                state.reserved_lock = lock;
                state.reserved_revisions = revisions;
            }
            None => {
                self.taint(&mut state, LedgerScanPhaseV1::Budget);
                return Err(EnforcerError::Unhealthy);
            }
        }
        if let Err(error) = self.publish(
            &mut state,
            closing.arm_id,
            RetirementLedgerTransitionV1::Finalize(next),
        ) {
            self.taint(&mut state, LedgerScanPhaseV1::Deltas);
            return Err(error);
        }
        Ok((receipt, state.head.clone().ok_or(EnforcerError::Unhealthy)?))
    }

    fn taint(&self, state: &mut State, phase: LedgerScanPhaseV1) {
        self.lease.ledger_lock().taint_ledger();
        state.fail(phase, LedgerFailureV1::Tainted);
    }

    fn preflight(&self, state: &mut State) -> EnforcerResult<()> {
        state.complete()?;
        let result = (|| {
            self.lease.revalidate()?;
            require(disk(self.lease.ledger_lock().ledger_bytes())? == state.lock_bytes)?;
            disk(
                state
                    .current
                    .as_ref()
                    .ok_or(EnforcerError::Unhealthy)?
                    .persistent_revalidate(),
            )?;
            for file in &state.records {
                disk(file.persistent_revalidate())?;
            }
            let root = child(self.lease.ledger_root(), "retirement")?;
            let expected_deltas = *state
                .head
                .as_ref()
                .ok_or(EnforcerError::Unhealthy)?
                .revision() as usize;
            require(
                disk(child(&root, "deltas")?.persistent_entries(MAX_REVISIONS as u32 + 1))?.len()
                    == expected_deltas,
            )?;
            let expected_tombs = state
                .records
                .iter()
                .filter(|f| f.path().parent() == Some(root.path().join("tombstones").as_path()))
                .count();
            require(
                disk(child(&root, "tombstones")?.persistent_entries(MAX_ARMS as u32 + 1))?.len()
                    == expected_tombs,
            )?;
            require(disk(root.persistent_entries(8))?.len() == 4)?;
            Ok(())
        })();
        if result.is_err() {
            self.taint(state, LedgerScanPhaseV1::Deltas);
        }
        result
    }

    fn publish(
        &self,
        state: &mut State,
        arm_id: Uuid,
        transition: RetirementLedgerTransitionV1,
    ) -> EnforcerResult<()> {
        state.complete()?;
        let identity = state.identity.clone().ok_or(EnforcerError::Unhealthy)?;
        let old = state.head.as_ref().ok_or(EnforcerError::Unhealthy)?;
        let revision = old
            .revision()
            .checked_add(1)
            .ok_or(EnforcerError::Capacity)?;
        let delta = record(RetirementDeltaV1::new(
            identity.clone(),
            revision,
            old.hash().clone(),
            arm_id,
            state
                .entries
                .get(&arm_id)
                .map(|e| record(e.digest()))
                .transpose()?,
            transition,
        ))?;
        record(delta.validate_previous(state.entries.get(&arm_id)))?;
        let bytes = record(delta.canonical_bytes())?;
        let hash = record(delta.digest())?;
        let floor = record(LedgerFloorV1::new(
            identity.clone(),
            revision,
            Some(hash.clone()),
            Some(record(
                state
                    .floors
                    .last()
                    .ok_or(EnforcerError::Unhealthy)?
                    .digest(),
            )?),
        ))?;
        let framed = frame(&record(floor.canonical_bytes())?)?;
        let head = record(LedgerHeadV1::new(identity, revision, hash))?;
        let pointer = record(head.canonical_bytes())?;
        let logical = bytes.len() as u64 + framed.len() as u64 + 2 * pointer.len() as u64;
        Self::check_budget(state, logical, framed.len(), 1)?;
        let result = (|| {
            let root = child(self.lease.ledger_root(), "retirement")?;
            let deltas = child(&root, "deltas")?;
            disk(
                state
                    .current
                    .as_ref()
                    .ok_or(EnforcerError::Unhealthy)?
                    .persistent_revalidate(),
            )?;
            let delta_file = create(&deltas, &delta_name(revision), &bytes)?;
            fault!(DeltaSynced);
            disk(
                self.lease
                    .ledger_lock()
                    .append_ledger(&state.lock_bytes, &framed),
            )?;
            fault!(FloorSynced);
            let next = create(&root, "current-next.json", &pointer)?;
            disk(root.persistent_publish(
                &next,
                state.current.as_ref().ok_or(EnforcerError::Unhealthy)?,
                "current.json",
            ))?;
            fault!(PointerSynced);
            state.current = Some(read(&root, "current.json")?);
            state.lock_bytes.extend_from_slice(&framed);
            state.floors.push(floor);
            state.logical_bytes += logical;
            state.head = Some(head);
            state
                .entries
                .insert(arm_id, delta.transition().entry().clone());
            state.records.push(delta_file);
            Self::recompute_reservations(state)?;
            state.refresh()
        })();
        if result.is_err() {
            self.taint(state, LedgerScanPhaseV1::Deltas);
        }
        result
    }

    fn check_budget(
        state: &State,
        logical: u64,
        lock: usize,
        revisions: u64,
    ) -> EnforcerResult<()> {
        let okay = state
            .logical_bytes
            .checked_add(state.reserved_bytes)
            .and_then(|n| n.checked_add(logical))
            .is_some_and(|n| n <= MAX_LOGICAL_BYTES)
            && state
                .lock_bytes
                .len()
                .checked_add(state.reserved_lock)
                .and_then(|n| n.checked_add(lock))
                .is_some_and(|n| n <= MAX_LOCK_BYTES)
            && state.floors.len() as u64 + state.reserved_revisions + revisions
                <= MAX_FLOORS as u64
            && state.head.as_ref().is_some_and(|h| {
                *h.revision() + state.reserved_revisions + revisions <= MAX_REVISIONS
            })
            && state.entries.len() <= MAX_ARMS;
        if okay {
            Ok(())
        } else {
            Err(EnforcerError::Capacity)
        }
    }

    fn check_retirement_id(
        state: &State,
        arm_id: Uuid,
        next: &RetirementLedgerEntryV1,
    ) -> EnforcerResult<()> {
        let identity = next.accepted().identity();
        let scope = record(crate::broker::journal::canonical(identity.scope()))?;
        for (other_arm, other) in &state.entries {
            let previous = other.accepted().identity();
            if *other_arm != arm_id
                && previous.job_operation_id() == identity.job_operation_id()
                && previous.retirement_id() == identity.retirement_id()
            {
                // A job is scoped by every persisted JournalScope field.
                // Another profile/tenant/origin/namespace cannot consume its
                // operation or retirement identity merely by sharing UUIDs.
                require(record(crate::broker::journal::canonical(previous.scope()))? != scope)?;
            }
        }
        Ok(())
    }

    /// These records compute worst-case byte sizes only. They are never
    /// published, returned as permissions, or accepted by a finalize API.
    fn remaining_reservation(
        ledger: &LedgerIdentityV1,
        entry: &RetirementLedgerEntryV1,
    ) -> EnforcerResult<(u64, usize, u64)> {
        if matches!(entry.phase(), RetirementLedgerPhaseV1::Final { .. }) {
            return Ok((0, 0, 0));
        }
        let accepted = entry.accepted();
        let identity_hash = record(accepted.identity().digest())?;
        let closing_hash = record(accepted.digest())?;
        let tomb_hash = entry.expected_tombstone_hash().clone();
        let tombstone = record(RetiredTombstoneV1::new(
            ledger.clone(),
            identity_hash.clone(),
            closing_hash.clone(),
        ))?;
        let joined_sizes_only = record(JoinedQuiescenceV1::new(
            identity_hash.clone(),
            closing_hash.clone(),
            tomb_hash.clone(),
            *accepted.identity().original_epoch(),
            *accepted.evidence().original_actor_nonce(),
        ))?;
        let core_sizes_only = record(FinalizationCoreV1::new(
            identity_hash.clone(),
            closing_hash,
            tomb_hash.clone(),
            joined_sizes_only,
        ))?;
        let receipt_sizes_only = record(RetiredReceiptV1::new(
            ledger.clone(),
            *accepted.identity().retirement_id(),
            identity_hash,
            core_sizes_only,
        ))?;
        let final_entry = record(RetirementLedgerEntryV1::new(
            accepted.clone(),
            tomb_hash.clone(),
            RetirementLedgerPhaseV1::Final {
                tombstone_hash: tomb_hash.clone(),
                receipt: Box::new(receipt_sizes_only),
            },
        ))?;
        let tomb_entry = record(RetirementLedgerEntryV1::new(
            accepted.clone(),
            tomb_hash.clone(),
            RetirementLedgerPhaseV1::Tombstone {
                tombstone_hash: tomb_hash,
            },
        ))?;
        let arm = Uuid::parse_str(&accepted.identity().arm().operation_id)
            .map_err(|_| EnforcerError::InvalidInput)?;
        let dummy_hash = record(entry.digest())?;
        let floor = record(LedgerFloorV1::new(
            ledger.clone(),
            MAX_REVISIONS,
            Some(dummy_hash.clone()),
            Some(dummy_hash.clone()),
        ))?;
        let lock_per = frame(&record(floor.canonical_bytes())?)?.len();
        let pointer_per = record(LedgerHeadV1::new(
            ledger.clone(),
            MAX_REVISIONS,
            dummy_hash.clone(),
        ))?;
        let pointer_bytes = 2 * record(pointer_per.canonical_bytes())?.len() as u64;
        let mut bytes = 0;
        let mut revisions = 0;
        let mut previous = entry.clone();
        let mut transitions = Vec::new();
        if matches!(entry.phase(), RetirementLedgerPhaseV1::Closing) {
            bytes += record(tombstone.canonical_bytes())?.len() as u64;
            transitions.push(RetirementLedgerTransitionV1::CommitTombstone(tomb_entry));
        }
        transitions.push(RetirementLedgerTransitionV1::Finalize(final_entry));
        for transition in transitions {
            let delta = record(RetirementDeltaV1::new(
                ledger.clone(),
                MAX_REVISIONS,
                dummy_hash.clone(),
                arm,
                Some(record(previous.digest())?),
                transition,
            ))?;
            let size = record(delta.canonical_bytes())?.len() as u64;
            if size > MAX_RECORD_BYTES {
                return Err(EnforcerError::Capacity);
            }
            bytes += size + lock_per as u64 + pointer_bytes;
            revisions += 1;
            previous = delta.transition().entry().clone();
        }
        Ok((bytes, lock_per * revisions as usize, revisions))
    }
    fn recompute_reservations(state: &mut State) -> EnforcerResult<()> {
        let identity = state.identity.as_ref().ok_or(EnforcerError::Unhealthy)?;
        let (mut bytes, mut lock, mut revisions) = (0, 0, 0);
        for entry in state.entries.values() {
            let remaining = Self::remaining_reservation(identity, entry)?;
            bytes += remaining.0;
            lock += remaining.1;
            revisions += remaining.2;
        }
        state.reserved_bytes = bytes;
        state.reserved_lock = lock;
        state.reserved_revisions = revisions;
        Ok(())
    }
}

#[cfg(test)]
#[path = "ledger_tests.rs"]
mod tests;
