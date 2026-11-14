//! Resolve a bounded control credential before parsing protected request input.

use super::{ApiResult, GatewayState, TenantGateway, TenantState};
use axum::{
    Json,
    extract::FromRequestParts,
    http::{HeaderMap, StatusCode, header, request::Parts},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

mod control_recovery;
/// Proposed owned broker-supervision proof, authenticated only by the gate.
pub(super) mod owned_broker_supervision;
pub(super) mod owned_control;
mod review_credentials;
pub use control_recovery::GatewayControlRecoveryRetirementV2;
#[cfg(test)]
pub(crate) use control_recovery::with_authenticated_control_recovery_for_test;
#[cfg(test)]
pub(crate) use control_recovery::with_control_recovery_auth_pair_for_test;
pub(crate) use control_recovery::{AuthenticatedControlRecoveryV2, ControlRecoveryUsageV2};
pub(super) use control_recovery::{ControlRecoveryAuthEpochV2, VerifiedControlRecoveryRunnerV2};
pub(crate) use review_credentials::AuthenticatedReviewRunnerCredential;
pub(super) use review_credentials::ReviewCredentialRegistry;
#[cfg(test)]
pub(crate) use review_credentials::authenticate_review_runner_fixture;
pub use review_credentials::preparation::{
    EnvelopeExpectationsV1, TechnicalCredentialIdentityV1, TechnicalCredentialInput,
    TechnicalCredentialRoleV1, prepare_envelope_configuration,
    validate_technical_credentials_configuration,
};

const MAX_CREDENTIAL_BYTES: usize = 4096;

fn valid_credential(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_CREDENTIAL_BYTES
        && !token.chars().any(char::is_whitespace)
}

/// Multiple fields are ambiguous even when the first happens to be valid.
pub(super) fn bearer(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let token = value.to_str().ok()?.strip_prefix("Bearer ")?;
    valid_credential(token).then_some(token)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CredentialRole {
    Runner,
    Viewer,
}

impl CredentialRole {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Runner => "runner",
            Self::Viewer => "viewer",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct CredentialBinding {
    pub(super) tenant_index: usize,
    pub(super) role: CredentialRole,
}

#[derive(Default, Clone)]
pub(super) struct CredentialIndex {
    entries: HashMap<[u8; 32], CredentialBinding>,
}

struct ControlCredentialInput<'a> {
    tenant_id: &'a str,
    role: CredentialRole,
    token: &'a str,
}

impl CredentialIndex {
    pub(super) fn digests(&self) -> impl Iterator<Item = &[u8; 32]> {
        self.entries.keys()
    }
    pub(super) fn for_tenants(tenants: &[TenantGateway]) -> anyhow::Result<Self> {
        let order = tenants
            .iter()
            .map(|tenant| tenant.tenant_id.clone())
            .collect::<Vec<_>>();
        Self::from_configured_inputs(
            &order,
            tenants.iter().flat_map(|tenant| {
                [
                    ControlCredentialInput {
                        tenant_id: &tenant.tenant_id,
                        role: CredentialRole::Runner,
                        token: &tenant.runner_token,
                    },
                    ControlCredentialInput {
                        tenant_id: &tenant.tenant_id,
                        role: CredentialRole::Viewer,
                        token: &tenant.viewer_token,
                    },
                ]
            }),
        )
    }
    fn from_configured_inputs<'a>(
        tenant_order: &[String],
        inputs: impl Iterator<Item = ControlCredentialInput<'a>>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !tenant_order.is_empty() && tenant_order.len() <= 256,
            "invalid credential configuration"
        );
        let mut tenants = HashMap::new();
        for (i, tenant) in tenant_order.iter().enumerate() {
            crate::tenancy::validate_tenant_id(tenant)?;
            anyhow::ensure!(
                tenants.insert(tenant.as_str(), i).is_none(),
                "ambiguous credential tenant"
            );
        }
        let mut index = Self::default();
        let mut tuples = std::collections::HashSet::new();
        for ControlCredentialInput {
            tenant_id,
            role,
            token,
        } in inputs
        {
            let tenant_index = *tenants
                .get(tenant_id)
                .ok_or_else(|| anyhow::anyhow!("unknown credential tenant"))?;
            anyhow::ensure!(
                valid_credential(token) && tuples.insert((tenant_index, role.as_str())),
                "invalid credential: expected 1 to 4096 bytes without whitespace"
            );
            let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
            anyhow::ensure!(
                index
                    .entries
                    .insert(digest, CredentialBinding { tenant_index, role })
                    .is_none(),
                "ambiguous credentials"
            );
        }
        anyhow::ensure!(
            index.entries.len() == tenant_order.len() * 2,
            "incomplete credential configuration"
        );
        Ok(index)
    }

    /// One bounded digest and one lookup; this is not a constant-time lookup.
    pub(super) fn lookup(&self, token: &str) -> Option<CredentialBinding> {
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        self.lookup_digest(&digest)
    }

    pub(super) fn lookup_digest(&self, digest: &[u8; 32]) -> Option<CredentialBinding> {
        self.entries.get(digest).copied()
    }
}

/// Keep this before Path/Query/Json parameters in every protected read handler.
pub(super) struct ReadIdentity(pub(super) TenantState, pub(super) &'static str);

impl FromRequestParts<GatewayState> for ReadIdentity {
    type Rejection = (StatusCode, Json<Value>);

    async fn from_request_parts(parts: &mut Parts, state: &GatewayState) -> ApiResult<Self> {
        state
            .authorize(&parts.headers, false)
            .await
            .map(|(tenant, role)| Self(tenant, role))
    }
}

/// Resolve the write role before any mutation request body can be consumed.
pub(super) struct RunnerIdentity(pub(super) TenantState);

impl FromRequestParts<GatewayState> for RunnerIdentity {
    type Rejection = (StatusCode, Json<Value>);

    async fn from_request_parts(parts: &mut Parts, state: &GatewayState) -> ApiResult<Self> {
        state
            .authorize(&parts.headers, true)
            .await
            .map(|(tenant, _)| Self(tenant))
    }
}
