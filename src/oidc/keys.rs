use super::{OidcConfigV1, OidcError};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{DecodingKey, jwk::Jwk};
use openidconnect::core::CoreJsonWebKeySet;
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone)]
pub(super) struct SigningKey {
    pub kid: String,
    pub key: DecodingKey,
}
pub(super) fn parse(
    bytes: &[u8],
    config: &OidcConfigV1,
) -> Result<(Vec<SigningKey>, CoreJsonWebKeySet), OidcError> {
    let invalid = OidcError::InvalidKeys;
    let value = super::json::parse(bytes).map_err(|_| invalid)?;
    let values = value.get("keys").and_then(Value::as_array).ok_or(invalid)?;
    if values.is_empty() || values.len() > config.limits.max_jwks_keys {
        return Err(invalid);
    }
    let mut kids = BTreeSet::new();
    let mut keys = Vec::new();
    let mut compatible = Vec::new();
    for value in values {
        let kid = value
            .get("kid")
            .and_then(Value::as_str)
            .filter(|v| super::config::text(v, 128))
            .ok_or(invalid)?;
        if !kids.insert(kid) {
            return Err(invalid);
        }
        // Public verification only. Never import RSA private operations from a provider response.
        if ["d", "p", "q", "dp", "dq", "qi", "oth", "k"]
            .iter()
            .any(|f| value.get(f).is_some())
        {
            return Err(invalid);
        }
        if value.get("kty").and_then(Value::as_str) != Some("RSA") {
            continue;
        }
        if value.get("use").is_some_and(|v| v.as_str() != Some("sig"))
            || value
                .get("alg")
                .is_some_and(|v| v.as_str() != Some("RS256"))
        {
            continue;
        }
        if let Some(ops) = value.get("key_ops")
            && ops
                .as_array()
                .is_none_or(|ops| ops.len() != 1 || ops[0].as_str() != Some("verify"))
        {
            continue;
        }
        let n = integer(value.get("n"))?;
        let e = integer(value.get("e"))?;
        if n.len() < 256 || n.len() > 1024 || n[0] < 128 || e.len() > 4 {
            return Err(invalid);
        }
        let exponent = e.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
        if exponent < 3 || exponent % 2 == 0 {
            return Err(invalid);
        }
        let jwk: Jwk = serde_json::from_value(value.clone()).map_err(|_| invalid)?;
        keys.push(SigningKey {
            kid: kid.to_owned(),
            key: DecodingKey::from_jwk(&jwk).map_err(|_| invalid)?,
        });
        compatible.push(value.clone());
    }
    if keys.is_empty() {
        return Err(invalid);
    }
    let jwks =
        serde_json::from_value(serde_json::json!({"keys":compatible})).map_err(|_| invalid)?;
    Ok((keys, jwks))
}
fn integer(v: Option<&Value>) -> Result<Vec<u8>, OidcError> {
    let s = v
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 1366)
        .ok_or(OidcError::InvalidKeys)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|_| OidcError::InvalidKeys)?;
    if bytes.is_empty() || bytes[0] == 0 || URL_SAFE_NO_PAD.encode(&bytes) != s {
        return Err(OidcError::InvalidKeys);
    }
    Ok(bytes)
}
