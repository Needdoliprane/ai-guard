//! Ownership of the already-admitted filesystem work for a held CREATE.
//! No network is initiated here; dropping a waiter must not drop queued work.
use super::journal::{AdmissionJournal, JournalResult};
use tokio::{sync::OwnedSemaphorePermit, task::JoinHandle};

pub(crate) struct JournalIo {
    journal: AdmissionJournal,
    _capacity: OwnedSemaphorePermit,
}

impl JournalIo {
    /// The caller acquires existing bounded capacity before any possible effect.
    pub(crate) fn new(journal: AdmissionJournal, capacity: OwnedSemaphorePermit) -> Self {
        Self {
            journal,
            _capacity: capacity,
        }
    }

    pub(crate) fn journal(&self) -> &AdmissionJournal {
        &self.journal
    }

    /// Dispatch is synchronous. Only joining the already-owned work may await.
    pub(crate) fn dispatch<T: Send + 'static>(
        self,
        action: impl FnOnce(&mut AdmissionJournal) -> JournalResult<T> + Send + 'static,
    ) -> JoinHandle<JournalResult<(Self, T)>> {
        tokio::task::spawn_blocking(move || {
            // Both journal lock and admission survive loss of the async waiter.
            // Do not acquire capacity or initiate HTTP from this closure.
            let mut owned = self;
            let result = action(&mut owned.journal)?;
            Ok((owned, result))
        })
    }
}
