use super::admission::{journal::AdmissionJournal, permission::FreshKubernetesCreatePermission};
use super::{
    Authentication, BeforeKubernetesPreflightGuard, KubernetesAuthorityV1, KubernetesCredentials,
    KubernetesError, KubernetesPreflightTargetV1, KubernetesResult,
};
use reqwest::{
    Client,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::de::DeserializeOwned;
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use tokio::sync::Semaphore;

#[path = "prerequisites_transport.rs"]
mod prerequisites;
#[path = "session_transport.rs"]
mod session;
pub(super) use session::{CreatedClaimResponseV2, CreatedSessionResponseV2};

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_PREFLIGHT_REQUEST_BYTES: usize = 64 * 1024;
const MAX_CREATE_REQUEST_BYTES: usize = 32 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

pub(super) struct Transport {
    client: Client,
    endpoint: String,
    credentials: KubernetesCredentials,
    permits: Semaphore,
    /// Slots of the V2 preamble-only attach attempts. They are separate from
    /// `permits`, so an open attach never takes a request slot. The field is
    /// read only by the V2 attach route, which has no production caller yet.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "read only by the V2 attach route, which has no production caller (DESIGN section 3.6)"
        )
    )]
    attach_permits: Arc<Semaphore>,
}
/// The outer Result is reserved for refusals before HTTP initiation. Once the
/// consumed guard has admitted initiation, every failure is an uncertain effect.
// Closed held-CREATE transport target; production wiring pending.
#[allow(dead_code)]
pub(super) enum HeldCreateResponse {
    Complete(Vec<u8>),
    Uncertain(KubernetesError),
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
            attach_permits: Arc::new(Semaphore::new(super::session_attach::MAX_CONCURRENT_ATTACH)),
        })
    }
    /// All callers use fixed API paths and validated DNS names, never caller URLs.
    pub(super) async fn get<T: DeserializeOwned>(&self, path: &str) -> KubernetesResult<T> {
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| KubernetesError::Busy)?;
        self.credentials.revalidate()?;
        let response = self
            .client
            .get(format!("{}{path}", self.endpoint))
            .header("accept", "application/json")
            .send()
            .await
            .map_err(transport_error)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(KubernetesError::ApiRejected(response.status().as_u16()));
        }
        read_response(response).await
    }

    /// This method only posts a Pod collection server-side dry-run. The caller
    /// cannot supply a path, query, HTTP method, field manager or raw body.
    pub(super) async fn dry_run_pod<T: DeserializeOwned>(
        &self,
        namespace: &str,
        pod: &serde_json::Value,
        target: &KubernetesPreflightTargetV1,
        guard: Box<dyn BeforeKubernetesPreflightGuard>,
    ) -> KubernetesResult<T> {
        if !super::valid_dns_label(namespace) {
            return Err(KubernetesError::InvalidAuthority);
        }
        let body = serde_json::to_vec(pod).map_err(|_| KubernetesError::InvalidPreflightPlan)?;
        if body.len() > MAX_PREFLIGHT_REQUEST_BYTES {
            return Err(KubernetesError::InvalidPreflightPlan);
        }
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| KubernetesError::Busy)?;
        self.credentials.revalidate()?;
        let request = self
            .client
            .post(format!(
                "{}/api/v1/namespaces/{namespace}/pods?dryRun=All&fieldValidation=Strict",
                self.endpoint
            ))
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .body(body);
        // No await between this consumed durable-control check and send.
        guard.check_before_dry_run(target)?;
        let response = request.send().await.map_err(transport_error)?;
        if !matches!(
            response.status(),
            reqwest::StatusCode::OK | reqwest::StatusCode::CREATED
        ) {
            return Err(KubernetesError::ApiRejected(response.status().as_u16()));
        }
        read_response(response).await
    }

    /// One fixed retained-CREATE route, exact bytes from a nonserialized permit.
    /// Neither a caller-supplied URL/body nor the dry-run guard can reach it.
    // Closed held-CREATE transport target; production wiring pending.
    #[allow(dead_code)]
    pub(super) async fn create_held_pod(
        &self,
        binding: &KubernetesAuthorityV1,
        permission: FreshKubernetesCreatePermission,
        journal: &AdmissionJournal,
        cancelled: &AtomicBool,
    ) -> KubernetesResult<HeldCreateResponse> {
        binding.validate()?;
        if !super::valid_dns_label(binding.namespace())
            || self.endpoint != binding.endpoint.trim_end_matches('/')
        {
            return Err(KubernetesError::HeldCreateAuthorizationDenied);
        }
        let body = permission.pod_bytes().to_vec();
        if body.len() > MAX_CREATE_REQUEST_BYTES {
            return Err(KubernetesError::HeldCreateAuthorizationDenied);
        }
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| KubernetesError::Busy)?;
        self.credentials.revalidate()?;
        let request = self
            .client
            .post(format!(
                "{}/api/v1/namespaces/{}/pods?fieldValidation=Strict",
                self.endpoint,
                binding.namespace()
            ))
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .body(body.clone())
            .build()
            .map_err(|_| KubernetesError::HeldCreateAuthorizationDenied)?;
        // Request construction and all filesystem work precede the last clock
        // sample in this consumed guard. No await or further check intervenes
        // before initiating the already-built request.
        permission.check_before_create(binding, &body, journal, cancelled, &self.credentials)?;
        let response = match self.client.execute(request).await {
            Ok(response) => response,
            Err(error) => return Ok(HeldCreateResponse::Uncertain(transport_error(error))),
        };
        // 201 is this adapter's confirmation policy, not a claim that the
        // Kubernetes API has no other documented successful status codes.
        if response.status() != reqwest::StatusCode::CREATED {
            return Ok(HeldCreateResponse::Uncertain(KubernetesError::ApiRejected(
                response.status().as_u16(),
            )));
        }
        Ok(match read_response_bytes(response).await {
            Ok(bytes) => HeldCreateResponse::Complete(bytes),
            Err(error) => HeldCreateResponse::Uncertain(error),
        })
    }
}

async fn read_response<T: DeserializeOwned>(response: reqwest::Response) -> KubernetesResult<T> {
    let bytes = read_response_bytes(response).await?;
    serde_json::from_slice(&bytes).map_err(|_| KubernetesError::InvalidResponse)
}
async fn read_response_bytes(response: reqwest::Response) -> KubernetesResult<Vec<u8>> {
    read_response_bytes_limited(response, MAX_RESPONSE_BYTES).await
}
async fn read_response_bytes_limited(
    mut response: reqwest::Response,
    maximum: usize,
) -> KubernetesResult<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|size| size > maximum as u64)
    {
        return Err(KubernetesError::ResponseTooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if chunk.len() > maximum.saturating_sub(bytes.len()) {
            return Err(KubernetesError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
fn transport_error(error: reqwest::Error) -> KubernetesError {
    if error.is_timeout() {
        KubernetesError::Timeout
    } else {
        KubernetesError::TransportUnavailable
    }
}
