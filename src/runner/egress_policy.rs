//! One-shot reserved policy administration; no public or legacy fallback.
use super::AuditClient;
use crate::control::egress_policy::{
    EgressPolicyAdminErrorV1 as E, EgressPolicyReceiptV1, EgressReplacePolicyRequestV1,
    EgressRevokePolicyRequestV1, RECEIPT_BYTES,
};

impl AuditClient {
    pub async fn egress_replace_policy(
        &self,
        request: &EgressReplacePolicyRequestV1,
    ) -> Result<EgressPolicyReceiptV1, E> {
        let bytes = request.canonical_bytes()?;
        let receipt = self
            .policy_post("/api/v1/egress/policies/replace", bytes)
            .await?;
        receipt.validate_for_replace(request)?;
        Ok(receipt)
    }
    pub async fn egress_revoke_policy(
        &self,
        request: &EgressRevokePolicyRequestV1,
    ) -> Result<EgressPolicyReceiptV1, E> {
        let bytes = request.canonical_bytes()?;
        let receipt = self
            .policy_post("/api/v1/egress/policies/revoke", bytes)
            .await?;
        receipt.validate_for_revoke(request)?;
        Ok(receipt)
    }
    async fn policy_post(&self, path: &str, bytes: Vec<u8>) -> Result<EgressPolicyReceiptV1, E> {
        if self.token.is_empty() {
            return Err(E::InvalidRequest);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| E::InvalidRequest)?;
        if bytes
            .windows(self.token.len())
            .any(|part| part == self.token.as_bytes())
        {
            return Err(E::InvalidRequest);
        }
        self.validate_identity_payload(&value)
            .map_err(|_| E::InvalidRequest)?;
        let url = self
            .control_url(reqwest::Method::POST, path)
            .map_err(|_| E::InvalidRequest)?;
        let mut response = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::CACHE_CONTROL, "no-store")
            .body(bytes)
            .send()
            .await
            .map_err(|_| E::Transport)?;
        let status = response.status().as_u16();
        if status == 200
            && response.headers().get(reqwest::header::CONTENT_TYPE)
                != Some(&reqwest::header::HeaderValue::from_static(
                    "application/json",
                ))
        {
            return Err(E::InvalidResponse);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| E::Transport)? {
            if chunk.len() > RECEIPT_BYTES - body.len() {
                return Err(E::InvalidResponse);
            }
            body.extend_from_slice(&chunk);
        }
        if status != 200 {
            return Err(policy_error(status, &body));
        }
        if body
            .windows(self.token.len())
            .any(|part| part == self.token.as_bytes())
        {
            return Err(E::InvalidResponse);
        }
        EgressPolicyReceiptV1::from_canonical_bytes(&body)
    }
}

fn policy_error(status: u16, body: &[u8]) -> E {
    // Existing pre-body gates have historical response envelopes. Their status
    // remains opaque, and no arbitrary remote message/chain reaches the caller.
    match status {
        401 => return E::Unauthenticated,
        403 => return E::Forbidden,
        429 => return E::Capacity,
        _ => {}
    }
    let value: serde_json::Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => {
            return if status == 409 {
                E::InvalidResponse
            } else {
                E::Unavailable
            };
        }
    };
    let code = value
        .pointer("/error/code")
        .and_then(serde_json::Value::as_str);
    match (status, code) {
        (400, Some("egress_policy_invalid_request")) => E::InvalidRequest,
        (409, Some("egress_policy_scope_conflict")) => E::ScopeConflict,
        (409, Some("egress_policy_conflict")) => E::Conflict,
        (409, Some("egress_policy_migration_required")) => E::MigrationRequired,
        (503, Some("egress_policy_busy")) => E::Busy,
        (503, Some("egress_policy_clock_unavailable")) => E::Clock,
        (409, _) => E::InvalidResponse,
        _ => E::Unavailable,
    }
}

mod inspection;

#[cfg(test)]
mod tests;
