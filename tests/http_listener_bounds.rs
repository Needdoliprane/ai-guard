//! Bounded, real loopback sockets only. No saturation, identities or remote service.
use agentsandbox::http_listener_bounds::{ListenerBounds, serve};
use axum::{Router, body::Body, routing::get};
use std::{io, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, oneshot},
    task::JoinHandle,
};

struct Server {
    address: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<io::Result<()>>>,
}
impl Server {
    async fn start(router: Router, bounds: ListenerBounds) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(serve(
            listener,
            router,
            async move {
                let _ = stopped.await;
            },
            bounds,
        ));
        Self {
            address,
            stop: Some(stop),
            task: Some(task),
        }
    }
    fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
    async fn finish(mut self) {
        self.stop();
        let joined =
            tokio::time::timeout(Duration::from_secs(2), self.task.as_mut().unwrap()).await;
        if joined.is_err() {
            let task = self.task.as_mut().unwrap();
            task.abort();
            let _ = task.await;
            self.task.take();
            panic!(
                "fixture drain deadline expired; serving owner aborted and joined, no drain success claimed"
            );
        }
        self.task.take();
        joined.unwrap().unwrap().unwrap();
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        // Failed assertions request cancellation of the retained fixture owner.
        // No successful drain is inferred; successful tests await finish/join.
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn request(stream: &mut TcpStream, close: bool) {
    tokio::time::timeout(
        Duration::from_secs(1),
        stream.write_all(if close {
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        } else {
            b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"
        }),
    )
    .await
    .unwrap()
    .unwrap();
}
async fn headers(stream: &mut TcpStream) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(1), async {
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            assert!(bytes.len() < 8192, "fixture response headers exceed bound");
            bytes.push(stream.read_u8().await.unwrap());
        }
        assert!(bytes.starts_with(b"HTTP/1.1 200"));
        bytes
    })
    .await
    .unwrap()
}
async fn ok(stream: &mut TcpStream) {
    tokio::time::timeout(Duration::from_secs(1), async {
        let head = headers(stream).await;
        assert!(String::from_utf8(head).unwrap().lines().any(|line| {
            line.split_once(':').is_some_and(|(key, value)| {
                key.eq_ignore_ascii_case("content-length") && value.trim() == "2"
            })
        }));
        let mut body = [0; 2];
        stream.read_exact(&mut body).await.unwrap();
        assert_eq!(&body, b"ok");
    })
    .await
    .unwrap();
}

#[test]
fn bounds_reject_disabled_or_unrepresentable_limits() {
    assert!(ListenerBounds::new(0, Duration::from_secs(1)).is_err());
    assert!(ListenerBounds::new(1, Duration::ZERO).is_err());
    assert!(ListenerBounds::new(usize::MAX, Duration::from_secs(1)).is_err());
    assert!(ListenerBounds::new(1, Duration::MAX).is_err());
}

#[tokio::test]
async fn completed_response_keeps_capacity_until_the_keepalive_io_closes() {
    let server = Server::start(
        Router::new().route("/", get(|| async { "ok" })),
        ListenerBounds::new(1, Duration::from_secs(2)).unwrap(),
    )
    .await;
    let mut first = TcpStream::connect(server.address).await.unwrap();
    request(&mut first, false).await;
    tokio::time::timeout(Duration::from_secs(1), ok(&mut first))
        .await
        .unwrap();
    // The first response is complete, but its real TCP IO remains open. A lease
    // tied to a response/handler instead of the IO would admit this second GET.
    let mut second = TcpStream::connect(server.address).await.unwrap();
    request(&mut second, true).await;
    let mut byte = [0];
    assert!(
        tokio::time::timeout(Duration::from_millis(50), second.read(&mut byte))
            .await
            .is_err()
    );
    drop(first);
    tokio::time::timeout(Duration::from_secs(1), ok(&mut second))
        .await
        .unwrap();
    drop(second);
    server.finish().await;
}

#[tokio::test]
async fn incomplete_first_headers_expire_without_running_a_handler_then_admit_a_real_get() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let server = Server::start(
        Router::new().route(
            "/",
            get(move || {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async { "ok" }
            }),
        ),
        ListenerBounds::new(1, Duration::from_millis(200)).unwrap(),
    )
    .await;
    let mut partial = TcpStream::connect(server.address).await.unwrap();
    partial.write_all(b"GET / HTTP/1.1\r\nHost:").await.unwrap();
    let mut bytes = [0; 256];
    assert!(
        tokio::time::timeout(Duration::from_millis(40), partial.read(&mut bytes))
            .await
            .is_err()
    );
    // A timeout response or EOF are both transport refusal. Neither calls the
    // route or creates an authenticated request. Read the actual close to EOF.
    let mut refusal = Vec::new();
    match tokio::time::timeout(Duration::from_secs(1), partial.read_to_end(&mut refusal))
        .await
        .unwrap()
    {
        Ok(_) => {}
        // Dropping a timed-out socket whose bytes were still unread can reset
        // TCP. This is a transport refusal too; other I/O faults remain failures.
        Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {}
        Err(error) => panic!("unexpected partial-header refusal: {error}"),
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    let mut legitimate = TcpStream::connect(server.address).await.unwrap();
    request(&mut legitimate, true).await;
    tokio::time::timeout(Duration::from_secs(1), ok(&mut legitimate))
        .await
        .unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    drop(legitimate);
    drop(partial);
    server.finish().await;
}

#[tokio::test]
async fn legitimate_keepalive_serves_two_requests_on_the_same_socket() {
    let server = Server::start(
        Router::new().route("/", get(|| async { "ok" })),
        ListenerBounds::new(1, Duration::from_secs(2)).unwrap(),
    )
    .await;
    let mut client = TcpStream::connect(server.address).await.unwrap();
    request(&mut client, false).await;
    ok(&mut client).await;
    request(&mut client, true).await;
    ok(&mut client).await;
    drop(client);
    server.finish().await;
}

#[tokio::test]
async fn shutdown_waits_for_the_in_flight_response_and_its_connection_join() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (inside, gate) = (entered.clone(), release.clone());
    let mut server = Server::start(
        Router::new().route(
            "/",
            get(move || {
                let (inside, gate) = (inside.clone(), gate.clone());
                async move {
                    inside.notify_one();
                    gate.notified().await;
                    "ok"
                }
            }),
        ),
        ListenerBounds::new(1, Duration::from_secs(2)).unwrap(),
    )
    .await;
    let mut client = TcpStream::connect(server.address).await.unwrap();
    request(&mut client, true).await;
    tokio::time::timeout(Duration::from_secs(1), entered.notified())
        .await
        .unwrap();
    server.stop();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), server.task.as_mut().unwrap())
            .await
            .is_err()
    );
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(1), ok(&mut client))
        .await
        .unwrap();
    drop(client);
    server.finish().await;
}

#[tokio::test]
async fn a_live_streaming_body_survives_the_initial_header_deadline() {
    let release = Arc::new(Notify::new());
    let gate = release.clone();
    let server = Server::start(
        Router::new().route(
            "/",
            get(move || {
                let gate = gate.clone();
                async move {
                    Body::from_stream(async_stream::stream! {
                        yield Ok::<_, io::Error>("data: one\n\n");
                        gate.notified().await;
                        yield Ok::<_, io::Error>("data: two\n\n");
                    })
                }
            }),
        ),
        ListenerBounds::new(1, Duration::from_millis(100)).unwrap(),
    )
    .await;
    let mut client = TcpStream::connect(server.address).await.unwrap();
    request(&mut client, true).await;
    tokio::time::timeout(Duration::from_secs(1), headers(&mut client))
        .await
        .unwrap();
    let mut first = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !first.windows(11).any(|w| w == b"data: one\n\n") {
            first.push(client.read_u8().await.unwrap());
            assert!(first.len() < 256);
        }
    })
    .await
    .unwrap();
    let mut chunk_end = [0; 2];
    tokio::time::timeout(Duration::from_secs(1), client.read_exact(&mut chunk_end))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&chunk_end, b"\r\n");
    let mut next = [0; 1];
    assert!(
        tokio::time::timeout(Duration::from_millis(150), client.read(&mut next))
            .await
            .is_err()
    );
    release.notify_one();
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), client.read_to_end(&mut rest))
        .await
        .unwrap()
        .unwrap();
    assert!(rest.windows(11).any(|w| w == b"data: two\n\n"));
    drop(client);
    server.finish().await;
}
