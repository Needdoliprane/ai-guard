//! Real SQLite identities and HTTP routing; no Kubernetes engine or model is called.
use super::*;
use crate::{
    broker_registry::{BrokerAuthorityV1, BrokerGrantMutationV1, BrokerRuntimeKindV1},
    control::kubernetes_admission::*,
    runtime::kubernetes::{KubernetesAuthorityV1, KubernetesPreflightResourcesV1},
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use axum::body::to_bytes;
use std::os::unix::fs::PermissionsExt;
use tower::ServiceExt;
use uuid::Uuid;

const BASE: &str = "/api/v1/brokers/kubernetes";
const EXPIRY: &str = "2100-01-01T00:00:00.000Z";

struct Fixture {
    _dir: tempfile::TempDir,
    tenant: String,
    token: String,
    store: Arc<Store>,
    intent: KubernetesCreateIntentV1,
}
impl Fixture {
    fn new(tenant: &str) -> Self {
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .expect("private API fixture");
        let store = Store::open_for_tenant(
            &dir.path().join("tenant.sqlite"),
            tenant,
            Redactor::new(vec![]),
            10_000,
        )
        .unwrap();
        store
            .migrate_control_v3(&dir.path().join("before3"))
            .unwrap();
        store
            .migrate_identity_v4(&dir.path().join("before4"))
            .unwrap();
        store
            .migrate_human_access_v5(&dir.path().join("before5"))
            .unwrap();
        store
            .migrate_workload_v6(&dir.path().join("before6"))
            .unwrap();
        store
            .migrate_human_brokers_v7(&dir.path().join("before7"))
            .unwrap();
        store
            .migrate_kubernetes_create_v8(&dir.path().join("before8"))
            .unwrap();
        store
            .migrate_kubernetes_policy_v9(&dir.path().join("before9"))
            .unwrap();
        let broker_id = Uuid::new_v4().to_string();
        let incarnation_id = Uuid::new_v4().to_string();
        let credential_id = Uuid::new_v4().to_string();
        let token = format!("public-api-broker-{tenant}-{}", "z".repeat(40));
        let authority = KubernetesAuthorityV1::new(
            Uuid::new_v4(),
            "https://127.0.0.1:6443",
            "11111111-1111-4111-8111-111111111111",
            tenant,
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
                sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                expires_at: EXPIRY.into(),
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
                not_after: EXPIRY.into(),
            })
            .unwrap();
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
            _dir: dir,
            tenant: tenant.into(),
            token,
            store: Arc::new(store),
            intent,
        }
    }
    fn policy(&self) -> KubernetesHeldCreatePolicyV1 {
        KubernetesHeldCreatePolicyV1 {
            version: 1,
            decision_ttl_seconds: 30,
            approvals: vec![KubernetesHeldCreateApprovalV1 {
                authority_fingerprint: self.intent.authority.fingerprint(),
                image: self.intent.image.clone(),
                profile: self.intent.profile.clone(),
            }],
        }
    }
    fn gateway(&self) -> TenantGateway {
        TenantGateway {
            tenant_id: self.tenant.clone(),
            store: Arc::clone(&self.store),
            runner_token: format!("public-runner-{}-{}", self.tenant, "r".repeat(40)),
            viewer_token: format!("public-viewer-{}-{}", self.tenant, "v".repeat(40)),
        }
    }
    fn state(&self, policy: bool, quota: QuotaPolicyV1) -> GatewayState {
        let state = GatewayState::for_tenants(
            vec![self.gateway()],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap()
        .with_worker_quotas(vec![(self.tenant.clone(), quota)])
        .unwrap();
        if policy {
            state
                .with_kubernetes_held_create_policies(vec![(self.tenant.clone(), self.policy())])
                .unwrap()
        } else {
            state
        }
    }
}

async fn call(
    router: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    bytes: Vec<u8>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("{BASE}{path}"))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let reply = router
        .clone()
        .oneshot(request.body(Body::from(bytes)).unwrap())
        .await
        .unwrap();
    let status = reply.status();
    let bytes = to_bytes(reply.into_body(), 128 * 1024)
        .await
        .expect("response is bounded");
    (
        status,
        serde_json::from_slice(&bytes).expect("closed JSON response"),
    )
}
fn bytes<T: serde::Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

#[tokio::test]
async fn held_http_uses_authenticated_tenant_policy_and_replays_exact_reservations() {
    let a = Fixture::new("tenant-a");
    let b = Fixture::new("tenant-b");
    let mut policy_a = a.policy();
    policy_a.approvals.extend(b.policy().approvals);
    assert!(
        policy_a.approves(&b.intent).unwrap(),
        "policy A would permit B's exact intent if the handler chose the wrong tenant configuration"
    );
    let state = GatewayState::for_tenants(
        vec![a.gateway(), b.gateway()],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap()
    .with_kubernetes_held_create_policies(vec![(a.tenant.clone(), policy_a)])
    .unwrap();
    let router = gateway_router(state);
    let (status, protocol) = call(
        &router,
        "GET",
        "/held-create-protocol",
        Some(&a.token),
        vec![],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(protocol["tenant_id"], "tenant-a");
    assert_eq!(protocol["schema_version"], 9);
    assert_eq!(protocol["admission_policy_configured"], true);
    let (status, original) = call(
        &router,
        "POST",
        "/held-creates",
        Some(&a.token),
        bytes(&a.intent),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(original["replayed"], false);
    let head = a.store.head().unwrap();
    let (status, replay) = call(
        &router,
        "POST",
        "/held-creates",
        Some(&a.token),
        bytes(&a.intent),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], original["receipt"]);
    assert_eq!(replay["snapshot"], original["snapshot"]);
    assert_eq!(a.store.head().unwrap(), head);
    let head_b = b.store.head().unwrap();
    let (status, _) = call(
        &router,
        "POST",
        "/held-creates",
        Some(&b.token),
        bytes(&a.intent),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(b.store.head().unwrap(), head_b);
    assert_eq!(a.store.head().unwrap(), head);
    let (status, _) = call(
        &router,
        "POST",
        "/held-creates",
        Some(&b.token),
        bytes(&b.intent),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "tenant B has no configured admission policy"
    );
    assert_eq!(b.store.head().unwrap(), head_b);

    let mut unapproved = a.intent.clone();
    unapproved.create_id = Uuid::new_v4().to_string();
    unapproved.session_id = Uuid::new_v4().to_string();
    unapproved.image = format!("different-worker@sha256:{}", "b".repeat(64));
    let (status, _) = call(
        &router,
        "POST",
        "/held-creates",
        Some(&a.token),
        bytes(&unapproved),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the same digest under a different image name is not configured"
    );
    assert_eq!(a.store.head().unwrap(), head);

    let lowered = QuotaPolicyV1 {
        max_active_workers: 0,
        max_cpu_millis: 0,
        max_memory_bytes: 0,
        ..QuotaPolicyV1::default()
    };
    let quota_router = gateway_router(a.state(true, lowered.clone()));
    let mut extra = a.intent.clone();
    extra.create_id = Uuid::new_v4().to_string();
    extra.session_id = Uuid::new_v4().to_string();
    let (status, _) = call(
        &quota_router,
        "POST",
        "/held-creates",
        Some(&a.token),
        bytes(&extra),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "admission uses the current tenant quota"
    );
    assert_eq!(a.store.head().unwrap(), head);

    let withdrawn = gateway_router(a.state(false, lowered));
    let cancel = KubernetesCreateCancelV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: a.intent.create_id.clone(),
        intent_hash: a.intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
    };
    let path = format!("/held-creates/{}/cancellations", a.intent.create_id);
    let (status, cancelled) = call(&withdrawn, "POST", &path, Some(&a.token), bytes(&cancel)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cancelled["snapshot"]["state"], "cancelled_before_emission");
    assert_eq!(cancelled["snapshot"]["quota_held"], false);
    let head = a.store.head().unwrap();
    let (status, replay) = call(&withdrawn, "POST", &path, Some(&a.token), bytes(&cancel)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["receipt"], cancelled["receipt"]);
    assert_eq!(replay["snapshot"], cancelled["snapshot"]);
    assert_eq!(a.store.head().unwrap(), head);
}

#[tokio::test]
async fn held_http_rejects_body_scope_overrides_ambiguous_json_and_secrets_before_store() {
    let mut a = Fixture::new("tenant-a");
    // This image is deliberately approved in config, so policy rejection cannot
    // hide a missing pre-persistence check for the current workload bearer.
    a.intent.image = format!("{}/worker@sha256:{}", a.token, "b".repeat(64));
    let router = gateway_router(a.state(true, QuotaPolicyV1::default()));
    let (status, _) = call(
        &router,
        "GET",
        "/held-create-protocol",
        Some(&a.token),
        vec![],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "positive authenticated precondition"
    );
    let parsed: Value = kubernetes::decode_request(br#"{"outer":{"k":1}}"#)
        .expect("positive strict JSON precondition");
    assert_eq!(parsed, json!({"outer":{"k":1}}));
    for malformed in [
        br#"{"outer":{"k":1,"k":1}}"#.to_vec(),
        ["[".repeat(65), "0".into(), "]".repeat(65)]
            .concat()
            .into_bytes(),
        b"{} {}".to_vec(),
    ] {
        let rejection = kubernetes::decode_request::<Value>(&malformed)
            .err()
            .expect("reject ambiguous/deep/trailing JSON");
        assert_eq!(rejection.0, StatusCode::BAD_REQUEST);
    }
    let output_redactor = Redactor::new(vec![a.token.clone()]);
    let positive =
        kubernetes::encode_response(StatusCode::OK, &json!({"version":1}), &output_redactor)
            .expect("positive exact response precondition");
    assert_eq!(positive.status(), StatusCode::OK);
    let mut secret_key = serde_json::Map::new();
    secret_key.insert(a.token.clone(), json!("public value"));
    for rejected in [
        json!({"value":a.token.clone()}),
        Value::Object(secret_key),
        json!({"value":"x".repeat(128*1024)}),
    ] {
        let rejection = kubernetes::encode_response(StatusCode::OK, &rejected, &output_redactor)
            .err()
            .expect("reject secret or oversized response");
        assert_eq!(rejection.0, StatusCode::SERVICE_UNAVAILABLE);
        assert!(!rejection.1.0.to_string().contains(&a.token));
    }
    let head = a.store.head().unwrap();
    let (status, body) = call(
        &router,
        "POST",
        "/held-creates",
        Some(&a.token),
        bytes(&a.intent),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!body.to_string().contains(&a.token));
    assert_eq!(a.store.head().unwrap(), head);
    let mut clean = a.intent.clone();
    clean.image = format!("worker@sha256:{}", "b".repeat(64));
    for key in ["tenant_id", "policy", "held_create_policy", "quota"] {
        let mut input = serde_json::to_value(&clean).unwrap();
        input[key] = json!({"allow": true});
        let (status, _) = call(
            &router,
            "POST",
            "/held-creates",
            Some(&a.token),
            bytes(&input),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "accepted scope override {key}"
        );
    }
    let input = serde_json::to_string(&clean).unwrap();
    let duplicate = input.replacen("\"version\":1", "\"version\":1,\"version\":1", 1);
    assert_ne!(duplicate, input);
    let under_limit = input.as_bytes();
    assert!(under_limit.len() < 64 * 1024);
    assert!(kubernetes::decode_request::<KubernetesCreateIntentV1>(under_limit).is_ok());
    let mut oversized = under_limit.to_vec();
    oversized.resize(64 * 1024 + 1, b' ');
    assert!(
        serde_json::from_slice::<KubernetesCreateIntentV1>(&oversized).is_ok(),
        "the oversized neighbor is otherwise valid JSON and intent"
    );
    for malformed in [
        duplicate.into_bytes(),
        format!("{input} {{}}").into_bytes(),
        oversized,
    ] {
        let (status, _) = call(&router, "POST", "/held-creates", Some(&a.token), malformed).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    for token in [
        None,
        Some("unknown-public-token-for-this-test-0000000000000000"),
    ] {
        let (status, _) = call(&router, "POST", "/held-creates", token, bytes(&clean)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let runner = a.gateway().runner_token;
    let (status, _) = call(
        &router,
        "POST",
        "/held-creates",
        Some(&runner),
        bytes(&clean),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(a.store.head().unwrap(), head);
}

#[tokio::test]
async fn held_http_reconciles_after_policy_withdrawal_without_allocating_or_replacing_old_receipts()
{
    let a = Fixture::new("tenant-a");
    let router = gateway_router(a.state(true, QuotaPolicyV1::default()));
    let (status, _) = call(
        &router,
        "POST",
        "/held-creates",
        Some(&a.token),
        bytes(&a.intent),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let claim = KubernetesCreateClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: a.intent.create_id.clone(),
        intent_hash: a.intent.intent_hash().unwrap(),
        expected_revision: 1,
        expected_create_epoch: 1,
        emitter_id: Uuid::new_v4().to_string(),
    };
    let claim_path = format!("/held-creates/{}/claims", a.intent.create_id);
    let (status, claimed) = call(&router, "POST", &claim_path, Some(&a.token), bytes(&claim)).await;
    assert!(matches!(status, StatusCode::CREATED | StatusCode::OK));
    assert_eq!(claimed["receipt"]["operation_id"], claim.operation_id);
    assert_eq!(claimed["snapshot"]["revision"], 2);
    let paused = gateway_router(a.state(
        false,
        QuotaPolicyV1 {
            max_active_workers: 0,
            max_cpu_millis: 0,
            max_memory_bytes: 0,
            ..QuotaPolicyV1::default()
        },
    ));
    let lookup = format!("{claim_path}/{}", claim.operation_id);
    let head = a.store.head().unwrap();
    let absent = format!("{claim_path}/{}", Uuid::new_v4());
    let (status, _) = call(&paused, "GET", &absent, Some(&a.token), vec![]).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an absent operation must not adopt snapshot.claim"
    );
    assert_eq!(a.store.head().unwrap(), head);
    let (status, historical) = call(&paused, "GET", &lookup, Some(&a.token), vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(historical["disposition"], "observe_only");
    assert_eq!(historical["receipt"], claimed["receipt"]);
    assert_eq!(historical["snapshot"], claimed["snapshot"]);
    assert_eq!(a.store.head().unwrap(), head);
    let result = KubernetesCreateResultV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        create_id: a.intent.create_id.clone(),
        intent_hash: a.intent.intent_hash().unwrap(),
        claim_operation_id: claim.operation_id.clone(),
        expected_revision: 2,
        expected_create_epoch: 1,
        outcome: KubernetesCreateOutcomeV1::OwnershipUnknown {
            reason: KubernetesCreateUnknownReasonV1::RecoveredAfterClaim,
            http_status: None,
        },
    };
    let (status, recorded) = call(
        &paused,
        "POST",
        &format!("/held-creates/{}/results", a.intent.create_id),
        Some(&a.token),
        bytes(&result),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recorded["snapshot"]["state"], "ownership_unknown");
    assert_eq!(recorded["snapshot"]["quota_held"], true);
    let head = a.store.head().unwrap();
    let (status, old) = call(&paused, "GET", &lookup, Some(&a.token), vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(old["receipt"], claimed["receipt"]);
    assert_eq!(
        old["snapshot"], claimed["snapshot"],
        "return the historical snapshot, not the current result state"
    );
    assert_eq!(a.store.head().unwrap(), head);
}
