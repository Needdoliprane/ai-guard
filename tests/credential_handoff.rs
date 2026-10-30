#![cfg(unix)]
use agentsandbox::runtime::{ContainerPurpose, ContainerSpec, NetworkMode, RuntimeKind};
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn direct_secret_handle_cannot_bypass_the_64_kib_credential_limit() {
    use agentsandbox::control_fs::Directory;
    let temporary = tempfile::tempdir().unwrap();
    let config =
        Directory::open_or_create(&temporary.path().canonicalize().unwrap().join("config"))
            .unwrap();
    let oversized = config.snapshot(&vec![b'x'; 64 * 1024 + 1]).unwrap();
    let spec = ContainerSpec::new(
        uuid::Uuid::new_v4(),
        ContainerPurpose::Run,
        "worker:fixture",
        vec!["/usr/bin/id".into()],
        NetworkMode::Bridge,
    )
    .unwrap();
    assert!(
        spec.with_secret_env_handle(oversized.private_file())
            .is_err(),
        "direct handle bypassed the credential size limit"
    );
}

#[tokio::test]
async fn export_refuses_a_fifo_checkpoint_without_blocking_or_contacting_runtime() {
    use std::os::unix::ffi::OsStrExt;
    let temporary = tempfile::tempdir().unwrap();
    let config = temporary.path().join("config");
    agentsandbox::operations::initialize_config(&config).unwrap();
    let session = uuid::Uuid::new_v4();
    let record = config.join("sessions").join(format!("{session}.json"));
    let name = std::ffi::CString::new(record.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
            .args(["export-workspace", "--session", &session.to_string()])
            .arg("--config-dir")
            .arg(&config)
            .arg("--output")
            .arg(temporary.path().join("exported"))
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("checkpoint FIFO blocked before validation")
    .unwrap();
    assert!(!output.status.success());
    assert!(!temporary.path().join("exported").exists());
}

fn native_client(root: &std::path::Path, source: &str) -> std::path::PathBuf {
    let input = root.join("client.rs");
    fs::write(&input, source).unwrap();
    let binary = root.join("engine");
    let compiled =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args([
                "--edition=2024",
                "--crate-name",
                "credential_handoff_fixture",
            ])
            .arg(input)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
    assert!(
        compiled.status.success(),
        "Rust handoff fixture failed to compile"
    );
    binary
}

#[tokio::test]
async fn up_rejects_a_config_root_replaced_after_doctor_without_compose_or_secret_rotation() {
    let temporary = tempfile::tempdir().unwrap();
    for trigger in [3, 4] {
        let root = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join(trigger.to_string());
        fs::create_dir(&root).unwrap();
        let config = root.join("config");
        agentsandbox::operations::initialize_config(&config).unwrap();
        let original = fs::read(config.join("secrets/runner")).unwrap();
        let binary = native_client(
            &root,
            r#"use std::{fs,io::Write,os::unix::fs::PermissionsExt};
fn main() {
 let executable = std::env::current_exe().unwrap();
 let args:Vec<_> = std::env::args().collect();
 if args.iter().any(|arg|arg=="context") { println!("\"unix:///fixture.sock\""); }
 else if args.iter().any(|arg|arg=="info") {
  let count_path=executable.with_extension("count");
  let count=fs::read_to_string(&count_path).unwrap_or_default().parse::<usize>().unwrap_or(0)+1;
  fs::write(count_path,count.to_string()).unwrap();
  let trigger=fs::read_to_string(executable.with_extension("trigger")).unwrap().parse::<usize>().unwrap();
  if count==trigger {
   let config=std::path::PathBuf::from(fs::read_to_string(executable.with_extension("config")).unwrap());
   fs::rename(&config,config.with_extension("retained")).unwrap();
   fs::create_dir(&config).unwrap(); fs::set_permissions(&config,fs::Permissions::from_mode(0o700)).unwrap();
   fs::write(config.join("successor"),"do-not-touch").unwrap();
  }
  println!("{{\"ID\":\"fixture\",\"SecurityOptions\":[\"name=rootless\"],\"ServerVersion\":\"fixture\"}}");
 } else if args.iter().any(|arg|arg=="compose") {
  fs::File::create(executable.with_extension("compose-effect")).unwrap().write_all(b"effect").unwrap();
  std::process::exit(125);
 } else { std::process::exit(126); }
}"#,
        );
        fs::write(binary.with_extension("config"), config.to_str().unwrap()).unwrap();
        fs::write(binary.with_extension("trigger"), trigger.to_string()).unwrap();
        let source = root.join("project");
        fs::create_dir_all(source.join("deploy/docker")).unwrap();
        fs::write(source.join("deploy/docker/compose.yaml"), "services: {}\n").unwrap();
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            tokio::process::Command::new(env!("CARGO_BIN_EXE_agentsandbox"))
                .args(["up", "--runtime-program"])
                .arg(&binary)
                .arg("--source")
                .arg(&source)
                .arg("--config-dir")
                .arg(&config)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!output.status.success());
        assert!(
            !binary.with_extension("compose-effect").exists(),
            "Compose used the replacement configuration"
        );
        assert!(
            fs::read(config.with_extension("retained").join("secrets/runner")).unwrap() == original,
            "the source token was rotated"
        );
        assert_eq!(fs::read(config.join("successor")).unwrap(), b"do-not-touch");
    }
}

#[tokio::test]
async fn shared_snapshot_survives_handoff_and_native_cli_reads_its_exact_prepared_bytes() {
    use agentsandbox::{control_fs::Directory, runtime::Runtime};
    use sha2::{Digest, Sha256};
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let config = Directory::open_or_create(&root.join("config")).unwrap();
    let bytes = b"OPENAI_API_KEY=fixture-private-provider-value\n";
    let snapshot = config
        .child("credential-snapshots")
        .unwrap()
        .snapshot(bytes)
        .unwrap();
    assert_eq!(snapshot.digest(), hex::encode(Sha256::digest(bytes)));
    let path = snapshot.path().to_owned();
    let spec = ContainerSpec::new(
        uuid::Uuid::new_v4(),
        ContainerPurpose::Run,
        "worker:fixture",
        vec!["/usr/bin/id".into()],
        NetworkMode::Bridge,
    )
    .unwrap()
    .with_secret_env_handle(snapshot.private_file())
    .unwrap();
    drop(snapshot);
    assert!(
        path.exists(),
        "the spec lost the snapshot capability before create"
    );
    assert!(!format!("{spec:?}").contains("fixture-private-provider-value"));
    let source = root.join("client.rs");
    fs::write(
        &source,
        r#"fn main() {
        let args:Vec<_> = std::env::args().collect();
        let path = &args[args.iter().position(|arg|arg=="--env-file").unwrap()+1];
        let executable=std::env::current_exe().unwrap();
        std::fs::write(executable.with_extension("waiting"), b"ready").unwrap();
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(5);
        while !executable.with_extension("release").exists() {
            if std::time::Instant::now()>deadline { std::process::exit(125); }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let bytes = std::fs::read(path).unwrap();
        std::fs::write(std::env::current_exe().unwrap().with_extension("received"), bytes).unwrap();
        println!("created-fixture");
    }"#,
    )
    .unwrap();
    let binary = root.join("engine");
    let compiled =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .args([
                "--edition=2024",
                "--crate-name",
                "credential_handoff_fixture",
            ])
            .arg(source)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
    assert!(compiled.status.success());
    let runtime = Runtime::with_program(RuntimeKind::Docker, &binary).unwrap();
    let args = spec.create_args(RuntimeKind::Docker).unwrap();
    let running =
        tokio::spawn(async move { runtime.cli(&args, std::time::Duration::from_secs(8)).await });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while !binary.with_extension("waiting").exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "native fixture did not reach the handoff"
        );
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    assert!(
        path.exists(),
        "snapshot disappeared while the native CLI was waiting"
    );
    fs::write(binary.with_extension("release"), b"continue").unwrap();
    running.await.unwrap().unwrap();
    assert_eq!(fs::read(binary.with_extension("received")).unwrap(), bytes);
    drop(spec);
    assert!(
        !path.exists(),
        "created snapshot was not cleaned after its last handle"
    );
}

#[test]
fn snapshot_cleanup_does_not_remove_a_successor_after_detecting_substitution() {
    use agentsandbox::control_fs::Directory;
    let temporary = tempfile::tempdir().unwrap();
    let config =
        Directory::open_or_create(&temporary.path().canonicalize().unwrap().join("config"))
            .unwrap();
    let snapshot = config
        .snapshot(b"OPENAI_API_KEY=fixture-original-value\n")
        .unwrap();
    let path = snapshot.path().to_owned();
    let spec = ContainerSpec::new(
        uuid::Uuid::new_v4(),
        ContainerPurpose::Run,
        "worker:fixture",
        vec!["/usr/bin/id".into()],
        NetworkMode::Bridge,
    )
    .unwrap()
    .with_secret_env_handle(snapshot.private_file())
    .unwrap();
    drop(snapshot);
    let original = config.path().join("retained-original");
    fs::rename(&path, &original).unwrap();
    fs::write(&path, b"successor-controlled-by-test").unwrap();
    assert!(spec.create_args(RuntimeKind::Docker).is_err());
    drop(spec);
    assert_eq!(fs::read(&path).unwrap(), b"successor-controlled-by-test");
    assert_eq!(
        fs::read(original).unwrap(),
        b"OPENAI_API_KEY=fixture-original-value\n"
    );
}

#[test]
fn env_file_substitution_is_refused_before_create_arguments_can_reach_the_cli() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("provider.env");
    fs::write(&source, "OPENAI_API_KEY=private-original-value\n").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
    let spec = ContainerSpec::new(
        uuid::Uuid::new_v4(),
        ContainerPurpose::Run,
        "worker:fixture",
        vec!["/usr/bin/id".into()],
        NetworkMode::Bridge,
    )
    .unwrap()
    .with_secret_env_file(&source)
    .unwrap();
    fs::rename(&source, temporary.path().join("retained-original")).unwrap();
    fs::write(&source, "OPENAI_API_KEY=private-substitute-value\n").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        spec.create_args(RuntimeKind::Docker).is_err(),
        "a substituted pathname reached create --env-file"
    );
}

#[test]
fn init_refuses_a_private_leaf_below_a_parent_writable_by_another_uid() {
    let temporary = tempfile::tempdir().unwrap();
    let shared = temporary.path().join("shared");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).unwrap();
    let config = shared.join("config");
    assert!(agentsandbox::operations::initialize_config(&config).is_err());
    assert!(
        !config.join("secrets/runner").exists(),
        "credentials were created under a replaceable parent"
    );
}
