//! Store/HTTP/private-journal consumer regressions. Runtime facts are synthetic.
//! The post-terminal helper is real; its verified-job/service wrapper is not qualified here.
use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    broker::{clock::ClockResult, relay_owner::journal::ChannelJournal},
    control::*,
    model::{CommandSpec, DecisionRequest},
    policy::{self, Policy},
    redaction::Redactor,
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::Method,
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::atomic::{AtomicBool, AtomicUsize},
};
use tokio::sync::oneshot;

#[path = "../../../../../tests/support/relay_admission_fixture.rs"]
mod store_fixture;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
// Public fixture material only; no host credential or provider is read.
const RUNNER: &str = "history-consumer-runner-public-fixture-0000000000000000";
const VIEWER: &str = "history-consumer-viewer-public-fixture-0000000000000000";
const WORKLOAD: &str = "history-consumer-workload-public-fixture-000000000000";

#[derive(Default)]
struct Calls {
    total: AtomicUsize,
    claims: AtomicUsize,
    reserves: AtomicUsize,
    transitions: AtomicUsize,
    histories: AtomicUsize,
    lose_next_transition_reply: AtomicBool,
    refuse_histories: AtomicBool,
    lost_replies: AtomicUsize,
}
async fn count(State(calls): State<Arc<Calls>>, request: Request, next: Next) -> Response {
    calls.total.fetch_add(1, Ordering::SeqCst);
    let transition = request.method() == Method::POST
        && request
            .uri()
            .path()
            .starts_with("/control/api/v1/relay/channels/")
        && request.uri().path().ends_with("/transitions");
    let history = request.method() == Method::POST
        && request
            .uri()
            .path()
            .starts_with("/control/api/v1/relay/channels/")
        && request.uri().path().ends_with("/reconcile");
    if request.method() == Method::POST {
        let path = request.uri().path();
        if path.starts_with("/control/api/v1/relay/channels/") {
            if path.ends_with("/claims") {
                calls.claims.fetch_add(1, Ordering::SeqCst);
            }
            if path.ends_with("/admissions") {
                calls.reserves.fetch_add(1, Ordering::SeqCst);
            }
            if path.ends_with("/transitions") {
                calls.transitions.fetch_add(1, Ordering::SeqCst);
            }
            if path.ends_with("/reconcile") {
                calls.histories.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    if history && calls.refuse_histories.load(Ordering::SeqCst) {
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let response = next.run(request).await;
    if transition
        && response.status().is_success()
        && calls
            .lose_next_transition_reply
            .swap(false, Ordering::SeqCst)
    {
        // The real gateway has already committed. Lose delivery of this exact
        // response, then keep this pass unresolved until the test resumes it.
        calls.lost_replies.fetch_add(1, Ordering::SeqCst);
        calls.refuse_histories.store(true, Ordering::SeqCst);
        let (parts, _) = response.into_parts();
        return Response::from_parts(
            parts,
            Body::from_stream(futures_util::stream::once(async {
                Err::<Vec<u8>, std::io::Error>(std::io::Error::from(
                    std::io::ErrorKind::ConnectionReset,
                ))
            })),
        );
    }
    response
}

struct Server {
    origin: String,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}
impl Server {
    async fn open(app: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        });
        Self {
            origin,
            stop: Some(stop),
            task,
        }
    }
    async fn close(mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), &mut self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

struct DataClock(HostClockSampleV1);
impl HostClock for DataClock {
    fn sample(&self) -> ClockResult<HostClockSampleV1> {
        Ok(self.0.clone())
    }
}

pub(super) struct Fixture {
    _temporary: tempfile::TempDir,
    database: PathBuf,
    pub(super) business: Directory,
    pub(super) registry: Arc<RelayRegistry>,
    pub(super) entry: Arc<Entry>,
    pub(super) retained: Retained,
    store: Arc<Store>,
    pub(super) terminal_request: WorkerMutationV1,
    pub(super) terminal_result: MutationResultV1,
    old_close: RelayChannelTransitionV1,
    old_record: Vec<u8>,
    calls: Arc<Calls>,
    server: Option<Server>,
}

#[derive(Clone, Copy)]
enum LegacyFixture {
    ReservedClose,
    AcceptedReady,
    ReservedCancelled,
    ReservedRefused,
    NoAdmission,
}
impl Fixture {
    async fn reserved_with_old_close() -> Self {
        Self::legacy(LegacyFixture::ReservedClose).await
    }

    async fn accepted_with_old_ready() -> Self {
        Self::legacy(LegacyFixture::AcceptedReady).await
    }

    async fn accepted_with_unjoined_old_claim() -> Self {
        Self::legacy_with_join(LegacyFixture::AcceptedReady, false).await
    }

    async fn reserved_without_claim_or_join() -> Self {
        Self::legacy_with_join(LegacyFixture::ReservedClose, false).await
    }

    pub(super) async fn without_admission() -> Self {
        Self::legacy(LegacyFixture::NoAdmission).await
    }

    pub(super) async fn finalizable(
        state: RelayChannelStateV1,
    ) -> (Self, crate::broker::relay_owner::journal::ChannelHistoryV2) {
        let mode = match state {
            RelayChannelStateV1::Closed => LegacyFixture::AcceptedReady,
            RelayChannelStateV1::CancelledBeforeClaim => LegacyFixture::ReservedCancelled,
            RelayChannelStateV1::KnownRefusedBeforeSpawn => LegacyFixture::ReservedRefused,
            _ => panic!("finalizable fixture requires an explicit Core terminal"),
        };
        let f = Self::legacy(mode).await;
        let history = fencing_tests::prepare_finalizable(&f).await;
        assert_eq!(history.fact.core.as_ref().unwrap().state, state);
        (f, history)
    }

    pub(super) async fn close(&mut self) {
        self.retained.io.join_pending().await.unwrap();
        self.server.take().unwrap().close().await;
    }

    pub(super) fn records(&self) -> usize {
        self.business
            .persistent_child("relay_channels_v1", false)
            .unwrap()
            .unwrap()
            .persistent_entries(64)
            .unwrap()
            .iter()
            .filter(|entry| entry.name.starts_with("fact-"))
            .count()
    }

    pub(super) fn total_calls(&self) -> usize {
        self.calls.total.load(Ordering::SeqCst)
    }

    pub(super) fn status(&self) -> RelayStatusV1 {
        self.registry
            .status(&self.entry.source.scope, self.entry.source.job_id)
            .unwrap()
    }

    async fn legacy(mode: LegacyFixture) -> Self {
        Self::legacy_with_join(mode, true).await
    }

    async fn legacy_with_join(mode: LegacyFixture, local_joined: bool) -> Self {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        let store = Arc::new(store_fixture::fresh(temporary.path(), "tenant-a", 10000));
        let database = temporary.path().join("tenant-a.sqlite");
        let running = store_fixture::running_now(&store);
        let (credential, revision) = store_fixture::bind(&store, &running, WORKLOAD);
        let admission: RelayChannelAdmissionV1 = serde_json::from_value(store_fixture::request(
            &store,
            &running,
            &credential,
            revision,
            WORKLOAD,
        ))
        .unwrap();
        let reserved = if matches!(mode, LegacyFixture::NoAdmission) {
            None
        } else {
            let result = store.reserve_relay_channel(&admission).unwrap();
            assert_eq!(result.channel.state, RelayChannelStateV1::Reserved);
            assert_eq!(result.channel.revision, 1);
            Some(result.channel)
        };
        let (core, claim) = match mode {
            LegacyFixture::ReservedClose
            | LegacyFixture::ReservedCancelled
            | LegacyFixture::ReservedRefused => (reserved, None),
            LegacyFixture::NoAdmission => (None, None),
            LegacyFixture::AcceptedReady => {
                let claim = RelayChannelClaimV1 {
                    version: 1,
                    operation_id: Uuid::new_v4().to_string(),
                    channel_id: admission.channel_id.clone(),
                    epoch: admission.epoch,
                    expected_revision: 1,
                    admission_input_hash: admission.input_hash().unwrap(),
                };
                let accepted = store.claim_relay_channel_exec(&claim).unwrap();
                assert_eq!(
                    accepted.channel.state,
                    RelayChannelStateV1::AcceptedBeforeExec
                );
                assert_eq!(accepted.channel.revision, 2);
                (Some(accepted.channel), Some(claim))
            }
        };
        let mut observation = running.last_observation.clone().unwrap();
        observation.observed_at = chrono::Utc::now().to_rfc3339();
        observation.status = RuntimeStatusV1::Exited;
        observation.running = Some(false);
        observation.finished_at = Some(chrono::Utc::now().to_rfc3339());
        observation.exit_code = Some(0);
        let terminal_request =
            store_fixture::mutation(&running, WorkerTransitionV1::Observe { observation });
        let terminal_result = store.mutate_worker(&terminal_request).unwrap();
        assert_eq!(terminal_result.worker.state, WorkerStateV1::Terminal);
        assert!(!terminal_result.worker.quota_held);
        let old_close = RelayChannelTransitionV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            channel_id: admission.channel_id.clone(),
            epoch: admission.epoch,
            expected_revision: core.as_ref().map_or(1, |core| core.revision),
            admission_input_hash: admission.input_hash().unwrap(),
            transition: match mode {
                LegacyFixture::ReservedClose | LegacyFixture::NoAdmission => {
                    RelayChannelTransitionKindV1::CloseExactWorkerTerminal {
                        worker_revision: terminal_result.worker.revision,
                        terminal_observation_id: terminal_request.operation_id.clone(),
                    }
                }
                LegacyFixture::AcceptedReady => RelayChannelTransitionKindV1::Ready,
                LegacyFixture::ReservedCancelled => RelayChannelTransitionKindV1::CancelReserved,
                LegacyFixture::ReservedRefused => {
                    RelayChannelTransitionKindV1::KnownRefusedBeforeSpawn
                }
            },
        };
        old_close.validate().unwrap();
        if matches!(
            mode,
            LegacyFixture::ReservedCancelled | LegacyFixture::ReservedRefused
        ) {
            store.transition_relay_channel(&old_close).unwrap();
        }
        let calls = Arc::new(Calls::default());
        let gateway = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: "tenant-a".into(),
                store: store.clone(),
                runner_token: RUNNER.into(),
                viewer_token: VIEWER.into(),
            }],
            Policy::default(),
            None,
            Redactor::new(vec![RUNNER.into(), VIEWER.into(), WORKLOAD.into()]),
        )
        .unwrap();
        let server = Server::open(
            gateway_router(gateway).layer(middleware::from_fn_with_state(calls.clone(), count)),
        )
        .await;
        let scope = JournalScopeV1::new(
            Uuid::new_v4(),
            "tenant-a",
            &server.origin,
            Uuid::parse_str(&admission.control_namespace_uuid).unwrap(),
            "fixture",
        )
        .unwrap();
        let root = Directory::open_existing(temporary.path()).unwrap();
        let control = root.persistent_child("control", true).unwrap().unwrap();
        let role = Arc::new(RoleLease::acquire(&control).unwrap());
        let registry = RelayRegistry::new(
            control.clone(),
            role.clone(),
            BTreeMap::from([(
                "fixture".into(),
                vec![PathBuf::from("/synthetic/unused-operation")],
            )]),
            vec![RUNNER.into(), VIEWER.into(), WORKLOAD.into()],
        );
        let tenant = registry.tenant(&scope).unwrap();
        let business = control.persistent_child("business", true).unwrap().unwrap();
        let arm = worker_arm(&database, &running.worker_id);
        let job = Uuid::new_v4();
        let no_admission = matches!(mode, LegacyFixture::NoAdmission);
        let fact = ChannelFact {
            version: 1,
            job_id: job,
            source_profile_id: "fixture".into(),
            endpoint: admission.endpoint.clone(),
            source_head_revision: 1,
            source_head_hash: "2".repeat(64),
            arm_id: arm.operation_id.clone(),
            original_clock: arm.clock.clone(),
            original_deadline_ns_decimal: arm.deadline_ns_decimal.clone(),
            phase: if no_admission {
                AttachmentPhase::AwaitingIssuedWorkload
            } else {
                AttachmentPhase::Unknown
            },
            observed_at: chrono::Utc::now().to_rfc3339(),
            error: if no_admission {
                None
            } else {
                Some(RelayOwnerError::Unknown)
            },
            admission: if no_admission {
                None
            } else {
                Some(admission.clone())
            },
            claim,
            core,
            transition: if no_admission {
                None
            } else {
                Some(old_close.clone())
            },
            local_joined,
        };
        // Explicit legacy V1 bytes: future V2 writers must still read this
        // prefix and must not rewrite it to make the regression succeed.
        let literals = vec![RUNNER.into(), VIEWER.into(), WORKLOAD.into()];
        drop(ChannelJournal::open(&business, &tenant.scope, &literals).unwrap());
        let journal = business
            .persistent_child("relay_channels_v1", false)
            .unwrap()
            .unwrap();
        let old_record = crate::broker::journal::canonical(&json!({
            "version":1,"sequence":1,"previous_sha256":null,"fact":fact,
        }))
        .unwrap();
        assert!(old_record.len() <= 16_384);
        crate::relay::channel::control::validate_private_record(&old_record, &literals).unwrap();
        let file = journal
            .persistent_create("fact-00000000000000000001.json", &old_record)
            .unwrap();
        file.persistent_sync().unwrap();
        journal.sync().unwrap();
        let io = TenantRelayIo::open(
            &business,
            &tenant.scope,
            role,
            literals,
            tenant.bytes.clone(),
            tenant.slots.clone(),
            registry.global_bytes.clone(),
            registry.global_io.clone(),
        )
        .unwrap();
        *tenant.io.lock().unwrap() = Some(io.clone());
        let durable = io.channel(job).await.unwrap().unwrap();
        assert_eq!(
            crate::broker::journal::canonical(&durable).unwrap(),
            crate::broker::journal::canonical(&fact).unwrap(),
            "positive control: real IO must validate and reload the exact legacy prefix before the consumer oracle"
        );
        let cancel = CancelGeneration::new();
        let source = Arc::new(Source {
            scope: scope.clone(), job_id: job, head_revision: 1, head_hash: "2".repeat(64), worker: running.clone(),
            handle: serde_json::from_value(json!({"authority_fingerprint":admission.authority_binding_hash,
                "session":admission.session_id,"operation":job,"id":admission.inspection.container_id,
                "name":admission.inspection.stable_name,"configuration_fingerprint_version":2})).unwrap(),
            deadline: Arc::new(WorkerRelayDeadline { clock: Arc::new(DataClock(arm.clock.clone())),
                generation: cancel.current(), cancel, last_ns: Mutex::new(arm.clock.continuous_ns().unwrap()), arm }),
            execution: None, client: AuditClient::new(&server.origin, RUNNER).unwrap(),
            gateway_trust: crate::tls::ClientTrust::default(),
        });
        let (stop, _) = watch::channel(true);
        let entry = Arc::new(Entry {
            source: source.clone(),
            view: Mutex::new(Some(durable.clone())),
            error: Mutex::new(durable.error),
            stop,
            task: AsyncMutex::new(None),
            retained: AsyncMutex::new(None),
            // Mirrors the conservative production default until this exact
            // prefix was read and scoped; no read is inferred from construction.
            handoff_started: AtomicBool::new(true),
            no_channel: AtomicBool::new(false),
        });
        registry
            .entries
            .lock()
            .unwrap()
            .insert(("tenant-a".into(), job), entry.clone());
        Self {
            _temporary: temporary,
            database,
            business,
            registry,
            entry,
            retained: Retained {
                source,
                tenant,
                io,
                fact: durable,
                _authority: None,
                transport: None,
                owner: None,
                terminal_observed: false,
            },
            store,
            terminal_request,
            terminal_result,
            old_close,
            old_record,
            calls,
            server: Some(server),
        }
    }

    fn relay_events(&self) -> Vec<Value> {
        let db =
            Connection::open_with_flags(&self.database, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let mut statement = db
            .prepare("SELECT payload FROM events WHERE kind='relay_channel.v1' ORDER BY seq")
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
            .collect()
    }
}

#[path = "history_tests/fencing_tests.rs"]
mod fencing_tests;

pub(super) async fn assert_cancelled_real_history_proof_and_finalize() {
    fencing_tests::assert_cancelled_real_history_proof_and_finalize().await;
}

fn worker_arm(database: &Path, worker: &str) -> ArmV1 {
    let db = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut statement = db
        .prepare("SELECT payload_json FROM execution_arms WHERE worker_id=?1")
        .unwrap();
    let arms: Vec<ArmV1> = statement
        .query_map([worker], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
        .collect();
    let mut worker_arms = arms
        .into_iter()
        .filter(|arm| matches!(arm.subject, ArmSubjectV1::Worker));
    let result = worker_arms.next().unwrap();
    assert!(worker_arms.next().is_none());
    result
}

#[tokio::test]
async fn legacy_close_at_reserved_cas_retires_without_replaying_claim_or_losing_intent() {
    let mut f = Fixture::reserved_with_old_close().await;
    let request = RelayChannelReconcileV1 {
        version: 1,
        admission: f.retained.fact.admission.clone().unwrap(),
        pending: Some(RelayChannelPendingV1::Transition(f.old_close.clone())),
    };
    let positive = f
        .retained
        .source
        .client
        .reconcile_relay_channel(&request)
        .await
        .unwrap();
    assert!(
        matches!(
            positive.outcome,
            RelayChannelHistoryOutcomeV1::Historical {
                channel: RelayChannelSnapshotV1 {
                    state: RelayChannelStateV1::Reserved,
                    revision: 1,
                    ..
                },
                pending: Some(RelayChannelPendingHistoryV1 {
                    resolution: RelayChannelPendingResolutionV1::InapplicableState,
                    ..
                }),
                ..
            }
        ),
        "positive control: authenticated historical lookup must resolve old Close exactly at Reserved/CAS1"
    );
    assert_eq!(f.calls.histories.load(Ordering::SeqCst), 1);
    assert_eq!(f.calls.transitions.load(Ordering::SeqCst), 0);
    assert_eq!(f.calls.claims.load(Ordering::SeqCst), 0);
    let worker = f.terminal_result.worker.clone();
    let outcome = f
        .registry
        .retire_confirmed_terminal(
            &f.entry,
            &mut f.retained,
            &worker,
            &f.terminal_request,
            &f.terminal_result,
        )
        .await;
    f.retained.io.join_pending().await.unwrap();
    // Complete the owned loopback listener before the first discriminating
    // assertion. No cleanup mutation or retry is issued after a RED result.
    f.server.take().unwrap().close().await;
    assert_eq!(
        outcome,
        Ok(()),
        "R1 consumer still replays old Close(CAS1) instead of preserving it and fencing Reserved with CancelReserved"
    );

    let final_core = f.store.reconcile_relay_channel(&request).unwrap();
    assert!(matches!(
        final_core.outcome,
        RelayChannelHistoryOutcomeV1::Historical {
            channel: RelayChannelSnapshotV1 {
                state: RelayChannelStateV1::CancelledBeforeClaim,
                ..
            },
            ..
        }
    ));
    assert_eq!(f.calls.claims.load(Ordering::SeqCst), 0);
    assert_eq!(f.calls.reserves.load(Ordering::SeqCst), 0);
    assert_eq!(f.calls.transitions.load(Ordering::SeqCst), 1);
    let events = f.relay_events();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["data"]["input"]["action"] == "claim")
            .count(),
        0
    );
    assert_eq!(
        events
            .iter()
            .filter(
                |event| event["data"]["input"]["request"]["transition"]["kind"]
                    == "cancel_reserved"
            )
            .count(),
        1
    );
    let journal = f
        .business
        .persistent_child("relay_channels_v1", false)
        .unwrap()
        .unwrap();
    assert_eq!(
        journal
            .persistent_read("fact-00000000000000000001.json", 16_384, &[])
            .unwrap()
            .unwrap()
            .bytes(),
        f.old_record.as_slice(),
        "legacy intention bytes were rewritten"
    );
    assert_eq!(f.retained.fact.phase, AttachmentPhase::Closed);
    assert!(f.retained.fact.claim.is_none());
}
