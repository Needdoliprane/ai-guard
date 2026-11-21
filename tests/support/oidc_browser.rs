//! Signed synthetic issuer used only by the opt-in Chromium refresh test.

use super::controlled::signed_token;
use axum::{
    Form, Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::Redirect,
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub(super) const SUBJECT: &str = "browser-refresh-member";
pub(super) const ADMIN_ROLE: &str = "tenant-admin";
const CLIENT: &str = "web-public";
const LOGIN_LIFETIME_SECONDS: i64 = 24;
const REFRESH_LIFETIME_SECONDS: i64 = 120;

#[derive(Clone)]
pub(super) struct BrowserIssuer {
    issuer: String,
    redirect_uri: Url,
    exchange: Arc<Mutex<Exchange>>,
}

enum Exchange {
    Ready,
    Code {
        value: String,
        nonce: String,
        challenge: String,
    },
    Refresh {
        credential: String,
    },
    Complete,
}

impl BrowserIssuer {
    pub(super) fn new(issuer: &str, redirect_uri: Url) -> Self {
        Self {
            issuer: issuer.into(),
            redirect_uri,
            exchange: Arc::new(Mutex::new(Exchange::Ready)),
        }
    }

    pub(super) fn routes(&self) -> Router {
        Router::new()
            .route("/realm/authorize", get(authorize))
            .route("/realm/token", post(token))
            .with_state(self.clone())
    }

    pub(super) fn completed(&self) -> bool {
        matches!(*self.exchange.lock().unwrap(), Exchange::Complete)
    }
}

#[derive(Deserialize)]
struct AuthorizationRequest {
    client_id: String,
    redirect_uri: String,
    response_type: String,
    state: String,
    nonce: String,
    code_challenge: String,
    code_challenge_method: String,
}

async fn authorize(
    State(issuer): State<BrowserIssuer>,
    Query(query): Query<AuthorizationRequest>,
) -> Result<Redirect, StatusCode> {
    if query.client_id != CLIENT
        || query.redirect_uri != issuer.redirect_uri.as_str()
        || query.response_type != "code"
        || query.code_challenge_method != "S256"
        || query.code_challenge.len() != 43
        || query.state.is_empty()
        || query.nonce.is_empty()
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut exchange = issuer.exchange.lock().unwrap();
    if !matches!(*exchange, Exchange::Ready) {
        return Err(StatusCode::CONFLICT);
    }
    let code = Uuid::new_v4().to_string();
    let mut callback = issuer.redirect_uri.clone();
    callback
        .query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("state", &query.state);
    *exchange = Exchange::Code {
        value: code,
        nonce: query.nonce,
        challenge: query.code_challenge,
    };
    Ok(Redirect::to(callback.as_str()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenRequest {
    client_id: String,
    grant_type: String,
    code: Option<String>,
    code_verifier: Option<String>,
    redirect_uri: Option<String>,
    refresh_token: Option<String>,
    #[serde(rename = "scope")]
    _scope: Option<String>,
}

async fn token(
    State(issuer): State<BrowserIssuer>,
    headers: HeaderMap,
    Form(request): Form<TokenRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let invalid = || {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant"})),
        )
    };
    if request.client_id != CLIENT || headers.contains_key(header::AUTHORIZATION) {
        return Err(invalid());
    }
    let (nonce, roles, lifetime, credential) = {
        let mut exchange = issuer.exchange.lock().unwrap();
        match (&*exchange, request.grant_type.as_str()) {
            (
                Exchange::Code {
                    value,
                    nonce,
                    challenge,
                },
                "authorization_code",
            ) if request.code.as_deref() == Some(value.as_str())
                && request.redirect_uri.as_deref() == Some(issuer.redirect_uri.as_str())
                && request.code_verifier.as_ref().is_some_and(|verifier| {
                    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) == *challenge
                }) =>
            {
                let nonce = nonce.clone();
                let credential = Uuid::new_v4().to_string();
                *exchange = Exchange::Refresh {
                    credential: credential.clone(),
                };
                (
                    Some(nonce),
                    &[ADMIN_ROLE][..],
                    LOGIN_LIFETIME_SECONDS,
                    credential,
                )
            }
            (Exchange::Refresh { credential }, "refresh_token")
                if request.refresh_token.as_deref() == Some(credential.as_str()) =>
            {
                *exchange = Exchange::Complete;
                (
                    None,
                    &[][..],
                    REFRESH_LIFETIME_SECONDS,
                    Uuid::new_v4().to_string(),
                )
            }
            _ => return Err(invalid()),
        }
    };
    let id = Uuid::new_v4().to_string();
    let mut response = json!({
        "token_type": "Bearer", "expires_in": lifetime, "refresh_token": credential,
        "access_token": signed_token(&issuer.issuer, CLIENT, SUBJECT, None, roles, &id, lifetime)
    });
    if let Some(nonce) = nonce {
        response["id_token"] = json!(signed_token(
            &issuer.issuer,
            CLIENT,
            SUBJECT,
            Some(&nonce),
            roles,
            &id,
            lifetime
        ));
    }
    Ok(Json(response))
}
