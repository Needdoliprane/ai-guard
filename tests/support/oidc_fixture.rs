//! Test-only controlled issuer with public synthetic RSA material from
//! jsonwebtoken's MIT-licensed tests. This is never an installation credential.
use super::test_product::oidc::{OidcConfigV1, OidcLimitsV1, OidcTransportModeV1};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::Request,
    response::Response,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
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
    response: Value,
    requests: Vec<(String, String)>,
    keys_barrier: Option<Arc<EndpointBarrier>>,
    token_barrier: Option<Arc<EndpointBarrier>>,
}
/// Test-only causal control of a real owned HTTP response. It is never linked
/// into the product library or exposed through the gateway's API.
#[derive(Default)]
pub struct EndpointBarrier {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl EndpointBarrier {
    pub async fn entered(&self) {
        self.entered.notified().await;
    }
    pub fn release(&self) {
        self.release.notify_one();
    }
}
pub struct ControlledIssuer {
    pub issuer: String,
    state: Arc<Mutex<Behavior>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for ControlledIssuer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl ControlledIssuer {
    pub async fn new() -> Self {
        Self::with_transport(None).await
    }
    pub async fn with_tls(identity: super::test_product::tls::ServerIdentity) -> Self {
        Self::with_transport(Some(identity)).await
    }
    async fn with_transport(identity: Option<super::test_product::tls::ServerIdentity>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let scheme = if identity.is_some() { "https" } else { "http" };
        let issuer = format!("{scheme}://{}/realm", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(Behavior::default()));
        let base = issuer.clone();
        let endpoint_state = state.clone();
        let app = Router::new().fallback(move |State(state): State<Arc<Mutex<Behavior>>>, request: Request<Body>| {
            let issuer = base.clone(); async move {
                let path = request.uri().path().to_owned();
                let body = to_bytes(request.into_body(), 35_000).await.unwrap();
                let (value,barrier)={let mut state = state.lock().unwrap();
                state.requests.push((path.clone(), String::from_utf8(body.to_vec()).unwrap()));
                let value = if path.ends_with("openid-configuration") { json!({
                    "issuer":issuer,"authorization_endpoint":format!("{issuer}/authorize"),
                    "token_endpoint":format!("{issuer}/token"),"jwks_uri":format!("{issuer}/keys"),
                    "response_types_supported":["code"],"subject_types_supported":["public"],
                    "id_token_signing_alg_values_supported":["RS256"],"token_endpoint_auth_methods_supported":["none"],
                    "code_challenge_methods_supported":["S256"]
                }) } else if path.ends_with("/keys") { json!({"keys":[{"kty":"RSA","kid":"controlled-1","use":"sig","alg":"RS256","n":RSA_N,"e":"AQAB"}]}) }
                else { state.response.clone() };
                let barrier=if path.ends_with("/keys") {state.keys_barrier.clone()} else if path.ends_with("/token") {state.token_barrier.clone()} else {None};
                (value,barrier)};
                if let Some(barrier)=&barrier {barrier.entered.notify_one();barrier.release.notified().await;}
                Response::builder().header("content-type", "application/json").body(Body::from(value.to_string())).unwrap()
            }
        }).with_state(endpoint_state);
        let task = tokio::spawn(async move {
            if let Some(identity) = identity {
                super::test_product::tls::serve(
                    listener,
                    app,
                    identity,
                    std::future::pending::<()>(),
                )
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
    pub fn config(&self) -> OidcConfigV1 {
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
            web_post_logout_redirect_uri: None,
            cli_post_logout_redirect_uri: None,
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
    pub fn token(
        &self,
        client: &str,
        subject: &str,
        nonce: Option<&str>,
        roles: &[&str],
        salt: &str,
    ) -> String {
        let now = chrono::Utc::now().timestamp();
        let mut claims = json!({"iss":self.issuer,"sub":subject,"aud":if nonce.is_some() {client} else {"guard-api"},
            "azp":client,"exp":now+120,"iat":now,"jti":salt,"roles":roles});
        if let Some(nonce) = nonce {
            claims["nonce"] = json!(nonce);
        } else {
            claims["typ"] = json!("Bearer");
        }
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("controlled-1".into());
        jsonwebtoken::encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_der(&hex::decode(RSA_DER_HEX).unwrap()),
        )
        .unwrap()
    }
    pub fn login_response(&self, nonce: &str, subject: &str, salt: &str) -> String {
        self.login_response_with_roles(nonce, subject, salt, &[])
    }
    pub fn login_response_with_roles(
        &self,
        nonce: &str,
        subject: &str,
        salt: &str,
        roles: &[&str],
    ) -> String {
        let access = self.token("web-public", subject, None, roles, salt);
        self.state.lock().unwrap().response = json!({"token_type":"Bearer","access_token":access,"refresh_token":format!("private-refresh-{salt}"),
            "id_token":self.token("web-public",subject,Some(nonce),roles,salt),"expires_in":120});
        access
    }
    pub fn requests(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().requests.clone()
    }
    pub fn block_keys(&self) -> Arc<EndpointBarrier> {
        let barrier = Arc::new(EndpointBarrier::default());
        self.state.lock().unwrap().keys_barrier = Some(barrier.clone());
        barrier
    }
    pub fn clear_keys_barrier(&self) {
        self.state.lock().unwrap().keys_barrier = None;
    }
    pub fn block_token(&self) -> Arc<EndpointBarrier> {
        let barrier = Arc::new(EndpointBarrier::default());
        self.state.lock().unwrap().token_barrier = Some(barrier.clone());
        barrier
    }
    pub fn token_with_tenant(&self, subject: &str, tenant: &str) -> String {
        let now = chrono::Utc::now().timestamp();
        let claims = json!({"iss":self.issuer,"sub":subject,"aud":"guard-api","azp":"cli-public","exp":now+120,"iat":now,"jti":format!("isolation-{subject}"),"roles":[],"typ":"Bearer","tenant_id":tenant});
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("controlled-1".into());
        jsonwebtoken::encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_der(&hex::decode(RSA_DER_HEX).unwrap()),
        )
        .unwrap()
    }
}
