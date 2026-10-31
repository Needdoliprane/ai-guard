#![cfg(unix)]

use agentsandbox::{broker::enforcer::EnforcerClient, control_fs::Directory};
use serde_json::Value;
use std::{
    fs, io,
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::{Duration, Instant},
};
use tokio::process::{Child, Command};

#[cfg(target_os = "macos")]
const PRIVATE_TMP: &str = "/private/tmp";
#[cfg(not(target_os = "macos"))]
const PRIVATE_TMP: &str = "/tmp";

const POLL: Duration = Duration::from_millis(20);

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

#[tokio::test]
async fn real_enforcer_child_health_stop_recovery_and_idle_shutdown() {
    let root = tempfile::Builder::new()
        .prefix("ef-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(PRIVATE_TMP)
        .unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    // Keep the private root if an assertion or process transition fails so its
    // endpoint/lease state remains available for diagnosis. Remove it on PASS.
    let root_path = root.keep();
    let root_handle = Directory::open_existing(&root_path).unwrap();
    // Seed the exact private authentication record expected by the real child.
    // It is generated from the OS CSPRNG and never logged or hashed.
    let mut auth = [0_u8; 32];
    fs::File::open("/dev/urandom")
        .unwrap()
        .read_exact(&mut auth)
        .unwrap();
    let mut auth_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root_path.join("auth"))
        .unwrap();
    auth_file.write_all(&auth).unwrap();
    auth_file.sync_all().unwrap();
    drop(auth_file);
    fs::File::open(&root_path).unwrap().sync_all().unwrap();
    auth.fill(0);

    let executable = PathBuf::from(env!("CARGO_BIN_EXE_agentsandbox"))
        .canonicalize()
        .unwrap();
    assert!(executable.is_absolute());

    let mut child = Command::new(&executable)
        .arg("__broker-enforcer")
        .arg("--root")
        .arg(&root_path)
        .arg("--max-slots")
        .arg("8")
        .env_clear()
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid: libc::pid_t = child.id().unwrap().try_into().unwrap();

    let published = wait_for_announcement(&root_path, Duration::from_secs(5))
        .await
        .expect("the child must durably publish a real endpoint announcement");
    assert!(published.is_file());
    let announcement_before = announcement_names(&root_path).unwrap();
    assert_eq!(announcement_before.len(), 1);

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
    fs::remove_dir_all(&root_path).unwrap();
}
