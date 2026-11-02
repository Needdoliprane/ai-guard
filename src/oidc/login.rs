use super::{
    IdValidationContext, OidcClientKind, OidcError, OidcProvider, PendingLogin, TokenBundle,
    VerifiedAccess, VerifiedLogin,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use openidconnect::{
    AccessTokenHash, AuthorizationCode, ClientId, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, Nonce, NonceVerifier, OAuth2TokenResponse, PkceCodeChallenge, RedirectUrl,
    RefreshToken, RequestTokenError, TokenResponse,
    core::{
        CoreAuthenticationFlow, CoreClient, CoreJwsSigningAlgorithm, CoreTokenResponse,
        CoreTokenType,
    },
};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Instant};

type PublicClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;
impl OidcProvider {
    fn client(&self, kind: OidcClientKind) -> Result<PublicClient, OidcError> {
        // Public clients only: no secret, request-object signing or private-key JWT authentication.
        Ok(CoreClient::from_provider_metadata(
            self.metadata()?,
            ClientId::new(self.config.client_id(kind).to_owned()),
            None,
        ))
    }
    /// Once discovery is ready this only builds an authorization request; it performs no IO.
    pub async fn begin_login(
        &self,
        kind: OidcClientKind,
        redirect: &str,
    ) -> Result<PendingLogin, OidcError> {
        let _operation = self
            .operations
            .try_acquire()
            .map_err(|_| OidcError::Capacity)?;
        self.config.redirect(kind, redirect)?;
        let client = self.client(kind)?.set_redirect_uri(
            RedirectUrl::new(redirect.to_owned()).map_err(|_| OidcError::InvalidLogin)?,
        );
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, state, nonce) = client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .set_pkce_challenge(challenge)
            .url();
        Ok(PendingLogin {
            authorization_url: url.to_string(),
            provider_id: self.provider_id,
            kind,
            redirect: redirect.to_owned(),
            state: state.secret().to_owned(),
            nonce,
            verifier,
            created: Instant::now(),
        })
    }
    pub async fn complete_login(
        &self,
        pending: PendingLogin,
        code: &str,
        state: &str,
    ) -> Result<VerifiedLogin, OidcError> {
        tokio::time::timeout(
            std::time::Duration::from_millis(self.config.limits.request_timeout_ms),
            self.complete_login_inner(pending, code, state),
        )
        .await
        .map_err(|_| OidcError::TransportUnavailable)?
    }
    async fn complete_login_inner(
        &self,
        pending: PendingLogin,
        code: &str,
        state: &str,
    ) -> Result<VerifiedLogin, OidcError> {
        let _operation = self
            .operations
            .try_acquire()
            .map_err(|_| OidcError::Capacity)?;
        if pending.provider_id != self.provider_id
            || pending.created.elapsed().as_secs() >= self.config.limits.pkce_ttl_seconds
            || !super::config::text(code, 2048)
            || !super::config::text(state, 256)
            || Sha256::digest(state.as_bytes()) != Sha256::digest(pending.state.as_bytes())
        {
            return Err(OidcError::InvalidLogin);
        }
        self.config.redirect(pending.kind, &pending.redirect)?;
        self.ensure_ready().await?;
        let client = self.client(pending.kind)?.set_redirect_uri(
            RedirectUrl::new(pending.redirect).map_err(|_| OidcError::InvalidLogin)?,
        );
        let response = client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .map_err(|_| OidcError::InvalidConfiguration)?
            .set_pkce_verifier(pending.verifier)
            .request_async(&self.transport)
            .await
            .map_err(exchange_error)?;
        self.validate_response(&response)?;
        let mut access = self
            .verify_access_inner(response.access_token().secret())
            .await?;
        if access.client_id != self.config.client_id(pending.kind) {
            return Err(OidcError::InvalidLogin);
        }
        let context = self
            .validate_id(&response, pending.kind, &pending.nonce, true, None, &access)
            .await?;
        access.login_context = Some(Arc::new(context));
        Ok(VerifiedLogin {
            access,
            tokens: bundle(&response),
            logout_context: None,
        })
    }
    /// The caller serializes refresh rotation and audits intention before calling this method.
    /// An ambiguous result is an error, never an automatic second use of a rotating token.
    pub async fn refresh(
        &self,
        kind: OidcClientKind,
        refresh: &str,
        expected: &VerifiedAccess,
    ) -> Result<VerifiedLogin, OidcError> {
        tokio::time::timeout(
            std::time::Duration::from_millis(self.config.limits.request_timeout_ms),
            self.refresh_inner(kind, refresh, expected),
        )
        .await
        .map_err(|_| OidcError::TransportUnavailable)?
    }
    async fn refresh_inner(
        &self,
        kind: OidcClientKind,
        refresh: &str,
        expected: &VerifiedAccess,
    ) -> Result<VerifiedLogin, OidcError> {
        let _operation = self
            .operations
            .try_acquire()
            .map_err(|_| OidcError::Capacity)?;
        let original = expected
            .login_context
            .as_ref()
            .ok_or(OidcError::InvalidLogin)?;
        if expected.provider_id != self.provider_id
            || expected.issuer != self.config.issuer
            || expected.client_id != self.config.client_id(kind)
            || expected.expires_at <= chrono::Utc::now().timestamp()
            || !super::config::text(refresh, self.config.limits.max_token_bytes)
        {
            return Err(OidcError::InvalidLogin);
        }
        self.ensure_ready().await?;
        let client = self.client(kind)?;
        let refresh_token = RefreshToken::new(refresh.to_owned());
        let response = client
            .exchange_refresh_token(&refresh_token)
            .map_err(|_| OidcError::InvalidConfiguration)?
            .request_async(&self.transport)
            .await
            .map_err(exchange_error)?;
        self.validate_response(&response)?;
        let mut access = self
            .verify_access_inner(response.access_token().secret())
            .await?;
        if access.issuer != expected.issuer
            || access.subject != expected.subject
            || access.client_id != expected.client_id
        {
            return Err(OidcError::InvalidLogin);
        }
        if response.id_token().is_some() {
            self.validate_id(
                &response,
                kind,
                &Nonce::new(original.nonce.clone()),
                false,
                Some(original),
                &access,
            )
            .await?;
        }
        access.login_context = Some(Arc::clone(original));
        Ok(VerifiedLogin {
            access,
            tokens: bundle(&response),
            logout_context: None,
        })
    }
    fn validate_response(&self, response: &CoreTokenResponse) -> Result<(), OidcError> {
        if response.token_type() != &CoreTokenType::Bearer
            || !super::config::text(
                response.access_token().secret(),
                self.config.limits.max_token_bytes,
            )
            || response.refresh_token().is_some_and(|t| {
                !super::config::text(t.secret(), self.config.limits.max_token_bytes)
            })
            || response
                .id_token()
                .is_some_and(|id| id.to_string().len() > self.config.limits.max_token_bytes)
            || response.expires_in().is_some_and(|d| {
                d.is_zero() || d.as_secs() > self.config.limits.max_token_lifetime_seconds
            })
        {
            return Err(OidcError::InvalidLogin);
        }
        Ok(())
    }
    async fn validate_id(
        &self,
        response: &CoreTokenResponse,
        kind: OidcClientKind,
        nonce: &Nonce,
        require_nonce: bool,
        original: Option<&IdValidationContext>,
        access: &VerifiedAccess,
    ) -> Result<IdValidationContext, OidcError> {
        let invalid = OidcError::InvalidLogin;
        let id = response.id_token().ok_or(invalid)?;
        let compact = id.to_string();
        let header = super::access::token_header(&compact, self.config.limits.max_token_bytes)
            .map_err(|_| invalid)?;
        if header.alg != jsonwebtoken::Algorithm::RS256 || header.typ.as_deref() != Some("JWT") {
            return Err(invalid);
        }
        self.access_key(header.kid.as_deref().ok_or(invalid)?)
            .await?;
        let payload = compact.split('.').nth(1).ok_or(invalid)?;
        let raw = super::json::parse(&URL_SAFE_NO_PAD.decode(payload).map_err(|_| invalid)?)
            .map_err(|_| invalid)?;
        let client = self.client(kind)?;
        let now = chrono::Utc::now().timestamp();
        let skew = self.config.limits.clock_skew_seconds as i64;
        let maximum = self.config.limits.max_token_lifetime_seconds as i64;
        let verifier = client
            .id_token_verifier()
            .set_allowed_algs([CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256])
            .set_issue_time_verifier_fn(move |iat| {
                if iat.timestamp() > now.saturating_add(skew)
                    || now.saturating_sub(iat.timestamp()) > maximum
                {
                    Err("invalid ID issue time".into())
                } else {
                    Ok(())
                }
            });
        let claims = id
            .claims(&verifier, |observed: Option<&Nonce>| {
                if observed.is_none() && !require_nonce {
                    Ok(())
                } else {
                    nonce.verify(observed)
                }
            })
            .map_err(|_| invalid)?;
        let audiences = super::access::audiences(&raw).map_err(|_| invalid)?;
        let client_id = self.config.client_id(kind);
        if claims.issuer().as_str() != self.config.issuer
            || claims.subject().as_str() != access.subject
            || audiences != [client_id.to_owned()]
            || raw
                .get("azp")
                .is_some_and(|v| v.as_str() != Some(client_id))
        {
            return Err(invalid);
        }
        let exp = super::access::numeric_date(&raw, "exp").map_err(|_| invalid)?;
        let iat = super::access::numeric_date(&raw, "iat").map_err(|_| invalid)?;
        if exp <= iat || exp.saturating_sub(iat) > maximum {
            return Err(invalid);
        }
        let auth_time = raw
            .get("auth_time")
            .map(|v| v.as_i64().filter(|t| *t > 0 && *t <= iat).ok_or(invalid))
            .transpose()?;
        if let Some(original) = original {
            if original.audiences != audiences
                || (raw.get("auth_time").is_some() && auth_time != original.auth_time)
            {
                return Err(invalid);
            }
        }
        if let Some(expected_hash) = claims.access_token_hash() {
            let actual = AccessTokenHash::from_token(
                response.access_token(),
                id.signing_alg().map_err(|_| invalid)?,
                id.signing_key(&verifier).map_err(|_| invalid)?,
            )
            .map_err(|_| invalid)?;
            if actual != *expected_hash {
                return Err(invalid);
            }
        }
        Ok(IdValidationContext {
            nonce: nonce.secret().to_owned(),
            audiences,
            auth_time,
        })
    }
}
fn bundle(response: &CoreTokenResponse) -> TokenBundle {
    TokenBundle {
        access_token: response.access_token().secret().to_owned(),
        refresh_token: response.refresh_token().map(|t| t.secret().to_owned()),
    }
}
fn exchange_error<E>(error: RequestTokenError<OidcError, E>) -> OidcError
where
    E: openidconnect::ErrorResponse,
{
    match error {
        RequestTokenError::Request(error) => error,
        _ => OidcError::ProviderRejected,
    }
}
