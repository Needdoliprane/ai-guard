//! Real extractor/blocking regressions; no authority is fabricated by a DTO.
//! Prepared before implementation. Execution requires Root's composed S2 GO.
use super::*;
use crate::api::{Policy, Redactor, TenantGateway};
use axum::{
    body::Body,
    http::{Request, header},
};
use std::{
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicBool, Ordering},
};

const RUNNER_A: &str = "fixture-control-recovery-tenant-a-runner-20261108";
const VIEWER_A: &str = "fixture-control-recovery-tenant-a-viewer-20261108";
const RUNNER_B: &str = "fixture-control-recovery-tenant-b-runner-20261108";
const VIEWER_B: &str = "fixture-control-recovery-tenant-b-viewer-20261108";
const ROTATED_RUNNER_A: &str = "fixture-control-recovery-tenant-a-runner-rotated-20261108";
const ROTATED_VIEWER_A: &str = "fixture-control-recovery-tenant-a-viewer-rotated-20261108";

fn fixture() -> (tempfile::TempDir, GatewayState) {
    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = Arc::new(
        Store::open_for_tenant(
            &directory.path().join("tenant.sqlite"),
            "tenant-a",
            Redactor::new(vec![]),
            1000,
        )
        .unwrap(),
    );
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "tenant-a".into(),
            store,
            runner_token: "fixture-control-recovery-runner".repeat(2),
            viewer_token: "fixture-control-recovery-viewer".repeat(2),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    (directory, state)
}

async fn extract(state: &GatewayState) -> VerifiedControlRecoveryRunnerV2 {
    let (mut parts, _) = Request::builder()
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", "fixture-control-recovery-runner".repeat(2)),
        )
        .body(Body::empty())
        .unwrap()
        .into_parts();
    VerifiedControlRecoveryRunnerV2::from_request_parts(&mut parts, state)
        .await
        .unwrap()
}

#[tokio::test]
async fn real_runner_usage_rejects_a_different_store_even_for_the_same_tenant() {
    // Dropping the exact Store pointer check would admit this independent DB.
    let (directory, state) = fixture();
    let wrong_store = Store::open_for_tenant(
        &directory.path().join("other.sqlite"),
        "tenant-a",
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    extract(&state)
        .await
        .blocking(move |auth, tenant| {
            assert!(auth.acquire_usage(&wrong_store).is_err());
            let _usage = auth.acquire_usage(&tenant.store).unwrap();
            assert_eq!(auth.tenant_id(), "tenant-a");
            assert!(!auth.auth_epoch_id().is_nil());
            assert!(!auth.request_auth_id().is_nil());
            assert_eq!(auth.authenticated_at().len(), 24);
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn real_proof_expires_on_either_utc_or_monotone_clock() {
    // Checking one clock alone would admit one of the expired cases.
    let (_directory, state) = fixture();
    extract(&state)
        .await
        .blocking(|auth, _tenant| {
            assert!(
                auth.lifetime_at(auth.authenticated_utc, auth.created)
                    .is_ok()
            );
            assert!(auth.lifetime_at(auth.expires_utc, auth.created).is_err());
            assert!(
                auth.lifetime_at(auth.authenticated_utc, auth.deadline)
                    .is_err()
            );
            assert!(
                auth.lifetime_at(
                    auth.authenticated_utc - chrono::Duration::milliseconds(1),
                    auth.created,
                )
                .is_err()
            );
            assert!(auth.monotone_deadline() <= auth.created + Duration::from_secs(60));
            Ok(())
        })
        .await
        .unwrap();
}

fn binding_schema(directory: &tempfile::TempDir, state: &GatewayState) {
    let path = directory.path().join("tenant.sqlite");
    let store = &state.tenants[0].store;
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
}

fn collide_with_the_real_runner(store: &Store) {
    use sha2::{Digest, Sha256};
    store
        .issue_workload_binding(&crate::workload::WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: Uuid::new_v4().to_string(),
            expected_revision: 0,
            subject: crate::workload::WorkloadSubjectV1::Broker {
                broker_id: Uuid::new_v4().to_string(),
                incarnation_id: Uuid::new_v4().to_string(),
            },
            sha256_digest: hex::encode(Sha256::digest(
                "fixture-control-recovery-runner".repeat(2).as_bytes(),
            )),
            expires_at: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
}

#[tokio::test]
async fn real_collision_after_extraction_is_denied_before_the_blocking_operation() {
    let (directory, state) = fixture();
    binding_schema(&directory, &state);
    let runner = extract(&state).await;
    collide_with_the_real_runner(&state.tenants[0].store);
    let head = state.tenants[0].store.head().unwrap();
    let called = Arc::new(AtomicBool::new(false));
    let reached = Arc::clone(&called);
    assert!(
        runner
            .blocking(move |_, _| {
                reached.store(true, Ordering::SeqCst);
                Ok(())
            })
            .await
            .is_err()
    );
    assert!(!called.load(Ordering::SeqCst));
    assert_eq!(state.tenants[0].store.head().unwrap(), head);
}

#[tokio::test]
async fn every_usage_reauthenticates_a_real_new_collision() {
    // A one-time factory check would admit the second usage incorrectly.
    let (directory, state) = fixture();
    binding_schema(&directory, &state);
    extract(&state)
        .await
        .blocking(|auth, tenant| {
            drop(auth.acquire_usage(&tenant.store).unwrap());
            collide_with_the_real_runner(&tenant.store);
            let head = tenant.store.head().unwrap();
            assert!(auth.acquire_usage(&tenant.store).is_err());
            assert_eq!(tenant.store.head().unwrap(), head);
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn two_real_requests_to_the_same_store_retain_distinct_auth_contexts() {
    let (_directory, state) = fixture();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", "fixture-control-recovery-runner".repeat(2))
            .parse()
            .unwrap(),
    );
    with_control_recovery_auth_pair_for_test(&state, headers, |first, second, store| {
        assert!(!std::ptr::eq(first, second));
        assert_ne!(first.request_auth_id(), second.request_auth_id());
        assert_eq!(first.auth_epoch_id(), second.auth_epoch_id());
        let _first_usage = first.acquire_usage(&store)?;
        let _second_usage = second.acquire_usage(&store)?;
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn retiring_a_clone_denies_unused_proofs_but_drains_live_usage() {
    // Retiring an unrelated epoch or reporting drained while leased breaks this.
    let (_directory, state) = fixture();
    let retirement = state.clone().control_recovery_retirement();
    extract(&state)
        .await
        .blocking(move |auth, tenant| {
            let usage = auth.acquire_usage(&tenant.store).unwrap();
            retirement.retire();
            assert!(!retirement.drained());
            assert!(auth.acquire_usage(&tenant.store).is_err());
            drop(usage);
            assert!(retirement.drained());
            retirement.retire();
            assert!(retirement.drained());
            Ok(())
        })
        .await
        .unwrap();
    let reached = Arc::new(AtomicBool::new(false));
    let called = Arc::clone(&reached);
    assert!(
        extract(&state)
            .await
            .blocking(move |_, _| {
                called.store(true, Ordering::SeqCst);
                Ok(())
            })
            .await
            .is_err()
    );
    assert!(!reached.load(Ordering::SeqCst));
}

#[tokio::test]
async fn retirement_handle_drop_closes_only_its_shared_epoch() {
    let (_directory, state) = fixture();
    let (_other_directory, other) = fixture();
    let handle = state.control_recovery_retirement();
    let clone_handle = state.clone().control_recovery_retirement();
    drop(handle);
    assert!(clone_handle.drained());
    assert!(extract(&state).await.blocking(|_, _| Ok(())).await.is_err());
    extract(&other)
        .await
        .blocking(|auth, tenant| {
            let _usage = auth.acquire_usage(&tenant.store).unwrap();
            Ok(())
        })
        .await
        .unwrap();
}

#[test]
fn queue_retirement_is_rechecked_before_the_operation_runs() {
    // Extractor success alone must not authorize work queued before shutdown.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (_directory, state) = fixture();
        let runner = extract(&state).await;
        let (occupied, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            occupied.send(()).unwrap();
            wait.recv().unwrap();
        });
        ready.await.unwrap();
        let called = Arc::new(AtomicBool::new(false));
        let reached = Arc::clone(&called);
        let retirement = state.control_recovery_retirement();
        let (queued, seen_queue) = tokio::sync::oneshot::channel();
        let job = tokio::spawn(async move {
            let future = runner.blocking(move |_, _| {
                reached.store(true, Ordering::SeqCst);
                Ok(())
            });
            tokio::pin!(future);
            assert!(futures_util::poll!(&mut future).is_pending());
            queued.send(()).unwrap();
            future.await
        });
        seen_queue.await.unwrap();
        retirement.retire();
        release.send(()).unwrap();
        blocker.await.unwrap();
        assert!(job.await.unwrap().is_err());
        assert!(!called.load(Ordering::SeqCst));
        assert!(retirement.drained());
    });
}

#[tokio::test]
async fn usage_reuses_the_admitted_scope_when_the_control_lane_is_full() {
    // Losing Scope after authorize_sync would require an unavailable extra slot.
    let (_directory, state) = fixture();
    let runner = extract(&state).await;
    let tenant_io = Arc::clone(&state.tenants[0].io);
    let mut remaining_slots = Vec::new();
    for _ in 0..127 {
        remaining_slots.push(tenant_io.admit(store_io::Lane::Control, false).unwrap());
    }
    runner
        .blocking(move |auth, tenant| {
            assert!(tenant_io.admit(store_io::Lane::Control, false).is_err());
            let _usage = auth.acquire_usage(&tenant.store).unwrap();
            // Each usage must also keep the same scope after real reauth.
            let _second = auth.acquire_usage(&tenant.store).unwrap();
            drop(remaining_slots);
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn cancelled_waiter_keeps_its_store_lease_until_the_blocking_operation_finishes() {
    let (_directory, state) = fixture();
    let runner = extract(&state).await;
    let mut slots = Vec::new();
    for _ in 0..127 {
        slots.push(
            state.tenants[0]
                .io
                .admit(store_io::Lane::Control, false)
                .unwrap(),
        );
    }
    let (started, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let (finished, finish) = tokio::sync::oneshot::channel();
    let job = tokio::spawn(runner.blocking(move |auth, tenant| {
        let usage = auth.acquire_usage(&tenant.store).unwrap();
        started.send(()).unwrap();
        wait.recv().unwrap();
        drop(usage);
        finished.send(()).unwrap();
        Ok(())
    }));
    ready.await.unwrap();
    job.abort();
    assert!(job.await.unwrap_err().is_cancelled());
    assert!(
        state.tenants[0]
            .io
            .admit(store_io::Lane::Control, false)
            .is_err()
    );
    let retirement = state.control_recovery_retirement();
    retirement.retire();
    assert!(!retirement.drained());
    release.send(()).unwrap();
    finish.await.unwrap();
    assert!(retirement.drained());
    drop(slots);
}

#[tokio::test]
async fn policy_builders_are_closed_canonical_and_share_the_retirement_epoch() {
    use crate::control::kubernetes_session::*;
    let (_directory, state) = fixture();
    assert!(state.tenants[0].session_reservation_policy.is_none());
    assert!(state.tenants[0].session_recovery_policy.is_none());
    assert!(state.tenants[0].session_control_recovery_policy.is_none());
    let policy = KubernetesSessionControlRecoveryPolicyV2 {
        version: 2,
        decision_ttl_seconds: 60,
        allowed_broker_ids: vec![
            "ffffffff-ffff-4fff-8fff-ffffffffffff".into(),
            "11111111-1111-4111-8111-111111111111".into(),
        ],
    };
    assert!(
        state
            .clone()
            .with_kubernetes_session_control_recovery_policies(vec![(
                "unknown".into(),
                policy.clone()
            )])
            .is_err()
    );
    assert!(
        state
            .clone()
            .with_kubernetes_session_control_recovery_policies(vec![
                ("tenant-a".into(), policy.clone()),
                ("tenant-a".into(), policy.clone())
            ])
            .is_err()
    );
    let mut invalid = policy.clone();
    invalid.decision_ttl_seconds = 61;
    assert!(
        state
            .clone()
            .with_kubernetes_session_control_recovery_policies(vec![("tenant-a".into(), invalid)])
            .is_err()
    );
    let built = state
        .clone()
        .with_kubernetes_session_reservation_policies(vec![(
            "tenant-a".into(),
            KubernetesSessionReservationPolicyV2 {
                version: 2,
                decision_ttl_seconds: 60,
                approvals: vec![],
            },
        )])
        .unwrap()
        .with_kubernetes_session_recovery_policies(vec![(
            "tenant-a".into(),
            KubernetesSessionRecoveryPolicyV2 {
                version: 2,
                decision_ttl_seconds: 60,
                allowed_broker_ids: vec![],
            },
        )])
        .unwrap()
        .with_kubernetes_session_control_recovery_policies(vec![("tenant-a".into(), policy)])
        .unwrap();
    assert_eq!(
        built.tenants[0]
            .session_control_recovery_policy
            .as_ref()
            .unwrap()
            .allowed_broker_ids,
        [
            "11111111-1111-4111-8111-111111111111",
            "ffffffff-ffff-4fff-8fff-ffffffffffff"
        ],
    );
    let epoch_before = extract(&state)
        .await
        .blocking(|auth, _| Ok(auth.auth_epoch_id()))
        .await
        .unwrap();
    let epoch_after = extract(&built)
        .await
        .blocking(|auth, _| Ok(auth.auth_epoch_id()))
        .await
        .unwrap();
    assert_eq!(epoch_before, epoch_after);
    let retirement = built.control_recovery_retirement();
    retirement.retire();
    assert!(extract(&state).await.blocking(|_, _| Ok(())).await.is_err());
}

fn session_intent(
    fixture: &crate::audit::kubernetes_admission_state_tests::Fixture,
) -> crate::control::kubernetes_session::KubernetesSessionReservationIntentV2 {
    use crate::{provider::*, runtime::kubernetes::session_profile::*};
    let authority = &fixture.intent.authority;
    let binding = KubernetesSessionBindingV2::new(
        authority,
        KubernetesSessionIdsV2 {
            installation_id: Uuid::new_v4(),
            control_namespace_uuid: fixture.intent.control_namespace_uuid.parse().unwrap(),
            broker_id: fixture.intent.broker_id.parse().unwrap(),
            incarnation_id: fixture.intent.incarnation_id.parse().unwrap(),
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
        credential_names: vec![BOOTSTRAP_ENV.into()],
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
        "public synthetic session prompt",
    ]
    .map(str::to_owned);
    let profile = KubernetesSessionProfileV2::build(
        authority,
        &binding,
        &fixture.intent.image,
        KubernetesSessionResourcesV2 {
            cpu_millis: 500,
            memory_mib: 128,
            temporary_mib: 8,
            home_mib: 8,
            deadline_seconds: 60,
        },
        &provider,
        KubernetesSessionInvocationV2::new(BOOTSTRAP_PROGRAM, &args).unwrap(),
        &crate::workspace::Manifest::new(),
    )
    .unwrap();
    crate::control::kubernetes_session::KubernetesSessionReservationIntentV2::new(
        &Uuid::new_v4().to_string(),
        &Uuid::new_v4().to_string(),
        authority,
        &profile,
    )
    .unwrap()
}

struct RecoveryHttp {
    endpoint: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    server: Option<tokio::task::JoinHandle<()>>,
    retirement: Arc<GatewayControlRecoveryRetirementV2>,
}

impl RecoveryHttp {
    async fn start(state: GatewayState) -> Self {
        let retirement = Arc::new(state.control_recovery_retirement());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (shutdown, stopped) = tokio::sync::oneshot::channel();
        let owner = Arc::clone(&retirement);
        let after = Arc::clone(&retirement);
        let server = tokio::spawn(async move {
            let result = axum::serve(listener, crate::api::gateway_router(state))
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                    owner.retire();
                })
                .await;
            after.retire();
            result.unwrap();
        });
        Self {
            endpoint,
            shutdown: Some(shutdown),
            server: Some(server),
            retirement,
        }
    }

    async fn stop(mut self) {
        self.retirement.retire();
        self.shutdown.take().unwrap().send(()).unwrap();
        self.server.take().unwrap().await.unwrap();
        assert!(self.retirement.drained());
    }
}

impl Drop for RecoveryHttp {
    fn drop(&mut self) {
        self.retirement.retire();
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

async fn recovery_post(
    server: &RecoveryHttp,
    id: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
        .post(format!(
            "{}/api/v1/kubernetes/session-reservations/{id}/control-recovery",
            server.endpoint
        ))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.bytes().await.unwrap();
    assert!(bytes.len() <= 131_072);
    assert!(!String::from_utf8_lossy(&bytes).contains(token));
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn route_recovers_after_all_broker_authority_is_revoked_and_store_reopens_inactive() {
    // Requiring a live broker, charging control write, or reattributing a retry
    // would break this real reservation/revoke/reopen/HTTP lifecycle.
    use crate::{
        audit::{
            EventQuery,
            read_test_observer::{Call, observe},
        },
        control::{QuotaPolicyV1, kubernetes_session::*},
        provider::*,
    };
    use serde_json::json;
    let fixture = crate::audit::kubernetes_admission_state_tests::Fixture::new();
    let path = fixture._directory.path().join("tenant.sqlite");
    fixture
        .store
        .migrate_kubernetes_policy_v9(&path.with_extension("before9"))
        .unwrap();
    fixture
        .store
        .migrate_human_workers_v10(&path.with_extension("before10"))
        .unwrap();
    fixture
        .store
        .migrate_kubernetes_session_reservations_v11(&path.with_extension("before11"))
        .unwrap();
    let input = session_intent(&fixture);
    let reservation_policy = KubernetesSessionReservationPolicyV2 {
        version: 2,
        decision_ttl_seconds: 60,
        approvals: vec![KubernetesSessionReservationApprovalV2 {
            authority_id: input.authority().authority_id().to_string(),
            authority_fingerprint: input.authority().fingerprint(),
            image: fixture.intent.image.clone(),
            client: ClientKind::Codex,
            auth_mode: AuthMode::Api,
            transport: ClientTransport::Batch,
            profile_id: "default".into(),
            profile_revision: 1,
        }],
    };
    let quota = QuotaPolicyV1::default();
    let authorization = fixture
        .store
        .authorize_kubernetes_session_reservation(
            &fixture.identity,
            &input,
            &reservation_policy,
            &quota,
        )
        .unwrap();
    let reserved = fixture
        .store
        .reserve_kubernetes_session(
            &fixture.identity,
            &authorization,
            &input,
            &reservation_policy,
            &quota,
        )
        .unwrap();
    assert_eq!(fixture.store.worker_quota(&quota).unwrap().held_workers, 1);
    fixture
        .store
        .revoke_broker(&crate::broker_registry::BrokerRevocationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: fixture.identity.broker_id().into(),
            expected_revision: fixture.identity.grant_revision(),
        })
        .unwrap();
    fixture
        .store
        .revoke_workload_binding(&crate::workload::WorkloadBindingRevocationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: fixture.identity.credential_id().into(),
            expected_revision: fixture.identity.binding_revision(),
        })
        .unwrap();
    let revoked_error = match fixture
        .store
        .authenticate_workload_token("public-held-create-test-broker-token-0000000000000000")
    {
        Err(error) => error,
        Ok(_) => panic!("revoked workload binding must return the typed Unauthorized error"),
    };
    assert_eq!(
        revoked_error.downcast_ref::<crate::workload::WorkloadFailureV1>(),
        Some(&crate::workload::WorkloadFailureV1::Unauthorized)
    );
    let historical = fixture
        .store
        .broker_for_authority(&fixture.intent.authority.authority_id().to_string())
        .unwrap()
        .unwrap();
    assert_eq!(
        historical.state,
        crate::broker_registry::BrokerStateV1::Revoked
    );
    let broker_id = fixture.identity.broker_id().to_owned();
    drop(authorization);
    drop(fixture.store);
    let reopened = Arc::new(
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 10_000).unwrap(),
    );
    assert!(!reopened.supervision_state().unwrap().active);
    let other_path = fixture._directory.path().join("tenant-b.sqlite");
    let other = Arc::new(
        Store::open_for_tenant(&other_path, "tenant-b", Redactor::new(vec![]), 10_000).unwrap(),
    );
    let mut tenants = vec![
        TenantGateway {
            tenant_id: "tenant-a".into(),
            store: reopened,
            runner_token: RUNNER_A.into(),
            viewer_token: VIEWER_A.into(),
        },
        TenantGateway {
            tenant_id: "tenant-b".into(),
            store: other,
            runner_token: RUNNER_B.into(),
            viewer_token: VIEWER_B.into(),
        },
    ];
    // B needs its own real schema11 Store. This never copies A's reservation.
    let b = &tenants[1].store;
    b.migrate_control_v3(&other_path.with_extension("before3"))
        .unwrap();
    b.migrate_identity_v4(&other_path.with_extension("before4"))
        .unwrap();
    b.migrate_human_access_v5(&other_path.with_extension("before5"))
        .unwrap();
    b.migrate_workload_v6(&other_path.with_extension("before6"))
        .unwrap();
    b.migrate_human_brokers_v7(&other_path.with_extension("before7"))
        .unwrap();
    b.migrate_kubernetes_create_v8(&other_path.with_extension("before8"))
        .unwrap();
    b.migrate_kubernetes_policy_v9(&other_path.with_extension("before9"))
        .unwrap();
    b.migrate_human_workers_v10(&other_path.with_extension("before10"))
        .unwrap();
    b.migrate_kubernetes_session_reservations_v11(&other_path.with_extension("before11"))
        .unwrap();
    let policy = KubernetesSessionControlRecoveryPolicyV2 {
        version: 2,
        decision_ttl_seconds: 60,
        allowed_broker_ids: vec![broker_id.clone()],
    };
    let state = GatewayState::for_tenants(
        std::mem::take(&mut tenants),
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap()
    .with_kubernetes_session_control_recovery_policies(vec![
        ("tenant-a".into(), policy.clone()),
        ("tenant-b".into(), policy),
    ])
    .unwrap();
    let a = Arc::clone(&state.tenants[0].store);
    let b = Arc::clone(&state.tenants[1].store);
    let app = RecoveryHttp::start(state.clone()).await;
    let observe_request = json!({"version":2,"action":"observe_reserved",
        "reservation_input_hash":reserved.snapshot.input_hash});
    let head = a.head().unwrap();
    let observations = observe(&a);
    let (status, rejected) = recovery_post(
        &app,
        input.reservation_id(),
        RUNNER_B,
        observe_request.clone(),
    )
    .await;
    assert!(matches!(
        status,
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
    ));
    assert!(rejected.get("snapshot").is_none());
    assert_eq!(a.head().unwrap(), head);
    assert_eq!(
        observations
            .try_iter()
            .filter(|sample| sample.call == Call::BeginChecked)
            .count(),
        0,
        "Runner B must not open A's reservation transaction; collision metadata scans are separate"
    );
    let (status, current) =
        recovery_post(&app, input.reservation_id(), RUNNER_A, observe_request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(current["snapshot"]["state"]["state"], "reserved");
    assert_eq!(a.head().unwrap(), head);
    let close = KubernetesSessionReservationCloseV2 {
        version: 2,
        operation_id: Uuid::new_v4().to_string(),
        reservation_id: input.reservation_id().into(),
        reservation_input_hash: input.input_hash("tenant-a").unwrap(),
        expected_revision: 1,
        reason: KubernetesSessionReservationCloseReasonV2::OperatorCancelled,
    };
    let close_request = json!({"version":2,"action":"close_before_effect","close":close});
    let (status, closed) = recovery_post(
        &app,
        input.reservation_id(),
        RUNNER_A,
        close_request.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(closed["snapshot"]["state"]["state"], "closed_before_effect");
    assert_eq!(closed["replayed"], false);
    assert_eq!(a.worker_quota(&quota).unwrap().held_workers, 0);
    let head = a.head().unwrap();
    let records = a
        .events(&EventQuery {
            kind: Some("control.kubernetes.session.closed_before_effect".into()),
            order: Some("asc".into()),
            limit: Some(10),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].event.data["actor"]["kind"],
        "control_shared_runner"
    );
    assert_eq!(
        records[0].event.data["creator"]["actor"]["broker_id"],
        broker_id
    );
    let original_actor = records[0].event.data["actor"].clone();
    let (status, retry) =
        recovery_post(&app, input.reservation_id(), RUNNER_A, close_request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(retry["receipt"], closed["receipt"]);
    assert_eq!(retry["replayed"], true);
    assert_eq!(a.head().unwrap(), head);
    let lookup = json!({"version":2,"action":"lookup_close_receipt",
        "reservation_input_hash":close.reservation_input_hash,"close_input_hash":close.input_hash().unwrap(),
        "operation_id":close.operation_id});
    let retirement = state.control_recovery_retirement();
    retirement.retire();
    assert!(retirement.drained());
    let fresh = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "tenant-a".into(),
            store: a.clone(),
            runner_token: ROTATED_RUNNER_A.into(),
            viewer_token: ROTATED_VIEWER_A.into(),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap()
    .with_kubernetes_session_control_recovery_policies(vec![(
        "tenant-a".into(),
        KubernetesSessionControlRecoveryPolicyV2 {
            version: 2,
            decision_ttl_seconds: 60,
            allowed_broker_ids: vec![broker_id],
        },
    )])
    .unwrap();
    let fresh_app = RecoveryHttp::start(fresh).await;
    assert_eq!(
        recovery_post(&fresh_app, input.reservation_id(), RUNNER_A, lookup.clone())
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, retry) =
        recovery_post(&fresh_app, input.reservation_id(), ROTATED_RUNNER_A, lookup).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(retry["receipt"], closed["receipt"]);
    assert_eq!(a.head().unwrap(), head);
    let after = a
        .events(&EventQuery {
            kind: Some("control.kubernetes.session.closed_before_effect".into()),
            order: Some("asc".into()),
            limit: Some(10),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].event.data["actor"], original_actor);
    assert_eq!(b.worker_quota(&quota).unwrap().held_workers, 0);
    assert!(a.integrity().unwrap().valid);
    app.stop().await;
    fresh_app.stop().await;
}
