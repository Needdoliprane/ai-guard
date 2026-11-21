//! Real extractor/blocking bridges and real SQLite; no proof constructor,
//! cluster, provider, HTTP server or runtime-effect claim is used here.
use super::*;
use crate::{
    api::{
        GatewayState, with_authenticated_control_recovery_for_test,
        with_control_recovery_auth_pair_for_test,
    },
    audit::{
        AuditCapacityFailureV1, AuditCapacityV1, WorkloadAuditPolicyV1,
        kubernetes_admission_state_tests::Fixture,
    },
    control::QuotaPolicyV1,
    provider::{
        AuthMode, ChoiceSource, ClientKind, ClientTransport, ProviderChoiceProvenanceV1,
        ProviderResolutionV1,
    },
    runtime::kubernetes::{KubernetesAuthorityV1, session_profile::*},
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use axum::http::{HeaderMap, HeaderValue, header};
use std::sync::Arc;
use uuid::Uuid;

const RUNNER: &str = "public-core-control-recovery-runner-token-000000000001";
const VIEWER: &str = "public-core-control-recovery-viewer-token-000000000001";
const ROTATED_RUNNER: &str = "public-core-control-recovery-runner-token-rotated-00002";
const ROTATED_VIEWER: &str = "public-core-control-recovery-viewer-token-rotated-00002";

struct Setup {
    directory: tempfile::TempDir,
    state: GatewayState,
    policy: KubernetesSessionControlRecoveryPolicyV2,
    intents: Vec<KubernetesSessionReservationIntentV2>,
    broker: BrokerIdentityV1,
}
fn headers(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    headers
}
fn seed(
    store: &Store,
    identity: &BrokerIdentityV1,
    authority: &KubernetesAuthorityV1,
    incarnation: &str,
) -> KubernetesSessionReservationIntentV2 {
    let binding = KubernetesSessionBindingV2::new(
        authority,
        KubernetesSessionIdsV2 {
            installation_id: Uuid::new_v4(),
            control_namespace_uuid: store
                .supervision_state()
                .unwrap()
                .control_namespace_uuid
                .unwrap()
                .parse()
                .unwrap(),
            broker_id: identity.broker_id().parse().unwrap(),
            incarnation_id: incarnation.parse().unwrap(),
            session_id: Uuid::new_v4(),
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
        "public Core recovery test",
    ]
    .map(str::to_owned);
    let image = format!("worker@sha256:{}", "b".repeat(64));
    let profile = KubernetesSessionProfileV2::build(
        authority,
        &binding,
        &image,
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
    )
    .unwrap();
    let intent = KubernetesSessionReservationIntentV2::new(
        &Uuid::new_v4().to_string(),
        &Uuid::new_v4().to_string(),
        authority,
        &profile,
    )
    .unwrap();
    let policy = KubernetesSessionReservationPolicyV2 {
        version: 2,
        decision_ttl_seconds: 60,
        approvals: vec![KubernetesSessionReservationApprovalV2 {
            authority_id: authority.authority_id().to_string(),
            authority_fingerprint: authority.fingerprint(),
            image,
            client: ClientKind::Codex,
            auth_mode: AuthMode::Api,
            transport: ClientTransport::Batch,
            profile_id: "default".into(),
            profile_revision: 1,
        }],
    };
    let auth = store
        .authorize_kubernetes_session_reservation(
            identity,
            &intent,
            &policy,
            &QuotaPolicyV1::default(),
        )
        .unwrap();
    store
        .reserve_kubernetes_session(identity, &auth, &intent, &policy, &QuotaPolicyV1::default())
        .unwrap();
    let db = store.db.lock().unwrap();
    let (stored, record) = replay::reservation(&db, intent.reservation_id(), "tenant-a").unwrap();
    assert_eq!(
        serde_json::to_value(record.authorization.broker().unwrap()).unwrap(),
        stored.event.data["control_decision"]
    );
    assert_eq!(serde_json::to_value(&record).unwrap(), stored.event.data);
    drop(db);
    intent
}
fn setup(count: usize, separate_broker: bool, budget: Option<WorkloadAuditPolicyV1>) -> Setup {
    let mut f = Fixture::new();
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
    if let Some(budget) = budget {
        f.store = f.store.with_workload_audit_policy(budget).unwrap();
    }
    let mut intents = vec![seed(
        &f.store,
        &f.identity,
        &f.intent.authority,
        &f.intent.incarnation_id,
    )];
    if count == 2 {
        if separate_broker {
            let broker = Uuid::new_v4().to_string();
            let incarnation = Uuid::new_v4().to_string();
            let credential = Uuid::new_v4().to_string();
            let token = "public-core-second-broker-token-0000000000000000001";
            let authority = KubernetesAuthorityV1::new(
                Uuid::new_v4(),
                "https://127.0.0.1:6443",
                "11111111-1111-4111-8111-111111111111",
                "tenant-a",
                "22222222-2222-4222-8222-222222222222",
                &"a".repeat(64),
            )
            .unwrap();
            f.store
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
            f.store
                .mutate_broker_grant(&crate::broker_registry::BrokerGrantMutationV1 {
                    version: 1,
                    operation_id: Uuid::new_v4().to_string(),
                    broker_id: broker,
                    expected_revision: None,
                    credential_id: credential,
                    incarnation_id: incarnation.clone(),
                    authorities: vec![crate::broker_registry::BrokerAuthorityV1 {
                        authority_id: authority.authority_id().to_string(),
                        runtime_kind: BrokerRuntimeKindV1::Kubernetes,
                    }],
                    lease_seconds: 120,
                    not_after: "2100-01-01T00:00:00.000Z".into(),
                })
                .unwrap();
            let Some(crate::workload::AuthenticatedWorkload::Broker(identity)) =
                f.store.authenticate_workload_token(token).unwrap()
            else {
                panic!("real second broker required")
            };
            intents.push(seed(&f.store, &identity, &authority, &incarnation));
        } else {
            intents.push(seed(
                &f.store,
                &f.identity,
                &f.intent.authority,
                &f.intent.incarnation_id,
            ));
        }
    }
    let policy = KubernetesSessionControlRecoveryPolicyV2 {
        version: 2,
        decision_ttl_seconds: 60,
        allowed_broker_ids: intents
            .iter()
            .map(|intent| intent.ids().unwrap().broker_id.to_string())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
    };
    let state = GatewayState::new(
        Arc::new(f.store),
        RUNNER.into(),
        VIEWER.into(),
        crate::policy::Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .with_kubernetes_session_control_recovery_policies(vec![("tenant-a".into(), policy.clone())])
    .unwrap();
    Setup {
        directory: f._directory,
        state,
        policy,
        intents,
        broker: f.identity,
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
fn before_effect<'a>(
    value: KubernetesSessionControlRecoveryAuthorizationV2<'a>,
) -> ControlRecoverBeforeEffectV2<'a> {
    let KubernetesSessionControlRecoveryAuthorizationV2::BeforeEffect(cap) = value else {
        panic!("effect-free capability required")
    };
    cap
}
fn unchanged(store: &Store, head: &i64, count: u32) {
    assert_eq!(&store.head().unwrap(), head);
    assert_eq!(
        store
            .worker_quota(&QuotaPolicyV1::default())
            .unwrap()
            .held_workers,
        count
    );
    assert!(store.integrity().unwrap().valid);
}

#[tokio::test]
async fn control_caps_bind_request_scope_policy_origin_and_both_deadlines() {
    let s = setup(1, false, None);
    let input = s.intents[0].clone();
    let request = close(&input);
    let policy = s.policy.clone();
    with_control_recovery_auth_pair_for_test(
        &s.state,
        headers(RUNNER),
        move |auth, other, store| {
            let head = store.head()?;
            assert_ne!(auth.request_auth_id(), other.request_auth_id());
            let cap = before_effect(store.authorize_kubernetes_session_control_recovery(
                auth,
                &KubernetesSessionControlRecoveryTargetV2::ObserveReserved {
                    reservation_id: input.reservation_id().into(),
                    reservation_input_hash: input.input_hash("tenant-a")?,
                },
                &policy,
            )?);
            assert!(
                store
                    .observe_kubernetes_session_control_recovery(other, &cap, &policy)
                    .is_err()
            );
            assert!(
                store
                    .close_kubernetes_session_control_before_effect(auth, &cap, &request, &policy)
                    .is_err()
            );
            assert_eq!(
                store
                    .observe_kubernetes_session_control_recovery(auth, &cap, &policy)?
                    .snapshot
                    .state
                    .revision,
                1
            );
            let head = store.head()?;
            let confirmed =
                store.confirm_kubernetes_session_control_before_effect(auth, &cap, &policy)?;
            assert_eq!(confirmed.tenant_id(), store.tenant_id());
            assert_eq!(
                confirmed.snapshot().input_hash,
                input.input_hash(store.tenant_id())?
            );
            assert!(confirmed.audit_seq() > 0);
            assert_eq!(store.head()?, head);
            assert!(
                store
                    .confirm_kubernetes_session_control_before_effect(other, &cap, &policy)
                    .is_err()
            );
            assert_eq!(store.head()?, head);
            let mut cap = before_effect(store.authorize_kubernetes_session_control_recovery(
                auth,
                &KubernetesSessionControlRecoveryTargetV2::CloseBeforeEffect {
                    close: request.clone(),
                },
                &policy,
            )?);
            let mut changed_policy = policy.clone();
            changed_policy.decision_ttl_seconds = 59;
            assert!(
                store
                    .close_kubernetes_session_control_before_effect(
                        auth,
                        &cap,
                        &request,
                        &changed_policy
                    )
                    .is_err()
            );
            let mut different = request.clone();
            different.operation_id = Uuid::new_v4().to_string();
            assert!(
                store
                    .close_kubernetes_session_control_before_effect(auth, &cap, &different, &policy)
                    .is_err()
            );
            let original = cap.evidence.clone();
            cap.evidence.origin_hash = "e".repeat(64);
            cap.evidence.decision_hash = cap.evidence.commitment()?;
            assert!(
                store
                    .close_kubernetes_session_control_before_effect(auth, &cap, &request, &policy)
                    .is_err()
            );
            cap.evidence = original.clone();
            cap.evidence.control_namespace_uuid = Uuid::new_v4().to_string();
            cap.evidence.decision_hash = cap.evidence.commitment()?;
            assert!(
                store
                    .close_kubernetes_session_control_before_effect(auth, &cap, &request, &policy)
                    .is_err()
            );
            cap.evidence = original;
            cap.evidence
                .validate(&input, parse_time(&cap.evidence.issued_at)?)?;
            assert!(
                cap.evidence
                    .validate(&input, parse_time(&cap.evidence.expires_at)?)
                    .is_err()
            );
            cap.monotone_deadline = Instant::now();
            assert!(
                store
                    .close_kubernetes_session_control_before_effect(auth, &cap, &request, &policy)
                    .is_err()
            );
            // Real auth is still fresh. Give only the private capability a shorter
            // UTC expiry, keeping its hash, policy and monotone bound valid; prove
            // the actual Store path rejects that expiry independently of monotone.
            let mut utc_cap = before_effect(store.authorize_kubernetes_session_control_recovery(
                auth,
                &KubernetesSessionControlRecoveryTargetV2::CloseBeforeEffect {
                    close: request.clone(),
                },
                &policy,
            )?);
            utc_cap.evidence.expires_at = format_time(Utc::now() + chrono::Duration::seconds(1));
            utc_cap.evidence.decision_hash = utc_cap.evidence.commitment()?;
            utc_cap.evidence.validate(&input, Utc::now())?;
            std::thread::sleep(Duration::from_millis(1050));
            assert!(Instant::now() < utc_cap.monotone_deadline);
            drop(auth.acquire_usage(&store)?);
            assert!(
                store
                    .close_kubernetes_session_control_before_effect(
                        auth, &utc_cap, &request, &policy
                    )
                    .is_err()
            );
            unchanged(&store, &head, 1);
            Ok(())
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn issued_control_cap_cannot_be_used_after_real_epoch_retirement() {
    let s = setup(1, false, None);
    let handle = s.state.control_recovery_retirement();
    let policy = s.policy.clone();
    let request = close(&s.intents[0]);
    with_authenticated_control_recovery_for_test(&s.state, headers(RUNNER), move |auth, store| {
        let head = store.head()?;
        let cap = before_effect(store.authorize_kubernetes_session_control_recovery(
            auth,
            &KubernetesSessionControlRecoveryTargetV2::CloseBeforeEffect {
                close: request.clone(),
            },
            &policy,
        )?);
        handle.retire();
        assert!(handle.drained());
        assert!(
            store
                .close_kubernetes_session_control_before_effect(auth, &cap, &request, &policy)
                .is_err()
        );
        unchanged(&store, &head, 1);
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn usage_lease_covers_real_commit_and_new_epoch_only_looks_up_the_exact_receipt() {
    let s = setup(1, false, None);
    let handle = Arc::new(s.state.control_recovery_retirement());
    let state = s.state.clone();
    let policy = s.policy.clone();
    let request = close(&s.intents[0]);
    let saved_request = request.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let task = tokio::spawn(async move {
        with_authenticated_control_recovery_for_test(&state, headers(RUNNER), move |auth, store| {
            let cap = before_effect(store.authorize_kubernetes_session_control_recovery(
                auth,
                &KubernetesSessionControlRecoveryTargetV2::CloseBeforeEffect {
                    close: request.clone(),
                },
                &policy,
            )?);
            BEFORE_COMMIT.with(|hook| *hook.borrow_mut() = Some((ready_tx, resume_rx)));
            let closed = store
                .close_kubernetes_session_control_before_effect(auth, &cap, &request, &policy)?;
            Ok((closed, auth.auth_epoch_id().to_string()))
        })
        .await
    });
    tokio::task::spawn_blocking(move || ready_rx.recv_timeout(Duration::from_secs(5)))
        .await
        .unwrap()
        .unwrap();
    handle.retire();
    assert!(!handle.drained());
    resume_tx.send(()).unwrap();
    let (closed, old_epoch) = task.await.unwrap().unwrap();
    assert!(!closed.replayed);
    assert!(handle.drained());
    // A new extractor performs actual collision reads; invoke it only after
    // the deliberately held transaction is released, so this test cannot
    // deadlock its own commit barrier. Unused-cap denial has its own canary.
    assert!(
        with_authenticated_control_recovery_for_test(&s.state, headers(RUNNER), |_, _| Ok(()))
            .await
            .is_err()
    );
    let reopened = Store::open_existing_for_tenant(
        &s.directory.path().join("tenant.sqlite"),
        "tenant-a",
        Redactor::new(vec![]),
        10_000,
    )
    .unwrap();
    assert!(!reopened.supervision_state().unwrap().active);
    let head = reopened.head().unwrap();
    let policy = s.policy.clone();
    let state = GatewayState::new(
        Arc::new(reopened),
        ROTATED_RUNNER.into(),
        ROTATED_VIEWER.into(),
        crate::policy::Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .with_kubernetes_session_control_recovery_policies(vec![("tenant-a".into(), policy.clone())])
    .unwrap();
    assert!(
        with_authenticated_control_recovery_for_test(&state, headers(RUNNER), |_, _| Ok(()))
            .await
            .is_err()
    );
    with_authenticated_control_recovery_for_test(
        &state,
        headers(ROTATED_RUNNER),
        move |auth, store| {
            assert_ne!(auth.auth_epoch_id().to_string(), old_epoch);
            let target = KubernetesSessionControlRecoveryTargetV2::LookupCloseReceipt {
                reservation_id: saved_request.reservation_id.clone(),
                reservation_input_hash: saved_request.reservation_input_hash.clone(),
                close_input_hash: saved_request.input_hash()?,
                operation_id: saved_request.operation_id.clone(),
            };
            let KubernetesSessionControlRecoveryAuthorizationV2::ReceiptLookup(cap) =
                store.authorize_kubernetes_session_control_recovery(auth, &target, &policy)?
            else {
                panic!("lookup only")
            };
            let replay =
                store.lookup_kubernetes_session_control_recovery_receipt(auth, &cap, &policy)?;
            assert!(replay.replayed);
            assert_eq!(replay.receipt, closed.receipt);
            let db = store.db.lock().unwrap();
            let (stored, record) = replay::operation(&db, &saved_request.operation_id)?.unwrap();
            assert_eq!(
                record.authorization.control().unwrap().actor.auth_epoch_id,
                old_epoch
            );
            assert_eq!(stored.seq, replay.receipt.audit_seq);
            drop(db);
            unchanged(&store, &head, 0);
            Ok(())
        },
    )
    .await
    .unwrap();
}

async fn budget_reopen_case(separate_broker: bool, budget: WorkloadAuditPolicyV1) {
    let s = setup(2, separate_broker, Some(budget.clone()));
    let first = close(&s.intents[0]);
    let second = close(&s.intents[1]);
    let policy = s.policy.clone();
    with_authenticated_control_recovery_for_test(&s.state, headers(RUNNER), move |auth, store| {
        let cap = before_effect(store.authorize_kubernetes_session_control_recovery(
            auth,
            &KubernetesSessionControlRecoveryTargetV2::CloseBeforeEffect {
                close: first.clone(),
            },
            &policy,
        )?);
        store.close_kubernetes_session_control_before_effect(auth, &cap, &first, &policy)?;
        Ok(())
    })
    .await
    .unwrap();
    let reopened = Store::open_existing_for_tenant(
        &s.directory.path().join("tenant.sqlite"),
        "tenant-a",
        Redactor::new(vec![]),
        10_000,
    )
    .unwrap()
    .with_workload_audit_policy(budget)
    .unwrap();
    let head = reopened.head().unwrap();
    let policy = s.policy.clone();
    let state = GatewayState::new(
        Arc::new(reopened),
        ROTATED_RUNNER.into(),
        ROTATED_VIEWER.into(),
        crate::policy::Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .with_kubernetes_session_control_recovery_policies(vec![("tenant-a".into(), policy.clone())])
    .unwrap();
    with_authenticated_control_recovery_for_test(
        &state,
        headers(ROTATED_RUNNER),
        move |auth, store| {
            let cap = before_effect(store.authorize_kubernetes_session_control_recovery(
                auth,
                &KubernetesSessionControlRecoveryTargetV2::CloseBeforeEffect {
                    close: second.clone(),
                },
                &policy,
            )?);
            let error = store
                .close_kubernetes_session_control_before_effect(auth, &cap, &second, &policy)
                .unwrap_err();
            assert_eq!(
                error.downcast_ref::<AuditCapacityFailureV1>(),
                Some(&AuditCapacityFailureV1::WorkloadAuditBudgetExhausted)
            );
            unchanged(&store, &head, 1);
            Ok(())
        },
    )
    .await
    .unwrap();
}
#[tokio::test]
async fn control_close_spends_the_original_broker_budget_after_reopen() {
    budget_reopen_case(
        false,
        WorkloadAuditPolicyV1 {
            version: 1,
            tenant: AuditCapacityV1 {
                events: 4,
                bytes: 4_000_000,
            },
            principal: AuditCapacityV1 {
                events: 3,
                bytes: 3_000_000,
            },
        },
    )
    .await;
}
#[tokio::test]
async fn control_close_spends_shared_tenant_budget_without_switching_principal() {
    budget_reopen_case(
        true,
        WorkloadAuditPolicyV1 {
            version: 1,
            tenant: AuditCapacityV1 {
                events: 3,
                bytes: 3_000_000,
            },
            principal: AuditCapacityV1 {
                events: 2,
                bytes: 2_000_000,
            },
        },
    )
    .await;
}

#[tokio::test]
async fn late_control_close_failure_rolls_back_audit_projection_and_workload_budget() {
    let s = setup(
        1,
        false,
        Some(WorkloadAuditPolicyV1 {
            version: 1,
            tenant: AuditCapacityV1 {
                events: 3,
                bytes: 3_000_000,
            },
            principal: AuditCapacityV1 {
                events: 2,
                bytes: 2_000_000,
            },
        }),
    );
    let policy = s.policy.clone();
    let request = close(&s.intents[0]);
    with_authenticated_control_recovery_for_test(&s.state, headers(RUNNER), move |auth, store| {
        let head = store.head()?;
        let cap = before_effect(store.authorize_kubernetes_session_control_recovery(
            auth,
            &KubernetesSessionControlRecoveryTargetV2::CloseBeforeEffect {
                close: request.clone(),
            },
            &policy,
        )?);
        let (ready, receiver) = std::sync::mpsc::channel();
        drop(receiver);
        let (resume, suspended) = std::sync::mpsc::channel();
        drop(resume);
        BEFORE_COMMIT.with(|hook| *hook.borrow_mut() = Some((ready, suspended)));
        // This hook fails only after the real typed append and CAS, while both
        // the SQLite transaction and actual Auth usage lease are still held.
        assert!(
            store
                .close_kubernetes_session_control_before_effect(auth, &cap, &request, &policy)
                .is_err()
        );
        unchanged(&store, &head, 1);
        let cap = before_effect(store.authorize_kubernetes_session_control_recovery(
            auth,
            &KubernetesSessionControlRecoveryTargetV2::CloseBeforeEffect {
                close: request.clone(),
            },
            &policy,
        )?);
        let closed =
            store.close_kubernetes_session_control_before_effect(auth, &cap, &request, &policy)?;
        assert!(!closed.replayed);
        assert_eq!(store.head()?, head + 1);
        let db = store.db.lock().unwrap();
        let (_, record) = replay::operation(&db, &request.operation_id)?.unwrap();
        assert!(record.actor.identity().is_err());
        assert_eq!(
            record.authorization.control().unwrap().actor.kind,
            "control_shared_runner"
        );
        let (_, _, creator) = original_creator(&record.creator)?;
        assert_eq!(
            creator.broker_id,
            record.intent.ids()?.broker_id.to_string()
        );
        assert_eq!(
            serde_json::to_value(&record.authorization)?,
            serde_json::to_value(record.authorization.control().unwrap())?
        );
        drop(db);
        assert!(store.integrity()?.valid);
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn two_real_control_requests_race_close_with_one_cas_append_and_release() {
    let s = setup(1, false, None);
    let requests = [close(&s.intents[0]), close(&s.intents[0])];
    let (ready, waiting) = std::sync::mpsc::channel();
    let mut tasks = Vec::new();
    let mut releases = Vec::new();
    for request in requests {
        let state = s.state.clone();
        let policy = s.policy.clone();
        let ready = ready.clone();
        let (release, resume) = std::sync::mpsc::channel();
        releases.push(release);
        tasks.push(tokio::spawn(async move {
            with_authenticated_control_recovery_for_test(
                &state,
                headers(RUNNER),
                move |auth, store| {
                    let cap = before_effect(store.authorize_kubernetes_session_control_recovery(
                        auth,
                        &KubernetesSessionControlRecoveryTargetV2::CloseBeforeEffect {
                            close: request.clone(),
                        },
                        &policy,
                    )?);
                    ready.send(()).map_err(|_| Failure::Unavailable)?;
                    resume
                        .recv_timeout(Duration::from_secs(5))
                        .map_err(|_| Failure::Unavailable)?;
                    store.close_kubernetes_session_control_before_effect(
                        auth, &cap, &request, &policy,
                    )
                },
            )
            .await
        }));
    }
    drop(ready);
    tokio::task::spawn_blocking(move || {
        waiting.recv_timeout(Duration::from_secs(5))?;
        waiting.recv_timeout(Duration::from_secs(5))
    })
    .await
    .unwrap()
    .unwrap();
    for release in releases {
        release.send(()).unwrap();
    }
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap());
    }
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let closed = results.into_iter().find_map(Result::ok).unwrap();
    assert!(!closed.replayed);
    let policy = s.policy.clone();
    let input = s.intents[0].clone();
    with_authenticated_control_recovery_for_test(&s.state,headers(RUNNER),move|auth,store| {
        assert_eq!(store.worker_quota(&QuotaPolicyV1::default())?.held_workers,0);
        let head=store.head()?;let db=store.db.lock().unwrap();
        let count:i64=db.query_row("SELECT count(*) FROM events WHERE kind='control.kubernetes.session.closed_before_effect'",[],|row|row.get(0))?;
        assert_eq!(count,1);let (snapshot,_)=replay::load_snapshot(&db,input.reservation_id(),"tenant-a")?.unwrap();drop(db);
        let request=snapshot.state.last_close.unwrap();
        let KubernetesSessionControlRecoveryAuthorizationV2::ReceiptLookup(cap)=store.authorize_kubernetes_session_control_recovery(auth,&KubernetesSessionControlRecoveryTargetV2::CloseBeforeEffect {close:request},&policy)? else {panic!("closed exact retry is lookup only")};
        let looked_up=store.lookup_kubernetes_session_control_recovery_receipt(auth,&cap,&policy)?;
        assert_eq!(looked_up.receipt,closed.receipt);assert!(looked_up.replayed);
        unchanged(&store,&head,0);Ok(())
    }).await.unwrap();
}

#[tokio::test]
async fn broker_close_invalidates_competing_runner_close_and_restart_only_replays_receipt() {
    let Setup {
        directory,
        state,
        policy,
        intents,
        broker,
    } = setup(1, false, None);
    let intent = &intents[0];
    let broker_close = close(intent);
    let runner_close = close(intent);
    let winning_close = broker_close.clone();
    let active_policy = policy.clone();

    let (receipt, head) = with_authenticated_control_recovery_for_test(
        &state,
        headers(RUNNER),
        move |auth, store| {
            let head = store.head()?;
            unchanged(&store, &head, 1);
            let broker_cap = store.authorize_kubernetes_session_reconciliation(
                &broker,
                broker_close.reservation_id.as_str(),
                Some(&broker_close),
            )?;
            let observation = store.authorize_kubernetes_session_reconciliation(
                &broker,
                broker_close.reservation_id.as_str(),
                None,
            )?;
            let confirmed = store.confirm_kubernetes_session_before_effect(
                &broker,
                &observation,
                broker_close.reservation_id.as_str(),
            )?;
            assert_eq!(confirmed.audit_seq(), head);
            let runner_cap = before_effect(store.authorize_kubernetes_session_control_recovery(
                auth,
                &KubernetesSessionControlRecoveryTargetV2::CloseBeforeEffect {
                    close: runner_close.clone(),
                },
                &active_policy,
            )?);

            // Both capabilities were obtained at revision 1. Order the commits
            // explicitly so the losing path must recheck the durable state.
            let committed = store.close_kubernetes_session_before_effect(
                &broker,
                &broker_cap,
                &broker_close,
            )?;
            assert!(!committed.replayed);
            assert_eq!(committed.receipt.operation_id, broker_close.operation_id);
            assert_eq!(committed.receipt.audit_seq, head + 1);
            let error = store
                .close_kubernetes_session_control_before_effect(
                    auth,
                    &runner_cap,
                    &runner_close,
                    &active_policy,
                )
                .unwrap_err();
            assert_eq!(error.downcast_ref::<Failure>(), Some(&Failure::NotFound));
            let confirmation = store.confirm_kubernetes_session_before_effect(
                &broker,
                &observation,
                broker_close.reservation_id.as_str(),
            );
            assert!(matches!(confirmation, Err(ref error)
                if error.downcast_ref::<Failure>() == Some(&Failure::PermissionDenied)));
            unchanged(&store, &(head + 1), 0);
            Ok((committed.receipt, head + 1))
        },
    )
    .await
    .unwrap();
    drop(state);

    let reopened = Store::open_existing_for_tenant(
        &directory.path().join("tenant.sqlite"),
        "tenant-a",
        Redactor::new(vec![]),
        10_000,
    )
    .unwrap();
    assert!(!reopened.supervision_state().unwrap().active);
    let state = GatewayState::new(
        Arc::new(reopened),
        ROTATED_RUNNER.into(),
        ROTATED_VIEWER.into(),
        crate::policy::Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .with_kubernetes_session_control_recovery_policies(vec![("tenant-a".into(), policy.clone())])
    .unwrap();
    with_authenticated_control_recovery_for_test(
        &state,
        headers(ROTATED_RUNNER),
        move |auth, store| {
            let exact = KubernetesSessionControlRecoveryTargetV2::CloseBeforeEffect {
                close: winning_close,
            };
            let KubernetesSessionControlRecoveryAuthorizationV2::ReceiptLookup(lookup) =
                store.authorize_kubernetes_session_control_recovery(auth, &exact, &policy)?
            else {
                anyhow::bail!("closed reservation must yield receipt lookup only");
            };
            let replay =
                store.lookup_kubernetes_session_control_recovery_receipt(auth, &lookup, &policy)?;
            assert!(replay.replayed);
            assert_eq!(replay.receipt, receipt);
            assert_eq!(
                replay.snapshot.state.state,
                KubernetesSessionReservationStateV2::ClosedBeforeEffect
            );
            assert_eq!(replay.snapshot.state.revision, 2);
            assert!(!replay.snapshot.state.quota_held);
            assert_eq!(
                replay.snapshot.state.effect_fence,
                KubernetesSessionEffectFenceV2::NeverIssued { revision: 0 }
            );
            unchanged(&store, &head, 0);
            Ok(())
        },
    )
    .await
    .unwrap();
}

#[test]
fn control_decision_preimages_match_independent_r3_sha256_goldens() {
    // Literal domain+NUL+canonical-array oracles were hashed with system
    // shasum, independently of the Rust commitment helper. These values are
    // descriptive fixtures, never Auth/Usage/Store authority constructors.
    let actor: ControlActor = serde_json::from_value(json!({"kind":"control_shared_runner","tenant_id":"tenant-a",
        "mechanism":"legacy_runner_bearer","role":"runner","auth_origin":"observed_gateway_verification",
        "auth_epoch_id":"00000000-0000-0000-0000-000000000005","request_auth_id":"00000000-0000-0000-0000-000000000006",
        "authenticated_at":"2026-11-08T22:00:00.000Z"})).unwrap();
    let creator: Creator = serde_json::from_value(json!({"kind":"reservation","reservation_seq":10,"reservation_hash":"a".repeat(64),
        "actor":{"tenant_id":"tenant-a","broker_id":"00000000-0000-0000-0000-000000000002",
            "incarnation_id":"00000000-0000-0000-0000-000000000003","credential_id":"00000000-0000-0000-0000-000000000004",
            "binding_revision":1,"grant_revision":1}})).unwrap();
    let policy = KubernetesSessionControlRecoveryPolicyV2 {
        version: 2,
        decision_ttl_seconds: 60,
        allowed_broker_ids: vec!["00000000-0000-0000-0000-000000000002".into()],
    };
    assert_eq!(
        policy.policy_hash().unwrap(),
        "0e5900377f4b0cee3eee23d872b9161f2f395da0f41641266175cedfcc242055"
    );
    let mut evidence = ControlEvidence {
        version: 2,
        scope: ControlScope::ControlRecoverBeforeEffect,
        actor,
        control_namespace_uuid: "00000000-0000-0000-0000-000000000001".into(),
        creator_reference: creator,
        reservation_seq: 10,
        reservation_hash: "a".repeat(64),
        reservation_input_hash: "b".repeat(64),
        session_hash: "c".repeat(64),
        attempt_hash: "d".repeat(64),
        profile_hash: "e".repeat(64),
        origin_seq: 11,
        origin_hash: "f".repeat(64),
        state: KubernetesSessionReservationStateV2::Reserved,
        revision: 1,
        effect_fence: KubernetesSessionEffectFenceV2::NeverIssued { revision: 0 },
        action: ControlAction::CloseReserved,
        target_hash: "1".repeat(64),
        policy_hash: policy.policy_hash().unwrap(),
        policy_snapshot: policy,
        issued_at: "2026-11-08T22:00:00.001Z".into(),
        expires_at: "2026-11-08T22:01:00.000Z".into(),
        close_seq: None,
        close_hash: None,
        decision_hash: String::new(),
    };
    assert_eq!(
        evidence.commitment().unwrap(),
        "988ca601c683ee991e0c023a0b492233473ead50da05fbce1382a339ace12852"
    );
    evidence.scope = ControlScope::ControlRecoveryReceiptLookup;
    evidence.action = ControlAction::LookupCloseReceipt;
    evidence.state = KubernetesSessionReservationStateV2::ClosedBeforeEffect;
    evidence.revision = 2;
    evidence.close_seq = Some(12);
    evidence.close_hash = Some("2".repeat(64));
    evidence.origin_seq = 13;
    evidence.origin_hash = "3".repeat(64);
    assert_eq!(
        evidence.commitment().unwrap(),
        "29685b43dbbaf719d845c1120dcc72f26d7a72ae63b7ccebc3fd639ea75cbec4"
    );
}
