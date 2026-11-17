//! Ownership of native work, separate from the asynchronous listener.
//!
//! There is one joiner per acquired role lease. It never mutates a slot or
//! claims retirement: legacy actors can run until process exit.

use super::{
    Actor, EnforcerError, EnforcerResult, ProspectiveActor, RecoveryAttemptOwner, RoleLease, State,
};
use crate::broker::discovery::EnforcerRoleLeaseV2;
#[cfg(test)]
use std::sync::atomic::AtomicU64;
use std::{
    collections::BTreeMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel},
    },
    thread::JoinHandle,
    time::Duration,
};
use uuid::Uuid;

#[derive(Clone)]
enum LeaseOwner {
    Legacy(Arc<RoleLease>),
    Sealed(EnforcerRoleLeaseV2),
}
impl LeaseOwner {
    fn revalidate(&self) -> EnforcerResult<()> {
        match self {
            Self::Legacy(lease) => lease.revalidate().map_err(Into::into),
            Self::Sealed(lease) => lease.revalidate().map_err(Into::into),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::broker::enforcer) struct ActorKey {
    pub(in crate::broker::enforcer) arm_id: Uuid,
    pub(in crate::broker::enforcer) epoch: Uuid,
    pub(in crate::broker::enforcer) nonce: Uuid,
}

struct KeyedHandle {
    key: ActorKey,
    native: NativeHandle,
}

#[derive(Clone)]
pub(in crate::broker::enforcer) struct NativeOwners {
    sender: SyncSender<Envelope>,
    lease: LeaseOwner,
    failed: Arc<AtomicBool>,
    keyed: Arc<Mutex<BTreeMap<Uuid, KeyedHandle>>>,
    legacy_custody: Arc<Mutex<LegacyCustody>>,
    #[cfg(test)]
    joined_native: Arc<AtomicU64>,
}

// These are aliases to the SAME native cells/maps used by the listener. This
// adds no admission pool, runtime or thread. The independent joiner owns their
// actual snapshots/raw causes even after a native thread and State disappear.
#[derive(Default)]
struct LegacyCustody {
    targets: Option<LegacyTargets>,
    catchup: Option<RecoveryAttemptOwner>,
}
struct LegacyTargets {
    slots: Arc<Mutex<BTreeMap<String, Arc<Actor>>>>,
    prospective: Arc<Mutex<BTreeMap<String, Arc<ProspectiveActor>>>>,
}

enum Work {
    Progress(Option<NativeHandle>),
    Actor(Option<NativeHandle>),
    ListenerEnd { idle: bool },
}

// A failed/unwound transfer must not drop either a native handle or its lease.
// The envelope also guards messages that have no handle (known spawn failure).
struct Envelope {
    work: Option<Work>,
    lease: LeaseOwner,
    failed: Arc<AtomicBool>,
}

impl Drop for Envelope {
    fn drop(&mut self) {
        if let Some(work) = self.work.take() {
            self.failed.store(true, Ordering::SeqCst);
            std::mem::forget((self.lease.clone(), work));
        }
    }
}

pub(in crate::broker::enforcer) struct NativeHandle {
    handle: Option<JoinHandle<()>>,
    lease: LeaseOwner,
    failed: Arc<AtomicBool>,
    #[cfg(test)]
    joined_native: Arc<AtomicU64>,
}

impl NativeHandle {
    pub(in crate::broker::enforcer) fn join(&mut self) -> EnforcerResult<()> {
        // join consumes the handle even when its thread panicked. The joiner's
        // outer owner still retains the same lease and all remaining handles.
        let result = self
            .handle
            .take()
            .ok_or(EnforcerError::Unhealthy)?
            .join()
            .map_err(|_| EnforcerError::Unhealthy);
        #[cfg(test)]
        if result.is_ok() {
            self.joined_native.fetch_add(1, Ordering::SeqCst);
        }
        result
    }
}

impl Drop for NativeHandle {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.failed.store(true, Ordering::SeqCst);
            std::mem::forget((self.lease.clone(), handle));
        }
    }
}

enum Progress {
    Pending,
    Started(NativeHandle),
    NotStarted,
    Joined,
}

struct Joiner {
    _lease: LeaseOwner,
    receiver: Receiver<Envelope>,
    done: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    progress: Progress,
    actors: Vec<NativeHandle>,
    actor_limit: usize,
    actor_count: usize,
    actor_not_started: bool,
    ended: Option<bool>,
    keyed: Arc<Mutex<BTreeMap<Uuid, KeyedHandle>>>,
    _legacy_custody: Arc<Mutex<LegacyCustody>>,
}

impl NativeOwners {
    pub(in crate::broker::enforcer) async fn finish_sealed_idle(
        &self,
        joiner: JoinHandle<EnforcerResult<()>>,
    ) -> EnforcerResult<()> {
        self.send(Work::ListenerEnd { idle: true })?;
        tokio::task::spawn_blocking(move || joiner.join().map_err(|_| EnforcerError::Unhealthy)?)
            .await
            .map_err(|_| EnforcerError::Unhealthy)?
    }
    pub(super) fn start(
        lease: Arc<RoleLease>,
        done: Arc<AtomicBool>,
        max_slots: u32,
    ) -> EnforcerResult<(Self, JoinHandle<EnforcerResult<()>>)> {
        Self::start_owned(LeaseOwner::Legacy(lease), done, max_slots)
    }

    pub(in crate::broker::enforcer) fn start_sealed(
        lease: EnforcerRoleLeaseV2,
        done: Arc<AtomicBool>,
        max_slots: u32,
    ) -> EnforcerResult<(Self, JoinHandle<EnforcerResult<()>>)> {
        Self::start_owned(LeaseOwner::Sealed(lease), done, max_slots)
    }

    fn start_owned(
        lease: LeaseOwner,
        done: Arc<AtomicBool>,
        max_slots: u32,
    ) -> EnforcerResult<(Self, JoinHandle<EnforcerResult<()>>)> {
        lease.revalidate()?;
        let actor_limit = max_slots as usize;
        // Exactly one outcome per newly reserved slot (no replay sends), one
        // progress outcome and one listener end. Slots are never evicted.
        let (sender, receiver) = sync_channel(actor_limit + 2);
        let failed = Arc::new(AtomicBool::new(false));
        let keyed = Arc::new(Mutex::new(BTreeMap::new()));
        let legacy_custody = Arc::new(Mutex::new(LegacyCustody::default()));
        let joiner = Joiner {
            _lease: lease.clone(),
            receiver,
            done,
            failed: Arc::clone(&failed),
            progress: Progress::Pending,
            actors: Vec::with_capacity(actor_limit),
            actor_limit,
            actor_count: 0,
            actor_not_started: false,
            ended: None,
            keyed: Arc::clone(&keyed),
            _legacy_custody: Arc::clone(&legacy_custody),
        };
        // Refuse startup before progress, actors or IPC admission if this
        // owner cannot be created. No new flock or discovery format is used.
        let handle = std::thread::Builder::new()
            .name("enforcer-owners".into())
            .stack_size(256 * 1024)
            .spawn(move || joiner.execute())
            .map_err(|_| EnforcerError::Unavailable)?;
        Ok((
            Self {
                sender,
                lease,
                failed,
                keyed,
                legacy_custody,
                #[cfg(test)]
                joined_native: Arc::new(AtomicU64::new(0)),
            },
            handle,
        ))
    }

    pub(in crate::broker::enforcer) fn failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    pub(super) fn retain_legacy_targets(
        &self,
        slots: Arc<Mutex<BTreeMap<String, Arc<Actor>>>>,
        prospective: Arc<Mutex<BTreeMap<String, Arc<ProspectiveActor>>>>,
    ) -> EnforcerResult<()> {
        let mut custody = self
            .legacy_custody
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        if custody.targets.is_some() {
            return Err(EnforcerError::Conflict);
        }
        custody.targets = Some(LegacyTargets { slots, prospective });
        Ok(())
    }
    pub(in crate::broker::enforcer) fn retain_legacy_catchup(
        &self,
        attempt: RecoveryAttemptOwner,
    ) -> EnforcerResult<()> {
        let mut custody = self
            .legacy_custody
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        if custody.targets.is_none() || custody.catchup.is_some() {
            return Err(EnforcerError::Conflict);
        }
        custody.catchup = Some(attempt);
        Ok(())
    }
    pub(in crate::broker::enforcer) fn failure_latch(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.failed)
    }
    #[cfg(test)]
    pub(super) fn fixture_join_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.joined_native)
    }
    pub(super) fn release_legacy_idle_custody(&self) -> EnforcerResult<()> {
        let mut custody = self
            .legacy_custody
            .lock()
            .map_err(|_| EnforcerError::Unhealthy)?;
        if let Some(targets) = &custody.targets {
            if !targets
                .slots
                .lock()
                .map_err(|_| EnforcerError::Unhealthy)?
                .is_empty()
                || !targets
                    .prospective
                    .lock()
                    .map_err(|_| EnforcerError::Unhealthy)?
                    .is_empty()
            {
                return Err(EnforcerError::NotIdle);
            }
        }
        if self.failed()
            || custody
                .catchup
                .as_ref()
                .is_some_and(|attempt| !attempt.known_idle_joined())
        {
            return Err(EnforcerError::Unhealthy);
        }
        custody.catchup.take();
        custody.targets.take();
        Ok(())
    }

    pub(in crate::broker::enforcer) fn actor_exited(&self) {
        self.failed.store(true, Ordering::SeqCst);
    }

    fn native_handle(&self, handle: JoinHandle<()>) -> NativeHandle {
        NativeHandle {
            handle: Some(handle),
            lease: self.lease.clone(),
            failed: Arc::clone(&self.failed),
            #[cfg(test)]
            joined_native: Arc::clone(&self.joined_native),
        }
    }

    pub(in crate::broker::enforcer) fn progress(
        &self,
        handle: Option<JoinHandle<()>>,
    ) -> EnforcerResult<()> {
        self.send(Work::Progress(
            handle.map(|handle| self.native_handle(handle)),
        ))
    }

    pub(super) fn actor(&self, handle: Option<JoinHandle<()>>) -> EnforcerResult<()> {
        self.send(Work::Actor(handle.map(|handle| self.native_handle(handle))))
    }

    pub(in crate::broker::enforcer) fn actor_keyed(
        &self,
        key: ActorKey,
        handle: JoinHandle<()>,
    ) -> EnforcerResult<()> {
        let native = self.native_handle(handle);
        let mut keyed = self.keyed.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if key.arm_id.is_nil()
            || key.epoch.is_nil()
            || key.nonce.is_nil()
            || keyed.contains_key(&key.arm_id)
        {
            return Err(EnforcerError::Conflict);
        }
        keyed.insert(key.arm_id, KeyedHandle { key, native });
        Ok(())
    }

    #[cfg(test)]
    pub(in crate::broker::enforcer) fn keyed_thread_id(
        &self,
        key: ActorKey,
    ) -> EnforcerResult<std::thread::ThreadId> {
        let keyed = self.keyed.lock().map_err(|_| EnforcerError::Unhealthy)?;
        let owned = keyed
            .get(&key.arm_id)
            .filter(|owned| owned.key == key)
            .ok_or(EnforcerError::Conflict)?;
        Ok(owned
            .native
            .handle
            .as_ref()
            .ok_or(EnforcerError::Unhealthy)?
            .thread()
            .id())
    }
    #[cfg(test)]
    pub(super) fn keyed_finished(&self, key: ActorKey) -> bool {
        self.keyed
            .lock()
            .unwrap()
            .get(&key.arm_id)
            .filter(|owner| owner.key == key)
            .is_some_and(|owner| {
                owner
                    .native
                    .handle
                    .as_ref()
                    .is_some_and(|handle| handle.is_finished())
            })
    }

    pub(in crate::broker::enforcer) fn take_retiring(
        &self,
        key: ActorKey,
    ) -> EnforcerResult<NativeHandle> {
        let mut keyed = self.keyed.lock().map_err(|_| EnforcerError::Unhealthy)?;
        if keyed.get(&key.arm_id).is_none_or(|owned| owned.key != key) {
            return Err(EnforcerError::Conflict);
        }
        Ok(keyed
            .remove(&key.arm_id)
            .ok_or(EnforcerError::Conflict)?
            .native)
    }

    fn send(&self, work: Work) -> EnforcerResult<()> {
        let message = Envelope {
            work: Some(work),
            lease: self.lease.clone(),
            failed: Arc::clone(&self.failed),
        };
        // On Full/Disconnected or unwind the message's guard pins ownership.
        self.sender
            .try_send(message)
            .map_err(|_| EnforcerError::Unhealthy)
    }
}

impl Joiner {
    fn execute(mut self) -> EnforcerResult<()> {
        let result = catch_unwind(AssertUnwindSafe(|| self.run()));
        match result {
            Ok(Ok(())) => Ok(()),
            _ => {
                self.failed.store(true, Ordering::SeqCst);
                self.done.store(true, Ordering::SeqCst);
                // Make the independent progress join attempt before fencing
                // any unknown actor ownership. A failed join may already have
                // consumed its handle; it must never be reconstructed.
                let _ = catch_unwind(AssertUnwindSafe(|| self.join_progress()));
                // Unknown is deliberately fenced until process exit. Bound:
                // one such owner per role, <= N actor handles + 1 progress
                // handle and <= N + 2 guarded messages. There is no restart,
                // slot eviction or extra clock thread on this path.
                std::mem::forget(self);
                Err(EnforcerError::Unhealthy)
            }
        }
    }

    fn run(&mut self) -> EnforcerResult<()> {
        loop {
            let received = if self.done.load(Ordering::SeqCst)
                && matches!(self.progress, Progress::NotStarted | Progress::Joined)
            {
                // Once progress is known stopped, retain legacy actors with
                // a blocking memory receive, not a new polling clock loop.
                self.receiver
                    .recv()
                    .map_err(|_| RecvTimeoutError::Disconnected)
            } else {
                self.receiver.recv_timeout(Duration::from_millis(25))
            };
            match received {
                Ok(mut envelope) => match envelope.work.take().ok_or(EnforcerError::Unhealthy)? {
                    Work::Progress(handle) => {
                        if !matches!(self.progress, Progress::Pending) {
                            return Err(EnforcerError::Unhealthy);
                        }
                        self.progress = match handle {
                            Some(handle) => Progress::Started(handle),
                            None => Progress::NotStarted,
                        };
                    }
                    Work::Actor(handle) => {
                        if self.actor_count >= self.actor_limit {
                            return Err(EnforcerError::Unhealthy);
                        }
                        self.actor_count += 1;
                        if let Some(handle) = handle {
                            self.actors.push(handle);
                        } else {
                            self.actor_not_started = true;
                        }
                    }
                    Work::ListenerEnd { idle } => {
                        if self.ended.replace(idle).is_some() {
                            return Err(EnforcerError::Unhealthy);
                        }
                        self.done.store(true, Ordering::SeqCst);
                    }
                },
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.done.store(true, Ordering::SeqCst);
                    self.join_progress()?;
                    if self.ended.is_none() || self.actor_not_started {
                        return Err(EnforcerError::Unhealthy);
                    }
                    // The progress thread has actually joined before waiting
                    // for any legacy STOP actor. No business STOP is awaited
                    // in order to stop the independent clock.
                    for actor in &mut self.actors {
                        actor.join()?;
                    }
                    for actor in self
                        .keyed
                        .lock()
                        .map_err(|_| EnforcerError::Unhealthy)?
                        .values_mut()
                    {
                        actor.native.join()?;
                    }
                    return if self.actor_count == 0 && !self.failed.load(Ordering::SeqCst) {
                        Ok(())
                    } else {
                        // A returning actor does not establish retirement.
                        Err(EnforcerError::Unhealthy)
                    };
                }
            }
            if self.done.load(Ordering::SeqCst) && !matches!(self.progress, Progress::Pending) {
                self.join_progress()?;
            }
            if self.ended == Some(true) {
                // ShutdownIfIdle closes admission while holding the slot map
                // and only sends this message when that map is empty.
                if self.actor_count != 0
                    || !self
                        .keyed
                        .lock()
                        .map_err(|_| EnforcerError::Unhealthy)?
                        .is_empty()
                {
                    return Err(EnforcerError::Unhealthy);
                }
                if matches!(self.progress, Progress::NotStarted | Progress::Joined) {
                    return if self.failed.load(Ordering::SeqCst) {
                        Err(EnforcerError::Unhealthy)
                    } else {
                        Ok(())
                    };
                }
            }
        }
    }

    fn join_progress(&mut self) -> EnforcerResult<()> {
        match &mut self.progress {
            Progress::Started(handle) => {
                handle.join()?;
                self.progress = Progress::Joined;
                Ok(())
            }
            Progress::NotStarted | Progress::Joined => Ok(()),
            Progress::Pending => Err(EnforcerError::Unhealthy),
        }
    }
}

pub(super) struct ListenerOwner {
    owners: NativeOwners,
    done: Arc<AtomicBool>,
    state: Weak<State>,
    joiner: Option<JoinHandle<EnforcerResult<()>>>,
    ended: bool,
}

impl ListenerOwner {
    pub(super) fn new(
        owners: NativeOwners,
        done: Arc<AtomicBool>,
        state: Weak<State>,
        joiner: JoinHandle<EnforcerResult<()>>,
    ) -> Self {
        Self {
            owners,
            done,
            state,
            joiner: Some(joiner),
            ended: false,
        }
    }

    fn end(&mut self, idle: bool) -> EnforcerResult<()> {
        self.done.store(true, Ordering::SeqCst);
        if let Some(state) = self.state.upgrade() {
            state.healthy.store(false, Ordering::SeqCst);
            state.closing.store(true, Ordering::SeqCst);
        }
        self.ended = true;
        self.owners.send(Work::ListenerEnd { idle })
    }

    pub(super) async fn finish_idle(mut self) -> EnforcerResult<()> {
        self.end(true)?;
        let joiner = self.joiner.take().ok_or(EnforcerError::Unhealthy)?;
        // No slot/state lock is held. Only the zero-actor normal shutdown path
        // waits; abandonment never waits for native work in a Drop handler.
        tokio::task::spawn_blocking(move || joiner.join().map_err(|_| EnforcerError::Unhealthy)?)
            .await
            .map_err(|_| EnforcerError::Unhealthy)?
    }
}

impl Drop for ListenerOwner {
    fn drop(&mut self) {
        if !self.ended {
            let _ = catch_unwind(AssertUnwindSafe(|| self.end(false)));
        }
        // Explicitly detach only the nonmutative support joiner. It owns the
        // real actor/progress handles and the original lease; this is not a
        // join or a receipt of native quiescence.
        drop(self.joiner.take());
    }
}

pub(super) struct ProgressDone(pub(super) Arc<AtomicBool>, pub(super) NativeOwners);

impl Drop for ProgressDone {
    fn drop(&mut self) {
        if !self.0.swap(true, Ordering::SeqCst) {
            self.1.actor_exited();
        }
    }
}

pub(super) struct ActorExit(pub(super) NativeOwners);

impl Drop for ActorExit {
    fn drop(&mut self) {
        self.0.actor_exited();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{broker::local_ipc::IpcError, control_fs::Directory};
    use std::{fs, os::unix::fs::PermissionsExt};

    fn lease_fixture() -> (tempfile::TempDir, Directory, Arc<RoleLease>) {
        let temporary = tempfile::tempdir().expect("private owner fixture");
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700))
            .expect("private fixture permissions");
        let root = Directory::open_existing(
            &temporary
                .path()
                .canonicalize()
                .expect("fixture canonical path"),
        )
        .expect("private role root");
        let lease = Arc::new(RoleLease::acquire(&root).expect("original lease"));
        (temporary, root, lease)
    }

    fn owned_thread() -> (JoinHandle<()>, SyncSender<()>, Receiver<()>) {
        let (entered, entered_receiver) = sync_channel(1);
        let (release, release_receiver) = sync_channel(1);
        let (exited, exited_receiver) = sync_channel(1);
        let handle = std::thread::spawn(move || {
            entered.send(()).expect("owned thread entry");
            // No lease is captured here: only the handoff guard can retain
            // the flock after the test drops its ordinary lease references.
            let _ = release_receiver.recv();
            let _ = exited.send(());
        });
        entered_receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("real owned thread entered");
        // Dropping release also releases this test's thread on assertion panic.
        (handle, release, exited_receiver)
    }

    fn assert_busy(root: &Directory) {
        assert!(matches!(
            RoleLease::acquire_existing(root),
            Err(IpcError::Conflict)
        ));
    }

    #[tokio::test]
    async fn known_unstarted_progress_without_actors_releases_original_lease() {
        let (_temporary, root, lease) = lease_fixture();
        let done = Arc::new(AtomicBool::new(false));
        let (owners, joiner) = NativeOwners::start(Arc::clone(&lease), Arc::clone(&done), 1)
            .expect("real native owner");
        assert_busy(&root);
        owners.progress(None).expect("known NotStarted outcome");
        drop(lease);
        tokio::time::timeout(
            Duration::from_secs(3),
            ListenerOwner::new(owners, done, Weak::new(), joiner).finish_idle(),
        )
        .await
        .expect("bounded native owner join")
        .expect("zero-actor native owner actually joined");
        let _reacquired = RoleLease::acquire_existing(&root)
            .expect("NotStarted is known absence, not a permanent unknown fence");
    }

    #[test]
    fn refused_transfer_retains_original_lease_and_native_handle() {
        let (temporary, root, lease) = lease_fixture();
        // A retained unknown lease is intentionally process-lived; never
        // unlink the inode that proves its fence when this test returns.
        let preserved = temporary.keep();
        eprintln!("private S0 retained fence: {}", preserved.display());
        let (sender, receiver) = sync_channel(3);
        let failed = Arc::new(AtomicBool::new(false));
        let owners = NativeOwners {
            sender,
            lease: LeaseOwner::Legacy(Arc::clone(&lease)),
            failed: Arc::clone(&failed),
            keyed: Arc::new(Mutex::new(BTreeMap::new())),
            legacy_custody: Arc::new(Mutex::new(LegacyCustody::default())),
            #[cfg(test)]
            joined_native: Arc::new(AtomicU64::new(0)),
        };
        let (handle, release, exited) = owned_thread();
        assert_busy(&root);
        drop(receiver);
        assert_eq!(owners.actor(Some(handle)), Err(EnforcerError::Unhealthy));
        drop(owners);
        drop(lease);
        assert!(failed.load(Ordering::SeqCst));
        assert_busy(&root);
        drop(release);
        exited
            .recv_timeout(Duration::from_secs(3))
            .expect("owned fixture payload released");
        // This marker is not a join. The pinned native handle and lease have
        // no recovery/quiescence claim and remain retained until process exit.
        assert_busy(&root);
    }

    #[test]
    fn unwound_transfer_retains_original_lease_and_native_handle() {
        let (temporary, root, lease) = lease_fixture();
        let preserved = temporary.keep();
        eprintln!("private S0 retained fence: {}", preserved.display());
        let (sender, receiver) = sync_channel(3);
        let failed = Arc::new(AtomicBool::new(false));
        let owners = NativeOwners {
            sender,
            lease: LeaseOwner::Legacy(Arc::clone(&lease)),
            failed: Arc::clone(&failed),
            keyed: Arc::new(Mutex::new(BTreeMap::new())),
            legacy_custody: Arc::new(Mutex::new(LegacyCustody::default())),
            #[cfg(test)]
            joined_native: Arc::new(AtomicU64::new(0)),
        };
        let (handle, release, exited) = owned_thread();
        assert_busy(&root);
        let message = Envelope {
            work: Some(Work::Actor(Some(owners.native_handle(handle)))),
            lease: owners.lease.clone(),
            failed: Arc::clone(&failed),
        };
        let unwound = catch_unwind(AssertUnwindSafe(move || {
            let _message = message;
            panic!("fixture unwind with a protected native handoff message");
        }));
        assert!(unwound.is_err());
        drop(receiver);
        drop(owners);
        drop(lease);
        assert!(failed.load(Ordering::SeqCst));
        assert_busy(&root);
        drop(release);
        exited
            .recv_timeout(Duration::from_secs(3))
            .expect("owned fixture payload released without a join claim");
        assert_busy(&root);
    }
}
