//! Real E1 checked-writer regressions; all require Root/Luna execution.
use super::*;
use super::fixtures::Fixture;
use crate::control::egress_policy::EgressPolicyExpectedScopeV1;
use crate::audit::human_modern::HumanDiscoveryGeneration;

fn context(f: &Fixture, epoch: &HumanSourcesWriteEpoch) -> (EgressPolicyExpectedScopeV1, HumanDiscoveryGeneration) {
    let scope = EgressPolicyExpectedScopeV1::new(f.stores[0].tenant_id(), &f.sources[0].descriptor().durable_namespace_id.to_string()).unwrap();
    let access = HumanSourceAccess::write(epoch, &f.sources[0]).unwrap();
    let generation = f.stores[0].modern_discovery_generation(&access).unwrap();
    (scope,generation)
}

#[test]
fn actual_checked_e1_scope_precedes_fresh_and_retry_policy_lookup_without_neighbor_sql() {
    let f = Fixture::new(EgressStoreLimitsV1::default());
    let request = f.replace(EgressPolicySelectorV1::Tenant,0,0,&["example.com:443"]);
    let neighbor = crate::audit::read_test_observer::observe(&f.stores[1]);
    for committed in [false,true] {
        if committed { f.install(&request).unwrap(); }
        let before = f.rows(0);
        let epoch = f.owner.try_write_epoch().unwrap();
        let (scope,generation) = context(&f,&epoch);
        let wrong = EgressPolicyExpectedScopeV1::new("B",scope.control_namespace_uuid()).unwrap();
        let error = f.stores[0].egress_replace_policy_scoped(&epoch,&f.sources[0],&f.credential,&wrong,generation,&request).unwrap_err();
        assert_eq!(error.downcast_ref::<EgressStoreFailure>(),Some(&EgressStoreFailure::ScopeConflict));
        drop(epoch);
        assert_eq!(f.rows(0),before);
        assert!(neighbor.try_recv().is_err());
    }
}

#[test]
fn actual_checked_e1_nowait_mutex_and_sqlite_restore_713_before_real_recovery() {
    let f = Fixture::new(EgressStoreLimitsV1::default());
    let store = &f.stores[0];
    store.owned_control_set_busy_timeout_for_test(713).unwrap();
    let request = f.replace(EgressPolicySelectorV1::Tenant,0,0,&["example.com:443"]);
    let before = f.rows(0);
    {
        let epoch = f.owner.try_write_epoch().unwrap();
        let (scope,generation) = context(&f,&epoch);
        let held = store.db.lock().unwrap();
        let error = store.egress_replace_policy_scoped(&epoch,&f.sources[0],&f.credential,&scope,generation,&request).unwrap_err();
        assert_eq!(error.downcast_ref::<review_access::ReviewAccessFailure>(),Some(&review_access::ReviewAccessFailure::Busy));
        drop(held);
    }
    assert_eq!(store.owned_control_busy_timeout_for_test().unwrap(),713);
    assert_eq!(f.rows(0),before);
    {
        let epoch = f.owner.try_write_epoch().unwrap();
        let (scope,generation) = context(&f,&epoch);
        let lock = Connection::open(f.sources[0].descriptor().storage_binding.path()).unwrap();
        lock.execute_batch("BEGIN IMMEDIATE").unwrap();
        let error = store.egress_replace_policy_scoped(&epoch,&f.sources[0],&f.credential,&scope,generation,&request).unwrap_err();
        assert_eq!(error.downcast_ref::<review_access::ReviewAccessFailure>(),Some(&review_access::ReviewAccessFailure::Busy));
        assert_eq!(store.owned_control_busy_timeout_for_test().unwrap(),713);
        lock.execute_batch("ROLLBACK").unwrap();
    }
    assert_eq!(f.rows(0),before);
    let epoch = f.owner.try_write_epoch().unwrap();
    let (scope,generation) = context(&f,&epoch);
    store.egress_replace_policy_scoped(&epoch,&f.sources[0],&f.credential,&scope,generation,&request).unwrap();
    drop(epoch);
    assert_eq!(store.owned_control_busy_timeout_for_test().unwrap(),713);
    assert_eq!(f.rows(0).operations,before.operations+1);
}

#[test]
fn actual_checked_e1_sqlite_full_rolls_back_policy_reserve_and_restores_713() {
    let f = Fixture::new(EgressStoreLimitsV1::default());
    f.install(&f.replace(EgressPolicySelectorV1::Tenant,0,0,&[])).unwrap();
    let store = &f.stores[0];
    store.owned_control_set_busy_timeout_for_test(713).unwrap();
    f.force_sqlite_full();
    let before = f.rows(0);
    let epoch = f.owner.try_write_epoch().unwrap();
    let (scope,generation) = context(&f,&epoch);
    let error = store.egress_replace_policy_scoped(&epoch,&f.sources[0],&f.credential,&scope,generation,&f.large_replace()).unwrap_err();
    assert!(error.chain().any(|cause|matches!(cause.downcast_ref::<rusqlite::Error>(),Some(rusqlite::Error::SqliteFailure(code,_)) if code.code==rusqlite::ErrorCode::DiskFull)));
    drop(epoch);
    assert_eq!(store.owned_control_busy_timeout_for_test().unwrap(),713);
    assert_eq!(f.rows(0),before);
    // No flag reset, fake successful recovery or weakened capacity assertion.
}

#[test]
fn actual_checked_e1_stale_discovery_refuses_before_a_policy_operation() {
    let f = Fixture::new(EgressStoreLimitsV1::default());
    let old = {
        let epoch = f.owner.try_write_epoch().unwrap();
        context(&f,&epoch)
    };
    f.create_session(&EgressPolicySelectorV1::Session(uuid::Uuid::new_v4().to_string()));
    let before = f.rows(0);
    let epoch = f.owner.try_write_epoch().unwrap();
    let error = f.stores[0].egress_replace_policy_scoped(
        &epoch,&f.sources[0],&f.credential,&old.0,old.1,
        &f.replace(EgressPolicySelectorV1::Tenant,0,0,&["example.com:443"]),
    ).unwrap_err();
    assert_eq!(error.downcast_ref::<EgressStoreFailure>(),Some(&EgressStoreFailure::DiscoveryStale));
    drop(epoch);
    assert_eq!(f.rows(0),before);
}
