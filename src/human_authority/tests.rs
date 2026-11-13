//! SOURCE-ONLY: these tests are not executed until Root admits frozen QA.

use super::*;
use std::{os::unix::fs::PermissionsExt, sync::mpsc};

struct Fixture {
    _temporary: tempfile::TempDir,
    root: Directory,
    sources: Vec<HumanSourceDescriptorV1>,
    configuration: HumanAuthorityConfigurationV1,
    manifest: PrivateFile,
    files: Vec<(HumanConfigurationFileKindV1, PrivateFile)>,
}

fn binding(parent: &Directory, name: &str) -> SourceStorageBindingV1 {
    let (parent_device, parent_inode) = parent.physical_identity().unwrap();
    let (source_device, source_inode) = parent
        .private_entry(std::ffi::OsStr::new(name))
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

impl Fixture {
    fn new(count: usize) -> Self {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        let root = Directory::open_existing(temporary.path()).unwrap();
        let installation_id = Uuid::new_v4();
        let mut sources = Vec::new();
        for index in 0..count {
            let parent = root
                .persistent_child(&format!("source-{index}"), true)
                .unwrap()
                .unwrap();
            // An OS owner is deliberately not a SQLite format/marker proof.
            parent
                .persistent_create("tenant.sqlite", b"filesystem fixture only")
                .unwrap();
            sources.push(HumanSourceDescriptorV1 {
                source_id: Uuid::from_u128(index as u128 + 1),
                kind: HumanSourceKindV1::TenantStore,
                installation_id,
                durable_namespace_id: Uuid::new_v4(),
                tenant_id: Some(format!("tenant-{index}")),
                storage_binding: binding(&parent, "tenant.sqlite"),
                required_format: 14,
                authority_marker_receipt: HumanAuthorityMarkerReceiptV1 {
                    seq: 1,
                    hash: "ab".repeat(32),
                },
            });
        }
        let config_root = root
            .persistent_child("configuration", true)
            .unwrap()
            .unwrap();
        let control = config_root
            .persistent_create("control.json", b"private config fixture")
            .unwrap();
        let credentials = config_root
            .persistent_create("credentials.json", b"secret-canary-do-not-log")
            .unwrap();
        let configuration = HumanAuthorityConfigurationV1 {
            control_config_revision: 1,
            public_config_digest: "cd".repeat(32),
            oidc: None,
            files: vec![
                HumanConfigurationFileV1 {
                    kind: HumanConfigurationFileKindV1::ControlConfiguration,
                    storage_binding: binding(&config_root, "control.json"),
                },
                HumanConfigurationFileV1 {
                    kind: HumanConfigurationFileKindV1::TechnicalCredentials,
                    storage_binding: binding(&config_root, "credentials.json"),
                },
            ],
        };
        let mut closure = HumanAuthorityClosureV1 {
            version: 1,
            installation_id,
            closure_id: Uuid::new_v4(),
            closure_revision: 1,
            closure_digest: String::new(),
            sources: sources.clone(),
            configuration: configuration.clone(),
            supported_storage_profile: HumanStorageProfileV1::LocalPosixFlockV1,
        };
        closure.closure_digest = closure.compute_digest().unwrap();
        let manifest = config_root
            .persistent_create("authority.json", &serde_json::to_vec(&closure).unwrap())
            .unwrap();
        Self {
            _temporary: temporary,
            root,
            sources,
            configuration,
            manifest,
            files: vec![
                (HumanConfigurationFileKindV1::ControlConfiguration, control),
                (
                    HumanConfigurationFileKindV1::TechnicalCredentials,
                    credentials,
                ),
            ],
        }
    }

    fn acquire(&self) -> Result<OwnedHumanSources, HumanAuthorityFailure> {
        OwnedHumanSources::acquire(
            self.manifest.clone(),
            HumanSourcesConfigurationV1::new(
                self.sources.clone(),
                self.configuration.clone(),
                self.files.clone(),
            )?,
        )
    }

    fn use_new_manifest(&mut self, mut closure: HumanAuthorityClosureV1) {
        closure.closure_digest = closure.compute_digest().unwrap();
        let parent = Directory::open_existing(self.manifest.path().parent().unwrap()).unwrap();
        self.sources = closure.sources.clone();
        self.manifest = parent
            .persistent_create(
                "replacement-manifest.json",
                &serde_json::to_vec(&closure).unwrap(),
            )
            .unwrap();
    }
}

#[test]
fn independent_owners_exclude_and_tokens_retain_actual_flocks() {
    // Removing persistent_try_lock or detaching token lifetime breaks this.
    let fixture = Fixture::new(1);
    let owner = fixture.acquire().unwrap();
    let old_incarnation = owner.incarnation();
    assert!(matches!(
        fixture.acquire(),
        Err(HumanAuthorityFailure::OwnerContended)
    ));
    let token = owner.source_token(fixture.sources[0].source_id).unwrap();
    drop(owner);
    assert!(matches!(
        fixture.acquire(),
        Err(HumanAuthorityFailure::OwnerContended)
    ));
    drop(token);
    let reopened = fixture.acquire().unwrap();
    assert_ne!(reopened.incarnation(), Uuid::nil());
    assert_ne!(reopened.incarnation(), old_incarnation);
}

#[test]
fn canonical_acquisition_releases_earlier_parents_on_contention() {
    // A failed later parent must not strand a partial owner's earlier lock.
    let mut fixture = Fixture::new(2);
    let mut ordered = fixture.sources.clone();
    ordered.sort_by_key(|source| {
        (
            source.storage_binding.parent_device,
            source.storage_binding.parent_inode,
        )
    });
    // Put the physically later parent first in manifest source-ID order.
    // A source-order implementation then fails before creating the earlier
    // lock, and cannot satisfy the real earlier-lock probe below.
    let mut closure = HumanAuthorityClosureV1::from_bytes(fixture.manifest.bytes()).unwrap();
    for source in &mut closure.sources {
        source.source_id = if source.storage_binding.parent_identity()
            == ordered[1].storage_binding.parent_identity()
        {
            Uuid::from_u128(1)
        } else {
            Uuid::from_u128(2)
        };
    }
    closure.sources.sort_by_key(|source| source.source_id);
    fixture.use_new_manifest(closure);
    let later = Directory::open_existing(&ordered[1].storage_binding.parent_path).unwrap();
    let held = later
        .persistent_create(OWNER_LOCK_NAME, OWNER_LOCK_PROTOCOL)
        .unwrap();
    assert!(held.persistent_try_lock().unwrap());
    assert!(matches!(
        fixture.acquire(),
        Err(HumanAuthorityFailure::OwnerContended)
    ));
    let earlier = Directory::open_existing(&ordered[0].storage_binding.parent_path).unwrap();
    let probe = earlier
        .persistent_read(OWNER_LOCK_NAME, 128, &[])
        .unwrap()
        .unwrap();
    assert!(probe.persistent_try_lock().unwrap());
    drop(probe);
    drop(held);
    fixture.acquire().unwrap();
}

#[test]
fn two_sources_in_the_same_physical_parent_share_its_fixed_lock() {
    // Taking a separate independent flock for every source would self-contend.
    let mut fixture = Fixture::new(2);
    let parent = Directory::open_existing(&fixture.sources[0].storage_binding.parent_path).unwrap();
    parent
        .persistent_create("second.sqlite", b"second filesystem fixture")
        .unwrap();
    let mut closure = HumanAuthorityClosureV1::from_bytes(fixture.manifest.bytes()).unwrap();
    closure.sources[1].storage_binding = binding(&parent, "second.sqlite");
    fixture.use_new_manifest(closure);
    let owner = fixture.acquire().unwrap();
    let first = owner.source_token(fixture.sources[0].source_id).unwrap();
    let second = owner.source_token(fixture.sources[1].source_id).unwrap();
    let epoch = owner.try_read_epoch().unwrap();
    epoch.validate_source(&first).unwrap();
    epoch.validate_source(&second).unwrap();
    assert!(matches!(
        fixture.acquire(),
        Err(HumanAuthorityFailure::OwnerContended)
    ));
}

#[test]
fn lock_protocol_and_changed_source_modes_fail_closed_without_repair() {
    // Accepting arbitrary lock bytes, or failing to revalidate source modes,
    // creates an apparently valid owner over a changed storage binding.
    let fixture = Fixture::new(1);
    let parent = Directory::open_existing(&fixture.sources[0].storage_binding.parent_path).unwrap();
    parent
        .persistent_create(OWNER_LOCK_NAME, b"wrong-protocol secret-canary-do-not-log")
        .unwrap();
    assert!(matches!(
        fixture.acquire(),
        Err(HumanAuthorityFailure::StorageBindingMismatch)
    ));
    let fixture = Fixture::new(1);
    let owner = fixture.acquire().unwrap();
    let token = owner.source_token(fixture.sources[0].source_id).unwrap();
    let epoch = owner.try_read_epoch().unwrap();
    let source_path = fixture.sources[0].storage_binding.path();
    std::fs::set_permissions(&source_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        epoch.validate_source(&token),
        Err(HumanAuthorityFailure::OwnerInvalid)
    );
    std::fs::set_permissions(&source_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        owner.try_read_epoch(),
        Err(HumanAuthorityFailure::OwnerInvalid)
    ));
    assert_eq!(
        std::fs::read(source_path).unwrap(),
        b"filesystem fixture only"
    );
}

#[test]
fn replacement_configuration_and_source_inode_are_detected() {
    // Metadata-only main-file checks and retained config-content checks must
    // both execute on every epoch validation.
    let fixture = Fixture::new(1);
    let owner = fixture.acquire().unwrap();
    std::fs::write(fixture.files[1].1.path(), b"different-private-credential").unwrap();
    assert!(matches!(
        owner.try_read_epoch(),
        Err(HumanAuthorityFailure::OwnerInvalid)
    ));

    let fixture = Fixture::new(1);
    let owner = fixture.acquire().unwrap();
    let path = fixture.sources[0].storage_binding.path();
    std::fs::rename(&path, path.with_extension("retained")).unwrap();
    let replacement = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .unwrap();
    replacement
        .set_permissions(std::fs::Permissions::from_mode(0o600))
        .unwrap();
    assert!(matches!(
        owner.try_write_epoch(),
        Err(HumanAuthorityFailure::OwnerInvalid)
    ));
}

#[test]
fn source_mount_validation_keeps_exact_parent_and_entry_bindings() {
    // Comparing only the source inode, or only the retained parent, must not
    // admit a binding whose other physical identity differs from observation.
    let fixture = Fixture::new(2);
    let expected = &fixture.sources[0].storage_binding;
    let other = &fixture.sources[1].storage_binding;
    let parent = Directory::open_existing(&expected.parent_path).unwrap();

    let mut wrong_parent = expected.clone();
    wrong_parent.parent_device = other.parent_device;
    wrong_parent.parent_inode = other.parent_inode;
    let mut wrong_entry = expected.clone();
    wrong_entry.source_device = other.source_device;
    wrong_entry.source_inode = other.source_inode;
    for binding in [wrong_parent, wrong_entry] {
        assert!(matches!(
            bound_source_entry(&parent, &binding),
            Err(OwnerFilesystemFailure::Permanent(
                HumanAuthorityFailure::StorageBindingMismatch
            ))
        ));
    }
}

#[test]
fn source_mount_failures_keep_parent_unavailable_and_entry_binding_categories() {
    // R1: the first parent observation used to map through unavailable in
    // bound_entry. A later private-entry failure must still map through binding.
    let fixture = Fixture::new(1);
    let binding = &fixture.sources[0].storage_binding;
    let parent = Directory::open_existing(&binding.parent_path).unwrap();
    std::fs::set_permissions(&binding.parent_path, std::fs::Permissions::from_mode(0o750)).unwrap();
    let parent_failure = bound_source_entry(&parent, binding).unwrap_err();
    std::fs::set_permissions(&binding.parent_path, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        HumanAuthorityFailure::from(parent_failure),
        HumanAuthorityFailure::StorageUnavailable
    );

    std::fs::set_permissions(binding.path(), std::fs::Permissions::from_mode(0o640)).unwrap();
    let entry_failure = bound_source_entry(&parent, binding).unwrap_err();
    assert_eq!(
        HumanAuthorityFailure::from(entry_failure),
        HumanAuthorityFailure::StorageBindingMismatch
    );
}

#[test]
fn an_accepted_epoch_rejects_parent_substitution_after_a_successful_source_check() {
    // A successful check cannot authorize a later use of the same epoch/token
    // after another source's parent has been physically substituted.
    let fixture = Fixture::new(2);
    let owner = fixture.acquire().unwrap();
    let token = owner.source_token(fixture.sources[0].source_id).unwrap();
    let epoch = owner.try_read_epoch().unwrap();
    epoch.validate_source(&token).unwrap();

    let original = &fixture.sources[1].storage_binding.parent_path;
    let retained = original.with_extension("retained");
    let replacement = original.with_extension("replacement");
    std::fs::rename(original, &retained).unwrap();
    let substituted = fixture.root.child(original.file_name().unwrap()).unwrap();
    substituted
        .persistent_create("tenant.sqlite", b"filesystem fixture only")
        .unwrap();
    assert_eq!(
        epoch.validate_source(&token),
        Err(HumanAuthorityFailure::OwnerInvalid)
    );

    // Restoring the original name cannot undo the proven invalidation, and
    // neither the failed check nor restoration releases the retained locks.
    std::fs::rename(original, &replacement).unwrap();
    std::fs::rename(&retained, original).unwrap();
    assert_eq!(epoch.check(), Err(HumanAuthorityFailure::OwnerInvalid));
    assert!(matches!(
        owner.try_read_epoch(),
        Err(HumanAuthorityFailure::OwnerInvalid)
    ));
    assert!(matches!(
        fixture.acquire(),
        Err(HumanAuthorityFailure::OwnerContended)
    ));
}

#[test]
fn exact_closure_domains_and_foreign_source_tokens_are_rejected() {
    // A token with equal source UUID from another owner is not membership.
    let first = Fixture::new(1);
    let second = Fixture::new(1);
    let a = first.acquire().unwrap();
    let b = second.acquire().unwrap();
    let foreign = b.source_token(second.sources[0].source_id).unwrap();
    assert_eq!(
        a.try_read_epoch().unwrap().validate_source(&foreign),
        Err(HumanAuthorityFailure::SourceMismatch)
    );
    assert!(matches!(
        a.source_token(Uuid::new_v4()),
        Err(HumanAuthorityFailure::SourceMismatch)
    ));
    let incomplete = HumanSourcesConfigurationV1::new(
        Vec::new(),
        first.configuration.clone(),
        first.files.clone(),
    );
    assert!(matches!(
        incomplete,
        Err(HumanAuthorityFailure::InvalidManifest)
    ));
    let mut duplicated = first.sources.clone();
    duplicated.push(duplicated[0].clone());
    assert!(matches!(
        HumanSourcesConfigurationV1::new(
            duplicated,
            first.configuration.clone(),
            first.files.clone()
        ),
        Err(HumanAuthorityFailure::InvalidManifest)
    ));
}

#[test]
fn manifest_is_closed_digest_bound_and_configuration_exact() {
    // Unknown fields, changed private binding or configuration revision cannot
    // be silently ignored by parsing or an OS-only constructor.
    let fixture = Fixture::new(1);
    let mut value: serde_json::Value = serde_json::from_slice(fixture.manifest.bytes()).unwrap();
    value["unexpected"] = serde_json::json!(true);
    assert!(HumanAuthorityClosureV1::from_bytes(&serde_json::to_vec(&value).unwrap()).is_err());
    value.as_object_mut().unwrap().remove("unexpected");
    value["closure_revision"] = serde_json::json!(2);
    assert!(matches!(
        HumanAuthorityClosureV1::from_bytes(&serde_json::to_vec(&value).unwrap()),
        Err(HumanAuthorityFailure::InvalidManifest)
    ));
    let mut configuration = fixture.configuration.clone();
    configuration.control_config_revision += 1;
    let configured = HumanSourcesConfigurationV1::new(
        fixture.sources.clone(),
        configuration,
        fixture.files.clone(),
    )
    .unwrap();
    assert!(matches!(
        OwnedHumanSources::acquire(fixture.manifest.clone(), configured),
        Err(HumanAuthorityFailure::ConfigurationMismatch)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_drains_cancelled_observer_and_retains_the_owner_until_task_finishes() {
    // Dropping a handler future must not unlock while its blocking commit can
    // still execute. Channels coordinate the actual accepted closure.
    let fixture = Fixture::new(1);
    let owner = fixture.acquire().unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let observer_owner = owner.clone();
    let observer = tokio::spawn(async move {
        observer_owner
            .run_write(move |epoch| {
                epoch.check()?;
                let _ = started_tx.send(());
                release_rx
                    .recv_timeout(Duration::from_secs(1))
                    .map_err(|_| HumanAuthorityFailure::TaskFailed)?;
                epoch.check()
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), started_rx)
        .await
        .unwrap()
        .unwrap();
    owner.begin_close();
    observer.abort();
    let _ = observer.await;
    assert!(matches!(
        owner.try_read_epoch(),
        Err(HumanAuthorityFailure::Closing)
    ));
    assert!(matches!(
        fixture.acquire(),
        Err(HumanAuthorityFailure::OwnerContended)
    ));
    let drain_owner = owner.clone();
    let mut draining = tokio::spawn(async move { drain_owner.drain().await });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut draining)
            .await
            .is_err()
    );
    release_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), draining)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(owner);
    fixture.acquire().unwrap();
}

#[tokio::test]
async fn read_write_ordering_and_bounded_wait_do_not_poison_the_owner() {
    // A write cannot overtake a retained read; a timed-out admission is local.
    let fixture = Fixture::new(1);
    let owner = fixture.acquire().unwrap();
    let read = owner.try_read_epoch().unwrap();
    assert!(matches!(
        owner.try_write_epoch(),
        Err(HumanAuthorityFailure::GateBusy)
    ));
    assert!(matches!(
        tokio::time::timeout(Duration::from_millis(750), owner.write_epoch())
            .await
            .unwrap(),
        Err(HumanAuthorityFailure::GateBusy)
    ));
    read.check().unwrap();
    drop(read);
    let write = owner.try_write_epoch().unwrap();
    assert!(matches!(
        owner.try_read_epoch(),
        Err(HumanAuthorityFailure::GateBusy)
    ));
    drop(write);
    owner.try_read_epoch().unwrap().check().unwrap();
}

#[test]
fn an_expired_epoch_does_not_latch_unrelated_future_operations() {
    // Epoch expiry is per operation, whereas a failed identity is global.
    let fixture = Fixture::new(1);
    let owner = fixture.acquire().unwrap();
    let mut epoch = owner.try_read_epoch().unwrap();
    epoch.lease.deadline = Instant::now();
    assert_eq!(epoch.check(), Err(HumanAuthorityFailure::EpochExpired));
    drop(epoch);
    owner.try_write_epoch().unwrap().check().unwrap();
}

#[test]
fn diagnostics_and_runtime_debug_do_not_include_private_paths_or_contents() {
    let fixture = Fixture::new(1);
    let owner = fixture.acquire().unwrap();
    let token = owner.source_token(fixture.sources[0].source_id).unwrap();
    let epoch = owner.try_read_epoch().unwrap();
    let rendering = format!(
        "{owner:?} {token:?} {epoch:?} {:?} {}",
        HumanAuthorityFailure::StorageBindingMismatch,
        HumanAuthorityFailure::StorageBindingMismatch
    );
    assert!(!rendering.contains(fixture.root.path().to_str().unwrap()));
    assert!(!rendering.contains("secret-canary"));
    assert!(!rendering.contains("tenant.sqlite"));
}

#[test]
fn replacing_the_lock_inode_latches_the_original_owner_closed() {
    // A retained old flock is not authority over a newly substituted entry.
    let fixture = Fixture::new(1);
    let owner = fixture.acquire().unwrap();
    let parent = Directory::open_existing(&fixture.sources[0].storage_binding.parent_path).unwrap();
    std::fs::rename(
        parent.path().join(OWNER_LOCK_NAME),
        parent.path().join("old-retained-owner.lock"),
    )
    .unwrap();
    parent
        .persistent_create(OWNER_LOCK_NAME, OWNER_LOCK_PROTOCOL)
        .unwrap();
    assert!(matches!(
        owner.try_read_epoch(),
        Err(HumanAuthorityFailure::OwnerInvalid)
    ));
    assert!(matches!(
        owner.source_token(fixture.sources[0].source_id),
        Err(HumanAuthorityFailure::OwnerInvalid)
    ));
    assert_eq!(
        std::fs::read(parent.path().join("old-retained-owner.lock")).unwrap(),
        OWNER_LOCK_PROTOCOL
    );
}

#[test]
fn hardlinks_and_symlinks_cannot_alias_a_source() {
    // Relaxing single-link or nofollow checks would make this owner reusable.
    let fixture = Fixture::new(1);
    let owner = fixture.acquire().unwrap();
    let path = fixture.sources[0].storage_binding.path();
    std::fs::hard_link(&path, path.with_extension("alias")).unwrap();
    assert!(matches!(
        owner.try_write_epoch(),
        Err(HumanAuthorityFailure::OwnerInvalid)
    ));
    let fixture = Fixture::new(1);
    let owner = fixture.acquire().unwrap();
    let path = fixture.sources[0].storage_binding.path();
    let retained = path.with_extension("retained");
    std::fs::rename(&path, &retained).unwrap();
    std::os::unix::fs::symlink(&retained, &path).unwrap();
    assert!(matches!(
        owner.try_read_epoch(),
        Err(HumanAuthorityFailure::OwnerInvalid)
    ));
}

#[test]
fn manifest_source_bound_is_not_bypassed_with_unique_descriptors() {
    // The 257th *unique* source must fail for size, not just a duplicate ID.
    let fixture = Fixture::new(1);
    let sources: Vec<_> = (1..=257)
        .map(|index| {
            let mut source = fixture.sources[0].clone();
            source.source_id = Uuid::from_u128(index);
            source.durable_namespace_id = Uuid::from_u128(index + 1024);
            source.tenant_id = Some(format!("tenant-{index}"));
            source.storage_binding.source_inode = index as u64 + 1024;
            source.storage_binding.file_name = format!("tenant-{index}.sqlite");
            source
        })
        .collect();
    assert!(matches!(
        HumanSourcesConfigurationV1::new(
            sources,
            fixture.configuration.clone(),
            fixture.files.clone()
        ),
        Err(HumanAuthorityFailure::InvalidManifest)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detached_work_alone_retains_the_os_owner_after_the_observer_is_cancelled() {
    // No caller/token/owner handle remains here to accidentally keep the flock.
    let fixture = Fixture::new(1);
    let owner = fixture.acquire().unwrap();
    let weak = Arc::downgrade(&owner.owner);
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let observer_owner = owner.clone();
    let observer = tokio::spawn(async move {
        observer_owner
            .run_read(move |epoch| {
                epoch.check()?;
                let _ = started_tx.send(());
                release_rx
                    .recv_timeout(Duration::from_secs(1))
                    .map_err(|_| HumanAuthorityFailure::TaskFailed)?;
                epoch.check()
            })
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), started_rx)
        .await
        .unwrap()
        .unwrap();
    observer.abort();
    let _ = observer.await;
    drop(owner);
    assert!(matches!(
        fixture.acquire(),
        Err(HumanAuthorityFailure::OwnerContended)
    ));
    release_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture.acquire().unwrap();
}

#[test]
fn an_independent_process_observes_the_retained_source_owner() {
    // This observes the actual OS flock from another PID, not a RAM registry.
    let fixture = Fixture::new(1);
    let _owner = fixture.acquire().unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "human_authority::tests::helper_process_observes_retained_owner_contention",
            "--ignored",
        ])
        .env_clear()
        .env("AIGUARD_HUMAN_OWNER_TEST_MANIFEST", fixture.manifest.path())
        .current_dir(fixture.root.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    struct ChildOwner(std::process::Child);
    impl Drop for ChildOwner {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = ChildOwner(child);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(
                status.success(),
                "independent owner helper refused its oracle"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "independent owner helper exceeded its bound"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "internal child helper; only invoked by its bounded parent"]
fn helper_process_observes_retained_owner_contention() {
    let path = std::env::var_os("AIGUARD_HUMAN_OWNER_TEST_MANIFEST").unwrap();
    let manifest_file =
        PrivateFile::open(std::path::Path::new(&path), MAX_HUMAN_MANIFEST_BYTES as u64).unwrap();
    let manifest = HumanAuthorityClosureV1::from_bytes(manifest_file.bytes()).unwrap();
    let mut files = Vec::new();
    for descriptor in &manifest.configuration.files {
        let parent = Directory::open_existing(&descriptor.storage_binding.parent_path).unwrap();
        let file = parent
            .persistent_read(&descriptor.storage_binding.file_name, 1_048_576, &[])
            .unwrap()
            .unwrap();
        files.push((descriptor.kind, file));
    }
    let configured =
        HumanSourcesConfigurationV1::new(manifest.sources, manifest.configuration, files).unwrap();
    assert!(matches!(
        OwnedHumanSources::acquire(manifest_file, configured),
        Err(HumanAuthorityFailure::OwnerContended)
    ));
}

#[tokio::test]
async fn typed_source_invalidation_rejects_all_accepted_epochs_and_retains_locks_until_drop() {
    // HARB-01: a proven mismatch in source A must invalidate already accepted
    // reads in B too, without turning invalidation into an owner-lock release.
    let fixture = Fixture::new(2);
    let owner = fixture.acquire().unwrap();
    let token_a = owner.source_token(fixture.sources[0].source_id).unwrap();
    let token_b = owner.source_token(fixture.sources[1].source_id).unwrap();
    let epoch_a = owner.try_read_epoch().unwrap();
    let epoch_b = owner.try_read_epoch().unwrap();
    epoch_a.validate_source(&token_a).unwrap();
    epoch_b.validate_source(&token_b).unwrap();

    // This is the typed integration call after Root's durable-source oracle.
    // The fixture itself deliberately provides no fake SQLite marker proof.
    token_a.invalidate_authority(HumanAuthorityInvalidationReason::DurableAuthorityMismatch);
    assert_eq!(epoch_a.check(), Err(HumanAuthorityFailure::OwnerInvalid));
    assert_eq!(
        epoch_b.validate_source(&token_b),
        Err(HumanAuthorityFailure::OwnerInvalid)
    );
    assert!(matches!(
        owner.try_read_epoch(),
        Err(HumanAuthorityFailure::OwnerInvalid)
    ));
    assert!(matches!(
        owner.try_write_epoch(),
        Err(HumanAuthorityFailure::OwnerInvalid)
    ));
    assert!(matches!(
        owner.read_epoch().await,
        Err(HumanAuthorityFailure::OwnerInvalid)
    ));
    assert!(matches!(
        owner.write_epoch().await,
        Err(HumanAuthorityFailure::OwnerInvalid)
    ));
    for source in &fixture.sources {
        assert!(matches!(
            owner.source_token(source.source_id),
            Err(HumanAuthorityFailure::OwnerInvalid)
        ));
    }
    assert!(matches!(
        fixture.acquire(),
        Err(HumanAuthorityFailure::OwnerContended)
    ));

    let drain_owner = owner.clone();
    let mut draining = tokio::spawn(async move { drain_owner.drain().await });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut draining)
            .await
            .is_err()
    );
    drop(epoch_a);
    drop(epoch_b);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), draining)
            .await
            .unwrap()
            .unwrap(),
        Err(HumanAuthorityFailure::OwnerInvalid)
    );
    drop(owner);
    drop(token_a);
    // Token B still owns the actual invalid closure's OS locks after draining.
    assert!(matches!(
        fixture.acquire(),
        Err(HumanAuthorityFailure::OwnerContended)
    ));
    drop(token_b);
    fixture.acquire().unwrap();
}

#[test]
fn typed_owner_invalidation_rejects_an_already_accepted_write_epoch() {
    // The owner entrypoint must reach the same latch as the retained token.
    let fixture = Fixture::new(1);
    let owner = fixture.acquire().unwrap();
    let token = owner.source_token(fixture.sources[0].source_id).unwrap();
    let epoch = owner.try_write_epoch().unwrap();
    epoch.validate_source(&token).unwrap();
    owner.invalidate_authority(HumanAuthorityInvalidationReason::RetainedConfigurationMismatch);
    assert_eq!(epoch.check(), Err(HumanAuthorityFailure::OwnerInvalid));
    assert!(matches!(
        owner.source_token(token.source_id()),
        Err(HumanAuthorityFailure::OwnerInvalid)
    ));
    assert!(matches!(
        fixture.acquire(),
        Err(HumanAuthorityFailure::OwnerContended)
    ));
}

#[test]
fn normal_close_allows_existing_epochs_to_finish_without_invalidating_them() {
    // begin_close is intentionally not the durable-mismatch invalidator.
    let fixture = Fixture::new(2);
    let owner = fixture.acquire().unwrap();
    let token_a = owner.source_token(fixture.sources[0].source_id).unwrap();
    let token_b = owner.source_token(fixture.sources[1].source_id).unwrap();
    let epoch_a = owner.try_read_epoch().unwrap();
    let epoch_b = owner.try_read_epoch().unwrap();
    owner.begin_close();
    epoch_a.validate_source(&token_a).unwrap();
    epoch_b.validate_source(&token_b).unwrap();
    assert!(matches!(
        owner.try_read_epoch(),
        Err(HumanAuthorityFailure::Closing)
    ));
    assert!(matches!(
        owner.source_token(token_a.source_id()),
        Err(HumanAuthorityFailure::Closing)
    ));
}

#[tokio::test]
async fn individual_work_refusal_and_wrong_source_do_not_latch_the_authority() {
    // run_read must propagate an individual refusal, not feed every returned
    // error into the new global durable-mismatch invalidator.
    let fixture = Fixture::new(1);
    let owner = fixture.acquire().unwrap();
    let result = owner
        .run_read(|_| Err::<(), _>(HumanAuthorityFailure::SourceMismatch))
        .await;
    assert_eq!(result, Err(HumanAuthorityFailure::SourceMismatch));
    assert!(matches!(
        owner.source_token(Uuid::new_v4()),
        Err(HumanAuthorityFailure::SourceMismatch)
    ));
    let token = owner.source_token(fixture.sources[0].source_id).unwrap();
    owner
        .try_read_epoch()
        .unwrap()
        .validate_source(&token)
        .unwrap();
}

fn contextual_io_failure(errno: i32) -> anyhow::Error {
    anyhow::Error::new(std::io::Error::from_raw_os_error(errno))
        .context("inner private observation")
        .context("outer private observation secret-canary-do-not-log")
}

#[test]
fn initial_parent_observation_phase_keeps_structured_descriptor_shortage_causes() {
    // The typed context must preserve the original IO chain so that this
    // phase still refuses without turning EMFILE/ENFILE into a permanent fault.
    for errno in [libc::EMFILE, libc::ENFILE] {
        let failure = contextual_io_failure(errno)
            .context(crate::control_fs::InitialParentObservationFailure);
        assert!(failure.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.raw_os_error() == Some(errno))
        }));
        assert!(matches!(
            OwnerFilesystemFailure::source_binding(failure),
            OwnerFilesystemFailure::DescriptorShortage
        ));
    }
}

#[test]
fn only_structured_emfile_and_enfile_classify_as_descriptor_shortage() {
    // HARB-03: string matching, root-only downcasting, or broad unavailable
    // classification would fail these positive/negative IO-cause oracles.
    for errno in [libc::EMFILE, libc::ENFILE] {
        for permanent in [
            HumanAuthorityFailure::StorageUnavailable,
            HumanAuthorityFailure::StorageBindingMismatch,
        ] {
            assert!(matches!(
                OwnerFilesystemFailure::classify(contextual_io_failure(errno), permanent),
                OwnerFilesystemFailure::DescriptorShortage
            ));
        }
    }
    for errno in [
        libc::EIO,
        libc::ENOSPC,
        libc::ENOMEM,
        libc::EACCES,
        libc::ENOENT,
        libc::EAGAIN,
    ] {
        assert!(matches!(
            OwnerFilesystemFailure::classify(
                contextual_io_failure(errno),
                HumanAuthorityFailure::StorageUnavailable
            ),
            OwnerFilesystemFailure::Permanent(HumanAuthorityFailure::StorageUnavailable)
        ));
    }
    for error in [
        anyhow::anyhow!("EMFILE ENFILE secret-canary-do-not-log"),
        anyhow::Error::new(std::io::Error::other("EMFILE ENFILE")),
    ] {
        assert!(matches!(
            OwnerFilesystemFailure::classify(error, HumanAuthorityFailure::StorageBindingMismatch),
            OwnerFilesystemFailure::Permanent(HumanAuthorityFailure::StorageBindingMismatch)
        ));
    }
}

#[test]
fn structured_descriptor_shortage_refuses_without_unlock_and_same_owner_revalidates() {
    // The production result handler is exercised with a constructed IO cause,
    // not resource exhaustion or a replacement filesystem implementation.
    for errno in [libc::EMFILE, libc::ENFILE] {
        let fixture = Fixture::new(2);
        let owner = fixture.acquire().unwrap();
        let incarnation = owner.incarnation();
        let token = owner.source_token(fixture.sources[1].source_id).unwrap();
        let retained = owner.try_read_epoch().unwrap();
        let failure = OwnerFilesystemFailure::classify(
            contextual_io_failure(errno),
            HumanAuthorityFailure::StorageUnavailable,
        );
        assert_eq!(
            owner.owner.complete_filesystem_revalidation(Err(failure)),
            Err(HumanAuthorityFailure::StorageUnavailable)
        );
        assert!(!owner.owner.lock_state().unwrap().invalid);
        assert!(matches!(
            owner.try_write_epoch(),
            Err(HumanAuthorityFailure::GateBusy)
        ));
        assert!(matches!(
            fixture.acquire(),
            Err(HumanAuthorityFailure::OwnerContended)
        ));

        // These are new full production FS validations, not supplied Ok values.
        retained.validate_source(&token).unwrap();
        owner
            .try_read_epoch()
            .unwrap()
            .validate_source(&token)
            .unwrap();
        assert_eq!(owner.incarnation(), incarnation);
    }
}

#[test]
fn descriptor_shortage_followed_by_changed_other_source_never_reuses_an_old_check() {
    // Replacing the mandatory next full validation with cached success would
    // miss the actual source-B mode change while source A was being used.
    let fixture = Fixture::new(2);
    let owner = fixture.acquire().unwrap();
    let token_a = owner.source_token(fixture.sources[0].source_id).unwrap();
    let retained = owner.try_read_epoch().unwrap();
    let failure = OwnerFilesystemFailure::classify(
        contextual_io_failure(libc::EMFILE),
        HumanAuthorityFailure::StorageBindingMismatch,
    );
    assert_eq!(
        owner.owner.complete_filesystem_revalidation(Err(failure)),
        Err(HumanAuthorityFailure::StorageUnavailable)
    );
    let path_b = fixture.sources[1].storage_binding.path();
    std::fs::set_permissions(&path_b, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        retained.validate_source(&token_a),
        Err(HumanAuthorityFailure::OwnerInvalid)
    );
    std::fs::set_permissions(path_b, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        owner.try_read_epoch(),
        Err(HumanAuthorityFailure::OwnerInvalid)
    ));
    assert!(matches!(
        fixture.acquire(),
        Err(HumanAuthorityFailure::OwnerContended)
    ));
}

#[test]
fn unknown_and_non_descriptor_failures_still_latch_the_owner() {
    // StorageUnavailable as a public category must not become generally
    // retryable; only the privately classified structured shortage is narrow.
    for error in [
        contextual_io_failure(libc::EIO),
        contextual_io_failure(libc::ENOSPC),
        anyhow::anyhow!("unavailable observation EMFILE"),
    ] {
        let fixture = Fixture::new(1);
        let owner = fixture.acquire().unwrap();
        let retained = owner.try_read_epoch().unwrap();
        let failure =
            OwnerFilesystemFailure::classify(error, HumanAuthorityFailure::StorageUnavailable);
        assert_eq!(
            owner.owner.complete_filesystem_revalidation(Err(failure)),
            Err(HumanAuthorityFailure::OwnerInvalid)
        );
        assert_eq!(retained.check(), Err(HumanAuthorityFailure::OwnerInvalid));
        assert!(matches!(
            owner.try_read_epoch(),
            Err(HumanAuthorityFailure::OwnerInvalid)
        ));
        assert!(matches!(
            fixture.acquire(),
            Err(HumanAuthorityFailure::OwnerContended)
        ));
    }
}

#[test]
fn descriptor_shortage_cannot_clear_a_proven_durable_invalidation() {
    // HARB-01's permanent latch wins even if a later observation lacks FDs.
    let fixture = Fixture::new(1);
    let owner = fixture.acquire().unwrap();
    let retained = owner.try_read_epoch().unwrap();
    owner.invalidate_authority(HumanAuthorityInvalidationReason::DurableAuthorityMismatch);
    let failure = OwnerFilesystemFailure::classify(
        contextual_io_failure(libc::ENFILE),
        HumanAuthorityFailure::StorageUnavailable,
    );
    assert_eq!(
        owner.owner.complete_filesystem_revalidation(Err(failure)),
        Err(HumanAuthorityFailure::OwnerInvalid)
    );
    assert_eq!(retained.check(), Err(HumanAuthorityFailure::OwnerInvalid));
    assert!(matches!(
        fixture.acquire(),
        Err(HumanAuthorityFailure::OwnerContended)
    ));
}

#[test]
fn retained_tokens_compare_exact_owner_and_source_without_an_authority_assertion() {
    // Equal source UUIDs in another actual owner must not make Store's retained
    // token equal. Clones retain exactly the same owner/source/incarnation.
    let fixture = Fixture::new(2);
    let foreign_fixture = Fixture::new(1);
    let owner = fixture.acquire().unwrap();
    let foreign_owner = foreign_fixture.acquire().unwrap();
    let first = owner.source_token(fixture.sources[0].source_id).unwrap();
    let first_clone = first.clone();
    let second = owner.source_token(fixture.sources[1].source_id).unwrap();
    let foreign = foreign_owner
        .source_token(foreign_fixture.sources[0].source_id)
        .unwrap();
    assert!(first.same_owner_and_source(&first_clone));
    assert!(!first.same_owner_and_source(&second));
    assert!(!first.same_owner_and_source(&foreign));
}

#[test]
fn retained_token_revalidates_under_an_existing_write_epoch_without_reacquiring_gate() {
    // A recursive gate acquisition would fail under this actual write epoch;
    // a cached check would miss the later real source-B mode change.
    let fixture = Fixture::new(2);
    let owner = fixture.acquire().unwrap();
    let token = owner.source_token(fixture.sources[0].source_id).unwrap();
    let epoch = owner.try_write_epoch().unwrap();
    token.revalidate_authority().unwrap();
    owner.begin_close();
    token.revalidate_authority().unwrap();
    epoch.validate_source(&token).unwrap();
    let path_b = fixture.sources[1].storage_binding.path();
    std::fs::set_permissions(path_b, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        token.revalidate_authority(),
        Err(HumanAuthorityFailure::OwnerInvalid)
    );
    assert_eq!(epoch.check(), Err(HumanAuthorityFailure::OwnerInvalid));
}
