//! Product boundary tests using an owned loopback HTTP server and public test-only RSA material.
//! They do not qualify a real provider or relax the normal HTTPS policy.
use agentsandbox::oidc::{
    OidcClientKind, OidcConfigV1, OidcError, OidcLimitsV1, OidcProvider, OidcTransportModeV1,
};
use axum::{Router, body::Body, extract::State, http::Request, response::Response};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// Public synthetic RSA fixture from jsonwebtoken 11.1.0's MIT-licensed RSA tests.
// It is deliberately not a credential for any ai-guard installation.
const RSA_DER_HEX: &str = concat!(
    "308204a40201000282010100c9113aac7b8d47441b1cedc7dcab76a4e286",
    "56142a1995c89ce76e40dc57cee2a5bd04cb513bf8978b20821e7f098622",
    "fdcbc8f925d54fd90f592297c495c15ddff82e4bdc3ee51a901a0091f87e",
    "7a2155321d95ad4c96ca3dcc165d074d517d2b04572c07309111224b79e9",
    "4e11d1c88c6ecb464c7997f154be5aacc870d524442c1f07a067c6fc0b47",
    "f3d04813d8c304767d74b7a52bd6b5f38cc07fc2f0a0f2f1bc96f7225e67",
    "9dca8f7127ca0c3a1d30504831ce254330ca2f982f9a25cb5c1d4018b9bc",
    "2818df13cb372f9c6a8bec94a1dfa3f0cb6f223f35d9d912e1032245537f",
    "6f2da1dd963c2d8546aea6576537209f6ba39fcb8a8d72d9543e53750203",
    "0100010282010074449342343bd0ef10229d594a40985de663ba18f4f350",
    "8aee3861a7fe0284aec91a515064cc220737bb77a2bce13c00b5f4066614",
    "3b926db913590e9bede12030a829d55229c87fa0be787ff04aeac2a5aa52",
    "3340401e9e1a380ee2829b7238c80c3d8047340d5ef40ee3134742c4a58c",
    "9770d258dca3413938cde44610011f6cd8c7aeae980ad8c749948b996886",
    "f348677e7628b3d84eaf92672e7c92ed998513fe4fdd756bc4b73fb50e73",
    "ba6d11e1c6eb8331e240fe5181fc4625a122ed772085eac208e82eac9492",
    "6d81f6f46cfcbb97c56d2f9883cb3e3a20acb3772cd6b4a027dfde1d9bb3",
    "1847546550111fa8553234aec2fa954c5684f0082d95e102818100fae2cc",
    "9f890da879494b6039b809c2923f7467112d366dc4f2103152b86cb24b58",
    "e5748e00f065c727f946e446208986b54548af81ee2526b94d7e3c9f755d",
    "b7d3180a9acefe2625ea6a0d4523ede6aa302083899888d8da6a8f0df0ed",
    "ff3c3e7dc0c3de598226b165cdc9e1d2e79ad42a21c446d09cbddcca978b",
    "b7e59b53a902818100cd2a75f66d5a8e278592a828074613a6dfceae80f1",
    "dfcbef239ba722726511e3a343ea0801074ddb0a7106f4eb468acc53a3f4",
    "ee61546edd63f82a3a09b362c3e5fc1dc09aa982d1e5c95b24e396351dfa",
    "3eeccea502e47caf30a2ea41c735bd07c819998f2b07599b27efd64cfac4",
    "272e981a62040645b2950d95a56f0ee0ed02818100a5d6efbdf1419700fd",
    "6316eb706cfb31e34215b333f8cb9ca909ed20aa0a27dd4898fc6fb5cd32",
    "e9efd1867bf4a323656943b0255bb4c8d80d1b5d7bc34838b7ff90dfc9af",
    "542e7157bfdc9fbc7d50c164e8c92a562af7758184a8897f38fd60ad8d93",
    "abd1ed0798b21bd2fc3486a640e2ccc94aaa31420bc5db611f2166886902",
    "8181009fdf8f8e98c630f1f12f4823f10f156f3b05b68bba6f575e211440",
    "d821e64a5b65b51d606154ccc17c5079484f32682432a517cece3f5a28e0",
    "a6d858e3884ad5369910a22ea31ad2ef49c9e1640b7f96bd935d3ae5f0ec",
    "1590fac868dd1b1bf0126946319813c2a57bb752385f4225aa16afe6172c",
    "e5aa582fb8273d83de473d0281800b1c28ff262f174ac0e817f3215619c6",
    "748b363fdf4d35d9a8dd1d8fbf40ae65b0ca314e79f4b24752faca4bc5a4",
    "b96443b8aaf3d56fc440a0291630c6a5da93a19aab894150cf0cbb991dda",
    "c888a883336d1dcbdb0c7c7ca059c663438592c5ffc9ed3f5c6a72d92171",
    "611c0653df073d8781bc0410ad58dd9337f2ec199741",
);
const RSA_N: &str = "yRE6rHuNR0QbHO3H3Kt2pOKGVhQqGZXInOduQNxXzuKlvQTLUTv4l4sggh5_CYYi_cvI-SXVT9kPWSKXxJXBXd_4LkvcPuUakBoAkfh-eiFVMh2VrUyWyj3MFl0HTVF9KwRXLAcwkREiS3npThHRyIxuy0ZMeZfxVL5arMhw1SRELB8HoGfG_AtH89BIE9jDBHZ9dLelK9a184zAf8LwoPLxvJb3Il5nncqPcSfKDDodMFBIMc4lQzDKL5gvmiXLXB1AGLm8KBjfE8s3L5xqi-yUod-j8MtvIj812dkS4QMiRVN_by2h3ZY8LYVGrqZXZTcgn2ujn8uKjXLZVD5TdQ";

#[derive(Default)]
struct Behavior {
    metadata: Value,
    jwks: Value,
    token: Value,
    discovery_status: u16,
    discovery_delay: Duration,
    token_delay: Duration,
    requests: Vec<(String, String)>,
    methods: Vec<String>,
    authorization_present: Vec<bool>,
}
struct ProviderFixture {
    issuer: String,
    state: Arc<Mutex<Behavior>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for ProviderFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl ProviderFixture {
    async fn new() -> Self {
        Self::with_tls(None).await
    }
    async fn with_tls(identity: Option<agentsandbox::tls::ServerIdentity>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let scheme = if identity.is_some() { "https" } else { "http" };
        let issuer = format!("{scheme}://{}/realm", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(Behavior {
            metadata: json!({
                "issuer": issuer,
                "authorization_endpoint": format!("{issuer}/authorize"),
                "token_endpoint": format!("{issuer}/token"),
                "jwks_uri": format!("{issuer}/keys"),
                "response_types_supported": ["code"],
                "subject_types_supported": ["public"],
                "id_token_signing_alg_values_supported": ["RS256"],
                "token_endpoint_auth_methods_supported": ["none"],
                "code_challenge_methods_supported": ["S256"]
            }),
            jwks: json!({"keys": [rsa_jwk("key-1")]}),
            token: json!({}),
            discovery_status: 200,
            ..Behavior::default()
        }));
        let app = Router::new().fallback(endpoint).with_state(state.clone());
        let task = tokio::spawn(async move {
            if let Some(identity) = identity {
                agentsandbox::tls::serve(listener, app, identity, std::future::pending::<()>())
                    .await
                    .unwrap();
            } else {
                axum::serve(listener, app).await.unwrap();
            }
        });
        Self {
            issuer,
            state,
            task,
        }
    }
    fn config(&self) -> OidcConfigV1 {
        OidcConfigV1 {
            version: 1,
            issuer: self.issuer.clone(),
            web_client_id: "web-public".into(),
            cli_client_id: "cli-public".into(),
            api_audience: "guard-api".into(),
            authorization_endpoint: format!("{}/authorize", self.issuer),
            token_endpoint: format!("{}/token", self.issuer),
            jwks_uri: format!("{}/keys", self.issuer),
            web_redirect_uri: if self.issuer.starts_with("https:") {
                "https://127.0.0.1:18100/auth/oidc/callback"
            } else {
                "http://127.0.0.1:18100/auth/oidc/callback"
            }
            .into(),
            cli_callback_path: "/oidc/callback".into(),
            end_session_endpoint: None,
            transport: if self.issuer.starts_with("https:") {
                OidcTransportModeV1::HttpsOnly
            } else {
                OidcTransportModeV1::IsolatedDemoHttp {
                    exact_issuer: self.issuer.clone(),
                }
            },
            limits: OidcLimitsV1::default(),
        }
    }
    fn hits(&self, suffix: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|(path, _)| path.ends_with(suffix))
            .count()
    }
}
async fn endpoint(State(state): State<Arc<Mutex<Behavior>>>, request: Request<Body>) -> Response {
    let path = request.uri().path().to_owned();
    let full_path = request.uri().to_string();
    let method = request.method().to_string();
    let authorization_present = request.headers().contains_key("authorization");
    let bytes = axum::body::to_bytes(request.into_body(), 32_768)
        .await
        .unwrap();
    let (body, status, delay) = {
        let mut state = state.lock().unwrap();
        state
            .requests
            .push((full_path, String::from_utf8(bytes.to_vec()).unwrap()));
        state.methods.push(method);
        state.authorization_present.push(authorization_present);
        if path.ends_with("/openid-configuration") {
            (
                state.metadata.to_string(),
                state.discovery_status,
                state.discovery_delay,
            )
        } else if path.ends_with("/keys") {
            (state.jwks.to_string(), 200, Duration::ZERO)
        } else if path.ends_with("/token") {
            (state.token.to_string(), 200, state.token_delay)
        } else if path.ends_with("/logout") {
            ("{}".into(), 200, Duration::ZERO)
        } else {
            ("{}".into(), 404, Duration::ZERO)
        }
    };
    tokio::time::sleep(delay).await;
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("location", "/realm/keys")
        .body(Body::from(body))
        .unwrap()
}
fn rsa_jwk(kid: &str) -> Value {
    json!({"kty":"RSA", "kid":kid, "alg":"RS256", "use":"sig", "key_ops":["verify"], "n":RSA_N, "e":"AQAB"})
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn access_claims(issuer: &str) -> Value {
    json!({"iss":issuer,"sub":"subject-a","aud":"guard-api","azp":"web-public","typ":"Bearer","iat":now(),"nbf":now()-1,"exp":now()+120,"resource_access":{"guard-api":{"roles":["auditor"]}}})
}
fn sign_with_header(claims: &Value, header: Header) -> String {
    jsonwebtoken::encode(
        &header,
        claims,
        &EncodingKey::from_rsa_der(&hex::decode(RSA_DER_HEX).unwrap()),
    )
    .unwrap()
}
fn sign(claims: &Value) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("key-1".into());
    sign_with_header(claims, header)
}

// A normal constructor must be offline, while HTTP requires two explicit matching opt-ins.
#[tokio::test]
async fn configuration_is_offline_and_http_is_never_the_default() {
    let fixture = ProviderFixture::new().await;
    let mut config = fixture.config();
    config.transport = OidcTransportModeV1::HttpsOnly;
    assert!(OidcProvider::new(config).is_err());
    assert!(OidcProvider::new(fixture.config()).is_ok());
    assert_eq!(fixture.hits("openid-configuration"), 0);
    let mut config = fixture.config();
    config.transport = OidcTransportModeV1::IsolatedDemoHttp {
        exact_issuer: "http://127.0.0.1/other".into(),
    };
    assert!(OidcProvider::new(config).is_err());
}

// Discovery may confirm operator choices, but cannot grant itself a destination or follow redirects.
#[tokio::test]
async fn discovery_rejects_unapproved_destinations_and_redirects_without_following_them() {
    let fixture = ProviderFixture::new().await;
    let trap = ProviderFixture::new().await;
    fixture.state.lock().unwrap().metadata["jwks_uri"] = json!(format!("{}/hostile", trap.issuer));
    let provider = OidcProvider::new(fixture.config()).unwrap();
    assert!(matches!(
        provider.discover().await,
        Err(OidcError::InvalidMetadata)
    ));
    assert_eq!(fixture.hits("openid-configuration"), 1);
    assert_eq!(fixture.hits("/keys"), 0);
    assert!(trap.state.lock().unwrap().requests.is_empty());
    fixture.state.lock().unwrap().metadata["jwks_uri"] = json!(format!("{}/keys", fixture.issuer));
    fixture.state.lock().unwrap().discovery_status = 302;
    assert!(provider.discover().await.is_err());
    assert_eq!(fixture.hits("openid-configuration"), 2);
    assert_eq!(fixture.hits("/keys"), 0);
    assert!(trap.state.lock().unwrap().requests.is_empty());
}

// A slow/oversized provider cannot consume an unbounded request or trigger an automatic retry.
#[tokio::test]
async fn discovery_enforces_response_and_total_time_budgets_without_retry() {
    let fixture = ProviderFixture::new().await;
    let mut config = fixture.config();
    config.limits.max_response_bytes = 1024;
    fixture.state.lock().unwrap().metadata["padding"] = json!("x".repeat(2048));
    let provider = OidcProvider::new(config).unwrap();
    assert!(provider.discover().await.is_err());
    assert_eq!(fixture.hits("openid-configuration"), 1);
    let mut config = fixture.config();
    config.limits.request_timeout_ms = 50;
    fixture.state.lock().unwrap().discovery_delay = Duration::from_millis(250);
    let provider = OidcProvider::new(config).unwrap();
    let started = std::time::Instant::now();
    assert!(provider.discover().await.is_err());
    assert!(started.elapsed() < Duration::from_millis(200));
    assert_eq!(fixture.hits("openid-configuration"), 2);
}

// Valid signatures are insufficient when the API token's purpose/issuer/client/time profile differs.
#[tokio::test]
async fn access_requires_rs256_and_the_exact_api_profile() {
    let fixture = ProviderFixture::new().await;
    let provider = OidcProvider::new(fixture.config()).unwrap();
    provider.discover().await.unwrap();
    let valid = access_claims(&fixture.issuer);
    let token = sign(&valid);
    let proof = provider.verify_access(&token).await.unwrap();
    assert_eq!(proof.issuer(), fixture.issuer);
    assert_eq!(proof.subject(), "subject-a");
    assert_eq!(proof.client_id(), "web-public");
    assert_eq!(
        proof.claims()["resource_access"]["guard-api"]["roles"],
        json!(["auditor"])
    );
    for (claim, value) in [
        ("iss", json!("https://other.example/realm")),
        ("aud", json!("web-public")),
        ("typ", json!("ID")),
        ("azp", json!("realm-admin")),
        ("sub", json!("")),
        ("exp", json!(now() - 20)),
        ("nbf", json!(now() + 30)),
        ("iat", json!(now() + 30)),
    ] {
        let mut invalid = valid.clone();
        invalid[claim] = value;
        assert!(
            provider.verify_access(&sign(&invalid)).await.is_err(),
            "accepted {claim}"
        );
    }
    // Keep iat/nbf and lifetime coherent so expiry is the only failing condition.
    let time = now();
    let mut old_but_valid = valid.clone();
    old_but_valid["iat"] = json!(time - 120);
    old_but_valid["nbf"] = json!(time - 120);
    old_but_valid["exp"] = json!(time + 120);
    assert!(provider.verify_access(&sign(&old_but_valid)).await.is_ok());
    let mut expired = old_but_valid;
    expired["exp"] = json!(time - 20);
    assert!(matches!(
        provider.verify_access(&sign(&expired)).await,
        Err(OidcError::InvalidToken)
    ));
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("key-1".into());
    header.typ = Some("at+jwt".into());
    assert!(
        provider
            .verify_access(&sign_with_header(&valid, header))
            .await
            .is_err()
    );
    let hmac = jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &valid,
        &EncodingKey::from_secret(b"not-an-rsa-key"),
    )
    .unwrap();
    assert!(provider.verify_access(&hmac).await.is_err());
    assert_eq!(
        fixture.hits("/keys"),
        1,
        "valid cached key should not refetch"
    );
    assert!(!format!("{proof:?}").contains("subject-a"));
    assert!(!format!("{proof:?}").contains("auditor"));
}

// Ambiguous or encryption-only keys must never be selected by their first matching kid.
#[tokio::test]
async fn jwks_rejects_duplicate_kids_and_incompatible_metadata() {
    let control = ProviderFixture::new().await;
    let provider = OidcProvider::new(control.config()).unwrap();
    provider.discover().await.unwrap();
    assert!(
        provider
            .verify_access(&sign(&access_claims(&control.issuer)))
            .await
            .is_ok()
    );
    assert_eq!(control.hits("/keys"), 1);
    for duplicate in [true, false] {
        let fixture = ProviderFixture::new().await;
        if duplicate {
            fixture.state.lock().unwrap().jwks =
                json!({"keys":[rsa_jwk("key-1"), rsa_jwk("key-1")]});
        } else {
            fixture.state.lock().unwrap().jwks["keys"][0]["use"] = json!("enc");
        }
        let provider = OidcProvider::new(fixture.config()).unwrap();
        match provider.discover().await {
            Err(error) => assert!(matches!(
                error,
                OidcError::InvalidKeys | OidcError::InvalidMetadata
            )),
            Ok(()) => assert!(matches!(
                provider
                    .verify_access(&sign(&access_claims(&fixture.issuer)))
                    .await,
                Err(OidcError::InvalidToken)
            )),
        }
        assert_eq!(fixture.hits("openid-configuration"), 1);
        assert_eq!(fixture.hits("/keys"), 1);
    }
}

// An untrusted JWT can request at most a rate-limited issuer-key refresh, never a jku/x5u fetch.
#[tokio::test]
async fn unknown_kid_is_rate_limited_and_token_urls_are_not_network_authority() {
    let fixture = ProviderFixture::new().await;
    let trap = ProviderFixture::new().await;
    let provider = OidcProvider::new(fixture.config()).unwrap();
    provider.discover().await.unwrap();
    assert!(
        provider
            .verify_access(&sign(&access_claims(&fixture.issuer)))
            .await
            .is_ok()
    );
    assert_eq!(fixture.hits("openid-configuration"), 1);
    assert_eq!(fixture.hits("/keys"), 1);
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("unknown".into());
    let unknown = sign_with_header(&access_claims(&fixture.issuer), header);
    assert!(matches!(
        provider.verify_access(&unknown).await,
        Err(OidcError::InvalidToken)
    ));
    assert_eq!(
        fixture.hits("/keys"),
        2,
        "one unknown kid refresh must use only the configured JWKS URL"
    );
    for _ in 0..4 {
        assert!(matches!(
            provider.verify_access(&unknown).await,
            Err(OidcError::InvalidToken)
        ));
        assert_eq!(
            fixture.hits("/keys"),
            2,
            "repeated unknown kid must remain inside cooldown"
        );
    }
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("key-1".into());
    header.jku = Some(format!("{}/hostile-jwks", trap.issuer));
    header.x5u = Some(format!("{}/hostile-cert", trap.issuer));
    let invalid = sign_with_header(&access_claims(&fixture.issuer), header);
    for _ in 0..4 {
        assert!(provider.verify_access(&invalid).await.is_err());
    }
    assert_eq!(fixture.hits("/keys"), 2);
    assert_eq!(fixture.hits("openid-configuration"), 1);
    assert!(trap.state.lock().unwrap().requests.is_empty());
    assert!(
        !format!(
            "{:?}",
            provider.verify_access("secret-canary-invalid").await
        )
        .contains("secret-canary")
    );
}

// Beginning a login must bind the exact redirect/client and generate PKCE S256 without an exchange.
#[tokio::test]
async fn public_login_uses_s256_and_exact_web_or_literal_loopback_redirects() {
    let fixture = ProviderFixture::new().await;
    let provider = OidcProvider::new(fixture.config()).unwrap();
    provider.discover().await.unwrap();
    assert!(
        provider
            .begin_login(OidcClientKind::Web, "http://127.0.0.1:18100/evil")
            .await
            .is_err()
    );
    assert!(
        provider
            .begin_login(OidcClientKind::Cli, "http://localhost:34000/oidc/callback")
            .await
            .is_err()
    );
    let pending = provider
        .begin_login(
            OidcClientKind::Web,
            "http://127.0.0.1:18100/auth/oidc/callback",
        )
        .await
        .unwrap();
    let url = reqwest::Url::parse(pending.authorization_url()).unwrap();
    let query: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(query.get("response_type").map(String::as_str), Some("code"));
    assert_eq!(
        query.get("client_id").map(String::as_str),
        Some("web-public")
    );
    assert_eq!(
        query.get("code_challenge_method").map(String::as_str),
        Some("S256")
    );
    assert!(query.get("code_challenge").unwrap().len() >= 43);
    assert!(!query.contains_key("client_secret"));
    assert!(!format!("{pending:?}").contains(query.get("state").unwrap()));
    assert!(!format!("{pending:?}").contains(query.get("nonce").unwrap()));
    assert!(
        provider
            .complete_login(pending, "code-canary", "wrong-state")
            .await
            .is_err()
    );
    assert_eq!(
        fixture.hits("/token"),
        0,
        "wrong state must fail before external exchange"
    );
    assert!(
        provider
            .begin_login(OidcClientKind::Cli, "http://127.0.0.1:34000/oidc/callback")
            .await
            .is_ok()
    );
}

fn login_query(
    pending: &agentsandbox::oidc::PendingLogin,
) -> std::collections::BTreeMap<String, String> {
    reqwest::Url::parse(pending.authorization_url())
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}
fn id_claims(issuer: &str, nonce: &str) -> Value {
    json!({"iss":issuer,"sub":"subject-a","aud":"web-public","azp":"web-public","iat":now(),"exp":now()+120,"nonce":nonce})
}
fn token_response(issuer: &str, id: &Value) -> Value {
    json!({"access_token":sign(&access_claims(issuer)),"id_token":sign(id),"token_type":"Bearer","expires_in":120,"refresh_token":"synthetic-refresh-canary"})
}

// The token endpoint must receive the private matching verifier and the public client, never a secret.
#[tokio::test]
async fn code_exchange_validates_id_and_access_separately_and_keeps_tokens_private() {
    let fixture = ProviderFixture::new().await;
    let provider = OidcProvider::new(fixture.config()).unwrap();
    provider.discover().await.unwrap();
    let pending = provider
        .begin_login(
            OidcClientKind::Web,
            "http://127.0.0.1:18100/auth/oidc/callback",
        )
        .await
        .unwrap();
    let query = login_query(&pending);
    fixture.state.lock().unwrap().token = token_response(
        &fixture.issuer,
        &id_claims(&fixture.issuer, &query["nonce"]),
    );
    let login = provider
        .complete_login(pending, "synthetic-code-canary", &query["state"])
        .await
        .unwrap();
    assert_eq!(login.access().subject(), "subject-a");
    assert_eq!(
        login.tokens().refresh_token(),
        Some("synthetic-refresh-canary")
    );
    assert!(
        provider
            .verify_access(login.tokens().access_token())
            .await
            .is_ok()
    );
    let body = fixture
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .find(|(path, _)| path.ends_with("/token"))
        .unwrap()
        .1
        .clone();
    let fields: std::collections::BTreeMap<_, _> =
        reqwest::Url::parse(&format!("https://fixture.invalid/?{body}"))
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
    assert_eq!(fields["grant_type"], "authorization_code");
    assert_eq!(fields["client_id"], "web-public");
    assert_eq!(fields["code"], "synthetic-code-canary");
    assert!(!fields.contains_key("client_secret"));
    let challenge = openidconnect::PkceCodeChallenge::from_code_verifier_sha256(
        &openidconnect::PkceCodeVerifier::new(fields["code_verifier"].clone()),
    );
    assert_eq!(challenge.as_str(), query["code_challenge"]);
    for secret in [
        "synthetic-refresh-canary",
        "subject-a",
        login.tokens().access_token(),
    ] {
        assert!(!format!("{login:?}").contains(secret));
        assert!(!format!("{:?}", login.tokens()).contains(secret));
    }
}

// Authentic ID signatures must not bypass nonce, azp, time, at_hash or access-subject binding.
#[tokio::test]
async fn login_rejects_id_token_confusion_and_mismatched_access_identity() {
    let fixture = ProviderFixture::new().await;
    let provider = OidcProvider::new(fixture.config()).unwrap();
    provider.discover().await.unwrap();
    for mutation in ["nonce", "azp", "aud", "iat", "at_hash", "access_sub"] {
        let pending = provider
            .begin_login(
                OidcClientKind::Web,
                "http://127.0.0.1:18100/auth/oidc/callback",
            )
            .await
            .unwrap();
        let query = login_query(&pending);
        let mut id = id_claims(&fixture.issuer, &query["nonce"]);
        match mutation {
            "nonce" => id["nonce"] = json!("wrong-nonce"),
            "azp" => id["azp"] = json!("cli-public"),
            "aud" => id["aud"] = json!("guard-api"),
            "iat" => id["iat"] = json!(now() + 60),
            "at_hash" => id["at_hash"] = json!("wrong-access-hash"),
            "access_sub" => {}
            _ => unreachable!(),
        }
        let mut response = token_response(&fixture.issuer, &id);
        if mutation == "access_sub" {
            let mut access = access_claims(&fixture.issuer);
            access["sub"] = json!("subject-b");
            response["access_token"] = json!(sign(&access));
        }
        fixture.state.lock().unwrap().token = response;
        let error = provider
            .complete_login(pending, "sensitive-code-canary", &query["state"])
            .await
            .unwrap_err();
        assert!(!format!("{error:?} {error}").contains("sensitive-code-canary"));
    }
}

// Duplicated security claims and an oversized bearer are rejected even with an authentic signature.
#[tokio::test]
async fn access_rejects_duplicate_security_claims_and_oversized_tokens() {
    let fixture = ProviderFixture::new().await;
    let provider = OidcProvider::new(fixture.config()).unwrap();
    provider.discover().await.unwrap();
    let claims = access_claims(&fixture.issuer).to_string().replacen(
        "\"sub\":\"subject-a\"",
        "\"sub\":\"subject-a\",\"sub\":\"subject-b\"",
        1,
    );
    let raw = serde_json::value::RawValue::from_string(claims).unwrap();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("key-1".into());
    let token = jsonwebtoken::encode(
        &header,
        &raw,
        &EncodingKey::from_rsa_der(&hex::decode(RSA_DER_HEX).unwrap()),
    )
    .unwrap();
    assert!(provider.verify_access(&token).await.is_err());
    assert!(provider.verify_access(&"a".repeat(16_385)).await.is_err());
    assert_eq!(fixture.hits("/keys"), 1);
}

// A refresh response is one external attempt, bound to the previous issuer/subject/public client.
#[tokio::test]
async fn refresh_revalidates_principal_and_does_not_repeat_ambiguous_or_invalid_exchange() {
    let fixture = ProviderFixture::new().await;
    let provider = OidcProvider::new(fixture.config()).unwrap();
    provider.discover().await.unwrap();
    let pending = provider
        .begin_login(
            OidcClientKind::Web,
            "http://127.0.0.1:18100/auth/oidc/callback",
        )
        .await
        .unwrap();
    let query = login_query(&pending);
    fixture.state.lock().unwrap().token = token_response(
        &fixture.issuer,
        &id_claims(&fixture.issuer, &query["nonce"]),
    );
    let initial = provider
        .complete_login(pending, "refresh-setup-code", &query["state"])
        .await
        .unwrap();
    let refreshed = provider
        .refresh(
            OidcClientKind::Web,
            "synthetic-refresh-canary",
            initial.access(),
        )
        .await
        .unwrap();
    assert_eq!(refreshed.access().subject(), "subject-a");
    assert_eq!(fixture.hits("/token"), 2);
    let mut wrong = access_claims(&fixture.issuer);
    wrong["sub"] = json!("subject-b");
    fixture.state.lock().unwrap().token["access_token"] = json!(sign(&wrong));
    assert!(
        provider
            .refresh(
                OidcClientKind::Web,
                "synthetic-refresh-canary",
                refreshed.access()
            )
            .await
            .is_err()
    );
    assert_eq!(fixture.hits("/token"), 3);
    assert!(
        provider
            .refresh(
                OidcClientKind::Cli,
                "synthetic-refresh-canary",
                refreshed.access()
            )
            .await
            .is_err()
    );
    assert_eq!(
        fixture.hits("/token"),
        3,
        "wrong client must fail before exchange"
    );
}

async fn initial_login(
    provider: &OidcProvider,
    fixture: &ProviderFixture,
) -> agentsandbox::oidc::VerifiedLogin {
    let pending = provider
        .begin_login(
            OidcClientKind::Web,
            "http://127.0.0.1:18100/auth/oidc/callback",
        )
        .await
        .unwrap();
    let query = login_query(&pending);
    fixture.state.lock().unwrap().token = token_response(
        &fixture.issuer,
        &id_claims(&fixture.issuer, &query["nonce"]),
    );
    provider
        .complete_login(pending, "initial-code", &query["state"])
        .await
        .unwrap()
}

// The request may have consumed a rotating refresh token even when its response times out.
#[tokio::test]
async fn refresh_timeout_is_one_attempt_and_sends_only_the_bound_public_grant() {
    let fixture = ProviderFixture::new().await;
    let mut config = fixture.config();
    config.limits.request_timeout_ms = 100;
    let provider = OidcProvider::new(config).unwrap();
    provider.discover().await.unwrap();
    let login = initial_login(&provider, &fixture).await;
    fixture.state.lock().unwrap().token_delay = Duration::from_millis(500);
    let start = std::time::Instant::now();
    assert!(matches!(
        provider
            .refresh(
                OidcClientKind::Web,
                "rotating-refresh-canary",
                login.access()
            )
            .await,
        Err(OidcError::TransportUnavailable)
    ));
    assert!(start.elapsed() < Duration::from_millis(400));
    let state = fixture.state.lock().unwrap();
    let attempts: Vec<_> = state
        .requests
        .iter()
        .filter(|(path, _)| path.ends_with("/token"))
        .collect();
    assert_eq!(
        attempts.len(),
        2,
        "one login and one ambiguous refresh; never retry"
    );
    let fields: std::collections::BTreeMap<_, _> =
        reqwest::Url::parse(&format!("https://fixture.invalid/?{}", attempts[1].1))
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
    assert_eq!(fields.len(), 3);
    assert_eq!(fields["grant_type"], "refresh_token");
    assert_eq!(fields["client_id"], "web-public");
    assert_eq!(fields["refresh_token"], "rotating-refresh-canary");
    assert_eq!(state.methods.last().map(String::as_str), Some("POST"));
}

// Refresh cannot manufacture an observed login, or change the original ID-token binding.
#[tokio::test]
async fn refresh_preserves_original_id_context_and_allows_an_omitted_nonce_only_on_refresh() {
    let fixture = ProviderFixture::new().await;
    let provider = OidcProvider::new(fixture.config()).unwrap();
    provider.discover().await.unwrap();
    let direct = provider
        .verify_access(&sign(&access_claims(&fixture.issuer)))
        .await
        .unwrap();
    assert!(matches!(
        provider
            .refresh(OidcClientKind::Web, "refresh", &direct)
            .await,
        Err(OidcError::InvalidLogin)
    ));
    assert_eq!(fixture.hits("/token"), 0);
    let login = initial_login(&provider, &fixture).await;
    let mut id = id_claims(&fixture.issuer, "changed-nonce");
    fixture.state.lock().unwrap().token = token_response(&fixture.issuer, &id);
    assert!(
        provider
            .refresh(OidcClientKind::Web, "refresh", login.access())
            .await
            .is_err()
    );
    id.as_object_mut().unwrap().remove("nonce");
    fixture.state.lock().unwrap().token = token_response(&fixture.issuer, &id);
    let refreshed = provider
        .refresh(OidcClientKind::Web, "refresh", login.access())
        .await
        .unwrap();
    assert_eq!(refreshed.access().subject(), "subject-a");
    id["auth_time"] = json!(now() - 30);
    fixture.state.lock().unwrap().token = token_response(&fixture.issuer, &id);
    assert!(
        provider
            .refresh(OidcClientKind::Web, "refresh", refreshed.access())
            .await
            .is_err()
    );
}

// Validated ID tokens stay in an opaque context; logout sends them only in a private POST body.
#[tokio::test]
async fn idp_logout_consumes_private_context_and_posts_to_the_exact_approved_endpoint() {
    let fixture = ProviderFixture::new().await;
    let mut config = fixture.config();
    config.end_session_endpoint = Some(format!("{}/logout", fixture.issuer));
    fixture.state.lock().unwrap().metadata["end_session_endpoint"] =
        json!(format!("{}/logout", fixture.issuer));
    let provider = OidcProvider::new(config).unwrap();
    provider.discover().await.unwrap();
    let login = initial_login(&provider, &fixture).await;
    let (proof, tokens, context) = login.into_session_parts();
    let context = context.expect("a login must yield a private logout context");
    let id = fixture.state.lock().unwrap().token["id_token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!format!("{context:?}").contains(&id));
    drop(proof);
    drop(tokens);
    provider.logout_idp(context).await.unwrap();
    let state = fixture.state.lock().unwrap();
    let attempts: Vec<_> = state
        .requests
        .iter()
        .filter(|(path, _)| path.ends_with("/logout"))
        .collect();
    assert_eq!(attempts.len(), 1);
    let fields: std::collections::BTreeMap<_, _> =
        reqwest::Url::parse(&format!("https://fixture.invalid/?{}", attempts[0].1))
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
    assert_eq!(fields.len(), 2);
    assert_eq!(fields["client_id"], "web-public");
    assert_eq!(fields["id_token_hint"], id);
    assert_eq!(state.methods.last().map(String::as_str), Some("POST"));
    assert!(state.requests.iter().all(|(path, _)| !path.contains(&id)));
}

// Explicit roots replace WebPKI, reject mixed/private material and preserve name verification.
#[tokio::test]
async fn private_ca_is_strict_and_used_for_real_tls_without_a_name_bypass() {
    let temp = tempfile::tempdir().unwrap();
    let pki = temp.path().join("pki");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_ai-guard-demo-pki"))
        .env_clear()
        .arg("--out")
        .arg(&pki)
        .args(["--dns", "fixture.invalid"])
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
    let ca = std::fs::read(pki.join("root-ca.pem")).unwrap();
    let identity = agentsandbox::tls::ServerIdentity::from_pem(
        &std::fs::read(pki.join("server-chain.pem")).unwrap(),
        &std::fs::read(pki.join("server.key")).unwrap(),
    )
    .unwrap();
    let fixture = ProviderFixture::with_tls(Some(identity)).await;
    assert!(
        OidcProvider::new(fixture.config())
            .unwrap()
            .discover()
            .await
            .is_err()
    );
    let provider = OidcProvider::new_with_ca(fixture.config(), &ca).unwrap();
    provider.discover().await.unwrap();
    assert_eq!(fixture.hits("openid-configuration"), 1);
    for invalid in [
        Vec::new(),
        b"not PEM".to_vec(),
        vec![b' '; 65_537],
        std::fs::read(pki.join("server.key")).unwrap(),
        [ca.clone(), b"garbage".to_vec()].concat(),
        ca.repeat(9),
    ] {
        assert!(matches!(
            OidcProvider::new_with_ca(fixture.config(), &invalid),
            Err(OidcError::InvalidConfiguration)
        ));
    }
    let mut wrong_name = fixture.config();
    for url in [
        &mut wrong_name.issuer,
        &mut wrong_name.authorization_endpoint,
        &mut wrong_name.token_endpoint,
        &mut wrong_name.jwks_uri,
    ] {
        *url = url.replace("127.0.0.1", "localhost");
    }
    assert!(matches!(
        OidcProvider::new_with_ca(wrong_name, &ca)
            .unwrap()
            .discover()
            .await,
        Err(OidcError::TransportUnavailable)
    ));
    assert_eq!(
        fixture.hits("openid-configuration"),
        1,
        "wrong SAN must be rejected before HTTP"
    );
}

// Discovery authentication advertisements do not turn our configured public client into a secret client.
#[tokio::test]
async fn public_code_exchange_does_not_require_none_in_discovery_authentication_advertisements() {
    for advertised in [Some(json!(["client_secret_basic"])), None] {
        let fixture = ProviderFixture::new().await;
        {
            let mut state = fixture.state.lock().unwrap();
            if let Some(advertised) = advertised {
                state.metadata["token_endpoint_auth_methods_supported"] = advertised;
            } else {
                state
                    .metadata
                    .as_object_mut()
                    .unwrap()
                    .remove("token_endpoint_auth_methods_supported");
            }
        }
        let provider = OidcProvider::new(fixture.config()).unwrap();
        provider.discover().await.unwrap();
        let login = initial_login(&provider, &fixture).await;
        assert_eq!(login.access().subject(), "subject-a");
        let state = fixture.state.lock().unwrap();
        let (index, (_, body)) = state
            .requests
            .iter()
            .enumerate()
            .find(|(_, (path, _))| path.ends_with("/token"))
            .unwrap();
        let fields: std::collections::BTreeMap<_, _> =
            reqwest::Url::parse(&format!("https://fixture.invalid/?{body}"))
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect();
        assert_eq!(fields.len(), 5);
        assert_eq!(fields["grant_type"], "authorization_code");
        assert_eq!(fields["client_id"], "web-public");
        assert!(fields["code_verifier"].len() >= 43);
        assert!(!fields.contains_key("client_secret"));
        assert!(!state.authorization_present[index]);
        assert_eq!(state.methods[index], "POST");
    }
}
