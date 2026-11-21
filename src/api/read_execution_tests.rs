//! Execution-location regression, not a latency or availability benchmark.
//!
//! A direct Store call from the viewer request would make this test fail.
//! The paired real Store control validates the observer without sleeps or
//! injected waits. This does not establish a deployed cross-tenant exploit,
//! its severity, a latency bound, or the runner's behavior under contention.

use super::*;
use crate::audit::read_test_observer::{Call, observe};
use axum::http::Request;
use std::{os::unix::fs::PermissionsExt, thread};
use tower::ServiceExt;

pub(super) fn identity_change(
    change: crate::identity::IdentityChangeV1,
) -> crate::identity::IdentityMutationV1 {
    use crate::identity::*;
    IdentityMutationV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "fixture-control-operator".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    }
}

pub(super) fn local_identity_state() -> (tempfile::TempDir, GatewayState, String) {
    use crate::identity::*;
    use std::collections::BTreeSet;
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let token = generate_local_credential().unwrap();
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: "909d9572-f1a2-4a53-ad8b-a741030a2b18".into(),
        account_id: "290a1a3b-8538-49ef-a830-25df6fa19030".into(),
    };
    let mut tenants = Vec::new();
    for (index, tenant_id) in ["tenant-a", "tenant-b"].into_iter().enumerate() {
        let path = directory.path().join(format!("{tenant_id}.sqlite"));
        let store = Store::open_for_tenant(&path, tenant_id, Redactor::new(vec![]), 1000).unwrap();
        store
            .migrate_control_v3(&path.with_extension("before-v3"))
            .unwrap();
        store
            .migrate_identity_v4(&path.with_extension("before-v4"))
            .unwrap();
        store
            .migrate_human_access_v5(&path.with_extension("before-v5"))
            .unwrap();
        let account = identity_change(IdentityChangeV1::Account {
            expected_revision: 0,
            account: HumanAccountV1 {
                version: 1,
                principal: principal.clone(),
                display_name: "Local B reader".into(),
                revision: 1,
                active: true,
            },
        });
        let membership = identity_change(IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: MembershipV1 {
                version: 1,
                membership_id: uuid::Uuid::new_v4().to_string(),
                principal: principal.clone(),
                tenant_id: tenant_id.into(),
                organization_scope: "organization-ab".into(),
                revision: 1,
                active: true,
                capabilities: BTreeSet::from([HumanCapabilityV1::ReadMetadata]),
            },
        });
        if tenant_id == "tenant-b" {
            let credential = identity_change(IdentityChangeV1::Credential {
                expected_revision: 0,
                binding: LocalCredentialV1 {
                    version: 1,
                    credential_id: uuid::Uuid::new_v4().to_string(),
                    principal: principal.clone(),
                    sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                    revision: 1,
                    active: true,
                    expires_at: None,
                },
            });
            store
                .provision_local_human(&LocalHumanProvisionV1 {
                    version: 1,
                    mutations: [account, credential, membership],
                })
                .unwrap();
        } else {
            // A grants a real context to this principal without owning B's
            // credential. This also makes accidental reconciliation observable.
            store.mutate_identity(&account).unwrap();
            store.mutate_identity(&membership).unwrap();
        }
        tenants.push(TenantGateway {
            tenant_id: tenant_id.into(),
            store: Arc::new(store),
            runner_token: format!("fixture-runner-{index}").repeat(3),
            viewer_token: format!("fixture-viewer-{index}").repeat(3),
        });
    }
    let state =
        GatewayState::for_tenants(tenants, Policy::default(), None, Redactor::new(vec![])).unwrap();
    (directory, state, token)
}

pub(super) async fn human_get(
    router: &Router,
    token: &str,
    path: &str,
    context: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    if let Some(context) = context {
        request = request.header("x-aiguard-context", context);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 262_144)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn dashboard_reports_read_capacity_and_can_reload_after_recovery() {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = Arc::new(
        Store::open(
            &directory.path().join("audit.sqlite"),
            Redactor::new(vec![]),
            100,
        )
        .unwrap(),
    );
    let viewer = "fixture-viewer";
    let state = GatewayState::new(
        store,
        "fixture-runner".into(),
        viewer.into(),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = format!("http://{}", listener.local_addr().unwrap());
    let router = gateway_router(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let dashboard =
        dashboard_router(&upstream, viewer.into(), directory.path().to_owned()).unwrap();
    let slots: Vec<_> = (0..4)
        .map(|_| {
            state.tenants[0]
                .io
                .admit(store_io::Lane::Read, false)
                .unwrap()
        })
        .collect();

    let path = "/api/events?limit=100";
    let (status, unavailable) = human_get(&dashboard, viewer, path, None).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(unavailable["error"]["code"], "capacity_exhausted");
    assert_eq!(unavailable["error"]["retry_action"], "retry");

    drop(slots);
    let (status, available) = human_get(&dashboard, viewer, path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(available["items"].is_array());
    server.abort();
}

#[tokio::test]
async fn source_read_admission_rejects_before_opening_local_authentication_transactions() {
    let (_directory, state, token) = local_identity_state();
    let observations = observe(state.tenants[1].store.sqlite().unwrap());
    let router = gateway_router(state.clone());
    let (status, _) = human_get(&router, &token, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK);
    let baseline_transactions = observations
        .try_iter()
        .filter(|sample| sample.call == Call::BeginChecked)
        .count();
    assert!(baseline_transactions > 0);

    let mut source_slots = Vec::new();
    loop {
        match state.tenants[1].io.admit(store_io::Lane::Read, false) {
            Ok(permit) => source_slots.push(permit),
            Err((status, _)) => {
                assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
                break;
            }
        }
    }
    assert!(!source_slots.is_empty());
    // Metadata discovery is nonblocking and consumes no tenant lane. The
    // independently selected source still needs its own checked-read permit.
    let _available_discovery_slot = state.tenants[1]
        .io
        .admit(store_io::Lane::Discovery, false)
        .unwrap();
    let (status, _) = human_get(&router, &token, "/api/v1/human/identity", None).await;
    let transactions = observations
        .try_iter()
        .filter(|sample| sample.call == Call::BeginChecked)
        .count();
    assert_eq!(
        transactions, 0,
        "source B opened Store transactions despite saturated source reads; status={status}"
    );
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn runner_control_survives_saturated_source_discovery() {
    let (_directory, state, _human_token) = local_identity_state();
    let observations = observe(state.tenants[1].store.sqlite().unwrap());
    let mut source_metadata_slots = Vec::new();
    while let Ok(permit) = state.tenants[1].io.admit(store_io::Lane::Discovery, false) {
        source_metadata_slots.push(permit);
    }
    assert!(!source_metadata_slots.is_empty());
    drop(
        state.tenants[1]
            .io
            .admit(store_io::Lane::Control, false)
            .unwrap(),
    );
    let router = gateway_router(state);
    let (status, protocol) = human_get(
        &router,
        &"fixture-runner-1".repeat(3),
        "/api/v1/control/protocol",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(protocol["tenant_id"], "tenant-b");
    assert_eq!(protocol["role"], "runner");
    assert!(
        observations
            .try_iter()
            .filter(|sample| sample.call == Call::BeginChecked)
            .count()
            > 0
    );
}

#[tokio::test]
async fn saturated_neighbor_keeps_local_identity_partial_and_preserves_its_context() {
    let (_directory, state, token) = local_identity_state();
    let router = gateway_router(state.clone());
    let path = "/api/v1/human/identity";
    let (status, baseline) = human_get(&router, &token, path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(baseline["contexts_complete"], true);
    assert_eq!(baseline["contexts"].as_array().unwrap().len(), 2);
    let context_a = baseline["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|context| context["tenant_id"] == "tenant-a")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let source_slots: Vec<_> = (0..4)
        .map(|_| {
            state.tenants[1]
                .io
                .admit(store_io::Lane::Read, false)
                .unwrap()
        })
        .collect();
    let (status, _) = human_get(&router, &token, path, None).await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "the authenticated source must enforce its own admission"
    );
    drop(source_slots);

    let neighbor_slots: Vec<_> = (0..4)
        .map(|_| {
            state.tenants[0]
                .io
                .admit(store_io::Lane::Read, false)
                .unwrap()
        })
        .collect();
    let (status, partial) = human_get(&router, &token, path, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a saturated neighbor must not deny the healthy credential source"
    );
    assert_eq!(partial["contexts_complete"], false);
    let contexts = partial["contexts"].as_array().unwrap();
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0]["tenant_id"], "tenant-b");
    drop(neighbor_slots);

    // The skipped A snapshot is unknown, not an empty reconciliation. Its
    // previously issued context must remain usable after admission recovers.
    let (status, _) = human_get(&router, &token, "/api/v1/human/resources", Some(&context_a)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, recovered) = human_get(&router, &token, path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recovered["contexts_complete"], true);
    assert_eq!(recovered["contexts"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn ordinary_journal_distinguishes_invalid_filters_from_read_failure() {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().join("audit.sqlite");
    let store =
        Arc::new(Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 100).unwrap());
    store.append(Event::new("test.journal_read")).unwrap();
    let last = store.append(Event::new("test.journal_read")).unwrap();
    let viewer = "b".repeat(48);
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "tenant-a".into(),
            store,
            runner_token: "a".repeat(48),
            viewer_token: viewer.clone(),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let tenant = state.tenants[0].clone();
    let router = gateway_router(state);
    for route in ["/api/events", "/api/v1/events"] {
        let (status, page) = human_get(&router, &viewer, &format!("{route}?limit=1"), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert_eq!(page["items"][0]["seq"], last.seq);
        assert_eq!(page["next_cursor"], last.seq);
        let (status, next_page) = human_get(
            &router,
            &viewer,
            &format!("{route}?before={}", last.seq),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(next_page["items"][0]["seq"].as_i64().unwrap() < last.seq);
        for filter in [
            "from=invalid".into(),
            "to=invalid".into(),
            format!("q={}", "x".repeat(257)),
        ] {
            let (status, body) =
                human_get(&router, &viewer, &format!("{route}?{filter}"), None).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body, json!({"error": "Filtre invalide."}));
        }
        let (status, _) =
            human_get(&router, "unknown", &format!("{route}?from=invalid"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    let db = rusqlite::Connection::open(path).unwrap();
    db.execute(
        "UPDATE events SET hash='fixture-invalid' WHERE seq=?1",
        [last.seq],
    )
    .unwrap();
    assert!(!tenant.store.integrity().unwrap().valid);
    // Isolate the error mapping in the read handler from the authentication
    // step, which can also reject an unavailable Store.
    let (status, _) = list_events(ReadIdentity(tenant, "viewer"), Query(EventQuery::default()))
        .await
        .unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn viewer_integrity_store_work_stays_off_runtime_worker() {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = directory.path().join("audit.sqlite");
    let store =
        Arc::new(Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 100).unwrap());
    // Each string stays below Redactor::value's 32 KiB string limit. A single
    // 100,000-byte string would be clipped before persistence.
    let filler = vec!["x".repeat(25_000); 4];
    for index in 0..60 {
        let mut event = Event::new("test.read_execution");
        event.data = json!({"index":index,"filler":filler});
        store.append(event).unwrap();
    }
    let viewer = "b".repeat(48);
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "tenant-a".into(),
            store,
            runner_token: "a".repeat(48),
            viewer_token: viewer.clone(),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    // Gateway redaction attachment can replace the input Arc with a sibling.
    // Observe the exact Store reached by authentication and the HTTP handler.
    let actual_store = Arc::clone(state.tenants[0].store.sqlite().unwrap());
    let fixture = actual_store
        .events(&EventQuery {
            kind: Some("test.read_execution".into()),
            order: Some("asc".into()),
            limit: Some(100),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(fixture.len(), 60);
    for (index, stored) in fixture.iter().enumerate() {
        assert_eq!(stored.event.data["index"].as_u64(), Some(index as u64));
        let chunks = stored.event.data["filler"]
            .as_array()
            .expect("persisted fixture filler is an array");
        assert_eq!(chunks.len(), 4, "fixture event {index}: chunk count");
        let mut observed_filler_bytes = 0;
        for (chunk_index, chunk) in chunks.iter().enumerate() {
            let chunk = chunk.as_str().expect("persisted fixture chunk is a string");
            assert_eq!(
                chunk.len(),
                25_000,
                "fixture event {index}, chunk {chunk_index}: persisted length"
            );
            assert!(
                chunk.bytes().all(|byte| byte == b'x'),
                "fixture event {index}, chunk {chunk_index}: unexpected persisted bytes"
            );
            observed_filler_bytes += chunk.len();
        }
        assert_eq!(observed_filler_bytes, 100_000, "fixture event {index}");
    }
    // Store initialization also records control.schema.genesis. Derive the
    // complete count after gateway setup instead of assuming only fixture rows.
    let setup_integrity = actual_store.integrity().unwrap();
    assert!(setup_integrity.valid);
    let expected_count = setup_integrity.event_count;
    assert!(expected_count >= fixture.len() as u64);

    let tenant = state.tenants[0].clone();
    let router = gateway_router(state);
    let history = "/api/v1/events";
    let (status, page) = human_get(&router, &viewer, &format!("{history}?limit=1"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["next_cursor"], page["items"][0]["seq"]);
    for filter in [
        "from=invalid".into(),
        "to=invalid".into(),
        format!("q={}", "x".repeat(257)),
    ] {
        let (status, body) =
            human_get(&router, &viewer, &format!("{history}?{filter}"), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, json!({"error":"Filtre invalide."}));
    }
    let (status, _) = human_get(&router, "unknown", &format!("{history}?from=invalid"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let observations = observe(&actual_store);
    let control_store = Arc::clone(&actual_store);
    let control_worker = tokio::spawn(async move {
        let worker = thread::current().id();
        let result = tokio::task::spawn_blocking(move || control_store.integrity())
            .await
            .unwrap()
            .unwrap();
        assert!(result.valid);
        assert_eq!(result.event_count, expected_count);
        worker
    })
    .await
    .unwrap();
    let control: Vec<_> = observations.try_iter().collect();
    assert!(control.iter().any(|sample| sample.call == Call::Integrity));
    assert!(control.iter().all(|sample| sample.thread != control_worker));

    let (worker, status) = tokio::spawn(async move {
        let worker = thread::current().id();
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/integrity")
                    .header(header::AUTHORIZATION, format!("Bearer {viewer}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        (worker, response.status())
    })
    .await
    .unwrap();
    assert_eq!(status, StatusCode::OK);
    let request: Vec<_> = observations.try_iter().collect();
    assert!(
        request
            .iter()
            .any(|sample| sample.call == Call::BeginChecked)
    );
    assert!(request.iter().any(|sample| sample.call == Call::Integrity));
    assert!(
        request.iter().all(|sample| sample.thread != worker),
        "viewer authentication or integrity ran synchronous Store work on its runtime worker: {request:?}"
    );

    let db = rusqlite::Connection::open(path).unwrap();
    db.execute(
        "UPDATE events SET hash='tampered' WHERE seq=(SELECT MAX(seq) FROM events)",
        [],
    )
    .unwrap();
    assert!(!actual_store.integrity().unwrap().valid);
    // Exercise the read handler after identity selection. A router-level 503
    // alone could come from the collision check using the same unavailable Store.
    let (status, _) = list_events(ReadIdentity(tenant, "viewer"), Query(EventQuery::default()))
        .await
        .unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}
