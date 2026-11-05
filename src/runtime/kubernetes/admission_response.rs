//! Projection of a CREATE response into closed declared facts. No raw server
//! message, address, annotations or managedFields are retained in the outcome.
use super::{json::canonical_bytes, prepare, profile};
use crate::{
    control::kubernetes_admission::{
        KubernetesCreateIntentV1, KubernetesCreateMetadataV1, KubernetesCreateObservationV1,
        KubernetesCreateOutcomeV1, KubernetesCreateSchedulingConditionV1, KubernetesCreateStatusV1,
    },
    runtime::kubernetes::{
        KubernetesError, KubernetesPodHandle, KubernetesResult, preflight, valid_uid,
    },
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub(super) fn parse(
    intent: &KubernetesCreateIntentV1,
    bytes: &[u8],
    observed_at: &str,
) -> KubernetesResult<KubernetesCreateOutcomeV1> {
    let observed = chrono::DateTime::parse_from_rfc3339(observed_at)
        .map_err(|_| KubernetesError::InvalidResponse)?;
    if observed.timestamp() <= 0
        || observed
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            != observed_at
    {
        return Err(KubernetesError::InvalidResponse);
    }
    let pod = super::json::decode(bytes)?;
    let fields = object(&pod)?;
    if unexpected(
        fields,
        &["apiVersion", "kind", "metadata", "spec", "status"],
    ) {
        return Err(KubernetesError::InvalidResponse);
    }
    let metadata = metadata(
        fields
            .get("metadata")
            .ok_or(KubernetesError::InvalidResponse)?,
    )?;
    let status = fields.get("status").map(status).transpose()?;
    let admitted_spec = fields.get("spec").ok_or(KubernetesError::InvalidResponse)?;
    canonical_bytes(admitted_spec, 32 * 1024)?;
    let observation = KubernetesCreateObservationV1 {
        version: 1,
        api_version: string(fields, "apiVersion")?,
        kind: string(fields, "kind")?,
        metadata,
        status,
    };
    let digest = KubernetesPodHandle::configuration_digest(admitted_spec)?;
    validate(intent, &observation, admitted_spec, &digest)?;
    Ok(KubernetesCreateOutcomeV1::CreatedHeld {
        observation,
        admitted_spec: admitted_spec.clone(),
        admitted_configuration_sha256: digest,
        observed_at: observed_at.into(),
    })
}

pub(super) fn validate(
    intent: &KubernetesCreateIntentV1,
    observation: &KubernetesCreateObservationV1,
    admitted_spec: &Value,
    digest: &str,
) -> KubernetesResult<()> {
    let prepared = prepare(intent)?;
    let requested = profile::pod(intent)?;
    canonical_bytes(admitted_spec, 32 * 1024)?;
    canonical_bytes(
        &serde_json::to_value(observation).map_err(|_| KubernetesError::InvalidResponse)?,
        4 * 1024,
    )?;
    let metadata = &observation.metadata;
    if observation.version != 1
        || observation.api_version != "v1"
        || observation.kind != "Pod"
        || metadata.name != prepared.target().pod_name
        || metadata.namespace != intent.authority.namespace()
        || !valid_uid(&metadata.uid)
        || !resource_version(&metadata.resource_version)
        || metadata.annotation_count != 0
        || metadata.owner_reference_count != 0
        || metadata.finalizer_count != 0
        || metadata.deletion_requested
        || metadata.unexpected_fields_present
        || serde_json::to_value(&metadata.labels).map_err(|_| KubernetesError::InvalidResponse)?
            != requested["metadata"]["labels"]
        || KubernetesPodHandle::configuration_digest(admitted_spec)? != digest
    {
        return Err(KubernetesError::InvalidResponse);
    }
    if let Some(status) = &observation.status {
        if status
            .phase
            .as_deref()
            .is_some_and(|phase| phase != "Pending")
            || [
                status.container_status_count,
                status.init_container_status_count,
                status.ephemeral_container_status_count,
            ]
            .into_iter()
            .flatten()
            .any(|count| count != 0)
            || status.start_time_present
            || status.placement_fields_present
            || status.unexpected_fields_present
            || status.pod_scheduled.as_ref().is_some_and(|condition| {
                condition.status != "False" || condition.reason != "SchedulingGated"
            })
        {
            return Err(KubernetesError::InvalidResponse);
        }
    }
    // Reuse the historical normalizer unchanged, against the new request that
    // includes its mandatory gate. Persist/hash the real admitted spec, not
    // this normalized comparison value.
    let admitted = json!({"apiVersion":observation.api_version,"kind":observation.kind,
        "metadata":{"name":metadata.name,"namespace":metadata.namespace,"labels":metadata.labels},
        "spec":admitted_spec});
    preflight::validate_admitted(&requested, &admitted)
}

fn metadata(value: &Value) -> KubernetesResult<KubernetesCreateMetadataV1> {
    let fields = object(value)?;
    if let Some(value) = fields.get("creationTimestamp") {
        match value {
            Value::Null => {}
            Value::String(value) if value.len() <= 64 => {
                if chrono::DateTime::parse_from_rfc3339(value)
                    .ok()
                    .is_none_or(|date| date.timestamp() <= 0)
                {
                    return Err(KubernetesError::InvalidResponse);
                }
            }
            _ => return Err(KubernetesError::InvalidResponse),
        }
    }
    if let Some(value) = fields.get("managedFields")
        && value
            .as_array()
            .is_none_or(|values| values.iter().any(|value| !value.is_object()))
    {
        return Err(KubernetesError::InvalidResponse);
    }
    let labels: BTreeMap<String, String> = serde_json::from_value(
        fields
            .get("labels")
            .ok_or(KubernetesError::InvalidResponse)?
            .clone(),
    )
    .map_err(|_| KubernetesError::InvalidResponse)?;
    Ok(KubernetesCreateMetadataV1 {
        name: string(fields, "name")?,
        namespace: string(fields, "namespace")?,
        uid: string(fields, "uid")?,
        resource_version: string(fields, "resourceVersion")?,
        labels,
        annotation_count: match fields.get("annotations") {
            None => 0,
            Some(value) => count(object(value)?.len())?,
        },
        owner_reference_count: array_count(fields, "ownerReferences")?.unwrap_or(0),
        finalizer_count: array_count(fields, "finalizers")?.unwrap_or(0),
        deletion_requested: fields
            .get("deletionTimestamp")
            .is_some_and(|value| !value.is_null()),
        unexpected_fields_present: unexpected(
            fields,
            &[
                "name",
                "namespace",
                "uid",
                "resourceVersion",
                "labels",
                "annotations",
                "ownerReferences",
                "finalizers",
                "deletionTimestamp",
                "creationTimestamp",
                "managedFields",
            ],
        ),
    })
}

fn status(value: &Value) -> KubernetesResult<KubernetesCreateStatusV1> {
    let fields = object(value)?;
    let phase = fields
        .get("phase")
        .map(|value| {
            value
                .as_str()
                .filter(|phase| {
                    matches!(
                        *phase,
                        "Pending" | "Running" | "Succeeded" | "Failed" | "Unknown"
                    )
                })
                .map(str::to_owned)
                .ok_or(KubernetesError::InvalidResponse)
        })
        .transpose()?;
    if let Some(value) = fields.get("qosClass")
        && !value
            .as_str()
            .is_some_and(|value| matches!(value, "Guaranteed" | "Burstable" | "BestEffort"))
    {
        return Err(KubernetesError::InvalidResponse);
    }
    let mut placement = false;
    for field in ["hostIP", "podIP", "nominatedNodeName"] {
        if let Some(value) = fields.get(field) {
            placement |= !value
                .as_str()
                .ok_or(KubernetesError::InvalidResponse)?
                .is_empty();
        }
    }
    for field in ["hostIPs", "podIPs"] {
        if let Some(value) = fields.get(field) {
            placement |= !value
                .as_array()
                .ok_or(KubernetesError::InvalidResponse)?
                .is_empty();
        }
    }
    let pod_scheduled = if let Some(value) = fields.get("conditions") {
        let conditions = value.as_array().ok_or(KubernetesError::InvalidResponse)?;
        match conditions.as_slice() {
            [] => None,
            [condition] => {
                let condition = object(condition)?;
                if unexpected(condition, &["type", "status", "reason"])
                    || string(condition, "type")? != "PodScheduled"
                {
                    return Err(KubernetesError::InvalidResponse);
                }
                Some(KubernetesCreateSchedulingConditionV1 {
                    status: string(condition, "status")?,
                    reason: string(condition, "reason")?,
                })
            }
            _ => return Err(KubernetesError::InvalidResponse),
        }
    } else {
        None
    };
    Ok(KubernetesCreateStatusV1 {
        phase,
        container_status_count: array_count(fields, "containerStatuses")?,
        init_container_status_count: array_count(fields, "initContainerStatuses")?,
        ephemeral_container_status_count: array_count(fields, "ephemeralContainerStatuses")?,
        start_time_present: fields.contains_key("startTime"),
        placement_fields_present: placement,
        unexpected_fields_present: unexpected(
            fields,
            &[
                "phase",
                "containerStatuses",
                "initContainerStatuses",
                "ephemeralContainerStatuses",
                "startTime",
                "hostIP",
                "hostIPs",
                "podIP",
                "podIPs",
                "nominatedNodeName",
                "conditions",
                "qosClass",
            ],
        ),
        pod_scheduled,
    })
}

fn object(value: &Value) -> KubernetesResult<&Map<String, Value>> {
    value.as_object().ok_or(KubernetesError::InvalidResponse)
}
fn string(fields: &Map<String, Value>, name: &str) -> KubernetesResult<String> {
    fields
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(KubernetesError::InvalidResponse)
}
fn unexpected(fields: &Map<String, Value>, allowed: &[&str]) -> bool {
    fields.keys().any(|key| !allowed.contains(&key.as_str()))
}
fn count(value: usize) -> KubernetesResult<u32> {
    u32::try_from(value).map_err(|_| KubernetesError::InvalidResponse)
}
fn array_count(fields: &Map<String, Value>, name: &str) -> KubernetesResult<Option<u32>> {
    fields
        .get(name)
        .map(|value| {
            count(
                value
                    .as_array()
                    .ok_or(KubernetesError::InvalidResponse)?
                    .len(),
            )
        })
        .transpose()
}
fn resource_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
}
