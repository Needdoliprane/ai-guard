use super::*;
use crate::{
    broker_registry::BrokerAuthorityV1,
    control::{
        AdmissionRefusalV1, AdmissionV1, ControlFailure, DecisionPhaseV1, DecisionReceiptV1,
        LaunchSpecV1, NetworkV1, QuotaPolicyV1, RestartPolicyV1, SecurityPresetV1,
        SupervisedAdmissionV1, SupervisionProtocolV1,
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
        let mut launch = self.target.target.launch.clone();
        launch.session_id = confirmation.session_id;
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
        let request = LegacyNamedSupervisedAdmissionV1 {
            version: 1,
            binding_ref: Uuid::parse_str(&confirmation.binding_ref).unwrap(),
            broker_profile_id: self.target.target.broker_profile_id.clone(),
            admission: SupervisedAdmissionV1 {
                version: 1,
                protocol: SupervisionProtocolV1::BrokerSupervisionV1,
                intent_hash: "c".repeat(64),
                admission: AdmissionV1 {
                    version: 1,
                    operation_id: confirmation.operation_id.to_string(),
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
            },
        };
        (request, credential)
    }
}

#[test]
fn legacy_named_admission_commits_a_grant_and_replays_after_reopen() {
    let f = Fixture::new();
    let (store, _) = f.provision(f.create().unwrap()).unwrap();
    let (request, mut initiator) = f.confirmed_request(&store);
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
    let after_revocation = store.integrity().unwrap().head_hash;
    drop(store);

    let reopened = f.reopen();
    assert!(!reopened.supervision_state().unwrap().active);
    *reopened.named_admission_commit_observation.lock().unwrap() =
        Some(crate::broker_registry::parse_time(DEADLINE).unwrap());
    assert_eq!(
        apply(&reopened, &request)
            .err()
            .unwrap()
            .downcast_ref::<WorkloadFailureV1>(),
        Some(&WorkloadFailureV1::Unauthorized)
    );
    assert!(reopened.integrity().unwrap().valid);
    assert_eq!(reopened.integrity().unwrap().head_hash, after_revocation);
    let replay = apply(&reopened, &request).unwrap();
    assert!(replay.admission.replayed);
    assert_eq!(replay.admission.receipt, committed.admission.receipt);
    assert_eq!(replay.grant, committed.grant);
    assert_eq!(replay.audit_hash, committed.audit_hash);
    assert_eq!(reopened.integrity().unwrap().head_hash, after_revocation);
    assert_eq!(reopened.worker_quota(&quota).unwrap().held_workers, 1);
    assert!(apply(&reopened, &divergent).is_err());
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
    *store.named_admission_commit_observation.lock().unwrap() =
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
            .named_admission_commit_observation
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
