//! Bounded process-local arbitration before a Full3 claim. Nothing in this
//! module proves authentication, durable audit, actor revocation or retirement.

use super::protocol::BrokerErrorCodeV1;
use crate::broker::writer::{AuthorizedRetirement, BeginUnusedRetirement, PendingUnusedRetirement};
use crate::{
    broker::{
        enforcer::{
            CancelGeneration, EnforcerError, EnforcerResult,
            retirement::VerifiedRetirementReceipt,
        },
        retirement::{RetirementAuditAckV1, RetirementEvidenceV1, RetirementIdentityV1},
    },
    model::Event,
};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Notify;
use uuid::Uuid;

const MAX_PRECLAIM_ARMS: usize = 1024;

/// Closed request data only. The authenticated service supplies its scope and
/// verifies the exact confirmed Arm before calling the process-local arbiter.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct AbandonBeforeClaimV1 {
    pub version: u32,
    pub operation_id: Uuid,
    pub arm_id: Uuid,
    pub command_id: Uuid,
    pub expected_epoch: Uuid,
    pub generation: u64,
}

impl fmt::Debug for AbandonBeforeClaimV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AbandonBeforeClaimV1(<request data>)")
    }
}

impl AbandonBeforeClaimV1 {
    pub fn validate(&self) -> Result<(), BrokerErrorCodeV1> {
        if self.version != 1
            || self.operation_id.is_nil()
            || self.arm_id.is_nil()
            || self.command_id.is_nil()
            || self.expected_epoch.is_nil()
            || self.generation == u64::MAX
        {
            return Err(BrokerErrorCodeV1::InvalidInput);
        }
        Ok(())
    }

    fn event(&self) -> Event {
        let mut event = Event::new("broker.abandon-before-claim.requested");
        event.id = self.command_id.to_string();
        event.source = "ai-guard-broker".into();
        event.coverage = "native".into();
        event.data = serde_json::json!({
            "version": self.version,
            "operation_id": self.operation_id.to_string(),
            "arm_id": self.arm_id.to_string(),
            "command_id": self.command_id.to_string(),
            "expected_epoch": self.expected_epoch.to_string(),
            "generation": self.generation,
        });
        event
    }
}

fn canonical_uuid<'de, D: Deserializer<'de>>(decoder: D) -> Result<Uuid, D::Error> {
    let value = String::deserialize(decoder)?;
    let id = Uuid::parse_str(&value).map_err(|_| D::Error::custom("invalid preclaim identity"))?;
    if id.is_nil() || id.hyphenated().to_string() != value {
        return Err(D::Error::custom("invalid preclaim identity"));
    }
    Ok(id)
}

impl<'de> Deserialize<'de> for AbandonBeforeClaimV1 {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            version: u32,
            #[serde(deserialize_with = "canonical_uuid")]
            operation_id: Uuid,
            #[serde(deserialize_with = "canonical_uuid")]
            arm_id: Uuid,
            #[serde(deserialize_with = "canonical_uuid")]
            command_id: Uuid,
            #[serde(deserialize_with = "canonical_uuid")]
            expected_epoch: Uuid,
            generation: u64,
        }
        let wire =
            Wire::deserialize(decoder).map_err(|_| D::Error::custom("invalid preclaim request"))?;
        let request = Self {
            version: wire.version,
            operation_id: wire.operation_id,
            arm_id: wire.arm_id,
            command_id: wire.command_id,
            expected_epoch: wire.expected_epoch,
            generation: wire.generation,
        };
        request.validate().map_err(D::Error::custom)?;
        Ok(request)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreclaimStateV1 {
    RegistrationInFlight,
    OriginalUnclaimed,
    ClaimReserved,
    StopReserved,
    AbandonPendingAudit,
    AbandonRequested,
    Retiring,
    Unknown,
}

/// A projection of this process's coordination state, never runtime evidence.
/// Missing owners and poisoned reads remain explicitly unknown.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct PreclaimArmViewV1 {
    pub(crate) arm_id: Uuid,
    pub(crate) expected_epoch: Option<Uuid>,
    pub(crate) generation: Option<u64>,
    pub(crate) state: PreclaimStateV1,
    pub(crate) command_id: Option<Uuid>,
    pub(crate) audit_confirmed: bool,
    pub(crate) retirement_continuation_retained: Option<bool>,
    pub(crate) source: &'static str,
    pub(crate) freshness: &'static str,
}

#[derive(Clone)]
pub(crate) struct PreclaimControlV1 {
    inner: Arc<Inner>,
}

struct Inner {
    cancel: CancelGeneration,
    state: Mutex<State>,
    changed: Notify,
    capacity: usize,
    retained_unmatched: Mutex<Vec<RetainedRetirementV1>>,
    unmatched_retirement: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct State {
    operation: Option<Uuid>,
    arms: BTreeMap<Uuid, Arm>,
    commands: BTreeMap<Uuid, Uuid>,
}

struct Arm {
    epoch: Uuid,
    generation: u64,
    phase: PreclaimStateV1,
    signal: Option<Signal>,
    retained_retirement: Option<RetainedRetirementV1>,
}

/// Process owners survive their caller futures, including an uncertain
/// submission. They cannot be serialized or rebuilt from a journal projection.
pub(crate) enum RetainedRetirementV1 {
    Begin {
        _owner: BeginUnusedRetirement,
    },
    Pending {
        _owner: PendingUnusedRetirement,
    },
    Authorized {
        _owner: Box<AuthorizedRetirement>,
    },
    Submission {
        owner: Arc<RetirementSubmission>,
    },
    Verified {
        _owner: Box<crate::broker::enforcer::retirement::VerifiedRetirementReceipt>,
    },
}

/// One actual authorization, retained in its existing bounded Arm slot. Arc
/// handles share this owner; they cannot clone or reconstruct its capability.
pub(crate) struct RetirementSubmission {
    // Keep the original producer/core handle even after its single-use
    // revocation proof has been consumed at Submitted.
    authorization: Box<AuthorizedRetirement>,
    identity: RetirementIdentityV1,
    evidence: RetirementEvidenceV1,
    audit_ack: RetirementAuditAckV1,
    state: Mutex<RetirementSubmissionState>,
}

/// Borrowed continuation of the actual canonical Arm owner, never a projection
/// or a new authorization. Only this module can construct its request/owner pair.
pub(crate) struct SubmittedRetirementSettlement {
    request: AbandonBeforeClaimV1,
    owner: Arc<RetirementSubmission>,
}

impl SubmittedRetirementSettlement {
    pub(crate) fn owner(&self) -> &Arc<RetirementSubmission> {
        &self.owner
    }
}

impl fmt::Debug for SubmittedRetirementSettlement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SubmittedRetirementSettlement(<canonical owner>)")
    }
}

enum RetainedFinalInput {
    // Existing observations are not a productive retry permission.
    Blocked {
        _input: Box<VerifiedRetirementReceipt>,
    },
    Retryable(Box<VerifiedRetirementReceipt>),
    Borrowed,
}

/// Owns the actual Q input across a local handoff, never a copy of authority.
/// Drop before enqueue restores that same input; after enqueue the writer's
/// registry owns it independently of this token and of its caller future.
pub(super) struct PendingFinalInput<'guard> {
    _guard: &'guard super::jobs::RetirementQueryGuard,
    control: PreclaimControlV1,
    continuation: SubmittedRetirementSettlement,
    input: Option<VerifiedRetirementReceipt>,
}

impl PendingFinalInput<'_> {
    pub(super) fn input(&self) -> Result<&VerifiedRetirementReceipt, BrokerErrorCodeV1> {
        self.input
            .as_ref()
            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)
    }

    pub(super) fn take_input(&mut self) -> Result<VerifiedRetirementReceipt, BrokerErrorCodeV1> {
        self.input
            .take()
            .ok_or(BrokerErrorCodeV1::NeedsReconciliation)
    }

    pub(super) fn retain_returned(
        &mut self,
        input: VerifiedRetirementReceipt,
    ) -> Result<(), BrokerErrorCodeV1> {
        if self.input.is_none()
            && valid_final_input(self.continuation.owner(), &input).is_ok()
        {
            self.input = Some(input);
            return Ok(());
        }
        self.control.retain_retirement(
            self.continuation.request.arm_id,
            self.continuation.request.expected_epoch,
            self.continuation.request.generation,
            RetainedRetirementV1::Verified {
                _owner: Box::new(input),
            },
        );
        Err(BrokerErrorCodeV1::NeedsReconciliation)
    }
}

impl fmt::Debug for PendingFinalInput<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PendingFinalInput(<retained actual Q>)")
    }
}

impl Drop for PendingFinalInput<'_> {
    fn drop(&mut self) {
        self.control
            .restore_final_input(&self.continuation, self.input.take());
    }
}

fn valid_final_input(
    owner: &Arc<RetirementSubmission>,
    input: &VerifiedRetirementReceipt,
) -> Result<(), BrokerErrorCodeV1> {
    let digest = |result: crate::broker::journal::JournalResult<_>| {
        result.map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)
    };
    if input.completes_retained_submission(owner)
        || digest(input.identity().digest())? != digest(owner.identity().digest())?
        || digest(input.accepted().evidence().digest())? != digest(owner.evidence().digest())?
        || serde_json::to_vec(input.accepted().audit_ack())
            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
            != serde_json::to_vec(owner.audit_ack())
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
    {
        return Err(BrokerErrorCodeV1::NeedsReconciliation);
    }
    Ok(())
}

enum RetirementSubmissionState {
    Unsubmitted {
        attempted: bool,
    },
    Submitted {
        // A certainly unqueued Final still owns its actual verified input.
        // One fixed slot, not another list of continuation attempts.
        retained_verified: Option<RetainedFinalInput>,
    },
}

impl RetirementSubmission {
    fn new(authorized: AuthorizedRetirement) -> Self {
        let (identity, evidence, audit_ack) = authorized.request_parts();
        Self {
            authorization: Box::new(authorized),
            identity,
            evidence,
            audit_ack,
            state: Mutex::new(RetirementSubmissionState::Unsubmitted { attempted: false }),
        }
    }

    pub(crate) fn identity(&self) -> &RetirementIdentityV1 {
        &self.identity
    }

    pub(crate) fn evidence(&self) -> &RetirementEvidenceV1 {
        &self.evidence
    }

    pub(crate) fn audit_ack(&self) -> &RetirementAuditAckV1 {
        &self.audit_ack
    }

    pub(crate) fn matches_recovery_origin(
        &self,
        snapshot: &crate::broker::journal::StopJobSnapshotV1,
    ) -> bool {
        self.authorization.matches_recovery_origin(snapshot)
    }

    /// At most one automatic transport attempt, including a failed connect.
    /// Failure or cancellation never resets this reservation.
    pub(crate) fn reserve_attempt(&self) -> EnforcerResult<()> {
        let mut state = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        match &mut *state {
            RetirementSubmissionState::Unsubmitted { attempted, .. } if !*attempted => {
                *attempted = true;
                Ok(())
            }
            _ => Err(EnforcerError::Conflict),
        }
    }

    fn mark_submitted(&self) -> EnforcerResult<()> {
        let mut state = self.state.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if !matches!(
            &*state,
            RetirementSubmissionState::Unsubmitted {
                attempted: true,
                ..
            }
        ) {
            return Err(EnforcerError::Conflict);
        }
        // The live unused proof is consumed once, before any command byte can
        // be emitted. No error, cancellation or Q=None can recreate it.
        self.authorization
            .consume_original()
            .map_err(|_| EnforcerError::Conflict)?;
        *state = RetirementSubmissionState::Submitted {
            retained_verified: None,
        };
        Ok(())
    }

    pub(crate) fn is_submitted(&self) -> EnforcerResult<bool> {
        self.state
            .lock()
            .map(|state| matches!(&*state, RetirementSubmissionState::Submitted { .. }))
            .map_err(|_| EnforcerError::Unhealthy)
    }

    fn ready_for_settlement(&self) -> EnforcerResult<bool> {
        self.state
            .lock()
            .map(|state| {
                matches!(
                    &*state,
                    RetirementSubmissionState::Submitted {
                        retained_verified: None
                    }
                )
            })
            .map_err(|_| EnforcerError::Unhealthy)
    }

    fn has_retryable_final(&self) -> EnforcerResult<bool> {
        self.state
            .lock()
            .map(|state| {
                matches!(
                    &*state,
                    RetirementSubmissionState::Submitted {
                        retained_verified: Some(RetainedFinalInput::Retryable(_))
                    }
                )
            })
            .map_err(|_| EnforcerError::Unhealthy)
    }

    pub(crate) fn retain_verified(
        &self,
        verified: crate::broker::enforcer::retirement::VerifiedRetirementReceipt,
    ) -> Result<(), crate::broker::enforcer::retirement::VerifiedRetirementReceipt> {
        // Poison recovery only stores an already-owned input. All permission,
        // query and release checks keep their fail-closed poison behavior.
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let RetirementSubmissionState::Submitted { retained_verified } = &mut *state
            && retained_verified.is_none()
        {
            *retained_verified = Some(RetainedFinalInput::Blocked {
                _input: Box::new(verified),
            });
            return Ok(());
        }
        Err(verified)
    }

    #[cfg(test)]
    pub(crate) fn retains_original_for_test(&self) -> EnforcerResult<bool> {
        let unsubmitted = self
            .state
            .lock()
            .map(|state| matches!(&*state, RetirementSubmissionState::Unsubmitted { .. }))
            .map_err(|_| EnforcerError::Unhealthy)?;
        Ok(unsubmitted
            && self
                .authorization
                .retains_original_for_test()
                .map_err(|_| EnforcerError::Unhealthy)?)
    }
}

impl fmt::Debug for RetirementSubmission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RetirementSubmission(<one retained original>)")
    }
}

struct Signal {
    request: AbandonBeforeClaimV1,
    event: Arc<Event>,
    audit_confirmed: bool,
}

impl fmt::Debug for PreclaimControlV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PreclaimControlV1(<process-local arbitration>)")
    }
}

fn identity(arm: Uuid, epoch: Uuid, generation: u64) -> Result<(), BrokerErrorCodeV1> {
    if arm.is_nil() || epoch.is_nil() || generation == u64::MAX {
        Err(BrokerErrorCodeV1::InvalidInput)
    } else {
        Ok(())
    }
}

fn matching_arm(
    state: &mut State,
    arm: Uuid,
    epoch: Uuid,
    generation: u64,
) -> Result<&mut Arm, BrokerErrorCodeV1> {
    identity(arm, epoch, generation)?;
    let entry = state
        .arms
        .get_mut(&arm)
        .ok_or(BrokerErrorCodeV1::NeedsReconciliation)?;
    if entry.epoch != epoch || entry.generation != generation {
        return Err(BrokerErrorCodeV1::Conflict);
    }
    Ok(entry)
}

fn matching_settlement_arm<'a>(
    state: &'a mut State,
    continuation: &SubmittedRetirementSettlement,
) -> Result<&'a mut Arm, BrokerErrorCodeV1> {
    let request = &continuation.request;
    if state.operation != Some(request.operation_id) {
        return Err(BrokerErrorCodeV1::Conflict);
    }
    let entry = matching_arm(
        state,
        request.arm_id,
        request.expected_epoch,
        request.generation,
    )?;
    if !entry
        .signal
        .as_ref()
        .is_some_and(|signal| signal.audit_confirmed && signal.request == *request)
        || !matches!(
            &entry.retained_retirement,
            Some(RetainedRetirementV1::Submission { owner })
                if Arc::ptr_eq(owner, continuation.owner())
        )
    {
        return Err(BrokerErrorCodeV1::Conflict);
    }
    Ok(entry)
}

impl PreclaimControlV1 {
    pub(crate) fn new(cancel: CancelGeneration) -> Self {
        Self::bounded(cancel, MAX_PRECLAIM_ARMS)
    }

    fn bounded(cancel: CancelGeneration, capacity: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                cancel,
                state: Mutex::new(State::default()),
                changed: Notify::new(),
                capacity: capacity.min(MAX_PRECLAIM_ARMS),
                retained_unmatched: Mutex::new(Vec::new()),
                unmatched_retirement: std::sync::atomic::AtomicBool::new(false),
            }),
        }
    }

    #[cfg(test)]
    fn with_test_capacity(cancel: CancelGeneration, capacity: usize) -> Self {
        Self::bounded(cancel, capacity)
    }

    fn current(&self, generation: u64) -> Result<(), BrokerErrorCodeV1> {
        if generation == u64::MAX || self.inner.cancel.current() != generation {
            Err(BrokerErrorCodeV1::Interrupted)
        } else {
            Ok(())
        }
    }

    fn active_arm<'a>(
        &self,
        state: &'a mut State,
        arm: Uuid,
        epoch: Uuid,
        generation: u64,
    ) -> Result<&'a mut Arm, BrokerErrorCodeV1> {
        let entry = matching_arm(state, arm, epoch, generation)?;
        if let Err(error) = self.current(generation) {
            entry.phase = PreclaimStateV1::StopReserved;
            return Err(error);
        }
        Ok(entry)
    }

    /// Call only for a fresh authenticated Full3 Arm before its real Register.
    /// Exact replay preserves the current state; it never reopens a choice.
    pub(crate) fn announce_registration(
        &self,
        operation: Uuid,
        arm: Uuid,
        epoch: Uuid,
        generation: u64,
    ) -> Result<(), BrokerErrorCodeV1> {
        identity(arm, epoch, generation)?;
        if operation.is_nil() {
            return Err(BrokerErrorCodeV1::InvalidInput);
        }
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        self.current(generation)?;
        if state.operation.is_some_and(|known| known != operation) {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        if state.arms.contains_key(&arm) {
            matching_arm(&mut state, arm, epoch, generation)?;
            return Ok(());
        }
        if state.arms.len() >= self.inner.capacity {
            return Err(BrokerErrorCodeV1::LimitExceeded);
        }
        state.operation = Some(operation);
        state.arms.insert(
            arm,
            Arm {
                epoch,
                generation,
                phase: PreclaimStateV1::RegistrationInFlight,
                signal: None,
                retained_retirement: None,
            },
        );
        self.current(generation)
    }

    /// The winner cannot be undone by Drop. This reserves only the memory
    /// branch; the executor must still obtain its real enforcer claim guard.
    pub(crate) fn choose_claim(
        &self,
        arm: Uuid,
        epoch: Uuid,
        generation: u64,
    ) -> Result<PreclaimClaimReservationV1, BrokerErrorCodeV1> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let entry = self.active_arm(&mut state, arm, epoch, generation)?;
        if !matches!(
            entry.phase,
            PreclaimStateV1::RegistrationInFlight | PreclaimStateV1::OriginalUnclaimed
        ) {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        entry.phase = PreclaimStateV1::ClaimReserved;
        if let Err(error) = self.current(generation) {
            entry.phase = PreclaimStateV1::StopReserved;
            return Err(error);
        }
        drop(state);
        self.inner.changed.notify_waiters();
        Ok(PreclaimClaimReservationV1 {
            control: self.clone(),
            arm,
            epoch,
            generation,
        })
    }

    /// Reserves immediate inhibition, without IO or a durable-audit claim.
    /// The exact request and event remain cached even if every handle drops.
    pub(crate) fn request(
        &self,
        input: AbandonBeforeClaimV1,
    ) -> Result<PreclaimSignalReservationV1, BrokerErrorCodeV1> {
        input.validate()?;
        // UUID/clock generation and JSON allocation happen outside the mutex.
        // Only the winner's immutable event is retained; replay reuses it.
        let candidate = Arc::new(input.event());
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        if let Some(arm) = state.commands.get(&input.command_id) {
            let signal = state
                .arms
                .get(arm)
                .and_then(|entry| entry.signal.as_ref())
                .ok_or(BrokerErrorCodeV1::Unavailable)?;
            if signal.request != input {
                return Err(BrokerErrorCodeV1::Conflict);
            }
            return Ok(self.reservation(signal));
        }
        self.current(input.generation)?;
        if state.operation != Some(input.operation_id) {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let entry = self.active_arm(
            &mut state,
            input.arm_id,
            input.expected_epoch,
            input.generation,
        )?;
        if !matches!(
            entry.phase,
            PreclaimStateV1::RegistrationInFlight | PreclaimStateV1::OriginalUnclaimed
        ) {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        entry.phase = PreclaimStateV1::AbandonPendingAudit;
        entry.signal = Some(Signal {
            request: input.clone(),
            event: candidate,
            audit_confirmed: false,
        });
        let reservation = self.reservation(
            entry
                .signal
                .as_ref()
                .ok_or(BrokerErrorCodeV1::Unavailable)?,
        );
        state.commands.insert(input.command_id, input.arm_id);
        self.current(input.generation)?;
        drop(state);
        self.inner.changed.notify_waiters();
        Ok(reservation)
    }

    fn reservation(&self, signal: &Signal) -> PreclaimSignalReservationV1 {
        PreclaimSignalReservationV1 {
            control: self.clone(),
            request: signal.request.clone(),
            event: Arc::clone(&signal.event),
        }
    }

    /// Read immediately after Register. None allows the normal caller to race
    /// choose_claim without waiting; Some waits only for this selected signal.
    pub(crate) fn signal_for_arm(
        &self,
        arm: Uuid,
        epoch: Uuid,
        generation: u64,
    ) -> Result<Option<PreclaimSignalReservationV1>, BrokerErrorCodeV1> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let entry = self.active_arm(&mut state, arm, epoch, generation)?;
        if entry.phase == PreclaimStateV1::StopReserved {
            return Err(BrokerErrorCodeV1::Interrupted);
        }
        Ok(entry.signal.as_ref().map(|signal| self.reservation(signal)))
    }

    pub(crate) fn mark_original_unclaimed(
        &self,
        arm: Uuid,
        epoch: Uuid,
        generation: u64,
    ) -> Result<(), BrokerErrorCodeV1> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let entry = self.active_arm(&mut state, arm, epoch, generation)?;
        match entry.phase {
            PreclaimStateV1::RegistrationInFlight => {
                entry.phase = PreclaimStateV1::OriginalUnclaimed
            }
            PreclaimStateV1::OriginalUnclaimed
            | PreclaimStateV1::AbandonPendingAudit
            | PreclaimStateV1::AbandonRequested => {}
            _ => return Err(BrokerErrorCodeV1::Conflict),
        }
        self.current(generation)
    }

    #[cfg(test)]
    pub(crate) fn mark_stop_reserved(
        &self,
        arm: Uuid,
        epoch: Uuid,
        generation: u64,
    ) -> Result<(), BrokerErrorCodeV1> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        matching_arm(&mut state, arm, epoch, generation)?.phase = PreclaimStateV1::StopReserved;
        drop(state);
        self.inner.changed.notify_waiters();
        Ok(())
    }

    /// Metadata only, called after the real owner selected its retirement path.
    /// It creates no unused-slot witness and never bypasses STOP arbitration.
    pub(crate) fn mark_retiring(
        &self,
        arm: Uuid,
        epoch: Uuid,
        generation: u64,
    ) -> Result<(), BrokerErrorCodeV1> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let entry = self.active_arm(&mut state, arm, epoch, generation)?;
        if !matches!(
            entry.phase,
            PreclaimStateV1::AbandonRequested | PreclaimStateV1::Retiring
        ) || !entry
            .signal
            .as_ref()
            .is_some_and(|signal| signal.audit_confirmed)
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        entry.phase = PreclaimStateV1::Retiring;
        self.current(generation)
    }

    /// Retention grants no authority and must survive Cancel/poisoned routing.
    /// Each live input owns one original native slot, whose producer is capped
    /// at MAX_PRECLAIM_ARMS and cannot free that slot while its input is retained.
    /// The fallback preserves a mismatched owner for diagnosis, never replay.
    pub(crate) fn retain_retirement(
        &self,
        arm: Uuid,
        epoch: Uuid,
        generation: u64,
        continuation: RetainedRetirementV1,
    ) {
        {
            // Poison recovery is limited to storing an already-owned value.
            // Permission methods keep their existing fail-closed poison rule.
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = state.arms.get_mut(&arm)
                && entry.epoch == epoch
                && entry.generation == generation
                && entry.retained_retirement.is_none()
            {
                entry.retained_retirement = Some(continuation);
                return;
            }
        }
        self.inner
            .retained_unmatched
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(continuation);
        self.inner
            .unmatched_retirement
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Install the real input before the consumer can suspend. A refused
    /// installation returns that same input, so the existing retention path
    /// can preserve it without reconstructing authority or growing a retry map.
    pub(crate) fn retain_authorized_submission(
        &self,
        request: &AbandonBeforeClaimV1,
        authorized: AuthorizedRetirement,
    ) -> Result<Arc<RetirementSubmission>, (BrokerErrorCodeV1, AuthorizedRetirement)> {
        let mut state = match self.inner.state.lock() {
            Ok(state) => state,
            Err(_) => return Err((BrokerErrorCodeV1::Unavailable, authorized)),
        };
        let entry = match self.active_arm(
            &mut state,
            request.arm_id,
            request.expected_epoch,
            request.generation,
        ) {
            Ok(entry) => entry,
            Err(error) => return Err((error, authorized)),
        };
        let identity = authorized.identity();
        if entry.phase != PreclaimStateV1::Retiring
            || entry.retained_retirement.is_some()
            || !entry
                .signal
                .as_ref()
                .is_some_and(|signal| signal.audit_confirmed && signal.request == *request)
            || *identity.job_operation_id() != request.operation_id
            || identity.arm().operation_id != request.arm_id.to_string()
            || *identity.original_epoch() != request.expected_epoch
            || *identity.generation() != request.generation
            || *identity.retirement_id() != request.command_id
        {
            return Err((BrokerErrorCodeV1::Conflict, authorized));
        }
        let owner = Arc::new(RetirementSubmission::new(authorized));
        entry.retained_retirement = Some(RetainedRetirementV1::Submission {
            owner: Arc::clone(&owner),
        });
        Ok(owner)
    }

    /// Called under the actual cancellation gate immediately before the first
    /// possible command write. The canonical Arm slot must still own this
    /// exact process owner, with its same audited signal and no intervening STOP.
    pub(crate) fn mark_retirement_submitted(
        &self,
        request: &AbandonBeforeClaimV1,
        owner: &Arc<RetirementSubmission>,
    ) -> EnforcerResult<()> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        let entry = self
            .active_arm(
                &mut state,
                request.arm_id,
                request.expected_epoch,
                request.generation,
            )
            .map_err(|_| EnforcerError::NotReady)?;
        if entry.phase != PreclaimStateV1::Retiring
            || !entry
                .signal
                .as_ref()
                .is_some_and(|signal| signal.audit_confirmed && signal.request == *request)
            || !matches!(
                &entry.retained_retirement,
                Some(RetainedRetirementV1::Submission { owner: retained })
                    if Arc::ptr_eq(retained, owner)
            )
        {
            return Err(EnforcerError::Conflict);
        }
        owner.mark_submitted()
    }

    /// Clear only the same canonical owner after the enforcer client sealed
    /// strict Final plus actual local removal in its opaque verified wrapper.
    /// This is memory cleanup, not a new execution permission after Cancel.
    pub(crate) fn complete_retirement_submission(
        &self,
        request: &AbandonBeforeClaimV1,
        owner: &Arc<RetirementSubmission>,
        completion: &crate::broker::enforcer::retirement::VerifiedRetirementReceipt,
    ) -> Result<(), BrokerErrorCodeV1> {
        if !completion.completes_retained_submission(owner) {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let entry = matching_arm(
            &mut state,
            request.arm_id,
            request.expected_epoch,
            request.generation,
        )?;
        if !matches!(
            &entry.retained_retirement,
            Some(RetainedRetirementV1::Submission { owner: retained })
                if Arc::ptr_eq(retained, owner)
        ) {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        entry.retained_retirement = None;
        Ok(())
    }

    /// Borrow only the process owner already retained by this exact Arm. The
    /// snapshot identity selects an entry; it cannot create its capability.
    pub(crate) fn submitted_retirement_for_settlement(
        &self,
        identity: &RetirementIdentityV1,
    ) -> Result<Option<SubmittedRetirementSettlement>, BrokerErrorCodeV1> {
        self.select_retirement_for_settlement(identity, false)
    }

    /// Only an input retained by the private Final handoff may enter this lane.
    /// It does not make the ordinary query selector available or authorize Q.
    pub(crate) fn retained_final_for_settlement(
        &self,
        identity: &RetirementIdentityV1,
    ) -> Result<Option<SubmittedRetirementSettlement>, BrokerErrorCodeV1> {
        self.select_retirement_for_settlement(identity, true)
    }

    fn select_retirement_for_settlement(
        &self,
        identity: &RetirementIdentityV1,
        retained_only: bool,
    ) -> Result<Option<SubmittedRetirementSettlement>, BrokerErrorCodeV1> {
        if self.inner.unmatched_retirement.load(std::sync::atomic::Ordering::Acquire) {
            return Err(BrokerErrorCodeV1::NeedsReconciliation);
        }
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        if state.operation != Some(*identity.job_operation_id()) {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let arm = Uuid::parse_str(&identity.arm().operation_id)
            .map_err(|_| BrokerErrorCodeV1::Conflict)?;
        // Cleanup is not execution admission: cancellation may already have
        // changed the visible phase. Never use active_arm or reset its epoch.
        let entry = matching_arm(
            &mut state,
            arm,
            *identity.original_epoch(),
            *identity.generation(),
        )?;
        let Some(RetainedRetirementV1::Submission { owner }) = &entry.retained_retirement else {
            return Ok(None);
        };
        let eligible = if retained_only {
            owner.has_retryable_final()
        } else {
            owner.ready_for_settlement()
        }
        .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
        if !eligible {
            return Ok(None);
        }
        if owner
            .identity()
            .digest()
            .map_err(|_| BrokerErrorCodeV1::Conflict)?
            != identity.digest().map_err(|_| BrokerErrorCodeV1::Conflict)?
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let signal = entry
            .signal
            .as_ref()
            .filter(|signal| signal.audit_confirmed)
            .ok_or(BrokerErrorCodeV1::Conflict)?;
        if signal.request.operation_id != *identity.job_operation_id()
            || signal.request.arm_id != arm
            || signal.request.command_id != *identity.retirement_id()
            || signal.request.expected_epoch != *identity.original_epoch()
            || signal.request.generation != *identity.generation()
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        Ok(Some(SubmittedRetirementSettlement {
            request: signal.request.clone(),
            owner: Arc::clone(owner),
        }))
    }

    /// The caller retains the actual Job guard. Validation and reservation are
    /// synchronous; no IO or await occurs while either memory lock is held.
    pub(super) fn reserve_final_input<'guard>(
        &self,
        guard: &'guard super::jobs::RetirementQueryGuard,
        continuation: &SubmittedRetirementSettlement,
        input: VerifiedRetirementReceipt,
    ) -> Result<PendingFinalInput<'guard>, BrokerErrorCodeV1> {
        let accepted = (|| {
            if guard.operation_id() != continuation.request.operation_id
                || !Arc::ptr_eq(&self.inner, &guard.job().preclaim.inner)
                || self.inner.unmatched_retirement.load(std::sync::atomic::Ordering::Acquire)
            {
                return Err(BrokerErrorCodeV1::Conflict);
            }
            let mut state = self
                .inner
                .state
                .lock()
                .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
            matching_settlement_arm(&mut state, continuation)?;
            valid_final_input(continuation.owner(), &input)?;
            let mut submission = continuation
                .owner
                .state
                .lock()
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
            match &mut *submission {
                RetirementSubmissionState::Submitted { retained_verified }
                    if retained_verified.is_none() =>
                {
                    *retained_verified = Some(RetainedFinalInput::Borrowed);
                    Ok(())
                }
                _ => Err(BrokerErrorCodeV1::NeedsReconciliation),
            }
        })();
        if let Err(error) = accepted {
            self.retain_retirement(
                continuation.request.arm_id,
                continuation.request.expected_epoch,
                continuation.request.generation,
                RetainedRetirementV1::Verified {
                    _owner: Box::new(input),
                },
            );
            return Err(error);
        }
        Ok(self.final_input_token(guard, continuation, input))
    }

    /// Retrieve the very same retained input. A blocked legacy observation or
    /// another in-flight borrower never becomes a replacement query permit.
    pub(super) fn take_retained_final_input<'guard>(
        &self,
        guard: &'guard super::jobs::RetirementQueryGuard,
        continuation: &SubmittedRetirementSettlement,
    ) -> Result<Option<PendingFinalInput<'guard>>, BrokerErrorCodeV1> {
        if guard.operation_id() != continuation.request.operation_id
            || !Arc::ptr_eq(&self.inner, &guard.job().preclaim.inner)
            || self.inner.unmatched_retirement.load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        matching_settlement_arm(&mut state, continuation)?;
        let mut submission = continuation
            .owner
            .state
            .lock()
            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
        let RetirementSubmissionState::Submitted { retained_verified } = &mut *submission else {
            return Err(BrokerErrorCodeV1::NeedsReconciliation);
        };
        if retained_verified.is_none() {
            return Ok(None);
        }
        if !matches!(retained_verified, Some(RetainedFinalInput::Retryable(_))) {
            return Err(BrokerErrorCodeV1::NeedsReconciliation);
        }
        let input = match retained_verified.replace(RetainedFinalInput::Borrowed) {
            Some(RetainedFinalInput::Retryable(input)) => input,
            original => {
                *retained_verified = original;
                return Err(BrokerErrorCodeV1::NeedsReconciliation);
            }
        };
        // Release both locks before constructing or returning the local owner.
        drop(submission);
        drop(state);
        Ok(Some(self.final_input_token(guard, continuation, *input)))
    }

    fn final_input_token<'guard>(
        &self,
        guard: &'guard super::jobs::RetirementQueryGuard,
        continuation: &SubmittedRetirementSettlement,
        input: VerifiedRetirementReceipt,
    ) -> PendingFinalInput<'guard> {
        PendingFinalInput {
            _guard: guard,
            control: self.clone(),
            continuation: SubmittedRetirementSettlement {
                request: continuation.request.clone(),
                owner: Arc::clone(continuation.owner()),
            },
            input: Some(input),
        }
    }

    fn restore_final_input(
        &self,
        continuation: &SubmittedRetirementSettlement,
        mut input: Option<VerifiedRetirementReceipt>,
    ) {
        {
            // Poison recovery only keeps an already owned input. Normal
            // selectors and permission methods still fail closed on poison.
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if matching_settlement_arm(&mut state, continuation).is_ok() {
                let mut submission = continuation
                    .owner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let RetirementSubmissionState::Submitted { retained_verified } = &mut *submission
                    && matches!(retained_verified, Some(RetainedFinalInput::Borrowed))
                {
                    *retained_verified = input
                        .take()
                        .map(|input| RetainedFinalInput::Retryable(Box::new(input)));
                    return;
                }
            }
        }
        if let Some(input) = input {
            self.retain_retirement(
                continuation.request.arm_id,
                continuation.request.expected_epoch,
                continuation.request.generation,
                RetainedRetirementV1::Verified {
                    _owner: Box::new(input),
                },
            );
        }
    }

    /// Recheck the canonical holder after the last await, then perform only the
    /// synchronous local removal/clear. No IO or future is run under this lock.
    /// Lock order remains preclaim -> submission -> recovery gate -> slots.
    pub(crate) fn finish_retirement_settlement<F>(
        &self,
        continuation: &SubmittedRetirementSettlement,
        verified: &mut crate::broker::enforcer::retirement::VerifiedRetirementReceipt,
        remove_local: F,
    ) -> Result<(), BrokerErrorCodeV1>
    where
        F: FnOnce(
            &Arc<RetirementSubmission>,
            &mut crate::broker::enforcer::retirement::VerifiedRetirementReceipt,
        ) -> EnforcerResult<()>,
    {
        if self.inner.unmatched_retirement.load(std::sync::atomic::Ordering::Acquire) {
            return Err(BrokerErrorCodeV1::NeedsReconciliation);
        }
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let request = &continuation.request;
        if state.operation != Some(request.operation_id) {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        let entry = matching_arm(
            &mut state,
            request.arm_id,
            request.expected_epoch,
            request.generation,
        )?;
        if !entry
            .signal
            .as_ref()
            .is_some_and(|signal| signal.audit_confirmed && signal.request == *request)
            || !matches!(
                &entry.retained_retirement,
                Some(RetainedRetirementV1::Submission { owner })
                    if Arc::ptr_eq(owner, &continuation.owner)
            )
            || !continuation
                .owner
                .ready_for_settlement()
                .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?
        {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        remove_local(&continuation.owner, verified)
            .map_err(|_| BrokerErrorCodeV1::NeedsReconciliation)?;
        if !verified.completes_retained_submission(&continuation.owner) {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        // The sealed Q is borrowed only; putting it into this owner would make
        // local_completion's strong back-reference an ownership cycle.
        entry.retained_retirement = None;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn retirement_submission_for_test(
        &self,
        arm: Uuid,
    ) -> Option<Arc<RetirementSubmission>> {
        let state = self.inner.state.lock().ok()?;
        match &state.arms.get(&arm)?.retained_retirement {
            Some(RetainedRetirementV1::Submission { owner }) => Some(Arc::clone(owner)),
            _ => None,
        }
    }

    fn view(&self, arm: Uuid, entry: Option<&Arm>) -> PreclaimArmViewV1 {
        let current = entry
            .map(|entry| {
                if self.current(entry.generation).is_ok() {
                    entry.phase
                } else {
                    PreclaimStateV1::StopReserved
                }
            })
            .unwrap_or(PreclaimStateV1::Unknown);
        PreclaimArmViewV1 {
            arm_id: arm,
            expected_epoch: entry.map(|entry| entry.epoch),
            generation: entry.map(|entry| entry.generation),
            state: current,
            command_id: entry
                .and_then(|entry| entry.signal.as_ref())
                .map(|signal| signal.request.command_id),
            audit_confirmed: entry
                .and_then(|entry| entry.signal.as_ref())
                .is_some_and(|signal| signal.audit_confirmed),
            retirement_continuation_retained: if self
                .inner
                .unmatched_retirement
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                None
            } else {
                entry.map(|entry| entry.retained_retirement.is_some())
            },
            source: "process_local",
            freshness: if entry.is_some() {
                "current_process_state"
            } else {
                "unknown"
            },
        }
    }

    pub(crate) fn arm_view(&self, arm: Uuid) -> PreclaimArmViewV1 {
        match self.inner.state.lock() {
            Ok(state) => self.view(arm, state.arms.get(&arm)),
            Err(_) => self.view(arm, None),
        }
    }

    pub(crate) fn has_retained_retirement(&self) -> bool {
        if self
            .inner
            .unmatched_retirement
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return true;
        }
        self.inner
            .state
            .lock()
            .map(|state| {
                state
                    .arms
                    .values()
                    .any(|entry| entry.retained_retirement.is_some())
            })
            .unwrap_or(true)
    }
}

/// Single-use local branch reservation. No serialization, Clone or authority to
/// issue a control claim; consuming it still checks the retained cancellation.
pub(crate) struct PreclaimClaimReservationV1 {
    control: PreclaimControlV1,
    arm: Uuid,
    epoch: Uuid,
    generation: u64,
}

impl PreclaimClaimReservationV1 {
    pub(crate) fn consume(self) -> Result<(), BrokerErrorCodeV1> {
        let mut state = self
            .control
            .inner
            .state
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let entry = self
            .control
            .active_arm(&mut state, self.arm, self.epoch, self.generation)?;
        if entry.phase != PreclaimStateV1::ClaimReserved {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        self.control.current(self.generation)
    }
}

impl fmt::Debug for PreclaimClaimReservationV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PreclaimClaimReservationV1(<local branch>)")
    }
}

/// A retained request and its immutable event. This is process data, not proof
/// that an audit exists or that unused-slot retirement is authorized.
pub(crate) struct PreclaimSignalReservationV1 {
    control: PreclaimControlV1,
    request: AbandonBeforeClaimV1,
    event: Arc<Event>,
}

impl PreclaimSignalReservationV1 {
    #[cfg(test)]
    pub(crate) fn request(&self) -> &AbandonBeforeClaimV1 {
        &self.request
    }
    pub(crate) fn event(&self) -> &Event {
        &self.event
    }

    pub(crate) fn state(&self) -> Result<PreclaimArmViewV1, BrokerErrorCodeV1> {
        let mut state = self
            .control
            .inner
            .state
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let entry = matching_arm(
            &mut state,
            self.request.arm_id,
            self.request.expected_epoch,
            self.request.generation,
        )?;
        if entry.signal.as_ref().map(|signal| &signal.request) != Some(&self.request) {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        Ok(self.control.view(self.request.arm_id, Some(entry)))
    }

    /// Trusted coordinator metadata after its separate durable ACK check. This
    /// bool never constructs AuthorizedRetirement, Unused or a verified receipt.
    /// A late ACK is retained even when cancellation has already inhibited it.
    pub(crate) fn confirm_audit(&self) -> Result<PreclaimArmViewV1, BrokerErrorCodeV1> {
        let mut state = self
            .control
            .inner
            .state
            .lock()
            .map_err(|_| BrokerErrorCodeV1::Unavailable)?;
        let entry = matching_arm(
            &mut state,
            self.request.arm_id,
            self.request.expected_epoch,
            self.request.generation,
        )?;
        let signal = entry.signal.as_mut().ok_or(BrokerErrorCodeV1::Conflict)?;
        if signal.request != self.request {
            return Err(BrokerErrorCodeV1::Conflict);
        }
        signal.audit_confirmed = true;
        if entry.phase == PreclaimStateV1::AbandonPendingAudit {
            entry.phase = PreclaimStateV1::AbandonRequested;
        }
        let view = self.control.view(self.request.arm_id, Some(entry));
        drop(state);
        self.control.inner.changed.notify_waiters();
        Ok(view)
    }

    /// Call only after signal_for_arm returned Some. No ordinary registration
    /// waits for an operator. The caller retains its real Arm deadline/STOP.
    /// Periodic reads notice CancelGeneration even without a Notify producer.
    pub(crate) async fn wait_audit_confirmed(
        &self,
    ) -> Result<AbandonBeforeClaimV1, BrokerErrorCodeV1> {
        loop {
            match self.state()?.state {
                PreclaimStateV1::AbandonRequested | PreclaimStateV1::Retiring => {
                    return Ok(self.request.clone());
                }
                PreclaimStateV1::AbandonPendingAudit => {}
                PreclaimStateV1::StopReserved => return Err(BrokerErrorCodeV1::Interrupted),
                _ => return Err(BrokerErrorCodeV1::Conflict),
            }
            tokio::select! {
                _ = self.control.inner.changed.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(25)) => {}
            }
        }
    }
}

impl fmt::Debug for PreclaimSignalReservationV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PreclaimSignalReservationV1(<process request>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Barrier, thread};

    fn request() -> AbandonBeforeClaimV1 {
        AbandonBeforeClaimV1 {
            version: 1,
            operation_id: Uuid::from_u128(1),
            arm_id: Uuid::from_u128(2),
            command_id: Uuid::from_u128(3),
            expected_epoch: Uuid::from_u128(4),
            generation: 0,
        }
    }

    fn announced(cancel: CancelGeneration) -> PreclaimControlV1 {
        let control = PreclaimControlV1::new(cancel);
        let request = request();
        control
            .announce_registration(
                request.operation_id,
                request.arm_id,
                request.expected_epoch,
                request.generation,
            )
            .unwrap();
        control
    }

    #[test]
    fn preclaim_claim_and_abandon_compete_under_one_arbitration() {
        let control = announced(CancelGeneration::new());
        let barrier = Arc::new(Barrier::new(3));
        let claim_control = control.clone();
        let claim_barrier = Arc::clone(&barrier);
        let claim = thread::spawn(move || {
            let request = request();
            claim_barrier.wait();
            claim_control.choose_claim(request.arm_id, request.expected_epoch, request.generation)
        });
        let request_control = control.clone();
        let request_barrier = Arc::clone(&barrier);
        let abandon = thread::spawn(move || {
            request_barrier.wait();
            request_control.request(request())
        });
        barrier.wait();
        let claim = claim.join().unwrap();
        let abandon = abandon.join().unwrap();
        match (claim, abandon) {
            (Ok(claim), Err(BrokerErrorCodeV1::Conflict)) => {
                claim.consume().unwrap();
                assert_eq!(
                    control.arm_view(request().arm_id).state,
                    PreclaimStateV1::ClaimReserved
                );
            }
            (Err(BrokerErrorCodeV1::Conflict), Ok(signal)) => {
                assert_eq!(
                    signal.state().unwrap().state,
                    PreclaimStateV1::AbandonPendingAudit
                );
            }
            _ => panic!("exactly one preclaim selection must win"),
        }
    }

    #[tokio::test]
    async fn preclaim_cancel_before_audit_ack_never_delivers_a_signal() {
        let cancel = CancelGeneration::new();
        let control = announced(cancel.clone());
        let signal = control.request(request()).unwrap();
        let waiter = control.request(request()).unwrap();
        let waiting = tokio::spawn(async move { waiter.wait_audit_confirmed().await });
        cancel.invalidate();
        let view = signal.confirm_audit().unwrap();
        assert!(view.audit_confirmed);
        assert_eq!(view.state, PreclaimStateV1::StopReserved);
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), waiting)
                .await
                .unwrap()
                .unwrap(),
            Err(BrokerErrorCodeV1::Interrupted)
        ));
        let request = request();
        assert!(matches!(
            control.choose_claim(request.arm_id, request.expected_epoch, request.generation),
            Err(BrokerErrorCodeV1::Interrupted)
        ));
        let mut fresh_command = request.clone();
        fresh_command.command_id = Uuid::from_u128(5);
        assert!(matches!(
            control.request(fresh_command),
            Err(BrokerErrorCodeV1::Interrupted)
        ));
        // Exact replay remains a read of the same inhibited process state.
        assert_eq!(
            control.request(request).unwrap().state().unwrap().state,
            PreclaimStateV1::StopReserved
        );
    }

    #[test]
    fn preclaim_replay_retains_exact_signal_event_and_rejects_divergence() {
        let control = announced(CancelGeneration::new());
        let first = control.request(request()).unwrap();
        let replay = control.request(request()).unwrap();
        assert_eq!(first.request(), replay.request());
        assert_eq!(
            serde_json::to_value(first.event()).unwrap(),
            serde_json::to_value(replay.event()).unwrap()
        );
        first.event().validate().unwrap();
        assert_eq!(first.event().id, "00000000-0000-0000-0000-000000000003");
        assert_eq!(first.event().kind, "broker.abandon-before-claim.requested");
        assert_eq!(first.event().source, "ai-guard-broker");
        assert_eq!(first.event().provenance, "observed");
        assert_eq!(first.event().coverage, "native");
        assert_eq!(
            first.event().data,
            serde_json::json!({
                "version": 1,
                "operation_id": "00000000-0000-0000-0000-000000000001",
                "arm_id": "00000000-0000-0000-0000-000000000002",
                "command_id": "00000000-0000-0000-0000-000000000003",
                "expected_epoch": "00000000-0000-0000-0000-000000000004",
                "generation": 0,
            })
        );
        let mut divergent = request();
        divergent.expected_epoch = Uuid::from_u128(8);
        assert!(matches!(
            control.request(divergent),
            Err(BrokerErrorCodeV1::Conflict)
        ));
        let mut conflicting_command = request();
        conflicting_command.command_id = Uuid::from_u128(9);
        assert!(matches!(
            control.request(conflicting_command),
            Err(BrokerErrorCodeV1::Conflict)
        ));
        assert_eq!(
            control.arm_view(request().arm_id).command_id,
            Some(Uuid::from_u128(3))
        );
        let mut reassigned = request();
        reassigned.arm_id = Uuid::from_u128(10);
        control
            .announce_registration(
                reassigned.operation_id,
                reassigned.arm_id,
                reassigned.expected_epoch,
                reassigned.generation,
            )
            .unwrap();
        assert!(matches!(
            control.request(reassigned),
            Err(BrokerErrorCodeV1::Conflict)
        ));
    }

    #[tokio::test]
    async fn preclaim_dropped_signal_reservation_keeps_claim_closed_through_registration() {
        let control = announced(CancelGeneration::new());
        drop(control.request(request()).unwrap());
        let input = request();
        control
            .mark_original_unclaimed(input.arm_id, input.expected_epoch, input.generation)
            .unwrap();
        assert!(matches!(
            control.choose_claim(input.arm_id, input.expected_epoch, input.generation),
            Err(BrokerErrorCodeV1::Conflict)
        ));
        let signal = control
            .signal_for_arm(input.arm_id, input.expected_epoch, input.generation)
            .unwrap()
            .unwrap();
        assert_eq!(
            signal.state().unwrap().state,
            PreclaimStateV1::AbandonPendingAudit
        );
        signal.confirm_audit().unwrap();
        assert_eq!(signal.wait_audit_confirmed().await.unwrap(), input);
        control
            .mark_retiring(input.arm_id, input.expected_epoch, input.generation)
            .unwrap();
        assert_eq!(
            control.arm_view(input.arm_id).state,
            PreclaimStateV1::Retiring
        );
    }

    #[test]
    fn preclaim_dropped_claim_reservation_cannot_be_abandoned_or_reissued() {
        let control = announced(CancelGeneration::new());
        let input = request();
        drop(
            control
                .choose_claim(input.arm_id, input.expected_epoch, input.generation)
                .unwrap(),
        );
        assert!(matches!(
            control.request(input.clone()),
            Err(BrokerErrorCodeV1::Conflict)
        ));
        assert!(matches!(
            control.choose_claim(input.arm_id, input.expected_epoch, input.generation),
            Err(BrokerErrorCodeV1::Conflict)
        ));
    }

    #[test]
    fn preclaim_reserved_claim_is_unusable_after_cancellation() {
        let cancel = CancelGeneration::new();
        let control = announced(cancel.clone());
        let input = request();
        let claim = control
            .choose_claim(input.arm_id, input.expected_epoch, input.generation)
            .unwrap();
        cancel.invalidate();
        assert!(matches!(
            claim.consume(),
            Err(BrokerErrorCodeV1::Interrupted)
        ));
        assert_eq!(
            control.arm_view(input.arm_id).state,
            PreclaimStateV1::StopReserved
        );
    }

    #[tokio::test]
    async fn preclaim_explicit_stop_preempts_a_selected_signal_before_audit_ack() {
        let control = announced(CancelGeneration::new());
        let input = request();
        let signal = control.request(input.clone()).unwrap();
        control
            .mark_stop_reserved(input.arm_id, input.expected_epoch, input.generation)
            .unwrap();
        assert_eq!(
            signal.confirm_audit().unwrap().state,
            PreclaimStateV1::StopReserved
        );
        assert!(matches!(
            signal.wait_audit_confirmed().await,
            Err(BrokerErrorCodeV1::Interrupted)
        ));
        assert!(matches!(
            control.mark_retiring(input.arm_id, input.expected_epoch, input.generation),
            Err(BrokerErrorCodeV1::Conflict)
        ));
    }

    #[test]
    fn preclaim_stop_unknown_and_old_identity_never_enable_abandonment() {
        let cancel = CancelGeneration::new();
        let control = announced(cancel.clone());
        let input = request();
        assert_eq!(
            control.arm_view(Uuid::from_u128(90)).state,
            PreclaimStateV1::Unknown
        );
        let mut unknown = input.clone();
        unknown.arm_id = Uuid::from_u128(90);
        assert!(matches!(
            control.request(unknown),
            Err(BrokerErrorCodeV1::NeedsReconciliation)
        ));
        assert!(matches!(
            control.announce_registration(input.operation_id, input.arm_id, Uuid::from_u128(91), 0),
            Err(BrokerErrorCodeV1::Conflict)
        ));
        control
            .mark_stop_reserved(input.arm_id, input.expected_epoch, 0)
            .unwrap();
        assert!(matches!(
            control.request(input.clone()),
            Err(BrokerErrorCodeV1::Conflict)
        ));
        cancel.invalidate();
        assert!(matches!(
            control.announce_registration(
                input.operation_id,
                Uuid::from_u128(92),
                input.expected_epoch,
                0
            ),
            Err(BrokerErrorCodeV1::Interrupted)
        ));
        assert!(matches!(
            control.announce_registration(
                Uuid::from_u128(93),
                Uuid::from_u128(94),
                input.expected_epoch,
                1
            ),
            Err(BrokerErrorCodeV1::Conflict)
        ));
    }

    #[test]
    fn preclaim_capacity_is_global_and_exact_replays_do_not_consume_another_slot() {
        let control = PreclaimControlV1::with_test_capacity(CancelGeneration::new(), 2);
        let input = request();
        for arm in [input.arm_id, Uuid::from_u128(10)] {
            control
                .announce_registration(input.operation_id, arm, input.expected_epoch, 0)
                .unwrap();
        }
        control
            .announce_registration(input.operation_id, input.arm_id, input.expected_epoch, 0)
            .unwrap();
        assert!(matches!(
            control.announce_registration(
                input.operation_id,
                Uuid::from_u128(11),
                input.expected_epoch,
                0
            ),
            Err(BrokerErrorCodeV1::LimitExceeded)
        ));
        control.request(input.clone()).unwrap();
        control.request(input.clone()).unwrap();
        let mut second = input;
        second.arm_id = Uuid::from_u128(10);
        second.command_id = Uuid::from_u128(12);
        control.request(second).unwrap();
        let state = control.inner.state.lock().unwrap();
        assert_eq!(state.arms.len(), 2);
        assert_eq!(state.commands.len(), 2);
    }

    #[test]
    fn preclaim_dto_rejects_noncanonical_nil_unknown_version_and_generation_saturation() {
        let input = request();
        let value = serde_json::to_value(&input).unwrap();
        assert_eq!(
            serde_json::from_value::<AbandonBeforeClaimV1>(value.clone()).unwrap(),
            input
        );
        for field in ["operation_id", "arm_id", "command_id", "expected_epoch"] {
            for invalid in [
                "00000000-0000-0000-0000-000000000000",
                "00000000000000000000000000000001",
                "00000000-0000-0000-0000-00000000000A",
            ] {
                let mut invalid_value = value.clone();
                invalid_value[field] = invalid.into();
                assert!(serde_json::from_value::<AbandonBeforeClaimV1>(invalid_value).is_err());
            }
        }
        for (field, invalid) in [
            ("version", serde_json::json!(2)),
            ("generation", serde_json::json!(u64::MAX)),
            ("unexpected", serde_json::json!(true)),
        ] {
            let mut invalid_value = value.clone();
            invalid_value[field] = invalid;
            assert!(serde_json::from_value::<AbandonBeforeClaimV1>(invalid_value).is_err());
        }
    }
}
