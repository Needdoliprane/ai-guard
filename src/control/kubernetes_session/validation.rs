use super::KubernetesSessionReservationFailureV2 as Failure;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(crate) fn require(valid: bool) -> anyhow::Result<()> {
    if valid {
        Ok(())
    } else {
        Err(Failure::InvalidRequest.into())
    }
}
pub(crate) fn uuid(value: &str) -> anyhow::Result<()> {
    require(uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value))
}
pub(crate) fn hash(value: &str) -> anyhow::Result<()> {
    require(
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
    )
}
fn sorted(value: Value, depth: usize) -> anyhow::Result<Value> {
    require(depth <= 64)?;
    Ok(match value {
        Value::Object(values) => {
            let ordered: BTreeMap<_, _> = values.into_iter().collect();
            let mut output = serde_json::Map::new();
            for (key, value) in ordered {
                output.insert(key, sorted(value, depth + 1)?);
            }
            Value::Object(output)
        }
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| sorted(value, depth + 1))
                .collect::<anyhow::Result<_>>()?,
        ),
        Value::Number(ref number) if number.is_f64() => return Err(Failure::InvalidRequest.into()),
        value => value,
    })
}
pub(crate) fn canonical(value: &impl Serialize) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&sorted(
        serde_json::to_value(value)?,
        0,
    )?)?)
}
pub(crate) fn digest(domain: &[u8], value: &impl Serialize) -> anyhow::Result<String> {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(canonical(value)?);
    Ok(hex::encode(hash.finalize()))
}
pub(crate) fn unique(bytes: &[u8], maximum: usize) -> anyhow::Result<Value> {
    require(bytes.len() <= maximum)?;
    crate::json::decode_unique(bytes, 64).map_err(|_| Failure::InvalidRequest.into())
}
