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
