//! Live writer-to-enforcer handoff. Only the writer module can construct it,
//! after confirming the durable Pending and its exact intention audit ACK.
//! This is deliberately neither a durable record nor a deserializable token.
use super::super::{
    enforcer::retirement::UnusedPrepareRevoked,
    retirement::{RetirementAuditAckV1, RetirementEvidenceV1, RetirementIdentityV1},
};
use std::fmt;

pub(crate) struct AuthorizedRetirement {
    pub(super) unused: UnusedPrepareRevoked,
    pub(super) identity: RetirementIdentityV1,
    pub(super) evidence: RetirementEvidenceV1,
    pub(super) audit_ack: RetirementAuditAckV1,
}

impl AuthorizedRetirement {
    pub(crate) fn into_parts(
        self,
    ) -> (
        RetirementIdentityV1,
        RetirementEvidenceV1,
        RetirementAuditAckV1,
    ) {
        let Self {
            unused: _unused,
            identity,
            evidence,
            audit_ack,
        } = self;
        (identity, evidence, audit_ack)
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

pub(super) fn begin(
    journal: &Journal,
    input: BeginUnusedRetirement,
) -> JournalResult<PendingUnusedRetirement> {
    let input = *input.0;
    let (pending, intention, _receipt) = journal.begin_unused_retirement(
        input.operation_id,
        input.expected,
        &input.unused,
        input.intention_id,
        input.final_id,
    )?;
    Ok(PendingUnusedRetirement(Box::new(
        PendingUnusedRetirementData {
            unused: input.unused,
            pending,
            intention,
            #[cfg(test)]
            receipt: _receipt,
        },
    )))
}
pub(super) fn authorize(
    journal: &Journal,
    pending: PendingUnusedRetirement,
) -> JournalResult<AuthorizedRetirement> {
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
    let audit_ack = audit_ack(current.outbox(), entry.request())?;
    // Receipt is produced by the confirmed chain's durable ACK revision. The
    // matching input hash binds the exact intention payload; timestamps and IDs
    // are never regenerated while waiting for this ACK.
    let ack_receipt = job
        .outbox_receipt(pending.intention_event_id(), Some(*audit_ack.audit_seq()))
        .ok_or(JournalError::Conflict)?;
    if ack_receipt.input_hash() != audit_ack.input_hash().as_str()
        || ack_receipt.event_id() != *audit_ack.event_id()
    {
        return Err(JournalError::Conflict);
    }
    Ok(AuthorizedRetirement {
        identity: pending.0.pending.identity().clone(),
        evidence: pending.0.pending.evidence().clone(),
        audit_ack,
        unused: pending.0.unused,
    })
}

impl WriterClient {
    pub(crate) async fn begin_unused_retirement(
        &self,
        input: BeginUnusedRetirement,
    ) -> Result<PendingUnusedRetirement, CompletionFailure<BeginUnusedRetirement>> {
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(CompletionFailure::NotQueued {
                error: JournalError::Io,
                input,
            });
        }
        let (reply, receive) = oneshot::channel();
        match self
            .0
            .sender
            .try_send(Command::BeginUnusedRetirement { input, reply })
        {
            Ok(()) => self
                .0
                .receive(receive)
                .await
                .map_err(|error| CompletionFailure::Unknown { error }),
            Err(mpsc::TrySendError::Full(Command::BeginUnusedRetirement { input, .. })) => {
                Err(CompletionFailure::NotQueued {
                    error: JournalError::Busy,
                    input,
                })
            }
            Err(mpsc::TrySendError::Disconnected(Command::BeginUnusedRetirement {
                input, ..
            })) => Err(CompletionFailure::NotQueued {
                error: JournalError::Io,
                input,
            }),
            Err(_) => Err(CompletionFailure::Unknown {
                error: JournalError::Io,
            }),
        }
    }
    pub(crate) async fn authorize_unused_retirement(
        &self,
        pending: PendingUnusedRetirement,
    ) -> Result<AuthorizedRetirement, CompletionFailure<PendingUnusedRetirement>> {
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(CompletionFailure::NotQueued {
                error: JournalError::Io,
                input: pending,
            });
        }
        let (reply, receive) = oneshot::channel();
        match self
            .0
            .sender
            .try_send(Command::AuthorizeUnusedRetirement { pending, reply })
        {
            Ok(()) => self
                .0
                .receive(receive)
                .await
                .map_err(|error| CompletionFailure::Unknown { error }),
            Err(mpsc::TrySendError::Full(Command::AuthorizeUnusedRetirement {
                pending, ..
            })) => Err(CompletionFailure::NotQueued {
                error: JournalError::Busy,
                input: pending,
            }),
            Err(mpsc::TrySendError::Disconnected(Command::AuthorizeUnusedRetirement {
                pending,
                ..
            })) => Err(CompletionFailure::NotQueued {
                error: JournalError::Io,
                input: pending,
            }),
            Err(_) => Err(CompletionFailure::Unknown {
                error: JournalError::Io,
            }),
        }
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
    pub(crate) async fn finalize_retirement(
        &self,
        receipt: VerifiedRetirementReceipt,
    ) -> JournalResult<CheckpointReceiptV1> {
        self.0
            .call(move |reply| Command::FinalizeRetirement {
                receipt: Box::new(receipt),
                reply,
            })
            .await
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
