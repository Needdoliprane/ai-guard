//! Exact V2 session Pod request. The renderer is pure and deterministic: it
//! takes the confirmed reservation intent, the preparation binding that the
//! journal derives from its durable record, and the workspace claim named for
//! this attempt. That binding is a crate-internal convention, not a type
//! guarantee: its fields are visible inside `runtime::kubernetes`, and
//! `render` checks its ids, reservation, authority and profile against the
//! intent but its two digests for format only. A rendered request grants no
//! CREATE, release, attach, start, ownership or quota; its digest stays a
//! declared broker commitment until the central contract re-derives it. The
//! request bytes name the claim but do not carry its UID (a Pod binds a PVC by
//! name only), so the retained claim UID must be bound centrally.
//!
//! Workspace claims are single-use per attempt: the claim name is derived from
//! the attempt id, so another attempt cannot render against it. The reuse of a
//! claim UID across reservations is unproven and must be refused centrally.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "V2 request mechanics without a production caller (DESIGN section 0, layer B)"
    )
)]
use super::{
    KubernetesError, KubernetesPodHandle, KubernetesResult,
    admission::{cpu_quantity, mib},
    session::SessionBootstrapBindingV2,
    session_instance::KubernetesSessionWorkspaceClaimV2,
    session_profile::KubernetesSessionIdsV2,
    valid_digest,
};
use crate::control::kubernetes_session::KubernetesSessionReservationIntentV2;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub(crate) const SESSION_POD_RENDERER: &str = "kubernetes_session_pod_v2r1";
/// Effectful renderer (G11): terminal finalizer and protocol v3 modes. The r1
/// renderer stays unchanged so retained before-effect journals revalidate.
pub(crate) const SESSION_POD_RENDERER_V2R2: &str = "kubernetes_session_pod_v2r2";
pub(crate) const SESSION_SCHEDULING_GATE: &str = "ai-guard.io/session-v2";
/// Keeps the Pod object until the broker has read its terminal state by UID
/// (decision D5). Removed only by an exact JSON Patch after that observation.
pub(crate) const SESSION_TERMINAL_FINALIZER: &str = "ai-guard.io/session-terminal";
/// Protocol v3 modes of the existing bootstrap binary (R4). One fixed
/// argument each; neither is a business invocation.
pub(crate) const SESSION_RUN_MODE_ARG: &str = "kubernetes-session-v3";
pub(crate) const WORKSPACE_EXPORT_MODE_ARG: &str = "kubernetes-workspace-export-v3";
/// Runtime bound of the export helper Pod: a defence, not a measured need.
pub(crate) const EXPORT_DEADLINE_SECONDS: u32 = 1800;
/// Second argv entry of the existing bootstrap program. A two-entry argv is
/// never a business invocation (that needs `exec` and at least ten entries).
/// Spelling pending the Root decision Q6; not public API.
pub(crate) const SESSION_WAIT_MODE_ARG: &str = "kubernetes-session-wait-v2";
/// Same bound as the V1 retained CREATE body.
const MAX_REQUEST_BYTES: usize = 32 * 1024;
/// Writable-layer bound for the read-only root: a design choice, not a
/// measured need and not an enforcement proof.
const SESSION_EPHEMERAL_STORAGE: &str = "16Mi";
const REQUEST_DOMAIN: &[u8] = b"ai-guard:kubernetes-session-pod-request:v2\0";

/// `ai-guard-session-<attempt simple>`: 49 characters, a DNS label.
pub(crate) fn session_pod_name(attempt_id: Uuid) -> String {
    format!("ai-guard-session-{}", attempt_id.simple())
}
/// `ai-guard-ws-<attempt simple>`: 44 characters, a DNS label.
pub(crate) fn session_workspace_claim_name(attempt_id: Uuid) -> String {
    format!("ai-guard-ws-{}", attempt_id.simple())
}
/// `ai-guard-export-<attempt simple>`: 48 characters, a DNS label.
pub(crate) fn session_export_pod_name(attempt_id: Uuid) -> String {
    format!("ai-guard-export-{}", attempt_id.simple())
}

/// What an effectful (r2) Pod of one attempt does. The session Pod runs the
/// protocol v3 supervisor; the export helper mounts the same claim read-only
/// after the session Pod's terminal was observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionPodPurposeV2 {
    Session,
    Export,
}
#[derive(Clone, Copy)]
enum Shape {
    WaitingR1,
    Effectful(SessionPodPurposeV2),
}

/// Audit-safe commitments of one rendered request. Private fields, built only
/// by `render`; no Deserialize. Names never designate ownership.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct KubernetesSessionPodTargetV2 {
    version: u32,
    renderer: String,
    reservation_id: String,
    reservation_input_hash: String,
    session_id: String,
    attempt_id: String,
    authority_fingerprint: String,
    preparation_sha256: String,
    pod_name: String,
    workspace_claim_name: String,
    workspace_claim_uid: String,
    request_sha256: String,
    requested_configuration_sha256: String,
}
impl KubernetesSessionPodTargetV2 {
    pub(crate) fn authority_fingerprint(&self) -> &str {
        &self.authority_fingerprint
    }
    pub(crate) fn preparation_sha256(&self) -> &str {
        &self.preparation_sha256
    }
    pub(crate) fn pod_name(&self) -> &str {
        &self.pod_name
    }
    pub(crate) fn workspace_claim_name(&self) -> &str {
        &self.workspace_claim_name
    }
    pub(crate) fn workspace_claim_uid(&self) -> &str {
        &self.workspace_claim_uid
    }
    pub(crate) fn request_sha256(&self) -> &str {
        &self.request_sha256
    }
}

/// Exact canonical request bytes (at most 32 KiB) with their commitments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KubernetesSessionPodRequestV2 {
    target: KubernetesSessionPodTargetV2,
    bytes: Vec<u8>,
}
impl KubernetesSessionPodRequestV2 {
    /// Pure and deterministic; no effect, permit or ownership. Refuses
    /// (`InvalidPreflightPlan`) a preparation binding that is not this
    /// intent's, and a claim observed through another authority or namespace
    /// or not named `session_workspace_claim_name(attempt_id)`.
    pub(crate) fn render(
        intent: &KubernetesSessionReservationIntentV2,
        preparation: &SessionBootstrapBindingV2,
        claim: &KubernetesSessionWorkspaceClaimV2,
    ) -> KubernetesResult<Self> {
        Self::render_shape(intent, preparation, claim, Shape::WaitingR1)
    }
    /// Effectful r2 request of one purpose. Same refusals as `render`; its
    /// bytes depend only on the intent and the purpose, so the central ledger
    /// re-derives them with `effectful_request_bytes`.
    pub(crate) fn render_effectful(
        intent: &KubernetesSessionReservationIntentV2,
        preparation: &SessionBootstrapBindingV2,
        claim: &KubernetesSessionWorkspaceClaimV2,
        purpose: SessionPodPurposeV2,
    ) -> KubernetesResult<Self> {
        Self::render_shape(intent, preparation, claim, Shape::Effectful(purpose))
    }
    fn render_shape(
        intent: &KubernetesSessionReservationIntentV2,
        preparation: &SessionBootstrapBindingV2,
        claim: &KubernetesSessionWorkspaceClaimV2,
        shape: Shape,
    ) -> KubernetesResult<Self> {
        let invalid = KubernetesError::InvalidPreflightPlan;
        intent.validate().map_err(|_| invalid)?;
        let authority = intent.authority();
        let ids = intent.ids().map_err(|_| invalid)?;
        let fingerprint = authority.fingerprint();
        let claim_name = session_workspace_claim_name(ids.attempt_id);
        if preparation.version != 2
            || preparation.installation_id != ids.installation_id
            || preparation.control_namespace_uuid != ids.control_namespace_uuid
            || preparation.broker_id != ids.broker_id
            || preparation.incarnation_id != ids.incarnation_id
            || preparation.session_id != ids.session_id
            || preparation.attempt_id != ids.attempt_id
            || preparation.reservation_id != intent.reservation_id()
            || preparation.authority_fingerprint != fingerprint
            || preparation.profile_hash != intent.profile_hash().map_err(|_| invalid)?
            || !valid_digest(&preparation.reservation_input_hash)
            || !valid_digest(&preparation.preparation_sha256)
            || !claim.is_for(authority, &claim_name)
        {
            return Err(invalid);
        }
        let pod = pod(intent, &ids, claim.name(), shape);
        let bytes = canonical_request(&pod)?;
        let (renderer, pod_name) = match shape {
            Shape::WaitingR1 => (SESSION_POD_RENDERER, session_pod_name(ids.attempt_id)),
            Shape::Effectful(SessionPodPurposeV2::Session) => {
                (SESSION_POD_RENDERER_V2R2, session_pod_name(ids.attempt_id))
            }
            Shape::Effectful(SessionPodPurposeV2::Export) => (
                SESSION_POD_RENDERER_V2R2,
                session_export_pod_name(ids.attempt_id),
            ),
        };
        let request_sha256 = request_sha256(&fingerprint, &bytes);
        let target = KubernetesSessionPodTargetV2 {
            version: 2,
            renderer: renderer.into(),
            reservation_id: intent.reservation_id().into(),
            reservation_input_hash: preparation.reservation_input_hash.clone(),
            session_id: ids.session_id.to_string(),
            attempt_id: ids.attempt_id.to_string(),
            authority_fingerprint: fingerprint,
            preparation_sha256: preparation.preparation_sha256.clone(),
            pod_name,
            workspace_claim_name: claim_name,
            workspace_claim_uid: claim.uid().into(),
            request_sha256,
            requested_configuration_sha256: KubernetesPodHandle::configuration_digest(&pod["spec"])
                .map_err(|_| invalid)?,
        };
        Ok(Self { target, bytes })
    }
    pub(crate) fn target(&self) -> &KubernetesSessionPodTargetV2 {
        &self.target
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Central re-derivation of one effectful request: exact bytes and digest
/// from the confirmed intent and the purpose only. No preparation, claim
/// observation or journal value enters the bytes.
pub(crate) fn effectful_request_bytes(
    intent: &KubernetesSessionReservationIntentV2,
    purpose: SessionPodPurposeV2,
) -> KubernetesResult<(Vec<u8>, String)> {
    let invalid = KubernetesError::InvalidPreflightPlan;
    intent.validate().map_err(|_| invalid)?;
    let ids = intent.ids().map_err(|_| invalid)?;
    let claim_name = session_workspace_claim_name(ids.attempt_id);
    let bytes = canonical_request(&pod(intent, &ids, &claim_name, Shape::Effectful(purpose)))?;
    let digest = request_sha256(&intent.authority().fingerprint(), &bytes);
    Ok((bytes, digest))
}
fn canonical_request(pod: &Value) -> KubernetesResult<Vec<u8>> {
    let invalid = KubernetesError::InvalidPreflightPlan;
    let bytes =
        crate::control::kubernetes_session::validation::canonical(pod).map_err(|_| invalid)?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(invalid);
    }
    Ok(bytes)
}
fn request_sha256(fingerprint: &str, bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(REQUEST_DOMAIN);
    hash.update(fingerprint.as_bytes());
    hash.update(bytes);
    hex::encode(hash.finalize())
}

/// Closed PodSpec. It inherits the V1 baseline flags and renders no `false`
/// for a non-pointer omitempty bool except the five keys the shared admission
/// validator restores (`hostNetwork`, `hostPID`, `hostIPC`, `stdin`, `tty`),
/// and no `readOnly` key equal to `false` (declared external fact EXT-14, to
/// confirm). The command is one fixed mode of the existing bootstrap binary:
/// no business argv, environment, Secret, token, host path or init container.
///
/// r1 is the unchanged waiting-mode request. r2 adds the terminal finalizer
/// and the protocol v3 mode; its export helper is named for the attempt,
/// mounts the claim read-only and has its own runtime bound.
fn pod(
    intent: &KubernetesSessionReservationIntentV2,
    ids: &KubernetesSessionIdsV2,
    claim_name: &str,
    shape: Shape,
) -> Value {
    let resources = intent.resources();
    let limits = json!({"cpu":cpu_quantity(resources.cpu_millis),
        "memory":mib(resources.memory_mib),"ephemeral-storage":SESSION_EPHEMERAL_STORAGE});
    let security = json!({"runAsNonRoot":true,"runAsUser":1000,"runAsGroup":1000,
        "privileged":false,"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,
        "capabilities":{"drop":["ALL"]},"seccompProfile":{"type":"RuntimeDefault"}});
    let export = matches!(shape, Shape::Effectful(SessionPodPurposeV2::Export));
    let mode = match shape {
        Shape::WaitingR1 => SESSION_WAIT_MODE_ARG,
        Shape::Effectful(SessionPodPurposeV2::Session) => SESSION_RUN_MODE_ARG,
        Shape::Effectful(SessionPodPurposeV2::Export) => WORKSPACE_EXPORT_MODE_ARG,
    };
    let workspace_mount = if export {
        json!({"name":"workspace","mountPath":"/workspace","readOnly":true})
    } else {
        json!({"name":"workspace","mountPath":"/workspace"})
    };
    let worker = json!({
        "name":"worker","image":intent.profile().retained_image(),"imagePullPolicy":"Never",
        "command":[crate::provider::BOOTSTRAP_PROGRAM, mode],
        "workingDir":"/workspace","stdin":true,"stdinOnce":true,"tty":false,
        "terminationMessagePath":"/dev/termination-log","terminationMessagePolicy":"File",
        "securityContext":security,"resources":{"requests":limits,"limits":limits},
        "volumeMounts":[workspace_mount,
            {"name":"temporary","mountPath":"/tmp"},{"name":"home","mountPath":"/home/agent"}]
    });
    let pod_security = json!({"runAsNonRoot":true,"runAsUser":1000,"runAsGroup":1000,
        "fsGroup":1000,"fsGroupChangePolicy":"OnRootMismatch",
        "seccompProfile":{"type":"RuntimeDefault"}});
    let workspace_volume = if export {
        json!({"name":"workspace","persistentVolumeClaim":{"claimName":claim_name,"readOnly":true}})
    } else {
        json!({"name":"workspace","persistentVolumeClaim":{"claimName":claim_name}})
    };
    let volumes = json!([
        workspace_volume,
        {"name":"temporary","emptyDir":{"medium":"Memory","sizeLimit":mib(resources.temporary_mib)}},
        {"name":"home","emptyDir":{"medium":"Memory","sizeLimit":mib(resources.home_mib)}}
    ]);
    let deadline = if export {
        EXPORT_DEADLINE_SECONDS
    } else {
        resources.deadline_seconds
    };
    // activeDeadlineSeconds is a runtime defense only, never quiescence proof.
    let spec = json!({
        "automountServiceAccountToken":false,"serviceAccountName":"default","enableServiceLinks":false,
        "hostNetwork":false,"hostPID":false,"hostIPC":false,"shareProcessNamespace":false,
        "restartPolicy":"Never","activeDeadlineSeconds":deadline,
        "terminationGracePeriodSeconds":30,"dnsPolicy":"None","dnsConfig":{"nameservers":["127.0.0.1"]},
        "schedulerName":"default-scheduler","schedulingGates":[{"name":SESSION_SCHEDULING_GATE}],
        "securityContext":pod_security,"containers":[worker],"volumes":volumes
    });
    let (name, purpose) = if export {
        (session_export_pod_name(ids.attempt_id), "export-v2")
    } else {
        (session_pod_name(ids.attempt_id), "session-v2")
    };
    // Labels are descriptive selectors only, never identity or authority.
    let mut metadata = json!({
        "name":name,"namespace":intent.authority().namespace(),
        "labels":{"app.kubernetes.io/managed-by":"ai-guard",
            "ai-guard/session":ids.session_id.to_string(),
            "ai-guard/attempt":ids.attempt_id.to_string(),
            "ai-guard/purpose":purpose}});
    if matches!(shape, Shape::Effectful(_)) {
        metadata["finalizers"] = json!([SESSION_TERMINAL_FINALIZER]);
    }
    json!({"apiVersion":"v1","kind":"Pod","metadata":metadata,"spec":spec})
}

#[cfg(test)]
#[path = "session_pod_tests.rs"]
pub(crate) mod tests;
