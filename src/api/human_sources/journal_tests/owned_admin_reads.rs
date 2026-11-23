//! The G08 administration reads on the owned source profile: real owned
//! Router/Store reads over legacy stores that received the human rights
//! marker before adoption.
use super::{dashboard::LoopbackGateway, *};

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
        "usage_read",
    ] {
        assert_eq!(availability(operation), "available", "{operation}");
    }
    assert_eq!(availability("federation_read"), "profile_unavailable");
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
    assert_eq!(summary["sections"]["quota"]["status"], "ready");
    assert_eq!(summary["sections"]["usage"]["status"], "ready");
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

    let (status, usage) = get(&router, "/api/v1/human/admin/usage", Some(id)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the available usage projection may have no allocation"
    );
    assert!(usage["items"].as_array().unwrap().is_empty());
    assert_eq!(usage["billable"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_grant_read_format14_returns_the_existing_grant_and_one_receipt() {
    use super::{
        owned_workers::read,
        races::{connection, snapshot, state},
    };
    use HumanCapabilityV1 as C;

    let fixture = Fixture::new_owned_with_human_rights();
    grant(&fixture, &[C::ManageGrants, C::ReadMemberships]);
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: "owned-grant-read-session".into(),
    };
    seed_event(&fixture, "B", &resource.resource_id, fixture.principal());
    let state = state(&fixture);
    let database = connection(&state);
    let format: u32 = database
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(format, 14);
    let (gateway, dashboard, _front) = membership_dashboard(gateway_router(state)).await;
    let bearer = Fixture::bearer("B");
    let (status, identity) = read(&dashboard, &bearer, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    let context = identity["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|context| {
            context["tenant_id"] == "B"
                && context["organization_scope"] == "synthetic-org"
                && context["mode"] == "administration"
        })
        .expect("the fixture offers the matching administration context");

    let resource_key = resource.key().unwrap();
    let stored_grant = || -> String {
        database
            .query_row(
                "SELECT payload_json FROM identity_resources WHERE resource_key=?1",
                [&resource_key],
                |row| row.get(0),
            )
            .unwrap()
    };
    let prior_grant = stored_grant();
    let prior_head = snapshot(&database).0;
    let path = format!(
        "/api/v1/human/admin/grants?resource_kind=session&resource_id={}",
        resource.resource_id
    );
    let (status, body) = read(&dashboard, &bearer, &path, Some(context)).await;
    assert_eq!(status, StatusCode::OK);
    let body: HumanGrantV1 = serde_json::from_value(body).unwrap();
    assert_eq!(body.version, 1);
    assert_eq!(body.context_id, context["id"].as_str().unwrap());
    assert_eq!(
        body.authorization_version,
        context["authorization_version"].as_str().unwrap()
    );
    assert_eq!(body.source, "tenant_identity_resources");
    assert_eq!(body.resource, resource);
    assert_eq!(&body.owner, fixture.principal());
    assert_eq!(body.owner_display_name, "Synthetic local person");
    assert!(body.active);
    assert_eq!(body.revision, "1");
    assert!(body.shares.is_empty());

    let receipt = body.read_receipt;
    let (sequence, received_at, payload): (i64, String, String) = database
        .query_row(
            "SELECT seq,received_at,payload FROM events WHERE id=?1",
            [&receipt.audit_event_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let event: Event = serde_json::from_str(&payload).unwrap();
    assert_eq!(sequence, prior_head + 1);
    assert_eq!(snapshot(&database).0, sequence);
    assert_eq!(received_at, receipt.received_at);
    assert_eq!(event.kind, "control.identity.access");
    assert_eq!(event.data["action"], "grants");
    assert_eq!(event.data["request_id"], receipt.request_id);
    assert_eq!(event.data["principal"], json!(fixture.principal()));
    assert_eq!(event.data["mode"], "administration");
    assert_eq!(event.data["result_count"], 0);
    assert_eq!(stored_grant(), prior_grant);
    gateway.close().await;
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
    assert_eq!(sections["quota"]["status"], "ready");
    assert_eq!(sections["quota"]["data"]["held_workers"], "1");
    assert_eq!(sections["usage"]["status"], "ready");
    assert_eq!(sections["usage"]["data"]["allocations"], "0");
    assert_eq!(
        sections["attention"],
        json!({"status":"unavailable", "code":"profile_unavailable"})
    );
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
    assert_eq!(audited["data"]["result_count"], 5);
    assert_eq!(received_at, receipt["received_at"].as_str().unwrap());

    let (status, broker_identity) = read(&router, &broker_admin, IDENTITY, None).await;
    assert_eq!(status, StatusCode::OK);
    for (bearer, selected) in [
        (&reader, context(&identity, "personal")),
        (&broker_admin, context(&broker_identity, "administration")),
    ] {
        let (status, summary) = read(&router, bearer, SUMMARY, Some(selected)).await;
        assert_eq!(status, StatusCode::OK);
        for section in ["workers", "quota", "usage"] {
            assert_eq!(
                summary["sections"][section],
                json!({"status":"not_granted"})
            );
        }
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

async fn owned_accounting_summary_on(format: u32) {
    use super::owned_workers::{context, provision_reader, read};
    use crate::control::{QuotaPolicyV1, WorkerStateV1};
    use std::os::unix::fs::PermissionsExt;

    let historical = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let mut files = Store::owned_control_fixture_with_legacy_setup_for_test(format, |legacy| {
        for seconds in [0, 1] {
            let terminal = legacy.finish_usage_worker_for_test(Some(seconds));
            assert_eq!(terminal.worker.state, WorkerStateV1::Terminal);
            assert!(!terminal.worker.quota_held);
        }
        let unknown = legacy.finish_usage_worker_for_test(None);
        assert_eq!(unknown.worker.state, WorkerStateV1::InDoubt);
        assert!(unknown.worker.quota_held);
        legacy
            .migrate_human_rights_v2(&historical.path().join("before-rights.sqlite"))
            .unwrap();
    });
    let (bundle, credentials, tokens) = files.take_startup();
    let reader = provision_reader(&bundle, HumanCapabilityV1::ReadWorkerRegistry);
    let descriptor = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some("A"))
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
    .unwrap()
    .with_worker_quotas(vec![(
        "A".into(),
        QuotaPolicyV1 {
            version: 1,
            max_active_workers: 5,
            max_cpu_millis: 7000,
            max_memory_bytes: 4 * 1024 * 1024 * 1024,
            max_timeout_seconds: 7200,
        },
    )])
    .unwrap();
    let router = gateway_router(state);
    let (status, identity) = read(&router, &reader, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    let admin = context(&identity, "administration");
    let (status, summary) = read(
        &router,
        &reader,
        "/api/v1/human/summary?window=24h",
        Some(admin),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(summary["context_id"], admin["id"]);
    assert_eq!(
        summary["authorization_version"],
        admin["authorization_version"]
    );
    assert_eq!(summary["consistency"], "transaction_snapshot");
    let period = &summary["period"];
    assert_eq!(period["window"], "24h");
    assert_eq!(period["to"], summary["evaluated_at"]);
    let from = chrono::DateTime::parse_from_rfc3339(period["from"].as_str().unwrap()).unwrap();
    let to = chrono::DateTime::parse_from_rfc3339(period["to"].as_str().unwrap()).unwrap();
    assert_eq!(to - from, chrono::Duration::hours(24));

    let sections = &summary["sections"];
    assert_eq!(
        sections["quota"],
        json!({
            "status": "ready",
            "coverage": "complete_registry",
            "data": {
                "max_active_workers": "5",
                "max_cpu_millis": "7000",
                "max_memory_bytes": "4294967296",
                "held_workers": "1",
                "held_cpu_millis": "2000",
                "held_memory_bytes": "1073741824",
            },
        }),
    );
    assert_eq!(
        sections["usage"],
        json!({
            "status": "ready",
            "coverage": "complete_registry",
            "data": {
                "allocations": "2",
                "duration_ns": "1000000000",
                "cpu_millicore_nanoseconds": "2000000000000",
                "memory_byte_nanoseconds": "1073741824000000000",
                "method_version": 1,
                "billable": false,
            },
        }),
    );
    assert_eq!(sections["workers"]["data"]["runtime_inventory"], "unknown");
    assert_eq!(
        sections["attention"],
        json!({"status":"unavailable", "code":"profile_unavailable"})
    );

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
    assert_eq!(audited["data"]["result_count"], 5);
    assert_eq!(received_at, receipt["received_at"].as_str().unwrap());
    let count: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM events WHERE kind='control.identity.access' AND json_extract(payload,'$.data.action') IN ('summary','quota','usage')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "quota and usage share the summary receipt");
}

#[tokio::test]
async fn owned_accounting_summary_format14_reads_quota_and_observed_usage_with_one_receipt() {
    owned_accounting_summary_on(14).await;
}

#[tokio::test]
async fn owned_accounting_summary_format15_reads_quota_and_observed_usage_with_one_receipt() {
    owned_accounting_summary_on(15).await;
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

async fn owned_usage_on(format: u32, intervals: &[i64]) {
    use super::owned_workers::{context, provision_reader, read};
    use crate::control::{AllocationProvenanceV1, WorkerStateV1};
    use std::os::unix::fs::PermissionsExt;

    const IDENTITY: &str = "/api/v1/human/identity";
    const USAGE: &str = "/api/v1/human/admin/usage?limit=1";
    let historical = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let mut expected = Vec::new();
    let mut unknown_attempt = None;
    let mut files = Store::owned_control_fixture_with_legacy_setup_for_test(format, |legacy| {
        for &seconds in intervals {
            let result = legacy.finish_usage_worker_for_test(Some(seconds));
            assert_eq!(result.worker.state, WorkerStateV1::Terminal);
            assert!(!result.worker.quota_held);
            expected.push((result, seconds));
        }
        let unknown = legacy.finish_usage_worker_for_test(None);
        assert_eq!(unknown.worker.state, WorkerStateV1::InDoubt);
        assert!(unknown.worker.quota_held);
        unknown_attempt = Some(unknown.worker.attempt.attempt_id);
        legacy
            .migrate_human_rights_v2(&historical.path().join("before-rights.sqlite"))
            .unwrap();
    });
    let unknown_attempt = unknown_attempt.unwrap();
    expected.sort_unstable_by(|left, right| {
        left.0
            .worker
            .attempt
            .attempt_id
            .cmp(&right.0.worker.attempt.attempt_id)
    });
    let (bundle, credentials, tokens) = files.take_startup();
    let reader = provision_reader(&bundle, HumanCapabilityV1::ReadWorkerRegistry);
    let broker_admin = provision_reader(&bundle, HumanCapabilityV1::ReadBrokerRegistry);
    let descriptor = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some("A"))
        .unwrap();
    assert_eq!(descriptor.required_format, format);
    let source = descriptor.storage_binding.path();
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
    let (status, identity) = read(&router, &reader, IDENTITY, None).await;
    assert_eq!(status, StatusCode::OK);
    let admin = context(&identity, "administration");
    let usage = admin["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["operation"] == "usage_read")
        .unwrap();
    assert_eq!(usage["permission"], "granted");
    assert_eq!(usage["availability"], "available");

    let database = rusqlite::Connection::open_with_flags(
        source,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let mut path = USAGE.to_owned();
    let mut previous_receipt_seq = 0;
    for (index, (terminal, seconds)) in expected.iter().enumerate() {
        let (status, body) = read(&router, &reader, &path, Some(admin)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["context_id"], admin["id"]);
        assert_eq!(
            body["authorization_version"],
            admin["authorization_version"]
        );
        let page: HumanUsagePageV1 = serde_json::from_value(body).unwrap();
        assert_eq!(page.version, 1);
        assert_eq!(page.source, "tenant_terminal_allocations");
        assert_eq!(page.method_version, 1);
        assert!(!page.billable);
        assert_eq!(page.items.len(), 1);
        let item = &page.items[0];
        assert_ne!(item.attempt_id, unknown_attempt);
        assert_eq!(item.attempt_id, terminal.worker.attempt.attempt_id);
        assert_eq!(item.worker_id, terminal.worker.worker_id);
        assert_eq!(item.terminal_observation_id, terminal.receipt.operation_id);
        let duration = u128::try_from(*seconds).unwrap() * 1_000_000_000;
        assert_eq!(item.duration_ns, duration.to_string());
        assert_eq!(
            item.cpu_millicore_nanoseconds,
            (2000 * duration).to_string()
        );
        assert_eq!(
            item.memory_byte_nanoseconds,
            (1_073_741_824 * duration).to_string()
        );
        assert_eq!(item.method_version, 1);
        assert!(!item.billable);
        assert_eq!(item.source.provenance, AllocationProvenanceV1::Observed);

        let receipt = &page.read_receipt;
        let (seq, payload, received_at): (i64, String, String) = database
            .query_row(
                "SELECT seq,payload,received_at FROM events WHERE kind='control.identity.access' AND id=?1",
                [&receipt.audit_event_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let audited: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(audited["data"]["action"], "usage");
        assert_eq!(audited["data"]["mode"], "administration");
        assert_eq!(audited["data"]["principal"], identity["principal"]);
        assert_eq!(audited["data"]["request_id"], receipt.request_id);
        assert_eq!(audited["data"]["result_count"], 1);
        assert_eq!(received_at, receipt.received_at);
        assert!(seq > previous_receipt_seq);
        previous_receipt_seq = seq;

        assert_eq!(page.has_more, index + 1 < expected.len());
        if page.has_more {
            let cursor = page.next_cursor.as_deref().unwrap();
            let wrong_query = format!("/api/v1/human/admin/usage?limit=2&cursor={cursor}");
            let (status, error) = read(&router, &reader, &wrong_query, Some(admin)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(error["error"]["code"], "invalid_cursor");
            assert!(error.get("items").is_none() && error.get("read_receipt").is_none());
            path = format!("{USAGE}&cursor={cursor}");
        } else {
            assert!(page.next_cursor.is_none());
        }
    }

    let (status, broker_identity) = read(&router, &broker_admin, IDENTITY, None).await;
    assert_eq!(status, StatusCode::OK);
    for (bearer, selected) in [
        (&reader, context(&identity, "personal")),
        (&broker_admin, context(&broker_identity, "administration")),
    ] {
        let (status, error) = read(&router, bearer, USAGE, Some(selected)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(error["error"]["code"], "permission_denied");
        assert!(error.get("items").is_none() && error.get("read_receipt").is_none());
    }
    let count: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM events WHERE kind='control.identity.access' AND json_extract(payload,'$.data.action')='usage'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, i64::try_from(expected.len()).unwrap());
}

#[tokio::test]
async fn owned_usage_format14_pages_observed_allocations_and_omits_unknown_duration() {
    owned_usage_on(14, &[0, 1]).await;
}

#[tokio::test]
async fn owned_usage_format15_keeps_terminal_facts_and_audits_the_read() {
    owned_usage_on(15, &[1]).await;
}

async fn post(
    router: &Router,
    bearer: &str,
    path: &str,
    context: &str,
    body: &Value,
) -> (StatusCode, Value) {
    let request = axum::http::Request::post(path)
        .header("authorization", format!("Bearer {bearer}"))
        .header("x-aiguard-context", context)
        .header("content-type", "application/json; charset=utf-8")
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

fn seed_organization_member(fixture: &Fixture) -> MembershipV1 {
    use HumanCapabilityV1 as C;
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
    membership
}

fn seed_delegated_membership(fixture: &Fixture) -> MembershipV1 {
    use HumanCapabilityV1 as C;
    let membership = seed_organization_member(fixture);
    grant(
        fixture,
        &[
            C::ReadMetadata,
            C::ReadTenantMetadata,
            C::ReadMemberships,
            C::ManageMemberships,
        ],
    );
    membership
}

fn durable_identity_state(database: &rusqlite::Connection) -> (i64, i64) {
    database
        .query_row(
            "SELECT authorization_revision, (SELECT COUNT(*) FROM events WHERE kind='control.identity.mutation') \
             FROM identity_meta WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

#[tokio::test]
async fn owned_grant_change_format14_returns_one_durable_receipt_and_keeps_the_source_available() {
    use super::races::{connection, state};
    use HumanCapabilityV1 as C;

    const IDENTITY: &str = "/api/v1/human/identity";
    const GRANTS: &str = "/api/v1/human/admin/grants";
    const OWNED_GRANTS: &str = "/api/v1/human/owned/admin/grants";
    let fixture = Fixture::new_owned_with_human_rights();
    let member = seed_organization_member(&fixture);
    grant(&fixture, &[C::ManageGrants, C::ReadMemberships]);
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: "owned-grant-change-session".into(),
    };
    seed_event(&fixture, "B", &resource.resource_id, fixture.principal());
    let state = state(&fixture);
    let database = connection(&state);
    let format: u32 = database
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(format, 14);
    let (prior_revision, prior_mutations) = durable_identity_state(&database);
    let router = gateway_router(state);
    let bearer = Fixture::bearer("B");
    let admin_context_id = |identity: &Value| {
        identity["contexts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|context| {
                context["tenant_id"] == "B"
                    && context["organization_scope"] == member.organization_scope.as_str()
                    && context["mode"] == "administration"
            })
            .expect("the fixture offers the matching administration context")["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (status, identity) = get(&router, IDENTITY, None).await;
    assert_eq!(status, StatusCode::OK);
    let original_context = admin_context_id(&identity);
    let operation_id = uuid::Uuid::new_v4().to_string();
    let body = json!({
        "operation_id": operation_id,
        "resource": resource,
        "expected_revision": "1",
        "shares": [{"principal": member.principal, "permissions": ["metadata"]}],
    });
    let (status, original_receipt) = post(&router, &bearer, GRANTS, &original_context, &body).await;
    assert_eq!(status, StatusCode::OK);
    let receipt: HumanGrantChangeReceiptV1 =
        serde_json::from_value(original_receipt.clone()).unwrap();
    let next_revision = prior_revision + 1;
    assert_eq!(receipt.version, 1);
    assert_eq!(receipt.operation_id, operation_id);
    assert_eq!(receipt.resource, resource);
    assert_eq!(receipt.grant_revision, "2");
    assert_eq!(receipt.authorization_revision, next_revision.to_string());
    assert_eq!(
        durable_identity_state(&database),
        (next_revision, prior_mutations + 1)
    );

    let payload: String = database
        .query_row(
            "SELECT payload FROM events WHERE seq=?1",
            [receipt.audit_seq.parse::<i64>().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    let event: Event = serde_json::from_str(&payload).unwrap();
    assert_eq!(event.kind, "control.identity.mutation");
    assert_eq!(event.data["mutation"]["operation_id"], operation_id);
    assert_eq!(event.data["mutation"]["actor"], json!(fixture.principal()));

    let (status, identity) = get(&router, IDENTITY, None).await;
    assert_eq!(status, StatusCode::OK);
    let current_context = admin_context_id(&identity);
    assert_ne!(current_context, original_context);
    let (status, replay_receipt) = post(&router, &bearer, GRANTS, &current_context, &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay_receipt, original_receipt);
    assert_eq!(
        durable_identity_state(&database),
        (next_revision, prior_mutations + 1)
    );

    let read_path = format!(
        "{GRANTS}?resource_kind=session&resource_id={}",
        resource.resource_id
    );
    let (status, grant) = get(&router, &read_path, Some(&current_context)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(grant["resource"], body["resource"]);
    assert_eq!(grant["owner"], json!(fixture.principal()));
    assert_eq!(grant["revision"], "2");
    assert_eq!(
        grant["shares"],
        json!([{
            "principal": member.principal,
            "display_name": "Synthetic member",
            "permissions": ["metadata"],
        }])
    );

    let mut stale = body.clone();
    stale["operation_id"] = json!(uuid::Uuid::new_v4().to_string());
    let mut invalid = stale.clone();
    invalid["shares"][0]["permissions"] = json!([]);
    let (gateway, dashboard, _front) = membership_dashboard(router.clone()).await;
    for (target, path) in [
        (&router, GRANTS),
        (&router, OWNED_GRANTS),
        (&dashboard, GRANTS),
    ] {
        let (status, response) = post(target, &bearer, path, &current_context, &stale).await;
        assert_eq!(status, StatusCode::CONFLICT, "{path}: {response}");
        assert_eq!(response["error"]["code"], "conflict");
        assert_eq!(response["error"]["retry_action"], "reread");
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .starts_with("The shares changed")
        );
        let (status, response) = post(target, &bearer, path, &current_context, &invalid).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {response}");
        assert_eq!(response["error"]["code"], "invalid_request");
        assert_eq!(response["error"]["retry_action"], "retry");
        assert_eq!(
            response["error"]["message"],
            "Check the share change fields, then try again."
        );
    }
    let (status, response) = post(
        &router,
        &bearer,
        &format!("{OWNED_GRANTS}?resource_kind=session"),
        &current_context,
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    assert_eq!(response["error"]["code"], "invalid_request");
    assert_eq!(
        durable_identity_state(&database),
        (next_revision, prior_mutations + 1)
    );
    gateway.close().await;
}

async fn membership_dashboard(gateway: Router) -> (LoopbackGateway, Router, tempfile::TempDir) {
    let (gateway, address) = LoopbackGateway::start(gateway).await;
    let front = tempfile::tempdir().unwrap();
    let dashboard = dashboard_router_with_human_mode(
        &format!("http://{address}"),
        None,
        front.path().to_owned(),
        None,
        true,
    )
    .unwrap();
    (gateway, dashboard, front)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_sources_apply_a_delegated_membership_change_under_the_write_epoch() {
    use HumanCapabilityV1 as C;
    let fixture = Fixture::new_owned_with_human_rights();
    let bearer = Fixture::bearer("B");
    let membership = seed_delegated_membership(&fixture);
    let store = fixture.legacy_store("B");
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
    let (gateway, dashboard, _front) = membership_dashboard(router.clone()).await;
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
    let (status, receipt) = post(&dashboard, &bearer, &path, &context, &body).await;
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
    let (status, replayed) = post(&router, &bearer, &path, &context, &body).await;
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
    let (status, refusal) = post(&router, &bearer, &path, &context, &escalation).await;
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
    let (status, failure) = post(&router, &bearer, &path, &context, &reactivate).await;
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
    gateway.close().await;
}

fn provision_owned_organization(
    bundle: &HumanOwnedStartupBundle,
    descriptor: &crate::human_authority::HumanSourceDescriptorV1,
    organization: &str,
    administrator_capabilities: &[HumanCapabilityV1],
) -> (String, PrincipalRefV1, MembershipV1) {
    use HumanCapabilityV1 as C;

    let tenant = descriptor.tenant_id.as_deref().unwrap();
    let source = bundle.owner.source_token(descriptor.source_id).unwrap();
    let store = bundle.stores.get(&descriptor.source_id).unwrap();
    let provision = |display_name: &str, capabilities: &[C]| {
        let principal = PrincipalRefV1::HumanLocal {
            installation_id: descriptor.installation_id.to_string(),
            account_id: uuid::Uuid::new_v4().to_string(),
        };
        let bearer = generate_local_credential().unwrap();
        let membership = MembershipV1 {
            version: 1,
            membership_id: uuid::Uuid::new_v4().to_string(),
            principal: principal.clone(),
            tenant_id: tenant.into(),
            organization_scope: organization.into(),
            revision: 1,
            active: true,
            capabilities: capabilities.iter().copied().collect(),
        };
        let epoch = bundle.owner.try_write_epoch().unwrap();
        store
            .provision_local_human_scoped(
                &epoch,
                &source,
                &LocalHumanProvisionV1 {
                    version: 1,
                    mutations: [
                        change(IdentityChangeV1::Account {
                            expected_revision: 0,
                            account: HumanAccountV1 {
                                version: 1,
                                principal: principal.clone(),
                                display_name: display_name.into(),
                                revision: 1,
                                active: true,
                            },
                        }),
                        change(IdentityChangeV1::Credential {
                            expected_revision: 0,
                            binding: LocalCredentialV1 {
                                version: 1,
                                credential_id: uuid::Uuid::new_v4().to_string(),
                                principal,
                                sha256_digest: hex::encode(Sha256::digest(bearer.as_bytes())),
                                revision: 1,
                                active: true,
                                expires_at: None,
                            },
                        }),
                        change(IdentityChangeV1::Membership {
                            expected_revision: 0,
                            membership: membership.clone(),
                        }),
                    ],
                },
            )
            .unwrap();
        (bearer, membership)
    };
    let (bearer, administrator) = provision("Synthetic administrator", administrator_capabilities);
    let (_, member) = provision("Synthetic member", &[C::ReadMetadata]);
    (bearer, administrator.principal, member)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_membership_post_on_format15_requires_an_admin_and_returns_the_durable_receipt() {
    use super::owned_workers::{context, read};
    use HumanCapabilityV1 as C;
    use std::os::unix::fs::PermissionsExt;

    const IDENTITY: &str = "/api/v1/human/identity";
    const MEMBERSHIPS: &str = "/api/v1/human/admin/memberships";
    let historical = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let mut files = Store::owned_control_fixture_with_legacy_setup_for_test(15, |legacy| {
        legacy
            .migrate_human_rights_v2(&historical.path().join("before-rights.sqlite"))
            .unwrap();
    });
    let (bundle, credentials, tokens) = files.take_startup();
    let descriptor = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some("A"))
        .unwrap();
    assert_eq!(descriptor.required_format, 15);
    let database_path = descriptor.storage_binding.path();
    let (bearer, _, member) = provision_owned_organization(
        &bundle,
        descriptor,
        "synthetic-membership-administration",
        &[C::ReadMetadata, C::ReadMemberships, C::ManageMemberships],
    );
    let membership_id = member.membership_id;
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
    let (status, identity) = read(&router, &bearer, IDENTITY, None).await;
    assert_eq!(status, StatusCode::OK, "{identity}");
    let admin = context(&identity, "administration");
    let catalogue = admin["operations"].as_array().unwrap();
    let offer = catalogue
        .iter()
        .find(|entry| entry["operation"] == "memberships_manage")
        .unwrap();
    assert_eq!(offer["permission"], "granted");

    // The receipt must identify the durable mutation of this exact membership.
    let database = rusqlite::Connection::open_with_flags(
        database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let operation_id = uuid::Uuid::new_v4().to_string();
    let audit_positions = || {
        let mut statement = database
            .prepare("SELECT seq FROM events WHERE kind='control.identity.mutation' AND json_extract(payload,'$.data.mutation.operation_id')=?1")
            .unwrap();
        statement
            .query_map([&operation_id], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    let path = format!("{MEMBERSHIPS}/{membership_id}");
    let body = json!({
        "operation_id": operation_id,
        "expected_revision": "1",
        "capabilities": ["read_metadata"],
        "active": false,
    });
    let personal = context(&identity, "personal")["id"].as_str().unwrap();
    let (status, response) = post(&router, &bearer, &path, personal, &body).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{response}");
    assert_eq!(response["error"]["code"], "permission_denied");
    assert!(response.get("audit_seq").is_none());
    assert!(audit_positions().is_empty());

    let admin_id = admin["id"].as_str().unwrap();
    let request = |target: &str| {
        axum::http::Request::post(target)
            .header("authorization", format!("Bearer {bearer}"))
            .header("x-aiguard-context", admin_id)
    };
    for (content_type, content) in invalid_membership_uploads(&body) {
        let response = router
            .clone()
            .oneshot(
                request(&path)
                    .header("content-type", content_type)
                    .body(Body::from(content))
                    .unwrap(),
            )
            .await
            .unwrap();
        require_invalid_membership_request(response).await;
    }
    let encoded = serde_json::to_vec(&body).unwrap();
    let mut oversized = encoded.clone();
    oversized.resize(4097, b' ');
    let target_with_query = format!("{path}?limit=1");
    let mut revision_overflow = body.clone();
    revision_overflow["expected_revision"] = json!(u64::MAX.to_string());
    for invalid in [
        request(&path)
            .header("content-type", "application/json")
            .body(Body::from(oversized))
            .unwrap(),
        request(&target_with_query)
            .header("content-type", "application/json")
            .body(Body::from(encoded))
            .unwrap(),
        request(&path)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&revision_overflow).unwrap()))
            .unwrap(),
    ] {
        let response = router.clone().oneshot(invalid).await.unwrap();
        require_invalid_membership_request(response).await;
    }

    let stalled = request(&path)
        .header("content-type", "application/json")
        .body(Body::from_stream(futures_util::stream::pending::<
            Result<axum::body::Bytes, std::io::Error>,
        >()))
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(8), router.clone().oneshot(stalled))
        .await
        .expect("the authenticated body upload must be bounded")
        .unwrap();
    require_invalid_membership_request(response).await;
    let unauthenticated = axum::http::Request::post(&path)
        .header("content-type", "application/json")
        .body(Body::from_stream(futures_util::stream::poll_fn(
            |_| -> std::task::Poll<Option<Result<axum::body::Bytes, std::io::Error>>> {
                panic!("unauthenticated membership uploads must not be polled")
            },
        )))
        .unwrap();
    let response = router.clone().oneshot(unauthenticated).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(audit_positions().is_empty());

    let (gateway, dashboard, _front) = membership_dashboard(router.clone()).await;
    let mut repeated_capability = body.clone();
    repeated_capability["capabilities"] = json!(["read_metadata", "read_metadata"]);
    let (status, receipt) = post(&dashboard, &bearer, &path, admin_id, &repeated_capability).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["operation_id"], operation_id);
    assert_eq!(receipt["membership_id"], membership_id);
    assert_eq!(receipt["membership_revision"], "2");
    let audit_seq = receipt["audit_seq"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    assert_eq!(audit_positions(), [audit_seq]);

    let (status, denied) = read(&router, &bearer, MEMBERSHIPS, Some(admin)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(denied["error"]["code"], "context_revoked");
    let (status, current_identity) = read(&router, &bearer, IDENTITY, None).await;
    assert_eq!(status, StatusCode::OK);
    let current = context(&current_identity, "administration");
    let (status, replayed) = post(
        &router,
        &bearer,
        &path,
        current["id"].as_str().unwrap(),
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(replayed, receipt);
    assert_eq!(audit_positions(), [audit_seq]);
    let (status, page) = read(&router, &bearer, MEMBERSHIPS, Some(current)).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let member = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["membership_id"] == membership_id)
        .unwrap();
    assert_eq!(member["active"], false);
    assert_eq!(member["revision"], "2");
    gateway.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_grant_format15_reads_and_replays_one_durable_admin_change_through_dashboard() {
    use super::{
        owned_workers::{context, read},
        races::snapshot,
    };
    use crate::audit::identity::projection::Record as IdentityAuditRecord;
    use HumanCapabilityV1 as C;
    use ResourcePermissionV1 as P;
    use std::os::unix::fs::PermissionsExt;

    const IDENTITY: &str = "/api/v1/human/identity";
    const GRANTS: &str = "/api/v1/human/admin/grants";
    const ORGANIZATION: &str = "synthetic-grant-administration";
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: "owned-grant-format15-session".into(),
    };
    let historical = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let mut files = Store::owned_control_fixture_with_legacy_setup_for_test(15, |legacy| {
        let mut event = Event::new("fixture.owned-journal");
        event.session_id = Some(resource.resource_id.clone());
        legacy.append(event).unwrap();
        legacy
            .migrate_human_rights_v2(&historical.path().join("before-rights.sqlite"))
            .unwrap();
    });
    let (bundle, credentials, tokens) = files.take_startup();
    let descriptor = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some("A"))
        .unwrap();
    assert_eq!(descriptor.required_format, 15);
    let database_path = descriptor.storage_binding.path();
    let (bearer, administrator, member) = provision_owned_organization(
        &bundle,
        descriptor,
        ORGANIZATION,
        &[
            C::ReadMetadata,
            C::ReadContent,
            C::ReadMemberships,
            C::ManageGrants,
        ],
    );
    let initial_grant = ResourceGrantV1 {
        version: 1,
        tenant_id: member.tenant_id.clone(),
        organization_scope: ORGANIZATION.into(),
        resource: resource.clone(),
        owner: administrator.clone(),
        revision: 1,
        active: true,
        shares: vec![ResourceShareV1 {
            principal: member.principal.clone(),
            permissions: [P::Metadata].into_iter().collect(),
        }],
    };
    {
        let source = bundle.owner.source_token(descriptor.source_id).unwrap();
        let epoch = bundle.owner.try_write_epoch().unwrap();
        bundle
            .stores
            .get(&descriptor.source_id)
            .unwrap()
            .mutate_identity_scoped(
                &epoch,
                &source,
                &change(IdentityChangeV1::ResourceGrant {
                    expected_revision: 0,
                    grant: initial_grant.clone(),
                }),
            )
            .unwrap();
    }
    let database = rusqlite::Connection::open_with_flags(
        database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let format: u32 = database
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(format, 15);
    let (prior_revision, prior_mutations) = durable_identity_state(&database);
    let verify_read = |body, context: &Value, revision: &str, prior_head| {
        let grant: HumanGrantV1 = serde_json::from_value(body).unwrap();
        assert_eq!(grant.context_id, context["id"].as_str().unwrap());
        assert_eq!(
            grant.authorization_version,
            context["authorization_version"].as_str().unwrap()
        );
        assert_eq!(grant.resource, resource);
        assert_eq!(grant.owner, administrator);
        assert!(grant.active);
        assert_eq!(grant.revision, revision);
        assert_eq!(grant.shares.len(), 1);
        assert_eq!(grant.shares[0].principal, member.principal);
        assert_eq!(grant.shares[0].display_name, "Synthetic member");

        let receipt = &grant.read_receipt;
        let (seq, received_at, payload): (i64, String, String) = database
            .query_row(
                "SELECT seq,received_at,payload FROM events WHERE id=?1",
                [&receipt.audit_event_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let event: Event = serde_json::from_str(&payload).unwrap();
        assert_eq!(seq, prior_head + 1);
        assert_eq!(snapshot(&database).0, seq);
        assert_eq!(received_at, receipt.received_at);
        assert_eq!(event.kind, "control.identity.access");
        assert_eq!(event.data["action"], "grants");
        assert_eq!(event.data["request_id"], receipt.request_id);
        assert_eq!(event.data["principal"], json!(administrator));
        assert_eq!(event.data["mode"], "administration");
        assert_eq!(event.data["result_count"], 1);
        grant
    };
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
    let (gateway, dashboard, _front) = membership_dashboard(router).await;
    let (status, identity) = read(&dashboard, &bearer, IDENTITY, None).await;
    assert_eq!(status, StatusCode::OK, "{identity}");
    assert_eq!(identity["principal"], json!(administrator));
    let original = context(&identity, "administration");
    assert_eq!(original["organization_scope"], ORGANIZATION);
    let offer = original["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["operation"] == "grants_manage")
        .unwrap();
    assert_eq!(offer["permission"], "granted");
    assert_eq!(offer["availability"], "available");
    assert_eq!(offer["scope"], "organization");

    let path = format!(
        "{GRANTS}?resource_kind=session&resource_id={}",
        resource.resource_id
    );
    let prior_head = snapshot(&database).0;
    let (status, body) = read(&dashboard, &bearer, &path, Some(original)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let grant = verify_read(body, original, "1", prior_head);
    assert_eq!(grant.shares[0].permissions, [P::Metadata]);

    let operation_id = uuid::Uuid::new_v4().to_string();
    let body = json!({
        "operation_id": operation_id,
        "resource": resource,
        "expected_revision": grant.revision,
        "shares": [{"principal": member.principal, "permissions": ["metadata", "content"]}],
    });
    let (status, original_receipt) = post(
        &dashboard,
        &bearer,
        GRANTS,
        original["id"].as_str().unwrap(),
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{original_receipt}");
    let receipt: HumanGrantChangeReceiptV1 =
        serde_json::from_value(original_receipt.clone()).unwrap();
    let expected_state = (prior_revision + 1, prior_mutations + 1);
    assert_eq!(receipt.version, 1);
    assert_eq!(receipt.operation_id, operation_id);
    assert_eq!(receipt.resource, resource);
    assert_eq!(receipt.grant_revision, "2");
    assert_eq!(receipt.authorization_revision, expected_state.0.to_string());
    assert_eq!(durable_identity_state(&database), expected_state);
    let (status, denied) = read(&dashboard, &bearer, &path, Some(original)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");
    assert_eq!(denied["error"]["code"], "context_revoked");
    let (status, refreshed) = read(&dashboard, &bearer, IDENTITY, None).await;
    assert_eq!(status, StatusCode::OK, "{refreshed}");
    assert_eq!(refreshed["principal"], identity["principal"]);
    let current = context(&refreshed, "administration");
    assert_eq!(current["organization_scope"], ORGANIZATION);
    assert_ne!(current["id"], original["id"]);
    assert_ne!(
        current["authorization_version"],
        original["authorization_version"]
    );
    let (status, replayed) = post(
        &dashboard,
        &bearer,
        GRANTS,
        current["id"].as_str().unwrap(),
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(replayed, original_receipt);

    let audit_seq = receipt.audit_seq.parse::<i64>().unwrap();
    let resource_key = resource.key().unwrap();
    let (payload, indexed_hash, projected): (String, String, String) = database
        .query_row(
            "SELECT e.payload,o.input_hash,r.payload_json FROM events e
             JOIN identity_operations o ON o.event_seq=e.seq
             JOIN identity_resources r ON r.event_seq=e.seq
             WHERE e.seq=?1 AND o.operation_id=?2 AND r.resource_key=?3",
            rusqlite::params![audit_seq, operation_id, resource_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    let event: Event = serde_json::from_str(&payload).unwrap();
    assert_eq!(event.kind, "control.identity.mutation");
    let record = IdentityAuditRecord::decode_replay(&event.data).unwrap();
    let expected_grant = ResourceGrantV1 {
        revision: 2,
        shares: vec![ResourceShareV1 {
            principal: member.principal.clone(),
            permissions: [P::Metadata, P::Content].into_iter().collect(),
        }],
        ..initial_grant
    };
    assert_eq!(record.mutation.operation_id, operation_id);
    assert_eq!(record.mutation.actor, administrator);
    assert_eq!(
        record.authorization_revision.to_string(),
        receipt.authorization_revision
    );
    assert_eq!(record.input_hash, indexed_hash);
    assert_eq!(
        record.mutation.change,
        IdentityChangeV1::ResourceGrant {
            expected_revision: 1,
            grant: expected_grant.clone(),
        }
    );
    assert_eq!(
        serde_json::from_str::<ResourceGrantV1>(&projected).unwrap(),
        expected_grant
    );

    let prior_head = snapshot(&database).0;
    let (status, updated) = read(&dashboard, &bearer, &path, Some(current)).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    let grant = verify_read(updated, current, "2", prior_head);
    assert_eq!(grant.shares[0].permissions, [P::Metadata, P::Content]);
    assert_eq!(durable_identity_state(&database), expected_state);
    gateway.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_dashboard_refuses_legacy_gateway_without_changing_membership_or_audit() {
    let fixture = Fixture::new_owned_with_human_rights();
    let membership = seed_delegated_membership(&fixture);
    let bearer = Fixture::bearer("B");
    let state = GatewayState::new(
        Arc::new(fixture.legacy_store("B").sibling().unwrap()),
        "synthetic-membership-runner".into(),
        "synthetic-membership-viewer".into(),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    );
    assert!(state.owned_human.is_none());
    let store = state.tenants[0].store.sqlite().unwrap().clone();
    let router = gateway_router(state);
    let (status, identity) = get(&router, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK, "{identity}");
    let context_id = identity["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|context| context["tenant_id"] == "B" && context["mode"] == "administration")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let path = format!(
        "/api/v1/human/admin/memberships/{}",
        membership.membership_id
    );
    let change = json!({
        "operation_id": uuid::Uuid::new_v4().to_string(),
        "expected_revision": "1",
        "capabilities": ["read_metadata"],
        "active": false,
    });
    let before = store.head().unwrap();
    let observed = crate::audit::read_test_observer::observe(&store);
    let (gateway, dashboard, _front) = membership_dashboard(router.clone()).await;

    let (status, failure) = post(&dashboard, &bearer, &path, context_id, &change).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{failure}");
    assert_eq!(failure["error"]["code"], "identity_unavailable");
    assert!(failure.get("audit_seq").is_none());
    assert!(
        observed.try_recv().is_err(),
        "legacy refused before Store access"
    );
    assert_eq!(store.head().unwrap(), before);

    // The same authorized operation is still served by the historical route.
    let (status, receipt) = post(&router, &bearer, &path, context_id, &change).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["operation_id"], change["operation_id"]);
    assert_eq!(receipt["membership_id"], membership.membership_id);
    assert_eq!(receipt["membership_revision"], "2");
    assert_eq!(store.head().unwrap(), before + 1);
    gateway.close().await;
}
