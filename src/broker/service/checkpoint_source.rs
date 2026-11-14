//! Owned publication from acknowledged diagnostic heads, never a runtime
//! inventory or execution capability. The service retains this owner to join.
//! The legacy and owned presence profiles share this owner, its sampler and
//! its stop/join; the explicitly selected profile alone decides which runs.
use super::*;
use crate::{
    broker::presence::{PreparedPresence, PresenceRunOwner},
    broker_registry::{BrokerAuthorityV1, BrokerCheckpointSnapshotV1, BrokerJobCheckpointV1},
};
use std::sync::Weak;
use tokio::{sync::Mutex, task::JoinHandle};

type PublicationResult<T> = std::result::Result<T, PresenceError>;
/// Sampled acknowledged heads. `None` declares nothing; it is never zero.
pub(super) type Snapshots = watch::Receiver<Option<BrokerCheckpointSnapshotV1>>;

/// Checkpoint source of a profile whose configuration explicitly enables
/// checkpoints: the single authority its heads are declared for and the
/// sampling cadence. No source exists, and nothing is sampled, otherwise.
pub(super) struct CheckpointSource {
    pub(super) authority: BrokerAuthorityV1,
    pub(super) cadence: Duration,
}

/// A prepared presence profile the service can own. Implementations only
/// report their explicit configuration and start their retained actors; the
/// snapshots, when present, come exclusively from [`confirmed_snapshot`].
pub(super) trait PresencePublisher {
    /// `Some` exactly when the profile's configuration enables checkpoints.
    fn checkpoint_source(&self) -> Option<CheckpointSource>;
    /// Start the retained owner; `snapshots` is `Some` exactly when
    /// [`checkpoint_source`](Self::checkpoint_source) returned `Some`.
    fn start_owner(
        self,
        snapshots: Option<Snapshots>,
        shutdown: watch::Receiver<bool>,
        attempts: Arc<Semaphore>,
        io: Arc<Semaphore>,
    ) -> PublicationResult<PresenceRunOwner>;
}

impl PresencePublisher for PreparedPresence {
    fn checkpoint_source(&self) -> Option<CheckpointSource> {
        self.checkpoints_enabled().then(|| CheckpointSource {
            authority: self.checkpoint_authority(),
            cadence: self.publication_interval(),
        })
    }

    fn start_owner(
        self,
        snapshots: Option<Snapshots>,
        shutdown: watch::Receiver<bool>,
        attempts: Arc<Semaphore>,
        io: Arc<Semaphore>,
    ) -> PublicationResult<PresenceRunOwner> {
        match snapshots {
            Some(snapshots) => self.start_with_checkpoints(snapshots, shutdown, attempts, io),
            None => self.start(shutdown, attempts, io),
        }
    }
}

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
    /// One retained owner per prepared profile. With a checkpoint source, the
    /// publisher starts before its sampler, which offers only acknowledged
    /// heads of an already admitted Full writer; without one, no sampler runs.
    pub(super) fn start<P: PresencePublisher>(
        service: &Arc<Service>,
        profile: String,
        presence: P,
        shutdown: watch::Receiver<bool>,
        attempts: Arc<Semaphore>,
        io: Arc<Semaphore>,
    ) -> PublicationResult<Self> {
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| PresenceError::Unavailable)?;
        let (stop, stopped) = watch::channel(false);
        let (publisher, task) = match presence.checkpoint_source() {
            Some(CheckpointSource { authority, cadence }) => {
                let (snapshots, source) = watch::channel(None);
                let publisher =
                    presence.start_owner(Some(source), shutdown.clone(), attempts, io)?;
                let service = Arc::downgrade(service);
                let task = runtime.spawn(sample(
                    service, profile, authority, cadence, snapshots, shutdown, stopped,
                ));
                (publisher, Some(task))
            }
            None => (presence.start_owner(None, shutdown, attempts, io)?, None),
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
