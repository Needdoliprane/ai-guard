use crate::{
    model::{CommandSpec, DecisionRequest},
    redaction::Redactor,
    runtime::{ContainerPurpose, ContainerSpec, NetworkMode, Runtime, RuntimeKind},
    workspace::{self, Limits, Manifest},
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{IsTerminal, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;
pub mod broker;
mod cold_audit_http;
pub(crate) use cold_audit_http::{ColdAuditBudget, ExactStoredEventReceipt};
pub(crate) mod kubernetes_preflight;
mod managed;
/// DTO projection only; this does not authenticate a worker or authorize exec.
pub(crate) fn relay_worker_inspection(
    worker: &crate::control::WorkerV1,
    observed: &crate::runtime::ContainerObservation,
) -> anyhow::Result<crate::control::RuntimeInspectionV1> {
    managed::inspection(worker, observed)
}
pub(crate) mod relay_admission;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, clap::Args)]
pub struct RunOptions {
    #[arg(long, default_value = ".")]
    pub project: PathBuf,
    #[arg(long, default_value = ".agentsandbox")]
    pub config_dir: PathBuf,
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    pub gateway: String,
    #[arg(long, value_enum, default_value = "docker")]
    pub runtime: RuntimeKind,
    /// Absolute engine program selected by the trusted control operator.
    #[arg(long)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_program: Option<PathBuf>,
    /// Explicit local engine endpoint; otherwise Docker's current context is resolved once.
    #[arg(long)]
    #[serde(default)]
    pub runtime_endpoint: Option<String>,
    #[arg(long, default_value = "agentsandbox-worker:local")]
    pub image: String,
    #[arg(long,default_value="strict",value_parser=["strict","review","observe"])]
    pub profile: String,
    #[arg(long, value_enum, default_value = "none")]
    pub network: NetworkMode,
    #[arg(long)]
    pub allow_rootful_runtime: bool,
    #[arg(long, default_value_t = 300)]
    pub timeout: u64,
    #[arg(long,default_value="generic",value_parser=["generic","codex","claude","claude-code"])]
    pub provider: String,
    /// Resolve an explicitly configured client instead of supplying manual argv.
    #[arg(long, value_parser = ["codex", "claude-code", "auto"])]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    #[arg(long, value_enum)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<crate::provider::AuthPreference>,
    #[arg(long)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_profile: Option<String>,
    #[arg(long)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers_file: Option<PathBuf>,
    #[arg(long)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[arg(long)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[arg(long)]
    pub approve_hash: Option<String>,
    /// Store bounded raw stdout/stderr after secret masking (off by default).
    #[arg(long)]
    pub capture_output: bool,
    /// Connect an authorized worker to a filtered controlling terminal; no transcript.
    #[arg(long, conflicts_with = "capture_output")]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interactive: bool,
    /// Explicit API credentials outside the project; observe + bridge only.
    #[arg(long)]
    pub secret_env_file: Option<PathBuf>,
    #[arg(last=true,num_args=1..)]
    pub argv: Vec<String>,
}
#[derive(Clone)]
pub struct AuditClient {
    base: reqwest::Url,
    token: String,
    http: reqwest::Client,
    redactor: Redactor,
}

// Preserve the existing fifteen-second request timeout. At most three requests
// plus two one-second rate backoffs fit inside this overall bound.
const EVENT_POST_BUDGET: Duration = Duration::from_secs(47);
const EVENT_POST_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const EVENT_POST_ATTEMPTS: usize = 3;
const EVENT_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, PartialEq, Eq)]
enum EventPostPolicy {
    SingleAttempt,
    RetryRateWindow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventPostFailure {
    Unavailable,
    Rejected(reqwest::StatusCode),
    InvalidResponse,
    RateLimitRetryExhausted,
}
impl std::fmt::Display for EventPostFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => f.write_str("gateway event delivery unavailable or timed out"),
            Self::Rejected(status) => write!(f, "gateway rejected the event (HTTP {status})"),
            Self::InvalidResponse => f.write_str("gateway returned an invalid event receipt"),
            Self::RateLimitRetryExhausted => {
                f.write_str("gateway event write rate limit persisted (HTTP 429 Too Many Requests)")
            }
        }
    }
}
impl std::error::Error for EventPostFailure {}
pub enum Authorization {
    Allowed(Value),
    Review {
        request_hash: String,
        decision: Value,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlIdentityError {
    Unauthorized,
    Forbidden,
    Unavailable,
    InvalidResponse,
}
impl std::fmt::Display for ControlIdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unauthorized => "control credential is not authenticated",
            Self::Forbidden => "control credential is not authorized",
            Self::Unavailable => "control identity is unavailable",
            Self::InvalidResponse => "control identity response is incompatible",
        })
    }
}
impl std::error::Error for ControlIdentityError {}

/// Keep error context while explicitly masking typed engine captures before
/// decoding them. Generic RuntimeCommandFailure formatting never exposes its
/// stderr, so the capture boundary must be handled here while it is available.
fn redacted_error(redactor: &Redactor, error: &anyhow::Error) -> String {
    let parts: Vec<_> = error
        .chain()
        .map(|cause| {
            let context = redactor.text(&cause.to_string());
            if let Some(failure) = cause.downcast_ref::<crate::runtime::RuntimeCommandFailure>() {
                let captured = failure.stderr_capture();
                format!(
                    "{context} [stderr_retained_bytes={}, stderr_total_bytes={}, stderr_truncated={}]: {}",
                    captured.bytes.len(),
                    captured.total_bytes,
                    captured.truncated,
                    failure.redacted_stderr(redactor),
                )
            } else {
                context
            }
        })
        .collect();
    let masked = redactor.text(&parts.join(": "));
    // Durable worker reasons reject control characters. Preserve multiline
    // CLI context as readable escapes instead of losing the reason to HTTP400.
    let mut diagnostic = String::with_capacity(masked.len());
    for character in masked.chars() {
        if character.is_control() {
            diagnostic.extend(character.escape_default());
        } else {
            diagnostic.push(character);
        }
    }
    diagnostic
}

impl AuditClient {
    /// Select the trusted transport before building a URL. The closed route
    /// table is checked without URL decoding or normalization; a matching path
    /// still grants no identity or authority at the gateway.
    fn control_url(&self, method: reqwest::Method, path: &str) -> Result<reqwest::Url> {
        let path = if path.starts_with('/') {
            path.to_owned()
        } else {
            format!("/{path}")
        };
        anyhow::ensure!(
            crate::control_transport::control_route(&method, &path)
                == Some(crate::control_transport::ControlRouteIdentity::Runner),
            "operation is outside the trusted runner control transport"
        );
        self.base
            .join(&format!("control{path}"))
            .context("invalid control transport origin")
    }

    /// Read the existing public protocol diagnostic with its existing reader
    /// permissions. This does not select reserved runner transport capacity.
    pub async fn supervision_identity(
        &self,
    ) -> std::result::Result<crate::control::ControlProtocolV1, ControlIdentityError> {
        let url = self
            .base
            .join("api/v1/control/protocol")
            .map_err(|_| ControlIdentityError::InvalidResponse)?;
        self.protocol_identity_at(url).await
    }

    /// Authenticate a trusted runner consumer on the explicit control
    /// transport. A refusal never falls back to the public diagnostic.
    pub async fn control_supervision_identity(
        &self,
    ) -> std::result::Result<crate::control::ControlProtocolV1, ControlIdentityError> {
        let url = self
            .control_url(reqwest::Method::GET, "api/v1/control/protocol")
            .map_err(|_| ControlIdentityError::InvalidResponse)?;
        self.protocol_identity_at(url).await
    }

    /// Both explicit callers retain the same bounded parser and opaque error
    /// classes; neither response data nor an error selects another transport.
    async fn protocol_identity_at(
        &self,
        url: reqwest::Url,
    ) -> std::result::Result<crate::control::ControlProtocolV1, ControlIdentityError> {
        use ControlIdentityError as Error;
        let mut response = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|_| Error::Unavailable)?;
        match response.status() {
            reqwest::StatusCode::UNAUTHORIZED => return Err(Error::Unauthorized),
            reqwest::StatusCode::FORBIDDEN => return Err(Error::Forbidden),
            reqwest::StatusCode::OK => {}
            _ => return Err(Error::Unavailable),
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Unavailable)? {
            if bytes.len().saturating_add(chunk.len()) > 16 * 1024 {
                return Err(Error::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        let mut decoder = serde_json::Deserializer::from_slice(&bytes);
        let mut ignored = false;
        let protocol: crate::control::ControlProtocolV1 =
            serde_ignored::deserialize(&mut decoder, |_| ignored = true)
                .map_err(|_| Error::InvalidResponse)?;
        decoder.end().map_err(|_| Error::InvalidResponse)?;
        if ignored {
            return Err(Error::InvalidResponse);
        }
        protocol.validate().map_err(|_| Error::InvalidResponse)?;
        Ok(protocol)
    }

    fn validate_identity_payload(&self, value: &impl Serialize) -> Result<()> {
        let value = serde_json::to_value(value)?;
        if self.redactor.value(&value) != value {
            bail!(
                "request contains sensitive data in identity or command fields; remove it and use private credential injection; nothing was transmitted"
            );
        }
        Ok(())
    }
    pub fn new(base: &str, token: &str) -> Result<Self> {
        Self::with_gateway_trust(base, token, &crate::tls::ClientTrust::default())
    }

    /// Explicit immutable roots from trusted operator configuration. This
    /// changes only TLS trust for this exact origin, never runner authority.
    pub(crate) fn with_gateway_trust(
        base: &str,
        token: &str,
        trust: &crate::tls::ClientTrust,
    ) -> Result<Self> {
        let base = reqwest::Url::parse(base)?;
        let loopback = base.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if !(base.scheme() == "https" || base.scheme() == "http" && loopback)
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || base.path() != "/"
        {
            bail!(
                "gateway must be an HTTPS origin or a plain HTTP loopback origin, without URL credentials"
            );
        }
        if token.is_empty() || token.chars().any(char::is_whitespace) {
            bail!("invalid runner token");
        }
        anyhow::ensure!(
            trust.digest().is_none() || base.scheme() == "https",
            "explicit gateway CA requires HTTPS"
        );
        let builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy();
        Ok(Self {
            base,
            token: token.into(),
            http: trust.apply_to_client(builder)?.build()?,
            redactor: Redactor::new(vec![token.into()]),
        })
    }
    pub(crate) fn with_secrets(mut self, secrets: Vec<String>) -> Self {
        let mut secrets = secrets;
        secrets.push(self.token.clone());
        self.redactor = Redactor::new(secrets);
        self
    }
    async fn response(&self, request: reqwest::RequestBuilder) -> Result<Value> {
        let mut response = request
            .send()
            .await
            .context("gateway unavailable; no unaudited execution is permitted")?;
        if !response.status().is_success() {
            bail!(
                "gateway rejected the operation (HTTP {})",
                response.status()
            );
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > 512 * 1024 {
                bail!("gateway response is too large");
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).context("gateway returned invalid JSON")
    }
    /// Submit an already durable supervised reservation with this client's
    /// explicit runner credential. Policy/quota refusals have durable receipts
    /// even though their HTTP status is 403/429. Other errors stay unknown to the
    /// caller; this DTO never grants a runtime start or commits a checkpoint.
    pub async fn admit_supervised(
        &self,
        request: &crate::control::SupervisedAdmissionV1,
    ) -> Result<crate::control::AdmissionResultV1> {
        use crate::control::{AdmissionRefusalV1, AdmissionResultV1, WorkerStateV1};
        use reqwest::StatusCode;

        let input_hash = request
            .input_hash()
            .map_err(|_| anyhow::anyhow!("invalid supervised reservation"))?;
        self.validate_identity_payload(request)?;
        let body = serde_json::to_vec(request)
            .map_err(|_| anyhow::anyhow!("invalid supervised reservation"))?;
        anyhow::ensure!(
            body.len() <= 64 * 1024,
            "supervised reservation exceeds its input bound"
        );
        let url = self
            .control_url(
                reqwest::Method::POST,
                "api/v1/workers/supervised-reservations",
            )
            .map_err(|_| anyhow::anyhow!("invalid supervised reservation origin"))?;
        let mut response = self
            .http
            .post(url)
            .bearer_auth(&self.token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("supervised reservation response is unavailable"))?;
        let status = response.status();
        anyhow::ensure!(
            matches!(
                status,
                StatusCode::OK
                    | StatusCode::CREATED
                    | StatusCode::FORBIDDEN
                    | StatusCode::TOO_MANY_REQUESTS
            ),
            "gateway rejected the supervised reservation (HTTP {})",
            status.as_u16()
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("supervised reservation response is incomplete"))?
        {
            anyhow::ensure!(
                bytes.len().saturating_add(chunk.len()) <= 512 * 1024,
                "supervised reservation response exceeds its bound"
            );
            bytes.extend_from_slice(&chunk);
        }
        let mut decoder = serde_json::Deserializer::from_slice(&bytes);
        let mut ignored = false;
        let result: AdmissionResultV1 =
            serde_ignored::deserialize(&mut decoder, |_| ignored = true)
                .map_err(|_| anyhow::anyhow!("invalid supervised reservation response"))?;
        decoder
            .end()
            .map_err(|_| anyhow::anyhow!("invalid supervised reservation response"))?;
        anyhow::ensure!(!ignored, "invalid supervised reservation response");
        let value = serde_json::to_value(&result)
            .map_err(|_| anyhow::anyhow!("invalid supervised reservation response"))?;
        anyhow::ensure!(
            self.redactor.value(&value) == value,
            "sensitive data rejected from supervised reservation response"
        );
        let receipt = &result.receipt;
        anyhow::ensure!(
            receipt.operation_id == request.admission.operation_id
                && receipt.input_hash == input_hash
                && receipt.audit_seq > 0
                && crate::control::hash(&receipt.audit_hash).is_ok(),
            "supervised reservation receipt does not match its request"
        );
        match (&result.worker, result.refusal) {
            (Some(worker), None) => {
                let launch = serde_json::to_value(&worker.launch)
                    .map_err(|_| anyhow::anyhow!("invalid supervised reservation launch"))?;
                let expected_launch = serde_json::to_value(&request.admission.launch)
                    .map_err(|_| anyhow::anyhow!("invalid supervised reservation launch"))?;
                let decision_receipt = serde_json::to_value(&worker.receipt)
                    .map_err(|_| anyhow::anyhow!("invalid supervised reservation receipt"))?;
                let expected_receipt = serde_json::to_value(&request.admission.receipt)
                    .map_err(|_| anyhow::anyhow!("invalid supervised reservation receipt"))?;
                let expected_spec = request
                    .admission
                    .launch
                    .spec_hash()
                    .map_err(|_| anyhow::anyhow!("invalid supervised reservation launch"))?;
                let canonical_uuid = |value: &str| {
                    Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
                };
                let physical_name_matches = worker
                    .attempt
                    .stable_name
                    .strip_prefix("aig-")
                    .and_then(|name| name.split_once('-'))
                    .is_some_and(|(namespace, id)| {
                        namespace.len() == 32
                            && Uuid::parse_str(namespace).is_ok_and(|parsed| {
                                !parsed.is_nil() && parsed.simple().to_string() == namespace
                            })
                            && id == worker.worker_id.replace('-', "")
                            && worker.attempt.volume_name
                                == format!("{}-work", worker.attempt.stable_name)
                    });
                anyhow::ensure!(
                    result.accepted
                        && ((status == StatusCode::CREATED && !result.replayed)
                            || (status == StatusCode::OK && result.replayed))
                        && receipt.worker_id.as_deref() == Some(worker.worker_id.as_str())
                        && receipt.revision == Some(worker.revision)
                        && worker.revision == 1
                        && worker.state == WorkerStateV1::Reserved
                        && worker.quota_held
                        && worker.admission_operation_id == request.admission.operation_id
                        && worker.request_id == request.admission.decision_request.request_id
                        && launch == expected_launch
                        && decision_receipt == expected_receipt
                        && worker.spec_hash == expected_spec
                        && crate::control::hash(&worker.policy_hash).is_ok()
                        && canonical_uuid(&worker.worker_id)
                        && canonical_uuid(&worker.attempt.attempt_id)
                        && physical_name_matches
                        && worker.attempt.container_id.is_none()
                        && worker.attempt.created_inspection.is_none()
                        && worker.attempt.create_operation_id.is_none()
                        && worker.attempt.start_operation_id.is_none()
                        && worker.attempt.start_emitter_id.is_none()
                        && worker.observed_started_at.is_none()
                        && !worker.allocation_inconsistent
                        && worker.last_observation.is_none()
                        && worker.unknown_reason.is_none()
                        && worker.cleanup_removed.is_none()
                        && worker.cleanup_error.is_none(),
                    "supervised reservation status contradicts its result"
                );
            }
            (None, Some(refusal)) => {
                anyhow::ensure!(
                    !result.accepted
                        && receipt.worker_id.is_none()
                        && receipt.revision.is_none()
                        && matches!(
                            (status, refusal),
                            (StatusCode::FORBIDDEN, AdmissionRefusalV1::Policy)
                                | (StatusCode::TOO_MANY_REQUESTS, AdmissionRefusalV1::Quota)
                        ),
                    "supervised reservation status contradicts its refusal"
                );
            }
            _ => bail!("invalid supervised reservation outcome"),
        }
        Ok(result)
    }
    pub async fn get_json(&self, path: &str) -> Result<Value> {
        self.response(
            self.http
                .get(self.base.join(path)?)
                .bearer_auth(&self.token),
        )
        .await
    }
    pub async fn post_event(&self, event: Value) -> Result<i64> {
        self.post_event_to(
            self.base.join("api/events")?,
            event,
            EventPostPolicy::RetryRateWindow,
        )
        .await
    }
    /// Explicit trusted producer API. Neither Event kind, source nor provenance
    /// can select this route from the ordinary worker-derived ingestion path.
    pub(crate) async fn post_control_event(&self, event: Value) -> Result<i64> {
        self.post_event_to(
            self.control_url(reqwest::Method::POST, "api/v1/control/events")?,
            event,
            EventPostPolicy::RetryRateWindow,
        )
        .await
    }
    /// The mandatory start callback must fail on its first refused audit. This
    /// keeps the existing fifteen-second request bound without a rate window.
    pub(crate) async fn post_control_event_once(&self, event: Value) -> Result<i64> {
        self.post_event_to(
            self.control_url(reqwest::Method::POST, "api/v1/control/events")?,
            event,
            EventPostPolicy::SingleAttempt,
        )
        .await
    }
    async fn post_event_to(
        &self,
        url: reqwest::Url,
        event: Value,
        policy: EventPostPolicy,
    ) -> Result<i64> {
        let event = self.redactor.value(&event);
        // Freeze the entire body once, including UUID and timestamp. A rate
        // refusal occurs before append; retries must retain this exact input.
        let body = serde_json::to_vec(&event)?;
        if body.len() > 120 * 1024 {
            bail!("event exceeds the safe audit payload limit");
        }
        let (attempts, budget) = match policy {
            EventPostPolicy::SingleAttempt => (1, EVENT_POST_REQUEST_TIMEOUT),
            EventPostPolicy::RetryRateWindow => (EVENT_POST_ATTEMPTS, EVENT_POST_BUDGET),
        };
        let deadline = tokio::time::Instant::now() + budget;
        for attempt in 0..attempts {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(EventPostFailure::Unavailable.into());
            }
            let mut response = self
                .http
                .post(url.clone())
                .bearer_auth(&self.token)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.clone())
                .timeout(remaining.min(EVENT_POST_REQUEST_TIMEOUT))
                .send()
                .await
                .map_err(|_| EventPostFailure::Unavailable)?;
            let status = response.status();
            if !status.is_success() && status != reqwest::StatusCode::TOO_MANY_REQUESTS {
                // In particular, an old gateway's 404 is never followed by an
                // ingestion fallback. Auth, quota and audit errors are terminal.
                return Err(EventPostFailure::Rejected(status).into());
            }
            let mut retry_headers = response
                .headers()
                .get_all(reqwest::header::RETRY_AFTER)
                .iter();
            let retry_after_one_second = retry_headers.next().is_some_and(|value| value == "1")
                && retry_headers.next().is_none();
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| EventPostFailure::Unavailable)?
            {
                if bytes.len().saturating_add(chunk.len()) > 512 * 1024 {
                    return Err(EventPostFailure::InvalidResponse.into());
                }
                bytes.extend_from_slice(&chunk);
            }
            let answer: Value =
                serde_json::from_slice(&bytes).map_err(|_| EventPostFailure::InvalidResponse)?;
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                if policy == EventPostPolicy::SingleAttempt {
                    return Err(EventPostFailure::Rejected(status).into());
                }
                let retryable = retry_after_one_second
                    && answer.pointer("/error/code").and_then(Value::as_str)
                        == Some("write_rate_limited");
                if !retryable {
                    return Err(EventPostFailure::Rejected(status).into());
                }
                if attempt + 1 == attempts
                    || deadline.saturating_duration_since(tokio::time::Instant::now())
                        <= EVENT_RETRY_DELAY
                {
                    return Err(EventPostFailure::RateLimitRetryExhausted.into());
                }
                tokio::time::sleep(EVENT_RETRY_DELAY).await;
                continue;
            }
            return answer
                .get("seq")
                .and_then(Value::as_i64)
                .filter(|seq| *seq > 0)
                .ok_or_else(|| EventPostFailure::InvalidResponse.into());
        }
        Err(EventPostFailure::RateLimitRetryExhausted.into())
    }
    async fn decide(&self, request: &DecisionRequest) -> Result<Value> {
        self.validate_identity_payload(request)?;
        let answer = self
            .response(
                self.http
                    .post(self.control_url(reqwest::Method::POST, "api/decide")?)
                    .bearer_auth(&self.token)
                    .json(request),
            )
            .await?;
        if answer
            .get("audit_seq")
            .and_then(Value::as_i64)
            .filter(|n| *n > 0)
            .is_none()
        {
            bail!("decision has no durable audit acknowledgement");
        }
        if answer.get("request_hash").and_then(Value::as_str)
            != Some(crate::policy::request_hash(request).as_str())
        {
            bail!("decision is not bound to the submitted request");
        }
        Ok(answer)
    }
    pub async fn authorize(
        &self,
        request: &DecisionRequest,
        approval: Option<&str>,
    ) -> Result<Authorization> {
        let mut initial = request.clone();
        initial.human_approved = false;
        initial.approval_hash = None;
        let answer = self.decide(&initial).await?;
        match answer.pointer("/decision/outcome").and_then(Value::as_str) {
            Some("allow") => Ok(Authorization::Allowed(answer["decision"].clone())),
            Some("deny") => bail!(
                "policy denied the command: {}",
                answer
                    .pointer("/decision/reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unspecified reason")
            ),
            Some("review") => {
                let hash = answer["request_hash"]
                    .as_str()
                    .context("missing request hash")?;
                let Some(approval) = approval else {
                    return Ok(Authorization::Review {
                        request_hash: hash.into(),
                        decision: answer["decision"].clone(),
                    });
                };
                if approval != hash {
                    bail!("approval hash does not match this exact request");
                }
                let mut approved = initial;
                approved.human_approved = true;
                approved.approval_hash = Some(approval.into());
                let reviewed = self.decide(&approved).await?;
                if reviewed
                    .pointer("/decision/outcome")
                    .and_then(Value::as_str)
                    != Some("allow")
                {
                    bail!("reviewed request was not allowed; no execution performed");
                }
                Ok(Authorization::Allowed(reviewed["decision"].clone()))
            }
            _ => bail!("unknown policy outcome; refusing execution"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunPlan {
    #[serde(default = "legacy_plan_version")]
    version: u32,
    #[serde(default)]
    operation_id: Uuid,
    options: RunOptions,
    request: DecisionRequest,
    session: Uuid,
    #[serde(default)]
    managed: Option<managed::ManagedCheckpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_input: Option<RunOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_credential: Option<FrozenProviderCredentialV1>,
    #[serde(skip)]
    credential_source: Option<crate::control_fs::PrivateFile>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenProviderCredentialV1 {
    binding_id: Uuid,
    file_name: String,
    digest: String,
}

// This record is stored only in a 0600 file below the retained control root.
// Its contents and private path must never enter the request or public events.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredProviderCredentialV1 {
    version: u32,
    binding_id: Uuid,
    env_bytes: Vec<u8>,
    secrets: Vec<String>,
}
fn legacy_plan_version() -> u32 {
    1
}

/// Private recovery data. Storing it never grants permission to start a worker.
#[derive(Clone, Serialize)]
#[serde(transparent)]
pub struct CheckpointPayloadV1(CheckpointDataV1);
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointDataV1 {
    version: u32,
    current_request: DecisionRequest,
    managed: Option<managed::ManagedCheckpoint>,
}
/// Borrowed data for pure broker validation; never an execution capability.
pub(crate) struct BrokerRunnerContext<'a> {
    pub request: &'a DecisionRequest,
    pub admission: Option<&'a crate::control::AdmissionV1>,
    pub worker: Option<&'a crate::control::WorkerV1>,
    pub binding: Option<&'a crate::runtime::RuntimeAuthority>,
    pub owned: Option<&'a crate::runtime::OwnedContainerSnapshot>,
    pub managed: bool,
    pub legacy_pending: bool,
    pub legacy_import: bool,
    pub tenant_id: Option<&'a str>,
    pub gateway_origin: Option<&'a str>,
}

/// Pure projection of already retained data; this performs no runtime call and
/// supplies no independent observation or permission.
pub(crate) fn broker_observation_data(
    worker: &crate::control::WorkerV1,
    observation: &crate::runtime::ContainerObservation,
) -> Result<crate::control::RuntimeObservationV1> {
    managed::observation_dto(worker, observation)
}
impl std::fmt::Debug for CheckpointPayloadV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CheckpointPayloadV1(<private>)")
    }
}
impl<'de> Deserialize<'de> for CheckpointPayloadV1 {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        use serde::de::Error as _;
        let mut ignored = false;
        let data: CheckpointDataV1 = serde_ignored::deserialize(deserializer, |_| ignored = true)
            .map_err(|_| D::Error::custom("invalid private checkpoint"))?;
        if ignored {
            return Err(D::Error::custom("invalid private checkpoint"));
        }
        let checkpoint = Self(data);
        checkpoint
            .canonical_bytes()
            .map_err(|_| D::Error::custom("invalid private checkpoint"))?;
        Ok(checkpoint)
    }
}
impl CheckpointPayloadV1 {
    pub(crate) fn broker_context(&self) -> BrokerRunnerContext<'_> {
        let managed = self.0.managed.as_ref();
        BrokerRunnerContext {
            request: &self.0.current_request,
            admission: managed.map(|value| &value.admission),
            worker: managed.and_then(|value| value.worker.as_ref()),
            binding: managed.and_then(|value| value.binding.as_ref()),
            owned: managed.and_then(|value| value.owned.as_ref()),
            managed: managed.is_some(),
            legacy_pending: managed
                .is_some_and(|value| value.pending.is_some() || !value.uncertain.is_empty()),
            legacy_import: managed.is_some_and(|value| value.import.is_some()),
            tenant_id: managed.map(|value| value.tenant_id.as_str()),
            gateway_origin: managed.map(|value| value.gateway_origin.as_str()),
        }
    }
    pub fn initial(
        intent: &crate::broker::intent::ExecutionIntentV1,
    ) -> crate::broker::journal::JournalResult<Self> {
        use crate::broker::journal::JournalError;
        intent.validate().map_err(|_| JournalError::InvalidInput)?;
        Ok(Self(CheckpointDataV1 {
            version: 1,
            current_request: intent.request_data().clone(),
            managed: None,
        }))
    }
    pub fn from_plan_bytes(
        intent: &crate::broker::intent::ExecutionIntentV1,
        bytes: &[u8],
    ) -> crate::broker::journal::JournalResult<Self> {
        use crate::broker::{intent::InitialOptionsV1, journal::JournalError};
        if bytes.len() > 32 * 1024 * 1024 {
            return Err(JournalError::TooLarge);
        }
        intent.validate().map_err(|_| JournalError::InvalidInput)?;
        let mut decoder = serde_json::Deserializer::from_slice(bytes);
        let mut ignored = false;
        let plan: RunPlan = serde_ignored::deserialize(&mut decoder, |_| ignored = true)
            .map_err(|_| JournalError::InvalidInput)?;
        decoder.end().map_err(|_| JournalError::InvalidInput)?;
        if ignored {
            return Err(JournalError::InvalidInput);
        }
        let options = InitialOptionsV1::from(plan.options.clone());
        let provider_input = plan.provider_input.clone().map(InitialOptionsV1::from);
        let credential = plan.provider_credential.as_ref().map(|value| {
            (
                value.binding_id,
                value.file_name.as_str(),
                value.digest.as_str(),
            )
        });
        let expected_credential = intent
            .provider_credential()
            .map(|value| (value.binding_id(), value.file_name(), value.digest()));
        if plan.version != intent.plan_version()
            || plan.operation_id != intent.operation_id()
            || plan.session != intent.session()
            || &options != intent.initial_options()
            || provider_input.as_ref() != intent.provider_input()
            || credential != expected_credential
        {
            return Err(JournalError::Conflict);
        }
        let result = Self(CheckpointDataV1 {
            version: 1,
            current_request: plan.request,
            managed: plan.managed,
        });
        result.validate_request(intent)?;
        result.canonical_bytes()?;
        Ok(result)
    }
    pub fn from_bytes(bytes: &[u8]) -> crate::broker::journal::JournalResult<Self> {
        use crate::broker::journal::JournalError;
        if bytes.len() > 32 * 1024 * 1024 {
            return Err(JournalError::TooLarge);
        }
        let result: Self = serde_json::from_slice(bytes).map_err(|_| JournalError::Corrupt)?;
        result.canonical_bytes()?;
        Ok(result)
    }
    pub fn canonical_bytes(&self) -> crate::broker::journal::JournalResult<Vec<u8>> {
        use crate::broker::journal::JournalError;
        if self.0.version != 1 {
            return Err(JournalError::UnsupportedVersion);
        }
        self.validate_versions()?;
        let request = &self.0.current_request;
        if (request.human_approved
            && !request.approval_hash.as_deref().is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            }))
            || (!request.human_approved && request.approval_hash.is_some())
        {
            return Err(JournalError::Conflict);
        }
        let bytes = crate::broker::journal::canonical(self)?;
        if bytes.len() > 32 * 1024 * 1024 {
            return Err(JournalError::TooLarge);
        }
        Ok(bytes)
    }
    fn validate_versions(&self) -> crate::broker::journal::JournalResult<()> {
        use crate::broker::journal::JournalError;
        let Some(checkpoint) = &self.0.managed else {
            return Ok(());
        };
        if checkpoint
            .pending
            .iter()
            .chain(&checkpoint.uncertain)
            .any(|mutation| mutation.version != 1)
            || checkpoint
                .workspace_control_exclusion_version
                .is_some_and(|version| version != 1)
            || checkpoint
                .binding
                .as_ref()
                .and_then(|binding| binding.invocation.as_ref())
                .is_some_and(|invocation| !matches!(invocation.policy_version, 1 | 2))
        {
            return Err(JournalError::UnsupportedVersion);
        }
        for owned in checkpoint.owned.iter().chain(
            checkpoint
                .import
                .iter()
                .filter_map(|import| import.owned.as_ref()),
        ) {
            if !matches!(owned.handle.fingerprint_version(), 1..=3)
                || !matches!(
                    owned
                        .observation
                        .configuration_fingerprint_version
                        .unwrap_or(1),
                    1..=3
                )
            {
                return Err(JournalError::UnsupportedVersion);
            }
        }
        Ok(())
    }
    fn validate_request(
        &self,
        intent: &crate::broker::intent::ExecutionIntentV1,
    ) -> crate::broker::journal::JournalResult<()> {
        use crate::broker::journal::JournalError;
        let request = &self.0.current_request;
        let expected = crate::policy::request_hash(intent.request_data());
        if (request.human_approved && request.approval_hash.as_deref() != Some(expected.as_str()))
            || (!request.human_approved && request.approval_hash.is_some())
        {
            return Err(JournalError::Conflict);
        }
        let mut initial = request.clone();
        initial.human_approved = false;
        initial.approval_hash = None;
        if serde_json::to_value(initial).map_err(|_| JournalError::Corrupt)?
            != serde_json::to_value(intent.request_data()).map_err(|_| JournalError::Corrupt)?
        {
            return Err(JournalError::Conflict);
        }
        Ok(())
    }
    pub(crate) fn validate_journal_context(
        &self,
        intent: &crate::broker::intent::ExecutionIntentV1,
        tenant: &str,
        origin: &str,
        binding_id: Option<Uuid>,
        names: &[String],
        handoff_sha: Option<&str>,
    ) -> crate::broker::journal::JournalResult<()> {
        use crate::{broker::journal::JournalError, control::*};
        self.canonical_bytes()?;
        self.validate_request(intent)?;
        let Some(checkpoint) = &self.0.managed else {
            return Ok(());
        };
        let options = intent.initial_options().run_options();
        let launch = &checkpoint.admission.launch;
        let kind = match options.runtime {
            RuntimeKind::Docker => RuntimeKindV1::Docker,
            RuntimeKind::Podman => RuntimeKindV1::Podman,
        };
        let network = match options.network {
            NetworkMode::None => NetworkV1::None,
            NetworkMode::Bridge => NetworkV1::Bridge,
        };
        checkpoint
            .admission
            .validate()
            .map_err(|_| JournalError::Conflict)?;
        let same_origin = reqwest::Url::parse(origin)
            .ok()
            .zip(reqwest::Url::parse(&checkpoint.gateway_origin).ok())
            .is_some_and(|(left, right)| left == right);
        if checkpoint.tenant_id != tenant
            || !same_origin
            || checkpoint.config_root != options.config_dir
            || checkpoint.admission.operation_id != intent.operation_id().to_string()
            || launch.session_id != intent.session().to_string()
            || launch.command.argv != options.argv
            || launch.command.cwd != "/workspace"
            || launch.profile != options.profile
            || launch.runtime_kind != kind
            || launch.network != network
            || u64::from(launch.timeout_seconds) != options.timeout
            || launch.image_ref != options.image
            || launch.provider != options.provider
            || launch.capture_output != options.capture_output
            || launch.security_preset.is_interactive() != options.interactive
            || launch.credential_binding_id != binding_id.map(|id| id.to_string())
            || launch.credential_names != names
            || checkpoint.credential_digest.as_deref() != handoff_sha
            || content_hash("ai-guard:workspace-manifest:v1", &checkpoint.before)
                .map_err(|_| JournalError::Corrupt)?
                != launch.workspace_manifest_digest
            || serde_json::to_value(&checkpoint.admission.decision_request)
                .map_err(|_| JournalError::Corrupt)?
                != serde_json::to_value(&self.0.current_request)
                    .map_err(|_| JournalError::Corrupt)?
        {
            return Err(JournalError::Conflict);
        }
        if let Some(binding) = &checkpoint.binding
            && (binding.authority_id.to_string() != launch.authority_id
                || binding.kind != options.runtime)
        {
            return Err(JournalError::Conflict);
        }
        if let Some(worker) = &checkpoint.worker {
            if worker.admission_operation_id != checkpoint.admission.operation_id
                || worker.request_id != checkpoint.admission.decision_request.request_id
                || serde_json::to_value(&worker.receipt).map_err(|_| JournalError::Corrupt)?
                    != serde_json::to_value(&checkpoint.admission.receipt)
                        .map_err(|_| JournalError::Corrupt)?
                || worker.spec_hash != launch.spec_hash().map_err(|_| JournalError::Corrupt)?
                || worker
                    .launch
                    .spec_hash()
                    .map_err(|_| JournalError::Corrupt)?
                    != worker.spec_hash
            {
                return Err(JournalError::Conflict);
            }
            let worker_id =
                Uuid::parse_str(&worker.worker_id).map_err(|_| JournalError::Conflict)?;
            let attempt_id =
                Uuid::parse_str(&worker.attempt.attempt_id).map_err(|_| JournalError::Conflict)?;
            if worker_id.is_nil()
                || attempt_id.is_nil()
                || !worker.attempt.stable_name.starts_with("aig-")
                || !worker
                    .attempt
                    .stable_name
                    .ends_with(&format!("-{}", worker_id.simple()))
                || worker.attempt.volume_name != format!("{}-work", worker.attempt.stable_name)
            {
                return Err(JournalError::Conflict);
            }
            for mutation in checkpoint.pending.iter().chain(checkpoint.uncertain.iter()) {
                if mutation.worker_id != worker.worker_id
                    || mutation.attempt_id != worker.attempt.attempt_id
                {
                    return Err(JournalError::Conflict);
                }
            }
            if let Some(owned) = &checkpoint.owned
                && (owned.handle.operation_id() != intent.operation_id()
                    || owned.handle.session_id() != intent.session()
                    || owned.handle.name() != worker.attempt.stable_name
                    || owned.observation.name != owned.handle.name()
                    || owned.observation.id.as_deref() != Some(owned.handle.id())
                    || worker
                        .attempt
                        .container_id
                        .as_deref()
                        .is_some_and(|id| id != owned.handle.id())
                    || checkpoint.binding.as_ref().is_none_or(|binding| {
                        owned.handle.authority_fingerprint() != binding.fingerprint()
                    }))
            {
                return Err(JournalError::Conflict);
            }
        } else if checkpoint.owned.is_some()
            || checkpoint.pending.is_some()
            || !checkpoint.uncertain.is_empty()
        {
            return Err(JournalError::Conflict);
        }
        if let Some(import) = &checkpoint.import {
            if import.operation_id.is_nil() || (import.completed && !import.start_issued) {
                return Err(JournalError::Conflict);
            }
            if let Some(owned) = &import.owned
                && (owned.handle.operation_id() != import.operation_id
                    || owned.handle.session_id() != intent.session()
                    || owned.observation.name != owned.handle.name()
                    || owned.observation.id.as_deref() != Some(owned.handle.id())
                    || checkpoint.binding.as_ref().is_none_or(|binding| {
                        owned.handle.authority_fingerprint() != binding.fingerprint()
                    }))
            {
                return Err(JournalError::Conflict);
            }
        }
        Ok(())
    }
}
pub(crate) fn execution_intent_from_plan_bytes(
    bytes: &[u8],
) -> crate::broker::intent::IntentResult<crate::broker::intent::ExecutionIntentV1> {
    use crate::broker::intent::{ExecutionIntentV1, IntentError};
    let plan: RunPlan = serde_json::from_slice(bytes).map_err(|_| IntentError::InvalidPlan)?;
    ExecutionIntentV1::from_projection(
        plan.version,
        plan.operation_id,
        plan.session,
        plan.options,
        plan.request,
        plan.provider_input,
        plan.provider_credential
            .map(|reference| (reference.binding_id, reference.file_name, reference.digest)),
    )
}

pub(crate) fn validate_execution_intent_provider(
    intent: &crate::broker::intent::ExecutionIntentV1,
) -> crate::broker::intent::IntentResult<()> {
    let plan = RunPlan {
        version: intent.plan_version(),
        operation_id: intent.operation_id(),
        session: intent.session(),
        options: intent.initial_options().run_options(),
        request: intent.request_data().clone(),
        managed: None,
        provider_input: intent.provider_input().map(|input| input.run_options()),
        provider_credential: intent.provider_credential().map(|reference| {
            FrozenProviderCredentialV1 {
                binding_id: reference.binding_id(),
                file_name: reference.file_name().into(),
                digest: reference.digest().into(),
            }
        }),
        credential_source: None,
    };
    validate_provider_plan(&plan).map_err(|_| crate::broker::intent::IntentError::Incoherent)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunSummary {
    pub session_id: Uuid,
    pub volume: String,
    pub exit_code: i64,
    pub timed_out: bool,
    pub changed_files: Option<usize>,
    pub output_captured: bool,
    pub cleanup: CleanupSummary,
    pub postprocessing_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_stop_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CleanupState {
    Removed,
    Unconfirmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupErrorAudit {
    NotApplicable,
    Acknowledged,
    Unacknowledged,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CleanupSummary {
    pub container: String,
    pub state: CleanupState,
    pub error: Option<String>,
    pub error_audit: CleanupErrorAudit,
}

pub fn read_token(path: &Path) -> Result<String> {
    let path = crate::control_fs::absolute_path(path)?;
    let file = crate::control_fs::PrivateFile::open(&path, 4096).with_context(|| {
        format!(
            "cannot read {}; run agentsandbox init first",
            path.display()
        )
    })?;
    let token = std::str::from_utf8(file.bytes())?.trim();
    if token.len() < 32 || token.len() > 4096 || token.chars().any(char::is_whitespace) {
        bail!("token file is empty or malformed");
    }
    Ok(token.into())
}

fn write_json_new(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = crate::control_fs::Directory::open_existing(&crate::control_fs::absolute_path(
        path.parent().context("checkpoint has no parent")?,
    )?)?;
    parent.revalidate()?;
    let path = parent
        .path()
        .join(path.file_name().context("checkpoint has no filename")?);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}
fn read_plan(path: &Path) -> Result<RunPlan> {
    let path = crate::control_fs::absolute_path(path)?;
    let parent = crate::control_fs::Directory::open_existing(
        path.parent().context("session record has no parent")?,
    )?;
    parent.revalidate()?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options.open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        anyhow::ensure!(
            metadata.uid() == unsafe { libc::geteuid() }
                && metadata.nlink() == 1
                && metadata.mode() & 0o7777 == 0o600,
            "session record must be a private owned regular file without hard links"
        );
    }
    if !file.metadata()?.is_file() || file.metadata()?.len() > 32 * 1024 * 1024 {
        bail!("invalid session record");
    }
    Ok(serde_json::from_reader(file)?)
}
fn approval_path(config: &Path, hash: &str) -> Result<PathBuf> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        bail!("approval hash must be the exact 64-character lowercase digest");
    }
    Ok(config.join("pending").join(format!("{hash}.json")))
}
struct ChosenPlan {
    plan: RunPlan,
    config: crate::control_fs::Directory,
    provider_record: Option<crate::control_fs::PrivateFile>,
}

enum PlanPreparation<'a> {
    Legacy,
    Broker {
        operation_id: Uuid,
        control_literals: &'a [String],
    },
}

fn choose_plan(options: RunOptions) -> Result<RunPlan> {
    Ok(choose_plan_prepared(options, PlanPreparation::Legacy)?.plan)
}

fn choose_plan_prepared(
    mut options: RunOptions,
    preparation: PlanPreparation<'_>,
) -> Result<ChosenPlan> {
    let managed_selection = options.client.is_some()
        || options.auth_mode.is_some()
        || options.client_profile.is_some()
        || options.providers_file.is_some()
        || options.prompt.is_some()
        || options.model.is_some();
    anyhow::ensure!(
        !managed_selection || options.argv.is_empty(),
        "managed client selection conflicts with manual argv; choose one invocation mode"
    );
    anyhow::ensure!(
        !managed_selection || (options.provider == "generic" && options.secret_env_file.is_none()),
        "managed client selection conflicts with --provider or --secret-env-file; configure a private provider profile"
    );
    anyhow::ensure!(
        managed_selection || !options.argv.is_empty(),
        "manual argv is required unless --client or a managed provider selection is supplied"
    );
    validate_interactive_options(&options)?;
    if fs::symlink_metadata(&options.project)?
        .file_type()
        .is_symlink()
    {
        bail!("project root must not be a symlink");
    }
    options.project = options.project.canonicalize()?;
    let config = crate::control_fs::Directory::open_existing(&crate::control_fs::absolute_path(
        &options.config_dir,
    )?)
    .context("configuration is missing or unsafe; run agentsandbox init in a private location")?;
    options.config_dir = config.path().to_path_buf();
    if let Some(file) = &options.providers_file {
        options.providers_file = Some(crate::control_fs::absolute_path(file)?);
    }
    if let Some(file) = &options.secret_env_file {
        anyhow::ensure!(
            !fs::symlink_metadata(file)?.file_type().is_symlink(),
            "credential source must not be a symlink"
        );
        options.secret_env_file = Some(crate::control_fs::absolute_path(file)?);
    }
    crate::runtime::validate_image(&options.image)?;
    if options.timeout == 0 || options.timeout > 86400 {
        bail!("timeout must be between 1 and 86400 seconds");
    }
    if !["strict", "review", "observe"].contains(&options.profile.as_str()) {
        bail!("unknown policy profile");
    }
    if options.network != NetworkMode::None && options.profile != "observe" {
        bail!("network bridge is available only with the explicit observe profile");
    }
    // Approval consumes the original frozen choice. It never rereads config or
    // resolves Auto again, even if a provider profile has changed since review.
    if managed_selection && let Some(hash) = options.approve_hash.take() {
        let pending = read_plan(&approval_path(&options.config_dir, &hash)?).context(
            "no pending request matches this approval hash; rerun the original request first",
        )?;
        anyhow::ensure!(
            pending.version == 3
                && pending.provider_input.as_ref() == Some(&options)
                && crate::policy::request_hash(&pending.request) == hash,
            "pending approval does not match these managed client, project, network, image, or runtime options"
        );
        validate_provider_plan(&pending)?;
        return Ok(ChosenPlan {
            plan: pending,
            config,
            provider_record: None,
        });
    }
    let provider_input = managed_selection.then(|| options.clone());
    let mut provider_resolution = None;
    let mut provider_credential = None;
    let mut provider_record = None;
    if managed_selection {
        let providers_file = options
            .providers_file
            .clone()
            .unwrap_or_else(|| options.config_dir.join("providers.json"));
        let providers_source = crate::control_fs::PrivateFile::open(&providers_file, 64 * 1024).context(
            "provider configuration is missing or unsafe; run agentsandbox providers init --env-file PATH, then providers status",
        )?;
        anyhow::ensure!(
            !providers_source.is_within_directory(&options.project)?
                || providers_source.is_within_directory(config.path())?,
            "a private provider configuration inside the project must be within the excluded control directory"
        );
        let providers = crate::provider::load_config_source(&providers_source)?;
        providers_source.revalidate()?;
        anyhow::ensure!(
            options.profile == "observe" && options.network == NetworkMode::Bridge,
            "provider credentials require explicit --profile observe --network bridge; no policy profile or network was changed"
        );
        let client = match options.client.as_deref() {
            None | Some("auto") => None,
            Some("codex") => Some(crate::provider::ClientKind::Codex),
            Some("claude-code") => Some(crate::provider::ClientKind::ClaudeCode),
            Some(_) => bail!("unknown client selection"),
        };
        let prepared = crate::provider::prepare(
            &providers,
            &crate::provider::ProviderSelection {
                client,
                auth_mode: options.auth_mode.unwrap_or_default(),
                profile_id: options.client_profile.clone(),
                transport: if options.interactive {
                    crate::provider::ClientTransport::InteractiveTty
                } else {
                    crate::provider::ClientTransport::Batch
                },
                model: options.model.clone(),
            },
            &options.project,
            options.prompt.as_deref(),
            Uuid::new_v4(),
        )?;
        prepared.credential.validate_source()?;
        let source = prepared.credential.source();
        anyhow::ensure!(
            !source.is_within_directory(&options.project)?
                || source.path() == options.project.join(".env"),
            "a credential source inside the project must be the explicitly configured root .env, which the import policy excludes"
        );
        match &preparation {
            PlanPreparation::Legacy => {
                provider_credential = Some(freeze_provider_credential(&config, &prepared)?);
            }
            PlanPreparation::Broker {
                control_literals, ..
            } => {
                let (reference, record) = freeze_provider_credential_retained(
                    &config,
                    &prepared,
                    Some(control_literals),
                )?;
                provider_credential = Some(reference);
                provider_record = Some(record);
            }
        }
        options.argv = prepared.argv;
        options.provider = prepared.resolution.provider_name().into();
        provider_resolution = Some(prepared.resolution);
    }
    crate::runtime::validate_command(&options.argv)?;
    if options.secret_env_file.is_some()
        && (options.profile != "observe" || options.network != NetworkMode::Bridge)
    {
        bail!("API credentials require explicit --profile observe --network bridge");
    }
    if options
        .secret_env_file
        .as_ref()
        .is_some_and(|p| p.starts_with(&options.project))
    {
        bail!("credential files must be outside the project being copied");
    }
    let credential_source = options
        .secret_env_file
        .as_deref()
        .map(|path| crate::control_fs::PrivateFile::open(path, 64 * 1024))
        .transpose()?;
    if let Some(source) = &credential_source {
        anyhow::ensure!(
            !source.is_within_directory(&options.project)?,
            "credential files must be outside the project being copied"
        );
        anyhow::ensure!(
            !options.interactive
                || !std::str::from_utf8(source.bytes())?
                    .lines()
                    .filter_map(|line| line.trim().split_once('='))
                    .any(|(key, _)| key == "TERM"),
            "TERM is fixed by the interactive preset and cannot be supplied in a credential file"
        );
    }
    if let Some(hash) = options.approve_hash.take() {
        let mut pending = read_plan(&approval_path(&options.config_dir, &hash)?).context(
            "no pending request matches this approval hash; rerun the original request first",
        )?;
        if pending.options != options || crate::policy::request_hash(&pending.request) != hash {
            bail!(
                "pending approval does not match these command, project, network, image, or runtime options"
            );
        }
        pending.credential_source = credential_source;
        Ok(ChosenPlan {
            plan: pending,
            config,
            provider_record,
        })
    } else {
        let session = Uuid::new_v4();
        let request = DecisionRequest {
            request_id: Uuid::new_v4().to_string(),
            profile: options.profile.clone(),
            command: CommandSpec {
                argv: options.argv.clone(),
                cwd: "/workspace".into(),
            },
            project_id: Some(format!(
                "project-{}",
                &hex::encode(Sha256::digest(options.project.to_string_lossy().as_bytes()))[..24]
            )),
            sandbox_id: Some(session.to_string()),
            agent_id: Some(format!("{}-{session}", options.provider)),
            session_id: Some(session.to_string()),
            correlation_id: Some(Uuid::new_v4().to_string()),
            trace_id: Some(Uuid::new_v4().to_string()),
            human_approved: false,
            approval_hash: None,
            execution_mode: options
                .interactive
                .then_some(crate::model::ExecutionModeV1::InteractiveTtyV1),
            provider_resolution,
        };
        let operation_id = match preparation {
            PlanPreparation::Legacy => Uuid::new_v4(),
            PlanPreparation::Broker { operation_id, .. } => operation_id,
        };
        Ok(ChosenPlan {
            plan: RunPlan {
                version: if managed_selection { 3 } else { 2 },
                operation_id,
                options,
                request,
                session,
                managed: None,
                provider_input,
                provider_credential,
                credential_source,
            },
            config,
            provider_record,
        })
    }
}

fn validate_interactive_options(options: &RunOptions) -> Result<()> {
    if options.interactive {
        anyhow::ensure!(
            !options.capture_output,
            "--interactive cannot be combined with --capture-output; no admission or import was performed"
        );
        crate::runtime::terminal::validate_controlling_terminal()?;
    }
    Ok(())
}
pub(crate) fn exclude_project_runtime(runtime: &Runtime, project: &Path) -> Result<()> {
    // choose_plan / the operator source resolver recorded this canonical path
    // before import. Recovery must not require the original to still exist.
    anyhow::ensure!(
        project.is_absolute()
            && !project.components().any(|part| matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )),
        "recorded project path must be absolute and normalized"
    );
    let invocation = runtime.invocation();
    // Inspect every installation-link component, including aliases such as
    // /var -> /private/var. An entry outside the tree may traverse a link in it.
    let mut pending: std::collections::VecDeque<_> = invocation
        .approved_entry
        .components()
        .map(|part| part.as_os_str().to_os_string())
        .collect();
    let mut resolved = PathBuf::new();
    let mut links = 0_u8;
    while let Some(part) = pending.pop_front() {
        if part == "/" {
            resolved = PathBuf::from("/");
            continue;
        }
        if part == "." {
            continue;
        }
        if part == ".." {
            resolved.pop();
            continue;
        }
        resolved.push(part);
        anyhow::ensure!(
            !resolved.starts_with(project),
            "runtime program must be outside the imported project"
        );
        if fs::symlink_metadata(&resolved)?.file_type().is_symlink() {
            links += 1;
            anyhow::ensure!(
                links <= 64,
                "runtime installation link chain exceeds its bound"
            );
            let target = fs::read_link(&resolved)?;
            resolved.pop();
            for part in target.components().rev() {
                pending.push_front(part.as_os_str().to_os_string());
            }
        }
    }
    anyhow::ensure!(
        resolved == invocation.canonical_program && !resolved.starts_with(project),
        "runtime installation link changed or enters the imported project"
    );
    Ok(())
}
fn approved_runtime(options: &RunOptions, program: Option<&Path>) -> Result<Runtime> {
    let runtime = match program.or(options.runtime_program.as_deref()) {
        Some(program) => Runtime::with_program(options.runtime, program)?,
        None => Runtime::new(options.runtime)?,
    };
    exclude_project_runtime(&runtime, &options.project)?;
    Ok(runtime)
}
pub(crate) async fn docker_endpoint(runtime: &Runtime, explicit: Option<&str>) -> Result<String> {
    anyhow::ensure!(
        runtime.kind == RuntimeKind::Docker,
        "Docker endpoint requires a Docker program"
    );
    if let Some(endpoint) = explicit {
        return Ok(endpoint.to_owned());
    }
    let output = runtime
        .cli(
            &[
                "context".into(),
                "inspect".into(),
                "--format".into(),
                "{{json .Endpoints.docker.Host}}".into(),
            ],
            Duration::from_secs(15),
        )
        .await?;
    anyhow::ensure!(
        !output.truncated,
        "Docker context endpoint exceeds its bound"
    );
    serde_json::from_slice(&output.bytes).context(
        "Docker context has no explicit endpoint; use --runtime-endpoint unix:///path/to/socket",
    )
}
fn event(plan: &RunPlan, kind: &str, data: Value) -> Value {
    json!({"id":Uuid::new_v4().to_string(),"timestamp":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis,true),"kind":kind,"source":"runner","provenance":"observed","coverage":"observed",
        "session_id":plan.request.session_id,"project_id":plan.request.project_id,"sandbox_id":plan.request.sandbox_id,"agent_id":plan.request.agent_id,"parent_agent_id":null,"correlation_id":plan.request.correlation_id,"trace_id":plan.request.trace_id,
        "command":null,"decision":null,"result":null,"data":data})
}
async fn emit(client: &AuditClient, plan: &RunPlan, kind: &str, data: Value) -> Result<()> {
    client.post_control_event(event(plan, kind, data)).await?;
    Ok(())
}

struct CredentialFile {
    file: crate::control_fs::Snapshot,
    _source: crate::control_fs::PrivateFile,
    secrets: Vec<String>,
    names: Vec<String>,
}

pub(crate) struct PreparedCredentialMaterial {
    pub bytes: Vec<u8>,
    pub names: Vec<String>,
    pub secrets: Vec<String>,
}

pub(crate) fn prepare_provider_credentials(
    bytes: &[u8],
    binding_id: Uuid,
) -> Result<PreparedCredentialMaterial> {
    anyhow::ensure!(
        bytes.len() <= 512 * 1024,
        "private provider record exceeds its bound"
    );
    let record: StoredProviderCredentialV1 =
        serde_json::from_slice(bytes).context("invalid private provider credential binding")?;
    let text = std::str::from_utf8(&record.env_bytes)
        .context("invalid private provider bootstrap encoding")?;
    let prefix = format!("{}=", crate::provider::BOOTSTRAP_ENV);
    anyhow::ensure!(
        record.version == 1
            && record.binding_id == binding_id
            && record.env_bytes.len() <= 64 * 1024
            && text.starts_with(&prefix)
            && text.ends_with('\n')
            && text.lines().count() == 1
            && !record.secrets.is_empty(),
        "private provider credential binding conflicts with the authorized handoff"
    );
    Ok(PreparedCredentialMaterial {
        bytes: record.env_bytes,
        secrets: record.secrets,
        names: vec![crate::provider::BOOTSTRAP_ENV.into()],
    })
}

fn freeze_provider_credential(
    config: &crate::control_fs::Directory,
    prepared: &crate::provider::PreparedProvider,
) -> Result<FrozenProviderCredentialV1> {
    Ok(freeze_provider_credential_retained(config, prepared, None)?.0)
}

fn freeze_provider_credential_retained(
    config: &crate::control_fs::Directory,
    prepared: &crate::provider::PreparedProvider,
    control: Option<&[String]>,
) -> Result<(FrozenProviderCredentialV1, crate::control_fs::PrivateFile)> {
    prepared.resolution.validate()?;
    prepared.resolution.validate_argv(&prepared.argv)?;
    prepared.credential.validate_source()?;
    let secrets = prepared.credential.secrets();
    let legacy_control;
    let control = match control {
        Some(values) => values,
        None => {
            legacy_control = control_secrets(config.path())?;
            &legacy_control
        }
    };
    anyhow::ensure!(
        !secrets
            .iter()
            .any(|value| control.iter().any(|control| value.contains(control))),
        "provider credential contains a control credential; nothing was transmitted or launched"
    );
    let binding_id = prepared.resolution.credential_binding_id;
    let record = StoredProviderCredentialV1 {
        version: 1,
        binding_id,
        env_bytes: prepared.credential.canonical_env_bytes(),
        secrets,
    };
    anyhow::ensure!(
        record.env_bytes.len() <= 64 * 1024,
        "provider bootstrap exceeds the private handoff limit"
    );
    let bindings = config.child("credential-bindings")?;
    let file_name = format!("{binding_id}.json");
    write_json_new(&bindings.path().join(&file_name), &record)?;
    bindings.sync()?;
    let file = bindings.read_private_file(&file_name, 512 * 1024)?;
    Ok((
        FrozenProviderCredentialV1 {
            binding_id,
            file_name,
            digest: file.digest().into(),
        },
        file,
    ))
}

fn validate_provider_plan(plan: &RunPlan) -> Result<()> {
    if plan.version != 3 {
        anyhow::ensure!(
            plan.provider_input.is_none()
                && plan.provider_credential.is_none()
                && plan.request.provider_resolution.is_none(),
            "legacy plans cannot acquire a managed provider resolution"
        );
        return Ok(());
    }
    let resolution = plan
        .request
        .provider_resolution
        .as_ref()
        .context("managed provider plan has no frozen resolution")?;
    resolution.validate()?;
    resolution.validate_argv(&plan.options.argv)?;
    let frozen = plan
        .provider_credential
        .as_ref()
        .context("managed provider plan has no private credential binding")?;
    anyhow::ensure!(
        plan.provider_input.is_some()
            && frozen.binding_id == resolution.credential_binding_id
            && frozen.file_name == format!("{}.json", frozen.binding_id)
            && plan.request.command.argv == plan.options.argv
            && plan.options.provider == resolution.provider_name()
            && plan.options.interactive == resolution.transport.is_interactive()
            && plan.options.profile == "observe"
            && plan.options.network == NetworkMode::Bridge
            && plan.options.secret_env_file.is_none(),
        "managed provider plan conflicts with its frozen invocation or credential binding"
    );
    Ok(())
}

fn frozen_provider_credentials(
    plan: &RunPlan,
    config: &crate::control_fs::Directory,
) -> Result<Option<CredentialFile>> {
    validate_provider_plan(plan)?;
    let Some(frozen) = &plan.provider_credential else {
        return Ok(None);
    };
    let source = config
        .child("credential-bindings")?
        .read_private_file(&frozen.file_name, 512 * 1024)?;
    anyhow::ensure!(
        source.digest() == frozen.digest,
        "frozen provider credential changed; a new authorization is required"
    );
    let record = prepare_provider_credentials(source.bytes(), frozen.binding_id)?;
    let file = config
        .child("credential-snapshots")?
        .snapshot(&record.bytes)?;
    Ok(Some(CredentialFile {
        file,
        _source: source,
        secrets: record.secrets,
        names: vec![crate::provider::BOOTSTRAP_ENV.into()],
    }))
}
fn credentials(path: &Path, config: &crate::control_fs::Directory) -> Result<CredentialFile> {
    let source =
        crate::control_fs::PrivateFile::open(&crate::control_fs::absolute_path(path)?, 64 * 1024)?;
    credentials_from_source(source, config)
}
fn credentials_from_source(
    source: crate::control_fs::PrivateFile,
    config: &crate::control_fs::Directory,
) -> Result<CredentialFile> {
    let material = prepare_manual_credentials(source.bytes())?;
    let file = config
        .child("credential-snapshots")?
        .snapshot(&material.bytes)?;
    Ok(CredentialFile {
        file,
        _source: source,
        secrets: material.secrets,
        names: material.names,
    })
}

pub(crate) fn prepare_manual_credentials(source: &[u8]) -> Result<PreparedCredentialMaterial> {
    let text = std::str::from_utf8(source).context("credential source must be UTF-8")?;
    let mut secrets = Vec::new();
    let mut names = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .context("credential file uses NAME=value lines, without shell syntax")?;
        if matches!(key, "JEV_API_KEY" | "JEV_API_KEY_FILE") {
            bail!(
                "control credential variables are forbidden in worker credential injection; nothing was transmitted or launched"
            );
        }
        if key.is_empty()
            || key.len() > 128
            || names.iter().any(|name| name == key)
            || !key
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            || key.as_bytes()[0].is_ascii_digit()
            || [
                "PATH",
                "HOME",
                "LD_PRELOAD",
                "LD_LIBRARY_PATH",
                "NODE_OPTIONS",
                "PYTHONPATH",
                "BASH_ENV",
                "ENV",
            ]
            .contains(&key)
            || value.len() < 8
            || value.chars().any(char::is_control)
        {
            bail!("invalid credential variable; use private NAME=value API credentials only");
        }
        names.push(key.into());
        secrets.push(value.into());
    }
    if secrets.is_empty() {
        bail!("credential file has no API values");
    }
    let mut bytes = Vec::new();
    for (name, value) in names.iter().zip(&secrets) {
        writeln!(bytes, "{name}={value}")?;
    }
    anyhow::ensure!(
        bytes.len() <= 64 * 1024,
        "canonical credential env file exceeds 64 KiB; no admission was requested"
    );
    Ok(PreparedCredentialMaterial {
        bytes,
        secrets,
        names,
    })
}
fn control_secrets(config: &Path) -> Result<Vec<String>> {
    let mut values = Vec::new();
    for role in ["runner", "viewer", "jev"] {
        let path = config.join("secrets").join(role);
        match fs::symlink_metadata(&path) {
            Ok(_) => values.push(read_token(&path)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(values)
}
fn validate_credential_scope(
    credential: Option<&CredentialFile>,
    control: &[String],
) -> Result<()> {
    anyhow::ensure!(
        credential.is_none_or(|credential| !credential
            .secrets
            .iter()
            .any(|value| control.iter().any(|control| value.contains(control)))),
        "worker credential injection contains a control credential; nothing was transmitted or launched"
    );
    Ok(())
}

pub async fn run(options: RunOptions) -> Result<RunSummary> {
    let supplied_approval = options.approve_hash.clone();
    let mut plan = choose_plan(options)?;
    let config = crate::control_fs::Directory::open_existing(&plan.options.config_dir)?;
    let credential_file = if plan.version == 3 {
        frozen_provider_credentials(&plan, &config)?
    } else {
        plan.credential_source
            .take()
            .map(|source| credentials_from_source(source, &config))
            .transpose()?
    };
    let runner_token = read_token(&plan.options.config_dir.join("secrets/runner"))?;
    let mut known_control = control_secrets(&plan.options.config_dir)?;
    validate_credential_scope(credential_file.as_ref(), &known_control)?;
    known_control.extend(
        credential_file
            .as_ref()
            .map(|f| f.secrets.clone())
            .unwrap_or_default(),
    );
    let client =
        AuditClient::new(&plan.options.gateway, &runner_token)?.with_secrets(known_control);
    let redactor = client.redactor.clone();
    client.validate_identity_payload(&plan.request)?;
    if let Some(resolution) = &plan.request.provider_resolution {
        eprintln!("Resolved provider: {}", serde_json::to_string(resolution)?);
        if resolution.auth_mode == crate::provider::AuthMode::Api {
            eprintln!("API mode: usage is billed by the provider.");
        }
    }
    managed::execute_plan(plan, credential_file, client, supplied_approval, config)
        .await
        .map_err(|error| {
            anyhow::anyhow!(crate::redaction::clip(
                &redacted_error(&redactor, &error),
                8192
            ))
        })
}

pub async fn export_workspace(
    session: Uuid,
    output: PathBuf,
    config: &Path,
    gateway: &str,
) -> Result<usize> {
    export_workspace_with_program(session, output, config, gateway, None, false).await
}
pub async fn export_workspace_with_program(
    session: Uuid,
    output: PathBuf,
    config: &Path,
    gateway: &str,
    runtime_program: Option<&Path>,
    adopt_runtime_program: bool,
) -> Result<usize> {
    let plan = read_plan(&config.join("sessions").join(format!("{session}.json")))?;
    if matches!(plan.version, 2 | 3) {
        return managed::export_workspace(
            session,
            output,
            config,
            gateway,
            runtime_program,
            adopt_runtime_program,
        )
        .await;
    }
    managed::export_legacy_workspace(
        plan,
        session,
        output,
        config,
        gateway,
        runtime_program,
        adopt_runtime_program,
    )
    .await
}
pub async fn worker_status(session: Uuid, config: &Path, gateway: &str) -> Result<Value> {
    managed::worker_status(session, config, gateway).await
}
pub async fn resume_worker(
    session: Uuid,
    config: &Path,
    gateway: &str,
    wait_seconds: u64,
) -> Result<Value> {
    resume_worker_with_program(session, config, gateway, wait_seconds, None, false).await
}
pub async fn resume_worker_with_program(
    session: Uuid,
    config: &Path,
    gateway: &str,
    wait_seconds: u64,
    runtime_program: Option<&Path>,
    adopt_runtime_program: bool,
) -> Result<Value> {
    resume_worker_with_mode(
        session,
        config,
        gateway,
        wait_seconds,
        runtime_program,
        adopt_runtime_program,
        false,
    )
    .await
}
pub async fn resume_worker_with_mode(
    session: Uuid,
    config: &Path,
    gateway: &str,
    wait_seconds: u64,
    runtime_program: Option<&Path>,
    adopt_runtime_program: bool,
    interactive: bool,
) -> Result<Value> {
    managed::resume_worker(
        session,
        config,
        gateway,
        wait_seconds,
        runtime_program,
        adopt_runtime_program,
        interactive,
    )
    .await
}
pub async fn stop_worker(session: Uuid, config: &Path, gateway: &str) -> Result<Value> {
    stop_worker_with_program(session, config, gateway, None, false).await
}
pub async fn stop_worker_with_program(
    session: Uuid,
    config: &Path,
    gateway: &str,
    runtime_program: Option<&Path>,
    adopt_runtime_program: bool,
) -> Result<Value> {
    managed::stop_worker(
        session,
        config,
        gateway,
        runtime_program,
        adopt_runtime_program,
    )
    .await
}

#[cfg(all(test, unix))]
mod event_transport_tests {
    use super::*;
    use crate::{
        api::{GatewayState, TenantGateway, gateway_router},
        audit::{EventQuery, Store},
        model::Event,
        policy::Policy,
    };
    use axum::{
        body::{Body, to_bytes},
        extract::Request,
        http::StatusCode,
        middleware::{self, Next},
        response::IntoResponse,
    };
    use std::{
        os::unix::fs::PermissionsExt,
        sync::{Arc, Mutex},
    };

    #[derive(Clone, Copy)]
    enum Fault {
        TypedOnce,
        TypedAlways,
        Plain,
        Capacity,
        Unauthorized,
        Forbidden,
        Unavailable,
        DuplicateHeader,
        MissingHeader,
        FalseCode,
        OldGateway,
    }

    struct TestGateway {
        _directory: tempfile::TempDir,
        store: Arc<Store>,
        seen: Arc<Mutex<Vec<(String, Vec<u8>)>>>,
        client: AuditClient,
        server: tokio::task::JoinHandle<()>,
    }
    impl Drop for TestGateway {
        fn drop(&mut self) {
            self.server.abort();
        }
    }
    impl TestGateway {
        async fn new(fault: Fault) -> Self {
            let directory = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap();
            let store = Arc::new(
                Store::open(
                    &directory.path().join("audit.sqlite"),
                    Redactor::new(vec![]),
                    1000,
                )
                .unwrap(),
            );
            let state = GatewayState::for_tenants(
                vec![TenantGateway {
                    tenant_id: "local".into(),
                    store: store.clone(),
                    runner_token: "fixture-runner".into(),
                    viewer_token: "fixture-viewer".into(),
                }],
                Policy::default(),
                None,
                Redactor::new(vec![]),
            )
            .unwrap();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let recorded = seen.clone();
            let app=gateway_router(state).layer(middleware::from_fn(move|request:Request,next:Next|{
                let seen=recorded.clone();
                async move {
                    let (parts,body)=request.into_parts();
                    let bytes=to_bytes(body,128*1024).await.unwrap();
                    let count={let mut items=seen.lock().unwrap();items.push((parts.uri.path().into(),bytes.to_vec()));items.len()};
                    if matches!(fault,Fault::TypedOnce)&&count>1 {return next.run(Request::from_parts(parts,Body::from(bytes))).await;}
                    let status=match fault {Fault::Unauthorized=>StatusCode::UNAUTHORIZED,Fault::Forbidden=>StatusCode::FORBIDDEN,Fault::Unavailable=>StatusCode::SERVICE_UNAVAILABLE,Fault::OldGateway=>StatusCode::NOT_FOUND,_=>StatusCode::TOO_MANY_REQUESTS};
                    let body=match fault {Fault::Plain=>json!({"error":"untyped refusal"}),Fault::Capacity=>json!({"error":{"code":"audit_write_budget_exhausted","retry_action":"contact_operator"}}),Fault::FalseCode=>json!({"error":{"code":"different_rate_limit"}}),_=>json!({"error":{"code":"write_rate_limited"}})};
                    let mut response=(status,axum::Json(body)).into_response();
                    if !matches!(fault,Fault::MissingHeader){response.headers_mut().insert(reqwest::header::RETRY_AFTER,axum::http::HeaderValue::from_static("1"));}
                    if matches!(fault,Fault::DuplicateHeader){response.headers_mut().append(reqwest::header::RETRY_AFTER,axum::http::HeaderValue::from_static("1"));}
                    response
                }
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let client = AuditClient::new(
                &format!("http://{}", listener.local_addr().unwrap()),
                "fixture-runner",
            )
            .unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            Self {
                _directory: directory,
                store,
                seen,
                client,
                server,
            }
        }
    }

    #[tokio::test]
    async fn typed_rate_retry_preserves_exact_bytes_identity_and_single_storage() {
        for control in [false, true] {
            let gateway = TestGateway::new(Fault::TypedOnce).await;
            let event = serde_json::to_value(Event::new("command.requested")).unwrap();
            let identity = event["id"].clone();
            let seq = if control {
                gateway.client.post_control_event(event).await
            } else {
                gateway.client.post_event(event).await
            }
            .unwrap();
            let seen = gateway.seen.lock().unwrap();
            assert_eq!(seen.len(), 2);
            assert_eq!(seen[0].1, seen[1].1);
            assert_eq!(
                seen[0].0,
                if control {
                    "/control/api/v1/control/events"
                } else {
                    "/api/events"
                }
            );
            assert_eq!(seen[0].0, seen[1].0);
            let rows = gateway.store.events(&EventQuery::default()).unwrap();
            let matching = rows
                .iter()
                .filter(|row| row.event.id == identity.as_str().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(matching.len(), 1);
            assert_eq!(matching[0].seq, seq);
        }
    }

    #[tokio::test]
    async fn other_refusals_and_ambiguous_rate_headers_are_never_retried() {
        for fault in [
            Fault::Plain,
            Fault::Capacity,
            Fault::Unauthorized,
            Fault::Forbidden,
            Fault::Unavailable,
            Fault::DuplicateHeader,
            Fault::MissingHeader,
            Fault::FalseCode,
        ] {
            let gateway = TestGateway::new(fault).await;
            assert!(
                gateway
                    .client
                    .post_event(json!(Event::new("command.requested")))
                    .await
                    .is_err()
            );
            assert_eq!(gateway.seen.lock().unwrap().len(), 1);
            assert!(
                !gateway
                    .store
                    .events(&EventQuery::default())
                    .unwrap()
                    .iter()
                    .any(|row| row.event.kind == "command.requested")
            );
        }
    }

    #[tokio::test]
    async fn persistent_typed_rate_refusal_stops_after_three_identical_requests() {
        let gateway = TestGateway::new(Fault::TypedAlways).await;
        let result = gateway
            .client
            .post_event(json!(Event::new("command.requested")))
            .await
            .unwrap_err();
        assert_eq!(
            result.downcast_ref::<EventPostFailure>(),
            Some(&EventPostFailure::RateLimitRetryExhausted)
        );
        let seen = gateway.seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert!(seen.windows(2).all(|pair| pair[0].1 == pair[1].1));
    }

    #[tokio::test]
    async fn old_gateway_control_route_failure_has_no_ingestion_fallback() {
        let gateway = TestGateway::new(Fault::OldGateway).await;
        let error = gateway
            .client
            .post_control_event(json!(Event::new("command.requested")))
            .await
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<EventPostFailure>(),
            Some(&EventPostFailure::Rejected(StatusCode::NOT_FOUND))
        );
        let seen = gateway.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "/control/api/v1/control/events");
    }

    #[tokio::test]
    async fn mandatory_start_callback_is_one_control_attempt_without_public_fallback() {
        let gateway = TestGateway::new(Fault::TypedAlways).await;
        let error = gateway
            .client
            .post_control_event_once(json!(Event::new("command.started")))
            .await
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<EventPostFailure>(),
            Some(&EventPostFailure::Rejected(StatusCode::TOO_MANY_REQUESTS))
        );
        let seen = gateway.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "/control/api/v1/control/events");
    }
}

#[cfg(all(test, unix))]
mod control_transport_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    fn managed_fixture() -> (tempfile::TempDir, RunOptions) {
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let project = root.join("project");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("README.txt"), "ordinary project data").unwrap();
        let source = project.join(".env");
        fs::write(&source, "OPENAI_API_KEY=canary-selected-codex-private\nANTHROPIC_API_KEY=canary-unselected-claude-private\n").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
        let config = root.join("config");
        crate::operations::initialize_config(&config).unwrap();
        write_json_new(
            &config.join("providers.json"),
            &crate::provider::ProviderConfigV1 {
                version: 1,
                default_profile: Some("codex-api".into()),
                auto_priority: vec!["codex-api".into()],
                profiles: vec![crate::provider::ProviderProfileV1 {
                    id: "codex-api".into(),
                    revision: 1,
                    client: crate::provider::ClientKind::Codex,
                    auth_mode: crate::provider::AuthMode::Api,
                    model: None,
                    credential: crate::provider::CredentialSourceV1::ApiEnvFile {
                        path: source,
                        key: Some("OPENAI_API_KEY".into()),
                    },
                }],
            },
        )
        .unwrap();
        (
            temporary,
            RunOptions {
                project,
                config_dir: config,
                gateway: "http://127.0.0.1:1".into(),
                runtime: RuntimeKind::Docker,
                runtime_program: None,
                runtime_endpoint: Some("unix:///fixture.sock".into()),
                image: "worker:fixture".into(),
                profile: "observe".into(),
                network: NetworkMode::Bridge,
                allow_rootful_runtime: false,
                timeout: 5,
                provider: "generic".into(),
                client: Some("codex".into()),
                auth_mode: Some(crate::provider::AuthPreference::Api),
                client_profile: None,
                providers_file: None,
                prompt: Some("inspect project".into()),
                model: None,
                approve_hash: None,
                capture_output: false,
                interactive: false,
                secret_env_file: None,
                argv: Vec::new(),
            },
        )
    }

    #[cfg(unix)]
    #[test]
    fn managed_plan_freezes_only_selected_credentials_and_excludes_root_env() {
        let (_temporary, options) = managed_fixture();
        let plan = choose_plan(options).unwrap();
        assert_eq!(plan.version, 3);
        let config = crate::control_fs::Directory::open_existing(&plan.options.config_dir).unwrap();
        let credential = frozen_provider_credentials(&plan, &config)
            .unwrap()
            .unwrap();
        let bytes = std::str::from_utf8(credential.file.private_file().bytes())
            .unwrap()
            .to_owned();
        assert!(bytes.contains("canary-selected-codex-private"));
        assert!(!bytes.contains("canary-unselected-claude-private"));
        assert_eq!(credential.names, [crate::provider::BOOTSTRAP_ENV]);
        let public = serde_json::to_string(&plan.request).unwrap();
        for private in [
            "canary-selected-codex-private",
            "canary-unselected-claude-private",
            plan.options.project.to_str().unwrap(),
            &plan.provider_credential.as_ref().unwrap().digest,
        ] {
            assert!(
                !public.contains(private),
                "private credential metadata entered the decision request"
            );
        }
        let workspace =
            workspace::prepare_workspace(&plan.options.project, Limits::default()).unwrap();
        assert!(workspace.excluded.iter().any(|name| name == ".env"));
        assert!(
            !serde_json::to_string(&workspace.manifest)
                .unwrap()
                .contains(".env")
        );
    }

    #[cfg(unix)]
    #[test]
    fn provider_approval_reuses_frozen_choice_after_sources_and_configuration_change() {
        let (_temporary, mut options) = managed_fixture();
        let plan = choose_plan(options.clone()).unwrap();
        let hash = crate::policy::request_hash(&plan.request);
        write_json_new(
            &approval_path(&plan.options.config_dir, &hash).unwrap(),
            &plan,
        )
        .unwrap();
        fs::remove_file(plan.options.project.join(".env")).unwrap();
        fs::write(
            plan.options.config_dir.join("providers.json"),
            "invalid replacement configuration",
        )
        .unwrap();
        options.approve_hash = Some(hash);
        let approved = choose_plan(options).unwrap();
        assert_eq!(
            approved.request.provider_resolution,
            plan.request.provider_resolution
        );
        assert_eq!(approved.options.argv, plan.options.argv);
        let config =
            crate::control_fs::Directory::open_existing(&approved.options.config_dir).unwrap();
        assert!(
            frozen_provider_credentials(&approved, &config)
                .unwrap()
                .is_some()
        );
    }

    #[cfg(unix)]
    #[test]
    fn changed_frozen_provider_binding_is_rejected_before_handoff() {
        let (_temporary, options) = managed_fixture();
        let plan = choose_plan(options).unwrap();
        let frozen = plan.provider_credential.as_ref().unwrap();
        let config = crate::control_fs::Directory::open_existing(&plan.options.config_dir).unwrap();
        let path = config
            .path()
            .join("credential-bindings")
            .join(&frozen.file_name);
        let original = fs::read(&path).unwrap();
        let changed = String::from_utf8(original).unwrap().replace(
            "canary-selected-codex-private",
            "canary-substituted-private",
        );
        fs::write(path, changed).unwrap();
        assert!(frozen_provider_credentials(&plan, &config).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn provider_source_inside_project_must_be_the_explicit_excluded_root_env() {
        use std::os::unix::fs::PermissionsExt;
        let (_temporary, options) = managed_fixture();
        let unsafe_source = options.project.join("provider.env");
        fs::rename(options.project.join(".env"), &unsafe_source).unwrap();
        fs::set_permissions(&unsafe_source, fs::Permissions::from_mode(0o600)).unwrap();
        let path = options.config_dir.join("providers.json");
        let mut config = crate::provider::load_config(&path).unwrap();
        config.profiles[0].credential = crate::provider::CredentialSourceV1::ApiEnvFile {
            path: unsafe_source,
            key: Some("OPENAI_API_KEY".into()),
        };
        fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        assert!(choose_plan(options).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn provider_relative_source_is_resolved_from_the_retained_configuration() {
        let (_temporary, options) = managed_fixture();
        let config = crate::control_fs::Directory::open_existing(&options.config_dir).unwrap();
        let credentials = config.child("credentials").unwrap();
        let source = credentials.path().join("provider.env");
        fs::rename(options.project.join(".env"), &source).unwrap();
        let config_path = config.path().join("providers.json");
        let mut providers = crate::provider::load_config(&config_path).unwrap();
        providers.profiles[0].credential = crate::provider::CredentialSourceV1::ApiEnvFile {
            path: PathBuf::from("credentials/provider.env"),
            key: Some("OPENAI_API_KEY".into()),
        };
        fs::write(&config_path, serde_json::to_vec(&providers).unwrap()).unwrap();
        let loaded = crate::provider::load_config(&config_path).unwrap();
        assert_eq!(
            loaded.profiles[0].credential,
            crate::provider::CredentialSourceV1::ApiEnvFile {
                path: source,
                key: Some("OPENAI_API_KEY".into()),
            }
        );
        // The process keeps its repository CWD. The source is relative to the
        // retained private configuration, never CWD or the imported project.
        let plan = choose_plan(options).unwrap();
        let credential = frozen_provider_credentials(&plan, &config)
            .unwrap()
            .unwrap();
        let bytes = credential.file.private_file();
        let text = std::str::from_utf8(bytes.bytes()).unwrap();
        assert!(text.contains("canary-selected-codex-private"));
        assert!(!text.contains("canary-unselected-claude-private"));
    }
    #[cfg(unix)]
    #[test]
    fn canonical_credential_limit_is_checked_before_snapshot_and_admission() {
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let config = crate::control_fs::Directory::open_or_create(&root.join("config")).unwrap();
        let source = root.join("provider.env");
        let mut bytes = b"OPENAI_API_KEY=".to_vec();
        bytes.resize(64 * 1024, b'x');
        fs::write(&source, bytes).unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            credentials(&source, &config).is_err(),
            "canonical newline exceeded the env-file limit before admission"
        );
        assert!(!config.path().join("credential-snapshots").exists());
    }
    #[cfg(unix)]
    #[test]
    fn manual_credential_handoff_refuses_jev_names_before_snapshot() {
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("provider-canary.env");
        let business = "OPENAI_API_KEY=canary-business-openai-private\nANTHROPIC_API_KEY=canary-business-anthropic-private\n";
        fs::write(&source, business).unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
        let config = crate::control_fs::Directory::open_or_create(&root.join("business")).unwrap();
        let credential = credentials(&source, &config).unwrap();
        assert_eq!(credential.names, ["OPENAI_API_KEY", "ANTHROPIC_API_KEY"]);
        assert!(
            validate_credential_scope(
                Some(&credential),
                &["canary-business-openai-private".into()]
            )
            .is_err()
        );
        drop(credential);

        for (index, name) in ["JEV_API_KEY", "JEV_API_KEY_FILE"].iter().enumerate() {
            let config = crate::control_fs::Directory::open_or_create(
                &root.join(format!("blocked-{index}")),
            )
            .unwrap();
            fs::write(
                &source,
                format!("{business}{name}=canary-jev-control-private\n"),
            )
            .unwrap();
            let result = credentials(&source, &config);
            assert!(
                result.is_err(),
                "a Jev control credential variable was accepted"
            );
            let diagnostic = result.err().unwrap().to_string();
            assert_eq!(
                diagnostic,
                "control credential variables are forbidden in worker credential injection; nothing was transmitted or launched"
            );
            assert!(!diagnostic.contains("canary-jev-control-private"));
            assert!(
                !config.path().join("credential-snapshots").exists(),
                "a snapshot was created before rejecting a Jev control credential variable"
            );
        }
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn credential_source_case_alias_cannot_bypass_project_exclusion() {
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let project = root.join("project");
        fs::create_dir(&project).unwrap();
        let source = project.join("provider.env");
        fs::write(&source, "OPENAI_API_KEY=fixture-private-value\n").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
        let alias = root.join("PROJECT/provider.env");
        if !alias.exists() {
            eprintln!("NOT_APPLICABLE: case-sensitive test filesystem");
            return;
        }
        let config = root.join("config");
        crate::operations::initialize_config(&config).unwrap();
        let result = choose_plan(RunOptions {
            project,
            config_dir: config,
            gateway: "http://127.0.0.1:1".into(),
            runtime: RuntimeKind::Docker,
            runtime_program: None,
            runtime_endpoint: Some("unix:///fixture.sock".into()),
            image: "worker:fixture".into(),
            profile: "observe".into(),
            network: NetworkMode::Bridge,
            allow_rootful_runtime: false,
            timeout: 5,
            provider: "generic".into(),
            client: None,
            auth_mode: None,
            client_profile: None,
            providers_file: None,
            prompt: None,
            model: None,
            approve_hash: None,
            capture_output: false,
            interactive: false,
            secret_env_file: Some(alias),
            argv: vec!["/usr/bin/id".into()],
        });
        assert!(
            result.is_err(),
            "a source inside the project was accepted through a case alias"
        );
    }
    #[test]
    fn ordinary_error_causes_keep_context_without_known_secrets() {
        let secret = "private-fixture-value-never-in-diagnostics";
        let error = anyhow::anyhow!("inner context:\n{secret}\t")
            .context("middle context")
            .context(format!("outer context: {secret}"));
        let diagnostic = redacted_error(&Redactor::new(vec![secret.into()]), &error);
        assert!(diagnostic.contains("outer context"));
        assert!(diagnostic.contains("middle context"));
        assert!(diagnostic.contains("inner context"));
        assert!(!diagnostic.contains(secret));
        assert!(
            !diagnostic.chars().any(char::is_control),
            "a control diagnostic cannot be sent in a bounded worker reason"
        );
    }
    #[cfg(unix)]
    #[test]
    fn credential_snapshot_matches_the_exact_values_used_for_masking() {
        let mut source = tempfile::NamedTempFile::new().unwrap();
        source
            .write_all(b"OPENAI_API_KEY=example-private-value   \n")
            .unwrap();
        source.flush().unwrap();
        let config = tempfile::tempdir().unwrap();
        let directory = crate::control_fs::Directory::open_or_create(
            &config.path().canonicalize().unwrap().join("private-config"),
        )
        .unwrap();
        let credential = credentials(source.path(), &directory).unwrap();
        let injected = fs::read_to_string(credential.file.path()).unwrap();
        let value = injected.lines().next().unwrap().split_once('=').unwrap().1;
        assert_eq!(value, credential.secrets[0]);
    }
    #[cfg(unix)]
    #[test]
    fn credential_snapshot_ignores_hostile_tmpdir() {
        if let Some(config) = std::env::var_os("AI_GUARD_CREDENTIAL_CONFIG_PROBE") {
            let source =
                PathBuf::from(std::env::var_os("AI_GUARD_CREDENTIAL_SOURCE_PROBE").unwrap());
            let directory =
                crate::control_fs::Directory::open_existing(&PathBuf::from(&config)).unwrap();
            let credential = credentials(&source, &directory).unwrap();
            assert!(
                credential.file.path().starts_with(config),
                "snapshot escaped the private configuration"
            );
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let config = root.join("config");
        crate::operations::initialize_config(&config).unwrap();
        let source = root.join("provider.env");
        fs::write(&source, "OPENAI_API_KEY=example-private-value\n").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
        let hostile = root.join("shared-temp");
        fs::create_dir(&hostile).unwrap();
        fs::set_permissions(&hostile, fs::Permissions::from_mode(0o777)).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runner::tests::credential_snapshot_ignores_hostile_tmpdir",
                "--nocapture",
            ])
            .env("TMPDIR", &hostile)
            .env("AI_GUARD_CREDENTIAL_CONFIG_PROBE", &config)
            .env("AI_GUARD_CREDENTIAL_SOURCE_PROBE", &source)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(fs::read_dir(hostile).unwrap().next().is_none());
    }
    #[test]
    fn ipv6_loopback_is_an_allowed_local_gateway() {
        assert!(AuditClient::new("http://[::1]:8787", "local-token-value").is_ok());
    }
}
