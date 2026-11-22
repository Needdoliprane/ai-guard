//! Real private-FS/lock tests, written but NOT_RUN in this source-only task.
use super::*;
use crate::human_authority::{HumanConfigurationFileV1, OWNER_LOCK_NAME, OWNER_LOCK_PROTOCOL};
use std::os::unix::fs::PermissionsExt;

struct ViewFixture {
    view: HumanMaintenanceViewV1,
    files: Vec<(HumanConfigurationFileKindV1, PrivateFile)>,
}

struct Fixture {
    _temporary: tempfile::TempDir,
    root: Directory,
    prior: Option<ViewFixture>,
    target: ViewFixture,
    plan: HumanSourcesMaintenancePlanV1,
    plan_file: PrivateFile,
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
        file_name: name.to_owned(),
        parent_device,
        parent_inode,
        source_device,
        source_inode,
    }
}

fn make_source(
    root: &Directory,
    name: &str,
    source_id: Uuid,
    installation: Uuid,
) -> HumanMaintenanceSourceV1 {
    let parent = root.persistent_child(name, true).unwrap().unwrap();
    parent
        .persistent_create(
            "source.sqlite",
            b"FS fixture only; no SQLite format or marker proof",
        )
        .unwrap();
    HumanMaintenanceSourceV1 {
        source_id,
        kind: HumanSourceKindV1::TenantStore,
        installation_id: installation,
        durable_namespace_id: Uuid::from_u128(source_id.as_u128() + 1024),
        tenant_id: Some(format!("tenant-{}", source_id.as_u128())),
        storage_binding: binding(&parent, "source.sqlite"),
        required_format: 14,
    }
}

fn make_view(
    root: &Directory,
    name: &str,
    sources: Vec<HumanMaintenanceSourceV1>,
    revision: u64,
) -> ViewFixture {
    let parent = root.persistent_child(name, true).unwrap().unwrap();
    let config = parent
        .persistent_create("control.json", b"descriptive private configuration fixture")
        .unwrap();
    let credentials = parent
        .persistent_create("credentials.json", b"secret-canary-do-not-log")
        .unwrap();
    ViewFixture {
        view: HumanMaintenanceViewV1 {
            sources,
            configuration: HumanAuthorityConfigurationV1 {
                control_config_revision: revision,
                public_config_digest: "ac".repeat(32),
                oidc: None,
                files: vec![
                    HumanConfigurationFileV1 {
                        kind: HumanConfigurationFileKindV1::ControlConfiguration,
                        storage_binding: binding(&parent, "control.json"),
                    },
                    HumanConfigurationFileV1 {
                        kind: HumanConfigurationFileKindV1::TechnicalCredentials,
                        storage_binding: binding(&parent, "credentials.json"),
                    },
                ],
            },
        },
        files: vec![
            (HumanConfigurationFileKindV1::ControlConfiguration, config),
            (
                HumanConfigurationFileKindV1::TechnicalCredentials,
                credentials,
            ),
        ],
    }
}

impl Fixture {
    fn new(target_count: usize, moved_prior: bool) -> Self {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        let root = Directory::open_existing(temporary.path()).unwrap();
        let installation = Uuid::new_v4();
        let targets: Vec<_> = (1..=target_count)
            .map(|id| {
                make_source(
                    &root,
                    &format!("target-{id}"),
                    Uuid::from_u128(id as u128),
                    installation,
                )
            })
            .collect();
        let prior = if moved_prior {
            let mut source = make_source(&root, "prior-1", Uuid::from_u128(1), installation);
            source.required_format = 13;
            Some(make_view(&root, "prior-config", vec![source], 1))
        } else {
            None
        };
        let target = make_view(&root, "target-config", targets, 2);
        let plan = HumanSourcesMaintenancePlanV1 {
            version: 1,
            migration_id: Uuid::new_v4(),
            installation_id: installation,
            prior: prior.as_ref().map(|v| v.view.clone()),
            target: target.view.clone(),
            prior_active_manifest: None,
            supported_storage_profile: HumanStorageProfileV1::LocalPosixFlockV1,
            selected_source_id: None,
        };
        let plan_file = root
            .persistent_create("maintenance.json", &serde_json::to_vec(&plan).unwrap())
            .unwrap();
        Self {
            _temporary: temporary,
            root,
            prior,
            target,
            plan,
            plan_file,
        }
    }

    fn actual(&self) -> HumanSourcesMaintenanceConfigurationV1 {
        let prior = self.prior.as_ref().map(|v| {
            HumanMaintenanceResolvedViewV1::new(
                v.view.sources.clone(),
                v.view.configuration.clone(),
                v.files.clone(),
            )
            .unwrap()
        });
        let target = HumanMaintenanceResolvedViewV1::new(
            self.target.view.sources.clone(),
            self.target.view.configuration.clone(),
            self.target.files.clone(),
        )
        .unwrap();
        HumanSourcesMaintenanceConfigurationV1::new(prior, target, None)
    }

    fn acquire(&self) -> Result<HumanSourcesMaintenanceGuard, HumanAuthorityFailure> {
        HumanSourcesMaintenanceGuard::acquire(self.plan_file.clone(), self.actual())
    }

    fn rewrite_plan(&mut self) {
        self.plan_file = self
            .root
            .persistent_create(
                &format!("plan-{}.json", Uuid::new_v4()),
                &serde_json::to_vec(&self.plan).unwrap(),
            )
            .unwrap();
    }
}

fn probe(source: &HumanMaintenanceSourceV1) -> bool {
    let directory = Directory::open_existing(&source.storage_binding.parent_path).unwrap();
    let file = directory
        .persistent_read(OWNER_LOCK_NAME, 128, &[])
        .unwrap()
        .unwrap();
    file.persistent_try_lock().unwrap()
}

#[test]
fn initial_and_legacy_prior_plans_acquire_without_any_future_receipt() {
    // Initial target and legacy prior both have actual files but no marker
    // receipts. Adding a required fake active closure would break these paths.
    for prior in [false, true] {
        let fixture = Fixture::new(1, prior);
        let guard = fixture.acquire().unwrap();
        let token = guard.target_source(Uuid::from_u128(1)).unwrap();
        token.check().unwrap();
        assert_eq!(token.migration_id(), fixture.plan.migration_id);
        assert_eq!(token.descriptor().required_format, 14);
        assert!(fixture.plan.prior_active_manifest.is_none());
    }
}

#[test]
fn moved_source_token_retains_both_prior_and_target_parent_locks() {
    // Deduplicating just the logical source would omit one physical parent.
    let fixture = Fixture::new(1, true);
    let guard = fixture.acquire().unwrap();
    let token = guard.target_source(Uuid::from_u128(1)).unwrap();
    let clone = token.clone();
    assert!(token.same_guard_and_source(&clone));
    drop(guard);
    assert!(!probe(&fixture.prior.as_ref().unwrap().view.sources[0]));
    assert!(!probe(&fixture.target.view.sources[0]));
    drop(clone);
    drop(token);
    assert!(probe(&fixture.prior.as_ref().unwrap().view.sources[0]));
    assert!(probe(&fixture.target.view.sources[0]));
}

#[test]
fn canonical_parent_order_releases_partial_locks_after_later_contention() {
    // Reverse source-ID order relative to physical order; a wrong acquisition
    // order never creates the earlier lock that this actual probe requires.
    let mut fixture = Fixture::new(2, false);
    let mut ordered = fixture.plan.target.sources.clone();
    ordered.sort_by_key(|s| s.storage_binding.parent_identity());
    for source in &mut fixture.plan.target.sources {
        source.source_id = if source.storage_binding.parent_identity()
            == ordered[1].storage_binding.parent_identity()
        {
            Uuid::from_u128(1)
        } else {
            Uuid::from_u128(2)
        };
    }
    fixture.plan.target.sources.sort_by_key(|s| s.source_id);
    fixture.target.view.sources = fixture.plan.target.sources.clone();
    fixture.rewrite_plan();
    let later = Directory::open_existing(&ordered[1].storage_binding.parent_path).unwrap();
    let held = later
        .persistent_create(OWNER_LOCK_NAME, OWNER_LOCK_PROTOCOL)
        .unwrap();
    assert!(held.persistent_try_lock().unwrap());
    assert!(matches!(
        fixture.acquire(),
        Err(HumanAuthorityFailure::OwnerContended)
    ));
    assert!(probe(&ordered[0]));
    drop(held);
    fixture.acquire().unwrap();
}

#[test]
fn unchanged_source_shared_by_views_is_owned_without_self_contention() {
    let mut fixture = Fixture::new(1, true);
    fixture.plan.prior.as_mut().unwrap().sources[0].storage_binding =
        fixture.plan.target.sources[0].storage_binding.clone();
    fixture.prior.as_mut().unwrap().view.sources =
        fixture.plan.prior.as_ref().unwrap().sources.clone();
    fixture.rewrite_plan();
    let guard = fixture.acquire().unwrap();
    guard.check().unwrap();
    assert!(!probe(&fixture.target.view.sources[0]));
}

#[test]
fn actual_configuration_missing_or_adding_a_source_is_rejected_before_locks() {
    let fixture = Fixture::new(2, false);
    let mut missing = fixture.target.view.sources.clone();
    missing.pop();
    let view = HumanMaintenanceResolvedViewV1::new(
        missing,
        fixture.target.view.configuration.clone(),
        fixture.target.files.clone(),
    )
    .unwrap();
    assert!(matches!(
        HumanSourcesMaintenanceGuard::acquire(
            fixture.plan_file.clone(),
            HumanSourcesMaintenanceConfigurationV1::new(None, view, None)
        ),
        Err(HumanAuthorityFailure::ConfigurationMismatch)
    ));
    let mut extra = fixture.target.view.sources.clone();
    extra.push(make_source(
        &fixture.root,
        "extra",
        Uuid::from_u128(3),
        fixture.plan.installation_id,
    ));
    let view = HumanMaintenanceResolvedViewV1::new(
        extra,
        fixture.target.view.configuration.clone(),
        fixture.target.files.clone(),
    )
    .unwrap();
    assert!(matches!(
        HumanSourcesMaintenanceGuard::acquire(
            fixture.plan_file.clone(),
            HumanSourcesMaintenanceConfigurationV1::new(None, view, None)
        ),
        Err(HumanAuthorityFailure::ConfigurationMismatch)
    ));
    for source in &fixture.plan.target.sources {
        let parent = Directory::open_existing(&source.storage_binding.parent_path).unwrap();
        assert!(
            parent
                .private_entry(OsStr::new(OWNER_LOCK_NAME))
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn plan_and_retained_credentials_substitution_latch_without_releasing_sources() {
    for change_plan in [true, false] {
        let fixture = Fixture::new(1, true);
        let guard = fixture.acquire().unwrap();
        let token = guard.target_source(Uuid::from_u128(1)).unwrap();
        let file = if change_plan {
            &fixture.plan_file
        } else {
            &fixture.prior.as_ref().unwrap().files[1].1
        };
        std::fs::write(file.path(), b"changed private bytes").unwrap();
        assert_eq!(token.check(), Err(HumanAuthorityFailure::OwnerInvalid));
        assert_eq!(guard.check(), Err(HumanAuthorityFailure::OwnerInvalid));
        assert!(!probe(&fixture.target.view.sources[0]));
    }
}

#[test]
fn main_substitution_cannot_become_a_new_expected_binding() {
    // The original descriptor is retained; neither a fresh main inode nor an
    // alias is adopted during later checks.
    let fixture = Fixture::new(1, false);
    let guard = fixture.acquire().unwrap();
    let path = fixture.target.view.sources[0].storage_binding.path();
    std::fs::rename(&path, path.with_extension("retained")).unwrap();
    let parent = Directory::open_existing(path.parent().unwrap()).unwrap();
    parent
        .persistent_create("source.sqlite", b"different inode")
        .unwrap();
    assert_eq!(guard.check(), Err(HumanAuthorityFailure::OwnerInvalid));
    assert!(!probe(&fixture.target.view.sources[0]));
}

#[test]
fn structured_shortage_keeps_ownership_and_next_check_validates_the_whole_union() {
    // Constructed IO causes exercise the actual result handler. No FD limit is
    // changed and no permanent substitute filesystem is supplied.
    let fixture = Fixture::new(1, true);
    let guard = fixture.acquire().unwrap();
    let token = guard.target_source(Uuid::from_u128(1)).unwrap();
    for errno in [libc::EMFILE, libc::ENFILE] {
        let failure = OwnerFilesystemFailure::unavailable(
            anyhow::Error::new(std::io::Error::from_raw_os_error(errno))
                .context("private maintenance observation"),
        );
        assert_eq!(
            guard.owner.complete_revalidation(Err(failure)),
            Err(HumanAuthorityFailure::StorageUnavailable)
        );
        assert!(!probe(&fixture.prior.as_ref().unwrap().view.sources[0]));
        token.check().unwrap();
    }
    let failure = OwnerFilesystemFailure::unavailable(
        anyhow::Error::new(std::io::Error::from_raw_os_error(libc::ENFILE))
            .context("before a different prior source changes"),
    );
    assert_eq!(
        guard.owner.complete_revalidation(Err(failure)),
        Err(HumanAuthorityFailure::StorageUnavailable)
    );
    let path = fixture.prior.as_ref().unwrap().view.sources[0]
        .storage_binding
        .path();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(token.check(), Err(HumanAuthorityFailure::OwnerInvalid));
    assert!(!probe(&fixture.target.view.sources[0]));
}

#[test]
fn unknown_or_non_descriptor_failure_permanently_invalidates_the_union() {
    let fixture = Fixture::new(1, true);
    let guard = fixture.acquire().unwrap();
    let failure = OwnerFilesystemFailure::unavailable(
        anyhow::Error::new(std::io::Error::from_raw_os_error(libc::EIO))
            .context("unavailable maintenance observation"),
    );
    assert_eq!(
        guard.owner.complete_revalidation(Err(failure)),
        Err(HumanAuthorityFailure::OwnerInvalid)
    );
    assert_eq!(guard.check(), Err(HumanAuthorityFailure::OwnerInvalid));
    assert!(!probe(&fixture.prior.as_ref().unwrap().view.sources[0]));
}

#[test]
fn plan_is_closed_bounded_and_rejects_physical_or_logical_aliases() {
    let fixture = Fixture::new(2, false);
    let mut value = serde_json::to_value(&fixture.plan).unwrap();
    value["ready"] = serde_json::json!(true);
    assert!(
        HumanSourcesMaintenancePlanV1::from_bytes(&serde_json::to_vec(&value).unwrap()).is_err()
    );
    let mut plan = fixture.plan.clone();
    plan.target.sources[1].storage_binding = plan.target.sources[0].storage_binding.clone();
    assert!(
        HumanSourcesMaintenancePlanV1::from_bytes(&serde_json::to_vec(&plan).unwrap()).is_err()
    );
    assert!(
        HumanSourcesMaintenancePlanV1::from_bytes(&vec![b' '; MAX_MAINTENANCE_PLAN_BYTES + 1])
            .is_err()
    );
    let mut plan = fixture.plan.clone();
    let inode_floor = plan
        .target
        .configuration
        .files
        .iter()
        .map(|file| file.storage_binding.source_inode)
        .max()
        .unwrap()
        .checked_add(4096)
        .unwrap();
    plan.target.sources = (1..=257)
        .map(|index| {
            let mut source = fixture.plan.target.sources[0].clone();
            source.source_id = Uuid::from_u128(index);
            source.durable_namespace_id = Uuid::from_u128(index + 1024);
            source.tenant_id = Some(format!("unique-{index}"));
            source.storage_binding.file_name = format!("unique-{index}.sqlite");
            source.storage_binding.source_inode = inode_floor + index as u64;
            source
        })
        .collect();
    // Unique expected descriptors ensure this tests the count, not duplication.
    assert!(
        HumanSourcesMaintenancePlanV1::from_bytes(&serde_json::to_vec(&plan).unwrap()).is_err()
    );
    let mut namespace_alias = fixture.plan.clone();
    namespace_alias.target.sources[1].durable_namespace_id =
        namespace_alias.target.sources[0].durable_namespace_id;
    assert!(
        HumanSourcesMaintenancePlanV1::from_bytes(&serde_json::to_vec(&namespace_alias).unwrap())
            .is_err()
    );
    let mut tenant_alias = fixture.plan.clone();
    tenant_alias.target.sources[1].tenant_id = tenant_alias.target.sources[0].tenant_id.clone();
    assert!(
        HumanSourcesMaintenancePlanV1::from_bytes(&serde_json::to_vec(&tenant_alias).unwrap())
            .is_err()
    );
    let mut unconfigured_ledger = fixture.plan.clone();
    unconfigured_ledger.target.sources[1].kind = HumanSourceKindV1::AuthenticationLedger;
    unconfigured_ledger.target.sources[1].tenant_id = None;
    assert!(
        HumanSourcesMaintenancePlanV1::from_bytes(
            &serde_json::to_vec(&unconfigured_ledger).unwrap()
        )
        .is_err()
    );
}

#[test]
fn previous_active_manifest_is_not_fabricated_from_legacy_scope_or_arbitrary_bytes() {
    let mut fixture = Fixture::new(1, true);
    let file = fixture
        .root
        .persistent_create("old-active.json", b"not an active closure manifest")
        .unwrap();
    fixture.plan.prior_active_manifest = Some(HumanMaintenanceActiveManifestV1 {
        storage_binding: binding(&fixture.root, "old-active.json"),
        closure_id: Uuid::new_v4(),
        closure_revision: 1,
        closure_digest: "bd".repeat(32),
    });
    fixture.rewrite_plan();
    assert!(matches!(
        fixture.acquire(),
        Err(HumanAuthorityFailure::ConfigurationMismatch)
    ));
    let mut actual = fixture.actual();
    actual.prior_active_manifest = Some(file);
    assert!(matches!(
        HumanSourcesMaintenanceGuard::acquire(fixture.plan_file.clone(), actual),
        Err(HumanAuthorityFailure::InvalidManifest)
    ));
}

#[test]
fn unknown_target_and_foreign_guard_tokens_do_not_create_authority_or_expose_data() {
    let fixture = Fixture::new(1, false);
    let foreign_fixture = Fixture::new(1, false);
    let guard = fixture.acquire().unwrap();
    let foreign = foreign_fixture.acquire().unwrap();
    let token = guard.target_source(Uuid::from_u128(1)).unwrap();
    let foreign_token = foreign.target_source(Uuid::from_u128(1)).unwrap();
    assert!(!token.same_guard_and_source(&foreign_token));
    assert!(matches!(
        guard.target_source(Uuid::new_v4()),
        Err(HumanAuthorityFailure::SourceMismatch)
    ));
    token.check().unwrap();
    let text = format!(
        "{guard:?} {token:?} {}",
        HumanAuthorityFailure::ConfigurationMismatch
    );
    assert!(!text.contains("secret-canary"));
    assert!(!text.contains(fixture.root.path().to_str().unwrap()));
}
