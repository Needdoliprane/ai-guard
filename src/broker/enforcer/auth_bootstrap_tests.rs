//! Private test roots and a real held kernel lease; no helper is spawned.

use super::{EnforcerClient, EnforcerError, capability};
use crate::{
    broker::{
        discovery::{RoleLease, load_endpoint},
        local_ipc::IpcError,
    },
    control_fs::Directory,
};
use std::{
    ffi::OsStr,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
};

fn new_role_root() -> (tempfile::TempDir, Directory) {
    let temporary = tempfile::Builder::new()
        .prefix("ai-guard-auth-bootstrap-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let path = temporary.path().canonicalize().unwrap();
    let root = Directory::open_existing(&path).unwrap();
    (temporary, root)
}

#[tokio::test(flavor = "current_thread")]
async fn recovery_auth_does_not_regenerate_existing_authority() {
    let (_temporary, root) = new_role_root();
    let lease = RoleLease::acquire(&root).unwrap();
    assert!(
        root.private_entry(OsStr::new(".owner.lock"))
            .unwrap()
            .is_some(),
        "the fixture must have an existing role lock"
    );
    assert!(root.private_entry(OsStr::new("auth")).unwrap().is_none());
    assert!(load_endpoint(&root).unwrap().is_none());
    // The client probes this same independently opened flock before spawning.
    // Retaining lease through the call makes that path fail before any child.
    assert!(matches!(RoleLease::acquire(&root), Err(IpcError::Conflict)));
    let executable = std::env::current_exe().unwrap();
    assert!(executable.is_absolute());

    let result = EnforcerClient::connect_or_spawn(&root, &executable, 8).await;

    assert!(
        root.private_entry(OsStr::new("auth")).unwrap().is_none(),
        "recovery must not mint authentication for an existing role"
    );
    assert!(
        result.is_err(),
        "an unauthenticated occupied role is refused"
    );
    assert!(matches!(RoleLease::acquire(&root), Err(IpcError::Conflict)));
    drop(lease);
}

#[test]
fn recovery_auth_fresh_bootstrap_preserves_the_existing_capability_and_file() {
    let (_temporary, root) = new_role_root();
    assert!(root.persistent_entries(1).unwrap().is_empty());
    assert!(matches!(
        capability(&root, false),
        Err(EnforcerError::Unauthorized)
    ));
    assert!(root.private_entry(OsStr::new("auth")).unwrap().is_none());

    let original = capability(&root, true).unwrap();
    let path = root.path().join("auth");
    let before = fs::metadata(&path).unwrap();
    assert_eq!(before.len(), 32);
    assert_eq!(before.mode() & 0o7777, 0o600);
    // Compare internally: an assertion failure must never print auth bytes.
    assert!(original == capability(&root, false).unwrap());
    assert!(original == capability(&root, true).unwrap());
    let after = fs::metadata(&path).unwrap();
    assert_eq!(
        (
            before.dev(),
            before.ino(),
            before.mtime(),
            before.mtime_nsec(),
            before.ctime(),
            before.ctime_nsec(),
        ),
        (
            after.dev(),
            after.ino(),
            after.mtime(),
            after.mtime_nsec(),
            after.ctime(),
            after.ctime_nsec(),
        ),
        "reading an existing capability must not replace or rewrite its file"
    );
}
