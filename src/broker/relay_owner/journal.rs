//! Append-only metadata for broker relay attachment. No message body or proof.
//! A recovered fact never creates a fresh exec capability.
use super::*;
use crate::{
    control_fs::{Directory, PrivateFile},
    relay::admission::{
        RelayChannelAdmissionV1, RelayChannelClaimV1, RelayChannelSnapshotV1,
        RelayChannelTransitionV1,
    },
};
use std::collections::BTreeMap;
use uuid::Uuid;

mod budget;
mod history;
#[cfg(test)]
mod history_tests;
#[cfg(test)]
mod v2_tests;
// ChannelResolutionV2 is consumed by the closed replay tests; keep the re-export.
#[allow(unused_imports)]
pub(crate) use history::{
    ChannelChangeV2, ChannelHistoryV2, ChannelOperationPurposeV2, ChannelResolutionV2,
    ChannelTerminalV2,
};

const LEGACY_MAX: usize = 4096;
const MAX_FACTS: usize = 12288;
const MAX_RECORD: usize = 16_384;
const ORDINARY: usize = 16;
const TERMINAL: usize = 32;
const PER_JOB: usize = ORDINARY + TERMINAL;
const CHILD: &str = "relay_channels_v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AttachmentPhase {
    AwaitingIssuedWorkload,
    Admitted,
    Unknown,
    Closed,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelFact {
    pub version: u32,
    pub job_id: Uuid,
    pub source_profile_id: String,
    pub endpoint: RelayEndpointV1,
    pub source_head_revision: u64,
    pub source_head_hash: String,
    pub arm_id: String,
    pub original_clock: crate::control::HostClockSampleV1,
    pub original_deadline_ns_decimal: String,
    pub phase: AttachmentPhase,
    pub observed_at: String,
    pub error: Option<RelayOwnerError>,
    pub admission: Option<RelayChannelAdmissionV1>,
    pub claim: Option<RelayChannelClaimV1>,
    pub core: Option<RelayChannelSnapshotV1>,
    pub transition: Option<RelayChannelTransitionV1>,
    pub local_joined: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    sequence: u64,
    previous_sha256: Option<String>,
    fact: ChannelFact,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordV2 {
    version: u32,
    sequence: u64,
    previous_sha256: Option<String>,
    job_id: Uuid,
    observed_at: String,
    change: ChannelChangeV2,
}
pub(crate) struct ChannelJournal {
    directory: Directory,
    scope: PrivateFile,
    authority: JournalScopeV1,
    records: Vec<PrivateFile>,
    jobs: BTreeMap<Uuid, history::State>,
    poisoned: bool,
}
/// The durable assignment facts exposed to recovery. This deliberately omits
/// worker endpoints, channel IDs, claims, credentials, and runtime state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RetainedChannelEvidence {
    pub job_id: Uuid,
    pub source_profile_id: String,
    pub source_head_revision: u64,
    pub source_head_hash: String,
    pub arm_id: String,
    pub phase: AttachmentPhase,
}
impl ChannelJournal {
    /// Called only while the enclosing tenant Outbox flock and RoleLease are
    /// retained. Both namespaces are serialized by the same tenant IO owner.
    pub(crate) fn open(
        parent: &Directory,
        scope: &JournalScopeV1,
        secrets: &[String],
    ) -> Result<Self> {
        Self::open_mode(parent, scope, secrets, true)
    }

    /// Read an already retained channel journal while the caller still holds
    /// the tenant Outbox flock and RoleLease. This path never creates or repairs
    /// a namespace; absent or interrupted state fails closed.
    pub(crate) fn open_retained_read(
        parent: &Directory,
        scope: &JournalScopeV1,
        secrets: &[String],
    ) -> Result<Option<Self>> {
        protect_name(CHILD, secrets)?;
        protect_name("scope.json", secrets)?;
        let bytes = super::super::journal::canonical(scope)
            .map_err(|_| RelayOwnerError::InvalidMaterial)?;
        protect(&bytes, secrets)?;
        if parent
            .persistent_child(CHILD, false)
            .map_err(|_| RelayOwnerError::InvalidMaterial)?
            .is_none()
        {
            return Ok(None);
        }
        Self::open_mode(parent, scope, secrets, false).map(Some)
    }

    /// Recover only the durable assignment tuple needed to compare against a
    /// separately verified STOP snapshot. It is not endpoint or runtime proof.
    pub(crate) fn retained_evidence(
        parent: &Directory,
        scope: &JournalScopeV1,
        job: Uuid,
        secrets: &[String],
    ) -> Result<Option<RetainedChannelEvidence>> {
        let Some(journal) = Self::open_retained_read(parent, scope, secrets)? else {
            return Ok(None);
        };
        let fact = journal.get(job)?;
        Ok(fact.map(|fact| RetainedChannelEvidence {
            job_id: fact.job_id,
            source_profile_id: fact.source_profile_id,
            source_head_revision: fact.source_head_revision,
            source_head_hash: fact.source_head_hash,
            arm_id: fact.arm_id,
            phase: fact.phase,
        }))
    }

    fn open_mode(
        parent: &Directory,
        scope: &JournalScopeV1,
        secrets: &[String],
        allow_create: bool,
    ) -> Result<Self> {
        protect_name(CHILD, secrets)?;
        protect_name("scope.json", secrets)?;
        let bytes = super::super::journal::canonical(scope)
            .map_err(|_| RelayOwnerError::InvalidMaterial)?;
        protect(&bytes, secrets)?;
        let existing = parent
            .persistent_child(CHILD, false)
            .map_err(|_| RelayOwnerError::InvalidMaterial)?;
        let (directory, fresh) = match existing {
            Some(directory) => (directory, false),
            None if allow_create => (
                parent
                    .exclusive_child(CHILD)
                    .map_err(|_| RelayOwnerError::InvalidMaterial)?,
                true,
            ),
            None => return Err(RelayOwnerError::InvalidMaterial),
        };
        let scope_file = match directory
            .persistent_read("scope.json", MAX_RECORD as u64, &[])
            .map_err(|_| RelayOwnerError::InvalidMaterial)?
        {
            Some(file) => file,
            None => {
                // Only an exclusively new empty directory is eligible. A prior
                // interrupted namespace is never repaired or silently adopted.
                if !fresh
                    || !directory
                        .persistent_entries(1)
                        .map_err(|_| RelayOwnerError::InvalidMaterial)?
                        .is_empty()
                {
                    return Err(RelayOwnerError::InvalidMaterial);
                }
                // Existing empty children are also refused: caller must not
                // interpret an interrupted creation as fresh authority.
                let file = write(&directory, "scope.json", &bytes, secrets)?;
                directory
                    .durable_bootstrap()
                    .map_err(|_| RelayOwnerError::InvalidMaterial)?;
                file
            }
        };
        if scope_file.bytes() != bytes {
            return Err(RelayOwnerError::Scope);
        }
        let mut journal = Self {
            directory,
            scope: scope_file,
            authority: scope.clone(),
            records: Vec::new(),
            jobs: BTreeMap::new(),
            poisoned: false,
        };
        let mut entries = journal
            .directory
            .persistent_entries((MAX_FACTS + 1) as u32)
            .map_err(|_| RelayOwnerError::InvalidMaterial)?;
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        let mut legacy = BTreeMap::<Uuid, (ChannelFact, usize)>::new();
        let mut legacy_count = 0_usize;
        for entry in entries {
            protect_name(&entry.name, secrets)?;
            if entry.name == "scope.json" && !entry.directory {
                continue;
            }
            let sequence = journal.records.len() as u64 + 1;
            if entry.directory || entry.name != name(sequence) || entry.bytes > MAX_RECORD as u64 {
                return Err(RelayOwnerError::InvalidMaterial);
            }
            let file = journal
                .directory
                .persistent_read(&entry.name, MAX_RECORD as u64, &[])
                .map_err(|_| RelayOwnerError::InvalidMaterial)?
                .ok_or(RelayOwnerError::InvalidMaterial)?;
            protect(file.bytes(), secrets)?;
            let value = crate::json::decode_unique(file.bytes(), 64)
                .map_err(|_| RelayOwnerError::InvalidMaterial)?;
            match value.get("version").and_then(serde_json::Value::as_u64) {
                Some(1) => {
                    let record: Record = serde_json::from_value(value)
                        .map_err(|_| RelayOwnerError::InvalidMaterial)?;
                    if record.sequence != sequence
                        || record.previous_sha256.as_deref()
                            != journal.records.last().map(PrivateFile::digest)
                    {
                        return Err(RelayOwnerError::InvalidMaterial);
                    }
                    let job = record.fact.job_id;
                    let count = legacy.get(&job).map_or(0, |(_, count)| *count);
                    if count >= ORDINARY || legacy_count >= LEGACY_MAX {
                        return Err(RelayOwnerError::Capacity);
                    }
                    if let Some((previous, _)) = legacy.get(&job) {
                        validate_next(previous, &record.fact)?;
                    }
                    let first = !journal.jobs.contains_key(&job);
                    let state = journal
                        .jobs
                        .entry(job)
                        .or_insert_with(|| history::State::initial(record.fact.clone()));
                    state.legacy_fact(&record.fact, &journal.authority, first)?;
                    legacy.insert(job, (record.fact, count + 1));
                    legacy_count += 1;
                    let reserved: usize = legacy
                        .values()
                        .map(|(fact, count)| {
                            if fact.phase == AttachmentPhase::Closed {
                                *count
                            } else {
                                ORDINARY
                            }
                        })
                        .sum();
                    if reserved > LEGACY_MAX {
                        return Err(RelayOwnerError::Capacity);
                    }
                }
                Some(2) => {
                    let record: RecordV2 = serde_json::from_value(value)
                        .map_err(|_| RelayOwnerError::InvalidMaterial)?;
                    if record.sequence != sequence
                        || record.previous_sha256.as_deref()
                            != journal.records.last().map(PrivateFile::digest)
                    {
                        return Err(RelayOwnerError::InvalidMaterial);
                    }
                    let first = !journal.jobs.contains_key(&record.job_id);
                    let mut state = journal.state_for(record.job_id, &record.change)?;
                    if state
                        .apply(
                            &record.change,
                            &journal.authority,
                            &record.observed_at,
                            first,
                        )?
                        .is_none()
                    {
                        return Err(RelayOwnerError::InvalidMaterial);
                    }
                    if let ChannelChangeV2::RetirementIntent { request } = &record.change {
                        budget::before_cleanup(&state.history, record.job_id, request)?;
                    }
                    journal.jobs.insert(record.job_id, state);
                }
                _ => return Err(RelayOwnerError::InvalidMaterial),
            }
            if journal.reserved() > MAX_FACTS {
                return Err(RelayOwnerError::Capacity);
            }
            journal.validate_uuids(None)?;
            journal.records.push(file);
        }
        journal.revalidate()?;
        Ok(journal)
    }
    pub(crate) fn get(&self, job: Uuid) -> Result<Option<ChannelFact>> {
        self.revalidate()?;
        Ok(self.jobs.get(&job).map(|state| state.history.fact.clone()))
    }
    pub(crate) fn append(&mut self, fact: ChannelFact, secrets: &[String]) -> Result<ChannelFact> {
        self.append_v2(fact.job_id, ChannelChangeV2::Fact { fact }, secrets)
            .map(|history| history.fact)
    }
    pub(crate) fn history(&self, job: Uuid) -> Result<Option<ChannelHistoryV2>> {
        self.revalidate()?;
        Ok(self.jobs.get(&job).map(|state| state.history.clone()))
    }
    fn state_for(&self, job: Uuid, change: &ChannelChangeV2) -> Result<history::State> {
        if let Some(state) = self.jobs.get(&job) {
            return Ok(state.clone());
        }
        match change {
            ChannelChangeV2::Fact { fact } if fact.job_id == job => {
                Ok(history::State::initial(fact.clone()))
            }
            _ => Err(RelayOwnerError::Scope),
        }
    }
    fn reserved(&self) -> usize {
        self.jobs
            .values()
            .map(|state| {
                if state.history.fact.phase == AttachmentPhase::Closed {
                    state.count()
                } else {
                    PER_JOB
                }
            })
            .sum()
    }
    fn validate_uuids(&self, candidate: Option<(Uuid, &history::State)>) -> Result<()> {
        let mut ids = BTreeMap::new();
        for (job, current) in &self.jobs {
            let state = candidate
                .filter(|(id, _)| id == job)
                .map_or(current, |(_, state)| state);
            Self::index_uuids(&mut ids, *job, state)?;
        }
        if let Some((job, state)) = candidate
            && !self.jobs.contains_key(&job)
        {
            Self::index_uuids(&mut ids, job, state)?;
        }
        Ok(())
    }
    fn index_uuids(
        ids: &mut BTreeMap<String, Uuid>,
        job: Uuid,
        state: &history::State,
    ) -> Result<()> {
        let admission = state
            .history
            .fact
            .admission
            .as_ref()
            .map(|admission| admission.operation_id.as_str());
        for id in admission.into_iter().chain(
            state
                .history
                .operations
                .iter()
                .map(|op| op.request.operation_id()),
        ) {
            if ids.insert(id.into(), job).is_some() {
                return Err(RelayOwnerError::Scope);
            }
        }
        Ok(())
    }
    pub(crate) fn append_v2(
        &mut self,
        job: Uuid,
        change: ChannelChangeV2,
        secrets: &[String],
    ) -> Result<ChannelHistoryV2> {
        self.revalidate()?;
        if job.is_nil() {
            return Err(RelayOwnerError::Scope);
        }
        match &change {
            ChannelChangeV2::Fact { fact }
            | ChannelChangeV2::BeginRetirement { fact, .. }
            | ChannelChangeV2::Finalize { fact }
                if fact.job_id != job =>
            {
                return Err(RelayOwnerError::Scope);
            }
            _ => {}
        }
        let first = !self.jobs.contains_key(&job);
        if first && self.reserved().saturating_add(PER_JOB) > MAX_FACTS {
            return Err(RelayOwnerError::Capacity);
        }
        let observed_at = chrono::Utc::now().to_rfc3339();
        let mut state = self.state_for(job, &change)?;
        if state
            .apply(&change, &self.authority, &observed_at, first)?
            .is_none()
        {
            return self
                .jobs
                .get(&job)
                .map(|state| state.history.clone())
                .ok_or(RelayOwnerError::Scope);
        }
        if self.records.len() >= MAX_FACTS {
            return Err(RelayOwnerError::Capacity);
        }
        self.validate_uuids(Some((job, &state)))?;
        if let ChannelChangeV2::RetirementIntent { request } = &change {
            budget::before_cleanup(&state.history, job, request)?;
        }
        let sequence = self.records.len() as u64 + 1;
        let bytes = super::super::journal::canonical(&RecordV2 {
            version: 2,
            sequence,
            previous_sha256: self.records.last().map(|file| file.digest().into()),
            job_id: job,
            observed_at,
            change,
        })
        .map_err(|_| RelayOwnerError::InvalidMaterial)?;
        if bytes.len() > MAX_RECORD {
            return Err(RelayOwnerError::Capacity);
        }
        protect_name(&name(sequence), secrets)?;
        protect(&bytes, secrets)?;
        self.poisoned = true;
        let file = write(&self.directory, &name(sequence), &bytes, secrets)?;
        self.records.push(file);
        self.jobs.insert(job, state.clone());
        self.poisoned = false;
        Ok(state.history)
    }
    fn revalidate(&self) -> Result<()> {
        if self.poisoned {
            return Err(RelayOwnerError::InvalidMaterial);
        }
        self.scope
            .persistent_revalidate()
            .map_err(|_| RelayOwnerError::InvalidMaterial)?;
        for file in &self.records {
            file.persistent_revalidate()
                .map_err(|_| RelayOwnerError::InvalidMaterial)?;
        }
        if self
            .directory
            .persistent_entries((MAX_FACTS + 1) as u32)
            .map_err(|_| RelayOwnerError::InvalidMaterial)?
            .len()
            != self.records.len() + 1
        {
            return Err(RelayOwnerError::InvalidMaterial);
        }
        Ok(())
    }
}
fn validate(fact: &ChannelFact) -> Result<()> {
    if fact.version != 1
        || fact.job_id.is_nil()
        || fact.source_profile_id.is_empty()
        || fact.source_profile_id.len() > 80
        || !fact
            .source_profile_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        || fact.endpoint.validate().is_err()
        || fact.source_head_revision > i64::MAX as u64
        || crate::control::hash(&fact.source_head_hash).is_err()
        || crate::control::uuid(&fact.arm_id).is_err()
        || fact.observed_at.len() > 64
        || chrono::DateTime::parse_from_rfc3339(&fact.observed_at).is_err()
        || fact.original_clock.continuous_ns().is_err()
        || fact.original_deadline_ns_decimal.parse::<u128>().is_err()
    {
        return Err(RelayOwnerError::InvalidMaterial);
    }
    if let Some(admission) = &fact.admission {
        admission
            .validate()
            .map_err(|_| RelayOwnerError::InvalidMaterial)?;
        if admission.endpoint != fact.endpoint {
            return Err(RelayOwnerError::Scope);
        }
    }
    if let Some(claim) = &fact.claim {
        claim
            .validate()
            .map_err(|_| RelayOwnerError::InvalidMaterial)?;
        if fact.admission.as_ref().is_none_or(|admission| {
            admission.channel_id != claim.channel_id
                || admission.epoch != claim.epoch
                || admission.input_hash().ok().as_ref() != Some(&claim.admission_input_hash)
        }) {
            return Err(RelayOwnerError::Scope);
        }
    }
    if let Some(core) = &fact.core {
        let admission = fact.admission.as_ref().ok_or(RelayOwnerError::Scope)?;
        if core.version != 1
            || core.revision == 0
            || core.revision >= i64::MAX as u64
            || core.channel_id != admission.channel_id
            || core.epoch != admission.epoch
            || core.control_namespace_uuid != admission.control_namespace_uuid
            || core.endpoint != admission.endpoint
            || core.credential_id != admission.credential_id
            || core.binding_revision != admission.binding_revision
            || core.session_id != admission.session_id
            || core.expires_at != admission.expires_at
            || core.admission_operation_id != admission.operation_id
            || admission.input_hash().ok().as_ref() != Some(&core.admission_input_hash)
        {
            return Err(RelayOwnerError::Scope);
        }
    }
    if let Some(transition) = &fact.transition {
        transition
            .validate()
            .map_err(|_| RelayOwnerError::InvalidMaterial)?;
        let admission = fact.admission.as_ref().ok_or(RelayOwnerError::Scope)?;
        if transition.channel_id != admission.channel_id
            || transition.epoch != admission.epoch
            || admission.input_hash().ok().as_ref() != Some(&transition.admission_input_hash)
        {
            return Err(RelayOwnerError::Scope);
        }
    }
    if fact.phase == AttachmentPhase::Closed
        && (!fact.local_joined
            || fact.error.is_some()
            || fact.admission.is_some()
                && fact.core.as_ref().is_none_or(|core| {
                    !matches!(
                        core.state,
                        crate::relay::admission::RelayChannelStateV1::Closed
                            | crate::relay::admission::RelayChannelStateV1::CancelledBeforeClaim
                            | crate::relay::admission::RelayChannelStateV1::KnownRefusedBeforeSpawn
                    )
                }))
    {
        return Err(RelayOwnerError::Scope);
    }
    Ok(())
}
fn validate_next(previous: &ChannelFact, fact: &ChannelFact) -> Result<()> {
    if previous.endpoint != fact.endpoint
        || previous.arm_id != fact.arm_id
        || previous.source_profile_id != fact.source_profile_id
        || previous.source_head_revision != fact.source_head_revision
        || previous.source_head_hash != fact.source_head_hash
        || super::super::journal::canonical(&previous.original_clock).ok()
            != super::super::journal::canonical(&fact.original_clock).ok()
        || previous.original_deadline_ns_decimal != fact.original_deadline_ns_decimal
        || previous.phase == AttachmentPhase::Closed
        || previous
            .admission
            .as_ref()
            .is_some_and(|value| fact.admission.as_ref() != Some(value))
        || previous
            .claim
            .as_ref()
            .is_some_and(|value| fact.claim.as_ref() != Some(value))
        || previous.core.as_ref().is_some_and(|value| {
            fact.core.as_ref().is_none_or(|current| {
                current.revision < value.revision
                    || current.revision == value.revision && current != value
            })
        })
    {
        return Err(RelayOwnerError::Scope);
    }
    Ok(())
}
fn validate_scope(fact: &ChannelFact, scope: &JournalScopeV1) -> Result<()> {
    if fact.admission.as_ref().is_some_and(|input| {
        input.control_namespace_uuid != scope.control_namespace_uuid().to_string()
    }) || fact.core.as_ref().is_some_and(|core| {
        core.tenant_id != scope.tenant_id()
            || core.control_namespace_uuid != scope.control_namespace_uuid().to_string()
    }) {
        return Err(RelayOwnerError::Scope);
    }
    Ok(())
}
fn name(sequence: u64) -> String {
    format!("fact-{sequence:020}.json")
}
fn protect(bytes: &[u8], secrets: &[String]) -> Result<()> {
    crate::relay::channel::control::validate_private_record(bytes, secrets)
        .map_err(|_| RelayOwnerError::InvalidMaterial)
}
fn protect_name(name: &str, secrets: &[String]) -> Result<()> {
    for value in [name.to_owned(), format!("{CHILD}/{name}")] {
        let bytes = serde_json::to_vec(&value).map_err(|_| RelayOwnerError::InvalidMaterial)?;
        protect(&bytes, secrets)?;
    }
    Ok(())
}
fn write(
    directory: &Directory,
    name: &str,
    bytes: &[u8],
    secrets: &[String],
) -> Result<PrivateFile> {
    protect_name(name, secrets)?;
    protect(bytes, secrets)?;
    let file = directory
        .persistent_create(name, bytes)
        .map_err(|_| RelayOwnerError::InvalidMaterial)?;
    file.persistent_sync()
        .map_err(|_| RelayOwnerError::InvalidMaterial)?;
    directory
        .sync()
        .map_err(|_| RelayOwnerError::InvalidMaterial)?;
    Ok(file)
}
