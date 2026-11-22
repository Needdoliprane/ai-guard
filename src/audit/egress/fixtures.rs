//! Source-only candidate regressions. Root admits the frozen files to offline QA.
use super::*;
use crate::audit::review_authority as authority;
use crate::control_fs::{Directory, PrivateFile};
use crate::human_authority::{
    HumanAuthorityClosureV1, HumanAuthorityConfigurationV1, HumanAuthorityMarkerReceiptV1,
    HumanConfigurationFileKindV1, HumanConfigurationFileV1, HumanSourceDescriptorV1,
    HumanSourceKindV1, HumanSourcesConfigurationV1, HumanStorageProfileV1, OwnedHumanSources,
    SourceStorageBindingV1,
    maintenance::{
        HumanMaintenanceActiveManifestV1, HumanMaintenanceResolvedViewV1,
        HumanMaintenanceSourceToken, HumanMaintenanceSourceV1, HumanMaintenanceViewV1,
        HumanSourcesMaintenanceConfigurationV1, HumanSourcesMaintenanceGuard,
        HumanSourcesMaintenancePlanV1,
    },
};
use std::{ffi::OsStr, os::unix::fs::PermissionsExt};
use uuid::Uuid;

struct BaseFixture {
    _temporary: tempfile::TempDir,
    root: Directory,
    stores: Vec<Store>,
    plan: HumanSourcesMaintenancePlanV1,
    plan_file: PrivateFile,
    configuration_files: Vec<(HumanConfigurationFileKindV1, PrivateFile)>,
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

impl BaseFixture {
    fn new(tenants: &[&str]) -> Self {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        let root = Directory::open_existing(temporary.path()).unwrap();
        let installation_id = Uuid::new_v4();
        let mut stores = Vec::new();
        let mut sources = Vec::new();
        for (index, tenant) in tenants.iter().enumerate() {
            let parent = root
                .persistent_child(&format!("source-{index}"), true)
                .unwrap()
                .unwrap();
            let store = Store::open_for_tenant(
                &parent.path().join("tenant.sqlite"),
                tenant,
                Redactor::new(vec![]),
                10_000,
            )
            .unwrap();
            for target in 3..=13 {
                store
                    .migrate_control_version(
                        &root.path().join(format!("history-{index}-{target}.sqlite")),
                        target - 1,
                        target,
                    )
                    .unwrap();
            }
            sources.push(HumanMaintenanceSourceV1 {
                source_id: Uuid::from_u128(index as u128 + 1),
                kind: HumanSourceKindV1::TenantStore,
                installation_id,
                durable_namespace_id: authority::namespace(&store.db.lock().unwrap()).unwrap(),
                tenant_id: Some((*tenant).into()),
                storage_binding: binding(&parent, "tenant.sqlite"),
                required_format: 14,
            });
            stores.push(store);
        }
        let config = root
            .persistent_child("configuration", true)
            .unwrap()
            .unwrap();
        let control = config
            .persistent_create("control.json", b"synthetic configuration")
            .unwrap();
        let envelope = json!({"version":1,"installation_id":installation_id,"closure_id":Uuid::from_u128(9000),"closure_revision":1,"control_config_public_digest":"ac".repeat(32),"entries":tenants.iter().enumerate().flat_map(|(i,tenant)|[json!({"tenant_id":tenant,"role":"runner","credential_id":Uuid::from_u128(100+i as u128*2),"generation":1,"token":format!("synthetic-{tenant}-runner")}),json!({"tenant_id":tenant,"role":"viewer","credential_id":Uuid::from_u128(101+i as u128*2),"generation":1,"token":format!("synthetic-{tenant}-viewer")})]).collect::<Vec<_>>()});
        let credentials = config
            .persistent_create("credentials.json", &serde_json::to_vec(&envelope).unwrap())
            .unwrap();
        let configuration = HumanAuthorityConfigurationV1 {
            control_config_revision: 1,
            public_config_digest: "ac".repeat(32),
            oidc: None,
            files: vec![
                HumanConfigurationFileV1 {
                    kind: HumanConfigurationFileKindV1::ControlConfiguration,
                    storage_binding: binding(&config, "control.json"),
                },
                HumanConfigurationFileV1 {
                    kind: HumanConfigurationFileKindV1::TechnicalCredentials,
                    storage_binding: binding(&config, "credentials.json"),
                },
            ],
        };
        let plan = HumanSourcesMaintenancePlanV1 {
            version: 1,
            migration_id: Uuid::new_v4(),
            installation_id,
            prior: None,
            target: HumanMaintenanceViewV1 {
                sources,
                configuration,
            },
            prior_active_manifest: None,
            supported_storage_profile: HumanStorageProfileV1::LocalPosixFlockV1,
            selected_source_id: None,
        };
        let plan_file = config
            .persistent_create("maintenance.json", &serde_json::to_vec(&plan).unwrap())
            .unwrap();
        Self {
            _temporary: temporary,
            root,
            stores,
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

    fn maintenance(&self) -> HumanSourcesMaintenanceGuard {
        let target = HumanMaintenanceResolvedViewV1::new(
            self.plan.target.sources.clone(),
            self.plan.target.configuration.clone(),
            self.configuration_files.clone(),
        )
        .unwrap();
        HumanSourcesMaintenanceGuard::acquire(
            self.plan_file.clone(),
            HumanSourcesMaintenanceConfigurationV1::new(None, target, None),
        )
        .unwrap()
    }

    fn backup_path(&self, index: usize) -> std::path::PathBuf {
        self.root
            .persistent_child("backups", true)
            .unwrap()
            .unwrap()
            .path()
            .join(format!("source-{index}.sqlite"))
    }

    fn open_maintenance(
        &self,
        index: usize,
        token: &HumanMaintenanceSourceToken,
    ) -> ReviewStoreMaintenance {
        let source = &self.plan.target.sources[index];
        Store::open_review_maintenance_for_tenant(
            &source.storage_binding.path(),
            source.tenant_id.as_deref().unwrap(),
            Redactor::new(vec![]),
            10_000,
            token,
        )
        .unwrap()
    }

    fn migrate_all(&self) -> Vec<authority::AuthorityReceiptV1> {
        let guard = self.maintenance();
        self.plan
            .target
            .sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                let token = guard.target_source(source.source_id).unwrap();
                let opened = self.open_maintenance(index, &token);
                opened
                    .with_review_maintenance_writer(&token, |writer| {
                        let backup = writer.prepare_backup(&token, &self.backup_path(index))?;
                        let receipt = writer.install_format14(&token, &backup)?;
                        writer.commit()?;
                        Ok(receipt)
                    })
                    .unwrap()
            })
            .collect()
    }

    fn owner(
        &self,
        receipts: &[authority::AuthorityReceiptV1],
        file_name: &str,
        format: u32,
    ) -> OwnedHumanSources {
        let sources: Vec<_> = self
            .plan
            .target
            .sources
            .iter()
            .zip(receipts)
            .map(|(source, receipt)| HumanSourceDescriptorV1 {
                source_id: source.source_id,
                kind: source.kind,
                installation_id: source.installation_id,
                durable_namespace_id: source.durable_namespace_id,
                tenant_id: source.tenant_id.clone(),
                storage_binding: source.storage_binding.clone(),
                required_format: format,
                authority_marker_receipt: HumanAuthorityMarkerReceiptV1 {
                    seq: receipt.seq,
                    hash: receipt.hash.clone(),
                },
            })
            .collect();
        let mut closure = HumanAuthorityClosureV1 {
            version: 1,
            installation_id: self.plan.installation_id,
            closure_id: Uuid::from_u128(9000),
            closure_revision: 1,
            closure_digest: String::new(),
            sources: sources.clone(),
            configuration: self.plan.target.configuration.clone(),
            supported_storage_profile: HumanStorageProfileV1::LocalPosixFlockV1,
        };
        closure.closure_digest = closure.compute_digest().unwrap();
        let config = Directory::open_existing(self.plan_file.path().parent().unwrap()).unwrap();
        let manifest = config
            .persistent_create(file_name, &serde_json::to_vec(&closure).unwrap())
            .unwrap();
        OwnedHumanSources::acquire(
            manifest,
            HumanSourcesConfigurationV1::new(
                sources,
                closure.configuration,
                self.configuration_files.clone(),
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn startup(&self, index: usize, source: &OwnedSourceToken) -> ReviewStoreStartup {
        let descriptor = &self.plan.target.sources[index];
        Store::open_review_startup_for_tenant(
            &descriptor.storage_binding.path(),
            descriptor.tenant_id.as_deref().unwrap(),
            Redactor::new(vec![]),
            160,
            source,
        )
        .unwrap()
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct Rows {
    pub(super) heads: usize,
    pub(super) revisions: usize,
    pub(super) operations: usize,
    pub(super) reservations: usize,
    chain: (i64, String),
    quota: i64,
    all: Vec<Vec<rusqlite::types::Value>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FillClass {
    Ordinary,
    Control,
    Identity,
}
#[derive(Debug)]
pub(super) struct SaturationObservation {
    pub(super) accepted: u64,
    pub(super) before_events: u64,
    pub(super) before_bytes: u64,
    pub(super) at_events: u64,
    pub(super) at_bytes: u64,
    pub(super) event_limit: u64,
}
enum FillRequest {
    Identity(crate::identity::IdentityMutationV1),
    Event(Event),
}

pub(super) fn require_saturation_cause(
    class: FillClass,
    error: anyhow::Error,
) -> anyhow::Result<()> {
    let expected = match class {
        FillClass::Identity => {
            error.downcast_ref::<EgressStoreFailure>()
                == Some(&EgressStoreFailure::AuditReservationExhausted)
        }
        FillClass::Ordinary | FillClass::Control => {
            error.downcast_ref::<AuditCapacityFailureV1>()
                == Some(&AuditCapacityFailureV1::OrdinaryWriteBudgetExhausted)
        }
    };
    if expected {
        Ok(())
    } else {
        Err(error.context("audit filler stopped without its expected typed capacity refusal"))
    }
}
pub(super) struct Fixture {
    base: BaseFixture,
    pub(super) owner: OwnedHumanSources,
    pub(super) sources: Vec<OwnedSourceToken>,
    pub(super) stores: Vec<Arc<Store>>,
    pub(super) credential: AuthenticatedReviewRunnerCredential,
    account: crate::identity::HumanAccountV1,
    policy_start: u64,
    policy_end: u64,
}

impl Fixture {
    pub(super) fn new(limits: EgressStoreLimitsV1) -> Self {
        let base = BaseFixture::new(&["A", "B"]);
        let anchors = base.migrate_all();
        migrate15(&base, &limits);
        let owner = base.owner(&anchors, "active15.json", 15);
        let sources = base
            .plan
            .target
            .sources
            .iter()
            .map(|s| owner.source_token(s.source_id).unwrap())
            .collect::<Vec<_>>();
        let stores = sources
            .iter()
            .enumerate()
            .map(|(i, source)| {
                let startup = base.startup(i, source);
                let epoch = owner.try_write_epoch().unwrap();
                startup
                    .append_incarnation(&epoch, source, Uuid::new_v4(), 0)
                    .unwrap();
                let store = startup.retain(&epoch, source).unwrap();
                Arc::new(store)
            })
            .collect::<Vec<_>>();
        let mut command = crate::control::review_binding::tests::provision();
        command.binding.tenant_id = "A".into();
        command.policy.tenant_id = "A".into();
        command.binding.credential_id = Uuid::from_u128(100).to_string();
        command.binding.control_namespace_uuid =
            sources[0].descriptor().durable_namespace_id.to_string();
        command.policy.control_namespace_uuid = command.binding.control_namespace_uuid.clone();
        command.binding.closure_id = owner.closure().closure_id.to_string();
        command.binding.closure_revision = owner.closure().closure_revision;
        command.binding.closure_digest = owner.closure().closure_digest.clone();
        command.binding.control_config_public_digest =
            owner.closure().configuration.public_config_digest.clone();
        command.policy.shares.clear();
        command.policy.owner_permissions =
            vec![crate::control::review_binding::ReviewSessionPermissionV1::Content];
        command.binding.session_binding_policy_digest = command.policy.digest().unwrap();
        let account = crate::identity::HumanAccountV1 {
            version: 1,
            principal: command.policy.owner.clone(),
            display_name: "Synthetic owner".into(),
            revision: 1,
            active: true,
        };
        let member = crate::identity::MembershipV1 {
            version: 1,
            membership_id: Uuid::new_v4().to_string(),
            principal: account.principal.clone(),
            tenant_id: "A".into(),
            organization_scope: command.policy.organization_scope.clone(),
            revision: 1,
            active: true,
            capabilities: [crate::identity::HumanCapabilityV1::ReadContent]
                .into_iter()
                .collect(),
        };
        for change in [
            crate::identity::IdentityChangeV1::Account {
                expected_revision: 0,
                account: account.clone(),
            },
            crate::identity::IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: member,
            },
        ] {
            mutate(&owner, &sources[0], &stores[0], change);
        }
        {
            let epoch = owner.try_write_epoch().unwrap();
            stores[0]
                .provision_review_binding(&epoch, &sources[0], &command)
                .unwrap();
        }
        let file = base
            .configuration_files
            .iter()
            .find(|(kind, _)| *kind == HumanConfigurationFileKindV1::TechnicalCredentials)
            .unwrap()
            .1
            .clone();
        let credential = crate::api::authenticate_review_runner_fixture(
            file,
            owner.clone(),
            stores
                .iter()
                .enumerate()
                .map(|(i, s)| (if i == 0 { "A".into() } else { "B".into() }, Arc::clone(s)))
                .collect(),
            "synthetic-A-runner",
        )
        .unwrap();
        let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
        Self {
            base,
            owner,
            sources,
            stores,
            credential,
            account,
            policy_start: now.saturating_sub(60_000),
            policy_end: now + 600_000,
        }
    }
    pub(super) fn replace(
        &self,
        selector: EgressPolicySelectorV1,
        revision: u64,
        epoch: u64,
        destinations: &[&str],
    ) -> EgressReplacePolicyV1 {
        self.replace_with_id(Uuid::new_v4(), selector, revision, epoch, destinations)
    }
    pub(super) fn replace_with_id(
        &self,
        id: Uuid,
        selector: EgressPolicySelectorV1,
        revision: u64,
        epoch: u64,
        destinations: &[&str],
    ) -> EgressReplacePolicyV1 {
        let authorities = destinations
            .iter()
            .map(|d| Authority::parse_connect(d).unwrap())
            .collect::<Vec<_>>();
        let ceiling = if matches!(selector, EgressPolicySelectorV1::Session(_)) {
            let db = self.stores[0].db.lock().unwrap();
            let projection = replay(&db).unwrap();
            let head = projection
                .heads
                .get(&EgressPolicySelectorV1::Tenant)
                .unwrap();
            Some(
                EgressTenantCeilingV1::new(
                    head.policy.revision(),
                    head.epoch,
                    head.policy.core().unwrap().digest(),
                )
                .unwrap(),
            )
        } else {
            None
        };
        EgressReplacePolicyV1::new(
            id,
            selector,
            revision,
            epoch,
            EgressPolicyV1::new(
                revision + 1,
                self.policy_start,
                self.policy_end,
                &authorities,
                PolicyLimits::default(),
            )
            .unwrap(),
            ceiling,
        )
        .unwrap()
    }
    pub(super) fn large_replace(&self) -> EgressReplacePolicyV1 {
        let destinations = (0..128)
            .map(|i| {
                format!(
                    "{}{}.{}.{}.test:443",
                    "a".repeat(58),
                    i,
                    "b".repeat(63),
                    "c".repeat(63)
                )
            })
            .collect::<Vec<_>>();
        self.replace(
            EgressPolicySelectorV1::Tenant,
            1,
            1,
            &destinations.iter().map(String::as_str).collect::<Vec<_>>(),
        )
    }
    pub(super) fn install(
        &self,
        request: &EgressReplacePolicyV1,
    ) -> anyhow::Result<EgressPolicyReceiptV1> {
        self.install_on(&self.stores[0], request)
    }
    pub(super) fn install_on(
        &self,
        store: &Store,
        request: &EgressReplacePolicyV1,
    ) -> anyhow::Result<EgressPolicyReceiptV1> {
        let epoch = self.owner.try_write_epoch()?;
        let source = &self.sources[0];
        let access = HumanSourceAccess::write(&epoch, source)?;
        let generation = self.stores[0].modern_discovery_generation(&access)?;
        let expected = crate::control::egress_policy::EgressPolicyExpectedScopeV1::new(
            self.stores[0].tenant_id(),
            &source.descriptor().durable_namespace_id.to_string(),
        )?;
        store.egress_replace_policy_scoped(
            &epoch,
            source,
            &self.credential,
            &expected,
            generation,
            request,
        )
    }
    pub(super) fn install_with_other_credential(
        &self,
        request: &EgressReplacePolicyV1,
    ) -> anyhow::Result<EgressPolicyReceiptV1> {
        let file = self
            .base
            .configuration_files
            .iter()
            .find(|(kind, _)| *kind == HumanConfigurationFileKindV1::TechnicalCredentials)
            .unwrap()
            .1
            .clone();
        let credential = crate::api::authenticate_review_runner_fixture(
            file,
            self.owner.clone(),
            self.stores
                .iter()
                .enumerate()
                .map(|(i, s)| (if i == 0 { "A".into() } else { "B".into() }, Arc::clone(s)))
                .collect(),
            "synthetic-B-runner",
        )?;
        let epoch = self.owner.try_write_epoch()?;
        let access = HumanSourceAccess::write(&epoch, &self.sources[0])?;
        let generation = self.stores[0].modern_discovery_generation(&access)?;
        let expected = crate::control::egress_policy::EgressPolicyExpectedScopeV1::new(
            self.stores[0].tenant_id(),
            &self.sources[0]
                .descriptor()
                .durable_namespace_id
                .to_string(),
        )?;
        self.stores[0].egress_replace_policy_scoped(
            &epoch,
            &self.sources[0],
            &credential,
            &expected,
            generation,
            request,
        )
    }
    pub(super) fn revoke_control_binding(&self) {
        use crate::control::review_binding::{
            ReviewBindingRevocationReasonV1, ReviewBindingSnapshotV1, RevokeRunnerReviewBindingV1,
        };
        let snapshot: ReviewBindingSnapshotV1 = identity::json_row(
            &self.stores[0].db.lock().unwrap(),
            "SELECT snapshot_json FROM runner_review_bindings WHERE binding_id=?1",
            "11000000-0000-4000-8000-000000000001",
        )
        .unwrap()
        .unwrap();
        let command = RevokeRunnerReviewBindingV1 {
            version: 1,
            operation_id: Uuid::new_v4().to_string(),
            operator_delegation_id: Uuid::new_v4().to_string(),
            binding_id: snapshot.binding.binding_id,
            expected_revision: snapshot.binding.revision,
            expected_binding_digest: snapshot.binding_digest,
            reason: ReviewBindingRevocationReasonV1::OperatorRevoked,
        };
        let epoch = self.owner.try_write_epoch().unwrap();
        self.stores[0]
            .revoke_review_binding(&epoch, &self.sources[0], &command)
            .unwrap();
    }
    pub(super) fn advance_clock_floor(&self) {
        let epoch = self.owner.try_write_epoch().unwrap();
        let access = HumanSourceAccess::write(&epoch, &self.sources[0]).unwrap();
        let store = &self.stores[0];
        let mut db = store.db.lock().unwrap();
        let tx = store
            .begin_checked_scoped(&mut db, &access, StoreOperationClass::Business)
            .unwrap();
        let event = Event::new("fixture.future_clock");
        let (event, payload) = store.prepare_event(event).unwrap();
        store
            .append_with_capacity_class_at(
                &tx,
                event,
                payload,
                capacity::Class::ControlWrite,
                chrono::Utc::now() + chrono::Duration::seconds(60),
            )
            .unwrap();
        store.commit_checked(tx).unwrap();
    }
    pub(super) fn assert_expired_head_terminal_transition(&self) {
        let db = self.stores[0].db.lock().unwrap();
        let projection = replay(&db).unwrap();
        let stored:StoredEvent=db.query_row("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind='control.egress_policy.replaced' ORDER BY seq DESC LIMIT 1",[],row_event).unwrap();
        let original: MutationRecord = serde_json::from_value(stored.event.data).unwrap();
        let change = Change::Revoke {
            operation_id: Uuid::new_v4(),
            selector: EgressPolicySelectorV1::Tenant,
            expected_revision: 1,
            expected_epoch: 1,
        };
        let record = MutationRecord {
            version: 1,
            tenant_id: "A".into(),
            input_hash: MutationRecord::input_hash("A", &original.actor, &change).unwrap(),
            actor: original.actor,
            change,
            tenant_ceiling: None,
            after_epoch: 2,
        };
        assert_eq!(
            projection
                .preflight(&record, self.policy_end + 1)
                .unwrap()
                .state,
            EgressPolicyStateV1::RevokeRequested
        );
    }
    pub(super) fn revoke(
        &self,
        request: &EgressRevokePolicyV1,
    ) -> anyhow::Result<EgressPolicyReceiptV1> {
        let epoch = self.owner.try_write_epoch()?;
        let access = HumanSourceAccess::write(&epoch, &self.sources[0])?;
        let generation = self.stores[0].modern_discovery_generation(&access)?;
        let expected = crate::control::egress_policy::EgressPolicyExpectedScopeV1::new(
            self.stores[0].tenant_id(),
            &self.sources[0]
                .descriptor()
                .durable_namespace_id
                .to_string(),
        )?;
        self.stores[0].egress_revoke_policy_scoped(
            &epoch,
            &self.sources[0],
            &self.credential,
            &expected,
            generation,
            request,
        )
    }
    pub(super) fn rows(&self, index: usize) -> Rows {
        self.rows_for(&self.stores[index])
    }
    /// Full diagnostic under the same genuine modern source capability as
    /// normal scoped Store reads. The legacy integrity entrance stays closed.
    /// No authority is reconstructed from the policy tables or their receipt.
    pub(super) fn integrity_with_source(
        &self,
        store: &Store,
        index: usize,
    ) -> anyhow::Result<Integrity> {
        let epoch = self.owner.try_read_epoch()?;
        let source = self
            .sources
            .get(index)
            .context("fixture source index unavailable")?;
        let access = HumanSourceAccess::read(&epoch, source)?;
        let mut db = store
            .db
            .try_lock()
            .map_err(|_| review_access::ReviewAccessFailure::Busy)?;
        let tx = store.begin_checked_scoped(&mut db, &access, StoreOperationClass::Business)?;
        tx.require_business_access()?;
        validate_schema(&tx, false)?;
        Store::validate_metadata(&tx, store.tenant_id())?;
        let integrity = Store::check_integrity(&tx)?;
        if integrity.valid {
            control::validate(&tx)?;
        }
        tx.require_business_access()?;
        epoch.validate_source(source)?;
        store.commit_checked(tx)?;
        epoch.validate_source(source)?;
        Ok(integrity)
    }
    /// Synthetic imported/corrupt history through the private checked appender.
    /// This bypasses public ingestion only inside this fixture and claims no
    /// new HTTP/worker reachability. No policy projection is materialized.
    pub(super) fn append_history_event(&self, kind: &str) -> anyhow::Result<()> {
        append_history_event(&self.stores[0], &self.owner, &self.sources[0], kind)
    }
    pub(super) fn assert_genesis_anchor_order_and_impossible_history(&self) {
        let db = self.stores[0].db.lock().unwrap();
        let stored:StoredEvent=db.query_row("SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE kind='control.egress_policy.genesis'",[],row_event).unwrap();
        let mut genesis: Genesis = serde_json::from_value(stored.event.data).unwrap();
        require_genesis_anchor_order(&genesis, stored.seq).unwrap();
        let authority = review_authority::replay(&db).unwrap();
        assert_eq!(genesis.source_anchor, authority.anchor_receipt);
        // Preserve predecessor adjacency while moving the candidate genesis
        // before its referenced original14 anchor: this impossible ordering
        // must fail independently of the existing adjacency check.
        genesis.previous_seq = genesis.source_anchor.seq - 1;
        let error = require_genesis_anchor_order(&genesis, genesis.source_anchor.seq).unwrap_err();
        assert_eq!(
            error.downcast_ref::<EgressStoreFailure>(),
            Some(&EgressStoreFailure::Integrity)
        );
    }
    pub(super) fn rows_for(&self, store: &Store) -> Rows {
        let db = store.db.lock().unwrap();
        let count = |table: &str| -> anyhow::Result<usize> {
            let count: i64 =
                db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
            usize::try_from(count).context("fixture SQL count is negative or outside usize range")
        };
        let mut all = Vec::new();
        for (table, columns) in [
            ("egress_policy_meta", 5),
            ("egress_policy_revisions", 6),
            ("egress_policy_heads", 6),
            ("egress_policy_operations", 5),
            ("egress_policy_reservations", 4),
        ] {
            all.extend(
                db.prepare(&format!("SELECT * FROM {table} ORDER BY 1,2"))
                    .unwrap()
                    .query_map([], |r| {
                        (0..columns)
                            .map(|i| r.get(i))
                            .collect::<rusqlite::Result<Vec<rusqlite::types::Value>>>()
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap(),
            );
        }
        Rows {
            heads: count("egress_policy_heads").unwrap(),
            revisions: count("egress_policy_revisions").unwrap(),
            operations: count("egress_policy_operations").unwrap(),
            reservations: count("egress_policy_reservations").unwrap(),
            chain: audit_head(&db).unwrap(),
            quota: db
                .query_row("SELECT bytes FROM quota WHERE id=1", [], |r| r.get(0))
                .unwrap(),
            all,
        }
    }
    pub(super) fn try_reopen(&self, index: usize) -> anyhow::Result<Store> {
        self.try_reopen_redactor(index, Redactor::new(vec![]))
    }
    fn try_reopen_redactor(&self, index: usize, redactor: Redactor) -> anyhow::Result<Store> {
        let source = &self.sources[index];
        let d = source.descriptor();
        let startup = Store::open_review_startup_for_tenant(
            &d.storage_binding.path(),
            d.tenant_id.as_deref().unwrap(),
            redactor,
            160,
            source,
        )?;
        let epoch = self.owner.try_write_epoch()?;
        startup.retain(&epoch, source)
    }
    pub(super) fn reopen(&self, index: usize) -> Store {
        self.try_reopen(index).unwrap()
    }
    pub(super) fn reopen_with_redactor(&self, redactor: Redactor) -> Store {
        self.try_reopen_redactor(0, redactor).unwrap()
    }
    fn fill_request(&self, class: FillClass, index: u64) -> anyhow::Result<FillRequest> {
        if class == FillClass::Identity {
            let mut account: crate::identity::HumanAccountV1 = identity::json_row(
                &self.stores[0].db.lock().unwrap(),
                "SELECT payload_json FROM identity_accounts WHERE principal_key=?1",
                &self.account.principal.key()?,
            )?
            .context("filler account missing")?;
            let expected_revision = account.revision;
            account.revision = account
                .revision
                .checked_add(1)
                .context("fixture revision exhausted")?;
            Ok(FillRequest::Identity(crate::identity::IdentityMutationV1 {
                version: 1,
                operation_id: Uuid::new_v4().to_string(),
                actor: crate::identity::PrincipalRefV1::LegacyControl {
                    credential_id: "synthetic-control".into(),
                    role: crate::identity::LegacyControlRoleV1::Runner,
                },
                change: crate::identity::IdentityChangeV1::Account {
                    expected_revision,
                    account,
                },
            }))
        } else {
            let mut event = Event::new("fixture.capacity");
            event.data = json!({"index":index});
            Ok(FillRequest::Event(event))
        }
    }
    fn attempt_fill(
        &self,
        class: FillClass,
        source: &OwnedSourceToken,
        request: &FillRequest,
    ) -> anyhow::Result<()> {
        let epoch = self.owner.try_write_epoch()?;
        let store = &self.stores[0];
        match request {
            FillRequest::Identity(mutation) => {
                store.mutate_identity_scoped(&epoch, source, mutation)?;
                Ok(())
            }
            FillRequest::Event(event) => {
                let access = HumanSourceAccess::write(&epoch, source)?;
                let mut db = store.db.lock().unwrap();
                let tx =
                    store.begin_checked_scoped(&mut db, &access, StoreOperationClass::Business)?;
                let (event, payload) = store.prepare_event(event.clone())?;
                let admission = match class {
                    FillClass::Ordinary => capacity::Class::OrdinaryWrite,
                    FillClass::Control => capacity::Class::ControlWrite,
                    FillClass::Identity => anyhow::bail!(
                        "identity filler must use the real identity mutation entrance"
                    ),
                };
                store.append_with_capacity_class(&tx, event, payload, admission)?;
                store.commit_checked(tx)
            }
        }
    }
    fn saturation_event_limit(&self, class: FillClass) -> anyhow::Result<u64> {
        let store = &self.stores[0];
        let db = store.db.lock().unwrap();
        let held: i64 = db.query_row(
            "SELECT coalesce(sum(events),0) FROM egress_policy_reservations",
            [],
            |r| r.get(0),
        )?;
        ensure(held > 0)?;
        let class_reserve = match class {
            FillClass::Ordinary => store
                .capacity_policy
                .control_reserve
                .events
                .checked_add(store.capacity_policy.identity_reserve.events)
                .context("fixture reserve overflow")?,
            FillClass::Control => store.capacity_policy.identity_reserve.events,
            FillClass::Identity => 0,
        };
        store
            .max_events
            .checked_sub(u64::try_from(held)?)
            .and_then(|n| n.checked_sub(class_reserve))
            .context("fixture event boundary is invalid")
    }
    fn saturate_using_source(
        &self,
        class: FillClass,
        source: &OwnedSourceToken,
    ) -> anyhow::Result<SaturationObservation> {
        let (before_events, before_bytes) = self.audit_occupancy();
        for accepted in 0..512u64 {
            let request = self.fill_request(class, accepted)?;
            let before_attempt = self.rows(0);
            match self.attempt_fill(class, source, &request) {
                Ok(()) => {
                    let (events, bytes) = self.audit_occupancy();
                    ensure(
                        events
                            == u64::try_from(before_attempt.chain.0)?
                                .checked_add(1)
                                .context("fixture event count overflow")?
                            && bytes > u64::try_from(before_attempt.quota)?,
                    )?;
                }
                Err(error) => {
                    require_saturation_cause(class, error)?;
                    ensure(accepted > 0 && self.rows(0) == before_attempt)?;
                    let (at_events, at_bytes) = self.audit_occupancy();
                    let event_limit = self.saturation_event_limit(class)?;
                    ensure(
                        at_events == event_limit
                            && at_events.checked_sub(before_events) == Some(accepted)
                            && at_bytes > before_bytes,
                    )?;
                    let repeat = self
                        .attempt_fill(class, source, &request)
                        .err()
                        .context("the same refused fill operation unexpectedly succeeded")?;
                    require_saturation_cause(class, repeat)?;
                    ensure(self.rows(0) == before_attempt)?;
                    return Ok(SaturationObservation {
                        accepted,
                        before_events,
                        before_bytes,
                        at_events,
                        at_bytes,
                        event_limit,
                    });
                }
            }
        }
        anyhow::bail!("bounded audit filler did not reach the exact capacity boundary")
    }
    pub(super) fn saturate_audit(&self, class: FillClass) -> anyhow::Result<SaturationObservation> {
        self.saturate_using_source(class, &self.sources[0])
    }
    pub(super) fn saturate_audit_with_foreign_source(
        &self,
        class: FillClass,
    ) -> anyhow::Result<SaturationObservation> {
        self.saturate_using_source(class, &self.sources[1])
    }
    pub(super) fn audit_occupancy(&self) -> (u64, u64) {
        let db = self.stores[0].db.lock().unwrap();
        let (events, serialized_bytes): (i64, i64) = db
            .query_row(
                "SELECT count(*),coalesce(sum(length(CAST(payload AS BLOB))),0) FROM events",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let quota_bytes: i64 = db
            .query_row("SELECT bytes FROM quota WHERE id=1", [], |r| r.get(0))
            .unwrap();
        let events = u64::try_from(events)
            .context("fixture event count is negative")
            .unwrap();
        let serialized_bytes = u64::try_from(serialized_bytes)
            .context("fixture serialized byte count is negative")
            .unwrap();
        let quota_bytes = u64::try_from(quota_bytes)
            .context("fixture quota byte count is negative")
            .unwrap();
        assert_eq!(events, u64::try_from(audit_head(&db).unwrap().0).unwrap());
        assert_eq!(serialized_bytes, quota_bytes);
        (events, quota_bytes)
    }
    pub(super) fn reservation(
        &self,
        selector: &EgressPolicySelectorV1,
    ) -> anyhow::Result<Option<(u64, u64)>> {
        let (kind, id) = selector.sql();
        let reservation:Option<(i64,i64)>=self.stores[0].db.lock().unwrap().query_row("SELECT events,bytes FROM egress_policy_reservations WHERE scope_kind=?1 AND scope_id=?2",params![kind,id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        reservation
            .map(|(events, bytes)| -> anyhow::Result<(u64, u64)> {
                Ok((
                    u64::try_from(events).context("fixture reservation event count is negative")?,
                    u64::try_from(bytes).context("fixture reservation byte count is negative")?,
                ))
            })
            .transpose()
    }
    pub(super) fn exhaust_policy_audit(&self) {
        self.saturate_audit(FillClass::Identity).unwrap();
    }
    pub(super) fn force_sqlite_full(&self) {
        let db = self.stores[0].db.lock().unwrap();
        let pages: i64 = db
            .pragma_query_value(None, "page_count", |r| r.get(0))
            .unwrap();
        db.pragma_update(None, "max_page_count", pages).unwrap();
    }
    fn session_grant(
        &self,
        selector: &EgressPolicySelectorV1,
        active: bool,
        revision: u64,
    ) -> crate::identity::ResourceGrantV1 {
        let EgressPolicySelectorV1::Session(id) = selector else {
            panic!("session fixture requires a session")
        };
        crate::identity::ResourceGrantV1 {
            version: 1,
            tenant_id: "A".into(),
            organization_scope: "organization-A".into(),
            resource: crate::identity::ResourceRefV1 {
                kind: crate::identity::ResourceKindV1::Session,
                resource_id: id.clone(),
            },
            owner: self.account.principal.clone(),
            revision,
            active,
            shares: vec![],
        }
    }
    pub(super) fn create_session(&self, selector: &EgressPolicySelectorV1) {
        let EgressPolicySelectorV1::Session(id) = selector else {
            panic!("session required")
        };
        {
            let epoch = self.owner.try_write_epoch().unwrap();
            let access = HumanSourceAccess::write(&epoch, &self.sources[0]).unwrap();
            let store = &self.stores[0];
            let mut db = store.db.lock().unwrap();
            let tx = store
                .begin_checked_scoped(&mut db, &access, StoreOperationClass::Business)
                .unwrap();
            let mut event = Event::new("fixture.session_created");
            event.session_id = Some(id.clone());
            let (event, payload) = store.prepare_event(event).unwrap();
            store.append_in_transaction(&tx, event, payload).unwrap();
            store.commit_checked(tx).unwrap();
        }
        mutate(
            &self.owner,
            &self.sources[0],
            &self.stores[0],
            crate::identity::IdentityChangeV1::ResourceGrant {
                expected_revision: 0,
                grant: self.session_grant(selector, true, 1),
            },
        );
    }
    pub(super) fn end_session(&self, selector: &EgressPolicySelectorV1) {
        mutate(
            &self.owner,
            &self.sources[0],
            &self.stores[0],
            crate::identity::IdentityChangeV1::ResourceGrant {
                expected_revision: 1,
                grant: self.session_grant(selector, false, 2),
            },
        );
    }
    pub(super) fn current_intersection_allows(
        &self,
        selector: &EgressPolicySelectorV1,
        destination: &str,
    ) -> bool {
        let db = self.stores[0].db.lock().unwrap();
        let p = replay(&db).unwrap();
        let Some(tenant) = p.heads.get(&EgressPolicySelectorV1::Tenant) else {
            return false;
        };
        let Some(session) = p.heads.get(selector) else {
            return false;
        };
        tenant.state == EgressPolicyStateV1::Active
            && session.state == EgressPolicyStateV1::Active
            && matches!(
                tenant.policy.core().unwrap().evaluate(
                    &session.policy.core().unwrap(),
                    &Authority::parse_connect(destination).unwrap(),
                    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap()
                ),
                crate::egress::PolicyDecision::PolicyAllowed(_)
            )
    }
    pub(super) fn corrupt_projection(&self, table: &str) {
        let db = self.stores[0].db.lock().unwrap();
        let sql = match table {
            "egress_policy_meta" => "UPDATE egress_policy_meta SET limits_json='{}'",
            "egress_policy_heads" => "UPDATE egress_policy_heads SET epoch=epoch+1",
            "egress_policy_operations" => "UPDATE egress_policy_operations SET receipt_json='{}'",
            "egress_policy_reservations" => "DELETE FROM egress_policy_reservations",
            "egress_policy_revisions" => {
                "UPDATE egress_policy_revisions SET policy_digest=printf('%064d',0)"
            }
            _ => panic!("unknown fixture projection"),
        };
        db.execute(sql, []).unwrap();
    }
}

fn mutate(
    owner: &OwnedHumanSources,
    source: &OwnedSourceToken,
    store: &Store,
    change: crate::identity::IdentityChangeV1,
) {
    let epoch = owner.try_write_epoch().unwrap();
    let mutation = crate::identity::IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: crate::identity::PrincipalRefV1::LegacyControl {
            credential_id: "synthetic-control".into(),
            role: crate::identity::LegacyControlRoleV1::Runner,
        },
        change,
    };
    store
        .mutate_identity_scoped(&epoch, source, &mutation)
        .unwrap();
}

fn append_history_event(
    store: &Store,
    owner: &OwnedHumanSources,
    source: &OwnedSourceToken,
    kind: &str,
) -> anyhow::Result<()> {
    let epoch = owner.try_write_epoch()?;
    let access = HumanSourceAccess::write(&epoch, source)?;
    let mut db = store.db.lock().unwrap();
    let tx = store.begin_checked_scoped(&mut db, &access, StoreOperationClass::Business)?;
    let mut event = Event::new(kind);
    event.coverage = "native".into();
    let (event, payload) = store.prepare_event(event)?;
    store.append_in_transaction(&tx, event, payload)?;
    store.commit_checked(tx)
}

pub(super) fn assert_format14_reserved_history_refused(kind: &str) {
    let base = BaseFixture::new(&["A"]);
    let anchors = base.migrate_all();
    let owner = base.owner(&anchors, "active14-history.json", 14);
    let source = owner
        .source_token(base.plan.target.sources[0].source_id)
        .unwrap();
    let startup = base.startup(0, &source);
    let store = {
        let epoch = owner.try_write_epoch().unwrap();
        startup
            .append_incarnation(&epoch, &source, Uuid::new_v4(), 0)
            .unwrap();
        startup.retain(&epoch, &source).unwrap()
    };
    append_history_event(&store, &owner, &source, kind).unwrap();
    let d = source.descriptor();
    let error = Store::open_review_startup_for_tenant(
        &d.storage_binding.path(),
        "A",
        Redactor::new(vec![]),
        160,
        &source,
    )
    .err()
    .expect("format14 reserved history must refuse reopen");
    assert_eq!(
        error.downcast_ref::<EgressStoreFailure>(),
        Some(&EgressStoreFailure::Integrity)
    );
    assert!(
        !store
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name GLOB 'egress_policy_*')",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );
}

struct PreparedMaintenance15 {
    plan: HumanSourcesMaintenancePlanV1,
    file: PrivateFile,
}
impl PreparedMaintenance15 {
    fn acquire(
        &self,
        base: &BaseFixture,
    ) -> Result<HumanSourcesMaintenanceGuard, crate::human_authority::HumanAuthorityFailure> {
        self.acquire_with_configuration(base, self.plan.target.configuration.clone())
    }
    fn acquire_with_configuration(
        &self,
        base: &BaseFixture,
        configuration: HumanAuthorityConfigurationV1,
    ) -> Result<HumanSourcesMaintenanceGuard, crate::human_authority::HumanAuthorityFailure> {
        let resolved = HumanMaintenanceResolvedViewV1::new(
            self.plan.target.sources.clone(),
            configuration,
            base.configuration_files.clone(),
        )?;
        HumanSourcesMaintenanceGuard::acquire(
            self.file.clone(),
            HumanSourcesMaintenanceConfigurationV1::new(None, resolved, None),
        )
    }
}
fn prepare_maintenance15(base: &BaseFixture) -> anyhow::Result<PreparedMaintenance15> {
    let mut plan = base.plan.clone();
    plan.migration_id = Uuid::new_v4();
    for source in &mut plan.target.sources {
        source.required_format = 15;
    }
    let config = Directory::open_existing(
        base.plan_file
            .path()
            .parent()
            .context("fixture maintenance parent missing")?,
    )?;
    let file = config.persistent_create(
        &format!("maintenance15-{}.json", plan.migration_id),
        &serde_json::to_vec(&plan)?,
    )?;
    Ok(PreparedMaintenance15 { plan, file })
}
fn migrate15(base: &BaseFixture, limits: &EgressStoreLimitsV1) {
    let prepared = prepare_maintenance15(base).unwrap();
    let guard = prepared.acquire(base).unwrap();
    complete_migration15(base, limits, &prepared, &guard);
}
fn complete_migration15(
    base: &BaseFixture,
    limits: &EgressStoreLimitsV1,
    prepared: &PreparedMaintenance15,
    guard: &HumanSourcesMaintenanceGuard,
) {
    for (index, source) in prepared.plan.target.sources.iter().enumerate() {
        let token = guard.target_source(source.source_id).unwrap();
        let opened = base.open_maintenance(index, &token);
        let parent = base
            .root
            .persistent_child("egress-backups", true)
            .unwrap()
            .unwrap();
        let backup_path = parent.path().join(format!("egress-{index}.sqlite"));
        opened
            .with_review_maintenance_writer(&token, |writer| {
                let backup = writer.prepare_egress_backup(&token, &backup_path)?;
                writer.install_egress_format15(&token, &backup, limits)?;
                writer.commit()
            })
            .unwrap();
        let reopened = base.open_maintenance(index, &token);
        reopened
            .with_review_maintenance_writer(&token, |writer| {
                let backup = writer.resume_egress_backup(&token, &backup_path)?;
                writer.install_egress_format15(&token, &backup, limits)?;
                writer.commit()
            })
            .unwrap();
    }
}

pub(super) fn assert_operator_recovers_after_confirmed_genesis_before_closure_publish() {
    use crate::human_authority::{
        migration::operator::{
            STOP_AFTER_STORE15_CONFIRMATION, Store15UpgradeRefusal, Store15UpgradeRequest,
            upgrade_store15,
        },
        startup::{HumanOwnedSourceOptions, HumanOwnedStartupRequest},
    };

    let base = BaseFixture::new(&["A", "B"]);
    let anchors = base.migrate_all();
    let prior_name = "store15-operator-prior.json";
    let owner = base.owner(&anchors, prior_name, 14);
    let prior = owner.closure().clone();
    drop(owner);
    let manifest = PrivateFile::open(
        &base.plan_file.path().parent().unwrap().join(prior_name),
        crate::human_authority::MAX_HUMAN_MANIFEST_BYTES as u64,
    )
    .unwrap();
    let operation_id = Uuid::new_v4();
    let output = base.root.path().join("store15-operator-output");
    let configured = [
        ("A", "synthetic-A-runner", "synthetic-A-viewer"),
        ("B", "synthetic-B-runner", "synthetic-B-viewer"),
    ];
    let run = |resume| {
        let sources = prior
            .sources
            .iter()
            .map(|source| {
                let max_events = 10_000;
                let capacity = AuditCapacityPolicyV1::resolved(max_events);
                let workload = WorkloadAuditPolicyV1::resolved(max_events, &capacity);
                (
                    source.source_id,
                    HumanOwnedSourceOptions::TenantStore {
                        redactor: Redactor::new(vec![]),
                        max_events,
                        capacity,
                        workload,
                        retention: Default::default(),
                    },
                )
            })
            .collect();
        let configuration = HumanSourcesConfigurationV1::new(
            prior.sources.clone(),
            prior.configuration.clone(),
            base.configuration_files.clone(),
        )
        .unwrap();
        upgrade_store15(Store15UpgradeRequest {
            prior: HumanOwnedStartupRequest {
                manifest: manifest.clone(),
                configuration,
                sources,
            },
            configured_credentials: &configured,
            tenant: "A",
            operation_id,
            output_directory: &output,
            resume,
        })
    };
    let before = maintenance_source_state(&base);
    let neighbor = store_source_state(&base.stores[1]);
    STOP_AFTER_STORE15_CONFIRMATION.with(|stop| stop.set(true));
    let error = run(false)
        .err()
        .expect("confirmation interruption expected");
    assert_eq!(
        error.downcast_ref::<Store15UpgradeRefusal>(),
        Some(&Store15UpgradeRefusal::TransitionUnconfirmed)
    );
    let committed = maintenance_source_state(&base);
    assert_eq!(before.format, 14);
    assert_eq!(committed.format, 15);
    assert_eq!(committed.head.0, before.head.0 + 1);
    assert!(
        output
            .join("journal/egress15-confirmed-v2/record-v1.json")
            .is_file()
    );
    assert!(!output.join("closure").exists());
    assert_eq!(store_source_state(&base.stores[1]), neighbor);

    let result = run(true).unwrap();
    assert_eq!(result.genesis_sequence, committed.head.0);
    assert_eq!(result.closure_revision, prior.closure_revision + 1);
    assert!(output.join("closure/manifest.json").is_file());
    assert_eq!(maintenance_source_state(&base), committed);
    assert_eq!(store_source_state(&base.stores[1]), neighbor);
}

pub(super) fn assert_neighbor_store14_head_is_anchored_during_egress_upgrade() {
    use crate::control_fs::publication::FAIL_AFTER_RENAME;
    use crate::human_authority::migration::egress_journal::RetainedEgressIntent;

    let base = BaseFixture::new(&["A", "B"]);
    let anchors = base.migrate_all();
    let manifest_name = "active14-before-egress.json";
    let prior_owner = base.owner(&anchors, manifest_name, 14);
    let prior = &base.plan.target;
    let neighbor_id = prior.sources[1].source_id;
    let expected_head = {
        let source = prior_owner.source_token(neighbor_id).unwrap();
        let startup = base.startup(1, &source);
        let epoch = prior_owner.try_write_epoch().unwrap();
        startup
            .append_incarnation(&epoch, &source, Uuid::new_v4(), 0)
            .unwrap()
    };
    assert!(expected_head.seq > anchors[1].seq);
    let configuration = Directory::open_existing(base.plan_file.path().parent().unwrap()).unwrap();
    let prior_file = PrivateFile::open(
        &configuration.path().join(manifest_name),
        crate::human_authority::MAX_HUMAN_MANIFEST_BYTES as u64,
    )
    .unwrap();
    let closure = prior_owner.closure();
    let manifest = HumanMaintenanceActiveManifestV1 {
        storage_binding: binding(&configuration, manifest_name),
        closure_id: closure.closure_id,
        closure_revision: closure.closure_revision,
        closure_digest: closure.closure_digest.clone(),
    };
    drop(prior_owner);
    {
        let guard_without_manifest = base.maintenance();
        let token = guard_without_manifest.target_source(neighbor_id).unwrap();
        let error = Store::observe_unchanged_review_format14(&token).unwrap_err();
        assert_eq!(
            error.downcast_ref::<authority::ReviewAuthorityFailure>(),
            Some(&authority::ReviewAuthorityFailure::Required)
        );
    }

    let mut plan = base.plan.clone();
    plan.migration_id = Uuid::new_v4();
    plan.prior = Some(prior.clone());
    plan.prior_active_manifest = Some(manifest);
    plan.target.sources[0].required_format = 15;
    let resolve = |view: &HumanMaintenanceViewV1| {
        HumanMaintenanceResolvedViewV1::new(
            view.sources.clone(),
            view.configuration.clone(),
            base.configuration_files.clone(),
        )
        .unwrap()
    };
    let plan_file = configuration
        .persistent_create(
            "maintenance-a15-b14.json",
            &serde_json::to_vec(&plan).unwrap(),
        )
        .unwrap();
    let acquire = || {
        HumanSourcesMaintenanceGuard::acquire(
            plan_file.clone(),
            HumanSourcesMaintenanceConfigurationV1::new(
                Some(resolve(prior)),
                resolve(&plan.target),
                Some(prior_file.clone()),
            ),
        )
        .unwrap()
    };
    let guard = acquire();
    let selected = guard
        .target_source(plan.target.sources[0].source_id)
        .unwrap();
    let neighbor = guard.target_source(neighbor_id).unwrap();
    let error = Store::observe_unchanged_review_format14(&selected).unwrap_err();
    assert_eq!(
        error.downcast_ref::<authority::ReviewAuthorityFailure>(),
        Some(&authority::ReviewAuthorityFailure::Required)
    );
    assert_eq!(
        Store::observe_unchanged_review_format14(&neighbor).unwrap(),
        expected_head
    );

    let journal = base
        .root
        .persistent_child("egress-intent", true)
        .unwrap()
        .unwrap();
    let backup_parent = base
        .root
        .persistent_child("egress-backup", true)
        .unwrap()
        .unwrap();
    let copy = backup_parent.path().join("other-copy.sqlite");
    let limits = EgressStoreLimitsV1::default();
    let initial = maintenance_source_state(&base);
    assert_eq!(initial.format, 14);
    let opened = base.open_maintenance(0, &selected);
    let intent = opened
        .with_review_maintenance_writer(&selected, |writer| {
            assert!(
                RetainedEgressIntent::resume(
                    &selected,
                    writer,
                    journal.clone(),
                    backup_parent.clone(),
                    &limits,
                )
                .is_err()
            );
            assert!(journal.persistent_entries(1)?.is_empty());
            FAIL_AFTER_RENAME.with(|fault| fault.set(true));
            let interrupted = RetainedEgressIntent::prepare(
                &selected,
                writer,
                journal.clone(),
                backup_parent.clone(),
                &limits,
            )
            .map(|_| ())
            .unwrap_err();
            assert_eq!(
                interrupted.to_string(),
                "egress journal publication durability unknown"
            );
            assert_eq!(maintenance_source_state(&base), initial);
            assert!(
                RetainedEgressIntent::prepare(
                    &selected,
                    writer,
                    journal.clone(),
                    backup_parent.clone(),
                    &limits,
                )
                .is_err()
            );
            let intent = RetainedEgressIntent::resume(
                &selected,
                writer,
                journal.clone(),
                backup_parent.clone(),
                &limits,
            )?;
            let backup_path = intent.backup_path()?;
            assert_eq!(backup_path, backup_parent.path().join("source.sqlite"));
            assert!(intent.resume_backup(writer).is_err());
            assert!(!backup_path.exists());

            let unframed = RetainedEgressIntent::prepare(
                &selected,
                writer,
                base.root.exclusive_child("egress-unframed-journal")?,
                base.root.exclusive_child("egress-unframed-backup")?,
                &limits,
            )?;
            let unframed_path = unframed.backup_path()?;
            let unframed_backup = writer.prepare_egress_backup(&selected, &unframed_path)?;
            assert!(unframed.resume_backup(writer).is_err());
            assert!(unframed.prepare_backup(writer).is_err());
            unframed_backup.journal_observation(&selected)?;
            assert_eq!(maintenance_source_state(&base), initial);

            FAIL_AFTER_RENAME.with(|fault| fault.set(true));
            let interrupted = intent.prepare_backup(writer).map(|_| ()).unwrap_err();
            assert_eq!(
                interrupted.to_string(),
                "egress journal publication durability unknown"
            );
            assert!(intent.prepare_backup(writer).is_err());
            let backup = intent.resume_backup(writer)?;
            writer.prepare_egress_backup(&selected, &copy)?;
            let pending = backup.install_format15(writer)?;
            assert_eq!(pending.seq, initial.head.0 + 1);
            drop(backup);
            Ok(intent)
        })
        .unwrap();
    assert_eq!(maintenance_source_state(&base), initial);
    let uncommitted = intent
        .confirm_committed(Redactor::new(vec![]), 10_000)
        .unwrap_err();
    assert_eq!(
        uncommitted.downcast_ref::<EgressStoreFailure>(),
        Some(&EgressStoreFailure::Integrity)
    );
    assert_eq!(journal.persistent_entries(3).unwrap().len(), 2);
    let applied = opened
        .with_review_maintenance_writer(&selected, |writer| {
            let backup = intent.resume_backup(writer)?;
            let receipt = backup.install_format15(writer)?;
            writer.commit()?;
            Ok(receipt)
        })
        .unwrap();
    let committed = maintenance_source_state(&base);
    assert_eq!(committed.format, 15);
    assert_eq!(
        Store::observe_unchanged_review_format14(&neighbor).unwrap(),
        expected_head
    );
    drop(intent);
    drop(opened);
    drop(selected);
    drop(neighbor);
    drop(guard);
    let guard = acquire();
    let selected = guard
        .target_source(plan.target.sources[0].source_id)
        .unwrap();
    let reopened = base.open_maintenance(0, &selected);
    let retained = reopened
        .with_review_maintenance_writer(&selected, |writer| {
            let changed_limits = EgressStoreLimitsV1::new(1, 16_384, 16 * 1_048_576)?;
            let conflict = RetainedEgressIntent::resume(
                &selected,
                writer,
                journal.clone(),
                backup_parent.clone(),
                &changed_limits,
            )
            .map(|_| ())
            .unwrap_err();
            assert_eq!(conflict.to_string(), "egress intent differs");
            let new_journal = base
                .root
                .persistent_child("egress-intent-too-late", true)?
                .unwrap();
            let new_backup = base
                .root
                .persistent_child("egress-backup-too-late", true)?
                .unwrap();
            let installed = RetainedEgressIntent::prepare(
                &selected,
                writer,
                new_journal.clone(),
                new_backup,
                &limits,
            )
            .map(|_| ())
            .unwrap_err();
            assert_eq!(
                installed.downcast_ref::<EgressStoreFailure>(),
                Some(&EgressStoreFailure::Integrity)
            );
            assert!(new_journal.persistent_entries(1)?.is_empty());
            let retained = RetainedEgressIntent::resume(
                &selected,
                writer,
                journal.clone(),
                backup_parent.clone(),
                &limits,
            )?;
            retained.resume_backup(writer)?;
            Ok(retained)
        })
        .unwrap();
    drop(reopened);
    let abandoned = journal
        .exclusive_child(&format!("egress15-stage-{}", Uuid::new_v4()))
        .unwrap();
    let partial = abandoned.persistent_create("record-v1.json", b"{").unwrap();
    FAIL_AFTER_RENAME.with(|fault| fault.set(true));
    let interrupted = retained
        .confirm_committed(Redactor::new(vec![]), 10_000)
        .unwrap_err();
    assert_eq!(
        interrupted.to_string(),
        "egress journal publication durability unknown"
    );
    assert_eq!(maintenance_source_state(&base), committed);
    let read_record = |slot| {
        journal
            .persistent_child(slot, false)
            .unwrap()
            .unwrap()
            .persistent_read("record-v1.json", 128 * 1024, &[])
            .unwrap()
            .unwrap()
    };
    let confirmation = read_record("egress15-confirmed-v2");
    let intent_file = read_record("egress15-intent-v2");
    let frame = read_record("egress15-backup-v2");
    assert_eq!(
        serde_json::from_slice::<Value>(confirmation.bytes()).unwrap(),
        json!({
            "version": 1,
            "intent_digest": intent_file.digest(),
            "backup_digest": frame.digest(),
            "receipt": {"seq": applied.seq, "hash": applied.hash},
        }),
    );
    drop(retained);
    drop(selected);
    drop(guard);
    let guard = acquire();
    let selected = guard
        .target_source(plan.target.sources[0].source_id)
        .unwrap();
    let reopened = base.open_maintenance(0, &selected);
    let retained = reopened
        .with_review_maintenance_writer(&selected, |writer| {
            RetainedEgressIntent::resume(
                &selected,
                writer,
                journal.clone(),
                backup_parent.clone(),
                &limits,
            )
        })
        .unwrap();
    drop(reopened);
    let confirmed = retained
        .confirm_committed(Redactor::new(vec![]), 10_000)
        .unwrap();
    assert_eq!(confirmed, applied);
    partial.persistent_revalidate().unwrap();
    assert_eq!(partial.bytes(), b"{");
    confirmation.persistent_revalidate().unwrap();
    assert_eq!(maintenance_source_state(&base), committed);
    assert_eq!(
        Store::observe_unchanged_review_format14(&guard.target_source(neighbor_id).unwrap())
            .unwrap(),
        expected_head
    );

    // Both backups were produced by SQLite before the commit. Preserve the
    // original to test an ordinary copy substitution after closing owners.
    let move_backup = |from: &str, to: &str| {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let old = backup_parent.path().join(format!("{from}{suffix}"));
            let new = backup_parent.path().join(format!("{to}{suffix}"));
            assert!(!new.exists());
            if old.exists() {
                std::fs::rename(old, new).unwrap();
            }
        }
    };
    move_backup("source.sqlite", "original-preserved.sqlite");
    move_backup("other-copy.sqlite", "source.sqlite");
    let reopened = base.open_maintenance(0, &selected);
    reopened
        .with_review_maintenance_writer(&selected, |writer| {
            writer.resume_egress_backup(&selected, &retained.backup_path()?)?;
            Ok(())
        })
        .unwrap();
    drop(reopened);
    let mismatch = retained
        .confirm_committed(Redactor::new(vec![]), 10_000)
        .unwrap_err();
    assert_eq!(mismatch.to_string(), "egress backup record differs");
    frame.persistent_revalidate().unwrap();
    confirmation.persistent_revalidate().unwrap();
    assert_eq!(maintenance_source_state(&base), committed);
}

pub(super) fn assert_format14_compatibility_and_migration_refusals() {
    let base = BaseFixture::new(&["A"]);
    let anchors = base.migrate_all();
    {
        let guard = base.maintenance();
        let token = guard
            .target_source(base.plan.target.sources[0].source_id)
            .unwrap();
        let opened = base.open_maintenance(0, &token);
        {
            let db = base.stores[0].db.lock().unwrap();
            validate_schema(&db, false).unwrap();
            control::validate(&db).unwrap();
            assert_eq!(
                db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                    .unwrap(),
                14
            );
            assert!(
                !db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name GLOB 'egress_policy_*')",
                    [],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap()
            );
        }
        opened
            .with_review_maintenance_writer(&token, |writer| {
                assert!(
                    writer
                        .prepare_egress_backup(&token, &base.root.path().join("wrong-token.sqlite"))
                        .is_err()
                );
                Ok(())
            })
            .unwrap();
    }
    assert_eq!(anchors.len(), 1);
    migrate15(&base, &EgressStoreLimitsV1::default());
}

#[derive(Debug, PartialEq, Eq)]
struct MaintenanceSourceState {
    format: i64,
    head: (i64, String),
    schema: Vec<(String, String, String, Option<String>)>,
    quota: i64,
}
fn maintenance_source_state(base: &BaseFixture) -> MaintenanceSourceState {
    store_source_state(&base.stores[0])
}
fn store_source_state(store: &Store) -> MaintenanceSourceState {
    let db = store.db.lock().unwrap();
    MaintenanceSourceState {
        format: db
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap(),
        head: audit_head(&db).unwrap(),
        schema: db
            .prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap(),
        quota: db
            .query_row("SELECT bytes FROM quota WHERE id=1", [], |r| r.get(0))
            .unwrap(),
    }
}
pub(super) fn assert_format15_confirmation_only_reads_committed_receipt() {
    let base = BaseFixture::new(&["A"]);
    base.migrate_all();
    let prepared = prepare_maintenance15(&base).unwrap();
    let guard = prepared.acquire(&base).unwrap();
    let token = guard
        .target_source(prepared.plan.target.sources[0].source_id)
        .unwrap();
    let limits = EgressStoreLimitsV1::default();
    let backup_path = base.backup_path(1);
    let confirm = |token: &HumanMaintenanceSourceToken, limits: &EgressStoreLimitsV1| {
        Store::confirm_egress_format15(token, Redactor::new(vec![]), 10_000, &backup_path, limits)
    };
    let opened = base.open_maintenance(0, &token);
    let initial = maintenance_source_state(&base);
    opened
        .with_review_maintenance_writer(&token, |writer| {
            writer.prepare_egress_backup(&token, &backup_path)?;
            Ok(())
        })
        .unwrap();

    let before_commit = confirm(&token, &limits).unwrap_err();
    assert_eq!(
        before_commit.downcast_ref::<EgressStoreFailure>(),
        Some(&EgressStoreFailure::Integrity)
    );
    assert_eq!(maintenance_source_state(&base), initial);

    let pending = opened
        .with_review_maintenance_writer(&token, |writer| {
            let backup = writer.resume_egress_backup(&token, &backup_path)?;
            let receipt = writer.install_egress_format15(&token, &backup, &limits)?;
            writer.commit()?;
            Ok(receipt)
        })
        .unwrap();
    // Drop all maintenance capabilities without recording an acknowledgement.
    // The expected receipt stays only in the oracle.
    drop(opened);
    drop(token);
    drop(guard);
    let committed = maintenance_source_state(&base);
    assert_eq!(committed.format, 15);
    assert_eq!((pending.seq, pending.hash.clone()), committed.head);
    assert_eq!(pending.seq, initial.head.0 + 1);
    let reacquired = prepared.acquire(&base).unwrap();
    let token = reacquired
        .target_source(prepared.plan.target.sources[0].source_id)
        .unwrap();
    assert_eq!(confirm(&token, &limits).unwrap(), pending);
    let different_limits = EgressStoreLimitsV1::new(1, 16_384, 16 * 1_048_576).unwrap();
    let different = confirm(&token, &different_limits).unwrap_err();
    assert_eq!(
        different.downcast_ref::<EgressStoreFailure>(),
        Some(&EgressStoreFailure::Conflict)
    );
    assert_eq!(maintenance_source_state(&base), committed);
}

// Cross-process loss of the E1 acknowledgement. The parent prepares the real
// A15/B14 maintenance union, then re-executes this exact test with a phase
// variable. The child runs the real journal publication and SQLite commit,
// writes a test-only oracle and dies by SIGKILL: no destructor, unlock,
// SQLite close or acknowledgement runs. Only the parent judges the recovery.
const XPROC_TEST: &str = "audit::egress::tests::maintenance_confirmation_recovers_committed_genesis_after_child_process_kill";
const XPROC_PHASE: &str = "AI_GUARD_EGRESS15_XPROC_PHASE";
const XPROC_ROOT: &str = "AI_GUARD_EGRESS15_XPROC_ROOT";
const XPROC_ORACLE: &str = "AI_GUARD_EGRESS15_XPROC_ORACLE";
const XPROC_AFTER_COMMIT: &str = "after-sqlite-commit";
const XPROC_AFTER_CONFIRMATION: &str = "after-confirmation-record";
const XPROC_PLAN: &str = "maintenance-a15-b14-xproc.json";
const XPROC_PRIOR_MANIFEST: &str = "active14-before-xproc.json";
const XPROC_JOURNAL: &str = "egress-intent-xproc";
const XPROC_BACKUP: &str = "egress-backup-xproc";
const XPROC_INTENT_RECORD: &str = "egress15-intent-v2";
const XPROC_BACKUP_RECORD: &str = "egress15-backup-v2";
const XPROC_CONFIRMED_RECORD: &str = "egress15-confirmed-v2";
// Harness observation bound only, widened for a host under load 70-200 (the
// child normally ends far earlier by SIGKILL); no product deadline changes.
const XPROC_CHILD_BOUND: std::time::Duration = std::time::Duration::from_secs(720);

fn xproc_sha256(path: &std::path::Path) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(std::fs::read(path).unwrap()))
}

fn xproc_genesis_rows(store: &Store) -> (i64, Vec<(i64, String)>, i64) {
    let db = store.db.lock().unwrap();
    let genesis = db
        .query_row(
            "SELECT count(*) FROM events WHERE kind='control.egress_policy.genesis'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let meta = db
        .prepare("SELECT genesis_seq,genesis_hash FROM egress_policy_meta")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let operations = db
        .query_row("SELECT count(*) FROM egress_policy_operations", [], |r| {
            r.get(0)
        })
        .unwrap();
    (genesis, meta, operations)
}

fn xproc_journal_names(journal: &Directory) -> Vec<String> {
    let entries = journal.persistent_entries(8).unwrap();
    assert!(entries.iter().all(|entry| entry.directory));
    let mut names: Vec<_> = entries.into_iter().map(|entry| entry.name).collect();
    names.sort_unstable();
    names
}

// Main SQLite files only: WAL/SHM sidecars may legitimately appear on reads,
// and the reserved per-directory SQLite open lock is not a backup.
fn xproc_backup_mains(parent: &Directory) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(parent.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| {
            name != ".ai-guard-sqlite-open.lock"
                && !["-wal", "-shm", "-journal"]
                    .iter()
                    .any(|s| name.ends_with(s))
        })
        .collect();
    names.sort_unstable();
    names
}

/// Reopen plan, prior manifest and configuration files by path, as a fresh
/// process must, and take one real nonblocking maintenance guard.
fn acquire_xproc_maintenance(
    root: &Directory,
) -> (HumanSourcesMaintenancePlanV1, HumanSourcesMaintenanceGuard) {
    let configuration = root
        .persistent_child("configuration", false)
        .unwrap()
        .unwrap();
    let plan_file = PrivateFile::open(&configuration.path().join(XPROC_PLAN), 512 * 1024).unwrap();
    let plan: HumanSourcesMaintenancePlanV1 = serde_json::from_slice(plan_file.bytes()).unwrap();
    let prior_file = PrivateFile::open(
        &configuration.path().join(XPROC_PRIOR_MANIFEST),
        crate::human_authority::MAX_HUMAN_MANIFEST_BYTES as u64,
    )
    .unwrap();
    let files: Vec<_> = plan
        .target
        .configuration
        .files
        .iter()
        .map(|file| {
            (
                file.kind,
                PrivateFile::open(&file.storage_binding.path(), 1_048_576).unwrap(),
            )
        })
        .collect();
    let resolve = |view: &HumanMaintenanceViewV1| {
        HumanMaintenanceResolvedViewV1::new(
            view.sources.clone(),
            view.configuration.clone(),
            files.clone(),
        )
        .unwrap()
    };
    let guard = HumanSourcesMaintenanceGuard::acquire(
        plan_file,
        HumanSourcesMaintenanceConfigurationV1::new(
            Some(resolve(plan.prior.as_ref().unwrap())),
            resolve(&plan.target),
            Some(prior_file),
        ),
    )
    .unwrap();
    (plan, guard)
}

fn xproc_die_without_acknowledgement(
    oracle: &std::path::Path,
    phase: &str,
    receipt: &authority::AuthorityReceiptV1,
    backup_parent: &Directory,
    journal: &Directory,
) -> ! {
    let confirmed = journal
        .path()
        .join(XPROC_CONFIRMED_RECORD)
        .join("record-v1.json");
    let record = json!({
        "phase": phase,
        "pid": std::process::id(),
        "receipt": receipt,
        "backup_sha256": xproc_sha256(&backup_parent.path().join("source.sqlite")),
        "confirmed_record_sha256": confirmed.exists().then(|| xproc_sha256(&confirmed)),
    });
    // Test-only oracle outside every product path; it is no acknowledgement.
    std::fs::write(oracle, serde_json::to_vec(&record).unwrap()).unwrap();
    // SAFETY: signal to this very process. SIGKILL cannot be caught or
    // ignored, so no destructor, flock/SQLite close or acknowledgement runs.
    unsafe { libc::kill(libc::getpid(), libc::SIGKILL) };
    unreachable!("SIGKILL returned to the cross-process child")
}

fn xproc_child(phase: &str) -> ! {
    use crate::human_authority::migration::egress_journal::RetainedEgressIntent;
    assert!(
        matches!(phase, XPROC_AFTER_COMMIT | XPROC_AFTER_CONFIRMATION),
        "unknown cross-process phase {phase}"
    );
    let root =
        Directory::open_existing(std::path::Path::new(&std::env::var_os(XPROC_ROOT).unwrap()))
            .unwrap();
    let oracle = std::path::PathBuf::from(std::env::var_os(XPROC_ORACLE).unwrap());
    let (plan, guard) = acquire_xproc_maintenance(&root);
    let source = &plan.target.sources[0];
    let selected = guard.target_source(source.source_id).unwrap();
    let journal = root
        .persistent_child(XPROC_JOURNAL, false)
        .unwrap()
        .unwrap();
    let backup_parent = root.persistent_child(XPROC_BACKUP, false).unwrap().unwrap();
    let limits = EgressStoreLimitsV1::default();
    let opened = Store::open_review_maintenance_for_tenant(
        &source.storage_binding.path(),
        source.tenant_id.as_deref().unwrap(),
        Redactor::new(vec![]),
        10_000,
        &selected,
    )
    .unwrap();
    let (intent, receipt) = opened
        .with_review_maintenance_writer(&selected, |writer| {
            let intent = RetainedEgressIntent::prepare(
                &selected,
                writer,
                journal.clone(),
                backup_parent.clone(),
                &limits,
            )?;
            let backup = intent.prepare_backup(writer)?;
            let receipt = backup.install_format15(writer)?;
            writer.commit()?;
            drop(backup);
            if phase == XPROC_AFTER_COMMIT {
                // COMMIT returned; the receipt never leaves this closure.
                xproc_die_without_acknowledgement(
                    &oracle,
                    phase,
                    &receipt,
                    &backup_parent,
                    &journal,
                );
            }
            Ok((intent, receipt))
        })
        .unwrap();
    drop(opened);
    let confirmed = intent
        .confirm_committed(Redactor::new(vec![]), 10_000)
        .unwrap();
    assert_eq!(confirmed, receipt);
    // Confirmation record published; its result is never acknowledged.
    xproc_die_without_acknowledgement(&oracle, phase, &receipt, &backup_parent, &journal)
}

pub(super) fn assert_format15_confirmation_recovers_after_child_process_kill() {
    if let Ok(phase) = std::env::var(XPROC_PHASE) {
        xproc_child(&phase);
    }
    for phase in [XPROC_AFTER_COMMIT, XPROC_AFTER_CONFIRMATION] {
        xproc_parent(phase);
    }
}

fn xproc_parent(phase: &str) {
    use crate::human_authority::migration::egress_journal::RetainedEgressIntent;
    use std::os::unix::process::ExitStatusExt;

    let base = BaseFixture::new(&["A", "B"]);
    let anchors = base.migrate_all();
    let prior_owner = base.owner(&anchors, XPROC_PRIOR_MANIFEST, 14);
    let neighbor_id = base.plan.target.sources[1].source_id;
    let expected_head = {
        let source = prior_owner.source_token(neighbor_id).unwrap();
        let startup = base.startup(1, &source);
        let epoch = prior_owner.try_write_epoch().unwrap();
        startup
            .append_incarnation(&epoch, &source, Uuid::new_v4(), 0)
            .unwrap()
    };
    assert!(expected_head.seq > anchors[1].seq);
    let configuration = Directory::open_existing(base.plan_file.path().parent().unwrap()).unwrap();
    let closure = prior_owner.closure();
    let manifest = HumanMaintenanceActiveManifestV1 {
        storage_binding: binding(&configuration, XPROC_PRIOR_MANIFEST),
        closure_id: closure.closure_id,
        closure_revision: closure.closure_revision,
        closure_digest: closure.closure_digest.clone(),
    };
    drop(prior_owner);
    let mut plan = base.plan.clone();
    plan.migration_id = Uuid::new_v4();
    plan.prior = Some(base.plan.target.clone());
    plan.prior_active_manifest = Some(manifest);
    plan.target.sources[0].required_format = 15;
    drop(
        configuration
            .persistent_create(XPROC_PLAN, &serde_json::to_vec(&plan).unwrap())
            .unwrap(),
    );
    let journal = base
        .root
        .persistent_child(XPROC_JOURNAL, true)
        .unwrap()
        .unwrap();
    let backup_parent = base
        .root
        .persistent_child(XPROC_BACKUP, true)
        .unwrap()
        .unwrap();
    let backup_path = backup_parent.path().join("source.sqlite");
    let initial = maintenance_source_state(&base);
    assert_eq!(initial.format, 14);
    let neighbor_initial = store_source_state(&base.stores[1]);
    assert_eq!(neighbor_initial.format, 14);
    assert_eq!(
        neighbor_initial.head,
        (expected_head.seq, expected_head.hash.clone())
    );
    assert!(journal.persistent_entries(1).unwrap().is_empty());
    assert!(!backup_path.exists());

    // Same binary, same --exact filter; the phase variable selects the child.
    let observation = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
        .unwrap();
    let oracle = observation.path().join("oracle.json");
    let stdout_path = observation.path().join("child.stdout");
    let stderr_path = observation.path().join("child.stderr");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([XPROC_TEST, "--exact", "--test-threads=1", "--nocapture"])
        .env(XPROC_PHASE, phase)
        .env(XPROC_ROOT, base.root.path())
        .env(XPROC_ORACLE, &oracle)
        .stdin(std::process::Stdio::null())
        .stdout(std::fs::File::create(&stdout_path).unwrap())
        .stderr(std::fs::File::create(&stderr_path).unwrap())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > XPROC_CHILD_BOUND {
            let _ = child.kill();
            let _ = child.wait();
            panic!("cross-process child {phase} exceeded the harness observation bound");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let child_output = || {
        format!(
            "child stdout:\n{}\nchild stderr:\n{}",
            std::fs::read_to_string(&stdout_path).unwrap_or_default(),
            std::fs::read_to_string(&stderr_path).unwrap_or_default()
        )
    };
    assert_eq!(
        status.signal(),
        Some(libc::SIGKILL),
        "child {phase} must die by SIGKILL after its durable step ({status}); {}",
        child_output()
    );
    assert_ne!(child.id(), std::process::id());
    let record: Value = serde_json::from_slice(&std::fs::read(oracle.as_path()).unwrap()).unwrap();
    assert_eq!(record["phase"], phase);
    assert_eq!(record["pid"].as_u64(), Some(u64::from(child.id())));
    let pending: authority::AuthorityReceiptV1 =
        serde_json::from_value(record["receipt"].clone()).unwrap();
    let backup_digest = record["backup_sha256"].as_str().unwrap().to_owned();
    let child_confirmation = record["confirmed_record_sha256"]
        .as_str()
        .map(str::to_owned);
    assert_eq!(
        child_confirmation.is_some(),
        phase == XPROC_AFTER_CONFIRMATION
    );

    // Only the dead child committed: one genesis at head, B and backup intact.
    let committed = maintenance_source_state(&base);
    assert_eq!(committed.format, 15);
    assert_eq!((pending.seq, pending.hash.clone()), committed.head);
    assert_eq!(pending.seq, initial.head.0 + 1);
    let migrated = (1, vec![(pending.seq, pending.hash.clone())], 0);
    assert_eq!(xproc_genesis_rows(&base.stores[0]), migrated);
    assert_eq!(store_source_state(&base.stores[1]), neighbor_initial);
    assert_eq!(xproc_sha256(&backup_path), backup_digest);
    assert_eq!(xproc_backup_mains(&backup_parent), vec!["source.sqlite"]);
    let mut records = vec![XPROC_BACKUP_RECORD, XPROC_INTENT_RECORD];
    if phase == XPROC_AFTER_CONFIRMATION {
        records.insert(1, XPROC_CONFIRMED_RECORD);
    }
    assert_eq!(xproc_journal_names(&journal), records);
    let confirmed_path = journal
        .path()
        .join(XPROC_CONFIRMED_RECORD)
        .join("record-v1.json");
    if let Some(digest) = &child_confirmation {
        assert_eq!(&xproc_sha256(&confirmed_path), digest);
    }

    // Fresh view: the dead child's flocks are gone. A new migration and a
    // reinstall from the retained backup are refused before any effect.
    let (_, guard) = acquire_xproc_maintenance(&base.root);
    let selected = guard
        .target_source(plan.target.sources[0].source_id)
        .unwrap();
    let neighbor = guard.target_source(neighbor_id).unwrap();
    assert_eq!(
        Store::observe_unchanged_review_format14(&neighbor).unwrap(),
        expected_head
    );
    let limits = EgressStoreLimitsV1::default();
    let reopened = base.open_maintenance(0, &selected);
    let retained = reopened
        .with_review_maintenance_writer(&selected, |writer| {
            let late_journal = base
                .root
                .persistent_child("egress-intent-xproc-late", true)?
                .unwrap();
            let late_backup = base
                .root
                .persistent_child("egress-backup-xproc-late", true)?
                .unwrap();
            let late = RetainedEgressIntent::prepare(
                &selected,
                writer,
                late_journal.clone(),
                late_backup,
                &limits,
            )
            .map(|_| ())
            .unwrap_err();
            assert_eq!(
                late.downcast_ref::<EgressStoreFailure>(),
                Some(&EgressStoreFailure::Integrity)
            );
            assert!(late_journal.persistent_entries(1)?.is_empty());
            let retained = RetainedEgressIntent::resume(
                &selected,
                writer,
                journal.clone(),
                backup_parent.clone(),
                &limits,
            )?;
            let backup = retained.resume_backup(writer)?;
            assert!(backup.install_format15(writer).is_err());
            drop(backup);
            Ok(retained)
        })
        .unwrap();
    drop(reopened);
    assert_eq!(maintenance_source_state(&base), committed);

    // The confirmation attaches to the existing commit, idempotently.
    for _ in 0..2 {
        assert_eq!(
            retained
                .confirm_committed(Redactor::new(vec![]), 10_000)
                .unwrap(),
            pending
        );
    }
    assert_eq!(
        xproc_journal_names(&journal),
        vec![
            XPROC_BACKUP_RECORD,
            XPROC_CONFIRMED_RECORD,
            XPROC_INTENT_RECORD
        ]
    );
    let read_record = |slot: &str| {
        journal
            .persistent_child(slot, false)
            .unwrap()
            .unwrap()
            .persistent_read("record-v1.json", 128 * 1024, &[])
            .unwrap()
            .unwrap()
    };
    assert_eq!(
        serde_json::from_slice::<Value>(read_record(XPROC_CONFIRMED_RECORD).bytes()).unwrap(),
        json!({
            "version": 1,
            "intent_digest": read_record(XPROC_INTENT_RECORD).digest(),
            "backup_digest": read_record(XPROC_BACKUP_RECORD).digest(),
            "receipt": {"seq": pending.seq, "hash": pending.hash},
        }),
    );
    if let Some(digest) = &child_confirmation {
        assert_eq!(&xproc_sha256(&confirmed_path), digest);
    }
    let confirm = |limits: &EgressStoreLimitsV1| {
        Store::confirm_egress_format15(
            &selected,
            Redactor::new(vec![]),
            10_000,
            &backup_path,
            limits,
        )
    };
    assert_eq!(confirm(&limits).unwrap(), pending);
    let different_limits = EgressStoreLimitsV1::new(1, 16_384, 16 * 1_048_576).unwrap();
    assert_eq!(
        confirm(&different_limits)
            .unwrap_err()
            .downcast_ref::<EgressStoreFailure>(),
        Some(&EgressStoreFailure::Conflict)
    );

    // No second migration, the backup is kept, the Store14 neighbor unchanged.
    assert_eq!(maintenance_source_state(&base), committed);
    assert_eq!(xproc_genesis_rows(&base.stores[0]), migrated);
    assert_eq!(store_source_state(&base.stores[1]), neighbor_initial);
    assert_eq!(
        Store::observe_unchanged_review_format14(&neighbor).unwrap(),
        expected_head
    );
    assert_eq!(xproc_sha256(&backup_path), backup_digest);
    assert_eq!(xproc_backup_mains(&backup_parent), vec!["source.sqlite"]);
    eprintln!(
        "egress15 cross-process {phase}: child pid {} ended by SIGKILL; receipt seq {} recovered \
         unchanged; Store14 neighbor head seq {} unchanged",
        child.id(),
        pending.seq,
        expected_head.seq
    );
}

pub(super) fn assert_stopped_consumer_maintenance_refusal_is_typed_and_effect_free() {
    use crate::human_authority::HumanAuthorityFailure;
    let base = BaseFixture::new(&["A"]);
    let anchors = base.migrate_all();
    let prepared = prepare_maintenance15(&base).unwrap();
    let owner = base.owner(&anchors, "active14-exclusion.json", 14);
    let before = maintenance_source_state(&base);
    assert_eq!(before.format, 14);
    let error = prepared.acquire(&base).unwrap_err();
    assert_eq!(error, HumanAuthorityFailure::OwnerContended);
    assert_eq!(maintenance_source_state(&base), before);
    assert!(!base.root.path().join("egress-backups").exists());
    assert!(
        !base
            .root
            .path()
            .join("egress-backups/egress-0.sqlite")
            .exists()
    );
    // An unrelated real acquisition failure is distinguishable while that
    // same owner remains retained; it cannot satisfy the contention oracle.
    let mut wrong_configuration = prepared.plan.target.configuration.clone();
    wrong_configuration.control_config_revision += 1;
    assert_eq!(
        prepared
            .acquire_with_configuration(&base, wrong_configuration)
            .unwrap_err(),
        HumanAuthorityFailure::ConfigurationMismatch
    );
    assert_eq!(maintenance_source_state(&base), before);
    {
        let epoch = owner.try_read_epoch().unwrap();
        epoch.check().unwrap();
    }
    drop(owner);
    // Same retained plan and source, only the prior owner has been released.
    let guard = prepared.acquire(&base).unwrap();
    complete_migration15(&base, &EgressStoreLimitsV1::default(), &prepared, &guard);
    let token = guard
        .target_source(prepared.plan.target.sources[0].source_id)
        .unwrap();
    let reopened = base.open_maintenance(0, &token);
    drop(reopened);
    let after = maintenance_source_state(&base);
    assert_eq!(after.format, 15);
    assert_eq!(after.head.0, before.head.0 + 1);
    assert!(
        base.root
            .path()
            .join("egress-backups/egress-0.sqlite")
            .is_file()
    );
    let db = base.stores[0].db.lock().unwrap();
    validate_schema(&db, false).unwrap();
    control::validate(&db).unwrap();
    assert!(Store::check_integrity(&db).unwrap().valid);
    assert_eq!(
        u64::try_from(
            db.query_row(
                "SELECT count(*) FROM events WHERE kind='control.egress_policy.genesis'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap()
        )
        .context("fixture genesis count is negative")
        .unwrap(),
        1
    );
    assert_eq!(
        u64::try_from(
            db.query_row("SELECT count(*) FROM egress_policy_operations", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap()
        )
        .context("fixture operation count is negative")
        .unwrap(),
        0
    );
}

// A0 test-only adapter: reuse the existing real E1 migrations and the W0
// startup; never construct an actor proof or a diagnostic DTO from metadata.
pub(crate) struct OwnedControlFixture {
    _base: BaseFixture,
    bundle: Option<crate::human_authority::startup::HumanOwnedStartupBundle>,
    credentials: PrivateFile,
    tokens: Vec<(String, String, String)>,
}
impl OwnedControlFixture {
    pub(crate) fn take_startup(
        &mut self,
    ) -> (
        crate::human_authority::startup::HumanOwnedStartupBundle,
        PrivateFile,
        Vec<(String, String, String)>,
    ) {
        (
            self.bundle
                .take()
                .expect("fixture startup may be consumed once"),
            self.credentials.clone(),
            self.tokens.clone(),
        )
    }
}
impl Store {
    pub(crate) fn owned_control_fixture_for_test(format: u32) -> OwnedControlFixture {
        Self::owned_control_fixture_with_legacy_setup_for_test(format, |_| {})
    }

    pub(crate) fn owned_control_fixture_with_legacy_setup_for_test(
        format: u32,
        setup: impl FnOnce(&Store),
    ) -> OwnedControlFixture {
        use crate::human_authority::startup::{
            HumanOwnedSourceOptions, HumanOwnedStartupBundle, HumanOwnedStartupRequest,
        };
        assert!(matches!(format, 14 | 15));
        let base = BaseFixture::new(&["A", "B"]);
        // Test-only causal setup on the actual live legacy A Store, before
        // owned migration/startup deliberately reopen an inactive lease.
        setup(&base.stores[0]);
        let anchors = base.migrate_all();
        if format == 15 {
            migrate15(&base, &EgressStoreLimitsV1::default());
        }
        let owner = base.owner(&anchors, "a0-active.json", format);
        let closure = owner.closure().clone();
        // Release the actual temporary owner before the genuine startup opens
        // its retained consumer and CAS incarnation. No lock is fabricated.
        drop(owner);
        let credentials = base
            .configuration_files
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
                    format!("synthetic-{tenant}-runner"),
                    format!("synthetic-{tenant}-viewer"),
                )
            })
            .collect::<Vec<_>>();
        let redactor = Redactor::new(
            tokens
                .iter()
                .flat_map(|(_, runner, viewer)| [runner.clone(), viewer.clone()])
                .collect(),
        );
        let configuration = HumanSourcesConfigurationV1::new(
            closure.sources.clone(),
            closure.configuration.clone(),
            base.configuration_files.clone(),
        )
        .unwrap();
        let sources = closure
            .sources
            .iter()
            .map(|source| {
                let capacity = AuditCapacityPolicyV1::resolved(10_000);
                (
                    source.source_id,
                    HumanOwnedSourceOptions::TenantStore {
                        redactor: redactor.clone(),
                        max_events: 10_000,
                        workload: WorkloadAuditPolicyV1::resolved(10_000, &capacity),
                        capacity,
                        retention: crate::relay::RelayRetentionPolicyV1::default(),
                    },
                )
            })
            .collect();
        let manifest = PrivateFile::open(
            &base.root.path().join("configuration/a0-active.json"),
            1024 * 1024,
        )
        .unwrap();
        let bundle = HumanOwnedStartupBundle::open(HumanOwnedStartupRequest {
            manifest,
            configuration,
            sources,
        })
        .unwrap();
        let source = bundle
            .owner
            .source_token(closure.sources[0].source_id)
            .unwrap();
        let store = bundle.stores.get(&source.source_id()).unwrap();
        let mut command = crate::control::review_binding::tests::provision();
        command.binding.tenant_id = "A".into();
        command.policy.tenant_id = "A".into();
        command.binding.credential_id = Uuid::from_u128(100).to_string();
        command.binding.control_namespace_uuid =
            source.descriptor().durable_namespace_id.to_string();
        command.policy.control_namespace_uuid = command.binding.control_namespace_uuid.clone();
        command.binding.closure_id = bundle.owner.closure().closure_id.to_string();
        command.binding.closure_revision = bundle.owner.closure().closure_revision;
        command.binding.closure_digest = bundle.owner.closure().closure_digest.clone();
        command.binding.control_config_public_digest = bundle
            .owner
            .closure()
            .configuration
            .public_config_digest
            .clone();
        command.policy.shares.clear();
        command.policy.owner_permissions =
            vec![crate::control::review_binding::ReviewSessionPermissionV1::Content];
        command.binding.session_binding_policy_digest = command.policy.digest().unwrap();
        let account = crate::identity::HumanAccountV1 {
            version: 1,
            principal: command.policy.owner.clone(),
            display_name: "Synthetic A0 owner".into(),
            revision: 1,
            active: true,
        };
        let membership = crate::identity::MembershipV1 {
            version: 1,
            membership_id: Uuid::new_v4().to_string(),
            principal: account.principal.clone(),
            tenant_id: "A".into(),
            organization_scope: command.policy.organization_scope.clone(),
            revision: 1,
            active: true,
            capabilities: [crate::identity::HumanCapabilityV1::ReadContent]
                .into_iter()
                .collect(),
        };
        for change in [
            crate::identity::IdentityChangeV1::Account {
                expected_revision: 0,
                account,
            },
            crate::identity::IdentityChangeV1::Membership {
                expected_revision: 0,
                membership,
            },
        ] {
            mutate(&bundle.owner, &source, store, change);
        }
        {
            let epoch = bundle.owner.try_write_epoch().unwrap();
            store
                .provision_review_binding(&epoch, &source, &command)
                .unwrap();
        }
        OwnedControlFixture {
            _base: base,
            bundle: Some(bundle),
            credentials,
            tokens,
        }
    }

    // Called only after the real registry lookup, on the real retained Store.
    // It deliberately avoids ControlProtocolV1::validate: diagnostic15 is the
    // behavior under test, not a prerequisite that may fail before its oracle.
    pub(crate) fn assert_owned_control_fixture_binding_for_test(
        &self,
        epoch: &crate::human_authority::HumanSourcesReadEpoch,
        source: &OwnedSourceToken,
        credential: &AuthenticatedReviewRunnerCredential,
        expected_format: u32,
    ) -> (String, bool) {
        let access = HumanSourceAccess::read(epoch, source).unwrap();
        let mut db = self.db.try_lock().unwrap();
        let tx = self
            .begin_checked_scoped(&mut db, &access, StoreOperationClass::Business)
            .unwrap();
        let actual: u32 = tx
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(actual, expected_format);
        let verified = self
            .verify_review_binding_in_transaction(&tx, epoch, source, credential)
            .unwrap();
        assert_eq!(verified.binding().tenant_id, "A");
        assert_eq!(
            verified.binding().credential_id,
            credential.credential_id().to_string()
        );
        let namespace = authority::namespace(&tx).unwrap().to_string();
        assert_eq!(verified.binding().control_namespace_uuid, namespace);
        assert!(verified.binding().active);
        if expected_format == 15 {
            let anchored: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM egress_policy_meta m JOIN events e ON e.seq=m.genesis_seq AND e.hash=m.genesis_hash WHERE m.singleton=1)",
                [], |row| row.get(0),
            ).unwrap();
            assert!(
                anchored,
                "format15 must come from its actual anchored E1 migration"
            );
        }
        let active = tx.lease.active;
        verified.revalidate().unwrap();
        drop(verified);
        self.commit_checked(tx).unwrap();
        epoch.check().unwrap();
        (namespace, active)
    }
}

// Exact retained-connection probes for A0 negative fixtures only.
impl Store {
    #[cfg(test)]
    pub(crate) fn owned_control_set_busy_timeout_for_test(
        &self,
        milliseconds: u64,
    ) -> anyhow::Result<()> {
        let db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("fixture Store mutex poisoned"))?;
        db.busy_timeout(std::time::Duration::from_millis(milliseconds))?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn owned_control_busy_timeout_for_test(&self) -> anyhow::Result<i64> {
        let db = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("fixture Store mutex poisoned"))?;
        Ok(db.pragma_query_value(None, "busy_timeout", |row| row.get(0))?)
    }
}

// Keep the real audit-private migration call inside its test-only module.
impl Store {
    #[cfg(test)]
    pub(crate) fn owned_control_migrate_legacy_fixture_for_test(
        &self,
        backups: &std::path::Path,
    ) -> anyhow::Result<()> {
        for target in 3..=13 {
            self.migrate_control_version(
                &backups.join(format!("history-{target}.sqlite")),
                target - 1,
                target,
            )?;
        }
        Ok(())
    }
}

/// In-place identity markers on adopted owned sources (journaled maintenance).
mod identity_markers_tests;
