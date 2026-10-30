//! Native broker-clock contracts. These tests do not qualify watchdog or
//! process-survival behavior.

use agentsandbox::broker::clock::{
    ClockError, HostClock, NativeHostClock, canonical_boot_id, checked_deadline, ticks_to_ns,
    timespec_to_ns,
};
use agentsandbox::control::{HostClockMethodV1, HostClockSampleV1};
use std::{
    io::{Read, Write},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

const CHILD_PROBE_ENV: &str = "AI_GUARD_NATIVE_CLOCK_CHILD_PROBE";
const CHILD_SAMPLE_PREFIX: &[u8] = b"AG_CLOCK_SAMPLE:";

#[test]
fn timespec_conversion_is_bounded_and_rejects_invalid_readings() {
    assert_eq!(timespec_to_ns(1, 999_999_999).unwrap(), 1_999_999_999);
    assert_eq!(timespec_to_ns(0, 0).unwrap(), 0);
    assert!(matches!(
        timespec_to_ns(-1, 0),
        Err(ClockError::InvalidReading)
    ));
    assert!(matches!(
        timespec_to_ns(0, -1),
        Err(ClockError::InvalidReading)
    ));
    assert!(matches!(
        timespec_to_ns(0, 1_000_000_000),
        Err(ClockError::InvalidReading)
    ));
    assert!(matches!(
        timespec_to_ns(i128::MAX, 0),
        Err(ClockError::Overflow)
    ));
}

#[test]
fn darwin_timebase_conversion_uses_integer_floor_and_checks_overflow() {
    assert_eq!(ticks_to_ns(3, 125, 3).unwrap(), 125);
    assert_eq!(ticks_to_ns(1, 1, 3).unwrap(), 0);
    assert!(matches!(
        ticks_to_ns(1, 0, 3),
        Err(ClockError::InvalidReading)
    ));
    assert!(matches!(
        ticks_to_ns(1, 3, 0),
        Err(ClockError::InvalidReading)
    ));
    assert!(matches!(
        ticks_to_ns(u128::MAX, 2, 1),
        Err(ClockError::Overflow)
    ));
}

#[test]
fn deadline_math_is_checked_and_zero_timeout_is_invalid() {
    assert_eq!(
        checked_deadline(u128::MAX - 1_000_000_000, 1).unwrap(),
        u128::MAX
    );
    assert!(matches!(
        checked_deadline(u128::MAX - 1_000_000_000 + 1, 1),
        Err(ClockError::Overflow)
    ));
    assert!(matches!(
        checked_deadline(0, 0),
        Err(ClockError::InvalidReading)
    ));
}

#[test]
fn boot_identity_and_native_sample_are_canonical_and_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<NativeHostClock>();

    let upper = b"A0B1C2D3-E4F5-4678-9ABC-DEF012345678";
    assert_eq!(
        canonical_boot_id(upper).unwrap(),
        "a0b1c2d3-e4f5-4678-9abc-def012345678"
    );
    for bad in [
        &b"00000000-0000-0000-0000-000000000000"[..],
        &b""[..],
        &b"a0b1c2d3e4f546789abcdef012345678"[..],
        &b"a0b1c2d3-e4f5-4678-9abc-def012345678\n\n"[..],
        &b"a0b1c2d3-e4f5-4678-9abc-\xff012345678"[..],
        &b"a0b1c2d3-e4f5-4678-9abc-def012345678\0"[..],
    ] {
        assert!(matches!(
            canonical_boot_id(bad),
            Err(ClockError::InvalidBootId)
        ));
    }
    assert!(matches!(
        canonical_boot_id(&[b'a'; 4096]),
        Err(ClockError::InvalidBootId)
    ));

    let clock = NativeHostClock::new().expect("native continuous clock should initialize");
    let first = clock.sample().unwrap();
    validate_native_sample(&first);
    let second = clock.sample().unwrap();
    validate_native_sample(&second);
    assert_eq!(first.boot_id, second.boot_id);
    assert_eq!(first.method, second.method);
    assert!(second.continuous_ns().unwrap() >= first.continuous_ns().unwrap());
}

#[test]
fn independent_native_process_samples_share_boot_and_monotonic_counter() {
    let clock = NativeHostClock::new().expect("native continuous clock should initialize");
    let before = clock.sample().unwrap();
    let child = run_child_probe_bounded();
    let after = clock.sample().unwrap();
    validate_native_sample(&before);
    validate_native_sample(&child);
    validate_native_sample(&after);

    assert_eq!(before.method, child.method);
    assert_eq!(child.method, after.method);
    assert_eq!(before.boot_id, child.boot_id);
    assert_eq!(child.boot_id, after.boot_id);
    let before_ns = before.continuous_ns().unwrap();
    let child_ns = child.continuous_ns().unwrap();
    let after_ns = after.continuous_ns().unwrap();
    assert!(before_ns <= child_ns && child_ns <= after_ns);
}

/// Invoked only by the parent test using the test executable and an anonymous
/// stdout pipe. Its payload stays inside that pipe and is never logged.
#[test]
#[ignore = "private subprocess probe; invoked by independent_native_process_samples_share_boot_and_monotonic_counter"]
fn native_child_probe() {
    if std::env::var_os(CHILD_PROBE_ENV).is_none() {
        return;
    }
    let sample = NativeHostClock::new()
        .and_then(|clock| clock.sample())
        .expect("native child clock sample");
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(CHILD_SAMPLE_PREFIX).unwrap();
    serde_json::to_writer(&mut stdout, &sample).unwrap();
    stdout.write_all(b"\n").unwrap();
}

fn validate_native_sample(sample: &HostClockSampleV1) {
    sample.validate().unwrap();
    assert_eq!(sample.version, 1);
    assert!(Uuid::parse_str(&sample.boot_id).is_ok_and(|id| !id.is_nil()));
    assert_eq!(sample.boot_id, sample.boot_id.to_ascii_lowercase());
    assert_eq!(sample.observed_utc, None);
    #[cfg(target_os = "linux")]
    assert_eq!(sample.method, HostClockMethodV1::LinuxBoottimeV1);
    #[cfg(target_os = "macos")]
    assert_eq!(sample.method, HostClockMethodV1::DarwinMachContinuousV1);
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn run_child_probe_bounded() -> HostClockSampleV1 {
    let executable = std::env::current_exe().expect("current test executable");
    let mut child = ChildGuard(
        Command::new(executable)
            .args(["--exact", "native_child_probe", "--ignored", "--nocapture"])
            .env_clear()
            .env(CHILD_PROBE_ENV, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn bounded native clock probe"),
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.0.try_wait().expect("poll child process") {
            assert!(status.success(), "native child probe failed");
            break;
        }
        assert!(Instant::now() < deadline, "native child probe exceeded 5s");
        thread::sleep(Duration::from_millis(10));
    }

    let mut output = Vec::new();
    child
        .0
        .stdout
        .take()
        .expect("child stdout pipe")
        .read_to_end(&mut output)
        .expect("read bounded child output");
    let payload = output
        .split(|byte| *byte == b'\n')
        .find_map(|line| line.strip_prefix(CHILD_SAMPLE_PREFIX))
        .expect("child sample marker in private pipe");
    serde_json::from_slice(payload).expect("decode private child sample")
}
