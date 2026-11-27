mod support;

use agentsandbox::{
    audit::{AuditCapacityFailureV1, AuditCapacityPolicyV1, AuditCapacityV1, Store},
    broker_registry::*,
    redaction::Redactor,
};

fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn grant(broker_id: &str, authority: &str) -> BrokerGrantMutationV1 {
    BrokerGrantMutationV1 {
        version: 1,
        operation_id: id(),
        broker_id: broker_id.into(),
        expected_revision: None,
        credential_id: id(),
        incarnation_id: id(),
        authorities: vec![BrokerAuthorityV1 {
            authority_id: authority.into(),
            runtime_kind: BrokerRuntimeKindV1::Docker,
        }],
        lease_seconds: 30,
        not_after: "2100-01-01T00:00:00.000Z".into(),
    }
}

fn store(path: &std::path::Path, tenant: &str) -> Store {
    store_capacity(path, tenant, 10000)
}

fn store_capacity(path: &std::path::Path, tenant: &str, capacity: u64) -> Store {
    let store = Store::open_for_tenant(path, tenant, Redactor::new(vec![]), capacity).unwrap();
    store
        .migrate_control_v3(&path.with_extension("backup3"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("backup4"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("backup5"))
        .unwrap();
    store
        .migrate_workload_v6(&path.with_extension("backup6"))
        .unwrap();
    store
}

#[test]
fn two_brokers_are_durable_and_tenant_views_do_not_cross_databases() {
    let directory = support::private_tempdir();
    let a_path = directory.path().join("a.sqlite");
    let b_path = directory.path().join("b.sqlite");
    let a = store(&a_path, "tenant-a");
    let b = store(&b_path, "tenant-b");
    let first = grant(&id(), "host-a:docker");
    let second = grant(&id(), "host-b:docker");
    let receipt = a.mutate_broker_grant(&first).unwrap();
    a.mutate_broker_grant(&second).unwrap();
    assert!(!receipt.replayed);
    let page = a.brokers_page(None, 1).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(
        a.brokers_page(Some(&page[0].broker.broker_id), 1)
            .unwrap()
            .len(),
        1
    );
    assert!(b.brokers_page(None, 100).unwrap().is_empty());
    assert!(b.broker(&first.broker_id).unwrap().is_none());
    drop(a);
    let a =
        Store::open_existing_for_tenant(&a_path, "tenant-a", Redactor::new(vec![]), 10000).unwrap();
    assert_eq!(a.brokers_page(None, 100).unwrap().len(), 2);
    let replay = a.mutate_broker_grant(&first).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.receipt, receipt.receipt);
    assert_eq!(replay.broker, receipt.broker);
    assert_eq!(
        a.broker(&first.broker_id).unwrap().unwrap().state,
        BrokerStateV1::Unseen
    );
    assert!(
        !a.broker(&first.broker_id)
            .unwrap()
            .unwrap()
            .runtime_inventory_complete
    );
    assert!(a.integrity().unwrap().valid);
}

#[test]
fn authority_cannot_be_stolen_even_after_revocation_or_restart() {
    let directory = support::private_tempdir();
    let path = directory.path().join("a.sqlite");
    let a = store(&path, "tenant-a");
    let original = grant(&id(), "host-a:docker");
    a.mutate_broker_grant(&original).unwrap();
    let competing = grant(&id(), "host-a:docker");
    let head = a.head().unwrap();
    assert!(a.mutate_broker_grant(&competing).is_err());
    assert_eq!(
        a.broker_for_authority("host-a:docker")
            .unwrap()
            .unwrap()
            .broker
            .broker_id,
        original.broker_id
    );
    assert_eq!(a.head().unwrap(), head);
    let revoke = BrokerRevocationV1 {
        version: 1,
        operation_id: id(),
        broker_id: original.broker_id.clone(),
        expected_revision: 1,
    };
    let revoked = a.revoke_broker(&revoke).unwrap();
    assert_eq!(revoked.broker.grant_revision, 2);
    assert_eq!(
        a.broker(&original.broker_id).unwrap().unwrap().state,
        BrokerStateV1::Revoked
    );
    assert!(a.revoke_broker(&revoke).unwrap().replayed);
    drop(a);
    let a =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10000).unwrap();
    assert!(a.mutate_broker_grant(&competing).is_err());
    let mut rearm = original.clone();
    rearm.operation_id = id();
    rearm.expected_revision = Some(2);
    assert!(
        a.mutate_broker_grant(&rearm).is_err(),
        "an incarnation cannot be silently reused"
    );
    rearm.incarnation_id = id();
    a.mutate_broker_grant(&rearm).unwrap();
    assert_eq!(
        a.broker(&original.broker_id).unwrap().unwrap().state,
        BrokerStateV1::Unseen
    );
    assert!(a.integrity().unwrap().valid);
    let mut removed = rearm.clone();
    removed.operation_id = id();
    removed.expected_revision = Some(3);
    removed.incarnation_id = id();
    removed.authorities[0].authority_id = "host-a:podman".into();
    removed.authorities[0].runtime_kind = BrokerRuntimeKindV1::Podman;
    a.mutate_broker_grant(&removed).unwrap();
    let historical = a.broker_for_authority("host-a:docker").unwrap().unwrap();
    assert_eq!(historical.broker.broker_id, original.broker_id);
    assert!(
        !historical
            .broker
            .authorities
            .iter()
            .any(|authority| authority.authority_id == "host-a:docker")
    );
    assert!(
        a.mutate_broker_grant(&competing).is_err(),
        "removed scope retains historical ownership"
    );
}

#[test]
fn divergent_operation_and_stale_revision_do_not_mutate_the_registry() {
    let directory = support::private_tempdir();
    let path = directory.path().join("a.sqlite");
    let a = store(&path, "tenant-a");
    let original = grant(&id(), "host-a:docker");
    a.mutate_broker_grant(&original).unwrap();
    let head = a.head().unwrap();
    let mut divergent = original.clone();
    divergent.lease_seconds = 31;
    assert!(a.mutate_broker_grant(&divergent).is_err());
    let mut stale = original.clone();
    stale.operation_id = id();
    stale.incarnation_id = id();
    stale.expected_revision = Some(2);
    assert!(a.mutate_broker_grant(&stale).is_err());
    assert_eq!(a.head().unwrap(), head);
    assert_eq!(
        a.broker(&original.broker_id)
            .unwrap()
            .unwrap()
            .broker
            .grant_revision,
        1
    );
}

#[test]
fn forged_typed_event_and_tampered_projection_are_refused() {
    let directory = support::private_tempdir();
    let path = directory.path().join("a.sqlite");
    let a = store(&path, "tenant-a");
    a.mutate_broker_grant(&grant(&id(), "host-a:docker"))
        .unwrap();
    assert!(
        a.append(agentsandbox::model::Event::new(
            "control.broker_registry.heartbeat"
        ))
        .is_err()
    );
    drop(a);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "UPDATE broker_registry_brokers SET heartbeat_revision=10",
        [],
    )
    .unwrap();
    drop(db);
    assert!(
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10000)
            .map_or(true, |store| !store.healthy())
    );
}

#[test]
fn ordinary_saturation_preserves_one_control_grant_and_identity_revocation() {
    let directory = support::private_tempdir();
    let path = directory.path().join("a.sqlite");
    let a = store_capacity(&path, "tenant-a", 64);
    let mut capacity = AuditCapacityPolicyV1::resolved_for(64, a.physical_bytes());
    capacity.control_reserve = AuditCapacityV1 {
        events: 1,
        bytes: 1,
    };
    capacity.identity_reserve = AuditCapacityV1 {
        events: 1,
        bytes: 1,
    };
    let a = a.with_capacity_policy(capacity).unwrap();
    let original = grant(&id(), "host-a:docker");
    a.mutate_broker_grant(&original).unwrap();
    let ordinary_failure = (0..64)
        .find_map(
            |_| match a.append(agentsandbox::model::Event::new("test.ordinary")) {
                Ok(_) => None,
                Err(error) => Some(error),
            },
        )
        .expect("ordinary lane must saturate within its bounded event capacity");
    assert_eq!(
        ordinary_failure.downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted)
    );

    // The D5 control reserve has exactly one event slot after the ordinary
    // ceiling; a distinct grant may consume it, but not the identity reserve.
    let control_grant = grant(&id(), "host-b:docker");
    a.mutate_broker_grant(&control_grant).unwrap();
    assert!(a.broker(&control_grant.broker_id).unwrap().is_some());
    let head = a.head().unwrap();
    let refused = grant(&id(), "host-c:docker");
    let refusal = a.mutate_broker_grant(&refused).unwrap_err();
    assert_eq!(
        refusal.downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted)
    );
    assert_eq!(a.head().unwrap(), head);
    assert!(a.broker(&refused.broker_id).unwrap().is_none());
    assert!(a.broker_for_authority("host-c:docker").unwrap().is_none());
    let revoke = BrokerRevocationV1 {
        version: 1,
        operation_id: id(),
        broker_id: original.broker_id.clone(),
        expected_revision: 1,
    };
    a.revoke_broker(&revoke).unwrap();
    assert_eq!(
        a.broker(&original.broker_id).unwrap().unwrap().state,
        BrokerStateV1::Revoked
    );
    assert_eq!(a.head().unwrap(), head + 1);
    assert!(a.integrity().unwrap().valid);
}
