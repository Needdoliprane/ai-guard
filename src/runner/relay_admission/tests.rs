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
