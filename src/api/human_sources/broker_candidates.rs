//! Source-indexed owned broker candidates and the owned broker read protocol
//! of the retained human-source bundle, for the proposed
//! `broker_supervision_owned_v1` profile. Proposal submitted to Root: nothing
//! here opens a route, admits a credential or replaces a Store revalidation.
//!
//! The startup candidate index covers every retained source completely. One
//! `Workload(i)` entry names the only source whose Store may authenticate the
//! presented digest; no SQL ever runs on a neighboring tenant's Store. An
//! absence from a complete index is an unknown bearer. A collision, another
//! credential class or an incomplete coverage is never reported as an absence.
//! Every owned broker job runs its single target transaction inside the
//! discovery-generation window of the retained source: the Store compares the
//! startup generation in that transaction, before its work and before its
//! commit, so a discordant generation refuses without mutation and requires a
//! rebuild; no generation is compared outside that transaction. Epoch and
//! coverage are then checked in memory: a coverage closure latched by another
//! context can still withhold a committed outcome, which the publisher
//! reconciles by the same operation. Refusals
//! use only the closed owned vocabulary; after a possibly durable mutation,
//! any later refusal withholds the response instead of reporting a failure.
use super::*;
use crate::{
    api::owned_brokers::withheld,
    audit::{HumanBrokerFrameV1, OwnedBrokerWindow, OwnedHumanBrokerStorePageV1},
    broker_registry::owned_supervision::{OwnedChecked, OwnedSupervisionErrorCodeV1 as Code},
    human_authority::HumanAuthorityFailure,
};

/// Closed owned code of a human-authority refusal. A permanent mismatch of
/// the retained bundle requires a rebuild; a busy, closing or expired epoch is
/// a temporary unavailability retried with the same operation.
fn authority(failure: HumanAuthorityFailure) -> Code {
    match failure {
        HumanAuthorityFailure::InvalidManifest
        | HumanAuthorityFailure::ConfigurationMismatch
        | HumanAuthorityFailure::StorageBindingMismatch
        | HumanAuthorityFailure::OwnerInvalid
        | HumanAuthorityFailure::SourceMismatch => Code::SourceRebuildRequired,
        HumanAuthorityFailure::StorageUnavailable
        | HumanAuthorityFailure::OwnerContended
        | HumanAuthorityFailure::Closing
        | HumanAuthorityFailure::GateBusy
        | HumanAuthorityFailure::EpochExpired
        | HumanAuthorityFailure::TaskFailed
        | HumanAuthorityFailure::OpeningTimedOut => Code::SupervisionUnavailable,
    }
}

impl OwnedHumanGateway {
    /// Coverage of the retained bundle as a closed owned code: closing is
    /// temporary, a required rebuild or an invalid bundle waits for the
    /// operator. Never an absence.
    fn owned_coverage(&self) -> OwnedChecked<()> {
        match self.coverage.load(Ordering::Acquire) {
            COMPLETE => Ok(()),
            CLOSING => Err(Code::SupervisionUnavailable),
            _ => Err(Code::SourceRebuildRequired),
        }
    }

    /// Memory-only health of the retained bundle, checked again after each
    /// owned broker job; no filesystem or SQLite access on the async runtime.
    pub(in crate::api) fn owned_health(&self) -> OwnedChecked<()> {
        self.owned_coverage()?;
        if self.owner.admission_available() {
            Ok(())
        } else {
            Err(Code::SupervisionUnavailable)
        }
    }

    /// Source-indexed resolution of a presented broker bearer digest in the
    /// complete startup index: exactly one workload candidate yields its
    /// source. Absence from a complete index requires authentication; a
    /// single human or technical credential is another class and is
    /// forbidden here; any collision (or an out-of-range source index) makes
    /// the ownership ambiguous until the operator resolves it and restarts.
    pub(in crate::api) fn broker_source(&self, digest: &[u8; 32]) -> OwnedChecked<usize> {
        self.owned_coverage()?;
        match self.candidates.entries.get(digest).map(Vec::as_slice) {
            None => Err(Code::AuthenticationRequired),
            Some([Candidate::Workload(source)]) if *source < self.sources.len() => Ok(*source),
            Some([Candidate::Local(_) | Candidate::Technical]) => Err(Code::Forbidden),
            Some(_) => Err(Code::CredentialAmbiguous),
        }
    }

    /// One owned broker job of the retained source `index`: coverage, a real
    /// read epoch and scoped access, then `work` with the discovery window
    /// (startup generation and the presented bearer) that its single target
    /// transaction checks itself. A stale window latches the rebuild
    /// requirement, which closes the human contexts exactly as `read` does.
    /// Epoch and coverage are then checked in memory; for a mutating entry a
    /// refusal at that point is `ResponseWithheld`, since the transaction may
    /// have committed and is reconciled by the same operation.
    pub(in crate::api) fn owned_broker_job<T>(
        &self,
        index: usize,
        store: &Store,
        token: &str,
        mutating: bool,
        work: impl FnOnce(&HumanSourceAccess<'_>, &OwnedBrokerWindow<'_>) -> OwnedChecked<T>,
    ) -> OwnedChecked<T> {
        self.owned_coverage()?;
        let source = self.sources.get(index).ok_or(Code::SourceRebuildRequired)?;
        let epoch = self.owner.try_read_epoch().map_err(|failure| {
            if matches!(
                failure,
                HumanAuthorityFailure::OwnerInvalid
                    | HumanAuthorityFailure::StorageBindingMismatch
                    | HumanAuthorityFailure::ConfigurationMismatch
            ) {
                self.coverage.store(INVALID, Ordering::Release);
            }
            authority(failure)
        })?;
        let access = HumanSourceAccess::read(&epoch, source)
            .map_err(|error| store.owned_failure_code(&error))?;
        let expected = *self
            .discovery_generations
            .get(index)
            .ok_or(Code::SourceRebuildRequired)?;
        let window = OwnedBrokerWindow::new(expected, token);
        let outcome = work(&access, &window);
        if window.stale() {
            self.require_rebuild();
        }
        let value = outcome?;
        epoch
            .check()
            .map_err(authority)
            .and_then(|()| self.owned_coverage())
            .map_err(|code| withheld(mutating, code))?;
        Ok(value)
    }

    /// Owned human broker registry page through the exact `read` protocol and
    /// its human read vocabulary; the Store projection appends the read audit
    /// before any page, including an empty one.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::api) fn human_brokers(
        &self,
        index: usize,
        store: &Store,
        scope: &AuthorizedScopeV1,
        frame: &HumanBrokerFrameV1,
        after: Option<&str>,
        limit: usize,
        redactor: &Redactor,
    ) -> ApiResult<OwnedHumanBrokerStorePageV1> {
        self.read(index, store, |access| {
            store.owned_human_brokers(access, scope, frame, after, limit, redactor)
        })
    }
}
