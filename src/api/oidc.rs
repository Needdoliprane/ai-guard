//! Gateway-owned OIDC presentation. No cookie authenticates a business API.
use super::*;
use crate::audit::authentication::{
    AuthenticationActionV1 as Action, AuthenticationOutcomeV1 as Outcome,
    AuthenticationPhaseV1 as Phase, AuthenticationReasonV1 as Reason, AuthenticationReceiptV1,
    AuthenticationRecordV1, AuthenticationSessionBindingV1 as SessionBinding,
    AuthenticationSessionStateV1 as SessionState, AuthenticationTokenFingerprintV1 as Fingerprint,
    OidcSessionKindV1 as SessionKind,
};
use crate::{
    audit::authentication::AuthenticationAudit,
    identity::{HumanCapabilityV1, PrincipalRefV1},
    oidc::{
        OidcConfigV1, OidcLogoutContext, OidcProvider, OidcTransportModeV1, PendingLogin,
        VerifiedAccess,
    },
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use uuid::Uuid;
mod gateway;
use gateway::{admit, callback, logout, native_intent, refresh, rotate, start, tokens};

#[derive(Clone)]
pub(super) struct OidcHumanProof {
    pub principal: PrincipalRefV1,
    pub authentication_id: String,
    pub generation: u32,
    pub digest: [u8; 32],
    pub expires_at: i64,
    pub deadline: Instant,
    pub ceiling: BTreeSet<HumanCapabilityV1>,
    pub mapping_revision: String,
}
struct Session {
    proof: OidcHumanProof,
    binding: SessionBinding,
    access: Arc<VerifiedAccess>,
    logout_context: Option<OidcLogoutContext>,
    created: Instant,
    used: Instant,
    rotating: bool,
    pending_refresh: Option<PendingRefresh>,
    maximum_expiry: i64,
}
struct PendingRefresh {
    record: AuthenticationRecordV1,
    receipt: AuthenticationReceiptV1,
    deadline: Instant,
}
// Cookie/PKCE correlation is unverified, bounded, expiring memory state. It
// must not reserve durable authentication capacity before provider proof.
struct BrowserTransaction {
    pending: Option<PendingLogin>,
    expected_state: [u8; 32],
    transaction_id: Uuid,
    operation_id: Uuid,
    session_id: Uuid,
    created: Instant,
}
struct NativeIntent {
    session_id: Uuid,
    operation_id: Uuid,
    // Initial login nonces have no durable authority. Only a refresh of an
    // already admitted session carries a durable pending-transition receipt.
    intention: Option<AuthenticationReceiptV1>,
    action: Action,
    previous: Option<SessionBinding>,
    created: Instant,
}
#[derive(Serialize)]
struct TokenDelivery {
    version: u32,
    authentication_id: String,
    generation: u32,
    token_type: &'static str,
    access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    expires_at: String,
    expires_in_seconds: u64,
}
enum Delivery {
    Tokens(TokenDelivery),
    Error(&'static str),
}
struct PendingDelivery {
    result: Delivery,
    created: Instant,
}
struct ExchangeMarker {
    transaction_id: Uuid,
    // Retain this even before proof. If admission succeeds but the callback
    // disappears before delivery, reap must close that exact admitted session.
    session_id: Uuid,
    created: Instant,
}
#[derive(Default)]
struct Registry {
    transactions: HashMap<[u8; 32], BrowserTransaction>,
    deliveries: HashMap<[u8; 32], PendingDelivery>,
    exchanging: HashMap<[u8; 32], ExchangeMarker>,
    sessions: HashMap<String, Session>,
    native: HashMap<Uuid, NativeIntent>,
    rate: Option<(Instant, u32)>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcClaimRuleV1 {
    pub claim_path: Vec<String>,
    pub equals: String,
    pub capabilities: BTreeSet<HumanCapabilityV1>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcClaimsMappingV1 {
    pub revision: String,
    pub default_capabilities: BTreeSet<HumanCapabilityV1>,
    pub rules: Vec<OidcClaimRuleV1>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcGatewayConfigV1 {
    pub version: u32,
    pub provider: OidcConfigV1,
    pub provider_label: String,
    pub web_origin: String,
    pub mapping: OidcClaimsMappingV1,
}
impl OidcGatewayConfigV1 {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.version == 1
                && self.provider.limits.max_token_bytes <= 16_384
                && self.provider.limits.pkce_ttl_seconds <= 300,
            "unsupported OIDC gateway configuration or token bound"
        );
        let origin = reqwest::Url::parse(&self.web_origin)?;
        let callback = reqwest::Url::parse(&self.provider.web_redirect_uri)?;
        let demo = matches!(
            self.provider.transport,
            OidcTransportModeV1::IsolatedDemoHttp { .. }
        );
        anyhow::ensure!(
            self.web_origin == origin.origin().ascii_serialization()
                && (origin.scheme() == "https"
                    || (demo
                        && origin.scheme() == "http"
                        && origin
                            .host_str()
                            .and_then(|h| h
                                .trim_matches(['[', ']'])
                                .parse::<std::net::IpAddr>()
                                .ok())
                            .is_some_and(|ip| ip.is_loopback())))
                && callback.origin() == origin.origin()
                && callback.path() == "/auth/oidc/callback",
            "invalid OIDC web origin or callback"
        );
        anyhow::ensure!(
            safe_text(&self.provider_label, 128)
                && safe_text(&self.mapping.revision, 128)
                && self.mapping.rules.len() <= 64
                && self
                    .mapping
                    .default_capabilities
                    .iter()
                    .all(|cap| !cap.is_administration()),
            "invalid OIDC claim mapping"
        );
        let mut seen = BTreeSet::new();
        for rule in &self.mapping.rules {
            anyhow::ensure!(
                !rule.claim_path.is_empty()
                    && rule.claim_path.len() <= 16
                    && rule.claim_path.iter().all(|part| safe_text(part, 128))
                    && safe_text(&rule.equals, 256)
                    && !rule.capabilities.is_empty()
                    && seen.insert((rule.claim_path.clone(), rule.equals.clone())),
                "ambiguous OIDC claim mapping"
            );
        }
        // Offline validation precedes opening or initializing the private audit.
        OidcProvider::new(self.provider.clone())?;
        Ok(())
    }
}
fn safe_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
impl OidcClaimsMappingV1 {
    fn capabilities(&self, claims: &Value) -> anyhow::Result<BTreeSet<HumanCapabilityV1>> {
        let mut capabilities = self.default_capabilities.clone();
        for rule in &self.rules {
            let mut value = claims;
            let mut absent = false;
            for part in &rule.claim_path {
                let object = value
                    .as_object()
                    .ok_or_else(|| anyhow::anyhow!("invalid mapped OIDC claim path type"))?;
                match object.get(part) {
                    Some(next) => value = next,
                    None => {
                        absent = true;
                        break;
                    }
                }
            }
            if absent {
                continue;
            }
            let values = value
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("invalid mapped OIDC claim type"))?;
            anyhow::ensure!(
                values.len() <= 256
                    && values
                        .iter()
                        .all(|v| v.as_str().is_some_and(|s| safe_text(s, 256))),
                "invalid mapped OIDC claim values"
            );
            if values.iter().any(|v| v.as_str() == Some(&rule.equals)) {
                capabilities.extend(&rule.capabilities);
            }
        }
        Ok(capabilities)
    }
}

/// An explicitly configured provider and separate operator-private audit.
/// Construction is offline. Status/profile never probe the identity provider.
pub struct OidcGateway {
    pub(super) config: OidcGatewayConfigV1,
    provider: Arc<OidcProvider>,
    pub(super) audit: Arc<AuthenticationAudit>,
    ca_bundle_pem: Option<String>,
    registry: Mutex<Registry>,
    cleanup_slots: Arc<tokio::sync::Semaphore>,
}
impl OidcGateway {
    pub fn new(
        config: OidcGatewayConfigV1,
        audit: Arc<AuthenticationAudit>,
        ca_bundle_pem: Option<String>,
    ) -> anyhow::Result<Self> {
        config.validate()?;
        let provider = match &ca_bundle_pem {
            Some(pem) => {
                anyhow::ensure!(
                    !pem.is_empty()
                        && pem.len() <= 262_144
                        && pem.contains("-----BEGIN CERTIFICATE-----")
                        && !pem.contains("PRIVATE KEY"),
                    "invalid public OIDC CA bundle"
                );
                OidcProvider::new_with_ca(config.provider.clone(), pem.as_bytes())?
            }
            None => OidcProvider::new(config.provider.clone())?,
        };
        audit.verify()?;
        Ok(Self {
            config,
            provider: Arc::new(provider),
            audit,
            ca_bundle_pem,
            registry: Mutex::new(Registry::default()),
            cleanup_slots: Arc::new(tokio::sync::Semaphore::new(32)),
        })
    }
    pub(super) fn authenticate(&self, token: &str) -> ApiResult<OidcHumanProof> {
        self.audit.verify().map_err(|_| audit_unavailable())?;
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let mut registry = self.registry.lock().map_err(|_| audit_unavailable())?;
        let session = registry
            .sessions
            .values_mut()
            .find(|session| session.proof.digest == digest)
            .ok_or_else(credential_invalid)?;
        if session.proof.expires_at <= chrono::Utc::now().timestamp()
            || session.proof.deadline <= Instant::now()
            || session.created.elapsed() >= Duration::from_secs(3600)
            || session.used.elapsed() >= Duration::from_secs(900)
        {
            return Err(credential_invalid());
        }
        if session.proof.mapping_revision != self.config.mapping.revision {
            return Err(credential_invalid());
        }
        self.active_binding(&session.binding)?;
        if session.rotating {
            return Err(credential_invalid());
        }
        session.used = Instant::now();
        Ok(session.proof.clone())
    }
    pub(super) fn reauthenticate(&self, proof: &OidcHumanProof) -> ApiResult<()> {
        self.audit.verify().map_err(|_| audit_unavailable())?;
        let registry = self.registry.lock().map_err(|_| audit_unavailable())?;
        let session = registry
            .sessions
            .get(&proof.authentication_id)
            .ok_or_else(credential_invalid)?;
        if session.proof.digest != proof.digest
            || session.proof.generation != proof.generation
            || session.proof.principal != proof.principal
            || session.proof.mapping_revision != proof.mapping_revision
            || session.proof.ceiling != proof.ceiling
            || session.proof.expires_at <= chrono::Utc::now().timestamp()
            || session.proof.deadline <= Instant::now()
            || session.rotating
            || session.created.elapsed() >= Duration::from_secs(3600)
            || session.used.elapsed() >= Duration::from_secs(900)
        {
            return Err(credential_invalid());
        }
        self.active_binding(&session.binding)?;
        Ok(())
    }
    fn active_binding(&self, binding: &SessionBinding) -> ApiResult<()> {
        let current = self
            .audit
            .session_current(binding.session_id)
            .map_err(audit_failure)?
            .ok_or_else(credential_invalid)?;
        if current.binding != *binding || current.state != SessionState::Active {
            return Err(credential_invalid());
        }
        Ok(())
    }
}
pub(super) fn routes() -> Router<GatewayState> {
    Router::new()
        .route("/status", get(status))
        .route("/cli-profile", get(cli_profile))
        .route("/start", post(start))
        .route("/callback", get(callback))
        .route("/tokens", post(tokens))
        .route("/refresh", post(refresh))
        .route("/native-intent", post(native_intent))
        .route("/admit", post(admit))
        .route("/rotate", post(rotate))
        .route("/logout", post(logout))
}
async fn status(State(state): State<GatewayState>, headers: HeaderMap) -> ApiResult<Response> {
    state
        .blocking_identity(move |state| status_sync(State(state), headers))
        .await
}
fn status_sync(State(state): State<GatewayState>, headers: HeaderMap) -> ApiResult<Response> {
    let mut response = Json(match &state.oidc {
        Some(oidc) => json!({"version":1,"enabled":true,"issuer":oidc.config.provider.issuer,"provider_label":oidc.config.provider_label}),
        None => json!({"version":1,"enabled":false}),
    }).into_response();
    if let Some(oidc) = &state.oidc {
        if !public_status_origin(&headers, &oidc.config.web_origin) {
            return Err(origin_denied());
        }
        let mut registry = oidc.registry.lock().map_err(|_| audit_unavailable())?;
        gateway::reap(oidc, &mut registry)?;
        if registry.transactions.len()
            + registry.deliveries.len()
            + registry.native.len()
            + registry.exchanging.len()
            >= 256
        {
            return Err(capacity());
        }
        drop(registry);
        if correlation_cookie(&headers)?.is_none() {
            let raw =
                hex::encode(crate::identity::random_bytes().map_err(|_| audit_unavailable())?);
            response.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_str(&cookie(
                    &raw,
                    oidc.config.web_origin.starts_with("https://"),
                    false,
                ))
                .map_err(|_| audit_unavailable())?,
            );
        }
    }
    Ok(response)
}
async fn cli_profile(State(state): State<GatewayState>) -> Json<Value> {
    let Some(oidc) = &state.oidc else {
        return Json(json!({"version":1,"enabled":false}));
    };
    // Allowlisted public provider DTO. Never serialize the gateway config.
    let config = &oidc.config.provider;
    let mut value = json!({"version":1,"enabled":true,"provider": {
        "version":config.version,"issuer":config.issuer,"web_client_id":config.web_client_id,
        "cli_client_id":config.cli_client_id,"api_audience":config.api_audience,
        "authorization_endpoint":config.authorization_endpoint,"token_endpoint":config.token_endpoint,
        "jwks_uri":config.jwks_uri,"web_redirect_uri":config.web_redirect_uri,
        "cli_callback_path":config.cli_callback_path,"end_session_endpoint":config.end_session_endpoint,
        "transport":config.transport,"limits":config.limits,
    }});
    for (key, uri) in [
        (
            "web_post_logout_redirect_uri",
            &config.web_post_logout_redirect_uri,
        ),
        (
            "cli_post_logout_redirect_uri",
            &config.cli_post_logout_redirect_uri,
        ),
    ] {
        if let Some(uri) = uri {
            value["provider"][key] = Value::String(uri.clone());
        }
    }
    if let Some(pem) = &oidc.ca_bundle_pem {
        value["ca_bundle_pem"] = Value::String(pem.clone());
    }
    Json(value)
}
fn not_configured() -> (StatusCode, Json<Value>) {
    human::failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "oidc_not_configured",
        "OIDC is not configured on this gateway.",
        "contact_operator",
    )
}
fn audit_unavailable() -> (StatusCode, Json<Value>) {
    human::failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "audit_unavailable",
        "Mandatory authentication audit is unavailable.",
        "contact_operator",
    )
}
fn credential_invalid() -> (StatusCode, Json<Value>) {
    human::failure(
        StatusCode::UNAUTHORIZED,
        "credential_invalid",
        "The admitted OIDC session is unavailable, expired or revoked.",
        "authenticate",
    )
}
fn origin_denied() -> (StatusCode, Json<Value>) {
    human::failure(
        StatusCode::FORBIDDEN,
        "origin_denied",
        "The exact configured web origin is required.",
        "authenticate",
    )
}
fn capacity() -> (StatusCode, Json<Value>) {
    human::failure(
        StatusCode::TOO_MANY_REQUESTS,
        "capacity_exhausted",
        "Live authentication capacity is full.",
        "retry",
    )
}
fn conflict() -> (StatusCode, Json<Value>) {
    human::failure(
        StatusCode::CONFLICT,
        "session_conflict",
        "The authentication operation is consumed, superseded or pending.",
        "authenticate",
    )
}
fn audit_failure(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    use crate::audit::authentication::AuthenticationAuditFailureV1 as Failure;
    match error.downcast_ref::<Failure>() {
        Some(Failure::Conflict | Failure::PhaseOrder) => conflict(),
        Some(Failure::Capacity) => audit_unavailable(),
        _ => audit_unavailable(),
    }
}
pub(super) fn human_bearer(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let token = value.to_str().ok()?.strip_prefix("Bearer ")?;
    (!token.is_empty()
        && token.len() <= 16_384
        && !token
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace()))
    .then_some(token)
}
pub(super) fn browser_post(method: &axum::http::Method, path: &str) -> bool {
    method == "POST"
        && [
            "/auth/oidc/start",
            "/auth/oidc/tokens",
            "/auth/oidc/refresh",
            "/auth/oidc/logout",
        ]
        .contains(&path)
}
pub(super) fn same_origin(headers: &HeaderMap, expected: &str) -> bool {
    let mut origins = headers.get_all(header::ORIGIN).iter();
    origins
        .next()
        .is_some_and(|origin| origin.as_bytes() == expected.as_bytes())
        && origins.next().is_none()
        && optional_header(headers, "sec-fetch-site", &["same-origin"])
        && optional_header(headers, "sec-fetch-mode", &["cors", "same-origin"])
}
fn optional_header(headers: &HeaderMap, name: &str, allowed: &[&str]) -> bool {
    let mut values = headers.get_all(name).iter();
    values
        .next()
        .is_none_or(|value| value.to_str().is_ok_and(|value| allowed.contains(&value)))
        && values.next().is_none()
}
pub(super) fn public_status_origin(headers: &HeaderMap, expected: &str) -> bool {
    let mut origins = headers.get_all(header::ORIGIN).iter();
    origins
        .next()
        .is_none_or(|origin| origin.as_bytes() == expected.as_bytes())
        && origins.next().is_none()
        && optional_header(headers, "sec-fetch-site", &["same-origin", "none"])
}
pub(super) fn invalid_request() -> (StatusCode, Json<Value>) {
    human::failure(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        "The authentication request is invalid or exceeds its bounds.",
        "authenticate",
    )
}
pub(super) fn correlation_cookie(headers: &HeaderMap) -> ApiResult<Option<String>> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE).iter() {
        if value.as_bytes().len() > 4096 {
            return Err(invalid_request());
        }
        let value = value.to_str().map_err(|_| invalid_request())?;
        for pair in value.split(';') {
            let Some((name, value)) = pair.trim().split_once('=') else {
                continue;
            };
            if name != "aiguard_oidc" {
                continue;
            }
            if found.is_some() || !hex_digest(value) {
                return Err(invalid_request());
            }
            found = Some(value.to_owned());
        }
    }
    Ok(found)
}
fn hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn cookie(value: &str, secure: bool, clear: bool) -> String {
    format!(
        "aiguard_oidc={value}; Path=/auth/oidc; HttpOnly; SameSite=Lax; Max-Age={}{}",
        if clear { 0 } else { 300 },
        if secure { "; Secure" } else { "" }
    )
}
pub(super) fn valid_response_cookie(value: &HeaderValue, secure: bool) -> bool {
    value.to_str().is_ok_and(|value| {
        let Some(raw) = value
            .strip_prefix("aiguard_oidc=")
            .and_then(|v| v.split(';').next())
        else {
            return false;
        };
        (raw.is_empty() || hex_digest(raw)) && value == cookie(raw, secure, raw.is_empty())
    })
}
