//! Replay of owned fixture records; no runtime, secret or external endpoint.
use super::*;
use crate::audit::kubernetes_admission_state_tests::Fixture;
use uuid::Uuid;

fn rewrite_last_decision_expiry(path: &Path, expires_at: &str) {
    let db = Connection::open(path).unwrap();
    let (seq, received_at, previous, payload): (i64, String, String, String) = db
        .query_row(
            "SELECT seq,received_at,prev_hash,payload FROM events ORDER BY seq DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let mut event: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(event["kind"], "control.kubernetes.create.emission_possible");
    let mut decision: AuthorizationEvidence =
        serde_json::from_value(event["data"]["control_decision"].clone()).unwrap();
    decision.expires_at = expires_at.into();
    decision.decision_hash = decision.commitment().unwrap();
    event["data"]["control_decision"] = serde_json::to_value(decision).unwrap();
    let payload = serde_json::to_string(&event).unwrap();
    let hash = crate::audit::digest(seq, &received_at, &previous, &payload);
    db.execute(
        "UPDATE events SET payload=?1,hash=?2 WHERE seq=?3",
        params![payload, hash, seq],
    )
    .unwrap();
    db.execute("UPDATE quota SET bytes=(SELECT COALESCE(SUM(length(CAST(payload AS BLOB))),0) FROM events) WHERE id=1", []).unwrap();
}

fn rewritten_expiry_is_valid(binding: &str, grant: &str, rewritten_expiry: &str) -> bool {
    let f = Fixture::new_expiries(binding, grant);
    let authorization = f
        .store
        .authorize_held_create_for_test(
            &f.identity,
            &f.intent,
            &QuotaPolicyV1::default(),
            "2080-01-01T00:00:00.000Z",
        )
        .unwrap();
    f.store
        .reserve_kubernetes_create(&f.identity, &authorization, &f.intent)
        .unwrap();
    let claim = KubernetesCreateClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: f.intent.create_id.clone(),
        intent_hash: f.intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
        emitter_id: Uuid::new_v4().to_string(),
    };
    f.store
        .claim_kubernetes_create(&f.identity, &authorization, &claim)
        .unwrap();
    assert!(
        f.store.integrity().unwrap().valid,
        "unmodified real Store must be valid"
    );
    let path = f._directory.path().join("tenant.sqlite");
    drop(f.store);
    rewrite_last_decision_expiry(&path, rewritten_expiry);
    let reopened =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap();
    let valid = match reopened.integrity() {
        Ok(report) => report.valid,
        Err(error) => {
            assert_eq!(
                error.downcast_ref::<KubernetesCreateFailureV1>(),
                Some(&KubernetesCreateFailureV1::PermissionDenied),
                "the malformed decision must fail its specific authorization check"
            );
            false
        }
    };
    assert_eq!(reopened.healthy(), valid);
    if !valid {
        assert!(
            reopened
                .append(Event::new("fixture.denied_after_invalid_decision"))
                .is_err()
        );
    }
    valid
}

#[test]
fn historical_decision_expiry_cannot_exceed_its_binding_or_grant() {
    for (binding, grant) in [
        ("2090-01-01T00:00:00.000Z", "2100-01-01T00:00:00.000Z"),
        ("2100-01-01T00:00:00.000Z", "2090-01-01T00:00:00.000Z"),
    ] {
        assert!(
            rewritten_expiry_is_valid(binding, grant, "2085-01-01T00:00:00.000Z"),
            "the neighboring valid commitment isolates the expiry rule from harness failures"
        );
        assert!(
            !rewritten_expiry_is_valid(binding, grant, "2095-01-01T00:00:00.000Z"),
            "a self-consistent decision commitment may not prolong the historical identity"
        );
    }
}

#[test]
fn an_exact_capability_can_be_reused_after_a_later_control_event() {
    let f = Fixture::new();
    let authorization = f
        .store
        .authorize_held_create_for_test(
            &f.identity,
            &f.intent,
            &QuotaPolicyV1::default(),
            "2080-01-01T00:00:00.000Z",
        )
        .unwrap();
    f.store
        .reserve_kubernetes_create(&f.identity, &authorization, &f.intent)
        .unwrap();
    // Arrange a later millisecond, not a performance/latency assertion. A host
    // pause advances UTC too; bounded waiting only detects a nonadvancing clock.
    let wait_started = std::time::Instant::now();
    while format_time(Utc::now()) <= authorization.evidence.issued_at {
        assert!(
            wait_started.elapsed() < std::time::Duration::from_secs(30),
            "fixture UTC clock did not advance"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let mut unrelated = Event::new("fixture.control_progress");
    unrelated.timestamp = format_time(Utc::now());
    f.store.append(unrelated).unwrap();
    let later = f
        .store
        .events(&EventQuery {
            kind: Some("fixture.control_progress".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(later[0].received_at > authorization.evidence.issued_at);
    let claim = KubernetesCreateClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: f.intent.create_id.clone(),
        intent_hash: f.intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
        emitter_id: Uuid::new_v4().to_string(),
    };
    f.store
        .claim_kubernetes_create(&f.identity, &authorization, &claim)
        .unwrap();
    assert!(f.store.integrity().unwrap().valid);
}
