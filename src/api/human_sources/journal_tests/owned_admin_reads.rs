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
