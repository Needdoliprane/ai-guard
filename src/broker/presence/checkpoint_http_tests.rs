//! Same real gateway/SQLite fixtures as presence, with the two new scoped
//! routes supplied by Root's composition. No remote host or engine is implied.
use super::*;
use crate::broker_registry::{BrokerCheckpointSnapshotV1, BrokerJobCheckpointV1};
use std::os::unix::fs::PermissionsExt;

pub(super) fn enable(fixture: &mut LiveFixture) -> PathBuf {
    let db = fixture.filesystem.root.join("tenant.sqlite");
    fixture
        .store
        .migrate_human_brokers_v7(&db.with_extension("before7"))
        .unwrap();
    fixture
        .store
        .migrate_kubernetes_create_v8(&db.with_extension("before8"))
        .unwrap();
    fixture
        .store
        .migrate_kubernetes_policy_v9(&db.with_extension("before9"))
        .unwrap();
    fixture
        .store
        .migrate_human_workers_v10(&db.with_extension("before10"))
        .unwrap();
    fixture
        .store
        .migrate_kubernetes_session_reservations_v11(&db.with_extension("before11"))
        .unwrap();
    fixture
        .store
        .migrate_broker_checkpoints_v12(&db.with_extension("before12"))
        .unwrap();
    let state = fixture.filesystem.root.join("checkpoint-state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = fixture.filesystem.value.clone();
    config["version"] = serde_json::json!(2);
    config["checkpoint_state_dir"] = serde_json::json!(&state);
    config["checkpoint_max_operations"] = serde_json::json!(32);
    config["checkpoint_max_state_bytes"] = serde_json::json!(1048576);
    fs::write(
        &fixture.filesystem.config,
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    *fixture.probe.checkpoint_state.lock().unwrap() = Some(state.clone());
    state
}
pub(super) fn snapshot(fixture: &LiveFixture, revision: u64) -> BrokerCheckpointSnapshotV1 {
    BrokerCheckpointSnapshotV1 {
        version: 1,
        authority: BrokerAuthorityV1 {
            authority_id: fixture.filesystem.value["authority_id"]
                .as_str()
                .unwrap()
                .into(),
            runtime_kind: BrokerRuntimeKindV1::Docker,
        },
        jobs: vec![BrokerJobCheckpointV1 {
            job_id: "60000000-0000-4000-8000-000000000001".into(),
            revision,
            checkpoint_hash: "a".repeat(64),
        }],
        has_more: true,
    }
}
fn start(
    fixture: &LiveFixture,
    snapshots: watch::Receiver<Option<BrokerCheckpointSnapshotV1>>,
) -> (watch::Sender<bool>, JoinHandle<Result<(), PresenceError>>) {
    let (shutdown, receiver) = watch::channel(false);
    let task = tokio::spawn(fixture.prepared().run_with_checkpoints(
        snapshots,
        receiver,
        Arc::new(Semaphore::new(4)),
        Arc::new(Semaphore::new(2)),
    ));
    (shutdown, task)
}
async fn wait_checkpoint_count(state: &std::path::Path, count: usize) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while record_paths(state, "receipt.json").len() < count {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("condition-bound checkpoint ACK watchdog");
}
async fn finish(shutdown: watch::Sender<bool>, task: JoinHandle<Result<(), PresenceError>>) {
    shutdown.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(15), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_checkpoint_ack_replays_exact_pending_before_publishing_the_newest_source_head() {
    let mut fixture = LiveFixture::new().await;
    let state = enable(&mut fixture);
    *fixture.probe.checkpoint_lost_acks.lock().unwrap() = 1;
    let (source, receiver) = watch::channel(Some(snapshot(&fixture, 17)));
    let (shutdown, task) = start(&fixture, receiver);
    assert!(wait_for_faults(&fixture.probe, 1).await);
    source.send(Some(snapshot(&fixture, 18))).unwrap();
    wait_checkpoint_count(&state, 2).await;
    finish(shutdown, task).await;
    let requests = fixture.probe.checkpoint_requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[0], requests[1],
        "lost response must replay identical operation/sequence/content"
    );
    assert_ne!(requests[1].operation_id, requests[2].operation_id);
    assert_eq!(requests[2].previous_sequence, 1);
    assert_eq!(requests[2].sequence, 2);
    assert_eq!(requests[2].snapshot.jobs[0].revision, 18);
    let events = fixture
        .store
        .events(&crate::audit::EventQuery {
            kind: Some("control.broker_registry.checkpoint".into()),
            limit: Some(16),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(events.len(), 2, "retry must not duplicate mandatory audit");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_source_does_not_publish_an_empty_inventory_or_stop_real_presence() {
    let mut fixture = LiveFixture::new().await;
    enable(&mut fixture);
    let (_source, receiver) = watch::channel(None);
    let (shutdown, task) = start(&fixture, receiver);
    wait_for_receipt(&fixture.filesystem.state).await;
    assert!(fixture.probe.checkpoint_requests.lock().unwrap().is_empty());
    finish(shutdown, task).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn checkpoint_restart_finishes_the_same_pending_without_minting_replacement_identity() {
    let mut fixture = LiveFixture::new().await;
    let state = enable(&mut fixture);
    *fixture.probe.checkpoint_lost_acks.lock().unwrap() = usize::MAX;
    let (source, receiver) = watch::channel(Some(snapshot(&fixture, 17)));
    let (shutdown, task) = start(&fixture, receiver);
    assert!(wait_for_faults(&fixture.probe, 1).await);
    finish(shutdown, task).await;
    assert!(record_paths(&state, "receipt.json").is_empty());
    let first = fixture.probe.checkpoint_requests.lock().unwrap()[0].clone();
    *fixture.probe.checkpoint_lost_acks.lock().unwrap() = 0;
    // None prevents a new source report. Only the already durable operation
    // may complete during this explicit same-scope restart.
    source.send_replace(None);
    let (shutdown, task) = start(&fixture, source.subscribe());
    wait_checkpoint_count(&state, 1).await;
    finish(shutdown, task).await;
    assert!(
        fixture
            .probe
            .checkpoint_requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request == &first)
    );
    let events = fixture
        .store
        .events(&crate::audit::EventQuery {
            kind: Some("control.broker_registry.checkpoint".into()),
            limit: Some(16),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(events.len(), 1);
}
