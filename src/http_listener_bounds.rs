//! Plain HTTP admission before routing. No identity or authority is created here.
//!
//! First-request timing includes protocol detection. HTTP/1 subsequent header
//! timing is delegated to Hyper with a real timer. New HTTP/2 streams after the
//! first request are NOT covered by that header timeout. Bodies/SSE keep their
//! existing route policies. Listener count is not a tenant or workload quota.
use axum::{
    Router,
    extract::Request,
    middleware::{self, Next},
    response::IntoResponse,
};
use hyper_util::{
    rt::{TokioExecutor, TokioIo, TokioTimer},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{Notify, OwnedSemaphorePermit, Semaphore, watch},
    task::JoinSet,
    time::Instant,
};

/// Trusted listener policy, validated without installing a listener or task.
/// Callers choose each listener separately; there is no unbounded/disabled mode.
#[derive(Clone, Copy, Debug)]
pub struct ListenerBounds {
    max_connections: usize,
    first_header_timeout: Duration,
}
impl ListenerBounds {
    pub fn new(max_connections: usize, first_header_timeout: Duration) -> io::Result<Self> {
        if max_connections == 0 || max_connections > Semaphore::MAX_PERMITS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid HTTP listener connection bound",
            ));
        }
        if first_header_timeout.is_zero()
            || Instant::now().checked_add(first_header_timeout).is_none()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid HTTP listener header deadline",
            ));
        }
        Ok(Self {
            max_connections,
            first_header_timeout,
        })
    }
}

// No Clone, raw-stream getter, split or permit extraction. Rust drops fields in
// declaration order: first the actual TCP FD, then the capacity lease. An HTTP
// upgrade carries this WHOLE IO, so finishing a response/task cannot release its
// capacity while an upgraded IO is retained elsewhere.
struct LeasedIo {
    stream: TcpStream,
    _permit: OwnedSemaphorePermit,
}
impl AsyncRead for LeasedIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}
impl AsyncWrite for LeasedIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write_vectored(cx, bytes)
    }
}

struct FirstHead {
    deadline: Instant,
    complete: AtomicBool,
    changed: Notify,
}
impl FirstHead {
    fn observe(&self) -> bool {
        if self.complete.load(Ordering::Acquire) {
            return true;
        }
        if Instant::now() >= self.deadline {
            return false;
        }
        self.complete.store(true, Ordering::Release);
        // There is only one connection waiter. notify_one retains a notification
        // if marking happens before the waiter is polled.
        self.changed.notify_one();
        true
    }
    async fn observed(&self) {
        while !self.complete.load(Ordering::Acquire) {
            self.changed.notified().await;
        }
    }
}

/// Internal socket ownership only. This is not an authentication/admission proof.
pub(crate) struct AcceptedConnection {
    io: LeasedIo,
    accepted: Instant,
    bounds: ListenerBounds,
}

/// One admission owner per listener, shared by all of its accept attempts.
pub(crate) struct ConnectionAdmission {
    slots: Arc<Semaphore>,
    bounds: ListenerBounds,
}
impl ConnectionAdmission {
    pub(crate) fn new(bounds: ListenerBounds) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(bounds.max_connections)),
            bounds,
        }
    }
    pub(crate) async fn accept(
        &self,
        listener: &mut TcpListener,
    ) -> io::Result<AcceptedConnection> {
        let permit = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| io::Error::other("HTTP listener capacity closed"))?;
        // Preserve the existing Axum accept error/retry policy. A full listener
        // waits BEFORE accept; kernel backlog sockets are not application-owned.
        let (stream, _) = axum::serve::Listener::accept(listener).await;
        Ok(AcceptedConnection {
            io: LeasedIo {
                stream,
                _permit: permit,
            },
            accepted: Instant::now(),
            bounds: self.bounds,
        })
    }
}

pub(crate) async fn serve_connection(
    connection: AcceptedConnection,
    router: Router,
    mut stopping: watch::Receiver<bool>,
) {
    let AcceptedConnection {
        io,
        accepted,
        bounds,
    } = connection;
    let Some(deadline) = accepted.checked_add(bounds.first_header_timeout) else {
        return;
    };
    let first = Arc::new(FirstHead {
        deadline,
        complete: AtomicBool::new(false),
        changed: Notify::new(),
    });
    let signal = first.clone();
    let router = router.layer(middleware::from_fn(move |request: Request, next: Next| {
        let signal = signal.clone();
        async move {
            // Pure transport observation. A parsed request after the original
            // deadline never reaches any business/authentication handler.
            if !signal.observe() {
                return axum::http::StatusCode::REQUEST_TIMEOUT.into_response();
            }
            next.run(request).await
        }
    }));
    let mut builder = Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(Some(bounds.first_header_timeout));
    // Same auto-protocol/upgrades entry point used by the locked Axum serve.
    let service = TowerToHyperService::new(router);
    let serving = builder.serve_connection_with_upgrades(TokioIo::new(io), service);
    tokio::pin!(serving);
    let first_deadline = tokio::time::sleep_until(deadline);
    tokio::pin!(first_deadline);
    let mut head_complete = false;
    let mut graceful = false;
    loop {
        tokio::select! {
            biased;
            _ = stopping.wait_for(|stopped| *stopped), if !graceful => {
                graceful = true;
                serving.as_mut().graceful_shutdown();
            }
            _ = &mut first_deadline, if !head_complete => {
                // An already observed, timely head wins even if this timer was
                // polled later. No activity refreshes the original Instant.
                if first.complete.load(Ordering::Acquire) { head_complete = true; }
                else { return; }
            }
            _ = first.observed(), if !head_complete => { head_complete = true; }
            _ = &mut serving => { return; }
        }
    }
}

/// Serve the existing Router with listener-owned capacity and initial timing.
/// On shutdown: stop accepting, request graceful close, await owned task joins.
/// The caller retains its existing outer drain deadline/process teardown. If
/// this future is cancelled, JoinSet aborts owned tasks; cancellation is NOT a
/// join or proof of closure. The permit still follows each actual IO/upgrade.
pub async fn serve(
    mut listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send,
    bounds: ListenerBounds,
) -> io::Result<()> {
    let admission = ConnectionAdmission::new(bounds);
    let (stop, _) = watch::channel(false);
    let mut tasks = JoinSet::<()>::new();
    let mut task_failed = false;
    let mut accept_failure = None;
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => { break; }
            joined = tasks.join_next(), if !tasks.is_empty() => {
                if joined.is_some_and(|result| result.is_err()) { task_failed = true; }
            }
            accepted = admission.accept(&mut listener) => {
                match accepted {
                    Ok(connection) => { tasks.spawn(serve_connection(connection, router.clone(), stop.subscribe())); }
                    Err(error) => { accept_failure = Some(error); break; }
                }
            }
        }
    }
    drop(listener);
    let _ = stop.send(true);
    while let Some(joined) = tasks.join_next().await {
        if joined.is_err() {
            task_failed = true;
        }
    }
    if let Some(error) = accept_failure {
        return Err(error);
    }
    if task_failed {
        return Err(io::Error::other("HTTP connection task failed"));
    }
    Ok(())
}

#[cfg(test)]
mod lease_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn cancelled_join_waiter_keeps_capacity_until_real_io_drop_and_task_join() {
        let mut listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut peer = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let admission =
            ConnectionAdmission::new(ListenerBounds::new(1, Duration::from_secs(1)).unwrap());
        let AcceptedConnection { io, .. } = admission.accept(&mut listener).await.unwrap();
        let slots = admission.slots.clone();
        let entered = Arc::new(Notify::new());
        let inside = entered.clone();
        let (_release, wait) = tokio::sync::oneshot::channel::<()>();
        let mut tasks = JoinSet::new();
        tasks.spawn(async move {
            let mut io = io;
            inside.notify_one();
            let _ = wait.await;
            io.flush().await
        });
        entered.notified().await;
        let cancelled_waiter = tasks.join_next();
        drop(cancelled_waiter);
        assert!(
            slots.clone().try_acquire_owned().is_err(),
            "dropping a waiter is not an IO drain"
        );
        tasks.abort_all();
        while let Some(joined) = tasks.join_next().await {
            assert!(joined.unwrap_err().is_cancelled());
        }
        let mut byte = [0];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), peer.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        assert!(
            slots.try_acquire_owned().is_ok(),
            "capacity returns only after the held IO was dropped"
        );
    }
}
