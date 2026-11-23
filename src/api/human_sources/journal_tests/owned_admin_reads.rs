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
        "workers_read",
        "quota_read",
    ] {
        assert_eq!(availability(operation), "available", "{operation}");
    }
    for operation in ["federation_read", "usage_read"] {
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
    assert_eq!(
        summary["sections"]["workers"],
        json!({
            "status": "ready",
            "coverage": "complete_registry",
            "data": {
                "by_state": {},
                "quota_held": "0",
                "groups": [],
                "groups_complete": true,
                "runtime_inventory": "unknown",
            },
        })
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

    let (status, _) = get(&router, "/api/v1/human/admin/usage", Some(id)).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "usage stays unavailable on the owned profile"
    );
}

async fn owned_worker_summary_on(format: u32) {
    use super::owned_workers::{context, provision_reader, read};
    use crate::api::human_sources::owned_control_tests::negative::reserve_workload_fixture_worker;
    use std::os::unix::fs::PermissionsExt;

    let historical = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let mut files = Store::owned_control_fixture_with_legacy_setup_for_test(format, |legacy| {
        reserve_workload_fixture_worker(legacy);
        legacy
            .migrate_human_rights_v2(&historical.path().join("before-rights.sqlite"))
            .unwrap();
    });
    let (bundle, credentials, tokens) = files.take_startup();
    let reader = provision_reader(&bundle, HumanCapabilityV1::ReadWorkerRegistry);
    let broker_admin = provision_reader(&bundle, HumanCapabilityV1::ReadBrokerRegistry);
    let descriptor = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|descriptor| descriptor.tenant_id.as_deref() == Some("A"))
        .unwrap();
    assert_eq!(descriptor.required_format, format);
    let source = descriptor.storage_binding.path();
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
    const IDENTITY: &str = "/api/v1/human/identity";
    const SUMMARY: &str = "/api/v1/human/summary?window=all";
    let (status, identity) = read(&router, &reader, IDENTITY, None).await;
    assert_eq!(status, StatusCode::OK);
    let admin = context(&identity, "administration");
    let (status, summary) = read(&router, &reader, SUMMARY, Some(admin)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(summary["context_id"], admin["id"]);
    assert_eq!(
        summary["authorization_version"],
        admin["authorization_version"]
    );
    assert_eq!(summary["consistency"], "transaction_snapshot");
    let sections = &summary["sections"];
    assert_eq!(
        sections["workers"],
        json!({
            "status": "ready",
            "coverage": "complete_registry",
            "data": {
                "by_state": {"reserved": "1"},
                "quota_held": "1",
                "groups": [{
                    "association": "unregistered",
                    "runtime_kind": "docker",
                    "broker": null,
                    "by_state": {"reserved": "1"},
                    "quota_held": "1",
                }],
                "groups_complete": true,
                "runtime_inventory": "unknown",
            },
        })
    );
    for section in ["quota", "usage", "attention"] {
        assert_eq!(
            sections[section],
            json!({"status":"unavailable", "code":"profile_unavailable"}),
            "{section}"
        );
    }
    assert_eq!(sections["brokers"], json!({"status":"not_granted"}));
    assert_eq!(sections["resources"]["status"], "ready");
    assert_eq!(sections["decisions"]["status"], "ready");

    let database = rusqlite::Connection::open_with_flags(
        source,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let receipt = &summary["read_receipt"];
    let (payload, received_at): (String, String) = database
        .query_row(
            "SELECT payload,received_at FROM events WHERE kind='control.identity.access' AND id=?1",
            [receipt["audit_event_id"].as_str().unwrap()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let audited: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(audited["data"]["action"], "summary");
    assert_eq!(audited["data"]["mode"], "administration");
    assert_eq!(audited["data"]["principal"], identity["principal"]);
    assert_eq!(audited["data"]["request_id"], receipt["request_id"]);
    assert_eq!(audited["data"]["result_count"], 3);
    assert_eq!(received_at, receipt["received_at"].as_str().unwrap());

    let (status, broker_identity) = read(&router, &broker_admin, IDENTITY, None).await;
    assert_eq!(status, StatusCode::OK);
    for (bearer, selected) in [
        (&reader, context(&identity, "personal")),
        (&broker_admin, context(&broker_identity, "administration")),
    ] {
        let (status, summary) = read(&router, bearer, SUMMARY, Some(selected)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            summary["sections"]["workers"],
            json!({"status":"not_granted"})
        );
    }
}

#[tokio::test]
async fn owned_worker_summary_format14_counts_only_durable_worker_facts() {
    owned_worker_summary_on(14).await;
}

#[tokio::test]
async fn owned_worker_summary_format15_counts_only_durable_worker_facts() {
    owned_worker_summary_on(15).await;
}

async fn owned_quota_on(format: u32) {
    use super::owned_workers::{context, provision_reader, read};
    use crate::{
        api::human_sources::owned_control_tests::negative::reserve_workload_fixture_worker,
        control::QuotaPolicyV1,
    };
    use rusqlite::OpenFlags;
    use std::os::unix::fs::PermissionsExt;

    const IDENTITY: &str = "/api/v1/human/identity";
    const QUOTA: &str = "/api/v1/human/admin/quota";
    const USAGE: &str = "/api/v1/human/admin/usage";
    let historical = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let mut files = Store::owned_control_fixture_with_legacy_setup_for_test(format, |legacy| {
        reserve_workload_fixture_worker(legacy);
        legacy
            .migrate_human_rights_v2(&historical.path().join("before-rights.sqlite"))
            .unwrap();
    });
    let (bundle, credentials, tokens) = files.take_startup();
    let reader = provision_reader(&bundle, HumanCapabilityV1::ReadWorkerRegistry);
    let broker_admin = provision_reader(&bundle, HumanCapabilityV1::ReadBrokerRegistry);
    let descriptor = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|descriptor| descriptor.tenant_id.as_deref() == Some("A"))
        .unwrap();
    assert_eq!(descriptor.required_format, format);
    let source = descriptor.storage_binding.path();
    let policy = QuotaPolicyV1 {
        version: 1,
        max_active_workers: 5,
        max_cpu_millis: 7000,
        max_memory_bytes: 4 * 1024 * 1024 * 1024,
        max_timeout_seconds: 7200,
    };
    let state = GatewayState::for_owned_human_sources(
        bundle,
        credentials,
        tokens,
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap()
    .with_worker_quotas(vec![("A".into(), policy)])
    .unwrap();
    let router = gateway_router(state);
    let (status, identity) = read(&router, &reader, IDENTITY, None).await;
    assert_eq!(status, StatusCode::OK);
    let admin = context(&identity, "administration");
    let operations = admin["operations"].as_array().unwrap();
    let operation = |name: &str| {
        operations
            .iter()
            .find(|entry| entry["operation"] == name)
            .unwrap()
    };
    assert_eq!(operation("quota_read")["permission"], "granted");
    assert_eq!(operation("quota_read")["availability"], "available");
    assert_eq!(
        operation("usage_read")["availability"],
        "profile_unavailable"
    );

    let (status, body) = read(&router, &reader, QUOTA, Some(admin)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["context_id"], admin["id"]);
    assert_eq!(
        body["authorization_version"],
        admin["authorization_version"]
    );
    let quota: HumanQuotaV1 = serde_json::from_value(body).unwrap();
    assert_eq!(quota.version, 1);
    assert_eq!(quota.source, "tenant_worker_reservations");
    assert_eq!(quota.max_timeout_seconds, "7200");
    assert_eq!(quota.quota.max_active_workers, "5");
    assert_eq!(quota.quota.max_cpu_millis, "7000");
    assert_eq!(quota.quota.max_memory_bytes, "4294967296");
    assert_eq!(quota.quota.held_workers, "1");
    assert_eq!(quota.quota.held_cpu_millis, "2000");
    assert_eq!(quota.quota.held_memory_bytes, "1073741824");

    let database = rusqlite::Connection::open_with_flags(
        &source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let receipt = &quota.read_receipt;
    let (payload, received_at): (String, String) = database
        .query_row(
            "SELECT payload,received_at FROM events WHERE kind='control.identity.access' AND id=?1",
            [&receipt.audit_event_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let audited: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(audited["data"]["action"], "quota");
    assert_eq!(audited["data"]["mode"], "administration");
    assert_eq!(audited["data"]["principal"], identity["principal"]);
    assert_eq!(audited["data"]["request_id"], receipt.request_id);
    assert_eq!(audited["data"]["result_count"], 1);
    assert_eq!(received_at, receipt.received_at);

    let (status, broker_identity) = read(&router, &broker_admin, IDENTITY, None).await;
    assert_eq!(status, StatusCode::OK);
    for (bearer, selected) in [
        (&reader, context(&identity, "personal")),
        (&broker_admin, context(&broker_identity, "administration")),
    ] {
        let (status, refused) = read(&router, bearer, QUOTA, Some(selected)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(refused["error"]["code"], "permission_denied");
        assert!(refused.get("quota").is_none() && refused.get("read_receipt").is_none());
    }
    let (status, _) = read(&router, &reader, USAGE, Some(admin)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    if format == 14 {
        let held = rusqlite::Connection::open_with_flags(
            &source,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .unwrap();
        held.execute_batch("BEGIN IMMEDIATE").unwrap();
        let (status, unavailable) = read(&router, &reader, QUOTA, Some(admin)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(unavailable.get("quota").is_none() && unavailable.get("read_receipt").is_none());
        held.execute_batch("ROLLBACK").unwrap();
    }
    let count: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM events WHERE kind='control.identity.access' AND json_extract(payload,'$.data.action') IN ('quota','usage')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "only the successful quota read leaves a receipt");
}

#[tokio::test]
async fn owned_quota_format14_reports_configured_limits_and_durable_reservations() {
    owned_quota_on(14).await;
}

#[tokio::test]
async fn owned_quota_format15_reports_configured_limits_and_durable_reservations() {
    owned_quota_on(15).await;
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
    let withheld = C::Export;
    let administrator = store.identity_memberships(fixture.principal()).unwrap();
    assert_eq!(administrator.len(), 1);
    assert!(
        !administrator[0].capabilities.contains(&withheld),
        "the fixture administrator must lack export"
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
    let store = state.tenants[tenant].store.sqlite().unwrap().clone();
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
        "capabilities": [C::ReadMetadata, withheld],
        "active": true,
    });
    let audit_before_refusal = super::races::snapshot(&database);
    let (status, refusal) = post(&router, &path, &context, &escalation).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refusal}");
    assert_eq!(refusal["error"]["code"], "permission_denied");
    assert_eq!(super::races::snapshot(&database), audit_before_refusal);
    assert!(expected_generation() == after);
    assert_eq!(count_mutations(), mutations_before + 1);
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
