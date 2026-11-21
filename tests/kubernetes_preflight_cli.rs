//! Real CLI and Axum/SQLite audit. The regular tests use an owned TLS fixture.
//! The ignored live test uses an operator-prepared loopback cluster and dry-run only.
mod support;
use agentsandbox::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    control_fs::PrivateFile,
    model::{Event, StoredEvent},
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

async fn run_preflight_cli(
    root: &Path,
    authority: &Path,
    ca: &Path,
    token: &Path,
    plan: &Path,
    control: &Path,
    deadline: Duration,
) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentsandbox"));
    child
        .env_clear()
        .current_dir(root)
        .args(["kubernetes", "preflight", "--authority-file"])
        .arg(authority)
        .arg("--ca-file")
        .arg(ca)
        .arg("--token-file")
        .arg(token)
        .arg("--plan-file")
        .arg(plan)
        .arg("--control-file")
        .arg(control)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    tokio::time::timeout(deadline, child.output())
        .await
        .expect("preflight CLI deadline exceeded")
        .expect("could not spawn or wait for preflight CLI")
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
        run_preflight_cli(
            &self.root,
            &self.authority,
            &self.ca,
            &self.kube_token,
            &self.plan,
            &self.control,
            Duration::from_secs(45),
        )
        .await
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

fn live_qa_path(variable: &str) -> PathBuf {
    let path =
        PathBuf::from(std::env::var_os(variable).unwrap_or_else(|| panic!("set {variable}")));
    assert!(path.is_absolute(), "{variable} requires an absolute path");
    path
}

fn explicit_loopback_https(endpoint: &str) -> bool {
    endpoint
        .strip_prefix("https://127.0.0.1:")
        .or_else(|| endpoint.strip_prefix("https://[::1]:"))
        .and_then(|target| target.strip_suffix('/'))
        .is_some_and(|port| {
            port.parse::<u16>()
                .is_ok_and(|number| number > 0 && number.to_string() == port)
        })
}

fn assert_live_receipt(receipt: &Value, stored: &StoredEvent) {
    assert!(
        receipt["version"] == 1
            && receipt["event_id"] == stored.event.id
            && receipt["seq"] == stored.seq
            && receipt["hash"] == stored.hash
            && receipt["prev_hash"] == stored.prev_hash
            && receipt["received_at"] == stored.received_at,
        "CLI receipt differs from the independently read Store event"
    );
    assert!(
        stored.event.source == "trusted-control"
            && stored.event.provenance == "declared"
            && stored.event.coverage == "partial",
        "stored preflight event has unexpected provenance"
    );
}

/// Run only with an approved local QA Namespace, restricted inspection RBAC,
/// a scoped Pod-create grant, and a dedicated short-lived ServiceAccount token.
/// This test never provisions the cluster or starts a worker.
#[tokio::test]
#[ignore = "requires an explicitly prepared Docker Desktop loopback Kubernetes QA authority"]
async fn real_loopback_cluster_preflight() {
    const TENANT: &str = "qa-kubernetes-preflight";
    let authority_file = live_qa_path("AI_GUARD_QA_KUBE_AUTHORITY");
    let ca = live_qa_path("AI_GUARD_QA_KUBE_CA");
    let token = live_qa_path("AI_GUARD_QA_KUBE_TOKEN");
    let image = std::env::var("AI_GUARD_QA_KUBE_IMAGE")
        .expect("set AI_GUARD_QA_KUBE_IMAGE to an immutable reference");
    let authority_source =
        PrivateFile::open(&authority_file, 16 * 1024).expect("open private QA authority");
    let authority: KubernetesAuthorityV1 = serde_json::from_slice(authority_source.bytes())
        .unwrap_or_else(|_| panic!("QA authority does not match KubernetesAuthorityV1"));
    assert!(
        authority
            .namespace()
            .strip_prefix("aig-v1-")
            .is_some_and(|suffix| !suffix.is_empty()),
        "QA authority requires a dedicated aig-v1- namespace"
    );
    let authority_json = serde_json::to_value(&authority).expect("serialize closed authority");
    assert!(
        authority_json["endpoint"]
            .as_str()
            .is_some_and(explicit_loopback_https),
        "QA authority must specify a literal HTTPS loopback address and port"
    );
    let operation = Uuid::new_v4();
    let session = Uuid::new_v4();
    let plan = KubernetesPreflightPlanV1::new(
        &authority,
        session,
        operation,
        &image,
        KubernetesPreflightResourcesV1 {
            cpu_millis: 2000,
            memory_mib: 1024,
            workspace_mib: 128,
            temporary_mib: 64,
            deadline_seconds: 60,
        },
    )
    .expect("invalid QA authority or immutable image reference");
    let target = serde_json::to_value(plan.target(&authority).expect("build QA target"))
        .expect("serialize QA target");
    let temp = support::private_tempdir();
    let root = temp
        .path()
        .canonicalize()
        .expect("resolve private QA directory");
    // The CLI receives the exact authority whose endpoint and Namespace passed the checks.
    let retained_authority = root.join("authority.json");
    private(&retained_authority, authority_source.bytes());
    let runner_file = root.join("runner");
    let runner = Uuid::new_v4().simple().to_string();
    let viewer = Uuid::new_v4().simple().to_string();
    private(&runner_file, runner.as_bytes());
    let store = Arc::new(
        Store::create_supervised_for_tenant(
            &root.join("audit.sqlite"),
            TENANT,
            Redactor::new(vec![runner.clone(), viewer.clone()]),
            10_000,
            &root.join("before-v3.sqlite"),
        )
        .expect("initialize private schema3 QA audit"),
    );
    let initial = store.integrity().expect("verify initial QA audit");
    assert!(initial.valid && store.workers().expect("read initial workers").is_empty());
    let control_namespace = store
        .supervision_state()
        .expect("read QA supervision identity")
        .control_namespace_uuid
        .expect("QA control namespace is missing");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind QA gateway to loopback");
    let gateway = format!(
        "http://{}",
        listener.local_addr().expect("local QA address")
    );
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: TENANT.into(),
            store: Arc::clone(&store),
            runner_token: runner,
            viewer_token: viewer,
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .expect("configure QA gateway");
    let gateway_task =
        tokio::spawn(async move { axum::serve(listener, gateway_router(state)).await });
    let plan_file = root.join("plan.json");
    let control_file = root.join("control.json");
    private(
        &plan_file,
        &serde_json::to_vec(&plan).expect("serialize QA plan"),
    );
    private(
        &control_file,
        &serde_json::to_vec(&json!({
            "version": 1,
            "gateway": gateway,
            "control_namespace_uuid": control_namespace,
            "runner_token_file": runner_file,
        }))
        .expect("serialize private QA control references"),
    );
    let output = run_preflight_cli(
        &root,
        &retained_authority,
        &ca,
        &token,
        &plan_file,
        &control_file,
        Duration::from_secs(100),
    )
    .await;
    gateway_task.abort();
    let _ = gateway_task.await;
    let events = |kind: &str| {
        store
            .events(&EventQuery {
                kind: Some(kind.into()),
                limit: Some(2),
                ..Default::default()
            })
            .expect("read independent QA audit")
    };
    let requested = events("runtime.kubernetes.preflight.requested");
    let result = events("runtime.kubernetes.preflight.result");
    let failure = if let Some(result) = result.first() {
        match result.event.data["runtime_error"].as_str() {
            Some("api_rejected") => "api_rejected",
            Some("invalid_response") => "invalid_response",
            Some("preflight_permission_denied") => "preflight_permission_denied",
            Some("preflight_permission_unconfirmed") => "preflight_permission_unconfirmed",
            Some("preflight_profile_changed") => "preflight_profile_changed",
            Some("transport_unavailable") => "transport_unavailable",
            Some("timeout") => "timeout",
            _ => "no recognized runtime error",
        }
    } else {
        "no result receipt retained"
    };
    let api_status = result
        .first()
        .and_then(|event| event.event.data["api_status"].as_u64())
        .filter(|status| (100..=599).contains(status));
    assert!(
        output.status.success(),
        "live preflight exit {:?}, requested={}, result={}, diagnostic={failure}, HTTP={api_status:?}",
        output.status.code(),
        requested.len(),
        result.len()
    );
    let report: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("live preflight report is invalid JSON"));
    assert!(
        report["version"] == 1
            && report["tenant_id"] == TENANT
            && report["control_namespace_uuid"] == control_namespace
            && report["admission_available"] == false
            && report.get("pod_uid").is_none(),
        "unexpected no-start CLI report"
    );
    assert!(
        report["observation"]["target"] == target
            && report["observation"]["admission_gaps"]
                .as_array()
                .is_some_and(|gaps| gaps.len() == 10)
            && report["observation"]["admitted_configuration_sha256"]
                .as_str()
                .is_some_and(|digest| digest.len() == 64
                    && digest
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())),
        "dry-run observation or admission gaps are missing"
    );
    assert!(
        requested.len() == 1 && result.len() == 1,
        "expected one request and one result"
    );
    let (requested, result) = (&requested[0], &result[0]);
    assert_live_receipt(&report["request_receipt"], requested);
    assert_live_receipt(&report["result_receipt"], result);
    for event in [&requested.event, &result.event] {
        assert!(
            event.session_id.as_deref() == Some(session.to_string().as_str())
                && event.correlation_id.as_deref() == Some(operation.to_string().as_str())
                && event.data["target"] == target
                && event.data["tenant_id"] == TENANT
                && event.data["control_namespace_uuid"] == control_namespace
                && event.data["admission_available"] == false,
            "audit event is not bound to the exact QA operation"
        );
    }
    assert!(
        requested.event.id == operation.to_string()
            && result.event.data["request_receipt"] == report["request_receipt"]
            && result.event.data["emission_guard_consumed"] == true
            && result.event.data["admission_effect"] == "dry_run_response_validated"
            && result.event.data["observation"] == report["observation"],
        "durable result does not confirm the exact admission dry-run"
    );
    let integrity = store.integrity().expect("verify final QA audit");
    assert!(
        integrity.valid
            && integrity.event_count == initial.event_count + 2
            && requested.prev_hash == initial.head_hash
            && requested.seq + 1 == result.seq
            && result.prev_hash == requested.hash
            && integrity.head_hash == result.hash,
        "QA audit chain was not extended by exactly two linked events"
    );
    assert!(store.workers().expect("read final workers").is_empty());
    println!(
        "Kubernetes dry-run: operation={operation} request_seq={} result_seq={} audit_head={} workers=0",
        requested.seq, result.seq, integrity.head_hash,
    );
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
    if request.method() == "POST"
        && path == "/apis/authorization.k8s.io/v1/selfsubjectaccessreviews"
    {
        assert!(request.uri().query().is_none());
        assert_eq!(
            request.headers().get("content-type").unwrap(),
            "application/json"
        );
        assert!(request.headers().get("impersonate-user").is_none());
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
            "request audit must be durable before the authorization query"
        );
        assert_eq!(events[0].event.source, "trusted-control");
        assert_eq!(events[0].event.provenance, "declared");
        assert_eq!(events[0].event.coverage, "partial");
        let bytes = to_bytes(request.into_body(), 4096).await.unwrap();
        let review: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            review,
            json!({
                "apiVersion":"authorization.k8s.io/v1", "kind":"SelfSubjectAccessReview",
                "spec":{"resourceAttributes":{
                    "namespace":"tenant-a", "group":"", "version":"v1",
                    "resource":"pods", "verb":"create"
                }}
            })
        );
        return Response::builder()
            .status(201)
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "apiVersion":"authorization.k8s.io/v1", "kind":"SelfSubjectAccessReview",
                    "spec":review["spec"], "status":{"allowed":true}
                }))
                .unwrap(),
            ))
            .unwrap();
    }
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
