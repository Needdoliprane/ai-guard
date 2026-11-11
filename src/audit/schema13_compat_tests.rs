//! Real migration/backup/checkpoint continuity. No cluster or runtime effect.
use super::*;
use crate::broker_registry::{
    BrokerAuthorityV1, BrokerCheckpointSnapshotV1, BrokerCheckpointV1, BrokerJobCheckpointV1,
    BrokerRuntimeKindV1,
};

fn retained_schema12() -> (tempfile::TempDir, std::path::PathBuf, Store) {
    let (directory, source, store) = retained_schema11();
    store
        .migrate_broker_checkpoints_v12(&directory.path().join("before12.sqlite"))
        .unwrap();
    assert_eq!(version(&store), 12);
    assert!(store.integrity().unwrap().valid);
    (directory, source, store)
}

#[test]
fn schema13_explicit_migration_preserves_exact12_backup_namespace_and_quota() {
    let (directory, source, store) = retained_schema12();
    let before = rows(&store);
    let namespace = store.supervision_state().unwrap().control_namespace_uuid;
    let quota = held(&store);
    assert!(!store.supervision_state().unwrap().active);
    let backup_path = directory.path().join("before13.sqlite");
    let result = store.migrate_control_version(&backup_path, 12, 13);
    let backup =
        Store::open_existing_for_tenant(&backup_path, "tenant-a", Redactor::new(vec![]), 10_000)
            .unwrap();
    assert_eq!(version(&backup), 12);
    assert_eq!(rows(&backup), before);
    assert_eq!(held(&backup), quota);
    assert!(backup.integrity().unwrap().valid);
    assert!(!backup.supervision_state().unwrap().active);
    drop(backup);
    let report = result.unwrap_or_else(|error| {
        panic!("explicit schema13 migration must commit after an exact verified12 backup: {error}")
    });
    assert_eq!((report.from_version, report.to_version), (12, 13));
    assert_eq!(version(&store), 13);
    assert_eq!(held(&store), quota);
    assert_eq!(
        store.supervision_state().unwrap().control_namespace_uuid,
        namespace
    );
    assert!(!store.supervision_state().unwrap().active);
    let after = rows(&store);
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(&after[..before.len()], before.as_slice());
    let marker: Event = serde_json::from_str(&after.last().unwrap().2).unwrap();
    assert_eq!(marker.kind, "control.kubernetes.create.recovery.genesis");
    assert_eq!(marker.data["version"], 13);
    assert_eq!(marker.data["from_schema"], 12);
    assert_eq!(marker.data["protocol"], "kubernetes_create_recovery_v3");
    assert_eq!(
        (report.genesis_seq, &report.genesis_hash),
        (after.last().unwrap().0, &after.last().unwrap().1)
    );
    assert_eq!(
        after
            .iter()
            .filter(|(_, _, payload)| {
                serde_json::from_str::<Event>(payload).unwrap().kind
                    == "control.broker_checkpoints.genesis"
            })
            .count(),
        1,
        "the schema12 marker must not be replaced or repeated"
    );
    assert!(store.integrity().unwrap().valid);
    drop(store);
    let reopened =
        Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
            .unwrap();
    assert_eq!(version(&reopened), 13);
    assert_eq!(rows(&reopened), after);
    assert_eq!(held(&reopened), quota);
    assert!(!reopened.supervision_state().unwrap().active);
    assert!(reopened.integrity().unwrap().valid);
}

#[test]
fn schema13_marker_audit_refusal_preserves_source_and_the_verified12_backup() {
    let (directory, source, store) = retained_schema12();
    let before = durable_snapshot(&store.db.lock().unwrap());
    let capacity = u64::try_from(store.head().unwrap()).unwrap();
    drop(store);
    let store =
        Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), capacity)
            .unwrap();
    let backup_path = directory.path().join("refused-before13.sqlite");
    let error = store
        .migrate_control_version(&backup_path, 12, 13)
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("audit capacity exhausted"),
        "must reach mandatory marker audit: {error:#}"
    );
    assert_eq!(version(&store), 12);
    assert_eq!(durable_snapshot(&store.db.lock().unwrap()), before);
    assert_eq!(held(&store), (1, 500, 134_217_728));
    assert!(!store.supervision_state().unwrap().active);
    assert!(store.integrity().unwrap().valid);
    let backup =
        Store::open_existing_for_tenant(&backup_path, "tenant-a", Redactor::new(vec![]), 10_000)
            .unwrap();
    assert_eq!(version(&backup), 12);
    assert_eq!(durable_snapshot(&backup.db.lock().unwrap()), before);
    assert!(backup.integrity().unwrap().valid);
}

#[test]
fn a_declared_schema13_without_its_recovery_marker_is_not_adopted_or_repaired() {
    #[derive(Debug, PartialEq)]
    struct UnchangedDatabase {
        schema_version: i64,
        objects: Vec<(String, String, String, Option<String>)>,
        rows: Vec<(String, Vec<Vec<rusqlite::types::Value>>)>,
    }

    // Compare every persisted table and schema object, not only the audit or
    // the held-CREATE projection, so a refused access cannot repair metadata.
    fn snapshot(db: &Connection) -> UnchangedDatabase {
        let mut schema = db
            .prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name")
            .unwrap();
        let objects = schema
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let mut tables = db
            .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
            .unwrap();
        let names = tables
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let mut rows = Vec::new();
        for name in names {
            let sql = format!("SELECT * FROM \"{}\"", name.replace('\"', "\"\""));
            let columns = db.prepare(&sql).unwrap().column_count();
            assert!(columns > 0);
            let ordering = (1..=columns)
                .map(|column| column.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let mut table = db.prepare(&format!("{sql} ORDER BY {ordering}")).unwrap();
            let values = table
                .query_map([], |row| {
                    (0..columns)
                        .map(|column| row.get::<_, rusqlite::types::Value>(column))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            rows.push((name, values));
        }
        UnchangedDatabase {
            schema_version: db
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap(),
            objects,
            rows,
        }
    }

    let (directory, source, store) = retained_schema12();
    assert!(store.healthy());
    drop(store);
    // Only the synthetic fixture's version is changed. This is not a supported
    // migration and cannot establish a valid control or execution capability.
    let db = Connection::open_with_flags(
        &source,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    db.pragma_update(None, "user_version", 13).unwrap();
    let expected = snapshot(&db);
    assert_eq!(expected.schema_version, 13);
    drop(db);

    // An existing Store deliberately remains available for diagnostics after
    // semantic validation fails. Its health and live origin must remain closed.
    let opened =
        Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
            .expect("retain the existing unavailable Store for diagnostics");
    let assert_unchanged_and_closed = |phase: &str| {
        assert!(!opened.healthy(), "{phase}: invalid schema13 was adopted");
        {
            let lease = opened.lock_supervision_lease().unwrap();
            assert!(lease.invalidated, "{phase}: missing invalidation latch");
            assert!(!lease.active, "{phase}: execution authority became active");
            assert!(!lease.pending_activation, "{phase}: activation was armed");
        }
        assert_eq!(snapshot(&opened.db.lock().unwrap()), expected, "{phase}");
    };
    let assert_latched = |error: anyhow::Error, operation: &str| {
        assert!(
            error
                .to_string()
                .contains("audit integrity failure is latched; recovery required"),
            "{operation} must refuse at the invalidated origin: {error:#}"
        );
    };
    assert_unchanged_and_closed("diagnostic open");
    assert_latched(opened.supervision_state().unwrap_err(), "supervision read");
    assert_unchanged_and_closed("refused supervision read");
    let event = Event::new("fixture.schema13.missing_marker.write");
    event.validate().unwrap();
    assert_latched(opened.append(event).unwrap_err(), "ordinary append");
    assert_unchanged_and_closed("refused append");

    let recovery_backup = directory.path().join("refused-marker-repair.sqlite");
    assert_latched(
        opened
            .migrate_control_version(&recovery_backup, 12, 13)
            .unwrap_err(),
        "explicit recovery migration",
    );
    assert!(!recovery_backup.exists());
    assert_unchanged_and_closed("refused recovery migration");
    let activation_backup = directory.path().join("refused-activation.sqlite");
    assert_latched(
        opened.migrate_control_v3(&activation_backup).unwrap_err(),
        "activation migration",
    );
    assert!(!activation_backup.exists());
    assert_unchanged_and_closed("refused activation migration");

    match opened.integrity() {
        Ok(integrity) => assert!(!integrity.valid, "missing recovery marker passed integrity"),
        Err(error) => assert!(
            format!("{error:#}").contains("invalid schema13 recovery marker count"),
            "integrity must report the missing semantic marker: {error:#}"
        ),
    }
    assert_unchanged_and_closed("diagnostic integrity");
    drop(opened);
    let db = Connection::open_with_flags(
        &source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    assert_eq!(snapshot(&db), expected);
}

#[test]
fn schema13_refuses_a_skipped_source_and_a_retained_backup_without_replacing_it() {
    use std::os::unix::fs::MetadataExt;
    let (directory, source, store) = retained_schema11();
    let before11 = rows(&store);
    let skipped = directory.path().join("skipped-before13.sqlite");
    let error = store.migrate_control_version(&skipped, 11, 13).unwrap_err();
    assert_eq!(
        error.downcast_ref::<ControlFailure>(),
        Some(&ControlFailure::InvalidRequest)
    );
    assert!(
        !skipped.exists(),
        "12-to13 is the only permitted source edge"
    );
    assert_eq!(rows(&store), before11);
    store
        .migrate_broker_checkpoints_v12(&directory.path().join("before12.sqlite"))
        .unwrap();
    let before12 = rows(&store);
    let existing = directory.path().join("before12.sqlite");
    let bytes = std::fs::read(&existing).unwrap();
    let identity = std::fs::metadata(&existing).unwrap();
    assert!(store.migrate_control_version(&existing, 12, 13).is_err());
    let after = std::fs::metadata(&existing).unwrap();
    assert_eq!((identity.dev(), identity.ino()), (after.dev(), after.ino()));
    assert_eq!(std::fs::read(&existing).unwrap(), bytes);
    assert_eq!(version(&store), 12);
    assert_eq!(rows(&store), before12);
    assert!(store.integrity().unwrap().valid);
    drop(store);
    assert!(
        Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
            .unwrap()
            .integrity()
            .unwrap()
            .valid
    );
}

#[test]
fn schema12_checkpoint_receipt_replays_exactly_after_real_schema13_migration() {
    use uuid::Uuid;
    let fixture = kubernetes_admission_state_tests::Fixture::new();
    let source = fixture._directory.path().join("tenant.sqlite");
    let identity = fixture.identity;
    let authority = BrokerAuthorityV1 {
        authority_id: fixture.intent.authority.authority_id().to_string(),
        runtime_kind: BrokerRuntimeKindV1::Kubernetes,
    };
    let store = fixture.store;
    store
        .migrate_kubernetes_policy_v9(&source.with_extension("before9"))
        .unwrap();
    store
        .migrate_human_workers_v10(&source.with_extension("before10"))
        .unwrap();
    store
        .migrate_kubernetes_session_reservations_v11(&source.with_extension("before11"))
        .unwrap();
    store
        .migrate_broker_checkpoints_v12(&source.with_extension("before12"))
        .unwrap();
    let original_identity = store.broker_authenticated_identity(&identity).unwrap();
    assert_eq!(original_identity.schema_version, 12);
    let checkpoint = BrokerCheckpointV1 {
        version: 1,
        operation_id: Uuid::new_v4().to_string(),
        incarnation_id: original_identity.incarnation_id.clone(),
        previous_sequence: 0,
        sequence: 1,
        snapshot: BrokerCheckpointSnapshotV1 {
            version: 1,
            authority,
            jobs: vec![BrokerJobCheckpointV1 {
                job_id: Uuid::new_v4().to_string(),
                revision: 17,
                checkpoint_hash: "a".repeat(64),
            }],
            has_more: false,
        },
    };
    let first = store.broker_checkpoint(&identity, &checkpoint).unwrap();
    assert!(!first.replayed);
    let original_input =
        Store::broker_checkpoint_input_hash(&original_identity, &checkpoint).unwrap();
    assert_eq!(original_input, first.receipt.input_hash);
    drop(store);
    let store = Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
        .unwrap();
    store
        .migrate_control_version(&source.with_extension("before13"), 12, 13)
        .unwrap();
    let current_identity = store.broker_authenticated_identity(&identity).unwrap();
    assert_eq!(current_identity.schema_version, 13);
    let mut same_scope = current_identity.clone();
    same_scope.schema_version = 12;
    assert_eq!(
        same_scope, original_identity,
        "only the schema can differ after migration"
    );
    assert_eq!(
        Store::broker_checkpoint_input_hash(&current_identity, &checkpoint).unwrap(),
        original_input
    );
    let before_replay = rows(&store);
    let replay = store.broker_checkpoint(&identity, &checkpoint).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.receipt, first.receipt);
    assert_eq!(replay.checkpoint, checkpoint);
    assert_eq!(rows(&store), before_replay);
    let retained = store
        .broker_checkpoint_for_identity(&identity)
        .unwrap()
        .checkpoint
        .unwrap();
    assert_eq!(retained.receipt, first.receipt);
    assert!(retained.in_current_grant);
    let mut next = checkpoint.clone();
    next.operation_id = Uuid::new_v4().to_string();
    next.previous_sequence = 1;
    next.sequence = 2;
    next.snapshot.jobs[0].revision = 18;
    let accepted = store.broker_checkpoint(&identity, &next).unwrap();
    assert!(!accepted.replayed);
    assert_eq!(accepted.checkpoint, next);
    assert!(store.integrity().unwrap().valid);
    assert!(!store.supervision_state().unwrap().active);
    drop(store);
    let reopened =
        Store::open_existing_for_tenant(&source, "tenant-a", Redactor::new(vec![]), 10_000)
            .unwrap();
    assert_eq!(
        reopened
            .broker_checkpoint_for_identity(&identity)
            .unwrap()
            .checkpoint
            .unwrap()
            .receipt,
        accepted.receipt
    );
    assert!(reopened.integrity().unwrap().valid);
}
