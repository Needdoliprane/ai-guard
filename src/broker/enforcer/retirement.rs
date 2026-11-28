//! Live retirement authority. Durable records describe facts; only the owner
//! of an original actor and its actual native JoinHandle can create witnesses.
use super::native_stop_failure::{CauseTarget, FailureFacts, NativeStopFailures};
use super::*;
use super::{
    ledger::{ClosingCommitted, LedgerCoverageV1, RetirementLedger, TombstoneCommitted},
    recovery::{CancelReferenceV1, RecoveryAttemptOwner},
    server::lifetime::{ActorKey, NativeOwners},
};
use crate::{
    broker::{
        clock::{HostClock, NativeHostClock},
        retirement::*,
        retirement_checkpoint::Full3RegistrationBindingV1,
    },
    runtime::ManagedRuntime,
};
use serde::de::Error as _;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Condvar,
        atomic::{AtomicBool, AtomicU8},
    },
    time::Instant,
};
use tokio::sync::watch;

#[path = "retirement_prospective.rs"]
mod prospective;
use prospective::ProspectiveActor;

/// Explicit new sealed authority. The child creates this root itself and is
/// never handed a reconstructible creation capability through argv.
pub async fn run_internal_full3_fresh(
    parent: &crate::control_fs::Directory,
    name: &str,
    installation_id: Uuid,
    max_slots: u32,
) -> EnforcerResult<()> {
    run_internal_full3_fresh_with_budget(parent, name, installation_id, max_slots, 0).await
}

pub async fn run_internal_full3_fresh_with_budget(
    parent: &crate::control_fs::Directory,
    name: &str,
    installation_id: Uuid,
    max_slots: u32,
    max_recovery_stop_owners: u32,
) -> EnforcerResult<()> {
    if !(1..=MAX_SLOTS).contains(&max_slots)
        || max_recovery_stop_owners > MAX_SLOTS
        || installation_id.is_nil()
    {
        return Err(EnforcerError::InvalidInput);
    }
    let parent = parent.clone();
    let name = name.to_owned();
    let ledger = tokio::task::spawn_blocking(move || {
        let fresh = parent
            .create_enforcer_root(&name)
            .map_err(|_| EnforcerError::UnsafeFilesystem)?;
        let ledger = RetirementLedger::initialize_new(fresh, installation_id)?;
        // Only the successful exclusive fresh factory is allowed to create
        // auth. Existing roots always require the retained original bytes.
        capability(ledger.lease().ledger_root(), true)?;
        Ok::<_, EnforcerError>(Arc::new(ledger))
    })
    .await
    .map_err(|_| EnforcerError::Unavailable)??;
    super::recovery::run_full_listener_with_budget(ledger, max_slots, max_recovery_stop_owners)
        .await
}

pub async fn run_internal_full3_existing(
    root: &crate::control_fs::Directory,
    max_slots: u32,
) -> EnforcerResult<()> {
    run_internal_full3_existing_with_budget(root, max_slots, 0).await
}

/// The recovery budget is distinct from the unchanged Original capacity.
/// Existing roots still enter through the restrictive native recovery role.
pub async fn run_internal_full3_existing_with_budget(
    root: &crate::control_fs::Directory,
    max_slots: u32,
    max_recovery_stop_owners: u32,
) -> EnforcerResult<()> {
    if !(1..=MAX_SLOTS).contains(&max_slots) || max_recovery_stop_owners > MAX_SLOTS {
        return Err(EnforcerError::InvalidInput);
    }
    super::recovery::run_full_existing_listener_with_budget(
        root,
        max_slots,
        max_recovery_stop_owners,
    )
    .await
}

fn data<T>(result: crate::broker::journal::JournalResult<T>) -> EnforcerResult<T> {
    result.map_err(|_| EnforcerError::InvalidInput)
}

struct ActorState {
    status: EnforcerSlotStatusV1,
    gate: OriginalGate,
    durable_stop: Option<(u64, CancelReferenceV1)>,
    retained_terminal: RetainedTerminalObservation,
}

#[derive(Default)]
enum RetainedTerminalObservation {
    #[default]
    Pending,
    Observed(Box<ContainerObservation>),
    Conflicting,
}

impl ActorState {
    fn retain_terminal_candidate(
        &mut self,
        captured_stop: Option<&(u64, CancelReferenceV1)>,
        observation: &ContainerObservation,
        registration: &RegistrationV1,
    ) {
        if !captured_stop.is_some_and(|captured| {
            captured.0 == self.status.generation && self.durable_stop.as_ref() == Some(captured)
        }) {
            return;
        }
        let terminal = observation
            .validate_terminal(registration.authority.kind, &registration.owned.observation)
            .is_ok();
        match &self.retained_terminal {
            RetainedTerminalObservation::Pending if terminal => {
                self.retained_terminal =
                    RetainedTerminalObservation::Observed(Box::new(observation.clone()));
            }
            RetainedTerminalObservation::Observed(previous)
                if !terminal
                    || previous.process.started_at != observation.process.started_at
                    || previous.process.finished_at != observation.process.finished_at
                    || previous.process.exit_code != observation.process.exit_code =>
            {
                self.retained_terminal = RetainedTerminalObservation::Conflicting;
            }
            _ => {}
        }
    }
}
struct FullActor {
    registration: RegistrationV1,
    digest: String,
    key: ActorKey,
    original_registration_nonce: Uuid,
    state: Mutex<ActorState>,
    wake: Condvar,
    // Only T permits the original thread to return normally. Closing alone
    // keeps the thread and lease retained if durable publication is unknown.
    exit: AtomicBool,
    // Published only by the real native thread, after NativeOwners retained
    // its handle and this thread constructed its own runtime context.
    runtime_installed: Arc<AtomicBool>,
    failures: Arc<NativeStopFailures>,
    // Retained native owner handle; read by the pending Full recovery wiring.
    #[allow(dead_code)]
    owners: NativeOwners,
}
type FinalResult = EnforcerResult<(RetiredReceiptV1, LedgerHeadV1)>;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RegistryOwner {
    Full,
    Restored,
    Prospective,
}
const FRESH_ADMISSION: u8 = 0;
const RECOVERY_INTENT: u8 = 1;
const IDLE_CLOSING: u8 = 2;

/// The admission policy belongs to the same native owner as its retained
/// registrations. Every Full view, including one published after a scan,
/// receives this one registry. Constructing a view cannot reopen admission.
pub(super) struct NativeAdmissionRegistry {
    entries: Mutex<BTreeMap<String, RegistryOwner>>,
    phase: AtomicU8,
    // Written only while entries is locked. Zero is the exclusive fresh
    // generation; this protocol has no transition back from recovery.
    recovery_generation: AtomicU64,
}
pub(super) type AdmissionRegistry = Arc<NativeAdmissionRegistry>;

impl NativeAdmissionRegistry {
    /// Only the exclusive fresh-root factory may select this constructor.
    pub(super) fn fresh() -> AdmissionRegistry {
        Arc::new(Self {
            entries: Mutex::new(BTreeMap::new()),
            phase: AtomicU8::new(FRESH_ADMISSION),
            recovery_generation: AtomicU64::new(0),
        })
    }

    /// Existing/cold roles start closed before reading any recovery source.
    pub(super) fn recovery_fenced() -> AdmissionRegistry {
        Arc::new(Self {
            entries: Mutex::new(BTreeMap::new()),
            phase: AtomicU8::new(RECOVERY_INTENT),
            recovery_generation: AtomicU64::new(1),
        })
    }

    pub(super) fn lock(
        &self,
    ) -> std::sync::LockResult<std::sync::MutexGuard<'_, BTreeMap<String, RegistryOwner>>> {
        self.entries.lock()
    }

    #[cfg(test)]
    pub(super) fn begin_recovery(&self) -> EnforcerResult<u64> {
        self.publish_recovery_intent()?;
        self.commit_recovery()
    }

    /// Restrictive intent still precedes waiting on the admission mutex. An
    /// idle closure and a first STOP intent contend for the same transition.
    pub(super) fn publish_recovery_intent(&self) -> EnforcerResult<()> {
        match self.phase.compare_exchange(
            FRESH_ADMISSION,
            RECOVERY_INTENT,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) | Err(RECOVERY_INTENT) => Ok(()),
            Err(_) => Err(EnforcerError::Unhealthy),
        }
    }

    pub(super) fn commit_recovery(&self) -> EnforcerResult<u64> {
        let _admitted = self.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if !self.fenced() {
            return Err(EnforcerError::Unhealthy);
        }
        let current = self.recovery_generation.load(Ordering::SeqCst);
        let generation = if current == 0 { 1 } else { current };
        self.recovery_generation.store(generation, Ordering::SeqCst);
        Ok(generation)
    }

    pub(super) fn fenced(&self) -> bool {
        self.phase.load(Ordering::SeqCst) == RECOVERY_INTENT
    }

    /// The caller holds this registry's empty admission map. A recovery
    /// intent already published outside that lock always defeats closure.
    fn close_fresh_idle(&self) -> EnforcerResult<()> {
        self.phase
            .compare_exchange(
                FRESH_ADMISSION,
                IDLE_CLOSING,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .map(|_| ())
            .map_err(|_| EnforcerError::NotIdle)
    }

    pub(super) fn recovery_generation(&self) -> Option<u64> {
        match self.recovery_generation.load(Ordering::SeqCst) {
            0 => None,
            generation => Some(generation),
        }
    }

    /// Caller holds this registry's admission lock through the mutation or
    /// grant. A failed/poisoned gate never yields a new Original or Prepare.
    fn check_original(&self) -> EnforcerResult<()> {
        if self.phase.load(Ordering::SeqCst) != FRESH_ADMISSION {
            return Err(EnforcerError::NotReady);
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum FullCommand {
    Status {},
    Watch {},
    ShutdownIfIdle {},
    Slots {
        arm_ids: Vec<String>,
    },
    Register {
        registration: Box<RegistrationV1>,
        generation: u64,
        observe_only: bool,
    },
    ReserveProspective {
        prospective: Box<ProspectiveRegistrationV1>,
        generation: u64,
        replay_only: bool,
    },
    BeginProspectiveArmExchange {
        reservation: ProspectiveReservationV1,
        generation: u64,
    },
    BindProspective {
        reservation: ProspectiveReservationV1,
        registration: Box<RegistrationV1>,
        generation: u64,
        observe_only: bool,
        #[serde(deserialize_with = "strict_optional_uuid")]
        original_exchange_nonce: Option<Uuid>,
    },
    Prepare {
        arm_id: String,
        registration_hash: RetirementDigestV1,
        generation: u64,
    },
    Cancel {
        arm_id: String,
        registration_hash: RetirementDigestV1,
        generation: u64,
    },
    RevokeUnused {
        arm_id: String,
        registration_hash: RetirementDigestV1,
        generation: u64,
        #[serde(deserialize_with = "crate::broker::retirement::strict_uuid")]
        actor_nonce: Uuid,
        #[serde(deserialize_with = "crate::broker::retirement::strict_uuid")]
        original_registration_nonce: Uuid,
        #[serde(deserialize_with = "crate::broker::retirement::strict_uuid")]
        retirement_id: Uuid,
    },
    RetireUnused {
        identity: Box<RetirementIdentityV1>,
        evidence: RetirementEvidenceV1,
        audit_ack: RetirementAuditAckV1,
    },
    QueryRetirement {
        identity: Box<RetirementIdentityV1>,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum FullReply {
    Status {
        value: EnforcerStatusV1,
    },
    Slots {
        values: Vec<EnforcerSlotStatusV1>,
    },
    Registered {
        value: Box<EnforcerSlotStatusV1>,
        replayed: bool,
        binding: Full3RegistrationBindingV1,
        #[serde(deserialize_with = "crate::broker::retirement::strict_uuid")]
        actor_nonce: Uuid,
        #[serde(deserialize_with = "strict_optional_uuid")]
        original_registration_nonce: Option<Uuid>,
    },
    ProspectiveReserved {
        reservation: ProspectiveReservationV1,
    },
    ProspectiveArmExchange {
        #[serde(deserialize_with = "strict_optional_uuid")]
        original_exchange_nonce: Option<Uuid>,
    },
    Prepared {
        value: Box<EnforcerSlotStatusV1>,
    },
    Revoked {
        #[serde(deserialize_with = "crate::broker::retirement::strict_uuid")]
        revocation_nonce: Uuid,
    },
    Retirement {
        receipt: Option<Box<RetiredReceiptV1>>,
        head: LedgerHeadV1,
        accepted: Option<Box<ClosingAcceptanceV1>>,
    },
    Acknowledged {},
    Error {
        code: EnforcerError,
    },
}
fn strict_optional_uuid<'de, D: Deserializer<'de>>(decoder: D) -> Result<Option<Uuid>, D::Error> {
    let text = Option::<String>::deserialize(decoder)?;
    text.map(|text| {
        let id = Uuid::parse_str(&text).map_err(D::Error::custom)?;
        if id.is_nil() || id.to_string() != text {
            return Err(D::Error::custom("noncanonical original registration nonce"));
        }
        Ok(id)
    })
    .transpose()
}
struct RetiringOperation {
    identity_hash: RetirementDigestV1,
    receiver: watch::Receiver<Option<FinalResult>>,
}
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct RetirementOperationKey {
    scope: Vec<u8>,
    job_id: Uuid,
    retirement_id: Uuid,
}
impl RetirementOperationKey {
    fn from_identity(identity: &RetirementIdentityV1) -> EnforcerResult<Self> {
        Ok(Self {
            scope: data(crate::broker::journal::canonical(identity.scope()))?,
            job_id: *identity.job_operation_id(),
            retirement_id: *identity.retirement_id(),
        })
    }
}
struct RetiringActor {
    actor: Arc<FullActor>,
    accepted: ClosingAcceptanceV1,
}
pub(super) struct FullState {
    pub(super) epoch: Uuid,
    pub(super) ledger: Arc<RetirementLedger>,
    owners: NativeOwners,
    joiner: Mutex<Option<std::thread::JoinHandle<EnforcerResult<()>>>>,
    max_slots: u32,
    admitted: AdmissionRegistry,
    clock: Option<Arc<NativeHostClock>>,
    actors: Mutex<BTreeMap<Uuid, Arc<FullActor>>>,
    // These are actual pre-Arm native owners, charged to S before the RPC.
    // A bound entry retains its original cell until the real native Q path.
    prospective: Mutex<BTreeMap<Uuid, Arc<ProspectiveActor>>>,
    retiring: Mutex<BTreeMap<RetirementOperationKey, RetiringOperation>>,
    closed_arms: Mutex<BTreeSet<Uuid>>,
    closed: AtomicBool,
    // Separate from ledger/Full failure: recovery STOP remains available on
    // those failures. Read and close this fence only while admitted is held.
    idle_admission_closed: AtomicBool,
    io: Arc<tokio::sync::Semaphore>,
}

impl FullState {
    pub(super) fn failure_facts(&self) -> FailureFacts {
        let mut facts = FailureFacts::default();
        let mut seen = BTreeSet::new();
        let actors = self.actors.lock().unwrap_or_else(|p| p.into_inner());
        for actor in actors.values() {
            if seen.insert(Arc::as_ptr(&actor.failures) as usize) {
                facts.merge(actor.failures.facts());
            }
        }
        drop(actors);
        let prospective = self.prospective.lock().unwrap_or_else(|p| p.into_inner());
        for actor in prospective.values() {
            if seen.insert(Arc::as_ptr(&actor.failures) as usize) {
                facts.merge(actor.failures.facts());
            }
        }
        facts
    }
    /// Called after committing the shared recovery fence and before its ACK.
    /// This retains every pre-fence admission, including a Register waiting
    /// for native Ready. Closing keeps its real continuation; it is not turned
    /// into a STOP/absence/join proof. No ledger or runtime IO is performed.
    pub(super) fn reserve_recovery_stops(&self) -> EnforcerResult<()> {
        let mut wakes = Vec::new();
        let mut pending_wakes = Vec::new();
        {
            let _admitted = self.admitted.lock().map_err(|_| EnforcerError::Unhealthy)?;
            if self.admitted.recovery_generation().is_none() {
                return Err(EnforcerError::NotReady);
            }
            let actors = self.actors.lock().map_err(|_| EnforcerError::Unhealthy)?;
            for actor in actors.values() {
                let mut state = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
                if !state.gate.is_closing() {
                    state.gate.reserve_stop()?;
                    state.status.phase = SlotPhaseV1::Stopping;
                    wakes.push(Arc::clone(actor));
                }
            }
            let pending = self
                .prospective
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            for actor in pending.values() {
                actor.reserve_stop()?;
                pending_wakes.push(Arc::clone(actor));
            }
        }
        for actor in wakes {
            actor.wake.notify_all();
        }
        for actor in pending_wakes {
            actor.wake();
        }
        Ok(())
    }

    async fn ledger_io<T, F>(self: &Arc<Self>, promised: bool, operation: F) -> EnforcerResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&RetirementLedger) -> EnforcerResult<T> + Send + 'static,
    {
        let permit = if promised {
            Arc::clone(&self.io)
                .acquire_owned()
                .await
                .map_err(|_| EnforcerError::Unhealthy)?
        } else {
            Arc::clone(&self.io)
                .try_acquire_owned()
                .map_err(|_| EnforcerError::Unavailable)?
        };
        let ledger = Arc::clone(&self.ledger);
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            operation(&ledger)
        })
        .await
        .map_err(|_| EnforcerError::Unhealthy)?
    }
    pub(super) async fn finish_idle(self: &Arc<Self>) -> EnforcerResult<()> {
        self.complete().await?;
        {
            let admitted = self.admitted.lock().map_err(|_| EnforcerError::Unhealthy)?;
            if !admitted.is_empty() {
                return Err(EnforcerError::NotIdle);
            }
            self.seal_idle_admission();
        }
        self.join_idle().await
    }

    /// The RecoveryState already checked ledger coverage and retains the
    /// actual W owner. Serialize both closures with admitted Full and STOP work.
    pub(super) fn reserve_idle_with_recovery(
        &self,
        attempt: &RecoveryAttemptOwner,
    ) -> EnforcerResult<()> {
        let admitted = self.admitted.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if self.closed.load(Ordering::SeqCst) || self.owners.failed() {
            return Err(EnforcerError::Unhealthy);
        }
        if !admitted.is_empty() {
            return Err(EnforcerError::NotIdle);
        }
        attempt.check_sealed_idle()?;
        self.admitted.close_fresh_idle()?;
        self.seal_idle_admission();
        attempt.seal_idle();
        Ok(())
    }

    fn seal_idle_admission(&self) {
        self.idle_admission_closed.store(true, Ordering::SeqCst);
        self.closed.store(true, Ordering::SeqCst);
    }

    pub(super) async fn join_idle(&self) -> EnforcerResult<()> {
        let joiner = self
            .joiner
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .take()
            .ok_or(EnforcerError::Conflict)?;
        self.owners.finish_sealed_idle(joiner).await
    }
    // The caller holds the shared admission mutex across this check and any
    // insertion. finish_idle closes the monotone fence under that same mutex.
    pub(super) fn check_new_stop_admission(&self) -> EnforcerResult<()> {
        if self.idle_admission_closed.load(Ordering::SeqCst) {
            return Err(EnforcerError::Unhealthy);
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) async fn finish_empty_fixture_support(&self) -> EnforcerResult<()> {
        // Test teardown joins the actual support owner after a real ledger
        // failure. It creates no retirement witness and never reopens Full.
        let joiner = {
            let admitted = self.admitted.lock().map_err(|_| EnforcerError::Unhealthy)?;
            let actors = self.actors.lock().map_err(|_| EnforcerError::Unhealthy)?;
            let retiring = self.retiring.lock().map_err(|_| EnforcerError::Unhealthy)?;
            if !self.closed.load(Ordering::SeqCst)
                || !admitted.is_empty()
                || !actors.is_empty()
                || !retiring.is_empty()
            {
                return Err(EnforcerError::NotIdle);
            }
            self.joiner
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .take()
                .ok_or(EnforcerError::Conflict)?
        };
        self.owners.finish_sealed_idle(joiner).await
    }
    pub(super) fn restore_existing(
        &self,
        registration: &RegistrationV1,
        generation: u64,
    ) -> EnforcerResult<Option<EnforcerSlotStatusV1>> {
        let arm_id = Uuid::parse_str(&registration.arm.operation_id)
            .map_err(|_| EnforcerError::InvalidInput)?;
        let closed = self
            .closed_arms
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        if closed.contains(&arm_id) {
            return Err(EnforcerError::Conflict);
        }
        let actors = self.actors.lock().map_err(|_| EnforcerError::Unhealthy)?;
        let Some(actor) = actors.get(&arm_id) else {
            return Ok(None);
        };
        if actor.digest != registration.digest()? {
            return Err(EnforcerError::Conflict);
        }
        let mut state = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if state.gate.is_closing() {
            return Err(EnforcerError::Conflict);
        }
        state.gate.restore();
        state.status.prepared_once = true;
        state.status.generation = state.status.generation.max(generation);
        Ok(Some(state.status.clone()))
    }
    pub(super) fn slot_snapshot(
        &self,
        arm_id: &str,
        digest: &str,
    ) -> EnforcerResult<EnforcerSlotStatusV1> {
        let actor = self.actor(arm_id, digest)?;
        let state = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        Ok(state.status.clone())
    }

    /// Return a native observation only if the actor began that inspection
    /// after taking ownership of this exact durable STOP.
    pub(super) fn terminal_after_durable_stop(
        &self,
        arm_id: &str,
        digest: &str,
        cancel: &CancelReferenceV1,
    ) -> EnforcerResult<EnforcerSlotStatusV1> {
        if self.admitted.recovery_generation().is_none() {
            return Err(EnforcerError::NotReady);
        }
        let actor = self.actor(arm_id, digest)?;
        if actor.key.epoch != self.epoch
            || !matches!(&actor.registration.arm.subject, ArmSubjectV1::Worker)
        {
            return Err(EnforcerError::Conflict);
        }
        let state = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if state.gate.is_closing()
            || !state.gate.stop_reserved
            || self.owners.failed()
            || !actor.runtime_installed.load(Ordering::SeqCst)
            || state
                .durable_stop
                .as_ref()
                .is_none_or(|(generation, retained)| {
                    *generation != state.status.generation || retained != cancel
                })
        {
            return Err(EnforcerError::NotReady);
        }
        let observation = match &state.retained_terminal {
            RetainedTerminalObservation::Observed(observation) => observation.as_ref(),
            RetainedTerminalObservation::Pending => return Err(EnforcerError::NotReady),
            RetainedTerminalObservation::Conflicting => return Err(EnforcerError::Conflict),
        };
        let mut value = state.status.clone();
        value.observation = Some(observation.clone());
        Ok(value)
    }

    /// Positive installation evidence comes from the retained native worker,
    /// never from Ready/status, a digest, a semaphore or a spawned task alone.
    /// The receipt says nothing about a runtime effect or an old start drain.
    pub(super) fn native_stop_installed(&self, arm_id: &str, digest: &str) -> EnforcerResult<()> {
        let actor = self.actor(arm_id, digest)?;
        let state = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if state.gate.is_closing()
            || self.owners.failed()
            || !actor.runtime_installed.load(Ordering::SeqCst)
        {
            return Err(EnforcerError::NotReady);
        }
        Ok(())
    }
    pub(super) fn check_restore(&self, arm_id: &str) -> EnforcerResult<()> {
        let arm_id = Uuid::parse_str(arm_id).map_err(|_| EnforcerError::InvalidInput)?;
        if self
            .closed_arms
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .contains(&arm_id)
        {
            return Err(EnforcerError::Conflict);
        }
        Ok(())
    }
    async fn retire(
        self: &Arc<Self>,
        identity: RetirementIdentityV1,
        evidence: RetirementEvidenceV1,
        audit_ack: RetirementAuditAckV1,
    ) -> FinalResult {
        let head = self.complete().await?;
        data(identity.validate())?;
        data(evidence.validate())?;
        data(audit_ack.validate())?;
        let hash = data(identity.digest())?;
        if evidence.identity_hash() != &hash
            || identity.scope().installation_id() != *head.ledger().installation_id()
        {
            return Err(EnforcerError::Conflict);
        }
        // A Q replay never starts a thread or asks an actor to close again.
        let query_identity = identity.clone();
        if let Some(entry) = self
            .ledger_io(false, move |ledger| ledger.query(&query_identity))
            .await?
            && let RetirementLedgerPhaseV1::Final { receipt, .. } = entry.phase()
        {
            return Ok((receipt.as_ref().clone(), head));
        }
        let mut receiver = {
            let operation_key = RetirementOperationKey::from_identity(&identity)?;
            let mut operations = self.retiring.lock().map_err(|_| EnforcerError::Unhealthy)?;
            if let Some(operation) = operations.get(&operation_key) {
                if operation.identity_hash != hash {
                    return Err(EnforcerError::Conflict);
                }
                operation.receiver.clone()
            } else {
                let actor = self.actor(
                    &identity.arm().operation_id,
                    identity.registration_hash().as_str(),
                )?;
                let accepted = data(ClosingAcceptanceV1::new(
                    identity.clone(),
                    evidence.clone(),
                    audit_ack,
                    Uuid::new_v4(),
                ))?;
                {
                    let mut closed = self
                        .closed_arms
                        .lock()
                        .map_err(|_| EnforcerError::Unhealthy)?;
                    let mut state = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
                    if actor.key.epoch != *identity.original_epoch()
                        || actor.key.nonce != *evidence.original_actor_nonce()
                        || state.status.generation != *identity.generation()
                        || actor
                            .registration
                            .arm
                            .input_hash()
                            .map_err(|_| EnforcerError::InvalidInput)?
                            != identity
                                .arm()
                                .input_hash()
                                .map_err(|_| EnforcerError::InvalidInput)?
                        || serde_json::to_vec(&actor.registration.receipt)
                            .map_err(|_| EnforcerError::InvalidInput)?
                            != serde_json::to_vec(identity.arm_receipt())
                                .map_err(|_| EnforcerError::InvalidInput)?
                    {
                        return Err(EnforcerError::Conflict);
                    }
                    state.gate.close(
                        *identity.retirement_id(),
                        *evidence.prepare_revocation_nonce(),
                    )?;
                    closed.insert(actor.key.arm_id);
                }
                let (sender, receiver) = watch::channel(None);
                operations.insert(
                    operation_key,
                    RetiringOperation {
                        identity_hash: hash,
                        receiver: receiver.clone(),
                    },
                );
                let owner = Arc::clone(self);
                // The coordinator belongs to the role, independently of the
                // IPC waiter. Losing that waiter cannot cancel a join/finalize.
                tokio::spawn(async move {
                    let outcome = owner.coordinate(RetiringActor { actor, accepted }).await;
                    if outcome.is_err() {
                        owner.closed.store(true, Ordering::SeqCst);
                    }
                    sender.send_replace(Some(outcome));
                });
                receiver
            }
        };
        loop {
            if let Some(result) = receiver.borrow().clone() {
                return result;
            }
            receiver
                .changed()
                .await
                .map_err(|_| EnforcerError::Unhealthy)?;
        }
    }

    async fn coordinate(self: &Arc<Self>, retiring: RetiringActor) -> FinalResult {
        let acceptance = retiring.accepted.clone();
        let closing = self
            .ledger_io(true, move |ledger| ledger.begin_closing(acceptance))
            .await?;
        let (closing, tombstone) = self
            .ledger_io(true, move |ledger| {
                let tombstone = ledger.commit_tombstone(&closing)?;
                Ok::<_, EnforcerError>((closing, tombstone))
            })
            .await?;
        let arm_id = retiring.actor.key.arm_id;
        let actor = Arc::clone(&retiring.actor);
        let joined = self
            .owners
            .join_retiring(retiring, &closing, &tombstone)
            .await?;
        let result = self
            .ledger_io(true, move |ledger| {
                ledger.finalize(&closing, &tombstone, joined)
            })
            .await?;
        // The active map is the capacity counter. It changes exactly once,
        // after Q, and remains independent of cumulative ledger admissions.
        let mut actors = self.actors.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if actors
            .get(&arm_id)
            .is_none_or(|current| !Arc::ptr_eq(current, &actor))
        {
            return Err(EnforcerError::Conflict);
        }
        actors.remove(&arm_id);
        drop(actors);
        self.prospective
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .remove(&arm_id);
        self.admitted
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?
            .remove(&arm_id.to_string());
        Ok(result)
    }
    pub(super) async fn dispatch(
        self: &Arc<Self>,
        command: FullCommand,
        progress: u64,
    ) -> EnforcerResult<FullReply> {
        match command {
            FullCommand::ShutdownIfIdle {} => {
                self.finish_idle().await?;
                Ok(FullReply::Acknowledged {})
            }
            FullCommand::Status {} => {
                self.complete().await?;
                let count = self
                    .admitted
                    .lock()
                    .map_err(|_| EnforcerError::Unhealthy)?
                    .len();
                Ok(FullReply::Status {
                    value: EnforcerStatusV1 {
                        epoch: self.epoch,
                        slot_count: count,
                        max_slots: self.max_slots,
                        progress,
                        healthy: self
                            .clock
                            .as_ref()
                            .is_some_and(|clock| clock.sample().is_ok()),
                    },
                })
            }
            FullCommand::Watch {} => Err(EnforcerError::InvalidInput),
            FullCommand::Slots { arm_ids } => {
                if arm_ids.is_empty() || arm_ids.len() > MAX_SLOT_QUERY {
                    return Err(EnforcerError::InvalidInput);
                }
                let actors = self.actors.lock().map_err(|_| EnforcerError::Unhealthy)?;
                let mut values = Vec::with_capacity(arm_ids.len());
                for id in arm_ids {
                    let id = Uuid::parse_str(&id).map_err(|_| EnforcerError::InvalidInput)?;
                    let actor = actors.get(&id).ok_or(EnforcerError::Conflict)?;
                    values.push(
                        actor
                            .state
                            .lock()
                            .map_err(|_| EnforcerError::Unhealthy)?
                            .status
                            .clone(),
                    );
                }
                Ok(FullReply::Slots { values })
            }
            FullCommand::Register {
                registration,
                generation,
                observe_only,
            } => {
                let (value, replayed, binding, actor_nonce, original_registration_nonce) = self
                    .register(*registration, generation, observe_only)
                    .await?;
                Ok(FullReply::Registered {
                    value: Box::new(value),
                    replayed,
                    binding,
                    actor_nonce,
                    original_registration_nonce,
                })
            }
            FullCommand::ReserveProspective {
                prospective,
                generation,
                replay_only,
            } => Ok(FullReply::ProspectiveReserved {
                reservation: self
                    .reserve_prospective(*prospective, generation, replay_only)
                    .await?,
            }),
            FullCommand::BeginProspectiveArmExchange {
                reservation,
                generation,
            } => Ok(FullReply::ProspectiveArmExchange {
                original_exchange_nonce: self
                    .begin_prospective_arm_exchange(&reservation, generation)?,
            }),
            FullCommand::BindProspective {
                reservation,
                registration,
                generation,
                observe_only,
                original_exchange_nonce,
            } => {
                let (value, replayed, binding, actor_nonce, original_registration_nonce) = self
                    .bind_prospective(
                        reservation,
                        *registration,
                        generation,
                        observe_only,
                        original_exchange_nonce,
                    )
                    .await?;
                Ok(FullReply::Registered {
                    value: Box::new(value),
                    replayed,
                    binding,
                    actor_nonce,
                    original_registration_nonce,
                })
            }
            FullCommand::Prepare {
                arm_id,
                registration_hash,
                generation,
            } => {
                self.complete().await?;
                Ok(FullReply::Prepared {
                    value: Box::new(self.prepare(
                        &arm_id,
                        registration_hash.as_str(),
                        generation,
                    )?),
                })
            }
            FullCommand::Cancel {
                arm_id,
                registration_hash,
                generation,
            } => {
                // STOP does not depend on the ledger FS mutex or Full health.
                self.cancel(&arm_id, registration_hash.as_str(), generation)?;
                Ok(FullReply::Acknowledged {})
            }
            FullCommand::RevokeUnused {
                arm_id,
                registration_hash,
                generation,
                actor_nonce,
                original_registration_nonce,
                retirement_id,
            } => {
                self.complete().await?;
                Ok(FullReply::Revoked {
                    revocation_nonce: self.revoke(
                        &arm_id,
                        registration_hash.as_str(),
                        generation,
                        actor_nonce,
                        original_registration_nonce,
                        retirement_id,
                    )?,
                })
            }
            FullCommand::RetireUnused {
                identity,
                evidence,
                audit_ack,
            } => {
                let expected = identity.clone();
                let (receipt, head) = self.retire(*identity, evidence, audit_ack).await?;
                let accepted = self
                    .ledger_io(false, move |ledger| ledger.query(&expected))
                    .await?
                    .map(|entry| entry.accepted().clone())
                    .ok_or(EnforcerError::Conflict)?;
                Ok(FullReply::Retirement {
                    receipt: Some(Box::new(receipt)),
                    head,
                    accepted: Some(Box::new(accepted)),
                })
            }
            FullCommand::QueryRetirement { identity } => {
                let head = self.complete().await?;
                let entry = self
                    .ledger_io(false, move |ledger| ledger.query(&identity))
                    .await?;
                let accepted = entry.as_ref().map(|entry| entry.accepted().clone());
                let receipt = entry.and_then(|entry| match entry.phase() {
                    RetirementLedgerPhaseV1::Final { receipt, .. } => {
                        Some(receipt.as_ref().clone())
                    }
                    _ => None,
                });
                Ok(FullReply::Retirement {
                    receipt: receipt.map(Box::new),
                    head,
                    accepted: accepted.map(Box::new),
                })
            }
        }
    }
    pub(super) fn new(
        ledger: Arc<RetirementLedger>,
        epoch: Uuid,
        max_slots: u32,
        clock: Option<Arc<NativeHostClock>>,
        admitted: AdmissionRegistry,
    ) -> EnforcerResult<Arc<Self>> {
        let closed_arms = ledger.known_arms()?.into_iter().collect();
        let done = Arc::new(AtomicBool::new(false));
        let (owners, joiner) = NativeOwners::start_sealed(ledger.lease(), done, max_slots)?;
        // This Full listener has no native progress thread. The owner retains
        // all per-Arm handles; detaching its support joiner claims no join.
        owners.progress(None)?;
        Ok(Arc::new(Self {
            epoch,
            ledger,
            owners,
            joiner: Mutex::new(Some(joiner)),
            max_slots,
            admitted,
            clock,
            actors: Mutex::new(BTreeMap::new()),
            prospective: Mutex::new(BTreeMap::new()),
            retiring: Mutex::new(BTreeMap::new()),
            closed_arms: Mutex::new(closed_arms),
            closed: AtomicBool::new(false),
            idle_admission_closed: AtomicBool::new(false),
            // Slow ledger IO has its own bounded budget. It cannot occupy all
            // 32 transport slots or allocate an unbounded blocking task queue.
            io: Arc::new(tokio::sync::Semaphore::new(4)),
        }))
    }

    pub(super) async fn complete(self: &Arc<Self>) -> EnforcerResult<LedgerHeadV1> {
        if self.closed.load(Ordering::SeqCst) || self.owners.failed() {
            return Err(EnforcerError::Unhealthy);
        }
        self.complete_ledger(false).await
    }

    pub(super) async fn revalidate_idle_ledger(self: &Arc<Self>) -> EnforcerResult<()> {
        if !self.idle_admission_closed.load(Ordering::SeqCst) || self.owners.failed() {
            return Err(EnforcerError::Unhealthy);
        }
        // The same ledger retains and revalidates its append-only prefix. A
        // retirement may have finalized before the idle reservation won.
        self.complete_ledger(true).await.map(|_| ())
    }

    async fn complete_ledger(self: &Arc<Self>, promised: bool) -> EnforcerResult<LedgerHeadV1> {
        let coverage = self.ledger_io(promised, RetirementLedger::coverage).await;
        match coverage {
            Ok(LedgerCoverageV1::Complete { head, .. }) => Ok(head),
            Err(EnforcerError::Unavailable) => Err(EnforcerError::Unavailable),
            _ => {
                self.closed.store(true, Ordering::SeqCst);
                Err(EnforcerError::Unhealthy)
            }
        }
    }

    pub(super) async fn register(
        self: &Arc<Self>,
        registration: RegistrationV1,
        generation: u64,
        observe_only: bool,
    ) -> EnforcerResult<(
        EnforcerSlotStatusV1,
        bool,
        Full3RegistrationBindingV1,
        Uuid,
        Option<Uuid>,
    )> {
        let head = self.complete().await?;
        registration.validate()?;
        if generation == u64::MAX && !observe_only {
            return Err(EnforcerError::InvalidInput);
        }
        let arm_id = Uuid::parse_str(&registration.arm.operation_id)
            .map_err(|_| EnforcerError::InvalidInput)?;
        // Positive Closing/Q excludes a new actor, including Restore. The
        // coverage check above never turns unknown into absence.
        if self
            .ledger_io(false, move |ledger| ledger.known_entry(arm_id))
            .await?
            .is_some()
        {
            return Err(EnforcerError::Conflict);
        }
        let digest = registration.digest()?;
        let (actor, replayed) = {
            let admitted = self.admitted.lock().map_err(|_| EnforcerError::Unhealthy)?;
            if self.closed.load(Ordering::SeqCst) || self.owners.failed() {
                return Err(EnforcerError::Unhealthy);
            }
            if admitted.get(&registration.arm.operation_id) == Some(&RegistryOwner::Restored) {
                return Err(EnforcerError::Conflict);
            }
            let actors = self.actors.lock().map_err(|_| EnforcerError::Unhealthy)?;
            if let Some(actor) = actors.get(&arm_id) {
                if actor.digest != digest {
                    return Err(EnforcerError::Conflict);
                }
                if observe_only || self.admitted.fenced() {
                    let mut state = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
                    if state.gate.is_closing() {
                        return Err(EnforcerError::Conflict);
                    }
                    state.gate.restore();
                    state.status.prepared_once = true;
                }
                (Arc::clone(actor), true)
            } else {
                if admitted.get(&registration.arm.operation_id) == Some(&RegistryOwner::Prospective)
                {
                    // A native pending owner exists, but its receipt has not
                    // been bound by the exact reservation yet.
                    return Err(EnforcerError::Conflict);
                }
                // The real prospective native owner must precede the central
                // Arm RPC. A receipt-bearing DATA record cannot create one
                // afterward, even on an otherwise fresh Full listener.
                return Err(EnforcerError::NotReady);
            }
        };
        let expires = Instant::now()
            + if observe_only {
                POLL
            } else {
                REGISTER_TIMEOUT - Duration::from_secs(1)
            };
        loop {
            if !observe_only {
                // Do not wait for the old Ready timeout after recovery has
                // already made this native owner restrictive. The owner and
                // handle remain retained even when this waiter returns.
                let _admitted = self.admitted.lock().map_err(|_| EnforcerError::Unhealthy)?;
                self.admitted.check_original()?;
            }
            let (ready, value) = {
                let state = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
                if state.gate.is_closing() {
                    return Err(EnforcerError::Conflict);
                }
                (
                    state.status.phase == SlotPhaseV1::Ready || observe_only,
                    state.status.clone(),
                )
            };
            if ready {
                // Register may have waited for ledger/native Ready while a
                // recovery fence committed. Keep its real actor and resources,
                // but never publish a usable late Original grant.
                let _admitted = self.admitted.lock().map_err(|_| EnforcerError::Unhealthy)?;
                if !observe_only {
                    self.admitted.check_original()?;
                }
                if self.owners.failed() || !actor.runtime_installed.load(Ordering::SeqCst) {
                    return Err(EnforcerError::NotReady);
                }
                let binding = data(Full3RegistrationBindingV1::new(
                    arm_id,
                    data(RetirementDigestV1::parse(&digest))?,
                    head.ledger().clone(),
                    self.epoch,
                ))?;
                return Ok((
                    value,
                    replayed,
                    binding,
                    actor.key.nonce,
                    if replayed || observe_only {
                        None
                    } else {
                        Some(actor.original_registration_nonce)
                    },
                ));
            }
            if Instant::now() >= expires {
                return Err(EnforcerError::NotReady);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    fn actor(&self, arm_id: &str, digest: &str) -> EnforcerResult<Arc<FullActor>> {
        let id = Uuid::parse_str(arm_id).map_err(|_| EnforcerError::InvalidInput)?;
        let actors = self.actors.lock().map_err(|_| EnforcerError::Unhealthy)?;
        let actor = actors.get(&id).ok_or(EnforcerError::Conflict)?;
        if actor.digest != digest {
            return Err(EnforcerError::Conflict);
        }
        Ok(Arc::clone(actor))
    }

    pub(super) fn revoke(
        &self,
        arm_id: &str,
        digest: &str,
        generation: u64,
        actor_nonce: Uuid,
        original_registration_nonce: Uuid,
        retirement_id: Uuid,
    ) -> EnforcerResult<Uuid> {
        let actor = self.actor(arm_id, digest)?;
        if actor.key.nonce != actor_nonce
            || actor.original_registration_nonce != original_registration_nonce
        {
            return Err(EnforcerError::Unauthorized);
        }
        let mut state = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if self.closed.load(Ordering::SeqCst) || self.owners.failed() {
            return Err(EnforcerError::Unhealthy);
        }
        if state.status.generation != generation {
            return Err(EnforcerError::Conflict);
        }
        state.gate.revoke(retirement_id)
    }

    pub(super) fn prepare(
        &self,
        arm_id: &str,
        digest: &str,
        generation: u64,
    ) -> EnforcerResult<EnforcerSlotStatusV1> {
        let _admitted = self.admitted.lock().map_err(|_| EnforcerError::Unhealthy)?;
        self.admitted.check_original()?;
        let actor = self.actor(arm_id, digest)?;
        let clock = self.clock.as_ref().ok_or(EnforcerError::Unhealthy)?;
        let sample = clock.sample().map_err(|_| EnforcerError::Unhealthy)?;
        actor
            .registration
            .arm
            .validate_clock_check(&sample)
            .map_err(|_| EnforcerError::NotReady)?;
        let mut state = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if self.closed.load(Ordering::SeqCst) || self.owners.failed() {
            return Err(EnforcerError::Unhealthy);
        }
        if generation == u64::MAX
            || state.status.generation != generation
            || state.status.phase != SlotPhaseV1::Ready
        {
            return Err(EnforcerError::NotReady);
        }
        state.gate.prepare()?;
        state.status.prepared_once = true;
        state.status.clock = Some(sample);
        Ok(state.status.clone())
    }

    pub(super) fn cancel(&self, arm_id: &str, digest: &str, generation: u64) -> EnforcerResult<()> {
        self.reserve_cancel(arm_id, digest, generation, None)
    }

    pub(super) fn cancel_for_durable_stop(
        &self,
        arm_id: &str,
        digest: &str,
        generation: u64,
        cancel: &CancelReferenceV1,
    ) -> EnforcerResult<()> {
        self.reserve_cancel(arm_id, digest, generation, Some(cancel))
    }

    fn reserve_cancel(
        &self,
        arm_id: &str,
        digest: &str,
        generation: u64,
        cancel: Option<&CancelReferenceV1>,
    ) -> EnforcerResult<()> {
        let actor = self.actor(arm_id, digest)?;
        let mut state = actor.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if generation < state.status.generation {
            return Err(EnforcerError::Conflict);
        }
        state.gate.reserve_stop()?;
        if cancel.is_some_and(|cancel| {
            state
                .durable_stop
                .as_ref()
                .is_some_and(|(retained_generation, retained)| {
                    *retained_generation == generation && retained == cancel
                })
        }) && generation == state.status.generation
        {
            return Ok(());
        }
        if generation != state.status.generation || cancel.is_some() {
            state.durable_stop = cancel.cloned().map(|cancel| (generation, cancel));
            // A conflicting incarnation cannot be qualified in this
            // original actor, even after another STOP or recovery generation.
            if !matches!(
                state.retained_terminal,
                RetainedTerminalObservation::Conflicting
            ) {
                state.retained_terminal = RetainedTerminalObservation::Pending;
            }
        }
        state.status.generation = generation;
        state.status.phase = SlotPhaseV1::Stopping;
        actor.wake.notify_all();
        Ok(())
    }
}

impl NativeOwners {
    async fn join_retiring(
        &self,
        retiring: RetiringActor,
        closing: &ClosingCommitted,
        tombstone: &TombstoneCommitted,
    ) -> EnforcerResult<JoinedQuiescence> {
        let accepted = &retiring.accepted;
        let committed = closing.accepted();
        let tombstone_record = tombstone.tombstone();
        let identity_hash = data(accepted.identity().digest())?;
        let closing_hash = data(accepted.digest())?;
        if data(committed.digest())? != closing_hash
            || tombstone_record.identity_hash() != &identity_hash
            || tombstone_record.closing_hash() != &closing_hash
            || retiring.actor.key.epoch != *accepted.identity().original_epoch()
            || retiring.actor.key.nonce != *accepted.evidence().original_actor_nonce()
        {
            return Err(EnforcerError::Conflict);
        }
        let tombstone_hash = data(tombstone_record.digest())?;
        {
            let state = retiring
                .actor
                .state
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?;
            if !state.gate.is_closing() {
                return Err(EnforcerError::Conflict);
            }
            retiring.actor.exit.store(true, Ordering::SeqCst);
            retiring.actor.wake.notify_all();
        }
        let key = retiring.actor.key;
        let mut native = self.take_retiring(key)?;
        let (returned, receiver) = tokio::sync::oneshot::channel();
        let joiner = std::thread::Builder::new()
            .name("enforcer-retiring-join".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                let _ = returned.send(native.join());
            })
            .map_err(|_| EnforcerError::Unavailable)?;
        // The per-Arm support joiner owns the original guarded handle/lease.
        // It must not occupy Tokio's shared blocking pool: many blocked joins
        // cannot starve independent STOP transport revalidation or ledger IO.
        drop(joiner);
        receiver.await.map_err(|_| EnforcerError::Unhealthy)??;
        if self.failed() {
            return Err(EnforcerError::Unhealthy);
        }
        // This is the sole live witness constructor. Neither a finished flag,
        // a durable record, Drop nor a cancelled future can reach this line.
        let record = data(JoinedQuiescenceV1::new(
            identity_hash,
            closing_hash,
            tombstone_hash,
            key.epoch,
            key.nonce,
        ))?;
        Ok(JoinedQuiescence { record })
    }
}

impl FullActor {
    fn cause_target(&self) -> CauseTarget {
        CauseTarget::fixed(self.registration.arm_id(), &self.digest)
    }
    fn failed(&self) {
        if let Ok(mut state) = self.state.lock() {
            // Closing is represented in its own latch and never erased by
            // Ready/Quarantined status updates or an actor failure.
            if !state.gate.is_closing() {
                state.status.phase = SlotPhaseV1::Quarantined;
            }
            state.status.stop_failures = state.status.stop_failures.saturating_add(1);
        }
    }

    /// The prospective factory transfers its already installed runtime to
    /// this same native actor; binding never starts a second OS thread.
    fn run_installed(
        self: &Arc<Self>,
        clock: Option<Arc<NativeHostClock>>,
        runtime: tokio::runtime::Runtime,
    ) -> EnforcerResult<()> {
        self.runtime_installed.store(true, Ordering::SeqCst);
        #[cfg(test)]
        if let Some(result) = tests::synthetic_ready_original(self) {
            return result;
        }
        self.run_inner(clock, runtime)
    }
    fn run_inner(
        &self,
        clock: Option<Arc<NativeHostClock>>,
        runtime: tokio::runtime::Runtime,
    ) -> EnforcerResult<()> {
        let managed = loop {
            // Reopen performs info/inspect only. It is owned by this native
            // thread, so its real return is drained by the retained join.
            if self.exit.load(Ordering::SeqCst) {
                return Ok(());
            }
            match self.failures.execute(
                &runtime,
                self.cause_target(),
                ManagedRuntime::reopen_native(self.registration.authority.clone()),
            ) {
                Some(settled) => break settled.value,
                None => {
                    self.failed();
                    let state = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
                    let _waiting = self
                        .wake
                        .wait_timeout(state, Duration::from_secs(1))
                        .map_err(|_| EnforcerError::Unhealthy)?;
                }
            }
        };
        let mut last = None;
        loop {
            if self.exit.load(Ordering::SeqCst) {
                return Ok(());
            }
            let sample = clock.as_ref().and_then(|clock| clock.sample().ok());
            let live = sample.as_ref().is_some_and(|sample| {
                sample.continuous_ns().is_ok_and(|now| {
                    let monotone = last.is_none_or(|old| now >= old);
                    last = Some(now);
                    monotone && self.registration.arm.validate_clock_check(sample).is_ok()
                })
            });
            let mut state = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
            if self.exit.load(Ordering::SeqCst) {
                return Ok(());
            }
            if state.gate.is_closing() {
                // A C publication failure keeps the native owner fenced. A T
                // coordinator will wake it after setting exit, outside IO.
                drop(
                    self.wake
                        .wait(state)
                        .map_err(|_| EnforcerError::Unhealthy)?,
                );
                continue;
            }
            if !live {
                state.gate.reserve_stop()?;
            }
            state.status.clock = sample;
            let stopping = state.gate.stop_reserved;
            state.status.phase = if stopping {
                SlotPhaseV1::Stopping
            } else {
                SlotPhaseV1::Ready
            };
            let durable_stop = state
                .durable_stop
                .as_ref()
                .filter(|(generation, _)| *generation == state.status.generation)
                .cloned();
            drop(state);
            if stopping {
                // The irreversible reservation happened under exactly the
                // same mutex as Closing; all runtime IO happens after it.
                let result = self.failures.execute(
                    &runtime,
                    self.cause_target(),
                    managed.stop_snapshot_owned_native(&self.registration.owned),
                );
                let mut state = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
                match result {
                    Some(settled) => {
                        // A result can update diagnostics even if a new STOP
                        // arrived during IO. Only a matching capture may
                        // contribute to the retained terminal candidate.
                        state.retain_terminal_candidate(
                            durable_stop.as_ref(),
                            &settled.value.observation,
                            &self.registration,
                        );
                        self.failures
                            .observed_stop(self.cause_target(), settled.value.observation.clone());
                        state.status.observation = Some(settled.value.observation);
                        if let Some(cause) = settled.value.kill_failure {
                            self.failures
                                .returned(self.cause_target(), cause, settled.proof);
                            state.status.stop_failures =
                                state.status.stop_failures.saturating_add(1);
                        }
                    }
                    None => {
                        state.status.stop_failures = state.status.stop_failures.saturating_add(1)
                    }
                }
                state.status.phase = SlotPhaseV1::Quarantined;
            }
            let state = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
            let _waiting = self
                .wake
                .wait_timeout(
                    state,
                    if stopping {
                        Duration::from_millis(250)
                    } else {
                        Duration::from_millis(25)
                    },
                )
                .map_err(|_| EnforcerError::Unhealthy)?;
        }
    }
}

#[derive(Default)]
pub(super) struct SlotLatch(AtomicU8);
impl SlotLatch {
    pub(super) fn reserve_claim(&self) -> EnforcerResult<()> {
        self.0
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| ())
            .map_err(|_| EnforcerError::Conflict)
    }
    pub(super) fn revoke(&self) -> EnforcerResult<()> {
        self.0
            .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| ())
            .map_err(|_| EnforcerError::Conflict)
    }
    pub(super) fn revoked(&self) -> bool {
        self.0.load(Ordering::SeqCst) == 2
    }
    pub(super) fn claim_reserved(&self) -> bool {
        self.0.load(Ordering::SeqCst) == 1
    }
}

/// This latch belongs to one slot. Dropping the reservation never changes it
/// back to Unclaimed, even if the central claim reply was lost.
pub(crate) use super::client::ClaimReservation;

/// All methods are called under the actor's existing status mutex. They do no
/// IO and arbitrate Prepare, deadline/reopen STOP, Cancel and Closing together.
pub(super) struct OriginalGate {
    original: bool,
    prepared: bool,
    stop_reserved: bool,
    revoked: Option<(Uuid, Uuid)>,
    closing: bool,
}
impl OriginalGate {
    pub(super) fn new(original: bool) -> Self {
        Self {
            original,
            prepared: !original,
            stop_reserved: false,
            revoked: None,
            closing: false,
        }
    }
    pub(super) fn prepare(&mut self) -> EnforcerResult<()> {
        if self.closing || self.revoked.is_some() || self.stop_reserved {
            return Err(EnforcerError::Conflict);
        }
        if self.prepared {
            return Err(EnforcerError::AlreadyPrepared);
        }
        self.prepared = true;
        Ok(())
    }
    pub(super) fn reserve_stop(&mut self) -> EnforcerResult<()> {
        if self.closing {
            return Err(EnforcerError::Conflict);
        }
        self.stop_reserved = true;
        Ok(())
    }
    pub(super) fn restore(&mut self) {
        self.original = false;
        self.prepared = true;
    }
    pub(super) fn revoke(&mut self, retirement_id: Uuid) -> EnforcerResult<Uuid> {
        if !self.original {
            return Err(EnforcerError::Unauthorized);
        }
        if self.prepared {
            return Err(EnforcerError::AlreadyPrepared);
        }
        if self.stop_reserved || self.closing || retirement_id.is_nil() {
            return Err(EnforcerError::Conflict);
        }
        if let Some((old, nonce)) = self.revoked {
            return if old == retirement_id {
                Ok(nonce)
            } else {
                Err(EnforcerError::Conflict)
            };
        }
        let nonce = Uuid::new_v4();
        self.revoked = Some((retirement_id, nonce));
        Ok(nonce)
    }
    pub(super) fn close(&mut self, retirement_id: Uuid, nonce: Uuid) -> EnforcerResult<()> {
        if self.closing
            || !self.original
            || self.prepared
            || self.stop_reserved
            || self.revoked != Some((retirement_id, nonce))
        {
            return Err(EnforcerError::Conflict);
        }
        self.closing = true;
        Ok(())
    }
    pub(super) fn is_closing(&self) -> bool {
        self.closing
    }
}

/// Kept by the writer until the audited Pending has authorized its consumption.
/// There is no constructor from stored records or public status.
pub(crate) use super::client::{UnusedPrepareRevoked, VerifiedRetirementReceipt};

/// Non-Clone/non-Serde permission, created only after the retained handle's
/// join returned successfully. The record itself cannot recreate this type.
pub(crate) struct JoinedQuiescence {
    record: JoinedQuiescenceV1,
}
impl JoinedQuiescence {
    pub(super) fn into_record(self) -> JoinedQuiescenceV1 {
        self.record
    }
}

#[cfg(test)]
#[path = "retirement_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "retirement_test_support.rs"]
pub(crate) mod test_support;
