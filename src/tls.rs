//! Bounded TLS material and listener for the trusted model transport. No
//! authentication, tenant authority, system trust installation or automatic reload.
use std::{
    future::Future,
    net::SocketAddr,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{Notify, OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
    time::{Instant, Sleep},
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
    server::TlsStream,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TlsError;
impl std::fmt::Display for TlsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TLS material or configuration is invalid or unavailable")
    }
}
impl std::error::Error for TlsError {}

const MAX_CERT_BYTES: usize = 65_536;
const MAX_KEY_BYTES: usize = 16_384;

/// Accept only the declared PEM sections and whitespace. PemObject's normal
/// iterator skips unrelated sections, which is unsuitable for a trust file.
fn sections<'a>(bytes: &'a [u8], labels: &[&str], limit: usize) -> Result<Vec<&'a [u8]>, TlsError> {
    if bytes.is_empty() || bytes.len() > limit || !bytes.is_ascii() {
        return Err(TlsError);
    }
    let mut remaining = bytes;
    let mut blocks = Vec::new();
    while !remaining.is_empty() {
        remaining = remaining.trim_ascii_start();
        if remaining.is_empty() {
            break;
        }
        let label = labels
            .iter()
            .find(|label| remaining.starts_with(format!("-----BEGIN {label}-----").as_bytes()))
            .ok_or(TlsError)?;
        let begin = format!("-----BEGIN {label}-----");
        if !matches!(remaining.get(begin.len()), Some(b'\n' | b'\r')) {
            return Err(TlsError);
        }
        let end = format!("-----END {label}-----");
        let offset = remaining
            .windows(end.len())
            .position(|value| value == end.as_bytes())
            .ok_or(TlsError)?;
        let body = remaining.get(begin.len()..offset).ok_or(TlsError)?;
        // The typed PEM iterator can reset at a nested BEGIN and skip a
        // different section. Pass it exactly one base64-only body instead:
        // no marker, comment or other material can be silently discarded.
        if !body.iter().any(|byte| !byte.is_ascii_whitespace())
            || body.iter().any(|byte| {
                !(byte.is_ascii_alphanumeric()
                    || byte.is_ascii_whitespace()
                    || matches!(byte, b'+' | b'/' | b'='))
            })
        {
            return Err(TlsError);
        }
        let boundary = offset + end.len();
        if remaining
            .get(boundary)
            .is_some_and(|byte| !byte.is_ascii_whitespace())
        {
            return Err(TlsError);
        }
        blocks.push(&remaining[..boundary]);
        if blocks.len() > 8 {
            return Err(TlsError);
        }
        remaining = &remaining[boundary..];
    }
    if blocks.is_empty() {
        return Err(TlsError);
    }
    Ok(blocks)
}

pub(crate) fn certificates(bytes: &[u8]) -> Result<Vec<CertificateDer<'static>>, TlsError> {
    let mut store = rustls::RootCertStore::empty();
    sections(bytes, &["CERTIFICATE"], MAX_CERT_BYTES)?
        .into_iter()
        .map(|block| {
            let certificate = CertificateDer::from_pem_slice(block).map_err(|_| TlsError)?;
            store.add(certificate.clone()).map_err(|_| TlsError)?;
            Ok(certificate)
        })
        .collect()
}
pub(crate) fn read_material(path: &Path, maximum: usize) -> Result<Vec<u8>, TlsError> {
    crate::control_fs::read_trusted_projected_file(path, maximum as u64).map_err(|_| TlsError)
}

#[derive(Clone)]
pub struct ServerIdentity(Arc<rustls::ServerConfig>);
impl ServerIdentity {
    pub fn from_pem(chain: &[u8], key: &[u8]) -> Result<Self, TlsError> {
        let chain = certificates(chain)?;
        let key_blocks = sections(
            key,
            &["PRIVATE KEY", "RSA PRIVATE KEY", "EC PRIVATE KEY"],
            MAX_KEY_BYTES,
        )?;
        if key_blocks.len() != 1 {
            return Err(TlsError);
        }
        let key = PrivateKeyDer::from_pem_slice(key_blocks[0]).map_err(|_| TlsError)?;
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| TlsError)?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|_| TlsError)?;
        Ok(Self(Arc::new(config)))
    }
    pub fn from_files(chain: &Path, key: &Path) -> Result<Self, TlsError> {
        Self::from_pem(
            &read_material(chain, MAX_CERT_BYTES)?,
            &read_material(key, MAX_KEY_BYTES)?,
        )
    }
}

struct ForcedClose {
    closed: AtomicBool,
    changed: Arc<Notify>,
}
struct CloseOnDrop(Arc<ForcedClose>);
impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::Release);
        self.0.changed.notify_waiters();
    }
}
impl ForcedClose {
    async fn wait(&self) {
        let changed = self.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if !self.closed.load(Ordering::Acquire) {
            changed.await;
        }
    }
}
async fn bound_exchange(
    axum::extract::State(force): axum::extract::State<Arc<ForcedClose>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    tokio::select! {
        _=force.wait()=>axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response(),
        response=next.run(request)=>response,
    }
}
type Closed = std::pin::Pin<Box<tokio::sync::futures::OwnedNotified>>;
struct Connection {
    stream: TlsStream<TcpStream>,
    _permit: OwnedSemaphorePermit,
    idle: std::pin::Pin<Box<Sleep>>,
    force: Arc<ForcedClose>,
    changed: Closed,
}
impl Connection {
    fn check(&mut self, cx: &mut std::task::Context<'_>) -> std::io::Result<()> {
        if self.force.closed.load(Ordering::Acquire) {
            return Err(std::io::ErrorKind::ConnectionAborted.into());
        }
        if self.changed.as_mut().poll(cx).is_ready() || self.idle.as_mut().poll(cx).is_ready() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        if self.force.closed.load(Ordering::Acquire) {
            return Err(std::io::ErrorKind::ConnectionAborted.into());
        }
        Ok(())
    }
    fn activity(&mut self) {
        self.idle
            .as_mut()
            .reset(Instant::now() + Duration::from_secs(30));
    }
}
impl AsyncRead for Connection {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if let Err(error) = self.check(cx) {
            return std::task::Poll::Ready(Err(error));
        }
        let before = buffer.filled().len();
        let result = std::pin::Pin::new(&mut self.stream).poll_read(cx, buffer);
        if result.is_ready() && buffer.filled().len() > before {
            self.activity();
        }
        result
    }
}
impl AsyncWrite for Connection {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        if let Err(error) = self.check(cx) {
            return std::task::Poll::Ready(Err(error));
        }
        let result = std::pin::Pin::new(&mut self.stream).poll_write(cx, bytes);
        if matches!(result, std::task::Poll::Ready(Ok(count)) if count > 0) {
            self.activity();
        }
        result
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if let Err(error) = self.check(cx) {
            return std::task::Poll::Ready(Err(error));
        }
        std::pin::Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if let Err(error) = self.check(cx) {
            return std::task::Poll::Ready(Err(error));
        }
        std::pin::Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
struct Listener {
    socket: TcpListener,
    acceptor: TlsAcceptor,
    connections: Arc<Semaphore>,
    handshakes: Arc<Semaphore>,
    pending: JoinSet<Option<(Connection, SocketAddr)>>,
    force: Arc<ForcedClose>,
}
impl axum::serve::Listener for Listener {
    type Io = Connection;
    type Addr = SocketAddr;
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            tokio::select! {
                complete = self.pending.join_next(), if !self.pending.is_empty() => {
                    if let Some(Ok(Some(connection))) = complete { return connection; }
                }
                accepted = self.socket.accept() => {
                    let Ok((stream, address)) = accepted else { tokio::time::sleep(Duration::from_millis(100)).await; continue; };
                    let Ok(permit) = self.connections.clone().try_acquire_owned() else { continue; };
                    let Ok(handshake) = self.handshakes.clone().try_acquire_owned() else { continue; };
                    let acceptor = self.acceptor.clone();
                    let force = self.force.clone();
                    self.pending.spawn(async move {
                        let result = tokio::time::timeout(Duration::from_secs(2), acceptor.accept(stream)).await;
                        drop(handshake);
                        let Ok(Ok(stream)) = result else { return None; };
                        Some((Connection { stream, _permit: permit, idle: Box::pin(tokio::time::sleep(Duration::from_secs(30))), changed: Box::pin(force.changed.clone().notified_owned()), force }, address))
                    });
                }
            }
        }
    }
    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.socket.local_addr()
    }
}

/// Owns pending handshakes and forces active IO closed after a bounded drain.
/// The material is retained in memory; changing its source files is not reload.
pub async fn serve(
    listener: TcpListener,
    router: axum::Router,
    identity: ServerIdentity,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    use futures_util::FutureExt;
    let force = Arc::new(ForcedClose {
        closed: AtomicBool::new(false),
        changed: Arc::new(Notify::new()),
    });
    let _close_on_drop = CloseOnDrop(force.clone());
    let router = router.layer(axum::middleware::from_fn_with_state(
        force.clone(),
        bound_exchange,
    ));
    let listener = Listener {
        socket: listener,
        acceptor: TlsAcceptor::from(identity.0),
        connections: Arc::new(Semaphore::new(32)),
        handshakes: Arc::new(Semaphore::new(8)),
        pending: JoinSet::new(),
        force: force.clone(),
    };
    let shutdown = shutdown.shared();
    let axum_shutdown = {
        let shutdown = shutdown.clone();
        let force = force.clone();
        async move {
            // Axum owns a forwarding task for this signal. It must also
            // finish when this serving owner is dropped, so its Shared
            // cannot retain the caller's Pending shutdown/resources forever.
            tokio::select! { _ = shutdown => {}, _ = force.wait() => {} }
        }
    };
    let serving = axum::serve(listener, router)
        .with_graceful_shutdown(axum_shutdown)
        .into_future();
    tokio::pin!(serving);
    tokio::select! {
        result = &mut serving => result,
        _ = shutdown => {
            match tokio::time::timeout(Duration::from_secs(5), &mut serving).await {
                Ok(result) => result,
                Err(_) => {
                    force.closed.store(true, Ordering::Release);
                    force.changed.notify_waiters();
                    // Axum owns connection tasks; forced IO wakes their reads
                    // and writes. This future remains owned until they join.
                    serving.await
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
