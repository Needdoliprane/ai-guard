//! Real loopback HTTP/SQLite contracts for cold broker audit reads and writes.
//! The adversarial peer below is limited to response-integrity faults; it does
//! not replace the real gateway in the tenant-scoped path.
use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{EventQuery, Store},
    control::{
        AdmissionV1, DecisionPhaseV1, DecisionReceiptV1, LaunchSpecV1, NetworkV1, QuotaPolicyV1,
        RestartPolicyV1, RuntimeKindV1, SecurityPresetV1,
    },
    model::{CommandSpec, DecisionRequest, Event, StoredEvent},
    policy::{self, Policy},
    redaction::Redactor,
};
use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Notify,
    task::JoinHandle,
    time::Instant,
};
use uuid::Uuid;

struct GatewayFixture {
    _directory: tempfile::TempDir,
    origin: String,
    store_a: Arc<Store>,
    store_b: Arc<Store>,
    token_a: String,
    token_b: String,
    worker_a: crate::control::WorkerV1,
    worker_b: crate::control::WorkerV1,
    task: JoinHandle<()>,
}

impl Drop for GatewayFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl GatewayFixture {
    async fn close(&mut self) {
        self.task.abort();
        let _ = (&mut self.task).await;
    }

    async fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("cold-audit-http-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .expect("private cold audit fixture");
        let store_a = Arc::new(
            Store::open_for_tenant(
                &directory.path().join("tenant-a.sqlite"),
                "tenant-a",
                Redactor::new(vec![]),
                10_000,
            )
            .unwrap(),
        );
        let store_b = Arc::new(
            Store::open_for_tenant(
                &directory.path().join("tenant-b.sqlite"),
                "tenant-b",
                Redactor::new(vec![]),
                10_000,
            )
            .unwrap(),
        );
        let worker_a = seed_worker(&store_a);
        let worker_b = seed_worker(&store_b);
        let token_a = format!("cold-a-runner-{}", "a".repeat(40));
        let token_b = format!("cold-b-runner-{}", "b".repeat(40));
        let state = GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: Arc::clone(&store_a),
                    runner_token: token_a.clone(),
                    viewer_token: format!("cold-a-viewer-{}", "v".repeat(40)),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: Arc::clone(&store_b),
                    runner_token: token_b.clone(),
                    viewer_token: format!("cold-b-viewer-{}", "w".repeat(40)),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, gateway_router(state)).await.unwrap();
        });
        Self {
            _directory: directory,
            origin,
            store_a,
            store_b,
            token_a,
            token_b,
            worker_a,
            worker_b,
            task,
        }
    }
}

fn seed_worker(store: &Store) -> crate::control::WorkerV1 {
    let session = Uuid::new_v4().to_string();
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
        session_id: Some(session.clone()),
        correlation_id: None,
        trace_id: None,
        human_approved: false,
        approval_hash: None,
        execution_mode: None,
        provider_resolution: None,
    };
    let stored = store
        .decision(
            &request,
            Some((policy::evaluate(&Policy::default(), &request), Value::Null)),
        )
        .unwrap()
        .unwrap();
    let admission = AdmissionV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        decision_request: request.clone(),
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
            session_id: session,
            command: request.command.clone(),
            profile: "strict".into(),
            runtime_kind: RuntimeKindV1::Docker,
            authority_id: "local-daemon-cold-audit-fixture".into(),
            image_ref: "worker:cold-audit-fixture".into(),
            image_id: format!("sha256:{}", "a".repeat(64)),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1024 * 1024 * 1024,
            pids_limit: 256,
            timeout_seconds: 60,
            security_preset: SecurityPresetV1::FixedSandboxV1,
            restart_policy: RestartPolicyV1::No,
            workspace_manifest_digest: "b".repeat(64),
            workspace_import_policy_version: 1,
            provider: "generic".into(),
            capture_output: false,
            credential_binding_id: None,
            credential_names: vec![],
            provider_resolution: None,
        },
    };
    store
        .admit_worker(
            &admission,
            &Policy::default(),
            policy::evaluate(&Policy::default(), &request),
            Value::Null,
            &QuotaPolicyV1::default(),
        )
        .unwrap()
        .worker
        .expect("real tenant Store reserves the worker")
}

fn event_for(tenant: &str, id: &str) -> Event {
    let mut event = Event::new("observer.coverage");
    event.id = id.to_owned();
    event.source = "cold_audit_test".into();
    event.provenance = "declared".into();
    event.coverage = "partial".into();
    event.data = json!({"fixture_tenant":tenant,"observation":"synthetic declared fact"});
    event
}

fn stored_for(store: &Store, event_id: &str) -> StoredEvent {
    store
        .events(&EventQuery {
            kind: Some("observer.coverage".into()),
            limit: Some(100),
            ..Default::default()
        })
        .unwrap()
        .into_iter()
        .find(|stored| stored.event.id == event_id)
        .expect("exact Event is present in the tenant Store")
}

#[tokio::test]
async fn cold_http_reads_authenticated_workers_and_replays_exact_events_per_tenant() {
    let mut fixture = GatewayFixture::new().await;
    assert!(!fixture.task.is_finished());
    let client_a = AuditClient::new(&fixture.origin, &fixture.token_a).unwrap();
    let client_b = AuditClient::new(&fixture.origin, &fixture.token_b).unwrap();
    let budget_a = ColdAuditBudget::start();
    let budget_b = ColdAuditBudget::start();

    let received_a = client_a
        .cold_worker(&fixture.worker_a.worker_id, &budget_a)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&received_a).unwrap(),
        serde_json::to_value(&fixture.worker_a).unwrap()
    );
    let received_b = client_b
        .cold_worker(&fixture.worker_b.worker_id, &budget_b)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&received_b).unwrap(),
        serde_json::to_value(&fixture.worker_b).unwrap()
    );
    assert!(
        client_a
            .cold_worker(&fixture.worker_b.worker_id, &budget_a)
            .await
            .is_err()
    );

    let shared_id = Uuid::new_v4().to_string();
    let event_a = event_for("tenant-a", &shared_id);
    let event_b = event_for("tenant-b", &shared_id);
    let receipt_a = client_a
        .post_cold_audit_event(&event_a, &budget_a)
        .await
        .unwrap();
    let receipt_b = client_b
        .post_cold_audit_event(&event_b, &budget_b)
        .await
        .unwrap();
    let replay_a = client_a
        .post_cold_audit_event(&event_a, &budget_a)
        .await
        .unwrap();
    let replay_b = client_b
        .post_cold_audit_event(&event_b, &budget_b)
        .await
        .unwrap();

    let stored_a = stored_for(&fixture.store_a, &shared_id);
    let stored_b = stored_for(&fixture.store_b, &shared_id);
    assert_eq!(
        serde_json::to_value(receipt_a.stored_event()).unwrap(),
        serde_json::to_value(&stored_a).unwrap()
    );
    assert_eq!(
        serde_json::to_value(receipt_b.stored_event()).unwrap(),
        serde_json::to_value(&stored_b).unwrap()
    );
    assert_eq!(
        serde_json::to_value(replay_a.stored_event()).unwrap(),
        serde_json::to_value(&stored_a).unwrap()
    );
    assert_eq!(
        serde_json::to_value(replay_b.stored_event()).unwrap(),
        serde_json::to_value(&stored_b).unwrap()
    );
    assert_eq!(receipt_a.seq(), stored_a.seq);
    assert_eq!(replay_a.seq(), stored_a.seq);
    assert_eq!(receipt_b.seq(), stored_b.seq);
    assert_eq!(replay_b.seq(), stored_b.seq);
    assert_eq!(stored_a.event.data["fixture_tenant"], "tenant-a");
    assert_eq!(stored_b.event.data["fixture_tenant"], "tenant-b");
    assert_eq!(
        fixture.store_a.head().unwrap(),
        stored_a.seq,
        "tenant A replay does not append a second copy"
    );
    assert_eq!(
        fixture.store_b.head().unwrap(),
        stored_b.seq,
        "tenant B replay does not append a second copy"
    );
    assert_ne!(stored_a.hash, stored_b.hash);
    fixture.close().await;
}

const PEER_SEQUENCE_ONLY: usize = 0;
const PEER_UNKNOWN_FIELD: usize = 1;
const PEER_DUPLICATE_FIELD: usize = 2;
const PEER_MUTATED_RECORD: usize = 3;
const PEER_OVERSIZED_BODY: usize = 4;
const PEER_CREATED_STATUS: usize = 5;
const PEER_REDIRECT: usize = 6;
const PEER_EXACT: usize = 7;

#[derive(Clone)]
struct PeerState {
    expected: StoredEvent,
    worker: crate::control::WorkerV1,
    worker_override: Arc<Mutex<Option<Value>>>,
    mode: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    redirects: Arc<AtomicUsize>,
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
}

async fn event_peer(State(state): State<PeerState>, body: Bytes) -> Response {
    state.requests.fetch_add(1, Ordering::SeqCst);
    state.bodies.lock().unwrap().push(body.to_vec());
    let mode = state.mode.load(Ordering::SeqCst);
    if mode == PEER_CREATED_STATUS {
        return (StatusCode::CREATED, axum::Json(state.expected.clone())).into_response();
    }
    if mode == PEER_REDIRECT {
        return Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, "/redirect-target")
            .body(Body::empty())
            .unwrap();
    }
    let bytes = match mode {
        PEER_SEQUENCE_ONLY => serde_json::to_vec(&json!({"seq":state.expected.seq})).unwrap(),
        PEER_UNKNOWN_FIELD => {
            let mut value = serde_json::to_value(&state.expected).unwrap();
            value["unrecognized"] = json!(true);
            serde_json::to_vec(&value).unwrap()
        }
        PEER_DUPLICATE_FIELD => {
            let valid = String::from_utf8(serde_json::to_vec(&state.expected).unwrap()).unwrap();
            let first = format!("\"seq\":{}", state.expected.seq);
            valid
                .replacen(
                    &first,
                    &format!("{first},\"seq\":{}", state.expected.seq),
                    1,
                )
                .into_bytes()
        }
        PEER_MUTATED_RECORD => {
            let mut value = serde_json::to_value(&state.expected).unwrap();
            value["data"]["mutation"] = json!("different durable event bytes");
            serde_json::to_vec(&value).unwrap()
        }
        PEER_OVERSIZED_BODY => {
            let mut bytes = serde_json::to_vec(&state.expected).unwrap();
            assert!(serde_json::from_slice::<StoredEvent>(&bytes).is_ok());
            bytes.resize(2 * 1024 * 1024, b' ');
            assert!(serde_json::from_slice::<StoredEvent>(&bytes).is_ok());
            bytes
        }
        PEER_EXACT => serde_json::to_vec(&state.expected).unwrap(),
        _ => unreachable!("closed local peer mode"),
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(bytes))
        .unwrap()
}

async fn worker_peer(State(state): State<PeerState>, Path(_id): Path<String>) -> Response {
    state.requests.fetch_add(1, Ordering::SeqCst);
    let value = state
        .worker_override
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| serde_json::to_value(&state.worker).unwrap());
    (StatusCode::OK, axum::Json(value)).into_response()
}

async fn redirect_target(State(state): State<PeerState>) -> Response {
    state.redirects.fetch_add(1, Ordering::SeqCst);
    (StatusCode::OK, "{}").into_response()
}

async fn adversarial_peer(state: PeerState) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new()
        .route("/api/v1/events", post(event_peer))
        .route("/api/v1/workers/{id}", get(worker_peer))
        .route("/redirect-target", any(redirect_target))
        .with_state(state);
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (origin, task)
}

#[tokio::test]
async fn cold_http_rejects_non_exact_event_receipts_and_screens_local_secrets() {
    let directory = tempfile::Builder::new()
        .prefix("cold-audit-peer-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = Store::open_for_tenant(
        &directory.path().join("peer.sqlite"),
        "tenant-peer",
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let worker = seed_worker(&store);
    let event = event_for("tenant-peer", &Uuid::new_v4().to_string());
    let expected = store.append(event.clone()).unwrap();
    let state = PeerState {
        expected,
        worker,
        worker_override: Arc::new(Mutex::new(None)),
        mode: Arc::new(AtomicUsize::new(PEER_SEQUENCE_ONLY)),
        requests: Arc::new(AtomicUsize::new(0)),
        redirects: Arc::new(AtomicUsize::new(0)),
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let (origin, task) = adversarial_peer(state.clone()).await;
    let client = AuditClient::new(&origin, "synthetic-cold-runner-token-aaaaaaaa").unwrap();
    let budget = ColdAuditBudget::start();

    state.mode.store(PEER_EXACT, Ordering::SeqCst);
    let exact = client
        .post_cold_audit_event(&event, &budget)
        .await
        .expect("the neighboring exact response is accepted");
    assert_eq!(
        serde_json::to_value(exact.stored_event()).unwrap(),
        serde_json::to_value(&state.expected).unwrap()
    );
    assert_eq!(exact.seq(), state.expected.seq);
    let valid_body = state.bodies.lock().unwrap()[0].clone();
    assert_eq!(
        serde_json::from_slice::<Value>(&valid_body).unwrap(),
        serde_json::to_value(&event).unwrap(),
        "the successful peer receives the exact submitted event"
    );

    for mode in [
        PEER_SEQUENCE_ONLY,
        PEER_UNKNOWN_FIELD,
        PEER_DUPLICATE_FIELD,
        PEER_MUTATED_RECORD,
        PEER_OVERSIZED_BODY,
    ] {
        state.mode.store(mode, Ordering::SeqCst);
        let requests_before = state.requests.load(Ordering::SeqCst);
        let result = client.post_cold_audit_event(&event, &budget).await;
        assert_eq!(
            result.err(),
            Some(ColdAuditHttpError::InvalidResponse),
            "peer response mode {mode} is not an exact StoredEvent receipt"
        );
        assert_eq!(
            state.requests.load(Ordering::SeqCst),
            requests_before + 1,
            "each adversarial response comes from one real loopback request"
        );
    }

    state.mode.store(PEER_CREATED_STATUS, Ordering::SeqCst);
    assert_eq!(
        client.post_cold_audit_event(&event, &budget).await.err(),
        Some(ColdAuditHttpError::InvalidResponse),
        "201 is not the API's exact durable receipt response"
    );
    state.mode.store(PEER_REDIRECT, Ordering::SeqCst);
    assert_eq!(
        client.post_cold_audit_event(&event, &budget).await.err(),
        Some(ColdAuditHttpError::InvalidResponse),
        "redirects cannot move the retained audit origin"
    );
    assert_eq!(state.redirects.load(Ordering::SeqCst), 0);

    let requested_worker_id = state.worker.worker_id.clone();
    let mut wrong_worker = state.worker.clone();
    wrong_worker.worker_id = Uuid::new_v4().to_string();
    *state.worker_override.lock().unwrap() = Some(serde_json::to_value(wrong_worker).unwrap());
    let requests_before_wrong_worker = state.requests.load(Ordering::SeqCst);
    assert_eq!(
        client
            .cold_worker(&requested_worker_id, &budget)
            .await
            .err(),
        Some(ColdAuditHttpError::InvalidResponse),
        "a valid but different worker cannot answer the requested UUID"
    );
    assert_eq!(
        state.requests.load(Ordering::SeqCst),
        requests_before_wrong_worker + 1
    );

    let mut nested_unknown = serde_json::to_value(&state.worker).unwrap();
    nested_unknown["launch"]["unrecognized"] = json!(true);
    *state.worker_override.lock().unwrap() = Some(nested_unknown);
    let requests_before_nested_unknown = state.requests.load(Ordering::SeqCst);
    assert_eq!(
        client
            .cold_worker(&requested_worker_id, &budget)
            .await
            .err(),
        Some(ColdAuditHttpError::InvalidResponse),
        "unknown nested Worker.launch fields are rejected"
    );
    assert_eq!(
        state.requests.load(Ordering::SeqCst),
        requests_before_nested_unknown + 1
    );

    *state.worker_override.lock().unwrap() = None;
    let requests_before_invalid_worker = state.requests.load(Ordering::SeqCst);
    assert_eq!(
        client
            .cold_worker("not-a-canonical-worker-uuid", &budget)
            .await
            .err(),
        Some(ColdAuditHttpError::InvalidRequest),
        "invalid local worker UUID is rejected before HTTP"
    );
    assert_eq!(
        state.requests.load(Ordering::SeqCst),
        requests_before_invalid_worker
    );

    let mut noncanonical_timestamp = event_for("tenant-peer", &Uuid::new_v4().to_string());
    noncanonical_timestamp.timestamp =
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false);
    assert!(chrono::DateTime::parse_from_rfc3339(&noncanonical_timestamp.timestamp).is_ok());
    let requests_before_timestamp = state.requests.load(Ordering::SeqCst);
    assert_eq!(
        client
            .post_cold_audit_event(&noncanonical_timestamp, &budget)
            .await
            .err(),
        Some(ColdAuditHttpError::InvalidRequest),
        "valid RFC3339 with a noncanonical offset is rejected before HTTP"
    );
    assert_eq!(
        state.requests.load(Ordering::SeqCst),
        requests_before_timestamp
    );

    let mut clipped_event = event_for("tenant-peer", &Uuid::new_v4().to_string());
    clipped_event.data["long_value"] = json!("x".repeat(32_769));
    assert!(serde_json::to_vec(&clipped_event).unwrap().len() <= 120 * 1024);
    let requests_before_clipping = state.requests.load(Ordering::SeqCst);
    assert_eq!(
        client
            .post_cold_audit_event(&clipped_event, &budget)
            .await
            .err(),
        Some(ColdAuditHttpError::InvalidRequest),
        "a value changed by the real 32,768-byte Redactor clipping is rejected before HTTP"
    );
    assert_eq!(
        state.requests.load(Ordering::SeqCst),
        requests_before_clipping
    );

    let secret = "synthetic-private-output-0123456789";
    let mut secret_event = event_for("tenant-peer", &Uuid::new_v4().to_string());
    secret_event.data["private_fixture"] = json!(secret);
    state.mode.store(PEER_EXACT, Ordering::SeqCst);
    let secret_client = client.clone().with_secrets(vec![secret.into()]);
    let requests_before_secret = state.requests.load(Ordering::SeqCst);
    let bodies_before_secret = state.bodies.lock().unwrap().clone();
    assert_eq!(
        secret_client
            .post_cold_audit_event(&secret_event, &budget)
            .await
            .err(),
        Some(ColdAuditHttpError::InvalidRequest)
    );
    assert_eq!(
        state.requests.load(Ordering::SeqCst),
        requests_before_secret
    );
    assert_eq!(
        *state.bodies.lock().unwrap(),
        bodies_before_secret,
        "screened local secret never reaches the peer"
    );
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn cold_http_reuses_one_deadline_and_makes_no_request_after_expiry() {
    let directory = tempfile::Builder::new()
        .prefix("cold-audit-budget-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = Store::open_for_tenant(
        &directory.path().join("budget.sqlite"),
        "tenant-budget",
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let worker = seed_worker(&store);
    let expected = store
        .append(event_for("tenant-budget", &Uuid::new_v4().to_string()))
        .unwrap();
    let state = PeerState {
        expected,
        worker: worker.clone(),
        worker_override: Arc::new(Mutex::new(None)),
        mode: Arc::new(AtomicUsize::new(PEER_EXACT)),
        requests: Arc::new(AtomicUsize::new(0)),
        redirects: Arc::new(AtomicUsize::new(0)),
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let (origin, task) = adversarial_peer(state.clone()).await;
    let client = AuditClient::new(&origin, "synthetic-budget-runner-token-bbbbbbbb").unwrap();
    let mut budget = ColdAuditBudget::start();
    let original_deadline = budget.deadline();

    let fetched = client
        .cold_worker(&worker.worker_id, &budget)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(fetched).unwrap(),
        serde_json::to_value(&worker).unwrap()
    );
    assert_eq!(budget.deadline(), original_deadline);
    assert_eq!(state.requests.load(Ordering::SeqCst), 1);

    // This synthetically expires the test object's sampled deadline. It does
    // not measure wall-clock 45-second behavior; it proves a later request
    // cannot renew the shared budget and is rejected before send.
    budget.deadline = Instant::now() - Duration::from_millis(1);
    let expired_deadline = budget.deadline();
    let event = event_for("tenant-budget", &Uuid::new_v4().to_string());
    assert_eq!(
        client.post_cold_audit_event(&event, &budget).await.err(),
        Some(ColdAuditHttpError::Expired)
    );
    assert_eq!(
        client.cold_worker(&worker.worker_id, &budget).await.err(),
        Some(ColdAuditHttpError::Expired)
    );
    assert_eq!(budget.deadline(), expired_deadline);
    assert_eq!(state.requests.load(Ordering::SeqCst), 1);
    task.abort();
    let _ = task.await;
}

struct ChunkedWorkerObservation {
    requests: AtomicUsize,
    body_bytes_sent: AtomicUsize,
    fragments_sent: AtomicUsize,
    worker_get: std::sync::atomic::AtomicBool,
    response_headers: Mutex<Vec<u8>>,
    first_fragment_sent: Notify,
}

impl ChunkedWorkerObservation {
    fn new() -> Self {
        Self {
            requests: AtomicUsize::new(0),
            body_bytes_sent: AtomicUsize::new(0),
            fragments_sent: AtomicUsize::new(0),
            worker_get: std::sync::atomic::AtomicBool::new(false),
            response_headers: Mutex::new(Vec::new()),
            first_fragment_sent: Notify::new(),
        }
    }
}

struct ChunkedWorkerPeer {
    origin: String,
    task: Option<JoinHandle<()>>,
    observation: Arc<ChunkedWorkerObservation>,
}

impl Drop for ChunkedWorkerPeer {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

impl ChunkedWorkerPeer {
    async fn close(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

async fn chunked_worker_peer(body: Vec<u8>, stall_after_first_fragment: bool) -> ChunkedWorkerPeer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let observation = Arc::new(ChunkedWorkerObservation::new());
    let peer_observation = Arc::clone(&observation);
    let task = tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        peer_observation.requests.fetch_add(1, Ordering::SeqCst);

        let mut request_headers = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !request_headers
            .windows(4)
            .any(|window| window == b"\r\n\r\n")
        {
            let Ok(read) = socket.read(&mut buffer).await else {
                return;
            };
            if read == 0 {
                return;
            }
            request_headers.extend_from_slice(&buffer[..read]);
            if request_headers.len() > 16 * 1024 {
                return;
            }
        }
        let first_line_end = request_headers
            .windows(2)
            .position(|window| window == b"\r\n")
            .unwrap_or(request_headers.len());
        peer_observation.worker_get.store(
            request_headers[..first_line_end].starts_with(b"GET /api/v1/workers/"),
            Ordering::SeqCst,
        );

        let headers = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
        *peer_observation.response_headers.lock().unwrap() = headers.to_vec();
        if socket.write_all(headers).await.is_err() {
            return;
        }

        let chunk_size = if stall_after_first_fragment {
            64
        } else {
            16 * 1024
        };
        let first_end = body.len().min(chunk_size);
        if write_http_chunk(&mut socket, &body[..first_end], &peer_observation)
            .await
            .is_err()
        {
            return;
        }
        if stall_after_first_fragment {
            futures_util::future::pending::<()>().await;
        }

        for chunk in body[first_end..].chunks(chunk_size) {
            if write_http_chunk(&mut socket, chunk, &peer_observation)
                .await
                .is_err()
            {
                return;
            }
        }
        let _ = socket.write_all(b"0\r\n\r\n").await;
    });
    ChunkedWorkerPeer {
        origin,
        task: Some(task),
        observation,
    }
}

async fn write_http_chunk(
    socket: &mut TcpStream,
    chunk: &[u8],
    observation: &ChunkedWorkerObservation,
) -> std::io::Result<()> {
    socket
        .write_all(format!("{:X}\r\n", chunk.len()).as_bytes())
        .await?;
    socket.write_all(chunk).await?;
    socket.write_all(b"\r\n").await?;
    socket.flush().await?;
    observation
        .body_bytes_sent
        .fetch_add(chunk.len(), Ordering::SeqCst);
    observation.fragments_sent.fetch_add(1, Ordering::SeqCst);
    observation.first_fragment_sent.notify_one();
    Ok(())
}

fn valid_chunked_response_headers(headers: &[u8]) -> bool {
    let headers = String::from_utf8_lossy(headers).to_ascii_lowercase();
    headers.contains("transfer-encoding: chunked") && !headers.contains("content-length:")
}

#[tokio::test]
async fn cold_http_rejects_valid_chunked_worker_larger_than_body_limit() {
    let directory = tempfile::Builder::new()
        .prefix("cold-audit-streamed-worker-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = Store::open_for_tenant(
        &directory.path().join("streamed-worker.sqlite"),
        "tenant-streamed-worker",
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let mut worker = seed_worker(&store);
    worker.unknown_reason = Some("u".repeat(600 * 1024));
    assert!(Uuid::parse_str(&worker.worker_id).is_ok());
    assert_eq!(worker.launch.version, 1);
    assert_eq!(worker.receipt.version, 1);
    let body = serde_json::to_vec(&worker).unwrap();
    assert!(body.len() > MAX_RESPONSE_BYTES);
    assert_eq!(
        serde_json::from_slice::<crate::control::WorkerV1>(&body)
            .unwrap()
            .worker_id,
        worker.worker_id,
        "the oversized JSON body is an otherwise valid WorkerV1 with matching identity"
    );

    let worker_id = worker.worker_id.clone();
    let mut peer = chunked_worker_peer(body, false).await;
    let client = AuditClient::new(&peer.origin, "synthetic-stream-runner-token-cccccccc").unwrap();
    let budget = ColdAuditBudget::start();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        client.cold_worker(&worker_id, &budget),
    )
    .await;
    peer.close().await;

    assert_eq!(
        result.expect("10-second test watchdog").err(),
        Some(ColdAuditHttpError::InvalidResponse),
        "a valid chunked WorkerV1 over the 512-KiB cap is rejected"
    );
    assert_eq!(peer.observation.requests.load(Ordering::SeqCst), 1);
    assert!(peer.observation.worker_get.load(Ordering::SeqCst));
    assert!(
        valid_chunked_response_headers(&peer.observation.response_headers.lock().unwrap()),
        "the actual loopback response is chunked and has no Content-Length"
    );
    assert!(
        peer.observation.body_bytes_sent.load(Ordering::SeqCst) > MAX_RESPONSE_BYTES,
        "the peer streamed more than the response limit"
    );
    assert!(peer.observation.fragments_sent.load(Ordering::SeqCst) > 1);
}

#[tokio::test]
async fn cold_http_deadline_expiring_during_incomplete_worker_body_returns_expired() {
    let directory = tempfile::Builder::new()
        .prefix("cold-audit-stalled-worker-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = Store::open_for_tenant(
        &directory.path().join("stalled-worker.sqlite"),
        "tenant-stalled-worker",
        Redactor::new(vec![]),
        1000,
    )
    .unwrap();
    let worker = seed_worker(&store);
    let body = serde_json::to_vec(&worker).unwrap();
    let worker_id = worker.worker_id.clone();
    let mut peer = chunked_worker_peer(body, true).await;
    let client = AuditClient::new(&peer.origin, "synthetic-stall-runner-token-dddddddd").unwrap();
    let mut budget = ColdAuditBudget::start();
    budget.deadline = Instant::now() + Duration::from_secs(3);
    let shared_deadline = budget.deadline();
    let request_client = client.clone();
    let request_worker_id = worker_id.clone();
    let mut request = tokio::spawn(async move {
        let result = request_client
            .cold_worker(&request_worker_id, &budget)
            .await;
        (budget, result)
    });

    let first_fragment = tokio::time::timeout(
        Duration::from_secs(10),
        peer.observation.first_fragment_sent.notified(),
    )
    .await;
    let request_outcome = match tokio::time::timeout(Duration::from_secs(10), &mut request).await {
        Ok(joined) => Some(joined),
        Err(_) => {
            request.abort();
            let _ = request.await;
            None
        }
    };
    peer.close().await;

    assert!(
        first_fragment.is_ok(),
        "the peer sent the first response fragment"
    );
    let (budget, result) = request_outcome
        .expect("10-second task watchdog")
        .expect("HTTP request task did not panic");
    assert_eq!(
        result.err(),
        Some(ColdAuditHttpError::Expired),
        "the synthetic shared deadline expires while the body stream is stalled"
    );
    assert_eq!(budget.deadline(), shared_deadline);
    assert_eq!(peer.observation.requests.load(Ordering::SeqCst), 1);
    assert_eq!(peer.observation.fragments_sent.load(Ordering::SeqCst), 1);
    assert_eq!(peer.observation.body_bytes_sent.load(Ordering::SeqCst), 64);
    assert!(
        valid_chunked_response_headers(&peer.observation.response_headers.lock().unwrap()),
        "the stalled loopback response is chunked and has no Content-Length"
    );

    // The existing expiration parent checks that a locally expired budget
    // does not send; this assertion reuses the same object after body timeout.
    assert_eq!(
        client.cold_worker(&worker_id, &budget).await.err(),
        Some(ColdAuditHttpError::Expired),
        "the same budget remains expired after the body-read timeout"
    );
}
