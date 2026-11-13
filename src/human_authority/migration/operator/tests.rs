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
