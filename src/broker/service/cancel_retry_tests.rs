//! Handler control-flow regression using a real Journal/Writer. Delivery is a
//! bounded test seam; this module makes no claim about IPC or runtime stopping.
use super::*;
use crate::{
    broker::{intent::ExecutionIntentV1, journal::BindingInputs},
    runner::CheckpointPayloadV1,
};
use std::{cell::RefCell, fs, ops::Deref, os::unix::fs::PermissionsExt};

/// Production aliases EnforcerClient directly. A probe-only Service cannot
/// access a native client: leaving the explicitly scoped seam is a test bug.
pub(super) enum TestEnforcer {
    Native(EnforcerClient),
    CancelDeliveryProbe,
}
impl Deref for TestEnforcer {
    type Target = EnforcerClient;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Native(client) => client,
            Self::CancelDeliveryProbe => {
                panic!("cancel control-flow fixture escaped its scoped delivery seam")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Attempt {
    operation: Uuid,
    generation: u64,
}
#[derive(Default)]
struct Probe {
    attempts: Vec<Attempt>,
    overflow: bool,
}
thread_local! {
    static DELIVERY: RefCell<Option<Probe>> = const { RefCell::new(None) };
}

pub(super) fn delivery(operation: Uuid, generation: u64) -> Option<Result<(), BrokerErrorCodeV1>> {
    DELIVERY.with(|cell| {
        let mut state = cell.borrow_mut();
        let probe = state.as_mut()?;
        if probe.attempts.len() >= 2 {
            probe.overflow = true;
            return Some(Err(BrokerErrorCodeV1::Busy));
        }
        probe.attempts.push(Attempt {
            operation,
            generation,
        });
        Some(if probe.attempts.len() == 1 {
            Err(BrokerErrorCodeV1::NeedsReconciliation)
        } else {
            Ok(())
        })
    })
}
struct DeliveryScope;
impl DeliveryScope {
    fn begin() -> Self {
        DELIVERY.with(|cell| {
            assert!(
                cell.borrow().is_none(),
                "one delivery probe per test thread"
            );
            *cell.borrow_mut() = Some(Probe::default());
        });
        Self
    }
    fn attempts(&self) -> Vec<Attempt> {
        DELIVERY.with(|cell| cell.borrow().as_ref().unwrap().attempts.clone())
    }
    fn overflowed(&self) -> bool {
        DELIVERY.with(|cell| cell.borrow().as_ref().unwrap().overflow)
    }
}
impl Drop for DeliveryScope {
    fn drop(&mut self) {
        DELIVERY.with(|cell| *cell.borrow_mut() = None);
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    service: Service,
    jobs: Arc<ProfileJobs>,
    journal: Journal,
    writer: WriterClient,
    operation: Uuid,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let base = temp.path().canonicalize().unwrap();
        for name in ["root", "configuration", "sources", "exports", "journal"] {
            fs::create_dir(base.join(name)).unwrap();
            fs::set_permissions(base.join(name), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let installation = Uuid::from_u128(0x11111111111141118111111111111111);
        let namespace = Uuid::from_u128(0x22222222222242228222222222222222);
        let scope = JournalScopeV1::new(
            installation,
            "tenant-cancel-retry-test",
            "http://127.0.0.1:8787",
            namespace,
            "cancel_retry_test",
        )
        .unwrap();
        // Real private configuration parsing, without a gateway call or a
        // credential read. Authentication is outside this private-handler test.
        let config = base.join("registry.json");
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "version": 1,
                "installation_id": installation,
                "root": base.join("root"),
                "max_enforcer_slots": 1,
                "profiles": [{
                    "id": scope.profile_id(),
                    "gateway": scope.gateway_origin(),
                    "tenant_id": scope.tenant_id(),
                    "control_namespace_uuid": namespace,
                    "config_dir": base.join("configuration"),
                    "runner_token_file": base.join("configuration/never-read.token"),
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
            }))
            .unwrap(),
        )
        .unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
        let registry = Arc::new(Registry::load(&config).unwrap());
        let journal = Journal::open(
            &Directory::open_existing(&base.join("journal")).unwrap(),
            scope.clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(Vec::new()).unwrap(),
        )
        .unwrap();
        let operation = Uuid::from_u128(0x33333333333343338333333333333333);
        let intent = intent(operation);
        let ticket = match journal
            .reserve(operation, br#"{"cancel_retry_test":true}"#)
            .unwrap()
        {
            ReserveResult::New(ticket) => ticket,
            other => panic!("fresh real journal reservation required: {other:?}"),
        };
        journal
            .accept(
                ticket,
                &intent,
                BindingInputs::None,
                &CheckpointPayloadV1::initial(&intent).unwrap(),
            )
            .unwrap();
        let writer = WriterClient::start(journal.clone(), 8, Duration::from_secs(10)).unwrap();
        let jobs = Arc::new(ProfileJobs::new());
        let service = Service {
            registry,
            root: Directory::open_existing(&base.join("root")).unwrap(),
            material: Arc::new(Material {
                literals: Vec::new(),
                files: Vec::new(),
                parents: Vec::new(),
                exclusions: FsExclusions::new(),
                complete: true,
            }),
            profiles: BTreeMap::from([(scope.profile_id().into(), Arc::clone(&jobs))]),
            enforcer: TestEnforcer::CancelDeliveryProbe,
            blocking: Arc::new(Semaphore::new(1)),
            preparation: Arc::new(Semaphore::new(1)),
            execution: Arc::new(Semaphore::new(1)),
            stopping: Arc::new(AtomicBool::new(false)),
            fenced: AtomicBool::new(false),
        };
        Self {
            _temp: temp,
            service,
            jobs,
            journal,
            writer,
            operation,
        }
    }
}

fn intent(operation: Uuid) -> ExecutionIntentV1 {
    let mut plan: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/broker_intent-plan-v3.json"
    )))
    .unwrap();
    plan["version"] = json!(1);
    plan["operation_id"] = json!(operation);
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
    ExecutionIntentV1::from_plan_bytes(&serde_json::to_vec(&plan).unwrap()).unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn acknowledged_cancel_retry_redelivers_without_revoking_later_execution() {
    let fixture = Fixture::new();
    let command = Uuid::from_u128(0x44444444444444448444444444444444);
    let probe = DeliveryScope::begin();
    let first = fixture
        .service
        .cancel(
            fixture.writer.clone(),
            Arc::clone(&fixture.jobs),
            fixture.operation,
            command,
        )
        .await;
    assert!(matches!(first, Err(BrokerErrorCodeV1::NeedsReconciliation)));
    let job = fixture.jobs.job(fixture.operation).unwrap();
    assert_eq!(
        probe.attempts(),
        vec![Attempt {
            operation: fixture.operation,
            generation: job.cancel.current(),
        }],
        "positive control: the first durable request attempted delivery"
    );
    let receipt = fixture
        .journal
        .command_receipt(fixture.operation, command)
        .unwrap()
        .unwrap();
    let head = fixture
        .writer
        .load(fixture.operation)
        .await
        .unwrap()
        .unwrap()
        .head()
        .clone();
    assert_eq!(receipt.head(), &head);

    // This is the real execution gate held by a later Export, not a simulated
    // runtime export or a claim that any helper has been launched.
    let later_export = job
        .enter()
        .expect("execution may enter after acknowledged Cancel");
    let generation = later_export.generation();
    assert!(matches!(job.enter(), Err(BrokerErrorCodeV1::Busy)));
    let replay = tokio::time::timeout(
        Duration::from_secs(5),
        fixture.service.cancel(
            fixture.writer.clone(),
            Arc::clone(&fixture.jobs),
            fixture.operation,
            command,
        ),
    )
    .await
    .expect("Cancel replay must not wait behind the later execution gate")
    .unwrap_or_else(|code| panic!("replayed handler returned {code:?}"));
    assert_eq!(
        serde_json::to_value(replay.reply).unwrap(),
        serde_json::to_value(command_ack(fixture.operation, &receipt)).unwrap()
    );
    assert!(replay.run.is_none());
    assert_eq!(
        job.cancel.current(),
        generation,
        "old Cancel must not invalidate later execution"
    );
    assert!(matches!(job.enter(), Err(BrokerErrorCodeV1::Busy)));
    assert_eq!(
        fixture
            .writer
            .load(fixture.operation)
            .await
            .unwrap()
            .unwrap()
            .head(),
        &head
    );
    assert_eq!(
        fixture
            .journal
            .command_receipt(fixture.operation, command)
            .unwrap()
            .unwrap()
            .head(),
        receipt.head()
    );
    assert!(!probe.overflowed());
    assert_eq!(
        probe.attempts(),
        vec![
            Attempt {
                operation: fixture.operation,
                generation
            },
            Attempt {
                operation: fixture.operation,
                generation
            },
        ],
        "a durable Cancel ACK is not a delivered stop: replay must retry delivery after the first failure"
    );
}
