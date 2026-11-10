//! Sample the existing Full writer's durable heads, never runtime inventory.
//! The service task retains this publication through the actual source/FS join.
use super::*;
use crate::{
    broker::presence::PresenceRunOwner,
    broker_registry::{BrokerAuthorityV1, BrokerCheckpointSnapshotV1, BrokerJobCheckpointV1},
};
use futures_util::FutureExt;
use std::{panic::AssertUnwindSafe, sync::Weak};
use tokio::{sync::Mutex, task::JoinHandle};

type PublicationResult<T> = std::result::Result<T, PresenceError>;

struct SourceState {
    task: Option<JoinHandle<PublicationResult<()>>>,
    result: Option<PublicationResult<()>>,
}

pub(super) struct Publication {
    publisher: Arc<PresenceRunOwner>,
    stop: watch::Sender<bool>,
    source: Mutex<SourceState>,
}

impl Publication {
    pub(super) fn start(
        service: &Arc<Service>,
        profile: String,
        presence: PreparedPresence,
        shutdown: watch::Receiver<bool>,
        attempts: Arc<Semaphore>,
        io: Arc<Semaphore>,
    ) -> PublicationResult<Self> {
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| PresenceError::Unavailable)?;
        let (stop, stopped) = watch::channel(false);
        let (publisher, task) = if presence.checkpoints_enabled() {
            let authority = presence.checkpoint_authority();
            let cadence = presence.publication_interval();
            let (snapshots, source) = watch::channel(None);
            let publisher = Arc::new(presence.start_with_checkpoints(
                source,
                shutdown.clone(),
                attempts,
                io,
            )?);
            let source_publisher = Arc::clone(&publisher);
            let service = Arc::downgrade(service);
            let task = runtime.spawn(async move {
                let result = AssertUnwindSafe(sample(
                    service, profile, authority, cadence, snapshots, shutdown, stopped,
                ))
                .catch_unwind()
                .await
                .map_err(|_| PresenceError::Unavailable);
                // Even an unexpected source panic stops its sibling. The
                // retained publisher still drains all admitted filesystem work.
                source_publisher.stop();
                result
            });
            (publisher, Some(task))
        } else {
            (Arc::new(presence.start(shutdown, attempts, io)?), None)
        };
        Ok(Self {
            publisher,
            stop,
            source: Mutex::new(SourceState { task, result: None }),
        })
    }

    pub(super) fn stop(&self) {
        self.stop.send_replace(true);
        self.publisher.stop();
    }

    /// Dropping a waiter does not take either owned handle. The service's
    /// JoinSet retains this publication until the actual join completes.
    pub(super) async fn join(&self) -> PublicationResult<()> {
        let publication = self.publisher.join().await;
        self.stop();
        let mut source = self.source.lock().await;
        if let Some(result) = source.result {
            return result;
        }
        let joined = match source.task.as_mut() {
            Some(task) => task.await.unwrap_or(Err(PresenceError::Unavailable)),
            None => Ok(()),
        };
        source.task.take();
        let result = publication.and(joined);
        source.result = Some(result);
        result
    }
}

impl Drop for Publication {
    fn drop(&mut self) {
        // Stop is a fallback, never evidence of a successful join.
        self.stop();
    }
}

async fn stopping(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow() {
            return;
        }
        if stop.changed().await.is_err() {
            return;
        }
    }
}

async fn sample(
    service: Weak<Service>,
    profile: String,
    authority: BrokerAuthorityV1,
    cadence: Duration,
    snapshots: watch::Sender<Option<BrokerCheckpointSnapshotV1>>,
    mut shutdown: watch::Receiver<bool>,
    mut stop: watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval(cadence);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! { biased;
            _ = stopping(&mut shutdown) => break,
            _ = stopping(&mut stop) => break,
            _ = snapshots.closed() => break,
            _ = tick.tick() => {
                let Some(service) = service.upgrade() else { break };
                // One existing FIFO read at a time. No writer is opened or
                // created to populate a diagnostic view.
                let snapshot = capture(&service, &profile, &authority).await;
                if *shutdown.borrow() || *stop.borrow()
                    || shutdown.has_changed().is_err() || stop.has_changed().is_err()
                {
                    break;
                }
                snapshots.send_replace(if available(&service) { snapshot } else { None });
            },
        }
    }
    snapshots.send_replace(None);
}

fn available(service: &Service) -> bool {
    !service.stopping.load(Ordering::SeqCst)
        && !service.fenced.load(Ordering::SeqCst)
        && service.material.complete
}

fn scope_matches(service: &Service, profile_id: &str, scope: &JournalScopeV1) -> bool {
    service.registry.profile(profile_id).is_ok_and(|profile| {
        scope.installation_id() == service.registry.data.installation_id
            && scope.profile_id() == profile_id
            && scope.tenant_id() == profile.tenant_id
            && scope.control_namespace_uuid() == profile.control_namespace_uuid
            && scope.gateway_origin() == profile.gateway
    })
}

async fn capture(
    service: &Service,
    profile_id: &str,
    authority: &BrokerAuthorityV1,
) -> Option<BrokerCheckpointSnapshotV1> {
    if !available(service) {
        return None;
    }
    let jobs = service.profiles.get(profile_id)?;
    let writer = jobs.writer.get()?.full()?;
    if !scope_matches(service, profile_id, writer.scope()) {
        return None;
    }
    // A failed/tainted/busy writer stays unknown. Only this real successful
    // ACK can establish a known empty source, still declared_partial.
    let page = writer.list(None, 100).await.ok()?;
    if !available(service) || !scope_matches(service, profile_id, writer.scope()) {
        return None;
    }
    let mut missing_heads = false;
    let mut heads = Vec::with_capacity(page.entries().len());
    for entry in page.entries() {
        if let Some(head) = entry.head() {
            heads.push(BrokerJobCheckpointV1 {
                job_id: entry.operation_id().to_string(),
                revision: head.revision(),
                checkpoint_hash: head.hash().into(),
            });
        } else {
            missing_heads = true;
        }
    }
    let snapshot = BrokerCheckpointSnapshotV1 {
        version: 1,
        authority: authority.clone(),
        jobs: heads,
        // Paging or omitted non-head entries cannot imply full coverage.
        has_more: page.next_after().is_some() || missing_heads,
    };
    snapshot.validate().ok()?;
    Some(snapshot)
}
