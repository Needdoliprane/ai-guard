//! Diagnostic only, in the same checked Business transaction as its binding.
use super::*;
use crate::{api::AuthenticatedReviewRunnerCredential, control::SupervisionStateV1};

#[derive(Debug)]
struct StaleControlDiscovery;
impl std::fmt::Display for StaleControlDiscovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("owned control discovery requires rebuilding")
    }
}
impl std::error::Error for StaleControlDiscovery {}

impl Store {
    /// Source/format admission only. No request body or expected_scope exists
    /// yet, and this read epoch is never promoted into writer authority.
    pub(crate) fn modern_control_write_admission(
        &self,
        access: &HumanSourceAccess<'_>,
    ) -> anyhow::Result<()> {
        self.with_modern_read(access, |tx| {
            let format: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
            if format != 15 {
                return Err(crate::audit::egress::EgressStoreFailure::MigrationRequired.into());
            }
            Ok(())
        })
    }

    /// None means the complete startup collision index needs rebuilding. It
    /// never means an absent identity or permission to fall back to legacy.
    pub(crate) fn modern_control_protocol(
        &self,
        access: &HumanSourceAccess<'_>,
        credential: &AuthenticatedReviewRunnerCredential,
        expected: HumanDiscoveryGeneration,
    ) -> anyhow::Result<Option<SupervisionStateV1>> {
        let HumanSourceAccess::Read { epoch, source } = access else {
            return Err(
                crate::control::review_binding::ReviewBindingFailureV1::PermissionDenied.into(),
            );
        };
        let result = self.with_modern_read(access, |tx| {
            let binding =
                self.verify_review_binding_in_transaction(tx, epoch, source, credential)?;
            if discovery_generation(tx)? != expected {
                return Err(StaleControlDiscovery.into());
            }
            let state = self.supervision_state_checked(tx)?;
            binding.revalidate()?;
            Ok(state)
        });
        match result {
            // with_modern_read has rolled back and restored the prior timeout;
            // stale discovery cannot commit even a diagnostic transaction.
            Err(error) if error.downcast_ref::<StaleControlDiscovery>().is_some() => Ok(None),
            result => result.map(Some),
        }
    }
}
