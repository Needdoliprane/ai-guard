//! Real client/router/SQLite contracts, without engine or provider execution.
use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    control::{
        AdmissionV1, DecisionReceiptV1, LaunchSpecV1, MutationResultV1, NetworkV1, RestartPolicyV1,
        RuntimeKindV1, SecurityPresetV1, SupervisedAdmissionV1, SupervisionProtocolV1,
        WorkerMutationV1, WorkerStateV1, WorkerTransitionV1, WorkerV1,
    },
    model::Event,
    policy::Policy,
};
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{Method, StatusCode},
    middleware::{self, Next},
};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
};

struct Gateway {
    _directory: tempfile::TempDir,
    store: Arc<Store>,
    seen: Arc<Mutex<Vec<(Method, String)>>>,
    client: AuditClient,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Gateway {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Gateway {
    async fn new(refuse_control: Option<StatusCode>) -> Self {
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let store = Arc::new(
            Store::create_supervised_for_tenant(
                &directory.path().join("tenant.sqlite"),
                "fixture-tenant",
                Redactor::new(vec![]),
                1000,
                &directory.path().join("before-supervision.sqlite"),
            )
            .unwrap(),
        );
        let state = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: "fixture-tenant".into(),
                store: store.clone(),
                runner_token: "fixture-control-client-runner".into(),
                viewer_token: "fixture-control-client-viewer".into(),
            }],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = seen.clone();
        let app = gateway_router(state).layer(middleware::from_fn(
            move |request: Request, next: Next| {
                let recorded = recorded.clone();
                async move {
                    let path = request.uri().path().to_owned();
                    recorded
                        .lock()
                        .unwrap()
                        .push((request.method().clone(), path.clone()));
                    if path.starts_with("/control/")
                        && let Some(status) = refuse_control
                    {
                        return axum::http::Response::builder()
                            .status(status)
                            .body(Body::from("{}"))
                            .unwrap();
                    }
                    // Preserve the exact body through the real Rust handler.
                    let (parts, body) = request.into_parts();
                    let bytes = to_bytes(body, 262_144).await.unwrap();
                    next.run(Request::from_parts(parts, Body::from(bytes)))
                        .await
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = AuditClient::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            "fixture-control-client-runner",
        )
        .unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            _directory: directory,
            store,
            seen,
            client,
            task,
        }
    }
}

fn decision_request() -> DecisionRequest {
    let session = Uuid::new_v4().to_string();
    DecisionRequest {
        request_id: Uuid::new_v4().to_string(),
        profile: "observe".into(),
        command: CommandSpec {
            argv: vec!["/usr/bin/id".into(), "-u".into()],
            cwd: "/workspace".into(),
        },
        project_id: None,
        sandbox_id: Some(session.clone()),
        agent_id: None,
        session_id: Some(session),
        correlation_id: None,
        trace_id: None,
        human_approved: false,
        approval_hash: None,
        execution_mode: None,
        provider_resolution: None,
    }
}

fn supervised(request: DecisionRequest, receipt: DecisionReceiptV1) -> SupervisedAdmissionV1 {
    SupervisedAdmissionV1 {
        version: 1,
        protocol: SupervisionProtocolV1::BrokerSupervisionV1,
        intent_hash: "c".repeat(64),
        admission: AdmissionV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            launch: LaunchSpecV1 {
                version: 1,
                session_id: request.session_id.clone().unwrap(),
                command: request.command.clone(),
                profile: request.profile.clone(),
                runtime_kind: RuntimeKindV1::Docker,
                authority_id: Uuid::new_v4().to_string(),
                image_ref: "worker:fixture".into(),
                image_id: format!("sha256:{}", "a".repeat(64)),
                network: NetworkV1::None,
                cpu_millis: 2000,
                memory_bytes: 1_073_741_824,
                pids_limit: 256,
                timeout_seconds: 600,
                security_preset: SecurityPresetV1::FixedSandboxV2,
                restart_policy: RestartPolicyV1::No,
                workspace_manifest_digest: "b".repeat(64),
                workspace_import_policy_version: 1,
                provider: "generic".into(),
                capture_output: false,
                credential_binding_id: None,
                credential_names: vec![],
                provider_resolution: None,
            },
            decision_request: request,
            receipt,
        },
    }
}

#[tokio::test]
async fn trusted_clients_choose_control_while_generic_reads_and_capture_stay_public() {
    // Break caught: one trusted method accidentally rejoins public admission,
    // or worker-controlled event metadata selects the reserved transport.
    let gateway = Gateway::new(None).await;
    let client = &gateway.client;
    let protocol = client.control_supervision_identity().await.unwrap();
    protocol.validate().unwrap();
    let identity: Value = client.get_typed("api/v1/identity").await.unwrap();
    assert_eq!(identity["tenant_id"], "fixture-tenant");
    let request = decision_request();
    let decision = client.decide(&request).await.unwrap();
    let reservation = supervised(
        request,
        serde_json::from_value(decision["receipt"].clone()).unwrap(),
    );
    let admitted = client.admit_supervised(&reservation).await.unwrap();
    let worker = admitted.worker.unwrap();
    assert_eq!(worker.state, WorkerStateV1::Reserved);
    assert_eq!(
        gateway
            .store
            .worker_operation(&reservation.admission.operation_id)
            .unwrap(),
        Some(admitted.receipt)
    );
    let read_path = format!("api/v1/workers/{}", worker.worker_id);
    let read: WorkerV1 = client.get_typed(&read_path).await.unwrap();
    assert_eq!(read.worker_id, worker.worker_id);
    let mutation = WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition: WorkerTransitionV1::Prepare,
    };
    let write_path = format!("api/v1/workers/{}/transitions", worker.worker_id);
    let changed: MutationResultV1 = client.post_control(&write_path, &mutation).await.unwrap();
    assert_eq!(changed.worker.state, WorkerStateV1::Preparing);
    assert_eq!(
        gateway
            .store
            .worker_operation(&mutation.operation_id)
            .unwrap(),
        Some(changed.receipt)
    );
    client.get_json("api/integrity").await.unwrap();
    let mut capture = json!(Event::new("command.started"));
    capture["data"] = json!({"source":"control","provenance":"observed"});
    let capture_id = capture["id"].clone();
    client.post_event(capture).await.unwrap();
    assert!(
        gateway
            .store
            .events(&EventQuery::default())
            .unwrap()
            .iter()
            .any(|row| row.event.id == capture_id.as_str().unwrap())
    );
    assert_eq!(
        *gateway.seen.lock().unwrap(),
        vec![
            (Method::GET, "/control/api/v1/control/protocol".into()),
            (Method::GET, "/control/api/v1/identity".into()),
            (Method::POST, "/control/api/decide".into()),
            (
                Method::POST,
                "/control/api/v1/workers/supervised-reservations".into(),
            ),
            (Method::GET, format!("/control/{read_path}")),
            (Method::POST, format!("/control/{write_path}")),
            (Method::GET, "/api/integrity".into()),
            (Method::POST, "/api/events".into()),
        ]
    );
}

#[test]
fn control_url_rejects_nonroutes_methods_and_normalization_before_joining() {
    let client = AuditClient::new("http://127.0.0.1:8787", "synthetic-token").unwrap();
    for (method, path) in [
        (Method::GET, "api/integrity"),
        (Method::POST, "api/v1/events"),
        (Method::GET, "api/v1/decide"),
        (Method::GET, "api/v1/brokers/identity"),
        (Method::GET, "control/api/v1/identity"),
        (Method::GET, "/control/api/v1/identity"),
        (Method::GET, "api/v1/workers/x/../identity"),
        (Method::GET, "api/v1/workers/%2e%2e"),
        (Method::GET, "api/v1/identity?tenant_id=other"),
        (Method::GET, "//other.example/api/v1/identity"),
        (Method::GET, "https://other.example/api/v1/identity"),
    ] {
        assert!(client.control_url(method, path).is_err(), "{path}");
    }
    assert_eq!(
        client
            .control_url(Method::GET, "/api/v1/identity")
            .unwrap()
            .as_str(),
        "http://127.0.0.1:8787/control/api/v1/identity"
    );
}

#[tokio::test]
async fn trusted_client_refusal_never_retries_on_the_public_transport() {
    for status in [
        StatusCode::NOT_FOUND,
        StatusCode::SERVICE_UNAVAILABLE,
        StatusCode::TEMPORARY_REDIRECT,
    ] {
        let gateway = Gateway::new(Some(status)).await;
        let before = gateway.store.head().unwrap();
        assert!(gateway.client.control_supervision_identity().await.is_err());
        assert!(
            gateway
                .client
                .get_typed::<Value>("api/v1/identity")
                .await
                .is_err()
        );
        let request = decision_request();
        assert!(gateway.client.decide(&request).await.is_err());
        assert!(
            gateway
                .client
                .post_control::<Value>("api/v1/workers/reservations", &json!({}))
                .await
                .is_err()
        );
        assert_eq!(gateway.store.head().unwrap(), before);
        assert_eq!(
            *gateway.seen.lock().unwrap(),
            vec![
                (Method::GET, "/control/api/v1/control/protocol".into()),
                (Method::GET, "/control/api/v1/identity".into()),
                (Method::POST, "/control/api/decide".into()),
                (Method::POST, "/control/api/v1/workers/reservations".into()),
            ]
        );
    }
}

#[tokio::test]
async fn control_capabilities_never_fall_back_to_the_public_transport_or_protocol() {
    for status in [
        StatusCode::NOT_FOUND,
        StatusCode::SERVICE_UNAVAILABLE,
        StatusCode::FORBIDDEN,
        StatusCode::TEMPORARY_REDIRECT,
    ] {
        let gateway = Gateway::new(Some(status)).await;
        let before = gateway.store.head().unwrap();
        assert!(gateway.client.control_capabilities().await.is_err());
        assert_eq!(gateway.store.head().unwrap(), before);
        assert_eq!(
            *gateway.seen.lock().unwrap(),
            vec![(Method::GET, "/control/api/v1/control/capabilities".into())],
            "a refusal must end the request without another route"
        );
    }
}

#[tokio::test]
async fn control_capabilities_bind_the_runner_tenant_and_namespace_on_sqlite() {
    use crate::control::{ControlOperationV1, ControlStorageBackendV1};
    let gateway = Gateway::new(None).await;
    let before = gateway.store.head().unwrap();
    let capabilities = gateway.client.control_capabilities().await.unwrap();
    let identity = gateway.client.control_supervision_identity().await.unwrap();
    assert_eq!(capabilities.role, "runner");
    assert_eq!(capabilities.tenant_id, "fixture-tenant");
    assert_eq!(capabilities.storage, ControlStorageBackendV1::Sqlite);
    assert_eq!(
        capabilities.control_namespace_uuid,
        identity.control_namespace_uuid
    );
    assert_eq!(
        capabilities.operations,
        [
            ControlOperationV1::RecoveryInspect,
            ControlOperationV1::StopReconcile,
            ControlOperationV1::SupervisedAdmission,
        ]
    );
    let viewer = AuditClient::new(
        gateway.client.base.as_str(),
        "fixture-control-client-viewer",
    )
    .unwrap();
    assert_eq!(
        viewer.control_capabilities().await.unwrap_err(),
        ControlIdentityError::Forbidden
    );
    assert_eq!(gateway.store.head().unwrap(), before);
}

#[tokio::test]
async fn public_supervision_diagnostic_keeps_viewer_permissions_separate_from_control_identity() {
    // Break caught: moving the existing read diagnostic onto Runner-only
    // capacity, or falling back to that public diagnostic after a control 403.
    let gateway = Gateway::new(None).await;
    let viewer = AuditClient::new(
        gateway.client.base.as_str(),
        "fixture-control-client-viewer",
    )
    .unwrap();
    let before = gateway.store.head().unwrap();
    let diagnostic = viewer.supervision_identity().await.unwrap();
    assert_eq!(diagnostic.role, "viewer");
    assert_eq!(diagnostic.tenant_id, "fixture-tenant");
    assert!(diagnostic.active);
    assert_eq!(
        viewer.control_supervision_identity().await.unwrap_err(),
        ControlIdentityError::Forbidden
    );
    let control = gateway.client.control_supervision_identity().await.unwrap();
    assert_eq!(control.role, "runner");
    assert_eq!(control.tenant_id, "fixture-tenant");
    assert_eq!(
        control.control_namespace_uuid,
        diagnostic.control_namespace_uuid
    );
    assert_eq!(gateway.store.head().unwrap(), before);
    assert_eq!(
        *gateway.seen.lock().unwrap(),
        vec![
            (Method::GET, "/api/v1/control/protocol".into()),
            (Method::GET, "/control/api/v1/control/protocol".into()),
            (Method::GET, "/control/api/v1/control/protocol".into()),
        ]
    );
}
