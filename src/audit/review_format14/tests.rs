//! Real SQLite/FS fixtures; the modern positive is created only by migration.
use super::*;
use crate::control_fs::{Directory, PrivateFile};
use crate::human_authority::{
    HumanAuthorityConfigurationV1, HumanConfigurationFileKindV1, HumanConfigurationFileV1,
    HumanSourceKindV1, HumanStorageProfileV1, SourceStorageBindingV1,
    maintenance::{
        HumanMaintenanceResolvedViewV1, HumanMaintenanceSourceToken, HumanMaintenanceSourceV1,
        HumanMaintenanceViewV1, HumanSourcesMaintenanceConfigurationV1,
        HumanSourcesMaintenanceGuard, HumanSourcesMaintenancePlanV1,
    },
};
use crate::identity::{
    HumanAccountV1, HumanCapabilityV1, IdentityChangeV1, IdentityFailureV1, IdentityMutationV1,
    LegacyControlRoleV1, MembershipV1, PrincipalRefV1,
};
use std::{ffi::OsStr, os::unix::fs::PermissionsExt};
use uuid::Uuid;

struct Fixture {
    _temporary: tempfile::TempDir,
    root: Directory,
    store: Store,
    source: HumanMaintenanceSourceV1,
    plan: HumanSourcesMaintenancePlanV1,
    plan_file: PrivateFile,
    config_files: Vec<(HumanConfigurationFileKindV1, PrivateFile)>,
}

fn principal() -> PrincipalRefV1 {
    PrincipalRefV1::HumanLocal {
        installation_id: Uuid::from_u128(1).to_string(),
        account_id: Uuid::from_u128(2).to_string(),
    }
}

fn mutation(change: IdentityChangeV1) -> IdentityMutationV1 {
    IdentityMutationV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        actor: PrincipalRefV1::LegacyControl {
            credential_id: "synthetic-private-control".into(),
            role: LegacyControlRoleV1::Runner,
        },
        change,
    }
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
    fn ordinary13() -> Self {
        let temporary = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(std::fs::canonicalize(std::env::temp_dir()).unwrap())
            .unwrap();
        let root = Directory::open_existing(temporary.path()).unwrap();
        let source_parent = root.persistent_child("source", true).unwrap().unwrap();
        let store = Store::open_for_tenant(
            &source_parent.path().join("tenant.sqlite"),
            "A",
            Redactor::new(vec![]),
            10_000,
        )
        .unwrap();
        for target in 3..=13 {
            store
                .migrate_control_version(
                    &root.path().join(format!("history-{target}.sqlite")),
                    target - 1,
                    target,
                )
                .unwrap();
        }
        store
            .mutate_identity(&mutation(IdentityChangeV1::Account {
                expected_revision: 0,
                account: HumanAccountV1 {
                    version: 1,
                    principal: principal(),
                    display_name: "Synthetic private account".into(),
                    revision: 1,
                    active: true,
                },
            }))
            .unwrap();
        let source = HumanMaintenanceSourceV1 {
            source_id: Uuid::new_v4(),
            kind: HumanSourceKindV1::TenantStore,
            installation_id: Uuid::new_v4(),
            durable_namespace_id: review_authority::namespace(&store.db.lock().unwrap()).unwrap(),
            tenant_id: Some("A".into()),
            storage_binding: binding(&source_parent, "tenant.sqlite"),
            required_format: 14,
        };
        let config_parent = root
            .persistent_child("configuration", true)
            .unwrap()
            .unwrap();
        let control = config_parent
            .persistent_create("control.json", b"synthetic private control configuration")
            .unwrap();
        let credentials = config_parent
            .persistent_create(
                "credentials.json",
                b"synthetic private credential reference",
            )
            .unwrap();
        let configuration = HumanAuthorityConfigurationV1 {
            control_config_revision: 1,
            public_config_digest: "ac".repeat(32),
            oidc: None,
            files: vec![
                HumanConfigurationFileV1 {
                    kind: HumanConfigurationFileKindV1::ControlConfiguration,
                    storage_binding: binding(&config_parent, "control.json"),
                },
                HumanConfigurationFileV1 {
                    kind: HumanConfigurationFileKindV1::TechnicalCredentials,
                    storage_binding: binding(&config_parent, "credentials.json"),
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
        let plan_file = config_parent
            .persistent_create("maintenance.json", &serde_json::to_vec(&plan).unwrap())
            .unwrap();
        Self {
            _temporary: temporary,
            root,
            store,
            source,
            plan,
            plan_file,
            config_files: vec![
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
            self.config_files.clone(),
        )
        .unwrap();
        HumanSourcesMaintenanceGuard::acquire(
            self.plan_file.clone(),
            HumanSourcesMaintenanceConfigurationV1::new(None, target, None),
        )
        .unwrap()
    }

    fn backup_path(&self) -> std::path::PathBuf {
        self.root
            .persistent_child("backups", true)
            .unwrap()
            .unwrap()
            .path()
            .join("format13.sqlite")
    }

    fn migrate(&self, token: &HumanMaintenanceSourceToken) -> review_authority::AuthorityReceiptV1 {
        let mut db = self.store.db.lock().unwrap();
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let backup = self
            .store
            .prepare_review_source_backup(&tx, token, &self.backup_path())
            .unwrap();
        let receipt = self
            .store
            .install_review_source_format14_in_transaction(&tx, token, &backup)
            .unwrap();
        tx.commit().unwrap();
        receipt
    }

    fn reopen(&self) -> Connection {
        Connection::open_with_flags(
            self.store.backing.path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .unwrap()
    }
}

fn history(db: &Connection, through: i64) -> Vec<Vec<rusqlite::types::Value>> {
    let mut query = db
        .prepare(
            "SELECT seq,received_at,prev_hash,hash,payload FROM events WHERE seq<=?1 ORDER BY seq",
        )
        .unwrap();
    query
        .query_map([through], |row| (0..5).map(|i| row.get(i)).collect())
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn complete_replay(db: &Connection) -> anyhow::Result<()> {
    validate_schema(db, false)?;
    control::validate(db)?;
    if db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))? == 14 {
        validate(db)?;
    }
    anyhow::ensure!(
        Store::check_integrity(db)?.valid,
        "invalid event chain or time floors"
    );
    Ok(())
}

// RED before the real14 profile: installation reaches validate_schema and refuses.
#[test]
fn real_maintenance_migration_creates_closed_format14_and_preserves_history() {
    let fixture = Fixture::ordinary13();
    let prefix = audit_head(&fixture.store.db.lock().unwrap()).unwrap();
    let previous = history(&fixture.store.db.lock().unwrap(), prefix.0);
    let guard = fixture.maintenance();
    let token = guard.target_source(fixture.source.source_id).unwrap();
    let receipt = fixture.migrate(&token);
    let db = fixture.reopen();
    assert_eq!(history(&db, prefix.0), previous);
    assert_eq!(receipt.seq, prefix.0 + 1);
    assert!(!validate_schema(&db, false).unwrap());
    validate(&db).unwrap();
    complete_replay(&db).unwrap();
    let snapshot = review_authority::replay(&db).unwrap();
    assert_eq!(snapshot.anchor_receipt, receipt);
    assert_eq!(snapshot.anchor.source_id, fixture.source.source_id);
    assert_eq!(snapshot.anchor.migration_id, fixture.plan.migration_id);
    assert_eq!(
        snapshot.anchor.durable_namespace_id,
        fixture.source.durable_namespace_id
    );
    assert_eq!(snapshot.anchor.previous_seq, prefix.0);
    assert_eq!(snapshot.anchor.previous_hash, prefix.1);
    let (anchors, incarnations, bindings): (i64, i64, i64) = db
        .query_row(
            "SELECT (SELECT COUNT(*) FROM events WHERE kind='control.review_authority.source_anchor'),(SELECT COUNT(*) FROM events WHERE kind='control.review_authority.incarnation'),(SELECT COUNT(*) FROM runner_review_bindings)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((anchors, incarnations, bindings), (1, 0, 0));
    let backup = Connection::open_with_flags(
        fixture.backup_path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    assert_eq!(history(&backup, prefix.0), previous);
    assert_eq!(
        backup
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        13
    );
    // Reopen alone supplies no epoch or current incarnation to legacy APIs.
    assert!(
        Store::open_for_tenant(
            fixture.store.backing.path(),
            "A",
            Redactor::new(vec![]),
            10_000
        )
        .is_err()
    );
}

#[test]
fn format14_requires_all_exact_new_objects_and_no_extra_schema() {
    let fixture = Fixture::ordinary13();
    let guard = fixture.maintenance();
    let token = guard.target_source(fixture.source.source_id).unwrap();
    fixture.migrate(&token);
    let mut db = fixture.reopen();
    for &(kind, name, _) in schema_objects() {
        let tx = db.transaction().unwrap();
        tx.execute_batch(&format!("DROP {kind} {name}")).unwrap();
        assert!(validate_schema(&tx, false).is_err(), "missing {name}");
        tx.rollback().unwrap();
    }
    for sql in [
        "CREATE TABLE unexpected_review_data(value TEXT)",
        "CREATE VIEW unexpected_review_view AS SELECT seq FROM events",
        "CREATE TRIGGER unexpected_review_trigger AFTER INSERT ON runner_review_bindings BEGIN SELECT 1; END",
    ] {
        let tx = db.transaction().unwrap();
        tx.execute_batch(sql).unwrap();
        assert!(validate_schema(&tx, false).is_err());
        tx.rollback().unwrap();
    }
    validate_schema(&db, false).unwrap();
    validate(&db).unwrap();
}

#[test]
fn schema14_replay_rejects_missing_or_divergent_inherited_projection() {
    let fixture = Fixture::ordinary13();
    let guard = fixture.maintenance();
    let token = guard.target_source(fixture.source.source_id).unwrap();
    fixture.migrate(&token);
    let mut db = fixture.reopen();
    // Every mutation is a negative against a genuinely migrated private source.
    for sql in [
        "UPDATE identity_meta SET authorization_revision=authorization_revision+1 WHERE singleton=1",
        "UPDATE workload_meta SET genesis_hash=printf('%064d',0) WHERE singleton=1",
        "DELETE FROM audit_time_floors WHERE event_seq=(SELECT MAX(event_seq) FROM audit_time_floors)",
        "UPDATE events SET kind='fixture.missing_recovery_marker' WHERE kind='control.kubernetes.create.recovery.genesis'",
    ] {
        let tx = db.transaction().unwrap();
        let affected = tx.execute(sql, []).unwrap();
        assert_eq!(
            affected, 1,
            "negative must reach a real inherited row: {sql}"
        );
        assert!(complete_replay(&tx).is_err(), "{sql}");
        tx.rollback().unwrap();
    }
    complete_replay(&db).unwrap();
}

#[test]
fn format13_rejects_modern_markers_and_approve_capability() {
    let fixture = Fixture::ordinary13();
    let mut membership = MembershipV1 {
        version: 1,
        membership_id: Uuid::new_v4().to_string(),
        principal: principal(),
        tenant_id: "A".into(),
        organization_scope: "private-org".into(),
        revision: 1,
        active: true,
        capabilities: [HumanCapabilityV1::ReadContent].into_iter().collect(),
    };
    fixture
        .store
        .mutate_identity(&mutation(IdentityChangeV1::Membership {
            expected_revision: 0,
            membership: membership.clone(),
        }))
        .unwrap();
    membership.revision = 2;
    membership
        .capabilities
        .insert(HumanCapabilityV1::ApproveLaunch);
    let head = audit_head(&fixture.store.db.lock().unwrap()).unwrap();
    let denied = fixture
        .store
        .mutate_identity(&mutation(IdentityChangeV1::Membership {
            expected_revision: 1,
            membership,
        }))
        .unwrap_err();
    assert_eq!(
        denied.downcast_ref::<IdentityFailureV1>(),
        Some(&IdentityFailureV1::MigrationRequired)
    );
    assert_eq!(audit_head(&fixture.store.db.lock().unwrap()).unwrap(), head);
    let mut db = fixture.store.db.lock().unwrap();
    assert!(validate(&db).is_err());
    for kind in [
        review_authority::ANCHOR_KIND,
        review_authority::INCARNATION_KIND,
        "control.review_binding.provisioned",
        "control.review_binding.revoked",
    ] {
        let tx = db.transaction().unwrap();
        let (event, payload) = fixture.store.prepare_event(Event::new(kind)).unwrap();
        fixture
            .store
            .append_in_transaction(&tx, event, payload)
            .unwrap();
        assert!(complete_replay(&tx).is_err(), "{kind}");
        tx.rollback().unwrap();
    }
}

#[test]
fn prepared_format13_backup_resumes_without_rewriting_then_installs_once() {
    let fixture = Fixture::ordinary13();
    let guard = fixture.maintenance();
    let token = guard.target_source(fixture.source.source_id).unwrap();
    let path = fixture.backup_path();
    let before = audit_head(&fixture.store.db.lock().unwrap()).unwrap();
    {
        let mut db = fixture.store.db.lock().unwrap();
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        fixture
            .store
            .prepare_review_source_backup(&tx, &token, &path)
            .unwrap();
        tx.rollback().unwrap();
    }
    let backup_bytes = std::fs::read(&path).unwrap();
    let receipt = {
        let mut db = fixture.store.db.lock().unwrap();
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let backup = fixture
            .store
            .resume_review_source_backup(&tx, &token, &path)
            .unwrap();
        let first = fixture
            .store
            .install_review_source_format14_in_transaction(&tx, &token, &backup)
            .unwrap();
        let retry = fixture
            .store
            .install_review_source_format14_in_transaction(&tx, &token, &backup)
            .unwrap();
        assert_eq!(first, retry);
        assert_eq!(first.seq, before.0 + 1);
        tx.commit().unwrap();
        first
    };
    assert_eq!(std::fs::read(&path).unwrap(), backup_bytes);
    {
        let mut db = fixture.store.db.lock().unwrap();
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let backup = fixture
            .store
            .resume_review_source_backup(&tx, &token, &path)
            .unwrap();
        let retry = fixture
            .store
            .install_review_source_format14_in_transaction(&tx, &token, &backup)
            .unwrap();
        assert_eq!(retry, receipt);
        tx.commit().unwrap();
    }
    complete_replay(&fixture.reopen()).unwrap();
}

#[test]
fn binding_marker_dispatch_uses_the_closed_record_and_exact_envelope() {
    use crate::control::review_binding::{ReviewBindingSnapshotV1, tests::provision};
    let command = provision();
    let snapshot = ReviewBindingSnapshotV1 {
        version: 1,
        binding: command.binding.clone(),
        policy: command.policy.clone(),
        binding_digest: command.binding.digest().unwrap(),
        policy_digest: command.policy.digest().unwrap(),
    };
    let mut event = Event::new("control.review_binding.provisioned");
    event.id = command.operation_id.clone();
    event.coverage = "native".into();
    event.data = serde_json::json!({
        "version":1,
        "authority_source_id":Uuid::from_u128(40).to_string(),
        "owner_incarnation":Uuid::from_u128(41).to_string(),
        "input_hash":command.input_hash().unwrap(),
        "previous_revision":0,
        "change":{"action":"provision","command":command},
        "snapshot":snapshot,
    });
    // This is a descriptive decoder fixture, never a runtime ownership proof.
    review_binding::marker(&event, "tenant-A").unwrap();
    assert!(review_binding::marker(&event, "tenant-B").is_err());
    for changed in [
        {
            let mut e = event.clone();
            e.kind = "control.review_binding.unknown".into();
            e
        },
        {
            let mut e = event.clone();
            e.kind = "control.review_binding.revoked".into();
            e
        },
        {
            let mut e = event.clone();
            e.id = Uuid::new_v4().to_string();
            e
        },
        {
            let mut e = event.clone();
            e.source = "worker".into();
            e
        },
        {
            let mut e = event.clone();
            e.coverage = "observed".into();
            e
        },
        {
            let mut e = event.clone();
            e.session_id = Some(Uuid::new_v4().to_string());
            e
        },
        {
            let mut e = event.clone();
            e.data["unexpected"] = serde_json::json!(true);
            e
        },
        {
            let mut e = event.clone();
            e.data["change"]["action"] = serde_json::json!("unknown");
            e
        },
        {
            let mut e = event.clone();
            e.data["input_hash"] = serde_json::json!("00".repeat(32));
            e
        },
    ] {
        assert!(review_binding::marker(&changed, "tenant-A").is_err());
    }
}

#[test]
fn converted14_rechecks_inherited_replay_and_real_backup_before_retry() {
    let fixture = Fixture::ordinary13();
    let guard = fixture.maintenance();
    let token = guard.target_source(fixture.source.source_id).unwrap();
    let receipt = fixture.migrate(&token);
    let mut db = fixture.store.db.lock().unwrap();
    let head = audit_head(&db).unwrap();
    {
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let backup = fixture
            .store
            .resume_review_source_backup(&tx, &token, &fixture.backup_path())
            .unwrap();
        tx.execute("UPDATE identity_meta SET authorization_revision=authorization_revision+1 WHERE singleton=1", []).unwrap();
        assert!(
            fixture
                .store
                .inspect_review_source_migration(&tx, &token)
                .is_err()
        );
        assert!(
            fixture
                .store
                .install_review_source_format14_in_transaction(&tx, &token, &backup)
                .is_err()
        );
        tx.rollback().unwrap();
    }
    assert_eq!(audit_head(&db).unwrap(), head);
    {
        let backup_db = Connection::open_with_flags(
            fixture.backup_path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .unwrap();
        backup_db
            .execute(
                "UPDATE events SET hash=?1 WHERE seq=(SELECT MAX(seq) FROM events)",
                ["00".repeat(32)],
            )
            .unwrap();
        backup_db.close().unwrap();
    }
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    assert!(
        fixture
            .store
            .resume_review_source_backup(&tx, &token, &fixture.backup_path())
            .is_err()
    );
    assert_eq!(
        review_authority::replay(&tx).unwrap().anchor_receipt,
        receipt
    );
    assert_eq!(audit_head(&tx).unwrap(), head);
    tx.rollback().unwrap();
}

#[test]
fn exhausted_audit_refuses_conversion_and_preserves_the_new_backup() {
    let mut fixture = Fixture::ordinary13();
    let head = audit_head(&fixture.store.db.lock().unwrap()).unwrap();
    fixture.store.max_events = u64::try_from(head.0).unwrap();
    let guard = fixture.maintenance();
    let token = guard.target_source(fixture.source.source_id).unwrap();
    let mut db = fixture.store.db.lock().unwrap();
    let before = history(&db, head.0);
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let backup = fixture
        .store
        .prepare_review_source_backup(&tx, &token, &fixture.backup_path())
        .unwrap();
    assert!(
        fixture
            .store
            .install_review_source_format14_in_transaction(&tx, &token, &backup)
            .is_err()
    );
    tx.rollback().unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        13
    );
    assert_eq!(history(&db, head.0), before);
    assert!(fixture.backup_path().exists());
    complete_replay(&db).unwrap();
}

#[test]
fn preopened13_identity_retry_and_projection_refuse_after_real14_commit() {
    let fixture = Fixture::ordinary13();
    let account_revision = mutation(IdentityChangeV1::Account {
        expected_revision: 1,
        account: HumanAccountV1 {
            version: 1,
            principal: principal(),
            display_name: "Synthetic revised account".into(),
            revision: 2,
            active: true,
        },
    });
    fixture.store.mutate_identity(&account_revision).unwrap();
    let guard = fixture.maintenance();
    let token = guard.target_source(fixture.source.source_id).unwrap();
    fixture.migrate(&token);
    let before = audit_head(&fixture.store.db.lock().unwrap()).unwrap();
    let result = fixture
        .store
        .mutate_identity(&account_revision)
        .unwrap_err();
    assert_eq!(
        result.downcast_ref::<review_authority::ReviewAuthorityFailure>(),
        Some(&review_authority::ReviewAuthorityFailure::Required)
    );
    assert!(fixture.store.identity_memberships(&principal()).is_err());
    assert!(
        fixture
            .store
            .authentication_membership_snapshot(&principal())
            .is_err()
    );
    assert!(fixture.store.identity_authorization_revision().is_err());
    assert!(fixture.store.healthy());
    assert_eq!(
        audit_head(&fixture.store.db.lock().unwrap()).unwrap(),
        before
    );
    complete_replay(&fixture.reopen()).unwrap();
}
