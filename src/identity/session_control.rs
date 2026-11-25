//! Pure catalogue and launch authorization over a current human scope.

use super::{
    AuthorizedScopeV1, ContextModeV1, HumanCapabilityV1, IdentityFailureV1, PrincipalRefV1,
};
use crate::control::session_control::SessionLaunchTargetSelectionV1;
use std::fmt;

const MAX_LAUNCHERS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLaunchTargetFailureV1 {
    Identity(IdentityFailureV1),
    /// The visible target changed. Read the catalogue again before selecting.
    Stale,
}

impl From<IdentityFailureV1> for SessionLaunchTargetFailureV1 {
    fn from(error: IdentityFailureV1) -> Self {
        Self::Identity(error)
    }
}

impl fmt::Display for SessionLaunchTargetFailureV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Identity(error) => fmt::Display::fmt(error, f),
            Self::Stale => f.write_str("session launch target revision is stale"),
        }
    }
}

impl std::error::Error for SessionLaunchTargetFailureV1 {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Identity(error) => Some(error),
            Self::Stale => None,
        }
    }
}

/// Authorization fields of the current server-side target policy. Callers
/// resolve the target in the same authority source as the human scope.
#[derive(Debug, Clone, Copy)]
pub struct SessionLaunchTargetPolicyV1<'a> {
    pub selection: SessionLaunchTargetSelectionV1,
    pub active: bool,
    pub tenant_id: &'a str,
    pub organization_scope: &'a str,
    pub launchers: &'a [PrincipalRefV1],
}

impl SessionLaunchTargetPolicyV1<'_> {
    pub fn may_list(&self, scope: &AuthorizedScopeV1) -> bool {
        self.authorize_scope(scope).is_ok()
    }

    /// Check the exact selection against a revalidated scope and policy.
    /// A mismatched ID has the same result as an absent target in the Store.
    /// Call again in the transaction that owns the effect.
    pub fn authorize_launch(
        &self,
        scope: &AuthorizedScopeV1,
        selected: SessionLaunchTargetSelectionV1,
    ) -> Result<(), SessionLaunchTargetFailureV1> {
        self.authorize_scope(scope)?;
        if self.selection.launch_target_id() != selected.launch_target_id() {
            return Err(IdentityFailureV1::NotFound.into());
        }
        if self.selection.expected_target_revision() != selected.expected_target_revision() {
            return Err(SessionLaunchTargetFailureV1::Stale);
        }
        Ok(())
    }

    fn authorize_scope(&self, scope: &AuthorizedScopeV1) -> Result<(), IdentityFailureV1> {
        scope.require_oidc_capability_ceiling()?;
        if scope.mode() != ContextModeV1::Personal
            || !scope.principal().is_human()
            || !scope.has_capability(HumanCapabilityV1::LaunchSession)
        {
            return Err(IdentityFailureV1::PermissionDenied);
        }
        if !self.active
            || self.tenant_id != scope.tenant_id()
            || self.organization_scope != scope.organization_scope()
        {
            return Err(IdentityFailureV1::NotFound);
        }
        if self.launchers.is_empty()
            || self.launchers.len() > MAX_LAUNCHERS
            || !self.launchers.windows(2).all(|pair| pair[0] < pair[1])
            || self
                .launchers
                .iter()
                .any(|principal| !principal.is_human() || principal.validate().is_err())
        {
            return Err(IdentityFailureV1::IdentityUnavailable);
        }
        if self.launchers.binary_search(scope.principal()).is_err() {
            return Err(IdentityFailureV1::NotFound);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "session_control_tests.rs"]
mod tests;
