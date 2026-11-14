//! Raw TLS HTTP/1.1 loopback peer for the V2 session transport tests.
//!
//! One owned listener per runtime, one request per connection (the client
//! keeps no idle connection), bounded request parsing and no new dependency.
//! It can drop a connection after the full request without any response
//! byte. For the attach tests it can also answer a head with chosen headers
//! and hand the raw TLS stream to a test script, served by its own task so
//! that the listener keeps answering while that stream is open. Its answers
//! are declared fixture facts: they never qualify a Kubernetes API server,
//! admission, scheduling, deletion, an attach endpoint or a kubelet stream.
use super::admission::transport_tests::TlsMaterial;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    future::Future,
    io,
    path::Path,
    pin::Pin,
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

/// The server side of one accepted connection after its request was read.
pub(crate) type PeerStream = tokio_rustls::server::TlsStream<TcpStream>;
pub(crate) type PeerScriptFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
pub(crate) type PeerScript = Box<dyn FnOnce(PeerStream) -> PeerScriptFuture + Send>;

pub(crate) enum PeerReply {
    Json {
        status: u16,
        body: Vec<u8>,
    },
    /// The whole request was read; the connection is dropped without any
    /// response byte, as after a lost answer.
    CloseWithoutResponse,
    /// A head with exactly these headers and no body, then the connection is
    /// closed.
    Head {
        status: u16,
        headers: Vec<(String, String)>,
    },
    /// These head bytes (possibly none), then the raw TLS stream goes to
    /// `script`, served by its own task.
    Handoff {
        head: Vec<u8>,
        script: PeerScript,
    },
}
impl PeerReply {
    pub(crate) fn json(status: u16, value: &serde_json::Value) -> Self {
        Self::Json {
            status,
            body: serde_json::to_vec(value).expect("serialize a fixture reply"),
        }
    }
    pub(crate) fn head(status: u16, headers: &[(&str, &str)]) -> Self {
        Self::Head {
            status,
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
        }
    }
    /// `101 Switching Protocols` with exactly these headers, then `script`
    /// owns the stream.
    pub(crate) fn switching<F, Fut>(headers: &[(&str, &str)], script: F) -> Self
    where
        F: FnOnce(PeerStream) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        Self::Handoff {
            head: response_head(101, "Switching Protocols", &headers, ""),
            script: Box::new(move |stream: PeerStream| -> PeerScriptFuture {
                Box::pin(script(stream))
            }),
        }
    }
    /// The request is read and never answered: the connection stays open
    /// until the client closes it or the peer is dropped.
    pub(crate) fn never_answer() -> Self {
        Self::Handoff {
            head: Vec::new(),
            script: Box::new(|mut stream: PeerStream| -> PeerScriptFuture {
                Box::pin(async move {
                    let mut sink = [0_u8; 64];
                    while matches!(stream.read(&mut sink).await, Ok(count) if count > 0) {}
                })
            }),
        }
    }
}

fn response_head(
    status: u16,
    reason: &str,
    headers: &[(String, String)],
    framing: &str,
) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
    for (name, value) in headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str(framing);
    head.push_str("\r\n");
    head.into_bytes()
}

type Responder = Box<dyn FnMut(&PeerRequest) -> PeerReply + Send>;
struct PeerState {
    responder: Responder,
    seen: Vec<PeerRequest>,
    /// TCP connections accepted, counted before the TLS handshake and the
    /// request parsing.
    accepted: usize,
    /// Tasks of the handed-off streams, kept so that none runs unobserved.
    scripts: Vec<JoinHandle<()>>,
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
        if let Ok(mut state) = self.state.lock() {
            for script in state.scripts.drain(..) {
                script.abort();
            }
        }
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
            scripts: Vec::new(),
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
    /// Waits for every handed-off script started so far and re-raises a
    /// script panic in the test. A script that is still running after the
    /// exchange limit fails the test instead of being left unobserved.
    pub(crate) async fn finish_scripts(&self) {
        let scripts = std::mem::take(&mut self.state.lock().expect("peer state").scripts);
        for script in scripts {
            tokio::time::timeout(EXCHANGE_LIMIT, script)
                .await
                .expect("a peer script is still running")
                .expect("a peer script panicked or was cancelled");
        }
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
        PeerReply::Head { status, headers } => {
            let head = response_head(
                status,
                "Fixture",
                &headers,
                "content-length: 0\r\nconnection: close\r\n",
            );
            stream.write_all(&head).await?;
            stream.shutdown().await
        }
        PeerReply::Handoff { head, script } => {
            if !head.is_empty() {
                stream.write_all(&head).await?;
                stream.flush().await?;
            }
            // Its own task: this exchange ends here and the listener keeps
            // answering while the script holds the stream.
            let task = tokio::spawn(script(stream));
            state
                .lock()
                .map_err(|_| io::Error::other("peer state is poisoned"))?
                .scripts
                .push(task);
            Ok(())
        }
    }
}

/// `Sec-WebSocket-Accept` of RFC 6455 section 4.2.2, written for the fixture
/// independently of the client code under test.
pub(crate) fn websocket_accept_value(key: &str) -> String {
    let joined = format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    STANDARD.encode(ring::digest::digest(
        &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
        joined.as_bytes(),
    ))
}

/// One unmasked server frame in the minimal length form. `first` is the raw
/// first byte (FIN, reserved bits and opcode), so a test can also build a
/// frame that the client must refuse.
pub(crate) fn server_frame(first: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![first];
    if let Ok(length) = u8::try_from(payload.len())
        && length <= 125
    {
        frame.push(length);
    } else if let Ok(length) = u16::try_from(payload.len()) {
        frame.push(126);
        frame.extend_from_slice(&length.to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(
            &u64::try_from(payload.len())
                .expect("a fixture payload length fits 64 bits")
                .to_be_bytes(),
        );
    }
    frame.extend_from_slice(payload);
    frame
}

/// One complete binary server message on `channel`.
pub(crate) fn server_message(channel: u8, payload: &[u8]) -> Vec<u8> {
    let mut message = vec![channel];
    message.extend_from_slice(payload);
    server_frame(0x82, &message)
}

/// One frame read from the client, unmasked.
pub(crate) struct ClientFrame {
    /// Raw first byte: FIN, reserved bits and opcode.
    pub(crate) first: u8,
    pub(crate) payload: Vec<u8>,
}
impl ClientFrame {
    pub(crate) fn opcode(&self) -> u8 {
        self.first & 0x0F
    }
    pub(crate) fn is_final(&self) -> bool {
        self.first & 0x80 != 0
    }
}

/// Reads one client frame. `Ok(None)` is the end of the stream before any
/// byte of a frame. A frame without the mask bit, with a reserved bit or in
/// the 64-bit length form is an error: the fixture accepts no unmasked client
/// frame, so every frame it returns proves that the client masked it.
pub(crate) async fn read_client_frame<S: AsyncRead + Unpin>(
    stream: &mut S,
) -> io::Result<Option<ClientFrame>> {
    let mut head = [0_u8; 2];
    if stream.read(&mut head[..1]).await? == 0 {
        return Ok(None);
    }
    stream.read_exact(&mut head[1..]).await?;
    if head[0] & 0x70 != 0 || head[1] & 0x80 == 0 {
        return Err(io::Error::other(
            "client frame without a mask or with a reserved bit",
        ));
    }
    let length = match head[1] & 0x7F {
        126 => {
            let mut extended = [0_u8; 2];
            stream.read_exact(&mut extended).await?;
            usize::from(u16::from_be_bytes(extended))
        }
        127 => return Err(io::Error::other("oversized client frame")),
        short => usize::from(short),
    };
    let mut key = [0_u8; 4];
    stream.read_exact(&mut key).await?;
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).await?;
    for (byte, mask) in payload.iter_mut().zip(key.iter().cycle()) {
        *byte ^= mask;
    }
    Ok(Some(ClientFrame {
        first: head[0],
        payload,
    }))
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
