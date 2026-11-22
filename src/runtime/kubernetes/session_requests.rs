//! Exact bodies and digests of the PATCH and DELETE effect requests.
//!
//! One pure source for the runtime effect functions (which send these exact
//! bodies) and for the central effect ledger (which re-derives the digest a
//! broker claims before granting `EmitOnce`). The digest is the transport's
//! `effect_request_sha256` over the fixed route the transport builds for the
//! same effect, so a permit for one of these digests matches only that
//! request.
use super::{
    KubernetesAuthorityV1,
    session_effect::{SessionEffectKindV2, effect_request_sha256},
    session_instance::KubernetesSessionRuntimeError,
    session_pod::{SESSION_SCHEDULING_GATE, SESSION_TERMINAL_FINALIZER},
    valid_dns_label,
};
use serde_json::{Map, Value, json};

type Outcome<T> = Result<T, KubernetesSessionRuntimeError>;
const REFUSED: KubernetesSessionRuntimeError =
    KubernetesSessionRuntimeError::EffectRefusedBeforeSend;

/// Body and digest of one effect request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionEffectRequestBodyV2 {
    pub(crate) body: Vec<u8>,
    pub(crate) request_sha256: String,
}

fn canonical(value: &Value) -> Outcome<Vec<u8>> {
    crate::control::kubernetes_session::validation::canonical(value).map_err(|_| REFUSED)
}
fn pod_path(authority: &KubernetesAuthorityV1, name: &str, query: &str) -> Outcome<String> {
    if !valid_dns_label(name) || !valid_dns_label(authority.namespace()) {
        return Err(REFUSED);
    }
    Ok(format!(
        "/api/v1/namespaces/{}/pods/{name}{query}",
        authority.namespace()
    ))
}
fn finish(
    kind: SessionEffectKindV2,
    authority: &KubernetesAuthorityV1,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> SessionEffectRequestBodyV2 {
    let request_sha256 = effect_request_sha256(kind, &authority.fingerprint(), method, path, &body);
    SessionEffectRequestBodyV2 {
        body,
        request_sha256,
    }
}

/// Gate removal tested on the UID, the observed resourceVersion and the gate.
pub(crate) fn release_gate_request(
    authority: &KubernetesAuthorityV1,
    pod_name: &str,
    pod_uid: &str,
    resource_version: &str,
) -> Outcome<SessionEffectRequestBodyV2> {
    let path = pod_path(authority, pod_name, "?fieldValidation=Strict")?;
    let body = canonical(&json!([
        {"op":"test","path":"/metadata/uid","value":pod_uid},
        {"op":"test","path":"/metadata/resourceVersion","value":resource_version},
        {"op":"test","path":"/spec/schedulingGates","value":[{"name":SESSION_SCHEDULING_GATE}]},
        {"op":"remove","path":"/spec/schedulingGates"}
    ]))?;
    Ok(finish(
        SessionEffectKindV2::ReleaseGate,
        authority,
        "PATCH",
        &path,
        body,
    ))
}

/// Terminal finalizer removal tested on the UID and the exact finalizers.
pub(crate) fn release_finalizer_request(
    authority: &KubernetesAuthorityV1,
    pod_name: &str,
    pod_uid: &str,
) -> Outcome<SessionEffectRequestBodyV2> {
    let path = pod_path(authority, pod_name, "?fieldValidation=Strict")?;
    let body = canonical(&json!([
        {"op":"test","path":"/metadata/uid","value":pod_uid},
        {"op":"test","path":"/metadata/finalizers","value":[SESSION_TERMINAL_FINALIZER]},
        {"op":"remove","path":"/metadata/finalizers"}
    ]))?;
    Ok(finish(
        SessionEffectKindV2::ReleaseFinalizer,
        authority,
        "PATCH",
        &path,
        body,
    ))
}

/// Pod DELETE with the UID precondition, plus the resourceVersion one for a
/// Held observation. No grace period override is ever sent.
pub(crate) fn delete_pod_request(
    authority: &KubernetesAuthorityV1,
    pod_name: &str,
    pod_uid: &str,
    held_resource_version: Option<&str>,
) -> Outcome<SessionEffectRequestBodyV2> {
    let path = pod_path(authority, pod_name, "")?;
    let mut preconditions = Map::new();
    preconditions.insert("uid".into(), json!(pod_uid));
    if let Some(version) = held_resource_version {
        preconditions.insert("resourceVersion".into(), json!(version));
    }
    let body = canonical(&json!({
        "apiVersion":"v1","kind":"DeleteOptions",
        "preconditions":Value::Object(preconditions),"propagationPolicy":"Background"
    }))?;
    Ok(finish(
        SessionEffectKindV2::DeleteExact,
        authority,
        "DELETE",
        &path,
        body,
    ))
}

/// Claim DELETE with the UID precondition.
pub(crate) fn delete_claim_request(
    authority: &KubernetesAuthorityV1,
    claim_name: &str,
    claim_uid: &str,
) -> Outcome<SessionEffectRequestBodyV2> {
    if !valid_dns_label(claim_name) || !valid_dns_label(authority.namespace()) {
        return Err(REFUSED);
    }
    let path = format!(
        "/api/v1/namespaces/{}/persistentvolumeclaims/{claim_name}",
        authority.namespace()
    );
    let body = canonical(&json!({
        "apiVersion":"v1","kind":"DeleteOptions",
        "preconditions":{"uid":claim_uid},"propagationPolicy":"Background"
    }))?;
    Ok(finish(
        SessionEffectKindV2::DeleteClaimExact,
        authority,
        "DELETE",
        &path,
        body,
    ))
}
