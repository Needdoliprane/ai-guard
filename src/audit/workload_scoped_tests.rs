//! Real migrated A/B/authentication sources; no authority is built from SQL.
use super::*;
use crate::{
    broker_registry::{
        BrokerAuthorityV1, BrokerGrantMutationV1, BrokerHeartbeatV1, BrokerRegistryFailureV1,
        BrokerRevocationV1, BrokerRuntimeKindV1,
    },
    human_authority::{
        HumanAuthorityFailure, HumanAuthorityInvalidationReason,
        migration::{MigratedHumanSources, tests::Fixture},
    },
};
use uuid::Uuid;

const TOKEN: &str = "public-migrated-workload-fixture-token-000000000000";

fn source_id(bundle: &MigratedHumanSources, tenant: &str) -> Uuid {
    bundle
        .owner()
        .closure()
        .sources
        .iter()
        .find(|source| source.tenant_id.as_deref() == Some(tenant))
        .unwrap()
        .source_id
}

fn access<T>(
    bundle: &MigratedHumanSources,
    tenant: &str,
    operation: impl FnOnce(&Store, &HumanSourceAccess<'_>) -> T,
) -> T {
    let source = bundle
        .owner()
        .source_token(source_id(bundle, tenant))
        .unwrap();
    let epoch = bundle.owner().try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &source).unwrap();
    operation(bundle.store(source.source_id()).unwrap(), &access)
}

fn grant() -> BrokerGrantMutationV1 {
    BrokerGrantMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        broker_id: Uuid::new_v4().to_string(),
        expected_revision: None,
        credential_id: Uuid::new_v4().to_string(),
        incarnation_id: Uuid::new_v4().to_string(),
        authorities: vec![BrokerAuthorityV1 {
            authority_id: "migrated-workload-fixture-authority".into(),
            runtime_kind: BrokerRuntimeKindV1::Docker,
        }],
        lease_seconds: 30,
        not_after: "2100-01-01T00:00:00.000Z".into(),
    }
}

fn issue(grant: &BrokerGrantMutationV1) -> WorkloadBindingIssueV1 {
    WorkloadBindingIssueV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: grant.credential_id.clone(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Broker {
            broker_id: grant.broker_id.clone(),
            incarnation_id: grant.incarnation_id.clone(),
        },
        sha256_digest: hex::encode(Sha256::digest(TOKEN.as_bytes())),
        expires_at: "2100-01-01T00:00:00.000Z".into(),
    }
}

fn heartbeat(grant: &BrokerGrantMutationV1) -> BrokerHeartbeatV1 {
    BrokerHeartbeatV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        expected_heartbeat_revision: 0,
        incarnation_id: grant.incarnation_id.clone(),
        sequence: 1,
    }
}

fn identity(bundle: &MigratedHumanSources) -> BrokerIdentityV1 {
    access(bundle, "A", |store, access| {
        match store
            .authenticate_workload_token_scoped(access, TOKEN)
            .unwrap()
            .unwrap()
        {
            AuthenticatedWorkload::Broker(identity) => identity,
            AuthenticatedWorkload::Worker(_) => panic!("fixture is a broker"),
        }
    })
}

/// Audit bytes and every workload/broker durable projection, not just a count.
fn snapshot(store: &Store) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    let db = store.db.lock().unwrap();
    std::iter::once("SELECT * FROM events ORDER BY seq")
        .chain(QUERIES.iter().map(|(_, query)| *query))
        .chain([
            "SELECT * FROM broker_registry_meta ORDER BY singleton",
            "SELECT * FROM broker_registry_brokers ORDER BY broker_id",
            "SELECT * FROM broker_registry_authorities ORDER BY authority_id",
            "SELECT * FROM broker_registry_incarnations ORDER BY incarnation_id",
            "SELECT * FROM broker_registry_operations ORDER BY operation_id",
            "SELECT * FROM audit_time_floors ORDER BY event_seq",
        ])
        .map(|query| {
            let mut statement = db.prepare(query).unwrap();
            let columns = statement.column_count();
            statement
                .query_map([], |row| (0..columns).map(|index| row.get(index)).collect())
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        })
        .collect()
}

#[test]
fn migrated_workload_lifecycle_and_broker_reads_use_real_owned_business_access() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    let grant = grant();
    let issue = issue(&grant);
    let issued = access(&bundle, "A", |store, access| {
        store.issue_workload_binding_scoped(access, &issue).unwrap()
    });
    assert_eq!(issued.revision, 1);
    let replay = access(&bundle, "A", |store, access| {
        store.issue_workload_binding_scoped(access, &issue).unwrap()
    });
    assert!(replay.replayed);
    assert_eq!(issued.audit_seq, replay.audit_seq);
    assert_eq!(issued.audit_hash, replay.audit_hash);
    access(&bundle, "A", |store, access| {
        store.mutate_broker_grant_scoped(access, &grant).unwrap();
    });
    let identity = identity(&bundle);
    let store = bundle.store(source_id(&bundle, "A")).unwrap();
    let before = snapshot(store);
    let revoke_workload = WorkloadBindingRevocationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: grant.credential_id.clone(),
        expected_revision: 1,
    };
    let revoke_broker = BrokerRevocationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        broker_id: grant.broker_id.clone(),
        expected_revision: 1,
    };
    // Valid durable identities ensure these refusals cannot pass merely
    // because the broker/binding is absent or the revocation input is invalid.
    access(&bundle, "B", |b, wrong_access| {
        assert!(
            store
                .revoke_workload_binding_scoped(wrong_access, &revoke_workload)
                .is_err()
        );
        assert!(
            store
                .revoke_broker_scoped(wrong_access, &revoke_broker)
                .is_err()
        );
        assert!(
            store
                .broker_heartbeat_scoped(wrong_access, &identity, &heartbeat(&grant))
                .is_err()
        );
        assert!(store.broker_scoped(wrong_access, &grant.broker_id).is_err());
        assert!(
            store
                .broker_for_identity_scoped(wrong_access, &identity)
                .is_err()
        );
        assert!(
            store
                .broker_authenticated_identity_scoped(wrong_access, &identity)
                .is_err()
        );
        assert!(
            store
                .broker_for_authority_scoped(wrong_access, &grant.authorities[0].authority_id)
                .is_err()
        );
        assert!(
            b.authenticate_workload_token_scoped(wrong_access, TOKEN)
                .unwrap()
                .is_none()
        );
        assert!(
            b.broker_heartbeat_scoped(wrong_access, &identity, &heartbeat(&grant))
                .is_err()
        );
    });
    assert!(store.revoke_workload_binding(&revoke_workload).is_err());
    assert!(store.revoke_broker(&revoke_broker).is_err());
    assert!(
        store
            .broker_heartbeat(&identity, &heartbeat(&grant))
            .is_err()
    );
    assert!(store.broker_for_identity(&identity).is_err());
    assert!(store.broker_authenticated_identity(&identity).is_err());
    assert!(
        store
            .broker_for_authority(&grant.authorities[0].authority_id)
            .is_err()
    );
    assert_eq!(snapshot(store), before);
    access(&bundle, "A", |store, access| {
        assert_eq!(
            store
                .broker_scoped(access, &grant.broker_id)
                .unwrap()
                .unwrap()
                .broker
                .broker_id,
            grant.broker_id
        );
        assert_eq!(
            store
                .broker_for_identity_scoped(access, &identity)
                .unwrap()
                .broker
                .broker_id,
            grant.broker_id
        );
        let view = store
            .broker_authenticated_identity_scoped(access, &identity)
            .unwrap();
        assert_eq!(view.schema_version, 14);
        Store::broker_heartbeat_input_hash(&view, &heartbeat(&grant)).unwrap();
        assert_eq!(
            store
                .broker_for_authority_scoped(access, &grant.authorities[0].authority_id)
                .unwrap()
                .unwrap()
                .broker
                .broker_id,
            grant.broker_id
        );
        assert_eq!(
            store.brokers_page_scoped(access, None, 100).unwrap().len(),
            1
        );
        let digest: [u8; 32] = Sha256::digest(TOKEN.as_bytes()).into();
        assert_eq!(
            store
                .workload_digest_binding_scoped(access, &digest)
                .unwrap()
                .unwrap()
                .credential_id,
            grant.credential_id
        );
        assert!(
            store
                .workload_digest_binding_scoped(access, &[0; 32])
                .unwrap()
                .is_none()
        );
    });
    let heartbeat = heartbeat(&grant);
    let accepted = access(&bundle, "A", |store, access| {
        store
            .broker_heartbeat_scoped(access, &identity, &heartbeat)
            .unwrap()
    });
    let before = snapshot(store);
    access(&bundle, "A", |store, access| {
        let replay = store
            .broker_heartbeat_scoped(access, &identity, &heartbeat)
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.receipt, accepted.receipt);
        let mut divergent = heartbeat.clone();
        divergent.sequence = 2;
        assert_eq!(
            store
                .broker_heartbeat_scoped(access, &identity, &divergent)
                .unwrap_err()
                .downcast_ref::<BrokerRegistryFailureV1>(),
            Some(&BrokerRegistryFailureV1::Conflict)
        );
    });
    assert_eq!(snapshot(store), before);
    access(&bundle, "A", |store, access| {
        let replay = store.mutate_broker_grant_scoped(access, &grant).unwrap();
        assert!(replay.replayed);
        let mut divergent = issue.clone();
        divergent.expires_at = "2099-01-01T00:00:00.000Z".into();
        assert_eq!(
            store
                .issue_workload_binding_scoped(access, &divergent)
                .unwrap_err()
                .downcast_ref::<WorkloadFailureV1>(),
            Some(&WorkloadFailureV1::Conflict)
        );
        let mut duplicate_digest = issue.clone();
        duplicate_digest.operation_id = Uuid::new_v4().to_string();
        duplicate_digest.credential_id = Uuid::new_v4().to_string();
        assert_eq!(
            store
                .issue_workload_binding_scoped(access, &duplicate_digest)
                .unwrap_err()
                .downcast_ref::<WorkloadFailureV1>(),
            Some(&WorkloadFailureV1::Conflict)
        );
        let mut divergent = grant.clone();
        divergent.lease_seconds = 31;
        assert_eq!(
            store
                .mutate_broker_grant_scoped(access, &divergent)
                .unwrap_err()
                .downcast_ref::<BrokerRegistryFailureV1>(),
            Some(&BrokerRegistryFailureV1::Conflict)
        );
    });
    assert_eq!(snapshot(store), before);
    let revoke = WorkloadBindingRevocationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: grant.credential_id.clone(),
        expected_revision: 1,
    };
    access(&bundle, "A", |store, access| {
        store
            .revoke_workload_binding_scoped(access, &revoke)
            .unwrap();
        assert!(
            store
                .revoke_workload_binding_scoped(access, &revoke)
                .unwrap()
                .replayed
        );
        assert!(
            store
                .authenticate_workload_token_scoped(access, TOKEN)
                .is_err()
        );
        assert!(
            store
                .broker_heartbeat_scoped(access, &identity, &heartbeat)
                .is_err()
        );
        let digest: [u8; 32] = Sha256::digest(TOKEN.as_bytes()).into();
        assert_eq!(
            store
                .workload_digest_binding_scoped(access, &digest)
                .unwrap()
                .unwrap()
                .credential_id,
            grant.credential_id
        );
    });
    assert!(!format!("{:?}", snapshot(store)).contains(TOKEN));
    assert!(store.healthy());
}

#[test]
fn migrated_broker_revocation_refuses_old_authenticated_heartbeat_replay() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    let grant = grant();
    access(&bundle, "A", |store, access| {
        store
            .issue_workload_binding_scoped(access, &issue(&grant))
            .unwrap();
        store.mutate_broker_grant_scoped(access, &grant).unwrap();
    });
    let identity = identity(&bundle);
    let heartbeat = heartbeat(&grant);
    access(&bundle, "A", |store, access| {
        store
            .broker_heartbeat_scoped(access, &identity, &heartbeat)
            .unwrap();
        let revoke = BrokerRevocationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: grant.broker_id.clone(),
            expected_revision: 1,
        };
        store.revoke_broker_scoped(access, &revoke).unwrap();
        assert!(
            store
                .revoke_broker_scoped(access, &revoke)
                .unwrap()
                .replayed
        );
    });
    let store = bundle.store(source_id(&bundle, "A")).unwrap();
    let before = snapshot(store);
    access(&bundle, "A", |store, access| {
        assert!(
            store
                .authenticate_workload_token_scoped(access, TOKEN)
                .is_err()
        );
        assert!(store.broker_for_identity_scoped(access, &identity).is_err());
        assert!(
            store
                .broker_authenticated_identity_scoped(access, &identity)
                .is_err()
        );
        assert!(
            store
                .broker_heartbeat_scoped(access, &identity, &heartbeat)
                .is_err()
        );
    });
    assert_eq!(snapshot(store), before);
    assert!(store.healthy());
}

#[test]
fn migrated_workload_and_registry_refuse_foreign_sources_and_legacy_without_mutation() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    let grant = grant();
    let issue = issue(&grant);
    let a = bundle.store(source_id(&bundle, "A")).unwrap();
    let b = bundle.store(source_id(&bundle, "B")).unwrap();
    let before_a = snapshot(a);
    let before_b = snapshot(b);
    access(&bundle, "B", |_, wrong_access| {
        assert!(
            a.issue_workload_binding_scoped(wrong_access, &issue)
                .is_err()
        );
        assert!(a.mutate_broker_grant_scoped(wrong_access, &grant).is_err());
        assert!(
            a.authenticate_workload_token_scoped(wrong_access, TOKEN)
                .is_err()
        );
        assert!(a.brokers_page_scoped(wrong_access, None, 100).is_err());
        assert!(
            a.workload_digest_binding_scoped(wrong_access, &[0; 32])
                .is_err()
        );
    });
    access(&bundle, "A", |_, wrong_access| {
        assert!(
            b.issue_workload_binding_scoped(wrong_access, &issue)
                .is_err()
        );
        assert!(b.mutate_broker_grant_scoped(wrong_access, &grant).is_err());
    });
    assert!(a.issue_workload_binding(&issue).is_err());
    assert!(a.mutate_broker_grant(&grant).is_err());
    assert!(a.authenticate_workload_token(TOKEN).is_err());
    assert!(a.workload_digest_binding(&[0; 32]).is_err());
    assert!(a.broker(&grant.broker_id).is_err());
    assert!(a.brokers_page(None, 100).is_err());
    assert_eq!(snapshot(a), before_a);
    assert_eq!(snapshot(b), before_b);
    assert!(a.healthy() && b.healthy());
}

#[test]
fn migrated_workload_and_registry_refuse_expired_epoch_without_mutation() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    let source = bundle
        .owner()
        .source_token(source_id(&bundle, "A"))
        .unwrap();
    let epoch = bundle.owner().try_write_epoch().unwrap();
    let expired_access = HumanSourceAccess::write(&epoch, &source).unwrap();
    let store = bundle.store(source.source_id()).unwrap();
    let before = snapshot(store);
    // Observe the actual fixed lease; never extend or fabricate its deadline.
    std::thread::sleep(
        epoch
            .test_deadline()
            .saturating_duration_since(std::time::Instant::now()),
    );
    let grant = grant();
    assert_eq!(
        store
            .issue_workload_binding_scoped(&expired_access, &issue(&grant))
            .unwrap_err()
            .downcast_ref::<HumanAuthorityFailure>(),
        Some(&HumanAuthorityFailure::EpochExpired)
    );
    assert_eq!(
        store
            .mutate_broker_grant_scoped(&expired_access, &grant)
            .unwrap_err()
            .downcast_ref::<HumanAuthorityFailure>(),
        Some(&HumanAuthorityFailure::EpochExpired)
    );
    assert!(
        store
            .brokers_page_scoped(&expired_access, None, 100)
            .is_err()
    );
    assert_eq!(snapshot(store), before);
    assert!(store.healthy());
    drop(expired_access);
    drop(epoch);
    access(&bundle, "A", |store, access| {
        store
            .issue_workload_binding_scoped(access, &issue(&grant))
            .unwrap();
    });
}

#[test]
fn migrated_workload_and_registry_refuse_invalidated_owner_and_closed_admission() {
    let fixture = Fixture::new();
    let bundle = fixture.migrated();
    let source = bundle
        .owner()
        .source_token(source_id(&bundle, "A"))
        .unwrap();
    let epoch = bundle.owner().try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &source).unwrap();
    let store = bundle.store(source.source_id()).unwrap();
    let before = snapshot(store);
    bundle.owner().begin_close();
    assert!(bundle.owner().try_read_epoch().is_err());
    // Closing drains accepted work. It does not revoke an already admitted epoch.
    assert!(
        store
            .brokers_page_scoped(&access, None, 100)
            .unwrap()
            .is_empty()
    );
    bundle
        .owner()
        .invalidate_authority(HumanAuthorityInvalidationReason::DurableAuthorityMismatch);
    let grant = grant();
    assert!(
        store
            .issue_workload_binding_scoped(&access, &issue(&grant))
            .is_err()
    );
    assert!(store.mutate_broker_grant_scoped(&access, &grant).is_err());
    assert!(store.brokers_page_scoped(&access, None, 100).is_err());
    assert_eq!(snapshot(store), before);
}
