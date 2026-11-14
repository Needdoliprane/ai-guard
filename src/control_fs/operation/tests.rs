use super::*;
use crate::control_fs::cost_probe::{self, Measurement, Operation};
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
};

fn fixture() -> (tempfile::TempDir, Directory) {
    let temp = tempfile::Builder::new()
        .prefix("a1-operation-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let directory = Directory::open_existing(temp.path()).unwrap();
    (temp, directory)
}
fn prepare(parent: &Directory) {
    DurableOperationDirectory::prepare_new(
        parent,
        "operation",
        b"request",
        b"metadata",
        b"prepared",
    )
    .unwrap();
}
fn create(path: &Path, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

#[test]
fn reopen_syncs_required_optional_and_lock_fds_and_both_directories_under_real_flock() {
    let _gate = cost_probe::EXCLUSIVE.blocking_lock();
    let (temp, parent) = fixture();
    prepare(&parent);
    let path = temp.path().join("operation");
    create(&path.join("current-next.json"), b"pending");
    create(&path.join("receipt.json"), b"receipt");
    let probe = Measurement::start(temp.path()).unwrap();
    let operation = DurableOperationDirectory::open_existing(&path).unwrap();
    let measured = probe.finish().unwrap();
    assert_eq!(measured.metric(Operation::FileSyncAll).succeeded, 6);
    assert_eq!(measured.metric(Operation::DirectorySyncAll).succeeded, 2);
    assert_eq!(
        operation.record(OperationRecord::State).unwrap().bytes(),
        b"prepared"
    );
    assert_eq!(
        operation
            .record(OperationRecord::PendingState)
            .unwrap()
            .bytes(),
        b"pending"
    );
    assert_eq!(
        DurableOperationDirectory::open_existing(&path).err(),
        Some(OperationFsError::Busy)
    );
    assert_eq!(
        DurableOperationDirectory::open_existing(&path).err(),
        Some(OperationFsError::Busy)
    );
    drop(operation);
    // Child-only scope counts one directory sync, proving the second was its parent.
    let probe = Measurement::start(&path).unwrap();
    let reopened = DurableOperationDirectory::open_existing(&path).unwrap();
    let child_scope = probe.finish().unwrap();
    assert_eq!(child_scope.metric(Operation::FileSyncAll).succeeded, 6);
    assert_eq!(child_scope.metric(Operation::DirectorySyncAll).succeeded, 1);
    drop(reopened);
}

#[test]
fn injected_wrapper_error_after_real_reopen_parent_sync_refuses_without_claiming_fsync_errno() {
    let _gate = cost_probe::EXCLUSIVE.blocking_lock();
    let (temp, parent) = fixture();
    prepare(&parent);
    let path = temp.path().join("operation");
    let probe = Measurement::start(temp.path()).unwrap();
    FAIL_WRAPPER_AFTER_REAL_SYNC
        .with(|fault| fault.set(Some(Checkpoint::ReopenAfterRealParentSync)));
    assert_eq!(
        DurableOperationDirectory::open_existing(&path).err(),
        Some(OperationFsError::DurabilityUnknown)
    );
    let measured = probe.finish().unwrap();
    assert_eq!(measured.metric(Operation::FileSyncAll).succeeded, 4);
    assert_eq!(measured.metric(Operation::DirectorySyncAll).succeeded, 2);
    assert_eq!(fs::read(path.join("request.json")).unwrap(), b"request");
    // New explicit open establishes its own barrier; no failed object is healed.
    assert!(DurableOperationDirectory::open_existing(&path).is_ok());
}

#[test]
fn new_and_identical_receipt_acknowledgements_both_sync_file_and_directory() {
    let _gate = cost_probe::EXCLUSIVE.blocking_lock();
    let (temp, parent) = fixture();
    prepare(&parent);
    let mut operation =
        DurableOperationDirectory::open_existing(&temp.path().join("operation")).unwrap();
    for _ in 0..2 {
        let probe = Measurement::start(temp.path()).unwrap();
        operation.record_receipt(b"retained receipt bytes").unwrap();
        let measured = probe.finish().unwrap();
        assert_eq!(measured.metric(Operation::FileSyncAll).succeeded, 1);
        assert_eq!(measured.metric(Operation::DirectorySyncAll).succeeded, 1);
    }
}

#[test]
fn injected_wrapper_error_after_real_receipt_sync_taints_new_and_identical_branches() {
    let _gate = cost_probe::EXCLUSIVE.blocking_lock();
    for existing in [false, true] {
        let (temp, parent) = fixture();
        prepare(&parent);
        let path = temp.path().join("operation");
        let mut operation = DurableOperationDirectory::open_existing(&path).unwrap();
        if existing {
            operation.record_receipt(b"retained receipt bytes").unwrap();
        }
        let probe = Measurement::start(temp.path()).unwrap();
        FAIL_WRAPPER_AFTER_REAL_SYNC
            .with(|fault| fault.set(Some(Checkpoint::ReceiptAfterRealDirectorySync)));
        assert_eq!(
            operation.record_receipt(b"retained receipt bytes"),
            Err(OperationFsError::DurabilityUnknown)
        );
        let measured = probe.finish().unwrap();
        assert_eq!(measured.metric(Operation::FileSyncAll).succeeded, 1);
        assert_eq!(measured.metric(Operation::DirectorySyncAll).succeeded, 1);
        assert_eq!(
            operation.revalidate(),
            Err(OperationFsError::UnsafeOrIncomplete)
        );
        assert_eq!(
            operation.publish_state(b"later"),
            Err(OperationFsError::UnsafeOrIncomplete)
        );
        assert_eq!(
            fs::read(path.join("receipt.json")).unwrap(),
            b"retained receipt bytes"
        );
        drop(operation);
        let mut reopened = DurableOperationDirectory::open_existing(&path).unwrap();
        reopened.record_receipt(b"retained receipt bytes").unwrap();
    }
}

#[test]
fn pending_state_is_never_implicitly_published_and_only_explicit_recovery_moves_the_pointer() {
    let (temp, parent) = fixture();
    prepare(&parent);
    let path = temp.path().join("operation");
    create(&path.join("current-next.json"), b"next");
    let mut operation = DurableOperationDirectory::open_existing(&path).unwrap();
    assert_eq!(
        operation.record(OperationRecord::State).unwrap().bytes(),
        b"prepared"
    );
    assert_eq!(
        operation.publish_state(b"overwrite"),
        Err(OperationFsError::UnsafeOrIncomplete)
    );
    assert_eq!(fs::read(path.join("current-next.json")).unwrap(), b"next");
    drop(operation);
    let mut reopened = DurableOperationDirectory::open_existing(&path).unwrap();
    reopened.publish_retained_pending_state().unwrap();
    assert_eq!(
        reopened.record(OperationRecord::State).unwrap().bytes(),
        b"next"
    );
    assert!(reopened.record(OperationRecord::PendingState).is_none());
    assert_eq!(fs::read(path.join("request.json")).unwrap(), b"request");
}

#[test]
fn divergent_receipt_or_changed_ancestry_taints_without_overwrite_or_revalidation_repair() {
    let (temp, parent) = fixture();
    prepare(&parent);
    let path = temp.path().join("operation");
    let mut operation = DurableOperationDirectory::open_existing(&path).unwrap();
    operation.record_receipt(b"first").unwrap();
    assert_eq!(
        operation.record_receipt(b"different"),
        Err(OperationFsError::UnsafeOrIncomplete)
    );
    assert_eq!(
        operation.revalidate(),
        Err(OperationFsError::UnsafeOrIncomplete)
    );
    assert_eq!(fs::read(path.join("receipt.json")).unwrap(), b"first");
    drop(operation);
    let operation = DurableOperationDirectory::open_existing(&path).unwrap();
    fs::rename(&path, temp.path().join("retained")).unwrap();
    assert_eq!(
        operation.revalidate(),
        Err(OperationFsError::UnsafeOrIncomplete)
    );
    assert_eq!(
        operation.revalidate(),
        Err(OperationFsError::UnsafeOrIncomplete)
    );
    assert_eq!(
        fs::read(temp.path().join("retained/receipt.json")).unwrap(),
        b"first"
    );
}

#[test]
fn exclusive_preparation_preserves_existing_paths_and_real_ambiguous_publication() {
    let (temp, parent) = fixture();
    prepare(&parent);
    assert!(
        DurableOperationDirectory::prepare_new(&parent, "operation", b"other", b"other", b"other")
            .is_err()
    );
    assert_eq!(
        fs::read(temp.path().join("operation/request.json")).unwrap(),
        b"request"
    );
    super::super::publication::FAIL_AFTER_RENAME.with(|fault| fault.set(true));
    assert_eq!(
        DurableOperationDirectory::prepare_new(
            &parent,
            "uncertain",
            b"request",
            b"metadata",
            b"prepared"
        ),
        Err(OperationFsError::DurabilityUnknown)
    );
    assert!(temp.path().join("uncertain").is_dir());
    assert!(DurableOperationDirectory::open_existing(&temp.path().join("uncertain")).is_ok());
}

#[test]
fn oversized_unknown_or_symlink_records_are_refused_without_repair() {
    let (temp, parent) = fixture();
    assert!(
        DurableOperationDirectory::prepare_new(
            &parent,
            "large",
            &vec![b'x'; 65_537],
            b"metadata",
            b"prepared"
        )
        .is_err()
    );
    assert!(!temp.path().join("large").exists());
    prepare(&parent);
    let path = temp.path().join("operation");
    std::os::unix::fs::symlink(path.join("request.json"), path.join("receipt.json")).unwrap();
    assert!(DurableOperationDirectory::open_existing(&path).is_err());
    assert!(path.join("receipt.json").is_symlink());
    assert_eq!(fs::read(path.join("request.json")).unwrap(), b"request");
}

#[test]
fn containment_checks_retained_physical_ancestry_without_mutating_records() {
    let _gate = cost_probe::EXCLUSIVE.blocking_lock();
    let (temp, parent) = fixture();
    prepare(&parent);
    let path = temp.path().join("operation");
    let operation = DurableOperationDirectory::open_existing(&path).unwrap();
    let same = Directory::open_existing(&path).unwrap();
    let outside = parent.child("outside").unwrap();
    assert!(operation.contains_directory(&same).unwrap());
    assert!(!operation.contains_directory(&parent).unwrap());
    assert!(!operation.contains_directory(&outside).unwrap());
    operation.revalidate().unwrap();
    assert_eq!(fs::read(path.join("request.json")).unwrap(), b"request");
    assert_eq!(fs::read_dir(&path).unwrap().count(), 4);
}

#[test]
fn changed_candidate_ancestry_refuses_and_taints_the_retained_instance() {
    let _gate = cost_probe::EXCLUSIVE.blocking_lock();
    let (temp, parent) = fixture();
    prepare(&parent);
    let path = temp.path().join("operation");
    let operation = DurableOperationDirectory::open_existing(&path).unwrap();
    let outside = parent.child("outside").unwrap();
    fs::rename(
        temp.path().join("outside"),
        temp.path().join("moved-outside"),
    )
    .unwrap();
    assert_eq!(
        operation.contains_directory(&outside),
        Err(OperationFsError::UnsafeOrIncomplete)
    );
    fs::rename(
        temp.path().join("moved-outside"),
        temp.path().join("outside"),
    )
    .unwrap();
    assert_eq!(
        operation.revalidate(),
        Err(OperationFsError::UnsafeOrIncomplete)
    );
    assert_eq!(fs::read(path.join("request.json")).unwrap(), b"request");
    drop(operation);
    assert!(DurableOperationDirectory::open_existing(&path).is_ok());
}
