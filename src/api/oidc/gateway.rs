use super::*;
use crate::oidc::{OidcClientKind, OidcError, TokenBundle};

#[cfg(test)]
#[path = "admission_tests.rs"]
mod admission_tests;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Version {
    version: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Admission {
    version: u32,
    intent_id: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rotation {
    version: u32,
    intent_id: Uuid,
    authentication_id: Uuid,
    generation: u32,
    access_token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Refresh {
    version: u32,
    authentication_id: Uuid,
    generation: u32,
    refresh_token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Logout {
    version: u32,
    authentication_id: Uuid,
    generation: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeRequest {
    version: u32,
    action: NativeAction,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum NativeAction {
    Login,
    Refresh {
        authentication_id: Uuid,
        generation: u32,
    },
    Finish {
        intent_id: Uuid,
        outcome: NativeOutcome,
    },
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum NativeOutcome {
    Cancelled,
    Failed,
}

fn configured(state: &GatewayState) -> ApiResult<Arc<OidcGateway>> {
    state.oidc.clone().ok_or_else(not_configured)
}
fn web(oidc: &OidcGateway, headers: &HeaderMap) -> ApiResult<()> {
    if same_origin(headers, &oidc.config.web_origin) {
        Ok(())
    } else {
        Err(origin_denied())
    }
}
fn native(headers: &HeaderMap) -> ApiResult<()> {
    if headers.contains_key(header::ORIGIN)
        || headers
            .get("sec-fetch-site")
            .is_some_and(|site| site != "none")
    {
        Err(origin_denied())
    } else {
        Ok(())
    }
}
async fn json_body<T: serde::de::DeserializeOwned>(request: Request) -> ApiResult<T> {
    if request.uri().query().is_some() {
        return Err(invalid_request());
    }
    let mut types = request.headers().get_all(header::CONTENT_TYPE).iter();
    if !types
        .next()
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value == "application/json" || value == "application/json; charset=utf-8"
        })
        || types.next().is_some()
    {
        return Err(invalid_request());
    }
    let bytes = tokio::time::timeout(
        Duration::from_secs(5),
        axum::body::to_bytes(request.into_body(), 35_000),
    )
    .await
    .map_err(|_| invalid_request())?
    .map_err(|_| invalid_request())?;
    serde_json::from_slice(&bytes).map_err(|_| invalid_request())
}
fn record(
    oidc: &OidcGateway,
    action: Action,
    session: Uuid,
    client: &str,
    principal: Option<PrincipalRefV1>,
    transaction: Option<Uuid>,
    operation: Uuid,
) -> AuthenticationRecordV1 {
    AuthenticationRecordV1 {
        operation_id: operation,
        action,
        phase: Phase::Intention,
        issuer: oidc.config.provider.issuer.clone(),
        client_id: client.into(),
        principal,
        transaction_id: transaction,
        session_id: Some(session),
        causal: None,
    }
}
fn result(
    mut record: AuthenticationRecordV1,
    intention: AuthenticationReceiptV1,
    outcome: Outcome,
    reason: Reason,
    principal: Option<PrincipalRefV1>,
) -> AuthenticationRecordV1 {
    record.phase = Phase::Result { outcome, reason };
    record.principal = principal;
    record.causal = Some(intention);
    record
}
fn transaction_record(
    oidc: &OidcGateway,
    transaction: &BrowserTransaction,
) -> AuthenticationRecordV1 {
    record(
        oidc,
        Action::Login,
        transaction.session_id,
        &oidc.config.provider.web_client_id,
        None,
        Some(transaction.transaction_id),
        transaction.operation_id,
    )
}
fn native_record(oidc: &OidcGateway, intent: &NativeIntent) -> AuthenticationRecordV1 {
    record(
        oidc,
        intent.action,
        intent.session_id,
        &oidc.config.provider.cli_client_id,
        intent
            .previous
            .as_ref()
            .map(|binding| binding.principal.clone()),
        None,
        intent.operation_id,
    )
}
fn provider_error(error: OidcError) -> (StatusCode, Json<Value>) {
    match error {
        OidcError::Capacity => capacity(),
        OidcError::TransportUnavailable | OidcError::InvalidMetadata | OidcError::InvalidKeys => {
            human::failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "provider_unavailable",
                "The configured identity provider is unavailable.",
                "authenticate",
            )
        }
        _ => credential_invalid(),
    }
}
fn provider_result(error: OidcError) -> (Outcome, Reason) {
    match error {
        OidcError::TransportUnavailable => (Outcome::Unknown, Reason::ProviderUnavailable),
        OidcError::ProviderRejected => (Outcome::Rejected, Reason::ProviderRejected),
        OidcError::Capacity => (Outcome::Unknown, Reason::Interrupted),
        _ => (Outcome::Rejected, Reason::InvalidResponse),
    }
}
fn session_dto(binding: &SessionBinding) -> Value {
    json!({"version":1,"authentication_id":binding.session_id,"generation":binding.generation,"expires_at":expiry(binding.expires_at_unix_seconds)})
}
fn expiry(seconds: i64) -> String {
    chrono::DateTime::from_timestamp(seconds, 0)
        .map(|value| value.to_rfc3339())
        .unwrap_or_default()
}
fn raw_digest(raw: &str) -> [u8; 32] {
    Sha256::digest(raw.as_bytes()).into()
}
fn delivery(binding: &SessionBinding, tokens: TokenBundle) -> ApiResult<TokenDelivery> {
    let remaining = binding
        .expires_at_unix_seconds
        .saturating_sub(chrono::Utc::now().timestamp());
    if remaining <= 0 {
        return Err(credential_invalid());
    }
    Ok(TokenDelivery {
        version: 1,
        authentication_id: binding.session_id.to_string(),
        generation: binding.generation,
        token_type: "Bearer",
        access_token: tokens.access_token().into(),
        refresh_token: tokens.refresh_token().map(str::to_owned),
        expires_at: expiry(binding.expires_at_unix_seconds),
        expires_in_seconds: remaining as u64,
    })
}
fn membership(
    state: &GatewayState,
    oidc: &OidcGateway,
    access: &VerifiedAccess,
) -> ApiResult<BTreeSet<HumanCapabilityV1>> {
    let principal = PrincipalRefV1::HumanOidc {
        issuer: access.issuer().into(),
        subject: access.subject().into(),
    };
    let ceiling = oidc
        .config
        .mapping
        .capabilities(access.claims())
        .map_err(|_| credential_invalid())?;
    let mut allowed = false;
    let mut unavailable = false;
    let mut admission_failure = None;
    for tenant in state.tenants.iter() {
        let profile = match state
            .io_scope
            .with_store(tenant, store_io::Lane::Read, true, |store| {
                store.human_profile(&principal, &state.redactor)
            }) {
            Ok(profile) => profile,
            Err(error) => {
                unavailable = true;
                admission_failure = Some(error);
                continue;
            }
        };
        let profile = match profile {
            Ok(profile) => profile,
            Err(_) => {
                unavailable = true;
                continue;
            }
        };
        if profile.account.is_some()
            && profile.memberships.iter().any(|member| {
                member.active && member.capabilities.intersection(&ceiling).next().is_some()
            })
        {
            allowed = true;
        }
    }
    if !allowed {
        if let Some(error) = admission_failure {
            return Err(error);
        }
        if unavailable {
            return Err(audit_unavailable());
        }
        return Err(human::failure(
            StatusCode::FORBIDDEN,
            "permission_denied",
            "The verified account has no approved membership and mapped capability.",
            "contact_operator",
        ));
    }
    Ok(ceiling)
}
fn reserve(registry: &mut Registry, new_session: bool) -> ApiResult<()> {
    if registry.transactions.len()
        + registry.deliveries.len()
        + registry.native.len()
        + registry.exchanging.len()
        >= 256
        || new_session
            && registry.sessions.len()
                + registry.transactions.len()
                + registry.native.len()
                + registry.exchanging.len()
                >= 4096
    {
        return Err(capacity());
    }
    let rate = registry.rate.get_or_insert((Instant::now(), 0));
    if rate.0.elapsed() >= Duration::from_secs(1) {
        *rate = (Instant::now(), 0);
    }
    if rate.1 >= 16 {
        return Err(capacity());
    }
    rate.1 += 1;
    Ok(())
}
fn fail_initial(
    oidc: &OidcGateway,
    record: AuthenticationRecordV1,
    receipt: AuthenticationReceiptV1,
    error: (StatusCode, Json<Value>),
    principal: Option<PrincipalRefV1>,
) -> (StatusCode, Json<Value>) {
    let (outcome, reason, verified) = if error.0 == StatusCode::FORBIDDEN {
        (Outcome::Rejected, Reason::NoMembership, principal)
    } else {
        (Outcome::Rejected, Reason::InvalidResponse, None)
    };
    match oidc
        .audit
        .append_typed(result(record, receipt, outcome, reason, verified))
    {
        Ok(_) => error,
        Err(error) => audit_failure(error),
    }
}
struct NewAdmission<'a> {
    record: AuthenticationRecordV1,
    access: VerifiedAccess,
    raw: &'a str,
    refresh: Option<&'a str>,
    logout_context: Option<OidcLogoutContext>,
    kind: SessionKind,
    browser_transaction: Option<([u8; 32], Uuid)>,
}
fn install(
    state: &GatewayState,
    oidc: &OidcGateway,
    admission: NewAdmission<'_>,
) -> ApiResult<SessionBinding> {
    let NewAdmission {
        record,
        access,
        raw,
        refresh,
        logout_context,
        kind,
        browser_transaction,
    } = admission;
    let session_id = record.session_id.ok_or_else(invalid_request)?;
    // The blocking queue cannot extend the lifetime of the provider proof or
    // its browser exchange marker. Recheck before the first durable intention;
    // the registry check below is repeated at the actual session mutation.
    if access.expires_at() <= chrono::Utc::now().timestamp() {
        return Err(credential_invalid());
    }
    if let Some((key, transaction)) = browser_transaction {
        let registry = oidc.registry.lock().map_err(|_| audit_unavailable())?;
        if registry.exchanging.get(&key).is_none_or(|entry| {
            entry.transaction_id != transaction
                || entry.session_id != session_id
                || entry.created.elapsed() >= Duration::from_secs(35)
        }) {
            return Err(conflict());
        }
    }
    // VerifiedAccess is produced only by the configured provider's verifier.
    // Reserve durable audit capacity now, before membership reads (which can
    // themselves produce business audit) or any session admission/mutation.
    // Anonymous nonce creation and rejected provider input never reach here.
    let receipt = oidc
        .audit
        .append_typed(record.clone())
        .map_err(audit_failure)?;
    if state.credential_index.lookup(raw).is_some()
        || human::reject_control_collision(state, raw, None).is_err()
    {
        return Err(fail_initial(
            oidc,
            record,
            receipt,
            credential_invalid(),
            None,
        ));
    }
    let principal = PrincipalRefV1::HumanOidc {
        issuer: access.issuer().into(),
        subject: access.subject().into(),
    };
    let ceiling = match membership(state, oidc, &access) {
        Ok(value) => value,
        Err(error) => return Err(fail_initial(oidc, record, receipt, error, Some(principal))),
    };
    let access_fingerprint = Fingerprint::access_token(raw);
    if oidc
        .audit
        .fingerprint_seen(&access_fingerprint)
        .map_err(audit_failure)?
    {
        oidc.audit
            .append_typed(result(
                record,
                receipt,
                Outcome::Rejected,
                Reason::Revoked,
                None,
            ))
            .map_err(audit_failure)?;
        return Err(conflict());
    }
    let mut registry = oidc.registry.lock().map_err(|_| audit_unavailable())?;
    if let Some((key, transaction)) = browser_transaction
        && registry.exchanging.get(&key).is_none_or(|entry| {
            entry.transaction_id != transaction
                || entry.session_id != session_id
                || entry.created.elapsed() >= Duration::from_secs(35)
        })
    {
        return Err(conflict());
    }
    if registry.sessions.len() >= 4096
        || registry
            .sessions
            .values()
            .filter(|session| session.proof.principal == principal)
            .count()
            >= 64
    {
        return Err(fail_initial(
            oidc,
            record,
            receipt,
            capacity(),
            Some(principal),
        ));
    }
    let now = chrono::Utc::now().timestamp();
    let authenticated_at = Instant::now();
    let maximum_expiry = now.saturating_add(3600);
    let expires_at = access.expires_at().min(maximum_expiry);
    if expires_at <= now {
        return Err(fail_initial(
            oidc,
            record,
            receipt,
            credential_invalid(),
            Some(principal),
        ));
    }
    let binding = SessionBinding {
        session_id,
        generation: 1,
        principal: principal.clone(),
        issuer: access.issuer().into(),
        client_id: access.client_id().into(),
        kind,
        access_fingerprint,
        refresh_fingerprint: refresh.map(Fingerprint::refresh_token),
        expires_at_unix_seconds: expires_at,
    };
    oidc.audit
        .admit_session(
            result(
                record,
                receipt,
                Outcome::Succeeded,
                Reason::Verified,
                Some(principal.clone()),
            ),
            binding.clone(),
        )
        .map_err(audit_failure)?;
    let proof = OidcHumanProof {
        principal,
        authentication_id: session_id.to_string(),
        generation: 1,
        digest: raw_digest(raw),
        expires_at,
        deadline: authenticated_at
            + Duration::from_secs(expires_at.saturating_sub(now).max(0) as u64),
        ceiling,
        mapping_revision: oidc.config.mapping.revision.clone(),
    };
    registry.sessions.insert(
        session_id.to_string(),
        Session {
            proof,
            binding: binding.clone(),
            access: Arc::new(access),
            logout_context,
            created: Instant::now(),
            used: Instant::now(),
            rotating: false,
            pending_refresh: None,
            maximum_expiry,
        },
    );
    Ok(binding)
}
fn close_record(
    oidc: &OidcGateway,
    session: &Session,
    action: Action,
    reason: Reason,
) -> ApiResult<()> {
    let intention = record(
        oidc,
        action,
        session.binding.session_id,
        &session.binding.client_id,
        Some(session.binding.principal.clone()),
        None,
        Uuid::new_v4(),
    );
    let receipt = oidc
        .audit
        .append_typed(intention.clone())
        .map_err(audit_failure)?;
    oidc.audit
        .close_session(
            result(
                intention,
                receipt,
                Outcome::Succeeded,
                reason,
                Some(session.binding.principal.clone()),
            ),
            session.binding.generation,
            session.binding.access_fingerprint,
        )
        .map_err(audit_failure)?;
    Ok(())
}
pub(super) fn reap(oidc: &OidcGateway, registry: &mut Registry) -> ApiResult<()> {
    let expired: Vec<_> = registry
        .exchanging
        .iter()
        .filter(|(_, entry)| entry.created.elapsed() >= Duration::from_secs(35))
        .map(|(id, _)| *id)
        .collect();
    for id in expired {
        if let Some(entry) = registry.exchanging.remove(&id)
            && let Some(session) = registry.sessions.remove(&entry.session_id.to_string())
        {
            close_record(oidc, &session, Action::Expiration, Reason::Revoked)?;
        }
    }
    // Unverified attempts release their volatile budget only. An installed
    // session is handled above by its exchange marker or below by its lifetime.
    registry.transactions.retain(|_, entry| {
        entry.created.elapsed() < Duration::from_secs(oidc.config.provider.limits.pkce_ttl_seconds)
    });
    registry
        .deliveries
        .retain(|_, entry| entry.created.elapsed() < Duration::from_secs(30));
    let expired: Vec<_> = registry
        .native
        .iter()
        .filter(|(_, entry)| {
            entry.created.elapsed()
                >= Duration::from_secs(if entry.previous.is_some() { 15 } else { 300 })
        })
        .map(|(id, _)| *id)
        .collect();
    for id in expired {
        if let Some(entry) = registry.native.remove(&id) {
            let previous = entry.previous.clone();
            if let Some(binding) = previous {
                let receipt = entry.intention.clone().ok_or_else(audit_unavailable)?;
                registry.sessions.remove(&binding.session_id.to_string());
                oidc.audit
                    .close_session(
                        result(
                            native_record(oidc, &entry),
                            receipt,
                            Outcome::Unknown,
                            Reason::Interrupted,
                            Some(binding.principal.clone()),
                        ),
                        binding.generation,
                        binding.access_fingerprint,
                    )
                    .map_err(audit_failure)?;
            }
        }
    }
    let abandoned: Vec<_> = registry
        .sessions
        .iter()
        .filter(|(_, session)| {
            session.rotating
                && session
                    .pending_refresh
                    .as_ref()
                    .is_none_or(|pending| Instant::now() >= pending.deadline)
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in abandoned {
        if let Some(session) = registry.sessions.remove(&id) {
            registry
                .native
                .retain(|_, intent| intent.session_id != session.binding.session_id);
            let pending = session.pending_refresh.ok_or_else(audit_unavailable)?;
            oidc.audit
                .close_session(
                    result(
                        pending.record,
                        pending.receipt,
                        Outcome::Unknown,
                        Reason::Interrupted,
                        Some(session.binding.principal.clone()),
                    ),
                    session.binding.generation,
                    session.binding.access_fingerprint,
                )
                .map_err(audit_failure)?;
        }
    }
    let now = chrono::Utc::now().timestamp();
    let expired: Vec<_> = registry
        .sessions
        .iter()
        .filter(|(_, session)| {
            session.binding.expires_at_unix_seconds <= now
                || session.proof.deadline <= Instant::now()
                || session.created.elapsed() >= Duration::from_secs(3600)
                || session.used.elapsed() >= Duration::from_secs(900)
        })
        .filter(|(_, session)| !session.rotating)
        .map(|(id, _)| id.clone())
        .collect();
    for id in expired {
        if let Some(session) = registry.sessions.remove(&id) {
            close_record(oidc, &session, Action::Expiration, Reason::Expired)?;
        }
    }
    Ok(())
}

pub(super) async fn start(
    State(state): State<GatewayState>,
    request: Request,
) -> ApiResult<Response> {
    let oidc = configured(&state)?;
    web(&oidc, request.headers())?;
    let correlation = correlation_cookie(request.headers())?.ok_or_else(invalid_request)?;
    let body: Version = json_body(request).await?;
    if body.version != 1 {
        return Err(invalid_request());
    }
    let key = raw_digest(&correlation);
    let tx = Uuid::new_v4();
    let operation = Uuid::new_v4();
    let session = Uuid::new_v4();
    let preparation = Arc::clone(&oidc);
    state
        .blocking_identity(move |_state| {
            let oidc = preparation;
            let mut registry = oidc.registry.lock().map_err(|_| audit_unavailable())?;
            reap(&oidc, &mut registry)?;
            if registry.transactions.contains_key(&key)
                || registry.deliveries.contains_key(&key)
                || registry.exchanging.contains_key(&key)
            {
                return Err(conflict());
            }
            reserve(&mut registry, true)?;
            // A caller-created correlation cookie is not an authenticated identity.
            // The durable intention is created only by install after verification.
            registry.transactions.insert(
                key,
                BrowserTransaction {
                    pending: None,
                    expected_state: [0; 32],
                    transaction_id: tx,
                    operation_id: operation,
                    session_id: session,
                    created: Instant::now(),
                },
            );
            Ok(())
        })
        .await?;
    let pending = async {
        oidc.provider.discover().await?;
        oidc.provider
            .begin_login(OidcClientKind::Web, &oidc.config.provider.web_redirect_uri)
            .await
    }
    .await;
    state
        .blocking_identity(move |_state| {
            let mut registry = oidc.registry.lock().map_err(|_| audit_unavailable())?;
            match pending {
                Ok(pending) => {
                    let url = pending.authorization_url().to_owned();
                    let state = reqwest::Url::parse(&url)
                        .ok()
                        .and_then(|url| {
                            url.query_pairs()
                                .find(|(key, _)| key == "state")
                                .map(|(_, value)| value.into_owned())
                        })
                        .ok_or_else(invalid_request)?;
                    let entry = registry
                        .transactions
                        .get_mut(&key)
                        .filter(|entry| {
                            entry.transaction_id == tx
                                && entry.created.elapsed()
                                    < Duration::from_secs(
                                        oidc.config.provider.limits.pkce_ttl_seconds,
                                    )
                        })
                        .ok_or_else(conflict)?;
                    entry.expected_state = raw_digest(&state);
                    entry.pending = Some(pending);
                    let mut response =
                        Json(json!({"version":1,"authorization_url":url})).into_response();
                    response.headers_mut().insert(
                        header::SET_COOKIE,
                        HeaderValue::from_str(&cookie(
                            &correlation,
                            oidc.config.web_origin.starts_with("https://"),
                            false,
                        ))
                        .map_err(|_| invalid_request())?,
                    );
                    Ok(response)
                }
                Err(error) => {
                    registry.transactions.remove(&key);
                    Err(provider_error(error))
                }
            }
        })
        .await
}

pub(super) async fn callback(
    State(state): State<GatewayState>,
    request: Request,
) -> ApiResult<Response> {
    let oidc = configured(&state)?;
    let correlation = correlation_cookie(request.headers())?.ok_or_else(invalid_request)?;
    let key = raw_digest(&correlation);
    let query = request
        .uri()
        .query()
        .filter(|query| query.len() <= 4096)
        .ok_or_else(invalid_request)?;
    let mut params = HashMap::new();
    for (name, value) in reqwest::Url::parse(&format!("http://127.0.0.1/?{query}"))
        .map_err(|_| invalid_request())?
        .query_pairs()
    {
        if ![
            "state",
            "code",
            "error",
            "error_description",
            "error_uri",
            "session_state",
            "iss",
        ]
        .contains(&name.as_ref())
            || value.len() > 2048
            || params
                .insert(name.into_owned(), value.into_owned())
                .is_some()
        {
            return Err(invalid_request());
        }
    }
    let state_value = params
        .get("state")
        .filter(|value| safe_text(value, 256))
        .ok_or_else(invalid_request)?;
    let expected_state = raw_digest(state_value);
    let preparation = Arc::clone(&oidc);
    let transaction = state
        .blocking_identity(move |_state| {
            let oidc = preparation;
            let mut registry = oidc.registry.lock().map_err(|_| audit_unavailable())?;
            let entry = registry.transactions.get(&key).ok_or_else(conflict)?;
            if entry.pending.is_none() || entry.expected_state != expected_state {
                return Err(invalid_request());
            }
            let transaction = registry.transactions.remove(&key).ok_or_else(conflict)?;
            registry.exchanging.insert(
                key,
                ExchangeMarker {
                    transaction_id: transaction.transaction_id,
                    session_id: transaction.session_id,
                    created: Instant::now(),
                },
            );
            Ok(transaction)
        })
        .await?;
    let record = transaction_record(&oidc, &transaction);
    let expired = transaction.created.elapsed()
        >= Duration::from_secs(oidc.config.provider.limits.pkce_ttl_seconds);
    let valid = params
        .get("iss")
        .is_none_or(|issuer| issuer == &oidc.config.provider.issuer)
        && params.contains_key("code") != params.contains_key("error")
        && params.get("code").is_none_or(|code| safe_text(code, 2048));
    let outcome = if expired || !valid || params.contains_key("error") {
        Delivery::Error(if expired {
            "credential_expired"
        } else {
            "login_cancelled"
        })
    } else {
        let pending = transaction.pending.ok_or_else(conflict)?;
        match oidc
            .provider
            .complete_login(pending, &params["code"], state_value)
            .await
        {
            Ok(login) => {
                let (access, tokens, logout) = login.into_session_parts();
                let transaction_id = transaction.transaction_id;
                let installed = state
                    .blocking_identity(move |state| {
                        let oidc = configured(&state)?;
                        let installed = install(
                            &state,
                            &oidc,
                            NewAdmission {
                                record,
                                access,
                                raw: tokens.access_token(),
                                refresh: tokens.refresh_token(),
                                logout_context: logout,
                                kind: SessionKind::WebPkce,
                                browser_transaction: Some((key, transaction_id)),
                            },
                        );
                        installed.and_then(|binding| delivery(&binding, tokens))
                    })
                    .await;
                match installed {
                    Ok(tokens) => Delivery::Tokens(tokens),
                    Err((_, Json(error))) => Delivery::Error(
                        error["error"]["code"]
                            .as_str()
                            .and_then(safe_code)
                            .unwrap_or("identity_unavailable"),
                    ),
                }
            }
            Err(error) => Delivery::Error(if matches!(error, OidcError::TransportUnavailable) {
                "provider_unavailable"
            } else {
                "credential_invalid"
            }),
        }
    };
    state
        .blocking_identity(move |_state| {
            let mut registry = oidc.registry.lock().map_err(|_| audit_unavailable())?;
            if registry
                .exchanging
                .remove(&key)
                .is_none_or(|entry| entry.transaction_id != transaction.transaction_id)
            {
                return Err(conflict());
            }
            if registry.deliveries.contains_key(&key) {
                return Err(conflict());
            }
            registry.deliveries.insert(
                key,
                PendingDelivery {
                    result: outcome,
                    created: Instant::now(),
                },
            );
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::SEE_OTHER;
            response.headers_mut().insert(
                header::LOCATION,
                HeaderValue::from_static("/auth/oidc/complete"),
            );
            Ok(response)
        })
        .await
}
fn safe_code(code: &str) -> Option<&'static str> {
    match code {
        "audit_unavailable" => Some("audit_unavailable"),
        "permission_denied" => Some("permission_denied"),
        "session_conflict" => Some("session_conflict"),
        "capacity_exhausted" => Some("capacity_exhausted"),
        "credential_invalid" => Some("credential_invalid"),
        _ => None,
    }
}
pub(super) async fn tokens(
    State(state): State<GatewayState>,
    request: Request,
) -> ApiResult<Response> {
    let oidc = configured(&state)?;
    web(&oidc, request.headers())?;
    let correlation = correlation_cookie(request.headers())?.ok_or_else(invalid_request)?;
    let key = raw_digest(&correlation);
    let body: Version = json_body(request).await?;
    if body.version != 1 {
        return Err(invalid_request());
    }
    state
        .blocking_identity(move |_state| {
            let entry = oidc
                .registry
                .lock()
                .map_err(|_| audit_unavailable())?
                .deliveries
                .remove(&key)
                .ok_or_else(conflict)?;
            let mut response = if entry.created.elapsed() >= Duration::from_secs(30) {
                credential_invalid().into_response()
            } else {
                match entry.result {
                    Delivery::Tokens(tokens) => Json(tokens).into_response(),
                    Delivery::Error(code) => human::failure(
                        if code == "permission_denied" {
                            StatusCode::FORBIDDEN
                        } else if code == "audit_unavailable" {
                            StatusCode::SERVICE_UNAVAILABLE
                        } else {
                            StatusCode::UNAUTHORIZED
                        },
                        code,
                        "The correlated login did not establish an authorized session.",
                        "authenticate",
                    )
                    .into_response(),
                }
            };
            response.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_str(&cookie(
                    "",
                    oidc.config.web_origin.starts_with("https://"),
                    true,
                ))
                .map_err(|_| invalid_request())?,
            );
            Ok(response)
        })
        .await
}

pub(super) async fn native_intent(
    State(state): State<GatewayState>,
    request: Request,
) -> ApiResult<Json<Value>> {
    let oidc = configured(&state)?;
    native(request.headers())?;
    let bearer = human_bearer(request.headers()).map(str::to_owned);
    let body: NativeRequest = json_body(request).await?;
    if body.version != 1 {
        return Err(invalid_request());
    }
    state
        .blocking_identity(move |state| native_intent_sync(state, oidc, bearer, body))
        .await
}
fn native_intent_sync(
    state: GatewayState,
    oidc: Arc<OidcGateway>,
    bearer: Option<String>,
    body: NativeRequest,
) -> ApiResult<Json<Value>> {
    match body.action {
        NativeAction::Login => {
            let intent_id = Uuid::new_v4();
            let session_id = Uuid::new_v4();
            let mut registry = oidc.registry.lock().map_err(|_| audit_unavailable())?;
            reap(&oidc, &mut registry)?;
            reserve(&mut registry, true)?;
            registry.native.insert(
                intent_id,
                NativeIntent {
                    session_id,
                    operation_id: intent_id,
                    intention: None,
                    action: Action::SessionAdmission,
                    previous: None,
                    created: Instant::now(),
                },
            );
            Ok(Json(
                json!({"version":1,"intent_id":intent_id,"expires_at":expiry(chrono::Utc::now().timestamp()+300)}),
            ))
        }
        NativeAction::Refresh {
            authentication_id,
            generation,
        } => {
            let raw = bearer.ok_or_else(credential_invalid)?;
            human::reject_control_collision(&state, &raw, None)?;
            if state.credential_index.lookup(&raw).is_some() {
                return Err(credential_invalid());
            }
            let mut previous = begin_refresh(
                &oidc,
                &raw,
                authentication_id,
                generation,
                SessionKind::NativeCli,
                None,
            )?;
            let intent_id = previous.record.operation_id;
            let mut registry = oidc.registry.lock().map_err(|_| audit_unavailable())?;
            // Sample UTC before the monotonic remainder and round down; lock
            // or processing time cannot extend the published ticket lifetime.
            let now = chrono::Utc::now().timestamp();
            let remaining = previous
                .completion_deadline
                .and_then(|deadline| deadline.checked_duration_since(Instant::now()))
                .filter(|remaining| remaining.as_secs() > 0)
                .ok_or_else(credential_invalid)?;
            let expires_at = expiry(now.saturating_add(remaining.as_secs() as i64));
            registry.native.insert(
                intent_id,
                NativeIntent {
                    session_id: authentication_id,
                    operation_id: intent_id,
                    intention: Some(previous.receipt.clone()),
                    action: Action::Refresh,
                    previous: Some(previous.binding.clone()),
                    created: previous.created,
                },
            );
            previous.cancel_on_drop = false;
            Ok(Json(
                json!({"version":1,"intent_id":intent_id,"expires_at":expires_at}),
            ))
        }
        NativeAction::Finish { intent_id, outcome } => {
            let mut registry = oidc.registry.lock().map_err(|_| audit_unavailable())?;
            let entry = registry.native.get(&intent_id).ok_or_else(conflict)?;
            if let Some(binding) = &entry.previous {
                let raw = bearer.ok_or_else(credential_invalid)?;
                if Fingerprint::access_token(&raw) != binding.access_fingerprint
                    || entry.created.elapsed() >= Duration::from_secs(15)
                {
                    return Err(credential_invalid());
                }
            } else if entry.created.elapsed() >= Duration::from_secs(300) {
                return Err(conflict());
            }
            let entry = registry.native.remove(&intent_id).ok_or_else(conflict)?;
            let _declared_outcome = match outcome {
                NativeOutcome::Cancelled => "cancelled",
                NativeOutcome::Failed => "failed",
            };
            let record = native_record(&oidc, &entry);
            if let Some(binding) = &entry.previous {
                let receipt = entry.intention.ok_or_else(audit_unavailable)?;
                registry.sessions.remove(&binding.session_id.to_string());
                oidc.audit
                    .close_session(
                        result(
                            record,
                            receipt,
                            Outcome::Unknown,
                            Reason::Interrupted,
                            Some(binding.principal.clone()),
                        ),
                        binding.generation,
                        binding.access_fingerprint,
                    )
                    .map_err(audit_failure)?;
            }
            Ok(Json(json!({"version":1,"status":"finished"})))
        }
    }
}
pub(super) async fn admit(
    State(state): State<GatewayState>,
    request: Request,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let oidc = configured(&state)?;
    native(request.headers())?;
    let raw = human_bearer(request.headers())
        .ok_or_else(credential_invalid)?
        .to_owned();
    let body: Admission = json_body(request).await?;
    if body.version != 1 {
        return Err(invalid_request());
    }
    let preparation = Arc::clone(&oidc);
    let intent = state
        .blocking_identity(move |_state| {
            let oidc = preparation;
            let mut registry = oidc.registry.lock().map_err(|_| audit_unavailable())?;
            let entry = registry.native.get(&body.intent_id).ok_or_else(conflict)?;
            if entry.action != Action::SessionAdmission
                || entry.previous.is_some()
                || entry.created.elapsed() >= Duration::from_secs(300)
            {
                return Err(conflict());
            }
            let intent = registry
                .native
                .remove(&body.intent_id)
                .ok_or_else(conflict)?;
            Ok(intent)
        })
        .await?;
    let record = native_record(&oidc, &intent);
    let deadline = intent.created + Duration::from_secs(300);
    let access = match oidc.provider.verify_access(&raw).await {
        Ok(access) if access.client_id() == oidc.config.provider.cli_client_id => access,
        Ok(_) => return Err(credential_invalid()),
        Err(error) => return Err(provider_error(error)),
    };
    state
        .blocking_identity(move |state| {
            if Instant::now() >= deadline {
                return Err(conflict());
            }
            let binding = install(
                &state,
                &oidc,
                NewAdmission {
                    record,
                    access,
                    raw: &raw,
                    refresh: None,
                    logout_context: None,
                    kind: SessionKind::NativeCli,
                    browser_transaction: None,
                },
            )?;
            Ok((StatusCode::CREATED, Json(session_dto(&binding))))
        })
        .await
}

struct RefreshOperation {
    record: AuthenticationRecordV1,
    receipt: AuthenticationReceiptV1,
    binding: SessionBinding,
    access: Arc<VerifiedAccess>,
    maximum_expiry: i64,
    completion_deadline: Option<Instant>,
    created: Instant,
    owner: std::sync::Weak<OidcGateway>,
    cancel_on_drop: bool,
    cleanup: Option<store_io::CleanupReservation>,
    inline_cleanup: bool,
}
impl RefreshOperation {
    fn into_async(mut self) -> Self {
        self.inline_cleanup = false;
        self
    }
    fn enter_blocking(&mut self) {
        self.inline_cleanup = true;
    }
}
async fn complete_refresh<T, F>(
    state: &GatewayState,
    mut operation: RefreshOperation,
    completion: F,
) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(GatewayState, &RefreshOperation) -> ApiResult<T> + Send + 'static,
{
    // This obligation was reserved before refresh_pending/intent removal. Its
    // completion must not reacquire a lane that could refuse its terminal write.
    let reservation = operation.cleanup.take().ok_or_else(audit_unavailable)?;
    let state = state.clone();
    reservation
        .dispatch(move || {
            operation.enter_blocking();
            completion(state, &operation)
        })
        .await
        .map_err(|_| audit_unavailable())?
}
// No destructor on this payload: rejecting a queued task during shutdown must
// never execute SQLite on the thread that drops that task.
struct RefreshCleanup {
    record: AuthenticationRecordV1,
    receipt: AuthenticationReceiptV1,
    binding: SessionBinding,
    owner: std::sync::Weak<OidcGateway>,
}
impl RefreshCleanup {
    fn run(self) {
        let Some(owner) = self.owner.upgrade() else {
            return;
        };
        let removed = {
            let Ok(mut registry) = owner.registry.lock() else {
                return;
            };
            let owned = registry
                .sessions
                .get(&self.binding.session_id.to_string())
                .is_some_and(|session| {
                    session.binding == self.binding
                        && session.rotating
                        && session.pending_refresh.as_ref().is_some_and(|pending| {
                            pending.record.operation_id == self.record.operation_id
                                && pending.receipt == self.receipt
                        })
                });
            if !owned {
                return;
            }
            let removed = registry
                .sessions
                .remove(&self.binding.session_id.to_string())
                .is_some();
            registry.native.retain(|_, intent| {
                intent.session_id != self.binding.session_id
                    || intent.operation_id != self.record.operation_id
            });
            removed
        };
        if removed {
            // Cancellation cannot return an audit error to the dropped future.
            // Private authority is already gone even if this terminal write fails.
            let _ = owner.audit.close_session(
                result(
                    self.record.clone(),
                    self.receipt.clone(),
                    Outcome::Unknown,
                    Reason::Interrupted,
                    Some(self.binding.principal.clone()),
                ),
                self.binding.generation,
                self.binding.access_fingerprint,
            );
        }
    }
}
impl Drop for RefreshOperation {
    fn drop(&mut self) {
        if !self.cancel_on_drop {
            return;
        }
        let payload = RefreshCleanup {
            record: self.record.clone(),
            receipt: self.receipt.clone(),
            binding: self.binding.clone(),
            owner: self.owner.clone(),
        };
        if self.inline_cleanup {
            // Only private synchronous preparation/completion segments use this
            // mode. Crossing back to the HTTP future calls into_async first.
            payload.run();
        } else if let Some(reservation) = self.cleanup.take() {
            let _task = reservation.dispatch(move || payload.run());
        }
    }
}
fn begin_refresh(
    oidc: &Arc<OidcGateway>,
    raw: &str,
    id: Uuid,
    generation: u32,
    kind: SessionKind,
    refresh: Option<&str>,
) -> ApiResult<RefreshOperation> {
    let cleanup = store_io::CleanupReservation::reserve(&oidc.cleanup_slots)?;
    let mut registry = oidc.registry.lock().map_err(|_| audit_unavailable())?;
    reap(oidc, &mut registry)?;
    if kind == SessionKind::NativeCli {
        reserve(&mut registry, false)?;
    }
    let session = registry
        .sessions
        .get_mut(&id.to_string())
        .ok_or_else(credential_invalid)?;
    if session.binding.kind != kind
        || session.proof.digest != raw_digest(raw)
        || session.binding.access_fingerprint != Fingerprint::access_token(raw)
        || session.binding.expires_at_unix_seconds <= chrono::Utc::now().timestamp()
        || session.proof.deadline <= Instant::now()
    {
        return Err(credential_invalid());
    }
    if session.binding.generation != generation || session.rotating {
        return Err(conflict());
    }
    if generation == u32::MAX {
        let session = registry
            .sessions
            .remove(&id.to_string())
            .ok_or_else(credential_invalid)?;
        close_record(oidc, &session, Action::Expiration, Reason::Revoked)?;
        return Err(credential_invalid());
    }
    if kind == SessionKind::WebPkce
        && refresh.map(Fingerprint::refresh_token) != session.binding.refresh_fingerprint
    {
        return Err(credential_invalid());
    }
    oidc.active_binding(&session.binding)?;
    let record = record(
        oidc,
        Action::Refresh,
        id,
        &session.binding.client_id,
        Some(session.binding.principal.clone()),
        None,
        Uuid::new_v4(),
    );
    let pending = oidc
        .audit
        .refresh_pending(
            record.clone(),
            generation,
            session.binding.access_fingerprint,
        )
        .map_err(audit_failure)?;
    if !pending.inserted {
        return Err(conflict());
    }
    let created = Instant::now();
    let deadline = created
        + if kind == SessionKind::NativeCli {
            Duration::from_secs(15)
        } else {
            Duration::from_millis(oidc.config.provider.limits.request_timeout_ms)
        };
    session.rotating = true;
    session.pending_refresh = Some(PendingRefresh {
        record: record.clone(),
        receipt: pending.receipt.clone(),
        deadline,
    });
    Ok(RefreshOperation {
        record,
        receipt: pending.receipt,
        binding: session.binding.clone(),
        access: session.access.clone(),
        maximum_expiry: session.maximum_expiry,
        completion_deadline: Some(deadline),
        created,
        owner: Arc::downgrade(oidc),
        cancel_on_drop: true,
        cleanup: Some(cleanup),
        inline_cleanup: true,
    })
}
fn fail_refresh(
    oidc: &OidcGateway,
    operation: &RefreshOperation,
    outcome: Outcome,
    reason: Reason,
) -> ApiResult<()> {
    let mut registry = oidc.registry.lock().map_err(|_| audit_unavailable())?;
    if registry
        .sessions
        .get(&operation.binding.session_id.to_string())
        .is_some_and(|session| session.binding == operation.binding)
    {
        registry
            .sessions
            .remove(&operation.binding.session_id.to_string());
    }
    oidc.audit
        .close_session(
            result(
                operation.record.clone(),
                operation.receipt.clone(),
                outcome,
                reason,
                Some(operation.binding.principal.clone()),
            ),
            operation.binding.generation,
            operation.binding.access_fingerprint,
        )
        .map_err(audit_failure)?;
    Ok(())
}
struct RotatedAccess<'a> {
    access: VerifiedAccess,
    raw: &'a str,
    refresh: Option<&'a str>,
    logout: Option<OidcLogoutContext>,
}
fn install_rotation(
    state: &GatewayState,
    oidc: &OidcGateway,
    operation: &RefreshOperation,
    rotated: RotatedAccess<'_>,
) -> ApiResult<SessionBinding> {
    if operation
        .completion_deadline
        .is_some_and(|deadline| Instant::now() >= deadline)
    {
        fail_refresh(oidc, operation, Outcome::Rejected, Reason::Expired)?;
        return Err(credential_invalid());
    }
    let RotatedAccess {
        access,
        raw,
        refresh,
        logout,
    } = rotated;
    let principal = PrincipalRefV1::HumanOidc {
        issuer: access.issuer().into(),
        subject: access.subject().into(),
    };
    let validation = (|| {
        if principal != operation.binding.principal
            || access.issuer() != operation.binding.issuer
            || access.client_id() != operation.binding.client_id
            || state.credential_index.lookup(raw).is_some()
            || human::reject_control_collision(state, raw, None).is_err()
        {
            return Err(credential_invalid());
        }
        let ceiling = membership(state, oidc, &access)?;
        let expiry = access.expires_at().min(operation.maximum_expiry);
        if expiry <= chrono::Utc::now().timestamp() {
            return Err(credential_invalid());
        }
        let generation = operation
            .binding
            .generation
            .checked_add(1)
            .ok_or_else(credential_invalid)?;
        let access_fingerprint = Fingerprint::access_token(raw);
        if oidc
            .audit
            .fingerprint_seen(&access_fingerprint)
            .map_err(audit_failure)?
        {
            return Err(conflict());
        }
        let refresh_fingerprint = refresh.map(Fingerprint::refresh_token);
        if refresh_fingerprint.is_some()
            && refresh_fingerprint == operation.binding.refresh_fingerprint
        {
            return Err(credential_invalid());
        }
        Ok((
            ceiling,
            SessionBinding {
                session_id: operation.binding.session_id,
                generation,
                principal: principal.clone(),
                issuer: access.issuer().into(),
                client_id: access.client_id().into(),
                kind: operation.binding.kind,
                access_fingerprint,
                refresh_fingerprint,
                expires_at_unix_seconds: expiry,
            },
        ))
    })();
    let (ceiling, binding) = match validation {
        Ok(value) => value,
        Err(error) => {
            fail_refresh(
                oidc,
                operation,
                Outcome::Rejected,
                if error.0 == StatusCode::FORBIDDEN {
                    Reason::NoMembership
                } else {
                    Reason::InvalidResponse
                },
            )?;
            return Err(error);
        }
    };
    let mut registry = oidc.registry.lock().map_err(|_| audit_unavailable())?;
    let session = registry
        .sessions
        .get_mut(&binding.session_id.to_string())
        .ok_or_else(conflict)?;
    if session.binding != operation.binding
        || !session.rotating
        || session.pending_refresh.as_ref().is_none_or(|pending| {
            pending.record.operation_id != operation.record.operation_id
                || pending.receipt != operation.receipt
        })
    {
        return Err(conflict());
    }
    // Membership/JWKS verification may consume the remaining ticket lifetime.
    // Recheck the retained monotonic deadline at the durable install boundary.
    if operation
        .completion_deadline
        .is_some_and(|deadline| Instant::now() >= deadline)
        || session
            .pending_refresh
            .as_ref()
            .is_some_and(|pending| Instant::now() >= pending.deadline)
    {
        drop(registry);
        fail_refresh(oidc, operation, Outcome::Rejected, Reason::Expired)?;
        return Err(credential_invalid());
    }
    if let Err(error) = oidc.audit.rotate_session(
        result(
            operation.record.clone(),
            operation.receipt.clone(),
            Outcome::Succeeded,
            Reason::Verified,
            Some(principal.clone()),
        ),
        binding.clone(),
        operation.binding.generation,
        operation.binding.access_fingerprint,
    ) {
        registry.sessions.remove(&binding.session_id.to_string());
        return Err(audit_failure(error));
    }
    session.binding = binding.clone();
    session.access = Arc::new(access);
    session.rotating = false;
    session.pending_refresh = None;
    session.used = Instant::now();
    if logout.is_some() {
        session.logout_context = logout;
    }
    session.proof = OidcHumanProof {
        principal,
        authentication_id: binding.session_id.to_string(),
        generation: binding.generation,
        digest: raw_digest(raw),
        expires_at: binding.expires_at_unix_seconds,
        deadline: (Instant::now()
            + Duration::from_secs(
                binding
                    .expires_at_unix_seconds
                    .saturating_sub(chrono::Utc::now().timestamp())
                    .max(0) as u64,
            ))
        .min(session.created + Duration::from_secs(3600)),
        ceiling,
        mapping_revision: oidc.config.mapping.revision.clone(),
    };
    Ok(binding)
}
pub(super) async fn refresh(
    State(state): State<GatewayState>,
    request: Request,
) -> ApiResult<Json<TokenDelivery>> {
    let oidc = configured(&state)?;
    web(&oidc, request.headers())?;
    let raw = human_bearer(request.headers())
        .ok_or_else(credential_invalid)?
        .to_owned();
    let body: Refresh = json_body(request).await?;
    let (operation, refresh_token) = state
        .blocking_identity(move |state| {
            let oidc = configured(&state)?;
            human::reject_control_collision(&state, &raw, None)?;
            if state.credential_index.lookup(&raw).is_some() {
                return Err(credential_invalid());
            }
            if body.version != 1
                || !valid_private_token(
                    &body.refresh_token,
                    oidc.config.provider.limits.max_token_bytes,
                )
            {
                return Err(invalid_request());
            }
            let operation = begin_refresh(
                &oidc,
                &raw,
                body.authentication_id,
                body.generation,
                SessionKind::WebPkce,
                Some(&body.refresh_token),
            )?;
            Ok((operation.into_async(), body.refresh_token))
        })
        .await?;
    let response = oidc
        .provider
        .refresh(OidcClientKind::Web, &refresh_token, &operation.access)
        .await;
    complete_refresh(&state, operation, move |state, operation| {
        let oidc = configured(&state)?;
        match response {
            Ok(login) => {
                let (access, tokens, logout) = login.into_session_parts();
                let binding = install_rotation(
                    &state,
                    &oidc,
                    operation,
                    RotatedAccess {
                        access,
                        raw: tokens.access_token(),
                        refresh: tokens.refresh_token(),
                        logout,
                    },
                )?;
                Ok(Json(delivery(&binding, tokens)?))
            }
            Err(error) => {
                let (outcome, reason) = provider_result(error);
                fail_refresh(&oidc, operation, outcome, reason)?;
                Err(provider_error(error))
            }
        }
    })
    .await
}
fn valid_private_token(raw: &str, max: usize) -> bool {
    !raw.is_empty()
        && raw.len() <= max
        && !raw.chars().any(|ch| ch.is_control() || ch.is_whitespace())
}
pub(super) async fn rotate(
    State(state): State<GatewayState>,
    request: Request,
) -> ApiResult<Json<Value>> {
    let oidc = configured(&state)?;
    native(request.headers())?;
    let raw = human_bearer(request.headers())
        .ok_or_else(credential_invalid)?
        .to_owned();
    let body: Rotation = json_body(request).await?;
    if body.version != 1
        || !valid_private_token(
            &body.access_token,
            oidc.config.provider.limits.max_token_bytes,
        )
    {
        return Err(invalid_request());
    }
    let next_raw = body.access_token.clone();
    let preparation = Arc::clone(&oidc);
    let operation = state
        .blocking_identity(move |_state| {
            let oidc = preparation;
            let cleanup = store_io::CleanupReservation::reserve(&oidc.cleanup_slots)?;
            let mut registry = oidc.registry.lock().map_err(|_| audit_unavailable())?;
            let intent = registry.native.get(&body.intent_id).ok_or_else(conflict)?;
            let previous = intent.previous.as_ref().ok_or_else(conflict)?;
            if intent.action != Action::Refresh
                || intent.created.elapsed() >= Duration::from_secs(15)
                || previous.session_id != body.authentication_id
                || previous.generation != body.generation
                || previous.access_fingerprint != Fingerprint::access_token(&raw)
            {
                return Err(credential_invalid());
            }
            let session = registry
                .sessions
                .get(&body.authentication_id.to_string())
                .ok_or_else(credential_invalid)?;
            if session.binding != *previous
                || !session.rotating
                || session.proof.digest != raw_digest(&raw)
            {
                return Err(credential_invalid());
            }
            let record = native_record(&oidc, intent);
            let binding = previous.clone();
            let access = session.access.clone();
            let maximum_expiry = session.maximum_expiry;
            let created = intent.created;
            let completion_deadline = Some(intent.created + Duration::from_secs(15));
            let receipt = intent.intention.clone().ok_or_else(audit_unavailable)?;
            registry
                .native
                .remove(&body.intent_id)
                .ok_or_else(conflict)?;
            Ok(RefreshOperation {
                record,
                receipt,
                binding,
                access,
                maximum_expiry,
                completion_deadline,
                created,
                owner: Arc::downgrade(&oidc),
                cancel_on_drop: true,
                cleanup: Some(cleanup),
                inline_cleanup: true,
            }
            .into_async())
        })
        .await?;
    let response = oidc.provider.verify_access(&next_raw).await;
    complete_refresh(&state, operation, move |state, operation| {
        let oidc = configured(&state)?;
        let access = match response {
            Ok(access) => access,
            Err(error) => {
                let (outcome, reason) = provider_result(error);
                fail_refresh(&oidc, operation, outcome, reason)?;
                return Err(provider_error(error));
            }
        };
        let binding = install_rotation(
            &state,
            &oidc,
            operation,
            RotatedAccess {
                access,
                raw: &next_raw,
                refresh: None,
                logout: None,
            },
        )?;
        Ok(Json(session_dto(&binding)))
    })
    .await
}
struct LogoutReady {
    session: Session,
    intention: AuthenticationRecordV1,
    receipt: AuthenticationReceiptV1,
}
enum LogoutPreparation {
    Complete(Json<Value>),
    Ready(Box<LogoutReady>),
}
pub(super) async fn logout(
    State(state): State<GatewayState>,
    request: Request,
) -> ApiResult<Json<Value>> {
    let oidc = configured(&state)?;
    let headers = request.headers().clone();
    let raw = human_bearer(&headers).map(str::to_owned);
    let body: Logout = json_body(request).await?;
    if body.version != 1 || body.generation == 0 {
        return Err(invalid_request());
    }
    let prepared = state.blocking_identity(move |state| {
    let oidc = configured(&state)?;
    let session = {
        let mut registry = oidc.registry.lock().map_err(|_| audit_unavailable())?;
        let session = registry.sessions.get(&body.authentication_id.to_string());
        let Some(session) = session.filter(|session| {
            session.binding.generation == body.generation
                && raw.as_ref().is_some_and(|raw| {
                    session.proof.digest == raw_digest(raw)
                        && session.binding.access_fingerprint == Fingerprint::access_token(raw)
                })
        }) else {
            return Ok(LogoutPreparation::Complete(Json(
                json!({"version":1,"local_session":"unknown","provider_logout":"not_attempted","audit":"unavailable"}),
            )));
        };
        if session.binding.kind == SessionKind::WebPkce {
            web(&oidc, &headers)?;
        } else {
            native(&headers)?;
        }
        let session = registry
            .sessions
            .remove(&body.authentication_id.to_string())
            .ok_or_else(conflict)?;
        registry
            .native
            .retain(|_, intent| intent.session_id != body.authentication_id);
        session
    };
    let intention = record(
        &oidc,
        Action::Logout,
        session.binding.session_id,
        &session.binding.client_id,
        Some(session.binding.principal.clone()),
        None,
        Uuid::new_v4(),
    );
    let receipt = match oidc.audit.append_typed(intention.clone()) {
        Ok(receipt) => receipt,
        Err(_) => {
            return Ok(LogoutPreparation::Complete(Json(
                json!({"version":1,"local_session":"closed","provider_logout":"not_attempted","audit":"unavailable"}),
            )));
        }
    };
    Ok(LogoutPreparation::Ready(Box::new(LogoutReady {session, intention, receipt})))
    }).await?;
    let mut prepared = match prepared {
        LogoutPreparation::Complete(response) => return Ok(response),
        LogoutPreparation::Ready(prepared) => prepared,
    };
    let session = &mut prepared.session;
    let provider = if session.binding.kind == SessionKind::NativeCli {
        "not_attempted"
    } else if let Some(context) = session.logout_context.take() {
        match oidc.provider.logout_idp(context).await {
            Ok(()) => "accepted",
            Err(OidcError::NotConfigured) => "unsupported",
            Err(OidcError::TransportUnavailable) => "unavailable",
            Err(_) => "failed",
        }
    } else {
        "unsupported"
    };
    state.blocking_identity(move |state| {
    let oidc = configured(&state)?;
    let LogoutReady { session, intention, receipt } = *prepared;
    let audited = oidc
        .audit
        .close_session(
            result(
                intention,
                receipt,
                Outcome::Succeeded,
                Reason::UserRequested,
                Some(session.binding.principal.clone()),
            ),
            session.binding.generation,
            session.binding.access_fingerprint,
        )
        .is_ok();
    Ok(Json(
        json!({"version":1,"local_session":"closed","provider_logout":provider,"audit":if audited {"recorded"}else{"unavailable"}}),
    ))
    }).await
}
