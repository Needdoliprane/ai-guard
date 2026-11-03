use super::{
    KubernetesAuthorityV1, KubernetesError, KubernetesResult, valid_digest, valid_dns_label,
    valid_uid,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// A retained control checkpoint, not ownership inferred from Pod labels.
/// Only trusted control may select these inputs. Construction grants neither
/// workload admission nor mutation rights; this slice only inspects the Pod.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KubernetesPodHandle {
    version: u32,
    authority_fingerprint: String,
    session: Uuid,
    operation: Uuid,
    name: String,
    uid: String,
    configuration_sha256: String,
}
impl KubernetesPodHandle {
    pub fn from_control_checkpoint(
        authority: &KubernetesAuthorityV1,
        session: Uuid,
        operation: Uuid,
        name: &str,
        uid: &str,
        configuration_sha256: &str,
    ) -> KubernetesResult<Self> {
        let handle = Self {
            version: 1,
            authority_fingerprint: authority.fingerprint(),
            session,
            operation,
            name: name.into(),
            uid: uid.into(),
            configuration_sha256: configuration_sha256.into(),
        };
        handle.validate(authority)?;
        Ok(handle)
    }
    pub(super) fn validate(&self, authority: &KubernetesAuthorityV1) -> KubernetesResult<()> {
        authority.validate()?;
        if self.version != 1
            || self.authority_fingerprint != authority.fingerprint()
            || self.session.is_nil()
            || self.operation.is_nil()
            || !valid_pod_name(&self.name)
            || !valid_uid(&self.uid)
            || !valid_digest(&self.configuration_sha256)
        {
            return Err(KubernetesError::PodChanged);
        }
        Ok(())
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn uid(&self) -> &str {
        &self.uid
    }
    pub fn session_id(&self) -> Uuid {
        self.session
    }
    pub fn operation_id(&self) -> Uuid {
        self.operation
    }
    /// Exact post-admission Pod spec commitment. Kubernetes defaults and
    /// mutations are included; this digest is not an enforcement attestation.
    /// Object keys are sorted recursively, arrays retain order, max depth 64.
    pub fn configuration_digest(spec: &Value) -> KubernetesResult<String> {
        if !spec.is_object() {
            return Err(KubernetesError::InvalidResponse);
        }
        let mut canonical = Vec::new();
        canonical_json(spec, &mut canonical, 0)?;
        let mut hash = Sha256::new();
        hash.update(b"ai-guard:kubernetes-pod-spec:v1\0");
        hash.update(&canonical);
        Ok(hex::encode(hash.finalize()))
    }
}
fn valid_pod_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 253 && value.split('.').all(valid_dns_label)
}
fn canonical_json(value: &Value, output: &mut Vec<u8>, depth: usize) -> KubernetesResult<()> {
    if depth > 64 || output.len() > 1024 * 1024 {
        return Err(KubernetesError::InvalidResponse);
    }
    match value {
        Value::Object(fields) => {
            output.push(b'{');
            let sorted: std::collections::BTreeMap<_, _> = fields.iter().collect();
            for (index, (key, value)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key)
                    .map_err(|_| KubernetesError::InvalidResponse)?;
                output.push(b':');
                canonical_json(value, output, depth + 1)?;
            }
            output.push(b'}');
        }
        Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                canonical_json(value, output, depth + 1)?;
            }
            output.push(b']');
        }
        scalar => serde_json::to_writer(&mut *output, scalar)
            .map_err(|_| KubernetesError::InvalidResponse)?,
    }
    if output.len() > 1024 * 1024 {
        return Err(KubernetesError::InvalidResponse);
    }
    Ok(())
}

/// Configured Pod API limits only: no runtime allocation, CPU consumption,
/// enforcement, metering interval or zero is inferred from these strings.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct KubernetesConfiguredLimitsV1 {
    pub cpu: Option<String>,
    pub memory: Option<String>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KubernetesProcessStateV1 {
    Waiting,
    Running,
    Terminated,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct KubernetesProcessObservationV1 {
    pub state: Option<KubernetesProcessStateV1>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub exit_code: Option<i32>,
    pub oom_killed: Option<bool>,
    pub restart_count: Option<u64>,
    pub container_id: Option<String>,
    pub image_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KubernetesPodObservationV1 {
    pub authority_fingerprint: String,
    pub pod_uid: String,
    pub resource_version: String,
    pub name: String,
    pub namespace: String,
    pub node_name: Option<String>,
    /// Pod phase only; it is not substituted for the container terminal state.
    pub phase: Option<String>,
    pub deletion_requested_at: Option<DateTime<Utc>>,
    pub process: KubernetesProcessObservationV1,
    pub configured_limits: KubernetesConfiguredLimitsV1,
    pub configuration_sha256: String,
    pub observed_at: DateTime<Utc>,
}

pub(super) fn parse(
    authority: &KubernetesAuthorityV1,
    handle: &KubernetesPodHandle,
    pod: Value,
) -> KubernetesResult<KubernetesPodObservationV1> {
    handle.validate(authority)?;
    let spec = pod.get("spec").ok_or(KubernetesError::InvalidResponse)?;
    if pod.get("apiVersion").and_then(Value::as_str) != Some("v1")
        || pod.get("kind").and_then(Value::as_str) != Some("Pod")
        || pod.pointer("/metadata/namespace").and_then(Value::as_str) != Some(authority.namespace())
        || pod.pointer("/metadata/name").and_then(Value::as_str) != Some(handle.name())
        || pod.pointer("/metadata/uid").and_then(Value::as_str) != Some(handle.uid())
        || KubernetesPodHandle::configuration_digest(spec)? != handle.configuration_sha256
    {
        return Err(KubernetesError::PodChanged);
    }
    let containers = spec
        .get("containers")
        .and_then(Value::as_array)
        .ok_or(KubernetesError::InvalidResponse)?;
    if containers.len() != 1 || containers[0].get("name").and_then(Value::as_str) != Some("worker")
    {
        return Err(KubernetesError::PodChanged);
    }
    for field in ["initContainers", "ephemeralContainers"] {
        if spec
            .get(field)
            .is_some_and(|value| value.as_array().is_none_or(|values| !values.is_empty()))
        {
            return Err(KubernetesError::PodChanged);
        }
    }
    let resource_version = pod
        .pointer("/metadata/resourceVersion")
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
        })
        .ok_or(KubernetesError::InvalidResponse)?
        .to_owned();
    let node_name = optional_string(spec.get("nodeName"), 253)?;
    if node_name.as_ref().is_some_and(|name| !valid_pod_name(name)) {
        return Err(KubernetesError::InvalidResponse);
    }
    let phase = optional_string(pod.pointer("/status/phase"), 16)?;
    if phase.as_ref().is_some_and(|phase| {
        !matches!(
            phase.as_str(),
            "Pending" | "Running" | "Succeeded" | "Failed" | "Unknown"
        )
    }) {
        return Err(KubernetesError::InvalidResponse);
    }
    let mut process = KubernetesProcessObservationV1::default();
    if let Some(statuses) = pod.pointer("/status/containerStatuses") {
        let statuses = statuses
            .as_array()
            .ok_or(KubernetesError::InvalidResponse)?;
        if !statuses.is_empty() {
            if statuses.len() != 1
                || statuses[0].get("name").and_then(Value::as_str) != Some("worker")
            {
                return Err(KubernetesError::PodChanged);
            }
            let status = &statuses[0];
            process.restart_count = match status.get("restartCount") {
                Some(value) => Some(value.as_u64().ok_or(KubernetesError::InvalidResponse)?),
                None => None,
            };
            process.container_id = optional_identity(status.get("containerID"))?;
            process.image_id = optional_identity(status.get("imageID"))?;
            if let Some(state) = status.get("state") {
                let state = state.as_object().ok_or(KubernetesError::InvalidResponse)?;
                if state.len() > 1 {
                    return Err(KubernetesError::InvalidResponse);
                }
                if let Some((kind, facts)) = state.iter().next() {
                    if !facts.is_object() {
                        return Err(KubernetesError::InvalidResponse);
                    }
                    process.state = Some(match kind.as_str() {
                        "waiting" => KubernetesProcessStateV1::Waiting,
                        "running" => KubernetesProcessStateV1::Running,
                        "terminated" => KubernetesProcessStateV1::Terminated,
                        _ => return Err(KubernetesError::InvalidResponse),
                    });
                    if kind == "running" || kind == "terminated" {
                        process.started_at = timestamp(facts.get("startedAt"))?;
                    }
                    if kind == "terminated" {
                        process.finished_at = timestamp(facts.get("finishedAt"))?;
                        process.exit_code = match facts.get("exitCode") {
                            Some(value) => Some(
                                value
                                    .as_i64()
                                    .and_then(|value| i32::try_from(value).ok())
                                    .filter(|value| *value >= 0)
                                    .ok_or(KubernetesError::InvalidResponse)?,
                            ),
                            None => None,
                        };
                        process.oom_killed = optional_string(facts.get("reason"), 128)?
                            .map(|reason| reason == "OOMKilled");
                    }
                }
            }
        }
    }
    if process
        .started_at
        .zip(process.finished_at)
        .is_some_and(|(start, finish)| finish < start)
    {
        return Err(KubernetesError::InvalidResponse);
    }
    let limits = containers[0].pointer("/resources/limits");
    if limits.is_some_and(|value| !value.is_object()) {
        return Err(KubernetesError::InvalidResponse);
    }
    Ok(KubernetesPodObservationV1 {
        authority_fingerprint: authority.fingerprint(),
        pod_uid: handle.uid.clone(),
        resource_version,
        name: handle.name.clone(),
        namespace: authority.namespace().into(),
        node_name,
        phase,
        deletion_requested_at: timestamp(pod.pointer("/metadata/deletionTimestamp"))?,
        process,
        configured_limits: KubernetesConfiguredLimitsV1 {
            cpu: quantity(limits.and_then(|value| value.get("cpu")))?,
            memory: quantity(limits.and_then(|value| value.get("memory")))?,
        },
        configuration_sha256: handle.configuration_sha256.clone(),
        observed_at: Utc::now(),
    })
}
fn optional_string(value: Option<&Value>, limit: usize) -> KubernetesResult<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value))
            if !value.is_empty()
                && value.len() <= limit
                && !value.chars().any(char::is_control) =>
        {
            Ok(Some(value.clone()))
        }
        _ => Err(KubernetesError::InvalidResponse),
    }
}
fn optional_identity(value: Option<&Value>) -> KubernetesResult<Option<String>> {
    // Kubernetes uses an empty identity before the container is created.
    if value.and_then(Value::as_str) == Some("") {
        return Ok(None);
    }
    let result = optional_string(value, 2048)?;
    if result.as_ref().is_some_and(|value| {
        !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/:@._-+".contains(&byte))
    }) {
        return Err(KubernetesError::InvalidResponse);
    }
    Ok(result)
}
fn timestamp(value: Option<&Value>) -> KubernetesResult<Option<DateTime<Utc>>> {
    optional_string(value, 64)?
        .map(|value| {
            DateTime::parse_from_rfc3339(&value)
                .map(|value| value.with_timezone(&Utc))
                .map_err(|_| KubernetesError::InvalidResponse)
                .and_then(|value| {
                    if value.timestamp() > 0 {
                        Ok(value)
                    } else {
                        Err(KubernetesError::InvalidResponse)
                    }
                })
        })
        .transpose()
}
fn quantity(value: Option<&Value>) -> KubernetesResult<Option<String>> {
    let result = optional_string(value, 64)?;
    if result.as_ref().is_some_and(|value| !valid_quantity(value)) {
        return Err(KubernetesError::InvalidResponse);
    }
    Ok(result)
}
fn valid_quantity(value: &str) -> bool {
    let end = value
        .find(|character: char| !character.is_ascii_digit() && character != '.')
        .unwrap_or(value.len());
    let (number, suffix) = value.split_at(end);
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let number_valid = number.split_once('.').map_or_else(
        || digits(number),
        |(whole, fraction)| digits(whole) && digits(fraction),
    );
    number_valid
        && (matches!(
            suffix,
            "" | "n"
                | "u"
                | "m"
                | "k"
                | "K"
                | "M"
                | "G"
                | "T"
                | "P"
                | "E"
                | "Ki"
                | "Mi"
                | "Gi"
                | "Ti"
                | "Pi"
                | "Ei"
        ) || suffix
            .strip_prefix('e')
            .or_else(|| suffix.strip_prefix('E'))
            .is_some_and(|exponent| {
                digits(
                    exponent
                        .strip_prefix('+')
                        .or_else(|| exponent.strip_prefix('-'))
                        .unwrap_or(exponent),
                )
            }))
}
