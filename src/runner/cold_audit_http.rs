//! Strict, bounded control HTTP data for retained audit delivery.
//! Neither a Worker response nor an event receipt grants journal or runtime rights.
use super::AuditClient;
use crate::{
    control::WorkerV1,
    model::{Event, StoredEvent},
};
use reqwest::{Method, StatusCode, header};
use serde::de::{DeserializeOwned, IntoDeserializer};
use serde_json::Value;
use std::{
    fmt,
    io::{self, Write},
    time::Duration,
};
use tokio::time::Instant;
use uuid::Uuid;

const TOTAL_BUDGET: Duration = Duration::from_secs(45);
const MAX_REQUEST_BYTES: usize = 120 * 1024;
const MAX_RESPONSE_BYTES: usize = 512 * 1024;
const MAX_JSON_DEPTH: usize = 64;
const STORAGE_FIELDS: [&str; 4] = ["seq", "received_at", "prev_hash", "hash"];
const EVENT_FIELDS: [&str; 17] = [
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

/// One deadline sampled before the identity read and retained through body reads.
/// There is no serialized origin or method that extends an existing budget.
pub(crate) struct ColdAuditBudget {
    deadline: Instant,
}

impl ColdAuditBudget {
    pub(crate) fn start() -> Self {
        let now = Instant::now();
        Self {
            deadline: now.checked_add(TOTAL_BUDGET).unwrap_or(now),
        }
    }

    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }

    pub(crate) fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    fn require_remaining(&self) -> Result<(), ColdAuditHttpError> {
        if self.remaining().is_zero() {
            Err(ColdAuditHttpError::Expired)
        } else {
            Ok(())
        }
    }

    fn transport_failure(&self) -> ColdAuditHttpError {
        if self.remaining().is_zero() {
            ColdAuditHttpError::Expired
        } else {
            ColdAuditHttpError::Unavailable
        }
    }
}

impl fmt::Debug for ColdAuditBudget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ColdAuditBudget(<private deadline>)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ColdAuditHttpError {
    InvalidRequest,
    InvalidResponse,
    Expired,
    Unavailable,
}

impl fmt::Display for ColdAuditHttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRequest => "cold audit request is invalid",
            Self::InvalidResponse => "cold audit response failed validation",
            Self::Expired => "cold audit HTTP budget expired",
            Self::Unavailable => "cold audit control HTTP is unavailable",
        })
    }
}

impl std::error::Error for ColdAuditHttpError {}

/// Exact data returned by the control HTTP exchange, never a journal ACK.
/// Its producer must verify the entire stored Event against the retained input.
pub(crate) struct ExactStoredEventReceipt {
    stored: StoredEvent,
}

impl ExactStoredEventReceipt {
    pub(crate) fn stored_event(&self) -> &StoredEvent {
        &self.stored
    }

    pub(crate) fn seq(&self) -> i64 {
        self.stored.seq
    }
}

impl fmt::Debug for ExactStoredEventReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExactStoredEventReceipt(<private>)")
    }
}

impl AuditClient {
    /// Descriptive response only: this does not establish a tenant or terminal.
    pub(crate) async fn cold_worker(
        &self,
        worker_id: &str,
        budget: &ColdAuditBudget,
    ) -> Result<WorkerV1, ColdAuditHttpError> {
        budget.require_remaining()?;
        if !canonical_id(worker_id) || self.redactor.text(worker_id) != worker_id {
            return Err(ColdAuditHttpError::InvalidRequest);
        }
        let value = self
            .cold_exchange(&format!("api/v1/workers/{worker_id}"), None, budget)
            .await?;
        let worker: WorkerV1 = decode_closed(value)?;
        // WorkerV1 has no top-level version field. Its versioned launch and
        // decision receipt must retain their existing format, without treating
        // any of these descriptive fields as a terminal or start permission.
        if worker.worker_id != worker_id
            || worker.launch.version != 1
            || worker.receipt.version != 1
        {
            return Err(ColdAuditHttpError::InvalidResponse);
        }
        budget.require_remaining()?;
        Ok(worker)
    }

    pub(crate) async fn post_cold_audit_event(
        &self,
        event: &Event,
        budget: &ColdAuditBudget,
    ) -> Result<ExactStoredEventReceipt, ColdAuditHttpError> {
        budget.require_remaining()?;
        let (body, expected) = self.cold_event_body(event)?;
        let value = self
            .cold_exchange("api/v1/events", Some(body), budget)
            .await?;
        exact_stored_event_receipt(value, &expected, budget)
    }

    /// Exact data for this one authenticated control request. A positive seq
    /// alone, changed masking or a changed stored Event never grants an ACK.
    // Closed retirement resume HTTP target; production wiring pending.
    #[allow(dead_code)]
    pub(crate) async fn post_retirement_resume_event(
        &self,
        event: &Event,
        original_until: Instant,
    ) -> Result<ExactStoredEventReceipt, ColdAuditHttpError> {
        let bounded = ColdAuditBudget::start();
        let budget = ColdAuditBudget {
            deadline: bounded.deadline().min(original_until),
        };
        budget.require_remaining()?;
        let (body, expected) = self.retirement_resume_body(event)?;
        // Reuse the closed runner transport selection, not a caller URL or an
        // ordinary-ingestion fallback. The cold exchange retains existing IO caps.
        let path = "control/api/v1/control/events";
        let selected = self
            .control_url(Method::POST, "api/v1/control/events")
            .map_err(|_| ColdAuditHttpError::InvalidRequest)?;
        if self
            .base
            .join(path)
            .map_err(|_| ColdAuditHttpError::InvalidRequest)?
            != selected
        {
            return Err(ColdAuditHttpError::InvalidRequest);
        }
        let value = self.cold_exchange(path, Some(body), &budget).await?;
        exact_stored_event_receipt(value, &expected, &budget)
    }

    /// Validation before the local outbox append as well as before HTTP. A
    /// changed redaction is refused; it cannot silently change a bound digest.
    // Closed retirement resume HTTP target; production wiring pending.
    #[allow(dead_code)]
    pub(crate) fn validate_retirement_resume_event(
        &self,
        event: &Event,
    ) -> Result<(), ColdAuditHttpError> {
        self.retirement_resume_body(event).map(|_| ())
    }

    // Closed retirement resume HTTP target; production wiring pending.
    #[allow(dead_code)]
    fn retirement_resume_body(
        &self,
        event: &Event,
    ) -> Result<(Vec<u8>, Value), ColdAuditHttpError> {
        crate::audit::retirement_resume_record(event)
            .map_err(|_| ColdAuditHttpError::InvalidRequest)?;
        if event.validate().is_err() || !canonical_timestamp(&event.timestamp) {
            return Err(ColdAuditHttpError::InvalidRequest);
        }
        let mut body = RequestBytes(Vec::new());
        serde_json::to_writer(&mut body, event).map_err(|_| ColdAuditHttpError::InvalidRequest)?;
        let expected = crate::json::decode_unique(&body.0, MAX_JSON_DEPTH)
            .map_err(|_| ColdAuditHttpError::InvalidRequest)?;
        if self.redactor.value(&expected) != expected {
            return Err(ColdAuditHttpError::InvalidRequest);
        }
        Ok((body.0, expected))
    }

    fn cold_event_body(&self, event: &Event) -> Result<(Vec<u8>, Value), ColdAuditHttpError> {
        if !matches!(
            event.kind.as_str(),
            "broker.batch.result" | "observer.coverage"
        ) || !canonical_id(&event.id)
            || event.timestamp.len() > 64
            || !canonical_timestamp(&event.timestamp)
            || event.validate().is_err()
        {
            return Err(ColdAuditHttpError::InvalidRequest);
        }
        // Bound arbitrary data/result before recursive serialization and cap the
        // output allocation before making a Value copy for the real redactor.
        let mut nodes_left = MAX_REQUEST_BYTES;
        if !bounded_value(&event.data, 1, &mut nodes_left)
            || event
                .result
                .as_ref()
                .is_some_and(|value| !bounded_value(value, 1, &mut nodes_left))
        {
            return Err(ColdAuditHttpError::InvalidRequest);
        }
        let mut body = RequestBytes(Vec::new());
        serde_json::to_writer(&mut body, event).map_err(|_| ColdAuditHttpError::InvalidRequest)?;
        let value = crate::json::decode_unique(&body.0, MAX_JSON_DEPTH)
            .map_err(|_| ColdAuditHttpError::InvalidRequest)?;
        if self.redactor.value(&value) != value {
            return Err(ColdAuditHttpError::InvalidRequest);
        }
        Ok((body.0, value))
    }

    async fn cold_exchange(
        &self,
        path: &str,
        body: Option<Vec<u8>>,
        budget: &ColdAuditBudget,
    ) -> Result<Value, ColdAuditHttpError> {
        let url = self
            .base
            .join(path)
            .map_err(|_| ColdAuditHttpError::InvalidRequest)?;
        let mut request = self
            .http
            .request(
                if body.is_some() {
                    Method::POST
                } else {
                    Method::GET
                },
                url,
            )
            .bearer_auth(&self.token)
            .header(header::ACCEPT, "application/json")
            .header(header::CACHE_CONTROL, "no-store");
        if let Some(body) = body {
            request = request
                .header(header::CONTENT_TYPE, "application/json")
                .body(body);
        }
        // The caller's single deadline also covers send/header and every body
        // read. No preparation or handoff may renew it or initiate a late send.
        budget.require_remaining()?;
        let mut response = tokio::time::timeout_at(budget.deadline(), request.send())
            .await
            .map_err(|_| ColdAuditHttpError::Expired)?
            .map_err(|_| budget.transport_failure())?;
        if response.status() != StatusCode::OK
            || response
                .content_length()
                .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
            || response
                .headers()
                .get_all(header::CONTENT_TYPE)
                .iter()
                .count()
                != 1
            || !response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next())
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
        {
            return Err(ColdAuditHttpError::InvalidResponse);
        }
        let mut bytes = Vec::new();
        loop {
            budget.require_remaining()?;
            let chunk = tokio::time::timeout_at(budget.deadline(), response.chunk())
                .await
                .map_err(|_| ColdAuditHttpError::Expired)?
                .map_err(|_| budget.transport_failure())?;
            let Some(chunk) = chunk else {
                break;
            };
            if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
                return Err(ColdAuditHttpError::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        budget.require_remaining()?;
        let value = crate::json::decode_unique(&bytes, MAX_JSON_DEPTH)
            .map_err(|_| ColdAuditHttpError::InvalidResponse)?;
        budget.require_remaining()?;
        Ok(value)
    }
}

fn exact_stored_event_receipt(
    value: Value,
    expected: &Value,
    budget: &ColdAuditBudget,
) -> Result<ExactStoredEventReceipt, ColdAuditHttpError> {
    let fields = value
        .as_object()
        .ok_or(ColdAuditHttpError::InvalidResponse)?;
    if fields.len() != STORAGE_FIELDS.len() + EVENT_FIELDS.len()
        || !STORAGE_FIELDS
            .iter()
            .chain(EVENT_FIELDS.iter())
            .all(|field| fields.contains_key(*field))
    {
        return Err(ColdAuditHttpError::InvalidResponse);
    }
    // Compare before deserializing the flattened Event. No ignored field,
    // arbitrary data/result member, or absent Option can disappear here.
    let mut returned_event = fields.clone();
    for field in STORAGE_FIELDS {
        returned_event.remove(field);
    }
    if &Value::Object(returned_event) != expected {
        return Err(ColdAuditHttpError::InvalidResponse);
    }
    let stored: StoredEvent = decode_closed(value)?;
    if stored.seq <= 0
        || !hash64(&stored.prev_hash)
        || !hash64(&stored.hash)
        || stored.received_at.len() > 64
        || chrono::DateTime::parse_from_rfc3339(&stored.received_at).is_err()
        || serde_json::to_value(&stored.event).map_err(|_| ColdAuditHttpError::InvalidResponse)?
            != *expected
    {
        return Err(ColdAuditHttpError::InvalidResponse);
    }
    budget.require_remaining()?;
    Ok(ExactStoredEventReceipt { stored })
}

fn decode_closed<T: DeserializeOwned>(value: Value) -> Result<T, ColdAuditHttpError> {
    let mut ignored = false;
    let decoded = serde_ignored::deserialize(value.into_deserializer(), |_| ignored = true)
        .map_err(|_| ColdAuditHttpError::InvalidResponse)?;
    if ignored {
        return Err(ColdAuditHttpError::InvalidResponse);
    }
    Ok(decoded)
}

fn canonical_id(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}

fn canonical_timestamp(value: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(value).is_ok_and(|timestamp| {
        timestamp
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            == value
    })
}

fn hash64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn bounded_value(value: &Value, depth: usize, nodes_left: &mut usize) -> bool {
    if depth > MAX_JSON_DEPTH || *nodes_left == 0 {
        return false;
    }
    *nodes_left -= 1;
    match value {
        Value::Array(values) => values
            .iter()
            .all(|value| bounded_value(value, depth + 1, nodes_left)),
        Value::Object(fields) => fields
            .values()
            .all(|value| bounded_value(value, depth + 1, nodes_left)),
        _ => true,
    }
}

struct RequestBytes(Vec<u8>);

impl Write for RequestBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_REQUEST_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other("cold audit request exceeds its bound"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "cold_audit_http_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "retirement_resume_http_tests.rs"]
mod retirement_resume_tests;
