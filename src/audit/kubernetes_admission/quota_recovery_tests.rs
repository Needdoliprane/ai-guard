//! A real Store/binding/grant regression candidate for run-2 D6.
//! No runtime is contacted and no Pod absence is inferred.
use super::*;
use crate::audit::kubernetes_admission_state_tests::Fixture;
use crate::broker_registry::{
    BrokerAuthorityV1, BrokerGrantMutationV1, BrokerIdentityV1, BrokerRevocationV1,
    BrokerRuntimeKindV1,
};
use crate::workload::{AuthenticatedWorkload, WorkloadBindingIssueV1, WorkloadSubjectV1};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const EXPIRY: &str = "2100-01-01T00:00:00.000Z";
const SUCCESSOR_TOKEN: &str = "public-held-create-successor-test-token-0000000000000000";

fn policy(intent: &KubernetesCreateIntentV1) -> KubernetesHeldCreatePolicyV1 {
    KubernetesHeldCreatePolicyV1 {
        version: 1,
        decision_ttl_seconds: 17,
        approvals: vec![KubernetesHeldCreateApprovalV1 {
            authority_fingerprint: intent.authority.fingerprint(),
            image: intent.image.clone(),
            profile: intent.profile.clone(),
        }],
    }
}

fn reserve(f: &Fixture, quota: &QuotaPolicyV1) -> KubernetesCreateSnapshotV1 {
    f.store
        .migrate_kubernetes_policy_v9(&f._directory.path().join("before-policy-v9.sqlite"))
        .unwrap();
    let admission = f
        .store
        .authorize_kubernetes_create_admission(&f.identity, &f.intent, &policy(&f.intent), quota)
        .unwrap();
    let (_, snapshot, replayed) = f
        .store
        .reserve_kubernetes_create(&f.identity, &admission, &f.intent)
        .unwrap();
    assert!(!replayed);
    assert_eq!(snapshot.state, KubernetesCreateStateV1::Reserved);
    assert_eq!(snapshot.revision, 1);
    assert_eq!(snapshot.create_epoch, 1);
    assert!(snapshot.quota_held);
    assert!(snapshot.claim.is_none());
    assert!(snapshot.emission_window.is_none());
    assert!(snapshot.last_result.is_none());
    assert!(snapshot.last_cancel.is_none());
    assert_eq!(f.store.worker_quota(quota).unwrap().held_workers, 1);
    snapshot
}

fn cancellation(intent: &KubernetesCreateIntentV1) -> KubernetesCreateCancelV1 {
    KubernetesCreateCancelV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: intent.create_id.clone(),
        intent_hash: intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
    }
}

fn revoke_then_authenticate_successor(f: &Fixture) -> BrokerIdentityV1 {
    f.store
        .revoke_broker(&BrokerRevocationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: f.intent.broker_id.clone(),
            expected_revision: 1,
        })
        .unwrap();
    authenticate_successor(f, 2, f.intent.authority.authority_id().to_string())
}

fn authenticate_successor(
    f: &Fixture,
    expected_revision: u64,
    authority_id: String,
) -> BrokerIdentityV1 {
    let incarnation_id = Uuid::new_v4().to_string();
    let credential_id = Uuid::new_v4().to_string();
    f.store
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential_id.clone(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: f.intent.broker_id.clone(),
                incarnation_id: incarnation_id.clone(),
            },
            sha256_digest: hex::encode(Sha256::digest(SUCCESSOR_TOKEN.as_bytes())),
            expires_at: EXPIRY.into(),
        })
        .unwrap();
    f.store
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: f.intent.broker_id.clone(),
            expected_revision: Some(expected_revision),
            credential_id,
            incarnation_id: incarnation_id.clone(),
            authorities: vec![BrokerAuthorityV1 {
                authority_id,
                runtime_kind: BrokerRuntimeKindV1::Kubernetes,
            }],
            lease_seconds: 120,
            not_after: EXPIRY.into(),
        })
        .unwrap();
    let Some(AuthenticatedWorkload::Broker(successor)) = f
        .store
        .authenticate_workload_token(SUCCESSOR_TOKEN)
        .unwrap()
    else {
        panic!("the real successor binding and grant must authenticate")
    };
    assert_eq!(successor.broker_id(), f.identity.broker_id());
    assert_ne!(successor.credential_id(), f.identity.credential_id());
    assert_ne!(incarnation_id, f.intent.incarnation_id);
    successor
}

fn assert_closed_once(
    f: &Fixture,
    actor: &BrokerIdentityV1,
    authorization: &KubernetesCreateAuthorization,
    quota: &QuotaPolicyV1,
) -> i64 {
    let head = f.store.head().unwrap();
    let cancel = cancellation(&f.intent);
    let (receipt, snapshot, replayed) = f
        .store
        .cancel_kubernetes_create(actor, authorization, &cancel)
        .expect("the exact never-claimed reservation must have an audited exit");
    assert!(!replayed);
    assert_eq!(
        snapshot.state,
        KubernetesCreateStateV1::CancelledBeforeEmission
    );
    assert_eq!(snapshot.revision, 2);
    assert_eq!(snapshot.create_epoch, 2);
    assert!(!snapshot.quota_held);
    assert!(snapshot.claim.is_none());
    assert!(snapshot.emission_window.is_none());
    assert!(snapshot.intent == f.intent);
    assert_eq!(receipt.audit_seq, head + 1);
    let usage = f.store.worker_quota(quota).unwrap();
    assert_eq!(usage.held_workers, 0);
    assert_eq!(usage.held_cpu_millis, 0);
    assert_eq!(usage.held_memory_bytes, 0);

    let after = f.store.head().unwrap();
    let (again, historical, replayed) = f
        .store
        .cancel_kubernetes_create(actor, authorization, &cancel)
        .unwrap();
    assert!(replayed);
    assert_eq!(again, receipt);
    assert!(historical == snapshot);
    assert_eq!(f.store.head().unwrap(), after);
    let events = f
        .store
        .events(&EventQuery {
            kind: Some("control.kubernetes.create.cancelled".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].event.data["intent"],
        serde_json::to_value(&f.intent).unwrap()
    );
    assert_eq!(
        events[0].event.data["identity"]["credential_id"],
        actor.credential_id()
    );
    assert!(f.store.integrity().unwrap().valid);
    after
}

fn assert_reopened_after_close(f: Fixture, quota: &QuotaPolicyV1, head: i64) {
    let path = f._directory.path().join("tenant.sqlite");
    drop(f.store);
    let reopened =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap();
    assert!(reopened.integrity().unwrap().valid);
    assert_eq!(reopened.head().unwrap(), head);
    assert_eq!(reopened.worker_quota(quota).unwrap().held_workers, 0);
}

#[test]
fn current_broker_cancels_an_exact_never_claimed_reservation_with_audit() {
    let f = Fixture::new();
    let quota = QuotaPolicyV1 {
        max_active_workers: 1,
        ..Default::default()
    };
    reserve(&f, &quota);
    let authorization = f
        .store
        .authorize_kubernetes_create_reconciliation(&f.identity, &f.intent.create_id, &quota)
        .unwrap();
    let head = assert_closed_once(&f, &f.identity, &authorization, &quota);
    assert_reopened_after_close(f, &quota, head);
}

#[test]
fn authenticated_successor_cancels_an_exact_never_claimed_reservation_after_revocation() {
    let f = Fixture::new();
    let quota = QuotaPolicyV1 {
        max_active_workers: 1,
        ..Default::default()
    };
    reserve(&f, &quota);
    let successor = revoke_then_authenticate_successor(&f);
    assert!(
        f.store
            .authorize_kubernetes_create_reconciliation(&f.identity, &f.intent.create_id, &quota)
            .is_err(),
        "the revoked creator must remain unauthorized"
    );
    let head = f.store.head().unwrap();
    assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 1);
    let authorization =
        f.store
            .authorize_kubernetes_create_reconciliation(&successor, &f.intent.create_id, &quota);
    assert_eq!(
        f.store.head().unwrap(),
        head,
        "issuance never changes the ledger"
    );
    assert!(
        authorization.is_err(),
        "Reconcile must not adopt the successor"
    );
    migrate_recovery(&f);
    let request = recovery_request(&f.intent);
    let result = recover(&f, &successor, &request, &quota);
    assert!(!result.replayed);
    assert_eq!(result.snapshot.intent, f.intent);
    assert!(!result.snapshot.quota_held);
    assert_eq!(
        result.snapshot.state,
        KubernetesCreateStateV1::CancelledBeforeEmission
    );
    assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 0);
    let head = f.store.head().unwrap();
    let again = recover(&f, &successor, &request, &quota);
    assert!(again.replayed);
    assert_eq!(again.receipt, result.receipt);
    assert_eq!(again.snapshot, result.snapshot);
    assert_eq!(f.store.head().unwrap(), head);
    let proof = f
        .store
        .authorize_kubernetes_create_recovery_lookup(
            &successor,
            &request.create_id,
            &request.operation_id,
        )
        .unwrap();
    let looked_up = f
        .store
        .lookup_kubernetes_create_recovery(&successor, &proof)
        .unwrap();
    assert_eq!(looked_up.receipt, result.receipt);
    let entries = f
        .store
        .events(&EventQuery {
            kind: Some("control.kubernetes.create.recovered_before_emission".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(entries.len(), 1);
    let data = &entries[0].event.data;
    assert_eq!(data["identity"]["credential_id"], successor.credential_id());
    assert_eq!(
        data["control_decision"]["basis"]["creator_incarnation_id"],
        f.intent.incarnation_id
    );
    assert_ne!(
        data["control_decision"]["incarnation_id"],
        f.intent.incarnation_id
    );
    assert_reopened_after_close(f, &quota, head);
}

fn migrate_recovery(f: &Fixture) {
    f.store
        .migrate_human_workers_v10(&f._directory.path().join("before10"))
        .unwrap();
    f.store
        .migrate_kubernetes_session_reservations_v11(&f._directory.path().join("before11"))
        .unwrap();
    f.store
        .migrate_broker_checkpoints_v12(&f._directory.path().join("before12"))
        .unwrap();
    f.store
        .migrate_kubernetes_recovery_v13(&f._directory.path().join("before13"))
        .unwrap();
}

fn recovery_request(intent: &KubernetesCreateIntentV1) -> KubernetesCreateRecoveryV3 {
    KubernetesCreateRecoveryV3 {
        version: 3,
        operation_id: Uuid::new_v4().to_string(),
        create_id: intent.create_id.clone(),
        intent_hash: intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
    }
}

fn recover(
    f: &Fixture,
    actor: &BrokerIdentityV1,
    request: &KubernetesCreateRecoveryV3,
    quota: &QuotaPolicyV1,
) -> KubernetesCreateRecoveryResultV3 {
    use crate::audit::kubernetes_admission::recovery::PreparedRecovery;
    match f
        .store
        .prepare_kubernetes_create_recovery(actor, request, quota)
        .unwrap()
    {
        PreparedRecovery::New(proof) => f.store.recover_kubernetes_create(actor, &proof).unwrap(),
        PreparedRecovery::Historical(proof) => f
            .store
            .lookup_kubernetes_create_recovery(actor, &proof)
            .unwrap(),
    }
}

#[test]
fn recovery_preparation_preserves_claim_cas_and_unknown_quota() {
    use crate::audit::kubernetes_admission::recovery::PreparedRecovery;
    let f = Fixture::new();
    let quota = QuotaPolicyV1::default();
    reserve(&f, &quota);
    migrate_recovery(&f);
    let request = recovery_request(&f.intent);
    let PreparedRecovery::New(proof) = f
        .store
        .prepare_kubernetes_create_recovery(&f.identity, &request, &quota)
        .unwrap()
    else {
        panic!("initial recovery must be New")
    };
    let admission = f
        .store
        .authorize_kubernetes_create_admission(&f.identity, &f.intent, &policy(&f.intent), &quota)
        .unwrap();
    let claim = KubernetesCreateClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: f.intent.create_id.clone(),
        intent_hash: f.intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
        emitter_id: Uuid::new_v4().to_string(),
    };
    // This is the actual production claim. Its fresh proof is never dispatched.
    let (committed, fresh) = f
        .store
        .claim_kubernetes_create(&f.identity, &admission, &claim)
        .unwrap();
    drop(fresh);
    let head = f.store.head().unwrap();
    assert!(
        f.store
            .recover_kubernetes_create(&f.identity, &proof)
            .is_err()
    );
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 1);
    assert_eq!(
        committed.snapshot.state,
        KubernetesCreateStateV1::EmissionPossible
    );
    assert!(
        f.store
            .prepare_kubernetes_create_recovery(&f.identity, &request, &quota)
            .is_err()
    );
    assert!(f.store.integrity().unwrap().valid);
}

#[test]
fn recovery_revalidates_identity_before_mutation_and_historical_lookup() {
    use crate::audit::kubernetes_admission::recovery::PreparedRecovery;
    let f = Fixture::new();
    let quota = QuotaPolicyV1::default();
    reserve(&f, &quota);
    migrate_recovery(&f);
    let request = recovery_request(&f.intent);
    let PreparedRecovery::New(proof) = f
        .store
        .prepare_kubernetes_create_recovery(&f.identity, &request, &quota)
        .unwrap()
    else {
        panic!("initial proof")
    };
    let successor = revoke_then_authenticate_successor(&f);
    let head = f.store.head().unwrap();
    assert!(
        f.store
            .recover_kubernetes_create(&f.identity, &proof)
            .is_err()
    );
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 1);
    let closed = recover(&f, &successor, &request, &quota);
    let lookup = f
        .store
        .authorize_kubernetes_create_recovery_lookup(
            &successor,
            &request.create_id,
            &request.operation_id,
        )
        .unwrap();
    f.store
        .revoke_broker(&BrokerRevocationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: f.intent.broker_id.clone(),
            expected_revision: 3,
        })
        .unwrap();
    let head = f.store.head().unwrap();
    assert!(
        f.store
            .lookup_kubernetes_create_recovery(&successor, &lookup)
            .is_err()
    );
    assert!(
        f.store
            .prepare_kubernetes_create_recovery(&successor, &request, &quota)
            .is_err()
    );
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(
        closed.snapshot.state,
        KubernetesCreateStateV1::CancelledBeforeEmission
    );
}

#[test]
fn recovery_exact_replay_wins_prepare_race_and_conflicts_never_mutate() {
    use crate::audit::kubernetes_admission::recovery::PreparedRecovery;
    let f = Fixture::new();
    let quota = QuotaPolicyV1::default();
    reserve(&f, &quota);
    migrate_recovery(&f);
    let actor = revoke_then_authenticate_successor(&f);
    let request = recovery_request(&f.intent);
    let PreparedRecovery::New(earlier) = f
        .store
        .prepare_kubernetes_create_recovery(&actor, &request, &quota)
        .unwrap()
    else {
        panic!("initial proof")
    };
    let first = recover(&f, &actor, &request, &quota);
    let head = f.store.head().unwrap();
    let concurrent = f.store.recover_kubernetes_create(&actor, &earlier).unwrap();
    assert!(concurrent.replayed);
    assert_eq!(concurrent.receipt, first.receipt);
    let mut divergent = request.clone();
    divergent.intent_hash = "f".repeat(64);
    assert!(
        f.store
            .prepare_kubernetes_create_recovery(&actor, &divergent, &quota)
            .is_err()
    );
    assert!(
        f.store
            .authorize_kubernetes_create_recovery_lookup(
                &actor,
                &request.create_id,
                &Uuid::new_v4().to_string()
            )
            .is_err()
    );
    let other_tenant = BrokerIdentityV1::authenticated(
        "tenant-b".into(),
        actor.broker_id().into(),
        actor.credential_id().into(),
        actor.binding_revision(),
        actor.grant_revision(),
    )
    .unwrap();
    assert!(
        f.store
            .prepare_kubernetes_create_recovery(&other_tenant, &request, &quota)
            .is_err()
    );
    assert!(
        f.store
            .authorize_kubernetes_create_recovery_lookup(
                &other_tenant,
                &request.create_id,
                &request.operation_id
            )
            .is_err()
    );
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 0);
}

#[test]
fn expired_creator_grant_recovers_through_current_successor_without_revocation() {
    use crate::audit::kubernetes_admission::recovery::PreparedRecovery;
    let expiration =
        crate::broker_registry::format_time(chrono::Utc::now() + chrono::Duration::seconds(15));
    let f = Fixture::new_expiries(EXPIRY, &expiration);
    let quota = QuotaPolicyV1::default();
    reserve(&f, &quota);
    migrate_recovery(&f);
    let request = recovery_request(&f.intent);
    let PreparedRecovery::New(expiring) = f
        .store
        .prepare_kubernetes_create_recovery(&f.identity, &request, &quota)
        .unwrap()
    else {
        panic!("initial proof before expiry")
    };
    let remaining = (crate::broker_registry::parse_time(&expiration).unwrap() - chrono::Utc::now())
        .to_std()
        .unwrap_or_default();
    std::thread::sleep(remaining + std::time::Duration::from_millis(50));
    assert!(
        f.store
            .authorize_kubernetes_create_reconciliation(&f.identity, &f.intent.create_id, &quota)
            .is_err()
    );
    assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 1);
    let head = f.store.head().unwrap();
    assert!(
        f.store
            .recover_kubernetes_create(&f.identity, &expiring)
            .is_err()
    );
    assert_eq!(f.store.head().unwrap(), head);
    // No revoke event is introduced: the replacement follows actual expiry.
    let successor = authenticate_successor(&f, 1, f.intent.authority.authority_id().to_string());
    let closed = recover(&f, &successor, &request, &quota);
    assert_eq!(
        closed.snapshot.state,
        KubernetesCreateStateV1::CancelledBeforeEmission
    );
    assert!(!closed.snapshot.quota_held);
    assert!(f.store.integrity().unwrap().valid);
}

#[test]
fn recovery_requires_the_current_grants_historical_authority_and_preserves_quota_on_audit_refusal()
{
    use crate::audit::capacity::WorkloadAuditPolicyV1;
    use crate::audit::kubernetes_admission::recovery::PreparedRecovery;
    use crate::audit::{AuditCapacityFailureV1, AuditCapacityV1};
    let f = Fixture::new();
    let f = Fixture {
        store: f
            .store
            .with_workload_audit_policy(WorkloadAuditPolicyV1 {
                version: 1,
                tenant: AuditCapacityV1 {
                    events: 2,
                    bytes: 2 * 1024 * 1024,
                },
                principal: AuditCapacityV1 {
                    events: 1,
                    bytes: 1024 * 1024,
                },
            })
            .unwrap(),
        ..f
    };
    let quota = QuotaPolicyV1::default();
    reserve(&f, &quota);
    migrate_recovery(&f);
    let request = recovery_request(&f.intent);
    let PreparedRecovery::New(proof) = f
        .store
        .prepare_kubernetes_create_recovery(&f.identity, &request, &quota)
        .unwrap()
    else {
        panic!("initial proof")
    };
    let head = f.store.head().unwrap();
    let refused = f
        .store
        .recover_kubernetes_create(&f.identity, &proof)
        .unwrap_err();
    assert_eq!(
        refused.downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::WorkloadAuditBudgetExhausted)
    );
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 1);
    assert!(f.store.integrity().unwrap().valid);

    let unrelated = authenticate_successor(&f, 1, Uuid::new_v4().to_string());
    let head = f.store.head().unwrap();
    assert!(
        f.store
            .prepare_kubernetes_create_recovery(&unrelated, &request, &quota)
            .is_err()
    );
    assert!(
        f.store
            .authorize_kubernetes_create_recovery_lookup(
                &unrelated,
                &request.create_id,
                &request.operation_id
            )
            .is_err()
    );
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 1);
}

#[test]
fn recovery_preserves_mixed_v1_v2_history_and_closed_input_shapes() {
    let f = Fixture::new();
    let quota = QuotaPolicyV1::default();
    let original = f
        .store
        .authorize_held_create_for_test(&f.identity, &f.intent, &quota, EXPIRY)
        .unwrap();
    let (reserved, _, _) = f
        .store
        .reserve_kubernetes_create(&f.identity, &original, &f.intent)
        .unwrap();
    f.store
        .migrate_kubernetes_policy_v9(&f._directory.path().join("before9"))
        .unwrap();
    migrate_recovery(&f);
    let request = recovery_request(&f.intent);
    let before = f.store.head().unwrap();
    for bad in [
        serde_json::json!({"version":3,"operation_id":request.operation_id,
        "create_id":request.create_id,"intent_hash":request.intent_hash,
        "expected_revision":1,"expected_create_epoch":1,"tenant_id":"tenant-b"}),
        serde_json::json!({"version":3,"operation_id":request.operation_id,
        "create_id":request.create_id,"intent_hash":request.intent_hash,
        "expected_revision":2,"expected_create_epoch":1}),
    ] {
        if let Ok(parsed) = serde_json::from_value::<KubernetesCreateRecoveryV3>(bad) {
            assert!(parsed.validate().is_err());
        }
    }
    assert_eq!(f.store.head().unwrap(), before);
    let result = recover(&f, &f.identity, &request, &quota);
    assert!(result.receipt.emission_window.is_none());
    let recovered = f
        .store
        .events(&EventQuery {
            kind: Some("control.kubernetes.create.recovered_before_emission".into()),
            ..Default::default()
        })
        .unwrap();
    let descriptor: AuthorizationEvidence =
        serde_json::from_value(recovered[0].event.data["control_decision"].clone()).unwrap();
    let denied_actions = [
        model::Change::Reserve {
            intent: f.intent.clone(),
        },
        model::Change::Cancel {
            cancel: cancellation(&f.intent),
        },
        model::Change::Claim {
            claim: KubernetesCreateClaimV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                create_id: f.intent.create_id.clone(),
                intent_hash: f.intent.intent_hash().unwrap(),
                expected_revision: 1,
                expected_create_epoch: 1,
                emitter_id: Uuid::new_v4().to_string(),
            },
        },
        model::Change::Result {
            result: KubernetesCreateResultV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                create_id: f.intent.create_id.clone(),
                intent_hash: f.intent.intent_hash().unwrap(),
                claim_operation_id: Uuid::new_v4().to_string(),
                expected_revision: 2,
                expected_create_epoch: 1,
                outcome: KubernetesCreateOutcomeV1::OwnershipUnknown {
                    reason: KubernetesCreateUnknownReasonV1::TransportUnknown,
                    http_status: None,
                },
            },
        },
    ];
    for action in &denied_actions {
        assert!(
            descriptor
                .validate_access(action, mutations::HeldChangeAccess::Mutate)
                .is_err()
        );
        assert!(
            descriptor
                .validate_access(action, mutations::HeldChangeAccess::ObserveClaim)
                .is_err()
        );
    }
    let mut nil = request.clone();
    nil.operation_id = Uuid::nil().to_string();
    assert!(nil.validate().is_err());
    assert_eq!(
        recovered[0].event.data["control_decision"]["basis"]["reservation_seq"],
        reserved.audit_seq
    );
    let head = f.store.head().unwrap();
    assert_reopened_after_close(f, &quota, head);
}

#[test]
fn recovery_refuses_recorded_created_or_unknown_post_claim_states() {
    for created in [false, true] {
        let f = Fixture::new();
        let quota = QuotaPolicyV1::default();
        reserve(&f, &quota);
        migrate_recovery(&f);
        let admission = f
            .store
            .authorize_kubernetes_create_admission(
                &f.identity,
                &f.intent,
                &policy(&f.intent),
                &quota,
            )
            .unwrap();
        let claim = KubernetesCreateClaimV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            create_id: f.intent.create_id.clone(),
            intent_hash: f.intent.intent_hash().unwrap(),
            expected_revision: 1,
            expected_create_epoch: 1,
            emitter_id: Uuid::new_v4().to_string(),
        };
        let (_, fresh) = f
            .store
            .claim_kubernetes_create(&f.identity, &admission, &claim)
            .unwrap();
        drop(fresh);
        let outcome = if created {
            // A declared response fixture, not a runtime observation or a Pod.
            let rendered = crate::runtime::kubernetes::admission::prepare(&f.intent).unwrap();
            let mut pod: Value = serde_json::from_slice(rendered.pod_bytes()).unwrap();
            pod["metadata"]["uid"] = serde_json::json!(Uuid::new_v4().to_string());
            pod["metadata"]["resourceVersion"] = serde_json::json!("1");
            crate::runtime::kubernetes::admission::parse_created_held(
                &f.intent,
                &serde_json::to_vec(&pod).unwrap(),
                &crate::broker_registry::format_time(chrono::Utc::now()),
            )
            .unwrap()
        } else {
            KubernetesCreateOutcomeV1::OwnershipUnknown {
                reason: KubernetesCreateUnknownReasonV1::TransportUnknown,
                http_status: None,
            }
        };
        let reconciliation = f
            .store
            .authorize_kubernetes_create_reconciliation(&f.identity, &f.intent.create_id, &quota)
            .unwrap();
        let result = KubernetesCreateResultV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            create_id: f.intent.create_id.clone(),
            intent_hash: f.intent.intent_hash().unwrap(),
            claim_operation_id: claim.operation_id,
            expected_revision: 2,
            expected_create_epoch: 1,
            outcome,
        };
        let ack = f
            .store
            .record_kubernetes_create_result(&f.identity, &reconciliation, &result)
            .unwrap();
        assert_eq!(
            ack.snapshot().state,
            if created {
                KubernetesCreateStateV1::CreatedHeld
            } else {
                KubernetesCreateStateV1::OwnershipUnknown
            }
        );
        let head = f.store.head().unwrap();
        assert!(
            f.store
                .prepare_kubernetes_create_recovery(
                    &f.identity,
                    &recovery_request(&f.intent),
                    &quota
                )
                .is_err()
        );
        assert_eq!(f.store.head().unwrap(), head);
        assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 1);
        assert!(f.store.integrity().unwrap().valid);
    }
}

#[test]
fn one_journal_keeps_v1_v2_v3_and_accepts_a_new_v2_after_recovery_then_reopens() {
    let f = Fixture::new();
    let quota = QuotaPolicyV1::default();
    let legacy = f
        .store
        .authorize_held_create_for_test(&f.identity, &f.intent, &quota, EXPIRY)
        .unwrap();
    let (v1, _, replayed) = f
        .store
        .reserve_kubernetes_create(&f.identity, &legacy, &f.intent)
        .unwrap();
    assert!(!replayed);
    f.store
        .migrate_kubernetes_policy_v9(&f._directory.path().join("mixed-before9"))
        .unwrap();

    let mut second = f.intent.clone();
    second.create_id = Uuid::new_v4().to_string();
    second.session_id = Uuid::new_v4().to_string();
    let admission = f
        .store
        .authorize_kubernetes_create_admission(&f.identity, &second, &policy(&second), &quota)
        .unwrap();
    let (v2_before, _, replayed) = f
        .store
        .reserve_kubernetes_create(&f.identity, &admission, &second)
        .unwrap();
    assert!(!replayed);
    let held = f.store.worker_quota(&quota).unwrap();
    assert_eq!(held.held_workers, 2);
    assert_eq!(held.held_cpu_millis, 1_000);
    assert_eq!(held.held_memory_bytes, 268_435_456);
    migrate_recovery(&f);

    let request = recovery_request(&f.intent);
    let recovered = recover(&f, &f.identity, &request, &quota);
    assert!(!recovered.replayed);
    let held = f.store.worker_quota(&quota).unwrap();
    assert_eq!(held.held_workers, 1);
    assert_eq!(held.held_cpu_millis, 500);
    assert_eq!(held.held_memory_bytes, 134_217_728);

    // A genuine V2 operation AFTER the V3 event must keep its old semantics.
    let reconciliation = f
        .store
        .authorize_kubernetes_create_reconciliation(&f.identity, &second.create_id, &quota)
        .unwrap();
    let cancel = cancellation(&second);
    let (v2_after, closed, replayed) = f
        .store
        .cancel_kubernetes_create(&f.identity, &reconciliation, &cancel)
        .unwrap();
    assert!(!replayed);
    assert_eq!(closed.intent, second);
    assert_eq!(
        closed.state,
        KubernetesCreateStateV1::CancelledBeforeEmission
    );
    assert!(!closed.quota_held);
    assert!(v1.audit_seq < v2_before.audit_seq);
    assert!(v2_before.audit_seq < recovered.receipt.audit_seq);
    assert!(recovered.receipt.audit_seq < v2_after.audit_seq);

    let events = f.store.events(&EventQuery::default()).unwrap();
    for (id, version, receipt) in [
        (&f.intent.create_id, 1, &v1),
        (&second.create_id, 2, &v2_before),
        (&request.operation_id, 3, &recovered.receipt),
        (&cancel.operation_id, 2, &v2_after),
    ] {
        let event = events.iter().find(|stored| &stored.event.id == id).unwrap();
        assert_eq!(event.event.data["version"], version);
        assert_eq!(event.event.data["control_decision"]["version"], version);
        assert_eq!(event.seq, receipt.audit_seq);
        assert_eq!(event.hash, receipt.audit_hash);
    }
    let zero = f.store.worker_quota(&quota).unwrap();
    assert_eq!(zero.held_workers, 0);
    assert_eq!(zero.held_cpu_millis, 0);
    assert_eq!(zero.held_memory_bytes, 0);
    let integrity = f.store.integrity().unwrap();
    assert!(integrity.valid);
    let head = f.store.head().unwrap();
    assert_eq!(head, v2_after.audit_seq);
    let path = f._directory.path().join("tenant.sqlite");
    drop(f.store);
    let reopened =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap();
    let checked = reopened.integrity().unwrap();
    assert!(checked.valid);
    assert_eq!(checked.event_count, integrity.event_count);
    assert_eq!(checked.head_hash, integrity.head_hash);
    assert_eq!(reopened.head().unwrap(), head);
    let zero = reopened.worker_quota(&quota).unwrap();
    assert_eq!(zero.held_workers, 0);
    assert_eq!(zero.held_cpu_millis, 0);
    assert_eq!(zero.held_memory_bytes, 0);
}
