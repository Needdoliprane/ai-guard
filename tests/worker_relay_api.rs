//! Relay storage regression fixtures use real typed supervision and SQLite, without a runtime.
mod support;

#[path = "support/relay_client_cases.rs"]
mod client_cases;

#[path = "support/relay_client_profile_cases.rs"]
mod client_profile_cases;

#[path = "support/control_lane_cases.rs"]
mod control_lane_cases;

#[path = "support/relay_admission_cases.rs"]
mod relay_admission_cases;

#[path = "support/relay_admission_fixture.rs"]
mod relay_admission_fixture;
use relay_admission_fixture::*;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{AuditCapacityV1, EventQuery, Store, WorkloadAuditPolicyV1},
    broker_registry::{
        BrokerAuthorityV1, BrokerGrantMutationV1, BrokerHeartbeatV1, BrokerRevocationV1,
        BrokerRuntimeKindV1,
    },
    control::*,
    identity::{
        HumanAccountV1, HumanCapabilityV1, IdentityChangeV1, IdentityMutationV1,
        LegacyControlRoleV1, LocalCredentialV1, MembershipV1, PrincipalRefV1,
    },
    model::{CommandSpec, DecisionRequest},
    policy::{self, Policy},
    redaction::Redactor,
    relay::*,
    workload::{WorkloadBindingIssueV1, WorkloadBindingRevocationV1, WorkloadSubjectV1},
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
use tower::ServiceExt;
use uuid::Uuid;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
// The production issuer deliberately rejects concurrent HTTP emissions across
// GatewayState instances in one process. Isolate the two fixture campaigns.
static HTTP_ISSUE_TEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

fn bind_local_human_metadata(store: &Store, token: &str) -> PrincipalRefV1 {
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: Uuid::new_v4().to_string(),
        account_id: Uuid::new_v4().to_string(),
    };
    let account = HumanAccountV1 {
        version: 1,
        principal: principal.clone(),
        display_name: "Local identity collision fixture".into(),
        revision: 1,
        active: true,
    };
    let credential = LocalCredentialV1 {
        version: 1,
        credential_id: Uuid::new_v4().to_string(),
        principal: principal.clone(),
        sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
        revision: 1,
        active: true,
        expires_at: None,
    };
    for change in [
        IdentityChangeV1::Account {
            expected_revision: 0,
            account,
        },
        IdentityChangeV1::Credential {
            expected_revision: 0,
            binding: credential,
        },
    ] {
        store
            .mutate_identity(&IdentityMutationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                actor: PrincipalRefV1::LegacyControl {
                    credential_id: "operator-fixture".into(),
                    role: LegacyControlRoleV1::Runner,
                },
                change,
            })
            .unwrap();
    }
    principal
}
fn directed(source: &WorkerV1, destination: &WorkerV1, seconds: i64) -> RelayGrantV1 {
    RelayGrantV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: Uuid::new_v4().to_string(),
        source: endpoint(source),
        destination: endpoint(destination),
        channel: "tasks".into(),
        expires_at: (chrono::Utc::now() + chrono::Duration::seconds(seconds)).to_rfc3339(),
    }
}
fn send(grant: &RelayGrantV1, body: &str) -> RelaySendV1 {
    RelaySendV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: grant.grant_id.clone(),
        ttl_seconds: 60,
        body: body.into(),
    }
}
fn router(a: Arc<Store>, b: Arc<Store>) -> Router {
    router_with_runner_tokens(a, b, ["a", "b"])
}
fn router_with_runner_tokens(a: Arc<Store>, b: Arc<Store>, runners: [&str; 2]) -> Router {
    gateway_router(
        GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: a,
                    runner_token: runners[0].repeat(48),
                    viewer_token: "v".repeat(48),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: b,
                    runner_token: runners[1].repeat(48),
                    viewer_token: "w".repeat(48),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap(),
    )
}
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .header("x-tenant-id", "tenant-b")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn bound_worker_relay_credential_cannot_publish_control_events_before_body() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 2000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 2000));
    let worker = running_worker(&a);
    let token = "7".repeat(48);
    bind(&a, &worker, &token);
    let app = router(a.clone(), b.clone());
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/relay/messages?limit=32",
            &token,
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    let before = (a.head().unwrap(), b.head().unwrap());
    let polled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = polled.clone();
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/control/events")
        .header("authorization", format!("Bearer {token}"))
        .header("x-tenant-id", "tenant-b")
        .header("content-type", "application/json")
        .body(Body::from_stream(futures_util::stream::once(async move {
            observed.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok::<_, std::io::Error>("unread invalid body")
        })))
        .unwrap();
    assert_eq!(
        app.oneshot(request).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!((a.head().unwrap(), b.head().unwrap()), before);
}

#[tokio::test]
async fn human_identity_refuses_live_and_historical_workload_collision_in_either_tenant() {
    // This deliberately authorized Store provisioning represents operator
    // misconfiguration, not a CLI default or a reproduced worker exploit.
    for human_in_b in [false, true] {
        let directory = support::private_tempdir();
        let a = Arc::new(fresh(directory.path(), "tenant-a", 2000));
        let b = Arc::new(fresh(directory.path(), "tenant-b", 2000));
        let worker = running_worker(&a);
        let token = format!("aigh1_{}", "1".repeat(64));
        let (credential, revision) = bind(&a, &worker, &token);
        let app = router(a.clone(), b.clone());
        assert_eq!(
            call(&app, "GET", "/api/v1/human/identity", &token, Value::Null)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        bind_local_human_metadata(if human_in_b { &b } else { &a }, &token);
        let result = call(&app, "GET", "/api/v1/human/identity", &token, Value::Null).await;
        assert_eq!(
            result.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "readable workload ownership did not block human authentication"
        );
        assert_eq!(result.1["error"]["code"], "identity_unavailable");
        assert_eq!(result.1["error"]["retry_action"], "contact_operator");
        assert!(result.1.get("principal").is_none() && result.1.get("contexts").is_none());
        assert_eq!(
            call(
                &app,
                "GET",
                "/api/v1/relay/messages?limit=32",
                &token,
                Value::Null
            )
            .await
            .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        let rotated = format!("aigh1_{}", "2".repeat(64));
        let receipt = a
            .issue_workload_binding(&WorkloadBindingIssueV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                credential_id: credential.clone(),
                expected_revision: revision,
                subject: WorkloadSubjectV1::Worker {
                    worker_id: worker.worker_id,
                    attempt_id: worker.attempt.attempt_id,
                    generation: 1,
                },
                sha256_digest: hex::encode(Sha256::digest(rotated.as_bytes())),
                expires_at: (chrono::Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
            })
            .unwrap();
        assert_eq!(
            call(&app, "GET", "/api/v1/human/identity", &token, Value::Null)
                .await
                .0,
            StatusCode::SERVICE_UNAVAILABLE,
            "rotation forgot historical ownership"
        );
        a.revoke_workload_binding(&WorkloadBindingRevocationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential,
            expected_revision: receipt.revision,
        })
        .unwrap();
        let historical = call(&app, "GET", "/api/v1/human/identity", &token, Value::Null).await;
        assert_eq!(
            historical.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "revocation forgot historical ownership"
        );
        assert_eq!(historical.1["error"]["code"], "identity_unavailable");
        assert!(historical.1.get("principal").is_none() && historical.1.get("contexts").is_none());
    }
}

#[tokio::test]
async fn extracted_human_stream_revalidates_workload_collision_before_first_content_poll() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 2000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 2000));
    let worker = running_worker(&a);
    let token = format!("aigh1_{}", "3".repeat(64));
    let principal = bind_local_human_metadata(&a, &token);
    a.mutate_identity(&IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "operator-fixture".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change: IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: MembershipV1 {
                version: 1,
                membership_id: Uuid::new_v4().to_string(),
                principal,
                tenant_id: "tenant-a".into(),
                organization_scope: "organization-stream".into(),
                revision: 1,
                active: true,
                capabilities: std::collections::BTreeSet::from([
                    HumanCapabilityV1::ReadMetadata,
                    HumanCapabilityV1::ReadTenantMetadata,
                ]),
            },
        },
    })
    .unwrap();
    a.append(agentsandbox::model::Event::new(
        "fixture.human_stream_visible",
    ))
    .unwrap();
    let app = router(a.clone(), b);
    let identity = call(&app, "GET", "/api/v1/human/identity", &token, Value::Null).await;
    assert_eq!(identity.0, StatusCode::OK);
    let context = identity.1["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|context| context["mode"] == "administration")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let request = Request::builder()
        .uri("/api/v1/human/stream?limit=1&view=metadata")
        .header("authorization", format!("Bearer {token}"))
        .header("x-aiguard-context", context)
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    bind(&a, &worker, &token);
    let mut frames = response.into_body().into_data_stream();
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), frames.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let frame = String::from_utf8(first.to_vec()).unwrap();
    assert!(
        frame.contains("event: error")
            && frame.contains("identity_unavailable")
            && frame.contains("contact_operator")
    );
    assert!(!frame.contains("human_audit") && !frame.contains(&token));
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), frames.next())
            .await
            .unwrap()
            .is_none(),
        "collision stream did not terminate after its error frame"
    );
}

#[tokio::test]
async fn healthy_human_source_keeps_partial_context_status_when_a_neighbor_is_unavailable() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 1000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 1000));
    let token = format!("aigh1_{}", "4".repeat(64));
    bind_local_human_metadata(&a, &token);
    let app = router(a.clone(), b);
    let initial = call(&app, "GET", "/api/v1/human/identity", &token, Value::Null).await;
    assert_eq!(initial.0, StatusCode::OK);
    assert_eq!(initial.1["contexts_complete"], true);
    let corrupted = rusqlite::Connection::open(directory.path().join("tenant-b.sqlite")).unwrap();
    corrupted
        .execute("UPDATE quota SET bytes=bytes+1 WHERE id=1", [])
        .unwrap();
    drop(corrupted);
    let partial = call(&app, "GET", "/api/v1/human/identity", &token, Value::Null).await;
    assert_eq!(partial.0, StatusCode::OK);
    assert_eq!(
        partial.1["contexts_complete"], false,
        "unavailable neighbor was presented as a complete empty profile"
    );
    let corrupted = rusqlite::Connection::open(directory.path().join("tenant-a.sqlite")).unwrap();
    corrupted
        .execute("UPDATE quota SET bytes=bytes+1 WHERE id=1", [])
        .unwrap();
    drop(corrupted);
    let unavailable = call(&app, "GET", "/api/v1/human/identity", &token, Value::Null).await;
    assert_eq!(unavailable.0, StatusCode::SERVICE_UNAVAILABLE);
    assert!(unavailable.1.get("principal").is_none() && unavailable.1.get("contexts").is_none());
}

#[tokio::test]
async fn workload_http_issuance_refuses_aliases_and_valid_auth_survives_an_unavailable_neighbor() {
    let _issue_campaign = HTTP_ISSUE_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 2000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 2000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let b1 = running_worker(&b);
    let token = "5".repeat(48);
    let peer = "2".repeat(48);
    let neighbor = "3".repeat(48);
    let human_token = "8".repeat(48);
    bind_local_human_metadata(&b, &human_token);
    bind(&a, &a2, &peer);
    bind(&b, &b1, &neighbor);
    let app = router(a.clone(), b.clone());
    let input = WorkloadBindingIssueV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: Uuid::new_v4().to_string(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Worker {
            worker_id: a1.worker_id.clone(),
            attempt_id: a1.attempt.attempt_id.clone(),
            generation: 1,
        },
        sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
        expires_at: (chrono::Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
    };
    let issued = call(
        &app,
        "POST",
        "/api/v1/workload-bindings",
        &"a".repeat(48),
        json!(input),
    )
    .await;
    assert_eq!(issued.0, StatusCode::CREATED);
    assert!(!issued.1.to_string().contains(&token));
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/workload-bindings",
            &"a".repeat(48),
            json!(input)
        )
        .await
        .0,
        StatusCode::OK
    );
    for forbidden_token in [&"a".repeat(48), &"v".repeat(48), &neighbor, &human_token] {
        let collision = WorkloadBindingIssueV1 {
            operation_id: Uuid::new_v4().to_string(),
            credential_id: Uuid::new_v4().to_string(),
            sha256_digest: hex::encode(Sha256::digest(forbidden_token.as_bytes())),
            ..input.clone()
        };
        assert_eq!(
            call(
                &app,
                "POST",
                "/api/v1/workload-bindings",
                &"a".repeat(48),
                json!(collision)
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
    }
    let grant = directed(&a1, &a2, 300);
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/grants",
            &"a".repeat(48),
            json!(grant)
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token,
            json!(send(&grant, &token))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "current bearer was accepted as message content"
    );
    for (headers, expected) in [
        (
            vec![
                ("authorization", format!("Bearer {token}")),
                ("authorization", format!("Bearer {token}")),
            ],
            StatusCode::UNAUTHORIZED,
        ),
        (
            vec![("authorization", format!("Bearer {}", "x".repeat(513)))],
            StatusCode::UNAUTHORIZED,
        ),
        (
            vec![
                ("authorization", format!("Bearer {token}")),
                ("origin", "https://untrusted.example".into()),
            ],
            StatusCode::FORBIDDEN,
        ),
    ] {
        let polled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = polled.clone();
        let body = Body::from_stream(futures_util::stream::once(async move {
            observed.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"untrusted unread input"))
        }));
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/v1/relay/messages")
            .header("content-type", "application/json");
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert!(
            !polled.load(std::sync::atomic::Ordering::SeqCst),
            "invalid proof/origin polled body"
        );
    }
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/relay/messages?limit=32",
            &"z".repeat(48),
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let before = rows(
        &directory.path().join("tenant-a.sqlite"),
        "SELECT COUNT(*) FROM workload_bindings",
    );
    let corrupted = rusqlite::Connection::open(directory.path().join("tenant-b.sqlite")).unwrap();
    corrupted
        .execute("UPDATE quota SET bytes=bytes+1 WHERE id=1", [])
        .unwrap();
    drop(corrupted);
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token,
            json!(send(&grant, "authorized source despite unrelated failure"))
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/relay/messages?limit=32",
            &neighbor,
            Value::Null
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable source became an absent credential"
    );
    let new = WorkloadBindingIssueV1 {
        operation_id: Uuid::new_v4().to_string(),
        credential_id: Uuid::new_v4().to_string(),
        sha256_digest: hex::encode(Sha256::digest("6".repeat(48).as_bytes())),
        ..input
    };
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/workload-bindings",
            &"a".repeat(48),
            json!(new)
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE,
        "emission assumed uniqueness in an unreadable neighbor"
    );
    assert_eq!(
        rows(
            &directory.path().join("tenant-a.sqlite"),
            "SELECT COUNT(*) FROM workload_bindings"
        ),
        before
    );
    let revoke = WorkloadBindingRevocationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: input.credential_id.clone(),
        expected_revision: issued.1["revision"].as_u64().unwrap(),
    };
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/workload-bindings/{}/revoke", Uuid::new_v4()),
            &"a".repeat(48),
            json!(revoke)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/workload-bindings/{}/revoke", revoke.credential_id),
            &"a".repeat(48),
            json!(revoke)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/relay/messages?limit=32",
            &token,
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert!(a.integrity().unwrap().valid);
}

#[tokio::test]
async fn broker_http_heartbeat_and_self_are_bound_to_the_authenticated_broker() {
    let _issue_campaign = HTTP_ISSUE_TEST_SERIAL.lock().await;
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 1000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 1000));
    let app = router(a.clone(), b);
    let token = "7".repeat(48);
    let grant = BrokerGrantMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        broker_id: Uuid::new_v4().to_string(),
        expected_revision: None,
        credential_id: Uuid::new_v4().to_string(),
        incarnation_id: Uuid::new_v4().to_string(),
        authorities: vec![BrokerAuthorityV1 {
            authority_id: "fixture-broker-authority-a".into(),
            runtime_kind: BrokerRuntimeKindV1::Docker,
        }],
        lease_seconds: 30,
        not_after: (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
    };
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/brokers/grants",
            &"a".repeat(48),
            json!(grant)
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let binding = WorkloadBindingIssueV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: grant.credential_id.clone(),
        expected_revision: 0,
        subject: WorkloadSubjectV1::Broker {
            broker_id: grant.broker_id.clone(),
            incarnation_id: grant.incarnation_id.clone(),
        },
        sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
        expires_at: grant.not_after.clone(),
    };
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/workload-bindings",
            &"a".repeat(48),
            json!(binding)
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let heartbeat = BrokerHeartbeatV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        expected_heartbeat_revision: 0,
        incarnation_id: grant.incarnation_id.clone(),
        sequence: 1,
    };
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/brokers/heartbeat",
            &token,
            json!(heartbeat)
        )
        .await
        .0,
        StatusCode::OK
    );
    let own = call(
        &app,
        "GET",
        "/api/v1/brokers/self?broker_id=untrusted-selector",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(own.0, StatusCode::OK);
    assert_eq!(own.1["broker"]["broker_id"], grant.broker_id);
    assert_eq!(own.1["runtime_inventory_complete"], false);
    let a_list = call(
        &app,
        "GET",
        "/api/v1/brokers?limit=100",
        &"v".repeat(48),
        Value::Null,
    )
    .await;
    assert_eq!(a_list.0, StatusCode::OK);
    assert_eq!(a_list.1["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/brokers?limit=100",
            &"w".repeat(48),
            Value::Null
        )
        .await
        .1["items"],
        json!([])
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/brokers?limit=0",
            &"v".repeat(48),
            Value::Null
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/brokers", &token, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/relay/messages?limit=32",
            &token,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let mut spoofed = json!(heartbeat);
    spoofed["broker_id"] = json!(Uuid::new_v4().to_string());
    assert_eq!(
        call(&app, "POST", "/api/v1/brokers/heartbeat", &token, spoofed)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let revoke = BrokerRevocationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        broker_id: grant.broker_id.clone(),
        expected_revision: 1,
    };
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/brokers/{}/revoke", Uuid::new_v4()),
            &"a".repeat(48),
            json!(revoke)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/brokers/{}/revoke", grant.broker_id),
            &"a".repeat(48),
            json!(revoke)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/brokers/self", &token, Value::Null)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert!(a.integrity().unwrap().valid);
}

#[tokio::test]
async fn readable_cross_tenant_workload_collision_is_refused_without_revealing_owners() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 1000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 1000));
    let a1 = running_worker(&a);
    let b1 = running_worker(&b);
    let token = "1".repeat(48);
    // A deliberately conflicting trusted provision, bypassing HTTP issue preflight,
    // represents legacy/operator state. HTTP must refuse its ambiguous authority.
    bind(&a, &a1, &token);
    bind(&b, &b1, &token);
    let app = router(a, b);
    let result = call(
        &app,
        "GET",
        "/api/v1/relay/messages?limit=32",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(result.0, StatusCode::SERVICE_UNAVAILABLE);
    let encoded = result.1.to_string();
    for forbidden in ["tenant-a", "tenant-b", &a1.worker_id, &b1.worker_id, &token] {
        assert!(!encoded.contains(forbidden));
    }
}

#[tokio::test]
async fn worker_proof_is_revalidated_after_a_slow_body_and_revocation_prevents_commit() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 1000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 1000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let token = "1".repeat(48);
    let peer = "2".repeat(48);
    let (credential, revision) = bind(&a, &a1, &token);
    bind(&a, &a2, &peer);
    let app = router(a.clone(), b);
    let grant = directed(&a1, &a2, 300);
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/grants",
            &"a".repeat(48),
            json!(grant)
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let payload = serde_json::to_vec(&send(
        &grant,
        "held request must be denied after revocation",
    ))
    .unwrap();
    let (polled, body_polled) = tokio::sync::oneshot::channel();
    let (release, body_release) = tokio::sync::oneshot::channel();
    let body = Body::from_stream(futures_util::stream::once(async move {
        let _ = polled.send(());
        body_release.await.unwrap();
        Ok::<_, std::io::Error>(axum::body::Bytes::from(payload))
    }));
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/relay/messages")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(body)
        .unwrap();
    let response = tokio::spawn(app.oneshot(request));
    body_polled.await.unwrap();
    a.revoke_workload_binding(&WorkloadBindingRevocationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: credential,
        expected_revision: revision,
    })
    .unwrap();
    release.send(()).unwrap();
    assert_eq!(
        response.await.unwrap().unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        rows(
            &directory.path().join("tenant-a.sqlite"),
            "SELECT COUNT(*) FROM relay_messages"
        ),
        0
    );
    assert!(a.integrity().unwrap().valid);
}
fn rows(path: &Path, query: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(query, [], |r| r.get(0))
        .unwrap()
}

#[tokio::test]
async fn inbox_pagination_obeys_encoded_envelope_and_authorized_cursor() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 5000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 5000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let token1 = "1".repeat(48);
    let token2 = "2".repeat(48);
    bind(&a, &a1, &token1);
    bind(&a, &a2, &token2);
    let app = router(a.clone(), b);
    let grant = directed(&a1, &a2, 300);
    assert!(
        call(
            &app,
            "POST",
            "/api/v1/relay/grants",
            &"a".repeat(48),
            json!(grant)
        )
        .await
        .0
        .is_success()
    );
    // Newlines are one UTF-8 byte but require two encoded JSON bytes.
    let body = "\n".repeat(MAX_MESSAGE_BYTES);
    for _ in 0..20 {
        assert!(
            call(
                &app,
                "POST",
                "/api/v1/relay/messages",
                &token1,
                json!(send(&grant, &body))
            )
            .await
            .0
            .is_success()
        );
    }
    let mut after = None;
    let mut seen = std::collections::BTreeSet::new();
    let mut pages = 0;
    loop {
        let path = after.map_or_else(
            || "/api/v1/relay/messages?limit=32".into(),
            |cursor| format!("/api/v1/relay/messages?limit=32&after={cursor}"),
        );
        let (status, inbox) = call(&app, "GET", &path, &token2, Value::Null).await;
        assert!(status.is_success());
        assert_eq!(inbox["version"], 1);
        assert!(serde_json::to_vec(&inbox).unwrap().len() <= MAX_INBOX_BYTES);
        let items = inbox["items"].as_array().unwrap();
        assert!(!items.is_empty());
        assert!(items.len() <= 32);
        for item in items {
            assert!(seen.insert(item["metadata"]["message_id"].as_str().unwrap().to_owned()));
            assert_eq!(item["body"], body);
        }
        pages += 1;
        assert!(pages <= 20, "pagination made no progress");
        if inbox["next_after"].is_null() {
            break;
        }
        after = Some(inbox["next_after"].as_i64().unwrap());
        assert_eq!(after, items.last().unwrap()["sequence"].as_i64());
    }
    assert!(pages >= 2, "encoded envelope bound was not exercised");
    assert_eq!(seen.len(), 20);
    let empty = call(
        &app,
        "GET",
        "/api/v1/relay/messages?limit=32",
        &token1,
        Value::Null,
    )
    .await;
    assert!(empty.0.is_success());
    assert_eq!(empty.1["items"], json!([]));
    assert!(empty.1["next_after"].is_null());
    assert!(a.integrity().unwrap().valid);
}

#[tokio::test]
async fn one_worker_cannot_spend_its_peers_or_controls_audit_budget_and_rotation_does_not_reset_usage()
 {
    let directory = support::private_tempdir();
    let a = Arc::new(
        fresh(directory.path(), "tenant-a", 1000)
            .with_workload_audit_policy(WorkloadAuditPolicyV1 {
                version: 1,
                tenant: AuditCapacityV1 {
                    events: 12,
                    bytes: 96 * 1024,
                },
                principal: AuditCapacityV1 {
                    events: 3,
                    bytes: 24 * 1024,
                },
            })
            .unwrap(),
    );
    let b = Arc::new(fresh(directory.path(), "tenant-b", 1000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let token1 = "1".repeat(48);
    let token2 = "2".repeat(48);
    let (credential, revision) = bind(&a, &a1, &token1);
    bind(&a, &a2, &token2);
    let app = router(a.clone(), b);
    let forward = directed(&a1, &a2, 300);
    let backward = directed(&a2, &a1, 300);
    for grant in [&forward, &backward] {
        assert!(
            call(
                &app,
                "POST",
                "/api/v1/relay/grants",
                &"a".repeat(48),
                json!(grant)
            )
            .await
            .0
            .is_success()
        );
    }
    for index in 0..3 {
        assert!(
            call(
                &app,
                "POST",
                "/api/v1/relay/messages",
                &token1,
                json!(send(&forward, &format!("bounded principal {index}")))
            )
            .await
            .0
            .is_success()
        );
    }
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token1,
            json!(send(&forward, "over principal audit budget"))
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        rows(
            &directory.path().join("tenant-a.sqlite"),
            "SELECT COUNT(*) FROM relay_messages"
        ),
        3
    );
    let peer = call(
        &app,
        "GET",
        "/api/v1/relay/messages?limit=32",
        &token2,
        Value::Null,
    )
    .await;
    assert!(peer.0.is_success());
    assert_eq!(peer.1["items"].as_array().unwrap().len(), 3);
    assert!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token2,
            json!(send(&backward, "peer still has its independent share"))
        )
        .await
        .0
        .is_success()
    );
    let rotated = "4".repeat(48);
    a.issue_workload_binding(&WorkloadBindingIssueV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: credential,
        expected_revision: revision,
        subject: WorkloadSubjectV1::Worker {
            worker_id: a1.worker_id,
            attempt_id: a1.attempt.attempt_id,
            generation: 1,
        },
        sha256_digest: hex::encode(Sha256::digest(rotated.as_bytes())),
        expires_at: (chrono::Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
    })
    .unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &rotated,
            json!(send(&forward, "rotation cannot recharge a stable worker"))
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    let revoke = RelayRevokeV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: forward.grant_id.clone(),
    };
    assert!(
        call(
            &app,
            "POST",
            &format!("/api/v1/relay/grants/{}/revoke", forward.grant_id),
            &"a".repeat(48),
            json!(revoke)
        )
        .await
        .0
        .is_success()
    );
    assert!(a.integrity().unwrap().valid);
}

#[tokio::test]
async fn recipient_ack_and_revocation_stop_delivery_and_do_not_restore_a_stale_capability() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 1000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 1000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let token1 = "1".repeat(48);
    let token2 = "2".repeat(48);
    let (credential, revision) = bind(&a, &a1, &token1);
    bind(&a, &a2, &token2);
    let app = router(a.clone(), b);
    let grant = directed(&a1, &a2, 300);
    assert!(
        call(
            &app,
            "POST",
            "/api/v1/relay/grants",
            &"a".repeat(48),
            json!(grant)
        )
        .await
        .0
        .is_success()
    );
    let payload = send(&grant, "acknowledge the business message");
    assert!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token1,
            json!(payload)
        )
        .await
        .0
        .is_success()
    );
    let ack = RelayAckV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        message_id: payload.operation_id.clone(),
    };
    let path = format!("/api/v1/relay/messages/{}/ack", payload.operation_id);
    assert_eq!(
        call(&app, "POST", &path, &token1, json!(ack)).await.0,
        StatusCode::FORBIDDEN,
        "sender acknowledged recipient delivery"
    );
    assert!(
        call(&app, "POST", &path, &token2, json!(ack))
            .await
            .0
            .is_success()
    );
    let replay = call(&app, "POST", &path, &token2, json!(ack)).await;
    assert!(replay.0.is_success());
    assert_eq!(replay.1["replayed"], true);
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/relay/messages?limit=32",
            &token2,
            Value::Null
        )
        .await
        .1["items"],
        json!([])
    );
    assert_eq!(
        rows(
            &directory.path().join("tenant-a.sqlite"),
            "SELECT COUNT(*) FROM relay_messages WHERE body IS NOT NULL"
        ),
        1,
        "ACK silently destroyed retained business content"
    );
    let queued = send(&grant, "pending before explicit grant revocation");
    assert!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token1,
            json!(queued)
        )
        .await
        .0
        .is_success()
    );
    let revoke = RelayRevokeV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: grant.grant_id.clone(),
    };
    let path = format!("/api/v1/relay/grants/{}/revoke", grant.grant_id);
    assert_eq!(
        call(&app, "POST", &path, &token1, json!(revoke)).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert!(
        call(&app, "POST", &path, &"a".repeat(48), json!(revoke))
            .await
            .0
            .is_success()
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/relay/messages?limit=32",
            &token2,
            Value::Null
        )
        .await
        .1["items"],
        json!([])
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token1,
            json!(send(&grant, "not authorized after revoke"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let operations = rows(
        &directory.path().join("tenant-a.sqlite"),
        "SELECT COUNT(*) FROM relay_operations",
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token1,
            json!(payload)
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "exact send receipt bypassed grant revocation"
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/relay/messages/{}/ack", ack.message_id),
            &token2,
            json!(ack)
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "exact ACK receipt bypassed grant revocation"
    );
    assert_eq!(
        rows(
            &directory.path().join("tenant-a.sqlite"),
            "SELECT COUNT(*) FROM relay_operations"
        ),
        operations
    );
    a.revoke_workload_binding(&WorkloadBindingRevocationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: credential,
        expected_revision: revision,
    })
    .unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token1,
            json!(payload)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED,
        "revoked worker reused an idempotent send as authorization"
    );
    assert!(a.integrity().unwrap().valid);
}

#[tokio::test]
async fn ttl_size_secret_detection_rate_limit_and_body_prune_are_enforced_by_the_real_store() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 2000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 2000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let token1 = "1".repeat(48);
    let token2 = "2".repeat(48);
    bind(&a, &a1, &token1);
    bind(&a, &a2, &token2);
    let app = router(a.clone(), b);
    let grant = directed(&a1, &a2, 300);
    assert!(
        call(
            &app,
            "POST",
            "/api/v1/relay/grants",
            &"a".repeat(48),
            json!(grant)
        )
        .await
        .0
        .is_success()
    );
    for body in [
        "x".repeat(MAX_MESSAGE_BYTES + 1),
        "fixture-private-provider-secret".into(),
        r#"{"api_key":"unknown-but-explicit-secret-field"}"#.into(),
    ] {
        assert_eq!(
            call(
                &app,
                "POST",
                "/api/v1/relay/messages",
                &token1,
                json!(send(&grant, &body))
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut invalid_ttl = send(&grant, "invalid TTL");
    invalid_ttl.ttl_seconds = MAX_MESSAGE_TTL_SECONDS + 1;
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token1,
            json!(invalid_ttl)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let mut expiring = send(&grant, "short lived message");
    expiring.ttl_seconds = 1;
    assert!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token1,
            json!(expiring)
        )
        .await
        .0
        .is_success()
    );
    let mut second_expiring = send(&grant, "second short lived message");
    second_expiring.ttl_seconds = 1;
    assert!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token1,
            json!(second_expiring)
        )
        .await
        .0
        .is_success()
    );
    let acknowledged = RelayAckV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        message_id: expiring.operation_id.clone(),
    };
    let ack_path = format!("/api/v1/relay/messages/{}/ack", acknowledged.message_id);
    assert!(
        call(&app, "POST", &ack_path, &token2, json!(acknowledged))
            .await
            .0
            .is_success()
    );
    tokio::time::sleep(std::time::Duration::from_millis(1150)).await;
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/relay/messages?limit=32",
            &token2,
            Value::Null
        )
        .await
        .1["items"],
        json!([])
    );
    let prune = RelayPruneV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        message_id: expiring.operation_id.clone(),
    };
    let path = format!("/api/v1/relay/messages/{}/prune", prune.message_id);
    assert_eq!(
        call(&app, "POST", &path, &token1, json!(prune)).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert!(
        call(&app, "POST", &path, &"a".repeat(48), json!(prune))
            .await
            .0
            .is_success()
    );
    assert_eq!(
        call(&app, "POST", &path, &"a".repeat(48), json!(prune))
            .await
            .1["replayed"],
        true
    );
    let divergent = RelayPruneV1 {
        message_id: second_expiring.operation_id.clone(),
        ..prune
    };
    let ack_replay = call(&app, "POST", &ack_path, &token2, json!(acknowledged)).await;
    assert!(ack_replay.0.is_success());
    assert_eq!(
        ack_replay.1["replayed"], true,
        "message expiration/pruning incorrectly revoked an otherwise active grant"
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/relay/messages/{}/prune", divergent.message_id),
            &"a".repeat(48),
            json!(divergent)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        rows(
            &directory.path().join("tenant-a.sqlite"),
            "SELECT COUNT(*) FROM relay_messages WHERE body IS NULL"
        ),
        1
    );
    for index in 2..MAX_MESSAGES_PER_MINUTE {
        assert!(
            call(
                &app,
                "POST",
                "/api/v1/relay/messages",
                &token1,
                json!(send(&grant, &format!("bounded business message {index}")))
            )
            .await
            .0
            .is_success()
        );
    }
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token1,
            json!(send(&grant, "over durable grant rate"))
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    let replay = call(
        &app,
        "POST",
        "/api/v1/relay/messages",
        &token1,
        json!(expiring),
    )
    .await;
    assert!(replay.0.is_success());
    assert_eq!(
        replay.1["replayed"], true,
        "an exact retry spent a second rate slot"
    );
    assert!(a.integrity().unwrap().valid);
}

#[tokio::test]
async fn exact_send_and_ack_receipts_require_an_unexpired_grant() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 1000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 1000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let token1 = "1".repeat(48);
    let token2 = "2".repeat(48);
    bind(&a, &a1, &token1);
    bind(&a, &a2, &token2);
    let app = router(a.clone(), b);
    let grant = directed(&a1, &a2, 2);
    assert!(
        call(
            &app,
            "POST",
            "/api/v1/relay/grants",
            &"a".repeat(48),
            json!(grant)
        )
        .await
        .0
        .is_success()
    );
    let payload = send(&grant, "already accepted before grant deadline");
    assert!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token1,
            json!(payload)
        )
        .await
        .0
        .is_success()
    );
    let ack = RelayAckV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        message_id: payload.operation_id.clone(),
    };
    let ack_path = format!("/api/v1/relay/messages/{}/ack", ack.message_id);
    assert!(
        call(&app, "POST", &ack_path, &token2, json!(ack))
            .await
            .0
            .is_success()
    );
    let operations = rows(
        &directory.path().join("tenant-a.sqlite"),
        "SELECT COUNT(*) FROM relay_operations",
    );
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token1,
            json!(payload)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "POST", &ack_path, &token2, json!(ack)).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        rows(
            &directory.path().join("tenant-a.sqlite"),
            "SELECT COUNT(*) FROM relay_operations"
        ),
        operations
    );
    assert!(a.integrity().unwrap().valid);
}

#[tokio::test]
async fn exact_receipts_require_the_opposite_endpoint_to_still_be_in_scope() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 1000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 1000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let token1 = "1".repeat(48);
    let token2 = "2".repeat(48);
    bind(&a, &a1, &token1);
    let (credential, revision) = bind(&a, &a2, &token2);
    let app = router(a.clone(), b);
    let forward = directed(&a1, &a2, 300);
    let reverse = directed(&a2, &a1, 300);
    for grant in [&forward, &reverse] {
        assert!(
            call(
                &app,
                "POST",
                "/api/v1/relay/grants",
                &"a".repeat(48),
                json!(grant)
            )
            .await
            .0
            .is_success()
        );
    }
    let first = send(&forward, "before destination revocation");
    let second = send(&reverse, "before source revocation");
    assert!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token1,
            json!(first)
        )
        .await
        .0
        .is_success()
    );
    assert!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token2,
            json!(second)
        )
        .await
        .0
        .is_success()
    );
    let ack = RelayAckV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        message_id: second.operation_id,
    };
    let ack_path = format!("/api/v1/relay/messages/{}/ack", ack.message_id);
    assert!(
        call(&app, "POST", &ack_path, &token1, json!(ack))
            .await
            .0
            .is_success()
    );
    a.revoke_workload_binding(&WorkloadBindingRevocationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: credential,
        expected_revision: revision,
    })
    .unwrap();
    let operations = rows(
        &directory.path().join("tenant-a.sqlite"),
        "SELECT COUNT(*) FROM relay_operations",
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token1,
            json!(first)
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "send replay ignored revoked destination binding"
    );
    assert_eq!(
        call(&app, "POST", &ack_path, &token1, json!(ack)).await.0,
        StatusCode::FORBIDDEN,
        "ACK replay ignored revoked source binding"
    );
    assert_eq!(
        rows(
            &directory.path().join("tenant-a.sqlite"),
            "SELECT COUNT(*) FROM relay_operations"
        ),
        operations
    );
    assert!(a.integrity().unwrap().valid);
}

#[tokio::test]
async fn audit_exhaustion_blocks_message_but_preserves_the_private_grant_revocation_path() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 256));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 256));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let token1 = "1".repeat(48);
    let token2 = "2".repeat(48);
    bind(&a, &a1, &token1);
    bind(&a, &a2, &token2);
    let app = router(a.clone(), b);
    let grant = directed(&a1, &a2, 300);
    assert!(
        call(
            &app,
            "POST",
            "/api/v1/relay/grants",
            &"a".repeat(48),
            json!(grant)
        )
        .await
        .0
        .is_success()
    );
    let mut exhausted = false;
    for _ in 0..512 {
        if a.append(agentsandbox::model::Event::new("ordinary.capacity.fixture"))
            .is_err()
        {
            exhausted = true;
            break;
        }
    }
    assert!(
        exhausted,
        "fixture did not exercise mandatory audit exhaustion"
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token1,
            json!(send(&grant, "must not persist without audit"))
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        rows(
            &directory.path().join("tenant-a.sqlite"),
            "SELECT COUNT(*) FROM relay_messages"
        ),
        0
    );
    let revoke = RelayRevokeV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        grant_id: grant.grant_id.clone(),
    };
    assert!(
        call(
            &app,
            "POST",
            &format!("/api/v1/relay/grants/{}/revoke", grant.grant_id),
            &"a".repeat(48),
            json!(revoke)
        )
        .await
        .0
        .is_success(),
        "ordinary worker traffic consumed revocation capacity"
    );
    assert!(a.integrity().unwrap().valid);
}

#[tokio::test]
async fn reopened_control_stays_inactive_and_a_tampered_body_latches_unhealthy_before_any_delivery()
{
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 1000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 1000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let token1 = "1".repeat(48);
    let token2 = "2".repeat(48);
    bind(&a, &a1, &token1);
    bind(&a, &a2, &token2);
    let app = router(a.clone(), b.clone());
    let grant = directed(&a1, &a2, 300);
    assert!(
        call(
            &app,
            "POST",
            "/api/v1/relay/grants",
            &"a".repeat(48),
            json!(grant)
        )
        .await
        .0
        .is_success()
    );
    let payload = send(&grant, "durable original business body");
    assert!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &token1,
            json!(payload)
        )
        .await
        .0
        .is_success()
    );
    drop(app);
    drop(a);
    drop(b);
    let path = directory.path().join("tenant-a.sqlite");
    let a = Arc::new(
        Store::open_existing_for_tenant(&path, "tenant-a", Redactor::new(vec![]), 1000).unwrap(),
    );
    let b = Arc::new(
        Store::open_existing_for_tenant(
            &directory.path().join("tenant-b.sqlite"),
            "tenant-b",
            Redactor::new(vec![]),
            1000,
        )
        .unwrap(),
    );
    assert!(!a.supervision_state().unwrap().active);
    assert!(a.healthy());
    assert!(a.integrity().unwrap().valid);
    let app = router(a.clone(), b);
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/relay/messages",
        &token1,
        json!(payload),
    )
    .await;
    assert!(
        !status.is_success(),
        "reopen silently restored execution/relay authority"
    );
    assert_eq!(rows(&path, "SELECT COUNT(*) FROM relay_messages"), 1);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "UPDATE relay_messages SET body='modified outside mandatory audit' WHERE message_id=?1",
        [payload.operation_id],
    )
    .unwrap();
    drop(db);
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/relay/messages?limit=32",
            &token2,
            Value::Null
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(
        !a.healthy(),
        "body hash mismatch did not latch the independent audit failure"
    );
}

#[tokio::test]
async fn schema7_preserves_directed_relay_and_workload_identity_boundaries() {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 10_000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 10_000));
    for (store, tenant) in [(&a, "tenant-a"), (&b, "tenant-b")] {
        store
            .migrate_human_brokers_v7(&directory.path().join(format!("{tenant}-before-v7.sqlite")))
            .unwrap();
        let state = store.supervision_state().unwrap();
        assert_eq!(state.schema_version, 7);
        assert!(state.active);
    }
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let b1 = running_worker(&b);
    let a1_token = "7".repeat(48);
    let a2_token = "8".repeat(48);
    let b1_token = "9".repeat(48);
    bind(&a, &a1, &a1_token);
    bind(&a, &a2, &a2_token);
    bind(&b, &b1, &b1_token);
    let app = router(a.clone(), b.clone());
    let before = (a.head().unwrap(), b.head().unwrap());
    assert_eq!(
        call(&app, "GET", "/api/v1/human/brokers", &a1_token, Value::Null)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        (a.head().unwrap(), b.head().unwrap()),
        before,
        "a bound worker proof cannot create a human broker read receipt"
    );

    let grant = directed(&a1, &a2, 300);
    assert!(
        call(
            &app,
            "POST",
            "/api/v1/relay/grants",
            &"a".repeat(48),
            json!(grant)
        )
        .await
        .0
        .is_success()
    );
    let before_cross = (a.head().unwrap(), b.head().unwrap());
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/grants",
            &"a".repeat(48),
            json!(directed(&a1, &b1, 300))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!((a.head().unwrap(), b.head().unwrap()), before_cross);
    let payload = send(&grant, "schema7-directed-business-body");
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &b1_token,
            json!(payload)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, first) = call(
        &app,
        "POST",
        "/api/v1/relay/messages",
        &a1_token,
        json!(payload),
    )
    .await;
    assert!(status.is_success());
    assert_eq!(first["replayed"], false);
    let after_first = a.head().unwrap();
    let (status, replay) = call(
        &app,
        "POST",
        "/api/v1/relay/messages",
        &a1_token,
        json!(payload),
    )
    .await;
    assert!(status.is_success());
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["audit_seq"], first["audit_seq"]);
    assert_eq!(
        a.head().unwrap(),
        after_first,
        "an exact send replay must not append again"
    );
    let (status, inbox) = call(
        &app,
        "GET",
        "/api/v1/relay/messages?limit=32",
        &a2_token,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(inbox["items"].as_array().unwrap().len(), 1);
    assert_eq!(inbox["items"][0]["body"], payload.body);
    let (status, foreign) = call(
        &app,
        "GET",
        "/api/v1/relay/messages?limit=32",
        &b1_token,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(foreign["items"], json!([]));
    assert!(!foreign.to_string().contains(&payload.body));
    assert_eq!(
        rows(
            &directory.path().join("tenant-a.sqlite"),
            "SELECT COUNT(*) FROM relay_messages"
        ),
        1
    );
    for store in [&a, &b] {
        let audit = store
            .events(&EventQuery {
                order: Some("asc".into()),
                limit: Some(1000),
                ..Default::default()
            })
            .unwrap();
        let encoded = serde_json::to_string(&audit).unwrap();
        for private in [&payload.body, &a1_token, &a2_token, &b1_token] {
            assert!(!encoded.contains(private));
        }
        assert!(store.integrity().unwrap().valid);
    }
}

#[tokio::test]
async fn directed_worker_messages_are_tenant_scoped_idempotent_and_do_not_duplicate_bodies_in_audit()
 {
    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 10_000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 10_000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let b1 = running_worker(&b);
    let a1_token = "1".repeat(48);
    let a2_token = "2".repeat(48);
    let b1_token = "3".repeat(48);
    bind(&a, &a1, &a1_token);
    bind(&a, &a2, &a2_token);
    bind(&b, &b1, &b1_token);
    let app = router(a.clone(), b.clone());
    let grant = directed(&a1, &a2, 300);
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/relay/grants",
        &"a".repeat(48),
        json!(grant),
    )
    .await;
    assert!(
        status.is_success(),
        "directed tenant-a grant refused: {status}"
    );
    let cross = directed(&a1, &b1, 300);
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/grants",
            &"a".repeat(48),
            json!(cross)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/grants",
            &"v".repeat(48),
            json!(directed(&a1, &a2, 300))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    for path in [
        "/api/v1/federation/workers",
        "/api/v1/workers",
        "/api/v1/quota",
        "/api/v1/events",
        "/api/v1/overview",
    ] {
        assert_eq!(
            call(&app, "GET", path, &a1_token, Value::Null).await.0,
            StatusCode::UNAUTHORIZED,
            "worker accessed {path}"
        );
    }
    let payload = send(&grant, "relay-business-canary-without-secret");
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &b1_token,
            json!(payload)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, first) = call(
        &app,
        "POST",
        "/api/v1/relay/messages",
        &a1_token,
        json!(payload),
    )
    .await;
    assert!(status.is_success(), "worker send refused: {status}");
    assert_eq!(first["replayed"], false);
    let (status, replay) = call(
        &app,
        "POST",
        "/api/v1/relay/messages",
        &a1_token,
        json!(payload),
    )
    .await;
    assert!(status.is_success());
    assert_eq!(replay["replayed"], true);
    assert_eq!(first["audit_seq"], replay["audit_seq"]);
    let mut changed = payload.clone();
    changed.body = "different business content".into();
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/relay/messages",
            &a1_token,
            json!(changed)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        rows(
            &directory.path().join("tenant-a.sqlite"),
            "SELECT COUNT(*) FROM relay_messages"
        ),
        1
    );
    let (status, inbox) = call(
        &app,
        "GET",
        "/api/v1/relay/messages?limit=32",
        &a2_token,
        Value::Null,
    )
    .await;
    assert!(status.is_success());
    assert_eq!(inbox["items"].as_array().unwrap().len(), 1);
    assert_eq!(inbox["items"][0]["body"], payload.body);
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/relay/messages?limit=32",
            &a1_token,
            Value::Null
        )
        .await
        .1["items"],
        json!([])
    );
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/relay/messages?limit=32",
            &b1_token,
            Value::Null
        )
        .await
        .1["items"],
        json!([])
    );
    let audit = a
        .events(&EventQuery {
            order: Some("asc".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(
        !serde_json::to_string(&audit)
            .unwrap()
            .contains(&payload.body),
        "audit duplicated the business body"
    );
    assert!(a.integrity().unwrap().valid);
    assert!(b.integrity().unwrap().valid);
}

async fn start_reopened_relay_gateway(
    directory: &Path,
) -> anyhow::Result<(
    tokio::process::Child,
    tokio::io::BufReader<tokio::process::ChildStderr>,
    String,
)> {
    use anyhow::Context;
    use std::{io::Write, os::unix::fs::OpenOptionsExt, process::Stdio, time::Duration};
    use tokio::io::{AsyncBufReadExt, BufReader};

    let mut tenants = Vec::new();
    for (tenant, runner, viewer, max_active_workers) in [("a", "r", "v", 2), ("b", "s", "w", 1)] {
        let runner_file = format!("{tenant}-runner");
        let viewer_file = format!("{tenant}-viewer");
        for (file, token) in [(&runner_file, runner), (&viewer_file, viewer)] {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(directory.join(file))?
                .write_all(token.repeat(48).as_bytes())?;
        }
        tenants.push(json!({
            "tenant_id": format!("tenant-{tenant}"),
            "data": format!("tenant-{tenant}.sqlite"),
            "runner_token_file": runner_file,
            "viewer_token_file": viewer_file,
            "worker_quota": QuotaPolicyV1 {max_active_workers, ..Default::default()},
        }));
    }
    let config = directory.join("reopened-tenants.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&json!({"version":1,"tenants":tenants}))?,
    )?;
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
        .args([
            "gateway",
            "--listen",
            "127.0.0.1:0",
            "--max-events",
            "10000",
            "--tenants-file",
        ])
        .arg(&config)
        .arg("--policy")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("policies/default.json"))
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stderr = BufReader::new(child.stderr.take().context("gateway stderr unavailable")?);
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(15), stderr.read_line(&mut line)).await??;
    let address: std::net::SocketAddr = line
        .strip_prefix("Gateway ")
        .and_then(|line| line.split_whitespace().next())
        .context("the gateway did not report readiness")?
        .parse()?;
    anyhow::ensure!(address.ip().is_loopback(), "gateway is not on loopback");
    Ok((child, stderr, format!("http://{address}")))
}

async fn read_reopened_gateway(
    http: &reqwest::Client,
    origin: &str,
    route: &str,
    token: &str,
) -> anyhow::Result<Value> {
    Ok(http
        .get(format!("{origin}/api/v1/{route}"))
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

#[tokio::test]
async fn reopened_gateway_keeps_three_worker_quotas_terminal_usage_and_directed_audit_separate()
-> anyhow::Result<()> {
    use std::time::Duration;

    let directory = support::private_tempdir();
    let a = Arc::new(fresh(directory.path(), "tenant-a", 10000));
    let b = Arc::new(fresh(directory.path(), "tenant-b", 10000));
    let a1 = running_worker(&a);
    let a2 = running_worker(&a);
    let b1 = running_worker(&b);
    let a1_token = "1".repeat(48);
    let a2_token = "2".repeat(48);
    bind(&a, &a1, &a1_token);
    bind(&a, &a2, &a2_token);
    bind(&b, &b1, &"3".repeat(48));

    // Synthetic runtime observations above populate the real Store. Use the
    // real HTTP handlers over TCP while those same stores are still active.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let warm_origin = format!("http://{}", listener.local_addr()?);
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    // Keep the gateway's redaction dictionary disjoint from fixture image
    // and manifest digests. The real terminal HTTP mutation checks both.
    let app = router_with_runner_tokens(a.clone(), b.clone(), ["r", "s"]);
    let warm_gateway = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let grant = directed(&a1, &a2, 300);
    let message = send(&grant, "cold-gateway relay canary body");
    let response = http
        .post(format!("{warm_origin}/api/v1/relay/grants"))
        .bearer_auth("r".repeat(48))
        .json(&grant)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        response.json::<RelayReceiptV1>().await?.resource_id,
        grant.grant_id
    );
    let relay_url = format!("{warm_origin}/api/v1/relay/messages");
    let response = http
        .post(&relay_url)
        .bearer_auth(&a1_token)
        .json(&message)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let sent: RelayReceiptV1 = response.json().await?;
    let response = http
        .post(&relay_url)
        .bearer_auth(&a1_token)
        .json(&message)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let replay: RelayReceiptV1 = response.json().await?;
    assert_eq!(
        replay,
        RelayReceiptV1 {
            replayed: true,
            ..sent.clone()
        }
    );
    let inbox: RelayInboxV1 = http
        .get(&relay_url)
        .bearer_auth(&a2_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(inbox.items.len(), 1);
    let received = &inbox.items[0];
    assert_eq!(received.metadata.message_id, sent.resource_id);
    assert_eq!(received.metadata.grant_id, grant.grant_id);
    assert_eq!(received.metadata.channel, grant.channel);
    assert_eq!(received.metadata.source, grant.source);
    assert_eq!(received.metadata.destination, grant.destination);
    assert_eq!(received.body, message.body);

    // Leave both A endpoints Running. Their prior exact send is otherwise
    // eligible, so the cold refusal exercises loss of the active authority.
    let mut observation = parent_observation(
        &b1,
        RuntimeStatusV1::Exited,
        false,
        Some("2026-10-30T12:00:03Z"),
        Some(0),
    );
    observation.observed_at = "2026-10-30T12:00:04Z".into();
    let terminal = mutation(&b1, WorkerTransitionV1::Observe { observation });
    let terminal_url = format!("{warm_origin}/api/v1/workers/{}/transitions", b1.worker_id);
    let response = http
        .post(&terminal_url)
        .bearer_auth("s".repeat(48))
        .json(&terminal)
        .send()
        .await?
        .error_for_status()?;
    let first_terminal: MutationResultV1 = response.json().await?;
    assert_eq!(first_terminal.worker.state, WorkerStateV1::Terminal);
    assert!(!first_terminal.worker.quota_held);
    let again: MutationResultV1 = http
        .post(&terminal_url)
        .bearer_auth("s".repeat(48))
        .json(&terminal)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(first_terminal.receipt, again.receipt);

    let audit_a = serde_json::to_value(a.integrity()?)?;
    let audit_b = serde_json::to_value(b.integrity()?)?;
    assert_eq!(audit_a["valid"], true);
    assert_eq!(audit_b["valid"], true);
    let _ = shutdown.send(());
    tokio::time::timeout(Duration::from_secs(15), warm_gateway).await???;
    drop(a);
    drop(b);

    let (mut child, _stderr, origin) = start_reopened_relay_gateway(directory.path()).await?;
    for (token, workers, held) in [
        ("v".repeat(48), vec![&a1, &a2], vec![&a1, &a2]),
        ("w".repeat(48), vec![&first_terminal.worker], vec![]),
    ] {
        let protocol = read_reopened_gateway(&http, &origin, "control/protocol", &token).await?;
        assert_eq!(protocol["schema_version"], 6);
        assert_eq!(protocol["active"], false);
        let page = read_reopened_gateway(&http, &origin, "workers", &token).await?;
        let actual: Vec<WorkerV1> = serde_json::from_value(page["items"].clone())?;
        assert_eq!(actual.len(), workers.len());
        for expected in workers {
            assert!(actual.iter().any(|row| row.worker_id == expected.worker_id
                && row.attempt.attempt_id == expected.attempt.attempt_id
                && row.state == expected.state
                && row.quota_held == expected.quota_held));
        }
        let quota: QuotaUsageV1 =
            serde_json::from_value(read_reopened_gateway(&http, &origin, "quota", &token).await?)?;
        assert_eq!(quota.policy.max_active_workers as usize, actual.len());
        assert_eq!(quota.held_workers as usize, held.len());
        assert_eq!(
            quota.held_cpu_millis,
            held.iter()
                .map(|worker| u64::from(worker.launch.cpu_millis))
                .sum::<u64>()
        );
        assert_eq!(
            quota.held_memory_bytes,
            held.iter()
                .map(|worker| worker.launch.memory_bytes)
                .sum::<u64>()
        );
    }
    assert_eq!(
        read_reopened_gateway(&http, &origin, "usage", &"v".repeat(48)).await?["items"],
        json!([])
    );
    let usage = read_reopened_gateway(&http, &origin, "usage", &"w".repeat(48)).await?;
    let allocations: Vec<AllocationV1> = serde_json::from_value(usage["items"].clone())?;
    assert_eq!(allocations.len(), 1);
    let allocation = &allocations[0];
    assert_eq!(allocation.worker_id, b1.worker_id);
    assert_eq!(allocation.attempt_id, b1.attempt.attempt_id);
    assert_eq!(allocation.terminal_observation_id, terminal.operation_id);
    assert_eq!(allocation.duration_ns, "3000000000");
    assert_eq!(allocation.source.authority_id, b1.launch.authority_id);
    assert_eq!(allocation.source.spec_hash, b1.spec_hash);
    assert_eq!(
        allocation.cpu_millicore_nanoseconds,
        (3_000_000_000_u128 * u128::from(b1.launch.cpu_millis)).to_string()
    );
    assert_eq!(
        allocation.memory_byte_nanoseconds,
        (3_000_000_000_u128 * u128::from(b1.launch.memory_bytes)).to_string()
    );
    assert!(!allocation.billable);

    let refused = http
        .post(format!("{origin}/api/v1/relay/messages"))
        .bearer_auth(&a1_token)
        .json(&message)
        .send()
        .await?;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        refused.json::<Value>().await?["error"]["code"],
        "workload_forbidden"
    );
    for (viewer, mut integrity) in [("v", audit_a), ("w", audit_b)] {
        let token = viewer.repeat(48);
        integrity["stored_events"] = integrity["event_count"].clone();
        integrity["retention"] = json!({
            "capability": false,
            "segments": 0,
            "archived_through_seq": 0,
            "reclaimed_events": 0,
            "reclaimed_bytes": 0,
            "last": null,
            "segment_list": [],
        });
        assert_eq!(
            read_reopened_gateway(&http, &origin, "integrity", &token).await?,
            integrity
        );
        let rows = read_reopened_gateway(&http, &origin, "export?format=json", &token).await?;
        assert_eq!(
            rows.as_array().expect("audit export rows").len() as u64,
            integrity["event_count"].as_u64().expect("audit count")
        );
        assert_eq!(
            rows.as_array()
                .and_then(|rows| rows.last())
                .map(|row| &row["hash"]),
            Some(&integrity["head_hash"])
        );
        let relay_rows = rows
            .as_array()
            .expect("audit export rows")
            .iter()
            .filter(|row| {
                row["kind"] == "control.relay.mutation"
                    && row["data"]["operation_id"] == message.operation_id
            })
            .collect::<Vec<_>>();
        if viewer == "v" {
            assert_eq!(relay_rows.len(), 1);
            assert_eq!(relay_rows[0]["seq"], sent.audit_seq);
            assert_eq!(relay_rows[0]["hash"], sent.audit_hash);
            assert_eq!(
                relay_rows[0]["data"]["resource"]["metadata"],
                serde_json::to_value(&received.metadata)?
            );
        } else {
            assert!(relay_rows.is_empty());
        }
        let exported = serde_json::to_string(&rows)?;
        assert!(!exported.contains(&message.body));
        for private in [a1_token.as_str(), a2_token.as_str()] {
            assert!(!exported.contains(private));
        }
        let hidden_workers = if viewer == "v" {
            vec![&b1]
        } else {
            vec![&a1, &a2]
        };
        for hidden in hidden_workers {
            assert!(!exported.contains(&hidden.worker_id));
        }
    }
    child.kill().await?;
    assert_eq!(
        rows(
            &directory.path().join("tenant-a.sqlite"),
            "SELECT COUNT(*) FROM relay_messages"
        ),
        1
    );
    assert_eq!(
        rows(
            &directory.path().join("tenant-b.sqlite"),
            "SELECT COUNT(*) FROM relay_messages"
        ),
        0
    );
    Ok(())
}
