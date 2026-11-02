//! Native ai-guard human SSO CLI regressions over owned loopback HTTP fixtures.
//! These are product protocol tests, not real-provider interoperability evidence.
mod support;
use axum::{Router, body::Body, extract::State, http::Request, response::Response};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
};

const RSA_N: &str = "yRE6rHuNR0QbHO3H3Kt2pOKGVhQqGZXInOduQNxXzuKlvQTLUTv4l4sggh5_CYYi_cvI-SXVT9kPWSKXxJXBXd_4LkvcPuUakBoAkfh-eiFVMh2VrUyWyj3MFl0HTVF9KwRXLAcwkREiS3npThHRyIxuy0ZMeZfxVL5arMhw1SRELB8HoGfG_AtH89BIE9jDBHZ9dLelK9a184zAf8LwoPLxvJb3Il5nncqPcSfKDDodMFBIMc4lQzDKL5gvmiXLXB1AGLm8KBjfE8s3L5xqi-yUod-j8MtvIj812dkS4QMiRVN_by2h3ZY8LYVGrqZXZTcgn2ujn8uKjXLZVD5TdQ";

// Public synthetic RSA fixture from jsonwebtoken 11.1.0's MIT-licensed RSA
// tests, shared in substance with oidc_transport. Not an installation secret.
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
const AUTH_ID: &str = "88888888-8888-4888-8888-888888888888";
const LOGIN_INTENT: &str = "99999999-9999-4999-8999-999999999999";
const REFRESH_INTENT: &str = "77777777-7777-4777-8777-777777777777";

#[derive(Default)]
struct Behavior {
    issuer: String,
    enabled: bool,
    malformed: bool,
    requests: Vec<String>,
    nonce: String,
    challenge: String,
    access: String,
    rotated_access: String,
    expires_at: String,
    generation: u32,
    refresh_delay: Duration,
    invalid_nonce: bool,
    logout_unavailable: bool,
    rotate_unavailable: bool,
    native_intent_unavailable: bool,
    login_intent: bool,
    refresh_intent: bool,
    intent_actions: Vec<String>,
    admit_gate: Option<Arc<tokio::sync::Notify>>,
    admit_accepted: bool,
    reflect_generations: bool,
    short_refresh: bool,
    malformed_admission: bool,
}
struct Fixture {
    origin: String,
    state: Arc<Mutex<Behavior>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new(enabled: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let issuer = format!("{origin}/realm");
        let now = chrono::Utc::now().timestamp();
        let access = sign(
            &json!({"iss":issuer,"sub":"subject-a","aud":"guard-api","azp":"cli-public",
            "typ":"Bearer","iat":now,"nbf":now-1,"exp":now+120,"jti":"initial"}),
        );
        let rotated_access = sign(
            &json!({"iss":issuer,"sub":"subject-a","aud":"guard-api","azp":"cli-public",
            "typ":"Bearer","iat":now,"nbf":now-1,"exp":now+120,"jti":"rotated"}),
        );
        let state = Arc::new(Mutex::new(Behavior {
            issuer,
            enabled,
            access,
            rotated_access,
            expires_at: chrono::DateTime::from_timestamp(now + 120, 0)
                .unwrap()
                .to_rfc3339(),
            generation: 1,
            ..Behavior::default()
        }));
        let router = Router::new().fallback(endpoint).with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            origin,
            state,
            task,
        }
    }
    fn spawn(&self) -> Child {
        Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
            .args([
                "human",
                "sso",
                "--gateway",
                &self.origin,
                "--browser",
                "manual",
            ])
            .env("BROWSER", "untrusted-browser-must-never-run")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    }
    fn hits(&self, path: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|p| p.as_str() == path)
            .count()
    }
}
async fn endpoint(State(state): State<Arc<Mutex<Behavior>>>, request: Request<Body>) -> Response {
    let path = request.uri().path().to_owned();
    let query = request.uri().query().unwrap_or("").to_owned();
    let authorization = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let method = request.method().clone();
    let bytes = axum::body::to_bytes(request.into_body(), 65_536)
        .await
        .unwrap();
    let (value, status, delay, gate) = {
        let mut state = state.lock().unwrap();
        state.requests.push(path.clone());
        let issuer = state.issuer.clone();
        if path.starts_with("/realm/") || path == "/auth/oidc/cli-profile" {
            assert!(authorization.is_none());
        }
        let mut status = 200;
        let mut delay = Duration::ZERO;
        let mut gate = None;
        let value = if path == "/auth/oidc/native-intent" {
            assert_eq!(method, reqwest::Method::POST);
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["version"], 1);
            let kind = value["action"]["kind"].as_str().unwrap();
            state.intent_actions.push(kind.to_owned());
            if state.native_intent_unavailable {
                status = 503;
                json!({"error":{"code":"audit_unavailable","message":"private-provider-canary","retry_action":"retry"}})
            } else if kind == "login" {
                assert!(authorization.is_none());
                assert!(!state.login_intent);
                assert_eq!(value, json!({"version":1,"action":{"kind":"login"}}));
                state.login_intent = true;
                json!({"version":1,"intent_id":LOGIN_INTENT,"expires_at":(chrono::Utc::now()+chrono::Duration::seconds(300)).to_rfc3339()})
            } else if kind == "refresh" {
                assert_eq!(
                    authorization.as_deref(),
                    Some(format!("Bearer {}", state.access).as_str())
                );
                assert_eq!(
                    value,
                    json!({"version":1,"action":{"kind":"refresh","authentication_id":AUTH_ID,"generation":1}})
                );
                assert!(!state.refresh_intent);
                state.refresh_intent = true;
                json!({"version":1,"intent_id":REFRESH_INTENT,"expires_at":(chrono::Utc::now()+chrono::Duration::seconds(15)).to_rfc3339()})
            } else {
                assert_eq!(kind, "finish");
                assert!(matches!(
                    value["action"]["outcome"].as_str(),
                    Some("cancelled" | "failed")
                ));
                let intent = value["action"]["intent_id"].as_str().unwrap();
                assert!(intent == LOGIN_INTENT || intent == REFRESH_INTENT);
                if intent == REFRESH_INTENT {
                    assert_eq!(
                        authorization.as_deref(),
                        Some(format!("Bearer {}", state.access).as_str())
                    );
                } else {
                    assert!(authorization.is_none());
                }
                json!({"version":1,"status":"finished"})
            }
        } else if path == "/auth/oidc/cli-profile" {
            if state.malformed {
                json!({"version":1,"enabled":true,"provider":{"client_secret":"private-provider-canary"}})
            } else if !state.enabled {
                json!({"version":1,"enabled":false})
            } else {
                json!({"version":1,"enabled":true,"provider":{
                    "version":1,"issuer":issuer,"web_client_id":"web-public","cli_client_id":"cli-public",
                    "api_audience":"guard-api","authorization_endpoint":format!("{issuer}/authorize"),
                    "token_endpoint":format!("{issuer}/token"),"jwks_uri":format!("{issuer}/keys"),
                    "web_redirect_uri":format!("{}/auth/oidc/callback",issuer.trim_end_matches("/realm")),
                    "cli_callback_path":"/oidc/callback","end_session_endpoint":null,
                    "transport":{"kind":"isolated_demo_http","exact_issuer":issuer},
                    "limits":{"request_timeout_ms":1000,"max_response_bytes":131072,"max_token_bytes":16384,
                        "max_jwks_keys":32,"max_inflight":8,"metadata_ttl_seconds":300,"jwks_ttl_seconds":300,
                        "unknown_kid_cooldown_seconds":30,"pkce_ttl_seconds":5,"clock_skew_seconds":5,
                        "max_token_lifetime_seconds":3600}
                }})
            }
        } else if path.ends_with("/.well-known/openid-configuration") && !state.login_intent {
            status = 503;
            json!({"error":{"code":"audit_unavailable","message":"native intent ACK required first","retry_action":"retry"}})
        } else if path.ends_with("/.well-known/openid-configuration") {
            json!({"issuer":issuer,"authorization_endpoint":format!("{issuer}/authorize"),
            "token_endpoint":format!("{issuer}/token"),"jwks_uri":format!("{issuer}/keys"),
            "response_types_supported":["code"],"subject_types_supported":["public"],
            "id_token_signing_alg_values_supported":["RS256"],"token_endpoint_auth_methods_supported":["none"],
            "code_challenge_methods_supported":["S256"]})
        } else if path.ends_with("/keys") {
            json!({"keys":[{"kty":"RSA","kid":"key-1","alg":"RS256","use":"sig",
            "key_ops":["verify"],"n":RSA_N,"e":"AQAB"}]})
        } else if path == "/realm/token" {
            assert_eq!(method, reqwest::Method::POST);
            assert!(
                state.login_intent,
                "native login intent must precede provider exchange"
            );
            let form: std::collections::BTreeMap<_, _> = reqwest::Url::parse(&format!(
                "http://127.0.0.1/?{}",
                std::str::from_utf8(&bytes).unwrap()
            ))
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
            assert_eq!(form["client_id"], "cli-public");
            assert!(!form.contains_key("client_secret"));
            if form["grant_type"] == "authorization_code" {
                assert_eq!(form["code"], "private-code-canary");
                assert_eq!(
                    URL_SAFE_NO_PAD.encode(Sha256::digest(form["code_verifier"].as_bytes())),
                    state.challenge
                );
                let nonce = if state.invalid_nonce {
                    "wrong-nonce"
                } else {
                    &state.nonce
                };
                let now = chrono::Utc::now().timestamp();
                let id = sign(
                    &json!({"iss":issuer,"sub":"subject-a","aud":"cli-public","azp":"cli-public",
                "iat":now,"exp":now+120,"nonce":nonce}),
                );
                json!({"access_token":state.access,"refresh_token":if state.short_refresh {"abc"}else{"oidc-refresh-one"},"id_token":id,"token_type":"Bearer","expires_in":120})
            } else {
                assert!(
                    state.refresh_intent,
                    "native refresh intent must precede provider exchange"
                );
                assert_eq!(form["grant_type"], "refresh_token");
                assert_eq!(form["refresh_token"], "oidc-refresh-one");
                delay = state.refresh_delay;
                json!({"access_token":state.rotated_access,"refresh_token":"oidc-refresh-two","token_type":"Bearer","expires_in":120})
            }
        } else if path == "/auth/oidc/admit" {
            assert_eq!(method, reqwest::Method::POST);
            assert_eq!(
                authorization.as_deref(),
                Some(format!("Bearer {}", state.access).as_str())
            );
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes).unwrap(),
                json!({"version":1,"intent_id":LOGIN_INTENT})
            );
            status = 201;
            state.admit_accepted = true;
            gate = state.admit_gate.clone();
            json!({"version":1,"authentication_id":if state.malformed_admission {"not-a-valid-authentication-id"}else{AUTH_ID},"generation":1,"expires_at":state.expires_at})
        } else if path == "/auth/oidc/rotate" {
            assert_eq!(method, reqwest::Method::POST);
            assert_eq!(
                authorization.as_deref(),
                Some(format!("Bearer {}", state.access).as_str())
            );
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                value,
                json!({"version":1,"authentication_id":AUTH_ID,"generation":1,"intent_id":REFRESH_INTENT,"access_token":state.rotated_access})
            );
            if state.rotate_unavailable {
                status = 503;
                json!({"error":{"code":"audit_unavailable","message":"private-provider-canary","retry_action":"retry"}})
            } else {
                state.generation = 2;
                json!({"version":1,"authentication_id":AUTH_ID,"generation":2,"expires_at":state.expires_at})
            }
        } else if path == "/auth/oidc/logout" {
            assert_eq!(method, reqwest::Method::POST);
            let expected = if state.generation == 1 {
                &state.access
            } else {
                &state.rotated_access
            };
            assert_eq!(
                authorization.as_deref(),
                Some(format!("Bearer {expected}").as_str())
            );
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes).unwrap(),
                json!({"version":1,"authentication_id":AUTH_ID,"generation":state.generation})
            );
            if state.logout_unavailable {
                status = 503;
                json!({"error":{"code":"audit_unavailable","message":"private-provider-canary","retry_action":"retry"}})
            } else {
                json!({"version":1,"local_session":"closed","provider_logout":"not_attempted","audit":"recorded"})
            }
        } else if path.starts_with("/api/v1/human/") {
            assert_eq!(method, reqwest::Method::GET);
            let expected = if state.generation == 1 {
                &state.access
            } else {
                &state.rotated_access
            };
            assert_eq!(
                authorization.as_deref(),
                Some(format!("Bearer {expected}").as_str())
            );
            let context = format!("context-{}", state.generation);
            let version = "a".repeat(64);
            if path.ends_with("/identity") {
                json!({"version":1,"principal":{"kind":"human_oidc","issuer":issuer,"subject":"subject-a"},
                "display_name":"Fixture human","authentication":{"method":"oidc_access_token","credential_id":AUTH_ID,"expires_at":state.expires_at},
                "contexts":[{"id":context,"tenant_id":"tenant-a","organization_scope":"org-a","mode":"personal",
                    "capabilities":["read_metadata","read_content","export"],"membership_revision":"1",
                    "authorization_version":version,"expires_at":state.expires_at}]})
            } else {
                let event = path.ends_with("/events") || path.ends_with("/export");
                let content = query.contains("view=content");
                let mut page = json!({"version":1,"context_id":context,"authorization_version":version,
                "items":[],"next_cursor":null,"has_more":false,"coverage":"authorized_selection",
                "read_receipt":{"request_id":"11111111-1111-4111-8111-111111111111","audit_event_id":"22222222-2222-4222-8222-222222222222",
                    "received_at":chrono::Utc::now().to_rfc3339()}});
                if event {
                    page["view"] = json!(if content { "content" } else { "metadata" });
                    page["selection_complete"] = json!(true);
                    page["items"] = json!([{"metadata":{"id":"33333333-3333-4333-8333-333333333333","timestamp":chrono::Utc::now().to_rfc3339(),
                    "received_at":chrono::Utc::now().to_rfc3339(),"kind":"fixture","source":"product-test","provenance":"observed",
                    "coverage":"fixture","resource":{"kind":"session","resource_id":"session-a"},"decision":null},
                    "content":if content {json!({"command":null,"decision":null,"result":null,"data":{"text":"visible fixture content"}})} else { Value::Null }}]);
                    if content && state.reflect_generations && state.generation == 2 {
                        // Deliberately ordinary content and keys, without
                        // sensitive field names or a Bearer prefix. Only the
                        // complete literal dictionary can cover both epochs.
                        let data = page["items"][0]["content"]["data"].as_object_mut().unwrap();
                        data.insert(
                            "paragraph".into(),
                            json!(format!(
                                "previous {} previous {} current {} current {}",
                                state.access,
                                "oidc-refresh-one",
                                state.rotated_access,
                                "oidc-refresh-two"
                            )),
                        );
                        for value in [
                            state.access.as_str(),
                            state.rotated_access.as_str(),
                            "oidc-refresh-one",
                            "oidc-refresh-two",
                        ] {
                            data.insert(
                                format!("ordinary source {value}"),
                                json!("ordinary key payload"),
                            );
                        }
                    }
                }
                page
            }
        } else {
            status = 404;
            json!({"unexpected":true})
        };
        (value, status, delay, gate)
    };
    if let Some(gate) = gate {
        gate.notified().await;
    }
    tokio::time::sleep(delay).await;
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("cache-control", "no-store")
        .body(Body::from(value.to_string()))
        .unwrap()
}
async fn finish(mut child: Child) -> (std::process::ExitStatus, String, String) {
    let mut stdout = String::new();
    let mut stderr = String::new();
    let out = child.stdout.take();
    let err = child.stderr.take();
    let collect = async {
        if let Some(mut out) = out {
            out.read_to_string(&mut stdout).await.unwrap();
        }
        if let Some(mut err) = err {
            err.read_to_string(&mut stderr).await.unwrap();
        }
        child.wait().await.unwrap()
    };
    let status = tokio::time::timeout(Duration::from_secs(8), collect)
        .await
        .expect("native CLI must terminate within its bound");
    (status, stdout, stderr)
}
async fn authorization(child: &mut Child) -> reqwest::Url {
    let out = child.stdout.take().unwrap();
    let mut reader = BufReader::new(out);
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert!(
        !line.is_empty(),
        "native CLI must offer its own browser transaction"
    );
    let value: Value = serde_json::from_str(&line).expect("typed authorization status");
    assert_eq!(value["status"], "browser_authorization_required");
    child.stdout = Some(reader.into_inner());
    reqwest::Url::parse(value["authorization_url"].as_str().unwrap()).unwrap()
}
fn callback(url: &reqwest::Url) -> reqwest::Url {
    let pairs: std::collections::BTreeMap<_, _> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(pairs["response_type"], "code");
    assert_eq!(pairs["client_id"], "cli-public");
    assert_eq!(pairs["code_challenge_method"], "S256");
    assert!(pairs["code_challenge"].len() >= 43);
    assert!(pairs["state"].len() >= 20 && pairs["nonce"].len() >= 20);
    assert!(!pairs.contains_key("code_verifier") && !pairs.contains_key("client_secret"));
    for forbidden in ["access_token", "id_token", "refresh_token", "code"] {
        assert!(!pairs.contains_key(forbidden));
    }
    let redirect = reqwest::Url::parse(&pairs["redirect_uri"]).unwrap();
    assert_eq!(redirect.scheme(), "http");
    assert_eq!(redirect.host_str(), Some("127.0.0.1"));
    assert_eq!(redirect.path(), "/oidc/callback");
    assert!(redirect.port().unwrap() > 0);
    redirect
}
fn no_secrets(stdout: &str, stderr: &str) {
    for canary in [
        "private-provider-canary",
        "private-code-canary",
        "private-error-description",
    ] {
        assert!(!stdout.contains(canary));
        assert!(!stderr.contains(canary));
    }
}

fn sign(claims: &Value) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("key-1".into());
    jsonwebtoken::encode(
        &header,
        claims,
        &EncodingKey::from_rsa_der(&hex::decode(RSA_DER_HEX).unwrap()),
    )
    .unwrap()
}
async fn next_json(child: &mut Child) -> Value {
    let out = child.stdout.as_mut().unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let byte = out
                .read_u8()
                .await
                .expect("native CLI ended before its typed response");
            if byte == b'\n' {
                break;
            }
            assert!(bytes.len() < 262_144);
            bytes.push(byte);
        }
    })
    .await
    .expect("typed response deadline");
    serde_json::from_slice(&bytes).expect("typed native CLI response")
}
async fn send(child: &mut Child, command: &str) {
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(command.as_bytes())
        .await
        .unwrap();
}
async fn code_callback(fixture: &Fixture, child: &mut Child) {
    let url = authorization(child).await;
    let mut redirect = callback(&url);
    let parameters: std::collections::BTreeMap<_, _> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    {
        let mut state = fixture.state.lock().unwrap();
        state.nonce = parameters["nonce"].clone();
        state.challenge = parameters["code_challenge"].clone();
    }
    redirect
        .query_pairs_mut()
        .append_pair("state", &parameters["state"])
        .append_pair("code", "private-code-canary");
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(redirect)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert!(
        !response
            .text()
            .await
            .unwrap()
            .contains("private-code-canary")
    );
}
async fn signed_in(fixture: &Fixture) -> Child {
    let mut child = fixture.spawn();
    code_callback(fixture, &mut child).await;
    let ready = next_json(&mut child).await;
    assert_eq!(ready["status"], "session_ready");
    assert_eq!(ready["generation"], 1);
    no_tokens(fixture, &ready.to_string());
    child
}
fn no_tokens(fixture: &Fixture, text: &str) {
    let state = fixture.state.lock().unwrap();
    for secret in [
        state.access.as_str(),
        state.rotated_access.as_str(),
        "oidc-refresh-one",
        "oidc-refresh-two",
    ] {
        assert!(
            !text.contains(secret),
            "native output or artifact contained a token"
        );
    }
}
fn assert_exported(fixture: &Fixture, value: &Value) {
    // Diagnostics are restricted to the local refusal reason, masked using
    // every fixture literal, then bounded. Never dump a whole response/body.
    let secrets = {
        let state = fixture.state.lock().unwrap();
        vec![
            state.access.clone(),
            state.rotated_access.clone(),
            "oidc-refresh-one".into(),
            "oidc-refresh-two".into(),
            "private-provider-canary".into(),
            "private-code-canary".into(),
            "private-error-description".into(),
        ]
    };
    let redactor = agentsandbox::redaction::Redactor::new(secrets);
    let reason = redactor.text(
        value
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("no refusal reason"),
    );
    let status = match value.get("status").and_then(Value::as_str) {
        Some("exported") => "exported",
        Some("command_refused") => "command_refused",
        _ => "unsupported status",
    };
    assert!(
        value["status"] == "exported",
        "expected exported; safe status={status}; safe bounded refusal={}",
        agentsandbox::redaction::clip(&reason, 768)
    );
}

#[tokio::test]
async fn sso_disabled_reports_operator_action_without_local_credential_fallback() {
    let fixture = Fixture::new(false).await;
    let child = fixture.spawn();
    let (status, out, err) = finish(child).await;
    assert!(!status.success());
    assert!(
        err.contains("OIDC is disabled"),
        "expected a specific disabled-SSO diagnostic"
    );
    assert_eq!(fixture.hits("/auth/oidc/cli-profile"), 1);
    assert_eq!(fixture.state.lock().unwrap().requests.len(), 1);
    no_secrets(&out, &err);
}
#[tokio::test]
async fn malformed_public_profile_is_refused_without_echoing_remote_fields() {
    let fixture = Fixture::new(true).await;
    fixture.state.lock().unwrap().malformed = true;
    let (status, out, err) = finish(fixture.spawn()).await;
    assert!(!status.success());
    assert!(err.contains("OIDC CLI profile is invalid"));
    assert_eq!(fixture.hits("/auth/oidc/cli-profile"), 1);
    no_secrets(&out, &err);
}
#[tokio::test]
async fn native_pkce_provider_cancellation_closes_callback_without_token_exchange() {
    let fixture = Fixture::new(true).await;
    let mut child = fixture.spawn();
    let url = authorization(&mut child).await;
    let mut redirect = callback(&url);
    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    redirect
        .query_pairs_mut()
        .append_pair("state", &state)
        .append_pair("error", "access_denied")
        .append_pair("error_description", "private-error-description");
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client.get(redirect.clone()).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = response.text().await.unwrap();
    assert!(!body.contains("private-error-description"));
    let (status, out, err) = finish(child).await;
    assert!(!status.success());
    assert!(err.contains("cancelled or refused"));
    assert_eq!(fixture.hits("/realm/token"), 0);
    redirect.set_query(None);
    assert!(
        client.get(redirect).send().await.is_err(),
        "callback listener must close"
    );
    no_secrets(&out, &err);
}
#[tokio::test]
async fn duplicate_callback_parameter_is_refused_before_exchange() {
    let fixture = Fixture::new(true).await;
    let mut child = fixture.spawn();
    let url = authorization(&mut child).await;
    let mut redirect = callback(&url);
    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    redirect
        .query_pairs_mut()
        .append_pair("state", &state)
        .append_pair("state", &state)
        .append_pair("code", "private-code-canary");
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(redirect)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let (status, out, err) = finish(child).await;
    assert!(!status.success());
    assert!(err.contains("OIDC callback is invalid"));
    assert_eq!(fixture.hits("/realm/token"), 0);
    no_secrets(&out, &err);
}
#[tokio::test]
async fn eof_cancels_pending_native_login_and_closes_its_listener() {
    let fixture = Fixture::new(true).await;
    let mut child = fixture.spawn();
    let url = authorization(&mut child).await;
    let redirect = callback(&url);
    drop(child.stdin.take());
    let (status, out, err) = finish(child).await;
    assert!(status.success());
    assert!(out.contains("session_closed"));
    assert_eq!(fixture.hits("/realm/token"), 0);
    assert!(
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(redirect)
            .send()
            .await
            .is_err()
    );
    no_secrets(&out, &err);
}

#[tokio::test]
async fn signed_native_session_reads_exports_resumes_and_rotates_without_token_files() {
    let fixture = Fixture::new(true).await;
    let files = support::private_tempdir();
    let parent = files.path().join("private parent with spaces");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&parent)
        .unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = parent.join("fresh-export");
    let mut child = signed_in(&fixture).await;
    send(&mut child, "identity\n").await;
    let identity = next_json(&mut child).await;
    assert_eq!(identity["authentication"]["method"], "oidc_access_token");
    assert_eq!(identity["authentication"]["credential_id"], AUTH_ID);
    assert_eq!(identity["contexts"][0]["tenant_id"], "tenant-a");
    send(&mut child, "context context-1\n").await;
    assert_eq!(next_json(&mut child).await["status"], "context_selected");
    send(&mut child, "resources --limit 1\n").await;
    let page = next_json(&mut child).await;
    assert_eq!(page["context_id"], "context-1");
    assert_eq!(page["items"], json!([]));
    assert!(page["read_receipt"]["audit_event_id"].is_string());
    send(
        &mut child,
        "events --view metadata --resource-kind session --resource-id session-a\n",
    )
    .await;
    let events = next_json(&mut child).await;
    assert_eq!(events["view"], "metadata");
    assert!(events["items"][0]["content"].is_null());
    send(&mut child,&format!("export --view content --resource-kind session --resource-id session-a --output-dir \"{}\"\n",output.display())).await;
    let exported = next_json(&mut child).await;
    assert_exported(&fixture, &exported);
    assert_eq!(exported["selection_complete"], true);
    let selection = std::fs::read(output.join("selection.json")).unwrap();
    let manifest = std::fs::read(output.join("completion.json")).unwrap();
    for file in ["selection.json", "page-0001.json", "completion.json"] {
        let bytes = std::fs::read(output.join(file)).unwrap();
        no_tokens(&fixture, std::str::from_utf8(&bytes).unwrap());
    }
    send(
        &mut child,
        &format!("resume-export --output-dir \"{}\"\n", output.display()),
    )
    .await;
    assert_exported(&fixture, &next_json(&mut child).await);
    send(&mut child, "refresh\n").await;
    let refreshed = next_json(&mut child).await;
    assert_eq!(refreshed["status"], "session_refreshed");
    assert_eq!(refreshed["generation"], 2);
    let reads = fixture.hits("/api/v1/human/resources");
    send(&mut child, "resources\n").await;
    assert_eq!(next_json(&mut child).await["status"], "command_refused");
    assert_eq!(
        fixture.hits("/api/v1/human/resources"),
        reads,
        "refresh must discard the old selected context"
    );
    send(&mut child, "context context-2\n").await;
    assert_eq!(next_json(&mut child).await["status"], "context_selected");
    send(
        &mut child,
        &format!("resume-export --output-dir \"{}\"\n", output.display()),
    )
    .await;
    assert_eq!(
        next_json(&mut child).await["status"],
        "command_refused",
        "new generation must not adopt old export authentication"
    );
    assert_eq!(
        std::fs::read(output.join("selection.json")).unwrap(),
        selection
    );
    assert_eq!(
        std::fs::read(output.join("completion.json")).unwrap(),
        manifest
    );
    send(&mut child, "logout\n").await;
    let (status, out, err) = finish(child).await;
    assert!(status.success());
    assert!(out.contains("session_closed"));
    assert_eq!(fixture.hits("/auth/oidc/logout"), 1);
    assert_eq!(fixture.hits("/auth/oidc/admit"), 1);
    assert_eq!(fixture.hits("/auth/oidc/rotate"), 1);
    assert_eq!(fixture.hits("/realm/token"), 2);
    no_tokens(&fixture, &out);
    no_tokens(&fixture, &err);
    no_tokens(&fixture, &identity.to_string());
    no_tokens(&fixture, &events.to_string());
    no_secrets(&out, &err);
    assert!(!output.with_extension("preparing").exists());
    assert_eq!(
        std::fs::read_dir(files.path()).unwrap().count(),
        1,
        "only the requested export is persisted"
    );
    assert_eq!(std::fs::read_dir(parent).unwrap().count(), 1);
}
#[tokio::test]
async fn invalid_signed_id_nonce_cannot_reach_control_admission() {
    let fixture = Fixture::new(true).await;
    fixture.state.lock().unwrap().invalid_nonce = true;
    let mut child = fixture.spawn();
    code_callback(&fixture, &mut child).await;
    let (status, out, err) = finish(child).await;
    assert!(!status.success());
    assert!(err.contains("OIDC login response is invalid"));
    assert_eq!(fixture.hits("/realm/token"), 1);
    assert_eq!(fixture.hits("/auth/oidc/admit"), 0);
    no_tokens(&fixture, &out);
    no_tokens(&fixture, &err);
    no_secrets(&out, &err);
}
#[tokio::test]
async fn eof_during_refresh_cancels_late_result_without_rotating_or_reopening() {
    let fixture = Fixture::new(true).await;
    fixture.state.lock().unwrap().refresh_delay = Duration::from_millis(600);
    let mut child = signed_in(&fixture).await;
    send(&mut child, "refresh\n").await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.hits("/realm/token") < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    drop(child.stdin.take());
    let (status, out, err) = finish(child).await;
    assert!(status.success());
    assert!(out.contains("session_closed"));
    assert!(!out.contains("session_refreshed"));
    assert_eq!(fixture.hits("/auth/oidc/rotate"), 0);
    assert_eq!(fixture.hits("/auth/oidc/logout"), 1);
    assert_eq!(
        fixture.hits("/realm/token"),
        2,
        "ambiguous provider refresh is never retried"
    );
    no_tokens(&fixture, &out);
    no_tokens(&fixture, &err);
    no_secrets(&out, &err);
}
#[tokio::test]
async fn failed_rotation_closes_session_without_repeating_provider_refresh() {
    let fixture = Fixture::new(true).await;
    fixture.state.lock().unwrap().rotate_unavailable = true;
    let mut child = signed_in(&fixture).await;
    send(&mut child, "refresh\n").await;
    let (status, out, err) = finish(child).await;
    assert!(!status.success());
    assert!(out.contains("session_closed"));
    assert!(err.contains("no automatic refresh retry"));
    assert_eq!(fixture.hits("/realm/token"), 2);
    assert_eq!(fixture.hits("/auth/oidc/rotate"), 1);
    assert_eq!(fixture.hits("/auth/oidc/logout"), 1);
    no_tokens(&fixture, &out);
    no_tokens(&fixture, &err);
    no_secrets(&out, &err);
}
#[tokio::test]
async fn logout_outage_still_closes_foreground_tokens_and_reports_unknown_control_state() {
    let fixture = Fixture::new(true).await;
    fixture.state.lock().unwrap().logout_unavailable = true;
    let mut child = signed_in(&fixture).await;
    send(&mut child, "logout\n").await;
    let (status, out, err) = finish(child).await;
    assert!(status.success());
    let value: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(value["status"], "session_closed");
    assert_eq!(value["local_session"], "unknown");
    assert_eq!(value["audit"], "unavailable");
    assert_eq!(fixture.hits("/auth/oidc/logout"), 1);
    no_tokens(&fixture, &out);
    no_tokens(&fixture, &err);
    no_secrets(&out, &err);
}

#[tokio::test]
async fn admitted_expiry_closes_idle_foreground_session_without_waiting_for_input() {
    let fixture = Fixture::new(true).await;
    fixture.state.lock().unwrap().expires_at =
        (chrono::Utc::now() + chrono::Duration::seconds(2)).to_rfc3339();
    let child = signed_in(&fixture).await;
    let (status, out, err) = finish(child).await;
    assert!(!status.success());
    assert!(err.contains("session expired"));
    assert!(out.contains("session_closed"));
    assert_eq!(fixture.hits("/auth/oidc/logout"), 1);
    no_tokens(&fixture, &out);
    no_tokens(&fixture, &err);
    no_secrets(&out, &err);
}

#[tokio::test]
async fn native_intent_audit_failure_blocks_discovery_browser_and_exchange() {
    let fixture = Fixture::new(true).await;
    fixture.state.lock().unwrap().native_intent_unavailable = true;
    let (status, out, err) = finish(fixture.spawn()).await;
    assert!(!status.success());
    assert!(err.contains("audit_unavailable"));
    assert_eq!(
        fixture.hits("/auth/oidc/native-intent"),
        1,
        "native intent must be durably acknowledged first"
    );
    assert_eq!(fixture.hits("/realm/.well-known/openid-configuration"), 0);
    assert_eq!(fixture.hits("/realm/token"), 0);
    assert!(!out.contains("browser_authorization_required"));
    no_tokens(&fixture, &out);
    no_tokens(&fixture, &err);
    no_secrets(&out, &err);
}
#[tokio::test]
async fn native_intent_ack_precedes_browser_and_each_provider_exchange() {
    let fixture = Fixture::new(true).await;
    let mut child = signed_in(&fixture).await;
    {
        let state = fixture.state.lock().unwrap();
        assert_eq!(state.intent_actions, vec!["login"]);
        let intent = state
            .requests
            .iter()
            .position(|p| p == "/auth/oidc/native-intent")
            .unwrap();
        let discovery = state
            .requests
            .iter()
            .position(|p| p == "/realm/.well-known/openid-configuration")
            .unwrap();
        assert!(intent < discovery);
    }
    send(&mut child, "refresh\n").await;
    assert_eq!(next_json(&mut child).await["status"], "session_refreshed");
    {
        let state = fixture.state.lock().unwrap();
        assert_eq!(state.intent_actions, vec!["login", "refresh"]);
        let intent = state
            .requests
            .iter()
            .rposition(|p| p == "/auth/oidc/native-intent")
            .unwrap();
        let token = state
            .requests
            .iter()
            .rposition(|p| p == "/realm/token")
            .unwrap();
        assert!(intent < token);
    }
    send(&mut child, "logout\n").await;
    let (status, out, err) = finish(child).await;
    assert!(status.success());
    no_tokens(&fixture, &out);
    no_tokens(&fixture, &err);
    no_secrets(&out, &err);
}
#[tokio::test]
async fn native_intent_refresh_audit_failure_prevents_provider_token_reuse() {
    let fixture = Fixture::new(true).await;
    let mut child = signed_in(&fixture).await;
    fixture.state.lock().unwrap().native_intent_unavailable = true;
    send(&mut child, "refresh\n").await;
    let (status, out, err) = finish(child).await;
    assert!(!status.success());
    assert!(out.contains("session_closed"));
    assert_eq!(
        fixture.hits("/realm/token"),
        1,
        "no refresh exchange without durable intent ACK"
    );
    assert_eq!(fixture.hits("/auth/oidc/rotate"), 0);
    assert_eq!(
        fixture.hits("/auth/oidc/native-intent"),
        2,
        "each intent is attempted once"
    );
    no_tokens(&fixture, &out);
    no_tokens(&fixture, &err);
    no_secrets(&out, &err);
}

async fn admission_cancellation(signal: bool) {
    let fixture = Fixture::new(true).await;
    let gate = Arc::new(tokio::sync::Notify::new());
    fixture.state.lock().unwrap().admit_gate = Some(gate.clone());
    let mut child = fixture.spawn();
    code_callback(&fixture, &mut child).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while !fixture.state.lock().unwrap().admit_accepted {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("signed provider login must reach the held accepted admission");
    assert_eq!(fixture.hits("/realm/token"), 1);
    assert_eq!(fixture.hits("/auth/oidc/admit"), 1);
    assert_eq!(fixture.hits("/api/v1/human/identity"), 0);
    assert!(child.try_wait().unwrap().is_none());
    let started = std::time::Instant::now();
    if signal {
        // This PID belongs to the child owned by this test; no signal is sent
        // to the user's process, process group or terminal.
        let pid = i32::try_from(child.id().unwrap()).unwrap();
        assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
    } else {
        drop(child.stdin.take());
    }
    // The response is still withheld when cancellation reaches the child.
    tokio::time::sleep(Duration::from_millis(100)).await;
    if signal {
        // A command queued after cancellation must never become a business
        // read when the outstanding admission eventually completes.
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(b"identity\n").await;
        }
    }
    gate.notify_one();
    tokio::time::sleep(Duration::from_millis(200)).await;
    // Keep a broken implementation finite without hiding any ready/read it
    // already emitted. This is cleanup, not the cancellation under test.
    drop(child.stdin.take());
    let (status, out, err) = finish(child).await;
    assert!(
        status.success(),
        "owned cancellation must close the foreground session"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "resolved loopback admission cancellation must be bounded"
    );
    assert!(
        !out.contains("session_ready"),
        "a cancelled admission must never announce a usable session"
    );
    assert_eq!(
        fixture.hits("/api/v1/human/identity"),
        0,
        "queued work must not run after admission cancellation"
    );
    assert_eq!(fixture.hits("/api/v1/human/resources"), 0);
    assert_eq!(
        fixture.hits("/auth/oidc/admit"),
        1,
        "original admission must not be retried"
    );
    assert_eq!(
        fixture.hits("/auth/oidc/logout"),
        1,
        "known accepted admission must be closed after the held response resolves"
    );
    assert!(out.contains("session_closed"));
    no_tokens(&fixture, &out);
    no_tokens(&fixture, &err);
    no_secrets(&out, &err);
}

#[tokio::test]
async fn review_regression_sigint_during_admission_never_announces_or_reads() {
    admission_cancellation(true).await;
}

#[tokio::test]
async fn review_regression_eof_during_admission_closes_the_resolved_session() {
    admission_cancellation(false).await;
}

async fn refreshed_reflection_session(fixture: &Fixture) -> Child {
    let mut child = signed_in(fixture).await;
    send(&mut child, "refresh\n").await;
    assert_eq!(next_json(&mut child).await["status"], "session_refreshed");
    send(&mut child, "context context-2\n").await;
    assert_eq!(next_json(&mut child).await["status"], "context_selected");
    fixture.state.lock().unwrap().reflect_generations = true;
    child
}

#[tokio::test]
async fn review_regression_refreshed_content_masks_all_known_generations() {
    let fixture = Fixture::new(true).await;
    let mut child = refreshed_reflection_session(&fixture).await;
    send(
        &mut child,
        "events --view content --resource-kind session --resource-id session-a\n",
    )
    .await;
    let events = next_json(&mut child).await;
    assert_eq!(events["view"], "content");
    assert_eq!(
        events["items"][0]["content"]["data"]["text"],
        "visible fixture content"
    );
    assert_eq!(fixture.hits("/api/v1/human/events"), 1);
    send(&mut child, "logout\n").await;
    let (status, out, err) = finish(child).await;
    assert!(status.success());
    // Scan the entire object so both ordinary string values and object keys
    // must mask the current and retired access/refresh literal dictionaries.
    no_tokens(&fixture, &events.to_string());
    no_tokens(&fixture, &out);
    no_tokens(&fixture, &err);
    no_secrets(&out, &err);
}

#[tokio::test]
async fn review_regression_refreshed_export_masks_all_known_generations() {
    let fixture = Fixture::new(true).await;
    let files = support::private_tempdir();
    let parent = files.path().join("private parent with spaces");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&parent)
        .unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = parent.join("rotated-export");
    let mut child = refreshed_reflection_session(&fixture).await;
    send(&mut child,&format!("export --view content --resource-kind session --resource-id session-a --output-dir \"{}\"\n",output.display())).await;
    let exported = next_json(&mut child).await;
    assert_exported(&fixture, &exported);
    assert_eq!(exported["selection_complete"], true);
    let page = std::fs::read_to_string(output.join("page-0001.json")).unwrap();
    let parsed: Value = serde_json::from_str(&page).unwrap();
    assert_eq!(
        parsed["page"]["items"][0]["content"]["data"]["text"],
        "visible fixture content"
    );
    let selection = std::fs::read_to_string(output.join("selection.json")).unwrap();
    let completion = std::fs::read_to_string(output.join("completion.json")).unwrap();
    assert_eq!(fixture.hits("/api/v1/human/export"), 1);
    send(&mut child, "logout\n").await;
    let (status, out, err) = finish(child).await;
    assert!(status.success());
    no_tokens(&fixture, &page);
    no_tokens(&fixture, &selection);
    no_tokens(&fixture, &completion);
    no_tokens(&fixture, &exported.to_string());
    no_tokens(&fixture, &out);
    no_tokens(&fixture, &err);
    no_secrets(&out, &err);
}

fn no_business_reads(fixture: &Fixture) {
    assert!(
        fixture
            .state
            .lock()
            .unwrap()
            .requests
            .iter()
            .all(|path| !path.starts_with("/api/v1/human/")),
        "an unusable admission must not read business data"
    );
}

#[tokio::test]
async fn review_preparation_short_refresh_rejects_before_grant_or_closes_known_admission() {
    let fixture = Fixture::new(true).await;
    fixture.state.lock().unwrap().short_refresh = true;
    let mut child = fixture.spawn();
    code_callback(&fixture, &mut child).await;
    let started = std::time::Instant::now();
    let (status, out, err) = finish(child).await;
    assert!(
        !status.success(),
        "an unmaskable refresh literal must be refused"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        fixture.hits("/realm/token"),
        1,
        "the signed provider bundle must reach local preparation"
    );
    assert!(!out.contains("session_ready"));
    no_business_reads(&fixture);
    let admitted = fixture.hits("/auth/oidc/admit");
    assert!(
        admitted <= 1,
        "local preparation failure must not retry admission"
    );
    assert_eq!(
        fixture.hits("/auth/oidc/logout"),
        admitted,
        "reject before grant or close exactly once the known accepted tuple"
    );
    no_tokens(&fixture, &out);
    no_tokens(&fixture, &err);
    no_secrets(&out, &err);
}

#[tokio::test]
async fn review_preparation_expired_accepted_tuple_is_closed_without_ready_or_reads() {
    let fixture = Fixture::new(true).await;
    let gate = Arc::new(tokio::sync::Notify::new());
    let expires = chrono::Utc::now() + chrono::Duration::seconds(3);
    {
        let mut state = fixture.state.lock().unwrap();
        state.expires_at = expires.to_rfc3339();
        state.admit_gate = Some(gate.clone());
    }
    let mut child = fixture.spawn();
    code_callback(&fixture, &mut child).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while !fixture.state.lock().unwrap().admit_accepted {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("signed login must reach the accepted response before it expires");
    assert!(
        chrono::Utc::now() < expires,
        "fixture must hold a still-valid accepted tuple"
    );
    assert_eq!(fixture.hits("/auth/oidc/admit"), 1);
    let delay = expires
        .signed_duration_since(chrono::Utc::now())
        .to_std()
        .unwrap()
        + Duration::from_millis(100);
    tokio::time::sleep(delay).await;
    assert!(chrono::Utc::now() >= expires);
    gate.notify_one();
    let started = std::time::Instant::now();
    let (status, out, err) = finish(child).await;
    assert!(
        !status.success(),
        "a locally expired accepted tuple must be refused"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!out.contains("session_ready"));
    no_business_reads(&fixture);
    assert_eq!(fixture.hits("/realm/token"), 1);
    assert_eq!(fixture.hits("/auth/oidc/admit"), 1);
    assert_eq!(
        fixture.hits("/auth/oidc/logout"),
        1,
        "the valid known tuple must be closed even after its business expiry"
    );
    no_tokens(&fixture, &out);
    no_tokens(&fixture, &err);
    no_secrets(&out, &err);
}

#[tokio::test]
async fn review_preparation_malformed_reference_never_becomes_logout_authority() {
    let fixture = Fixture::new(true).await;
    fixture.state.lock().unwrap().malformed_admission = true;
    let mut child = fixture.spawn();
    code_callback(&fixture, &mut child).await;
    let (status, out, err) = finish(child).await;
    assert!(!status.success());
    assert!(!out.contains("session_ready"));
    no_business_reads(&fixture);
    assert_eq!(fixture.hits("/auth/oidc/admit"), 1);
    assert_eq!(
        fixture.hits("/auth/oidc/logout"),
        0,
        "a malformed authentication reference is never trusted for cleanup"
    );
    no_tokens(&fixture, &out);
    no_tokens(&fixture, &err);
    no_secrets(&out, &err);
}
