//! Derived index of the complete V1/V2 prefix. No record grants exec authority.
use super::*;
use crate::{
    control::{MutationReceiptV1, RuntimeStatusV1, WorkerMutationV1, WorkerTransitionV1},
    relay::admission::*,
    runner::relay_admission::{
        validate_relay_channel_history, validate_relay_channel_transition_result,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChannelOperationPurposeV2 {
    Attachment,
    Retirement,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ChannelResolutionV2 {
    History {
        response: RelayChannelReconciliationV1,
    },
    Mutation {
        response: RelayChannelResultV1,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelOperationV2 {
    pub request: RelayChannelPendingV1,
    pub purpose: ChannelOperationPurposeV2,
    pub resolution: Option<ChannelResolutionV2>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelTerminalV2 {
    pub request: WorkerMutationV1,
    pub receipt: MutationReceiptV1,
    pub worker_revision: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelHistoryV2 {
    pub fact: ChannelFact,
    pub legacy_closed: bool,
    pub operations: Vec<ChannelOperationV2>,
    pub admission_proof: Option<RelayChannelReconciliationV1>,
    pub terminal: Option<ChannelTerminalV2>,
    pub ordinary_records: usize,
    pub terminal_records: usize,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ChannelChangeV2 {
    Fact {
        fact: ChannelFact,
    },
    BeginRetirement {
        fact: ChannelFact,
        terminal: ChannelTerminalV2,
    },
    History {
        pending_operation_id: Option<String>,
        response: RelayChannelReconciliationV1,
    },
    RetirementIntent {
        request: RelayChannelTransitionV1,
    },
    Mutation {
        operation_id: String,
        response: RelayChannelResultV1,
    },
    Finalize {
        fact: ChannelFact,
    },
}

#[derive(Clone)]
pub(super) struct State {
    pub history: ChannelHistoryV2,
    pub v2: bool,
    head: Option<(i64, String)>,
    seen_proofs: std::collections::BTreeSet<String>,
}

fn ensure(value: bool) -> Result<()> {
    if value {
        Ok(())
    } else {
        Err(RelayOwnerError::Scope)
    }
}
pub(super) fn same<T: Serialize>(a: &T, b: &T) -> bool {
    match (
        super::super::super::journal::canonical(a),
        super::super::super::journal::canonical(b),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
fn timestamp(value: &str) -> Result<()> {
    ensure(value.len() <= 64 && chrono::DateTime::parse_from_rfc3339(value).is_ok())
}

impl State {
    pub(super) fn initial(fact: ChannelFact) -> Self {
        Self {
            history: ChannelHistoryV2 {
                legacy_closed: fact.phase == AttachmentPhase::Closed,
                fact,
                operations: Vec::new(),
                admission_proof: None,
                terminal: None,
                ordinary_records: 0,
                terminal_records: 0,
            },
            v2: false,
            head: None,
            seen_proofs: Default::default(),
        }
    }
    pub(super) fn count(&self) -> usize {
        self.history.ordinary_records + self.history.terminal_records
    }
    fn admission(&self) -> Result<&RelayChannelAdmissionV1> {
        self.history
            .fact
            .admission
            .as_ref()
            .ok_or(RelayOwnerError::Scope)
    }
    fn insert(
        &mut self,
        request: RelayChannelPendingV1,
        purpose: ChannelOperationPurposeV2,
    ) -> Result<()> {
        let admission = self.admission()?;
        request
            .validate(admission)
            .map_err(|_| RelayOwnerError::Scope)?;
        ensure(request.operation_id() != admission.operation_id)?;
        if let Some(previous) = self
            .history
            .operations
            .iter()
            .find(|op| op.request.operation_id() == request.operation_id())
        {
            return ensure(same(&previous.request, &request) && previous.purpose == purpose);
        }
        ensure(self.history.operations.len() < PER_JOB)?;
        self.history.operations.push(ChannelOperationV2 {
            request,
            purpose,
            resolution: None,
        });
        Ok(())
    }
    fn index_fact(&mut self, fact: &ChannelFact, legacy: bool) -> Result<()> {
        if let Some(admission) = &fact.admission {
            ensure(
                self.history
                    .operations
                    .iter()
                    .all(|op| op.request.operation_id() != admission.operation_id),
            )?;
        }
        if let Some(claim) = &fact.claim {
            self.insert(
                RelayChannelPendingV1::Claim(claim.clone()),
                ChannelOperationPurposeV2::Attachment,
            )?;
        }
        if let Some(transition) = &fact.transition {
            let known = self
                .history
                .operations
                .iter()
                .any(|op| op.request.operation_id() == transition.operation_id);
            if !legacy && !known {
                ensure(
                    fact.core
                        .as_ref()
                        .is_some_and(|core| core.revision == transition.expected_revision),
                )?;
            }
            self.insert(
                RelayChannelPendingV1::Transition(transition.clone()),
                ChannelOperationPurposeV2::Attachment,
            )?;
        }
        Ok(())
    }
    pub(super) fn legacy_fact(
        &mut self,
        fact: &ChannelFact,
        scope: &JournalScopeV1,
        first: bool,
    ) -> Result<()> {
        ensure(!self.v2)?;
        validate(fact)?;
        validate_scope(fact, scope)?;
        if !first {
            validate_next(&self.history.fact, fact)?;
        }
        self.history.fact = fact.clone();
        self.index_fact(fact, true)?;
        self.history.ordinary_records += 1;
        self.history.legacy_closed = fact.phase == AttachmentPhase::Closed;
        Ok(())
    }
    fn snapshot(&mut self, snapshot: &RelayChannelSnapshotV1) -> Result<bool> {
        let mut candidate = self.history.fact.clone();
        candidate.core = Some(snapshot.clone());
        // A snapshot is data, never an opportunity to publish Closed.
        candidate.phase = AttachmentPhase::Unknown;
        validate(&candidate)?;
        if let Some(previous) = &self.history.fact.core {
            ensure(
                snapshot.revision >= previous.revision
                    && (snapshot.revision != previous.revision || snapshot == previous),
            )?;
        }
        let progress = self.history.fact.core.as_ref() != Some(snapshot);
        self.history.fact.core = Some(snapshot.clone());
        Ok(progress)
    }
    fn terminal_matches(&self, proof: &ChannelTerminalV2) -> Result<()> {
        let request = &proof.request;
        let receipt = &proof.receipt;
        ensure(
            request.version == 1
                && request.worker_id == self.history.fact.endpoint.worker_id
                && request.attempt_id == self.history.fact.endpoint.attempt_id
                && request.expected_revision > 0
                && proof.worker_revision < i64::MAX as u64
                && request.expected_revision.checked_add(1) == receipt.revision
                && receipt
                    .revision
                    .is_some_and(|revision| revision <= proof.worker_revision)
                && receipt.operation_id == request.operation_id
                && receipt.worker_id.as_deref() == Some(request.worker_id.as_str())
                && receipt.audit_seq > 0
                && crate::control::hash(&receipt.audit_hash).is_ok()
                && crate::control::content_hash("ai-guard:worker-mutation:v1", request)
                    .ok()
                    .as_ref()
                    == Some(&receipt.input_hash),
        )?;
        for text in [
            &request.operation_id,
            &request.worker_id,
            &request.attempt_id,
        ] {
            ensure(
                uuid::Uuid::parse_str(text).is_ok_and(|id| !id.is_nil() && id.to_string() == *text),
            )?;
        }
        let WorkerTransitionV1::Observe { observation } = &request.transition else {
            return Err(RelayOwnerError::Scope);
        };
        timestamp(&observation.observed_at)?;
        ensure(
            matches!(
                observation.status,
                RuntimeStatusV1::Exited | RuntimeStatusV1::Dead
            ) && observation.running == Some(false)
                && observation.restart_count == Some(0),
        )?;
        let started = observation
            .started_at
            .as_deref()
            .ok_or(RelayOwnerError::Scope)?;
        let finished = observation
            .finished_at
            .as_deref()
            .ok_or(RelayOwnerError::Scope)?;
        timestamp(started)?;
        timestamp(finished)?;
        ensure(
            chrono::DateTime::parse_from_rfc3339(finished).map_err(|_| RelayOwnerError::Scope)?
                >= chrono::DateTime::parse_from_rfc3339(started)
                    .map_err(|_| RelayOwnerError::Scope)?,
        )?;
        if let Some(admission) = &self.history.fact.admission {
            ensure(
                observation.inspection == admission.inspection
                    && crate::relay::admission::time(started).ok()
                        == crate::relay::admission::time(&admission.started_at).ok(),
            )?;
        }
        Ok(())
    }
    fn observe_head(&mut self, response: &RelayChannelReconciliationV1) -> Result<()> {
        if let Some((seq, hash)) = &self.head {
            ensure(
                response.audit_seq >= *seq
                    && (response.audit_seq != *seq || response.audit_hash == *hash),
            )?;
        }
        self.head = Some((response.audit_seq, response.audit_hash.clone()));
        Ok(())
    }
    fn final_class(resolution: &ChannelResolutionV2) -> Result<Option<RelayChannelReceiptV1>> {
        match resolution {
            ChannelResolutionV2::Mutation { response } => Ok(Some(response.receipt.clone())),
            ChannelResolutionV2::History { response } => match &response.outcome {
                RelayChannelHistoryOutcomeV1::Historical {
                    pending: Some(pending),
                    ..
                } => match &pending.resolution {
                    RelayChannelPendingResolutionV1::Recorded { receipt } => {
                        Ok(Some(receipt.clone()))
                    }
                    RelayChannelPendingResolutionV1::InapplicableCas
                    | RelayChannelPendingResolutionV1::InapplicableState => Ok(None),
                    RelayChannelPendingResolutionV1::Unresolved => Err(RelayOwnerError::Scope),
                },
                RelayChannelHistoryOutcomeV1::AdmissionNeverRecordedExactTerminal { .. } => {
                    Ok(None)
                }
                _ => Err(RelayOwnerError::Scope),
            },
        }
    }
    fn resolve(&mut self, index: usize, resolution: ChannelResolutionV2) -> Result<bool> {
        let class = Self::final_class(&resolution)?;
        if let Some(previous) = &self.history.operations[index].resolution {
            ensure(Self::final_class(previous)? == class)?;
            return Ok(false);
        }
        self.history.operations[index].resolution = Some(resolution);
        Ok(true)
    }
    fn proof(
        &mut self,
        operation_id: Option<&str>,
        response: &RelayChannelReconciliationV1,
        scope: &JournalScopeV1,
    ) -> Result<bool> {
        ensure(self.history.terminal.is_some())?;
        let index = operation_id
            .map(|id| {
                self.history
                    .operations
                    .iter()
                    .position(|op| op.request.operation_id() == id)
                    .ok_or(RelayOwnerError::Scope)
            })
            .transpose()?;
        let request = RelayChannelReconcileV1 {
            version: 1,
            admission: self.admission()?.clone(),
            pending: index.map(|i| self.history.operations[i].request.clone()),
        };
        validate_relay_channel_history(scope.tenant_id(), &request, response)
            .map_err(|_| RelayOwnerError::Scope)?;
        let proof_hash = crate::control::content_hash("ai-guard:relay-owner:history:v2", response)
            .map_err(|_| RelayOwnerError::InvalidMaterial)?;
        if self.seen_proofs.contains(&proof_hash) {
            return Ok(false);
        }
        let mut progress = false;
        match &response.outcome {
            RelayChannelHistoryOutcomeV1::Historical {
                channel, pending, ..
            } => {
                ensure(!matches!(
                    self.history.admission_proof.as_ref().map(|p| &p.outcome),
                    Some(RelayChannelHistoryOutcomeV1::AdmissionNeverRecordedExactTerminal { .. })
                ))?;
                progress |= self.snapshot(channel)?;
                progress |= self.history.admission_proof.is_none();
                if let (Some(index), Some(pending)) = (index, pending) {
                    if pending.resolution != RelayChannelPendingResolutionV1::Unresolved {
                        progress |= self.resolve(
                            index,
                            ChannelResolutionV2::History {
                                response: response.clone(),
                            },
                        )?;
                    } else {
                        ensure(self.history.operations[index].resolution.is_none())?;
                    }
                }
            }
            RelayChannelHistoryOutcomeV1::AdmissionNeverRecordedExactTerminal {
                worker_revision,
                terminal_observation_receipt,
                ..
            } => {
                let terminal = self
                    .history
                    .terminal
                    .as_ref()
                    .ok_or(RelayOwnerError::Scope)?;
                ensure(
                    self.history.fact.core.is_none()
                        && terminal.worker_revision == *worker_revision
                        && same(&terminal.receipt, terminal_observation_receipt),
                )?;
                progress |= self.history.admission_proof.is_none();
                for index in 0..self.history.operations.len() {
                    progress |= self.resolve(
                        index,
                        ChannelResolutionV2::History {
                            response: response.clone(),
                        },
                    )?;
                }
            }
            RelayChannelHistoryOutcomeV1::AdmissionUnresolved => return Ok(false),
        }
        if progress {
            self.observe_head(response)?;
            self.history.admission_proof = Some(response.clone());
            self.seen_proofs.insert(proof_hash);
        }
        Ok(progress)
    }
    pub(super) fn apply(
        &mut self,
        change: &ChannelChangeV2,
        scope: &JournalScopeV1,
        observed_at: &str,
        first: bool,
    ) -> Result<Option<bool>> {
        timestamp(observed_at)?;
        let before = self.history.clone();
        match change {
            ChannelChangeV2::Fact { fact }
            | ChannelChangeV2::BeginRetirement { fact, .. }
            | ChannelChangeV2::Finalize { fact } => ensure(fact.job_id == before.fact.job_id)?,
            _ => {}
        }
        let terminal_unit = match change {
            ChannelChangeV2::Fact { fact } => {
                let mut normalized = fact.clone();
                normalized.observed_at = before.fact.observed_at.clone();
                if same(&normalized, &before.fact) && !first {
                    return Ok(None);
                }
                ensure(before.terminal.is_none() && fact.phase != AttachmentPhase::Closed)?;
                validate(fact)?;
                validate_scope(fact, scope)?;
                if !first {
                    validate_next(&before.fact, fact)?;
                }
                self.history.fact = fact.clone();
                self.index_fact(fact, false)?;
                false
            }
            ChannelChangeV2::BeginRetirement { fact, terminal } => {
                if let Some(previous) = &before.terminal {
                    ensure(same(previous, terminal))?;
                    let mut normalized = fact.clone();
                    normalized.observed_at = before.fact.observed_at.clone();
                    ensure(same(&normalized, &before.fact))?;
                    return Ok(None);
                }
                ensure(
                    !first
                        && before.fact.phase != AttachmentPhase::Closed
                        && fact.phase == AttachmentPhase::Unknown
                        && fact.local_joined
                        && fact.error.is_some()
                        && before
                            .fact
                            .error
                            .is_none_or(|error| fact.error == Some(error)),
                )?;
                validate(fact)?;
                validate_scope(fact, scope)?;
                validate_next(&before.fact, fact)?;
                ensure(
                    same(&before.fact.admission, &fact.admission)
                        && same(&before.fact.claim, &fact.claim)
                        && same(&before.fact.transition, &fact.transition)
                        && before.fact.core == fact.core,
                )?;
                self.terminal_matches(terminal)?;
                self.history.fact = fact.clone();
                self.history.terminal = Some(terminal.clone());
                true
            }
            ChannelChangeV2::History {
                pending_operation_id,
                response,
            } => {
                if !self.proof(pending_operation_id.as_deref(), response, scope)? {
                    return Ok(None);
                }
                true
            }
            ChannelChangeV2::RetirementIntent { request } => {
                ensure(before.terminal.is_some() && before.fact.phase != AttachmentPhase::Closed)?;
                request.validate().map_err(|_| RelayOwnerError::Scope)?;
                let pending = RelayChannelPendingV1::Transition(request.clone());
                pending
                    .validate(self.admission()?)
                    .map_err(|_| RelayOwnerError::Scope)?;
                if let Some(previous) = before
                    .operations
                    .iter()
                    .find(|op| op.request.operation_id() == request.operation_id)
                {
                    ensure(
                        previous.purpose == ChannelOperationPurposeV2::Retirement
                            && same(&previous.request, &pending),
                    )?;
                    return Ok(None);
                }
                ensure(before.operations.iter().all(|op| {
                    op.purpose != ChannelOperationPurposeV2::Retirement || op.resolution.is_some()
                }))?;
                let core = before.fact.core.as_ref().ok_or(RelayOwnerError::Scope)?;
                ensure(
                    core.revision == request.expected_revision
                        && request
                            .expected_revision
                            .checked_add(1)
                            .is_some_and(|revision| revision < i64::MAX as u64)
                        && matches!(before.admission_proof.as_ref().map(|proof| &proof.outcome), Some(RelayChannelHistoryOutcomeV1::Historical { channel, .. }) if channel == core),
                )?;
                match &request.transition {
                    RelayChannelTransitionKindV1::CancelReserved => {
                        ensure(core.state == RelayChannelStateV1::Reserved)?
                    }
                    RelayChannelTransitionKindV1::CloseExactWorkerTerminal {
                        worker_revision,
                        terminal_observation_id,
                    } => {
                        let terminal = before.terminal.as_ref().ok_or(RelayOwnerError::Scope)?;
                        ensure(
                            *worker_revision == terminal.worker_revision
                                && terminal_observation_id == &terminal.request.operation_id
                                && request.transition.target_state(core.state)
                                    == Some(RelayChannelStateV1::Closed),
                        )?;
                    }
                    _ => return Err(RelayOwnerError::Scope),
                }
                self.insert(pending, ChannelOperationPurposeV2::Retirement)?;
                true
            }
            ChannelChangeV2::Mutation {
                operation_id,
                response,
            } => {
                let index = before
                    .operations
                    .iter()
                    .position(|op| op.request.operation_id() == operation_id)
                    .ok_or(RelayOwnerError::Scope)?;
                let RelayChannelPendingV1::Transition(request) = &before.operations[index].request
                else {
                    return Err(RelayOwnerError::Scope);
                };
                validate_relay_channel_transition_result(
                    scope.tenant_id(),
                    self.admission()?,
                    request,
                    response,
                )
                .map_err(|_| RelayOwnerError::Scope)?;
                if let Some(previous) = &before.operations[index].resolution {
                    if Self::final_class(previous)?.as_ref() == Some(&response.receipt) {
                        if let Some(core) = &before.fact.core {
                            ensure(
                                response.channel.revision != core.revision
                                    || response.channel == *core,
                            )?;
                            if response.channel.revision <= core.revision {
                                return Ok(None);
                            }
                        }
                    }
                }
                if matches!(
                    request.transition,
                    RelayChannelTransitionKindV1::CancelReserved
                        | RelayChannelTransitionKindV1::KnownRefusedBeforeSpawn
                        | RelayChannelTransitionKindV1::CloseExactWorkerTerminal { .. }
                ) {
                    ensure(
                        before.fact.local_joined
                            && request.expected_revision.checked_add(1)
                                == Some(response.channel.revision)
                            && request.transition.target_state(
                                before
                                    .fact
                                    .core
                                    .as_ref()
                                    .ok_or(RelayOwnerError::Scope)?
                                    .state,
                            ) == Some(response.channel.state),
                    )?;
                }
                let mut progress = self.resolve(
                    index,
                    ChannelResolutionV2::Mutation {
                        response: response.clone(),
                    },
                )?;
                progress |= self.snapshot(&response.channel)?;
                if !progress {
                    return Ok(None);
                }
                before.terminal.is_some()
                    || before.operations[index].purpose == ChannelOperationPurposeV2::Retirement
            }
            ChannelChangeV2::Finalize { fact } => {
                if before.fact.phase == AttachmentPhase::Closed {
                    let mut normalized = fact.clone();
                    normalized.observed_at = before.fact.observed_at.clone();
                    ensure(same(&normalized, &before.fact))?;
                    return Ok(None);
                }
                ensure(
                    before.terminal.is_some()
                        && fact.phase == AttachmentPhase::Closed
                        && fact.local_joined
                        && fact.error.is_none()
                        && before.operations.iter().all(|op| op.resolution.is_some()),
                )?;
                let mut check = fact.clone();
                check.phase = AttachmentPhase::Unknown;
                validate(&check)?;
                validate_scope(&check, scope)?;
                let mut expected = before.fact.clone();
                expected.phase = AttachmentPhase::Closed;
                expected.error = None;
                expected.local_joined = true;
                expected.observed_at = fact.observed_at.clone();
                ensure(same(&expected, fact))?;
                let local_only = fact.admission.is_none()
                    && fact.claim.is_none()
                    && fact.transition.is_none()
                    && fact.core.is_none()
                    && before.operations.is_empty();
                let never_recorded = matches!(
                    before.admission_proof.as_ref().map(|p| &p.outcome),
                    Some(RelayChannelHistoryOutcomeV1::AdmissionNeverRecordedExactTerminal { .. })
                ) && fact.core.is_none();
                let terminal_core = fact.core.as_ref().is_some_and(|core| !core.state.active())
                    && before.admission_proof.is_some();
                ensure(local_only || never_recorded || terminal_core)?;
                self.history.fact = fact.clone();
                true
            }
        };
        self.v2 = true;
        self.history.legacy_closed = false;
        self.history.fact.observed_at = observed_at.into();
        if terminal_unit {
            self.history.terminal_records += 1;
        } else {
            self.history.ordinary_records += 1;
        }
        if self.history.ordinary_records > ORDINARY
            || self.history.terminal_records > TERMINAL
            || self.count() > PER_JOB
        {
            return Err(RelayOwnerError::Capacity);
        }
        Ok(Some(terminal_unit))
    }
}
