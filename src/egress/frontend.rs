//! Untrusted, in-memory HTTP CONNECT adapter. This module opens no listener,
//! resolves no name and holds no control credential or execution permission.
//! A local success transition only consumes a broker response; the trusted
//! owner must independently enforce binding, durable authorization and IO.

const MAX_HEAD_BYTES: usize = 16 * 1024;
const MAX_HEADERS: usize = 32;
const MAX_AUTHORITY_BYTES: usize = 512;
const HEAD_TIMEOUT_MS: u64 = 5_000;
const OPEN_TIMEOUT_MS: u64 = 10_000;
const IDLE_TIMEOUT_MS: u64 = 60_000;
const SUCCESS: &[u8] = b"HTTP/1.1 200 Connection Established\r\n\r\n";
const REFUSED: &[u8] = b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";

/// Closed diagnostics intentionally retain neither authority nor header bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrontendError {
    Size,
    Syntax,
    Method,
    Version,
    Authority,
    Header,
    PrematureData,
    State,
    Timeout,
    Clock,
}
impl std::fmt::Display for FrontendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Size => "CONNECT input exceeds its bound",
            Self::Syntax => "CONNECT framing is incompatible",
            Self::Method => "only CONNECT is supported",
            Self::Version => "CONNECT requires HTTP/1.1",
            Self::Authority => "CONNECT authority is incompatible",
            Self::Header => "CONNECT headers are incompatible",
            Self::PrematureData => "CONNECT data arrived before broker acceptance",
            Self::State => "CONNECT flow is not in the required state",
            Self::Timeout => "CONNECT flow deadline expired",
            Self::Clock => "CONNECT monotone clock moved backwards",
        })
    }
}
impl std::error::Error for FrontendError {}

/// Borrowed raw request target, never an identity or validated domain object.
pub struct ConnectHead<'a> {
    authority: &'a str,
}
impl ConnectHead<'_> {
    pub fn raw_authority(&self) -> &str {
        self.authority
    }
}

/// Parse one complete head without allocation. Callers may accumulate at most
/// MAX_HEAD_BYTES and must retain the original header deadline across fragments.
/// The standard HTTP parser owns syntax; the checks here restrict its accepted
/// grammar to this closed CONNECT profile. No header is forwarded upstream.
pub fn parse_connect_head(input: &[u8]) -> Result<Option<ConnectHead<'_>>, FrontendError> {
    if input.len() > MAX_HEAD_BYTES {
        return Err(FrontendError::Size);
    }
    for (index, byte) in input.iter().enumerate() {
        if *byte == b'\n' && (index == 0 || input[index - 1] != b'\r')
            || *byte == b'\r' && input.get(index + 1).is_some_and(|next| *next != b'\n')
        {
            return Err(FrontendError::Syntax);
        }
    }
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut request = httparse::Request::new(&mut headers);
    let status = request.parse(input).map_err(|_| FrontendError::Syntax)?;
    let httparse::Status::Complete(consumed) = status else {
        return if input.len() == MAX_HEAD_BYTES {
            Err(FrontendError::Size)
        } else {
            Ok(None)
        };
    };
    if consumed != input.len() {
        return Err(FrontendError::PrematureData);
    }
    if request.method != Some("CONNECT") {
        return Err(FrontendError::Method);
    }
    if request.version != Some(1) {
        return Err(FrontendError::Version);
    }
    let authority = request.path.ok_or(FrontendError::Authority)?;
    if authority.is_empty()
        || authority.len() > MAX_AUTHORITY_BYTES
        || !authority
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':'))
    {
        return Err(FrontendError::Authority);
    }
    let Some((name, port)) = authority.split_once(':') else {
        return Err(FrontendError::Authority);
    };
    if name.is_empty() || port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(FrontendError::Authority);
    }
    let line_end = input
        .windows(2)
        .position(|pair| pair == b"\r\n")
        .ok_or(FrontendError::Syntax)?;
    let line = &input[..line_end];
    if !line.starts_with(b"CONNECT ")
        || !line.ends_with(b" HTTP/1.1")
        || line.len() != 17 + authority.len()
        || &line[8..line.len() - 9] != authority.as_bytes()
    {
        return Err(FrontendError::Syntax);
    }
    let mut host_seen = false;
    for header in request.headers {
        if header.name.eq_ignore_ascii_case("host") {
            if host_seen || header.value != authority.as_bytes() {
                return Err(FrontendError::Header);
            }
            host_seen = true;
        } else if [
            "content-length",
            "transfer-encoding",
            "authorization",
            "proxy-authorization",
        ]
        .iter()
        .any(|name| header.name.eq_ignore_ascii_case(name))
        {
            return Err(FrontendError::Header);
        }
    }
    if !host_seen {
        return Err(FrontendError::Header);
    }
    Ok(Some(ConnectHead { authority }))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Reading,
    Pending,
    Tunnel,
    Closed,
}

/// One local flow, without bytes, credentials or authoritative permission.
pub struct ConnectState {
    phase: Phase,
    phase_since: u64,
    last_now: u64,
}
impl ConnectState {
    pub fn new(now_ms: u64) -> Self {
        Self {
            phase: Phase::Reading,
            phase_since: now_ms,
            last_now: now_ms,
        }
    }

    pub fn receive_head<'a>(
        &mut self,
        bytes: &'a [u8],
        now_ms: u64,
    ) -> Result<Option<ConnectHead<'a>>, FrontendError> {
        let result = (|| {
            self.check(now_ms)?;
            if self.phase != Phase::Reading {
                return Err(FrontendError::State);
            }
            let head = parse_connect_head(bytes)?;
            if head.is_some() {
                self.phase = Phase::Pending;
                self.phase_since = now_ms;
            }
            Ok(head)
        })();
        self.finish(result)
    }

    pub fn broker_accepted(&mut self, now_ms: u64) -> Result<&'static [u8], FrontendError> {
        let result = (|| {
            self.check(now_ms)?;
            if self.phase != Phase::Pending {
                return Err(FrontendError::State);
            }
            self.phase = Phase::Tunnel;
            self.phase_since = now_ms;
            Ok(SUCCESS)
        })();
        self.finish(result)
    }

    pub fn broker_rejected(&mut self, now_ms: u64) -> Result<&'static [u8], FrontendError> {
        let result = (|| {
            self.check(now_ms)?;
            if self.phase != Phase::Pending {
                return Err(FrontendError::State);
            }
            self.close();
            Ok(REFUSED)
        })();
        self.finish(result)
    }

    pub fn tunnel_data(&mut self, bytes: &[u8], now_ms: u64) -> Result<(), FrontendError> {
        let result = (|| {
            self.check(now_ms)?;
            if self.phase != Phase::Tunnel {
                return Err(FrontendError::State);
            }
            if bytes.is_empty() || bytes.len() > super::protocol::MAX_DATA_BYTES {
                return Err(FrontendError::Size);
            }
            self.phase_since = now_ms;
            Ok(())
        })();
        self.finish(result)
    }

    fn check(&mut self, now_ms: u64) -> Result<(), FrontendError> {
        if self.phase == Phase::Closed {
            return Err(FrontendError::State);
        }
        if now_ms < self.last_now {
            return Err(FrontendError::Clock);
        }
        self.last_now = now_ms;
        let limit = match self.phase {
            Phase::Reading => HEAD_TIMEOUT_MS,
            Phase::Pending => OPEN_TIMEOUT_MS,
            Phase::Tunnel => IDLE_TIMEOUT_MS,
            Phase::Closed => return Err(FrontendError::State),
        };
        if now_ms - self.phase_since >= limit {
            Err(FrontendError::Timeout)
        } else {
            Ok(())
        }
    }

    fn finish<T>(&mut self, result: Result<T, FrontendError>) -> Result<T, FrontendError> {
        if result.is_err() {
            self.close();
        }
        result
    }

    pub fn close(&mut self) {
        self.phase = Phase::Closed;
    }
    pub fn is_tunnel(&self) -> bool {
        self.phase == Phase::Tunnel
    }
    pub fn is_closed(&self) -> bool {
        self.phase == Phase::Closed
    }
}
