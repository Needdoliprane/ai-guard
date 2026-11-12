//! Scoped relay operations keep source selection separate from worker authority.
use super::*;
use crate::human_authority::{HumanAuthorityFailure, migration::tests::Fixture};
use crate::{
    control::*,
    policy::Policy,
    workload::{AuthenticatedWorkload, WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use uuid::Uuid;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

#[path = "../../../tests/support/relay_admission_fixture.rs"]
#[allow(dead_code)]
mod seed;

#[test]
fn migrated_directed_relay_revocation_is_durable_and_idempotent_without_reactivating_workers() {
    let fixture = Fixture::new();
    let legacy = fixture.legacy_store("A");
    let first = seed::running_now(legacy);
    let second = seed::running_now(legacy);
    let token = "synthetic-a1-worker-token-000000000000000000000000";
    seed::bind(legacy, &first, token);
    seed::bind(
        legacy,
        &second,
        "synthetic-a2-worker-token-000000000000000000000000",
    );
    let Some(AuthenticatedWorkload::Worker(identity)) =
        legacy.authenticate_workload_token(token).unwrap()
    else {
        panic!("real seeded workload binding did not authenticate");
    };
    let grant = RelayGrantV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: Uuid::new_v4().to_string(),
        source: seed::endpoint(&first),
        destination: seed::endpoint(&second),
        channel: "tasks".into(),
        expires_at: (Utc::now() + Duration::minutes(5)).to_rfc3339(),
    };
    legacy.relay_grant(&grant).unwrap();
    let message = RelaySendV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: grant.grant_id.clone(),
        ttl_seconds: 60,
        body: "synthetic directed message".into(),
    };
    let sent = legacy.relay_send(&identity, &message).unwrap();
    assert!(!sent.replayed);
    assert!(legacy.relay_send(&identity, &message).unwrap().replayed);
    let mut wrong_tenant = identity.clone();
    wrong_tenant.tenant_id = "B".into();
    assert_eq!(
        legacy
            .relay_send(&wrong_tenant, &message)
            .unwrap_err()
            .downcast_ref::<RelayFailure>(),
        Some(&RelayFailure::Forbidden)
    );
    let bundle = fixture.migrated();
    let source_id = |tenant: &str| {
        bundle
            .owner()
            .closure()
            .sources
            .iter()
            .find(|source| source.tenant_id.as_deref() == Some(tenant))
            .unwrap()
            .source_id
    };
    let store = bundle.store(source_id("A")).unwrap();
    let token = bundle.owner().source_token(source_id("A")).unwrap();
    let other = bundle.owner().source_token(source_id("B")).unwrap();
    let epoch = bundle.owner().try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &token).unwrap();
    let foreign = HumanSourceAccess::read(&epoch, &other).unwrap();
    let input = RelayRevokeV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: grant.grant_id,
    };
    let before = audit_head(&store.db.lock().unwrap()).unwrap();
    assert!(store.relay_retention_status(4).is_err());
    assert!(store.relay_retention_status_scoped(&foreign, 4).is_err());
    let status = store.relay_retention_status_scoped(&access, 4).unwrap();
    assert_eq!(status.tenant.retained.messages, 1);
    assert_eq!(status.tenant.retained.bytes, 26);
    assert_eq!(status.senders_total, 1);
    assert_eq!(status.senders[0].worker_id, first.worker_id);
    assert_eq!(status.grants_total, 1);
    assert_eq!(status.grants[0].destination_worker_id, second.worker_id);
    assert!(
        !serde_json::to_string(&status)
            .unwrap()
            .contains("synthetic directed message")
    );
    assert!(store.relay_revoke_scoped(&foreign, &input).is_err());
    assert!(store.relay_revoke(&input).is_err());
    assert_eq!(audit_head(&store.db.lock().unwrap()).unwrap(), before);
    let revoked = store.relay_revoke_scoped(&access, &input).unwrap();
    assert!(!revoked.replayed);
    let replayed = store.relay_revoke_scoped(&access, &input).unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.audit_hash, revoked.audit_hash);
    let after = audit_head(&store.db.lock().unwrap()).unwrap();
    assert_eq!(after.0, before.0 + 1);
    // An accepted modern source is not an activation proof for workloads.
    for error in [
        store
            .relay_send_scoped(&access, &identity, &message)
            .unwrap_err(),
        match store.relay_inbox_scoped(&access, &identity, None, 4) {
            Err(error) => error,
            Ok(_) => panic!("inactive supervision must refuse the relay inbox"),
        },
        store
            .relay_send_scoped(&access, &wrong_tenant, &message)
            .unwrap_err(),
    ] {
        assert_eq!(
            error.downcast_ref::<RelayFailure>(),
            Some(&RelayFailure::Forbidden)
        );
    }
    assert_eq!(audit_head(&store.db.lock().unwrap()).unwrap(), after);
    assert_eq!(
        store
            .relay_retention_status_scoped(&access, 4)
            .unwrap()
            .tenant
            .retained
            .messages,
        1
    );
    assert!(store.permanently_healthy());
}

#[test]
fn migrated_relay_revoke_validates_source_before_lookup_and_leaves_unknown_grant_unwritten() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    let source_id = |tenant: &str| {
        bundle
            .owner()
            .closure()
            .sources
            .iter()
            .find(|source| source.tenant_id.as_deref() == Some(tenant))
            .unwrap()
            .source_id
    };
    let a = source_id("A");
    let token = bundle.owner().source_token(a).unwrap();
    let other = bundle.owner().source_token(source_id("B")).unwrap();
    let store = bundle.store(a).unwrap();
    let epoch = bundle.owner().try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &token).unwrap();
    let foreign = HumanSourceAccess::read(&epoch, &other).unwrap();
    let input = RelayRevokeV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: Uuid::new_v4().to_string(),
    };
    let head = audit_head(&store.db.lock().unwrap()).unwrap();
    for error in [
        store.relay_revoke(&input).unwrap_err(),
        store.relay_revoke_scoped(&foreign, &input).unwrap_err(),
    ] {
        assert_eq!(
            error.downcast_ref::<review_authority::ReviewAuthorityFailure>(),
            Some(&review_authority::ReviewAuthorityFailure::Required)
        );
    }
    assert_eq!(
        store
            .relay_revoke_scoped(&access, &input)
            .unwrap_err()
            .downcast_ref::<RelayFailure>(),
        Some(&RelayFailure::NotFound)
    );
    assert_eq!(audit_head(&store.db.lock().unwrap()).unwrap(), head);
    assert!(store.permanently_healthy());
}

#[test]
fn expired_relay_access_refuses_without_creating_a_mutation_or_latching_source() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    let source_id = bundle
        .owner()
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some("A"))
        .unwrap()
        .source_id;
    let store = bundle.store(source_id).unwrap();
    let token = bundle.owner().source_token(source_id).unwrap();
    let epoch = bundle.owner().try_write_epoch().unwrap();
    let access = HumanSourceAccess::write(&epoch, &token).unwrap();
    let input = RelayRevokeV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: Uuid::new_v4().to_string(),
    };
    let head = audit_head(&store.db.lock().unwrap()).unwrap();
    std::thread::sleep(
        epoch
            .test_deadline()
            .saturating_duration_since(std::time::Instant::now()),
    );
    assert_eq!(
        store
            .relay_revoke_scoped(&access, &input)
            .unwrap_err()
            .downcast_ref::<HumanAuthorityFailure>(),
        Some(&HumanAuthorityFailure::EpochExpired)
    );
    assert_eq!(audit_head(&store.db.lock().unwrap()).unwrap(), head);
    assert!(store.permanently_healthy());
}
