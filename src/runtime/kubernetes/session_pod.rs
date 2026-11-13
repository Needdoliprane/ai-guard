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
pub(crate) const SESSION_SCHEDULING_GATE: &str = "ai-guard.io/session-v2";
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
        let pod = pod(intent, &ids, claim);
        let bytes =
            crate::control::kubernetes_session::validation::canonical(&pod).map_err(|_| invalid)?;
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(invalid);
        }
        let mut hash = Sha256::new();
        hash.update(REQUEST_DOMAIN);
        hash.update(fingerprint.as_bytes());
        hash.update(&bytes);
        let target = KubernetesSessionPodTargetV2 {
            version: 2,
            renderer: SESSION_POD_RENDERER.into(),
            reservation_id: intent.reservation_id().into(),
            reservation_input_hash: preparation.reservation_input_hash.clone(),
            session_id: ids.session_id.to_string(),
            attempt_id: ids.attempt_id.to_string(),
            authority_fingerprint: fingerprint,
            preparation_sha256: preparation.preparation_sha256.clone(),
            pod_name: session_pod_name(ids.attempt_id),
            workspace_claim_name: claim_name,
            workspace_claim_uid: claim.uid().into(),
            request_sha256: hex::encode(hash.finalize()),
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

/// Closed PodSpec. It inherits the V1 baseline flags and renders no `false`
/// for a non-pointer omitempty bool except the five keys the shared admission
/// validator restores (`hostNetwork`, `hostPID`, `hostIPC`, `stdin`, `tty`),
/// and no `readOnly` key (declared external fact EXT-14, to confirm). The
/// command is the waiting mode of the existing bootstrap binary: no business
/// argv, environment, Secret, token, host path or init container.
fn pod(
    intent: &KubernetesSessionReservationIntentV2,
    ids: &KubernetesSessionIdsV2,
    claim: &KubernetesSessionWorkspaceClaimV2,
) -> Value {
    let resources = intent.resources();
    let limits = json!({"cpu":cpu_quantity(resources.cpu_millis),
        "memory":mib(resources.memory_mib),"ephemeral-storage":SESSION_EPHEMERAL_STORAGE});
    let security = json!({"runAsNonRoot":true,"runAsUser":1000,"runAsGroup":1000,
        "privileged":false,"allowPrivilegeEscalation":false,"readOnlyRootFilesystem":true,
        "capabilities":{"drop":["ALL"]},"seccompProfile":{"type":"RuntimeDefault"}});
    let worker = json!({
        "name":"worker","image":intent.profile().retained_image(),"imagePullPolicy":"Never",
        "command":[crate::provider::BOOTSTRAP_PROGRAM, SESSION_WAIT_MODE_ARG],
        "workingDir":"/workspace","stdin":true,"stdinOnce":true,"tty":false,
        "terminationMessagePath":"/dev/termination-log","terminationMessagePolicy":"File",
        "securityContext":security,"resources":{"requests":limits,"limits":limits},
        "volumeMounts":[{"name":"workspace","mountPath":"/workspace"},
            {"name":"temporary","mountPath":"/tmp"},{"name":"home","mountPath":"/home/agent"}]
    });
    let pod_security = json!({"runAsNonRoot":true,"runAsUser":1000,"runAsGroup":1000,
        "fsGroup":1000,"fsGroupChangePolicy":"OnRootMismatch",
        "seccompProfile":{"type":"RuntimeDefault"}});
    let volumes = json!([
        {"name":"workspace","persistentVolumeClaim":{"claimName":claim.name()}},
        {"name":"temporary","emptyDir":{"medium":"Memory","sizeLimit":mib(resources.temporary_mib)}},
        {"name":"home","emptyDir":{"medium":"Memory","sizeLimit":mib(resources.home_mib)}}
    ]);
    // activeDeadlineSeconds is a runtime defense only, never quiescence proof.
    let spec = json!({
        "automountServiceAccountToken":false,"serviceAccountName":"default","enableServiceLinks":false,
        "hostNetwork":false,"hostPID":false,"hostIPC":false,"shareProcessNamespace":false,
        "restartPolicy":"Never","activeDeadlineSeconds":resources.deadline_seconds,
        "terminationGracePeriodSeconds":30,"dnsPolicy":"None","dnsConfig":{"nameservers":["127.0.0.1"]},
        "schedulerName":"default-scheduler","schedulingGates":[{"name":SESSION_SCHEDULING_GATE}],
        "securityContext":pod_security,"containers":[worker],"volumes":volumes
    });
    // Labels are descriptive selectors only, never identity or authority.
    json!({"apiVersion":"v1","kind":"Pod","metadata":{
        "name":session_pod_name(ids.attempt_id),"namespace":intent.authority().namespace(),
        "labels":{"app.kubernetes.io/managed-by":"ai-guard",
            "ai-guard/session":ids.session_id.to_string(),
            "ai-guard/attempt":ids.attempt_id.to_string(),
            "ai-guard/purpose":"session-v2"}},"spec":spec})
}

#[cfg(test)]
#[path = "session_pod_tests.rs"]
pub(crate) mod tests;
