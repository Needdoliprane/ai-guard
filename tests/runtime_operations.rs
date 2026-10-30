use agentsandbox::operations::{initialize_config, validate_provider_version};
use std::fs;

#[cfg(target_os = "macos")]
#[test]
fn init_refuses_extended_source_acl_before_refreshing_projections_without_removing_acl() {
    use std::os::unix::fs::MetadataExt;
    let temporary = tempfile::tempdir().unwrap();
    let config = temporary.path().join("config");
    initialize_config(&config).unwrap();
    let source = config.join("secrets/runner");
    let projected = config.join("mounted-secrets/runner");
    let original = fs::read(&source).unwrap();
    let projected_identity = fs::metadata(&projected).unwrap().ino();
    let result = std::process::Command::new("/bin/chmod")
        .args(["+a", "everyone allow read"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "owned ACL test fixture could not be prepared"
    );
    assert!(
        initialize_config(&config).is_err(),
        "initialization accepted a source its private reader rejects"
    );
    assert!(
        fs::read(&source).unwrap() == original,
        "initialization changed an existing source value"
    );
    assert_eq!(
        fs::metadata(&projected).unwrap().ino(),
        projected_identity,
        "projection refreshed despite source rejection"
    );
    let acl = std::process::Command::new("/bin/ls")
        .args(["-le"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(acl.status.success());
    assert!(
        String::from_utf8(acl.stdout)
            .unwrap()
            .contains("everyone allow read"),
        "initialization silently removed an ACL"
    );
}

#[test]
fn init_generates_distinct_32_byte_keys_and_never_replaces_them() {
    let parent = tempfile::tempdir().unwrap();
    let config = parent.path().join("config");
    initialize_config(&config).unwrap();
    let runner = fs::read_to_string(config.join("secrets/runner")).unwrap();
    let viewer = fs::read_to_string(config.join("secrets/viewer")).unwrap();
    assert_eq!(hex::decode(runner.trim()).unwrap().len(), 32);
    assert_eq!(hex::decode(viewer.trim()).unwrap().len(), 32);
    assert_ne!(runner, viewer);
    initialize_config(&config).unwrap();
    assert_eq!(
        fs::read_to_string(config.join("secrets/runner")).unwrap(),
        runner
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(config.join("secrets"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(config.join("secrets/runner"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[cfg(unix)]
#[test]
fn mount_projections_are_separate_readable_files_inside_a_private_directory() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let parent = tempfile::tempdir().unwrap();
    let config = parent.path().join("config");
    initialize_config(&config).unwrap();
    assert_eq!(
        fs::metadata(config.join("mounted-secrets"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    for role in ["runner", "viewer"] {
        let source = config.join("secrets").join(role);
        let projected = config.join("mounted-secrets").join(role);
        assert_eq!(fs::read(&source).unwrap(), fs::read(&projected).unwrap());
        let source_metadata = fs::metadata(source).unwrap();
        let projected_metadata = fs::metadata(projected).unwrap();
        assert_eq!(source_metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(projected_metadata.permissions().mode() & 0o777, 0o444);
        assert_ne!(source_metadata.ino(), projected_metadata.ino());
        assert_eq!(projected_metadata.nlink(), 1);
    }
}

#[cfg(unix)]
#[test]
fn secret_projection_refresh_is_atomic_and_never_replaces_source_values() {
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    let parent = tempfile::tempdir().unwrap();
    let config = parent.path().join("config");
    initialize_config(&config).unwrap();
    let source = config.join("secrets/runner");
    let projected = config.join("mounted-secrets/runner");
    let previous = fs::read_to_string(&source).unwrap();
    let mut old_mount = fs::File::open(&projected).unwrap();
    let replacement = format!("{}\n", "a".repeat(64));
    fs::write(&source, &replacement).unwrap();
    initialize_config(&config).unwrap();
    let mut still_mounted = String::new();
    old_mount.read_to_string(&mut still_mounted).unwrap();
    assert_eq!(
        still_mounted, previous,
        "an existing bind must keep its old inode until recreation"
    );
    assert_eq!(fs::read_to_string(&projected).unwrap(), replacement);
    assert_eq!(fs::read_to_string(&source).unwrap(), replacement);
    assert_eq!(
        fs::metadata(source).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[cfg(unix)]
#[test]
fn optional_jev_is_projected_only_when_a_private_source_exists() {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let parent = tempfile::tempdir().unwrap();
    let config = parent.path().join("config");
    initialize_config(&config).unwrap();
    assert!(!config.join("secrets/jev").exists());
    assert!(!config.join("mounted-secrets/jev").exists());
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(config.join("secrets/jev"))
        .unwrap();
    writeln!(file, "{}", "j".repeat(64)).unwrap();
    drop(file);
    initialize_config(&config).unwrap();
    assert_eq!(
        fs::read(config.join("secrets/jev")).unwrap(),
        fs::read(config.join("mounted-secrets/jev")).unwrap()
    );
    assert_eq!(
        fs::metadata(config.join("mounted-secrets/jev"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o444
    );
    let original = fs::read(config.join("secrets/jev")).unwrap();
    initialize_config(&config).unwrap();
    assert_eq!(fs::read(config.join("secrets/jev")).unwrap(), original);
    assert_eq!(
        fs::read(config.join("mounted-secrets/jev")).unwrap(),
        original
    );
    fs::remove_file(config.join("secrets/jev")).unwrap();
    initialize_config(&config).unwrap();
    assert!(
        !config.join("mounted-secrets/jev").exists(),
        "a removed optional source must not leave a stale mount projection"
    );
}

#[cfg(unix)]
#[test]
fn init_refuses_jev_control_key_reuse_without_refreshing_any_projection() {
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    for reused_role in ["runner", "viewer"] {
        let parent = tempfile::tempdir().unwrap();
        let config = parent.path().join("config");
        initialize_config(&config).unwrap();
        let jev_source = config.join("secrets/jev");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&jev_source)
            .unwrap();
        writeln!(file, "{}", "j".repeat(48)).unwrap();
        drop(file);
        initialize_config(&config).unwrap();
        let reused = fs::read(config.join("secrets").join(reused_role)).unwrap();
        fs::write(&jev_source, &reused).unwrap();

        let existing: Vec<_> = ["secrets", "mounted-secrets"]
            .into_iter()
            .flat_map(|directory| {
                ["runner", "viewer", "jev"]
                    .into_iter()
                    .map(move |role| (directory, role))
            })
            .map(|(directory, role)| {
                let path = config.join(directory).join(role);
                let bytes = fs::read(&path).unwrap();
                let identity = fs::metadata(&path).unwrap().ino();
                (path, bytes, identity)
            })
            .collect();

        let result = initialize_config(&config);
        assert!(result.is_err(), "Jev must not reuse a control credential");
        let diagnostic = format!("{:#}", result.unwrap_err());
        assert!(diagnostic.contains("Jev") && diagnostic.contains("distinct"));
        for (path, bytes, identity) in existing {
            assert!(fs::read(&path).unwrap() == bytes, "secret content changed");
            assert_eq!(fs::metadata(&path).unwrap().ino(), identity);
            let value = String::from_utf8(bytes).unwrap();
            assert!(
                !diagnostic.contains(value.trim()),
                "credential in diagnostic"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn public_projection_directory_and_linked_projection_are_rejected() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let parent = tempfile::tempdir().unwrap();
    let config = parent.path().join("config");
    initialize_config(&config).unwrap();
    let mounted = config.join("mounted-secrets");
    fs::create_dir_all(&mounted).unwrap();
    fs::set_permissions(&mounted, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(initialize_config(&config).is_err());
    fs::set_permissions(&mounted, fs::Permissions::from_mode(0o700)).unwrap();
    let projected = mounted.join("runner");
    if projected.exists() {
        fs::remove_file(&projected).unwrap();
    }
    let victim = parent.path().join("keep.txt");
    fs::write(&victim, "must remain untouched").unwrap();
    symlink(&victim, &projected).unwrap();
    assert!(initialize_config(&config).is_err());
    assert_eq!(fs::read_to_string(victim).unwrap(), "must remain untouched");
}

#[cfg(unix)]
#[test]
fn legacy_readable_sources_are_hardened_without_rotation() {
    use std::os::unix::fs::PermissionsExt;
    let parent = tempfile::tempdir().unwrap();
    let config = parent.path().join("config");
    initialize_config(&config).unwrap();
    let source = config.join("secrets/runner");
    let original = fs::read(&source).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o444)).unwrap();
    initialize_config(&config).unwrap();
    assert_eq!(fs::read(&source).unwrap(), original);
    assert_eq!(
        fs::metadata(&source).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[cfg(unix)]
#[test]
fn init_refuses_existing_public_secret_directory_and_symlink() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let parent = tempfile::tempdir().unwrap();
    let config = parent.path().join("public");
    fs::create_dir_all(config.join("secrets")).unwrap();
    fs::set_permissions(config.join("secrets"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(initialize_config(&config).is_err());
    let config = parent.path().join("linked");
    fs::create_dir(&config).unwrap();
    symlink(parent.path(), config.join("secrets")).unwrap();
    assert!(initialize_config(&config).is_err());
}

#[test]
fn optional_provider_build_rejects_latest_ranges_and_shell_arguments() {
    for version in [
        "latest",
        "*",
        "^1.2.3",
        "1.2",
        "1.2.3 --ignore-scripts",
        "1.2.3; touch /tmp/x",
        "",
    ] {
        assert!(
            validate_provider_version(version).is_err(),
            "accepted {version}"
        );
    }
    for version in ["1.2.3", "0.10.0-beta.1"] {
        assert!(validate_provider_version(version).is_ok());
    }
}
