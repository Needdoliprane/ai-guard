//! D5 admission oracles on the current API, not an IdP/load qualification.
//! Tokens are signed synthetic values from the existing controlled fixture.
//! Anonymous intents take the real Router producer; no Session is fabricated.
use super::*;
use axum::{body::Body, http::Request};
use tower::ServiceExt;

#[path = "admission_capacity_controls.rs"]
mod controls;
#[path = "admission_registry_tests.rs"]
mod registry_tests;

struct Fixture {
    _files: tempfile::TempDir,
    _issuer: controlled::ControlledIssuer,
    state: GatewayState,
    raw: String,
    binding: SessionBinding,
    installation_id: Uuid,
    audit_policy: AuthenticationAuditPolicyV1,
}

async fn fixture() -> Fixture {
    fixture_with_plan(1000, Some(capacity_plan(32, 483, 483))).await
}

/// Compatibility-only V3 fixture for the exact G6/G7 terminal oracles. Its
/// public gateway is freshly assembled with Store A and a new credential index.
async fn fixture_with_record_budget(max_records: u64) -> Fixture {
    fixture_with_plan(max_records, None).await
}

async fn fixture_with_plan(
    max_records: u64,
    plan: Option<AuthenticationCapacityPlanV1>,
) -> Fixture {
    let (files, state, _) = local_identity_state();
    let state = if plan.is_some() {
        state
    } else {
        let store = state.tenants[0].store.clone();
        drop(state);
        GatewayState::for_tenants(
            vec![crate::api::TenantGateway {
                tenant_id: "tenant-a".into(),
                store: store.sqlite().unwrap().clone(),
                runner_token: "synthetic-legacy-runner".repeat(3),
                viewer_token: "synthetic-legacy-viewer".repeat(3),
            }],
            crate::policy::Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap()
    };
    let issuer = controlled::ControlledIssuer::new().await;
    let principal = PrincipalRefV1::HumanOidc {
        issuer: issuer.issuer.clone(),
        subject: "synthetic-capacity-reader".into(),
    };
    for tenant in state.tenants.iter() {
        tenant
            .store
            .sqlite()
            .unwrap()
            .mutate_identity(&identity_change(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Synthetic capacity reader".into(),
                    revision: 1,
                    active: true,
                },
            }))
            .unwrap();
        tenant
            .store
            .sqlite()
            .unwrap()
            .mutate_identity(&identity_change(IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: MembershipV1 {
                    version: 1,
                    membership_id: Uuid::new_v4().to_string(),
                    principal: principal.clone(),
                    tenant_id: tenant.tenant_id.clone(),
                    organization_scope: "organization-ab".into(),
                    revision: 1,
                    active: true,
                    capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                },
            }))
            .unwrap();
    }
    let installation_id = Uuid::new_v4();
    let audit_policy = AuthenticationAuditPolicyV1 {
        max_records,
        max_serialized_bytes: 4 * 1024 * 1024,
        approved_clients: ["web-public", "cli-public"]
            .map(|client_id| ApprovedAuthenticationClientV1 {
                issuer: issuer.issuer.clone(),
                client_id: client_id.into(),
            })
            .to_vec(),
    };
    let path = files.path().join("authentication.sqlite");
    let audit = Arc::new(
        match plan {
            Some(plan) => AuthenticationAudit::init_new_v4(
                &path,
                installation_id,
                audit_policy.clone(),
                plan,
                Redactor::new(vec![]),
            ),
            None => AuthenticationAudit::init_new(
                &path,
                installation_id,
                audit_policy.clone(),
                Redactor::new(vec![]),
            ),
        }
        .unwrap(),
    );
    let state = state
        .with_oidc(
            OidcGateway::new(
                OidcGatewayConfigV1 {
                    version: 1,
                    provider: issuer.config(),
                    provider_label: "Controlled software fixture".into(),
                    web_origin: "http://127.0.0.1:18100".into(),
                    mapping: OidcClaimsMappingV1 {
                        revision: "capacity-red-1".into(),
                        default_capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
                        rules: vec![],
                    },
                },
                audit,
                None,
            )
            .unwrap(),
        )
        .unwrap();
    let raw = issuer.token(
        "cli-public",
        "synthetic-capacity-reader",
        None,
        &[],
        "capacity-red",
    );
    let oidc = state.oidc.as_ref().unwrap();
    let access = oidc.provider.verify_access(&raw).await.unwrap();
    assert_eq!(access.subject(), "synthetic-capacity-reader");
    let installed_raw = raw.clone();
    let binding = state
        .blocking_identity(move |state| {
            let oidc = configured(&state)?;
            let record = record(
                &oidc,
                Action::SessionAdmission,
                Uuid::new_v4(),
                "cli-public",
                None,
                None,
                Uuid::new_v4(),
            );
            install(
                &state,
                &oidc,
                NewAdmission {
                    record,
                    access,
                    raw: &installed_raw,
                    refresh: None,
                    logout_context: None,
                    kind: SessionKind::NativeCli,
                    browser_transaction: None,
                },
            )
        })
        .await
        .unwrap();
    assert_eq!(
        oidc.authenticate(&raw).unwrap().authentication_id,
        binding.session_id.to_string()
    );
    Fixture {
        _files: files,
        _issuer: issuer,
        state,
        raw,
        binding,
        installation_id,
        audit_policy,
    }
}

async fn request(state: &GatewayState, action: Value, bearer: Option<&str>) -> (StatusCode, Value) {
    let mut request =
        Request::post("/auth/oidc/native-intent").header("content-type", "application/json");
    if let Some(raw) = bearer {
        request = request.header("authorization", format!("Bearer {raw}"));
    }
    let response = gateway_router(state.clone())
        .oneshot(
            request
                .body(Body::from(json!({"version":1,"action":action}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 35_000)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn reset_start_window(oidc: &OidcGateway) {
    oidc.registry.lock().unwrap().rate = None;
}
fn start_count(oidc: &OidcGateway) -> u32 {
    oidc.registry
        .lock()
        .unwrap()
        .rate
        .as_ref()
        .map_or(0, StartWindow::total)
}
fn retime_full_anonymous_window(oidc: &OidcGateway) {
    let mut registry = oidc.registry.lock().unwrap();
    let window = registry
        .rate
        .as_mut()
        .expect("eight real anonymous starts required");
    assert_eq!(
        window.anonymous, 8,
        "setup: anonymous window must actually be full"
    );
    assert!(window.authenticated.is_empty());
    assert_eq!(window.total(), 8);
    // Only this actual window's started Instant changes, never a session or
    // operation deadline. A rollover before this check is a setup failure.
    window.started = Instant::now();
}

async fn refresh_and_finish(fixture: &Fixture) {
    fixture
        .state
        .oidc
        .as_ref()
        .unwrap()
        .authenticate(&fixture.raw)
        .unwrap_or_else(|_| panic!("setup: admitted session is no longer authenticated"));
    let (status, reply) = request(
        &fixture.state,
        json!({"kind":"refresh",
        "authentication_id":fixture.binding.session_id,"generation":fixture.binding.generation}),
        Some(&fixture.raw),
    )
    .await;
    if status != StatusCode::OK {
        assert_eq!(
            status,
            StatusCode::TOO_MANY_REQUESTS,
            "setup: useful refresh failed outside the admission-capacity status"
        );
        assert_eq!(
            reply["error"]["code"], "capacity_exhausted",
            "setup: useful refresh failed outside the admission-capacity oracle"
        );
    }
    assert_eq!(
        status,
        StatusCode::OK,
        "D5: anonymous admission pressure denied the real admitted native refresh; code={}",
        reply["error"]["code"]
    );
    let intent = Uuid::parse_str(reply["intent_id"].as_str().unwrap()).unwrap();
    let oidc = fixture.state.oidc.as_ref().unwrap();
    {
        let registry = oidc.registry.lock().unwrap();
        let pending = registry.native.get(&intent).unwrap();
        assert_eq!(pending.previous.as_ref(), Some(&fixture.binding));
        assert!(
            pending.intention.is_some(),
            "admission needs its real durable intention"
        );
        assert!(registry.sessions[&fixture.binding.session_id.to_string()].rotating);
    }
    let (status, _) = request(
        &fixture.state,
        json!({"kind":"finish","intent_id":intent,"outcome":"cancelled"}),
        Some(&fixture.raw),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "real pending transition must close through its normal audited path"
    );
    assert!(!oidc.registry.lock().unwrap().native.contains_key(&intent));
}

#[tokio::test]
async fn anonymous_intent_pool_cannot_deny_an_existing_native_session_refresh() {
    let fixture = fixture().await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    let mut admitted = 0;
    for _ in 0..256 {
        // Capacity oracle only: advance the existing private rate-window seam
        // without wall-clock sleeps. This does not measure 256 starts over TCP
        // or claim that admission/rate limits can be bypassed by a real caller.
        reset_start_window(oidc);
        let (status, value) = request(&fixture.state, json!({"kind":"login"}), None).await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(value["error"]["code"], "capacity_exhausted");
            break;
        }
        assert_eq!(
            status,
            StatusCode::OK,
            "setup: anonymous producer failed before capacity"
        );
        admitted += 1;
    }
    assert!(admitted > 0 && admitted <= 256);
    let refused = reserve_anonymous(&mut oidc.registry.lock().unwrap());
    assert!(
        matches!(refused, Err((StatusCode::TOO_MANY_REQUESTS, _))),
        "setup: anonymous pool did not close within256entries"
    );
    refresh_and_finish(&fixture).await;
}

#[tokio::test]
async fn anonymous_start_rate_cannot_spend_an_admitted_session_refresh_budget() {
    let fixture = fixture().await;
    let mut admitted = 0;
    for _ in 0..8 {
        let (status, value) = request(&fixture.state, json!({"kind":"login"}), None).await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(value["error"]["code"], "capacity_exhausted");
            break;
        }
        assert_eq!(
            status,
            StatusCode::OK,
            "setup: anonymous producer failed before rate limit"
        );
        admitted += 1;
    }
    // Retime only the real, fully consumed anonymous class of the shared
    // window. This is not a measurement of requests per physical second.
    assert_eq!(admitted, 8, "setup: eight real Login producers required");
    retime_full_anonymous_window(fixture.state.oidc.as_ref().unwrap());
    refresh_and_finish(&fixture).await;
}

#[tokio::test]
async fn known_session_uuid_with_a_false_token_cannot_spend_the_admission_window() {
    let fixture = fixture().await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    let before = start_count(oidc);
    let (status, _) = request(
        &fixture.state,
        json!({"kind":"refresh",
        "authentication_id":fixture.binding.session_id,"generation":fixture.binding.generation}),
        Some("synthetic-unverified-access-token"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        start_count(oidc),
        before,
        "D5: an unverified bearer spent an admission budget before authentication"
    );
    assert!(oidc.registry.lock().unwrap().native.is_empty());
}

#[tokio::test]
async fn saturated_membership_reads_commit_a_funded_unscoped_terminal_without_a_session() {
    let fixture = fixture().await;
    let oidc = fixture.state.oidc.as_ref().unwrap();
    assert_eq!(oidc.audit.format_version().unwrap(), 4);
    assert_eq!(fixture.state.tenants.len(), 2);
    let held: Vec<_> = fixture
        .state
        .tenants
        .iter()
        .flat_map(|tenant| {
            (0..4)
                .map(|_| tenant.io.admit(store_io::Lane::Read, false).unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(held.len(), 8);
    for tenant in fixture.state.tenants.iter() {
        assert_eq!(
            tenant
                .io
                .admit(store_io::Lane::Read, false)
                .err()
                .unwrap()
                .0,
            StatusCode::TOO_MANY_REQUESTS
        );
    }
    let raw = fixture._issuer.token(
        "cli-public",
        "synthetic-capacity-reader",
        None,
        &[],
        "all-membership-reads-saturated",
    );
    assert_ne!(
        raw, fixture.raw,
        "setup: a genuinely new signed proof is required"
    );
    let (status, intent) = request(&fixture.state, json!({"kind":"login"}), None).await;
    assert_eq!(status, StatusCode::OK);
    let before = oidc.audit.verify().unwrap();
    let sessions_before: BTreeSet<_> = oidc
        .registry
        .lock()
        .unwrap()
        .sessions
        .keys()
        .cloned()
        .collect();
    let response = gateway_router(fixture.state.clone())
        .oneshot(
            Request::post("/auth/oidc/admit")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {raw}"))
                .body(Body::from(
                    json!({"version":1,"intent_id":intent["intent_id"]}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(
        response.status(),
        StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
    ));
    let after = oidc.audit.verify().unwrap();
    assert_eq!(
        after.records,
        before.records + 2,
        "the verified preflight failure must consume its funded intention and terminal, without leaving a promise pending"
    );
    assert!(after.serialized_bytes > before.serialized_bytes);
    assert_eq!(
        oidc.registry
            .lock()
            .unwrap()
            .sessions
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        sessions_before
    );
    assert!(oidc.authenticate(&raw).is_err());
    assert_eq!(
        oidc.authenticate(&fixture.raw).unwrap().authentication_id,
        fixture.binding.session_id.to_string()
    );

    // Inspect only this private fixture's two actual persisted entries. An HTTP
    // refusal without its valid terminal cannot satisfy this regression.
    let db = rusqlite::Connection::open_with_flags(
        fixture._files.path().join("authentication.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let payload = |sequence: u64| -> Value {
        let text: String = db
            .query_row(
                "SELECT payload FROM authentication_entries WHERE seq=?1",
                [i64::try_from(sequence).unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        serde_json::from_str(&text).unwrap()
    };
    let intention = payload(before.records + 1);
    let terminal = payload(before.records + 2);
    assert_eq!(intention["kind"], "charged");
    assert_eq!(intention["charge"]["source"], "unscoped");
    assert_eq!(intention["entry"]["kind"], "verified_initial");
    let new_session =
        Uuid::parse_str(intention["entry"]["record"]["session_id"].as_str().unwrap()).unwrap();
    assert!(oidc.audit.session_current(new_session).unwrap().is_none());
    assert_eq!(terminal["charge"]["source"], "unscoped");
    assert_eq!(terminal["entry"]["kind"], "authentication");
    assert_eq!(
        terminal["entry"]["record"]["operation_id"],
        intention["entry"]["record"]["operation_id"]
    );
    assert_eq!(
        terminal["entry"]["record"]["phase"],
        json!({"kind":"result","outcome":"unknown","reason":"interrupted"})
    );
    assert!(terminal["entry"]["record"]["principal"].is_null());
    drop(db);
    let reopened = AuthenticationAudit::open_existing(
        &fixture._files.path().join("authentication.sqlite"),
        fixture.installation_id,
        fixture.audit_policy.clone(),
        Redactor::new(vec![]),
    )
    .unwrap();
    assert_eq!(reopened.verify().unwrap(), after);
    assert!(reopened.session_current(new_session).unwrap().is_none());
    assert_eq!(
        reopened
            .session_current(fixture.binding.session_id)
            .unwrap(),
        oidc.audit
            .session_current(fixture.binding.session_id)
            .unwrap()
    );
    drop(held);
    for tenant in fixture.state.tenants.iter() {
        assert!(tenant.io.admit(store_io::Lane::Read, false).is_ok());
    }
    assert_eq!(oidc.audit.verify().unwrap(), after);
}
