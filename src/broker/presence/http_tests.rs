//! Real gateway/SQLite regressions. Faults affect only the HTTP exchange.
#[path = "cost_tests.rs"]
mod cost_tests;

use super::preparation_tests::{Fixture, OWN_TOKEN};
use super::{PreparedPresence, PresenceError, PresenceScope};
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::{AuditCapacityV1, Store, WorkloadAuditPolicyV1},
    broker_registry::{
        BrokerAuthorityV1, BrokerGrantMutationV1, BrokerHeartbeatV1, BrokerRevocationV1,
        BrokerRuntimeKindV1,
    },
    policy::Policy,
    redaction::Redactor,
    workload::{AuthenticatedWorkload, WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, Response},
    middleware::{self, Next},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    sync::{Semaphore, watch},
    task::JoinHandle,
};
use uuid::Uuid;

struct Probe {
    state: Mutex<PathBuf>,
    requests: Mutex<Vec<BrokerHeartbeatV1>>,
    statuses: Mutex<Vec<u16>>,
    lose_ack: AtomicBool,
    tamper_ack: AtomicBool,
    temporary_refusals: Mutex<usize>,
    temporary_lost_acks: Mutex<usize>,
    faults_observed: Semaphore,
    post_times: Mutex<Vec<Instant>>,
    identity_refusal: Mutex<Option<(u16, &'static str)>>,
    identity_requests: Mutex<usize>,
    transport_paths: Mutex<Vec<String>>,
    transport_refusal: Mutex<Option<u16>>,
}

fn consume_fault(remaining: &Mutex<usize>) -> bool {
    let mut remaining = remaining.lock().unwrap();
    if *remaining == 0 {
        false
    } else {
        *remaining -= 1;
        true
    }
}

fn record_paths(state: &std::path::Path, leaf: &str) -> Vec<PathBuf> {
    fs::read_dir(state)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .map(|path| path.join(leaf))
        .filter(|path| path.exists())
        .collect()
}

async fn inspect_post(probe: Arc<Probe>, request: Request<Body>, next: Next) -> Response<Body> {
    let path = request.uri().path();
    probe.transport_paths.lock().unwrap().push(path.into());
    if path.starts_with("/control/")
        && let Some(status) = *probe.transport_refusal.lock().unwrap()
    {
        return Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
    }
    if path == "/control/api/v1/brokers/identity" {
        *probe.identity_requests.lock().unwrap() += 1;
        let response = next.run(request).await;
        let refusal = *probe.identity_refusal.lock().unwrap();
        if let Some((status, code)) = refusal {
            assert_eq!(
                response.status(),
                200,
                "real Broker authentication precedes the HTTP fault"
            );
            return Response::builder()
                .status(status)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"error":{"code":code}}).to_string(),
                ))
                .unwrap();
        }
        return response;
    }
    if request.uri().path() != "/control/api/v1/brokers/heartbeat" {
        return next.run(request).await;
    }
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 16_384).await.unwrap();
    let heartbeat: BrokerHeartbeatV1 = serde_json::from_slice(&bytes).unwrap();
    // Count the attempted POST before assertions: a broken preparation cannot
    // disappear from the test's observations when this middleware refuses it.
    probe.requests.lock().unwrap().push(heartbeat.clone());
    probe.post_times.lock().unwrap().push(Instant::now());
    // The break caught here is POST-before-persist. This reads the real record
    // from the retained stream namespace before the actual gateway sees it.
    let state = probe.state.lock().unwrap().clone();
    let pending = record_paths(&state, "pending.json");
    let durable: Value = pending
        .iter()
        .map(|path| serde_json::from_slice::<Value>(&fs::read(path).unwrap()).unwrap())
        .find(|record| record["heartbeat"]["operation_id"] == heartbeat.operation_id)
        .expect("the exact POST must already have its durable pending");
    assert_eq!(
        durable["heartbeat"],
        serde_json::to_value(&heartbeat).unwrap()
    );
    if consume_fault(&probe.temporary_refusals) {
        probe.statuses.lock().unwrap().push(503);
        // Refuse before the real mutation. The barrier counts actual requests,
        // rather than assuming four attempts from an elapsed sleep.
        probe.faults_observed.add_permits(1);
        return Response::builder()
            .status(503)
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
    }
    let response = next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await;
    probe
        .statuses
        .lock()
        .unwrap()
        .push(response.status().as_u16());
    if response.status() != 200 {
        return response;
    }
    if probe.lose_ack.load(Ordering::SeqCst) || consume_fault(&probe.temporary_lost_acks) {
        probe.faults_observed.add_permits(1);
        return Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(Body::from_stream(futures_util::stream::once(async {
                Err::<Vec<u8>, _>(std::io::Error::other("fixture lost committed ACK"))
            })))
            .unwrap();
    }
    if probe.tamper_ack.load(Ordering::SeqCst) {
        let (parts, body) = response.into_parts();
        let mut result: Value =
            serde_json::from_slice(&to_bytes(body, 16_384).await.unwrap()).unwrap();
        result["receipt"]["input_hash"] = Value::String("f".repeat(64));
        let mut response =
            Response::from_parts(parts, Body::from(serde_json::to_vec(&result).unwrap()));
        response.headers_mut().remove("content-length");
        return response;
    }
    response
}

struct LiveFixture {
    filesystem: Fixture,
    store: Arc<Store>,
    probe: Arc<Probe>,
    scope: PresenceScope,
    server: JoinHandle<()>,
}
impl Drop for LiveFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl LiveFixture {
    async fn new() -> Self {
        Self::with_workload_policy(None).await
    }
    async fn with_workload_policy(policy: Option<WorkloadAuditPolicyV1>) -> Self {
        let filesystem = Fixture::new();
        let db = filesystem.root.join("tenant.sqlite");
        let mut store =
            Store::open_for_tenant(&db, "fixture-tenant", Redactor::new(vec![]), 10_000).unwrap();
        if let Some(policy) = policy {
            store = store.with_workload_audit_policy(policy).unwrap();
        }
        let store = Arc::new(store);
        store
            .migrate_control_v3(&db.with_extension("before3"))
            .unwrap();
        store
            .migrate_identity_v4(&db.with_extension("before4"))
            .unwrap();
        store
            .migrate_human_access_v5(&db.with_extension("before5"))
            .unwrap();
        store
            .migrate_workload_v6(&db.with_extension("before6"))
            .unwrap();
        store
            .issue_workload_binding(&WorkloadBindingIssueV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                credential_id: filesystem.value["credential_id"].as_str().unwrap().into(),
                expected_revision: 0,
                subject: WorkloadSubjectV1::Broker {
                    broker_id: filesystem.value["broker_id"].as_str().unwrap().into(),
                    incarnation_id: filesystem.value["incarnation_id"].as_str().unwrap().into(),
                },
                sha256_digest: hex::encode(Sha256::digest(OWN_TOKEN.as_bytes())),
                expires_at: "2100-01-01T00:00:00.000Z".into(),
            })
            .unwrap();
        store
            .mutate_broker_grant(&BrokerGrantMutationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                broker_id: filesystem.value["broker_id"].as_str().unwrap().into(),
                expected_revision: None,
                credential_id: filesystem.value["credential_id"].as_str().unwrap().into(),
                incarnation_id: filesystem.value["incarnation_id"].as_str().unwrap().into(),
                authorities: vec![BrokerAuthorityV1 {
                    authority_id: "fixture-authority".into(),
                    runtime_kind: BrokerRuntimeKindV1::Docker,
                }],
                lease_seconds: 120,
                not_after: "2100-01-01T00:00:00.000Z".into(),
            })
            .unwrap();
        filesystem.write_config();
        let state = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: "fixture-tenant".into(),
                store: Arc::clone(&store),
                runner_token: "presence-fixture-runner-0123456789abcdef".into(),
                viewer_token: "presence-fixture-viewer-0123456789abcdef".into(),
            }],
            Policy::default(),
            None,
            Redactor::new(vec![OWN_TOKEN.into()]),
        )
        .unwrap();
        let probe = Arc::new(Probe {
            state: Mutex::new(filesystem.state.clone()),
            requests: Mutex::new(Vec::new()),
            statuses: Mutex::new(Vec::new()),
            lose_ack: AtomicBool::new(false),
            tamper_ack: AtomicBool::new(false),
            temporary_refusals: Mutex::new(0),
            temporary_lost_acks: Mutex::new(0),
            faults_observed: Semaphore::new(0),
            post_times: Mutex::new(Vec::new()),
            identity_refusal: Mutex::new(None),
            identity_requests: Mutex::new(0),
            transport_paths: Mutex::new(Vec::new()),
            transport_refusal: Mutex::new(None),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut scope = filesystem.scope();
        scope.gateway = format!("http://{}/", listener.local_addr().unwrap());
        scope.control_namespace_uuid = store
            .supervision_state()
            .unwrap()
            .control_namespace_uuid
            .unwrap()
            .parse()
            .expect("the real Store namespace is a canonical UUID");
        let middleware_probe = Arc::clone(&probe);
        let app = gateway_router(state).layer(middleware::from_fn(move |request, next| {
            inspect_post(Arc::clone(&middleware_probe), request, next)
        }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            filesystem,
            store,
            probe,
            scope,
            server,
        }
    }

    fn prepared(&self) -> PreparedPresence {
        let mut prepared =
            PreparedPresence::load(&self.filesystem.config, self.scope.clone(), vec![]).unwrap();
        prepared.finalize(&[]).unwrap();
        prepared
    }

    fn prepared_with_five_second_interval(&self) -> PreparedPresence {
        let mut config = self.filesystem.value.clone();
        config["interval_seconds"] = serde_json::json!(5);
        fs::write(
            &self.filesystem.config,
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        self.prepared()
    }
}

fn actor(
    prepared: PreparedPresence,
) -> (watch::Sender<bool>, JoinHandle<Result<(), PresenceError>>) {
    let (shutdown, receiver) = watch::channel(false);
    let task = tokio::spawn(prepared.run(
        receiver,
        Arc::new(Semaphore::new(4)),
        Arc::new(Semaphore::new(2)),
    ));
    (shutdown, task)
}

async fn wait_for_receipt(state: &std::path::Path) {
    wait_for_receipt_count(state, 1).await;
}
async fn wait_for_receipt_count(state: &std::path::Path, count: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if record_paths(state, "receipt.json").len() >= count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

async fn wait_for_faults(probe: &Probe, count: u32) -> bool {
    match tokio::time::timeout(
        Duration::from_secs(30),
        probe.faults_observed.acquire_many(count),
    )
    .await
    {
        Ok(Ok(permits)) => {
            permits.forget();
            true
        }
        _ => false,
    }
}

#[derive(Debug)]
enum RecoveryProgress {
    Receipt,
    Exited(std::result::Result<Result<(), PresenceError>, tokio::task::JoinError>),
    Watchdog,
}

async fn receipt_or_exit(
    state: &std::path::Path,
    task: &mut JoinHandle<Result<(), PresenceError>>,
) -> RecoveryProgress {
    match tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if !record_paths(state, "receipt.json").is_empty() {
                return RecoveryProgress::Receipt;
            }
            tokio::select! {
                result = &mut *task => return RecoveryProgress::Exited(result),
                _ = tokio::time::sleep(Duration::from_millis(20)) => {},
            }
        }
    })
    .await
    {
        Ok(progress) => progress,
        Err(_) => RecoveryProgress::Watchdog,
    }
}

async fn finish_recovery_actor(
    shutdown: &watch::Sender<bool>,
    task: &mut JoinHandle<Result<(), PresenceError>>,
    progress: &RecoveryProgress,
) -> bool {
    let _ = shutdown.send(true);
    if let RecoveryProgress::Exited(result) = progress {
        // The observing select already joined this handle. Do not poll it twice.
        return matches!(result, Ok(Ok(())));
    }
    match tokio::time::timeout(Duration::from_secs(5), &mut *task).await {
        Ok(result) => matches!(result, Ok(Ok(()))),
        Err(_) => {
            task.abort();
            let _ = task.await;
            false
        }
    }
}

#[tokio::test]
async fn temporary_gateway_refusals_resume_in_the_same_actor_after_the_fourth_failure() {
    let fixture = LiveFixture::new().await;
    // One further failure after the first burst distinguishes a slow retry
    // sequence from an incorrectly restarted 100/250/500ms burst.
    *fixture.probe.temporary_refusals.lock().unwrap() = 5;
    let before = fixture.store.head().unwrap();
    let (shutdown, mut task) = actor(fixture.prepared_with_five_second_interval());
    let four_failures = wait_for_faults(&fixture.probe, 4).await;
    let progress = receipt_or_exit(&fixture.filesystem.state, &mut task).await;
    let clean_stop = finish_recovery_actor(&shutdown, &mut task, &progress).await;

    // The old actor returns Unavailable at request four. A restarted actor or a
    // new operation cannot satisfy the live handle and exact-pending oracles.
    assert!(four_failures, "four actual HTTP refusals must be observed");
    assert!(
        matches!(progress, RecoveryProgress::Receipt) && clean_stop,
        "same actor must resume after four 503 responses; progress={progress:?}, clean_stop={clean_stop}"
    );
    assert_eq!(
        *fixture.probe.statuses.lock().unwrap(),
        vec![503, 503, 503, 503, 503, 200]
    );
    let requests = fixture.probe.requests.lock().unwrap();
    assert_eq!(requests.len(), 6);
    assert!(requests.iter().all(|request| request == &requests[0]));
    let times = fixture.probe.post_times.lock().unwrap();
    assert!(times[4].duration_since(times[3]) >= Duration::from_secs(5));
    assert!(times[5].duration_since(times[4]) >= Duration::from_secs(5));
    assert_eq!(fixture.store.head().unwrap(), before + 1);
    assert_eq!(
        record_paths(&fixture.filesystem.state, "pending.json").len(),
        1
    );
    assert_eq!(
        record_paths(&fixture.filesystem.state, "receipt.json").len(),
        1
    );
}

#[tokio::test]
async fn four_lost_committed_acks_resume_the_exact_pending_without_a_restart() {
    let fixture = LiveFixture::new().await;
    *fixture.probe.temporary_lost_acks.lock().unwrap() = 4;
    let before = fixture.store.head().unwrap();
    let (shutdown, mut task) = actor(fixture.prepared_with_five_second_interval());
    let four_failures = wait_for_faults(&fixture.probe, 4).await;
    let head_after_faults = fixture.store.head().unwrap();
    let progress = receipt_or_exit(&fixture.filesystem.state, &mut task).await;
    let clean_stop = finish_recovery_actor(&shutdown, &mut task, &progress).await;

    assert!(
        four_failures,
        "four real committed ACK losses must be observed"
    );
    assert_eq!(head_after_faults, before + 1);
    assert!(
        matches!(progress, RecoveryProgress::Receipt) && clean_stop,
        "same actor must recover its committed pending; progress={progress:?}, clean_stop={clean_stop}"
    );
    let requests = fixture.probe.requests.lock().unwrap();
    assert_eq!(requests.len(), 5);
    assert!(requests.iter().all(|request| request == &requests[0]));
    assert_eq!(*fixture.probe.statuses.lock().unwrap(), vec![200; 5]);
    assert_eq!(fixture.store.head().unwrap(), before + 1);
    let pending = record_paths(&fixture.filesystem.state, "pending.json");
    let receipts = record_paths(&fixture.filesystem.state, "receipt.json");
    assert_eq!(pending.len(), 1);
    assert_eq!(receipts.len(), 1);
    let retained_pending: Value = serde_json::from_slice(&fs::read(&pending[0]).unwrap()).unwrap();
    let retained_receipt: Value = serde_json::from_slice(&fs::read(&receipts[0]).unwrap()).unwrap();
    assert_eq!(
        retained_pending["heartbeat"]["operation_id"],
        requests[0].operation_id
    );
    assert_eq!(
        retained_receipt["receipt"]["operation_id"],
        requests[0].operation_id
    );
}

#[tokio::test]
async fn shutdown_during_slow_recovery_releases_permits_and_never_posts_after_join() {
    let fixture = LiveFixture::new().await;
    *fixture.probe.temporary_refusals.lock().unwrap() = 4;
    let before = fixture.store.head().unwrap();
    let attempts = Arc::new(Semaphore::new(1));
    let io = Arc::new(Semaphore::new(1));
    let (shutdown, receiver) = watch::channel(false);
    let mut task = tokio::spawn(fixture.prepared_with_five_second_interval().run(
        receiver,
        Arc::clone(&attempts),
        Arc::clone(&io),
    ));
    let four_failures = wait_for_faults(&fixture.probe, 4).await;
    let permits_released = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if attempts.available_permits() == 1 && io.available_permits() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .is_ok();
    let still_running = !task.is_finished();
    let progress = RecoveryProgress::Watchdog;
    let clean_stop = finish_recovery_actor(&shutdown, &mut task, &progress).await;
    let count_after_join = fixture.probe.requests.lock().unwrap().len();
    // Cross the configured retry cadence after join; a detached timer/HTTP
    // owner would produce a fifth POST and a real Store mutation here.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let count_after_interval = fixture.probe.requests.lock().unwrap().len();

    assert!(
        four_failures && still_running && permits_released && clean_stop,
        "backoff must keep the actor alive without permits; four_failures={four_failures}, still_running={still_running}, permits_released={permits_released}, clean_stop={clean_stop}"
    );
    assert_eq!(count_after_join, 4);
    assert_eq!(count_after_interval, count_after_join);
    assert_eq!(fixture.store.head().unwrap(), before);
    assert!(record_paths(&fixture.filesystem.state, "receipt.json").is_empty());
}

#[tokio::test]
async fn closed_refusals_and_changed_private_material_remain_terminal_without_retry() {
    for (status, code, expected) in [
        (401, "fixture_authentication", PresenceError::Authentication),
        (403, "fixture_permission", PresenceError::Forbidden),
        (409, "fixture_conflict", PresenceError::Conflict),
        (429, "fixture_capacity", PresenceError::Capacity),
        (503, "control_clock_uncertain", PresenceError::Clock),
        (503, "fixture_unknown", PresenceError::Protocol),
    ] {
        let fixture = LiveFixture::new().await;
        *fixture.probe.identity_refusal.lock().unwrap() = Some((status, code));
        let before = fixture.store.head().unwrap();
        let (shutdown, mut task) = actor(fixture.prepared());
        let actual = match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
            Ok(Ok(result)) => Some(result),
            Ok(Err(_)) => None,
            Err(_) => {
                let _ =
                    finish_recovery_actor(&shutdown, &mut task, &RecoveryProgress::Watchdog).await;
                None
            }
        };
        assert_eq!(actual, Some(Err(expected)), "status={status}, code={code}");
        assert_eq!(*fixture.probe.identity_requests.lock().unwrap(), 1);
        assert!(fixture.probe.requests.lock().unwrap().is_empty());
        assert!(record_paths(&fixture.filesystem.state, "pending.json").is_empty());
        assert_eq!(fixture.store.head().unwrap(), before);
    }

    let fixture = LiveFixture::new().await;
    let prepared = fixture.prepared();
    let before = fixture.store.head().unwrap();
    fs::write(
        fixture.filesystem.value["broker_token_file"]
            .as_str()
            .unwrap(),
        b"changed-private-fixture-bearer-0123456789abcdef",
    )
    .unwrap();
    let (shutdown, mut task) = actor(prepared);
    let actual = match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
        Ok(Ok(result)) => Some(result),
        Ok(Err(_)) => None,
        Err(_) => {
            let _ = finish_recovery_actor(&shutdown, &mut task, &RecoveryProgress::Watchdog).await;
            None
        }
    };
    assert_eq!(actual, Some(Err(PresenceError::UnsafeFilesystem)));
    assert_eq!(*fixture.probe.identity_requests.lock().unwrap(), 0);
    assert!(fixture.probe.requests.lock().unwrap().is_empty());
    assert_eq!(fixture.store.head().unwrap(), before);
}

#[tokio::test]
async fn committed_lost_ack_restart_replays_same_durable_operation_without_duplicate_audit() {
    let fixture = LiveFixture::new().await;
    let original_head = fixture.store.head().unwrap();
    fixture.probe.lose_ack.store(true, Ordering::SeqCst);
    let (first_shutdown, mut first) = actor(fixture.prepared());
    let fault_observed = wait_for_faults(&fixture.probe, 1).await;
    let first_stopped =
        finish_recovery_actor(&first_shutdown, &mut first, &RecoveryProgress::Watchdog).await;
    assert!(fault_observed && first_stopped);
    assert!(record_paths(&fixture.filesystem.state, "receipt.json").is_empty());
    let requests = fixture.probe.requests.lock().unwrap().clone();
    assert!(!requests.is_empty());
    assert!(requests.iter().all(|request| request == &requests[0]));
    assert_eq!(fixture.store.head().unwrap(), original_head + 1);
    let first_attempts = requests.len();

    fixture.probe.lose_ack.store(false, Ordering::SeqCst);
    let (shutdown, second) = actor(fixture.prepared());
    wait_for_receipt(&fixture.filesystem.state).await;
    shutdown.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(2), second)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let requests = fixture.probe.requests.lock().unwrap();
    assert_eq!(requests.len(), first_attempts + 1);
    assert!(requests.iter().all(|request| request == &requests[0]));
    assert_eq!(fixture.store.head().unwrap(), original_head + 1);
    assert_eq!(
        fixture
            .store
            .broker(fixture.filesystem.value["broker_id"].as_str().unwrap())
            .unwrap()
            .unwrap()
            .broker
            .last_heartbeat
            .unwrap()
            .sequence,
        1
    );
}

#[tokio::test]
async fn response_with_wrong_input_commitment_cannot_ack_or_mint_another_operation() {
    let fixture = LiveFixture::new().await;
    fixture.probe.tamper_ack.store(true, Ordering::SeqCst);
    let (_shutdown, task) = actor(fixture.prepared());
    assert!(
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert_eq!(fixture.probe.requests.lock().unwrap().len(), 1);
    assert_eq!(
        record_paths(&fixture.filesystem.state, "pending.json").len(),
        1
    );
    assert!(record_paths(&fixture.filesystem.state, "receipt.json").is_empty());
}

#[tokio::test]
async fn authenticated_other_tenant_is_refused_before_heartbeat() {
    let mut fixture = LiveFixture::new().await;
    fixture.scope.expected_tenant_id = "other-tenant".into();
    let (_shutdown, task) = actor(fixture.prepared());
    assert!(
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(fixture.probe.requests.lock().unwrap().is_empty());
    assert!(record_paths(&fixture.filesystem.state, "pending.json").is_empty());
}

fn prepare_local_pending(fixture: &LiveFixture) {
    // Use an actual authenticated Store proof to prepare the request, but do
    // not call its mutation path: the first possible POST belongs to restart.
    let proof = match fixture
        .store
        .authenticate_workload_token(OWN_TOKEN)
        .unwrap()
        .unwrap()
    {
        AuthenticatedWorkload::Broker(proof) => proof,
        _ => panic!("the fixture must authenticate its dedicated Broker proof"),
    };
    let identity = fixture.store.broker_authenticated_identity(&proof).unwrap();
    let prior = fixture.store.broker_for_identity(&proof).unwrap();
    let heartbeat = BrokerHeartbeatV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        expected_heartbeat_revision: prior.broker.heartbeat_revision,
        incarnation_id: identity.incarnation_id.clone(),
        sequence: 1,
    };
    let input_hash = Store::broker_heartbeat_input_hash(&identity, &heartbeat).unwrap();
    let prepared = fixture.prepared();
    prepared
        .stream
        .as_ref()
        .unwrap()
        .append(
            &prepared,
            super::storage::Pending {
                version: 1,
                identity,
                prior,
                heartbeat,
                input_hash,
            },
        )
        .unwrap();
}

#[tokio::test]
async fn presence_uses_only_its_three_control_routes_with_the_same_durable_pending() {
    // Break caught: a presence method silently returns to public admission or
    // acquires a runner route while reconciling a retained heartbeat.
    let fixture = LiveFixture::new().await;
    prepare_local_pending(&fixture);
    let pending_path = record_paths(&fixture.filesystem.state, "pending.json")
        .pop()
        .unwrap();
    let retained = fs::read(&pending_path).unwrap();
    let prepared = fixture.prepared();
    let client = super::http::Client::new(&prepared).unwrap();
    let identity = client.identity(&prepared).await.unwrap();
    let view = client.view(&prepared).await.unwrap();
    let pending: super::storage::Pending = serde_json::from_slice(&retained).unwrap();
    assert_eq!(identity, pending.identity);
    assert_eq!(view.broker, pending.prior.broker);
    let before = fixture.store.head().unwrap();
    let first = client.post(&prepared, &pending).await.unwrap();
    let replay = client.post(&prepared, &pending).await.unwrap();
    assert_eq!(first.receipt, replay.receipt);
    assert_eq!(fixture.store.head().unwrap(), before + 1);
    assert_eq!(fs::read(&pending_path).unwrap(), retained);
    assert_eq!(
        *fixture.probe.transport_paths.lock().unwrap(),
        vec![
            "/control/api/v1/brokers/identity",
            "/control/api/v1/brokers/self",
            "/control/api/v1/brokers/heartbeat",
            "/control/api/v1/brokers/heartbeat",
        ]
    );
    let requests = fixture.probe.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| request == &pending.heartbeat));
}

#[tokio::test]
async fn presence_control_refusals_do_not_fall_back_or_change_the_pending() {
    for status in [404, 503, 307] {
        let fixture = LiveFixture::new().await;
        prepare_local_pending(&fixture);
        let pending_path = record_paths(&fixture.filesystem.state, "pending.json")
            .pop()
            .unwrap();
        let retained = fs::read(&pending_path).unwrap();
        let pending: super::storage::Pending = serde_json::from_slice(&retained).unwrap();
        let prepared = fixture.prepared();
        let client = super::http::Client::new(&prepared).unwrap();
        *fixture.probe.transport_refusal.lock().unwrap() = Some(status);
        let before = fixture.store.head().unwrap();
        assert!(client.identity(&prepared).await.is_err());
        assert!(client.view(&prepared).await.is_err());
        assert!(client.post(&prepared, &pending).await.is_err());
        assert_eq!(fixture.store.head().unwrap(), before);
        assert_eq!(fs::read(&pending_path).unwrap(), retained);
        assert_eq!(
            *fixture.probe.transport_paths.lock().unwrap(),
            vec![
                "/control/api/v1/brokers/identity",
                "/control/api/v1/brokers/self",
                "/control/api/v1/brokers/heartbeat",
            ]
        );
    }
}

#[tokio::test]
async fn restart_reserves_receipt_capacity_before_replaying_an_already_counted_pending() {
    // Break caught: a lowered byte budget currently allows the remote commit,
    // then refuses the ACK locally. The operation cap must still permit replay.
    for enough_receipt_space in [true, false] {
        let fixture = LiveFixture::new().await;
        prepare_local_pending(&fixture);
        let pending_path = record_paths(&fixture.filesystem.state, "pending.json")
            .pop()
            .unwrap();
        let pending_bytes = fs::read(&pending_path).unwrap();
        let anchor_bytes = fs::read(fixture.filesystem.state.join("anchor.json")).unwrap();
        let mut config: Value =
            serde_json::from_slice(&fs::read(&fixture.filesystem.config).unwrap()).unwrap();
        config["max_operations"] = serde_json::json!(1);
        config["max_state_bytes"] = serde_json::json!(
            pending_bytes.len()
                + anchor_bytes.len()
                + if enough_receipt_space { 16_384 } else { 0 }
        );
        fs::write(
            &fixture.filesystem.config,
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let original_head = fixture.store.head().unwrap();
        let preparation =
            PreparedPresence::load(&fixture.filesystem.config, fixture.scope.clone(), vec![])
                .and_then(|mut prepared| {
                    prepared.finalize(&[])?;
                    Ok(prepared)
                });
        if enough_receipt_space {
            let (shutdown, task) = actor(preparation.unwrap());
            wait_for_receipt(&fixture.filesystem.state).await;
            shutdown.send(true).unwrap();
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(fixture.probe.requests.lock().unwrap().len(), 1);
            assert_eq!(fixture.store.head().unwrap(), original_head + 1);
        } else {
            match preparation {
                Err(error) => assert_eq!(error, PresenceError::Capacity),
                Ok(prepared) => {
                    let (_shutdown, task) = actor(prepared);
                    let error = tokio::time::timeout(Duration::from_secs(5), task)
                        .await
                        .unwrap()
                        .unwrap()
                        .unwrap_err();
                    assert_eq!(error, PresenceError::Capacity);
                }
            }
            assert_eq!(
                fixture.probe.requests.lock().unwrap().len(),
                0,
                "insufficient receipt space must be refused before any POST"
            );
            assert_eq!(
                fixture.store.head().unwrap(),
                original_head,
                "an unrecordable receipt must not leave a new remote audit effect"
            );
            assert!(record_paths(&fixture.filesystem.state, "receipt.json").is_empty());
        }
        assert_eq!(fs::read(&pending_path).unwrap(), pending_bytes);
        assert_eq!(
            fs::read(fixture.filesystem.state.join("anchor.json")).unwrap(),
            anchor_bytes
        );
    }
}

#[tokio::test]
async fn live_history_substitution_stops_before_the_next_heartbeat_post() {
    // Break caught: validating only root entries and cached Head.bytes permits
    // a new mutation after historical evidence was removed or contaminated.
    for remove_receipt in [true, false] {
        let fixture = LiveFixture::new().await;
        let mut config: Value =
            serde_json::from_slice(&fs::read(&fixture.filesystem.config).unwrap()).unwrap();
        config["interval_seconds"] = serde_json::json!(5);
        fs::write(
            &fixture.filesystem.config,
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let original_head = fixture.store.head().unwrap();
        let (shutdown, task) = actor(fixture.prepared());
        wait_for_receipt(&fixture.filesystem.state).await;
        let receipt = record_paths(&fixture.filesystem.state, "receipt.json")
            .pop()
            .unwrap();
        let unexpected = receipt.parent().unwrap().join("unexpected-record.json");
        if remove_receipt {
            fs::remove_file(&receipt).unwrap();
        } else {
            use std::os::unix::fs::PermissionsExt;
            fs::write(&unexpected, b"retained unexpected evidence").unwrap();
            fs::set_permissions(&unexpected, fs::Permissions::from_mode(0o600)).unwrap();
        }
        // These are harness deadlines around the configured five-second tick,
        // not a latency guarantee or a worker supervision measurement.
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if task.is_finished() || fixture.probe.requests.lock().unwrap().len() > 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let _ = shutdown.send(true);
        let _outcome = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            fixture.probe.requests.lock().unwrap().len(),
            1,
            "the next tick must refuse altered historical evidence before POST"
        );
        assert_eq!(fixture.store.head().unwrap(), original_head + 1);
        if remove_receipt {
            assert!(!receipt.exists());
        } else {
            assert_eq!(
                fs::read(&unexpected).unwrap(),
                b"retained unexpected evidence"
            );
        }
    }
}

const ROTATED_TOKEN: &str = "presence-fixture-rotated-bearer-abcdef0123456789";

fn issue_binding(
    fixture: &LiveFixture,
    credential: &str,
    incarnation: &str,
    token: &str,
    expected_revision: u64,
) {
    fixture
        .store
        .issue_workload_binding(&WorkloadBindingIssueV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            credential_id: credential.into(),
            expected_revision,
            subject: WorkloadSubjectV1::Broker {
                broker_id: fixture.filesystem.value["broker_id"]
                    .as_str()
                    .unwrap()
                    .into(),
                incarnation_id: incarnation.into(),
            },
            sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
            expires_at: "2100-02-01T00:00:00.000Z".into(),
        })
        .unwrap();
}

fn replace_grant(fixture: &LiveFixture, credential: &str, incarnation: &str) {
    fixture
        .store
        .mutate_broker_grant(&BrokerGrantMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            broker_id: fixture.filesystem.value["broker_id"]
                .as_str()
                .unwrap()
                .into(),
            expected_revision: Some(1),
            credential_id: credential.into(),
            incarnation_id: incarnation.into(),
            authorities: vec![BrokerAuthorityV1 {
                authority_id: "fixture-authority".into(),
                runtime_kind: BrokerRuntimeKindV1::Docker,
            }],
            lease_seconds: 120,
            not_after: "2100-02-01T00:00:00.000Z".into(),
        })
        .unwrap();
}

async fn stop_actor(shutdown: watch::Sender<bool>, task: JoinHandle<Result<(), PresenceError>>) {
    shutdown.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn new_incarnation_preserves_the_global_revision_and_starts_its_own_sequence_at_one() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = LiveFixture::new().await;
    let (shutdown, task) = actor(fixture.prepared());
    wait_for_receipt(&fixture.filesystem.state).await;
    stop_actor(shutdown, task).await;
    let historical_receipt = record_paths(&fixture.filesystem.state, "receipt.json")
        .pop()
        .unwrap();
    let historical_bytes = fs::read(&historical_receipt).unwrap();
    let broker = fixture.filesystem.value["broker_id"].as_str().unwrap();
    let previous = fixture.store.broker(broker).unwrap().unwrap().broker;
    let credential = Uuid::new_v4().to_string();
    let incarnation = Uuid::new_v4().to_string();
    issue_binding(&fixture, &credential, &incarnation, ROTATED_TOKEN, 0);
    replace_grant(&fixture, &credential, &incarnation);
    let regranted = fixture.store.broker(broker).unwrap().unwrap().broker;
    assert_eq!(regranted.heartbeat_revision, previous.heartbeat_revision);
    assert_eq!(regranted.last_heartbeat, previous.last_heartbeat);

    let state = fixture.filesystem.root.join("new-incarnation-state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = fixture.filesystem.value.clone();
    config["credential_id"] = serde_json::json!(credential);
    config["incarnation_id"] = serde_json::json!(incarnation);
    config["state_dir"] = serde_json::json!(state);
    *fixture.probe.state.lock().unwrap() = state.clone();
    fs::write(
        fixture.filesystem.value["broker_token_file"]
            .as_str()
            .unwrap(),
        ROTATED_TOKEN,
    )
    .unwrap();
    fs::write(
        &fixture.filesystem.config,
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let (shutdown, task) = actor(fixture.prepared());
    wait_for_receipt(&state).await;
    stop_actor(shutdown, task).await;
    let requests = fixture.probe.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].expected_heartbeat_revision,
        previous.heartbeat_revision
    );
    assert_eq!(requests[1].sequence, 1);
    assert_eq!(requests[1].incarnation_id, incarnation);
    let current = fixture.store.broker(broker).unwrap().unwrap().broker;
    assert_eq!(current.heartbeat_revision, previous.heartbeat_revision + 1);
    assert_eq!(current.last_heartbeat.unwrap().sequence, 1);
    assert_eq!(fs::read(&historical_receipt).unwrap(), historical_bytes);
}

#[tokio::test]
async fn explicit_restart_accepts_rotation_only_when_the_stream_is_fully_acknowledged() {
    for unresolved in [false, true] {
        let fixture = LiveFixture::new().await;
        if unresolved {
            prepare_local_pending(&fixture);
        } else {
            let (shutdown, task) = actor(fixture.prepared());
            wait_for_receipt(&fixture.filesystem.state).await;
            stop_actor(shutdown, task).await;
        }
        let pending_paths = record_paths(&fixture.filesystem.state, "pending.json");
        let retained = fs::read(&pending_paths[0]).unwrap();
        issue_binding(
            &fixture,
            fixture.filesystem.value["credential_id"].as_str().unwrap(),
            fixture.filesystem.value["incarnation_id"].as_str().unwrap(),
            ROTATED_TOKEN,
            1,
        );
        fs::write(
            fixture.filesystem.value["broker_token_file"]
                .as_str()
                .unwrap(),
            ROTATED_TOKEN,
        )
        .unwrap();
        let before = fixture.store.head().unwrap();
        let (shutdown, task) = actor(fixture.prepared());
        if unresolved {
            let error = tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err();
            assert_eq!(error, PresenceError::Conflict);
            assert!(fixture.probe.requests.lock().unwrap().is_empty());
            assert_eq!(fixture.store.head().unwrap(), before);
        } else {
            wait_for_receipt_count(&fixture.filesystem.state, 2).await;
            stop_actor(shutdown, task).await;
            assert_eq!(fixture.probe.requests.lock().unwrap().len(), 2);
            assert_eq!(fixture.store.head().unwrap(), before + 1);
            let second = record_paths(&fixture.filesystem.state, "pending.json")
                .into_iter()
                .map(|path| serde_json::from_slice::<Value>(&fs::read(path).unwrap()).unwrap())
                .find(|value| value["heartbeat"]["sequence"] == 2)
                .unwrap();
            assert_eq!(second["identity"]["binding_revision"], 2);
        }
        assert_eq!(fs::read(&pending_paths[0]).unwrap(), retained);
    }
}

#[tokio::test]
async fn unresolved_pending_does_not_cross_a_changed_or_revoked_grant() {
    for revoke in [true, false] {
        let fixture = LiveFixture::new().await;
        prepare_local_pending(&fixture);
        let pending = record_paths(&fixture.filesystem.state, "pending.json")
            .pop()
            .unwrap();
        let retained = fs::read(&pending).unwrap();
        if revoke {
            fixture
                .store
                .revoke_broker(&BrokerRevocationV1 {
                    version: 1,
                    operation_id: Uuid::new_v4().to_string(),
                    broker_id: fixture.filesystem.value["broker_id"]
                        .as_str()
                        .unwrap()
                        .into(),
                    expected_revision: 1,
                })
                .unwrap();
        } else {
            let credential = Uuid::new_v4().to_string();
            let incarnation = Uuid::new_v4().to_string();
            issue_binding(&fixture, &credential, &incarnation, ROTATED_TOKEN, 0);
            replace_grant(&fixture, &credential, &incarnation);
        }
        let before = fixture.store.head().unwrap();
        let (_shutdown, task) = actor(fixture.prepared());
        let error = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(
            matches!(
                error,
                PresenceError::Authentication | PresenceError::Forbidden | PresenceError::Conflict
            ),
            "opaque refusal category {error:?}; revoke={revoke}"
        );
        assert!(fixture.probe.requests.lock().unwrap().is_empty());
        assert_eq!(fixture.store.head().unwrap(), before);
        assert_eq!(fs::read(&pending).unwrap(), retained);
        assert!(record_paths(&fixture.filesystem.state, "receipt.json").is_empty());
    }
}

#[tokio::test]
async fn cumulative_audit_budget_refusal_stops_without_retry_or_new_operation() {
    let fixture = LiveFixture::with_workload_policy(Some(WorkloadAuditPolicyV1 {
        version: 1,
        tenant: AuditCapacityV1 {
            events: 4,
            bytes: 64 * 1024,
        },
        principal: AuditCapacityV1 {
            events: 1,
            bytes: 16 * 1024,
        },
    }))
    .await;
    let mut config = fixture.filesystem.value.clone();
    config["interval_seconds"] = serde_json::json!(5);
    fs::write(
        &fixture.filesystem.config,
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let before = fixture.store.head().unwrap();
    let (_shutdown, task) = actor(fixture.prepared());
    let error = tokio::time::timeout(Duration::from_secs(8), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error, PresenceError::Capacity);
    assert_eq!(*fixture.probe.statuses.lock().unwrap(), vec![200, 429]);
    let requests = fixture.probe.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].sequence, 1);
    assert_eq!(requests[1].sequence, 2);
    assert_eq!(fixture.store.head().unwrap(), before + 1);
    assert_eq!(
        record_paths(&fixture.filesystem.state, "pending.json").len(),
        2
    );
    assert_eq!(
        record_paths(&fixture.filesystem.state, "receipt.json").len(),
        1
    );
}
