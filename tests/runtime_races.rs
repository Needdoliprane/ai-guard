#[cfg(unix)]
mod unix {
    use agentsandbox::{
        runtime::{ContainerPurpose, ContainerSpec, NetworkMode, Runtime, RuntimeKind},
        workspace::{Limits, extract_workspace},
    };
    use std::{
        fs,
        io::{Cursor, Read},
        path::PathBuf,
        time::Duration,
    };

    struct ParentSwap {
        inner: Cursor<Vec<u8>>,
        parent: PathBuf,
        parked: PathBuf,
        original: PathBuf,
        swapped: bool,
    }
    impl Read for ParentSwap {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if !self.swapped {
                fs::rename(&self.parent, &self.parked)?;
                std::os::unix::fs::symlink(&self.original, &self.parent)?;
                self.swapped = true;
            }
            self.inner.read(buffer)
        }
    }
    #[test]
    fn substituted_export_parent_cannot_redirect_writes_or_cleanup_into_original() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original");
        let parent = temp.path().join("export-parent");
        let parked = temp.path().join("parked-parent");
        fs::create_dir(&original).unwrap();
        fs::create_dir(&parent).unwrap();
        fs::write(original.join("keep.txt"), "unchanged").unwrap();
        let mut archive = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        archive
            .append_data(&mut header, "nested/marker.txt", b"probe".as_slice())
            .unwrap();
        let reader = ParentSwap {
            inner: Cursor::new(archive.into_inner().unwrap()),
            parent: parent.clone(),
            parked: parked.clone(),
            original: original.clone(),
            swapped: false,
        };
        let error = extract_workspace(reader, &parent.join("result"), &original, Limits::default())
            .expect_err("changed parent identity must fail the export");
        assert!(
            error.chain().any(|cause| cause.to_string()
                == "export parent path or ancestry changed; export refused"),
            "the export refusal must identify the changed parent: {error:#}"
        );
        assert_eq!(
            fs::read_dir(&original).unwrap().count(),
            1,
            "neither data nor a staging directory may be created in the original"
        );
        assert_eq!(
            fs::read_to_string(original.join("keep.txt")).unwrap(),
            "unchanged"
        );
        let mut retained = fs::read_dir(&parked).unwrap();
        let staging = retained
            .next()
            .expect("private staging must be retained")
            .unwrap();
        assert!(
            staging.file_type().unwrap().is_dir()
                && staging
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".agentsandbox-export-"),
            "the moved parent must contain the private staging directory"
        );
        assert!(
            retained.next().is_none(),
            "only staging may remain when publication is refused"
        );
    }

    #[test]
    fn helpers_for_the_same_session_have_distinct_operation_names() {
        let session = uuid::Uuid::new_v4();
        let a = ContainerSpec::new(
            session,
            ContainerPurpose::Export,
            "worker:local",
            vec!["/bin/tar".into()],
            NetworkMode::None,
        )
        .unwrap();
        let b = ContainerSpec::new(
            session,
            ContainerPurpose::Export,
            "worker:local",
            vec!["/bin/tar".into()],
            NetworkMode::None,
        )
        .unwrap();
        assert_eq!(a.volume(), b.volume());
        assert_ne!(a.name(), b.name());
        assert_eq!(a.name(), a.clone().name());
    }

    #[test]
    fn failed_create_never_removes_a_preexisting_container() {
        const CHILD: &str = "AGENTSANDBOX_RACE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let temporary = tempfile::tempdir().unwrap();
            let binary = temporary.path().join("docker");
            let marker = temporary.path().join("existing-container");
            let log = temporary.path().join("runtime.log");
            fs::write(&marker, "another invocation owns this container").unwrap();
            let rustc = std::env::var_os("RUSTC")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("CARGO_HOME")
                        .map(PathBuf::from)
                        .map(|home| home.join("bin/rustc"))
                        .filter(|p| p.is_file())
                })
                .unwrap_or_else(|| PathBuf::from("rustc"));
            let compiled = std::process::Command::new(rustc)
                .args(["--edition=2024", "--crate-name", "create_conflict_fixture"])
                .arg(
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/runtime_fixtures/create_conflict.rs"),
                )
                .arg("-o")
                .arg(&binary)
                .output()
                .unwrap();
            assert!(
                compiled.status.success(),
                "{}",
                String::from_utf8_lossy(&compiled.stderr)
            );
            // The test process explicitly approves its absolute native CLI fixture.
            let metadata = binary.with_extension("fixture");
            fs::create_dir_all(&metadata).unwrap();
            fs::write(metadata.join("log"), log.to_str().unwrap()).unwrap();
            fs::write(metadata.join("marker"), marker.to_str().unwrap()).unwrap();
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "unix::failed_create_never_removes_a_preexisting_container",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("AI_GUARD_RUNTIME_PROGRAM", &binary)
                .env("AGENTSANDBOX_RACE_LOG", &log)
                .env("AGENTSANDBOX_EXISTING_CONTAINER", &marker)
                .output()
                .unwrap();
            assert!(
                child.status.success(),
                "child test failed:\n{}\n{}",
                String::from_utf8_lossy(&child.stdout),
                String::from_utf8_lossy(&child.stderr)
            );
            assert!(marker.is_file());
            assert_eq!(fs::read_to_string(log).unwrap(), "create\n");
            return;
        }
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let runtime = Runtime::with_program(
            RuntimeKind::Docker,
            &PathBuf::from(std::env::var_os("AI_GUARD_RUNTIME_PROGRAM").unwrap()),
        )
        .unwrap();
        let spec = ContainerSpec::new(
            uuid::Uuid::new_v4(),
            ContainerPurpose::Export,
            "worker:local",
            vec!["/bin/tar".into()],
            NetworkMode::None,
        )
        .unwrap();
        let outcome = executor.block_on(runtime.execute(
            &spec,
            None,
            Duration::from_secs(1),
            1024,
            |_| async { Ok(()) },
        ));
        assert!(outcome.is_err());
        assert!(
            PathBuf::from(std::env::var_os("AGENTSANDBOX_EXISTING_CONTAINER").unwrap()).is_file(),
            "another invocation's container was removed after create failed"
        );
    }
}
