use super::*;
use crate::{
    broker_registry::{BrokerAuthorityV1, BrokerGrantMutationV1},
    control::{
        AdmissionV1, DecisionPhaseV1, DecisionReceiptV1, LaunchSpecV1, NetworkV1, QuotaPolicyV1,
        RestartPolicyV1, RuntimeInspectionV1, SecurityPresetV1, WorkerMutationV1, WorkerStateV1,
        WorkerTransitionV1, WorkerV1,
        session_control::{LegacySessionTargetV1, SessionLaunchTargetRecordV1},
    },
    identity::{
        HumanAccountV1, IdentityChangeV1, IdentityMutationV1, LocalCredentialV1, MembershipV1,
    },
    model::{CommandSpec, DecisionRequest},
    policy::{self, Policy},
    workload::{WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use sha2::{Digest, Sha256};
use std::{os::unix::fs::PermissionsExt, path::Path};

const MAX_EVENTS: u64 = 128;
const DEADLINE: &str = "2126-01-01T00:00:00Z";

struct Fixture {
    directory: tempfile::TempDir,
    store: Store,
    scope: AuthorizedScopeV1,
    credential_id: String,
    digest: [u8; 32],
    target_id: Uuid,
    worker: WorkerV1,
}

impl Fixture {
    fn open(path: &Path, redactor: Redactor) -> Store {
        Store::open_existing_for_tenant(path, "tenant-a", redactor, MAX_EVENTS).unwrap()
    }

    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = directory
            .path()
            .canonicalize()
            .unwrap()
            .join("legacy.sqlite");
        let store =
            Store::open_for_tenant(&path, "tenant-a", Redactor::new(vec![]), MAX_EVENTS).unwrap();
        let target_id = Uuid::new_v4();
        let launch = LaunchSpecV1 {
            version: 1,
            session_id: target_id.to_string(),
            command: CommandSpec {
                argv: vec!["/usr/bin/id".into(), "-u".into()],
                cwd: "/workspace".into(),
            },
            profile: "observe".into(),
            runtime_kind: RuntimeKindV1::Docker,
            authority_id: Uuid::new_v4().to_string(),
            image_ref: "worker:fixture".into(),
            image_id: format!("sha256:{}", "a".repeat(64)),
            network: NetworkV1::None,
            cpu_millis: 2000,
            memory_bytes: 1_073_741_824,
            pids_limit: 256,
            timeout_seconds: 60,
            security_preset: SecurityPresetV1::FixedSandboxV2,
            restart_policy: RestartPolicyV1::No,
            workspace_manifest_digest: "b".repeat(64),
            workspace_import_policy_version: 1,
            provider: "generic".into(),
            capture_output: false,
            credential_binding_id: None,
            credential_names: vec![],
            provider_resolution: None,
            egress: None,
        };
        let worker = historical_created_worker(&store, launch.clone());
        for (index, migrate) in [
            Store::migrate_control_v3
                as fn(&Store, &Path) -> anyhow::Result<crate::control::MigrationReportV1>,
            Store::migrate_identity_v4,
            Store::migrate_human_access_v5,
            Store::migrate_workload_v6,
            Store::migrate_human_brokers_v7,
            Store::migrate_kubernetes_create_v8,
            Store::migrate_kubernetes_policy_v9,
            Store::migrate_human_workers_v10,
            Store::migrate_human_rights_v2,
            Store::enable_legacy_session_prepare_v1,
        ]
        .into_iter()
        .enumerate()
        {
            migrate(&store, &path.with_extension(format!("before-{index}"))).unwrap();
        }
        let actor = PrincipalRefV1::LegacyControl {
            credential_id: "fixture-control".into(),
            role: LegacyControlRoleV1::Runner,
        };
        let principal = PrincipalRefV1::HumanLocal {
            installation_id: Uuid::new_v4().to_string(),
            account_id: Uuid::new_v4().to_string(),
        };
        let credential_id = Uuid::new_v4().to_string();
        let membership_id = Uuid::new_v4().to_string();
        let digest: [u8; 32] = Sha256::digest(b"synthetic local authentication fixture").into();
        let changes = [
            IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Local fixture".into(),
                    revision: 1,
                    active: true,
                },
            },
            IdentityChangeV1::Credential {
                expected_revision: 0,
                binding: LocalCredentialV1 {
                    version: 1,
                    credential_id: credential_id.clone(),
                    principal: principal.clone(),
                    sha256_digest: hex::encode(digest),
                    revision: 1,
                    active: true,
                    expires_at: None,
                },
            },
            IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: MembershipV1 {
                    version: 1,
                    membership_id: membership_id.clone(),
                    principal: principal.clone(),
                    tenant_id: "tenant-a".into(),
                    organization_scope: "fixture-org".into(),
                    revision: 1,
                    active: true,
                    capabilities: [HumanCapabilityV1::LaunchSession].into_iter().collect(),
                },
            },
        ];
        for change in changes {
            store
                .mutate_identity(&IdentityMutationV1 {
                    version: 1,
                    operation_id: Uuid::new_v4().to_string(),
                    actor: actor.clone(),
                    change,
                })
                .unwrap();
        }
        let broker_id = Uuid::new_v4();
        let incarnation_id = Uuid::new_v4();
        let workload_id = Uuid::new_v4().to_string();
        store
            .issue_workload_binding(&WorkloadBindingIssueV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                credential_id: workload_id.clone(),
                expected_revision: 0,
                subject: WorkloadSubjectV1::Broker {
                    broker_id: broker_id.to_string(),
                    incarnation_id: incarnation_id.to_string(),
                },
                sha256_digest: hex::encode(Sha256::digest(
                    b"synthetic broker authentication fixture",
                )),
                expires_at: DEADLINE.into(),
            })
            .unwrap();
        store
            .mutate_broker_grant(&BrokerGrantMutationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                broker_id: broker_id.to_string(),
                expected_revision: None,
                credential_id: workload_id,
                incarnation_id: incarnation_id.to_string(),
                authorities: vec![BrokerAuthorityV1 {
                    authority_id: launch.authority_id.clone(),
                    runtime_kind: BrokerRuntimeKindV1::Docker,
                }],
                lease_seconds: 120,
                not_after: DEADLINE.into(),
            })
            .unwrap();
        store
            .provision_legacy_session_target(
                actor,
                ProvisionLegacySessionTargetV1 {
                    expected_revision: 0,
                    target: LegacySessionTargetV1 {
                        version: 1,
                        catalogue: SessionLaunchTargetRecordV1 {
                            version: 1,
                            launch_target_id: target_id,
                            revision: 1,
                            tenant_id: "tenant-a".into(),
                            organization_scope: "fixture-org".into(),
                            active: true,
                            launchers: vec![principal.clone()],
                        },
                        broker_id,
                        incarnation_id,
                        broker_grant_revision: 1,
                        broker_profile_id: "fixture-profile".into(),
                        launch,
                    },
                },
            )
            .unwrap();
        let scope = store
            .authorize_human_context(&principal, &membership_id, 1, ContextModeV1::Personal)
            .unwrap();
        Self {
            directory,
            store,
            scope,
            credential_id,
            digest,
            target_id,
            worker,
        }
    }

    fn proof(&self) -> LegacySessionLocalProof<'_> {
        LegacySessionLocalProof {
            scope: &self.scope,
            digest: &self.digest,
            credential_id: &self.credential_id,
            credential_revision: 1,
            context_expires_at: DEADLINE,
        }
    }

    fn request(&self) -> SessionLaunchPrepareRequestV1 {
        SessionLaunchPrepareRequestV1 {
            version: 1,
            operation_id: Uuid::new_v4(),
            launch_target_id: self.target_id,
            expected_target_revision: 1,
            expires_at: DEADLINE.into(),
        }
    }
}

fn mutation(worker: &WorkerV1, transition: WorkerTransitionV1) -> WorkerMutationV1 {
    WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition,
    }
}

fn historical_created_worker(store: &Store, mut launch: LaunchSpecV1) -> WorkerV1 {
    // A software fixture traverses the supported Store2 transitions before
    // migration. No process, claim or engine is involved.
    launch.session_id = Uuid::new_v4().to_string();
    let input = DecisionRequest {
        request_id: Uuid::new_v4().to_string(),
        profile: launch.profile.clone(),
        command: launch.command.clone(),
        project_id: None,
        sandbox_id: Some(launch.session_id.clone()),
        agent_id: None,
        session_id: Some(launch.session_id.clone()),
        correlation_id: None,
        trace_id: None,
        human_approved: false,
        approval_hash: None,
        execution_mode: None,
        provider_resolution: None,
    };
    let policy = Policy::default();
    let decision = policy::evaluate(&policy, &input);
    let jev = serde_json::Value::Null;
    let stored = store
        .decision(&input, Some((decision.clone(), jev.clone())))
        .unwrap()
        .unwrap();
    let admission = AdmissionV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        receipt: DecisionReceiptV1 {
            version: 1,
            request_id: input.request_id.clone(),
            request_hash: policy::request_hash(&input),
            phase: DecisionPhaseV1::Initial,
            audit_seq: stored.seq,
            audit_hash: stored.hash,
        },
        decision_request: input,
        launch,
    };
    let worker = store
        .admit_worker(
            &admission,
            &policy,
            decision,
            jev,
            &QuotaPolicyV1::default(),
        )
        .unwrap()
        .worker
        .unwrap();
    let worker = store
        .mutate_worker(&mutation(&worker, WorkerTransitionV1::Prepare))
        .unwrap()
        .worker;
    let worker = store
        .mutate_worker(&mutation(&worker, WorkerTransitionV1::CreateIssued))
        .unwrap()
        .worker;
    let inspection = RuntimeInspectionV1 {
        authority_id: worker.launch.authority_id.clone(),
        runtime_kind: worker.launch.runtime_kind,
        container_id: "c".repeat(64),
        stable_name: worker.attempt.stable_name.clone(),
        volume_name: worker.attempt.volume_name.clone(),
        image_id: worker.launch.image_id.clone(),
        spec_hash: worker.spec_hash.clone(),
        cpu_millis: worker.launch.cpu_millis,
        memory_bytes: worker.launch.memory_bytes,
        pids_limit: worker.launch.pids_limit,
        network: worker.launch.network,
        security_preset: worker.launch.security_preset,
        restart_policy: worker.launch.restart_policy,
    };
    store
        .mutate_worker(&mutation(
            &worker,
            WorkerTransitionV1::Created { inspection },
        ))
        .unwrap()
        .worker
}

#[test]
fn legacy_prepare_and_confirm_leave_the_stop_audit_reserve_available() {
    let f = Fixture::new();
    let prepared = f
        .store
        .legacy_session_prepare_local(f.proof(), f.request())
        .unwrap();
    let confirmation = SessionLaunchRequestV1 {
        version: 1,
        operation_id: Uuid::new_v4(),
        launch_target_id: f.target_id,
        expected_target_revision: 1,
        preparation_operation_id: prepared.operation_id,
        preparation_input_hash: prepared.input_hash,
    };
    let policy = f.store.capacity_policy();
    let ordinary_limit =
        MAX_EVENTS - policy.control_reserve.events - policy.identity_reserve.events;
    for _ in f.store.integrity().unwrap().event_count..ordinary_limit - 2 {
        f.store.append(Event::new("fixture.capacity")).unwrap();
    }
    let head = f.store.head().unwrap();
    let result = f
        .store
        .legacy_session_confirm_local(f.proof(), confirmation);
    let refusal = result.unwrap_err();
    assert_eq!(
        refusal.downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted)
    );
    assert_eq!(
        f.store.head().unwrap(),
        head,
        "partial confirmation and fences must roll back"
    );
    for _ in 0..2 {
        f.store
            .legacy_session_prepare_local(f.proof(), f.request())
            .unwrap();
    }
    let refusal = f
        .store
        .legacy_session_prepare_local(f.proof(), f.request())
        .unwrap_err();
    assert_eq!(
        refusal.downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted)
    );
    assert_eq!(f.store.integrity().unwrap().event_count, ordinary_limit);
    let stop = f
        .store
        .mutate_worker(&mutation(&f.worker, WorkerTransitionV1::Stop))
        .unwrap();
    assert_eq!(stop.worker.state, WorkerStateV1::Stopping);
    assert_eq!(f.store.head().unwrap(), stop.receipt.audit_seq);
    assert_eq!(f.store.integrity().unwrap().event_count, ordinary_limit + 1);
}

#[test]
fn legacy_confirmation_keeps_its_shorter_deadline_after_reopen_and_replay() {
    let mut f = Fixture::new();
    let prepared = f
        .store
        .legacy_session_prepare_local(f.proof(), f.request())
        .unwrap();
    let request = SessionLaunchRequestV1 {
        version: 1,
        operation_id: Uuid::new_v4(),
        launch_target_id: f.target_id,
        expected_target_revision: 1,
        preparation_operation_id: prepared.operation_id,
        preparation_input_hash: prepared.input_hash,
    };
    let prepared_revision = f.scope.authorization_revision();
    f.store
        .mutate_identity(&IdentityMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            actor: PrincipalRefV1::LegacyControl {
                credential_id: "fixture-control".into(),
                role: LegacyControlRoleV1::Runner,
            },
            change: IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: PrincipalRefV1::HumanLocal {
                        installation_id: Uuid::new_v4().to_string(),
                        account_id: Uuid::new_v4().to_string(),
                    },
                    display_name: "Unrelated account".into(),
                    revision: 1,
                    active: true,
                },
            },
        })
        .unwrap();
    f.scope = f
        .store
        .authorize_human_context(
            f.scope.principal(),
            f.scope.membership_id(),
            f.scope.membership_revision(),
            ContextModeV1::Personal,
        )
        .unwrap();
    assert_eq!(f.scope.authorization_revision(), prepared_revision + 1);
    let confirmation_deadline = "2124-01-01T00:00:00.123456789Z";
    let proof = LegacySessionLocalProof {
        context_expires_at: confirmation_deadline,
        ..f.proof()
    };
    let confirmation = f
        .store
        .legacy_session_confirm_local(proof, request.clone())
        .unwrap();
    let query = crate::audit::EventQuery {
        kind: Some("control.legacy_session_prepare.confirmed".into()),
        ..Default::default()
    };
    let records = f.store.events(&query).unwrap();
    assert_eq!(records.len(), 1);
    let event = &records[0].event;
    assert_eq!(event.id, confirmation.binding_ref);
    assert_eq!(
        event.data["admission_authority"]["expires_at"],
        confirmation_deadline
    );
    assert_eq!(
        event.data["admission_authority"]["authorization_revision"],
        f.scope.authorization_revision()
    );

    let path = f
        .directory
        .path()
        .canonicalize()
        .unwrap()
        .join("legacy.sqlite");
    drop(f.store);
    f.store = Fixture::open(&path, Redactor::new(vec![]));
    let head = f.store.head().unwrap();
    let replay = f
        .store
        .legacy_session_confirm_local(f.proof(), request)
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.binding_ref, confirmation.binding_ref);
    assert_eq!(replay.audit_hash, confirmation.audit_hash);
    assert_eq!(f.store.head().unwrap(), head);
    let replayed = f.store.events(&query).unwrap();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].event.data, event.data);
}

#[test]
fn legacy_confirmation_replays_the_v1_receipt_after_reopen() {
    let mut f = Fixture::new();
    let prepared = f
        .store
        .legacy_session_prepare_local(f.proof(), f.request())
        .unwrap();
    let request = SessionLaunchRequestV1 {
        version: 1,
        operation_id: Uuid::new_v4(),
        launch_target_id: f.target_id,
        expected_target_revision: 1,
        preparation_operation_id: prepared.operation_id,
        preparation_input_hash: prepared.input_hash,
    };
    let original = {
        let mut db = f.store.db.lock().unwrap();
        let tx = f.store.begin_checked(&mut db).unwrap();
        let clock = workload::capture_control_time(&tx).unwrap();
        let preparation = event_by_id(&tx, &prepared.preparation_id).unwrap().unwrap();
        let id = confirmation_ref(&preparation, prepared.operation_id);
        let record = Confirmed {
            version: 1,
            request: request.clone(),
            preparation_id: prepared.preparation_id,
            preparation_audit_hash: preparation.hash,
            session_id: session_id(request.operation_id).unwrap(),
            admission_authority: None,
        };
        let retained = f
            .store
            .append_legacy_session_record(&tx, &clock, CONFIRMED, id.clone(), &record)
            .unwrap();
        assert!(retained.event.data.get("admission_authority").is_none());
        for kind in [FenceKind::Operation, FenceKind::Session] {
            f.store
                .append_legacy_session_record(
                    &tx,
                    &clock,
                    FENCED,
                    fence_id(kind, &record),
                    &Fence {
                        version: 1,
                        binding_ref: id.clone(),
                        kind,
                    },
                )
                .unwrap();
        }
        f.store.commit_authorized(tx, &clock).unwrap();
        confirmed_receipt(&retained, &record, false)
    };
    let path = f
        .directory
        .path()
        .canonicalize()
        .unwrap()
        .join("legacy.sqlite");
    drop(f.store);
    f.store = Fixture::open(&path, Redactor::new(vec![]));
    assert!(f.store.integrity().unwrap().valid);
    let head = f.store.head().unwrap();
    let replay = f
        .store
        .legacy_session_confirm_local(f.proof(), request)
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.session_id, original.session_id);
    assert_eq!(replay.binding_ref, original.binding_ref);
    assert_eq!(replay.audit_hash, original.audit_hash);
    assert_eq!(f.store.head().unwrap(), head);
}

#[test]
fn redacted_legacy_prepare_event_identifier_leaves_the_head_and_reopen_intact() {
    let mut f = Fixture::new();
    let request = f.request();
    let id = prepare_id(request.operation_id);
    let before = f.store.integrity().unwrap();
    let path = f
        .directory
        .path()
        .canonicalize()
        .unwrap()
        .join("legacy.sqlite");
    drop(f.store);
    f.store = Fixture::open(&path, Redactor::new(vec![id]));
    let refusal = f
        .store
        .legacy_session_prepare_local(f.proof(), request)
        .unwrap_err();
    assert_eq!(
        refusal.to_string(),
        "Legacy reference record requires redaction"
    );
    assert_eq!(f.store.integrity().unwrap().head_hash, before.head_hash);
    drop(f.store);
    let reopened = Fixture::open(&path, Redactor::new(vec![]));
    let after = reopened.integrity().unwrap();
    assert!(after.valid);
    assert_eq!(
        (after.event_count, after.head_hash),
        (before.event_count, before.head_hash)
    );
}
