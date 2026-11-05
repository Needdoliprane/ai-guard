//! Real tenant SQLite/broker bindings; Kubernetes observations below are synthetic.
use super::*;
use crate::{
    broker_registry::{
        BrokerAuthorityV1, BrokerGrantMutationV1, BrokerIdentityV1, BrokerRuntimeKindV1,
    },
    control::{QuotaPolicyV1, kubernetes_admission::*},
    runtime::kubernetes::{KubernetesAuthorityV1, KubernetesPreflightResourcesV1},
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use std::{fs, os::unix::fs::PermissionsExt};
use uuid::Uuid;

const TOKEN: &str = "public-held-create-test-broker-token-0000000000000000";
const EXPIRY: &str = "2100-01-01T00:00:00.000Z";

pub(crate) struct Fixture {
    pub(crate) _directory: tempfile::TempDir,
    pub(crate) store: Store,
    pub(crate) identity: BrokerIdentityV1,
    pub(crate) intent: KubernetesCreateIntentV1,
}
impl Fixture {
    pub(crate) fn new() -> Self {
        Self::new_expiries(EXPIRY, EXPIRY)
    }
    pub(crate) fn new_expiries(binding_expiry: &str, grant_expiry: &str) -> Self {
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = directory.path().join("tenant.sqlite");
        let store =
            Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap();
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
        let broker_id = Uuid::new_v4().to_string();
        let incarnation_id = Uuid::new_v4().to_string();
        let credential_id = Uuid::new_v4().to_string();
        let authority = KubernetesAuthorityV1::new(
            Uuid::new_v4(),
            "https://127.0.0.1:6443",
            "11111111-1111-4111-8111-111111111111",
            "tenant-a",
            "22222222-2222-4222-8222-222222222222",
            &"a".repeat(64),
        )
        .unwrap();
        store
            .issue_workload_binding(&WorkloadBindingIssueV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                credential_id: credential_id.clone(),
                expected_revision: 0,
                subject: WorkloadSubjectV1::Broker {
                    broker_id: broker_id.clone(),
                    incarnation_id: incarnation_id.clone(),
                },
                sha256_digest: hex::encode(Sha256::digest(TOKEN.as_bytes())),
                expires_at: binding_expiry.into(),
            })
            .unwrap();
        store
            .mutate_broker_grant(&BrokerGrantMutationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                broker_id: broker_id.clone(),
                expected_revision: None,
                credential_id,
                incarnation_id: incarnation_id.clone(),
                authorities: vec![BrokerAuthorityV1 {
                    authority_id: authority.authority_id().to_string(),
                    runtime_kind: BrokerRuntimeKindV1::Kubernetes,
                }],
                lease_seconds: 120,
                not_after: grant_expiry.into(),
            })
            .unwrap();
        let Some(crate::workload::AuthenticatedWorkload::Broker(identity)) =
            store.authenticate_workload_token(TOKEN).unwrap()
        else {
            panic!("fixture must authenticate its real broker binding and grant")
        };
        let intent = KubernetesCreateIntentV1 {
            version: 1,
            create_id: Uuid::new_v4().to_string(),
            session_id: Uuid::new_v4().to_string(),
            control_namespace_uuid: store
                .supervision_state()
                .unwrap()
                .control_namespace_uuid
                .unwrap(),
            broker_id,
            incarnation_id,
            authority,
            profile: KUBERNETES_HELD_PROFILE_V1.into(),
            image: format!("worker@sha256:{}", "b".repeat(64)),
            resources: KubernetesPreflightResourcesV1 {
                cpu_millis: 500,
                memory_mib: 128,
                workspace_mib: 8,
                temporary_mib: 8,
                deadline_seconds: 60,
            },
        };
        Self {
            _directory: directory,
            store,
            identity,
            intent,
        }
    }
}

fn claim(intent: &KubernetesCreateIntentV1) -> KubernetesCreateClaimV1 {
    KubernetesCreateClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: intent.create_id.clone(),
        intent_hash: intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
        emitter_id: Uuid::new_v4().to_string(),
    }
}
fn cancel(
    intent: &KubernetesCreateIntentV1,
    revision: u64,
    epoch: u64,
) -> KubernetesCreateCancelV1 {
    KubernetesCreateCancelV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: intent.create_id.clone(),
        intent_hash: intent.intent_hash().unwrap(),
        expected_revision: revision,
        expected_create_epoch: epoch,
    }
}

#[test]
fn held_create_claim_is_fresh_once_and_replay_keeps_historical_state() {
    let f = Fixture::new();
    let quota = QuotaPolicyV1::default();
    let authorization = f
        .store
        .authorize_held_create_for_test(&f.identity, &f.intent, &quota, EXPIRY)
        .unwrap();
    let (reserved_receipt, reserved, replayed) = f
        .store
        .reserve_kubernetes_create(&f.identity, &authorization, &f.intent)
        .unwrap();
    assert!(!replayed);
    assert_eq!(reserved.state, KubernetesCreateStateV1::Reserved);
    assert_eq!(reserved.revision, 1);
    assert!(reserved.quota_held);
    let request = claim(&f.intent);
    let (first, fresh) = f
        .store
        .claim_kubernetes_create(&f.identity, &authorization, &request)
        .unwrap();
    assert_eq!(first.disposition, KubernetesCreateDispositionV1::EmitOnce);
    let fresh = fresh.expect("only a real first committed claim produces fresh evidence");
    assert_eq!(fresh.claim(), &request);
    assert_eq!(fresh.receipt(), &first.receipt);
    assert_eq!(first.snapshot.revision, 2);
    let window = first.receipt.emission_window.as_ref().unwrap();
    let duration = crate::broker_registry::parse_time(&window.not_after).unwrap()
        - crate::broker_registry::parse_time(&window.issued_at).unwrap();
    assert!(duration > chrono::Duration::zero() && duration <= chrono::Duration::seconds(8));
    let (_, cancelled, _) = f
        .store
        .cancel_kubernetes_create(&f.identity, &authorization, &cancel(&f.intent, 2, 1))
        .unwrap();
    assert_eq!(cancelled.state, KubernetesCreateStateV1::OwnershipUnknown);
    assert_eq!(cancelled.create_epoch, 2);
    assert!(cancelled.quota_held);
    let head = f.store.head().unwrap();
    let (history, fresh) = f
        .store
        .claim_kubernetes_create(&f.identity, &authorization, &request)
        .unwrap();
    assert_eq!(
        history.disposition,
        KubernetesCreateDispositionV1::ObserveOnly
    );
    assert!(fresh.is_none());
    assert_eq!(
        history.snapshot, first.snapshot,
        "replay is a historical result, never current state"
    );
    assert_eq!(history.receipt, first.receipt);
    let (receipt, old_reserved, replayed) = f
        .store
        .reserve_kubernetes_create(&f.identity, &authorization, &f.intent)
        .unwrap();
    assert!(replayed);
    assert_eq!(receipt, reserved_receipt);
    assert_eq!(old_reserved, reserved);
    assert_eq!(f.store.head().unwrap(), head);
    assert!(f.store.integrity().unwrap().valid);
}

#[test]
fn held_create_cancel_before_claim_releases_shared_quota_and_is_idempotent() {
    let f = Fixture::new();
    let quota = QuotaPolicyV1 {
        max_active_workers: 1,
        ..Default::default()
    };
    let authorization = f
        .store
        .authorize_held_create_for_test(&f.identity, &f.intent, &quota, EXPIRY)
        .unwrap();
    f.store
        .reserve_kubernetes_create(&f.identity, &authorization, &f.intent)
        .unwrap();
    let usage = f.store.worker_quota(&quota).unwrap();
    assert_eq!(usage.held_workers, 1);
    assert_eq!(usage.held_cpu_millis, 500);
    assert_eq!(usage.held_memory_bytes, 128 * 1_048_576);
    let mut other = f.intent.clone();
    other.create_id = Uuid::new_v4().to_string();
    other.session_id = Uuid::new_v4().to_string();
    let other_auth = f
        .store
        .authorize_held_create_for_test(&f.identity, &other, &quota, EXPIRY)
        .unwrap();
    let head = f.store.head().unwrap();
    assert!(
        f.store
            .reserve_kubernetes_create(&f.identity, &other_auth, &other)
            .is_err()
    );
    assert_eq!(
        f.store.head().unwrap(),
        head,
        "quota denial writes no reservation"
    );
    let cancellation = cancel(&f.intent, 1, 1);
    let (receipt, snapshot, replayed) = f
        .store
        .cancel_kubernetes_create(&f.identity, &authorization, &cancellation)
        .unwrap();
    assert!(!replayed);
    assert_eq!(
        snapshot.state,
        KubernetesCreateStateV1::CancelledBeforeEmission
    );
    assert!(!snapshot.quota_held);
    assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 0);
    let head = f.store.head().unwrap();
    let (same, state, replayed) = f
        .store
        .cancel_kubernetes_create(&f.identity, &authorization, &cancellation)
        .unwrap();
    assert!(replayed);
    assert_eq!(same, receipt);
    assert_eq!(state, snapshot);
    assert_eq!(f.store.head().unwrap(), head);
    assert!(
        f.store
            .claim_kubernetes_create(&f.identity, &authorization, &claim(&f.intent))
            .is_err()
    );
    f.store
        .reserve_kubernetes_create(&f.identity, &other_auth, &other)
        .unwrap();
    assert!(f.store.integrity().unwrap().valid);
}

#[test]
fn held_create_result_ack_replays_exactly_and_late_epoch_cannot_promote() {
    let f = Fixture::new();
    let authorization = f
        .store
        .authorize_held_create_for_test(&f.identity, &f.intent, &QuotaPolicyV1::default(), EXPIRY)
        .unwrap();
    f.store
        .reserve_kubernetes_create(&f.identity, &authorization, &f.intent)
        .unwrap();
    let request = claim(&f.intent);
    f.store
        .claim_kubernetes_create(&f.identity, &authorization, &request)
        .unwrap();
    let result = KubernetesCreateResultV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: f.intent.create_id.clone(),
        intent_hash: f.intent.intent_hash().unwrap(),
        claim_operation_id: request.operation_id.clone(),
        expected_revision: 2,
        expected_create_epoch: 1,
        outcome: KubernetesCreateOutcomeV1::OwnershipUnknown {
            reason: KubernetesCreateUnknownReasonV1::TransportUnknown,
            http_status: None,
        },
    };
    let ack = f
        .store
        .record_kubernetes_create_result(&f.identity, &authorization, &result)
        .unwrap();
    assert_eq!(ack.result(), &result);
    assert_eq!(
        ack.snapshot().state,
        KubernetesCreateStateV1::OwnershipUnknown
    );
    let head = f.store.head().unwrap();
    let replay = f
        .store
        .record_kubernetes_create_result(&f.identity, &authorization, &result)
        .unwrap();
    assert_eq!(ack.receipt(), replay.receipt());
    assert_eq!(ack.snapshot(), replay.snapshot());
    assert_eq!(f.store.head().unwrap(), head);
    let mut divergent = result.clone();
    divergent.outcome = KubernetesCreateOutcomeV1::OwnershipUnknown {
        reason: KubernetesCreateUnknownReasonV1::ApiRejected,
        http_status: Some(403),
    };
    assert!(
        f.store
            .record_kubernetes_create_result(&f.identity, &authorization, &divergent)
            .is_err()
    );
    let (_, cancelled, _) = f
        .store
        .cancel_kubernetes_create(&f.identity, &authorization, &cancel(&f.intent, 3, 1))
        .unwrap();
    assert_eq!(cancelled.create_epoch, 2);
    let mut late = result;
    late.operation_id = Uuid::new_v4().to_string();
    late.expected_revision = 4;
    assert!(
        f.store
            .record_kubernetes_create_result(&f.identity, &authorization, &late)
            .is_err()
    );
    assert!(f.store.integrity().unwrap().valid);
}

#[test]
fn held_create_authorization_binds_complete_authority_and_rejects_event_id_collision() {
    let positive = Fixture::new();
    let permitted = positive
        .store
        .authorize_held_create_for_test(
            &positive.identity,
            &positive.intent,
            &QuotaPolicyV1::default(),
            EXPIRY,
        )
        .unwrap();
    positive
        .store
        .reserve_kubernetes_create(&positive.identity, &permitted, &positive.intent)
        .unwrap();
    assert!(positive.store.integrity().unwrap().valid);
    let f = Fixture::new();
    let quota = QuotaPolicyV1::default();
    let authorization = f
        .store
        .authorize_held_create_for_test(&f.identity, &f.intent, &quota, EXPIRY)
        .unwrap();
    let mut changed = f.intent.clone();
    let mut value = serde_json::to_value(&changed.authority).unwrap();
    value["endpoint"] = json!("https://127.0.0.1:7443");
    changed.authority = serde_json::from_value(value).unwrap();
    assert_eq!(
        changed.authority.authority_id(),
        f.intent.authority.authority_id()
    );
    let head = f.store.head().unwrap();
    assert!(
        f.store
            .reserve_kubernetes_create(&f.identity, &authorization, &changed)
            .is_err()
    );
    assert_eq!(f.store.head().unwrap(), head);
    let mut unrelated = Event::new("fixture.unrelated");
    unrelated.id = f.intent.create_id.clone();
    f.store.append(unrelated).unwrap();
    let head = f.store.head().unwrap();
    assert!(
        f.store
            .reserve_kubernetes_create(&f.identity, &authorization, &f.intent)
            .is_err()
    );
    assert_eq!(f.store.head().unwrap(), head);
}

#[test]
fn held_create_safe_result_survives_cancel_and_inactive_reopen_without_reactivation() {
    let f = Fixture::new();
    let authorization = f
        .store
        .authorize_held_create_for_test(&f.identity, &f.intent, &QuotaPolicyV1::default(), EXPIRY)
        .unwrap();
    f.store
        .reserve_kubernetes_create(&f.identity, &authorization, &f.intent)
        .unwrap();
    let request = claim(&f.intent);
    f.store
        .claim_kubernetes_create(&f.identity, &authorization, &request)
        .unwrap();
    let rendered = crate::runtime::kubernetes::admission::prepare(&f.intent).unwrap();
    let mut pod: Value = serde_json::from_slice(rendered.pod_bytes()).unwrap();
    pod["metadata"]["uid"] = json!(Uuid::new_v4().to_string());
    pod["metadata"]["resourceVersion"] = json!("1");
    let observed_at = crate::broker_registry::format_time(chrono::Utc::now());
    let outcome = crate::runtime::kubernetes::admission::parse_created_held(
        &f.intent,
        &serde_json::to_vec(&pod).unwrap(),
        &observed_at,
    )
    .unwrap();
    let result = KubernetesCreateResultV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: f.intent.create_id.clone(),
        intent_hash: f.intent.intent_hash().unwrap(),
        claim_operation_id: request.operation_id.clone(),
        expected_revision: 2,
        expected_create_epoch: 1,
        outcome,
    };
    let mut unsafe_result = result.clone();
    if let KubernetesCreateOutcomeV1::CreatedHeld { admitted_spec, .. } = &mut unsafe_result.outcome
    {
        admitted_spec["serviceAccountName"] = json!("unexpected-service-account");
    } else {
        panic!("the controlled response must reach created_held before the negative case");
    }
    let before = f.store.head().unwrap();
    assert!(
        f.store
            .record_kubernetes_create_result(&f.identity, &authorization, &unsafe_result)
            .is_err()
    );
    assert_eq!(f.store.head().unwrap(), before);
    let confirmed = f
        .store
        .record_kubernetes_create_result(&f.identity, &authorization, &result)
        .unwrap();
    assert_eq!(confirmed.tenant_id(), "tenant-a");
    assert_eq!(
        confirmed.snapshot().state,
        KubernetesCreateStateV1::CreatedHeld
    );
    let (_, cancelled, _) = f
        .store
        .cancel_kubernetes_create(&f.identity, &authorization, &cancel(&f.intent, 3, 1))
        .unwrap();
    assert_eq!(cancelled.state, KubernetesCreateStateV1::CreatedHeld);
    assert_eq!(cancelled.create_epoch, 2);
    assert_eq!(cancelled.last_result.as_ref(), Some(&result));
    assert!(cancelled.quota_held);
    let event = f
        .store
        .events(&EventQuery {
            kind: Some("control.kubernetes.create.result".into()),
            ..Default::default()
        })
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(event.event.provenance, "declared");
    assert_eq!(event.event.coverage, "partial");
    let path = f._directory.path().join("tenant.sqlite");
    drop(f.store);
    let reopened =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap();
    assert!(reopened.integrity().unwrap().valid);
    assert!(!reopened.supervision_state().unwrap().active);
    let replay = reopened
        .record_kubernetes_create_result(&f.identity, &authorization, &result)
        .unwrap();
    assert_eq!(replay.receipt(), confirmed.receipt());
    assert_eq!(replay.snapshot(), confirmed.snapshot());
    assert!(
        reopened
            .claim_kubernetes_create(&f.identity, &authorization, &request)
            .is_err()
    );
    assert!(!reopened.supervision_state().unwrap().active);
}

#[test]
fn held_create_mutations_share_the_cumulative_broker_audit_budget() {
    let f = Fixture::new();
    let mut budget = f.store.workload_audit_policy().clone();
    budget.tenant.events = 2;
    budget.principal.events = 1;
    let store = f.store.with_workload_audit_policy(budget).unwrap();
    let authorization = store
        .authorize_held_create_for_test(&f.identity, &f.intent, &QuotaPolicyV1::default(), EXPIRY)
        .unwrap();
    store
        .reserve_kubernetes_create(&f.identity, &authorization, &f.intent)
        .unwrap();
    let head = store.head().unwrap();
    let error = store
        .claim_kubernetes_create(&f.identity, &authorization, &claim(&f.intent))
        .err()
        .expect("second mutation exceeds the broker's single-event budget");
    assert_eq!(
        error.downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::WorkloadAuditBudgetExhausted)
    );
    assert_eq!(store.head().unwrap(), head);
    assert_eq!(
        store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
    assert!(store.integrity().unwrap().valid);
}

#[test]
fn held_create_replay_requires_a_current_nonrevoked_broker_identity() {
    let f = Fixture::new();
    let authorization = f
        .store
        .authorize_held_create_for_test(&f.identity, &f.intent, &QuotaPolicyV1::default(), EXPIRY)
        .unwrap();
    f.store
        .reserve_kubernetes_create(&f.identity, &authorization, &f.intent)
        .unwrap();
    f.store
        .revoke_broker(&crate::broker_registry::BrokerRevocationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: f.intent.broker_id.clone(),
            expected_revision: 1,
        })
        .unwrap();
    let head = f.store.head().unwrap();
    assert!(
        f.store
            .reserve_kubernetes_create(&f.identity, &authorization, &f.intent)
            .is_err()
    );
    assert!(
        f.store
            .claim_kubernetes_create(&f.identity, &authorization, &claim(&f.intent))
            .is_err()
    );
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(
        f.store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
    assert!(
        f.store.integrity().unwrap().valid,
        "a revoked identity does not rewrite valid old events"
    );
}
