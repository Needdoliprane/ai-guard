//! Live writer-to-enforcer handoff. Only the writer module can construct it,
//! after confirming the durable Pending and its exact intention audit ACK.
//! This is deliberately neither a durable record nor a deserializable token.
use super::super::{
    enforcer::retirement::UnusedPrepareRevoked,
    retirement::{RetirementAuditAckV1, RetirementEvidenceV1, RetirementIdentityV1},
};
pub(crate) use super::retirement_owners::RetirementContinuation;
use super::retirement_owners::{Owner, OwnerKey, OwnerPhase, OwnerRecovery, Work};
use crate::broker::journal::retirement::RetirementMutationError;
use std::{fmt, sync::Mutex};

pub(crate) enum RetirementFailure<I> {
    NotQueued {
        error: JournalError,
        input: I,
    },
    Refused {
        error: JournalError,
        continuation: RetirementContinuation<I>,
    },
    Unknown {
        error: JournalError,
    },
}

/// Process-local ownership state, not durable evidence or a new authority.
pub(crate) enum FinalRetirementRecovery {
    Absent,
    InFlight,
    Completed(CheckpointReceiptV1),
}
impl<I> RetirementFailure<I> {
    pub(crate) fn error(&self) -> JournalError {
        match self {
            Self::NotQueued { error, .. }
            | Self::Refused { error, .. }
            | Self::Unknown { error } => *error,
        }
    }
    pub(crate) fn into_input(self) -> Option<I> {
        match self {
            Self::NotQueued { input, .. } => Some(input),
            Self::Refused { .. } | Self::Unknown { .. } => None,
        }
    }
}
impl<I> fmt::Debug for RetirementFailure<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotQueued { .. } => "RetirementFailure::NotQueued(<private>)",
            Self::Refused { .. } => "RetirementFailure::Refused(<private>)",
            Self::Unknown { .. } => "RetirementFailure::Unknown(<private>)",
        })
    }
}
impl<I> fmt::Display for RetirementFailure<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}
impl<I> std::error::Error for RetirementFailure<I> {}

pub(crate) struct AuthorizedRetirement {
    _unused: Mutex<Option<UnusedPrepareRevoked>>,
    // The actual producer's retained core, never a pathname/scope/DTO origin.
    origin: Journal,
    pub(super) identity: RetirementIdentityV1,
    pub(super) evidence: RetirementEvidenceV1,
    pub(super) audit_ack: RetirementAuditAckV1,
}

impl AuthorizedRetirement {
    pub(crate) fn identity(&self) -> &RetirementIdentityV1 {
        &self.identity
    }
    /// Closed request data only. Borrowing it never clones the live original
    /// proof, which remains inside this non-Clone authorization.
    pub(crate) fn request_parts(
        &self,
    ) -> (
        RetirementIdentityV1,
        RetirementEvidenceV1,
        RetirementAuditAckV1,
    ) {
        (
            self.identity.clone(),
            self.evidence.clone(),
            self.audit_ack.clone(),
        )
    }

    pub(crate) fn consume_original(&self) -> JournalResult<()> {
        let original = self
            ._unused
            .lock()
            .map_err(|_| JournalError::Corrupt)?
            .take()
            .ok_or(JournalError::Conflict)?;
        drop(original);
        Ok(())
    }

    pub(crate) fn matches_recovery_origin(&self, snapshot: &StopJobSnapshotV1) -> bool {
        self.origin.matches_recovery_origin(snapshot)
    }

    #[cfg(test)]
    pub(crate) fn retains_original_for_test(&self) -> JournalResult<bool> {
        self._unused
            .lock()
            .map(|original| original.is_some())
            .map_err(|_| JournalError::Corrupt)
    }

    #[cfg(test)]
    pub(crate) fn into_parts(
        self,
    ) -> JournalResult<(
        RetirementIdentityV1,
        RetirementEvidenceV1,
        RetirementAuditAckV1,
    )> {
        let Self {
            _unused,
            origin: _origin,
            identity,
            evidence,
            audit_ack,
        } = self;
        let original = _unused
            .into_inner()
            .map_err(|_| JournalError::Corrupt)?
            .ok_or(JournalError::Conflict)?;
        drop(original);
        Ok((identity, evidence, audit_ack))
    }
}

impl fmt::Debug for AuthorizedRetirement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthorizedRetirement(<audited writer capability>)")
    }
}

use super::*;
use crate::broker::{
    enforcer::retirement::VerifiedRetirementReceipt,
    retirement_checkpoint::{Full3RegistrationBindingV1, PendingRetirementV1, audit_ack},
};

/// Owns the one live original-actor revocation proof. UUIDs are allocated once
/// before queuing and survive NotQueued retry through ownership of this input.
pub(crate) struct BeginUnusedRetirement(Box<BeginUnusedRetirementData>);
struct BeginUnusedRetirementData {
    operation_id: Uuid,
    expected: RevisionRefV1,
    unused: UnusedPrepareRevoked,
    intention_id: Uuid,
    final_id: Uuid,
}
impl BeginUnusedRetirement {
    pub(crate) fn new(
        operation_id: Uuid,
        expected: RevisionRefV1,
        unused: UnusedPrepareRevoked,
    ) -> JournalResult<Self> {
        if operation_id.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        super::super::journal::valid_hash(expected.hash())?;
        Ok(Self(Box::new(BeginUnusedRetirementData {
            operation_id,
            expected,
            unused,
            intention_id: Uuid::new_v4(),
            final_id: Uuid::new_v4(),
        })))
    }
    #[cfg(test)]
    pub(crate) fn operation_id(&self) -> Uuid {
        self.0.operation_id
    }
    #[cfg(test)]
    pub(crate) fn retirement_id(&self) -> Uuid {
        self.0.unused.retirement_id()
    }
    #[cfg(test)]
    pub(crate) fn intention_event_id(&self) -> Uuid {
        self.0.intention_id
    }
    #[cfg(test)]
    pub(crate) fn final_event_id(&self) -> Uuid {
        self.0.final_id
    }
}

pub(crate) struct PendingUnusedRetirement(Box<PendingUnusedRetirementData>);
struct PendingUnusedRetirementData {
    unused: UnusedPrepareRevoked,
    pending: PendingRetirementV1,
    intention: Event,
    #[cfg(test)]
    receipt: CheckpointReceiptV1,
}
impl PendingUnusedRetirement {
    pub(crate) fn identity(&self) -> &RetirementIdentityV1 {
        self.0.pending.identity()
    }
    #[cfg(test)]
    pub(crate) fn evidence(&self) -> &RetirementEvidenceV1 {
        self.0.pending.evidence()
    }
    pub(crate) fn intention_event_id(&self) -> Uuid {
        self.0.pending.intention_event_id()
    }
    #[cfg(test)]
    pub(crate) fn final_event_id(&self) -> Uuid {
        self.0.pending.final_event_id()
    }
    pub(crate) fn intention_event(&self) -> &Event {
        &self.0.intention
    }
    #[cfg(test)]
    pub(crate) fn head(&self) -> &RevisionRefV1 {
        self.0.receipt.head()
    }
}
impl fmt::Debug for BeginUnusedRetirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BeginUnusedRetirement(<live original proof>)")
    }
}
impl fmt::Debug for PendingUnusedRetirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PendingUnusedRetirement(<live original proof>)")
    }
}

pub(super) fn process_begin(
    backend: &Backend,
    input: &mut Option<BeginUnusedRetirement>,
) -> Work<PendingUnusedRetirement> {
    let Some(current) = input.as_ref() else {
        return Work::Unknown(JournalError::Corrupt);
    };
    let journal = match backend.full() {
        Ok(journal) => journal,
        Err(error) => return Work::Unknown(error),
    };
    let result = journal.begin_unused_retirement(
        current.0.operation_id,
        current.0.expected.clone(),
        &current.0.unused,
        current.0.intention_id,
        current.0.final_id,
    );
    match result {
        Err(RetirementMutationError::ColdReserved) => Work::Refused(JournalError::Busy),
        Err(RetirementMutationError::Journal(error)) => Work::Unknown(error),
        Ok((pending, intention, _receipt)) => {
            let Some(owned) = input.take() else {
                return Work::Unknown(JournalError::Corrupt);
            };
            Work::Complete(PendingUnusedRetirement(Box::new(
                PendingUnusedRetirementData {
                    unused: owned.0.unused,
                    pending,
                    intention,
                    #[cfg(test)]
                    receipt: _receipt,
                },
            )))
        }
    }
}

fn authorized_ack(
    journal: &Journal,
    pending: &PendingUnusedRetirement,
) -> JournalResult<RetirementAuditAckV1> {
    let job = journal
        .load(*pending.identity().job_operation_id())?
        .ok_or(JournalError::InvalidInput)?;
    let current = job.broker_checkpoint_v5().ok_or(JournalError::Conflict)?;
    let entry = current
        .retirements()
        .iter()
        .find(|entry| {
            entry.request().identity().retirement_id() == pending.identity().retirement_id()
        })
        .ok_or(JournalError::Conflict)?;
    if entry.finalized().is_some() || !entry.request().same_request(&pending.0.pending)? {
        return Err(JournalError::Conflict);
    }
    let ack = audit_ack(current.outbox(), entry.request())?;
    let receipt = job
        .outbox_receipt(pending.intention_event_id(), Some(*ack.audit_seq()))
        .ok_or(JournalError::Conflict)?;
    if receipt.input_hash() != ack.input_hash().as_str() || receipt.event_id() != *ack.event_id() {
        return Err(JournalError::Conflict);
    }
    Ok(ack)
}

pub(super) fn process_authorize(
    backend: &Backend,
    input: &mut Option<PendingUnusedRetirement>,
) -> Work<AuthorizedRetirement> {
    let Some(current) = input.as_ref() else {
        return Work::Unknown(JournalError::Corrupt);
    };
    let result = backend
        .full()
        .and_then(|journal| authorized_ack(journal, current).map(|ack| (ack, journal.clone())));
    match result {
        Err(error) => Work::Unknown(error),
        Ok((audit_ack, origin)) => {
            let Some(owned) = input.take() else {
                return Work::Unknown(JournalError::Corrupt);
            };
            Work::Complete(AuthorizedRetirement {
                identity: owned.0.pending.identity().clone(),
                evidence: owned.0.pending.evidence().clone(),
                audit_ack,
                _unused: Mutex::new(Some(owned.0.unused)),
                origin,
            })
        }
    }
}

pub(super) fn process_final(
    backend: &Backend,
    input: &mut Option<VerifiedRetirementReceipt>,
) -> Work<CheckpointReceiptV1> {
    let Some(verified) = input.as_ref() else {
        return Work::Unknown(JournalError::Corrupt);
    };
    let result = match backend {
        Backend::Full(journal) => StopJournal::from_full(journal).finalize_retirement(verified),
        Backend::Stops(journal) => journal.finalize_retirement(verified),
    };
    match result {
        Ok(receipt) => Work::Complete(receipt),
        Err(RetirementMutationError::ColdReserved) => Work::Refused(JournalError::Busy),
        Err(RetirementMutationError::Journal(error)) => Work::Unknown(error),
    }
}

pub(super) trait RetainedInput: Send + 'static {
    type Output: Send + 'static;
    fn key(&self, scope: &JournalScopeV1) -> JournalResult<(OwnerKey, String)>;
}
fn key(operation: Uuid, arm: Uuid, retirement: Uuid, phase: OwnerPhase) -> JournalResult<OwnerKey> {
    if operation.is_nil() || arm.is_nil() || retirement.is_nil() {
        return Err(JournalError::InvalidInput);
    }
    Ok(OwnerKey {
        operation,
        arm,
        retirement,
        phase,
    })
}
fn commitment(value: &impl serde::Serialize) -> JournalResult<String> {
    Ok(super::super::journal::digest(
        "ai-guard:retirement-owner:v1",
        &super::super::journal::canonical(value)?,
    ))
}
impl RetainedInput for BeginUnusedRetirement {
    type Output = PendingUnusedRetirement;
    fn key(&self, scope: &JournalScopeV1) -> JournalResult<(OwnerKey, String)> {
        use sha2::{Digest, Sha256};
        let input = &self.0;
        let registration = input.unused.registration();
        if registration.tenant_id() != scope.tenant_id()
            || registration.control_origin_hash()
                != hex::encode(Sha256::digest(scope.gateway_origin().as_bytes()))
            || registration.receipt().control_namespace_uuid
                != scope.control_namespace_uuid().to_string()
            || *input.unused.ledger().installation_id() != scope.installation_id()
        {
            return Err(JournalError::Conflict);
        }
        let arm = Uuid::parse_str(registration.arm_id()).map_err(|_| JournalError::InvalidInput)?;
        Ok((
            key(
                input.operation_id,
                arm,
                input.unused.retirement_id(),
                OwnerPhase::Begin,
            )?,
            commitment(&(
                scope,
                input.operation_id,
                &input.expected,
                registration.digest().map_err(|_| JournalError::Conflict)?,
                input.unused.ledger(),
                input.unused.epoch(),
                input.unused.generation(),
                input.unused.actor_nonce(),
                input.unused.revocation_nonce(),
                input.intention_id,
                input.final_id,
            ))?,
        ))
    }
}
impl RetainedInput for PendingUnusedRetirement {
    type Output = AuthorizedRetirement;
    fn key(&self, scope: &JournalScopeV1) -> JournalResult<(OwnerKey, String)> {
        let identity = self.identity();
        if super::super::journal::canonical(identity.scope())?
            != super::super::journal::canonical(scope)?
        {
            return Err(JournalError::Conflict);
        }
        let arm = Uuid::parse_str(&identity.arm().operation_id)
            .map_err(|_| JournalError::InvalidInput)?;
        Ok((
            key(
                *identity.job_operation_id(),
                arm,
                *identity.retirement_id(),
                OwnerPhase::Authorize,
            )?,
            commitment(&(&self.0.pending, &self.0.intention))?,
        ))
    }
}
impl RetainedInput for VerifiedRetirementReceipt {
    type Output = CheckpointReceiptV1;
    fn key(&self, scope: &JournalScopeV1) -> JournalResult<(OwnerKey, String)> {
        let identity = self.identity();
        if super::super::journal::canonical(identity.scope())?
            != super::super::journal::canonical(scope)?
        {
            return Err(JournalError::Conflict);
        }
        let arm = Uuid::parse_str(&identity.arm().operation_id)
            .map_err(|_| JournalError::InvalidInput)?;
        Ok((
            key(
                *identity.job_operation_id(),
                arm,
                *identity.retirement_id(),
                OwnerPhase::Final,
            )?,
            commitment(&(identity, self.receipt(), self.head(), self.accepted()))?,
        ))
    }
}
impl ClientState {
    async fn retry_retirement<I: RetainedInput>(
        &self,
        continuation: RetirementContinuation<I>,
        command: impl FnOnce(Arc<Owner<I, I::Output>>, Reply<()>) -> Command,
    ) -> Result<I::Output, RetirementFailure<I>> {
        let (key, owner) = self.retirements.retry::<I, I::Output>(continuation)?;
        let (reply, receive) = oneshot::channel();
        let error = match self.sender.try_send(command(owner, reply)) {
            Ok(()) => None,
            Err(mpsc::TrySendError::Full(_)) => Some(JournalError::Busy),
            Err(mpsc::TrySendError::Disconnected(_)) => Some(JournalError::Io),
        };
        if let Some(error) = error {
            return Err(self
                .retirements
                .retry_not_queued::<I, I::Output>(key, error));
        }
        self.receive(receive)
            .await
            .map_err(|error| RetirementFailure::Unknown { error })?;
        self.retirements
            .take::<I, I::Output>(key)?
            .ok_or(RetirementFailure::Unknown {
                error: JournalError::Io,
            })
    }

    pub(super) fn enqueue_retirement<I: RetainedInput>(
        &self,
        input: I,
        command: impl FnOnce(Arc<Owner<I, I::Output>>, Reply<()>) -> Command,
    ) -> Result<(OwnerKey, oneshot::Receiver<JournalResult<()>>), RetirementFailure<I>> {
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(RetirementFailure::NotQueued {
                error: JournalError::Io,
                input,
            });
        }
        let (key, commitment) = match input.key(&self.scope) {
            Ok(value) => value,
            Err(error) => return Err(RetirementFailure::NotQueued { error, input }),
        };
        let owner = self
            .retirements
            .admit::<I, I::Output>(key, commitment, input)?;
        let (reply, receive) = oneshot::channel();
        let error = match self.sender.try_send(command(owner, reply)) {
            Ok(()) => None,
            Err(mpsc::TrySendError::Full(_)) => Some(JournalError::Busy),
            Err(mpsc::TrySendError::Disconnected(_)) => Some(JournalError::Io),
        };
        if let Some(error) = error {
            return match self.retirements.unqueue::<I, I::Output>(key) {
                Ok(input) => Err(RetirementFailure::NotQueued { error, input }),
                Err(error) => Err(RetirementFailure::Unknown { error }),
            };
        }
        Ok((key, receive))
    }

    async fn retained_retirement<I: RetainedInput>(
        &self,
        input: I,
        command: impl FnOnce(Arc<Owner<I, I::Output>>, Reply<()>) -> Command,
    ) -> Result<I::Output, RetirementFailure<I>> {
        let (key, receive) = self.enqueue_retirement(input, command)?;
        self.receive(receive)
            .await
            .map_err(|error| RetirementFailure::Unknown { error })?;
        self.retirements
            .take::<I, I::Output>(key)?
            .ok_or(RetirementFailure::Unknown {
                error: JournalError::Io,
            })
    }
}

impl WriterClient {
    pub(crate) async fn retry_begin_unused_retirement(
        &self,
        continuation: RetirementContinuation<BeginUnusedRetirement>,
    ) -> Result<PendingUnusedRetirement, RetirementFailure<BeginUnusedRetirement>> {
        self.0
            .retry_retirement(continuation, |owner, reply| {
                Command::BeginUnusedRetirement { owner, reply }
            })
            .await
    }
    pub(crate) async fn retry_authorized_retirement(
        &self,
        continuation: RetirementContinuation<PendingUnusedRetirement>,
    ) -> Result<AuthorizedRetirement, RetirementFailure<PendingUnusedRetirement>> {
        self.0
            .retry_retirement(continuation, |owner, reply| {
                Command::AuthorizeUnusedRetirement { owner, reply }
            })
            .await
    }
    pub(crate) async fn begin_unused_retirement(
        &self,
        input: BeginUnusedRetirement,
    ) -> Result<PendingUnusedRetirement, RetirementFailure<BeginUnusedRetirement>> {
        self.0
            .retained_retirement(input, |owner, reply| Command::BeginUnusedRetirement {
                owner,
                reply,
            })
            .await
    }
    pub(crate) async fn authorize_unused_retirement(
        &self,
        input: PendingUnusedRetirement,
    ) -> Result<AuthorizedRetirement, RetirementFailure<PendingUnusedRetirement>> {
        self.0
            .retained_retirement(input, |owner, reply| Command::AuthorizeUnusedRetirement {
                owner,
                reply,
            })
            .await
    }
    pub(crate) fn recover_begin_unused_retirement(
        &self,
        operation: Uuid,
        arm: Uuid,
        retirement: Uuid,
    ) -> Result<Option<PendingUnusedRetirement>, RetirementFailure<BeginUnusedRetirement>> {
        let key = key(operation, arm, retirement, OwnerPhase::Begin)
            .map_err(|error| RetirementFailure::Unknown { error })?;
        self.0.retirements.take(key)
    }
    pub(crate) fn recover_authorized_retirement(
        &self,
        operation: Uuid,
        arm: Uuid,
        retirement: Uuid,
    ) -> Result<Option<AuthorizedRetirement>, RetirementFailure<PendingUnusedRetirement>> {
        let key = key(operation, arm, retirement, OwnerPhase::Authorize)
            .map_err(|error| RetirementFailure::Unknown { error })?;
        self.0.retirements.take(key)
    }
    pub(crate) async fn bind_full3_registration(
        &self,
        operation_id: Uuid,
        expected: RevisionRefV1,
        binding: Full3RegistrationBindingV1,
    ) -> JournalResult<CheckpointReceiptV1> {
        self.0
            .call(move |reply| Command::BindFull3 {
                operation_id,
                expected,
                binding: Box::new(binding),
                reply,
            })
            .await
    }
}
impl StopWriterClient {
    #[cfg(test)]
    pub(in crate::broker) fn enqueue_final_for_test(
        &self,
        input: VerifiedRetirementReceipt,
    ) -> Result<oneshot::Receiver<JournalResult<()>>, RetirementFailure<VerifiedRetirementReceipt>>
    {
        self.0
            .enqueue_retirement(input, |owner, reply| Command::FinalizeRetirement {
                owner,
                reply,
            })
            .map(|(_, receive)| receive)
    }
    pub(crate) async fn retry_final_retirement(
        &self,
        continuation: RetirementContinuation<VerifiedRetirementReceipt>,
    ) -> Result<CheckpointReceiptV1, RetirementFailure<VerifiedRetirementReceipt>> {
        self.0
            .retry_retirement(continuation, |owner, reply| Command::FinalizeRetirement {
                owner,
                reply,
            })
            .await
    }
    pub(crate) async fn finalize_retirement(
        &self,
        receipt: VerifiedRetirementReceipt,
    ) -> Result<CheckpointReceiptV1, RetirementFailure<VerifiedRetirementReceipt>> {
        self.0
            .retained_retirement(receipt, |owner, reply| Command::FinalizeRetirement {
                owner,
                reply,
            })
            .await
    }
    pub(crate) fn recover_final_retirement(
        &self,
        operation: Uuid,
        arm: Uuid,
        retirement: Uuid,
    ) -> Result<Option<CheckpointReceiptV1>, RetirementFailure<VerifiedRetirementReceipt>> {
        let key = key(operation, arm, retirement, OwnerPhase::Final)
            .map_err(|error| RetirementFailure::Unknown { error })?;
        self.0.retirements.take(key)
    }
    /// Unlike the legacy bounded-wait callback above, this distinguishes a
    /// live owner before a reconciler considers querying/admitting any work.
    pub(crate) fn recover_final_retirement_state(
        &self,
        operation: Uuid,
        arm: Uuid,
        retirement: Uuid,
    ) -> Result<FinalRetirementRecovery, RetirementFailure<VerifiedRetirementReceipt>> {
        let key = key(operation, arm, retirement, OwnerPhase::Final)
            .map_err(|error| RetirementFailure::Unknown { error })?;
        match self.0.retirements.recover(key)? {
            OwnerRecovery::Absent => Ok(FinalRetirementRecovery::Absent),
            OwnerRecovery::InFlight => Ok(FinalRetirementRecovery::InFlight),
            OwnerRecovery::Completed(receipt) => Ok(FinalRetirementRecovery::Completed(receipt)),
        }
    }
    pub(crate) fn close_retirement_owners(&self) -> JournalResult<()> {
        self.0.retirements.close()
    }
    pub(crate) fn has_retained_retirement(&self) -> bool {
        self.0.retirements.has_retained()
    }
    pub(crate) async fn drain_retirement_owners(&self) -> JournalResult<()> {
        self.0.retirements.drain().await
    }
}

impl StopWriterClient {
    pub(crate) async fn pending_retirement_events(
        &self,
        operation_id: Uuid,
    ) -> JournalResult<Vec<OutboxEventV1>> {
        self.0
            .call(move |reply| Command::PendingRetirementEvents {
                operation_id,
                reply,
            })
            .await
    }
    pub(crate) async fn acknowledge_retirement_events(
        &self,
        operation_id: Uuid,
        acknowledgements: Vec<(Uuid, String, i64)>,
    ) -> JournalResult<Vec<OutboxReceiptV1>> {
        if operation_id.is_nil() || acknowledgements.is_empty() || acknowledgements.len() > 512 {
            return Err(JournalError::InvalidInput);
        }
        for (id, hash, seq) in &acknowledgements {
            if id.is_nil() || *seq <= 0 {
                return Err(JournalError::InvalidInput);
            }
            super::super::journal::valid_hash(hash)?;
        }
        self.0
            .call(move |reply| Command::AcknowledgeRetirementEvents {
                operation_id,
                acknowledgements,
                reply,
            })
            .await
    }
}
