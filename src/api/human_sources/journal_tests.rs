//! Test-first behavior over real existing APIs. NOT_RUN until admitted QA.
use super::*;
use crate::{human_authority::migration::tests::Fixture, model::Event};
use tower::ServiceExt;

fn change(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "synthetic-journal-fixture-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    }
}

fn seed_event(fixture: &Fixture, tenant: &str, resource: &str, owner: &PrincipalRefV1) -> String {
    let store = fixture.legacy_store(tenant);
    let mut event = Event::new("fixture.owned-journal");
    event.session_id = Some(resource.into());
    event.data = json!({"private_note": "synthetic-private-journal-content"});
    let id = store.append(event).unwrap().event.id;
    store
        .mutate_identity(&change(IdentityChangeV1::ResourceGrant {
            expected_revision: 0,
            grant: ResourceGrantV1 {
                version: 1,
                tenant_id: tenant.into(),
                organization_scope: "synthetic-org".into(),
                resource: ResourceRefV1 {
                    kind: ResourceKindV1::Session,
                    resource_id: resource.into(),
                },
                owner: owner.clone(),
                revision: 1,
                active: true,
                shares: Vec::new(),
            },
        }))
        .unwrap();
    id
}

async fn get(router: &Router, path: &str, context: Option<&str>) -> (StatusCode, Value) {
    let mut request = axum::http::Request::get(path)
        .header("authorization", format!("Bearer {}", Fixture::bearer("B")));
    if let Some(context) = context {
        request = request.header("x-aiguard-context", context);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 262_144)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn owned_events_metadata_returns_authorized_items_and_committed_scope_receipt() {
    // All history/grants are prepared by existing real Store APIs before the
    // existing owned migration/startup protocol. No modern helper is needed.
    let fixture = Fixture::new_owned();
    let PrincipalRefV1::HumanLocal { installation_id, .. } = fixture.principal() else {
        panic!("fixture must retain its local human identity");
    };
    let other = PrincipalRefV1::HumanLocal {
        installation_id: installation_id.clone(),
        account_id: uuid::Uuid::new_v4().to_string(),
    };
    fixture
        .legacy_store("B")
        .mutate_identity(&change(IdentityChangeV1::Account {
            expected_revision: 0,
            account: HumanAccountV1 {
                version: 1,
                principal: other.clone(),
                display_name: "Synthetic other owner".into(),
                revision: 1,
                active: true,
            },
        }))
        .unwrap();
    let visible = seed_event(&fixture, "B", "owned-visible-b", fixture.principal());
    let hidden = seed_event(&fixture, "B", "owned-hidden-b", &other);
    let foreign = seed_event(&fixture, "A", "owned-foreign-a", fixture.principal());
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
    let neighbor = crate::audit::read_test_observer::observe(&state.tenants[0].store);
    let router = gateway_router(state.clone());
    let (status, identity) = get(&router, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK, "{identity}");
    let context = identity["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|context| context["tenant_id"] == "B" && context["mode"] == "personal")
        .unwrap();
    let (status, page) = get(
        &router,
        "/api/v1/human/events?view=metadata&limit=10",
        Some(context["id"].as_str().unwrap()),
    )
    .await;
    // Expected baseline oracle:503 versus200 here. Execution remains NOT_RUN.
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["view"], "metadata");
    assert_eq!(page["context_id"], context["id"]);
    assert_eq!(page["authorization_version"], context["authorization_version"]);
    assert_eq!(page["coverage"], "authorized_selection");
    assert_eq!(page["has_more"], false);
    assert!(page["next_cursor"].is_null());
    assert!(page.get("live_cursor").is_none());
    assert!(page.get("selection_complete").is_none());
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["metadata"]["id"], visible);
    assert!(items[0]["content"].is_null());
    let serialized = page.to_string();
    assert!(!serialized.contains(&hidden));
    assert!(!serialized.contains(&foreign));
    assert!(!serialized.contains("synthetic-private-journal-content"));
    assert!(neighbor.try_recv().is_err());

    // Independent SQLite observation proves actual commit and scope. A UUID
    // returned by the handler alone would not establish a durable read audit.
    let source = &state.owned_human.as_ref().unwrap().sources[1];
    let connection = rusqlite::Connection::open_with_flags(
        source.descriptor().storage_binding.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let receipt = &page["read_receipt"];
    let (payload, received): (String, String) = connection
        .query_row(
            "SELECT payload,received_at FROM events WHERE id=?1",
            [receipt["audit_event_id"].as_str().unwrap()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let audit: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(audit["kind"], "control.identity.access");
    let record = &audit["data"];
    assert_eq!(record["action"], "events");
    assert_eq!(record["result_count"], 1);
    assert_eq!(record["request_id"], receipt["request_id"]);
    assert_eq!(record["principal"], identity["principal"]);
    assert_eq!(record["mode"], context["mode"]);
    assert_eq!(received, receipt["received_at"].as_str().unwrap());
    let membership: String = connection
        .query_row(
            "SELECT payload_json FROM identity_memberships WHERE membership_id=?1",
            [record["membership_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    let membership: Value = serde_json::from_str(&membership).unwrap();
    assert_eq!(membership["tenant_id"], "B");
    assert_eq!(membership["principal"], identity["principal"]);
    assert_eq!(membership["organization_scope"], context["organization_scope"]);
    assert_eq!(record["membership_revision"], membership["revision"]);
    assert_eq!(
        record["membership_revision"].as_u64().unwrap().to_string(),
        context["membership_revision"].as_str().unwrap()
    );
    let authorization_revision: u64 = connection
        .query_row(
            "SELECT authorization_revision FROM identity_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(record["authorization_revision"], authorization_revision);
    let floor: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM audit_time_floors f JOIN events e ON f.event_seq>=e.seq WHERE e.id=?1)",
            [receipt["audit_event_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(floor, "real committed receipt must retain its durable wall floor");
}
