use super::{
    Authentication, KubernetesAuthorityV1, KubernetesCredentials, KubernetesError, KubernetesResult,
};
use reqwest::{
    Client,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::de::DeserializeOwned;
use std::time::Duration;
use tokio::sync::Semaphore;

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

pub(super) struct Transport {
    client: Client,
    endpoint: String,
    credentials: KubernetesCredentials,
    permits: Semaphore,
}
impl Transport {
    pub(super) fn new(
        binding: &KubernetesAuthorityV1,
        credentials: KubernetesCredentials,
    ) -> KubernetesResult<Self> {
        credentials.revalidate()?;
        if credentials.ca.digest() != binding.trust_sha256 {
            return Err(KubernetesError::InvalidCredentials);
        }
        let certificates = crate::tls::certificates(credentials.ca.bytes())
            .map_err(|_| KubernetesError::InvalidCredentials)?;
        let mut builder = Client::builder()
            .https_only(true)
            .tls_built_in_root_certs(false)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(3))
            .timeout(REQUEST_TIMEOUT)
            .pool_max_idle_per_host(0);
        for certificate in certificates {
            builder = builder.add_root_certificate(
                reqwest::Certificate::from_der(certificate.as_ref())
                    .map_err(|_| KubernetesError::InvalidCredentials)?,
            );
        }
        let mut headers = HeaderMap::new();
        match &credentials.authentication {
            Authentication::Bearer(token) => {
                let bytes = token.bytes();
                if !(32..=8192).contains(&bytes.len())
                    || !bytes
                        .iter()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-._~+/=".contains(byte))
                {
                    return Err(KubernetesError::InvalidCredentials);
                }
                let token =
                    std::str::from_utf8(bytes).map_err(|_| KubernetesError::InvalidCredentials)?;
                let mut header = HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|_| KubernetesError::InvalidCredentials)?;
                header.set_sensitive(true);
                headers.insert(AUTHORIZATION, header);
            }
            Authentication::MutualTls { certificate, key } => {
                // Reuse strict PEM/key matching validation, not a permissive PEM scanner.
                crate::tls::ServerIdentity::from_pem(certificate.bytes(), key.bytes())
                    .map_err(|_| KubernetesError::InvalidCredentials)?;
                let mut pem = certificate.bytes().to_vec();
                pem.push(b'\n');
                pem.extend_from_slice(key.bytes());
                builder = builder.identity(
                    reqwest::Identity::from_pem(&pem)
                        .map_err(|_| KubernetesError::InvalidCredentials)?,
                );
            }
        }
        let client = builder
            .default_headers(headers)
            .build()
            .map_err(|_| KubernetesError::InvalidCredentials)?;
        Ok(Self {
            client,
            endpoint: binding.endpoint.trim_end_matches('/').into(),
            credentials,
            permits: Semaphore::new(4),
        })
    }
    /// All callers use fixed API paths and validated DNS names, never caller URLs.
    pub(super) async fn get<T: DeserializeOwned>(&self, path: &str) -> KubernetesResult<T> {
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| KubernetesError::Busy)?;
        self.credentials.revalidate()?;
        let mut response = self
            .client
            .get(format!("{}{path}", self.endpoint))
            .header("accept", "application/json")
            .send()
            .await
            .map_err(transport_error)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(KubernetesError::ApiRejected(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
        {
            return Err(KubernetesError::ResponseTooLarge);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
            if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
                return Err(KubernetesError::ResponseTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| KubernetesError::InvalidResponse)
    }
}
fn transport_error(error: reqwest::Error) -> KubernetesError {
    if error.is_timeout() {
        KubernetesError::Timeout
    } else {
        KubernetesError::TransportUnavailable
    }
}
