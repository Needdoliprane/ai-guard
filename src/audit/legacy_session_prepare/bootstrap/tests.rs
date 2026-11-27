use super::*;
use crate::{
    broker_registry::BrokerAuthorityV1,
    control::{
        AdmissionRefusalV1, AdmissionV1, ArmSubjectV1, ArmV1, ControlFailure, DecisionPhaseV1,
        DecisionReceiptV1, HelperPermissionV1, HelperPurposeV1, HelperStartClaimV1,
        HostClockMethodV1, HostClockSampleV1, LaunchSpecV1, NetworkV1, QuotaPolicyV1,
        RestartPolicyV1, RuntimeInspectionV1, SecurityPresetV1, SupervisedAdmissionV1,
        SupervisionProtocolV1, WorkerMutationV1, WorkerStateV1, WorkerTransitionV1, WorkerV1,
        helper_execution_policy_v1,
        session_control::{
            LegacyNamedSupervisedAdmissionV1, LegacySessionTargetV1, SessionLaunchPrepareRequestV1,
            SessionLaunchRequestV1, SessionLaunchTargetRecordV1,
        },
    },
    identity::{
        ContextModeV1, HumanAccountV1, HumanCapabilityV1, IdentityChangeV1, IdentityMutationV1,
        LocalCredentialV1, MembershipV1, ResourceKindV1, ResourceRefV1,
    },
    model::{CommandSpec, Decision, DecisionRequest, Outcome},
    policy::{self, Policy},
    workload::{AuthenticatedWorkload, WorkloadFailureV1},
};
use sha2::{Digest, Sha256};
use std::{os::unix::fs::PermissionsExt, path::PathBuf};

const TENANT: &str = "tenant-fresh";
const MAX_EVENTS: u64 = 1000;
const MAX_BYTES: u64 = 2 * DEFAULT_PHYSICAL_BYTES;
const DEADLINE: &str = "2126-01-01T00:00:00Z";
const BROKER_TOKEN: &str = "synthetic-bootstrap-broker-identity-fixture";

struct Fixture {
    _directory: tempfile::TempDir,
    source: PathBuf,
    backups: PathBuf,
    limits: AuditCapacityV1,
    policy: AuditCapacityPolicyV1,
    credential: WorkloadBindingIssueV1,
    grant: BrokerGrantMutationV1,
    operator: PrincipalRefV1,
    target: ProvisionLegacySessionTargetV1,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = directory.path().canonicalize().unwrap();
        let broker_id = Uuid::new_v4();
        let incarnation_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let authority_id = Uuid::new_v4().to_string();
        let credential_id = Uuid::new_v4().to_string();
        let mut policy = AuditCapacityPolicyV1::resolved_for(MAX_EVENTS, MAX_BYTES);
        policy.control_reserve.events += 1;
        Self {
            _directory: directory,
            source: root.join("legacy.sqlite"),
            backups: root.join("backups"),
            limits: AuditCapacityV1 {
                events: MAX_EVENTS,
                bytes: MAX_BYTES,
            },
            policy,
            credential: WorkloadBindingIssueV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                credential_id: credential_id.clone(),
                expected_revision: 0,
                subject: WorkloadSubjectV1::Broker {
                    broker_id: broker_id.to_string(),
                    incarnation_id: incarnation_id.to_string(),
                },
                sha256_digest: hex::encode(Sha256::digest(BROKER_TOKEN)),
                expires_at: DEADLINE.into(),
            },
            grant: BrokerGrantMutationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                broker_id: broker_id.to_string(),
                expected_revision: None,
                credential_id,
                incarnation_id: incarnation_id.to_string(),
                authorities: vec![BrokerAuthorityV1 {
                    authority_id: authority_id.clone(),
                    runtime_kind: BrokerRuntimeKindV1::Docker,
                }],
                lease_seconds: 120,
                not_after: DEADLINE.into(),
            },
            operator: PrincipalRefV1::LegacyControl {
                credential_id: "fixture-bootstrap".into(),
                role: LegacyControlRoleV1::Runner,
            },
            target: ProvisionLegacySessionTargetV1 {
                expected_revision: 0,
                target: LegacySessionTargetV1 {
                    version: 1,
                    catalogue: SessionLaunchTargetRecordV1 {
                        version: 1,
                        launch_target_id: target_id,
                        revision: 1,
                        tenant_id: TENANT.into(),
                        organization_scope: "fixture-org".into(),
                        active: true,
                        launchers: vec![PrincipalRefV1::HumanLocal {
                            installation_id: Uuid::new_v4().to_string(),
                            account_id: Uuid::new_v4().to_string(),
                        }],
                    },
                    broker_id,
                    incarnation_id,
                    broker_grant_revision: 1,
                    broker_profile_id: "fixture-bootstrap-profile".into(),
                    launch: LaunchSpecV1 {
                        version: 1,
                        session_id: target_id.to_string(),
                        command: CommandSpec {
                            argv: vec!["/usr/bin/id".into()],
                            cwd: "/workspace".into(),
                        },
                        profile: "observe".into(),
                        runtime_kind: RuntimeKindV1::Docker,
                        authority_id,
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
                    },
                },
            },
        }
    }

    fn create(&self) -> anyhow::Result<FreshLegacySessionBootstrap> {
        FreshLegacySessionBootstrap::create(
            &self.source,
            TENANT,
            Redactor::new(vec![]),
            self.limits,
            Some(self.policy.clone()),
            &self.backups,
            (&self.credential, &self.grant),
        )
    }

    fn provision(
        &self,
        startup: FreshLegacySessionBootstrap,
    ) -> anyhow::Result<(Store, LegacyTargetReceipt)> {
        startup.provision_target(self.operator.clone(), self.target.clone())
    }

    fn reopen(&self) -> Store {
        Store::open_existing_for_tenant(
            &self.source,
            TENANT,
            Redactor::new(vec![]),
            self.limits.events,
        )
        .unwrap()
        .with_physical_bytes(self.limits.bytes)
        .unwrap()
        .with_capacity_policy(self.policy.clone())
        .unwrap()
    }

    fn confirmed_request(
        &self,
        store: &Store,
    ) -> (LegacyNamedSupervisedAdmissionV1, LocalCredentialV1) {
        let catalogue = &self.target.target.catalogue;
        let principal = &catalogue.launchers[0];
        let membership_id = Uuid::new_v4().to_string();
        let credential_id = Uuid::new_v4().to_string();
        let digest: [u8; 32] = Sha256::digest(b"synthetic named admission initiator").into();
        let credential = LocalCredentialV1 {
            version: 1,
            credential_id: credential_id.clone(),
            principal: principal.clone(),
            sha256_digest: hex::encode(digest),
            revision: 1,
            active: true,
            expires_at: None,
        };
        for change in [
            IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Admission fixture".into(),
                    revision: 1,
                    active: true,
                },
            },
            IdentityChangeV1::Credential {
                expected_revision: 0,
                binding: credential.clone(),
            },
            IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: MembershipV1 {
                    version: 1,
                    membership_id: membership_id.clone(),
                    principal: principal.clone(),
                    tenant_id: TENANT.into(),
                    organization_scope: catalogue.organization_scope.clone(),
                    revision: 1,
                    active: true,
                    capabilities: [HumanCapabilityV1::LaunchSession].into_iter().collect(),
                },
            },
        ] {
            store
                .mutate_identity(&IdentityMutationV1 {
                    version: 1,
                    actor: self.operator.clone(),
                    operation_id: Uuid::new_v4().to_string(),
                    change,
                })
                .unwrap();
        }
        let scope = store
            .authorize_human_context(principal, &membership_id, 1, ContextModeV1::Personal)
            .unwrap();
        let proof = || LegacySessionLocalProof {
            scope: &scope,
            digest: &digest,
            credential_id: &credential_id,
            credential_revision: 1,
            context_expires_at: DEADLINE,
        };
        let prepared = store
            .legacy_session_prepare_local(
                proof(),
                SessionLaunchPrepareRequestV1 {
                    version: 1,
                    operation_id: Uuid::new_v4(),
                    launch_target_id: catalogue.launch_target_id,
                    expected_target_revision: 1,
                    expires_at: DEADLINE.into(),
                },
            )
            .unwrap();
        let confirmation = store
            .legacy_session_confirm_local(
                proof(),
                SessionLaunchRequestV1 {
                    version: 1,
                    operation_id: Uuid::new_v4(),
                    launch_target_id: catalogue.launch_target_id,
                    expected_target_revision: 1,
                    preparation_operation_id: prepared.operation_id,
                    preparation_input_hash: prepared.input_hash,
                },
            )
            .unwrap();
        let request = LegacyNamedSupervisedAdmissionV1 {
            version: 1,
            binding_ref: Uuid::parse_str(&confirmation.binding_ref).unwrap(),
            broker_profile_id: self.target.target.broker_profile_id.clone(),
            admission: self.supervised_request(
                store,
                confirmation.operation_id,
                confirmation.session_id,
            ),
        };
        (request, credential)
    }

    fn supervised_request(
        &self,
        store: &Store,
        operation_id: Uuid,
        session_id: String,
    ) -> SupervisedAdmissionV1 {
        let mut launch = self.target.target.launch.clone();
        launch.session_id = session_id;
        let decision_request = DecisionRequest {
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
        let decision = policy::evaluate(&Policy::default(), &decision_request);
        let decision_event = store
            .decision(&decision_request, Some((decision, Value::Null)))
            .unwrap()
            .unwrap();
        SupervisedAdmissionV1 {
            version: 1,
            protocol: SupervisionProtocolV1::BrokerSupervisionV1,
            intent_hash: "c".repeat(64),
            admission: AdmissionV1 {
                version: 1,
                operation_id: operation_id.to_string(),
                receipt: DecisionReceiptV1 {
                    version: 1,
                    request_id: decision_request.request_id.clone(),
                    request_hash: policy::request_hash(&decision_request),
                    phase: DecisionPhaseV1::Initial,
                    audit_seq: decision_event.seq,
                    audit_hash: decision_event.hash,
                },
                decision_request,
                launch,
            },
        }
    }
}

#[test]
fn legacy_named_admission_commits_a_grant_and_replays_after_reopen() {
    let f = Fixture::new();
    let (store, _) = f.provision(f.create().unwrap()).unwrap();
    let (request, mut initiator) = f.confirmed_request(&store);
    {
        let after_fences = store.head().unwrap() + 1;
        let db = store.db.lock().unwrap();
        let confirmed = event_by_id(&db, &request.binding_ref.to_string())
            .unwrap()
            .unwrap();
        let launch = &request.admission.admission;
        let unrelated = Uuid::new_v4().to_string();
        for (operation, session) in [
            (launch.operation_id.as_str(), unrelated.as_str()),
            (unrelated.as_str(), launch.launch.session_id.as_str()),
        ] {
            protect_manual_at(&db, operation, session, confirmed.seq).unwrap();
            assert_eq!(
                protect_manual_at(&db, operation, session, after_fences)
                    .unwrap_err()
                    .downcast_ref::<ControlFailure>(),
                Some(&ControlFailure::Conflict)
            );
        }
    }
    let confirmation_revision = store.identity_authorization_revision().unwrap();
    store
        .mutate_identity(&IdentityMutationV1 {
            version: 1,
            actor: f.operator.clone(),
            operation_id: Uuid::new_v4().to_string(),
            change: IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: PrincipalRefV1::HumanLocal {
                        installation_id: Uuid::new_v4().to_string(),
                        account_id: Uuid::new_v4().to_string(),
                    },
                    display_name: "Unrelated admission fixture account".into(),
                    revision: 1,
                    active: true,
                },
            },
        })
        .unwrap();
    let admission_revision = store.identity_authorization_revision().unwrap();
    assert_eq!(admission_revision, confirmation_revision + 1);
    let Some(AuthenticatedWorkload::Broker(broker)) =
        store.authenticate_workload_token(BROKER_TOKEN).unwrap()
    else {
        panic!("the retained workload binding must authenticate the broker");
    };
    let policy = Policy::default();
    let quota = QuotaPolicyV1::default();
    let apply = |store: &Store, request: &LegacyNamedSupervisedAdmissionV1| {
        store.admit_legacy_named(
            &broker,
            request,
            &policy,
            policy::evaluate(&policy, &request.admission.admission.decision_request),
            Value::Null,
            &quota,
        )
    };
    let before = store.head().unwrap();
    assert_eq!(
        store
            .admit_supervised(
                &request.admission,
                &policy,
                policy::evaluate(&policy, &request.admission.admission.decision_request),
                Value::Null,
                &quota,
            )
            .unwrap_err()
            .downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::Conflict)
    );
    let mut divergent = request.clone();
    divergent.admission.admission.launch.timeout_seconds += 1;
    assert_eq!(
        apply(&store, &divergent)
            .err()
            .unwrap()
            .downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::Conflict)
    );
    assert_eq!(store.head().unwrap(), before);
    assert!(store.workers().unwrap().is_empty());

    let committed = apply(&store, &request).unwrap();
    assert!(committed.admission.accepted && !committed.admission.replayed);
    let worker = committed.admission.worker.as_ref().unwrap();
    assert_eq!(store.workers().unwrap().len(), 1);
    assert_eq!(store.worker_quota(&quota).unwrap().held_workers, 1);
    let resource = ResourceRefV1 {
        kind: ResourceKindV1::Session,
        resource_id: worker.launch.session_id.clone(),
    };
    let grant = store.identity_resource_grant(&resource).unwrap().unwrap();
    assert_eq!(grant.owner, f.target.target.catalogue.launchers[0]);
    assert!(grant.active && grant.shares.is_empty());
    let integrity = store.integrity().unwrap();
    assert!(integrity.valid);
    assert_eq!(integrity.head_hash, committed.audit_hash);
    assert_eq!(
        committed.grant.as_ref().unwrap().audit_seq,
        committed.admission.receipt.audit_seq + 1
    );
    assert_eq!(
        committed.audit_seq,
        committed.admission.receipt.audit_seq + 2
    );
    let fact = store
        .events(&EventQuery {
            kind: Some("control.legacy_session_prepare.admission".into()),
            limit: Some(1),
            ..Default::default()
        })
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(fact.hash, committed.audit_hash);
    assert_eq!(
        fact.event.data["authorization_revision"],
        admission_revision
    );
    initiator.active = false;
    initiator.revision += 1;
    store
        .mutate_identity(&IdentityMutationV1 {
            version: 1,
            actor: f.operator.clone(),
            operation_id: Uuid::new_v4().to_string(),
            change: IdentityChangeV1::Credential {
                expected_revision: 1,
                binding: initiator,
            },
        })
        .unwrap();
    assert!(
        store
            .legacy_confirmed_plan(&broker, request.binding_ref)
            .is_err()
    );
    drop(store);

    let reopened = f.reopen();
    assert!(!reopened.supervision_state().unwrap().active);
    let mut retired_target = f.target.clone();
    retired_target.expected_revision = 1;
    retired_target.target.catalogue.revision = 2;
    retired_target.target.catalogue.active = false;
    let retired = reopened
        .provision_legacy_session_target(f.operator.clone(), retired_target)
        .unwrap();
    drop(reopened);
    let reopened = f.reopen();
    assert!(reopened.integrity().unwrap().valid);
    *reopened.named_control_commit_observation.lock().unwrap() =
        Some(crate::broker_registry::parse_time(DEADLINE).unwrap());
    assert_eq!(
        apply(&reopened, &request)
            .err()
            .unwrap()
            .downcast_ref::<WorkloadFailureV1>(),
        Some(&WorkloadFailureV1::Unauthorized)
    );
    assert!(reopened.integrity().unwrap().valid);
    assert_eq!(reopened.integrity().unwrap().head_hash, retired.audit_hash);
    let replay = apply(&reopened, &request).unwrap();
    assert!(replay.admission.replayed);
    assert_eq!(replay.admission.receipt, committed.admission.receipt);
    assert_eq!(replay.grant, committed.grant);
    assert_eq!(replay.audit_hash, committed.audit_hash);
    assert_eq!(reopened.integrity().unwrap().head_hash, retired.audit_hash);
    assert_eq!(reopened.worker_quota(&quota).unwrap().held_workers, 1);
    assert!(apply(&reopened, &divergent).is_err());
}

#[test]
fn legacy_named_execution_rechecks_authority_after_reopen() {
    for revoke_grant in [false, true] {
        let f = Fixture::new();
        let (store, _) = f.provision(f.create().unwrap()).unwrap();
        let (request, mut credential) = f.confirmed_request(&store);
        let Some(AuthenticatedWorkload::Broker(broker)) =
            store.authenticate_workload_token(BROKER_TOKEN).unwrap()
        else {
            panic!("fixture broker identity");
        };
        let policy = Policy::default();
        let quota = QuotaPolicyV1::default();
        let admit = |store: &Store| {
            store.admit_legacy_named(
                &broker,
                &request,
                &policy,
                policy::evaluate(&policy, &request.admission.admission.decision_request),
                Value::Null,
                &quota,
            )
        };
        let committed = admit(&store).unwrap();
        let worker = committed.admission.worker.as_ref().unwrap();
        let namespace = store
            .supervision_state()
            .unwrap()
            .control_namespace_uuid
            .unwrap();
        let check = |store: &Store, namespace: &str, worker: &WorkerV1| {
            let mut db = store.db.lock().unwrap();
            let tx = store.begin_checked(&mut db)?;
            let clock = workload::capture_control_time(&tx)?;
            let deadlines = store.require_legacy_named_execution(
                &tx,
                &broker,
                request.binding_ref,
                namespace,
                worker,
                &clock,
            )?;
            store.commit_checked(tx)?;
            Ok::<_, anyhow::Error>(deadlines)
        };
        let before_reopen = check(&store, &namespace, worker).unwrap();
        drop(store);
        let reopened = f.reopen();
        assert!(!reopened.supervision_state().unwrap().active);
        assert_eq!(check(&reopened, &namespace, worker).unwrap(), before_reopen);
        let mut other_attempt = worker.clone();
        other_attempt.attempt.attempt_id = Uuid::new_v4().to_string();
        assert!(check(&reopened, &namespace, &other_attempt).is_err());
        let mut other_worker = worker.clone();
        other_worker.worker_id = Uuid::new_v4().to_string();
        assert!(check(&reopened, &namespace, &other_worker).is_err());
        let mut other_session = worker.clone();
        other_session.launch.session_id = Uuid::new_v4().to_string();
        assert!(check(&reopened, &namespace, &other_session).is_err());
        assert!(check(&reopened, &Uuid::new_v4().to_string(), worker).is_err());
        assert_eq!(reopened.head().unwrap(), committed.audit_seq);

        let change = if revoke_grant {
            let resource = ResourceRefV1 {
                kind: ResourceKindV1::Session,
                resource_id: worker.launch.session_id.clone(),
            };
            let mut grant = reopened
                .identity_resource_grant(&resource)
                .unwrap()
                .unwrap();
            grant.active = false;
            grant.revision = 2;
            IdentityChangeV1::ResourceGrant {
                expected_revision: 1,
                grant,
            }
        } else {
            credential.active = false;
            credential.revision = 2;
            IdentityChangeV1::Credential {
                expected_revision: 1,
                binding: credential,
            }
        };
        let revoked = reopened
            .mutate_identity(&IdentityMutationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                actor: f.operator.clone(),
                change,
            })
            .unwrap();
        assert_eq!(
            check(&reopened, &namespace, worker)
                .unwrap_err()
                .downcast_ref::<ControlFailure>(),
            Some(&ControlFailure::NotFound)
        );
        let replay = admit(&reopened).unwrap();
        assert!(replay.admission.replayed);
        assert_eq!(replay.admission.receipt, committed.admission.receipt);
        assert_eq!(replay.audit_hash, committed.audit_hash);
        assert_eq!(reopened.head().unwrap(), revoked.audit_seq);
        assert_eq!(reopened.worker_quota(&quota).unwrap().held_workers, 1);
    }
}

#[test]
fn legacy_named_reservation_refuses_generic_prepare_and_stays_cancellable() {
    let f = Fixture::new();
    let (store, _) = f.provision(f.create().unwrap()).unwrap();
    let (request, _) = f.confirmed_request(&store);
    let Some(AuthenticatedWorkload::Broker(broker)) =
        store.authenticate_workload_token(BROKER_TOKEN).unwrap()
    else {
        panic!("fixture broker identity");
    };
    let policy = Policy::default();
    let quota = QuotaPolicyV1::default();
    let named = store
        .admit_legacy_named(
            &broker,
            &request,
            &policy,
            policy::evaluate(&policy, &request.admission.admission.decision_request),
            Value::Null,
            &quota,
        )
        .unwrap()
        .admission
        .worker
        .unwrap();
    let mutation_for = |worker: &WorkerV1, transition| WorkerMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: worker.worker_id.clone(),
        attempt_id: worker.attempt.attempt_id.clone(),
        expected_revision: worker.revision,
        transition,
    };

    assert_eq!(named.state, WorkerStateV1::Reserved);
    let before = store.head().unwrap();
    let named_prepare = mutation_for(&named, WorkerTransitionV1::Prepare);
    assert_eq!(
        store
            .mutate_worker(&named_prepare)
            .unwrap_err()
            .downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::Conflict)
    );
    assert_eq!(store.head().unwrap(), before);
    let retained = store.worker(&named.worker_id).unwrap().unwrap();
    assert_eq!(retained.state, WorkerStateV1::Reserved);
    assert_eq!(retained.revision, named.revision);
    assert!(retained.quota_held);

    let mut manual_request =
        f.supervised_request(&store, Uuid::new_v4(), Uuid::new_v4().to_string());
    manual_request.intent_hash = "d".repeat(64);
    let manual = store
        .admit_supervised(
            &manual_request,
            &policy,
            policy::evaluate(&policy, &manual_request.admission.decision_request),
            Value::Null,
            &quota,
        )
        .unwrap()
        .worker
        .unwrap();
    let manual_prepare = mutation_for(&manual, WorkerTransitionV1::Prepare);
    let prepared = store.mutate_worker(&manual_prepare).unwrap();
    assert_eq!(prepared.worker.state, WorkerStateV1::Preparing);
    assert!(prepared.worker.quota_held);

    let cancel = mutation_for(&named, WorkerTransitionV1::CancelReserved);
    let cancelled = store.mutate_worker(&cancel).unwrap();
    assert_eq!(cancelled.worker.state, WorkerStateV1::Cancelled);
    assert!(!cancelled.worker.quota_held);
    assert_eq!(store.worker_quota(&quota).unwrap().held_workers, 1);
    drop(store);

    let reopened = f.reopen();
    let replayed_prepare = reopened.mutate_worker(&manual_prepare).unwrap();
    let replayed_cancel = reopened.mutate_worker(&cancel).unwrap();
    assert_eq!(replayed_prepare.receipt, prepared.receipt);
    assert_eq!(replayed_cancel.receipt, cancelled.receipt);
    let integrity = reopened.integrity().unwrap();
    assert!(integrity.valid);
    assert_eq!(integrity.head_hash, cancelled.receipt.audit_hash);
}

#[test]
fn legacy_named_import_arm_preserves_its_prefix_and_closes_manual_claims() {
    let f = Fixture::new();
    let (store, _) = f.provision(f.create().unwrap()).unwrap();
    let (request, mut credential) = f.confirmed_request(&store);
    let Some(AuthenticatedWorkload::Broker(broker)) =
        store.authenticate_workload_token(BROKER_TOKEN).unwrap()
    else {
        panic!("fixture broker identity");
    };
    let policy = Policy::default();
    let quota = QuotaPolicyV1::default();
    let committed = store
        .admit_legacy_named(
            &broker,
            &request,
            &policy,
            policy::evaluate(&policy, &request.admission.admission.decision_request),
            Value::Null,
            &quota,
        )
        .unwrap();
    let parent = committed.admission.worker.unwrap();
    let helper_id = Uuid::new_v4();
    let helper = helper_execution_policy_v1(&parent, HelperPurposeV1::Import, helper_id).unwrap();
    let namespace = store
        .supervision_state()
        .unwrap()
        .control_namespace_uuid
        .unwrap();
    let clock = HostClockSampleV1 {
        version: 1,
        method: HostClockMethodV1::DarwinMachContinuousV1,
        boot_id: Uuid::new_v4().to_string(),
        continuous_ns_decimal: "100000000000".into(),
        observed_utc: None,
    };
    // The inspection describes a synthetic Created helper. No runtime
    // observation, execution authority or cancellation proof is exercised.
    let arm = ArmV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        worker_id: parent.worker_id.clone(),
        attempt_id: parent.attempt.attempt_id.clone(),
        expected_revision: parent.revision + 1,
        intent_hash: request.admission.intent_hash,
        subject: ArmSubjectV1::Helper {
            helper_id: helper_id.to_string(),
            purpose: HelperPurposeV1::Import,
            policy_version: 1,
            permission: Box::new(HelperPermissionV1::Admission {
                receipt: committed.admission.receipt,
            }),
        },
        inspection: RuntimeInspectionV1 {
            authority_id: helper.authority_id.clone(),
            runtime_kind: helper.runtime_kind,
            container_id: "d".repeat(64),
            stable_name: helper.stable_name.clone(),
            volume_name: helper.volume_name.clone(),
            image_id: helper.image_id.clone(),
            spec_hash: helper.spec_hash().unwrap(),
            cpu_millis: helper.cpu_millis,
            memory_bytes: helper.memory_bytes,
            pids_limit: helper.pids_limit,
            network: helper.network,
            security_preset: helper.security_preset,
            restart_policy: helper.restart_policy,
        },
        authority_binding_hash: "a".repeat(64),
        configuration_fingerprint_version: 2,
        configuration_fingerprint: "b".repeat(64),
        clock: clock.clone(),
        timeout_seconds: 120,
        deadline_ns_decimal: "220000000000".into(),
    };
    let write = |store: &Store, namespace: &str, given: &ArmV1| {
        store.arm_legacy_named_import(&broker, request.binding_ref, namespace, given)
    };
    let before = store.head().unwrap();
    assert!(write(&store, &Uuid::new_v4().to_string(), &arm).is_err());
    let mut wrong_receipt = arm.clone();
    let ArmSubjectV1::Helper { permission, .. } = &mut wrong_receipt.subject else {
        panic!("fixture requires an Import helper");
    };
    let HelperPermissionV1::Admission { receipt } = permission.as_mut() else {
        panic!("fixture requires an admission receipt");
    };
    receipt.audit_seq = committed.audit_seq;
    assert!(write(&store, &namespace, &wrong_receipt).is_err());
    let mut other_attempt = arm.clone();
    other_attempt.attempt_id = Uuid::new_v4().to_string();
    assert!(write(&store, &namespace, &other_attempt).is_err());
    let mut other_intent = arm.clone();
    other_intent.intent_hash = "e".repeat(64);
    assert!(write(&store, &namespace, &other_intent).is_err());
    assert_eq!(store.head().unwrap(), before);

    let expiry = crate::broker_registry::parse_time(DEADLINE).unwrap();
    *store.named_control_commit_observation.lock().unwrap() = Some(expiry);
    assert_eq!(
        write(&store, &namespace, &arm)
            .unwrap_err()
            .downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::NotFound)
    );
    assert_eq!(store.head().unwrap(), before);
    assert_eq!(
        store.worker(&parent.worker_id).unwrap().unwrap().state,
        WorkerStateV1::Reserved
    );
    assert!(store.integrity().unwrap().valid);

    let armed = write(&store, &namespace, &arm).unwrap();
    assert!(!armed.replayed);
    assert_eq!(armed.receipt.control_namespace_uuid, namespace);
    armed.receipt.validate_for_arm(&arm).unwrap();
    let claim = HelperStartClaimV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        helper_id: helper_id.to_string(),
        emitter_id: Uuid::new_v4().to_string(),
        arm_receipt: armed.receipt.clone(),
        clock_check: HostClockSampleV1 {
            continuous_ns_decimal: "100000000001".into(),
            ..clock
        },
    };
    assert!(store.execution_arm(&arm.operation_id).unwrap().is_none());
    assert_eq!(
        store
            .claim_helper_start(&claim)
            .unwrap_err()
            .downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::Conflict)
    );
    assert!(
        store
            .worker_operation(&claim.operation_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(store.head().unwrap(), armed.receipt.audit_seq);
    let prepared = store.worker(&parent.worker_id).unwrap().unwrap();
    assert_eq!(prepared.state, WorkerStateV1::Preparing);
    assert_eq!(prepared.revision, parent.revision + 1);
    assert!(prepared.quota_held);
    let generic_cancel = WorkerMutationV1 {
        version: 1,
        worker_id: prepared.worker_id.clone(),
        attempt_id: prepared.attempt.attempt_id.clone(),
        operation_id: Uuid::new_v4().to_string(),
        expected_revision: prepared.revision,
        transition: WorkerTransitionV1::CancelReserved,
    };
    assert_eq!(
        store
            .mutate_worker(&generic_cancel)
            .unwrap_err()
            .downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::Conflict)
    );
    assert_eq!(store.head().unwrap(), armed.receipt.audit_seq);
    assert_eq!(store.worker_quota(&quota).unwrap().held_workers, 1);

    credential.active = false;
    credential.revision += 1;
    let revoked = store
        .mutate_identity(&IdentityMutationV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            actor: f.operator.clone(),
            change: IdentityChangeV1::Credential {
                expected_revision: 1,
                binding: credential,
            },
        })
        .unwrap();
    drop(store);

    let reopened = f.reopen();
    assert_eq!(
        write(&reopened, &namespace, &wrong_receipt)
            .unwrap_err()
            .downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::Conflict)
    );
    *reopened.named_control_commit_observation.lock().unwrap() = Some(expiry);
    assert_eq!(
        write(&reopened, &namespace, &arm)
            .unwrap_err()
            .downcast_ref::<WorkloadFailureV1>(),
        Some(&WorkloadFailureV1::Unauthorized)
    );
    let replay = write(&reopened, &namespace, &arm).unwrap();
    assert!(replay.replayed);
    assert_eq!(
        serde_json::to_value(replay.receipt).unwrap(),
        serde_json::to_value(&armed.receipt).unwrap()
    );
    let retained = reopened
        .worker_operation(&arm.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(retained.audit_seq, armed.receipt.audit_seq);
    assert_eq!(retained.audit_hash, armed.receipt.audit_hash);
    assert!(reopened.execution_arm(&arm.operation_id).unwrap().is_none());
    assert!(reopened.integrity().unwrap().valid);
    assert_eq!(reopened.head().unwrap(), revoked.audit_seq);
    assert_eq!(reopened.worker_quota(&quota).unwrap().held_workers, 1);
}

#[tokio::test]
async fn legacy_named_admission_route_replays_the_committed_receipt_after_reopen() {
    use crate::{
        api::{GatewayState, TenantGateway, gateway_router},
        control::session_control::LegacyNamedAdmissionReceiptV1,
    };
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use std::sync::Arc;
    use tower::ServiceExt;

    const RUNNER: &str = "synthetic-bootstrap-runner-identity-fixture";
    let make_router = |store| {
        gateway_router(
            GatewayState::for_tenants(
                vec![TenantGateway {
                    tenant_id: TENANT.into(),
                    store,
                    runner_token: RUNNER.into(),
                    viewer_token: "synthetic-bootstrap-viewer-identity-fixture".into(),
                }],
                Policy::default(),
                None,
                Redactor::new(vec![]),
            )
            .unwrap(),
        )
    };
    let post = |path: &str, secret: &str, body: Vec<u8>| {
        Request::post(path)
            .header(header::AUTHORIZATION, format!("Bearer {secret}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap()
    };
    let f = Fixture::new();
    let (store, _) = f.provision(f.create().unwrap()).unwrap();
    let (request, _) = f.confirmed_request(&store);
    let store = Arc::new(store);
    let app = make_router(Arc::clone(&store));
    let path = format!(
        "/api/v1/control/session-confirmations/{}/admission",
        request.binding_ref
    );
    let private = format!("/control{path}");
    let mismatched = format!(
        "/control/api/v1/control/session-confirmations/{}/admission",
        Uuid::new_v4()
    );
    let body = serde_json::to_vec(&request).unwrap();
    let before = store.head().unwrap();
    for (uri, token, expected) in [
        (path.as_str(), BROKER_TOKEN, StatusCode::NOT_FOUND),
        (private.as_str(), RUNNER, StatusCode::FORBIDDEN),
        (mismatched.as_str(), BROKER_TOKEN, StatusCode::BAD_REQUEST),
    ] {
        let response = app
            .clone()
            .oneshot(post(uri, token, body.clone()))
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    assert_eq!(store.head().unwrap(), before);

    let response = app
        .oneshot(post(&private, BROKER_TOKEN, body.clone()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let receipt: LegacyNamedAdmissionReceiptV1 =
        serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await.unwrap()).unwrap();
    let worker = receipt.admission.worker.as_ref().unwrap();
    assert_eq!(receipt.version, 1);
    assert!(!receipt.admission.replayed);
    assert_eq!(worker.state, WorkerStateV1::Reserved);
    assert_eq!(receipt.admission.receipt.audit_seq, before + 1);
    assert_eq!(receipt.grant.as_ref().unwrap().audit_seq, before + 2);
    assert_eq!(receipt.audit_seq, before + 3);
    assert_eq!(store.integrity().unwrap().head_hash, receipt.audit_hash);
    drop(store);

    let reopened = Arc::new(f.reopen());
    assert!(!reopened.supervision_state().unwrap().active);
    let response = make_router(Arc::clone(&reopened))
        .oneshot(post(&private, BROKER_TOKEN, body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let replay: LegacyNamedAdmissionReceiptV1 =
        serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await.unwrap()).unwrap();
    assert!(replay.admission.replayed);
    assert_eq!(replay.admission.receipt, receipt.admission.receipt);
    assert_eq!(replay.grant, receipt.grant);
    assert_eq!(
        replay.admission.worker.as_ref().unwrap().worker_id,
        worker.worker_id
    );
    assert_eq!(replay.audit_seq, receipt.audit_seq);
    assert_eq!(replay.audit_hash, receipt.audit_hash);
    assert_eq!(reopened.head().unwrap(), receipt.audit_seq);
}

#[test]
fn legacy_named_checkpoint_retains_the_exact_store_admission_after_reopen() {
    use crate::{
        broker::{
            checkpoint::{
                BrokerCheckpointV2, DecisionAnswerV1, LegacyNamedAdmissionCheckpointV1,
                SupervisionCheckpointV1, SupervisionOperationV1,
            },
            intent::ExecutionIntentV1,
            journal::{
                BindingInputs, ControlSecrets, FsExclusions, Journal, JournalError,
                JournalLimitsV1, JournalScopeV1, ReserveResult,
            },
        },
        control_fs::Directory,
        runner::CheckpointPayloadV1,
    };

    let f = Fixture::new();
    let (store, _) = f.provision(f.create().unwrap()).unwrap();
    let (mut request, _) = f.confirmed_request(&store);
    let Some(AuthenticatedWorkload::Broker(broker)) =
        store.authenticate_workload_token(BROKER_TOKEN).unwrap()
    else {
        panic!("fixture broker identity");
    };
    let plan = store
        .legacy_confirmed_plan(&broker, request.binding_ref)
        .unwrap();
    let namespace = Uuid::parse_str(&plan.control_namespace_uuid).unwrap();
    let root = f._directory.path().canonicalize().unwrap();
    let launch = &request.admission.admission.launch;
    let gateway = "http://127.0.0.1:8787";
    let options = serde_json::from_value(json!({
        "project": root,
        "config_dir": root,
        "gateway": gateway,
        "runtime": "docker",
        "image": launch.image_ref,
        "profile": launch.profile,
        "network": "none",
        "allow_rootful_runtime": false,
        "timeout": launch.timeout_seconds,
        "provider": launch.provider,
        "capture_output": false,
        "argv": launch.command.argv
    }))
    .unwrap();
    let decision_request = request.admission.admission.decision_request.clone();
    let intent = ExecutionIntentV1::from_projection(
        1,
        plan.operation_id,
        plan.session_id,
        options,
        decision_request.clone(),
        None,
        None,
        None,
    )
    .unwrap();
    request.admission.intent_hash = intent.intent_hash().unwrap();
    let scope = JournalScopeV1::new(
        Uuid::new_v4(),
        TENANT,
        gateway,
        namespace,
        &plan.broker_profile_id,
    )
    .unwrap();
    let journal_root = Directory::open_existing(&root).unwrap();
    let open = || {
        Journal::open(
            &journal_root,
            scope.clone(),
            &FsExclusions::new(),
            JournalLimitsV1::default(),
            ControlSecrets::new(vec![]).unwrap(),
        )
        .unwrap()
    };
    let journal = open();
    let operation = intent.operation_id();
    let ReserveResult::New(ticket) = journal
        .reserve(operation, br#"{"fixture":"named-admission"}"#)
        .unwrap()
    else {
        panic!("the fixture requires a fresh job");
    };
    let runner = CheckpointPayloadV1::initial(&intent).unwrap();
    let accepted = journal
        .accept(ticket, &intent, BindingInputs::None, &runner)
        .unwrap();
    let decision_checkpoint = |response| {
        SupervisionCheckpointV1::new(
            namespace,
            vec![],
            vec![SupervisionOperationV1::Decision {
                request: decision_request.clone(),
                response,
            }],
        )
        .unwrap()
    };
    let ordinary = BrokerCheckpointV2::new(runner.clone(), decision_checkpoint(None)).unwrap();
    let ordinary_bytes = ordinary.canonical_bytes().unwrap();
    assert!(!String::from_utf8_lossy(&ordinary_bytes).contains("legacy_named_admission"));
    assert_eq!(
        BrokerCheckpointV2::from_bytes(&ordinary_bytes)
            .unwrap()
            .canonical_bytes()
            .unwrap(),
        ordinary_bytes
    );
    journal
        .advance_supervised(operation, accepted.head().clone(), &ordinary)
        .unwrap();
    let decision = policy::evaluate(&Policy::default(), &decision_request);
    let answer = DecisionAnswerV1::new(
        decision.clone(),
        request.admission.admission.receipt.clone(),
    )
    .unwrap();
    let decided =
        BrokerCheckpointV2::new(runner.clone(), decision_checkpoint(Some(answer.clone()))).unwrap();
    journal
        .advance_supervised(
            operation,
            journal.load(operation).unwrap().unwrap().head().clone(),
            &decided,
        )
        .unwrap();
    let checkpoint = |named| {
        BrokerCheckpointV2::new(
            runner.clone(),
            decision_checkpoint(Some(answer.clone()))
                .with_legacy_named_admission(named)
                .unwrap(),
        )
        .unwrap()
    };
    let pending = LegacyNamedAdmissionCheckpointV1::new(plan.clone(), request.clone()).unwrap();
    let current = || journal.load(operation).unwrap().unwrap().head().clone();
    let manual = SupervisionCheckpointV1::new(
        namespace,
        vec![],
        vec![SupervisionOperationV1::Admission {
            request: request.admission.clone(),
            response: None,
        }],
    )
    .unwrap();
    assert!(manual.with_legacy_named_admission(pending.clone()).is_err());

    let mut other_profile = plan.clone();
    other_profile.broker_profile_id = "another-profile".into();
    let mut other_request = request.clone();
    other_request.broker_profile_id = other_profile.broker_profile_id.clone();
    let wrong_scope = LegacyNamedAdmissionCheckpointV1::new(other_profile, other_request).unwrap();
    assert!(matches!(
        journal.advance_supervised(operation, current(), &checkpoint(wrong_scope)),
        Err(JournalError::Conflict)
    ));
    journal
        .advance_supervised(operation, current(), &checkpoint(pending.clone()))
        .unwrap();
    let receipt = store
        .admit_legacy_named(
            &broker,
            &request,
            &Policy::default(),
            decision,
            Value::Null,
            &QuotaPolicyV1::default(),
        )
        .unwrap();
    let mut other_receipt = receipt.admission.clone();
    other_receipt.receipt.operation_id = Uuid::new_v4().to_string();
    assert!(
        pending
            .clone()
            .acknowledge(
                other_receipt,
                receipt.grant.clone(),
                receipt.audit_seq,
                receipt.audit_hash.clone()
            )
            .is_err()
    );
    let response = pending
        .acknowledge(
            receipt.admission,
            receipt.grant,
            receipt.audit_seq,
            receipt.audit_hash,
        )
        .unwrap();
    let acknowledged = checkpoint(response.clone());
    assert!(matches!(
        acknowledged.validate_transition(Some(&decided)),
        Err(JournalError::Conflict)
    ));
    journal
        .advance_supervised(operation, current(), &acknowledged)
        .unwrap();
    assert!(matches!(
        journal.advance_supervised(operation, current(), &decided),
        Err(JournalError::Conflict)
    ));
    let retained_receipt = response.admission_result().unwrap().receipt.clone();
    drop(journal);
    let reopened = open().load(operation).unwrap().unwrap();
    let retained = reopened.broker_checkpoint_v2().unwrap();
    assert_eq!(
        retained.canonical_bytes().unwrap(),
        acknowledged.canonical_bytes().unwrap()
    );
    let checkpoint = retained.supervision().legacy_named_admission().unwrap();
    assert_eq!(checkpoint.plan(), &plan);
    assert_eq!(
        checkpoint.admission_result().unwrap().receipt,
        retained_receipt
    );
    assert_eq!(
        checkpoint
            .admission_result()
            .unwrap()
            .worker
            .as_ref()
            .unwrap()
            .launch
            .session_id,
        intent.session().to_string()
    );
}

#[test]
fn legacy_named_admission_retains_policy_and_quota_refusals_without_a_grant() {
    for refusal in [AdmissionRefusalV1::Policy, AdmissionRefusalV1::Quota] {
        let f = Fixture::new();
        let (store, _) = f.provision(f.create().unwrap()).unwrap();
        let (request, _) = f.confirmed_request(&store);
        let Some(AuthenticatedWorkload::Broker(broker)) =
            store.authenticate_workload_token(BROKER_TOKEN).unwrap()
        else {
            panic!("fixture broker identity");
        };
        let policy = Policy::default();
        let mut quota = QuotaPolicyV1::default();
        let mut decision = policy::evaluate(&policy, &request.admission.admission.decision_request);
        if refusal == AdmissionRefusalV1::Policy {
            decision = Decision::new(Outcome::Deny, "fixture", "operator test policy");
        } else {
            quota.max_active_workers = 0;
        }
        let refused = store
            .admit_legacy_named(&broker, &request, &policy, decision, Value::Null, &quota)
            .unwrap();
        assert_eq!(refused.admission.refusal, Some(refusal));
        assert!(!refused.admission.accepted && refused.admission.worker.is_none());
        assert!(refused.grant.is_none());
        let session = ResourceRefV1 {
            kind: ResourceKindV1::Session,
            resource_id: request.admission.admission.launch.session_id.clone(),
        };
        assert!(store.identity_resource_grant(&session).unwrap().is_none());
        assert_eq!(store.worker_quota(&quota).unwrap().held_workers, 0);
        assert_eq!(store.integrity().unwrap().head_hash, refused.audit_hash);

        let retry = store
            .admit_legacy_named(
                &broker,
                &request,
                &policy,
                policy::evaluate(&policy, &request.admission.admission.decision_request),
                Value::Null,
                &QuotaPolicyV1::default(),
            )
            .unwrap();
        assert!(retry.admission.replayed);
        assert_eq!(retry.admission.refusal, Some(refusal));
        assert_eq!(retry.admission.receipt, refused.admission.receipt);
        assert_eq!(store.integrity().unwrap().head_hash, refused.audit_hash);
    }
}

#[test]
fn legacy_named_admission_rolls_back_worker_and_grant_when_link_exceeds_audit_capacity() {
    let f = Fixture::new();
    let (store, _) = f.provision(f.create().unwrap()).unwrap();
    let (request, _) = f.confirmed_request(&store);
    let Some(AuthenticatedWorkload::Broker(broker)) =
        store.authenticate_workload_token(BROKER_TOKEN).unwrap()
    else {
        panic!("fixture broker identity");
    };
    let before = store.integrity().unwrap();
    let ordinary_limit = before.event_count + 2;
    let mut capacity = f.policy.clone();
    capacity.control_reserve.events =
        MAX_EVENTS - capacity.identity_reserve.events - ordinary_limit;
    capacity.human_tenant.events = ordinary_limit;
    capacity.human_principal.events = 1;
    let store = store.with_capacity_policy(capacity).unwrap();
    let quota = QuotaPolicyV1::default();
    let policy = Policy::default();
    let refusal = store
        .admit_legacy_named(
            &broker,
            &request,
            &policy,
            policy::evaluate(&policy, &request.admission.admission.decision_request),
            Value::Null,
            &quota,
        )
        .err()
        .expect("there is room for the reservation and grant but not their ordinary audit link");
    assert_eq!(
        refusal.downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted)
    );
    let after = store.integrity().unwrap();
    assert!(after.valid);
    assert_eq!(after.head_hash, before.head_hash);
    assert!(store.workers().unwrap().is_empty());
    assert_eq!(store.worker_quota(&quota).unwrap().held_workers, 0);
    assert!(
        store
            .identity_resource_grant(&ResourceRefV1 {
                kind: ResourceKindV1::Session,
                resource_id: request.admission.admission.launch.session_id,
            })
            .unwrap()
            .is_none()
    );
}

#[test]
fn legacy_named_admission_rolls_back_at_commit_when_human_deadline_is_reached() {
    let f = Fixture::new();
    let (store, _) = f.provision(f.create().unwrap()).unwrap();
    let (request, _) = f.confirmed_request(&store);
    let Some(AuthenticatedWorkload::Broker(broker)) =
        store.authenticate_workload_token(BROKER_TOKEN).unwrap()
    else {
        panic!("fixture broker identity");
    };
    let before = store.integrity().unwrap();
    *store.named_control_commit_observation.lock().unwrap() =
        Some(crate::broker_registry::parse_time(DEADLINE).unwrap());
    let policy = Policy::default();
    let quota = QuotaPolicyV1::default();
    let refusal = store
        .admit_legacy_named(
            &broker,
            &request,
            &policy,
            policy::evaluate(&policy, &request.admission.admission.decision_request),
            Value::Null,
            &quota,
        )
        .err()
        .expect("the retained human authority expires at the final observation");
    assert_eq!(
        refusal.downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::NotFound)
    );
    assert!(
        store
            .named_control_commit_observation
            .lock()
            .unwrap()
            .is_none()
    );
    assert!(store.supervision_state().unwrap().active);
    let after = store.integrity().unwrap();
    assert!(after.valid);
    assert_eq!(after.head_hash, before.head_hash);
    assert_eq!(store.worker_quota(&quota).unwrap().held_workers, 0);
    assert!(store.workers().unwrap().is_empty());
    assert!(
        store
            .identity_resource_grant(&ResourceRefV1 {
                kind: ResourceKindV1::Session,
                resource_id: request.admission.admission.launch.session_id,
            })
            .unwrap()
            .is_none()
    );
}

#[test]
fn fresh_bootstrap_provisions_at_the_live_head_and_reopen_cannot_construct() {
    let f = Fixture::new();
    let startup = f.create().unwrap();
    let initial = startup.store.supervision_state().unwrap();
    assert_eq!(initial.schema_version, 13);
    assert!(initial.active);
    assert_eq!(startup.store.max_events, MAX_EVENTS);
    assert_eq!(startup.store.physical_bytes(), MAX_BYTES);
    assert_eq!(startup.store.capacity_policy(), &f.policy);
    let head = startup.store.head().unwrap();
    assert!(
        startup
            .store
            .provision_legacy_session_target(f.operator.clone(), f.target.clone())
            .is_err()
    );
    assert_eq!(startup.store.head().unwrap(), head);
    {
        let reopened = f.reopen();
        let mut db = reopened.db.lock().unwrap();
        let tx = reopened.begin_checked(&mut db).unwrap();
        let error = startup.check(&tx).unwrap_err();
        assert_eq!(
            error.downcast_ref::<IdentityFailureV1>(),
            Some(&IdentityFailureV1::PermissionDenied)
        );
        assert_eq!(audit_head(&tx).unwrap().0, head);
    }

    let (store, receipt) = f.provision(startup).unwrap();
    let state = store.supervision_state().unwrap();
    assert!(state.active);
    assert_eq!(state.schema_version, initial.schema_version);
    assert_eq!(state.control_namespace_uuid, initial.control_namespace_uuid);
    assert_eq!(
        receipt.target_id,
        f.target.target.catalogue.launch_target_id
    );
    assert_eq!(receipt.audit_seq, head + 1);
    assert!(!receipt.replayed);
    assert!(store.workers().unwrap().is_empty());
    let integrity = store.integrity().unwrap();
    assert!(integrity.valid);
    assert_eq!(integrity.head_hash, receipt.audit_hash);
    drop(store);

    let reopened = f.reopen();
    let state = reopened.supervision_state().unwrap();
    assert!(!state.active);
    assert_eq!(state.control_namespace_uuid, initial.control_namespace_uuid);
    assert!(f.create().is_err());
    let after = reopened.integrity().unwrap();
    assert!(after.valid);
    assert_eq!(after.head_hash, receipt.audit_hash);
    assert_eq!(after.event_count, integrity.event_count);
}

#[test]
fn fresh_bootstrap_rejects_inconsistent_configuration_before_creating_files() {
    let mut f = Fixture::new();
    f.limits.bytes = DEFAULT_PHYSICAL_BYTES;
    let refusal = f.create().err().expect("policy exceeds the selected cap");
    assert!(refusal.to_string().contains("audit capacity policy"));
    assert_eq!(std::fs::read_dir(f._directory.path()).unwrap().count(), 0);

    f.limits.bytes = MAX_BYTES;
    f.grant.incarnation_id = Uuid::new_v4().to_string();
    let refusal = f
        .create()
        .err()
        .expect("binding is for another incarnation");
    assert_eq!(
        refusal.downcast_ref::<IdentityFailureV1>(),
        Some(&IdentityFailureV1::Conflict)
    );
    assert_eq!(std::fs::read_dir(f._directory.path()).unwrap().count(), 0);
}

#[test]
fn fresh_bootstrap_applies_ordinary_audit_limit_before_legacy_capability() {
    let mut f = Fixture::new();
    f.policy.control_reserve.events = MAX_EVENTS - f.policy.identity_reserve.events - 2;
    f.policy.human_tenant.events = 2;
    f.policy.human_principal.events = 1;
    let refusal = f
        .create()
        .err()
        .expect("two ordinary event slots are insufficient");
    assert_eq!(
        refusal.downcast_ref::<AuditCapacityFailureV1>(),
        Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted)
    );
    assert!(f.backups.join("before-prepare.sqlite").is_file());
    let store = f.reopen();
    assert!(store.integrity().unwrap().valid);
    assert!(
        store
            .events(&EventQuery {
                kind: Some(GENESIS_KIND.into()),
                limit: Some(1),
                ..Default::default()
            })
            .unwrap()
            .is_empty()
    );
}

#[test]
fn fresh_bootstrap_refuses_a_head_advanced_by_another_connection() {
    let f = Fixture::new();
    let startup = f.create().unwrap();
    let independent = f.reopen();
    let intervening = independent
        .append(Event::new("fixture.intervening"))
        .unwrap();
    let error = f.provision(startup).err().expect("stale construction head");
    assert_eq!(
        error.downcast_ref::<IdentityFailureV1>(),
        Some(&IdentityFailureV1::Conflict)
    );
    let after = independent.integrity().unwrap();
    assert!(after.valid);
    assert_eq!(after.head_hash, intervening.hash);
}
