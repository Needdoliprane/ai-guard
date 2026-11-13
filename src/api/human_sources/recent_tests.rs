//! Test-only baseline: existing owned startup, context and real router.
//! No future application API is referenced. Execution is NOT_RUN until Root QA.
use super::*;

#[tokio::test]
async fn owned_recent_metadata_returns_snapshot_only_after_a_real_read_commit() {
    let (_fixture, state) = fixture();
    let router = gateway_router(state.clone());
    let (status, identity) = get(&router, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK, "{identity}");
    let context = identity["contexts"][0]["id"].as_str().unwrap();
    let (status, page) = get(
        &router,
        "/api/v1/human/events/recent?limit=1&view=metadata",
        Some(context),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["version"], 1);
    assert_eq!(page["view"], "metadata");
    assert_eq!(page["order"], "newest_first");
    assert_eq!(page["coverage"], "authorized_selection");
    assert_eq!(page["stream_mode"], "snapshot_only");
    assert!(page["live_cursor"].is_null());
    assert!(
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["content"].is_null())
    );
    let path = state.owned_human.as_ref().unwrap().sources[1]
        .descriptor()
        .storage_binding
        .path();
    let connection = rusqlite::Connection::open_with_flags(
        path,
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
    let event: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(event["data"]["action"], "events");
    assert_eq!(event["data"]["request_id"], receipt["request_id"]);
    assert_eq!(event["data"]["principal"], identity["principal"]);
    assert_eq!(received, receipt["received_at"].as_str().unwrap());
    let floor: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM audit_time_floors f JOIN events e ON f.event_seq>=e.seq WHERE e.id=?1)",
            [receipt["audit_event_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(floor, "the published receipt must have a committed time floor");
}

// Additional parents use only application APIs present on120b. The original
// baseline above is deliberately unchanged. All execution remains NOT_RUN.
mod extended {
    use super::*;
    use crate::model::Event;
    use std::collections::BTreeSet;

    const RECENT: &str = "/api/v1/human/events/recent";

    fn change(change: IdentityChangeV1) -> IdentityMutationV1 {
        IdentityMutationV1 {
            version: 1,
            operation_id: uuid::Uuid::new_v4().to_string(),
            actor: PrincipalRefV1::LegacyControl {
                credential_id: "synthetic-recent-fixture-control".into(),
                role: LegacyControlRoleV1::Runner,
            },
            change,
        }
    }

    fn grant(
        fixture: &Fixture,
        tenant: &str,
        resource: &str,
        owner: &PrincipalRefV1,
        shared: bool,
    ) {
        fixture
            .legacy_store(tenant)
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
                    shares: if shared {
                        vec![ResourceShareV1 {
                            principal: fixture.principal().clone(),
                            permissions: BTreeSet::from([ResourcePermissionV1::Metadata]),
                        }]
                    } else {
                        Vec::new()
                    },
                },
            }))
            .unwrap();
    }

    fn append(fixture: &Fixture, tenant: &str, resource: &str) -> String {
        let mut event = Event::new("fixture.owned-recent");
        event.session_id = Some(resource.into());
        event.data = json!({"private_note":"synthetic-recent-content-must-stay-private"});
        fixture.legacy_store(tenant).append(event).unwrap().event.id
    }

    fn seeded(administration: bool) -> (Fixture, Vec<String>, Vec<String>) {
        let fixture = Fixture::new_owned();
        let PrincipalRefV1::HumanLocal { installation_id, .. } = fixture.principal() else {
            panic!("fixture is a local identity");
        };
        let other = PrincipalRefV1::HumanLocal {
            installation_id: installation_id.clone(),
            account_id: uuid::Uuid::new_v4().to_string(),
        };
        let store = fixture.legacy_store("B");
        store
            .mutate_identity(&change(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: other.clone(),
                    display_name: "Synthetic recent other owner".into(),
                    revision: 1,
                    active: true,
                },
            }))
            .unwrap();
        // Resource ownership requires a real active organization membership,
        // even when the other owner has no capability or reader credential.
        store
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
                    capabilities: BTreeSet::new(),
                },
            }))
            .unwrap();
        if administration {
            let mut membership = store.identity_memberships(fixture.principal()).unwrap().remove(0);
            let expected_revision = membership.revision;
            membership.revision += 1;
            membership.capabilities.insert(HumanCapabilityV1::ReadTenantMetadata);
            store
                .mutate_identity(&change(IdentityChangeV1::Membership {
                    expected_revision,
                    membership,
                }))
                .unwrap();
        }
        grant(&fixture, "B", "recent-owned-b", fixture.principal(), false);
        grant(&fixture, "B", "recent-shared-b", &other, true);
        grant(&fixture, "B", "recent-hidden-b", &other, false);
        grant(&fixture, "A", "recent-foreign-a", fixture.principal(), false);
        let owned_old = append(&fixture, "B", "recent-owned-b");
        let shared = append(&fixture, "B", "recent-shared-b");
        let hidden_old = append(&fixture, "B", "recent-hidden-b");
        let owned_new = append(&fixture, "B", "recent-owned-b");
        let hidden_new = append(&fixture, "B", "recent-hidden-b");
        let foreign = append(&fixture, "A", "recent-foreign-a");
        let visible = if administration {
            vec![hidden_new, owned_new, hidden_old, shared, owned_old]
        } else {
            vec![owned_new, shared, owned_old]
        };
        (fixture, visible, vec![foreign])
    }

    fn state(fixture: &Fixture) -> GatewayState {
        state_from_fixture(fixture)
    }

    async fn context(router: &Router, mode: &str) -> String {
        let (status, identity) = get(router, "/api/v1/human/identity", None).await;
        assert_eq!(status, StatusCode::OK, "{identity}");
        identity["contexts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|context| context["tenant_id"] == "B" && context["mode"] == mode)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn ids(page: &Value) -> Vec<String> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["metadata"]["id"].as_str().unwrap().to_owned())
            .collect()
    }

    fn connection(state: &GatewayState) -> rusqlite::Connection {
        rusqlite::Connection::open_with_flags(
            state.owned_human.as_ref().unwrap().sources[1]
                .descriptor()
                .storage_binding
                .path(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .unwrap()
    }

    fn events_receipts(database: &rusqlite::Connection) -> i64 {
        database
            .query_row(
                "SELECT COUNT(*) FROM events WHERE kind='control.identity.access' AND json_extract(payload,'$.data.action')='events'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[tokio::test]
    async fn owned_recent_owner_share_and_admin_apply_acl_before_descending_limit() {
        for administration in [false, true] {
            let (fixture, expected, foreign) = seeded(administration);
            let state = state(&fixture);
            let neighbor = crate::audit::read_test_observer::observe(&state.tenants[0].store);
            let router = gateway_router(state.clone());
            let context = context(&router, if administration { "administration" } else { "personal" }).await;
            let mut selection = format!("{RECENT}?view=metadata&limit=1");
            let mut actual = Vec::new();
            for _ in 0..8 {
                let (status, page) = get(&router, &selection, Some(&context)).await;
                assert_eq!(status, StatusCode::OK, "{page}");
                assert_eq!(page["stream_mode"], "snapshot_only");
                assert!(page["live_cursor"].is_null());
                assert!(page["items"].as_array().unwrap().iter().all(|item| item["content"].is_null()));
                assert!(!page.to_string().contains("synthetic-recent-content-must-stay-private"));
                actual.extend(ids(&page));
                if let Some(cursor) = page["next_cursor"].as_str() {
                    assert_eq!(page["has_more"], true);
                    selection = format!("{RECENT}?view=metadata&limit=1&cursor={cursor}");
                } else {
                    assert_eq!(page["has_more"], false);
                    break;
                }
            }
            assert_eq!(actual, expected, "hidden events must not consume the SQL limit");
            assert!(foreign.iter().all(|id| !actual.contains(id)));
            assert_eq!(events_receipts(&connection(&state)), i64::try_from(expected.len()).unwrap());
            assert!(neighbor.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn owned_recent_older_cursor_is_immutable_and_bound_to_limit_resource_and_context() {
        let (fixture, all, _) = seeded(true);
        let expected = vec![all[1].clone(), all[3].clone(), all[4].clone()];
        let state = state(&fixture);
        let router = gateway_router(state.clone());
        let context = context(&router, "personal").await;
        let (status, first) = get(&router, &format!("{RECENT}?view=metadata&limit=1"), Some(&context)).await;
        assert_eq!(status, StatusCode::OK, "{first}");
        assert_eq!(ids(&first), expected[..1]);
        let cursor = first["next_cursor"].as_str().unwrap();
        let older = format!("{RECENT}?view=metadata&limit=1&cursor={cursor}");
        let (status, second) = get(&router, &older, Some(&context)).await;
        assert_eq!(status, StatusCode::OK, "{second}");
        assert_eq!(ids(&second), expected[1..2]);
        // The intervening first-page read commits another real events receipt.
        // No unavailable owned business writer is invented to append a fixture.
        let (status, refreshed) = get(&router, &format!("{RECENT}?view=metadata&limit=1"), Some(&context)).await;
        assert_eq!(status, StatusCode::OK, "{refreshed}");
        let (status, repeated) = get(&router, &older, Some(&context)).await;
        assert_eq!(status, StatusCode::OK, "{repeated}");
        assert_eq!(repeated["items"], second["items"]);
        assert_eq!(repeated["has_more"], second["has_more"]);
        assert!(repeated["live_cursor"].is_null());
        for query in [
            format!("{RECENT}?view=metadata&limit=2&cursor={cursor}"),
            format!("{RECENT}?view=metadata&limit=1&resource_kind=session&resource_id=recent-shared-b&cursor={cursor}"),
        ] {
            let (status, refused) = get(&router, &query, Some(&context)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
            assert_eq!(refused["error"]["code"], "invalid_cursor");
            assert!(refused.get("items").is_none());
        }
        // An unchanged personal binding reuses its registered context. Use
        // the genuinely distinct administration context, not a second call
        // that would merely obtain the same personal id again.
        let another = self::context(&router, "administration").await;
        assert_ne!(context, another);
        let (status, refused) = get(&router, &older, Some(&another)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert_eq!(refused["error"]["code"], "invalid_cursor");
        let filtered = format!("{RECENT}?view=metadata&limit=10&resource_kind=session&resource_id=recent-shared-b");
        let (status, filtered) = get(&router, &filtered, Some(&another)).await;
        assert_eq!(status, StatusCode::OK, "{filtered}");
        assert_eq!(ids(&filtered), expected[1..2]);
    }

    #[tokio::test]
    async fn owned_recent_keeps_content_detail_export_stream_head_and_unknown_contracts_closed() {
        let (fixture, expected, _) = seeded(false);
        let state = state(&fixture);
        let router = gateway_router(state.clone());
        let context = context(&router, "personal").await;
        let (status, positive) = get(&router, RECENT, Some(&context)).await;
        assert_eq!(status, StatusCode::OK, "{positive}");
        let database = connection(&state);
        let before = events_receipts(&database);
        for path in [
            format!("{RECENT}?view=content"),
            format!("/api/v1/human/events/{}?view=metadata", expected[0]),
            "/api/v1/human/export?view=metadata".into(),
            "/api/v1/human/stream?view=metadata".into(),
        ] {
            let (status, refused) = get(&router, &path, Some(&context)).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}: {refused}");
            assert!(refused.get("items").is_none());
            assert!(refused.get("stream_mode").is_none());
        }
        for (method, path, expected_status) in [
            ("HEAD", RECENT, StatusCode::SERVICE_UNAVAILABLE),
            ("POST", RECENT, StatusCode::METHOD_NOT_ALLOWED),
            ("GET", "/api/v1/human/does-not-exist", StatusCode::NOT_FOUND),
        ] {
            let response = router.clone().oneshot(
                axum::http::Request::builder().method(method).uri(path)
                    .header("authorization", format!("Bearer {}", Fixture::bearer("B")))
                    .header("x-aiguard-context", &context).body(Body::empty()).unwrap(),
            ).await.unwrap();
            assert_eq!(response.status(), expected_status, "{method} {path}");
        }
        let (status, refused) = get(&router, &format!("{RECENT}?event_id={}", expected[0]), Some(&context)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert_eq!(refused["error"]["code"], "invalid_request");
        assert_eq!(events_receipts(&database), before, "closed selectors must not append an events receipt");
    }

    struct HeldMutex {
        release: Option<std::sync::mpsc::Sender<()>>,
        thread: Option<std::thread::JoinHandle<()>>,
    }
    impl HeldMutex {
        fn acquire(store: Arc<Store>) -> Self {
            let (ready, reached) = std::sync::mpsc::channel();
            let (release, resume) = std::sync::mpsc::channel();
            let thread = std::thread::spawn(move || store.modern_test_hold_mutex(ready, resume));
            reached.recv_timeout(Duration::from_secs(5)).unwrap();
            Self { release: Some(release), thread: Some(thread) }
        }
    }
    impl Drop for HeldMutex {
        fn drop(&mut self) {
            if let Some(release) = self.release.take() { let _ = release.send(()); }
            if let Some(thread) = self.thread.take() { let _ = thread.join(); }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn owned_recent_b_commits_while_real_neighbor_a_mutex_and_sqlite_stay_locked() {
        let (fixture, expected, _) = seeded(false);
        let state = state(&fixture);
        let router = gateway_router(state.clone());
        let context = context(&router, "personal").await;
        let neighbor = crate::audit::read_test_observer::observe(&state.tenants[0].store);
        let held = HeldMutex::acquire(state.tenants[0].store.clone());
        let path = state.owned_human.as_ref().unwrap().sources[0].descriptor().storage_binding.path();
        let a = rusqlite::Connection::open(path).unwrap();
        a.execute_batch("BEGIN IMMEDIATE").unwrap();
        let (status, page) = get(&router, &format!("{RECENT}?limit=10"), Some(&context)).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(ids(&page), expected);
        assert!(!a.is_autocommit());
        assert_eq!(events_receipts(&connection(&state)), 1);
        assert!(neighbor.try_recv().is_err());
        a.execute_batch("ROLLBACK").unwrap();
        drop(held);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn owned_recent_own_mutex_and_sqlite_busy_refuse_then_recover_without_neighbor_lookup() {
        let (fixture, _, _) = seeded(false);
        let state = state(&fixture);
        let router = gateway_router(state.clone());
        let context = context(&router, "personal").await;
        let neighbor = crate::audit::read_test_observer::observe(&state.tenants[0].store);
        let held = HeldMutex::acquire(state.tenants[1].store.clone());
        let (status, refused) = get(&router, RECENT, Some(&context)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{refused}");
        assert!(refused.get("items").is_none());
        drop(held);
        let path = state.owned_human.as_ref().unwrap().sources[1].descriptor().storage_binding.path();
        let b = rusqlite::Connection::open(path).unwrap();
        b.execute_batch("BEGIN IMMEDIATE").unwrap();
        let (status, refused) = get(&router, RECENT, Some(&context)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{refused}");
        assert!(!b.is_autocommit());
        assert!(refused.get("items").is_none());
        b.execute_batch("ROLLBACK").unwrap();
        let (status, recovered) = get(&router, RECENT, Some(&context)).await;
        assert_eq!(status, StatusCode::OK, "{recovered}");
        assert_eq!(events_receipts(&connection(&state)), 1);
        assert!(neighbor.try_recv().is_err());
        // These Router refusals may be authentication admission; the separate
        // Store characterization must not be credited from this test.
    }

    #[tokio::test]
    async fn owned_recent_exhausted_real_read_budget_returns_429_without_page_or_extra_receipt() {
        let (fixture, _, _) = seeded(false);
        let (mut request, credentials, tokens) = fixture.owned_request();
        for options in request.sources.values_mut() {
            if let crate::human_authority::startup::HumanOwnedSourceOptions::TenantStore { capacity, .. } = options {
                capacity.human_tenant.events = 4;
                capacity.human_principal.events = 2;
            }
        }
        let bundle = crate::human_authority::startup::HumanOwnedStartupBundle::open(request).unwrap();
        let state = GatewayState::for_owned_human_sources(bundle, credentials, tokens, Policy::default(), None, Redactor::new(vec![])).unwrap();
        let router = gateway_router(state.clone());
        let context = context(&router, "personal").await;
        let (status, positive) = get(&router, RECENT, Some(&context)).await;
        assert_eq!(status, StatusCode::OK, "{positive}");
        let database = connection(&state);
        let before = events_receipts(&database);
        assert_eq!(before, 1);
        let (status, refused) = get(&router, RECENT, Some(&context)).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
        assert_eq!(refused["error"]["code"], "audit_read_budget_exhausted");
        assert_eq!(refused["error"]["retry_action"], "contact_operator");
        for field in ["items", "read_receipt", "next_cursor", "live_cursor", "stream_mode"] {
            assert!(refused.get(field).is_none(), "{field}");
        }
        assert_eq!(events_receipts(&database), before);
    }
    // Root composes the actual Journal cfg(test) hook. The recent handler
    // pauses after the real read returns, before final proof/context checks.
    mod publication_races {
        use super::*;

        async fn committed_race(revoke: bool) {
            let fixture = Fixture::new_owned();
            grant(&fixture, "B", "recent-postcommit-b", fixture.principal(), false);
            append(&fixture, "B", "recent-postcommit-b");
            let state = state(&fixture);
            let router = gateway_router(state.clone());
            let context = context(&router, "personal").await;
            let database = connection(&state);
            let before: i64 = database.query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |row| row.get(0)).unwrap();
            let (reached, release) = human::event_read_test_hook::install(&state);
            let request = tokio::spawn(async move { get(&router, RECENT, Some(&context)).await });
            tokio::task::spawn_blocking(move || reached.recv_timeout(Duration::from_secs(15))).await.unwrap().unwrap();
            let (audit_id, payload): (String, String) = database.query_row(
                "SELECT id,payload FROM events WHERE seq>?1 AND kind='control.identity.access' AND json_extract(payload,'$.data.action')='events'",
                [before], |row| Ok((row.get(0)?, row.get(1)?)),
            ).unwrap();
            let audit: Value = serde_json::from_str(&payload).unwrap();
            assert_eq!(audit["data"]["result_count"], 1);
            assert_eq!(audit["data"]["principal"], serde_json::to_value(fixture.principal()).unwrap());
            let floor: bool = database.query_row(
                "SELECT EXISTS(SELECT 1 FROM audit_time_floors f JOIN events e ON f.event_seq>=e.seq WHERE e.id=?1)",
                [&audit_id], |row| row.get(0),
            ).unwrap();
            assert!(floor, "the pause must follow a real committed read receipt");
            if revoke {
                // Acquiring the write epoch also proves no read epoch is kept
                // through the postcommit pause. This existing scoped writer
                // is a test intervention, not an adopted public write route.
                let owned = state.owned_human.as_ref().unwrap();
                let epoch = owned.owner.try_write_epoch().unwrap();
                let access = HumanSourceAccess::write(&epoch, &owned.sources[1]).unwrap();
                let digest: [u8; 32] = Sha256::digest(Fixture::bearer("B").as_bytes()).into();
                let mut binding = state.tenants[1].store.local_credential_binding_scoped(&access, &digest).unwrap().unwrap();
                let expected_revision = binding.revision;
                binding.revision += 1; binding.active = false;
                state.tenants[1].store.mutate_identity_scoped(
                    &epoch, &owned.sources[1], &change(IdentityChangeV1::Credential { expected_revision, binding }),
                ).unwrap();
                drop(epoch);
            } else { state.human.close_contexts(); }
            drop(release); // Journal guard also releases on panic/unwind.
            let (status, refused) = tokio::time::timeout(Duration::from_secs(15), request).await.unwrap().unwrap();
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{refused}");
            assert_eq!(refused["error"]["code"], "identity_unavailable");
            for field in ["items", "read_receipt", "next_cursor", "live_cursor", "stream_mode"] {
                assert!(refused.get(field).is_none(), "{field}");
            }
            let retained: String = database.query_row("SELECT payload FROM events WHERE id=?1", [&audit_id], |row| row.get(0)).unwrap();
            assert_eq!(retained, payload, "the committed historical receipt is retained");
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn owned_recent_real_commit_then_credential_revocation_refuses_publication() {
            committed_race(true).await;
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn owned_recent_real_commit_then_context_close_refuses_publication() {
            committed_race(false).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn owned_recent_aborted_waiter_keeps_committed_work_usage_until_release_and_drain() {
            let fixture = Fixture::new_owned();
            grant(&fixture, "B", "recent-cancel-b", fixture.principal(), false);
            append(&fixture, "B", "recent-cancel-b");
            let state = state(&fixture);
            let router = gateway_router(state.clone());
            let context = context(&router, "personal").await;
            let database = connection(&state);
            let before = events_receipts(&database);
            let owned = state.owned_human.as_ref().unwrap().clone();
            let (reached, release) = human::event_read_test_hook::install(&state);
            let request = tokio::spawn(async move { get(&router, RECENT, Some(&context)).await });
            tokio::task::spawn_blocking(move || reached.recv_timeout(Duration::from_secs(15))).await.unwrap().unwrap();
            assert_eq!(events_receipts(&database), before + 1, "real receipt committed before waiter cancellation");
            // The owned read epoch is already gone; usage and I/O ownership
            // must nevertheless survive cancellation of the HTTP observer.
            drop(owned.owner.try_write_epoch().unwrap());
            request.abort();
            assert!(request.await.unwrap_err().is_cancelled());
            assert!(owned.usages.load(Ordering::Acquire) > 0);
            let retiring = owned.clone();
            let drain = tokio::spawn(async move { retiring.drain().await });
            tokio::time::timeout(Duration::from_secs(15), async {
                while owned.coverage.load(Ordering::Acquire) != CLOSING {
                    tokio::task::yield_now().await;
                }
            }).await.unwrap();
            assert!(!drain.is_finished(), "accepted work still holds its usage while paused");
            drop(release);
            tokio::time::timeout(Duration::from_secs(15), drain).await.unwrap().unwrap().unwrap();
            assert_eq!(owned.usages.load(Ordering::Acquire), CLOSED);
            assert_eq!(events_receipts(&database), before + 1);
        }
    }
}
