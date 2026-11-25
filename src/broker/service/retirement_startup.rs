//! One retained startup continuation per profile, before any finalization FS.
//! No sender, new authority, reconstruction or automatic mutation retry.
//! A retained IO ambiguity can only confirm an already committed exact Final.
use super::super::{
    enforcer::retirement::VerifiedRetirementReceipt,
    journal::{CheckpointReceiptV1, JournalError, JournalResult, recovery::StopRecoveryReportV1},
    retirement::RetirementIdentityV1,
};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
pub(super) struct StartupSlot(Mutex<Option<Arc<StartupOwner>>>);
pub(super) struct StartupOwner {
    state: Mutex<State>,
    continuation: Arc<tokio::sync::Mutex<()>>,
    restore: RestoreProgress,
    restored: OnceLock<(usize, bool)>,
    uncertain: AtomicBool,
    #[cfg(test)]
    fail_ack: AtomicBool,
    #[cfg(test)]
    fail_confirmation: AtomicBool,
}
#[derive(Clone, Copy, Default)]
pub(super) struct RestoreProgress {
    pub(super) restored: usize,
    pub(super) partial: bool,
    pub(super) split: bool,
}
struct Data {
    report: StopRecoveryReportV1,
    job_index: usize,
    retirement_index: usize,
    querying: bool,
    verified: Option<VerifiedRetirementReceipt>,
    result: Option<JournalResult<CheckpointReceiptV1>>,
    failed: bool,
}
enum State {
    Ready(Box<Data>),
    Running,
    Completed(Arc<StopRecoveryReportV1>),
}
// Error returns the verified receipt for recovery; keep the value contract.
#[allow(clippy::large_enum_variant)]
pub(super) enum InstallError {
    Retained(JournalError),
    Returned {
        error: JournalError,
        input: VerifiedRetirementReceipt,
    },
}
struct InFlight {
    owner: Arc<StartupOwner>,
    data: Option<Box<Data>>,
}
impl Drop for InFlight {
    fn drop(&mut self) {
        if let Some(mut data) = self.data.take() {
            data.failed = true;
            *self.owner.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Ready(data);
        }
    }
}
impl StartupSlot {
    #[cfg(test)]
    pub(super) fn install(&self, report: StopRecoveryReportV1) -> JournalResult<Arc<StartupOwner>> {
        self.install_with_progress(report, RestoreProgress::default())
    }
    pub(super) fn install_with_progress(
        &self,
        report: StopRecoveryReportV1,
        restore: RestoreProgress,
    ) -> JournalResult<Arc<StartupOwner>> {
        let mut slot = self.0.lock().map_err(|_| JournalError::Corrupt)?;
        if slot.is_some() {
            return Err(JournalError::Busy);
        }
        let owner = Arc::new(StartupOwner {
            continuation: Arc::new(tokio::sync::Mutex::new(())),
            restore,
            restored: OnceLock::new(),
            uncertain: AtomicBool::new(false),
            #[cfg(test)]
            fail_ack: AtomicBool::new(false),
            #[cfg(test)]
            fail_confirmation: AtomicBool::new(false),
            state: Mutex::new(State::Ready(Box::new(Data {
                report,
                job_index: 0,
                retirement_index: 0,
                querying: false,
                verified: None,
                result: None,
                failed: false,
            }))),
        });
        *slot = Some(Arc::clone(&owner));
        Ok(owner)
    }
    pub(super) fn retained(&self) -> JournalResult<Option<Arc<StartupOwner>>> {
        self.0
            .lock()
            .map(|slot| slot.as_ref().map(Arc::clone))
            .map_err(|_| JournalError::Corrupt)
    }
    pub(super) fn has_retained(&self) -> bool {
        self.0.lock().map(|slot| slot.is_some()).unwrap_or(true)
    }
    pub(super) fn release_after_factory(&self, owner: &Arc<StartupOwner>) -> JournalResult<()> {
        let mut slot = self.0.lock().map_err(|_| JournalError::Corrupt)?;
        if !slot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, owner))
        {
            return Err(JournalError::Conflict);
        }
        let state = owner.state.lock().map_err(|_| JournalError::Corrupt)?;
        if !matches!(&*state, State::Completed(report) if report.retirement_factory_consumed()) {
            return Err(JournalError::Conflict);
        }
        *slot = None;
        Ok(())
    }
}
impl StartupOwner {
    pub(super) fn enter(&self) -> JournalResult<tokio::sync::OwnedMutexGuard<()>> {
        Arc::clone(&self.continuation)
            .try_lock_owned()
            .map_err(|_| JournalError::Busy)
    }
    pub(super) fn restore_progress(&self) -> RestoreProgress {
        self.restore
    }
    pub(super) fn had_uncertainty(&self) -> bool {
        self.uncertain.load(Ordering::Acquire)
    }
    pub(super) fn restored(&self) -> Option<(usize, bool)> {
        self.restored.get().copied()
    }
    pub(super) fn record_restored(&self, count: usize, partial: bool) -> JournalResult<()> {
        let value = (count, partial);
        if let Some(existing) = self.restored.get() {
            return if *existing == value {
                Ok(())
            } else {
                Err(JournalError::Conflict)
            };
        }
        self.restored.set(value).map_err(|_| JournalError::Conflict)
    }
    pub(super) fn has_step(&self) -> JournalResult<bool> {
        let state = self.state.lock().map_err(|_| JournalError::Corrupt)?;
        match &*state {
            State::Ready(data) if data.failed => Err(JournalError::Interrupted),
            State::Ready(data) => Ok(data.verified.is_some()),
            State::Running => Err(JournalError::Busy),
            State::Completed(_) => Ok(false),
        }
    }
    #[cfg(test)]
    pub(super) fn fail_ack_and_confirmation_for_test(&self) {
        self.fail_ack.store(true, Ordering::Release);
        self.fail_confirmation.store(true, Ordering::Release);
    }
    #[cfg(test)]
    pub(super) fn retained_receipt_for_test(&self) -> Option<String> {
        let state = self.state.lock().ok()?;
        match &*state {
            State::Ready(data) => data
                .verified
                .as_ref()?
                .receipt()
                .digest()
                .ok()
                .map(|hash| hash.as_str().to_owned()),
            _ => None,
        }
    }
    pub(super) fn next_query(&self) -> JournalResult<Option<RetirementIdentityV1>> {
        let mut state = self.state.lock().map_err(|_| JournalError::Corrupt)?;
        let State::Ready(data) = &mut *state else {
            return Err(JournalError::Busy);
        };
        if data.failed || data.querying || data.verified.is_some() {
            return Err(JournalError::Interrupted);
        }
        // The reader already bounded jobs/records/retirements. Retain a cursor
        // into that actual report and clone only the one current identity;
        // no new per-profile cardinality budget or duplicated inventory.
        let next = if advance_to_pending(data)? {
            current_identity(data).cloned()
        } else {
            None
        };
        data.querying = next.is_some();
        Ok(next)
    }
    pub(super) fn query_failed(&self, identity: &RetirementIdentityV1) -> JournalResult<()> {
        let mut state = self.state.lock().map_err(|_| JournalError::Corrupt)?;
        let State::Ready(data) = &mut *state else {
            return Err(JournalError::Busy);
        };
        if data.failed || !data.querying || data.verified.is_some() || data.result.is_some() {
            return Err(JournalError::Conflict);
        }
        let expected = current_identity(data).ok_or(JournalError::Corrupt)?;
        if super::super::journal::canonical(expected)?
            != super::super::journal::canonical(identity)?
        {
            return Err(JournalError::Conflict);
        }
        // Only a returned read-only Query error reaches this transition, not
        // cancellation/Drop. Retain the exact cursor/report for a later call;
        // no mutation result or verified receipt is discarded or retried here.
        data.querying = false;
        Ok(())
    }
    pub(super) fn no_query_receipt(&self) -> JournalResult<()> {
        let mut state = self.state.lock().map_err(|_| JournalError::Corrupt)?;
        let State::Ready(data) = &mut *state else {
            return Err(JournalError::Busy);
        };
        if data.failed || !data.querying || data.verified.is_some() {
            return Err(JournalError::Conflict);
        }
        data.querying = false;
        data.retirement_index = data
            .retirement_index
            .checked_add(1)
            .ok_or(JournalError::LimitExceeded)?;
        Ok(())
    }
    // A refusal returns the verified receipt for recovery; keep the contract.
    #[allow(clippy::result_large_err)]
    pub(super) fn retain_verified(
        &self,
        verified: VerifiedRetirementReceipt,
    ) -> Result<(), InstallError> {
        // Poison recovery here stores an existing capability only. It never
        // grants permission; subsequent work remains closed when failed.
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let State::Ready(data) = &mut *state else {
            return Err(InstallError::Returned {
                error: JournalError::Busy,
                input: verified,
            });
        };
        if data.verified.is_some() {
            return Err(InstallError::Returned {
                error: JournalError::Conflict,
                input: verified,
            });
        }
        let valid = !data.failed
            && data.querying
            && current_identity(data).is_some_and(|identity| {
                match (
                    super::super::journal::canonical(identity),
                    super::super::journal::canonical(verified.identity()),
                ) {
                    (Ok(expected), Ok(actual)) => expected == actual,
                    _ => false,
                }
            });
        data.verified = Some(verified);
        data.querying = false;
        if !valid {
            data.failed = true;
            return Err(InstallError::Retained(JournalError::Conflict));
        }
        Ok(())
    }
    pub(super) fn process(self: &Arc<Self>) -> JournalResult<()> {
        let data = {
            let mut state = self.state.lock().map_err(|_| JournalError::Corrupt)?;
            if matches!(&*state, State::Ready(data) if !data.failed && matches!(data.result, Some(Ok(_))))
            {
                return Ok(());
            }
            match std::mem::replace(&mut *state, State::Running) {
                State::Ready(data)
                    if !data.failed
                        && !data.querying
                        && data.verified.is_some()
                        && (data.result.is_none()
                            || matches!(data.result, Some(Err(JournalError::Io)))) =>
                {
                    data
                }
                old => {
                    *state = old;
                    return Err(JournalError::Conflict);
                }
            }
        };
        let mut guard = InFlight {
            owner: Arc::clone(self),
            data: Some(data),
        };
        let data = guard.data.as_mut().ok_or(JournalError::Corrupt)?;
        // No ownership mutex is held during IO. The closure owns this guard;
        // Service owns the slot, even if its notification waiter disappears.
        let verified = data.verified.as_ref().ok_or(JournalError::Corrupt)?;
        let result = if data.result.is_some() {
            // Never repeat the mutation after an IO ambiguity. The same lease,
            // report and Verified may only recognize an already committed F.
            #[cfg(test)]
            let injected = self.fail_confirmation.swap(false, Ordering::AcqRel);
            #[cfg(not(test))]
            let injected = false;
            if injected {
                Err(JournalError::Io)
            } else {
                data.report.confirm_retirement_retained(verified)
            }
        } else {
            #[cfg(test)]
            if self.fail_ack.swap(false, Ordering::AcqRel) {
                super::super::journal::fail_next_cold_ack_for_test();
            }
            data.report.finalize_retirement_retained(verified)
        };
        if result.is_err() {
            self.uncertain.store(true, Ordering::Release);
        }
        data.failed |= matches!(result, Err(error) if error != JournalError::Io);
        data.result = Some(result);
        let data = guard.data.take().ok_or(JournalError::Corrupt)?;
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = State::Ready(data);
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn confirm_step(&self) -> JournalResult<()> {
        self.confirm_step_inner(None)
    }
    pub(super) fn confirm_step_with_original_slot(
        &self,
        full: &super::super::enforcer::EnforcerFullClientV3,
    ) -> JournalResult<()> {
        self.confirm_step_inner(Some(full))
    }
    fn confirm_step_inner(
        &self,
        full: Option<&super::super::enforcer::EnforcerFullClientV3>,
    ) -> JournalResult<()> {
        let mut state = self.state.lock().map_err(|_| JournalError::Corrupt)?;
        let State::Ready(data) = &mut *state else {
            return Err(JournalError::Busy);
        };
        if data.failed {
            return Err(JournalError::Interrupted);
        }
        let receipt = data
            .result
            .as_ref()
            .ok_or(JournalError::Interrupted)?
            .as_ref()
            .map_err(|e| *e)?;
        let verified = data.verified.as_ref().ok_or(JournalError::Interrupted)?;
        let job = data
            .report
            .jobs()
            .get(data.job_index)
            .ok_or(JournalError::Corrupt)?;
        let entry = job
            .retirements()
            .get(data.retirement_index)
            .ok_or(JournalError::Corrupt)?;
        let finalized = entry.finalized().ok_or(JournalError::Conflict)?;
        if receipt.operation_id() != job.operation_id()
            || receipt.head() != job.head()
            || super::super::journal::canonical(finalized.receipt())?
                != super::super::journal::canonical(verified.receipt())?
            || super::super::journal::canonical(entry.request().identity())?
                != super::super::journal::canonical(verified.identity())?
        {
            data.failed = true;
            return Err(JournalError::Conflict);
        }
        let next = data
            .retirement_index
            .checked_add(1)
            .ok_or(JournalError::LimitExceeded)?;
        if let Some(full) = full
            && full.retire_local_slot(verified).is_err()
        {
            data.failed = true;
            return Err(JournalError::Conflict);
        }
        drop(data.verified.take());
        let _ = data.result.take();
        data.retirement_index = next;
        Ok(())
    }
    pub(super) fn completed_report(&self) -> JournalResult<Arc<StopRecoveryReportV1>> {
        let mut state = self.state.lock().map_err(|_| JournalError::Corrupt)?;
        if let State::Completed(report) = &*state {
            return Ok(Arc::clone(report));
        }
        match std::mem::replace(&mut *state, State::Running) {
            State::Ready(mut data) => {
                if data.failed || data.querying || data.verified.is_some() || data.result.is_some()
                {
                    *state = State::Ready(data);
                    return Err(JournalError::Interrupted);
                }
                match advance_to_pending(&mut data) {
                    Ok(false) => {}
                    Ok(true) => {
                        *state = State::Ready(data);
                        return Err(JournalError::Interrupted);
                    }
                    Err(error) => {
                        *state = State::Ready(data);
                        return Err(error);
                    }
                }
                let report = Arc::new(data.report);
                *state = State::Completed(Arc::clone(&report));
                Ok(report)
            }
            old => {
                *state = old;
                Err(JournalError::Interrupted)
            }
        }
    }
}

fn current_identity(data: &Data) -> Option<&RetirementIdentityV1> {
    Some(
        data.report
            .jobs()
            .get(data.job_index)?
            .retirements()
            .get(data.retirement_index)?
            .request()
            .identity(),
    )
}
fn advance_to_pending(data: &mut Data) -> JournalResult<bool> {
    while let Some(job) = data.report.jobs().get(data.job_index) {
        if let Some(entry) = job.retirements().get(data.retirement_index) {
            if entry.finalized().is_none() && entry.request().audit_ack().is_some() {
                return Ok(true);
            }
            data.retirement_index = data
                .retirement_index
                .checked_add(1)
                .ok_or(JournalError::LimitExceeded)?;
        } else {
            data.job_index = data
                .job_index
                .checked_add(1)
                .ok_or(JournalError::LimitExceeded)?;
            data.retirement_index = 0;
        }
    }
    Ok(false)
}
