//! Untrusted terminal transport. No transcript, inherited worker output or blocking stdin thread.
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::VecDeque,
    fs::File,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::unix::AsyncFd,
    process::{Child, Command},
    time::Instant,
};

const MAX_CSI: usize = 256;
const MAX_SEQUENCE: usize = 64 * 1024;
const MAX_PENDING: usize = 256 * 1024;
const SEQUENCE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_DRAIN_BYTES: usize = 256 * 1024;
pub(super) const DRAIN_TIMEOUT: Duration = Duration::from_secs(3);
const PRESENTATION_RESET: &[u8] = b"\x1b[?2004l\x1b[?1049l\x1b[r\x1b[?25h\x1b[0m";
pub const TERM: &str = "xterm-256color";

#[derive(Default)]
enum Parser {
    #[default]
    Ground,
    Escape,
    Csi(Vec<u8>),
    String {
        osc: bool,
        escape: bool,
        bytes: usize,
    },
}
enum SafeToken {
    Text { byte: u8, hidden: bool },
    Control(Vec<u8>),
}
struct Pattern {
    bytes: Vec<u8>,
    prefix: Vec<usize>,
    matched: usize,
}
impl Pattern {
    fn new(bytes: Vec<u8>) -> Self {
        let mut prefix = vec![0; bytes.len()];
        for index in 1..bytes.len() {
            let mut matched = prefix[index - 1];
            while matched > 0 && bytes[index] != bytes[matched] {
                matched = prefix[matched - 1];
            }
            if bytes[index] == bytes[matched] {
                matched += 1;
            }
            prefix[index] = matched;
        }
        Self {
            bytes,
            prefix,
            matched: 0,
        }
    }
    fn feed(&mut self, byte: u8) -> bool {
        while self.matched > 0 && self.bytes[self.matched] != byte {
            self.matched = self.prefix[self.matched - 1];
        }
        if self.bytes[self.matched] == byte {
            self.matched += 1;
        }
        if self.matched == self.bytes.len() {
            self.matched = self.prefix[self.matched - 1];
            true
        } else {
            false
        }
    }
}

/// Streaming allowlist followed by literal masking, including across safe control tokens.
/// It deliberately makes no claim about unknown or encoded secrets or cursor reconstruction.
pub struct DisplayFilter {
    parser: Parser,
    utf8: Vec<u8>,
    patterns: Vec<Pattern>,
    pending: VecDeque<SafeToken>,
    pending_bytes: usize,
    pending_text: usize,
    redacting: bool,
    sequence_started: Option<Instant>,
}
impl DisplayFilter {
    pub fn new(secrets: Vec<String>) -> Result<Self> {
        ensure!(
            secrets.len() <= 1024 && secrets.iter().map(String::len).sum::<usize>() <= 192 * 1024,
            "terminal masking dictionary exceeds its bound"
        );
        Ok(Self {
            parser: Parser::Ground,
            utf8: Vec::with_capacity(4),
            patterns: secrets
                .into_iter()
                .filter(|value| !value.is_empty())
                .map(|value| Pattern::new(value.into_bytes()))
                .collect(),
            pending: VecDeque::new(),
            pending_bytes: 0,
            pending_text: 0,
            redacting: false,
            sequence_started: None,
        })
    }
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<u8>> {
        self.check_deadline()?;
        let mut output = Vec::with_capacity(bytes.len());
        for &byte in bytes {
            self.feed(byte, &mut output)?;
        }
        Ok(output)
    }
    pub fn check_deadline(&self) -> Result<()> {
        ensure!(
            self.sequence_started
                .is_none_or(|start| start.elapsed() < SEQUENCE_TIMEOUT),
            "terminal control sequence did not terminate within its bound"
        );
        Ok(())
    }
    /// Only called at a real transport EOF. Possible secret prefixes are masked, never flushed raw.
    pub fn finish(&mut self) -> Result<Vec<u8>> {
        self.check_deadline()?;
        ensure!(
            matches!(self.parser, Parser::Ground),
            "runtime terminal ended with an incomplete control sequence"
        );
        let hold = self.hold();
        self.hide_last(hold);
        self.parser = Parser::Ground;
        self.utf8.clear();
        self.sequence_started = None;
        let mut output = Vec::new();
        self.drain(0, &mut output);
        Ok(output)
    }
    fn hold(&self) -> usize {
        self.patterns
            .iter()
            .map(|pattern| pattern.matched)
            .max()
            .unwrap_or(0)
    }
    fn hide_last(&mut self, mut count: usize) {
        for token in self.pending.iter_mut().rev() {
            if count == 0 {
                break;
            }
            if let SafeToken::Text { hidden, .. } = token {
                *hidden = true;
                count -= 1;
            }
        }
    }
    fn text(&mut self, bytes: &[u8], output: &mut Vec<u8>) -> Result<()> {
        for &byte in bytes {
            self.pending.push_back(SafeToken::Text {
                byte,
                hidden: false,
            });
            self.pending_bytes += 1;
            self.pending_text += 1;
            let mut matches = Vec::new();
            for pattern in &mut self.patterns {
                if pattern.feed(byte) {
                    matches.push(pattern.bytes.len());
                }
            }
            for length in matches {
                self.hide_last(length);
            }
            self.drain(self.hold(), output);
            self.bound()?;
        }
        Ok(())
    }
    fn control(&mut self, bytes: Vec<u8>, output: &mut Vec<u8>) -> Result<()> {
        self.pending_bytes += bytes.len();
        self.pending.push_back(SafeToken::Control(bytes));
        self.drain(self.hold(), output);
        self.bound()
    }
    fn bound(&self) -> Result<()> {
        ensure!(
            self.pending_bytes <= MAX_PENDING && self.pending.len() <= MAX_PENDING,
            "terminal masking window exceeds its bound"
        );
        Ok(())
    }
    fn drain(&mut self, hold: usize, output: &mut Vec<u8>) {
        while self.pending_text > hold || hold == 0 && !self.pending.is_empty() {
            let Some(token) = self.pending.pop_front() else {
                break;
            };
            match token {
                SafeToken::Control(bytes) => {
                    self.pending_bytes -= bytes.len();
                    output.extend(bytes);
                }
                SafeToken::Text { byte, hidden } => {
                    self.pending_bytes -= 1;
                    self.pending_text -= 1;
                    if hidden {
                        if !self.redacting {
                            output.extend_from_slice(b"[REDACTED]");
                        }
                        self.redacting = true;
                    } else {
                        output.push(byte);
                        self.redacting = false;
                    }
                }
            }
        }
    }
    fn enter(&mut self, parser: Parser) {
        self.parser = parser;
        if self.sequence_started.is_none() {
            self.sequence_started = Some(Instant::now());
        }
    }
    fn feed(&mut self, byte: u8, output: &mut Vec<u8>) -> Result<()> {
        let parser = std::mem::take(&mut self.parser);
        match parser {
            Parser::Ground => self.ground(byte, output)?,
            Parser::Escape => match byte {
                b'[' => self.enter(Parser::Csi(Vec::new())),
                b']' | b'P' | b'X' | b'^' | b'_' => self.enter(Parser::String {
                    osc: byte == b']',
                    escape: false,
                    bytes: 0,
                }),
                b'7' | b'8' | b'D' | b'E' | b'M' => {
                    self.sequence_started = None;
                    self.control(vec![0x1b, byte], output)?;
                }
                0x1b => self.enter(Parser::Escape),
                _ => {
                    self.sequence_started = None;
                }
            },
            Parser::Csi(mut body) => {
                ensure!(body.len() < MAX_CSI, "terminal CSI exceeds its bound");
                if (0x40..=0x7e).contains(&byte) {
                    self.sequence_started = None;
                    if safe_csi(&body, byte) {
                        let mut control = b"\x1b[".to_vec();
                        control.append(&mut body);
                        control.push(byte);
                        self.control(control, output)?;
                    }
                } else if (0x20..=0x3f).contains(&byte) {
                    body.push(byte);
                    self.enter(Parser::Csi(body));
                } else if byte == 0x1b {
                    self.enter(Parser::Escape);
                } else {
                    self.sequence_started = None;
                }
            }
            Parser::String { osc, escape, bytes } => {
                ensure!(
                    bytes < MAX_SEQUENCE,
                    "terminal string control exceeds its bound"
                );
                if byte == 0x9c || escape && byte == b'\\' || osc && byte == 7 {
                    self.sequence_started = None;
                } else {
                    self.enter(Parser::String {
                        osc,
                        escape: byte == 0x1b,
                        bytes: bytes + 1,
                    });
                }
            }
        }
        Ok(())
    }
    fn ground(&mut self, byte: u8, output: &mut Vec<u8>) -> Result<()> {
        if !self.utf8.is_empty() {
            if byte & 0xc0 != 0x80 {
                self.utf8.clear();
                self.text("�".as_bytes(), output)?;
                return self.ground(byte, output);
            }
            self.utf8.push(byte);
            let length = match self.utf8[0] {
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                _ => 4,
            };
            if self.utf8.len() == length {
                let bytes = std::mem::take(&mut self.utf8);
                match std::str::from_utf8(&bytes) {
                    Ok(value) => {
                        let character = value.chars().next().context("missing UTF-8 character")?;
                        if ('\u{80}'..='\u{9f}').contains(&character) {
                            self.c1(character as u8);
                        } else {
                            self.text(&bytes, output)?;
                        }
                    }
                    Err(_) => self.text("�".as_bytes(), output)?,
                }
            }
            return Ok(());
        }
        match byte {
            0x1b => self.enter(Parser::Escape),
            0x80..=0x9f => self.c1(byte),
            b'\n' | b'\r' | b'\t' | 8 | 0x20..=0x7e => self.text(&[byte], output)?,
            0xc2..=0xf4 => self.utf8.push(byte),
            0xa0..=0xc1 | 0xf5..=0xff => self.text("�".as_bytes(), output)?,
            _ => {}
        }
        Ok(())
    }
    fn c1(&mut self, byte: u8) {
        match byte {
            // Consume C1 CSI, but never promote it into an allowed ESC sequence.
            0x9b => self.enter(Parser::Csi(vec![0x80])),
            0x90 | 0x98 | 0x9d | 0x9e | 0x9f => self.enter(Parser::String {
                osc: byte == 0x9d,
                escape: false,
                bytes: 0,
            }),
            _ => {}
        }
    }
}

fn safe_csi(body: &[u8], final_byte: u8) -> bool {
    if matches!(final_byte, b'h' | b'l') {
        return matches!(body, b"?25" | b"?1049" | b"?2004");
    }
    if !body
        .iter()
        .all(|byte| byte.is_ascii_digit() || *byte == b';')
    {
        return false;
    }
    let mut params = Vec::new();
    if !body.is_empty() {
        for part in body.split(|byte| *byte == b';') {
            let value = if part.is_empty() {
                0
            } else {
                let Ok(value) = std::str::from_utf8(part).unwrap_or("").parse::<u32>() else {
                    return false;
                };
                value
            };
            if value > 10000 {
                return false;
            }
            params.push(value);
            if params.len() > 32 {
                return false;
            }
        }
    }
    match final_byte {
        b'm' => {
            let mut index = 0;
            while index < params.len() {
                match params[index] {
                    38 | 48 => match params.get(index + 1) {
                        Some(5) if params.get(index + 2).is_some_and(|value| *value <= 255) => {
                            index += 3
                        }
                        Some(2)
                            if params
                                .get(index + 2..index + 5)
                                .is_some_and(|rgb| rgb.iter().all(|value| *value <= 255)) =>
                        {
                            index += 5
                        }
                        _ => return false,
                    },
                    0..=9
                    | 21..=29
                    | 30..=37
                    | 39
                    | 40..=47
                    | 49..=55
                    | 59..=65
                    | 90..=97
                    | 100..=107 => index += 1,
                    _ => return false,
                }
            }
            true
        }
        b'H' | b'f' => params.len() <= 2,
        b'r' => params.len() <= 2 && (params.len() < 2 || params[1] == 0 || params[0] <= params[1]),
        b'J' | b'K' => params.len() <= 1 && params.first().is_none_or(|value| *value <= 2),
        b's' | b'u' => params.is_empty(),
        b'A' | b'B' | b'C' | b'D' | b'E' | b'F' | b'G' | b'L' | b'M' | b'P' | b'S' | b'T'
        | b'X' | b'@' | b'd' | b'a' | b'e' | b'`' => params.len() <= 1,
        _ => false,
    }
}

fn tty_file() -> Result<File> {
    use std::{
        ffi::CStr,
        os::{
            fd::BorrowedFd,
            unix::{
                ffi::OsStrExt,
                fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
            },
        },
        path::{Component, Path},
    };
    let controlling = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open("/dev/tty")
        .context("interactive execution requires a controlling /dev/tty")?;
    foreground_terminal(controlling.as_raw_fd())?;
    for descriptor in [0, 1, 2] {
        foreground_terminal(descriptor)?;
    }
    // Darwin's /dev/tty is an indirect device that kqueue cannot register with
    // Tokio's normal readiness flags. Keep it only for controlling-TTY attestation;
    // resolve and independently open the real stream device without sharing the
    // shell's file status flags. No fallback to the indirect mux is permitted.
    let indirect = controlling
        .metadata()
        .context("cannot inspect controlling terminal identity")?;
    for descriptor in [0, 1, 2] {
        // SAFETY: these standard stream descriptors have just been checked. The
        // owned duplicate is used only for metadata; its shared flags are never changed.
        let original = File::from(
            unsafe { BorrowedFd::borrow_raw(descriptor) }
                .try_clone_to_owned()
                .context("cannot retain foreground terminal device identity")?,
        );
        let expected = original
            .metadata()
            .context("cannot inspect foreground stream identity")?;
        if expected.dev() == indirect.dev()
            && expected.ino() == indirect.ino()
            && expected.rdev() == indirect.rdev()
        {
            continue;
        }
        let mut name = [0_u8; 4096];
        ensure!(
            unsafe { libc::ttyname_r(descriptor, name.as_mut_ptr().cast(), name.len()) } == 0,
            "cannot resolve the foreground terminal device; no admission or import was performed"
        );
        let name =
            CStr::from_bytes_until_nul(&name).context("terminal device name is not terminated")?;
        let path = Path::new(std::ffi::OsStr::from_bytes(name.to_bytes()));
        if path == Path::new("/dev/tty") {
            continue;
        }
        ensure!(
            path.is_absolute()
                && path.starts_with("/dev")
                && !path
                    .components()
                    .any(|component| matches!(component, Component::ParentDir)),
            "terminal device name is outside the supported device namespace"
        );
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| anyhow::anyhow!("cannot safely open the foreground terminal device"))?;
        let actual = file
            .metadata()
            .context("cannot inspect owned terminal identity")?;
        ensure!(
            expected.file_type().is_char_device()
                && actual.file_type().is_char_device()
                && expected.dev() == actual.dev()
                && expected.ino() == actual.ino()
                && expected.rdev() == actual.rdev(),
            "foreground terminal device changed while opening; no admission or import was performed"
        );
        foreground_terminal(file.as_raw_fd())?;
        foreground_terminal(controlling.as_raw_fd())?;
        return Ok(file);
    }
    bail!(
        "only indirect /dev/tty stream aliases were found; this terminal configuration is unsupported; use direct terminal stdin, stdout and stderr; no admission or import was performed"
    )
}
fn foreground_terminal(descriptor: i32) -> Result<()> {
    let session = unsafe { libc::getsid(0) };
    ensure!(
        session >= 0
            && unsafe { libc::isatty(descriptor) } == 1
            && unsafe { libc::tcgetpgrp(descriptor) } == unsafe { libc::getpgrp() }
            && unsafe { libc::tcgetsid(descriptor) } == session,
        "interactive execution requires the foreground controlling terminal"
    );
    Ok(())
}
/// Validate the terminal and its readiness registration before admission. Requires
/// the active Tokio I/O runtime used by run/resume; it never changes terminal modes.
pub fn validate_controlling_terminal() -> Result<()> {
    use std::io::IsTerminal;
    ensure!(
        std::io::stdin().is_terminal()
            && std::io::stdout().is_terminal()
            && std::io::stderr().is_terminal(),
        "--interactive requires terminal stdin, stdout and stderr; no admission or import was performed"
    );
    let file = tty_file()?;
    ensure!(
        tokio::runtime::Handle::try_current().is_ok(),
        "interactive terminal validation requires an active I/O runtime"
    );
    // SAFETY: the independently opened File owns this exact descriptor for its lifetime.
    let _readiness = unsafe { AsyncFd::register(file) }.map_err(std::io::Error::from)
        .context("foreground terminal device does not support asynchronous readiness; no admission or import was performed")?;
    Ok(())
}

struct TerminalGuard {
    file: File,
    saved: libc::termios,
    raw: bool,
}
impl TerminalGuard {
    fn open() -> Result<Self> {
        validate_controlling_terminal()?;
        let file = tty_file()?;
        let mut saved = std::mem::MaybeUninit::<libc::termios>::uninit();
        ensure!(
            unsafe { libc::tcgetattr(file.as_raw_fd(), saved.as_mut_ptr()) } == 0,
            "cannot save terminal attributes"
        );
        Ok(Self {
            file,
            saved: unsafe { saved.assume_init() },
            raw: false,
        })
    }
    fn enter_raw(&mut self) -> Result<()> {
        let mut raw = self.saved;
        unsafe { libc::cfmakeraw(&mut raw) };
        ensure!(
            unsafe { libc::tcsetattr(self.file.as_raw_fd(), libc::TCSANOW, &raw) } == 0,
            "cannot activate interactive terminal attributes"
        );
        self.raw = true;
        Ok(())
    }
    fn restore(&mut self) -> Result<()> {
        if !self.raw {
            return Ok(());
        }
        // Small, fixed presentation recovery is the sole unfiltered escape source.
        // Nonblocking writes are bounded; tcsetattr is attempted even if presentation fails.
        let mut remaining = PRESENTATION_RESET;
        let mut presentation_ok = true;
        while !remaining.is_empty() {
            let wrote = unsafe {
                libc::write(
                    self.file.as_raw_fd(),
                    remaining.as_ptr().cast(),
                    remaining.len(),
                )
            };
            if wrote > 0 {
                remaining = &remaining[wrote as usize..];
            } else {
                presentation_ok = false;
                break;
            }
        }
        let attributes_ok =
            unsafe { libc::tcsetattr(self.file.as_raw_fd(), libc::TCSANOW, &self.saved) } == 0;
        if attributes_ok {
            self.raw = false;
        }
        ensure!(
            attributes_ok && presentation_ok,
            "terminal restoration incomplete; use stty sane in this terminal"
        );
        Ok(())
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn size(fd: i32) -> Result<libc::winsize> {
    let mut size = std::mem::MaybeUninit::<libc::winsize>::zeroed();
    ensure!(
        unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, size.as_mut_ptr()) } == 0,
        "cannot read terminal size"
    );
    let size = unsafe { size.assume_init() };
    ensure!(
        size.ws_row > 0 && size.ws_col > 0 && size.ws_row <= 10000 && size.ws_col <= 10000,
        "terminal dimensions are unavailable or exceed the transport bound"
    );
    Ok(size)
}
fn resize(master: i32, dimensions: &libc::winsize) -> Result<()> {
    ensure!(
        unsafe { libc::ioctl(master, libc::TIOCSWINSZ, dimensions) } == 0,
        "cannot propagate terminal resize"
    );
    Ok(())
}
fn pty(dimensions: &libc::winsize) -> Result<(OwnedFd, File)> {
    let (mut master, mut slave) = (-1, -1);
    let mut dimensions = *dimensions;
    ensure!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut dimensions,
            )
        } == 0,
        "cannot allocate local terminal transport"
    );
    // openpty has transferred both valid descriptors on success.
    let master = unsafe { OwnedFd::from_raw_fd(master) };
    let slave = unsafe { File::from_raw_fd(slave) };
    for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
        ensure!(
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == 0,
            "cannot protect terminal descriptor"
        );
    }
    ensure!(
        unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } == 0,
        "cannot make terminal transport nonblocking"
    );
    let mut attributes = std::mem::MaybeUninit::<libc::termios>::uninit();
    ensure!(
        unsafe { libc::tcgetattr(slave.as_raw_fd(), attributes.as_mut_ptr()) } == 0,
        "cannot read PTY attributes"
    );
    let mut attributes = unsafe { attributes.assume_init() };
    unsafe { libc::cfmakeraw(&mut attributes) };
    ensure!(
        unsafe { libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &attributes) } == 0,
        "cannot configure PTY attributes"
    );
    Ok((master, slave))
}
async fn read_fd(fd: &AsyncFd<impl AsRawFd>, buffer: &mut [u8]) -> std::io::Result<usize> {
    loop {
        let mut ready = fd.readable().await?;
        match ready.try_io(|inner| read_now(inner.as_raw_fd(), buffer)) {
            Ok(result) => return result,
            Err(_) => continue,
        }
    }
}
fn read_now(fd: i32, buffer: &mut [u8]) -> std::io::Result<usize> {
    let result = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(result as usize)
    }
}
async fn write_fd(fd: &AsyncFd<impl AsRawFd>, mut bytes: &[u8]) -> Result<()> {
    let write = async {
        while !bytes.is_empty() {
            let mut ready = fd.writable().await?;
            match ready.try_io(|inner| {
                let result =
                    unsafe { libc::write(inner.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
                if result < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(result as usize)
                }
            }) {
                Ok(Ok(0)) => bail!("terminal transport write closed"),
                Ok(Ok(count)) => bytes = &bytes[count..],
                Ok(Err(error)) => return Err(error.into()),
                Err(_) => continue,
            }
        }
        Ok(())
    };
    tokio::time::timeout(Duration::from_secs(2), write)
        .await
        .context("terminal transport write stalled")?
}

pub(super) struct TerminalSession {
    terminal: TerminalGuard,
    physical: AsyncFd<File>,
    master: AsyncFd<OwnedFd>,
    /// Prevent Darwin's last-slave close from discarding unread terminal output.
    /// Released only after a reaped producer's real queue is drained to EAGAIN.
    slave_keeper: Option<File>,
    transport_pid: libc::pid_t,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalEnd {
    RuntimeStreamEnded,
    PhysicalDisconnected,
    OperatorStop,
}
impl TerminalEnd {
    pub(super) const fn reason(self) -> &'static str {
        match self {
            Self::RuntimeStreamEnded => "runtime terminal transport ended",
            Self::PhysicalDisconnected => "physical terminal disconnected; worker stop requested",
            Self::OperatorStop => "operator requested worker stop with Ctrl-X Ctrl-X",
        }
    }
}
pub(super) enum TransportIssue {
    End(TerminalEnd),
    Failure(String),
    Deadline,
    ExternalSignal,
}
impl TransportIssue {
    pub(super) fn reason(self, independently_terminal: bool) -> Option<String> {
        match self {
            Self::End(TerminalEnd::RuntimeStreamEnded) if independently_terminal => None,
            Self::End(end) => Some(end.reason().into()),
            Self::Failure(reason) => Some(reason),
            Self::Deadline => Some("authorized worker duration elapsed".into()),
            Self::ExternalSignal => Some("external operator signal requested worker stop; terminal suspension is not supported".into()),
        }
    }
}
/// Consuming the builder is significant: Command retains its Stdio values for reuse.
/// No parent copy of the slave may keep the master artificially alive after child exit.
fn spawn_transport(
    command: Command,
    dimensions: &libc::winsize,
) -> Result<(AsyncFd<OwnedFd>, File, Child)> {
    spawn_transport_with_veto(command, dimensions, None)
}
fn spawn_transport_with_veto(
    mut command: Command,
    dimensions: &libc::winsize,
    veto: Option<(super::LegacyStartVeto, String)>,
) -> Result<(AsyncFd<OwnedFd>, File, Child)> {
    let (master, slave) = pty(dimensions)?;
    let slave_keeper = slave.try_clone()?;
    // SAFETY: OwnedFd retains this exact open descriptor until AsyncFd is dropped.
    let master = unsafe { AsyncFd::register(master) }
        .map_err(std::io::Error::from)
        .context("cannot register the runtime PTY master for asynchronous readiness")?;
    command
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave))
        .env("TERM", TERM)
        .kill_on_drop(true);
    // No controlling TTY is acquired. Darwin revokes a session leader's controlling
    // terminal on exit, discarding its unread output even if another slave fd is open.
    // setsid alone isolates the transport from the broker's physical /dev/tty;
    // isatty/termios/sizing still operate on its three real PTY slave descriptors.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    // All PTY, stdio and pre_exec preparation is complete. Consume the same
    // retained control-owner veto at the actual final process boundary.
    if let Some((veto, target)) = veto {
        veto.check(&target)?;
    }
    let child = command
        .spawn()
        .context("cannot start filtered terminal transport")?;
    drop(command);
    Ok((master, slave_keeper, child))
}
impl TerminalSession {
    pub(super) fn spawn(command: Command) -> Result<(Self, Child)> {
        Self::spawn_checked(command, None)
    }
    pub(super) fn spawn_with_veto(
        command: Command,
        veto: super::LegacyStartVeto,
        target: String,
    ) -> Result<(Self, Child)> {
        Self::spawn_checked(command, Some((veto, target)))
    }
    fn spawn_checked(
        command: Command,
        veto: Option<(super::LegacyStartVeto, String)>,
    ) -> Result<(Self, Child)> {
        let mut terminal = TerminalGuard::open()?;
        let dimensions = size(terminal.file.as_raw_fd())?;
        // SAFETY: File/OwnedFd retain these exact open descriptors until AsyncFd is dropped.
        let physical = unsafe { AsyncFd::register(terminal.file.try_clone()?) }
            .map_err(std::io::Error::from)
            .context(
                "cannot register the validated foreground terminal for asynchronous readiness",
            )?;
        use std::io::Write;
        std::io::stderr().write_all(b"Interactive worker: Ctrl-X Ctrl-X requests stop. Typed Ctrl-C is sent to the worker.\n")?;
        terminal.enter_raw()?;
        let (master, slave_keeper, child) = match veto {
            Some(veto) => spawn_transport_with_veto(command, &dimensions, Some(veto)),
            None => spawn_transport(command, &dimensions),
        }?;
        let transport_pid = libc::pid_t::try_from(
            child
                .id()
                .context("terminal transport has no child identity")?,
        )?;
        Ok((
            Self {
                terminal,
                physical,
                master,
                slave_keeper: Some(slave_keeper),
                transport_pid,
            },
            child,
        ))
    }
    pub(super) fn restore(&mut self) -> Result<()> {
        self.terminal.restore()
    }
    pub(super) async fn pump(
        &mut self,
        secrets: Vec<String>,
        mut draining: tokio::sync::watch::Receiver<bool>,
    ) -> Result<TerminalEnd> {
        let mut filter = DisplayFilter::new(secrets)?;
        let mut input = [0; 4096];
        let mut output = [0; 4096];
        let mut escape = false;
        let mut drained_bytes = 0;
        let mut window =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())?;
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        loop {
            let drain_requested = *draining.borrow();
            if drain_requested && self.slave_keeper.is_some() {
                // Keep the queue alive through slow observation callbacks and producer exit.
                // EAGAIN is a measured empty queue, not EOF: close our explicit keeper
                // only then, and await the separate real EOF/EIO proof below.
                match read_now(self.master.as_raw_fd(), &mut output) {
                    Ok(count) if count > 0 => {
                        drained_bytes += count;
                        ensure!(
                            drained_bytes <= MAX_DRAIN_BYTES,
                            "runtime terminal drain exceeded its byte bound without EOF"
                        );
                        let safe = filter.push(&output[..count])?;
                        write_fd(&self.physical, &safe).await?;
                        tokio::task::yield_now().await;
                        continue;
                    }
                    Ok(_) => {
                        drop(self.slave_keeper.take());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        drop(self.slave_keeper.take());
                        continue;
                    }
                    Err(error) if error.raw_os_error() == Some(libc::EIO) => {
                        drop(self.slave_keeper.take());
                    }
                    Err(error) => return Err(error).context("runtime terminal queue drain failed"),
                }
            }
            tokio::select! {
                result = read_fd(&self.physical, &mut input), if !drain_requested => {
                    let count = result.context("physical terminal read failed")?;
                    if count == 0 { return Ok(TerminalEnd::PhysicalDisconnected); }
                    let mut safe = Vec::with_capacity(count + 1);
                    for &byte in &input[..count] {
                        if escape {
                            escape = false;
                            if byte == 0x18 { return Ok(TerminalEnd::OperatorStop); }
                            safe.push(0x18);
                        }
                        if byte == 0x18 { escape = true; } else { safe.push(byte); }
                    }
                    // A finished transport disables input even if a previous write was backpressured.
                    if !*draining.borrow() {
                        tokio::select! {
                            biased;
                            changed = draining.changed() => { changed.context("terminal drain controller disconnected")?; }
                            result = write_fd(&self.master, &safe) => { result?; }
                        }
                    }
                }
                result = read_fd(&self.master, &mut output) => {
                    let count = match result {
                        Ok(count) => count,
                        Err(error) if error.raw_os_error() == Some(libc::EIO) => 0,
                        Err(error) => return Err(error).context("runtime terminal transport disconnected"),
                    };
                    if count == 0 {
                        write_fd(&self.physical, &filter.finish()?).await?;
                        return Ok(TerminalEnd::RuntimeStreamEnded);
                    }
                    if drain_requested {
                        drained_bytes += count;
                        ensure!(drained_bytes <= MAX_DRAIN_BYTES, "runtime terminal drain exceeded its byte bound without EOF");
                    }
                    let safe = filter.push(&output[..count])?;
                    write_fd(&self.physical, &safe).await?;
                    tokio::task::yield_now().await;
                }
                value = window.recv(), if !drain_requested => {
                    ensure!(value.is_some(), "terminal resize signal stream ended");
                    // child.wait synchronously sets draining before the pump is polled
                    // again. Never signal a PID once it may have been reaped/reused.
                    if *draining.borrow() { continue; }
                    resize(self.master.as_raw_fd(), &size(self.physical.as_raw_fd())?)?;
                    if unsafe { libc::kill(self.transport_pid, libc::SIGWINCH) } < 0 {
                        let error = std::io::Error::last_os_error();
                        if error.raw_os_error() != Some(libc::ESRCH) {
                            return Err(error).context("cannot signal owned terminal transport resize");
                        }
                    }
                }
                changed = draining.changed(), if !drain_requested => {
                    changed.context("terminal drain controller disconnected")?;
                }
                _ = interval.tick() => { filter.check_deadline()?; }
            }
        }
    }
}

pub(super) struct ExternalSignals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
    hangup: tokio::signal::unix::Signal,
    suspend: tokio::signal::unix::Signal,
    quit: tokio::signal::unix::Signal,
}
impl ExternalSignals {
    pub(super) fn new() -> Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
            hangup: signal(SignalKind::hangup())?,
            suspend: signal(SignalKind::from_raw(libc::SIGTSTP))?,
            quit: signal(SignalKind::quit())?,
        })
    }
    pub(super) async fn received(&mut self) {
        tokio::select! { _ = self.interrupt.recv() => {}, _ = self.terminate.recv() => {},
        _ = self.hangup.recv() => {}, _ = self.suspend.recv() => {}, _ = self.quit.recv() => {} }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn controlling_terminal_opens_owned_readiness_capable_device_without_changing_stdin_flags()
     {
        const CHILD: &str = "AGENTSANDBOX_TEST_CONTROLLING_TTY_CHILD";
        if let Some(mode) = std::env::var_os(CHILD) {
            if mode != "direct" {
                let alias = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open("/dev/tty")
                    .unwrap();
                let descriptors: &[i32] = if mode == "all_alias" {
                    &[0, 1, 2]
                } else {
                    &[0]
                };
                for descriptor in descriptors {
                    assert_eq!(
                        unsafe { libc::dup2(alias.as_raw_fd(), *descriptor) },
                        *descriptor
                    );
                }
            }
            let before = unsafe { libc::fcntl(0, libc::F_GETFL) };
            assert!(before >= 0);
            if mode == "all_alias" {
                let error = validate_controlling_terminal().unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("only indirect /dev/tty stream aliases were found")
                );
                assert_eq!(unsafe { libc::fcntl(0, libc::F_GETFL) }, before);
                return;
            }
            let terminal = TerminalGuard::open().unwrap();
            let descriptor = terminal.file.try_clone().unwrap();
            // Real controlling /dev/tty context, not an already-open slave passed
            // directly to AsyncFd: the Darwin alias fails this registration.
            let physical = unsafe { AsyncFd::register(descriptor) }
                .expect("validated controlling terminal must support async readiness");
            assert_ne!(
                unsafe { libc::fcntl(physical.as_raw_fd(), libc::F_GETFL) } & libc::O_NONBLOCK,
                0
            );
            assert_eq!(unsafe { libc::fcntl(0, libc::F_GETFL) }, before);
            return;
        }
        for mode in ["direct", "alias_stdin", "all_alias"] {
            let dimensions = libc::winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            let (master, slave) = pty(&dimensions).unwrap();
            let master = unsafe { AsyncFd::register(master) }.unwrap();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", "runtime::terminal::tests::controlling_terminal_opens_owned_readiness_capable_device_without_changing_stdin_flags", "--nocapture", "--test-threads=1"])
            .env(CHILD, mode).env("RUST_BACKTRACE", "0")
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave)).kill_on_drop(true);
            // This test-only child owns the private physical terminal. It never owns a worker.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    #[allow(clippy::unnecessary_cast)]
                    let request = libc::TIOCSCTTY as libc::c_ulong;
                    if libc::ioctl(0, request, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let mut child = command.spawn().unwrap();
            drop(command);
            let reader = tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut buffer = [0; 1024];
                loop {
                    match read_fd(&master, &mut buffer).await {
                        Ok(0) => break,
                        Ok(count) => {
                            if bytes.len() < 16 * 1024 {
                                bytes.extend_from_slice(&buffer[..count]);
                            }
                        }
                        Err(_) => break,
                    }
                }
                bytes
            });
            let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
                .await
                .unwrap()
                .unwrap();
            let output = tokio::time::timeout(Duration::from_secs(2), reader)
                .await
                .unwrap()
                .unwrap();
            assert!(
                status.success(),
                "controlling-terminal child {mode} failed: {}",
                String::from_utf8_lossy(&output)
            );
        }
    }
    fn filtered(parts: &[&[u8]], secrets: Vec<String>) -> Result<Vec<u8>> {
        let mut filter = DisplayFilter::new(secrets)?;
        let mut output = Vec::new();
        for part in parts {
            output.extend(filter.push(part)?);
        }
        output.extend(filter.finish()?);
        Ok(output)
    }
    #[test]
    fn dangerous_families_are_removed_at_every_partition() {
        for control in [
            b"\x1b]52;c;Zm9v\x07".as_slice(),
            b"\x1b]8;;https://invalid\x1b\\",
            b"\x1bPpayload\x1b\\",
            b"\x1b_payload\x1b\\",
            b"\x1b^payload\x1b\\",
            b"\x1bXpayload\x1b\\",
            b"\x1b[8;40;120t",
            b"\x1b[5i",
            b"\x1b[?1000h",
            b"\xc2\x9d52;ignored\x07",
            b"\x9d52;ignored\x9c",
            b"\x9b31m",
            b"\xc2\x9b31m",
        ] {
            let input = [b"before".as_slice(), control, b"after"].concat();
            for split in 0..=input.len() {
                assert_eq!(
                    filtered(&[&input[..split], &input[split..]], vec![]).unwrap(),
                    b"beforeafter"
                );
            }
        }
    }
    #[test]
    fn utf8_and_allowed_cursor_sequences_survive_all_partitions() {
        let input = "é🙂\x1b[38;2;2;50;255mhello\x1b[?1049h\x1b[2J\x1b[12;5H\x1b[?25l\x1b[?2004h"
            .as_bytes();
        for split in 0..=input.len() {
            assert_eq!(
                filtered(&[&input[..split], &input[split..]], vec![]).unwrap(),
                input
            );
        }
    }
    #[test]
    fn known_literals_are_masked_across_chunks_and_safe_controls_without_prefix_flush() {
        let secret = "abcdef";
        let input = b"safe ab\x1b[31mcdef abcd";
        for split in 0..=input.len() {
            let output =
                filtered(&[&input[..split], &input[split..]], vec![secret.into()]).unwrap();
            assert_eq!(output, b"safe [REDACTED]\x1b[31m [REDACTED]");
        }
        assert_eq!(
            filtered(&[b"abababa!"], vec!["ababa".into(), "babab".into()]).unwrap(),
            b"[REDACTED]!"
        );
    }
    #[test]
    fn sequences_and_pending_mask_window_have_strict_bounds() {
        assert!(safe_csi(&b";".repeat(31), b'm'));
        assert!(!safe_csi(&b";".repeat(32), b'm'));
        assert!(!safe_csi(b"1;10001", b'H'));
        let mut filter = DisplayFilter::new(vec![]).unwrap();
        assert!(
            filter
                .push(&[b"\x1b[".as_slice(), &vec![b';'; MAX_CSI + 1]].concat())
                .is_err()
        );
        let mut filter = DisplayFilter::new(vec![]).unwrap();
        assert!(
            filter
                .push(&[b"\x1b]52;".as_slice(), &vec![b'x'; MAX_SEQUENCE + 1]].concat())
                .is_err()
        );
        let mut filter = DisplayFilter::new(vec!["abcd".into()]).unwrap();
        filter.push(b"abc").unwrap();
        let controls = b"\x1b[31m".repeat(MAX_PENDING / 5 + 1);
        assert!(filter.push(&controls).is_err());
        assert!(filter.pending_bytes <= MAX_PENDING + MAX_CSI);
    }
    #[tokio::test]
    async fn real_local_pty_transports_bytes_and_resizes() {
        let initial = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let (master, mut slave) = pty(&initial).unwrap();
        assert_eq!(unsafe { libc::isatty(slave.as_raw_fd()) }, 1);
        assert_eq!(size(slave.as_raw_fd()).unwrap().ws_col, 80);
        let changed = libc::winsize {
            ws_row: 40,
            ws_col: 120,
            ..initial
        };
        resize(master.as_raw_fd(), &changed).unwrap();
        assert_eq!(size(slave.as_raw_fd()).unwrap().ws_row, 40);
        let master = unsafe { AsyncFd::register(master) }.unwrap();
        use std::io::Write;
        slave.write_all(b"physical-pty").unwrap();
        let mut bytes = [0; 32];
        let count = read_fd(&master, &mut bytes).await.unwrap();
        assert_eq!(&bytes[..count], b"physical-pty");
    }

    fn attributes(fd: i32) -> libc::termios {
        let mut attributes = std::mem::MaybeUninit::<libc::termios>::uninit();
        assert_eq!(unsafe { libc::tcgetattr(fd, attributes.as_mut_ptr()) }, 0);
        unsafe { attributes.assume_init() }
    }
    fn assert_same_attributes(actual: libc::termios, expected: libc::termios) {
        assert_eq!(actual.c_iflag, expected.c_iflag);
        assert_eq!(actual.c_oflag, expected.c_oflag);
        assert_eq!(actual.c_cflag, expected.c_cflag);
        assert_eq!(actual.c_lflag, expected.c_lflag);
        assert_eq!(actual.c_cc, expected.c_cc);
    }
    fn guarded_pty() -> (OwnedFd, File, TerminalGuard) {
        let dimensions = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let (master, slave) = pty(&dimensions).unwrap();
        let mut saved = attributes(slave.as_raw_fd());
        saved.c_lflag |= libc::ICANON | libc::ECHO | libc::ISIG;
        saved.c_iflag |= libc::ICRNL;
        saved.c_oflag |= libc::OPOST;
        assert_eq!(
            unsafe { libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &saved) },
            0
        );
        // Save the kernel's accepted state, exactly as TerminalGuard::open does.
        // Darwin adds PENDIN when ICANON is enabled without flushing pending input
        // (XNU bsd/kern/tty.c, TIOCSETA, ICANON transition and final t_lflag assignment).
        // Comparing to the pre-tcsetattr request would mistake kernel state for a lost setting.
        let requested = saved;
        let saved = attributes(slave.as_raw_fd());
        #[cfg(target_os = "macos")]
        assert_eq!(saved.c_lflag, requested.c_lflag | libc::PENDIN);
        assert_eq!(saved.c_iflag, requested.c_iflag);
        assert_eq!(saved.c_oflag, requested.c_oflag);
        assert_eq!(saved.c_cflag, requested.c_cflag);
        assert_eq!(saved.c_cc, requested.c_cc);
        let guard = TerminalGuard {
            file: slave.try_clone().unwrap(),
            saved,
            raw: false,
        };
        (master, slave, guard)
    }
    #[test]
    fn guard_explicit_restore_and_drop_restore_presentation_and_attributes() {
        use std::io::Read;
        let (master, slave, mut guard) = guarded_pty();
        let saved = guard.saved;
        guard.enter_raw().unwrap();
        assert_eq!(
            attributes(slave.as_raw_fd()).c_lflag & (libc::ICANON | libc::ECHO | libc::ISIG),
            0
        );
        guard.restore().unwrap();
        guard.restore().unwrap();
        assert_same_attributes(attributes(slave.as_raw_fd()), saved);
        let mut master = File::from(master);
        let mut output = [0; 128];
        let count = master.read(&mut output).unwrap();
        assert_eq!(&output[..count], PRESENTATION_RESET);
        guard.enter_raw().unwrap();
        drop(guard);
        assert_same_attributes(attributes(slave.as_raw_fd()), saved);
    }
    #[tokio::test]
    async fn task_cancellation_restores_real_pty_attributes() {
        let (_master, slave, mut guard) = guarded_pty();
        let saved = guard.saved;
        let (ready, entered) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            guard.enter_raw().unwrap();
            let _ = ready.send(());
            std::future::pending::<()>().await;
            drop(guard);
        });
        entered.await.unwrap();
        assert_eq!(attributes(slave.as_raw_fd()).c_lflag & libc::ICANON, 0);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_same_attributes(attributes(slave.as_raw_fd()), saved);
    }
    #[test]
    fn terminal_worker_proof_only_neutralizes_normal_transport_eof() {
        assert!(
            TransportIssue::End(TerminalEnd::RuntimeStreamEnded)
                .reason(true)
                .is_none()
        );
        assert!(
            TransportIssue::End(TerminalEnd::RuntimeStreamEnded)
                .reason(false)
                .is_some()
        );
        for issue in [
            TransportIssue::End(TerminalEnd::OperatorStop),
            TransportIssue::End(TerminalEnd::PhysicalDisconnected),
            TransportIssue::Failure("terminal filter bound exceeded".into()),
            TransportIssue::Deadline,
            TransportIssue::ExternalSignal,
        ] {
            assert!(
                issue.reason(true).is_some(),
                "explicit transport faults survive a real worker exit 0"
            );
        }
    }
    #[test]
    fn presentation_reset_restores_authorized_scroll_region() {
        assert!(safe_csi(b"2;20", b'r'));
        assert!(
            PRESENTATION_RESET
                .windows(3)
                .any(|bytes| bytes == b"\x1b[r"),
            "authorized scrolling margins must be reset by the trusted presentation cleanup"
        );
    }

    #[tokio::test]
    async fn final_veto_refuses_after_pty_preparation_before_transport_spawn() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        #[derive(Debug)]
        struct FinalVetoRefusal;
        impl std::fmt::Display for FinalVetoRefusal {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("synthetic final veto refusal")
            }
        }
        impl std::error::Error for FinalVetoRefusal {}
        struct Refuse {
            checked: Arc<AtomicBool>,
        }
        impl super::super::BeforeSpawnGuard for Refuse {
            fn check_before_spawn(self: Box<Self>, target: &str) -> Result<()> {
                assert_eq!(target, "exact-owned-target");
                self.checked.store(true, Ordering::SeqCst);
                Err(FinalVetoRefusal.into())
            }
        }
        let checked = Arc::new(AtomicBool::new(false));
        let veto = super::super::LegacyStartVeto::new(Box::new(Refuse {
            checked: Arc::clone(&checked),
        }));
        let dimensions = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // Even a nonexistent executable must never reach Command::spawn when
        // this final restriction refuses. The exact original cause is returned.
        let command = Command::new("/nonexistent/d1-final-veto-must-not-spawn");
        let error = match spawn_transport_with_veto(
            command,
            &dimensions,
            Some((veto, "exact-owned-target".into())),
        ) {
            Ok(_) => panic!("refused transport must not be spawned"),
            Err(error) => error,
        };
        assert!(checked.load(Ordering::SeqCst));
        assert!(error.is::<FinalVetoRefusal>());
    }

    async fn queued_tail(payload: &str, secrets: Vec<String>) -> (Result<TerminalEnd>, Vec<u8>) {
        let (physical_master, _physical_slave, mut terminal) = guarded_pty();
        terminal.enter_raw().unwrap();
        let physical = unsafe { AsyncFd::register(terminal.file.try_clone().unwrap()) }.unwrap();
        let dimensions = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let mut command = Command::new("/usr/bin/printf");
        command.args(["%s", payload]);
        let (master, slave_keeper, mut child) = spawn_transport(command, &dimensions).unwrap();
        let transport_pid = libc::pid_t::try_from(child.id().unwrap()).unwrap();
        assert_eq!(unsafe { libc::isatty(slave_keeper.as_raw_fd()) }, 1);
        assert_eq!(
            unsafe { libc::tcgetsid(slave_keeper.as_raw_fd()) },
            -1,
            "the transport PTY must not become a session's controlling terminal"
        );
        // Intentionally reap before the first pump poll: every byte is a post-exit queued tail.
        assert!(child.wait().await.unwrap().success());
        // Stronger than a slow audit callback: the producer is already gone and
        // no pump poll has run. Its actual queued bytes must still be preserved.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let mut session = TerminalSession {
            terminal,
            physical,
            master,
            slave_keeper: Some(slave_keeper),
            transport_pid,
        };
        let (_drain, draining) = tokio::sync::watch::channel(true);
        let result = tokio::time::timeout(DRAIN_TIMEOUT, session.pump(secrets, draining))
            .await
            .expect("the real slave must close; timeout is not an EOF proof");
        let physical_master = unsafe { AsyncFd::register(physical_master) }.unwrap();
        let mut output = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            match tokio::time::timeout(
                Duration::from_millis(20),
                read_fd(&physical_master, &mut buffer),
            )
            .await
            {
                Ok(Ok(0)) | Err(_) => break,
                Ok(Ok(count)) => output.extend_from_slice(&buffer[..count]),
                Ok(Err(error)) => panic!("physical test terminal read failed: {error}"),
            }
        }
        (result, output)
    }
    #[tokio::test]
    async fn reaped_transport_drains_actual_pty_tail_until_real_eof_and_masks_private_suffix() {
        let (result, output) = queued_tail(
            "queued tail \x1b[31mknown-fixt",
            vec!["known-fixture-credential".into()],
        )
        .await;
        assert_eq!(result.unwrap(), TerminalEnd::RuntimeStreamEnded);
        assert_eq!(output, b"queued tail \x1b[31m[REDACTED]");
    }
    #[tokio::test]
    async fn reaped_transport_cannot_hide_a_filter_failure_or_incomplete_control_in_its_tail() {
        let overlong = format!("\x1b[{}", ";".repeat(MAX_CSI + 1));
        for (payload, expected) in [
            (overlong.as_str(), "CSI exceeds its bound"),
            ("\x1b]52;unfinished", "incomplete control sequence"),
        ] {
            let (result, output) = queued_tail(payload, vec![]).await;
            assert!(result.unwrap_err().to_string().contains(expected));
            assert!(
                output.is_empty(),
                "rejected tail must never be forwarded raw"
            );
        }
    }
}
