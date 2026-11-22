//! V2 session instance, observation and workspace-claim facts. Every type here
//! is descriptive: a parsed claim, a name presence or a Pod observation is
//! never an identity, ownership, quota or effect capability. A
//! `SessionInstanceV2` comes only from a validated CREATE 201 (later also from
//! a central acknowledged instance), never from a journal file alone.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "V2 mechanics without a production caller (DESIGN section 0, layers B and C); only the workspace-claim inspection is public"
    )
)]
use super::{
    KubernetesAuthorityV1, KubernetesError, KubernetesPodHandle, KubernetesResult,
    KubernetesRuntime, observation, preflight,
    session_pod::{KubernetesSessionPodRequestV2, KubernetesSessionPodTargetV2},
    transport::CreatedSessionResponseV2,
    valid_digest, valid_dns_label, valid_uid,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::fmt;

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
/// Documented PVC protection finalizer (EXT-10: to confirm on v1.36).
const PVC_PROTECTION_FINALIZER: &str = "kubernetes.io/pvc-protection";
const READ_WRITE_ONCE_POD: &str = "ReadWriteOncePod";

/// Closed V2 runtime failures. No server body, URL, path or token is carried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KubernetesSessionRuntimeError {
    Kubernetes(KubernetesError),
    ProfileChanged,
    InstanceReplaced,
    InstanceRestarted,
    WorkspaceClaimInvalid,
    WorkspaceClaimPending,
    WorkspaceClaimAccessMode,
    WorkspaceClaimStorageClass,
    /// A local precondition, the consumed guard or a cancellation refused
    /// the effect: no effect request was sent.
    EffectRefusedBeforeSend,
    /// The exact instance is being deleted; its gate is never released.
    InstanceDeleting,
    /// The attach upgrade was initiated and its answer was not the exact
    /// accepted `101`: no stream is held.
    AttachHandshakeRefused,
    /// The attach stream broke the bounded v5 channel framing or its bounds,
    /// or the peer closed it with a code that this client refuses.
    AttachProtocol,
    /// The attach ended without two equal observations of the exact
    /// instance around the preamble.
    AttachIdentityUnproven,
    /// No attach slot was available.
    AttachCapacity,
    /// The attach deadline is elapsed, beyond its bound, or was reached
    /// before the preamble completed or while a write to the stream was
    /// still pending.
    AttachDeadline,
}
impl fmt::Display for KubernetesSessionRuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kubernetes(error) => fmt::Display::fmt(error, f),
            Self::ProfileChanged => {
                f.write_str("Kubernetes session Pod differs from the admitted V2 profile")
            }
            Self::InstanceReplaced => {
                f.write_str("Kubernetes session Pod name now designates another instance")
            }
            Self::InstanceRestarted => f.write_str(
                "Kubernetes session container restarted; its identity is no longer exact",
            ),
            Self::WorkspaceClaimInvalid
            | Self::WorkspaceClaimPending
            | Self::WorkspaceClaimAccessMode
            | Self::WorkspaceClaimStorageClass => f.write_str(
                "Kubernetes session workspace claim is not a bound ReadWriteOncePod claim for this attempt",
            ),
            Self::EffectRefusedBeforeSend => f.write_str(
                "Kubernetes session effect was refused before any request was sent",
            ),
            Self::InstanceDeleting => f.write_str(
                "Kubernetes session Pod deletion is requested; its gate is not released",
            ),
            Self::AttachHandshakeRefused => f.write_str(
                "Kubernetes session attach upgrade was refused or was not the exact v5 channel handshake",
            ),
            Self::AttachProtocol => f.write_str(
                "Kubernetes session attach stream violated the bounded v5 channel protocol",
            ),
            Self::AttachIdentityUnproven => f.write_str(
                "Kubernetes session attach ended without a proven exact instance; no identity is established",
            ),
            Self::AttachCapacity => {
                f.write_str("Kubernetes session attach capacity is unavailable")
            }
            Self::AttachDeadline => f.write_str(
                "Kubernetes session attach deadline elapsed or exceeds its bound",
            ),
        }
    }
}
impl std::error::Error for KubernetesSessionRuntimeError {}

impl KubernetesSessionRuntimeError {
    fn is_workspace_prerequisite(&self) -> bool {
        matches!(
            self,
            Self::WorkspaceClaimPending
                | Self::WorkspaceClaimAccessMode
                | Self::WorkspaceClaimStorageClass
        )
    }
}

type Outcome<T> = Result<T, KubernetesSessionRuntimeError>;
const INVALID: KubernetesSessionRuntimeError =
    KubernetesSessionRuntimeError::Kubernetes(KubernetesError::InvalidResponse);

/// Descriptive observation of one operator-provisioned claim. Built only by
/// `parse_workspace_claim` or by revalidating retained journal bytes. It is
/// not an ownership, exclusive-mount, empty-volume or quiescence proof:
/// ReadWriteOncePod is necessary but not sufficient, and the reuse of a claim
/// across attempts stays unproven until the central contract refuses it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct KubernetesSessionWorkspaceClaimV2(ClaimWire);
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimWire {
    version: u32,
    authority_fingerprint: String,
    namespace: String,
    name: String,
    uid: String,
    resource_version: String,
    storage_class_name: String,
    volume_name: String,
    capacity: String,
    access_mode: String,
    observed_at: String,
}
impl ClaimWire {
    fn validate(&self) -> Outcome<()> {
        let capacity = Value::String(self.capacity.clone());
        if self.version != 1
            || !valid_digest(&self.authority_fingerprint)
            || !valid_dns_label(&self.namespace)
            || !valid_dns_label(&self.name)
            || !valid_uid(&self.uid)
            || !valid_resource_version(&self.resource_version)
            || !valid_subdomain(&self.storage_class_name)
            || !valid_subdomain(&self.volume_name)
            || !matches!(observation::quantity(Some(&capacity)), Ok(Some(_)))
            || self.access_mode != READ_WRITE_ONCE_POD
            || !exact_timestamp(&self.observed_at)
        {
            return Err(KubernetesSessionRuntimeError::WorkspaceClaimInvalid);
        }
        Ok(())
    }
}
impl KubernetesSessionWorkspaceClaimV2 {
    pub fn authority_fingerprint(&self) -> &str {
        &self.0.authority_fingerprint
    }
    pub fn namespace(&self) -> &str {
        &self.0.namespace
    }
    pub fn name(&self) -> &str {
        &self.0.name
    }
    pub fn uid(&self) -> &str {
        &self.0.uid
    }
    pub fn resource_version(&self) -> &str {
        &self.0.resource_version
    }
    pub fn storage_class_name(&self) -> &str {
        &self.0.storage_class_name
    }
    pub fn volume_name(&self) -> &str {
        &self.0.volume_name
    }
    /// Observed status capacity, not an enforced byte or inode quota.
    pub fn capacity(&self) -> &str {
        &self.0.capacity
    }
    pub fn access_mode(&self) -> &str {
        &self.0.access_mode
    }
    pub fn observed_at(&self) -> &str {
        &self.0.observed_at
    }
    /// Journal reader only: revalidates retained record fields. A retained
    /// claim stays descriptive and grants nothing.
    pub(crate) fn retained_from_value(value: Value) -> Outcome<Self> {
        let wire: ClaimWire = serde_json::from_value(value)
            .map_err(|_| KubernetesSessionRuntimeError::WorkspaceClaimInvalid)?;
        wire.validate()?;
        Ok(Self(wire))
    }
    /// True only for this exact authority, its namespace and this claim name.
    pub(crate) fn is_for(&self, authority: &KubernetesAuthorityV1, name: &str) -> bool {
        self.0.authority_fingerprint == authority.fingerprint()
            && self.0.namespace == authority.namespace()
            && self.0.name == name
    }
}

/// Closed projection of one PVC GET for `name`. Refuses anything other than a
/// bound, filesystem, ReadWriteOncePod claim of this namespace that is not
/// being deleted and has no prepopulating data source.
pub(crate) fn parse_workspace_claim(
    authority: &KubernetesAuthorityV1,
    name: &str,
    pvc: Value,
    observed_at: &str,
) -> Outcome<KubernetesSessionWorkspaceClaimV2> {
    parse_workspace_claim_detailed(authority, name, pvc, observed_at).map_err(|error| {
        if error.is_workspace_prerequisite() {
            KubernetesSessionRuntimeError::WorkspaceClaimInvalid
        } else {
            error
        }
    })
}

fn parse_workspace_claim_detailed(
    authority: &KubernetesAuthorityV1,
    name: &str,
    pvc: Value,
    observed_at: &str,
) -> Outcome<KubernetesSessionWorkspaceClaimV2> {
    let invalid = KubernetesSessionRuntimeError::WorkspaceClaimInvalid;
    authority
        .validate()
        .map_err(KubernetesSessionRuntimeError::Kubernetes)?;
    let text = |pointer: &str| pvc.pointer(pointer).and_then(Value::as_str);
    let required = |pointer: &str| text(pointer).map(str::to_owned).ok_or(invalid);
    let metadata = pvc
        .get("metadata")
        .and_then(Value::as_object)
        .ok_or(invalid)?;
    let finalizers = match metadata.get("finalizers") {
        None => true,
        Some(Value::Array(values)) => {
            values.len() <= 1
                && values
                    .iter()
                    .all(|value| value.as_str() == Some(PVC_PROTECTION_FINALIZER))
        }
        Some(_) => false,
    };
    if !valid_dns_label(name)
        || text("/apiVersion") != Some("v1")
        || text("/kind") != Some("PersistentVolumeClaim")
        || text("/metadata/namespace") != Some(authority.namespace())
        || text("/metadata/name") != Some(name)
        || metadata
            .get("deletionTimestamp")
            .is_some_and(|value| !value.is_null())
        || !finalizers
        || pvc
            .pointer("/spec/volumeMode")
            .is_some_and(|mode| mode.as_str() != Some("Filesystem"))
        || pvc
            .pointer("/spec/dataSource")
            .is_some_and(|source| !source.is_null())
        || pvc
            .pointer("/spec/dataSourceRef")
            .is_some_and(|source| !source.is_null())
    {
        return Err(invalid);
    }
    let uid = required("/metadata/uid")?;
    let resource_version = required("/metadata/resourceVersion")?;
    if !valid_uid(&uid) || !valid_resource_version(&resource_version) {
        return Err(invalid);
    }
    let pending = match text("/status/phase") {
        Some("Bound") => false,
        Some("Pending") => true,
        _ => return Err(invalid),
    };
    let requested_rwop =
        read_write_once_pod_only(pvc.pointer("/spec/accessModes").ok_or(invalid)?)?;
    let observed_rwop = pvc
        .pointer("/status/accessModes")
        .map(read_write_once_pod_only)
        .transpose()?;
    let volume_name = pvc
        .pointer("/spec/volumeName")
        .map(|value| {
            value
                .as_str()
                .filter(|name| valid_subdomain(name))
                .ok_or(invalid)
        })
        .transpose()?;
    let capacity = match pvc.pointer("/status/capacity") {
        None if pending => None,
        Some(Value::Object(capacity)) => capacity.get("storage"),
        _ => return Err(invalid),
    };
    let capacity = capacity
        .map(|value| {
            observation::quantity(Some(value))
                .ok()
                .flatten()
                .ok_or(invalid)
        })
        .transpose()?;
    if !pending && (observed_rwop.is_none() || volume_name.is_none() || capacity.is_none()) {
        return Err(invalid);
    }
    let storage_class_name = match pvc.pointer("/spec/storageClassName") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) if name.is_empty() => None,
        Some(Value::String(name)) if valid_subdomain(name) => Some(name.clone()),
        _ => return Err(invalid),
    };
    if !requested_rwop || observed_rwop == Some(false) {
        return Err(KubernetesSessionRuntimeError::WorkspaceClaimAccessMode);
    }
    let storage_class_name =
        storage_class_name.ok_or(KubernetesSessionRuntimeError::WorkspaceClaimStorageClass)?;
    if pending {
        return Err(KubernetesSessionRuntimeError::WorkspaceClaimPending);
    }
    let wire = ClaimWire {
        version: 1,
        authority_fingerprint: authority.fingerprint(),
        namespace: authority.namespace().into(),
        name: name.into(),
        uid,
        resource_version,
        storage_class_name,
        volume_name: volume_name.ok_or(invalid)?.into(),
        capacity: capacity.ok_or(invalid)?,
        access_mode: READ_WRITE_ONCE_POD.into(),
        observed_at: observed_at.into(),
    };
    wire.validate()?;
    Ok(KubernetesSessionWorkspaceClaimV2(wire))
}

fn read_write_once_pod_only(value: &Value) -> Outcome<bool> {
    let invalid = KubernetesSessionRuntimeError::WorkspaceClaimInvalid;
    let modes = value.as_array().ok_or(invalid)?;
    if modes.is_empty()
        || !modes.iter().all(|mode| {
            matches!(
                mode.as_str(),
                Some("ReadWriteOnce" | "ReadOnlyMany" | "ReadWriteMany" | READ_WRITE_ONCE_POD)
            )
        })
    {
        return Err(invalid);
    }
    Ok(matches!(modes.as_slice(), [mode] if mode.as_str() == Some(READ_WRITE_ONCE_POD)))
}

/// Exact retained instance. Created only by `validate_admitted_session_pod`
/// on a 201 for the exact retained request (or, once the central contract
/// exists, from an acknowledged instance matching the journal). Never from
/// disk alone. Not Clone and not serializable.
#[derive(Debug)]
pub(crate) struct SessionInstanceV2 {
    target: KubernetesSessionPodTargetV2,
    pod_uid: String,
    created_resource_version: String,
    admitted_configuration_sha256: String,
    released_configuration_sha256: String,
}
impl SessionInstanceV2 {
    pub(crate) fn target(&self) -> &KubernetesSessionPodTargetV2 {
        &self.target
    }
    pub(crate) fn pod_uid(&self) -> &str {
        &self.pod_uid
    }
    pub(crate) fn created_resource_version(&self) -> &str {
        &self.created_resource_version
    }
}

/// CREATE 201 projection for the exact retained request. It mirrors the V1
/// held response checks and reuses the shared admission normalizer, which
/// restores only the five omitted non-pointer bools. Any refusal must be kept
/// as unknown ownership by an effect caller, never read as absence.
///
/// The native constructor consumes the affine response from the exact
/// transport CREATE201. A GET, name, raw body or journal record cannot satisfy
/// this input. The response still passes the complete admission projection.
pub(super) fn validate_admitted_session_pod(
    request: &KubernetesSessionPodRequestV2,
    response: CreatedSessionResponseV2,
) -> Outcome<SessionInstanceV2> {
    let bytes = response.into_bytes_for(request)?;
    project_admitted_session_pod(request, &bytes)
}

/// Declared-fixture projection only, unavailable to a production caller.
#[cfg(test)]
pub(super) fn validate_declared_admitted_session_pod(
    request: &KubernetesSessionPodRequestV2,
    response: &[u8],
) -> Outcome<SessionInstanceV2> {
    project_admitted_session_pod(request, response)
}

fn project_admitted_session_pod(
    request: &KubernetesSessionPodRequestV2,
    response: &[u8],
) -> Outcome<SessionInstanceV2> {
    if response.len() > MAX_RESPONSE_BYTES {
        return Err(KubernetesSessionRuntimeError::Kubernetes(
            KubernetesError::ResponseTooLarge,
        ));
    }
    let admitted = crate::json::decode_unique(response, 64).map_err(|_| INVALID)?;
    let requested: Value = serde_json::from_slice(request.bytes()).map_err(|_| INVALID)?;
    let target = request.target();
    let fields = admitted.as_object().ok_or(INVALID)?;
    let metadata = fields
        .get("metadata")
        .and_then(Value::as_object)
        .ok_or(INVALID)?;
    if unexpected(
        fields,
        &["apiVersion", "kind", "metadata", "spec", "status"],
    ) || unexpected(
        metadata,
        &[
            "name",
            "namespace",
            "uid",
            "resourceVersion",
            "generation",
            "labels",
            "annotations",
            "ownerReferences",
            "finalizers",
            "deletionTimestamp",
            "creationTimestamp",
            "managedFields",
        ],
    ) || metadata.get("name").and_then(Value::as_str) != Some(target.pod_name())
        || metadata
            .get("generation")
            .is_some_and(|generation| generation.as_u64() != Some(1))
        || metadata
            .get("creationTimestamp")
            .is_some_and(|value| !optional_time(value))
        || metadata.get("managedFields").is_some_and(|value| {
            value
                .as_array()
                .is_none_or(|values| values.iter().any(|value| !value.is_object()))
        })
        || fields
            .get("status")
            .is_some_and(|status| !held_status(status))
    {
        return Err(INVALID);
    }
    let pod_uid = metadata
        .get("uid")
        .and_then(Value::as_str)
        .filter(|uid| valid_uid(uid))
        .ok_or(INVALID)?;
    let created_resource_version = metadata
        .get("resourceVersion")
        .and_then(Value::as_str)
        .filter(|version| valid_resource_version(version))
        .ok_or(INVALID)?;
    preflight::validate_admitted(&requested, &admitted).map_err(|error| match error {
        KubernetesError::PreflightProfileChanged => KubernetesSessionRuntimeError::ProfileChanged,
        other => KubernetesSessionRuntimeError::Kubernetes(other),
    })?;
    let spec = fields.get("spec").ok_or(INVALID)?;
    let mut released = spec.as_object().cloned().ok_or(INVALID)?;
    released.remove("schedulingGates");
    Ok(SessionInstanceV2 {
        target: target.clone(),
        pod_uid: pod_uid.into(),
        created_resource_version: created_resource_version.into(),
        admitted_configuration_sha256: KubernetesPodHandle::configuration_digest(spec)
            .map_err(KubernetesSessionRuntimeError::Kubernetes)?,
        released_configuration_sha256: KubernetesPodHandle::configuration_digest(&Value::Object(
            released,
        ))
        .map_err(KubernetesSessionRuntimeError::Kubernetes)?,
    })
}

/// Container facts of the single `worker` container, from the API only.
/// Declared observations: never a terminal settlement by themselves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionContainerStateV2 {
    Absent,
    Waiting,
    Running {
        container_id: String,
        image_id: Option<String>,
        started_at: DateTime<Utc>,
    },
    Terminated {
        container_id: String,
        exit_code: Option<i32>,
        oom_killed: Option<bool>,
        started_at: Option<DateTime<Utc>>,
        finished_at: Option<DateTime<Utc>>,
    },
}
/// Observed lifecycle position of the exact instance. A deletion request is
/// not terminal; a missing Pod is an API answer handled by the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionObservationStateV2 {
    /// Gate present, no node binding and no container status.
    Held,
    /// Gate removed, not yet bound to a node.
    ReleasedUnscheduled,
    Scheduled {
        node_name: String,
        container: SessionContainerStateV2,
    },
    Deleting {
        container: SessionContainerStateV2,
    },
}
/// Built only by `parse_session_observation` for one exact instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionObservedV2 {
    pod_uid: String,
    resource_version: String,
    state: SessionObservationStateV2,
    /// The scheduling gate was still present in this observation.
    gated: bool,
}
impl SessionObservedV2 {
    /// True when this exact observation shows the gate still present: the
    /// Pod was never released, so it was never scheduled or started.
    pub(crate) fn gated(&self) -> bool {
        self.gated
    }
    pub(crate) fn pod_uid(&self) -> &str {
        &self.pod_uid
    }
    pub(crate) fn resource_version(&self) -> &str {
        &self.resource_version
    }
    pub(crate) fn state(&self) -> &SessionObservationStateV2 {
        &self.state
    }
}

/// Exact observation of one Pod GET against a retained instance. Another UID
/// is another instance; a spec other than the admitted (gated) or released
/// (gate and node binding removed) form is a profile change; any restart
/// voids the container identity.
pub(crate) fn parse_session_observation(
    authority: &KubernetesAuthorityV1,
    instance: &SessionInstanceV2,
    pod: Value,
) -> Outcome<SessionObservedV2> {
    authority
        .validate()
        .map_err(KubernetesSessionRuntimeError::Kubernetes)?;
    let target = &instance.target;
    if authority.fingerprint() != target.authority_fingerprint() {
        return Err(KubernetesSessionRuntimeError::Kubernetes(
            KubernetesError::InvalidAuthority,
        ));
    }
    if pod.get("apiVersion").and_then(Value::as_str) != Some("v1")
        || pod.get("kind").and_then(Value::as_str) != Some("Pod")
        || pod.pointer("/metadata/namespace").and_then(Value::as_str) != Some(authority.namespace())
        || pod.pointer("/metadata/name").and_then(Value::as_str) != Some(target.pod_name())
    {
        return Err(INVALID);
    }
    if pod.pointer("/metadata/uid").and_then(Value::as_str) != Some(instance.pod_uid.as_str()) {
        return Err(KubernetesSessionRuntimeError::InstanceReplaced);
    }
    let resource_version = pod
        .pointer("/metadata/resourceVersion")
        .and_then(Value::as_str)
        .filter(|version| valid_resource_version(version))
        .ok_or(INVALID)?
        .to_owned();
    let spec = pod.get("spec").ok_or(INVALID)?;
    let mut released = spec.as_object().cloned().ok_or(INVALID)?;
    let node_name = observation::optional_string(released.remove("nodeName").as_ref(), 253)
        .map_err(KubernetesSessionRuntimeError::Kubernetes)?;
    if node_name
        .as_ref()
        .is_some_and(|name| !valid_subdomain(name))
    {
        return Err(INVALID);
    }
    let held = released
        .remove("schedulingGates")
        .is_some_and(|gates| gates.as_array().is_none_or(|gates| !gates.is_empty()));
    let profile_matches = if held {
        node_name.is_none()
            && KubernetesPodHandle::configuration_digest(spec)
                .map_err(KubernetesSessionRuntimeError::Kubernetes)?
                == instance.admitted_configuration_sha256
    } else {
        KubernetesPodHandle::configuration_digest(&Value::Object(released))
            .map_err(KubernetesSessionRuntimeError::Kubernetes)?
            == instance.released_configuration_sha256
    };
    if !profile_matches {
        return Err(KubernetesSessionRuntimeError::ProfileChanged);
    }
    let container = container_state(&pod)?;
    let deleting = observation::timestamp(pod.pointer("/metadata/deletionTimestamp"))
        .map_err(KubernetesSessionRuntimeError::Kubernetes)?
        .is_some();
    let state = if deleting {
        SessionObservationStateV2::Deleting { container }
    } else if held {
        if !matches!(container, SessionContainerStateV2::Absent) {
            return Err(INVALID);
        }
        SessionObservationStateV2::Held
    } else if let Some(node_name) = node_name {
        SessionObservationStateV2::Scheduled {
            node_name,
            container,
        }
    } else {
        if !matches!(container, SessionContainerStateV2::Absent) {
            return Err(INVALID);
        }
        SessionObservationStateV2::ReleasedUnscheduled
    };
    Ok(SessionObservedV2 {
        pod_uid: instance.pod_uid.clone(),
        resource_version,
        state,
        gated: held,
    })
}

fn container_state(pod: &Value) -> Outcome<SessionContainerStateV2> {
    let statuses = match pod.pointer("/status/containerStatuses") {
        None | Some(Value::Null) => return Ok(SessionContainerStateV2::Absent),
        Some(value) => value.as_array().ok_or(INVALID)?,
    };
    let status = match statuses.as_slice() {
        [] => return Ok(SessionContainerStateV2::Absent),
        [status] if status.get("name").and_then(Value::as_str) == Some("worker") => status,
        _ => return Err(KubernetesSessionRuntimeError::ProfileChanged),
    };
    match status.get("restartCount").and_then(Value::as_u64) {
        Some(0) => {}
        Some(_) => return Err(KubernetesSessionRuntimeError::InstanceRestarted),
        None => return Err(INVALID),
    }
    let kubernetes = KubernetesSessionRuntimeError::Kubernetes;
    let container_id =
        observation::optional_identity(status.get("containerID")).map_err(kubernetes)?;
    let image_id = observation::optional_identity(status.get("imageID")).map_err(kubernetes)?;
    let state = status
        .get("state")
        .and_then(Value::as_object)
        .ok_or(INVALID)?;
    if state.len() != 1 {
        return Err(INVALID);
    }
    let (kind, facts) = state.iter().next().ok_or(INVALID)?;
    if !facts.is_object() {
        return Err(INVALID);
    }
    Ok(match kind.as_str() {
        "waiting" => SessionContainerStateV2::Waiting,
        "running" => SessionContainerStateV2::Running {
            container_id: container_id.ok_or(INVALID)?,
            image_id,
            started_at: observation::timestamp(facts.get("startedAt"))
                .map_err(kubernetes)?
                .ok_or(INVALID)?,
        },
        "terminated" => {
            let started_at = observation::timestamp(facts.get("startedAt")).map_err(kubernetes)?;
            let finished_at =
                observation::timestamp(facts.get("finishedAt")).map_err(kubernetes)?;
            if started_at
                .zip(finished_at)
                .is_some_and(|(start, finish)| finish < start)
            {
                return Err(INVALID);
            }
            SessionContainerStateV2::Terminated {
                container_id: container_id.ok_or(INVALID)?,
                exit_code: facts
                    .get("exitCode")
                    .map(|value| {
                        value
                            .as_i64()
                            .and_then(|value| i32::try_from(value).ok())
                            .filter(|value| *value >= 0)
                            .ok_or(INVALID)
                    })
                    .transpose()?,
                oom_killed: observation::optional_string(facts.get("reason"), 128)
                    .map_err(kubernetes)?
                    .map(|reason| reason == "OOMKilled"),
                started_at,
                finished_at,
            }
        }
        _ => return Err(INVALID),
    })
}

/// Presence of one object at a session name. Presence never designates
/// ownership; a 404 is an observed answer, never a terminal or a settlement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "presence", rename_all = "snake_case")]
pub(crate) enum KubernetesSessionObjectPresenceV2 {
    AbsentObserved404,
    Present {
        uid: String,
        resource_version: String,
        deletion_requested: bool,
    },
}
/// Descriptive inspection of the names of one rendered target. There is
/// deliberately no ownership field.
#[derive(Debug, Serialize)]
pub(crate) struct KubernetesSessionNameInspectionV2 {
    version: u32,
    authority_fingerprint: String,
    namespace: String,
    pod_name: String,
    pod: KubernetesSessionObjectPresenceV2,
    workspace_claim_name: String,
    workspace_claim: KubernetesSessionObjectPresenceV2,
    workspace_claim_uid_matches_retained: Option<bool>,
    observed_at: String,
}
impl KubernetesSessionNameInspectionV2 {
    pub(crate) fn pod(&self) -> &KubernetesSessionObjectPresenceV2 {
        &self.pod
    }
    pub(crate) fn workspace_claim(&self) -> &KubernetesSessionObjectPresenceV2 {
        &self.workspace_claim
    }
    pub(crate) fn workspace_claim_uid_matches_retained(&self) -> Option<bool> {
        self.workspace_claim_uid_matches_retained
    }
}

impl KubernetesRuntime {
    /// Read-only inspection of one operator-provisioned session workspace
    /// claim. `name` must be a DNS label, checked before any request. The
    /// authority anchors are read before and after the claim; no mutation,
    /// adoption or ownership follows from the result.
    pub async fn inspect_session_workspace_claim(
        &self,
        name: &str,
    ) -> KubernetesResult<KubernetesSessionWorkspaceClaimV2> {
        self.inspect_workspace_claim(name, false)
            .await
            .map_err(|error| match error {
                KubernetesSessionRuntimeError::Kubernetes(error) => error,
                _ => KubernetesError::InvalidResponse,
            })
    }

    /// Same inspection with closed prerequisite diagnostics for the operator CLI.
    pub(crate) async fn inspect_session_workspace_claim_detailed(
        &self,
        name: &str,
    ) -> Outcome<KubernetesSessionWorkspaceClaimV2> {
        self.inspect_workspace_claim(name, true).await
    }

    async fn inspect_workspace_claim(
        &self,
        name: &str,
        diagnose_prerequisites: bool,
    ) -> Outcome<KubernetesSessionWorkspaceClaimV2> {
        use KubernetesSessionRuntimeError::Kubernetes;
        if !valid_dns_label(name) {
            return Err(Kubernetes(KubernetesError::InvalidAuthority));
        }
        self.verify_active_authority().await.map_err(Kubernetes)?;
        let namespace = self.binding.namespace();
        let pvc: Value = self
            .transport
            .get_unique(&format!(
                "/api/v1/namespaces/{namespace}/persistentvolumeclaims/{name}"
            ))
            .await
            .map_err(Kubernetes)?;
        let observed_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let claim = if diagnose_prerequisites {
            parse_workspace_claim_detailed(&self.binding, name, pvc, &observed_at)
        } else {
            parse_workspace_claim(&self.binding, name, pvc, &observed_at)
        };
        if claim.is_ok()
            || claim
                .as_ref()
                .is_err_and(KubernetesSessionRuntimeError::is_workspace_prerequisite)
        {
            self.verify_active_authority().await.map_err(Kubernetes)?;
        }
        claim
    }

    /// Names come only from a rendered target and are revalidated before any
    /// request. GET anchors, Pod, claim, anchors. No mutation or adoption.
    pub(crate) async fn inspect_session_names(
        &self,
        target: &KubernetesSessionPodTargetV2,
    ) -> KubernetesResult<KubernetesSessionNameInspectionV2> {
        let pod_name = target.pod_name();
        let claim_name = target.workspace_claim_name();
        if target.authority_fingerprint() != self.binding.fingerprint()
            || !valid_dns_label(pod_name)
            || !valid_dns_label(claim_name)
        {
            return Err(KubernetesError::InvalidAuthority);
        }
        self.verify_active_authority().await?;
        let namespace = self.binding.namespace();
        let pod = self
            .session_object_presence(
                &format!("/api/v1/namespaces/{namespace}/pods/{pod_name}"),
                pod_name,
            )
            .await?;
        let workspace_claim = self
            .session_object_presence(
                &format!("/api/v1/namespaces/{namespace}/persistentvolumeclaims/{claim_name}"),
                claim_name,
            )
            .await?;
        self.verify_active_authority().await?;
        let workspace_claim_uid_matches_retained = match &workspace_claim {
            KubernetesSessionObjectPresenceV2::Present { uid, .. } => {
                Some(uid.as_str() == target.workspace_claim_uid())
            }
            KubernetesSessionObjectPresenceV2::AbsentObserved404 => None,
        };
        Ok(KubernetesSessionNameInspectionV2 {
            version: 1,
            authority_fingerprint: self.binding.fingerprint(),
            namespace: namespace.into(),
            pod_name: pod_name.into(),
            pod,
            workspace_claim_name: claim_name.into(),
            workspace_claim,
            workspace_claim_uid_matches_retained,
            observed_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        })
    }

    async fn session_object_presence(
        &self,
        path: &str,
        name: &str,
    ) -> KubernetesResult<KubernetesSessionObjectPresenceV2> {
        let object: Value = match self.transport.get_unique(path).await {
            Ok(object) => object,
            Err(KubernetesError::ApiRejected(404)) => {
                return Ok(KubernetesSessionObjectPresenceV2::AbsentObserved404);
            }
            Err(error) => return Err(error),
        };
        let metadata = object
            .get("metadata")
            .and_then(Value::as_object)
            .ok_or(KubernetesError::InvalidResponse)?;
        if metadata.get("name").and_then(Value::as_str) != Some(name)
            || metadata.get("namespace").and_then(Value::as_str) != Some(self.binding.namespace())
        {
            return Err(KubernetesError::InvalidResponse);
        }
        let uid = metadata
            .get("uid")
            .and_then(Value::as_str)
            .filter(|uid| valid_uid(uid))
            .ok_or(KubernetesError::InvalidResponse)?;
        let resource_version = metadata
            .get("resourceVersion")
            .and_then(Value::as_str)
            .filter(|version| valid_resource_version(version))
            .ok_or(KubernetesError::InvalidResponse)?;
        let deletion_requested =
            observation::timestamp(metadata.get("deletionTimestamp"))?.is_some();
        Ok(KubernetesSessionObjectPresenceV2::Present {
            uid: uid.into(),
            resource_version: resource_version.into(),
            deletion_requested,
        })
    }
}

/// Pending, gated, without placement or container facts. Bounded condition
/// details are tolerated but never retained (declared fact, to confirm).
fn held_status(status: &Value) -> bool {
    let Some(fields) = status.as_object() else {
        return false;
    };
    let empty = |name: &str| {
        fields
            .get(name)
            .is_none_or(|value| value.as_array().is_some_and(Vec::is_empty))
    };
    !unexpected(
        fields,
        &[
            "phase",
            "conditions",
            "qosClass",
            "containerStatuses",
            "initContainerStatuses",
            "ephemeralContainerStatuses",
        ],
    ) && fields
        .get("phase")
        .is_none_or(|phase| phase.as_str() == Some("Pending"))
        && fields.get("qosClass").is_none_or(|class| {
            matches!(
                class.as_str(),
                Some("Guaranteed" | "Burstable" | "BestEffort")
            )
        })
        && empty("containerStatuses")
        && empty("initContainerStatuses")
        && empty("ephemeralContainerStatuses")
        && fields.get("conditions").is_none_or(|conditions| {
            match conditions.as_array().map(Vec::as_slice) {
                Some([]) => true,
                Some([condition]) => gated_condition(condition),
                _ => false,
            }
        })
}
fn gated_condition(condition: &Value) -> bool {
    let Some(fields) = condition.as_object() else {
        return false;
    };
    let time = |name: &str| fields.get(name).is_none_or(optional_time);
    !unexpected(
        fields,
        &[
            "type",
            "status",
            "reason",
            "message",
            "lastProbeTime",
            "lastTransitionTime",
        ],
    ) && fields.get("type").and_then(Value::as_str) == Some("PodScheduled")
        && fields.get("status").and_then(Value::as_str) == Some("False")
        && fields.get("reason").and_then(Value::as_str) == Some("SchedulingGated")
        && fields
            .get("message")
            .is_none_or(|message| message.as_str().is_some_and(|text| text.len() <= 1024))
        && time("lastProbeTime")
        && time("lastTransitionTime")
}
fn optional_time(value: &Value) -> bool {
    value.is_null()
        || value.as_str().is_some_and(|text| {
            text.len() <= 64
                && DateTime::parse_from_rfc3339(text).is_ok_and(|time| time.timestamp() > 0)
        })
}
fn unexpected(fields: &Map<String, Value>, allowed: &[&str]) -> bool {
    fields.keys().any(|key| !allowed.contains(&key.as_str()))
}
fn valid_resource_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
}
fn valid_subdomain(value: &str) -> bool {
    !value.is_empty() && value.len() <= 253 && value.split('.').all(valid_dns_label)
}
fn exact_timestamp(value: &str) -> bool {
    DateTime::parse_from_rfc3339(value).is_ok_and(|time| {
        time.timestamp() > 0
            && time
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true)
                == value
    })
}

#[cfg(test)]
#[path = "session_instance_tests.rs"]
pub(crate) mod tests;
