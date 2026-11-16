use agentsandbox::runtime::{
    ContainerPurpose, ContainerSpec, NetworkMode, RuntimeKind, capture_stream, parse_runtime_state,
};
use agentsandbox::workspace::{
    Limits, extract_workspace, prepare_workspace, validate_relative_path,
};
use std::{fs, io::Cursor, path::Path};
use uuid::Uuid;

#[test]
fn every_worker_creation_requires_an_existing_local_image() {
    let image = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    for kind in [RuntimeKind::Docker, RuntimeKind::Podman] {
        for purpose in [
            ContainerPurpose::Import,
            ContainerPurpose::Run,
            ContainerPurpose::Export,
        ] {
            let spec = ContainerSpec::new(
                Uuid::new_v4(),
                purpose,
                image,
                vec!["/bin/echo".into(), "--pull=always".into()],
                NetworkMode::None,
            )
            .unwrap();
            let args = spec.create_args(kind).unwrap();
            let end = args.iter().position(|arg| arg == "--").unwrap();
            assert_eq!(&args[..2], ["create", "--pull=never"]);
            assert_eq!(
                args[..end]
                    .iter()
                    .filter(|arg| arg.starts_with("--pull"))
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                ["--pull=never"],
                "no later creation option may override local-image-only policy"
            );
            // The entrypoint is a creation option; the command's pull-looking
            // argument follows IMAGE and remains data.
            assert_eq!(args.len(), end + 3);
            assert_eq!(args[end + 1], image);
            assert_eq!(args[end + 2], "--pull=always");
        }
    }
}

#[test]
fn worker_arguments_confine_a_named_volume_and_preserve_literal_arguments() {
    let session = Uuid::parse_str("bd249f63-d29e-4c62-a7ef-eb23511ca8de").unwrap();
    let spec = ContainerSpec::new(
        session,
        ContainerPurpose::Run,
        "agentsandbox-worker:local",
        vec![
            "/bin/echo".into(),
            "$(touch /tmp/host); --privileged".into(),
        ],
        NetworkMode::None,
    )
    .unwrap();
    let args = spec.create_args(RuntimeKind::Docker).unwrap();
    for pair in [
        ["--network", "none"],
        ["--cap-drop", "ALL"],
        ["--security-opt", "no-new-privileges"],
        ["--user", "1000:1000"],
        ["--pids-limit", "256"],
        ["--memory", "1g"],
        ["--cpus", "2"],
    ] {
        assert!(args.windows(2).any(|a| a == pair));
    }
    assert!(args.iter().any(|a| a == "--read-only"));
    assert!(args.iter().any(|a| a == "type=volume,source=agentsandbox-work-bd249f63-d29e-4c62-a7ef-eb23511ca8de,target=/workspace"));
    assert_eq!(args.last().unwrap(), "$(touch /tmp/host); --privileged");
    assert!(
        !args.iter().any(|a| a == "--privileged"
            || a.starts_with("type=bind")
            || a.contains("docker.sock"))
    );
}

#[test]
fn runtime_rejects_option_injection_and_invalid_program_paths() {
    for image in ["--privileged", "worker image", "", "x\n--network=host"] {
        assert!(
            ContainerSpec::new(
                Uuid::new_v4(),
                ContainerPurpose::Run,
                image,
                vec!["echo".into()],
                NetworkMode::None
            )
            .is_err()
        );
    }
    for program in [
        "",
        "--privileged",
        "../bin/sh",
        "/bin/../bin/sh",
        "bad\0arg",
    ] {
        assert!(
            ContainerSpec::new(
                Uuid::new_v4(),
                ContainerPurpose::Run,
                "worker:local",
                vec![program.into()],
                NetworkMode::None
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn output_is_drained_but_retained_bytes_are_bounded() {
    let bytes = vec![b'x'; 2048];
    let result = capture_stream(bytes.as_slice(), 33).await.unwrap();
    assert_eq!(result.bytes.len(), 33);
    assert_eq!(result.total_bytes, 2048);
    assert!(result.truncated);
}

#[test]
fn container_start_requires_runtime_started_at_evidence() {
    let created = parse_runtime_state(
        br#"{"Status":"created","Running":false,"StartedAt":"0001-01-01T00:00:00Z","ExitCode":0}"#,
    )
    .unwrap();
    assert!(!created.has_started());
    let exited = parse_runtime_state(br#"{"Status":"exited","Running":false,"StartedAt":"2026-10-29T10:42:00.000Z","ExitCode":0}"#).unwrap();
    assert!(exited.has_started());
}

#[test]
fn project_copy_excludes_credentials_and_git_and_preserves_safe_files() {
    let source = tempfile::tempdir().unwrap();
    fs::create_dir(source.path().join("src")).unwrap();
    fs::create_dir(source.path().join(".git")).unwrap();
    fs::create_dir(source.path().join("secrets")).unwrap();
    fs::write(source.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    for path in [
        ".env",
        ".env.production",
        ".npmrc",
        "private.key",
        ".git/config",
        "secrets/password",
    ] {
        fs::write(source.path().join(path), "SECRET").unwrap();
    }
    let prepared = prepare_workspace(source.path(), Limits::default()).unwrap();
    assert_eq!(prepared.manifest.len(), 1);
    assert!(prepared.manifest.contains_key("src/main.rs"));
    let mut archive = tar::Archive::new(prepared.archive.reopen().unwrap());
    let paths: Vec<_> = archive
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(paths.contains(&"src/main.rs".into()));
    assert!(!paths.iter().any(|p| p.contains(".env")
        || p.contains(".git")
        || p.contains("secret")
        || p.contains(".key")
        || p.contains(".npmrc")));
}

#[cfg(unix)]
#[test]
fn project_copy_rejects_symlinks_instead_of_following_them() {
    let source = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("/etc/passwd", source.path().join("innocent.txt")).unwrap();
    assert!(prepare_workspace(source.path(), Limits::default()).is_err());
}

#[cfg(unix)]
#[test]
fn project_copy_rejects_a_fifo_without_blocking_on_open() {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let source = tempfile::tempdir().unwrap();
    let name = CString::new(source.path().join("pipe").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = prepare_workspace(source.path(), Limits::default());
        let _ = send.send(result.is_err());
    });
    assert!(
        receive
            .recv_timeout(std::time::Duration::from_millis(300))
            .unwrap()
    );
}

#[test]
fn project_copy_rejects_size_and_file_count_overflow() {
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("large"), [0; 101]).unwrap();
    assert!(
        prepare_workspace(
            source.path(),
            Limits {
                max_files: 3,
                max_bytes: 100,
                max_file_bytes: 100
            }
        )
        .is_err()
    );
    fs::write(source.path().join("large"), "x").unwrap();
    fs::write(source.path().join("second"), "y").unwrap();
    assert!(
        prepare_workspace(
            source.path(),
            Limits {
                max_files: 1,
                max_bytes: 100,
                max_file_bytes: 100
            }
        )
        .is_err()
    );
}

#[test]
fn custom_config_location_does_not_copy_mounted_secret_projections() {
    let source = tempfile::tempdir().unwrap();
    fs::create_dir_all(source.path().join("custom-config/mounted-secrets")).unwrap();
    fs::write(
        source.path().join("custom-config/mounted-secrets/runner"),
        "private-runtime-key",
    )
    .unwrap();
    fs::write(source.path().join("source.rs"), "fn main() {}\n").unwrap();
    let prepared = prepare_workspace(source.path(), Limits::default()).unwrap();
    assert!(prepared.manifest.contains_key("source.rs"));
    assert!(
        !prepared
            .manifest
            .contains_key("custom-config/mounted-secrets/runner"),
        "excluded={:?}; manifest paths={:?}",
        prepared.excluded,
        prepared.manifest.keys().collect::<Vec<_>>()
    );
}

fn archive_entry(path: &str, entry_type: u8, data: &[u8]) -> Vec<u8> {
    let mut header = tar::Header::new_gnu();
    header.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
    header.set_entry_type(tar::EntryType::new(entry_type));
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    let mut builder = tar::Builder::new(Vec::new());
    builder.append(&header, data).unwrap();
    builder.into_inner().unwrap()
}

#[test]
fn archive_paths_reject_traversal_absolute_and_windows_variants() {
    for path in [
        "../escape",
        "/etc/passwd",
        "a/../../escape",
        "C:\\secrets",
        "a\\..\\secret",
    ] {
        assert!(
            validate_relative_path(Path::new(path)).is_err(),
            "accepted {path}"
        );
        let parent = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let dest = parent.path().join("out");
        assert!(
            extract_workspace(
                Cursor::new(archive_entry(path, b'0', b"x")),
                &dest,
                source.path(),
                Limits::default()
            )
            .is_err()
        );
        assert!(!dest.exists());
    }
}

#[test]
fn export_rejects_links_special_files_and_original_destination() {
    for kind in *b"12346S" {
        let parent = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let dest = parent.path().join("out");
        assert!(
            extract_workspace(
                Cursor::new(archive_entry("danger", kind, b"")),
                &dest,
                source.path(),
                Limits::default()
            )
            .is_err()
        );
        assert!(!dest.exists());
    }
    let source = tempfile::tempdir().unwrap();
    assert!(
        extract_workspace(
            Cursor::new(archive_entry("new.txt", b'0', b"safe")),
            &source.path().join("export"),
            source.path(),
            Limits::default()
        )
        .is_err()
    );
}

#[test]
fn valid_export_is_atomic_and_only_writes_the_new_destination() {
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("original.txt"), "unchanged").unwrap();
    let parent = tempfile::tempdir().unwrap();
    let dest = parent.path().join("result");
    let manifest = extract_workspace(
        Cursor::new(archive_entry("nested/result.txt", b'0', b"result")),
        &dest,
        source.path(),
        Limits::default(),
    )
    .unwrap();
    assert_eq!(fs::read(dest.join("nested/result.txt")).unwrap(), b"result");
    assert_eq!(manifest["nested/result.txt"].bytes, 6);
    assert_eq!(
        fs::read(source.path().join("original.txt")).unwrap(),
        b"unchanged"
    );
}
