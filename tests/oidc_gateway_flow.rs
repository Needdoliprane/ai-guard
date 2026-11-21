//! Controlled issuer HTTP plus real private authentication/business SQLite.
//! These regressions do not qualify actual Keycloak, a browser or a runtime.
#[path = "support/oidc_browser.rs"]
mod browser;
#[path = "support/oidc_fixture.rs"]
mod controlled;
mod support;
use agentsandbox as test_product;
use agentsandbox::{
    api::{GatewayState, OidcClaimsMappingV1, OidcGateway, OidcGatewayConfigV1, gateway_router},
    audit::{
        Store,
        authentication::{
            ApprovedAuthenticationClientV1, AuthenticationAudit, AuthenticationAuditPolicyV1,
        },
    },
    identity::*,
    policy::Policy,
    redaction::Redactor,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use controlled::ControlledIssuer;
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tower::ServiceExt;
use uuid::Uuid;
const ORIGIN: &str = "http://127.0.0.1:18100";
struct Fixture {
    _files: tempfile::TempDir,
    issuer: ControlledIssuer,
    store: Arc<Store>,
    audit: Arc<AuthenticationAudit>,
    router: Router,
    config: OidcGatewayConfigV1,
    ca_bundle_pem: Option<String>,
}
impl Fixture {
    async fn new() -> Self {
        let files = support::private_tempdir();
        let issuer = ControlledIssuer::new().await;
        Self::with_issuer(files, issuer, None).await
    }
    async fn with_issuer(
        files: tempfile::TempDir,
        issuer: ControlledIssuer,
        ca_bundle_pem: Option<String>,
    ) -> Self {
        Self::with_issuer_and_audit_limits(files, issuer, ca_bundle_pem, 10000, 4 * 1024 * 1024)
            .await
    }
    async fn with_audit_limits(max_records: u64, max_serialized_bytes: u64) -> Self {
        let files = support::private_tempdir();
        let issuer = ControlledIssuer::new().await;
        Self::with_issuer_and_audit_limits(files, issuer, None, max_records, max_serialized_bytes)
            .await
    }
    async fn with_issuer_and_audit_limits(
        files: tempfile::TempDir,
        issuer: ControlledIssuer,
        ca_bundle_pem: Option<String>,
        max_records: u64,
        max_serialized_bytes: u64,
    ) -> Self {
        let path = files.path().join("tenant.sqlite");
        let store =
            Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10000).unwrap();
        store
            .migrate_control_v3(&path.with_extension("before-v3"))
            .unwrap();
        store
            .migrate_identity_v4(&path.with_extension("before-v4"))
            .unwrap();
        store
            .migrate_human_access_v5(&path.with_extension("before-v5"))
            .unwrap();
        let store = Arc::new(store);
        let audit = Arc::new(
            AuthenticationAudit::init_new(
                &files.path().join("authentication.sqlite"),
                Uuid::new_v4(),
                AuthenticationAuditPolicyV1 {
                    max_records,
                    max_serialized_bytes,
                    approved_clients: ["web-public", "cli-public"]
                        .map(|client_id| ApprovedAuthenticationClientV1 {
                            issuer: issuer.issuer.clone(),
                            client_id: client_id.into(),
                        })
                        .to_vec(),
                },
                Redactor::new(vec![]),
            )
            .unwrap(),
        );
        let config = OidcGatewayConfigV1 {
            version: 1,
            provider: issuer.config(),
            provider_label: "Controlled issuer".into(),
            web_origin: if issuer.issuer.starts_with("https:") {
                "https://127.0.0.1:18100"
            } else {
                ORIGIN
            }
            .into(),
            mapping: OidcClaimsMappingV1 {
                revision: "mapping-1".into(),
                default_capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                rules: vec![],
            },
        };
        let router = Self::router_with_ca(&store, &audit, &config, ca_bundle_pem.clone());
        Self {
            _files: files,
            issuer,
            store,
            audit,
            router,
            config,
            ca_bundle_pem,
        }
    }
    fn router(
        store: &Arc<Store>,
        audit: &Arc<AuthenticationAudit>,
        config: &OidcGatewayConfigV1,
    ) -> Router {
        Self::router_with_ca(store, audit, config, None)
    }
    fn router_with_ca(
        store: &Arc<Store>,
        audit: &Arc<AuthenticationAudit>,
        config: &OidcGatewayConfigV1,
        ca: Option<String>,
    ) -> Router {
        gateway_router(
            GatewayState::new(
                store.clone(),
                "r".repeat(48),
                "v".repeat(48),
                Policy::default(),
                None,
                Redactor::new(vec![]),
            )
            .with_oidc(OidcGateway::new(config.clone(), audit.clone(), ca).unwrap())
            .unwrap(),
        )
    }
    fn map_administration_role(&mut self, role: &str) {
        self.config.mapping.rules = vec![agentsandbox::api::OidcClaimRuleV1 {
            claim_path: vec!["roles".into()],
            equals: role.into(),
            capabilities: BTreeSet::from([HumanCapabilityV1::ReadTenantMetadata]),
        }];
        self.router = Self::router(&self.store, &self.audit, &self.config);
    }
    fn member(&self, subject: &str) {
        let principal = PrincipalRefV1::HumanOidc {
            issuer: self.issuer.issuer.clone(),
            subject: subject.into(),
        };
        for change in [
            IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Named OIDC reader".into(),
                    revision: 1,
                    active: true,
                },
            },
            IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: MembershipV1 {
                    version: 1,
                    membership_id: Uuid::new_v4().to_string(),
                    principal,
                    tenant_id: "tenant-a".into(),
                    organization_scope: "organization-a".into(),
                    revision: 1,
                    active: true,
                    capabilities: BTreeSet::from([
                        HumanCapabilityV1::ReadMetadata,
                        HumanCapabilityV1::ReadContent,
                        HumanCapabilityV1::Export,
                        HumanCapabilityV1::ReadTenantMetadata,
                    ]),
                },
            },
        ] {
            self.store
                .mutate_identity(&IdentityMutationV1 {
                    version: 1,
                    operation_id: Uuid::new_v4().to_string(),
                    actor: PrincipalRefV1::LegacyControl {
                        credential_id: "fixture-operator".into(),
                        role: LegacyControlRoleV1::Runner,
                    },
                    change,
                })
                .unwrap();
        }
    }
}
async fn json_response(router: Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 262144).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({"invalid_test_response":true})),
    )
}
fn post(
    path: &str,
    value: Value,
    bearer: Option<&str>,
    cookie: Option<&str>,
    origin: Option<&str>,
) -> Request<Body> {
    let mut request = Request::post(path).header("content-type", "application/json");
    if let Some(value) = bearer {
        request = request.header("authorization", format!("Bearer {value}"));
    }
    if let Some(value) = cookie {
        request = request.header("cookie", value);
    }
    if let Some(value) = origin {
        request = request
            .header("origin", value)
            .header("sec-fetch-site", "same-origin");
    }
    request.body(Body::from(value.to_string())).unwrap()
}

#[tokio::test]
async fn public_profile_is_allowlisted_and_status_never_probes_issuer() {
    let f = Fixture::new().await;
    let (status, profile) = json_response(
        f.router.clone(),
        Request::get("/auth/oidc/cli-profile")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(profile["provider"]["issuer"], f.issuer.issuer);
    assert_eq!(
        profile["provider"],
        serde_json::to_value(&f.config.provider).unwrap()
    );
    assert!(
        profile["provider"]
            .get("web_post_logout_redirect_uri")
            .is_none()
    );
    assert!(
        profile["provider"]
            .get("cli_post_logout_redirect_uri")
            .is_none()
    );
    assert_eq!(
        profile
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["version", "enabled", "provider"])
    );
    assert!(!profile.to_string().contains("mapping-1"));
    assert!(!profile.to_string().contains("sqlite"));
    let (status, value) = json_response(
        f.router,
        Request::get("/auth/oidc/status")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        value,
        json!({"version":1,"enabled":true,"issuer":f.issuer.issuer,"provider_label":"Controlled issuer"})
    );
    assert!(f.issuer.requests().is_empty());
}

#[tokio::test]
async fn public_profile_projects_only_configured_logout_returns_without_probing_the_issuer() {
    let f = Fixture::new().await;
    for (web, cli) in [(true, false), (false, true), (true, true)] {
        let mut config = f.config.clone();
        config.provider.end_session_endpoint = Some(format!("{}/logout", f.issuer.issuer));
        config.provider.web_post_logout_redirect_uri =
            web.then(|| "https://logout.invalid/web-complete".into());
        config.provider.cli_post_logout_redirect_uri =
            cli.then(|| "https://logout.invalid/cli-complete".into());
        let router = Fixture::router(&f.store, &f.audit, &config);
        let (status, profile) = json_response(
            router,
            Request::get("/auth/oidc/cli-profile")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            profile["provider"],
            serde_json::to_value(&config.provider).unwrap()
        );
        let decoded: agentsandbox::oidc::OidcConfigV1 =
            serde_json::from_value(profile["provider"].clone()).unwrap();
        assert_eq!(decoded.web_post_logout_redirect_uri.is_some(), web);
        assert_eq!(decoded.cli_post_logout_redirect_uri.is_some(), cli);
    }
    assert!(f.issuer.requests().is_empty());
}

#[tokio::test]
async fn explicit_cli_admission_limits_contexts_and_does_not_reenroll_on_restart() {
    let f = Fixture::new().await;
    f.member("member-a");
    let token = f
        .issuer
        .token("cli-public", "member-a", None, &[], "fresh-cli");
    let raw_identity = Request::get("/api/v1/human/identity")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        json_response(f.router.clone(), raw_identity).await.0,
        StatusCode::UNAUTHORIZED,
        "identity must never auto-admit a JWT"
    );
    let (intent_status, intent) = json_response(
        f.router.clone(),
        post(
            "/auth/oidc/native-intent",
            json!({"version":1,"action":{"kind":"login"}}),
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(intent_status, StatusCode::OK);
    assert!(
        f.issuer.requests().is_empty(),
        "native durable ACK precedes any provider request"
    );
    let (status, session) = json_response(
        f.router.clone(),
        post(
            "/auth/oidc/admit",
            json!({"version":1,"intent_id":intent["intent_id"]}),
            Some(&token),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "explicit valid CLI admission must succeed after private durable audit"
    );
    assert_eq!(session["generation"], 1);
    assert!(Uuid::parse_str(session["authentication_id"].as_str().unwrap()).is_ok());
    let (status, identity) = json_response(
        f.router.clone(),
        Request::get("/api/v1/human/identity")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(identity["authentication"]["method"], "oidc_access_token");
    assert_eq!(
        identity["authentication"]["credential_id"],
        session["authentication_id"]
    );
    assert_eq!(
        identity["contexts"].as_array().unwrap().len(),
        1,
        "external default cannot add administration"
    );
    assert_eq!(
        identity["contexts"][0]["capabilities"],
        json!(["read_metadata"])
    );
    let restarted = Fixture::router(&f.store, &f.audit, &f.config);
    assert_eq!(
        json_response(
            restarted.clone(),
            Request::get("/api/v1/human/identity")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap()
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (_, intent) = json_response(
        restarted.clone(),
        post(
            "/auth/oidc/native-intent",
            json!({"version":1,"action":{"kind":"login"}}),
            None,
            None,
            None,
        ),
    )
    .await;
    let (status, _) = json_response(
        restarted,
        post(
            "/auth/oidc/admit",
            json!({"version":1,"intent_id":intent["intent_id"]}),
            Some(&token),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "durable fingerprint blocks replay after empty memory registry restart"
    );
}

#[tokio::test]
#[ignore = "runs the native CLI against loopback HTTP and waits for natural token expiry"]
async fn native_cli_reads_then_observes_durable_membership_revocation_and_expiry() {
    use agentsandbox::{audit::authentication::AuthenticationSessionStateV1, model::Event};
    use axum::{
        Form, Json,
        http::{HeaderMap, header},
        routing,
    };
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use sha2::{Digest, Sha256};
    use std::{collections::BTreeMap, process::Stdio, sync::Mutex};
    use tokio::{
        io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
        process::ChildStdout,
    };

    struct Grant {
        code: String,
        redirect: String,
        challenge: String,
        response: Value,
    }

    async fn line(out: &mut BufReader<ChildStdout>, timeout: Duration, secrets: &[&str]) -> Value {
        let mut text = String::new();
        let mut bounded = out.take(262_145);
        let bytes = tokio::time::timeout(timeout, bounded.read_line(&mut text))
            .await
            .expect("native CLI reply deadline")
            .expect("read native CLI reply");
        assert!((1..=262_144).contains(&bytes));
        assert!(
            secrets.iter().all(|value| !text.contains(*value)),
            "native CLI output contains a private value"
        );
        serde_json::from_str(&text).expect("typed native CLI reply")
    }

    const SUBJECT: &str = "native-cli-operator-revocation";
    const LIFETIME: i64 = 12;
    let grant = Arc::new(Mutex::new(None::<Grant>));
    let issuer = ControlledIssuer::with_transport(None, |_| {
        let pending = Arc::clone(&grant);
        Router::new().route(
            "/realm/token",
            routing::post(
                move |headers: HeaderMap, Form(form): Form<BTreeMap<String, String>>| {
                    let pending = Arc::clone(&pending);
                    async move {
                        let mut pending = pending.lock().unwrap();
                        let get = |key: &str| form.get(key).map(String::as_str);
                        let accepted = pending.as_ref().is_some_and(|grant| {
                            get("grant_type") == Some("authorization_code")
                                && get("client_id") == Some("cli-public")
                                && get("code") == Some(grant.code.as_str())
                                && get("redirect_uri") == Some(grant.redirect.as_str())
                                && get("code_verifier").is_some_and(|verifier| {
                                    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
                                        == grant.challenge
                                })
                                && !headers.contains_key(header::AUTHORIZATION)
                                && !form.contains_key("client_secret")
                        });
                        if accepted {
                            (StatusCode::OK, Json(pending.take().unwrap().response))
                        } else {
                            (
                                StatusCode::BAD_REQUEST,
                                Json(json!({"error":"invalid_grant"})),
                            )
                        }
                    }
                },
            ),
        )
    })
    .await;
    let f = Fixture::with_issuer(support::private_tempdir(), issuer, None).await;
    f.member(SUBJECT);
    let mut event = Event::new("fixture.native_cli_resource");
    event.session_id = Some("cli-authorized-session".into());
    f.store.append(event).unwrap();
    let principal = PrincipalRefV1::HumanOidc {
        issuer: f.issuer.issuer.clone(),
        subject: SUBJECT.into(),
    };
    let actor = PrincipalRefV1::LegacyControl {
        credential_id: "fixture-operator".into(),
        role: LegacyControlRoleV1::Runner,
    };
    f.store
        .mutate_identity(&IdentityMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            actor: actor.clone(),
            change: IdentityChangeV1::ResourceGrant {
                expected_revision: 0,
                grant: ResourceGrantV1 {
                    version: 1,
                    tenant_id: "tenant-a".into(),
                    organization_scope: "organization-a".into(),
                    resource: ResourceRefV1 {
                        kind: ResourceKindV1::Session,
                        resource_id: "cli-authorized-session".into(),
                    },
                    owner: principal.clone(),
                    revision: 1,
                    active: true,
                    shares: vec![],
                },
            },
        })
        .unwrap();
    let gateway = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", gateway.local_addr().unwrap());
    let router = f.router.clone();
    let task = tokio::spawn(async move { axum::serve(gateway, router).await });
    let cli_dir = support::private_tempdir();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args(["human", "sso", "--gateway", &origin, "--browser", "manual"])
        .current_dir(cli_dir.path())
        .env_clear()
        .env("HOME", cli_dir.path())
        .env("TMPDIR", cli_dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("start native CLI");
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut input = child.stdin.take().unwrap();
    let step = Duration::from_secs(5);
    let authorize = line(&mut out, step, &[]).await;
    assert_eq!(authorize["status"], "browser_authorization_required");
    let url = reqwest::Url::parse(authorize["authorization_url"].as_str().unwrap()).unwrap();
    assert!(
        url.as_str()
            .starts_with(&format!("{}/authorize?", f.issuer.issuer))
    );
    let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert!(query["client_id"] == "cli-public" && query["code_challenge_method"] == "S256");
    assert!(query["response_type"] == "code" && query["code_challenge"].len() == 43);
    assert!(!query.contains_key("client_secret") && !query.contains_key("code_verifier"));
    let mut callback = reqwest::Url::parse(&query["redirect_uri"]).unwrap();
    assert!(
        callback.scheme() == "http"
            && callback.host_str() == Some("127.0.0.1")
            && callback.port().is_some()
            && callback.path() == "/oidc/callback"
            && callback.username().is_empty()
            && callback.password().is_none()
            && callback.query().is_none()
            && callback.fragment().is_none()
    );
    let code = Uuid::new_v4().to_string();
    let refresh = Uuid::new_v4().to_string();
    let access = controlled::signed_token(
        &f.issuer.issuer,
        "cli-public",
        SUBJECT,
        None,
        &[],
        "native-live-access",
        LIFETIME,
    );
    let id_token = controlled::signed_token(
        &f.issuer.issuer,
        "cli-public",
        SUBJECT,
        Some(&query["nonce"]),
        &[],
        "native-live-id",
        LIFETIME,
    );
    let secrets = [
        code.as_str(),
        refresh.as_str(),
        access.as_str(),
        id_token.as_str(),
    ];
    *grant.lock().unwrap() = Some(Grant {
        code: code.clone(),
        redirect: callback.to_string(),
        challenge: query["code_challenge"].clone(),
        response: json!({"token_type":"Bearer", "access_token":access, "id_token":id_token, "refresh_token":refresh, "expires_in":LIFETIME}),
    });
    callback
        .query_pairs_mut()
        .append_pair("state", &query["state"])
        .append_pair("code", &code);
    let response = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(step)
        .build()
        .unwrap()
        .get(callback)
        .send()
        .await
        .map_err(|error| error.without_url())
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let ready = line(&mut out, step, &secrets).await;
    assert_eq!(ready["status"], "session_ready");
    assert_eq!(ready["generation"], 1);
    assert_eq!(ready["storage"], "foreground_memory_only");
    assert!(
        grant.lock().unwrap().is_none(),
        "provider must accept the actual code exchange"
    );
    let session = Uuid::parse_str(ready["authentication_id"].as_str().unwrap()).unwrap();
    assert_eq!(
        f.audit.session_current(session).unwrap().unwrap().state,
        AuthenticationSessionStateV1::Active
    );

    input.write_all(b"identity\n").await.unwrap();
    let identity = line(&mut out, step, &secrets).await;
    assert_eq!(
        identity["principal"],
        serde_json::to_value(&principal).unwrap()
    );
    assert_eq!(identity["contexts_complete"], true);
    let contexts = identity["contexts"].as_array().unwrap();
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0]["tenant_id"], "tenant-a");
    let context = contexts[0]["id"].as_str().unwrap();
    input
        .write_all(format!("context {context}\n").as_bytes())
        .await
        .unwrap();
    assert_eq!(
        line(&mut out, step, &secrets).await["status"],
        "context_selected"
    );
    input.write_all(b"resources --limit 1\n").await.unwrap();
    let page = line(&mut out, step, &secrets).await;
    assert_eq!(
        page["items"][0]["resource"]["resource_id"],
        "cli-authorized-session"
    );
    assert!(page["read_receipt"]["audit_event_id"].is_string());

    let mut membership = f.store.identity_memberships(&principal).unwrap().remove(0);
    let expected_revision = membership.revision;
    membership.revision += 1;
    membership.active = false;
    f.store
        .mutate_identity(&IdentityMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            actor,
            change: IdentityChangeV1::Membership {
                expected_revision,
                membership,
            },
        })
        .unwrap();
    let head = f.store.head().unwrap();
    input.write_all(b"resources --limit 1\n").await.unwrap();
    let denied = line(&mut out, step, &secrets).await;
    assert_eq!(denied["status"], "command_refused");
    assert!(
        denied["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("context_revoked"))
    );
    assert!(denied.get("items").is_none());
    assert_eq!(f.store.head().unwrap(), head);
    input.write_all(b"identity\n").await.unwrap();
    let identity = line(&mut out, step, &secrets).await;
    assert_eq!(identity["contexts_complete"], true);
    assert_eq!(identity["contexts"], json!([]));

    // Keep stdin open so the observed close can only be the native idle expiry.
    let closed = line(&mut out, Duration::from_secs(20), &secrets).await;
    assert_eq!(closed["status"], "session_closed");
    assert_eq!(closed["local_session"], "closed");
    assert_eq!(closed["audit"], "recorded");
    let exit = tokio::time::timeout(step, child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    task.abort();
    let error = std::str::from_utf8(&exit.stderr).unwrap();
    assert!(!exit.status.success() && error.contains("OIDC foreground session expired"));
    assert!(secrets.iter().all(|value| !error.contains(*value)));
    assert!(matches!(
        f.audit.session_current(session).unwrap().unwrap().state,
        AuthenticationSessionStateV1::Closed { .. }
    ));
    assert!(f.audit.verify().is_ok() && f.store.integrity().unwrap().valid);
    assert!(std::fs::read_dir(cli_dir.path()).unwrap().next().is_none());
    println!(
        "native_cli_pkce=true authorized_read=true membership_revoked=true expiry_closed=true no_credential_file=true"
    );
}

#[tokio::test]
async fn browser_pkce_callback_is_correlated_consumed_and_delivered_once() {
    let f = Fixture::new().await;
    f.member("web-member");
    let hostile = post(
        "/auth/oidc/start",
        json!({"version":1}),
        None,
        None,
        Some("https://hostile.example.test"),
    );
    assert_eq!(
        json_response(f.router.clone(), hostile).await.0,
        StatusCode::FORBIDDEN
    );
    assert!(f.issuer.requests().is_empty());
    let seeded = f
        .router
        .clone()
        .oneshot(
            Request::get("/auth/oidc/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(seeded.status(), StatusCode::OK);
    assert!(
        seeded.headers().get("set-cookie").is_none(),
        "status must not pre-seed aiguard_oidc"
    );
    let response = f
        .router
        .clone()
        .oneshot(post(
            "/auth/oidc/start",
            json!({"version":1}),
            None,
            None,
            Some(ORIGIN),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert!(
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Path=/auth/oidc; HttpOnly; SameSite=Lax")
    );
    let start: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    let correlation_token = start["correlation_token"].as_str().unwrap().to_owned();
    assert_eq!(correlation_token.len(), 64);
    assert!(
        correlation_token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    let url = reqwest::Url::parse(start["authorization_url"].as_str().unwrap()).unwrap();
    let params = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(params["code_challenge_method"], "S256");
    let token = f
        .issuer
        .login_response(&params["nonce"], "web-member", "web-login");
    let path = format!(
        "/auth/oidc/callback?code=controlled-code&state={}",
        params["state"]
    );
    let callback = || {
        Request::get(&path)
            .header("cookie", &cookie)
            .header("sec-fetch-site", "cross-site")
            .body(Body::empty())
            .unwrap()
    };
    let response = f.router.clone().oneshot(callback()).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()["location"], "/auth/oidc/complete");
    assert_eq!(
        f.issuer
            .requests()
            .iter()
            .filter(|(p, _)| p.ends_with("/token"))
            .count(),
        1
    );
    assert_ne!(
        f.router.clone().oneshot(callback()).await.unwrap().status(),
        StatusCode::SEE_OTHER,
        "replay cannot replace pending handoff"
    );
    let (status, delivery) = json_response(
        f.router.clone(),
        post(
            "/auth/oidc/tokens",
            json!({"version":1,"correlation_token":correlation_token}),
            None,
            Some(&cookie),
            Some(ORIGIN),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(delivery["access_token"], token);
    assert!(delivery.get("id_token").is_none());
    assert_eq!(
        json_response(
            f.router.clone(),
            post(
                "/auth/oidc/tokens",
                json!({"version":1,"correlation_token":correlation_token}),
                None,
                Some(&cookie),
                Some(ORIGIN)
            )
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (status, logout) = json_response(
        f.router.clone(),
        post(
            "/auth/oidc/logout",
            json!({"version":1,"authentication_id":delivery["authentication_id"],"generation":1}),
            Some(&token),
            None,
            Some(ORIGIN),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(logout["local_session"], "closed");
    assert_eq!(
        json_response(
            f.router,
            Request::get("/api/v1/human/identity")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap()
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn no_membership_and_web_bearer_are_private_audited_cli_refusals() {
    let f = Fixture::new().await;
    let before = f.audit.verify().unwrap().records;
    let token = f.issuer.token(
        "cli-public",
        "unapproved-subject",
        None,
        &[],
        "unknown-member",
    );
    let (_, intent) = json_response(
        f.router.clone(),
        post(
            "/auth/oidc/native-intent",
            json!({"version":1,"action":{"kind":"login"}}),
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(
        json_response(
            f.router.clone(),
            post(
                "/auth/oidc/admit",
                json!({"version":1,"intent_id":intent["intent_id"]}),
                Some(&token),
                None,
                None
            )
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert!(f.audit.verify().unwrap().records >= before + 2);
    let web = f
        .issuer
        .token("web-public", "unapproved-subject", None, &[], "not-cli");
    let (_, intent) = json_response(
        f.router.clone(),
        post(
            "/auth/oidc/native-intent",
            json!({"version":1,"action":{"kind":"login"}}),
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(
        json_response(
            f.router,
            post(
                "/auth/oidc/admit",
                json!({"version":1,"intent_id":intent["intent_id"]}),
                Some(&web),
                None,
                None
            )
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn oidc_subjects_read_only_their_durable_tenant_even_with_a_forged_tenant_claim() {
    use agentsandbox::audit::authentication::{
        AuthenticationCapacityPlanV1, AuthenticationCapacityV1, AuthenticationTenantCapacityV1,
    };
    use agentsandbox::{api::TenantGateway, model::Event};
    let f = Fixture::new().await;
    f.member("subject-a");
    let path = f._files.path().join("tenant-b.sqlite");
    let b = Store::open_for_tenant(&path, "tenant-b", Redactor::new(vec![]), 10000).unwrap();
    b.migrate_control_v3(&path.with_extension("before-v3"))
        .unwrap();
    b.migrate_identity_v4(&path.with_extension("before-v4"))
        .unwrap();
    b.migrate_human_access_v5(&path.with_extension("before-v5"))
        .unwrap();
    let b = Arc::new(b);
    let principal_b = PrincipalRefV1::HumanOidc {
        issuer: f.issuer.issuer.clone(),
        subject: "subject-b".into(),
    };
    let mutation = |change| IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "fixture-operator".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    };
    b.mutate_identity(&mutation(IdentityChangeV1::Account {
        expected_revision: 0,
        account: HumanAccountV1 {
            version: 1,
            principal: principal_b.clone(),
            display_name: "Named B".into(),
            revision: 1,
            active: true,
        },
    }))
    .unwrap();
    b.mutate_identity(&mutation(IdentityChangeV1::Membership {
        expected_revision: 0,
        membership: MembershipV1 {
            version: 1,
            membership_id: Uuid::new_v4().to_string(),
            principal: principal_b,
            tenant_id: "tenant-b".into(),
            organization_scope: "organization-b".into(),
            revision: 1,
            active: true,
            capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
        },
    }))
    .unwrap();
    for (store, subject, organization, resource) in [
        (&f.store, "subject-a", "organization-a", "session-a-private"),
        (&b, "subject-b", "organization-b", "session-b-private"),
    ] {
        let mut event = Event::new("fixture.oidc-isolation");
        event.session_id = Some(resource.into());
        event.data = json!({"label":resource});
        store.append(event).unwrap();
        store
            .mutate_identity(&mutation(IdentityChangeV1::ResourceGrant {
                expected_revision: 0,
                grant: ResourceGrantV1 {
                    version: 1,
                    tenant_id: store.tenant_id().into(),
                    organization_scope: organization.into(),
                    resource: ResourceRefV1 {
                        kind: ResourceKindV1::Session,
                        resource_id: resource.into(),
                    },
                    owner: PrincipalRefV1::HumanOidc {
                        issuer: f.issuer.issuer.clone(),
                        subject: subject.into(),
                    },
                    revision: 1,
                    active: true,
                    shares: vec![],
                },
            }))
            .unwrap();
    }
    // This two-tenant assembly uses a separate, genuinely new V4 audit. The
    // later mono-tenant legacy-collision control retains its original V3 audit.
    let multi_audit = Arc::new(
        AuthenticationAudit::init_new_v4(
            &f._files.path().join("authentication-multi.sqlite"),
            Uuid::new_v4(),
            AuthenticationAuditPolicyV1 {
                max_records: 10_000,
                max_serialized_bytes: 4 * 1024 * 1024,
                approved_clients: ["web-public", "cli-public"]
                    .map(|client_id| ApprovedAuthenticationClientV1 {
                        issuer: f.issuer.issuer.clone(),
                        client_id: client_id.into(),
                    })
                    .to_vec(),
            },
            AuthenticationCapacityPlanV1 {
                version: 1,
                unscoped: AuthenticationCapacityV1 {
                    max_records: 64,
                    max_serialized_bytes: 256 * 1024,
                },
                tenants: ["tenant-a", "tenant-b"]
                    .map(|tenant_id| AuthenticationTenantCapacityV1 {
                        tenant_id: tenant_id.into(),
                        capacity: AuthenticationCapacityV1 {
                            max_records: 4967,
                            max_serialized_bytes: 1536 * 1024,
                        },
                    })
                    .to_vec(),
            },
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    assert_eq!(multi_audit.format_version().unwrap(), 4);
    assert_eq!(multi_audit.verify().unwrap().records, 2);
    let router = gateway_router(
        GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: f.store.clone(),
                    runner_token: "r".repeat(48),
                    viewer_token: "v".repeat(48),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: b.clone(),
                    runner_token: "s".repeat(48),
                    viewer_token: "w".repeat(48),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap()
        .with_oidc(OidcGateway::new(f.config.clone(), multi_audit, None).unwrap())
        .unwrap(),
    );
    let mut proofs = Vec::new();
    for (subject, forged, expected, resource) in [
        ("subject-a", "tenant-b", "tenant-a", "session-a-private"),
        ("subject-b", "tenant-a", "tenant-b", "session-b-private"),
    ] {
        let token = f.issuer.token_with_tenant(subject, forged);
        let (status, intent) = json_response(
            router.clone(),
            post(
                "/auth/oidc/native-intent",
                json!({"version":1,"action":{"kind":"login"}}),
                None,
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json_response(
                router.clone(),
                post(
                    "/auth/oidc/admit",
                    json!({"version":1,"intent_id":intent["intent_id"]}),
                    Some(&token),
                    None,
                    None
                )
            )
            .await
            .0,
            StatusCode::CREATED
        );
        let (status, identity) = json_response(
            router.clone(),
            Request::get("/api/v1/human/identity")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(identity["principal"]["subject"], subject);
        let contexts = identity["contexts"].as_array().unwrap();
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0]["tenant_id"], expected);
        let context = contexts[0]["id"].as_str().unwrap().to_owned();
        let (status, page) = json_response(
            router.clone(),
            Request::get("/api/v1/human/resources")
                .header("authorization", format!("Bearer {token}"))
                .header("x-aiguard-context", &context)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert_eq!(page["items"][0]["resource"]["resource_id"], resource);
        proofs.push((token, context, resource));
    }
    for (token, context, foreign_resource, foreign_context) in [
        (&proofs[0].0, &proofs[0].1, proofs[1].2, &proofs[1].1),
        (&proofs[1].0, &proofs[1].1, proofs[0].2, &proofs[0].1),
    ] {
        let (status,page)=json_response(router.clone(),Request::get(format!("/api/v1/human/events?view=metadata&resource_kind=session&resource_id={foreign_resource}")).header("authorization",format!("Bearer {token}")).header("x-aiguard-context",context).body(Body::empty()).unwrap()).await;
        assert!(status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND);
        assert!(page.get("items").is_none());
        assert_eq!(
            json_response(
                router.clone(),
                Request::get("/api/v1/human/resources")
                    .header("authorization", format!("Bearer {token}"))
                    .header("x-aiguard-context", foreign_context)
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    let collided = f
        .issuer
        .token("cli-public", "subject-a", None, &[], "control-collision");
    let collision_router = gateway_router(
        GatewayState::new(
            f.store.clone(),
            "r".repeat(48),
            collided.clone(),
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .with_oidc(OidcGateway::new(f.config.clone(), f.audit.clone(), None).unwrap())
        .unwrap(),
    );
    assert_eq!(
        json_response(
            collision_router.clone(),
            Request::get("/api/v1/identity")
                .header("authorization", format!("Bearer {collided}"))
                .body(Body::empty())
                .unwrap()
        )
        .await
        .0,
        StatusCode::OK,
        "positive legacy viewer control"
    );
    let (_, intent) = json_response(
        collision_router.clone(),
        post(
            "/auth/oidc/native-intent",
            json!({"version":1,"action":{"kind":"login"}}),
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(
        json_response(
            collision_router.clone(),
            post(
                "/auth/oidc/admit",
                json!({"version":1,"intent_id":intent["intent_id"]}),
                Some(&collided),
                None,
                None
            )
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED,
        "a valid signed token already configured as control cannot acquire OIDC authority"
    );
    assert_eq!(
        json_response(
            collision_router,
            Request::get("/api/v1/human/identity")
                .header("authorization", format!("Bearer {collided}"))
                .body(Body::empty())
                .unwrap()
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_oidc_neighbor_fault_isolation(&f, &router, &proofs).await;
}

async fn assert_oidc_neighbor_fault_isolation(
    fixture: &Fixture,
    router: &Router,
    proofs: &[(String, String, &str)],
) {
    // The controlled issuer and durable admissions above remain unchanged.
    // Only tenant A's business audit is corrupted, using a separate SQLite
    // connection just as the technical-credential regression does.
    let db = rusqlite::Connection::open(fixture._files.path().join("tenant.sqlite")).unwrap();
    assert!(db.execute("UPDATE events SET hash='tampered'", []).unwrap() > 0);
    drop(db);

    let (status, identity) = json_response(
        router.clone(),
        Request::get("/api/v1/human/identity")
            .header("authorization", format!("Bearer {}", proofs[1].0))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(identity["principal"]["subject"], "subject-b");
    assert_eq!(identity["contexts_complete"], false);
    assert_eq!(identity["contexts"].as_array().unwrap().len(), 1);
    assert_eq!(identity["contexts"][0]["tenant_id"], "tenant-b");
    assert_eq!(identity["contexts"][0]["id"], proofs[1].1);

    // A's previously issued context is not reclassified as revoked or empty.
    // Both its identity and reads still require its now-unavailable store.
    for (path, context) in [
        ("/api/v1/human/identity", None),
        ("/api/v1/human/resources", Some(proofs[0].1.as_str())),
    ] {
        let mut request =
            Request::get(path).header("authorization", format!("Bearer {}", proofs[0].0));
        if let Some(context) = context {
            request = request.header("x-aiguard-context", context);
        }
        let (status, error) =
            json_response(router.clone(), request.body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error["error"]["code"], "audit_unavailable");
        assert!(error.get("principal").is_none());
        assert!(error.get("contexts").is_none());
        assert!(error.get("items").is_none());
    }

    for (subject, salt, expected) in [
        (
            "subject-b",
            "healthy-neighbor-admission",
            StatusCode::CREATED,
        ),
        (
            "unknown-during-neighbor-fault",
            "unknown-neighbor-admission",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let (status, intent) = json_response(
            router.clone(),
            post(
                "/auth/oidc/native-intent",
                json!({"version":1,"action":{"kind":"login"}}),
                None,
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let token = fixture.issuer.token("cli-public", subject, None, &[], salt);
        let (status, response) = json_response(
            router.clone(),
            post(
                "/auth/oidc/admit",
                json!({"version":1,"intent_id":intent["intent_id"]}),
                Some(&token),
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, expected);
        if expected == StatusCode::SERVICE_UNAVAILABLE {
            assert_eq!(response["error"]["code"], "audit_unavailable");
            assert!(response.get("authentication_id").is_none());
        }
    }
}

#[tokio::test]
async fn native_rotation_crossing_ticket_deadline_during_jwks_verification_closes_without_new_generation()
 {
    use agentsandbox::{audit::authentication::AuthenticationSessionStateV1, oidc::OidcProvider};
    let mut f = Fixture::new().await;
    f.member("deadline-member");
    // Expire the approved cache before rotation, then hold its real JWKS HTTP
    // response. The 2 s hold stays below the configured 5 s HTTP deadline.
    f.config.provider.limits.jwks_ttl_seconds = 1;
    f.config.provider.limits.metadata_ttl_seconds = 1;
    assert_eq!(f.config.provider.limits.request_timeout_ms, 5000);
    f.router = Fixture::router(&f.store, &f.audit, &f.config);
    let old = f
        .issuer
        .token("cli-public", "deadline-member", None, &[], "deadline-old");
    let (_, intent) = json_response(
        f.router.clone(),
        post(
            "/auth/oidc/native-intent",
            json!({"version":1,"action":{"kind":"login"}}),
            None,
            None,
            None,
        ),
    )
    .await;
    let (status, admitted) = json_response(
        f.router.clone(),
        post(
            "/auth/oidc/admit",
            json!({"version":1,"intent_id":intent["intent_id"]}),
            Some(&old),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "positive initial admission");
    let id = Uuid::parse_str(admitted["authentication_id"].as_str().unwrap()).unwrap();
    let before = f.audit.verify().unwrap().records;
    let sent = tokio::time::Instant::now();
    let (status, ticket) = json_response(
        f.router.clone(),
        post(
            "/auth/oidc/native-intent",
            json!({"version":1,"action":{"kind":"refresh","authentication_id":id,"generation":1}}),
            Some(&old),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let acknowledged = tokio::time::Instant::now();
    let next = f
        .issuer
        .token("cli-public", "deadline-member", None, &[], "deadline-next");
    let gate = f.issuer.block_keys();
    tokio::time::sleep_until(acknowledged + Duration::from_secs(14)).await;
    let rotating=tokio::spawn(f.router.clone().oneshot(post("/auth/oidc/rotate",json!({"version":1,"intent_id":ticket["intent_id"],"authentication_id":id,"generation":1,"access_token":next}),Some(&old),None,None)));
    tokio::time::timeout(Duration::from_secs(2), gate.entered())
        .await
        .expect("setup must reach JWKS while the ticket is valid");
    let entered = tokio::time::Instant::now();
    let entered_before_deadline = entered < sent + Duration::from_secs(15);
    tokio::time::sleep_until(acknowledged + Duration::from_secs(16)).await;
    f.issuer.clear_keys_barrier();
    gate.release();
    let response = rotating.await.unwrap().unwrap();
    let status = response.status();
    let verification_elapsed = entered.elapsed();
    let current = f.audit.session_current(id).unwrap().unwrap();
    let (identity_status, _) = json_response(
        f.router.clone(),
        Request::get("/api/v1/human/identity")
            .header("authorization", format!("Bearer {next}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    // Independent positive verification makes a JWT/signature/profile or
    // token-expiry refusal an invalid explanation for the expected denial.
    let verified = OidcProvider::new(f.config.provider.clone())
        .unwrap()
        .verify_access(&next)
        .await
        .unwrap();
    assert_eq!(verified.subject(), "deadline-member");
    assert!(verified.expires_at() > chrono::Utc::now().timestamp() + 60);
    assert!(
        entered_before_deadline,
        "setup reached JWKS after the earliest possible ticket deadline"
    );
    assert!(
        verification_elapsed < Duration::from_secs(5),
        "the controlled response must finish within the provider request limit"
    );
    assert!(
        status.is_client_error(),
        "a valid rotation result arriving after the native15s ticket must be refused"
    );
    assert_eq!(
        identity_status,
        StatusCode::UNAUTHORIZED,
        "the delayed new JWT must never acquire usable authority"
    );
    assert_eq!(current.binding.generation, 1);
    assert!(
        matches!(current.state, AuthenticationSessionStateV1::Closed { .. }),
        "expired native operation must have a terminal durable state"
    );
    assert!(
        f.audit.verify().unwrap().records >= before + 2,
        "pending and terminal phases must both be durable"
    );
}

async fn web_delivery(f: &Fixture, subject: &str, salt: &str) -> (Value, String) {
    web_delivery_roles(f, subject, salt, &[]).await
}

#[tokio::test]
async fn successful_web_refresh_withdraws_administration_and_preserves_personal_scope() {
    use agentsandbox::audit::authentication::AuthenticationSessionStateV1;

    let mut f = Fixture::new().await;
    let subject = "web-refresh-member";
    f.member(subject);
    f.map_administration_role("tenant-admin");

    let get = |path: &str, token: &str, context: Option<&str>| {
        let mut request = Request::get(path).header("authorization", format!("Bearer {token}"));
        if let Some(context) = context {
            request = request.header("x-aiguard-context", context);
        }
        request.body(Body::empty()).unwrap()
    };
    let identity_path = "/api/v1/human/identity";
    let resources_path = "/api/v1/human/resources";
    let (initial, nonce) =
        web_delivery_roles(&f, subject, "refresh-admin", &["tenant-admin"]).await;
    let id = Uuid::parse_str(initial["authentication_id"].as_str().unwrap()).unwrap();
    let previous = initial["access_token"].as_str().unwrap();
    let (status, identity) =
        json_response(f.router.clone(), get(identity_path, previous, None)).await;
    assert_eq!(status, StatusCode::OK);
    let contexts = identity["contexts"].as_array().unwrap();
    assert_eq!(contexts.len(), 2);
    let administration = contexts
        .iter()
        .find(|context| context["mode"] == "administration")
        .unwrap();
    assert_eq!(administration["tenant_id"], "tenant-a");
    assert_eq!(
        administration["capabilities"],
        json!(["read_metadata", "read_tenant_metadata"])
    );
    let admin_context = administration["id"].as_str().unwrap();
    assert_eq!(
        json_response(
            f.router.clone(),
            get(resources_path, previous, Some(admin_context))
        )
        .await
        .0,
        StatusCode::OK
    );
    let before_refresh = f.audit.verify().unwrap().records;
    let expected_next = f.issuer.login_response(&nonce, subject, "refresh-personal");
    let (status, rotated) = json_response(
        f.router.clone(),
        post(
            "/auth/oidc/refresh",
            json!({"version":1,"authentication_id":id,"generation":1,
                "refresh_token":initial["refresh_token"]}),
            Some(previous),
            None,
            Some(ORIGIN),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rotated["authentication_id"], initial["authentication_id"]);
    assert_eq!(rotated["generation"], 2);
    let next = rotated["access_token"].as_str().unwrap();
    assert!(next == expected_next);
    let persisted = f.audit.session_current(id).unwrap().unwrap();
    assert_eq!(persisted.state, AuthenticationSessionStateV1::Active);
    assert_eq!(persisted.binding.generation, 2);
    assert_eq!(f.audit.verify().unwrap().records, before_refresh + 2);
    assert_eq!(
        f.issuer
            .requests()
            .iter()
            .filter(|(path, body)| path.ends_with("/token")
                && body.contains("grant_type=refresh_token"))
            .count(),
        1
    );

    let (status, after) = json_response(f.router.clone(), get(identity_path, next, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(after["principal"], identity["principal"]);
    assert_eq!(after["contexts_complete"], true);
    let contexts = after["contexts"].as_array().unwrap();
    assert_eq!(contexts.len(), 1);
    let personal = &contexts[0];
    assert_eq!(personal["mode"], "personal");
    assert_eq!(personal["tenant_id"], "tenant-a");
    assert_eq!(personal["capabilities"], json!(["read_metadata"]));
    let personal_context = personal["id"].as_str().unwrap();
    let before_refusals = f.store.head().unwrap();
    for (token, expected) in [
        (previous, StatusCode::UNAUTHORIZED),
        (next, StatusCode::FORBIDDEN),
    ] {
        let (status, response) = json_response(
            f.router.clone(),
            get(resources_path, token, Some(admin_context)),
        )
        .await;
        assert_eq!(status, expected);
        assert!(response.get("items").is_none());
    }
    assert_eq!(f.store.head().unwrap(), before_refusals);
    let (status, page) = json_response(
        f.router.clone(),
        get(resources_path, next, Some(personal_context)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(page["read_receipt"]["audit_event_id"].is_string());
}

#[tokio::test]
#[ignore = "requires front/dist, Node.js, Playwright, and Chromium"]
async fn chromium_oidc_refresh_withdraws_administration_and_keeps_personal_resources() {
    use agentsandbox::{api::dashboard_router_with_oidc, model::Event};
    use std::{
        path::{Path, PathBuf},
        process::Stdio,
    };

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let front = std::env::var_os("AI_GUARD_TEST_FRONT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("front/dist"));
    assert!(
        front.is_absolute() && front.join("index.html").is_file(),
        "build the frontend and use an absolute directory for the Chromium recipe"
    );
    let dashboard = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dashboard_origin = format!("http://{}", dashboard.local_addr().unwrap());
    let redirect_uri =
        reqwest::Url::parse(&format!("{dashboard_origin}/auth/oidc/callback")).unwrap();
    let mut browser_issuer = None;
    let issuer = ControlledIssuer::with_transport(None, |issuer| {
        let fixture = browser::BrowserIssuer::new(issuer, redirect_uri.clone());
        let routes = fixture.routes();
        browser_issuer = Some(fixture);
        routes
    })
    .await;
    let browser_issuer = browser_issuer.unwrap();
    let mut f = Fixture::with_issuer(support::private_tempdir(), issuer, None).await;
    f.config.web_origin = dashboard_origin.clone();
    f.config.provider.web_redirect_uri = redirect_uri.to_string();
    f.map_administration_role(browser::ADMIN_ROLE);
    const OTHER_OWNER: &str = "browser-refresh-other-owner";
    for subject in [browser::SUBJECT, OTHER_OWNER] {
        f.member(subject);
    }
    for (session, owner) in [
        ("fixture-personal-session", browser::SUBJECT),
        ("fixture-admin-only-session", OTHER_OWNER),
    ] {
        let mut event = Event::new("fixture.browser_refresh");
        event.session_id = Some(session.into());
        f.store.append(event).unwrap();
        f.store
            .mutate_identity(&IdentityMutationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                actor: PrincipalRefV1::LegacyControl {
                    credential_id: "fixture-operator".into(),
                    role: LegacyControlRoleV1::Runner,
                },
                change: IdentityChangeV1::ResourceGrant {
                    expected_revision: 0,
                    grant: ResourceGrantV1 {
                        version: 1,
                        tenant_id: "tenant-a".into(),
                        organization_scope: "organization-a".into(),
                        resource: ResourceRefV1 {
                            kind: ResourceKindV1::Session,
                            resource_id: session.into(),
                        },
                        owner: PrincipalRefV1::HumanOidc {
                            issuer: f.issuer.issuer.clone(),
                            subject: owner.into(),
                        },
                        revision: 1,
                        active: true,
                        shares: vec![],
                    },
                },
            })
            .unwrap();
    }

    let gateway = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_origin = format!("http://{}", gateway.local_addr().unwrap());
    let dashboard_app =
        dashboard_router_with_oidc(&gateway_origin, None, front, Some(dashboard_origin.clone()))
            .unwrap();
    let gateway_app = f.router.clone();
    let gateway_task = tokio::spawn(async move { axum::serve(gateway, gateway_app).await });
    let dashboard_task = tokio::spawn(async move { axum::serve(dashboard, dashboard_app).await });

    let public_status = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
        .get(format!("{dashboard_origin}/auth/oidc/status"))
        .header("sec-fetch-site", "same-origin")
        .header("sec-fetch-mode", "cors")
        .send()
        .await
        .expect("the Rust dashboard must serve the public SSO status");
    let public_http = public_status.status();
    let public: Value = public_status.json().await.unwrap_or(Value::Null);
    let metadata_match = public
        == json!({"version":1,"enabled":true,"issuer":f.issuer.issuer,"provider_label":f.config.provider_label});
    println!(
        "oidc_signed_fixture_refresh rust_public_status_http={} metadata_match={metadata_match}",
        public_http.as_u16()
    );
    assert!(public_http == StatusCode::OK && metadata_match);

    let mut driver = tokio::process::Command::new("node");
    driver
        .current_dir(root)
        .arg(root.join("front/tests/browser_oidc_refresh.mjs"))
        .env_clear()
        .env("AI_GUARD_TEST_OIDC_DASHBOARD", dashboard_origin)
        .env("AI_GUARD_TEST_OIDC_ISSUER", &f.issuer.issuer)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    for key in [
        "PATH",
        "HOME",
        "TMPDIR",
        "NODE_PATH",
        "CODEX_PRIMARY_RUNTIME_NODE_MODULES",
        "PLAYWRIGHT_BROWSERS_PATH",
        "PLAYWRIGHT_CHROMIUM_EXECUTABLE",
        "AI_GUARD_TEST_ARTIFACT_DIR",
    ] {
        if let Some(value) = std::env::var_os(key) {
            driver.env(key, value);
        }
    }
    let result = tokio::time::timeout(Duration::from_secs(60), driver.output()).await;
    gateway_task.abort();
    dashboard_task.abort();
    let result = result
        .expect("Chromium recipe exceeded its deadline")
        .expect("could not run the Chromium driver");
    let report: Value = serde_json::from_slice(&result.stdout).unwrap_or(Value::Null);
    let status = match report["status"].as_str() {
        Some(status @ ("PASS" | "FAIL" | "BLOCKED")) => status,
        _ => "UNKNOWN",
    };
    let stage = match report["stage"].as_str() {
        Some(
            stage @ ("configuration" | "chromium" | "public_status" | "login" | "admin_read"
            | "scheduled_refresh" | "admin_purged" | "personal_read" | "complete"),
        ) => stage,
        _ => "unknown",
    };
    let admin_purge = report["adminPurge"].as_bool().unwrap_or(false);
    let personal_read = report["personalRead"].as_bool().unwrap_or(false);
    let old_stream_closed = report["oldStreamClosed"].as_bool().unwrap_or(false);
    println!(
        "oidc_signed_fixture_refresh status={status} stage={stage} admin_purge={admin_purge} personal_read={personal_read} old_stream_closed={old_stream_closed}"
    );
    if let Some(diagnostic) = report["diagnostic"].as_str().filter(|code| {
        [
            "dashboard_navigation_error",
            "dashboard_navigation_http",
            "public_status_missing",
            "public_status_http",
            "public_status_json",
            "public_status_version",
            "public_status_disabled",
            "public_status_provider",
            "public_status_issuer",
            "browser_route_blocked",
        ]
        .contains(code)
    }) {
        println!("browser_diagnostic={diagnostic}");
    }
    for (field, label) in [
        ("dashboardHttp", "dashboard_http"),
        ("publicStatusHttp", "browser_public_status_http"),
    ] {
        if let Some(code) = report[field]
            .as_u64()
            .filter(|code| (100..=599).contains(code))
        {
            println!("{label}={code}");
        }
    }
    if let Some(code) = report["publicStatusError"].as_str().filter(|code| {
        [
            "origin_denied",
            "oidc_not_configured",
            "identity_unavailable",
            "audit_unavailable",
            "invalid_request",
            "capacity_exhausted",
        ]
        .contains(code)
    }) {
        println!("browser_public_status_error={code}");
    }
    if let Some(directory) = report["artifacts"]["directory"].as_str().filter(|name| {
        name.strip_prefix("browser-failure-").is_some_and(|suffix| {
            suffix.len() == 6 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
        })
    }) && let Some(files) = report["artifacts"]["files"].as_array()
    {
        for file in ["page.html", "page.png"] {
            if files.iter().any(|entry| entry == file) {
                println!("artifact={directory}/{file}");
            }
        }
    }
    assert!(
        result.status.success()
            && report["mode"] == "oidc_signed_fixture_refresh"
            && status == "PASS"
            && stage == "complete"
            && admin_purge
            && personal_read
            && old_stream_closed,
        "Chromium recipe failed at {stage}, exit status {}",
        result.status
    );
    assert!(
        browser_issuer.completed(),
        "the signed issuer must accept exactly one refresh exchange"
    );
    assert_eq!(
        f.audit.verify().unwrap().records,
        5,
        "login and refresh must commit their durable phases"
    );
    assert!(f.store.integrity().unwrap().valid);
    println!(
        "oidc_signed_fixture_refresh durable_records=5 provider_refresh=true business_audit_valid=true"
    );
}

#[tokio::test]
async fn web_refresh_refuses_terminal_capacity_before_contacting_the_token_endpoint() {
    use agentsandbox::audit::authentication::AuthenticationSessionStateV1;
    let f = Fixture::with_audit_limits(4, 131_072).await;
    f.member("capacity-member");
    let (delivery, nonce) = web_delivery(&f, "capacity-member", "capacity-old").await;
    let id = Uuid::parse_str(delivery["authentication_id"].as_str().unwrap()).unwrap();
    let before = f.audit.verify().unwrap();
    assert_eq!(
        before.records, 3,
        "real Web login must establish the three-record seed"
    );
    let binding = f.audit.session_current(id).unwrap().unwrap();
    assert_eq!(binding.state, AuthenticationSessionStateV1::Active);
    assert_eq!(binding.binding.generation, 1);
    let token_requests = f
        .issuer
        .requests()
        .iter()
        .filter(|(path, _)| path.ends_with("/token"))
        .count();
    assert_eq!(
        token_requests, 1,
        "setup must perform the real code exchange"
    );
    // If the gate is missing, this synthetic provider is ready to return a
    // legitimate refreshed response. No provider barrier or error creates 503.
    f.issuer
        .login_response(&nonce, "capacity-member", "capacity-next");
    let (status, response) = json_response(
        f.router.clone(),
        post(
            "/auth/oidc/refresh",
            json!({"version":1,"authentication_id":id,"generation":1,
                "refresh_token":delivery["refresh_token"]}),
            Some(delivery["access_token"].as_str().unwrap()),
            None,
            Some(ORIGIN),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response["error"]["code"], "audit_unavailable");
    assert_eq!(
        f.issuer
            .requests()
            .iter()
            .filter(|(path, _)| path.ends_with("/token"))
            .count(),
        token_requests,
        "terminal capacity refusal must occur before any provider refresh exchange"
    );
    assert_eq!(
        f.audit.verify().unwrap(),
        before,
        "refusal must not consume the last record as RefreshPending"
    );
    assert_eq!(
        f.audit.session_current(id).unwrap().unwrap(),
        binding,
        "the durable generation must remain Active rather than Pending"
    );
    // This asserts durable audit truth only. It does not promise that the old
    // bearer remains admitted by every volatile gateway/authentication path.
}
async fn web_delivery_roles(
    f: &Fixture,
    subject: &str,
    salt: &str,
    roles: &[&str],
) -> (Value, String) {
    let seeded = f
        .router
        .clone()
        .oneshot(
            Request::get("/auth/oidc/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(seeded.status(), StatusCode::OK);
    assert!(seeded.headers().get("set-cookie").is_none());
    let start_response = f
        .router
        .clone()
        .oneshot(post(
            "/auth/oidc/start",
            json!({"version":1}),
            None,
            None,
            Some(&f.config.web_origin),
        ))
        .await
        .unwrap();
    assert_eq!(start_response.status(), StatusCode::OK);
    let cookie = start_response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let start: Value =
        serde_json::from_slice(&to_bytes(start_response.into_body(), 65536).await.unwrap())
            .unwrap();
    let correlation_token = start["correlation_token"].as_str().unwrap().to_owned();
    let url = reqwest::Url::parse(start["authorization_url"].as_str().unwrap()).unwrap();
    let params = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    let nonce = params["nonce"].to_string();
    f.issuer
        .login_response_with_roles(&nonce, subject, salt, roles);
    let callback = f
        .router
        .clone()
        .oneshot(
            Request::get(format!(
                "/auth/oidc/callback?code=controlled&state={}",
                params["state"]
            ))
            .header("cookie", &cookie)
            .header("sec-fetch-site", "cross-site")
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(callback.status(), StatusCode::SEE_OTHER);
    let (status, delivery) = json_response(
        f.router.clone(),
        post(
            "/auth/oidc/tokens",
            json!({"version":1,"correlation_token":correlation_token}),
            None,
            Some(&cookie),
            Some(&f.config.web_origin),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    (delivery, nonce)
}

#[tokio::test]
async fn tls_pkce_broker_reads_require_both_explicit_membership_and_verified_claim_ceiling() {
    let files = support::private_tempdir();
    let pki = files.path().join("pki");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_ai-guard-demo-pki"))
        .env_clear()
        .arg("--out")
        .arg(&pki)
        .args(["--dns", "localhost"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(120), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    let ca = std::fs::read_to_string(pki.join("root-ca.pem")).unwrap();
    let identity = agentsandbox::tls::ServerIdentity::from_pem(
        &std::fs::read(pki.join("server-chain.pem")).unwrap(),
        &std::fs::read(pki.join("server.key")).unwrap(),
    )
    .unwrap();
    let issuer = ControlledIssuer::with_tls(identity).await;
    let mut f = Fixture::with_issuer(files, issuer, Some(ca)).await;
    f.store
        .migrate_workload_v6(&f._files.path().join("before-v6"))
        .unwrap();
    f.store
        .migrate_human_brokers_v7(&f._files.path().join("before-v7"))
        .unwrap();
    f.member("explicit-broker-reader");
    f.member("other-reader");
    let broker = HumanCapabilityV1::ReadBrokerRegistry;
    let rule = |capability| agentsandbox::api::OidcClaimRuleV1 {
        claim_path: vec!["roles".into()],
        equals: "broker-reader".into(),
        capabilities: BTreeSet::from([capability]),
    };
    f.config.mapping.rules = vec![rule(broker)];
    f.config.mapping.revision = "broker-mapping-1".into();
    f.router = Fixture::router_with_ca(&f.store, &f.audit, &f.config, f.ca_bundle_pem.clone());
    let request = |path: &str, token: &str, context: Option<&str>| {
        let mut request = Request::get(path).header("authorization", format!("Bearer {token}"));
        if let Some(context) = context {
            request = request.header("x-aiguard-context", context);
        }
        request.body(Body::empty()).unwrap()
    };
    let selected = |identity: &Value, mode: &str| {
        identity["contexts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|context| context["mode"] == mode)
            .map(|context| context["id"].as_str().unwrap().to_owned())
    };
    // Mapping alone cannot promote the historical membership.
    let (delivery, _) = web_delivery_roles(
        &f,
        "explicit-broker-reader",
        "mapping-alone",
        &["broker-reader"],
    )
    .await;
    let token = delivery["access_token"].as_str().unwrap();
    let (status, identity) = json_response(
        f.router.clone(),
        request("/api/v1/human/identity", token, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(selected(&identity, "administration").is_none());
    assert_eq!(
        json_response(
            f.router.clone(),
            request(
                "/api/v1/human/brokers",
                token,
                selected(&identity, "personal").as_deref()
            )
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let principal = PrincipalRefV1::HumanOidc {
        issuer: f.issuer.issuer.clone(),
        subject: "explicit-broker-reader".into(),
    };
    let mut membership = f.store.identity_memberships(&principal).unwrap().remove(0);
    membership.revision += 1;
    membership.capabilities.insert(broker);
    let mutation = |membership: MembershipV1| IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "fixture-operator".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change: IdentityChangeV1::Membership {
            expected_revision: membership.revision - 1,
            membership,
        },
    };
    f.store
        .mutate_identity(&mutation(membership.clone()))
        .unwrap();
    // Membership alone with an old verified ceiling still cannot read brokers.
    f.config.mapping.rules = vec![rule(HumanCapabilityV1::ReadTenantMetadata)];
    f.config.mapping.revision = "broker-old-ceiling".into();
    f.router = Fixture::router_with_ca(&f.store, &f.audit, &f.config, f.ca_bundle_pem.clone());
    let (delivery, _) = web_delivery_roles(
        &f,
        "explicit-broker-reader",
        "membership-alone",
        &["broker-reader"],
    )
    .await;
    let token = delivery["access_token"].as_str().unwrap();
    let (status, identity) = json_response(
        f.router.clone(),
        request("/api/v1/human/identity", token, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let before = f.store.head().unwrap();
    assert_eq!(
        json_response(
            f.router.clone(),
            request(
                "/api/v1/human/brokers",
                token,
                selected(&identity, "administration").as_deref()
            )
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(f.store.head().unwrap(), before);
    f.config.mapping.rules = vec![rule(broker)];
    f.config.mapping.revision = "broker-explicit-both".into();
    f.router = Fixture::router_with_ca(&f.store, &f.audit, &f.config, f.ca_bundle_pem.clone());
    let (delivery, _) = web_delivery_roles(
        &f,
        "explicit-broker-reader",
        "both-grants",
        &["broker-reader"],
    )
    .await;
    let token = delivery["access_token"].as_str().unwrap().to_owned();
    let (status, identity) = json_response(
        f.router.clone(),
        request("/api/v1/human/identity", &token, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let context = selected(&identity, "administration").unwrap();
    let (status, page) = json_response(
        f.router.clone(),
        request("/api/v1/human/brokers", &token, Some(&context)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["source"], "tenant_broker_registry");
    assert!(page["read_receipt"]["audit_event_id"].is_string());
    let (other, _) =
        web_delivery_roles(&f, "other-reader", "foreign-proof", &["broker-reader"]).await;
    let before = f.store.head().unwrap();
    assert_eq!(
        json_response(
            f.router.clone(),
            request(
                "/api/v1/human/brokers",
                other["access_token"].as_str().unwrap(),
                Some(&context)
            )
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // A signed token which has no real PKCE admission is never a human proof.
    let unadmitted = f.issuer.token(
        "web-public",
        "explicit-broker-reader",
        None,
        &["broker-reader"],
        "fresh-valid-client-without-admission",
    );
    assert_eq!(
        json_response(
            f.router.clone(),
            request("/api/v1/human/brokers", &unadmitted, Some(&context))
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(f.store.head().unwrap(), before);
    membership.revision += 1;
    membership.capabilities.remove(&broker);
    f.store.mutate_identity(&mutation(membership)).unwrap();
    let before = f.store.head().unwrap();
    assert_eq!(
        json_response(
            f.router.clone(),
            request("/api/v1/human/brokers", &token, Some(&context))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(f.store.head().unwrap(), before);
    let mut forbidden_default = f.config.clone();
    forbidden_default
        .mapping
        .default_capabilities
        .insert(broker);
    let requests = f.issuer.requests().len();
    assert!(forbidden_default.validate().is_err());
    assert_eq!(f.issuer.requests().len(), requests);
    assert!(
        f.issuer
            .requests()
            .iter()
            .any(|(path, body)| path.ends_with("/token") && body.contains("code_verifier=")),
        "real PKCE exchange must use its private verifier"
    );
    assert!(f.store.integrity().unwrap().valid);
}

#[tokio::test]
async fn cancelling_owned_web_refresh_future_cleans_pending_capacity_and_fences_late_response() {
    use agentsandbox::audit::authentication::AuthenticationSessionStateV1;
    let f = Fixture::new().await;
    f.member("cancel-member");
    let (delivery, nonce) = web_delivery(&f, "cancel-member", "cancel-old").await;
    let id = Uuid::parse_str(delivery["authentication_id"].as_str().unwrap()).unwrap();
    let old = delivery["access_token"].as_str().unwrap();
    let next = f
        .issuer
        .login_response(&nonce, "cancel-member", "cancel-next");
    let gate = f.issuer.block_token();
    let before = f.audit.verify().unwrap().records;
    let token_requests = f
        .issuer
        .requests()
        .iter()
        .filter(|(path, _)| path.ends_with("/token"))
        .count();
    let started = tokio::time::Instant::now();
    // Explicitly owned future cancellation; this does not claim that a real
    // TCP peer disconnect is delivered as handler cancellation by Axum.
    let refreshing=tokio::spawn(f.router.clone().oneshot(post("/auth/oidc/refresh",json!({"version":1,"authentication_id":id,"generation":1,"refresh_token":delivery["refresh_token"]}),Some(old),None,Some(ORIGIN))));
    tokio::time::timeout(Duration::from_secs(2), gate.entered())
        .await
        .expect("actual provider refresh must begin before cancellation");
    assert!(matches!(
        f.audit.session_current(id).unwrap().unwrap().state,
        AuthenticationSessionStateV1::RefreshPending { .. }
    ));
    refreshing.abort();
    assert!(refreshing.await.unwrap_err().is_cancelled());
    let cleanup_bound =
        Duration::from_millis(f.config.provider.limits.request_timeout_ms) + Duration::from_secs(1);
    tokio::time::sleep_until(started + cleanup_bound).await;
    assert_eq!(
        json_response(
            f.router.clone(),
            Request::get("/auth/oidc/status")
                .body(Body::empty())
                .unwrap()
        )
        .await
        .0,
        StatusCode::OK,
        "public status drives bounded lazy cleanup"
    );
    let cleaned = f.audit.session_current(id).unwrap().unwrap();
    let after_cleanup = f.audit.verify().unwrap().records;
    // Release the signed response only after the cancellation/cleanup boundary.
    // The HTTP server may itself have dropped the cancelled exchange; this
    // control does not require or claim delivery to the abandoned client.
    gate.release();
    tokio::task::yield_now().await;
    let (next_status, _) = json_response(
        f.router.clone(),
        Request::get("/api/v1/human/identity")
            .header("authorization", format!("Bearer {next}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let final_state = f.audit.session_current(id).unwrap().unwrap();
    assert!(
        matches!(cleaned.state, AuthenticationSessionStateV1::Closed { .. }),
        "cancelled Web refresh must not remain Pending beyond its bounded cleanup deadline"
    );
    assert_eq!(cleaned.binding.generation, 1);
    assert!(
        after_cleanup >= before + 2,
        "pending and terminal audit phases are required"
    );
    assert_eq!(next_status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        final_state, cleaned,
        "late response cannot reopen a closed generation"
    );
    assert_eq!(
        f.issuer
            .requests()
            .iter()
            .filter(|(path, _)| path.ends_with("/token"))
            .count(),
        token_requests + 1,
        "no retry of the ambiguous provider exchange"
    );
    // Public behavior proves the cancelled entry no longer consumes one of
    // the 64 per-principal slots. Every replacement is a real signed admission.
    for index in 0..64 {
        let token = f.issuer.token(
            "cli-public",
            "cancel-member",
            None,
            &[],
            &format!("capacity-after-cancel-{index}"),
        );
        let mut intent = json_response(
            f.router.clone(),
            post(
                "/auth/oidc/native-intent",
                json!({"version":1,"action":{"kind":"login"}}),
                None,
                None,
                None,
            ),
        )
        .await;
        if intent.0 == StatusCode::TOO_MANY_REQUESTS {
            tokio::time::sleep(Duration::from_secs(1)).await;
            intent = json_response(
                f.router.clone(),
                post(
                    "/auth/oidc/native-intent",
                    json!({"version":1,"action":{"kind":"login"}}),
                    None,
                    None,
                    None,
                ),
            )
            .await;
        }
        assert_eq!(intent.0, StatusCode::OK, "bounded anonymous rate recovery");
        assert_eq!(
            json_response(
                f.router.clone(),
                post(
                    "/auth/oidc/admit",
                    json!({"version":1,"intent_id":intent.1["intent_id"]}),
                    Some(&token),
                    None,
                    None
                )
            )
            .await
            .0,
            StatusCode::CREATED,
            "cancelled pending session must release its principal capacity before replacement {index}"
        );
    }
}

#[tokio::test]
async fn v1_planted_cookie_cannot_hijack_browser_token_delivery() {
    let f = Fixture::new().await;
    f.member("v1-victim");
    let planted_value = format!("{}abcd", "c0ffee".repeat(10));
    let planted_cookie = format!("aiguard_oidc={planted_value}");

    // (1) GET /status never emits Set-Cookie, whether cookie is absent or planted.
    for header in [None, Some(planted_cookie.as_str())] {
        let mut req = Request::get("/auth/oidc/status");
        if let Some(cookie) = header {
            req = req.header("cookie", cookie);
        }
        let status_res = f
            .router
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(status_res.status(), StatusCode::OK);
        assert!(status_res.headers().get("set-cookie").is_none());
    }

    // (2) POST /start with planted cookie C ignores C, rotates to fresh_cookie,
    //     and returns an independent 64-hex correlation_token.
    let start_res = f
        .router
        .clone()
        .oneshot(post(
            "/auth/oidc/start",
            json!({"version":1}),
            None,
            Some(&planted_cookie),
            Some(ORIGIN),
        ))
        .await
        .unwrap();
    assert_eq!(start_res.status(), StatusCode::OK);
    let set_cookie_header = start_res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(set_cookie_header.contains("Path=/auth/oidc; HttpOnly; SameSite=Lax; Max-Age=300"));
    let start_cookie = set_cookie_header.split(';').next().unwrap().to_owned();
    assert_ne!(start_cookie, planted_cookie);
    let start_body: Value =
        serde_json::from_slice(&to_bytes(start_res.into_body(), 65536).await.unwrap()).unwrap();
    let correlation_token = start_body["correlation_token"].as_str().unwrap().to_owned();
    assert_eq!(correlation_token.len(), 64);
    assert!(
        correlation_token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert_ne!(format!("aiguard_oidc={correlation_token}"), start_cookie);

    let url = reqwest::Url::parse(start_body["authorization_url"].as_str().unwrap()).unwrap();
    let params = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    let victim_token = f
        .issuer
        .login_response(&params["nonce"], "v1-victim", "v1-victim-login");

    // Old planted cookie cannot complete callback.
    let stale_callback = f
        .router
        .clone()
        .oneshot(
            Request::get(format!(
                "/auth/oidc/callback?code=controlled-code&state={}",
                params["state"]
            ))
            .header("cookie", &planted_cookie)
            .header("sec-fetch-site", "cross-site")
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale_callback.status(), StatusCode::CONFLICT);

    // (3) Victim completes callback with rotated cookie.
    let callback_res = f
        .router
        .clone()
        .oneshot(
            Request::get(format!(
                "/auth/oidc/callback?code=controlled-code&state={}",
                params["state"]
            ))
            .header("cookie", &start_cookie)
            .header("sec-fetch-site", "cross-site")
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(callback_res.status(), StatusCode::SEE_OTHER);
    assert_eq!(callback_res.headers()["location"], "/auth/oidc/complete");

    // (4) Attacker with old planted cookie C fails: 400 without correlation_token,
    //     409 with guessed 64-hex correlation_token.
    let (legacy_status, legacy_body) = json_response(
        f.router.clone(),
        Request::post("/auth/oidc/tokens")
            .header("content-type", "application/json")
            .header("cookie", &planted_cookie)
            .header("origin", ORIGIN)
            .body(Body::from(json!({"version":1}).to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(legacy_status, StatusCode::BAD_REQUEST);
    assert_eq!(legacy_body["error"]["code"], "invalid_request");

    let wrong_token = "f".repeat(64);
    assert_ne!(wrong_token, correlation_token);
    let (planted_status, planted_body) = json_response(
        f.router.clone(),
        Request::post("/auth/oidc/tokens")
            .header("content-type", "application/json")
            .header("cookie", &planted_cookie)
            .header("origin", ORIGIN)
            .body(Body::from(
                json!({"version":1,"correlation_token":wrong_token}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(planted_status, StatusCode::CONFLICT);
    assert_eq!(planted_body["error"]["code"], "session_conflict");

    // (5) Attacker with rotated cookie and wrong correlation_token fails 401 WITHOUT evicting delivery;
    //     malformed DTOs fail 400 and cross-site Fetch Metadata fails 403.
    let (wrong_status, wrong_body) = json_response(
        f.router.clone(),
        Request::post("/auth/oidc/tokens")
            .header("content-type", "application/json")
            .header("cookie", &start_cookie)
            .header("origin", ORIGIN)
            .body(Body::from(
                json!({"version":1,"correlation_token":wrong_token}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(wrong_status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong_body["error"]["code"], "credential_invalid");

    for bad_body in [
        json!({"version":1}),
        json!({"version":2,"correlation_token":correlation_token}),
        json!({"version":1,"correlation_token":"short"}),
        json!({"version":1,"correlation_token":"A".repeat(64)}),
        json!({"version":1,"correlation_token":correlation_token,"extra":true}),
    ] {
        let (bad_status, bad_err) = json_response(
            f.router.clone(),
            post(
                "/auth/oidc/tokens",
                bad_body,
                None,
                Some(&start_cookie),
                Some(ORIGIN),
            ),
        )
        .await;
        assert_eq!(bad_status, StatusCode::BAD_REQUEST);
        assert_eq!(bad_err["error"]["code"], "invalid_request");
    }

    let (cross_status, cross_err) = json_response(
        f.router.clone(),
        Request::post("/auth/oidc/tokens")
            .header("content-type", "application/json")
            .header("cookie", &start_cookie)
            .header("origin", ORIGIN)
            .header("sec-fetch-site", "cross-site")
            .body(Body::from(
                json!({"version":1,"correlation_token":correlation_token}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(cross_status, StatusCode::FORBIDDEN);
    assert_eq!(cross_err["error"], "Requête cross-site refusée.");

    let (same_site_status, same_site_err) = json_response(
        f.router.clone(),
        Request::post("/auth/oidc/tokens")
            .header("content-type", "application/json")
            .header("cookie", &start_cookie)
            .header("origin", ORIGIN)
            .header("sec-fetch-site", "same-site")
            .body(Body::from(
                json!({"version":1,"correlation_token":correlation_token}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(same_site_status, StatusCode::FORBIDDEN);
    assert_eq!(same_site_err["error"]["code"], "origin_denied");

    // (6) Legitimate victim with rotated cookie + correlation_token redeems one-shot (200).
    let victim_res = f
        .router
        .clone()
        .oneshot(
            Request::post("/auth/oidc/tokens")
                .header("content-type", "application/json")
                .header("cookie", &start_cookie)
                .header("origin", ORIGIN)
                .header("sec-fetch-site", "same-origin")
                .header("sec-fetch-mode", "cors")
                .body(Body::from(
                    json!({"version":1,"correlation_token":correlation_token}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(victim_res.status(), StatusCode::OK);
    let clear_header = victim_res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        clear_header
            .starts_with("aiguard_oidc=; Path=/auth/oidc; HttpOnly; SameSite=Lax; Max-Age=0")
    );
    let victim_delivery: Value =
        serde_json::from_slice(&to_bytes(victim_res.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(victim_delivery["access_token"], victim_token);

    // (7) Replay returns 409 session_conflict.
    let (replay_status, replay_body) = json_response(
        f.router.clone(),
        post(
            "/auth/oidc/tokens",
            json!({"version":1,"correlation_token":correlation_token}),
            None,
            Some(&start_cookie),
            Some(ORIGIN),
        ),
    )
    .await;
    assert_eq!(replay_status, StatusCode::CONFLICT);
    assert_eq!(replay_body["error"]["code"], "session_conflict");

    // (8) Victim's bearer authenticates normally.
    let (ident_status, ident_body) = json_response(
        f.router.clone(),
        Request::get("/api/v1/human/identity")
            .header("authorization", format!("Bearer {victim_token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(ident_status, StatusCode::OK);
    assert_eq!(ident_body["principal"]["subject"], "v1-victim");
}

#[tokio::test]
async fn v1_cookie_syntax_rejects_malformed_and_duplicate_cookies_without_state_leak() {
    let f = Fixture::new().await;
    for bad_cookie in [
        "aiguard_oidc=bad-value".to_owned(),
        format!(
            "aiguard_oidc={}; aiguard_oidc={}",
            "a".repeat(64),
            "b".repeat(64)
        ),
        format!("aiguard_oidc={}", "a".repeat(4097)),
    ] {
        let (st_status, st_err) = json_response(
            f.router.clone(),
            Request::get("/auth/oidc/status")
                .header("cookie", &bad_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(st_status, StatusCode::BAD_REQUEST);
        assert_eq!(st_err["error"]["code"], "invalid_request");

        let (start_status, start_err) = json_response(
            f.router.clone(),
            post(
                "/auth/oidc/start",
                json!({"version":1}),
                None,
                Some(&bad_cookie),
                Some(ORIGIN),
            ),
        )
        .await;
        assert_eq!(start_status, StatusCode::BAD_REQUEST);
        assert_eq!(start_err["error"]["code"], "invalid_request");

        let (tok_status, tok_err) = json_response(
            f.router.clone(),
            post(
                "/auth/oidc/tokens",
                json!({"version":1,"correlation_token":"a".repeat(64)}),
                None,
                Some(&bad_cookie),
                Some(ORIGIN),
            ),
        )
        .await;
        assert_eq!(tok_status, StatusCode::BAD_REQUEST);
        assert_eq!(tok_err["error"]["code"], "invalid_request");
    }
}
