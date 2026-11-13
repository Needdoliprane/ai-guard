use super::*;
use super::races::{connection, context, snapshot, state};

#[tokio::test]
async fn owned_events_metadata_cursor_is_query_bound() {
    // Removing context/query binding or ACL-before-pagination would expose a
    // different selection or skip/repeat an authorized event.
    let fixture = Fixture::new_owned();
    let first = seed_event(&fixture, "B", "owned-page-first-b", fixture.principal());
    let second = seed_event(&fixture, "B", "owned-page-second-b", fixture.principal());
    let state = state(&fixture);
    let router = gateway_router(state);
    let context = context(&router).await;
    let (status, page) = get(&router, "/api/v1/human/events?limit=1", Some(&context)).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["metadata"]["id"], first);
    assert_eq!(page["has_more"], true);
    let cursor = page["next_cursor"].as_str().unwrap();
    let next = format!("/api/v1/human/events?limit=1&cursor={cursor}");
    let (status, page) = get(&router, &next, Some(&context)).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["metadata"]["id"], second);
    assert_eq!(page["has_more"], false);
    assert!(page["next_cursor"].is_null());
    for path in [
        format!("/api/v1/human/events?limit=2&cursor={cursor}"),
        format!("/api/v1/human/events?limit=1&resource_kind=session&resource_id=owned-page-first-b&cursor={cursor}"),
    ] {
        let (status, body) = get(&router, &path, Some(&context)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["error"]["code"], "invalid_cursor");
        assert!(body.get("items").is_none());
    }
    let a = router.clone().oneshot(
        axum::http::Request::get("/api/v1/human/identity")
            .header("authorization", format!("Bearer {}", Fixture::bearer("A")))
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(a.status(), StatusCode::OK);
    let a: Value = serde_json::from_slice(
        &axum::body::to_bytes(a.into_body(), 262_144).await.unwrap()
    ).unwrap();
    let a_context = a["contexts"].as_array().unwrap().iter()
        .find(|context| context["tenant_id"] == "A").unwrap()["id"].as_str().unwrap();
    let (status, wrong_context) = get(&router, &next, Some(a_context)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{wrong_context}");
    assert_eq!(wrong_context["error"]["code"], "context_invalid");
    assert!(wrong_context.get("items").is_none());
    let (status, filtered) = get(
        &router,
        "/api/v1/human/events?resource_kind=session&resource_id=owned-page-second-b",
        Some(&context),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{filtered}");
    assert_eq!(filtered["items"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["items"][0]["metadata"]["id"], second);
}

#[tokio::test]
async fn owned_events_metadata_refuses_unadopted_views_and_routes() {
    // Widening the modern route/filter gate would admit unsupported content,
    // detail, live or export paths before their owned contracts exist.
    let fixture = Fixture::new_owned();
    let event_id = seed_event(&fixture, "B", "owned-closed-b", fixture.principal());
    let state = state(&fixture);
    let router = gateway_router(state.clone());
    let context = context(&router).await;
    let database = connection(&state);
    let before = snapshot(&database);
    let (status, body) = get(
        &router,
        "/api/v1/human/events?view=content",
        Some(&context),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["code"], "identity_unavailable");
    assert!(body.get("items").is_none());
    assert_eq!(snapshot(&database), before);
    let observed = crate::audit::read_test_observer::observe(&state.tenants[1].store);
    for (method, path) in [
        ("HEAD", "/api/v1/human/events".to_owned()),
        ("GET", "/api/v1/human/events/recent".to_owned()),
        ("GET", format!("/api/v1/human/events/{event_id}")),
        ("GET", "/api/v1/human/export".to_owned()),
        ("GET", "/api/v1/human/stream".to_owned()),
    ] {
        let request = axum::http::Request::builder()
            .method(method)
            .uri(&path)
            .header("authorization", format!("Bearer {}", Fixture::bearer("B")))
            .header("x-aiguard-context", &context)
            .body(Body::from_stream(futures_util::stream::pending::<Result<axum::body::Bytes, std::io::Error>>()))
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), router.clone().oneshot(request))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{method} {path}");
    }
    assert!(observed.try_recv().is_err());
    assert_eq!(snapshot(&database), before);
    assert_eq!(get(&router, "/api/v1/human/events", None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        get(&router, "/api/v1/human/events?resource_kind=session", Some(&context)).await.0,
        StatusCode::BAD_REQUEST
    );
    let response = router.clone().oneshot(
        axum::http::Request::post("/api/v1/human/events").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    let response = router.oneshot(
        axum::http::Request::get("/api/v1/human/not-an-existing-route").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn owned_events_metadata_does_not_query_neighbor_while_neighbor_locked() {
    let fixture = Fixture::new_owned();
    seed_event(&fixture, "B", "owned-contention-b", fixture.principal());
    let state = state(&fixture);
    let router = gateway_router(state.clone());
    let context = context(&router).await;
    let observed = crate::audit::read_test_observer::observe(&state.tenants[0].store);
    let neighbor = rusqlite::Connection::open(
        state.owned_human.as_ref().unwrap().sources[0].descriptor().storage_binding.path()
    ).unwrap();
    neighbor.execute_batch("BEGIN IMMEDIATE").unwrap();
    // Connection Drop rolls back even if an assertion fails below.
    let (status, page) = get(&router, "/api/v1/human/events", Some(&context)).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let database = connection(&state);
    let receipt: bool = database.query_row(
        "SELECT EXISTS(SELECT 1 FROM events WHERE id=?1 AND json_extract(payload,'$.data.action')='events')",
        [page["read_receipt"]["audit_event_id"].as_str().unwrap()],
        |row| row.get(0),
    ).unwrap();
    assert!(receipt);
    assert!(!neighbor.is_autocommit());
    assert!(observed.try_recv().is_err());
    neighbor.execute_batch("ROLLBACK").unwrap();

    let own = rusqlite::Connection::open(
        state.owned_human.as_ref().unwrap().sources[1].descriptor().storage_binding.path()
    ).unwrap();
    own.execute_batch("BEGIN IMMEDIATE").unwrap();
    let before = snapshot(&database);
    let (status, refused) = get(&router, "/api/v1/human/events", Some(&context)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{refused}");
    assert!(refused.get("items").is_none());
    assert!(refused.get("read_receipt").is_none());
    assert_eq!(snapshot(&database), before);
    assert!(state.owned_human.as_ref().unwrap().healthy());
    assert!(observed.try_recv().is_err());
    own.execute_batch("ROLLBACK").unwrap();
    assert_eq!(get(&router, "/api/v1/human/events", Some(&context)).await.0, StatusCode::OK);
}

#[tokio::test]
async fn owned_events_metadata_budget_exhaustion_is_429_without_publication() {
    let fixture = Fixture::new_owned();
    seed_event(&fixture, "B", "owned-budget-b", fixture.principal());
    let (mut request, credentials, tokens) = fixture.owned_request();
    for options in request.sources.values_mut() {
        if let crate::human_authority::startup::HumanOwnedSourceOptions::TenantStore { capacity, .. } = options {
            capacity.human_tenant.events = 4;
            capacity.human_principal.events = 2;
        }
    }
    let bundle = crate::human_authority::startup::HumanOwnedStartupBundle::open(request).unwrap();
    let state = GatewayState::for_owned_human_sources(
        bundle, credentials, tokens, Policy::default(), None, Redactor::new(vec![])
    ).unwrap();
    let router = gateway_router(state.clone());
    let context = context(&router).await; // First actual human read debit.
    let (status, page) = get(&router, "/api/v1/human/events", Some(&context)).await;
    assert_eq!(status, StatusCode::OK, "{page}"); // Second and last principal debit.
    let database = connection(&state);
    let before = snapshot(&database);
    let usage = state.tenants[1].store.modern_test_human_usage();
    let (status, refused) = get(&router, "/api/v1/human/events", Some(&context)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
    assert_eq!(refused["error"]["code"], "audit_read_budget_exhausted");
    assert!(refused.get("items").is_none());
    assert!(refused.get("read_receipt").is_none());
    assert_eq!(snapshot(&database), before);
    assert_eq!(state.tenants[1].store.modern_test_human_usage(), usage);
}
