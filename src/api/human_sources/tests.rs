use super::*;
use crate::human_authority::migration::tests::Fixture;
use tower::ServiceExt;

pub(crate) fn fixture() -> (Fixture, GatewayState) {
    let fixture = Fixture::new_owned();
    let state = state_from_fixture(&fixture);
    (fixture, state)
}

#[tokio::test]
async fn identity_announces_actual_owned_read_profiles_and_omits_them_on_legacy() {
    let (_owned_files, owned) = fixture();
    let (status, body) = get(&gateway_router(owned), "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["read_profiles"],
        json!({
            "broker_registry":"human_broker_registry_owned_v1",
            "worker_registry":"unavailable", "federation_registry":"unavailable"
        })
    );
    let (_legacy_files, legacy, token) = crate::api::read_execution_tests::local_identity_state();
    let response = gateway_router(legacy)
        .oneshot(
            axum::http::Request::get("/api/v1/human/identity")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 262_144)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body.get("read_profiles").is_none());
}
fn state_from_fixture(fixture: &Fixture) -> GatewayState {
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
    state
}

#[tokio::test]
async fn same_incarnation_identity_or_workload_change_requires_rebuild_without_neighbor_sql() {
    for workload_change in [false, true] {
        let (_fixture, state) = fixture();
        let owned = state.owned_human.as_ref().unwrap();
        let observed_a =
            crate::audit::read_test_observer::observe(state.tenants[0].store.sqlite().unwrap());
        let incarnation = owned.owner.incarnation();
        let source = &owned.sources[1];
        let epoch = owned.owner.try_write_epoch().unwrap();
        let access = HumanSourceAccess::write(&epoch, source).unwrap();
        let store = &state.tenants[1].store;
        if workload_change {
            store
                .sqlite()
                .unwrap()
                .issue_workload_binding_scoped(
                    &access,
                    &crate::workload::WorkloadBindingIssueV1 {
                        version: 1,
                        operation_id: uuid::Uuid::new_v4().to_string(),
                        credential_id: uuid::Uuid::new_v4().to_string(),
                        expected_revision: 0,
                        subject: crate::workload::WorkloadSubjectV1::Broker {
                            broker_id: uuid::Uuid::new_v4().to_string(),
                            incarnation_id: uuid::Uuid::new_v4().to_string(),
                        },
                        sha256_digest: "73".repeat(32),
                        expires_at: (chrono::Utc::now() + chrono::Duration::minutes(5))
                            .to_rfc3339(),
                    },
                )
                .unwrap();
        } else {
            let digest: [u8; 32] = Sha256::digest(Fixture::bearer("B").as_bytes()).into();
            let mut binding = store
                .sqlite()
                .unwrap()
                .local_credential_binding_scoped(&access, &digest)
                .unwrap()
                .unwrap();
            binding.revision += 1;
            store
                .sqlite()
                .unwrap()
                .mutate_identity_scoped(
                    &epoch,
                    source,
                    &IdentityMutationV1 {
                        version: 1,
                        operation_id: uuid::Uuid::new_v4().to_string(),
                        actor: PrincipalRefV1::LegacyControl {
                            credential_id: "synthetic-offline-control".into(),
                            role: LegacyControlRoleV1::Runner,
                        },
                        change: IdentityChangeV1::Credential {
                            expected_revision: 1,
                            binding,
                        },
                    },
                )
                .unwrap();
        }
        drop(epoch);
        assert_eq!(owned.owner.incarnation(), incarnation);
        let (status, body) = get(
            &gateway_router(state.clone()),
            "/api/v1/human/identity",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert_eq!(body["error"]["code"], "identity_unavailable");
        assert_eq!(owned.coverage.load(Ordering::Acquire), REBUILD_REQUIRED);
        assert!(
            owned.owner.admission_available(),
            "projection-generation mismatch must not invent global source invalidation"
        );
        assert!(observed_a.try_recv().is_err());
    }
}

#[tokio::test]
async fn real_exhausted_owned_profile_budget_preserves_429_without_publishing_contexts() {
    let fixture = Fixture::new_owned();
    let (mut request, credentials, tokens) = fixture.owned_request();
    for options in request.sources.values_mut() {
        if let crate::human_authority::startup::HumanOwnedSourceOptions::TenantStore {
            capacity,
            ..
        } = options
        {
            capacity.human_tenant.events = 4;
            capacity.human_principal.events = 2;
        }
    }
    let bundle = crate::human_authority::startup::HumanOwnedStartupBundle::open(request).unwrap();
    let state = GatewayState::for_owned_human_sources(
        bundle,
        credentials,
        tokens,
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let owned = state.owned_human.as_ref().unwrap();
    let observed_a =
        crate::audit::read_test_observer::observe(state.tenants[0].store.sqlite().unwrap());
    let source = &owned.sources[1];
    let epoch = owned.owner.try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, source).unwrap();
    for _ in 0..2 {
        state.tenants[1]
            .store
            .sqlite()
            .unwrap()
            .modern_human_profile(&access, fixture.principal(), &Redactor::new(vec![]))
            .unwrap();
    }
    let error = state.tenants[1]
        .store
        .sqlite()
        .unwrap()
        .modern_human_profile(&access, fixture.principal(), &Redactor::new(vec![]))
        .err()
        .unwrap();
    assert_eq!(
        error.downcast_ref::<crate::audit::AuditCapacityFailureV1>(),
        Some(&crate::audit::AuditCapacityFailureV1::HumanReadBudgetExhausted)
    );
    drop(epoch);
    let (status, body) = get(
        &gateway_router(state.clone()),
        "/api/v1/human/identity",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "audit_read_budget_exhausted");
    assert_eq!(body["error"]["retry_action"], "contact_operator");
    assert!(body.get("contexts").is_none());
    assert!(observed_a.try_recv().is_err());
}

#[tokio::test]
async fn retained_read_gate_a_does_not_require_global_write_for_b() {
    let (_fixture, state) = fixture();
    let owned = state.owned_human.as_ref().unwrap();
    let epoch = owned.owner.try_read_epoch().unwrap();
    epoch.validate_source(&owned.sources[0]).unwrap();
    assert!(matches!(
        owned.owner.try_write_epoch(),
        Err(crate::human_authority::HumanAuthorityFailure::GateBusy)
    ));
    // This is a concurrency oracle, not a latency test. Keep the real read
    // guard across B requests; the production two-second authority deadline
    // remains unchanged and need not stay valid across several requests.
    require_b_identity_resources(&state).await;
    assert!(matches!(
        owned.owner.try_write_epoch(),
        Err(crate::human_authority::HumanAuthorityFailure::GateBusy)
    ));
    drop(epoch);
    drop(owned.owner.try_write_epoch().unwrap());
}

#[tokio::test]
async fn real_shared_write_gate_refusal_is_separate_from_neighbor_sqlite_availability() {
    let (_fixture, state) = fixture();
    let owned = state.owned_human.as_ref().unwrap();
    let epoch = owned.owner.try_write_epoch().unwrap();
    assert_eq!(
        get(
            &gateway_router(state.clone()),
            "/api/v1/human/identity",
            None
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    drop(epoch);
    require_b_identity_resources(&state).await;
}

#[tokio::test]
async fn modern_on_bound_error_is_preserved_after_real_cleanup() {
    let (_fixture, state) = fixture();
    let server = crate::gateway_server::GatewayServer::new_owned(
        state.control_recovery_retirement(),
        state.human_retirement().unwrap(),
    )
    .unwrap();
    let result = server
        .serve(
            (
                "127.0.0.1:0".parse().unwrap(),
                gateway_router(state.clone()),
            ),
            None,
            futures_util::future::pending(),
            |_, _| anyhow::bail!("synthetic on_bound failure"),
        )
        .await;
    assert!(format!("{:#}", result.unwrap_err()).contains("synthetic on_bound failure"));
    assert!(state.owned_human.as_ref().unwrap().admit().is_err());
}

#[tokio::test]
async fn historical_revoked_neighbor_collision_refuses_even_while_a_sqlite_is_busy() {
    let fixture = Fixture::new_owned();
    fixture
        .legacy_store("A")
        .mutate_identity(&IdentityMutationV1 {
            version: 1,
            operation_id: uuid::Uuid::new_v4().to_string(),
            actor: PrincipalRefV1::LegacyControl {
                credential_id: "synthetic-offline-control".into(),
                role: LegacyControlRoleV1::Runner,
            },
            change: IdentityChangeV1::Credential {
                expected_revision: 0,
                binding: LocalCredentialV1 {
                    version: 1,
                    credential_id: uuid::Uuid::new_v4().to_string(),
                    principal: fixture.principal().clone(),
                    sha256_digest: hex::encode(Sha256::digest(Fixture::bearer("B").as_bytes())),
                    revision: 1,
                    active: false,
                    expires_at: None,
                },
            },
        })
        .unwrap();
    let state = state_from_fixture(&fixture);
    let observed_a =
        crate::audit::read_test_observer::observe(state.tenants[0].store.sqlite().unwrap());
    let observed_b =
        crate::audit::read_test_observer::observe(state.tenants[1].store.sqlite().unwrap());
    let a = rusqlite::Connection::open(
        state.owned_human.as_ref().unwrap().sources[0]
            .descriptor()
            .storage_binding
            .path(),
    )
    .unwrap();
    a.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert_eq!(
        get(&gateway_router(state), "/api/v1/human/identity", None)
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(!a.is_autocommit());
    assert!(observed_a.try_recv().is_err());
    assert!(observed_b.try_recv().is_err());
    a.execute_batch("ROLLBACK").unwrap();
}

#[tokio::test]
async fn real_revoked_and_expired_own_credentials_remain_authentication_refusals() {
    for expired in [false, true] {
        let fixture = Fixture::new_owned();
        let store = fixture.legacy_store("B");
        let digest: [u8; 32] = Sha256::digest(Fixture::bearer("B").as_bytes()).into();
        let mut binding = store.local_credential_binding(&digest).unwrap().unwrap();
        binding.revision += 1;
        binding.active = expired;
        binding.expires_at =
            expired.then(|| (chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339());
        store
            .mutate_identity(&IdentityMutationV1 {
                version: 1,
                operation_id: uuid::Uuid::new_v4().to_string(),
                actor: PrincipalRefV1::LegacyControl {
                    credential_id: "synthetic-offline-control".into(),
                    role: LegacyControlRoleV1::Runner,
                },
                change: IdentityChangeV1::Credential {
                    expected_revision: 1,
                    binding,
                },
            })
            .unwrap();
        let state = state_from_fixture(&fixture);
        let (status, body) = get(&gateway_router(state), "/api/v1/human/identity", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert_eq!(
            body["error"]["code"],
            if expired {
                "credential_expired"
            } else {
                "identity_revoked"
            }
        );
    }
}

async fn get(router: &Router, path: &str, context: Option<&str>) -> (StatusCode, Value) {
    let mut builder = axum::http::Request::get(path)
        .header("authorization", format!("Bearer {}", Fixture::bearer("B")));
    if let Some(context) = context {
        builder = builder.header("x-aiguard-context", context);
    }
    let response = tokio::time::timeout(
        Duration::from_secs(15),
        router.clone().oneshot(builder.body(Body::empty()).unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 262_144)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn require_b_identity_resources(state: &GatewayState) {
    let router = gateway_router(state.clone());
    let (status, identity) = get(&router, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK, "{identity}");
    assert_eq!(identity["contexts_complete"], false);
    assert_eq!(identity["authentication"]["method"], "local_credential");
    let contexts = identity["contexts"].as_array().unwrap();
    assert!(!contexts.is_empty());
    assert!(contexts.iter().all(|context| context["tenant_id"] == "B"));
    let context = contexts[0]["id"].as_str().unwrap();
    let (status, resources) = get(&router, "/api/v1/human/resources", Some(context)).await;
    assert_eq!(status, StatusCode::OK, "{resources}");
    assert!(resources.get("read_receipt").is_some(), "{resources}");
    // A separate real connection observes the committed B receipt while the
    // caller still retains A's mutex/SQLite writer. A JSON UUID is not proof.
    let path = state.owned_human.as_ref().unwrap().sources[1]
        .descriptor()
        .storage_binding
        .path();
    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let receipt = &resources["read_receipt"];
    let (payload, received): (String, String) = connection
        .query_row(
            "SELECT payload,received_at FROM events WHERE id=?1",
            [receipt["audit_event_id"].as_str().unwrap()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let event: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(event["data"]["request_id"], receipt["request_id"]);
    assert_eq!(event["data"]["principal"], identity["principal"]);
    assert_eq!(event["data"]["action"], "resources");
    assert_eq!(received, receipt["received_at"].as_str().unwrap());
    let floor:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM audit_time_floors f JOIN events e ON f.event_seq>=e.seq WHERE e.id=?1)",[receipt["audit_event_id"].as_str().unwrap()],|row|row.get(0)).unwrap();
    assert!(
        floor,
        "committed receipt must retain a real durable wall floor"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_b_proof_and_receipt_while_mutex_a_stays_held_without_neighbor_sql() {
    let (_fixture, state) = fixture();
    let observed =
        crate::audit::read_test_observer::observe(state.tenants[0].store.sqlite().unwrap());
    let (ready, ready_rx) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let a = state.tenants[0].store.clone();
    let held = std::thread::spawn(move || {
        a.sqlite()
            .unwrap()
            .modern_test_hold_mutex(ready, release_rx)
    });
    ready_rx.recv().unwrap();
    require_b_identity_resources(&state).await;
    assert!(observed.try_recv().is_err());
    release.send(()).unwrap();
    held.join().unwrap();
}

#[tokio::test]
async fn real_b_proof_and_receipt_while_sqlite_a_immediate_stays_held_without_neighbor_sql() {
    let (_fixture, state) = fixture();
    let observed =
        crate::audit::read_test_observer::observe(state.tenants[0].store.sqlite().unwrap());
    let path = state.owned_human.as_ref().unwrap().sources[0]
        .descriptor()
        .storage_binding
        .path();
    let a = rusqlite::Connection::open(path).unwrap();
    a.execute_batch("BEGIN IMMEDIATE").unwrap();
    require_b_identity_resources(&state).await;
    assert!(!a.is_autocommit());
    assert!(observed.try_recv().is_err());
    a.execute_batch("ROLLBACK").unwrap();
}

#[tokio::test]
async fn own_sqlite_busy_is_503_without_legacy_or_neighbor_lookup() {
    let (_fixture, state) = fixture();
    let observed =
        crate::audit::read_test_observer::observe(state.tenants[0].store.sqlite().unwrap());
    let path = state.owned_human.as_ref().unwrap().sources[1]
        .descriptor()
        .storage_binding
        .path();
    let b = rusqlite::Connection::open(path).unwrap();
    b.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert_eq!(
        get(
            &gateway_router(state.clone()),
            "/api/v1/human/identity",
            None
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(observed.try_recv().is_err());
    b.execute_batch("ROLLBACK").unwrap();
    require_b_identity_resources(&state).await;
}

#[tokio::test]
async fn modern_route_matrix_closes_before_body_extractors_and_store() {
    let (_fixture, state) = fixture();
    let observed =
        crate::audit::read_test_observer::observe(state.tenants[0].store.sqlite().unwrap());
    let observed_b =
        crate::audit::read_test_observer::observe(state.tenants[1].store.sqlite().unwrap());
    let router = gateway_router(state.clone());
    for (path, methods) in REGISTERED_ROUTES {
        for method in *methods {
            if *method == "GET"
                && [
                    "/api/v1/human/identity",
                    "/api/v1/human/resources",
                    "/api/v1/human/events",
                    "/api/v1/human/events/recent",
                    "/api/v1/human/owned/brokers",
                ]
                .contains(path)
                || *path == "/healthz"
                || *path == "/api/v1/status" && ["GET", "HEAD"].contains(method)
            {
                continue;
            }
            // OIDC selected in the real fixture: even OIDC status is closed.
            let path = path
                .replace("{id}", "synthetic")
                .replace("{create_id}", "synthetic")
                .replace("{operation_id}", "synthetic")
                .replace("{arm_id}", "synthetic");
            let request = axum::http::Request::builder()
                .method(*method)
                .uri(&path)
                .body(Body::from_stream(futures_util::stream::pending::<
                    Result<axum::body::Bytes, std::io::Error>,
                >()))
                .unwrap();
            let response =
                tokio::time::timeout(Duration::from_secs(5), router.clone().oneshot(request))
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{method} {path}"
            );
            assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
            if path == "/api/v1/events/batch" {
                let body = axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap();
                assert_eq!(
                    serde_json::from_slice::<Value>(&body).unwrap()["error"]["code"],
                    "identity_unavailable"
                );
            }
        }
    }
    assert!(observed.try_recv().is_err());
    assert!(observed_b.try_recv().is_err());
    let response = router
        .clone()
        .oneshot(
            axum::http::Request::get("/not-an-existing-route")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = router
        .oneshot(
            axum::http::Request::post("/api/v1/human/identity")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn server_drop_before_serve_keeps_real_accepted_usage_until_completion() {
    let (_fixture, state) = fixture();
    let inner = state.owned_human.as_ref().unwrap().clone();
    let weak = Arc::downgrade(&inner);
    let usage = inner.admit().unwrap();
    let server = crate::gateway_server::GatewayServer::new_owned(
        state.control_recovery_retirement(),
        state.human_retirement().unwrap(),
    )
    .unwrap();
    drop(state);
    drop(inner);
    drop(server);
    tokio::task::yield_now().await;
    assert!(weak.upgrade().is_some());
    assert!(usage.bundle.admit().is_err());
    drop(usage);
    tokio::time::timeout(Duration::from_secs(5), async {
        while weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn modern_server_constructor_without_runtime_refuses_without_panicking() {
    let (_fixture, state) = fixture();
    let result = crate::gateway_server::GatewayServer::new_owned(
        state.control_recovery_retirement(),
        state.human_retirement().unwrap(),
    );
    assert!(result.is_err());
}

#[tokio::test]
async fn modern_second_bind_failure_closes_and_drains_real_bundle() {
    let (_fixture, state) = fixture();
    let server = crate::gateway_server::GatewayServer::new_owned(
        state.control_recovery_retirement(),
        state.human_retirement().unwrap(),
    )
    .unwrap();
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = occupied.local_addr().unwrap();
    let result = server
        .serve(
            (
                "127.0.0.1:0".parse().unwrap(),
                gateway_router(state.clone()),
            ),
            Some((address, gateway_control_router(state.clone()))),
            futures_util::future::pending(),
            |_, _| Ok(()),
        )
        .await;
    assert!(result.is_err());
    assert!(state.owned_human.as_ref().unwrap().admit().is_err());
}

#[tokio::test]
async fn real_configuration_mismatch_and_bind_failure_preserve_both_errors() {
    use std::io::Write;
    let (_fixture, state) = fixture();
    let inner = state.owned_human.as_ref().unwrap();
    let server = crate::gateway_server::GatewayServer::new_owned(
        state.control_recovery_retirement(),
        state.human_retirement().unwrap(),
    )
    .unwrap();
    let path = inner.owner.closure().configuration.files[0]
        .storage_binding
        .path();
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(b"synthetic mismatch")
        .unwrap();
    assert!(
        inner
            .owner
            .source_token(inner.sources[0].source_id())
            .is_err()
    );
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let error = server
        .serve(
            (
                occupied.local_addr().unwrap(),
                gateway_router(state.clone()),
            ),
            None,
            futures_util::future::pending(),
            |_, _| Ok(()),
        )
        .await
        .unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("gateway listener bind failed"),
        "{message}"
    );
    assert!(
        message.contains("human source drain not established"),
        "{message}"
    );
}

#[test]
fn queued_blocking_usage_survives_waiter_abort_and_server_drop_before_epoch() {
    use std::sync::atomic::AtomicBool;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (_fixture, state) = fixture();
        let inner = state.owned_human.as_ref().unwrap().clone();
        let weak = Arc::downgrade(&inner);
        let server = crate::gateway_server::GatewayServer::new_owned(
            state.control_recovery_retirement(),
            state.human_retirement().unwrap(),
        )
        .unwrap();
        let (ready, ready_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            ready.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        ready_rx.recv().unwrap();
        let ran = Arc::new(AtomicBool::new(false));
        let observed = ran.clone();
        let queued_state = state.clone();
        let waiter = tokio::spawn(async move {
            queued_state
                .blocking_identity(move |_| {
                    observed.store(true, Ordering::Release);
                    Ok(())
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while inner.usages.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        waiter.abort();
        assert!(waiter.await.is_err());
        drop(server);
        drop(state);
        drop(inner);
        assert!(weak.upgrade().is_some());
        assert!(!ran.load(Ordering::Acquire));
        release.send(()).unwrap();
        blocker.await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(ran.load(Ordering::Acquire));
    });
}

#[tokio::test]
async fn no_oidc_selection_adopts_only_existing_disabled_status_and_profile() {
    let fixture = Fixture::new_owned_local();
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
    for path in ["/auth/oidc/status", "/auth/oidc/cli-profile"] {
        let (status, body) = get(&router, path, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["enabled"], false);
    }
}

#[tokio::test]
async fn dashboard_head_and_unadopted_get_refuse_before_any_upstream_or_store_work() {
    use std::os::unix::fs::PermissionsExt;
    let (_fixture, state) = fixture();
    let observed =
        crate::audit::read_test_observer::observe(state.tenants[0].store.sqlite().unwrap());
    let front = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    std::fs::write(
        front.path().join("index.html"),
        "synthetic dashboard static page",
    )
    .unwrap();
    // A deliberately unavailable upstream makes a hidden verification/proxy
    // request discriminate from the required in-process coverage refusal.
    let dashboard = dashboard_router_with_human_mode(
        "http://127.0.0.1:9",
        None,
        front.path().to_owned(),
        None,
        true,
    )
    .unwrap();
    for (method, path) in [
        ("HEAD", "/api/v1/human/identity"),
        ("HEAD", "/api/v1/human/resources"),
        ("GET", "/api/v1/overview"),
        ("HEAD", "/auth/oidc/status"),
    ] {
        let response = dashboard
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    }
    assert!(observed.try_recv().is_err());
    assert!(
        dashboard_router_with_human_mode(
            "http://127.0.0.1:9",
            Some("synthetic-viewer".into()),
            front.path().to_owned(),
            None,
            true
        )
        .is_err()
    );
}

#[test]
fn candidates_keep_collisions_across_classes_and_sources() {
    let mut index = CandidateIndex::default();
    index.insert([7; 32], Candidate::Local(1)).unwrap();
    assert_eq!(index.local_source(&[7; 32]).unwrap(), Some(1));
    index.insert([7; 32], Candidate::Local(2)).unwrap();
    assert!(index.local_source(&[7; 32]).is_err());
    let mut index = CandidateIndex::default();
    index.insert([9; 32], Candidate::Local(0)).unwrap();
    index.insert([9; 32], Candidate::Workload(0)).unwrap();
    assert!(index.local_source(&[9; 32]).is_err());
}

#[test]
fn candidate_limit_refuses_before_insertion_and_never_truncates() {
    let mut index = CandidateIndex::default();
    for value in 0..MAX_CANDIDATES {
        let mut digest = [0; 32];
        digest[..8].copy_from_slice(&(value as u64).to_be_bytes());
        index.insert(digest, Candidate::Local(0)).unwrap();
    }
    assert!(index.insert([255; 32], Candidate::Workload(0)).is_err());
    assert_eq!(index.count, MAX_CANDIDATES);
    assert_eq!(index.entries.len(), MAX_CANDIDATES);
}

#[tokio::test]
async fn every_existing_control_alias_and_head_closes_before_body_and_store() {
    let (_fixture, state) = fixture();
    let observed_a =
        crate::audit::read_test_observer::observe(state.tenants[0].store.sqlite().unwrap());
    let observed_b =
        crate::audit::read_test_observer::observe(state.tenants[1].store.sqlite().unwrap());
    let dedicated = gateway_control_router(state.clone());
    let nested = gateway_router(state);
    let routes = &[
        ("GET", "/api/v1/identity"),
        ("GET", "/api/identity"),
        (
            "GET",
            "/api/v1/workers/00000000-0000-4000-8000-000000000001",
        ),
        ("GET", "/api/workers/00000000-0000-4000-8000-000000000001"),
        (
            "GET",
            "/api/v1/workers/operations/00000000-0000-4000-8000-000000000001",
        ),
        (
            "GET",
            "/api/workers/operations/00000000-0000-4000-8000-000000000001",
        ),
        ("POST", "/api/v1/decide"),
        ("POST", "/api/decide"),
        ("POST", "/api/v1/workers/reservations"),
        ("POST", "/api/workers/reservations"),
        (
            "POST",
            "/api/v1/workers/00000000-0000-4000-8000-000000000001/transitions",
        ),
        (
            "POST",
            "/api/workers/00000000-0000-4000-8000-000000000001/transitions",
        ),
        ("GET", "/api/v1/control/protocol"),
        (
            "GET",
            "/api/v1/workers/00000000-0000-4000-8000-000000000001/arms/00000000-0000-4000-8000-000000000001",
        ),
        ("POST", "/api/v1/control/events"),
        ("POST", "/api/v1/workers/supervised-reservations"),
        (
            "POST",
            "/api/v1/workers/00000000-0000-4000-8000-000000000001/arms",
        ),
        (
            "POST",
            "/api/v1/workers/00000000-0000-4000-8000-000000000001/armed-start-claims",
        ),
        (
            "POST",
            "/api/v1/workers/00000000-0000-4000-8000-000000000001/helper-start-claims",
        ),
        (
            "POST",
            "/api/v1/workers/00000000-0000-4000-8000-000000000001/helper-observations",
        ),
        (
            "POST",
            "/api/v1/workers/00000000-0000-4000-8000-000000000001/helper-cleanup",
        ),
        (
            "POST",
            "/api/v1/workers/00000000-0000-4000-8000-000000000001/export-requests",
        ),
        ("POST", "/api/v1/workload-bindings"),
        (
            "POST",
            "/api/v1/workload-bindings/00000000-0000-4000-8000-000000000001/revoke",
        ),
        ("POST", "/api/v1/brokers/grants"),
        (
            "POST",
            "/api/v1/brokers/00000000-0000-4000-8000-000000000001/revoke",
        ),
        ("POST", "/api/v1/relay/grants"),
        (
            "POST",
            "/api/v1/relay/grants/00000000-0000-4000-8000-000000000001/revoke",
        ),
        (
            "POST",
            "/api/v1/relay/messages/00000000-0000-4000-8000-000000000001/prune",
        ),
        ("POST", "/api/v1/relay/channels/admissions"),
        (
            "POST",
            "/api/v1/relay/channels/00000000-0000-4000-8000-000000000001/claims",
        ),
        (
            "POST",
            "/api/v1/relay/channels/00000000-0000-4000-8000-000000000001/transitions",
        ),
        (
            "POST",
            "/api/v1/kubernetes/session-reservations/00000000-0000-4000-8000-000000000001/control-recovery",
        ),
        (
            "POST",
            "/api/v1/relay/channels/00000000-0000-4000-8000-000000000001/reconcile",
        ),
        ("GET", "/api/v1/brokers/identity"),
        ("GET", "/api/v1/brokers/self"),
        ("POST", "/api/v1/brokers/heartbeat"),
        ("POST", "/api/v1/brokers/checkpoints"),
        ("GET", "/api/v1/brokers/checkpoints/self"),
        ("POST", "/api/v1/relay/messages"),
        ("GET", "/api/v1/relay/messages"),
        (
            "POST",
            "/api/v1/relay/messages/00000000-0000-4000-8000-000000000001/ack",
        ),
    ];
    for (method, path) in routes {
        for (router, prefix) in [(&dedicated, ""), (&nested, "/control")] {
            let methods = if *method == "GET" {
                vec!["GET", "HEAD"]
            } else {
                vec![*method]
            };
            for method in methods {
                let request = axum::http::Request::builder()
                    .method(method)
                    .uri(format!("{prefix}{path}"))
                    .body(Body::from_stream(futures_util::stream::pending::<
                        Result<axum::body::Bytes, std::io::Error>,
                    >()))
                    .unwrap();
                let response = router.clone().oneshot(request).await.unwrap();
                assert_eq!(
                    response.status(),
                    StatusCode::SERVICE_UNAVAILABLE,
                    "{method} {prefix}{path}"
                );
            }
        }
    }
    assert!(observed_a.try_recv().is_err());
    assert!(observed_b.try_recv().is_err());
}

#[test]
fn workload_discovery_retains_its_nonzero_source_index_without_neighbor_sql() {
    let fixture = Fixture::new_owned();
    let (bundle, credentials, tokens) = fixture.owned_startup();
    let source_id = bundle
        .owner
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some("B"))
        .unwrap()
        .source_id;
    let digest = [0x73; 32];
    {
        let source = bundle.owner.source_token(source_id).unwrap();
        let epoch = bundle.owner.try_write_epoch().unwrap();
        let access = HumanSourceAccess::write(&epoch, &source).unwrap();
        bundle
            .stores
            .get(&source_id)
            .unwrap()
            .issue_workload_binding_scoped(
                &access,
                &crate::workload::WorkloadBindingIssueV1 {
                    version: 1,
                    operation_id: uuid::Uuid::new_v4().to_string(),
                    credential_id: uuid::Uuid::new_v4().to_string(),
                    expected_revision: 0,
                    subject: crate::workload::WorkloadSubjectV1::Broker {
                        broker_id: uuid::Uuid::new_v4().to_string(),
                        incarnation_id: uuid::Uuid::new_v4().to_string(),
                    },
                    sha256_digest: hex::encode(digest),
                    expires_at: (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
                },
            )
            .unwrap();
        epoch.check().unwrap();
    }
    // Discovery is built after the genuine workload mutation; it is not a
    // stale-generation fixture or a hand-filled candidate index.
    let state = GatewayState::for_owned_human_sources(
        bundle,
        credentials,
        tokens,
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let expected = state
        .tenants
        .iter()
        .position(|tenant| tenant.tenant_id == "B")
        .unwrap();
    assert!(expected > 0, "fixture must exercise a nonzero source index");
    let observers = state
        .tenants
        .iter()
        .map(|tenant| crate::audit::read_test_observer::observe(tenant.store.sqlite().unwrap()))
        .collect::<Vec<_>>();
    let owned = state.owned_human.as_ref().unwrap();
    assert!(matches!(
        owned.candidates.entries.get(&digest).map(Vec::as_slice),
        Some([Candidate::Workload(index)]) if *index == expected
    ));
    assert_eq!(owned.broker_source(&digest).unwrap(), expected);
    assert_eq!(owned.source(&digest).unwrap(), None);
    assert!(!owned.technical_candidate(&digest).unwrap());
    assert!(
        observers
            .iter()
            .all(|observer| observer.try_recv().is_err())
    );
}

#[tokio::test]
async fn owned_broker_page_requires_human_auth_while_legacy_and_head_stay_closed() {
    let (_fixture, owned) = fixture();
    let (_legacy_files, legacy, _legacy_token) =
        crate::api::read_execution_tests::local_identity_state();
    assert!(owned.owned_human.is_some());
    assert!(legacy.owned_human.is_none());
    let observers = owned
        .tenants
        .iter()
        .chain(legacy.tenants.iter())
        .map(|tenant| crate::audit::read_test_observer::observe(tenant.store.sqlite().unwrap()))
        .collect::<Vec<_>>();
    let owned_router = gateway_router(owned);
    let legacy_router = gateway_router(legacy);
    for (router, method, path, status, code) in [
        (
            &owned_router,
            "GET",
            "/api/v1/human/owned/brokers",
            StatusCode::UNAUTHORIZED,
            Some("authentication_required"),
        ),
        (
            &owned_router,
            "HEAD",
            "/api/v1/human/owned/brokers",
            StatusCode::SERVICE_UNAVAILABLE,
            None,
        ),
        (
            &owned_router,
            "GET",
            "/api/v1/human/brokers",
            StatusCode::SERVICE_UNAVAILABLE,
            Some("identity_unavailable"),
        ),
        (
            &legacy_router,
            "GET",
            "/api/v1/human/owned/brokers",
            StatusCode::SERVICE_UNAVAILABLE,
            Some("identity_unavailable"),
        ),
    ] {
        let polls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&polls);
        let stream = futures_util::stream::poll_fn(move |_| {
            observed.fetch_add(1, Ordering::AcqRel);
            std::task::Poll::Pending::<Option<Result<axum::body::Bytes, std::io::Error>>>
        });
        // No bearer: the owned GET must reach the existing human extractor,
        // while the legacy path/profile and HEAD refuse before that extractor.
        let request = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .body(Body::from_stream(stream))
            .unwrap();
        let response =
            tokio::time::timeout(Duration::from_secs(5), router.clone().oneshot(request))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(response.status(), status, "{method} {path}");
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
        assert_eq!(polls.load(Ordering::Acquire), 0, "{method} {path}");
        if let Some(code) = code {
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["error"]["code"], code, "{method} {path}");
            assert!(body.get("items").is_none() && body.get("read_receipt").is_none());
        }
    }
    assert!(
        observers
            .iter()
            .all(|observer| observer.try_recv().is_err())
    );
}

#[path = "recent_tests.rs"]
mod recent_contract_tests;

#[path = "browser_fixture.rs"]
#[cfg(test)]
mod owned_browser_fixture;
