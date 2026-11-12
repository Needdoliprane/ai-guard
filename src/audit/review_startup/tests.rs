//! Source-only candidate regressions. Root admits the frozen files to offline QA.
use super::*;
use crate::control_fs::{Directory, PrivateFile};
use crate::human_authority::{
    HumanAuthorityClosureV1, HumanAuthorityConfigurationV1, HumanAuthorityMarkerReceiptV1,
    HumanConfigurationFileKindV1, HumanConfigurationFileV1, HumanSourceDescriptorV1,
    HumanSourcesConfigurationV1, HumanStorageProfileV1, OwnedHumanSources, SourceStorageBindingV1,
    maintenance::{
        HumanMaintenanceResolvedViewV1, HumanMaintenanceSourceV1, HumanMaintenanceViewV1,
        HumanSourcesMaintenanceConfigurationV1, HumanSourcesMaintenanceGuard,
        HumanSourcesMaintenancePlanV1,
    },
};
use std::{ffi::OsStr, os::unix::fs::PermissionsExt};
use uuid::Uuid;

struct Fixture {
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

impl Fixture {
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
        let credentials = config
            .persistent_create("credentials.json", b"synthetic reference")
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
                required_format: 14,
                authority_marker_receipt: HumanAuthorityMarkerReceiptV1 {
                    seq: receipt.seq,
                    hash: receipt.hash.clone(),
                },
            })
            .collect();
        let mut closure = HumanAuthorityClosureV1 {
            version: 1,
            installation_id: self.plan.installation_id,
            closure_id: Uuid::new_v4(),
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
            10_000,
            source,
        )
        .unwrap()
    }
}

// Break: opening14 only through legacy/active paths makes a committed partial
// migration impossible to inspect or resume under its genuine retained backup.
#[test]
fn maintenance_reopens_real_migration_and_resumes_the_same_committed_anchor() {
    let fixture = Fixture::new(&["A"]);
    let guard = fixture.maintenance();
    let token = guard
        .target_source(fixture.plan.target.sources[0].source_id)
        .unwrap();
    let opened = fixture.open_maintenance(0, &token);
    let anchor = opened
        .with_review_maintenance_writer(&token, |writer| {
            let backup = writer.prepare_backup(&token, &fixture.backup_path(0))?;
            let receipt = writer.install_format14(&token, &backup)?;
            writer.commit()?;
            assert!(writer.inspect().is_err()); // A consumed writer cannot mutate/retry.
            Ok(receipt)
        })
        .unwrap();
    drop(opened);
    let reopened = fixture.open_maintenance(0, &token);
    reopened.with_review_maintenance_writer(&token, |writer| {
        let backup = writer.resume_backup(&token, &fixture.backup_path(0))?;
        assert_eq!(writer.install_format14(&token, &backup)?, anchor);
        assert!(matches!(writer.inspect()?, ReviewSourceMigrationState::Converted14 { anchor: ref found } if found == &anchor));
        writer.commit()
    }).unwrap();
    assert!(
        Store::open_existing_for_tenant(
            &fixture.plan.target.sources[0].storage_binding.path(),
            "A",
            Redactor::new(vec![]),
            10_000
        )
        .is_err()
    );
    assert!(
        fixture.stores[0]
            .append(Event::new("fixture.legacy_refused"))
            .is_err()
    );
}

// Break: starting backups/DDL before the last real writer was acquired, or
// treating a foreign token/tenant as the retained source's authority.
#[test]
fn maintenance_nested_writers_refuse_foreign_tokens_and_busy_union_without_effect() {
    let fixture = Fixture::new(&["A", "B"]);
    let guard = fixture.maintenance();
    let a = guard
        .target_source(fixture.plan.target.sources[0].source_id)
        .unwrap();
    let b = guard
        .target_source(fixture.plan.target.sources[1].source_id)
        .unwrap();
    let opened_a = fixture.open_maintenance(0, &a);
    let opened_b = fixture.open_maintenance(1, &b);
    assert!(
        opened_a
            .with_review_maintenance_writer(&b, |_| Ok(()))
            .is_err()
    );
    assert!(
        Store::open_review_maintenance_for_tenant(
            &fixture.plan.target.sources[0].storage_binding.path(),
            "B",
            Redactor::new(vec![]),
            10_000,
            &a
        )
        .is_err()
    );
    let mut blocker =
        Connection::open(&fixture.plan.target.sources[1].storage_binding.path()).unwrap();
    let blocked = blocker
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    assert!(
        opened_a
            .with_review_maintenance_writer(&a, |_| opened_b
                .with_review_maintenance_writer(&b, |_| Ok(())))
            .is_err()
    );
    assert!(!fixture.backup_path(0).exists());
    assert!(!fixture.backup_path(1).exists());
    drop(blocked);
    opened_a
        .with_review_maintenance_writer(&a, |wa| {
            opened_b.with_review_maintenance_writer(&b, |wb| {
                assert!(matches!(
                    wa.inspect()?,
                    ReviewSourceMigrationState::Ordinary13 { .. }
                ));
                assert!(matches!(
                    wb.inspect()?,
                    ReviewSourceMigrationState::Ordinary13 { .. }
                ));
                let ba = wa.prepare_backup(&a, &fixture.backup_path(0))?;
                let bb = wb.prepare_backup(&b, &fixture.backup_path(1))?;
                wa.install_format14(&a, &ba)?;
                wb.install_format14(&b, &bb)?;
                wb.commit()?;
                wa.commit()
            })
        })
        .unwrap();
}

// Break: returning a usable Store on anchor/manifest alone, or retaining a
// previous owner's incarnation instead of committing this actual WriteEpoch.
#[test]
fn startup_requires_current_real_incarnation_before_retention_and_scoped_access() {
    let fixture = Fixture::new(&["A"]);
    let receipts = fixture.migrate_all();
    let owner = fixture.owner(&receipts, "active.json");
    let source = owner
        .source_token(fixture.plan.target.sources[0].source_id)
        .unwrap();
    let startup = fixture.startup(0, &source);
    let epoch = owner.try_write_epoch().unwrap();
    assert_eq!(startup.current_incarnation_revision(&epoch).unwrap(), 0);
    assert!(startup.retain(&epoch, &source).is_err());
    // A refusal is not a rollback/repair of the genuinely migrated source.
    let startup = fixture.startup(0, &source);
    let before = startup.durable_wall_floor().unwrap();
    let receipt = startup
        .append_incarnation(&epoch, &source, Uuid::new_v4(), 0)
        .unwrap();
    assert_eq!(startup.current_incarnation_revision(&epoch).unwrap(), 1);
    assert!(receipt.seq > receipts[0].seq);
    assert!(startup.durable_wall_floor().unwrap() >= before);
    let store = startup.retain(&epoch, &source).unwrap();
    assert!(
        store
            .append(Event::new("fixture.legacy_on_retained"))
            .is_err()
    );
    let access = HumanSourceAccess::write(&epoch, &source).unwrap();
    let mut db = store.db.lock().unwrap();
    let tx = store
        .begin_checked_scoped(&mut db, &access, StoreOperationClass::IdentityAuthority)
        .unwrap();
    tx.require_identity_authority().unwrap();
    drop(tx);
    drop(db);
    drop(store);
    drop(epoch);
    drop(source);
    drop(owner);
    let next_owner = fixture.owner(&receipts, "next-active.json");
    let next_source = next_owner
        .source_token(fixture.plan.target.sources[0].source_id)
        .unwrap();
    let next_epoch = next_owner.try_write_epoch().unwrap();
    assert!(
        fixture
            .startup(0, &next_source)
            .retain(&next_epoch, &next_source)
            .is_err()
    );
    let startup = fixture.startup(0, &next_source);
    startup
        .append_incarnation(&next_epoch, &next_source, Uuid::new_v4(), 1)
        .unwrap();
    startup.retain(&next_epoch, &next_source).unwrap();
}

// Break: a caller-selected neighbouring source becomes the active authority,
// or its refusal permanently invalidates the correctly retained source.
#[test]
fn startup_refuses_foreign_source_and_tenant_without_poisoning_exact_owner() {
    let fixture = Fixture::new(&["A", "B"]);
    let receipts = fixture.migrate_all();
    let owner = fixture.owner(&receipts, "active.json");
    let a = owner
        .source_token(fixture.plan.target.sources[0].source_id)
        .unwrap();
    let b = owner
        .source_token(fixture.plan.target.sources[1].source_id)
        .unwrap();
    let path = fixture.plan.target.sources[0].storage_binding.path();
    assert!(
        Store::open_review_startup_for_tenant(&path, "A", Redactor::new(vec![]), 10_000, &b)
            .is_err()
    );
    assert!(
        Store::open_review_startup_for_tenant(&path, "B", Redactor::new(vec![]), 10_000, &a)
            .is_err()
    );
    let startup = fixture.startup(0, &a);
    let epoch = owner.try_write_epoch().unwrap();
    assert!(
        startup
            .append_incarnation(&epoch, &b, Uuid::new_v4(), 0)
            .is_err()
    );
    startup
        .append_incarnation(&epoch, &a, Uuid::new_v4(), 0)
        .unwrap();
    startup.retain(&epoch, &a).unwrap();
}

// Break: a late epoch expiry erases the known COMMIT floor or returns a fresh
// ACK. Closing admissions is deliberately not used: accepted epochs may drain.
#[test]
fn startup_known_commit_floor_survives_late_epoch_expiry_without_ack() {
    let fixture = Fixture::new(&["A"]);
    let receipts = fixture.migrate_all();
    let owner = fixture.owner(&receipts, "active.json");
    let source = owner
        .source_token(fixture.plan.target.sources[0].source_id)
        .unwrap();
    let startup = fixture.startup(0, &source);
    let epoch = owner.try_write_epoch().unwrap();
    let reached_known_commit = std::cell::Cell::new(false);
    let result = startup.append_incarnation_inner(&epoch, &source, Uuid::new_v4(), 0, || {
        reached_known_commit.set(true);
        // The authority contract bounds an accepted epoch to two seconds.
        // Wait only after the real commit and its retained floor are known.
        std::thread::sleep(std::time::Duration::from_millis(2_100));
        assert!(
            epoch.check().is_err(),
            "the admitted epoch must have expired"
        );
    });
    assert!(
        reached_known_commit.get(),
        "the real commit must be reached"
    );
    assert!(result.is_err());
    let head = audit_head(&startup.store.db.lock().unwrap()).unwrap();
    assert!(head.0 > receipts[0].seq);
    assert_eq!(
        startup.store.supervision_lease.state.lock().unwrap().floor,
        head
    );
    assert!(startup.store.review_authority.is_none());
    assert!(startup.retain(&epoch, &source).is_err());
    drop(epoch);
    // Expiry refuses an acknowledgement but does not invalidate a healthy
    // owner or erase its committed incarnation. A fresh epoch can retain it.
    let retry_epoch = owner.try_write_epoch().unwrap();
    fixture
        .startup(0, &source)
        .retain(&retry_epoch, &source)
        .unwrap();
}

// F1 break: a second startup wrapper returns a getter value from a newer real
// committed head but keeps only the older head witnessed at its own opening.
#[test]
fn startup_getters_retain_the_real_head_observed_after_another_wrapper_commits() {
    // Revision is intentionally first: a baseline failure stops here and proves
    // only that reached branch. A future GREEN must finish both fresh fixtures.
    for revision_getter in [true, false] {
        let fixture = Fixture::new(&["A"]);
        let anchors = fixture.migrate_all();
        let owner = fixture.owner(&anchors, "active.json");
        let source = owner
            .source_token(fixture.plan.target.sources[0].source_id)
            .unwrap();
        let writer = fixture.startup(0, &source);
        let reader = fixture.startup(0, &source);
        let h0 = writer
            .store
            .supervision_lease
            .state
            .lock()
            .unwrap()
            .floor
            .clone();
        assert_eq!(h0, (anchors[0].seq, anchors[0].hash.clone()));
        assert_eq!(
            reader.store.supervision_lease.state.lock().unwrap().floor,
            h0
        );
        let epoch = owner.try_write_epoch().unwrap();
        let receipt = writer
            .append_incarnation(&epoch, &source, Uuid::new_v4(), 0)
            .unwrap();
        assert!(receipt.seq > h0.0);
        // Expectations come from writer1's actual committed receipt/replay,
        // never from an injected cache, authority field or fabricated format14.
        let (expected_revision, expected_wall_floor) = {
            let db = writer.store.db.lock().unwrap();
            let snapshot = authority::replay(&db).unwrap();
            assert_eq!(snapshot.incarnation_receipt(), Some(&receipt));
            let wall_floor: String = db
                .query_row(
                    "SELECT f.max_received_at FROM audit_time_floors f JOIN events e ON e.seq=f.event_seq WHERE e.seq=?1 AND e.hash=?2",
                    rusqlite::params![receipt.seq, receipt.hash],
                    |row| row.get(0),
                )
                .unwrap();
            (
                snapshot.incarnation_revision(),
                chrono::DateTime::parse_from_rfc3339(&wall_floor)
                    .unwrap()
                    .with_timezone(&chrono::Utc),
            )
        };
        let getter = if revision_getter {
            assert_eq!(
                reader.current_incarnation_revision(&epoch).unwrap(),
                expected_revision
            );
            "current_incarnation_revision"
        } else {
            assert_eq!(reader.durable_wall_floor().unwrap(), expected_wall_floor);
            "durable_wall_floor"
        };
        let witness = reader
            .store
            .supervision_lease
            .state
            .lock()
            .unwrap()
            .floor
            .clone();
        assert!(
            witness.0 >= receipt.seq && (witness.0 != receipt.seq || witness.1 == receipt.hash),
            "F1: startup getter {getter} returned the real H1 value but retained witness seq {} below committed receipt seq {} or with a divergent hash",
            witness.0,
            receipt.seq,
        );
    }
}
