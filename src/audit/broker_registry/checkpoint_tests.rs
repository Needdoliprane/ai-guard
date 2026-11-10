use super::*;
use crate::workload::{AuthenticatedWorkload, WorkloadBindingIssueV1, WorkloadSubjectV1};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};
use uuid::Uuid;

fn store(path: &Path, tenant: &str) -> Store {
    let store = Store::open_for_tenant(path, tenant, Redactor::new(vec![]), 10_000).unwrap();
    store
        .migrate_control_v3(&path.with_extension("before3"))
        .unwrap();
    store
        .migrate_identity_v4(&path.with_extension("before4"))
        .unwrap();
    store
        .migrate_human_access_v5(&path.with_extension("before5"))
        .unwrap();
    store
        .migrate_workload_v6(&path.with_extension("before6"))
        .unwrap();
    store
        .migrate_human_brokers_v7(&path.with_extension("before7"))
        .unwrap();
    store
        .migrate_kubernetes_create_v8(&path.with_extension("before8"))
        .unwrap();
    store
        .migrate_kubernetes_policy_v9(&path.with_extension("before9"))
        .unwrap();
    store
        .migrate_human_workers_v10(&path.with_extension("before10"))
        .unwrap();
    store
        .migrate_kubernetes_session_reservations_v11(&path.with_extension("before11"))
        .unwrap();
    store
}

fn grant(store: &Store) -> (BrokerIdentityV1, BrokerCheckpointV1) {
    let broker = Uuid::new_v4().to_string();
    let credential = Uuid::new_v4().to_string();
    let incarnation = Uuid::new_v4().to_string();
    let token = format!("checkpoint-public-fixture-proof-{}", Uuid::new_v4());
    store
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential.clone(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: broker.clone(),
                incarnation_id: incarnation.clone(),
            },
            sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
            expires_at: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    let authority = BrokerAuthorityV1 {
        authority_id: format!("checkpoint-test-host-{broker}"),
        runtime_kind: BrokerRuntimeKindV1::Docker,
    };
    store
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: broker,
            expected_revision: None,
            credential_id: credential,
            incarnation_id: incarnation.clone(),
            authorities: vec![authority.clone()],
            lease_seconds: 30,
            not_after: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    let AuthenticatedWorkload::Broker(identity) =
        store.authenticate_workload_token(&token).unwrap().unwrap()
    else {
        panic!("real fixture binding must authenticate as Broker");
    };
    let checkpoint = BrokerCheckpointV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        incarnation_id: incarnation,
        previous_sequence: 0,
        sequence: 1,
        snapshot: BrokerCheckpointSnapshotV1 {
            version: 1,
            authority,
            jobs: vec![BrokerJobCheckpointV1 {
                job_id: Uuid::new_v4().to_string(),
                revision: 17,
                checkpoint_hash: "a".repeat(64),
            }],
            has_more: false,
        },
    };
    (identity, checkpoint)
}

#[test]
fn checkpoint_is_schema_gated_and_exact_replay_survives_reopen_without_heartbeat_or_quota_effect() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = root.path().join("tenant.sqlite");
    let store = store(&path, "tenant-a");
    let (identity, checkpoint) = grant(&store);
    assert!(store.broker_checkpoint(&identity, &checkpoint).is_err());
    store
        .migrate_broker_checkpoints_v12(&path.with_extension("before12"))
        .unwrap();
    let before = store.broker(identity.broker_id()).unwrap().unwrap().broker;
    let quota = store
        .worker_quota(&crate::control::QuotaPolicyV1::default())
        .unwrap();
    let first = store.broker_checkpoint(&identity, &checkpoint).unwrap();
    assert!(!first.replayed);
    assert_eq!(first.checkpoint, checkpoint);
    assert_eq!(
        store.broker(identity.broker_id()).unwrap().unwrap().broker,
        before
    );
    assert_eq!(
        serde_json::to_value(
            store
                .worker_quota(&crate::control::QuotaPolicyV1::default())
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(quota).unwrap()
    );
    let mut divergent = checkpoint.clone();
    divergent.snapshot.jobs[0].revision += 1;
    assert!(store.broker_checkpoint(&identity, &divergent).is_err());
    let view = store.broker_checkpoint_for_identity(&identity).unwrap();
    let observation = view.checkpoint.unwrap();
    assert_eq!(
        observation.coverage,
        BrokerCheckpointCoverageV1::DeclaredPartial
    );
    assert_eq!(observation.checkpoint, checkpoint);
    drop(store);
    let reopened =
        Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap();
    let replay = reopened.broker_checkpoint(&identity, &checkpoint).unwrap();
    assert!(replay.replayed);
    assert_eq!(first.receipt, replay.receipt);
    assert_eq!(
        reopened
            .broker_checkpoint_for_identity(&identity)
            .unwrap()
            .checkpoint
            .unwrap()
            .checkpoint,
        checkpoint
    );
}

#[test]
fn checkpoint_refuses_foreign_tenant_or_authority_and_revocation_before_exact_replay() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let a_path = root.path().join("a.sqlite");
    let b_path = root.path().join("b.sqlite");
    let a = store(&a_path, "tenant-a");
    let b = store(&b_path, "tenant-b");
    let (identity, checkpoint) = grant(&a);
    a.migrate_broker_checkpoints_v12(&a_path.with_extension("before12"))
        .unwrap();
    b.migrate_broker_checkpoints_v12(&b_path.with_extension("before12"))
        .unwrap();
    assert!(b.broker_checkpoint(&identity, &checkpoint).is_err());
    let mut outside = checkpoint.clone();
    outside.snapshot.authority.authority_id = "other-host".into();
    assert!(a.broker_checkpoint(&identity, &outside).is_err());
    a.broker_checkpoint(&identity, &checkpoint).unwrap();
    a.revoke_broker(&BrokerRevocationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        broker_id: identity.broker_id().into(),
        expected_revision: 1,
    })
    .unwrap();
    assert!(a.broker_checkpoint(&identity, &checkpoint).is_err());
    assert!(a.broker_checkpoint_for_identity(&identity).is_err());
}

#[test]
fn checkpoint_audit_refusal_commits_no_operation_or_projection() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = root.path().join("tenant.sqlite");
    let store = store(&path, "tenant-a");
    let (identity, checkpoint) = grant(&store);
    store
        .migrate_broker_checkpoints_v12(&path.with_extension("before12"))
        .unwrap();
    store.db.lock().unwrap().execute_batch("CREATE TEMP TRIGGER refuse_checkpoint BEFORE INSERT ON events WHEN NEW.kind='control.broker_registry.checkpoint' BEGIN SELECT RAISE(ABORT,'fixture audit refused'); END;").unwrap();
    let error = store.broker_checkpoint(&identity, &checkpoint).unwrap_err();
    assert!(
        format!("{error:#}").contains("fixture audit refused"),
        "the oracle must reach the actual mandatory audit insert"
    );
    let count: i64 = store
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM broker_registry_operations WHERE operation_id=?1",
            [&checkpoint.operation_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 0,
        "failed mandatory audit must leave no checkpoint operation"
    );
}

#[test]
fn two_brokers_share_one_tenant_without_choosing_each_others_checkpoint_sequence() {
    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = root.path().join("tenant.sqlite");
    let store = store(&path, "tenant-a");
    let (a, first) = grant(&store);
    let (b, second) = grant(&store);
    store
        .migrate_broker_checkpoints_v12(&path.with_extension("before12"))
        .unwrap();
    store.broker_checkpoint(&a, &first).unwrap();
    store.broker_checkpoint(&b, &second).unwrap();
    assert_eq!(
        store
            .broker_checkpoint_for_identity(&a)
            .unwrap()
            .checkpoint
            .unwrap()
            .checkpoint,
        first
    );
    assert_eq!(
        store
            .broker_checkpoint_for_identity(&b)
            .unwrap()
            .checkpoint
            .unwrap()
            .checkpoint,
        second
    );
    let mut borrowed = first.clone();
    borrowed.operation_id = Uuid::new_v4().to_string();
    assert!(store.broker_checkpoint(&b, &borrowed).is_err());
}
