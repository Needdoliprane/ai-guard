//! Bounded worker-only HTTP relay client. This does not provision a credential,
//! grant a network destination, or connect a network-disabled runtime.
//!
//! Callers retain their exact send/ACK requests before submission. A failed or
//! lost response may follow a durable mutation: retries are explicit and reuse
//! the same operation ID and content. No method generates an operation ID.
use super::validation::{self, MAX_RECEIPT_BYTES, validate_receipt};
use super::{
    MAX_INBOX_BYTES, RelayAckV1, RelayEndpointV1, RelayInboxV1, RelayReceiptV1, RelaySendV1,
};
use futures_util::StreamExt;
use reqwest::{Method, StatusCode, header};
use serde::{Serialize, de::DeserializeOwned};
use std::{net::IpAddr, path::Path, time::Duration};

/// Closed errors never retain a URL, credential, body or remote error chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayClientError {
    InvalidConfiguration,
    InvalidRequest,
    Authentication,
    Forbidden,
    NotFound,
    Conflict,
    Capacity,
    Unavailable,
    Transport,
    Protocol,
}
impl RelayClientError {
    /// A mutation may already be durable. Preserve and reconcile the same request.
    /// A false value describes rejection, never proof of a remote rollback.
    pub fn outcome_unknown(self) -> bool {
        matches!(self, Self::Unavailable | Self::Transport | Self::Protocol)
    }
}
impl std::fmt::Display for RelayClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidConfiguration => "relay origin, workload credential, endpoint or TLS trust is invalid",
            Self::InvalidRequest => "relay request is invalid, contains the workload credential or exceeds its bound",
            Self::Authentication => "relay workload authentication failed; obtain current worker authority from trusted control",
            Self::Forbidden => "relay exchange is forbidden; verify the directed grant and current worker authority with trusted control",
            Self::NotFound => "relay resource was not found in the authorized scope",
            Self::Conflict => "relay operation conflicts with durable content; preserve the exact request",
            Self::Capacity => "relay or mandatory audit capacity refused the exchange; contact the operator",
            Self::Unavailable => "relay control or mandatory storage is unavailable; outcome is unresolved, retain and explicitly retry only the same operation",
            Self::Transport => "relay connection, TLS, timeout or delivery failed; outcome is unknown, retain and explicitly retry only the same operation",
            Self::Protocol => "relay response is incompatible or unbounded; outcome is unknown, retain and explicitly retry only the same operation",
        })
    }
}
impl std::error::Error for RelayClientError {}
type Result<T> = std::result::Result<T, RelayClientError>;

/// Only send, inbox and recipient ACK are exposed. The server authenticates the
/// workload proof on every request; the expected endpoint is a response check,
/// not a source or tenant selector. There is deliberately no Debug implementation.
pub struct WorkerRelayClient {
    http: reqwest::Client,
    origin: reqwest::Url,
    authorization: header::HeaderValue,
    credential: String,
    endpoint: RelayEndpointV1,
}
impl WorkerRelayClient {
    /// Plain HTTP is accepted only for a literal loopback IP. The caller supplies
    /// the exclusive worker proof through its private workload handoff, never a
    /// runner/SSO token or a complete environment file.
    pub fn new(origin: &str, workload_credential: &str, endpoint: RelayEndpointV1) -> Result<Self> {
        Self::build(origin, workload_credential, endpoint, None)
    }

    /// Explicit public CA roots replace built-in roots for this immutable client.
    /// The existing bounded projected-file policy applies; no private CA key or
    /// insecure TLS option is accepted. This does not authorize a runtime route.
    pub fn with_ca_file(
        origin: &str,
        workload_credential: &str,
        endpoint: RelayEndpointV1,
        ca_file: &Path,
    ) -> Result<Self> {
        Self::build(origin, workload_credential, endpoint, Some(ca_file))
    }

    fn build(
        origin: &str,
        workload_credential: &str,
        endpoint: RelayEndpointV1,
        ca_file: Option<&Path>,
    ) -> Result<Self> {
        let origin = checked_origin(origin)?;
        if endpoint.validate().is_err()
            || !(32..=512).contains(&workload_credential.len())
            || !workload_credential.is_ascii()
            || workload_credential
                .bytes()
                .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
            || origin.as_str().contains(workload_credential)
            || ca_file.is_some() && origin.scheme() != "https"
        {
            return Err(RelayClientError::InvalidConfiguration);
        }
        let mut authorization =
            header::HeaderValue::from_str(&format!("Bearer {workload_credential}"))
                .map_err(|_| RelayClientError::InvalidConfiguration)?;
        authorization.set_sensitive(true);
        let builder = reqwest::Client::builder()
            .https_only(origin.scheme() == "https")
            .http1_only()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(3))
            .read_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(12))
            .pool_max_idle_per_host(0);
        let trust = ca_file
            .map(crate::tls::ClientTrust::from_ca_file)
            .transpose()
            .map_err(|_| RelayClientError::InvalidConfiguration)?
            .unwrap_or_default();
        let http = trust
            .apply_to_client(builder)
            .map_err(|_| RelayClientError::InvalidConfiguration)?
            .build()
            .map_err(|_| RelayClientError::InvalidConfiguration)?;
        Ok(Self {
            http,
            origin,
            authorization,
            credential: workload_credential.to_owned(),
            endpoint,
        })
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let mut url = self.origin.clone();
        url.set_path(path);
        self.http
            .request(method, url)
            .header(header::AUTHORIZATION, self.authorization.clone())
            .header(header::ACCEPT, "application/json")
            .header(header::CACHE_CONTROL, "no-store")
    }

    pub async fn send(&self, input: &RelaySendV1) -> Result<RelayReceiptV1> {
        validation::validate_send(input, Some(&self.credential))?;
        let bytes = self.request_bytes(input)?;
        let response = self
            .request(Method::POST, "/api/v1/relay/messages")
            .header(header::CONTENT_TYPE, "application/json")
            .body(bytes)
            .send()
            .await
            .map_err(|_| RelayClientError::Transport)?;
        let (status, bytes) = response_bytes(response, MAX_RECEIPT_BYTES).await?;
        require_success(status)?;
        let receipt: RelayReceiptV1 = self.decode(&bytes)?;
        validate_receipt(&receipt, &input.operation_id, &input.operation_id)?;
        if !matches!(
            (status, receipt.replayed),
            (StatusCode::CREATED, false) | (StatusCode::OK, true)
        ) {
            return Err(RelayClientError::Protocol);
        }
        Ok(receipt)
    }

    /// Return one bounded page; an error never becomes an empty inbox. The caller
    /// decides whether to advance its retained cursor and whether to acknowledge.
    pub async fn inbox(&self, after: Option<i64>, limit: u32) -> Result<RelayInboxV1> {
        validation::validate_inbox_request(after, limit)?;
        let mut request = self
            .request(Method::GET, "/api/v1/relay/messages")
            .query(&[("limit", limit.to_string())]);
        if let Some(after) = after {
            request = request.query(&[("after", after.to_string())]);
        }
        let response = request
            .send()
            .await
            .map_err(|_| RelayClientError::Transport)?;
        let (status, bytes) = response_bytes(response, MAX_INBOX_BYTES).await?;
        require_success(status)?;
        if status != StatusCode::OK {
            return Err(RelayClientError::Protocol);
        }
        let inbox: RelayInboxV1 = self.decode(&bytes)?;
        validation::validate_inbox(&inbox, &self.endpoint, after, limit)?;
        Ok(inbox)
    }

    /// An ACK records recipient acknowledgement only, never execution or an effect.
    pub async fn ack(&self, input: &RelayAckV1) -> Result<RelayReceiptV1> {
        validation::validate_ack(input)?;
        // This check precedes formatting message_id into the path: a workload
        // proof can itself be a canonical UUID accepted by the wire contract.
        let bytes = self.request_bytes(input)?;
        let response = self
            .request(
                Method::POST,
                &format!("/api/v1/relay/messages/{}/ack", input.message_id),
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(bytes)
            .send()
            .await
            .map_err(|_| RelayClientError::Transport)?;
        let (status, bytes) = response_bytes(response, MAX_RECEIPT_BYTES).await?;
        require_success(status)?;
        if status != StatusCode::OK {
            return Err(RelayClientError::Protocol);
        }
        let receipt: RelayReceiptV1 = self.decode(&bytes)?;
        validate_receipt(&receipt, &input.operation_id, &input.message_id)?;
        Ok(receipt)
    }

    fn request_bytes<T: Serialize>(&self, input: &T) -> Result<Vec<u8>> {
        validation::request_bytes(input, Some(&self.credential))
    }

    fn decode<T: DeserializeOwned + Serialize>(&self, bytes: &[u8]) -> Result<T> {
        validation::decode(bytes, Some(&self.credential))
    }
}

fn checked_origin(value: &str) -> Result<reqwest::Url> {
    if value.len() > 2048
        || value
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace() || matches!(b, b'%' | b'\\'))
    {
        return Err(RelayClientError::InvalidConfiguration);
    }
    let origin = reqwest::Url::parse(value).map_err(|_| RelayClientError::InvalidConfiguration)?;
    let local = origin.host_str().is_some_and(|host| {
        host.trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    // Check the supplied origin too: URL parsing must not normalize an arbitrary
    // path such as /a/.. into the accepted root destination.
    let authority = value.split_once("://").map(|(_, tail)| tail).unwrap_or("");
    let supplied_path = authority.find('/').map(|at| &authority[at..]).unwrap_or("");
    if origin.host_str().is_none()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || !matches!(supplied_path, "" | "/")
        || origin.query().is_some()
        || origin.fragment().is_some()
        || !(origin.scheme() == "https" || origin.scheme() == "http" && local)
    {
        return Err(RelayClientError::InvalidConfiguration);
    }
    Ok(origin)
}

async fn response_bytes(
    response: reqwest::Response,
    maximum: usize,
) -> Result<(StatusCode, Vec<u8>)> {
    let headers = response.headers();
    let header_bytes: usize = headers
        .iter()
        .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
        .sum();
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if headers.len() > 64
        || header_bytes > 32_768
        || response
            .content_length()
            .is_some_and(|length| length > maximum as u64)
        || content_type.split(';').next() != Some("application/json")
        || headers
            .get(header::CONTENT_ENCODING)
            .is_some_and(|value| value != "identity")
        || response.status().is_redirection()
    {
        return Err(RelayClientError::Protocol);
    }
    let status = response.status();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| RelayClientError::Transport)?;
        if bytes.len().saturating_add(chunk.len()) > maximum {
            return Err(RelayClientError::Protocol);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((status, bytes))
}
fn require_success(status: StatusCode) -> Result<()> {
    if status.is_success() {
        return Ok(());
    }
    Err(match status {
        StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE => RelayClientError::InvalidRequest,
        StatusCode::UNAUTHORIZED => RelayClientError::Authentication,
        StatusCode::FORBIDDEN => RelayClientError::Forbidden,
        StatusCode::NOT_FOUND => RelayClientError::NotFound,
        StatusCode::CONFLICT => RelayClientError::Conflict,
        StatusCode::TOO_MANY_REQUESTS => RelayClientError::Capacity,
        StatusCode::SERVICE_UNAVAILABLE => RelayClientError::Unavailable,
        _ => RelayClientError::Protocol,
    })
}
