//! Foreground CLI over a real HTTPS gateway, authentication audit and tenant
//! SQLite. The independent owned issuer is a protocol fixture, not Keycloak QA.
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
    model::Event,
    policy::Policy,
    redaction::Redactor,
    tls::{ServerIdentity, serve},
};
use controlled::ControlledIssuer;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};
use uuid::Uuid;

async fn certificates(parent: &Path, label: &str) -> PathBuf {
    let path = parent.join(label);
    let mut child = Command::new(env!("CARGO_BIN_EXE_ai-guard-demo-pki"))
        .env_clear()
        .arg("--out")
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(120), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success(),
        "ephemeral TLS fixture generation failed"
    );
    path
}
fn server_identity(pki: &Path) -> ServerIdentity {
    ServerIdentity::from_files(&pki.join("server-chain.pem"), &pki.join("server.key")).unwrap()
}
fn membership(store: &Store, issuer: &str) {
    let principal = PrincipalRefV1::HumanOidc {
        issuer: issuer.into(),
        subject: "cli-tls-reader".into(),
    };
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: "tls-session".into(),
    };
    let mut event = Event::new("session.observed");
    event.session_id = Some(resource.resource_id.clone());
    store.append(event).unwrap();
    for change in [
        IdentityChangeV1::Account {
            expected_revision: 0,
            account: HumanAccountV1 {
                version: 1,
                principal: principal.clone(),
                display_name: "Named SSO TLS reader".into(),
                revision: 1,
                active: true,
            },
        },
        IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: MembershipV1 {
                version: 1,
                membership_id: Uuid::new_v4().to_string(),
                principal: principal.clone(),
                tenant_id: "tenant-sso-tls".into(),
                organization_scope: "organization-sso-tls".into(),
                revision: 1,
                active: true,
                capabilities: BTreeSet::from([
                    HumanCapabilityV1::ReadMetadata,
                    HumanCapabilityV1::Export,
                ]),
            },
        },
        IdentityChangeV1::ResourceGrant {
            expected_revision: 0,
            grant: ResourceGrantV1 {
                version: 1,
                tenant_id: "tenant-sso-tls".into(),
                organization_scope: "organization-sso-tls".into(),
                resource,
                owner: principal,
                revision: 1,
                active: true,
                shares: vec![],
            },
        },
    ] {
        store
            .mutate_identity(&IdentityMutationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                actor: PrincipalRefV1::LegacyControl {
                    credential_id: "owned-test-operator".into(),
                    role: LegacyControlRoleV1::Runner,
                },
                change,
            })
            .unwrap();
    }
}
async fn next(reader: &mut BufReader<ChildStdout>) -> Value {
    let mut line = String::new();
    let count = tokio::time::timeout(Duration::from_secs(8), reader.read_line(&mut line))
        .await
        .expect("foreground CLI output exceeded its deadline")
        .unwrap();
    assert!(
        count > 0,
        "foreground CLI closed before the expected status"
    );
    assert!(
        line.len() <= 524_288,
        "foreground output exceeded its bound"
    );
    serde_json::from_str(&line).expect("typed foreground JSON")
}
async fn command(input: &mut ChildStdin, line: &str) {
    input.write_all(line.as_bytes()).await.unwrap();
    input.write_all(b"\n").await.unwrap();
    input.flush().await.unwrap();
}
fn no_tokens(value: &Value, tokens: &[String]) {
    let rendered = value.to_string();
    for token in tokens {
        assert!(
            !rendered.contains(token),
            "foreground output exposed a token"
        );
    }
}
async fn bounded_exit(child: &mut Child) -> std::process::ExitStatus {
    tokio::time::timeout(Duration::from_secs(8), child.wait())
        .await
        .expect("foreground CLI failed to close within its deadline")
        .unwrap()
}

#[tokio::test]
async fn foreground_uses_one_gateway_trust_snapshot_and_independent_provider_ca() {
    let temp = support::private_tempdir();
    let gateway_pki = certificates(temp.path(), "gateway-pki").await;
    let provider_pki = certificates(temp.path(), "provider-pki").await;
    let issuer = ControlledIssuer::with_tls(server_identity(&provider_pki)).await;
    let provider_ca = std::fs::read_to_string(provider_pki.join("root-ca.pem")).unwrap();
    let data = temp.path().join("tenant.sqlite");
    let store = Store::create_supervised_for_tenant(
        &data,
        "tenant-sso-tls",
        Redactor::new(vec![]),
        10000,
        &temp.path().join("before-v3.sqlite"),
    )
    .unwrap();
    store
        .migrate_identity_v4(&temp.path().join("before-v4.sqlite"))
        .unwrap();
    store
        .migrate_human_access_v5(&temp.path().join("before-v5.sqlite"))
        .unwrap();
    membership(&store, &issuer.issuer);
    let audit = Arc::new(
        AuthenticationAudit::init_new(
            &temp.path().join("authentication.sqlite"),
            Uuid::new_v4(),
            AuthenticationAuditPolicyV1 {
                max_records: 10000,
                max_serialized_bytes: 4 * 1024 * 1024,
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
        provider_label: "Owned protocol issuer".into(),
        web_origin: "https://127.0.0.1:18100".into(),
        mapping: OidcClaimsMappingV1 {
            revision: "tls-mapping-1".into(),
            default_capabilities: BTreeSet::from([
                HumanCapabilityV1::ReadMetadata,
                HumanCapabilityV1::Export,
            ]),
            rules: vec![],
        },
    };
    let routes = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed = routes.clone();
    let router = gateway_router(
        GatewayState::new(
            Arc::new(store),
            "runner-sso-tls-fixture".into(),
            "viewer-sso-tls-fixture".into(),
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .with_oidc(OidcGateway::new(config, audit.clone(), Some(provider_ca.clone())).unwrap())
        .unwrap(),
    )
    .layer(axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let observed = observed.clone();
            async move {
                observed.lock().unwrap().push(request.uri().path().into());
                next.run(request).await
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("https://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(serve(
        listener,
        router,
        server_identity(&gateway_pki),
        std::future::pending(),
    ));
    let gateway_ca = gateway_pki.join("root-ca.pem");
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .env_clear()
        .args(["human", "sso", "--gateway", &origin, "--gateway-ca-file"])
        .arg(&gateway_ca)
        .args(["--browser", "manual"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let authorization = next(&mut reader).await;
    assert_eq!(authorization["status"], "browser_authorization_required");
    let url = reqwest::Url::parse(authorization["authorization_url"].as_str().unwrap()).unwrap();
    let params: BTreeMap<_, _> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(params["client_id"], "cli-public");
    assert_eq!(params["code_challenge_method"], "S256");
    let first = issuer.login_response_for_client(
        "cli-public",
        &params["nonce"],
        "cli-tls-reader",
        "tls-first",
        &[],
    );
    let mut tokens = vec![first, "private-refresh-tls-first".into()];

    // Replace the source with a different valid CA after the first control
    // request. Admission, business, export, refresh and logout must keep the
    // original gateway snapshot; provider TLS still uses its own profile CA.
    std::fs::write(&gateway_ca, provider_ca.as_bytes()).unwrap();
    let mut callback = reqwest::Url::parse(&params["redirect_uri"]).unwrap();
    callback
        .query_pairs_mut()
        .append_pair("state", &params["state"])
        .append_pair("code", "owned-protocol-code");
    assert_eq!(
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(callback)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    let ready = next(&mut reader).await;
    no_tokens(&ready, &tokens);
    assert_eq!(ready["status"], "session_ready");
    command(&mut input, "identity").await;
    let identity = next(&mut reader).await;
    no_tokens(&identity, &tokens);
    assert_eq!(identity["authentication"]["method"], "oidc_access_token");
    let context = identity["contexts"][0]["id"].as_str().unwrap();
    command(&mut input, &format!("context {context}")).await;
    assert_eq!(next(&mut reader).await["status"], "context_selected");
    let exported = temp.path().join("sso-export");
    command(&mut input, &format!("export --view metadata --resource-kind session --resource-id tls-session --output-dir {}", exported.display())).await;
    let receipt = next(&mut reader).await;
    no_tokens(&receipt, &tokens);
    assert!(
        exported.join("completion.json").exists(),
        "SSO export must publish its real authorized observation"
    );
    let rotated = issuer.login_response_for_client(
        "cli-public",
        &params["nonce"],
        "cli-tls-reader",
        "tls-next",
        &[],
    );
    tokens.extend([rotated, "private-refresh-tls-next".into()]);
    command(&mut input, "refresh").await;
    let refreshed = next(&mut reader).await;
    no_tokens(&refreshed, &tokens);
    assert_eq!(refreshed["status"], "session_refreshed");
    assert_eq!(refreshed["generation"], 2);
    command(&mut input, "identity").await;
    let refreshed_identity = next(&mut reader).await;
    no_tokens(&refreshed_identity, &tokens);
    assert_eq!(
        refreshed_identity["authentication"]["method"],
        "oidc_access_token"
    );
    command(&mut input, "logout").await;
    let closed = next(&mut reader).await;
    no_tokens(&closed, &tokens);
    assert_eq!(closed["local_session"], "closed");
    assert_eq!(closed["audit"], "recorded");
    assert!(bounded_exit(&mut child).await.success());
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .await
        .unwrap();
    for token in &tokens {
        assert!(
            !stderr.contains(token),
            "foreground error output exposed a token"
        );
    }
    assert!(
        stderr.is_empty(),
        "successful foreground session must not emit an error"
    );
    let routes = routes.lock().unwrap();
    for required in [
        "/auth/oidc/cli-profile",
        "/auth/oidc/admit",
        "/api/v1/human/identity",
        "/api/v1/human/export",
        "/auth/oidc/rotate",
        "/auth/oidc/logout",
    ] {
        assert!(
            routes.iter().any(|route| route == required),
            "expected gateway route was not exercised"
        );
    }
    assert!(audit.verify().is_ok());
    server.abort();
}
