//! Real SQLite migrations and authenticated broker capabilities; no runtime or network.
use super::*;
use crate::audit::kubernetes_admission_state_tests::Fixture;
use crate::control::QuotaPolicyV1;
use uuid::Uuid;

const EXPIRY: &str = "2100-01-01T00:00:00.000Z";

fn policy(intent: &KubernetesCreateIntentV1) -> KubernetesHeldCreatePolicyV1 {
    KubernetesHeldCreatePolicyV1 {
        version: 1,
        decision_ttl_seconds: 300,
        approvals: vec![KubernetesHeldCreateApprovalV1 {
            authority_fingerprint: intent.authority.fingerprint(),
            image: intent.image.clone(),
            profile: intent.profile.clone(),
        }],
    }
}

fn migrate_to_12(fixture: &Fixture) {
    let path = fixture._directory.path();
    fixture
        .store
        .migrate_kubernetes_policy_v9(&path.join("before9.sqlite"))
        .unwrap();
    migrate_9_to_12(fixture);
}

fn migrate_9_to_12(fixture: &Fixture) {
    let path = fixture._directory.path();
    fixture
        .store
        .migrate_human_workers_v10(&path.join("before10.sqlite"))
        .unwrap();
    fixture
        .store
        .migrate_kubernetes_session_reservations_v11(&path.join("before11.sqlite"))
        .unwrap();
    fixture
        .store
        .migrate_broker_checkpoints_v12(&path.join("before12.sqlite"))
        .unwrap();
    let state = fixture.store.supervision_state().unwrap();
    assert_eq!(state.schema_version, 12);
    assert_eq!(
        state.control_namespace_uuid.as_deref(),
        Some(fixture.intent.control_namespace_uuid.as_str())
    );
    assert!(fixture.store.integrity().unwrap().valid);
    let markers = fixture
        .store
        .events(&EventQuery {
            kind: Some("control.broker_checkpoints.genesis".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(markers.len(), 1);
}

fn cancel(intent: &KubernetesCreateIntentV1) -> KubernetesCreateCancelV1 {
    KubernetesCreateCancelV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: intent.create_id.clone(),
        intent_hash: intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
    }
}

fn assert_failure<T>(result: anyhow::Result<T>, expected: KubernetesCreateFailureV1) {
    let error = result
        .err()
        .expect("the incompatible request must be refused");
    assert_eq!(
        error.downcast_ref::<KubernetesCreateFailureV1>(),
        Some(&expected)
    );
}

fn assert_quota(store: &Store, workers: u32) {
    let usage = store.worker_quota(&QuotaPolicyV1::default()).unwrap();
    assert_eq!(usage.held_workers, workers);
    assert_eq!(usage.held_cpu_millis, u64::from(workers) * 500);
    assert_eq!(usage.held_memory_bytes, u64::from(workers) * 134_217_728);
}

fn assert_v2_event(store: &Store, kind: &str, receipt: &KubernetesCreateReceiptV1) {
    let events = store
        .events(&EventQuery {
            kind: Some(kind.into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(events.len(), 1);
    let stored = &events[0];
    assert_eq!(stored.seq, receipt.audit_seq);
    assert_eq!(stored.hash, receipt.audit_hash);
    assert_eq!(stored.event.id, receipt.operation_id);
    assert_eq!(stored.event.data["version"], 2);
    assert_eq!(stored.event.data["control_decision"]["version"], 2);
}

#[test]
fn schema_and_scope_refusals_preserve_audit_and_quota_across_real_migrations() {
    let fixture = Fixture::new();
    let quota = QuotaPolicyV1::default();
    let legacy = fixture
        .store
        .authorize_held_create_for_test(&fixture.identity, &fixture.intent, &quota, EXPIRY)
        .unwrap();
    let head8 = fixture.store.head().unwrap();
    assert_failure(
        fixture.store.authorize_kubernetes_create_admission(
            &fixture.identity,
            &fixture.intent,
            &policy(&fixture.intent),
            &quota,
        ),
        KubernetesCreateFailureV1::MigrationRequired,
    );
    assert_eq!(fixture.store.head().unwrap(), head8);
    assert_quota(&fixture.store, 0);

    fixture
        .store
        .migrate_kubernetes_policy_v9(&fixture._directory.path().join("before9.sqlite"))
        .unwrap();
    let admission = fixture
        .store
        .authorize_kubernetes_create_admission(
            &fixture.identity,
            &fixture.intent,
            &policy(&fixture.intent),
            &quota,
        )
        .unwrap();
    let (_, reserved, replayed) = fixture
        .store
        .reserve_kubernetes_create(&fixture.identity, &admission, &fixture.intent)
        .unwrap();
    assert!(!replayed && reserved.quota_held);
    assert_eq!(reserved.state, KubernetesCreateStateV1::Reserved);
    let head9 = fixture.store.head().unwrap();
    assert_failure(
        fixture.store.cancel_kubernetes_create(
            &fixture.identity,
            &admission,
            &cancel(&fixture.intent),
        ),
        KubernetesCreateFailureV1::PermissionDenied,
    );
    let reconcile = fixture
        .store
        .authorize_kubernetes_create_reconciliation(
            &fixture.identity,
            &fixture.intent.create_id,
            &quota,
        )
        .unwrap();
    assert_failure(
        fixture
            .store
            .reserve_kubernetes_create(&fixture.identity, &reconcile, &fixture.intent),
        KubernetesCreateFailureV1::PermissionDenied,
    );
    assert_eq!(fixture.store.head().unwrap(), head9);
    assert_quota(&fixture.store, 1);

    migrate_9_to_12(&fixture);
    let head12 = fixture.store.head().unwrap();
    assert_failure(
        fixture
            .store
            .reserve_kubernetes_create(&fixture.identity, &legacy, &fixture.intent),
        KubernetesCreateFailureV1::MigrationRequired,
    );
    assert_failure(
        fixture.store.cancel_kubernetes_create(
            &fixture.identity,
            &legacy,
            &cancel(&fixture.intent),
        ),
        KubernetesCreateFailureV1::MigrationRequired,
    );
    assert_eq!(fixture.store.head().unwrap(), head12);
    assert_quota(&fixture.store, 1);
    assert!(fixture.store.integrity().unwrap().valid);
    eprintln!("schema12_canary: real migrations, V1 refusals and V2 scope refusals verified");
}

#[test]
fn schema12_v2_admission_reserves_once_and_reopens_its_exact_audit() {
    let Fixture {
        _directory,
        store,
        identity,
        intent,
    } = {
        let fixture = Fixture::new();
        migrate_to_12(&fixture);
        fixture
    };
    let quota = QuotaPolicyV1::default();
    assert!(store.supervision_state().unwrap().active);
    assert_quota(&store, 0);
    let head_before = store.head().unwrap();
    let admission = store
        .authorize_kubernetes_create_admission(&identity, &intent, &policy(&intent), &quota)
        .expect("schema12 issuer must accept its real current broker and exact policy");
    assert_eq!(store.head().unwrap(), head_before);
    eprintln!("schema12_reserve_ready: issuer accepted, real schema12 valid, quota=0");
    let (receipt, reserved, replayed) = store
        .reserve_kubernetes_create(&identity, &admission, &intent)
        .expect("schema12 must consume its own V2 admission without another migration");
    assert!(!replayed);
    assert_eq!(reserved.state, KubernetesCreateStateV1::Reserved);
    assert_eq!(reserved.revision, 1);
    assert_eq!(reserved.create_epoch, 1);
    assert!(reserved.quota_held && reserved.claim.is_none());
    assert!(reserved.emission_window.is_none());
    assert_v2_event(&store, "control.kubernetes.create.reserved", &receipt);
    assert_quota(&store, 1);
    assert_eq!(store.head().unwrap(), head_before + 1);
    let (replayed_receipt, replayed_snapshot, replayed) = store
        .reserve_kubernetes_create(&identity, &admission, &intent)
        .unwrap();
    assert!(replayed);
    assert_eq!(replayed_receipt, receipt);
    assert!(replayed_snapshot == reserved);
    assert_eq!(store.head().unwrap(), head_before + 1);
    assert_quota(&store, 1);
    assert!(store.integrity().unwrap().valid);
    drop(store);
    let reopened = Store::open_existing_for_tenant(
        &_directory.path().join("tenant.sqlite"),
        "tenant-a",
        Redactor::new(vec![]),
        10_000,
    )
    .unwrap();
    assert!(!reopened.supervision_state().unwrap().active);
    assert_eq!(reopened.supervision_state().unwrap().schema_version, 12);
    assert!(reopened.integrity().unwrap().valid);
    assert_quota(&reopened, 1);
    assert_v2_event(&reopened, "control.kubernetes.create.reserved", &receipt);
    let db = reopened.db.lock().unwrap();
    assert!(replay::load_snapshot(&db, &intent.create_id).unwrap() == Some(reserved));
}

#[test]
fn schema12_v2_reconcile_cancels_migrated_v1_reservation_without_activation() {
    let fixture = Fixture::new();
    let quota = QuotaPolicyV1::default();
    let original = fixture
        .store
        .authorize_held_create_for_test(&fixture.identity, &fixture.intent, &quota, EXPIRY)
        .unwrap();
    let (reserve_receipt, _, _) = fixture
        .store
        .reserve_kubernetes_create(&fixture.identity, &original, &fixture.intent)
        .unwrap();
    migrate_to_12(&fixture);
    let Fixture {
        _directory,
        store,
        identity,
        intent,
    } = fixture;
    drop(store);
    let path = _directory.path().join("tenant.sqlite");
    let store =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap();
    assert_eq!(store.supervision_state().unwrap().schema_version, 12);
    assert!(!store.supervision_state().unwrap().active);
    assert!(store.integrity().unwrap().valid);
    assert_quota(&store, 1);
    let head_before = store.head().unwrap();
    let reconcile = store
        .authorize_kubernetes_create_reconciliation(&identity, &intent.create_id, &quota)
        .expect("current schema12 broker must receive its exact reconciliation capability");
    assert_eq!(store.head().unwrap(), head_before);
    eprintln!("schema12_cancel_ready: exact V1 reserve retained, V2 issuer accepted, inactive");
    let cancellation = cancel(&intent);
    let (receipt, closed, replayed) = store
        .cancel_kubernetes_create(&identity, &reconcile, &cancellation)
        .expect("schema12 must consume V2 reconciliation of its retained initial reservation");
    assert!(!replayed);
    assert_eq!(
        closed.state,
        KubernetesCreateStateV1::CancelledBeforeEmission
    );
    assert_eq!(closed.revision, 2);
    assert_eq!(closed.create_epoch, 2);
    assert!(!closed.quota_held && closed.claim.is_none());
    assert!(closed.intent == intent);
    assert_eq!(closed.last_cancel.as_ref(), Some(&cancellation));
    assert_v2_event(&store, "control.kubernetes.create.cancelled", &receipt);
    assert_eq!(store.head().unwrap(), head_before + 1);
    assert_quota(&store, 0);
    let (old_receipt, old_snapshot, replayed) = store
        .cancel_kubernetes_create(&identity, &reconcile, &cancellation)
        .unwrap();
    assert!(replayed && old_snapshot == closed);
    assert_eq!(old_receipt, receipt);
    assert_eq!(store.head().unwrap(), head_before + 1);
    assert_quota(&store, 0);
    assert!(!store.supervision_state().unwrap().active);
    assert!(store.integrity().unwrap().valid);
    drop(store);
    let reopened =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap();
    assert!(!reopened.supervision_state().unwrap().active);
    assert!(reopened.integrity().unwrap().valid);
    assert_quota(&reopened, 0);
    assert_v2_event(&reopened, "control.kubernetes.create.cancelled", &receipt);
    let reservations = reopened
        .events(&EventQuery {
            kind: Some("control.kubernetes.create.reserved".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(reservations.len(), 1);
    assert_eq!(reservations[0].seq, reserve_receipt.audit_seq);
    assert_eq!(reservations[0].hash, reserve_receipt.audit_hash);
    assert_eq!(reservations[0].event.data["version"], 1);
    let db = reopened.db.lock().unwrap();
    assert!(replay::load_snapshot(&db, &intent.create_id).unwrap() == Some(closed));
}
