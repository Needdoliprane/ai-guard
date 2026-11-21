//! Result-query authorization through real broker IPC and gateway routing.
use super::owned_ipc_peer::OwnedIpcPeer;
use super::*;
use crate::{
    api::{GatewayState, TenantGateway, gateway_router},
    audit::Store,
    broker::{
        intent::ExecutionIntentV1,
        journal::{BindingInputs, JournalError},
    },
    policy::Policy,
    runner::CheckpointPayloadV1,
};
use anyhow::Context;
use std::{fs, net::SocketAddr, os::unix::fs::PermissionsExt, path::PathBuf};
use tokio::net::TcpListener;

const PROFILE: &str = "result_auth";
const TENANT_A: &str = "tenant-result-a";
const TENANT_B: &str = "tenant-result-b";
const A_RUNNER: &str = "result-test-runner-alpha-aaaaaaaaaaaaaaaa";
const A_VIEWER: &str = "result-test-viewer-alpha-bbbbbbbbbbbbbbbb";
const B_RUNNER: &str = "result-test-runner-bravo-cccccccccccccccc";
const B_VIEWER: &str = "result-test-viewer-bravo-dddddddddddddddd";

struct GatewayTask {
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl GatewayTask {
    async fn finish(mut self) -> Result<()> {
        let _ = self.stop.take().unwrap().send(());
        tokio::time::timeout(Duration::from_secs(5), &mut self.task).await???;
        Ok(())
    }
}

impl Drop for GatewayTask {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn connect_service(service: &Arc<Service>) -> Result<OwnedIpcPeer> {
    let serving = Arc::clone(service);
    OwnedIpcPeer::start(move |stream| {
        let serving = Arc::clone(&serving);
        async move {
            Box::pin(serving.connection(stream)).await;
            Ok(())
        }
    })
    .await
}

async fn exchange(
    peer: &OwnedIpcPeer,
    credential: &str,
    request: BrokerCommandV1,
) -> BrokerReplyV1 {
    crate::broker::service::request(&peer.root, PROFILE, credential, request)
        .await
        .unwrap()
}

async fn query(
    peer: &OwnedIpcPeer,
    credential: &str,
    version: u32,
    operation_id: Uuid,
) -> BrokerReplyV1 {
    exchange(
        peer,
        credential,
        BrokerCommandV1::Result {
            version,
            operation_id,
        },
    )
    .await
}

struct ResultFixture {
    _temp: tempfile::TempDir,
    base: PathBuf,
    registry: Arc<Registry>,
    address: SocketAddr,
}

impl ResultFixture {
    async fn new() -> Result<(Self, TcpListener)> {
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()?;
        let base = temp.path().canonicalize()?;
        for directory in ["broker", "configuration", "sources", "exports"] {
            let directory = base.join(directory);
            fs::create_dir(&directory)?;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let database_a = base.join("a.sqlite");
        let seed = Store::create_supervised_for_tenant(
            &database_a,
            TENANT_A,
            Redactor::new(vec![]),
            1000,
            &base.join("a-before-v3.sqlite"),
        )?;
        let supervision = seed.supervision_state()?;
        let namespace = Uuid::parse_str(
            supervision
                .control_namespace_uuid
                .as_deref()
                .context("fixture namespace")?,
        )?;
        drop(seed);

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let gateway_origin = format!("http://{address}");

        let root = base.join("broker");
        let configuration = base.join("configuration");
        let registry_path = base.join("registry.json");
        fs::write(
            &registry_path,
            serde_json::to_vec(&json!({
                "version": 1,
                "installation_id": Uuid::new_v4(),
                "root": root,
                "max_enforcer_slots": 1,
                "profiles": [{
                    "id": PROFILE,
                    "gateway": gateway_origin,
                    "tenant_id": TENANT_A,
                    "control_namespace_uuid": namespace,
                    "config_dir": configuration,
                    "runner_token_file": configuration.join("never-read.token"),
                    "runtime": "docker",
                    "runtime_program": "/usr/bin/false",
                    "runtime_endpoint": "unix:///never-used",
                    "image": "never-used:test",
                    "policy_profile": "strict",
                    "network": "none",
                    "allow_rootful_runtime": false,
                    "max_timeout_seconds": 30,
                    "allow_capture_output": false,
                    "managed_clients": false,
                    "source_roots": [base.join("sources")],
                    "export_roots": [base.join("exports")]
                }]
            }))?,
        )?;
        fs::set_permissions(&registry_path, fs::Permissions::from_mode(0o600))?;
        let registry = Arc::new(Registry::load(&registry_path)?);
        Ok((
            Self {
                _temp: temp,
                base,
                registry,
                address,
            },
            listener,
        ))
    }

    fn start_gateway(&self, listener: TcpListener) -> Result<(GatewayTask, Arc<Store>)> {
        assert_eq!(listener.local_addr()?, self.address);
        let a = Arc::new(Store::open_existing_for_tenant(
            &self.base.join("a.sqlite"),
            TENANT_A,
            Redactor::new(vec![]),
            1000,
        )?);
        let b = Arc::new(Store::open_for_tenant(
            &self.base.join("b.sqlite"),
            TENANT_B,
            Redactor::new(vec![]),
            1000,
        )?);
        let state = a.supervision_state()?;
        assert_eq!(state.schema_version, 3);
        assert!(!state.active);
        let tenant = |id: &str, store, runner: &str, viewer: &str| TenantGateway {
            tenant_id: id.into(),
            store,
            runner_token: runner.into(),
            viewer_token: viewer.into(),
        };
        let gateway = gateway_router(GatewayState::for_tenants(
            vec![
                tenant(TENANT_A, a.clone(), A_RUNNER, A_VIEWER),
                tenant(TENANT_B, b, B_RUNNER, B_VIEWER),
            ],
            Policy::default(),
            None,
            Redactor::new(vec![]),
        )?);
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, gateway)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        });
        Ok((
            GatewayTask {
                stop: Some(stop),
                task,
            },
            a,
        ))
    }

    fn service(&self) -> Result<Arc<Service>> {
        let root = Directory::open_existing(&self.base.join("broker"))?;
        let enforcer = EnforcerClient::unavailable_for_test(&root)?.into();
        Ok(Arc::new(Service {
            registry: self.registry.clone(),
            root,
            material: Arc::new(Material {
                literals: Vec::new(),
                files: Vec::new(),
                parents: Vec::new(),
                exclusions: FsExclusions::new(),
                complete: true,
            }),
            profiles: BTreeMap::from([(PROFILE.into(), Arc::new(ProfileJobs::new()))]),
            enforcer: Some(cancel_retry_tests::TestEnforcer::Native(enforcer)),
            stop_enforcer: None,
            blocking: Arc::new(Semaphore::new(1)),
            preparation: Arc::new(Semaphore::new(1)),
            execution: Arc::new(Semaphore::new(1)),
            cold_owners: retained_audit::ColdOwners::new(1)?,
            full_owners: FullOwners::new(1)?,
            stopping: Arc::new(AtomicBool::new(false)),
            fenced: Arc::new(AtomicBool::new(false)),
        }))
    }
}

#[tokio::test(flavor = "current_thread")]
async fn result_query_requires_the_profile_runner_and_allows_inactive_control() {
    let (fixture, listener) = ResultFixture::new().await.unwrap();
    let (_gateway, _store) = fixture.start_gateway(listener).unwrap();
    let service = fixture.service().unwrap();
    let jobs = service.profile_jobs(PROFILE).unwrap();
    let peer = connect_service(&service).await.unwrap();
    let operation_id = Uuid::new_v4();
    for credential in [A_VIEWER, B_RUNNER] {
        assert!(matches!(
            query(&peer, credential, 1, operation_id).await,
            BrokerReplyV1::Error {
                code: BrokerErrorCodeV1::Forbidden
            }
        ));
    }
    for (version, operation) in [(2, operation_id), (1, Uuid::nil())] {
        assert!(matches!(
            query(&peer, A_RUNNER, version, operation).await,
            BrokerReplyV1::Error {
                code: BrokerErrorCodeV1::InvalidInput
            }
        ));
    }
    assert!(jobs.writer.get().is_none());
    assert_eq!(fs::read_dir(service.root.path()).unwrap().count(), 0);

    for (fenced, stopping) in [(true, false), (false, true), (false, false)] {
        service.fenced.store(fenced, Ordering::SeqCst);
        service.stopping.store(stopping, Ordering::SeqCst);
        assert!(matches!(
            query(&peer, A_RUNNER, 1, operation_id).await,
            BrokerReplyV1::Error {
                code: BrokerErrorCodeV1::NeedsReconciliation
            }
        ));
        assert!(jobs.writer.get().is_none());
        assert_eq!(fs::read_dir(service.root.path()).unwrap().count(), 0);
    }

    let verified = VerifiedProfile::authenticate(&service.registry, PROFILE, A_RUNNER, false)
        .await
        .unwrap();
    service.writer(PROFILE, &verified.scope).await.unwrap();

    let BrokerReplyV1::Result { result } = query(&peer, A_RUNNER, 1, operation_id).await else {
        panic!("an authorized runner must receive a versioned result during maintenance");
    };
    assert_eq!(result.version, 1);
    assert_eq!(result.operation_id, operation_id);
    assert!(!result.cancel_requested);
    assert!(matches!(
        result.outcome,
        JobResultOutcomeV1::Ambiguous { .. }
    ));
    assert!(
        jobs.writer
            .get()
            .is_some_and(|writer| writer.full().is_some())
    );
    peer.finish().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn confirmed_cancel_replays_after_gateway_outage_and_broker_reopen() -> Result<()> {
    let (fixture, listener) = ResultFixture::new().await?;
    let (gateway, store) = fixture.start_gateway(listener)?;
    let original_audit = serde_json::to_value(store.integrity()?)?;
    assert_eq!(original_audit["valid"], true);
    let first = fixture.service()?;
    let verified =
        VerifiedProfile::authenticate(&fixture.registry, PROFILE, A_RUNNER, false).await?;
    let operation = Uuid::new_v4();
    let command = Uuid::new_v4();
    let journal_root = first
        .root
        .persistent_child("journals", true)?
        .context("journal root missing")?
        .persistent_child(PROFILE, true)?
        .context("journal profile missing")?;
    let open_journal = || {
        Journal::open(
            &journal_root,
            verified.scope.clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(Vec::new()).unwrap(),
        )
    };
    {
        let journal = open_journal()?;
        // Seed a real accepted intention before any decision or admission.
        // The production Cancel and Result paths below need no runtime facts.
        let mut plan: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/broker_intent-plan-v3.json"
        ))?;
        plan["version"] = json!(1);
        plan["operation_id"] = json!(operation);
        let options = plan["options"]
            .as_object_mut()
            .context("fixture options missing")?;
        for field in [
            "client",
            "auth_mode",
            "client_profile",
            "providers_file",
            "prompt",
            "model",
        ] {
            options.remove(field);
        }
        options.insert("project".into(), json!(fixture.base.join("sources")));
        options.insert(
            "config_dir".into(),
            json!(fixture.base.join("configuration")),
        );
        options.insert("gateway".into(), json!(verified.scope.gateway_origin()));
        options.insert("profile".into(), json!("strict"));
        options.insert("network".into(), json!("none"));
        options.insert("provider".into(), json!("generic"));
        options.insert("argv".into(), json!(["/usr/bin/id"]));
        plan["request"]["profile"] = json!("strict");
        plan["request"]["agent_id"] = json!("generic-22222222-2222-4222-8222-222222222222");
        plan["request"]["command"]["argv"] = json!(["/usr/bin/id"]);
        plan["request"]
            .as_object_mut()
            .context("fixture request missing")?
            .remove("provider_resolution");
        plan["provider_input"] = Value::Null;
        plan["provider_credential"] = Value::Null;
        let intent = ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan)?)?;
        let ReserveResult::New(ticket) =
            journal.reserve(operation, br#"{"fixture":"cancel-before-admission"}"#)?
        else {
            anyhow::bail!("fixture must reserve one new job");
        };
        journal.accept(
            ticket,
            &intent,
            BindingInputs::None,
            &CheckpointPayloadV1::initial(&intent)?,
        )?;
    }
    let cancel = || BrokerCommandV1::Cancel {
        operation_id: operation,
        command_id: command,
    };
    let peer = connect_service(&first).await?;
    let ack = exchange(&peer, A_RUNNER, cancel()).await;
    let BrokerReplyV1::CommandAcknowledged {
        operation_id,
        command_id,
        revision,
        checkpoint_hash,
        ..
    } = &ack
    else {
        anyhow::bail!("the first Cancel did not return a confirmed journal receipt");
    };
    assert_eq!((*operation_id, *command_id), (operation, command));
    assert_eq!(*revision, 1);
    peer.finish().await?;
    drop(first);
    gateway.finish().await?;
    drop(store);

    let second = fixture.service()?;
    let jobs = second.profile_jobs(PROFILE)?;
    let peer = connect_service(&second).await?;
    assert!(matches!(
        query(&peer, A_RUNNER, 1, operation).await,
        BrokerReplyV1::Error {
            code: BrokerErrorCodeV1::Unavailable
        }
    ));
    assert!(jobs.writer.get().is_none());
    assert!(jobs.known(operation).is_none());

    // Wait only for the old actor to release its genuine filesystem lease.
    // This observer closes it before the new service opens its own writer.
    let journal = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match open_journal() {
                Err(JournalError::Busy) => tokio::time::sleep(Duration::from_millis(10)).await,
                result => break result,
            }
        }
    })
    .await??;
    let confirmed = journal
        .load(operation)?
        .context("confirmed job missing after outage")?;
    assert_eq!(confirmed.head().revision(), *revision);
    assert_eq!(confirmed.head().hash(), checkpoint_hash);
    drop(journal);

    let listener = TcpListener::bind(fixture.address).await?;
    let (gateway, store) = fixture.start_gateway(listener)?;
    assert_eq!(
        store.supervision_state()?.control_namespace_uuid.as_deref(),
        Some(verified.scope.control_namespace_uuid().to_string().as_str())
    );
    let BrokerReplyV1::Result { result } = query(&peer, A_RUNNER, 1, operation).await else {
        anyhow::bail!("the new service did not return the recorded batch result");
    };
    assert_eq!(result.version, 1);
    assert_eq!(result.operation_id, operation);
    assert!(result.cancel_requested);
    assert!(matches!(result.outcome, JobResultOutcomeV1::Cancelled));
    assert!(
        jobs.known(operation).is_none(),
        "result query must not launch anything"
    );

    let replay = exchange(&peer, A_RUNNER, cancel()).await;
    assert_eq!(serde_json::to_value(&replay)?, serde_json::to_value(&ack)?);
    let writer = jobs
        .writer
        .get()
        .and_then(ProfileWriterCapabilities::full)
        .context("reopened writer missing")?;
    let job = writer
        .load(operation)
        .await?
        .context("replayed job missing")?;
    assert_eq!(job.head(), confirmed.head());
    let checkpoint = job
        .broker_checkpoint_v3()
        .context("cancel checkpoint missing")?;
    assert_eq!(checkpoint.commands().len(), 1);
    assert_eq!(checkpoint.commands()[0].command_id(), command);
    assert!(matches!(
        checkpoint.commands()[0].request(),
        LocalCommandRequestV1::CancelRequested
    ));
    assert!(
        job.broker_checkpoint_v2()
            .context("supervision checkpoint missing")?
            .supervision()
            .operations()
            .is_empty()
    );
    assert!(job.broker_checkpoint_v4().is_none());
    assert!(store.workers()?.is_empty());
    assert_eq!(serde_json::to_value(store.integrity()?)?, original_audit);
    peer.finish().await?;
    gateway.finish().await?;
    Ok(())
}
