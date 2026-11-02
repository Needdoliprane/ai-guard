use super::{OidcClientKind, OidcError};
use serde::{Deserialize, Serialize};

/// Operator-approved URLs are exact. Discovery may confirm them, never add destinations.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcConfigV1 {
    pub version: u32,
    pub issuer: String,
    pub web_client_id: String,
    pub cli_client_id: String,
    pub api_audience: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    pub web_redirect_uri: String,
    pub cli_callback_path: String,
    #[serde(default)]
    pub end_session_endpoint: Option<String>,
    pub transport: OidcTransportModeV1,
    #[serde(default)]
    pub limits: OidcLimitsV1,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OidcTransportModeV1 {
    HttpsOnly,
    IsolatedDemoHttp { exact_issuer: String },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcLimitsV1 {
    pub request_timeout_ms: u64,
    pub max_response_bytes: usize,
    pub max_token_bytes: usize,
    pub max_jwks_keys: usize,
    pub max_inflight: usize,
    pub metadata_ttl_seconds: u64,
    pub jwks_ttl_seconds: u64,
    pub unknown_kid_cooldown_seconds: u64,
    pub pkce_ttl_seconds: u64,
    pub clock_skew_seconds: u64,
    pub max_token_lifetime_seconds: u64,
}
impl Default for OidcLimitsV1 {
    fn default() -> Self {
        Self {
            request_timeout_ms: 5000,
            max_response_bytes: 131_072,
            max_token_bytes: 16_384,
            max_jwks_keys: 32,
            max_inflight: 8,
            metadata_ttl_seconds: 300,
            jwks_ttl_seconds: 300,
            unknown_kid_cooldown_seconds: 30,
            pkce_ttl_seconds: 300,
            clock_skew_seconds: 5,
            max_token_lifetime_seconds: 3600,
        }
    }
}

impl OidcConfigV1 {
    pub(super) fn validate(&self) -> Result<(), OidcError> {
        let invalid = OidcError::InvalidConfiguration;
        if self.version != 1 {
            return Err(invalid);
        }
        for value in [&self.web_client_id, &self.cli_client_id, &self.api_audience] {
            if !text(value, 128) {
                return Err(invalid);
            }
        }
        if self.web_client_id == self.cli_client_id
            || self.api_audience == self.web_client_id
            || self.api_audience == self.cli_client_id
        {
            return Err(invalid);
        }
        let issuer = url(&self.issuer)?;
        let demo = match &self.transport {
            OidcTransportModeV1::HttpsOnly => false,
            OidcTransportModeV1::IsolatedDemoHttp { exact_issuer } => {
                if exact_issuer != &self.issuer || issuer.scheme() != "http" || !demo_host(&issuer)
                {
                    return Err(invalid);
                }
                true
            }
        };
        if !demo && issuer.scheme() != "https" {
            return Err(invalid);
        }
        for endpoint in [
            &self.authorization_endpoint,
            &self.token_endpoint,
            &self.jwks_uri,
        ]
        .into_iter()
        .chain(self.end_session_endpoint.iter())
        {
            let endpoint = url(endpoint)?;
            if demo {
                if endpoint.origin() != issuer.origin()
                    || !endpoint
                        .path()
                        .starts_with(&format!("{}/", issuer.path().trim_end_matches('/')))
                {
                    return Err(invalid);
                }
            } else if endpoint.scheme() != "https" {
                return Err(invalid);
            }
        }
        if self.authorization_endpoint == self.token_endpoint
            || self.token_endpoint == self.jwks_uri
            || self.authorization_endpoint == self.jwks_uri
        {
            return Err(invalid);
        }
        if self.end_session_endpoint.as_ref().is_some_and(|endpoint| {
            [
                &self.authorization_endpoint,
                &self.token_endpoint,
                &self.jwks_uri,
            ]
            .contains(&endpoint)
        }) {
            return Err(invalid);
        }
        let redirect = url(&self.web_redirect_uri)?;
        if redirect.scheme() != "https"
            && !(demo && literal_loopback(&redirect) && redirect.scheme() == "http")
        {
            return Err(invalid);
        }
        if !self.cli_callback_path.starts_with('/')
            || self.cli_callback_path.starts_with("//")
            || self.cli_callback_path.len() > 128
            || self
                .cli_callback_path
                .bytes()
                .any(|b| !b.is_ascii_alphanumeric() && !b"/-_".contains(&b))
        {
            return Err(invalid);
        }
        let l = &self.limits;
        if !(25..=30_000).contains(&l.request_timeout_ms)
            || !(1024..=1_048_576).contains(&l.max_response_bytes)
            || !(1024..=65_536).contains(&l.max_token_bytes)
            || !(1..=128).contains(&l.max_jwks_keys)
            || !(1..=32).contains(&l.max_inflight)
            || !(1..=3600).contains(&l.metadata_ttl_seconds)
            || !(1..=3600).contains(&l.jwks_ttl_seconds)
            || !(1..=300).contains(&l.unknown_kid_cooldown_seconds)
            || !(1..=600).contains(&l.pkce_ttl_seconds)
            || l.clock_skew_seconds > 30
            || !(1..=86_400).contains(&l.max_token_lifetime_seconds)
        {
            return Err(invalid);
        }
        Ok(())
    }
    pub(super) fn discovery_url(&self) -> String {
        format!(
            "{}/.well-known/openid-configuration",
            self.issuer.trim_end_matches('/')
        )
    }
    pub(super) fn client_id(&self, kind: OidcClientKind) -> &str {
        match kind {
            OidcClientKind::Web => &self.web_client_id,
            OidcClientKind::Cli => &self.cli_client_id,
        }
    }
    pub(super) fn redirect(&self, kind: OidcClientKind, redirect: &str) -> Result<(), OidcError> {
        let valid = match kind {
            OidcClientKind::Web => redirect == self.web_redirect_uri,
            OidcClientKind::Cli => url(redirect).is_ok_and(|u| {
                u.scheme() == "http"
                    && literal_loopback(&u)
                    && u.port().is_some_and(|p| p != 0)
                    && u.path() == self.cli_callback_path
            }),
        };
        if valid {
            Ok(())
        } else {
            Err(OidcError::InvalidLogin)
        }
    }
}
pub(super) fn text(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && !value.chars().any(char::is_control)
        && !value.trim().is_empty()
}
fn url(value: &str) -> Result<reqwest::Url, OidcError> {
    if !text(value, 2048) || value.chars().any(char::is_whitespace) {
        return Err(OidcError::InvalidConfiguration);
    }
    let u = reqwest::Url::parse(value).map_err(|_| OidcError::InvalidConfiguration)?;
    if !matches!(u.scheme(), "https" | "http")
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return Err(OidcError::InvalidConfiguration);
    }
    // Reject URL normalization that would silently change a configured path or authority.
    if u.as_str() != value && !(u.path() == "/" && u.as_str().strip_suffix('/') == Some(value)) {
        return Err(OidcError::InvalidConfiguration);
    }
    Ok(u)
}
fn literal_loopback(u: &reqwest::Url) -> bool {
    u.host_str()
        .and_then(|h| h.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok())
        .is_some_and(|ip| ip.is_loopback())
}
fn demo_host(u: &reqwest::Url) -> bool {
    literal_loopback(u)
        || (u.host_str() == Some("keycloak.ai-guard.test")
            && u.port() == Some(18080)
            && u.path() == "/realms/ai-guard-demo")
}
