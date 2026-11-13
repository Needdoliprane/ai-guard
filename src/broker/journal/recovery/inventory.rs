//! Bounded discovery on the retained STOP reader, never the business loader.
use super::*;

const MAX_PAGE: u32 = 100;
const MAX_ISSUES: usize = 100;

pub struct StopInventoryEntryV1 {
    operation_id: Uuid,
    head: RevisionRefV1,
    incomplete: bool,
}

/// Confirmed per-job heads, not a transactional filesystem snapshot or an
/// inventory of runtime activity. No credential or business job is retained.
pub struct StopInventoryPageV1 {
    scope: JournalScopeV1,
    entries: Vec<StopInventoryEntryV1>,
    issues: Vec<StopRecoveryIssueV1>,
    incomplete: bool,
    issues_truncated: bool,
    next_after: Option<Uuid>,
}
opaque!(StopInventoryEntryV1, StopInventoryPageV1);

impl StopInventoryEntryV1 {
    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }
    pub fn head(&self) -> &RevisionRefV1 {
        &self.head
    }
    pub fn incomplete(&self) -> bool {
        self.incomplete
    }
}
impl StopInventoryPageV1 {
    pub fn scope(&self) -> &JournalScopeV1 {
        &self.scope
    }
    pub fn entries(&self) -> &[StopInventoryEntryV1] {
        &self.entries
    }
    pub fn issues(&self) -> &[StopRecoveryIssueV1] {
        &self.issues
    }
    pub fn incomplete(&self) -> bool {
        self.incomplete
    }
    pub fn issues_truncated(&self) -> bool {
        self.issues_truncated
    }
    pub fn next_after(&self) -> Option<Uuid> {
        self.next_after
    }
}

impl StopJournal {
    /// Uses this core's mutex, floors, observed identities and retained lease.
    /// A page is bounded independently of the JournalLimits-bounded scan. An
    /// incomplete scan supplies no continuation guarantee or invented total.
    pub fn recovery_inventory(
        &self,
        after: Option<Uuid>,
        limit: u32,
    ) -> JournalResult<StopInventoryPageV1> {
        if !(1..=MAX_PAGE).contains(&limit) || after.is_some_and(|id| id.is_nil()) {
            return Err(JournalError::InvalidInput);
        }
        let mut writer = self.core.writer.lock().map_err(|_| JournalError::Corrupt)?;
        let integrity_failure = |error| {
            self.core
                .status_observation_failed
                .store(true, Ordering::SeqCst);
            error
        };
        let mut report = StopRecoveryReaderV1::from_core(&self.core)
            .map_err(integrity_failure)?
            .scan();
        // A shared anchor/lease failure purges every projection. It cannot be
        // downgraded to an incomplete or empty successful reply.
        if let Some(issue) = report
            .issues
            .iter()
            .find(|issue| issue.phase == StopRecoveryPhaseV1::SharedScope)
        {
            return Err(integrity_failure(issue.error));
        }
        if self.scope_incomplete {
            report.issue(
                None,
                StopRecoveryPhaseV1::Inventory,
                JournalError::Interrupted,
            );
        }
        let observed_count = writer.observed_jobs.union(&report.observed_jobs).count();
        if observed_count > self.core.limits.jobs as usize {
            return Err(JournalError::LimitExceeded);
        }
        let mut conflicts = HashSet::new();
        let confirmed = report
            .jobs
            .iter()
            .map(|job| (job.operation_id, job))
            .collect::<HashMap<_, _>>();
        for (id, floor) in &writer.floors {
            let matches = confirmed
                .get(id)
                .and_then(|job| {
                    usize::try_from(floor.revision)
                        .ok()
                        .map(|index| job.chain.get(index) == Some(floor))
                })
                .unwrap_or(false);
            if !matches {
                conflicts.insert(*id);
            }
        }
        for (id, hash) in &writer.rejected_floors {
            if report.rejected_floors.get(id) != Some(hash) {
                conflicts.insert(*id);
            }
        }
        for id in &writer.observed_jobs {
            if !report.observed_jobs.contains(id) {
                conflicts.insert(*id);
            }
        }
        for job in &report.jobs {
            if writer.rejected_floors.contains_key(&job.operation_id) {
                conflicts.insert(job.operation_id);
            }
        }
        drop(confirmed);
        // Deterministic diagnostics contain only authenticated-scope UUIDs and
        // closed error values, never an untrusted filename or error string.
        let mut conflicts = conflicts.into_iter().collect::<Vec<_>>();
        conflicts.sort_unstable();
        for id in &conflicts {
            report.issue(Some(*id), StopRecoveryPhaseV1::Job, JournalError::Conflict);
        }
        if report.issues_truncated
            || report
                .issues
                .iter()
                .any(|issue| issue.error != JournalError::Interrupted)
        {
            self.core
                .status_observation_failed
                .store(true, Ordering::SeqCst);
        }
        report
            .jobs
            .retain(|job| conflicts.binary_search(&job.operation_id).is_err());
        report.reader.check_shared().map_err(integrity_failure)?;
        writer
            .observed_jobs
            .extend(report.observed_jobs.iter().copied());
        for (id, hash) in &report.rejected_floors {
            if !writer.floors.contains_key(id) && conflicts.binary_search(id).is_err() {
                writer.rejected_floors.insert(*id, hash.clone());
            }
        }
        for job in &report.jobs {
            writer.floors.insert(job.operation_id, job.head.clone());
        }
        let shared_partial = self.scope_incomplete
            || report
                .issues
                .iter()
                .any(|issue| issue.operation_id.is_none());
        let partial_jobs = report
            .issues
            .iter()
            .filter_map(|issue| issue.operation_id)
            .collect::<HashSet<_>>();
        let mut candidates = report
            .jobs
            .iter()
            .filter(|job| after.is_none_or(|id| job.operation_id > id));
        let entries = candidates
            .by_ref()
            .take(limit as usize)
            .map(|job| StopInventoryEntryV1 {
                operation_id: job.operation_id,
                head: job.head.clone(),
                incomplete: shared_partial || partial_jobs.contains(&job.operation_id),
            })
            .collect::<Vec<_>>();
        let has_more = candidates.next().is_some();
        // Partial+empty cannot masquerade as a successful absence of activity.
        if entries.is_empty() && report.incomplete {
            return Err(report
                .issues
                .first()
                .map_or(JournalError::Interrupted, |issue| issue.error));
        }
        let next_after = if !report.incomplete && has_more {
            entries.last().map(|entry| entry.operation_id)
        } else {
            None
        };
        let issues_truncated = report.issues_truncated || report.issues.len() > MAX_ISSUES;
        report.issues.truncate(MAX_ISSUES);
        report.reader.check_shared().map_err(integrity_failure)?;
        Ok(StopInventoryPageV1 {
            scope: self.scope().clone(),
            entries,
            issues: report.issues,
            incomplete: report.incomplete,
            issues_truncated,
            next_after,
        })
    }
}

#[cfg(test)]
#[path = "inventory_tests.rs"]
mod tests;
