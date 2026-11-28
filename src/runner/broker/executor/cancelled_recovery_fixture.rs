//! Test infrastructure only: real Store/HTTP/Journal authorization and receipts,
//! with explicitly synthetic engine observations. No Docker/Podman is invoked.
#[path = "egress_ack_tests.rs"]
mod egress_ack_tests;

use super::super::*;
use crate::{
    broker::{
        enforcer::retirement::test_support::OriginalUnusedFixture,
        journal::{JournalLimitsV1, recovery::StopRecoveryReaderV1},
    },
    control::*,
    runtime::{ContainerHandle, ManagedRuntime, OwnedContainerSnapshot, RuntimeAuthority},
};
use std::{collections::BTreeMap, path::Path, sync::atomic::Ordering};

const STARTED: &str = "2026-10-30T12:00:00Z";
const FINISHED: &str = "2026-10-30T12:00:02Z";
const CID: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const ENGINE: &str = "cancel-recovery-synthetic-engine";

#[derive(Clone, Copy)]
pub(super) enum Case {
    TerminalAfterCancel,
    TerminalCreatedOwned,
    TerminalCleanupRecovery,
    TerminalCleanupWithoutScan,
    StillRunning,
    Full3WorkerClaim,
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
        tokio::task::spawn_blocking(move || Self::seed(case, &handle, false, None))
            .await
            .unwrap()
    }

    async fn new_for_cold_audit_test() -> Self {
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            Self::seed(Case::TerminalAfterCancel, &handle, true, None)
        })
        .await
        .unwrap()
    }

    async fn new_for_cold_audit_test_with_scope(scope: (Uuid, String)) -> Self {
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            Self::seed(Case::TerminalAfterCancel, &handle, true, Some(scope))
        })
        .await
        .unwrap()
    }

    // The calling test keeps its original runtime/IO driver alive while it
    // awaits this single blocking owner. Each phase has a separate polling
    // frame; no runtime, larger stack or detached seed is introduced here.
    fn seed(
        case: Case,
        handle: &tokio::runtime::Handle,
        service_layout: bool,
        scope_override: Option<(Uuid, String)>,
    ) -> Self {
        let full3_worker_claim = matches!(case, Case::Full3WorkerClaim);
        eprintln!("cancelled_recovery_fixture phase=seed_begin");
        // Domain proof with writer_timeout=60s; production writer_timeout=10s is NOT qualified.
        eprintln!("cancelled_recovery_fixture budget writer_timeout_seconds=60 domain_only=true");
        let base = Arc::new(if let Some((installation, profile)) = scope_override {
            assert!(service_layout);
            handle.block_on(Fixture::new_cold_recovery_with_scope(
                Duration::from_secs(60),
                installation,
                profile,
            ))
        } else if service_layout {
            handle.block_on(Fixture::new_cold_recovery_with_writer_timeout(
                Duration::from_secs(60),
            ))
        } else {
            handle.block_on(Fixture::new_with_writer_timeout(Duration::from_secs(60)))
        });
        eprintln!("cancelled_recovery_fixture phase=base_ready");
        let peer = handle.block_on(NativePeer::for_admission(&base));
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
            base.writer().clone(),
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
                egress: None,
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
        let (import, import_arm, import_terminal) = handle
            .block_on(boxed_execution_phase(|| {
                seed_helper(
                    &execution,
                    &worker,
                    &authority,
                    accepted.intent_hash(),
                    HelperPurposeV1::Import,
                    HelperPermissionV1::Admission {
                        receipt: admission_receipt,
                    },
                    None,
                )
            }))
            .unwrap();
        handle
            .block_on(boxed_execution_phase(|| {
                seed_helper(
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
                )
            }))
            .unwrap();
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
        if matches!(
            case,
            Case::TerminalCreatedOwned
                | Case::TerminalCleanupRecovery
                | Case::TerminalCleanupWithoutScan
                | Case::Full3WorkerClaim
        ) {
            // The initial snapshot is Created. Cases continuing past claim
            // record their synthetic Running observation separately.
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
        let claim = ArmedStartClaimV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            worker_id: worker.worker_id.clone(),
            attempt_id: worker.attempt.attempt_id.clone(),
            expected_revision: worker.revision,
            emitter_id: Uuid::new_v4().to_string(),
            arm_receipt: armed.receipt,
            clock_check: arm.clock.clone(),
        };
        if full3_worker_claim {
            handle
                .block_on(boxed_execution_phase(|| {
                    seed_full3_worker_claim(&execution, &arm, &owned, &authority, claim)
                }))
                .expect("Full3 Worker binding and claim setup");
        } else {
            handle
                .block_on(execution.rpc(Operation::WorkerClaim {
                    request: claim,
                    response: None,
                }))
                .unwrap();
        }
        eprintln!("cancelled_recovery_fixture phase=arm_claim_ready");
        let running = if full3_worker_claim {
            None
        } else {
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
            Some(running)
        };
        let cancel = Uuid::new_v4();
        if !full3_worker_claim {
            write_observation_fixture(base.root.path(), &inspect, case);
        }
        if matches!(
            case,
            Case::TerminalCleanupRecovery | Case::TerminalCleanupWithoutScan
        ) {
            let terminal = handle
                .block_on(boxed_execution_phase(|| {
                    managed_runtime.resume_terminal(&owned.handle)
                }))
                .unwrap();
            handle
                .block_on(boxed_execution_phase(|| {
                    execution.save_observed_result(&terminal)
                }))
                .unwrap();
            let running = running.as_ref().expect("cleanup fixture Running Worker");
            let observation = managed::observation_dto(running, &terminal).unwrap();
            handle
                .block_on(boxed_execution_phase(|| {
                    execution.mutate(WorkerTransitionV1::Observe { observation })
                }))
                .unwrap();
            handle
                .block_on(boxed_execution_phase(|| execution.post_worker_result()))
                .unwrap();
            if matches!(case, Case::TerminalCleanupRecovery) {
                // Represent a previously published scan to isolate recovery of
                // the worker cleanup without starting a scan helper.
                handle
                    .block_on(execution.fact_event(
                        "broker.batch.scan",
                        &json!({
                            "version": 1, "worker_id": running.worker_id,
                            "helper_id": Uuid::new_v4(), "manifest_digest": "a".repeat(64),
                            "changed_files": 0, "truncated": false, "changes": []
                        }),
                    ))
                    .unwrap();
            }
            fs::write(base.root.path().join("allow-fixture-cleanup"), b"").unwrap();
        } else {
            handle
                .block_on(base.writer().append_command(
                    operation,
                    cancel,
                    LocalCommandRequestV1::CancelRequested,
                ))
                .unwrap();
            eprintln!("cancelled_recovery_fixture phase=cancel_ready");
            assert!(base.store.worker_usage().unwrap().is_empty());
        }
        drop(execution);
        if !full3_worker_claim {
            fs::write(base.root.path().join("calls"), b"").unwrap();
        }
        Self {
            base,
            _peer: peer,
            executor,
            runtime,
            operation,
            cancel,
            worker_id: worker.worker_id,
            inspect,
        }
    }

    pub(super) fn run_sync(
        &self,
        driver: &tokio::runtime::Runtime,
    ) -> std::result::Result<(), BatchFailure> {
        driver.block_on(self.run_fresh())
    }

    /// The caller must keep the runtime driving I/O while this blocking task runs.
    /// The cold seed does so by awaiting its blocking task on the same runtime.
    pub(super) fn run_with_handle(
        &self,
        driver: &tokio::runtime::Handle,
    ) -> std::result::Result<(), BatchFailure> {
        driver.block_on(self.run_fresh())
    }

    async fn run_fresh(&self) -> std::result::Result<(), BatchFailure> {
        // A new authenticated context and Execution; no old execution survives.
        let context = self.context().await.map_err(|_| BatchFailure)?;
        eprintln!("cancelled_recovery_run phase=context_ready");
        eprintln!("cancelled_recovery_run phase=executor_enter");
        let result = Box::pin(self.executor.run(self.operation, &context))
            .await
            .map(|_| ());
        eprintln!("cancelled_recovery_run phase=executor_complete");
        result
    }
    pub(super) async fn run_invalidated_during_cleanup_authority(
        &self,
    ) -> std::result::Result<(), BatchFailure> {
        let context = self.context().await.unwrap();
        let root = self.base.root.path();
        let pause = root.join("pause-cleanup-authority");
        let ready = root.join("cleanup-authority-ready");
        fs::write(&pause, b"").unwrap();
        let run = Box::pin(self.executor.run(self.operation, &context));
        let invalidate = async {
            tokio::time::timeout(Duration::from_secs(10), async {
                while !ready.is_file() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("fixture did not reach final cleanup authority check");
            assert_eq!(
                context.cancel_generation().invalidate(),
                context.expected_generation() + 1
            );
            fs::remove_file(&pause).unwrap();
        };
        let (result, ()) = tokio::join!(run, invalidate);
        result.map(|_| ())
    }

    async fn context(&self) -> Result<AuthenticatedBatchContext> {
        AuthenticatedBatchContext::for_executor_test(
            self.base.client.clone(),
            self.base.scope.clone(),
            self.base.config.clone(),
            self.runtime.clone(),
        )
        .await
    }
    pub(super) async fn record_fixture_cleanup(&self, removed: bool) {
        let context = self.context().await.unwrap();
        let execution = Execution::open(&self.executor, self.operation, &context)
            .await
            .unwrap();
        boxed_execution_phase(|| {
            execution.mutate(WorkerTransitionV1::CleanupResult {
                removed,
                error: (!removed).then(|| "fixture cleanup was not confirmed".into()),
            })
        })
        .await
        .unwrap();
    }
    fn job(&self) -> AcceptedJobV1 {
        self.base.journal().load(self.operation).unwrap().unwrap()
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
        let job = self.base.journal().load(self.operation).unwrap().unwrap();
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
    pub(super) fn completed_cleanup_receipts(&self) -> Vec<MutationReceiptV1> {
        let job = self.job();
        let worker = self.worker();
        job.broker_checkpoint_v2()
            .unwrap()
            .supervision()
            .operations()
            .iter()
            .filter_map(|operation| match operation {
                Operation::WorkerMutation {
                    request,
                    response: Some(response),
                } if request.worker_id == worker.worker_id
                    && request.attempt_id == worker.attempt.attempt_id
                    && matches!(
                        request.transition,
                        WorkerTransitionV1::CleanupResult {
                            removed: true,
                            error: None
                        }
                    ) =>
                {
                    Some(response.receipt.clone())
                }
                _ => None,
            })
            .collect()
    }
    pub(super) fn cancel_id(&self) -> Uuid {
        self.cancel
    }
    pub(super) fn cancel_present(&self) -> bool {
        self.base
            .journal()
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
        let job = self.base.journal().load(self.operation).unwrap().unwrap();
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
        match self.base.journal().load(self.operation) {
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
        write_observation_fixture(self.base.root.path(), &self.inspect, case);
    }
}

/// Store-acknowledged inputs for service projection only. No cleanup command
/// or further runtime observation is issued for these two recorded outcomes.
pub(crate) async fn terminal_cleanup_jobs_for_test() -> [AcceptedJobV1; 3] {
    let fixture = RecoveryFixture::new(Case::TerminalCleanupRecovery).await;
    let absent = fixture.job();
    fixture.record_fixture_cleanup(false).await;
    let unconfirmed = fixture.job();
    fixture.record_fixture_cleanup(true).await;
    [absent, unconfirmed, fixture.job()]
}

fn write_observation_fixture(root: &Path, initial: &Value, case: Case) {
    let mut inspect = initial.clone();
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
    fs::write(root.join("missing"), if missing { "yes" } else { "no" }).unwrap();
    fs::write(
        root.join("inspect.json"),
        serde_json::to_vec(&inspect).unwrap(),
    )
    .unwrap();
}

/// Opaque descriptor for the real committed-terminal/pending-V4 fixture. It
/// retains only the synthetic gateway/store and private temp directory; the
/// broker executor, full Journal, Writer actor and empty enforcer child have
/// already been closed before this value is returned.
pub(crate) struct ColdRecoverySeed {
    base: Arc<Fixture>,
    root: std::path::PathBuf,
    journal_root: std::path::PathBuf,
    database: std::path::PathBuf,
    config: std::path::PathBuf,
    scope: JournalScopeV1,
    operation: Uuid,
    cancel: Uuid,
    worker_id: String,
    attempt_id: String,
    container_id: String,
    event_id: Uuid,
    event_input_hash: String,
    coverage_event_id: Uuid,
    coverage_event_input_hash: String,
    coverage_audit_seq: Option<i64>,
    terminal: bool,
    quota_held: bool,
    allocation_count: usize,
    result_pending: bool,
    result_acknowledged: bool,
    runtime_call_snapshot: Vec<String>,
    decision_request_count: usize,
    arm_claim_snapshot: Value,
}

impl ColdRecoverySeed {
    pub(crate) async fn with_scope_for_cold_audit_test(
        installation_id: Uuid,
        profile_id: &str,
    ) -> Self {
        terminal_pending_seed_with_scope(Some((installation_id, profile_id.to_owned()))).await
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
    pub(crate) fn journal_root(&self) -> &Path {
        &self.journal_root
    }
    pub(crate) fn database(&self) -> &Path {
        &self.database
    }
    pub(crate) fn config(&self) -> &Path {
        &self.config
    }
    pub(crate) fn scope(&self) -> &JournalScopeV1 {
        &self.scope
    }
    pub(crate) fn operation_id(&self) -> Uuid {
        self.operation
    }
    pub(crate) fn cancel_id(&self) -> Uuid {
        self.cancel
    }
    pub(crate) fn worker_id(&self) -> &str {
        &self.worker_id
    }
    pub(crate) fn attempt_id(&self) -> &str {
        &self.attempt_id
    }
    pub(crate) fn container_id(&self) -> &str {
        &self.container_id
    }
    pub(crate) fn event_id(&self) -> Uuid {
        self.event_id
    }
    pub(crate) fn event_input_hash(&self) -> &str {
        &self.event_input_hash
    }
    pub(crate) fn coverage_event_id(&self) -> Uuid {
        self.coverage_event_id
    }
    pub(crate) fn coverage_event_input_hash(&self) -> &str {
        &self.coverage_event_input_hash
    }
    pub(crate) fn coverage_audit_seq(&self) -> Option<i64> {
        self.coverage_audit_seq
    }
    pub(crate) fn is_terminal(&self) -> bool {
        self.terminal
    }
    pub(crate) fn quota_held(&self) -> bool {
        self.quota_held
    }
    pub(crate) fn allocation_count(&self) -> usize {
        self.allocation_count
    }
    pub(crate) fn result_pending(&self) -> bool {
        self.result_pending
    }
    pub(crate) fn result_acknowledged(&self) -> bool {
        self.result_acknowledged
    }
    pub(crate) fn runtime_calls(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
    pub(crate) fn runtime_call_snapshot(&self) -> &[String] {
        &self.runtime_call_snapshot
    }
    pub(crate) fn decision_request_count_snapshot(&self) -> usize {
        self.decision_request_count
    }
    pub(crate) fn current_decision_request_count(&self) -> usize {
        self.base.requests.load(Ordering::SeqCst)
    }
    pub(crate) fn arm_claim_snapshot(&self) -> &Value {
        &self.arm_claim_snapshot
    }
    pub(crate) fn retained_gateway_is_live(&self) -> bool {
        !self.base.gateway.is_finished()
    }
    /// Arm a test-only barrier after a successful audit POST has committed in
    /// the retained gateway's real Store and before its HTTP response is
    /// delivered to the broker client.
    pub(crate) fn pause_next_audit_response_for_test(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        self.base.pause_next_audit_response_for_test()
    }
}

/// Build the same real V4 lost-ACK state as the existing recovery scenario,
/// then release all broker-side journal owners and verify that a fresh
/// StopRecoveryReader can acquire the exact configured profile lease.
pub(crate) async fn terminal_pending_seed_for_cold_audit_test() -> ColdRecoverySeed {
    terminal_pending_seed_with_scope(None).await
}

async fn terminal_pending_seed_with_scope(
    scope_override: Option<(Uuid, String)>,
) -> ColdRecoverySeed {
    let driver = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let fixture = match scope_override {
            Some(scope) => {
                driver.block_on(RecoveryFixture::new_for_cold_audit_test_with_scope(scope))
            }
            None => driver.block_on(RecoveryFixture::new_for_cold_audit_test()),
        };
        let before = fixture.worker();
        assert_eq!(before.state, WorkerStateV1::Running);
        assert!(before.quota_held);
        let cancel_id = fixture.cancel_id();
        let worker_id = before.worker_id.clone();
        let attempt_id = before.attempt.attempt_id.clone();
        let container_id = before
            .attempt
            .container_id
            .clone()
            .expect("seeded Running worker has a retained CID");

        fixture.arm_transition_response_cut();
        let first = fixture.run_with_handle(&driver);
        assert!(first.is_err(), "the committed terminal response is cut");
        assert!(!fixture.transition_response_cut_pending());
        eprintln!("cold_recovery_seed phase=cut_return_verified");
        let worker = fixture.worker();
        assert_eq!(worker.state, WorkerStateV1::Terminal);
        assert!(!worker.quota_held);
        assert_eq!(worker.worker_id, worker_id);
        assert_eq!(worker.attempt.attempt_id, attempt_id);
        assert_eq!(
            worker.attempt.container_id.as_deref(),
            Some(container_id.as_str())
        );
        eprintln!("cold_recovery_seed phase=terminal_store_verified");
        assert!(fixture.cancel_present());
        assert_eq!(fixture.cancel_id(), cancel_id);
        eprintln!("cold_recovery_seed phase=cancel_journal_verified");
        let usage = fixture.usage();
        assert_eq!(usage.len(), 1);
        assert!(!usage[0].billable);
        eprintln!("cold_recovery_seed phase=allocation_verified");

        let publication = fixture.result_publication_snapshot();
        assert!(!publication.output_acknowledged);
        assert_eq!(publication.result_outbox.len(), 1);
        let (event_id, audit_seq) = publication.result_outbox[0];
        assert_eq!(audit_seq, None);
        assert!(publication.stored_result_events.is_empty());
        eprintln!("cold_recovery_seed phase=result_pending_verified");
        let job = fixture
            .base
            .journal()
            .load(fixture.operation)
            .unwrap()
            .unwrap();
        let v4_events = job.broker_checkpoint_v4().unwrap().events();
        let result_events = v4_events
            .iter()
            .filter(|event| event.event().kind == "broker.batch.result")
            .collect::<Vec<_>>();
        assert_eq!(result_events.len(), 1, "one result event is pending");
        let pending_result = result_events[0];
        assert_eq!(pending_result.event_id(), event_id);
        assert_eq!(pending_result.audit_seq(), None);
        let event_input_hash = pending_result.input_hash().to_owned();
        let coverage_events = v4_events
            .iter()
            .filter(|event| event.event().kind == "observer.coverage")
            .collect::<Vec<_>>();
        assert_eq!(coverage_events.len(), 1, "coverage is a separate event");
        let acknowledged_coverage = coverage_events[0];
        let coverage_audit_seq = acknowledged_coverage.audit_seq();
        let coverage_event_id = acknowledged_coverage.event_id();
        let coverage_event_input_hash = acknowledged_coverage.input_hash().to_owned();
        assert_eq!(
            coverage_audit_seq,
            None,
            "coverage is genuinely pending and must not be acknowledged by fixture setup"
        );
        let pending_v4_ids = v4_events
            .iter()
            .filter(|entry| entry.audit_seq().is_none())
            .map(|entry| (entry.event().kind.as_str(), entry.event_id()))
            .collect::<Vec<_>>();
        assert_eq!(
            pending_v4_ids,
            vec![
                ("observer.coverage", coverage_event_id),
                ("broker.batch.result", event_id),
            ],
            "the complete V4 pending set must be the observed coverage/result pair; do not alter either event to force the fixture"
        );
        eprintln!("cold_recovery_seed phase=v4_outbox_verified");
        let runtime_call_snapshot = fixture.runtime_calls();
        let decision_request_count = fixture.base.requests.load(Ordering::SeqCst);
        let arm_claim_snapshot = fixture.arm_claim_snapshot();
        eprintln!("cold_recovery_seed phase=arm_claim_snapshot_ready");

        let base = fixture.base.clone();
        let root = base.root.path().to_owned();
        let journal_root = root.join("journals").join(base.scope.profile_id());
        let database = root.join("audit.sqlite");
        let config = base.config.path().to_owned();
        let scope = base.scope.clone();
        let usage_count = usage.len();
        let operation = fixture.operation;
        let result_pending = !publication.output_acknowledged
            && publication.result_outbox.len() == 1
            && publication.result_outbox[0].1.is_none();
        drop(base);

        let retained_base = close_cold_seed_owners(fixture, &driver, &journal_root, &scope);
        ColdRecoverySeed {
            base: retained_base,
            root,
            journal_root,
            database,
            config,
            scope,
            operation,
            cancel: cancel_id,
            worker_id,
            attempt_id,
            container_id,
            event_id,
            event_input_hash,
            coverage_event_id,
            coverage_event_input_hash,
            coverage_audit_seq,
            terminal: worker.state == WorkerStateV1::Terminal,
            quota_held: worker.quota_held,
            allocation_count: usage_count,
            result_pending,
            result_acknowledged: publication.output_acknowledged,
            runtime_call_snapshot,
            decision_request_count,
            arm_claim_snapshot,
        }
    })
    .await
    .expect("cold recovery seed task must join")
}

/// The original loopback Store and its confirmed Running/Cancel prefix,
/// held before the terminal mutation sequence of the cold audit seed.
pub(crate) struct RunningColdRecoverySeed {
    base: Arc<Fixture>,
    pub(crate) journal_root: std::path::PathBuf,
    pub(crate) operation_id: Uuid,
    pub(crate) cancel_id: Uuid,
    pub(crate) worker_id: String,
}

impl RunningColdRecoverySeed {
    pub(crate) fn root(&self) -> &Path {
        self.base.root.path()
    }

    pub(crate) fn config(&self) -> &Path {
        self.base.config.path()
    }

    pub(crate) fn scope(&self) -> &JournalScopeV1 {
        &self.base.scope
    }

    pub(crate) fn store(&self) -> &crate::audit::Store {
        &self.base.store
    }

    pub(crate) fn worker_get_count(&self) -> usize {
        self.base.worker_gets.load(Ordering::SeqCst)
    }

    pub(crate) fn post_count(&self) -> usize {
        self.base.posts.load(Ordering::SeqCst)
    }

    pub(crate) fn arm_transition_response_cut_for_test(&self) {
        self.base
            .drop_next_worker_transition_response
            .store(true, Ordering::SeqCst);
    }

    pub(crate) fn transition_response_cut_pending_for_test(&self) -> bool {
        self.base
            .drop_next_worker_transition_response
            .load(Ordering::SeqCst)
    }
}

pub(crate) async fn running_seed_for_cold_terminal_test() -> RunningColdRecoverySeed {
    let driver = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let fixture = RecoveryFixture::seed(Case::StillRunning, &driver, true, None);
        let worker = fixture.worker();
        assert_eq!(worker.state, WorkerStateV1::Running);
        assert!(worker.quota_held && fixture.cancel_present());
        let operation_id = fixture.operation;
        let cancel_id = fixture.cancel_id();
        let scope = fixture.base.scope.clone();
        let journal_root = fixture
            .base
            .root
            .path()
            .join("journals")
            .join(scope.profile_id());
        let base = close_cold_seed_owners(fixture, &driver, &journal_root, &scope);
        RunningColdRecoverySeed {
            base,
            journal_root,
            operation_id,
            cancel_id,
            worker_id: worker.worker_id,
        }
    })
    .await
    .expect("cold Running recovery seed task must join")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full3_worker_claim_and_stop_survive_journal_reopen() {
    use crate::broker::journal::canonical;

    let driver = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let fixture = RecoveryFixture::seed(Case::Full3WorkerClaim, &driver, true, None);
        let job = fixture.job();
        let checkpoint = job.broker_checkpoint_v5().expect("confirmed V5 checkpoint");
        let [binding] = checkpoint.bindings() else {
            panic!("one native Full3 Worker binding");
        };
        let operations = checkpoint
            .outbox()
            .lifecycle()
            .broker()
            .supervision()
            .operations();
        let Some(Operation::WorkerClaim {
            request: claim,
            response: Some(acknowledged),
        }) = operations.last()
        else {
            panic!("the last supervision operation is an acknowledged Worker claim");
        };
        let scope = job.scope().clone();
        let operation = job.operation_id();
        let head = job.head().clone();
        let cancel = fixture.cancel_id();
        let before = fixture.worker();
        assert!(before.quota_held);
        assert_eq!(before.state, WorkerStateV1::StartIssued);
        assert_eq!(before.observed_started_at, None);
        assert!(canonical(&before).unwrap() == canonical(&acknowledged.worker).unwrap());
        assert_eq!(acknowledged.disposition, Some(ClaimDispositionV1::EmitOnce));
        assert_eq!(acknowledged.receipt.operation_id, claim.operation_id);
        assert_eq!(acknowledged.receipt.input_hash, claim.input_hash().unwrap());
        assert!(acknowledged.receipt.audit_seq > claim.arm_receipt.audit_seq);
        let binding_bytes = canonical(binding).unwrap();
        let arm_receipt_bytes = canonical(&claim.arm_receipt).unwrap();
        let (arm, owned) = operations
            .iter()
            .find_map(|operation| match operation {
                Operation::Arm {
                    request,
                    owned,
                    response: Some(_),
                    ..
                } if request.operation_id == claim.arm_receipt.arm_id => Some((request, owned)),
                _ => None,
            })
            .expect("acknowledged Arm for the confirmed claim");
        let target_bytes = canonical(&(arm, owned)).unwrap();
        drop(job);
        let calls = fixture.runtime_calls();
        assert!(!calls.is_empty());
        assert!(
            calls.iter().all(|call| {
                call == "info" || call == "image inspect" || call.starts_with("inspect ")
            }),
            "only the synthetic fixture's original inspection ran"
        );
        let journal_root = fixture
            .base
            .root
            .path()
            .join("journals")
            .join(scope.profile_id());
        let base = close_cold_seed_owners(fixture, &driver, &journal_root, &scope);
        let profile = Directory::open_existing(&journal_root).unwrap();
        let report = StopRecoveryReaderV1::open_existing(
            &profile,
            scope.clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(Vec::new()).unwrap(),
        )
        .unwrap()
        .unwrap()
        .scan();
        assert!(!report.incomplete());
        assert!(canonical(report.scope()).unwrap() == canonical(&scope).unwrap());
        let [recovered] = report.jobs() else {
            panic!("one recovered job");
        };
        assert_eq!(recovered.operation_id(), operation);
        // The same verified head commits the WorkerClaim ACK and the later STOP.
        assert_eq!(recovered.head(), &head);
        let mut worker_stops = recovered
            .stops()
            .iter()
            .filter(|target| matches!(&target.arm().subject, ArmSubjectV1::Worker));
        let target = worker_stops.next().expect("recovered Worker Arm");
        assert!(worker_stops.next().is_none());
        assert_eq!(target.cancel_after_arm(), Some(cancel));
        assert!(canonical(target.receipt()).unwrap() == arm_receipt_bytes);
        assert!(canonical(target.full3_registration_binding().unwrap()).unwrap() == binding_bytes);
        assert!(canonical(&(target.arm(), target.owned())).unwrap() == target_bytes);
        assert_eq!(
            target.owned().handle.id(),
            target.arm().inspection.container_id
        );
        assert_eq!(target.owned().observation.process.started_at, None);
        assert!(base.store.worker_usage().unwrap().is_empty());
        let current = base.store.worker(&before.worker_id).unwrap().unwrap();
        assert!(canonical(&current).unwrap() == canonical(&before).unwrap());
    })
    .await
    .expect("Full3 claim seed and cold journal read must join");
}

async fn seed_full3_worker_claim(
    execution: &Execution<'_>,
    arm: &ArmV1,
    owned: &OwnedContainerSnapshot,
    authority: &RuntimeAuthority,
    mut claim: ArmedStartClaimV1,
) -> Result<()> {
    let scope = execution.context.scope();
    let registration = RegistrationV1::new(
        hex::encode(Sha256::digest(scope.gateway_origin().as_bytes())),
        scope.tenant_id().into(),
        arm.clone(),
        claim.arm_receipt.clone(),
        authority.clone(),
        owned.clone(),
    )?;
    let registration_hash = registration.digest()?;
    let (native, client, slot) = OriginalUnusedFixture::register_with_generation(
        scope.installation_id(),
        registration,
        execution.context.cancel_generation().clone(),
    )
    .await?;
    let claimed = boxed_execution_phase(|| async {
        let binding = slot.full3_registration_binding()?;
        ensure!(
            binding.arm_id() == Uuid::parse_str(&arm.operation_id)?
                && binding.registration_hash().as_str() == registration_hash
                && binding.original_epoch() == client.epoch()
                && binding.ledger().installation_id() == &scope.installation_id(),
            "Full3 Worker registration returned a different binding"
        );
        let (job, _) = execution.load().await?;
        execution
            .executor
            .writer
            .bind_full3_registration(execution.operation, job.head().clone(), binding)
            .await?;
        let reservation = slot.reserve_claim(execution.generation)?;
        slot.consume_claim_reservation(reservation, arm, execution.generation)?;
        claim.clock_check = NativeHostClock::new()?.sample()?;
        let (operation, fresh) = execution
            .rpc_checked(
                Operation::WorkerClaim {
                    request: claim,
                    response: None,
                },
                Some(&slot),
            )
            .await?;
        ensure!(
            fresh && super::super::emits_once(&operation),
            "Full3 Worker claim did not obtain the first SQLite acknowledgement"
        );
        // The test stops at the durable claim, before Prepare or any Start.
        Ok(())
    })
    .await;
    drop(slot);
    drop(client);
    native.finish().await;
    claimed
}

fn close_cold_seed_owners(
    fixture: RecoveryFixture,
    driver: &tokio::runtime::Handle,
    journal_root: &Path,
    scope: &JournalScopeV1,
) -> Arc<Fixture> {
    // Retain the actual loopback gateway and release the full journal lease.
    let RecoveryFixture {
        _peer,
        executor,
        base: mut retained_base,
        ..
    } = fixture;
    driver.block_on(_peer.close());
    eprintln!("cold_recovery_seed phase=native_owner_closed");
    drop(executor);
    Arc::get_mut(&mut retained_base)
        .expect("cold seed must have one fixture owner")
        .close_journal_owners_for_cold_test();
    eprintln!("cold_recovery_seed phase=full_journal_owners_dropped");

    let profile = Directory::open_existing(journal_root).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        match StopRecoveryReaderV1::open_existing(
            &profile,
            scope.clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(Vec::new()).unwrap(),
        ) {
            Ok(Some(reader)) => {
                drop(reader);
                break;
            }
            Ok(None) => panic!("cold journal profile disappeared"),
            Err(crate::broker::journal::JournalError::Busy) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "writer lease did not release within the bounded cold-close wait"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("cold StopRecoveryReader reopen failed: {error}"),
        }
    }

    eprintln!("cold_recovery_seed phase=stop_reader_reopened");
    retained_base
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

async fn seed_helper(
    execution: &Execution<'_>,
    worker: &WorkerV1,
    authority: &RuntimeAuthority,
    intent: &str,
    purpose: HelperPurposeV1,
    permission: HelperPermissionV1,
    verification: Option<ImportVerificationReportV1>,
) -> Result<(Uuid, ArmReceiptV1, MutationReceiptV1)> {
    let helper = Uuid::new_v4();
    let policy = helper_execution_policy_v1(worker, purpose, helper)?;
    execution
        .patch(|draft| {
            draft.helpers.push(HelperPreparationV1::new(
                policy.clone(),
                permission.clone(),
                None,
            )?);
            Ok(())
        })
        .await?;
    let inspect = RuntimeInspectionV1 {
        authority_id: policy.authority_id.clone(),
        runtime_kind: policy.runtime_kind,
        container_id: format!("{:064x}", helper.as_u128()),
        stable_name: policy.stable_name.clone(),
        volume_name: policy.volume_name.clone(),
        image_id: policy.image_id.clone(),
        spec_hash: policy.spec_hash()?,
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
    }))?;
    execution
        .patch(|draft| {
            let entry = draft
                .helpers
                .last_mut()
                .context("fixture helper preparation missing")?;
            *entry =
                HelperPreparationV1::new(policy.clone(), permission.clone(), Some(owned.clone()))?;
            Ok(())
        })
        .await?;
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
    let (armed, _) = execution
        .rpc(Operation::Arm {
            parent: worker.clone(),
            owned,
            request: arm.clone(),
            response: None,
        })
        .await?;
    let Operation::Arm {
        response: Some(armed),
        ..
    } = armed
    else {
        anyhow::bail!("fixture helper arm missing")
    };
    execution
        .rpc(Operation::HelperClaim {
            request: HelperStartClaimV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                helper_id: helper.to_string(),
                emitter_id: Uuid::new_v4().to_string(),
                arm_receipt: armed.receipt.clone(),
                clock_check: arm.clock,
            },
            response: None,
        })
        .await?;
    let (terminal, _) = execution
        .rpc(Operation::HelperObservation {
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
        })
        .await?;
    let Operation::HelperObservation {
        response: Some(receipt),
        ..
    } = terminal
    else {
        anyhow::bail!("fixture helper terminal missing")
    };
    Ok((helper, armed.receipt, receipt))
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
use std::{fmt, fs, io::Write};

fn log(calls: &mut fs::File, line: fmt::Arguments<'_>) {
    // Append the whole record in one write because fixture processes share this log.
    let record = format!("{line}\n");
    let written = calls.write(record.as_bytes()).unwrap();
    assert_eq!(written, record.len());
}

fn main() {
    let root = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_owned();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let start = if args.first().map(String::as_str) == Some("--host")
        && args.get(1).map(String::as_str) == Some("unix:///cancel-recovery-fixture.sock")
    { 2 } else { 0 };
    let args = &args[start..];
    let mut calls = fs::OpenOptions::new().append(true).create(true).open(root.join("calls")).unwrap();
    match args.first().map(String::as_str) {
        Some("info") => {
            log(&mut calls, format_args!("info"));
            let pause = root.join("pause-cleanup-authority");
            if pause.is_file() && fs::read_to_string(root.join("calls")).unwrap()
                .lines().filter(|line| line.starts_with("inspect ")).count() == 2
            {
                fs::write(root.join("cleanup-authority-ready"), b"").unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while pause.is_file() {
                    if std::time::Instant::now() >= deadline {
                        std::process::exit(125);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
            println!("{{\"ID\":\"cancel-recovery-synthetic-engine\",\"SecurityOptions\":[\"name=rootless\"]}}");
        }
        Some("image") if args.len() == 5 && args[1..4] == ["inspect", "--format", "{{.Id}}"]
            && args[4] == format!("sha256:{}", "a".repeat(64)) => {
            log(&mut calls, format_args!("image inspect"));
            println!("{}", args[4]);
        }
        Some("inspect") => {
            log(&mut calls, format_args!("inspect {}", args.last().unwrap()));
            if fs::read_to_string(root.join("missing")).is_ok_and(|v| v == "yes") {std::process::exit(125)}
            print!("{}",fs::read_to_string(root.join("inspect.json")).unwrap());
        }
        Some("rm") if args.len() == 2 && args[1] == "d".repeat(64)
            && root.join("allow-fixture-cleanup").is_file() => {
            log(&mut calls, format_args!("rm {}", args[1]));
            if fs::read_to_string(root.join("missing")).is_ok_and(|v| v == "yes") {std::process::exit(125)}
            fs::write(root.join("missing"), "yes").unwrap();
        }
        _ => { log(&mut calls, format_args!("{}", args.join(" "))); std::process::exit(125); }
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
