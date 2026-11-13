//! Idle-close admission regression with real sealed role, ledger, mutexes and
//! authenticated V3 connections. Runtime metadata and audit prefixes are
//! explicitly synthetic; spawn_once fails before any business OS thread.
use super::*;
use crate::broker::enforcer::{ledger::RetirementLedger, retirement::RegistryOwner};

struct IdleStopFixture {
    files: Option<tempfile::TempDir>,
    root: Directory,
    listener: PrivateListener,
    state: Arc<RecoveryState>,
    progress: Option<tokio::task::JoinHandle<()>>,
    connections: tokio::task::JoinSet<EnforcerResult<()>>,
}

impl IdleStopFixture {
    fn new() -> Self {
        let configured = std::env::var_os("AI_GUARD_TEST_D5_IPC_ROOT").map(PathBuf::from);
        let fallback = if cfg!(target_os = "macos") {
            "/private/tmp"
        } else {
            "/tmp"
        };
        let requested = configured.as_deref().unwrap_or_else(|| Path::new(fallback));
        assert!(
            requested.is_absolute(),
            "AI_GUARD_TEST_D5_IPC_ROOT must name an absolute directory"
        );
        let base = std::fs::canonicalize(requested)
            .expect("AI_GUARD_TEST_D5_IPC_ROOT or the native fallback must exist");
        assert!(base.is_dir(), "the D5 IPC fixture root must be a directory");
        if configured.is_some() {
            assert_eq!(
                requested,
                base.as_path(),
                "AI_GUARD_TEST_D5_IPC_ROOT must already be canonical"
            );
        }
        let files = tempfile::Builder::new()
            .prefix("idle-stop-")
            .rand_bytes(6)
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(base)
            .unwrap();
        let parent = Directory::open_existing(&files.path().canonicalize().unwrap()).unwrap();
        let ledger = Arc::new(
            RetirementLedger::initialize_new(
                parent.create_enforcer_root("role").unwrap(),
                Uuid::new_v4(),
            )
            .unwrap(),
        );
        let root = ledger.lease().ledger_root().clone();
        capability(&root, true).unwrap();
        let listener = PrivateListener::bind_new(&ledger.lease().listener_root().unwrap()).unwrap();
        let mut state = full_fixture_state(Arc::clone(&ledger), listener.endpoint(), 8).unwrap();
        // No inherited fault paths or external process selection. Every new
        // restored reservation consumes a locally rearmed pre-spawn failure.
        Arc::get_mut(&mut state).unwrap().faults = TestFaults {
            stage: "spawn_once".into(),
            arm_id: None,
            armed: AtomicBool::new(true),
            barrier_arm: None,
            barrier_root: None,
            barrier_once: AtomicBool::new(true),
            repeat_barrier: false,
            started_marker: None,
            fault_marker: None,
        };
        ledger
            .lease()
            .publish_endpoint(listener.endpoint())
            .unwrap();
        let ticking = Arc::clone(&state);
        let progress = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(25));
            loop {
                interval.tick().await;
                ticking.progress.fetch_add(1, Ordering::SeqCst);
            }
        });
        Self {
            files: Some(files),
            root,
            listener,
            state,
            progress: Some(progress),
            connections: tokio::task::JoinSet::new(),
        }
    }

    async fn authenticated(&mut self, role: RequestedRole) -> (UnixStream, Session3) {
        let endpoints = self.state.lease.as_ref().unwrap().listener_root().unwrap();
        let (server, client) = tokio::join!(
            self.listener.accept_checked(),
            self.listener.endpoint().connect_checked(&endpoints)
        );
        self.connections
            .spawn(serve_connection(server.unwrap(), Arc::clone(&self.state)));
        let mut stream = client.unwrap();
        let challenge = read_preface(&mut stream, IO_TIMEOUT).await.unwrap();
        assert_eq!(challenge.epoch, self.state.epoch);
        let session = client_handshake(
            &mut stream,
            &challenge,
            &self.state.capability,
            Instant::now() + HANDSHAKE_WINDOW,
            role,
            8,
        )
        .await
        .unwrap();
        assert_eq!(
            recovery_root_binding(&self.root, self.listener.endpoint()).unwrap(),
            challenge.root_binding
        );
        (stream, session)
    }

    async fn stop_connection(&mut self) -> StopConnection {
        let (stream, session) = self.authenticated(RequestedRole::StopsOnly).await;
        StopConnection {
            stream,
            epoch: self.state.epoch,
            v3: Some(session),
        }
    }

    fn restore_request(&self, suffix: &str) -> RestoreRequiredStopV1 {
        let registration = unavailable_registration(self.root.path(), suffix);
        let value = RestoreRequiredStopV1 {
            version: 1,
            request_id: Uuid::new_v4(),
            registration_hash: registration.digest().unwrap(),
            source: StopSourceV1 {
                scope_hash: "d".repeat(64),
                job_id: Uuid::new_v4(),
                intent_hash: registration.arm.intent_hash.clone(),
                confirmed_head: RevisionRefV1::new(1, &"b".repeat(64)).unwrap(),
                arm_operation_index: 0,
            },
            registration,
            cancel_generation: 0,
            durable_cancel: None,
            cancel_prefix: Vec::new(),
        };
        value.validate().unwrap();
        value
    }

    fn discard_unstarted_canary(&self, arm_id: &str) {
        // Fixture teardown only: this removes no running actor and produces no
        // retirement/quiescence witness. The actual reservation above is the
        // positive canary; its pre-spawn failure is independently checked.
        let mut admitted = self.state.admitted.lock().unwrap();
        let mut slots = self.state.slots.lock().unwrap();
        let actor = slots.get(arm_id).unwrap();
        assert!(!actor.worker_owned.load(Ordering::SeqCst));
        assert!(!actor.fail_spawn_once.load(Ordering::SeqCst));
        let snapshot = actor.snapshot().unwrap();
        assert_eq!(snapshot.phase, StopPhaseV1::Quarantined);
        assert_eq!(snapshot.stop_failures, 1);
        assert_eq!(admitted.len(), 1);
        assert!(matches!(
            admitted.get(arm_id),
            Some(RegistryOwner::Restored)
        ));
        slots.remove(arm_id);
        admitted.remove(arm_id);
        self.state.faults.armed.store(true, Ordering::SeqCst);
    }

    async fn finish(mut self) {
        while let Some(result) = self.connections.join_next().await {
            result.unwrap().unwrap();
        }
        if let Some(progress) = self.progress.take() {
            progress.abort();
            let _ = progress.await;
        }
        // A bad post-close reservation must also have failed before spawning.
        for actor in self.state.slots.lock().unwrap().values() {
            assert!(!actor.worker_owned.load(Ordering::SeqCst));
        }
        drop(self.files.take());
    }
}

impl Drop for IdleStopFixture {
    fn drop(&mut self) {
        self.connections.abort_all();
        if let Some(progress) = self.progress.take() {
            progress.abort();
        }
        if let Some(files) = self.files.take() {
            let _preserved = files.keep();
        }
    }
}

#[tokio::test]
async fn authenticated_stop_cannot_reserve_new_actor_after_idle_close() {
    let mut fixture = IdleStopFixture::new();

    let before = fixture.restore_request("idle-before-close");
    let before_arm = before.registration.arm.operation_id.clone();
    let mut before_connection = fixture.stop_connection().await;
    let before_reply = request(
        &mut before_connection,
        StopRequest::RestoreRequiredStop {
            value: Box::new(before),
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        before_reply,
        ReplyValue3::StopReserved {
            replayed: false,
            ..
        }
    ));
    drop(before_connection);
    fixture.discard_unstarted_canary(&before_arm);

    // Hold these real grants across close. The on-demand accept fixture keeps
    // them owned, so listener task cancellation cannot mask the admission bug.
    let (mut retained_full, retained_full_session) =
        fixture.authenticated(RequestedRole::Full).await;
    let (mut closing, closing_session) = fixture.authenticated(RequestedRole::Full).await;
    let mut retained_stop = fixture.stop_connection().await;
    let shutdown = request_full(
        &mut closing,
        &closing_session,
        FullCommand::ShutdownIfIdle {},
        REGISTER_TIMEOUT,
    )
    .await
    .unwrap();
    assert!(matches!(shutdown, FullReply::Acknowledged {}));
    assert!(fixture.state.shutdown_requested.load(Ordering::SeqCst));
    assert!(fixture.state.admitted.lock().unwrap().is_empty());
    assert!(fixture.state.slots.lock().unwrap().is_empty());

    let register = request_full(
        &mut retained_full,
        &retained_full_session,
        FullCommand::Register {
            registration: Box::new(unavailable_registration(
                fixture.root.path(),
                "idle-full-after-close",
            )),
            generation: 0,
            observe_only: false,
        },
        REGISTER_TIMEOUT,
    )
    .await
    .unwrap();
    assert!(matches!(
        register,
        FullReply::Error {
            code: EnforcerError::Unhealthy
        }
    ));

    let after = fixture.restore_request("idle-stop-after-close");
    let late_result = request(
        &mut retained_stop,
        StopRequest::RestoreRequiredStop {
            value: Box::new(after),
        },
    )
    .await;
    let admitted_after = fixture.state.admitted.lock().unwrap().len();
    let slots_after = fixture.state.slots.lock().unwrap().len();
    let refused = matches!(&late_result, Err(EnforcerError::Unhealthy));
    let reserved = matches!(&late_result, Ok(ReplyValue3::StopReserved { .. }));
    let spawn_failure_unused = fixture.state.faults.armed.load(Ordering::SeqCst);
    drop((retained_stop, retained_full, closing));
    fixture.finish().await;

    assert!(
        refused,
        "idle close must reject a new authenticated STOP reservation; StopReserved={reserved}, admitted={admitted_after}, slots={slots_after}"
    );
    assert_eq!(admitted_after, 0);
    assert_eq!(slots_after, 0);
    assert!(spawn_failure_unused);
}

#[tokio::test]
async fn real_ledger_failure_preserves_authenticated_new_stop_recovery() {
    use crate::broker::enforcer::ledger::LedgerCoverageV1;
    use std::io::Write;

    let mut fixture = IdleStopFixture::new();
    let full = fixture.state.full_owner().unwrap().unwrap();
    let (mut full_stream, full_session) = fixture.authenticated(RequestedRole::Full).await;

    // This is the real current pointer created by this fixture's fresh ledger.
    // Change its retained file length, then exercise coverage through the real
    // Full dispatcher. No failure flag is assigned by the test.
    let pointer_path = fixture.root.path().join("retirement/current.json");
    let original_length = std::fs::metadata(&pointer_path).unwrap().len();
    let mut pointer = std::fs::OpenOptions::new()
        .append(true)
        .open(&pointer_path)
        .unwrap();
    pointer.write_all(b"\n").unwrap();
    pointer.sync_all().unwrap();
    drop(pointer);
    assert_eq!(
        std::fs::metadata(&pointer_path).unwrap().len(),
        original_length + 1
    );
    let full_reply = request_full(
        &mut full_stream,
        &full_session,
        FullCommand::Status {},
        IO_TIMEOUT,
    )
    .await
    .unwrap();
    let full_refused = matches!(
        full_reply,
        FullReply::Error {
            code: EnforcerError::Unhealthy
        }
    );
    let ledger_partial = matches!(
        full.ledger.coverage().unwrap(),
        LedgerCoverageV1::Partial { .. }
    );
    let full_still_unhealthy = matches!(full.complete().await, Err(EnforcerError::Unhealthy));
    assert!(!fixture.state.shutdown_requested.load(Ordering::SeqCst));

    let value = fixture.restore_request("ledger-failure-new-stop");
    let arm_id = value.registration.arm.operation_id.clone();
    let mut stop = fixture.stop_connection().await;
    let reserved = request(
        &mut stop,
        StopRequest::RestoreRequiredStop {
            value: Box::new(value.clone()),
        },
    )
    .await
    .unwrap();
    let new_reserved = matches!(
        reserved,
        ReplyValue3::StopReserved {
            replayed: false,
            ..
        }
    );

    let actor = fixture.state.slots.lock().unwrap().get(&arm_id).cloned();
    let Some(actor) = actor else {
        drop((full_stream, stop));
        drop(full);
        finish_failed_ledger_fixture(fixture).await;
        panic!("a ledger failure must preserve new STOP reservation; reserved={new_reserved}");
    };
    assert!(!actor.worker_owned.load(Ordering::SeqCst));
    assert!(!actor.fail_spawn_once.load(Ordering::SeqCst));
    let first_snapshot = actor.snapshot().unwrap();
    assert_eq!(first_snapshot.phase, StopPhaseV1::Quarantined);
    assert_eq!(first_snapshot.stop_failures, 1);

    // Defer this fixture's retry deadline, as in the anchor transport fixture.
    // The additional pre-spawn guard catches any bypass without an OS thread.
    // This verifies replay while deferred, not the runtime retry schedule.
    *actor.next_start.lock().unwrap() = Instant::now() + Duration::from_secs(3600);
    actor.fail_spawn_once.store(true, Ordering::SeqCst);
    let mut replay = fixture.stop_connection().await;
    let replayed = request(
        &mut replay,
        StopRequest::RestoreRequiredStop {
            value: Box::new(value),
        },
    )
    .await
    .unwrap();
    let replay_reserved = matches!(replayed, ReplyValue3::StopReserved { replayed: true, .. });
    let no_replay_attempt = actor.fail_spawn_once.swap(false, Ordering::SeqCst);
    let final_snapshot = actor.snapshot().unwrap();
    assert!(!actor.worker_owned.load(Ordering::SeqCst));

    drop((full_stream, stop, replay));
    // Teardown owns this exact failed-before-spawn fixture actor. Removing it
    // produces no retirement/Q claim; the support joiner is joined separately.
    {
        let mut admitted = fixture.state.admitted.lock().unwrap();
        let mut slots = fixture.state.slots.lock().unwrap();
        assert!(Arc::ptr_eq(slots.get(&arm_id).unwrap(), &actor));
        assert_eq!(slots.len(), 1);
        assert_eq!(admitted.len(), 1);
        slots.remove(&arm_id);
        admitted.remove(&arm_id);
    }
    drop(actor);
    drop(full);
    finish_failed_ledger_fixture(fixture).await;

    assert!(full_refused);
    assert!(ledger_partial);
    assert!(full_still_unhealthy);
    assert!(new_reserved);
    assert!(replay_reserved);
    assert!(no_replay_attempt, "replay attempted another business spawn");
    assert_eq!(final_snapshot.stop_failures, 1);
}

async fn finish_failed_ledger_fixture(mut fixture: IdleStopFixture) {
    while let Some(result) = fixture.connections.join_next().await {
        result.unwrap().unwrap();
    }
    {
        let full = fixture.state.full_owner().unwrap().unwrap();
        full.finish_empty_fixture_support().await.unwrap();
    }
    if let Some(progress) = fixture.progress.take() {
        progress.abort();
        let _ = progress.await;
    }
    // The actual support join and every task finish precede releasing this
    // failed fixture's ledger/lease. Only then may TempDir remove its files.
    drop(std::mem::replace(&mut fixture.state, fixture_state()));
    fixture.finish().await;
}
