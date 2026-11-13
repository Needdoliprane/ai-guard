//! Owned publication from acknowledged diagnostic heads, never a runtime
//! inventory or execution capability. The service retains this owner to join.
use super::*;
use crate::{
    broker::presence::PresenceRunOwner,
    broker_registry::{BrokerAuthorityV1, BrokerCheckpointSnapshotV1, BrokerJobCheckpointV1},
};
use std::sync::Weak;
use tokio::{sync::Mutex, task::JoinHandle};

type PublicationResult<T> = std::result::Result<T, PresenceError>;

struct SourceState {
    task: Option<JoinHandle<()>>,
    result: Option<PublicationResult<()>>,
}

pub(super) struct Publication {
    publisher: PresenceRunOwner,
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
            let publisher =
                presence.start_with_checkpoints(source, shutdown.clone(), attempts, io)?;
            let service = Arc::downgrade(service);
            let task = runtime.spawn(sample(
                service, profile, authority, cadence, snapshots, shutdown, stopped,
            ));
            (publisher, Some(task))
        } else {
            (presence.start(shutdown, attempts, io)?, None)
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

    /// Cancelling this observer retains both real task handles for another
    /// observer. No owner or service lease may be released on a join timeout.
    pub(super) async fn join(&self) -> PublicationResult<()> {
        let publication = self.publisher.join().await;
        self.stop();
        let mut source = self.source.lock().await;
        if let Some(result) = source.result {
            return result;
        }
        let joined = match source.task.as_mut() {
            Some(task) => task.await.map_err(|_| PresenceError::Unavailable),
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
        // Stop is a fallback, not evidence of a completed join.
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
                // No file, gateway, engine or writer command runs here. A cold
                // source is unavailable, never a known empty inventory.
                let snapshot = service.upgrade().and_then(|service| {
                    confirmed_snapshot(&service, &profile, &authority)
                });
                snapshots.send_replace(snapshot);
            },
        }
    }
    snapshots.send_replace(None);
}

pub(super) fn confirmed_snapshot(
    service: &Service,
    profile_id: &str,
    authority: &BrokerAuthorityV1,
) -> Option<BrokerCheckpointSnapshotV1> {
    if service.stopping.load(Ordering::SeqCst)
        || service.fenced.load(Ordering::SeqCst)
        || !service.material.complete
    {
        return None;
    }
    let profile = service.registry.profile(profile_id).ok()?;
    let jobs = service.profiles.get(profile_id)?;
    // Do not open, create or authenticate a writer to populate a dashboard.
    // Only an already admitted Full owner can have acknowledged heads.
    let writer = jobs.writer.get()?.full()?;
    let scope = writer.scope();
    if scope.installation_id() != service.registry.data.installation_id
        || scope.profile_id() != profile_id
        || scope.tenant_id() != profile.tenant_id
        || scope.control_namespace_uuid() != profile.control_namespace_uuid
        || scope.gateway_origin() != profile.gateway
    {
        return None;
    }
    let heads = writer.last_confirmed_heads(100).ok()?;
    if heads.entries().is_empty() {
        return None;
    }
    let snapshot = BrokerCheckpointSnapshotV1 {
        version: 1,
        authority: authority.clone(),
        jobs: heads
            .entries()
            .iter()
            .map(|(operation, head)| BrokerJobCheckpointV1 {
                job_id: operation.to_string(),
                revision: head.revision(),
                checkpoint_hash: head.hash().into(),
            })
            .collect(),
        // This describes the retained diagnostic-cache window only. The
        // gateway always labels the whole report declared_partial.
        has_more: heads.has_more(),
    };
    snapshot.validate().ok()?;
    Some(snapshot)
}
