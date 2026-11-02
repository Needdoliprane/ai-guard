use super::{OidcConfigV1, OidcError};
use futures_util::StreamExt;
use openidconnect::{AsyncHttpClient, HttpRequest, HttpResponse};
use std::{future::Future, pin::Pin, time::Duration};
use tokio::sync::Semaphore;

pub(super) struct Transport {
    client: reqwest::Client,
    config: OidcConfigV1,
    permits: Semaphore,
}
impl Transport {
    pub(super) fn new(config: &OidcConfigV1, ca: Option<&[u8]>) -> Result<Self, OidcError> {
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_millis(config.limits.request_timeout_ms))
            .timeout(Duration::from_millis(config.limits.request_timeout_ms))
            .pool_max_idle_per_host(0);
        if let Some(pem) = ca {
            if !matches!(config.transport, super::OidcTransportModeV1::HttpsOnly) {
                return Err(OidcError::InvalidConfiguration);
            }
            let certificates =
                crate::tls::certificates(pem).map_err(|_| OidcError::InvalidConfiguration)?;
            // Explicit roots replace WebPKI for this client alone. Hostname/validity checks remain active.
            builder = builder.tls_built_in_root_certs(false);
            for certificate in certificates {
                builder = builder.add_root_certificate(
                    reqwest::Certificate::from_der(certificate.as_ref())
                        .map_err(|_| OidcError::InvalidConfiguration)?,
                );
            }
        }
        let client = builder
            .build()
            .map_err(|_| OidcError::InvalidConfiguration)?;
        Ok(Self {
            client,
            config: config.clone(),
            permits: Semaphore::new(config.limits.max_inflight),
        })
    }
    pub(super) async fn get(&self, url: &str) -> Result<Vec<u8>, OidcError> {
        let request = axum::http::Request::builder()
            .uri(url)
            .header("accept", "application/json")
            .body(Vec::new())
            .map_err(|_| OidcError::InvalidConfiguration)?;
        let response = self.call(request).await?;
        if response.status() != 200 {
            return Err(OidcError::ProviderRejected);
        }
        Ok(response.into_body())
    }
    pub(super) async fn logout(
        &self,
        endpoint: &str,
        client_id: &str,
        id_token: &str,
    ) -> Result<(), OidcError> {
        let body =
            serde_urlencoded::to_string([("client_id", client_id), ("id_token_hint", id_token)])
                .map_err(|_| OidcError::InvalidLogin)?;
        let request = axum::http::Request::builder()
            .method("POST")
            .uri(endpoint)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("accept", "application/json")
            .body(body.into_bytes())
            .map_err(|_| OidcError::InvalidConfiguration)?;
        let response = self.call(request).await?;
        if response.status() == 204 && response.body().is_empty() {
            return Ok(());
        }
        if response.status() == 200
            && (response.body().is_empty()
                || super::json::parse(response.body())
                    .is_ok_and(|v| v.as_object().is_some_and(|v| v.is_empty())))
        {
            return Ok(());
        }
        // Redirects, interactive HTML and ambiguous bodies are never confirmation of IdP logout.
        Err(OidcError::ProviderRejected)
    }
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, OidcError> {
        let url = request.uri().to_string();
        let logout = request.method() == "POST"
            && self.config.end_session_endpoint.as_deref() == Some(url.as_str());
        let allowed = (request.method() == "GET"
            && (url == self.config.discovery_url() || url == self.config.jwks_uri))
            || (request.method() == "POST" && url == self.config.token_endpoint)
            || logout;
        if !allowed || request.body().len() > self.config.limits.max_response_bytes {
            return Err(OidcError::InvalidMetadata);
        }
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| OidcError::Capacity)?;
        let timeout = Duration::from_millis(self.config.limits.request_timeout_ms);
        tokio::time::timeout(timeout, async {
            let (parts, body) = request.into_parts();
            let response = self
                .client
                .request(parts.method, &url)
                .headers(parts.headers)
                .body(body)
                .send()
                .await
                .map_err(|_| OidcError::TransportUnavailable)?;
            if response
                .content_length()
                .is_some_and(|n| n > self.config.limits.max_response_bytes as u64)
            {
                return Err(OidcError::TransportUnavailable);
            }
            let mut result = axum::http::Response::builder().status(response.status());
            if let Some(headers) = result.headers_mut() {
                *headers = response.headers().clone();
            }
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| OidcError::TransportUnavailable)?;
                if chunk.len()
                    > self
                        .config
                        .limits
                        .max_response_bytes
                        .saturating_sub(bytes.len())
                {
                    return Err(OidcError::TransportUnavailable);
                }
                bytes.extend_from_slice(&chunk);
            }
            // Protocol parsing never gets duplicate-field ambiguity or a text/error body.
            if !(logout && bytes.is_empty())
                && result
                    .headers_ref()
                    .and_then(|h| h.get("content-type"))
                    .and_then(|v| v.to_str().ok())
                    .is_none_or(|v| {
                        v.split(';')
                            .next()
                            .is_none_or(|v| !v.trim().eq_ignore_ascii_case("application/json"))
                    })
            {
                return Err(OidcError::ProviderRejected);
            }
            if !bytes.is_empty() {
                super::json::parse(&bytes).map_err(|_| OidcError::ProviderRejected)?;
            }
            result
                .body(bytes)
                .map_err(|_| OidcError::TransportUnavailable)
        })
        .await
        .map_err(|_| OidcError::TransportUnavailable)?
    }
}
impl<'a> AsyncHttpClient<'a> for Transport {
    type Error = OidcError;
    type Future = Pin<Box<dyn Future<Output = Result<HttpResponse, OidcError>> + Send + 'a>>;
    fn call(&'a self, request: HttpRequest) -> Self::Future {
        Box::pin(self.execute(request))
    }
}
