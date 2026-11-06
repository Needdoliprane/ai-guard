//! Descriptor exhaustion is confined to a separate child with a low soft limit.
//! The parent and its enclosing sandbox retain their original descriptor limit.
#![cfg(unix)]

use agentsandbox::{audit::Store, model::Event, redaction::Redactor};
use std::{
    fs::{File, Permissions},
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const CHILD: &str = "AI_GUARD_STORE_FD_RECOVERY_CHILD_V1";
const RESTORED: &str = "STORE_FD_RECOVERY_RESTORED_V1";
const RECOVERED: &str = "STORE_FD_RECOVERY_VALIDATED_V1";

#[test]
fn temporary_emfile_refuses_append_without_permanently_latching_store() {
    run_child("zero_free");
}

#[test]
fn one_free_descriptor_preserves_the_openat_emfile_cause_and_recovers() {
    run_child("one_free");
}

fn run_child(mode: &str) {
    let scratch = tempfile::Builder::new()
        .permissions(Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "store_fd_recovery_child",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .env(CHILD, mode)
        .env("TMPDIR", scratch.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // All communication descriptors are opened by spawn, before the child
    // lowers its limit. Output is fixed and bounded; it cannot fill the pipes.
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!("FD child exceeded its deadline: {:?}", output.status);
        }
        // This sleep bounds parent polling; it is not a test synchronization
        // condition or evidence of exhaustion/recovery.
        std::thread::sleep(Duration::from_millis(5));
    }
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stdout.len() + stderr.len() <= 16_384,
        "unbounded child output"
    );
    assert!(
        stdout.contains(RESTORED),
        "child did not restore FDs: {stdout}\n{stderr}"
    );
    assert!(
        output.status.success(),
        "EMFILE child failed: {stdout}\n{stderr}"
    );
    assert!(
        stdout.contains(RECOVERED),
        "missing recovery oracle: {stdout}"
    );
}

struct SoftLimit(libc::rlimit);

impl SoftLimit {
    fn bounded() -> Self {
        let mut original = std::mem::MaybeUninit::<libc::rlimit>::uninit();
        // SAFETY: getrlimit initializes the valid output pointer on success.
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, original.as_mut_ptr()) },
            0
        );
        // SAFETY: the successful call initialized original.
        let original = unsafe { original.assume_init() };
        assert!(
            original.rlim_cur >= 128,
            "sandbox nofile must permit the 128-FD child"
        );
        let bounded = libc::rlimit {
            rlim_cur: 128,
            rlim_max: original.rlim_max,
        };
        // SAFETY: only this private child's soft limit is changed.
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &bounded) }, 0);
        Self(original)
    }

    fn restore(&self) {
        // SAFETY: restore this child's previously read limit, never the parent.
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &self.0) }, 0);
    }
}

#[test]
#[ignore = "executed only by its parent in a separately bounded process"]
fn store_fd_recovery_child() {
    let mode = std::env::var(CHILD).unwrap();
    assert!(matches!(mode.as_str(), "zero_free" | "one_free"));
    let directory = tempfile::Builder::new()
        .permissions(Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let store = Store::open(
        &directory.path().join("audit.sqlite"),
        Redactor::new(vec![]),
        100,
    )
    .unwrap();
    store
        .append(Event::new("fixture.before_fd_exhaustion"))
        .unwrap();
    let before = store.integrity().unwrap();
    assert!(before.valid && store.healthy());

    let limit = SoftLimit::bounded();
    let mut held = Vec::with_capacity(128);
    let exhausted = loop {
        match File::open("/dev/null") {
            Ok(file) => {
                held.push(file);
                assert!(held.len() <= 128, "child failed to enforce its FD bound");
            }
            Err(error) => break error,
        }
    };
    if mode == "one_free" {
        // Reopening / consumes the one free slot; the next directory openat
        // must fail. The typed cause must survive that exact native boundary.
        drop(
            held.pop()
                .expect("the bounded child must own at least one FD"),
        );
    }
    let refusal = store.append(Event::new("fixture.refused_during_emfile"));
    let released = held.len();
    drop(held);
    limit.restore();
    // All assertions that could fail on the product run after restoration.
    println!("{RESTORED} released={released}");
    assert_eq!(exhausted.raw_os_error(), Some(libc::EMFILE));
    let error = refusal.expect_err("an unauditable append must fail closed");
    assert!(
        error.chain().any(|cause| cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.raw_os_error() == Some(libc::EMFILE))),
        "append must preserve the typed EMFILE cause: {error:#}"
    );
    if mode == "one_free" {
        assert!(
            error
                .chain()
                .any(|cause| cause.to_string()
                    == "control directory must exist without symbolic links"),
            "the single free descriptor must reach openat: {error:#}"
        );
        println!("STORE_FD_RECOVERY_BOUNDARY_V1 point=openat");
    }
    let validated = store.integrity().unwrap();
    assert!(validated.valid, "the real chain must remain intact");
    assert_eq!(
        validated.event_count, before.event_count,
        "refused append was committed"
    );
    assert_eq!(
        validated.head_hash, before.head_hash,
        "refused append changed the head"
    );
    println!(
        "STORE_FD_RECOVERY_STATE_V1 healthy={} integrity_valid={}",
        store.healthy(),
        validated.valid
    );
    assert!(
        store.healthy(),
        "temporary EMFILE must not permanently latch this Store"
    );
    let stored = store
        .append(Event::new("fixture.after_fd_recovery"))
        .unwrap();
    let after = store.integrity().unwrap();
    assert!(after.valid);
    assert_eq!(after.event_count, before.event_count + 1);
    assert_eq!(after.head_hash, stored.hash);
    println!("{RECOVERED}");
}
