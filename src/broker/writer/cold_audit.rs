//! A separate, private facade for the existing Stops actor.
//! It grants no start, general journal mutation or authenticated HTTP access.
use super::{ClientState, Command, CompletionFailure, JournalError, JournalResult, queue};
use crate::{
    broker::journal::recovery::{
        StopJournal,
        cold_audit::{PreparedColdAudit, completion::ConfirmedColdAudit},
    },
    runner::ExactStoredEventReceipt,
};
use std::{
    fmt,
    sync::{Arc, mpsc},
};
use tokio::sync::oneshot;
use uuid::Uuid;

#[derive(Clone)]
pub(in crate::broker) struct ColdAuditWriterClient(Arc<ClientState>);

/// The exact preparation, lease and queue ticket retained before any HTTP.
/// A detached authenticated owner must retain this entire non-clonable value;
/// the caller waiting for that owner must not own its cancellation lifetime.
pub(in crate::broker) struct ReservedColdAudit {
    actor: Arc<ClientState>,
    prepared: PreparedColdAudit,
    completion: queue::CompletionPermit<Command>,
    #[cfg(test)]
    before_ack_fault: bool,
}

/// A consumed preparation and exact receipt batch, without an actor sender.
/// The actor may retain this on failure without keeping its own queue alive.
pub(in crate::broker) struct ColdAuditCompletion {
    prepared: PreparedColdAudit,
    receipts: Vec<ExactStoredEventReceipt>,
    #[cfg(test)]
    before_ack_fault: bool,
}

pub(in crate::broker) struct ColdAuditCompletionWaiter {
    actor: Arc<ClientState>,
    receive: oneshot::Receiver<JournalResult<ConfirmedColdAudit>>,
}

impl fmt::Debug for ColdAuditWriterClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ColdAuditWriterClient(<private>)")
    }
}

impl fmt::Debug for ReservedColdAudit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReservedColdAudit(<private>)")
    }
}

impl fmt::Debug for ColdAuditCompletion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ColdAuditCompletion(<private>)")
    }
}

impl fmt::Debug for ColdAuditCompletionWaiter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ColdAuditCompletionWaiter(<private>)")
    }
}

impl ColdAuditWriterClient {
    // Only the parent's actual Backend::Stops admission constructs this facade.
    // It has no constructor from StopWriterClient or WriterClient.
    pub(super) fn from_stops_actor(actor: Arc<ClientState>) -> Self {
        Self(actor)
    }

    pub(in crate::broker) async fn prepare(
        &self,
        operation_id: Uuid,
    ) -> JournalResult<ReservedColdAudit> {
        if operation_id.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        tokio::runtime::Handle::try_current().map_err(|_| JournalError::Io)?;
        // Acquire the unique completion slot before asking for the logical
        // reserve. Both are owned before returning to the HTTP owner. A lost
        // preparation reply drops its strong reserve in the actor; a lost wait
        // drops this queue permit without cancelling accepted journal work.
        let completion = self
            .0
            .sender
            .try_reserve_completion()
            .map_err(|error| match error {
                queue::ReserveError::Full => JournalError::Busy,
                queue::ReserveError::Disconnected => JournalError::Io,
            })?;
        let prepared = self
            .0
            .call(move |reply| Command::PrepareColdAudit {
                operation_id,
                reply,
            })
            .await?;
        Ok(ReservedColdAudit {
            actor: Arc::clone(&self.0),
            prepared,
            completion,
            #[cfg(test)]
            before_ack_fault: false,
        })
    }

    /// Confirm only the actor's retained exact lot on the same STOP lease.
    /// None means no matching quarantine, never evidence of an ACK. An error
    /// or lost reply does not discard an input whose confirmation failed.
    pub(in crate::broker) async fn recover_quarantined(
        &self,
        operation_id: Uuid,
    ) -> JournalResult<Option<ConfirmedColdAudit>> {
        if operation_id.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        self.0
            .call(move |reply| Command::RecoverColdAudit {
                operation_id,
                reply,
            })
            .await
    }
}

impl ReservedColdAudit {
    /// Exercise the existing publisher fault on the actual actor thread. It
    /// neither replaces publication nor supplies a simulated confirmation.
    #[cfg(test)]
    pub(in crate::broker) fn with_test_before_ack_fault(mut self) -> Self {
        self.before_ack_fault = true;
        self
    }
    pub(in crate::broker) fn prepared(&self) -> &PreparedColdAudit {
        &self.prepared
    }

    /// Synchronous and single-use: after the last HTTP response the owner can
    /// enqueue without any new await, queue acquisition or replacement client.
    /// A receipt batch is still descriptive data; only the authenticated owner
    /// may choose to submit it, and the journal rechecks every exact entry.
    // A refusal returns the exact batch for recovery; keep the contract.
    #[allow(clippy::result_large_err)]
    pub(in crate::broker) fn submit(
        self,
        receipts: Vec<ExactStoredEventReceipt>,
    ) -> Result<ColdAuditCompletionWaiter, CompletionFailure<ColdAuditCompletion>> {
        let Self {
            actor,
            prepared,
            completion,
            #[cfg(test)]
            before_ack_fault,
        } = self;
        let input = ColdAuditCompletion {
            prepared,
            receipts,
            #[cfg(test)]
            before_ack_fault,
        };
        if input.receipts.is_empty() || input.receipts.len() > 2 {
            return Err(CompletionFailure::NotQueued {
                error: JournalError::InvalidInput,
                input,
            });
        }
        let (reply, receive) = oneshot::channel();
        match completion.try_send(Command::CompleteColdAudit { input, reply }) {
            Ok(()) => Ok(ColdAuditCompletionWaiter { actor, receive }),
            Err(mpsc::TrySendError::Full(Command::CompleteColdAudit { input, .. })) => {
                Err(CompletionFailure::NotQueued {
                    error: JournalError::Busy,
                    input,
                })
            }
            Err(mpsc::TrySendError::Disconnected(Command::CompleteColdAudit { input, .. })) => {
                Err(CompletionFailure::NotQueued {
                    error: JournalError::Io,
                    input,
                })
            }
            Err(_) => Err(CompletionFailure::Unknown {
                error: JournalError::Io,
            }),
        }
    }
}

impl ColdAuditCompletion {
    pub(super) fn operation_id(&self) -> Uuid {
        self.prepared.operation_id()
    }

    pub(super) fn apply(&mut self, journal: &StopJournal) -> JournalResult<ConfirmedColdAudit> {
        #[cfg(test)]
        if std::mem::take(&mut self.before_ack_fault) {
            crate::broker::journal::fail_next_cold_ack_for_test();
        }
        crate::broker::journal::recovery::cold_audit::completion::complete(
            journal,
            &self.prepared,
            &self.receipts,
        )
    }

    pub(super) fn confirm(&self, journal: &StopJournal) -> JournalResult<ConfirmedColdAudit> {
        crate::broker::journal::recovery::cold_audit::completion::confirm(
            journal,
            &self.prepared,
            &self.receipts,
        )
    }
}

impl ColdAuditCompletionWaiter {
    // A refusal returns the exact batch for recovery; keep the contract.
    #[allow(clippy::result_large_err)]
    pub(in crate::broker) async fn confirmed(
        self,
    ) -> Result<ConfirmedColdAudit, CompletionFailure<ColdAuditCompletion>> {
        tokio::runtime::Handle::try_current().map_err(|_| CompletionFailure::Unknown {
            error: JournalError::Io,
        })?;
        self.actor
            .receive(self.receive)
            .await
            .map_err(|error| CompletionFailure::Unknown { error })
    }
}
