//! The G08 administration reads on the owned source profile: real owned
//! Router/Store reads over legacy stores that received the human rights
//! marker before adoption.
use super::*;

fn grant(fixture: &Fixture, capabilities: &[HumanCapabilityV1]) {
    let store = fixture.legacy_store("B");
    let mut members = store.identity_memberships(fixture.principal()).unwrap();
    assert_eq!(members.len(), 1);
    let mut member = members.remove(0);
    let previous = member.revision;
    member.revision += 1;
    member.capabilities.extend(capabilities.iter().copied());
    store
        .mutate_identity(&change(IdentityChangeV1::Membership {
            expected_revision: previous,
            membership: member,
        }))
        .unwrap();
}

#[tokio::test]
async fn owned_sources_serve_the_summary_memberships_and_configuration() {
    use HumanCapabilityV1 as C;
    let fixture = Fixture::new_owned_with_human_rights();
    grant(
        &fixture,
        &[
            C::ReadMetadata,
            C::ReadTenantMetadata,
            C::ReadBrokerRegistry,
            C::ReadWorkerRegistry,
            C::ReadMemberships,
            C::ReadConfiguration,
        ],
    );
    let (bundle, credentials, tokens) = fixture.owned_startup();
    let state = GatewayState::for_owned_human_sources(
        bundle,
        credentials,
        tokens,
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let router = gateway_router(state);
    let (status, identity) = get(&router, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK, "{identity}");
    let context = identity["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|context| context["tenant_id"] == "B" && context["mode"] == "administration")
        .expect("an administration context on tenant B")
        .clone();
    let availability = |operation: &str| {
        context["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["operation"] == operation)
            .unwrap()["availability"]
            .clone()
    };
    for operation in [
        "summary_read",
        "memberships_read",
        "configuration_read",
        "brokers_read",
    ] {
        assert_eq!(availability(operation), "available", "{operation}");
    }
    for operation in [
        "workers_read",
        "federation_read",
        "quota_read",
        "usage_read",
    ] {
        assert_eq!(
            availability(operation),
            "profile_unavailable",
            "{operation}"
        );
    }
    let id = context["id"].as_str().unwrap();

    let (status, summary) = get(&router, "/api/v1/human/summary?window=all", Some(id)).await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["sections"]["resources"]["status"], "ready");
    assert_eq!(summary["sections"]["brokers"]["status"], "ready");
    assert_eq!(summary["sections"]["workers"]["status"], "unavailable");
    assert_eq!(
        summary["sections"]["workers"]["code"],
        "profile_unavailable"
    );
    assert_eq!(summary["sections"]["quota"]["code"], "profile_unavailable");
    assert!(summary["read_receipt"]["audit_event_id"].is_string());

    let (status, memberships) = get(&router, "/api/v1/human/admin/memberships", Some(id)).await;
    assert_eq!(status, StatusCode::OK, "{memberships}");
    assert!(
        memberships["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["own"] == true)
    );

    let (status, configuration) = get(&router, "/api/v1/human/admin/configuration", Some(id)).await;
    assert_eq!(status, StatusCode::OK, "{configuration}");
    let value = |key: &str| {
        configuration["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["key"] == key)
            .unwrap()["value"]
            .clone()
    };
    assert_eq!(value("source_profile"), "owned_sources");
    assert_eq!(value("human_rights_marker"), "installed");

    let (status, _) = get(&router, "/api/v1/human/admin/quota", Some(id)).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "no worker protocol on the owned profile yet"
    );
}

async fn post(router: &Router, path: &str, context: &str, body: &Value) -> (StatusCode, Value) {
    let request = axum::http::Request::post(path)
        .header("authorization", format!("Bearer {}", Fixture::bearer("B")))
        .header("x-aiguard-context", context)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 262_144)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn owned_sources_apply_a_delegated_membership_change_under_the_write_epoch() {
    use HumanCapabilityV1 as C;
    let fixture = Fixture::new_owned_with_human_rights();
    let PrincipalRefV1::HumanLocal {
        installation_id, ..
    } = fixture.principal()
    else {
        panic!("fixture must retain its local human identity");
    };
    let member = PrincipalRefV1::HumanLocal {
        installation_id: installation_id.clone(),
        account_id: uuid::Uuid::new_v4().to_string(),
    };
    let store = fixture.legacy_store("B");
    store
        .mutate_identity(&change(IdentityChangeV1::Account {
            expected_revision: 0,
            account: HumanAccountV1 {
                version: 1,
                principal: member.clone(),
                display_name: "Synthetic member".into(),
                revision: 1,
                active: true,
            },
        }))
        .unwrap();
    let organization = store.identity_memberships(fixture.principal()).unwrap()[0]
        .organization_scope
        .clone();
    let membership = MembershipV1 {
        version: 1,
        membership_id: uuid::Uuid::new_v4().to_string(),
        principal: member,
        tenant_id: "B".into(),
        organization_scope: organization,
        revision: 1,
        active: true,
        capabilities: [C::ReadMetadata].into_iter().collect(),
    };
    store
        .mutate_identity(&change(IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: membership.clone(),
        }))
        .unwrap();
    grant(
        &fixture,
        &[
            C::ReadMetadata,
            C::ReadTenantMetadata,
            C::ReadMemberships,
            C::ManageMemberships,
        ],
    );
    let (bundle, credentials, tokens) = fixture.owned_startup();
    let state = GatewayState::for_owned_human_sources(
        bundle,
        credentials,
        tokens,
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let owned = state.owned_human.as_ref().unwrap().clone();
    let tenant = state
        .tenants
        .iter()
        .position(|tenant| tenant.tenant_id == membership.tenant_id)
        .unwrap();
    let store = state.tenants[tenant].store.clone();
    let database = super::races::connection(&state);
    let expected_generation = || {
        let _epoch = owned.owner.try_read_epoch().unwrap();
        owned.discovery_generation(tenant).unwrap()
    };
    let count_mutations = || {
        database
            .query_row(
                "SELECT COUNT(*) FROM events WHERE kind='control.identity.mutation'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
    };
    let before = expected_generation();
    let mutations_before = count_mutations();
    let router = gateway_router(state);
    let (status, identity) = get(&router, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK, "{identity}");
    let context = identity["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|context| context["tenant_id"] == "B" && context["mode"] == "administration")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = format!(
        "/api/v1/human/admin/memberships/{}",
        membership.membership_id
    );
    let body = serde_json::json!({
        "operation_id": uuid::Uuid::new_v4().to_string(),
        "expected_revision": "1",
        "capabilities": ["read_metadata"],
        "active": false,
    });
    let (status, receipt) = post(&router, &path, &context, &body).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["membership_revision"], "2");
    let after = expected_generation();
    assert!(after != before);
    assert_eq!(count_mutations(), mutations_before + 1);
    let (status, stale) = get(&router, "/api/v1/human/admin/memberships", Some(&context)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{stale}");
    assert_eq!(stale["error"]["code"], "context_revoked");

    let (status, identity) = get(&router, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK, "{identity}");
    let context = identity["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|context| context["tenant_id"] == "B" && context["mode"] == "administration")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, replayed) = post(&router, &path, &context, &body).await;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(replayed, receipt);
    assert!(expected_generation() == after);
    assert_eq!(count_mutations(), mutations_before + 1);

    // A right the administrator does not hold is refused on this profile too.
    let escalation = serde_json::json!({
        "operation_id": uuid::Uuid::new_v4().to_string(),
        "expected_revision": "2",
        "capabilities": ["read_metadata", "read_content"],
        "active": true,
    });
    let (status, refusal) = post(&router, &path, &context, &escalation).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refusal}");
    // The owned memberships read shows the durable change.
    let (status, page) = get(&router, "/api/v1/human/admin/memberships", Some(&context)).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let item = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["membership_id"] == membership.membership_id.as_str())
        .unwrap()
        .clone();
    assert_eq!(item["active"], false);
    assert_eq!(item["revision"], "2");

    let operation_id = uuid::Uuid::new_v4().to_string();
    let fault = store.test_fail_after_membership_commit(operation_id.clone());
    let reactivate = serde_json::json!({
        "operation_id": operation_id,
        "expected_revision": "2",
        "capabilities": ["read_metadata"],
        "active": true,
    });
    let (status, failure) = post(&router, &path, &context, &reactivate).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{failure}");
    assert!(failure.get("audit_seq").is_none());
    assert!(
        fault.load(Ordering::Acquire),
        "the real SQLite commit was reached"
    );
    assert_eq!(count_mutations(), mutations_before + 2);
    assert!(expected_generation() == after);
    let (status, failure) = get(&router, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{failure}");
    assert_eq!(owned.coverage.load(Ordering::Acquire), REBUILD_REQUIRED);
}
