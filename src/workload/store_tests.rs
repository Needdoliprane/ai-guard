use super::*;
use crate::broker_registry::*;
fn private_tempdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;

    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("create a private workload test directory")
}

fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn open(path: &Path, tenant: &str, capacity: u64) -> Store {
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
fn grant() -> BrokerGrantMutationV1 {
    BrokerGrantMutationV1 {
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
        not_after: "2100-01-01T00:00:00.000Z".into(),
    }
}
fn issue(grant: &BrokerGrantMutationV1, token: &str) -> WorkloadBindingIssueV1 {
    WorkloadBindingIssueV1 {
        version: 1,
        operation_id: id(),
        credential_id: grant.credential_id.clone(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Broker {
            broker_id: grant.broker_id.clone(),
            incarnation_id: grant.incarnation_id.clone(),
        },
        sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
        expires_at: "2100-01-01T00:00:00.000Z".into(),
    }
}
fn identity(store: &Store, token: &str) -> BrokerIdentityV1 {
    match store.authenticate_workload_token(token).unwrap().unwrap() {
        AuthenticatedWorkload::Broker(identity) => identity,
        AuthenticatedWorkload::Worker(_) => panic!("broker token became worker"),
    }
}
fn heartbeat(grant: &BrokerGrantMutationV1, revision: u64, sequence: u64) -> BrokerHeartbeatV1 {
    BrokerHeartbeatV1 {
        version: 1,
        operation_id: id(),
        expected_heartbeat_revision: revision,
        incarnation_id: grant.incarnation_id.clone(),
        sequence,
    }
}

#[test]
fn broker_workload_token_drives_durable_heartbeat_replay_and_tenant_isolation() {
    let directory = private_tempdir();
    let path = directory.path().join("a.sqlite");
    let a = open(&path, "tenant-a", 10000);
    let b = open(&directory.path().join("b.sqlite"), "tenant-b", 10000);
    let token = "broker-a-credential-test-material-0123456789abcdef";
    let grant = grant();
    a.mutate_broker_grant(&grant).unwrap();
    let issue = issue(&grant, token);
    let issued = a.issue_workload_binding(&issue).unwrap();
    assert_eq!(issued.revision, 1);
    assert!(a.issue_workload_binding(&issue).unwrap().replayed);
    let identity = identity(&a, token);
    let first = heartbeat(&grant, 0, 1);
    let accepted = a.broker_heartbeat(&identity, &first).unwrap();
    assert_eq!(accepted.broker.heartbeat_revision, 1);
    let head = a.head().unwrap();
    let replay = a.broker_heartbeat(&identity, &first).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.receipt, accepted.receipt);
    assert_eq!(a.head().unwrap(), head);
    let mut divergent = first.clone();
    divergent.sequence = 2;
    assert!(a.broker_heartbeat(&identity, &divergent).is_err());
    assert!(b.broker_heartbeat(&identity, &first).is_err());
    assert!(b.brokers_page(None, 100).unwrap().is_empty());
    let second = heartbeat(&grant, 1, 2);
    a.broker_heartbeat(&identity, &second).unwrap();
    let last = a
        .broker(&grant.broker_id)
        .unwrap()
        .unwrap()
        .broker
        .last_heartbeat;
    a.broker_heartbeat(&identity, &first).unwrap();
    assert_eq!(
        a.broker(&grant.broker_id)
            .unwrap()
            .unwrap()
            .broker
            .last_heartbeat,
        last
    );
    let events = a
        .events(&EventQuery {
            limit: Some(100),
            ..Default::default()
        })
        .unwrap();
    assert!(!serde_json::to_string(&events).unwrap().contains(token));
    drop(a);
    let a =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10000).unwrap();
    assert_eq!(
        a.broker(&grant.broker_id)
            .unwrap()
            .unwrap()
            .broker
            .heartbeat_revision,
        2
    );
    assert!(a.broker_heartbeat(&identity, &second).unwrap().replayed);
    assert!(a.integrity().unwrap().valid);
}

#[test]
fn binding_revocation_and_rotation_are_checked_before_old_operation_replay() {
    let directory = private_tempdir();
    let path = directory.path().join("a.sqlite");
    let a = open(&path, "tenant-a", 10000);
    let token = "broker-a-credential-test-material-0123456789abcdef";
    let replacement = "broker-a-replacement-test-material-0123456789abcdef";
    let grant = grant();
    a.mutate_broker_grant(&grant).unwrap();
    let original = issue(&grant, token);
    a.issue_workload_binding(&original).unwrap();
    let old = identity(&a, token);
    let first = heartbeat(&grant, 0, 1);
    a.broker_heartbeat(&old, &first).unwrap();
    let revoke = WorkloadBindingRevocationV1 {
        version: 1,
        operation_id: id(),
        credential_id: grant.credential_id.clone(),
        expected_revision: 1,
    };
    a.revoke_workload_binding(&revoke).unwrap();
    let head = a.head().unwrap();
    assert!(a.broker_heartbeat(&old, &first).is_err());
    assert!(a.authenticate_workload_token(token).is_err());
    assert_eq!(a.head().unwrap(), head);
    let mut rotate = issue(&grant, replacement);
    rotate.expected_revision = 2;
    a.issue_workload_binding(&rotate).unwrap();
    assert!(a.authenticate_workload_token(token).unwrap().is_none());
    assert!(a.broker_heartbeat(&old, &first).is_err());
    let current = identity(&a, replacement);
    assert_eq!(current.binding_revision(), 3);
    a.broker_heartbeat(&current, &heartbeat(&grant, 1, 2))
        .unwrap();
    let mut reused = issue(&grant, token);
    reused.credential_id = id();
    assert!(
        a.issue_workload_binding(&reused).is_err(),
        "historical digest owner cannot change"
    );
    let mut steal = rotate;
    steal.operation_id = id();
    steal.expected_revision = 3;
    steal.subject = WorkloadSubjectV1::Broker {
        broker_id: id(),
        incarnation_id: id(),
    };
    assert!(a.issue_workload_binding(&steal).is_err());
    drop(a);
    let a =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10000).unwrap();
    assert!(
        a.integrity().unwrap().valid,
        "historical heartbeat remains valid after later revocation/rotation"
    );
}

#[test]
fn expiry_and_broker_rearm_reject_old_workload_proofs_without_refreshing_liveness() {
    let directory = private_tempdir();
    let path = directory.path().join("a.sqlite");
    let a = open(&path, "tenant-a", 10000);
    let token = "broker-a-credential-test-material-0123456789abcdef";
    let grant = grant();
    a.mutate_broker_grant(&grant).unwrap();
    let issue = issue(&grant, token);
    a.issue_workload_binding(&issue).unwrap();
    let old = identity(&a, token);
    let first = heartbeat(&grant, 0, 1);
    a.broker_heartbeat(&old, &first).unwrap();
    {
        let db = a.db.lock().unwrap();
        assert!(
            revalidate_broker_identity(&db, &old, parse_time(&issue.expires_at).unwrap()).is_err()
        );
    }
    let mut rearm = grant.clone();
    rearm.operation_id = id();
    rearm.expected_revision = Some(1);
    rearm.incarnation_id = id();
    rearm.credential_id = id();
    a.mutate_broker_grant(&rearm).unwrap();
    let head = a.head().unwrap();
    assert!(a.broker_heartbeat(&old, &first).is_err());
    assert!(a.authenticate_workload_token(token).is_err());
    assert_eq!(a.head().unwrap(), head);
    let view = a.broker(&grant.broker_id).unwrap().unwrap();
    assert_eq!(view.state, BrokerStateV1::Unseen);
    assert_eq!(view.broker.last_heartbeat.as_ref().unwrap().sequence, 1);
    assert!(!view.runtime_inventory_complete);
    assert!(a.integrity().unwrap().valid);
}

#[test]
fn heartbeat_budget_exhaustion_preserves_control_mutations_and_binding_revocation() {
    let directory = private_tempdir();
    let path = directory.path().join("a.sqlite");
    let a = open(&path, "tenant-a", 64);
    let token = "broker-a-credential-test-material-0123456789abcdef";
    let broker_grant = grant();
    a.mutate_broker_grant(&broker_grant).unwrap();
    a.issue_workload_binding(&issue(&broker_grant, token))
        .unwrap();
    let identity = identity(&a, token);
    let mut accepted = 0;
    for _ in 0..64 {
        if a.broker_heartbeat(&identity, &heartbeat(&broker_grant, accepted, accepted + 1))
            .is_err()
        {
            break;
        }
        accepted += 1;
    }
    assert!(
        accepted > 0 && accepted < 64,
        "the private workload budget must bound heartbeats"
    );
    let head = a.head().unwrap();
    assert!(
        a.broker_heartbeat(&identity, &heartbeat(&broker_grant, accepted, accepted + 1))
            .is_err()
    );
    assert_eq!(a.head().unwrap(), head);
    assert_eq!(
        a.broker(&broker_grant.broker_id)
            .unwrap()
            .unwrap()
            .broker
            .heartbeat_revision,
        accepted
    );
    a.mutate_broker_grant(&grant()).unwrap();
    a.revoke_workload_binding(&WorkloadBindingRevocationV1 {
        version: 1,
        operation_id: id(),
        credential_id: broker_grant.credential_id,
        expected_revision: 1,
    })
    .unwrap();
    assert!(a.authenticate_workload_token(token).is_err());
    assert_eq!(a.head().unwrap(), head + 2);
    assert!(a.integrity().unwrap().valid);
}

#[test]
fn canonical_broker_budget_survives_rotation_incarnation_siblings_external_commits_and_reopen() {
    let directory = private_tempdir();
    let path = directory.path().join("a.sqlite");
    let policy = WorkloadAuditPolicyV1 {
        version: 1,
        tenant: AuditCapacityV1 {
            events: 16,
            bytes: 131072,
        },
        principal: AuditCapacityV1 {
            events: 2,
            bytes: 65536,
        },
    };
    let a = open(&path, "tenant-a", 128)
        .with_workload_audit_policy(policy.clone())
        .unwrap();
    let token_a = "broker-a-credential-test-material-0123456789abcdef";
    let token_b = "broker-b-credential-test-material-0123456789abcdef";
    let broker_a = grant();
    let broker_b = grant();
    for (grant, token) in [(&broker_a, token_a), (&broker_b, token_b)] {
        a.mutate_broker_grant(grant).unwrap();
        a.issue_workload_binding(&issue(grant, token)).unwrap();
    }
    let identity_a = identity(&a, token_a);
    let identity_b = identity(&a, token_b);
    let sibling = a.sibling().unwrap();
    assert_eq!(sibling.workload_audit_policy(), &policy);
    a.broker_heartbeat(&identity_a, &heartbeat(&broker_a, 0, 1))
        .unwrap();
    sibling
        .broker_heartbeat(&identity_a, &heartbeat(&broker_a, 1, 2))
        .unwrap();
    assert!(
        a.broker_heartbeat(&identity_a, &heartbeat(&broker_a, 2, 3))
            .is_err()
    );
    assert!(a.healthy());
    let external = Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 128)
        .unwrap()
        .with_workload_audit_policy(policy.clone())
        .unwrap();
    external
        .broker_heartbeat(&identity_b, &heartbeat(&broker_b, 0, 1))
        .unwrap();
    assert!(a.integrity().unwrap().valid);
    assert!(
        sibling
            .broker_heartbeat(&identity_a, &heartbeat(&broker_a, 2, 3))
            .is_err()
    );
    let replacement = "broker-a-replacement-test-material-0123456789abcdef";
    let mut rotate = issue(&broker_a, replacement);
    rotate.expected_revision = 1;
    a.issue_workload_binding(&rotate).unwrap();
    let rotated = identity(&a, replacement);
    assert!(
        a.broker_heartbeat(&rotated, &heartbeat(&broker_a, 2, 3))
            .is_err()
    );
    let new_token = "broker-a-new-incarnation-test-material-0123456789abcdef";
    let mut rearm = broker_a.clone();
    rearm.operation_id = id();
    rearm.expected_revision = Some(1);
    rearm.credential_id = id();
    rearm.incarnation_id = id();
    a.mutate_broker_grant(&rearm).unwrap();
    a.issue_workload_binding(&issue(&rearm, new_token)).unwrap();
    let rearmed = identity(&a, new_token);
    assert!(
        a.broker_heartbeat(&rearmed, &heartbeat(&rearm, 2, 1))
            .is_err()
    );
    let scanned = a.supervision_lease.capacity.lock().unwrap().scanned;
    for _ in 0..3 {
        a.broker(&broker_a.broker_id).unwrap();
    }
    assert_eq!(
        a.supervision_lease.capacity.lock().unwrap().scanned,
        scanned,
        "stable metadata reads do not rescan the chain"
    );
    a.mutate_broker_grant(&grant()).unwrap();
    a.revoke_workload_binding(&WorkloadBindingRevocationV1 {
        version: 1,
        operation_id: id(),
        credential_id: broker_b.credential_id,
        expected_revision: 1,
    })
    .unwrap();
    drop((a, sibling, external));
    let reopened = Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 128)
        .unwrap()
        .with_workload_audit_policy(policy)
        .unwrap();
    let rearmed = identity(&reopened, new_token);
    assert!(
        reopened
            .broker_heartbeat(&rearmed, &heartbeat(&rearm, 2, 1))
            .is_err()
    );
    assert!(reopened.healthy());
}

#[test]
fn historical_digest_owner_survives_rotation_and_revocation_without_authenticating_it() {
    let directory = private_tempdir();
    let path = directory.path().join("a.sqlite");
    let store = open(&path, "tenant-a", 1000);
    let token = "broker-a-credential-test-material-0123456789abcdef";
    let replacement = "broker-a-replacement-test-material-0123456789abcdef";
    let broker = grant();
    store.mutate_broker_grant(&broker).unwrap();
    let binding = issue(&broker, token);
    store.issue_workload_binding(&binding).unwrap();
    let old_digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let new_digest: [u8; 32] = Sha256::digest(replacement.as_bytes()).into();
    let expected = WorkloadDigestOwnerV1 {
        credential_id: binding.credential_id.clone(),
        subject: binding.subject.clone(),
    };
    assert_eq!(
        store.workload_digest_binding(&old_digest).unwrap(),
        Some(expected.clone())
    );
    let mut rotate = issue(&broker, replacement);
    rotate.expected_revision = 1;
    store.issue_workload_binding(&rotate).unwrap();
    assert_eq!(
        store.workload_digest_binding(&old_digest).unwrap(),
        Some(expected.clone())
    );
    assert!(store.authenticate_workload_token(token).unwrap().is_none());
    store
        .revoke_workload_binding(&WorkloadBindingRevocationV1 {
            version: 1,
            operation_id: id(),
            credential_id: broker.credential_id,
            expected_revision: 2,
        })
        .unwrap();
    assert_eq!(
        store.workload_digest_binding(&old_digest).unwrap(),
        Some(expected.clone())
    );
    assert_eq!(
        store.workload_digest_binding(&new_digest).unwrap(),
        Some(expected)
    );
    assert!(store.authenticate_workload_token(replacement).is_err());
    assert!(store.workload_digest_binding(&[0; 32]).unwrap().is_none());
    drop(store);
    let store =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert!(
        store
            .workload_digest_binding(&old_digest)
            .unwrap()
            .is_some()
    );
    let db = Connection::open(&path).unwrap();
    db.execute(
        "UPDATE workload_digests SET sha256_digest=?1 WHERE sha256_digest=?2",
        params![hex::encode([0; 32]), hex::encode(old_digest)],
    )
    .unwrap();
    assert!(
        store.workload_digest_binding(&old_digest).is_err(),
        "an altered digest projection must fail closed, not become an absent credential"
    );
    assert!(!store.healthy());
}

#[test]
fn healthy_pre_v6_store_has_no_workload_digest_owner() {
    let directory = private_tempdir();
    let path = directory.path().join("v5.sqlite");
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
    let head = store.head().unwrap();
    assert!(store.workload_digest_binding(&[0; 32]).unwrap().is_none());
    assert_eq!(store.head().unwrap(), head);
    assert!(store.healthy());
}

#[test]
fn broker_self_view_revalidates_proof_without_refreshing_receipt_liveness() {
    let directory = private_tempdir();
    let path = directory.path().join("a.sqlite");
    let store = open(&path, "tenant-a", 1000);
    let other = open(&directory.path().join("b.sqlite"), "tenant-b", 1000);
    let token = "broker-a-credential-test-material-0123456789abcdef";
    let broker = grant();
    store.mutate_broker_grant(&broker).unwrap();
    store
        .issue_workload_binding(&issue(&broker, token))
        .unwrap();
    let proof = identity(&store, token);
    let head = store.head().unwrap();
    let view = store.broker_for_identity(&proof).unwrap();
    assert_eq!(view.broker.broker_id, broker.broker_id);
    assert_eq!(view.state, BrokerStateV1::Unseen);
    assert!(!view.runtime_inventory_complete);
    assert_eq!(store.head().unwrap(), head);
    store
        .broker_heartbeat(&proof, &heartbeat(&broker, 0, 1))
        .unwrap();
    let head = store.head().unwrap();
    let receipt = store
        .broker_for_identity(&proof)
        .unwrap()
        .broker
        .last_heartbeat;
    assert_eq!(
        store
            .broker_for_identity(&proof)
            .unwrap()
            .broker
            .last_heartbeat,
        receipt
    );
    assert_eq!(store.head().unwrap(), head);
    assert!(other.broker_for_identity(&proof).is_err());
    store
        .revoke_workload_binding(&WorkloadBindingRevocationV1 {
            version: 1,
            operation_id: id(),
            credential_id: broker.credential_id,
            expected_revision: 1,
        })
        .unwrap();
    assert!(store.broker_for_identity(&proof).is_err());
}

#[test]
fn issuing_a_historical_digest_cannot_restore_rotated_or_revoked_keys() {
    let directory = private_tempdir();
    let path = directory.path().join("a.sqlite");
    let store = open(&path, "tenant-a", 1000);
    let original_token = "broker-a-credential-test-material-0123456789abcdef";
    let replacement = "broker-a-replacement-test-material-0123456789abcdef";
    let fresh = "broker-a-after-revocation-test-material-0123456789abcdef";
    let broker = grant();
    store.mutate_broker_grant(&broker).unwrap();
    let original = issue(&broker, original_token);
    store.issue_workload_binding(&original).unwrap();
    let mut renewal = issue(&broker, original_token);
    renewal.expected_revision = 1;
    store.issue_workload_binding(&renewal).unwrap();
    let mut rotate = issue(&broker, replacement);
    rotate.expected_revision = 2;
    store.issue_workload_binding(&rotate).unwrap();
    let head = store.head().unwrap();
    let mut old = issue(&broker, original_token);
    old.expected_revision = 3;
    assert!(
        store.issue_workload_binding(&old).is_err(),
        "rotation cannot be undone by issuing the old digest"
    );
    assert!(
        store
            .authenticate_workload_token(original_token)
            .unwrap()
            .is_none()
    );
    assert_eq!(store.head().unwrap(), head);
    assert!(
        store.issue_workload_binding(&original).unwrap().replayed,
        "old exact operator receipt does not restore its binding"
    );
    assert!(
        store
            .authenticate_workload_token(original_token)
            .unwrap()
            .is_none()
    );
    assert_eq!(store.head().unwrap(), head);
    store
        .revoke_workload_binding(&WorkloadBindingRevocationV1 {
            version: 1,
            operation_id: id(),
            credential_id: broker.credential_id.clone(),
            expected_revision: 3,
        })
        .unwrap();
    let head = store.head().unwrap();
    let mut revoked = issue(&broker, replacement);
    revoked.expected_revision = 4;
    assert!(
        store.issue_workload_binding(&revoked).is_err(),
        "revoked current key must not be reactivated"
    );
    assert!(store.authenticate_workload_token(replacement).is_err());
    assert_eq!(store.head().unwrap(), head);
    let mut reissue = issue(&broker, fresh);
    reissue.expected_revision = 4;
    store.issue_workload_binding(&reissue).unwrap();
    assert!(store.authenticate_workload_token(fresh).unwrap().is_some());
    drop(store);
    let store =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap();
    assert!(store.integrity().unwrap().valid);
    assert!(
        store
            .authenticate_workload_token(original_token)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .authenticate_workload_token(replacement)
            .unwrap()
            .is_none()
    );
}
