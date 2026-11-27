//! In-place identity markers on adopted owned sources (Store14/15): real
//! private SQLite, the real maintenance guard, journal and verified backup.
use super::*;
use crate::audit::identity_markers::{IdentityMarkersFailure, InPlaceIdentityMarkerV1};
use crate::control::session_control::{
    ProvisionSessionLaunchTargetV1, SessionLaunchPrepareRequestV1, SessionLaunchTargetRecordV1,
};
use crate::control_fs::publication::FAIL_AFTER_RENAME;
use crate::human_authority::migration::egress_journal::RetainedSourceIntent;
use crate::human_authority::migration::operator::{
    OwnedIdentityMarkersRefusalV1, OwnedIdentityMarkersReportV1, OwnedIdentityMarkersRequest,
    install_owned_identity_markers,
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
                    // Unique per provisioning: the credential digest is unique
                    // across the source, so several test principals may coexist.
                    sha256_digest: format!(
                        "{}{}",
                        Uuid::new_v4().simple(),
                        Uuid::new_v4().simple()
                    ),
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
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[0].marker, InPlaceIdentityMarkerV1::HumanRightsV2);
    assert_eq!(receipts[0].seq, before.head.0 + 1);
    assert_eq!(
        receipts[1].marker,
        InPlaceIdentityMarkerV1::SessionPrepareV1
    );
    assert_eq!(receipts[1].seq, before.head.0 + 2);
    let after = maintenance_source_state(&adopted.base);
    assert_eq!(after.format, i64::from(format));
    assert_eq!(after.head, (receipts[1].seq, receipts[1].hash.clone()));
    // SessionPrepareV1 installs the preparation and conversation tables and
    // their primary key autoindexes. Preserve the format and prior objects.
    let mut expected_schema = before.schema.clone();
    for (_, name, sql) in crate::audit::session_prepare::SCHEMA
        .into_iter()
        .chain(crate::audit::session_message::SCHEMA)
    {
        expected_schema.push(("table".into(), name.into(), name.into(), Some(sql.into())));
        expected_schema.push((
            "index".into(),
            format!("sqlite_autoindex_{name}_1"),
            name.into(),
            None,
        ));
    }
    expected_schema.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
    assert_eq!(after.schema, expected_schema);
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
fn lost_acknowledgements_resume_to_exactly_the_committed_markers() {
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
                [
                    InPlaceIdentityMarkerV1::HumanRightsV2,
                    InPlaceIdentityMarkerV1::SessionPrepareV1
                ]
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
    assert_eq!(committed.head.0, initial.head.0 + 2);
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
            "receipts": [
                {"marker": "human_rights_v2", "seq": applied[0].seq, "hash": applied[0].hash},
                {"marker": "session_prepare_v1", "seq": applied[1].seq, "hash": applied[1].hash}
            ],
        })
    );
    let intent: Value = serde_json::from_slice(intent_file.bytes()).unwrap();
    assert_eq!(intent["step"], "in_place_identity_markers");
    assert_eq!(
        intent["markers"],
        json!(["human_rights_v2", "session_prepare_v1"])
    );
    assert_eq!(intent["migration_id"], json!(plan.migration_id));
    assert_eq!(
        intent["source_prefix"],
        json!({"seq": initial.head.0, "hash": initial.head.1})
    );
}

#[test]
fn operator_resume_preserves_an_unpublished_backup_and_confirms_the_markers() {
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
    assert_eq!(installed.markers.len(), 2);
    assert_eq!(installed.markers[0].seq, initial.head.0 + 1);
    assert_eq!(installed.markers[1].seq, initial.head.0 + 2);
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

// ---------------------------------------------------------------------------
// G04 `session_prepare_v1`: durable nominative launch preparation on the
// adopted owned source. Real private SQLite, the real maintenance and the
// real Store transactions; no grant, reservation or claim may appear.
// ---------------------------------------------------------------------------

const PREPARE_MARKER_KIND: &str = "control.session_prepare.genesis";
const PREPARED_KIND: &str = "control.session_prepare.prepared";
const FUTURE_DEADLINE: &str = "2126-01-01T00:00:00Z";
const PAST_DEADLINE: &str = "2000-01-01T00:00:00Z";

// e3c9f82f added this table to the three in 9ba8b9b6 without changing
// session_prepare_v1. Keep the historical DDL independent of later installers.
const HISTORICAL_LAUNCHES: &str = "CREATE TABLE session_launches(operation_id TEXT PRIMARY KEY,input_hash TEXT NOT NULL,session_id TEXT NOT NULL,preparation_operation_id TEXT NOT NULL,event_seq INTEGER NOT NULL REFERENCES events(seq),event_hash TEXT NOT NULL,receipt_json TEXT NOT NULL)";

fn seed_session_prepare(adopted: &Adopted) {
    let (plan, _guard) = adopted.in_place(0);
    let store = &adopted.base.stores[0];
    let mut db = store.db.lock().unwrap();
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    InPlaceIdentityMarkerV1::HumanRightsV2
        .install(store, &tx, plan.migration_id)
        .unwrap();
    InPlaceIdentityMarkerV1::SessionPrepareV1
        .install(store, &tx, plan.migration_id)
        .unwrap();
    tx.commit().unwrap();
}

fn leave_historical_session_catalog(store: &Store, with_launches: bool) {
    let mut db = store.db.lock().unwrap();
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    tx.execute_batch("DROP TABLE session_messages; DROP TABLE session_conversations;")
        .unwrap();
    if !with_launches {
        assert_eq!(
            tx.query_row("SELECT COUNT(*) FROM session_launches", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        tx.execute_batch("DROP TABLE session_launches;").unwrap();
    }
    tx.commit().unwrap();
}

#[test]
fn historical_session_prepare_catalogs_enter_owned_maintenance() {
    let [(_, _, conversations), (_, _, messages)] = crate::audit::session_message::SCHEMA;
    let mut refused_history = Vec::new();
    for format in [14, 15] {
        let adopted = adopted(format);
        seed_session_prepare(&adopted);
        let store = &adopted.base.stores[0];
        leave_historical_session_catalog(store, false);
        let (_, guard) = adopted.in_place(0);
        let token = guard.target_source(adopted.source_id(0)).unwrap();
        let source = token.descriptor();
        let path = source.storage_binding.path();

        let wrong_launch_definition =
            HISTORICAL_LAUNCHES.replace("event_hash TEXT NOT NULL", "event_hash TEXT");
        let historical_launch_definition =
            format!("DROP TABLE session_launches; {HISTORICAL_LAUNCHES}");
        let messages_without_launch = format!("{messages}; DROP TABLE session_launches;");
        for (catalog, ddl, must_open) in [
            ("9ba8b9b6", "", true),
            (
                "wrong launch definition",
                wrong_launch_definition.as_str(),
                false,
            ),
            ("e3c9f82f", historical_launch_definition.as_str(), true),
            ("partial message family", conversations, false),
            (
                "message family without launch",
                messages_without_launch.as_str(),
                false,
            ),
        ] {
            store.db.lock().unwrap().execute_batch(ddl).unwrap();
            let before = maintenance_source_state(&adopted.base);
            let result = Store::open_review_maintenance_for_tenant(
                &path,
                source.tenant_id.as_deref().unwrap(),
                Redactor::new(vec![]),
                10_000,
                &token,
            )
            .map(drop);
            assert_eq!(maintenance_source_state(&adopted.base), before);
            match (must_open, result) {
                (true, Err(error)) => {
                    refused_history.push(format!("Store{format} {catalog}: {error:#}"));
                }
                (false, Ok(())) => panic!("Store{format} admitted {catalog}"),
                _ => {}
            }
        }
    }
    assert!(refused_history.is_empty(), "{}", refused_history.join("\n"));
}

fn historical_session_work(
    retained: &Retained,
    with_launches: bool,
) -> (
    crate::control::session_control::SessionPrepareReceiptV1,
    Option<crate::control::session_control::SessionLaunchReceiptV1>,
) {
    use crate::identity::{ContextModeV1, HumanCapabilityV1};
    let (principal, membership) = provision_membership(
        retained,
        &[
            HumanCapabilityV1::LaunchSession,
            HumanCapabilityV1::ReadTenantMetadata,
        ],
    )
    .unwrap();
    let scope = personal_scope(retained, &principal, &membership, ContextModeV1::Personal);
    let target = Uuid::new_v4();
    let epoch = retained.owner.try_write_epoch().unwrap();
    retained
        .store
        .provision_session_launch_target_scoped(
            &epoch,
            &retained.source,
            &target_command(Uuid::new_v4(), target, 1, 0, &principal),
        )
        .unwrap();
    drop(epoch);
    let preparation = prepare(
        retained,
        &scope,
        &prepare_request(Uuid::new_v4(), target, 1, FUTURE_DEADLINE),
    )
    .unwrap();
    let launch = with_launches.then(|| {
        let launch = launch(
            retained,
            &scope,
            &launch_request(Uuid::new_v4(), target, 1, &preparation),
        )
        .unwrap();
        // Earlier work was valid at revision 1. The retained target must
        // reflect the last event, including the now-inactive revision 2.
        let mut deactivate = target_command(Uuid::new_v4(), target, 2, 1, &principal);
        deactivate.target.active = false;
        let epoch = retained.owner.try_write_epoch().unwrap();
        retained
            .store
            .provision_session_launch_target_scoped(&epoch, &retained.source, &deactivate)
            .unwrap();
        launch
    });
    (preparation, launch)
}

fn run_historical_maintenance(
    adopted: &Adopted,
    name: &str,
    resume: bool,
) -> anyhow::Result<OwnedIdentityMarkersReportV1> {
    install_owned_identity_markers(OwnedIdentityMarkersRequest {
        manifest: adopted.manifest(),
        configuration: adopted.closure.configuration.clone(),
        files: adopted.base.configuration_files.clone(),
        tenant_id: "A".into(),
        redactor: Redactor::new(vec![]),
        max_events: 10_000,
        maintenance_directory: adopted.base.root.path().join(name),
        excluded_directories: vec![adopted.configuration_dir().path().to_owned()],
        resume,
    })
}

#[test]
fn historical_session_prepare_refuses_a_lost_projection_before_backup() {
    let adopted = adopted(14);
    seed_session_prepare(&adopted);
    let retained = adopted.retained(0, "before-lost-projection.json", 14);
    let (preparation, _) = historical_session_work(&retained, false);
    drop(retained);
    let store = &adopted.base.stores[0];
    leave_historical_session_catalog(store, false);
    let original = maintenance_source_state(&adopted.base);
    {
        let db = store.db.lock().unwrap();
        assert_eq!(
            db.execute(
                "DELETE FROM session_preparations WHERE operation_id=?1",
                [preparation.operation_id.to_string()],
            )
            .unwrap(),
            1
        );
        assert!(Store::check_integrity(&db).unwrap().valid);
    }
    assert_eq!(maintenance_source_state(&adopted.base), original);

    let error = run_historical_maintenance(&adopted, "lost-preparation", false).unwrap_err();
    assert_eq!(
        error.downcast_ref::<OwnedIdentityMarkersRefusalV1>(),
        Some(&OwnedIdentityMarkersRefusalV1::FormatUnsupported)
    );
    assert!(format!("{error:#}").contains("historical session preparation projection is missing"));
    let maintenance = adopted.base.root.path().join("lost-preparation");
    for child in ["journal", "backups"] {
        let directory = Directory::open_existing(&maintenance.join(child)).unwrap();
        assert!(journal_names(&directory).is_empty());
    }
    assert_eq!(maintenance_source_state(&adopted.base), original);
    assert_eq!(table_count(store, "session_preparations"), 0);
}

fn historical_session_rows(
    db: &rusqlite::Connection,
    preparation: Uuid,
    launch: Option<Uuid>,
) -> (String, Option<String>) {
    let preparation = db
        .query_row(
            "SELECT json_array(operation_id,input_hash,principal_key,membership_id,organization_scope,
                               launch_target_id,target_revision,expires_at,event_seq,event_hash)
             FROM session_preparations WHERE operation_id=?1",
            [preparation.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let launch = launch.map(|launch| {
        db.query_row(
            "SELECT json_array(operation_id,input_hash,session_id,preparation_operation_id,
                               event_seq,event_hash,receipt_json)
             FROM session_launches WHERE operation_id=?1",
            [launch.to_string()],
            |row| row.get(0),
        )
        .unwrap()
    });
    (preparation, launch)
}

#[test]
fn historical_session_prepare_catalogs_upgrade_through_owned_operator() {
    let mut refused_history = Vec::new();
    for format in [14, 15] {
        for (catalog, with_launches) in [("9ba8b9b6", false), ("e3c9f82f", true)] {
            let adopted = adopted(format);
            seed_session_prepare(&adopted);
            let store = &adopted.base.stores[0];
            // Both generations used these Prepare/Launch producers and audit
            // payloads. Only the projection tables introduced later are lost
            // from this private fixture, before any conversation effect.
            let retained = adopted.retained(0, "before-historical-fixture.json", format);
            let (preparation, launch) = historical_session_work(&retained, with_launches);
            drop(retained);
            let launch_id = launch.as_ref().map(|launch| launch.operation_id);
            let original = historical_session_rows(
                &store.db.lock().unwrap(),
                preparation.operation_id,
                launch_id,
            );
            leave_historical_session_catalog(store, with_launches);
            let before = maintenance_source_state(&adopted.base);
            let open_owned = |manifest| {
                let owner = adopted.base.owner(&adopted.anchors, manifest, format);
                let source = owner.source_token(adopted.source_id(0)).unwrap();
                Store::open_review_startup_for_tenant(
                    &source.descriptor().storage_binding.path(),
                    "A",
                    Redactor::new(vec![]),
                    10_000,
                    &source,
                )
                .map(drop)
            };
            assert!(open_owned("before-historical-maintenance.json").is_err());
            assert_eq!(maintenance_source_state(&adopted.base), before);

            let run = |resume| run_historical_maintenance(&adopted, "historical-upgrade", resume);
            let installed = match run(false) {
                Ok(report) => report,
                Err(error) => {
                    assert_eq!(maintenance_source_state(&adopted.base), before);
                    refused_history.push(format!("Store{format} {catalog}: {error:#}"));
                    continue;
                }
            };
            assert_eq!(installed.state, "installed");
            assert_eq!(installed.store_format, format);
            assert_eq!(installed.source_id, adopted.source_id(0));
            let [extension] = installed.markers.as_slice() else {
                panic!("historical upgrade should append exactly one marker");
            };
            assert_eq!(extension.marker, "session_prepare_catalog_upgrade_v1");
            assert_eq!(extension.seq, before.head.0 + 1);

            let backup = rusqlite::Connection::open_with_flags(
                &installed.backup_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            assert_eq!(audit_head(&backup).unwrap(), before.head);
            assert!(Store::check_integrity(&backup).unwrap().valid);
            assert_eq!(
                historical_session_rows(&backup, preparation.operation_id, launch_id),
                original
            );
            drop(backup);
            {
                let db = store.db.lock().unwrap();
                let previous_hash: String = db
                    .query_row(
                        "SELECT prev_hash FROM events WHERE seq=?1",
                        [extension.seq],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(previous_hash, before.head.1);
                assert_eq!(
                    historical_session_rows(&db, preparation.operation_id, launch_id),
                    original
                );
                if let Some(launch) = &launch {
                    let (binding, seq, hash): (String, i64, String) = db
                        .query_row(
                            "SELECT binding_json,event_seq,event_hash FROM session_conversations WHERE session_id=?1",
                            [&launch.session_id],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                        )
                        .unwrap();
                    assert_eq!(
                        binding,
                        serde_json::to_string(&launch.conversation).unwrap()
                    );
                    assert_eq!(seq, launch.audit_seq);
                    assert_eq!(hash, launch.audit_hash);
                }
            }

            open_owned("after-historical-maintenance.json").unwrap();
            let upgraded = maintenance_source_state(&adopted.base);
            assert_eq!(upgraded.head, (extension.seq, extension.hash.clone()));
            assert_eq!(upgraded.format, before.format);
            assert_eq!(
                table_count(store, "session_conversations"),
                i64::from(with_launches)
            );
            assert_eq!(table_count(store, "session_messages"), 0);

            if with_launches && format == 15 {
                let retained = adopted.retained(0, "after-upgrade-new-work.json", format);
                let _ = historical_session_work(&retained, false);
            }
            let before_confirmation = maintenance_source_state(&adopted.base);
            let confirmed = run(true).unwrap();
            assert_eq!(confirmed.state, "confirmed");
            assert_eq!(confirmed.backup_path, installed.backup_path);
            assert_eq!(confirmed.markers, installed.markers);
            assert_eq!(maintenance_source_state(&adopted.base), before_confirmation);

            if with_launches && format == 14 {
                // The audit extension now proves that these tables existed.
                // Their later loss must refuse even with the old SQL shape.
                leave_historical_session_catalog(store, true);
                let damaged = maintenance_source_state(&adopted.base);
                assert!(run(true).is_err());
                assert_eq!(maintenance_source_state(&adopted.base), damaged);
            } else if let Some(launch) = launch {
                // The same backup, with the same catalog and audit prefix,
                // is unusable on confirmation after a causal receipt diverges.
                let backup = rusqlite::Connection::open_with_flags(
                    &installed.backup_path,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
                )
                .unwrap();
                assert_eq!(
                    backup
                        .execute(
                            "UPDATE session_launches SET receipt_json=json_set(receipt_json,'$.grant_revision',2) WHERE operation_id=?1",
                            [launch.operation_id.to_string()],
                        )
                        .unwrap(),
                    1
                );
                assert_eq!(audit_head(&backup).unwrap(), before.head);
                assert!(Store::check_integrity(&backup).unwrap().valid);
                drop(backup);
                let error = run(true).unwrap_err();
                assert!(format!("{error:#}").contains("historical session launch grant differs"));
                assert_eq!(maintenance_source_state(&adopted.base), before_confirmation);
            }
        }
    }
    assert!(refused_history.is_empty(), "{}", refused_history.join("\n"));
}

/// Trusted offline provisioning of one human membership carrying `capability`.
/// Returns the principal and the committed membership for scope construction.
fn provision_membership(
    retained: &Retained,
    capabilities: &[crate::identity::HumanCapabilityV1],
) -> anyhow::Result<(
    crate::identity::PrincipalRefV1,
    crate::identity::MembershipV1,
)> {
    use crate::identity::*;
    let principal = PrincipalRefV1::HumanLocal {
        installation_id: retained.source.descriptor().installation_id.to_string(),
        account_id: Uuid::new_v4().to_string(),
    };
    let membership = MembershipV1 {
        version: 1,
        membership_id: Uuid::new_v4().to_string(),
        principal: principal.clone(),
        tenant_id: "A".into(),
        organization_scope: "org-a".into(),
        revision: 1,
        active: true,
        capabilities: capabilities.iter().copied().collect(),
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
                    display_name: "Synthetic launcher".into(),
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
                    // Unique per provisioning: the credential digest is unique
                    // across the source, so several test principals may coexist.
                    sha256_digest: format!(
                        "{}{}",
                        Uuid::new_v4().simple(),
                        Uuid::new_v4().simple()
                    ),
                    revision: 1,
                    active: true,
                    expires_at: None,
                },
            }),
            mutation(IdentityChangeV1::Membership {
                expected_revision: 0,
                membership: membership.clone(),
            }),
        ],
    };
    let epoch = retained.owner.try_write_epoch()?;
    retained
        .store
        .provision_local_human_scoped(&epoch, &retained.source, &provision)?;
    Ok((principal, membership))
}

/// Deactivate the account, credential and membership of a prior provisioning.
fn revoke_membership(
    retained: &Retained,
    principal: &crate::identity::PrincipalRefV1,
    membership: &crate::identity::MembershipV1,
) -> anyhow::Result<()> {
    use crate::identity::*;
    let _ = principal;
    let mut revoked = membership.clone();
    revoked.revision = membership.revision + 1;
    revoked.active = false;
    // Deactivating the membership closes the personal launch context; the
    // individual mutation path advances the existing record (the triple
    // provisioning entry point only admits fresh revision-1 records).
    let mutation = IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "synthetic-offline-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change: IdentityChangeV1::Membership {
            expected_revision: membership.revision,
            membership: revoked,
        },
    };
    let epoch = retained.owner.try_write_epoch()?;
    retained
        .store
        .mutate_identity_scoped(&epoch, &retained.source, &mutation)?;
    Ok(())
}

/// A personal scope bound to the source's current authorization revision.
fn personal_scope(
    retained: &Retained,
    principal: &crate::identity::PrincipalRefV1,
    membership: &crate::identity::MembershipV1,
    mode: crate::identity::ContextModeV1,
) -> crate::identity::AuthorizedScopeV1 {
    let revision = {
        let db = retained.store.db.lock().unwrap();
        crate::audit::identity::authorization_revision(&db).unwrap()
    };
    crate::identity::authorize_context(principal, membership, mode, membership.revision)
        .unwrap()
        .at_authorization_revision(revision)
}

/// The discovery generation the gateway would pass, read under a read epoch.
fn generation(retained: &Retained) -> crate::audit::human_modern::HumanDiscoveryGeneration {
    let epoch = retained.owner.try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &retained.source).unwrap();
    let mut db = retained.store.db.lock().unwrap();
    let tx = retained
        .store
        .begin_checked_scoped(&mut db, &access, StoreOperationClass::Business)
        .unwrap();
    let generation = crate::audit::human_modern::discovery_generation(&tx).unwrap();
    drop(tx);
    generation
}

fn target_command(
    operation_id: Uuid,
    launch_target_id: Uuid,
    revision: u64,
    expected_revision: u64,
    launcher: &crate::identity::PrincipalRefV1,
) -> ProvisionSessionLaunchTargetV1 {
    ProvisionSessionLaunchTargetV1 {
        version: 1,
        operation_id,
        expected_revision,
        target: SessionLaunchTargetRecordV1 {
            version: 1,
            launch_target_id,
            revision,
            tenant_id: "A".into(),
            organization_scope: "org-a".into(),
            active: true,
            launchers: vec![launcher.clone()],
        },
    }
}

fn prepare_request(
    operation_id: Uuid,
    launch_target_id: Uuid,
    expected_target_revision: u64,
    expires_at: &str,
) -> SessionLaunchPrepareRequestV1 {
    SessionLaunchPrepareRequestV1 {
        version: 1,
        operation_id,
        launch_target_id,
        expected_target_revision,
        expires_at: expires_at.into(),
    }
}

fn prepare(
    retained: &Retained,
    scope: &crate::identity::AuthorizedScopeV1,
    request: &SessionLaunchPrepareRequestV1,
) -> anyhow::Result<crate::control::session_control::SessionPrepareReceiptV1> {
    let expected = generation(retained);
    let epoch = retained.owner.try_write_epoch()?;
    retained
        .store
        .modern_session_launch_prepare(&epoch, &retained.source, scope, request, expected)
        .map(|(receipt, _)| receipt)
}

fn identity_failure(error: &anyhow::Error) -> Option<crate::identity::IdentityFailureV1> {
    error
        .downcast_ref::<crate::identity::IdentityFailureV1>()
        .copied()
}

fn table_count(store: &Store, table: &str) -> i64 {
    let db = store.db.lock().unwrap();
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn head(store: &Store) -> (i64, String) {
    let db = store.db.lock().unwrap();
    audit_head(&db).unwrap()
}

/// The complete Prepare contract on one adopted owned format: durable receipt,
/// exact replay, divergence conflict, stale revision, revocation, unavailable
/// audit, zero grant/reservation/claim, and fail-closed schema validation.
fn assert_session_prepare_contract(format: u32) {
    use crate::identity::{ContextModeV1, HumanCapabilityV1, IdentityFailureV1 as F};
    let adopted = adopted(format);

    // Before the capability is installed, Prepare is refused and nothing moves.
    // The capability gate precedes identity revalidation, so an in-memory scope
    // suffices: no membership is provisioned on the unextended source.
    {
        use crate::identity::{MembershipV1, PrincipalRefV1};
        let retained = adopted.retained(0, &format!("pre-prepare-{format}.json"), format);
        let principal = PrincipalRefV1::HumanLocal {
            installation_id: retained.source.descriptor().installation_id.to_string(),
            account_id: Uuid::new_v4().to_string(),
        };
        let membership = MembershipV1 {
            version: 1,
            membership_id: Uuid::new_v4().to_string(),
            principal: principal.clone(),
            tenant_id: "A".into(),
            organization_scope: "org-a".into(),
            revision: 1,
            active: true,
            capabilities: [HumanCapabilityV1::LaunchSession].into_iter().collect(),
        };
        let scope =
            crate::identity::authorize_context(&principal, &membership, ContextModeV1::Personal, 1)
                .unwrap();
        let before = head(&retained.store);
        let refusal = prepare(
            &retained,
            &scope,
            &prepare_request(Uuid::new_v4(), Uuid::new_v4(), 1, FUTURE_DEADLINE),
        )
        .unwrap_err();
        assert_eq!(identity_failure(&refusal), Some(F::MigrationRequired));
        assert_eq!(head(&retained.store), before);
        let present: bool = {
            let db = retained.store.db.lock().unwrap();
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='session_preparations')",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert!(!present);
    }

    // The journaled maintenance installs both markers in one transaction.
    let (plan, guard) = adopted.in_place(0);
    let receipts = install_all(&adopted, &guard, &format!("prepare-{format}"));
    assert_eq!(receipts.len(), 2);
    assert_eq!(
        receipts[1].marker,
        InPlaceIdentityMarkerV1::SessionPrepareV1
    );
    drop(guard);
    let retained = adopted.retained(0, &format!("installed-{format}.json"), format);
    assert!(
        retained
            .store
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM events WHERE kind=?1)",
                [PREPARE_MARKER_KIND],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );

    // ReadTenantMetadata is an administration capability: it lets the same
    // membership form an Administration context, which Prepare must refuse.
    let (principal, membership) = provision_membership(
        &retained,
        &[
            HumanCapabilityV1::LaunchSession,
            HumanCapabilityV1::ReadTenantMetadata,
        ],
    )
    .unwrap();
    let scope = personal_scope(&retained, &principal, &membership, ContextModeV1::Personal);
    let target_id = Uuid::new_v4();

    // Offline catalog provisioning: exact replay, divergent conflict, revision.
    // Each provisioning takes its own fresh write epoch: the product bounds an
    // epoch hold to MAX_EPOCH_TIME, so one epoch must not span several writes.
    let provision = |command: &crate::control::session_control::ProvisionSessionLaunchTargetV1| {
        let epoch = retained.owner.try_write_epoch().unwrap();
        retained
            .store
            .provision_session_launch_target_scoped(&epoch, &retained.source, command)
    };
    let command = target_command(Uuid::new_v4(), target_id, 1, 0, &principal);
    let first = provision(&command).unwrap();
    assert!(!first.replayed);
    let replayed = provision(&command).unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.audit_seq, first.audit_seq);
    assert_eq!(replayed.audit_hash, first.audit_hash);
    let mut divergent = command.clone();
    divergent.target.organization_scope = "org-other".into();
    let conflict = provision(&divergent).unwrap_err();
    assert_eq!(
        identity_failure(&conflict),
        Some(F::Conflict),
        "{conflict:#}"
    );
    // Contract-valid (revision = expected + 1) but stale against the catalog.
    let wrong_revision = target_command(Uuid::new_v4(), target_id, 4, 3, &principal);
    let conflict = provision(&wrong_revision).unwrap_err();
    assert_eq!(
        identity_failure(&conflict),
        Some(F::Conflict),
        "{conflict:#}"
    );

    // Prepare retains one durable receipt and nothing else.
    let operation = Uuid::new_v4();
    let request = prepare_request(operation, target_id, 1, FUTURE_DEADLINE);
    let resources_before = table_count(&retained.store, "identity_resources");
    let workers_before = table_count(&retained.store, "workers");
    let before = head(&retained.store);
    let receipt = prepare(&retained, &scope, &request).unwrap();
    assert!(!receipt.replayed);
    assert_eq!(receipt.operation_id, operation);
    assert_eq!(receipt.launch_target_id, target_id);
    assert_eq!(receipt.target_revision, 1);
    assert_eq!(receipt.input_hash, request.input_hash().unwrap());
    assert_eq!(receipt.audit_seq, before.0 + 1);
    assert_eq!(head(&retained.store).0, before.0 + 1);
    assert_eq!(table_count(&retained.store, "session_preparations"), 1);
    // Zero grant, reservation or claim: only the preparation row and its event.
    assert_eq!(
        table_count(&retained.store, "identity_resources"),
        resources_before
    );
    assert_eq!(table_count(&retained.store, "workers"), workers_before);
    {
        let db = retained.store.db.lock().unwrap();
        let (kind, payload): (String, String) = db
            .query_row(
                "SELECT kind,payload FROM events WHERE seq=?1",
                [receipt.audit_seq],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kind, PREPARED_KIND);
        let event: Event = serde_json::from_str(&payload).unwrap();
        assert_eq!(event.session_id, None);
        assert_eq!(event.id, receipt.preparation_id);
        let prepared: Vec<(i64,)> = db
            .prepare("SELECT seq FROM events WHERE kind=?1")
            .unwrap()
            .query_map([PREPARED_KIND], |r| Ok((r.get(0)?,)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(prepared.len(), 1);
    }

    // A lost response replays to the identical receipt without a second event.
    let again = prepare(&retained, &scope, &request).unwrap();
    assert!(again.replayed);
    assert_eq!(again.audit_seq, receipt.audit_seq);
    assert_eq!(again.audit_hash, receipt.audit_hash);
    assert_eq!(again.preparation_id, receipt.preparation_id);
    assert_eq!(head(&retained.store).0, before.0 + 1);
    assert_eq!(table_count(&retained.store, "session_preparations"), 1);

    // A divergent intent under the same operation is a conflict.
    let divergent = prepare_request(operation, target_id, 1, "2127-01-01T00:00:00Z");
    let conflict = prepare(&retained, &scope, &divergent).unwrap_err();
    assert_eq!(
        identity_failure(&conflict),
        Some(F::Conflict),
        "{conflict:#}"
    );
    assert_eq!(head(&retained.store).0, before.0 + 1);

    // A stale expected revision is a conflict, never a retained preparation.
    let stale = prepare_request(Uuid::new_v4(), target_id, 2, FUTURE_DEADLINE);
    let conflict = prepare(&retained, &scope, &stale).unwrap_err();
    assert_eq!(
        identity_failure(&conflict),
        Some(F::Conflict),
        "{conflict:#}"
    );
    assert_eq!(head(&retained.store).0, before.0 + 1);
    assert_eq!(table_count(&retained.store, "session_preparations"), 1);

    // The catalog advances; the old expectation becomes stale, the new admits.
    let epoch = retained.owner.try_write_epoch().unwrap();
    let advance = target_command(Uuid::new_v4(), target_id, 2, 1, &principal);
    retained
        .store
        .provision_session_launch_target_scoped(&epoch, &retained.source, &advance)
        .unwrap();
    drop(epoch);
    let stale = prepare_request(Uuid::new_v4(), target_id, 1, FUTURE_DEADLINE);
    assert_eq!(
        identity_failure(&prepare(&retained, &scope, &stale).unwrap_err()),
        Some(F::Conflict)
    );
    let current = prepare_request(Uuid::new_v4(), target_id, 2, FUTURE_DEADLINE);
    assert!(prepare(&retained, &scope, &current).is_ok());

    // Unknown target, expired deadline and wrong context are refused.
    let unknown = prepare_request(Uuid::new_v4(), Uuid::new_v4(), 1, FUTURE_DEADLINE);
    assert_eq!(
        identity_failure(&prepare(&retained, &scope, &unknown).unwrap_err()),
        Some(F::NotFound)
    );
    let expired = prepare_request(Uuid::new_v4(), target_id, 2, PAST_DEADLINE);
    assert_eq!(
        identity_failure(&prepare(&retained, &scope, &expired).unwrap_err()),
        Some(F::InvalidRequest)
    );
    let admin = personal_scope(
        &retained,
        &principal,
        &membership,
        ContextModeV1::Administration,
    );
    let refused = prepare_request(Uuid::new_v4(), target_id, 2, FUTURE_DEADLINE);
    assert_eq!(
        identity_failure(&prepare(&retained, &admin, &refused).unwrap_err()),
        Some(F::PermissionDenied)
    );

    // The retained receipt reads back for a still-authorized caller.
    let read = |retained: &Retained, operation: Uuid| {
        let epoch = retained.owner.try_read_epoch().unwrap();
        let access = HumanSourceAccess::read(&epoch, &retained.source).unwrap();
        retained.store.modern_session_preparation(
            &access,
            &personal_scope(retained, &principal, &membership, ContextModeV1::Personal),
            &operation.to_string(),
        )
    };
    let read_receipt = read(&retained, operation).unwrap();
    assert!(read_receipt.replayed);
    assert_eq!(read_receipt.audit_seq, receipt.audit_seq);
    assert_eq!(read_receipt.audit_hash, receipt.audit_hash);
    assert_eq!(read_receipt.input_hash, receipt.input_hash);

    // An unavailable mandatory audit blocks the effect: nothing is retained.
    let quota_before: i64 = {
        let db = retained.store.db.lock().unwrap();
        db.query_row("SELECT bytes FROM quota WHERE id=1", [], |r| r.get(0))
            .unwrap()
    };
    let blocked_operation = Uuid::new_v4();
    let blocked = prepare_request(blocked_operation, target_id, 2, FUTURE_DEADLINE);
    let before = head(&retained.store);
    // The storage fault refuses the checked transaction itself; drive the
    // Store entry point with the last good generation to observe the refusal.
    let expected_before = generation(&retained);
    {
        let db = retained.store.db.lock().unwrap();
        db.execute("UPDATE quota SET bytes=?1", [i64::MAX / 2])
            .unwrap();
    }
    let epoch = retained.owner.try_write_epoch().unwrap();
    assert!(
        retained
            .store
            .modern_session_launch_prepare(
                &epoch,
                &retained.source,
                &scope,
                &blocked,
                expected_before
            )
            .is_err()
    );
    drop(epoch);
    assert_eq!(head(&retained.store), before);
    assert_eq!(table_count(&retained.store, "session_preparations"), 2);
    {
        let db = retained.store.db.lock().unwrap();
        db.execute("UPDATE quota SET bytes=?1", [quota_before])
            .unwrap();
    }
    // The integrity fault latched the live store; clearing the backing alone
    // does not unlatch it. Recovery is a reopen that revalidates the source.
    drop(retained);
    let retained = adopted.retained(0, &format!("recovered-{format}.json"), format);
    assert!(prepare(&retained, &scope, &blocked).is_ok());

    // A reopened source replays the preparation and serves the same receipt.
    drop(retained);
    let reopened = adopted.retained(0, &format!("reopened-{format}.json"), format);
    let reread = read(&reopened, operation).unwrap();
    assert_eq!(reread.audit_seq, receipt.audit_seq);
    assert_eq!(reread.audit_hash, receipt.audit_hash);
    let replayed = prepare(&reopened, &scope, &request).unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.audit_hash, receipt.audit_hash);

    // Revocation closes both the replay and the read.
    revoke_membership(&reopened, &principal, &membership).unwrap();
    let before = head(&reopened.store);
    let refusal = prepare(&reopened, &scope, &request).unwrap_err();
    assert!(
        matches!(
            identity_failure(&refusal),
            Some(F::ContextRevoked) | Some(F::IdentityRevoked)
        ),
        "{refusal:#}"
    );
    assert!(read(&reopened, operation).is_err());
    assert_eq!(head(&reopened.store), before);
    drop(reopened);

    // Fail-closed schema: tables without their marker, or a marker without its
    // tables, never open healthy — the protection an older reader relies on.
    let copies = adopted
        .base
        .root
        .persistent_child(&format!("prepare-copies-{format}"), true)
        .unwrap()
        .unwrap();
    let tables_without_marker = copies.path().join("tables-without-marker.sqlite");
    let (_journal, backups) = adopted.directories(&format!("prepare-{format}"));
    std::fs::copy(backups.path().join("source.sqlite"), &tables_without_marker).unwrap();
    {
        let db = rusqlite::Connection::open(&tables_without_marker).unwrap();
        for (_, _, sql) in crate::audit::session_prepare::SCHEMA {
            db.execute_batch(sql).unwrap();
        }
    }
    let refused =
        Store::open_existing_for_tenant(&tables_without_marker, "A", Redactor::new(vec![]), 10_000);
    match refused {
        Ok(store) => assert!(!store.healthy()),
        Err(_) => {}
    }
    let marker_without_tables = copies.path().join("marker-without-tables.sqlite");
    // Copy the installed source that carries both the validated marker and its
    // tables (the plan binding path is the pre-install target, without them).
    let installed = adopted.retained(0, &format!("failclosed-{format}.json"), format);
    let installed_path: String = {
        let db = installed.store.db.lock().unwrap();
        // Flush the WAL into the main file so the copy carries the tables and
        // the marker chain (a bare file copy would miss uncheckpointed pages).
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        db.query_row("PRAGMA database_list", [], |r| r.get(2))
            .unwrap()
    };
    std::fs::copy(&installed_path, &marker_without_tables).unwrap();
    drop(installed);
    {
        let db = rusqlite::Connection::open(&marker_without_tables).unwrap();
        db.execute_batch(
            "DROP TABLE session_preparations; DROP TABLE session_launch_target_operations; DROP TABLE session_launch_targets;",
        )
        .unwrap();
    }
    let refused =
        Store::open_existing_for_tenant(&marker_without_tables, "A", Redactor::new(vec![]), 10_000);
    match refused {
        Ok(store) => assert!(!store.healthy()),
        Err(_) => {}
    }
    // The plan that installed the capability stays readable and bound.
    assert_eq!(plan.target.sources[0].required_format, format);
}

#[test]
fn owned_store14_session_prepare_is_durable_idempotent_and_fenced() {
    assert_session_prepare_contract(14);
}

#[test]
fn owned_store15_session_prepare_is_durable_idempotent_and_fenced() {
    assert_session_prepare_contract(15);
}

const LAUNCHED_KIND: &str = "control.session_prepare.launched";

fn launch_request(
    operation_id: Uuid,
    launch_target_id: Uuid,
    expected_target_revision: u64,
    preparation: &crate::control::session_control::SessionPrepareReceiptV1,
) -> crate::control::session_control::SessionLaunchRequestV1 {
    crate::control::session_control::SessionLaunchRequestV1 {
        version: 1,
        operation_id,
        launch_target_id,
        expected_target_revision,
        preparation_operation_id: preparation.operation_id,
        preparation_input_hash: preparation.input_hash.clone(),
    }
}

fn launch(
    retained: &Retained,
    scope: &crate::identity::AuthorizedScopeV1,
    request: &crate::control::session_control::SessionLaunchRequestV1,
) -> anyhow::Result<crate::control::session_control::SessionLaunchReceiptV1> {
    let expected = generation(retained);
    let epoch = retained.owner.try_write_epoch()?;
    retained
        .store
        .modern_session_launch(&epoch, &retained.source, scope, request, expected)
        .map(|(receipt, _)| receipt)
}

/// The complete Launch contract on one adopted owned format: exact consumption of
/// the retained preparation, real nominative grant, durable audit before effect,
/// retained conversation binding, exact replay, divergence/consumption conflict,
/// stale target, expiry, nominative/permission refusal, revocation and unavailable
/// audit. The supervised worker admission and runtime-observed start are the
/// broker's separate effect and are not asserted here.
fn assert_session_launch_contract(format: u32) {
    use crate::identity::{ContextModeV1, HumanCapabilityV1, IdentityFailureV1 as F};
    let adopted = adopted(format);
    let (plan, guard) = adopted.in_place(0);
    let receipts = install_all(&adopted, &guard, &format!("launch-{format}"));
    assert_eq!(receipts.len(), 2);
    drop(guard);
    let retained = adopted.retained(0, &format!("launch-installed-{format}.json"), format);

    let (principal, membership) = provision_membership(
        &retained,
        &[
            HumanCapabilityV1::LaunchSession,
            HumanCapabilityV1::ReadTenantMetadata,
        ],
    )
    .unwrap();
    let scope = personal_scope(&retained, &principal, &membership, ContextModeV1::Personal);
    let target_id = Uuid::new_v4();
    let epoch = retained.owner.try_write_epoch().unwrap();
    retained
        .store
        .provision_session_launch_target_scoped(
            &epoch,
            &retained.source,
            &target_command(Uuid::new_v4(), target_id, 1, 0, &principal),
        )
        .unwrap();
    drop(epoch);

    // Retain one preparation, then commit it as a real launch.
    let prepare_intent = prepare_request(Uuid::new_v4(), target_id, 1, FUTURE_DEADLINE);
    let preparation = prepare(&retained, &scope, &prepare_intent).unwrap();
    let resources_before = table_count(&retained.store, "identity_resources");
    let before = head(&retained.store);
    let operation = Uuid::new_v4();
    let request = launch_request(operation, target_id, 1, &preparation);
    let receipt = launch(&retained, &scope, &request).unwrap();
    assert!(!receipt.replayed);
    assert_eq!(receipt.operation_id, operation);
    assert_eq!(receipt.launch_target_id, target_id);
    assert_eq!(receipt.target_revision, 1);
    assert_eq!(receipt.preparation_operation_id, preparation.operation_id);
    assert_eq!(receipt.preparation_id, preparation.preparation_id);
    assert_eq!(receipt.input_hash, request.input_hash().unwrap());
    assert_eq!(receipt.grant_revision, 1);
    // The launch audit is the first new event; the grant follows it durably.
    assert_eq!(receipt.audit_seq, before.0 + 1);
    assert_eq!(receipt.grant_audit_seq, before.0 + 2);
    assert_eq!(head(&retained.store).0, before.0 + 2);
    assert_eq!(table_count(&retained.store, "session_launches"), 1);
    // A real nominative session grant was created through the grant producer.
    assert_eq!(
        table_count(&retained.store, "identity_resources"),
        resources_before + 1
    );
    {
        let db = retained.store.db.lock().unwrap();
        let (kind, payload): (String, String) = db
            .query_row(
                "SELECT kind,payload FROM events WHERE seq=?1",
                [receipt.audit_seq],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kind, LAUNCHED_KIND);
        let event: Event = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            event.session_id.as_deref(),
            Some(receipt.session_id.as_str())
        );
        assert_eq!(event.id, receipt.launch_id);
        // The grant names the authenticated initiator as owner, never a share.
        let (owner_key, resource_kind, resource_id, shares): (String, String, String, String) = db
            .query_row(
                "SELECT owner_key,resource_kind,resource_id,payload_json FROM identity_resources WHERE resource_id=?1",
                [&receipt.session_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(owner_key, principal.key().unwrap());
        assert_eq!(resource_kind, "session");
        assert_eq!(resource_id, receipt.session_id);
        let grant: crate::identity::ResourceGrantV1 = serde_json::from_str(&shares).unwrap();
        assert!(grant.shares.is_empty());
        assert_eq!(grant.owner, principal);
        assert!(grant.active);
    }
    // The retained conversation binding anchors the future message channel; the
    // broker-side facts stay unknown until the supervised admission resolves them.
    assert_eq!(receipt.conversation.tenant_id, "A");
    assert_eq!(receipt.conversation.session_id, receipt.session_id);
    assert_eq!(receipt.conversation.launch_target_id, target_id);
    assert_eq!(receipt.conversation.target_revision, 1);
    assert_eq!(receipt.conversation.input_epoch, 0);
    assert_eq!(receipt.conversation.expires_at, FUTURE_DEADLINE);
    assert_eq!(
        receipt.conversation.status,
        crate::control::session_control::SessionConversationStatusV1::Prepared
    );
    assert!(receipt.conversation.worker_id.is_none());
    assert!(receipt.conversation.attempt_id.is_none());
    assert!(receipt.conversation.broker_incarnation.is_none());
    assert!(receipt.conversation.provider.is_none());

    // A lost response replays to the identical receipt without a second effect.
    let again = launch(&retained, &scope, &request).unwrap();
    assert!(again.replayed);
    assert_eq!(again.audit_seq, receipt.audit_seq);
    assert_eq!(again.audit_hash, receipt.audit_hash);
    assert_eq!(again.grant_audit_seq, receipt.grant_audit_seq);
    assert_eq!(again.session_id, receipt.session_id);
    assert_eq!(again.launch_id, receipt.launch_id);
    assert_eq!(head(&retained.store).0, before.0 + 2);
    assert_eq!(table_count(&retained.store, "session_launches"), 1);
    assert_eq!(
        table_count(&retained.store, "identity_resources"),
        resources_before + 1
    );

    // A divergent intent under the same launch operation is a conflict.
    let mut divergent = request.clone();
    divergent.preparation_input_hash = "f".repeat(64);
    let conflict = launch(&retained, &scope, &divergent).unwrap_err();
    assert_eq!(
        identity_failure(&conflict),
        Some(F::Conflict),
        "{conflict:#}"
    );
    assert_eq!(head(&retained.store).0, before.0 + 2);

    // One preparation commits at most one launch: a second distinct operation
    // referencing the consumed preparation is a conflict, never a second session.
    let second = launch_request(Uuid::new_v4(), target_id, 1, &preparation);
    let conflict = launch(&retained, &scope, &second).unwrap_err();
    assert_eq!(
        identity_failure(&conflict),
        Some(F::Conflict),
        "{conflict:#}"
    );
    assert_eq!(table_count(&retained.store, "session_launches"), 1);

    // An unknown preparation and a wrong fingerprint are refused.
    let unknown = launch_request(Uuid::new_v4(), target_id, 1, &preparation);
    let mut unknown = unknown;
    unknown.preparation_operation_id = Uuid::new_v4();
    assert_eq!(
        identity_failure(&launch(&retained, &scope, &unknown).unwrap_err()),
        Some(F::NotFound)
    );

    // A source/config change invalidates the preparation: retain a preparation at
    // the current revision, advance the catalog, then the launch re-resolves the
    // target and refuses the now-stale revision before any effect.
    let epoch = retained.owner.try_write_epoch().unwrap();
    retained
        .store
        .provision_session_launch_target_scoped(
            &epoch,
            &retained.source,
            &target_command(Uuid::new_v4(), target_id, 2, 1, &principal),
        )
        .unwrap();
    drop(epoch);
    let fresh_preparation = prepare(
        &retained,
        &scope,
        &prepare_request(Uuid::new_v4(), target_id, 2, FUTURE_DEADLINE),
    )
    .unwrap();
    let epoch = retained.owner.try_write_epoch().unwrap();
    retained
        .store
        .provision_session_launch_target_scoped(
            &epoch,
            &retained.source,
            &target_command(Uuid::new_v4(), target_id, 3, 2, &principal),
        )
        .unwrap();
    drop(epoch);
    let stale = launch_request(Uuid::new_v4(), target_id, 2, &fresh_preparation);
    let before_stale = head(&retained.store);
    assert_eq!(
        identity_failure(&launch(&retained, &scope, &stale).unwrap_err()),
        Some(F::Conflict)
    );
    assert_eq!(head(&retained.store), before_stale);

    // An expired preparation is refused before any effect. Prepare retains a
    // future deadline; expire the retained row to exercise the launch guard.
    let expiring = prepare(
        &retained,
        &scope,
        &prepare_request(Uuid::new_v4(), target_id, 3, FUTURE_DEADLINE),
    )
    .unwrap();
    {
        let db = retained.store.db.lock().unwrap();
        db.execute(
            "UPDATE session_preparations SET expires_at=?1 WHERE operation_id=?2",
            rusqlite::params![PAST_DEADLINE, expiring.operation_id.to_string()],
        )
        .unwrap();
    }
    let expired = launch_request(Uuid::new_v4(), target_id, 3, &expiring);
    let before_expired = head(&retained.store);
    assert_eq!(
        identity_failure(&launch(&retained, &scope, &expired).unwrap_err()),
        Some(F::InvalidRequest)
    );
    assert_eq!(head(&retained.store), before_expired);

    // The launch is nominative: another principal cannot commit this preparation,
    // and an administration context is refused even for the same membership.
    let (other_principal, other_membership) =
        provision_membership(&retained, &[HumanCapabilityV1::LaunchSession]).unwrap();
    let other_scope = personal_scope(
        &retained,
        &other_principal,
        &other_membership,
        ContextModeV1::Personal,
    );
    // Provisioning the other principal advanced the authorization revision; the
    // initiator's scope must be reacquired at the current revision.
    let scope = personal_scope(&retained, &principal, &membership, ContextModeV1::Personal);

    // Another personal launcher in the same tenant and organization does not
    // inherit the initiator's access to either retained receipt.
    let before_receipt_access = head(&retained.store);
    {
        let epoch = retained.owner.try_read_epoch().unwrap();
        let access = HumanSourceAccess::read(&epoch, &retained.source).unwrap();
        let result = retained.store.modern_session_preparation(
            &access,
            &other_scope,
            &preparation.operation_id.to_string(),
        );
        assert_eq!(
            identity_failure(&result.unwrap_err()),
            Some(F::PermissionDenied)
        );
    }
    assert_eq!(
        identity_failure(&prepare(&retained, &other_scope, &prepare_intent).unwrap_err()),
        Some(F::PermissionDenied)
    );
    assert_eq!(
        identity_failure(&launch(&retained, &other_scope, &request).unwrap_err()),
        Some(F::PermissionDenied)
    );

    // The initiator can still reconcile both original receipts after the target
    // advances and the authorization revision changes.
    let mut expected_preparation = preparation.clone();
    expected_preparation.replayed = true;
    assert_eq!(
        prepare(&retained, &scope, &prepare_intent).unwrap(),
        expected_preparation
    );
    let mut expected_launch = receipt.clone();
    expected_launch.replayed = true;
    assert_eq!(
        launch(&retained, &scope, &request).unwrap(),
        expected_launch
    );
    assert_eq!(head(&retained.store), before_receipt_access);

    let current_preparation = prepare(
        &retained,
        &scope,
        &prepare_request(Uuid::new_v4(), target_id, 3, FUTURE_DEADLINE),
    )
    .unwrap();
    let stolen = launch_request(Uuid::new_v4(), target_id, 3, &current_preparation);
    assert_eq!(
        identity_failure(&launch(&retained, &other_scope, &stolen).unwrap_err()),
        Some(F::PermissionDenied)
    );
    let admin = personal_scope(
        &retained,
        &principal,
        &membership,
        ContextModeV1::Administration,
    );
    let admin_launch = launch_request(Uuid::new_v4(), target_id, 3, &current_preparation);
    assert_eq!(
        identity_failure(&launch(&retained, &admin, &admin_launch).unwrap_err()),
        Some(F::PermissionDenied)
    );
    // The intended initiator still commits it after the refused attempts.
    let committed = launch(
        &retained,
        &scope,
        &launch_request(Uuid::new_v4(), target_id, 3, &current_preparation),
    )
    .unwrap();
    assert!(!committed.replayed);

    // An unavailable mandatory audit blocks the launch: nothing is retained.
    let blocked_preparation = prepare(
        &retained,
        &scope,
        &prepare_request(Uuid::new_v4(), target_id, 3, FUTURE_DEADLINE),
    )
    .unwrap();
    let blocked = launch_request(Uuid::new_v4(), target_id, 3, &blocked_preparation);
    let before_blocked = head(&retained.store);
    let launches_before = table_count(&retained.store, "session_launches");
    let expected_before = generation(&retained);
    // Snapshot after the PREPARED append so the fault-injection restore matches
    // the durable payload sum checked at reopen.
    let quota_before: i64 = {
        let db = retained.store.db.lock().unwrap();
        db.query_row("SELECT bytes FROM quota WHERE id=1", [], |r| r.get(0))
            .unwrap()
    };
    {
        let db = retained.store.db.lock().unwrap();
        db.execute("UPDATE quota SET bytes=?1", [i64::MAX / 2])
            .unwrap();
    }
    let epoch = retained.owner.try_write_epoch().unwrap();
    assert!(
        retained
            .store
            .modern_session_launch(&epoch, &retained.source, &scope, &blocked, expected_before)
            .is_err()
    );
    drop(epoch);
    assert_eq!(head(&retained.store), before_blocked);
    assert_eq!(
        table_count(&retained.store, "session_launches"),
        launches_before
    );
    {
        let db = retained.store.db.lock().unwrap();
        db.execute("UPDATE quota SET bytes=?1", [quota_before])
            .unwrap();
    }
    // The integrity fault latched the store; recovery is a reopen.
    drop(retained);
    let retained = adopted.retained(0, &format!("launch-recovered-{format}.json"), format);
    assert!(launch(&retained, &scope, &blocked).is_ok());

    // A reopened source replays the launch and serves the same receipt.
    drop(retained);
    let reopened = adopted.retained(0, &format!("launch-reopened-{format}.json"), format);
    let replayed = launch(&reopened, &scope, &request).unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.audit_hash, receipt.audit_hash);
    assert_eq!(replayed.session_id, receipt.session_id);

    // Revocation closes the replay.
    revoke_membership(&reopened, &principal, &membership).unwrap();
    let before_revoke = head(&reopened.store);
    let refusal = launch(&reopened, &scope, &request).unwrap_err();
    assert!(
        matches!(
            identity_failure(&refusal),
            Some(F::ContextRevoked) | Some(F::IdentityRevoked)
        ),
        "{refusal:#}"
    );
    assert_eq!(head(&reopened.store), before_revoke);
    assert_eq!(plan.target.sources[0].required_format, format);
}

#[test]
fn owned_store14_session_launch_is_durable_idempotent_and_fenced() {
    assert_session_launch_contract(14);
}

#[test]
fn owned_store15_session_launch_is_durable_idempotent_and_fenced() {
    assert_session_launch_contract(15);
}

// --- SendSessionMessage: durable enqueue, effect-point revalidation, truthful
// delivery statuses, STOP priority, idempotence and mandatory audit.

const MESSAGE_ENQUEUED_KIND: &str = "control.session_message.enqueued";

fn message_request(
    operation_id: Uuid,
    session_id: Uuid,
    expected_input_epoch: u64,
    content: &str,
) -> crate::control::session_control::SessionMessageRequestV1 {
    crate::control::session_control::SessionMessageRequestV1 {
        version: 1,
        operation_id,
        session_id,
        expected_input_epoch,
        content: content.into(),
    }
}

fn send_message(
    retained: &Retained,
    scope: &crate::identity::AuthorizedScopeV1,
    request: &crate::control::session_control::SessionMessageRequestV1,
) -> anyhow::Result<crate::control::session_control::SessionMessageReceiptV1> {
    let expected = generation(retained);
    let epoch = retained.owner.try_write_epoch()?;
    retained
        .store
        .modern_session_message_send(&epoch, &retained.source, scope, request, expected)
        .map(|(receipt, _)| receipt)
}

fn message_failure(
    error: &anyhow::Error,
) -> Option<crate::audit::session_message::SessionMessageFailureV1> {
    error
        .downcast_ref::<crate::audit::session_message::SessionMessageFailureV1>()
        .copied()
}

fn bind_command(
    operation_id: Uuid,
    session_id: &str,
    expected_input_epoch: u64,
) -> crate::control::session_control::BindSessionConversationV1 {
    crate::control::session_control::BindSessionConversationV1 {
        version: 1,
        operation_id,
        session_id: session_id.into(),
        expected_input_epoch,
        channel: crate::control::session_control::SessionConversationChannelV1 {
            broker_id: "broker-1".into(),
            broker_incarnation: "incarnation-1".into(),
            worker_id: "worker-1".into(),
            attempt_id: "attempt-1".into(),
            runtime_instance: "runtime-1".into(),
            provider: "codex".into(),
            protocol: "codex-app-server-v2".into(),
        },
    }
}

fn bind_conversation(
    retained: &Retained,
    command: &crate::control::session_control::BindSessionConversationV1,
) -> anyhow::Result<crate::control::session_control::SessionConversationBindingV1> {
    let epoch = retained.owner.try_write_epoch()?;
    retained
        .store
        .bind_session_conversation_scoped(&epoch, &retained.source, command)
}

fn close_conversation(
    retained: &Retained,
    operation_id: Uuid,
    session_id: &str,
) -> anyhow::Result<crate::control::session_control::SessionConversationBindingV1> {
    let epoch = retained.owner.try_write_epoch()?;
    retained.store.close_session_conversation_scoped(
        &epoch,
        &retained.source,
        &crate::control::session_control::CloseSessionConversationV1 {
            version: 1,
            operation_id,
            session_id: session_id.into(),
        },
    )
}

fn issue_emission(
    retained: &Retained,
    operation_id: Uuid,
    expected_binding_fingerprint: &str,
) -> anyhow::Result<crate::control::session_control::SessionMessageEmissionV1> {
    let epoch = retained.owner.try_write_epoch()?;
    retained.store.issue_session_message_emission_scoped(
        &epoch,
        &retained.source,
        &crate::control::session_control::IssueSessionMessageEmissionV1 {
            version: 1,
            operation_id,
            expected_binding_fingerprint: expected_binding_fingerprint.into(),
        },
    )
}

fn record_transmitted(
    retained: &Retained,
    operation_id: Uuid,
) -> anyhow::Result<crate::control::session_control::SessionMessageReceiptV1> {
    let epoch = retained.owner.try_write_epoch()?;
    retained
        .store
        .record_session_message_transmitted_scoped(&epoch, &retained.source, operation_id)
}

fn record_acknowledged(
    retained: &Retained,
    operation_id: Uuid,
    ack: &crate::control::session_control::SessionMessageAckV1,
) -> anyhow::Result<crate::control::session_control::SessionMessageReceiptV1> {
    let epoch = retained.owner.try_write_epoch()?;
    retained.store.record_session_message_acknowledged_scoped(
        &epoch,
        &retained.source,
        operation_id,
        ack,
    )
}

/// One launched session with its retained Prepared binding.
fn launched_session(
    retained: &Retained,
    scope: &crate::identity::AuthorizedScopeV1,
    principal: &crate::identity::PrincipalRefV1,
    target_id: Uuid,
    revision: u64,
) -> crate::control::session_control::SessionLaunchReceiptV1 {
    let epoch = retained.owner.try_write_epoch().unwrap();
    retained
        .store
        .provision_session_launch_target_scoped(
            &epoch,
            &retained.source,
            &target_command(Uuid::new_v4(), target_id, revision, revision - 1, principal),
        )
        .unwrap();
    drop(epoch);
    let preparation = prepare(
        retained,
        scope,
        &prepare_request(Uuid::new_v4(), target_id, revision, FUTURE_DEADLINE),
    )
    .unwrap();
    launch(
        retained,
        scope,
        &launch_request(Uuid::new_v4(), target_id, revision, &preparation),
    )
    .unwrap()
}

/// The complete SendSessionMessage control-plane contract on one owned format.
/// The conversation adapter's stdio behavior is proven separately with a
/// fixture child; no provider, credential or network is involved here.
fn assert_session_message_contract(format: u32) {
    use crate::audit::session_message::SessionMessageFailureV1 as M;
    use crate::control::session_control::SessionMessageStatusV1 as S;
    use crate::identity::{ContextModeV1, HumanCapabilityV1, IdentityFailureV1 as F};
    let adopted = adopted(format);
    let (_plan, guard) = adopted.in_place(0);
    install_all(&adopted, &guard, &format!("message-{format}"));
    drop(guard);
    let retained = adopted.retained(0, &format!("message-installed-{format}.json"), format);

    let (principal, membership) = provision_membership(
        &retained,
        &[
            HumanCapabilityV1::LaunchSession,
            HumanCapabilityV1::SendSessionMessage,
            HumanCapabilityV1::ReadTenantMetadata,
        ],
    )
    .unwrap();
    let scope = personal_scope(&retained, &principal, &membership, ContextModeV1::Personal);
    let target_id = Uuid::new_v4();
    let session = launched_session(&retained, &scope, &principal, target_id, 1);
    let session_id = Uuid::parse_str(&session.session_id).unwrap();
    let prepared_fingerprint = session.conversation.fingerprint().unwrap();

    // Enqueue is durable, nominative and audited before any byte. The audit
    // records metadata only: the body never enters the chain.
    let body = "synthetic instruction: summarize the fixture report";
    let operation = Uuid::new_v4();
    let request = message_request(operation, session_id, 0, body);
    let before = head(&retained.store);
    let receipt = send_message(&retained, &scope, &request).unwrap();
    assert!(!receipt.replayed);
    assert_eq!(receipt.operation_id, operation);
    assert_eq!(receipt.session_id, session_id);
    assert_eq!(receipt.status, S::Queued);
    assert!(receipt.ack.is_none());
    assert_eq!(receipt.input_hash, request.input_hash().unwrap());
    assert_eq!(receipt.content_hash, request.content_hash().unwrap());
    assert_eq!(receipt.binding_fingerprint, prepared_fingerprint);
    assert_eq!(receipt.input_epoch, 0);
    assert_eq!(receipt.audit_seq, before.0 + 1);
    assert_eq!(head(&retained.store).0, before.0 + 1);
    {
        let db = retained.store.db.lock().unwrap();
        let (kind, payload): (String, String) = db
            .query_row(
                "SELECT kind,payload FROM events WHERE seq=?1",
                [receipt.audit_seq],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kind, MESSAGE_ENQUEUED_KIND);
        assert!(
            !payload.contains(body),
            "the audit must not duplicate the body"
        );
        let event: Event = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            event.session_id.as_deref(),
            Some(session.session_id.as_str())
        );
        assert_eq!(event.id, receipt.message_id);
        // The body is protected tenant business data in the tenant store.
        let stored: String = db
            .query_row(
                "SELECT content FROM session_messages WHERE operation_id=?1",
                [operation.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, body);
    }

    // A lost response replays to the identical retained result, no second effect.
    let again = send_message(&retained, &scope, &request).unwrap();
    assert!(again.replayed);
    assert_eq!(again.audit_seq, receipt.audit_seq);
    assert_eq!(again.audit_hash, receipt.audit_hash);
    assert_eq!(again.message_id, receipt.message_id);
    assert_eq!(head(&retained.store).0, before.0 + 1);

    // Divergent content under the same operation is a conflict.
    let divergent = message_request(operation, session_id, 0, "a different instruction");
    assert_eq!(
        identity_failure(&send_message(&retained, &scope, &divergent).unwrap_err()),
        Some(F::Conflict)
    );
    // A stale epoch pin never silently retargets the binding.
    let stale_epoch = message_request(Uuid::new_v4(), session_id, 7, body);
    assert_eq!(
        message_failure(&send_message(&retained, &scope, &stale_epoch).unwrap_err()),
        Some(M::BindingChanged)
    );
    assert_eq!(head(&retained.store).0, before.0 + 1);

    // Emission requires the resolved channel: a Prepared binding is explicitly
    // not admitted, never a silent fallback.
    assert_eq!(
        message_failure(&issue_emission(&retained, operation, &prepared_fingerprint).unwrap_err()),
        Some(M::ChannelNotAdmitted)
    );

    // SendSessionMessage is distinct and nominative: another principal, an
    // administration context, or LaunchSession alone never imply it.
    let (other_principal, other_membership) =
        provision_membership(&retained, &[HumanCapabilityV1::SendSessionMessage]).unwrap();
    let other_scope = personal_scope(
        &retained,
        &other_principal,
        &other_membership,
        ContextModeV1::Personal,
    );
    let scope = personal_scope(&retained, &principal, &membership, ContextModeV1::Personal);
    let stolen = message_request(Uuid::new_v4(), session_id, 0, body);
    assert_eq!(
        identity_failure(&send_message(&retained, &other_scope, &stolen).unwrap_err()),
        Some(F::PermissionDenied)
    );
    let admin = personal_scope(
        &retained,
        &principal,
        &membership,
        ContextModeV1::Administration,
    );
    assert_eq!(
        identity_failure(&send_message(&retained, &admin, &stolen).unwrap_err()),
        Some(F::PermissionDenied)
    );
    let (launcher_principal, launcher_membership) =
        provision_membership(&retained, &[HumanCapabilityV1::LaunchSession]).unwrap();
    let launcher_scope = personal_scope(
        &retained,
        &launcher_principal,
        &launcher_membership,
        ContextModeV1::Personal,
    );
    assert_eq!(
        identity_failure(&send_message(&retained, &launcher_scope, &stolen).unwrap_err()),
        Some(F::PermissionDenied)
    );
    let scope = personal_scope(&retained, &principal, &membership, ContextModeV1::Personal);

    // The trusted control binds the resolved channel explicitly; the epoch
    // advances. The message pinned to the Prepared binding is never retargeted.
    let bound = bind_conversation(
        &retained,
        &bind_command(Uuid::new_v4(), &session.session_id, 0),
    )
    .unwrap();
    assert_eq!(
        bound.status,
        crate::control::session_control::SessionConversationStatusV1::Active
    );
    assert_eq!(bound.input_epoch, 1);
    assert_eq!(bound.worker_id.as_deref(), Some("worker-1"));
    assert_eq!(bound.protocol.as_deref(), Some("codex-app-server-v2"));
    let active_fingerprint = bound.fingerprint().unwrap();
    assert_ne!(active_fingerprint, prepared_fingerprint);
    assert_eq!(
        message_failure(&issue_emission(&retained, operation, &prepared_fingerprint).unwrap_err()),
        Some(M::BindingChanged)
    );
    // Bind replay is idempotent and returns the retained binding.
    let bind_op = Uuid::new_v4();
    let session2 = launched_session(&retained, &scope, &principal, Uuid::new_v4(), 1);
    let first =
        bind_conversation(&retained, &bind_command(bind_op, &session2.session_id, 0)).unwrap();
    let replayed_bind =
        bind_conversation(&retained, &bind_command(bind_op, &session2.session_id, 0)).unwrap();
    assert_eq!(first, replayed_bind);

    // A fresh message pinned to the Active binding emits: the durable issue
    // latch precedes the first byte; delivery stays unknown until the complete
    // write is observed, then acknowledged only by the exact correlated ack.
    let active_request = message_request(Uuid::new_v4(), session_id, 1, body);
    let active_receipt = send_message(&retained, &scope, &active_request).unwrap();
    assert_eq!(active_receipt.binding_fingerprint, active_fingerprint);
    let before_issue = head(&retained.store);
    let emission =
        issue_emission(&retained, active_receipt.operation_id, &active_fingerprint).unwrap();
    assert_eq!(emission.content, body);
    assert_eq!(emission.session_id, session.session_id);
    assert_eq!(emission.input_epoch, 1);
    assert_eq!(head(&retained.store).0, before_issue.0 + 1);
    let issued = send_message(&retained, &scope, &active_request).unwrap();
    assert_eq!(issued.status, S::Unknown);
    // A replayed issue never re-emits: delivery uncertainty is retained.
    assert_eq!(
        message_failure(
            &issue_emission(&retained, active_receipt.operation_id, &active_fingerprint)
                .unwrap_err()
        ),
        Some(M::EmissionAlreadyIssued)
    );
    let transmitted = record_transmitted(&retained, active_receipt.operation_id).unwrap();
    assert_eq!(transmitted.status, S::Transmitted);
    let transmitted_again = record_transmitted(&retained, active_receipt.operation_id).unwrap();
    assert!(transmitted_again.replayed);
    assert_eq!(transmitted_again.status, S::Transmitted);
    let ack = crate::control::session_control::SessionMessageAckV1 {
        rpc_id: 7,
        turn_id: "turn-fixture-1".into(),
    };
    let acknowledged = record_acknowledged(&retained, active_receipt.operation_id, &ack).unwrap();
    assert_eq!(acknowledged.status, S::Acknowledged);
    assert_eq!(acknowledged.ack.as_ref(), Some(&ack));
    let acknowledged_again =
        record_acknowledged(&retained, active_receipt.operation_id, &ack).unwrap();
    assert!(acknowledged_again.replayed);
    // A divergent acknowledgement under the same operation is a conflict.
    let divergent_ack = crate::control::session_control::SessionMessageAckV1 {
        rpc_id: 7,
        turn_id: "turn-other".into(),
    };
    assert_eq!(
        identity_failure(
            &record_acknowledged(&retained, active_receipt.operation_id, &divergent_ack)
                .unwrap_err()
        ),
        Some(F::Conflict)
    );
    // Acknowledging a message that was never transmitted is refused.
    let queued_only = message_request(Uuid::new_v4(), session_id, 1, "never transmitted");
    let queued_receipt = send_message(&retained, &scope, &queued_only).unwrap();
    assert!(record_acknowledged(&retained, queued_receipt.operation_id, &ack).is_err());

    // STOP closes the right to emit first. The queued message is never emitted
    // and a new enqueue is refused; the close replay is idempotent.
    let stop_op = Uuid::new_v4();
    let closed = close_conversation(&retained, stop_op, &session.session_id).unwrap();
    assert_eq!(
        closed.status,
        crate::control::session_control::SessionConversationStatusV1::Closed
    );
    let closed_again = close_conversation(&retained, stop_op, &session.session_id).unwrap();
    assert_eq!(closed, closed_again);
    assert_eq!(
        message_failure(
            &issue_emission(&retained, queued_receipt.operation_id, &active_fingerprint)
                .unwrap_err()
        ),
        Some(M::SessionClosed)
    );
    let after_stop = message_request(Uuid::new_v4(), session_id, 1, "after stop");
    assert_eq!(
        message_failure(&send_message(&retained, &scope, &after_stop).unwrap_err()),
        Some(M::SessionClosed)
    );

    // The original deadline keeps priority: an expired binding refuses both
    // enqueue and emission before any effect.
    let session3 = launched_session(&retained, &scope, &principal, Uuid::new_v4(), 1);
    let session3_id = Uuid::parse_str(&session3.session_id).unwrap();
    let bound3 = bind_conversation(
        &retained,
        &bind_command(Uuid::new_v4(), &session3.session_id, 0),
    )
    .unwrap();
    let expiring = message_request(Uuid::new_v4(), session3_id, 1, "before expiry");
    let expiring_receipt = send_message(&retained, &scope, &expiring).unwrap();
    {
        let db = retained.store.db.lock().unwrap();
        let mut binding = bound3.clone();
        binding.expires_at = PAST_DEADLINE.into();
        db.execute(
            "UPDATE session_conversations SET binding_json=?1 WHERE session_id=?2",
            rusqlite::params![
                serde_json::to_string(&binding).unwrap(),
                session3.session_id
            ],
        )
        .unwrap();
    }
    let before_expired = head(&retained.store);
    assert_eq!(
        message_failure(
            &issue_emission(
                &retained,
                expiring_receipt.operation_id,
                &bound3.fingerprint().unwrap()
            )
            .unwrap_err()
        ),
        Some(M::BindingExpired)
    );
    assert_eq!(
        message_failure(
            &send_message(
                &retained,
                &scope,
                &message_request(Uuid::new_v4(), session3_id, 1, "after expiry")
            )
            .unwrap_err()
        ),
        Some(M::BindingExpired)
    );
    assert_eq!(head(&retained.store), before_expired);

    // An unavailable mandatory audit blocks the enqueue: nothing is retained.
    let session4 = launched_session(&retained, &scope, &principal, Uuid::new_v4(), 1);
    let session4_id = Uuid::parse_str(&session4.session_id).unwrap();
    let blocked = message_request(Uuid::new_v4(), session4_id, 0, "blocked by audit");
    let before_blocked = head(&retained.store);
    let expected_before = generation(&retained);
    let quota_before: i64 = {
        let db = retained.store.db.lock().unwrap();
        db.query_row("SELECT bytes FROM quota WHERE id=1", [], |r| r.get(0))
            .unwrap()
    };
    {
        let db = retained.store.db.lock().unwrap();
        db.execute("UPDATE quota SET bytes=?1", [i64::MAX / 2])
            .unwrap();
    }
    let epoch = retained.owner.try_write_epoch().unwrap();
    assert!(
        retained
            .store
            .modern_session_message_send(
                &epoch,
                &retained.source,
                &scope,
                &blocked,
                expected_before
            )
            .is_err()
    );
    drop(epoch);
    assert_eq!(head(&retained.store), before_blocked);
    assert_eq!(table_count(&retained.store, "session_messages"), 4);
    {
        let db = retained.store.db.lock().unwrap();
        db.execute("UPDATE quota SET bytes=?1", [quota_before])
            .unwrap();
    }
    // The integrity fault latched the store; recovery is a reopen.
    drop(retained);
    let retained = adopted.retained(0, &format!("message-recovered-{format}.json"), format);
    assert!(send_message(&retained, &scope, &blocked).is_ok());

    // A reopened source replays the enqueue and serves the same receipt.
    drop(retained);
    let reopened = adopted.retained(0, &format!("message-reopened-{format}.json"), format);
    let replayed = send_message(&reopened, &scope, &request).unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.audit_hash, receipt.audit_hash);
    assert_eq!(replayed.message_id, receipt.message_id);

    // Revocation closes both a new enqueue and the replay.
    revoke_membership(&reopened, &principal, &membership).unwrap();
    let before_revoke = head(&reopened.store);
    let refusal = send_message(&reopened, &scope, &request).unwrap_err();
    assert!(
        matches!(
            identity_failure(&refusal),
            Some(F::ContextRevoked) | Some(F::IdentityRevoked)
        ),
        "{refusal:#}"
    );
    assert_eq!(head(&reopened.store), before_revoke);
}

#[test]
fn owned_store14_session_message_is_durable_idempotent_and_fenced() {
    assert_session_message_contract(14);
}

#[test]
fn owned_store15_session_message_is_durable_idempotent_and_fenced() {
    assert_session_message_contract(15);
}

fn assert_session_message_ack_fenced(
    format: u32,
    change_binding: impl FnOnce(
        &Retained,
        &str,
    ) -> crate::audit::session_message::SessionMessageFailureV1,
) {
    use crate::control::session_control::{SessionMessageAckV1, SessionMessageStatusV1};
    use crate::identity::{ContextModeV1, HumanCapabilityV1};

    let adopted = adopted(format);
    let (_plan, guard) = adopted.in_place(0);
    install_all(&adopted, &guard, "message-ack-fence");
    drop(guard);
    let retained = adopted.retained(0, "message-ack-fence.json", format);
    let (principal, membership) = provision_membership(
        &retained,
        &[
            HumanCapabilityV1::LaunchSession,
            HumanCapabilityV1::SendSessionMessage,
        ],
    )
    .unwrap();
    let scope = personal_scope(&retained, &principal, &membership, ContextModeV1::Personal);
    let session = launched_session(&retained, &scope, &principal, Uuid::new_v4(), 1);
    let session_id = Uuid::parse_str(&session.session_id).unwrap();
    let bound = bind_conversation(
        &retained,
        &bind_command(Uuid::new_v4(), &session.session_id, 0),
    )
    .unwrap();
    let fingerprint = bound.fingerprint().unwrap();
    let transmit = |content| {
        let operation_id = Uuid::new_v4();
        let request = message_request(operation_id, session_id, bound.input_epoch, content);
        send_message(&retained, &scope, &request).unwrap();
        issue_emission(&retained, operation_id, &fingerprint).unwrap();
        let receipt = record_transmitted(&retained, operation_id).unwrap();
        assert_eq!(receipt.status, SessionMessageStatusV1::Transmitted);
        assert!(receipt.ack.is_none());
        receipt
    };

    let canary = transmit("acknowledge on the active binding");
    let ack = SessionMessageAckV1 {
        rpc_id: 1,
        turn_id: "turn-before-binding-change".into(),
    };
    let before_ack = head(&retained.store);
    let acknowledged = record_acknowledged(&retained, canary.operation_id, &ack).unwrap();
    assert_eq!(acknowledged.status, SessionMessageStatusV1::Acknowledged);
    assert_eq!(acknowledged.ack.as_ref(), Some(&ack));
    assert!(!acknowledged.replayed);
    assert_eq!(head(&retained.store).0, before_ack.0 + 1);

    let pending = transmit("leave the acknowledgement pending");
    // Change the binding directly through the Store. No local slot or
    // retirement notification participates in this ordering.
    let expected_refusal = change_binding(&retained, &session.session_id);
    let fenced_head = head(&retained.store);
    let late_ack = SessionMessageAckV1 {
        rpc_id: 2,
        turn_id: "turn-after-binding-change".into(),
    };
    let refusal = record_acknowledged(&retained, pending.operation_id, &late_ack).unwrap_err();
    assert_eq!(message_failure(&refusal), Some(expected_refusal));
    assert_eq!(head(&retained.store), fenced_head);

    let mut expected_replay = acknowledged;
    expected_replay.replayed = true;
    let replay = record_acknowledged(&retained, canary.operation_id, &ack).unwrap();
    assert_eq!(replay, expected_replay);
    assert_eq!(head(&retained.store), fenced_head);

    let epoch = retained.owner.try_read_epoch().unwrap();
    let access = HumanSourceAccess::read(&epoch, &retained.source).unwrap();
    let receipt = retained
        .store
        .modern_session_message_receipt(&access, &scope, &pending.operation_id.to_string())
        .unwrap();
    let mut expected_receipt = pending;
    expected_receipt.replayed = true;
    assert_eq!(receipt, expected_receipt);
}

#[test]
fn owned_store14_session_message_ack_is_fenced_by_stop() {
    use crate::control::session_control::SessionConversationStatusV1;

    assert_session_message_ack_fenced(14, |retained, session_id| {
        let closed = close_conversation(retained, Uuid::new_v4(), session_id).unwrap();
        assert_eq!(closed.status, SessionConversationStatusV1::Closed);
        assert_eq!(closed.input_epoch, 1);
        crate::audit::session_message::SessionMessageFailureV1::SessionClosed
    });
}

#[test]
fn owned_store15_session_message_ack_is_fenced_by_rebind() {
    use crate::control::session_control::SessionConversationStatusV1;

    assert_session_message_ack_fenced(15, |retained, session_id| {
        let mut command = bind_command(Uuid::new_v4(), session_id, 1);
        command.channel.attempt_id = "attempt-2".into();
        command.channel.runtime_instance = "runtime-2".into();
        let rebound = bind_conversation(retained, &command).unwrap();
        assert_eq!(rebound.status, SessionConversationStatusV1::Active);
        assert_eq!(rebound.input_epoch, 2);
        crate::audit::session_message::SessionMessageFailureV1::BindingChanged
    });
}

/// Retain an owned source with a caller-chosen redactor, to exercise the
/// approved masking policy at the enqueue seam.
fn retained_with_redactor(
    adopted: &Adopted,
    index: usize,
    manifest: &str,
    format: u32,
    redactor: Redactor,
) -> Retained {
    let owner = adopted.base.owner(&adopted.anchors, manifest, format);
    let source = owner.source_token(adopted.source_id(index)).unwrap();
    let descriptor = &adopted.base.plan.target.sources[index];
    let startup = Store::open_review_startup_for_tenant(
        &descriptor.storage_binding.path(),
        descriptor.tenant_id.as_deref().unwrap(),
        redactor,
        160,
        &source,
    )
    .unwrap();
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

/// The masking policy refuses an instruction it cannot preserve safely, before
/// enqueue; the bounded unresolved queue refuses overflow without consuming the
/// stop reserves. Both are proven on a real owned Store14 source.
#[test]
fn owned_store14_session_message_masking_and_capacity() {
    use crate::audit::session_message::SessionMessageFailureV1 as M;
    use crate::identity::{ContextModeV1, HumanCapabilityV1};
    let adopted = adopted(14);
    let (_plan, guard) = adopted.in_place(0);
    install_all(&adopted, &guard, "message-mask-cap");
    drop(guard);
    let retained = retained_with_redactor(
        &adopted,
        0,
        "message-mask-cap-installed.json",
        14,
        Redactor::new(vec!["synthetic-secret-value".into()]),
    );
    let (principal, membership) = provision_membership(
        &retained,
        &[
            HumanCapabilityV1::LaunchSession,
            HumanCapabilityV1::SendSessionMessage,
        ],
    )
    .unwrap();
    let scope = personal_scope(&retained, &principal, &membership, ContextModeV1::Personal);
    let session = launched_session(&retained, &scope, &principal, Uuid::new_v4(), 1);
    let session_id = Uuid::parse_str(&session.session_id).unwrap();

    // An instruction containing a known secret is refused before enqueue: the
    // masking policy cannot preserve its meaning safely, and it is never
    // silently altered. Nothing is retained.
    let before = head(&retained.store);
    let secret = message_request(
        Uuid::new_v4(),
        session_id,
        0,
        "use synthetic-secret-value to authenticate",
    );
    assert_eq!(
        message_failure(&send_message(&retained, &scope, &secret).unwrap_err()),
        Some(M::ContentRequiresRedaction)
    );
    assert_eq!(head(&retained.store), before);
    assert_eq!(table_count(&retained.store, "session_messages"), 0);
    // A clean instruction is retained.
    assert!(
        send_message(
            &retained,
            &scope,
            &message_request(Uuid::new_v4(), session_id, 0, "clean instruction")
        )
        .is_ok()
    );

    // The bounded unresolved per-session queue refuses overflow. The first
    // clean message occupies one slot; fill the rest, then refuse the next.
    for _ in 1..32 {
        assert!(
            send_message(
                &retained,
                &scope,
                &message_request(Uuid::new_v4(), session_id, 0, "queued instruction")
            )
            .is_ok()
        );
    }
    assert_eq!(table_count(&retained.store, "session_messages"), 32);
    let before_overflow = head(&retained.store);
    assert_eq!(
        message_failure(
            &send_message(
                &retained,
                &scope,
                &message_request(Uuid::new_v4(), session_id, 0, "overflow")
            )
            .unwrap_err()
        ),
        Some(M::CapacityExhausted)
    );
    assert_eq!(head(&retained.store), before_overflow);
    assert_eq!(table_count(&retained.store, "session_messages"), 32);
}
