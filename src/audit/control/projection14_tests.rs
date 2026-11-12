//! Nonempty histories use the real typed Store and maintenance migration.
//! Runtime observations remain declared fixtures; no engine or network is used.
use super::*;
use crate::{
    audit::{
        kubernetes_admission::KubernetesCreateAuthorization,
        kubernetes_session::KubernetesSessionReservationAuthorizationV2,
    },
    broker_registry::{
        BrokerAuthorityV1, BrokerGrantMutationV1, BrokerIdentityV1, BrokerRuntimeKindV1,
    },
    control::{kubernetes_admission::*, kubernetes_session::*},
    control_fs::{Directory, PrivateFile},
    human_authority::{
        HumanAuthorityConfigurationV1, HumanConfigurationFileKindV1, HumanConfigurationFileV1,
        HumanSourceKindV1, HumanStorageProfileV1, SourceStorageBindingV1,
        maintenance::{
            HumanMaintenanceResolvedViewV1, HumanMaintenanceSourceV1, HumanMaintenanceViewV1,
            HumanSourcesMaintenanceConfigurationV1, HumanSourcesMaintenanceGuard,
            HumanSourcesMaintenancePlanV1,
        },
    },
    policy::Policy,
    provider::{
        AuthMode, ChoiceSource, ClientKind, ClientTransport, ProviderChoiceProvenanceV1,
        ProviderResolutionV1,
    },
    relay::{RelayEndpointV1, admission::RelayChannelAdmissionV1},
    runtime::kubernetes::{
        KubernetesAuthorityV1, KubernetesPreflightResourcesV1, session_profile::*,
    },
    workload::{AuthenticatedWorkload, WorkloadBindingIssueV1, WorkloadSubjectV1},
};
use std::{ffi::OsStr, os::unix::fs::PermissionsExt, path::Path};
use uuid::Uuid;

// Reuse the established real admission/import/Arm/start producers. This module
// contains no tests, network calls or authority construction from a DTO.
#[path = "../../../tests/support/relay_admission_fixture.rs"]
mod relay_fixture;

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const BROKER_TOKEN: &str = "public-projection14-broker-token-0000000000000000";
const WORKER_TOKEN: &str = "public-projection14-worker-token-0000000000000000";
const EXPIRY: &str = "2100-01-01T00:00:00.000Z";
const MIB: u64 = 1_048_576;

struct Fixture {
    _temporary: tempfile::TempDir,
    root: Directory,
    store: Store,
    worker: WorkerV1,
    worker_binding: WorkloadBindingIssueV1,
    relay: RelayChannelAdmissionV1,
    broker: BrokerIdentityV1,
    create: KubernetesCreateIntentV1,
    create_authorization: KubernetesCreateAuthorization,
    create_seq: i64,
    session: KubernetesSessionReservationIntentV2,
    session_authorization: KubernetesSessionReservationAuthorizationV2,
    session_policy: KubernetesSessionReservationPolicyV2,
    session_seq: i64,
    source: HumanMaintenanceSourceV1,
    plan: HumanSourcesMaintenancePlanV1,
    plan_file: PrivateFile,
    configuration_files: Vec<(HumanConfigurationFileKindV1, PrivateFile)>,
}

fn storage_binding(parent: &Directory, name: &str) -> SourceStorageBindingV1 {
    let (parent_device, parent_inode) = parent.physical_identity().unwrap();
    let (source_device, source_inode) = parent
        .private_entry(OsStr::new(name))
        .unwrap()
        .unwrap()
        .physical_identity();
    SourceStorageBindingV1 {
        parent_path: parent.path().to_owned(),
        file_name: name.into(),
        parent_device,
        parent_inode,
        source_device,
        source_inode,
    }
}

impl Fixture {
    fn ordinary13() -> Self {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        let root = Directory::open_existing(temporary.path()).unwrap();
        let source_parent = root.persistent_child("source", true).unwrap().unwrap();
        let store = relay_fixture::fresh(source_parent.path(), "tenant-a", 10_000);
        let worker = relay_fixture::running_now(&store);
        assert_eq!(worker.state, WorkerStateV1::Running);
        let (credential_id, revision) = relay_fixture::bind(&store, &worker, WORKER_TOKEN);
        // Recover the exact issuer input recorded by the existing typed helper,
        // including its original expiry, for the later identical retry oracle.
        let worker_binding: WorkloadBindingIssueV1 = serde_json::from_value(
            store
                .events(&EventQuery {
                    kind: Some("control.workload.issue".into()),
                    ..Default::default()
                })
                .unwrap()
                .into_iter()
                .find(|stored| {
                    stored.event.data["change"]["input"]["credential_id"] == credential_id
                })
                .unwrap()
                .event
                .data["change"]["input"]
                .clone(),
        )
        .unwrap();
        let relay = serde_json::from_value(relay_fixture::request(
            &store,
            &worker,
            &credential_id,
            revision,
            WORKER_TOKEN,
        ))
        .unwrap();
        assert!(!store.reserve_relay_channel(&relay).unwrap().replayed);
        let relay_head = store.head().unwrap();
        assert!(store.reserve_relay_channel(&relay).unwrap().replayed);
        assert!(
            store
                .issue_workload_binding(&worker_binding)
                .unwrap()
                .replayed
        );
        assert_eq!(store.head().unwrap(), relay_head);

        for target in 7..=11 {
            store
                .migrate_control_version(
                    &source_parent.path().join(format!("before-{target}.sqlite")),
                    target - 1,
                    target,
                )
                .unwrap();
        }
        let authority = KubernetesAuthorityV1::new(
            Uuid::new_v4(),
            "https://127.0.0.1:6443",
            "11111111-1111-4111-8111-111111111111",
            "tenant-a",
            "22222222-2222-4222-8222-222222222222",
            HASH_A,
        )
        .unwrap();
        let broker_id = Uuid::new_v4().to_string();
        let incarnation_id = Uuid::new_v4().to_string();
        let broker_credential = Uuid::new_v4().to_string();
        store
            .issue_workload_binding(&WorkloadBindingIssueV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                credential_id: broker_credential.clone(),
                expected_revision: 0,
                subject: WorkloadSubjectV1::Broker {
                    broker_id: broker_id.clone(),
                    incarnation_id: incarnation_id.clone(),
                },
                sha256_digest: hex::encode(Sha256::digest(BROKER_TOKEN.as_bytes())),
                expires_at: EXPIRY.into(),
            })
            .unwrap();
        store
            .mutate_broker_grant(&BrokerGrantMutationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                broker_id: broker_id.clone(),
                expected_revision: None,
                credential_id: broker_credential,
                incarnation_id: incarnation_id.clone(),
                authorities: vec![BrokerAuthorityV1 {
                    authority_id: authority.authority_id().to_string(),
                    runtime_kind: BrokerRuntimeKindV1::Kubernetes,
                }],
                lease_seconds: 120,
                not_after: EXPIRY.into(),
            })
            .unwrap();
        let Some(AuthenticatedWorkload::Broker(broker)) =
            store.authenticate_workload_token(BROKER_TOKEN).unwrap()
        else {
            panic!("real binding and grant must authenticate the fixture broker")
        };
        let namespace = store
            .supervision_state()
            .unwrap()
            .control_namespace_uuid
            .unwrap();
        let create = KubernetesCreateIntentV1 {
            version: 1,
            create_id: Uuid::new_v4().to_string(),
            session_id: Uuid::new_v4().to_string(),
            control_namespace_uuid: namespace.clone(),
            broker_id: broker_id.clone(),
            incarnation_id: incarnation_id.clone(),
            authority: authority.clone(),
            profile: KUBERNETES_HELD_PROFILE_V1.into(),
            image: format!("worker@sha256:{HASH_B}"),
            resources: KubernetesPreflightResourcesV1 {
                cpu_millis: 500,
                memory_mib: 128,
                workspace_mib: 8,
                temporary_mib: 8,
                deadline_seconds: 60,
            },
        };
        let quota = QuotaPolicyV1::default();
        let create_policy = KubernetesHeldCreatePolicyV1 {
            version: 1,
            decision_ttl_seconds: 60,
            approvals: vec![KubernetesHeldCreateApprovalV1 {
                authority_fingerprint: authority.fingerprint(),
                image: create.image.clone(),
                profile: create.profile.clone(),
            }],
        };
        let create_authorization = store
            .authorize_kubernetes_create_admission(&broker, &create, &create_policy, &quota)
            .unwrap();
        let (create_receipt, create_snapshot, replayed) = store
            .reserve_kubernetes_create(&broker, &create_authorization, &create)
            .unwrap();
        assert!(!replayed && create_snapshot.quota_held);
        let create_head = store.head().unwrap();
        assert!(
            store
                .reserve_kubernetes_create(&broker, &create_authorization, &create)
                .unwrap()
                .2
        );
        assert_eq!(store.head().unwrap(), create_head);
        let (session, session_authorization, session_policy, session_seq) =
            reserve_session(&store, &broker, &create, &quota);
        for target in 12..=13 {
            store
                .migrate_control_version(
                    &source_parent.path().join(format!("before-{target}.sqlite")),
                    target - 1,
                    target,
                )
                .unwrap();
        }
        assert!(store.integrity().unwrap().valid);
        let source = HumanMaintenanceSourceV1 {
            source_id: Uuid::new_v4(),
            kind: HumanSourceKindV1::TenantStore,
            installation_id: Uuid::new_v4(),
            durable_namespace_id: review_authority::namespace(&store.db.lock().unwrap()).unwrap(),
            tenant_id: Some("tenant-a".into()),
            storage_binding: storage_binding(&source_parent, "tenant-a.sqlite"),
            required_format: 14,
        };
        let configuration_parent = root
            .persistent_child("configuration", true)
            .unwrap()
            .unwrap();
        let control = configuration_parent
            .persistent_create("control.json", b"synthetic projection14 configuration")
            .unwrap();
        let credentials = configuration_parent
            .persistent_create(
                "credentials.json",
                b"synthetic projection14 credential reference",
            )
            .unwrap();
        let configuration = HumanAuthorityConfigurationV1 {
            control_config_revision: 1,
            public_config_digest: "ac".repeat(32),
            oidc: None,
            files: vec![
                HumanConfigurationFileV1 {
                    kind: HumanConfigurationFileKindV1::ControlConfiguration,
                    storage_binding: storage_binding(&configuration_parent, "control.json"),
                },
                HumanConfigurationFileV1 {
                    kind: HumanConfigurationFileKindV1::TechnicalCredentials,
                    storage_binding: storage_binding(&configuration_parent, "credentials.json"),
                },
            ],
        };
        let plan = HumanSourcesMaintenancePlanV1 {
            version: 1,
            migration_id: Uuid::new_v4(),
            installation_id: source.installation_id,
            prior: None,
            target: HumanMaintenanceViewV1 {
                sources: vec![source.clone()],
                configuration,
            },
            prior_active_manifest: None,
            supported_storage_profile: HumanStorageProfileV1::LocalPosixFlockV1,
        };
        let plan_file = configuration_parent
            .persistent_create("maintenance.json", &serde_json::to_vec(&plan).unwrap())
            .unwrap();
        Self {
            _temporary: temporary,
            root,
            store,
            worker,
            worker_binding,
            relay,
            broker,
            create,
            create_authorization,
            create_seq: create_receipt.audit_seq,
            session,
            session_authorization,
            session_policy,
            session_seq,
            source,
            plan,
            plan_file,
            configuration_files: vec![
                (HumanConfigurationFileKindV1::ControlConfiguration, control),
                (
                    HumanConfigurationFileKindV1::TechnicalCredentials,
                    credentials,
                ),
            ],
        }
    }

    fn migrate14(&self) {
        let resolved = HumanMaintenanceResolvedViewV1::new(
            self.plan.target.sources.clone(),
            self.plan.target.configuration.clone(),
            self.configuration_files.clone(),
        )
        .unwrap();
        let guard = HumanSourcesMaintenanceGuard::acquire(
            self.plan_file.clone(),
            HumanSourcesMaintenanceConfigurationV1::new(None, resolved, None),
        )
        .unwrap();
        let token = guard.target_source(self.source.source_id).unwrap();
        let mut db = self.store.db.lock().unwrap();
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let backup_path = self
            .root
            .persistent_child("backups", true)
            .unwrap()
            .unwrap()
            .path()
            .join("before14.sqlite");
        let backup = self
            .store
            .prepare_review_source_backup(&tx, &token, &backup_path)
            .unwrap();
        self.store
            .install_review_source_format14_in_transaction(&tx, &token, &backup)
            .unwrap();
        tx.commit().unwrap();
    }
}

fn reserve_session(
    store: &Store,
    broker: &BrokerIdentityV1,
    create: &KubernetesCreateIntentV1,
    quota: &QuotaPolicyV1,
) -> (
    KubernetesSessionReservationIntentV2,
    KubernetesSessionReservationAuthorizationV2,
    KubernetesSessionReservationPolicyV2,
    i64,
) {
    let binding = KubernetesSessionBindingV2::new(
        &create.authority,
        KubernetesSessionIdsV2 {
            installation_id: Uuid::new_v4(),
            control_namespace_uuid: create.control_namespace_uuid.parse().unwrap(),
            broker_id: create.broker_id.parse().unwrap(),
            incarnation_id: create.incarnation_id.parse().unwrap(),
            session_id: Uuid::new_v4(),
            attempt_id: Uuid::new_v4(),
        },
    )
    .unwrap();
    let provider = ProviderResolutionV1 {
        version: 1,
        client: ClientKind::Codex,
        auth_mode: AuthMode::Api,
        transport: ClientTransport::Batch,
        profile_id: "default".into(),
        profile_revision: 1,
        model: None,
        credential_binding_id: Uuid::new_v4(),
        credential_names: vec![crate::provider::BOOTSTRAP_ENV.into()],
        invocation_preset_version: 1,
        provenance: ProviderChoiceProvenanceV1 {
            client: ChoiceSource::Cli,
            auth_mode: ChoiceSource::Cli,
            profile: ChoiceSource::Cli,
            model: ChoiceSource::Cli,
        },
    };
    let args = [
        "exec",
        "--client",
        "codex",
        "--auth-mode",
        "api",
        "--transport",
        "batch",
        "--preset-version",
        "1",
        "--prompt",
        "public projection14 quota fixture",
    ]
    .map(str::to_owned);
    let profile = KubernetesSessionProfileV2::build(
        &create.authority,
        &binding,
        &create.image,
        KubernetesSessionResourcesV2 {
            cpu_millis: 500,
            memory_mib: 128,
            temporary_mib: 8,
            home_mib: 8,
            deadline_seconds: 60,
        },
        &provider,
        KubernetesSessionInvocationV2::new(crate::provider::BOOTSTRAP_PROGRAM, &args).unwrap(),
        &crate::workspace::Manifest::new(),
    )
    .unwrap();
    let intent = KubernetesSessionReservationIntentV2::new(
        &Uuid::new_v4().to_string(),
        &Uuid::new_v4().to_string(),
        &create.authority,
        &profile,
    )
    .unwrap();
    let policy = KubernetesSessionReservationPolicyV2 {
        version: 2,
        decision_ttl_seconds: 60,
        approvals: vec![KubernetesSessionReservationApprovalV2 {
            authority_id: create.authority.authority_id().to_string(),
            authority_fingerprint: create.authority.fingerprint(),
            image: create.image.clone(),
            client: ClientKind::Codex,
            auth_mode: AuthMode::Api,
            transport: ClientTransport::Batch,
            profile_id: "default".into(),
            profile_revision: 1,
        }],
    };
    let authorization = store
        .authorize_kubernetes_session_reservation(broker, &intent, &policy, quota)
        .unwrap();
    let result = store
        .reserve_kubernetes_session(broker, &authorization, &intent, &policy, quota)
        .unwrap();
    assert!(!result.replayed && result.snapshot.state.quota_held);
    let head = store.head().unwrap();
    assert!(
        store
            .reserve_kubernetes_session(broker, &authorization, &intent, &policy, quota)
            .unwrap()
            .replayed
    );
    assert_eq!(store.head().unwrap(), head);
    (intent, authorization, policy, result.receipt.audit_seq)
}

fn usage_tuple(usage: QuotaUsageV1) -> (u32, u64, u64) {
    (
        usage.held_workers,
        usage.held_cpu_millis,
        usage.held_memory_bytes,
    )
}

fn history(db: &Connection, through: i64) -> Vec<Vec<rusqlite::types::Value>> {
    db.prepare(
        "SELECT seq,id,received_at,prev_hash,hash,payload,kind FROM events WHERE seq<=?1 ORDER BY seq",
    )
    .unwrap()
    .query_map([through], |row| (0..7).map(|i| row.get(i)).collect())
    .unwrap()
    .collect::<rusqlite::Result<Vec<_>>>()
    .unwrap()
}

fn complete_replay(db: &Connection) -> anyhow::Result<()> {
    validate_schema(db, false)?;
    validate(db)?;
    review_format14::validate(db)?;
    anyhow::ensure!(Store::check_integrity(db)?.valid, "invalid audit/floors");
    Ok(())
}

#[test]
fn format14_replays_nonempty_worker_workload_relay_and_shared_kubernetes_quota() {
    let f = Fixture::ordinary13();
    let quota = QuotaPolicyV1::default();
    let before = audit_head(&f.store.db.lock().unwrap()).unwrap();
    let prefix = history(&f.store.db.lock().unwrap(), before.0);
    assert_eq!(
        usage_tuple(f.store.worker_quota(&quota).unwrap()),
        (3, 3000, 1280 * MIB)
    );
    f.migrate14();
    let mut db = Connection::open_with_flags(
        f.store.backing.path(),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    assert_eq!(history(&db, before.0), prefix);
    let retained: (i64, i64, i64, i64, i64) = db.query_row(
        "SELECT (SELECT COUNT(*) FROM workers),(SELECT COUNT(*) FROM workload_bindings),(SELECT COUNT(*) FROM events WHERE kind='relay_channel.v1'),(SELECT COUNT(*) FROM kubernetes_creates),(SELECT COUNT(*) FROM kubernetes_session_reservations)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    ).unwrap();
    assert_eq!(retained, (1, 2, 1, 1, 1));
    assert_eq!(
        usage_tuple(quota_usage(&db, &quota).unwrap()),
        (3, 3000, 1280 * MIB)
    );
    assert_eq!(
        usage_tuple(quota_usage_before(&db, f.create_seq, &quota).unwrap()),
        (1, 2000, 1024 * MIB)
    );
    assert_eq!(
        usage_tuple(quota_usage_before(&db, f.session_seq, &quota).unwrap()),
        (2, 2500, 1152 * MIB)
    );
    assert_eq!(
        usage_tuple(quota_usage_before(&db, before.0 + 1, &quota).unwrap()),
        (3, 3000, 1280 * MIB)
    );
    assert_eq!(
        serde_json::to_value(
            worker_at(&db, &f.worker.worker_id, f.create_seq)
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&f.worker).unwrap()
    );
    complete_replay(&db).unwrap();
    for sql in [
        "UPDATE workers SET cpu_millis=cpu_millis+1",
        "UPDATE kubernetes_creates SET cpu_millis=cpu_millis+1",
        "UPDATE kubernetes_session_reservations SET cpu_millis=cpu_millis+1",
    ] {
        let tx = db.transaction().unwrap();
        assert_eq!(
            tx.execute(sql, []).unwrap(),
            1,
            "negative must change a real retained row"
        );
        assert!(complete_replay(&tx).is_err(), "{sql}");
        tx.rollback().unwrap();
    }
    complete_replay(&db).unwrap();
    let tx = db.transaction().unwrap();
    // This is only a negative against a genuinely migrated14 fixture. Future
    // versions cannot acquire the14 projections through an open-ended range.
    tx.pragma_update(None, "user_version", 15).unwrap();
    assert!(worker_at(&tx, &f.worker.worker_id, before.0 + 1).is_err());
    assert!(quota_usage_before(&tx, before.0 + 1, &quota).is_err());
    assert!(quota_usage(&tx, &quota).is_err());
    tx.rollback().unwrap();
}

#[test]
fn format14_nonempty_history_keeps_legacy_reads_and_identical_retries_fenced() {
    let f = Fixture::ordinary13();
    let quota = QuotaPolicyV1::default();
    // The fixture already observed exact retries immediately after their real
    // commits. Do not make this oracle depend on their short live TTLs surviving
    // all subsequent migrations on a loaded QA machine.
    f.migrate14();
    let before = audit_head(&f.store.db.lock().unwrap()).unwrap();
    let refused = [
        f.store.worker(&f.worker.worker_id).unwrap_err(),
        f.store.workers_page(None, 10).unwrap_err(),
        f.store.worker_quota(&quota).unwrap_err(),
        f.store.reserve_relay_channel(&f.relay).unwrap_err(),
        f.store
            .issue_workload_binding(&f.worker_binding)
            .unwrap_err(),
        f.store
            .reserve_kubernetes_create(&f.broker, &f.create_authorization, &f.create)
            .unwrap_err(),
        f.store
            .reserve_kubernetes_session(
                &f.broker,
                &f.session_authorization,
                &f.session,
                &f.session_policy,
                &quota,
            )
            .unwrap_err(),
    ];
    for error in refused {
        assert_eq!(
            error.downcast_ref::<review_authority::ReviewAuthorityFailure>(),
            Some(&review_authority::ReviewAuthorityFailure::Required)
        );
    }
    let db = f.store.db.lock().unwrap();
    assert!(require_control(&db).is_err());
    assert_eq!(audit_head(&db).unwrap(), before);
    assert_eq!(
        usage_tuple(quota_usage(&db, &quota).unwrap()),
        (3, 3000, 1280 * MIB)
    );
    complete_replay(&db).unwrap();
    assert!(f.store.healthy());
}
