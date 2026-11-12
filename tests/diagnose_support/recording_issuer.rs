//! A loopback issuer that keeps what it is sent.
//!
//! It answers the discovery document and the keys of its own issuer, and it
//! keeps the request line, the header names and the size of the body of
//! every request. It reads the bytes of each connection itself: what it keeps
//! is what was sent, not what a server library made of it. Header values are
//! not kept. Its key is a public number that belongs to no key pair: it
//! verifies nothing and protects nothing.

use serde_json::json;
use std::{
    io::{Error, ErrorKind, Read, Result, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// The path of the issuer below its address.
const REALM: &str = "/realm";
const MAX_HEAD_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024;
const EXCHANGE_BOUND: Duration = Duration::from_secs(10);

/// One request, as it arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedRequest {
    /// The first line: method, target and protocol version.
    pub line: String,
    /// The header names in the order sent, in lower case.
    pub header_names: Vec<String>,
    /// The bytes that followed the head.
    pub body_bytes: usize,
}

pub struct RecordingIssuer {
    pub issuer: String,
    address: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    stopped: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
}

impl RecordingIssuer {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the recording issuer");
        let address = listener.local_addr().expect("the address of the recorder");
        let issuer = format!("http://{address}{REALM}");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stopped = Arc::new(AtomicBool::new(false));
        let server = {
            let (issuer, requests, stopped) = (issuer.clone(), requests.clone(), stopped.clone());
            thread::spawn(move || {
                for stream in listener.incoming() {
                    if stopped.load(Ordering::SeqCst) {
                        break;
                    }
                    // One request per connection. An exchange that fails
                    // leaves no record or no answer: the test sees either.
                    if let Ok(stream) = stream {
                        let _ = exchange(stream, &issuer, &requests);
                    }
                }
            })
        };
        Self {
            issuer,
            address,
            requests,
            stopped,
            server: Some(server),
        }
    }

    /// Every request received so far, in order.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("the record").clone()
    }
}

impl Drop for RecordingIssuer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        // Wake the accept loop so that it sees the flag. A loop that could
        // not be woken is left to the end of the test process: joining it
        // would wait for a connection that never comes.
        let woken = TcpStream::connect_timeout(&self.address, EXCHANGE_BOUND).is_ok();
        if let Some(server) = self.server.take()
            && woken
        {
            let _ = server.join();
        }
    }
}

/// The endpoints are those of the repository's controlled issuer, so that a
/// provider block written for that one fits this one once its issuer is
/// replaced.
fn discovery_document(issuer: &str) -> String {
    json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/keys"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "code_challenge_methods_supported": ["S256"]
    })
    .to_string()
}

/// One RSA public key whose modulus is 2^2048 - 1: well formed, and the
/// public half of no key pair.
fn keys_document() -> String {
    json!({"keys": [{
        "kty": "RSA", "kid": "recorder-1", "use": "sig", "alg": "RS256",
        "n": format!("{}w", "_".repeat(341)), "e": "AQAB"
    }]})
    .to_string()
}

fn malformed(what: &'static str) -> Error {
    Error::new(ErrorKind::InvalidData, what)
}

fn exchange(
    mut stream: TcpStream,
    issuer: &str,
    requests: &Mutex<Vec<RecordedRequest>>,
) -> Result<()> {
    stream.set_read_timeout(Some(EXCHANGE_BOUND))?;
    stream.set_write_timeout(Some(EXCHANGE_BOUND))?;
    let mut received = Vec::new();
    let mut buffer = [0_u8; 4096];
    let head_end = loop {
        if let Some(end) = received.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        if received.len() > MAX_HEAD_BYTES {
            return Err(malformed("the head of the request is too large"));
        }
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            // A connection that sends nothing: the wake of `drop`.
            return Err(ErrorKind::UnexpectedEof.into());
        }
        received.extend_from_slice(&buffer[..count]);
    };
    let head = std::str::from_utf8(&received[..head_end])
        .map_err(|_| malformed("the head of the request is not UTF-8"))?;
    let mut lines = head.split("\r\n");
    let line = lines.next().unwrap_or_default().to_owned();
    let mut header_names = Vec::new();
    let mut declared_body = 0_usize;
    for header in lines {
        let (name, value) = header
            .split_once(':')
            .ok_or_else(|| malformed("a header line has no name"))?;
        let name = name.trim().to_ascii_lowercase();
        if name == "content-length" {
            declared_body = value
                .trim()
                .parse()
                .map_err(|_| malformed("the declared length is not a number"))?;
        }
        header_names.push(name);
    }
    if declared_body > MAX_BODY_BYTES {
        return Err(malformed("the declared body is too large"));
    }
    let mut body_bytes = received.len() - (head_end + 4);
    while body_bytes < declared_body {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        body_bytes += count;
    }
    // Whatever the method: a request that should not have been sent is
    // answered too, and stays in the record.
    let (status, body) = match line
        .split(' ')
        .nth(1)
        .and_then(|target| target.strip_prefix(REALM))
    {
        Some("/.well-known/openid-configuration") => ("200 OK", discovery_document(issuer)),
        Some("/keys") => ("200 OK", keys_document()),
        _ => ("404 Not Found", "{}".to_owned()),
    };
    requests.lock().expect("the record").push(RecordedRequest {
        line,
        header_names,
        body_bytes,
    });
    write!(
        stream,
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()?;
    stream.shutdown(Shutdown::Both)
}
