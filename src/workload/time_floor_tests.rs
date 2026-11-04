//! Deterministic UTC oracles with real private SQLite; the OS clock is untouched.
use super::*;
use std::os::unix::fs::PermissionsExt;

fn files() -> tempfile::TempDir {
    let mut builder = tempfile::Builder::new();
    builder.permissions(std::fs::Permissions::from_mode(0o700));
    builder.tempdir().unwrap()
}
fn store(path: &Path) -> Store {
    let store = Store::open_for_tenant(path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    store
        .migrate_control_v3(&path.with_extension("v2.bak"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("v3.bak"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("v4.bak"))
        .unwrap();
    store
        .migrate_workload_v6(&path.with_extension("v5.bak"))
        .unwrap();
    store
}
fn future() -> DateTime<Utc> {
    parse_time(&format_time(Utc::now() + chrono::Duration::days(1))).unwrap()
}
fn append_at(store: &Store, at: DateTime<Utc>) -> StoredEvent {
    let mut db = store.db.lock().unwrap();
    let tx = store.begin_checked(&mut db).unwrap();
    let mut event = Event::new("test.clock.received");
    event.timestamp = format_time(at);
    let (event, payload) = store.prepare_event(event).unwrap();
    let stored = store
        .append_with_capacity_class_at(
            &tx,
            event,
            payload,
            super::super::capacity::Class::OrdinaryWrite,
            at,
        )
        .unwrap();
    store.commit_checked(tx).unwrap();
    stored
}
fn guard(store: &Store, at: DateTime<Utc>, before: Option<i64>) -> anyhow::Result<()> {
    let mut db = store.db.lock().unwrap();
    let tx = store.begin_checked(&mut db)?;
    validate_control_time(&tx, &format_time(at), before)
}

#[test]
fn schema7_preserves_durable_time_floor_workload_guards_and_tamper_detection() {
    for remove_last in [false, true] {
        let files = files();
        let path = files.path().join("audit.sqlite");
        let audit = store(&path);
        let lower = future();
        let higher6 = lower + chrono::Duration::hours(2);
        let higher7 = lower + chrono::Duration::hours(4);
        let token = "schema7-clock-workload-test-material-0123456789abcdef";
        let (_, _, issue) = broker_with_expiry(&audit, lower + chrono::Duration::days(2), token);
        let high6 = append_at(&audit, higher6);
        let low6 = append_at(&audit, lower);
        let before = audit.integrity().unwrap();
        let namespace = audit.supervision_state().unwrap().control_namespace_uuid;
        audit
            .migrate_human_brokers_v7(&path.with_extension("v6.bak"))
            .unwrap();
        assert_eq!(audit.supervision_state().unwrap().schema_version, 7);
        assert_eq!(
            audit.supervision_state().unwrap().control_namespace_uuid,
            namespace
        );
        assert_eq!(
            audit.integrity().unwrap().event_count,
            before.event_count + 1
        );
        let high7 = append_at(&audit, higher7);
        let low7 = append_at(&audit, lower);
        assert!(high7.seq > low6.seq && low7.seq > high7.seq);
        let head = audit.head().unwrap();
        assert_eq!(
            guard(&audit, lower, None)
                .unwrap_err()
                .downcast_ref::<WorkloadFailureV1>(),
            Some(&WorkloadFailureV1::ClockRegression)
        );
        assert!(guard(&audit, higher7, None).is_ok());
        assert_eq!(
            audit
                .authenticate_workload_token(token)
                .map(|_| ())
                .unwrap_err()
                .downcast_ref::<WorkloadFailureV1>(),
            Some(&WorkloadFailureV1::ClockRegression)
        );
        assert_eq!(audit.head().unwrap(), head);
        {
            let db = audit.db.lock().unwrap();
            let (floor_count, maximum): (i64, String) = db.query_row("SELECT (SELECT COUNT(*) FROM audit_time_floors),max_received_at FROM audit_time_floors WHERE event_seq=?1", [head], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
            assert_eq!(floor_count, head);
            assert_eq!(maximum, format_time(higher7));
            for historical in [&high6, &low6] {
                let received: String = db
                    .query_row(
                        "SELECT received_at FROM events WHERE seq=?1",
                        [historical.seq],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(received, historical.received_at);
            }
        }
        assert!(audit.integrity().unwrap().valid);
        drop(audit);
        let reopened =
            Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000)
                .unwrap();
        assert_eq!(reopened.supervision_state().unwrap().schema_version, 7);
        assert!(!reopened.supervision_state().unwrap().active);
        assert_eq!(
            guard(&reopened, lower, None)
                .unwrap_err()
                .downcast_ref::<WorkloadFailureV1>(),
            Some(&WorkloadFailureV1::ClockRegression)
        );
        assert!(guard(&reopened, higher7, None).is_ok());
        assert_eq!(
            reopened
                .authenticate_workload_token(token)
                .map(|_| ())
                .unwrap_err()
                .downcast_ref::<WorkloadFailureV1>(),
            Some(&WorkloadFailureV1::ClockRegression)
        );
        assert!(reopened.integrity().unwrap().valid);
        drop(reopened);

        let db = rusqlite::Connection::open(&path).unwrap();
        let raw_head: (i64, String) = db
            .query_row(
                "SELECT seq,hash FROM events ORDER BY seq DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(raw_head, (head, low7.hash.clone()));
        if remove_last {
            assert_eq!(
                db.execute("DELETE FROM audit_time_floors WHERE event_seq=?1", [head])
                    .unwrap(),
                1
            );
        } else {
            assert_eq!(
                db.execute(
                    "UPDATE audit_time_floors SET max_received_at=?1 WHERE event_seq=?2",
                    rusqlite::params![format_time(lower), head]
                )
                .unwrap(),
                1
            );
        }
        let tampered_floor: Option<String> = db
            .query_row(
                "SELECT max_received_at FROM audit_time_floors WHERE event_seq=?1",
                [head],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        let expected_floor = (!remove_last).then(|| format_time(lower));
        assert_eq!(
            tampered_floor, expected_floor,
            "fixture must alter only the intended final floor row"
        );
        let prior_floor: String = db
            .query_row(
                "SELECT max_received_at FROM audit_time_floors WHERE event_seq=?1",
                [head - 1],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(prior_floor, format_time(higher7));
        drop(db);
        // Replay failures retain a diagnostic handle, never execution authority.
        let rejected =
            Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000)
                .unwrap();
        assert!(!rejected.healthy());
        let integrity = rejected.integrity().unwrap();
        assert!(!integrity.valid);
        assert_eq!(
            integrity.error,
            Some(format!("integrity violation at sequence {head}"))
        );
        assert_eq!(integrity.event_count, u64::try_from(head - 1).unwrap());
        assert_eq!(integrity.head_hash, high7.hash);
        assert!(rejected.authenticate_workload_token(token).is_err());
        assert!(rejected.issue_workload_binding(&issue).is_err());
        // This same clock point passed before corruption, so its refusal now
        // cannot be explained solely by the deliberately earlier UTC fixture.
        assert!(guard(&rejected, higher7, None).is_err());
        assert!(rejected.brokers_page(None, 1).is_err());
        assert!(
            rejected
                .append(Event::new("test.refused.after_floor_corruption"))
                .is_err()
        );
        let db = rusqlite::Connection::open(&path).unwrap();
        let after_head: (i64, String) = db
            .query_row(
                "SELECT seq,hash FROM events ORDER BY seq DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(after_head, raw_head);
        let after_floor: Option<String> = db
            .query_row(
                "SELECT max_received_at FROM audit_time_floors WHERE event_seq=?1",
                [head],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(
            after_floor, expected_floor,
            "diagnostic access must not repair the tampered row"
        );
    }
}

#[test]
fn durable_time_floor_survives_a_lower_generic_receipt_and_reopen() {
    let files = files();
    let path = files.path().join("audit.sqlite");
    let audit = store(&path);
    let lower = future();
    let higher = lower + chrono::Duration::hours(3);
    let high = append_at(&audit, higher);
    let low = append_at(&audit, lower);
    assert!(low.seq > high.seq);
    assert!(parse_time(&low.received_at).unwrap() < parse_time(&high.received_at).unwrap());
    let head = audit.head().unwrap();
    let error = guard(&audit, lower, None).unwrap_err();
    assert_eq!(
        error.downcast_ref::<WorkloadFailureV1>(),
        Some(&WorkloadFailureV1::ClockRegression)
    );
    assert!(guard(&audit, higher, None).is_ok());
    assert_eq!(audit.head().unwrap(), head);
    assert!(audit.healthy());
    drop(audit);
    let audit =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert!(!audit.supervision_state().unwrap().active);
    assert_eq!(
        guard(&audit, lower, None)
            .unwrap_err()
            .downcast_ref::<WorkloadFailureV1>(),
        Some(&WorkloadFailureV1::ClockRegression)
    );
    assert!(audit.integrity().unwrap().valid);
}

#[test]
fn historical_time_floor_uses_the_true_maximum_of_its_prefix() {
    let files = files();
    let audit = store(&files.path().join("audit.sqlite"));
    let lower = future();
    let high = append_at(&audit, lower + chrono::Duration::hours(3));
    let low = append_at(&audit, lower);
    assert!(
        guard(&audit, lower, Some(high.seq)).is_ok(),
        "future receipt does not taint an earlier prefix"
    );
    assert_eq!(
        guard(&audit, lower, Some(low.seq + 1))
            .unwrap_err()
            .downcast_ref::<WorkloadFailureV1>(),
        Some(&WorkloadFailureV1::ClockRegression)
    );
}

#[test]
fn same_millisecond_authorizations_work_but_a_fresh_raw_receipt_before_the_proof_is_refused() {
    let files = files();
    let audit = store(&files.path().join("audit.sqlite"));
    let base = future() + chrono::Duration::milliseconds(123);
    for (authorized, fresh) in [(100_000, 500_000), (900_000, 950_000)] {
        let mut db = audit.db.lock().unwrap();
        let tx = audit.begin_checked(&mut db).unwrap();
        let clock = control_time_at(&tx, base + chrono::Duration::nanoseconds(authorized)).unwrap();
        let (event, payload) = audit
            .prepare_event(Event::new("test.clock.authorized"))
            .unwrap();
        let stored = audit
            .append_with_authorized_capacity_class_at(
                &tx,
                event,
                payload,
                super::super::capacity::Class::OrdinaryWrite,
                &clock,
                base + chrono::Duration::nanoseconds(fresh),
            )
            .unwrap();
        assert_eq!(stored.received_at, format_time(base));
        validate_fresh_control_time_at(&tx, &clock, base + chrono::Duration::nanoseconds(fresh))
            .unwrap();
        // This deterministic clock sink uses the exact production fresh guard;
        // it intentionally does not read or change the real wall clock.
        audit.commit_checked(tx).unwrap();
    }
    let head = audit.head().unwrap();
    {
        let mut db = audit.db.lock().unwrap();
        let tx = audit.begin_checked(&mut db).unwrap();
        let clock = control_time_at(&tx, base + chrono::Duration::nanoseconds(990_000)).unwrap();
        let (event, payload) = audit
            .prepare_event(Event::new("test.clock.refused"))
            .unwrap();
        let error = audit
            .append_with_authorized_capacity_class_at(
                &tx,
                event,
                payload,
                super::super::capacity::Class::OrdinaryWrite,
                &clock,
                base + chrono::Duration::nanoseconds(980_000),
            )
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<WorkloadFailureV1>(),
            Some(&WorkloadFailureV1::ClockRegression)
        );
    }
    assert_eq!(audit.head().unwrap(), head);
    assert!(audit.healthy());
    assert!(audit.integrity().unwrap().valid);
}

#[test]
fn expiry_is_validated_in_its_durable_precision_without_rounding_into_a_new_authorization() {
    let at = future();
    let id = || uuid::Uuid::new_v4().to_string();
    let mut input = WorkloadBindingIssueV1 {
        version: 1,
        operation_id: id(),
        credential_id: id(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Broker {
            broker_id: id(),
            incarnation_id: id(),
        },
        sha256_digest: "a".repeat(64),
        expires_at: (at + chrono::Duration::nanoseconds(900_000))
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
    };
    let record = |input: WorkloadBindingIssueV1| {
        let change = Change::Issue { input };
        Record {
            version: 1,
            input_hash: change.input_hash().unwrap(),
            received_at: format_time(at),
            change,
        }
    };
    let mut projection = Projection::default();
    assert!(projection.apply(&record(input.clone()), 1).is_err());
    assert!(projection.bindings.is_empty() && projection.operations.is_empty());
    input.expires_at =
        (at + chrono::Duration::milliseconds(1) + chrono::Duration::nanoseconds(900_000))
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    let binding = projection.apply(&record(input), 1).unwrap();
    assert_eq!(
        binding.expires_at,
        format_time(at + chrono::Duration::milliseconds(1))
    );
}

#[test]
fn a_declared_generic_timestamp_cannot_advance_the_received_time_floor() {
    let files = files();
    let audit = store(&files.path().join("audit.sqlite"));
    let received = future();
    {
        let mut db = audit.db.lock().unwrap();
        let tx = audit.begin_checked(&mut db).unwrap();
        let mut event = Event::new("test.declared.timestamp");
        event.timestamp = format_time(received + chrono::Duration::days(30));
        let (event, payload) = audit.prepare_event(event).unwrap();
        audit
            .append_with_capacity_class_at(
                &tx,
                event,
                payload,
                super::super::capacity::Class::OrdinaryWrite,
                received,
            )
            .unwrap();
        audit.commit_checked(tx).unwrap();
    }
    assert!(guard(&audit, received, None).is_ok());
    assert!(audit.integrity().unwrap().valid);
}

fn broker_with_expiry(
    audit: &Store,
    expires: DateTime<Utc>,
    token: &str,
) -> (
    crate::broker_registry::BrokerGrantMutationV1,
    BrokerIdentityV1,
    WorkloadBindingIssueV1,
) {
    use crate::broker_registry::*;
    let id = || uuid::Uuid::new_v4().to_string();
    let grant = BrokerGrantMutationV1 {
        version: 1,
        operation_id: id(),
        broker_id: id(),
        expected_revision: None,
        credential_id: id(),
        incarnation_id: id(),
        authorities: vec![BrokerAuthorityV1 {
            authority_id: id(),
            runtime_kind: BrokerRuntimeKindV1::Docker,
        }],
        lease_seconds: 30,
        not_after: format_time(expires),
    };
    audit.mutate_broker_grant(&grant).unwrap();
    let issue = WorkloadBindingIssueV1 {
        version: 1,
        operation_id: id(),
        credential_id: grant.credential_id.clone(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Broker {
            broker_id: grant.broker_id.clone(),
            incarnation_id: grant.incarnation_id.clone(),
        },
        sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
        expires_at: format_time(expires),
    };
    audit.issue_workload_binding(&issue).unwrap();
    let Some(AuthenticatedWorkload::Broker(identity)) =
        audit.authenticate_workload_token(token).unwrap()
    else {
        panic!("valid broker fixture did not authenticate")
    };
    (grant, identity, issue)
}

#[test]
fn rollback_blocks_authentication_replay_and_self_read_before_lease_and_expiry_checks() {
    use crate::broker_registry::BrokerHeartbeatV1;
    let files = files();
    let path = files.path().join("audit.sqlite");
    let audit = store(&path);
    let t1 = future();
    let t2 = t1 + chrono::Duration::hours(2);
    let t3 = t1 + chrono::Duration::hours(3);
    let token = "broker-clock-token-test-material-0123456789abcdef";
    let (grant, identity, issue) = broker_with_expiry(&audit, t2, token);
    let (_, still_valid, _) = broker_with_expiry(
        &audit,
        t3 + chrono::Duration::hours(2),
        "other-clock-token-test-material-0123456789abcdef",
    );
    assert!(audit.issue_workload_binding(&issue).unwrap().replayed);
    {
        let mut db = audit.db.lock().unwrap();
        let tx = audit.begin_checked(&mut db).unwrap();
        revalidate_broker_identity(&tx, &identity, t1).unwrap();
        assert_eq!(
            revalidate_broker_identity(&tx, &identity, t2)
                .unwrap_err()
                .downcast_ref::<WorkloadFailureV1>(),
            Some(&WorkloadFailureV1::Unauthorized),
            "expiry is exclusive at the exact durable millisecond"
        );
    }
    append_at(&audit, t3);
    append_at(&audit, t1);
    let head = audit.head().unwrap();
    for result in [
        audit.authenticate_workload_token(token).map(|_| ()),
        audit.broker_for_identity(&identity).map(|_| ()),
        audit.mutate_broker_grant(&grant).map(|_| ()),
        audit.issue_workload_binding(&issue).map(|_| ()),
        audit
            .broker_heartbeat(
                &identity,
                &BrokerHeartbeatV1 {
                    version: 1,
                    operation_id: uuid::Uuid::new_v4().to_string(),
                    expected_heartbeat_revision: 0,
                    incarnation_id: grant.incarnation_id,
                    sequence: 1,
                },
            )
            .map(|_| ()),
    ] {
        assert_eq!(
            result.unwrap_err().downcast_ref::<WorkloadFailureV1>(),
            Some(&WorkloadFailureV1::ClockRegression)
        );
    }
    assert_eq!(audit.head().unwrap(), head);
    assert!(audit.healthy());
    assert!(audit.integrity().unwrap().valid);
    {
        let mut db = audit.db.lock().unwrap();
        let tx = audit.begin_checked(&mut db).unwrap();
        assert_eq!(
            revalidate_broker_identity(&tx, &identity, t1)
                .unwrap_err()
                .downcast_ref::<WorkloadFailureV1>(),
            Some(&WorkloadFailureV1::ClockRegression)
        );
        assert!(
            revalidate_broker_identity(&tx, &identity, t3).is_err(),
            "expired rights remain expired when the clock catches up"
        );
        revalidate_broker_identity(&tx, &still_valid, t3).unwrap();
    }
    drop(audit);
    let audit =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    let mut db = audit.db.lock().unwrap();
    let tx = audit.begin_checked(&mut db).unwrap();
    let worker = WorkerRelayIdentity {
        tenant_id: "tenant-a".into(),
        credential_id: identity.credential_id().into(),
        worker_id: uuid::Uuid::new_v4().to_string(),
        attempt_id: uuid::Uuid::new_v4().to_string(),
        generation: 1,
        binding_revision: 1,
    };
    // The worker fixture is intentionally not a workload identity proof. This
    // oracle tests that the shared clock refusal precedes the inactive lease.
    assert_eq!(
        audit
            .checked_worker_relay_identity(&tx, &worker, t1)
            .unwrap_err()
            .downcast_ref::<WorkloadFailureV1>(),
        Some(&WorkloadFailureV1::ClockRegression)
    );
    assert_eq!(
        revalidate_broker_identity(&tx, &identity, t1)
            .unwrap_err()
            .downcast_ref::<WorkloadFailureV1>(),
        Some(&WorkloadFailureV1::ClockRegression)
    );
}

#[test]
fn time_projection_write_failure_rolls_back_event_quota_and_unpublished_counters() {
    let files = files();
    let audit = store(&files.path().join("audit.sqlite"));
    let head = audit.head().unwrap();
    let before = audit
        .supervision_lease
        .capacity
        .lock()
        .unwrap()
        .workload_tenant;
    {
        let mut db = audit.db.lock().unwrap();
        let tx = audit.begin_checked(&mut db).unwrap();
        tx.execute_batch("CREATE TEMP TRIGGER fixture_time_write_failure BEFORE INSERT ON main.audit_time_floors BEGIN SELECT RAISE(ABORT,'fixture time projection write failure'); END").unwrap();
        let (event, payload) = audit
            .prepare_event(Event::new("test.time.write.failure"))
            .unwrap();
        assert!(
            audit
                .append_with_capacity_class_at(
                    &tx,
                    event,
                    payload,
                    super::super::capacity::Class::OrdinaryWrite,
                    Utc::now()
                )
                .is_err()
        );
    }
    assert_eq!(audit.head().unwrap(), head);
    assert_eq!(
        audit
            .supervision_lease
            .capacity
            .lock()
            .unwrap()
            .workload_tenant,
        before
    );
    let db = audit.db.lock().unwrap();
    let floor: i64 = db
        .query_row(
            "SELECT COALESCE(MAX(event_seq),0) FROM audit_time_floors",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(floor, head);
}

#[test]
fn explicit_migration_seeds_old_maximum_without_rewriting_historical_receipts() {
    let files = files();
    let path = files.path().join("audit.sqlite");
    let audit = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    audit
        .migrate_control_v3(&path.with_extension("v2.bak"))
        .unwrap();
    audit
        .migrate_identity_v4(&path.with_extension("v3.bak"))
        .unwrap();
    audit
        .migrate_human_access_v5(&path.with_extension("v4.bak"))
        .unwrap();
    let low = future();
    let high = append_at(&audit, low + chrono::Duration::hours(2));
    let lower = append_at(&audit, low);
    audit
        .migrate_workload_v6(&path.with_extension("v5.bak"))
        .unwrap();
    assert!(guard(&audit, low, None).is_err());
    let db = audit.db.lock().unwrap();
    let original: String = db
        .query_row(
            "SELECT received_at FROM events WHERE seq=?1",
            [high.seq],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(original, high.received_at);
    let original: String = db
        .query_row(
            "SELECT received_at FROM events WHERE seq=?1",
            [lower.seq],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(original, lower.received_at);
}
