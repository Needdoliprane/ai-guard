//! Test infrastructure only: real Store/HTTP/Journal authorization and receipts,
//! with explicitly synthetic engine observations. No Docker/Podman is invoked.
use super::super::*;
use crate::{
    control::*,
    runtime::{ContainerHandle, ManagedRuntime, OwnedContainerSnapshot, RuntimeAuthority},
};
use std::{collections::BTreeMap, path::Path};

const STARTED: &str = "2026-10-30T12:00:00Z";
const FINISHED: &str = "2026-10-30T12:00:02Z";
const CID: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const ENGINE: &str = "cancel-recovery-synthetic-engine";

#[derive(Clone, Copy)]
pub(super) enum Case {
    TerminalAfterCancel,
    TerminalCreatedOwned,
    StillRunning,
    UnknownContainer,
    ChangedCid,
    ChangedImage,
    ChangedConfiguration,
    Restarted,
    ChangedStartedAt,
    MissingStartedAt,
}

pub(super) struct RecoveryFixture {
    _peer: NativePeer,
    executor: BatchExecutor,
    runtime: Runtime,
    operation: Uuid,
    cancel: Uuid,
    worker_id: String,
    inspect: Value,
    // Fields drop in declaration order: reap the owned native test child and
    // drop executor clients before Fixture removes their temporary root.
    base: Arc<Fixture>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResultPublicationSnapshot {
    pub output_acknowledged: bool,
    pub result_outbox: Vec<(Uuid, Option<i64>)>,
    pub stored_result_events: Vec<(String, i64)>,
}

impl RecoveryFixture {
    pub(super) async fn new(case: Case) -> Self {
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || Self::seed(case, &handle))
            .await
            .unwrap()
    }

    // The calling test keeps its original runtime/IO driver alive while it
    // awaits this single blocking owner. Each phase has a separate polling
    // frame; no runtime, larger stack or detached seed is introduced here.
    fn seed(case: Case, handle: &tokio::runtime::Handle) -> Self {
        eprintln!("cancelled_recovery_fixture phase=seed_begin");
        // Domain proof with writer_timeout=60s; production writer_timeout=10s is NOT qualified.
        eprintln!("cancelled_recovery_fixture budget writer_timeout_seconds=60 domain_only=true");
        let base =
            Arc::new(handle.block_on(Fixture::new_with_writer_timeout(Duration::from_secs(60))));
        eprintln!("cancelled_recovery_fixture phase=base_ready");
        let peer = handle.block_on(NativePeer::empty(&base));
        eprintln!("cancelled_recovery_fixture phase=peer_ready");
        let program = engine_program(base.root.path());
        eprintln!("cancelled_recovery_fixture phase=engine_program_ready");
        let runtime = Runtime::with_program(RuntimeKind::Docker, &program).unwrap();
        eprintln!("cancelled_recovery_fixture phase=runtime_ready");
        let authority: RuntimeAuthority = serde_json::from_value(json!({
            "authority_id":Uuid::new_v4(), "kind":"docker",
            "endpoint":"unix:///cancel-recovery-fixture.sock", "engine_id":ENGINE,
            "invocation":runtime.invocation(), "podman":null
        }))
        .unwrap();
        eprintln!("cancelled_recovery_fixture phase=authority_ready");
        let executor = BatchExecutor::new(
            base.writer.clone(),
            peer.client.clone(),
            Arc::new(NativeHostClock::new().unwrap()),
        );
        let context = handle
            .block_on(AuthenticatedBatchContext::for_executor_test(
                base.client.clone(),
                base.scope.clone(),
                base.config.clone(),
                runtime.clone(),
            ))
            .unwrap();
        eprintln!("cancelled_recovery_fixture phase=context_ready");
        let operation = Uuid::new_v4();
        let accepted = base.accept(operation, false);
        eprintln!("cancelled_recovery_fixture phase=job_accepted");
        eprintln!("cancelled_recovery_fixture phase=accept_ready");
        let execution = handle
            .block_on(Execution::open(&executor, operation, &context))
            .unwrap();
        eprintln!("cancelled_recovery_fixture phase=execution_opened");
        let request = accepted.checkpoint().0.current_request.clone();
        let (decision, _) = handle
            .block_on(execution.rpc(Operation::Decision {
                request: request.clone(),
                response: None,
            }))
            .unwrap();
        let Operation::Decision {
            response: Some(answer),
            ..
        } = decision
        else {
            panic!("fixture decision receipt missing");
        };
        assert_eq!(answer.decision().outcome, Outcome::Allow);
        eprintln!("cancelled_recovery_fixture phase=decision_ready");
        let admission = AdmissionV1 {
            version: 1,
            operation_id: operation.to_string(),
            decision_request: request.clone(),
            receipt: answer.receipt().clone(),
            launch: LaunchSpecV1 {
                version: 1,
                session_id: request.session_id.clone().unwrap(),
                command: request.command.clone(),
                profile: request.profile.clone(),
                runtime_kind: RuntimeKindV1::Docker,
                authority_id: authority.authority_id.to_string(),
                image_ref: "agentsandbox-worker:local".into(),
                image_id: format!("sha256:{}", "a".repeat(64)),
                network: NetworkV1::None,
                cpu_millis: 2000,
                memory_bytes: 1_073_741_824,
                pids_limit: 256,
                timeout_seconds: 600,
                security_preset: SecurityPresetV1::FixedSandboxV2,
                restart_policy: RestartPolicyV1::No,
                workspace_manifest_digest: content_hash(
                    "ai-guard:workspace-manifest:v1",
                    &BTreeMap::<String, crate::workspace::Fingerprint>::new(),
                )
                .unwrap(),
                workspace_import_policy_version: 1,
                provider: "generic".into(),
                capture_output: false,
                credential_binding_id: None,
                credential_names: vec![],
                provider_resolution: None,
            },
        };
        let prepared: managed::ManagedCheckpoint = serde_json::from_value(json!({
            "tenant_id":TENANT, "gateway_origin":base.scope.gateway_origin(),
            "config_root":base.config.path(), "binding":authority, "authority_file":"runtime-authority.json",
            "admission":admission, "worker":null, "owned":null, "pending":null, "uncertain":[],
            "lock_identity":null, "before":{}, "import":null, "decision":answer.decision(),
            "output_event":null, "output_acknowledged":false, "result":null,
            "archive_name":"worker.tar", "archive_digest":"e".repeat(64), "credential_digest":null,
            "workspace_control_exclusion_version":1, "invocation_event":null
        })).unwrap();
        let admission_pending = Operation::Admission {
            request: SupervisedAdmissionV1 {
                version: 1,
                protocol: SupervisionProtocolV1::BrokerSupervisionV1,
                intent_hash: accepted.intent_hash().into(),
                admission,
            },
            response: None,
        };
        handle
            .block_on(execution.patch(|draft| {
                draft.run.plan.managed = Some(prepared.clone());
                draft.operations.push(admission_pending.clone());
                Ok(())
            }))
            .unwrap();
        handle.block_on(execution.rpc(admission_pending)).unwrap();
        eprintln!("cancelled_recovery_fixture phase=admission_ready");
        let worker = handle
            .block_on(execution.mutate(WorkerTransitionV1::Prepare))
            .unwrap();
        eprintln!("cancelled_recovery_fixture phase=prepare_ready");
        let (_, snapshot) = handle.block_on(execution.load()).unwrap();
        let admission_receipt = snapshot
            .operations
            .iter()
            .find_map(|operation| match operation {
                Operation::Admission {
                    response: Some(response),
                    ..
                } => Some(response.receipt.clone()),
                _ => None,
            })
            .unwrap();
        let (import, import_arm, import_terminal) = seed_helper(
            handle,
            &execution,
            &worker,
            &authority,
            accepted.intent_hash(),
            HelperPurposeV1::Import,
            HelperPermissionV1::Admission {
                receipt: admission_receipt,
            },
            None,
        );
        seed_helper(
            handle,
            &execution,
            &worker,
            &authority,
            accepted.intent_hash(),
            HelperPurposeV1::ImportVerification,
            HelperPermissionV1::ImportVerification {
                import_helper_id: import.to_string(),
                import_arm,
                import_terminal,
                manifest_digest: worker.launch.workspace_manifest_digest.clone(),
            },
            Some(ImportVerificationReportV1 {
                import_helper_id: import.to_string(),
                expected_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
                observed_manifest_digest: worker.launch.workspace_manifest_digest.clone(),
            }),
        );
        eprintln!("cancelled_recovery_fixture phase=helpers_ready");
        let worker = handle
            .block_on(execution.mutate(WorkerTransitionV1::CreateIssued))
            .unwrap();
        let inspection = inspection(
            &worker,
            CID,
            worker.attempt.stable_name.clone(),
            worker.spec_hash.clone(),
        );
        let worker = handle
            .block_on(execution.mutate(WorkerTransitionV1::Created {
                inspection: inspection.clone(),
            }))
            .unwrap();
        eprintln!("cancelled_recovery_fixture phase=created_ready");
        let inspect = inspect_facts(&worker);
        fs::write(
            base.root.path().join("inspect.json"),
            serde_json::to_vec(&inspect).unwrap(),
        )
        .unwrap();
        let container_handle: ContainerHandle = serde_json::from_value(json!({
            "authority_fingerprint":authority.fingerprint(),
            "session":Uuid::parse_str(&worker.launch.session_id).unwrap(),
            "operation":operation,
            "id":CID, "name":worker.attempt.stable_name, "configuration_fingerprint_version":2
        }))
        .unwrap();
        let managed_runtime = handle
            .block_on(ManagedRuntime::reopen_with_runtime(
                authority.clone(),
                runtime.clone(),
            ))
            .unwrap();
        let mut observed = handle
            .block_on(managed_runtime.observe(&container_handle))
            .unwrap();
        assert!(observed.configuration_fingerprint.is_some());
        if matches!(case, Case::TerminalCreatedOwned) {
            // Production retains the initial Created snapshot. The later
            // Running transition below still supplies central StartedAt proof.
            observed.process.status = Some("created".into());
            observed.process.running = Some(false);
            observed.process.started_at = None;
            observed.process.finished_at = None;
            observed.process.exit_code = None;
        }
        let owned = OwnedContainerSnapshot {
            handle: container_handle,
            observation: observed,
        };
        handle
            .block_on(execution.patch(|draft| {
                draft.run.plan.managed.as_mut().unwrap().owned = Some(owned.clone());
                Ok(())
            }))
            .unwrap();
        let arm = arm(
            &worker,
            &authority,
            accepted.intent_hash(),
            inspection.clone(),
            ArmSubjectV1::Worker,
            owned.observation.configuration_fingerprint.clone().unwrap(),
        );
        let (armed, _) = handle
            .block_on(execution.rpc(Operation::Arm {
                parent: worker.clone(),
                owned: owned.clone(),
                request: arm.clone(),
                response: None,
            }))
            .unwrap();
        let Operation::Arm {
            response: Some(armed),
            ..
        } = armed
        else {
            panic!("fixture arm receipt missing")
        };
        handle
            .block_on(execution.rpc(Operation::WorkerClaim {
                request: ArmedStartClaimV1 {
                    version: 1,
                    operation_id: Uuid::new_v4().to_string(),
                    worker_id: worker.worker_id.clone(),
                    attempt_id: worker.attempt.attempt_id.clone(),
                    expected_revision: worker.revision,
                    emitter_id: Uuid::new_v4().to_string(),
                    arm_receipt: armed.receipt,
                    clock_check: arm.clock,
                },
                response: None,
            }))
            .unwrap();
        eprintln!("cancelled_recovery_fixture phase=arm_claim_ready");
        let running = handle
            .block_on(execution.mutate(WorkerTransitionV1::Observe {
                observation: RuntimeObservationV1 {
                    inspection,
                    observed_at: STARTED.into(),
                    status: RuntimeStatusV1::Running,
                    running: Some(true),
                    started_at: Some(STARTED.into()),
                    finished_at: None,
                    exit_code: None,
                    oom_killed: Some(false),
                    restart_count: Some(0),
                },
            }))
            .unwrap();
        assert_eq!(running.state, WorkerStateV1::Running);
        assert!(running.quota_held);
        eprintln!("cancelled_recovery_fixture phase=running_ready");
        let cancel = Uuid::new_v4();
        handle
            .block_on(base.writer.append_command(
                operation,
                cancel,
                LocalCommandRequestV1::CancelRequested,
            ))
            .unwrap();
        eprintln!("cancelled_recovery_fixture phase=cancel_ready");
        drop(execution);
        let fixture = Self {
            base,
            _peer: peer,
            executor,
            runtime,
            operation,
            cancel,
            worker_id: worker.worker_id,
            inspect,
        };
        fixture.set_case(case);
        fs::write(fixture.base.root.path().join("calls"), b"").unwrap();
        assert!(fixture.usage().is_empty());
        fixture
    }

    pub(super) fn run_sync(
        &self,
        driver: &tokio::runtime::Runtime,
    ) -> std::result::Result<(), BatchFailure> {
        // A new authenticated context and Execution; no old execution survives.
        let context = driver
            .block_on(AuthenticatedBatchContext::for_executor_test(
                self.base.client.clone(),
                self.base.scope.clone(),
                self.base.config.clone(),
                self.runtime.clone(),
            ))
            .map_err(|_| BatchFailure)?;
        eprintln!("cancelled_recovery_run phase=context_ready");
        eprintln!("cancelled_recovery_run phase=executor_enter");
        let result = driver
            .block_on(self.executor.run(self.operation, &context))
            .map(|_| ());
        eprintln!("cancelled_recovery_run phase=executor_complete");
        result
    }
    pub(super) fn worker(&self) -> WorkerV1 {
        self.base.store.worker(&self.worker_id).unwrap().unwrap()
    }
    pub(super) fn usage(&self) -> Vec<AllocationV1> {
        self.base.store.worker_usage().unwrap()
    }
    pub(super) fn runtime_calls(&self) -> Vec<String> {
        fs::read_to_string(self.base.root.path().join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
    pub(super) fn arm_claim_snapshot(&self) -> Value {
        let job = self.base.journal.load(self.operation).unwrap().unwrap();
        let operations = job
            .broker_checkpoint_v2()
            .unwrap()
            .supervision()
            .operations()
            .iter()
            .filter(|operation| {
                matches!(
                    operation,
                    Operation::Arm { .. }
                        | Operation::WorkerClaim { .. }
                        | Operation::HelperClaim { .. }
                )
            })
            .collect::<Vec<_>>();
        serde_json::to_value(operations).unwrap()
    }
    pub(super) fn cancel_id(&self) -> Uuid {
        self.cancel
    }
    pub(super) fn cancel_present(&self) -> bool {
        self.base
            .journal
            .load(self.operation)
            .unwrap()
            .unwrap()
            .broker_checkpoint_v3()
            .unwrap()
            .commands()
            .iter()
            .any(|command| {
                command.command_id() == self.cancel
                    && matches!(command.request(), LocalCommandRequestV1::CancelRequested)
            })
    }
    pub(super) fn arm_transition_response_cut(&self) {
        self.base
            .drop_next_worker_transition_response
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    pub(super) fn transition_response_cut_pending(&self) -> bool {
        self.base
            .drop_next_worker_transition_response
            .load(std::sync::atomic::Ordering::SeqCst)
    }
    pub(super) fn result_publication_snapshot(&self) -> ResultPublicationSnapshot {
        let job = self.base.journal.load(self.operation).unwrap().unwrap();
        let private = serde_json::to_value(job.checkpoint()).unwrap();
        let output_acknowledged = private["managed"]["output_acknowledged"].as_bool().unwrap();
        let result_outbox = job
            .broker_checkpoint_v4()
            .map(|checkpoint| {
                checkpoint
                    .events()
                    .iter()
                    .filter(|entry| entry.event().kind == "broker.batch.result")
                    .map(|entry| (entry.event_id(), entry.audit_seq()))
                    .collect()
            })
            .unwrap_or_default();
        let stored_result_events = self
            .base
            .store
            .events(&crate::audit::EventQuery {
                kind: Some("broker.batch.result".into()),
                ..Default::default()
            })
            .unwrap()
            .into_iter()
            .map(|event| (event.event.id, event.seq))
            .collect();
        ResultPublicationSnapshot {
            output_acknowledged,
            result_outbox,
            stored_result_events,
        }
    }
    pub(super) fn report_post_run_diagnostic(&self) {
        let (worker_state, quota_held) = match self.base.store.worker(&self.worker_id) {
            Ok(Some(worker)) => {
                let state = match worker.state {
                    WorkerStateV1::Reserved => "reserved",
                    WorkerStateV1::Preparing => "preparing",
                    WorkerStateV1::Created => "created",
                    WorkerStateV1::StartIssued => "start_issued",
                    WorkerStateV1::Running => "running",
                    WorkerStateV1::Stopping => "stopping",
                    WorkerStateV1::Terminal => "terminal",
                    WorkerStateV1::InDoubt => "in_doubt",
                    WorkerStateV1::Cancelled => "cancelled",
                };
                (state, Some(worker.quota_held))
            }
            Ok(None) => ("missing", None),
            Err(_) => ("read_error", None),
        };
        let allocation_count = self
            .base
            .store
            .worker_usage()
            .map(|allocations| allocations.len())
            .ok();

        let mut journal_load = "read_error";
        let mut managed_output_event = None;
        let mut managed_output_acknowledged = None;
        let mut outbox_v4 = "unknown";
        let mut event_counts = [None; 6];
        match self.base.journal.load(self.operation) {
            Ok(Some(job)) => {
                journal_load = "present";
                if let Ok(private) = serde_json::to_value(job.checkpoint()) {
                    if let Some(managed) = private.get("managed").and_then(Value::as_object) {
                        managed_output_event =
                            managed.get("output_event").map(|event| !event.is_null());
                        managed_output_acknowledged =
                            managed.get("output_acknowledged").and_then(Value::as_bool);
                    }
                }
                match job.broker_checkpoint_v4() {
                    Some(checkpoint) => {
                        outbox_v4 = "present";
                        event_counts = [Some(0); 6];
                        for event in checkpoint.events() {
                            let base_index = match event.event().kind.as_str() {
                                "broker.batch.result" => 0,
                                "observer.coverage" => 2,
                                "broker.batch.failure" => 4,
                                _ => continue,
                            };
                            let index = base_index + usize::from(event.audit_seq().is_some());
                            if let Some(count) = event_counts[index].as_mut() {
                                *count += 1;
                            }
                        }
                    }
                    None => outbox_v4 = "missing",
                }
            }
            Ok(None) => journal_load = "missing",
            Err(_) => {}
        }
        let stored_result_count = self
            .base
            .store
            .events(&crate::audit::EventQuery {
                kind: Some("broker.batch.result".into()),
                ..Default::default()
            })
            .map(|events| events.len())
            .ok();
        eprintln!(
            "cancelled_recovery_post_run worker_state={worker_state} quota_held={quota_held:?} allocations={allocation_count:?} journal_load={journal_load} managed_output_event={managed_output_event:?} managed_output_acknowledged={managed_output_acknowledged:?} outbox_v4={outbox_v4} result_pending={:?} result_acked={:?} coverage_pending={:?} coverage_acked={:?} failure_pending={:?} failure_acked={:?} store_broker_batch_result_count={stored_result_count:?}",
            event_counts[0],
            event_counts[1],
            event_counts[2],
            event_counts[3],
            event_counts[4],
            event_counts[5]
        );
    }
    pub(super) fn set_case(&self, case: Case) {
        let mut inspect = self.inspect.clone();
        let missing = matches!(case, Case::UnknownContainer);
        if !matches!(case, Case::StillRunning) {
            inspect["State"]["Status"] = json!("exited");
            inspect["State"]["Running"] = json!(false);
            inspect["State"]["FinishedAt"] = json!(FINISHED);
            inspect["State"]["ExitCode"] = json!(0);
        }
        if matches!(case, Case::ChangedCid) {
            inspect["Id"] = json!("f".repeat(64));
        }
        if matches!(case, Case::ChangedImage) {
            inspect["Image"] = json!(format!("sha256:{}", "e".repeat(64)));
        }
        if matches!(case, Case::ChangedConfiguration) {
            inspect["HostConfig"]["ReadonlyRootfs"] = json!(false);
        }
        if matches!(case, Case::Restarted) {
            inspect["RestartCount"] = json!(1);
        }
        if matches!(case, Case::ChangedStartedAt) {
            inspect["State"]["StartedAt"] = json!("2026-10-30T12:00:01Z");
        }
        if matches!(case, Case::MissingStartedAt) {
            inspect["State"]
                .as_object_mut()
                .unwrap()
                .remove("StartedAt");
        }
        fs::write(
            self.base.root.path().join("missing"),
            if missing { "yes" } else { "no" },
        )
        .unwrap();
        fs::write(
            self.base.root.path().join("inspect.json"),
            serde_json::to_vec(&inspect).unwrap(),
        )
        .unwrap();
    }
}

fn inspection(
    worker: &WorkerV1,
    cid: &str,
    name: String,
    spec_hash: String,
) -> RuntimeInspectionV1 {
    RuntimeInspectionV1 {
        authority_id: worker.launch.authority_id.clone(),
        runtime_kind: worker.launch.runtime_kind,
        container_id: cid.into(),
        stable_name: name,
        volume_name: worker.attempt.volume_name.clone(),
        image_id: worker.launch.image_id.clone(),
        spec_hash,
        cpu_millis: worker.launch.cpu_millis,
        memory_bytes: worker.launch.memory_bytes,
        pids_limit: worker.launch.pids_limit,
        network: worker.launch.network,
        security_preset: worker.launch.security_preset,
        restart_policy: worker.launch.restart_policy,
    }
}

fn arm(
    worker: &WorkerV1,
    authority: &RuntimeAuthority,
    intent: &str,
    inspection: RuntimeInspectionV1,
    subject: ArmSubjectV1,
    fingerprint: String,
) -> ArmV1 {
    let clock = NativeHostClock::new().unwrap().sample().unwrap();
    let timeout = if matches!(subject, ArmSubjectV1::Worker) {
        worker.launch.timeout_seconds
    } else {
        120
    };
    let deadline = checked_deadline(clock.continuous_ns_decimal.parse().unwrap(), timeout).unwrap();
    ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        intent_hash: intent.into(),
        subject,
        configuration_fingerprint_version: inspection
            .security_preset
            .configuration_fingerprint_version(),
        inspection,
        authority_binding_hash: authority.fingerprint(),
        configuration_fingerprint: fingerprint,
        clock,
        timeout_seconds: timeout,
        deadline_ns_decimal: deadline.to_string(),
    }
}

fn seed_helper(
    handle: &tokio::runtime::Handle,
    execution: &Execution<'_>,
    worker: &WorkerV1,
    authority: &RuntimeAuthority,
    intent: &str,
    purpose: HelperPurposeV1,
    permission: HelperPermissionV1,
    verification: Option<ImportVerificationReportV1>,
) -> (Uuid, ArmReceiptV1, MutationReceiptV1) {
    let helper = Uuid::new_v4();
    let policy = helper_execution_policy_v1(worker, purpose, helper).unwrap();
    handle
        .block_on(execution.patch(|draft| {
            draft.helpers.push(HelperPreparationV1::new(
                policy.clone(),
                permission.clone(),
                None,
            )?);
            Ok(())
        }))
        .unwrap();
    let inspect = RuntimeInspectionV1 {
        authority_id: policy.authority_id.clone(),
        runtime_kind: policy.runtime_kind,
        container_id: format!("{:064x}", helper.as_u128()),
        stable_name: policy.stable_name.clone(),
        volume_name: policy.volume_name.clone(),
        image_id: policy.image_id.clone(),
        spec_hash: policy.spec_hash().unwrap(),
        cpu_millis: policy.cpu_millis,
        memory_bytes: policy.memory_bytes,
        pids_limit: policy.pids_limit,
        network: policy.network,
        security_preset: policy.security_preset,
        restart_policy: policy.restart_policy,
    };
    let fingerprint = "c".repeat(64);
    let owned: OwnedContainerSnapshot = serde_json::from_value(json!({
        "handle":{"authority_fingerprint":authority.fingerprint(), "session":worker.launch.session_id,
            "operation":helper, "id":inspect.container_id, "name":inspect.stable_name,
            "configuration_fingerprint_version":2},
        "observation":{"id":inspect.container_id, "name":inspect.stable_name, "image_id":inspect.image_id,
            "restart_count":0, "process":{"status":"running", "running":true, "started_at":STARTED,
                "finished_at":null, "exit_code":null, "oom_killed":false},
            "allocation":{"cpu":{"quota":u64::from(inspect.cpu_millis)*100,"period":100000},
                "memory_bytes":inspect.memory_bytes,"pids_limit":inspect.pids_limit},
            "configuration_fingerprint":fingerprint,"configuration_fingerprint_version":2,
            "observed_at":STARTED,"diagnostics":[]}
    })).unwrap();
    handle
        .block_on(execution.patch(|draft| {
            let entry = draft.helpers.last_mut().unwrap();
            *entry =
                HelperPreparationV1::new(policy.clone(), permission.clone(), Some(owned.clone()))?;
            Ok(())
        }))
        .unwrap();
    let arm = arm(
        worker,
        authority,
        intent,
        inspect.clone(),
        ArmSubjectV1::Helper {
            helper_id: helper.to_string(),
            purpose,
            policy_version: 1,
            permission: Box::new(permission),
        },
        fingerprint,
    );
    let (armed, _) = handle
        .block_on(execution.rpc(Operation::Arm {
            parent: worker.clone(),
            owned,
            request: arm.clone(),
            response: None,
        }))
        .unwrap();
    let Operation::Arm {
        response: Some(armed),
        ..
    } = armed
    else {
        panic!("fixture helper arm missing")
    };
    handle
        .block_on(execution.rpc(Operation::HelperClaim {
            request: HelperStartClaimV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                helper_id: helper.to_string(),
                emitter_id: Uuid::new_v4().to_string(),
                arm_receipt: armed.receipt.clone(),
                clock_check: arm.clock,
            },
            response: None,
        }))
        .unwrap();
    let (terminal, _) = handle
        .block_on(execution.rpc(Operation::HelperObservation {
            request: HelperObservationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                helper_id: helper.to_string(),
                arm_receipt: armed.receipt.clone(),
                verification,
                observation: RuntimeObservationV1 {
                    inspection: inspect,
                    observed_at: FINISHED.into(),
                    status: RuntimeStatusV1::Exited,
                    running: Some(false),
                    started_at: Some(STARTED.into()),
                    finished_at: Some(FINISHED.into()),
                    exit_code: Some(0),
                    oom_killed: Some(false),
                    restart_count: Some(0),
                },
            },
            response: None,
        }))
        .unwrap();
    let Operation::HelperObservation {
        response: Some(receipt),
        ..
    } = terminal
    else {
        panic!("fixture helper terminal missing")
    };
    (helper, armed.receipt, receipt)
}

fn inspect_facts(worker: &WorkerV1) -> Value {
    json!({
        "Id":CID, "Name":format!("/{}",worker.attempt.stable_name), "Image":worker.launch.image_id,
        "RestartCount":0, "State":{"Status":"running","Running":true,"StartedAt":STARTED,
            "FinishedAt":"0001-01-01T00:00:00Z","ExitCode":0,"OOMKilled":false},
        "Config":{"Labels":{},"User":"1000:1000","WorkingDir":"/workspace",
            "Entrypoint":["/usr/bin/id"],"Cmd":null,"Tty":false,"OpenStdin":false,"StdinOnce":false},
        "HostConfig":{"CpuQuota":200000,"CpuPeriod":100000,"Memory":1_073_741_824,"PidsLimit":256,
            "Privileged":false,"ReadonlyRootfs":true,"NetworkMode":"none","CapDrop":["ALL"],"CapAdd":[],
            "SecurityOpt":["no-new-privileges"],"RestartPolicy":{"Name":"no"},"AutoRemove":false,
            "Tmpfs":{},"LogConfig":{"Type":"none","Config":{}}},
        "Mounts":[{"Type":"volume","Name":worker.attempt.volume_name,"Destination":"/workspace","RW":true}],
        "NetworkSettings":{"Networks":{}}
    })
}

fn engine_program(root: &Path) -> PathBuf {
    let directory = root.join("control");
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let source = directory.join("engine.rs");
    fs::write(&source, r#"
use std::{fs, io::Write};
fn main() {
    let root = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_owned();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let start = args.iter().position(|arg| arg == "info" || arg == "inspect").unwrap_or(0);
    let args = &args[start..];
    let mut calls = fs::OpenOptions::new().append(true).create(true).open(root.join("calls")).unwrap();
    match args.first().map(String::as_str) {
        Some("info") => {
            writeln!(calls,"info").unwrap();
            println!("{{\"ID\":\"cancel-recovery-synthetic-engine\"}}");
        }
        Some("inspect") => {
            writeln!(calls,"inspect {}",args.last().unwrap()).unwrap();
            if fs::read_to_string(root.join("missing")).is_ok_and(|v| v == "yes") {std::process::exit(125)}
            print!("{}",fs::read_to_string(root.join("inspect.json")).unwrap());
        }
        _ => { writeln!(calls,"{}",args.join(" ")).unwrap(); std::process::exit(125); }
    }
}
"#).unwrap();
    let program = directory.join("engine");
    let output = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .args([
            "--edition=2024",
            "--crate-name",
            "cancel_recovery_engine_fixture",
        ])
        .arg(&source)
        .arg("-o")
        .arg(&program)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "synthetic engine fixture must compile"
    );
    program
}
