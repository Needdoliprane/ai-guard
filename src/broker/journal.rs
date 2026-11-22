//! Durable private job data. This module never authorizes or starts a workload.
use super::{
    checkpoint::BrokerCheckpointV2,
    intent::ExecutionIntentV1,
    lifecycle::{BrokerCheckpointV3, LocalCommandReceiptV1},
    outbox::{BrokerCheckpointV4, OutboxReceiptV1},
};
use crate::runtime::launch_profile::{
    DockerLaunchProfileBindingV1, LaunchProfileSelection, RetainedDockerLaunchProfile,
};
use crate::{
    control_fs::{Directory, PrivateFile},
    runner::CheckpointPayloadV1,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::{Arc, Mutex, Weak},
};
use uuid::Uuid;
#[cfg(test)]
mod canonical_tests;
mod confirmed;
pub mod recovery;
pub(crate) mod retirement;
#[cfg(all(test, unix))]
pub(crate) mod retirement_live_fixture;
#[cfg(all(test, unix))]
mod retirement_live_tests;
#[cfg(all(test, unix))]
mod retirement_tests;
pub(crate) mod status_projection;
pub use recovery::{CancelStopProjectionV1, StopJobSnapshotV1, StopJournal};
mod types;
pub use types::*;
use types::{AcceptedData, BindingData, LoadedCheckpoint};

macro_rules! stage {
    ($stage:ident) => {{
        #[cfg(test)]
        {
            tests::checkpoint(FaultStage::$stage)?;
        }
    }};
}

const SMALL: u64 = 8 * 1024;
const CHECKPOINT: u64 = 32 * 1024 * 1024;

/// Complement to the scoped filesystem probe, never present in production.
/// Roots, operation IDs and client addresses select one private sample only.
#[cfg(test)]
pub(in crate::broker) mod cost_measurement {
    use super::Journal;
    use crate::control_fs::cost_probe::ProbeError;
    use serde::Serialize;
    use std::{
        path::{Path, PathBuf},
        sync::{
            Arc, RwLock,
            atomic::{AtomicU64, Ordering},
        },
        time::Instant,
    };
    use uuid::Uuid;

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(in crate::broker) enum WriterKind {
        Load,
        Status,
    }
    #[derive(Clone, Copy)]
    pub(in crate::broker) enum InventoryKind {
        Scope,
        Jobs,
        JobFiles,
        Bindings,
    }
    enum Counter {
        Attempted,
        Sent,
        Taken,
        ReplyReady,
        ActorErrors,
        Received,
        CallerErrors,
        CallerAbandoned,
        ActorAbandoned,
        TakeSamples,
        SubmitToTakeNs,
        ActorNs,
        CallerNs,
    }
    const WRITER_COUNTERS: usize = 13;
    #[derive(Default)]
    struct WriterCounters([AtomicU64; WRITER_COUNTERS]);
    impl WriterCounters {
        fn add(&self, counter: Counter, amount: u64) {
            add(&self.0[counter as usize], amount);
        }
        fn snapshot(&self) -> [u64; WRITER_COUNTERS] {
            std::array::from_fn(|index| self.0[index].load(Ordering::Relaxed))
        }
    }
    #[derive(Serialize)]
    pub(in crate::broker) struct Snapshot {
        usage_calls: u64,
        inventories: [u64; 4],
        entries_visited: [u64; 4],
        loads_started: u64,
        accepted_loads_completed: u64,
        verified_revisions: u64,
        verified_revision_bytes: u64,
        writer_load: [u64; WRITER_COUNTERS],
        writer_status: [u64; WRITER_COUNTERS],
        pub(in crate::broker) caller_outstanding: u64,
        pub(in crate::broker) actor_outstanding: u64,
        pub(in crate::broker) context_conflicts: u64,
        pub(in crate::broker) missed_hooks: u64,
    }
    #[derive(Serialize)]
    pub(in crate::broker) struct Coverage {
        caller: bool,
        actor: bool,
        usage: bool,
        inventory: bool,
        accepted_load: bool,
        revisions: bool,
    }
    impl Coverage {
        pub(in crate::broker) fn is_complete(&self) -> bool {
            self.caller
                && self.actor
                && self.usage
                && self.inventory
                && self.accepted_load
                && self.revisions
        }
    }
    impl Snapshot {
        pub(in crate::broker) fn writer_read_coverage(&self, kind: WriterKind) -> Coverage {
            let writer = match kind {
                WriterKind::Load => &self.writer_load,
                WriterKind::Status => &self.writer_status,
            };
            Coverage {
                caller: [Counter::Attempted, Counter::Sent, Counter::Received]
                    .into_iter()
                    .all(|counter| writer[counter as usize] > 0),
                actor: [Counter::Taken, Counter::ReplyReady]
                    .into_iter()
                    .all(|counter| writer[counter as usize] > 0),
                usage: self.usage_calls > 0,
                inventory: [
                    InventoryKind::Scope,
                    InventoryKind::Jobs,
                    InventoryKind::JobFiles,
                ]
                .into_iter()
                .all(|kind| {
                    self.inventories[kind as usize] > 0 && self.entries_visited[kind as usize] > 0
                }),
                accepted_load: self.loads_started > 0 && self.accepted_loads_completed > 0,
                revisions: self.verified_revisions > 0 && self.verified_revision_bytes > 0,
            }
        }
    }
    struct Active {
        token: u64,
        root: PathBuf,
        client: usize,
        operation: Uuid,
        kind: WriterKind,
        started: Instant,
        submit_at_ns: AtomicU64,
        usage_calls: AtomicU64,
        inventories: [AtomicU64; 4],
        entries_visited: [AtomicU64; 4],
        loads_started: AtomicU64,
        accepted_loads_completed: AtomicU64,
        verified_revisions: AtomicU64,
        verified_revision_bytes: AtomicU64,
        load: WriterCounters,
        status: WriterCounters,
        caller_outstanding: AtomicU64,
        actor_outstanding: AtomicU64,
        context_conflicts: AtomicU64,
    }
    static ACTIVE: RwLock<Option<Arc<Active>>> = RwLock::new(None);
    static NEXT: AtomicU64 = AtomicU64::new(1);
    static ACTIVE_TOKEN: AtomicU64 = AtomicU64::new(0);
    static MISSED_HOOKS: AtomicU64 = AtomicU64::new(0);
    fn add(counter: &AtomicU64, amount: u64) {
        let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            Some(value.saturating_add(amount))
        });
    }
    fn decrement(counter: &AtomicU64) {
        let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            Some(value.saturating_sub(1))
        });
    }
    fn elapsed(started: Instant) -> u64 {
        u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
    fn active() -> Option<Arc<Active>> {
        // Hooks never wait for the test's lifecycle lock or EXCLUSIVE gate.
        // A contended lifecycle selection is explicit missing coverage.
        match ACTIVE.try_read() {
            Ok(active) => active.as_ref().cloned(),
            Err(_) => {
                if ACTIVE_TOKEN.load(Ordering::Acquire) != 0 {
                    add(&MISSED_HOOKS, 1);
                }
                None
            }
        }
    }
    fn selected(root: &Path) -> Option<Arc<Active>> {
        active().filter(|value| root == value.root.as_path())
    }
    impl Active {
        fn writer(&self, kind: WriterKind) -> &WriterCounters {
            match kind {
                WriterKind::Load => &self.load,
                WriterKind::Status => &self.status,
            }
        }
        fn snapshot(&self) -> Snapshot {
            Snapshot {
                usage_calls: self.usage_calls.load(Ordering::Relaxed),
                inventories: std::array::from_fn(|i| self.inventories[i].load(Ordering::Relaxed)),
                entries_visited: std::array::from_fn(|i| {
                    self.entries_visited[i].load(Ordering::Relaxed)
                }),
                loads_started: self.loads_started.load(Ordering::Relaxed),
                accepted_loads_completed: self.accepted_loads_completed.load(Ordering::Relaxed),
                verified_revisions: self.verified_revisions.load(Ordering::Relaxed),
                verified_revision_bytes: self.verified_revision_bytes.load(Ordering::Relaxed),
                writer_load: self.load.snapshot(),
                writer_status: self.status.snapshot(),
                caller_outstanding: self.caller_outstanding.load(Ordering::Relaxed),
                actor_outstanding: self.actor_outstanding.load(Ordering::Relaxed),
                context_conflicts: self.context_conflicts.load(Ordering::Relaxed),
                missed_hooks: MISSED_HOOKS.load(Ordering::Relaxed),
            }
        }
    }
    pub(in crate::broker) struct Measurement {
        active: Arc<Active>,
        finished: bool,
    }
    impl Measurement {
        pub(in crate::broker) fn start(
            journal: &Journal,
            client: usize,
            operation: Uuid,
            kind: WriterKind,
        ) -> Result<Self, ProbeError> {
            // Derive the exact scope directory from the actual Journal.
            // The fixture's outer temp directory is not a journal selector.
            let root = journal.0.directory.path();
            if !root.is_absolute() {
                return Err(ProbeError::InvalidRoot);
            }
            let mut slot = ACTIVE.write().map_err(|_| ProbeError::Poisoned)?;
            if slot.is_some() {
                return Err(ProbeError::AlreadyActive);
            }
            let token = NEXT
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| ProbeError::TokenExhausted)?;
            let active = Arc::new(Active {
                token,
                root: root.to_owned(),
                client,
                operation,
                kind,
                started: Instant::now(),
                submit_at_ns: AtomicU64::new(0),
                usage_calls: AtomicU64::new(0),
                inventories: Default::default(),
                entries_visited: Default::default(),
                loads_started: AtomicU64::new(0),
                accepted_loads_completed: AtomicU64::new(0),
                verified_revisions: AtomicU64::new(0),
                verified_revision_bytes: AtomicU64::new(0),
                load: WriterCounters::default(),
                status: WriterCounters::default(),
                caller_outstanding: AtomicU64::new(0),
                actor_outstanding: AtomicU64::new(0),
                context_conflicts: AtomicU64::new(0),
            });
            MISSED_HOOKS.store(0, Ordering::Relaxed);
            *slot = Some(Arc::clone(&active));
            ACTIVE_TOKEN.store(token, Ordering::Release);
            Ok(Self {
                active,
                finished: false,
            })
        }
        pub(in crate::broker) fn finish(mut self) -> Result<Snapshot, ProbeError> {
            let mut slot = ACTIVE.write().map_err(|_| ProbeError::Poisoned)?;
            if slot.as_ref().map(|value| value.token) != Some(self.active.token) {
                return Err(ProbeError::ScopeLost);
            }
            ACTIVE_TOKEN.store(0, Ordering::Release);
            slot.take();
            self.finished = true;
            Ok(self.active.snapshot())
        }
    }
    impl Drop for Measurement {
        fn drop(&mut self) {
            if !self.finished
                && let Ok(mut slot) = ACTIVE.write()
                && slot.as_ref().map(|value| value.token) == Some(self.active.token)
            {
                ACTIVE_TOKEN.store(0, Ordering::Release);
                slot.take();
            }
        }
    }
    pub(super) fn usage(root: &Path) {
        if let Some(active) = selected(root) {
            add(&active.usage_calls, 1);
        }
    }
    pub(super) fn inventory(root: &Path, kind: InventoryKind, entry: bool) {
        if let Some(active) = selected(root) {
            if entry {
                add(&active.entries_visited[kind as usize], 1);
            } else {
                add(&active.inventories[kind as usize], 1);
            }
        }
    }
    pub(super) fn load(root: &Path, operation: Uuid, completed: bool) {
        if let Some(active) = selected(root) {
            if active.operation != operation {
                add(&active.context_conflicts, 1);
                return;
            }
            if completed {
                add(&active.accepted_loads_completed, 1);
            } else {
                add(&active.loads_started, 1);
            }
        }
    }
    pub(super) fn verified(root: &Path, bytes: usize) {
        if let Some(active) = selected(root) {
            add(&active.verified_revisions, 1);
            add(
                &active.verified_revision_bytes,
                u64::try_from(bytes).unwrap_or(u64::MAX),
            );
        }
    }
    pub(in crate::broker) struct Caller {
        active: Arc<Active>,
        kind: WriterKind,
        started: Instant,
        finished: bool,
    }
    pub(in crate::broker) fn caller(
        client: usize,
        kind: WriterKind,
        operation: Uuid,
    ) -> Option<Caller> {
        let active = active().filter(|value| value.client == client)?;
        if active.operation != operation
            || active.kind != kind
            || active
                .caller_outstanding
                .compare_exchange(0, 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        {
            add(&active.context_conflicts, 1);
            return None;
        }
        let started = Instant::now();
        active
            .submit_at_ns
            .store(elapsed(active.started).saturating_add(1), Ordering::Release);
        active.writer(kind).add(Counter::Attempted, 1);
        Some(Caller {
            active,
            kind,
            started,
            finished: false,
        })
    }
    impl Caller {
        pub(in crate::broker) fn sent(&self) {
            self.active.writer(self.kind).add(Counter::Sent, 1);
        }
        pub(in crate::broker) fn finish(mut self, received: bool, succeeded: bool) {
            let counters = self.active.writer(self.kind);
            counters.add(Counter::Received, u64::from(received));
            counters.add(Counter::CallerErrors, u64::from(!succeeded));
            counters.add(Counter::CallerNs, elapsed(self.started));
            decrement(&self.active.caller_outstanding);
            self.finished = true;
        }
    }
    impl Drop for Caller {
        fn drop(&mut self) {
            if !self.finished {
                let counters = self.active.writer(self.kind);
                counters.add(Counter::CallerAbandoned, 1);
                counters.add(Counter::CallerNs, elapsed(self.started));
                decrement(&self.active.caller_outstanding);
            }
        }
    }
    pub(in crate::broker) struct Actor {
        active: Arc<Active>,
        kind: WriterKind,
        started: Instant,
        finished: bool,
    }
    pub(in crate::broker) fn actor(
        journal: &Journal,
        kind: WriterKind,
        operation: Uuid,
    ) -> Option<Actor> {
        let active = selected(journal.0.directory.path())?;
        if active.operation != operation || active.kind != kind {
            add(&active.context_conflicts, 1);
            return None;
        }
        let counters = active.writer(kind);
        counters.add(Counter::Taken, 1);
        let submitted = active.submit_at_ns.load(Ordering::Acquire);
        if submitted != 0 {
            counters.add(Counter::TakeSamples, 1);
            counters.add(
                Counter::SubmitToTakeNs,
                elapsed(active.started).saturating_sub(submitted - 1),
            );
        } else {
            add(&active.context_conflicts, 1);
        }
        add(&active.actor_outstanding, 1);
        Some(Actor {
            active,
            kind,
            started: Instant::now(),
            finished: false,
        })
    }
    impl Actor {
        /// Readiness immediately before send; reception is counted by Caller.
        pub(in crate::broker) fn reply_ready(mut self, succeeded: bool) {
            let counters = self.active.writer(self.kind);
            counters.add(Counter::ReplyReady, 1);
            counters.add(Counter::ActorErrors, u64::from(!succeeded));
            counters.add(Counter::ActorNs, elapsed(self.started));
            decrement(&self.active.actor_outstanding);
            self.finished = true;
        }
    }
    impl Drop for Actor {
        fn drop(&mut self) {
            if !self.finished {
                let counters = self.active.writer(self.kind);
                counters.add(Counter::ActorAbandoned, 1);
                counters.add(Counter::ActorNs, elapsed(self.started));
                decrement(&self.active.actor_outstanding);
            }
        }
    }
}

pub(crate) fn canonical(value: &impl Serialize) -> JournalResult<Vec<u8>> {
    let mut value = serde_json::to_value(value).map_err(|_| JournalError::InvalidInput)?;
    // This preserves str/BTreeMap key ordering under either serde_json map
    // backend, without rebuilding every already ordered object in the tree.
    value.sort_all_objects();
    serde_json::to_vec(&value).map_err(|_| JournalError::InvalidInput)
}
pub(super) fn digest(domain: &str, bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(domain.as_bytes());
    hash.update([0]);
    hash.update(bytes);
    hex::encode(hash.finalize())
}
fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub(super) fn valid_hash(hash: &str) -> JournalResult<()> {
    if hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(JournalError::InvalidInput)
    }
}
fn fscheck<T>(result: anyhow::Result<T>) -> JournalResult<T> {
    result.map_err(|error| {
        if error.is::<crate::control_fs::persistent::PersistentLimit>() {
            JournalError::LimitExceeded
        } else {
            JournalError::UnsafeFilesystem
        }
    })
}
fn io<T>(result: anyhow::Result<T>) -> JournalResult<T> {
    result.map_err(|_| JournalError::Io)
}
fn revision_name(revision: u64) -> String {
    format!("rev-{revision:020}.json")
}

#[derive(Clone)]
pub struct Journal(Arc<Inner>);
struct Inner {
    scope: JournalScopeV1,
    scope_hash: String,
    directory: Directory,
    jobs: Directory,
    lock: PrivateFile,
    exclusions: FsExclusions,
    limits: JournalLimitsV1,
    control: ControlSecrets,
    writer: Mutex<Writer>,
    budget_incomplete: std::sync::atomic::AtomicBool,
    status_observation_failed: std::sync::atomic::AtomicBool,
}
fn note_status_read_failure<T>(core: &Inner, result: &JournalResult<T>) {
    if matches!(result, Err(error) if !matches!(error, JournalError::InvalidInput | JournalError::Busy))
    {
        core.status_observation_failed
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}
#[derive(Default)]
struct Writer {
    tickets: HashMap<Uuid, Weak<()>>,
    floors: HashMap<Uuid, RevisionRefV1>,
    rejected_floors: HashMap<Uuid, String>,
    observed_jobs: HashSet<Uuid>,
    cold_audit: Weak<recovery::cold_audit::ReservationState>,
}
pub(super) enum ExistingProfileV1 {
    Full(Journal),
    Stops(StopJournal),
}
pub(super) fn admit_recovery_report(
    report: &recovery::StopRecoveryReportV1,
    force_stops: bool,
) -> JournalResult<ExistingProfileV1> {
    recovery::admit_report(report, force_stops)
}
pub struct ReservationTicket {
    owner: Arc<Inner>,
    lease: Arc<()>,
    operation: Uuid,
    submit_hash: String,
}

macro_rules! record {
    ($name:ident { $($field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Serialize, Deserialize)] #[serde(deny_unknown_fields)]
        struct $name { $($field: $ty),* }
    };
}
record!(Reservation {
    version: u32,
    scope_hash: String,
    operation_id: Uuid,
    submit_hash: String
});
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bindings {
    version: u32,
    operation_id: Uuid,
    intent_hash: String,
    binding: BindingManifestV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    docker_launch_profile: Option<DockerLaunchProfileBindingV1>,
}
impl Bindings {
    fn hash(&self, bytes: &[u8]) -> JournalResult<String> {
        match (self.version, self.docker_launch_profile.is_some()) {
            (1, false) => Ok(digest("ai-guard:broker-bindings:v1", bytes)),
            (2, true) => Ok(digest("ai-guard:broker-bindings:v2", bytes)),
            _ => Err(JournalError::Corrupt),
        }
    }
    fn validate_launch(
        &self,
        accepted_version: u32,
        scope: &str,
        operation: Uuid,
        intent: &ExecutionIntentV1,
    ) -> JournalResult<()> {
        if self.version != accepted_version {
            return Err(JournalError::Corrupt);
        }
        if !matches!(
            (self.version, self.docker_launch_profile.is_some()),
            (1, false) | (2, true)
        ) {
            return Err(JournalError::Corrupt);
        }
        if let Some(profile) = &self.docker_launch_profile {
            profile.validate().map_err(|_| JournalError::Corrupt)?;
            let options = intent.initial_options().run_options();
            if profile.journal_scope_hash != scope
                || profile.parent_operation_id != operation
                || profile.intent_hash != self.intent_hash
                || options.runtime != crate::runtime::RuntimeKind::Docker
                || options.runtime_endpoint.as_deref() != Some(profile.runtime_endpoint.as_str())
            {
                return Err(JournalError::Corrupt);
            }
        }
        Ok(())
    }
}
record!(Checkpoint { version:u32, operation_id:Uuid, intent_hash:String, revision:u64, previous:Option<RevisionRefV1>, payload:CheckpointPayloadV1 });
record!(SupervisedCheckpoint { version:u32, operation_id:Uuid, intent_hash:String, revision:u64, previous:Option<RevisionRefV1>, payload:BrokerCheckpointV2 });
record!(LifecycleCheckpoint { version:u32, operation_id:Uuid, intent_hash:String, revision:u64, previous:Option<RevisionRefV1>, payload:BrokerCheckpointV3 });
record!(OutboxCheckpoint { version:u32, operation_id:Uuid, intent_hash:String, revision:u64, previous:Option<RevisionRefV1>, payload:BrokerCheckpointV4 });
record!(RetirementCheckpoint { version:u32, operation_id:Uuid, intent_hash:String, revision:u64, previous:Option<RevisionRefV1>, payload:super::retirement_checkpoint::BrokerCheckpointV5 });
record!(Rejected {
    version: u32,
    scope_hash: String,
    operation_id: Uuid,
    submit_hash: String,
    phase: RejectionPhaseV1,
    code: RejectionCodeV1
});
record!(Current {
    version: u32,
    operation_id: Uuid,
    intent_hash: String,
    head: RevisionRefV1
});
record!(Accepted {
    version: u32,
    scope_hash: String,
    operation_id: Uuid,
    submit_hash: String,
    intent_hash: String,
    bindings_hash: String,
    initial_checkpoint: RevisionRefV1
});

#[derive(Default)]
struct Usage {
    bytes: u64,
    entries: u32,
    jobs: u32,
    job_bytes: u64,
    job_entries: u32,
}
impl Usage {
    fn add(
        &mut self,
        bytes: u64,
        entries: u32,
        job: bool,
        limits: &JournalLimitsV1,
    ) -> JournalResult<()> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or(JournalError::LimitExceeded)?;
        self.entries = self
            .entries
            .checked_add(entries)
            .ok_or(JournalError::LimitExceeded)?;
        if job {
            self.job_bytes = self
                .job_bytes
                .checked_add(bytes)
                .ok_or(JournalError::LimitExceeded)?;
            self.job_entries = self
                .job_entries
                .checked_add(entries)
                .ok_or(JournalError::LimitExceeded)?;
        }
        if self.bytes > limits.scope_bytes
            || self.entries > limits.scope_entries
            || self.jobs > limits.jobs
            || self.job_bytes > limits.job_bytes
            || self.job_entries > limits.job_entries
        {
            return Err(JournalError::LimitExceeded);
        }
        Ok(())
    }
}

fn checked_core_record(core: &Inner, value: &impl Serialize, max: u64) -> JournalResult<Vec<u8>> {
    let bytes = canonical(value)?;
    if bytes.len() as u64 > max {
        return Err(JournalError::TooLarge);
    }
    core.control.record(&bytes)?;
    Ok(bytes)
}

fn encode_checkpoint(
    core: &Inner,
    operation_id: Uuid,
    intent_hash: &str,
    expected: &RevisionRefV1,
    next: &LoadedCheckpoint,
) -> JournalResult<(Vec<u8>, RevisionRefV1)> {
    let revision = expected
        .revision
        .checked_add(1)
        .ok_or(JournalError::LimitExceeded)?;
    macro_rules! encode {
        ($record:ident, $version:literal, $payload:expr, $domain:literal) => {{
            let bytes = checked_core_record(
                core,
                &$record {
                    version: $version,
                    operation_id,
                    intent_hash: intent_hash.to_owned(),
                    revision,
                    previous: Some(expected.clone()),
                    payload: $payload.clone(),
                },
                CHECKPOINT,
            )?;
            let head = RevisionRefV1 {
                revision,
                hash: digest($domain, &bytes),
            };
            Ok((bytes, head))
        }};
    }
    match next {
        LoadedCheckpoint::V1(value) => {
            encode!(Checkpoint, 1, value, "ai-guard:broker-checkpoint:v1")
        }
        LoadedCheckpoint::V2(value) => encode!(
            SupervisedCheckpoint,
            2,
            value,
            "ai-guard:broker-checkpoint:v2"
        ),
        LoadedCheckpoint::V3(value) => encode!(
            LifecycleCheckpoint,
            3,
            value,
            "ai-guard:broker-checkpoint:v3"
        ),
        LoadedCheckpoint::V4(value) => {
            encode!(OutboxCheckpoint, 4, value, "ai-guard:broker-checkpoint:v4")
        }
        LoadedCheckpoint::V5(value) => {
            encode!(
                RetirementCheckpoint,
                5,
                value,
                "ai-guard:broker-checkpoint:v5"
            )
        }
    }
}

struct PublicationResources<'a> {
    usage: Usage,
    budget: retirement::PublicationBudget,
    floors: &'a HashMap<Uuid, RevisionRefV1>,
}

/// The sole filesystem publication algorithm. Callers have already validated
/// their typed transition and supplied a complete metadata budget.
fn publish_revision(
    core: &Inner,
    operation_id: Uuid,
    intent_hash: &str,
    expected: &RevisionRefV1,
    head: &RevisionRefV1,
    bytes: &[u8],
    resources: PublicationResources<'_>,
) -> JournalResult<()> {
    let PublicationResources {
        mut usage,
        budget: publication,
        floors,
    } = resources;
    if head.revision >= u64::from(core.limits.revisions) {
        return Err(JournalError::LimitExceeded);
    }
    let job = fscheck(core.jobs.persistent_child(&operation_id.to_string(), false))?
        .ok_or(JournalError::Interrupted)?;
    if fscheck(job.persistent_read(
        &revision_name(head.revision),
        CHECKPOINT,
        &core.exclusions.0,
    ))?
    .is_some()
        || fscheck(job.persistent_read("current-next.json", SMALL, &core.exclusions.0))?.is_some()
    {
        return Err(JournalError::Interrupted);
    }
    let current = Current {
        version: 1,
        operation_id,
        intent_hash: intent_hash.to_owned(),
        head: head.clone(),
    };
    let pointer = checked_core_record(core, &current, SMALL)?;
    usage.add(
        bytes.len() as u64 + pointer.len() as u64,
        2,
        true,
        &core.limits,
    )?;
    retirement::reserve_future(
        core,
        operation_id,
        Some((head, bytes)),
        &usage,
        &publication,
        floors,
    )?;
    let file = io(job.persistent_create(&revision_name(head.revision), bytes))?;
    stage!(RevisionWritten);
    io(file.persistent_sync())?;
    io(job.sync())?;
    stage!(RevisionDurable);
    let temporary = io(job.persistent_create("current-next.json", &pointer))?;
    stage!(CurrentWritten);
    io(temporary.persistent_sync())?;
    let old = fscheck(job.persistent_read("current.json", SMALL, &core.exclusions.0))?
        .ok_or(JournalError::Corrupt)?;
    let old_current: Current = parse(old.bytes())?;
    if old_current.head != *expected {
        return Err(JournalError::Conflict);
    }
    io(job.persistent_publish(&temporary, &old, "current.json"))?;
    stage!(CurrentPublished);
    io(job.sync())?;
    stage!(CurrentDurable);
    Ok(())
}

#[cfg(test)]
fn stop_before_ack() -> JournalResult<()> {
    tests::checkpoint(FaultStage::BeforeAck)
}
#[cfg(test)]
pub(in crate::broker) fn fail_next_cold_ack_for_test() {
    tests::fail_once_at(FaultStage::BeforeAck);
}
/// The next revision of this thread's journal fails before its new head is
/// published: the revision is not current and the writer reports an error.
#[cfg(test)]
pub(in crate::broker) fn fail_next_publication_for_test() {
    tests::fail_once_at(FaultStage::CurrentWritten);
}
#[cfg(test)]
fn full_namespace_durable() -> JournalResult<()> {
    tests::checkpoint(FaultStage::NamespaceDurable)
}

impl Journal {
    /// The immutable scope admitted by this journal, never a caller assertion.
    pub fn scope(&self) -> &JournalScopeV1 {
        &self.0.scope
    }
    pub fn open(
        root: &Directory,
        scope: JournalScopeV1,
        exclusions: &FsExclusions,
        limits: JournalLimitsV1,
        control_secrets: ControlSecrets,
    ) -> JournalResult<Self> {
        scope.validate()?;
        let scope_bytes = canonical(&scope)?;
        control_secrets.record(&scope_bytes)?;
        if scope_bytes.len() as u64 > SMALL
            || limits.scope_entries < 3
            || scope_bytes.len() as u64 > limits.scope_bytes
        {
            return Err(JournalError::LimitExceeded);
        }
        let scope_hash = digest("ai-guard:broker-journal-scope:v1", &scope_bytes);
        fscheck(root.durable_bootstrap())?;
        let namespace =
            fscheck(root.persistent_child("broker-journal-v1", true))?.ok_or(JournalError::Io)?;
        let directory =
            fscheck(namespace.persistent_child(&scope_hash, true))?.ok_or(JournalError::Io)?;
        let lock = match fscheck(directory.persistent_read(".writer.lock", 0, &exclusions.0))? {
            Some(file) => file,
            None => {
                stage!(WriterLockAbsent);
                let file = match directory.persistent_create(".writer.lock", &[]) {
                    Ok(file) => file,
                    Err(error)
                        if error.downcast_ref::<std::io::Error>().is_some_and(|error| {
                            error.kind() == std::io::ErrorKind::AlreadyExists
                        }) =>
                    {
                        // Another first opener won O_EXCL. Reinspect once with
                        // the same no-link/ACL/exclusion guards, then try flock.
                        fscheck(directory.persistent_read(".writer.lock", 0, &exclusions.0))?
                            .ok_or(JournalError::Io)?
                    }
                    Err(_) => return Err(JournalError::Io),
                };
                io(file.persistent_sync())?;
                io(directory.sync())?;
                file
            }
        };
        if !fscheck(lock.persistent_try_lock())? {
            return Err(JournalError::Busy);
        }
        match fscheck(directory.persistent_read("scope.json", SMALL, &exclusions.0))? {
            Some(file) => {
                let existing: JournalScopeV1 = parse(file.bytes())?;
                if existing.0 != scope.0 || file.bytes() != scope_bytes {
                    return Err(JournalError::Conflict);
                }
                io(file.persistent_sync())?;
            }
            None => {
                let entries = fscheck(directory.persistent_entries(limits.scope_entries))?;
                if entries.iter().any(|entry| entry.name != ".writer.lock") {
                    return Err(JournalError::Corrupt);
                }
                let file = io(directory.persistent_create("scope.json", &scope_bytes))?;
                io(file.persistent_sync())?;
            }
        }
        io(directory.sync())?;
        let jobs = fscheck(directory.persistent_child("jobs", true))?.ok_or(JournalError::Io)?;
        let journal = Self(Arc::new(Inner {
            scope,
            scope_hash,
            directory,
            jobs,
            lock,
            exclusions: exclusions.clone(),
            limits,
            control: control_secrets,
            writer: Mutex::new(Writer::default()),
            budget_incomplete: std::sync::atomic::AtomicBool::new(false),
            status_observation_failed: std::sync::atomic::AtomicBool::new(false),
        }));
        journal.usage(None)?;
        stage!(NamespaceDurable);
        Ok(journal)
    }

    pub fn reserve(&self, operation_id: Uuid, submit_bytes: &[u8]) -> JournalResult<ReserveResult> {
        if submit_bytes.len() > 256 * 1024 {
            return Err(JournalError::TooLarge);
        }
        if operation_id.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        self.0.control.bytes(submit_bytes)?;
        let submit_hash = digest("ai-guard:broker-submit:v1", submit_bytes);
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        let name = operation_id.to_string();
        if let Some(job) = fscheck(self.0.jobs.persistent_child(&name, false))? {
            if self.read_file(&job, "reservation.json", SMALL)?.is_none() {
                self.status_locked(operation_id, &mut writer)?;
                return Ok(ReserveResult::Interrupted);
            }
            let reservation: Reservation = self.read(&job, "reservation.json", SMALL)?;
            self.check_reservation(&reservation, operation_id)?;
            if reservation.submit_hash != submit_hash {
                return Err(JournalError::Conflict);
            }
            let result = match self.status_locked(operation_id, &mut writer)? {
                Some(JournalStatusV1::Accepted(job)) => ReserveResult::Accepted(job),
                Some(JournalStatusV1::Rejected(receipt)) => ReserveResult::Rejected(receipt),
                Some(JournalStatusV1::ReservedInProcess) => ReserveResult::InProgress,
                Some(JournalStatusV1::Interrupted) => ReserveResult::Interrupted,
                None => return Err(JournalError::Interrupted),
            };
            stage!(BeforeAck);
            return Ok(result);
        }
        if writer.observed_jobs.contains(&operation_id)
            || writer.floors.contains_key(&operation_id)
            || writer.rejected_floors.contains_key(&operation_id)
        {
            return Err(JournalError::Conflict);
        }
        self.check_observation_capacity(operation_id, &writer)?;
        let reservation = Reservation {
            version: 1,
            scope_hash: self.0.scope_hash.clone(),
            operation_id,
            submit_hash: submit_hash.clone(),
        };
        let bytes = self.checked_record(&reservation, SMALL)?;
        let mut usage = self.usage(None)?;
        usage.jobs = usage
            .jobs
            .checked_add(1)
            .ok_or(JournalError::LimitExceeded)?;
        usage.add(bytes.len() as u64, 2, true, &self.0.limits)?;
        retirement::reserve_future(
            &self.0,
            operation_id,
            None,
            &usage,
            &retirement::PublicationBudget::NewAllocation,
            &writer.floors,
        )?;
        let job = fscheck(self.0.jobs.persistent_child(&name, true))?.ok_or(JournalError::Io)?;
        let file = io(job.persistent_create("reservation.json", &bytes))?;
        stage!(ReservationWritten);
        io(file.persistent_sync())?;
        io(job.sync())?;
        io(self.0.jobs.sync())?;
        stage!(ReservationDurable);
        writer.observed_jobs.insert(operation_id);
        let lease = Arc::new(());
        writer.tickets.insert(operation_id, Arc::downgrade(&lease));
        Ok(ReserveResult::New(ReservationTicket {
            owner: self.0.clone(),
            lease,
            operation: operation_id,
            submit_hash,
        }))
    }

    pub fn reject(
        &self,
        ticket: ReservationTicket,
        phase: RejectionPhaseV1,
        code: RejectionCodeV1,
    ) -> JournalResult<RejectionReceiptV1> {
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        if !Arc::ptr_eq(&ticket.owner, &self.0)
            || writer
                .tickets
                .get(&ticket.operation)
                .and_then(Weak::upgrade)
                .is_none_or(|lease| !Arc::ptr_eq(&lease, &ticket.lease))
            || writer.floors.contains_key(&ticket.operation)
            || writer.rejected_floors.contains_key(&ticket.operation)
        {
            return Err(JournalError::Conflict);
        }
        let job = self.job(ticket.operation)?;
        let entries = fscheck(job.persistent_entries(self.0.limits.job_entries))?;
        if entries.iter().any(|entry| entry.name == "accepted.json")
            && entries.iter().any(|entry| entry.name == "rejected.json")
        {
            return Err(JournalError::Corrupt);
        }
        if entries.len() != 1 || entries[0].name != "reservation.json" || entries[0].directory {
            return Err(JournalError::Interrupted);
        }
        let reservation: Reservation = self.read_canonical(&job, "reservation.json", SMALL)?;
        self.check_reservation(&reservation, ticket.operation)?;
        if reservation.submit_hash != ticket.submit_hash {
            return Err(JournalError::Conflict);
        }
        let record = Rejected {
            version: 1,
            scope_hash: self.0.scope_hash.clone(),
            operation_id: ticket.operation,
            submit_hash: ticket.submit_hash.clone(),
            phase,
            code,
        };
        let bytes = self.checked_record(&record, SMALL)?;
        self.budget(
            &ticket.operation.to_string(),
            bytes.len() as u64,
            1,
            &writer,
        )?;
        let file = io(job.persistent_create("rejected.json", &bytes))?;
        stage!(RejectedWritten);
        io(file.persistent_sync())?;
        io(job.sync())?;
        io(self.0.jobs.sync())?;
        stage!(RejectedDurable);
        let receipt = self.read_rejection(&job, &reservation, &mut writer)?;
        writer.tickets.remove(&ticket.operation);
        stage!(BeforeAck);
        Ok(receipt)
    }

    pub fn status(&self, operation_id: Uuid) -> JournalResult<Option<JournalStatusV1>> {
        if operation_id.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        self.status_locked(operation_id, &mut writer)
    }

    fn status_locked(
        &self,
        operation_id: Uuid,
        writer: &mut Writer,
    ) -> JournalResult<Option<JournalStatusV1>> {
        let result = self.status_locked_inner(operation_id, writer);
        self.note_status_failure(&result);
        result
    }
    fn status_locked_inner(
        &self,
        operation_id: Uuid,
        writer: &mut Writer,
    ) -> JournalResult<Option<JournalStatusV1>> {
        let name = operation_id.to_string();
        let Some(job) = fscheck(self.0.jobs.persistent_child(&name, false))? else {
            if writer.observed_jobs.contains(&operation_id)
                || writer.floors.contains_key(&operation_id)
                || writer.rejected_floors.contains_key(&operation_id)
            {
                return Err(JournalError::Conflict);
            }
            return Ok(None);
        };
        self.usage(Some(&name))?;
        self.check_observation_capacity(operation_id, writer)?;
        let accepted = self.read_file(&job, "accepted.json", SMALL)?.is_some();
        let rejected = self.read_file(&job, "rejected.json", SMALL)?.is_some();
        if accepted && rejected {
            return Err(JournalError::Corrupt);
        }
        if (writer.floors.contains_key(&operation_id) && !accepted)
            || (writer.rejected_floors.contains_key(&operation_id) && !rejected)
        {
            return Err(JournalError::Conflict);
        }
        if accepted {
            let value = self
                .load_locked(operation_id, writer)?
                .ok_or(JournalError::Corrupt)?;
            return Ok(Some(JournalStatusV1::Accepted(value)));
        }
        if self.read_file(&job, "reservation.json", SMALL)?.is_none() {
            return if rejected {
                Err(JournalError::Corrupt)
            } else {
                writer.observed_jobs.insert(operation_id);
                Ok(Some(JournalStatusV1::Interrupted))
            };
        }
        let reservation: Reservation = self.read_canonical(&job, "reservation.json", SMALL)?;
        self.check_reservation(&reservation, operation_id)?;
        if rejected {
            return Ok(Some(JournalStatusV1::Rejected(self.read_rejection(
                &job,
                &reservation,
                writer,
            )?)));
        }
        let entries = fscheck(job.persistent_entries(self.0.limits.job_entries))?;
        writer.observed_jobs.insert(operation_id);
        if entries.len() == 1
            && entries[0].name == "reservation.json"
            && writer
                .tickets
                .get(&operation_id)
                .and_then(Weak::upgrade)
                .is_some()
        {
            Ok(Some(JournalStatusV1::ReservedInProcess))
        } else {
            Ok(Some(JournalStatusV1::Interrupted))
        }
    }

    fn read_rejection(
        &self,
        job: &Directory,
        reservation: &Reservation,
        writer: &mut Writer,
    ) -> JournalResult<RejectionReceiptV1> {
        let entries = fscheck(job.persistent_entries(self.0.limits.job_entries))?;
        if entries.len() != 2
            || entries.iter().any(|entry| {
                entry.directory
                    || !matches!(entry.name.as_str(), "reservation.json" | "rejected.json")
            })
        {
            return Err(JournalError::Corrupt);
        }
        let record: Rejected = self.read_canonical(job, "rejected.json", SMALL)?;
        if record.version != 1
            || record.scope_hash != self.0.scope_hash
            || record.operation_id != reservation.operation_id
            || record.submit_hash != reservation.submit_hash
        {
            return Err(JournalError::Corrupt);
        }
        let record_hash = digest("ai-guard:broker-rejection:v1", &canonical(&record)?);
        if writer.floors.contains_key(&record.operation_id)
            || writer
                .rejected_floors
                .get(&record.operation_id)
                .is_some_and(|old| old != &record_hash)
        {
            return Err(JournalError::Conflict);
        }
        io(job.sync())?;
        io(self.0.jobs.sync())?;
        writer.observed_jobs.insert(record.operation_id);
        writer
            .rejected_floors
            .insert(record.operation_id, record_hash.clone());
        Ok(RejectionReceiptV1 {
            operation_id: record.operation_id,
            submit_hash: record.submit_hash,
            phase: record.phase,
            code: record.code,
            record_hash,
        })
    }

    pub fn accept(
        &self,
        ticket: ReservationTicket,
        intent: &ExecutionIntentV1,
        bindings: BindingInputs<'_>,
        checkpoint0: &CheckpointPayloadV1,
    ) -> JournalResult<AcceptedJobV1> {
        self.accept_with_launch_profile(ticket, intent, bindings, checkpoint0, None)
    }
    pub(crate) fn accept_with_launch_profile(
        &self,
        ticket: ReservationTicket,
        intent: &ExecutionIntentV1,
        bindings: BindingInputs<'_>,
        checkpoint0: &CheckpointPayloadV1,
        launch: Option<&LaunchProfileSelection>,
    ) -> JournalResult<AcceptedJobV1> {
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        if !Arc::ptr_eq(&ticket.owner, &self.0)
            || ticket.operation != intent.operation_id()
            || writer
                .tickets
                .get(&ticket.operation)
                .and_then(Weak::upgrade)
                .is_none_or(|lease| !Arc::ptr_eq(&lease, &ticket.lease))
        {
            return Err(JournalError::Conflict);
        }
        intent.validate().map_err(|_| JournalError::InvalidInput)?;
        let initial = CheckpointPayloadV1::initial(intent)?;
        if initial.canonical_bytes()? != checkpoint0.canonical_bytes()? {
            return Err(JournalError::Conflict);
        }
        let intent_bytes = intent
            .canonical_bytes()
            .map_err(|_| JournalError::InvalidInput)?;
        self.0.control.record(&intent_bytes)?;
        let intent_hash = intent
            .intent_hash()
            .map_err(|_| JournalError::InvalidInput)?;
        let (binding, binding_bytes) = self.prepare_binding(intent, bindings)?;
        self.context(checkpoint0, intent, &binding)?;
        let manifest = Bindings {
            version: if launch.is_some() { 2 } else { 1 },
            operation_id: ticket.operation,
            intent_hash: intent_hash.clone(),
            binding,
            docker_launch_profile: launch
                .map(|selection| {
                    selection.binding(&self.0.scope_hash, ticket.operation, &intent_hash)
                })
                .transpose()
                .map_err(|_| JournalError::InvalidInput)?,
        };
        manifest
            .validate_launch(
                manifest.version,
                &self.0.scope_hash,
                ticket.operation,
                intent,
            )
            .map_err(|_| JournalError::InvalidInput)?;
        let manifest_bytes = self.checked_record(&manifest, SMALL)?;
        let checkpoint = Checkpoint {
            version: 1,
            operation_id: ticket.operation,
            intent_hash: intent_hash.clone(),
            revision: 0,
            previous: None,
            payload: checkpoint0.clone(),
        };
        let checkpoint_bytes = self.checked_record(&checkpoint, CHECKPOINT)?;
        let head = RevisionRefV1 {
            revision: 0,
            hash: digest("ai-guard:broker-checkpoint:v1", &checkpoint_bytes),
        };
        let current = Current {
            version: 1,
            operation_id: ticket.operation,
            intent_hash: intent_hash.clone(),
            head: head.clone(),
        };
        let current_bytes = self.checked_record(&current, SMALL)?;
        let accepted = Accepted {
            version: manifest.version,
            scope_hash: self.0.scope_hash.clone(),
            operation_id: ticket.operation,
            submit_hash: ticket.submit_hash.clone(),
            intent_hash,
            bindings_hash: manifest.hash(&manifest_bytes)?,
            initial_checkpoint: head,
        };
        let accepted_bytes = self.checked_record(&accepted, SMALL)?;
        let name = ticket.operation.to_string();
        let job = self.job(ticket.operation)?;
        let reservation: Reservation = self.read(&job, "reservation.json", SMALL)?;
        self.check_reservation(&reservation, ticket.operation)?;
        if reservation.submit_hash != ticket.submit_hash {
            return Err(JournalError::Conflict);
        }
        let entries = fscheck(job.persistent_entries(self.0.limits.job_entries))?;
        if entries.len() != 1 || entries[0].name != "reservation.json" {
            return Err(JournalError::Interrupted);
        }
        let added_bytes = intent_bytes.len() as u64
            + manifest_bytes.len() as u64
            + checkpoint_bytes.len() as u64
            + current_bytes.len() as u64
            + accepted_bytes.len() as u64
            + binding_bytes.as_ref().map_or(0, |b| b.len() as u64);
        self.budget(
            &name,
            added_bytes + if launch.is_some() { 3 } else { 0 },
            5 + if binding_bytes.is_some() { 2 } else { 0 } + if launch.is_some() { 2 } else { 0 },
            &writer,
        )?;
        if let Some(bytes) = binding_bytes {
            let directory =
                fscheck(job.persistent_child("bindings", true))?.ok_or(JournalError::Io)?;
            let filename = manifest.binding.file_name().ok_or(JournalError::Corrupt)?;
            let file = io(directory.persistent_create(filename, &bytes))?;
            stage!(BindingWritten);
            io(file.persistent_sync())?;
            io(directory.sync())?;
            io(job.sync())?;
            stage!(BindingDurable);
        }
        if let Some(profile) = &manifest.docker_launch_profile {
            io(RetainedDockerLaunchProfile::create(&job, profile))?;
        }
        self.write(&job, "intent.json", &intent_bytes)?;
        stage!(IntentDurable);
        self.write(&job, "bindings.json", &manifest_bytes)?;
        stage!(ManifestDurable);
        let revision = io(job.persistent_create(&revision_name(0), &checkpoint_bytes))?;
        stage!(RevisionWritten);
        io(revision.persistent_sync())?;
        io(job.sync())?;
        stage!(RevisionDurable);
        let file = io(job.persistent_create("current.json", &current_bytes))?;
        stage!(CurrentWritten);
        io(file.persistent_sync())?;
        io(job.sync())?;
        stage!(CurrentDurable);
        let file = io(job.persistent_create("accepted.json", &accepted_bytes))?;
        stage!(AcceptedWritten);
        io(file.persistent_sync())?;
        io(job.sync())?;
        io(self.0.jobs.sync())?;
        stage!(AcceptedDurable);
        let result = self
            .load_locked(ticket.operation, &mut writer)?
            .ok_or(JournalError::Corrupt)?;
        stage!(BeforeAck);
        Ok(result)
    }

    pub fn load(&self, operation_id: Uuid) -> JournalResult<Option<AcceptedJobV1>> {
        if operation_id.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        match self.status_locked(operation_id, &mut writer)? {
            None => Ok(None),
            Some(JournalStatusV1::Accepted(value)) => Ok(Some(value)),
            Some(JournalStatusV1::Rejected(_)) => Err(JournalError::InvalidInput),
            Some(_) => Err(JournalError::Interrupted),
        }
    }

    pub fn list(&self, after: Option<Uuid>, limit: u32) -> JournalResult<JournalPageV1> {
        let result = self.list_inner(after, limit);
        self.note_status_failure(&result);
        result
    }
    fn list_inner(&self, after: Option<Uuid>, limit: u32) -> JournalResult<JournalPageV1> {
        if !(1..=100).contains(&limit) || after.is_some_and(|id| id.is_nil()) {
            return Err(JournalError::InvalidInput);
        }
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        self.usage(None)?;
        let mut ids = Vec::new();
        for entry in fscheck(self.0.jobs.persistent_entries(self.0.limits.scope_entries))? {
            let id = Uuid::parse_str(&entry.name).map_err(|_| JournalError::Corrupt)?;
            if !entry.directory || id.is_nil() || id.to_string() != entry.name {
                return Err(JournalError::Corrupt);
            }
            ids.push(id);
        }
        ids.sort_unstable();
        let observed: HashSet<_> = writer
            .observed_jobs
            .iter()
            .chain(writer.floors.keys())
            .chain(writer.rejected_floors.keys())
            .copied()
            .collect();
        for id in observed {
            if ids.binary_search(&id).is_err() {
                return Err(JournalError::Conflict);
            }
            self.status_locked(id, &mut writer)?
                .ok_or(JournalError::Conflict)?;
        }
        let mut selected = ids
            .into_iter()
            .filter(|id| after.is_none_or(|cursor| *id > cursor));
        let mut entries = Vec::new();
        for id in selected.by_ref().take(limit as usize) {
            let status = self
                .status_locked(id, &mut writer)?
                .ok_or(JournalError::Interrupted)?;
            entries.push(Self::summary(id, &status));
        }
        let next_after = if selected.next().is_some() {
            entries.last().map(|entry| entry.operation_id)
        } else {
            None
        };
        Ok(JournalPageV1 {
            entries,
            next_after,
        })
    }

    fn summary(operation_id: Uuid, status: &JournalStatusV1) -> JournalEntryV1 {
        let (kind, job) = match status {
            JournalStatusV1::ReservedInProcess => (JournalEntryKindV1::ReservedInProcess, None),
            JournalStatusV1::Interrupted => (JournalEntryKindV1::Interrupted, None),
            JournalStatusV1::Rejected(_) => (JournalEntryKindV1::Rejected, None),
            JournalStatusV1::Accepted(job) => (JournalEntryKindV1::Accepted, Some(job)),
        };
        Self::summary_data(operation_id, kind, job)
    }

    fn accepted_summary(operation_id: Uuid, job: &AcceptedJobV1) -> JournalEntryV1 {
        Self::summary_data(operation_id, JournalEntryKindV1::Accepted, Some(job))
    }

    fn summary_data(
        operation_id: Uuid,
        kind: JournalEntryKindV1,
        job: Option<&AcceptedJobV1>,
    ) -> JournalEntryV1 {
        let mut entry = JournalEntryV1 {
            operation_id,
            kind,
            head: None,
            cancel_requested: false,
            approval_submitted: false,
            failure_count: 0,
            pending_rpc_count: None,
            worker_state: None,
            quota_held: None,
            runtime_status: None,
            last_observed_at: None,
        };
        if let Some(job) = job {
            entry.head = Some(job.head().clone());
            if let Some(checkpoint) = job.broker_checkpoint_v2() {
                entry.pending_rpc_count = Some(
                    checkpoint
                        .supervision()
                        .operations()
                        .iter()
                        .filter(|op| !op.has_response())
                        .count(),
                );
            }
            if let Some(checkpoint) = job.broker_checkpoint_v3() {
                use super::lifecycle::LocalCommandRequestV1;
                for command in checkpoint.commands() {
                    match command.request() {
                        LocalCommandRequestV1::CancelRequested => entry.cancel_requested = true,
                        LocalCommandRequestV1::ApprovalSubmitted { .. } => {
                            entry.approval_submitted = true
                        }
                        LocalCommandRequestV1::FailureObserved { .. } => entry.failure_count += 1,
                    }
                }
            }
            if let Some(worker) = job.checkpoint().broker_context().worker {
                entry.worker_state = Some(worker.state);
                entry.quota_held = Some(worker.quota_held);
                if let Some(observation) = &worker.last_observation {
                    entry.runtime_status = Some(observation.status);
                    entry.last_observed_at = Some(observation.observed_at.clone());
                }
            }
        }
        entry
    }

    pub fn command_receipt(
        &self,
        operation_id: Uuid,
        command_id: Uuid,
    ) -> JournalResult<Option<LocalCommandReceiptV1>> {
        if operation_id.is_nil() || command_id.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        let Some(JournalStatusV1::Accepted(mut job)) =
            self.status_locked(operation_id, &mut writer)?
        else {
            return Err(JournalError::InvalidInput);
        };
        Ok(job.0.command_receipts.remove(&command_id))
    }

    /// The first confirmed revision for this delivery state, derived from the
    /// validated chain. `None` asks for birth even if an ACK was later recorded.
    pub fn outbox_receipt(
        &self,
        operation_id: Uuid,
        event_id: Uuid,
        audit_seq: Option<i64>,
    ) -> JournalResult<Option<OutboxReceiptV1>> {
        if operation_id.is_nil() || event_id.is_nil() || audit_seq.is_some_and(|seq| seq <= 0) {
            return Err(JournalError::InvalidInput);
        }
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        let Some(JournalStatusV1::Accepted(mut job)) =
            self.status_locked(operation_id, &mut writer)?
        else {
            return Err(JournalError::InvalidInput);
        };
        Ok(job.0.outbox_receipts.remove(&(event_id, audit_seq)))
    }

    pub fn advance(
        &self,
        operation_id: Uuid,
        expected: RevisionRefV1,
        next: &CheckpointPayloadV1,
    ) -> JournalResult<CheckpointReceiptV1> {
        self.advance_checkpoint(operation_id, expected, LoadedCheckpoint::V1(next.clone()))
    }
    pub fn advance_supervised(
        &self,
        operation_id: Uuid,
        expected: RevisionRefV1,
        next: &BrokerCheckpointV2,
    ) -> JournalResult<CheckpointReceiptV1> {
        self.advance_checkpoint(operation_id, expected, LoadedCheckpoint::V2(next.clone()))
    }
    pub fn advance_lifecycle(
        &self,
        operation_id: Uuid,
        expected: RevisionRefV1,
        next: &BrokerCheckpointV3,
    ) -> JournalResult<CheckpointReceiptV1> {
        self.advance_checkpoint(operation_id, expected, LoadedCheckpoint::V3(next.clone()))
    }
    pub fn advance_outbox(
        &self,
        operation_id: Uuid,
        expected: RevisionRefV1,
        next: &BrokerCheckpointV4,
    ) -> JournalResult<CheckpointReceiptV1> {
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        let loaded = self
            .load_locked(operation_id, &mut writer)?
            .ok_or(JournalError::InvalidInput)?;
        let next = match loaded.broker_checkpoint_v5() {
            Some(current) => LoadedCheckpoint::V5(current.with_outbox(next.clone())?),
            None => LoadedCheckpoint::V4(next.clone()),
        };
        valid_hash(&expected.hash)?;
        self.advance_loaded_checkpoint(operation_id, expected, next, loaded, &mut writer)
    }
    /// Writer adaptation shares the mutation's strict preload and mutex. A V4
    /// writer update requires the current head even for an otherwise exact replay.
    pub(crate) fn advance_lifecycle_preserving_outbox(
        &self,
        operation_id: Uuid,
        expected: RevisionRefV1,
        next: &BrokerCheckpointV3,
    ) -> JournalResult<CheckpointReceiptV1> {
        if operation_id.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        let loaded = match self.status_locked(operation_id, &mut writer)? {
            Some(JournalStatusV1::Accepted(job)) => job,
            None | Some(JournalStatusV1::Rejected(_)) => return Err(JournalError::InvalidInput),
            Some(_) => return Err(JournalError::Interrupted),
        };
        let next = if let Some(current) = loaded.broker_checkpoint_v5() {
            if loaded.head() != &expected {
                return Err(JournalError::Conflict);
            }
            LoadedCheckpoint::V5(current.with_outbox(BrokerCheckpointV4::new(
                next.clone(),
                current.outbox().events().to_vec(),
            )?)?)
        } else if let Some(current) = loaded.broker_checkpoint_v4() {
            if loaded.head() != &expected {
                return Err(JournalError::Conflict);
            }
            LoadedCheckpoint::V4(BrokerCheckpointV4::new(
                next.clone(),
                current.events().to_vec(),
            )?)
        } else {
            super::outbox::validate_command_ids(next, &[])?;
            LoadedCheckpoint::V3(next.clone())
        };
        // Preserve the writer adapter's ordering: load and V4 head/ID checks
        // precede the expected-hash validation of the ordinary advance path.
        valid_hash(&expected.hash)?;
        self.advance_loaded_checkpoint(operation_id, expected, next, loaded, &mut writer)
    }
    fn advance_checkpoint(
        &self,
        operation_id: Uuid,
        expected: RevisionRefV1,
        next: LoadedCheckpoint,
    ) -> JournalResult<CheckpointReceiptV1> {
        valid_hash(&expected.hash)?;
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        let loaded = self
            .load_locked(operation_id, &mut writer)?
            .ok_or(JournalError::InvalidInput)?;
        self.advance_loaded_checkpoint(operation_id, expected, next, loaded, &mut writer)
    }

    /// The caller retains the writer mutex from this strict preload through
    /// publication and the unchanged strict postload below.
    fn advance_loaded_checkpoint(
        &self,
        operation_id: Uuid,
        expected: RevisionRefV1,
        next: LoadedCheckpoint,
        loaded: AcceptedJobV1,
        writer: &mut Writer,
    ) -> JournalResult<CheckpointReceiptV1> {
        if (matches!(next, LoadedCheckpoint::V1(_)) && loaded.broker_checkpoint_v2().is_some())
            || (next.lifecycle().is_none() && loaded.broker_checkpoint_v3().is_some())
            || (next.outbox().is_none() && loaded.broker_checkpoint_v4().is_some())
            || (next.retirement().is_none() && loaded.broker_checkpoint_v5().is_some())
        {
            return Err(JournalError::Conflict);
        }
        confirmed::context_checkpoint_with_launch(
            &self.0.scope,
            &self.0.control,
            &next,
            &loaded.0.intent,
            &loaded.0.binding,
            loaded.0.launch_binding.as_ref(),
        )?;
        let (bytes, head) = encode_checkpoint(
            &self.0,
            operation_id,
            &loaded.0.intent_hash,
            &expected,
            &next,
        )?;
        if expected.revision < loaded.0.head.revision {
            let index = usize::try_from(expected.revision).map_err(|_| JournalError::Conflict)?;
            if loaded.0.chain.get(index) == Some(&expected)
                && loaded.0.chain.get(index + 1) == Some(&head)
            {
                stage!(BeforeAck);
                return Ok(CheckpointReceiptV1 { operation_id, head });
            }
            return Err(JournalError::Conflict);
        }
        if expected != loaded.0.head {
            return Err(JournalError::Conflict);
        }
        self.check_transition(
            &loaded.0.checkpoint,
            &next,
            loaded.0.legacy_managed_seen,
            loaded.0.last_cancel_operations,
        )?;
        let usage = self.usage(Some(&operation_id.to_string()))?;
        publish_revision(
            &self.0,
            operation_id,
            &loaded.0.intent_hash,
            &expected,
            &head,
            &bytes,
            PublicationResources {
                usage,
                budget: retirement::PublicationBudget::NewAllocation,
                floors: &writer.floors,
            },
        )?;
        // The CAS and directory barrier committed this head even when a later
        // business credential recheck fails. Preserve the conservative floor.
        writer.floors.insert(operation_id, head.clone());
        writer.observed_jobs.insert(operation_id);
        let result = self
            .load_locked(operation_id, writer)?
            .ok_or(JournalError::Corrupt)?;
        if result.0.head != head {
            return Err(JournalError::Corrupt);
        }
        stage!(BeforeAck);
        Ok(CheckpointReceiptV1 { operation_id, head })
    }
}

impl Journal {
    fn prepare_binding(
        &self,
        intent: &ExecutionIntentV1,
        inputs: BindingInputs<'_>,
    ) -> JournalResult<(BindingManifestV1, Option<Vec<u8>>)> {
        let options = intent.initial_options().run_options();
        match inputs {
            BindingInputs::None => {
                if options.secret_env_file.is_some() || intent.provider_credential().is_some() {
                    return Err(JournalError::InvalidInput);
                }
                Ok((BindingManifestV1(BindingData::None), None))
            }
            BindingInputs::Manual { source } => {
                if intent.provider_credential().is_some()
                    || options.secret_env_file.as_deref() != Some(source.path())
                {
                    return Err(JournalError::Conflict);
                }
                if source.bytes().len() > 64 * 1024 {
                    return Err(JournalError::TooLarge);
                }
                fscheck(source.persistent_revalidate())?;
                let material = crate::runner::prepare_manual_credentials(source.bytes())
                    .map_err(|_| JournalError::InvalidInput)?;
                self.check_material(&material)?;
                let stored_sha256 = sha(&material.bytes);
                let manifest = BindingManifestV1(BindingData::Manual {
                    binding_id: Uuid::new_v4(),
                    file_name: "manual-env-v1.bin".into(),
                    names: material.names,
                    source_sha256: source.digest().into(),
                    handoff_sha256: stored_sha256.clone(),
                    stored_sha256,
                    stored_bytes: material.bytes.len() as u64,
                });
                Ok((manifest, Some(material.bytes)))
            }
            BindingInputs::Provider { source } => {
                let reference = intent
                    .provider_credential()
                    .ok_or(JournalError::InvalidInput)?;
                if options.secret_env_file.is_some() || source.digest() != reference.digest() {
                    return Err(JournalError::Conflict);
                }
                if source.bytes().len() > 512 * 1024 {
                    return Err(JournalError::TooLarge);
                }
                fscheck(source.persistent_revalidate())?;
                self.0.control.record(source.bytes())?;
                let material = crate::runner::prepare_provider_credentials(
                    source.bytes(),
                    reference.binding_id(),
                )
                .map_err(|_| JournalError::InvalidInput)?;
                self.check_material(&material)?;
                self.check_provider_context(intent, &material.bytes)?;
                let manifest = BindingManifestV1(BindingData::Provider {
                    binding_id: reference.binding_id(),
                    file_name: reference.file_name().into(),
                    names: material.names,
                    source_sha256: source.digest().into(),
                    stored_sha256: source.digest().into(),
                    handoff_sha256: sha(&material.bytes),
                    stored_bytes: source.bytes().len() as u64,
                });
                Ok((manifest, Some(source.bytes().to_vec())))
            }
        }
    }
    fn check_material(
        &self,
        material: &crate::runner::PreparedCredentialMaterial,
    ) -> JournalResult<()> {
        self.0.control.bytes(&material.bytes)?;
        for secret in &material.secrets {
            self.0.control.bytes(secret.as_bytes())?;
        }
        for name in &material.names {
            self.0.control.bytes(name.as_bytes())?;
        }
        Ok(())
    }
    fn check_provider_context(
        &self,
        intent: &ExecutionIntentV1,
        env_bytes: &[u8],
    ) -> JournalResult<()> {
        let options = intent.initial_options().run_options();
        let strings = crate::provider::validate_frozen_credential(&options.argv, env_bytes)
            .map_err(|_| JournalError::InvalidInput)?;
        for value in strings {
            self.0.control.bytes(value.as_bytes())?;
        }
        Ok(())
    }
    fn load_binding(
        &self,
        job: &Directory,
        intent: &ExecutionIntentV1,
        binding: &BindingManifestV1,
    ) -> JournalResult<Option<PrivateFile>> {
        let options = intent.initial_options().run_options();
        let directory = fscheck(job.persistent_child("bindings", false))?;
        if binding.kind() == BindingKindV1::None {
            if options.secret_env_file.is_some() || intent.provider_credential().is_some() {
                return Err(JournalError::Corrupt);
            }
            if let Some(directory) = directory {
                if !fscheck(directory.persistent_entries(self.0.limits.job_entries))?.is_empty() {
                    return Err(JournalError::Corrupt);
                }
                io(directory.sync())?;
            }
            return Ok(None);
        }
        let id = binding.binding_id().ok_or(JournalError::Corrupt)?;
        if id.is_nil() || binding.stored_bytes() == 0 {
            return Err(JournalError::Corrupt);
        }
        for hash in [
            binding.source_sha(),
            binding.stored_sha(),
            binding.handoff_sha(),
        ] {
            valid_hash(hash.ok_or(JournalError::Corrupt)?).map_err(|_| JournalError::Corrupt)?;
        }
        let directory = directory.ok_or(JournalError::Interrupted)?;
        let name = binding.file_name().ok_or(JournalError::Corrupt)?;
        let entries = fscheck(directory.persistent_entries(self.0.limits.job_entries))?;
        if entries.len() != 1 || entries[0].name != name || entries[0].directory {
            return Err(JournalError::Corrupt);
        }
        let max = match binding.kind() {
            BindingKindV1::Manual => 64 * 1024,
            BindingKindV1::Provider => 512 * 1024,
            BindingKindV1::None => return Err(JournalError::Corrupt),
        };
        let file = self
            .read_file(&directory, name, max)?
            .ok_or(JournalError::Interrupted)?;
        if file.digest() != binding.stored_sha().ok_or(JournalError::Corrupt)?
            || file.bytes().len() as u64 != binding.stored_bytes()
        {
            return Err(JournalError::Corrupt);
        }
        let material = match binding.kind() {
            BindingKindV1::Manual => {
                if intent.provider_credential().is_some()
                    || options.secret_env_file.is_none()
                    || name != "manual-env-v1.bin"
                {
                    return Err(JournalError::Corrupt);
                }
                let material = crate::runner::prepare_manual_credentials(file.bytes())
                    .map_err(|_| JournalError::Corrupt)?;
                if material.bytes != file.bytes() || binding.handoff_sha() != binding.stored_sha() {
                    return Err(JournalError::Corrupt);
                }
                material
            }
            BindingKindV1::Provider => {
                let reference = intent.provider_credential().ok_or(JournalError::Corrupt)?;
                if options.secret_env_file.is_some()
                    || id != reference.binding_id()
                    || name != reference.file_name()
                    || binding.stored_sha() != Some(reference.digest())
                    || binding.source_sha() != Some(reference.digest())
                {
                    return Err(JournalError::Corrupt);
                }
                self.0.control.record(file.bytes())?;
                crate::runner::prepare_provider_credentials(file.bytes(), id)
                    .map_err(|_| JournalError::Corrupt)?
            }
            BindingKindV1::None => return Err(JournalError::Corrupt),
        };
        self.check_material(&material)?;
        if binding.kind() == BindingKindV1::Provider {
            self.check_provider_context(intent, &material.bytes)?;
        }
        if binding.names() != material.names
            || binding.handoff_sha() != Some(sha(&material.bytes).as_str())
        {
            return Err(JournalError::Corrupt);
        }
        io(file.persistent_sync())?;
        io(directory.sync())?;
        Ok(Some(file))
    }
    fn load_locked(
        &self,
        operation_id: Uuid,
        writer: &mut Writer,
    ) -> JournalResult<Option<AcceptedJobV1>> {
        let result = self.load_locked_inner(operation_id, writer);
        self.note_status_failure(&result);
        result
    }
    fn load_locked_inner(
        &self,
        operation_id: Uuid,
        writer: &mut Writer,
    ) -> JournalResult<Option<AcceptedJobV1>> {
        #[cfg(test)]
        cost_measurement::load(self.0.directory.path(), operation_id, false);
        let name = operation_id.to_string();
        let Some(job) = fscheck(self.0.jobs.persistent_child(&name, false))? else {
            return if writer.observed_jobs.contains(&operation_id)
                || writer.floors.contains_key(&operation_id)
                || writer.rejected_floors.contains_key(&operation_id)
            {
                Err(JournalError::Conflict)
            } else {
                Ok(None)
            };
        };
        self.usage(Some(&name))?;
        self.check_observation_capacity(operation_id, writer)?;
        let has_accepted = self.read_file(&job, "accepted.json", SMALL)?.is_some();
        let has_rejected = self.read_file(&job, "rejected.json", SMALL)?.is_some();
        if has_accepted && has_rejected {
            return Err(JournalError::Corrupt);
        }
        if (writer.floors.contains_key(&operation_id) && !has_accepted)
            || writer.rejected_floors.contains_key(&operation_id) && !has_rejected
        {
            return Err(JournalError::Conflict);
        }
        if has_rejected {
            let reservation: Reservation = self.read_canonical(&job, "reservation.json", SMALL)?;
            self.check_reservation(&reservation, operation_id)?;
            self.read_rejection(&job, &reservation, writer)?;
            return Err(JournalError::InvalidInput);
        }
        let reservation: Reservation = self.read(&job, "reservation.json", SMALL)?;
        self.check_reservation(&reservation, operation_id)?;
        let accepted: Accepted = self.read(&job, "accepted.json", SMALL)?;
        if !matches!(accepted.version, 1 | 2)
            || accepted.operation_id != operation_id
            || accepted.scope_hash != self.0.scope_hash
            || accepted.submit_hash != reservation.submit_hash
        {
            return Err(JournalError::Corrupt);
        }
        let intent_file = self
            .read_file(&job, "intent.json", 256 * 1024)?
            .ok_or(JournalError::Interrupted)?;
        self.0.control.record(intent_file.bytes())?;
        let intent = ExecutionIntentV1::from_bytes(intent_file.bytes())
            .map_err(|_| JournalError::Corrupt)?;
        let intent_hash = intent.intent_hash().map_err(|_| JournalError::Corrupt)?;
        if intent.operation_id() != operation_id
            || accepted.intent_hash != intent_hash
            || intent
                .canonical_bytes()
                .map_err(|_| JournalError::Corrupt)?
                != intent_file.bytes()
        {
            return Err(JournalError::Corrupt);
        }
        io(intent_file.persistent_sync())?;
        let binding_file = self
            .read_file(&job, "bindings.json", SMALL)?
            .ok_or(JournalError::Interrupted)?;
        self.0.control.record(binding_file.bytes())?;
        let bindings: Bindings = parse(binding_file.bytes())?;
        if bindings.version != accepted.version
            || bindings.operation_id != operation_id
            || bindings.intent_hash != intent_hash
            || accepted.bindings_hash != bindings.hash(binding_file.bytes())?
            || canonical(&bindings)? != binding_file.bytes()
        {
            return Err(JournalError::Corrupt);
        }
        bindings.validate_launch(accepted.version, &self.0.scope_hash, operation_id, &intent)?;
        io(binding_file.persistent_sync())?;
        let source = self.load_binding(&job, &intent, &bindings.binding)?;
        let launch_profile = bindings
            .docker_launch_profile
            .as_ref()
            .map(|binding| {
                fscheck(RetainedDockerLaunchProfile::load(
                    &job,
                    binding,
                    &self.0.exclusions.0,
                ))
            })
            .transpose()?;
        if bindings.docker_launch_profile.is_none()
            && fscheck(job.persistent_child(crate::runtime::launch_profile::DIRECTORY, false))?
                .is_some()
        {
            return Err(JournalError::Corrupt);
        }
        let current: Current = self.read(&job, "current.json", SMALL)?;
        if current.version != 1
            || current.operation_id != operation_id
            || current.intent_hash != intent_hash
            || current.head.revision >= u64::from(self.0.limits.revisions)
        {
            return Err(JournalError::Corrupt);
        }
        valid_hash(&current.head.hash).map_err(|_| JournalError::Corrupt)?;
        let mut verifier = confirmed::ChainVerifierV1::new_with_launch_profile(
            &self.0.scope,
            &self.0.control,
            &intent,
            &bindings.binding,
            &accepted.initial_checkpoint,
            bindings.docker_launch_profile.as_ref(),
        )?;
        for revision in 0..=current.head.revision {
            let file = self
                .read_file(&job, &revision_name(revision), CHECKPOINT)?
                .ok_or(JournalError::Interrupted)?;
            verifier.push(revision, file.bytes())?;
            #[cfg(test)]
            cost_measurement::verified(self.0.directory.path(), file.bytes().len());
            io(file.persistent_sync())?;
        }
        let confirmed::ConfirmedChainV1 {
            payload,
            legacy_managed_seen,
            chain,
            command_receipts,
            cancel_boundaries: _,
            outbox_receipts,
            last_cancel_operations,
        } = verifier.finish(&current.head)?;
        if let Some(floor) = writer.floors.get(&operation_id) {
            let index = usize::try_from(floor.revision).map_err(|_| JournalError::Corrupt)?;
            if chain.get(index) != Some(floor) {
                return Err(JournalError::Conflict);
            }
        }
        if let Some(profile) = &launch_profile {
            fscheck(profile.revalidate())?;
        }
        io(job.sync())?;
        io(self.0.jobs.sync())?;
        io(self.0.directory.sync())?;
        writer.floors.insert(operation_id, current.head.clone());
        writer.observed_jobs.insert(operation_id);
        #[cfg(test)]
        cost_measurement::load(self.0.directory.path(), operation_id, true);
        Ok(Some(AcceptedJobV1(Box::new(AcceptedData {
            scope: self.0.scope.clone(),
            operation_id,
            intent,
            intent_hash,
            submit_hash: accepted.submit_hash,
            binding: bindings.binding,
            source,
            launch_binding: bindings.docker_launch_profile,
            launch_profile,
            head: current.head,
            checkpoint: payload,
            legacy_managed_seen,
            chain,
            command_receipts,
            outbox_receipts,
            last_cancel_operations,
        }))))
    }
    fn check_writer(&self) -> JournalResult<()> {
        let result = self.check_writer_inner();
        self.note_status_failure(&result);
        result
    }
    pub(in crate::broker) fn status_projection_limits(&self) -> (u32, u64) {
        (self.0.limits.jobs, self.0.limits.scope_bytes)
    }
    pub(in crate::broker) fn status_projection_failed(&self) -> bool {
        self.0
            .status_observation_failed
            .load(std::sync::atomic::Ordering::SeqCst)
    }
    fn note_status_failure<T>(&self, result: &JournalResult<T>) {
        // Shared with the STOP reader's same retained core. This is not an
        // expected mutation CAS conflict and grants no journal authority.
        note_status_read_failure(&self.0, result);
    }
    fn check_writer_inner(&self) -> JournalResult<()> {
        fscheck(self.0.lock.persistent_revalidate())?;
        fscheck(self.0.directory.revalidate())?;
        let scope = self
            .read_file(&self.0.directory, "scope.json", SMALL)?
            .ok_or(JournalError::Corrupt)?;
        if scope.bytes() != canonical(&self.0.scope)? {
            return Err(JournalError::Conflict);
        }
        self.0.control.record(scope.bytes())
    }
    fn checked_record(&self, value: &impl Serialize, max: u64) -> JournalResult<Vec<u8>> {
        checked_core_record(&self.0, value, max)
    }
    fn write(&self, directory: &Directory, name: &str, bytes: &[u8]) -> JournalResult<()> {
        let file = io(directory.persistent_create(name, bytes))?;
        io(file.persistent_sync())?;
        io(directory.sync())
    }
    fn read_file(
        &self,
        directory: &Directory,
        name: &str,
        max: u64,
    ) -> JournalResult<Option<PrivateFile>> {
        fscheck(directory.persistent_read(name, max, &self.0.exclusions.0))
    }
    fn read<T: DeserializeOwned>(
        &self,
        directory: &Directory,
        name: &str,
        max: u64,
    ) -> JournalResult<T> {
        let file = self
            .read_file(directory, name, max)?
            .ok_or(JournalError::Interrupted)?;
        self.0.control.record(file.bytes())?;
        let value = parse(file.bytes())?;
        io(file.persistent_sync())?;
        Ok(value)
    }
    fn read_canonical<T: DeserializeOwned + Serialize>(
        &self,
        directory: &Directory,
        name: &str,
        max: u64,
    ) -> JournalResult<T> {
        let file = self
            .read_file(directory, name, max)?
            .ok_or(JournalError::Interrupted)?;
        self.0.control.record(file.bytes())?;
        let value: T = parse(file.bytes())?;
        if canonical(&value)? != file.bytes() {
            return Err(JournalError::Corrupt);
        }
        io(file.persistent_sync())?;
        Ok(value)
    }
    fn job(&self, operation: Uuid) -> JournalResult<Directory> {
        fscheck(self.0.jobs.persistent_child(&operation.to_string(), false))?
            .ok_or(JournalError::Interrupted)
    }
    fn check_reservation(&self, reservation: &Reservation, operation: Uuid) -> JournalResult<()> {
        if reservation.version != 1
            || reservation.scope_hash != self.0.scope_hash
            || reservation.operation_id != operation
        {
            return Err(JournalError::Corrupt);
        }
        valid_hash(&reservation.submit_hash).map_err(|_| JournalError::Corrupt)
    }
    fn context(
        &self,
        checkpoint: &CheckpointPayloadV1,
        intent: &ExecutionIntentV1,
        binding: &BindingManifestV1,
    ) -> JournalResult<()> {
        confirmed::context(&self.0.scope, &self.0.control, checkpoint, intent, binding)
    }
    fn check_transition(
        &self,
        previous: &LoadedCheckpoint,
        next: &LoadedCheckpoint,
        legacy_managed_seen: bool,
        last_cancel_operations: Option<usize>,
    ) -> JournalResult<()> {
        confirmed::check_transition(previous, next, legacy_managed_seen, last_cancel_operations)
    }
    fn budget(&self, name: &str, bytes: u64, entries: u32, writer: &Writer) -> JournalResult<()> {
        let mut usage = self.usage(Some(name))?;
        usage.add(bytes, entries, true, &self.0.limits)?;
        let operation = Uuid::parse_str(name).map_err(|_| JournalError::InvalidInput)?;
        retirement::reserve_future(
            &self.0,
            operation,
            None,
            &usage,
            &retirement::PublicationBudget::NewAllocation,
            &writer.floors,
        )
    }
    fn check_observation_capacity(&self, operation_id: Uuid, writer: &Writer) -> JournalResult<()> {
        if !writer.observed_jobs.contains(&operation_id)
            && writer.observed_jobs.len() >= self.0.limits.jobs as usize
        {
            return Err(JournalError::LimitExceeded);
        }
        Ok(())
    }
    fn usage(&self, selected: Option<&str>) -> JournalResult<Usage> {
        #[cfg(test)]
        cost_measurement::usage(self.0.directory.path());
        let mut usage = Usage::default();
        #[cfg(test)]
        cost_measurement::inventory(
            self.0.directory.path(),
            cost_measurement::InventoryKind::Scope,
            false,
        );
        for entry in fscheck(
            self.0
                .directory
                .persistent_entries(self.0.limits.scope_entries),
        )? {
            #[cfg(test)]
            cost_measurement::inventory(
                self.0.directory.path(),
                cost_measurement::InventoryKind::Scope,
                true,
            );
            if !matches!(entry.name.as_str(), ".writer.lock" | "scope.json" | "jobs")
                || entry.directory != (entry.name == "jobs")
            {
                return Err(JournalError::Corrupt);
            }
            usage.add(entry.bytes, 1, false, &self.0.limits)?;
        }
        #[cfg(test)]
        cost_measurement::inventory(
            self.0.directory.path(),
            cost_measurement::InventoryKind::Jobs,
            false,
        );
        for entry in fscheck(self.0.jobs.persistent_entries(self.0.limits.scope_entries))? {
            #[cfg(test)]
            cost_measurement::inventory(
                self.0.directory.path(),
                cost_measurement::InventoryKind::Jobs,
                true,
            );
            let id = Uuid::parse_str(&entry.name).map_err(|_| JournalError::Corrupt)?;
            if !entry.directory || id.is_nil() || id.to_string() != entry.name {
                return Err(JournalError::Corrupt);
            }
            usage.jobs += 1;
            let active = selected == Some(entry.name.as_str());
            usage.add(0, 1, active, &self.0.limits)?;
            let directory = fscheck(self.0.jobs.persistent_child(&entry.name, false))?
                .ok_or(JournalError::Corrupt)?;
            let mut bytes = 0u64;
            let mut count = 1u32;
            let mut revisions = 0u32;
            #[cfg(test)]
            cost_measurement::inventory(
                self.0.directory.path(),
                cost_measurement::InventoryKind::JobFiles,
                false,
            );
            for file in fscheck(directory.persistent_entries(self.0.limits.job_entries))? {
                #[cfg(test)]
                cost_measurement::inventory(
                    self.0.directory.path(),
                    cost_measurement::InventoryKind::JobFiles,
                    true,
                );
                if file.directory {
                    if file.name != "bindings"
                        && file.name != crate::runtime::launch_profile::DIRECTORY
                    {
                        return Err(JournalError::Corrupt);
                    }
                    let bindings = fscheck(directory.persistent_child(&file.name, false))?
                        .ok_or(JournalError::Corrupt)?;
                    #[cfg(test)]
                    cost_measurement::inventory(
                        self.0.directory.path(),
                        cost_measurement::InventoryKind::Bindings,
                        false,
                    );
                    for binding in fscheck(bindings.persistent_entries(self.0.limits.job_entries))?
                    {
                        #[cfg(test)]
                        cost_measurement::inventory(
                            self.0.directory.path(),
                            cost_measurement::InventoryKind::Bindings,
                            true,
                        );
                        if binding.directory
                            || !(if file.name == crate::runtime::launch_profile::DIRECTORY {
                                binding.name == crate::runtime::launch_profile::FILE
                            } else {
                                binding.name == "manual-env-v1.bin"
                                    || binding.name.strip_suffix(".json").is_some_and(|id| {
                                        Uuid::parse_str(id).is_ok_and(|uuid| {
                                            !uuid.is_nil() && uuid.to_string() == id
                                        })
                                    })
                            })
                        {
                            return Err(JournalError::Corrupt);
                        }
                        bytes = bytes
                            .checked_add(binding.bytes)
                            .ok_or(JournalError::LimitExceeded)?;
                        count += 1;
                        usage.add(binding.bytes, 1, active, &self.0.limits)?;
                    }
                } else if let Some(number) = file
                    .name
                    .strip_prefix("rev-")
                    .and_then(|name| name.strip_suffix(".json"))
                {
                    if number.len() != 20 || !number.bytes().all(|b| b.is_ascii_digit()) {
                        return Err(JournalError::Corrupt);
                    }
                    revisions += 1;
                } else if !matches!(
                    file.name.as_str(),
                    "reservation.json"
                        | "intent.json"
                        | "bindings.json"
                        | "current.json"
                        | "current-next.json"
                        | "accepted.json"
                        | "rejected.json"
                ) {
                    return Err(JournalError::Corrupt);
                }
                bytes = bytes
                    .checked_add(file.bytes)
                    .ok_or(JournalError::LimitExceeded)?;
                count += 1;
                usage.add(file.bytes, 1, active, &self.0.limits)?;
            }
            if bytes > self.0.limits.job_bytes
                || count > self.0.limits.job_entries
                || revisions > self.0.limits.revisions
            {
                return Err(JournalError::LimitExceeded);
            }
        }
        Ok(usage)
    }
}

fn parse<T: DeserializeOwned>(bytes: &[u8]) -> JournalResult<T> {
    serde_json::from_slice(bytes).map_err(|_| JournalError::Corrupt)
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FaultStage {
    WriterLockAbsent,
    NamespaceDurable,
    ReservationWritten,
    ReservationDurable,
    BindingWritten,
    BindingDurable,
    IntentDurable,
    ManifestDurable,
    RevisionWritten,
    RevisionDurable,
    CurrentWritten,
    CurrentPublished,
    CurrentDurable,
    AcceptedWritten,
    AcceptedDurable,
    RejectedWritten,
    RejectedDurable,
    BeforeAck,
}
#[cfg(test)]
mod launch_profile_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(super) use tests::CurrentDurableObserver;
