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
/// Bounds of one protected exchange. A non-200 body stays transient memory.
#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub request: u64,
    pub success: u64,
    pub error: u64,
}
impl Limits {
    fn uniform(limit: u64) -> Self {
        Self {
            request: limit,
            success: limit,
            error: limit,
        }
    }
}
impl Client {
    pub(super) fn new(p: &PreparedPresence) -> Result<Self> {
        Self::build(&p.scope.gateway, p.ca.as_ref(), &p.token)
    }
    /// Protected transport shared by the legacy and owned profiles: exact
    /// origin, HTTPS unless a loopback IP, HTTP/1 only, no proxy, redirect or
    /// automatic retry, bounded timeouts, an optional private CA replacing the
    /// public roots and a sensitive bearer header.
    pub(super) fn build(gateway: &str, ca: Option<&PrivateFile>, token: &str) -> Result<Self> {
        let origin = origin(gateway)?;
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
        if let Some(ca) = ca {
            builder = builder.tls_built_in_root_certs(false);
            for cert in certificates(ca)? {
                builder = builder.add_root_certificate(cert);
            }
        }
        let mut authorization = header::HeaderValue::from_str(&format!("Bearer {token}"))
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
        self.exchange_bounded(p, path, request, RECORD_BYTES).await
    }
    async fn exchange_bounded<T: DeserializeOwned>(
        &self,
        p: &PreparedPresence,
        path: &str,
        request: Option<Vec<u8>>,
        limit: u64,
    ) -> Result<T> {
        let method = if request.is_some() {
            reqwest::Method::POST
        } else {
            reqwest::Method::GET
        };
        if crate::control_transport::control_route(&method, path)
            != Some(crate::control_transport::ControlRouteIdentity::BrokerPresence)
        {
            return Err(PresenceError::Protocol);
        }
        let (status, bytes) = self
            .send_bounded(method, path, request, Limits::uniform(limit))
            .await?;
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
    /// Protected exchange shared by both profiles, after the caller's exact
    /// route classification. It returns the status and the bounded JSON body;
    /// the caller alone interprets them and never keeps a non-200 body.
    pub(super) async fn send_bounded(
        &self,
        method: reqwest::Method,
        path: &str,
        request: Option<Vec<u8>>,
        limits: Limits,
    ) -> Result<(StatusCode, Vec<u8>)> {
        let url = self
            .origin
            .join(&format!("control{path}"))
            .map_err(|_| PresenceError::InvalidConfig)?;
        let mut builder = self
            .http
            .request(method, url)
            .header(header::AUTHORIZATION, self.authorization.clone())
            .header(header::ACCEPT, "application/json")
            .header(header::CACHE_CONTROL, "no-store");
        if let Some(request) = request {
            if request.len() as u64 > limits.request {
                return Err(PresenceError::Protocol);
            }
            builder = builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(request);
        }
        let response = builder.send().await.map_err(|_| PresenceError::Transport)?;
        let status = response.status();
        let limit = if status == StatusCode::OK {
            limits.success
        } else {
            limits.error
        };
        let header_bytes: usize = response
            .headers()
            .iter()
            .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
            .sum();
        if response.headers().len() > 64
            || header_bytes > 32768
            || response.content_length().is_some_and(|n| n > limit)
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
        if status.is_redirection() {
            return Err(PresenceError::Protocol);
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| PresenceError::Transport)?;
            if bytes.len().saturating_add(chunk.len()) > limit as usize {
                return Err(PresenceError::Protocol);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((status, bytes))
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
    pub(super) async fn checkpoint_view(
        &self,
        p: &PreparedPresence,
    ) -> Result<crate::broker_registry::BrokerCheckpointViewV1> {
        self.exchange_bounded(
            p,
            "/api/v1/brokers/checkpoints/self",
            None,
            crate::broker_registry::MAX_BROKER_CHECKPOINT_BYTES as u64,
        )
        .await
    }
    pub(super) async fn checkpoint_post(
        &self,
        p: &PreparedPresence,
        pending: &super::checkpoint_storage::Pending,
    ) -> Result<crate::broker_registry::BrokerCheckpointResultV1> {
        self.exchange_bounded(
            p,
            "/api/v1/brokers/checkpoints",
            Some(serde_json::to_vec(&pending.checkpoint).map_err(|_| PresenceError::Protocol)?),
            crate::broker_registry::MAX_BROKER_CHECKPOINT_BYTES as u64,
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
