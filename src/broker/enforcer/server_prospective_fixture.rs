//! Explicit software fixture for client ports. Real exclusive factory, IPC,
//! native handles and runtime contexts; no SyntheticReady, engine or Q. Exit
//! hooks collect only test-owned contexts AFTER the case has ended.
use super::*;
use crate::broker::discovery::load_endpoint;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{OnceLock, Weak, atomic::AtomicUsize},
};
use tokio::sync::oneshot;

pub(in crate::broker::enforcer) fn registration(root: &std::path::Path) -> RegistrationV1 {
    prospective::fixture_registration(root)
}

struct HeldReply {
    reached: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

/// Count authenticated reservations at the wire and hold one real capabilities
/// response after dispatch. Each private server owns an independent probe.
#[derive(Default)]
pub(super) struct ReserveProbe {
    requests: AtomicUsize,
    next_capabilities: Mutex<Option<HeldReply>>,
    next_before_start: Mutex<Option<HeldReply>>,
    next_reserve_ack: Mutex<Option<HeldReply>>,
    refuse_negotiation: AtomicBool,
}

impl ReserveProbe {
    pub(super) fn observe_request(&self, request: &Request) {
        if matches!(request, Request::ReserveProspective { .. }) {
            self.requests.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub(super) fn refuses_negotiation(&self) -> bool {
        self.refuse_negotiation.load(Ordering::SeqCst)
    }

    async fn hold(slot: &Mutex<Option<HeldReply>>) -> EnforcerResult<()> {
        let held = slot.lock().unwrap().take();
        if let Some(held) = held {
            let _ = held.reached.send(());
            tokio::time::timeout(IO_TIMEOUT, held.release)
                .await
                .map_err(|_| EnforcerError::Unavailable)?
                .map_err(|_| EnforcerError::Unavailable)?;
        }
        Ok(())
    }

    pub(super) async fn hold_capabilities_reply(&self) -> EnforcerResult<()> {
        Self::hold(&self.next_capabilities).await
    }

    pub(super) async fn hold_before_start(&self) -> EnforcerResult<()> {
        Self::hold(&self.next_before_start).await
    }

    pub(super) async fn hold_reserve_ack(&self) -> EnforcerResult<()> {
        Self::hold(&self.next_reserve_ack).await
    }
}

static STATES: OnceLock<Mutex<BTreeMap<PathBuf, Weak<State>>>> = OnceLock::new();
fn states() -> &'static Mutex<BTreeMap<PathBuf, Weak<State>>> {
    STATES.get_or_init(|| Mutex::new(BTreeMap::new()))
}
pub(super) fn observe_state(state: &Arc<State>) {
    states().lock().unwrap().insert(
        state._lease.root().path().to_path_buf(),
        Arc::downgrade(state),
    );
}
pub(in crate::broker::enforcer) struct LegacyIpcFixture {
    files: Option<tempfile::TempDir>,
    root: Directory,
    native: Option<Arc<State>>,
    server: Option<tokio::task::JoinHandle<EnforcerResult<()>>>,
    launch_nonce: Uuid,
}
impl LegacyIpcFixture {
    pub(in crate::broker::enforcer) async fn start(s: u32, w: u32) -> Self {
        let tmp = if cfg!(target_os = "macos") {
            "/private/tmp"
        } else {
            "/tmp"
        };
        let files = tempfile::Builder::new()
            .prefix("d7p-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(tmp)
            .unwrap();
        let parent = Directory::open_existing(&files.path().canonicalize().unwrap()).unwrap();
        let root_path = parent.path().join("r");
        let launch_nonce = Uuid::new_v4();
        let server = tokio::spawn(async move {
            run_internal_legacy_fresh(&parent, "r", s, w, launch_nonce).await
        });
        let native = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if let Some(native) = states()
                    .lock()
                    .unwrap()
                    .get(&root_path)
                    .and_then(Weak::upgrade)
                {
                    break native;
                }
                assert!(
                    !server.is_finished(),
                    "native fresh factory failed before publication"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("real native fresh factory state");
        let root = native._lease.root().clone();
        tokio::time::timeout(Duration::from_secs(8), async {
            while load_endpoint(&root).unwrap().is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("actual native endpoint publication");
        Self {
            files: Some(files),
            root,
            native: Some(native),
            server: Some(server),
            launch_nonce,
        }
    }
    pub(in crate::broker::enforcer) fn root(&self) -> &Directory {
        &self.root
    }
    pub(in crate::broker::enforcer) fn launch_nonce(&self) -> Uuid {
        self.launch_nonce
    }
    pub(in crate::broker::enforcer) fn reserve_requests(&self) -> usize {
        self.native
            .as_ref()
            .unwrap()
            .reserve_probe
            .requests
            .load(Ordering::SeqCst)
    }
    pub(in crate::broker::enforcer) fn pause_capabilities_reply(
        &self,
    ) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (reached, entered) = oneshot::channel();
        let (resume, release) = oneshot::channel();
        let probe = &self.native.as_ref().unwrap().reserve_probe;
        let mut next = probe.next_capabilities.lock().unwrap();
        assert!(
            next.is_none(),
            "only one capabilities response can be retained"
        );
        *next = Some(HeldReply { reached, release });
        (entered, resume)
    }

    /// Hold the next fresh reservation after its cell is retained but before
    /// the native worker starts, so a test can decide the installation outcome.
    pub(in crate::broker::enforcer) fn pause_before_start(
        &self,
    ) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (reached, entered) = oneshot::channel();
        let (resume, release) = oneshot::channel();
        let probe = &self.native.as_ref().unwrap().reserve_probe;
        let mut next = probe.next_before_start.lock().unwrap();
        assert!(next.is_none(), "only one pre-start hold can be retained");
        *next = Some(HeldReply { reached, release });
        (entered, resume)
    }

    /// Hold the next fresh reservation after its native worker started, before
    /// the ACK control loop publishes a grant.
    pub(in crate::broker::enforcer) fn pause_reserve_ack(
        &self,
    ) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (reached, entered) = oneshot::channel();
        let (resume, release) = oneshot::channel();
        let probe = &self.native.as_ref().unwrap().reserve_probe;
        let mut next = probe.next_reserve_ack.lock().unwrap();
        assert!(next.is_none(), "only one reserve ACK hold can be retained");
        *next = Some(HeldReply { reached, release });
        (entered, resume)
    }

    /// Simulate the historical peer decoder boundary: the negotiation request
    /// kind is unknown there, so the connection closes without any response.
    pub(in crate::broker::enforcer) fn refuse_motif_negotiation(&self) {
        self.native
            .as_ref()
            .unwrap()
            .reserve_probe
            .refuse_negotiation
            .store(true, Ordering::SeqCst);
    }

    pub(in crate::broker::enforcer) fn pending_reservation(
        &self,
        arm_id: &str,
    ) -> Option<ProspectiveReservationV1> {
        self.native
            .as_ref()
            .unwrap()
            .prospective
            .lock()
            .unwrap()
            .get(arm_id)
            .map(|actor| actor.fixture_reservation())
    }

    /// Make the retained native worker exit before installing any runtime
    /// context. Only a test that already holds the pre-start barrier may use
    /// this; the reservation then observes the real installation deadline.
    pub(in crate::broker::enforcer) fn exit_pending_before_install(&self, arm_id: &str) {
        let native = self.native.as_ref().unwrap();
        let actor = native
            .prospective
            .lock()
            .unwrap()
            .get(arm_id)
            .cloned()
            .expect("retained prospective target");
        let _key = actor.fixture_exit_key();
    }

    pub(in crate::broker::enforcer) async fn finish(mut self) {
        let native = self.native.as_ref().unwrap();
        {
            let _admission = native.admission.lock().unwrap();
            native.closing.store(true, Ordering::SeqCst);
        }
        let pending = native
            .prospective
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        // This small fixture collects prospective continuations only. It does
        // not pretend that an unkeyed historical STOP has retired.
        assert!(
            native.slots.lock().unwrap().keys().all(|arm| native
                .prospective
                .lock()
                .unwrap()
                .contains_key(arm)),
            "use the owned-child fixture for historical STOP targets"
        );
        let keys = pending
            .iter()
            .map(|owner| owner.fixture_exit_key())
            .collect::<Vec<_>>();
        for key in keys {
            let mut handle = native.owners.take_retiring(key).unwrap();
            tokio::task::spawn_blocking(move || handle.join())
                .await
                .unwrap()
                .expect("actual native test context joined");
        }
        drop(pending);
        native.slots.lock().unwrap().clear();
        native.prospective.lock().unwrap().clear();
        native.admission.lock().unwrap().entries.clear();
        if let Some(attempt) = &native.recovery_attempt {
            attempt.reserve_legacy_idle_shutdown().unwrap();
        }
        native.shutdown.send_replace(true);
        self.server
            .take()
            .unwrap()
            .await
            .unwrap()
            .expect("actual progress and single support joiner collected");
        states().lock().unwrap().remove(self.root.path());
        drop(self.native.take());
        drop(self.files.take());
    }
}
impl Drop for LegacyIpcFixture {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
            // A failed case is not quiescence. Keep private source/lock files
            // instead of unlinking an authority still held by native owners.
            if let Some(files) = self.files.take() {
                let _preserved = files.keep();
            }
        }
    }
}
