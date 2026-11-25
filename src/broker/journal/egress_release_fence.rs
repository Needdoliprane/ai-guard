//! Per-job exclusion between journal publication and one live E3 submission.
use super::{Journal, JournalError, JournalResult, LoadedCheckpoint, RevisionRefV1, Writer};
use crate::{
    broker::{journal::JournalStatusV1, lifecycle::LocalCommandRequestV1},
    control_fs::PrivateFile,
    runner::egress_release::EgressReleaseIntentV1,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
};
use uuid::Uuid;

const AVAILABLE: u8 = 0;
const WRITING: u8 = 1;
const REVOKED: u8 = 2;

/// Owned by the writer actor. A delivered permit cannot outlive its issuer's
/// authority, and it does not own a queue sender.
pub(in crate::broker) struct EgressReleaseFenceIssuer(Arc<AtomicBool>);

impl EgressReleaseFenceIssuer {
    pub(in crate::broker) fn new() -> Self {
        Self(Arc::new(AtomicBool::new(true)))
    }
}

impl Drop for EgressReleaseFenceIssuer {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// An in-process proof of the exact head checked at issuance. Publication
/// revokes it before writing, or receives Busy if a claim already owns it.
#[must_use]
pub(crate) struct EgressReleaseHeadPermit {
    state: Arc<AtomicU8>,
    issuer: Arc<AtomicBool>,
    _lease: PrivateFile,
}

/// Keeps publication excluded through the synchronous write. Drop irrevocably
/// consumes the permit, including after an ambiguous write.
#[must_use = "hold the guard through the synchronous write"]
pub(crate) struct EgressReleaseHeadGuard(EgressReleaseHeadPermit);

// The live consumer is connected in the executor's separate lot.
#[allow(dead_code)]
impl EgressReleaseHeadPermit {
    /// Takes no journal lock and performs no I/O.
    pub(crate) fn claim(self) -> JournalResult<EgressReleaseHeadGuard> {
        self.state
            .compare_exchange(AVAILABLE, WRITING, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| JournalError::Conflict)?;
        let guard = EgressReleaseHeadGuard(self);
        // Check issuer liveness after claiming so issuer exit orders with the
        // claim. The guard retains the journal lease for the winning write.
        if !guard.0.issuer.load(Ordering::SeqCst) {
            return Err(JournalError::Conflict);
        }
        Ok(guard)
    }
}

impl Drop for EgressReleaseHeadGuard {
    fn drop(&mut self) {
        self.0.state.store(REVOKED, Ordering::SeqCst);
    }
}

impl Journal {
    pub(in crate::broker) fn fence_egress_release_head(
        &self,
        operation_id: Uuid,
        expected: &RevisionRefV1,
        intent: &EgressReleaseIntentV1,
        issuer: &EgressReleaseFenceIssuer,
    ) -> JournalResult<EgressReleaseHeadPermit> {
        if operation_id.is_nil() {
            return Err(JournalError::InvalidInput);
        }
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        let job = match self.status_locked(operation_id, &mut writer)? {
            Some(JournalStatusV1::Accepted(job)) => job,
            Some(JournalStatusV1::Rejected(_)) | None => return Err(JournalError::InvalidInput),
            Some(_) => return Err(JournalError::Interrupted),
        };
        if job.head() != expected {
            return Err(JournalError::Conflict);
        }
        require_release_intent(&job.0.checkpoint, intent)?;
        writer.issue_release_head(operation_id, &self.0.lock, issuer)
    }

    /// Publication fixture using the same registration and claim on a real
    /// journal head. E3 payload requirements are checked by the strict issuer.
    #[cfg(test)]
    pub(in crate::broker) fn issue_release_head_for_test(
        &self,
        operation_id: Uuid,
        expected: &RevisionRefV1,
        issuer: &EgressReleaseFenceIssuer,
    ) -> JournalResult<EgressReleaseHeadPermit> {
        let mut writer = self.0.writer.lock().map_err(|_| JournalError::Corrupt)?;
        self.check_writer()?;
        let job = self
            .load_locked(operation_id, &mut writer)?
            .ok_or(JournalError::InvalidInput)?;
        if job.head() != expected {
            return Err(JournalError::Conflict);
        }
        writer.issue_release_head(operation_id, &self.0.lock, issuer)
    }
}

pub(super) fn require_release_intent(
    checkpoint: &LoadedCheckpoint,
    intent: &EgressReleaseIntentV1,
) -> JournalResult<()> {
    let context = checkpoint.runner().broker_context();
    if context.egress_release == Some(intent)
        && context
            .worker
            .is_some_and(|worker| intent.matches_running(worker))
        && checkpoint.lifecycle().is_some_and(|lifecycle| {
            !lifecycle
                .commands()
                .iter()
                .any(|entry| matches!(entry.request(), LocalCommandRequestV1::CancelRequested))
        })
    {
        Ok(())
    } else {
        Err(JournalError::Conflict)
    }
}

impl Writer {
    fn issue_release_head(
        &mut self,
        operation_id: Uuid,
        lease: &PrivateFile,
        issuer: &EgressReleaseFenceIssuer,
    ) -> JournalResult<EgressReleaseHeadPermit> {
        if self
            .egress_release_heads
            .get(&operation_id)
            .and_then(std::sync::Weak::upgrade)
            .is_some_and(|state| state.load(Ordering::SeqCst) != REVOKED)
        {
            return Err(JournalError::Busy);
        }
        let state = Arc::new(AtomicU8::new(AVAILABLE));
        self.egress_release_heads
            .insert(operation_id, Arc::downgrade(&state));
        Ok(EgressReleaseHeadPermit {
            state,
            issuer: Arc::clone(&issuer.0),
            _lease: lease.clone(),
        })
    }

    pub(super) fn revoke_release_head(&mut self, operation_id: Uuid) -> JournalResult<()> {
        if let Some(state) = self
            .egress_release_heads
            .get(&operation_id)
            .and_then(std::sync::Weak::upgrade)
            && state.compare_exchange(AVAILABLE, REVOKED, Ordering::SeqCst, Ordering::SeqCst)
                == Err(WRITING)
        {
            return Err(JournalError::Busy);
        }
        self.egress_release_heads.remove(&operation_id);
        Ok(())
    }
}
