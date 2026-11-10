//! Synthetic control-request facts on real router/SQLite. No broker permission,
//! named-human instruction, original owner or runtime effect is proven here.
use super::*;

const KIND: &str = "broker.retirement.resume.requested";
fn resume_fact() -> Event {
    let mut event = Event::new(KIND);
    event.source = "ai-guard-broker".into();
    event.provenance = "observed".into();
    event.coverage = "native".into();
    event.data = json!({
        "version":1,
        "operation_id":Uuid::new_v4(),
        "command_id":event.id,
        "retirement_id":Uuid::new_v4(),
        "arm_id":Uuid::new_v4(),
        "original_epoch":Uuid::new_v4(),
        "generation":0,
        "identity_sha256":"a".repeat(64),
        "evidence_sha256":"b".repeat(64),
        "pending_ack_sha256":"c".repeat(64),
        "deadline_ns_decimal":"123456789012345"
    });
    event
}

#[tokio::test]
async fn retirement_resume_fact_requires_runner_control_route_and_replays_exact_receipt() {
    let files = support::private_tempdir();
    let store = Arc::new(fresh(
        &files.path().join("audit.sqlite"),
        64,
        AuditCapacityPolicyV1::resolved(64),
    ));
    let app = router(&store);
    let (ready, _) = request(
        &app,
        "POST",
        CONTROL_PATH,
        RUNNER,
        json!(fact("broker.retirement.pending")),
    )
    .await;
    assert_eq!(
        ready,
        StatusCode::OK,
        "actual existing trusted route must authenticate this runner"
    );
    let event = resume_fact();
    let head = store.integrity().unwrap().head_hash;
    for (token, expected) in [
        (VIEWER, StatusCode::FORBIDDEN),
        ("unknown-resume-credential", StatusCode::UNAUTHORIZED),
    ] {
        let polled = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&polled);
        let response = app
            .clone()
            .oneshot(
                Request::post(CONTROL_PATH)
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from_stream(futures_util::stream::once(async move {
                        observed.store(true, Ordering::SeqCst);
                        Ok::<_, std::io::Error>("invalid unread body")
                    })))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert!(!polled.load(Ordering::SeqCst));
    }
    assert_eq!(store.integrity().unwrap().head_hash, head);
    let (status, receipt) = request(&app, "POST", CONTROL_PATH, RUNNER, json!(event)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the new closed request fact must use trusted control delivery"
    );
    let receipt: StoredEvent = serde_json::from_value(receipt).unwrap();
    let (status, replayed) = request(&app, "POST", CONTROL_PATH, RUNNER, json!(event)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed["seq"], receipt.seq);
    assert_eq!(replayed["hash"], receipt.hash);
    let before = store.integrity().unwrap();
    assert_eq!(
        request(&app, "POST", "/api/events", RUNNER, json!(resume_fact()))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(store.integrity().unwrap().head_hash, before.head_hash);
    assert_eq!(store.integrity().unwrap().event_count, before.event_count);
}

#[tokio::test]
async fn retirement_resume_fact_uses_durable_control_reserve_without_ordinary_promotion() {
    let files = support::private_tempdir();
    let path = files.path().join("audit.sqlite");
    let policy = AuditCapacityPolicyV1::resolved(32);
    let store = Arc::new(fresh(&path, 32, policy.clone()));
    // Establish spare ordinary capacity before testing this kind's routing.
    store.append(fact("fixture.spare-ordinary-canary")).unwrap();
    let before = store.integrity().unwrap();
    assert!(before.event_count < 20);
    let quota_bytes = || {
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap()
            .query_row("SELECT bytes FROM quota WHERE id=1", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
    };
    let bytes_before = quota_bytes();
    let refusal = store
        .append(resume_fact())
        .expect_err("ordinary append accepted Resume with spare budget");
    assert_eq!(
        refusal.to_string(),
        "control events require typed mutation",
        "spare ordinary budget cannot authorize this control kind"
    );
    let after = store.integrity().unwrap();
    assert_eq!(after.head_hash, before.head_hash);
    assert_eq!(after.event_count, before.event_count);
    assert_eq!(quota_bytes(), bytes_before);
    assert!(after.valid);
    let count = store.integrity().unwrap().event_count;
    for _ in count..20 {
        store.append(fact("fixture.ordinary")).unwrap();
    }
    assert_eq!(store.integrity().unwrap().event_count, 20);
    let ordinary = store.append(resume_fact()).unwrap_err();
    // A kind does not promote public ordinary append into the trusted reserve.
    assert!(
        ordinary.downcast_ref::<AuditCapacityFailureV1>()
            == Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted)
            || ordinary
                .to_string()
                .contains("control events require typed mutation")
    );
    let app = router(&store);
    let event = resume_fact();
    let (status, stored) = request(&app, "POST", CONTROL_PATH, RUNNER, json!(event)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "ordinary exhaustion must leave the exact control request admissible"
    );
    let receipt: StoredEvent = serde_json::from_value(stored).unwrap();
    let after = store.integrity().unwrap();
    assert_eq!(after.event_count, 21);
    drop(app);
    drop(store);
    let reopened = Store::open_existing_for_tenant(&path, "tenant-d5", Redactor::new(vec![]), 32)
        .unwrap()
        .with_capacity_policy(policy)
        .unwrap();
    assert_eq!(reopened.integrity().unwrap().head_hash, after.head_hash);
    let rows = reopened
        .events(&agentsandbox::audit::EventQuery {
            kind: Some(KIND.into()),
            limit: Some(2),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].seq, receipt.seq);
    assert_eq!(rows[0].hash, receipt.hash);
    assert_eq!(rows[0].event.id, event.id);
}

#[tokio::test]
async fn retirement_resume_fact_refuses_noncanonical_or_unknown_request_fields_without_mutation() {
    let files = support::private_tempdir();
    let store = Arc::new(fresh(
        &files.path().join("audit.sqlite"),
        64,
        AuditCapacityPolicyV1::resolved(64),
    ));
    let app = router(&store);
    let head = store.integrity().unwrap().head_hash;
    for field in [
        "unknown",
        "tenant_id",
        "command_id",
        "arm_id",
        "identity_sha256",
        "generation",
        "deadline_ns_decimal",
        "source",
        "provenance",
        "result",
    ] {
        let mut event = resume_fact();
        match field {
            "source" => event.source = "worker".into(),
            "provenance" => event.provenance = "declared".into(),
            "result" => event.result = Some(json!({"started":true})),
            "unknown" | "tenant_id" => event.data[field] = json!("not-an-authority"),
            "command_id" => event.data[field] = json!(Uuid::new_v4()),
            "arm_id" => event.data[field] = json!("AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA"),
            "identity_sha256" => event.data[field] = json!("A".repeat(64)),
            "generation" => event.data[field] = json!(u64::MAX),
            "deadline_ns_decimal" => event.data[field] = json!("0123"),
            _ => unreachable!(),
        }
        let (status, _) = request(&app, "POST", CONTROL_PATH, RUNNER, json!(event)).await;
        assert!(!status.is_success(), "malformed request accepted: {field}");
        assert_eq!(store.integrity().unwrap().head_hash, head);
        assert!(store.healthy());
    }
}

#[tokio::test]
async fn retirement_resume_redacts_before_storage_and_refuses_redacted_binding_collisions() {
    let files = support::private_tempdir();
    let path = files.path().join("audit.sqlite");
    let canary = "synthetic-private-resume-canary";
    let store = Store::open_for_tenant(
        &path,
        "tenant-d5",
        Redactor::new(vec![canary.into(), "a".repeat(64)]),
        64,
    )
    .unwrap();
    store
        .migrate_control_v3(&path.with_extension("before3"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("before4"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("before5"))
        .unwrap();
    let store = Arc::new(store);
    let app = router(&store);
    let mut event = resume_fact();
    event.data["identity_sha256"] = json!("d".repeat(64));
    event.agent_id = Some(canary.into());
    let (status, value) = request(&app, "POST", CONTROL_PATH, RUNNER, json!(event)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!value.to_string().contains(canary));
    let rows = store
        .events(&agentsandbox::audit::EventQuery {
            kind: Some(KIND.into()),
            limit: Some(2),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(!serde_json::to_string(&rows).unwrap().contains(canary));
    let before = store.integrity().unwrap();
    // A configured literal collides with a canonical digest. After redaction,
    // the binding must be rejected rather than persisted with changed meaning.
    let (status, _) = request(&app, "POST", CONTROL_PATH, RUNNER, json!(resume_fact())).await;
    assert!(!status.is_success());
    assert_eq!(store.integrity().unwrap().head_hash, before.head_hash);
    assert_eq!(store.integrity().unwrap().event_count, before.event_count);
}
