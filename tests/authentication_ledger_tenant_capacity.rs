//! NV-02: tenant A's valid rotations must leave admission capacity for tenant B.
//! Real router, controlled loopback issuer and separate tenant SQLite stores.
//! No real provider, native child, engine or shared service is exercised.
#[path = "support/oidc_fixture.rs"]
mod controlled;
mod support;
use agentsandbox as test_product;
use agentsandbox::{
    api::{
        GatewayState, OidcClaimsMappingV1, OidcGateway, OidcGatewayConfigV1, TenantGateway,
        gateway_router,
    },
    audit::{
        Store,
        authentication::{
            ApprovedAuthenticationClientV1, AuthenticationAudit, AuthenticationAuditPolicyV1,
            AuthenticationCapacityPlanV1, AuthenticationCapacityV1, AuthenticationSessionStateV1,
            AuthenticationTenantCapacityV1,
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
use std::{
    collections::{BTreeSet, HashMap},
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:18100";
const RECORD_LIMIT: u64 = 17;
const MAX_A_ROTATIONS: u32 = 7;

fn capacity_plan() -> AuthenticationCapacityPlanV1 {
    AuthenticationCapacityPlanV1 {
        version: 1,
        // G remains17: genesis + plan + A8 + B4 + unscoped3.
        unscoped: AuthenticationCapacityV1 {
            max_records: 3,
            max_serialized_bytes: 36_000,
        },
        tenants: [("tenant-a", 8, 56_000), ("tenant-b", 4, 36_000)]
            .map(
                |(tenant_id, max_records, max_serialized_bytes)| AuthenticationTenantCapacityV1 {
                    tenant_id: tenant_id.into(),
                    capacity: AuthenticationCapacityV1 {
                        max_records,
                        max_serialized_bytes,
                    },
                },
            )
            .to_vec(),
    }
}

struct Fixture {
    router: Router,
    audit: Arc<AuthenticationAudit>,
    stores: [Arc<Store>; 2],
    config: OidcGatewayConfigV1,
    installation: Uuid,
    policy: AuthenticationAuditPolicyV1,
    issuer: ControlledIssuer,
    // Keep scratch until the router, databases and issuer have been dropped.
    files: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Self {
        let files = support::private_tempdir();
        let issuer = ControlledIssuer::new().await;
        let stores =
            [("tenant-a", "member-a"), ("tenant-b", "member-b")].map(|(tenant, subject)| {
                let path = files.path().join(format!("{tenant}.sqlite"));
                let store =
                    Store::open_for_tenant(&path, tenant, Redactor::new(vec![]), 10_000).unwrap();
                store
                    .migrate_control_v3(&path.with_extension("before-v3"))
                    .unwrap();
                store
                    .migrate_identity_v4(&path.with_extension("before-v4"))
                    .unwrap();
                store
                    .migrate_human_access_v5(&path.with_extension("before-v5"))
                    .unwrap();
                let principal = PrincipalRefV1::HumanOidc {
                    issuer: issuer.issuer.clone(),
                    subject: subject.into(),
                };
                for change in [
                    IdentityChangeV1::Account {
                        expected_revision: 0,
                        account: HumanAccountV1 {
                            version: 1,
                            principal: principal.clone(),
                            display_name: format!("Synthetic {subject}"),
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
                            tenant_id: tenant.into(),
                            organization_scope: format!("organization-{tenant}"),
                            revision: 1,
                            active: true,
                            capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                        },
                    },
                ] {
                    store
                        .mutate_identity(&IdentityMutationV1 {
                            version: 1,
                            operation_id: Uuid::new_v4().to_string(),
                            actor: PrincipalRefV1::LegacyControl {
                                credential_id: "synthetic-ledger-operator".into(),
                                role: LegacyControlRoleV1::Runner,
                            },
                            change,
                        })
                        .unwrap();
                }
                Arc::new(store)
            });
        let policy = AuthenticationAuditPolicyV1 {
            max_records: RECORD_LIMIT,
            max_serialized_bytes: 131_072,
            approved_clients: ["web-public", "cli-public"]
                .map(|client_id| ApprovedAuthenticationClientV1 {
                    issuer: issuer.issuer.clone(),
                    client_id: client_id.into(),
                })
                .to_vec(),
        };
        let installation = Uuid::new_v4();
        let audit = Arc::new(
            AuthenticationAudit::init_new_v4(
                &files.path().join("authentication.sqlite"),
                installation,
                policy.clone(),
                capacity_plan(),
                Redactor::new(vec![]),
            )
            .unwrap(),
        );
        let config = OidcGatewayConfigV1 {
            version: 1,
            provider: issuer.config(),
            provider_label: "Synthetic NV02 issuer".into(),
            web_origin: ORIGIN.into(),
            mapping: OidcClaimsMappingV1 {
                revision: "synthetic-nv02-mapping".into(),
                default_capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                rules: vec![],
            },
        };
        let router = make_router(&stores, &audit, &config);
        Self {
            router,
            audit,
            stores,
            config,
            installation,
            policy,
            issuer,
            files,
        }
    }

    async fn cleanup(self) {
        let address = reqwest::Url::parse(&self.issuer.issuer).unwrap();
        let address = SocketAddr::new(
            address.host_str().unwrap().parse::<IpAddr>().unwrap(),
            address.port().unwrap(),
        );
        let Self {
            router,
            audit,
            stores,
            config,
            installation: _,
            policy: _,
            issuer,
            files,
        } = self;
        drop(router);
        drop(audit);
        drop(stores);
        drop(config);
        // The existing test issuer aborts its owned task on Drop. Observe that
        // its listener is released; do not call this a join of all HTTP tasks.
        drop(issuer);
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match tokio::net::TcpListener::bind(address).await {
                    Ok(listener) => {
                        drop(listener);
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                        tokio::task::yield_now().await;
                    }
                    Err(error) => {
                        panic!("NV02_SETUP: cannot observe issuer listener release: {error}")
                    }
                }
            }
        })
        .await
        .expect("NV02_SETUP: issuer listener was not released");
        drop(files);
    }
}

fn make_router(
    stores: &[Arc<Store>; 2],
    audit: &Arc<AuthenticationAudit>,
    config: &OidcGatewayConfigV1,
) -> Router {
    gateway_router(
        GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: stores[0].clone(),
                    runner_token: "r".repeat(48),
                    viewer_token: "v".repeat(48),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: stores[1].clone(),
                    runner_token: "s".repeat(48),
                    viewer_token: "w".repeat(48),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap()
        .with_oidc(OidcGateway::new(config.clone(), audit.clone(), None).unwrap())
        .unwrap(),
    )
}

fn post(
    path: &str,
    body: Value,
    bearer: Option<&str>,
    cookie: Option<&str>,
    web: bool,
) -> Request<Body> {
    let mut request = Request::post(path).header("content-type", "application/json");
    if let Some(bearer) = bearer {
        request = request.header("authorization", format!("Bearer {bearer}"));
    }
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    if web {
        request = request
            .header("origin", ORIGIN)
            .header("sec-fetch-site", "same-origin");
    }
    request.body(Body::from(body.to_string())).unwrap()
}

async fn response(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 262_144).await.unwrap();
    let value = serde_json::from_slice(&bytes).expect("NV02_SETUP: expected a JSON route response");
    (status, value)
}

async fn identity(router: &Router, token: &str) -> (StatusCode, Value) {
    response(
        router,
        Request::get("/api/v1/human/identity")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn native_admit(router: &Router, token: &str) -> (StatusCode, Value) {
    let (status, intent) = response(
        router,
        post(
            "/auth/oidc/native-intent",
            json!({"version":1,"action":{"kind":"login"}}),
            None,
            None,
            false,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "NV02_SETUP: native intent");
    response(
        router,
        post(
            "/auth/oidc/admit",
            json!({"version":1,"intent_id":intent["intent_id"]}),
            Some(token),
            None,
            false,
        ),
    )
    .await
}

async fn login_a(fixture: &Fixture) -> (Value, String) {
    let seeded = fixture
        .router
        .clone()
        .oneshot(
            Request::get("/auth/oidc/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        seeded.status(),
        StatusCode::OK,
        "NV02_SETUP: correlation cookie"
    );
    let cookie = seeded.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    drop(seeded);
    let (status, start) = response(
        &fixture.router,
        post(
            "/auth/oidc/start",
            json!({"version":1}),
            None,
            Some(&cookie),
            true,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "NV02_SETUP: A PKCE start");
    let url = reqwest::Url::parse(start["authorization_url"].as_str().unwrap()).unwrap();
    let params = url.query_pairs().collect::<HashMap<_, _>>();
    let nonce = params["nonce"].to_string();
    fixture
        .issuer
        .login_response(&nonce, "member-a", "nv02-a-login");
    let callback = fixture
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
    assert_eq!(
        callback.status(),
        StatusCode::SEE_OTHER,
        "NV02_SETUP: A callback"
    );
    drop(callback);
    let (status, delivery) = response(
        &fixture.router,
        post(
            "/auth/oidc/tokens",
            json!({"version":1}),
            None,
            Some(&cookie),
            true,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "NV02_SETUP: A token delivery");
    (delivery, nonce)
}

fn token_requests(issuer: &ControlledIssuer) -> usize {
    issuer
        .requests()
        .iter()
        .filter(|(path, _)| path.ends_with("/token"))
        .count()
}

#[tokio::test]
async fn tenant_a_rotations_preserve_first_admission_capacity_for_tenant_b() {
    let fixture = Fixture::new().await;
    let token_b = fixture
        .issuer
        .token("cli-public", "member-b", None, &[], "nv02-b-first");
    // Positive control: the same B credential, membership, two-tenant router
    // and immutable policy admit B when A has not spent this separate ledger.
    let control = Arc::new(
        AuthenticationAudit::init_new_v4(
            &fixture.files.path().join("control-authentication.sqlite"),
            Uuid::new_v4(),
            fixture.policy.clone(),
            capacity_plan(),
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
    let control_router = make_router(&fixture.stores, &control, &fixture.config);
    assert_eq!(
        native_admit(&control_router, &token_b).await.0,
        StatusCode::CREATED,
        "NV02_SETUP: B positive admission control"
    );
    let (status, control_identity) = identity(&control_router, &token_b).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "NV02_SETUP: B positive identity control"
    );
    assert_eq!(control_identity["contexts"].as_array().unwrap().len(), 1);
    assert_eq!(control_identity["contexts"][0]["tenant_id"], "tenant-b");
    assert_eq!(control.verify().unwrap().records, 4);
    drop(control_router);
    drop(control);

    assert_eq!(
        fixture.audit.verify().unwrap().records,
        2,
        "NV02_SETUP: B control must not spend the shared ledger"
    );
    let (mut delivery, nonce) = login_a(&fixture).await;
    let id_a = Uuid::parse_str(delivery["authentication_id"].as_str().unwrap()).unwrap();
    let (status, owner_a) =
        identity(&fixture.router, delivery["access_token"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK, "NV02_SETUP: real A identity");
    assert_eq!(owner_a["contexts"].as_array().unwrap().len(), 1);
    assert_eq!(owner_a["contexts"][0]["tenant_id"], "tenant-a");
    assert_eq!(fixture.audit.verify().unwrap().records, 4);
    assert_eq!(token_requests(&fixture.issuer), 1);
    let mut rotations = 0u32;
    let mut refused_before_exchange = false;
    for index in 0..MAX_A_ROTATIONS {
        fixture
            .issuer
            .login_response(&nonce, "member-a", &format!("nv02-a-rotation-{index}"));
        let exchanges = token_requests(&fixture.issuer);
        let (status, next) = response(
            &fixture.router,
            post(
                "/auth/oidc/refresh",
                json!({"version":1,"authentication_id":id_a,
                    "generation":delivery["generation"],"refresh_token":delivery["refresh_token"]}),
                Some(delivery["access_token"].as_str().unwrap()),
                None,
                true,
            ),
        )
        .await;
        if status == StatusCode::SERVICE_UNAVAILABLE {
            assert_eq!(
                next["error"]["code"], "audit_unavailable",
                "NV02_SETUP: only an explicit audit-budget refusal is admissible"
            );
            assert_eq!(
                token_requests(&fixture.issuer),
                exchanges,
                "NV02_SETUP: a budget refusal must precede another provider exchange"
            );
            refused_before_exchange = true;
            break;
        }
        assert_eq!(
            status,
            StatusCode::OK,
            "NV02_SETUP: A refresh must be valid"
        );
        rotations += 1;
        assert_eq!(token_requests(&fixture.issuer), exchanges + 1);
        assert_eq!(next["authentication_id"], delivery["authentication_id"]);
        assert_eq!(next["generation"].as_u64(), Some(u64::from(rotations + 1)));
        assert_eq!(
            fixture.audit.verify().unwrap().records,
            4 + 2 * u64::from(rotations)
        );
        delivery = next;
    }
    let before_b = fixture.audit.verify().unwrap();
    assert!(
        refused_before_exchange,
        "NV02_SETUP: A must exhaust its own finite part"
    );
    assert_eq!(
        rotations, 3,
        "NV02_SETUP: A owns eight records, including admission"
    );
    assert!(before_b.records <= RECORD_LIMIT);
    assert!(
        before_b.serialized_bytes <= 65_536,
        "NV02_SETUP: leave two maximum-size entries within the original byte policy"
    );
    let a_before_b = fixture.audit.session_current(id_a).unwrap().unwrap();
    assert_eq!(a_before_b.state, AuthenticationSessionStateV1::Active);
    assert_eq!(a_before_b.binding.generation, rotations + 1);
    // This is B's first request to the shared authentication ledger. No tenant
    // field in this request selects authority: B's verified membership does.
    let (b_status, b_response) = native_admit(&fixture.router, &token_b).await;
    assert!(
        b_status == StatusCode::CREATED || b_status == StatusCode::SERVICE_UNAVAILABLE,
        "NV02_SETUP: unexpected B admission status {b_status}"
    );
    let (b_identity_status, b_identity) = identity(&fixture.router, &token_b).await;
    let after_b = fixture.audit.verify().unwrap();
    assert_eq!(
        fixture.audit.session_current(id_a).unwrap().unwrap(),
        a_before_b,
        "NV02_SETUP: probing B must preserve A's durable generation"
    );
    if b_status == StatusCode::SERVICE_UNAVAILABLE {
        assert_eq!(
            b_response["error"]["code"], "audit_unavailable",
            "NV02_SETUP: expected global audit capacity refusal"
        );
        assert_eq!(
            after_b, before_b,
            "NV02_SETUP: refused B must not rewrite history or quota"
        );
        assert_eq!(
            b_identity_status,
            StatusCode::UNAUTHORIZED,
            "NV02_SETUP: refused B must have no admitted shared-ledger session"
        );
    } else if b_status == StatusCode::CREATED {
        assert_eq!(b_identity_status, StatusCode::OK);
        assert_eq!(b_identity["contexts"].as_array().unwrap().len(), 1);
        assert_eq!(b_identity["contexts"][0]["tenant_id"], "tenant-b");
        assert!(
            after_b.records <= RECORD_LIMIT,
            "global policy must remain enforced"
        );
    }
    let reopened = AuthenticationAudit::open_existing(
        &fixture.files.path().join("authentication.sqlite"),
        fixture.installation,
        fixture.policy.clone(),
        Redactor::new(vec![]),
    )
    .unwrap();
    assert_eq!(
        reopened.verify().unwrap(),
        after_b,
        "NV02_SETUP: the measured quota/head must survive reopening"
    );
    assert_eq!(reopened.session_current(id_a).unwrap().unwrap(), a_before_b);
    drop(reopened);
    // Complete the fixture's owned cleanup before the sole discriminating RED
    // assertion. Listener release is observed, not reported as a task join.
    fixture.cleanup().await;
    assert_eq!(
        b_status,
        StatusCode::CREATED,
        "NV02_TENANT_CAPACITY: A's valid rotations exhausted B's first durable admission"
    );
}
