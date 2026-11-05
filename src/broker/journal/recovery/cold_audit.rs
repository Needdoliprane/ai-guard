//! Internal, metadata-only preparation for retained V4 audit events.
//!
//! This surface is not authorization to send HTTP or acknowledge an event. It
//! cannot obtain a full journal, start permission, runtime or credential blob.
//! The capacity reserve is logical, in memory and conservative. It neither
//! reserves filesystem blocks/inodes nor makes a publication immune to I/O
//! failure. No persistent acknowledgement is exposed by this module.

use super::*;
use crate::broker::{lifecycle::LocalCommandV1, outbox::OutboxEventV1};
use serde::ser::{Error as _, SerializeSeq};
use std::{cell::Cell, io::Write};

const COMMANDS_LIMIT: u64 = 64 * 1024;
const OUTBOX_LIMIT: u64 = 4 * 1024 * 1024;
const COMMAND_LIMIT: u64 = 64;

/// The strong owner of one logical reservation, not a physical disk quota.
/// Its only registry entry is weak, so a lost preparation response cannot pin
/// capacity. No Drop implementation takes the journal mutex.
pub(in crate::broker::journal) struct ReservationState {
    operation_id: Uuid,
    intent_hash: String,
    head: RevisionRefV1,
    jobs: Vec<JobBound>,
}

/// A retained lease, exact selected metadata and one logical capacity reserve.
/// Neither this value nor its descriptive events prove central identity,
/// terminal ownership, Backend::Stops admission or an HTTP acknowledgement.
pub(in crate::broker) struct PreparedColdAudit {
    owner: Arc<Inner>,
    reservation: Arc<ReservationState>,
    selected: RecoveredJobV1,
    pending_events: Vec<OutboxEventV1>,
}

impl fmt::Debug for PreparedColdAudit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PreparedColdAudit(<private>)")
    }
}

impl PreparedColdAudit {
    pub(in crate::broker) fn operation_id(&self) -> Uuid {
        self.selected.operation_id
    }

    pub(in crate::broker) fn scope(&self) -> &JournalScopeV1 {
        &self.owner.scope
    }

    pub(in crate::broker) fn head(&self) -> &RevisionRefV1 {
        &self.selected.head
    }

    /// At most two exact pending entries copied from the retained V4 pair.
    pub(in crate::broker) fn pending_events(&self) -> &[OutboxEventV1] {
        &self.pending_events
    }
}

/// Physical metadata accounting, including every observed job and blob size.
/// Inspecting blob metadata does not open its contents. Unknown usage is an
/// error rather than an empty/default instance returned by the inventory.
#[derive(Clone, Default)]
struct PhysicalUsage {
    bytes: u64,
    entries: u32,
    jobs: HashMap<Uuid, JobUsage>,
    // Inventory alone never establishes a closed rejection. Preparation adds
    // these only after the strict reader and shared rejection floor agree.
    verified_rejections: HashSet<Uuid>,
}

#[derive(Clone, Default)]
struct JobUsage {
    bytes: u64,
    entries: u32,
    revisions: u32,
}

/// Scalar upper bounds from one additional sizing serialization of this job.
/// V1/V2 are measured in the V3 shape of their next Cancel. The selected V4
/// sizing view replaces pending audit_seq values with i64::MAX without
/// modifying its retained payload or persisting a synthetic acknowledgement.
#[derive(Clone)]
struct JobBound {
    operation_id: Uuid,
    head: RevisionRefV1,
    remaining_cancels: u64,
    checkpoint_bytes: u64,
    pointer_bytes: u64,
    cancel_delta_bytes: u64,
    commands_bytes: u64,
    outbox_bytes: Option<u64>,
    cancel_bytes: u64,
    completion_bytes: u64,
}

opaque!(ReservationState, PhysicalUsage, JobUsage, JobBound);

pub(in crate::broker) fn prepare(
    journal: &StopJournal,
    operation_id: Uuid,
) -> JournalResult<PreparedColdAudit> {
    if operation_id.is_nil() {
        return Err(JournalError::InvalidInput);
    }
    let mut writer = journal
        .core
        .writer
        .lock()
        .map_err(|_| JournalError::Corrupt)?;
    if writer.cold_audit.upgrade().is_some() {
        return Err(JournalError::Busy);
    }
    if journal.scope_incomplete {
        return Err(JournalError::Interrupted);
    }
    let mut physical = inventory_usage(&journal.core)?;
    if writer
        .observed_jobs
        .iter()
        .chain(writer.floors.keys())
        .chain(writer.rejected_floors.keys())
        .any(|id| !physical.jobs.contains_key(id))
    {
        return Err(JournalError::Conflict);
    }
    if !physical.jobs.contains_key(&operation_id) {
        return Err(JournalError::InvalidInput);
    }

    // The scope inventory above is shared by all reads. Calling read_locked
    // for each job would repeat its entire scope inventory N times. Keep its
    // existence/floor/rejection checks here, using the same strict job reader.
    let reader = StopRecoveryReaderV1::from_core(&journal.core)?;
    let mut ids: Vec<_> = physical.jobs.keys().copied().collect();
    ids.sort_unstable();
    let mut bounds = Vec::new();
    let mut selected = None;
    for id in ids {
        writer.observed_jobs.insert(id);
        let mut records = Usage::default();
        let mut blobs = RecoveryBlobBytes::default();
        let (recovered, rejected, partial) = reader.read_job(id, &mut records, &mut blobs)?;
        if writer
            .rejected_floors
            .get(&id)
            .is_some_and(|old| Some(old) != rejected.as_ref())
            || writer.floors.contains_key(&id) && rejected.is_some()
        {
            return Err(JournalError::Conflict);
        }
        if let Some(hash) = rejected {
            if recovered.is_some() {
                return Err(JournalError::Corrupt);
            }
            writer.rejected_floors.insert(id, hash);
            if partial || id == operation_id {
                return Err(JournalError::Interrupted);
            }
            compare_usage(&physical, id, &records, &blobs)?;
            physical.verified_rejections.insert(id);
            continue;
        }
        if writer.rejected_floors.contains_key(&id) {
            return Err(JournalError::Conflict);
        }
        let recovered = recovered.ok_or(if writer.floors.contains_key(&id) {
            JournalError::Conflict
        } else {
            JournalError::Interrupted
        })?;
        if let Some(floor) = writer.floors.get(&id) {
            let index = usize::try_from(floor.revision).map_err(|_| JournalError::Corrupt)?;
            if recovered.chain.get(index) != Some(floor) {
                return Err(JournalError::Conflict);
            }
        }
        writer.floors.insert(id, recovered.head.clone());
        if partial {
            return Err(JournalError::Interrupted);
        }
        compare_usage(&physical, id, &records, &blobs)?;
        let metadata = recovered.metadata.as_ref().ok_or(JournalError::Corrupt)?;
        bounds.push(estimate_job(
            id,
            &recovered.head,
            metadata,
            id == operation_id,
        )?);
        if id == operation_id {
            selected = Some(recovered);
        }
        // Every sibling's payload is dropped here. The reservation retains
        // scalar bounds, not a scope full of prompts and checkpoint bodies.
    }
    let selected = selected.ok_or(JournalError::InvalidInput)?;
    check_capacity(&journal.core.limits, &physical, &bounds)?;
    let completion = bounds
        .iter()
        .find(|bound| bound.completion_bytes != 0)
        .ok_or(JournalError::Conflict)?;
    if completion.operation_id != operation_id || completion.head != selected.head {
        return Err(JournalError::Conflict);
    }
    reader.check_shared()?;
    let metadata = selected.metadata.as_ref().ok_or(JournalError::Corrupt)?;
    let outbox = metadata
        .checkpoint
        .outbox()
        .ok_or(JournalError::UnsupportedVersion)?;
    check_selected_pair(outbox)?;
    let pending_events = outbox
        .events()
        .iter()
        .filter(|event| event.audit_seq().is_none())
        .cloned()
        .collect();
    let reservation = Arc::new(ReservationState {
        operation_id,
        intent_hash: metadata
            .intent
            .intent_hash()
            .map_err(|_| JournalError::Corrupt)?,
        head: selected.head.clone(),
        jobs: bounds,
    });
    // Install only after every bound and revalidation succeeded. The reply
    // owns the only strong reference, so an undelivered reply cannot leak it.
    writer.cold_audit = Arc::downgrade(&reservation);
    Ok(PreparedColdAudit {
        owner: Arc::clone(&journal.core),
        reservation,
        selected,
        pending_events,
    })
}

fn compare_usage(
    physical: &PhysicalUsage,
    id: Uuid,
    records: &Usage,
    blobs: &RecoveryBlobBytes,
) -> JournalResult<()> {
    let expected = physical.jobs.get(&id).ok_or(JournalError::Conflict)?;
    let bytes = records
        .job_bytes
        .checked_add(blobs.job.ok_or(JournalError::Interrupted)?)
        .ok_or(JournalError::LimitExceeded)?;
    if bytes != expected.bytes || records.job_entries != expected.entries {
        return Err(JournalError::Conflict);
    }
    Ok(())
}

impl PhysicalUsage {
    fn add(&mut self, bytes: u64, limits: &JournalLimitsV1) -> JournalResult<()> {
        self.bytes = add(self.bytes, bytes)?;
        self.entries = self
            .entries
            .checked_add(1)
            .ok_or(JournalError::LimitExceeded)?;
        if self.bytes > limits.scope_bytes || self.entries > limits.scope_entries {
            return Err(JournalError::LimitExceeded);
        }
        Ok(())
    }
}

impl JobUsage {
    fn add(&mut self, bytes: u64, limits: &JournalLimitsV1) -> JournalResult<()> {
        self.bytes = add(self.bytes, bytes)?;
        self.entries = self
            .entries
            .checked_add(1)
            .ok_or(JournalError::LimitExceeded)?;
        if self.bytes > limits.job_bytes || self.entries > limits.job_entries {
            return Err(JournalError::LimitExceeded);
        }
        Ok(())
    }
}

fn inventory_usage(core: &Inner) -> JournalResult<PhysicalUsage> {
    let reader = StopRecoveryReaderV1::from_core(core)?;
    // Preserve read_locked's shared namespace completeness check once, rather
    // than once per selected/sibling job. These parent entries are outside
    // the scope's physical budget, as in stop_budget_usage.
    for entry in fscheck(
        reader
            .namespace
            .persistent_entries(core.limits.scope_entries),
    )? {
        if !entry.directory || entry.name != core.scope_hash {
            return Err(JournalError::Interrupted);
        }
    }
    let mut usage = PhysicalUsage::default();
    for entry in fscheck(core.directory.persistent_entries(core.limits.scope_entries))? {
        if !matches!(entry.name.as_str(), ".writer.lock" | "scope.json" | "jobs")
            || entry.directory != (entry.name == "jobs")
        {
            return Err(JournalError::Interrupted);
        }
        usage.add(entry.bytes, &core.limits)?;
    }
    for entry in fscheck(core.jobs.persistent_entries(core.limits.scope_entries))? {
        let id = Uuid::parse_str(&entry.name).map_err(|_| JournalError::Corrupt)?;
        if !entry.directory || id.is_nil() || id.to_string() != entry.name {
            return Err(JournalError::Corrupt);
        }
        if usage.jobs.len() >= core.limits.jobs as usize {
            return Err(JournalError::LimitExceeded);
        }
        usage.add(entry.bytes, &core.limits)?;
        let mut job_usage = JobUsage::default();
        job_usage.add(entry.bytes, &core.limits)?;
        let job = fscheck(core.jobs.persistent_child(&entry.name, false))?
            .ok_or(JournalError::Interrupted)?;
        for file in fscheck(job.persistent_entries(core.limits.job_entries))? {
            usage.add(file.bytes, &core.limits)?;
            job_usage.add(file.bytes, &core.limits)?;
            if file.directory {
                if file.name != "bindings" && file.name != crate::runtime::launch_profile::DIRECTORY
                {
                    return Err(JournalError::Corrupt);
                }
                let nested = fscheck(job.persistent_child(&file.name, false))?
                    .ok_or(JournalError::Interrupted)?;
                for blob in fscheck(nested.persistent_entries(core.limits.job_entries))? {
                    if blob.directory {
                        return Err(JournalError::Corrupt);
                    }
                    usage.add(blob.bytes, &core.limits)?;
                    job_usage.add(blob.bytes, &core.limits)?;
                }
                fscheck(nested.revalidate())?;
            } else if let Some(number) = file
                .name
                .strip_prefix("rev-")
                .and_then(|s| s.strip_suffix(".json"))
            {
                let revision = number.parse::<u64>().map_err(|_| JournalError::Corrupt)?;
                if revision_name(revision) != file.name {
                    return Err(JournalError::Corrupt);
                }
                job_usage.revisions = job_usage
                    .revisions
                    .checked_add(1)
                    .ok_or(JournalError::LimitExceeded)?;
                if job_usage.revisions > core.limits.revisions {
                    return Err(JournalError::LimitExceeded);
                }
            } else if !matches!(
                file.name.as_str(),
                "reservation.json"
                    | "intent.json"
                    | "bindings.json"
                    | "current.json"
                    | "accepted.json"
                    | "rejected.json"
            ) {
                // This includes current-next.json: no outstanding/ambiguous
                // publication may be treated as a fresh capacity reserve.
                return Err(JournalError::Interrupted);
            }
        }
        fscheck(job.revalidate())?;
        if usage.jobs.insert(id, job_usage).is_some() {
            return Err(JournalError::Conflict);
        }
    }
    reader.check_shared()?;
    Ok(usage)
}

fn check_selected_pair(outbox: &BrokerCheckpointV4) -> JournalResult<()> {
    let mut coverage = 0_u32;
    let mut result = 0_u32;
    let mut pending = 0_u32;
    for entry in outbox.events() {
        match entry.event().kind.as_str() {
            "observer.coverage" => coverage += 1,
            "broker.batch.result" => result += 1,
            _ if entry.audit_seq().is_none() => return Err(JournalError::InvalidInput),
            _ => {}
        }
        pending += u32::from(entry.audit_seq().is_none());
    }
    if coverage != 1 || result != 1 || !(1..=2).contains(&pending) {
        return Err(JournalError::InvalidInput);
    }
    Ok(())
}

fn add(a: u64, b: u64) -> JournalResult<u64> {
    a.checked_add(b).ok_or(JournalError::LimitExceeded)
}

fn multiply(a: u64, b: u64) -> JournalResult<u64> {
    a.checked_mul(b).ok_or(JournalError::LimitExceeded)
}

// The serializer writes only to a bounded byte counter. Object key ordering
// changes no encoded length; serde_json's escaping/number encoding is the
// same as canonical(), without materializing another 32 MiB checkpoint.
struct CountSink<'a> {
    bytes: &'a Cell<u64>,
    exceeded: &'a Cell<bool>,
    maximum: u64,
}

impl Write for CountSink<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next = u64::try_from(bytes.len())
            .ok()
            .and_then(|n| self.bytes.get().checked_add(n));
        match next {
            Some(next) if next <= self.maximum => {
                self.bytes.set(next);
                Ok(bytes.len())
            }
            _ => {
                self.exceeded.set(true);
                Err(std::io::Error::other("private metadata sizing limit"))
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn count_bytes(value: &impl Serialize, maximum: u64) -> JournalResult<u64> {
    let bytes = Cell::new(0);
    let exceeded = Cell::new(false);
    serialize_counted(value, &bytes, &exceeded, maximum)?;
    Ok(bytes.get())
}

fn serialize_counted(
    value: &impl Serialize,
    bytes: &Cell<u64>,
    exceeded: &Cell<bool>,
    maximum: u64,
) -> JournalResult<()> {
    serde_json::to_writer(
        CountSink {
            bytes,
            exceeded,
            maximum,
        },
        value,
    )
    .map_err(|_| {
        if exceeded.get() {
            JournalError::TooLarge
        } else {
            JournalError::InvalidInput
        }
    })
}

struct CountSection<'a, T: ?Sized> {
    value: &'a T,
    total: &'a Cell<u64>,
    section: &'a Cell<u64>,
}

impl<T: Serialize + ?Sized> Serialize for CountSection<'_, T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let before = self.total.get();
        let value = self.value.serialize(serializer)?;
        self.section.set(
            self.total
                .get()
                .checked_sub(before)
                .ok_or_else(|| S::Error::custom("invalid private sizing section"))?,
        );
        Ok(value)
    }
}

#[derive(Serialize)]
struct EmptySupervisionBound {
    version: u32,
    control_namespace_uuid: Uuid,
    helpers: [(); 0],
    operations: [(); 0],
}

#[derive(Serialize)]
#[serde(untagged)]
enum BrokerBound<'a> {
    Existing(&'a BrokerCheckpointV2),
    Legacy {
        version: u32,
        runner: &'a CheckpointPayloadV1,
        supervision: EmptySupervisionBound,
    },
}

#[derive(Serialize)]
struct LifecycleBound<'a> {
    version: u32,
    broker: BrokerBound<'a>,
    commands: CountSection<'a, [LocalCommandV1]>,
}

struct EntriesBound<'a> {
    entries: &'a [OutboxEventV1],
    selected: bool,
}

impl Serialize for EntriesBound<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct EntryBound<'a> {
            version: u32,
            event: &'a crate::model::Event,
            input_hash: &'a str,
            audit_seq: Option<i64>,
        }
        let mut seq = serializer.serialize_seq(Some(self.entries.len()))?;
        for entry in self.entries {
            seq.serialize_element(&EntryBound {
                version: 1,
                event: entry.event(),
                input_hash: entry.input_hash(),
                audit_seq: if self.selected && entry.audit_seq().is_none() {
                    Some(i64::MAX)
                } else {
                    entry.audit_seq()
                },
            })?;
        }
        seq.end()
    }
}

#[derive(Serialize)]
struct OutboxBound<'a> {
    version: u32,
    lifecycle: LifecycleBound<'a>,
    events: CountSection<'a, EntriesBound<'a>>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum PayloadBound<'a> {
    Lifecycle(LifecycleBound<'a>),
    Outbox(OutboxBound<'a>),
}

#[derive(Serialize)]
struct EnvelopeBound<'a> {
    version: u32,
    operation_id: Uuid,
    intent_hash: &'a str,
    revision: u64,
    previous: Option<RevisionRefV1>,
    payload: PayloadBound<'a>,
}

fn estimate_job(
    operation_id: Uuid,
    head: &RevisionRefV1,
    metadata: &StopMetadata,
    selected: bool,
) -> JournalResult<JobBound> {
    if operation_id.is_nil() || metadata.intent.operation_id() != operation_id {
        return Err(JournalError::InvalidInput);
    }
    valid_hash(head.hash())?;
    let intent_hash = metadata
        .intent
        .intent_hash()
        .map_err(|_| JournalError::Corrupt)?;
    valid_hash(&intent_hash)?;
    let commands = metadata
        .checkpoint
        .lifecycle()
        .map_or(&[][..], |cp| cp.commands());
    let remaining_cancels = COMMAND_LIMIT
        .checked_sub(u64::try_from(commands.len()).map_err(|_| JournalError::LimitExceeded)?)
        .ok_or(JournalError::LimitExceeded)?;
    let outbox = metadata.checkpoint.outbox();
    if selected {
        check_selected_pair(outbox.ok_or(JournalError::UnsupportedVersion)?)?;
    }

    let total = Cell::new(0);
    let exceeded = Cell::new(false);
    let commands_bytes = Cell::new(0);
    let outbox_bytes = Cell::new(0);
    let lifecycle = LifecycleBound {
        version: 3,
        broker: metadata.checkpoint.supervised().map_or_else(
            || BrokerBound::Legacy {
                version: 2,
                runner: metadata.checkpoint.runner(),
                supervision: EmptySupervisionBound {
                    version: 1,
                    // Width-only UUID in this nonpersistent sizing view. The
                    // actual Cancel uses the retained scope's namespace instead.
                    control_namespace_uuid: operation_id,
                    helpers: [],
                    operations: [],
                },
            },
            BrokerBound::Existing,
        ),
        commands: CountSection {
            value: commands,
            total: &total,
            section: &commands_bytes,
        },
    };
    let entries = EntriesBound {
        entries: outbox.map_or(&[][..], |cp| cp.events()),
        selected,
    };
    let payload = if outbox.is_some() {
        PayloadBound::Outbox(OutboxBound {
            version: 4,
            lifecycle,
            events: CountSection {
                value: &entries,
                total: &total,
                section: &outbox_bytes,
            },
        })
    } else {
        PayloadBound::Lifecycle(lifecycle)
    };
    let maximal_head = RevisionRefV1::new(u64::MAX, head.hash())?;
    let envelope = EnvelopeBound {
        version: if outbox.is_some() { 4 } else { 3 },
        operation_id,
        intent_hash: &intent_hash,
        revision: u64::MAX,
        previous: Some(maximal_head.clone()),
        payload,
    };
    serialize_counted(&envelope, &total, &exceeded, CHECKPOINT)?;
    let checkpoint_bytes = total.get();
    let pointer_bytes = count_bytes(
        &Current {
            version: 1,
            operation_id,
            intent_hash: intent_hash.clone(),
            head: maximal_head,
        },
        SMALL,
    )?;
    let cancel = LocalCommandV1::new(
        operation_id,
        &intent_hash,
        operation_id,
        LocalCommandRequestV1::CancelRequested,
    )?;
    // A separator also covers the empty-array case conservatively.
    let cancel_delta_bytes = add(count_bytes(&cancel, COMMANDS_LIMIT)?, 1)?;
    let commands_bytes = commands_bytes.get();
    let outbox_bytes = outbox.map(|_| outbox_bytes.get());
    if commands_bytes > COMMANDS_LIMIT || outbox_bytes.is_some_and(|n| n > OUTBOX_LIMIT) {
        return Err(JournalError::TooLarge);
    }
    let triangular = multiply(remaining_cancels, add(remaining_cancels, 1)?)? / 2;
    let cancel_bytes = add(
        multiply(remaining_cancels, add(checkpoint_bytes, pointer_bytes)?)?,
        multiply(cancel_delta_bytes, triangular)?,
    )?;
    let completion_bytes = if selected {
        add(
            add(
                checkpoint_bytes,
                multiply(remaining_cancels, cancel_delta_bytes)?,
            )?,
            pointer_bytes,
        )?
    } else {
        0
    };
    Ok(JobBound {
        operation_id,
        head: head.clone(),
        remaining_cancels,
        checkpoint_bytes,
        pointer_bytes,
        cancel_delta_bytes,
        commands_bytes,
        outbox_bytes,
        cancel_bytes,
        completion_bytes,
    })
}

fn check_capacity(
    limits: &JournalLimitsV1,
    physical: &PhysicalUsage,
    bounds: &[JobBound],
) -> JournalResult<()> {
    if physical.jobs.len() > limits.jobs as usize || bounds.len() > physical.jobs.len() {
        return Err(JournalError::LimitExceeded);
    }
    for (id, usage) in &physical.jobs {
        if id.is_nil() {
            return Err(JournalError::Corrupt);
        }
        if usage.bytes > limits.job_bytes
            || usage.entries > limits.job_entries
            || usage.revisions > limits.revisions
        {
            return Err(JournalError::LimitExceeded);
        }
    }
    let mut seen = HashSet::new();
    let mut scope_bytes = physical.bytes;
    let mut scope_entries = u64::from(physical.entries);
    let mut completions = 0_u32;
    for bound in bounds {
        if !seen.insert(bound.operation_id)
            || physical.verified_rejections.contains(&bound.operation_id)
        {
            return Err(JournalError::Conflict);
        }
        let usage = physical
            .jobs
            .get(&bound.operation_id)
            .ok_or(JournalError::Conflict)?;
        if bound.remaining_cancels > COMMAND_LIMIT {
            return Err(JournalError::LimitExceeded);
        }
        if bound.checkpoint_bytes == 0 || bound.pointer_bytes == 0 || bound.cancel_delta_bytes == 0
        {
            return Err(JournalError::Corrupt);
        }
        if u64::from(usage.revisions) != add(bound.head.revision, 1)? {
            return Err(JournalError::Conflict);
        }
        let growth = multiply(bound.remaining_cancels, bound.cancel_delta_bytes)?;
        if add(bound.commands_bytes, growth)? > COMMANDS_LIMIT
            || add(bound.checkpoint_bytes, growth)? > CHECKPOINT
            || bound.outbox_bytes.is_some_and(|n| n > OUTBOX_LIMIT)
        {
            return Err(JournalError::TooLarge);
        }
        let selected = u64::from(bound.completion_bytes != 0);
        completions += u32::from(selected != 0);
        let triangular = multiply(bound.remaining_cancels, add(bound.remaining_cancels, 1)?)? / 2;
        let expected_cancel_bytes = add(
            multiply(
                bound.remaining_cancels,
                add(bound.checkpoint_bytes, bound.pointer_bytes)?,
            )?,
            multiply(bound.cancel_delta_bytes, triangular)?,
        )?;
        let expected_completion_bytes = if selected != 0 {
            add(add(bound.checkpoint_bytes, growth)?, bound.pointer_bytes)?
        } else {
            0
        };
        if bound.cancel_bytes != expected_cancel_bytes
            || bound.completion_bytes != expected_completion_bytes
        {
            return Err(JournalError::Corrupt);
        }
        let entries = multiply(2, add(bound.remaining_cancels, selected)?)?;
        let bytes = add(bound.cancel_bytes, bound.completion_bytes)?;
        if add(usage.bytes, bytes)? > limits.job_bytes
            || add(u64::from(usage.entries), entries)? > u64::from(limits.job_entries)
            || add(add(bound.head.revision, bound.remaining_cancels)?, selected)?
                >= u64::from(limits.revisions)
            || add(
                u64::from(usage.revisions),
                add(bound.remaining_cancels, selected)?,
            )? > u64::from(limits.revisions)
        {
            return Err(JournalError::LimitExceeded);
        }
        scope_bytes = add(scope_bytes, bytes)?;
        scope_entries = add(scope_entries, entries)?;
    }
    if completions != 1
        || physical
            .jobs
            .keys()
            .any(|id| !seen.contains(id) && !physical.verified_rejections.contains(id))
        || physical
            .verified_rejections
            .iter()
            .any(|id| !physical.jobs.contains_key(id))
    {
        return Err(JournalError::Conflict);
    }
    if scope_bytes > limits.scope_bytes || scope_entries > u64::from(limits.scope_entries) {
        return Err(JournalError::LimitExceeded);
    }
    Ok(())
}

#[cfg(test)]
#[path = "cold_audit_tests.rs"]
mod tests;
