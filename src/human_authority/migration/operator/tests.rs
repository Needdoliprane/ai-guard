use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Closed {
    version: u32,
    values: Vec<String>,
}

#[test]
fn closed_documents_reject_duplicates_and_bound_before_insertion() {
    let valid: Closed = decode_closed(br#"{"version":1,"values":["a"]}"#, 128, 2).unwrap();
    assert_eq!((valid.version, valid.values.len()), (1, 1));
    for bytes in [
        br#"{"version":1,"version":1,"values":[]}"#.as_slice(),
        br#"{"version":1,"values":["a","b","c"]}"#,
        br#"{"version":1,"values":[],"ignored":1}"#,
    ] {
        assert!(decode_closed::<Closed>(bytes, 128, 2).is_err());
    }
    assert!(decode_closed::<Closed>(&vec![b' '; 129], 128, 2).is_err());
    let aliases=br#"{"source_profiles":{"abcdef00-0000-4000-8000-000000000001":{},"ABCDEF00-0000-4000-8000-000000000001":{}}}"#;
    assert!(decode_closed::<serde_json::Value>(aliases, 1024, 515).is_err());
}

#[test]
fn publication_reuses_identical_bytes_and_preserves_conflicting_inode() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let directory =
        PrivatePreparationDirectory::create(&root.path().canonicalize().unwrap().join("new"))
            .unwrap();
    let first = directory
        .publish_or_verify_identical("recovery.json", b"complete", 1024)
        .unwrap();
    let binding = observe_private_file_binding(&first).unwrap();
    assert_eq!(
        directory
            .publish_or_verify_identical("recovery.json", b"complete", 1024)
            .unwrap()
            .bytes(),
        b"complete"
    );
    assert!(
        directory
            .publish_or_verify_identical("recovery.json", b"changed", 1024)
            .is_err()
    );
    assert!(observe_private_file_binding(&first).unwrap() == binding);
    assert_eq!(std::fs::read(first.path()).unwrap(), b"complete");
    assert!(PrivatePreparationDirectory::create(directory.path()).is_err());
}

#[test]
fn budget_is_reserved_before_read_and_repeated_reference_is_not_reopened() {
    let mut budget = InputBudget::new();
    assert!(budget.reserve(MAX_RETAINED_BYTES + 1).is_err());
    assert_eq!(budget.remaining(), MAX_RETAINED_BYTES);
    budget.reserve(MAX_RETAINED_BYTES).unwrap();
    assert_eq!(budget.remaining(), 0);
    assert!(budget.reserve(1).is_err());
}

#[tokio::test]
async fn real_temporary_migration_owner_closes_admission_and_waits_for_drain() {
    let fixture = super::super::tests::Fixture::new_owned_local();
    let migrated = fixture.migrated();
    let owner = migrated.owner.clone();
    let epoch = owner.try_read_epoch().unwrap();
    let close = tokio::spawn(close_migrated(migrated));
    tokio::task::yield_now().await;
    assert!(owner.try_read_epoch().is_err());
    assert!(
        !close.is_finished(),
        "the real accepted epoch must drain before activation publication"
    );
    drop(epoch);
    let manifest = close.await.unwrap().unwrap();
    manifest.persistent_revalidate().unwrap();
    assert!(owner.try_read_epoch().is_err());
}

#[tokio::test]
async fn retained_input_resync_observes_real_syncs_for_registry_tokens_oidc_and_ca_without_reopen()
{
    use crate::control_fs::cost_probe::{EXCLUSIVE, Measurement, Operation};
    use std::{
        fs,
        io::Write,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
    };
    let _exclusive = EXCLUSIVE.lock().await;
    let temporary = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let inputs: [(&str, &[u8]); 5] = [
        ("registry.json", br#"{"version":1,"tenants":[]}"#),
        ("runner.token", b"synthetic-runner-credential-long-enough"),
        ("viewer.token", b"synthetic-viewer-credential-long-enough"),
        (
            "oidc.json",
            include_bytes!("../../../../docs/setup/sso/examples/gateway.https.json"),
        ),
        (
            "public-ca.pem",
            include_bytes!("../../../../tests/diagnose_support/fixtures/public-test-root.pem"),
        ),
    ];
    let mut budget = InputBudget::new();
    for (name, bytes) in inputs {
        let path = root.join(name);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        budget.read(&path, 1_048_576).unwrap();
    }
    // Each scoped collector observes exactly that retained input's actual
    // std::File::sync_all span; no pathname or credential leaves the collector.
    for (name, _) in inputs {
        let measurement = Measurement::start(&root.join(name)).unwrap();
        budget.resync_all().unwrap();
        let observed = measurement.finish().unwrap();
        assert_eq!(observed.metric(Operation::FileSyncAll).calls, 1);
        assert_eq!(observed.metric(Operation::FileSyncAll).succeeded, 1);
        assert_eq!(
            observed.metric(Operation::Read).calls,
            0,
            "resync must use the retained FD"
        );
        assert_eq!(observed.outstanding, 0);
    }
}

#[test]
fn retained_input_resync_refuses_same_bytes_replacement_without_adopting_new_inode() {
    use std::{
        fs,
        io::Write,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
    };
    let temporary = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let path = root.join("runner.token");
    let bytes = b"synthetic-runner-credential-long-enough";
    let write = || {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
    };
    write();
    let mut budget = InputBudget::new();
    let retained = budget.read(&path, 1024).unwrap();
    let original = observe_private_file_binding(&retained).unwrap();
    let preserved = root.join("preserved-original.token");
    fs::rename(&path, &preserved).unwrap();
    write();
    let replacement =
        observe_private_file_binding(&PrivateFile::open(&path, 1024).unwrap()).unwrap();
    assert_ne!(
        (original.source_device, original.source_inode),
        (replacement.source_device, replacement.source_inode)
    );
    assert!(budget.resync_all().is_err());
    assert_eq!(retained.bytes(), bytes);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::read(&preserved).unwrap(), bytes);
}

type CasAuditRows = Vec<(i64, String, String, String, String)>;

fn cas_audit_rows(db: &rusqlite::Connection, sql: &str) -> CasAuditRows {
    let mut statement = db.prepare(sql).unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[derive(Debug, PartialEq, Eq)]
struct StoreCasObservation {
    revision: i64,
    projection: String,
    projection_hash: String,
    audit: CasAuditRows,
    floors: Vec<(i64, String)>,
}
fn observe_store_cas(db: &rusqlite::Connection) -> StoreCasObservation {
    let (revision,projection,projection_hash)=db.query_row("SELECT incarnation_revision,snapshot_json,snapshot_hash FROM review_authority_meta WHERE singleton=1",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
    let audit = cas_audit_rows(
        db,
        "SELECT seq,received_at,prev_hash,hash,payload FROM events ORDER BY seq",
    );
    let mut statement = db
        .prepare("SELECT event_seq,max_received_at FROM audit_time_floors ORDER BY event_seq")
        .unwrap();
    let floors = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    StoreCasObservation {
        revision,
        projection,
        projection_hash,
        audit,
        floors,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct AuthCasObservation {
    projection: String,
    projection_hash: String,
    wall_floor: String,
    audit: CasAuditRows,
    quota: (i64, i64),
}
fn observe_auth_cas(db: &rusqlite::Connection) -> AuthCasObservation {
    let (projection,projection_hash,wall_floor)=db.query_row("SELECT snapshot_json,snapshot_hash,wall_floor FROM authentication_source_authority WHERE singleton=1",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
    let audit = cas_audit_rows(
        db,
        "SELECT seq,received_at,prev_hash,hash,payload FROM authentication_entries ORDER BY seq",
    );
    let quota = db
        .query_row(
            "SELECT records,bytes FROM authentication_quota WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    AuthCasObservation {
        projection,
        projection_hash,
        wall_floor,
        audit,
        quota,
    }
}

fn assert_cas_projection_current(
    projection: &str,
    owner: &crate::human_authority::OwnedHumanSources,
    revision: u64,
) {
    let snapshot: serde_json::Value = serde_json::from_str(projection).unwrap();
    assert_eq!(
        snapshot["incarnation"][0]["owner_incarnation"],
        serde_json::json!(owner.incarnation())
    );
    assert_eq!(
        snapshot["incarnation"][0]["revision"],
        serde_json::json!(revision)
    );
}

#[tokio::test]
async fn store14_startup_stale_cas_refuses_without_changing_revision_head_incarnation_or_floor() {
    use crate::human_authority::{OwnedHumanSources, startup::HumanOwnedSourceOptions};
    let fixture = super::super::tests::Fixture::new_owned_local();
    // owned_request performs the real13->14 coordinator and returns its actual
    // PrivateFile/closure. No test writes marker, receipt or incarnation SQL.
    let (mut request, _, _) = fixture.owned_request();
    let descriptor = request
        .configuration
        .sources
        .iter()
        .find(|source| source.kind == HumanSourceKindV1::TenantStore)
        .unwrap()
        .clone();
    assert_eq!(descriptor.required_format, 14);
    let HumanOwnedSourceOptions::TenantStore {
        redactor,
        max_events,
        capacity,
        workload,
        retention,
    } = request.sources.remove(&descriptor.source_id).unwrap()
    else {
        panic!("tenant fixture option required")
    };
    let owner = OwnedHumanSources::acquire(request.manifest, request.configuration).unwrap();
    let source = owner.source_token(descriptor.source_id).unwrap();
    let mut startup = Store::open_review_startup_for_tenant(
        &descriptor.storage_binding.path(),
        descriptor.tenant_id.as_deref().unwrap(),
        redactor,
        max_events,
        &source,
    )
    .unwrap();
    startup
        .configure_owned_read_policies(capacity, workload, retention)
        .unwrap();
    let observer = rusqlite::Connection::open_with_flags(
        descriptor.storage_binding.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let predecessor = {
        let epoch = owner.try_write_epoch().unwrap();
        startup.current_incarnation_revision(&epoch).unwrap()
    };
    let accepted = {
        let epoch = owner.try_write_epoch().unwrap();
        startup
            .append_incarnation(&epoch, &source, Uuid::new_v4(), predecessor)
            .unwrap()
    };
    let current = predecessor.checked_add(1).unwrap();
    let before = observe_store_cas(&observer);
    let floor = startup.durable_wall_floor().unwrap();
    assert_eq!(u64::try_from(before.revision).unwrap(), current);
    assert_eq!(
        (
            before.audit.last().unwrap().0,
            before.audit.last().unwrap().3.as_str()
        ),
        (accepted.seq, accepted.hash.as_str())
    );
    assert_cas_projection_current(&before.projection, &owner, current);
    let stale_operation = Uuid::new_v4();
    {
        let epoch = owner.try_write_epoch().unwrap();
        epoch.validate_source(&source).unwrap();
        assert_eq!(
            startup.current_incarnation_revision(&epoch).unwrap(),
            current
        );
        let refusal = startup
            .append_incarnation(&epoch, &source, stale_operation, predecessor)
            .unwrap_err();
        assert_eq!(
            refusal.to_string(),
            "human source authority differs from durable source"
        );
        assert!(
            refusal
                .downcast_ref::<crate::human_authority::HumanAuthorityFailure>()
                .is_none()
        );
        epoch.validate_source(&source).unwrap();
    }
    assert_eq!(observe_store_cas(&observer), before);
    assert_eq!(startup.durable_wall_floor().unwrap(), floor);
    // Positive control with the SAME previously refused operation proves that
    // owner/source/gate/operation are valid: only the CAS predecessor changes.
    {
        let epoch = owner.try_write_epoch().unwrap();
        assert_eq!(
            startup.current_incarnation_revision(&epoch).unwrap(),
            current
        );
        let corrected = startup
            .append_incarnation(&epoch, &source, stale_operation, current)
            .unwrap();
        assert!(corrected.seq > accepted.seq);
        assert_eq!(
            startup.current_incarnation_revision(&epoch).unwrap(),
            current + 1
        );
    }
    drop(startup);
    drop(observer);
    owner.begin_close();
    owner.drain().await.unwrap();
}

#[tokio::test]
async fn auth5_startup_stale_cas_refuses_without_changing_revision_head_incarnation_or_floor() {
    use crate::human_authority::{OwnedHumanSources, startup::HumanOwnedSourceOptions};
    let fixture = super::super::tests::Fixture::new_owned();
    let (mut request, _, _) = fixture.owned_request();
    let descriptor = request
        .configuration
        .sources
        .iter()
        .find(|source| source.kind == HumanSourceKindV1::AuthenticationLedger)
        .unwrap()
        .clone();
    assert_eq!(descriptor.required_format, 5);
    let HumanOwnedSourceOptions::AuthenticationLedger {
        redactor,
        policy,
        capacity_plan,
    } = request.sources.remove(&descriptor.source_id).unwrap()
    else {
        panic!("authentication fixture option required")
    };
    let owner = OwnedHumanSources::acquire(request.manifest, request.configuration).unwrap();
    let source = owner.source_token(descriptor.source_id).unwrap();
    let startup = AuthenticationAudit::open_review_startup(
        &descriptor.storage_binding.path(),
        descriptor.installation_id,
        policy,
        redactor,
        &source,
    )
    .unwrap();
    startup.require_capacity_plan(&capacity_plan).unwrap();
    let observer = rusqlite::Connection::open_with_flags(
        descriptor.storage_binding.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let predecessor = {
        let epoch = owner.try_write_epoch().unwrap();
        startup
            .current_incarnation_revision(&epoch, &source)
            .unwrap()
    };
    let accepted = {
        let epoch = owner.try_write_epoch().unwrap();
        startup
            .append_incarnation(&epoch, &source, Uuid::new_v4(), predecessor)
            .unwrap()
    };
    let current = predecessor.checked_add(1).unwrap();
    let before = observe_auth_cas(&observer);
    let floor = startup.durable_wall_floor().unwrap();
    assert_eq!(
        (
            u64::try_from(before.audit.last().unwrap().0).unwrap(),
            before.audit.last().unwrap().3.as_str()
        ),
        (accepted.sequence, accepted.hash.as_str())
    );
    assert_cas_projection_current(&before.projection, &owner, current);
    let stale_operation = Uuid::new_v4();
    {
        let epoch = owner.try_write_epoch().unwrap();
        epoch.validate_source(&source).unwrap();
        assert_eq!(
            startup
                .current_incarnation_revision(&epoch, &source)
                .unwrap(),
            current
        );
        let refusal = startup
            .append_incarnation(&epoch, &source, stale_operation, predecessor)
            .unwrap_err();
        assert_eq!(
            refusal.to_string(),
            "authentication source authority differs from durable source"
        );
        assert!(
            refusal
                .downcast_ref::<crate::human_authority::HumanAuthorityFailure>()
                .is_none()
        );
        epoch.validate_source(&source).unwrap();
    }
    assert_eq!(observe_auth_cas(&observer), before);
    assert_eq!(startup.durable_wall_floor().unwrap(), floor);
    {
        let epoch = owner.try_write_epoch().unwrap();
        assert_eq!(
            startup
                .current_incarnation_revision(&epoch, &source)
                .unwrap(),
            current
        );
        let corrected = startup
            .append_incarnation(&epoch, &source, stale_operation, current)
            .unwrap();
        assert!(corrected.sequence > accepted.sequence);
        assert_eq!(
            startup
                .current_incarnation_revision(&epoch, &source)
                .unwrap(),
            current + 1
        );
    }
    drop(startup);
    drop(observer);
    owner.begin_close();
    owner.drain().await.unwrap();
}
