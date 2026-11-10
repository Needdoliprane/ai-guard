//! Owned barriers after real private FS work, never a simulated HTTP success.
use super::*;
use crate::broker::presence::{preparation_tests, storage};
use std::{
    sync::{
        Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::Poll,
    time::Duration,
};

pub(in crate::broker::presence) struct DiskBarrier {
    entered: tokio::sync::Notify,
    fired: AtomicBool,
    released: Mutex<bool>,
    changed: Condvar,
}
impl DiskBarrier {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: tokio::sync::Notify::new(),
            fired: AtomicBool::new(false),
            released: Mutex::new(false),
            changed: Condvar::new(),
        })
    }
    pub(in crate::broker::presence) fn after_operation<T>(&self) {
        // Only the actual heartbeat head read is held. The checkpoint actor
        // can independently refuse its source while this worker still lives.
        if std::any::type_name::<T>() != std::any::type_name::<storage::Head>()
            || self.fired.swap(true, Ordering::SeqCst)
        {
            return;
        }
        self.entered.notify_one();
        let mut released = self.released.lock().unwrap();
        while !*released {
            released = self.changed.wait(released).unwrap();
        }
    }
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.changed.notify_all();
    }
    async fn wait(&self) {
        tokio::time::timeout(Duration::from_secs(10), self.entered.notified())
            .await
            .expect("actual FS worker must reach owned barrier");
    }
}

/// Test-only controller around the existing barrier after real heartbeat FS.
/// One controller releases the worker on every unwind path; no fake IO result.
pub(in crate::broker) struct ServiceDiskBarrier(Arc<DiskBarrier>);
impl ServiceDiskBarrier {
    pub(in crate::broker) fn new() -> Self {
        Self(DiskBarrier::new())
    }
    pub(in crate::broker) fn attach(&self, prepared: &mut PreparedPresence) {
        prepared.disk_barrier = Some(Arc::clone(&self.0));
    }
    pub(in crate::broker) async fn wait(&self) {
        self.0.wait().await;
    }
    pub(in crate::broker) fn release(&self) {
        self.0.release();
    }
}
impl Drop for ServiceDiskBarrier {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct Release(Arc<DiskBarrier>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_and_cancelled_join_waiter_retain_actual_fs_work_until_rejoined() {
    let mut fixture = LiveFixture::new().await;
    checkpoint_http_tests::enable(&mut fixture);
    let barrier = DiskBarrier::new();
    let _release = Release(Arc::clone(&barrier));
    let mut prepared = fixture.prepared();
    prepared.disk_barrier = Some(Arc::clone(&barrier));
    let (_source, snapshots) = watch::channel(None);
    let (_shutdown, shutdown) = watch::channel(false);
    let io = Arc::new(Semaphore::new(1));
    let owner = prepared
        .start_with_checkpoints(
            snapshots,
            shutdown,
            Arc::new(Semaphore::new(2)),
            Arc::clone(&io),
        )
        .unwrap();
    barrier.wait().await;
    assert_eq!(io.available_permits(), 0);
    owner.stop();
    let mut first_waiter = Box::pin(owner.join());
    assert!(matches!(
        futures_util::poll!(&mut first_waiter),
        Poll::Pending
    ));
    drop(first_waiter); // Only the observer is abandoned, not the task handle.
    let mut resumed = Box::pin(owner.join());
    assert!(matches!(futures_util::poll!(&mut resumed), Poll::Pending));
    assert_eq!(io.available_permits(), 0);
    assert!(fixture.probe.transport_paths.lock().unwrap().is_empty());
    barrier.release();
    tokio::time::timeout(Duration::from_secs(10), resumed)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owner.join().await, Ok(()));
    assert_eq!(io.available_permits(), 1);
    assert!(
        fixture.probe.transport_paths.lock().unwrap().is_empty(),
        "stopped FS return must not continue into HTTP"
    );
    drop(fixture.prepared()); // Reacquires the actual same-scope stream locks.
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_checkpoint_refusal_stops_sibling_but_waits_for_its_fs_join() {
    let mut fixture = LiveFixture::new().await;
    checkpoint_http_tests::enable(&mut fixture);
    let AuthenticatedWorkload::Broker(identity) = fixture
        .store
        .authenticate_workload_token(OWN_TOKEN)
        .unwrap()
        .unwrap()
    else {
        panic!("real fixture Broker binding");
    };
    let snapshot = checkpoint_http_tests::snapshot(&fixture, 17);
    fixture
        .store
        .broker_checkpoint(
            &identity,
            &crate::broker_registry::BrokerCheckpointV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                incarnation_id: fixture.filesystem.value["incarnation_id"]
                    .as_str()
                    .unwrap()
                    .into(),
                previous_sequence: 0,
                sequence: 1,
                snapshot: snapshot.clone(),
            },
        )
        .unwrap();
    let barrier = DiskBarrier::new();
    let _release = Release(Arc::clone(&barrier));
    let mut prepared = fixture.prepared();
    prepared.disk_barrier = Some(Arc::clone(&barrier));
    let (source, snapshots) = watch::channel(None);
    let (_shutdown, shutdown) = watch::channel(false);
    let io = Arc::new(Semaphore::new(2));
    let owner = prepared
        .start_with_checkpoints(
            snapshots,
            shutdown,
            Arc::new(Semaphore::new(2)),
            Arc::clone(&io),
        )
        .unwrap();
    barrier.wait().await;
    // The real remote publication cannot be adopted by an empty local stream.
    // This produces the actual Conflict path after identity and self reads.
    source.send(Some(snapshot)).unwrap();
    // This barrier is the supervisor's actual sibling-stop signal, not a sleep.
    tokio::time::timeout(Duration::from_secs(10), owner.stopped())
        .await
        .unwrap();
    let requests_at_stop = fixture.probe.transport_paths.lock().unwrap().clone();
    assert_eq!(requests_at_stop.len(), 2);
    let mut join = Box::pin(owner.join());
    assert!(matches!(futures_util::poll!(&mut join), Poll::Pending));
    assert_eq!(io.available_permits(), 1);
    barrier.release();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), join)
            .await
            .unwrap(),
        Err(PresenceError::Conflict)
    );
    assert_eq!(owner.join().await, Err(PresenceError::Conflict));
    assert_eq!(io.available_permits(), 2);
    assert_eq!(
        *fixture.probe.transport_paths.lock().unwrap(),
        requests_at_stop,
        "stopped FS return must not initiate another HTTP request"
    );
}

#[test]
fn owner_start_without_tokio_is_a_typed_refusal() {
    use std::{fs, os::unix::fs::PermissionsExt};
    let mut fixture = preparation_tests::Fixture::new();
    let root = fixture.root.join("checkpoint-state");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fixture.value["version"] = serde_json::json!(2);
    fixture.value["checkpoint_state_dir"] = serde_json::json!(root);
    fixture.value["checkpoint_max_operations"] = serde_json::json!(8);
    fixture.value["checkpoint_max_state_bytes"] = serde_json::json!(1048576);
    fixture.write_config();
    let mut prepared = PreparedPresence::load(&fixture.config, fixture.scope(), vec![]).unwrap();
    prepared.finalize(&[]).unwrap();
    let (_source, snapshots) = watch::channel(None);
    let (_shutdown, shutdown) = watch::channel(false);
    assert!(matches!(
        prepared.start_with_checkpoints(
            snapshots,
            shutdown,
            Arc::new(Semaphore::new(2)),
            Arc::new(Semaphore::new(1))
        ),
        Err(PresenceError::Unavailable)
    ));
}
