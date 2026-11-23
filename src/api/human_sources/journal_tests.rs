//! Test-first behavior over real existing APIs. NOT_RUN until admitted QA.
use super::*;
use crate::{human_authority::migration::tests::Fixture, model::Event};
use tower::ServiceExt;

mod dashboard;
mod owned_admin_reads;
mod owned_brokers_live;
mod owned_workers;
mod races;
mod selection;

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

fn invalid_membership_uploads(valid: &Value) -> Vec<(&'static str, Vec<u8>)> {
    let object = serde_json::to_string(valid).unwrap();
    let positional = json!([
        valid["operation_id"],
        valid["expected_revision"],
        valid["capabilities"],
        valid["active"],
    ]);
    let duplicate = format!("{{\"active\":true,{}", &object[1..]);
    let mut unknown_capability = valid.clone();
    unknown_capability["capabilities"] = json!(["unrecognized"]);
    vec![
        ("application/json; charset", object.into_bytes()),
        ("application/json", serde_json::to_vec(&positional).unwrap()),
        ("application/json", duplicate.into_bytes()),
        (
            "application/json",
            serde_json::to_vec(&unknown_capability).unwrap(),
        ),
    ]
}

async fn require_invalid_membership_request(response: axum::response::Response) {
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let encoded = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let error: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(error["error"]["code"], "invalid_request");
    assert_eq!(error["error"]["retry_action"], "retry");
    assert_eq!(
        error["error"]["message"],
        "Check the membership change fields, then try again."
    );
    assert!(error.get("audit_seq").is_none());
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
async fn owned_event_page_reports_the_archived_prefix_after_adoption() {
    use crate::audit::retention::{AuditRetentionPolicyV1, ReclaimClassV1, RetentionApplyRequest};
    use std::{collections::BTreeSet, os::unix::fs::PermissionsExt};

    let fixture = Fixture::new_owned_local();
    let archive_home = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let archive_home_path = archive_home.path().canonicalize().unwrap();
    let store = fixture.legacy_store("B");
    assert_eq!(store.supervision_state().unwrap().schema_version, 13);
    store
        .install_audit_retention(&archive_home_path.join("before.sqlite"))
        .unwrap();

    let resource = "owned-archived-prefix";
    let first_id = seed_event(&fixture, "B", resource, fixture.principal());
    let append = |kind: &str| {
        let mut event = Event::new(kind);
        event.session_id = Some(resource.into());
        store.append(event).unwrap()
    };
    let reclaimed = append("fixture.journal.archived");
    let boundary = append("fixture.journal.boundary");
    let recent = append("fixture.journal.recent");
    let policy = AuditRetentionPolicyV1 {
        version: 1,
        min_age_seconds: 86_400,
        keep_recent_events: 1,
        max_segment_events: 100_000,
        classes: vec![ReclaimClassV1::Ordinary],
    };
    let now = chrono::Utc::now() + chrono::Duration::days(2);
    let kept = BTreeSet::new();
    let planned = store
        .audit_retention_plan(&policy, now, None, &kept)
        .unwrap();
    let receipt = store
        .apply_audit_retention(&RetentionApplyRequest {
            policy: &policy,
            now,
            through_seq: planned.plan.last_seq,
            expect_plan_sha256: &planned.plan_sha256,
            archive_dir: &archive_home_path.join("archive"),
            kept_positions: &kept,
        })
        .unwrap();
    assert_eq!(receipt.archive.last_seq, boundary.seq);
    assert!(receipt.reclaimed_events > 0);

    let (bundle, credentials, tokens) = fixture.owned_startup();
    assert!(bundle.owner.closure().sources.iter().any(|source| {
        source.tenant_id.as_deref() == Some("B") && source.required_format == 14
    }));
    let router = gateway_router(
        GatewayState::for_owned_human_sources(
            bundle,
            credentials,
            tokens,
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap(),
    );
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
        "/api/v1/human/events?view=metadata&limit=10&resource_kind=session&resource_id=owned-archived-prefix",
        context["id"].as_str(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["archived_through_seq"], receipt.archive.last_seq);
    let ids = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["metadata"]["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        [
            first_id.as_str(),
            boundary.event.id.as_str(),
            recent.event.id.as_str()
        ]
    );
    assert!(!ids.contains(&reclaimed.event.id.as_str()));
}

#[tokio::test]
async fn owned_events_metadata_returns_authorized_items_and_committed_scope_receipt() {
    // All history/grants are prepared by existing real Store APIs before the
    // existing owned migration/startup protocol. No modern helper is needed.
    let fixture = Fixture::new_owned();
    let PrincipalRefV1::HumanLocal {
        installation_id, ..
    } = fixture.principal()
    else {
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
    // A resource owner must also be an active member of the grant's
    // organization. This adds no capability, credential or share to the reader.
    fixture
        .legacy_store("B")
        .mutate_identity(&change(IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: MembershipV1 {
                version: 1,
                membership_id: uuid::Uuid::new_v4().to_string(),
                principal: other.clone(),
                tenant_id: "B".into(),
                organization_scope: "synthetic-org".into(),
                revision: 1,
                active: true,
                capabilities: std::collections::BTreeSet::new(),
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
    let neighbor =
        crate::audit::read_test_observer::observe(state.tenants[0].store.sqlite().unwrap());
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
    assert_eq!(
        page["authorization_version"],
        context["authorization_version"]
    );
    assert_eq!(page["coverage"], "authorized_selection");
    assert_eq!(page["has_more"], false);
    assert!(page["next_cursor"].is_null());
    assert!(page.get("archived_through_seq").is_none());
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

    let context_id = context["id"].as_str().unwrap();
    let detail_path = format!("/api/v1/human/events/{visible}?view=metadata&limit=10");
    let (status, detail) = get(&router, &detail_path, Some(context_id)).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    for field in [
        "view",
        "context_id",
        "authorization_version",
        "coverage",
        "items",
    ] {
        assert_eq!(detail[field], page[field], "{field}");
    }
    assert_eq!(detail["has_more"], false);
    assert!(detail["next_cursor"].is_null());
    let item: crate::identity::HumanEventItemV1 =
        serde_json::from_value(detail["items"][0].clone()).unwrap();
    assert_eq!(item.metadata.resource.resource_id, "owned-visible-b");
    assert!(item.content.is_none());
    assert!(
        !detail
            .to_string()
            .contains("synthetic-private-journal-content")
    );

    let (status, denied) = get(&router, &detail_path, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(denied["error"]["code"], "context_required");
    assert!(denied.get("items").is_none());
    let missing = uuid::Uuid::new_v4().to_string();
    for id in [&hidden, &foreign, &missing] {
        let (status, denied) = get(
            &router,
            &format!("/api/v1/human/events/{id}"),
            Some(context_id),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{denied}");
        assert_eq!(denied["error"]["code"], "not_found");
        assert!(denied.get("items").is_none());
        assert!(denied.get("read_receipt").is_none());
    }
    assert!(neighbor.try_recv().is_err());

    // Independent SQLite observation proves actual commit and scope. A UUID
    // returned by the handler alone would not establish a durable read audit.
    let source = &state.owned_human.as_ref().unwrap().sources[1];
    let connection = rusqlite::Connection::open_with_flags(
        source.descriptor().storage_binding.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    assert_committed_journal_receipt(
        &connection,
        &page,
        "events",
        &identity["principal"],
        context,
    );
    assert_committed_journal_receipt(
        &connection,
        &detail,
        "event",
        &identity["principal"],
        context,
    );
}

fn assert_committed_journal_receipt(
    connection: &rusqlite::Connection,
    response: &Value,
    action: &str,
    principal: &Value,
    context: &Value,
) {
    let receipt = &response["read_receipt"];
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
    assert_eq!(record["action"], action);
    assert_eq!(record["result_count"], 1);
    assert_eq!(record["request_id"], receipt["request_id"]);
    assert_eq!(record["principal"], *principal);
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
    assert_eq!(membership["tenant_id"], context["tenant_id"]);
    assert_eq!(membership["principal"], *principal);
    assert_eq!(
        membership["organization_scope"],
        context["organization_scope"]
    );
    assert_eq!(record["membership_revision"], membership["revision"]);
    assert_eq!(
        record["membership_revision"].as_u64().unwrap().to_string(),
        context["membership_revision"].as_str().unwrap()
    );
    let authorization_revision: i64 = connection
        .query_row(
            "SELECT authorization_revision FROM identity_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let authorization_revision = u64::try_from(authorization_revision).unwrap();
    assert_eq!(record["authorization_revision"], authorization_revision);
    let floor: bool = connection
    .query_row(
        "SELECT EXISTS(SELECT 1 FROM audit_time_floors f JOIN events e ON f.event_seq>=e.seq WHERE e.id=?1)",
        [receipt["audit_event_id"].as_str().unwrap()],
        |row| row.get(0),
    )
    .unwrap();
    assert!(
        floor,
        "real committed receipt must retain its durable wall floor"
    );
}
