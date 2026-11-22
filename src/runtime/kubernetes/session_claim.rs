//! Per-attempt workspace claim created by the broker (R5, decision D4).
//!
//! The request is pure and deterministic: it is rendered from the confirmed
//! reservation intent and the tenant storage request only, so the central
//! ledger can re-render it and compare its digest before granting the effect.
//! The claim name derives from the attempt id; a claim is single-use and its
//! UID must be bound centrally (Q7) before a Pod is rendered against it.
//!
//! The CREATE and DELETE effects consume a guard of the sealed
//! `SessionEffectGuardV2`. A validated 201 is HTTP provenance of one exact
//! object, not a binding, a capacity or a byte-quota proof. Every other answer
//! of an initiated CREATE leaves ownership unknown: an object may exist, and
//! no second POST follows.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "R5 claim mechanics without a production caller until the broker driver (K4) lands"
    )
)]
use super::{
    KubernetesAuthorityV1, KubernetesError, KubernetesResult, KubernetesRuntime,
    session_effect::{
        SessionDeleteOutcomeV2, SessionEffectGuardV2, SessionHttpEffectV2, SessionHttpResponseV2,
    },
    session_instance::{KubernetesSessionRuntimeError, KubernetesSessionWorkspaceClaimV2},
    session_pod::session_workspace_claim_name,
    session_requests::delete_claim_request,
    valid_dns_label, valid_uid,
};
use crate::control::kubernetes_session::KubernetesSessionReservationIntentV2;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) const SESSION_CLAIM_RENDERER: &str = "kubernetes_session_claim_v2r1";
pub(crate) const READ_WRITE_ONCE_POD: &str = "ReadWriteOncePod";
const REQUEST_DOMAIN: &[u8] = b"ai-guard:kubernetes-session-claim-request:v2\0";
const MAX_REQUEST_BYTES: usize = 4 * 1024;
/// 256 GiB: a configuration bound, not a measured need. No provisioner byte
/// enforcement follows from it.
pub(crate) const MAX_CAPACITY_MIB: u32 = 262_144;
const PVC_PROTECTION_FINALIZER: &str = "kubernetes.io/pvc-protection";
type Outcome<T> = Result<T, KubernetesSessionRuntimeError>;
const REFUSED: KubernetesSessionRuntimeError =
    KubernetesSessionRuntimeError::EffectRefusedBeforeSend;
const INVALID: KubernetesSessionRuntimeError =
    KubernetesSessionRuntimeError::Kubernetes(KubernetesError::InvalidResponse);

/// Tenant storage request for one attempt. Server configuration only: the
/// central ledger compares it with the tenant storage policy before any
/// CREATE. A storage class name is not a proof of CSI, `Immediate` binding or
/// `ReadWriteOncePod` support; the admission check observes the class.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesSessionStorageRequestV2 {
    pub version: u32,
    pub storage_class_name: String,
    pub capacity_mib: u32,
}
impl KubernetesSessionStorageRequestV2 {
    pub fn validate(&self) -> KubernetesResult<()> {
        if self.version == 1
            && valid_subdomain(&self.storage_class_name)
            && (1..=MAX_CAPACITY_MIB).contains(&self.capacity_mib)
        {
            Ok(())
        } else {
            Err(KubernetesError::InvalidPreflightPlan)
        }
    }
}

/// Audit-safe commitments of one rendered claim request. Built only by
/// `render`; names never designate ownership.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct KubernetesSessionClaimTargetV2 {
    version: u32,
    renderer: String,
    reservation_id: String,
    session_id: String,
    attempt_id: String,
    authority_fingerprint: String,
    claim_name: String,
    storage_class_name: String,
    capacity: String,
    request_sha256: String,
}
impl KubernetesSessionClaimTargetV2 {
    pub(crate) fn authority_fingerprint(&self) -> &str {
        &self.authority_fingerprint
    }
    pub(crate) fn claim_name(&self) -> &str {
        &self.claim_name
    }
    pub(crate) fn request_sha256(&self) -> &str {
        &self.request_sha256
    }
}

/// Exact canonical claim request bytes with their commitments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KubernetesSessionClaimRequestV2 {
    target: KubernetesSessionClaimTargetV2,
    bytes: Vec<u8>,
}
impl KubernetesSessionClaimRequestV2 {
    /// Pure and deterministic; no effect, permit or ownership.
    pub(crate) fn render(
        intent: &KubernetesSessionReservationIntentV2,
        storage: &KubernetesSessionStorageRequestV2,
    ) -> KubernetesResult<Self> {
        let invalid = KubernetesError::InvalidPreflightPlan;
        intent.validate().map_err(|_| invalid)?;
        storage.validate()?;
        let ids = intent.ids().map_err(|_| invalid)?;
        let authority = intent.authority();
        let claim_name = session_workspace_claim_name(ids.attempt_id);
        let capacity = format!("{}Mi", storage.capacity_mib);
        let claim = json!({"apiVersion":"v1","kind":"PersistentVolumeClaim","metadata":{
            "name":claim_name,"namespace":authority.namespace(),
            "labels":{"app.kubernetes.io/managed-by":"ai-guard",
                "ai-guard/session":ids.session_id.to_string(),
                "ai-guard/attempt":ids.attempt_id.to_string(),
                "ai-guard/purpose":"workspace-v2"}},
            "spec":{"accessModes":[READ_WRITE_ONCE_POD],"volumeMode":"Filesystem",
                "storageClassName":storage.storage_class_name,
                "resources":{"requests":{"storage":capacity}}}});
        let bytes = crate::control::kubernetes_session::validation::canonical(&claim)
            .map_err(|_| invalid)?;
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(invalid);
        }
        let fingerprint = authority.fingerprint();
        let mut hash = Sha256::new();
        hash.update(REQUEST_DOMAIN);
        hash.update(fingerprint.as_bytes());
        hash.update(&bytes);
        Ok(Self {
            target: KubernetesSessionClaimTargetV2 {
                version: 2,
                renderer: SESSION_CLAIM_RENDERER.into(),
                reservation_id: intent.reservation_id().into(),
                session_id: ids.session_id.to_string(),
                attempt_id: ids.attempt_id.to_string(),
                authority_fingerprint: fingerprint,
                claim_name,
                storage_class_name: storage.storage_class_name.clone(),
                capacity,
                request_sha256: hex::encode(hash.finalize()),
            },
            bytes,
        })
    }
    pub(crate) fn target(&self) -> &KubernetesSessionClaimTargetV2 {
        &self.target
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Exact object created by this request: HTTP provenance of one 201 only.
/// Not Clone and not serializable; the binding to a reservation is central.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SessionClaimCreatedV2 {
    claim_name: String,
    uid: String,
    resource_version: String,
}
impl SessionClaimCreatedV2 {
    pub(crate) fn claim_name(&self) -> &str {
        &self.claim_name
    }
    pub(crate) fn uid(&self) -> &str {
        &self.uid
    }
    pub(crate) fn resource_version(&self) -> &str {
        &self.resource_version
    }
    /// The bound observation must be the very object of this 201.
    pub(crate) fn is_observed_as(&self, claim: &KubernetesSessionWorkspaceClaimV2) -> bool {
        claim.name() == self.claim_name && claim.uid() == self.uid
    }
}

#[derive(Debug)]
pub(crate) enum SessionClaimCreateOutcomeV2 {
    Created(SessionClaimCreatedV2),
    OwnershipUnknown {
        http_status: Option<u16>,
        cause: KubernetesSessionRuntimeError,
    },
}

/// 201 projection of the exact claim request. The server may add only the
/// documented metadata, the PVC protection finalizer and an empty or
/// `Pending` status; any other difference keeps ownership unknown.
pub(super) fn project_created_claim(
    request: &KubernetesSessionClaimRequestV2,
    authority: &KubernetesAuthorityV1,
    response: &[u8],
) -> Outcome<SessionClaimCreatedV2> {
    let admitted = crate::json::decode_unique(response, 64).map_err(|_| INVALID)?;
    let requested: Value = serde_json::from_slice(request.bytes()).map_err(|_| INVALID)?;
    let fields = admitted.as_object().ok_or(INVALID)?;
    let metadata = fields
        .get("metadata")
        .and_then(Value::as_object)
        .ok_or(INVALID)?;
    let only = |map: &Map<String, Value>, allowed: &[&str]| {
        map.keys().all(|key| allowed.contains(&key.as_str()))
    };
    if !only(
        fields,
        &["apiVersion", "kind", "metadata", "spec", "status"],
    ) || !only(
        metadata,
        &[
            "name",
            "namespace",
            "uid",
            "resourceVersion",
            "labels",
            "annotations",
            "finalizers",
            "creationTimestamp",
            "managedFields",
        ],
    ) || fields.get("apiVersion") != requested.get("apiVersion")
        || fields.get("kind") != requested.get("kind")
        || metadata.get("name") != requested.pointer("/metadata/name")
        || metadata.get("namespace").and_then(Value::as_str) != Some(authority.namespace())
        || metadata.get("labels") != requested.pointer("/metadata/labels")
        || metadata
            .get("annotations")
            .is_some_and(|annotations| annotations.as_object().is_none_or(|map| !map.is_empty()))
        || metadata.get("finalizers").is_some_and(|finalizers| {
            finalizers.as_array().is_none_or(|values| {
                values.len() > 1
                    || values
                        .iter()
                        .any(|value| value.as_str() != Some(PVC_PROTECTION_FINALIZER))
            })
        })
        || fields.get("spec") != requested.get("spec")
        || fields.get("status").is_some_and(|status| {
            status.as_object().is_none_or(|status| {
                !status.is_empty()
                    && (status.len() != 1
                        || status.get("phase").and_then(Value::as_str) != Some("Pending"))
            })
        })
    {
        return Err(INVALID);
    }
    let uid = metadata
        .get("uid")
        .and_then(Value::as_str)
        .filter(|uid| valid_uid(uid))
        .ok_or(INVALID)?;
    let resource_version = metadata
        .get("resourceVersion")
        .and_then(Value::as_str)
        .filter(|version| valid_resource_version(version))
        .ok_or(INVALID)?;
    Ok(SessionClaimCreatedV2 {
        claim_name: request.target().claim_name().into(),
        uid: uid.into(),
        resource_version: resource_version.into(),
    })
}

impl KubernetesRuntime {
    /// CREATE of the exact retained claim bytes, sent at most once.
    pub(in crate::runtime::kubernetes) async fn create_session_workspace_claim<
        G: SessionEffectGuardV2,
    >(
        &self,
        request: &KubernetesSessionClaimRequestV2,
        guard: G,
        cancelled: &AtomicBool,
    ) -> Outcome<SessionClaimCreateOutcomeV2> {
        let target = request.target();
        self.require_claim_target(
            target.authority_fingerprint(),
            target.claim_name(),
            cancelled,
        )?;
        self.verify_active_authority()
            .await
            .map_err(KubernetesSessionRuntimeError::Kubernetes)?;
        let effect = SessionHttpEffectV2::CreateClaim { request };
        let unknown = |http_status, cause| SessionClaimCreateOutcomeV2::OwnershipUnknown {
            http_status,
            cause,
        };
        let created = match self
            .transport
            .send_session_effect(&self.binding, effect, guard, cancelled)
            .await?
        {
            SessionHttpResponseV2::CreatedClaim(response) => response,
            SessionHttpResponseV2::Complete { status, .. } => {
                return Ok(unknown(
                    Some(status),
                    KubernetesSessionRuntimeError::Kubernetes(KubernetesError::ApiRejected(status)),
                ));
            }
            SessionHttpResponseV2::CreatedResponse(_) => return Ok(unknown(Some(201), INVALID)),
            SessionHttpResponseV2::Uncertain { status, error } => {
                return Ok(unknown(
                    status,
                    KubernetesSessionRuntimeError::Kubernetes(error),
                ));
            }
        };
        let created = match created
            .into_bytes_for(request)
            .and_then(|bytes| project_created_claim(request, &self.binding, &bytes))
        {
            Ok(created) => created,
            Err(cause) => return Ok(unknown(Some(201), cause)),
        };
        if let Err(error) = self.verify_active_authority().await {
            return Ok(unknown(
                Some(201),
                KubernetesSessionRuntimeError::Kubernetes(error),
            ));
        }
        Ok(SessionClaimCreateOutcomeV2::Created(created))
    }

    /// DELETE of the exact claim by UID. Never a terminal or a quota proof:
    /// absence needs a later observation.
    pub(in crate::runtime::kubernetes) async fn delete_session_workspace_claim_exact<
        G: SessionEffectGuardV2,
    >(
        &self,
        claim: &KubernetesSessionWorkspaceClaimV2,
        guard: G,
        cancelled: &AtomicBool,
    ) -> Outcome<SessionDeleteOutcomeV2> {
        self.require_claim_target(claim.authority_fingerprint(), claim.name(), cancelled)?;
        if !valid_uid(claim.uid()) {
            return Err(REFUSED);
        }
        let body = delete_claim_request(&self.binding, claim.name(), claim.uid())?.body;
        self.verify_active_authority()
            .await
            .map_err(KubernetesSessionRuntimeError::Kubernetes)?;
        let effect = SessionHttpEffectV2::DeleteClaimExact {
            name: claim.name(),
            body,
        };
        let uncertain =
            |http_status, cause| SessionDeleteOutcomeV2::Uncertain { http_status, cause };
        let status = match self
            .transport
            .send_session_effect(&self.binding, effect, guard, cancelled)
            .await?
        {
            SessionHttpResponseV2::Complete { status, .. } => status,
            SessionHttpResponseV2::CreatedResponse(_) | SessionHttpResponseV2::CreatedClaim(_) => {
                return Ok(uncertain(Some(201), INVALID));
            }
            SessionHttpResponseV2::Uncertain { status, error } => {
                return Ok(uncertain(
                    status,
                    KubernetesSessionRuntimeError::Kubernetes(error),
                ));
            }
        };
        let outcome = match status {
            200 | 202 => SessionDeleteOutcomeV2::DeletionRequested,
            404 => SessionDeleteOutcomeV2::NotFound,
            409 => SessionDeleteOutcomeV2::PreconditionFailed,
            _ => {
                return Ok(uncertain(
                    Some(status),
                    KubernetesSessionRuntimeError::Kubernetes(KubernetesError::ApiRejected(status)),
                ));
            }
        };
        if let Err(error) = self.verify_active_authority().await {
            return Ok(uncertain(
                Some(status),
                KubernetesSessionRuntimeError::Kubernetes(error),
            ));
        }
        Ok(outcome)
    }

    fn require_claim_target(
        &self,
        authority_fingerprint: &str,
        claim_name: &str,
        cancelled: &AtomicBool,
    ) -> Outcome<()> {
        if authority_fingerprint != self.binding.fingerprint() || !valid_dns_label(claim_name) {
            return Err(KubernetesSessionRuntimeError::Kubernetes(
                KubernetesError::InvalidAuthority,
            ));
        }
        if cancelled.load(Ordering::Acquire) {
            return Err(REFUSED);
        }
        Ok(())
    }
}

fn valid_subdomain(value: &str) -> bool {
    !value.is_empty() && value.len() <= 253 && value.split('.').all(valid_dns_label)
}
/// Same opaque form as the session instance parser.
fn valid_resource_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
}

#[cfg(test)]
#[path = "session_claim_tests.rs"]
mod tests;
