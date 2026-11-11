//! Real HTTP/Axum/SQLite composition of the private admission SDK. Runtime facts
//! are synthetic and traverse the same strict helpers as worker_relay_api.
//! The only injected effects are a lost response body and a private SQLite lock.
use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    control::*,
    model::{CommandSpec, DecisionRequest},
    policy::{self, Policy},
    redaction::Redactor,
    relay::RelayEndpointV1,
    workload::{WorkloadBindingIssueV1, WorkloadBindingRevocationV1, WorkloadSubjectV1},
};
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::{self, Next},
    response::Response,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering},
    time::Duration,
};
use tokio::{sync::oneshot, task::JoinHandle, time::timeout};
use uuid::Uuid;

#[path = "../../../tests/support/relay_admission_fixture.rs"]
mod relay_admission_fixture;
#[path = "../../../tests/support/mod.rs"]
mod support;
use relay_admission_fixture::*;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
// Public synthetic material. No host configuration or real credential is read.
const A_RUNNER: &str = "sdk-admission-a-runner-public-fixture-0123456789";
const A_VIEWER: &str = "sdk-admission-a-viewer-public-fixture-0123456789";
const B_RUNNER: &str = "sdk-admission-b-runner-public-fixture-0123456789";
const B_VIEWER: &str = "sdk-admission-b-viewer-public-fixture-0123456789";
const A_WORKLOAD: &str = "sdk-admission-a-workload-public-fixture-0123456789";
const MATERIAL: [&str; 5] = [A_RUNNER, A_VIEWER, B_RUNNER, B_VIEWER, A_WORKLOAD];

struct Faults {
    a_database: PathBuf,
    lose_claim_response: AtomicBool,
    hold_claim_write: AtomicBool,
    lost_responses: AtomicUsize,
    claim_posts: AtomicUsize,
    locked_status: AtomicU16,
}

async fn fault_boundary(
    State(faults): State<Arc<Faults>>,
    request: Request,
    next: Next,
) -> Response {
    let claim = request.method() == Method::POST
        && request
            .uri()
            .path()
            .starts_with("/control/api/v1/relay/channels/")
        && request.uri().path().ends_with("/claims");
    let held = if claim && faults.hold_claim_write.swap(false, Ordering::SeqCst) {
        let db = Connection::open_with_flags(
            &faults.a_database,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        db.execute_batch("BEGIN IMMEDIATE").unwrap();
        Some(db)
    } else {
        None
    };
    if claim {
        faults.claim_posts.fetch_add(1, Ordering::SeqCst);
    }
    let response = next.run(request).await;
    if let Some(db) = held {
        faults
            .locked_status
            .store(response.status().as_u16(), Ordering::SeqCst);
        db.execute_batch("ROLLBACK").unwrap();
    }
    if claim
        && response.status() == StatusCode::CREATED
        && faults.lose_claim_response.swap(false, Ordering::SeqCst)
    {
        faults.lost_responses.fetch_add(1, Ordering::SeqCst);
        // The real gateway has returned after its durable commit. Do not create
        // any receipt or replacement JSON; fail only delivery of that real body.
        let (parts, _) = response.into_parts();
        let body = Body::from_stream(futures_util::stream::once(async {
            Err::<Vec<u8>, io::Error>(io::Error::from(io::ErrorKind::ConnectionReset))
        }));
        return Response::from_parts(parts, body);
    }
    response
}

struct Server {
    origin: String,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<io::Result<()>>,
}
impl Server {
    async fn start(app: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        });
        Self {
            origin,
            stop: Some(stop),
            task,
        }
    }
    async fn close(mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        timeout(Duration::from_secs(5), &mut self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.task.abort();
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    a: Arc<Store>,
    b: Arc<Store>,
    worker: WorkerV1,
    admission: RelayChannelAdmissionV1,
    claim: RelayChannelClaimV1,
    faults: Arc<Faults>,
    server: Server,
    client: AuditClient,
    b_client: AuditClient,
}
impl Fixture {
    async fn new() -> Self {
        let directory = support::private_tempdir();
        let a = Arc::new(fresh(directory.path(), "tenant-a", 10000));
        let b = Arc::new(fresh(directory.path(), "tenant-b", 10000));
        let worker = running_now(&a);
        let (credential, revision) = bind(&a, &worker, A_WORKLOAD);
        let admission: RelayChannelAdmissionV1 =
            serde_json::from_value(request(&a, &worker, &credential, revision, A_WORKLOAD))
                .unwrap();
        let claim = RelayChannelClaimV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            channel_id: admission.channel_id.clone(),
            epoch: admission.epoch,
            expected_revision: 1,
            admission_input_hash: admission.input_hash().unwrap(),
        };
        let state = GatewayState::for_tenants(
            vec![
                TenantGateway {
                    tenant_id: "tenant-a".into(),
                    store: a.clone(),
                    runner_token: A_RUNNER.into(),
                    viewer_token: A_VIEWER.into(),
                },
                TenantGateway {
                    tenant_id: "tenant-b".into(),
                    store: b.clone(),
                    runner_token: B_RUNNER.into(),
                    viewer_token: B_VIEWER.into(),
                },
            ],
            Policy::default(),
            None,
            Redactor::new(MATERIAL.iter().map(|s| (*s).to_owned()).collect()),
        )
        .unwrap();
        let faults = Arc::new(Faults {
            a_database: directory.path().join("tenant-a.sqlite"),
            lose_claim_response: AtomicBool::new(false),
            hold_claim_write: AtomicBool::new(false),
            lost_responses: AtomicUsize::new(0),
            claim_posts: AtomicUsize::new(0),
            locked_status: AtomicU16::new(0),
        });
        let app = gateway_router(state).layer(middleware::from_fn_with_state(
            faults.clone(),
            fault_boundary,
        ));
        let server = Server::start(app).await;
        let client = AuditClient::new(&server.origin, A_RUNNER).unwrap();
        let b_client = AuditClient::new(&server.origin, B_RUNNER).unwrap();
        Self {
            directory,
            a,
            b,
            worker,
            admission,
            claim,
            faults,
            server,
            client,
            b_client,
        }
    }
    fn observer(&self) -> Connection {
        Connection::open_with_flags(
            self.directory.path().join("tenant-a.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
    }
    fn claim_rows(&self) -> Vec<(i64, String, Value)> {
        let db = self.observer();
        let mut statement = db
            .prepare(
                "SELECT seq,hash,payload FROM events WHERE kind='relay_channel.v1' ORDER BY seq",
            )
            .unwrap();
        statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .unwrap()
            .map(|row| {
                let (seq, hash, payload) = row.unwrap();
                (seq, hash, serde_json::from_str::<Value>(&payload).unwrap())
            })
            .filter(|(_, _, value)| value["data"]["input"]["action"] == "claim")
            .collect()
    }
    fn assert_no_credential_persisted(&self) {
        for tenant in ["tenant-a", "tenant-b"] {
            let db = Connection::open_with_flags(
                self.directory.path().join(format!("{tenant}.sqlite")),
                OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let mut statement = db.prepare("SELECT payload FROM events").unwrap();
            for row in statement.query_map([], |r| r.get::<_, String>(0)).unwrap() {
                let payload = row.unwrap();
                for credential in MATERIAL {
                    assert!(
                        !payload.contains(credential),
                        "raw fixture credential persisted in audit"
                    );
                }
            }
        }
    }
}
fn rejected<T>(result: Result<T>) -> Error {
    match result {
        Err(error) => error,
        Ok(_) => panic!("refused authority unexpectedly produced success"),
    }
}
fn only_fresh(result: Result<RelayExecClaimResult>) -> FreshRelayExecClaim {
    match result.unwrap() {
        RelayExecClaimResult::Fresh(value) => value,
        RelayExecClaimResult::Replayed(_) => panic!("first successful claim was not fresh"),
    }
}
fn assert_opaque(error: Error) {
    let diagnostic = format!("{error:?}: {error}");
    for credential in MATERIAL {
        assert!(!diagnostic.contains(credential));
    }
}

#[tokio::test]
async fn sdk_http_fresh_claim_current_scope_and_revocation() {
    let f = Fixture::new().await;
    let reserved = f
        .client
        .reserve_relay_channel("tenant-a", &f.admission)
        .await
        .unwrap();
    assert_eq!(reserved.channel.state, RelayChannelStateV1::Reserved);
    let fresh = only_fresh(
        f.client
            .claim_relay_channel_exec("tenant-a", &f.admission, &f.claim)
            .await,
    );
    assert_eq!(fresh.admission(), &f.admission);
    assert_eq!(fresh.receipt().input_hash, f.claim.input_hash().unwrap());
    assert_eq!(format!("{fresh:?}"), "FreshRelayExecClaim([private])");
    let authority = fresh.current_authority();
    assert_eq!(
        f.client
            .current_relay_channel(&authority)
            .await
            .unwrap()
            .state,
        RelayChannelStateV1::AcceptedBeforeExec
    );
    let head = f.a.head().unwrap();
    match f
        .client
        .claim_relay_channel_exec("tenant-a", &f.admission, &f.claim)
        .await
        .unwrap()
    {
        RelayExecClaimResult::Replayed(value) => assert_eq!(value.receipt, fresh.receipt().clone()),
        RelayExecClaimResult::Fresh(_) => panic!("claim replay minted a second exec capability"),
    }
    assert_eq!(f.a.head().unwrap(), head);
    let ready = RelayChannelTransitionV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        channel_id: f.admission.channel_id.clone(),
        epoch: f.admission.epoch,
        expected_revision: 2,
        admission_input_hash: f.admission.input_hash().unwrap(),
        transition: RelayChannelTransitionKindV1::Ready,
    };
    assert_eq!(
        f.client
            .transition_relay_channel("tenant-a", &f.admission, &ready)
            .await
            .unwrap()
            .channel
            .state,
        RelayChannelStateV1::Ready
    );
    assert_eq!(
        f.client
            .current_relay_channel(&authority)
            .await
            .unwrap()
            .state,
        RelayChannelStateV1::Ready
    );
    let posts = f.faults.claim_posts.load(Ordering::SeqCst);
    let b_head = f.b.head().unwrap();
    let wrong_scope = rejected(f.b_client.current_relay_channel(&authority).await);
    assert_eq!(wrong_scope, Error::Forbidden);
    assert_opaque(wrong_scope);
    assert_eq!(
        f.faults.claim_posts.load(Ordering::SeqCst),
        posts,
        "mismatched authenticated identity reached the claim route"
    );
    assert_eq!(f.b.head().unwrap(), b_head);
    f.a.revoke_workload_binding(&WorkloadBindingRevocationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: f.admission.credential_id.clone(),
        expected_revision: f.admission.binding_revision,
    })
    .unwrap();
    let revoked_head = f.a.head().unwrap();
    let error = rejected(f.client.current_relay_channel(&authority).await);
    assert_eq!(error, Error::Forbidden);
    assert_opaque(error);
    assert_eq!(f.a.head().unwrap(), revoked_head);
    assert_eq!(f.claim_rows().len(), 1);
    f.assert_no_credential_persisted();
    f.server.close().await;
}

#[tokio::test]
async fn sdk_http_lost_claim_response_replays_without_fresh_capability() {
    let f = Fixture::new().await;
    f.client
        .reserve_relay_channel("tenant-a", &f.admission)
        .await
        .unwrap();
    f.faults.lose_claim_response.store(true, Ordering::SeqCst);
    let error = rejected(
        f.client
            .claim_relay_channel_exec("tenant-a", &f.admission, &f.claim)
            .await,
    );
    assert!(matches!(error, Error::Unavailable | Error::Protocol));
    assert_opaque(error);
    assert_eq!(f.faults.lost_responses.load(Ordering::SeqCst), 1);
    let committed = f.claim_rows();
    assert_eq!(
        committed.len(),
        1,
        "response loss must follow the real Store claim commit"
    );
    assert_eq!(
        committed[0].2["data"]["input"]["request"]["operation_id"],
        f.claim.operation_id
    );
    let head = f.a.head().unwrap();
    match f
        .client
        .claim_relay_channel_exec("tenant-a", &f.admission, &f.claim)
        .await
        .unwrap()
    {
        RelayExecClaimResult::Replayed(value) => {
            assert_eq!(value.receipt.audit_seq, committed[0].0);
            assert_eq!(value.receipt.audit_hash, committed[0].1);
            assert_eq!(value.channel.state, RelayChannelStateV1::AcceptedBeforeExec);
        }
        RelayExecClaimResult::Fresh(_) => panic!("lost response replay minted an exec capability"),
    }
    assert_eq!(f.a.head().unwrap(), head);
    assert_eq!(f.claim_rows().len(), 1);
    let mut replacement = f.claim.clone();
    replacement.operation_id = Uuid::new_v4().to_string();
    assert_eq!(
        rejected(
            f.client
                .claim_relay_channel_exec("tenant-a", &f.admission, &replacement)
                .await
        ),
        Error::Conflict
    );
    assert_eq!(f.a.head().unwrap(), head);
    f.assert_no_credential_persisted();
    f.server.close().await;
}

#[tokio::test]
async fn sdk_http_sqlite_write_unavailability_cannot_mint_claim() {
    let f = Fixture::new().await;
    f.client
        .reserve_relay_channel("tenant-a", &f.admission)
        .await
        .unwrap();
    let before = f.a.head().unwrap();
    assert!(
        f.client
            .control_supervision_identity()
            .await
            .unwrap()
            .active
    );
    // A real independent SQLite writer makes the next ClaimExec request's
    // checked transaction unavailable. It holds no mutation, then rolls back.
    // This is precommit contention, not an EIO/ENOSPC or power-loss simulation.
    f.faults.hold_claim_write.store(true, Ordering::SeqCst);
    let error = rejected(
        timeout(
            Duration::from_secs(20),
            f.client
                .claim_relay_channel_exec("tenant-a", &f.admission, &f.claim),
        )
        .await
        .unwrap(),
    );
    assert_eq!(error, Error::Unavailable);
    assert_opaque(error);
    assert_eq!(
        f.faults.locked_status.load(Ordering::SeqCst),
        StatusCode::SERVICE_UNAVAILABLE.as_u16()
    );
    assert_eq!(f.a.head().unwrap(), before);
    assert!(f.claim_rows().is_empty());
    let replay = f
        .client
        .reserve_relay_channel("tenant-a", &f.admission)
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.channel.state, RelayChannelStateV1::Reserved);
    assert_eq!(
        f.a.worker(&f.worker.worker_id).unwrap().unwrap().state,
        WorkerStateV1::Running
    );
    assert!(f.a.worker(&f.worker.worker_id).unwrap().unwrap().quota_held);
    let fresh = only_fresh(
        f.client
            .claim_relay_channel_exec("tenant-a", &f.admission, &f.claim)
            .await,
    );
    assert_eq!(fresh.receipt().audit_seq, before + 1);
    assert_eq!(f.claim_rows().len(), 1);
    assert!(
        f.b_client
            .control_supervision_identity()
            .await
            .unwrap()
            .active
    );
    f.assert_no_credential_persisted();
    f.server.close().await;
}

fn history_request(f: &Fixture, pending: Value) -> Value {
    json!({"version":1,"admission":f.admission,"pending":pending})
}
async fn history_post(f: &Fixture, token: &str, request: &Value) -> (StatusCode, Value) {
    let response = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(12))
        .build()
        .unwrap()
        .post(format!(
            "{}control/api/v1/relay/channels/{}/reconcile",
            f.server.origin, f.admission.channel_id
        ))
        .bearer_auth(token)
        .json(request)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.bytes().await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
fn terminal_worker(f: &Fixture) -> WorkerV1 {
    let mut terminal = f.worker.last_observation.clone().unwrap();
    terminal.observed_at = chrono::Utc::now().to_rfc3339();
    terminal.status = RuntimeStatusV1::Exited;
    terminal.running = Some(false);
    terminal.finished_at = Some(chrono::Utc::now().to_rfc3339());
    terminal.exit_code = Some(0);
    f.a.mutate_worker(&mutation(
        &f.worker,
        WorkerTransitionV1::Observe {
            observation: terminal,
        },
    ))
    .unwrap()
    .worker
}

// RED oracle: the route is currently absent. These requests must become pure
// historical lookups; substituting Reserve/Claim fails after terminal/revoke.
#[tokio::test]
async fn history_http_records_exact_pending_after_terminal_and_revocation_without_claim() {
    let f = Fixture::new().await;
    let admission = f.a.reserve_relay_channel(&f.admission).unwrap();
    let claim = f.a.claim_relay_channel_exec(&f.claim).unwrap();
    let terminal = terminal_worker(&f);
    assert_eq!(terminal.state, WorkerStateV1::Terminal);
    f.a.revoke_workload_binding(&WorkloadBindingRevocationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: f.admission.credential_id.clone(),
        expected_revision: f.admission.binding_revision,
    })
    .unwrap();
    let head = f.a.head().unwrap();
    let pending = json!({"kind":"claim","request":f.claim});
    let (status, value) = history_post(&f, A_RUNNER, &history_request(&f, pending.clone())).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "historical route unavailable: {value}"
    );
    assert_eq!(value["tenant_id"], "tenant-a");
    assert_eq!(value["outcome"]["kind"], "historical");
    assert_eq!(
        value["outcome"]["channel"],
        serde_json::to_value(claim.channel).unwrap()
    );
    assert_eq!(
        value["outcome"]["admission_receipt"],
        serde_json::to_value(admission.receipt).unwrap()
    );
    assert_eq!(value["outcome"]["pending"]["pending"], pending);
    assert_eq!(
        value["outcome"]["pending"]["resolution"]["kind"],
        "recorded"
    );
    assert_eq!(
        value["outcome"]["pending"]["resolution"]["receipt"],
        serde_json::to_value(claim.receipt).unwrap()
    );
    assert_eq!(f.a.head().unwrap(), head);
    assert_eq!(f.claim_rows().len(), 1);
    assert_eq!(f.faults.claim_posts.load(Ordering::SeqCst), 0);
    assert!(f.a.reserve_relay_channel(&f.admission).is_err());
    assert!(f.a.claim_relay_channel_exec(&f.claim).is_err());
    f.server.close().await;
}

#[tokio::test]
async fn history_http_pending_is_unresolved_until_same_snapshot_proves_cas_inapplicable() {
    let f = Fixture::new().await;
    f.a.reserve_relay_channel(&f.admission).unwrap();
    let pending = json!({"kind":"claim","request":f.claim});
    let request = history_request(&f, pending.clone());
    let before = f.a.head().unwrap();
    let (status, unresolved) = history_post(&f, A_RUNNER, &request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        unresolved["outcome"]["pending"]["resolution"]["kind"],
        "unresolved"
    );
    assert_eq!(f.a.head().unwrap(), before);
    let cancel = RelayChannelTransitionV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        channel_id: f.admission.channel_id.clone(),
        epoch: f.admission.epoch,
        expected_revision: 1,
        admission_input_hash: f.admission.input_hash().unwrap(),
        transition: RelayChannelTransitionKindV1::CancelReserved,
    };
    let cancelled = f.a.transition_relay_channel(&cancel).unwrap();
    let head = f.a.head().unwrap();
    let (status, resolved) = history_post(&f, A_RUNNER, &request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resolved["outcome"]["pending"]["pending"], pending);
    assert_eq!(
        resolved["outcome"]["pending"]["resolution"]["kind"],
        "inapplicable_cas"
    );
    assert_eq!(
        resolved["outcome"]["channel"]["revision"],
        cancelled.channel.revision
    );
    assert!(
        resolved["outcome"]["pending"]["resolution"]
            .get("receipt")
            .is_none()
    );
    let transition = history_request(&f, json!({"kind":"transition","request":cancel}));
    let (_, recorded) = history_post(&f, A_RUNNER, &transition).await;
    assert_eq!(
        recorded["outcome"]["pending"]["resolution"]["receipt"],
        serde_json::to_value(cancelled.receipt).unwrap()
    );
    let mut divergent = transition;
    divergent["pending"]["request"]["transition"]["kind"] = json!("known_refused_before_spawn");
    assert_eq!(
        history_post(&f, A_RUNNER, &divergent).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(f.a.head().unwrap(), head);
    assert!(f.claim_rows().is_empty());
    f.server.close().await;
}

#[tokio::test]
async fn history_http_absence_roles_scope_and_divergence_remain_conservative() {
    let f = Fixture::new().await;
    let request = history_request(&f, Value::Null);
    let before = f.a.head().unwrap();
    let (status, absent) = history_post(&f, A_RUNNER, &request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(absent["outcome"]["kind"], "admission_unresolved");
    for token in [A_VIEWER, A_WORKLOAD, B_RUNNER, "unknown-public-fixture"] {
        let status = history_post(&f, token, &request).await.0;
        assert!(matches!(
            status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ));
    }
    let mut wrong = request.clone();
    wrong["admission"]["control_namespace_uuid"] = json!(Uuid::new_v4().to_string());
    assert_eq!(
        history_post(&f, A_RUNNER, &wrong).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(f.a.head().unwrap(), before);
    f.a.reserve_relay_channel(&f.admission).unwrap();
    let head = f.a.head().unwrap();
    for field in ["epoch", "authority_binding_hash"] {
        let mut wrong = request.clone();
        wrong["admission"][field] = if field == "epoch" {
            json!(2)
        } else {
            json!(HASH_C)
        };
        assert_eq!(
            history_post(&f, A_RUNNER, &wrong).await.0,
            StatusCode::CONFLICT
        );
    }
    let mut free_tenant = request;
    free_tenant["tenant_id"] = json!("tenant-b");
    assert_eq!(
        history_post(&f, A_RUNNER, &free_tenant).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(f.a.head().unwrap(), head);
    assert!(f.claim_rows().is_empty());
    f.server.close().await;
}

#[tokio::test]
async fn history_http_absent_admission_requires_exact_terminal_worker_snapshot() {
    let f = Fixture::new().await;
    let terminal = terminal_worker(&f);
    let request = history_request(&f, Value::Null);
    let head = f.a.head().unwrap();
    let (status, proof) = history_post(&f, A_RUNNER, &request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        proof["outcome"]["kind"],
        "admission_never_recorded_exact_terminal"
    );
    assert_eq!(proof["outcome"]["worker_revision"], terminal.revision);
    assert_eq!(
        proof["outcome"]["endpoint"],
        serde_json::to_value(&f.admission.endpoint).unwrap()
    );
    assert_eq!(
        proof["outcome"]["admission_operation_id"],
        f.admission.operation_id
    );
    assert!(
        proof["outcome"]["terminal_observation_receipt"]["audit_seq"]
            .as_i64()
            .unwrap()
            > 0
    );
    assert!(
        proof["audit_seq"].as_i64().unwrap()
            >= proof["outcome"]["terminal_observation_receipt"]["audit_seq"]
                .as_i64()
                .unwrap()
    );
    for field in ["session_id", "endpoint", "inspection", "started_at"] {
        let mut wrong = request.clone();
        match field {
            "session_id" => wrong["admission"][field] = json!(Uuid::new_v4().to_string()),
            "endpoint" => {
                wrong["admission"][field]["attempt_id"] = json!(Uuid::new_v4().to_string())
            }
            "inspection" => wrong["admission"][field]["container_id"] = json!("e".repeat(64)),
            _ => wrong["admission"][field] = json!("2026-10-30T12:00:00Z"),
        }
        let (status, unknown) = history_post(&f, A_RUNNER, &wrong).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            unknown["outcome"]["kind"], "admission_unresolved",
            "mismatch {field} produced proof"
        );
    }
    assert!(f.a.reserve_relay_channel(&f.admission).is_err());
    assert_eq!(f.a.head().unwrap(), head);
    assert!(f.claim_rows().is_empty());
    f.server.close().await;
}

#[tokio::test]
async fn history_sdk_reads_recorded_claim_in_inactive_namespace_without_fresh_authority() {
    let f = Fixture::new().await;
    let reserved = f.a.reserve_relay_channel(&f.admission).unwrap();
    let claimed = f.a.claim_relay_channel_exec(&f.claim).unwrap();
    terminal_worker(&f);
    f.a.revoke_workload_binding(&WorkloadBindingRevocationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        credential_id: f.admission.credential_id.clone(),
        expected_revision: f.admission.binding_revision,
    })
    .unwrap();
    let Fixture {
        directory,
        a,
        b,
        admission,
        claim,
        server,
        ..
    } = f;
    server.close().await;
    drop(a);
    drop(b);
    let store = Arc::new(
        Store::open_for_tenant(
            &directory.path().join("tenant-a.sqlite"),
            "tenant-a",
            Redactor::new(vec![]),
            10000,
        )
        .unwrap(),
    );
    assert!(!store.supervision_state().unwrap().active);
    let state = GatewayState::for_tenants(
        vec![TenantGateway {
            tenant_id: "tenant-a".into(),
            store: store.clone(),
            runner_token: A_RUNNER.into(),
            viewer_token: A_VIEWER.into(),
        }],
        Policy::default(),
        None,
        Redactor::new(vec![]),
    )
    .unwrap();
    let server = Server::start(gateway_router(state)).await;
    let client = AuditClient::new(&server.origin, A_RUNNER).unwrap();
    let viewer = AuditClient::new(&server.origin, A_VIEWER).unwrap();
    let request = RelayChannelReconcileV1 {
        version: 1,
        admission: admission.clone(),
        pending: Some(RelayChannelPendingV1::Claim(claim.clone())),
    };
    let head = store.head().unwrap();
    let history = client.reconcile_relay_channel(&request).await.unwrap();
    let RelayChannelHistoryOutcomeV1::Historical {
        channel,
        admission_receipt,
        pending: Some(pending),
    } = history.outcome
    else {
        panic!("exact recorded claim was not historical");
    };
    assert_eq!(channel.state, RelayChannelStateV1::AcceptedBeforeExec);
    assert_eq!(admission_receipt, reserved.receipt);
    assert_eq!(pending.pending, RelayChannelPendingV1::Claim(claim.clone()));
    assert_eq!(
        pending.resolution,
        RelayChannelPendingResolutionV1::Recorded {
            receipt: claimed.receipt
        }
    );
    let mut absent = request.clone();
    absent.admission.operation_id = Uuid::new_v4().to_string();
    absent.admission.channel_id = Uuid::new_v4().to_string();
    absent.pending = None;
    let proof = client.reconcile_relay_channel(&absent).await.unwrap();
    assert!(matches!(
        &proof.outcome,
        RelayChannelHistoryOutcomeV1::AdmissionNeverRecordedExactTerminal { .. }
    ));
    let mut wrong = proof.clone();
    let RelayChannelHistoryOutcomeV1::AdmissionNeverRecordedExactTerminal {
        terminal_observation_receipt,
        ..
    } = &mut wrong.outcome
    else {
        unreachable!();
    };
    terminal_observation_receipt.worker_id = Some(Uuid::new_v4().to_string());
    assert_eq!(
        history_context(&wrong, "tenant-a", &absent),
        Err(Error::Protocol)
    );
    assert_eq!(
        rejected(viewer.reconcile_relay_channel(&request).await),
        Error::Forbidden
    );
    assert_eq!(
        rejected(
            client
                .claim_relay_channel_exec("tenant-a", &admission, &claim)
                .await
        ),
        Error::Forbidden
    );
    assert_eq!(store.head().unwrap(), head);
    server.close().await;
}

#[tokio::test]
async fn history_sdk_rejects_divergent_pending_scope_and_receipts_outside_snapshot() {
    let f = Fixture::new().await;
    f.a.reserve_relay_channel(&f.admission).unwrap();
    f.a.claim_relay_channel_exec(&f.claim).unwrap();
    let request = RelayChannelReconcileV1 {
        version: 1,
        admission: f.admission.clone(),
        pending: Some(RelayChannelPendingV1::Claim(f.claim.clone())),
    };
    let history = f.a.reconcile_relay_channel(&request).unwrap();
    history_context(&history, "tenant-a", &request).unwrap();
    for field in [
        "tenant_id",
        "control_namespace_uuid",
        "admission_input_hash",
        "audit_seq",
        "audit_hash",
    ] {
        let mut value = serde_json::to_value(&history).unwrap();
        value[field] = match field {
            "tenant_id" => json!("tenant-b"),
            "control_namespace_uuid" => json!(Uuid::new_v4().to_string()),
            "audit_seq" => json!(0),
            _ => json!(HASH_C),
        };
        let wrong: RelayChannelReconciliationV1 = serde_json::from_value(value).unwrap();
        assert_eq!(
            history_context(&wrong, "tenant-a", &request),
            Err(Error::Protocol),
            "field {field}"
        );
    }
    let mut wrong = history.clone();
    let RelayChannelHistoryOutcomeV1::Historical {
        pending: Some(pending),
        ..
    } = &mut wrong.outcome
    else {
        panic!("expected pending");
    };
    if let RelayChannelPendingV1::Claim(claim) = &mut pending.pending {
        claim.operation_id = Uuid::new_v4().to_string();
    }
    assert_eq!(
        history_context(&wrong, "tenant-a", &request),
        Err(Error::Protocol)
    );
    let mut wrong = history;
    let RelayChannelHistoryOutcomeV1::Historical {
        pending: Some(pending),
        ..
    } = &mut wrong.outcome
    else {
        panic!("expected pending");
    };
    let RelayChannelPendingResolutionV1::Recorded { receipt } = &mut pending.resolution else {
        panic!("expected recorded receipt");
    };
    receipt.audit_seq += 1;
    assert_eq!(
        history_context(&wrong, "tenant-a", &request),
        Err(Error::Protocol)
    );
    f.server.close().await;
}

#[tokio::test]
async fn history_sdk_resolves_old_terminal_close_at_reserved_cas_without_replacing_intention() {
    let f = Fixture::new().await;
    f.a.reserve_relay_channel(&f.admission).unwrap();
    let terminal = terminal_worker(&f);
    let terminal_observation_id: String = f
        .observer()
        .query_row(
            "SELECT terminal_observation_id FROM worker_allocations WHERE attempt_id=?1",
            [&f.admission.endpoint.attempt_id],
            |row| row.get(0),
        )
        .unwrap();
    let old_close = RelayChannelTransitionV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        channel_id: f.admission.channel_id.clone(),
        epoch: f.admission.epoch,
        expected_revision: 1,
        admission_input_hash: f.admission.input_hash().unwrap(),
        transition: RelayChannelTransitionKindV1::CloseExactWorkerTerminal {
            worker_revision: terminal.revision,
            terminal_observation_id,
        },
    };
    let request = RelayChannelReconcileV1 {
        version: 1,
        admission: f.admission.clone(),
        pending: Some(RelayChannelPendingV1::Transition(old_close.clone())),
    };
    let head = f.a.head().unwrap();
    let proof = f.client.reconcile_relay_channel(&request).await.unwrap();
    let value = serde_json::to_value(&proof).unwrap();
    assert_eq!(value["outcome"]["channel"]["state"], "reserved");
    assert_eq!(value["outcome"]["channel"]["revision"], 1);
    assert_eq!(
        value["outcome"]["pending"]["pending"]["request"],
        serde_json::to_value(&old_close).unwrap()
    );
    assert_eq!(
        value["outcome"]["pending"]["resolution"]["kind"],
        "inapplicable_state"
    );
    assert!(
        value["outcome"]["pending"]["resolution"]
            .get("receipt")
            .is_none()
    );
    assert!(f.a.transition_relay_channel(&old_close).is_err());
    assert_eq!(f.a.head().unwrap(), head);
    let cancel = RelayChannelTransitionV1 {
        operation_id: Uuid::new_v4().to_string(),
        transition: RelayChannelTransitionKindV1::CancelReserved,
        ..old_close.clone()
    };
    let cancelled = f.a.transition_relay_channel(&cancel).unwrap();
    assert_eq!(cancelled.channel.revision, 2);
    let head = f.a.head().unwrap();
    let proof = f.client.reconcile_relay_channel(&request).await.unwrap();
    let value = serde_json::to_value(proof).unwrap();
    assert_eq!(
        value["outcome"]["pending"]["pending"]["request"],
        serde_json::to_value(old_close).unwrap()
    );
    assert_eq!(
        value["outcome"]["pending"]["resolution"]["kind"],
        "inapplicable_cas"
    );
    assert_eq!(f.a.head().unwrap(), head);
    assert!(f.claim_rows().is_empty());
    f.server.close().await;
}
