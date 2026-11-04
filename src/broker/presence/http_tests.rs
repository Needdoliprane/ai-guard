//! Real gateway/SQLite regressions. The sole fault injection is the HTTP ACK.
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
    time::Duration,
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
    if request.uri().path() != "/api/v1/brokers/heartbeat" {
        return next.run(request).await;
    }
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 16_384).await.unwrap();
    let heartbeat: BrokerHeartbeatV1 = serde_json::from_slice(&bytes).unwrap();
    // Count the attempted POST before assertions: a broken preparation cannot
    // disappear from the test's observations when this middleware refuses it.
    probe.requests.lock().unwrap().push(heartbeat.clone());
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
    if probe.lose_ack.load(Ordering::SeqCst) {
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

#[tokio::test]
async fn committed_lost_ack_restart_replays_same_durable_operation_without_duplicate_audit() {
    let fixture = LiveFixture::new().await;
    let original_head = fixture.store.head().unwrap();
    fixture.probe.lose_ack.store(true, Ordering::SeqCst);
    let (_first_shutdown, first) = actor(fixture.prepared());
    assert!(
        tokio::time::timeout(Duration::from_secs(10), first)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(record_paths(&fixture.filesystem.state, "receipt.json").is_empty());
    let requests = fixture.probe.requests.lock().unwrap().clone();
    assert_eq!(
        requests.len(),
        4,
        "initial request plus at most three retries"
    );
    assert!(requests.iter().all(|request| request == &requests[0]));
    assert_eq!(fixture.store.head().unwrap(), original_head + 1);

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
    assert_eq!(requests.len(), 5);
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
