//! Real fresh SQLite and authenticated broker bindings; descriptive profiles
//! contain synthetic metadata. No cluster, provider or runtime is exercised.
use super::*;
use crate::{
    audit::kubernetes_admission_state_tests::Fixture,
    control::QuotaPolicyV1,
    provider::{
        AuthMode, ChoiceSource, ClientKind, ClientTransport, ProviderChoiceProvenanceV1,
        ProviderResolutionV1,
    },
    runtime::kubernetes::session_profile::*,
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use uuid::Uuid;

#[path = "confirmation_tests.rs"]
mod s2_confirmation_tests;

pub(crate) fn prepare(f: &Fixture) {
    let path = f._directory.path().join("tenant.sqlite");
    f.store
        .migrate_kubernetes_policy_v9(&path.with_extension("before9"))
        .unwrap();
    f.store
        .migrate_human_workers_v10(&path.with_extension("before10"))
        .unwrap();
    f.store
        .migrate_kubernetes_session_reservations_v11(&path.with_extension("before11"))
        .unwrap();
}
pub(crate) fn intent(f: &Fixture, session_id: Uuid) -> KubernetesSessionReservationIntentV2 {
    intent_with_network(
        f,
        session_id,
        crate::runtime::kubernetes::session_profile::KubernetesSessionNetworkV2::Denied,
    )
}
pub(crate) fn intent_with_network(
    f: &Fixture,
    session_id: Uuid,
    network: crate::runtime::kubernetes::session_profile::KubernetesSessionNetworkV2,
) -> KubernetesSessionReservationIntentV2 {
    let binding = KubernetesSessionBindingV2::new(
        &f.intent.authority,
        KubernetesSessionIdsV2 {
            installation_id: Uuid::new_v4(),
            control_namespace_uuid: f.intent.control_namespace_uuid.parse().unwrap(),
            broker_id: f.intent.broker_id.parse().unwrap(),
            incarnation_id: f.intent.incarnation_id.parse().unwrap(),
            session_id,
            attempt_id: Uuid::new_v4(),
        },
    )
    .unwrap();
    let provider = ProviderResolutionV1 {
        version: 1,
        client: ClientKind::Codex,
        auth_mode: AuthMode::Api,
        transport: ClientTransport::Batch,
        profile_id: "default".into(),
        profile_revision: 1,
        model: None,
        credential_binding_id: Uuid::new_v4(),
        credential_names: vec![crate::provider::BOOTSTRAP_ENV.into()],
        invocation_preset_version: 1,
        provenance: ProviderChoiceProvenanceV1 {
            client: ChoiceSource::Cli,
            auth_mode: ChoiceSource::Cli,
            profile: ChoiceSource::Cli,
            model: ChoiceSource::Cli,
        },
    };
    let args = [
        "exec",
        "--client",
        "codex",
        "--auth-mode",
        "api",
        "--transport",
        "batch",
        "--preset-version",
        "1",
        "--prompt",
        "public test prompt",
    ]
    .map(str::to_owned);
    let profile = KubernetesSessionProfileV2::build_with_network(
        &f.intent.authority,
        &binding,
        &f.intent.image,
        KubernetesSessionResourcesV2 {
            cpu_millis: 500,
            memory_mib: 128,
            temporary_mib: 8,
            home_mib: 8,
            deadline_seconds: 60,
        },
        &provider,
        KubernetesSessionInvocationV2::new(crate::provider::BOOTSTRAP_PROGRAM, &args).unwrap(),
        &crate::workspace::Manifest::new(),
        network,
    )
    .unwrap();
    KubernetesSessionReservationIntentV2::new(
        &Uuid::new_v4().to_string(),
        &Uuid::new_v4().to_string(),
        &f.intent.authority,
        &profile,
    )
    .unwrap()
}
pub(crate) fn policy(
    intent: &KubernetesSessionReservationIntentV2,
) -> KubernetesSessionReservationPolicyV2 {
    KubernetesSessionReservationPolicyV2 {
        version: 2,
        decision_ttl_seconds: 60,
        approvals: vec![KubernetesSessionReservationApprovalV2 {
            authority_id: intent.authority().authority_id().to_string(),
            authority_fingerprint: intent.authority().fingerprint(),
            image: intent.profile().retained_image().into(),
            client: ClientKind::Codex,
            auth_mode: AuthMode::Api,
            transport: ClientTransport::Batch,
            profile_id: "default".into(),
            profile_revision: 1,
        }],
    }
}
fn close(intent: &KubernetesSessionReservationIntentV2) -> KubernetesSessionReservationCloseV2 {
    KubernetesSessionReservationCloseV2 {
        version: 2,
        operation_id: Uuid::new_v4().to_string(),
        reservation_id: intent.reservation_id().into(),
        reservation_input_hash: intent.input_hash("tenant-a").unwrap(),
        expected_revision: 1,
        reason: KubernetesSessionReservationCloseReasonV2::OperatorCancelled,
    }
}
fn reserve(
    f: &Fixture,
    intent: &KubernetesSessionReservationIntentV2,
    quota: &QuotaPolicyV1,
) -> KubernetesSessionReservationMutationResponseV2 {
    let policy = policy(intent);
    let authorization = f
        .store
        .authorize_kubernetes_session_reservation(&f.identity, intent, &policy, quota)
        .unwrap();
    f.store
        .reserve_kubernetes_session(&f.identity, &authorization, intent, &policy, quota)
        .unwrap()
}

#[test]
fn confirmation_retains_only_a_real_reserved_observation_without_new_event() {
    let f = Fixture::new();
    prepare(&f);
    let intent = intent(&f, Uuid::new_v4());
    let reserved = reserve(&f, &intent, &QuotaPolicyV1::default());
    let observe = f
        .store
        .authorize_kubernetes_session_reconciliation(&f.identity, intent.reservation_id(), None)
        .unwrap();
    let head = f.store.head().unwrap();
    let confirmed = f
        .store
        .confirm_kubernetes_session_before_effect(&f.identity, &observe, intent.reservation_id())
        .unwrap();
    assert_eq!(confirmed.tenant_id(), "tenant-a");
    assert_eq!(
        confirmed.snapshot().input_hash,
        reserved.snapshot.input_hash
    );
    assert_eq!(confirmed.audit_seq(), reserved.receipt.audit_seq);
    assert_eq!(confirmed.audit_hash(), reserved.receipt.audit_hash);
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(
        f.store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
}

#[test]
fn confirmation_refuses_closed_reservations_and_a_close_authorization() {
    let f = Fixture::new();
    prepare(&f);
    let intent = intent(&f, Uuid::new_v4());
    reserve(&f, &intent, &QuotaPolicyV1::default());
    let request = close(&intent);
    let close = f
        .store
        .authorize_kubernetes_session_reconciliation(
            &f.identity,
            intent.reservation_id(),
            Some(&request),
        )
        .unwrap();
    let head = f.store.head().unwrap();
    let error = f
        .store
        .confirm_kubernetes_session_before_effect(&f.identity, &close, intent.reservation_id())
        .err()
        .unwrap();
    assert_eq!(
        error.downcast_ref::<Failure>(),
        Some(&Failure::PermissionDenied)
    );
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(
        f.store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
    f.store
        .close_kubernetes_session_before_effect(&f.identity, &close, &request)
        .unwrap();
    let observe = f
        .store
        .authorize_kubernetes_session_reconciliation(&f.identity, intent.reservation_id(), None)
        .unwrap();
    let head = f.store.head().unwrap();
    let error = f
        .store
        .confirm_kubernetes_session_before_effect(&f.identity, &observe, intent.reservation_id())
        .err()
        .unwrap();
    assert_eq!(
        error.downcast_ref::<Failure>(),
        Some(&Failure::PermissionDenied)
    );
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(
        f.store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0
    );
}

#[test]
fn schema12_preserves_v2_reservation_quota_and_before_effect_confirmation() {
    let f = Fixture::new();
    prepare(&f);
    let intent = intent(&f, Uuid::new_v4());
    let policy = QuotaPolicyV1::default();
    let first = reserve(&f, &intent, &policy);
    let before = f.store.worker_quota(&policy).unwrap();
    let path = f._directory.path().join("tenant.sqlite");
    f.store
        .migrate_broker_checkpoints_v12(&path.with_extension("before12"))
        .unwrap();
    let after = f.store.worker_quota(&policy).unwrap();
    assert_eq!(
        (
            after.held_workers,
            after.held_cpu_millis,
            after.held_memory_bytes
        ),
        (
            before.held_workers,
            before.held_cpu_millis,
            before.held_memory_bytes
        )
    );
    let observe = f
        .store
        .authorize_kubernetes_session_reconciliation(&f.identity, intent.reservation_id(), None)
        .unwrap();
    let confirmed = f
        .store
        .confirm_kubernetes_session_before_effect(&f.identity, &observe, intent.reservation_id())
        .unwrap();
    assert_eq!(confirmed.snapshot().input_hash, first.snapshot.input_hash);
    assert_eq!(confirmed.audit_seq(), first.receipt.audit_seq);
    assert_eq!(confirmed.audit_hash(), first.receipt.audit_hash);
    assert!(f.store.integrity().unwrap().valid);
}

#[test]
fn reserve_close_replay_and_inactive_reopen_preserve_the_effect_fence() {
    let f = Fixture::new();
    prepare(&f);
    let intent = intent(&f, Uuid::new_v4());
    let quota = QuotaPolicyV1::default();
    let first = reserve(&f, &intent, &quota);
    assert!(!first.replayed);
    assert_eq!(first.snapshot.state.revision, 1);
    assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 1);
    let head = f.store.head().unwrap();
    let retry = reserve(&f, &intent, &quota);
    assert!(retry.replayed);
    assert_eq!(retry.receipt, first.receipt);
    assert_eq!(f.store.head().unwrap(), head);
    let request = close(&intent);
    let cap = f
        .store
        .authorize_kubernetes_session_reconciliation(
            &f.identity,
            intent.reservation_id(),
            Some(&request),
        )
        .unwrap();
    let closed = f
        .store
        .close_kubernetes_session_before_effect(&f.identity, &cap, &request)
        .unwrap();
    assert_eq!(
        closed.snapshot.state.state,
        KubernetesSessionReservationStateV2::ClosedBeforeEffect
    );
    assert_eq!(
        closed.snapshot.state.effect_fence,
        KubernetesSessionEffectFenceV2::NeverIssued { revision: 0 }
    );
    assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 0);
    let cap = f
        .store
        .authorize_kubernetes_session_reconciliation(
            &f.identity,
            intent.reservation_id(),
            Some(&request),
        )
        .unwrap();
    let head = f.store.head().unwrap();
    let retry = f
        .store
        .close_kubernetes_session_before_effect(&f.identity, &cap, &request)
        .unwrap();
    assert!(retry.replayed);
    assert_eq!(retry.receipt, closed.receipt);
    assert_eq!(f.store.head().unwrap(), head);
    let different = close(&intent);
    assert!(
        f.store
            .authorize_kubernetes_session_reconciliation(
                &f.identity,
                intent.reservation_id(),
                Some(&different)
            )
            .is_err()
    );
    assert_eq!(f.store.head().unwrap(), head);
    assert!(f.store.integrity().unwrap().valid);
    let path = f._directory.path().join("tenant.sqlite");
    drop(f.store);
    let store =
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap();
    assert!(!store.supervision_state().unwrap().active);
    let cap = store
        .authorize_kubernetes_session_reconciliation(&f.identity, intent.reservation_id(), None)
        .unwrap();
    let current = store
        .observe_kubernetes_session(&f.identity, &cap, intent.reservation_id())
        .unwrap();
    assert_eq!(current.snapshot.state, closed.snapshot.state);
    assert_eq!(store.head().unwrap(), head);
    assert!(store.integrity().unwrap().valid);
}

#[test]
fn mixed_v1_v2_quota_refusal_and_global_id_conflicts_leave_audit_unchanged() {
    let f = Fixture::new();
    prepare(&f);
    let mut quota = QuotaPolicyV1::default();
    quota.max_active_workers = 1;
    let v1_policy = crate::control::kubernetes_admission::KubernetesHeldCreatePolicyV1 {
        version: 1,
        decision_ttl_seconds: 60,
        approvals: vec![
            crate::control::kubernetes_admission::KubernetesHeldCreateApprovalV1 {
                authority_fingerprint: f.intent.authority.fingerprint(),
                image: f.intent.image.clone(),
                profile: f.intent.profile.clone(),
            },
        ],
    };
    let authorization = f
        .store
        .authorize_kubernetes_create_admission(&f.identity, &f.intent, &v1_policy, &quota)
        .unwrap();
    f.store
        .reserve_kubernetes_create(&f.identity, &authorization, &f.intent)
        .unwrap();
    let input = intent(&f, Uuid::new_v4());
    let admission_policy = policy(&input);
    let authorization = f
        .store
        .authorize_kubernetes_session_reservation(&f.identity, &input, &admission_policy, &quota)
        .unwrap();
    let head = f.store.head().unwrap();
    assert!(
        f.store
            .reserve_kubernetes_session(
                &f.identity,
                &authorization,
                &input,
                &admission_policy,
                &quota
            )
            .is_err()
    );
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(f.store.worker_quota(&quota).unwrap().held_workers, 1);
    let collision = intent(&f, f.intent.session_id.parse().unwrap());
    let cap = f
        .store
        .authorize_kubernetes_session_reservation(
            &f.identity,
            &collision,
            &policy(&collision),
            &QuotaPolicyV1::default(),
        )
        .unwrap();
    assert!(
        f.store
            .reserve_kubernetes_session(
                &f.identity,
                &cap,
                &collision,
                &policy(&collision),
                &QuotaPolicyV1::default()
            )
            .is_err()
    );
    assert_eq!(f.store.head().unwrap(), head);
    assert!(f.store.integrity().unwrap().valid);
}

#[test]
fn successor_recovers_revoked_original_and_old_credentials_are_denied() {
    let f = Fixture::new();
    prepare(&f);
    let input = intent(&f, Uuid::new_v4());
    reserve(&f, &input, &QuotaPolicyV1::default());
    let request = close(&input);
    let old = f
        .store
        .authorize_kubernetes_session_reconciliation(
            &f.identity,
            input.reservation_id(),
            Some(&request),
        )
        .unwrap();
    f.store
        .revoke_broker(&crate::broker_registry::BrokerRevocationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: f.identity.broker_id().into(),
            expected_revision: f.identity.grant_revision(),
        })
        .unwrap();
    assert!(
        f.store
            .close_kubernetes_session_before_effect(&f.identity, &old, &request)
            .is_err()
    );
    let token = "public-successor-session-test-token-0000000000000000";
    let credential = Uuid::new_v4().to_string();
    let incarnation = Uuid::new_v4().to_string();
    f.store
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential.clone(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: f.identity.broker_id().into(),
                incarnation_id: incarnation.clone(),
            },
            sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
            expires_at: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    f.store
        .mutate_broker_grant(&crate::broker_registry::BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: f.identity.broker_id().into(),
            expected_revision: Some(f.identity.grant_revision() + 1),
            credential_id: credential,
            incarnation_id: incarnation,
            authorities: vec![crate::broker_registry::BrokerAuthorityV1 {
                authority_id: Uuid::new_v4().to_string(),
                runtime_kind: BrokerRuntimeKindV1::Kubernetes,
            }],
            lease_seconds: 120,
            not_after: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    let Some(crate::workload::AuthenticatedWorkload::Broker(successor)) =
        f.store.authenticate_workload_token(token).unwrap()
    else {
        panic!("real successor required");
    };
    let policy = KubernetesSessionRecoveryPolicyV2 {
        version: 2,
        decision_ttl_seconds: 60,
        allowed_broker_ids: vec![f.identity.broker_id().into()],
    };
    let KubernetesSessionRecoveryAuthorizationV2::BeforeEffect(cap) = f
        .store
        .authorize_kubernetes_session_recovery(
            &successor,
            input.reservation_id(),
            Some(&request),
            &policy,
        )
        .unwrap()
    else {
        panic!("reserved only");
    };
    let closed = f
        .store
        .recover_kubernetes_session_before_effect(&successor, &cap, &request, &policy)
        .unwrap();
    assert_eq!(
        f.store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0
    );
    let KubernetesSessionRecoveryAuthorizationV2::ReceiptLookup(cap) = f
        .store
        .authorize_kubernetes_session_recovery(
            &successor,
            input.reservation_id(),
            Some(&request),
            &policy,
        )
        .unwrap()
    else {
        panic!("historical lookup only");
    };
    let head = f.store.head().unwrap();
    let retry = f
        .store
        .lookup_kubernetes_session_recovery_receipt(&successor, &cap, &request, &policy)
        .unwrap();
    assert!(retry.replayed);
    assert_eq!(retry.receipt, closed.receipt);
    assert_eq!(f.store.head().unwrap(), head);
    assert!(f.store.integrity().unwrap().valid);
}

#[test]
fn shared_workload_budget_survives_reopen_and_saturation_keeps_quota() {
    let mut f = Fixture::new();
    prepare(&f);
    let capacity = WorkloadAuditPolicyV1 {
        version: 1,
        tenant: AuditCapacityV1 {
            events: 3,
            bytes: 3_000_000,
        },
        principal: AuditCapacityV1 {
            events: 1,
            bytes: 1_000_000,
        },
    };
    f.store = f
        .store
        .with_workload_audit_policy(capacity.clone())
        .unwrap();
    let input = intent(&f, Uuid::new_v4());
    reserve(&f, &input, &QuotaPolicyV1::default());
    let request = close(&input);
    let cap = f
        .store
        .authorize_kubernetes_session_reconciliation(
            &f.identity,
            input.reservation_id(),
            Some(&request),
        )
        .unwrap();
    let head = f.store.head().unwrap();
    let error = f
        .store
        .close_kubernetes_session_before_effect(&f.identity, &cap, &request)
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::WorkloadAuditBudgetExhausted)
    );
    assert_eq!(
        f.store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
    assert_eq!(f.store.head().unwrap(), head);
    let path = f._directory.path().join("tenant.sqlite");
    drop(f.store);
    let store = Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000)
        .unwrap()
        .with_workload_audit_policy(capacity)
        .unwrap();
    let cap = store
        .authorize_kubernetes_session_reconciliation(
            &f.identity,
            input.reservation_id(),
            Some(&request),
        )
        .unwrap();
    assert!(
        store
            .close_kubernetes_session_before_effect(&f.identity, &cap, &request)
            .is_err()
    );
    assert_eq!(store.head().unwrap(), head);
    assert_eq!(
        store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        1
    );
    let store = store
        .with_workload_audit_policy(WorkloadAuditPolicyV1 {
            version: 1,
            tenant: AuditCapacityV1 {
                events: 3,
                bytes: 3_000_000,
            },
            principal: AuditCapacityV1 {
                events: 2,
                bytes: 2_000_000,
            },
        })
        .unwrap();
    let cap = store
        .authorize_kubernetes_session_reconciliation(
            &f.identity,
            input.reservation_id(),
            Some(&request),
        )
        .unwrap();
    assert!(
        !store
            .close_kubernetes_session_before_effect(&f.identity, &cap, &request)
            .unwrap()
            .replayed
    );
    assert_eq!(
        store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0
    );
    assert!(store.integrity().unwrap().valid);
}

#[test]
fn rejected_policy_and_wire_bounds_leave_quota_and_audit_unchanged() {
    let f = Fixture::new();
    prepare(&f);
    let input = intent(&f, Uuid::new_v4());
    let base = policy(&input);
    let head = f.store.head().unwrap();
    let mut invalid = base.clone();
    invalid.decision_ttl_seconds = 0;
    assert!(
        f.store
            .authorize_kubernetes_session_reservation(
                &f.identity,
                &input,
                &invalid,
                &QuotaPolicyV1::default()
            )
            .is_err()
    );
    invalid = base.clone();
    invalid.decision_ttl_seconds = 61;
    assert!(invalid.validate().is_err());
    invalid = base.clone();
    invalid.approvals.push(invalid.approvals[0].clone());
    assert!(invalid.validate().is_err());
    invalid = base.clone();
    invalid.approvals[0].profile_id = "a".repeat(81);
    assert!(invalid.validate().is_err());
    let mut bounded = base.clone();
    bounded.approvals[0].profile_id = "a".repeat(80);
    bounded.validate().unwrap();
    bounded = base.clone();
    bounded.approvals = (0..64)
        .map(|i| {
            let mut approval = base.approvals[0].clone();
            approval.profile_id = format!("profile{i:03}{}", "a".repeat(60));
            approval.image = format!("{}@sha256:{}", "a".repeat(180), "b".repeat(64));
            approval
        })
        .collect();
    assert!(validation::canonical(&bounded).unwrap().len() > 16_384);
    assert!(
        f.store
            .authorize_kubernetes_session_reservation(
                &f.identity,
                &input,
                &bounded,
                &QuotaPolicyV1::default()
            )
            .is_err()
    );
    let mut value = serde_json::to_value(&input).unwrap();
    value["generation"] = json!(2);
    assert!(KubernetesSessionReservationIntentV2::retained_from_value(value).is_err());
    let mut value = serde_json::to_value(&input).unwrap();
    value["reservation_id"] = json!(Uuid::nil().to_string());
    assert!(KubernetesSessionReservationIntentV2::retained_from_value(value).is_err());
    let mut encoded = serde_json::to_vec(&input).unwrap();
    encoded.extend(std::iter::repeat_n(b' ', 80 * 1024));
    assert!(validation::unique(&encoded, 80 * 1024).is_err());
    assert!(validation::unique(br#"{"version":2,"version":2}"#, 80 * 1024).is_err());
    assert_eq!(f.store.head().unwrap(), head);
    assert_eq!(
        f.store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        0
    );
    assert!(f.store.integrity().unwrap().valid);
}
