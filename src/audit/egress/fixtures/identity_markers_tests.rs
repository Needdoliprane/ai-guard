//! In-place identity markers on adopted owned sources (Store14/15): real
//! private SQLite, the real maintenance guard, journal and verified backup.
use super::*;
use crate::audit::identity_markers::{IdentityMarkersFailure, InPlaceIdentityMarkerV1};
use crate::control_fs::publication::FAIL_AFTER_RENAME;
use crate::human_authority::migration::egress_journal::RetainedSourceIntent;
use crate::human_authority::migration::operator::{
    OwnedIdentityMarkersRefusalV1, OwnedIdentityMarkersRequest, install_owned_identity_markers,
};

const MARKER_KIND: &str = "control.identity.human_rights.genesis";

/// An adopted union whose published active closure lists every source at
/// `format`. The startup owner is released; only its manifest is retained.
struct Adopted {
    base: BaseFixture,
    anchors: Vec<authority::AuthorityReceiptV1>,
    closure: HumanAuthorityClosureV1,
    manifest_name: String,
}

fn adopted(format: u32) -> Adopted {
    let base = BaseFixture::new(&["A", "B"]);
    let anchors = base.migrate_all();
    if format == 15 {
        migrate15(&base, &EgressStoreLimitsV1::default());
    }
    let manifest_name = format!("active{format}-identity-markers.json");
    let owner = base.owner(&anchors, &manifest_name, format);
    let closure = owner.closure().clone();
    drop(owner);
    Adopted {
        base,
        anchors,
        closure,
        manifest_name,
    }
}

impl Adopted {
    fn configuration_dir(&self) -> Directory {
        Directory::open_existing(self.base.plan_file.path().parent().unwrap()).unwrap()
    }

    fn manifest(&self) -> PrivateFile {
        PrivateFile::open(
            &self.configuration_dir().path().join(&self.manifest_name),
            crate::human_authority::MAX_HUMAN_MANIFEST_BYTES as u64,
        )
        .unwrap()
    }

    /// A new in-place plan (prior = target = active closure) and its guard.
    fn in_place(
        &self,
        index: usize,
    ) -> (HumanSourcesMaintenancePlanV1, HumanSourcesMaintenanceGuard) {
        let configuration = self.configuration_dir();
        let plan = HumanSourcesMaintenancePlanV1::in_place(
            Uuid::new_v4(),
            &self.closure,
            binding(&configuration, &self.manifest_name),
            self.source_id(index),
        );
        let file = configuration
            .persistent_create(
                &format!("in-place-{}.json", plan.migration_id),
                &serde_json::to_vec(&plan).unwrap(),
            )
            .unwrap();
        let guard = self.acquire(&plan, &file).unwrap();
        (plan, guard)
    }

    fn acquire(
        &self,
        plan: &HumanSourcesMaintenancePlanV1,
        file: &PrivateFile,
    ) -> Result<HumanSourcesMaintenanceGuard, crate::human_authority::HumanAuthorityFailure> {
        let resolve = || {
            HumanMaintenanceResolvedViewV1::new(
                plan.target.sources.clone(),
                plan.target.configuration.clone(),
                self.base.configuration_files.clone(),
            )
        };
        HumanSourcesMaintenanceGuard::acquire(
            file.clone(),
            HumanSourcesMaintenanceConfigurationV1::new(
                Some(resolve()?),
                resolve()?,
                Some(self.manifest()),
            ),
        )
    }

    fn directories(&self, tag: &str) -> (Directory, Directory) {
        (
            self.base
                .root
                .persistent_child(&format!("markers-journal-{tag}"), true)
                .unwrap()
                .unwrap(),
            self.base
                .root
                .persistent_child(&format!("markers-backup-{tag}"), true)
                .unwrap()
                .unwrap(),
        )
    }

    fn source_id(&self, index: usize) -> Uuid {
        self.base.plan.target.sources[index].source_id
    }

    /// Reopen the source through a fresh owner startup, replaying every
    /// event, and retain it as the gateway would.
    fn retained(&self, index: usize, manifest: &str, format: u32) -> Retained {
        let owner = self.base.owner(&self.anchors, manifest, format);
        let source = owner.source_token(self.source_id(index)).unwrap();
        let startup = self.base.startup(index, &source);
        let epoch = owner.try_write_epoch().unwrap();
        let revision = startup.current_incarnation_revision(&epoch).unwrap();
        startup
            .append_incarnation(&epoch, &source, Uuid::new_v4(), revision)
            .unwrap();
        let store = startup.retain(&epoch, &source).unwrap();
        drop(epoch);
        Retained {
            store,
            source,
            owner,
        }
    }
}

/// An owned source retained by a live owner, as the gateway holds it.
struct Retained {
    store: Store,
    source: OwnedSourceToken,
    owner: OwnedHumanSources,
}

fn marker_rows(store: &Store) -> Vec<(i64, String)> {
    let db = store.db.lock().unwrap();
    db.prepare("SELECT seq,id FROM events WHERE kind=?1 ORDER BY seq")
        .unwrap()
        .query_map([MARKER_KIND], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn journal_names(journal: &Directory) -> Vec<String> {
    let mut names: Vec<_> = journal
        .persistent_entries(8)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    names
}

/// A new human read action through the owned read path, never committed.
fn summary_read_refusal(retained: &Retained) -> Option<crate::identity::IdentityFailureV1> {
    let principal = crate::identity::PrincipalRefV1::HumanLocal {
        installation_id: Uuid::new_v4().to_string(),
        account_id: Uuid::new_v4().to_string(),
    };
    let epoch = retained.owner.try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &retained.source).unwrap();
    let store = &retained.store;
    let mut db = store.db.lock().unwrap();
    let tx = store
        .begin_checked_scoped(&mut db, &access, StoreOperationClass::Business)
        .unwrap();
    let result = store.record_human_read_checked(
        &tx,
        &principal,
        None,
        "summary",
        0,
        &Redactor::new(vec![]),
    );
    // Dropped without commit: the probe leaves the store unchanged.
    drop(tx);
    result.err().and_then(|e| {
        e.downcast_ref::<crate::identity::IdentityFailureV1>()
            .copied()
    })
}

/// Trusted owned provisioning of a membership carrying a fenced capability.
fn provision_fenced(
    retained: &Retained,
) -> anyhow::Result<Vec<crate::identity::IdentityReceiptV1>> {
    use crate::identity::*;
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: retained.source.descriptor().installation_id.to_string(),
        account_id: Uuid::new_v4().to_string(),
    };
    let mutation = |change| IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "synthetic-offline-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    };
    let provision = LocalHumanProvisionV1 {
        version: 1,
        mutations: [
            mutation(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal.clone(),
                    display_name: "Synthetic membership reader".into(),
                    revision: 1,
                    active: true,
                },
            }),
            mutation(IdentityChangeV1::Credential {
                expected_revision: 0,
                binding: LocalCredentialV1 {
                    version: 1,
                    credential_id: Uuid::new_v4().to_string(),
                    principal: principal.clone(),
                    sha256_digest: "a".repeat(64),
                    revision: 1,
                    active: true,
                    expires_at: None,
                },
            }),
            mutation(IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: MembershipV1 {
                    version: 1,
                    membership_id: Uuid::new_v4().to_string(),
                    principal,
                    tenant_id: "A".into(),
                    organization_scope: "org-a".into(),
                    revision: 1,
                    active: true,
                    capabilities: [HumanCapabilityV1::ReadMemberships].into_iter().collect(),
                },
            }),
        ],
    };
    let epoch = retained.owner.try_write_epoch()?;
    retained
        .store
        .provision_local_human_scoped(&epoch, &retained.source, &provision)
}

/// Full journaled run on source 0; returns the committed receipts.
fn install_all(
    adopted: &Adopted,
    guard: &HumanSourcesMaintenanceGuard,
    tag: &str,
) -> Vec<crate::audit::identity_markers::IdentityMarkerReceiptV1> {
    let token = guard.target_source(adopted.source_id(0)).unwrap();
    let (journal, backups) = adopted.directories(tag);
    let opened = adopted.base.open_maintenance(0, &token);
    let intent = opened
        .with_review_maintenance_writer(&token, |writer| {
            let intent =
                RetainedSourceIntent::prepare_identity_markers(&token, writer, journal, backups)?;
            let backup = intent.prepare_backup(writer)?;
            backup.install_identity_markers(writer)?;
            drop(backup);
            writer.commit()?;
            Ok(intent)
        })
        .unwrap();
    drop(opened);
    intent
        .confirm_identity_markers(Redactor::new(vec![]), 10_000)
        .unwrap()
}

fn assert_installs_once_and_admits(format: u32) {
    let adopted = adopted(format);
    let before = maintenance_source_state(&adopted.base);
    assert_eq!(before.format, i64::from(format));
    assert!(marker_rows(&adopted.base.stores[0]).is_empty());
    {
        let retained = adopted.retained(0, &format!("before-{format}.json"), format);
        let head = store_source_state(&retained.store).head;
        assert_eq!(
            summary_read_refusal(&retained),
            Some(crate::identity::IdentityFailureV1::MigrationRequired)
        );
        assert_eq!(
            provision_fenced(&retained)
                .unwrap_err()
                .downcast_ref::<crate::identity::IdentityFailureV1>(),
            Some(&crate::identity::IdentityFailureV1::MigrationRequired)
        );
        assert_eq!(store_source_state(&retained.store).head, head);
    }
    let before = maintenance_source_state(&adopted.base);
    let neighbor = store_source_state(&adopted.base.stores[1]);
    let (plan, guard) = adopted.in_place(0);
    let receipts = install_all(&adopted, &guard, "once");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].marker, InPlaceIdentityMarkerV1::HumanRightsV2);
    assert_eq!(receipts[0].seq, before.head.0 + 1);
    let after = maintenance_source_state(&adopted.base);
    assert_eq!(after.format, i64::from(format));
    assert_eq!(after.head, (receipts[0].seq, receipts[0].hash.clone()));
    assert_eq!(after.schema, before.schema);
    assert_eq!(
        marker_rows(&adopted.base.stores[0]),
        vec![(
            receipts[0].seq,
            InPlaceIdentityMarkerV1::HumanRightsV2.event_id(plan.migration_id)
        )]
    );
    // The neighbor source of the same union is untouched.
    assert_eq!(store_source_state(&adopted.base.stores[1]), neighbor);
    let (journal, backups) = adopted.directories("once");
    assert_eq!(
        journal_names(&journal),
        [
            "identity-markers-backup-v1",
            "identity-markers-confirmed-v1",
            "identity-markers-intent-v1"
        ]
    );
    // The backup was written and verified before the commit: it holds the
    // exact pre-marker prefix and no marker.
    let backup = rusqlite::Connection::open_with_flags(
        backups.path().join("source.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(audit_head(&backup).unwrap(), before.head);
    assert_eq!(
        backup
            .query_row(
                "SELECT count(*) FROM events WHERE kind=?1",
                [MARKER_KIND],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    drop(backup);
    drop(guard);
    // A fresh owner startup replays the marker and the gates now admit.
    let retained = adopted.retained(0, &format!("after-{format}.json"), format);
    assert!(retained.store.human_rights_installed().unwrap());
    assert_ne!(
        summary_read_refusal(&retained),
        Some(crate::identity::IdentityFailureV1::MigrationRequired)
    );
    assert_eq!(provision_fenced(&retained).unwrap().len(), 3);
    drop(retained);
    // The fenced membership itself replays on the next owner startup.
    let replayed = adopted.retained(0, &format!("replayed-{format}.json"), format);
    assert!(replayed.store.human_rights_installed().unwrap());
    assert_eq!(marker_rows(&replayed.store).len(), 1);
}

#[test]
fn owned_store14_receives_the_marker_once_and_admits_the_fenced_capabilities() {
    assert_installs_once_and_admits(14);
}

#[test]
fn owned_store15_receives_the_marker_once_and_keeps_its_egress_genesis() {
    assert_installs_once_and_admits(15);
}

#[test]
fn lost_acknowledgements_resume_to_exactly_one_marker() {
    let adopted = adopted(14);
    let initial = maintenance_source_state(&adopted.base);
    let (plan, guard) = adopted.in_place(0);
    let token = guard.target_source(adopted.source_id(0)).unwrap();
    let (journal, backups) = adopted.directories("lost");
    let opened = adopted.base.open_maintenance(0, &token);
    let resume = |writer: &ReviewStoreMaintenanceWriter<'_, '_>| {
        RetainedSourceIntent::resume_identity_markers(
            &token,
            writer,
            journal.clone(),
            backups.clone(),
        )
    };
    let intent = opened
        .with_review_maintenance_writer(&token, |writer| {
            // No intent yet: resume cannot start an operation.
            assert!(resume(writer).is_err());
            FAIL_AFTER_RENAME.with(|fault| fault.set(true));
            let lost = RetainedSourceIntent::prepare_identity_markers(
                &token,
                writer,
                journal.clone(),
                backups.clone(),
            )
            .map(|_| ())
            .unwrap_err();
            assert_eq!(
                lost.to_string(),
                "identity marker journal publication durability unknown"
            );
            // The published intent is never prepared twice; resume reads it.
            assert!(
                RetainedSourceIntent::prepare_identity_markers(
                    &token,
                    writer,
                    journal.clone(),
                    backups.clone()
                )
                .is_err()
            );
            let intent = resume(writer)?;
            assert_eq!(
                intent.identity_markers(),
                [InPlaceIdentityMarkerV1::HumanRightsV2]
            );
            FAIL_AFTER_RENAME.with(|fault| fault.set(true));
            let lost = intent.prepare_backup(writer).map(|_| ()).unwrap_err();
            assert_eq!(
                lost.to_string(),
                "identity marker journal publication durability unknown"
            );
            assert!(intent.prepare_backup(writer).is_err());
            let backup = intent.resume_backup(writer)?;
            backup.install_identity_markers(writer)?;
            Ok(intent)
        })
        .unwrap();
    // Dropped without commit: the writer transaction rolled back.
    assert_eq!(maintenance_source_state(&adopted.base), initial);
    assert!(
        intent
            .confirm_identity_markers(Redactor::new(vec![]), 10_000)
            .is_err()
    );
    let applied = opened
        .with_review_maintenance_writer(&token, |writer| {
            let backup = intent.resume_backup(writer)?;
            let receipts = backup.install_identity_markers(writer)?;
            drop(backup);
            writer.commit()?;
            Ok(receipts)
        })
        .unwrap();
    drop(opened);
    drop(intent);
    drop(token);
    drop(guard);
    // Committed, then the acknowledgement was lost twice: no confirmation
    // record yet, and the first confirmation publication is interrupted.
    let committed = maintenance_source_state(&adopted.base);
    assert_eq!(committed.head.0, initial.head.0 + 1);
    let configuration = adopted.configuration_dir();
    let plan_file = configuration
        .persistent_read(
            &format!("in-place-{}.json", plan.migration_id),
            512 * 1024,
            &[],
        )
        .unwrap()
        .unwrap();
    let guard = adopted.acquire(&plan, &plan_file).unwrap();
    let token = guard.target_source(adopted.source_id(0)).unwrap();
    let reopened = adopted.base.open_maintenance(0, &token);
    let retained = reopened
        .with_review_maintenance_writer(&token, |writer| {
            assert!(writer.identity_markers_committed(&token)?);
            let fresh = adopted.directories("lost-fresh");
            // A new maintenance cannot start: nothing is pending.
            let refused = RetainedSourceIntent::prepare_identity_markers(
                &token,
                writer,
                fresh.0.clone(),
                fresh.1,
            )
            .map(|_| ())
            .unwrap_err();
            assert_eq!(
                refused.downcast_ref::<IdentityMarkersFailure>(),
                Some(&IdentityMarkersFailure::Conflict)
            );
            assert!(fresh.0.persistent_entries(1)?.is_empty());
            let retained = RetainedSourceIntent::resume_identity_markers(
                &token,
                writer,
                journal.clone(),
                backups.clone(),
            )?;
            retained.resume_backup(writer)?;
            Ok(retained)
        })
        .unwrap();
    drop(reopened);
    FAIL_AFTER_RENAME.with(|fault| fault.set(true));
    let lost = retained
        .confirm_identity_markers(Redactor::new(vec![]), 10_000)
        .unwrap_err();
    assert_eq!(
        lost.to_string(),
        "identity marker journal publication durability unknown"
    );
    let confirmed = retained
        .confirm_identity_markers(Redactor::new(vec![]), 10_000)
        .unwrap();
    assert_eq!(confirmed, applied);
    // Idempotent after the confirmation record exists.
    assert_eq!(
        retained
            .confirm_identity_markers(Redactor::new(vec![]), 10_000)
            .unwrap(),
        applied
    );
    assert_eq!(maintenance_source_state(&adopted.base), committed);
    assert_eq!(marker_rows(&adopted.base.stores[0]).len(), 1);
    let read = |slot: &str| {
        journal
            .persistent_child(slot, false)
            .unwrap()
            .unwrap()
            .persistent_read("record-v1.json", 128 * 1024, &[])
            .unwrap()
            .unwrap()
    };
    let intent_file = read("identity-markers-intent-v1");
    let frame = read("identity-markers-backup-v1");
    assert_eq!(
        serde_json::from_slice::<Value>(read("identity-markers-confirmed-v1").bytes()).unwrap(),
        json!({
            "version": 1,
            "intent_digest": intent_file.digest(),
            "backup_digest": frame.digest(),
            "receipts": [{"marker": "human_rights_v2", "seq": applied[0].seq, "hash": applied[0].hash}],
        })
    );
    let intent: Value = serde_json::from_slice(intent_file.bytes()).unwrap();
    assert_eq!(intent["step"], "in_place_identity_markers");
    assert_eq!(intent["markers"], json!(["human_rights_v2"]));
    assert_eq!(intent["migration_id"], json!(plan.migration_id));
    assert_eq!(
        intent["source_prefix"],
        json!({"seq": initial.head.0, "hash": initial.head.1})
    );
}

#[test]
fn operator_resume_preserves_an_unpublished_backup_and_confirms_one_marker() {
    let adopted = adopted(14);
    let initial = maintenance_source_state(&adopted.base);
    let neighbor = store_source_state(&adopted.base.stores[1]);
    let directory = adopted
        .base
        .root
        .exclusive_child("interrupted-backup")
        .unwrap();
    let journal = directory.exclusive_child("journal").unwrap();
    let backups = directory.exclusive_child("backups").unwrap();
    let configuration = adopted.configuration_dir();
    let plan = HumanSourcesMaintenancePlanV1::in_place(
        Uuid::new_v4(),
        &adopted.closure,
        binding(&configuration, &adopted.manifest_name),
        adopted.source_id(0),
    );
    let plan_bytes = serde_json::to_vec(&plan).unwrap();
    let plan_file = directory
        .persistent_create("plan.json", &plan_bytes)
        .unwrap();
    plan_file.persistent_sync().unwrap();
    directory.sync().unwrap();

    // Persist a real intent and SQLite copy, stopping before the backup receipt.
    let unpublished = backups.path().join("source.sqlite");
    {
        let guard = adopted.acquire(&plan, &plan_file).unwrap();
        let token = guard.target_source(adopted.source_id(0)).unwrap();
        let opened = adopted.base.open_maintenance(0, &token);
        opened
            .with_review_maintenance_writer(&token, |writer| {
                RetainedSourceIntent::prepare_identity_markers(
                    &token,
                    writer,
                    journal.clone(),
                    backups.clone(),
                )?;
                writer.prepare_identity_markers_backup(&token, &unpublished)?;
                Ok(())
            })
            .unwrap();
    }
    let retained_copy = std::fs::read(&unpublished).unwrap();
    let retained_inode = backups
        .private_entry(OsStr::new("source.sqlite"))
        .unwrap()
        .unwrap();
    assert_eq!(journal_names(&journal), ["identity-markers-intent-v1"]);
    assert_eq!(maintenance_source_state(&adopted.base), initial);

    let resume = || {
        install_owned_identity_markers(OwnedIdentityMarkersRequest {
            manifest: adopted.manifest(),
            configuration: adopted.closure.configuration.clone(),
            files: adopted.base.configuration_files.clone(),
            tenant_id: "A".into(),
            redactor: Redactor::new(vec![]),
            max_events: 10_000,
            maintenance_directory: directory.path().to_owned(),
            excluded_directories: vec![configuration.path().to_owned()],
            resume: true,
        })
    };

    // A non-private backup directory and a full inventory are distinguishable
    // before resume creates a copy or commits a marker.
    let private_permissions = std::fs::metadata(backups.path()).unwrap().permissions();
    std::fs::set_permissions(backups.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let directory_error = resume().unwrap_err();
    std::fs::set_permissions(backups.path(), private_permissions).unwrap();
    assert_eq!(
        directory_error.downcast_ref::<OwnedIdentityMarkersRefusalV1>(),
        Some(&OwnedIdentityMarkersRefusalV1::BackupDirectoryInvalid)
    );

    let extra_names: Vec<_> = (journal_names(&backups).len()..28)
        .map(|number| format!("inventory-fixture-{number}"))
        .collect();
    for name in &extra_names {
        backups.persistent_create(name, b"test entry").unwrap();
    }
    let inventory_error = resume().unwrap_err();
    assert_eq!(
        inventory_error.downcast_ref::<OwnedIdentityMarkersRefusalV1>(),
        Some(&OwnedIdentityMarkersRefusalV1::BackupInventoryLimit)
    );
    assert_eq!(backups.persistent_entries(28).unwrap().len(), 28);
    assert_eq!(journal_names(&journal), ["identity-markers-intent-v1"]);
    assert_eq!(maintenance_source_state(&adopted.base), initial);
    let extras = adopted
        .base
        .root
        .exclusive_child("retained-fixtures")
        .unwrap();
    for name in &extra_names {
        std::fs::rename(backups.path().join(name), extras.path().join(name)).unwrap();
    }

    let installed = resume().unwrap();
    assert_eq!(installed.state, "installed");
    assert_eq!(installed.backup_path.parent(), Some(backups.path()));
    assert_ne!(installed.backup_path, unpublished);
    assert!(installed.backup_path.is_file());
    assert_eq!(installed.markers.len(), 1);
    assert_eq!(installed.markers[0].seq, initial.head.0 + 1);
    assert_eq!(marker_rows(&adopted.base.stores[0]).len(), 1);
    assert_eq!(std::fs::read(&unpublished).unwrap(), retained_copy);
    assert_eq!(
        backups.private_entry(OsStr::new("source.sqlite")).unwrap(),
        Some(retained_inode)
    );
    assert_eq!(std::fs::read(plan_file.path()).unwrap(), plan_bytes);
    assert_eq!(store_source_state(&adopted.base.stores[1]), neighbor);

    let backup_names = journal_names(&backups);
    let confirmed = resume().unwrap();
    assert_eq!(confirmed.state, "confirmed");
    assert_eq!(confirmed.backup_path, installed.backup_path);
    assert_eq!(confirmed.markers, installed.markers);
    assert_eq!(journal_names(&backups), backup_names);
    assert_eq!(marker_rows(&adopted.base.stores[0]).len(), 1);

    // Losing the chosen copy after publication must not select another one.
    std::fs::rename(
        &installed.backup_path,
        installed.backup_path.with_extension("retained"),
    )
    .unwrap();
    let before_refusal = journal_names(&backups);
    let error = resume().unwrap_err();
    assert_eq!(
        error.downcast_ref::<OwnedIdentityMarkersRefusalV1>(),
        Some(&OwnedIdentityMarkersRefusalV1::JournalConflict)
    );
    assert_eq!(journal_names(&backups), before_refusal);
    assert_eq!(std::fs::read(&unpublished).unwrap(), retained_copy);
    assert_eq!(marker_rows(&adopted.base.stores[0]).len(), 1);
}

#[test]
fn refusals_leave_the_store_and_journal_unchanged() {
    let adopted = adopted(14);
    let (_, guard) = adopted.in_place(0);
    install_all(&adopted, &guard, "first");
    drop(guard);
    let installed = maintenance_source_state(&adopted.base);

    // Already installed: a new maintenance is refused before any record.
    let (_, guard) = adopted.in_place(0);
    let token = guard.target_source(adopted.source_id(0)).unwrap();
    let (journal, backups) = adopted.directories("again");
    let opened = adopted.base.open_maintenance(0, &token);
    opened
        .with_review_maintenance_writer(&token, |writer| {
            let refused = RetainedSourceIntent::prepare_identity_markers(
                &token,
                writer,
                journal.clone(),
                backups.clone(),
            )
            .map(|_| ())
            .unwrap_err();
            assert_eq!(
                refused.downcast_ref::<IdentityMarkersFailure>(),
                Some(&IdentityMarkersFailure::AlreadyInstalled)
            );
            assert!(
                writer
                    .prepare_identity_markers_backup(&token, &backups.path().join("source.sqlite"))
                    .is_err()
            );
            Ok(())
        })
        .unwrap();
    assert!(journal_names(&journal).is_empty());
    assert!(!backups.path().join("source.sqlite").exists());
    assert_eq!(maintenance_source_state(&adopted.base), installed);
    drop(opened);
    drop(token);
    drop(guard);

    // On source B (still pending): occupied journal, foreign backup frame,
    // changed backup directory and an unframed backup are all refused.
    let (_, guard) = adopted.in_place(1);
    let token = guard.target_source(adopted.source_id(1)).unwrap();
    let neighbor = store_source_state(&adopted.base.stores[1]);
    let opened = adopted.base.open_maintenance(1, &token);
    opened
        .with_review_maintenance_writer(&token, |writer| {
            let (journal, backups) = adopted.directories("b");
            journal.persistent_child("unrelated", true)?;
            assert!(
                RetainedSourceIntent::prepare_identity_markers(
                    &token,
                    writer,
                    journal.clone(),
                    backups.clone()
                )
                .is_err()
            );
            let (journal, backups) = adopted.directories("b-unframed");
            let intent = RetainedSourceIntent::prepare_identity_markers(
                &token,
                writer,
                journal.clone(),
                backups.clone(),
            )?;
            let other = adopted.directories("b-other").1;
            let moved = RetainedSourceIntent::resume_identity_markers(
                &token,
                writer,
                journal.clone(),
                other,
            )
            .map(|_| ())
            .unwrap_err();
            assert_eq!(moved.to_string(), "identity marker intent differs");
            // A backup without its journal frame is never adopted.
            writer.prepare_identity_markers_backup(&token, &intent.backup_path()?)?;
            assert!(intent.resume_backup(writer).is_err());
            Ok(())
        })
        .unwrap();
    assert_eq!(store_source_state(&adopted.base.stores[1]), neighbor);
    drop(opened);
    drop(token);
    drop(guard);

    // A substituted backup copy no longer matches its frame at confirmation.
    let (_, guard) = adopted.in_place(1);
    let token = guard.target_source(adopted.source_id(1)).unwrap();
    let (journal, backups) = adopted.directories("b-swap");
    let copy = adopted
        .directories("b-swap-copy")
        .1
        .path()
        .join("copy.sqlite");
    let opened = adopted.base.open_maintenance(1, &token);
    let intent = opened
        .with_review_maintenance_writer(&token, |writer| {
            let intent =
                RetainedSourceIntent::prepare_identity_markers(&token, writer, journal, backups)?;
            let backup = intent.prepare_backup(writer)?;
            writer.prepare_identity_markers_backup(&token, &copy)?;
            backup.install_identity_markers(writer)?;
            drop(backup);
            writer.commit()?;
            Ok(intent)
        })
        .unwrap();
    drop(opened);
    let committed = store_source_state(&adopted.base.stores[1]);
    let path = intent.backup_path().unwrap();
    std::fs::rename(&path, path.with_extension("original")).unwrap();
    std::fs::rename(&copy, &path).unwrap();
    assert!(
        intent
            .confirm_identity_markers(Redactor::new(vec![]), 10_000)
            .is_err()
    );
    assert_eq!(store_source_state(&adopted.base.stores[1]), committed);
    assert_eq!(marker_rows(&adopted.base.stores[1]).len(), 1);
}

#[test]
fn in_place_tokens_and_egress_or_adoption_methods_never_cross() {
    // Store14: the in-place token cannot drive egress or 13→14 adoption.
    let adopted14 = adopted(14);
    let (_, guard) = adopted14.in_place(0);
    let token = guard.target_source(adopted14.source_id(0)).unwrap();
    let before = maintenance_source_state(&adopted14.base);
    let (journal, backups) = adopted14.directories("cross");
    let opened = adopted14.base.open_maintenance(0, &token);
    let limits = EgressStoreLimitsV1::default();
    opened
        .with_review_maintenance_writer(&token, |writer| {
            let path = backups.path().join("source.sqlite");
            assert!(writer.egress_maintenance_prefix(&token, true).is_err());
            assert!(writer.prepare_egress_backup(&token, &path).is_err());
            assert!(writer.resume_egress_backup(&token, &path).is_err());
            assert!(writer.prepare_backup(&token, &path).is_err());
            assert!(writer.resume_backup(&token, &path).is_err());
            assert!(writer.inspect().is_err());
            assert!(
                RetainedSourceIntent::prepare(
                    &token,
                    writer,
                    journal.clone(),
                    backups.clone(),
                    &limits
                )
                .is_err()
            );
            // An in-place proof is refused by the egress installer.
            let proof = writer
                .prepare_identity_markers_backup(&token, &backups.path().join("proof.sqlite"))?;
            assert!(
                writer
                    .install_egress_format15(&token, &proof, &limits)
                    .is_err()
            );
            Ok(())
        })
        .unwrap();
    assert!(journal_names(&journal).is_empty());
    assert_eq!(maintenance_source_state(&adopted14.base), before);
    drop(opened);
    drop(token);
    drop(guard);

    // The egress 14→15 token cannot drive the identity-marker step.
    let mut plan = adopted14.base.plan.clone();
    plan.migration_id = Uuid::new_v4();
    plan.prior = Some(plan.target.clone());
    let configuration = adopted14.configuration_dir();
    plan.prior_active_manifest = Some(HumanMaintenanceActiveManifestV1 {
        storage_binding: binding(&configuration, &adopted14.manifest_name),
        closure_id: adopted14.closure.closure_id,
        closure_revision: adopted14.closure.closure_revision,
        closure_digest: adopted14.closure.closure_digest.clone(),
    });
    plan.target.sources[0].required_format = 15;
    let file = configuration
        .persistent_create(
            &format!("egress-cross-{}.json", plan.migration_id),
            &serde_json::to_vec(&plan).unwrap(),
        )
        .unwrap();
    let resolve = |view: &HumanMaintenanceViewV1| {
        HumanMaintenanceResolvedViewV1::new(
            view.sources.clone(),
            view.configuration.clone(),
            adopted14.base.configuration_files.clone(),
        )
        .unwrap()
    };
    let guard = HumanSourcesMaintenanceGuard::acquire(
        file,
        HumanSourcesMaintenanceConfigurationV1::new(
            Some(resolve(plan.prior.as_ref().unwrap())),
            resolve(&plan.target),
            Some(adopted14.manifest()),
        ),
    )
    .unwrap();
    let token = guard.target_source(adopted14.source_id(0)).unwrap();
    assert!(token.in_place_anchor().unwrap().is_none());
    let opened = adopted14.base.open_maintenance(0, &token);
    let (journal, backups) = adopted14.directories("egress-token");
    opened
        .with_review_maintenance_writer(&token, |writer| {
            let path = backups.path().join("source.sqlite");
            assert!(
                writer
                    .identity_markers_maintenance_prefix(&token, true)
                    .is_err()
            );
            assert!(
                writer
                    .prepare_identity_markers_backup(&token, &path)
                    .is_err()
            );
            assert!(
                RetainedSourceIntent::prepare_identity_markers(
                    &token,
                    writer,
                    journal.clone(),
                    backups.clone()
                )
                .is_err()
            );
            Ok(())
        })
        .unwrap();
    assert!(journal_names(&journal).is_empty());
    assert_eq!(maintenance_source_state(&adopted14.base), before);
    drop(opened);
    drop(token);
    drop(guard);

    // Store15: the in-place token cannot resume or confirm the egress step,
    // and an in-place plan at the wrong format cannot open the source.
    let adopted15 = adopted(15);
    let (_, guard) = adopted15.in_place(0);
    let token = guard.target_source(adopted15.source_id(0)).unwrap();
    let before = maintenance_source_state(&adopted15.base);
    let opened = adopted15.base.open_maintenance(0, &token);
    opened
        .with_review_maintenance_writer(&token, |writer| {
            let path = adopted15
                .base
                .root
                .path()
                .join("egress-backups/egress-0.sqlite");
            assert!(writer.egress_maintenance_prefix(&token, false).is_err());
            assert!(writer.resume_egress_backup(&token, &path).is_err());
            Ok(())
        })
        .unwrap();
    drop(opened);
    assert!(
        Store::confirm_egress_format15(
            &token,
            Redactor::new(vec![]),
            10_000,
            &adopted15
                .base
                .root
                .path()
                .join("egress-backups/egress-0.sqlite"),
            &limits,
        )
        .is_err()
    );
    drop(token);
    drop(guard);
    assert_eq!(maintenance_source_state(&adopted15.base), before);
    // An in-place plan built from an outdated Store14 closure of the same
    // union cannot open the Store15 source.
    let Adopted { base, anchors, .. } = adopted15;
    let stale_owner = base.owner(&anchors, "stale14.json", 14);
    let closure = stale_owner.closure().clone();
    drop(stale_owner);
    let stale = Adopted {
        base,
        anchors,
        closure,
        manifest_name: "stale14.json".into(),
    };
    let (_, guard) = stale.in_place(0);
    let token = guard.target_source(stale.source_id(0)).unwrap();
    let source = &stale.base.plan.target.sources[0];
    assert!(
        Store::open_review_maintenance_for_tenant(
            &source.storage_binding.path(),
            "A",
            Redactor::new(vec![]),
            10_000,
            &token,
        )
        .is_err()
    );
    assert_eq!(maintenance_source_state(&stale.base), before);
}
