//! One explicit metadata inspection; no fallback, retry or policy effect.
use super::{AuditClient, E, policy_error};
use crate::control::egress_policy::{
    EgressPolicyInspectionRequestV1, EgressPolicyInspectionV1, RECEIPT_BYTES,
};

impl AuditClient {
    pub async fn egress_inspect_policy(
        &self,
        request: &EgressPolicyInspectionRequestV1,
    ) -> Result<EgressPolicyInspectionV1, E> {
        let bytes = request.canonical_bytes()?;
        if self.token.is_empty() {
            return Err(E::InvalidRequest);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| E::InvalidRequest)?;
        if bytes.windows(self.token.len()).any(|part| part == self.token.as_bytes()) {
            return Err(E::InvalidRequest);
        }
        self.validate_identity_payload(&value).map_err(|_| E::InvalidRequest)?;
        let url = self
            .control_url(reqwest::Method::POST, "/api/v1/egress/policies/inspect")
            .map_err(|_| E::InvalidRequest)?;
        let mut response = self.http
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
            && (response.headers().get(reqwest::header::CONTENT_TYPE)
                != Some(&reqwest::header::HeaderValue::from_static("application/json"))
                || response.headers().get(reqwest::header::CACHE_CONTROL)
                    != Some(&reqwest::header::HeaderValue::from_static("no-store")))
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
        if body.windows(self.token.len()).any(|part| part == self.token.as_bytes()) {
            return Err(E::InvalidResponse);
        }
        let observation = EgressPolicyInspectionV1::from_canonical_bytes(&body)?;
        observation.validate_for(request)?;
        Ok(observation)
    }
}

#[cfg(test)]
mod tests;
