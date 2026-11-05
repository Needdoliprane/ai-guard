use super::*;
use crate::runtime::kubernetes::KubernetesPreflightPlanV1;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserializer, de::Error};
use sha2::{Digest, Sha256};
use uuid::Uuid;

fn require(ok: bool) -> anyhow::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(KubernetesCreateFailureV1::InvalidRequest.into())
    }
}
fn id(value: &str) -> anyhow::Result<Uuid> {
    let parsed = Uuid::parse_str(value).map_err(|_| KubernetesCreateFailureV1::InvalidRequest)?;
    require(!parsed.is_nil() && parsed.to_string() == value)?;
    Ok(parsed)
}
fn hash(value: &str) -> anyhow::Result<()> {
    require(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
    )
}
fn bounded(value: &str, maximum: usize) -> anyhow::Result<()> {
    require(!value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control))
}
fn revision(value: u64) -> anyhow::Result<()> {
    require(value > 0 && value <= i64::MAX as u64)
}
pub(super) fn utc(value: &str) -> anyhow::Result<DateTime<Utc>> {
    bounded(value, 64)?;
    let time = DateTime::parse_from_rfc3339(value)
        .map_err(|_| KubernetesCreateFailureV1::InvalidRequest)?
        .with_timezone(&Utc);
    require(time.timestamp() > 0 && time.to_rfc3339_opts(SecondsFormat::Millis, true) == value)?;
    Ok(time)
}

// The existing authority uses a typed UUID. Preserve the new wire contract's
// canonical spelling before that UUID could normalize the original input.
pub(super) fn deserialize_authority<'de, D>(
    deserializer: D,
) -> Result<KubernetesAuthorityV1, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Wire {
        version: u32,
        authority_id: String,
        endpoint: String,
        cluster_namespace_uid: String,
        namespace: String,
        namespace_uid: String,
        trust_sha256: String,
    }
    let wire = Wire::deserialize(deserializer)?;
    if wire.version != 1 {
        return Err(D::Error::custom("invalid held Kubernetes authority"));
    }
    let authority_id = id(&wire.authority_id)
        .map_err(|_| D::Error::custom("invalid held Kubernetes authority"))?;
    KubernetesAuthorityV1::new(
        authority_id,
        &wire.endpoint,
        &wire.cluster_namespace_uid,
        &wire.namespace,
        &wire.namespace_uid,
        &wire.trust_sha256,
    )
    .map_err(|_| D::Error::custom("invalid held Kubernetes authority"))
}

fn depth(value: &Value, level: usize) -> anyhow::Result<()> {
    require(level <= 64)?;
    match value {
        Value::Object(values) => {
            for value in values.values() {
                depth(value, level + 1)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                depth(value, level + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn sorted(value: Value, level: usize) -> anyhow::Result<Value> {
    require(level <= 64)?;
    Ok(match value {
        Value::Object(values) => {
            let ordered: BTreeMap<_, _> = values.into_iter().collect();
            let mut result = serde_json::Map::new();
            for (key, value) in ordered {
                result.insert(key, sorted(value, level + 1)?);
            }
            Value::Object(result)
        }
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| sorted(value, level + 1))
                .collect::<anyhow::Result<Vec<_>>>()?,
        ),
        other => other,
    })
}
fn bytes<T: Serialize>(value: &T) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&sorted(
        serde_json::to_value(value)?,
        0,
    )?)?)
}
pub(crate) fn canonical_hash<T: Serialize>(domain: &[u8], value: &T) -> anyhow::Result<String> {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(bytes(value)?);
    Ok(hex::encode(digest.finalize()))
}

impl KubernetesCreateIntentV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(self.version == 1 && self.profile == KUBERNETES_HELD_PROFILE_V1)?;
        let create = id(&self.create_id)?;
        let session = id(&self.session_id)?;
        for value in [
            &self.control_namespace_uuid,
            &self.broker_id,
            &self.incarnation_id,
        ] {
            id(value)?;
        }
        // Pure validation only. A preflight plan is not an emission permission.
        KubernetesPreflightPlanV1::new(
            &self.authority,
            session,
            create,
            &self.image,
            self.resources.clone(),
        )
        .map_err(|_| KubernetesCreateFailureV1::InvalidRequest)?;
        require(bytes(self)?.len() <= 16 * 1024)
    }
    pub fn intent_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        canonical_hash(b"ai-guard:kubernetes-create-intent:v1\0", self)
    }
}

fn mutation(
    version: u32,
    operation: &str,
    create: &str,
    intent: &str,
    expected_revision: u64,
    expected_epoch: u64,
) -> anyhow::Result<()> {
    require(version == 1 && operation != create)?;
    id(operation)?;
    id(create)?;
    hash(intent)?;
    revision(expected_revision)?;
    revision(expected_epoch)
}
impl KubernetesCreateClaimV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        mutation(
            self.version,
            &self.operation_id,
            &self.create_id,
            &self.intent_hash,
            self.expected_revision,
            self.expected_create_epoch,
        )?;
        id(&self.emitter_id)?;
        require(bytes(self)?.len() <= 64 * 1024)
    }
    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        canonical_hash(b"ai-guard:kubernetes-create-claim:v1\0", self)
    }
}
impl KubernetesCreateCancelV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        mutation(
            self.version,
            &self.operation_id,
            &self.create_id,
            &self.intent_hash,
            self.expected_revision,
            self.expected_create_epoch,
        )?;
        require(bytes(self)?.len() <= 64 * 1024)
    }
    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        canonical_hash(b"ai-guard:kubernetes-create-cancel:v1\0", self)
    }
}

fn projection_shape(observation: &KubernetesCreateObservationV1) -> anyhow::Result<()> {
    require(
        observation.version == 1 && observation.api_version == "v1" && observation.kind == "Pod",
    )?;
    let metadata = &observation.metadata;
    bounded(&metadata.name, 253)?;
    bounded(&metadata.namespace, 63)?;
    id(&metadata.uid)?;
    bounded(&metadata.resource_version, 128)?;
    require(
        metadata.labels.len() == 4
            && metadata.annotation_count == 0
            && metadata.owner_reference_count == 0
            && metadata.finalizer_count == 0
            && !metadata.deletion_requested
            && !metadata.unexpected_fields_present,
    )?;
    for (key, value) in &metadata.labels {
        bounded(key, 128)?;
        bounded(value, 256)?;
    }
    if let Some(status) = &observation.status {
        require(
            status
                .phase
                .as_deref()
                .is_none_or(|phase| phase == "Pending")
                && status.container_status_count.is_none_or(|count| count == 0)
                && status
                    .init_container_status_count
                    .is_none_or(|count| count == 0)
                && status
                    .ephemeral_container_status_count
                    .is_none_or(|count| count == 0)
                && !status.start_time_present
                && !status.placement_fields_present
                && !status.unexpected_fields_present
                && status.pod_scheduled.as_ref().is_none_or(|condition| {
                    condition.status == "False" && condition.reason == "SchedulingGated"
                }),
        )?;
    }
    require(bytes(observation)?.len() <= 4 * 1024)
}
impl KubernetesCreateResultV1 {
    /// Structural validation only; the Store must also validate the safe spec
    /// and projection against the retained intent before any persistence.
    pub fn validate(&self) -> anyhow::Result<()> {
        mutation(
            self.version,
            &self.operation_id,
            &self.create_id,
            &self.intent_hash,
            self.expected_revision,
            self.expected_create_epoch,
        )?;
        id(&self.claim_operation_id)?;
        require(
            self.claim_operation_id != self.operation_id
                && self.claim_operation_id != self.create_id,
        )?;
        match &self.outcome {
            KubernetesCreateOutcomeV1::OwnershipUnknown {
                reason,
                http_status,
            } => match reason {
                KubernetesCreateUnknownReasonV1::ApiRejected => {
                    require(http_status.is_some_and(|status| (100..=599).contains(&status)))?;
                }
                _ => require(http_status.is_none())?,
            },
            KubernetesCreateOutcomeV1::CreatedHeld {
                observation,
                admitted_spec,
                admitted_configuration_sha256,
                observed_at,
            } => {
                projection_shape(observation)?;
                hash(admitted_configuration_sha256)?;
                utc(observed_at)?;
                require(admitted_spec.is_object())?;
                depth(admitted_spec, 0)?;
                require(bytes(admitted_spec)?.len() <= 32 * 1024)?;
            }
        }
        require(bytes(self)?.len() <= 64 * 1024)
    }
    pub fn input_hash(&self) -> anyhow::Result<String> {
        self.validate()?;
        canonical_hash(b"ai-guard:kubernetes-create-result:v1\0", self)
    }
}
impl KubernetesCreateReceiptV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        require(self.version == 1 && self.audit_seq > 0)?;
        id(&self.operation_id)?;
        id(&self.control_namespace_uuid)?;
        hash(&self.input_hash)?;
        hash(&self.audit_hash)?;
        if let Some(window) = &self.emission_window {
            let duration = utc(&window.not_after)? - utc(&window.issued_at)?;
            require(
                duration > chrono::TimeDelta::zero() && duration <= chrono::TimeDelta::seconds(8),
            )?;
        }
        // Receipts are historical data: current expiry is checked only by the
        // fresh runtime permission, never turned into authority by this parser.
        Ok(())
    }
}

impl KubernetesCreateSnapshotV1 {
    pub(crate) fn snapshot_hash(&self) -> anyhow::Result<String> {
        canonical_hash(b"ai-guard:kubernetes-create-snapshot:v1\0", self)
    }
}
