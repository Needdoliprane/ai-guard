//! Real private SQLite files. Positive14/5 states only come from actual
//! backup/install/commit APIs. These tests are source-only until admitted QA.
use super::*;
use crate::{
    audit::{
        ReviewStoreMaintenance,
        authentication::{
            ApprovedAuthenticationClientV1, AuthenticationActionV1, AuthenticationAuditFailureV1,
            AuthenticationCapacityPlanV1, AuthenticationCapacityV1, AuthenticationPhaseV1,
            AuthenticationRecordV1, AuthenticationSourceAccess, AuthenticationTenantCapacityV1,
            AuthenticationTokenFingerprintV1,
        },
    },
    human_authority::{
        HumanAuthorityConfigurationV1, HumanConfigurationFileV1, HumanOidcConfigurationV1,
        HumanStorageProfileV1, SourceStorageBindingV1,
        maintenance::{
            HumanMaintenanceResolvedViewV1, HumanMaintenanceSourceV1, HumanMaintenanceViewV1,
        },
    },
    identity::{
        HumanAccountV1, HumanCapabilityV1, IdentityChangeV1, IdentityMutationV1,
        LegacyControlRoleV1, LocalCredentialV1, MembershipV1, PrincipalRefV1,
    },
};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::{ffi::OsStr, os::unix::fs::PermissionsExt, path::Path};

pub(crate) struct Fixture {
    _temporary: tempfile::TempDir,
    plan: HumanSourcesMaintenancePlanV1,
    plan_file: PrivateFile,
    files: Vec<(HumanConfigurationFileKindV1, PrivateFile)>,
    policy: AuthenticationAuditPolicyV1,
    capacity: AuthenticationCapacityPlanV1,
    backup_paths: BTreeMap<Uuid, PathBuf>,
    journal: Directory,
    closure_id: Uuid,
    legacy: Vec<Store>,
    legacy_auth: AuthenticationAudit,
    account: PrincipalRefV1,
    retry: IdentityMutationV1,
}
fn private_child(parent: &Directory, name: &str) -> Directory {
    parent.persistent_child(name, true).unwrap().unwrap()
}
fn binding(parent: &Directory, name: &str) -> SourceStorageBindingV1 {
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
fn read(path: &Path) -> Connection {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    db.busy_timeout(std::time::Duration::ZERO).unwrap();
    db
}
fn raw_format(source: &HumanMaintenanceSourceV1) -> u32 {
    read(&source.storage_binding.path())
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap()
}
fn identity_change(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "synthetic-trusted-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    }
}
fn history(source: &HumanMaintenanceSourceV1) -> Vec<(i64, String)> {
    assert_eq!(source.kind, HumanSourceKindV1::TenantStore);
    let db = read(&source.storage_binding.path());
    let mut query = db
        .prepare("SELECT seq,hash FROM events ORDER BY seq")
        .unwrap();
    query
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
fn identity_projection(source: &HumanMaintenanceSourceV1) -> Vec<String> {
    assert_eq!(source.kind, HumanSourceKindV1::TenantStore);
    let db = read(&source.storage_binding.path());
    let mut query = db.prepare("SELECT payload_json FROM identity_credentials UNION ALL SELECT payload_json FROM identity_memberships ORDER BY payload_json").unwrap();
    query
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

impl Fixture {
    pub(crate) fn new() -> Self {
        Self::new_configured(false, true, false)
    }
    pub(crate) fn new_owned() -> Self {
        Self::new_configured(true, true, false)
    }
    pub(crate) fn new_owned_local() -> Self {
        Self::new_configured(true, false, false)
    }
    /// Legacy stores that receive the human rights marker before adoption,
    /// as the identity guide asks of an operator.
    pub(crate) fn new_owned_with_human_rights() -> Self {
        Self::new_configured(true, true, true)
    }
    pub(crate) fn bearer(tenant: &str) -> String {
        format!(
            "aigh1_{}",
            if tenant == "A" { "11" } else { "22" }.repeat(32)
        )
    }
    fn new_configured(owned: bool, oidc: bool, human_rights: bool) -> Self {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        let root = Directory::open_existing(temporary.path()).unwrap();
        let installation = Uuid::new_v4();
        let closure_id = Uuid::new_v4();
        let account = PrincipalRefV1::HumanLocal {
            installation_id: installation.to_string(),
            account_id: Uuid::new_v4().to_string(),
        };
        let mut legacy = Vec::new();
        let mut descriptors = Vec::new();
        let mut retry = None;
        for (index, tenant) in ["A", "B"].into_iter().enumerate() {
            let parent = private_child(&root, &format!("tenant-{tenant}"));
            let store = Store::open_for_tenant(
                &parent.path().join("tenant.sqlite"),
                tenant,
                Redactor::new(vec![]),
                10_000,
            )
            .unwrap();
            let historical = private_child(&root, &format!("historical-{tenant}"));
            let migrations = [
                Store::migrate_control_v3,
                Store::migrate_identity_v4,
                Store::migrate_human_access_v5,
                Store::migrate_workload_v6,
                Store::migrate_human_brokers_v7,
                Store::migrate_kubernetes_create_v8,
                Store::migrate_kubernetes_policy_v9,
                Store::migrate_human_workers_v10,
                Store::migrate_kubernetes_session_reservations_v11,
                Store::migrate_broker_checkpoints_v12,
                Store::migrate_kubernetes_recovery_v13,
            ];
            for (offset, migrate) in migrations.into_iter().enumerate() {
                migrate(
                    &store,
                    &historical
                        .path()
                        .join(format!("before-{}.sqlite", offset + 3)),
                )
                .unwrap();
            }
            if human_rights {
                store
                    .migrate_human_rights_v2(&historical.path().join("before-rights.sqlite"))
                    .unwrap();
            }
            let change = identity_change(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: account.clone(),
                    display_name: "Synthetic local person".into(),
                    revision: 1,
                    active: true,
                },
            });
            store.mutate_identity(&change).unwrap();
            if index == 0 {
                retry = Some(change);
            }
            store
                .mutate_identity(&identity_change(IdentityChangeV1::Credential {
                    expected_revision: 0,
                    binding: LocalCredentialV1 {
                        version: 1,
                        credential_id: Uuid::new_v4().to_string(),
                        principal: account.clone(),
                        sha256_digest: if owned {
                            use sha2::Digest;
                            hex::encode(sha2::Sha256::digest(Self::bearer(tenant).as_bytes()))
                        } else {
                            "ad".repeat(32)
                        },
                        revision: 1,
                        active: true,
                        expires_at: None,
                    },
                }))
                .unwrap();
            store
                .mutate_identity(&identity_change(IdentityChangeV1::Membership {
                    expected_revision: 0,
                    membership: MembershipV1 {
                        version: 1,
                        membership_id: Uuid::new_v4().to_string(),
                        principal: account.clone(),
                        tenant_id: tenant.into(),
                        organization_scope: "synthetic-org".into(),
                        revision: 1,
                        active: true,
                        capabilities: BTreeSet::from([
                            HumanCapabilityV1::ReadMetadata,
                            HumanCapabilityV1::ReadContent,
                        ]),
                    },
                }))
                .unwrap();
            let namespace: String = read(&parent.path().join("tenant.sqlite"))
                .query_row(
                    "SELECT namespace_uuid FROM control_meta WHERE singleton=1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            descriptors.push(HumanMaintenanceSourceV1 {
                source_id: Uuid::from_u128(index as u128 + 1),
                kind: HumanSourceKindV1::TenantStore,
                installation_id: installation,
                durable_namespace_id: Uuid::parse_str(&namespace).unwrap(),
                tenant_id: Some(tenant.into()),
                storage_binding: binding(&parent, "tenant.sqlite"),
                required_format: 14,
            });
            legacy.push(store);
        }
        let auth_parent = private_child(&root, "authentication");
        let policy = AuthenticationAuditPolicyV1 {
            max_records: 128,
            max_serialized_bytes: 3_000_000,
            approved_clients: vec![ApprovedAuthenticationClientV1 {
                issuer: "https://identity.example.test/realm".into(),
                client_id: "synthetic-public-client".into(),
            }],
        };
        let capacity = AuthenticationCapacityPlanV1 {
            version: 1,
            unscoped: AuthenticationCapacityV1 {
                max_records: 24,
                max_serialized_bytes: 500_000,
            },
            tenants: ["A", "B"]
                .into_iter()
                .map(|tenant| AuthenticationTenantCapacityV1 {
                    tenant_id: tenant.into(),
                    capacity: AuthenticationCapacityV1 {
                        max_records: 32,
                        max_serialized_bytes: 1_000_000,
                    },
                })
                .collect(),
        };
        let legacy_auth = AuthenticationAudit::init_new_v4(
            &auth_parent.path().join("authentication.sqlite"),
            installation,
            policy.clone(),
            capacity.clone(),
            Redactor::new(vec![]),
        )
        .unwrap();
        if oidc {
            descriptors.push(HumanMaintenanceSourceV1 {
                source_id: Uuid::from_u128(3),
                kind: HumanSourceKindV1::AuthenticationLedger,
                installation_id: installation,
                durable_namespace_id: legacy_auth.verify().unwrap().namespace_id,
                tenant_id: None,
                storage_binding: binding(&auth_parent, "authentication.sqlite"),
                required_format: 5,
            });
        }
        let configuration_parent = private_child(&root, "configuration");
        let mut files = Vec::new();
        let mut config_descriptors = Vec::new();
        for (index, kind) in [
            HumanConfigurationFileKindV1::ControlConfiguration,
            HumanConfigurationFileKindV1::TechnicalCredentials,
            HumanConfigurationFileKindV1::OidcConfiguration,
            HumanConfigurationFileKindV1::OidcTrustBundle,
        ]
        .into_iter()
        .enumerate()
        {
            if !oidc
                && matches!(
                    kind,
                    HumanConfigurationFileKindV1::OidcConfiguration
                        | HumanConfigurationFileKindV1::OidcTrustBundle
                )
            {
                continue;
            }
            let name = format!("config-{index}.json");
            let bytes = if owned && kind == HumanConfigurationFileKindV1::TechnicalCredentials {
                let entries=["A","B"].into_iter().flat_map(|tenant|["runner","viewer"].into_iter().map(move |role|serde_json::json!({"tenant_id":tenant,"role":role,"credential_id":Uuid::new_v4(),"generation":1,"token":format!("synthetic-{tenant}-{role}-credential-long-enough")}))).collect::<Vec<_>>();
                serde_json::to_vec(&serde_json::json!({"version":1,"installation_id":installation,"closure_id":closure_id,"closure_revision":1,"control_config_public_digest":"ab".repeat(32),"entries":entries})).unwrap()
            } else {
                b"private synthetic resolved configuration".to_vec()
            };
            files.push((
                kind,
                configuration_parent
                    .persistent_create(&name, &bytes)
                    .unwrap(),
            ));
            config_descriptors.push(HumanConfigurationFileV1 {
                kind,
                storage_binding: binding(&configuration_parent, &name),
            });
        }
        let configuration = HumanAuthorityConfigurationV1 {
            control_config_revision: 1,
            public_config_digest: "ab".repeat(32),
            oidc: if oidc {
                Some(HumanOidcConfigurationV1 {
                    public_config_digest: "cd".repeat(32),
                    mapping_revision: 1,
                })
            } else {
                None
            },
            files: config_descriptors,
        };
        let plan = HumanSourcesMaintenancePlanV1 {
            version: 1,
            migration_id: Uuid::new_v4(),
            installation_id: installation,
            prior: None,
            target: HumanMaintenanceViewV1 {
                sources: descriptors,
                configuration,
            },
            prior_active_manifest: None,
            supported_storage_profile: HumanStorageProfileV1::LocalPosixFlockV1,
            selected_source_id: None,
        };
        let plan_file = configuration_parent
            .persistent_create("maintenance.json", &serde_json::to_vec(&plan).unwrap())
            .unwrap();
        let backup_paths = plan
            .target
            .sources
            .iter()
            .map(|s| {
                (
                    s.source_id,
                    private_child(&root, &format!("backup-{}", s.source_id))
                        .path()
                        .join("prior.sqlite"),
                )
            })
            .collect();
        let journal = private_child(&root, "progress");
        Self {
            _temporary: temporary,
            plan,
            plan_file,
            files,
            policy,
            capacity,
            backup_paths,
            journal,
            closure_id,
            legacy,
            legacy_auth,
            account,
            retry: retry.unwrap(),
        }
    }
    /// Test-only, real full-union migration; no schema/authority is fabricated.
    pub(crate) fn migrated(&self) -> MigratedHumanSources {
        migrate_human_sources(self.request()).unwrap()
    }
    pub(crate) fn owned_startup(
        &self,
    ) -> (
        crate::human_authority::startup::HumanOwnedStartupBundle,
        PrivateFile,
        Vec<(String, String, String)>,
    ) {
        let (request, credentials, tokens) = self.owned_request();
        (
            crate::human_authority::startup::HumanOwnedStartupBundle::open(request).unwrap(),
            credentials,
            tokens,
        )
    }
    pub(crate) fn owned_request(
        &self,
    ) -> (
        crate::human_authority::startup::HumanOwnedStartupRequest,
        PrivateFile,
        Vec<(String, String, String)>,
    ) {
        use crate::human_authority::startup::*;
        let migrated = self.migrated();
        let manifest = migrated.manifest().clone();
        let closure = migrated.owner().closure().clone();
        drop(migrated);
        let configuration = HumanSourcesConfigurationV1::new(
            closure.sources.clone(),
            closure.configuration.clone(),
            self.files.clone(),
        )
        .unwrap();
        let redactor = Redactor::new(
            ["A", "B"]
                .into_iter()
                .flat_map(|tenant| {
                    ["runner", "viewer"].into_iter().map(move |role| {
                        format!("synthetic-{tenant}-{role}-credential-long-enough")
                    })
                })
                .collect(),
        );
        let sources = closure
            .sources
            .iter()
            .map(|source| {
                (
                    source.source_id,
                    match source.kind {
                        HumanSourceKindV1::TenantStore => {
                            let capacity = crate::audit::AuditCapacityPolicyV1::resolved(10_000);
                            HumanOwnedSourceOptions::TenantStore {
                                redactor: redactor.clone(),
                                max_events: 10_000,
                                workload: crate::audit::WorkloadAuditPolicyV1::resolved(
                                    10_000, &capacity,
                                ),
                                capacity,
                                retention: crate::relay::RelayRetentionPolicyV1::default(),
                            }
                        }
                        HumanSourceKindV1::AuthenticationLedger => {
                            HumanOwnedSourceOptions::AuthenticationLedger {
                                redactor: redactor.clone(),
                                policy: self.policy.clone(),
                                capacity_plan: self.capacity.clone(),
                            }
                        }
                    },
                )
            })
            .collect();
        let request = HumanOwnedStartupRequest {
            manifest,
            configuration,
            sources,
        };
        let credentials = self
            .files
            .iter()
            .find(|(kind, _)| *kind == HumanConfigurationFileKindV1::TechnicalCredentials)
            .unwrap()
            .1
            .clone();
        let tokens = ["A", "B"]
            .into_iter()
            .map(|tenant| {
                (
                    tenant.to_owned(),
                    format!("synthetic-{tenant}-runner-credential-long-enough"),
                    format!("synthetic-{tenant}-viewer-credential-long-enough"),
                )
            })
            .collect();
        (request, credentials, tokens)
    }
    pub(crate) fn principal(&self) -> &PrincipalRefV1 {
        &self.account
    }
    /// Prepare history through real legacy APIs before calling migrated().
    pub(crate) fn legacy_store(&self, tenant: &str) -> &Store {
        self.legacy
            .iter()
            .find(|store| store.tenant_id() == tenant)
            .unwrap()
    }

    fn actual(&self) -> HumanSourcesMaintenanceConfigurationV1 {
        HumanSourcesMaintenanceConfigurationV1::new(
            None,
            HumanMaintenanceResolvedViewV1::new(
                self.plan.target.sources.clone(),
                self.plan.target.configuration.clone(),
                self.files.clone(),
            )
            .unwrap(),
            None,
        )
    }
    fn request(&self) -> HumanSourcesMigrationRequest {
        let sources = self
            .plan
            .target
            .sources
            .iter()
            .map(|s| {
                (
                    s.source_id,
                    match s.kind {
                        HumanSourceKindV1::TenantStore => {
                            HumanMigrationSourceOptions::TenantStore {
                                redactor: Redactor::new(vec![]),
                                max_events: 10_000,
                                capacity: crate::audit::AuditCapacityPolicyV1::resolved(10_000),
                                workload: crate::audit::WorkloadAuditPolicyV1::resolved(
                                    10_000,
                                    &crate::audit::AuditCapacityPolicyV1::resolved(10_000),
                                ),
                                retention: crate::relay::RelayRetentionPolicyV1::default(),
                            }
                        }
                        HumanSourceKindV1::AuthenticationLedger => {
                            HumanMigrationSourceOptions::AuthenticationLedger {
                                redactor: Redactor::new(vec![]),
                                policy: self.policy.clone(),
                                capacity_plan: self.capacity.clone(),
                            }
                        }
                    },
                )
            })
            .collect();
        HumanSourcesMigrationRequest {
            plan_file: self.plan_file.clone(),
            maintenance_configuration: self.actual(),
            activation_files: self.files.clone(),
            sources,
            backup_paths: self.backup_paths.clone(),
            journal_directory: self.journal.clone(),
            closure_id: self.closure_id,
            closure_revision: 1,
        }
    }
    fn source(&self, id: u128) -> &HumanMaintenanceSourceV1 {
        self.plan
            .target
            .sources
            .iter()
            .find(|s| s.source_id == Uuid::from_u128(id))
            .unwrap()
    }
}

#[test]
fn real_migration_a_b_auth_is_owned_and_scoped_after_reopen() {
    let fixture = Fixture::new();
    let before_a = history(fixture.source(1));
    let before_b = history(fixture.source(2));
    let authority_a = identity_projection(fixture.source(1));
    let authority_b = identity_projection(fixture.source(2));
    let auth_before = fixture.legacy_auth.verify().unwrap();
    let bundle = migrate_human_sources(fixture.request()).unwrap();
    assert_eq!(bundle.owner().closure().sources.len(), 3);
    assert_eq!(bundle.stores.len(), 2);
    assert!(bundle.authentication().is_some());
    for source in &fixture.plan.target.sources {
        let descriptor = bundle
            .owner()
            .closure()
            .sources
            .iter()
            .find(|s| s.source_id == source.source_id)
            .unwrap();
        assert!(descriptor.storage_binding == source.storage_binding);
        assert_eq!(descriptor.durable_namespace_id, source.durable_namespace_id);
        assert_eq!(descriptor.tenant_id, source.tenant_id);
        assert_eq!(
            raw_format(source),
            if source.kind == HumanSourceKindV1::TenantStore {
                14
            } else {
                1_000_005
            }
        );
    }
    assert_eq!(
        &history(fixture.source(1))[..before_a.len()],
        before_a.as_slice()
    );
    assert_eq!(
        &history(fixture.source(2))[..before_b.len()],
        before_b.as_slice()
    );
    assert!(identity_projection(fixture.source(1)) == authority_a);
    assert!(identity_projection(fixture.source(2)) == authority_b);
    for source in [fixture.source(1), fixture.source(2)] {
        let bindings: i64 = read(&source.storage_binding.path())
            .query_row("SELECT COUNT(*) FROM runner_review_bindings", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(bindings, 0);
    }
    let token = bundle.owner().source_token(Uuid::from_u128(1)).unwrap();
    {
        let epoch = bundle.owner().try_write_epoch().unwrap();
        bundle
            .store(Uuid::from_u128(1))
            .unwrap()
            .mutate_identity_scoped(
                &epoch,
                &token,
                &identity_change(IdentityChangeV1::Account {
                    expected_revision: 1,
                    account: HumanAccountV1 {
                        version: 1,
                        principal: fixture.account.clone(),
                        display_name: "Revised synthetic person".into(),
                        revision: 2,
                        active: true,
                    },
                }),
            )
            .unwrap();
    }
    let auth_token = bundle.owner().source_token(Uuid::from_u128(3)).unwrap();
    let auth_session_id = Uuid::new_v4();
    let access_fingerprint =
        AuthenticationTokenFingerprintV1::access_token("synthetic-union-storage-access");
    let auth_initial_receipt = {
        let epoch = bundle.owner().try_write_epoch().unwrap();
        let access = AuthenticationSourceAccess::write(&epoch, &auth_token).unwrap();
        let audit = bundle.authentication().unwrap();
        let before = audit.verify_owned(&access).unwrap();
        let operation_id = Uuid::new_v4();
        let anonymous = AuthenticationRecordV1 {
            operation_id,
            action: AuthenticationActionV1::Login,
            phase: AuthenticationPhaseV1::Intention,
            issuer: fixture.policy.approved_clients[0].issuer.clone(),
            client_id: fixture.policy.approved_clients[0].client_id.clone(),
            principal: None,
            transaction_id: Some(operation_id),
            session_id: None,
            causal: None,
        };
        // A generic Login intention cannot choose its own capacity provenance.
        let refused = audit
            .append_typed_owned(&epoch, &auth_token, anonymous.clone())
            .unwrap_err();
        assert_eq!(
            refused.downcast_ref::<AuthenticationAuditFailureV1>(),
            Some(&AuthenticationAuditFailureV1::Format)
        );
        assert_eq!(audit.verify_owned(&access).unwrap(), before);
        // Storage fixture only: this synthetic principal/fingerprint supplies
        // the typed initial shape, without claiming real token verification.
        let initial = audit
            .begin_verified_initial_owned(
                &epoch,
                &auth_token,
                AuthenticationRecordV1 {
                    session_id: Some(auth_session_id),
                    ..anonymous
                },
                PrincipalRefV1::HumanOidc {
                    issuer: fixture.policy.approved_clients[0].issuer.clone(),
                    subject: "synthetic-union-storage-subject".into(),
                },
                access_fingerprint,
            )
            .unwrap();
        assert!(initial.inserted);
        let after = audit.verify_owned(&access).unwrap();
        assert_eq!(after.records, before.records + 1);
        assert_eq!(after.head, initial.receipt);
        assert!(
            audit
                .fingerprint_seen_owned(&access, &access_fingerprint)
                .unwrap()
        );
        initial.receipt
    };
    {
        let epoch = bundle.owner().try_read_epoch().unwrap();
        let access = AuthenticationSourceAccess::read(&epoch, &auth_token).unwrap();
        let verified = bundle
            .authentication()
            .unwrap()
            .verify_owned(&access)
            .unwrap();
        assert_eq!(verified.namespace_id, auth_before.namespace_id);
        assert!(verified.records > auth_before.records);
        assert_eq!(verified.head, auth_initial_receipt);
        assert!(
            bundle
                .authentication()
                .unwrap()
                .fingerprint_seen_owned(&access, &access_fingerprint)
                .unwrap()
        );
        let tenants = BTreeSet::from(["A".into(), "B".into()]);
        bundle
            .authentication()
            .unwrap()
            .validate_capacity_tenants_owned(&access, &tenants)
            .unwrap();
        assert!(
            bundle
                .authentication()
                .unwrap()
                .session_current_owned(&access, auth_session_id)
                .unwrap()
                .is_none()
        );
    }
    assert!(fixture.legacy[0].mutate_identity(&fixture.retry).is_err());
    assert!(
        fixture.legacy[0]
            .identity_memberships(&fixture.account)
            .is_err()
    );
    assert!(fixture.legacy_auth.verify().is_err());
    assert!(bundle.manifest().persistent_revalidate().is_ok());
    drop(auth_token);
    drop(token);
    drop(bundle);
    let reopened = migrate_human_sources(fixture.request()).unwrap();
    assert_eq!(reopened.owner().closure().sources.len(), 3);
    for source in &fixture.plan.target.sources {
        let db = read(&source.storage_binding.path());
        if source.kind == HumanSourceKindV1::TenantStore {
            let anchors: i64 = db.query_row("SELECT COUNT(*) FROM events WHERE kind='control.review_authority.source_anchor'", [], |r| r.get(0)).unwrap();
            let revision: i64 = db
                .query_row(
                    "SELECT incarnation_revision FROM review_authority_meta WHERE singleton=1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(anchors, 1);
            assert_eq!(revision, 2);
        }
    }
}

#[test]
fn last_source_writer_busy_mutates_nothing_and_keeps_backups_absent() {
    let fixture = Fixture::new();
    let last = *writer_order(&fixture.plan).last().unwrap();
    assert_eq!(last.kind, HumanSourceKindV1::TenantStore);
    let mut competing = Connection::open_with_flags(
        last.storage_binding.path(),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    competing.busy_timeout(std::time::Duration::ZERO).unwrap();
    let held = competing
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    assert!(migrate_human_sources(fixture.request()).is_err());
    for source in &fixture.plan.target.sources {
        assert_eq!(
            raw_format(source),
            if source.kind == HumanSourceKindV1::TenantStore {
                13
            } else {
                1_000_004
            }
        );
        assert!(!fixture.backup_paths[&source.source_id].exists());
    }
    assert!(fixture.journal.persistent_entries(16).unwrap().is_empty());
    held.rollback().unwrap();
}

#[test]
fn legacy_session_prepare_refuses_owned_transition_before_journal_or_backup() {
    let fixture = Fixture::new_owned_with_human_rights();
    let last = *writer_order(&fixture.plan).last().unwrap();
    assert_eq!(last.kind, HumanSourceKindV1::TenantStore);
    let store = fixture
        .legacy
        .iter()
        .find(|store| Some(store.tenant_id()) == last.tenant_id.as_deref())
        .unwrap();
    let prior =
        fixture.backup_paths[&last.source_id].with_file_name("before-legacy-capability.sqlite");
    store.enable_legacy_session_prepare_v1(&prior).unwrap();
    let heads: Vec<_> = fixture
        .legacy
        .iter()
        .map(|store| store.integrity().unwrap().head_hash)
        .collect();

    let refusal = migrate_human_sources(fixture.request())
        .err()
        .expect("marked source must stay Legacy");
    for source in &fixture.plan.target.sources {
        assert_eq!(
            raw_format(source),
            if source.kind == HumanSourceKindV1::TenantStore {
                13
            } else {
                1_000_004
            }
        );
        assert!(!fixture.backup_paths[&source.source_id].exists());
    }
    assert!(fixture.journal.persistent_entries(16).unwrap().is_empty());
    assert!(prior.is_file());
    for (store, expected) in fixture.legacy.iter().zip(heads) {
        let integrity = store.integrity().unwrap();
        assert!(integrity.valid);
        assert_eq!(integrity.head_hash, expected);
    }
    assert_eq!(
        refusal.to_string(),
        "human_sources_migration_legacy_session_prepare_unsupported"
    );
}

#[test]
fn partial_first_commit_resumes_same_plan_without_new_anchor() {
    let fixture = Fixture::new();
    FAIL_AFTER_COMMIT.with(|point| point.set(Some(0)));
    assert_eq!(
        migrate_human_sources(fixture.request()).err(),
        Some(HumanSourcesMigrationFailure::ReconciliationRequired)
    );
    assert_eq!(raw_format(fixture.source(3)), 1_000_005);
    assert_eq!(raw_format(fixture.source(1)), 13);
    assert_eq!(raw_format(fixture.source(2)), 13);
    let backup_identities: Vec<_> = fixture
        .backup_paths
        .values()
        .map(|path| {
            binding(
                &Directory::open_existing(path.parent().unwrap()).unwrap(),
                "prior.sqlite",
            )
        })
        .collect();
    let intent = MigrationIntent::new(&fixture.request(), &fixture.plan).unwrap();
    let before = Journal::load(fixture.journal.clone(), intent).unwrap();
    assert_eq!(before.confirmed_count(), 0);
    drop(before);
    let first = {
        let guard =
            HumanSourcesMaintenanceGuard::acquire(fixture.plan_file.clone(), fixture.actual())
                .unwrap();
        let token = guard.target_source(Uuid::from_u128(3)).unwrap();
        let source = MaintenanceSource::open(
            fixture.source(3),
            fixture.request().sources.get(&Uuid::from_u128(3)).unwrap(),
            &token,
        )
        .unwrap();
        source
            .inspect_committed(&token, &fixture.backup_paths[&Uuid::from_u128(3)])
            .unwrap()
            .anchor
            .unwrap()
    };
    let bundle = migrate_human_sources(fixture.request()).unwrap();
    assert!(
        bundle
            .owner()
            .closure()
            .sources
            .iter()
            .find(|s| s.source_id == Uuid::from_u128(3))
            .unwrap()
            .authority_marker_receipt
            == first
    );
    let after: Vec<_> = fixture
        .backup_paths
        .values()
        .map(|path| {
            binding(
                &Directory::open_existing(path.parent().unwrap()).unwrap(),
                "prior.sqlite",
            )
        })
        .collect();
    assert!(backup_identities == after);
}

#[test]
fn only_b_migrated_never_returns_a_modern_source_bundle() {
    let fixture = Fixture::new();
    let guard =
        HumanSourcesMaintenanceGuard::acquire(fixture.plan_file.clone(), fixture.actual()).unwrap();
    let token = guard.target_source(Uuid::from_u128(2)).unwrap();
    let source: ReviewStoreMaintenance = Store::open_review_maintenance_for_tenant(
        &fixture.source(2).storage_binding.path(),
        "B",
        Redactor::new(vec![]),
        10_000,
        &token,
    )
    .unwrap();
    source
        .with_review_maintenance_writer(&token, |writer| {
            let backup =
                writer.prepare_backup(&token, &fixture.backup_paths[&Uuid::from_u128(2)])?;
            writer.install_format14(&token, &backup)?;
            writer.commit()
        })
        .unwrap();
    drop(source);
    drop(token);
    drop(guard);
    assert!(migrate_human_sources(fixture.request()).is_err());
    assert_eq!(
        (
            raw_format(fixture.source(1)),
            raw_format(fixture.source(2)),
            raw_format(fixture.source(3))
        ),
        (13, 14, 1_000_004)
    );
    assert!(fixture.backup_paths[&Uuid::from_u128(2)].exists());
}

#[test]
fn missing_extra_or_changed_migration_sources_refuse_without_backup() {
    let fixture = Fixture::new();
    let mut missing = fixture.request();
    missing.sources.remove(&Uuid::from_u128(2));
    assert_eq!(
        migrate_human_sources(missing).err(),
        Some(HumanSourcesMigrationFailure::InvalidRequest)
    );
    let mut extra = fixture.request();
    extra.sources.insert(
        Uuid::new_v4(),
        HumanMigrationSourceOptions::TenantStore {
            redactor: Redactor::new(vec![]),
            max_events: 10_000,
            capacity: crate::audit::AuditCapacityPolicyV1::resolved(10_000),
            workload: crate::audit::WorkloadAuditPolicyV1::resolved(
                10_000,
                &crate::audit::AuditCapacityPolicyV1::resolved(10_000),
            ),
            retention: crate::relay::RelayRetentionPolicyV1::default(),
        },
    );
    assert_eq!(
        migrate_human_sources(extra).err(),
        Some(HumanSourcesMigrationFailure::InvalidRequest)
    );
    for path in fixture.backup_paths.values() {
        assert!(!path.exists());
    }
}

#[test]
fn authentication_capacity_scope_must_match_complete_tenant_union() {
    let mut fixture = Fixture::new();
    // The target resolver omits B while the real Auth4 plan still allocates B.
    // This is an independently valid DTO/physical union, but not a complete
    // matching authentication scope; the coordinator must discover it in DB.
    fixture
        .plan
        .target
        .sources
        .retain(|s| s.source_id != Uuid::from_u128(2));
    fixture.backup_paths.remove(&Uuid::from_u128(2));
    let parent = Directory::open_existing(fixture.plan_file.path().parent().unwrap()).unwrap();
    fixture.plan_file = parent
        .persistent_create(
            "maintenance-without-b.json",
            &serde_json::to_vec(&fixture.plan).unwrap(),
        )
        .unwrap();
    assert_eq!(
        migrate_human_sources(fixture.request()).err(),
        Some(HumanSourcesMigrationFailure::ReconciliationRequired)
    );
    for source in &fixture.plan.target.sources {
        assert_eq!(
            raw_format(source),
            if source.kind == HumanSourceKindV1::TenantStore {
                13
            } else {
                1_000_004
            }
        );
    }
    for path in fixture.backup_paths.values() {
        assert!(!path.exists());
    }
}

#[test]
fn maximum_floor_keeps_submillisecond_precision_and_never_defaults_to_zero() {
    let lower = DateTime::parse_from_rfc3339("2026-11-12T10:00:00.123456789Z")
        .unwrap()
        .with_timezone(&Utc);
    let maximum = lower + chrono::Duration::nanoseconds(1);
    assert_eq!(
        check_maximum_floor(lower, maximum),
        Err(HumanSourcesMigrationFailure::ClockBehind)
    );
    assert_eq!(check_maximum_floor(maximum, maximum), Ok(()));
    let candidates = [lower - chrono::Duration::seconds(1), maximum, lower];
    assert_eq!(candidates.into_iter().max().unwrap(), maximum);
}

#[test]
fn changed_journal_plan_after_partial_commit_never_rewrites_backup_or_marker() {
    let fixture = Fixture::new();
    FAIL_AFTER_COMMIT.with(|point| point.set(Some(0)));
    assert!(migrate_human_sources(fixture.request()).is_err());
    let before = std::fs::read(&fixture.backup_paths[&Uuid::from_u128(3)]).unwrap();
    let mut request = fixture.request();
    request.closure_id = Uuid::new_v4();
    assert!(migrate_human_sources(request).is_err());
    assert_eq!(
        std::fs::read(&fixture.backup_paths[&Uuid::from_u128(3)]).unwrap(),
        before
    );
    assert_eq!(raw_format(fixture.source(1)), 13);
    assert!(migrate_human_sources(fixture.request()).is_ok());
}

#[test]
fn fresh_migration_refuses_preexisting_exact_legacy_backup_before_intent() {
    let fixture = Fixture::new();
    let before_auth = fixture.legacy_auth.verify().unwrap();
    let before_a = history(fixture.source(1));
    let before_b = history(fixture.source(2));
    let before_formats: Vec<_> = fixture.plan.target.sources.iter().map(raw_format).collect();
    let occupied = &fixture.backup_paths[&Uuid::from_u128(3)];
    // A real ordinary Auth4 backup predates this migration's absent journal.
    // Matching bytes/prefix do not prove that this migration created it.
    let backed_up = fixture.legacy_auth.backup_new(occupied).unwrap();
    assert!(backed_up == before_auth);
    let backup_parent = Directory::open_existing(occupied.parent().unwrap()).unwrap();
    let before_binding = binding(&backup_parent, "prior.sqlite");
    let before_bytes = std::fs::read(occupied).unwrap();
    assert!(fixture.journal.persistent_entries(16).unwrap().is_empty());
    assert_eq!(fixture.legacy_auth.verify().unwrap(), before_auth);

    let result = migrate_human_sources(fixture.request());

    // U1's first discriminating oracle: even a later startup error must not
    // hide adoption of this preexisting backup after writing a fresh intent.
    assert!(
        fixture.journal.persistent_entries(16).unwrap().is_empty(),
        "U1: occupied initial backup must refuse before any journal intent"
    );
    assert!(
        result.is_err(),
        "U1: occupied initial backup must refuse migration"
    );
    assert_eq!(
        fixture
            .plan
            .target
            .sources
            .iter()
            .map(raw_format)
            .collect::<Vec<_>>(),
        before_formats
    );
    assert_eq!(history(fixture.source(1)), before_a);
    assert_eq!(history(fixture.source(2)), before_b);
    assert_eq!(fixture.legacy_auth.verify().unwrap(), before_auth);
    assert!(binding(&backup_parent, "prior.sqlite") == before_binding);
    assert!(std::fs::read(occupied).unwrap() == before_bytes);
    for (source_id, path) in &fixture.backup_paths {
        if *source_id != Uuid::from_u128(3) {
            assert!(!path.exists());
        }
    }
}

#[test]
fn fresh_migration_refuses_preexisting_backup_sidecars_before_intent() {
    for suffix in ["-wal", "-shm", "-journal"] {
        let fixture = Fixture::new();
        let before_auth = fixture.legacy_auth.verify().unwrap();
        let before_a = history(fixture.source(1));
        let before_b = history(fixture.source(2));
        let before_formats: Vec<_> = fixture.plan.target.sources.iter().map(raw_format).collect();
        // A later tenant's namespace is occupied while every backup main is
        // absent. All destinations must be checked before the first intent.
        let backup = &fixture.backup_paths[&Uuid::from_u128(2)];
        let parent = Directory::open_existing(backup.parent().unwrap()).unwrap();
        let mut sidecar_name = backup.file_name().unwrap().to_os_string();
        sidecar_name.push(suffix);
        let sidecar = parent
            .persistent_create(
                sidecar_name.to_str().unwrap(),
                b"synthetic preexisting SQLite sidecar",
            )
            .unwrap();
        let before_binding = parent.private_entry(&sidecar_name).unwrap().unwrap();
        assert!(fixture.journal.persistent_entries(16).unwrap().is_empty());
        assert!(
            parent
                .private_entry(backup.file_name().unwrap())
                .unwrap()
                .is_none()
        );

        let result = migrate_human_sources(fixture.request());

        assert!(
            fixture.journal.persistent_entries(16).unwrap().is_empty(),
            "occupied initial backup sidecar {suffix} must refuse before any journal intent"
        );
        assert!(result.is_err(), "occupied backup sidecar {suffix}");
        assert_eq!(
            fixture
                .plan
                .target
                .sources
                .iter()
                .map(raw_format)
                .collect::<Vec<_>>(),
            before_formats
        );
        assert_eq!(history(fixture.source(1)), before_a);
        assert_eq!(history(fixture.source(2)), before_b);
        assert_eq!(fixture.legacy_auth.verify().unwrap(), before_auth);
        sidecar.persistent_revalidate().unwrap();
        assert!(parent.private_entry(&sidecar_name).unwrap() == Some(before_binding));
        assert_eq!(
            std::fs::read(parent.path().join(&sidecar_name)).unwrap(),
            b"synthetic preexisting SQLite sidecar"
        );
        for path in fixture.backup_paths.values() {
            let parent = Directory::open_existing(path.parent().unwrap()).unwrap();
            assert!(
                parent
                    .private_entry(path.file_name().unwrap())
                    .unwrap()
                    .is_none()
            );
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct FixtureFileSnapshot {
    identity: (u64, u64),
    mode: u32,
    bytes: Vec<u8>,
}

fn fixture_file_snapshot(path: &Path) -> Option<FixtureFileSnapshot> {
    let parent = Directory::open_existing(path.parent().unwrap()).unwrap();
    let name = path.file_name().unwrap();
    let identity = parent.private_entry(name).unwrap()?;
    let mode = std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777;
    let bytes = std::fs::read(path).unwrap();
    assert!(parent.private_entry(name).unwrap() == Some(identity));
    Some(FixtureFileSnapshot {
        identity: identity.physical_identity(),
        mode,
        bytes,
    })
}

fn fixture_input_snapshots(
    plan: &HumanSourcesMaintenancePlanV1,
    request: &HumanSourcesMigrationRequest,
) -> BTreeMap<PathBuf, Option<FixtureFileSnapshot>> {
    let mut paths = Vec::new();
    for source in &plan.target.sources {
        let path = source.storage_binding.path();
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut name = path.file_name().unwrap().to_os_string();
            name.push(suffix);
            paths.push(path.parent().unwrap().join(name));
        }
    }
    paths.push(request.plan_file.path().to_owned());
    paths.extend(
        request
            .activation_files
            .iter()
            .map(|(_, file)| file.path().to_owned()),
    );
    paths
        .into_iter()
        .map(|path| {
            let snapshot = fixture_file_snapshot(&path);
            (path, snapshot)
        })
        .collect()
}

fn assert_physical_substitution_refused(
    request: HumanSourcesMigrationRequest,
    expected: BTreeMap<PathBuf, Option<FixtureFileSnapshot>>,
) {
    let journal = request.journal_directory.clone();
    let backups = request.backup_paths.clone();
    assert!(journal.persistent_entries(16).unwrap().is_empty());
    assert_eq!(
        migrate_human_sources(request).err(),
        Some(HumanSourcesMigrationFailure::MaintenanceRefused)
    );
    assert!(journal.persistent_entries(16).unwrap().is_empty());
    for path in backups.values() {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut name = path.file_name().unwrap().to_os_string();
            name.push(suffix);
            assert!(fixture_file_snapshot(&path.parent().unwrap().join(name)).is_none());
        }
    }
    for (path, snapshot) in expected {
        assert!(
            fixture_file_snapshot(&path) == snapshot,
            "fixture input changed after refusal: {}",
            path.display()
        );
    }
}

#[test]
fn physical_authentication_source_substitution_refuses_before_any_backup_or_intent() {
    let fixture = Fixture::new();
    let source = fixture.source(3).clone();
    let original = source.storage_binding.path();
    let parent = Directory::open_existing(original.parent().unwrap()).unwrap();
    let root = Directory::open_existing(fixture._temporary.path()).unwrap();
    let material = private_child(&root, "source-substitution-material");
    let replacement = material.path().join("replacement.sqlite");
    // The replacement is a real independent Auth4 backup at the same verified
    // logical prefix, not a changed source ID or a hand-built modern database.
    let before = fixture.legacy_auth.verify().unwrap();
    assert_eq!(
        fixture.legacy_auth.backup_new(&replacement).unwrap(),
        before
    );
    let request = fixture.request();
    // Close all SQLite handles before raw byte snapshots or pathname changes.
    drop(fixture.legacy);
    drop(fixture.legacy_auth);
    let mut expected = fixture_input_snapshots(&fixture.plan, &request);
    let original_snapshot = expected.get(&original).unwrap().clone().unwrap();
    let replacement_snapshot = fixture_file_snapshot(&replacement).unwrap();
    assert_eq!(
        original_snapshot.identity,
        (
            source.storage_binding.source_device,
            source.storage_binding.source_inode
        )
    );
    assert_ne!(original_snapshot.identity, replacement_snapshot.identity);
    let retained = parent.path().join("retained-authentication.sqlite");
    assert!(fixture_file_snapshot(&retained).is_none());
    std::fs::rename(&original, &retained).unwrap();
    std::fs::rename(&replacement, &original).unwrap();
    assert!(fixture_file_snapshot(&retained) == Some(original_snapshot.clone()));
    assert!(fixture_file_snapshot(&original) == Some(replacement_snapshot.clone()));
    assert!(fixture_file_snapshot(&replacement).is_none());
    expected.insert(retained, Some(original_snapshot));
    expected.insert(original, Some(replacement_snapshot));
    assert_physical_substitution_refused(request, expected);
}

#[test]
fn physical_configuration_substitution_refuses_before_any_backup_or_intent() {
    let fixture = Fixture::new();
    let request = fixture.request();
    let original = request.activation_files[0].1.path().to_owned();
    let parent = Directory::open_existing(original.parent().unwrap()).unwrap();
    // Keeping the exact bytes isolates physical substitution from semantic
    // configuration changes. The request retains the original checked file.
    let replacement = parent
        .persistent_create(
            "replacement-config.json",
            request.activation_files[0].1.bytes(),
        )
        .unwrap();
    drop(fixture.legacy);
    drop(fixture.legacy_auth);
    let mut expected = fixture_input_snapshots(&fixture.plan, &request);
    let original_snapshot = expected.get(&original).unwrap().clone().unwrap();
    let replacement_snapshot = fixture_file_snapshot(replacement.path()).unwrap();
    assert!(original_snapshot.bytes == replacement_snapshot.bytes);
    assert_ne!(original_snapshot.identity, replacement_snapshot.identity);
    let retained = parent.path().join("retained-config.json");
    assert!(fixture_file_snapshot(&retained).is_none());
    std::fs::rename(&original, &retained).unwrap();
    std::fs::rename(replacement.path(), &original).unwrap();
    assert!(fixture_file_snapshot(&retained) == Some(original_snapshot.clone()));
    assert!(fixture_file_snapshot(&original) == Some(replacement_snapshot.clone()));
    assert!(
        request.activation_files[0]
            .1
            .persistent_revalidate()
            .is_err()
    );
    request.plan_file.persistent_revalidate().unwrap();
    expected.insert(retained, Some(original_snapshot));
    expected.insert(original, Some(replacement_snapshot));
    assert_physical_substitution_refused(request, expected);
}

fn independently_read_durable_floor(source: &HumanMaintenanceSourceV1) -> DateTime<Utc> {
    let db = read(&source.storage_binding.path());
    let (history_sql, projection_sql) = match source.kind {
        HumanSourceKindV1::TenantStore => (
            "SELECT received_at FROM events ORDER BY seq",
            "SELECT max_received_at FROM audit_time_floors ORDER BY event_seq DESC LIMIT 1",
        ),
        HumanSourceKindV1::AuthenticationLedger => (
            "SELECT received_at FROM authentication_entries ORDER BY seq",
            "SELECT wall_floor FROM authentication_source_authority WHERE singleton=1",
        ),
    };
    let mut query = db.prepare(history_sql).unwrap();
    let history: Vec<String> = query
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let maximum = history
        .iter()
        .map(|value| {
            DateTime::parse_from_rfc3339(value)
                .unwrap()
                .with_timezone(&Utc)
        })
        .max()
        .unwrap();
    let projected: String = db.query_row(projection_sql, [], |row| row.get(0)).unwrap();
    assert_eq!(
        DateTime::parse_from_rfc3339(&projected)
            .unwrap()
            .with_timezone(&Utc),
        maximum
    );
    maximum
}

struct StartupFloorsObserverReset;
impl Drop for StartupFloorsObserverReset {
    fn drop(&mut self) {
        STARTUP_FLOORS_OBSERVER.with(|observer| {
            observer.borrow_mut().take();
        });
    }
}

#[test]
fn startup_collects_three_real_durable_floors_and_preserves_submillisecond_maximum() {
    let fixture = Fixture::new();
    let sources = fixture.plan.target.sources.clone();
    let observations = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let captured = std::rc::Rc::clone(&observations);
    let observed_sources = sources.clone();
    let _reset = StartupFloorsObserverReset;
    STARTUP_FLOORS_OBSERVER.with(|observer| {
        assert!(observer.borrow().is_none());
        *observer.borrow_mut() = Some(Box::new(move |floors, maximum| {
            assert_eq!(floors.len(), 3);
            assert_eq!(
                floors.iter().map(|(id, _)| *id).collect::<BTreeSet<_>>(),
                BTreeSet::from([Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3)])
            );
            let durable: BTreeMap<_, _> = observed_sources
                .iter()
                .map(|source| (source.source_id, independently_read_durable_floor(source)))
                .collect();
            assert_eq!(floors.iter().copied().collect::<BTreeMap<_, _>>(), durable);
            assert_eq!(maximum, *durable.values().max().unwrap());
            captured.borrow_mut().push((durable, maximum));
        }));
    });

    // First startup and all positive modern sources use the real coordinator's
    // backups, conversions and commits. The observer cannot supply a floor.
    let first = migrate_human_sources(fixture.request()).unwrap();
    assert_eq!(observations.borrow().len(), 1);
    let store_maximum = sources
        .iter()
        .filter(|source| source.kind == HumanSourceKindV1::TenantStore)
        .map(independently_read_durable_floor)
        .max()
        .unwrap();
    let auth_source = fixture.source(3);
    let auth_token = first.owner().source_token(auth_source.source_id).unwrap();
    let mut fractional_maximum = None;
    // At most four genuine pending initials, within the fixture's unscoped
    // capacity. A coarse clock is an explicit unmet fixture precondition; no
    // synthetic DateTime, SQL UPDATE or clock hook creates the precision case.
    for _ in 0..4 {
        let operation_id = Uuid::new_v4();
        let epoch = first.owner().try_write_epoch().unwrap();
        let inserted = first
            .authentication()
            .unwrap()
            .begin_verified_initial_owned(
                &epoch,
                &auth_token,
                AuthenticationRecordV1 {
                    operation_id,
                    action: AuthenticationActionV1::Login,
                    phase: AuthenticationPhaseV1::Intention,
                    issuer: fixture.policy.approved_clients[0].issuer.clone(),
                    client_id: fixture.policy.approved_clients[0].client_id.clone(),
                    principal: None,
                    transaction_id: Some(operation_id),
                    session_id: Some(Uuid::new_v4()),
                    causal: None,
                },
                PrincipalRefV1::HumanOidc {
                    issuer: fixture.policy.approved_clients[0].issuer.clone(),
                    subject: "synthetic-durable-floor-subject".into(),
                },
                AuthenticationTokenFingerprintV1::access_token(&format!(
                    "synthetic-durable-floor-{operation_id}"
                )),
            )
            .unwrap();
        assert!(inserted.inserted);
        drop(epoch);
        let floor = independently_read_durable_floor(auth_source);
        if floor > store_maximum && floor.timestamp_subsec_nanos() % 1_000_000 != 0 {
            fractional_maximum = Some(floor);
            break;
        }
    }
    let fractional_maximum = fractional_maximum
        .expect("fixture did not obtain a durable submillisecond Auth maximum in four real writes");
    let before_resume: BTreeMap<_, _> = sources
        .iter()
        .map(|source| (source.source_id, independently_read_durable_floor(source)))
        .collect();
    assert_eq!(
        before_resume.values().copied().max().unwrap(),
        fractional_maximum
    );
    drop(auth_token);
    drop(first);

    let resumed = migrate_human_sources(fixture.request()).unwrap();
    assert_eq!(resumed.owner().closure().sources.len(), 3);
    let observed = observations.borrow();
    assert_eq!(observed.len(), 2);
    assert_eq!(observed[1].0, before_resume);
    assert_eq!(observed[1].1, fractional_maximum);
    assert_ne!(observed[1].1.timestamp_subsec_nanos() % 1_000_000, 0);
}
