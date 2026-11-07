//! Pure business checks shared by the HTTP client and the private frame protocol.
//! A frame never supplies a credential; trusted control can separately supply
//! the retained proof to these checks before any persistence or dispatch.
use super::client::RelayClientError;
use super::{
    MAX_MESSAGE_BYTES, MAX_MESSAGE_TTL_SECONDS, RelayAckV1, RelayEndpointV1, RelayInboxV1,
    RelayReceiptV1, RelaySendV1, channel_valid,
};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

pub(super) const MAX_RECEIPT_BYTES: usize = 16_384;
pub(super) const MAX_REQUEST_BYTES: usize = 262_144;
type Result<T> = std::result::Result<T, RelayClientError>;

pub(super) fn validate_send(input: &RelaySendV1, credential: Option<&str>) -> Result<()> {
    if input.version != 1
        || !uuid(&input.operation_id)
        || !uuid(&input.grant_id)
        || input.ttl_seconds == 0
        || input.ttl_seconds > MAX_MESSAGE_TTL_SECONDS
        || input.body.is_empty()
        || input.body.len() > MAX_MESSAGE_BYTES
    {
        return Err(RelayClientError::InvalidRequest);
    }
    // Inspect decoded body literals before a map could erase an ambiguous key.
    // Plain text remains plain text; this is not an exhaustive secret detector.
    match crate::json::decode_unique(input.body.as_bytes(), 64) {
        Ok(value) if credential.is_some_and(|secret| contains_credential(&value, secret)) => {
            Err(RelayClientError::InvalidRequest)
        }
        Err(_) if serde_json::from_str::<serde_json::Value>(&input.body).is_ok() => {
            Err(RelayClientError::InvalidRequest)
        }
        _ => Ok(()),
    }
}

pub(super) fn validate_ack(input: &RelayAckV1) -> Result<()> {
    if input.version != 1 || !uuid(&input.operation_id) || !uuid(&input.message_id) {
        Err(RelayClientError::InvalidRequest)
    } else {
        Ok(())
    }
}

pub(super) fn validate_inbox_request(after: Option<i64>, limit: u32) -> Result<()> {
    if after.is_some_and(|value| value < 0) || !(1..=32).contains(&limit) {
        Err(RelayClientError::InvalidRequest)
    } else {
        Ok(())
    }
}

pub(super) fn request_bytes<T: Serialize>(input: &T, credential: Option<&str>) -> Result<Vec<u8>> {
    let value = serde_json::to_value(input).map_err(|_| RelayClientError::InvalidRequest)?;
    if credential.is_some_and(|secret| contains_credential(&value, secret)) {
        return Err(RelayClientError::InvalidRequest);
    }
    let bytes = serde_json::to_vec(&value).map_err(|_| RelayClientError::InvalidRequest)?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(RelayClientError::InvalidRequest);
    }
    Ok(bytes)
}

pub(super) fn decode<T: DeserializeOwned + Serialize>(
    bytes: &[u8],
    credential: Option<&str>,
) -> Result<T> {
    let value = crate::json::decode_unique(bytes, 64).map_err(|_| RelayClientError::Protocol)?;
    if credential.is_some_and(|secret| contains_credential(&value, secret)) {
        return Err(RelayClientError::Protocol);
    }
    let decoded: T =
        serde_json::from_value(value.clone()).map_err(|_| RelayClientError::Protocol)?;
    // Option fields require their explicit null value on the closed wire.
    if serde_json::to_value(&decoded).map_err(|_| RelayClientError::Protocol)? != value {
        return Err(RelayClientError::Protocol);
    }
    Ok(decoded)
}

pub(super) fn validate_inbox(
    inbox: &RelayInboxV1,
    endpoint: &RelayEndpointV1,
    after: Option<i64>,
    limit: u32,
) -> Result<()> {
    if inbox.version != 1 || inbox.items.len() > limit as usize {
        return Err(RelayClientError::Protocol);
    }
    let mut previous = after.unwrap_or(0);
    let mut message_ids = std::collections::HashSet::new();
    for item in &inbox.items {
        let meta = &item.metadata;
        let times = chrono::DateTime::parse_from_rfc3339(&meta.created_at).and_then(|created| {
            chrono::DateTime::parse_from_rfc3339(&meta.expires_at).map(|expires| expires > created)
        });
        if item.sequence <= previous
            || meta.version != 1
            || !uuid(&meta.message_id)
            || !uuid(&meta.grant_id)
            || !message_ids.insert(meta.message_id.as_str())
            || meta.source.validate().is_err()
            || &meta.destination != endpoint
            || !channel_valid(&meta.channel)
            || meta.acknowledged
            || meta.body_pruned
            || item.body.is_empty()
            || item.body.len() > MAX_MESSAGE_BYTES
            || meta.body_bytes != item.body.len() as u64
            || meta.body_sha256 != hex::encode(Sha256::digest(item.body.as_bytes()))
            || !times.is_ok_and(|ordered| ordered)
        {
            return Err(RelayClientError::Protocol);
        }
        previous = item.sequence;
    }
    if inbox
        .next_after
        .is_some_and(|cursor| inbox.items.is_empty() || cursor != previous)
    {
        return Err(RelayClientError::Protocol);
    }
    Ok(())
}

pub(super) fn uuid(value: &str) -> bool {
    crate::control::uuid(value).is_ok()
}

pub(super) fn validate_receipt(
    receipt: &RelayReceiptV1,
    operation: &str,
    resource: &str,
) -> Result<()> {
    if receipt.version != 1
        || receipt.operation_id != operation
        || receipt.resource_id != resource
        || receipt.audit_seq <= 0
        || !hash(&receipt.audit_hash)
    {
        return Err(RelayClientError::Protocol);
    }
    Ok(())
}

fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}

fn contains_credential(value: &serde_json::Value, credential: &str) -> bool {
    match value {
        serde_json::Value::String(text) => text.contains(credential),
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| contains_credential(value, credential)),
        serde_json::Value::Object(values) => values
            .iter()
            .any(|(key, value)| key.contains(credential) || contains_credential(value, credential)),
        _ => false,
    }
}
