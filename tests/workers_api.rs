//! Real HTTP/SQLite control contracts. These tests do not qualify a runtime.
mod support;

use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    control::*,
    model::{CommandSpec, DecisionRequest},
    policy::Policy,
    redaction::Redactor,
};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::sync::Arc;

#[test]
fn nested_command_contract_rejects_unknown_fields() {
    assert!(
        serde_json::from_value::<CommandSpec>(
            json!({"argv":["id"],"cwd":"/workspace","host_command":"unexpected"})
        )
        .is_err()
    );
}

struct Server {
    base: String,
    a: Arc<Store>,
    b: Arc<Store>,
    task: tokio::task::JoinHandle<()>,
    _files: tempfile::TempDir,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server() -> Server {
    let files = support::private_tempdir();
    let a = Arc::new(
        Store::open_for_tenant(
            &files.path().join("a.sqlite"),
            "a",
            Redactor::new(vec![]),
            1000,
        )
        .unwrap(),
    );
    let b = Arc::new(
        Store::open_for_tenant(
            &files.path().join("b.sqlite"),
            "b",
            Redactor::new(vec![]),
            1000,
        )
        .unwrap(),
    );
    let state = GatewayState::for_tenants(
        vec![
            TenantGateway {
                tenant_id: "a".into(),
                store: a.clone(),
                runner_token: "a".repeat(48),
                viewer_token: "v".repeat(48),
            },
            TenantGateway {
                tenant_id: "b".into(),
                store: b.clone(),
                runner_token: "b".repeat(48),
                viewer_token: "w".repeat(48),
            },
        ],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task =
        tokio::spawn(async move { axum::serve(listener, gateway_router(state)).await.unwrap() });
    Server {
        base,
        a,
        b,
        task,
        _files: files,
    }
}
async fn admission(server: &Server) -> AdmissionV1 {
    admission_for(server, "a").await
}
async fn admission_for(server: &Server, tenant: &str) -> AdmissionV1 {
    let (store, token) = if tenant == "a" {
        (&server.a, "a")
    } else {
        (&server.b, "b")
    };
    let session = uuid::Uuid::new_v4().to_string();
    let command = CommandSpec {
        argv: vec!["/usr/bin/id".into(), "-u".into()],
        cwd: "/workspace".into(),
    };
    let request = DecisionRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        profile: "observe".into(),
        command: command.clone(),
        session_id: Some(session.clone()),
        sandbox_id: Some(session.clone()),
        project_id: None,
        agent_id: None,
        correlation_id: None,
        trace_id: None,
        human_approved: false,
        approval_hash: None,
        execution_mode: None,
        provider_resolution: None,
    };
    let reply = Client::new()
        .post(format!("{}/api/v1/decide", server.base))
        .bearer_auth(token.repeat(48))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::OK);
    let reply: Value = reply.json().await.unwrap();
    let decision = store
        .events(&EventQuery {
            kind: Some("security.decision".into()),
            ..Default::default()
        })
        .unwrap()
        .into_iter()
        .find(|event| event.seq == reply["audit_seq"].as_i64().unwrap())
        .unwrap();
    AdmissionV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        decision_request: request,
        receipt: DecisionReceiptV1 {
            version: 1,
            request_id: decision.event.data["request_id"].as_str().unwrap().into(),
            request_hash: reply["request_hash"].as_str().unwrap().into(),
            phase: DecisionPhaseV1::Initial,
            audit_seq: decision.seq,
            audit_hash: decision.hash,
        },
        launch: LaunchSpecV1 {
            version: 1,
            session_id: session,
            command,
            profile: "observe".into(),
            runtime_kind: RuntimeKindV1::Docker,
            authority_id: uuid::Uuid::new_v4().to_string(),
            image_ref: "worker:fixture".into(),
            image_id: format!("sha256:{}", "d".repeat(64)),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1024 * 1024 * 1024,
            pids_limit: 256,
            timeout_seconds: 60,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: RestartPolicyV1::No,
            workspace_manifest_digest: "e".repeat(64),
            workspace_import_policy_version: 1,
            provider: "generic".into(),
            capture_output: false,
            credential_binding_id: None,
            credential_names: vec![],
            provider_resolution: None,
        },
    }
}

async fn admit_with_authority(
    server: &Server,
    tenant: &str,
    label: &str,
    authority: &str,
) -> WorkerV1 {
    let mut request = admission_for(server, tenant).await;
    request.launch.authority_id = authority.to_owned();
    let token = if tenant == "a" { "a" } else { "b" };
    let response = Client::new()
        .post(format!("{}/api/v1/workers/reservations", server.base))
        .bearer_auth(token.repeat(48))
        .json(&request)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{label} reservation returned {status}: {body}"
    );
    serde_json::from_str::<AdmissionResultV1>(&body)
        .unwrap()
        .worker
        .unwrap()
}

#[tokio::test]
async fn security_presets_are_bound_to_admissions_and_created_inspections() {
    let server = server().await;
    let http = Client::new();
    let endpoint = format!("{}/api/v1/workers/reservations", server.base);
    let mut legacy = admission(&server).await;
    legacy.launch.security_preset = SecurityPresetV1::FixedSandboxV1;
    let current = admission(&server).await;
    let mut admitted = Vec::new();
    for request in [&legacy, &current] {
        let response = http
            .post(&endpoint)
            .bearer_auth("a".repeat(48))
            .json(request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let result: AdmissionResultV1 = response.json().await.unwrap();
        let worker = result.worker.unwrap();
        assert_eq!(
            worker.launch.security_preset,
            request.launch.security_preset
        );
        assert_eq!(worker.spec_hash, request.launch.spec_hash().unwrap());
        admitted.push(worker);
    }
    let head = server.a.head().unwrap();
    let mut replacement = legacy.clone();
    replacement.launch.security_preset = current.launch.security_preset;
    let conflict = http
        .post(&endpoint)
        .bearer_auth("a".repeat(48))
        .json(&replacement)
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let mut unknown = serde_json::to_value(&current).unwrap();
    unknown["launch"]["security_preset"] = json!("fixed_sandbox_v3");
    let invalid = http
        .post(&endpoint)
        .bearer_auth("a".repeat(48))
        .json(&unknown)
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let replay = http
        .post(&endpoint)
        .bearer_auth("a".repeat(48))
        .json(&legacy)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let replay: AdmissionResultV1 = replay.json().await.unwrap();
    assert!(replay.replayed);
    assert_eq!(
        replay.worker.unwrap().launch.security_preset,
        SecurityPresetV1::FixedSandboxV1
    );
    assert_eq!(server.a.head().unwrap(), head);

    let mut worker = admitted.pop().unwrap();
    let transitions = format!(
        "{}/api/v1/workers/{}/transitions",
        server.base, worker.worker_id
    );
    for transition in [
        WorkerTransitionV1::Prepare,
        WorkerTransitionV1::CreateIssued,
    ] {
        let change = WorkerMutationV1 {
            version: 1,
            operation_id: uuid::Uuid::new_v4().to_string(),
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            expected_revision: worker.revision,
            transition,
        };
        let response = http
            .post(&transitions)
            .bearer_auth("a".repeat(48))
            .json(&change)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        worker = response.json::<MutationResultV1>().await.unwrap().worker;
    }
    let inspection = RuntimeInspectionV1 {
        authority_id: worker.launch.authority_id.clone(),
        runtime_kind: worker.launch.runtime_kind,
        container_id: "c".repeat(64),
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
    };
    let mut mismatched = inspection.clone();
    mismatched.security_preset = SecurityPresetV1::FixedSandboxV1;
    let change = |inspection| WorkerMutationV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition: WorkerTransitionV1::Created { inspection },
    };
    let before_rejected_inspection = server.a.head().unwrap();
    let rejected = http
        .post(&transitions)
        .bearer_auth("a".repeat(48))
        .json(&change(mismatched))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    assert_eq!(server.a.head().unwrap(), before_rejected_inspection);
    let response = http
        .post(&transitions)
        .bearer_auth("a".repeat(48))
        .json(&change(inspection.clone()))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let created = response.json::<MutationResultV1>().await.unwrap().worker;
    assert_eq!(created.attempt.created_inspection, Some(inspection));
    assert_eq!(
        created.launch.security_preset,
        current.launch.security_preset
    );
    assert!(server.a.integrity().unwrap().valid);
}

#[tokio::test]
async fn worker_api_enforces_tenant_roles_and_reservation_quota() {
    let server = server().await;
    let http = Client::new();
    let mut workers = Vec::new();
    for index in 0..4 {
        let admission = admission(&server).await;
        let endpoint = format!("{}/api/v1/workers/reservations", server.base);
        if index == 0 {
            assert_eq!(
                http.post(&endpoint)
                    .json(&admission)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                http.post(&endpoint)
                    .bearer_auth("v".repeat(48))
                    .json(&admission)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
        let response = http
            .post(&endpoint)
            .bearer_auth("a".repeat(48))
            .header("x-tenant-id", "b")
            .json(&admission)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if index < 3 {
                StatusCode::CREATED
            } else {
                StatusCode::TOO_MANY_REQUESTS
            }
        );
        let body: AdmissionResultV1 = response.json().await.unwrap();
        if index < 3 {
            let worker = body.worker.unwrap();
            assert_eq!(
                worker.launch.security_preset,
                admission.launch.security_preset
            );
            assert!(worker.quota_held);
            assert_eq!(worker.state, WorkerStateV1::Reserved);
            let replay = http
                .post(&endpoint)
                .bearer_auth("a".repeat(48))
                .json(&admission)
                .send()
                .await
                .unwrap();
            assert_eq!(replay.status(), StatusCode::OK);
            assert_eq!(
                replay
                    .json::<AdmissionResultV1>()
                    .await
                    .unwrap()
                    .worker
                    .unwrap()
                    .worker_id,
                worker.worker_id
            );
            workers.push(worker);
        } else {
            assert_eq!(body.refusal, Some(AdmissionRefusalV1::Quota));
        }
    }
    let quota: QuotaUsageV1 = http
        .get(format!("{}/api/quota", server.base))
        .bearer_auth("v".repeat(48))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(quota.held_workers, 3);
    assert_eq!(quota.held_memory_bytes, 3 * 1024 * 1024 * 1024);
    let b: Value = http
        .get(format!("{}/api/workers", server.base))
        .bearer_auth("w".repeat(48))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(b["items"], json!([]));
    assert_eq!(
        http.get(format!(
            "{}/api/workers/{}",
            server.base, workers[0].worker_id
        ))
        .bearer_auth("w".repeat(48))
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::NOT_FOUND
    );
    let first: Value = http
        .get(format!("{}/api/workers?limit=1", server.base))
        .bearer_auth("v".repeat(48))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert!(first["next_cursor"].is_string());
    let unknown = http
        .get(format!("{}/api/workers?tenant_id=b", server.base))
        .bearer_auth("v".repeat(48))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        http.get(format!("{}/api/workers?limit=0", server.base))
            .bearer_auth("v".repeat(48))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let mutation = WorkerMutationV1 {
        version: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        worker_id: workers[0].worker_id.clone(),
        attempt_id: workers[0].attempt.attempt_id.clone(),
        expected_revision: workers[0].revision,
        transition: WorkerTransitionV1::Prepare,
    };
    let path = format!(
        "{}/api/workers/{}/transitions",
        server.base, workers[0].worker_id
    );
    assert_eq!(
        http.post(&path)
            .bearer_auth("v".repeat(48))
            .json(&mutation)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        http.post(&path)
            .bearer_auth("a".repeat(48))
            .json(&mutation)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let mut stale = mutation.clone();
    stale.operation_id = uuid::Uuid::new_v4().to_string();
    let response = http
        .post(&path)
        .bearer_auth("a".repeat(48))
        .json(&stale)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response.json::<Value>().await.unwrap()["code"],
        "worker_conflict"
    );
    assert_eq!(
        http.post(&path)
            .bearer_auth("b".repeat(48))
            .json(&stale)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert!(server.a.integrity().unwrap().valid);
}

#[tokio::test]
async fn decision_response_supplies_the_exact_receipt_required_by_admission() {
    let server = server().await;
    let request = admission(&server).await.decision_request;
    let reply: Value = Client::new()
        .post(format!("{}/api/decide", server.base))
        .bearer_auth("a".repeat(48))
        .json(&request)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let receipt: DecisionReceiptV1 = serde_json::from_value(reply["receipt"].clone())
        .expect("complete durable receipt required");
    assert_eq!(receipt.request_id, request.request_id);
    assert_eq!(receipt.audit_seq, reply["audit_seq"]);
    assert_eq!(receipt.audit_hash, reply["audit_hash"]);
    assert_eq!(receipt.phase, DecisionPhaseV1::Initial);
}

#[tokio::test]
async fn dashboard_refuses_a_control_token_even_if_its_file_was_misconfigured() {
    use tower::ServiceExt;
    let backend = server().await;
    let front = support::private_tempdir();
    let dashboard = agentsandbox::api::dashboard_router(
        &backend.base,
        "a".repeat(48),
        front.path().to_path_buf(),
    )
    .unwrap();
    let response = dashboard
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/v1/quota")
                .header("authorization", format!("Bearer {}", "a".repeat(48)))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "the presentation plane accepts viewer identities only"
    );
}

#[tokio::test]
async fn shared_dashboard_uses_the_authenticated_viewer_scope_and_remains_read_only() {
    use tower::ServiceExt;
    let backend = server().await;
    let authority_a1 = uuid::Uuid::new_v4().to_string();
    let authority_a2 = uuid::Uuid::new_v4().to_string();
    let authority_b1 = uuid::Uuid::new_v4().to_string();
    let worker_a1 = admit_with_authority(&backend, "a", "A1", &authority_a1).await;
    let worker_a2 = admit_with_authority(&backend, "a", "A2", &authority_a2).await;
    let worker_b1 = admit_with_authority(&backend, "b", "B1", &authority_b1).await;
    let front = support::private_tempdir();
    let dashboard =
        agentsandbox::api::dashboard_shared_router(&backend.base, front.path().to_path_buf())
            .unwrap();
    for (token, tenant, active) in [("v", "a", 2), ("w", "b", 1)] {
        let response = dashboard
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/identity")
                    .header("authorization", format!("Bearer {}", token.repeat(48)))
                    .header("x-tenant-id", if tenant == "a" { "b" } else { "a" })
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let identity: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(identity, json!({"tenant_id":tenant,"role":"viewer"}));
        let response = dashboard
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/workers")
                    .header("authorization", format!("Bearer {}", token.repeat(48)))
                    .header("x-tenant-id", if tenant == "a" { "b" } else { "a" })
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let page: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 65536)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(page["items"].as_array().unwrap().len(), active);
        if tenant == "a" {
            let rows = page["items"].as_array().unwrap();
            let authorities = rows
                .iter()
                .map(|row| row["launch"]["authority_id"].as_str().unwrap())
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(
                authorities,
                std::collections::BTreeSet::from([authority_a1.as_str(), authority_a2.as_str(),])
            );
            assert!(rows.iter().all(|row| row["last_observation"].is_null()));
            assert!(
                rows.iter()
                    .any(|row| row["worker_id"] == worker_a1.worker_id)
            );
            assert!(
                rows.iter()
                    .any(|row| row["worker_id"] == worker_a2.worker_id)
            );
        } else {
            let rows = page["items"].as_array().unwrap();
            assert_eq!(rows[0]["worker_id"], worker_b1.worker_id);
            assert_eq!(rows[0]["launch"]["authority_id"], authority_b1);
            assert!(rows[0]["last_observation"].is_null());
        }
    }
    for (method, path, token, expected) in [
        (
            "GET",
            "/api/v1/quota".into(),
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            "GET",
            "/api/v1/quota".into(),
            Some("a"),
            StatusCode::FORBIDDEN,
        ),
        (
            "GET",
            "/api/v1/quota".into(),
            Some("x"),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "GET",
            format!("/api/v1/workers/{}", worker_a1.worker_id),
            Some("w"),
            StatusCode::NOT_FOUND,
        ),
        (
            "GET",
            "/api/v1/workers/operations/unknown".into(),
            Some("v"),
            StatusCode::NOT_FOUND,
        ),
        (
            "POST",
            "/api/v1/workers/reservations".into(),
            Some("v"),
            StatusCode::METHOD_NOT_ALLOWED,
        ),
    ] {
        let mut request = axum::http::Request::builder().method(method).uri(path);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {}", token.repeat(48)));
        }
        assert_eq!(
            dashboard
                .clone()
                .oneshot(request.body(axum::body::Body::empty()).unwrap())
                .await
                .unwrap()
                .status(),
            expected
        );
    }
    for (token, worker_id) in [("v", &worker_b1.worker_id), ("w", &worker_a1.worker_id)] {
        assert_eq!(
            dashboard
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri(format!("/api/v1/workers/{worker_id}"))
                        .header("authorization", format!("Bearer {}", token.repeat(48)))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND,
            "a viewer cannot fetch a worker from another tenant"
        );
    }
    // A single-viewer deployment preserves its explicit configured scope.
    let fixed = agentsandbox::api::dashboard_router(
        &backend.base,
        "v".repeat(48),
        front.path().to_path_buf(),
    )
    .unwrap();
    assert_eq!(
        fixed
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/identity")
                    .header("authorization", format!("Bearer {}", "w".repeat(48)))
                    .body(axum::body::Body::empty())
                    .unwrap()
            )
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        backend.a.workers().unwrap().len(),
        2,
        "presentation cannot mutate the registry"
    );
}
