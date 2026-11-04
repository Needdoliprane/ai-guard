//! Bounded, real-filesystem observations of one successful publisher cycle.
//! The output describes this fixture and host only; it is not an SLO.

use super::{LiveFixture, OWN_TOKEN, record_paths};
use crate::{
    audit::Store,
    broker::presence::{PreparedPresence, storage::Pending},
    broker_registry::BrokerHeartbeatV1,
    control::QuotaPolicyV1,
    control_fs::cost_probe::{EXCLUSIVE, Measurement, Operation},
    workload::AuthenticatedWorkload,
};
use std::{
    fs,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{Semaphore, watch},
    task::JoinHandle,
};
use uuid::Uuid;

const HISTORY_COUNTS: [usize; 3] = [0, 8, 32];

fn authenticated_proof(fixture: &LiveFixture) -> crate::broker_registry::BrokerIdentityV1 {
    match fixture
        .store
        .authenticate_workload_token(OWN_TOKEN)
        .unwrap()
        .unwrap()
    {
        AuthenticatedWorkload::Broker(proof) => proof,
        _ => panic!("the fixture token must resolve to its broker proof"),
    }
}

/// Prepare real durable history outside the measurement window. Every entry
/// comes from the real Store mutation path and its returned receipt.
fn seed_acknowledged_history(fixture: &LiveFixture, count: usize) {
    let proof = authenticated_proof(fixture);
    let prepared = fixture.prepared();
    let stream = prepared.stream.as_ref().expect("finalized fixture stream");

    for _ in 0..count {
        let identity = fixture.store.broker_authenticated_identity(&proof).unwrap();
        let prior = fixture.store.broker_for_identity(&proof).unwrap();
        let previous = prior
            .broker
            .last_heartbeat
            .as_ref()
            .filter(|heartbeat| heartbeat.incarnation_id == identity.incarnation_id)
            .map_or(0, |heartbeat| heartbeat.sequence);
        let heartbeat = BrokerHeartbeatV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            expected_heartbeat_revision: prior.broker.heartbeat_revision,
            incarnation_id: identity.incarnation_id.clone(),
            sequence: previous.checked_add(1).expect("bounded fixture sequence"),
        };
        let input_hash = Store::broker_heartbeat_input_hash(&identity, &heartbeat).unwrap();
        stream
            .append(
                &prepared,
                Pending {
                    version: 1,
                    identity,
                    prior,
                    heartbeat: heartbeat.clone(),
                    input_hash,
                },
            )
            .unwrap();
        let result = fixture.store.broker_heartbeat(&proof, &heartbeat).unwrap();
        stream.acknowledge(&prepared, result).unwrap();
    }
    drop(prepared);
}

fn start_actor_with_single_io_permit(
    prepared: PreparedPresence,
) -> (
    watch::Sender<bool>,
    Arc<Semaphore>,
    JoinHandle<crate::broker::presence::Result<()>>,
) {
    let (shutdown, receiver) = watch::channel(false);
    let io = Arc::new(Semaphore::new(1));
    let task = tokio::spawn(prepared.run(receiver, Arc::new(Semaphore::new(4)), Arc::clone(&io)));
    (shutdown, io, task)
}

async fn wait_for_receipt_path(state: &std::path::Path, operation: usize) -> bool {
    let receipt = state
        .join(format!("op-{operation:020}"))
        .join("receipt.json");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if receipt.is_file() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok_and(|visible| visible)
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
struct ProcessObservation {
    fd_entries: usize,
    rss_kib: u64,
}

#[cfg(target_os = "linux")]
fn process_observation() -> Option<ProcessObservation> {
    let fd_entries = fs::read_dir("/proc/self/fd").ok()?.count();
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let rss_kib = status.lines().find_map(|line| {
        line.strip_prefix("VmRSS:")?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()
    })?;
    Some(ProcessObservation {
        fd_entries,
        rss_kib,
    })
}

#[cfg(not(target_os = "linux"))]
#[derive(Clone, Copy)]
struct ProcessObservation;

#[cfg(not(target_os = "linux"))]
fn process_observation() -> Option<ProcessObservation> {
    None
}

fn observation_fields(observation: Option<ProcessObservation>) -> (String, String) {
    #[cfg(target_os = "linux")]
    if let Some(observation) = observation {
        return (
            observation.fd_entries.to_string(),
            observation.rss_kib.to_string(),
        );
    }
    let _ = observation;
    ("unavailable".into(), "unavailable".into())
}

#[tokio::test]
#[ignore = "manual bounded cost observation; run explicitly in the inspected sandbox"]
async fn publisher_cycles_one_real_heartbeat_at_history_depths_0_8_32() {
    for history_count in HISTORY_COUNTS {
        let preparation_started = Instant::now();
        let fixture = LiveFixture::new().await;
        seed_acknowledged_history(&fixture, history_count);

        let expected_previous = fixture
            .store
            .broker("10000000-0000-4000-8000-000000000001")
            .unwrap()
            .unwrap()
            .broker
            .last_heartbeat
            .as_ref()
            .map_or(0, |heartbeat| heartbeat.sequence);
        assert_eq!(expected_previous, history_count as u64);
        assert_eq!(
            record_paths(&fixture.filesystem.state, "pending.json").len(),
            history_count
        );
        assert_eq!(
            record_paths(&fixture.filesystem.state, "receipt.json").len(),
            history_count
        );

        // Opening/revalidating prior history and all Store/stream seeding are
        // deliberately outside the cost window.
        let prepared = fixture.prepared();
        let history_head = prepared
            .stream
            .as_ref()
            .expect("finalized fixture stream")
            .cached_head()
            .unwrap();
        assert_eq!(history_head.count as usize, history_count);
        assert!(history_head.pending.is_none());
        drop(prepared);
        let prepared = fixture.prepared();
        let store_head_before = fixture.store.head().unwrap();
        let state = fixture.filesystem.state.clone();
        let depth = state.ancestors().count();
        let preparation_elapsed_ns =
            u64::try_from(preparation_started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let resource_before = process_observation();

        // This gate serializes the shared global counter with its other users.
        // Keep it through cleanup and Measurement::finish.
        let _exclusive = EXCLUSIVE.lock().await;
        let measurement = Measurement::start(&state).expect("private fixture measurement scope");
        let operation_started = Instant::now();
        let (shutdown, io, mut actor) = start_actor_with_single_io_permit(prepared);

        // The existing middleware inspects every prior pending record before
        // forwarding POST. Those paths are inside state_dir, but its standard
        // fs::read/read_dir calls are outside the wrapped syscall counters;
        // state_dir is tmpfs, so elapsed time is not physical disk latency.
        let receipt_visible = wait_for_receipt_path(&state, history_count + 1).await;
        let permit = match tokio::time::timeout(
            Duration::from_secs(10),
            Arc::clone(&io).acquire_owned(),
        )
        .await
        {
            Ok(Ok(permit)) => permit,
            _ => {
                let _ = shutdown.send(true);
                actor.abort();
                let actor_joined = tokio::time::timeout(Duration::from_secs(5), &mut actor)
                    .await
                    .is_ok();
                eprintln!(
                    "publisher_cost incomplete=true phase=io_barrier actor_joined={actor_joined} process_exit=86"
                );
                // A timeout means the blocking closure's completion is unknown.
                // Exit before dropping the fixture or measurement gate; Root
                // checks the owner process before normal sandbox cleanup.
                std::process::exit(86);
            }
        };
        let operation_elapsed_ns =
            u64::try_from(operation_started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let resource_at_ack_barrier = process_observation();
        let shutdown_sent = shutdown.send(true).is_ok();
        drop(permit);

        // The I/O permit barrier above proves no blocking filesystem closure is
        // still active. Shutdown is sent while holding it, before it is freed.
        let joined = tokio::time::timeout(Duration::from_secs(10), &mut actor).await;
        let actor_result = match joined {
            Ok(result) => Some(result),
            Err(_) => {
                actor.abort();
                match tokio::time::timeout(Duration::from_secs(5), &mut actor).await {
                    Ok(result) => Some(result),
                    Err(_) => {
                        eprintln!(
                            "publisher_cost incomplete=true phase=actor_join_after_barrier actor_joined=false process_exit=86"
                        );
                        // The I/O barrier was held across shutdown, but the
                        // actor's terminal state is still unknown.
                        std::process::exit(86);
                    }
                }
            }
        };
        let resource_after = process_observation();
        let snapshot = measurement
            .finish()
            .expect("measurement scope remains owned");
        drop(_exclusive);
        let actor_clean = matches!(actor_result, Some(Ok(Ok(()))));

        if !receipt_visible || !actor_clean {
            eprintln!(
                "publisher_cost incomplete=true phase=receipt_or_actor n={history_count} receipt_visible={receipt_visible} actor_clean={actor_clean} file_sync_all={} directory_sync_all={} outstanding={}",
                snapshot.metric(Operation::FileSyncAll).calls,
                snapshot.metric(Operation::DirectorySyncAll).calls,
                snapshot.outstanding,
            );
            assert!(
                receipt_visible,
                "the measured cycle did not publish its receipt"
            );
            assert!(actor_clean, "actor did not stop cleanly after the barrier");
        }

        // No assertions occur between actor start and full cleanup/finish.
        // Reopen only after the measured window to verify the committed state.
        let prepared_after = fixture.prepared();
        let reopened = prepared_after
            .stream
            .as_ref()
            .expect("reopened fixture stream")
            .cached_head()
            .unwrap();
        let operation_dir = state.join(format!("op-{:020}", history_count + 1));
        let pending: Pending =
            serde_json::from_slice(&fs::read(operation_dir.join("pending.json")).unwrap()).unwrap();
        let receipt_bytes = fs::read(operation_dir.join("receipt.json")).unwrap();
        let receipt_value: serde_json::Value = serde_json::from_slice(&receipt_bytes).unwrap();
        let request = fixture.probe.requests.lock().unwrap().clone();
        let statuses = fixture.probe.statuses.lock().unwrap().clone();
        let store_head_after = fixture.store.head().unwrap();
        let proof = authenticated_proof(&fixture);
        let replay = request
            .first()
            .map(|heartbeat| fixture.store.broker_heartbeat(&proof, heartbeat).unwrap());
        let store_head_after_replay = fixture.store.head().unwrap();
        let replayed_receipt_matches = replay.as_ref().is_some_and(|result| {
            let mut committed = result.clone();
            committed.replayed = false;
            serde_json::to_value(committed).is_ok_and(|value| value == receipt_value)
        });
        let file_sync = snapshot.metric(Operation::FileSyncAll);
        let directory_sync = snapshot.metric(Operation::DirectorySyncAll);
        let read_to_end = snapshot.metric(Operation::ReadToEnd);
        let read_at = snapshot.metric(Operation::ReadAt);
        // The preserved baseline was four scans: at D=5, file syncs 6/70/262,
        // directory syncs 63/255/831, and ReadToEnd calls 4/68/260 for N=0/8/32.
        // After removing only the getter's redundant non-authorizing scan, the
        // retained three scans predict 6N+6 file syncs, (3N+10)D+3N+8 directory
        // syncs, and 6N+4 ReadToEnd calls. At D=5 these are 6/54/198, 58/202/634,
        // and 4/52/196 respectively. Counts are source/fixture observations;
        // elapsed timings on the tmpfs sandbox are not physical disk latency.
        let expected_file_syncs = 6 * history_count as u64 + 6;
        let expected_directory_syncs =
            (3 * history_count as u64 + 10) * depth as u64 + 3 * history_count as u64 + 8;
        let expected_reads = 6 * history_count as u64 + 4;
        let no_fs_errors = [
            Operation::Read,
            Operation::ReadToEnd,
            Operation::Revalidate,
            Operation::ReadAt,
            Operation::DirectoryRevalidate,
            Operation::DirectorySync,
            Operation::DirectorySyncAll,
            Operation::FileSync,
            Operation::FileSyncAll,
            Operation::Create,
            Operation::Publish,
        ]
        .into_iter()
        .map(|operation| snapshot.metric(operation))
        .all(|metric| metric.failed == 0 && metric.abandoned == 0);
        let worker_observation = fixture
            .store
            .worker_usage()
            .is_ok_and(|usage| usage.is_empty());
        let quota_observation = fixture
            .store
            .worker_quota(&QuotaPolicyV1::default())
            .is_ok_and(|usage| usage.held_workers == 0);
        let (fd_before, rss_before) = observation_fields(resource_before);
        let (fd_at_ack_barrier, rss_at_ack_barrier) = observation_fields(resource_at_ack_barrier);
        let (fd_after, rss_after) = observation_fields(resource_after);

        println!(
            "publisher_cost sample=real_store_http_cycle n={history_count} d={depth} file_sync_all={} expected_file_sync_all={expected_file_syncs} directory_sync_all={} expected_directory_sync_all={expected_directory_syncs} read_to_end={} expected_read_to_end={expected_reads} read_at_calls={} read_at_bytes={} file_sync_all_ns={} directory_sync_all_ns={} elapsed_fixture_ns={operation_elapsed_ns} prep_wall_ns={preparation_elapsed_ns} outstanding={} no_fs_errors={no_fs_errors} no_worker_usage={worker_observation} no_worker_quota={quota_observation} fd_entries_process_before={fd_before} fd_entries_process_at_ack_barrier={fd_at_ack_barrier} fd_entries_process_after={fd_after} rss_kib_process_before={rss_before} rss_kib_process_at_ack_barrier={rss_at_ack_barrier} rss_kib_process_after={rss_after}",
            file_sync.calls,
            directory_sync.calls,
            read_to_end.calls,
            read_at.calls,
            read_at.bytes,
            file_sync.elapsed_ns,
            directory_sync.elapsed_ns,
            snapshot.outstanding,
        );

        assert!(
            receipt_visible,
            "the measured cycle did not publish its receipt"
        );
        assert!(
            shutdown_sent,
            "actor shutdown receiver disappeared before cleanup"
        );
        assert!(actor_clean, "actor did not stop cleanly");
        assert_eq!(snapshot.outstanding, 0);
        assert!(
            no_fs_errors,
            "filesystem probe saw a failed or abandoned span"
        );
        assert_eq!(file_sync.calls, expected_file_syncs);
        assert_eq!(directory_sync.calls, expected_directory_syncs);
        assert_eq!(read_to_end.calls, expected_reads);
        assert_eq!(request.len(), 1, "the sample must contain exactly one POST");
        assert_eq!(statuses, [200]);
        assert_eq!(request[0].sequence, history_count as u64 + 1);
        assert!(
            pending.heartbeat == request[0],
            "persisted pending differs from posted operation"
        );
        assert!(
            replay
                .as_ref()
                .is_some_and(|result| pending.input_hash == result.receipt.input_hash),
            "pending commitment differs from Store replay"
        );
        assert_eq!(
            record_paths(&state, "pending.json").len(),
            history_count + 1
        );
        assert_eq!(
            record_paths(&state, "receipt.json").len(),
            history_count + 1
        );
        assert_eq!(reopened.count as usize, history_count + 1);
        assert!(reopened.pending.is_none());
        assert!(
            reopened
                .last
                .as_ref()
                .is_some_and(|last| last.pending.heartbeat == request[0]),
            "reopened stream operation differs from posted operation"
        );
        assert!(
            replayed_receipt_matches,
            "persisted receipt differs from Store replay"
        );
        assert!(replay.as_ref().is_some_and(|result| result.replayed));
        assert_eq!(store_head_after, store_head_before + 1);
        assert_eq!(store_head_after_replay, store_head_after);
        assert_eq!(
            fixture
                .store
                .broker("10000000-0000-4000-8000-000000000001")
                .unwrap()
                .unwrap()
                .broker
                .last_heartbeat
                .unwrap()
                .sequence,
            history_count as u64 + 1
        );
        assert!(worker_observation);
        assert!(quota_observation);
    }
}

#[tokio::test]
async fn validated_head_snapshot_does_not_resynchronize_history() {
    let history_count = 8usize;
    let fixture = LiveFixture::new().await;
    seed_acknowledged_history(&fixture, history_count);
    let prepared = fixture.prepared();
    let stream = prepared.stream.as_ref().expect("finalized fixture stream");
    let store_head_before = fixture.store.head().unwrap();
    let state = fixture.filesystem.state.clone();

    assert_eq!(record_paths(&state, "pending.json").len(), history_count);
    assert_eq!(record_paths(&state, "receipt.json").len(), history_count);

    let _exclusive = EXCLUSIVE.lock().await;
    let measurement = Measurement::start(&state).expect("private fixture measurement scope");
    let observed_head = stream.cached_head();
    let snapshot = measurement
        .finish()
        .expect("measurement scope remains owned");
    drop(_exclusive);

    let store_head_after = fixture.store.head().unwrap();
    let no_http = fixture.probe.requests.lock().unwrap().is_empty()
        && fixture.probe.statuses.lock().unwrap().is_empty();
    let file_sync = snapshot.metric(Operation::FileSyncAll);
    let directory_sync = snapshot.metric(Operation::DirectorySyncAll);
    let read_to_end = snapshot.metric(Operation::ReadToEnd);
    let no_fs_errors = [
        Operation::Read,
        Operation::ReadToEnd,
        Operation::Revalidate,
        Operation::ReadAt,
        Operation::DirectoryRevalidate,
        Operation::DirectorySync,
        Operation::DirectorySyncAll,
        Operation::FileSync,
        Operation::FileSyncAll,
        Operation::Create,
        Operation::Publish,
    ]
    .into_iter()
    .map(|operation| snapshot.metric(operation))
    .all(|metric| metric.failed == 0 && metric.abandoned == 0);

    assert!(observed_head.is_ok(), "validated head snapshot was refused");
    let head = observed_head.unwrap();
    assert_eq!(file_sync.calls, 0);
    assert_eq!(directory_sync.calls, 0);
    assert_eq!(read_to_end.calls, 0);
    assert_eq!(snapshot.outstanding, 0);
    assert!(
        no_fs_errors,
        "filesystem probe saw a failed or abandoned span"
    );
    assert_eq!(head.count as usize, history_count);
    assert!(head.pending.is_none());
    assert_eq!(
        head.last
            .as_ref()
            .map(|last| last.pending.heartbeat.sequence),
        Some(history_count as u64)
    );
    assert_eq!(store_head_after, store_head_before);
    assert!(no_http, "head snapshot must not contact the gateway");
}
