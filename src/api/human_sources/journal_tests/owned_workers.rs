use super::*;
use crate::api::human_sources::owned_control_tests::negative::reserve_workload_fixture_worker;
use crate::human_authority::startup::HumanOwnedStartupBundle;
use sha2::{Digest, Sha256};

const WORKERS: &str = "/api/v1/human/workers?limit=1";
const RECEIPT_KIND: &str = "control.identity.workers.access";

fn provision_reader(bundle: &HumanOwnedStartupBundle, capability: HumanCapabilityV1) -> String {
    let descriptor = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some("A"))
        .unwrap();
    let source = bundle.owner.source_token(descriptor.source_id).unwrap();
    let store = bundle.stores.get(&descriptor.source_id).unwrap();
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: descriptor.installation_id.to_string(),
        account_id: uuid::Uuid::new_v4().to_string(),
    };
    let bearer = generate_local_credential().unwrap();
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
                            display_name: "Synthetic worker registry reader".into(),
                            revision: 1,
                            active: true,
                        },
                    }),
                    change(IdentityChangeV1::Credential {
                        expected_revision: 0,
                        binding: LocalCredentialV1 {
                            version: 1,
                            credential_id: uuid::Uuid::new_v4().to_string(),
                            principal: principal.clone(),
                            sha256_digest: hex::encode(Sha256::digest(bearer.as_bytes())),
                            revision: 1,
                            active: true,
                            expires_at: None,
                        },
                    }),
                    change(IdentityChangeV1::Membership {
                        expected_revision: 0,
                        membership: MembershipV1 {
                            version: 1,
                            membership_id: uuid::Uuid::new_v4().to_string(),
                            principal,
                            tenant_id: "A".into(),
                            organization_scope: "synthetic-owned-worker-registry".into(),
                            revision: 1,
                            active: true,
                            capabilities: [HumanCapabilityV1::ReadMetadata, capability]
                                .into_iter()
                                .collect(),
                        },
                    }),
                ],
            },
        )
        .unwrap();
    epoch.check().unwrap();
    bearer
}

async fn read(
    router: &Router,
    bearer: &str,
    path: &str,
    context: Option<&Value>,
) -> (StatusCode, Value) {
    let mut request =
        axum::http::Request::get(path).header("authorization", format!("Bearer {bearer}"));
    if let Some(context) = context {
        request = request.header("x-aiguard-context", context["id"].as_str().unwrap());
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let bytes = axum::body::to_bytes(response.into_body(), 262_144)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn context<'a>(identity: &'a Value, mode: &str) -> &'a Value {
    identity["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|context| context["tenant_id"] == "A" && context["mode"] == mode)
        .expect("the gateway offers the expected fixture context")
}

async fn owned_workers_on(format: u32) {
    let mut worker = None;
    let mut fixture = Store::owned_control_fixture_with_legacy_setup_for_test(format, |legacy| {
        worker = Some(reserve_workload_fixture_worker(legacy));
    });
    let worker = worker.unwrap();
    let (bundle, credentials, tokens) = fixture.take_startup();
    let reader = provision_reader(&bundle, HumanCapabilityV1::ReadWorkerRegistry);
    let other_admin = provision_reader(&bundle, HumanCapabilityV1::ReadConfiguration);
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
    .unwrap();
    let router = gateway_router(state);
    let (status, identity) = read(&router, &reader, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    let admin = context(&identity, "administration");
    let (status, body) = read(&router, &reader, WORKERS, Some(admin)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        identity["read_profiles"]["worker_registry"],
        "human_worker_registry_owned_v1"
    );
    assert_eq!(
        identity["read_profiles"]["federation_registry"],
        "unavailable"
    );
    let operations = admin["operations"].as_array().unwrap();
    let worker_access = operations
        .iter()
        .find(|entry| entry["operation"] == "workers_read")
        .unwrap();
    assert_eq!(worker_access["availability"], "available");
    assert_eq!(worker_access["permission"], "granted");
    let federation = operations
        .iter()
        .find(|entry| entry["operation"] == "federation_read")
        .unwrap();
    assert_eq!(federation["availability"], "profile_unavailable");
    assert_eq!(body["context_id"], admin["id"]);
    assert_eq!(
        body["authorization_version"],
        admin["authorization_version"]
    );
    let page: HumanWorkerRegistryPageV1 = serde_json::from_value(body).unwrap();
    assert_eq!(page.source, "tenant_worker_broker_registry");
    assert_eq!(page.consistency, "snapshot_per_page");
    assert_eq!(page.runtime_inventory, "unknown");
    assert_eq!(page.items.len(), 1);
    assert!(!page.has_more);
    assert!(page.next_cursor.is_none());
    let item = &page.items[0];
    assert_eq!(item.worker_id, worker.worker_id);
    assert_eq!(item.attempt_id, worker.attempt.attempt_id);
    assert_eq!(item.session_id, worker.launch.session_id);
    assert_eq!(item.session_provenance, "declared");
    assert_eq!(item.state, crate::control::WorkerStateV1::Reserved);
    assert!(item.quota_held && item.last_observation.is_none());

    let database = rusqlite::Connection::open_with_flags(
        source,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let receipt = &page.read_receipt;
    let (payload, received_at): (String, String) = database
        .query_row(
            "SELECT payload,received_at FROM events WHERE kind=?1 AND id=?2",
            rusqlite::params![RECEIPT_KIND, receipt.audit_event_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let audited: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(audited["data"]["principal"], identity["principal"]);
    assert_eq!(audited["data"]["mode"], "administration");
    assert_eq!(audited["data"]["request_id"], receipt.request_id);
    assert_eq!(audited["data"]["result_count"], 1);
    assert_eq!(received_at, receipt.received_at);

    let (status, other_identity) =
        read(&router, &other_admin, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    for (bearer, selected) in [
        (&reader, context(&identity, "personal")),
        (&other_admin, context(&other_identity, "administration")),
    ] {
        let (status, denied) = read(&router, bearer, WORKERS, Some(selected)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(denied["error"]["code"], "permission_denied");
        assert!(denied.get("items").is_none());
        assert!(denied.get("read_receipt").is_none());
    }
    let count: i64 = database
        .query_row(
            "SELECT count(*) FROM events WHERE kind=?1",
            [RECEIPT_KIND],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "refused reads must not append a receipt");
}

#[tokio::test]
async fn owned_workers_format14_returns_registered_metadata_and_durable_receipt() {
    owned_workers_on(14).await;
}

#[tokio::test]
async fn owned_workers_format15_returns_registered_metadata_and_durable_receipt() {
    owned_workers_on(15).await;
}
