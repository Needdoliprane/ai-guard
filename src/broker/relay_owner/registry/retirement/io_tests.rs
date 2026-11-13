//! Real private journal/IO, declared source metadata and a loopback refusal counter.
//! No AcceptedJob, fresh exec capability, engine, CLI or child is fabricated.
use super::*;
use crate::{
    broker::{clock::ClockResult, relay_owner::tests::arm},
    control::{HostClockSampleV1, WorkerAttemptV1},
};
use std::{os::unix::fs::PermissionsExt, sync::atomic::AtomicUsize};
use tokio::sync::oneshot;

const RUNNER: &str = "retirement-io-public-runner-fixture-0000000000000000";
const SENTINEL: &str = "retirement-io-private-record-fixture-0000000000000000";

struct DataClock(HostClockSampleV1);
impl HostClock for DataClock {
    fn sample(&self) -> ClockResult<HostClockSampleV1> {
        Ok(self.0.clone())
    }
}

struct RefusalCounter {
    origin: String,
    calls: Arc<AtomicUsize>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl RefusalCounter {
    async fn open() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        // Every path is refused. This fixture supplies no Core receipt/state;
        // its sole oracle is whether any request escaped a failed local save.
        let app = axum::Router::new().fallback(move || {
            let counted = counted.clone();
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
                axum::http::StatusCode::SERVICE_UNAVAILABLE
            }
        });
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self {
            origin,
            calls,
            stop: Some(stop),
            task: Some(task),
        }
    }
    async fn close(mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap();
    }
}
impl Drop for RefusalCounter {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

struct Fixture {
    _temporary: tempfile::TempDir,
    business: Directory,
    registry: Arc<RelayRegistry>,
    entry: Arc<Entry>,
    retained: Retained,
}
impl Fixture {
    async fn open(origin: &str, with_admission: bool) -> Self {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        let root = Directory::open_existing(temporary.path()).unwrap();
        let role = Arc::new(RoleLease::acquire(&root).unwrap());
        let registry = RelayRegistry::new(
            root.clone(),
            role.clone(),
            BTreeMap::from([(
                "fixture".into(),
                vec![PathBuf::from("/synthetic/unused-operation")],
            )]),
            vec![RUNNER.into(), SENTINEL.into()],
        );
        let scope = JournalScopeV1::new(
            Uuid::from_u128(28),
            "tenant-a",
            origin,
            Uuid::from_u128(22),
            "fixture",
        )
        .unwrap();
        let tenant = registry.tenant(&scope).unwrap();
        let business = root.persistent_child("business", true).unwrap().unwrap();
        let io = TenantRelayIo::open(
            &business,
            &tenant.scope,
            role,
            vec![RUNNER.into(), SENTINEL.into()],
            tenant.bytes.clone(),
            tenant.slots.clone(),
            registry.global_bytes.clone(),
            registry.global_io.clone(),
        )
        .unwrap();
        *tenant.io.lock().unwrap() = Some(io.clone());
        let arm = arm();
        let endpoint = RelayEndpointV1 {
            worker_id: arm.worker_id.clone(),
            attempt_id: arm.attempt_id.clone(),
            generation: 1,
        };
        let admission = RelayChannelAdmissionV1 {
            version: 1,
            operation_id: Uuid::from_u128(20).to_string(),
            channel_id: Uuid::from_u128(21).to_string(),
            epoch: 1,
            control_namespace_uuid: Uuid::from_u128(22).to_string(),
            endpoint: endpoint.clone(),
            credential_id: Uuid::from_u128(23).to_string(),
            binding_revision: 1,
            credential_sha256_digest: "1".repeat(64),
            session_id: Uuid::from_u128(24).to_string(),
            expected_worker_revision: 1,
            inspection: arm.inspection.clone(),
            started_at: "2026-11-08T00:00:00Z".into(),
            runtime_source: RelayRuntimeSourceV1::BrokerReportedRuntime,
            authority_binding_hash: arm.authority_binding_hash.clone(),
            configuration_fingerprint_version: arm.configuration_fingerprint_version,
            configuration_fingerprint: arm.configuration_fingerprint.clone(),
            expires_at: "2026-11-08T00:01:00Z".into(),
        };
        let core = RelayChannelSnapshotV1 {
            version: 1,
            tenant_id: "tenant-a".into(),
            control_namespace_uuid: admission.control_namespace_uuid.clone(),
            channel_id: admission.channel_id.clone(),
            epoch: 1,
            admission_operation_id: admission.operation_id.clone(),
            admission_input_hash: admission.input_hash().unwrap(),
            endpoint: endpoint.clone(),
            credential_id: admission.credential_id.clone(),
            binding_revision: 1,
            session_id: admission.session_id.clone(),
            expires_at: admission.expires_at.clone(),
            revision: 2,
            state: RelayChannelStateV1::AcceptedBeforeExec,
        };
        let mut fact = ChannelFact {
            version: 1,
            job_id: Uuid::from_u128(25),
            source_profile_id: "fixture".into(),
            endpoint,
            source_head_revision: 1,
            source_head_hash: "2".repeat(64),
            arm_id: arm.operation_id.clone(),
            original_clock: arm.clock.clone(),
            original_deadline_ns_decimal: arm.deadline_ns_decimal.clone(),
            phase: AttachmentPhase::Admitted,
            observed_at: "2026-11-08T00:00:00Z".into(),
            error: None,
            admission: Some(admission.clone()),
            claim: Some(RelayChannelClaimV1 {
                version: 1,
                operation_id: Uuid::from_u128(30).to_string(),
                channel_id: admission.channel_id.clone(),
                epoch: 1,
                expected_revision: 1,
                admission_input_hash: admission.input_hash().unwrap(),
            }),
            core: Some(core),
            transition: None,
            local_joined: true,
        };
        if !with_admission {
            fact.phase = AttachmentPhase::AwaitingIssuedWorkload;
            fact.admission = None;
            fact.claim = None;
            fact.core = None;
        }
        // Existing complete DTO fixture, used as metadata only. No verified
        // broker job, runtime authority or fresh claim is built from it.
        let managed: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../tests/fixtures/broker_intent-managed-checkpoint.json"
        ))
        .unwrap();
        let mut worker = WorkerV1 {
            worker_id: arm.worker_id.clone(),
            admission_operation_id: admission.operation_id.clone(),
            request_id: managed["admission"]["receipt"]["request_id"]
                .as_str()
                .unwrap()
                .into(),
            receipt: serde_json::from_value(managed["admission"]["receipt"].clone()).unwrap(),
            launch: serde_json::from_value(managed["admission"]["launch"].clone()).unwrap(),
            spec_hash: arm.inspection.spec_hash.clone(),
            policy_hash: "3".repeat(64),
            state: WorkerStateV1::Running,
            quota_held: true,
            revision: 1,
            attempt: WorkerAttemptV1 {
                attempt_id: arm.attempt_id.clone(),
                stable_name: arm.inspection.stable_name.clone(),
                volume_name: arm.inspection.volume_name.clone(),
                container_id: Some(arm.inspection.container_id.clone()),
                created_inspection: Some(arm.inspection.clone()),
                create_operation_id: None,
                start_operation_id: None,
                start_emitter_id: None,
            },
            observed_started_at: Some(admission.started_at.clone()),
            allocation_inconsistent: false,
            last_observation: None,
            unknown_reason: None,
            cleanup_removed: None,
            cleanup_error: None,
        };
        worker.launch.session_id = admission.session_id.clone();
        let cancel = CancelGeneration::new();
        let source = Arc::new(Source {
            scope: scope.clone(),
            job_id: fact.job_id,
            head_revision: 1,
            head_hash: "2".repeat(64),
            worker,
            handle: serde_json::from_value(serde_json::json!({
                "authority_fingerprint":arm.authority_binding_hash,
                "session":admission.session_id,"operation":fact.job_id,
                "id":arm.inspection.container_id,"name":arm.inspection.stable_name,
                "configuration_fingerprint_version":2
            }))
            .unwrap(),
            deadline: Arc::new(WorkerRelayDeadline {
                clock: Arc::new(DataClock(arm.clock.clone())),
                generation: cancel.current(),
                cancel,
                last_ns: Mutex::new(arm.clock.continuous_ns().unwrap()),
                arm,
            }),
            execution: None,
            client: AuditClient::new(origin, RUNNER).unwrap(),
            gateway_trust: crate::tls::ClientTrust::default(),
        });
        let (stop, _) = watch::channel(false);
        let entry = Arc::new(Entry {
            source: source.clone(),
            view: Mutex::new(None),
            error: Mutex::new(None),
            stop,
            task: AsyncMutex::new(None),
            retained: AsyncMutex::new(None),
            handoff_started: AtomicBool::new(false),
            no_channel: AtomicBool::new(false),
        });
        registry
            .entries
            .lock()
            .unwrap()
            .insert(("tenant-a".into(), fact.job_id), entry.clone());
        registry
            .save(&entry, &tenant, &io, &mut fact)
            .await
            .unwrap();
        Self {
            _temporary: temporary,
            business,
            registry,
            entry,
            retained: Retained {
                source,
                tenant,
                io,
                fact,
                _authority: None,
                transport: None,
                owner: None,
                terminal_observed: false,
            },
        }
    }
    fn records(&self) -> usize {
        self.business
            .persistent_child("relay_channels_v1", false)
            .unwrap()
            .unwrap()
            .persistent_entries(32)
            .unwrap()
            .iter()
            .filter(|entry| entry.name.starts_with("fact-"))
            .count()
    }
    fn status(&self) -> RelayStatusV1 {
        self.registry
            .status(&self.entry.source.scope, self.entry.source.job_id)
            .unwrap()
    }
}

fn close() -> RelayChannelTransitionKindV1 {
    RelayChannelTransitionKindV1::CloseExactWorkerTerminal {
        worker_revision: 10,
        terminal_observation_id: Uuid::from_u128(27).to_string(),
    }
}

async fn hold_all_slots(slots: Arc<Semaphore>) -> tokio::sync::OwnedSemaphorePermit {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(held) = slots.clone().try_acquire_many_owned(4) {
                break held;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn failed_intent_io_admission_blocks_core_and_no_admission_close_publication() {
    let counter = RefusalCounter::open().await;
    let mut f = Fixture::open(&counter.origin, true).await;
    let before = crate::broker::journal::canonical(&f.retained.fact).unwrap();
    let held = hold_all_slots(f.retained.tenant.slots.clone()).await;
    assert_eq!(
        f.registry
            .transition(
                &f.entry,
                &f.retained.tenant,
                &f.retained.io,
                &f.retained.source,
                &mut f.retained.fact,
                close(),
            )
            .await,
        Err(RelayOwnerError::Unknown)
    );
    assert_eq!(
        crate::broker::journal::canonical(&f.retained.fact).unwrap(),
        before
    );
    assert_eq!(f.status().phase, Some(RelayAttachmentPhaseV1::Unknown));
    assert!(f.status().reconciliation_required);
    assert_eq!(counter.calls.load(Ordering::SeqCst), 0);
    drop(held);
    assert_eq!(
        crate::broker::journal::canonical(
            &f.retained
                .io
                .channel(f.entry.source.job_id)
                .await
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        before
    );
    assert_eq!(f.records(), 1);

    let mut unjoined = f.retained.fact.clone();
    unjoined.local_joined = false;
    let request = RelayChannelTransitionV1 {
        version: 1,
        operation_id: Uuid::from_u128(31).to_string(),
        channel_id: unjoined.admission.as_ref().unwrap().channel_id.clone(),
        epoch: 1,
        expected_revision: 2,
        admission_input_hash: unjoined.admission.as_ref().unwrap().input_hash().unwrap(),
        transition: close(),
    };
    let mut terminal = unjoined.core.clone().unwrap();
    terminal.state = RelayChannelStateV1::Closed;
    terminal.revision = 3;
    assert!(
        super::super::attachment::acknowledged_transition(&unjoined, &request, terminal).is_err()
    );

    let mut local = super::history_tests::Fixture::without_admission().await;
    assert_eq!(
        local.total_calls(),
        0,
        "local fixture setup made an unexpected HTTP request"
    );
    let initial = crate::broker::journal::canonical(&local.retained.fact).unwrap();
    let (started, mut ready) = oneshot::channel();
    let (release, finish) = std::sync::mpsc::channel();
    local.retained.io.pause_next(started, finish);
    let slots = local.retained.tenant.slots.clone();
    let worker = local.terminal_result.worker.clone();
    let mut retirement = Box::pin(local.registry.retire_confirmed_terminal(
        &local.entry,
        &mut local.retained,
        &worker,
        &local.terminal_request,
        &local.terminal_result,
    ));
    tokio::select! {
        result = &mut retirement => panic!("journal read unexpectedly completed before its barrier: {result:?}"),
        result = &mut ready => result.unwrap(),
    }
    release.send(()).unwrap();
    // Observe all permits returned by the completed real read before allowing
    // the observer to continue to its next required V2 commit. Capacity refusal
    // here is not a filesystem fault and cannot authorize local Finalize.
    let held = hold_all_slots(slots).await;
    assert_eq!(retirement.await, Err(RelayOwnerError::Unknown));
    assert_ne!(local.retained.fact.phase, AttachmentPhase::Closed);
    assert_eq!(local.status().phase, Some(RelayAttachmentPhaseV1::Unknown));
    assert!(local.status().reconciliation_required);
    drop(held);
    assert_eq!(
        crate::broker::journal::canonical(
            &local
                .retained
                .io
                .channel(local.entry.source.job_id)
                .await
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        initial
    );
    assert_eq!(local.records(), 1);
    assert_eq!(
        local.total_calls(),
        0,
        "the local capacity refusal sent a request, including identity lookup"
    );
    assert_eq!(counter.calls.load(Ordering::SeqCst), 0);
    f.retained.io.join_pending().await.unwrap();
    local.close().await;
    assert_eq!(local.total_calls(), 0);
    counter.close().await;
}

#[tokio::test]
async fn cancelled_saves_recover_exact_intent_and_publish_only_durable_terminal() {
    // The old synthetic Closed Fact is no longer a valid V2 precondition.
    // Keep the original publication/identity/idempotence oracles with a real
    // Store receipt, full Begin/history and interrupted proof/Finalize writes.
    // The separate cancelled-cleanup parent retains the intent-cut assertions.
    super::history_tests::assert_cancelled_real_history_proof_and_finalize().await;
}
