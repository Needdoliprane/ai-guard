//! Strict acknowledged audit reception for Kubernetes admission inspection.
//! No receipt here authorizes workload execution or independently observes it.
use super::AuditClient;
use crate::model::{Event, StoredEvent};
use serde::{
    Deserialize, Serialize,
    de::{MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::Duration;

const MAX_EVENT_BYTES: usize = 32 * 1024;
const MAX_RECEIPT_BYTES: usize = 64 * 1024;

/// Decode before constructing Value: its ordinary map decoder would discard
/// duplicate members. Reject them at every depth, including target bindings.
struct UniqueJson(Value);
impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON without duplicate members")
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Bool(value)))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Number(value.into())))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Number(value.into())))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(value)
                    .map(|number| UniqueJson(Value::Number(number)))
                    .ok_or_else(|| E::custom("invalid JSON number"))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value.into())))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(UniqueJson(value)) = sequence.next_element::<UniqueJson>()? {
                    values.push(value);
                }
                Ok(UniqueJson(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut fields: A) -> Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(key) = fields.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom("duplicate JSON member"));
                    }
                    let UniqueJson(value) = fields.next_value::<UniqueJson>()?;
                    values.insert(key, value);
                }
                Ok(UniqueJson(Value::Object(values)))
            }
        }
        decoder.deserialize_any(UniqueVisitor)
    }
}

fn decode_receipt(bytes: &[u8]) -> Result<StoredEvent, KubernetesPreflightAuditError> {
    use KubernetesPreflightAuditError as Error;
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let UniqueJson(value) =
        UniqueJson::deserialize(&mut decoder).map_err(|_| Error::InvalidReceipt)?;
    decoder.end().map_err(|_| Error::InvalidReceipt)?;
    // StoredEvent flattens Event. Do not depend on serde_ignored to observe
    // unknown flattened fields: close this exact root envelope explicitly.
    const FIELDS: [&str; 21] = [
        "seq",
        "received_at",
        "prev_hash",
        "hash",
        "id",
        "timestamp",
        "kind",
        "source",
        "provenance",
        "coverage",
        "session_id",
        "project_id",
        "sandbox_id",
        "agent_id",
        "parent_agent_id",
        "correlation_id",
        "trace_id",
        "command",
        "decision",
        "result",
        "data",
    ];
    let fields = value.as_object().ok_or(Error::InvalidReceipt)?;
    if fields.len() != FIELDS.len() || !FIELDS.iter().all(|key| fields.contains_key(*key)) {
        return Err(Error::InvalidReceipt);
    }
    serde_json::from_value(value).map_err(|_| Error::InvalidReceipt)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KubernetesPreflightAuditError {
    InvalidEvent,
    Unavailable,
    Rejected(u16),
    InvalidReceipt,
}
impl std::fmt::Display for KubernetesPreflightAuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEvent => f.write_str("Kubernetes preflight audit event is invalid or contains protected material"),
            Self::Unavailable => f.write_str("Kubernetes preflight audit receipt is unavailable; reception is unknown and no retry was attempted"),
            Self::Rejected(status) => write!(f,"Kubernetes preflight audit request was rejected with HTTP {status}; no retry was attempted"),
            Self::InvalidReceipt => f.write_str("Kubernetes preflight audit receipt differs from the submitted event or is incomplete; reception is unknown"),
        }
    }
}

/// Only constructed after exact event echo and envelope checks. This is an
/// API acknowledgement of durable reception, not a remote attestation.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct KubernetesPreflightAuditReceiptV1 {
    version: u32,
    event_id: String,
    event_sha256: String,
    seq: i64,
    received_at: String,
    prev_hash: String,
    hash: String,
}
impl KubernetesPreflightAuditReceiptV1 {
    pub(crate) fn matches_event(&self, event: &Event) -> bool {
        self.version == 1
            && self.event_id == event.id
            && event_commitment(event).is_ok_and(|hash| hash == self.event_sha256)
    }
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn event_commitment(event: &Event) -> Result<String, KubernetesPreflightAuditError> {
    let value =
        serde_json::to_value(event).map_err(|_| KubernetesPreflightAuditError::InvalidEvent)?;
    let bytes =
        serde_json::to_vec(&value).map_err(|_| KubernetesPreflightAuditError::InvalidEvent)?;
    let mut hash = Sha256::new();
    hash.update(b"ai-guard:kubernetes-preflight-audit-event:v1\0");
    hash.update(bytes);
    Ok(hex::encode(hash.finalize()))
}

impl AuditClient {
    pub(crate) async fn append_kubernetes_preflight_event(
        &self,
        event: &Event,
    ) -> Result<KubernetesPreflightAuditReceiptV1, KubernetesPreflightAuditError> {
        use KubernetesPreflightAuditError as Error;
        event.validate().map_err(|_| Error::InvalidEvent)?;
        if !matches!(
            event.kind.as_str(),
            "runtime.kubernetes.preflight.requested" | "runtime.kubernetes.preflight.result"
        ) || event.source != "trusted-control"
            || event.provenance != "declared"
            || event.coverage != "partial"
        {
            return Err(Error::InvalidEvent);
        }
        let value = serde_json::to_value(event).map_err(|_| Error::InvalidEvent)?;
        if self.redactor.value_unclipped(&value) != value {
            return Err(Error::InvalidEvent);
        }
        let bytes = serde_json::to_vec(event).map_err(|_| Error::InvalidEvent)?;
        if bytes.len() > MAX_EVENT_BYTES {
            return Err(Error::InvalidEvent);
        }
        // This mutation has no automatic retry, including protocol-level retry.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|_| Error::Unavailable)?;
        let url = self
            .base
            .join("api/v1/events")
            .map_err(|_| Error::InvalidEvent)?;
        let mut response = client
            .post(url)
            .bearer_auth(&self.token)
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .body(bytes)
            .send()
            .await
            .map_err(|_| Error::Unavailable)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::Rejected(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RECEIPT_BYTES as u64)
        {
            return Err(Error::InvalidReceipt);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Unavailable)? {
            if chunk.len() > MAX_RECEIPT_BYTES.saturating_sub(bytes.len()) {
                return Err(Error::InvalidReceipt);
            }
            bytes.extend_from_slice(&chunk);
        }
        let stored = decode_receipt(&bytes)?;
        let echoed = serde_json::to_value(&stored.event).map_err(|_| Error::InvalidReceipt)?;
        let received = chrono::DateTime::parse_from_rfc3339(&stored.received_at)
            .map_err(|_| Error::InvalidReceipt)?;
        if echoed != value
            || stored.seq <= 0
            || !digest(&stored.hash)
            || !digest(&stored.prev_hash)
            || stored.received_at.len() > 64
            || received.timestamp() <= 0
        {
            return Err(Error::InvalidReceipt);
        }
        Ok(KubernetesPreflightAuditReceiptV1 {
            version: 1,
            event_id: stored.event.id.clone(),
            event_sha256: event_commitment(event)?,
            seq: stored.seq,
            received_at: stored.received_at,
            prev_hash: stored.prev_hash,
            hash: stored.hash,
        })
    }
}
