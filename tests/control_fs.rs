#![cfg(unix)]

use agentsandbox::control_fs::{Directory, PrivateFile, absolute_path};
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = temporary.path().canonicalize().unwrap();
    (temporary, path)
}

fn private_file(path: &Path, bytes: &[u8]) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

#[test]
fn mutable_non_sticky_parent_is_refused_without_changing_existing_data() {
    let (_temporary, path) = fixture();
    let parent = path.join("shared");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o777)).unwrap();
    let root = parent.join("control");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    private_file(&root.join("existing"), b"preserved");
    assert!(Directory::open_existing(&root).is_err());
    assert!(Directory::open_or_create(&root).is_err());
    assert_eq!(fs::read(root.join("existing")).unwrap(), b"preserved");
    assert_eq!(
        fs::metadata(&parent).unwrap().permissions().mode() & 0o7777,
        0o777
    );
}

#[test]
fn group_writable_parent_is_refused_even_when_owned_by_control() {
    let (_temporary, path) = fixture();
    let parent = path.join("shared");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o770)).unwrap();
    assert!(Directory::open_or_create(&parent.join("control")).is_err());
    assert!(!parent.join("control").exists());
}

#[test]
fn root_sticky_parent_accepts_own_private_child() {
    use std::os::unix::fs::MetadataExt;
    let parent = Path::new("/tmp").canonicalize().unwrap();
    let facts = fs::metadata(&parent).unwrap();
    assert_eq!(facts.uid(), 0);
    assert_ne!(facts.permissions().mode() & 0o1000, 0);
    let temporary = tempfile::tempdir_in(&parent).unwrap();
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let directory = Directory::open_existing(temporary.path()).unwrap();
    directory.revalidate().unwrap();
}

#[test]
fn sticky_mutable_parent_exception_is_reserved_to_uid_zero() {
    let (_temporary, path) = fixture();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o1777)).unwrap();
    let result = Directory::open_or_create(&path.join("control"));
    assert_eq!(result.is_ok(), unsafe { libc::geteuid() } == 0);
}

#[test]
fn absolute_paths_normalize_only_lexical_current_directory_components() {
    let current = std::env::current_dir().unwrap();
    assert_eq!(
        absolute_path(Path::new("./config")).unwrap(),
        current.join("config")
    );
    assert!(absolute_path(Path::new("../config")).is_err());
    assert!(absolute_path(Path::new("/private/../config")).is_err());
    let (_temporary, path) = fixture();
    let target = path.join("target");
    Directory::open_or_create(&target).unwrap();
    std::os::unix::fs::symlink(&target, path.join("link")).unwrap();
    let normalized = absolute_path(&path.join("link")).unwrap();
    assert_eq!(normalized, path.join("link"));
    assert!(Directory::open_existing(&normalized).is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn absolute_paths_support_only_fixed_darwin_system_aliases() {
    assert_eq!(
        absolute_path(Path::new("/var/folders/test")).unwrap(),
        Path::new("/private/var/folders/test")
    );
    assert_eq!(
        absolute_path(Path::new("/tmp/test")).unwrap(),
        Path::new("/private/tmp/test")
    );
    assert_eq!(
        absolute_path(Path::new("/etc/test")).unwrap(),
        Path::new("/private/etc/test")
    );
    assert_eq!(
        absolute_path(Path::new("/variable/test")).unwrap(),
        Path::new("/variable/test")
    );
}

#[test]
fn links_and_traversal_are_refused_for_ancestors_and_leaves() {
    let (_temporary, path) = fixture();
    let real = path.join("real");
    Directory::open_or_create(&real).unwrap();
    std::os::unix::fs::symlink(&real, path.join("alias")).unwrap();
    assert!(Directory::open_existing(&path.join("alias")).is_err());
    assert!(Directory::open_or_create(&path.join("alias/child")).is_err());
    assert!(Directory::open_or_create(&path.join("real/../escape")).is_err());
    private_file(&real.join("source"), b"preserved");
    std::os::unix::fs::symlink("source", real.join("link")).unwrap();
    assert!(PrivateFile::open(&real.join("link"), 1024).is_err());
    assert!(PrivateFile::open(&path.join("alias/source"), 1024).is_err());
    assert_eq!(fs::read(real.join("source")).unwrap(), b"preserved");
}

#[test]
fn wrong_modes_hardlinks_and_fifo_are_refused_before_reading() {
    let (_temporary, path) = fixture();
    let directory = Directory::open_existing(&path).unwrap();
    private_file(&path.join("readable"), b"not private");
    fs::set_permissions(path.join("readable"), fs::Permissions::from_mode(0o640)).unwrap();
    assert!(directory.read_private_file("readable", 1024).is_err());
    private_file(&path.join("linked"), b"hardlink");
    fs::hard_link(path.join("linked"), path.join("second")).unwrap();
    assert!(directory.read_private_file("linked", 1024).is_err());
    let fifo = CString::new(path.join("fifo").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let started = Instant::now();
    assert!(directory.read_private_file("fifo", 1024).is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
    let nonprivate = path.join("nonprivate");
    fs::create_dir(&nonprivate).unwrap();
    fs::set_permissions(&nonprivate, fs::Permissions::from_mode(0o750)).unwrap();
    assert!(Directory::open_existing(&nonprivate).is_err());
    assert_eq!(
        fs::metadata(&nonprivate).unwrap().permissions().mode() & 0o777,
        0o750
    );
}

#[test]
fn source_read_is_bounded_and_uses_validated_fd_bytes() {
    let (_temporary, path) = fixture();
    let source = path.join("source");
    private_file(&source, b"exact bytes");
    assert!(PrivateFile::open(&source, 10).is_err());
    let file = PrivateFile::open(&source, 11).unwrap();
    assert_eq!(file.bytes(), b"exact bytes");
    assert_eq!(file.digest(), hex::encode(Sha256::digest(b"exact bytes")));
    fs::rename(&source, path.join("previous")).unwrap();
    private_file(&source, b"replacement");
    assert_eq!(file.bytes(), b"exact bytes");
    assert!(file.revalidate().is_err());
    assert!(!format!("{file:?}").contains("exact bytes"));
    drop(file);
    assert_eq!(fs::read(source).unwrap(), b"replacement");
}

#[test]
fn snapshots_keep_prepared_digest_and_are_shared_until_last_handle() {
    let (_temporary, path) = fixture();
    let directory = Directory::open_existing(&path).unwrap();
    let snapshot = directory.snapshot(b"KEY=prepared\n").unwrap();
    let shared = snapshot.clone();
    let snapshot_path = snapshot.path().to_owned();
    assert_eq!(fs::read(&snapshot_path).unwrap(), b"KEY=prepared\n");
    assert_eq!(
        snapshot.digest(),
        hex::encode(Sha256::digest(b"KEY=prepared\n"))
    );
    snapshot.revalidate().unwrap();
    assert_eq!(
        fs::metadata(&snapshot_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!format!("{snapshot:?}").contains("prepared"));
    drop(snapshot);
    assert!(snapshot_path.exists());
    drop(shared);
    assert!(!snapshot_path.exists());
}

#[test]
fn snapshot_cleanup_uses_retained_directory_and_preserves_a_path_successor() {
    let (_temporary, path) = fixture();
    let root = path.join("control");
    let directory = Directory::open_or_create(&root).unwrap();
    let snapshot = directory.snapshot(b"created").unwrap();
    let name = snapshot.path().file_name().unwrap().to_owned();
    let moved = path.join("moved");
    fs::rename(&root, &moved).unwrap();
    Directory::open_or_create(&root).unwrap();
    private_file(&root.join(&name), b"successor");
    assert!(directory.revalidate().is_err());
    assert!(snapshot.revalidate().is_err());
    drop(snapshot);
    assert!(!moved.join(&name).exists());
    assert_eq!(fs::read(root.join(&name)).unwrap(), b"successor");
}

#[test]
fn snapshot_cleanup_does_not_remove_replacement_in_the_retained_directory() {
    let (_temporary, path) = fixture();
    let directory = Directory::open_existing(&path).unwrap();
    let snapshot = directory.snapshot(b"created").unwrap();
    let snapshot_path = snapshot.path().to_owned();
    fs::rename(&snapshot_path, path.join("preserved-original")).unwrap();
    private_file(&snapshot_path, b"replacement");
    assert!(snapshot.revalidate().is_err());
    assert_eq!(snapshot.digest(), hex::encode(Sha256::digest(b"created")));
    drop(snapshot);
    assert_eq!(fs::read(snapshot_path).unwrap(), b"replacement");
    assert_eq!(
        fs::read(path.join("preserved-original")).unwrap(),
        b"created"
    );
}

#[test]
fn initialization_is_idempotent_and_never_repairs_existing_modes() {
    let (_temporary, path) = fixture();
    let root = path.join("new/parent/control");
    let directory = Directory::open_or_create(&root).unwrap();
    private_file(&root.join("data"), b"existing");
    let child = directory.child("snapshots").unwrap();
    assert_eq!(child.path(), root.join("snapshots"));
    Directory::open_or_create(&root)
        .unwrap()
        .revalidate()
        .unwrap();
    assert_eq!(fs::read(root.join("data")).unwrap(), b"existing");
    assert!(directory.child("../escape").is_err());
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Directory::open_or_create(&root).is_err());
    assert_eq!(
        fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[cfg(target_os = "macos")]
fn chmod_acl(path: &Path, acl: &str) {
    let status = std::process::Command::new("/bin/chmod")
        .arg("+a")
        .arg(acl)
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

#[cfg(target_os = "macos")]
#[test]
fn ancestor_deny_and_read_only_acls_are_accepted_but_mutating_acl_is_refused() {
    let (_temporary, path) = fixture();
    let root = path.join("control");
    Directory::open_or_create(&root).unwrap();
    chmod_acl(&path, "everyone deny delete");
    Directory::open_existing(&root).unwrap();
    chmod_acl(&path, "everyone allow list,search,readattr,readsecurity");
    Directory::open_existing(&root).unwrap();
    chmod_acl(&path, "everyone allow delete_child");
    assert!(Directory::open_existing(&root).is_err());
    // Remove only the ACL on this owned test fixture so TempDir can clean up.
    assert!(
        std::process::Command::new("/bin/chmod")
            .arg("-N")
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn private_directory_and_file_with_any_extended_acl_are_refused_and_preserved() {
    let (_temporary, path) = fixture();
    let source = path.join("source");
    private_file(&source, b"preserved");
    chmod_acl(&source, "everyone allow read");
    assert!(PrivateFile::open(&source, 1024).is_err());
    assert_eq!(fs::read(&source).unwrap(), b"preserved");
    chmod_acl(&path, "everyone deny delete");
    assert!(Directory::open_existing(&path).is_err());
    assert!(
        std::process::Command::new("/bin/chmod")
            .arg("-N")
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_named_posix_acl_is_bounded_by_the_mode_mask() {
    use std::os::unix::fs::MetadataExt;
    let (_temporary, path) = fixture();
    let source = path.join("source");
    private_file(&source, b"preserved");
    let encoded = CString::new(source.as_os_str().as_bytes()).unwrap();
    let attribute = c"system.posix_acl_access";
    let acl = |mask: u16| {
        let mut bytes = 2_u32.to_le_bytes().to_vec();
        // Linux POSIX ACL xattr format: tag, permissions, optional numeric ID.
        for (tag, permissions, id) in [
            (1_u16, 6_u16, u32::MAX),
            (2, 6, unsafe { libc::geteuid() }.saturating_add(1)),
            (4, 0, u32::MAX),
            (16, mask, u32::MAX),
            (32, 0, u32::MAX),
        ] {
            bytes.extend(tag.to_le_bytes());
            bytes.extend(permissions.to_le_bytes());
            bytes.extend(id.to_le_bytes());
        }
        bytes
    };
    let set_acl = |bytes: &[u8]| {
        assert_eq!(
            unsafe {
                libc::setxattr(
                    encoded.as_ptr(),
                    attribute.as_ptr(),
                    bytes.as_ptr().cast(),
                    bytes.len(),
                    0,
                )
            },
            0,
            "owned test filesystem must support the POSIX ACL scenario: {}",
            std::io::Error::last_os_error()
        );
    };
    let masked = acl(0);
    set_acl(&masked);
    assert_eq!(fs::metadata(&source).unwrap().mode() & 0o777, 0o600);
    let file = PrivateFile::open(&source, 1024).unwrap();
    assert_eq!(file.bytes(), b"preserved");
    drop(file);
    let mut preserved = vec![0_u8; masked.len()];
    assert_eq!(
        unsafe {
            libc::getxattr(
                encoded.as_ptr(),
                attribute.as_ptr(),
                preserved.as_mut_ptr().cast(),
                preserved.len(),
            )
        },
        masked.len() as isize
    );
    assert_eq!(
        preserved, masked,
        "opening must never strip or rewrite ACLs"
    );
    set_acl(&acl(4));
    assert_eq!(fs::metadata(&source).unwrap().mode() & 0o777, 0o640);
    assert!(PrivateFile::open(&source, 1024).is_err());
    assert_eq!(fs::read(source).unwrap(), b"preserved");
}
