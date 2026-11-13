//! R2 cases: actual committed receipt races and a real SQLite audit INSERT abort.
use super::*;

pub(super) fn state(fixture: &Fixture) -> GatewayState {
    let (bundle, credentials, tokens) = fixture.owned_startup();
    GatewayState::for_owned_human_sources(
        bundle,
        credentials,
        tokens,
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap()
}

pub(super) fn connection(state: &GatewayState) -> rusqlite::Connection {
    rusqlite::Connection::open_with_flags(
        state.owned_human.as_ref().unwrap().sources[1]
            .descriptor()
            .storage_binding
            .path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap()
}

pub(super) fn snapshot(connection: &rusqlite::Connection) -> (i64, String, i64, i64) {
    connection
        .query_row(
            "SELECT COALESCE(MAX(seq),0),COALESCE((SELECT hash FROM events ORDER BY seq DESC LIMIT 1),''),COUNT(*),COALESCE(SUM(length(CAST(payload AS BLOB))),0) FROM events",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap()
}

pub(super) async fn context(router: &Router) -> String {
    let (status, identity) = get(router, "/api/v1/human/identity", None).await;
    assert_eq!(status, StatusCode::OK, "{identity}");
    identity["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|context| context["tenant_id"] == "B" && context["mode"] == "personal")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn committed_read_race(revoke_credential: bool) {
    let fixture = Fixture::new_owned();
    seed_event(&fixture, "B", "owned-race-b", fixture.principal());
    let state = state(&fixture);
    let router = gateway_router(state.clone());
    let context = context(&router).await;
    let database = connection(&state);
    let before = snapshot(&database);
    let (reached, release) = human::event_read_test_hook::install(&state);
    let request = tokio::spawn(async move {
        get(&router, "/api/v1/human/events", Some(&context)).await
    });
    // A separate blocking participant observes the barrier; the Tokio worker
    // remains available to run the actual HTTP request.
    tokio::task::spawn_blocking(move || reached.recv_timeout(Duration::from_secs(15)))
        .await
        .unwrap()
        .unwrap();
    let (audit_id, payload): (String, String) = database
        .query_row(
            "SELECT id,payload FROM events WHERE seq>?1 AND kind='control.identity.access' AND json_extract(payload,'$.data.action')='events'",
            [before.0],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let audit: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(audit["data"]["result_count"], 1);
    assert_eq!(audit["data"]["principal"], serde_json::to_value(fixture.principal()).unwrap());
    assert_eq!(audit["data"]["mode"], "personal");
    assert!(audit["data"]["membership_id"].is_string());
    let committed = snapshot(&database);
    assert_eq!(committed.0, before.0 + 1);
    let floor: bool = database
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM audit_time_floors f JOIN events e ON f.event_seq>=e.seq WHERE e.id=?1)",
            [&audit_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(floor, "the publication barrier must follow the real receipt commit");
    if revoke_credential {
        // Controlled test intervention through an existing scoped writer.
        // It does not adopt a public identity mutation route.
        let owned = state.owned_human.as_ref().unwrap();
        let epoch = owned.owner.try_write_epoch().unwrap();
        let access = HumanSourceAccess::write(&epoch, &owned.sources[1]).unwrap();
        let digest: [u8; 32] = Sha256::digest(Fixture::bearer("B").as_bytes()).into();
        let mut binding = state.tenants[1]
            .store
            .local_credential_binding_scoped(&access, &digest)
            .unwrap()
            .unwrap();
        binding.revision += 1;
        binding.active = false;
        state.tenants[1]
            .store
            .mutate_identity_scoped(
                &epoch,
                &owned.sources[1],
                &change(IdentityChangeV1::Credential {
                    expected_revision: 1,
                    binding,
                }),
            )
            .unwrap();
        drop(epoch);
    } else {
        state.human.close_contexts();
    }
    // RAII also releases this request if any SQLite observation above panics.
    drop(release);
    let (status, body) = tokio::time::timeout(Duration::from_secs(15), request)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(status, StatusCode::OK, "{body}");
    // Both a changed source generation and a closed cursor registry map to
    // the existing identity_unavailable code; their recovery actions differ.
    assert_eq!(body["error"]["code"], "identity_unavailable");
    if !revoke_credential {
        assert_eq!(body["error"]["retry_action"], "retry");
    }
    assert!(body.get("items").is_none());
    assert!(body.get("read_receipt").is_none());
    assert!(body.get("next_cursor").is_none());
    let retained: String = database
        .query_row("SELECT payload FROM events WHERE id=?1", [&audit_id], |row| row.get(0))
        .unwrap();
    assert_eq!(retained, payload, "the earlier committed read remains historical");
    if !revoke_credential {
        assert_eq!(snapshot(&database), committed);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_events_metadata_authorization_change_after_committed_read_refuses_publication() {
    // Missing post-read proof/source revalidation would publish stale metadata.
    committed_read_race(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owned_events_metadata_context_close_after_committed_read_refuses_publication() {
    // Missing final context/cursor checks would publish into a closed context.
    committed_read_race(false).await;
}

#[tokio::test]
async fn owned_events_metadata_real_audit_insert_abort_rolls_back_without_publication() {
    let fixture = Fixture::new_owned();
    seed_event(&fixture, "B", "owned-audit-abort-b", fixture.principal());
    let state = state(&fixture);
    let router = gateway_router(state.clone());
    let context = context(&router).await;
    let (status, positive) = get(&router, "/api/v1/human/events", Some(&context)).await;
    assert_eq!(status, StatusCode::OK, "{positive}");
    assert_eq!(positive["items"].as_array().unwrap().len(), 1);
    let database = connection(&state);
    let receipt_exists: bool = database
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE id=?1 AND kind='control.identity.access' AND json_extract(payload,'$.data.action')='events')",
            [positive["read_receipt"]["audit_event_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(receipt_exists);
    let trigger = state.tenants[1].store.modern_test_abort_events_audit().unwrap();
    let before = snapshot(&database);
    let usage = state.tenants[1].store.modern_test_human_usage();
    let (status, refused) = get(&router, "/api/v1/human/events", Some(&context)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{refused}");
    assert_eq!(refused["error"]["code"], "audit_unavailable");
    assert!(refused.get("items").is_none());
    assert!(refused.get("read_receipt").is_none());
    assert!(refused.get("next_cursor").is_none());
    assert!(!refused.to_string().contains("synthetic owned events audit insertion failure"));
    assert!(!refused.to_string().contains("synthetic-private-journal-content"));
    assert_eq!(snapshot(&database), before);
    assert_eq!(state.tenants[1].store.modern_test_human_usage(), usage);
    assert!(!state.tenants[1].store.healthy());
    // Removing fixture injection does not repair the permanently latched
    // Store. A separate operator recovery procedure is not exercised here.
    trigger.remove().unwrap();
    assert!(!state.tenants[1].store.healthy());
    let (status, latched) = get(&router, "/api/v1/human/events", Some(&context)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{latched}");
    assert_eq!(latched["error"]["code"], "audit_unavailable");
    assert!(latched.get("items").is_none());
    assert!(latched.get("read_receipt").is_none());
    assert!(latched.get("next_cursor").is_none());
    assert_eq!(snapshot(&database), before);
    assert_eq!(state.tenants[1].store.modern_test_human_usage(), usage);
    assert!(!state.tenants[1].store.healthy());
}
