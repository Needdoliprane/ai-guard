//! Authenticated broker scope over the real gateway and tenant SQLite stores.
//! Registry grants are declarations; no runtime or worker transport is exercised.
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    broker_registry::{
        BrokerAuthorityV1, BrokerCheckpointSnapshotV1, BrokerCheckpointV1, BrokerGrantMutationV1,
        BrokerJobCheckpointV1, BrokerRevocationV1, BrokerRuntimeKindV1,
    },
    control::*,
    model::{CommandSpec, DecisionRequest},
    policy::{self, Policy},
    redaction::Redactor,
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path, sync::Arc};
use tower::ServiceExt;
use uuid::Uuid;

const BROKER: &str = "22222222-2222-4222-8222-222222222222";
const INCARNATION: &str = "33333333-3333-4333-8333-333333333333";
const CREDENTIAL: &str = "44444444-4444-4444-8444-444444444444";
const BROKER_A: &str = "public-broker-identity-fixture-a-000000000000000000";
const BROKER_B: &str = "public-broker-identity-fixture-b-000000000000000000";
const WORKER: &str = "public-broker-identity-fixture-worker-0000000000000";
const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

fn checkpoint(hash: &str) -> BrokerCheckpointV1 {
    BrokerCheckpointV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        incarnation_id: INCARNATION.into(),
        previous_sequence: 0,
        sequence: 1,
        snapshot: BrokerCheckpointSnapshotV1 {
            version: 1,
            authority: BrokerAuthorityV1 {
                authority_id: "declared-software-authority".into(),
                runtime_kind: BrokerRuntimeKindV1::Docker,
            },
            jobs: vec![BrokerJobCheckpointV1 {
                job_id: Uuid::new_v4().to_string(),
                revision: 1,
                checkpoint_hash: hash.into(),
            }],
            has_more: false,
        },
    }
}

#[tokio::test]
async fn checkpoint_http_refuses_the_presented_proof_in_valid_metadata_and_retained_output() {
    let directory = support::private_tempdir();
    let a = store(directory.path(), "tenant-a", 12);
    let b = store(directory.path(), "tenant-b", 12);
    // A synthetic current proof can also have the syntax of a valid SHA256
    // field. Its format does not make it public metadata.
    let proof = hex::encode(Sha256::digest(b"public checkpoint metadata proof fixture"));
    bind_broker(&a, &proof);
    bind_broker(&b, BROKER_B);
    let app = router(a.clone(), b);
    let input = checkpoint(&proof);
    let head = a.head().unwrap();
    let result = checkpoint_post(
        &app,
        "/api/v1/brokers/checkpoints",
        &proof,
        serde_json::to_vec(&input).unwrap(),
    )
    .await;
    assert_eq!(
        result.0,
        StatusCode::BAD_REQUEST,
        "presented proof must be rejected before checkpoint persistence"
    );
    assert_eq!(a.head().unwrap(), head);
}

#[tokio::test]
async fn checkpoint_http_withholds_retained_output_after_a_fresh_binding_and_incarnation() {
    let directory = support::private_tempdir();
    let a = store(directory.path(), "tenant-a", 12);
    let b = store(directory.path(), "tenant-b", 12);
    bind_broker(&a, BROKER_A);
    bind_broker(&b, BROKER_B);
    let app = router(a.clone(), b);
    let proof = hex::encode(Sha256::digest(b"public checkpoint metadata proof fixture"));
    let input = checkpoint(&proof);
    assert_eq!(
        checkpoint_post(
            &app,
            "/api/v1/brokers/checkpoints",
            BROKER_A,
            serde_json::to_vec(&input).unwrap()
        )
        .await
        .0,
        StatusCode::OK
    );
    // A new binding and grant require a fresh incarnation. The old publication
    // remains historical data; it must not echo a proof now known to the API.
    let credential = Uuid::new_v4().to_string();
    let incarnation = Uuid::new_v4().to_string();
    a.issue_workload_binding(&WorkloadBindingIssueV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: credential.clone(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Broker {
            broker_id: BROKER.into(),
            incarnation_id: incarnation.clone(),
        },
        sha256_digest: hex::encode(Sha256::digest(proof.as_bytes())),
        expires_at: "2100-01-01T00:00:00.000Z".into(),
    })
    .unwrap();
    a.mutate_broker_grant(&BrokerGrantMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        broker_id: BROKER.into(),
        expected_revision: Some(1),
        credential_id: credential,
        incarnation_id: incarnation,
        authorities: vec![input.snapshot.authority.clone()],
        lease_seconds: 120,
        not_after: "2100-01-01T00:00:00.000Z".into(),
    })
    .unwrap();
    let retained_head = a.head().unwrap();
    let (status, value) = get(&app, "/api/v1/brokers/checkpoints/self", &proof).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!serde_json::to_string(&value).unwrap().contains(&proof));
    assert_eq!(a.head().unwrap(), retained_head);
}

async fn checkpoint_post(
    app: &Router,
    path: &str,
    token: &str,
    bytes: Vec<u8>,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(bytes))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(bytes.len() <= 32_768);
    for secret in [BROKER_A, BROKER_B, WORKER] {
        assert!(
            !bytes
                .windows(secret.len())
                .any(|part| part == secret.as_bytes())
        );
    }
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn checkpoint_http_uses_verified_tenant_and_replays_only_the_exact_operation() {
    let directory = support::private_tempdir();
    let a = store(directory.path(), "tenant-a", 12);
    let b = store(directory.path(), "tenant-b", 12);
    bind_broker(&a, BROKER_A);
    bind_broker(&b, BROKER_B);
    let app = router(a.clone(), b.clone());
    let first = checkpoint(&hex::encode(Sha256::digest(b"public checkpoint A")));
    let other = checkpoint(&hex::encode(Sha256::digest(b"public checkpoint B")));
    let b_head = b.head().unwrap();
    let (status, receipt) = checkpoint_post(
        &app,
        "/control/api/v1/brokers/checkpoints",
        BROKER_A,
        serde_json::to_vec(&first).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(receipt["replayed"], false);
    assert_eq!(receipt["checkpoint"], serde_json::to_value(&first).unwrap());
    assert_eq!(b.head().unwrap(), b_head, "A must not append to B's audit");
    let a_head = a.head().unwrap();
    let replay = checkpoint_post(
        &app,
        "/api/v1/brokers/checkpoints",
        BROKER_A,
        serde_json::to_vec(&first).unwrap(),
    )
    .await;
    assert_eq!(replay.0, StatusCode::OK);
    assert_eq!(replay.1["replayed"], true);
    assert_eq!(replay.1["receipt"], receipt["receipt"]);
    assert_eq!(
        a.head().unwrap(),
        a_head,
        "replay must not duplicate mandatory audit"
    );
    let mut divergent = first.clone();
    divergent.snapshot.jobs[0].checkpoint_hash = HASH_C.into();
    assert_eq!(
        checkpoint_post(
            &app,
            "/api/v1/brokers/checkpoints",
            BROKER_A,
            serde_json::to_vec(&divergent).unwrap()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(a.head().unwrap(), a_head);
    assert_eq!(
        checkpoint_post(
            &app,
            "/api/v1/brokers/checkpoints",
            BROKER_B,
            serde_json::to_vec(&other).unwrap()
        )
        .await
        .0,
        StatusCode::OK
    );
    for (token, input) in [(BROKER_A, &first), (BROKER_B, &other)] {
        let (status, view) = get(&app, "/control/api/v1/brokers/checkpoints/self", token).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            view["checkpoint"]["checkpoint"],
            serde_json::to_value(input).unwrap()
        );
        assert_eq!(view["checkpoint"]["coverage"], "declared_partial");
    }
}

#[tokio::test]
async fn checkpoint_http_refuses_other_roles_scope_fields_oversize_and_revoked_replay() {
    let directory = support::private_tempdir();
    let a = store(directory.path(), "tenant-a", 12);
    let b = store(directory.path(), "tenant-b", 12);
    bind_broker(&a, BROKER_A);
    bind_broker(&b, BROKER_B);
    let app = router(a.clone(), b.clone());
    let input = checkpoint(&hex::encode(Sha256::digest(b"public checkpoint A")));
    let bytes = serde_json::to_vec(&input).unwrap();
    let head = a.head().unwrap();
    for token in [
        "a".repeat(48),
        "v".repeat(48),
        "unknown-public-fixture-proof".into(),
    ] {
        let status = checkpoint_post(
            &app,
            "/control/api/v1/brokers/checkpoints",
            &token,
            bytes.clone(),
        )
        .await
        .0;
        assert!(
            matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
            "wrong role: {status}"
        );
    }
    let mut forged = serde_json::to_value(&input).unwrap();
    forged["tenant_id"] = json!("tenant-b");
    assert_eq!(
        checkpoint_post(
            &app,
            "/api/v1/brokers/checkpoints",
            BROKER_A,
            serde_json::to_vec(&forged).unwrap()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let mut oversized = bytes.clone();
    oversized.resize(32_769, b' ');
    let status = checkpoint_post(
        &app,
        "/control/api/v1/brokers/checkpoints",
        BROKER_A,
        oversized.clone(),
    )
    .await
    .0;
    assert!(matches!(
        status,
        StatusCode::BAD_REQUEST | StatusCode::PAYLOAD_TOO_LARGE
    ));
    assert_eq!(
        checkpoint_post(
            &app,
            "/control/api/v1/brokers/checkpoints",
            "unknown-public-fixture-proof",
            oversized
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED,
        "authenticate before accepting a body"
    );
    assert_eq!(a.head().unwrap(), head);
    assert_eq!(
        checkpoint_post(&app, "/api/v1/brokers/checkpoints", BROKER_A, bytes.clone())
            .await
            .0,
        StatusCode::OK
    );
    a.revoke_broker(&BrokerRevocationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        broker_id: BROKER.into(),
        expected_revision: 1,
    })
    .unwrap();
    let revoked_head = a.head().unwrap();
    assert_eq!(
        checkpoint_post(&app, "/api/v1/brokers/checkpoints", BROKER_A, bytes)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&app, "/api/v1/brokers/checkpoints/self", BROKER_A)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(a.head().unwrap(), revoked_head);
}

fn store(root: &Path, tenant: &str, schema: u32) -> Arc<Store> {
    let path = root.join(format!("{tenant}.sqlite"));
    let store = Store::open_for_tenant(&path, tenant, Redactor::new(vec![]), 10_000).unwrap();
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
    if schema >= 7 {
        store
            .migrate_human_brokers_v7(&path.with_extension("before7"))
            .unwrap();
    }
    if schema == 12 {
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
            .migrate_broker_checkpoints_v12(&path.with_extension("before12"))
            .unwrap();
    }
    Arc::new(store)
}

fn bind_broker(store: &Store, token: &str) {
    store
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: CREDENTIAL.into(),
            expected_revision: 0,
            subject: WorkloadSubjectV1::Broker {
                broker_id: BROKER.into(),
                incarnation_id: INCARNATION.into(),
            },
            sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
            expires_at: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
    store
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: BROKER.into(),
            expected_revision: None,
            credential_id: CREDENTIAL.into(),
            incarnation_id: INCARNATION.into(),
            authorities: vec![BrokerAuthorityV1 {
                authority_id: "declared-software-authority".into(),
                runtime_kind: BrokerRuntimeKindV1::Docker,
            }],
            lease_seconds: 120,
            not_after: "2100-01-01T00:00:00.000Z".into(),
        })
        .unwrap();
}

fn router(a: Arc<Store>, b: Arc<Store>) -> Router {
    gateway_router(
        GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: a,
                    runner_token: "a".repeat(48),
                    viewer_token: "v".repeat(48),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: b,
                    runner_token: "b".repeat(48),
                    viewer_token: "w".repeat(48),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![BROKER_A.into(), BROKER_B.into()]),
        )
        .unwrap(),
    )
}

async fn get(app: &Router, path: &str, token: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(bytes.len() <= 16_384, "bounded broker identity response");
    for secret in [BROKER_A, BROKER_B, WORKER, token] {
        assert!(
            !bytes
                .windows(secret.len())
                .any(|bytes| bytes == secret.as_bytes()),
            "private broker proof must not enter a response"
        );
    }
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn broker_identity_is_derived_from_the_authenticated_broker_scope() {
    let directory = support::private_tempdir();
    let a = store(directory.path(), "tenant-a", 6);
    let b = store(directory.path(), "tenant-b", 7);
    // Deliberately reuse resource IDs across tenants: the presented proof,
    // rather than any caller label or UUID, must select the response scope.
    bind_broker(&a, BROKER_A);
    bind_broker(&b, BROKER_B);
    let app = router(a.clone(), b.clone());
    for (store, tenant, token, schema) in
        [(&a, "tenant-a", BROKER_A, 6), (&b, "tenant-b", BROKER_B, 7)]
    {
        assert_eq!(
            get(&app, "/api/v1/brokers/self", token).await.0,
            StatusCode::OK,
            "the fixture must first authenticate a real broker binding and grant"
        );
        let a_head = a.head().unwrap();
        let b_head = b.head().unwrap();
        let (status, identity) = get(&app, "/api/v1/brokers/identity", token).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "the broker-only identity route must expose the verified scope"
        );
        assert_eq!(
            identity,
            json!({
                "version":1, "tenant_id":tenant,
                "control_namespace_uuid":store.supervision_state().unwrap().control_namespace_uuid.unwrap(),
                "schema_version":schema, "broker_id":BROKER, "incarnation_id":INCARNATION,
                "credential_id":CREDENTIAL, "binding_revision":1, "grant_revision":1
            })
        );
        let fields: BTreeSet<_> = identity
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            fields,
            BTreeSet::from([
                "version",
                "tenant_id",
                "control_namespace_uuid",
                "schema_version",
                "broker_id",
                "incarnation_id",
                "credential_id",
                "binding_revision",
                "grant_revision"
            ])
        );
        assert_eq!(
            a.head().unwrap(),
            a_head,
            "identity reads do not fabricate an audit mutation"
        );
        assert_eq!(b.head().unwrap(), b_head);
    }
}

#[tokio::test]
async fn broker_identity_rejects_control_roles_and_a_real_worker_proof() {
    let directory = support::private_tempdir();
    let a = store(directory.path(), "tenant-a", 6);
    let b = store(directory.path(), "tenant-b", 7);
    bind_broker(&a, BROKER_A);
    bind_broker(&b, BROKER_B);
    // These are real typed authorization/SQLite projections with declared
    // software observations. No worker or container is launched by the test.
    let worker = running_worker(&a);
    a.issue_workload_binding(&WorkloadBindingIssueV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: Uuid::new_v4().to_string(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Worker {
            worker_id: worker.worker_id,
            attempt_id: worker.attempt.attempt_id,
            generation: 1,
        },
        sha256_digest: hex::encode(Sha256::digest(WORKER.as_bytes())),
        expires_at: "2100-01-01T00:00:00.000Z".into(),
    })
    .unwrap();
    let app = router(a.clone(), b.clone());
    assert_eq!(
        get(&app, "/api/v1/relay/messages", WORKER).await.0,
        StatusCode::OK,
        "the negative case must use a currently authenticated worker proof"
    );
    let a_head = a.head().unwrap();
    let b_head = b.head().unwrap();
    for token in ["a".repeat(48), "v".repeat(48), WORKER.into()] {
        let (status, body) = get(&app, "/api/v1/brokers/identity", &token).await;
        assert!(
            matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
            "control and worker proofs must not become broker identity proofs"
        );
        assert!(body.get("broker_id").is_none());
        assert_eq!(a.head().unwrap(), a_head);
        assert_eq!(b.head().unwrap(), b_head);
    }
}

#[tokio::test]
async fn broker_identity_refuses_a_namespace_that_is_the_current_bearer_secret() {
    let directory = support::private_tempdir();
    let a = store(directory.path(), "tenant-a", 6);
    let b = store(directory.path(), "tenant-b", 7);
    // A canonical namespace UUID is also a valid 36-byte Bearer. The issuer
    // accepts it; an identity projection must refuse rather than publish it.
    let bearer = a
        .supervision_state()
        .unwrap()
        .control_namespace_uuid
        .unwrap();
    bind_broker(&a, &bearer);
    bind_broker(&b, BROKER_B);
    let app = router(a.clone(), b.clone());
    assert_eq!(
        get(&app, "/api/v1/brokers/self", &bearer).await.0,
        StatusCode::OK,
        "the collision case must first authenticate its real broker proof"
    );
    let a_head = a.head().unwrap();
    let b_head = b.head().unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/brokers/identity")
                .header("authorization", format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(
        !status.is_success(),
        "a valid proof must not expose its raw bytes as identity metadata"
    );
    assert!(
        !bytes
            .windows(bearer.len())
            .any(|bytes| bytes == bearer.as_bytes()),
        "the opaque refusal must not echo the proof"
    );
    let error: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(error.get("broker_id").is_none());
    assert!(
        error.get("control_namespace_uuid").is_none(),
        "no masked or partial identity view is returned"
    );
    assert_eq!(a.head().unwrap(), a_head);
    assert_eq!(b.head().unwrap(), b_head);
}
fn decision_admission(store: &Store) -> AdmissionV1 {
    let session_id = Uuid::new_v4().to_string();
    let request = DecisionRequest {
        request_id: Uuid::new_v4().to_string(),
        profile: "strict".into(),
        command: CommandSpec {
            argv: vec!["/usr/bin/id".into()],
            cwd: "/workspace".into(),
        },
        project_id: None,
        sandbox_id: None,
        agent_id: None,
        session_id: Some(session_id.clone()),
        correlation_id: None,
        trace_id: None,
        human_approved: false,
        approval_hash: None,
        execution_mode: None,
        provider_resolution: None,
    };
    let policy = Policy::default();
    let stored = store
        .decision(
            &request,
            Some((policy::evaluate(&policy, &request), json!(null))),
        )
        .unwrap()
        .unwrap();
    AdmissionV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        receipt: DecisionReceiptV1 {
            version: 1,
            request_id: request.request_id.clone(),
            request_hash: policy::request_hash(&request),
            phase: DecisionPhaseV1::Initial,
            audit_seq: stored.seq,
            audit_hash: stored.hash,
        },
        launch: LaunchSpecV1 {
            version: 1,
            session_id,
            command: request.command.clone(),
            profile: request.profile.clone(),
            runtime_kind: RuntimeKindV1::Docker,
            authority_id: Uuid::new_v4().to_string(),
            image_ref: "worker:fixture".into(),
            image_id: format!("sha256:{HASH_A}"),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1_073_741_824,
            pids_limit: 256,
            timeout_seconds: 600,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: RestartPolicyV1::No,
            workspace_manifest_digest: HASH_B.into(),
            workspace_import_policy_version: 1,
            provider: "generic".into(),
            capture_output: false,
            credential_binding_id: None,
            credential_names: vec![],
            provider_resolution: None,
        },
        decision_request: request,
    }
}

fn supervised_request(admission: AdmissionV1) -> SupervisedAdmissionV1 {
    let compact = Uuid::new_v4().simple().to_string();
    SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: format!("{compact}{compact}"),
        admission,
    }
}

fn admit_supervised(store: &Store, request: &SupervisedAdmissionV1) -> AdmissionResultV1 {
    let policy = Policy::default();
    store
        .admit_supervised(
            request,
            &policy,
            policy::evaluate(&policy, &request.admission.decision_request),
            json!(null),
            &QuotaPolicyV1::default(),
        )
        .unwrap()
}

fn mutation(worker: &WorkerV1, transition: WorkerTransitionV1) -> WorkerMutationV1 {
    WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition,
    }
}

fn inspection(worker: &WorkerV1) -> RuntimeInspectionV1 {
    RuntimeInspectionV1 {
        authority_id: worker.launch.authority_id.clone(),
        runtime_kind: worker.launch.runtime_kind,
        container_id: "d".repeat(64),
        stable_name: worker.attempt.stable_name.clone(),
        volume_name: worker.attempt.volume_name.clone(),
        image_id: worker.launch.image_id.clone(),
        spec_hash: worker.spec_hash.clone(),
        cpu_millis: worker.launch.cpu_millis,
        memory_bytes: worker.launch.memory_bytes,
        pids_limit: worker.launch.pids_limit,
        network: worker.launch.network,
        security_preset: worker.launch.security_preset,
        restart_policy: worker.launch.restart_policy,
    }
}

fn host_clock(ns: u128) -> HostClockSampleV1 {
    HostClockSampleV1 {
        version: 1,
        method: HostClockMethodV1::DarwinMachContinuousV1,
        boot_id: "11111111-1111-4111-8111-111111111111".into(),
        continuous_ns_decimal: ns.to_string(),
        observed_utc: Some("2026-10-30T12:00:00Z".into()),
    }
}

fn worker_arm(worker: &WorkerV1, intent_hash: &str) -> ArmV1 {
    let inspection = worker.attempt.created_inspection.clone().unwrap();
    let start = 100_000_000_000_u128;
    let timeout = worker.launch.timeout_seconds;
    ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        intent_hash: intent_hash.into(),
        subject: ArmSubjectV1::Worker,
        inspection,
        authority_binding_hash: HASH_A.into(),
        configuration_fingerprint_version: 2,
        configuration_fingerprint: HASH_B.into(),
        clock: host_clock(start),
        timeout_seconds: timeout,
        deadline_ns_decimal: (start + u128::from(timeout) * 1_000_000_000).to_string(),
    }
}

fn helper_arm(
    worker: &WorkerV1,
    intent_hash: &str,
    helper_id: Uuid,
    purpose: HelperPurposeV1,
    permission: HelperPermissionV1,
) -> ArmV1 {
    worker
        .launch
        .validate()
        .expect("parent launch must be valid");
    assert_eq!(worker.spec_hash, worker.launch.spec_hash().unwrap());
    let policy = helper_execution_policy_v1(worker, purpose, helper_id).unwrap();
    let start = 100_000_000_000_u128;
    let timeout = 120_u32;
    ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        intent_hash: intent_hash.into(),
        subject: ArmSubjectV1::Helper {
            helper_id: helper_id.to_string(),
            purpose,
            policy_version: 1,
            permission: Box::new(permission),
        },
        inspection: RuntimeInspectionV1 {
            authority_id: policy.authority_id.clone(),
            runtime_kind: policy.runtime_kind,
            container_id: format!("{:064x}", helper_id.as_u128()),
            stable_name: policy.stable_name.clone(),
            volume_name: policy.volume_name.clone(),
            image_id: policy.image_id.clone(),
            spec_hash: policy.spec_hash().unwrap(),
            cpu_millis: policy.cpu_millis,
            memory_bytes: policy.memory_bytes,
            pids_limit: policy.pids_limit,
            network: policy.network,
            security_preset: policy.security_preset,
            restart_policy: policy.restart_policy,
        },
        authority_binding_hash: HASH_A.into(),
        configuration_fingerprint_version: 2,
        configuration_fingerprint: HASH_C.into(),
        clock: host_clock(start),
        timeout_seconds: timeout,
        deadline_ns_decimal: (start + u128::from(timeout) * 1_000_000_000).to_string(),
    }
}

fn helper_claim(_arm: &ArmV1, receipt: &ArmReceiptV1, helper_id: Uuid) -> HelperStartClaimV1 {
    HelperStartClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: helper_id.to_string(),
        emitter_id: Uuid::new_v4().to_string(),
        arm_receipt: receipt.clone(),
        clock_check: host_clock(100_000_000_001),
    }
}

fn helper_observation(
    arm: &ArmV1,
    receipt: &ArmReceiptV1,
    helper_id: Uuid,
    running: Option<bool>,
    exit_code: i64,
    verification: Option<agentsandbox::control::ImportVerificationReportV1>,
) -> HelperObservationV1 {
    HelperObservationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: helper_id.to_string(),
        arm_receipt: receipt.clone(),
        observation: RuntimeObservationV1 {
            inspection: arm.inspection.clone(),
            observed_at: "2026-10-30T12:00:02Z".into(),
            status: if running == Some(true) {
                RuntimeStatusV1::Running
            } else {
                RuntimeStatusV1::Exited
            },
            running,
            started_at: Some("2026-10-30T12:00:00Z".into()),
            finished_at: Some("2026-10-30T12:00:01Z".into()),
            exit_code: Some(exit_code),
            oom_killed: Some(false),
            restart_count: Some(0),
        },
        verification,
    }
}

fn parent_observation(
    worker: &WorkerV1,
    status: RuntimeStatusV1,
    running: bool,
    finished_at: Option<&str>,
    exit_code: Option<i64>,
) -> RuntimeObservationV1 {
    RuntimeObservationV1 {
        inspection: worker.attempt.created_inspection.clone().unwrap(),
        observed_at: "2026-10-30T12:00:02Z".into(),
        status,
        running: Some(running),
        started_at: Some("2026-10-30T12:00:00Z".into()),
        finished_at: finished_at.map(str::to_owned),
        exit_code,
        oom_killed: Some(false),
        restart_count: Some(0),
    }
}

fn running_worker(store: &Store) -> WorkerV1 {
    let request = supervised_request(decision_admission(store));
    let admitted = admit_supervised(store, &request);
    let mut worker = admitted.worker.unwrap();
    worker = store
        .mutate_worker(&mutation(&worker, WorkerTransitionV1::Prepare))
        .unwrap()
        .worker;

    // The real typed import and independent verification gates remain active in this fixture.
    let import_id = Uuid::new_v4();
    let import_arm = helper_arm(
        &worker,
        &request.intent_hash,
        import_id,
        HelperPurposeV1::Import,
        HelperPermissionV1::Admission {
            receipt: admitted.receipt,
        },
    );
    let imported = store.arm_execution(&import_arm).unwrap();
    store
        .claim_helper_start(&helper_claim(&import_arm, &imported.receipt, import_id))
        .unwrap();
    let import_terminal = store
        .observe_helper(&helper_observation(
            &import_arm,
            &imported.receipt,
            import_id,
            Some(false),
            0,
            None,
        ))
        .unwrap();

    let verify_id = Uuid::new_v4();
    let verify_arm = helper_arm(
        &worker,
        &request.intent_hash,
        verify_id,
        HelperPurposeV1::ImportVerification,
        HelperPermissionV1::ImportVerification {
            import_helper_id: import_id.to_string(),
            import_arm: imported.receipt,
            import_terminal,
            manifest_digest: worker.launch.workspace_manifest_digest.clone(),
        },
    );
    let verified = store.arm_execution(&verify_arm).unwrap();
    store
        .claim_helper_start(&helper_claim(&verify_arm, &verified.receipt, verify_id))
        .unwrap();
    store
        .observe_helper(&helper_observation(
            &verify_arm,
            &verified.receipt,
            verify_id,
            Some(false),
            0,
            Some(ImportVerificationReportV1 {
                import_helper_id: import_id.to_string(),
                expected_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
                observed_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
            }),
        ))
        .unwrap();

    worker = store
        .mutate_worker(&mutation(&worker, WorkerTransitionV1::CreateIssued))
        .unwrap()
        .worker;
    let mut observed = inspection(&worker);
    observed.container_id = format!("{:064x}", Uuid::new_v4().as_u128());
    worker = store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::Created {
                inspection: observed,
            },
        ))
        .unwrap()
        .worker;
    let arm = worker_arm(&worker, &request.intent_hash);
    let armed = store.arm_execution(&arm).unwrap();
    worker = store
        .claim_armed_start(&ArmedStartClaimV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            expected_revision: worker.revision,
            emitter_id: Uuid::new_v4().to_string(),
            arm_receipt: armed.receipt,
            clock_check: host_clock(100_000_000_001),
        })
        .unwrap()
        .worker;
    let observation = parent_observation(&worker, RuntimeStatusV1::Running, true, None, None);
    let worker = store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::Observe { observation },
        ))
        .unwrap()
        .worker;
    assert_eq!(worker.state, WorkerStateV1::Running);
    worker
}
