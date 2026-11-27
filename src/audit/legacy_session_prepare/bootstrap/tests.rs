use super::*;
use crate::{
    broker_registry::BrokerAuthorityV1,
    control::{
        LaunchSpecV1, NetworkV1, RestartPolicyV1, SecurityPresetV1,
        session_control::{LegacySessionTargetV1, SessionLaunchTargetRecordV1},
    },
    model::CommandSpec,
};
use sha2::{Digest, Sha256};
use std::{os::unix::fs::PermissionsExt, path::PathBuf};

const TENANT: &str = "tenant-fresh";
const MAX_EVENTS: u64 = 1000;
const MAX_BYTES: u64 = 2 * DEFAULT_PHYSICAL_BYTES;
const DEADLINE: &str = "2126-01-01T00:00:00Z";

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
                sha256_digest: hex::encode(Sha256::digest(b"synthetic bootstrap broker")),
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
