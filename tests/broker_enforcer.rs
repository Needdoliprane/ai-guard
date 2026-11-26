#![cfg(unix)]

use agentsandbox::{
    broker::{
        enforcer::{EnforcerClient, EnforcerError, EnforcerStopClient},
        local_ipc::{EndpointRecordV1, IpcError},
    },
    control_fs::Directory,
};
use serde_json::Value;
use std::{
    fs, io,
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
};

#[cfg(target_os = "macos")]
const PRIVATE_TMP: &str = "/private/tmp";
#[cfg(not(target_os = "macos"))]
const PRIVATE_TMP: &str = "/tmp";

const POLL: Duration = Duration::from_millis(20);

#[tokio::test]
async fn explicit_recovery_cli_uses_existing_auth_and_keeps_full_closed() {
    let fixture = tempfile::Builder::new()
        .prefix("erv-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(PRIVATE_TMP)
        .unwrap();
    let base = fixture.path().canonicalize().unwrap();
    let executable = PathBuf::from(env!("CARGO_BIN_EXE_agentsandbox"))
        .canonicalize()
        .unwrap();
    let missing = base.join("missing-role");
    let mut refused = Command::new(&executable)
        .args([
            "__broker-enforcer",
            "--recovery-only",
            "--max-slots",
            "8",
            "--root",
        ])
        .arg(&missing)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    assert!(
        !wait_for_exit(&mut refused, Duration::from_secs(5))
            .await
            .unwrap()
            .success()
    );
    assert!(
        !missing.exists(),
        "recovery CLI must not create a role root"
    );

    let role = base.join("enforcer");
    fs::create_dir(&role).unwrap();
    fs::set_permissions(&role, fs::Permissions::from_mode(0o700)).unwrap();
    for directory in ["endpoints", "announcements"] {
        fs::create_dir(role.join(directory)).unwrap();
        fs::set_permissions(role.join(directory), fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(role.join(".owner.lock"))
        .unwrap();
    let mut no_auth = Command::new(&executable)
        .args([
            "__broker-enforcer",
            "--recovery-only",
            "--max-slots",
            "8",
            "--root",
        ])
        .arg(&role)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    assert!(
        !wait_for_exit(&mut no_auth, Duration::from_secs(5))
            .await
            .unwrap()
            .success()
    );
    assert!(
        !role.join("auth").exists(),
        "recovery CLI must never initialize auth"
    );
    assert!(announcement_names(&role).unwrap().is_empty());
    let mut auth = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(role.join("auth"))
        .unwrap();
    auth.write_all(&[0x5a; 32]).unwrap();
    auth.sync_all().unwrap();
    drop(auth);
    let auth_before = fs::read(role.join("auth")).unwrap();
    let lock_before = fs::metadata(role.join(".owner.lock")).unwrap();
    let mut child = Command::new(&executable)
        .args([
            "__broker-enforcer",
            "--recovery-only",
            "--max-slots",
            "8",
            "--root",
        ])
        .arg(&role)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    wait_for_announcement(&role, Duration::from_secs(20))
        .await
        .unwrap();
    let root = Directory::open_existing(&role).unwrap();
    let client = EnforcerStopClient::connect_or_spawn_for_recovery(&root, &executable, 8)
        .await
        .unwrap();
    let first = client.status().await.unwrap();
    assert!(first.transport_healthy());
    assert!(!first.admission_available());
    assert_eq!(first.slot_count(), 0);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(client.status().await.unwrap().progress() > first.progress());
    assert!(
        EnforcerClient::connect_or_spawn(&root, &executable, 8)
            .await
            .is_err(),
        "V1 must not reinterpret the V2 challenge"
    );
    assert!(child.try_wait().unwrap().is_none());
    assert_eq!(
        announcement_names(&role).unwrap().len(),
        1,
        "Full refusal must not spawn a replacement"
    );
    assert_eq!(fs::read(role.join("auth")).unwrap(), auth_before);
    let lock_after = fs::metadata(role.join(".owner.lock")).unwrap();
    assert_eq!(
        (lock_after.dev(), lock_after.ino()),
        (lock_before.dev(), lock_before.ino())
    );
    drop(client);
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}

async fn wait_for_announcement(root: &Path, timeout: Duration) -> io::Result<PathBuf> {
    let deadline = Instant::now() + timeout;
    loop {
        let announcements = root.join("announcements");
        match fs::read_dir(&announcements) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    let name = entry.file_name();
                    let Some(name) = name.to_str() else {
                        continue;
                    };
                    if !name.starts_with("e-") || !name.ends_with(".json") {
                        continue;
                    }
                    let bytes = fs::read(entry.path())?;
                    let record: Value = serde_json::from_slice(&bytes).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "invalid enforcer announcement")
                    })?;
                    if record["version"] == 1
                        && record["sequence"].as_u64().is_some_and(|value| value > 0)
                        && record["endpoint"]["epoch"].as_str().is_some()
                    {
                        return Ok(entry.path());
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "enforcer announcement was not published before deadline",
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

fn announcement_names(root: &Path) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(root.join("announcements"))? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with("e-") && name.ends_with(".json") {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

async fn wait_for_stopped(pid: libc::pid_t, timeout: Duration) -> io::Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut status = 0;
        // SAFETY: waits nonblocking only for the test's directly spawned child.
        let result = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED | libc::WNOHANG) };
        if result == pid {
            if libc::WIFSTOPPED(status) {
                return Ok(true);
            }
            if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
                return Ok(false);
            }
        } else if result < 0 {
            return Err(io::Error::last_os_error());
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(POLL).await;
    }
}

async fn wait_client_ready(client: &EnforcerClient, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if client.require_admission_ready().await.is_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL).await;
    }
}

async fn wait_for_exit(child: &mut Child, timeout: Duration) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "enforcer child did not exit before deadline",
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

fn endpoint_from_announcement(path: &Path) -> io::Result<EndpointRecordV1> {
    let bytes = fs::read(path)?;
    if bytes.len() > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "enforcer announcement exceeds its bound",
        ));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid announcement"))?;
    let endpoint = value
        .get("endpoint")
        .cloned()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing endpoint"))?;
    serde_json::from_value(endpoint)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid endpoint record"))
}

fn effective_uid() -> u32 {
    // SAFETY: read-only query of the test process credentials.
    unsafe { libc::geteuid() }
}

fn private_regular_file(path: &Path, expected_len: u64) -> io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    Ok(metadata.file_type().is_file()
        && metadata.uid() == effective_uid()
        && metadata.mode() & 0o7777 == 0o600
        && metadata.len() == expected_len)
}

fn role_lease_available(root: &Path) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    let path = root.join(".owner.lock");
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file()
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o7777 != 0o600
        || metadata.len() != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "private role lease file has unexpected metadata",
        ));
    }
    // A shared lock observes whether the daemon's exclusive role lease remains
    // held. It never acquires the exclusive service lease or starts a process.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
    if result == 0 {
        let unlocked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
        if unlocked != 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(error)
    }
}

async fn wait_for_endpoint_refusal(
    endpoint: &EndpointRecordV1,
    endpoint_root: &Directory,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match endpoint.connect_checked(endpoint_root).await {
            Err(IpcError::Unavailable) => return true,
            Ok(stream) => drop(stream),
            Err(_) => return false,
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL).await;
    }
}

async fn wait_for_role_lease_release(root: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match role_lease_available(root) {
            Ok(true) => return true,
            Ok(false) => {}
            Err(_) => return false,
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL).await;
    }
}

fn bounded_root_inventory(root: &Path) -> Vec<String> {
    let mut names = match fs::read_dir(root) {
        Ok(entries) => entries
            .take(32)
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect::<Vec<_>>(),
        Err(_) => vec!["<unavailable>".into()],
    };
    names.sort();
    names
}

#[tokio::test]
async fn real_enforcer_child_health_stop_recovery_and_idle_shutdown() {
    let parent = tempfile::Builder::new()
        .prefix("ef-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(PRIVATE_TMP)
        .unwrap();
    fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o700)).unwrap();
    // Keep the private root if an assertion or process transition fails so its
    // endpoint/lease state remains available for diagnosis. Remove it on PASS.
    let parent_path = parent.keep().canonicalize().unwrap();
    let executable = PathBuf::from(env!("CARGO_BIN_EXE_agentsandbox"))
        .canonicalize()
        .unwrap();
    assert!(executable.is_absolute());

    // An admission-ready real child is the genuine fresh Legacy factory: the
    // existing-Legacy transport joins an already initialized role and stays
    // recovery-pending (it never bootstraps a missing lock into fresh admission
    // and never clears recovery_incomplete), so require_admission_ready can only
    // succeed against a fresh factory child. The child exclusively creates its
    // own private role namespace, empty V1 owner lock and CSPRNG auth record.
    let root_path = parent_path.join("enforcer");
    let mut child = Command::new(&executable)
        .arg("__broker-enforcer")
        .arg("--root")
        .arg(&parent_path)
        .arg("--max-slots")
        .arg("8")
        .arg("--legacy-fresh")
        .arg("--role-name")
        .arg("enforcer")
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid: libc::pid_t = child.id().unwrap().try_into().unwrap();
    let mut launch_pipe = child.stdin.take().unwrap();
    launch_pipe
        .write_all(uuid::Uuid::new_v4().as_bytes())
        .await
        .unwrap();
    drop(launch_pipe);

    let published = wait_for_announcement(&root_path, Duration::from_secs(20))
        .await
        .expect("the child must durably publish a real endpoint announcement");
    assert!(published.is_file());
    let announcement_before = announcement_names(&root_path).unwrap();
    assert_eq!(announcement_before.len(), 1);

    let root_handle = Directory::open_existing(&root_path).unwrap();
    let client = EnforcerClient::connect_or_spawn(&root_handle, &executable, 8)
        .await
        .unwrap();
    client.require_admission_ready().await.unwrap();
    let initial_status = client.status().await.unwrap();
    assert!(initial_status.healthy());
    assert_eq!(initial_status.slot_count(), 0);
    assert_eq!(initial_status.max_slots(), 8);
    let epoch = initial_status.epoch();

    // Stop only the exact child spawned above and wait for the kernel stop fact.
    // SAFETY: pid is the direct child PID and remains owned by `child`.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGSTOP) }, 0);
    assert!(wait_for_stopped(pid, Duration::from_secs(2)).await.unwrap());
    assert!(child.try_wait().unwrap().is_none());

    let gate = tokio::time::timeout(Duration::from_secs(4), client.require_admission_ready()).await;
    assert!(
        matches!(gate, Err(_) | Ok(Err(_))),
        "admission readiness must refuse a stopped helper within the bound"
    );

    let reconnect = tokio::time::timeout(
        Duration::from_secs(4),
        EnforcerClient::connect_or_spawn(&root_handle, &executable, 8),
    )
    .await;
    assert!(
        matches!(reconnect, Err(_) | Ok(Err(_))),
        "an occupied lease must prevent takeover while the original helper is stopped"
    );
    assert!(child.try_wait().unwrap().is_none());
    assert_eq!(announcement_names(&root_path).unwrap(), announcement_before);

    // Resume the same child; health may recover only against its original epoch.
    // SAFETY: pid is still the same direct child and has not exited.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGCONT) }, 0);
    assert!(
        wait_client_ready(&client, Duration::from_secs(5)).await,
        "the original client must recover readiness after SIGCONT"
    );
    let recovered_status = client.status().await.unwrap();
    assert!(recovered_status.healthy());
    assert_eq!(recovered_status.slot_count(), 0);
    assert_eq!(recovered_status.max_slots(), 8);
    assert_eq!(recovered_status.epoch(), epoch);
    assert_eq!(announcement_names(&root_path).unwrap(), announcement_before);

    client.shutdown_if_idle().await.unwrap();
    let exit = wait_for_exit(&mut child, Duration::from_secs(5))
        .await
        .expect("shutdown_if_idle must terminate the owned child within the bound");
    assert!(exit.success());
    // The fresh Legacy factory never rewrites its empty V1 owner lease.
    assert!(private_regular_file(&root_path.join(".owner.lock"), 0).unwrap());
    fs::remove_dir_all(&parent_path).unwrap();
}

#[tokio::test]
async fn public_client_reconnects_to_the_genuinely_fresh_legacy_enforcer_and_shuts_it_down() {
    let parent = tempfile::Builder::new()
        .prefix("eb-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(PRIVATE_TMP)
        .unwrap();
    let parent_path = parent.keep().canonicalize().unwrap();
    let parent_handle = Directory::open_existing(&parent_path).unwrap();
    let executable = PathBuf::from(env!("CARGO_BIN_EXE_agentsandbox"))
        .canonicalize()
        .unwrap();
    assert!(matches!(
        EnforcerClient::connect_or_spawn(&parent_handle, &executable, 8).await,
        Err(EnforcerError::Unauthorized)
    ));
    assert!(fs::read_dir(&parent_path).unwrap().next().is_none());

    let root_path = parent_path.join("enforcer");
    let mut child = Command::new(&executable)
        .args(["__broker-enforcer", "--root"])
        .arg(&parent_path)
        .args([
            "--max-slots",
            "8",
            "--legacy-fresh",
            "--role-name",
            "enforcer",
        ])
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut launch_pipe = child.stdin.take().unwrap();
    launch_pipe
        .write_all(uuid::Uuid::new_v4().as_bytes())
        .await
        .unwrap();
    drop(launch_pipe);
    let announcement_path = wait_for_announcement(&root_path, Duration::from_secs(20))
        .await
        .expect("the genuine fresh child must publish its endpoint");
    let endpoint = endpoint_from_announcement(&announcement_path).unwrap();
    let role_files = [root_path.join("auth"), root_path.join(".owner.lock")];
    let identity = |path: &Path| {
        let metadata = fs::symlink_metadata(path).unwrap();
        (metadata.dev(), metadata.ino())
    };
    let identities = role_files.each_ref().map(|path| identity(path));
    let root_handle = Directory::open_existing(&root_path).unwrap();

    let client = match EnforcerClient::connect_or_spawn(&root_handle, &executable, 8).await {
        Ok(client) => client,
        Err(error) => {
            let inventory = bounded_root_inventory(&root_path);
            panic!(
                "public enforcer reconnect failed ({error:?}); root retained={root_path:?}; entries={inventory:?}"
            );
        }
    };

    // Gather all observations first; always request shutdown before assertions.
    let ready = client.require_admission_ready().await.is_ok();
    let status = client.status().await.ok();
    let status_ok = status.as_ref().is_some_and(|status| {
        status.healthy()
            && status.slot_count() == 0
            && status.max_slots() == 8
            && status.epoch() == endpoint.epoch()
    });
    let same_installation = identities == role_files.each_ref().map(|path| identity(path));
    let one_announcement = announcement_names(&root_path).unwrap().len() == 1;

    let announcement_metadata_ok = fs::symlink_metadata(&announcement_path).is_ok_and(|metadata| {
        metadata.file_type().is_file()
            && metadata.uid() == effective_uid()
            && metadata.mode() & 0o7777 == 0o600
            && metadata.len() <= 4096
    });
    let endpoint_root = Directory::open_existing(&root_path.join("endpoints")).unwrap();
    let auth_metadata_ok = private_regular_file(&root_path.join("auth"), 32).unwrap_or(false);

    let shutdown = client.shutdown_if_idle().await.is_ok();
    let endpoint_refused =
        wait_for_endpoint_refusal(&endpoint, &endpoint_root, Duration::from_secs(5)).await;
    let child_exited = wait_for_exit(&mut child, Duration::from_secs(5))
        .await
        .is_ok_and(|status| status.success());
    // Listener refusal alone does not prove the owner has completed shutdown.
    // Keep the private evidence until its exclusive role lease is released too.
    let lease_released = wait_for_role_lease_release(&root_path, Duration::from_secs(5)).await;

    drop(client);
    drop(endpoint_root);
    drop(root_handle);
    let success = ready
        && status_ok
        && same_installation
        && one_announcement
        && announcement_metadata_ok
        && auth_metadata_ok
        && shutdown
        && endpoint_refused
        && child_exited
        && lease_released;
    let inventory = bounded_root_inventory(&root_path);
    if success {
        fs::remove_dir_all(&parent_path).unwrap();
    } else {
        panic!(
            "public enforcer reconnect/shutdown observations failed: ready={ready}, status_ok={status_ok}, same_installation={same_installation}, one_announcement={one_announcement}, announcement_private={announcement_metadata_ok}, auth_private_len32={auth_metadata_ok}, shutdown_ack={shutdown}, endpoint_refused={endpoint_refused}, child_exited={child_exited}, role_lease_released={lease_released}; root retained={root_path:?}; entries={inventory:?}"
        );
    }
}
