//! Trusted control SDK. A free receipt DTO cannot construct an exec claim.
//! Runtime/RoleLease integration consumes FreshRelayExecClaim separately from
//! start guards. This module does not itself open an engine or renew deadlines.
#![allow(dead_code)] // Integration interfaces retained until owner/port composition.
use super::AuditClient;
use crate::relay::admission::*;
use serde::Serialize;
use std::sync::Arc;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RelayAdmissionClientError {
    InvalidRequest,
    Authentication,
    Forbidden,
    Conflict,
    Capacity,
    Unavailable,
    Protocol,
}
impl std::fmt::Display for RelayAdmissionClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidRequest => "relay admission request is invalid",
            Self::Authentication => "relay admission runner authentication failed",
            Self::Forbidden => "relay admission authority is unavailable or outside its scope",
            Self::Conflict => "relay admission conflicts with durable identity or state",
            Self::Capacity => "relay admission or mandatory audit capacity is exhausted",
            Self::Unavailable => "relay admission result is unresolved; retain the same operation",
            Self::Protocol => "relay admission response is incompatible; retain the same operation",
        })
    }
}
impl std::error::Error for RelayAdmissionClientError {}
type Result<T> = std::result::Result<T, RelayAdmissionClientError>;
use RelayAdmissionClientError as Error;

struct ClaimEvidence {
    tenant_id: String,
    admission: RelayChannelAdmissionV1,
    claim: RelayChannelClaimV1,
    receipt: RelayChannelReceiptV1,
}
/// Non-Clone, non-deserializable, fresh successful ClaimExec response only.
/// Consuming this in the runtime is necessary but does not replace its retained
/// deadline, RoleLease, exact incarnation, environment and profile checks.
pub(crate) struct FreshRelayExecClaim {
    evidence: Arc<ClaimEvidence>,
}
impl std::fmt::Debug for FreshRelayExecClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FreshRelayExecClaim([private])")
    }
}
#[derive(Clone)]
pub(crate) struct CurrentRelayAuthority {
    evidence: Arc<ClaimEvidence>,
}
impl FreshRelayExecClaim {
    pub(crate) fn admission(&self) -> &RelayChannelAdmissionV1 {
        &self.evidence.admission
    }
    pub(crate) fn receipt(&self) -> &RelayChannelReceiptV1 {
        &self.evidence.receipt
    }
    pub(crate) fn current_authority(&self) -> CurrentRelayAuthority {
        CurrentRelayAuthority {
            evidence: self.evidence.clone(),
        }
    }
}
impl CurrentRelayAuthority {
    pub(crate) fn admission(&self) -> &RelayChannelAdmissionV1 {
        &self.evidence.admission
    }
}
// Claim result value contract; keep the by-value shape.
#[allow(clippy::large_enum_variant)]
pub(crate) enum RelayExecClaimResult {
    Fresh(FreshRelayExecClaim),
    /// Historical receipt only. It cannot be converted to FreshRelayExecClaim.
    Replayed(RelayChannelResultV1),
}

fn check(value: bool) -> Result<()> {
    if value { Ok(()) } else { Err(Error::Protocol) }
}
fn context(
    result: &RelayChannelResultV1,
    tenant: &str,
    admission: &RelayChannelAdmissionV1,
    operation_id: &str,
    input_hash: &str,
) -> Result<()> {
    let c = &result.channel;
    let r = &result.receipt;
    check(
        result.version == 1
            && c.version == 1
            && r.version == 1
            && c.tenant_id == tenant
            && c.control_namespace_uuid == admission.control_namespace_uuid
            && c.channel_id == admission.channel_id
            && c.epoch == admission.epoch
            && c.admission_operation_id == admission.operation_id
            && c.admission_input_hash
                == admission.input_hash().map_err(|_| Error::InvalidRequest)?
            && c.endpoint == admission.endpoint
            && c.credential_id == admission.credential_id
            && c.binding_revision == admission.binding_revision
            && c.session_id == admission.session_id
            && c.expires_at == admission.expires_at
            && c.revision > 0
            && r.operation_id == operation_id
            && r.channel_id == admission.channel_id
            && r.epoch == admission.epoch
            && r.input_hash == input_hash
            && r.audit_seq > 0
            && crate::control::hash(&r.audit_hash).is_ok(),
    )
}

fn history_receipt_head(result: &RelayChannelReconciliationV1, seq: i64, hash: &str) -> Result<()> {
    check(
        seq > 0
            && seq <= result.audit_seq
            && crate::control::hash(hash).is_ok()
            && (seq != result.audit_seq || hash == result.audit_hash),
    )
}

/// Historical wire checks only. This function never constructs ClaimEvidence,
/// FreshRelayExecClaim or CurrentRelayAuthority, including recorded claim reads.
fn history_context(
    result: &RelayChannelReconciliationV1,
    tenant: &str,
    request: &RelayChannelReconcileV1,
) -> Result<()> {
    let admission = &request.admission;
    let admission_hash = admission.input_hash().map_err(|_| Error::InvalidRequest)?;
    check(
        result.version == 1
            && result.tenant_id == tenant
            && result.control_namespace_uuid == admission.control_namespace_uuid
            && result.admission_input_hash == admission_hash
            && result.audit_seq >= 0
            && crate::control::hash(&result.audit_hash).is_ok()
            && (result.audit_seq != 0 || result.audit_hash == "0".repeat(64)),
    )?;
    match &result.outcome {
        RelayChannelHistoryOutcomeV1::Historical {
            channel,
            admission_receipt,
            pending,
        } => {
            // Reuse the exact admission/snapshot/receipt correlation checks;
            // wrapping a DTO here does not grant the SDK's opaque exec types.
            let historical = RelayChannelResultV1 {
                version: 1,
                replayed: true,
                channel: channel.clone(),
                receipt: admission_receipt.clone(),
            };
            context(
                &historical,
                tenant,
                admission,
                &admission.operation_id,
                &admission_hash,
            )?;
            history_receipt_head(
                result,
                admission_receipt.audit_seq,
                &admission_receipt.audit_hash,
            )?;
            match (&request.pending, pending) {
                (None, None) => Ok(()),
                (Some(expected), Some(observed)) => {
                    check(expected == &observed.pending)?;
                    match &observed.resolution {
                        RelayChannelPendingResolutionV1::Recorded { receipt } => {
                            let recorded = RelayChannelResultV1 {
                                receipt: receipt.clone(),
                                ..historical
                            };
                            context(
                                &recorded,
                                tenant,
                                admission,
                                expected.operation_id(),
                                &expected.input_hash().map_err(|_| Error::InvalidRequest)?,
                            )?;
                            check(
                                channel.revision > expected.expected_revision()
                                    && receipt.audit_seq > admission_receipt.audit_seq,
                            )?;
                            history_receipt_head(result, receipt.audit_seq, &receipt.audit_hash)
                        }
                        RelayChannelPendingResolutionV1::InapplicableCas => {
                            check(channel.revision > expected.expected_revision())
                        }
                        RelayChannelPendingResolutionV1::InapplicableState => check(
                            channel.revision == expected.expected_revision()
                                && !expected.accepts_state(channel.state),
                        ),
                        RelayChannelPendingResolutionV1::Unresolved => check(
                            channel.revision < expected.expected_revision()
                                || channel.revision == expected.expected_revision()
                                    && expected.accepts_state(channel.state),
                        ),
                    }
                }
                _ => Err(Error::Protocol),
            }
        }
        RelayChannelHistoryOutcomeV1::AdmissionUnresolved => Ok(()),
        RelayChannelHistoryOutcomeV1::AdmissionNeverRecordedExactTerminal {
            admission_operation_id,
            channel_id,
            epoch,
            endpoint,
            worker_revision,
            terminal_observation_receipt: receipt,
        } => {
            check(
                admission_operation_id == &admission.operation_id
                    && channel_id == &admission.channel_id
                    && *epoch == admission.epoch
                    && endpoint == &admission.endpoint
                    && *worker_revision >= admission.expected_worker_revision
                    && *worker_revision < i64::MAX as u64
                    && receipt.worker_id.as_deref() == Some(admission.endpoint.worker_id.as_str())
                    && receipt.revision.is_some_and(|revision| {
                        revision >= admission.expected_worker_revision
                            && revision <= *worker_revision
                    })
                    && uuid::Uuid::parse_str(&receipt.operation_id)
                        .is_ok_and(|id| !id.is_nil() && id.to_string() == receipt.operation_id)
                    && crate::control::hash(&receipt.input_hash).is_ok(),
            )?;
            history_receipt_head(result, receipt.audit_seq, &receipt.audit_hash)
        }
    }
}

/// Revalidate a persisted historical response with its complete expected DTO.
/// The tenant comes from the retained journal scope, never the response.
pub(crate) fn validate_relay_channel_history(
    tenant: &str,
    request: &RelayChannelReconcileV1,
    response: &RelayChannelReconciliationV1,
) -> Result<()> {
    request.validate().map_err(|_| Error::InvalidRequest)?;
    history_context(response, tenant, request)
}

/// Pure correlation of a transition acknowledgement, with no fresh exec type.
/// Broker-local terminal/join and full-prefix conditions remain mandatory.
pub(crate) fn validate_relay_channel_transition_result(
    tenant: &str,
    admission: &RelayChannelAdmissionV1,
    request: &RelayChannelTransitionV1,
    response: &RelayChannelResultV1,
) -> Result<()> {
    admission.validate().map_err(|_| Error::InvalidRequest)?;
    let pending = RelayChannelPendingV1::Transition(request.clone());
    pending
        .validate(admission)
        .map_err(|_| Error::InvalidRequest)?;
    context(
        response,
        tenant,
        admission,
        &request.operation_id,
        &request.input_hash().map_err(|_| Error::InvalidRequest)?,
    )
}

impl AuditClient {
    /// Obtain tenant/namespace only from authenticated supervision identity.
    /// `active` describes namespace admission mode, not runner authentication;
    /// historical reads remain valid in maintenance after workload retirement.
    pub(crate) async fn reconcile_relay_channel(
        &self,
        request: &RelayChannelReconcileV1,
    ) -> Result<RelayChannelReconciliationV1> {
        request.validate().map_err(|_| Error::InvalidRequest)?;
        let identity = self
            .control_supervision_identity()
            .await
            .map_err(|error| match error {
                super::ControlIdentityError::Unauthorized => Error::Authentication,
                super::ControlIdentityError::Forbidden => Error::Forbidden,
                super::ControlIdentityError::Unavailable => Error::Unavailable,
                super::ControlIdentityError::InvalidResponse => Error::Protocol,
            })?;
        if identity.role != "runner"
            || identity.control_namespace_uuid.as_deref()
                != Some(request.admission.control_namespace_uuid.as_str())
        {
            return Err(Error::Forbidden);
        }
        check([6, 7, 8, 9, 10, 11, 12, 13].contains(&identity.schema_version))?;
        self.validate_identity_payload(request)
            .map_err(|_| Error::InvalidRequest)?;
        let bytes = serde_json::to_vec(request).map_err(|_| Error::InvalidRequest)?;
        if bytes.len() > 16_384 {
            return Err(Error::InvalidRequest);
        }
        let path = format!(
            "/api/v1/relay/channels/{}/reconcile",
            request.admission.channel_id
        );
        let url = self
            .control_url(reqwest::Method::POST, &path)
            .map_err(|_| Error::InvalidRequest)?;
        let mut response = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(bytes)
            .send()
            .await
            .map_err(|_| Error::Unavailable)?;
        use reqwest::StatusCode as S;
        match response.status() {
            S::OK => {}
            S::UNAUTHORIZED => return Err(Error::Authentication),
            S::FORBIDDEN => return Err(Error::Forbidden),
            S::CONFLICT => return Err(Error::Conflict),
            S::TOO_MANY_REQUESTS => return Err(Error::Capacity),
            S::BAD_REQUEST | S::PAYLOAD_TOO_LARGE => return Err(Error::InvalidRequest),
            S::CREATED => return Err(Error::Protocol),
            // An old/absent route (404), loss or other status supplies no
            // historical proof and never falls back to a mutation endpoint.
            _ => return Err(Error::Unavailable),
        }
        check(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|header| header.to_str().ok())
                == Some("application/json"),
        )?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Unavailable)? {
            if bytes.len().saturating_add(chunk.len()) > 16_384 {
                return Err(Error::Protocol);
            }
            bytes.extend_from_slice(&chunk);
        }
        let json = crate::json::decode_unique(&bytes, 64).map_err(|_| Error::Protocol)?;
        let result: RelayChannelReconciliationV1 =
            serde_json::from_value(json).map_err(|_| Error::Protocol)?;
        history_context(&result, &identity.tenant_id, request)?;
        Ok(result)
    }

    async fn relay_channel_identity(
        &self,
        tenant: &str,
        namespace: &str,
        active: bool,
    ) -> Result<()> {
        let identity = self
            .control_supervision_identity()
            .await
            .map_err(|e| match e {
                super::ControlIdentityError::Unauthorized => Error::Authentication,
                super::ControlIdentityError::Forbidden => Error::Forbidden,
                super::ControlIdentityError::Unavailable => Error::Unavailable,
                super::ControlIdentityError::InvalidResponse => Error::Protocol,
            })?;
        if identity.tenant_id != tenant
            || identity.control_namespace_uuid.as_deref() != Some(namespace)
            || identity.role != "runner"
            || active && !identity.active
        {
            return Err(Error::Forbidden);
        }
        check([6, 7, 8, 9, 10, 11, 12, 13].contains(&identity.schema_version))
    }
    async fn relay_channel_post(
        &self,
        path: &str,
        input: &impl Serialize,
        creates: bool,
    ) -> Result<(reqwest::StatusCode, RelayChannelResultV1)> {
        self.validate_identity_payload(input)
            .map_err(|_| Error::InvalidRequest)?;
        let bytes = serde_json::to_vec(input).map_err(|_| Error::InvalidRequest)?;
        if bytes.len() > 16_384 {
            return Err(Error::InvalidRequest);
        }
        let url = self
            .control_url(reqwest::Method::POST, path)
            .map_err(|_| Error::InvalidRequest)?;
        let mut response = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(bytes)
            .send()
            .await
            .map_err(|_| Error::Unavailable)?;
        use reqwest::StatusCode as S;
        let status = response.status();
        match status {
            S::OK | S::CREATED => {}
            S::UNAUTHORIZED => return Err(Error::Authentication),
            S::FORBIDDEN => return Err(Error::Forbidden),
            S::CONFLICT => return Err(Error::Conflict),
            S::TOO_MANY_REQUESTS => return Err(Error::Capacity),
            S::BAD_REQUEST => return Err(Error::InvalidRequest),
            _ => return Err(Error::Unavailable),
        }
        check(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|h| h.to_str().ok())
                == Some("application/json"),
        )?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Unavailable)? {
            if bytes.len().saturating_add(chunk.len()) > 16_384 {
                return Err(Error::Protocol);
            }
            bytes.extend_from_slice(&chunk);
        }
        let json = crate::json::decode_unique(&bytes, 64).map_err(|_| Error::Protocol)?;
        let result: RelayChannelResultV1 =
            serde_json::from_value(json).map_err(|_| Error::Protocol)?;
        check(
            status
                == if result.replayed || !creates {
                    S::OK
                } else {
                    S::CREATED
                },
        )?;
        Ok((status, result))
    }
    pub(crate) async fn reserve_relay_channel(
        &self,
        tenant: &str,
        input: &RelayChannelAdmissionV1,
    ) -> Result<RelayChannelResultV1> {
        input.validate().map_err(|_| Error::InvalidRequest)?;
        self.relay_channel_identity(tenant, &input.control_namespace_uuid, true)
            .await?;
        let (_, result) = self
            .relay_channel_post("/api/v1/relay/channels/admissions", input, true)
            .await?;
        context(
            &result,
            tenant,
            input,
            &input.operation_id,
            &input.input_hash().map_err(|_| Error::InvalidRequest)?,
        )?;
        if !result.replayed {
            check(
                result.channel.state == RelayChannelStateV1::Reserved
                    && result.channel.revision == 1,
            )?;
        }
        Ok(result)
    }
    pub(crate) async fn claim_relay_channel_exec(
        &self,
        tenant: &str,
        admission: &RelayChannelAdmissionV1,
        claim: &RelayChannelClaimV1,
    ) -> Result<RelayExecClaimResult> {
        admission.validate().map_err(|_| Error::InvalidRequest)?;
        claim.validate().map_err(|_| Error::InvalidRequest)?;
        check(
            claim.channel_id == admission.channel_id
                && claim.epoch == admission.epoch
                && claim.expected_revision == 1
                && claim.admission_input_hash
                    == admission.input_hash().map_err(|_| Error::InvalidRequest)?,
        )?;
        self.relay_channel_identity(tenant, &admission.control_namespace_uuid, true)
            .await?;
        let (_, result) = self
            .relay_channel_post(
                &format!("/api/v1/relay/channels/{}/claims", claim.channel_id),
                claim,
                true,
            )
            .await?;
        context(
            &result,
            tenant,
            admission,
            &claim.operation_id,
            &claim.input_hash().map_err(|_| Error::InvalidRequest)?,
        )?;
        if result.replayed {
            return Ok(RelayExecClaimResult::Replayed(result));
        }
        check(
            result.channel.state == RelayChannelStateV1::AcceptedBeforeExec
                && result.channel.revision == 2,
        )?;
        Ok(RelayExecClaimResult::Fresh(FreshRelayExecClaim {
            evidence: Arc::new(ClaimEvidence {
                tenant_id: tenant.into(),
                admission: admission.clone(),
                claim: claim.clone(),
                receipt: result.receipt,
            }),
        }))
    }
    pub(crate) async fn current_relay_channel(
        &self,
        authority: &CurrentRelayAuthority,
    ) -> Result<RelayChannelSnapshotV1> {
        let e = &authority.evidence;
        self.relay_channel_identity(&e.tenant_id, &e.admission.control_namespace_uuid, true)
            .await?;
        let (_, result) = self
            .relay_channel_post(
                &format!("/api/v1/relay/channels/{}/claims", e.claim.channel_id),
                &e.claim,
                true,
            )
            .await?;
        context(
            &result,
            &e.tenant_id,
            &e.admission,
            &e.claim.operation_id,
            &e.claim.input_hash().map_err(|_| Error::InvalidRequest)?,
        )?;
        check(result.replayed && result.receipt == e.receipt)?;
        if !result.channel.state.current_dispatch() {
            return Err(Error::Forbidden);
        }
        Ok(result.channel)
    }
    pub(crate) async fn transition_relay_channel(
        &self,
        tenant: &str,
        admission: &RelayChannelAdmissionV1,
        input: &RelayChannelTransitionV1,
    ) -> Result<RelayChannelResultV1> {
        admission.validate().map_err(|_| Error::InvalidRequest)?;
        input.validate().map_err(|_| Error::InvalidRequest)?;
        check(
            input.channel_id == admission.channel_id
                && input.epoch == admission.epoch
                && input.admission_input_hash
                    == admission.input_hash().map_err(|_| Error::InvalidRequest)?,
        )?;
        self.relay_channel_identity(tenant, &admission.control_namespace_uuid, false)
            .await?;
        let (_, result) = self
            .relay_channel_post(
                &format!("/api/v1/relay/channels/{}/transitions", input.channel_id),
                input,
                false,
            )
            .await?;
        context(
            &result,
            tenant,
            admission,
            &input.operation_id,
            &input.input_hash().map_err(|_| Error::InvalidRequest)?,
        )?;
        Ok(result)
    }
}
