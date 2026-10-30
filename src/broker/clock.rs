//! Native continuous host clocks with immutable boot identity and checked arithmetic.
//!
//! This producer does not enforce deadlines or authorize runtime effects.
use crate::control::{HostClockMethodV1, HostClockSampleV1};
use std::fmt;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::io;
use uuid::Uuid;

const NS_PER_SECOND: u128 = 1_000_000_000;
pub type ClockResult<T> = Result<T, ClockError>;

#[derive(Debug)]
pub enum ClockError {
    Unsupported,
    System {
        operation: &'static str,
        status: Option<i32>,
    },
    InvalidBootId,
    InvalidReading,
    Overflow,
}
impl fmt::Display for ClockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("native continuous host clock is unsupported"),
            Self::System { operation, status } => write!(
                f,
                "host clock {operation} failed (native status {status:?})"
            ),
            Self::InvalidBootId => f.write_str("native boot identity is invalid"),
            Self::InvalidReading => {
                f.write_str("native clock reading or conversion ratio is invalid")
            }
            Self::Overflow => f.write_str("host clock arithmetic overflow"),
        }
    }
}
impl std::error::Error for ClockError {}
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn system_error(operation: &'static str, error: io::Error) -> ClockError {
    ClockError::System {
        operation,
        status: error.raw_os_error(),
    }
}

pub trait HostClock: Send + Sync {
    fn sample(&self) -> ClockResult<HostClockSampleV1>;
}

/// Boot identity and timebase are immutable for this process; no public input path.
#[derive(Debug)]
pub struct NativeHostClock {
    boot_id: String,
    method: HostClockMethodV1,
    #[cfg(target_os = "macos")]
    numer: u32,
    #[cfg(target_os = "macos")]
    denom: u32,
}
impl NativeHostClock {
    pub fn new() -> ClockResult<Self> {
        #[cfg(target_os = "linux")]
        {
            use std::io::Read;
            let file = std::fs::File::open("/proc/sys/kernel/random/boot_id")
                .map_err(|error| system_error("open_boot_id", error))?;
            let mut bytes = Vec::with_capacity(38);
            file.take(38)
                .read_to_end(&mut bytes)
                .map_err(|error| system_error("read_boot_id", error))?;
            let raw = match bytes.strip_suffix(b"\n") {
                Some(raw) => raw,
                None => &bytes,
            };
            Ok(Self {
                boot_id: canonical_boot_id(raw)?,
                method: HostClockMethodV1::LinuxBoottimeV1,
            })
        }
        #[cfg(target_os = "macos")]
        {
            let mut bytes = [0_u8; 37];
            let mut len = bytes.len();
            // SAFETY: fixed NUL-terminated key, writable bounded buffer and length;
            // null newp/newlen=0 make this a read-only sysctl call.
            let result = unsafe {
                libc::sysctlbyname(
                    c"kern.bootsessionuuid".as_ptr(),
                    bytes.as_mut_ptr().cast(),
                    &mut len,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if result != 0 {
                return Err(system_error(
                    "read_boot_session_uuid",
                    io::Error::last_os_error(),
                ));
            }
            if len != bytes.len() || bytes[36] != 0 {
                return Err(ClockError::InvalidBootId);
            }
            let boot_id = canonical_boot_id(&bytes[..36])?;
            let mut ratio = MachTimebaseInfo { numer: 0, denom: 0 };
            // SAFETY: repr(C) layout matches mach/mach_time.h; ratio is writable.
            let result = unsafe { mach_timebase_info(&mut ratio) };
            if result != libc::KERN_SUCCESS {
                return Err(ClockError::System {
                    operation: "mach_timebase_info",
                    status: Some(result),
                });
            }
            ticks_to_ns(0, ratio.numer, ratio.denom)?;
            Ok(Self {
                boot_id,
                method: HostClockMethodV1::DarwinMachContinuousV1,
                numer: ratio.numer,
                denom: ratio.denom,
            })
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(ClockError::Unsupported)
        }
    }

    fn continuous_ns(&self) -> ClockResult<u128> {
        #[cfg(target_os = "linux")]
        {
            let mut value = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // SAFETY: value is initialized writable storage of the libc ABI type.
            if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut value) } != 0 {
                return Err(system_error(
                    "clock_gettime_boottime",
                    io::Error::last_os_error(),
                ));
            }
            timespec_to_ns(i128::from(value.tv_sec), i128::from(value.tv_nsec))
        }
        #[cfg(target_os = "macos")]
        {
            // SAFETY: no pointers/arguments; supported macOS ABI (10.12+), never UTC.
            let ticks = unsafe { mach_continuous_time() };
            ticks_to_ns(u128::from(ticks), self.numer, self.denom)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(ClockError::Unsupported)
        }
    }
}
impl HostClock for NativeHostClock {
    fn sample(&self) -> ClockResult<HostClockSampleV1> {
        let ns = self.continuous_ns()?;
        Ok(HostClockSampleV1 {
            version: 1,
            method: self.method,
            boot_id: self.boot_id.clone(),
            continuous_ns_decimal: ns.to_string(),
            observed_utc: None,
        })
    }
}

/// Accept the OS's hyphenated UUID spelling and normalize case once at startup.
pub fn canonical_boot_id(raw: &[u8]) -> ClockResult<String> {
    let text = std::str::from_utf8(raw).map_err(|_| ClockError::InvalidBootId)?;
    if text.len() != 36 {
        return Err(ClockError::InvalidBootId);
    }
    let id = Uuid::parse_str(text).map_err(|_| ClockError::InvalidBootId)?;
    let canonical = id.hyphenated().to_string();
    if id.is_nil() || !text.eq_ignore_ascii_case(&canonical) {
        return Err(ClockError::InvalidBootId);
    }
    Ok(canonical)
}
pub fn timespec_to_ns(seconds: i128, nanos: i128) -> ClockResult<u128> {
    if seconds < 0 || !(0..1_000_000_000).contains(&nanos) {
        return Err(ClockError::InvalidReading);
    }
    let seconds = u128::try_from(seconds).map_err(|_| ClockError::InvalidReading)?;
    let nanos = u128::try_from(nanos).map_err(|_| ClockError::InvalidReading)?;
    seconds
        .checked_mul(NS_PER_SECOND)
        .and_then(|value| value.checked_add(nanos))
        .ok_or(ClockError::Overflow)
}
/// Round fractional nanoseconds down consistently; never use floating point.
pub fn ticks_to_ns(ticks: u128, numer: u32, denom: u32) -> ClockResult<u128> {
    if numer == 0 || denom == 0 {
        return Err(ClockError::InvalidReading);
    }
    ticks
        .checked_mul(u128::from(numer))
        .map(|value| value / u128::from(denom))
        .ok_or(ClockError::Overflow)
}
/// Arithmetic only: the admission contract separately validates timeout policy.
pub fn checked_deadline(armed_ns: u128, timeout_seconds: u32) -> ClockResult<u128> {
    if timeout_seconds == 0 {
        return Err(ClockError::InvalidReading);
    }
    let duration = u128::from(timeout_seconds)
        .checked_mul(NS_PER_SECOND)
        .ok_or(ClockError::Overflow)?;
    armed_ns.checked_add(duration).ok_or(ClockError::Overflow)
}

// libc 0.2.191 lacks mach_continuous_time and deprecates its Mach timebase API.
// These two declarations match the installed Apple SDK and Apple's XNU header.
#[cfg(target_os = "macos")]
#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}
#[cfg(target_os = "macos")]
#[link(name = "System")]
unsafe extern "C" {
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> libc::c_int;
    fn mach_continuous_time() -> u64;
}
