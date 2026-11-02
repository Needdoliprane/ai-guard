use super::{OidcError, OidcProvider, VerifiedAccess, config::text, json::UniqueValue};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, Header, Validation};
use serde_json::Value;
use std::{collections::BTreeSet, time::Instant};

impl OidcProvider {
    pub async fn verify_access(&self, token: &str) -> Result<VerifiedAccess, OidcError> {
        let _operation = self
            .operations
            .try_acquire()
            .map_err(|_| OidcError::Capacity)?;
        self.verify_access_inner(token).await
    }
    pub(super) async fn verify_access_inner(
        &self,
        token: &str,
    ) -> Result<VerifiedAccess, OidcError> {
        let invalid = OidcError::InvalidToken;
        let header = token_header(token, self.config.limits.max_token_bytes)?;
        if header.alg != Algorithm::RS256 || header.typ.as_deref() != Some("JWT") {
            return Err(invalid);
        }
        let kid = header
            .kid
            .as_deref()
            .filter(|v| text(v, 128))
            .ok_or(invalid)?;
        self.ensure_ready().await?;
        let key = self.access_key(kid).await?;
        let mut policy = Validation::new(Algorithm::RS256);
        policy.set_issuer(&[&self.config.issuer]);
        policy.set_audience(&[&self.config.api_audience]);
        policy.set_required_spec_claims(&["exp", "iss", "sub", "aud"]);
        policy.leeway = self.config.limits.clock_skew_seconds;
        policy.validate_nbf = true;
        let claims = jsonwebtoken::decode::<UniqueValue>(token, &key.key, &policy)
            .map_err(|_| invalid)?
            .claims
            .0;
        let issuer = claim_text(&claims, "iss", 2048)?;
        let subject = claim_text(&claims, "sub", 512)?;
        let client_id = claim_text(&claims, "azp", 128)?;
        if issuer != self.config.issuer
            || claims.get("typ").and_then(Value::as_str) != Some("Bearer")
            || (client_id != self.config.web_client_id && client_id != self.config.cli_client_id)
        {
            return Err(invalid);
        }
        let audiences = audiences(&claims)?;
        if !audiences.contains(&self.config.api_audience) {
            return Err(invalid);
        }
        let now = chrono::Utc::now().timestamp();
        let exp = numeric_date(&claims, "exp")?;
        let iat = numeric_date(&claims, "iat")?;
        let skew = self.config.limits.clock_skew_seconds as i64;
        if iat > now.saturating_add(skew)
            || exp <= iat
            || exp.saturating_sub(iat) > self.config.limits.max_token_lifetime_seconds as i64
        {
            return Err(invalid);
        }
        if let Some(nbf) = claims.get("nbf") {
            let nbf = nbf.as_i64().filter(|n| *n > 0).ok_or(invalid)?;
            if nbf > now.saturating_add(skew) || nbf >= exp {
                return Err(invalid);
            }
        }
        Ok(VerifiedAccess {
            issuer: issuer.to_owned(),
            subject: subject.to_owned(),
            client_id: client_id.to_owned(),
            expires_at: exp,
            claims,
            provider_id: self.provider_id,
            login_context: None,
        })
    }
    pub(super) async fn access_key(&self, kid: &str) -> Result<super::keys::SigningKey, OidcError> {
        {
            let cache = self
                .cache
                .lock()
                .map_err(|_| OidcError::TransportUnavailable)?;
            if let Some(key) = cache.keys.iter().find(|key| key.kid == kid) {
                return Ok(key.clone());
            }
        }
        let _update = self.update.try_lock().map_err(|_| OidcError::Capacity)?;
        {
            let mut cache = self
                .cache
                .lock()
                .map_err(|_| OidcError::TransportUnavailable)?;
            if let Some(key) = cache.keys.iter().find(|key| key.kid == kid) {
                return Ok(key.clone());
            }
            if cache.unknown_refresh_at.is_some_and(|at| {
                at.elapsed().as_secs() < self.config.limits.unknown_kid_cooldown_seconds
            }) {
                return Err(OidcError::InvalidToken);
            }
            // Rate-limit failures as well as successful refreshes. Never repeat an unknown lookup blindly.
            cache.unknown_refresh_at = Some(Instant::now());
        }
        let (keys, jwks) = self.fetch_keys().await?;
        let found = keys.iter().find(|key| key.kid == kid).cloned();
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| OidcError::TransportUnavailable)?;
        cache.keys = keys;
        cache.keys_at = Some(Instant::now());
        if let Some(metadata) = cache.metadata.take() {
            cache.metadata = Some(metadata.set_jwks(jwks));
        }
        found.ok_or(OidcError::InvalidToken)
    }
}

pub(super) fn token_header(token: &str, maximum: usize) -> Result<Header, OidcError> {
    let invalid = OidcError::InvalidToken;
    if token.len() > maximum || !token.is_ascii() {
        return Err(invalid);
    }
    let mut parts = token.split('.');
    let header = parts
        .next()
        .filter(|v| !v.is_empty() && v.len() <= 2048)
        .ok_or(invalid)?;
    let claims = parts.next().filter(|v| !v.is_empty()).ok_or(invalid)?;
    let signature = parts
        .next()
        .filter(|v| !v.is_empty() && v.len() <= 1366)
        .ok_or(invalid)?;
    if parts.next().is_some()
        || [header, claims, signature].iter().any(|part| {
            !part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
    {
        return Err(invalid);
    }
    let raw = URL_SAFE_NO_PAD.decode(header).map_err(|_| invalid)?;
    let value = super::json::parse(&raw).map_err(|_| invalid)?;
    if value.as_object().is_none_or(|m| {
        m.keys()
            .any(|key| !matches!(key.as_str(), "alg" | "kid" | "typ"))
    }) {
        return Err(invalid);
    }
    // This is a bounded selector only; never follow jku/x5u/jwk or unsupported critical headers.
    serde_json::from_value(value).map_err(|_| invalid)
}
pub(super) fn numeric_date(value: &Value, field: &str) -> Result<i64, OidcError> {
    value
        .get(field)
        .and_then(Value::as_i64)
        .filter(|n| *n > 0)
        .ok_or(OidcError::InvalidToken)
}
pub(super) fn claim_text<'a>(
    value: &'a Value,
    field: &str,
    maximum: usize,
) -> Result<&'a str, OidcError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|v| text(v, maximum))
        .ok_or(OidcError::InvalidToken)
}
pub(super) fn audiences(value: &Value) -> Result<Vec<String>, OidcError> {
    let invalid = OidcError::InvalidToken;
    let audience = value.get("aud").ok_or(invalid)?;
    let values = if let Some(s) = audience.as_str() {
        vec![s]
    } else {
        let array = audience
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= 8)
            .ok_or(invalid)?;
        array
            .iter()
            .map(|v| v.as_str().ok_or(invalid))
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut distinct = BTreeSet::new();
    for value in values {
        if !text(value, 128) || !distinct.insert(value.to_owned()) {
            return Err(invalid);
        }
    }
    Ok(distinct.into_iter().collect())
}
