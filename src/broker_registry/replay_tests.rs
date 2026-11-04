//! Deterministic transition oracles; authentication uses separate SQLite tests.
use super::*;

fn private_tempdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;

    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("create a private broker replay test directory")
}

fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn record(change: Change, time: &str) -> Record {
    Record {
        version: 1,
        input_hash: change.input_hash().unwrap(),
        received_at: time.into(),
        change,
    }
}

fn grant() -> BrokerGrantMutationV1 {
    BrokerGrantMutationV1 {
        version: 1,
        operation_id: id(),
        broker_id: id(),
        expected_revision: None,
        credential_id: id(),
        incarnation_id: id(),
        authorities: vec![BrokerAuthorityV1 {
            authority_id: "host-a:docker".into(),
            runtime_kind: BrokerRuntimeKindV1::Docker,
        }],
        lease_seconds: 30,
        not_after: "2026-11-03T12:00:00.000Z".into(),
    }
}

fn heartbeat(grant: &BrokerGrantMutationV1, revision: u64, sequence: u64) -> Change {
    Change::Heartbeat {
        identity: IdentityEvidence {
            tenant_id: "tenant-a".into(),
            broker_id: grant.broker_id.clone(),
            credential_id: grant.credential_id.clone(),
            binding_revision: 1,
            grant_revision: 1,
        },
        heartbeat: BrokerHeartbeatV1 {
            version: 1,
            operation_id: id(),
            expected_heartbeat_revision: revision,
            incarnation_id: grant.incarnation_id.clone(),
            sequence,
        },
    }
}

#[test]
fn replay_sequence_and_cas_never_refresh_a_broker_on_conflict() {
    let mut projection = Projection::default();
    let grant = grant();
    projection
        .apply(
            &record(
                Change::Grant {
                    mutation: grant.clone(),
                },
                "2026-11-03T10:00:00.000Z",
            ),
            "tenant-a",
            1,
        )
        .unwrap();
    let accepted = record(heartbeat(&grant, 0, 1), "2026-11-03T10:00:01.000Z");
    let broker = projection.apply(&accepted, "tenant-a", 2).unwrap();
    assert!(projection.apply(&accepted, "tenant-a", 3).is_err());
    for change in [
        heartbeat(&grant, 0, 2),
        heartbeat(&grant, 1, 1),
        heartbeat(&grant, 1, 3),
    ] {
        assert!(
            projection
                .apply(&record(change, "2026-11-03T10:00:02.000Z"), "tenant-a", 3)
                .is_err()
        );
    }
    assert_eq!(projection.brokers[&grant.broker_id].0, broker);
    assert_eq!(projection.operations.len(), 2);
}

#[test]
fn rearming_retains_old_receipt_but_requires_new_incarnation_and_authorized_tenant() {
    let mut projection = Projection::default();
    let grant = grant();
    projection
        .apply(
            &record(
                Change::Grant {
                    mutation: grant.clone(),
                },
                "2026-11-03T10:00:00.000Z",
            ),
            "tenant-a",
            1,
        )
        .unwrap();
    let wrong_tenant = record(heartbeat(&grant, 0, 1), "2026-11-03T10:00:01.000Z");
    assert!(projection.apply(&wrong_tenant, "tenant-b", 2).is_err());
    let old = projection.apply(&wrong_tenant, "tenant-a", 2).unwrap();
    let mut rearm = grant.clone();
    rearm.operation_id = id();
    rearm.expected_revision = Some(1);
    rearm.incarnation_id = id();
    let broker = projection
        .apply(
            &record(
                Change::Grant {
                    mutation: rearm.clone(),
                },
                "2026-11-03T10:00:02.000Z",
            ),
            "tenant-a",
            3,
        )
        .unwrap();
    assert_eq!(broker.last_heartbeat, old.last_heartbeat);
    assert_eq!(broker.heartbeat_revision, 1);
    assert_eq!(
        broker.view_at("2026-11-03T10:00:03.000Z").unwrap().state,
        BrokerStateV1::Unseen
    );
    assert!(
        projection
            .apply(
                &record(heartbeat(&grant, 1, 2), "2026-11-03T10:00:03.000Z"),
                "tenant-a",
                4
            )
            .is_err()
    );
    let mut current = heartbeat(&rearm, 1, 1);
    if let Change::Heartbeat { identity, .. } = &mut current {
        identity.grant_revision = 2;
    }
    let current = projection
        .apply(&record(current, "2026-11-03T10:00:03.000Z"), "tenant-a", 4)
        .unwrap();
    assert_eq!(
        current.view_at("2026-11-03T10:00:03.000Z").unwrap().state,
        BrokerStateV1::Current
    );
}

#[test]
fn backward_clock_and_expired_grant_cannot_create_new_heartbeat() {
    let mut projection = Projection::default();
    let grant = grant();
    projection
        .apply(
            &record(
                Change::Grant {
                    mutation: grant.clone(),
                },
                "2026-11-03T10:00:00.000Z",
            ),
            "tenant-a",
            1,
        )
        .unwrap();
    assert!(
        projection
            .apply(
                &record(heartbeat(&grant, 0, 1), "2026-11-03T09:59:59.999Z"),
                "tenant-a",
                2
            )
            .is_err()
    );
    assert!(
        projection
            .apply(
                &record(heartbeat(&grant, 0, 1), "2026-11-03T12:00:00.000Z"),
                "tenant-a",
                2
            )
            .is_err()
    );
    assert_eq!(projection.brokers[&grant.broker_id].0.heartbeat_revision, 0);
    assert_eq!(projection.operations.len(), 1);
}

#[test]
fn pending_workload_byte_debit_uses_exact_utf8_payload_and_is_discarded_on_rollback() {
    let directory = private_tempdir();
    let path = directory.path().join("audit.sqlite");
    let store = Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    store
        .migrate_control_v3(&directory.path().join("v2.bak"))
        .unwrap();
    store
        .migrate_identity_v4(&directory.path().join("v3.bak"))
        .unwrap();
    store
        .migrate_human_access_v5(&directory.path().join("v4.bak"))
        .unwrap();
    store
        .migrate_workload_v6(&directory.path().join("v5.bak"))
        .unwrap();
    let head = store.head().unwrap();
    let grant = grant();
    // This white-box fixture exercises the private audit/cache transaction,
    // not workload authentication. No unauthenticated event is committed.
    {
        let mut db = store.db.lock().unwrap();
        let tx = store.begin_checked(&mut db).unwrap();
        let mut event = Event::new("control.broker_registry.heartbeat");
        event.project_id = Some("projet-é-東京".into());
        event.data =
            serde_json::to_value(record(heartbeat(&grant, 0, 1), &format_time(Utc::now())))
                .unwrap();
        let (event, payload) = store.prepare_event(event).unwrap();
        assert!(payload.len() > payload.chars().count());
        let bytes = payload.len() as u64;
        store
            .append_workload_in_transaction(
                &tx,
                event,
                payload,
                super::super::capacity::WorkloadAuditPrincipalV1::Broker {
                    broker_id: grant.broker_id,
                },
            )
            .unwrap();
        let mut cache = store.supervision_lease.capacity.lock().unwrap();
        let pending = cache.prepare(&tx).unwrap();
        assert_eq!(
            pending.workload_tenant,
            AuditCapacityV1 { events: 1, bytes }
        );
        assert_eq!(cache.workload_tenant, AuditCapacityV1::default());
    }
    assert_eq!(store.head().unwrap(), head);
    assert!(store.healthy());
    assert_eq!(
        store
            .supervision_lease
            .capacity
            .lock()
            .unwrap()
            .workload_tenant,
        AuditCapacityV1::default()
    );
}
