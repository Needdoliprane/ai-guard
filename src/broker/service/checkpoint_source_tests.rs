//! Real service assembly, journal heads and authenticated gateway receipts.
//! No enforcer, runtime, provider or broker command is started by this fixture.
use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    broker::{
        intent::ExecutionIntentV1,
        journal::{BindingInputs, RevisionRefV1},
        presence::ServiceDiskBarrier,
    },
    broker_registry::{
        BrokerAuthorityV1, BrokerCheckpointCoverageV1, BrokerCheckpointV1, BrokerClaimProvenanceV1,
        BrokerGrantMutationV1, BrokerReceptionProvenanceV1, BrokerRuntimeKindV1,
    },
    policy::Policy,
    runner::CheckpointPayloadV1,
    workload::{AuthenticatedWorkload, WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, Response},
    middleware::{self, Next},
};
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::Mutex,
    task::Poll,
};
use tokio::sync::Notify;

const TENANT: &str = "federation-service-tenant";
const RUNNER: &str = "federation-service-synthetic-runner-0000000000000000";
const VIEWER: &str = "federation-service-synthetic-viewer-0000000000000000";
const PROFILE: &str = "profile-0";

#[derive(Clone)]
struct SeenCheckpoint {
    profile: String,
    checkpoint: BrokerCheckpointV1,
}
struct ProfileProbe {
    profile: String,
    state: PathBuf,
}
struct Probe {
    profiles: BTreeMap<String, ProfileProbe>,
    checkpoints: Mutex<Vec<SeenCheckpoint>>,
    heartbeats: Mutex<BTreeMap<String, usize>>,
    paths: Mutex<Vec<String>>,
    hold_first_ack: AtomicBool,
    committed: Notify,
    release_ack: Notify,
}

fn records(root: &Path, leaf: &str) -> Vec<Value> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path().join(leaf))
        .filter(|path| path.is_file())
        .map(|path| serde_json::from_slice(&fs::read(path).unwrap()).unwrap())
        .collect()
}

fn record_count(root: &Path, leaf: &str) -> usize {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path().join(leaf))
        .filter(|path| path.is_file())
        .count()
}

async fn observe(probe: Arc<Probe>, request: Request<Body>, next: Next) -> Response<Body> {
    let path = request.uri().path().to_owned();
    probe.paths.lock().unwrap().push(path.clone());
    let profile = request
        .headers()
        .get("authorization")
        .and_then(|header| header.to_str().ok())
        .and_then(|header| header.strip_prefix("Bearer "))
        .and_then(|token| probe.profiles.get(token));
    if path == "/control/api/v1/brokers/heartbeat" {
        let profile = profile
            .expect("configured synthetic Broker proof")
            .profile
            .clone();
        let response = next.run(request).await;
        if response.status() == 200 {
            *probe.heartbeats.lock().unwrap().entry(profile).or_default() += 1;
        }
        return response;
    }
    if path != "/control/api/v1/brokers/checkpoints" {
        return next.run(request).await;
    }
    let profile = profile.expect("configured synthetic Broker proof");
    let name = profile.profile.clone();
    let state = profile.state.clone();
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 32768).await.unwrap();
    let checkpoint: BrokerCheckpointV1 = serde_json::from_slice(&bytes).unwrap();
    probe.checkpoints.lock().unwrap().push(SeenCheckpoint {
        profile: name,
        checkpoint: checkpoint.clone(),
    });
    assert!(
        records(&state, "pending.json")
            .iter()
            .any(|record| { record["checkpoint"] == serde_json::to_value(&checkpoint).unwrap() }),
        "every real POST must already have its exact durable Pending"
    );
    let response = next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await;
    if response.status() == 200 && probe.hold_first_ack.swap(false, Ordering::SeqCst) {
        // The actual Store has committed; only its HTTP reply is faulted.
        probe.committed.notify_one();
        return Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(Body::from_stream(futures_util::stream::once(async move {
                probe.release_ack.notified().await;
                Err::<Vec<u8>, _>(std::io::Error::other("synthetic lost committed ACK"))
            })))
            .unwrap();
    }
    response
}

fn private_dir(path: &Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
fn private_file(path: &Path, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

struct ProfileFixture {
    token: String,
    operation: Uuid,
    journal: Option<Journal>,
    payload: Option<CheckpointPayloadV1>,
    head: Option<RevisionRefV1>,
    checkpoint_state: PathBuf,
}
struct Fixture {
    service: Arc<Service>,
    prepared: Vec<(String, PreparedPresence)>,
    profiles: BTreeMap<String, ProfileFixture>,
    store: Arc<Store>,
    probe: Arc<Probe>,
    gateway: tokio::task::JoinHandle<()>,
    _temporary: tempfile::TempDir,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.probe.release_ack.notify_waiters();
        self.gateway.abort();
    }
}

impl Fixture {
    // The three-profile case contains one real Full scope, one cold scope,
    // and a Full writer whose real immutable tenant differs from Registry2.
    async fn new(version: u32, three_profiles: bool) -> Self {
        let temporary = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let controls = root.join("controls");
        let broker_root = root.join("broker");
        let project = root.join("project");
        let exports = root.join("exports");
        for path in [&controls, &broker_root, &project, &exports] {
            private_dir(path);
        }
        private_file(&controls.join("runner"), RUNNER.as_bytes());
        private_file(
            &project.join("readme.txt"),
            b"synthetic source, never executed",
        );
        let db = root.join("tenant.sqlite");
        let store =
            Arc::new(Store::open_for_tenant(&db, TENANT, Redactor::new(vec![]), 100_000).unwrap());
        store
            .migrate_control_v3(&db.with_extension("before3"))
            .unwrap();
        store
            .migrate_identity_v4(&db.with_extension("before4"))
            .unwrap();
        store
            .migrate_human_access_v5(&db.with_extension("before5"))
            .unwrap();
        store
            .migrate_workload_v6(&db.with_extension("before6"))
            .unwrap();
        store
            .migrate_human_brokers_v7(&db.with_extension("before7"))
            .unwrap();
        store
            .migrate_kubernetes_create_v8(&db.with_extension("before8"))
            .unwrap();
        store
            .migrate_kubernetes_policy_v9(&db.with_extension("before9"))
            .unwrap();
        store
            .migrate_human_workers_v10(&db.with_extension("before10"))
            .unwrap();
        store
            .migrate_kubernetes_session_reservations_v11(&db.with_extension("before11"))
            .unwrap();
        store
            .migrate_broker_checkpoints_v12(&db.with_extension("before12"))
            .unwrap();
        let namespace: Uuid = store
            .supervision_state()
            .unwrap()
            .control_namespace_uuid
            .unwrap()
            .parse()
            .unwrap();
        let installation = Uuid::new_v4();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway = format!("http://{}", listener.local_addr().unwrap());
        let mut entries = Vec::new();
        let mut probe_profiles = BTreeMap::new();
        let mut profiles = BTreeMap::new();
        let count = if three_profiles { 3 } else { 1 };
        for index in 0..count {
            let profile = format!("profile-{index}");
            let token = format!("federation-service-synthetic-broker-{index}-0000000000000000");
            let broker_id = Uuid::new_v4();
            let credential_id = Uuid::new_v4();
            let incarnation_id = Uuid::new_v4();
            let state = controls.join(format!("presence-state-{index}"));
            let checkpoint_state = controls.join(format!("checkpoint-state-{index}"));
            private_dir(&state);
            private_dir(&checkpoint_state);
            let token_path = controls.join(format!("broker-token-{index}"));
            private_file(&token_path, token.as_bytes());
            let presence_path = controls.join(format!("presence-{index}.json"));
            let mut presence = json!({
                "version":version,"broker_id":broker_id,"credential_id":credential_id,
                "incarnation_id":incarnation_id,"authority_id":format!("authority-{index}"),
                "broker_token_file":token_path,"state_dir":state,"interval_seconds":5,
                "max_operations":64,"max_state_bytes":1048576
            });
            if version == 2 {
                presence["checkpoint_state_dir"] = json!(&checkpoint_state);
                presence["checkpoint_max_operations"] = json!(64);
                presence["checkpoint_max_state_bytes"] = json!(1048576);
            }
            private_file(&presence_path, &serde_json::to_vec(&presence).unwrap());
            store
                .issue_workload_binding(&WorkloadBindingIssueV1 {
                    version: 1,
                    operation_id: Uuid::new_v4().to_string(),
                    credential_id: credential_id.to_string(),
                    expected_revision: 0,
                    subject: WorkloadSubjectV1::Broker {
                        broker_id: broker_id.to_string(),
                        incarnation_id: incarnation_id.to_string(),
                    },
                    sha256_digest: hex::encode(Sha256::digest(token.as_bytes())),
                    expires_at: "2100-01-01T00:00:00.000Z".into(),
                })
                .unwrap();
            store
                .mutate_broker_grant(&BrokerGrantMutationV1 {
                    version: 1,
                    operation_id: Uuid::new_v4().to_string(),
                    broker_id: broker_id.to_string(),
                    expected_revision: None,
                    credential_id: credential_id.to_string(),
                    incarnation_id: incarnation_id.to_string(),
                    authorities: vec![BrokerAuthorityV1 {
                        authority_id: format!("authority-{index}"),
                        runtime_kind: BrokerRuntimeKindV1::Docker,
                    }],
                    lease_seconds: 120,
                    not_after: "2100-01-01T00:00:00.000Z".into(),
                })
                .unwrap();
            entries.push(json!({
                "id":profile,"gateway":gateway,"tenant_id":TENANT,
                "control_namespace_uuid":namespace,"config_dir":controls,
                "runner_token_file":controls.join("runner"),"runtime":"docker",
                "runtime_program":"/usr/bin/false","runtime_endpoint":"unix:///unused.sock",
                "image":"fixture:unused","policy_profile":"strict","network":"none",
                "allow_rootful_runtime":false,"max_timeout_seconds":30,"allow_capture_output":false,
                "managed_clients":false,"providers_file":null,"manual_credential_file":null,
                "source_roots":[project],"export_roots":[exports],
                "prearmed_start_profile":{"version":1,"mode":"legacy"},"presence_file":presence_path
            }));
            probe_profiles.insert(
                token.clone(),
                ProfileProbe {
                    profile: profile.clone(),
                    state: checkpoint_state.clone(),
                },
            );
            let operation = Uuid::new_v4();
            let (journal, payload, head) = if index == 1 || version == 1 {
                (None, None, None)
            } else {
                let scope = JournalScopeV1::new(
                    installation,
                    if index == 2 {
                        "foreign-journal-tenant"
                    } else {
                        TENANT
                    },
                    &gateway,
                    namespace,
                    &profile,
                )
                .unwrap();
                let directory = Directory::open_existing(&broker_root).unwrap();
                let journal = Journal::open(
                    &directory,
                    scope,
                    &FsExclusions::new(),
                    JournalLimitsV1::default(),
                    ControlSecrets::new(Vec::new()).unwrap(),
                )
                .unwrap();
                let mut plan: Value = serde_json::from_str(include_str!(
                    "../../../tests/fixtures/broker_intent-plan-v3.json"
                ))
                .unwrap();
                plan["operation_id"] = json!(operation);
                plan["version"] = json!(1);
                plan["options"]["gateway"] = json!(&gateway);
                plan["options"]["profile"] = json!("strict");
                plan["options"]["network"] = json!("none");
                plan["options"]["provider"] = json!("generic");
                plan["options"]["argv"] = json!(["/usr/bin/id"]);
                for field in [
                    "client",
                    "auth_mode",
                    "client_profile",
                    "providers_file",
                    "prompt",
                    "model",
                ] {
                    plan["options"].as_object_mut().unwrap().remove(field);
                }
                plan["request"]["profile"] = json!("strict");
                plan["request"]["agent_id"] = json!("generic-22222222-2222-4222-8222-222222222222");
                plan["request"]["command"]["argv"] = json!(["/usr/bin/id"]);
                plan["request"]
                    .as_object_mut()
                    .unwrap()
                    .remove("provider_resolution");
                plan["provider_input"] = Value::Null;
                plan["provider_credential"] = Value::Null;
                let intent =
                    ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap())
                        .unwrap();
                let payload = CheckpointPayloadV1::initial(&intent).unwrap();
                let ReserveResult::New(ticket) = journal
                    .reserve(operation, b"synthetic federation head")
                    .unwrap()
                else {
                    panic!("fresh real journal reservation");
                };
                let accepted = journal
                    .accept(ticket, &intent, BindingInputs::None, &payload)
                    .unwrap();
                assert_eq!(accepted.operation_id(), operation);
                let head = accepted.head().clone();
                (Some(journal), Some(payload), Some(head))
            };
            profiles.insert(
                profile,
                ProfileFixture {
                    token,
                    operation,
                    journal,
                    payload,
                    head,
                    checkpoint_state,
                },
            );
        }
        let registry_path = controls.join("registry.json");
        private_file(
            &registry_path,
            &serde_json::to_vec(&json!({
                "version":2,"installation_id":installation,"root":broker_root,
                "max_enforcer_slots":8,"profiles":entries,"additional_control_secret_files":[]
            }))
            .unwrap(),
        );
        let registry = Arc::new(Registry::load(&registry_path).unwrap());
        let (material, prepared) = prepare_presence(Arc::clone(&registry)).await.unwrap();
        assert_eq!(prepared.len(), count);
        let jobs: BTreeMap<_, _> = profiles
            .keys()
            .map(|profile| (profile.clone(), Arc::new(ProfileJobs::new())))
            .collect();
        for (name, profile) in &profiles {
            if let Some(journal) = &profile.journal {
                let writer =
                    WriterClient::start(journal.clone(), 8, Duration::from_secs(2)).unwrap();
                let page = writer.list(None, 100).await.unwrap();
                assert_eq!(page.entries().len(), 1);
                assert_eq!(page.entries()[0].head(), profile.head.as_ref());
                assert!(
                    jobs[name]
                        .writer
                        .set(ProfileWriterCapabilities::from_full(writer))
                        .is_ok()
                );
            }
        }
        let service = Arc::new(Service {
            registry,
            root: Directory::open_existing(&broker_root).unwrap(),
            material: Arc::new(material),
            profiles: jobs,
            enforcer: None,
            stop_enforcer: None,
            blocking: Arc::new(Semaphore::new(8)),
            preparation: Arc::new(Semaphore::new(4)),
            execution: Arc::new(Semaphore::new(8)),
            cold_owners: retained_audit::ColdOwners::new(8).unwrap(),
            full_owners: FullOwners::new(8).unwrap(),
            stopping: Arc::new(AtomicBool::new(false)),
            fenced: AtomicBool::new(false),
        });
        let probe = Arc::new(Probe {
            profiles: probe_profiles,
            checkpoints: Mutex::new(Vec::new()),
            heartbeats: Mutex::new(BTreeMap::new()),
            paths: Mutex::new(Vec::new()),
            hold_first_ack: AtomicBool::new(false),
            committed: Notify::new(),
            release_ack: Notify::new(),
        });
        let state = GatewayState::for_tenants(
            vec![TenantGateway {
                tenant_id: TENANT.into(),
                store: Arc::clone(&store),
                runner_token: RUNNER.into(),
                viewer_token: VIEWER.into(),
            }],
            Policy::default(),
            None,
            Redactor::new(profiles.values().map(|p| p.token.clone()).collect()),
        )
        .unwrap();
        let observed = Arc::clone(&probe);
        let app = gateway_router(state).layer(middleware::from_fn(move |request, next| {
            observe(Arc::clone(&observed), request, next)
        }));
        let gateway = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            service,
            prepared,
            profiles,
            store,
            probe,
            gateway,
            _temporary: temporary,
        }
    }

    fn start(&mut self, stop: &watch::Sender<bool>) -> JoinSet<()> {
        self.service
            .spawn_presence(std::mem::take(&mut self.prepared), stop)
    }

    async fn progress(&self, tasks: &mut JoinSet<()>, condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if condition() { return; }
                tokio::select! {
                    result = tasks.join_next(), if !tasks.is_empty() => {
                        panic!("service presence assembly exited before real publication: {result:?}");
                    },
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {},
                }
            }
        }).await.expect("condition-bound real service publication watchdog");
    }

    async fn finish(&self, stop: &watch::Sender<bool>, tasks: &mut JoinSet<()>) {
        let reconciler = tokio::spawn(std::future::pending());
        let mut requests = JoinSet::new();
        tokio::time::timeout(
            Duration::from_secs(20),
            self.service
                .shutdown(reconciler, stop, tasks, &mut requests),
        )
        .await
        .unwrap()
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_checkpoint_uses_real_full_scope_and_durable_head_without_inventing_empty_source() {
    let mut fixture = Fixture::new(2, true).await;
    let (stop, _) = watch::channel(false);
    let mut tasks = fixture.start(&stop);
    fixture
        .progress(&mut tasks, || {
            fixture
                .probe
                .checkpoints
                .lock()
                .unwrap()
                .iter()
                .any(|seen| seen.profile == PROFILE)
                && record_count(&fixture.profiles[PROFILE].checkpoint_state, "receipt.json") >= 1
                && fixture
                    .probe
                    .heartbeats
                    .lock()
                    .unwrap()
                    .values()
                    .filter(|count| **count >= 2)
                    .count()
                    == 3
        })
        .await;
    fixture.finish(&stop, &mut tasks).await;
    let requests = fixture.probe.checkpoints.lock().unwrap();
    assert!(!requests.is_empty());
    let expected = &fixture.profiles[PROFILE];
    for seen in requests.iter() {
        assert_eq!(
            seen.profile, PROFILE,
            "cold or cross-tenant Full must stay unavailable"
        );
        assert_eq!(
            seen.checkpoint.snapshot.jobs.len(),
            1,
            "no synthetic empty snapshot"
        );
        let job = &seen.checkpoint.snapshot.jobs[0];
        assert_eq!(job.job_id, expected.operation.to_string());
        assert_eq!(job.revision, expected.head.as_ref().unwrap().revision());
        assert_eq!(job.checkpoint_hash, expected.head.as_ref().unwrap().hash());
    }
    drop(requests);
    let AuthenticatedWorkload::Broker(identity) = fixture
        .store
        .authenticate_workload_token(&expected.token)
        .unwrap()
        .unwrap()
    else {
        panic!("real Broker proof");
    };
    let observation = fixture
        .store
        .broker_checkpoint_for_identity(&identity)
        .unwrap()
        .checkpoint
        .unwrap();
    assert_eq!(
        observation.coverage,
        BrokerCheckpointCoverageV1::DeclaredPartial
    );
    assert_eq!(
        observation.publication_provenance,
        BrokerClaimProvenanceV1::Declared
    );
    assert_eq!(
        observation.reception_provenance,
        BrokerReceptionProvenanceV1::Observed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_checkpoint_keeps_pending_one_until_lost_ack_replays_before_real_head_two() {
    let mut fixture = Fixture::new(2, false).await;
    fixture.probe.hold_first_ack.store(true, Ordering::SeqCst);
    let (stop, _) = watch::channel(false);
    let mut tasks = fixture.start(&stop);
    fixture
        .progress(&mut tasks, || {
            !fixture.probe.checkpoints.lock().unwrap().is_empty()
        })
        .await;
    tokio::time::timeout(Duration::from_secs(10), fixture.probe.committed.notified())
        .await
        .unwrap();
    let profile = &fixture.profiles[PROFILE];
    let first = fixture.probe.checkpoints.lock().unwrap()[0]
        .checkpoint
        .clone();
    let second = profile
        .journal
        .as_ref()
        .unwrap()
        .advance(
            profile.operation,
            profile.head.as_ref().unwrap().clone(),
            profile.payload.as_ref().unwrap(),
        )
        .unwrap();
    assert_eq!(
        second.head().revision(),
        first.snapshot.jobs[0].revision + 1
    );
    let full = fixture.service.profiles[PROFILE]
        .writer
        .get()
        .unwrap()
        .full()
        .unwrap();
    assert_eq!(
        full.list(None, 100).await.unwrap().entries()[0].head(),
        Some(second.head())
    );
    assert!(
        records(&profile.checkpoint_state, "pending.json")
            .iter()
            .any(|record| { record["checkpoint"] == serde_json::to_value(&first).unwrap() })
    );
    fixture.probe.release_ack.notify_one();
    fixture
        .progress(&mut tasks, || {
            record_count(&profile.checkpoint_state, "receipt.json") >= 2
        })
        .await;
    fixture.finish(&stop, &mut tasks).await;
    let requests = fixture.probe.checkpoints.lock().unwrap();
    assert!(
        requests.len() >= 3,
        "real replay and the next committed head must both be observed"
    );
    assert_eq!(
        requests[0].checkpoint, requests[1].checkpoint,
        "lost ACK preserves exact Pending1"
    );
    let next = &requests[2].checkpoint;
    assert_ne!(next.operation_id, first.operation_id);
    assert_eq!((next.previous_sequence, next.sequence), (1, 2));
    assert_eq!(next.snapshot.jobs[0].job_id, profile.operation.to_string());
    assert_eq!(next.snapshot.jobs[0].revision, second.head().revision());
    assert_eq!(next.snapshot.jobs[0].checkpoint_hash, second.head().hash());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_checkpoint_shutdown_retains_real_fs_and_cancelled_observer_until_rejoined() {
    // V1 reaches the real FS on the pre-RED source too; this oracle isolates
    // the service drain from the separate missing V2 source assembly.
    let mut fixture = Fixture::new(1, false).await;
    let barrier = ServiceDiskBarrier::new();
    barrier.attach(&mut fixture.prepared[0].1);
    let (stop, _) = watch::channel(false);
    let mut tasks = fixture.start(&stop);
    barrier.wait().await;
    assert!(fixture.probe.paths.lock().unwrap().is_empty());
    let mut requests = JoinSet::new();
    // Keep this same real handle, but make its await ready before shutdown:
    // its first Pending must then come from the actual presence drain.
    let reconciler = tokio::spawn(async {});
    tokio::time::timeout(Duration::from_secs(10), async {
        while !reconciler.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("reconciler must be terminal without consuming its handle");
    let mut shutdown = Box::pin(fixture.service.shutdown(
        reconciler,
        &stop,
        &mut tasks,
        &mut requests,
    ));
    let mut first = Box::pin(async { shutdown.as_mut().await });
    assert!(
        matches!(futures_util::poll!(&mut first), Poll::Pending),
        "arm the real shutdown before observing time"
    );
    drop(first); // Cancel only a borrowed observer of the same pinned shutdown.
    let mut resumed = Box::pin(async { shutdown.as_mut().await });
    assert!(
        matches!(futures_util::poll!(&mut resumed), Poll::Pending),
        "the resumed observer is actually polled"
    );
    let premature = tokio::time::timeout(Duration::from_secs(16), &mut resumed).await;
    let returned_early = premature.is_ok();
    // This borrowed observer is gone; the one underlying shutdown/reconciler
    // and both JoinSets have not been replaced or restarted.
    drop(resumed);
    barrier.release();
    if !returned_early {
        tokio::time::timeout(Duration::from_secs(10), shutdown.as_mut())
            .await
            .unwrap()
            .unwrap();
    }
    drop(shutdown); // Do not poll a completed pre-RED future for a second time.
    // Wait for the actual stopped FS/stream locks, even on the expected RED.
    // Successful preparation scans the real same-scope stream before and
    // after the held operation; the barrier notification alone is no success.
    let reacquired = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (_, prepared) = prepare_presence(Arc::clone(&fixture.service.registry))
                .await
                .unwrap();
            if prepared.len() == 1 {
                prepared[0].1.revalidate().unwrap();
                return prepared;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("actual same-scope stream locks must be released and valid");
    assert!(
        fixture.probe.paths.lock().unwrap().is_empty(),
        "no HTTP after stopped FS returns"
    );
    drop(reacquired);
    assert!(
        !returned_early,
        "service shutdown returned while actual admitted FS remained blocked"
    );
}
