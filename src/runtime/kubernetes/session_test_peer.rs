//! Raw TLS HTTP/1.1 loopback peer for the V2 session transport tests.
//!
//! One owned listener per runtime, one request per connection (the client
//! keeps no idle connection), bounded request parsing and no new dependency.
//! It can drop a connection after the full request without any response
//! byte. Its answers are declared fixture facts: they never qualify a
//! Kubernetes API server, admission, scheduling or deletion.
use super::admission::transport_tests::TlsMaterial;
use std::{
    io,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{PrivateKeyDer, pem::PemObject},
    },
};

const MAX_HEAD_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024;
const EXCHANGE_LIMIT: Duration = Duration::from_secs(5);

/// One received request, recorded before the reply is chosen.
#[derive(Clone, Debug)]
pub(crate) struct PeerRequest {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}
impl PeerRequest {
    pub(crate) fn method(&self) -> &str {
        &self.method
    }
    /// Origin-form path and query, exactly as received.
    pub(crate) fn target(&self) -> &str {
        &self.target
    }
    /// First value of a header, by lowercase name.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
    pub(crate) fn body(&self) -> &[u8] {
        &self.body
    }
    pub(crate) fn line(&self) -> String {
        format!("{} {}", self.method, self.target)
    }
}

pub(crate) enum PeerReply {
    Json {
        status: u16,
        body: Vec<u8>,
    },
    /// The whole request was read; the connection is dropped without any
    /// response byte, as after a lost answer.
    CloseWithoutResponse,
}
impl PeerReply {
    pub(crate) fn json(status: u16, value: &serde_json::Value) -> Self {
        Self::Json {
            status,
            body: serde_json::to_vec(value).expect("serialize a fixture reply"),
        }
    }
}

type Responder = Box<dyn FnMut(&PeerRequest) -> PeerReply + Send>;
struct PeerState {
    responder: Responder,
    seen: Vec<PeerRequest>,
    /// TCP connections accepted, counted before the TLS handshake and the
    /// request parsing.
    accepted: usize,
}

pub(crate) struct SessionTestPeer {
    endpoint: String,
    listener: std::net::TcpListener,
    material: TlsMaterial,
    state: Arc<Mutex<PeerState>>,
    task: JoinHandle<()>,
}
impl Drop for SessionTestPeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl SessionTestPeer {
    pub(crate) async fn start() -> Self {
        let material = TlsMaterial::create().await;
        let chain = crate::tls::certificates(
            &std::fs::read(&material.certificate).expect("read the fixture server chain"),
        )
        .expect("parse the fixture server chain");
        let key = PrivateKeyDer::from_pem_slice(
            &std::fs::read(&material.key).expect("read the fixture server key"),
        )
        .expect("parse the fixture server key");
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("select the fixture TLS versions")
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .expect("build the fixture TLS configuration");
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind a loopback-only listener");
        listener
            .set_nonblocking(true)
            .expect("make the owned listener nonblocking");
        let endpoint = format!(
            "https://{}",
            listener.local_addr().expect("read the bound address")
        );
        let accepting = TcpListener::from_std(
            listener
                .try_clone()
                .expect("retain the exact listener proof"),
        )
        .expect("adopt the bound listener");
        let state = Arc::new(Mutex::new(PeerState {
            responder: Box::new(|_: &PeerRequest| {
                PeerReply::json(404, &serde_json::json!({"kind":"Status","code":404}))
            }),
            seen: Vec::new(),
            accepted: 0,
        }));
        let task = tokio::spawn(serve(accepting, acceptor, state.clone()));
        Self {
            endpoint,
            listener,
            material,
            state,
            task,
        }
    }
    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }
    /// The exact bound listener, as the owned-listener proof of test guards.
    pub(crate) fn listener(&self) -> &std::net::TcpListener {
        &self.listener
    }
    /// Private fixture root CA; its directory is private to the fixture.
    pub(crate) fn ca(&self) -> &Path {
        &self.material.ca
    }
    pub(crate) fn respond(
        &self,
        responder: impl FnMut(&PeerRequest) -> PeerReply + Send + 'static,
    ) {
        self.state.lock().expect("peer state").responder = Box::new(responder);
    }
    pub(crate) fn requests(&self) -> Vec<PeerRequest> {
        self.state.lock().expect("peer state").seen.clone()
    }
    /// Connections accepted so far, counted before TLS and parsing: a
    /// connection whose request could not be parsed is counted here but is
    /// missing from `requests`.
    pub(crate) fn accepted(&self) -> usize {
        self.state.lock().expect("peer state").accepted
    }
}

async fn serve(listener: TcpListener, acceptor: TlsAcceptor, state: Arc<Mutex<PeerState>>) {
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
        };
        // Counted before TLS and parsing, so an unparsable request still shows.
        if let Ok(mut guard) = state.lock() {
            guard.accepted += 1;
        }
        // One bounded exchange at a time; a failed exchange drops its stream.
        let _ = tokio::time::timeout(EXCHANGE_LIMIT, exchange(&acceptor, stream, &state)).await;
    }
}

async fn exchange(
    acceptor: &TlsAcceptor,
    stream: TcpStream,
    state: &Mutex<PeerState>,
) -> io::Result<()> {
    let mut stream = acceptor.accept(stream).await?;
    let Some(request) = read_request(&mut stream).await? else {
        return Ok(());
    };
    let reply = {
        let mut guard = state
            .lock()
            .map_err(|_| io::Error::other("peer state is poisoned"))?;
        let state = &mut *guard;
        let reply = (state.responder)(&request);
        state.seen.push(request);
        reply
    };
    match reply {
        PeerReply::Json { status, body } => {
            let head = format!(
                "HTTP/1.1 {status} Fixture\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).await?;
            stream.write_all(&body).await?;
            stream.shutdown().await
        }
        PeerReply::CloseWithoutResponse => Ok(()),
    }
}

/// Bounded HTTP/1.1 request: origin-form target, `Content-Length` body only.
async fn read_request<S: AsyncRead + Unpin>(stream: &mut S) -> io::Result<Option<PeerRequest>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let end = loop {
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        if buffer.len() > MAX_HEAD_BYTES {
            return Err(io::Error::other("oversized request head"));
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = std::str::from_utf8(&buffer[..end])
        .map_err(|_| io::Error::other("request head is not UTF-8"))?;
    let mut lines = head.split("\r\n");
    let mut start = lines.next().unwrap_or_default().split(' ');
    let (Some(method), Some(target), Some("HTTP/1.1"), None) =
        (start.next(), start.next(), start.next(), start.next())
    else {
        return Err(io::Error::other("invalid request line"));
    };
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| io::Error::other("invalid header line"))?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
    }
    if headers.iter().any(|(name, _)| name == "transfer-encoding") {
        return Err(io::Error::other("chunked request bodies are refused"));
    }
    let length = match headers.iter().find(|(name, _)| name == "content-length") {
        None => 0,
        Some((_, value)) => value
            .parse::<usize>()
            .map_err(|_| io::Error::other("invalid content-length"))?,
    };
    if length > MAX_BODY_BYTES {
        return Err(io::Error::other("oversized request body"));
    }
    let mut body = buffer[end + 4..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        body.extend_from_slice(&chunk[..read]);
    }
    if body.len() != length {
        return Err(io::Error::other("bytes beyond the declared body"));
    }
    Ok(Some(PeerRequest {
        method: method.to_owned(),
        target: target.to_owned(),
        headers,
        body,
    }))
}
