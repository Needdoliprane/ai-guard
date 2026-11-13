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
