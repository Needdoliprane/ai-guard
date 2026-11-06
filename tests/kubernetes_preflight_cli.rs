//! Real CLI + real Axum/SQLite audit + owned TLS API fixture. No cluster or
//! worker is created; admission response fixtures do not qualify Kubernetes.
mod support;
use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    control_fs::PrivateFile,
    model::Event,
    policy::Policy,
    redaction::Redactor,
    runtime::kubernetes::{
        KubernetesAuthorityV1, KubernetesPreflightPlanV1, KubernetesPreflightResourcesV1,
    },
    tls::{ServerIdentity, serve},
};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    middleware::{self, Next},
    response::Response,
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::process::Command;
use uuid::Uuid;

const RUNNER: &str = "public-preflight-control-runner-000000000000000000";
const VIEWER: &str = "public-preflight-control-viewer-000000000000000000";
const KUBE: &str = "public-preflight-kube-credential-000000000000000000";
const CANARY: &str = "private-remote-error-must-not-appear";
const CLUSTER: &str = "11111111-1111-4111-8111-111111111111";
const NAMESPACE: &str = "22222222-2222-4222-8222-222222222222";

#[derive(Clone, Copy, PartialEq, Eq)]
enum AuditMode {
    Normal,
    RejectRequested,
    DivergentRequested,
    InvalidReceipt(ReceiptFault),
    ChangePlanAfterRequested,
    ChangePlanAfterResult,
    RejectResult,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReceiptFault {
    Sequence,
    Hash,
    ReceivedAt,
    UnknownField,
    DuplicateSequence,
    DuplicateTargetHash,
}
#[derive(Clone)]
struct AuditFilterState {
    mode: Arc<Mutex<AuditMode>>,
    plan: PathBuf,
}
#[derive(Clone)]
struct ApiState {
    store: Arc<Store>,
    posts: Arc<Mutex<usize>>,
    reject_pod: Arc<Mutex<bool>>,
}
struct Fixture {
    _root: tempfile::TempDir,
    root: PathBuf,
    ca: PathBuf,
    kube_token: PathBuf,
    authority: PathBuf,
    plan: PathBuf,
    control: PathBuf,
    control_value: Value,
    store: Arc<Store>,
    posts: Arc<Mutex<usize>>,
    audit_mode: Arc<Mutex<AuditMode>>,
    reject_pod: Arc<Mutex<bool>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
fn private(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}
impl Fixture {
    async fn new() -> Self {
        let root = support::private_tempdir();
        let path = root.path().canonicalize().unwrap();
        let pki = path.join("pki");
        let mut child = Command::new(env!("CARGO_BIN_EXE_ai-guard-demo-pki"))
            .env_clear()
            .args(["--dns", "fixture.invalid"])
            .arg("--out")
            .arg(&pki)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(120), child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
        let ca = path.join("ca.pem");
        private(&ca, &fs::read(pki.join("root-ca.pem")).unwrap());
        let kube_token = path.join("kube-token");
        private(&kube_token, KUBE.as_bytes());
        let runner_token = path.join("runner-token");
        private(&runner_token, RUNNER.as_bytes());
        let store = Arc::new(
            Store::open_for_tenant(
                &path.join("audit.sqlite"),
                "server-selected-tenant",
                Redactor::new(vec![]),
                10_000,
            )
            .unwrap(),
        );
        store
            .migrate_control_v3(&path.join("before-v3.sqlite"))
            .unwrap();
        let gateway = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: "server-selected-tenant".into(),
                store: store.clone(),
                runner_token: RUNNER.into(),
                viewer_token: VIEWER.into(),
            }],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap();
        let audit_mode = Arc::new(Mutex::new(AuditMode::Normal));
        let audit_router = gateway_router(gateway).layer(middleware::from_fn_with_state(
            AuditFilterState {
                mode: audit_mode.clone(),
                plan: path.join("plan.json"),
            },
            audit_filter,
        ));
        let audit_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway_origin = format!("http://{}", audit_listener.local_addr().unwrap());
        let audit_task = tokio::spawn(async move {
            axum::serve(audit_listener, audit_router).await.unwrap();
        });
        let posts = Arc::new(Mutex::new(0));
        let reject_pod = Arc::new(Mutex::new(false));
        let state = ApiState {
            store: store.clone(),
            posts: posts.clone(),
            reject_pod: reject_pod.clone(),
        };
        let kube_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("https://{}", kube_listener.local_addr().unwrap());
        let identity =
            ServerIdentity::from_files(&pki.join("server-chain.pem"), &pki.join("server.key"))
                .unwrap();
        let kube_task = tokio::spawn(async move {
            serve(
                kube_listener,
                Router::new().fallback(kube_handler).with_state(state),
                identity,
                std::future::pending::<()>(),
            )
            .await
            .unwrap();
        });
        let binding = KubernetesAuthorityV1::new(
            Uuid::from_u128(1),
            &endpoint,
            CLUSTER,
            "tenant-a",
            NAMESPACE,
            PrivateFile::open(&ca, 65536).unwrap().digest(),
        )
        .unwrap();
        let authority = path.join("authority.json");
        private(&authority, &serde_json::to_vec(&binding).unwrap());
        let plan = path.join("plan.json");
        let value = KubernetesPreflightPlanV1::new(
            &binding,
            Uuid::from_u128(7),
            Uuid::from_u128(8),
            &format!("local/worker@sha256:{}", "b".repeat(64)),
            KubernetesPreflightResourcesV1 {
                cpu_millis: 2000,
                memory_mib: 1024,
                workspace_mib: 128,
                temporary_mib: 64,
                deadline_seconds: 60,
            },
        )
        .unwrap();
        private(&plan, &serde_json::to_vec(&value).unwrap());
        let control_value = json!({"version":1,"gateway":gateway_origin,"control_namespace_uuid":store.supervision_state().unwrap().control_namespace_uuid.unwrap(),"runner_token_file":runner_token});
        let control = path.join("control.json");
        private(&control, &serde_json::to_vec(&control_value).unwrap());
        Self {
            _root: root,
            root: path,
            ca,
            kube_token,
            authority,
            plan,
            control,
            control_value,
            store,
            posts,
            audit_mode,
            reject_pod,
            tasks: vec![audit_task, kube_task],
        }
    }
    async fn run(&self) -> std::process::Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
        child
            .env_clear()
            .current_dir(&self.root)
            .args(["kubernetes", "preflight", "--authority-file"])
            .arg(&self.authority)
            .arg("--ca-file")
            .arg(&self.ca)
            .arg("--token-file")
            .arg(&self.kube_token)
            .arg("--plan-file")
            .arg(&self.plan)
            .arg("--control-file")
            .arg(&self.control)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        tokio::time::timeout(Duration::from_secs(45), child.output())
            .await
            .unwrap()
            .unwrap()
    }
    fn events(&self, kind: &str) -> Vec<agentsandbox::model::StoredEvent> {
        self.store
            .events(&EventQuery {
                kind: Some(kind.into()),
                limit: Some(100),
                ..Default::default()
            })
            .unwrap()
    }
}
async fn audit_filter(
    State(state): State<AuditFilterState>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() != "POST" || request.uri().path() != "/api/v1/events" {
        return next.run(request).await;
    }
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 65536).await.unwrap();
    let event: Event = serde_json::from_slice(&bytes).unwrap();
    let mode = *state.mode.lock().unwrap();
    let requested = event.kind.ends_with(".requested");
    if (mode == AuditMode::RejectRequested && requested)
        || (mode == AuditMode::RejectResult && !requested)
    {
        return Response::builder()
            .status(503)
            .body(Body::from(CANARY))
            .unwrap();
    }
    let response = next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await;
    if (mode == AuditMode::ChangePlanAfterRequested && requested)
        || (mode == AuditMode::ChangePlanAfterResult && !requested)
    {
        let mut bytes = fs::read(&state.plan).unwrap();
        bytes.push(b'\n');
        private(&state.plan, &bytes);
    }
    if !matches!(
        mode,
        AuditMode::DivergentRequested | AuditMode::InvalidReceipt(_)
    ) || !requested
    {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes = to_bytes(body, 65536).await.unwrap();
    let mut receipt: Value = serde_json::from_slice(&bytes).unwrap();
    match mode {
        AuditMode::DivergentRequested => receipt["source"] = json!("divergent-echo"),
        AuditMode::InvalidReceipt(ReceiptFault::Sequence) => receipt["seq"] = json!(0),
        AuditMode::InvalidReceipt(ReceiptFault::Hash) => receipt["hash"] = json!("not-a-digest"),
        AuditMode::InvalidReceipt(ReceiptFault::ReceivedAt) => {
            receipt["received_at"] = json!("not-a-time")
        }
        AuditMode::InvalidReceipt(ReceiptFault::UnknownField) => {
            receipt["unrecognized"] = json!("must-refuse")
        }
        AuditMode::InvalidReceipt(
            ReceiptFault::DuplicateSequence | ReceiptFault::DuplicateTargetHash,
        ) => {}
        _ => unreachable!(),
    }
    let mut text = serde_json::to_string(&receipt).unwrap();
    if mode == AuditMode::InvalidReceipt(ReceiptFault::DuplicateSequence) {
        text.pop();
        text.push_str(&format!(",\"seq\":{}}}", receipt["seq"]));
    }
    if mode == AuditMode::InvalidReceipt(ReceiptFault::DuplicateTargetHash) {
        let pair = format!(
            "\"request_sha256\":{}",
            receipt["data"]["target"]["request_sha256"]
        );
        assert!(text.contains(&pair));
        text = text.replacen(&pair, &format!("{pair},{pair}"), 1);
    }
    let bytes = text.into_bytes();
    parts
        .headers
        .insert("content-length", bytes.len().to_string().parse().unwrap());
    Response::from_parts(parts, Body::from(bytes))
}
async fn kube_handler(State(state): State<ApiState>, request: Request) -> Response {
    assert_eq!(
        request.headers().get("authorization").unwrap().as_bytes(),
        format!("Bearer {KUBE}").as_bytes()
    );
    let path = request.uri().path().to_owned();
    let body = if request.method() == "POST" {
        assert_eq!(path, "/api/v1/namespaces/tenant-a/pods");
        assert_eq!(
            request.uri().query(),
            Some("dryRun=All&fieldValidation=Strict")
        );
        let events = state
            .store
            .events(&EventQuery {
                kind: Some("runtime.kubernetes.preflight.requested".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            events.len(),
            1,
            "durable requested event must exist before the actual TLS POST barrier"
        );
        assert_eq!(events[0].event.source, "trusted-control");
        assert_eq!(events[0].event.provenance, "declared");
        assert_eq!(events[0].event.coverage, "partial");
        *state.posts.lock().unwrap() += 1;
        if *state.reject_pod.lock().unwrap() {
            return Response::builder()
                .status(422)
                .body(Body::from(CANARY))
                .unwrap();
        }
        let bytes = to_bytes(request.into_body(), 65536).await.unwrap();
        serde_json::from_slice::<Value>(&bytes).unwrap()
    } else {
        assert_eq!(request.method(), "GET");
        match path.as_str() {
            "/api/v1/namespaces/kube-system" => {
                json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":"kube-system","uid":CLUSTER},"status":{"phase":"Active"}})
            }
            "/api/v1/namespaces/tenant-a" => {
                json!({"apiVersion":"v1","kind":"Namespace","metadata":{"name":"tenant-a","uid":NAMESPACE},"status":{"phase":"Active"}})
            }
            _ => panic!("unexpected API path"),
        }
    };
    Response::builder()
        .status(if path.ends_with("/pods") { 201 } else { 200 })
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}
fn refused(output: &std::process::Output) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    for secret in [RUNNER, VIEWER, KUBE, CANARY] {
        assert!(
            !output
                .stdout
                .windows(secret.len())
                .chain(output.stderr.windows(secret.len()))
                .any(|window| window == secret.as_bytes())
        );
    }
}

#[tokio::test]
async fn preflight_cli_requires_durable_request_before_https_and_durable_result_before_success() {
    let fixture = Fixture::new().await;
    let output = fixture.run().await;
    assert!(
        output.status.success(),
        "CLI did not complete closed preflight: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["admission_available"], false);
    assert_eq!(report["tenant_id"], "server-selected-tenant");
    assert!(report.get("pod_uid").is_none());
    assert_eq!(*fixture.posts.lock().unwrap(), 1);
    let requested = fixture.events("runtime.kubernetes.preflight.requested");
    let result = fixture.events("runtime.kubernetes.preflight.result");
    assert_eq!(requested.len(), 1);
    assert_eq!(result.len(), 1);
    assert!(requested[0].seq < result[0].seq);
    assert_eq!(
        requested[0].event.data["tenant_id"],
        "server-selected-tenant"
    );
    assert_eq!(
        result[0].event.data["admission_effect"],
        "dry_run_response_validated"
    );
    assert!(fixture.store.integrity().unwrap().valid);
    let mut conflicting: Value = serde_json::from_slice(&fs::read(&fixture.plan).unwrap()).unwrap();
    conflicting["image"] = json!(format!("local/worker@sha256:{}", "c".repeat(64)));
    private(&fixture.plan, &serde_json::to_vec(&conflicting).unwrap());
    refused(&fixture.run().await);
    assert_eq!(
        *fixture.posts.lock().unwrap(),
        1,
        "same operation with divergent target must not emit another POST"
    );
}

#[tokio::test]
async fn preflight_cli_rejects_invalid_or_extended_audit_receipts_before_pod_post() {
    let fixture = Fixture::new().await;
    for (index, fault) in [
        ReceiptFault::Sequence,
        ReceiptFault::Hash,
        ReceiptFault::ReceivedAt,
        ReceiptFault::UnknownField,
        ReceiptFault::DuplicateSequence,
        ReceiptFault::DuplicateTargetHash,
    ]
    .into_iter()
    .enumerate()
    {
        let mut plan: Value = serde_json::from_slice(&fs::read(&fixture.plan).unwrap()).unwrap();
        plan["operation_id"] = json!(Uuid::from_u128(100 + index as u128));
        private(&fixture.plan, &serde_json::to_vec(&plan).unwrap());
        *fixture.audit_mode.lock().unwrap() = AuditMode::InvalidReceipt(fault);
        refused(&fixture.run().await);
        assert_eq!(*fixture.posts.lock().unwrap(), 0);
    }
    assert_eq!(
        fixture
            .events("runtime.kubernetes.preflight.requested")
            .len(),
        6,
        "each bad receipt must follow a real durable append, not a failed fixture"
    );
}

#[tokio::test]
async fn preflight_cli_audit_rejection_or_divergent_receipt_never_reaches_pod_post() {
    let fixture = Fixture::new().await;
    for mode in [AuditMode::RejectRequested, AuditMode::DivergentRequested] {
        *fixture.audit_mode.lock().unwrap() = mode;
        refused(&fixture.run().await);
        assert_eq!(*fixture.posts.lock().unwrap(), 0);
    }
}

#[tokio::test]
async fn preflight_cli_rejects_viewer_wrong_control_namespace_and_client_selected_tenant() {
    let fixture = Fixture::new().await;
    let viewer = fixture.root.join("viewer");
    private(&viewer, VIEWER.as_bytes());
    for (field, value) in [
        ("runner_token_file", json!(viewer)),
        ("control_namespace_uuid", json!(Uuid::new_v4())),
        ("tenant_id", json!("chosen-by-client")),
    ] {
        let mut config = fixture.control_value.clone();
        config[field] = value;
        private(&fixture.control, &serde_json::to_vec(&config).unwrap());
        refused(&fixture.run().await);
        assert_eq!(*fixture.posts.lock().unwrap(), 0);
        assert!(
            fixture
                .events("runtime.kubernetes.preflight.requested")
                .is_empty()
        );
    }
}

#[tokio::test]
async fn preflight_cli_runtime_error_after_post_is_audited_as_unknown_and_never_succeeds() {
    let fixture = Fixture::new().await;
    *fixture.reject_pod.lock().unwrap() = true;
    refused(&fixture.run().await);
    assert_eq!(*fixture.posts.lock().unwrap(), 1);
    let result = fixture.events("runtime.kubernetes.preflight.result");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].event.data["admission_effect"], "unknown_possible");
    assert_eq!(result[0].event.data["outcome"], "error");
}

#[tokio::test]
async fn preflight_cli_result_audit_failure_suppresses_success_after_post() {
    let fixture = Fixture::new().await;
    *fixture.audit_mode.lock().unwrap() = AuditMode::RejectResult;
    refused(&fixture.run().await);
    assert_eq!(*fixture.posts.lock().unwrap(), 1);
    assert_eq!(
        fixture
            .events("runtime.kubernetes.preflight.requested")
            .len(),
        1
    );
    assert!(
        fixture
            .events("runtime.kubernetes.preflight.result")
            .is_empty()
    );
}

#[tokio::test]
async fn preflight_cli_revalidates_retained_plan_after_audit_and_refuses_the_post() {
    let fixture = Fixture::new().await;
    *fixture.audit_mode.lock().unwrap() = AuditMode::ChangePlanAfterRequested;
    refused(&fixture.run().await);
    assert_eq!(*fixture.posts.lock().unwrap(), 0);
    assert_eq!(
        fixture
            .events("runtime.kubernetes.preflight.requested")
            .len(),
        1
    );
    let result = fixture.events("runtime.kubernetes.preflight.result");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].event.data["emission_guard_consumed"], false);
    assert_eq!(result[0].event.data["outcome"], "error");
}

#[tokio::test]
async fn preflight_cli_final_file_substitution_reports_possible_effect_after_the_result_receipt() {
    let fixture = Fixture::new().await;
    *fixture.audit_mode.lock().unwrap() = AuditMode::ChangePlanAfterResult;
    let output = fixture.run().await;
    refused(&output);
    assert_eq!(*fixture.posts.lock().unwrap(), 1);
    assert_eq!(
        fixture.events("runtime.kubernetes.preflight.result").len(),
        1
    );
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("effect") && error.contains("possible") && error.contains("no retry"));
}

#[tokio::test]
async fn preflight_cli_rejects_known_credentials_in_the_image_before_audit_or_api_transmission() {
    let fixture = Fixture::new().await;
    let mut plan: Value = serde_json::from_slice(&fs::read(&fixture.plan).unwrap()).unwrap();
    plan["image"] = json!(format!("local/{KUBE}@sha256:{}", "b".repeat(64)));
    private(&fixture.plan, &serde_json::to_vec(&plan).unwrap());
    refused(&fixture.run().await);
    assert_eq!(*fixture.posts.lock().unwrap(), 0);
    assert!(
        fixture
            .events("runtime.kubernetes.preflight.requested")
            .is_empty()
    );
}
