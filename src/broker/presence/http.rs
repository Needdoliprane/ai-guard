use super::*;
use crate::broker_registry::{BrokerIdentityViewV1, BrokerMutationResultV1, BrokerViewV1};
use futures_util::StreamExt;
use reqwest::{StatusCode, header};
use serde::de::DeserializeOwned;
use std::time::Duration;

pub(super) fn certificates(file: &PrivateFile) -> Result<Vec<reqwest::Certificate>> {
    let certs = crate::tls::certificates(file.bytes()).map_err(|_| PresenceError::InvalidConfig)?;
    if certs.is_empty() || certs.len() > 8 {
        return Err(PresenceError::InvalidConfig);
    }
    certs
        .iter()
        .map(|c| {
            reqwest::Certificate::from_der(c.as_ref()).map_err(|_| PresenceError::InvalidConfig)
        })
        .collect()
}
pub(super) struct Client {
    http: reqwest::Client,
    origin: reqwest::Url,
    authorization: header::HeaderValue,
}
impl Client {
    pub(super) fn new(p: &PreparedPresence) -> Result<Self> {
        let origin = origin(&p.scope.gateway)?;
        let mut builder = reqwest::Client::builder()
            .https_only(origin.scheme() == "https")
            .http1_only()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(3))
            .read_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(12))
            .pool_max_idle_per_host(0);
        if let Some(ca) = &p.ca {
            builder = builder.tls_built_in_root_certs(false);
            for cert in certificates(ca)? {
                builder = builder.add_root_certificate(cert);
            }
        }
        let mut authorization = header::HeaderValue::from_str(&format!("Bearer {}", p.token))
            .map_err(|_| PresenceError::InvalidConfig)?;
        authorization.set_sensitive(true);
        Ok(Self {
            http: builder.build().map_err(|_| PresenceError::InvalidConfig)?,
            origin,
            authorization,
        })
    }
    async fn exchange<T: DeserializeOwned>(
        &self,
        p: &PreparedPresence,
        path: &str,
        request: Option<Vec<u8>>,
    ) -> Result<T> {
        let mut url = self.origin.clone();
        url.set_path(path);
        let mut builder = self
            .http
            .request(
                if request.is_some() {
                    reqwest::Method::POST
                } else {
                    reqwest::Method::GET
                },
                url,
            )
            .header(header::AUTHORIZATION, self.authorization.clone())
            .header(header::ACCEPT, "application/json")
            .header(header::CACHE_CONTROL, "no-store");
        if let Some(request) = request {
            if request.len() as u64 > RECORD_BYTES {
                return Err(PresenceError::Protocol);
            }
            builder = builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(request);
        }
        let response = builder.send().await.map_err(|_| PresenceError::Transport)?;
        let header_bytes: usize = response
            .headers()
            .iter()
            .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
            .sum();
        if response.headers().len() > 64
            || header_bytes > 32768
            || response.content_length().is_some_and(|n| n > RECORD_BYTES)
            || response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .split(';')
                .next()
                != Some("application/json")
        {
            return Err(PresenceError::Protocol);
        }
        let status = response.status();
        if status.is_redirection() {
            return Err(PresenceError::Protocol);
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| PresenceError::Transport)?;
            if bytes.len().saturating_add(chunk.len()) > RECORD_BYTES as usize {
                return Err(PresenceError::Protocol);
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| PresenceError::Protocol)?;
        if status != StatusCode::OK {
            // Error bodies remain bounded, transient memory only. Interpret
            // only status and recognized codes into fieldless local errors;
            // never return, persist or display the remote prose. Screening it
            // as success metadata would mistake e.g. "Bearer proof" for a key.
            return Err(remote_error(status, &value));
        }
        p.screen(&value)?;
        // Decode the typed object from original bytes so duplicate fields are
        // rejected too; the Value above is only the decoded secret screen.
        serde_json::from_slice(&bytes).map_err(|_| PresenceError::Protocol)
    }
    pub(super) async fn identity(&self, p: &PreparedPresence) -> Result<BrokerIdentityViewV1> {
        self.exchange(p, "/api/v1/brokers/identity", None).await
    }
    pub(super) async fn view(&self, p: &PreparedPresence) -> Result<BrokerViewV1> {
        self.exchange(p, "/api/v1/brokers/self", None).await
    }
    pub(super) async fn post(
        &self,
        p: &PreparedPresence,
        pending: &storage::Pending,
    ) -> Result<BrokerMutationResultV1> {
        self.exchange(
            p,
            "/api/v1/brokers/heartbeat",
            Some(serde_json::to_vec(&pending.heartbeat).map_err(|_| PresenceError::Protocol)?),
        )
        .await
    }
}
fn remote_error(status: StatusCode, value: &Value) -> PresenceError {
    let code = value
        .get("error")
        .and_then(|v| v.get("code"))
        .and_then(Value::as_str);
    if code == Some("control_clock_uncertain") {
        return PresenceError::Clock;
    }
    match status {
        StatusCode::UNAUTHORIZED => PresenceError::Authentication,
        StatusCode::FORBIDDEN => PresenceError::Forbidden,
        StatusCode::CONFLICT => PresenceError::Conflict,
        StatusCode::TOO_MANY_REQUESTS => PresenceError::Capacity,
        StatusCode::SERVICE_UNAVAILABLE if code.is_none() => PresenceError::Unavailable,
        _ => PresenceError::Protocol,
    }
}
