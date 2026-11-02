//! OIDC provider boundary. No authorization, membership, session or audit authority lives here.
//!
//! The caller owns durable authentication audit, browser correlation and single-use admission.
//! Verified access proofs are created only by this module and are private user data.
mod access;
mod config;
mod json;
mod keys;
mod login;
mod transport;

pub use config::{OidcConfigV1, OidcLimitsV1, OidcTransportModeV1};
use openidconnect::{Nonce, PkceCodeVerifier, core::CoreProviderMetadata};
use serde_json::Value;
use std::{
    fmt,
    sync::{Arc, Mutex},
    time::Instant,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OidcClientKind {
    Web,
    Cli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OidcError {
    NotConfigured,
    InvalidConfiguration,
    TransportUnavailable,
    ProviderRejected,
    InvalidMetadata,
    InvalidKeys,
    InvalidToken,
    InvalidLogin,
    Capacity,
}
impl fmt::Display for OidcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotConfigured => "OIDC is not configured",
            Self::InvalidConfiguration => "OIDC configuration is invalid",
            Self::TransportUnavailable => "OIDC provider transport is unavailable",
            Self::ProviderRejected => "OIDC provider rejected the exchange",
            Self::InvalidMetadata => "OIDC provider metadata is invalid",
            Self::InvalidKeys => "OIDC provider keys are invalid",
            Self::InvalidToken => "OIDC access token is invalid",
            Self::InvalidLogin => "OIDC login response is invalid",
            Self::Capacity => "OIDC request capacity is unavailable",
        })
    }
}
impl std::error::Error for OidcError {}

/// A verified API access token, never an ID token or a caller-constructible principal.
pub struct VerifiedAccess {
    issuer: String,
    subject: String,
    client_id: String,
    expires_at: i64,
    claims: Value,
    provider_id: uuid::Uuid,
    login_context: Option<Arc<IdValidationContext>>,
}
impl VerifiedAccess {
    pub fn issuer(&self) -> &str {
        &self.issuer
    }
    pub fn subject(&self) -> &str {
        &self.subject
    }
    pub fn client_id(&self) -> &str {
        &self.client_id
    }
    pub fn expires_at(&self) -> i64 {
        self.expires_at
    }
    /// Private verified claims for server mapping, never a display or logging DTO.
    pub fn claims(&self) -> &Value {
        &self.claims
    }
}
impl fmt::Debug for VerifiedAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VerifiedAccess([redacted])")
    }
}

/// Tokens are never serializable here. Delivery must be an explicit private caller operation.
pub struct TokenBundle {
    access_token: String,
    refresh_token: Option<String>,
}
impl TokenBundle {
    pub fn access_token(&self) -> &str {
        &self.access_token
    }
    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }
}
impl fmt::Debug for TokenBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenBundle([redacted])")
    }
}
pub struct VerifiedLogin {
    access: VerifiedAccess,
    tokens: TokenBundle,
    logout_context: Option<OidcLogoutContext>,
}
impl VerifiedLogin {
    pub fn access(&self) -> &VerifiedAccess {
        &self.access
    }
    pub fn tokens(&self) -> &TokenBundle {
        &self.tokens
    }
    pub fn into_parts(self) -> (VerifiedAccess, TokenBundle) {
        (self.access, self.tokens)
    }
    /// Root keeps the opaque ID context separately from the browser token delivery.
    /// A refresh without an ID token returns None: retain the preceding logout context.
    pub fn into_session_parts(self) -> (VerifiedAccess, TokenBundle, Option<OidcLogoutContext>) {
        (self.access, self.tokens, self.logout_context)
    }
}
impl fmt::Debug for VerifiedLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VerifiedLogin([redacted])")
    }
}

/// Owned, non-Clone PKCE state. Passing it to completion consumes the attempt even on failure.
pub struct PendingLogin {
    authorization_url: String,
    provider_id: uuid::Uuid,
    kind: OidcClientKind,
    redirect: String,
    state: String,
    nonce: Nonce,
    verifier: PkceCodeVerifier,
    created: Instant,
}
impl PendingLogin {
    pub fn authorization_url(&self) -> &str {
        &self.authorization_url
    }
}
impl fmt::Debug for PendingLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PendingLogin([redacted])")
    }
}

/// This handle never exposes the ID token, including through serialization or Debug.
pub struct OidcLogoutContext {
    provider_id: uuid::Uuid,
    kind: OidcClientKind,
    id_token: String,
}
impl fmt::Debug for OidcLogoutContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OidcLogoutContext([redacted])")
    }
}
struct IdValidationContext {
    nonce: String,
    audiences: Vec<String>,
    auth_time: Option<i64>,
}
#[derive(Default)]
struct Cache {
    metadata: Option<CoreProviderMetadata>,
    metadata_at: Option<Instant>,
    keys: Vec<keys::SigningKey>,
    keys_at: Option<Instant>,
    unknown_refresh_at: Option<Instant>,
}
pub struct OidcProvider {
    config: OidcConfigV1,
    provider_id: uuid::Uuid,
    transport: transport::Transport,
    cache: Mutex<Cache>,
    update: tokio::sync::Mutex<()>,
    operations: tokio::sync::Semaphore,
}
impl fmt::Debug for OidcProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OidcProvider([redacted])")
    }
}
impl OidcProvider {
    /// Explicit construction is offline and does not enable OIDC in any gateway by itself.
    pub fn new(config: OidcConfigV1) -> Result<Self, OidcError> {
        Self::with_ca(config, None)
    }
    fn with_ca(config: OidcConfigV1, pem: Option<&[u8]>) -> Result<Self, OidcError> {
        config.validate()?;
        let transport = transport::Transport::new(&config, pem)?;
        let operations = tokio::sync::Semaphore::new(config.limits.max_inflight);
        Ok(Self {
            config,
            provider_id: uuid::Uuid::new_v4(),
            transport,
            cache: Mutex::new(Cache::default()),
            update: tokio::sync::Mutex::new(()),
            operations,
        })
    }
    /// Trusted caller supplies only a pre-read public CA bundle; never an installation action.
    pub fn new_with_ca(config: OidcConfigV1, pem: &[u8]) -> Result<Self, OidcError> {
        Self::with_ca(config, Some(pem))
    }
    pub async fn discover(&self) -> Result<(), OidcError> {
        let _updating = self.update.try_lock().map_err(|_| OidcError::Capacity)?;
        tokio::time::timeout(
            std::time::Duration::from_millis(self.config.limits.request_timeout_ms),
            self.load_provider(),
        )
        .await
        .map_err(|_| OidcError::TransportUnavailable)?
    }
    async fn load_provider(&self) -> Result<(), OidcError> {
        let bytes = self.transport.get(&self.config.discovery_url()).await?;
        let value = json::parse(&bytes).map_err(|_| OidcError::InvalidMetadata)?;
        for (key, expected) in [
            ("issuer", &self.config.issuer),
            (
                "authorization_endpoint",
                &self.config.authorization_endpoint,
            ),
            ("token_endpoint", &self.config.token_endpoint),
            ("jwks_uri", &self.config.jwks_uri),
        ] {
            if value.get(key).and_then(Value::as_str) != Some(expected.as_str()) {
                return Err(OidcError::InvalidMetadata);
            }
        }
        if let Some(endpoint) = &self.config.end_session_endpoint
            && value.get("end_session_endpoint").and_then(Value::as_str) != Some(endpoint.as_str())
        {
            return Err(OidcError::InvalidMetadata);
        }
        if !string_array_contains(&value, "response_types_supported", "code")
            || !string_array_contains(&value, "id_token_signing_alg_values_supported", "RS256")
            || !string_array_contains(&value, "code_challenge_methods_supported", "S256")
        {
            return Err(OidcError::InvalidMetadata);
        }
        let metadata: CoreProviderMetadata =
            serde_json::from_value(value).map_err(|_| OidcError::InvalidMetadata)?;
        let (keys, jwks) = self.fetch_keys().await?;
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| OidcError::TransportUnavailable)?;
        cache.metadata = Some(metadata.set_jwks(jwks));
        cache.metadata_at = Some(Instant::now());
        cache.keys = keys;
        cache.keys_at = Some(Instant::now());
        Ok(())
    }
    async fn fetch_keys(
        &self,
    ) -> Result<
        (
            Vec<keys::SigningKey>,
            openidconnect::core::CoreJsonWebKeySet,
        ),
        OidcError,
    > {
        keys::parse(
            &self.transport.get(&self.config.jwks_uri).await?,
            &self.config,
        )
    }
    async fn ensure_ready(&self) -> Result<(), OidcError> {
        let ready = {
            let cache = self
                .cache
                .lock()
                .map_err(|_| OidcError::TransportUnavailable)?;
            cache
                .metadata_at
                .is_some_and(|at| at.elapsed().as_secs() < self.config.limits.metadata_ttl_seconds)
                && cache
                    .keys_at
                    .is_some_and(|at| at.elapsed().as_secs() < self.config.limits.jwks_ttl_seconds)
        };
        if !ready {
            self.discover().await?;
        }
        Ok(())
    }
    fn metadata(&self) -> Result<CoreProviderMetadata, OidcError> {
        let cache = self
            .cache
            .lock()
            .map_err(|_| OidcError::TransportUnavailable)?;
        if cache
            .metadata_at
            .is_none_or(|at| at.elapsed().as_secs() >= self.config.limits.metadata_ttl_seconds)
            || cache
                .keys_at
                .is_none_or(|at| at.elapsed().as_secs() >= self.config.limits.jwks_ttl_seconds)
        {
            return Err(OidcError::InvalidMetadata);
        }
        cache.metadata.clone().ok_or(OidcError::NotConfigured)
    }
    /// Caller must erase its local session regardless of this separately reported IdP outcome.
    pub async fn logout_idp(&self, context: OidcLogoutContext) -> Result<(), OidcError> {
        let _operation = self
            .operations
            .try_acquire()
            .map_err(|_| OidcError::Capacity)?;
        if context.provider_id != self.provider_id {
            return Err(OidcError::InvalidLogin);
        }
        let endpoint = self
            .config
            .end_session_endpoint
            .as_deref()
            .ok_or(OidcError::NotConfigured)?;
        tokio::time::timeout(
            std::time::Duration::from_millis(self.config.limits.request_timeout_ms),
            async {
                self.ensure_ready().await?;
                self.transport
                    .logout(
                        endpoint,
                        self.config.client_id(context.kind),
                        &context.id_token,
                    )
                    .await
            },
        )
        .await
        .map_err(|_| OidcError::TransportUnavailable)?
    }
}
fn string_array_contains(value: &Value, key: &str, wanted: &str) -> bool {
    value.get(key).and_then(Value::as_array).is_some_and(|v| {
        v.len() <= 32
            && v.iter().all(Value::is_string)
            && v.iter().any(|v| v.as_str() == Some(wanted))
    })
}
